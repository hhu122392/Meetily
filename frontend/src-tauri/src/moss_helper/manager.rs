use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moss_helper::pcm::PcmActivity;
#[cfg(test)]
use moss_helper::protocol::NativeSessionLimits;
use moss_helper::protocol::{
    read_server_message, write_client_message, AcceptedMessage, CancelCommand, CancelReason,
    ClientMessage, CompletedMessage, DeviceSpec, ErrorCode, ModelSpec, Operation, ProbeCommand,
    ProbeResultMessage, RuntimeSpec, ServerMessage, TaskPhase, TextStream, TranscribeCommand,
    PROTOCOL_VERSION,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::audio_stage::{stage_interleaved, stage_interleaved_with_cancel, StageError};
use super::windows_job::{spawn_suspended_assigned, JobControl, JobError};

const GRACE_PERIOD: Duration = Duration::from_secs(2);
const PIPE_EVENT_POLL: Duration = Duration::from_millis(100);
const PERFORMANCE_GATE_MINIMUM_SECONDS: f64 = 480.0;
const FROZEN_MAX_WALL_RTF: f64 = 1.0;
const MAX_NATIVE_CHUNK_SECONDS: u64 = moss_helper::pcm::MAX_AUDIO_SECONDS;
pub(crate) const MAX_REQUEST_SECONDS: u64 = MAX_NATIVE_CHUNK_SECONDS;
const SPEAKER_CHUNK_STRIDE: i32 = 100_000;
const RESERVATION_RUNNING: u8 = 0;
const RESERVATION_CANCELLED: u8 = 1;
const RESERVATION_COMMITTED: u8 = 2;

#[derive(Debug, thiserror::Error)]
pub enum ManagerError {
    #[error("MOSS helper is unavailable")]
    Unavailable,
    #[error("MOSS request is invalid")]
    InvalidRequest,
    #[error("MOSS audio exceeds the supported duration")]
    AudioTooLong,
    #[error("MOSS request is already active")]
    DuplicateRequest,
    #[error("another MOSS request is already running")]
    Busy,
    #[error("MOSS audio staging failed")]
    AudioStage,
    #[error("MOSS helper process launch failed: {0}")]
    Spawn(JobError),
    #[error("MOSS helper protocol failed")]
    Protocol,
    #[error("MOSS helper timed out")]
    Timeout,
    #[error("MOSS end-to-end real-time factor exceeded the frozen limit")]
    PerformanceGate,
    #[error("MOSS helper was cancelled")]
    Cancelled,
    #[error("MOSS native operation failed")]
    Native(ErrorCode),
}

impl From<JobError> for ManagerError {
    fn from(error: JobError) -> Self {
        // JobError contains only a fixed operation name, a numeric Windows
        // error, and the operating-system message. It deliberately carries no
        // executable path, meeting text, model path, or user data.
        log::error!("MOSS helper process control failed: {error}");
        Self::Spawn(error)
    }
}

impl From<StageError> for ManagerError {
    fn from(_: StageError) -> Self {
        Self::AudioStage
    }
}

#[derive(Debug, Clone)]
pub struct TranscribeInput {
    pub request_id: String,
    pub context_sha256: String,
    pub runtime_directory: PathBuf,
    pub model_path: PathBuf,
    pub model_bytes: u64,
    pub model_sha256: String,
    pub device_id: Option<String>,
    pub timeout: Duration,
    pub max_wall_rtf: Option<f64>,
    pub samples: Vec<f32>,
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub language_requested: String,
    pub decode_parameters_json: String,
    pub decode_parameters_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedSegment {
    pub segment_index: u32,
    pub t0_ms: i64,
    pub t1_ms: i64,
    pub speaker_id: i32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedTranscription {
    pub request_id: String,
    pub context_sha256: String,
    pub raw_text: String,
    pub clean_text: String,
    pub segments: Vec<ManagedSegment>,
    pub completed: CompletedMessage,
    pub heartbeat_count: u64,
    pub supervisor_wall_elapsed_ms: u64,
    pub supervisor_wall_rtf: f64,
    pub helper_process_id: u32,
    pub helper_total_processes: u32,
    pub helper_peak_job_memory_bytes: u64,
    pub residual_process_count: u32,
    pub helper_runs: Vec<ManagedHelperRun>,
    pub audio_activity: Option<PcmActivity>,
    pub language_requested: String,
    pub language_resolved: String,
    pub decode_parameters_json: String,
    pub decode_parameters_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedHelperRun {
    pub chunk_index: u32,
    pub input_offset_ms: u64,
    pub input_duration_ms: u64,
    pub process_id: u32,
    pub total_processes: u32,
    pub peak_job_memory_bytes: u64,
    pub residual_process_count: u32,
    pub terminal_count: u32,
    pub last_timestamp_ms: i64,
    pub native_rtf: f64,
    pub raw_text_sha256: String,
    pub clean_text_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagerStatus {
    pub active_request_ids: Vec<String>,
    pub active_process_ids: Vec<u32>,
    pub helper_processes_started: u64,
}

pub struct MossHelperManager {
    helper_path: Option<PathBuf>,
    cache_root: PathBuf,
    active: Mutex<HashMap<String, Arc<TaskControl>>>,
    reservation: Mutex<Option<Reservation>>,
    helper_processes_started: AtomicU64,
}

#[derive(Clone)]
struct Reservation {
    request_id: String,
    state: Arc<std::sync::atomic::AtomicU8>,
}

pub(crate) struct PreparationLease {
    manager: Arc<MossHelperManager>,
    request_id: String,
    state: Arc<std::sync::atomic::AtomicU8>,
}

impl PreparationLease {
    pub(crate) fn is_cancelled(&self) -> bool {
        self.state.load(Ordering::SeqCst) == RESERVATION_CANCELLED
    }
}

impl Drop for PreparationLease {
    fn drop(&mut self) {
        self.manager.release_reservation(&self.request_id);
    }
}

impl MossHelperManager {
    pub fn new(helper_path: Option<PathBuf>, cache_root: PathBuf) -> Self {
        Self {
            helper_path,
            cache_root,
            active: Mutex::new(HashMap::new()),
            reservation: Mutex::new(None),
            helper_processes_started: AtomicU64::new(0),
        }
    }

    pub fn is_available(&self) -> bool {
        self.helper_path.is_some()
    }

    pub fn resolve_helper_binary() -> Option<PathBuf> {
        if cfg!(debug_assertions) {
            if let Some(path) = std::env::var_os("MEETILY_MOSS_HELPER").map(PathBuf::from) {
                if let Some(path) = secure_existing_file(&path) {
                    return Some(path);
                }
            }
        }
        if let Ok(executable) = std::env::current_exe() {
            if let Some(directory) = executable.parent() {
                for name in ["moss-helper-x86_64-pc-windows-msvc.exe", "moss-helper.exe"] {
                    let candidate = directory.join(name);
                    if let Some(candidate) = secure_existing_file(&candidate) {
                        return Some(candidate);
                    }
                }
            }
        }
        if cfg!(debug_assertions) {
            let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let root = manifest.parent()?.parent()?;
            for candidate in [
                root.join("target").join("release").join("moss-helper.exe"),
                root.join("target").join("debug").join("moss-helper.exe"),
            ] {
                if let Some(candidate) = secure_existing_file(&candidate) {
                    return Some(candidate);
                }
            }
        }
        None
    }

    pub fn status(&self) -> ManagerStatus {
        let active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut active_request_ids: Vec<_> = active.keys().cloned().collect();
        let mut active_process_ids: Vec<_> =
            active.values().map(|task| task.job.process_id()).collect();
        drop(active);
        if active_request_ids.is_empty() {
            if let Some(reservation) = self
                .reservation
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
            {
                active_request_ids.push(reservation.request_id);
            }
        }
        active_request_ids.sort();
        active_process_ids.sort_unstable();
        ManagerStatus {
            active_request_ids,
            active_process_ids,
            helper_processes_started: self.helper_processes_started.load(Ordering::SeqCst),
        }
    }

    pub fn probe(
        &self,
        runtime_directory: PathBuf,
        request_id: String,
        device_id: Option<String>,
        timeout: Duration,
    ) -> Result<ProbeResultMessage, ManagerError> {
        let helper = self.helper_path.as_ref().ok_or(ManagerError::Unavailable)?;
        self.reserve(&request_id)?;
        let result = probe_with_helper(
            helper,
            runtime_directory,
            request_id.clone(),
            device_id,
            timeout,
        );
        self.finish_reserved(&request_id, result)
    }

    pub fn transcribe(&self, input: TranscribeInput) -> Result<ManagedTranscription, ManagerError> {
        self.transcribe_from_started(input, Instant::now())
    }

    pub(crate) fn transcribe_from_started(
        &self,
        input: TranscribeInput,
        supervisor_started: Instant,
    ) -> Result<ManagedTranscription, ManagerError> {
        self.transcribe_inner(input, supervisor_started, true, None)
    }

    pub(crate) fn begin_preparation(
        self: &Arc<Self>,
        request_id: &str,
    ) -> Result<PreparationLease, ManagerError> {
        if !moss_helper::protocol::is_uuid(request_id) {
            return Err(ManagerError::InvalidRequest);
        }
        let state = self.reserve(request_id)?;
        Ok(PreparationLease {
            manager: self.clone(),
            request_id: request_id.to_string(),
            state,
        })
    }

    pub(crate) fn transcribe_prepared(
        &self,
        input: TranscribeInput,
        supervisor_started: Instant,
        preparation: &PreparationLease,
    ) -> Result<ManagedTranscription, ManagerError> {
        if input.request_id != preparation.request_id {
            return Err(ManagerError::InvalidRequest);
        }
        self.transcribe_inner(
            input,
            supervisor_started,
            false,
            Some(preparation.state.clone()),
        )
    }

    fn transcribe_inner(
        &self,
        input: TranscribeInput,
        supervisor_started: Instant,
        reserve_request: bool,
        preparation_state: Option<Arc<std::sync::atomic::AtomicU8>>,
    ) -> Result<ManagedTranscription, ManagerError> {
        validate_input(&input)?;
        if preparation_state
            .as_ref()
            .is_some_and(|value| value.load(Ordering::SeqCst) == RESERVATION_CANCELLED)
        {
            return Err(ManagerError::Cancelled);
        }
        if supervisor_started.elapsed() >= input.timeout {
            return Err(ManagerError::Timeout);
        }
        let audio_duration_seconds =
            input.samples.len() as f64 / input.sample_rate_hz as f64 / input.channels as f64;
        let effective_max_wall_rtf =
            effective_max_wall_rtf(audio_duration_seconds, input.max_wall_rtf);
        let helper = self.helper_path.as_ref().ok_or(ManagerError::Unavailable)?;
        let reservation_state = if reserve_request {
            Some(self.reserve(&input.request_id)?)
        } else {
            None
        };
        let cancellation = preparation_state.or(reservation_state);
        let result = (|| {
            if cancellation
                .as_ref()
                .is_some_and(|value| value.load(Ordering::SeqCst) == RESERVATION_CANCELLED)
            {
                return Err(ManagerError::Cancelled);
            }
            let maximum_chunk_samples = usize::try_from(MAX_NATIVE_CHUNK_SECONDS)
                .ok()
                .and_then(|seconds| seconds.checked_mul(16_000))
                .ok_or(ManagerError::InvalidRequest)?;
            let mut chunks = Vec::new();
            for (chunk_index, (chunk_start_sample, chunk_end_sample)) in
                native_chunk_ranges(input.samples.len(), maximum_chunk_samples)?
                    .into_iter()
                    .enumerate()
            {
                let samples = &input.samples[chunk_start_sample..chunk_end_sample];
                let output_offset_ms = u64::try_from(chunk_start_sample)
                    .ok()
                    .and_then(|value| value.checked_mul(1_000))
                    .map(|value| value / 16_000)
                    .ok_or(ManagerError::InvalidRequest)?;
                if cancellation
                    .as_ref()
                    .is_some_and(|value| value.load(Ordering::SeqCst) == RESERVATION_CANCELLED)
                {
                    return Err(ManagerError::Cancelled);
                }
                if supervisor_started.elapsed() >= input.timeout {
                    return Err(ManagerError::Timeout);
                }
                let staged = if let Some(cancelled) = &cancellation {
                    match stage_interleaved_with_cancel(
                        &self.cache_root,
                        &input.request_id,
                        samples,
                        input.sample_rate_hz,
                        input.channels,
                        || {
                            cancelled.load(Ordering::SeqCst) == RESERVATION_CANCELLED
                                || supervisor_started.elapsed() >= input.timeout
                        },
                    ) {
                        Ok(value) => value,
                        Err(StageError::Cancelled)
                            if cancelled.load(Ordering::SeqCst) == RESERVATION_CANCELLED =>
                        {
                            return Err(ManagerError::Cancelled)
                        }
                        Err(StageError::Cancelled) => return Err(ManagerError::Timeout),
                        Err(error) => return Err(error.into()),
                    }
                } else {
                    stage_interleaved(
                        &self.cache_root,
                        &input.request_id,
                        samples,
                        input.sample_rate_hz,
                        input.channels,
                    )?
                };
                if cancellation
                    .as_ref()
                    .is_some_and(|value| value.load(Ordering::SeqCst) == RESERVATION_CANCELLED)
                {
                    staged.cleanup()?;
                    return Err(ManagerError::Cancelled);
                }
                if supervisor_started.elapsed() >= input.timeout {
                    staged.cleanup()?;
                    return Err(ManagerError::Timeout);
                }
                let chunk_duration_ms = staged
                    .spec
                    .samples
                    .checked_mul(1_000)
                    .map(|value| value / 16_000)
                    .ok_or(ManagerError::InvalidRequest)?;
                let mut staged_activity = staged.activity;
                staged_activity.audio_duration_ms = staged_activity
                    .audio_duration_ms
                    .checked_add(output_offset_ms)
                    .ok_or(ManagerError::InvalidRequest)?;
                staged_activity.first_active_ms = match staged_activity.first_active_ms {
                    Some(value) => Some(
                        value
                            .checked_add(output_offset_ms)
                            .ok_or(ManagerError::InvalidRequest)?,
                    ),
                    None => None,
                };
                staged_activity.last_active_ms = match staged_activity.last_active_ms {
                    Some(value) => Some(
                        value
                            .checked_add(output_offset_ms)
                            .ok_or(ManagerError::InvalidRequest)?,
                    ),
                    None => None,
                };
                #[cfg(test)]
                let chunk_input = p2d_faulted_chunk_input(&input, chunk_index);
                #[cfg(not(test))]
                let chunk_input: std::borrow::Cow<'_, TranscribeInput> =
                    std::borrow::Cow::Borrowed(&input);
                let result = self.transcribe_staged(
                    helper,
                    chunk_input.as_ref(),
                    staged.spec.clone(),
                    supervisor_started,
                    cancellation.as_ref(),
                );
                let cleanup = staged.cleanup();
                let mut value = match (result, cleanup) {
                    (Ok(value), Ok(())) => value,
                    (Err(error), _) => return Err(error),
                    (Ok(_), Err(_)) => return Err(ManagerError::AudioStage),
                };
                value.audio_activity = Some(staged_activity);
                value.helper_runs.push(ManagedHelperRun {
                    chunk_index: chunk_index
                        .try_into()
                        .map_err(|_| ManagerError::InvalidRequest)?,
                    input_offset_ms: output_offset_ms,
                    input_duration_ms: chunk_duration_ms,
                    process_id: value.helper_process_id,
                    total_processes: value.helper_total_processes,
                    peak_job_memory_bytes: value.helper_peak_job_memory_bytes,
                    residual_process_count: value.residual_process_count,
                    terminal_count: 1,
                    last_timestamp_ms: value.completed.last_timestamp_ms,
                    native_rtf: value.completed.native_rtf,
                    raw_text_sha256: value.completed.raw_text_sha256.clone(),
                    clean_text_sha256: value.completed.clean_text_sha256.clone(),
                });
                chunks.push(value);
            }
            let value = merge_chunk_results(
                &input.request_id,
                &input.context_sha256,
                chunks,
                audio_duration_seconds,
            )?;
            finalize_supervisor_metrics(
                value,
                supervisor_started.elapsed(),
                audio_duration_seconds,
                effective_max_wall_rtf,
            )
        })();
        self.finish_reserved(&input.request_id, result)
    }

    fn transcribe_staged(
        &self,
        helper: &Path,
        input: &TranscribeInput,
        audio: moss_helper::protocol::AudioSpec,
        supervisor_started: Instant,
        preparation_state: Option<&Arc<std::sync::atomic::AtomicU8>>,
    ) -> Result<ManagedTranscription, ManagerError> {
        let process = spawn_suspended_assigned(helper, &[] as &[OsString])?;
        self.helper_processes_started.fetch_add(1, Ordering::SeqCst);
        let control = Arc::new(TaskControl::new(
            input.request_id.clone(),
            process.control.clone(),
            process.stdin,
        ));
        self.register(control.clone())?;
        if preparation_state
            .is_some_and(|value| value.load(Ordering::SeqCst) == RESERVATION_CANCELLED)
        {
            control.request_cancel(CancelReason::User)?;
        }

        let result = (|| {
            let runtime_directory = input
                .runtime_directory
                .to_str()
                .ok_or(ManagerError::InvalidRequest)?
                .to_owned();
            let model_path = input
                .model_path
                .to_str()
                .ok_or(ManagerError::InvalidRequest)?
                .to_owned();
            drain_stderr(process.stderr);
            let receiver = read_stdout(process.stdout);
            let mut protocol = TranscribeProtocol::new(
                input.request_id.clone(),
                input.context_sha256.clone(),
                audio.samples,
                input.language_requested.clone(),
                input.decode_parameters_json.clone(),
                input.decode_parameters_sha256.clone(),
            );
            let deadline = supervisor_started + input.timeout;

            let hello = receive_until(&receiver, deadline, &control)?;
            protocol.accept(hello)?;

            control.send(ClientMessage::Transcribe(TranscribeCommand {
                v: PROTOCOL_VERSION,
                request_id: input.request_id.clone(),
                client_seq: 1,
                context_sha256: input.context_sha256.clone(),
                runtime: RuntimeSpec {
                    directory: runtime_directory,
                },
                device: exact_device(input.device_id.clone()),
                model: ModelSpec {
                    path: model_path,
                    bytes: input.model_bytes,
                    sha256: input.model_sha256.clone(),
                },
                audio,
                language_requested: input.language_requested.clone(),
                decode_parameters_json: input.decode_parameters_json.clone(),
                decode_parameters_sha256: input.decode_parameters_sha256.clone(),
            }))?;

            if let Some(state) = preparation_state {
                if state.load(Ordering::SeqCst) == RESERVATION_CANCELLED
                    && !control.cancellation_requested.load(Ordering::SeqCst)
                {
                    control.request_cancel(CancelReason::User)?;
                }
            }

            loop {
                let message = receive_until(&receiver, deadline, &control)?;
                match protocol.accept(message)? {
                    ProtocolOutcome::Continue => {}
                    ProtocolOutcome::Completed(value) => {
                        control.mark_terminal();
                        ensure_clean_terminal(&receiver, &control)?;
                        let accounting = control
                            .job
                            .accounting()
                            .map_err(|_| ManagerError::Protocol)?;
                        if accounting.active_processes != 0 {
                            return Err(ManagerError::Protocol);
                        }
                        let mut value = *value;
                        value.helper_process_id = control.job.process_id();
                        value.helper_total_processes = accounting.total_processes;
                        value.helper_peak_job_memory_bytes = accounting.peak_job_memory_bytes;
                        value.residual_process_count = accounting.active_processes;
                        return Ok(value);
                    }
                    ProtocolOutcome::Cancelled => {
                        control.mark_terminal();
                        ensure_clean_terminal(&receiver, &control)?;
                        return Err(ManagerError::Cancelled);
                    }
                    ProtocolOutcome::Failed(code) => {
                        control.mark_terminal();
                        ensure_clean_terminal(&receiver, &control)?;
                        return Err(ManagerError::Native(code));
                    }
                }
            }
        })();

        if result.is_err() {
            let _ = settle_or_terminate(&control.job);
        }
        self.unregister(&input.request_id);
        result
    }

    pub fn cancel(&self, request_id: &str) -> Result<bool, ManagerError> {
        let task = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(request_id)
            .cloned();
        {
            let reservation = self
                .reservation
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(reservation) = reservation
                .as_ref()
                .filter(|reservation| reservation.request_id == request_id)
            else {
                return Ok(false);
            };
            if reservation.state.load(Ordering::SeqCst) != RESERVATION_RUNNING {
                return Ok(false);
            }
            reservation
                .state
                .store(RESERVATION_CANCELLED, Ordering::SeqCst);
        }
        if let Some(task) = task {
            if !task.terminal.load(Ordering::SeqCst) {
                match task.request_cancel(CancelReason::User) {
                    Ok(()) => {
                        let job = task.job.clone();
                        std::thread::spawn(move || {
                            let _ = settle_or_terminate(&job);
                        });
                    }
                    Err(error) if !task.terminal.load(Ordering::SeqCst) => return Err(error),
                    Err(_) => {}
                }
            }
        }
        Ok(true)
    }

    pub fn shutdown_all(&self) {
        let tasks: Vec<_> = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .cloned()
            .collect();
        for task in tasks {
            let _ = task.request_cancel(CancelReason::Shutdown);
            let _ = settle_or_terminate(&task.job);
        }
    }

    fn register(&self, task: Arc<TaskControl>) -> Result<(), ManagerError> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !active.is_empty() {
            let _ = task.job.terminate_and_confirm(GRACE_PERIOD);
            return if active.contains_key(&task.request_id) {
                Err(ManagerError::DuplicateRequest)
            } else {
                Err(ManagerError::Busy)
            };
        }
        active.insert(task.request_id.clone(), task);
        Ok(())
    }

    fn unregister(&self, request_id: &str) {
        self.active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(request_id);
    }

    fn reserve(&self, request_id: &str) -> Result<Arc<std::sync::atomic::AtomicU8>, ManagerError> {
        let mut reservation = self
            .reservation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match reservation.as_ref() {
            None => {
                let state = Arc::new(std::sync::atomic::AtomicU8::new(RESERVATION_RUNNING));
                *reservation = Some(Reservation {
                    request_id: request_id.to_string(),
                    state: state.clone(),
                });
                Ok(state)
            }
            Some(active) if active.request_id == request_id => Err(ManagerError::DuplicateRequest),
            Some(_) => Err(ManagerError::Busy),
        }
    }

    fn release_reservation(&self, request_id: &str) {
        let mut reservation = self
            .reservation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if reservation
            .as_ref()
            .is_some_and(|reservation| reservation.request_id == request_id)
        {
            *reservation = None;
        }
    }

    fn finish_reserved<T>(
        &self,
        request_id: &str,
        result: Result<T, ManagerError>,
    ) -> Result<T, ManagerError> {
        let mut reservation = self
            .reservation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(current) = reservation
            .as_ref()
            .filter(|current| current.request_id == request_id)
        else {
            return Err(ManagerError::Protocol);
        };
        let outcome = match result {
            Ok(_) if current.state.load(Ordering::SeqCst) == RESERVATION_CANCELLED => {
                Err(ManagerError::Cancelled)
            }
            Ok(value) => {
                current.state.store(RESERVATION_COMMITTED, Ordering::SeqCst);
                Ok(value)
            }
            Err(error) => Err(error),
        };
        *reservation = None;
        outcome
    }
}

#[cfg(test)]
fn p2d_faulted_chunk_input(
    input: &TranscribeInput,
    chunk_index: usize,
) -> std::borrow::Cow<'_, TranscribeInput> {
    if chunk_index == 1 && std::env::var_os("MOSS_P2D_FAIL_SECOND_CHUNK").is_some() {
        let mut faulted = input.clone();
        faulted.model_bytes = faulted.model_bytes.saturating_add(1);
        return std::borrow::Cow::Owned(faulted);
    }
    std::borrow::Cow::Borrowed(input)
}

impl Drop for MossHelperManager {
    fn drop(&mut self) {
        self.shutdown_all();
    }
}

struct TaskControl {
    request_id: String,
    job: Arc<JobControl>,
    writer: Mutex<ControlWriter>,
    terminal: AtomicBool,
    cancellation_requested: AtomicBool,
}

struct ControlWriter {
    stdin: Option<BufWriter<File>>,
    next_client_seq: u64,
    pending_cancel: Option<CancelReason>,
}

impl TaskControl {
    fn new(request_id: String, job: Arc<JobControl>, stdin: File) -> Self {
        Self {
            request_id,
            job,
            writer: Mutex::new(ControlWriter {
                stdin: Some(BufWriter::new(stdin)),
                next_client_seq: 1,
                pending_cancel: None,
            }),
            terminal: AtomicBool::new(false),
            cancellation_requested: AtomicBool::new(false),
        }
    }

    fn send(&self, message: ClientMessage) -> Result<(), ManagerError> {
        if self.terminal.load(Ordering::SeqCst) {
            return Err(ManagerError::Protocol);
        }
        let mut state = self
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if message.client_seq() != state.next_client_seq || message.request_id() != self.request_id
        {
            return Err(ManagerError::Protocol);
        }
        let writer = state.stdin.as_mut().ok_or(ManagerError::Protocol)?;
        write_client_message(writer, &message).map_err(|_| ManagerError::Protocol)?;
        state.next_client_seq = state
            .next_client_seq
            .checked_add(1)
            .ok_or(ManagerError::Protocol)?;
        if matches!(message, ClientMessage::Transcribe(_)) {
            if let Some(reason) = state.pending_cancel.take() {
                let cancel = ClientMessage::Cancel(CancelCommand {
                    v: PROTOCOL_VERSION,
                    request_id: self.request_id.clone(),
                    client_seq: state.next_client_seq,
                    reason,
                });
                let writer = state.stdin.as_mut().ok_or(ManagerError::Protocol)?;
                write_client_message(writer, &cancel).map_err(|_| ManagerError::Protocol)?;
                state.next_client_seq = state
                    .next_client_seq
                    .checked_add(1)
                    .ok_or(ManagerError::Protocol)?;
            }
        }
        Ok(())
    }

    fn request_cancel(self: &Arc<Self>, reason: CancelReason) -> Result<(), ManagerError> {
        if self.terminal.load(Ordering::SeqCst) {
            return Err(ManagerError::Protocol);
        }
        let mut state = self
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.cancellation_requested.load(Ordering::SeqCst) {
            return Err(ManagerError::Protocol);
        }
        if state.next_client_seq == 1 {
            state.pending_cancel = Some(reason);
            self.cancellation_requested.store(true, Ordering::SeqCst);
            return Ok(());
        }
        let cancel = ClientMessage::Cancel(CancelCommand {
            v: PROTOCOL_VERSION,
            request_id: self.request_id.clone(),
            client_seq: state.next_client_seq,
            reason,
        });
        let writer = state.stdin.as_mut().ok_or(ManagerError::Protocol)?;
        write_client_message(writer, &cancel).map_err(|_| ManagerError::Protocol)?;
        state.next_client_seq = state
            .next_client_seq
            .checked_add(1)
            .ok_or(ManagerError::Protocol)?;
        self.cancellation_requested.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn mark_terminal(&self) {
        self.terminal.store(true, Ordering::SeqCst);
        self.writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .stdin
            .take();
    }
}

enum PipeEvent {
    Message(Box<ServerMessage>),
    Eof,
    Invalid(ErrorCode),
}

fn read_stdout(stdout: File) -> std::sync::mpsc::Receiver<PipeEvent> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match read_server_message(&mut reader) {
                Ok(message) => {
                    if sender.send(PipeEvent::Message(Box::new(message))).is_err() {
                        return;
                    }
                }
                Err(moss_helper::protocol::ProtocolError::Eof) => {
                    let _ = sender.send(PipeEvent::Eof);
                    return;
                }
                Err(error) => {
                    let _ = sender.send(PipeEvent::Invalid(error.code()));
                    return;
                }
            }
        }
    });
    receiver
}

fn drain_stderr(mut stderr: File) {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        let mut total = 0u64;
        loop {
            match stderr.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => total = total.saturating_add(read as u64),
                Err(_) => break,
            }
        }
        if total != 0 {
            log::warn!(
                "MOSS helper emitted a redacted diagnostic ({} bytes)",
                total
            );
        }
    });
}

fn receive_until(
    receiver: &std::sync::mpsc::Receiver<PipeEvent>,
    deadline: Instant,
    control: &Arc<TaskControl>,
) -> Result<ServerMessage, ManagerError> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            if !control.cancellation_requested.load(Ordering::SeqCst) {
                let _ = control.request_cancel(CancelReason::Timeout);
            }
            let _ = settle_or_terminate(&control.job);
            return Err(ManagerError::Timeout);
        }
        let wait = (deadline - now).min(PIPE_EVENT_POLL);
        match receiver.recv_timeout(wait) {
            Ok(PipeEvent::Message(message)) => return Ok(*message),
            Ok(PipeEvent::Eof) => {
                #[cfg(test)]
                {
                    let _ = control.job.wait(Duration::from_millis(100));
                    eprintln!(
                        "MOSS protocol rejection: unexpected EOF exit_code={:?}",
                        control.job.exit_code().ok().flatten()
                    );
                }
                let _ = control.job.terminate_and_confirm(GRACE_PERIOD);
                return if control.cancellation_requested.load(Ordering::SeqCst) {
                    Err(ManagerError::Cancelled)
                } else {
                    Err(ManagerError::Protocol)
                };
            }
            Ok(PipeEvent::Invalid(_code)) => {
                #[cfg(test)]
                eprintln!("MOSS protocol rejection: invalid JSONL code={_code:?}");
                let _ = control.job.terminate_and_confirm(GRACE_PERIOD);
                return if control.cancellation_requested.load(Ordering::SeqCst) {
                    Err(ManagerError::Cancelled)
                } else {
                    Err(ManagerError::Protocol)
                };
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let _ = control.job.terminate_and_confirm(GRACE_PERIOD);
                return Err(ManagerError::Protocol);
            }
        }
    }
}

fn settle_or_terminate(job: &JobControl) -> Result<(), JobError> {
    if job.wait(GRACE_PERIOD)? && job.confirm_zero(GRACE_PERIOD).is_ok() {
        return Ok(());
    }
    job.terminate_and_confirm(GRACE_PERIOD)
}

fn ensure_clean_terminal(
    receiver: &std::sync::mpsc::Receiver<PipeEvent>,
    control: &Arc<TaskControl>,
) -> Result<(), ManagerError> {
    match receiver.recv_timeout(GRACE_PERIOD) {
        Ok(PipeEvent::Eof) => {
            if !control
                .job
                .wait(GRACE_PERIOD)
                .map_err(|_| ManagerError::Protocol)?
            {
                control
                    .job
                    .terminate_and_confirm(GRACE_PERIOD)
                    .map_err(|_| ManagerError::Protocol)?;
                return Ok(());
            }
            if control.job.confirm_zero(GRACE_PERIOD).is_err() {
                control
                    .job
                    .terminate_and_confirm(GRACE_PERIOD)
                    .map_err(|_| ManagerError::Protocol)?;
            }
            Ok(())
        }
        Ok(PipeEvent::Message(_)) | Ok(PipeEvent::Invalid(_)) | Err(_) => {
            let _ = control.job.terminate_and_confirm(GRACE_PERIOD);
            Err(ManagerError::Protocol)
        }
    }
}

enum ProtocolOutcome {
    Continue,
    Completed(Box<ManagedTranscription>),
    Cancelled,
    Failed(ErrorCode),
}

struct SegmentAssembly {
    t0_ms: i64,
    t1_ms: i64,
    speaker_id: i32,
    next_part: u32,
    complete: bool,
    text: String,
}

struct TranscribeProtocol {
    request_id: String,
    context_sha256: String,
    language_requested: String,
    decode_parameters_json: String,
    decode_parameters_sha256: String,
    next_seq: u64,
    saw_hello: bool,
    saw_accepted: bool,
    terminal: bool,
    phase: Option<TaskPhase>,
    last_heartbeat_ms: u64,
    heartbeat_count: u64,
    raw_text: String,
    clean_text: String,
    raw_next_part: u32,
    clean_next_part: u32,
    raw_complete: bool,
    clean_complete: bool,
    segments: BTreeMap<u32, SegmentAssembly>,
    maximum_timestamp_ms: i64,
    audio_duration_seconds: f64,
}

impl TranscribeProtocol {
    fn new(
        request_id: String,
        context_sha256: String,
        audio_samples: u64,
        language_requested: String,
        decode_parameters_json: String,
        decode_parameters_sha256: String,
    ) -> Self {
        let duration_ms = audio_samples.saturating_mul(1_000) / 16_000;
        let maximum_timestamp_ms = duration_ms.saturating_add(49) / 50 * 50;
        Self {
            request_id,
            context_sha256,
            language_requested,
            decode_parameters_json,
            decode_parameters_sha256,
            next_seq: 0,
            saw_hello: false,
            saw_accepted: false,
            terminal: false,
            phase: None,
            last_heartbeat_ms: 0,
            heartbeat_count: 0,
            raw_text: String::new(),
            clean_text: String::new(),
            raw_next_part: 0,
            clean_next_part: 0,
            raw_complete: false,
            clean_complete: false,
            segments: BTreeMap::new(),
            maximum_timestamp_ms: maximum_timestamp_ms.try_into().unwrap_or(i64::MAX),
            audio_duration_seconds: audio_samples as f64 / 16_000.0,
        }
    }

    fn accept(&mut self, message: ServerMessage) -> Result<ProtocolOutcome, ManagerError> {
        if self.terminal
            || message_v(&message) != PROTOCOL_VERSION
            || message_seq(&message) != self.next_seq
        {
            #[cfg(test)]
            eprintln!(
                "MOSS protocol rejection: header terminal={} version={} seq={} expected_seq={}",
                self.terminal,
                message_v(&message),
                message_seq(&message),
                self.next_seq
            );
            return Err(ManagerError::Protocol);
        }
        self.next_seq = self.next_seq.checked_add(1).ok_or(ManagerError::Protocol)?;

        if !self.saw_hello {
            return match message {
                ServerMessage::Hello(hello)
                    if hello.seq == 0
                        && hello.capabilities.iter().any(|value| value == "transcribe") =>
                {
                    self.saw_hello = true;
                    Ok(ProtocolOutcome::Continue)
                }
                _ => Err(ManagerError::Protocol),
            };
        }
        if !self.saw_accepted {
            return match message {
                ServerMessage::Accepted(AcceptedMessage {
                    request_id,
                    operation: Operation::Transcribe,
                    context_sha256: Some(context),
                    ..
                }) if request_id == self.request_id && context == self.context_sha256 => {
                    self.saw_accepted = true;
                    Ok(ProtocolOutcome::Continue)
                }
                _ => Err(ManagerError::Protocol),
            };
        }

        ensure_request_id(&message, &self.request_id)?;
        match message {
            ServerMessage::Status(status) => {
                if status.context_sha256 != self.context_sha256
                    || !phase_advances(self.phase, status.phase)
                {
                    #[cfg(test)]
                    eprintln!("MOSS protocol rejection: status transition");
                    return Err(ManagerError::Protocol);
                }
                self.phase = Some(status.phase);
                Ok(ProtocolOutcome::Continue)
            }
            ServerMessage::Heartbeat(heartbeat) => {
                if heartbeat.context_sha256 != self.context_sha256
                    || heartbeat.phase != TaskPhase::NativeRunning
                    || self.phase != Some(TaskPhase::NativeRunning)
                    || heartbeat.elapsed_ms < self.last_heartbeat_ms
                {
                    #[cfg(test)]
                    eprintln!("MOSS protocol rejection: heartbeat");
                    return Err(ManagerError::Protocol);
                }
                self.last_heartbeat_ms = heartbeat.elapsed_ms;
                self.heartbeat_count += 1;
                Ok(ProtocolOutcome::Continue)
            }
            ServerMessage::TextChunk(chunk) => {
                if self.phase != Some(TaskPhase::ResultStreaming) {
                    #[cfg(test)]
                    eprintln!("MOSS protocol rejection: text phase");
                    return Err(ManagerError::Protocol);
                }
                let (text, next_part, complete) = match chunk.stream {
                    TextStream::Raw => (
                        &mut self.raw_text,
                        &mut self.raw_next_part,
                        &mut self.raw_complete,
                    ),
                    TextStream::Clean => (
                        &mut self.clean_text,
                        &mut self.clean_next_part,
                        &mut self.clean_complete,
                    ),
                };
                if *complete || chunk.part != *next_part {
                    #[cfg(test)]
                    eprintln!("MOSS protocol rejection: text sequence");
                    return Err(ManagerError::Protocol);
                }
                text.push_str(&chunk.text);
                *next_part += 1;
                *complete = chunk.last;
                Ok(ProtocolOutcome::Continue)
            }
            ServerMessage::Segment(segment) => {
                if self.phase != Some(TaskPhase::ResultStreaming) {
                    #[cfg(test)]
                    eprintln!("MOSS protocol rejection: segment phase");
                    return Err(ManagerError::Protocol);
                }
                let entry = self
                    .segments
                    .entry(segment.segment_index)
                    .or_insert(SegmentAssembly {
                        t0_ms: segment.t0_ms,
                        t1_ms: segment.t1_ms,
                        speaker_id: segment.speaker_id,
                        next_part: 0,
                        complete: false,
                        text: String::new(),
                    });
                if entry.complete
                    || entry.next_part != segment.text_part
                    || entry.t0_ms != segment.t0_ms
                    || entry.t1_ms != segment.t1_ms
                    || entry.speaker_id != segment.speaker_id
                {
                    #[cfg(test)]
                    eprintln!("MOSS protocol rejection: segment sequence");
                    return Err(ManagerError::Protocol);
                }
                entry.text.push_str(&segment.text);
                entry.next_part += 1;
                entry.complete = segment.text_last;
                Ok(ProtocolOutcome::Continue)
            }
            ServerMessage::Completed(completed) => {
                let envelope_invalid = completed.context_sha256 != self.context_sha256
                    || completed.language_requested != self.language_requested
                    || completed.language_resolved != moss_helper::native::MOSS_LANGUAGE_RESOLVED
                    || completed.decode_parameters_json != self.decode_parameters_json
                    || !completed
                        .decode_parameters_sha256
                        .eq_ignore_ascii_case(&self.decode_parameters_sha256)
                    || !completed
                        .decode_parameters_sha256
                        .eq_ignore_ascii_case(&format!(
                            "{:x}",
                            Sha256::digest(completed.decode_parameters_json.as_bytes())
                        ))
                    || !completed.terminal
                    || self.phase != Some(TaskPhase::ResultStreaming)
                    || !self.raw_complete
                    || !self.clean_complete
                    || self.raw_text.trim().is_empty()
                    || self.clean_text.trim().is_empty()
                    || self.segments.is_empty()
                    || self.segments.values().any(|segment| !segment.complete)
                    || completed.segment_count as usize != self.segments.len()
                    || completed.segment_count as usize > 100_000
                    || completed.device_description != "Intel(R) Arc(TM) Graphics"
                    || completed.backend != "Vulkan0"
                    || completed.was_aborted
                    || completed.was_truncated
                    || !completed.native_rtf.is_finite()
                    || completed.native_rtf < 0.0
                    || !completed.wall_rtf.is_finite()
                    || completed.wall_rtf < 0.0
                    || completed.wall_elapsed_ms < completed.native_run_elapsed_ms
                    || completed.native_session_limits.effective_n_ctx
                        != moss_helper::native::MOSS_SESSION_N_CTX
                    || completed.native_session_limits.effective_max_audio_ms
                        < (self.audio_duration_seconds * 1_000.0).ceil() as i64
                    || completed.native_session_limits.max_kv_bytes <= 0
                    || [
                        completed.native_timings.load_ms,
                        completed.native_timings.mel_ms,
                        completed.native_timings.encode_ms,
                        completed.native_timings.decode_ms,
                    ]
                    .iter()
                    .any(|value| !value.is_finite() || *value < 0.0);
                if envelope_invalid {
                    #[cfg(test)]
                    eprintln!("MOSS protocol rejection: completed envelope");
                    return Err(ManagerError::Protocol);
                }
                if !completed
                    .raw_text_sha256
                    .eq_ignore_ascii_case(&sha256_text(&self.raw_text))
                    || !completed
                        .clean_text_sha256
                        .eq_ignore_ascii_case(&sha256_text(&self.clean_text))
                {
                    #[cfg(test)]
                    eprintln!("MOSS protocol rejection: completed text hash");
                    return Err(ManagerError::Protocol);
                }
                let recomputed_wall_rtf =
                    completed.wall_elapsed_ms as f64 / 1_000.0 / self.audio_duration_seconds;
                if (completed.wall_rtf - recomputed_wall_rtf).abs() > 1e-9 {
                    #[cfg(test)]
                    eprintln!("MOSS protocol rejection: completed helper RTF");
                    return Err(ManagerError::Protocol);
                }
                let mut previous_t0 = 0_i64;
                let mut maximum_t1 = 0_i64;
                for (expected_index, (actual_index, segment)) in self.segments.iter().enumerate() {
                    if *actual_index != expected_index as u32
                        || segment.t0_ms < 0
                        || segment.t1_ms < segment.t0_ms
                        || segment.t0_ms < previous_t0
                        || segment.t1_ms > self.maximum_timestamp_ms
                        || segment.speaker_id < 0
                        || segment.text.trim().is_empty()
                    {
                        #[cfg(test)]
                        eprintln!("MOSS protocol rejection: completed segment metadata");
                        return Err(ManagerError::Protocol);
                    }
                    previous_t0 = segment.t0_ms;
                    maximum_t1 = maximum_t1.max(segment.t1_ms);
                }
                if completed.last_timestamp_ms != maximum_t1 {
                    #[cfg(test)]
                    eprintln!("MOSS protocol rejection: completed last timestamp");
                    return Err(ManagerError::Protocol);
                }
                self.terminal = true;
                let segments = std::mem::take(&mut self.segments)
                    .into_iter()
                    .map(|(segment_index, value)| ManagedSegment {
                        segment_index,
                        t0_ms: value.t0_ms,
                        t1_ms: value.t1_ms,
                        speaker_id: value.speaker_id,
                        text: value.text,
                    })
                    .collect();
                let language_requested = completed.language_requested.clone();
                let language_resolved = completed.language_resolved.clone();
                let decode_parameters_json = completed.decode_parameters_json.clone();
                let decode_parameters_sha256 = completed.decode_parameters_sha256.clone();
                Ok(ProtocolOutcome::Completed(Box::new(ManagedTranscription {
                    request_id: self.request_id.clone(),
                    context_sha256: self.context_sha256.clone(),
                    raw_text: std::mem::take(&mut self.raw_text),
                    clean_text: std::mem::take(&mut self.clean_text),
                    segments,
                    completed,
                    heartbeat_count: self.heartbeat_count,
                    supervisor_wall_elapsed_ms: 0,
                    supervisor_wall_rtf: 0.0,
                    helper_process_id: 0,
                    helper_total_processes: 0,
                    helper_peak_job_memory_bytes: 0,
                    residual_process_count: u32::MAX,
                    helper_runs: Vec::new(),
                    audio_activity: None,
                    language_requested,
                    language_resolved,
                    decode_parameters_json,
                    decode_parameters_sha256,
                })))
            }
            ServerMessage::Cancelled(cancelled) if cancelled.terminal => {
                self.terminal = true;
                Ok(ProtocolOutcome::Cancelled)
            }
            ServerMessage::Failed(failed) if failed.terminal => {
                self.terminal = true;
                Ok(ProtocolOutcome::Failed(failed.code))
            }
            _ => Err(ManagerError::Protocol),
        }
    }
}

fn sha256_text(value: &str) -> String {
    format!("{:X}", Sha256::digest(value.as_bytes()))
}

fn finalize_supervisor_metrics(
    mut value: ManagedTranscription,
    elapsed: Duration,
    audio_duration_seconds: f64,
    max_wall_rtf: Option<f64>,
) -> Result<ManagedTranscription, ManagerError> {
    value.supervisor_wall_elapsed_ms = elapsed.as_millis().try_into().unwrap_or(u64::MAX);
    value.supervisor_wall_rtf = elapsed.as_secs_f64() / audio_duration_seconds;
    if max_wall_rtf.is_some_and(|maximum| value.supervisor_wall_rtf > maximum) {
        Err(ManagerError::PerformanceGate)
    } else {
        Ok(value)
    }
}

fn native_chunk_ranges(
    total_samples: usize,
    maximum_chunk_samples: usize,
) -> Result<Vec<(usize, usize)>, ManagerError> {
    if total_samples == 0 || maximum_chunk_samples == 0 {
        return Err(ManagerError::InvalidRequest);
    }
    let mut ranges = Vec::new();
    let mut start = 0usize;
    while start < total_samples {
        let end = start
            .checked_add(maximum_chunk_samples)
            .map(|value| value.min(total_samples))
            .ok_or(ManagerError::InvalidRequest)?;
        ranges.push((start, end));
        start = end;
    }
    Ok(ranges)
}

fn merge_chunk_results(
    request_id: &str,
    context_sha256: &str,
    chunks: Vec<ManagedTranscription>,
    audio_duration_seconds: f64,
) -> Result<ManagedTranscription, ManagerError> {
    if chunks.is_empty() || !audio_duration_seconds.is_finite() || audio_duration_seconds <= 0.0 {
        return Err(ManagerError::Protocol);
    }
    let multiple_chunks = chunks.len() > 1;

    let mut raw_text = String::new();
    let mut clean_text = String::new();
    let mut segments = Vec::new();
    let mut helper_runs = Vec::new();
    let mut heartbeat_count = 0u64;
    let mut native_run_elapsed_ms = 0u64;
    let mut helper_wall_elapsed_ms = 0u64;
    let mut native_load_ms = 0.0f32;
    let mut native_mel_ms = 0.0f32;
    let mut native_encode_ms = 0.0f32;
    let mut native_decode_ms = 0.0f32;
    let mut helper_total_processes = 0u32;
    let mut helper_peak_job_memory_bytes = 0u64;
    let mut residual_process_count = 0u32;
    let mut final_template = None;
    let mut audio_activity: Option<PcmActivity> = None;
    let mut decode_contract: Option<(String, String, String, String)> = None;

    for mut chunk in chunks {
        if let Some(chunk_activity) = chunk.audio_activity.take() {
            audio_activity = Some(match audio_activity {
                None => chunk_activity,
                Some(mut accumulated) => {
                    if accumulated.frame_ms != chunk_activity.frame_ms
                        || accumulated.threshold_dbfs != chunk_activity.threshold_dbfs
                    {
                        return Err(ManagerError::Protocol);
                    }
                    accumulated.audio_duration_ms = accumulated
                        .audio_duration_ms
                        .max(chunk_activity.audio_duration_ms);
                    accumulated.first_active_ms =
                        match (accumulated.first_active_ms, chunk_activity.first_active_ms) {
                            (Some(left), Some(right)) => Some(left.min(right)),
                            (left, right) => left.or(right),
                        };
                    accumulated.last_active_ms =
                        match (accumulated.last_active_ms, chunk_activity.last_active_ms) {
                            (Some(left), Some(right)) => Some(left.max(right)),
                            (left, right) => left.or(right),
                        };
                    accumulated
                }
            });
        }
        let run = chunk
            .helper_runs
            .first()
            .cloned()
            .ok_or(ManagerError::Protocol)?;
        if chunk.request_id != request_id
            || chunk.context_sha256 != context_sha256
            || chunk.completed.request_id != request_id
            || chunk.completed.context_sha256 != context_sha256
            || chunk.completed.backend != "Vulkan0"
            || chunk.completed.device_description != "Intel(R) Arc(TM) Graphics"
            || chunk.completed.was_aborted
            || chunk.completed.was_truncated
            || chunk.residual_process_count != 0
            || run.residual_process_count != 0
            || run.terminal_count != 1
        {
            return Err(ManagerError::Protocol);
        }
        let chunk_decode_contract = (
            chunk.language_requested.clone(),
            chunk.language_resolved.clone(),
            chunk.decode_parameters_json.clone(),
            chunk.decode_parameters_sha256.clone(),
        );
        if chunk_decode_contract.0 != moss_helper::native::MOSS_LANGUAGE_REQUESTED
            || chunk_decode_contract.1 != moss_helper::native::MOSS_LANGUAGE_RESOLVED
            || chunk_decode_contract.2 != moss_helper::native::MOSS_DECODE_PARAMETERS_JSON
            || !chunk_decode_contract
                .3
                .eq_ignore_ascii_case(&moss_helper::native::moss_decode_parameters_sha256())
            || decode_contract
                .as_ref()
                .is_some_and(|expected| expected != &chunk_decode_contract)
        {
            return Err(ManagerError::Protocol);
        }
        decode_contract = Some(chunk_decode_contract);
        if !raw_text.is_empty() {
            raw_text.push('\n');
        }
        raw_text.push_str(&chunk.raw_text);
        if !clean_text.is_empty() {
            clean_text.push('\n');
        }
        clean_text.push_str(&chunk.clean_text);

        let timestamp_offset: i64 = run
            .input_offset_ms
            .try_into()
            .map_err(|_| ManagerError::Protocol)?;
        let speaker_offset = i32::try_from(run.chunk_index)
            .ok()
            .and_then(|value| value.checked_mul(SPEAKER_CHUNK_STRIDE))
            .ok_or(ManagerError::Protocol)?;
        for mut segment in chunk.segments.drain(..) {
            if segment.speaker_id >= SPEAKER_CHUNK_STRIDE {
                return Err(ManagerError::Protocol);
            }
            segment.segment_index = segments
                .len()
                .try_into()
                .map_err(|_| ManagerError::Protocol)?;
            segment.t0_ms = segment
                .t0_ms
                .checked_add(timestamp_offset)
                .ok_or(ManagerError::Protocol)?;
            segment.t1_ms = segment
                .t1_ms
                .checked_add(timestamp_offset)
                .ok_or(ManagerError::Protocol)?;
            segment.speaker_id = segment
                .speaker_id
                .checked_add(speaker_offset)
                .ok_or(ManagerError::Protocol)?;
            segments.push(segment);
        }

        heartbeat_count = heartbeat_count.saturating_add(chunk.heartbeat_count);
        native_run_elapsed_ms = native_run_elapsed_ms
            .checked_add(chunk.completed.native_run_elapsed_ms)
            .ok_or(ManagerError::Protocol)?;
        helper_wall_elapsed_ms = helper_wall_elapsed_ms
            .checked_add(chunk.completed.wall_elapsed_ms)
            .ok_or(ManagerError::Protocol)?;
        native_load_ms += chunk.completed.native_timings.load_ms;
        native_mel_ms += chunk.completed.native_timings.mel_ms;
        native_encode_ms += chunk.completed.native_timings.encode_ms;
        native_decode_ms += chunk.completed.native_timings.decode_ms;
        helper_total_processes =
            helper_total_processes.saturating_add(chunk.helper_total_processes);
        helper_peak_job_memory_bytes =
            helper_peak_job_memory_bytes.max(chunk.helper_peak_job_memory_bytes);
        residual_process_count =
            residual_process_count.saturating_add(chunk.residual_process_count);
        helper_runs.extend(chunk.helper_runs);
        final_template = Some(chunk.completed);
    }

    if raw_text.trim().is_empty()
        || clean_text.trim().is_empty()
        || segments.is_empty()
        || [
            native_load_ms,
            native_mel_ms,
            native_encode_ms,
            native_decode_ms,
        ]
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
    {
        return Err(ManagerError::Protocol);
    }
    let last_timestamp_ms = segments
        .last()
        .map(|segment| segment.t1_ms)
        .ok_or(ManagerError::Protocol)?;
    let segment_count = segments
        .len()
        .try_into()
        .map_err(|_| ManagerError::Protocol)?;
    let mut completed = final_template.ok_or(ManagerError::Protocol)?;
    completed.request_id = request_id.to_string();
    completed.context_sha256 = context_sha256.to_string();
    completed.native_run_elapsed_ms = native_run_elapsed_ms;
    completed.native_rtf = native_run_elapsed_ms as f64 / 1_000.0 / audio_duration_seconds;
    completed.wall_elapsed_ms = helper_wall_elapsed_ms;
    completed.wall_rtf = helper_wall_elapsed_ms as f64 / 1_000.0 / audio_duration_seconds;
    completed.last_timestamp_ms = last_timestamp_ms;
    completed.segment_count = segment_count;
    completed.raw_text_sha256 = sha256_text(&raw_text);
    completed.clean_text_sha256 = sha256_text(&clean_text);
    completed.native_timings.load_ms = native_load_ms;
    completed.native_timings.mel_ms = native_mel_ms;
    completed.native_timings.encode_ms = native_encode_ms;
    completed.native_timings.decode_ms = native_decode_ms;

    let (language_requested, language_resolved, decode_parameters_json, decode_parameters_sha256) =
        decode_contract.ok_or(ManagerError::Protocol)?;

    let helper_process_id = if multiple_chunks {
        0
    } else {
        helper_runs
            .last()
            .map(|run| run.process_id)
            .ok_or(ManagerError::Protocol)?
    };
    Ok(ManagedTranscription {
        request_id: request_id.to_string(),
        context_sha256: context_sha256.to_string(),
        raw_text,
        clean_text,
        segments,
        completed,
        heartbeat_count,
        supervisor_wall_elapsed_ms: 0,
        supervisor_wall_rtf: 0.0,
        helper_process_id,
        helper_total_processes,
        helper_peak_job_memory_bytes,
        residual_process_count,
        helper_runs,
        audio_activity,
        language_requested,
        language_resolved,
        decode_parameters_json,
        decode_parameters_sha256,
    })
}

fn effective_max_wall_rtf(duration_seconds: f64, internal_override: Option<f64>) -> Option<f64> {
    if duration_seconds >= PERFORMANCE_GATE_MINIMUM_SECONDS {
        Some(FROZEN_MAX_WALL_RTF)
    } else {
        internal_override
    }
}

fn validate_input(input: &TranscribeInput) -> Result<(), ManagerError> {
    if !moss_helper::protocol::is_uuid(&input.request_id)
        || !moss_helper::protocol::is_sha256(&input.context_sha256)
        || !moss_helper::protocol::is_sha256(&input.model_sha256)
        || !input.runtime_directory.is_absolute()
        || !input.model_path.is_absolute()
        || input.runtime_directory.to_str().is_none()
        || input.model_path.to_str().is_none()
        || input.timeout < Duration::from_secs(1)
        || input.timeout > Duration::from_secs(24 * 60 * 60)
        || input.samples.is_empty()
        || input.sample_rate_hz == 0
        || input.channels == 0
        || input.channels > 32
        || input.samples.len() % input.channels.max(1) as usize != 0
        || input
            .max_wall_rtf
            .is_some_and(|value| !value.is_finite() || value <= 0.0)
        || input.language_requested != moss_helper::native::MOSS_LANGUAGE_REQUESTED
        || input.decode_parameters_json != moss_helper::native::MOSS_DECODE_PARAMETERS_JSON
        || !input
            .decode_parameters_sha256
            .eq_ignore_ascii_case(&moss_helper::native::moss_decode_parameters_sha256())
    {
        return Err(ManagerError::InvalidRequest);
    }
    if input.sample_rate_hz != 16_000 || input.channels != 1 {
        return Err(ManagerError::InvalidRequest);
    }
    let maximum_samples = (input.sample_rate_hz as u64)
        .checked_mul(input.channels as u64)
        .and_then(|value| value.checked_mul(MAX_REQUEST_SECONDS))
        .ok_or(ManagerError::InvalidRequest)?;
    if input.samples.len() as u64 > maximum_samples {
        return Err(ManagerError::AudioTooLong);
    }
    Ok(())
}

fn secure_existing_file(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    for component in path.ancestors() {
        if component.as_os_str().is_empty() || path_is_reparse(component) {
            return None;
        }
    }
    let canonical = std::fs::canonicalize(path).ok()?;
    if !canonical.is_file() || canonical.to_str().is_none() {
        return None;
    }
    for component in canonical.ancestors() {
        if component.as_os_str().is_empty() || path_is_reparse(component) {
            return None;
        }
    }
    Some(canonical)
}

#[cfg(windows)]
fn path_is_reparse(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
        .unwrap_or(true)
}

#[cfg(not(windows))]
fn path_is_reparse(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(true)
}

fn exact_device(device_id: Option<String>) -> DeviceSpec {
    DeviceSpec {
        kind: "vulkan".to_string(),
        description: "Intel(R) Arc(TM) Graphics".to_string(),
        device_id,
        allow_primary_fallback: false,
    }
}

fn phase_advances(previous: Option<TaskPhase>, next: TaskPhase) -> bool {
    matches!(
        (previous, next),
        (None, TaskPhase::Preflight)
            | (Some(TaskPhase::Preflight), TaskPhase::NativeRunning)
            | (Some(TaskPhase::NativeRunning), TaskPhase::ResultStreaming)
    )
}

fn message_v(message: &ServerMessage) -> u16 {
    match message {
        ServerMessage::Hello(value) => value.v,
        ServerMessage::Accepted(value) => value.v,
        ServerMessage::Status(value) => value.v,
        ServerMessage::Heartbeat(value) => value.v,
        ServerMessage::ProbeResult(value) => value.v,
        ServerMessage::TextChunk(value) => value.v,
        ServerMessage::Segment(value) => value.v,
        ServerMessage::Completed(value) => value.v,
        ServerMessage::Cancelled(value) => value.v,
        ServerMessage::Shutdown(value) => value.v,
        ServerMessage::Failed(value) => value.v,
    }
}

fn message_seq(message: &ServerMessage) -> u64 {
    match message {
        ServerMessage::Hello(value) => value.seq,
        ServerMessage::Accepted(value) => value.seq,
        ServerMessage::Status(value) => value.seq,
        ServerMessage::Heartbeat(value) => value.seq,
        ServerMessage::ProbeResult(value) => value.seq,
        ServerMessage::TextChunk(value) => value.seq,
        ServerMessage::Segment(value) => value.seq,
        ServerMessage::Completed(value) => value.seq,
        ServerMessage::Cancelled(value) => value.seq,
        ServerMessage::Shutdown(value) => value.seq,
        ServerMessage::Failed(value) => value.seq,
    }
}

fn ensure_request_id(message: &ServerMessage, expected: &str) -> Result<(), ManagerError> {
    let actual = match message {
        ServerMessage::Accepted(value) => Some(value.request_id.as_str()),
        ServerMessage::Status(value) => Some(value.request_id.as_str()),
        ServerMessage::Heartbeat(value) => Some(value.request_id.as_str()),
        ServerMessage::ProbeResult(value) => Some(value.request_id.as_str()),
        ServerMessage::TextChunk(value) => Some(value.request_id.as_str()),
        ServerMessage::Segment(value) => Some(value.request_id.as_str()),
        ServerMessage::Completed(value) => Some(value.request_id.as_str()),
        ServerMessage::Cancelled(value) => Some(value.request_id.as_str()),
        ServerMessage::Shutdown(value) => Some(value.request_id.as_str()),
        ServerMessage::Failed(value) => value.request_id.as_deref(),
        ServerMessage::Hello(_) => None,
    };
    if actual == Some(expected) {
        Ok(())
    } else {
        #[cfg(test)]
        eprintln!("MOSS protocol rejection: request id");
        Err(ManagerError::Protocol)
    }
}

pub fn probe_with_helper(
    helper: &Path,
    runtime_directory: PathBuf,
    request_id: String,
    device_id: Option<String>,
    timeout: Duration,
) -> Result<ProbeResultMessage, ManagerError> {
    if !moss_helper::protocol::is_uuid(&request_id)
        || !runtime_directory.is_absolute()
        || runtime_directory.to_str().is_none()
    {
        return Err(ManagerError::InvalidRequest);
    }
    let process = spawn_suspended_assigned(helper, &[] as &[OsString])?;
    drain_stderr(process.stderr);
    let receiver = read_stdout(process.stdout);
    let control = Arc::new(TaskControl::new(
        request_id.clone(),
        process.control,
        process.stdin,
    ));
    let started = Instant::now();
    match receive_until(&receiver, started + timeout, &control)? {
        ServerMessage::Hello(hello) if hello.v == PROTOCOL_VERSION && hello.seq == 0 => {}
        _ => return Err(ManagerError::Protocol),
    }
    control.send(ClientMessage::Probe(ProbeCommand {
        v: PROTOCOL_VERSION,
        request_id: request_id.clone(),
        client_seq: 1,
        runtime: RuntimeSpec {
            directory: runtime_directory
                .to_str()
                .ok_or(ManagerError::InvalidRequest)?
                .to_owned(),
        },
        device: exact_device(device_id),
    }))?;
    match receive_until(&receiver, started + timeout, &control)? {
        ServerMessage::Accepted(value)
            if value.seq == 1
                && value.request_id == request_id
                && value.operation == Operation::Probe
                && value.context_sha256.is_none() => {}
        _ => return Err(ManagerError::Protocol),
    }
    match receive_until(&receiver, started + timeout, &control)? {
        ServerMessage::ProbeResult(value)
            if value.seq == 2 && value.request_id == request_id && value.terminal =>
        {
            control.mark_terminal();
            ensure_clean_terminal(&receiver, &control)?;
            Ok(value)
        }
        ServerMessage::Failed(value) if value.terminal => Err(ManagerError::Native(value.code)),
        _ => Err(ManagerError::Protocol),
    }
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::super::windows_job::SpawnedJob;
    use super::*;
    use moss_helper::protocol::{
        CancelledMessage, CompletionStatus, FailedMessage, HelloMessage, NativeTimings,
        SegmentMessage, StatusMessage, TextChunkMessage,
    };

    const REQUEST_ID: &str = "798d8c63-5ff1-40e3-9db8-0f706aeb930a";
    const OTHER_ID: &str = "298d8c63-5ff1-40e3-9db8-0f706aeb930b";
    const CONTEXT: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    fn protocol() -> TranscribeProtocol {
        TranscribeProtocol::new(
            REQUEST_ID.to_string(),
            CONTEXT.to_string(),
            16_000,
            moss_helper::native::MOSS_LANGUAGE_REQUESTED.to_owned(),
            moss_helper::native::MOSS_DECODE_PARAMETERS_JSON.to_owned(),
            moss_helper::native::moss_decode_parameters_sha256(),
        )
    }

    fn hello() -> ServerMessage {
        ServerMessage::Hello(HelloMessage {
            v: PROTOCOL_VERSION,
            seq: 0,
            helper_version: "test".to_string(),
            pid: 1,
            capabilities: vec!["transcribe".to_string()],
        })
    }

    fn accepted() -> ServerMessage {
        ServerMessage::Accepted(AcceptedMessage {
            v: PROTOCOL_VERSION,
            seq: 1,
            request_id: REQUEST_ID.to_string(),
            operation: Operation::Transcribe,
            context_sha256: Some(CONTEXT.to_string()),
        })
    }

    fn status(seq: u64, phase: TaskPhase) -> ServerMessage {
        ServerMessage::Status(StatusMessage {
            v: PROTOCOL_VERSION,
            seq,
            request_id: REQUEST_ID.to_string(),
            context_sha256: CONTEXT.to_string(),
            phase,
            elapsed_ms: seq,
        })
    }

    fn ready_protocol() -> TranscribeProtocol {
        let mut protocol = protocol();
        for message in [
            hello(),
            accepted(),
            status(2, TaskPhase::Preflight),
            status(3, TaskPhase::NativeRunning),
            status(4, TaskPhase::ResultStreaming),
            ServerMessage::TextChunk(TextChunkMessage {
                v: PROTOCOL_VERSION,
                seq: 5,
                request_id: REQUEST_ID.to_string(),
                stream: TextStream::Raw,
                part: 0,
                last: true,
                text: "raw".to_string(),
            }),
            ServerMessage::TextChunk(TextChunkMessage {
                v: PROTOCOL_VERSION,
                seq: 6,
                request_id: REQUEST_ID.to_string(),
                stream: TextStream::Clean,
                part: 0,
                last: true,
                text: "clean".to_string(),
            }),
            ServerMessage::Segment(SegmentMessage {
                v: PROTOCOL_VERSION,
                seq: 7,
                request_id: REQUEST_ID.to_string(),
                segment_index: 0,
                t0_ms: 0,
                t1_ms: 1_000,
                speaker_id: 1,
                text_part: 0,
                text_last: true,
                text: "clean".to_string(),
            }),
        ] {
            protocol.accept(message).unwrap();
        }
        protocol
    }

    fn completed(wall_elapsed_ms: u64, wall_rtf: f64) -> ServerMessage {
        ServerMessage::Completed(CompletedMessage {
            v: PROTOCOL_VERSION,
            seq: 8,
            request_id: REQUEST_ID.to_string(),
            context_sha256: CONTEXT.to_string(),
            terminal: true,
            status: CompletionStatus::Ok,
            backend: "Vulkan0".to_string(),
            device_description: "Intel(R) Arc(TM) Graphics".to_string(),
            native_run_elapsed_ms: 500,
            native_rtf: 0.5,
            wall_elapsed_ms,
            wall_rtf,
            last_timestamp_ms: 1_000,
            segment_count: 1,
            raw_text_sha256: sha256_text("raw"),
            clean_text_sha256: sha256_text("clean"),
            was_aborted: false,
            was_truncated: false,
            native_session_limits: NativeSessionLimits {
                effective_n_ctx: moss_helper::native::MOSS_SESSION_N_CTX,
                effective_max_audio_ms: 1_200_000,
                max_kv_bytes: 1_879_048_192,
            },
            native_timings: NativeTimings {
                load_ms: 1.0,
                mel_ms: 1.0,
                encode_ms: 1.0,
                decode_ms: 1.0,
            },
            language_requested: moss_helper::native::MOSS_LANGUAGE_REQUESTED.to_owned(),
            language_resolved: moss_helper::native::MOSS_LANGUAGE_RESOLVED.to_owned(),
            decode_parameters_json: moss_helper::native::MOSS_DECODE_PARAMETERS_JSON.to_owned(),
            decode_parameters_sha256: moss_helper::native::moss_decode_parameters_sha256(),
        })
    }

    #[test]
    fn protocol_rejects_unproved_native_decode_result_fields() {
        for mutation in 0..3 {
            let mut protocol = ready_protocol();
            let mut terminal = completed(600, 0.6);
            let ServerMessage::Completed(value) = &mut terminal else {
                unreachable!();
            };
            match mutation {
                0 => value.language_requested = "auto".to_owned(),
                1 => value.language_resolved = "en-US".to_owned(),
                2 => value.decode_parameters_sha256 = "0".repeat(64),
                _ => unreachable!(),
            }
            assert!(protocol.accept(terminal).is_err());
        }
    }

    #[test]
    fn protocol_rejects_out_of_order_phase_and_duplicate_terminal() {
        let mut out_of_order = protocol();
        assert!(out_of_order.accept(hello()).is_ok());
        assert!(out_of_order.accept(accepted()).is_ok());
        assert!(out_of_order
            .accept(status(2, TaskPhase::NativeRunning))
            .is_err());

        let mut duplicate_terminal = protocol();
        duplicate_terminal.accept(hello()).unwrap();
        duplicate_terminal.accept(accepted()).unwrap();
        assert!(matches!(
            duplicate_terminal
                .accept(ServerMessage::Cancelled(CancelledMessage {
                    v: PROTOCOL_VERSION,
                    seq: 2,
                    request_id: REQUEST_ID.to_string(),
                    terminal: true,
                    partial_result_available: false,
                }))
                .unwrap(),
            ProtocolOutcome::Cancelled
        ));
        assert!(duplicate_terminal
            .accept(ServerMessage::Failed(FailedMessage {
                v: PROTOCOL_VERSION,
                seq: 3,
                request_id: Some(REQUEST_ID.to_string()),
                terminal: true,
                code: ErrorCode::Internal,
                phase: "test".to_string(),
                retryable: false,
                native_status: None,
                message: "failed".to_string(),
            }))
            .is_err());
    }

    #[test]
    fn protocol_rejects_completed_text_hash_mismatch() {
        let mut protocol = protocol();
        for message in [
            hello(),
            accepted(),
            status(2, TaskPhase::Preflight),
            status(3, TaskPhase::NativeRunning),
            status(4, TaskPhase::ResultStreaming),
        ] {
            protocol.accept(message).unwrap();
        }
        protocol
            .accept(ServerMessage::TextChunk(TextChunkMessage {
                v: PROTOCOL_VERSION,
                seq: 5,
                request_id: REQUEST_ID.to_string(),
                stream: TextStream::Raw,
                part: 0,
                last: true,
                text: "raw".to_string(),
            }))
            .unwrap();
        protocol
            .accept(ServerMessage::TextChunk(TextChunkMessage {
                v: PROTOCOL_VERSION,
                seq: 6,
                request_id: REQUEST_ID.to_string(),
                stream: TextStream::Clean,
                part: 0,
                last: true,
                text: "clean".to_string(),
            }))
            .unwrap();
        protocol
            .accept(ServerMessage::Segment(SegmentMessage {
                v: PROTOCOL_VERSION,
                seq: 7,
                request_id: REQUEST_ID.to_string(),
                segment_index: 0,
                t0_ms: 0,
                t1_ms: 1_000,
                speaker_id: 1,
                text_part: 0,
                text_last: true,
                text: "clean".to_string(),
            }))
            .unwrap();
        assert!(protocol
            .accept(ServerMessage::Completed(CompletedMessage {
                v: PROTOCOL_VERSION,
                seq: 8,
                request_id: REQUEST_ID.to_string(),
                context_sha256: CONTEXT.to_string(),
                terminal: true,
                status: CompletionStatus::Ok,
                backend: "Vulkan0".to_string(),
                device_description: "Intel(R) Arc(TM) Graphics".to_string(),
                native_run_elapsed_ms: 500,
                native_rtf: 0.5,
                wall_elapsed_ms: 600,
                wall_rtf: 0.6,
                last_timestamp_ms: 1_000,
                segment_count: 1,
                raw_text_sha256: "0".repeat(64),
                clean_text_sha256: sha256_text("clean"),
                was_aborted: false,
                was_truncated: false,
                native_session_limits: NativeSessionLimits {
                    effective_n_ctx: moss_helper::native::MOSS_SESSION_N_CTX,
                    effective_max_audio_ms: 1_200_000,
                    max_kv_bytes: 1_879_048_192,
                },
                native_timings: NativeTimings {
                    load_ms: 1.0,
                    mel_ms: 1.0,
                    encode_ms: 1.0,
                    decode_ms: 1.0,
                },
                language_requested: moss_helper::native::MOSS_LANGUAGE_REQUESTED.to_owned(),
                language_resolved: moss_helper::native::MOSS_LANGUAGE_RESOLVED.to_owned(),
                decode_parameters_json: moss_helper::native::MOSS_DECODE_PARAMETERS_JSON.to_owned(),
                decode_parameters_sha256: moss_helper::native::moss_decode_parameters_sha256(),
            }))
            .is_err());
    }

    #[test]
    fn protocol_preserves_native_output_truncated_as_the_unique_terminal() {
        let mut protocol = protocol();
        protocol.accept(hello()).unwrap();
        protocol.accept(accepted()).unwrap();
        let outcome = protocol
            .accept(ServerMessage::Failed(FailedMessage {
                v: PROTOCOL_VERSION,
                seq: 2,
                request_id: Some(REQUEST_ID.to_string()),
                terminal: true,
                code: ErrorCode::NativeOutputTruncated,
                phase: "native_run".to_string(),
                retryable: false,
                native_status: Some(18),
                message: "native output was truncated".to_string(),
            }))
            .unwrap();
        assert!(matches!(
            outcome,
            ProtocolOutcome::Failed(ErrorCode::NativeOutputTruncated)
        ));
        assert!(protocol
            .accept(ServerMessage::Failed(FailedMessage {
                v: PROTOCOL_VERSION,
                seq: 3,
                request_id: Some(REQUEST_ID.to_string()),
                terminal: true,
                code: ErrorCode::NativeOutputTruncated,
                phase: "native_run".to_string(),
                retryable: false,
                native_status: Some(18),
                message: "native output was truncated".to_string(),
            }))
            .is_err());
    }

    #[test]
    fn protocol_accepts_overlapping_diarized_segments_with_monotonic_starts() {
        let mut protocol = ready_protocol();
        protocol
            .accept(ServerMessage::Segment(SegmentMessage {
                v: PROTOCOL_VERSION,
                seq: 8,
                request_id: REQUEST_ID.to_string(),
                segment_index: 1,
                t0_ms: 500,
                t1_ms: 900,
                speaker_id: 2,
                text_part: 0,
                text_last: true,
                text: "overlap".to_string(),
            }))
            .unwrap();
        let mut terminal = completed(600, 0.6);
        if let ServerMessage::Completed(value) = &mut terminal {
            value.seq = 9;
            value.segment_count = 2;
            value.last_timestamp_ms = 1_000;
        }
        assert!(matches!(
            protocol.accept(terminal).unwrap(),
            ProtocolOutcome::Completed(_)
        ));
    }

    #[test]
    fn native_rtf_below_one_cannot_hide_end_to_end_wall_rtf_above_one() {
        let mut protocol = ready_protocol();
        let ProtocolOutcome::Completed(value) = protocol.accept(completed(600, 0.6)).unwrap()
        else {
            panic!("expected a completed helper result");
        };
        assert_eq!(value.completed.native_rtf, 0.5);
        assert_eq!(value.completed.wall_rtf, 0.6);
        assert!(matches!(
            finalize_supervisor_metrics(*value, Duration::from_millis(1_100), 1.0, Some(1.0)),
            Err(ManagerError::PerformanceGate)
        ));

        let mut protocol = ready_protocol();
        let ProtocolOutcome::Completed(value) = protocol.accept(completed(600, 0.6)).unwrap()
        else {
            panic!("expected a completed helper result");
        };
        let result =
            finalize_supervisor_metrics(*value, Duration::from_millis(1_100), 1.0, None).unwrap();
        assert!((result.supervisor_wall_rtf - 1.1).abs() < f64::EPSILON);
    }

    #[test]
    fn inherited_command_start_controls_the_manager_timeout() {
        let manager = MossHelperManager::new(None, PathBuf::from("cache"));
        let input = TranscribeInput {
            request_id: REQUEST_ID.to_string(),
            context_sha256: CONTEXT.to_string(),
            runtime_directory: PathBuf::from(r"C:\frozen-runtime"),
            model_path: PathBuf::from(r"C:\frozen-model.gguf"),
            model_bytes: moss_helper::native::EXPECTED_MODEL_BYTES,
            model_sha256: moss_helper::native::EXPECTED_MODEL_SHA256.to_string(),
            device_id: None,
            timeout: Duration::from_secs(1),
            max_wall_rtf: None,
            samples: vec![0.0; 16_000],
            sample_rate_hz: 16_000,
            channels: 1,
            language_requested: moss_helper::native::MOSS_LANGUAGE_REQUESTED.to_owned(),
            decode_parameters_json: moss_helper::native::MOSS_DECODE_PARAMETERS_JSON.to_owned(),
            decode_parameters_sha256: moss_helper::native::moss_decode_parameters_sha256(),
        };
        assert!(matches!(
            manager.transcribe_from_started(input, Instant::now() - Duration::from_secs(2)),
            Err(ManagerError::Timeout)
        ));
    }

    #[test]
    fn frozen_long_block_performance_gate_cannot_be_disabled_or_relaxed() {
        assert_eq!(effective_max_wall_rtf(479.9, None), None);
        assert_eq!(effective_max_wall_rtf(480.0, None), Some(1.0));
        assert_eq!(effective_max_wall_rtf(600.0, Some(9.0)), Some(1.0));
    }

    #[test]
    fn preparation_cancel_and_timeout_finish_before_any_helper_can_start() {
        fn input() -> TranscribeInput {
            TranscribeInput {
                request_id: REQUEST_ID.to_string(),
                context_sha256: CONTEXT.to_string(),
                runtime_directory: PathBuf::from(r"C:\frozen-runtime"),
                model_path: PathBuf::from(r"C:\frozen-model.gguf"),
                model_bytes: moss_helper::native::EXPECTED_MODEL_BYTES,
                model_sha256: moss_helper::native::EXPECTED_MODEL_SHA256.to_string(),
                device_id: None,
                timeout: Duration::from_secs(1),
                max_wall_rtf: None,
                samples: vec![0.0; 16_000],
                sample_rate_hz: 16_000,
                channels: 1,
                language_requested: moss_helper::native::MOSS_LANGUAGE_REQUESTED.to_owned(),
                decode_parameters_json: moss_helper::native::MOSS_DECODE_PARAMETERS_JSON.to_owned(),
                decode_parameters_sha256: moss_helper::native::moss_decode_parameters_sha256(),
            }
        }

        let manager = Arc::new(MossHelperManager::new(None, PathBuf::from("cache")));
        let preparation = manager.begin_preparation(REQUEST_ID).unwrap();
        assert!(manager.cancel(REQUEST_ID).unwrap());
        assert!(matches!(
            manager.transcribe_prepared(input(), Instant::now(), &preparation),
            Err(ManagerError::Cancelled)
        ));
        assert!(manager.status().active_process_ids.is_empty());
        drop(preparation);

        let preparation = manager.begin_preparation(REQUEST_ID).unwrap();
        assert!(matches!(
            manager.transcribe_prepared(
                input(),
                Instant::now() - Duration::from_secs(2),
                &preparation
            ),
            Err(ManagerError::Timeout)
        ));
        assert!(manager.status().active_process_ids.is_empty());
        drop(preparation);
        assert!(manager.status().active_request_ids.is_empty());
    }

    #[test]
    fn global_gate_rejects_same_and_different_request_ids() {
        let manager = MossHelperManager::new(None, PathBuf::from("cache"));
        manager.reserve(REQUEST_ID).unwrap();
        assert!(matches!(
            manager.reserve(REQUEST_ID),
            Err(ManagerError::DuplicateRequest)
        ));
        assert!(matches!(manager.reserve(OTHER_ID), Err(ManagerError::Busy)));
        assert_eq!(
            manager.status().active_request_ids,
            vec![REQUEST_ID.to_string()]
        );
        manager.release_reservation(REQUEST_ID);
        manager.reserve(OTHER_ID).unwrap();
        manager.release_reservation(OTHER_ID);
    }

    #[test]
    fn normalized_native_ranges_cover_every_sample_once_at_exact_boundaries() {
        let maximum = 4_800_000usize;
        for total in [maximum, maximum + 1, maximum * 2, maximum * 2 + 123] {
            let ranges = native_chunk_ranges(total, maximum).unwrap();
            assert_eq!(ranges.first().map(|range| range.0), Some(0));
            assert_eq!(ranges.last().map(|range| range.1), Some(total));
            assert!(ranges
                .iter()
                .all(|(start, end)| { start < end && end - start <= maximum }));
            assert!(ranges.windows(2).all(|pair| pair[0].1 == pair[1].0));
            assert_eq!(
                ranges.iter().map(|(start, end)| end - start).sum::<usize>(),
                total
            );
        }
    }

    #[test]
    fn chunk_merge_preserves_global_first_and_last_active_audio_times() {
        fn chunk(
            chunk_index: u32,
            input_offset_ms: u64,
            input_duration_ms: u64,
            activity: PcmActivity,
        ) -> ManagedTranscription {
            let mut protocol = ready_protocol();
            let ProtocolOutcome::Completed(value) = protocol.accept(completed(600, 0.6)).unwrap()
            else {
                panic!("expected completed chunk");
            };
            let mut value = *value;
            value.helper_process_id = chunk_index + 1;
            value.helper_total_processes = 1;
            value.helper_peak_job_memory_bytes = 1_024;
            value.residual_process_count = 0;
            value.audio_activity = Some(activity);
            value.helper_runs.push(ManagedHelperRun {
                chunk_index,
                input_offset_ms,
                input_duration_ms,
                process_id: value.helper_process_id,
                total_processes: 1,
                peak_job_memory_bytes: 1_024,
                residual_process_count: 0,
                terminal_count: 1,
                last_timestamp_ms: value.completed.last_timestamp_ms,
                native_rtf: value.completed.native_rtf,
                raw_text_sha256: value.completed.raw_text_sha256.clone(),
                clean_text_sha256: value.completed.clean_text_sha256.clone(),
            });
            value
        }

        let result = merge_chunk_results(
            REQUEST_ID,
            CONTEXT,
            vec![
                chunk(
                    0,
                    0,
                    480_000,
                    PcmActivity {
                        frame_ms: 20,
                        threshold_dbfs: -50.0,
                        audio_duration_ms: 480_000,
                        first_active_ms: Some(20),
                        last_active_ms: Some(479_000),
                    },
                ),
                chunk(
                    1,
                    480_000,
                    120_000,
                    PcmActivity {
                        frame_ms: 20,
                        threshold_dbfs: -50.0,
                        audio_duration_ms: 600_000,
                        first_active_ms: Some(500_000),
                        last_active_ms: Some(599_980),
                    },
                ),
            ],
            600.0,
        )
        .unwrap();
        assert_eq!(
            result.audio_activity,
            Some(PcmActivity {
                frame_ms: 20,
                threshold_dbfs: -50.0,
                audio_duration_ms: 600_000,
                first_active_ms: Some(20),
                last_active_ms: Some(599_980),
            })
        );
        assert_eq!(result.helper_runs.len(), 2);
        assert_eq!(result.segments[1].t0_ms, 480_000);
    }

    fn successful_result() -> ManagedTranscription {
        let mut protocol = ready_protocol();
        let ProtocolOutcome::Completed(value) = protocol.accept(completed(600, 0.6)).unwrap()
        else {
            panic!("expected a completed helper result");
        };
        *value
    }

    #[test]
    fn reservation_commit_and_cancel_are_linearized_under_one_lock() {
        let manager = MossHelperManager::new(None, PathBuf::from("cache"));
        manager.reserve(REQUEST_ID).unwrap();
        assert!(manager.cancel(REQUEST_ID).unwrap());
        assert!(matches!(
            manager.finish_reserved(REQUEST_ID, Ok(successful_result())),
            Err(ManagerError::Cancelled)
        ));
        assert!(!manager.cancel(REQUEST_ID).unwrap());

        manager.reserve(REQUEST_ID).unwrap();
        assert!(manager
            .finish_reserved(REQUEST_ID, Ok(successful_result()))
            .is_ok());
        assert!(!manager.cancel(REQUEST_ID).unwrap());
    }

    #[test]
    fn concurrent_final_success_and_cancel_have_exactly_one_winner() {
        for _ in 0..100 {
            let manager = Arc::new(MossHelperManager::new(None, PathBuf::from("cache")));
            manager.reserve(REQUEST_ID).unwrap();
            let barrier = Arc::new(std::sync::Barrier::new(3));

            let cancel_manager = manager.clone();
            let cancel_barrier = barrier.clone();
            let cancel_thread = std::thread::spawn(move || {
                cancel_barrier.wait();
                cancel_manager.cancel(REQUEST_ID).unwrap()
            });

            let finish_manager = manager.clone();
            let finish_barrier = barrier.clone();
            let finish_thread = std::thread::spawn(move || {
                finish_barrier.wait();
                finish_manager.finish_reserved(REQUEST_ID, Ok(successful_result()))
            });

            barrier.wait();
            let cancel_won = cancel_thread.join().unwrap();
            let finish = finish_thread.join().unwrap();
            match (cancel_won, finish) {
                (true, Err(ManagerError::Cancelled)) | (false, Ok(_)) => {}
                _ => panic!("cancel and success did not produce one linearized winner"),
            }
            assert!(manager.status().active_request_ids.is_empty());
        }
    }

    #[cfg(windows)]
    fn system_executable(name: &str) -> PathBuf {
        PathBuf::from(std::env::var_os("WINDIR").unwrap())
            .join("System32")
            .join(name)
    }

    #[cfg(windows)]
    fn spawn_command(command: &str) -> SpawnedJob {
        super::spawn_suspended_assigned(
            &system_executable("cmd.exe"),
            &[
                OsString::from("/d"),
                OsString::from("/c"),
                OsString::from(command),
            ],
        )
        .unwrap()
    }

    #[cfg(windows)]
    fn spawn_sleep() -> (SpawnedJob, tempfile::TempDir) {
        let temporary = tempfile::tempdir().unwrap();
        let script = temporary.path().join("等待 循环.cmd");
        std::fs::write(
            &script,
            concat!(
                "@echo off\r\n",
                ":again\r\n",
                "\"%SystemRoot%\\System32\\ping.exe\" -n 2 127.0.0.1 >nul\r\n",
                "goto again\r\n"
            ),
        )
        .unwrap();
        let process = super::spawn_suspended_assigned(
            &system_executable("cmd.exe"),
            &[
                OsString::from("/d"),
                OsString::from("/c"),
                script.into_os_string(),
            ],
        )
        .unwrap();
        (process, temporary)
    }

    #[cfg(windows)]
    #[test]
    fn invalid_json_and_eof_kill_the_job_without_residual_processes() {
        for command in ["echo not-json & ping -n 30 127.0.0.1 >nul", "exit /b 0"] {
            let process = spawn_command(command);
            let receiver = read_stdout(process.stdout);
            drain_stderr(process.stderr);
            let control = Arc::new(TaskControl::new(
                REQUEST_ID.to_string(),
                process.control,
                process.stdin,
            ));
            assert!(matches!(
                receive_until(&receiver, Instant::now() + Duration::from_secs(3), &control),
                Err(ManagerError::Protocol)
            ));
            assert_eq!(control.job.active_processes().unwrap(), 0);
        }

        let powershell = PathBuf::from(std::env::var_os("WINDIR").unwrap())
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        let process = super::spawn_suspended_assigned(
            &powershell,
            &[
                OsString::from("-NoProfile"),
                OsString::from("-NonInteractive"),
                OsString::from("-Command"),
                OsString::from(format!(
                    "[Console]::Out.WriteLine('x' * {}); Start-Sleep -Seconds 30",
                    moss_helper::protocol::MAX_JSONL_BYTES + 1
                )),
            ],
        )
        .unwrap();
        let receiver = read_stdout(process.stdout);
        drain_stderr(process.stderr);
        let control = Arc::new(TaskControl::new(
            REQUEST_ID.to_string(),
            process.control,
            process.stdin,
        ));
        assert!(matches!(
            receive_until(&receiver, Instant::now() + Duration::from_secs(5), &control),
            Err(ManagerError::Protocol)
        ));
        assert_eq!(control.job.active_processes().unwrap(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn timeout_and_cancel_force_zero_residual_processes_after_grace() {
        let (process, _temporary) = spawn_sleep();
        let receiver = read_stdout(process.stdout);
        drain_stderr(process.stderr);
        let control = Arc::new(TaskControl::new(
            REQUEST_ID.to_string(),
            process.control,
            process.stdin,
        ));
        assert!(matches!(
            receive_until(
                &receiver,
                Instant::now() + Duration::from_millis(100),
                &control
            ),
            Err(ManagerError::Timeout)
        ));
        assert_eq!(control.job.active_processes().unwrap(), 0);

        let (process, _temporary) = spawn_sleep();
        let job = process.control.clone();
        let task = Arc::new(TaskControl::new(
            OTHER_ID.to_string(),
            process.control,
            process.stdin,
        ));
        drain_stderr(process.stderr);
        drop(process.stdout);
        let manager = MossHelperManager::new(None, PathBuf::from("cache"));
        manager.reserve(OTHER_ID).unwrap();
        manager
            .active
            .lock()
            .unwrap()
            .insert(OTHER_ID.to_string(), task);
        assert!(manager.cancel(OTHER_ID).unwrap());
        assert!(job.wait(Duration::from_secs(5)).unwrap());
        assert_eq!(job.active_processes().unwrap(), 0);
        manager.unregister(OTHER_ID);
        manager.release_reservation(OTHER_ID);
    }

    #[cfg(windows)]
    #[test]
    fn cancel_before_the_initial_command_is_queued_without_corrupting_sequence() {
        let (process, _temporary) = spawn_sleep();
        let task = Arc::new(TaskControl::new(
            REQUEST_ID.to_string(),
            process.control.clone(),
            process.stdin,
        ));
        drain_stderr(process.stderr);
        drop(process.stdout);
        task.request_cancel(CancelReason::User).unwrap();
        {
            let state = task.writer.lock().unwrap();
            assert_eq!(state.next_client_seq, 1);
            assert_eq!(state.pending_cancel, Some(CancelReason::User));
        }
        task.send(ClientMessage::Transcribe(TranscribeCommand {
            v: PROTOCOL_VERSION,
            request_id: REQUEST_ID.to_string(),
            client_seq: 1,
            context_sha256: CONTEXT.to_string(),
            runtime: RuntimeSpec {
                directory: r"C:\frozen-runtime".to_string(),
            },
            device: exact_device(None),
            model: ModelSpec {
                path: r"C:\frozen-model.gguf".to_string(),
                bytes: moss_helper::native::EXPECTED_MODEL_BYTES,
                sha256: moss_helper::native::EXPECTED_MODEL_SHA256.to_string(),
            },
            audio: moss_helper::protocol::AudioSpec {
                path: r"C:\frozen-audio.f32le".to_string(),
                format: "f32le".to_string(),
                sample_rate_hz: 16_000,
                channels: 1,
                samples: 16_000,
                bytes: 64_000,
                sha256: CONTEXT.to_string(),
            },
            language_requested: moss_helper::native::MOSS_LANGUAGE_REQUESTED.to_owned(),
            decode_parameters_json: moss_helper::native::MOSS_DECODE_PARAMETERS_JSON.to_owned(),
            decode_parameters_sha256: moss_helper::native::moss_decode_parameters_sha256(),
        }))
        .unwrap();
        {
            let state = task.writer.lock().unwrap();
            assert_eq!(state.next_client_seq, 3);
            assert_eq!(state.pending_cancel, None);
        }
        assert!(task.cancellation_requested.load(Ordering::SeqCst));
        process
            .control
            .terminate_and_confirm(Duration::from_secs(2))
            .unwrap();
        assert_eq!(process.control.active_processes().unwrap(), 0);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires real MOSS runtime/model/PCM fixtures and Intel Arc Vulkan"]
    fn real_manager_job_transcription_has_clean_terminal_and_zero_residual_processes() {
        let runtime_directory = PathBuf::from(
            std::env::var_os("MOSS_TEST_RUNTIME_DIR")
                .expect("MOSS_TEST_RUNTIME_DIR is required for the ignored real gate"),
        );
        let model_path = PathBuf::from(
            std::env::var_os("MOSS_TEST_MODEL_PATH")
                .expect("MOSS_TEST_MODEL_PATH is required for the ignored real gate"),
        );
        let pcm_path = PathBuf::from(
            std::env::var_os("MOSS_TEST_PCM_PATH")
                .expect("MOSS_TEST_PCM_PATH is required for the ignored real gate"),
        );
        let helper_path = PathBuf::from(
            std::env::var_os("MOSS_TEST_HELPER_PATH")
                .expect("MOSS_TEST_HELPER_PATH is required for the ignored real gate"),
        );
        let pcm_bytes = std::fs::read(pcm_path).unwrap();
        let samples = pcm_bytes
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
            .collect();
        let cache = tempfile::tempdir().unwrap();
        let manager =
            MossHelperManager::new(Some(helper_path), cache.path().join("中文 空格 MOSS缓存"));
        let result = manager
            .transcribe(TranscribeInput {
                request_id: REQUEST_ID.to_string(),
                context_sha256: CONTEXT.to_string(),
                runtime_directory,
                model_path,
                model_bytes: moss_helper::native::EXPECTED_MODEL_BYTES,
                model_sha256: moss_helper::native::EXPECTED_MODEL_SHA256.to_string(),
                device_id: None,
                timeout: Duration::from_secs(180),
                max_wall_rtf: None,
                samples,
                sample_rate_hz: 16_000,
                channels: 1,
                language_requested: moss_helper::native::MOSS_LANGUAGE_REQUESTED.to_owned(),
                decode_parameters_json: moss_helper::native::MOSS_DECODE_PARAMETERS_JSON.to_owned(),
                decode_parameters_sha256: moss_helper::native::moss_decode_parameters_sha256(),
            })
            .unwrap();
        assert_eq!(result.completed.backend, "Vulkan0");
        assert_eq!(
            result.completed.device_description,
            "Intel(R) Arc(TM) Graphics"
        );
        assert!(!result.completed.was_aborted);
        assert!(!result.completed.was_truncated);
        assert!(manager.status().active_request_ids.is_empty());
        assert!(!cache
            .path()
            .join("中文 空格 MOSS缓存")
            .join(REQUEST_ID)
            .exists());
    }

    #[cfg(windows)]
    fn p2c_path(name: &str) -> PathBuf {
        PathBuf::from(
            std::env::var_os(name)
                .unwrap_or_else(|| panic!("{name} is required for the ignored P2-C gate")),
        )
    }

    #[cfg(windows)]
    fn p2c_samples(name: &str) -> Vec<f32> {
        let bytes = std::fs::read(p2c_path(name)).unwrap();
        assert_eq!(bytes.len() % 4, 0);
        bytes
            .chunks_exact(4)
            .map(|value| f32::from_le_bytes(value.try_into().unwrap()))
            .collect()
    }

    #[cfg(windows)]
    fn p2c_pcm16_wav_samples(name: &str) -> Vec<f32> {
        let bytes = std::fs::read(p2c_path(name)).unwrap();
        assert!(bytes.len() >= 44, "WAV file is too short");
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");

        let mut cursor = 12usize;
        let mut format = None;
        let mut data = None;
        while cursor + 8 <= bytes.len() {
            let chunk_id = &bytes[cursor..cursor + 4];
            let chunk_len =
                u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
            let chunk_start = cursor + 8;
            let chunk_end = chunk_start.checked_add(chunk_len).unwrap();
            assert!(chunk_end <= bytes.len(), "WAV chunk exceeds file length");
            if chunk_id == b"fmt " {
                assert!(chunk_len >= 16, "WAV fmt chunk is incomplete");
                format = Some((
                    u16::from_le_bytes(bytes[chunk_start..chunk_start + 2].try_into().unwrap()),
                    u16::from_le_bytes(bytes[chunk_start + 2..chunk_start + 4].try_into().unwrap()),
                    u32::from_le_bytes(bytes[chunk_start + 4..chunk_start + 8].try_into().unwrap()),
                    u16::from_le_bytes(
                        bytes[chunk_start + 14..chunk_start + 16]
                            .try_into()
                            .unwrap(),
                    ),
                ));
            } else if chunk_id == b"data" {
                data = Some(&bytes[chunk_start..chunk_end]);
            }
            cursor = chunk_end + (chunk_len & 1);
        }

        assert_eq!(format, Some((1, 1, 16_000, 16)));
        let data = data.expect("WAV data chunk is missing");
        assert_eq!(data.len() % 2, 0);
        data.chunks_exact(2)
            .map(|value| i16::from_le_bytes(value.try_into().unwrap()) as f32 / 32_768.0)
            .collect()
    }

    #[cfg(windows)]
    fn p2c_input(request_id: String, context_sha256: String, samples: Vec<f32>) -> TranscribeInput {
        TranscribeInput {
            request_id,
            context_sha256,
            runtime_directory: p2c_path("MOSS_TEST_RUNTIME_DIR"),
            model_path: p2c_path("MOSS_TEST_MODEL_PATH"),
            model_bytes: moss_helper::native::EXPECTED_MODEL_BYTES,
            model_sha256: moss_helper::native::EXPECTED_MODEL_SHA256.to_string(),
            device_id: None,
            timeout: Duration::from_secs(180),
            max_wall_rtf: None,
            samples,
            sample_rate_hz: 16_000,
            channels: 1,
            language_requested: moss_helper::native::MOSS_LANGUAGE_REQUESTED.to_owned(),
            decode_parameters_json: moss_helper::native::MOSS_DECODE_PARAMETERS_JSON.to_owned(),
            decode_parameters_sha256: moss_helper::native::moss_decode_parameters_sha256(),
        }
    }

    #[cfg(windows)]
    fn p2c_request_id(index: usize) -> String {
        format!("798d8c63-5ff1-40e3-9db8-{index:012x}")
    }

    #[cfg(windows)]
    fn p2c_available_memory() -> u64 {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..unsafe { std::mem::zeroed() }
        };
        assert_ne!(unsafe { GlobalMemoryStatusEx(&mut status) }, 0);
        status.ullAvailPhys
    }

    #[cfg(windows)]
    fn p2c_write_json(path_variable: &str, value: &serde_json::Value) {
        use std::io::Write;

        let mut value = value.clone();
        let binding: serde_json::Value = serde_json::from_str(
            &std::env::var("MOSS_P2D_BINDING_FIELDS")
                .expect("MOSS_P2D_BINDING_FIELDS is required for real acceptance evidence"),
        )
        .expect("MOSS_P2D_BINDING_FIELDS must be valid JSON");
        let object = value
            .as_object_mut()
            .expect("evidence must be a JSON object");
        let binding = binding
            .as_object()
            .expect("MOSS_P2D_BINDING_FIELDS must be an object");
        for key in [
            "binding_sha256",
            "source_commit",
            "source_tree_clean",
            "cargo_lock_sha256",
            "test_executable_sha256",
            "helper_binary_sha256",
            "runtime_manifest_sha256",
            "model_sha256",
            "fixture_sha256",
        ] {
            object.insert(
                key.to_string(),
                binding
                    .get(key)
                    .unwrap_or_else(|| panic!("missing {key}"))
                    .clone(),
            );
        }
        let path = p2c_path(path_variable);
        let parent = path.parent().unwrap();
        std::fs::create_dir_all(parent).unwrap();
        let temporary = parent.join(format!(
            ".{}.{}.tmp",
            path.file_name().unwrap().to_str().unwrap(),
            std::process::id()
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .unwrap();
        let serialized = serde_json::to_vec_pretty(&value).unwrap();
        file.write_all(&serialized).unwrap();
        file.write_all(b"\n").unwrap();
        file.sync_all().unwrap();
        drop(file);
        std::fs::rename(&temporary, &path).unwrap();
        let digest = format!("{:X}", Sha256::digest(std::fs::read(&path).unwrap()));
        let sidecar = path.with_extension(format!(
            "{}.sha256",
            path.extension()
                .and_then(|value| value.to_str())
                .unwrap_or("json")
        ));
        let sidecar_temporary = parent.join(format!(
            ".{}.{}.sha256.tmp",
            path.file_name().unwrap().to_str().unwrap(),
            std::process::id()
        ));
        let mut sidecar_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&sidecar_temporary)
            .unwrap();
        sidecar_file
            .write_all(
                format!(
                    "{digest}  {}\n",
                    path.file_name().unwrap().to_str().unwrap()
                )
                .as_bytes(),
            )
            .unwrap();
        sidecar_file.sync_all().unwrap();
        drop(sidecar_file);
        std::fs::rename(sidecar_temporary, sidecar).unwrap();
    }

    #[cfg(windows)]
    fn p2c_wait_for_active(manager: &MossHelperManager) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let status = manager.status();
            if let Some(process_id) = status.active_process_ids.first() {
                return *process_id;
            }
            assert!(Instant::now() < deadline, "helper did not become active");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(windows)]
    fn p2c_process_is_alive(process_id: u32) -> bool {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject};
        const SYNCHRONIZE: u32 = 0x0010_0000;
        let process = unsafe { OpenProcess(SYNCHRONIZE, 0, process_id) };
        if process.is_null() {
            return false;
        }
        let alive = unsafe { WaitForSingleObject(process, 0) } == WAIT_TIMEOUT;
        unsafe { CloseHandle(process) };
        alive
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires release helper, real Q8/PCM fixtures, Intel Arc, and evidence output"]
    fn p2c_ten_sequential_real_manager_tasks_are_isolated_and_clean() {
        let samples = p2c_samples("MOSS_TEST_PCM_PATH");
        let cache = tempfile::tempdir().unwrap();
        let cache_root = cache.path().join("中文 空格 P2C MOSS缓存");
        let manager =
            MossHelperManager::new(Some(p2c_path("MOSS_TEST_HELPER_PATH")), cache_root.clone());
        let available_before = p2c_available_memory();
        let mut expected_raw = None;
        let mut expected_clean = None;
        let mut process_ids = std::collections::BTreeSet::new();
        let mut runs = Vec::new();
        for index in 1..=10 {
            let request_id = p2c_request_id(index);
            let context_sha256 = sha256_text(&format!("moss-p2c-context-{index}"));
            let started = Instant::now();
            let result = manager
                .transcribe(p2c_input(
                    request_id.clone(),
                    context_sha256.clone(),
                    samples.clone(),
                ))
                .unwrap();
            let observed_raw = sha256_text(&result.raw_text);
            let observed_clean = sha256_text(&result.clean_text);
            assert_eq!(observed_raw, result.completed.raw_text_sha256);
            assert_eq!(observed_clean, result.completed.clean_text_sha256);
            assert_eq!(result.context_sha256, context_sha256);
            assert_eq!(result.residual_process_count, 0);
            // The frozen Vulkan stack may create a short-lived worker process.  The
            // hard gate is that the whole Job is empty at the terminal, not that the
            // lifetime accounting contains only the helper itself.
            assert!(result.helper_total_processes >= 1);
            assert!(process_ids.insert(result.helper_process_id));
            assert!(manager.status().active_request_ids.is_empty());
            assert!(!cache_root.join(&request_id).exists());
            if let Some(value) = &expected_raw {
                assert_eq!(value, &observed_raw);
            } else {
                expected_raw = Some(observed_raw.clone());
            }
            if let Some(value) = &expected_clean {
                assert_eq!(value, &observed_clean);
            } else {
                expected_clean = Some(observed_clean.clone());
            }
            runs.push(serde_json::json!({
                "run": index,
                "request_id_sha256": sha256_text(&request_id),
                "context_sha256": context_sha256,
                "raw_text_sha256": observed_raw,
                "clean_text_sha256": observed_clean,
                "segment_count": result.completed.segment_count,
                "helper_process_id": result.helper_process_id,
                "wall_elapsed_ms": result.completed.wall_elapsed_ms,
                "wall_rtf": result.completed.wall_rtf,
                "supervisor_wall_elapsed_ms": result.supervisor_wall_elapsed_ms,
                "supervisor_wall_rtf": result.supervisor_wall_rtf,
                "native_rtf": result.completed.native_rtf,
                "helper_process_id": result.helper_process_id,
                "helper_total_processes": result.helper_total_processes,
                "helper_peak_job_memory_bytes": result.helper_peak_job_memory_bytes,
                "residual_process_count": result.residual_process_count,
                "manager_active_after": 0,
                "cache_entry_after": false,
                "test_wall_elapsed_ms": started.elapsed().as_millis() as u64,
                "terminal_count": 1
            }));
        }
        let available_after = p2c_available_memory();
        let peak_values: Vec<u64> = runs
            .iter()
            .map(|run| run["helper_peak_job_memory_bytes"].as_u64().unwrap())
            .collect();
        let payload = serde_json::json!({
            "schema_version": 1,
            "stage": "MOSS_V3_P2C_TEN_SEQUENTIAL_MANAGER",
            "status": "PASS",
            "git_commit": std::env::var("MOSS_P2C_GIT_COMMIT").unwrap_or_default(),
            "run_count": runs.len(),
            "distinct_helper_process_count": process_ids.len(),
            "unique_terminal_per_run": true,
            "cross_run_hash_isolation": true,
            "available_memory_before_bytes": available_before,
            "available_memory_after_bytes": available_after,
            "available_memory_delta_bytes": available_after as i64 - available_before as i64,
            "minimum_peak_job_memory_bytes": peak_values.iter().min().unwrap(),
            "maximum_peak_job_memory_bytes": peak_values.iter().max().unwrap(),
            "transcript_in_evidence": false,
            "absolute_paths_in_evidence": false,
            "runs": runs
        });
        p2c_write_json("MOSS_P2C_TEN_EVIDENCE", &payload);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires release helper, real Q8/PCM fixtures, Intel Arc, and evidence output"]
    fn p2c_real_cancel_and_timeout_leave_zero_processes() {
        let samples = p2c_samples("MOSS_TEST_PCM_PATH");
        let cache = tempfile::tempdir().unwrap();
        let manager = Arc::new(MossHelperManager::new(
            Some(p2c_path("MOSS_TEST_HELPER_PATH")),
            cache.path().join("P2C-control-cache"),
        ));

        let cancel_manager = manager.clone();
        let cancel_samples = samples.clone();
        let cancel_thread = std::thread::spawn(move || {
            cancel_manager.transcribe(p2c_input(
                p2c_request_id(101),
                sha256_text("moss-p2c-cancel"),
                cancel_samples,
            ))
        });
        let cancel_pid = p2c_wait_for_active(&manager);
        std::thread::sleep(Duration::from_millis(750));
        assert!(manager.cancel(&p2c_request_id(101)).unwrap());
        assert!(matches!(
            cancel_thread.join().unwrap(),
            Err(ManagerError::Cancelled)
        ));
        assert!(manager.status().active_request_ids.is_empty());
        assert!(!p2c_process_is_alive(cancel_pid));

        let timeout_manager = manager.clone();
        let timeout_thread = std::thread::spawn(move || {
            let mut input = p2c_input(
                p2c_request_id(102),
                sha256_text("moss-p2c-timeout"),
                samples,
            );
            input.timeout = Duration::from_secs(1);
            timeout_manager.transcribe(input)
        });
        let timeout_pid = p2c_wait_for_active(&manager);
        assert!(matches!(
            timeout_thread.join().unwrap(),
            Err(ManagerError::Timeout)
        ));
        assert!(manager.status().active_request_ids.is_empty());
        assert!(!p2c_process_is_alive(timeout_pid));

        p2c_write_json(
            "MOSS_P2C_CONTROL_EVIDENCE",
            &serde_json::json!({
                "schema_version": 1,
                "stage": "MOSS_V3_P2C_REAL_CONTROL",
                "status": "PASS",
                "cancel": {"terminal": "cancelled", "residual_process_count": 0},
                "timeout": {"terminal": "timeout", "residual_process_count": 0},
                "transcript_in_evidence": false,
                "absolute_paths_in_evidence": false
            }),
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires release helper, a real 600 second fixture, Q8 model, and Intel Arc"]
    fn p2c_chunked_cancel_and_timeout_never_start_a_later_child() {
        let samples = p2c_samples("MOSS_P2C_LONG_PCM_PATH");
        assert_eq!(samples.len(), 600 * 16_000);
        let cache = tempfile::tempdir().unwrap();
        let manager = Arc::new(MossHelperManager::new(
            Some(p2c_path("MOSS_TEST_HELPER_PATH")),
            cache.path().join("P2C-chunk-control-cache"),
        ));

        let cancel_manager = manager.clone();
        let cancel_samples = samples.clone();
        let cancel_thread = std::thread::spawn(move || {
            let mut input = p2c_input(
                p2c_request_id(701),
                sha256_text("moss-p2c-chunk-cancel"),
                cancel_samples,
            );
            input.timeout = Duration::from_secs(900);
            cancel_manager.transcribe(input)
        });
        let cancel_pid = p2c_wait_for_active(&manager);
        assert!(manager.cancel(&p2c_request_id(701)).unwrap());
        assert!(matches!(
            cancel_thread.join().unwrap(),
            Err(ManagerError::Cancelled)
        ));
        assert!(!p2c_process_is_alive(cancel_pid));
        assert_eq!(manager.status().helper_processes_started, 1);

        let timeout_manager = manager.clone();
        let timeout_thread = std::thread::spawn(move || {
            let mut input = p2c_input(
                p2c_request_id(702),
                sha256_text("moss-p2c-chunk-timeout"),
                samples,
            );
            input.timeout = Duration::from_secs(2);
            timeout_manager.transcribe(input)
        });
        let timeout_pid = p2c_wait_for_active(&manager);
        assert!(matches!(
            timeout_thread.join().unwrap(),
            Err(ManagerError::Timeout)
        ));
        assert!(!p2c_process_is_alive(timeout_pid));
        let status = manager.status();
        assert_eq!(status.helper_processes_started, 2);
        assert!(status.active_request_ids.is_empty());
        assert!(status.active_process_ids.is_empty());

        p2c_write_json(
            "MOSS_P2C_CHUNK_CONTROL_EVIDENCE",
            &serde_json::json!({
                "schema_version": 1,
                "stage": "MOSS_V3_P2C_CHUNK_CONTROL",
                "status": "PASS",
                "cancel": {"terminal": "cancelled", "helper_processes_started": 1, "later_child_started": false, "residual_process_count": 0},
                "timeout": {"terminal": "timeout", "helper_processes_started": 1, "later_child_started": false, "residual_process_count": 0},
                "transcript_in_evidence": false,
                "absolute_paths_in_evidence": false
            }),
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "victim process for the external P2-C parent-force termination gate"]
    fn p2c_parent_force_victim() {
        let manager = Arc::new(MossHelperManager::new(
            Some(p2c_path("MOSS_TEST_HELPER_PATH")),
            tempfile::tempdir()
                .unwrap()
                .path()
                .join("P2C-parent-force-cache"),
        ));
        let worker_manager = manager.clone();
        std::thread::spawn(move || {
            let _ = worker_manager.transcribe(p2c_input(
                p2c_request_id(103),
                sha256_text("moss-p2c-parent-force"),
                p2c_samples("MOSS_P2C_PARENT_PCM_PATH"),
            ));
        });
        let helper_pid = p2c_wait_for_active(&manager);
        p2c_write_json(
            "MOSS_P2C_PARENT_READY",
            &serde_json::json!({
                "schema_version": 1,
                "runner_process_id": std::process::id(),
                "helper_process_id": helper_pid,
                "ready": true
            }),
        );
        loop {
            std::thread::park_timeout(Duration::from_secs(60));
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires a frozen 480-600 second f32le fixture and Intel Arc"]
    fn p2c_real_three_hundred_second_single_child_gate() {
        let samples = p2c_samples("MOSS_P2C_300_PCM_PATH");
        assert_eq!(samples.len(), 300 * 16_000);
        let cache = tempfile::tempdir().unwrap();
        let manager = MossHelperManager::new(
            Some(p2c_path("MOSS_TEST_HELPER_PATH")),
            cache.path().join("P2C-300-performance-cache"),
        );
        let mut input = p2c_input(
            p2c_request_id(300),
            sha256_text("moss-p2c-single-300s"),
            samples,
        );
        input.timeout = Duration::from_secs(600);
        let result = manager.transcribe(input).unwrap();
        assert_eq!(result.helper_runs.len(), 1);
        assert_eq!(result.helper_runs[0].input_duration_ms, 300_000);
        assert_eq!(result.helper_runs[0].residual_process_count, 0);
        assert_eq!(result.helper_runs[0].terminal_count, 1);
        assert!(result.completed.last_timestamp_ms >= 290_000);
        p2c_write_json(
            "MOSS_P2C_300_EVIDENCE",
            &serde_json::json!({
                "schema_version": 1,
                "stage": "MOSS_V3_P2C_300S_SINGLE_CHILD",
                "status": "PASS",
                "duration_seconds": 300,
                "helper_binary_sha256": format!("{:X}", Sha256::digest(std::fs::read(p2c_path("MOSS_TEST_HELPER_PATH")).unwrap())),
                "supervisor_wall_elapsed_ms": result.supervisor_wall_elapsed_ms,
                "supervisor_wall_rtf": result.supervisor_wall_rtf,
                "last_timestamp_ms": result.completed.last_timestamp_ms,
                "segment_count": result.completed.segment_count,
                "helper_process_id": result.helper_process_id,
                "raw_text_sha256": result.completed.raw_text_sha256,
                "clean_text_sha256": result.completed.clean_text_sha256,
                "helper_runs": result.helper_runs,
                "residual_process_count": result.residual_process_count,
                "transcript_in_evidence": false,
                "absolute_paths_in_evidence": false
            }),
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires a frozen 480-600 second f32le fixture and Intel Arc"]
    fn p2c_real_four_hundred_eighty_second_wall_rtf_gate() {
        let samples = p2c_samples("MOSS_P2C_480_PCM_PATH");
        assert_eq!(samples.len(), 480 * 16_000);
        let cache = tempfile::tempdir().unwrap();
        let manager = MossHelperManager::new(
            Some(p2c_path("MOSS_TEST_HELPER_PATH")),
            cache.path().join("P2C-480-performance-cache"),
        );
        let mut input = p2c_input(
            p2c_request_id(480),
            sha256_text("moss-p2c-performance-480s"),
            samples,
        );
        input.timeout = Duration::from_secs(900);
        let result = manager.transcribe(input).unwrap();
        assert!(result.supervisor_wall_rtf <= 1.0);
        assert_eq!(result.residual_process_count, 0);
        assert_eq!(result.helper_runs.len(), 1);
        assert!(result.helper_process_id > 0);
        assert_eq!(result.helper_runs[0].input_offset_ms, 0);
        assert_eq!(result.helper_runs[0].input_duration_ms, 480_000);
        assert!(result
            .helper_runs
            .iter()
            .all(|run| run.residual_process_count == 0 && run.terminal_count == 1));
        assert!(result.completed.last_timestamp_ms >= 470_000);
        assert!(manager.status().active_request_ids.is_empty());
        p2c_write_json(
            "MOSS_P2C_480_EVIDENCE",
            &serde_json::json!({
                "schema_version": 1,
                "stage": "MOSS_V3_P2C_480S_PERFORMANCE",
                "status": "PASS",
                "duration_seconds": 480,
                "max_wall_rtf": 1.0,
                "helper_binary_sha256": format!("{:X}", Sha256::digest(std::fs::read(p2c_path("MOSS_TEST_HELPER_PATH")).unwrap())),
                "supervisor_wall_elapsed_ms": result.supervisor_wall_elapsed_ms,
                "supervisor_wall_rtf": result.supervisor_wall_rtf,
                "native_run_elapsed_ms": result.completed.native_run_elapsed_ms,
                "native_rtf": result.completed.native_rtf,
                "last_timestamp_ms": result.completed.last_timestamp_ms,
                "segment_count": result.completed.segment_count,
                "raw_text_sha256": result.completed.raw_text_sha256,
                "clean_text_sha256": result.completed.clean_text_sha256,
                "helper_runs": result.helper_runs,
                "helper_process_id": result.helper_process_id,
                "residual_process_count": result.residual_process_count,
                "transcript_in_evidence": false,
                "absolute_paths_in_evidence": false
            }),
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires a frozen 480-600 second f32le fixture and Intel Arc"]
    fn p2c_real_six_hundred_second_wall_rtf_gate() {
        let samples = p2c_samples("MOSS_P2C_LONG_PCM_PATH");
        assert_eq!(samples.len(), 600 * 16_000);
        let cache = tempfile::tempdir().unwrap();
        let manager = MossHelperManager::new(
            Some(p2c_path("MOSS_TEST_HELPER_PATH")),
            cache.path().join("P2C-performance-cache"),
        );
        let mut input = p2c_input(
            p2c_request_id(600),
            sha256_text("moss-p2c-performance-600s"),
            samples,
        );
        input.timeout = Duration::from_secs(900);
        input.max_wall_rtf = Some(1.0);
        let result = manager.transcribe(input).unwrap();
        assert!(result.supervisor_wall_rtf <= 1.0);
        assert_eq!(result.residual_process_count, 0);
        assert_eq!(result.helper_runs.len(), 1);
        assert!(result.helper_process_id > 0);
        assert_eq!(result.helper_runs[0].input_offset_ms, 0);
        assert_eq!(result.helper_runs[0].input_duration_ms, 600_000);
        assert!(result
            .helper_runs
            .iter()
            .all(|run| run.residual_process_count == 0 && run.terminal_count == 1));
        assert!(result.completed.last_timestamp_ms >= 590_000);
        assert!(manager.status().active_request_ids.is_empty());
        p2c_write_json(
            "MOSS_P2C_PERFORMANCE_EVIDENCE",
            &serde_json::json!({
                "schema_version": 1,
                "stage": "MOSS_V3_P2C_600S_PERFORMANCE",
                "status": "PASS",
                "duration_seconds": 600,
                "max_wall_rtf": 1.0,
                "helper_binary_sha256": format!("{:X}", Sha256::digest(std::fs::read(p2c_path("MOSS_TEST_HELPER_PATH")).unwrap())),
                "wall_elapsed_ms": result.completed.wall_elapsed_ms,
                "wall_rtf": result.completed.wall_rtf,
                "supervisor_wall_elapsed_ms": result.supervisor_wall_elapsed_ms,
                "supervisor_wall_rtf": result.supervisor_wall_rtf,
                "native_run_elapsed_ms": result.completed.native_run_elapsed_ms,
                "native_rtf": result.completed.native_rtf,
                "segment_count": result.completed.segment_count,
                "raw_text_sha256": result.completed.raw_text_sha256,
                "clean_text_sha256": result.completed.clean_text_sha256,
                "helper_peak_job_memory_bytes": result.helper_peak_job_memory_bytes,
                "helper_process_id": result.helper_process_id,
                "residual_process_count": result.residual_process_count,
                "helper_runs": result.helper_runs,
                "transcript_in_evidence": false,
                "absolute_paths_in_evidence": false
            }),
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires the frozen 737.728 second WAV, release helper, Q8 model, and Intel Arc"]
    fn r2_real_737_second_single_native_session_gate() {
        let samples = p2c_pcm16_wav_samples("MOSS_R2_737_WAV_PATH");
        assert_eq!(samples.len(), 11_803_648);
        let cache = tempfile::tempdir().unwrap();
        let manager = MossHelperManager::new(
            Some(p2c_path("MOSS_TEST_HELPER_PATH")),
            cache.path().join("R2-737-single-session-cache"),
        );
        let mut input = p2c_input(
            p2c_request_id(737),
            sha256_text("moss-r2-frozen-real-business-737728ms"),
            samples,
        );
        input.timeout = Duration::from_secs(900);
        input.max_wall_rtf = Some(1.0);

        let result = manager.transcribe(input).unwrap();
        let limits = result.completed.native_session_limits;
        assert_eq!(result.helper_runs.len(), 1);
        assert!(result.helper_process_id > 0);
        assert_eq!(result.residual_process_count, 0);
        assert_eq!(result.helper_runs[0].chunk_index, 0);
        assert_eq!(result.helper_runs[0].input_offset_ms, 0);
        assert_eq!(result.helper_runs[0].input_duration_ms, 737_728);
        assert_eq!(result.helper_runs[0].terminal_count, 1);
        assert_eq!(result.helper_runs[0].residual_process_count, 0);
        assert!(result.supervisor_wall_rtf <= 1.0);
        assert_eq!(
            limits.effective_n_ctx,
            moss_helper::native::MOSS_SESSION_N_CTX
        );
        assert!(limits.effective_max_audio_ms >= 737_728);
        assert!(limits.max_kv_bytes > 0);
        assert!(result.completed.last_timestamp_ms >= 720_000);
        assert!(result.completed.last_timestamp_ms <= 737_728);
        assert!(manager.status().active_request_ids.is_empty());

        p2c_write_json(
            "MOSS_R2_737_EVIDENCE",
            &serde_json::json!({
                "schema_version": 1,
                "stage": "MOSS_R2_737S_SINGLE_NATIVE_SESSION",
                "status": "PASS",
                "duration_ms": 737_728,
                "max_wall_rtf": 1.0,
                "supervisor_wall_elapsed_ms": result.supervisor_wall_elapsed_ms,
                "supervisor_wall_rtf": result.supervisor_wall_rtf,
                "native_run_elapsed_ms": result.completed.native_run_elapsed_ms,
                "native_rtf": result.completed.native_rtf,
                "native_session_limits": limits,
                "last_timestamp_ms": result.completed.last_timestamp_ms,
                "segment_count": result.completed.segment_count,
                "helper_process_id": result.helper_process_id,
                "helper_total_processes": result.helper_total_processes,
                "helper_peak_job_memory_bytes": result.helper_peak_job_memory_bytes,
                "residual_process_count": result.residual_process_count,
                "helper_runs": result.helper_runs,
                "raw_text_sha256": result.completed.raw_text_sha256,
                "clean_text_sha256": result.completed.clean_text_sha256,
                "transcript_in_evidence": false,
                "absolute_paths_in_evidence": false
            }),
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires release packaged helper, real 600 second fixture, Q8 model, and Intel Arc"]
    fn p2d_real_duplicate_request_starts_no_second_helper() {
        let samples = p2c_samples("MOSS_P2C_LONG_PCM_PATH");
        let cache = tempfile::tempdir().unwrap();
        let cache_root = cache.path().join("P2D-duplicate-cache");
        let manager = Arc::new(MossHelperManager::new(
            Some(p2c_path("MOSS_TEST_HELPER_PATH")),
            cache_root.clone(),
        ));
        let input = p2c_input(
            p2c_request_id(701),
            sha256_text("moss-p2d-duplicate"),
            samples,
        );
        let worker = manager.clone();
        let first = input.clone();
        let thread = std::thread::spawn(move || worker.transcribe(first));
        let _pid = p2c_wait_for_active(&manager);
        assert!(matches!(
            manager.transcribe(input),
            Err(ManagerError::DuplicateRequest)
        ));
        assert_eq!(manager.status().helper_processes_started, 1);
        assert!(manager.cancel(&p2c_request_id(701)).unwrap());
        assert!(matches!(
            thread.join().unwrap(),
            Err(ManagerError::Cancelled)
        ));
        assert!(manager.status().active_request_ids.is_empty());
        assert!(manager.status().active_process_ids.is_empty());
        assert!(!cache_root.join(p2c_request_id(701)).exists());
        p2c_write_json(
            "MOSS_P2D_DUPLICATE_EVIDENCE",
            &serde_json::json!({
                "schema_version": 1,
                "stage": "MOSS_V3_P2D_DUPLICATE_REQUEST",
                "status": "PASS",
                "first_helper_processes_started": 1,
                "duplicate_error": "MOSS_DUPLICATE_REQUEST",
                "second_helper_started": false,
                "residual_process_count": 0,
                "cache_entry_after": false,
                "transcript_in_evidence": false,
                "absolute_paths_in_evidence": false
            }),
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "allocates a 20 minute f32le boundary fixture"]
    fn p2d_above_supported_duration_is_rejected_before_helper_launch() {
        let samples = vec![0.0_f32; (MAX_REQUEST_SECONDS as usize * 16_000) + 1];
        let cache = tempfile::tempdir().unwrap();
        let cache_root = cache.path().join("P2D-over-limit-cache");
        let manager =
            MossHelperManager::new(Some(p2c_path("MOSS_TEST_HELPER_PATH")), cache_root.clone());
        let request_id = p2c_request_id(702);
        let input = p2c_input(
            request_id.clone(),
            sha256_text("moss-p2d-over-supported-duration"),
            samples,
        );
        assert!(matches!(
            manager.transcribe(input),
            Err(ManagerError::AudioTooLong)
        ));
        assert_eq!(manager.status().helper_processes_started, 0);
        assert!(manager.status().active_request_ids.is_empty());
        assert!(manager.status().active_process_ids.is_empty());
        assert!(!cache_root.join(&request_id).exists());
        p2c_write_json(
            "MOSS_P2D_SECOND_CHUNK_FAILURE_EVIDENCE",
            &serde_json::json!({
                "schema_version": 2,
                "stage": "MOSS_V3_R2_OVER_SUPPORTED_DURATION",
                "status": "PASS",
                "maximum_supported_seconds": MAX_REQUEST_SECONDS,
                "tested_sample_count": (MAX_REQUEST_SECONDS * 16_000) + 1,
                "partial_success_returned": false,
                "failure_code": "AUDIO_TOO_LONG",
                "helper_processes_started": 0,
                "residual_process_count": 0,
                "cache_entry_after": false,
                "transcript_in_evidence": false,
                "absolute_paths_in_evidence": false
            }),
        );
    }
}
