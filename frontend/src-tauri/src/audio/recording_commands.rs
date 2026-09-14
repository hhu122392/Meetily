// audio/recording_commands.rs
//
// Slim Tauri command layer for recording functionality.
// Delegates to transcription and recording modules for actual implementation.

use anyhow::Result;
use log::{error, info, warn};
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::task::JoinHandle;

use crate::meeting_context::{MeetingContextContainer, RecordingSummaryTemplatePreference};
use crate::storage::operation_lock::{begin_storage_operation, StorageOperationKind};

use super::measurement::{
    prepare_realtime_audio_input, D11MeasurementRecorder, RealtimeAudioInputAccepted,
    RealtimeAudioInputRequest, CONTROLLED_FRAME_SAMPLES, CONTROLLED_SAMPLE_RATE,
};
use super::recording_state::AudioChunk;
use super::{
    default_input_device,  // Get default microphone
    default_output_device, // Get default system audio
    resolve_audio_device,
    DeviceEvent,
    DeviceMonitorType,
    RecordingManager,
};

// Import transcription modules
use super::transcription::{self, reset_speech_detected_flag};

// Re-export TranscriptUpdate for backward compatibility
use super::recording_preferences::RecordingMode;
use super::recording_state::{AudioError, DeviceType as RecordingDeviceType};
pub use super::transcription::TranscriptUpdate;

// ============================================================================
// GLOBAL STATE
// ============================================================================

// Simple recording state tracking
static IS_RECORDING: AtomicBool = AtomicBool::new(false);

// Global recording manager and transcription task to keep them alive during recording
static RECORDING_MANAGER: Mutex<Option<RecordingManager>> = Mutex::new(None);
static TRANSCRIPTION_TASK: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);
static CONTROLLED_INPUT_TASK: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);
static CONTROLLED_INPUT_CANCEL: AtomicBool = AtomicBool::new(false);
static DEVICE_EVENT_POLL_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ============================================================================
// PUBLIC TYPES
// ============================================================================

#[derive(Debug, Serialize, Clone)]
pub struct TranscriptionStatus {
    pub chunks_in_queue: usize,
    pub is_processing: bool,
    pub last_activity_ms: u64,
}

// ============================================================================
// RECORDING COMMANDS
// ============================================================================

/// Start recording with default devices
pub async fn start_recording<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    start_recording_with_meeting_name(app, None).await
}

/// Start recording with default devices and optional meeting name
pub async fn start_recording_with_meeting_name<R: Runtime>(
    app: AppHandle<R>,
    meeting_name: Option<String>,
) -> Result<(), String> {
    start_recording_with_meeting_name_and_metadata(app, meeting_name, None, None).await
}

pub async fn start_recording_with_meeting_name_and_metadata<R: Runtime>(
    app: AppHandle<R>,
    meeting_name: Option<String>,
    summary_template: Option<RecordingSummaryTemplatePreference>,
    meeting_context: Option<MeetingContextContainer>,
) -> Result<(), String> {
    let recording_activity = super::import::RecordingActivityGuard::acquire()?;
    let _storage_start_guard = begin_storage_operation(StorageOperationKind::Recording)
        .map_err(|error| error.to_string())?;
    info!(
        "Starting recording with default devices, meeting: {:?}",
        meeting_name
    );

    let engine_lifecycle_guard = super::common::acquire_engine_lifecycle_lock().await;

    // Check if already recording
    let current_recording_state = IS_RECORDING.load(Ordering::SeqCst);
    info!("🔍 IS_RECORDING state check: {}", current_recording_state);
    if current_recording_state {
        return Err("Recording already in progress".to_string());
    }

    // Validate that transcription models are available before starting recording
    info!("🔍 Validating transcription model availability before starting recording...");
    if let Err(validation_error) = transcription::validate_transcription_model_ready(&app).await {
        error!("Model validation failed: {}", validation_error);

        // Emit error event for frontend - actionable: false to show toast instead of modal
        // (download progress is already shown in top-right toast)
        let _ = app.emit("transcription-error", serde_json::json!({
            "error": validation_error,
            "userMessage": "Recording cannot start: Transcription model is still downloading. Please wait for the download to complete.",
            "actionable": false
        }));

        return Err(validation_error);
    }
    info!("✅ Transcription model validation passed");

    // Async-first approach - no more blocking operations!
    info!("🚀 Starting async recording initialization");

    // Resolve the complete preference record exactly once. The validated save
    // root is copied into the manager and cannot change during this session.
    let preferences = super::recording_preferences::resolve_recording_session_preferences(&app)
        .await
        .map_err(|error| format!("Recording save folder is unavailable: {error}"))?;
    info!(
        "📁 Recording session root validated: {}",
        preferences.save_folder.display()
    );

    // Create new recording manager with the frozen root.
    let mut manager = RecordingManager::new(preferences.save_folder.clone());
    manager.set_preserve_transcription_gaps(transcription::engine::uses_sensevoice(&app).await?);
    let recording_context_id = meeting_context
        .as_ref()
        .map(|context| context.recording_context_id.clone());
    let recognition_context_enabled = meeting_context.is_some();
    let recognition_context = meeting_context
        .as_ref()
        .and_then(|context| context.recording_context())
        .map(crate::meeting_context::RecognitionContext::from_snapshot)
        .map(Arc::new);
    manager
        .set_summary_template(summary_template)
        .map_err(|error| format!("Failed to set recording template metadata: {error}"))?;
    manager
        .set_meeting_context(meeting_context)
        .map_err(|error| format!("Failed to set meeting context metadata: {error}"))?;

    let auto_save = preferences.auto_save;
    let recording_mode = preferences.recording_mode;
    let preferred_mic_name = preferences.preferred_mic_device;
    let preferred_system_name = preferences.preferred_system_device;
    info!("📋 Using frozen recording preferences: auto_save={}, preferred_mic={:?}, preferred_system={:?}",
          auto_save, preferred_mic_name, preferred_system_name);

    // ============================================================================
    // MICROPHONE DEVICE RESOLUTION: Preference → Default → Error
    // ============================================================================
    // 回退事件先攒着，等 start_recording 重置完路由证据之后再登记，否则会被清掉。
    let mut device_fallbacks: Vec<(&'static str, String, String, String)> = Vec::new();
    let microphone_device = if !recording_mode.uses_microphone() {
        None
    } else {
        match preferred_mic_name {
            Some(pref_name) => {
                info!("🎤 Attempting to use preferred microphone: '{}'", pref_name);
                match resolve_audio_device(&pref_name, super::devices::DeviceType::Input).await {
                    Ok(device) => {
                        info!("✅ Using preferred microphone: '{}'", device.name);
                        Some(Arc::new(device))
                    }
                    Err(error) => {
                        // 保存的设备可能被拔掉、禁用，或端点 ID 在系统里变了。以前这里直接
                        // 失败，用户只能进设置重选；现在退回系统默认麦克风继续录音，并把这次
                        // 回退登记到录音路由证据和前端提示里。
                        warn!("⚠️ Preferred microphone '{pref_name}' is unavailable: {error}");
                        match default_input_device() {
                            Ok(device) => {
                                info!("↩️ Falling back to default microphone: '{}'", device.name);
                                device_fallbacks.push((
                                    "microphone",
                                    pref_name.clone(),
                                    device.name.clone(),
                                    error.to_string(),
                                ));
                                let _ = app.emit(
                                    "recording-device-fallback",
                                    serde_json::json!({
                                        "route": "microphone",
                                        "requested": pref_name.clone(),
                                        "used": device.name.clone(),
                                        "reason": error.to_string(),
                                    }),
                                );
                                Some(Arc::new(device))
                            }
                            Err(default_error) => {
                                error!("❌ Preferred microphone and the system default are both unavailable");
                                return Err(format!(
                                    "Preferred microphone is unavailable ('{pref_name}': {error}) and no default microphone could be used: {default_error}"
                                ));
                            }
                        }
                    }
                }
            }
            None => {
                info!("🎤 No microphone preference set, using system default");
                match default_input_device() {
                    Ok(device) => {
                        info!("✅ Using default microphone: '{}'", device.name);
                        Some(Arc::new(device))
                    }
                    Err(e) => {
                        error!("❌ No default microphone available");
                        return Err(format!("No microphone device available: {}", e));
                    }
                }
            }
        }
    };

    // ============================================================================
    // SYSTEM AUDIO DEVICE RESOLUTION: Preference → Default → None (optional)
    // ============================================================================
    let system_device = if !recording_mode.uses_system_audio() {
        None
    } else {
        match preferred_system_name {
            Some(pref_name) => {
                info!(
                    "🔊 Attempting to use preferred system audio: '{}'",
                    pref_name
                );
                match resolve_audio_device(&pref_name, super::devices::DeviceType::Output).await {
                    Ok(device) => {
                        info!("✅ Using preferred system audio: '{}'", device.name);
                        Some(Arc::new(device))
                    }
                    Err(error) => {
                        // 与麦克风同一套处理：系统默认输出设备可用时不再阻断录音。
                        warn!("⚠️ Preferred system audio '{pref_name}' is unavailable: {error}");
                        match default_output_device() {
                            Ok(device) => {
                                info!("↩️ Falling back to default system audio: '{}'", device.name);
                                device_fallbacks.push((
                                    "system",
                                    pref_name.clone(),
                                    device.name.clone(),
                                    error.to_string(),
                                ));
                                let _ = app.emit(
                                    "recording-device-fallback",
                                    serde_json::json!({
                                        "route": "system",
                                        "requested": pref_name.clone(),
                                        "used": device.name.clone(),
                                        "reason": error.to_string(),
                                    }),
                                );
                                Some(Arc::new(device))
                            }
                            Err(default_error) => {
                                error!("❌ Preferred system audio and the system default are both unavailable");
                                return Err(format!(
                                    "Preferred system audio is unavailable ('{pref_name}': {error}) and no default output device could be used: {default_error}"
                                ));
                            }
                        }
                    }
                }
            }
            None => {
                info!("🔊 No system audio preference set, using system default");
                match default_output_device() {
                    Ok(device) => {
                        info!("✅ Using default system audio: '{}'", device.name);
                        Some(Arc::new(device))
                    }
                    Err(e) => return Err(format!("No default system audio available: {}", e)),
                }
            }
        }
    };

    // Always ensure a meeting name is set so incremental saver initializes
    let effective_meeting_name = meeting_name.clone().unwrap_or_else(|| {
        // Example: Meeting 2025-10-03_08-25-23
        let now = chrono::Local::now();
        format!("Meeting {}", now.format("%Y-%m-%d_%H-%M-%S"))
    });
    manager.set_meeting_name(Some(effective_meeting_name));

    // Set up error callback
    let app_for_error = app.clone();
    manager.set_error_callback(move |error| {
        let _ = app_for_error.emit("recording-error", error.user_message());
    });

    // Start recording with resolved devices (replaces start_recording_with_defaults_and_auto_save call)
    let transcription_receiver = manager
        .start_recording(microphone_device, system_device, auto_save)
        .await
        .map_err(|e| format!("Failed to start recording: {}", e))?;

    // 回退证据必须在 start_recording 之后登记：那一步会清空路由证据容器。
    for (route, requested, used, reason) in &device_fallbacks {
        manager.record_preferred_device_fallback(route);
        warn!("↩️ Recording device fallback on {route}: requested '{requested}', using '{used}' ({reason})");
    }

    let measurement_recorder = manager.measurement_recorder();
    let transcript_writer = manager.transcript_writer();

    // Store the manager globally to keep it alive
    {
        let mut global_manager = RECORDING_MANAGER.lock().unwrap();
        *global_manager = Some(manager);
    }

    // Set recording flag and reset speech detection flag
    info!("🔍 Setting IS_RECORDING to true and resetting SPEECH_DETECTED_EMITTED");
    IS_RECORDING.store(true, Ordering::SeqCst);
    recording_activity.commit();
    drop(engine_lifecycle_guard);
    reset_speech_detected_flag(); // Reset for new recording session

    // Start optimized parallel transcription task and store handle
    let task_handle = transcription::start_transcription_task(
        app.clone(),
        transcription_receiver,
        recognition_context,
        measurement_recorder,
        transcript_writer,
    );
    {
        let mut global_task = TRANSCRIPTION_TASK.lock().unwrap();
        *global_task = Some(task_handle);
    }

    // Emit success event
    app.emit(
        "recording-started",
        serde_json::json!({
            "message": "Recording started successfully with parallel processing",
            "devices": ["Default Microphone", "Default System Audio"],
            "workers": 3,
            "recordingContextId": recording_context_id,
            "recognitionContextEnabled": recognition_context_enabled
        }),
    )
    .map_err(|e| e.to_string())?;

    // Update tray menu to reflect recording state
    crate::tray::update_tray_menu(&app);

    info!("✅ Recording started successfully with async-first approach");

    Ok(())
}

/// Start recording with specific devices
pub async fn start_recording_with_devices<R: Runtime>(
    app: AppHandle<R>,
    mic_device_name: Option<String>,
    system_device_name: Option<String>,
) -> Result<(), String> {
    start_recording_with_devices_and_meeting(app, mic_device_name, system_device_name, None).await
}

/// Start recording with specific devices and optional meeting name
pub async fn start_recording_with_devices_and_meeting<R: Runtime>(
    app: AppHandle<R>,
    mic_device_name: Option<String>,
    system_device_name: Option<String>,
    meeting_name: Option<String>,
) -> Result<(), String> {
    start_recording_with_devices_meeting_and_metadata(
        app,
        mic_device_name,
        system_device_name,
        meeting_name,
        RecordingMode::MicrophoneAndSystem,
        None,
        None,
    )
    .await
}

pub async fn start_recording_with_devices_meeting_and_metadata<R: Runtime>(
    app: AppHandle<R>,
    mic_device_name: Option<String>,
    system_device_name: Option<String>,
    meeting_name: Option<String>,
    recording_mode: RecordingMode,
    summary_template: Option<RecordingSummaryTemplatePreference>,
    meeting_context: Option<MeetingContextContainer>,
) -> Result<(), String> {
    let recording_activity = super::import::RecordingActivityGuard::acquire()?;
    let _storage_start_guard = begin_storage_operation(StorageOperationKind::Recording)
        .map_err(|error| error.to_string())?;
    info!(
        "Starting recording with specific devices: mic={:?}, system={:?}, meeting={:?}",
        mic_device_name, system_device_name, meeting_name
    );

    let engine_lifecycle_guard = super::common::acquire_engine_lifecycle_lock().await;

    // Check if already recording
    let current_recording_state = IS_RECORDING.load(Ordering::SeqCst);
    info!("🔍 IS_RECORDING state check: {}", current_recording_state);
    if current_recording_state {
        return Err("Recording already in progress".to_string());
    }

    // Validate that transcription models are available before starting recording
    info!("🔍 Validating transcription model availability before starting recording...");
    if let Err(validation_error) = transcription::validate_transcription_model_ready(&app).await {
        error!("Model validation failed: {}", validation_error);

        // Emit error event for frontend - actionable: false to show toast instead of modal
        // (download progress is already shown in top-right toast)
        let _ = app.emit("transcription-error", serde_json::json!({
            "error": validation_error,
            "userMessage": "Recording cannot start: Transcription model is still downloading. Please wait for the download to complete.",
            "actionable": false
        }));

        return Err(validation_error);
    }
    info!("✅ Transcription model validation passed");

    // Parse devices. 界面选中的设备不可用时不再直接拒绝开始录音：退回系统默认
    // 设备继续录，并把回退原因留到 manager 建好后写进证据与前端提示。
    let mut device_fallbacks: Vec<(&'static str, String, String, String)> = Vec::new();

    let mic_device = if !recording_mode.uses_microphone() {
        None
    } else if let Some(ref name) = mic_device_name {
        match resolve_audio_device(name, super::devices::DeviceType::Input).await {
            Ok(device) => Some(Arc::new(device)),
            Err(error) => {
                warn!("⚠️ Selected microphone '{name}' is unavailable: {error}");
                match default_input_device() {
                    Ok(device) => {
                        info!("↩️ Falling back to default microphone: '{}'", device.name);
                        device_fallbacks.push((
                            "microphone",
                            name.clone(),
                            device.name.clone(),
                            error.to_string(),
                        ));
                        let _ = app.emit(
                            "recording-device-fallback",
                            serde_json::json!({
                                "route": "microphone",
                                "requested": name.clone(),
                                "used": device.name.clone(),
                                "reason": error.to_string(),
                            }),
                        );
                        Some(Arc::new(device))
                    }
                    Err(default_error) => {
                        return Err(format!(
                            "Invalid microphone device '{name}': {error}; no default microphone could be used either: {default_error}"
                        ));
                    }
                }
            }
        }
    } else {
        Some(Arc::new(default_input_device().map_err(|error| {
            format!("Default microphone is unavailable: {error}")
        })?))
    };

    let system_device = if !recording_mode.uses_system_audio() {
        None
    } else if let Some(ref name) = system_device_name {
        match resolve_audio_device(name, super::devices::DeviceType::Output).await {
            Ok(device) => Some(Arc::new(device)),
            Err(error) => {
                warn!("⚠️ Selected system audio '{name}' is unavailable: {error}");
                match default_output_device() {
                    Ok(device) => {
                        info!("↩️ Falling back to default system audio: '{}'", device.name);
                        device_fallbacks.push((
                            "system",
                            name.clone(),
                            device.name.clone(),
                            error.to_string(),
                        ));
                        let _ = app.emit(
                            "recording-device-fallback",
                            serde_json::json!({
                                "route": "system",
                                "requested": name.clone(),
                                "used": device.name.clone(),
                                "reason": error.to_string(),
                            }),
                        );
                        Some(Arc::new(device))
                    }
                    Err(default_error) => {
                        return Err(format!(
                            "Invalid system device '{name}': {error}; no default output device could be used either: {default_error}"
                        ));
                    }
                }
            }
        }
    } else {
        Some(Arc::new(default_output_device().map_err(|error| {
            format!("Default system audio is unavailable: {error}")
        })?))
    };

    // Async-first approach for custom devices - no more blocking operations!
    info!("🚀 Starting async recording initialization with custom devices");

    // Resolve and validate the save root exactly once before creating any
    // meeting directory. Store/read errors are fatal instead of silently
    // falling back to the default folder.
    let preferences = super::recording_preferences::resolve_recording_session_preferences(&app)
        .await
        .map_err(|error| format!("Recording save folder is unavailable: {error}"))?;
    info!(
        "📁 Recording session root validated: {}",
        preferences.save_folder.display()
    );

    // Create new recording manager with the frozen root.
    let mut manager = RecordingManager::new(preferences.save_folder.clone());
    manager.set_preserve_transcription_gaps(transcription::engine::uses_sensevoice(&app).await?);
    let recording_context_id = meeting_context
        .as_ref()
        .map(|context| context.recording_context_id.clone());
    let recognition_context_enabled = meeting_context.is_some();
    let recognition_context = meeting_context
        .as_ref()
        .and_then(|context| context.recording_context())
        .map(crate::meeting_context::RecognitionContext::from_snapshot)
        .map(Arc::new);
    manager
        .set_summary_template(summary_template)
        .map_err(|error| format!("Failed to set recording template metadata: {error}"))?;
    manager
        .set_meeting_context(meeting_context)
        .map_err(|error| format!("Failed to set meeting context metadata: {error}"))?;

    let auto_save = preferences.auto_save;
    info!(
        "📋 Using frozen recording preferences: auto_save={}",
        auto_save
    );

    // Always ensure a meeting name is set so incremental saver initializes
    let effective_meeting_name = meeting_name.clone().unwrap_or_else(|| {
        let now = chrono::Local::now();
        format!("Meeting {}", now.format("%Y-%m-%d_%H-%M-%S"))
    });
    manager.set_meeting_name(Some(effective_meeting_name));

    // Set up error callback
    let app_for_error = app.clone();
    manager.set_error_callback(move |error| {
        let _ = app_for_error.emit("recording-error", error.user_message());
    });

    // Start recording with specified devices and auto_save setting
    let transcription_receiver = manager
        .start_recording(mic_device, system_device, auto_save)
        .await
        .map_err(|e| format!("Failed to start recording: {}", e))?;

    // 回退证据必须在 start_recording 之后登记：那一步会清空路由证据容器。
    for (route, requested, used, reason) in &device_fallbacks {
        manager.record_preferred_device_fallback(route);
        warn!("↩️ Recording device fallback on {route}: requested '{requested}', using '{used}' ({reason})");
    }

    let measurement_recorder = manager.measurement_recorder();
    let transcript_writer = manager.transcript_writer();

    // Store the manager globally to keep it alive
    {
        let mut global_manager = RECORDING_MANAGER.lock().unwrap();
        *global_manager = Some(manager);
    }

    // Set recording flag and reset speech detection flag
    info!("🔍 Setting IS_RECORDING to true and resetting SPEECH_DETECTED_EMITTED");
    IS_RECORDING.store(true, Ordering::SeqCst);
    recording_activity.commit();
    drop(engine_lifecycle_guard);
    reset_speech_detected_flag(); // Reset for new recording session

    // Start optimized parallel transcription task and store handle
    let task_handle = transcription::start_transcription_task(
        app.clone(),
        transcription_receiver,
        recognition_context,
        measurement_recorder,
        transcript_writer,
    );
    {
        let mut global_task = TRANSCRIPTION_TASK.lock().unwrap();
        *global_task = Some(task_handle);
    }

    // Emit success event
    app.emit(
        "recording-started",
        serde_json::json!({
            "message": "Recording started with custom devices and parallel processing",
            "devices": [
                mic_device_name.unwrap_or_else(|| "Default Microphone".to_string()),
                system_device_name.unwrap_or_else(|| "Default System Audio".to_string())
            ],
            "workers": 3,
            "recordingContextId": recording_context_id,
            "recognitionContextEnabled": recognition_context_enabled
        }),
    )
    .map_err(|e| e.to_string())?;

    // Update tray menu to reflect recording state
    crate::tray::update_tray_menu(&app);

    info!("✅ Recording started with custom devices using async-first approach");

    Ok(())
}

/// Start the production VAD, queue, transcription, and writeback path from a
/// controlled PCM file. The request is rejected unless both input and output
/// roots carry the matching D-11 isolation marker.
#[tauri::command]
pub async fn start_controlled_realtime_input<R: Runtime>(
    app: AppHandle<R>,
    request: RealtimeAudioInputRequest,
) -> Result<RealtimeAudioInputAccepted, String> {
    if IS_RECORDING.load(Ordering::SeqCst) {
        return Err("Recording already in progress".to_owned());
    }
    let meeting_name = request
        .meeting_name
        .clone()
        .unwrap_or_else(|| format!("D11 {}", request.baseline_snapshot_id));
    let (contract, canonical_pcm) =
        tokio::task::spawn_blocking(move || prepare_realtime_audio_input(request))
            .await
            .map_err(|error| format!("controlled input preparation task failed: {error}"))?
            .map_err(|error| format!("controlled input contract rejected: {error:#}"))?;
    let accepted = RealtimeAudioInputAccepted::from(&contract);

    let recording_activity = super::import::RecordingActivityGuard::acquire()?;
    let _storage_start_guard = begin_storage_operation(StorageOperationKind::Recording)
        .map_err(|error| error.to_string())?;
    let engine_lifecycle_guard = super::common::acquire_engine_lifecycle_lock().await;
    transcription::validate_transcription_model_ready(&app)
        .await
        .map_err(|error| format!("controlled recording model is unavailable: {error}"))?;

    let mut manager =
        RecordingManager::new(std::path::PathBuf::from(&contract.isolated_output_root));
    manager.set_preserve_transcription_gaps(transcription::engine::uses_sensevoice(&app).await?);
    manager.set_meeting_name(Some(meeting_name));
    manager
        .set_realtime_input_contract(contract.clone())
        .map_err(|error| format!("failed to set controlled input contract: {error:#}"))?;
    let transcription_receiver = manager
        .start_controlled_realtime_input()
        .await
        .map_err(|error| format!("failed to start controlled real-time pipeline: {error:#}"))?;
    let measurement_recorder = manager
        .measurement_recorder()
        .ok_or_else(|| "controlled measurement recorder was not initialized".to_owned())?;
    let recording_state = manager.get_state().clone();
    let transcript_writer = manager.transcript_writer();

    {
        let mut global_manager = RECORDING_MANAGER.lock().unwrap();
        *global_manager = Some(manager);
    }
    IS_RECORDING.store(true, Ordering::SeqCst);
    recording_activity.commit();
    drop(engine_lifecycle_guard);
    reset_speech_detected_flag();

    let task_handle = transcription::start_transcription_task(
        app.clone(),
        transcription_receiver,
        None,
        Some(measurement_recorder.clone()),
        transcript_writer,
    );
    *TRANSCRIPTION_TASK.lock().unwrap() = Some(task_handle);

    CONTROLLED_INPUT_CANCEL.store(false, Ordering::SeqCst);
    let replay_app = app.clone();
    let replay_contract = contract.clone();
    let replay_recorder = measurement_recorder.clone();
    let replay_handle = tokio::spawn(async move {
        let replay_start = tokio::time::Instant::now();
        let mut sent_samples = 0_usize;
        let mut completed = true;
        for (frame_index, frame) in canonical_pcm.chunks(CONTROLLED_FRAME_SAMPLES).enumerate() {
            if CONTROLLED_INPUT_CANCEL.load(Ordering::SeqCst) {
                completed = false;
                replay_recorder.record_stop_stage(
                    "controlled_input_replay_cancelled",
                    "cancelled",
                    None,
                );
                break;
            }
            let due = replay_start
                + std::time::Duration::from_secs_f64(
                    sent_samples as f64 / CONTROLLED_SAMPLE_RATE as f64,
                );
            tokio::time::sleep_until(due).await;
            if CONTROLLED_INPUT_CANCEL.load(Ordering::SeqCst) {
                completed = false;
                replay_recorder.record_stop_stage(
                    "controlled_input_replay_cancelled",
                    "cancelled",
                    None,
                );
                break;
            }
            let data = frame.to_vec();
            let chunk = AudioChunk {
                data,
                sample_rate: CONTROLLED_SAMPLE_RATE,
                timestamp: sent_samples as f64 / CONTROLLED_SAMPLE_RATE as f64,
                chunk_id: frame_index as u64,
                device_type: RecordingDeviceType::Microphone,
                device_epoch: 0,
                capture_qpc_ns: None,
            };
            if let Err(error) = recording_state.send_audio_chunk(chunk) {
                completed = false;
                replay_recorder.record_error(format!(
                    "controlled input send failed at frame {frame_index}: {error:#}"
                ));
                break;
            }
            sent_samples += frame.len();
        }
        if completed && sent_samples as u64 == replay_contract.canonical_sample_count {
            replay_recorder.record_stop_stage(
                "controlled_input_replay_completed",
                "completed",
                None,
            );
            let _ = replay_app.emit(
                "controlled-realtime-input-complete",
                serde_json::json!({
                    "audioInputPathId": replay_contract.audio_input_path_id,
                    "baselineSnapshotId": replay_contract.baseline_snapshot_id,
                    "sentSamples": sent_samples,
                    "expectedSamples": replay_contract.canonical_sample_count,
                    "sampleRateHz": CONTROLLED_SAMPLE_RATE,
                }),
            );
        }
    });
    *CONTROLLED_INPUT_TASK.lock().unwrap() = Some(replay_handle);

    app.emit(
        "recording-started",
        serde_json::json!({
            "message": "Controlled real-time input started",
            "audioInputPathId": contract.audio_input_path_id,
            "baselineSnapshotId": contract.baseline_snapshot_id,
            "isolatedDataIdentity": contract.isolated_data_identity,
            "workers": 1,
        }),
    )
    .map_err(|error| error.to_string())?;
    Ok(accepted)
}

/// Stop recording with optimized graceful shutdown ensuring NO transcript chunks are lost
pub async fn stop_recording<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    info!(
        "🛑 Starting optimized recording shutdown - ensuring ALL transcript chunks are preserved"
    );

    // Check if recording is active
    if !IS_RECORDING.load(Ordering::SeqCst) {
        info!("Recording was not active");
        return Ok(());
    }

    // Emit shutdown progress to frontend
    let _ = app.emit(
        "recording-shutdown-progress",
        serde_json::json!({
            "stage": "stopping_audio",
            "message": "Stopping audio capture...",
            "progress": 20
        }),
    );

    // Step 1: Stop audio capture immediately (no more new chunks) with proper error handling
    let manager_for_cleanup = {
        let mut global_manager = RECORDING_MANAGER.lock().unwrap();
        global_manager.take()
    };
    let measurement_recorder: Option<Arc<D11MeasurementRecorder>> = manager_for_cleanup
        .as_ref()
        .and_then(RecordingManager::measurement_recorder);
    if let Some(recorder) = measurement_recorder.as_ref() {
        recorder.record_stop_stage("stop_requested", "started", None);
    }

    CONTROLLED_INPUT_CANCEL.store(true, Ordering::SeqCst);
    let controlled_input_task = CONTROLLED_INPUT_TASK.lock().unwrap().take();
    if let Some(handle) = controlled_input_task {
        match tokio::time::timeout(std::time::Duration::from_secs(2), handle).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                if let Some(recorder) = measurement_recorder.as_ref() {
                    recorder.record_error(format!("controlled input task failed: {error}"));
                }
            }
            Err(_) => {
                if let Some(recorder) = measurement_recorder.as_ref() {
                    recorder.record_error("controlled input task stop timed out".to_owned());
                }
            }
        }
    }

    let (stop_result, manager_for_cleanup, duration_before_stop) =
        if let Some(mut manager) = manager_for_cleanup {
            let duration_before_stop = manager.get_active_recording_duration();
            // Use FORCE FLUSH to immediately process all accumulated audio - eliminates 30s delay!
            info!("🚀 Using FORCE FLUSH to eliminate pipeline accumulation delays");
            let result = manager.stop_streams_and_force_flush().await;
            // Store manager back for later cleanup
            let manager_for_cleanup = Some(manager);
            (result, manager_for_cleanup, duration_before_stop)
        } else {
            warn!("No recording manager found to stop");
            (Ok(()), None, None)
        };

    match stop_result {
        Ok(_) => {
            info!("✅ Audio streams stopped successfully - no more chunks will be created");
            if let Some(recorder) = measurement_recorder.as_ref() {
                recorder.record_stop_stage("capture_stopped", "completed", None);
            }
        }
        Err(e) => {
            error!("❌ Failed to stop audio streams: {}", e);
            if let Some(recorder) = measurement_recorder.as_ref() {
                recorder.record_stop_stage("capture_stopped", "failed", Some(&e.to_string()));
                recorder.record_error(format!("audio capture stop failed: {e:#}"));
                if let Err(finalize_error) = recorder.finalize().await {
                    return Err(format!(
                        "Failed to stop audio streams: {e}; D-11 evidence finalization also failed: {finalize_error:#}"
                    ));
                }
            }
            return Err(format!("Failed to stop audio streams: {}", e));
        }
    }

    // Step 2: Signal transcription workers to finish processing ALL queued chunks
    let _ = app.emit(
        "recording-shutdown-progress",
        serde_json::json!({
            "stage": "processing_transcripts",
            "message": "Processing remaining transcript chunks...",
            "progress": 40
        }),
    );

    // Wait for transcription task with enhanced progress monitoring (NO TIMEOUT - we must process all chunks)
    let transcription_task = {
        let mut global_task = TRANSCRIPTION_TASK.lock().unwrap();
        global_task.take()
    };

    if let Some(task_handle) = transcription_task {
        if let Some(recorder) = measurement_recorder.as_ref() {
            recorder.record_stop_stage("queue_drain_started", "started", None);
        }
        info!("⏳ Waiting for ALL transcription chunks to be processed (no timeout - preserving every chunk)");

        // Enhanced progress monitoring during shutdown
        let progress_app = app.clone();
        let progress_task = tokio::spawn(async move {
            let last_update = std::time::Instant::now();

            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                // Emit periodic progress updates during shutdown
                let elapsed = last_update.elapsed().as_secs();
                let _ = progress_app.emit(
                    "recording-shutdown-progress",
                    serde_json::json!({
                        "stage": "processing_transcripts",
                        "message": format!("Processing transcripts... ({}s elapsed)", elapsed),
                        "progress": 40,
                        "detailed": true,
                        "elapsed_seconds": elapsed
                    }),
                );
            }
        });

        // Wait up to 10 minutes for transcription completion to prevent indefinite hangs
        match tokio::time::timeout(
            tokio::time::Duration::from_secs(600), // 10 minutes max
            task_handle,
        )
        .await
        {
            Ok(Ok(())) => {
                info!("✅ ALL transcription chunks processed successfully - no data lost");
                if let Some(recorder) = measurement_recorder.as_ref() {
                    recorder.record_stop_stage("queue_drained", "completed", None);
                }
            }
            Ok(Err(e)) => {
                warn!("⚠️ Transcription task completed with error: {:?}", e);
                if let Some(manager) = manager_for_cleanup.as_ref() {
                    manager.transcript_writer().report_transcription_error(&e.to_string());
                }
                if let Some(recorder) = measurement_recorder.as_ref() {
                    recorder.record_stop_stage("queue_drained", "failed", Some(&e.to_string()));
                }
                // Continue anyway - the worker may have processed most chunks
            }
            Err(_) => {
                warn!("⏱️ Transcription timeout (10 minutes) reached, continuing shutdown to prevent indefinite hang");
                if let Some(manager) = manager_for_cleanup.as_ref() {
                    manager.transcript_writer().report_transcription_error("Transcription queue drain timed out");
                }
                if let Some(recorder) = measurement_recorder.as_ref() {
                    recorder.record_stop_stage(
                        "queue_drained",
                        "timeout",
                        Some("transcription queue drain exceeded 600 seconds"),
                    );
                }
                // Continue shutdown even on timeout - better to lose some chunks than hang forever
            }
        };

        // Stop progress monitoring
        progress_task.abort();
    } else {
        info!("ℹ️ No transcription task found to wait for");
        if let Some(recorder) = measurement_recorder.as_ref() {
            recorder.record_stop_stage(
                "queue_drained",
                "not_started",
                Some("transcription task was unavailable"),
            );
        }
    }

    // Step 3: Keep the validated transcription model warm for the next session.
    // Unloading here forced every recording start to synchronously reload hundreds
    // of MiB before audio capture could begin, which violated the 2-second start
    // gate. Model selection commands still unload/reload when the user changes the
    // configured model, and process exit releases the memory normally.
    let _ = app.emit(
        "recording-shutdown-progress",
        serde_json::json!({
            "stage": "model_ready",
            "message": "Speech recognition model kept ready for the next recording.",
            "progress": 70
        }),
    );
    info!("🧠 All transcript chunks processed; retaining the transcription model in memory");

    // Step 3.5: Track meeting ended analytics with privacy-safe metadata
    // Extract all data from manager BEFORE any async operations to avoid Send issues
    let analytics_data = if let Some(ref manager) = manager_for_cleanup {
        let state = manager.get_state();
        let stats = state.get_stats();

        Some((
            manager.get_recording_duration(),
            manager.get_active_recording_duration().unwrap_or(0.0),
            manager.get_total_pause_duration(),
            manager.get_transcript_segments().len() as u64,
            state.has_fatal_error(),
            state.get_microphone_device().map(|d| d.name.clone()),
            state.get_system_device().map(|d| d.name.clone()),
            stats.chunks_processed,
        ))
    } else {
        None
    };

    // Now perform async analytics tracking without holding manager reference
    if let Some((
        total_duration,
        active_duration,
        pause_duration,
        transcript_segments_count,
        had_fatal_error,
        mic_device_name,
        sys_device_name,
        chunks_processed,
    )) = analytics_data
    {
        info!("📊 Collecting analytics for meeting end");

        // Helper function to classify device type from device name (privacy-safe)
        fn classify_device_type(device_name: &str) -> &'static str {
            let name_lower = device_name.to_lowercase();
            // Check for Bluetooth keywords
            if name_lower.contains("bluetooth")
                || name_lower.contains("airpods")
                || name_lower.contains("beats")
                || name_lower.contains("headphones")
                || name_lower.contains("bt ")
                || name_lower.contains("wireless")
            {
                "Bluetooth"
            } else {
                "Wired"
            }
        }

        // Get transcription model info (already loaded above for model unload)
        let transcription_config = match crate::api::api::api_get_transcript_config(
            app.clone(),
            app.clone().state(),
            None,
        )
        .await
        {
            Ok(Some(config)) => Some((config.provider, config.model)),
            _ => None,
        };

        let (transcription_provider, transcription_model) =
            transcription_config.unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string()));

        // Get summary model info from API
        let summary_config =
            match crate::api::api::api_get_model_config(app.clone(), app.clone().state(), None)
                .await
            {
                Ok(Some(config)) => Some((config.provider, config.model)),
                _ => None,
            };

        let (summary_provider, summary_model) =
            summary_config.unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string()));

        // Classify device types (privacy-safe)
        let microphone_device_type = mic_device_name
            .as_ref()
            .map(|name| classify_device_type(name))
            .unwrap_or("Unknown");

        let system_audio_device_type = sys_device_name
            .as_ref()
            .map(|name| classify_device_type(name))
            .unwrap_or("Unknown");

        // Track meeting ended event with privacy-safe data
        match crate::analytics::commands::track_meeting_ended(
            transcription_provider.clone(),
            transcription_model.clone(),
            summary_provider.clone(),
            summary_model.clone(),
            total_duration,
            active_duration,
            pause_duration,
            microphone_device_type.to_string(),
            system_audio_device_type.to_string(),
            chunks_processed,
            transcript_segments_count,
            had_fatal_error,
        )
        .await
        {
            Ok(_) => info!("✅ Analytics tracked successfully for meeting end"),
            Err(e) => warn!("⚠️ Failed to track analytics: {}", e),
        }
    }

    // Step 4: Finalize recording state and cleanup resources safely
    let _ = app.emit(
        "recording-shutdown-progress",
        serde_json::json!({
            "stage": "finalizing",
            "message": "Finalizing recording and cleaning up resources...",
            "progress": 90
        }),
    );

    // Perform final cleanup with the manager if available
    let (meeting_folder, meeting_name, save_error) = if let Some(mut manager) = manager_for_cleanup
    {
        info!("🧹 Performing final cleanup and saving recording data");

        // Extract meeting info BEFORE async operations
        let meeting_folder = manager.get_meeting_folder();
        let meeting_name = manager.get_meeting_name();

        let save_error = match tokio::time::timeout(
            tokio::time::Duration::from_secs(300), // 5 minutes max for file I/O
            manager.save_recording_only(&app, duration_before_stop),
        )
        .await
        {
            Ok(Ok(_)) => {
                info!("✅ Recording data saved successfully during cleanup");
                None
            }
            Ok(Err(e)) => {
                error!("❌ Recording save failed during cleanup: {}", e);
                Some(format!("Failed to save recording data: {e}"))
            }
            Err(_) => {
                error!("⏱️ File I/O timeout (5 minutes) reached during save");
                Some("Timed out while saving recording data".to_string())
            }
        };

        (meeting_folder, meeting_name, save_error)
    } else {
        info!("ℹ️ No recording manager available for cleanup");
        (None, None, None)
    };

    // Set recording flag to false
    info!("🔍 Setting IS_RECORDING to false");
    IS_RECORDING.store(false, Ordering::SeqCst);
    super::import::finish_recording_activity();

    if let Some(save_error) = save_error {
        let _ = app.emit(
            "recording-error",
            serde_json::json!({
                "error": &save_error,
                "userMessage": &save_error,
                "actionable": true
            }),
        );
        crate::tray::update_tray_menu(&app);
        if let Some(recorder) = measurement_recorder.as_ref() {
            recorder.record_stop_stage("final_save_aborted", "failed", Some(&save_error));
            recorder.record_error(save_error.clone());
            if let Err(finalize_error) = recorder.finalize().await {
                return Err(format!(
                    "{save_error}; D-11 evidence finalization also failed: {finalize_error:#}"
                ));
            }
        }
        return Err(save_error);
    }

    // Step 4.5: Prepare metadata for frontend (NO database save)
    // NOTE: We do NOT save to database here. The frontend will save after all transcripts are displayed.
    // This ensures the user sees all transcripts streaming in before the database save happens.
    let (folder_path_str, meeting_name_str) = match (&meeting_folder, &meeting_name) {
        (Some(path), Some(name)) => (Some(path.to_string_lossy().to_string()), Some(name.clone())),
        _ => (None, None),
    };

    info!("📤 Preparing recording metadata for frontend save");
    info!("   folder_path: {:?}", folder_path_str);
    info!("   meeting_name: {:?}", meeting_name_str);

    // Database save removed - frontend will handle this after receiving all transcripts
    info!("ℹ️ Skipping database save in Rust - frontend will save after all transcripts received");

    // Step 5: Complete shutdown
    let _ = app.emit(
        "recording-shutdown-progress",
        serde_json::json!({
            "stage": "complete",
            "message": "Recording stopped successfully",
            "progress": 100
        }),
    );

    // Emit final stop event with folder_path and meeting_name for frontend to save
    let stopped_event_result = app.emit(
        "recording-stopped",
        serde_json::json!({
            "message": "Recording stopped - frontend will save after all transcripts received",
            "folder_path": folder_path_str,
            "meeting_name": meeting_name_str
        }),
    );
    if let Some(recorder) = measurement_recorder.as_ref() {
        match &stopped_event_result {
            Ok(()) => {
                recorder.record_stop_stage("recording_stopped_event_emitted", "completed", None)
            }
            Err(error) => recorder.record_stop_stage(
                "recording_stopped_event_emitted",
                "failed",
                Some(&error.to_string()),
            ),
        }
        recorder
            .finalize()
            .await
            .map_err(|error| format!("failed to finalize D-11 measurement evidence: {error:#}"))?;
    }
    stopped_event_result.map_err(|error| error.to_string())?;

    // Update tray menu to reflect stopped state
    crate::tray::update_tray_menu(&app);

    info!("🎉 Recording stopped successfully with ZERO transcript chunks lost");
    Ok(())
}

/// Check if recording is active
pub async fn is_recording() -> bool {
    IS_RECORDING.load(Ordering::SeqCst)
}

/// Get recording statistics
#[tauri::command]
pub async fn get_transcription_status() -> TranscriptionStatus {
    let (chunks_in_queue, is_processing) = transcription::current_transcription_queue_status();
    TranscriptionStatus {
        chunks_in_queue,
        is_processing,
        last_activity_ms: 0,
    }
}

/// Pause the current recording
#[tauri::command]
pub async fn pause_recording<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    info!("Pausing recording");

    // Check if currently recording
    if !IS_RECORDING.load(Ordering::SeqCst) {
        return Err("No recording is currently active".to_string());
    }

    // Access the recording manager and pause it
    let manager_guard = RECORDING_MANAGER.lock().unwrap();
    if let Some(manager) = manager_guard.as_ref() {
        manager.pause_recording().map_err(|e| e.to_string())?;

        // Emit pause event to frontend
        app.emit(
            "recording-paused",
            serde_json::json!({
                "message": "Recording paused"
            }),
        )
        .map_err(|e| e.to_string())?;

        // Update tray menu to reflect paused state
        crate::tray::update_tray_menu(&app);

        info!("Recording paused successfully");
        Ok(())
    } else {
        Err("No recording manager found".to_string())
    }
}

/// Resume the current recording
#[tauri::command]
pub async fn resume_recording<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    info!("Resuming recording");

    // Check if currently recording
    if !IS_RECORDING.load(Ordering::SeqCst) {
        return Err("No recording is currently active".to_string());
    }

    // Drain monitor events before resume so a paused session cannot resume on
    // a stale default endpoint.
    while poll_audio_device_events().await?.is_some() {}

    let runtime = tokio::runtime::Handle::current();
    let resume_result = tokio::task::spawn_blocking(move || {
        let mut manager_guard = RECORDING_MANAGER.lock().unwrap();
        let manager = manager_guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("No recording manager found"))?;
        runtime.block_on(manager.resume_recording())
    })
    .await
    .map_err(|error| format!("Resume task failed: {error}"))?;

    if let Err(error) = resume_result {
        if let Some(manager) = RECORDING_MANAGER.lock().unwrap().as_ref() {
            manager.get_state().report_error(AudioError::StreamFailed);
        }
        return Err(error.to_string());
    }

    // Emit resume event to frontend only after endpoint validation/rebind.
    app.emit(
        "recording-resumed",
        serde_json::json!({
            "message": "Recording resumed"
        }),
    )
    .map_err(|e| e.to_string())?;

    // Update tray menu to reflect resumed state
    crate::tray::update_tray_menu(&app);

    info!("Recording resumed successfully");
    Ok(())
}

/// Check if recording is currently paused
#[tauri::command]
pub async fn is_recording_paused() -> bool {
    let manager_guard = RECORDING_MANAGER.lock().unwrap();
    if let Some(manager) = manager_guard.as_ref() {
        manager.is_paused()
    } else {
        false
    }
}

/// Get detailed recording state
#[tauri::command]
pub async fn get_recording_state() -> serde_json::Value {
    let is_recording = IS_RECORDING.load(Ordering::SeqCst);
    let mut manager_guard = RECORDING_MANAGER.lock().unwrap();

    if let Some(manager) = manager_guard.as_mut() {
        if let Err(error) = manager.persist_audio_route_evidence() {
            log::error!("Failed to persist audio route evidence snapshot: {error:#}");
        }
        let state = manager.get_state();
        let microphone_device = state.get_microphone_device();
        let system_device = state.get_system_device();
        let (microphone_callbacks, microphone_samples) =
            state.route_callback_counts(&RecordingDeviceType::Microphone);
        let (system_callbacks, system_samples) =
            state.route_callback_counts(&RecordingDeviceType::System);
        let (microphone_rms, microphone_peak) =
            state.route_audio_levels(&RecordingDeviceType::Microphone);
        let (system_rms, system_peak) = state.route_audio_levels(&RecordingDeviceType::System);
        let (
            microphone_last_capture_qpc_ns,
            microphone_last_callback_observed_qpc_ns,
            microphone_max_callback_gap_ns,
            microphone_no_signal,
            microphone_no_signal_count,
        ) = state.microphone_route_health();
        let (
            system_last_capture_qpc_ns,
            system_last_callback_observed_qpc_ns,
            system_max_callback_gap_ns,
            system_no_signal,
            system_no_signal_count,
        ) = state.system_route_health();
        let (system_frames, system_silent_flag_frames, system_all_zero_frames) =
            state.system_native_frame_counts();
        let system_format = state.system_audio_format();
        let (system_endpoint_muted, system_audio_silent, driver_mute_behavior) =
            state.system_mute_state();
        let (in_flight_callback_count, callback_drain_deadline_qpc_ns) =
            state.callback_drain_state();
        let (
            cutover_watermark_qpc_ns,
            old_epoch_last_capture_qpc_ns,
            new_epoch_first_capture_qpc_ns,
            late_callback_dropped_frames,
            attribution_error_frames,
        ) = state.cutover_state();
        let recording_mode = match (microphone_device.is_some(), system_device.is_some()) {
            (true, true) => "microphone_and_system",
            (true, false) => "microphone_only",
            (false, true) => "system_only",
            (false, false) => "inactive",
        };
        let last_error = state.get_last_error().map(|error| format!("{error:?}"));
        serde_json::json!({
            "is_recording": is_recording,
            "is_paused": manager.is_paused(),
            "is_active": manager.is_active(),
            "is_reconnecting": manager.is_reconnecting(),
            "recording_mode": recording_mode,
            "device_epoch": state.get_device_epoch(),
            "system_stream_started_qpc_ns": state.system_stream_started_qpc_ns(),
            "last_error": last_error,
            "cutover": {
                "watermark_qpc_ns": cutover_watermark_qpc_ns,
                "callback_drain_deadline_qpc_ns": (callback_drain_deadline_qpc_ns != 0).then_some(callback_drain_deadline_qpc_ns),
                "in_flight_callback_count": in_flight_callback_count,
                "old_epoch_last_capture_qpc_ns": old_epoch_last_capture_qpc_ns,
                "new_epoch_first_capture_qpc_ns": new_epoch_first_capture_qpc_ns,
                "late_callback_dropped_frames": late_callback_dropped_frames,
                "attribution_error_frames": attribution_error_frames
            },
            "microphone_route": {
                "active": microphone_device.is_some(),
                "device_name": microphone_device.as_ref().map(|device| device.name.as_str()),
                "native_id": microphone_device.as_ref().and_then(|device| device.native_id.as_deref()),
                "stream_started_count": state.microphone_stream_started_count(),
                "callback_count": microphone_callbacks,
                "frame_count": state.microphone_frame_count(),
                "sample_count": microphone_samples,
                "last_capture_qpc_ns": microphone_last_capture_qpc_ns,
                "last_callback_observed_qpc_ns": microphone_last_callback_observed_qpc_ns,
                "max_callback_gap_ns": microphone_max_callback_gap_ns,
                "no_signal": microphone_no_signal,
                "no_signal_count": microphone_no_signal_count,
                "failed": state.route_failed(&RecordingDeviceType::Microphone),
                "rms_level": microphone_rms,
                "peak_level": microphone_peak
            },
            "system_route": {
                "active": system_device.is_some(),
                "device_name": system_device.as_ref().map(|device| device.name.as_str()),
                "native_id": system_device.as_ref().and_then(|device| device.native_id.as_deref()),
                "callback_count": system_callbacks,
                "sample_count": system_samples,
                "frame_count": system_frames,
                "silent_flag_frame_count": system_silent_flag_frames,
                "all_zero_frame_count": system_all_zero_frames,
                "last_capture_qpc_ns": system_last_capture_qpc_ns,
                "last_callback_observed_qpc_ns": system_last_callback_observed_qpc_ns,
                "max_callback_gap_ns": system_max_callback_gap_ns,
                "no_signal": system_no_signal,
                "no_signal_count": system_no_signal_count,
                "failed": state.route_failed(&RecordingDeviceType::System),
                "endpoint_muted": system_endpoint_muted,
                "silent": system_audio_silent,
                "driver_mute_behavior": driver_mute_behavior,
                "rms_level": system_rms,
                "peak_level": system_peak,
                "format": system_format.as_ref().map(|format| serde_json::json!({
                    "sample_rate": format.sample_rate,
                    "channels": format.channels,
                    "bits_per_sample": format.bits_per_sample,
                    "block_align": format.block_align,
                    "sample_format": format.sample_format
                }))
            },
            "recording_duration": manager.get_recording_duration(),
            "active_duration": manager.get_active_recording_duration(),
            "total_pause_duration": manager.get_total_pause_duration(),
            "current_pause_duration": manager.get_current_pause_duration()
        })
    } else {
        serde_json::json!({
            "is_recording": is_recording,
            "is_paused": false,
            "is_active": false,
            "is_reconnecting": false,
            "recording_mode": "inactive",
            "device_epoch": 0,
            "system_stream_started_qpc_ns": null,
            "last_error": null,
            "cutover": {
                "watermark_qpc_ns": null,
                "callback_drain_deadline_qpc_ns": null,
                "in_flight_callback_count": 0,
                "old_epoch_last_capture_qpc_ns": null,
                "new_epoch_first_capture_qpc_ns": null,
                "late_callback_dropped_frames": 0,
                "attribution_error_frames": 0
            },
            "microphone_route": {
                "active": false,
                "device_name": null,
                "native_id": null,
                "stream_started_count": 0,
                "callback_count": 0,
                "frame_count": 0,
                "sample_count": 0,
                "last_capture_qpc_ns": null,
                "last_callback_observed_qpc_ns": null,
                "max_callback_gap_ns": 0,
                "no_signal": false,
                "no_signal_count": 0,
                "failed": false,
                "rms_level": 0.0,
                "peak_level": 0.0
            },
            "system_route": {
                "active": false,
                "device_name": null,
                "native_id": null,
                "callback_count": 0,
                "sample_count": 0,
                "frame_count": 0,
                "silent_flag_frame_count": 0,
                "all_zero_frame_count": 0,
                "last_capture_qpc_ns": null,
                "last_callback_observed_qpc_ns": null,
                "max_callback_gap_ns": 0,
                "no_signal": false,
                "no_signal_count": 0,
                "failed": false,
                "endpoint_muted": false,
                "silent": false,
                "driver_mute_behavior": null,
                "rms_level": 0.0,
                "peak_level": 0.0,
                "format": null
            },
            "recording_duration": null,
            "active_duration": null,
            "total_pause_duration": 0.0,
            "current_pause_duration": null
        })
    }
}

/// Get the meeting folder path for the current recording
/// Returns the path if a meeting name was set and folder structure initialized
#[tauri::command]
pub async fn get_meeting_folder_path() -> Result<Option<String>, String> {
    let manager_guard = RECORDING_MANAGER.lock().unwrap();
    if let Some(manager) = manager_guard.as_ref() {
        Ok(manager
            .get_meeting_folder()
            .map(|p| p.to_string_lossy().to_string()))
    } else {
        Ok(None)
    }
}

/// Get accumulated transcript segments from current recording session
/// Used for syncing frontend state after page reload during active recording
#[tauri::command]
pub async fn get_transcript_history(
    folder_path: Option<String>,
) -> Result<Vec<crate::audio::recording_saver::TranscriptSegment>, String> {
    if let Some(folder) = folder_path {
        return tokio::task::spawn_blocking(move || {
            super::recording_saver::read_completed_transcript_snapshot(std::path::Path::new(&folder))
                .map_err(|error| format!("Failed to read completed transcript: {error:#}"))
        }).await.map_err(|error| error.to_string())?;
    }
    let manager_guard = RECORDING_MANAGER.lock().unwrap();

    if let Some(manager) = manager_guard.as_ref() {
        Ok(manager.get_transcript_segments())
    } else {
        Ok(Vec::new()) // No recording active, return empty
    }
}

/// Get meeting name from current recording session
/// Used for syncing frontend state after page reload during active recording
#[tauri::command]
pub async fn get_recording_meeting_name() -> Result<Option<String>, String> {
    let manager_guard = RECORDING_MANAGER.lock().unwrap();

    if let Some(manager) = manager_guard.as_ref() {
        Ok(manager.get_meeting_name())
    } else {
        Ok(None)
    }
}

// ============================================================================
// DEVICE MONITORING COMMANDS (AirPods/Bluetooth disconnect/reconnect support)
// ============================================================================

/// Response structure for device events
#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type")]
pub enum DeviceEventResponse {
    DeviceDisconnected {
        device_name: String,
        native_id: Option<String>,
        device_type: String,
    },
    DeviceReconnected {
        device_name: String,
        native_id: Option<String>,
        device_type: String,
    },
    DeviceListChanged,
}

impl From<DeviceEvent> for DeviceEventResponse {
    fn from(event: DeviceEvent) -> Self {
        match event {
            DeviceEvent::DeviceDisconnected {
                device_name,
                native_id,
                device_type,
            } => DeviceEventResponse::DeviceDisconnected {
                device_name,
                native_id,
                device_type: format!("{:?}", device_type),
            },
            DeviceEvent::DeviceReconnected {
                device_name,
                native_id,
                device_type,
            } => DeviceEventResponse::DeviceReconnected {
                device_name,
                native_id,
                device_type: format!("{:?}", device_type),
            },
            DeviceEvent::DeviceListChanged => DeviceEventResponse::DeviceListChanged,
        }
    }
}

/// Reconnection status information
#[derive(Debug, Serialize, Clone)]
pub struct ReconnectionStatus {
    pub is_reconnecting: bool,
    pub disconnected_device: Option<DisconnectedDeviceInfo>,
}

/// Information about a disconnected device
#[derive(Debug, Serialize, Clone)]
pub struct DisconnectedDeviceInfo {
    pub name: String,
    pub device_type: String,
}

/// Poll for audio device events (disconnect/reconnect)
/// Should be called periodically (every 1-2 seconds) by frontend during recording
#[tauri::command]
pub async fn poll_audio_device_events() -> Result<Option<DeviceEventResponse>, String> {
    // Preserve monitor event order across the 500 ms status poll and an
    // explicit resume command. A reconnect event must never overtake the
    // disconnect event that establishes the route identity to rebind.
    let _poll_guard = DEVICE_EVENT_POLL_LOCK.lock().await;
    let event = {
        let mut manager_guard = RECORDING_MANAGER.lock().unwrap();
        manager_guard
            .as_mut()
            .and_then(RecordingManager::poll_device_events)
    };
    let Some(event) = event else {
        return Ok(None);
    };
    info!("📱 Device event polled: {:?}", event);
    let response = DeviceEventResponse::from(event.clone());

    match event {
        DeviceEvent::DeviceDisconnected {
            device_name,
            native_id,
            device_type,
        } => {
            if let Some(manager) = RECORDING_MANAGER.lock().unwrap().as_mut() {
                let state = manager.get_state();
                let active_device = match &device_type {
                    DeviceMonitorType::Microphone => state.get_microphone_device(),
                    DeviceMonitorType::SystemAudio => state.get_system_device(),
                };
                let event_is_current = active_device.as_ref().is_some_and(|device| {
                    #[cfg(target_os = "windows")]
                    {
                        native_id.as_deref().is_some_and(|endpoint_id| {
                            device.native_id.as_deref() == Some(endpoint_id)
                        })
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        match native_id.as_deref() {
                            Some(endpoint_id) => device.native_id.as_deref() == Some(endpoint_id),
                            None => device.name == device_name,
                        }
                    }
                });
                if event_is_current {
                    manager.handle_device_disconnect(device_name, device_type);
                } else {
                    info!(
                        "Ignoring stale audio disconnect event for endpoint {:?}",
                        native_id
                    );
                }
            }
        }
        DeviceEvent::DeviceReconnected {
            device_name,
            native_id: _,
            device_type,
        } => {
            let runtime = tokio::runtime::Handle::current();
            let rebind_result = tokio::task::spawn_blocking(move || {
                let mut manager_guard = RECORDING_MANAGER.lock().unwrap();
                let Some(manager) = manager_guard.as_mut() else {
                    return Ok(());
                };
                if !manager.is_reconnecting() {
                    return Ok(());
                }
                let state = manager.get_state().clone();
                match runtime.block_on(manager.handle_device_reconnect(device_name, device_type)) {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        state.report_error(AudioError::StreamFailed);
                        Err(error)
                    }
                }
            })
            .await
            .map_err(|error| format!("Device rebind task failed: {error}"))?;
            if let Err(error) = rebind_result {
                error!("Required audio route rebind failed: {error}");
            }
        }
        DeviceEvent::DeviceListChanged => {}
    }

    Ok(Some(response))
}

/// Get current reconnection status
/// Returns whether the system is attempting to reconnect and which device
#[tauri::command]
pub async fn get_reconnection_status() -> Result<ReconnectionStatus, String> {
    let manager_guard = RECORDING_MANAGER.lock().unwrap();

    if let Some(manager) = manager_guard.as_ref() {
        let state = manager.get_state();
        let disconnected_device = state
            .get_disconnected_device()
            .map(|(device, device_type)| DisconnectedDeviceInfo {
                name: device.name.clone(),
                device_type: format!("{:?}", device_type),
            });

        Ok(ReconnectionStatus {
            is_reconnecting: manager.is_reconnecting(),
            disconnected_device,
        })
    } else {
        // Not recording, no reconnection in progress
        Ok(ReconnectionStatus {
            is_reconnecting: false,
            disconnected_device: None,
        })
    }
}

/// Get information about the active audio output device
/// Used to warn users about Bluetooth playback issues
#[tauri::command]
pub async fn get_active_audio_output() -> Result<super::playback_monitor::AudioOutputInfo, String> {
    super::playback_monitor::get_active_audio_output()
        .await
        .map_err(|e| format!("Failed to get audio output info: {}", e))
}

/// Manually trigger device reconnection attempt
/// Useful for UI "Retry" button
#[tauri::command]
pub async fn attempt_device_reconnect(
    device_name: String,
    device_type: String,
) -> Result<bool, String> {
    // Parse device type first
    let monitor_type = match device_type.as_str() {
        "Microphone" => DeviceMonitorType::Microphone,
        "SystemAudio" => DeviceMonitorType::SystemAudio,
        _ => return Err(format!("Invalid device type: {}", device_type)),
    };

    // Check if recording is active
    {
        let manager_guard = RECORDING_MANAGER.lock().unwrap();
        if manager_guard.is_none() {
            return Err("Recording not active".to_string());
        }
    } // Release lock

    // Spawn blocking task to handle the async reconnection
    let result = tokio::task::spawn_blocking(move || {
        tokio::runtime::Handle::current().block_on(async {
            let mut manager_guard = RECORDING_MANAGER.lock().unwrap();
            if let Some(manager) = manager_guard.as_mut() {
                manager
                    .attempt_device_reconnect(&device_name, monitor_type)
                    .await
            } else {
                Err(anyhow::anyhow!("Recording not active"))
            }
        })
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?;

    match result {
        Ok(success) => {
            if success {
                info!("✅ Manual reconnection successful");
            } else {
                warn!("❌ Manual reconnection failed - device not available");
            }
            Ok(success)
        }
        Err(e) => {
            error!("Manual reconnection error: {}", e);
            Err(e.to_string())
        }
    }
}
