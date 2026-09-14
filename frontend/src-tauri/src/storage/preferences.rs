use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::layout::ensure_absolute_non_root;
use super::{StorageError, StorageResult};

pub const STORAGE_PREFERENCES_FILE_NAME: &str = "storage-preferences.v1.json";
pub const STORAGE_PREFERENCES_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoragePreferences {
    pub schema_version: u32,
    pub storage_root: PathBuf,
    pub previous_storage_root: Option<PathBuf>,
    pub migration_id: Option<String>,
    #[serde(default)]
    pub migration_completed: bool,
}

impl StoragePreferences {
    pub fn validate(&self) -> StorageResult<()> {
        if self.schema_version != STORAGE_PREFERENCES_SCHEMA_VERSION {
            return Err(StorageError::UnsupportedSchema {
                actual: self.schema_version,
                expected: STORAGE_PREFERENCES_SCHEMA_VERSION,
            });
        }

        ensure_absolute_non_root(&self.storage_root)?;
        if let Some(previous) = &self.previous_storage_root {
            ensure_absolute_non_root(previous)?;
        }

        if self.migration_completed
            && self
                .migration_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
        {
            return Err(StorageError::InvalidPreferences(
                "migration_id is required when migration_completed is true".to_string(),
            ));
        }

        Ok(())
    }
}

pub fn load(path: &Path) -> StorageResult<Option<StoragePreferences>> {
    if !path.exists() {
        return Ok(None);
    }

    let bytes = fs::read(path).map_err(|source| StorageError::Io {
        operation: "read",
        path: path.to_path_buf(),
        source,
    })?;
    let preferences = serde_json::from_slice::<StoragePreferences>(&bytes).map_err(|source| {
        StorageError::ParsePreferences {
            path: path.to_path_buf(),
            source,
        }
    })?;
    preferences.validate()?;
    Ok(Some(preferences))
}

pub fn save_atomic(path: &Path, preferences: &StoragePreferences) -> StorageResult<()> {
    preferences.validate()?;
    let parent = path.parent().ok_or_else(|| {
        StorageError::InvalidPreferences(format!(
            "storage preferences path has no parent: {}",
            path.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|source| StorageError::Io {
        operation: "create parent directory for",
        path: parent.to_path_buf(),
        source,
    })?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(STORAGE_PREFERENCES_FILE_NAME);
    let temporary_path = parent.join(format!(
        ".{file_name}.{}.tmp",
        uuid::Uuid::new_v4().simple()
    ));

    let result = (|| -> StorageResult<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
            .map_err(|source| StorageError::Io {
                operation: "create temporary",
                path: temporary_path.clone(),
                source,
            })?;
        let json = serde_json::to_vec_pretty(preferences).map_err(|source| {
            StorageError::SerializePreferences {
                path: temporary_path.clone(),
                source,
            }
        })?;
        file.write_all(&json).map_err(|source| StorageError::Io {
            operation: "write temporary",
            path: temporary_path.clone(),
            source,
        })?;
        file.write_all(b"\n").map_err(|source| StorageError::Io {
            operation: "finish temporary",
            path: temporary_path.clone(),
            source,
        })?;
        file.sync_all().map_err(|source| StorageError::Io {
            operation: "flush temporary",
            path: temporary_path.clone(),
            source,
        })?;
        drop(file);

        fs::rename(&temporary_path, path).map_err(|source| StorageError::Io {
            operation: "atomically replace",
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    })();

    if result.is_err() && temporary_path.exists() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preferences(root: PathBuf) -> StoragePreferences {
        StoragePreferences {
            schema_version: STORAGE_PREFERENCES_SCHEMA_VERSION,
            storage_root: root,
            previous_storage_root: Some(std::env::temp_dir().join("meetily-legacy")),
            migration_id: Some("storage-migration-test".to_string()),
            migration_completed: true,
        }
    }

    #[test]
    fn absent_file_is_legacy_signal_not_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(STORAGE_PREFERENCES_FILE_NAME);
        assert_eq!(load(&path).unwrap(), None);
    }

    #[test]
    fn atomic_save_round_trips_and_leaves_no_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(STORAGE_PREFERENCES_FILE_NAME);
        let first = preferences(directory.path().join("first-target"));
        let expected = preferences(directory.path().join("目标 目录 with spaces"));

        save_atomic(&path, &first).unwrap();
        save_atomic(&path, &expected).unwrap();
        let actual = load(&path).unwrap().unwrap();

        assert_eq!(actual, expected);
        let leftover = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"));
        assert!(!leftover);
    }

    #[test]
    fn unsupported_schema_is_rejected() {
        let mut value = preferences(std::env::temp_dir().join("schema-test"));
        value.schema_version = 2;
        let error = value.validate().unwrap_err();
        assert!(matches!(error, StorageError::UnsupportedSchema { .. }));
    }

    #[test]
    fn completed_migration_requires_an_id() {
        let mut value = preferences(std::env::temp_dir().join("missing-id"));
        value.migration_id = Some("  ".to_string());
        let error = value.validate().unwrap_err();
        assert!(matches!(error, StorageError::InvalidPreferences(_)));
    }
}
