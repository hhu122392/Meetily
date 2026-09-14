use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, SqliteConnection, SqlitePool};
use tauri::State;
use uuid::Uuid;

use crate::database::moss::{
    AudioTokenAlignmentInput, AudioTokenAlignmentRecord, BoundPerson, MossCandidateRepository,
    MossCompletion, MossRunDiagnosticsInput, MossRunDiagnosticsRecord, MossRunRecord,
    MossStoreError, NewMossRun, SegmentOverrideInput, TermCorrectionInput,
};
use crate::meeting_context::validation::comparison_key;
use crate::meeting_context::{
    normalize_and_validate_container, AttendanceStatus, MeetingContextContainer,
    MeetingContextSnapshot, SnapshotPerson,
};
use crate::moss_audio_token_alignment::{
    align_managed_transcription_with_audio_tokens, build_audio_token_track,
    machine_term_suggestions_for_managed, ContextTerm, MachineTermSuggestion,
    R5_ALIGNMENT_MODEL_NAME,
};
use crate::moss_helper::commands::{
    transcribe_file_with_preparation, MossCommandError, MossTranscribeFileRequest,
};
use crate::moss_helper::manager::MossHelperManager;
use crate::state::AppState;
use crate::storage::operation_lock::{begin_storage_operation, StorageOperationKind};
use crate::storage::validation::available_bytes_for_path;
#[cfg(test)]
use crate::storage::validation::available_bytes_for_path_from_disks;
use crate::storage::StorageLayoutState;

const SCHEMA_VERSION: u8 = 1;
const MODEL_FILE_NAME: &str = "MOSS-Transcribe-Diarize-Q8_0.gguf";
const EXPECTED_RUNTIME_MANIFEST_SHA256: &str =
    "c266622ee16a69c458eba57fd2b2f2114b9c5f2e34292e9603f1ce46c7a1dc73";
const EMPTY_CONTEXT_LABEL: &[u8] = b"meetily-moss-empty-context-v1";
const SYSTEM_STATUS_TTL: Duration = Duration::from_secs(30);
const TRANSCRIPTION_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1_000;

#[derive(Clone)]
pub struct MossReviewState {
    operation_lock: Arc<tokio::sync::Mutex<()>>,
    active_requests: Arc<Mutex<HashMap<String, String>>>,
    cancellation_requests: Arc<Mutex<HashSet<String>>>,
    status_cache: Arc<tokio::sync::Mutex<Option<CachedSystemStatus>>>,
}

impl MossReviewState {
    pub fn new() -> Self {
        Self {
            operation_lock: Arc::new(tokio::sync::Mutex::new(())),
            active_requests: Arc::new(Mutex::new(HashMap::new())),
            cancellation_requests: Arc::new(Mutex::new(HashSet::new())),
            status_cache: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    fn remember_request(&self, run_id: String, request_id: String) {
        self.active_requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(run_id, request_id);
    }

    fn forget_request(&self, run_id: &str) {
        self.active_requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(run_id);
        self.forget_cancellation_request(run_id);
    }

    fn forget_cancellation_request(&self, run_id: &str) {
        self.cancellation_requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(run_id);
    }

    fn request_for_run(&self, run_id: &str) -> Option<String> {
        self.active_requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(run_id)
            .cloned()
    }

    fn remember_cancellation_request(&self, run_id: String) {
        self.cancellation_requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(run_id);
    }

    fn is_cancel_requested(&self, run_id: &str) -> bool {
        self.cancellation_requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(run_id)
    }

    fn has_active_request(&self) -> bool {
        !self
            .active_requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty()
    }
}

impl Default for MossReviewState {
    fn default() -> Self {
        Self::new()
    }
}

struct CachedSystemStatus {
    expires_at: Instant,
    status: MossSystemStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MossApiError {
    code: String,
    retryable: bool,
    debug_id: String,
}

impl MossApiError {
    fn new(code: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            retryable,
            debug_id: format!("moss-{}", Uuid::new_v4().simple()),
        }
    }

    fn operation_failed() -> Self {
        Self::new("MOSS_OPERATION_FAILED", true)
    }
}

impl From<MossStoreError> for MossApiError {
    fn from(error: MossStoreError) -> Self {
        match error {
            MossStoreError::ActiveRunExists => Self::new("MOSS_RUN_ALREADY_ACTIVE", false),
            MossStoreError::RunNotFound => Self::new("MOSS_RUN_NOT_FOUND", false),
            MossStoreError::CandidateNotFound => Self::new("MOSS_CANDIDATE_NOT_READY", false),
            MossStoreError::TranscriptConflict { .. } => {
                Self::new("MOSS_ACTIVATION_CONFLICT", true)
            }
            MossStoreError::RollbackConflict { .. } => Self::new("MOSS_ROLLBACK_CONFLICT", true),
            MossStoreError::ActiveActivationExists => Self::new("MOSS_ACTIVATION_CONFLICT", true),
            MossStoreError::ActivationNotFound => Self::new("MOSS_ROLLBACK_CONFLICT", false),
            MossStoreError::InvalidRunState => Self::new("MOSS_CANDIDATE_NOT_READY", true),
            MossStoreError::MeetingNotFound
            | MossStoreError::Database(_)
            | MossStoreError::Serialization(_)
            | MossStoreError::InvalidInput(_)
            | MossStoreError::InjectedFailure => Self::operation_failed(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MossSystemStatus {
    schema_version: u8,
    availability: String,
    installed: bool,
    version: Option<String>,
    runtime_sha256: Option<String>,
    model_sha256: Option<String>,
    model_bytes: Option<u64>,
    available_disk_bytes: Option<u64>,
    device_name: Option<String>,
    health: String,
    supports_native_hotwords: bool,
    detail_code: Option<String>,
    checked_at: String,
}

#[derive(Debug, Clone)]
struct MossInstallation {
    runtime_directory: PathBuf,
    runtime_sha256: String,
    model_path: PathBuf,
    model_sha256: String,
    model_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MossRunProgress {
    stage: String,
    percentage: u8,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MossRunSummary {
    run_id: String,
    meeting_id: String,
    state: String,
    progress: Option<MossRunProgress>,
    created_at: String,
    updated_at: String,
    completed_at: Option<String>,
    error_code: Option<String>,
    can_cancel: bool,
    candidate_revision: Option<u64>,
    candidate_sha256: Option<String>,
    language_requested: Option<String>,
    language_resolved: Option<String>,
    decode_parameters_json: Option<String>,
    decode_parameters_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MossParticipant {
    person_id: String,
    display_name: String,
    attendance: String,
    department: Option<String>,
    role: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MossSpeakerBinding {
    speaker_label: String,
    person_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MossTranscriptSegment {
    segment_id: String,
    start_ms: u64,
    end_ms: u64,
    speaker_label: Option<String>,
    text: String,
    resolved_person_id: Option<String>,
    segment_override_person_id: Option<String>,
    speaker_resolution: String,
    text_source_layer: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MossTranscriptVersion {
    source: String,
    revision: u64,
    sha256: String,
    segments: Vec<MossTranscriptSegment>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MossCandidateVersion {
    source: String,
    revision: u64,
    sha256: String,
    segments: Vec<MossTranscriptSegment>,
    run_id: String,
    is_active: bool,
    alignments: Vec<MossCandidateAlignment>,
    diagnostics: Option<MossRunDiagnostics>,
    audio_token_alignment: Option<MossAudioTokenAlignment>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MossCandidateAlignment {
    segment_id: String,
    raw_segment_index: u32,
    raw_start_ms: u64,
    raw_end_ms: u64,
    raw_text_sha256: String,
    alignment_method: String,
    confidence: Option<f64>,
    source_anchor_ids: Vec<String>,
    source_transcript_sha256: String,
    audio_token_track_sha256: Option<String>,
    first_audio_token_index: Option<u32>,
    last_audio_token_index: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MossAudioTokenAlignment {
    status: String,
    audio_sha256: String,
    audio_duration_ms: Option<u64>,
    model_name: Option<String>,
    model_sha256: Option<String>,
    program_sha256: Option<String>,
    parameters_sha256: Option<String>,
    token_track_sha256: Option<String>,
    backend: Option<String>,
    global_match_coverage: Option<f64>,
    token_aligned_segment_count: u32,
    fallback_raw_segment_count: u32,
    fallback_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MossRunDiagnostics {
    audio_duration_ms: u64,
    activity_frame_ms: u32,
    activity_threshold_dbfs: f64,
    first_active_ms: Option<u64>,
    last_active_ms: Option<u64>,
    model_last_timestamp_ms: u64,
    tail_delta_ms: Option<i64>,
    aligned_segment_count: u32,
    fallback_segment_count: u32,
    source_anchor_count: u32,
    source_hash_verified: bool,
    source_expected_sha256: String,
    source_actual_sha256: String,
    fallback_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MossTermCorrection {
    correction_id: String,
    segment_id: String,
    original_text: String,
    corrected_text: String,
    matched_alias: String,
    canonical: String,
    rule_id: String,
    context_revision: u64,
    state: String,
    source_layer: String,
    machine_source: Option<MossMachineCorrectionSource>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MossMachineCorrectionSource {
    term_id: String,
    context_sha256: String,
    token_track_sha256: String,
    model_sha256: String,
    first_token_index: u32,
    last_token_index: u32,
    confidence: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MossActivationState {
    active_run_id: Option<String>,
    active_activation_id: Option<String>,
    current_transcript_sha256: String,
    can_activate: bool,
    activate_blocker: Option<String>,
    can_rollback: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MossCandidateReview {
    schema_version: u8,
    meeting_id: String,
    run: MossRunSummary,
    current: MossTranscriptVersion,
    candidate: MossCandidateVersion,
    participants: Vec<MossParticipant>,
    anonymous_speakers: Vec<String>,
    bindings: Vec<MossSpeakerBinding>,
    corrections: Vec<MossTermCorrection>,
    activation: MossActivationState,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MossWorkspace {
    schema_version: u8,
    meeting_id: String,
    system: MossSystemStatus,
    runs: Vec<MossRunSummary>,
    selected_run_id: Option<String>,
    review: Option<MossCandidateReview>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MossWorkspaceRequest {
    meeting_id: String,
    #[serde(default)]
    selected_run_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartMossRunRequest {
    meeting_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MossRunRequest {
    meeting_id: String,
    run_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveMossSpeakerBindingRequest {
    meeting_id: String,
    run_id: String,
    speaker_label: String,
    person_id: Option<String>,
    expected_candidate_revision: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveMossSegmentOverrideRequest {
    meeting_id: String,
    run_id: String,
    segment_id: String,
    person_id: Option<String>,
    expected_candidate_revision: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetMossCorrectionStateRequest {
    meeting_id: String,
    run_id: String,
    correction_id: String,
    applied: bool,
    expected_candidate_revision: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateMossCandidateSegmentRequest {
    meeting_id: String,
    run_id: String,
    segment_id: String,
    text: String,
    expected_candidate_revision: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivateMossCandidateRequest {
    meeting_id: String,
    run_id: String,
    expected_candidate_revision: u64,
    expected_current_transcript_sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RollbackMossActivationRequest {
    meeting_id: String,
    activation_id: String,
    expected_current_transcript_sha256: String,
}

#[derive(Debug, Clone, FromRow)]
struct CandidateSegmentRow {
    segment_id: String,
    segment_index: i64,
    start_ms: i64,
    end_ms: i64,
    speaker_label: String,
    raw_text: String,
    correction_text: Option<String>,
    correction_machine_source_id: Option<String>,
    replacement_text: Option<String>,
    override_person_id: Option<String>,
    override_person_display_name: Option<String>,
    binding_person_id: Option<String>,
    binding_person_display_name: Option<String>,
    alignment_raw_segment_index: Option<i64>,
    alignment_raw_start_ms: Option<i64>,
    alignment_raw_end_ms: Option<i64>,
    alignment_raw_text_sha256: Option<String>,
    alignment_method: Option<String>,
    alignment_confidence: Option<f64>,
    alignment_source_anchor_ids_json: Option<String>,
    alignment_source_transcript_sha256: Option<String>,
    audio_token_track_sha256: Option<String>,
    first_audio_token_index: Option<i64>,
    last_audio_token_index: Option<i64>,
}

#[derive(Debug, Clone, FromRow)]
struct CorrectionRow {
    correction_id: String,
    segment_id: String,
    segment_index: i64,
    original_text: String,
    replacement_text: String,
    result_text: String,
    start_char: i64,
    end_char: i64,
    rule_id: String,
    reverted_at: Option<String>,
    machine_term_id: Option<String>,
    machine_context_sha256: Option<String>,
    machine_token_track_sha256: Option<String>,
    machine_model_sha256: Option<String>,
    machine_first_token_index: Option<i64>,
    machine_last_token_index: Option<i64>,
    machine_confidence: Option<f64>,
}

#[derive(Debug, Clone, FromRow)]
struct ActiveOverrideRow {
    override_id: String,
    replacement_text: Option<String>,
    person_id: Option<String>,
    person_display_name: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
struct ActivationRow {
    activation_id: String,
    run_id: String,
    activated_transcript_sha256: String,
}

#[derive(Debug, Clone, FromRow, Serialize)]
struct CurrentTranscriptRow {
    id: String,
    meeting_id: String,
    transcript: String,
    timestamp: String,
    summary: Option<String>,
    action_items: Option<String>,
    key_points: Option<String>,
    audio_start_time: Option<f64>,
    audio_end_time: Option<f64>,
    duration: Option<f64>,
    speaker: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
struct ActivationSegmentRow {
    transcript_id: String,
    speaker_label: String,
    resolved_person_id: Option<String>,
    override_id: Option<String>,
    binding_id: Option<String>,
}

#[derive(Debug, Clone)]
struct CandidateData {
    revision: u64,
    sha256: String,
    segments: Vec<MossTranscriptSegment>,
    anonymous_speakers: Vec<String>,
    bindings: Vec<MossSpeakerBinding>,
    corrections: Vec<MossTermCorrection>,
    referenced_people: Vec<BoundPerson>,
    alignments: Vec<MossCandidateAlignment>,
    diagnostics: Option<MossRunDiagnostics>,
    audio_token_alignment: Option<MossAudioTokenAlignment>,
}

#[derive(Debug, Deserialize)]
struct MeetingMetadataEnvelope {
    #[serde(default)]
    meeting_context: Option<MeetingContextContainer>,
}

fn now_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sha256_serialized<T: Serialize>(value: &T) -> Result<String, MossApiError> {
    serde_json::to_vec(value)
        .map(|bytes| sha256_bytes(&bytes))
        .map_err(|_| MossApiError::operation_failed())
}

fn sha256_file(path: &Path) -> Result<String, MossApiError> {
    let mut file = File::open(path).map_err(|_| MossApiError::operation_failed())?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| MossApiError::operation_failed())?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn installation_from_paths(
    helper: &MossHelperManager,
    runtime_directory: PathBuf,
    model_path: PathBuf,
) -> Result<MossInstallation, MossApiError> {
    if !helper.is_available() {
        return Err(MossApiError::new("MOSS_NOT_INSTALLED", false));
    }
    let runtime_manifest = runtime_directory.join("contract.json");
    if !runtime_manifest.is_file() || !model_path.is_file() {
        return Err(MossApiError::new("MOSS_NOT_INSTALLED", false));
    }
    let runtime_sha256 = sha256_file(&runtime_manifest)?;
    let model_bytes = std::fs::metadata(&model_path)
        .map_err(|_| MossApiError::operation_failed())?
        .len();
    let model_sha256 = sha256_file(&model_path)?;
    if runtime_sha256 != EXPECTED_RUNTIME_MANIFEST_SHA256
        || model_bytes != moss_helper::native::EXPECTED_MODEL_BYTES
        || !model_sha256.eq_ignore_ascii_case(moss_helper::native::EXPECTED_MODEL_SHA256)
    {
        return Err(MossApiError::new("MOSS_UNHEALTHY", false));
    }
    Ok(MossInstallation {
        runtime_directory,
        runtime_sha256,
        model_path,
        model_sha256,
        model_bytes,
    })
}

fn build_system_status(
    helper: Arc<MossHelperManager>,
    runtime_directory: PathBuf,
    model_path: PathBuf,
) -> MossSystemStatus {
    let checked_at = now_timestamp();
    #[cfg(not(windows))]
    {
        let _ = (helper, runtime_directory, model_path);
        return MossSystemStatus {
            schema_version: SCHEMA_VERSION,
            availability: "unsupported".to_owned(),
            installed: false,
            version: None,
            runtime_sha256: None,
            model_sha256: None,
            model_bytes: None,
            available_disk_bytes: None,
            device_name: None,
            health: "unknown".to_owned(),
            supports_native_hotwords: false,
            detail_code: Some("MOSS_PLATFORM_UNSUPPORTED".to_owned()),
            checked_at,
        };
    }
    #[cfg(windows)]
    {
        let available_disk_bytes = available_bytes_for_path(&runtime_directory);
        let runtime_manifest = runtime_directory.join("contract.json");
        let runtime_sha256 = runtime_manifest
            .is_file()
            .then(|| sha256_file(&runtime_manifest).ok())
            .flatten();
        let model_metadata = std::fs::metadata(&model_path).ok();
        let model_bytes = model_metadata.as_ref().map(std::fs::Metadata::len);
        let model_sha256 = model_metadata
            .as_ref()
            .and_then(|_| sha256_file(&model_path).ok());
        let installed =
            helper.is_available() && runtime_manifest.is_file() && model_metadata.is_some();
        if !installed {
            return MossSystemStatus {
                schema_version: SCHEMA_VERSION,
                availability: "not_installed".to_owned(),
                installed: false,
                version: None,
                runtime_sha256,
                model_sha256,
                model_bytes,
                available_disk_bytes,
                device_name: None,
                health: "unknown".to_owned(),
                supports_native_hotwords: false,
                detail_code: Some("MOSS_ARTIFACTS_NOT_INSTALLED".to_owned()),
                checked_at,
            };
        }
        let artifacts_valid = runtime_sha256.as_deref() == Some(EXPECTED_RUNTIME_MANIFEST_SHA256)
            && model_bytes == Some(moss_helper::native::EXPECTED_MODEL_BYTES)
            && model_sha256.as_deref().is_some_and(|hash| {
                hash.eq_ignore_ascii_case(moss_helper::native::EXPECTED_MODEL_SHA256)
            });
        if !artifacts_valid {
            return MossSystemStatus {
                schema_version: SCHEMA_VERSION,
                availability: "unhealthy".to_owned(),
                installed: true,
                version: Some(moss_helper::native::EXPECTED_RUNTIME_VERSION.to_owned()),
                runtime_sha256,
                model_sha256,
                model_bytes,
                available_disk_bytes,
                device_name: None,
                health: "failed".to_owned(),
                supports_native_hotwords: false,
                detail_code: Some("MOSS_ARTIFACT_HASH_MISMATCH".to_owned()),
                checked_at,
            };
        }
        let request_id = Uuid::new_v4().to_string();
        match helper.probe(runtime_directory, request_id, None, Duration::from_secs(30)) {
            Ok(probe) => MossSystemStatus {
                schema_version: SCHEMA_VERSION,
                availability: "ready".to_owned(),
                installed: true,
                version: Some(probe.runtime_version),
                runtime_sha256,
                model_sha256,
                model_bytes,
                available_disk_bytes,
                device_name: Some(probe.device.description),
                health: "healthy".to_owned(),
                supports_native_hotwords: false,
                detail_code: None,
                checked_at,
            },
            Err(error) => {
                let public = MossCommandError::from(error);
                MossSystemStatus {
                    schema_version: SCHEMA_VERSION,
                    availability: "unhealthy".to_owned(),
                    installed: true,
                    version: Some(moss_helper::native::EXPECTED_RUNTIME_VERSION.to_owned()),
                    runtime_sha256,
                    model_sha256,
                    model_bytes,
                    available_disk_bytes,
                    device_name: None,
                    health: "failed".to_owned(),
                    supports_native_hotwords: false,
                    detail_code: Some(public.code.to_owned()),
                    checked_at,
                }
            }
        }
    }
}

async fn system_status(
    layout: &StorageLayoutState,
    helper: Arc<MossHelperManager>,
    review_state: &MossReviewState,
) -> MossSystemStatus {
    let mut cache = review_state.status_cache.lock().await;
    if let Some(cached) = cache.as_ref() {
        if Instant::now() < cached.expires_at || review_state.has_active_request() {
            return cached.status.clone();
        }
    }
    let runtime_directory = layout.layout().paths().moss_runtime.clone();
    let model_path = layout.layout().paths().moss_models.join(MODEL_FILE_NAME);
    let status = tauri::async_runtime::spawn_blocking(move || {
        build_system_status(helper, runtime_directory, model_path)
    })
    .await
    .unwrap_or_else(|_| MossSystemStatus {
        schema_version: SCHEMA_VERSION,
        availability: "unhealthy".to_owned(),
        installed: false,
        version: None,
        runtime_sha256: None,
        model_sha256: None,
        model_bytes: None,
        available_disk_bytes: None,
        device_name: None,
        health: "failed".to_owned(),
        supports_native_hotwords: false,
        detail_code: Some("MOSS_STATUS_FAILED".to_owned()),
        checked_at: now_timestamp(),
    });
    *cache = Some(CachedSystemStatus {
        expires_at: Instant::now() + SYSTEM_STATUS_TTL,
        status: status.clone(),
    });
    status
}

fn load_context_container(folder: &Path) -> Result<Option<MeetingContextContainer>, MossApiError> {
    let metadata_path = folder.join("metadata.json");
    if !metadata_path.is_file() {
        return Ok(None);
    }
    let bytes = std::fs::read(metadata_path).map_err(|_| MossApiError::operation_failed())?;
    let envelope: MeetingMetadataEnvelope =
        serde_json::from_slice(&bytes).map_err(|_| MossApiError::operation_failed())?;
    envelope
        .meeting_context
        .map(normalize_and_validate_container)
        .transpose()
        .map_err(|_| MossApiError::operation_failed())
}

fn context_for_hash(
    container: Option<&MeetingContextContainer>,
    context_sha256: &str,
) -> Option<MeetingContextSnapshot> {
    container?
        .contexts
        .iter()
        .find(|context| context.context_sha256.eq_ignore_ascii_case(context_sha256))
        .cloned()
}

fn participant_from_snapshot(person: &SnapshotPerson) -> Option<MossParticipant> {
    let attendance = match person.attendance {
        AttendanceStatus::Attending => "attending",
        AttendanceStatus::Expected => "expected",
        AttendanceStatus::Guest => "guest",
        AttendanceStatus::Absent => return None,
    };
    Some(MossParticipant {
        person_id: person.person_id.clone(),
        display_name: person.display_name.clone(),
        attendance: attendance.to_owned(),
        department: person.department.clone(),
        role: person.role.clone(),
    })
}

fn millis(value: Option<f64>) -> u64 {
    let value = value.unwrap_or(0.0);
    if value.is_finite() && value > 0.0 {
        (value * 1_000.0).round().max(0.0) as u64
    } else {
        0
    }
}

fn correction_before_text(row: &CorrectionRow) -> String {
    let result = row.result_text.chars().collect::<Vec<_>>();
    let start = usize::try_from(row.start_char).unwrap_or(usize::MAX);
    let replacement_len = row.replacement_text.chars().count();
    if start > result.len() || start.saturating_add(replacement_len) > result.len() {
        return row.original_text.clone();
    }
    let mut before = String::new();
    before.extend(result[..start].iter());
    before.push_str(&row.original_text);
    before.extend(result[start + replacement_len..].iter());
    before
}

async fn candidate_revision(
    connection: &mut SqliteConnection,
    run_id: &str,
) -> Result<u64, MossApiError> {
    let revision: i64 = sqlx::query_scalar(
        r#"
        SELECT 1
          + (SELECT COUNT(*) + COUNT(reverted_at)
               FROM moss_term_corrections
              WHERE segment_id IN (
                    SELECT segment_id FROM moss_candidate_segments WHERE run_id = ?
              ))
          + (SELECT COUNT(*) + COUNT(revoked_at)
               FROM moss_speaker_bindings WHERE run_id = ?)
          + (SELECT COUNT(*) + COUNT(revoked_at)
               FROM moss_segment_overrides
              WHERE segment_id IN (
                    SELECT segment_id FROM moss_candidate_segments WHERE run_id = ?
              ))
        "#,
    )
    .bind(run_id)
    .bind(run_id)
    .bind(run_id)
    .fetch_one(&mut *connection)
    .await
    .map_err(|_| MossApiError::operation_failed())?;
    u64::try_from(revision).map_err(|_| MossApiError::operation_failed())
}

async fn load_candidate_data(
    connection: &mut SqliteConnection,
    run: &MossRunRecord,
    context_revision: u64,
) -> Result<CandidateData, MossApiError> {
    let rows = sqlx::query_as::<_, CandidateSegmentRow>(
        r#"
        SELECT s.segment_id, s.segment_index, s.start_ms, s.end_ms,
               s.speaker_label, s.raw_text,
               c.result_text AS correction_text,
               mc.correction_id AS correction_machine_source_id,
               o.replacement_text, o.person_id AS override_person_id,
               o.person_display_name AS override_person_display_name,
               b.person_id AS binding_person_id,
               b.person_display_name AS binding_person_display_name,
               a.raw_segment_index AS alignment_raw_segment_index,
               a.raw_start_ms AS alignment_raw_start_ms,
               a.raw_end_ms AS alignment_raw_end_ms,
               a.raw_text_sha256 AS alignment_raw_text_sha256,
               a.alignment_method,
               a.confidence AS alignment_confidence,
               a.source_anchor_ids_json AS alignment_source_anchor_ids_json,
               a.source_transcript_sha256 AS alignment_source_transcript_sha256,
               tb.token_track_sha256 AS audio_token_track_sha256,
               tb.first_token_index AS first_audio_token_index,
               tb.last_token_index AS last_audio_token_index
          FROM moss_candidate_segments s
          LEFT JOIN moss_term_corrections c ON c.correction_id = (
               SELECT correction_id FROM moss_term_corrections
                WHERE segment_id = s.segment_id AND reverted_at IS NULL
                ORDER BY revision DESC LIMIT 1
          )
          LEFT JOIN moss_machine_term_correction_source mc
                 ON mc.correction_id = c.correction_id
          LEFT JOIN moss_segment_overrides o ON o.override_id = (
               SELECT override_id FROM moss_segment_overrides
                WHERE segment_id = s.segment_id AND revoked_at IS NULL
                ORDER BY revision DESC LIMIT 1
          )
          LEFT JOIN moss_speaker_bindings b ON b.binding_id = (
               SELECT binding_id FROM moss_speaker_bindings
                WHERE run_id = s.run_id AND speaker_label = s.speaker_label
                  AND revoked_at IS NULL
                ORDER BY created_at DESC LIMIT 1
          )
          LEFT JOIN moss_candidate_segment_alignment a ON a.segment_id = s.segment_id
          LEFT JOIN moss_candidate_audio_token_boundary tb ON tb.segment_id = s.segment_id
         WHERE s.run_id = ?
         ORDER BY s.segment_index
        "#,
    )
    .bind(&run.run_id)
    .fetch_all(&mut *connection)
    .await
    .map_err(|_| MossApiError::operation_failed())?;

    let mut labels = BTreeSet::new();
    let mut binding_people = BTreeMap::<String, Option<String>>::new();
    let mut referenced_people = BTreeMap::<String, String>::new();
    let mut segments = Vec::with_capacity(rows.len());
    let mut alignments = Vec::with_capacity(rows.len());
    for row in rows {
        let alignment = if let Some(raw_segment_index) = row.alignment_raw_segment_index {
            let source_anchor_ids = serde_json::from_str::<Vec<String>>(
                row.alignment_source_anchor_ids_json
                    .as_deref()
                    .ok_or_else(MossApiError::operation_failed)?,
            )
            .map_err(|_| MossApiError::operation_failed())?;
            MossCandidateAlignment {
                segment_id: row.segment_id.clone(),
                raw_segment_index: u32::try_from(raw_segment_index)
                    .map_err(|_| MossApiError::operation_failed())?,
                raw_start_ms: u64::try_from(
                    row.alignment_raw_start_ms
                        .ok_or_else(MossApiError::operation_failed)?,
                )
                .map_err(|_| MossApiError::operation_failed())?,
                raw_end_ms: u64::try_from(
                    row.alignment_raw_end_ms
                        .ok_or_else(MossApiError::operation_failed)?,
                )
                .map_err(|_| MossApiError::operation_failed())?,
                raw_text_sha256: row
                    .alignment_raw_text_sha256
                    .clone()
                    .ok_or_else(MossApiError::operation_failed)?,
                alignment_method: row
                    .alignment_method
                    .clone()
                    .ok_or_else(MossApiError::operation_failed)?,
                confidence: row.alignment_confidence,
                source_anchor_ids,
                source_transcript_sha256: row
                    .alignment_source_transcript_sha256
                    .clone()
                    .ok_or_else(MossApiError::operation_failed)?,
                audio_token_track_sha256: row.audio_token_track_sha256.clone(),
                first_audio_token_index: row
                    .first_audio_token_index
                    .map(u32::try_from)
                    .transpose()
                    .map_err(|_| MossApiError::operation_failed())?,
                last_audio_token_index: row
                    .last_audio_token_index
                    .map(u32::try_from)
                    .transpose()
                    .map_err(|_| MossApiError::operation_failed())?,
            }
        } else {
            MossCandidateAlignment {
                segment_id: row.segment_id.clone(),
                raw_segment_index: u32::try_from(row.segment_index)
                    .map_err(|_| MossApiError::operation_failed())?,
                raw_start_ms: u64::try_from(row.start_ms)
                    .map_err(|_| MossApiError::operation_failed())?,
                raw_end_ms: u64::try_from(row.end_ms)
                    .map_err(|_| MossApiError::operation_failed())?,
                raw_text_sha256: sha256_bytes(row.raw_text.as_bytes()),
                alignment_method: "moss_segment".to_owned(),
                confidence: None,
                source_anchor_ids: Vec::new(),
                source_transcript_sha256: run.source_transcript_sha256.clone(),
                audio_token_track_sha256: None,
                first_audio_token_index: None,
                last_audio_token_index: None,
            }
        };
        alignments.push(alignment);
        labels.insert(row.speaker_label.clone());
        binding_people
            .entry(row.speaker_label.clone())
            .or_insert_with(|| row.binding_person_id.clone());
        if let Some((person_id, display_name)) = row
            .override_person_id
            .as_ref()
            .zip(row.override_person_display_name.as_ref())
        {
            referenced_people.insert(person_id.clone(), display_name.clone());
        }
        if let Some((person_id, display_name)) = row
            .binding_person_id
            .as_ref()
            .zip(row.binding_person_display_name.as_ref())
        {
            referenced_people.insert(person_id.clone(), display_name.clone());
        }
        let (resolved_person_id, speaker_resolution) = if row.override_person_id.is_some() {
            (row.override_person_id.clone(), "segment_override")
        } else if row.binding_person_id.is_some() {
            (row.binding_person_id.clone(), "bulk_binding")
        } else {
            (None, "anonymous")
        };
        let text_source_layer = if row.replacement_text.is_some() {
            "human_edit"
        } else if row.correction_text.is_some() && row.correction_machine_source_id.is_some() {
            "audio_token_context"
        } else if row.correction_text.is_some() {
            "context_correction"
        } else {
            "raw_moss"
        };
        let text = row
            .replacement_text
            .clone()
            .or(row.correction_text.clone())
            .unwrap_or(row.raw_text);
        segments.push(MossTranscriptSegment {
            segment_id: row.segment_id,
            start_ms: u64::try_from(row.start_ms).map_err(|_| MossApiError::operation_failed())?,
            end_ms: u64::try_from(row.end_ms).map_err(|_| MossApiError::operation_failed())?,
            speaker_label: Some(row.speaker_label),
            text,
            resolved_person_id,
            segment_override_person_id: row.override_person_id,
            speaker_resolution: speaker_resolution.to_owned(),
            text_source_layer: text_source_layer.to_owned(),
        });
    }
    let diagnostics = sqlx::query_as::<_, MossRunDiagnosticsRecord>(
        r#"
        SELECT run_id, audio_duration_ms, activity_frame_ms,
               activity_threshold_dbfs, first_active_ms, last_active_ms,
               model_last_timestamp_ms, tail_delta_ms, aligned_segment_count,
               fallback_segment_count, source_anchor_count, source_hash_verified,
               source_expected_sha256, source_actual_sha256, fallback_reason, created_at
          FROM moss_run_diagnostics
         WHERE run_id = ?
        "#,
    )
    .bind(&run.run_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|_| MossApiError::operation_failed())?
    .map(moss_run_diagnostics_view)
    .transpose()?;
    let audio_token_alignment = sqlx::query_as::<_, AudioTokenAlignmentRecord>(
        r#"
        SELECT run_id, status, audio_sha256, audio_duration_ms,
               model_name, model_sha256, program_sha256,
               parameters_sha256, token_track_sha256, backend,
               global_match_coverage, token_aligned_segment_count,
               fallback_raw_segment_count, fallback_reason, created_at
          FROM moss_audio_token_runs
         WHERE run_id = ?
        "#,
    )
    .bind(&run.run_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|_| MossApiError::operation_failed())?
    .map(moss_audio_token_alignment_view)
    .transpose()?;
    let revision = candidate_revision(connection, &run.run_id).await?;
    let sha256 = sha256_serialized(&segments)?;
    let correction_rows = sqlx::query_as::<_, CorrectionRow>(
        r#"
        SELECT c.correction_id, c.segment_id, s.segment_index, c.original_text,
               c.replacement_text, c.result_text, c.start_char, c.end_char,
               c.rule_id, c.reverted_at,
               m.term_id AS machine_term_id,
               m.context_sha256 AS machine_context_sha256,
               m.token_track_sha256 AS machine_token_track_sha256,
               m.model_sha256 AS machine_model_sha256,
               m.first_token_index AS machine_first_token_index,
               m.last_token_index AS machine_last_token_index,
               m.confidence AS machine_confidence
          FROM moss_term_corrections c
          JOIN moss_candidate_segments s ON s.segment_id = c.segment_id
          LEFT JOIN moss_machine_term_correction_source m
                 ON m.correction_id = c.correction_id
         WHERE s.run_id = ?
         ORDER BY c.created_at, c.revision, c.correction_id
        "#,
    )
    .bind(&run.run_id)
    .fetch_all(&mut *connection)
    .await
    .map_err(|_| MossApiError::operation_failed())?;
    let corrections = correction_rows
        .into_iter()
        .map(|row| {
            let original_text = correction_before_text(&row);
            let machine_source = match row.machine_term_id {
                Some(term_id) => Some(MossMachineCorrectionSource {
                    term_id,
                    context_sha256: row
                        .machine_context_sha256
                        .ok_or_else(MossApiError::operation_failed)?,
                    token_track_sha256: row
                        .machine_token_track_sha256
                        .ok_or_else(MossApiError::operation_failed)?,
                    model_sha256: row
                        .machine_model_sha256
                        .ok_or_else(MossApiError::operation_failed)?,
                    first_token_index: u32::try_from(
                        row.machine_first_token_index
                            .ok_or_else(MossApiError::operation_failed)?,
                    )
                    .map_err(|_| MossApiError::operation_failed())?,
                    last_token_index: u32::try_from(
                        row.machine_last_token_index
                            .ok_or_else(MossApiError::operation_failed)?,
                    )
                    .map_err(|_| MossApiError::operation_failed())?,
                    confidence: row
                        .machine_confidence
                        .ok_or_else(MossApiError::operation_failed)?,
                }),
                None => None,
            };
            Ok::<_, MossApiError>(MossTermCorrection {
                correction_id: row.correction_id,
                segment_id: row.segment_id,
                original_text,
                corrected_text: row.result_text,
                matched_alias: row.original_text,
                canonical: row.replacement_text,
                rule_id: row.rule_id,
                context_revision,
                state: if row.reverted_at.is_some() {
                    "reverted".to_owned()
                } else {
                    "applied".to_owned()
                },
                source_layer: if machine_source.is_some() {
                    "audio_token_context".to_owned()
                } else {
                    "context_alias".to_owned()
                },
                machine_source,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let anonymous_speakers = labels.iter().cloned().collect::<Vec<_>>();
    let bindings = labels
        .into_iter()
        .map(|speaker_label| MossSpeakerBinding {
            person_id: binding_people.remove(&speaker_label).flatten(),
            speaker_label,
        })
        .collect();
    let referenced_people = referenced_people
        .into_iter()
        .map(|(person_id, display_name)| BoundPerson {
            person_id,
            display_name,
        })
        .collect();
    Ok(CandidateData {
        revision,
        sha256,
        segments,
        anonymous_speakers,
        bindings,
        corrections,
        referenced_people,
        alignments,
        diagnostics,
        audio_token_alignment,
    })
}

fn moss_audio_token_alignment_view(
    value: AudioTokenAlignmentRecord,
) -> Result<MossAudioTokenAlignment, MossApiError> {
    Ok(MossAudioTokenAlignment {
        status: value.status,
        audio_sha256: value.audio_sha256,
        audio_duration_ms: value
            .audio_duration_ms
            .map(u64::try_from)
            .transpose()
            .map_err(|_| MossApiError::operation_failed())?,
        model_name: value.model_name,
        model_sha256: value.model_sha256,
        program_sha256: value.program_sha256,
        parameters_sha256: value.parameters_sha256,
        token_track_sha256: value.token_track_sha256,
        backend: value.backend,
        global_match_coverage: value.global_match_coverage,
        token_aligned_segment_count: u32::try_from(value.token_aligned_segment_count)
            .map_err(|_| MossApiError::operation_failed())?,
        fallback_raw_segment_count: u32::try_from(value.fallback_raw_segment_count)
            .map_err(|_| MossApiError::operation_failed())?,
        fallback_reason: value.fallback_reason,
    })
}

fn moss_run_diagnostics_view(
    value: MossRunDiagnosticsRecord,
) -> Result<MossRunDiagnostics, MossApiError> {
    Ok(MossRunDiagnostics {
        audio_duration_ms: u64::try_from(value.audio_duration_ms)
            .map_err(|_| MossApiError::operation_failed())?,
        activity_frame_ms: u32::try_from(value.activity_frame_ms)
            .map_err(|_| MossApiError::operation_failed())?,
        activity_threshold_dbfs: value.activity_threshold_dbfs,
        first_active_ms: value
            .first_active_ms
            .map(u64::try_from)
            .transpose()
            .map_err(|_| MossApiError::operation_failed())?,
        last_active_ms: value
            .last_active_ms
            .map(u64::try_from)
            .transpose()
            .map_err(|_| MossApiError::operation_failed())?,
        model_last_timestamp_ms: u64::try_from(value.model_last_timestamp_ms)
            .map_err(|_| MossApiError::operation_failed())?,
        tail_delta_ms: value.tail_delta_ms,
        aligned_segment_count: u32::try_from(value.aligned_segment_count)
            .map_err(|_| MossApiError::operation_failed())?,
        fallback_segment_count: u32::try_from(value.fallback_segment_count)
            .map_err(|_| MossApiError::operation_failed())?,
        source_anchor_count: u32::try_from(value.source_anchor_count)
            .map_err(|_| MossApiError::operation_failed())?,
        source_hash_verified: value.source_hash_verified,
        source_expected_sha256: value.source_expected_sha256,
        source_actual_sha256: value.source_actual_sha256,
        fallback_reason: value.fallback_reason,
    })
}

async fn load_current_transcript(
    connection: &mut SqliteConnection,
    meeting_id: &str,
    active: Option<&ActivationRow>,
) -> Result<MossTranscriptVersion, MossApiError> {
    let rows = sqlx::query_as::<_, CurrentTranscriptRow>(
        r#"
        SELECT id, meeting_id, transcript, timestamp, summary, action_items,
               key_points, audio_start_time, audio_end_time, duration, speaker
          FROM transcripts
         WHERE meeting_id = ?
         ORDER BY CASE WHEN audio_start_time IS NULL THEN 1 ELSE 0 END,
                  audio_start_time, timestamp, id
        "#,
    )
    .bind(meeting_id)
    .fetch_all(&mut *connection)
    .await
    .map_err(|_| MossApiError::operation_failed())?;
    let sha256 = sha256_serialized(&rows)?;
    let activation_segments = if let Some(active) = active {
        sqlx::query_as::<_, ActivationSegmentRow>(
            r#"
            SELECT transcript_id, speaker_label, resolved_person_id,
                   override_id, binding_id
              FROM moss_activation_segments
             WHERE activation_id = ?
            "#,
        )
        .bind(&active.activation_id)
        .fetch_all(&mut *connection)
        .await
        .map_err(|_| MossApiError::operation_failed())?
        .into_iter()
        .map(|row| (row.transcript_id.clone(), row))
        .collect::<HashMap<_, _>>()
    } else {
        HashMap::new()
    };
    let mut segments = Vec::with_capacity(rows.len());
    for row in rows {
        let activation_segment = activation_segments.get(&row.id);
        let start_ms = millis(row.audio_start_time);
        let fallback_end = start_ms.saturating_add(millis(row.duration));
        let end_ms = millis(row.audio_end_time).max(fallback_end).max(start_ms);
        let resolved_person_id =
            activation_segment.and_then(|segment| segment.resolved_person_id.clone());
        let speaker_resolution = match activation_segment {
            Some(segment) if segment.override_id.is_some() && resolved_person_id.is_some() => {
                "segment_override"
            }
            Some(segment) if segment.binding_id.is_some() && resolved_person_id.is_some() => {
                "bulk_binding"
            }
            _ => "anonymous",
        };
        segments.push(MossTranscriptSegment {
            segment_id: row.id,
            start_ms,
            end_ms,
            speaker_label: activation_segment.map(|segment| segment.speaker_label.clone()),
            text: row.transcript,
            resolved_person_id: resolved_person_id.clone(),
            segment_override_person_id: activation_segment
                .filter(|segment| segment.override_id.is_some())
                .and_then(|_| resolved_person_id),
            speaker_resolution: speaker_resolution.to_owned(),
            text_source_layer: "current_transcript".to_owned(),
        });
    }
    let revision: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM moss_activation_snapshots WHERE meeting_id = ?")
            .bind(meeting_id)
            .fetch_one(&mut *connection)
            .await
            .map_err(|_| MossApiError::operation_failed())?;
    Ok(MossTranscriptVersion {
        source: if active.is_some() && !segments.is_empty() {
            "moss".to_owned()
        } else {
            current_transcript_source_label(connection, meeting_id).await
        },
        revision: u64::try_from(revision).unwrap_or(0),
        sha256,
        segments,
    })
}

/// Labels the local engine that produced the current (non-MOSS) transcript
/// rows so the review workspace stops calling every transcript "whisper".
async fn current_transcript_source_label(
    connection: &mut SqliteConnection,
    meeting_id: &str,
) -> String {
    let folder_path: Option<String> =
        sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = ?")
            .bind(meeting_id)
            .fetch_optional(&mut *connection)
            .await
            .ok()
            .flatten();
    if let Some(provider) = folder_path
        .as_deref()
        .and_then(read_metadata_transcription_provider)
    {
        return normalize_provider_label(Some(&provider));
    }

    let provider: Option<String> =
        sqlx::query_scalar("SELECT provider FROM transcript_settings WHERE id = '1' LIMIT 1")
            .fetch_optional(&mut *connection)
            .await
            .ok()
            .flatten();
    normalize_provider_label(provider.as_deref())
}

fn read_metadata_transcription_provider(folder_path: &str) -> Option<String> {
    let path = std::path::Path::new(folder_path).join("metadata.json");
    let raw = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value
        .get("transcription_provider")
        .and_then(|value| value.as_str())
        .map(str::to_owned)
}

fn normalize_provider_label(provider: Option<&str>) -> String {
    match provider.map(|value| value.trim().to_ascii_lowercase()) {
        Some(value) if value == "sensevoice" => "sensevoice".to_owned(),
        Some(value) if value == "parakeet" => "parakeet".to_owned(),
        _ => "whisper".to_owned(),
    }
}

fn run_summary(
    run: &MossRunRecord,
    candidate: Option<&CandidateData>,
    can_cancel: bool,
    cancel_requested: bool,
) -> MossRunSummary {
    let state = match run.status.as_str() {
        "running" if cancel_requested => "cancel_requested",
        "running" => "running",
        "completed" => "completed",
        "failed" => "failed",
        "cancelled" => "cancelled",
        _ => "failed",
    };
    MossRunSummary {
        run_id: run.run_id.clone(),
        meeting_id: run.meeting_id.clone(),
        state: state.to_owned(),
        progress: (run.status == "completed").then(|| MossRunProgress {
            stage: "complete".to_owned(),
            percentage: 100,
        }),
        created_at: run.created_at.clone(),
        updated_at: run.updated_at.clone(),
        completed_at: run.completed_at.clone(),
        error_code: run.error_code.clone(),
        can_cancel: run.status == "running" && can_cancel && !cancel_requested,
        candidate_revision: candidate.map(|candidate| candidate.revision),
        candidate_sha256: candidate.map(|candidate| candidate.sha256.clone()),
        language_requested: run.language_requested.clone(),
        language_resolved: run.language_resolved.clone(),
        decode_parameters_json: run.decode_parameters_json.clone(),
        decode_parameters_sha256: run.decode_parameters_sha256.clone(),
    }
}

async fn build_workspace(
    pool: &SqlitePool,
    review_state: &MossReviewState,
    meeting_id: &str,
    selected_run_id: Option<&str>,
    system: MossSystemStatus,
) -> Result<MossWorkspace, MossApiError> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| MossApiError::operation_failed())?;
    let connection = &mut *transaction;
    let folder_path =
        sqlx::query_as::<_, (Option<String>,)>("SELECT folder_path FROM meetings WHERE id = ?")
            .bind(meeting_id)
            .fetch_optional(&mut *connection)
            .await
            .map_err(|_| MossApiError::operation_failed())?
            .ok_or_else(MossApiError::operation_failed)?
            .0;
    let runs = sqlx::query_as::<_, MossRunRecord>(
        r#"
        SELECT run_id, meeting_id, status, created_at, updated_at, started_at,
               completed_at, source_transcript_sha256, audio_sha256,
               model_sha256, runtime_sha256, context_sha256, backend_name,
               backend_version, runtime_version, model_revision, device_name,
               raw_output_sha256, clean_output_sha256, candidate_sha256,
               segment_count, wall_elapsed_ms, wall_rtf, peak_memory_bytes,
               error_code, language_requested, language_resolved,
               decode_parameters_json, decode_parameters_sha256
          FROM moss_transcription_runs
         WHERE meeting_id = ?
         ORDER BY created_at DESC, run_id DESC
        "#,
    )
    .bind(meeting_id)
    .fetch_all(&mut *connection)
    .await
    .map_err(|_| MossApiError::operation_failed())?;
    let selected_run_id = match selected_run_id {
        Some(run_id) => {
            if !runs.iter().any(|run| run.run_id == run_id) {
                return Err(MossApiError::new("MOSS_RUN_NOT_FOUND", false));
            }
            Some(run_id.to_owned())
        }
        None => runs.first().map(|run| run.run_id.clone()),
    };
    let context_container = folder_path
        .as_deref()
        .filter(|path| !path.trim().is_empty())
        .map(Path::new)
        .map(load_context_container)
        .transpose()?
        .flatten();
    let mut candidate_by_run = HashMap::<String, CandidateData>::new();
    for run in runs.iter().filter(|run| run.status == "completed") {
        let context_revision = context_for_hash(context_container.as_ref(), &run.context_sha256)
            .map(|context| context.revision)
            .unwrap_or(0);
        let candidate = load_candidate_data(connection, run, context_revision).await?;
        candidate_by_run.insert(run.run_id.clone(), candidate);
    }
    let run_summaries = runs
        .iter()
        .map(|run| {
            run_summary(
                run,
                candidate_by_run.get(&run.run_id),
                review_state.request_for_run(&run.run_id).is_some(),
                review_state.is_cancel_requested(&run.run_id),
            )
        })
        .collect::<Vec<_>>();
    let review = if let Some(run_id) = selected_run_id.as_deref() {
        let run = runs.iter().find(|run| run.run_id == run_id).unwrap();
        if run.status != "completed" {
            None
        } else {
            let candidate_data = candidate_by_run
                .remove(run_id)
                .ok_or_else(MossApiError::operation_failed)?;
            let active = sqlx::query_as::<_, ActivationRow>(
                r#"
                SELECT activation_id, run_id, activated_transcript_sha256
                  FROM moss_activation_snapshots
                 WHERE meeting_id = ? AND status = 'active'
                "#,
            )
            .bind(meeting_id)
            .fetch_optional(&mut *connection)
            .await
            .map_err(|_| MossApiError::operation_failed())?;
            let current = load_current_transcript(connection, meeting_id, active.as_ref()).await?;
            let context = context_for_hash(context_container.as_ref(), &run.context_sha256);
            let mut participants = context
                .as_ref()
                .map(|context| {
                    context
                        .people
                        .iter()
                        .filter_map(participant_from_snapshot)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let known_people = participants
                .iter()
                .map(|participant| participant.person_id.clone())
                .collect::<BTreeSet<_>>();
            participants.extend(
                candidate_data
                    .referenced_people
                    .iter()
                    .filter(|person| !known_people.contains(&person.person_id))
                    .map(|person| MossParticipant {
                        person_id: person.person_id.clone(),
                        display_name: person.display_name.clone(),
                        attendance: "guest".to_owned(),
                        department: None,
                        role: None,
                    }),
            );
            participants.sort_by(|left, right| left.person_id.cmp(&right.person_id));
            let is_active = active
                .as_ref()
                .is_some_and(|value| value.run_id == run.run_id);
            let current_changed = current.sha256 != run.source_transcript_sha256;
            let incomplete = candidate_data.segments.is_empty();
            let can_activate = !is_active && active.is_none() && !current_changed && !incomplete;
            let activate_blocker = if current_changed {
                Some("CURRENT_TRANSCRIPT_CHANGED".to_owned())
            } else if incomplete {
                Some("CANDIDATE_INCOMPLETE".to_owned())
            } else if is_active || active.is_some() {
                Some("CANDIDATE_STALE".to_owned())
            } else {
                None
            };
            let can_rollback = active.as_ref().is_some_and(|activation| {
                activation.run_id == run.run_id
                    && activation.activated_transcript_sha256 == current.sha256
            });
            let run_summary = run_summary(run, Some(&candidate_data), false, false);
            Some(MossCandidateReview {
                schema_version: SCHEMA_VERSION,
                meeting_id: meeting_id.to_owned(),
                run: run_summary,
                current: current.clone(),
                candidate: MossCandidateVersion {
                    source: "moss".to_owned(),
                    revision: candidate_data.revision,
                    sha256: candidate_data.sha256,
                    segments: candidate_data.segments,
                    run_id: run.run_id.clone(),
                    is_active,
                    alignments: candidate_data.alignments,
                    diagnostics: candidate_data.diagnostics,
                    audio_token_alignment: candidate_data.audio_token_alignment,
                },
                participants,
                anonymous_speakers: candidate_data.anonymous_speakers,
                bindings: candidate_data.bindings,
                corrections: candidate_data.corrections,
                activation: MossActivationState {
                    active_run_id: active.as_ref().map(|value| value.run_id.clone()),
                    active_activation_id: active.as_ref().map(|value| value.activation_id.clone()),
                    current_transcript_sha256: current.sha256,
                    can_activate,
                    activate_blocker,
                    can_rollback,
                },
            })
        }
    } else {
        None
    };
    transaction
        .commit()
        .await
        .map_err(|_| MossApiError::operation_failed())?;
    Ok(MossWorkspace {
        schema_version: SCHEMA_VERSION,
        meeting_id: meeting_id.to_owned(),
        system,
        runs: run_summaries,
        selected_run_id,
        review,
    })
}

async fn run_for_meeting(
    pool: &SqlitePool,
    meeting_id: &str,
    run_id: &str,
) -> Result<MossRunRecord, MossApiError> {
    let run = MossCandidateRepository::get_run(pool, run_id)
        .await
        .map_err(MossApiError::from)?;
    if run.meeting_id != meeting_id {
        return Err(MossApiError::new("MOSS_RUN_NOT_FOUND", false));
    }
    Ok(run)
}

async fn meeting_folder(pool: &SqlitePool, meeting_id: &str) -> Result<PathBuf, MossApiError> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT folder_path FROM meetings WHERE id = ?")
            .bind(meeting_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| MossApiError::operation_failed())?;
    let folder = row
        .and_then(|(folder,)| folder)
        .filter(|folder| !folder.trim().is_empty())
        .ok_or_else(MossApiError::operation_failed)?;
    let folder = PathBuf::from(folder);
    if !folder.is_absolute() || !folder.is_dir() {
        return Err(MossApiError::operation_failed());
    }
    Ok(folder)
}

fn find_audio_file(folder: &Path) -> Result<PathBuf, MossApiError> {
    const PREFERRED: &[&str] = &[
        "audio.mp4",
        "audio.m4a",
        "audio.wav",
        "audio.mp3",
        "audio.flac",
        "audio.ogg",
        "recording.mp4",
        "audio.mkv",
        "audio.webm",
        "audio.wma",
    ];
    for name in PREFERRED {
        let candidate = folder.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    let entries = std::fs::read_dir(folder).map_err(|_| MossApiError::operation_failed())?;
    for entry in entries.flatten() {
        let path = entry.path();
        let supported = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .is_some_and(|extension| crate::audio::AUDIO_EXTENSIONS.contains(&extension.as_str()));
        if path.is_file() && supported {
            return Ok(path);
        }
    }
    Err(MossApiError::operation_failed())
}

async fn run_context(
    pool: &SqlitePool,
    run: &MossRunRecord,
) -> Result<Option<MeetingContextSnapshot>, MossApiError> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT folder_path FROM meetings WHERE id = ?")
            .bind(&run.meeting_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| MossApiError::operation_failed())?;
    let Some(folder) = row.and_then(|(folder,)| folder) else {
        return Ok(None);
    };
    if folder.trim().is_empty() {
        return Ok(None);
    }
    let container = load_context_container(Path::new(&folder))?;
    Ok(context_for_hash(container.as_ref(), &run.context_sha256))
}

fn bound_person(
    context: Option<&MeetingContextSnapshot>,
    person_id: &str,
) -> Result<BoundPerson, MossApiError> {
    let person = context
        .and_then(|context| {
            context.people.iter().find(|person| {
                person.person_id == person_id && person.attendance != AttendanceStatus::Absent
            })
        })
        .ok_or_else(|| MossApiError::new("MOSS_INVALID_BINDING", false))?;
    Ok(BoundPerson {
        person_id: person.person_id.clone(),
        display_name: person.display_name.clone(),
    })
}

async fn ensure_candidate_editable(
    pool: &SqlitePool,
    run: &MossRunRecord,
) -> Result<(), MossApiError> {
    if run.status != "completed" {
        return Err(MossApiError::new("MOSS_CANDIDATE_NOT_READY", true));
    }
    let active: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM moss_activation_snapshots
             WHERE meeting_id = ? AND status = 'active'
        )
        "#,
    )
    .bind(&run.meeting_id)
    .fetch_one(pool)
    .await
    .map_err(|_| MossApiError::operation_failed())?;
    if active {
        return Err(MossApiError::new("MOSS_CANDIDATE_CONFLICT", false));
    }
    Ok(())
}

async fn current_candidate_revision(pool: &SqlitePool, run_id: &str) -> Result<u64, MossApiError> {
    let mut connection = pool
        .acquire()
        .await
        .map_err(|_| MossApiError::operation_failed())?;
    candidate_revision(&mut connection, run_id).await
}

async fn ensure_expected_revision(
    pool: &SqlitePool,
    run_id: &str,
    expected: u64,
) -> Result<(), MossApiError> {
    if current_candidate_revision(pool, run_id).await? != expected {
        return Err(MossApiError::new("MOSS_CANDIDATE_STALE", true));
    }
    Ok(())
}

async fn ensure_segment_belongs_to_run(
    pool: &SqlitePool,
    run_id: &str,
    segment_id: &str,
) -> Result<(), MossApiError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM moss_candidate_segments WHERE run_id = ? AND segment_id = ?)",
    )
    .bind(run_id)
    .bind(segment_id)
    .fetch_one(pool)
    .await
    .map_err(|_| MossApiError::operation_failed())?;
    if !exists {
        return Err(MossApiError::new("MOSS_CANDIDATE_NOT_READY", false));
    }
    Ok(())
}

async fn active_override(
    pool: &SqlitePool,
    segment_id: &str,
) -> Result<Option<ActiveOverrideRow>, MossApiError> {
    sqlx::query_as::<_, ActiveOverrideRow>(
        r#"
        SELECT override_id, replacement_text, person_id, person_display_name
          FROM moss_segment_overrides
         WHERE segment_id = ? AND revoked_at IS NULL
         ORDER BY revision DESC LIMIT 1
        "#,
    )
    .bind(segment_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| MossApiError::operation_failed())
}

fn alias_rule_id(namespace: &str, source_id: &str, alias: &str) -> String {
    let hash = sha256_bytes(format!("{namespace}\0{source_id}\0{alias}").as_bytes());
    format!("{namespace}_ALIAS_{}", hash[..16].to_ascii_uppercase())
}

fn ascii_word(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

fn find_alias_range(text: &str, alias: &str, from: usize) -> Option<(usize, usize, String)> {
    let text_chars = text.chars().collect::<Vec<_>>();
    let alias_chars = alias.chars().collect::<Vec<_>>();
    if alias_chars.is_empty() || alias_chars.len() > text_chars.len() || from >= text_chars.len() {
        return None;
    }
    for start in from..=text_chars.len().saturating_sub(alias_chars.len()) {
        let end = start + alias_chars.len();
        let equal = if alias.is_ascii() {
            text_chars[start..end]
                .iter()
                .collect::<String>()
                .eq_ignore_ascii_case(alias)
        } else {
            text_chars[start..end] == alias_chars
        };
        if !equal {
            continue;
        }
        if alias.is_ascii()
            && ((start > 0 && ascii_word(text_chars[start - 1]))
                || (end < text_chars.len() && ascii_word(text_chars[end])))
        {
            continue;
        }
        return Some((
            start,
            end,
            text_chars[start..end].iter().collect::<String>(),
        ));
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExplicitAliasMatch {
    start: usize,
    end: usize,
    matched: String,
    namespace: String,
    source_id: String,
    alias: String,
    canonical: String,
}

fn select_explicit_alias_matches(
    raw_text: &str,
    aliases: &[(String, String, String, String)],
) -> Vec<ExplicitAliasMatch> {
    let mut candidates = Vec::new();
    for (namespace, source_id, alias, canonical) in aliases {
        let mut cursor = 0_usize;
        while let Some((start, end, matched)) = find_alias_range(raw_text, alias, cursor) {
            candidates.push(ExplicitAliasMatch {
                start,
                end,
                matched,
                namespace: namespace.clone(),
                source_id: source_id.clone(),
                alias: alias.clone(),
                canonical: canonical.clone(),
            });
            cursor = start.saturating_add(1);
        }
    }
    candidates.sort_by(|left, right| {
        (right.end - right.start)
            .cmp(&(left.end - left.start))
            .then_with(|| left.start.cmp(&right.start))
            .then_with(|| left.alias.cmp(&right.alias))
            .then_with(|| left.namespace.cmp(&right.namespace))
            .then_with(|| left.source_id.cmp(&right.source_id))
            .then_with(|| left.canonical.cmp(&right.canonical))
    });
    let mut selected = Vec::<ExplicitAliasMatch>::new();
    for candidate in candidates {
        let overlaps = selected
            .iter()
            .any(|existing| candidate.start < existing.end && existing.start < candidate.end);
        if !overlaps {
            selected.push(candidate);
        }
    }
    // P3 applies each correction to the result of the previous correction. Applying
    // immutable-source matches from right to left keeps every remaining raw offset valid.
    selected.sort_by(|left, right| {
        right
            .start
            .cmp(&left.start)
            .then_with(|| right.end.cmp(&left.end))
            .then_with(|| left.alias.cmp(&right.alias))
            .then_with(|| left.namespace.cmp(&right.namespace))
            .then_with(|| left.source_id.cmp(&right.source_id))
    });
    selected
}

async fn apply_explicit_term_corrections(
    pool: &SqlitePool,
    run: &MossRunRecord,
    context: Option<&MeetingContextSnapshot>,
) -> Result<(), MossApiError> {
    apply_context_term_corrections(pool, run, context, &[]).await
}

#[derive(Debug, Clone)]
enum PendingContextCorrection {
    Explicit(ExplicitAliasMatch),
    Machine(MachineTermSuggestion),
}

impl PendingContextCorrection {
    fn range(&self) -> (usize, usize) {
        match self {
            Self::Explicit(value) => (value.start, value.end),
            Self::Machine(value) => (value.start_char, value.end_char),
        }
    }
}

async fn apply_context_term_corrections(
    pool: &SqlitePool,
    run: &MossRunRecord,
    context: Option<&MeetingContextSnapshot>,
    machine_suggestions: &[MachineTermSuggestion],
) -> Result<(), MossApiError> {
    let Some(context) = context else {
        return Ok(());
    };
    let mut aliases = context
        .terms
        .iter()
        .flat_map(|term| {
            term.aliases.iter().map(move |alias| {
                (
                    "TERM".to_owned(),
                    term.term_id.clone(),
                    alias.clone(),
                    term.canonical.clone(),
                )
            })
        })
        .chain(context.people.iter().flat_map(|person| {
            person.aliases.iter().map(move |alias| {
                (
                    "PERSON".to_owned(),
                    person.person_id.clone(),
                    alias.clone(),
                    person.display_name.clone(),
                )
            })
        }))
        .filter(|(_, _, alias, canonical)| !alias.is_empty() && alias != canonical)
        .collect::<Vec<_>>();
    let mut canonical_keys_by_alias = BTreeMap::<String, BTreeSet<String>>::new();
    for (_, _, alias, canonical) in &aliases {
        canonical_keys_by_alias
            .entry(comparison_key(alias))
            .or_default()
            .insert(comparison_key(canonical));
    }
    aliases.retain(|(_, _, alias, _)| {
        canonical_keys_by_alias
            .get(&comparison_key(alias))
            .is_some_and(|canonical_keys| canonical_keys.len() == 1)
    });
    aliases.sort_by(|left, right| {
        right
            .2
            .chars()
            .count()
            .cmp(&left.2.chars().count())
            .then_with(|| left.2.cmp(&right.2))
            .then_with(|| left.0.cmp(&right.0))
            .then_with(|| left.1.cmp(&right.1))
    });
    let segments: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT segment_id, segment_index, raw_text FROM moss_candidate_segments WHERE run_id = ? ORDER BY segment_index",
    )
    .bind(&run.run_id)
    .fetch_all(pool)
    .await
    .map_err(|_| MossApiError::operation_failed())?;
    for (segment_id, segment_index, raw_text) in segments {
        let mut pending = select_explicit_alias_matches(&raw_text, &aliases)
            .into_iter()
            .map(PendingContextCorrection::Explicit)
            .collect::<Vec<_>>();
        let raw_chars = raw_text.chars().collect::<Vec<_>>();
        for suggestion in machine_suggestions.iter().filter(|suggestion| {
            i64::from(suggestion.segment_index) == segment_index
                && suggestion.context_sha256 == run.context_sha256
        }) {
            if suggestion.start_char >= suggestion.end_char
                || suggestion.end_char > raw_chars.len()
                || raw_chars[suggestion.start_char..suggestion.end_char]
                    .iter()
                    .collect::<String>()
                    != suggestion.original_text
            {
                continue;
            }
            let overlaps = pending.iter().any(|existing| {
                let (start, end) = existing.range();
                suggestion.start_char < end && start < suggestion.end_char
            });
            if !overlaps {
                pending.push(PendingContextCorrection::Machine(suggestion.clone()));
            }
        }
        pending.sort_by(|left, right| {
            let (left_start, left_end) = left.range();
            let (right_start, right_end) = right.range();
            right_start
                .cmp(&left_start)
                .then_with(|| right_end.cmp(&left_end))
        });
        for correction in pending {
            match correction {
                PendingContextCorrection::Explicit(selected) => {
                    MossCandidateRepository::add_term_correction(
                        pool,
                        &segment_id,
                        TermCorrectionInput {
                            start_char: selected.start,
                            end_char: selected.end,
                            original_text: selected.matched,
                            replacement_text: selected.canonical,
                            rule_id: alias_rule_id(
                                &selected.namespace,
                                &selected.source_id,
                                &selected.alias,
                            ),
                            expected_context_sha256: run.context_sha256.clone(),
                        },
                    )
                    .await
                    .map_err(MossApiError::from)?;
                }
                PendingContextCorrection::Machine(source) => {
                    MossCandidateRepository::add_machine_term_correction(pool, &segment_id, source)
                        .await
                        .map_err(MossApiError::from)?;
                }
            }
        }
    }
    Ok(())
}

async fn workspace_after_write(
    pool: &SqlitePool,
    review_state: &MossReviewState,
    meeting_id: &str,
    run_id: Option<&str>,
    system: MossSystemStatus,
) -> Result<MossWorkspace, MossApiError> {
    build_workspace(pool, review_state, meeting_id, run_id, system).await
}

async fn start_run_record(
    pool: &SqlitePool,
    input: NewMossRun,
) -> Result<MossRunRecord, MossApiError> {
    MossCandidateRepository::start_run(pool, input)
        .await
        .map_err(MossApiError::from)
}

#[tauri::command]
pub async fn api_moss_get_system_status(
    layout: State<'_, StorageLayoutState>,
    helper: State<'_, Arc<MossHelperManager>>,
    review_state: State<'_, MossReviewState>,
) -> Result<MossSystemStatus, MossApiError> {
    Ok(system_status(&layout, helper.inner().clone(), &review_state).await)
}

#[tauri::command]
pub async fn api_moss_get_workspace(
    app_state: State<'_, AppState>,
    layout: State<'_, StorageLayoutState>,
    helper: State<'_, Arc<MossHelperManager>>,
    review_state: State<'_, MossReviewState>,
    request: MossWorkspaceRequest,
) -> Result<MossWorkspace, MossApiError> {
    let status = system_status(&layout, helper.inner().clone(), &review_state).await;
    let _guard = review_state.operation_lock.lock().await;
    build_workspace(
        app_state.db_manager.pool(),
        &review_state,
        &request.meeting_id,
        request.selected_run_id.as_deref(),
        status,
    )
    .await
}

#[tauri::command]
pub async fn api_moss_start_run(
    app_state: State<'_, AppState>,
    layout: State<'_, StorageLayoutState>,
    helper: State<'_, Arc<MossHelperManager>>,
    review_state: State<'_, MossReviewState>,
    request: StartMossRunRequest,
) -> Result<MossWorkspace, MossApiError> {
    let moss_operation_guard = begin_storage_operation(StorageOperationKind::MossEnhancement)
        .map_err(|_| MossApiError::new("MOSS_QWEN_BUSY", true))?;
    let status = system_status(&layout, helper.inner().clone(), &review_state).await;
    if status.availability != "ready" {
        return Err(if status.installed {
            MossApiError::new("MOSS_UNHEALTHY", false)
        } else {
            MossApiError::new("MOSS_NOT_INSTALLED", false)
        });
    }
    let _guard = review_state.operation_lock.lock().await;
    let runtime_directory = layout.layout().paths().moss_runtime.clone();
    let model_path = layout.layout().paths().moss_models.join(MODEL_FILE_NAME);
    let whisper_models_directory = layout.layout().paths().whisper_models.clone();
    let helper_manager = helper.inner().clone();
    let installation = tauri::async_runtime::spawn_blocking({
        let helper_manager = helper_manager.clone();
        move || installation_from_paths(&helper_manager, runtime_directory, model_path)
    })
    .await
    .map_err(|_| MossApiError::operation_failed())??;
    let folder = meeting_folder(app_state.db_manager.pool(), &request.meeting_id).await?;
    let audio_path = find_audio_file(&folder)?;
    let context_container = load_context_container(&folder)?;
    let context = context_container
        .as_ref()
        .and_then(MeetingContextContainer::current_context)
        .cloned();
    let context_sha256 = context
        .as_ref()
        .map(|context| context.context_sha256.clone())
        .unwrap_or_else(|| sha256_bytes(EMPTY_CONTEXT_LABEL));
    let audio_hash_path = audio_path.clone();
    let audio_sha256 = tauri::async_runtime::spawn_blocking(move || sha256_file(&audio_hash_path))
        .await
        .map_err(|_| MossApiError::operation_failed())??;
    let helper_request_id = Uuid::new_v4().to_string();
    let preparation = helper_manager
        .begin_preparation(&helper_request_id)
        .map_err(MossCommandError::from)
        .map_err(|error| {
            if error.code == "MOSS_BUSY" || error.code == "MOSS_DUPLICATE_REQUEST" {
                MossApiError::new("MOSS_RUN_ALREADY_ACTIVE", true)
            } else {
                MossApiError::new("MOSS_OPERATION_FAILED", true)
            }
        })?;
    let run = start_run_record(
        app_state.db_manager.pool(),
        NewMossRun {
            meeting_id: request.meeting_id.clone(),
            audio_sha256,
            model_sha256: installation.model_sha256.clone(),
            runtime_sha256: installation.runtime_sha256.clone(),
            context_sha256: context_sha256.clone(),
            backend_name: "moss-transcribe-diarize".to_owned(),
            backend_version: moss_helper::native::EXPECTED_RUNTIME_VERSION.to_owned(),
            runtime_version: moss_helper::native::EXPECTED_SOURCE_COMMIT.to_owned(),
            model_revision: MODEL_FILE_NAME.trim_end_matches(".gguf").to_owned(),
            language_requested: moss_helper::native::MOSS_LANGUAGE_REQUESTED.to_owned(),
            decode_parameters_json: moss_helper::native::MOSS_DECODE_PARAMETERS_JSON.to_owned(),
            decode_parameters_sha256: moss_helper::native::moss_decode_parameters_sha256(),
        },
    )
    .await?;
    review_state.remember_request(run.run_id.clone(), helper_request_id.clone());
    let alignment_audio_path = audio_path.clone();
    let pool = app_state.db_manager.pool().clone();
    let worker_state = review_state.inner().clone();
    let worker_run = run.clone();
    tauri::async_runtime::spawn(async move {
        let _moss_operation_guard = moss_operation_guard;
        let result = transcribe_file_with_preparation(
            helper_manager,
            MossTranscribeFileRequest {
                request_id: helper_request_id,
                context_sha256,
                runtime_directory: installation.runtime_directory,
                model_path: installation.model_path,
                model_bytes: installation.model_bytes,
                model_sha256: installation.model_sha256,
                device_id: None,
                source_audio_path: audio_path,
                timeout_ms: TRANSCRIPTION_TIMEOUT_MS,
                language_requested: moss_helper::native::MOSS_LANGUAGE_REQUESTED.to_owned(),
                decode_parameters_json: moss_helper::native::MOSS_DECODE_PARAMETERS_JSON.to_owned(),
                decode_parameters_sha256: moss_helper::native::moss_decode_parameters_sha256(),
            },
            preparation,
        )
        .await;
        let _guard = worker_state.operation_lock.lock().await;
        match result {
            Ok(transcription) => {
                let alignment_model_path =
                    crate::moss_audio_token_alignment::model_path(&whisper_models_directory);
                let (token_track, token_failure_reason) = if !alignment_model_path.is_file() {
                    (None, Some("AUDIO_TOKEN_MODEL_MISSING".to_owned()))
                } else {
                    match build_audio_token_track(
                        &alignment_audio_path,
                        &whisper_models_directory,
                        R5_ALIGNMENT_MODEL_NAME,
                        "zh",
                    )
                    .await
                    {
                        Ok(track) if track.audio_sha256 == worker_run.audio_sha256 => {
                            (Some(track), None)
                        }
                        Ok(_) => (None, Some("AUDIO_TOKEN_AUDIO_HASH_MISMATCH".to_owned())),
                        Err(error) => {
                            log::warn!("R5 audio-token alignment fell back safely: {error:#}");
                            (None, Some("AUDIO_TOKEN_TRACK_FAILED".to_owned()))
                        }
                    }
                };
                match complete_product_aligned_candidate(
                    &pool,
                    &worker_run,
                    &transcription,
                    token_track.as_ref(),
                    token_failure_reason.as_deref(),
                    context.as_ref(),
                )
                .await
                {
                    Ok(completed) => {
                        if let Err(error) = apply_context_term_corrections(
                            &pool,
                            &completed.run,
                            context.as_ref(),
                            &completed.machine_suggestions,
                        )
                        .await
                        {
                            log::warn!(
                                "MOSS candidate was stored but explicit term correction failed: {}",
                                error.code
                            );
                        }
                    }
                    Err(error) => {
                        log::error!("Failed to store completed MOSS candidate: {error}");
                        let _ = MossCandidateRepository::mark_run_failed(
                            &pool,
                            &worker_run.run_id,
                            "MOSS_CANDIDATE_STORE_FAILED",
                        )
                        .await;
                    }
                }
            }
            Err(error) if error.code == "MOSS_CANCELLED" => {
                let _ =
                    MossCandidateRepository::mark_run_cancelled(&pool, &worker_run.run_id).await;
            }
            Err(error) => {
                let _ =
                    MossCandidateRepository::mark_run_failed(&pool, &worker_run.run_id, error.code)
                        .await;
            }
        }
        worker_state.forget_request(&worker_run.run_id);
    });
    workspace_after_write(
        app_state.db_manager.pool(),
        &review_state,
        &request.meeting_id,
        Some(&run.run_id),
        status,
    )
    .await
}

async fn cancel_run_impl<F>(
    pool: &SqlitePool,
    review_state: &MossReviewState,
    request: MossRunRequest,
    status: MossSystemStatus,
    cancel_helper: F,
) -> Result<MossWorkspace, MossApiError>
where
    F: FnOnce(&str) -> Result<bool, MossApiError>,
{
    let _guard = review_state.operation_lock.lock().await;
    let run = run_for_meeting(pool, &request.meeting_id, &request.run_id).await?;
    if run.status == "running" {
        let helper_request_id = review_state
            .request_for_run(&run.run_id)
            .ok_or_else(|| MossApiError::new("MOSS_CANCEL_FAILED", true))?;
        review_state.remember_cancellation_request(run.run_id.clone());
        match cancel_helper(&helper_request_id) {
            Ok(true) => {}
            Ok(false) => {
                review_state.forget_cancellation_request(&run.run_id);
                return Err(MossApiError::new("MOSS_CANCEL_FAILED", true));
            }
            Err(error) => {
                review_state.forget_cancellation_request(&run.run_id);
                return Err(error);
            }
        }
    }
    workspace_after_write(
        pool,
        review_state,
        &request.meeting_id,
        Some(&request.run_id),
        status,
    )
    .await
}

#[tauri::command]
pub async fn api_moss_cancel_run(
    app_state: State<'_, AppState>,
    layout: State<'_, StorageLayoutState>,
    helper: State<'_, Arc<MossHelperManager>>,
    review_state: State<'_, MossReviewState>,
    request: MossRunRequest,
) -> Result<MossWorkspace, MossApiError> {
    let status = system_status(&layout, helper.inner().clone(), &review_state).await;
    cancel_run_impl(
        app_state.db_manager.pool(),
        &review_state,
        request,
        status,
        |helper_request_id| {
            helper
                .cancel(helper_request_id)
                .map_err(|_| MossApiError::new("MOSS_CANCEL_FAILED", true))
        },
    )
    .await
}

async fn save_speaker_binding_impl(
    pool: &SqlitePool,
    review_state: &MossReviewState,
    request: SaveMossSpeakerBindingRequest,
    status: MossSystemStatus,
) -> Result<MossWorkspace, MossApiError> {
    let _guard = review_state.operation_lock.lock().await;
    let run = run_for_meeting(pool, &request.meeting_id, &request.run_id).await?;
    ensure_candidate_editable(pool, &run).await?;
    ensure_expected_revision(pool, &run.run_id, request.expected_candidate_revision).await?;
    let label_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM moss_candidate_segments WHERE run_id = ? AND speaker_label = ?)",
    )
    .bind(&run.run_id)
    .bind(&request.speaker_label)
    .fetch_one(pool)
    .await
    .map_err(|_| MossApiError::operation_failed())?;
    if !label_exists {
        return Err(MossApiError::new("MOSS_INVALID_BINDING", false));
    }
    if let Some(person_id) = request.person_id.as_deref() {
        let context = run_context(pool, &run).await?;
        let person = bound_person(context.as_ref(), person_id)?;
        MossCandidateRepository::bind_speaker(
            pool,
            &run.run_id,
            &request.speaker_label,
            person,
            &run.context_sha256,
        )
        .await
        .map_err(MossApiError::from)?;
    } else {
        sqlx::query(
            r#"
            UPDATE moss_speaker_bindings SET revoked_at = ?
             WHERE run_id = ? AND speaker_label = ? AND revoked_at IS NULL
            "#,
        )
        .bind(now_timestamp())
        .bind(&run.run_id)
        .bind(&request.speaker_label)
        .execute(pool)
        .await
        .map_err(|_| MossApiError::operation_failed())?;
    }
    workspace_after_write(
        pool,
        &review_state,
        &request.meeting_id,
        Some(&run.run_id),
        status,
    )
    .await
}

#[tauri::command]
pub async fn api_moss_save_speaker_binding(
    app_state: State<'_, AppState>,
    layout: State<'_, StorageLayoutState>,
    helper: State<'_, Arc<MossHelperManager>>,
    review_state: State<'_, MossReviewState>,
    request: SaveMossSpeakerBindingRequest,
) -> Result<MossWorkspace, MossApiError> {
    let status = system_status(&layout, helper.inner().clone(), &review_state).await;
    save_speaker_binding_impl(app_state.db_manager.pool(), &review_state, request, status).await
}

async fn save_segment_override_impl(
    pool: &SqlitePool,
    review_state: &MossReviewState,
    request: SaveMossSegmentOverrideRequest,
    status: MossSystemStatus,
) -> Result<MossWorkspace, MossApiError> {
    let _guard = review_state.operation_lock.lock().await;
    let run = run_for_meeting(pool, &request.meeting_id, &request.run_id).await?;
    ensure_candidate_editable(pool, &run).await?;
    ensure_expected_revision(pool, &run.run_id, request.expected_candidate_revision).await?;
    ensure_segment_belongs_to_run(pool, &run.run_id, &request.segment_id).await?;
    let current = active_override(pool, &request.segment_id).await?;
    if let Some(person_id) = request.person_id.as_deref() {
        let context = run_context(pool, &run).await?;
        let person = bound_person(context.as_ref(), person_id)
            .map_err(|_| MossApiError::new("MOSS_INVALID_OVERRIDE", false))?;
        MossCandidateRepository::set_segment_override(
            pool,
            &request.segment_id,
            SegmentOverrideInput {
                replacement_text: current.and_then(|value| value.replacement_text),
                person: Some(person),
                reason_code: "MANUAL_PERSON_OVERRIDE".to_owned(),
                expected_context_sha256: run.context_sha256.clone(),
            },
        )
        .await
        .map_err(MossApiError::from)?;
    } else if let Some(current) = current {
        if let Some(replacement_text) = current.replacement_text {
            MossCandidateRepository::set_segment_override(
                pool,
                &request.segment_id,
                SegmentOverrideInput {
                    replacement_text: Some(replacement_text),
                    person: None,
                    reason_code: "MANUAL_PERSON_OVERRIDE".to_owned(),
                    expected_context_sha256: run.context_sha256.clone(),
                },
            )
            .await
            .map_err(MossApiError::from)?;
        } else {
            MossCandidateRepository::revoke_segment_override(pool, &current.override_id)
                .await
                .map_err(MossApiError::from)?;
        }
    }
    workspace_after_write(
        pool,
        &review_state,
        &request.meeting_id,
        Some(&run.run_id),
        status,
    )
    .await
}

#[tauri::command]
pub async fn api_moss_save_segment_override(
    app_state: State<'_, AppState>,
    layout: State<'_, StorageLayoutState>,
    helper: State<'_, Arc<MossHelperManager>>,
    review_state: State<'_, MossReviewState>,
    request: SaveMossSegmentOverrideRequest,
) -> Result<MossWorkspace, MossApiError> {
    let status = system_status(&layout, helper.inner().clone(), &review_state).await;
    save_segment_override_impl(app_state.db_manager.pool(), &review_state, request, status).await
}

async fn update_candidate_segment_impl(
    pool: &SqlitePool,
    review_state: &MossReviewState,
    request: UpdateMossCandidateSegmentRequest,
    status: MossSystemStatus,
) -> Result<MossWorkspace, MossApiError> {
    let _guard = review_state.operation_lock.lock().await;
    let run = run_for_meeting(pool, &request.meeting_id, &request.run_id).await?;
    ensure_candidate_editable(pool, &run).await?;
    ensure_expected_revision(pool, &run.run_id, request.expected_candidate_revision).await?;
    ensure_segment_belongs_to_run(pool, &run.run_id, &request.segment_id).await?;
    let text = request.text.trim().to_owned();
    if text.is_empty() {
        return Err(MossApiError::new("MOSS_INVALID_OVERRIDE", false));
    }
    let current = active_override(pool, &request.segment_id).await?;
    let person = current.as_ref().and_then(|value| {
        value
            .person_id
            .as_ref()
            .zip(value.person_display_name.as_ref())
            .map(|(person_id, display_name)| BoundPerson {
                person_id: person_id.clone(),
                display_name: display_name.clone(),
            })
    });
    MossCandidateRepository::set_segment_override(
        pool,
        &request.segment_id,
        SegmentOverrideInput {
            replacement_text: Some(text),
            person,
            reason_code: "MANUAL_TEXT_EDIT".to_owned(),
            expected_context_sha256: run.context_sha256.clone(),
        },
    )
    .await
    .map_err(MossApiError::from)?;
    workspace_after_write(
        pool,
        &review_state,
        &request.meeting_id,
        Some(&run.run_id),
        status,
    )
    .await
}

#[tauri::command]
pub async fn api_moss_update_candidate_segment(
    app_state: State<'_, AppState>,
    layout: State<'_, StorageLayoutState>,
    helper: State<'_, Arc<MossHelperManager>>,
    review_state: State<'_, MossReviewState>,
    request: UpdateMossCandidateSegmentRequest,
) -> Result<MossWorkspace, MossApiError> {
    let status = system_status(&layout, helper.inner().clone(), &review_state).await;
    update_candidate_segment_impl(app_state.db_manager.pool(), &review_state, request, status).await
}

async fn set_correction_state_impl(
    pool: &SqlitePool,
    review_state: &MossReviewState,
    request: SetMossCorrectionStateRequest,
    status: MossSystemStatus,
) -> Result<MossWorkspace, MossApiError> {
    let _guard = review_state.operation_lock.lock().await;
    let run = run_for_meeting(pool, &request.meeting_id, &request.run_id).await?;
    ensure_candidate_editable(pool, &run).await?;
    ensure_expected_revision(pool, &run.run_id, request.expected_candidate_revision).await?;
    let correction = sqlx::query_as::<_, CorrectionRow>(
        r#"
        SELECT c.correction_id, c.segment_id, s.segment_index, c.original_text,
               c.replacement_text, c.result_text, c.start_char, c.end_char,
               c.rule_id, c.reverted_at,
               m.term_id AS machine_term_id,
               m.context_sha256 AS machine_context_sha256,
               m.token_track_sha256 AS machine_token_track_sha256,
               m.model_sha256 AS machine_model_sha256,
               m.first_token_index AS machine_first_token_index,
               m.last_token_index AS machine_last_token_index,
               m.confidence AS machine_confidence
          FROM moss_term_corrections c
          JOIN moss_candidate_segments s ON s.segment_id = c.segment_id
          LEFT JOIN moss_machine_term_correction_source m
                 ON m.correction_id = c.correction_id
         WHERE c.correction_id = ? AND s.run_id = ?
        "#,
    )
    .bind(&request.correction_id)
    .bind(&run.run_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| MossApiError::operation_failed())?
    .ok_or_else(|| MossApiError::new("MOSS_CORRECTION_NOT_FOUND", false))?;
    let currently_applied = correction.reverted_at.is_none();
    if request.applied != currently_applied {
        if request.applied {
            let base_text: String = sqlx::query_scalar(
                r#"
                SELECT COALESCE(
                    (SELECT result_text FROM moss_term_corrections
                      WHERE segment_id = ? AND reverted_at IS NULL
                      ORDER BY revision DESC LIMIT 1),
                    (SELECT raw_text FROM moss_candidate_segments WHERE segment_id = ?)
                )
                "#,
            )
            .bind(&correction.segment_id)
            .bind(&correction.segment_id)
            .fetch_one(pool)
            .await
            .map_err(|_| MossApiError::operation_failed())?;
            let start = usize::try_from(correction.start_char)
                .map_err(|_| MossApiError::new("MOSS_CORRECTION_NOT_FOUND", false))?;
            let end = usize::try_from(correction.end_char)
                .map_err(|_| MossApiError::new("MOSS_CORRECTION_NOT_FOUND", false))?;
            if end < start {
                return Err(MossApiError::new("MOSS_CORRECTION_NOT_FOUND", false));
            }
            let actual = base_text
                .chars()
                .skip(start)
                .take(end.saturating_sub(start))
                .collect::<String>();
            if actual != correction.original_text {
                return Err(MossApiError::new("MOSS_CANDIDATE_CONFLICT", true));
            }
            if let Some(term_id) = correction.machine_term_id.clone() {
                MossCandidateRepository::add_machine_term_correction(
                    pool,
                    &correction.segment_id,
                    MachineTermSuggestion {
                        segment_index: u32::try_from(correction.segment_index)
                            .map_err(|_| MossApiError::operation_failed())?,
                        start_char: start,
                        end_char: end,
                        original_text: correction.original_text,
                        replacement_text: correction.replacement_text,
                        term_id,
                        context_sha256: correction
                            .machine_context_sha256
                            .ok_or_else(MossApiError::operation_failed)?,
                        token_track_sha256: correction
                            .machine_token_track_sha256
                            .ok_or_else(MossApiError::operation_failed)?,
                        model_sha256: correction
                            .machine_model_sha256
                            .ok_or_else(MossApiError::operation_failed)?,
                        first_token_index: u32::try_from(
                            correction
                                .machine_first_token_index
                                .ok_or_else(MossApiError::operation_failed)?,
                        )
                        .map_err(|_| MossApiError::operation_failed())?,
                        last_token_index: u32::try_from(
                            correction
                                .machine_last_token_index
                                .ok_or_else(MossApiError::operation_failed)?,
                        )
                        .map_err(|_| MossApiError::operation_failed())?,
                        confidence: correction
                            .machine_confidence
                            .ok_or_else(MossApiError::operation_failed)?,
                    },
                )
                .await
                .map_err(MossApiError::from)?;
            } else {
                MossCandidateRepository::add_term_correction(
                    pool,
                    &correction.segment_id,
                    TermCorrectionInput {
                        start_char: start,
                        end_char: end,
                        original_text: correction.original_text,
                        replacement_text: correction.replacement_text,
                        rule_id: correction.rule_id,
                        expected_context_sha256: run.context_sha256.clone(),
                    },
                )
                .await
                .map_err(MossApiError::from)?;
            }
        } else {
            MossCandidateRepository::revert_latest_term_correction(pool, &correction.correction_id)
                .await
                .map_err(|_| MossApiError::new("MOSS_CANDIDATE_CONFLICT", true))?;
        }
    }
    workspace_after_write(
        pool,
        &review_state,
        &request.meeting_id,
        Some(&run.run_id),
        status,
    )
    .await
}

#[tauri::command]
pub async fn api_moss_set_correction_state(
    app_state: State<'_, AppState>,
    layout: State<'_, StorageLayoutState>,
    helper: State<'_, Arc<MossHelperManager>>,
    review_state: State<'_, MossReviewState>,
    request: SetMossCorrectionStateRequest,
) -> Result<MossWorkspace, MossApiError> {
    let status = system_status(&layout, helper.inner().clone(), &review_state).await;
    set_correction_state_impl(app_state.db_manager.pool(), &review_state, request, status).await
}

async fn activate_candidate_impl(
    pool: &SqlitePool,
    review_state: &MossReviewState,
    request: ActivateMossCandidateRequest,
    status: MossSystemStatus,
) -> Result<MossWorkspace, MossApiError> {
    let _guard = review_state.operation_lock.lock().await;
    let run = run_for_meeting(pool, &request.meeting_id, &request.run_id).await?;
    ensure_candidate_editable(pool, &run).await?;
    ensure_expected_revision(pool, &run.run_id, request.expected_candidate_revision).await?;
    let current = MossCandidateRepository::current_transcript_sha256(pool, &request.meeting_id)
        .await
        .map_err(MossApiError::from)?;
    if !current.eq_ignore_ascii_case(&request.expected_current_transcript_sha256) {
        return Err(MossApiError::new("MOSS_ACTIVATION_CONFLICT", true));
    }
    MossCandidateRepository::activate_candidate(pool, &run.run_id)
        .await
        .map_err(MossApiError::from)?;
    workspace_after_write(
        pool,
        &review_state,
        &request.meeting_id,
        Some(&run.run_id),
        status,
    )
    .await
}

#[tauri::command]
pub async fn api_moss_activate_candidate(
    app_state: State<'_, AppState>,
    layout: State<'_, StorageLayoutState>,
    helper: State<'_, Arc<MossHelperManager>>,
    review_state: State<'_, MossReviewState>,
    request: ActivateMossCandidateRequest,
) -> Result<MossWorkspace, MossApiError> {
    let status = system_status(&layout, helper.inner().clone(), &review_state).await;
    activate_candidate_impl(app_state.db_manager.pool(), &review_state, request, status).await
}

async fn rollback_activation_impl(
    pool: &SqlitePool,
    review_state: &MossReviewState,
    request: RollbackMossActivationRequest,
    status: MossSystemStatus,
) -> Result<MossWorkspace, MossApiError> {
    let _guard = review_state.operation_lock.lock().await;
    let active: Option<(String, String)> = sqlx::query_as(
        "SELECT activation_id, run_id FROM moss_activation_snapshots WHERE meeting_id = ? AND status = 'active'",
    )
    .bind(&request.meeting_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| MossApiError::operation_failed())?;
    let (_activation_id, run_id) = active
        .filter(|(activation_id, _)| activation_id == &request.activation_id)
        .ok_or_else(|| MossApiError::new("MOSS_ROLLBACK_CONFLICT", false))?;
    let current = MossCandidateRepository::current_transcript_sha256(pool, &request.meeting_id)
        .await
        .map_err(MossApiError::from)?;
    if !current.eq_ignore_ascii_case(&request.expected_current_transcript_sha256) {
        return Err(MossApiError::new("MOSS_ROLLBACK_CONFLICT", true));
    }
    MossCandidateRepository::rollback_active_activation(pool, &request.meeting_id)
        .await
        .map_err(MossApiError::from)?;
    workspace_after_write(
        pool,
        &review_state,
        &request.meeting_id,
        Some(&run_id),
        status,
    )
    .await
}

#[tauri::command]
pub async fn api_moss_rollback_activation(
    app_state: State<'_, AppState>,
    layout: State<'_, StorageLayoutState>,
    helper: State<'_, Arc<MossHelperManager>>,
    review_state: State<'_, MossReviewState>,
    request: RollbackMossActivationRequest,
) -> Result<MossWorkspace, MossApiError> {
    let status = system_status(&layout, helper.inner().clone(), &review_state).await;
    rollback_activation_impl(app_state.db_manager.pool(), &review_state, request, status).await
}

struct CompletedProductCandidate {
    run: MossRunRecord,
    machine_suggestions: Vec<MachineTermSuggestion>,
}

async fn complete_product_aligned_candidate(
    pool: &SqlitePool,
    run: &MossRunRecord,
    transcription: &crate::moss_helper::manager::ManagedTranscription,
    token_track: Option<&crate::moss_audio_token_alignment::AudioTokenTrack>,
    token_failure_reason: Option<&str>,
    context: Option<&MeetingContextSnapshot>,
) -> Result<CompletedProductCandidate, MossStoreError> {
    let snapshot = MossCandidateRepository::source_transcript_anchors(pool, &run.run_id).await?;
    let aligned =
        align_managed_transcription_with_audio_tokens(transcription, &snapshot, token_track)
            .map_err(|_| MossStoreError::InvalidInput("managed_alignment"))?;
    let context_terms = context
        .map(|context| {
            context
                .terms
                .iter()
                .map(|term| ContextTerm {
                    term_id: term.term_id.clone(),
                    canonical: term.canonical.clone(),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let machine_suggestions = match (token_track, context) {
        (Some(track), Some(context)) => machine_term_suggestions_for_managed(
            transcription,
            &aligned,
            track,
            &context_terms,
            &context.context_sha256,
        ),
        _ => Vec::new(),
    };
    let activity = transcription
        .audio_activity
        .ok_or(MossStoreError::InvalidInput("audio_activity"))?;
    let audio_token_input = AudioTokenAlignmentInput {
        track: token_track.cloned(),
        boundaries: aligned.token_boundaries.clone(),
        global_match_coverage: token_track.map(|_| {
            aligned
                .token_global_match_coverage
                .unwrap_or_default()
                .clamp(0.0, 1.0)
        }),
        token_aligned_segment_count: aligned.token_aligned_segment_count,
        fallback_raw_segment_count: aligned.token_fallback_raw_segment_count,
        fallback_reason: token_failure_reason
            .map(str::to_owned)
            .or_else(|| aligned.token_fallback_reason.clone()),
    };
    let completed = MossCandidateRepository::complete_run_with_r5_provenance(
        pool,
        &run.run_id,
        &aligned.product.segments,
        MossCompletion {
            device_name: transcription.completed.device_description.clone(),
            raw_output_sha256: transcription.completed.raw_text_sha256.clone(),
            clean_output_sha256: transcription.completed.clean_text_sha256.clone(),
            wall_elapsed_ms: transcription.supervisor_wall_elapsed_ms,
            wall_rtf: transcription.supervisor_wall_rtf,
            peak_memory_bytes: transcription.helper_peak_job_memory_bytes,
            language_requested: transcription.language_requested.clone(),
            language_resolved: transcription.language_resolved.clone(),
            decode_parameters_json: transcription.decode_parameters_json.clone(),
            decode_parameters_sha256: transcription.decode_parameters_sha256.clone(),
        },
        &aligned.product.provenance,
        MossRunDiagnosticsInput {
            audio_duration_ms: activity.audio_duration_ms,
            activity_frame_ms: activity.frame_ms,
            activity_threshold_dbfs: f64::from(activity.threshold_dbfs),
            first_active_ms: activity.first_active_ms,
            last_active_ms: activity.last_active_ms,
            model_last_timestamp_ms: transcription.completed.last_timestamp_ms,
            aligned_segment_count: aligned.product.aligned_segment_count,
            fallback_segment_count: aligned.product.fallback_segment_count,
            source_anchor_count: aligned.product.source_anchor_count,
            source_hash_verified: aligned.product.source_hash_verified,
            source_expected_sha256: aligned.product.source_expected_sha256,
            source_actual_sha256: aligned.product.source_actual_sha256,
            fallback_reason: aligned.product.fallback_reason,
        },
        audio_token_input,
    )
    .await?;
    Ok(CompletedProductCandidate {
        run: completed,
        machine_suggestions,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use chrono::Utc;
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::database::manager::DatabaseManager;
    use crate::database::moss::{
        CandidateAlignmentInput, CandidateSegmentInput, MossCompletion, MossRunDiagnosticsInput,
    };
    use crate::meeting_context::{
        MeetingContextProfile, PersonProfile, TermProfile, MEETING_CONTEXT_SCHEMA_VERSION,
    };

    struct TestDatabase {
        directory: TempDir,
        database_path: PathBuf,
        legacy_path: PathBuf,
        manager: DatabaseManager,
    }

    async fn database() -> TestDatabase {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("moss-p4.sqlite");
        let legacy_path = directory.path().join("legacy.db");
        let manager = DatabaseManager::new(
            database_path.to_str().unwrap(),
            legacy_path.to_str().unwrap(),
        )
        .await
        .unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(manager.pool())
            .await
            .unwrap();
        TestDatabase {
            directory,
            database_path,
            legacy_path,
            manager,
        }
    }

    fn digest(label: &str) -> String {
        sha256_bytes(label.as_bytes())
    }

    fn status() -> MossSystemStatus {
        MossSystemStatus {
            schema_version: SCHEMA_VERSION,
            availability: "ready".to_owned(),
            installed: true,
            version: Some("v0.2.2".to_owned()),
            runtime_sha256: Some(digest("runtime")),
            model_sha256: Some(digest("model")),
            model_bytes: Some(1),
            available_disk_bytes: Some(1_000_000),
            device_name: Some("test-device".to_owned()),
            health: "healthy".to_owned(),
            supports_native_hotwords: false,
            detail_code: None,
            checked_at: "2026-08-29T00:00:00.000Z".to_owned(),
        }
    }

    fn context() -> MeetingContextContainer {
        context_from_parts(
            vec![
                PersonProfile {
                    person_id: "person_alice".to_owned(),
                    display_name: "Alice".to_owned(),
                    aliases: vec![],
                    department: Some("Product".to_owned()),
                    role: Some("Host".to_owned()),
                    enabled: true,
                },
                PersonProfile {
                    person_id: "person_bob".to_owned(),
                    display_name: "Bob".to_owned(),
                    aliases: vec![],
                    department: Some("Engineering".to_owned()),
                    role: None,
                    enabled: true,
                },
            ],
            vec![TermProfile {
                term_id: "term_moss".to_owned(),
                canonical: "MOSS".to_owned(),
                aliases: vec!["moss".to_owned()],
                category: Some("product".to_owned()),
                enabled: true,
            }],
        )
    }

    fn context_from_parts(
        people: Vec<PersonProfile>,
        terms: Vec<TermProfile>,
    ) -> MeetingContextContainer {
        MeetingContextContainer::from_profile(
            MeetingContextProfile {
                schema_version: MEETING_CONTEXT_SCHEMA_VERSION,
                fixed_meeting_mechanism: Some("weekly review".to_owned()),
                people,
                terms,
            },
            "template_p4".to_owned(),
            1,
            "a".repeat(64),
            Utc::now(),
        )
        .unwrap()
    }

    async fn insert_meeting(
        database: &TestDatabase,
        meeting_id: &str,
        transcript: &str,
    ) -> MeetingContextContainer {
        insert_meeting_with_context(database, meeting_id, transcript, context()).await
    }

    async fn insert_meeting_with_context(
        database: &TestDatabase,
        meeting_id: &str,
        transcript: &str,
        context: MeetingContextContainer,
    ) -> MeetingContextContainer {
        let folder = database
            .directory
            .path()
            .join(format!("meeting-{meeting_id}"));
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("metadata.json"),
            serde_json::to_vec_pretty(&json!({ "meeting_context": context })).unwrap(),
        )
        .unwrap();
        let now = "2026-08-29T00:00:00.000Z";
        sqlx::query(
            "INSERT INTO meetings (id, title, created_at, updated_at, folder_path) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(meeting_id)
        .bind(format!("Meeting {meeting_id}"))
        .bind(now)
        .bind(now)
        .bind(folder.to_str().unwrap())
        .execute(database.manager.pool())
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO transcripts (
                id, meeting_id, transcript, timestamp, summary, action_items,
                key_points, audio_start_time, audio_end_time, duration, speaker
            ) VALUES (?, ?, ?, ?, ?, ?, ?, 0.0, 1.0, 1.0, 'microphone')
            "#,
        )
        .bind(format!("transcript-{meeting_id}"))
        .bind(meeting_id)
        .bind(transcript)
        .bind(now)
        .bind("preserved summary")
        .bind("preserved action")
        .bind("preserved key point")
        .execute(database.manager.pool())
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO transcript_chunks (
                meeting_id, transcript_text, model, model_name, created_at
            ) VALUES (?, ?, 'whisper', 'baseline', ?)
            "#,
        )
        .bind(meeting_id)
        .bind(transcript)
        .bind(now)
        .execute(database.manager.pool())
        .await
        .unwrap();
        context
    }

    fn run_input(meeting_id: &str, context_sha256: &str) -> NewMossRun {
        NewMossRun {
            meeting_id: meeting_id.to_owned(),
            audio_sha256: digest("audio"),
            model_sha256: digest("model"),
            runtime_sha256: digest("runtime"),
            context_sha256: context_sha256.to_owned(),
            backend_name: "moss-transcribe-diarize".to_owned(),
            backend_version: "v0.2.2".to_owned(),
            runtime_version: "c6a9257".to_owned(),
            model_revision: "MOSS-Transcribe-Diarize-Q8_0".to_owned(),
            language_requested: moss_helper::native::MOSS_LANGUAGE_REQUESTED.to_owned(),
            decode_parameters_json: moss_helper::native::MOSS_DECODE_PARAMETERS_JSON.to_owned(),
            decode_parameters_sha256: moss_helper::native::moss_decode_parameters_sha256(),
        }
    }

    fn candidate() -> Vec<CandidateSegmentInput> {
        vec![
            CandidateSegmentInput {
                segment_index: 0,
                start_ms: 0,
                end_ms: 1_000,
                speaker_label: "S01".to_owned(),
                text: "we use moss today".to_owned(),
            },
            CandidateSegmentInput {
                segment_index: 1,
                start_ms: 1_000,
                end_ms: 2_000,
                speaker_label: "S01".to_owned(),
                text: "second segment".to_owned(),
            },
            CandidateSegmentInput {
                segment_index: 2,
                start_ms: 2_000,
                end_ms: 3_000,
                speaker_label: "S02".to_owned(),
                text: "third segment".to_owned(),
            },
        ]
    }

    async fn completed_run(database: &TestDatabase, meeting_id: &str) -> MossRunRecord {
        completed_run_with(
            database,
            meeting_id,
            context(),
            candidate(),
            "original transcript",
        )
        .await
    }

    async fn completed_run_with(
        database: &TestDatabase,
        meeting_id: &str,
        context: MeetingContextContainer,
        segments: Vec<CandidateSegmentInput>,
        original_transcript: &str,
    ) -> MossRunRecord {
        let context =
            insert_meeting_with_context(database, meeting_id, original_transcript, context).await;
        let context_sha256 = context.current_context().unwrap().context_sha256.clone();
        let run = start_run_record(
            database.manager.pool(),
            run_input(meeting_id, &context_sha256),
        )
        .await
        .unwrap();
        MossCandidateRepository::complete_run(
            database.manager.pool(),
            &run.run_id,
            &segments,
            MossCompletion {
                device_name: "test-device".to_owned(),
                raw_output_sha256: digest("raw"),
                clean_output_sha256: digest("clean"),
                wall_elapsed_ms: 1_000,
                wall_rtf: 0.5,
                peak_memory_bytes: 1_024,
                language_requested: moss_helper::native::MOSS_LANGUAGE_REQUESTED.to_owned(),
                language_resolved: moss_helper::native::MOSS_LANGUAGE_RESOLVED.to_owned(),
                decode_parameters_json: moss_helper::native::MOSS_DECODE_PARAMETERS_JSON.to_owned(),
                decode_parameters_sha256: moss_helper::native::moss_decode_parameters_sha256(),
            },
        )
        .await
        .unwrap()
    }

    async fn segment_id(pool: &SqlitePool, run_id: &str, index: i64) -> String {
        sqlx::query_scalar(
            "SELECT segment_id FROM moss_candidate_segments WHERE run_id = ? AND segment_index = ?",
        )
        .bind(run_id)
        .bind(index)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    fn review(workspace: &MossWorkspace) -> &MossCandidateReview {
        workspace.review.as_ref().expect("candidate review")
    }

    #[tokio::test]
    async fn workspace_rebuilds_from_sqlite_after_database_reopen() {
        let database = database().await;
        let run = completed_run(&database, "meeting-reopen").await;
        let first = build_workspace(
            database.manager.pool(),
            &MossReviewState::new(),
            "meeting-reopen",
            Some(&run.run_id),
            status(),
        )
        .await
        .unwrap();
        let expected = serde_json::to_value(&first).unwrap();
        let first_review = review(&first);
        assert_eq!(
            first_review.candidate.alignments.len(),
            first_review.candidate.segments.len()
        );
        assert!(first_review.candidate.diagnostics.is_none());
        assert!(first_review
            .candidate
            .alignments
            .iter()
            .all(|alignment| alignment.alignment_method == "moss_segment"));
        let TestDatabase {
            directory,
            database_path,
            legacy_path,
            manager,
        } = database;
        manager.pool().close().await;
        drop(manager);
        let reopened = DatabaseManager::new(
            database_path.to_str().unwrap(),
            legacy_path.to_str().unwrap(),
        )
        .await
        .unwrap();
        let rebuilt = build_workspace(
            reopened.pool(),
            &MossReviewState::new(),
            "meeting-reopen",
            Some(&run.run_id),
            status(),
        )
        .await
        .unwrap();
        assert_eq!(serde_json::to_value(rebuilt).unwrap(), expected);
        assert!(directory.path().exists());
    }

    #[tokio::test]
    async fn workspace_exposes_hash_bound_alignment_and_audio_tail_diagnostics() {
        let database = database().await;
        let context = insert_meeting(&database, "meeting-r4", "CGS 项目").await;
        let run = start_run_record(
            database.manager.pool(),
            run_input(
                "meeting-r4",
                &context.current_context().unwrap().context_sha256,
            ),
        )
        .await
        .unwrap();
        let segments = vec![
            CandidateSegmentInput {
                segment_index: 0,
                start_ms: 0,
                end_ms: 500,
                speaker_label: "S01".to_owned(),
                text: "CGS ".to_owned(),
            },
            CandidateSegmentInput {
                segment_index: 1,
                start_ms: 500,
                end_ms: 1_000,
                speaker_label: "S01".to_owned(),
                text: "项目".to_owned(),
            },
        ];
        let raw_hash = digest("CGS 项目");
        let alignments = segments
            .iter()
            .enumerate()
            .map(|(index, segment)| CandidateAlignmentInput {
                segment_index: segment.segment_index,
                raw_segment_index: 0,
                raw_start_ms: 0,
                raw_end_ms: 1_000,
                raw_text_sha256: raw_hash.clone(),
                alignment_method: "source_transcript_segment".to_owned(),
                confidence: Some(0.95),
                source_anchor_ids: vec![format!("transcript-meeting-r4-{index}")],
            })
            .collect::<Vec<_>>();
        MossCandidateRepository::complete_run_with_provenance(
            database.manager.pool(),
            &run.run_id,
            &segments,
            MossCompletion {
                device_name: "test-device".to_owned(),
                raw_output_sha256: digest("r4-raw"),
                clean_output_sha256: digest("r4-clean"),
                wall_elapsed_ms: 800,
                wall_rtf: 0.8,
                peak_memory_bytes: 2_048,
                language_requested: moss_helper::native::MOSS_LANGUAGE_REQUESTED.to_owned(),
                language_resolved: moss_helper::native::MOSS_LANGUAGE_RESOLVED.to_owned(),
                decode_parameters_json: moss_helper::native::MOSS_DECODE_PARAMETERS_JSON.to_owned(),
                decode_parameters_sha256: moss_helper::native::moss_decode_parameters_sha256(),
            },
            &alignments,
            MossRunDiagnosticsInput {
                audio_duration_ms: 1_200,
                activity_frame_ms: 20,
                activity_threshold_dbfs: -50.0,
                first_active_ms: Some(0),
                last_active_ms: Some(980),
                model_last_timestamp_ms: 1_000,
                aligned_segment_count: 2,
                fallback_segment_count: 0,
                source_anchor_count: 2,
                source_hash_verified: true,
                source_expected_sha256: run.source_transcript_sha256.clone(),
                source_actual_sha256: run.source_transcript_sha256.clone(),
                fallback_reason: None,
            },
        )
        .await
        .unwrap();

        let workspace = build_workspace(
            database.manager.pool(),
            &MossReviewState::new(),
            &run.meeting_id,
            Some(&run.run_id),
            status(),
        )
        .await
        .unwrap();
        let candidate = &review(&workspace).candidate;
        assert_eq!(candidate.alignments.len(), 2);
        assert!(candidate
            .alignments
            .iter()
            .all(|alignment| alignment.alignment_method == "source_transcript_segment"));
        assert_eq!(candidate.alignments[0].raw_text_sha256, raw_hash);
        let diagnostics = candidate.diagnostics.as_ref().unwrap();
        assert!(diagnostics.source_hash_verified);
        assert_eq!(diagnostics.last_active_ms, Some(980));
        assert_eq!(diagnostics.model_last_timestamp_ms, 1_000);
        assert_eq!(diagnostics.tail_delta_ms, Some(20));
    }

    #[tokio::test]
    async fn bulk_binding_applies_to_label_and_segment_override_wins() {
        let database = database().await;
        let run = completed_run(&database, "meeting-binding").await;
        let state = MossReviewState::new();
        let initial_revision = current_candidate_revision(database.manager.pool(), &run.run_id)
            .await
            .unwrap();
        let bound = save_speaker_binding_impl(
            database.manager.pool(),
            &state,
            SaveMossSpeakerBindingRequest {
                meeting_id: run.meeting_id.clone(),
                run_id: run.run_id.clone(),
                speaker_label: "S01".to_owned(),
                person_id: Some("person_alice".to_owned()),
                expected_candidate_revision: initial_revision,
            },
            status(),
        )
        .await
        .unwrap();
        let bound_review = review(&bound);
        assert_eq!(
            bound_review.candidate.segments[0]
                .resolved_person_id
                .as_deref(),
            Some("person_alice")
        );
        assert_eq!(
            bound_review.candidate.segments[1]
                .resolved_person_id
                .as_deref(),
            Some("person_alice")
        );
        assert_eq!(
            bound_review.candidate.segments[0].speaker_resolution,
            "bulk_binding"
        );
        let overridden = save_segment_override_impl(
            database.manager.pool(),
            &state,
            SaveMossSegmentOverrideRequest {
                meeting_id: run.meeting_id.clone(),
                run_id: run.run_id.clone(),
                segment_id: bound_review.candidate.segments[1].segment_id.clone(),
                person_id: Some("person_bob".to_owned()),
                expected_candidate_revision: bound_review.candidate.revision,
            },
            status(),
        )
        .await
        .unwrap();
        let segments = &review(&overridden).candidate.segments;
        assert_eq!(
            segments[0].resolved_person_id.as_deref(),
            Some("person_alice")
        );
        assert_eq!(
            segments[1].resolved_person_id.as_deref(),
            Some("person_bob")
        );
        assert_eq!(
            segments[1].segment_override_person_id.as_deref(),
            Some("person_bob")
        );
        assert_eq!(segments[1].speaker_resolution, "segment_override");
        assert_eq!(segments[2].resolved_person_id, None);
    }

    #[tokio::test]
    async fn candidate_edit_updates_workspace_and_rejects_stale_revision() {
        let database = database().await;
        let run = completed_run(&database, "meeting-edit").await;
        let state = MossReviewState::new();
        let segment_id = segment_id(database.manager.pool(), &run.run_id, 0).await;
        let revision = current_candidate_revision(database.manager.pool(), &run.run_id)
            .await
            .unwrap();
        let updated = update_candidate_segment_impl(
            database.manager.pool(),
            &state,
            UpdateMossCandidateSegmentRequest {
                meeting_id: run.meeting_id.clone(),
                run_id: run.run_id.clone(),
                segment_id: segment_id.clone(),
                text: "  reviewed text  ".to_owned(),
                expected_candidate_revision: revision,
            },
            status(),
        )
        .await
        .unwrap();
        assert_eq!(review(&updated).candidate.segments[0].text, "reviewed text");
        assert!(review(&updated).candidate.revision > revision);
        let error = update_candidate_segment_impl(
            database.manager.pool(),
            &state,
            UpdateMossCandidateSegmentRequest {
                meeting_id: run.meeting_id.clone(),
                run_id: run.run_id.clone(),
                segment_id,
                text: "stale write".to_owned(),
                expected_candidate_revision: revision,
            },
            status(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "MOSS_CANDIDATE_STALE");
        assert!(error.retryable);
    }

    #[tokio::test]
    async fn term_correction_can_be_reverted_and_reapplied_through_command_impl() {
        let database = database().await;
        let run = completed_run(&database, "meeting-correction").await;
        let segment_id = segment_id(database.manager.pool(), &run.run_id, 0).await;
        let correction = MossCandidateRepository::add_term_correction(
            database.manager.pool(),
            &segment_id,
            TermCorrectionInput {
                start_char: 7,
                end_char: 11,
                original_text: "moss".to_owned(),
                replacement_text: "MOSS".to_owned(),
                rule_id: "explicit-term-moss".to_owned(),
                expected_context_sha256: run.context_sha256.clone(),
            },
        )
        .await
        .unwrap();
        let state = MossReviewState::new();
        let revision = current_candidate_revision(database.manager.pool(), &run.run_id)
            .await
            .unwrap();
        let reverted = set_correction_state_impl(
            database.manager.pool(),
            &state,
            SetMossCorrectionStateRequest {
                meeting_id: run.meeting_id.clone(),
                run_id: run.run_id.clone(),
                correction_id: correction.correction_id.clone(),
                applied: false,
                expected_candidate_revision: revision,
            },
            status(),
        )
        .await
        .unwrap();
        assert_eq!(
            review(&reverted).candidate.segments[0].text,
            "we use moss today"
        );
        assert_eq!(review(&reverted).corrections[0].state, "reverted");
        let reapplied = set_correction_state_impl(
            database.manager.pool(),
            &state,
            SetMossCorrectionStateRequest {
                meeting_id: run.meeting_id.clone(),
                run_id: run.run_id.clone(),
                correction_id: correction.correction_id,
                applied: true,
                expected_candidate_revision: review(&reverted).candidate.revision,
            },
            status(),
        )
        .await
        .unwrap();
        let reapplied_review = review(&reapplied);
        assert_eq!(
            reapplied_review.candidate.segments[0].text,
            "we use MOSS today"
        );
        assert_eq!(
            reapplied_review
                .corrections
                .iter()
                .filter(|value| value.state == "applied")
                .count(),
            1
        );
        assert_eq!(
            reapplied_review
                .corrections
                .iter()
                .filter(|value| value.state == "reverted")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn activate_and_rollback_return_consistent_workspaces_and_activation_blocks_edit() {
        let database = database().await;
        let run = completed_run(&database, "meeting-activation").await;
        let state = MossReviewState::new();
        let revision = current_candidate_revision(database.manager.pool(), &run.run_id)
            .await
            .unwrap();
        let current_sha256 = MossCandidateRepository::current_transcript_sha256(
            database.manager.pool(),
            &run.meeting_id,
        )
        .await
        .unwrap();
        let activated = activate_candidate_impl(
            database.manager.pool(),
            &state,
            ActivateMossCandidateRequest {
                meeting_id: run.meeting_id.clone(),
                run_id: run.run_id.clone(),
                expected_candidate_revision: revision,
                expected_current_transcript_sha256: current_sha256,
            },
            status(),
        )
        .await
        .unwrap();
        let activated_review = review(&activated);
        assert!(activated_review.candidate.is_active);
        assert!(activated_review.activation.can_rollback);
        assert_eq!(activated_review.current.segments.len(), candidate().len());
        let edit_error = update_candidate_segment_impl(
            database.manager.pool(),
            &state,
            UpdateMossCandidateSegmentRequest {
                meeting_id: run.meeting_id.clone(),
                run_id: run.run_id.clone(),
                segment_id: activated_review.candidate.segments[0].segment_id.clone(),
                text: "forbidden edit".to_owned(),
                expected_candidate_revision: activated_review.candidate.revision,
            },
            status(),
        )
        .await
        .unwrap_err();
        assert_eq!(edit_error.code, "MOSS_CANDIDATE_CONFLICT");
        let rolled_back = rollback_activation_impl(
            database.manager.pool(),
            &state,
            RollbackMossActivationRequest {
                meeting_id: run.meeting_id.clone(),
                activation_id: activated_review
                    .activation
                    .active_activation_id
                    .clone()
                    .unwrap(),
                expected_current_transcript_sha256: activated_review
                    .activation
                    .current_transcript_sha256
                    .clone(),
            },
            status(),
        )
        .await
        .unwrap();
        let rolled_back_review = review(&rolled_back);
        assert!(!rolled_back_review.candidate.is_active);
        assert_eq!(rolled_back_review.activation.active_activation_id, None);
        assert_eq!(rolled_back_review.current.segments.len(), 1);
        assert_eq!(
            rolled_back_review.current.segments[0].text,
            "original transcript"
        );
    }

    #[tokio::test]
    async fn duplicate_start_is_rejected_by_real_p3_repository_adapter() {
        let database = database().await;
        let context = insert_meeting(&database, "meeting-duplicate", "original transcript").await;
        let input = run_input(
            "meeting-duplicate",
            &context.current_context().unwrap().context_sha256,
        );
        let _first = start_run_record(database.manager.pool(), input.clone())
            .await
            .unwrap();
        let error = start_run_record(database.manager.pool(), input)
            .await
            .unwrap_err();
        assert_eq!(error.code, "MOSS_RUN_ALREADY_ACTIVE");
        assert!(!error.retryable);
    }

    #[tokio::test]
    async fn cancel_uses_the_run_to_helper_request_mapping() {
        let database = database().await;
        let context = insert_meeting(&database, "meeting-cancel", "original transcript").await;
        let run = start_run_record(
            database.manager.pool(),
            run_input(
                "meeting-cancel",
                &context.current_context().unwrap().context_sha256,
            ),
        )
        .await
        .unwrap();
        let state = MossReviewState::new();
        state.remember_request(run.run_id.clone(), "helper-request-42".to_owned());
        let observed = Arc::new(Mutex::new(None::<String>));
        let observed_for_cancel = observed.clone();
        let workspace = cancel_run_impl(
            database.manager.pool(),
            &state,
            MossRunRequest {
                meeting_id: run.meeting_id.clone(),
                run_id: run.run_id.clone(),
            },
            status(),
            move |helper_request_id| {
                *observed_for_cancel.lock().unwrap() = Some(helper_request_id.to_owned());
                Ok(true)
            },
        )
        .await
        .unwrap();
        assert_eq!(
            observed.lock().unwrap().as_deref(),
            Some("helper-request-42")
        );
        assert_eq!(workspace.runs[0].state, "cancel_requested");
        assert!(!workspace.runs[0].can_cancel);
        assert!(state.is_cancel_requested(&run.run_id));
        let missing_mapping_error = cancel_run_impl(
            database.manager.pool(),
            &MossReviewState::new(),
            MossRunRequest {
                meeting_id: run.meeting_id,
                run_id: run.run_id,
            },
            status(),
            |_| Ok(true),
        )
        .await
        .unwrap_err();
        assert_eq!(missing_mapping_error.code, "MOSS_CANCEL_FAILED");
    }

    #[test]
    fn explicit_alias_positive_match_uses_the_immutable_source_text() {
        let matches = select_explicit_alias_matches(
            "publish on U2B",
            &[(
                "TERM".to_owned(),
                "term_youtube".to_owned(),
                "U2B".to_owned(),
                "YouTube".to_owned(),
            )],
        );
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].matched, "U2B");
        assert_eq!(matches[0].canonical, "YouTube");
    }

    #[test]
    fn explicit_ascii_alias_requires_word_boundaries() {
        let matches = select_explicit_alias_matches(
            "mossy moss _moss moss-",
            &[(
                "TERM".to_owned(),
                "term_moss".to_owned(),
                "moss".to_owned(),
                "MOSS".to_owned(),
            )],
        );
        assert_eq!(matches.len(), 2);
        assert_eq!(
            matches
                .iter()
                .map(|value| (value.start, value.end))
                .collect::<Vec<_>>(),
            vec![(17, 21), (6, 10)]
        );
        assert!(select_explicit_alias_matches(
            "MOSS",
            &[(
                "TERM".to_owned(),
                "term_long".to_owned(),
                "MOSS candidate".to_owned(),
                "MOSS".to_owned(),
            )],
        )
        .is_empty());
    }

    #[test]
    fn overlapping_aliases_choose_the_longest_non_overlapping_match() {
        let matches = select_explicit_alias_matches(
            "data lake",
            &[
                (
                    "TERM".to_owned(),
                    "term_data".to_owned(),
                    "data".to_owned(),
                    "DATA".to_owned(),
                ),
                (
                    "TERM".to_owned(),
                    "term_data_lake".to_owned(),
                    "data lake".to_owned(),
                    "Lakehouse".to_owned(),
                ),
            ],
        );
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].alias, "data lake");
        assert_eq!(matches[0].canonical, "Lakehouse");
    }

    #[tokio::test]
    async fn registered_term_alias_is_applied_with_a_term_rule_id() {
        let database = database().await;
        let context = context();
        let snapshot = context.current_context().unwrap().clone();
        let run = completed_run_with(
            &database,
            "meeting-term-alias",
            context,
            candidate(),
            "original transcript",
        )
        .await;
        apply_explicit_term_corrections(database.manager.pool(), &run, Some(&snapshot))
            .await
            .unwrap();
        let workspace = build_workspace(
            database.manager.pool(),
            &MossReviewState::new(),
            &run.meeting_id,
            Some(&run.run_id),
            status(),
        )
        .await
        .unwrap();
        let candidate_review = review(&workspace);
        assert_eq!(
            candidate_review.candidate.segments[0].text,
            "we use MOSS today"
        );
        assert_eq!(candidate_review.corrections.len(), 1);
        assert!(candidate_review.corrections[0]
            .rule_id
            .starts_with("TERM_ALIAS_"));
    }

    #[tokio::test]
    async fn reviewed_google_package_alias_is_traceable_and_raw_text_is_preserved() {
        let database = database().await;
        let context = context_from_parts(
            vec![],
            vec![TermProfile {
                term_id: "term_google_package".to_owned(),
                canonical: "Google 包".to_owned(),
                aliases: vec!["宝宝".to_owned()],
                category: Some("acquisition".to_owned()),
                enabled: true,
            }],
        );
        let snapshot = context.current_context().unwrap().clone();
        let run = completed_run_with(
            &database,
            "meeting-google-package-alias",
            context,
            vec![CandidateSegmentInput {
                segment_index: 0,
                start_ms: 125_440,
                end_ms: 147_410,
                speaker_label: "S05".to_owned(),
                text: "会给宝宝另外一个邀请卡".to_owned(),
            }],
            "会给宝宝另外一个邀请卡",
        )
        .await;

        apply_explicit_term_corrections(database.manager.pool(), &run, Some(&snapshot))
            .await
            .unwrap();

        let workspace = build_workspace(
            database.manager.pool(),
            &MossReviewState::new(),
            &run.meeting_id,
            Some(&run.run_id),
            status(),
        )
        .await
        .unwrap();
        let candidate_review = review(&workspace);
        assert_eq!(
            candidate_review.candidate.segments[0].text,
            "会给Google 包另外一个邀请卡"
        );
        assert_eq!(candidate_review.corrections.len(), 1);
        assert!(candidate_review.corrections[0]
            .rule_id
            .starts_with("TERM_ALIAS_"));

        let stored_raw: String = sqlx::query_scalar(
            "SELECT raw_text FROM moss_candidate_segments WHERE run_id = ? AND segment_index = 0",
        )
        .bind(&run.run_id)
        .fetch_one(database.manager.pool())
        .await
        .unwrap();
        assert_eq!(stored_raw, "会给宝宝另外一个邀请卡");
    }

    #[tokio::test]
    async fn replacement_text_is_never_used_as_a_new_chain_match() {
        let database = database().await;
        let context = context_from_parts(
            vec![PersonProfile {
                person_id: "person_beta".to_owned(),
                display_name: "BETA".to_owned(),
                aliases: vec!["moss".to_owned()],
                department: None,
                role: None,
                enabled: true,
            }],
            vec![TermProfile {
                term_id: "term_gamma".to_owned(),
                canonical: "GAMMA".to_owned(),
                aliases: vec!["BETA".to_owned()],
                category: None,
                enabled: true,
            }],
        );
        let snapshot = context.current_context().unwrap().clone();
        let run = completed_run_with(
            &database,
            "meeting-no-chain",
            context,
            vec![CandidateSegmentInput {
                segment_index: 0,
                start_ms: 0,
                end_ms: 1_000,
                speaker_label: "S01".to_owned(),
                text: "we use moss today".to_owned(),
            }],
            "original transcript",
        )
        .await;
        apply_explicit_term_corrections(database.manager.pool(), &run, Some(&snapshot))
            .await
            .unwrap();
        let latest: String = sqlx::query_scalar(
            "SELECT result_text FROM moss_term_corrections ORDER BY revision DESC LIMIT 1",
        )
        .fetch_one(database.manager.pool())
        .await
        .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM moss_term_corrections")
            .fetch_one(database.manager.pool())
            .await
            .unwrap();
        assert_eq!(latest, "we use BETA today");
        assert!(!latest.contains("GAMMA"));
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn registered_person_alias_is_corrected_without_guessing_similar_names() {
        let database = database().await;
        let context = context_from_parts(
            vec![PersonProfile {
                person_id: "person_alice".to_owned(),
                display_name: "Alice".to_owned(),
                aliases: vec!["阿丽".to_owned()],
                department: None,
                role: None,
                enabled: true,
            }],
            vec![],
        );
        let snapshot = context.current_context().unwrap().clone();
        let run = completed_run_with(
            &database,
            "meeting-person-alias",
            context,
            vec![CandidateSegmentInput {
                segment_index: 0,
                start_ms: 0,
                end_ms: 1_000,
                speaker_label: "S01".to_owned(),
                text: "阿丽 和 艾丽丝 发言".to_owned(),
            }],
            "original transcript",
        )
        .await;
        apply_explicit_term_corrections(database.manager.pool(), &run, Some(&snapshot))
            .await
            .unwrap();
        let workspace = build_workspace(
            database.manager.pool(),
            &MossReviewState::new(),
            &run.meeting_id,
            Some(&run.run_id),
            status(),
        )
        .await
        .unwrap();
        let candidate_review = review(&workspace);
        assert_eq!(
            candidate_review.candidate.segments[0].text,
            "Alice 和 艾丽丝 发言"
        );
        assert_eq!(candidate_review.corrections.len(), 1);
        assert!(candidate_review.corrections[0]
            .rule_id
            .starts_with("PERSON_ALIAS_"));
        assert_eq!(
            candidate_review.candidate.segments[0].speaker_resolution,
            "anonymous"
        );
        assert_eq!(
            candidate_review.candidate.segments[0].resolved_person_id,
            None
        );
    }

    #[tokio::test]
    async fn cross_namespace_ambiguous_alias_is_not_auto_corrected() {
        let database = database().await;
        let context = context_from_parts(
            vec![PersonProfile {
                person_id: "person_alice".to_owned(),
                display_name: "Alice".to_owned(),
                aliases: vec!["ACE".to_owned()],
                department: None,
                role: None,
                enabled: true,
            }],
            vec![TermProfile {
                term_id: "term_ace".to_owned(),
                canonical: "Access Control Engine".to_owned(),
                aliases: vec!["ace".to_owned()],
                category: None,
                enabled: true,
            }],
        );
        let snapshot = context.current_context().unwrap().clone();
        let run = completed_run_with(
            &database,
            "meeting-ambiguous-alias",
            context,
            vec![CandidateSegmentInput {
                segment_index: 0,
                start_ms: 0,
                end_ms: 1_000,
                speaker_label: "S01".to_owned(),
                text: "ACE is here".to_owned(),
            }],
            "original transcript",
        )
        .await;
        apply_explicit_term_corrections(database.manager.pool(), &run, Some(&snapshot))
            .await
            .unwrap();
        let correction_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM moss_term_corrections")
                .fetch_one(database.manager.pool())
                .await
                .unwrap();
        let raw_text: String =
            sqlx::query_scalar("SELECT raw_text FROM moss_candidate_segments WHERE run_id = ?")
                .bind(&run.run_id)
                .fetch_one(database.manager.pool())
                .await
                .unwrap();
        assert_eq!(raw_text, "ACE is here");
        assert_eq!(correction_count, 0);
    }

    #[test]
    #[cfg(windows)]
    fn available_disk_field_comes_from_the_mount_containing_the_storage_path() {
        let storage_directory = tempfile::tempdir().unwrap();
        let storage_path = storage_directory.path().canonicalize().unwrap();
        let disks = sysinfo::Disks::new_with_refreshed_list();
        let available = available_bytes_for_path_from_disks(&storage_path, &disks)
            .expect("storage disk space for canonical Windows path");
        let expected = disks
            .list()
            .iter()
            .filter(|disk| {
                crate::storage::layout::path_is_same_or_descendant(
                    &storage_path,
                    disk.mount_point(),
                )
            })
            .max_by_key(|disk| disk.mount_point().components().count())
            .unwrap()
            .available_space();
        assert_eq!(available, expected);
        assert!(available > 0);
        assert!(storage_path.to_string_lossy().starts_with(r"\\?\"));
    }

    #[test]
    fn public_error_payload_is_code_only_and_does_not_leak_source_text_or_paths() {
        let source_secret = "C:\\Users\\private\\meeting.wav transcript secret";
        let error = MossApiError::from(MossStoreError::InvalidInput(source_secret));
        let serialized = serde_json::to_value(&error).unwrap();
        assert_eq!(serialized["code"], "MOSS_OPERATION_FAILED");
        assert_eq!(serialized["retryable"], true);
        assert!(serialized["debugId"].as_str().unwrap().starts_with("moss-"));
        assert_eq!(serialized.as_object().unwrap().len(), 3);
        let text = serialized.to_string();
        assert!(!text.contains(source_secret));
        assert!(!text.contains("meeting.wav"));
        assert!(!text.contains("transcript secret"));
    }
}
