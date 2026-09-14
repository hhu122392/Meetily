use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::State;

use moss_helper::protocol::ErrorCode;

use super::manager::{
    ManagedTranscription, ManagerError, ManagerStatus, MossHelperManager, PreparationLease,
    TranscribeInput, MAX_REQUEST_SECONDS,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MossTranscribeFileRequest {
    pub request_id: String,
    pub context_sha256: String,
    pub runtime_directory: PathBuf,
    pub model_path: PathBuf,
    pub model_bytes: u64,
    pub model_sha256: String,
    pub device_id: Option<String>,
    pub source_audio_path: PathBuf,
    pub timeout_ms: u64,
    pub language_requested: String,
    pub decode_parameters_json: String,
    pub decode_parameters_sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MossProbeRequest {
    pub request_id: String,
    pub runtime_directory: PathBuf,
    pub device_id: Option<String>,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MossCommandError {
    pub code: &'static str,
    pub message: String,
}

impl From<ManagerError> for MossCommandError {
    fn from(error: ManagerError) -> Self {
        let message = match &error {
            ManagerError::Spawn(source) => source.to_string(),
            _ => "MOSS operation failed".to_string(),
        };
        let code = match error {
            ManagerError::Unavailable => "MOSS_HELPER_UNAVAILABLE",
            ManagerError::InvalidRequest => "MOSS_INVALID_REQUEST",
            ManagerError::AudioTooLong => "MOSS_AUDIO_TOO_LONG",
            ManagerError::DuplicateRequest => "MOSS_DUPLICATE_REQUEST",
            ManagerError::Busy => "MOSS_BUSY",
            ManagerError::AudioStage => "MOSS_AUDIO_STAGE_FAILED",
            ManagerError::Spawn(_) => "MOSS_HELPER_SPAWN_FAILED",
            ManagerError::Protocol => "MOSS_PROTOCOL_FAILED",
            ManagerError::Timeout => "MOSS_TIMEOUT",
            ManagerError::PerformanceGate => "MOSS_PERFORMANCE_GATE_FAILED",
            ManagerError::Cancelled => "MOSS_CANCELLED",
            ManagerError::Native(code) => native_error_code(code),
        };
        Self { code, message }
    }
}

fn native_error_code(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::ProtocolLineTooLong => "MOSS_PROTOCOL_LINE_TOO_LONG",
        ErrorCode::ProtocolInvalidJson => "MOSS_PROTOCOL_INVALID_JSON",
        ErrorCode::ProtocolUnsupportedVersion => "MOSS_PROTOCOL_UNSUPPORTED_VERSION",
        ErrorCode::ProtocolInvalidRequestId => "MOSS_PROTOCOL_INVALID_REQUEST_ID",
        ErrorCode::ProtocolInvalidClientSeq => "MOSS_PROTOCOL_INVALID_CLIENT_SEQ",
        ErrorCode::ProtocolUnexpectedMessage => "MOSS_PROTOCOL_UNEXPECTED_MESSAGE",
        ErrorCode::ProtocolInvalidContextHash => "MOSS_PROTOCOL_INVALID_CONTEXT_HASH",
        ErrorCode::ProtocolEof => "MOSS_PROTOCOL_EOF",
        ErrorCode::ProtocolIo => "MOSS_PROTOCOL_IO",
        ErrorCode::RuntimeUnsupportedPlatform => "MOSS_RUNTIME_UNSUPPORTED_PLATFORM",
        ErrorCode::RuntimeEnvironmentForbidden => "MOSS_RUNTIME_ENVIRONMENT_FORBIDDEN",
        ErrorCode::RuntimeContractMissing => "MOSS_RUNTIME_CONTRACT_MISSING",
        ErrorCode::RuntimeContractMismatch => "MOSS_RUNTIME_CONTRACT_MISMATCH",
        ErrorCode::RuntimeHashMismatch => "MOSS_RUNTIME_HASH_MISMATCH",
        ErrorCode::RuntimeLoadFailed => "MOSS_RUNTIME_LOAD_FAILED",
        ErrorCode::RuntimeSymbolMissing => "MOSS_RUNTIME_SYMBOL_MISSING",
        ErrorCode::RuntimeVersionMismatch => "MOSS_RUNTIME_VERSION_MISMATCH",
        ErrorCode::RuntimeCommitMismatch => "MOSS_RUNTIME_COMMIT_MISMATCH",
        ErrorCode::RuntimeAbiMismatch => "MOSS_RUNTIME_ABI_MISMATCH",
        ErrorCode::BackendInitFailed => "MOSS_BACKEND_INIT_FAILED",
        ErrorCode::DevicePolicyRejected => "MOSS_DEVICE_POLICY_REJECTED",
        ErrorCode::DeviceNotFound => "MOSS_DEVICE_NOT_FOUND",
        ErrorCode::DeviceAmbiguous => "MOSS_DEVICE_AMBIGUOUS",
        ErrorCode::DeviceFallbackForbidden => "MOSS_DEVICE_FALLBACK_FORBIDDEN",
        ErrorCode::ModelContractMismatch => "MOSS_MODEL_CONTRACT_MISMATCH",
        ErrorCode::ModelLoadFailed => "MOSS_MODEL_LOAD_FAILED",
        ErrorCode::SessionInitFailed => "MOSS_SESSION_INIT_FAILED",
        ErrorCode::AudioContractMismatch => "MOSS_AUDIO_CONTRACT_MISMATCH",
        ErrorCode::AudioHashMismatch => "MOSS_AUDIO_HASH_MISMATCH",
        ErrorCode::AudioNonFinite => "MOSS_AUDIO_NON_FINITE",
        ErrorCode::AudioOutOfRange => "MOSS_AUDIO_OUT_OF_RANGE",
        ErrorCode::NativeRunFailed => "MOSS_NATIVE_RUN_FAILED",
        ErrorCode::NativeOutputInvalid => "MOSS_NATIVE_OUTPUT_INVALID",
        ErrorCode::NativeOutputTruncated => "MOSS_NATIVE_OUTPUT_TRUNCATED",
        ErrorCode::Cancelled => "MOSS_CANCELLED",
        ErrorCode::Internal => "MOSS_INTERNAL",
    }
}

#[tauri::command]
pub async fn moss_helper_transcribe_file(
    state: State<'_, Arc<MossHelperManager>>,
    request: MossTranscribeFileRequest,
) -> Result<ManagedTranscription, MossCommandError> {
    let timeout = Duration::from_millis(request.timeout_ms);
    if timeout < Duration::from_secs(1) || timeout > Duration::from_secs(24 * 60 * 60) {
        return Err(ManagerError::InvalidRequest.into());
    }
    let manager = state.inner().clone();
    let preparation = manager
        .begin_preparation(&request.request_id)
        .map_err(MossCommandError::from)?;
    transcribe_file_with_preparation(manager, request, preparation).await
}

pub(crate) async fn transcribe_file_with_preparation(
    manager: Arc<MossHelperManager>,
    request: MossTranscribeFileRequest,
    preparation: PreparationLease,
) -> Result<ManagedTranscription, MossCommandError> {
    let supervisor_started = Instant::now();
    let timeout = Duration::from_millis(request.timeout_ms);
    if timeout < Duration::from_secs(1) || timeout > Duration::from_secs(24 * 60 * 60) {
        return Err(ManagerError::InvalidRequest.into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let decoded = crate::audio::decoder::decode_audio_file_with_cancel_and_limit(
            &request.source_audio_path,
            MAX_REQUEST_SECONDS,
            || preparation.is_cancelled() || supervisor_started.elapsed() >= timeout,
        )
        .map_err(|error| {
            map_decode_failure(
                error,
                preparation.is_cancelled(),
                supervisor_started.elapsed() >= timeout,
            )
        })?;
        let normalized = decoded
            .to_moss_format_with_cancel(|| {
                preparation.is_cancelled() || supervisor_started.elapsed() >= timeout
            })
            .map_err(|_| {
                if preparation.is_cancelled() {
                    ManagerError::Cancelled.into()
                } else if supervisor_started.elapsed() >= timeout {
                    ManagerError::Timeout.into()
                } else {
                    MossCommandError {
                        code: "MOSS_AUDIO_STAGE_FAILED",
                        message: "MOSS operation failed".to_string(),
                    }
                }
            })?;
        manager
            .transcribe_prepared(
                TranscribeInput {
                    request_id: request.request_id,
                    context_sha256: request.context_sha256,
                    runtime_directory: request.runtime_directory,
                    model_path: request.model_path,
                    model_bytes: request.model_bytes,
                    model_sha256: request.model_sha256,
                    device_id: request.device_id,
                    timeout,
                    // The backend enforces its frozen 480-600 second performance rule;
                    // the WebView cannot disable or relax it.
                    max_wall_rtf: None,
                    samples: normalized,
                    sample_rate_hz: 16_000,
                    channels: 1,
                    language_requested: request.language_requested,
                    decode_parameters_json: request.decode_parameters_json,
                    decode_parameters_sha256: request.decode_parameters_sha256,
                },
                supervisor_started,
                &preparation,
            )
            .map_err(Into::into)
    })
    .await
    .map_err(|_| MossCommandError {
        code: "MOSS_INTERNAL",
        message: "MOSS operation failed".to_string(),
    })?
}

fn map_decode_failure(error: anyhow::Error, cancelled: bool, timed_out: bool) -> MossCommandError {
    if cancelled {
        ManagerError::Cancelled.into()
    } else if timed_out {
        ManagerError::Timeout.into()
    } else if error
        .downcast_ref::<crate::audio::decoder::AudioDurationLimitExceeded>()
        .is_some()
    {
        ManagerError::AudioTooLong.into()
    } else {
        MossCommandError {
            code: "MOSS_AUDIO_DECODE_FAILED",
            message: "MOSS operation failed".to_string(),
        }
    }
}

#[tauri::command]
pub async fn moss_helper_probe(
    state: State<'_, Arc<MossHelperManager>>,
    request: MossProbeRequest,
) -> Result<moss_helper::protocol::ProbeResultMessage, MossCommandError> {
    let manager = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager
            .probe(
                request.runtime_directory,
                request.request_id,
                request.device_id,
                Duration::from_millis(request.timeout_ms),
            )
            .map_err(Into::into)
    })
    .await
    .map_err(|_| MossCommandError {
        code: "MOSS_INTERNAL",
        message: "MOSS operation failed".to_string(),
    })?
}

#[tauri::command]
pub async fn moss_helper_cancel(
    state: State<'_, Arc<MossHelperManager>>,
    request_id: String,
) -> Result<bool, MossCommandError> {
    state.cancel(&request_id).map_err(Into::into)
}

#[tauri::command]
pub async fn moss_helper_status(
    state: State<'_, Arc<MossHelperManager>>,
) -> Result<ManagerStatus, MossCommandError> {
    Ok(state.status())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::moss_helper::windows_job::JobError;

    #[test]
    fn public_errors_keep_specific_model_audio_device_and_output_codes() {
        assert_eq!(
            MossCommandError::from(ManagerError::Native(ErrorCode::ModelContractMismatch)).code,
            "MOSS_MODEL_CONTRACT_MISMATCH"
        );
        assert_eq!(
            MossCommandError::from(ManagerError::Native(ErrorCode::AudioHashMismatch)).code,
            "MOSS_AUDIO_HASH_MISMATCH"
        );
        assert_eq!(
            MossCommandError::from(ManagerError::Native(ErrorCode::DeviceNotFound)).code,
            "MOSS_DEVICE_NOT_FOUND"
        );
        assert_eq!(
            MossCommandError::from(ManagerError::Native(ErrorCode::NativeOutputTruncated)).code,
            "MOSS_NATIVE_OUTPUT_TRUNCATED"
        );
        assert_eq!(
            MossCommandError::from(ManagerError::AudioTooLong).code,
            "MOSS_AUDIO_TOO_LONG"
        );
        assert_eq!(
            map_decode_failure(
                crate::audio::decoder::AudioDurationLimitExceeded.into(),
                false,
                false,
            )
            .code,
            "MOSS_AUDIO_TOO_LONG"
        );
        assert_eq!(
            map_decode_failure(anyhow::anyhow!("private decode detail"), false, false).code,
            "MOSS_AUDIO_DECODE_FAILED"
        );
        assert_eq!(
            map_decode_failure(anyhow::anyhow!("ignored"), true, true).code,
            "MOSS_CANCELLED"
        );
    }

    #[test]
    fn public_spawn_error_keeps_operation_and_code_without_a_private_path() {
        let error = MossCommandError::from(ManagerError::Spawn(JobError::Windows {
            operation: "CreateProcessW",
            code: 87,
            message: "The parameter is incorrect".to_string(),
        }));
        assert_eq!(error.code, "MOSS_HELPER_SPAWN_FAILED");
        assert!(error.message.contains("CreateProcessW"));
        assert!(error.message.contains("87"));
        assert!(!error.message.contains("C:\\"));
        assert!(!error.message.contains("/Users/"));
    }
}
