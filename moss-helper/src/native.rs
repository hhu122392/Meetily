use crate::protocol::{
    DeviceResult, DeviceSpec, ErrorCode, ModelSpec, NativeSessionLimits, NativeTimings, RuntimeSpec,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const EXPECTED_RUNTIME_VERSION: &str = "0.2.2";
pub const EXPECTED_HEADER_HASH: &str = "7df72bf9e667b8c2";
pub const EXPECTED_SOURCE_COMMIT: &str = "c6a9257cdf8e9c6918c0f8f876246db048a22103";
pub const REQUIRED_DEVICE_KIND: &str = "vulkan";
pub const REQUIRED_DEVICE_DESCRIPTION: &str = "Intel(R) Arc(TM) Graphics";
pub const EXPECTED_MODEL_BYTES: u64 = 986_899_616;
pub const EXPECTED_MODEL_SHA256: &str =
    "64EC654DC6FFCFDFE180422DFFCE1D33422B0C30959B7EDFD131BAD77EE35039";
/// The default 131K/32K Vulkan sessions crash while allocating `kv_self_k` on
/// the frozen Intel Arc host. R1 proved this 16K setting on the complete
/// 737.728 second business recording.
pub const MOSS_SESSION_N_CTX: i32 = 16_384;
pub const MOSS_LANGUAGE_REQUESTED: &str = "zh-CN";
pub const MOSS_LANGUAGE_RESOLVED: &str = "zh-CN";
pub const MOSS_DECODE_PARAMETERS_JSON: &str =
    r#"{"language":"zh","timestamps":"segment","diarize":"on"}"#;
const MAX_OUTPUT_SEGMENTS: usize = 100_000;
const TIMESTAMP_GRID_MS: i64 = 50;

const EXPECTED_RUNTIME_FILES: &[(&str, &str)] = &[
    (
        "contract.json",
        "C266622EE16A69C458EBA57FD2B2F2114B9C5F2E34292E9603F1CE46C7A1DC73",
    ),
    (
        "ggml-base.dll",
        "951BD8BC93B9F5327D81BCDA7627144265655ECFE8749D3B7BFCDE9B8AA6D781",
    ),
    (
        "ggml-cpu-alderlake.dll",
        "C03FDAF376FA1B10EF79DF1FDA9ECBB1C75858D7AEC8E4D6C79C1AE883F51D3D",
    ),
    (
        "ggml-cpu-cannonlake.dll",
        "6A6EF755425DE2A1BAC71B8CD5003882D9B7FA10807676948F050012F3F9026C",
    ),
    (
        "ggml-cpu-cascadelake.dll",
        "765D97EC6785D82AF76B023484A8F39C662D2C75BCB45649DE375A251B94F003",
    ),
    (
        "ggml-cpu-haswell.dll",
        "CDE7120183E5A83C1EB8D234916E087635CD4B809695B535BE9E1355C902A8A2",
    ),
    (
        "ggml-cpu-icelake.dll",
        "61FB173672BFE5AA7A8458F478DDF614D4F6BBA57434CF4D45D7724DC3C07F48",
    ),
    (
        "ggml-cpu-sandybridge.dll",
        "31F767685E1D41844EC5F83B09A7A80E2F1861EB23677EC848FB7C458188FB57",
    ),
    (
        "ggml-cpu-skylakex.dll",
        "FC63A42A473D3DA61512191A20B61D8A0D046F03A8FE537BDAFCC01D14D43DC4",
    ),
    (
        "ggml-cpu-sse42.dll",
        "8F2B1314B69103F488FE765BD3EF578FA1C7F4B874FE49BEEC55667BE8246BEE",
    ),
    (
        "ggml-cpu-x64.dll",
        "5D06E975855E72E0CFD8E2C140B9DA4C433422B98CFD40468BD41761EA202E5F",
    ),
    (
        "ggml-vulkan.dll",
        "52FEA282E87471078A2BD795CD5F31E8CF345B2CCDCAC94F4133FBE9CF7A20B6",
    ),
    (
        "ggml.dll",
        "CEA8F409E2E94E9A847BFA962499C384334A31A28EF14B3BF73CA930DB77AE1F",
    ),
    (
        "transcribe.dll",
        "3C19F92A9ACB8480377DAEBD415909887AD770E3A58DBC4BF50FDD2432071E63",
    ),
];

#[derive(Debug, Clone)]
pub struct RuntimeProbe {
    pub runtime_version: String,
    pub runtime_commit: String,
    pub backend: String,
    pub device: DeviceResult,
}

#[derive(Debug, Clone)]
pub struct NativeSegment {
    pub t0_ms: i64,
    pub t1_ms: i64,
    pub speaker_id: i32,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct NativeTranscription {
    pub probe: RuntimeProbe,
    pub raw_text: String,
    pub clean_text: String,
    pub segments: Vec<NativeSegment>,
    pub session_limits: NativeSessionLimits,
    pub timings: NativeTimings,
    pub was_aborted: bool,
    pub was_truncated: bool,
    pub wall_elapsed_ms: u64,
    pub rtf: f64,
    pub language_requested: String,
    pub language_resolved: String,
    pub decode_parameters_json: String,
    pub decode_parameters_sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct MossDecodeParameters {
    language: String,
    timestamps: String,
    diarize: String,
}

#[derive(Debug, Clone)]
struct ValidatedDecodeContract {
    language_requested: String,
    language_resolved: String,
    parameters: MossDecodeParameters,
    parameters_json: String,
    parameters_sha256: String,
}

pub fn moss_decode_parameters_sha256() -> String {
    format!(
        "{:x}",
        Sha256::digest(MOSS_DECODE_PARAMETERS_JSON.as_bytes())
    )
}

fn validate_decode_contract(
    language_requested: &str,
    decode_parameters_json: &str,
    decode_parameters_sha256: &str,
) -> Result<ValidatedDecodeContract, NativeError> {
    let parameters: MossDecodeParameters =
        serde_json::from_str(decode_parameters_json).map_err(|_| invalid_decode_contract())?;
    let canonical_json =
        serde_json::to_string(&parameters).map_err(|_| invalid_decode_contract())?;
    let canonical_sha256 = format!("{:x}", Sha256::digest(canonical_json.as_bytes()));
    if language_requested != MOSS_LANGUAGE_REQUESTED
        || parameters.language != "zh"
        || parameters.timestamps != "segment"
        || parameters.diarize != "on"
        || canonical_json != MOSS_DECODE_PARAMETERS_JSON
        || decode_parameters_json != canonical_json
        || !decode_parameters_sha256.eq_ignore_ascii_case(&canonical_sha256)
    {
        return Err(invalid_decode_contract());
    }
    Ok(ValidatedDecodeContract {
        language_requested: language_requested.to_owned(),
        // This is not a UI locale or detected-language guess. A successful
        // native run used the explicit `zh` pointer below, so its resolved
        // BCP-47 language is deterministically zh-CN.
        language_resolved: MOSS_LANGUAGE_RESOLVED.to_owned(),
        parameters,
        parameters_json: canonical_json,
        parameters_sha256: canonical_sha256,
    })
}

fn invalid_decode_contract() -> NativeError {
    NativeError::new(
        ErrorCode::NativeOutputInvalid,
        "decode_contract",
        "the native decode contract is invalid",
    )
}

fn invalid_native_output() -> NativeError {
    NativeError::new(
        ErrorCode::NativeOutputInvalid,
        "result_validation",
        "the native runtime returned an invalid result",
    )
}

fn validate_native_output(
    text: (&str, &str),
    segments: &[NativeSegment],
    timings: &NativeTimings,
    state_flags: (bool, bool),
    audio_duration_ms: i64,
    rtf: f64,
) -> Result<(), NativeError> {
    let (raw_text, clean_text) = text;
    let (was_aborted, was_truncated) = state_flags;
    if raw_text.trim().is_empty()
        || clean_text.trim().is_empty()
        || segments.is_empty()
        || segments.len() > MAX_OUTPUT_SEGMENTS
        || was_aborted
        || was_truncated
        || audio_duration_ms <= 0
        || !rtf.is_finite()
        || rtf < 0.0
    {
        return Err(invalid_native_output());
    }
    let native_timings = [
        timings.load_ms,
        timings.mel_ms,
        timings.encode_ms,
        timings.decode_ms,
    ];
    if native_timings
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
    {
        return Err(invalid_native_output());
    }

    // MOSS timestamps are emitted on a 50 ms grid, so the last grid point can
    // round up by less than one frame without extending semantic audio time.
    let maximum_timestamp_ms = audio_duration_ms
        .checked_add(TIMESTAMP_GRID_MS - 1)
        .ok_or_else(invalid_native_output)?
        / TIMESTAMP_GRID_MS
        * TIMESTAMP_GRID_MS;
    let mut previous_t0 = 0_i64;
    for segment in segments {
        if segment.t0_ms < 0
            || segment.t1_ms < segment.t0_ms
            || segment.t0_ms < previous_t0
            || segment.t1_ms > maximum_timestamp_ms
            || segment.speaker_id < 0
            || segment.text.trim().is_empty()
        {
            return Err(invalid_native_output());
        }
        previous_t0 = segment.t0_ms;
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct NativeError {
    pub code: ErrorCode,
    pub phase: &'static str,
    pub message: &'static str,
    pub native_status: Option<i32>,
}

impl NativeError {
    fn new(code: ErrorCode, phase: &'static str, message: &'static str) -> Self {
        Self {
            code,
            phase,
            message,
            native_status: None,
        }
    }

    fn status(
        code: ErrorCode,
        phase: &'static str,
        message: &'static str,
        native_status: u32,
    ) -> Self {
        Self {
            code,
            phase,
            message,
            native_status: Some(native_status as i32),
        }
    }
}

pub fn harden_process_environment() -> Result<(), NativeError> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::LibraryLoader::{
            SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_SYSTEM32,
        };

        // SAFETY: this is called before any worker or native DLL load.  Keeping
        // only System32 in the default search removes application-dir, CWD and
        // PATH from all later unqualified LoadLibrary calls (including Vulkan).
        if unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) } == 0 {
            return Err(NativeError::new(
                ErrorCode::RuntimeEnvironmentForbidden,
                "runtime_environment",
                "the process DLL search policy could not be hardened",
            ));
        }
    }
    let mut forbidden_nonempty = false;
    let overrides: Vec<_> = std::env::vars_os()
        .filter_map(|(name, value)| {
            let is_override = name.to_str().map_or(true, |name| {
                let normalized = name.to_ascii_uppercase();
                normalized.starts_with("VK_")
                    || normalized == "VULKAN_SDK"
                    || normalized.starts_with("TRANSCRIBE_")
                    || normalized.starts_with("GGML_")
            });
            if is_override {
                forbidden_nonempty |= !value.is_empty();
                Some(name)
            } else {
                None
            }
        })
        .collect();
    for name in &overrides {
        std::env::remove_var(name);
    }
    if forbidden_nonempty {
        return Err(NativeError::new(
            ErrorCode::RuntimeEnvironmentForbidden,
            "runtime_environment",
            "a native inference environment override is forbidden",
        ));
    }
    Ok(())
}

pub fn validate_device_policy(device: &DeviceSpec) -> Result<(), NativeError> {
    if device.kind != REQUIRED_DEVICE_KIND
        || device.description != REQUIRED_DEVICE_DESCRIPTION
        || device.allow_primary_fallback
    {
        return Err(NativeError::new(
            ErrorCode::DevicePolicyRejected,
            "device_policy",
            "the request does not match the frozen Intel Arc Vulkan policy",
        ));
    }
    Ok(())
}

pub fn validate_model_contract(model: &ModelSpec) -> Result<(), NativeError> {
    if model.bytes != EXPECTED_MODEL_BYTES
        || !model.sha256.eq_ignore_ascii_case(EXPECTED_MODEL_SHA256)
    {
        return Err(NativeError::new(
            ErrorCode::ModelContractMismatch,
            "model_contract",
            "the model does not match the frozen MOSS contract",
        ));
    }
    Ok(())
}

#[cfg(windows)]
mod imp {
    use std::collections::{BTreeMap, BTreeSet};
    use std::ffi::{c_char, c_void, CStr, CString, OsString};
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom};
    use std::mem::{align_of, size_of};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::fs::FileExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::path::{Path, PathBuf};
    use std::ptr::null_mut;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    use libloading::os::windows::Library;
    use serde::Deserialize;
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::pcm::{validate_pcm, MAX_AUDIO_SECONDS};
    use crate::protocol::TranscribeCommand;

    const LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR: u32 = 0x0000_0100;
    const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x0000_0800;
    const TRANSCRIBE_OK: u32 = 0;
    const TRANSCRIBE_ERR_ABORTED: u32 = 13;
    const TRANSCRIBE_ERR_OUTPUT_TRUNCATED: u32 = 18;
    const TRANSCRIBE_BACKEND_VULKAN: u32 = 3;
    const TRANSCRIBE_TIMESTAMPS_SEGMENT: u32 = 2;
    const TRANSCRIBE_DIARIZE_MODE_ON: u32 = 2;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct DeviceInfo {
        struct_size: u64,
        name: *const c_char,
        description: *const c_char,
        kind: *const c_char,
        device_id: *const c_char,
        memory_total: u64,
        memory_free: u64,
        device_type: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ModelLoadParams {
        struct_size: u64,
        backend: u32,
        device: *mut c_void,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SessionParams {
        struct_size: u64,
        n_threads: i32,
        kv_type: u32,
        n_ctx: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SessionLimits {
        struct_size: u64,
        effective_n_ctx: i32,
        effective_max_audio_ms: i64,
        max_kv_bytes: i64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct RunParams {
        struct_size: u64,
        task: u32,
        timestamps: u32,
        pnc: u32,
        itn: u32,
        diarize: u32,
        language: *const c_char,
        target_language: *const c_char,
        keep_special_tags: bool,
        family: *const c_void,
        spec_k_drafts: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Timings {
        struct_size: u64,
        load_ms: f32,
        mel_ms: f32,
        encode_ms: f32,
        decode_ms: f32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Segment {
        struct_size: u64,
        t0_ms: i64,
        t1_ms: i64,
        first_word: i32,
        n_words: i32,
        first_token: i32,
        n_tokens: i32,
        text: *const c_char,
        speaker_id: i32,
    }

    type FnVersion = unsafe extern "C" fn() -> *const c_char;
    type FnAbi = unsafe extern "C" fn(u32) -> usize;
    type LogCallback = unsafe extern "C" fn(u32, *const c_char, *mut c_void);
    type FnLogSet = unsafe extern "C" fn(Option<LogCallback>, *mut c_void);
    type FnInitBackends = unsafe extern "C" fn(*const c_char) -> u32;
    type FnDeviceCount = unsafe extern "C" fn() -> i32;
    type FnDeviceGet = unsafe extern "C" fn(i32) -> *mut c_void;
    type FnDeviceInfoInit = unsafe extern "C" fn(*mut DeviceInfo);
    type FnDeviceGetInfo = unsafe extern "C" fn(*mut c_void, *mut DeviceInfo) -> u32;
    type FnBackendAvailable = unsafe extern "C" fn(u32) -> bool;
    type FnModelLoadParamsInit = unsafe extern "C" fn(*mut ModelLoadParams);
    type FnModelLoad =
        unsafe extern "C" fn(*const c_char, *const ModelLoadParams, *mut *mut c_void) -> u32;
    type FnModelFree = unsafe extern "C" fn(*mut c_void);
    type FnModelBackend = unsafe extern "C" fn(*const c_void) -> *const c_char;
    type FnModelDevice = unsafe extern "C" fn(*const c_void) -> *mut c_void;
    type FnSessionParamsInit = unsafe extern "C" fn(*mut SessionParams);
    type FnSessionInit =
        unsafe extern "C" fn(*mut c_void, *const SessionParams, *mut *mut c_void) -> u32;
    type FnSessionFree = unsafe extern "C" fn(*mut c_void);
    type FnSessionLimitsInit = unsafe extern "C" fn(*mut SessionLimits);
    type FnSessionGetLimits = unsafe extern "C" fn(*const c_void, *mut SessionLimits) -> u32;
    type FnRunParamsInit = unsafe extern "C" fn(*mut RunParams);
    type FnRun = unsafe extern "C" fn(*mut c_void, *const f32, i32, *const RunParams) -> u32;
    type AbortCallback = unsafe extern "C" fn(*mut c_void) -> bool;
    type FnSetAbort = unsafe extern "C" fn(*mut c_void, Option<AbortCallback>, *mut c_void);
    type FnBoolSession = unsafe extern "C" fn(*const c_void) -> bool;
    type FnText = unsafe extern "C" fn(*const c_void) -> *const c_char;
    type FnCount = unsafe extern "C" fn(*const c_void) -> i32;
    type FnSegmentInit = unsafe extern "C" fn(*mut Segment);
    type FnGetSegment = unsafe extern "C" fn(*const c_void, i32, *mut Segment) -> u32;
    type FnTimingsInit = unsafe extern "C" fn(*mut Timings);
    type FnGetTimings = unsafe extern "C" fn(*const c_void, *mut Timings) -> u32;

    struct Functions {
        version: FnVersion,
        version_commit: FnVersion,
        abi_size: FnAbi,
        abi_align: FnAbi,
        log_set: FnLogSet,
        init_backends: FnInitBackends,
        device_count: FnDeviceCount,
        device_get: FnDeviceGet,
        device_info_init: FnDeviceInfoInit,
        device_get_info: FnDeviceGetInfo,
        backend_available: FnBackendAvailable,
        model_load_params_init: FnModelLoadParamsInit,
        model_load: FnModelLoad,
        model_free: FnModelFree,
        model_backend: FnModelBackend,
        model_device: FnModelDevice,
        session_params_init: FnSessionParamsInit,
        session_init: FnSessionInit,
        session_free: FnSessionFree,
        session_limits_init: FnSessionLimitsInit,
        session_get_limits: FnSessionGetLimits,
        run_params_init: FnRunParamsInit,
        run: FnRun,
        set_abort: FnSetAbort,
        was_aborted: FnBoolSession,
        was_truncated: FnBoolSession,
        full_text: FnText,
        raw_text: FnText,
        n_segments: FnCount,
        segment_init: FnSegmentInit,
        get_segment: FnGetSegment,
        timings_init: FnTimingsInit,
        get_timings: FnGetTimings,
    }

    struct LoadedRuntime {
        _library: Library,
        runtime: ValidatedRuntime,
        functions: Functions,
        version: String,
        commit: String,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RuntimeContract {
        version: String,
        header_hash: String,
        backends: Vec<String>,
        lane: String,
    }

    struct SelectedDevice {
        handle: *mut c_void,
        backend_name: String,
        result: DeviceResult,
    }

    struct ModelGuard {
        value: *mut c_void,
        free: FnModelFree,
        _lock: File,
    }

    struct SecureOpenedPath {
        file: File,
        final_path: PathBuf,
        volume_serial: u32,
        file_index: u64,
        is_directory: bool,
    }

    struct ValidatedRuntime {
        directory: SecureOpenedPath,
        files: BTreeMap<String, SecureOpenedPath>,
    }

    impl ValidatedRuntime {
        fn file(&self, name: &str) -> Result<&SecureOpenedPath, NativeError> {
            self.files.get(name).ok_or_else(|| {
                NativeError::new(
                    ErrorCode::RuntimeContractMissing,
                    "runtime_contract",
                    "a frozen runtime file is unavailable",
                )
            })
        }
    }

    impl Drop for ModelGuard {
        fn drop(&mut self) {
            // SAFETY: value is owned by this guard and freed exactly once.
            unsafe { (self.free)(self.value) };
        }
    }

    struct SessionGuard {
        value: *mut c_void,
        free: FnSessionFree,
        limits: NativeSessionLimits,
    }

    impl Drop for SessionGuard {
        fn drop(&mut self) {
            // SAFETY: value is owned by this guard and freed exactly once.
            unsafe { (self.free)(self.value) };
        }
    }

    pub fn probe_runtime(
        runtime: &RuntimeSpec,
        device: &DeviceSpec,
    ) -> Result<RuntimeProbe, NativeError> {
        validate_device_policy(device)?;
        let loaded = LoadedRuntime::open(runtime)?;
        let selected = loaded.initialize_and_select(device)?;
        Ok(RuntimeProbe {
            runtime_version: loaded.version.clone(),
            runtime_commit: loaded.commit.clone(),
            backend: REQUIRED_DEVICE_KIND.to_string(),
            device: selected.result,
        })
    }

    pub fn transcribe(
        command: &TranscribeCommand,
        cancel: &AtomicBool,
    ) -> Result<NativeTranscription, NativeError> {
        validate_device_policy(&command.device)?;
        validate_model_contract(&command.model)?;
        let decode_contract = validate_decode_contract(
            &command.language_requested,
            &command.decode_parameters_json,
            &command.decode_parameters_sha256,
        )?;
        let pcm = validate_pcm(&command.audio).map_err(|error| NativeError {
            code: error.code(),
            phase: "audio_validation",
            message: "audio validation failed",
            native_status: None,
        })?;
        if cancel.load(Ordering::SeqCst) {
            return Err(NativeError::new(
                ErrorCode::Cancelled,
                "preflight",
                "transcription was cancelled",
            ));
        }

        let loaded = LoadedRuntime::open(&command.runtime)?;
        let selected = loaded.initialize_and_select(&command.device)?;
        let model = loaded.load_model(&command.model, selected.handle, cancel)?;
        let actual_backend = loaded.model_backend(&model)?;
        // SAFETY: model is live and owns a process-lifetime device handle.
        let actual_device = unsafe { (loaded.functions.model_device)(model.value) };
        if !model_binding_matches(
            &actual_backend,
            actual_device,
            &selected.backend_name,
            selected.handle,
        ) {
            return Err(NativeError::new(
                ErrorCode::DeviceFallbackForbidden,
                "model_binding",
                "the model did not remain bound to the required Vulkan device",
            ));
        }
        let session = loaded.create_session(&model)?;
        let cancel_ptr = (cancel as *const AtomicBool).cast_mut().cast::<c_void>();
        // SAFETY: cancel lives through this synchronous run and the callback only performs an atomic load.
        unsafe {
            (loaded.functions.set_abort)(session.value, Some(abort_callback), cancel_ptr);
        }

        let mut run_params: RunParams = unsafe { std::mem::zeroed() };
        // SAFETY: ABI was verified before this init writes to the struct.
        unsafe { (loaded.functions.run_params_init)(&mut run_params) };
        run_params.timestamps = TRANSCRIBE_TIMESTAMPS_SEGMENT;
        run_params.diarize = TRANSCRIBE_DIARIZE_MODE_ON;
        let native_language = CString::new(decode_contract.parameters.language.as_str())
            .map_err(|_| invalid_decode_contract())?;
        run_params.language = native_language.as_ptr();

        let started = Instant::now();
        // SAFETY: PCM is an aligned, live read-only f32 mapping; session and params are live.
        let status = unsafe {
            (loaded.functions.run)(
                session.value,
                pcm.as_ptr(),
                pcm.samples() as i32,
                &run_params,
            )
        };
        let wall_elapsed = started.elapsed();
        validate_run_status(status)?;

        let raw_text = loaded.copy_text(unsafe { (loaded.functions.raw_text)(session.value) })?;
        let clean_text =
            loaded.copy_text(unsafe { (loaded.functions.full_text)(session.value) })?;
        let segments = loaded.copy_segments(&session)?;
        let timings = loaded.copy_timings(&session)?;
        let was_aborted = unsafe { (loaded.functions.was_aborted)(session.value) };
        let was_truncated = unsafe { (loaded.functions.was_truncated)(session.value) };
        let duration = pcm.duration_seconds();
        let rtf = if duration > 0.0 {
            wall_elapsed.as_secs_f64() / duration
        } else {
            0.0
        };

        validate_native_output(
            (&raw_text, &clean_text),
            &segments,
            &timings,
            (was_aborted, was_truncated),
            (duration * 1_000.0).round() as i64,
            rtf,
        )?;

        Ok(NativeTranscription {
            probe: RuntimeProbe {
                runtime_version: loaded.version.clone(),
                runtime_commit: loaded.commit.clone(),
                backend: actual_backend,
                device: selected.result,
            },
            raw_text,
            clean_text,
            segments,
            session_limits: session.limits,
            timings,
            was_aborted,
            was_truncated,
            wall_elapsed_ms: wall_elapsed.as_millis().try_into().unwrap_or(u64::MAX),
            rtf,
            language_requested: decode_contract.language_requested,
            language_resolved: decode_contract.language_resolved,
            decode_parameters_json: decode_contract.parameters_json,
            decode_parameters_sha256: decode_contract.parameters_sha256,
        })
    }

    impl LoadedRuntime {
        fn open(runtime: &RuntimeSpec) -> Result<Self, NativeError> {
            if std::env::var_os("GGML_BACKEND_PATH").is_some() {
                return Err(NativeError::new(
                    ErrorCode::RuntimeEnvironmentForbidden,
                    "runtime_environment",
                    "an external backend override is forbidden",
                ));
            }
            let requested_directory = PathBuf::from(&runtime.directory);
            if !requested_directory.is_absolute() {
                return Err(NativeError::new(
                    ErrorCode::RuntimeContractMissing,
                    "runtime_contract",
                    "the frozen runtime directory is unavailable",
                ));
            }
            let validated_runtime = validate_runtime(&requested_directory)?;
            let dll_path = validated_runtime.file("transcribe.dll")?.final_path.clone();
            // SAFETY: the path and all runtime hashes were verified, and restricted search flags
            // prevent current-directory/PATH dependency resolution.
            let library = unsafe {
                Library::load_with_flags(
                    &dll_path,
                    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
                )
            }
            .map_err(|_| {
                NativeError::new(
                    ErrorCode::RuntimeLoadFailed,
                    "runtime_load",
                    "the frozen native runtime could not be loaded",
                )
            })?;
            verify_core_module_paths(&validated_runtime)?;
            // SAFETY: every function pointer is resolved from the verified v0.2.2 DLL.
            let functions = unsafe { Functions::load(&library)? };
            let version = copy_static_cstr(unsafe { (functions.version)() })?;
            if version != EXPECTED_RUNTIME_VERSION {
                return Err(NativeError::new(
                    ErrorCode::RuntimeVersionMismatch,
                    "runtime_version",
                    "the native runtime version does not match",
                ));
            }
            let commit = copy_static_cstr(unsafe { (functions.version_commit)() })?;
            if commit == "unknown"
                || commit.len() < 7
                || !EXPECTED_SOURCE_COMMIT.starts_with(&commit)
            {
                return Err(NativeError::new(
                    ErrorCode::RuntimeCommitMismatch,
                    "runtime_commit",
                    "the native runtime commit does not match",
                ));
            }
            verify_abi(&functions)?;
            // SAFETY: installed once before backend initialization or worker creation.
            unsafe { (functions.log_set)(Some(redacted_native_log), null_mut()) };
            Ok(Self {
                _library: library,
                runtime: validated_runtime,
                functions,
                version,
                commit,
            })
        }

        fn initialize_and_select(
            &self,
            policy: &DeviceSpec,
        ) -> Result<SelectedDevice, NativeError> {
            let runtime_utf8 = self.runtime.directory.final_path.to_str().ok_or_else(|| {
                NativeError::new(
                    ErrorCode::RuntimeContractMismatch,
                    "backend_init",
                    "the runtime directory is not valid UTF-8",
                )
            })?;
            let runtime_c = CString::new(runtime_utf8).map_err(|_| {
                NativeError::new(
                    ErrorCode::RuntimeContractMismatch,
                    "backend_init",
                    "the runtime directory is invalid",
                )
            })?;
            // SAFETY: runtime_c remains live for the call.
            let status = unsafe { (self.functions.init_backends)(runtime_c.as_ptr()) };
            if status != TRANSCRIBE_OK {
                return Err(NativeError::status(
                    ErrorCode::BackendInitFailed,
                    "backend_init",
                    "native backend initialization failed",
                    status,
                ));
            }
            verify_backend_module_paths(&self.runtime)?;
            // SAFETY: backend initialization completed before this availability query.
            if !unsafe { (self.functions.backend_available)(TRANSCRIBE_BACKEND_VULKAN) } {
                return Err(NativeError::new(
                    ErrorCode::DeviceNotFound,
                    "device_selection",
                    "the required Vulkan backend is unavailable",
                ));
            }
            self.select_device(policy)
        }

        fn select_device(&self, policy: &DeviceSpec) -> Result<SelectedDevice, NativeError> {
            let mut matches = Vec::new();
            // SAFETY: the device registry is initialized and no mutation races this query.
            let count = unsafe { (self.functions.device_count)() }.max(0);
            for index in 0..count {
                // SAFETY: the index is in the enumerated range.
                let handle = unsafe { (self.functions.device_get)(index) };
                if handle.is_null() {
                    continue;
                }
                let mut raw: DeviceInfo = unsafe { std::mem::zeroed() };
                // SAFETY: ABI was checked and raw is caller-owned storage.
                unsafe { (self.functions.device_info_init)(&mut raw) };
                let status = unsafe { (self.functions.device_get_info)(handle, &mut raw) };
                if status != TRANSCRIBE_OK {
                    continue;
                }
                let kind = copy_static_cstr(raw.kind)?;
                let backend_name = copy_static_cstr(raw.name)?;
                let description = copy_static_cstr(raw.description)?;
                let device_id = copy_optional_cstr(raw.device_id)?;
                let id_matches = match policy.device_id.as_ref() {
                    Some(expected) => device_id.as_ref() == Some(expected),
                    None => true,
                };
                if kind == policy.kind && description == policy.description && id_matches {
                    matches.push(SelectedDevice {
                        handle,
                        backend_name,
                        result: DeviceResult {
                            kind,
                            description,
                            device_id,
                            device_type: device_type_name(raw.device_type).to_string(),
                            memory_total_bytes: raw.memory_total,
                            memory_free_bytes: raw.memory_free,
                        },
                    });
                }
            }
            match matches.len() {
                0 => Err(NativeError::new(
                    ErrorCode::DeviceNotFound,
                    "device_selection",
                    "the exact Intel Arc Vulkan device was not found",
                )),
                1 => Ok(matches.remove(0)),
                _ => Err(NativeError::new(
                    ErrorCode::DeviceAmbiguous,
                    "device_selection",
                    "more than one device matched the exact policy",
                )),
            }
        }

        fn load_model(
            &self,
            spec: &ModelSpec,
            device: *mut c_void,
            cancel: &AtomicBool,
        ) -> Result<ModelGuard, NativeError> {
            let path = Path::new(&spec.path);
            let locked = validate_model_file(path, spec, cancel)?;
            if cancel.load(Ordering::SeqCst) {
                return Err(NativeError::new(
                    ErrorCode::Cancelled,
                    "model_validation",
                    "transcription was cancelled",
                ));
            }
            let path_utf8 = locked.final_path.to_str().ok_or_else(|| {
                NativeError::new(
                    ErrorCode::ModelContractMismatch,
                    "model_contract",
                    "the model path is not valid UTF-8",
                )
            })?;
            let path_c = CString::new(path_utf8).map_err(|_| {
                NativeError::new(
                    ErrorCode::ModelContractMismatch,
                    "model_contract",
                    "the model path is invalid",
                )
            })?;
            let mut params: ModelLoadParams = unsafe { std::mem::zeroed() };
            unsafe { (self.functions.model_load_params_init)(&mut params) };
            params.backend = TRANSCRIBE_BACKEND_VULKAN;
            params.device = device;
            let mut model = null_mut();
            let status =
                unsafe { (self.functions.model_load)(path_c.as_ptr(), &params, &mut model) };
            if status != TRANSCRIBE_OK || model.is_null() {
                return Err(NativeError::status(
                    ErrorCode::ModelLoadFailed,
                    "model_load",
                    "the frozen MOSS model could not be loaded",
                    status,
                ));
            }
            Ok(ModelGuard {
                value: model,
                free: self.functions.model_free,
                _lock: locked.file,
            })
        }

        fn model_backend(&self, model: &ModelGuard) -> Result<String, NativeError> {
            copy_static_cstr(unsafe { (self.functions.model_backend)(model.value) })
        }

        fn create_session(&self, model: &ModelGuard) -> Result<SessionGuard, NativeError> {
            let mut params: SessionParams = unsafe { std::mem::zeroed() };
            unsafe { (self.functions.session_params_init)(&mut params) };
            params.n_ctx = MOSS_SESSION_N_CTX;
            let mut session = null_mut();
            let status =
                unsafe { (self.functions.session_init)(model.value, &params, &mut session) };
            if status != TRANSCRIBE_OK || session.is_null() {
                return Err(NativeError::status(
                    ErrorCode::SessionInitFailed,
                    "session_init",
                    "the native transcription session could not be created",
                    status,
                ));
            }
            let mut raw_limits: SessionLimits = unsafe { std::mem::zeroed() };
            unsafe { (self.functions.session_limits_init)(&mut raw_limits) };
            let limits_status = unsafe {
                (self.functions.session_get_limits)(session.cast_const(), &mut raw_limits)
            };
            let minimum_audio_ms = i64::try_from(MAX_AUDIO_SECONDS)
                .ok()
                .and_then(|seconds| seconds.checked_mul(1_000))
                .ok_or_else(|| {
                    NativeError::new(
                        ErrorCode::SessionInitFailed,
                        "session_limits",
                        "the configured MOSS audio limit is invalid",
                    )
                })?;
            if limits_status != TRANSCRIBE_OK
                || raw_limits.effective_n_ctx != MOSS_SESSION_N_CTX
                || raw_limits.effective_max_audio_ms < minimum_audio_ms
                || raw_limits.max_kv_bytes <= 0
            {
                // SessionGuard has not been built yet, so free this successfully
                // created native session before returning the failed admission.
                unsafe { (self.functions.session_free)(session) };
                return Err(NativeError::status(
                    ErrorCode::SessionInitFailed,
                    "session_limits",
                    "the native session does not satisfy the frozen Vulkan-16K limits",
                    limits_status,
                ));
            }
            Ok(SessionGuard {
                value: session,
                free: self.functions.session_free,
                limits: NativeSessionLimits {
                    effective_n_ctx: raw_limits.effective_n_ctx,
                    effective_max_audio_ms: raw_limits.effective_max_audio_ms,
                    max_kv_bytes: raw_limits.max_kv_bytes,
                },
            })
        }

        fn copy_text(&self, value: *const c_char) -> Result<String, NativeError> {
            copy_static_cstr(value)
        }

        fn copy_segments(&self, session: &SessionGuard) -> Result<Vec<NativeSegment>, NativeError> {
            let count = unsafe { (self.functions.n_segments)(session.value) };
            if count <= 0 || count as usize > MAX_OUTPUT_SEGMENTS {
                return Err(invalid_native_output());
            }
            let mut segments = Vec::with_capacity(count as usize);
            for index in 0..count {
                let mut raw: Segment = unsafe { std::mem::zeroed() };
                unsafe { (self.functions.segment_init)(&mut raw) };
                let status =
                    unsafe { (self.functions.get_segment)(session.value, index, &mut raw) };
                if status != TRANSCRIBE_OK {
                    return Err(NativeError::status(
                        ErrorCode::NativeRunFailed,
                        "result_copy",
                        "a native segment could not be copied",
                        status,
                    ));
                }
                segments.push(NativeSegment {
                    t0_ms: raw.t0_ms,
                    t1_ms: raw.t1_ms,
                    speaker_id: raw.speaker_id,
                    text: copy_static_cstr(raw.text)?,
                });
            }
            Ok(segments)
        }

        fn copy_timings(&self, session: &SessionGuard) -> Result<NativeTimings, NativeError> {
            let mut raw: Timings = unsafe { std::mem::zeroed() };
            unsafe { (self.functions.timings_init)(&mut raw) };
            let status = unsafe { (self.functions.get_timings)(session.value, &mut raw) };
            if status != TRANSCRIBE_OK {
                return Err(NativeError::status(
                    ErrorCode::NativeRunFailed,
                    "result_copy",
                    "native timings could not be copied",
                    status,
                ));
            }
            Ok(NativeTimings {
                load_ms: raw.load_ms,
                mel_ms: raw.mel_ms,
                encode_ms: raw.encode_ms,
                decode_ms: raw.decode_ms,
            })
        }
    }

    impl Functions {
        unsafe fn load(library: &Library) -> Result<Self, NativeError> {
            macro_rules! symbol {
                ($name:literal, $ty:ty) => {{
                    *library
                        .get::<$ty>(concat!($name, "\0").as_bytes())
                        .map_err(|_| {
                            NativeError::new(
                                ErrorCode::RuntimeSymbolMissing,
                                "runtime_symbols",
                                concat!("required native symbol is missing: ", $name),
                            )
                        })?
                }};
            }
            Ok(Self {
                version: symbol!("transcribe_version", FnVersion),
                version_commit: symbol!("transcribe_version_commit", FnVersion),
                abi_size: symbol!("transcribe_abi_struct_size", FnAbi),
                abi_align: symbol!("transcribe_abi_struct_align", FnAbi),
                log_set: symbol!("transcribe_log_set", FnLogSet),
                init_backends: symbol!("transcribe_init_backends", FnInitBackends),
                device_count: symbol!("transcribe_device_count", FnDeviceCount),
                device_get: symbol!("transcribe_device_get", FnDeviceGet),
                device_info_init: symbol!("transcribe_device_info_init", FnDeviceInfoInit),
                device_get_info: symbol!("transcribe_device_get_info", FnDeviceGetInfo),
                backend_available: symbol!("transcribe_backend_available", FnBackendAvailable),
                model_load_params_init: symbol!(
                    "transcribe_model_load_params_init",
                    FnModelLoadParamsInit
                ),
                model_load: symbol!("transcribe_model_load_file", FnModelLoad),
                model_free: symbol!("transcribe_model_free", FnModelFree),
                model_backend: symbol!("transcribe_model_backend", FnModelBackend),
                model_device: symbol!("transcribe_model_device", FnModelDevice),
                session_params_init: symbol!("transcribe_session_params_init", FnSessionParamsInit),
                session_init: symbol!("transcribe_session_init", FnSessionInit),
                session_free: symbol!("transcribe_session_free", FnSessionFree),
                session_limits_init: symbol!("transcribe_session_limits_init", FnSessionLimitsInit),
                session_get_limits: symbol!("transcribe_session_get_limits", FnSessionGetLimits),
                run_params_init: symbol!("transcribe_run_params_init", FnRunParamsInit),
                run: symbol!("transcribe_run", FnRun),
                set_abort: symbol!("transcribe_set_abort_callback", FnSetAbort),
                was_aborted: symbol!("transcribe_was_aborted", FnBoolSession),
                was_truncated: symbol!("transcribe_was_truncated", FnBoolSession),
                full_text: symbol!("transcribe_full_text", FnText),
                raw_text: symbol!("transcribe_raw_text", FnText),
                n_segments: symbol!("transcribe_n_segments", FnCount),
                segment_init: symbol!("transcribe_segment_init", FnSegmentInit),
                get_segment: symbol!("transcribe_get_segment", FnGetSegment),
                timings_init: symbol!("transcribe_timings_init", FnTimingsInit),
                get_timings: symbol!("transcribe_get_timings", FnGetTimings),
            })
        }
    }

    fn validate_runtime(directory: &Path) -> Result<ValidatedRuntime, NativeError> {
        let directory = secure_open_path(directory, true).map_err(|_| {
            NativeError::new(
                ErrorCode::RuntimeContractMissing,
                "runtime_contract",
                "the frozen runtime directory is unavailable",
            )
        })?;
        let mut files = BTreeMap::new();
        for (name, expected_hash) in EXPECTED_RUNTIME_FILES {
            let path = directory.final_path.join(name);
            let opened = secure_open_path(&path, false).map_err(|_| {
                NativeError::new(
                    ErrorCode::RuntimeContractMissing,
                    "runtime_contract",
                    "a frozen runtime file is unavailable",
                )
            })?;
            if opened.volume_serial != directory.volume_serial
                || !same_path(
                    opened.final_path.parent(),
                    Some(directory.final_path.as_path()),
                )
            {
                return Err(NativeError::new(
                    ErrorCode::RuntimeContractMismatch,
                    "runtime_contract",
                    "a frozen runtime file escaped its frozen directory",
                ));
            }
            let actual_hash = hash_locked_file(&opened.file, None).map_err(|_| {
                NativeError::new(
                    ErrorCode::RuntimeContractMissing,
                    "runtime_contract",
                    "a frozen runtime file is unavailable",
                )
            })?;
            if !actual_hash.eq_ignore_ascii_case(expected_hash) {
                return Err(NativeError::new(
                    ErrorCode::RuntimeHashMismatch,
                    "runtime_contract",
                    "a frozen runtime file hash does not match",
                ));
            }
            files.insert((*name).to_string(), opened);
        }
        let expected_dlls = EXPECTED_RUNTIME_FILES
            .iter()
            .filter_map(|(name, _)| name.ends_with(".dll").then_some(name.to_ascii_lowercase()))
            .collect::<BTreeSet<_>>();
        let entries = std::fs::read_dir(&directory.final_path).map_err(|_| {
            NativeError::new(
                ErrorCode::RuntimeContractMissing,
                "runtime_contract",
                "the frozen runtime directory is unavailable",
            )
        })?;
        let mut actual_dlls = BTreeSet::new();
        for entry in entries {
            let entry = entry.map_err(|_| {
                NativeError::new(
                    ErrorCode::RuntimeContractMismatch,
                    "runtime_contract",
                    "the frozen runtime directory cannot be enumerated",
                )
            })?;
            let name = entry.file_name().into_string().map_err(|_| {
                NativeError::new(
                    ErrorCode::RuntimeContractMismatch,
                    "runtime_contract",
                    "the frozen runtime directory contains an invalid file name",
                )
            })?;
            let name = name.to_ascii_lowercase();
            if name.ends_with(".dll") {
                actual_dlls.insert(name);
            }
        }
        if actual_dlls != expected_dlls {
            return Err(NativeError::new(
                ErrorCode::RuntimeContractMismatch,
                "runtime_contract",
                "the frozen runtime directory contains an unexpected DLL set",
            ));
        }
        let contract_file = files.get("contract.json").ok_or_else(|| {
            NativeError::new(
                ErrorCode::RuntimeContractMissing,
                "runtime_contract",
                "the frozen runtime contract is unavailable",
            )
        })?;
        let mut contract_reader = contract_file.file.try_clone().map_err(|_| {
            NativeError::new(
                ErrorCode::RuntimeContractMissing,
                "runtime_contract",
                "the frozen runtime contract cannot be read",
            )
        })?;
        contract_reader.seek(SeekFrom::Start(0)).map_err(|_| {
            NativeError::new(
                ErrorCode::RuntimeContractMissing,
                "runtime_contract",
                "the frozen runtime contract cannot be read",
            )
        })?;
        let mut bytes = Vec::new();
        contract_reader.read_to_end(&mut bytes).map_err(|_| {
            NativeError::new(
                ErrorCode::RuntimeContractMissing,
                "runtime_contract",
                "the frozen runtime contract cannot be read",
            )
        })?;
        let contract: RuntimeContract = serde_json::from_slice(&bytes).map_err(|_| {
            NativeError::new(
                ErrorCode::RuntimeContractMismatch,
                "runtime_contract",
                "the frozen runtime contract is invalid",
            )
        })?;
        if contract.version != EXPECTED_RUNTIME_VERSION
            || contract.header_hash != EXPECTED_HEADER_HASH
            || contract.lane != "cpu-vulkan"
            || contract.backends != ["vulkan", "cpu"]
        {
            return Err(NativeError::new(
                ErrorCode::RuntimeContractMismatch,
                "runtime_contract",
                "the frozen runtime contract does not match",
            ));
        }
        Ok(ValidatedRuntime { directory, files })
    }

    fn validate_model_file(
        path: &Path,
        spec: &ModelSpec,
        cancel: &AtomicBool,
    ) -> Result<SecureOpenedPath, NativeError> {
        if !path.is_absolute() {
            return Err(NativeError::new(
                ErrorCode::ModelContractMismatch,
                "model_contract",
                "the model path must be absolute",
            ));
        }
        let opened = secure_open_path(path, false).map_err(|_| {
            NativeError::new(
                ErrorCode::ModelContractMismatch,
                "model_contract",
                "the frozen model is unavailable",
            )
        })?;
        let hash = hash_locked_file(&opened.file, Some(cancel)).map_err(|error| {
            if error.kind() == std::io::ErrorKind::Interrupted {
                NativeError::new(
                    ErrorCode::Cancelled,
                    "model_validation",
                    "transcription was cancelled",
                )
            } else {
                NativeError::new(
                    ErrorCode::ModelContractMismatch,
                    "model_contract",
                    "the frozen model is unavailable",
                )
            }
        })?;
        let actual_bytes = opened.file.metadata().map(|meta| meta.len()).map_err(|_| {
            NativeError::new(
                ErrorCode::ModelContractMismatch,
                "model_contract",
                "the frozen model metadata is unavailable",
            )
        })?;
        if actual_bytes != spec.bytes || !hash.eq_ignore_ascii_case(&spec.sha256) {
            return Err(NativeError::new(
                ErrorCode::ModelContractMismatch,
                "model_contract",
                "the frozen model bytes or hash do not match",
            ));
        }
        Ok(opened)
    }

    fn secure_open_path(path: &Path, expect_directory: bool) -> std::io::Result<SecureOpenedPath> {
        use windows_sys::Win32::Foundation::{GENERIC_READ, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{
            CreateFileW, FileAttributeTagInfo, GetFileInformationByHandle,
            GetFileInformationByHandleEx, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY,
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, OPEN_EXISTING,
        };

        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let flags = FILE_FLAG_OPEN_REPARSE_POINT
            | if expect_directory {
                FILE_FLAG_BACKUP_SEMANTICS
            } else {
                0
            };
        // SAFETY: wide is a live NUL-terminated path and no security-template pointers are used.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ,
                null_mut(),
                OPEN_EXISTING,
                flags,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: CreateFileW returned an owned handle and this transfers it to File exactly once.
        let file = unsafe { File::from_raw_handle(handle.cast()) };
        let mut tag: FILE_ATTRIBUTE_TAG_INFO = unsafe { std::mem::zeroed() };
        // SAFETY: tag is correctly sized caller-owned output storage.
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle().cast(),
                FileAttributeTagInfo,
                (&mut tag as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let is_directory = tag.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
        if tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || is_directory != expect_directory
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the opened object type is forbidden",
            ));
        }
        let mut identity: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: identity is correctly sized caller-owned output storage.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), &mut identity) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let final_path = final_path_from_handle(&file, path)?;
        let final_wide: Vec<u16> = final_path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // Reopen the normalized name without following a leaf reparse point and prove that it
        // names the same object.  AppContainer denies DOS-volume path queries, so the normalized
        // volume-relative name is combined with the caller's drive and then identity-checked.
        let final_handle = unsafe {
            CreateFileW(
                final_wide.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ,
                null_mut(),
                OPEN_EXISTING,
                flags,
                std::ptr::null_mut(),
            )
        };
        if final_handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        let final_file = unsafe { File::from_raw_handle(final_handle.cast()) };
        let mut final_tag: FILE_ATTRIBUTE_TAG_INFO = unsafe { std::mem::zeroed() };
        if unsafe {
            GetFileInformationByHandleEx(
                final_file.as_raw_handle().cast(),
                FileAttributeTagInfo,
                (&mut final_tag as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let mut final_identity: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe {
            GetFileInformationByHandle(final_file.as_raw_handle().cast(), &mut final_identity)
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let final_is_directory = final_tag.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
        let original_index =
            ((identity.nFileIndexHigh as u64) << 32) | identity.nFileIndexLow as u64;
        let final_index =
            ((final_identity.nFileIndexHigh as u64) << 32) | final_identity.nFileIndexLow as u64;
        if final_tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || final_is_directory != expect_directory
            || final_identity.dwVolumeSerialNumber != identity.dwVolumeSerialNumber
            || final_index != original_index
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the normalized path did not preserve object identity",
            ));
        }
        Ok(SecureOpenedPath {
            file,
            final_path,
            volume_serial: identity.dwVolumeSerialNumber,
            file_index: original_index,
            is_directory,
        })
    }

    fn final_path_from_handle(file: &File, opened_path: &Path) -> std::io::Result<PathBuf> {
        use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;

        const VOLUME_NAME_NONE: u32 = 0x0000_0004;
        let needed = unsafe {
            GetFinalPathNameByHandleW(file.as_raw_handle().cast(), null_mut(), 0, VOLUME_NAME_NONE)
        };
        if needed == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut buffer = vec![0u16; needed as usize + 1];
        // SAFETY: buffer is writable for the size advertised by the first call.
        let written = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle().cast(),
                buffer.as_mut_ptr(),
                buffer.len().try_into().unwrap_or(u32::MAX),
                VOLUME_NAME_NONE,
            )
        } as usize;
        if written == 0 || written >= buffer.len() {
            return Err(std::io::Error::last_os_error());
        }
        buffer.truncate(written);
        let relative = OsString::from_wide(&buffer);
        let relative = relative.to_str().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid normalized path")
        })?;
        if !relative.starts_with('\\') {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "normalized path is not volume-relative",
            ));
        }
        let opened = opened_path.to_str().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid opened path")
        })?;
        let bytes = opened.as_bytes();
        if bytes.len() < 3
            || !bytes[0].is_ascii_alphabetic()
            || bytes[1] != b':'
            || !matches!(bytes[2], b'\\' | b'/')
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "opened path is not a local absolute drive path",
            ));
        }
        Ok(PathBuf::from(format!("{}:{relative}", bytes[0] as char)))
    }

    fn same_path(left: Option<&Path>, right: Option<&Path>) -> bool {
        match (left, right) {
            (Some(left), Some(right)) => match (left.to_str(), right.to_str()) {
                (Some(left), Some(right)) => left.eq_ignore_ascii_case(right),
                _ => false,
            },
            _ => false,
        }
    }

    fn same_identity(left: &SecureOpenedPath, right: &SecureOpenedPath) -> bool {
        left.volume_serial == right.volume_serial
            && left.file_index == right.file_index
            && left.is_directory == right.is_directory
    }

    fn verify_core_module_paths(runtime: &ValidatedRuntime) -> Result<(), NativeError> {
        for name in ["transcribe.dll", "ggml.dll", "ggml-base.dll"] {
            verify_loaded_module_path(name, runtime.file(name)?)?;
        }
        let system32 = system_directory()?;
        for name in ["MSVCP140.dll", "VCRUNTIME140.dll", "VCRUNTIME140_1.dll"] {
            let expected =
                secure_open_path(&system32.join(name), false).map_err(|_| module_path_error())?;
            verify_loaded_module_path(name, &expected)?;
        }
        Ok(())
    }

    fn verify_backend_module_paths(runtime: &ValidatedRuntime) -> Result<(), NativeError> {
        verify_loaded_module_path("ggml-vulkan.dll", runtime.file("ggml-vulkan.dll")?)?;
        let expected = secure_open_path(&system_directory()?.join("vulkan-1.dll"), false)
            .map_err(|_| module_path_error())?;
        verify_loaded_module_path("vulkan-1.dll", &expected)
    }

    fn verify_loaded_module_path(
        name: &str,
        expected: &SecureOpenedPath,
    ) -> Result<(), NativeError> {
        use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};

        let wide_name: Vec<u16> = std::ffi::OsStr::new(name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: wide_name is a live NUL-terminated module base name.
        let module = unsafe { GetModuleHandleW(wide_name.as_ptr()) };
        if module.is_null() {
            return Err(module_path_error());
        }
        let mut buffer = vec![0_u16; 32_768];
        // SAFETY: module is loaded and buffer is writable for its declared size.
        let length = unsafe {
            GetModuleFileNameW(
                module,
                buffer.as_mut_ptr(),
                buffer.len().try_into().unwrap_or(u32::MAX),
            )
        } as usize;
        if length == 0 || length >= buffer.len() {
            return Err(module_path_error());
        }
        buffer.truncate(length);
        let actual = secure_open_path(&PathBuf::from(OsString::from_wide(&buffer)), false)
            .map_err(|_| module_path_error())?;
        if !same_identity(&actual, expected) {
            return Err(module_path_error());
        }
        Ok(())
    }

    fn system_directory() -> Result<PathBuf, NativeError> {
        use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

        let mut buffer = vec![0_u16; 32_768];
        // SAFETY: buffer is writable for its declared size.
        let length = unsafe {
            GetSystemDirectoryW(
                buffer.as_mut_ptr(),
                buffer.len().try_into().unwrap_or(u32::MAX),
            )
        } as usize;
        if length == 0 || length >= buffer.len() {
            return Err(module_path_error());
        }
        buffer.truncate(length);
        Ok(PathBuf::from(OsString::from_wide(&buffer)))
    }

    fn module_path_error() -> NativeError {
        NativeError::new(
            ErrorCode::RuntimeLoadFailed,
            "runtime_module_path",
            "a loaded native module did not come from its frozen location",
        )
    }

    fn hash_locked_file(file: &File, cancel: Option<&AtomicBool>) -> std::io::Result<String> {
        let mut digest = Sha256::new();
        let mut buffer = [0_u8; 1024 * 1024];
        let mut offset = 0u64;
        loop {
            if cancel.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "cancelled",
                ));
            }
            let read = file.seek_read(&mut buffer, offset)?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
            offset = offset
                .checked_add(read as u64)
                .ok_or_else(|| std::io::Error::other("file offset overflow"))?;
        }
        Ok(format!("{:X}", digest.finalize()))
    }

    fn verify_abi(functions: &Functions) -> Result<(), NativeError> {
        let expected = [
            (
                0,
                size_of::<ModelLoadParams>(),
                align_of::<ModelLoadParams>(),
            ),
            (1, size_of::<SessionParams>(), align_of::<SessionParams>()),
            (11, size_of::<SessionLimits>(), align_of::<SessionLimits>()),
            (2, size_of::<RunParams>(), align_of::<RunParams>()),
            (5, size_of::<Timings>(), align_of::<Timings>()),
            (6, size_of::<Segment>(), align_of::<Segment>()),
            (13, size_of::<DeviceInfo>(), align_of::<DeviceInfo>()),
        ];
        for (kind, size, align) in expected {
            // SAFETY: the functions are pure ABI metadata queries.
            if unsafe { (functions.abi_size)(kind) } != size
                || unsafe { (functions.abi_align)(kind) } != align
            {
                return Err(NativeError::new(
                    ErrorCode::RuntimeAbiMismatch,
                    "runtime_abi",
                    "the native ABI layout does not match the helper",
                ));
            }
        }
        Ok(())
    }

    fn copy_static_cstr(value: *const c_char) -> Result<String, NativeError> {
        if value.is_null() {
            return Err(NativeError::new(
                ErrorCode::Internal,
                "native_string",
                "the native runtime returned an invalid string",
            ));
        }
        // SAFETY: native API contracts these pointers as live NUL-terminated strings.
        let value = unsafe { CStr::from_ptr(value) };
        value.to_str().map(str::to_owned).map_err(|_| {
            NativeError::new(
                ErrorCode::Internal,
                "native_string",
                "the native runtime returned invalid UTF-8",
            )
        })
    }

    fn copy_optional_cstr(value: *const c_char) -> Result<Option<String>, NativeError> {
        if value.is_null() {
            Ok(None)
        } else {
            copy_static_cstr(value).map(Some)
        }
    }

    fn device_type_name(value: u32) -> &'static str {
        match value {
            0 => "cpu",
            1 => "gpu",
            2 => "igpu",
            3 => "accel",
            _ => "unknown",
        }
    }

    fn model_binding_matches(
        actual_backend: &str,
        actual_device: *mut c_void,
        selected_backend_name: &str,
        selected_device: *mut c_void,
    ) -> bool {
        actual_backend == selected_backend_name
            && actual_device == selected_device
            && !actual_device.is_null()
    }

    fn validate_run_status(status: u32) -> Result<(), NativeError> {
        match status {
            TRANSCRIBE_OK => Ok(()),
            TRANSCRIBE_ERR_ABORTED => Err(NativeError::status(
                ErrorCode::Cancelled,
                "native_run",
                "transcription was cancelled",
                status,
            )),
            TRANSCRIBE_ERR_OUTPUT_TRUNCATED => Err(NativeError::status(
                ErrorCode::NativeOutputTruncated,
                "native_run",
                "native output was truncated",
                status,
            )),
            _ => Err(NativeError::status(
                ErrorCode::NativeRunFailed,
                "native_run",
                "native transcription failed",
                status,
            )),
        }
    }

    unsafe extern "C" fn redacted_native_log(
        _level: u32,
        _message: *const c_char,
        _userdata: *mut c_void,
    ) {
        // Intentionally discard native strings: they may contain local paths or model text.
    }

    unsafe extern "C" fn abort_callback(userdata: *mut c_void) -> bool {
        if userdata.is_null() {
            return false;
        }
        // SAFETY: userdata points to an AtomicBool that outlives the synchronous native run.
        unsafe { &*(userdata.cast::<AtomicBool>()) }.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    mod binding_tests {
        use super::*;

        #[test]
        fn model_binding_requires_exact_enumerated_backend_name_and_handle() {
            let selected = 0x10usize as *mut c_void;
            assert!(model_binding_matches(
                "Vulkan0", selected, "Vulkan0", selected
            ));
            assert!(!model_binding_matches(
                "vulkan", selected, "Vulkan0", selected
            ));
            assert!(!model_binding_matches(
                "Vulkan0-extra",
                selected,
                "Vulkan0",
                selected
            ));
            assert!(!model_binding_matches(
                "Vulkan0",
                0x20usize as *mut c_void,
                "Vulkan0",
                selected
            ));
            assert!(!model_binding_matches(
                "Vulkan0",
                std::ptr::null_mut(),
                "Vulkan0",
                std::ptr::null_mut()
            ));
        }

        #[test]
        fn secure_handle_path_freezes_identity_type_and_rejects_junctions() {
            let root = tempfile::tempdir().unwrap();
            let directory = root.path().join("冻结 目录");
            std::fs::create_dir(&directory).unwrap();
            let file_path = directory.join("model.gguf");
            std::fs::write(&file_path, b"frozen").unwrap();

            let first = secure_open_path(&file_path, false).unwrap();
            let second = secure_open_path(&first.final_path, false).unwrap();
            assert!(same_identity(&first, &second));
            assert!(!first.is_directory);
            assert!(secure_open_path(&file_path, true).is_err());
            assert!(secure_open_path(&directory, false).is_err());

            let junction = root.path().join("junction");
            let status = std::process::Command::new("cmd.exe")
                .args(["/d", "/c", "mklink", "/J"])
                .arg(&junction)
                .arg(&directory)
                .status()
                .unwrap();
            assert!(status.success());
            assert!(secure_open_path(&junction, true).is_err());
        }

        #[test]
        fn native_output_truncated_status_maps_to_stable_terminal_code() {
            let error = validate_run_status(TRANSCRIBE_ERR_OUTPUT_TRUNCATED).unwrap_err();
            assert_eq!(error.code, ErrorCode::NativeOutputTruncated);
            assert_eq!(error.phase, "native_run");
            assert_eq!(error.native_status, Some(18));
        }

        #[test]
        #[ignore = "requires MOSS_TEST_RUNTIME_DIR"]
        fn real_secure_runtime_handle_contract_is_valid() {
            let directory = PathBuf::from(std::env::var_os("MOSS_TEST_RUNTIME_DIR").unwrap());
            if let Err(error) = validate_runtime(&directory) {
                panic!("{}:{}", error.phase, error.message);
            }
        }
    }
}

#[cfg(windows)]
pub use imp::{probe_runtime, transcribe};

#[cfg(not(windows))]
pub fn probe_runtime(
    _runtime: &RuntimeSpec,
    _device: &DeviceSpec,
) -> Result<RuntimeProbe, NativeError> {
    Err(NativeError::new(
        ErrorCode::RuntimeUnsupportedPlatform,
        "runtime_load",
        "the frozen MOSS runtime is only available on Windows",
    ))
}

#[cfg(not(windows))]
pub fn transcribe(
    _command: &crate::protocol::TranscribeCommand,
    _cancel: &std::sync::atomic::AtomicBool,
) -> Result<NativeTranscription, NativeError> {
    Err(NativeError::new(
        ErrorCode::RuntimeUnsupportedPlatform,
        "runtime_load",
        "the frozen MOSS runtime is only available on Windows",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> DeviceSpec {
        DeviceSpec {
            kind: REQUIRED_DEVICE_KIND.to_string(),
            description: REQUIRED_DEVICE_DESCRIPTION.to_string(),
            device_id: None,
            allow_primary_fallback: false,
        }
    }

    #[test]
    fn exact_device_policy_is_required() {
        validate_device_policy(&device()).unwrap();
        let mut wrong = device();
        wrong.kind = "cpu".to_string();
        assert_eq!(
            validate_device_policy(&wrong).unwrap_err().code,
            ErrorCode::DevicePolicyRejected
        );
        let mut fallback = device();
        fallback.allow_primary_fallback = true;
        assert_eq!(
            validate_device_policy(&fallback).unwrap_err().code,
            ErrorCode::DevicePolicyRejected
        );
    }

    #[test]
    fn native_decode_contract_requires_explicit_zh_cn_and_exact_hash() {
        let expected_hash = moss_decode_parameters_sha256();
        let contract = validate_decode_contract(
            MOSS_LANGUAGE_REQUESTED,
            MOSS_DECODE_PARAMETERS_JSON,
            &expected_hash,
        )
        .unwrap();
        assert_eq!(contract.language_resolved, MOSS_LANGUAGE_RESOLVED);
        assert_eq!(contract.parameters.language, "zh");

        for (language, parameters, hash) in [
            ("auto", MOSS_DECODE_PARAMETERS_JSON, expected_hash.as_str()),
            ("en-US", MOSS_DECODE_PARAMETERS_JSON, expected_hash.as_str()),
            (MOSS_LANGUAGE_REQUESTED, MOSS_DECODE_PARAMETERS_JSON, "0"),
        ] {
            assert_eq!(
                validate_decode_contract(language, parameters, hash)
                    .unwrap_err()
                    .phase,
                "decode_contract"
            );
        }
    }

    #[test]
    fn frozen_model_contract_is_required() {
        let valid = ModelSpec {
            path: "model.gguf".to_string(),
            bytes: EXPECTED_MODEL_BYTES,
            sha256: EXPECTED_MODEL_SHA256.to_lowercase(),
        };
        validate_model_contract(&valid).unwrap();
        let mut wrong = valid;
        wrong.bytes -= 1;
        assert_eq!(
            validate_model_contract(&wrong).unwrap_err().code,
            ErrorCode::ModelContractMismatch
        );
    }

    fn valid_output() -> (String, String, Vec<NativeSegment>, NativeTimings) {
        (
            "raw".to_string(),
            "clean".to_string(),
            vec![NativeSegment {
                t0_ms: 0,
                t1_ms: 1_000,
                speaker_id: 1,
                text: "hello".to_string(),
            }],
            NativeTimings {
                load_ms: 1.0,
                mel_ms: 2.0,
                encode_ms: 3.0,
                decode_ms: 4.0,
            },
        )
    }

    #[test]
    fn native_result_validation_rejects_empty_abort_truncate_and_nonfinite_values() {
        let (raw, clean, segments, timings) = valid_output();
        assert!(validate_native_output(
            (&raw, &clean),
            &segments,
            &timings,
            (false, false),
            1_000,
            0.5
        )
        .is_ok());
        for (candidate_raw, candidate_clean, aborted, truncated, rtf) in [
            ("", "clean", false, false, 0.5),
            ("raw", "", false, false, 0.5),
            ("raw", "clean", true, false, 0.5),
            ("raw", "clean", false, true, 0.5),
            ("raw", "clean", false, false, f64::NAN),
            ("raw", "clean", false, false, f64::INFINITY),
        ] {
            assert_eq!(
                validate_native_output(
                    (candidate_raw, candidate_clean),
                    &segments,
                    &timings,
                    (aborted, truncated),
                    1_000,
                    rtf,
                )
                .unwrap_err()
                .code,
                ErrorCode::NativeOutputInvalid
            );
        }
        let mut bad_timing = timings;
        bad_timing.decode_ms = f32::NAN;
        assert!(validate_native_output(
            (&raw, &clean),
            &segments,
            &bad_timing,
            (false, false),
            1_000,
            0.5,
        )
        .is_err());
    }

    #[test]
    fn native_result_validation_rejects_bad_segments() {
        let (raw, clean, segments, timings) = valid_output();
        let invalid = [
            NativeSegment {
                t0_ms: -1,
                ..segments[0].clone()
            },
            NativeSegment {
                t0_ms: 900,
                t1_ms: 800,
                ..segments[0].clone()
            },
            NativeSegment {
                t1_ms: 1_001,
                ..segments[0].clone()
            },
            NativeSegment {
                speaker_id: -1,
                ..segments[0].clone()
            },
            NativeSegment {
                text: " ".to_string(),
                ..segments[0].clone()
            },
        ];
        for segment in invalid {
            assert!(validate_native_output(
                (&raw, &clean),
                &[segment],
                &timings,
                (false, false),
                1_000,
                0.5,
            )
            .is_err());
        }
        let overlapping_speakers = vec![
            NativeSegment {
                t0_ms: 0,
                t1_ms: 1_000,
                speaker_id: 1,
                text: "first".to_string(),
            },
            NativeSegment {
                t0_ms: 500,
                t1_ms: 900,
                speaker_id: 2,
                text: "overlap".to_string(),
            },
        ];
        assert!(validate_native_output(
            (&raw, &clean),
            &overlapping_speakers,
            &timings,
            (false, false),
            2_000,
            0.5,
        )
        .is_ok());
        let backwards = vec![
            NativeSegment {
                t0_ms: 500,
                t1_ms: 1_000,
                speaker_id: 1,
                text: "first".to_string(),
            },
            NativeSegment {
                t0_ms: 400,
                t1_ms: 900,
                speaker_id: 2,
                text: "backwards".to_string(),
            },
        ];
        assert!(validate_native_output(
            (&raw, &clean),
            &backwards,
            &timings,
            (false, false),
            2_000,
            0.5,
        )
        .is_err());
    }

    #[cfg(windows)]
    #[test]
    fn incomplete_runtime_is_rejected_before_dll_load() {
        let directory = tempfile::tempdir().unwrap();
        harden_process_environment().unwrap();
        let error = probe_runtime(
            &RuntimeSpec {
                directory: directory.path().to_str().unwrap().to_owned(),
            },
            &device(),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::RuntimeContractMissing);
        assert_eq!(error.phase, "runtime_contract");
    }
}
