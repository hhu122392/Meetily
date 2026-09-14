use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex as StdMutex;
// Removed unused import

// Performance optimization: Conditional logging macros for hot paths
#[cfg(debug_assertions)]
macro_rules! perf_debug {
    ($($arg:tt)*) => {
        log::debug!($($arg)*)
    };
}

#[cfg(not(debug_assertions))]
macro_rules! perf_debug {
    ($($arg:tt)*) => {};
}

#[cfg(debug_assertions)]
macro_rules! perf_trace {
    ($($arg:tt)*) => {
        log::trace!($($arg)*)
    };
}

#[cfg(not(debug_assertions))]
macro_rules! perf_trace {
    ($($arg:tt)*) => {};
}

// Make these macros available to other modules
pub(crate) use perf_debug;
pub(crate) use perf_trace;

// Re-export async logging macros for external use (removed due to macro conflicts)

// Declare audio module
pub mod analytics;
pub mod anthropic;
pub mod api;
pub mod audio;
pub mod config;
pub mod console_utils;
pub mod database;
pub mod formal_whisper_baseline;
pub mod groq;
pub mod i18n;
pub mod meeting_context;
pub mod moss_alignment;
pub mod moss_audio_token_alignment;
pub mod moss_helper;
pub mod moss_review;
pub mod notifications;
pub mod ollama;
pub mod onboarding;
pub mod sensevoice_engine;
pub mod transcript_term_correction;
pub mod openai;
pub mod openrouter;
pub mod parakeet_engine;
pub mod state;
pub mod storage;
pub mod summary;
pub mod transcript_revision;
pub mod transcript_proofread;
pub mod transcript_text_edit;
pub mod proofread_protocol;
pub mod transcript_file_store;
pub mod transcript_file_sync;
pub mod transcript_normalization;
pub mod tray;
pub mod utils;
pub mod whisper_engine;

use audio::{list_audio_devices, trigger_audio_permission, AudioDevice};
use log::{error as log_error, info as log_info};
use notifications::commands::NotificationManagerState;
use std::sync::Arc;
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::RwLock;
use uuid::Uuid;

static RECORDING_FLAG: AtomicBool = AtomicBool::new(false);
static APP_SESSION_ID: once_cell::sync::Lazy<String> =
    once_cell::sync::Lazy::new(|| Uuid::new_v4().to_string());

const DEFAULT_TRANSCRIPTION_LANGUAGE: &str = "auto";

#[tauri::command]
fn get_app_session_id() -> String {
    APP_SESSION_ID.clone()
}

// Global language preference storage. Preserve the detected source language by default;
// translating to English must only happen when the user explicitly selects auto-translate.
static LANGUAGE_PREFERENCE: std::sync::LazyLock<StdMutex<String>> =
    std::sync::LazyLock::new(|| StdMutex::new(DEFAULT_TRANSCRIPTION_LANGUAGE.to_string()));

#[tauri::command]
async fn start_recording<R: Runtime>(
    app: AppHandle<R>,
    mic_device_name: Option<String>,
    system_device_name: Option<String>,
    meeting_name: Option<String>,
) -> Result<(), String> {
    log_info!("🔥 CALLED start_recording with meeting: {:?}", meeting_name);
    log_info!(
        "📋 Backend received parameters - mic: {:?}, system: {:?}, meeting: {:?}",
        mic_device_name,
        system_device_name,
        meeting_name
    );

    if is_recording().await {
        return Err("Recording already in progress".to_string());
    }

    // Call the actual audio recording system with meeting name
    match audio::recording_commands::start_recording_with_devices_and_meeting(
        app.clone(),
        mic_device_name,
        system_device_name,
        meeting_name.clone(),
    )
    .await
    {
        Ok(_) => {
            RECORDING_FLAG.store(true, Ordering::SeqCst);
            tray::update_tray_menu(&app);

            log_info!("Recording started successfully");

            // Show recording started notification through NotificationManager
            // This respects user's notification preferences
            let notification_manager_state = app.state::<NotificationManagerState<R>>();
            if let Err(e) = notifications::commands::show_recording_started_notification(
                &app,
                &notification_manager_state,
                meeting_name.clone(),
            )
            .await
            {
                log_error!("Failed to show recording started notification: {}", e);
            } else {
                log_info!("Successfully showed recording started notification");
            }

            Ok(())
        }
        Err(e) => {
            log_error!("Failed to start audio recording: {}", e);
            Err(format!("Failed to start recording: {}", e))
        }
    }
}

#[tauri::command]
async fn stop_recording<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    log_info!("Attempting to stop recording...");

    // Check the actual audio recording system state instead of the flag
    if !audio::recording_commands::is_recording().await {
        log_info!("Recording is already stopped");
        return Ok(());
    }

    // Call the actual audio recording system to stop
    match audio::recording_commands::stop_recording(app.clone()).await {
        Ok(_) => {
            RECORDING_FLAG.store(false, Ordering::SeqCst);
            tray::update_tray_menu(&app);

            // Show recording stopped notification through NotificationManager
            // This respects user's notification preferences
            let notification_manager_state = app.state::<NotificationManagerState<R>>();
            if let Err(e) = notifications::commands::show_recording_stopped_notification(
                &app,
                &notification_manager_state,
            )
            .await
            {
                log_error!("Failed to show recording stopped notification: {}", e);
            } else {
                log_info!("Successfully showed recording stopped notification");
            }

            Ok(())
        }
        Err(e) => {
            log_error!("Failed to stop audio recording: {}", e);
            // Still update the flag even if stopping failed
            RECORDING_FLAG.store(false, Ordering::SeqCst);
            tray::update_tray_menu(&app);
            Err(format!("Failed to stop recording: {}", e))
        }
    }
}

#[tauri::command]
async fn is_recording() -> bool {
    audio::recording_commands::is_recording().await
}

#[tauri::command]
fn read_audio_file(file_path: String) -> Result<Vec<u8>, String> {
    match std::fs::read(&file_path) {
        Ok(data) => Ok(data),
        Err(e) => Err(format!("Failed to read audio file: {}", e)),
    }
}

#[tauri::command]
async fn save_transcript(file_path: String, content: String) -> Result<(), String> {
    log_info!("Saving transcript to: {}", file_path);

    // Ensure parent directory exists
    if let Some(parent) = std::path::Path::new(&file_path).parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory: {}", e))?;
        }
    }

    // Write content to file
    std::fs::write(&file_path, content)
        .map_err(|e| format!("Failed to write transcript: {}", e))?;

    log_info!("Transcript saved successfully");
    Ok(())
}

// Audio level monitoring commands
#[tauri::command]
async fn start_audio_level_monitoring<R: Runtime>(
    app: AppHandle<R>,
    device_names: Vec<String>,
) -> Result<(), String> {
    log_info!(
        "Starting audio level monitoring for devices: {:?}",
        device_names
    );

    audio::simple_level_monitor::start_monitoring(app, device_names)
        .await
        .map_err(|e| format!("Failed to start audio level monitoring: {}", e))
}

#[tauri::command]
async fn stop_audio_level_monitoring() -> Result<(), String> {
    log_info!("Stopping audio level monitoring");

    audio::simple_level_monitor::stop_monitoring()
        .await
        .map_err(|e| format!("Failed to stop audio level monitoring: {}", e))
}

#[tauri::command]
async fn is_audio_level_monitoring() -> bool {
    audio::simple_level_monitor::is_monitoring()
}

// Analytics commands are now handled by analytics::commands module

// Whisper commands are now handled by whisper_engine::commands module

#[tauri::command]
async fn get_audio_devices() -> Result<Vec<AudioDevice>, String> {
    list_audio_devices()
        .await
        .map_err(|e| format!("Failed to list audio devices: {}", e))
}

#[tauri::command]
async fn trigger_microphone_permission() -> Result<bool, String> {
    trigger_audio_permission()
        .map_err(|e| format!("Failed to trigger microphone permission: {}", e))
}

#[tauri::command]
async fn start_recording_with_devices<R: Runtime>(
    app: AppHandle<R>,
    mic_device_name: Option<String>,
    system_device_name: Option<String>,
) -> Result<(), String> {
    start_recording_with_devices_and_meeting(
        app,
        mic_device_name,
        system_device_name,
        None,
        None,
        None,
        None,
    )
    .await
}

#[tauri::command]
async fn start_recording_with_devices_and_meeting<R: Runtime>(
    app: AppHandle<R>,
    mic_device_name: Option<String>,
    system_device_name: Option<String>,
    meeting_name: Option<String>,
    recording_mode: Option<audio::recording_preferences::RecordingMode>,
    template_selection: Option<meeting_context::RecordingTemplateSelection>,
    meeting_context_draft: Option<meeting_context::RecordingMeetingContextDraft>,
) -> Result<(), String> {
    log_info!("🚀 CALLED start_recording_with_devices_and_meeting - Mic: {:?}, System: {:?}, Meeting: {:?}",
             mic_device_name, system_device_name, meeting_name);

    // Clone meeting_name for notification use later
    let meeting_name_for_notification = meeting_name.clone();

    // Resolve the template again in the native layer. The frontend supplies a
    // structured selection and adjustment draft, never an executable prompt or
    // an authoritative profile. Version/hash checks prevent a stale template
    // from being silently mixed into a new recording.
    let resolved_metadata = if template_selection.is_none() && meeting_context_draft.is_none() {
        // Keep the legacy no-template recording path independent of template
        // service initialization. This is both backwards compatible and a
        // useful fallback if the template repository is temporarily unavailable.
        meeting_context::ResolvedRecordingMetadata {
            summary_template: None,
            meeting_context: None,
        }
    } else {
        let template_service_state =
            app.state::<summary::template_commands_v2::TemplateServiceState>();
        let template_service = template_service_state
            .service()
            .map_err(|error| format!("RECORDING_TEMPLATE_SERVICE_UNAVAILABLE: {error:?}"))?;
        meeting_context::resolve_recording_metadata(
            &template_service,
            template_selection,
            meeting_context_draft,
            chrono::Utc::now(),
        )?
    };

    // Call the recording module functions that support meeting names
    let recording_result =
        audio::recording_commands::start_recording_with_devices_meeting_and_metadata(
            app.clone(),
            mic_device_name,
            system_device_name,
            meeting_name,
            recording_mode.unwrap_or_default(),
            resolved_metadata.summary_template,
            resolved_metadata.meeting_context,
        )
        .await;

    match recording_result {
        Ok(_) => {
            log_info!("Recording started successfully via tauri command");

            // Show recording started notification through NotificationManager
            // This respects user's notification preferences
            let notification_manager_state = app.state::<NotificationManagerState<R>>();
            if let Err(e) = notifications::commands::show_recording_started_notification(
                &app,
                &notification_manager_state,
                meeting_name_for_notification.clone(),
            )
            .await
            {
                log_error!("Failed to show recording started notification: {}", e);
            }

            Ok(())
        }
        Err(e) => {
            log_error!("Failed to start recording via tauri command: {}", e);
            Err(e)
        }
    }
}

#[tauri::command]
async fn set_language_preference(language: String) -> Result<(), String> {
    let language = normalize_transcription_language(&language)?;
    let mut lang_pref = LANGUAGE_PREFERENCE
        .lock()
        .map_err(|e| format!("Failed to set language preference: {}", e))?;
    log_info!("Setting language preference to: {}", language);
    *lang_pref = language;
    Ok(())
}

// Internal helper function to get language preference (for use within Rust code)
pub fn get_language_preference_internal() -> Option<String> {
    LANGUAGE_PREFERENCE.lock().ok().map(|lang| lang.clone())
}

fn normalize_transcription_language(language: &str) -> Result<String, String> {
    let normalized = language.trim().to_ascii_lowercase().replace('_', "-");
    let canonical = match normalized.as_str() {
        "zh-cn" | "zh-sg" | "zh-hans" | "cmn" | "cmn-hans" => "zh".to_string(),
        "en-us" | "en-gb" => "en".to_string(),
        "auto" | "auto-translate" => normalized,
        value
            if (2..=3).contains(&value.len())
                && value.bytes().all(|byte| byte.is_ascii_lowercase()) =>
        {
            value.to_string()
        }
        _ => {
            return Err(format!(
                "Unsupported transcription language preference: {}",
                language
            ));
        }
    };

    Ok(canonical)
}

#[cfg(test)]
mod transcription_language_preference_tests {
    use super::{normalize_transcription_language, DEFAULT_TRANSCRIPTION_LANGUAGE};

    #[test]
    fn default_keeps_the_detected_spoken_language() {
        assert_eq!(DEFAULT_TRANSCRIPTION_LANGUAGE, "auto");
    }

    #[test]
    fn simplified_chinese_aliases_resolve_to_whisper_chinese() {
        for alias in ["zh", "zh-CN", "zh_Hans", "cmn-Hans"] {
            assert_eq!(normalize_transcription_language(alias).unwrap(), "zh");
        }
    }

    #[test]
    fn english_translation_requires_the_explicit_mode() {
        assert_eq!(
            normalize_transcription_language("auto-translate").unwrap(),
            "auto-translate"
        );
        assert_eq!(normalize_transcription_language("auto").unwrap(), "auto");
        assert!(normalize_transcription_language("").is_err());
        assert!(normalize_transcription_language("translate-to-chinese").is_err());
    }
}

pub fn run() {
    log::set_max_level(log::LevelFilter::Info);

    let mut builder = tauri::Builder::default();

    #[cfg(any(target_os = "macos", windows, target_os = "linux"))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            log_info!(
                "Second app instance requested with args: {:?}, cwd: {:?}",
                args,
                cwd
            );

            tray::focus_main_window(app);
        }));
    }

    builder
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(Arc::new(RwLock::new(
            None::<notifications::manager::NotificationManager<tauri::Wry>>,
        )) as NotificationManagerState<tauri::Wry>)
        .manage(audio::init_system_audio_state())
        .manage(summary::summary_engine::ModelManagerState(Arc::new(
            tokio::sync::Mutex::new(None),
        )))
        .setup(|_app| {
            log::info!("Application setup complete");

            // Resolve storage exactly once before any model manager starts.
            // With no completed migration preference this intentionally keeps
            // the existing app-data models directory.
            let storage_layout = storage::resolve_layout_for_app(_app.handle())?;
            log::info!(
                "Resolved storage layout: source={:?}, root={}",
                storage_layout.source(),
                storage_layout.active_storage_root().display()
            );
            let whisper_models_dir = storage_layout.whisper_models_dir().to_path_buf();
            let common_models_root = storage_layout.models_root().to_path_buf();
            let moss_task_cache = storage_layout
                .active_storage_root()
                .join("cache")
                .join("moss-tasks");
            _app.manage(storage::StorageLayoutState::new(storage_layout));
            _app.manage(storage::coordinator::MigrationCoordinator::production());
            _app.manage(Arc::new(moss_helper::MossHelperManager::new(
                moss_helper::MossHelperManager::resolve_helper_binary(),
                moss_task_cache,
            )));
            _app.manage(moss_review::MossReviewState::new());
            _app.manage(
                whisper_engine::parallel_commands::ParallelProcessorState::new(
                    whisper_models_dir.clone(),
                ),
            );

            // Load the persisted native UI locale before creating tray or
            // notification surfaces. React will reconcile system-language
            // preferences through set_ui_locale after hydration.
            _app.manage(i18n::NativeI18nState::load(_app.handle()));

            // Initialize system tray
            if let Err(e) = tray::create_tray(_app.handle()) {
                log::error!("Failed to create system tray: {}", e);
            }

            // Initialize notification system with proper defaults
            log::info!("Initializing notification system...");
            let app_for_notif = _app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let notif_state = app_for_notif.state::<NotificationManagerState<tauri::Wry>>();
                match notifications::commands::initialize_notification_manager(
                    app_for_notif.clone(),
                )
                .await
                {
                    Ok(manager) => {
                        // Set default consent and permissions on first launch
                        if let Err(e) = manager.set_consent(true).await {
                            log::error!("Failed to set initial consent: {}", e);
                        }
                        if let Err(e) = manager.request_permission().await {
                            log::error!("Failed to request initial permission: {}", e);
                        }

                        // Store the initialized manager
                        let mut state_lock = notif_state.write().await;
                        *state_lock = Some(manager);
                        log::info!("Notification system initialized with default permissions");
                    }
                    Err(e) => {
                        log::error!("Failed to initialize notification manager: {}", e);
                    }
                }
            });

            // All model engines receive paths from the resolved StorageLayout.
            whisper_engine::commands::set_models_directory(whisper_models_dir)
                .map_err(std::io::Error::other)?;

            // Whisper stays lazy. SenseVoice is the default provider, and
            // initializing Whisper here needlessly loads native CPU code on
            // machines that never selected Whisper. It also made an old
            // saved Whisper choice capable of crashing the process during
            // startup on CPUs missing the model's instruction set. The
            // Whisper settings flow calls `whisper_init` only after the user
            // explicitly selects a Whisper model.

            // ParakeetEngine appends its `parakeet` subdirectory to this root.
            parakeet_engine::commands::set_models_directory(common_models_root.clone())
                .map_err(std::io::Error::other)?;

            // Initialize Parakeet engine on startup
            tauri::async_runtime::spawn(async {
                if let Err(e) = parakeet_engine::commands::parakeet_init().await {
                    log::error!("Failed to initialize Parakeet engine on startup: {}", e);
                }
            });

            // SenseVoiceEngine appends its `sensevoice` subdirectory to this root.
            sensevoice_engine::commands::set_models_directory(common_models_root)
                .map_err(std::io::Error::other)?;

            // Initialize SenseVoice engine on startup (model files are only
            // downloaded on user action, so this never touches the network).
            tauri::async_runtime::spawn(async {
                if let Err(e) = sensevoice_engine::commands::sensevoice_init().await {
                    log::error!("Failed to initialize SenseVoice engine on startup: {}", e);
                }
            });

            // Initialize ModelManager for summary engine (async, non-blocking)
            let app_handle_for_model_manager = _app.handle().clone();
            tauri::async_runtime::spawn(async move {
                match summary::summary_engine::commands::init_model_manager_at_startup(
                    &app_handle_for_model_manager,
                )
                .await
                {
                    Ok(_) => log::info!("ModelManager initialized successfully at startup"),
                    Err(e) => {
                        log::warn!("Failed to initialize ModelManager at startup: {}", e);
                        log::warn!("ModelManager will be lazy-initialized on first use");
                    }
                }
            });

            // Trigger system audio permission request on startup (similar to microphone permission)
            // #[cfg(target_os = "macos")]
            // {
            //     tauri::async_runtime::spawn(async {
            //         if let Err(e) = audio::permissions::trigger_system_audio_permission() {
            //             log::warn!("Failed to trigger system audio permission: {}", e);
            //         }
            //     });
            // }

            // Initialize database (handles first launch detection and conditional setup)
            tauri::async_runtime::block_on(async {
                database::setup::initialize_database_on_startup(&_app.handle()).await
            })
            .expect("Failed to initialize database");

            // Warm the configured transcription model after the database is ready.
            // This remains non-blocking for window startup, but removes model file
            // loading from the user's first Start Recording click in normal use.
            let app_handle_for_transcription_warmup = _app.handle().clone();
            tauri::async_runtime::spawn(async move {
                log::info!("Prewarming configured transcription model...");
                match audio::transcription::validate_transcription_model_ready(
                    &app_handle_for_transcription_warmup,
                )
                .await
                {
                    Ok(()) => log::info!("Configured transcription model prewarm completed"),
                    Err(e) => log::warn!(
                        "Configured transcription model prewarm skipped or failed: {}",
                        e
                    ),
                }
            });

            // Initialize bundled templates directory for dynamic template discovery
            log::info!("Initializing bundled templates directory...");
            let bundled_templates_dir =
                if let Ok(resource_path) = _app.handle().path().resource_dir() {
                    let templates_dir = resource_path.join("templates");
                    log::info!(
                        "Setting bundled templates directory to: {:?}",
                        templates_dir
                    );
                    summary::templates::set_bundled_templates_dir(templates_dir.clone());
                    Some(templates_dir)
                } else {
                    log::warn!("Failed to resolve resource directory for templates");
                    None
                };
            _app.manage(
                summary::template_commands_v2::TemplateServiceState::initialize(
                    bundled_templates_dir,
                ),
            );

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    if let Err(e) = window.hide() {
                        log::error!("Failed to hide main window on close request: {}", e);
                    } else {
                        log::info!("Main window hidden to tray on close request");
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_app_session_id,
            moss_helper::commands::moss_helper_probe,
            moss_helper::commands::moss_helper_transcribe_file,
            moss_helper::commands::moss_helper_cancel,
            moss_helper::commands::moss_helper_status,
            moss_review::api_moss_get_system_status,
            moss_review::api_moss_get_workspace,
            moss_review::api_moss_start_run,
            moss_review::api_moss_cancel_run,
            moss_review::api_moss_save_speaker_binding,
            moss_review::api_moss_save_segment_override,
            moss_review::api_moss_set_correction_state,
            moss_review::api_moss_update_candidate_segment,
            moss_review::api_moss_activate_candidate,
            moss_review::api_moss_rollback_activation,
            i18n::get_ui_locale,
            i18n::set_ui_locale,
            storage::commands::get_storage_status,
            storage::commands::validate_storage_target,
            storage::commands::get_storage_migration_status,
            storage::commands::preflight_storage_migration,
            storage::commands::start_storage_migration,
            storage::commands::cancel_storage_migration,
            storage::commands::resume_storage_migration,
            storage::commands::get_storage_operation_lock_status,
            start_recording,
            audio::recording_commands::start_controlled_realtime_input,
            stop_recording,
            is_recording,
            audio::recording_commands::get_transcription_status,
            read_audio_file,
            save_transcript,
            analytics::commands::init_analytics,
            analytics::commands::disable_analytics,
            analytics::commands::track_event,
            analytics::commands::identify_user,
            analytics::commands::track_meeting_started,
            analytics::commands::track_recording_started,
            analytics::commands::track_recording_stopped,
            analytics::commands::track_meeting_deleted,
            analytics::commands::track_settings_changed,
            analytics::commands::track_feature_used,
            analytics::commands::is_analytics_enabled,
            analytics::commands::start_analytics_session,
            analytics::commands::end_analytics_session,
            analytics::commands::track_daily_active_user,
            analytics::commands::track_user_first_launch,
            analytics::commands::is_analytics_session_active,
            analytics::commands::track_summary_generation_started,
            analytics::commands::track_summary_generation_completed,
            analytics::commands::track_summary_regenerated,
            analytics::commands::track_model_changed,
            analytics::commands::track_custom_prompt_used,
            analytics::commands::track_meeting_ended,
            analytics::commands::track_analytics_enabled,
            analytics::commands::track_analytics_disabled,
            analytics::commands::track_analytics_transparency_viewed,
            whisper_engine::commands::whisper_init,
            whisper_engine::commands::whisper_get_available_models,
            whisper_engine::commands::whisper_load_model,
            whisper_engine::commands::whisper_get_current_model,
            whisper_engine::commands::whisper_is_model_loaded,
            whisper_engine::commands::whisper_has_available_models,
            whisper_engine::commands::whisper_validate_model_ready,
            whisper_engine::commands::whisper_transcribe_audio,
            whisper_engine::commands::whisper_get_models_directory,
            whisper_engine::commands::whisper_download_model,
            whisper_engine::commands::whisper_cancel_download,
            whisper_engine::commands::whisper_delete_corrupted_model,
            // Parakeet engine commands
            parakeet_engine::commands::parakeet_init,
            sensevoice_engine::commands::sensevoice_init,
            sensevoice_engine::commands::sensevoice_get_available_models,
            sensevoice_engine::commands::sensevoice_load_model,
            sensevoice_engine::commands::sensevoice_is_model_loaded,
            sensevoice_engine::commands::sensevoice_get_current_model,
            sensevoice_engine::commands::sensevoice_get_models_directory,
            sensevoice_engine::commands::sensevoice_download_model,
            sensevoice_engine::commands::sensevoice_get_download_state,
            sensevoice_engine::commands::sensevoice_delete_model,
            sensevoice_engine::commands::sensevoice_transcribe_audio,
            sensevoice_engine::commands::sensevoice_validate_model_ready,
            parakeet_engine::commands::parakeet_get_available_models,
            parakeet_engine::commands::parakeet_load_model,
            parakeet_engine::commands::parakeet_get_current_model,
            parakeet_engine::commands::parakeet_is_model_loaded,
            parakeet_engine::commands::parakeet_has_available_models,
            parakeet_engine::commands::parakeet_validate_model_ready,
            parakeet_engine::commands::parakeet_transcribe_audio,
            parakeet_engine::commands::parakeet_get_models_directory,
            parakeet_engine::commands::parakeet_download_model,
            parakeet_engine::commands::parakeet_retry_download,
            parakeet_engine::commands::parakeet_cancel_download,
            parakeet_engine::commands::parakeet_delete_corrupted_model,
            parakeet_engine::commands::open_parakeet_models_folder,
            // Parallel processing commands
            whisper_engine::parallel_commands::initialize_parallel_processor,
            whisper_engine::parallel_commands::start_parallel_processing,
            whisper_engine::parallel_commands::pause_parallel_processing,
            whisper_engine::parallel_commands::resume_parallel_processing,
            whisper_engine::parallel_commands::stop_parallel_processing,
            whisper_engine::parallel_commands::get_parallel_processing_status,
            whisper_engine::parallel_commands::get_system_resources,
            whisper_engine::parallel_commands::check_resource_constraints,
            whisper_engine::parallel_commands::calculate_optimal_workers,
            whisper_engine::parallel_commands::prepare_audio_chunks,
            whisper_engine::parallel_commands::test_parallel_processing_setup,
            get_audio_devices,
            trigger_microphone_permission,
            start_recording_with_devices,
            start_recording_with_devices_and_meeting,
            start_audio_level_monitoring,
            stop_audio_level_monitoring,
            is_audio_level_monitoring,
            // Recording pause/resume commands
            audio::recording_commands::pause_recording,
            audio::recording_commands::resume_recording,
            audio::recording_commands::is_recording_paused,
            audio::recording_commands::get_recording_state,
            audio::recording_commands::get_meeting_folder_path,
            // Reload sync commands (retrieve transcript history and meeting name)
            audio::recording_commands::get_transcript_history,
            audio::recording_commands::get_recording_meeting_name,
            // Device monitoring commands (AirPods/Bluetooth disconnect/reconnect)
            audio::recording_commands::poll_audio_device_events,
            audio::recording_commands::get_reconnection_status,
            audio::recording_commands::attempt_device_reconnect,
            // Playback device detection (Bluetooth warning)
            audio::recording_commands::get_active_audio_output,
            // Audio recovery commands (for transcript recovery feature)
            audio::incremental_saver::recover_audio_from_checkpoints,
            audio::incremental_saver::cleanup_checkpoints,
            audio::incremental_saver::has_audio_checkpoints,
            console_utils::show_console,
            console_utils::hide_console,
            console_utils::toggle_console,
            ollama::get_ollama_models,
            ollama::pull_ollama_model,
            ollama::delete_ollama_model,
            ollama::get_ollama_model_context,
            openai::openai::get_openai_models,
            anthropic::anthropic::get_anthropic_models,
            groq::groq::get_groq_models,
            api::api_get_meetings,
            api::api_search_transcripts,
            api::api_get_profile,
            api::api_save_profile,
            api::api_update_profile,
            api::api_get_model_config,
            api::api_save_model_config,
            api::api_get_api_key,
            // api::api_get_auto_generate_setting,
            // api::api_save_auto_generate_setting,
            api::api_get_transcript_config,
            api::api_save_transcript_config,
            api::api_get_transcript_api_key,
            api::api_delete_meeting,
            api::api_get_meeting,
            api::api_get_meeting_metadata,
            api::api_get_meeting_transcripts,
            api::api_save_meeting_title,
            api::api_update_transcript_segment,
            api::api_save_transcript,
            api::open_meeting_folder,
            api::test_backend_connection,
            api::debug_backend_connection,
            api::open_external_url,
            // Custom OpenAI commands
            api::api_save_custom_openai_config,
            api::api_get_custom_openai_config,
            api::api_test_custom_openai_connection,
            // Summary commands
            summary::commands::api_begin_summary_measurement,
            summary::commands::api_process_transcript,
            summary::commands::api_get_summary,
            summary::commands::api_record_summary_frontend_stage,
            summary::commands::api_record_summary_page_completion,
            summary::commands::api_finish_summary_measurement,
            summary::commands::api_save_meeting_summary,
            summary::commands::api_list_manual_summary_revisions,
            summary::commands::api_restore_manual_summary_revision,
            summary::commands::api_get_meeting_summary_language,
            summary::commands::api_save_meeting_summary_language,
            summary::commands::api_get_meeting_detected_summary_language,
            summary::commands::api_save_meeting_detected_summary_language,
            summary::commands::api_detect_transcript_summary_language,
            summary::commands::api_cancel_summary,
            summary::generation_lifecycle::api_list_summary_generation_history,
            summary::generation_lifecycle::api_get_summary_generation_snapshot,
            summary::generation_lifecycle::api_preview_template_snapshot_cleanup,
            summary::generation_lifecycle::api_execute_template_snapshot_cleanup,
            // Template commands
            summary::template_commands::api_list_templates,
            summary::template_commands::api_get_template_details,
            summary::template_commands::api_validate_template,
            // Template v2 commands; old commands remain for one compatibility release.
            summary::template_commands_v2::api_get_templates_directory,
            summary::template_commands_v2::api_open_templates_directory,
            summary::template_commands_v2::api_preview_template_documents,
            summary::template_commands_v2::api_preview_template_imports,
            summary::template_commands_v2::api_get_template_import_job,
            summary::template_commands_v2::api_cancel_template_import_job,
            summary::template_commands_v2::api_cancel_template_import_item,
            summary::template_commands_v2::api_export_template_json,
            summary::template_commands_v2::api_preview_template_pack_export,
            summary::template_commands_v2::api_export_template_pack,
            summary::template_commands_v2::api_preview_template_pack_import,
            summary::template_commands_v2::api_plan_template_pack_import,
            summary::template_commands_v2::api_execute_template_pack_import,
            summary::template_commands_v2::api_cancel_template_pack_import,
            summary::template_commands_v2::api_list_templates_v2,
            summary::template_commands_v2::api_get_template_v2,
            summary::template_commands_v2::api_validate_template_v2,
            summary::template_commands_v2::api_create_template,
            summary::template_commands_v2::api_update_template,
            summary::template_commands_v2::api_duplicate_template,
            summary::template_commands_v2::api_get_template_usage,
            summary::template_commands_v2::api_list_meeting_template_snapshots,
            summary::template_commands_v2::api_delete_template,
            summary::template_commands_v2::api_list_deleted_templates,
            summary::template_commands_v2::api_restore_template,
            summary::template_commands_v2::api_purge_template,
            summary::template_commands_v2::api_get_default_template,
            summary::template_commands_v2::api_set_default_template,
            summary::template_commands_v2::api_get_meeting_template_preference,
            summary::template_commands_v2::api_save_meeting_template_preference,
            // Built-in AI commands
            summary::summary_engine::commands::builtin_ai_list_models,
            summary::summary_engine::commands::builtin_ai_get_model_info,
            summary::summary_engine::commands::builtin_ai_download_model,
            summary::summary_engine::commands::builtin_ai_cancel_download,
            summary::summary_engine::commands::builtin_ai_delete_model,
            summary::summary_engine::commands::builtin_ai_is_model_ready,
            summary::summary_engine::commands::builtin_ai_get_available_summary_model,
            summary::summary_engine::commands::builtin_ai_get_recommended_model,
            openrouter::get_openrouter_models,
            audio::recording_preferences::get_recording_preferences,
            audio::recording_preferences::set_recording_preferences,
            audio::recording_preferences::get_default_recordings_folder_path,
            audio::recording_preferences::open_recordings_folder,
            audio::recording_preferences::select_recording_folder,
            audio::recording_preferences::get_available_audio_backends,
            audio::recording_preferences::get_current_audio_backend,
            audio::recording_preferences::set_audio_backend,
            audio::recording_preferences::get_audio_backend_info,
            // Language preference commands
            set_language_preference,
            // Notification system commands
            notifications::commands::get_notification_settings,
            notifications::commands::set_notification_settings,
            notifications::commands::request_notification_permission,
            notifications::commands::show_notification,
            notifications::commands::show_test_notification,
            notifications::commands::is_dnd_active,
            notifications::commands::get_system_dnd_status,
            notifications::commands::set_manual_dnd,
            notifications::commands::set_notification_consent,
            notifications::commands::clear_notifications,
            notifications::commands::is_notification_system_ready,
            notifications::commands::initialize_notification_manager_manual,
            notifications::commands::test_notification_with_auto_consent,
            notifications::commands::get_notification_stats,
            // System audio capture commands
            audio::system_audio_commands::start_system_audio_capture_command,
            audio::system_audio_commands::list_system_audio_devices_command,
            audio::system_audio_commands::check_system_audio_permissions_command,
            audio::system_audio_commands::start_system_audio_monitoring,
            audio::system_audio_commands::stop_system_audio_monitoring,
            audio::system_audio_commands::get_system_audio_monitoring_status,
            // Screen Recording permission commands
            audio::permissions::check_screen_recording_permission_command,
            audio::permissions::request_screen_recording_permission_command,
            audio::permissions::trigger_system_audio_permission_command,
            // Database import commands
            database::commands::check_first_launch,
            database::commands::select_legacy_database_path,
            database::commands::detect_legacy_database,
            database::commands::check_default_legacy_database,
            database::commands::check_homebrew_database,
            database::commands::import_and_initialize_database,
            database::commands::initialize_fresh_database,
            // Database and Models path commands
            database::commands::get_database_directory,
            database::commands::open_database_folder,
            whisper_engine::commands::open_models_folder,
            // Onboarding commands
            onboarding::get_onboarding_status,
            onboarding::save_onboarding_status_cmd,
            onboarding::reset_onboarding_status_cmd,
            onboarding::complete_onboarding,
            // System settings commands
            #[cfg(target_os = "macos")]
            utils::open_system_settings,
            // Retranscription commands
            audio::retranscription::start_retranscription_command,
            audio::retranscription::finalize_recording_transcript_command,
            audio::retranscription::cancel_retranscription_command,
            audio::retranscription::is_retranscription_in_progress_command,
            audio::retranscription::get_retranscription_status_command,
            transcript_file_sync::api_transcript_file_sync_pending,
            transcript_file_sync::api_retry_transcript_file_sync,
            transcript_revision::api_list_transcript_backups,
            transcript_revision::api_get_transcript_revision_diff,
            transcript_revision::api_restore_transcript_revision,
            transcript_revision::api_apply_meeting_text_corrections,
            transcript_proofread::api_review_transcript_with_llm,
            transcript_proofread::api_apply_transcript_proofread_edits,
            transcript_proofread::api_list_proofread_models,
            transcript_proofread::api_get_proofread_target,
            transcript_proofread::api_set_proofread_target,
            // Import audio commands
            audio::import::select_and_validate_audio_command,
            audio::import::validate_audio_file_command,
            audio::import::start_import_audio_command,
            audio::import::cancel_import_command,
            audio::import::is_import_in_progress_command,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, event| {
            match event {
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen { .. } => {
                    tray::focus_main_window(_app_handle);
                }
                tauri::RunEvent::Exit => {
                    log::info!("Application exiting, cleaning up resources...");
                    if let Some(manager) =
                        _app_handle.try_state::<Arc<moss_helper::MossHelperManager>>()
                    {
                        manager.shutdown_all();
                    }
                    tauri::async_runtime::block_on(async {
                        // Clean up database connection and checkpoint WAL
                        if let Some(app_state) = _app_handle.try_state::<state::AppState>() {
                            log::info!("Starting database cleanup...");
                            if let Err(e) = app_state.db_manager.cleanup().await {
                                log::error!("Failed to cleanup database: {}", e);
                            } else {
                                log::info!("Database cleanup completed successfully");
                            }
                        } else {
                            log::warn!(
                                "AppState not available for database cleanup (likely first launch)"
                            );
                        }

                        // Clean up sidecar
                        log::info!("Cleaning up sidecar...");
                        if let Err(e) = summary::summary_engine::force_shutdown_sidecar().await {
                            log::error!("Failed to force shutdown sidecar: {}", e);
                        }
                    });
                    log::info!("Application cleanup complete");
                }
                _ => {}
            }
        });
}
