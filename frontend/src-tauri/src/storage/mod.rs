pub mod commands;
pub mod coordinator;
pub mod layout;
pub mod migration;
pub mod operation_lock;
pub mod preferences;
pub mod validation;

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager, Runtime};
use thiserror::Error;

pub use layout::{StorageLayout, StorageLayoutSource, StoragePaths, StorageStatus};
pub use preferences::{StoragePreferences, STORAGE_PREFERENCES_FILE_NAME};

pub type StorageResult<T> = Result<T, StorageError>;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("Storage path is not absolute: {0}")]
    PathNotAbsolute(PathBuf),

    #[error("Storage path cannot be a drive or filesystem root: {0}")]
    UnsafeFilesystemRoot(PathBuf),

    #[error("Storage path cannot be inside the legacy application-data directory: {0}")]
    TargetInsideLegacyData(PathBuf),

    #[error("Storage path {target} cannot contain the legacy application-data directory {legacy}")]
    TargetContainsLegacyData { target: PathBuf, legacy: PathBuf },

    #[error("Storage path cannot use a symbolic link, junction, or other reparse point: {0}")]
    UnsafeReparsePoint(PathBuf),

    #[error("Unsupported storage-preferences schema version {actual}; expected {expected}")]
    UnsupportedSchema { actual: u32, expected: u32 },

    #[error("Invalid storage preferences: {0}")]
    InvalidPreferences(String),

    #[error("Storage validation failed: {0}")]
    Validation(String),

    #[error("Failed to parse storage preferences at {path}: {source}")]
    ParsePreferences {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("Failed to serialize storage preferences for {path}: {source}")]
    SerializePreferences {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("Failed to {operation} storage file or directory {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to resolve the Tauri application-data directory: {0}")]
    AppData(String),
}

#[derive(Debug)]
pub struct StorageLayoutState {
    layout: StorageLayout,
}

impl StorageLayoutState {
    pub fn new(layout: StorageLayout) -> Self {
        Self { layout }
    }

    pub fn layout(&self) -> &StorageLayout {
        &self.layout
    }
}

pub fn preferences_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(STORAGE_PREFERENCES_FILE_NAME)
}

pub fn resolve_layout(app_data_dir: PathBuf) -> StorageResult<StorageLayout> {
    let preferences_file = preferences_path(&app_data_dir);
    let preferences = preferences::load(&preferences_file)?;
    let layout = StorageLayout::from_preferences(app_data_dir, preferences)?;

    if layout.migration_completed() {
        validation::validate_active_layout(&layout)?;
    }

    Ok(layout)
}

pub fn resolve_layout_for_app<R: Runtime>(app: &AppHandle<R>) -> StorageResult<StorageLayout> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| StorageError::AppData(error.to_string()))?;
    resolve_layout(app_data_dir)
}
