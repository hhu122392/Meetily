use crate::parakeet_engine::parakeet_engine::{is_download_cancelled, CancelDownloadOutcome};
use crate::sensevoice_engine::{DownloadProgress, ModelInfo, SenseVoiceEngine};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{command, AppHandle, Emitter, Manager, Runtime};

// Global sensevoice engine
pub static SENSEVOICE_ENGINE: Mutex<Option<Arc<SenseVoiceEngine>>> = Mutex::new(None);

pub async fn get_ready_engine<R: Runtime>(app: &AppHandle<R>, model: Option<&str>) -> Result<Arc<SenseVoiceEngine>, String> {
    set_models_directory(app);
    sensevoice_init().await?;
    let engine = SENSEVOICE_ENGINE.lock().map_err(|e| e.to_string())?.clone().ok_or("SenseVoice 未初始化")?;
    let name = model.filter(|s| !s.is_empty()).unwrap_or(crate::config::DEFAULT_SENSEVOICE_MODEL);
    if name != crate::config::DEFAULT_SENSEVOICE_MODEL { return Err(format!("不支持的 SenseVoice 模型：{name}")); }
    engine.discover_models().await.map_err(|e| e.to_string())?;
    engine.load_model(name).await.map_err(|e| e.to_string())?;
    Ok(engine)
}

pub struct SenseVoiceProvider(pub Arc<SenseVoiceEngine>);
#[async_trait::async_trait]
impl crate::audio::transcription::TranscriptionProvider for SenseVoiceProvider {
    async fn transcribe(&self, audio: Vec<f32>, language: Option<String>) -> Result<crate::audio::transcription::TranscriptResult, crate::audio::transcription::TranscriptionError> {
        let text = self.0.transcribe_audio_language(audio, language).await
            .map_err(|e| crate::audio::transcription::TranscriptionError::EngineFailed(e.to_string()))?;
        Ok(crate::audio::transcription::TranscriptResult { text, confidence: None, is_partial: false })
    }
    async fn is_model_loaded(&self) -> bool { self.0.is_model_loaded().await }
    async fn get_current_model(&self) -> Option<String> { self.0.get_current_model().await }
    fn provider_name(&self) -> &'static str { "SenseVoice" }
}

// Global models directory path (set during app initialization)
static MODELS_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Initialize the models directory path using app_data_dir
/// This should be called during app setup before sensevoice_init
pub fn set_models_directory<R: Runtime>(app: &AppHandle<R>) {
    let app_data_dir = app.path().app_data_dir()
        .expect("Failed to get app data dir");

    let models_dir = crate::model_storage::models_directory(&app_data_dir);

    // Create directory if it doesn't exist
    if !models_dir.exists() {
        if let Err(e) = std::fs::create_dir_all(&models_dir) {
            log::error!("Failed to create models directory: {}", e);
            return;
        }
    }

    log::info!("SenseVoice models directory set to: {}", models_dir.display());

    let mut guard = MODELS_DIR.lock().unwrap();
    *guard = Some(models_dir);
}

/// Get the configured models directory
fn get_models_directory() -> Option<PathBuf> {
    MODELS_DIR.lock().unwrap().clone()
}

#[command]
pub async fn sensevoice_init() -> Result<(), String> {
    let mut guard = SENSEVOICE_ENGINE.lock().unwrap();
    if guard.is_some() {
        return Ok(());
    }

    let models_dir = get_models_directory();
    let engine = SenseVoiceEngine::new_sensevoice(models_dir)
        .map_err(|e| format!("Failed to initialize SenseVoice engine: {}", e))?;
    *guard = Some(Arc::new(engine));
    Ok(())
}

#[command]
pub async fn sensevoice_get_available_models() -> Result<Vec<ModelInfo>, String> {
    let engine = {
        let guard = SENSEVOICE_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        engine
            .discover_models()
            .await
            .map_err(|e| format!("Failed to discover SenseVoice models: {}", e))
    } else {
        Err("SenseVoice engine not initialized".to_string())
    }
}

#[command]
pub async fn sensevoice_load_model<R: Runtime>(
    app_handle: AppHandle<R>,
    model_name: String
) -> Result<(), String> {
    let _model_files = crate::model_storage::use_model_files().await?;
    let engine = {
        let guard = SENSEVOICE_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        // Emit model loading started event
        if let Err(e) = app_handle.emit(
            "sensevoice-model-loading-started",
            serde_json::json!({
                "modelName": model_name
            }),
        ) {
            log::error!("Failed to emit sensevoice-model-loading-started event: {}", e);
        }

        let result = engine
            .load_model(&model_name)
            .await
            .map_err(|e| format!("Failed to load SenseVoice model: {}", e));

        // Emit model loading completed/failed event
        if result.is_ok() {
            if let Err(e) = app_handle.emit(
                "sensevoice-model-loading-completed",
                serde_json::json!({
                    "modelName": model_name
                }),
            ) {
                log::error!("Failed to emit sensevoice-model-loading-completed event: {}", e);
            }
        } else if let Err(ref error) = result {
            if let Err(e) = app_handle.emit(
                "sensevoice-model-loading-failed",
                serde_json::json!({
                    "modelName": model_name,
                    "error": error
                }),
            ) {
                log::error!("Failed to emit sensevoice-model-loading-failed event: {}", e);
            }
        }

        result
    } else {
        Err("SenseVoice engine not initialized".to_string())
    }
}

#[command]
pub async fn sensevoice_get_current_model() -> Result<Option<String>, String> {
    let engine = {
        let guard = SENSEVOICE_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        Ok(engine.get_current_model().await)
    } else {
        Err("SenseVoice engine not initialized".to_string())
    }
}

#[command]
pub async fn sensevoice_is_model_loaded() -> Result<bool, String> {
    let engine = {
        let guard = SENSEVOICE_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        Ok(engine.is_model_loaded().await)
    } else {
        Err("SenseVoice engine not initialized".to_string())
    }
}

#[command]
pub async fn sensevoice_has_available_models() -> Result<bool, String> {
    let engine = {
        let guard = SENSEVOICE_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        let models = engine
            .discover_models()
            .await
            .map_err(|e| format!("Failed to discover SenseVoice models: {}", e))?;

        // Check if at least one model is available
        let available_models: Vec<_> = models
            .iter()
            .filter(|model| matches!(model.status, crate::sensevoice_engine::ModelStatus::Available))
            .collect();

        Ok(!available_models.is_empty())
    } else {
        Ok(false)
    }
}

#[command]
pub async fn sensevoice_validate_model_ready() -> Result<String, String> {
    let engine = {
        let guard = SENSEVOICE_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        // Check if a model is currently loaded
        if engine.is_model_loaded().await {
            if let Some(current_model) = engine.get_current_model().await {
                return Ok(current_model);
            }
        }

        // No model loaded, check if any models are available to load
        let models = engine
            .discover_models()
            .await
            .map_err(|e| format!("Failed to discover SenseVoice models: {}", e))?;

        let available_models: Vec<_> = models
            .iter()
            .filter(|model| matches!(model.status, crate::sensevoice_engine::ModelStatus::Available))
            .collect();

        if available_models.is_empty() {
            return Err(
                "No SenseVoice models are available. Please download a model to enable fast transcription."
                    .to_string(),
            );
        }

        // Try to load the first available model (prefer int8 for speed)
        let first_model = available_models.iter()
            .find(|m| m.quantization == crate::sensevoice_engine::QuantizationType::Int8)
            .or_else(|| available_models.first())
            .unwrap();

        engine
            .load_model(&first_model.name)
            .await
            .map_err(|e| format!("Failed to load SenseVoice model {}: {}", first_model.name, e))?;

        Ok(first_model.name.clone())
    } else {
        Err("SenseVoice engine not initialized".to_string())
    }
}

/// Internal version of sensevoice_validate_model_ready that respects user's transcript config
/// This matches whisper_validate_model_ready_with_config for consistency
pub async fn sensevoice_validate_model_ready_with_config<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<String, String> {
    let engine = {
        let guard = SENSEVOICE_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        // Check if a model is currently loaded
        if engine.is_model_loaded().await {
            if let Some(current_model) = engine.get_current_model().await {
                log::info!("SenseVoice model already loaded: {}", current_model);
                return Ok(current_model);
            }
        }

        // No model loaded - try to load user's configured model from transcript config
        let model_to_load = match crate::api::api::api_get_transcript_config(
            app.clone(),
            app.state(),
            None,
        )
        .await
        {
            Ok(Some(config)) => {
                log::info!(
                    "Got transcript config from API - provider: {}, model: {}",
                    config.provider,
                    config.model
                );
                if config.provider == "sensevoice" && !config.model.is_empty() {
                    log::info!("Using user's configured SenseVoice model: {}", config.model);
                    Some(config.model)
                } else {
                    log::info!(
                        "API config uses non-SenseVoice provider ({}) or empty model, will auto-select",
                        config.provider
                    );
                    None
                }
            }
            Ok(None) => {
                log::info!("No transcript config found in API, will auto-select SenseVoice model");
                None
            }
            Err(e) => {
                log::warn!(
                    "Failed to get transcript config from API: {}, will auto-select SenseVoice model",
                    e
                );
                None
            }
        };

        // Check available models
        let models = engine
            .discover_models()
            .await
            .map_err(|e| format!("Failed to discover SenseVoice models: {}", e))?;

        let available_models: Vec<_> = models
            .iter()
            .filter(|model| matches!(model.status, crate::sensevoice_engine::ModelStatus::Available))
            .collect();

        if available_models.is_empty() {
            return Err(
                "No SenseVoice models are available. Please download a model to enable fast transcription."
                    .to_string(),
            );
        }

        // Try to load user's configured model if specified
        let model_name = if let Some(configured_model) = model_to_load {
            // Check if configured model is available
            if available_models.iter().any(|m| m.name == configured_model) {
                log::info!("Loading user's configured SenseVoice model: {}", configured_model);
                configured_model
            } else {
                log::warn!(
                    "Configured SenseVoice model '{}' not found, falling back to first available int8 model",
                    configured_model
                );
                // Prefer int8 quantization for best speed/quality tradeoff
                available_models
                    .iter()
                    .find(|m| m.quantization == crate::sensevoice_engine::QuantizationType::Int8)
                    .or_else(|| available_models.first())
                    .unwrap()
                    .name
                    .clone()
            }
        } else {
            // No configured model, prefer int8 for best speed/quality balance
            log::info!("No configured model, loading first available int8 SenseVoice model");
            available_models
                .iter()
                .find(|m| m.quantization == crate::sensevoice_engine::QuantizationType::Int8)
                .or_else(|| available_models.first())
                .unwrap()
                .name
                .clone()
        };

        engine
            .load_model(&model_name)
            .await
            .map_err(|e| format!("Failed to load SenseVoice model {}: {}", model_name, e))?;

        Ok(model_name)
    } else {
        Err("SenseVoice engine not initialized".to_string())
    }
}

#[command]
pub async fn sensevoice_transcribe_audio(audio_data: Vec<f32>) -> Result<String, String> {
    let _model_files = crate::model_storage::use_model_files().await?;
    let engine = {
        let guard = SENSEVOICE_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        engine
            .transcribe_audio(audio_data)
            .await
            .map_err(|e| format!("SenseVoice transcription failed: {}", e))
    } else {
        Err("SenseVoice engine not initialized".to_string())
    }
}

#[command]
pub async fn sensevoice_get_models_directory() -> Result<String, String> {
    let engine = {
        let guard = SENSEVOICE_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        let path = engine.get_models_directory().await;
        Ok(path.to_string_lossy().to_string())
    } else {
        Err("SenseVoice engine not initialized".to_string())
    }
}

#[command]
pub async fn sensevoice_download_model<R: Runtime>(
    app_handle: AppHandle<R>,
    model_name: String,
    source: Option<String>,
) -> Result<(), String> {
    let _model_files = crate::model_storage::use_model_files().await?;
    let engine = {
        let guard = SENSEVOICE_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        // Create progress callback that emits detailed events
        let app_handle_clone = app_handle.clone();
        let model_name_clone = model_name.clone();

        let progress_callback = Box::new(move |progress: DownloadProgress| {
            log::info!(
                "SenseVoice download progress for {}: {:.1} MB / {:.1} MB ({:.1} MB/s) - {}%",
                model_name_clone, progress.downloaded_mb, progress.total_mb,
                progress.speed_mbps, progress.percent
            );

            // Emit download progress event with detailed info
            if let Err(e) = app_handle_clone.emit(
                "sensevoice-model-download-progress",
                serde_json::json!({
                    "modelName": model_name_clone,
                    "progress": progress.percent,
                    "downloaded_bytes": progress.downloaded_bytes,
                    "total_bytes": progress.total_bytes,
                    "downloaded_mb": progress.downloaded_mb,
                    "total_mb": progress.total_mb,
                    "speed_mbps": progress.speed_mbps,
                    "status": if progress.percent == 100 { "completed" } else { "downloading" }
                }),
            ) {
                log::error!("Failed to emit sensevoice download progress event: {}", e);
            }
        });

        // Ensure models are discovered before downloading
        // This populates available_models so we don't get "Model not found" error
        if let Err(e) = engine.discover_models().await {
            log::warn!("Failed to discover models before download: {}", e);
            // Continue anyway, maybe it will work if the model is already known
        }

        let result = engine
            .download_model_detailed_with_source(&model_name, source.as_deref(), Some(progress_callback))
            .await;

        match result {
            Ok(()) => {
                // Emit completion event
                if let Err(e) = app_handle.emit(
                    "sensevoice-model-download-complete",
                    serde_json::json!({
                        "modelName": model_name
                    }),
                ) {
                    log::error!("Failed to emit sensevoice download complete event: {}", e);
                }

                // Update tray menu to reflect model is now available
                log::info!("SenseVoice model download complete - updating tray menu");
                crate::tray::update_tray_menu(&app_handle);

                Ok(())
            }
            Err(error) if is_download_cancelled(&error) => {
                if let Err(emit_error) = app_handle.emit(
                    "sensevoice-model-download-progress",
                    serde_json::json!({
                        "modelName": model_name,
                        "progress": 0,
                        "status": "cancelled"
                    }),
                ) {
                    log::error!(
                        "Failed to emit SenseVoice cancellation event: {}",
                        emit_error
                    );
                }
                log::info!("SenseVoice download cancelled: {}", model_name);
                Ok(())
            }
            Err(error) => {
                if let Err(emit_error) = app_handle.emit(
                    "sensevoice-model-download-error",
                    serde_json::json!({
                        "modelName": model_name,
                        "error": error.to_string()
                    }),
                ) {
                    log::error!("Failed to emit sensevoice download error event: {}", emit_error);
                }
                Err(format!("Failed to download SenseVoice model: {}", error))
            }
        }
    } else {
        Err("SenseVoice engine not initialized".to_string())
    }
}

#[command]
pub async fn sensevoice_cancel_download(
    model_name: String,
) -> Result<CancelDownloadOutcome, String> {
    let engine = {
        let guard = SENSEVOICE_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    let engine = engine.ok_or_else(|| "SenseVoice engine not initialized".to_string())?;
    engine
        .cancel_download(&model_name)
        .await
        .map_err(|error| format!("Failed to cancel SenseVoice download: {}", error))
}

#[command]
pub async fn sensevoice_retry_download<R: Runtime>(
    app_handle: AppHandle<R>,
    model_name: String,
    source: Option<String>,
) -> Result<(), String> {
    log::info!("Retrying download for: {}", model_name);

    let engine = {
        let guard = SENSEVOICE_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if engine.is_some() {
        // Retry uses the normal download entrypoint, whose owner reservation rejects
        // a second writer until cancellation cleanup has completed.
        sensevoice_download_model(app_handle, model_name, source).await
    } else {
        Err("SenseVoice engine not initialized".to_string())
    }
}

#[command]
pub async fn sensevoice_delete_corrupted_model(model_name: String) -> Result<String, String> {
    let _model_files = crate::model_storage::use_model_files().await?;
    let engine = {
        let guard = SENSEVOICE_ENGINE.lock().unwrap();
        guard.as_ref().cloned()
    };

    if let Some(engine) = engine {
        engine
            .delete_model(&model_name)
            .await
            .map_err(|e| format!("Failed to delete SenseVoice model: {}", e))
    } else {
        Err("SenseVoice engine not initialized".to_string())
    }
}

/// Open the SenseVoice models folder in the system file explorer
#[command]
pub async fn open_sensevoice_models_folder() -> Result<(), String> {
    let models_dir = get_models_directory()
        .ok_or_else(|| "SenseVoice models directory not initialized".to_string())?
        .join("sensevoice");

    // Ensure directory exists before trying to open it
    if !models_dir.exists() {
        std::fs::create_dir_all(&models_dir)
            .map_err(|e| format!("Failed to create directory: {}", e))?;
    }

    let folder_path = models_dir.to_string_lossy().to_string();

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&folder_path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&folder_path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&folder_path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }

    log::info!("Opened SenseVoice models folder: {}", folder_path);
    Ok(())
}
