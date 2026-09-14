//! D-12 summary timing and evidence capture.
//!
//! This module only records what the existing summary pipeline did. It does
//! not select models, prompts, languages, chunks, caches, or merge behavior.

use chrono::{DateTime, SecondsFormat, Utc};
use dashmap::DashMap;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use sysinfo::System;

pub const TIMED_ENDPOINT_ID: &str = "click_to_page_display_complete";
const MEASUREMENT_SCHEMA_VERSION: u32 = 1;
const RESOURCE_SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const STAGE_NAMES: [&str; 9] = [
    "wait_for_transcription",
    "prepare_input",
    "load_model",
    "chunk_summaries",
    "combine",
    "final_template",
    "translation",
    "save",
    "page_display_complete",
];

tokio::task_local! {
    static CURRENT_SUMMARY_GENERATION_ID: String;
}

static ACTIVE_MEASUREMENTS: Lazy<DashMap<String, Arc<MeasurementSession>>> =
    Lazy::new(DashMap::new);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementOutcome {
    InProgress,
    Completed,
    Failed,
    Cancelled,
    SaveFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageTiming {
    pub generation_id: String,
    pub stage: String,
    pub clock_domain: String,
    pub monotonic_start_ns: u64,
    pub monotonic_end_ns: Option<u64>,
    pub wall_started_at: String,
    pub wall_ended_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleEvent {
    pub generation_id: String,
    pub event: String,
    pub monotonic_offset_ns: u64,
    pub recorded_at: String,
    pub pids: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSample {
    pub generation_id: String,
    pub monotonic_offset_ns: u64,
    pub recorded_at: String,
    pub cpu_usage_percent: Option<f32>,
    pub gpu_usage_percent: Option<f64>,
    pub memory_commit_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryMeasurement {
    pub schema_version: u32,
    pub generation_id: String,
    pub meeting_id: String,
    pub timed_endpoint_id: String,
    pub click_at: String,
    pub click_monotonic_ns: u64,
    pub page_display_complete_at: Option<String>,
    pub page_display_complete_monotonic_ns: Option<u64>,
    pub stage_timings: Vec<StageTiming>,
    pub input_transcript_sha256: Option<String>,
    pub database_completed_body_sha256: Option<String>,
    pub page_displayed_body_sha256: Option<String>,
    pub page_matches_database: Option<bool>,
    pub helper_process_start_count: u64,
    pub job_distinct_pids: BTreeSet<u32>,
    pub model_loaded_event_count: u64,
    pub cleanup_entry_count: u64,
    pub graceful_shutdown_request_count: u64,
    pub job_confirm_zero_count: u64,
    pub lifecycle_events: Vec<LifecycleEvent>,
    pub load_timeline: Vec<ResourceSample>,
    pub load_timeline_path: String,
    pub load_timeline_sha256: Option<String>,
    pub baseline_non_moss_commit_peak_bytes: Option<u64>,
    pub peak_commit_ceiling_bytes: Option<u64>,
    pub outcome: MeasurementOutcome,
    pub outcome_reason: Option<String>,
    pub backend_save_completed: bool,
    pub eligible_for_performance: bool,
    pub performance_exclusion_reasons: Vec<String>,
}

struct MeasurementSession {
    origin: Instant,
    output_path: PathBuf,
    timeline_path: PathBuf,
    stopped: AtomicBool,
    record: Mutex<SummaryMeasurement>,
}

impl MeasurementSession {
    fn elapsed_ns(&self) -> u64 {
        self.origin.elapsed().as_nanos().min(u64::MAX as u128) as u64
    }

    fn persist(&self) -> Result<(), String> {
        let record = self
            .record
            .lock()
            .map_err(|_| "summary measurement lock poisoned".to_string())?
            .clone();
        let bytes = serde_json::to_vec_pretty(&record)
            .map_err(|error| format!("serialize summary measurement: {error}"))?;
        fs::write(&self.output_path, bytes)
            .map_err(|error| format!("write {}: {error}", self.output_path.display()))
    }

    fn add_resource_sample(&self, sample: ResourceSample) {
        if self.stopped.load(Ordering::SeqCst) {
            return;
        }
        if let Ok(mut record) = self.record.lock() {
            // Recheck while holding the same lock used for the terminal snapshot.
            // This keeps the embedded timeline equal to the JSONL bytes.
            if self.stopped.load(Ordering::SeqCst) {
                return;
            }
            record.load_timeline.push(sample);
        }
    }

    fn stop_and_hash_timeline(&self) -> Result<(), String> {
        self.stopped.store(true, Ordering::SeqCst);
        let samples = self
            .record
            .lock()
            .map_err(|_| "summary measurement lock poisoned".to_string())?
            .load_timeline
            .clone();
        let mut bytes = Vec::new();
        for sample in &samples {
            serde_json::to_writer(&mut bytes, sample)
                .map_err(|error| format!("serialize resource sample: {error}"))?;
            bytes.push(b'\n');
        }
        fs::write(&self.timeline_path, &bytes)
            .map_err(|error| format!("write {}: {error}", self.timeline_path.display()))?;
        let digest = sha256_bytes(&bytes);
        let mut record = self
            .record
            .lock()
            .map_err(|_| "summary measurement lock poisoned".to_string())?;
        record.load_timeline_sha256 = Some(digest);
        Ok(())
    }
}

pub struct StageGuard {
    generation_id: Option<String>,
    stage: &'static str,
    finished: bool,
}

impl StageGuard {
    pub fn finish(mut self) {
        if let Some(generation_id) = &self.generation_id {
            record_stage_end_for(generation_id, self.stage);
        }
        self.finished = true;
    }
}

impl Drop for StageGuard {
    fn drop(&mut self) {
        if !self.finished {
            if let Some(generation_id) = &self.generation_id {
                record_stage_end_for(generation_id, self.stage);
            }
        }
    }
}

pub struct TerminalOnDrop {
    generation_id: String,
    armed: bool,
}

impl TerminalOnDrop {
    pub fn new(generation_id: impl Into<String>) -> Self {
        Self {
            generation_id: generation_id.into(),
            armed: true,
        }
    }

    pub fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TerminalOnDrop {
    fn drop(&mut self) {
        if self.armed {
            let _ = finish_non_success(
                &self.generation_id,
                MeasurementOutcome::Failed,
                Some("summary_preflight_failed".to_string()),
            );
        }
    }
}

pub fn begin_measurement(
    meeting_folder: &Path,
    meeting_id: &str,
    generation_id: String,
    click_at: String,
    click_monotonic_ns: u64,
) -> Result<(), String> {
    DateTime::parse_from_rfc3339(&click_at)
        .map_err(|error| format!("clickAt must be RFC3339: {error}"))?;
    if meeting_id.trim().is_empty() {
        return Err("meetingId is required".to_string());
    }
    if generation_id.trim().is_empty()
        || !generation_id
            .chars()
            .all(|value| value.is_ascii_alphanumeric() || value == '_' || value == '-')
    {
        return Err("invalid measurement generation id".to_string());
    }

    let output_dir = meeting_folder.join("summary-measurements");
    fs::create_dir_all(&output_dir)
        .map_err(|error| format!("create {}: {error}", output_dir.display()))?;
    let output_path = output_dir.join(format!("{generation_id}.json"));
    let timeline_path = output_dir.join(format!("{generation_id}.load-timeline.jsonl"));
    if output_path.exists()
        || timeline_path.exists()
        || ACTIVE_MEASUREMENTS.contains_key(&generation_id)
    {
        return Err("measurement generation id already exists".to_string());
    }

    let record = SummaryMeasurement {
        schema_version: MEASUREMENT_SCHEMA_VERSION,
        generation_id: generation_id.clone(),
        meeting_id: meeting_id.to_string(),
        timed_endpoint_id: TIMED_ENDPOINT_ID.to_string(),
        click_at,
        click_monotonic_ns,
        page_display_complete_at: None,
        page_display_complete_monotonic_ns: None,
        stage_timings: Vec::new(),
        input_transcript_sha256: None,
        database_completed_body_sha256: None,
        page_displayed_body_sha256: None,
        page_matches_database: None,
        helper_process_start_count: 0,
        job_distinct_pids: BTreeSet::new(),
        model_loaded_event_count: 0,
        cleanup_entry_count: 0,
        graceful_shutdown_request_count: 0,
        job_confirm_zero_count: 0,
        lifecycle_events: Vec::new(),
        load_timeline: Vec::new(),
        load_timeline_path: timeline_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_string(),
        load_timeline_sha256: None,
        baseline_non_moss_commit_peak_bytes: None,
        peak_commit_ceiling_bytes: None,
        outcome: MeasurementOutcome::InProgress,
        outcome_reason: None,
        backend_save_completed: false,
        eligible_for_performance: false,
        performance_exclusion_reasons: vec!["measurement_in_progress".to_string()],
    };
    let session = Arc::new(MeasurementSession {
        origin: Instant::now(),
        output_path,
        timeline_path,
        stopped: AtomicBool::new(false),
        record: Mutex::new(record),
    });
    session.persist()?;
    ACTIVE_MEASUREMENTS.insert(generation_id, Arc::clone(&session));
    start_resource_sampler(session);
    Ok(())
}

pub fn claim_measurement(generation_id: &str, meeting_id: &str) -> Result<(), String> {
    let session = session(generation_id)?;
    let record = session
        .record
        .lock()
        .map_err(|_| "summary measurement lock poisoned".to_string())?;
    if record.meeting_id != meeting_id {
        return Err("measurement meeting id does not match".to_string());
    }
    if record.outcome != MeasurementOutcome::InProgress {
        return Err("measurement is already terminal".to_string());
    }
    Ok(())
}

pub async fn scope_generation<F>(generation_id: String, future: F) -> F::Output
where
    F: std::future::Future,
{
    CURRENT_SUMMARY_GENERATION_ID
        .scope(generation_id, future)
        .await
}

pub fn current_generation_id() -> Option<String> {
    CURRENT_SUMMARY_GENERATION_ID.try_with(Clone::clone).ok()
}

pub fn stage_guard(stage: &'static str) -> StageGuard {
    let generation_id = current_generation_id();
    if let Some(generation_id) = &generation_id {
        record_stage_start_for(generation_id, stage);
    }
    StageGuard {
        generation_id,
        stage,
        finished: false,
    }
}

pub fn stage_guard_for(generation_id: &str, stage: &'static str) -> StageGuard {
    record_stage_start_for(generation_id, stage);
    StageGuard {
        generation_id: Some(generation_id.to_string()),
        stage,
        finished: false,
    }
}

pub fn record_stage_start_for(generation_id: &str, stage: &str) {
    if !STAGE_NAMES.contains(&stage) {
        return;
    }
    if let Ok(session) = session(generation_id) {
        if let Ok(mut record) = session.record.lock() {
            record.stage_timings.push(StageTiming {
                generation_id: generation_id.to_string(),
                stage: stage.to_string(),
                clock_domain: "rust_std_instant".to_string(),
                monotonic_start_ns: session.elapsed_ns(),
                monotonic_end_ns: None,
                wall_started_at: wall_now(),
                wall_ended_at: None,
            });
        }
        let _ = session.persist();
    }
}

pub fn record_stage_end_for(generation_id: &str, stage: &str) {
    if let Ok(session) = session(generation_id) {
        if let Ok(mut record) = session.record.lock() {
            if let Some(timing) = record
                .stage_timings
                .iter_mut()
                .rev()
                .find(|timing| timing.stage == stage && timing.monotonic_end_ns.is_none())
            {
                timing.monotonic_end_ns = Some(session.elapsed_ns());
                timing.wall_ended_at = Some(wall_now());
            }
        }
        let _ = session.persist();
    }
}

pub fn record_frontend_stage(
    generation_id: &str,
    stage: &str,
    monotonic_start_ns: u64,
    monotonic_end_ns: u64,
) -> Result<(), String> {
    if !matches!(stage, "wait_for_transcription" | "page_display_complete") {
        return Err(
            "frontend may only record wait_for_transcription or page_display_complete".to_string(),
        );
    }
    if monotonic_end_ns < monotonic_start_ns {
        return Err("frontend monotonic end precedes start".to_string());
    }
    let session = session(generation_id)?;
    {
        let mut record = session
            .record
            .lock()
            .map_err(|_| "summary measurement lock poisoned".to_string())?;
        record.stage_timings.push(StageTiming {
            generation_id: generation_id.to_string(),
            stage: stage.to_string(),
            clock_domain: "webview_performance".to_string(),
            monotonic_start_ns,
            monotonic_end_ns: Some(monotonic_end_ns),
            wall_started_at: wall_now(),
            wall_ended_at: Some(wall_now()),
        });
    }
    session.persist()
}

pub fn record_input_transcript(generation_id: &str, transcript: &str) {
    if let Ok(session) = session(generation_id) {
        if let Ok(mut record) = session.record.lock() {
            record.input_transcript_sha256 = Some(sha256_text(transcript));
        }
        let _ = session.persist();
    }
}

pub fn record_database_completed_body(generation_id: &str, body: &str) {
    if let Ok(session) = session(generation_id) {
        if let Ok(mut record) = session.record.lock() {
            record.database_completed_body_sha256 = Some(sha256_text(body));
            record.backend_save_completed = true;
        }
        let _ = session.persist();
    }
}

pub fn record_lifecycle_event_for(generation_id: &str, event: &str, pids: &[u32]) {
    let Ok(session) = session(generation_id) else {
        return;
    };
    if let Ok(mut record) = session.record.lock() {
        match event {
            "helper_process_start" => record.helper_process_start_count += 1,
            "model_loaded" => record.model_loaded_event_count += 1,
            "cleanup_entry" => record.cleanup_entry_count += 1,
            "graceful_shutdown_request" => record.graceful_shutdown_request_count += 1,
            "job_confirm_zero" => record.job_confirm_zero_count += 1,
            _ => return,
        }
        record.job_distinct_pids.extend(pids.iter().copied());
        record.lifecycle_events.push(LifecycleEvent {
            generation_id: generation_id.to_string(),
            event: event.to_string(),
            monotonic_offset_ns: session.elapsed_ns(),
            recorded_at: wall_now(),
            pids: pids.to_vec(),
        });
    }
    let _ = session.persist();
}

pub fn finish_non_success(
    generation_id: &str,
    outcome: MeasurementOutcome,
    reason: Option<String>,
) -> Result<(), String> {
    if !matches!(
        outcome,
        MeasurementOutcome::Failed | MeasurementOutcome::Cancelled | MeasurementOutcome::SaveFailed
    ) {
        return Err(
            "non-success completion requires failed, cancelled, or save_failed".to_string(),
        );
    }
    let session = session(generation_id)?;
    session.stop_and_hash_timeline()?;
    {
        let mut record = session
            .record
            .lock()
            .map_err(|_| "summary measurement lock poisoned".to_string())?;
        if record.outcome == MeasurementOutcome::Completed {
            return Ok(());
        }
        record.outcome = outcome;
        record.outcome_reason = reason;
        record.eligible_for_performance = false;
        record.performance_exclusion_reasons = vec![match record.outcome {
            MeasurementOutcome::Failed => "failed",
            MeasurementOutcome::Cancelled => "cancelled",
            MeasurementOutcome::SaveFailed => "save_failed",
            _ => "not_completed",
        }
        .to_string()];
    }
    session.persist()?;
    ACTIVE_MEASUREMENTS.remove(generation_id);
    Ok(())
}

pub fn record_page_completion(
    generation_id: &str,
    database_body: &str,
    page_body: &str,
    page_display_complete_at: String,
    stage_start_monotonic_ns: u64,
    stage_end_monotonic_ns: u64,
) -> Result<SummaryMeasurement, String> {
    DateTime::parse_from_rfc3339(&page_display_complete_at)
        .map_err(|error| format!("pageDisplayCompleteAt must be RFC3339: {error}"))?;
    record_frontend_stage(
        generation_id,
        "page_display_complete",
        stage_start_monotonic_ns,
        stage_end_monotonic_ns,
    )?;
    let session = session(generation_id)?;
    session.stop_and_hash_timeline()?;
    {
        let mut record = session
            .record
            .lock()
            .map_err(|_| "summary measurement lock poisoned".to_string())?;
        if record.outcome != MeasurementOutcome::InProgress {
            return Err("measurement became terminal before page completion".to_string());
        }
        let database_hash = sha256_text(database_body);
        let page_hash = sha256_text(page_body);
        record.database_completed_body_sha256 = Some(database_hash.clone());
        record.page_displayed_body_sha256 = Some(page_hash.clone());
        record.page_matches_database = Some(database_hash == page_hash);
        record.page_display_complete_at = Some(page_display_complete_at);
        record.page_display_complete_monotonic_ns = Some(stage_end_monotonic_ns);
        record.outcome = MeasurementOutcome::Completed;
        record.outcome_reason = None;
        record.performance_exclusion_reasons = evaluate_performance_exclusions(&record);
        record.eligible_for_performance = record.performance_exclusion_reasons.is_empty();
    }
    session.persist()?;
    let record = session
        .record
        .lock()
        .map_err(|_| "summary measurement lock poisoned".to_string())?
        .clone();
    ACTIVE_MEASUREMENTS.remove(generation_id);
    Ok(record)
}

fn evaluate_performance_exclusions(record: &SummaryMeasurement) -> Vec<String> {
    let mut reasons = Vec::new();
    if record.outcome != MeasurementOutcome::Completed {
        reasons.push("not_completed".to_string());
    }
    if !record.backend_save_completed {
        reasons.push("database_save_not_confirmed".to_string());
    }
    if record.page_matches_database != Some(true) {
        reasons.push("page_database_hash_mismatch".to_string());
    }
    if record.input_transcript_sha256.is_none()
        || record.database_completed_body_sha256.is_none()
        || record.page_displayed_body_sha256.is_none()
    {
        reasons.push("body_hash_incomplete".to_string());
    }
    for stage in STAGE_NAMES {
        let complete = record.stage_timings.iter().any(|timing| {
            timing.stage == stage
                && timing
                    .monotonic_end_ns
                    .is_some_and(|end| end >= timing.monotonic_start_ns)
        });
        if !complete {
            reasons.push(format!("stage_incomplete:{stage}"));
        }
    }
    if record.helper_process_start_count == 0
        || record.model_loaded_event_count != record.helper_process_start_count
        || record.cleanup_entry_count < record.helper_process_start_count
        || record.graceful_shutdown_request_count < record.helper_process_start_count
        || record.job_confirm_zero_count < record.helper_process_start_count
        || record.job_distinct_pids.is_empty()
    {
        reasons.push("helper_job_lifecycle_incomplete".to_string());
    }
    if record.load_timeline.is_empty() || record.load_timeline_sha256.is_none() {
        reasons.push("load_timeline_incomplete".to_string());
    } else {
        if !record
            .load_timeline
            .iter()
            .any(|sample| sample.cpu_usage_percent.is_some())
        {
            reasons.push("cpu_timeline_unavailable".to_string());
        }
        if !record
            .load_timeline
            .iter()
            .any(|sample| sample.gpu_usage_percent.is_some())
        {
            reasons.push("gpu_timeline_unavailable".to_string());
        }
        if !record
            .load_timeline
            .iter()
            .any(|sample| sample.memory_commit_bytes.is_some())
        {
            reasons.push("memory_commit_timeline_unavailable".to_string());
        }
    }
    reasons
}

fn session(generation_id: &str) -> Result<Arc<MeasurementSession>, String> {
    ACTIVE_MEASUREMENTS
        .get(generation_id)
        .map(|entry| Arc::clone(entry.value()))
        .ok_or_else(|| "summary measurement not found".to_string())
}

fn start_resource_sampler(session: Arc<MeasurementSession>) {
    tokio::spawn(async move {
        let generation_id = session
            .record
            .lock()
            .ok()
            .map(|record| record.generation_id.clone())
            .unwrap_or_default();
        let mut sampler = ResourceSampler::new();
        sampler.capture(&session, &generation_id);
        let mut interval = tokio::time::interval(RESOURCE_SAMPLE_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            interval.tick().await;
            if session.stopped.load(Ordering::SeqCst) {
                break;
            }
            sampler.capture(&session, &generation_id);
        }
    });
}

struct ResourceSampler {
    system: System,
    #[cfg(windows)]
    gpu: Option<WindowsGpuUsageSampler>,
}

impl ResourceSampler {
    fn new() -> Self {
        let mut system = System::new();
        system.refresh_cpu_usage();
        Self {
            system,
            #[cfg(windows)]
            gpu: WindowsGpuUsageSampler::new(),
        }
    }

    fn capture(&mut self, session: &MeasurementSession, generation_id: &str) {
        self.system.refresh_cpu_usage();
        let cpu_usage_percent = Some(self.system.global_cpu_usage());
        #[cfg(windows)]
        let gpu_usage_percent = self.gpu.as_mut().and_then(WindowsGpuUsageSampler::sample);
        #[cfg(not(windows))]
        let gpu_usage_percent = None;
        session.add_resource_sample(ResourceSample {
            generation_id: generation_id.to_string(),
            monotonic_offset_ns: session.elapsed_ns(),
            recorded_at: wall_now(),
            cpu_usage_percent,
            gpu_usage_percent,
            memory_commit_bytes: system_commit_bytes(),
        });
    }
}

#[cfg(windows)]
struct WindowsGpuUsageSampler {
    query: isize,
    counter: isize,
}

#[cfg(windows)]
impl WindowsGpuUsageSampler {
    fn new() -> Option<Self> {
        use windows::core::PCWSTR;
        use windows::Win32::System::Performance::{
            PdhAddEnglishCounterW, PdhCollectQueryData, PdhOpenQueryW,
        };

        let mut query = 0isize;
        let mut counter = 0isize;
        let path: Vec<u16> = "\\GPU Engine(*)\\Utilization Percentage"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                return None;
            }
            if PdhAddEnglishCounterW(query, PCWSTR::from_raw(path.as_ptr()), 0, &mut counter) != 0 {
                let _ = windows::Win32::System::Performance::PdhCloseQuery(query);
                return None;
            }
            if PdhCollectQueryData(query) != 0 {
                let _ = windows::Win32::System::Performance::PdhCloseQuery(query);
                return None;
            }
        }
        Some(Self { query, counter })
    }

    fn sample(&mut self) -> Option<f64> {
        use windows::Win32::System::Performance::{
            PdhCollectQueryData, PdhGetFormattedCounterArrayW, PDH_CSTATUS_NEW_DATA,
            PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_MORE_DATA,
        };

        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return None;
            }
            let mut buffer_size = 0u32;
            let mut item_count = 0u32;
            let status = PdhGetFormattedCounterArrayW(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut buffer_size,
                &mut item_count,
                None,
            );
            if status != PDH_MORE_DATA || buffer_size == 0 {
                return None;
            }
            let word_count = (buffer_size as usize + std::mem::size_of::<u64>() - 1)
                / std::mem::size_of::<u64>();
            let mut storage = vec![0u64; word_count];
            let items = storage.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            if PdhGetFormattedCounterArrayW(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut buffer_size,
                &mut item_count,
                Some(items),
            ) != 0
            {
                return None;
            }
            let values = std::slice::from_raw_parts(items, item_count as usize);
            let total = values
                .iter()
                .filter(|item| {
                    item.FmtValue.CStatus == PDH_CSTATUS_VALID_DATA
                        || item.FmtValue.CStatus == PDH_CSTATUS_NEW_DATA
                })
                .map(|item| item.FmtValue.Anonymous.doubleValue)
                .filter(|value| value.is_finite() && *value >= 0.0)
                .sum::<f64>();
            Some(total.clamp(0.0, 100.0))
        }
    }
}

#[cfg(windows)]
impl Drop for WindowsGpuUsageSampler {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::System::Performance::PdhCloseQuery(self.query);
        }
    }
}

#[cfg(windows)]
fn system_commit_bytes() -> Option<u64> {
    use windows_sys::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};

    let mut info: PERFORMANCE_INFORMATION = unsafe { std::mem::zeroed() };
    info.cb = std::mem::size_of::<PERFORMANCE_INFORMATION>() as u32;
    let ok = unsafe { GetPerformanceInfo(&mut info, info.cb) };
    if ok == 0 {
        return None;
    }
    (info.CommitTotal as u64).checked_mul(info.PageSize as u64)
}

#[cfg(not(windows))]
fn system_commit_bytes() -> Option<u64> {
    None
}

fn sha256_text(value: &str) -> String {
    sha256_bytes(value.as_bytes())
}

fn sha256_bytes(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn wall_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn completed_measurement_binds_stages_hashes_lifecycle_and_timeline() {
        let directory = tempfile::tempdir().unwrap();
        let generation_id = format!("gen_test_{}", uuid::Uuid::new_v4().simple());
        begin_measurement(
            directory.path(),
            "meeting_test",
            generation_id.clone(),
            "2026-09-07T00:00:00.000Z".to_string(),
            10,
        )
        .unwrap();

        record_frontend_stage(&generation_id, "wait_for_transcription", 10, 20).unwrap();
        for stage in [
            "prepare_input",
            "load_model",
            "chunk_summaries",
            "combine",
            "final_template",
            "translation",
            "save",
        ] {
            stage_guard_for(&generation_id, stage).finish();
        }
        record_input_transcript(&generation_id, "authoritative transcript");
        record_database_completed_body(&generation_id, "## Final\nBody");
        for (event, pids) in [
            ("helper_process_start", &[101u32][..]),
            ("model_loaded", &[][..]),
            ("cleanup_entry", &[][..]),
            ("graceful_shutdown_request", &[][..]),
            ("job_confirm_zero", &[][..]),
        ] {
            record_lifecycle_event_for(&generation_id, event, pids);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;

        let record = record_page_completion(
            &generation_id,
            "## Final\nBody",
            "## Final\nBody",
            "2026-09-07T00:00:01.000Z".to_string(),
            20,
            30,
        )
        .unwrap();
        let timeline_path = directory
            .path()
            .join("summary-measurements")
            .join(&record.load_timeline_path);
        let timeline_bytes = fs::read(timeline_path).unwrap();

        assert_eq!(record.outcome, MeasurementOutcome::Completed);
        assert_eq!(record.timed_endpoint_id, TIMED_ENDPOINT_ID);
        assert_eq!(record.page_matches_database, Some(true));
        assert_eq!(
            record.input_transcript_sha256,
            Some(sha256_text("authoritative transcript"))
        );
        assert_eq!(
            record.database_completed_body_sha256,
            Some(sha256_text("## Final\nBody"))
        );
        assert_eq!(
            record.page_displayed_body_sha256,
            Some(sha256_text("## Final\nBody"))
        );
        assert!(STAGE_NAMES.iter().all(|stage| {
            record.stage_timings.iter().any(|timing| {
                timing.stage == *stage
                    && timing.generation_id == generation_id
                    && timing.monotonic_end_ns >= Some(timing.monotonic_start_ns)
            })
        }));
        assert_eq!(record.helper_process_start_count, 1);
        assert_eq!(record.model_loaded_event_count, 1);
        assert_eq!(record.cleanup_entry_count, 1);
        assert_eq!(record.graceful_shutdown_request_count, 1);
        assert_eq!(record.job_confirm_zero_count, 1);
        assert_eq!(record.job_distinct_pids, BTreeSet::from([101]));
        assert!(record
            .lifecycle_events
            .iter()
            .all(|event| event.generation_id == generation_id));
        assert!(!record.load_timeline.is_empty());
        assert_eq!(
            record.load_timeline_sha256,
            Some(sha256_bytes(&timeline_bytes))
        );
        assert_eq!(record.baseline_non_moss_commit_peak_bytes, None);
        assert_eq!(record.peak_commit_ceiling_bytes, None);
        assert!(session(&generation_id).is_err());
    }

    #[tokio::test]
    async fn failed_cancelled_and_save_failed_are_never_performance_eligible() {
        for outcome in [
            MeasurementOutcome::Failed,
            MeasurementOutcome::Cancelled,
            MeasurementOutcome::SaveFailed,
        ] {
            let directory = tempfile::tempdir().unwrap();
            let generation_id = format!("gen_test_{}", uuid::Uuid::new_v4().simple());
            begin_measurement(
                directory.path(),
                "meeting_test",
                generation_id.clone(),
                "2026-09-07T00:00:00.000Z".to_string(),
                10,
            )
            .unwrap();
            finish_non_success(&generation_id, outcome.clone(), Some("test".to_string())).unwrap();

            let output_path = directory
                .path()
                .join("summary-measurements")
                .join(format!("{generation_id}.json"));
            let record: SummaryMeasurement =
                serde_json::from_slice(&fs::read(output_path).unwrap()).unwrap();
            assert_eq!(record.outcome, outcome);
            assert!(!record.eligible_for_performance);
            assert!(!record.performance_exclusion_reasons.is_empty());
            assert!(session(&generation_id).is_err());
            assert!(record_page_completion(
                &generation_id,
                "body",
                "body",
                "2026-09-07T00:00:01.000Z".to_string(),
                20,
                30,
            )
            .is_err());
        }
    }

    #[test]
    fn timeline_file_hash_matches_exact_jsonl_and_rejects_late_samples() {
        let directory = tempfile::tempdir().unwrap();
        let timeline_path = directory.path().join("load-timeline.jsonl");
        let session = MeasurementSession {
            origin: Instant::now(),
            output_path: directory.path().join("measurement.json"),
            timeline_path: timeline_path.clone(),
            stopped: AtomicBool::new(false),
            record: Mutex::new(test_record()),
        };
        let sample = ResourceSample {
            generation_id: "gen_test".to_string(),
            monotonic_offset_ns: 7,
            recorded_at: "2026-09-06T00:00:00.000Z".to_string(),
            cpu_usage_percent: Some(12.5),
            gpu_usage_percent: Some(4.0),
            memory_commit_bytes: Some(1024),
        };
        session.add_resource_sample(sample.clone());
        session.stop_and_hash_timeline().unwrap();
        session.add_resource_sample(ResourceSample {
            monotonic_offset_ns: 8,
            ..sample.clone()
        });

        let mut expected_bytes = serde_json::to_vec(&sample).unwrap();
        expected_bytes.push(b'\n');
        let actual_bytes = fs::read(&timeline_path).unwrap();
        let expected_digest = sha256_bytes(&actual_bytes);
        let record = session.record.lock().unwrap();
        assert_eq!(actual_bytes, expected_bytes);
        assert_eq!(record.load_timeline.len(), 1);
        assert_eq!(
            record.load_timeline_sha256.as_deref(),
            Some(expected_digest.as_str())
        );
    }

    fn test_record() -> SummaryMeasurement {
        SummaryMeasurement {
            schema_version: 1,
            generation_id: "gen_test".to_string(),
            meeting_id: "meeting_test".to_string(),
            timed_endpoint_id: TIMED_ENDPOINT_ID.to_string(),
            click_at: "2026-09-06T00:00:00.000Z".to_string(),
            click_monotonic_ns: 0,
            page_display_complete_at: None,
            page_display_complete_monotonic_ns: None,
            stage_timings: Vec::new(),
            input_transcript_sha256: None,
            database_completed_body_sha256: None,
            page_displayed_body_sha256: None,
            page_matches_database: None,
            helper_process_start_count: 0,
            job_distinct_pids: BTreeSet::new(),
            model_loaded_event_count: 0,
            cleanup_entry_count: 0,
            graceful_shutdown_request_count: 0,
            job_confirm_zero_count: 0,
            lifecycle_events: Vec::new(),
            load_timeline: Vec::new(),
            load_timeline_path: "timeline.jsonl".to_string(),
            load_timeline_sha256: None,
            baseline_non_moss_commit_peak_bytes: None,
            peak_commit_ceiling_bytes: None,
            outcome: MeasurementOutcome::InProgress,
            outcome_reason: None,
            backend_save_completed: false,
            eligible_for_performance: false,
            performance_exclusion_reasons: Vec::new(),
        }
    }
}
