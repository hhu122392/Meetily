//! D-11 controlled real-time input and measurement evidence.
//!
//! The recorder is opt-in and is only created by the controlled real-time
//! command. Ordinary recordings keep their existing path and behavior.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use sysinfo::System;
use tokio::task::JoinHandle;

use super::decoder::decode_audio_file;

pub const ISOLATION_MARKER_FILE: &str = ".meetily-d11-isolated.json";
pub const INPUT_CONTRACT_FILE: &str = "audio-input-contract.json";
pub const CAPTURE_PCM_FILE: &str = "capture-pcm.f32le";
pub const TRACE_FILE: &str = "realtime-transcription-trace.jsonl";
pub const LOAD_TIMELINE_FILE: &str = "load-timeline.jsonl";
pub const STOP_TIMELINE_FILE: &str = "stop-save-timeline.jsonl";
pub const SUMMARY_FILE: &str = "realtime-measurement.json";
pub const CONTROLLED_SAMPLE_RATE: u32 = 16_000;
pub const CONTROLLED_FRAME_SAMPLES: usize = 1_600;
const LOAD_SAMPLE_INTERVAL_MS: u64 = 250;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RealtimeAudioInputRequest {
    pub input_path: String,
    pub controlled_input_root: String,
    pub isolated_output_root: String,
    pub isolated_data_identity: String,
    pub baseline_snapshot_id: String,
    pub expected_input_file_sha256: String,
    pub meeting_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RealtimeAudioInputContract {
    pub schema_version: u32,
    pub audio_input_path_id: String,
    pub input_path: String,
    pub controlled_input_root: String,
    pub isolated_output_root: String,
    pub isolated_data_identity: String,
    pub baseline_snapshot_id: String,
    pub input_file_sha256: String,
    pub canonical_pcm_sha256: String,
    pub canonical_sample_rate_hz: u32,
    pub canonical_channels: u16,
    pub canonical_sample_format: String,
    pub canonical_sample_count: u64,
    pub duration_seconds: f64,
    pub frame_samples: usize,
    pub pacing: String,
    pub injection_point: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RealtimeAudioInputAccepted {
    pub audio_input_path_id: String,
    pub input_file_sha256: String,
    pub canonical_pcm_sha256: String,
    pub canonical_sample_count: u64,
    pub canonical_sample_rate_hz: u32,
    pub duration_seconds: f64,
    pub baseline_snapshot_id: String,
    pub isolated_data_identity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RealtimeMeasurementReference {
    pub schema_version: u32,
    pub audio_input_path_id: String,
    pub baseline_snapshot_id: String,
    pub isolated_data_identity: String,
    pub input_contract_file: String,
    pub capture_pcm_file: String,
    pub trace_file: String,
    pub load_timeline_file: String,
    pub stop_save_timeline_file: String,
    pub summary_file: String,
}

impl From<&RealtimeAudioInputContract> for RealtimeMeasurementReference {
    fn from(value: &RealtimeAudioInputContract) -> Self {
        Self {
            schema_version: 1,
            audio_input_path_id: value.audio_input_path_id.clone(),
            baseline_snapshot_id: value.baseline_snapshot_id.clone(),
            isolated_data_identity: value.isolated_data_identity.clone(),
            input_contract_file: INPUT_CONTRACT_FILE.to_owned(),
            capture_pcm_file: CAPTURE_PCM_FILE.to_owned(),
            trace_file: TRACE_FILE.to_owned(),
            load_timeline_file: LOAD_TIMELINE_FILE.to_owned(),
            stop_save_timeline_file: STOP_TIMELINE_FILE.to_owned(),
            summary_file: SUMMARY_FILE.to_owned(),
        }
    }
}

impl From<&RealtimeAudioInputContract> for RealtimeAudioInputAccepted {
    fn from(value: &RealtimeAudioInputContract) -> Self {
        Self {
            audio_input_path_id: value.audio_input_path_id.clone(),
            input_file_sha256: value.input_file_sha256.clone(),
            canonical_pcm_sha256: value.canonical_pcm_sha256.clone(),
            canonical_sample_count: value.canonical_sample_count,
            canonical_sample_rate_hz: value.canonical_sample_rate_hz,
            duration_seconds: value.duration_seconds,
            baseline_snapshot_id: value.baseline_snapshot_id.clone(),
            isolated_data_identity: value.isolated_data_identity.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct IsolationMarker {
    schema_version: u32,
    purpose: String,
    isolated_data_identity: String,
}

fn validate_identifier(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
    {
        return Err(anyhow!(
            "{label} must contain only ASCII letters, digits, dash, underscore, or dot"
        ));
    }
    Ok(())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:X}", Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read controlled input {}", path.display()))?;
    Ok(sha256_bytes(&bytes))
}

fn pcm_f32le_bytes(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * std::mem::size_of::<f32>());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn ensure_not_formal_app_data(path: &Path) -> Result<()> {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    if lower.contains("com.meetily.ai") {
        return Err(anyhow!(
            "refusing formal application data root for D-11 controlled measurement"
        ));
    }
    Ok(())
}

fn read_isolation_marker(root: &Path, identity: &str) -> Result<()> {
    let marker_path = root.join(ISOLATION_MARKER_FILE);
    let marker: IsolationMarker = serde_json::from_slice(
        &std::fs::read(&marker_path)
            .with_context(|| format!("missing isolation marker {}", marker_path.display()))?,
    )
    .context("invalid D-11 isolation marker JSON")?;
    if marker.schema_version != 1
        || marker.purpose != "D11_REALTIME_BASELINE"
        || marker.isolated_data_identity != identity
    {
        return Err(anyhow!("D-11 isolation marker does not match the request"));
    }
    Ok(())
}

fn deterministic_input_id(
    input_file_sha256: &str,
    canonical_pcm_sha256: &str,
    sample_count: usize,
) -> String {
    let material = format!(
        "schema_version=1\ninput_file_sha256={input_file_sha256}\ncanonical_pcm_sha256={canonical_pcm_sha256}\nsample_rate_hz={CONTROLLED_SAMPLE_RATE}\nchannels=1\nsample_format=f32le\nsample_count={sample_count}\n"
    );
    format!(
        "audio_input_v1_{}",
        sha256_bytes(material.as_bytes()).to_ascii_lowercase()
    )
}

pub fn prepare_realtime_audio_input(
    request: RealtimeAudioInputRequest,
) -> Result<(RealtimeAudioInputContract, Vec<f32>)> {
    validate_identifier(&request.isolated_data_identity, "isolated_data_identity")?;
    validate_identifier(&request.baseline_snapshot_id, "baseline_snapshot_id")?;

    let input_path = PathBuf::from(&request.input_path)
        .canonicalize()
        .context("controlled input path is unavailable")?;
    let controlled_input_root = PathBuf::from(&request.controlled_input_root)
        .canonicalize()
        .context("controlled input root is unavailable")?;
    let isolated_output_root = PathBuf::from(&request.isolated_output_root)
        .canonicalize()
        .context("isolated output root is unavailable")?;
    ensure_not_formal_app_data(&input_path)?;
    ensure_not_formal_app_data(&controlled_input_root)?;
    ensure_not_formal_app_data(&isolated_output_root)?;
    input_path
        .strip_prefix(&controlled_input_root)
        .context("controlled input must be inside controlled_input_root")?;
    read_isolation_marker(&controlled_input_root, &request.isolated_data_identity)?;
    read_isolation_marker(&isolated_output_root, &request.isolated_data_identity)?;

    let expected = request
        .expected_input_file_sha256
        .trim()
        .to_ascii_uppercase();
    if expected.len() != 64 || !expected.chars().all(|value| value.is_ascii_hexdigit()) {
        return Err(anyhow!(
            "expected_input_file_sha256 must be 64 hexadecimal characters"
        ));
    }
    let input_file_sha256 = sha256_file(&input_path)?;
    if input_file_sha256 != expected {
        return Err(anyhow!(
            "controlled input SHA-256 mismatch: expected {expected}, got {input_file_sha256}"
        ));
    }

    let decoded = decode_audio_file(&input_path).context("failed to decode controlled input")?;
    let canonical_pcm = decoded.to_whisper_format();
    if canonical_pcm.is_empty() {
        return Err(anyhow!("controlled input decoded to empty canonical PCM"));
    }
    let canonical_pcm_sha256 = sha256_bytes(&pcm_f32le_bytes(&canonical_pcm));
    let audio_input_path_id = deterministic_input_id(
        &input_file_sha256,
        &canonical_pcm_sha256,
        canonical_pcm.len(),
    );
    let contract = RealtimeAudioInputContract {
        schema_version: 1,
        audio_input_path_id,
        input_path: input_path.to_string_lossy().into_owned(),
        controlled_input_root: controlled_input_root.to_string_lossy().into_owned(),
        isolated_output_root: isolated_output_root.to_string_lossy().into_owned(),
        isolated_data_identity: request.isolated_data_identity,
        baseline_snapshot_id: request.baseline_snapshot_id,
        input_file_sha256,
        canonical_pcm_sha256,
        canonical_sample_rate_hz: CONTROLLED_SAMPLE_RATE,
        canonical_channels: 1,
        canonical_sample_format: "f32le".to_owned(),
        canonical_sample_count: canonical_pcm.len() as u64,
        duration_seconds: canonical_pcm.len() as f64 / CONTROLLED_SAMPLE_RATE as f64,
        frame_samples: CONTROLLED_FRAME_SAMPLES,
        pacing: "monotonic_realtime".to_owned(),
        injection_point: "post_capture_post_mix_pre_vad".to_owned(),
    };
    Ok((contract, canonical_pcm))
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChunkMeasurementTrace {
    pub chunk_id: u64,
    pub source_chunk_ids: Vec<u64>,
    pub source_sample_start: Option<u64>,
    pub source_sample_end: Option<u64>,
    pub sample_rate_hz: Option<u32>,
    pub overlap_samples: u64,
    pub vad_decision: Option<String>,
    pub vad_exclusion_reason: Option<String>,
    pub enqueued_at_monotonic_ns: Option<u64>,
    pub dequeued_at_monotonic_ns: Option<u64>,
    pub inference_started_at_monotonic_ns: Option<u64>,
    pub inference_finished_at_monotonic_ns: Option<u64>,
    pub inference_batch_chunk_id: Option<u64>,
    pub inference_sample_start: Option<u64>,
    pub inference_sample_end: Option<u64>,
    pub inference_sample_count: Option<u64>,
    pub inference_pcm_sha256: Option<String>,
    pub transcription_provider: Option<String>,
    pub transcription_model: Option<String>,
    pub final_writeback_at_monotonic_ns: Option<u64>,
    pub text_before_dedup: Option<String>,
    pub text_after_dedup: Option<String>,
    pub text_after_context_normalization: Option<String>,
    pub dedup_trace_status: Option<String>,
    pub actual_language: Option<String>,
    pub language_applied_to_provider: Option<bool>,
    pub context_version_id: Option<String>,
    pub context_sha256: Option<String>,
    pub coalesced_into_chunk_id: Option<u64>,
    pub writeback_result: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VadWindowTrace {
    pub sequence: u64,
    pub source_sample_start: u64,
    pub source_sample_end: u64,
    pub vad_decision: String,
    pub vad_exclusion_reason: Option<String>,
    pub emitted_chunk_ids: Vec<u64>,
    pub observed_at_monotonic_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadTimelineSample {
    pub sequence: u64,
    pub monotonic_ns: u64,
    pub system_cpu_percent: Option<f32>,
    pub system_memory_used_bytes: u64,
    pub system_memory_total_bytes: u64,
    pub system_unavailable_reason: Option<String>,
    pub process_cpu_percent: Option<f32>,
    pub process_memory_used_bytes: Option<u64>,
    pub process_unavailable_reason: Option<String>,
    pub gpu_percent: Option<f32>,
    pub gpu_memory_used_bytes: Option<u64>,
    pub gpu_unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopSaveTimelineEvent {
    pub sequence: u64,
    pub stage: String,
    pub result: String,
    pub monotonic_ns: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeasurementFileRecord {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TranscriptionEngineIdentity {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealtimeMeasurementSummary {
    pub schema_version: u32,
    pub recording_id: String,
    pub audio_input_path_id: String,
    pub baseline_snapshot_id: String,
    pub isolated_data_identity: String,
    pub transcription_engine: Option<TranscriptionEngineIdentity>,
    pub measurement_started_at_monotonic_ns: u64,
    pub measurement_finalized_at_monotonic_ns: u64,
    pub capture_pcm_sha256: String,
    pub capture_pcm: MeasurementFileRecord,
    pub capture_pcm_sample_count: u64,
    pub capture_pcm_sample_rate_hz: Option<u32>,
    pub trace: MeasurementFileRecord,
    pub load_timeline: MeasurementFileRecord,
    pub load_timeline_sha256: String,
    pub stop_save_timeline: MeasurementFileRecord,
    pub chunk_count: usize,
    pub vad_window_count: usize,
    pub load_sample_count: usize,
    pub stop_save_event_count: usize,
    pub errors: Vec<String>,
}

pub struct D11MeasurementRecorder {
    meeting_folder: PathBuf,
    recording_id: String,
    contract: RealtimeAudioInputContract,
    origin: Instant,
    sequence: AtomicU64,
    chunks: Mutex<BTreeMap<u64, ChunkMeasurementTrace>>,
    vad_windows: Mutex<Vec<VadWindowTrace>>,
    load_samples: Mutex<Vec<LoadTimelineSample>>,
    stop_save_timeline: Mutex<Vec<StopSaveTimelineEvent>>,
    errors: Mutex<Vec<String>>,
    transcription_engine: Mutex<Option<TranscriptionEngineIdentity>>,
    capture_hasher: Mutex<Sha256>,
    capture_pcm_bytes: Mutex<Vec<u8>>,
    capture_sample_count: AtomicU64,
    capture_sample_rate_hz: AtomicU64,
    load_stop: Arc<AtomicBool>,
    load_handle: Mutex<Option<JoinHandle<()>>>,
    finalized: AtomicBool,
}

impl D11MeasurementRecorder {
    pub fn new(
        meeting_folder: PathBuf,
        recording_id: String,
        contract: RealtimeAudioInputContract,
    ) -> Result<Arc<Self>> {
        let recorder = Arc::new(Self {
            meeting_folder,
            recording_id,
            contract,
            origin: Instant::now(),
            sequence: AtomicU64::new(0),
            chunks: Mutex::new(BTreeMap::new()),
            vad_windows: Mutex::new(Vec::new()),
            load_samples: Mutex::new(Vec::new()),
            stop_save_timeline: Mutex::new(Vec::new()),
            errors: Mutex::new(Vec::new()),
            transcription_engine: Mutex::new(None),
            capture_hasher: Mutex::new(Sha256::new()),
            capture_pcm_bytes: Mutex::new(Vec::new()),
            capture_sample_count: AtomicU64::new(0),
            capture_sample_rate_hz: AtomicU64::new(0),
            load_stop: Arc::new(AtomicBool::new(false)),
            load_handle: Mutex::new(None),
            finalized: AtomicBool::new(false),
        });
        write_json_atomic(
            &recorder.meeting_folder.join(INPUT_CONTRACT_FILE),
            &recorder.contract,
        )?;
        Ok(recorder)
    }

    pub fn contract(&self) -> &RealtimeAudioInputContract {
        &self.contract
    }

    pub fn now_ns(&self) -> u64 {
        self.origin.elapsed().as_nanos().min(u64::MAX as u128) as u64
    }

    fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::SeqCst)
    }

    fn update_chunks<F>(&self, chunk_ids: &[u64], mut update: F)
    where
        F: FnMut(&mut ChunkMeasurementTrace),
    {
        if let Ok(mut traces) = self.chunks.lock() {
            for chunk_id in chunk_ids {
                let trace = traces
                    .entry(*chunk_id)
                    .or_insert_with(|| ChunkMeasurementTrace {
                        chunk_id: *chunk_id,
                        source_chunk_ids: vec![*chunk_id],
                        ..ChunkMeasurementTrace::default()
                    });
                update(trace);
            }
        }
    }

    pub fn record_vad_window(
        &self,
        source_sample_start: u64,
        source_sample_end: u64,
        vad_decision: &str,
        vad_exclusion_reason: Option<&str>,
        emitted_chunk_ids: Vec<u64>,
    ) {
        if let Ok(mut windows) = self.vad_windows.lock() {
            windows.push(VadWindowTrace {
                sequence: self.next_sequence(),
                source_sample_start,
                source_sample_end,
                vad_decision: vad_decision.to_owned(),
                vad_exclusion_reason: vad_exclusion_reason.map(ToOwned::to_owned),
                emitted_chunk_ids,
                observed_at_monotonic_ns: self.now_ns(),
            });
        }
    }

    pub fn record_vad_chunk(
        &self,
        chunk_id: u64,
        source_sample_start: u64,
        source_sample_end: u64,
        sample_rate_hz: u32,
        vad_decision: &str,
        vad_exclusion_reason: Option<&str>,
    ) {
        self.update_chunks(&[chunk_id], |trace| {
            trace.source_sample_start = Some(source_sample_start);
            trace.source_sample_end = Some(source_sample_end);
            trace.sample_rate_hz = Some(sample_rate_hz);
            trace.vad_decision = Some(vad_decision.to_owned());
            trace.vad_exclusion_reason = vad_exclusion_reason.map(ToOwned::to_owned);
        });
    }

    pub fn record_enqueued(&self, chunk_id: u64) {
        let now = self.now_ns();
        self.update_chunks(&[chunk_id], |trace| {
            trace.enqueued_at_monotonic_ns = Some(now);
        });
    }

    pub fn record_dequeued(&self, chunk_id: u64) {
        let now = self.now_ns();
        self.update_chunks(&[chunk_id], |trace| {
            trace.dequeued_at_monotonic_ns = Some(now);
        });
    }

    pub fn record_overlap(&self, base_chunk_id: u64, next_chunk_id: u64, overlap_samples: u64) {
        self.update_chunks(&[base_chunk_id], |trace| {
            if !trace.source_chunk_ids.contains(&next_chunk_id) {
                trace.source_chunk_ids.push(next_chunk_id);
            }
            trace.overlap_samples = trace.overlap_samples.saturating_add(overlap_samples);
        });
        self.update_chunks(&[next_chunk_id], |trace| {
            trace.overlap_samples = overlap_samples;
            trace.coalesced_into_chunk_id = Some(base_chunk_id);
        });
    }

    pub fn record_inference_started(
        &self,
        chunk_ids: &[u64],
        samples: &[f32],
        sample_rate_hz: u32,
        actual_language: Option<&str>,
        language_applied_to_provider: bool,
        context_version_id: Option<&str>,
        context_sha256: Option<&str>,
        transcription_provider: &str,
        transcription_model: &str,
    ) {
        let inference_pcm_bytes = pcm_f32le_bytes(samples);
        let inference_pcm_sha256 = sha256_bytes(&inference_pcm_bytes);
        if let Ok(mut hasher) = self.capture_hasher.lock() {
            hasher.update(&inference_pcm_bytes);
        }
        if let Ok(mut capture) = self.capture_pcm_bytes.lock() {
            capture.extend_from_slice(&inference_pcm_bytes);
        }
        let inference_sample_start = self
            .capture_sample_count
            .fetch_add(samples.len() as u64, Ordering::SeqCst);
        let inference_sample_end = inference_sample_start.saturating_add(samples.len() as u64);
        let _ = self.capture_sample_rate_hz.compare_exchange(
            0,
            sample_rate_hz as u64,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        let now = self.now_ns();
        let inference_batch_chunk_id = chunk_ids.first().copied();
        self.update_chunks(chunk_ids, |trace| {
            trace.inference_started_at_monotonic_ns = Some(now);
            trace.inference_batch_chunk_id = inference_batch_chunk_id;
            trace.inference_sample_start = Some(inference_sample_start);
            trace.inference_sample_end = Some(inference_sample_end);
            trace.inference_sample_count = Some(samples.len() as u64);
            trace.inference_pcm_sha256 = Some(inference_pcm_sha256.clone());
            trace.transcription_provider = Some(transcription_provider.to_owned());
            trace.transcription_model = Some(transcription_model.to_owned());
            trace.actual_language = actual_language.map(ToOwned::to_owned);
            trace.language_applied_to_provider = Some(language_applied_to_provider);
            trace.context_version_id = context_version_id.map(ToOwned::to_owned);
            trace.context_sha256 = context_sha256.map(ToOwned::to_owned);
        });
    }

    pub fn record_inference_finished(&self, chunk_ids: &[u64], error: Option<&str>) {
        let now = self.now_ns();
        self.update_chunks(chunk_ids, |trace| {
            trace.inference_finished_at_monotonic_ns = Some(now);
            trace.error = error.map(ToOwned::to_owned);
        });
    }

    pub fn record_text(
        &self,
        chunk_ids: &[u64],
        before_dedup: &str,
        after_dedup: &str,
        after_context_normalization: &str,
        dedup_trace_status: &str,
    ) {
        self.update_chunks(chunk_ids, |trace| {
            trace.text_before_dedup = Some(before_dedup.to_owned());
            trace.text_after_dedup = Some(after_dedup.to_owned());
            trace.text_after_context_normalization = Some(after_context_normalization.to_owned());
            trace.dedup_trace_status = Some(dedup_trace_status.to_owned());
        });
    }

    pub fn record_final_writeback(&self, chunk_ids: &[u64], result: Result<(), String>) {
        let now = self.now_ns();
        self.update_chunks(chunk_ids, |trace| {
            trace.final_writeback_at_monotonic_ns = Some(now);
            match &result {
                Ok(()) => trace.writeback_result = Some("written".to_owned()),
                Err(error) => {
                    trace.writeback_result = Some("failed".to_owned());
                    trace.error = Some(error.clone());
                }
            }
        });
        if let Err(error) = result {
            self.record_error(format!("final transcript writeback failed: {error}"));
        }
    }

    pub fn record_terminal_without_writeback(&self, chunk_ids: &[u64], reason: &str) {
        let now = self.now_ns();
        self.update_chunks(chunk_ids, |trace| {
            trace.final_writeback_at_monotonic_ns = Some(now);
            trace.writeback_result = Some(reason.to_owned());
        });
    }

    pub fn record_stop_stage(&self, stage: &str, result: &str, error: Option<&str>) {
        if let Ok(mut timeline) = self.stop_save_timeline.lock() {
            timeline.push(StopSaveTimelineEvent {
                sequence: self.next_sequence(),
                stage: stage.to_owned(),
                result: result.to_owned(),
                monotonic_ns: self.now_ns(),
                error: error.map(ToOwned::to_owned),
            });
        }
    }

    pub fn record_error(&self, error: String) {
        if let Ok(mut errors) = self.errors.lock() {
            errors.push(error);
        }
    }

    pub fn record_engine_identity(&self, provider: &str, model: &str) {
        if let Ok(mut identity) = self.transcription_engine.lock() {
            *identity = Some(TranscriptionEngineIdentity {
                provider: provider.to_owned(),
                model: model.to_owned(),
            });
        }
    }

    pub fn start_load_sampler(self: &Arc<Self>) {
        let mut handle = self.load_handle.lock().expect("load handle mutex poisoned");
        if handle.is_some() {
            return;
        }
        let recorder = Arc::clone(self);
        *handle = Some(tokio::spawn(async move {
            let mut system = System::new_all();
            let current_pid = sysinfo::get_current_pid().ok();
            loop {
                system.refresh_all();
                let cpu_count = system.cpus().len();
                let (system_cpu_percent, system_unavailable_reason) = if cpu_count == 0 {
                    (
                        None,
                        Some("system CPU metrics are unavailable from sysinfo".to_owned()),
                    )
                } else {
                    (
                        Some(
                            system.cpus().iter().map(|cpu| cpu.cpu_usage()).sum::<f32>()
                                / cpu_count as f32,
                        ),
                        None,
                    )
                };
                let current_process = current_pid.and_then(|pid| system.process(pid));
                let process_unavailable_reason = if current_process.is_none() {
                    Some("current process metrics are unavailable from sysinfo".to_owned())
                } else {
                    None
                };
                let sample = LoadTimelineSample {
                    sequence: recorder.next_sequence(),
                    monotonic_ns: recorder.now_ns(),
                    system_cpu_percent,
                    system_memory_used_bytes: system.used_memory(),
                    system_memory_total_bytes: system.total_memory(),
                    system_unavailable_reason,
                    process_cpu_percent: current_process.map(|process| process.cpu_usage()),
                    process_memory_used_bytes: current_process.map(|process| process.memory()),
                    process_unavailable_reason,
                    gpu_percent: None,
                    gpu_memory_used_bytes: None,
                    gpu_unavailable_reason: Some(
                        "GPU telemetry is not exposed by the current cross-platform runtime"
                            .to_owned(),
                    ),
                };
                if let Ok(mut samples) = recorder.load_samples.lock() {
                    samples.push(sample);
                }
                if recorder.load_stop.load(Ordering::SeqCst) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(LOAD_SAMPLE_INTERVAL_MS)).await;
            }
        }));
    }

    pub async fn finalize(&self) -> Result<RealtimeMeasurementSummary> {
        if self.finalized.swap(true, Ordering::SeqCst) {
            return Err(anyhow!("D-11 measurement was already finalized"));
        }
        self.load_stop.store(true, Ordering::SeqCst);
        let load_handle = self
            .load_handle
            .lock()
            .map_err(|_| anyhow!("load sampler handle mutex poisoned"))?
            .take();
        if let Some(handle) = load_handle {
            if let Err(error) = handle.await {
                self.record_error(format!("load sampler join failed: {error}"));
            }
        }
        self.record_stop_stage("measurement_finalized", "completed", None);
        let measurement_finalized_at_monotonic_ns = self.now_ns();

        let chunks = self
            .chunks
            .lock()
            .map_err(|_| anyhow!("chunk trace mutex poisoned"))?
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let vad_windows = self
            .vad_windows
            .lock()
            .map_err(|_| anyhow!("VAD window trace mutex poisoned"))?
            .clone();
        let load_samples = self
            .load_samples
            .lock()
            .map_err(|_| anyhow!("load timeline mutex poisoned"))?
            .clone();
        let stop_save_timeline = self
            .stop_save_timeline
            .lock()
            .map_err(|_| anyhow!("stop/save timeline mutex poisoned"))?
            .clone();

        let mut trace_rows = Vec::with_capacity(vad_windows.len() + chunks.len());
        for row in &vad_windows {
            trace_rows.push(serde_json::json!({"record_type": "vad_window", "value": row}));
        }
        for row in &chunks {
            trace_rows.push(serde_json::json!({"record_type": "chunk", "value": row}));
        }
        let trace = write_jsonl_atomic(&self.meeting_folder.join(TRACE_FILE), &trace_rows)?;
        let load_timeline =
            write_jsonl_atomic(&self.meeting_folder.join(LOAD_TIMELINE_FILE), &load_samples)?;
        let stop_save_timeline_record = write_jsonl_atomic(
            &self.meeting_folder.join(STOP_TIMELINE_FILE),
            &stop_save_timeline,
        )?;
        let capture_pcm_sha256 = self
            .capture_hasher
            .lock()
            .map_err(|_| anyhow!("capture PCM hasher mutex poisoned"))?
            .clone()
            .finalize();
        let capture_pcm_sha256 = format!("{capture_pcm_sha256:X}");
        let capture_pcm_bytes = self
            .capture_pcm_bytes
            .lock()
            .map_err(|_| anyhow!("capture PCM bytes mutex poisoned"))?
            .clone();
        let capture_pcm = write_binary_record_atomic(
            &self.meeting_folder.join(CAPTURE_PCM_FILE),
            &capture_pcm_bytes,
        )?;
        if capture_pcm.sha256 != capture_pcm_sha256 {
            return Err(anyhow!(
                "capture PCM hash mismatch between streaming and persisted evidence"
            ));
        }
        let sample_rate = self.capture_sample_rate_hz.load(Ordering::SeqCst);
        let summary = RealtimeMeasurementSummary {
            schema_version: 1,
            recording_id: self.recording_id.clone(),
            audio_input_path_id: self.contract.audio_input_path_id.clone(),
            baseline_snapshot_id: self.contract.baseline_snapshot_id.clone(),
            isolated_data_identity: self.contract.isolated_data_identity.clone(),
            transcription_engine: self
                .transcription_engine
                .lock()
                .map_err(|_| anyhow!("transcription engine identity mutex poisoned"))?
                .clone(),
            measurement_started_at_monotonic_ns: 0,
            measurement_finalized_at_monotonic_ns,
            capture_pcm_sha256,
            capture_pcm,
            capture_pcm_sample_count: self.capture_sample_count.load(Ordering::SeqCst),
            capture_pcm_sample_rate_hz: (sample_rate != 0).then_some(sample_rate as u32),
            trace,
            load_timeline_sha256: load_timeline.sha256.clone(),
            load_timeline,
            stop_save_timeline: stop_save_timeline_record,
            chunk_count: chunks.len(),
            vad_window_count: vad_windows.len(),
            load_sample_count: load_samples.len(),
            stop_save_event_count: stop_save_timeline.len(),
            errors: self
                .errors
                .lock()
                .map_err(|_| anyhow!("measurement error mutex poisoned"))?
                .clone(),
        };
        write_json_atomic(&self.meeting_folder.join(SUMMARY_FILE), &summary)?;
        Ok(summary)
    }
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    write_bytes_atomic(path, &bytes)
}

fn write_jsonl_atomic<T: Serialize>(path: &Path, values: &[T]) -> Result<MeasurementFileRecord> {
    let mut bytes = Vec::new();
    for value in values {
        serde_json::to_writer(&mut bytes, value)?;
        bytes.push(b'\n');
    }
    write_bytes_atomic(path, &bytes)?;
    Ok(MeasurementFileRecord {
        path: path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_owned(),
        bytes: bytes.len() as u64,
        sha256: sha256_bytes(&bytes),
    })
}

fn write_binary_record_atomic(path: &Path, bytes: &[u8]) -> Result<MeasurementFileRecord> {
    write_bytes_atomic(path, bytes)?;
    Ok(MeasurementFileRecord {
        path: path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_owned(),
        bytes: bytes.len() as u64,
        sha256: sha256_bytes(bytes),
    })
}

fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or("json")
    ));
    std::fs::write(&temporary, bytes)
        .with_context(|| format!("failed to write {}", temporary.display()))?;
    std::fs::rename(&temporary, path)
        .with_context(|| format!("failed to publish {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_isolation_marker(root: &Path, identity: &str) {
        std::fs::write(
            root.join(ISOLATION_MARKER_FILE),
            serde_json::json!({
                "schema_version": 1,
                "purpose": "D11_REALTIME_BASELINE",
                "isolated_data_identity": identity,
            })
            .to_string(),
        )
        .unwrap();
    }

    fn request(input: &Path, input_root: &Path, output_root: &Path) -> RealtimeAudioInputRequest {
        RealtimeAudioInputRequest {
            input_path: input.to_string_lossy().into_owned(),
            controlled_input_root: input_root.to_string_lossy().into_owned(),
            isolated_output_root: output_root.to_string_lossy().into_owned(),
            isolated_data_identity: "d11-test".to_owned(),
            baseline_snapshot_id: "baseline-test".to_owned(),
            expected_input_file_sha256: "0".repeat(64),
            meeting_name: None,
        }
    }

    #[test]
    fn deterministic_audio_input_path_id_depends_on_bytes_and_format() {
        let first = deterministic_input_id(&"A".repeat(64), &"B".repeat(64), 16_000);
        let second = deterministic_input_id(&"A".repeat(64), &"B".repeat(64), 16_000);
        let changed = deterministic_input_id(&"A".repeat(64), &"B".repeat(64), 16_001);
        assert_eq!(first, second);
        assert_ne!(first, changed);
        assert!(first.starts_with("audio_input_v1_"));
    }

    #[test]
    fn controlled_input_fails_closed_without_both_isolation_markers() {
        let directory = tempfile::tempdir().unwrap();
        let input_root = directory.path().join("input");
        let output_root = directory.path().join("output");
        std::fs::create_dir_all(&input_root).unwrap();
        std::fs::create_dir_all(&output_root).unwrap();
        let input = input_root.join("controlled.wav");
        std::fs::write(&input, b"not decoded because output marker is absent").unwrap();
        write_isolation_marker(&input_root, "d11-test");

        let error =
            prepare_realtime_audio_input(request(&input, &input_root, &output_root)).unwrap_err();
        assert!(error.to_string().contains("missing isolation marker"));
    }

    #[test]
    fn controlled_input_rejects_formal_application_data_paths_before_decode() {
        let directory = tempfile::tempdir().unwrap();
        let input_root = directory.path().join("com.meetily.ai").join("input");
        let output_root = directory.path().join("output");
        std::fs::create_dir_all(&input_root).unwrap();
        std::fs::create_dir_all(&output_root).unwrap();
        let input = input_root.join("controlled.wav");
        std::fs::write(&input, b"not decoded because this root is forbidden").unwrap();

        let error =
            prepare_realtime_audio_input(request(&input, &input_root, &output_root)).unwrap_err();
        assert!(error
            .to_string()
            .contains("refusing formal application data root"));
    }

    #[tokio::test]
    async fn recorder_persists_hashes_chunk_trace_load_and_stop_timeline() {
        let directory = tempfile::tempdir().unwrap();
        let contract = RealtimeAudioInputContract {
            schema_version: 1,
            audio_input_path_id: "audio_input_v1_test".to_owned(),
            input_path: "controlled.wav".to_owned(),
            controlled_input_root: directory.path().to_string_lossy().into_owned(),
            isolated_output_root: directory.path().to_string_lossy().into_owned(),
            isolated_data_identity: "d11-test".to_owned(),
            baseline_snapshot_id: "baseline-test".to_owned(),
            input_file_sha256: "A".repeat(64),
            canonical_pcm_sha256: "B".repeat(64),
            canonical_sample_rate_hz: 16_000,
            canonical_channels: 1,
            canonical_sample_format: "f32le".to_owned(),
            canonical_sample_count: 4,
            duration_seconds: 0.00025,
            frame_samples: 1_600,
            pacing: "monotonic_realtime".to_owned(),
            injection_point: "post_capture_post_mix_pre_vad".to_owned(),
        };
        let recorder = D11MeasurementRecorder::new(
            directory.path().to_path_buf(),
            "recording-test".to_owned(),
            contract,
        )
        .unwrap();
        recorder.start_load_sampler();
        recorder.record_engine_identity("whisper", "test-model");
        recorder.record_vad_chunk(7, 10, 14, 16_000, "speech_emitted", None);
        recorder.record_enqueued(7);
        recorder.record_dequeued(7);
        recorder.record_inference_started(
            &[7],
            &[0.25, -0.25, 0.5, -0.5],
            16_000,
            Some("zh"),
            true,
            Some("ctx-v1"),
            Some(&"C".repeat(64)),
            "whisper",
            "test-model",
        );
        recorder.record_text(
            &[7],
            "before",
            "after",
            "after-context",
            "captured_whisper_engine",
        );
        recorder.record_inference_finished(&[7], None);
        recorder.record_final_writeback(&[7], Ok(()));
        recorder.record_stop_stage("stop_requested", "started", None);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let summary = recorder.finalize().await.unwrap();
        assert_eq!(summary.chunk_count, 1);
        assert!(summary.load_sample_count >= 1);
        assert_eq!(summary.capture_pcm_sample_count, 4);
        assert_eq!(summary.capture_pcm_sha256.len(), 64);
        assert_eq!(summary.capture_pcm.bytes, 16);
        assert_eq!(summary.capture_pcm.sha256, summary.capture_pcm_sha256);
        assert_eq!(summary.load_timeline_sha256.len(), 64);
        assert_eq!(
            summary.transcription_engine,
            Some(TranscriptionEngineIdentity {
                provider: "whisper".to_owned(),
                model: "test-model".to_owned(),
            })
        );
        assert!(directory.path().join(TRACE_FILE).is_file());
        assert!(directory.path().join(CAPTURE_PCM_FILE).is_file());
        assert!(directory.path().join(LOAD_TIMELINE_FILE).is_file());
        assert!(directory.path().join(STOP_TIMELINE_FILE).is_file());
        assert!(directory.path().join(SUMMARY_FILE).is_file());
    }
}
