//! Tauri commands for the SenseVoice engine, mirroring the Parakeet surface.

use super::engine::SenseVoiceEngine;
use super::model::{self, ModelInfo};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{command, AppHandle, Emitter, Runtime};

#[derive(Clone, serde::Serialize)]
pub struct DownloadState {
    status: String,
    downloaded_bytes: u64,
    total_bytes: u64,
}
static DOWNLOAD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static DOWNLOAD_STATE: Mutex<Option<DownloadState>> = Mutex::new(None);

#[command]
pub async fn sensevoice_get_download_state() -> Result<DownloadState, String> {
    if let Some(state) = DOWNLOAD_STATE.lock().map_err(|e| e.to_string())?.clone() {
        if state.status == "downloading" || state.status == "error" { return Ok(state); }
    }
    let engine = get_engine().ok_or("SenseVoice engine not initialized")?;
    let info = model::model_info(engine.models_root(), crate::config::DEFAULT_SENSEVOICE_MODEL).map_err(|e| e.to_string())?;
    Ok(DownloadState {
        status: match info.status { model::ModelStatus::Available => "available", model::ModelStatus::Partial => "partial", model::ModelStatus::Missing => "missing" }.to_string(),
        downloaded_bytes: info.size_bytes,
        total_bytes: model::model_files().iter().map(|file| file.expected_bytes).sum(),
    })
}

/// Global SenseVoice engine instance.
pub static SENSEVOICE_ENGINE: Mutex<Option<Arc<SenseVoiceEngine>>> = Mutex::new(None);

static MODELS_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Set the common models root resolved by StorageLayout.
pub fn set_models_directory(models_dir: PathBuf) -> Result<(), String> {
    std::fs::create_dir_all(&models_dir)
        .map_err(|error| format!("Failed to create models root for SenseVoice: {error}"))?;
    log::info!("SenseVoice models root set to: {}", models_dir.display());
    let mut guard = MODELS_DIR
        .lock()
        .map_err(|_| "SenseVoice models directory lock is poisoned".to_string())?;
    *guard = Some(models_dir);
    Ok(())
}

fn get_models_directory() -> Option<PathBuf> {
    MODELS_DIR.lock().ok().and_then(|guard| guard.clone())
}

/// Read the initialized engine, if any.
pub fn get_engine() -> Option<Arc<SenseVoiceEngine>> {
    SENSEVOICE_ENGINE.lock().ok().and_then(|guard| guard.clone())
}

#[command]
pub async fn sensevoice_init() -> Result<(), String> {
    let mut guard = SENSEVOICE_ENGINE.lock().unwrap();
    if guard.is_some() {
        return Ok(());
    }
    let models_root =
        get_models_directory().ok_or_else(|| "SenseVoice models directory not initialized".to_string())?;
    let engine = SenseVoiceEngine::new_with_models_root(models_root)
        .map_err(|error| format!("Failed to initialize SenseVoice engine: {error}"))?;
    *guard = Some(Arc::new(engine));
    Ok(())
}

#[command]
pub async fn sensevoice_get_available_models() -> Result<Vec<ModelInfo>, String> {
    let engine = get_engine().ok_or_else(|| "SenseVoice engine not initialized".to_string())?;
    engine.discover_models().map_err(|error| error.to_string())
}

#[command]
pub async fn sensevoice_load_model(model_name: String) -> Result<(), String> {
    let engine = get_engine().ok_or_else(|| "SenseVoice engine not initialized".to_string())?;
    engine.load_model(&model_name).await.map_err(|error| error.to_string())
}

#[command]
pub async fn sensevoice_is_model_loaded() -> Result<bool, String> {
    let engine = get_engine().ok_or_else(|| "SenseVoice engine not initialized".to_string())?;
    Ok(engine.is_model_loaded().await)
}

#[command]
pub async fn sensevoice_get_current_model() -> Result<Option<String>, String> {
    let engine = get_engine().ok_or_else(|| "SenseVoice engine not initialized".to_string())?;
    Ok(engine.get_current_model().await)
}

#[command]
pub async fn sensevoice_get_models_directory() -> Result<String, String> {
    let engine = get_engine().ok_or_else(|| "SenseVoice engine not initialized".to_string())?;
    Ok(engine.models_directory().to_string_lossy().to_string())
}

#[command]
pub async fn sensevoice_download_model<R: Runtime>(
    app: AppHandle<R>,
    model_name: String,
) -> Result<String, String> {
    let _download_guard = DOWNLOAD_LOCK.try_lock().map_err(|_| "SenseVoice download already in progress")?;
    if model_name != crate::config::DEFAULT_SENSEVOICE_MODEL { return Err("Unknown SenseVoice model".to_string()); }
    let engine = get_engine().ok_or_else(|| "SenseVoice engine not initialized".to_string())?;
    let models_root = engine.models_root().to_path_buf();
    let event_app = app.clone();
    let event_model = model_name.clone();

    *DOWNLOAD_STATE.lock().map_err(|e| e.to_string())? = Some(DownloadState {
        status: "downloading".to_string(), downloaded_bytes: 0,
        total_bytes: model::model_files().iter().map(|file| file.expected_bytes).sum(),
    });
    let result = model::download_model(&models_root, &model_name, move |done, total, file| {
        if let Ok(mut state) = DOWNLOAD_STATE.lock() {
            *state = Some(DownloadState { status: "downloading".to_string(), downloaded_bytes: done, total_bytes: total });
        }
        let _ = event_app.emit(
            "sensevoice-download-progress",
            serde_json::json!({
                "modelName": event_model,
                "downloadedBytes": done,
                "totalBytes": total,
                "file": file,
            }),
        );
    })
    .await;
    if let Ok(mut state) = DOWNLOAD_STATE.lock() {
        if let Some(state) = state.as_mut() { state.status = if result.is_ok() { "available" } else { "error" }.to_string(); }
    }
    let directory = result.map_err(|error| error.to_string())?;

    let _ = app.emit(
        "sensevoice-download-complete",
        serde_json::json!({ "modelName": model_name, "path": directory.to_string_lossy() }),
    );
    Ok(directory.to_string_lossy().to_string())
}

#[command]
pub async fn sensevoice_delete_model(model_name: String) -> Result<String, String> {
    let _download_guard = DOWNLOAD_LOCK.try_lock().map_err(|_| "SenseVoice download in progress")?;
    let engine = get_engine().ok_or_else(|| "SenseVoice engine not initialized".to_string())?;
    engine.delete_model(&model_name).await.map_err(|error| error.to_string())?;
    Ok(format!("Deleted SenseVoice model {model_name}"))
}

#[command]
pub async fn sensevoice_transcribe_audio(audio_data: Vec<f32>) -> Result<String, String> {
    let engine = get_engine().ok_or_else(|| "SenseVoice engine not initialized".to_string())?;
    engine.transcribe(audio_data).await.map_err(|error| error.to_string())
}

#[command]
pub async fn sensevoice_validate_model_ready() -> Result<String, String> {
    let engine = get_engine().ok_or_else(|| "SenseVoice engine not initialized".to_string())?;
    let models = engine.discover_models().map_err(|error| error.to_string())?;
    let available = models
        .iter()
        .find(|model| model.status == model::ModelStatus::Available)
        .ok_or_else(|| "No downloaded SenseVoice model is available".to_string())?;
    if !engine.is_model_loaded().await {
        engine
            .load_model(&available.name)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(available.name.clone())
}
