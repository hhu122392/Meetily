use log::info;
use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::{resolve_store_path, StoreExt};

use anyhow::{anyhow, Result};
#[cfg(target_os = "macos")]
use log::{error, warn};

#[cfg(target_os = "macos")]
use crate::audio::capture::AudioCaptureBackend;

const RECORDING_PREFERENCES_STORE: &str = "recording_preferences.json";

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecordingMode {
    MicrophoneAndSystem,
    MicrophoneOnly,
    SystemOnly,
}

impl Default for RecordingMode {
    fn default() -> Self {
        Self::MicrophoneAndSystem
    }
}

impl RecordingMode {
    pub fn uses_microphone(self) -> bool {
        matches!(self, Self::MicrophoneAndSystem | Self::MicrophoneOnly)
    }

    pub fn uses_system_audio(self) -> bool {
        matches!(self, Self::MicrophoneAndSystem | Self::SystemOnly)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RecordingPreferences {
    pub save_folder: PathBuf,
    pub auto_save: bool,
    pub file_format: String,
    #[serde(default)]
    pub preferred_mic_device: Option<String>,
    #[serde(default)]
    pub preferred_system_device: Option<String>,
    #[serde(default)]
    pub recording_mode: RecordingMode,
    #[cfg(target_os = "macos")]
    #[serde(default)]
    pub system_audio_backend: Option<String>,
}

impl Default for RecordingPreferences {
    fn default() -> Self {
        Self {
            save_folder: get_default_recordings_folder(),
            auto_save: true,
            file_format: "mp4".to_string(),
            preferred_mic_device: None,
            preferred_system_device: None,
            recording_mode: RecordingMode::default(),
            #[cfg(target_os = "macos")]
            system_audio_backend: Some("coreaudio".to_string()),
        }
    }
}

/// Get the default recordings folder based on platform
pub fn get_default_recordings_folder() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        // Windows: %USERPROFILE%\Music\meetily-recordings
        if let Some(music_dir) = dirs::audio_dir() {
            music_dir.join("meetily-recordings")
        } else {
            // Fallback to Documents if Music folder is not available
            dirs::document_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("meetily-recordings")
        }
    }

    #[cfg(target_os = "macos")]
    {
        // macOS: ~/Movies/meetily-recordings
        if let Some(movies_dir) = dirs::video_dir() {
            movies_dir.join("meetily-recordings")
        } else {
            // Fallback to Documents if Movies folder is not available
            dirs::document_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("meetily-recordings")
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        // Linux/Others: ~/Documents/meetily-recordings
        dirs::document_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("meetily-recordings")
    }
}

/// Validate the selected recording root before a recording or import begins.
/// A real write probe is used because existence alone does not prove that the
/// current process can persist a meeting there.
pub fn validate_recordings_directory(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Err(anyhow!(
            "recording save folder must be an absolute path: {}",
            path.display()
        ));
    }

    std::fs::create_dir_all(path).map_err(|error| {
        anyhow!(
            "failed to create recording save folder {}: {error}",
            path.display()
        )
    })?;
    if !path.is_dir() {
        return Err(anyhow!(
            "recording save folder is not a directory: {}",
            path.display()
        ));
    }

    let probe_path = path.join(format!(
        ".meetily-write-probe-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let probe_result = (|| -> std::io::Result<()> {
        let mut probe = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe_path)?;
        probe.write_all(b"meetily recording directory probe")?;
        probe.sync_all()?;
        Ok(())
    })();

    if let Err(error) = probe_result {
        let _ = std::fs::remove_file(&probe_path);
        return Err(anyhow!(
            "recording save folder is not writable {}: {error}",
            path.display()
        ));
    }
    std::fs::remove_file(&probe_path).map_err(|error| {
        anyhow!(
            "failed to clean recording save-folder write probe {}: {error}",
            probe_path.display()
        )
    })?;

    Ok(path.to_path_buf())
}

/// Ensure the recordings directory exists and is writable.
pub fn ensure_recordings_directory(path: &PathBuf) -> Result<()> {
    validate_recordings_directory(path).map(|_| ())
}

/// Generate a unique filename for a recording
pub fn generate_recording_filename(format: &str) -> String {
    let now = chrono::Utc::now();
    let timestamp = now.format("%Y%m%d_%H%M%S");
    format!("recording_{}.{}", timestamp, format)
}

/// Load recording preferences from store
fn decode_recording_preferences(
    value: Option<serde_json::Value>,
    persisted_store_exists: bool,
) -> Result<RecordingPreferences> {
    match value {
        Some(value) => serde_json::from_value::<RecordingPreferences>(value)
            .map_err(|error| anyhow!("failed to deserialize saved recording preferences: {error}")),
        None if persisted_store_exists => Err(anyhow!(
            "recording preferences store exists but has no readable preferences; refusing default-folder fallback"
        )),
        None => {
            info!("No stored recording preferences found, using defaults");
            Ok(RecordingPreferences::default())
        }
    }
}

async fn load_recording_preferences_from_store<R: Runtime>(
    app: &AppHandle<R>,
    store_path: &Path,
) -> Result<RecordingPreferences> {
    let resolved_store_path = resolve_store_path(app, store_path)
        .map_err(|error| anyhow!("failed to resolve recording preferences store: {error}"))?;
    let persisted_store_exists = resolved_store_path.is_file();
    let store = app
        .store(store_path)
        .map_err(|error| anyhow!("failed to access recording preferences store: {error}"))?;
    #[allow(unused_mut)]
    let mut prefs = decode_recording_preferences(store.get("preferences"), persisted_store_exists)?;
    info!("Loaded recording preferences from store");

    // Update macOS backend to current value if needed.
    #[cfg(target_os = "macos")]
    {
        let backend = crate::audio::capture::get_current_backend();
        prefs.system_audio_backend = Some(backend.to_string());
    }

    info!("Loaded recording preferences: save_folder={:?}, auto_save={}, format={}, mic={:?}, system={:?}",
          prefs.save_folder, prefs.auto_save, prefs.file_format,
          prefs.preferred_mic_device, prefs.preferred_system_device);
    Ok(prefs)
}

pub async fn load_recording_preferences<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<RecordingPreferences> {
    load_recording_preferences_from_store(app, Path::new(RECORDING_PREFERENCES_STORE)).await
}

/// Resolve and freeze all settings used by one recording/import session.
/// The default folder is used only when no preference store has ever existed.
pub async fn resolve_recording_session_preferences<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<RecordingPreferences> {
    let mut preferences = load_recording_preferences(app).await?;
    preferences.save_folder = validate_recordings_directory(&preferences.save_folder)?;
    Ok(preferences)
}

/// Save recording preferences to store
async fn save_recording_preferences_to_store<R: Runtime>(
    app: &AppHandle<R>,
    preferences: &RecordingPreferences,
    store_path: &Path,
) -> Result<()> {
    info!("Saving recording preferences: save_folder={:?}, auto_save={}, format={}, mic={:?}, system={:?}",
          preferences.save_folder, preferences.auto_save, preferences.file_format,
          preferences.preferred_mic_device, preferences.preferred_system_device);

    // Reject a bad location before changing the durable preference store.
    validate_recordings_directory(&preferences.save_folder)?;

    // Get or create store
    let store = app
        .store(store_path)
        .map_err(|e| anyhow::anyhow!("Failed to access store: {}", e))?;

    // Serialize preferences to JSON value
    let prefs_value = serde_json::to_value(preferences)
        .map_err(|e| anyhow::anyhow!("Failed to serialize preferences: {}", e))?;

    // Save to store. If persistence fails, restore the in-memory value so the
    // current process cannot start a recording with a setting the UI reported
    // as unsaved.
    let previous_preferences = store.get("preferences");
    store.set("preferences", prefs_value);

    // Persist to disk
    if let Err(error) = store.save() {
        if let Some(previous) = previous_preferences {
            store.set("preferences", previous);
        } else {
            store.delete("preferences");
        }
        return Err(anyhow::anyhow!(
            "Failed to save recording preferences to disk: {}",
            error
        ));
    }

    info!("Successfully persisted recording preferences to disk");

    // Save backend preference to global config
    #[cfg(target_os = "macos")]
    if let Some(backend_str) = &preferences.system_audio_backend {
        if let Some(backend) = AudioCaptureBackend::from_string(backend_str) {
            info!("Setting audio capture backend to: {:?}", backend);
            crate::audio::capture::set_current_backend(backend);
        }
    }

    Ok(())
}

pub async fn save_recording_preferences<R: Runtime>(
    app: &AppHandle<R>,
    preferences: &RecordingPreferences,
) -> Result<()> {
    save_recording_preferences_to_store(app, preferences, Path::new(RECORDING_PREFERENCES_STORE))
        .await
}

/// Tauri commands for recording preferences
#[tauri::command]
pub async fn get_recording_preferences<R: Runtime>(
    app: AppHandle<R>,
) -> Result<RecordingPreferences, String> {
    load_recording_preferences(&app)
        .await
        .map_err(|e| format!("Failed to load recording preferences: {}", e))
}

#[tauri::command]
pub async fn set_recording_preferences<R: Runtime>(
    app: AppHandle<R>,
    preferences: RecordingPreferences,
) -> Result<(), String> {
    save_recording_preferences(&app, &preferences)
        .await
        .map_err(|e| format!("Failed to save recording preferences: {}", e))
}

#[tauri::command]
pub async fn get_default_recordings_folder_path() -> Result<String, String> {
    let path = get_default_recordings_folder();
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn open_recordings_folder<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let preferences = load_recording_preferences(&app)
        .await
        .map_err(|e| format!("Failed to load preferences: {}", e))?;

    // Ensure directory exists and is writable before trying to open it.
    validate_recordings_directory(&preferences.save_folder)
        .map_err(|e| format!("Failed to validate recording directory: {}", e))?;

    let folder_path = preferences.save_folder.to_string_lossy().to_string();

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

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(&folder_path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }

    info!("Opened recordings folder: {}", folder_path);
    Ok(())
}

#[tauri::command]
pub async fn select_recording_folder<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;

    let selected = tokio::task::spawn_blocking(move || app.dialog().file().blocking_pick_folder())
        .await
        .map_err(|error| format!("Folder dialog task failed: {error}"))?;
    Ok(selected.map(|path| path.to_string()))
}

// Backend selection commands

/// Get available audio capture backends for the current platform
#[tauri::command]
pub async fn get_available_audio_backends() -> Result<Vec<String>, String> {
    #[cfg(target_os = "macos")]
    {
        let backends = crate::audio::capture::get_available_backends();
        Ok(backends.iter().map(|b| b.to_string()).collect())
    }

    #[cfg(not(target_os = "macos"))]
    {
        // Only ScreenCaptureKit available on non-macOS
        Ok(vec!["screencapturekit".to_string()])
    }
}

/// Get current audio capture backend
#[tauri::command]
pub async fn get_current_audio_backend() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let backend = crate::audio::capture::get_current_backend();
        Ok(backend.to_string())
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok("screencapturekit".to_string())
    }
}

/// Set audio capture backend
#[tauri::command]
pub async fn set_audio_backend(backend: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use crate::audio::capture::AudioCaptureBackend;
        use crate::audio::permissions::{
            check_screen_recording_permission, request_screen_recording_permission,
        };

        let backend_enum = AudioCaptureBackend::from_string(&backend)
            .ok_or_else(|| format!("Invalid backend: {}", backend))?;

        // If switching to Core Audio, log information about Audio Capture permission
        if backend_enum == AudioCaptureBackend::CoreAudio {
            info!("🔐 Core Audio backend requires Audio Capture permission (macOS 14.4+)");
            info!("📍 Permission dialog will appear automatically when recording starts");

            // Check if permission is already granted (this is informational only)
            if !check_screen_recording_permission() {
                warn!("⚠️  Audio Capture permission may not be granted");

                // Attempt to open System Settings (opens System Settings)
                if let Err(e) = request_screen_recording_permission() {
                    error!("Failed to open System Settings: {}", e);
                }

                return Err(
                    "Core Audio requires Audio Capture permission. \
                    The permission dialog will appear when you start recording. \
                    If already denied, enable it in System Settings → Privacy & Security → Audio Capture, \
                    then restart the app.".to_string()
                );
            }

            info!(
                "✅ Core Audio backend selected - permission check will occur at recording start"
            );
        }

        info!("Setting audio backend to: {:?}", backend_enum);
        crate::audio::capture::set_current_backend(backend_enum);
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        if backend != "screencapturekit" {
            return Err(format!(
                "Backend {} not available on this platform",
                backend
            ));
        }
        Ok(())
    }
}

/// Get backend information (name and description)
#[derive(Serialize)]
pub struct BackendInfo {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[tauri::command]
pub async fn get_audio_backend_info() -> Result<Vec<BackendInfo>, String> {
    #[cfg(target_os = "macos")]
    {
        use crate::audio::capture::AudioCaptureBackend;

        let backends = vec![
            BackendInfo {
                id: AudioCaptureBackend::ScreenCaptureKit.to_string(),
                name: AudioCaptureBackend::ScreenCaptureKit.name().to_string(),
                description: AudioCaptureBackend::ScreenCaptureKit
                    .description()
                    .to_string(),
            },
            BackendInfo {
                id: AudioCaptureBackend::CoreAudio.to_string(),
                name: AudioCaptureBackend::CoreAudio.name().to_string(),
                description: AudioCaptureBackend::CoreAudio.description().to_string(),
            },
        ];
        Ok(backends)
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok(vec![BackendInfo {
            id: "screencapturekit".to_string(),
            name: "ScreenCaptureKit".to_string(),
            description: "Default system audio capture".to_string(),
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn mock_app() -> tauri::App<tauri::test::MockRuntime> {
        tauri::test::mock_builder()
            .plugin(tauri_plugin_store::Builder::default().build())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap()
    }

    #[test]
    fn missing_store_uses_default_but_unreadable_store_does_not() {
        assert!(decode_recording_preferences(None, false).is_ok());
        assert!(decode_recording_preferences(None, true)
            .unwrap_err()
            .to_string()
            .contains("refusing default-folder fallback"));
        assert!(
            decode_recording_preferences(Some(serde_json::json!({"save_folder": 7})), true,)
                .is_err()
        );
    }

    #[test]
    fn relative_saved_folder_is_rejected() {
        let error = validate_recordings_directory(Path::new("relative/recordings")).unwrap_err();
        assert!(error.to_string().contains("must be an absolute path"));
    }

    #[test]
    fn unwritable_saved_folder_fails_without_default_fallback() {
        let directory = tempdir().unwrap();
        let blocked_path = directory.path().join("selected-folder-is-a-file");
        std::fs::write(&blocked_path, b"not a directory").unwrap();
        let unrelated_default = directory.path().join("default-recordings");
        std::fs::create_dir(&unrelated_default).unwrap();

        let error = validate_recordings_directory(&blocked_path).unwrap_err();

        assert!(!error.to_string().is_empty());
        assert_eq!(std::fs::read(&blocked_path).unwrap(), b"not a directory");
        assert_eq!(std::fs::read_dir(&unrelated_default).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn saved_folder_survives_restart() {
        let directory = tempdir().unwrap();
        let selected_root = directory.path().join("selected-recordings");
        let store_path = directory.path().join("recording-preferences-restart.json");
        let expected = RecordingPreferences {
            save_folder: selected_root.clone(),
            auto_save: false,
            file_format: "mp4".to_string(),
            preferred_mic_device: Some("Test mic".to_string()),
            preferred_system_device: None,
            recording_mode: RecordingMode::MicrophoneOnly,
            #[cfg(target_os = "macos")]
            system_audio_backend: Some("coreaudio".to_string()),
        };

        {
            let app = mock_app();
            save_recording_preferences_to_store(app.handle(), &expected, &store_path)
                .await
                .unwrap();
        }

        let restarted_app = mock_app();
        let loaded = load_recording_preferences_from_store(restarted_app.handle(), &store_path)
            .await
            .unwrap();
        assert_eq!(loaded.save_folder, selected_root);
        assert!(!loaded.auto_save);
        assert_eq!(loaded.preferred_mic_device.as_deref(), Some("Test mic"));
    }
}
