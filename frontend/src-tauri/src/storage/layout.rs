use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::preferences::StoragePreferences;
use super::{preferences_path, StorageError, StorageResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageLayoutSource {
    Legacy,
    Migrated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoragePaths {
    pub models_root: PathBuf,
    pub whisper_models: PathBuf,
    pub parakeet_models: PathBuf,
    pub summary_models: PathBuf,
    pub moss_models: PathBuf,
    pub moss_runtime: PathBuf,
    pub download_cache: PathBuf,
    pub recordings_root: PathBuf,
    pub migration_root: PathBuf,
    pub backup_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageLayout {
    legacy_app_data_root: PathBuf,
    active_storage_root: PathBuf,
    configured_storage_root: Option<PathBuf>,
    previous_storage_root: Option<PathBuf>,
    migration_id: Option<String>,
    migration_completed: bool,
    preferences_present: bool,
    source: StorageLayoutSource,
    paths: StoragePaths,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStatus {
    pub schema_version: u32,
    pub source: StorageLayoutSource,
    pub migration_completed: bool,
    pub preferences_present: bool,
    pub preferences_path: PathBuf,
    pub legacy_app_data_root: PathBuf,
    pub active_storage_root: PathBuf,
    pub configured_storage_root: Option<PathBuf>,
    pub previous_storage_root: Option<PathBuf>,
    pub migration_id: Option<String>,
    pub paths: StoragePaths,
}

impl StorageLayout {
    pub fn legacy(app_data_root: PathBuf) -> StorageResult<Self> {
        Self::build(
            app_data_root.clone(),
            app_data_root,
            None,
            None,
            None,
            false,
            false,
            StorageLayoutSource::Legacy,
        )
    }

    pub fn from_preferences(
        legacy_app_data_root: PathBuf,
        preferences: Option<StoragePreferences>,
    ) -> StorageResult<Self> {
        ensure_absolute_non_root(&legacy_app_data_root)?;

        let Some(preferences) = preferences else {
            return Self::legacy(legacy_app_data_root);
        };

        preferences.validate()?;
        ensure_no_legacy_overlap(&preferences.storage_root, &legacy_app_data_root)?;
        let configured_storage_root = Some(preferences.storage_root.clone());
        let active_storage_root = if preferences.migration_completed {
            preferences.storage_root.clone()
        } else {
            legacy_app_data_root.clone()
        };
        let source = if preferences.migration_completed {
            StorageLayoutSource::Migrated
        } else {
            StorageLayoutSource::Legacy
        };

        Self::build(
            legacy_app_data_root,
            active_storage_root,
            configured_storage_root,
            preferences.previous_storage_root,
            preferences.migration_id,
            preferences.migration_completed,
            true,
            source,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        legacy_app_data_root: PathBuf,
        active_storage_root: PathBuf,
        configured_storage_root: Option<PathBuf>,
        previous_storage_root: Option<PathBuf>,
        migration_id: Option<String>,
        migration_completed: bool,
        preferences_present: bool,
        source: StorageLayoutSource,
    ) -> StorageResult<Self> {
        ensure_absolute_non_root(&legacy_app_data_root)?;
        ensure_absolute_non_root(&active_storage_root)?;

        let models_root = active_storage_root.join("models");
        let paths = StoragePaths {
            whisper_models: models_root.clone(),
            parakeet_models: models_root.join("parakeet"),
            summary_models: models_root.join("summary"),
            moss_models: models_root.join("moss"),
            models_root,
            moss_runtime: active_storage_root.join("runtime").join("moss"),
            download_cache: active_storage_root.join("cache").join("downloads"),
            recordings_root: active_storage_root.join("recordings"),
            migration_root: active_storage_root.join("migration"),
            backup_root: active_storage_root.join("backup"),
        };

        Ok(Self {
            legacy_app_data_root,
            active_storage_root,
            configured_storage_root,
            previous_storage_root,
            migration_id,
            migration_completed,
            preferences_present,
            source,
            paths,
        })
    }

    pub fn active_storage_root(&self) -> &Path {
        &self.active_storage_root
    }

    pub fn legacy_app_data_root(&self) -> &Path {
        &self.legacy_app_data_root
    }

    pub fn configured_storage_root(&self) -> Option<&Path> {
        self.configured_storage_root.as_deref()
    }

    pub fn migration_completed(&self) -> bool {
        self.migration_completed
    }

    pub fn source(&self) -> StorageLayoutSource {
        self.source
    }

    pub fn paths(&self) -> &StoragePaths {
        &self.paths
    }

    pub fn models_root(&self) -> &Path {
        &self.paths.models_root
    }

    pub fn whisper_models_dir(&self) -> &Path {
        &self.paths.whisper_models
    }

    pub fn parakeet_models_dir(&self) -> &Path {
        &self.paths.parakeet_models
    }

    pub fn summary_models_dir(&self) -> &Path {
        &self.paths.summary_models
    }

    pub fn status(&self) -> StorageStatus {
        StorageStatus {
            schema_version: super::preferences::STORAGE_PREFERENCES_SCHEMA_VERSION,
            source: self.source,
            migration_completed: self.migration_completed,
            preferences_present: self.preferences_present,
            preferences_path: preferences_path(&self.legacy_app_data_root),
            legacy_app_data_root: self.legacy_app_data_root.clone(),
            active_storage_root: self.active_storage_root.clone(),
            configured_storage_root: self.configured_storage_root.clone(),
            previous_storage_root: self.previous_storage_root.clone(),
            migration_id: self.migration_id.clone(),
            paths: self.paths.clone(),
        }
    }
}

pub(crate) fn ensure_absolute_non_root(path: &Path) -> StorageResult<()> {
    if !path.is_absolute() {
        return Err(StorageError::PathNotAbsolute(path.to_path_buf()));
    }

    if path.parent().is_none() {
        return Err(StorageError::UnsafeFilesystemRoot(path.to_path_buf()));
    }

    if path.components().any(|component| {
        matches!(
            component,
            std::path::Component::CurDir | std::path::Component::ParentDir
        )
    }) {
        return Err(StorageError::InvalidPreferences(format!(
            "storage path cannot contain '.' or '..' components: {}",
            path.display()
        )));
    }

    Ok(())
}

pub(crate) fn path_is_same_or_descendant(path: &Path, ancestor: &Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        let normalize = |value: &Path| {
            let normalized = value.to_string_lossy().replace('/', "\\").to_lowercase();
            let normalized = if let Some(rest) = normalized.strip_prefix(r"\\?\unc\") {
                format!(r"\\{rest}")
            } else if let Some(rest) = normalized.strip_prefix(r"\\?\") {
                rest.to_owned()
            } else {
                normalized
            };
            normalized.trim_end_matches('\\').to_owned()
        };
        let path = normalize(path);
        let ancestor = normalize(ancestor);
        return path == ancestor || path.starts_with(&format!("{ancestor}\\"));
    }

    #[cfg(not(target_os = "windows"))]
    {
        path == ancestor || path.starts_with(ancestor)
    }
}

pub(crate) fn ensure_no_legacy_overlap(target: &Path, legacy: &Path) -> StorageResult<()> {
    if path_is_same_or_descendant(target, legacy) {
        return Err(StorageError::TargetInsideLegacyData(target.to_path_buf()));
    }
    if path_is_same_or_descendant(legacy, target) {
        return Err(StorageError::TargetContainsLegacyData {
            target: target.to_path_buf(),
            legacy: legacy.to_path_buf(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::preferences::STORAGE_PREFERENCES_SCHEMA_VERSION;

    fn absolute_test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(name)
    }

    #[test]
    fn no_preferences_keeps_every_model_on_legacy_root() {
        let legacy = absolute_test_path("meetily-legacy-layout");
        let layout = StorageLayout::from_preferences(legacy.clone(), None).unwrap();

        assert_eq!(layout.source(), StorageLayoutSource::Legacy);
        assert!(!layout.migration_completed());
        assert_eq!(layout.active_storage_root(), legacy);
        assert_eq!(layout.whisper_models_dir(), legacy.join("models"));
        assert_eq!(
            layout.parakeet_models_dir(),
            legacy.join("models").join("parakeet")
        );
        assert_eq!(
            layout.summary_models_dir(),
            legacy.join("models").join("summary")
        );
    }

    #[test]
    fn incomplete_migration_records_target_but_stays_on_legacy_root() {
        let legacy = absolute_test_path("meetily-legacy-incomplete");
        let target = absolute_test_path("MeetilyData-staged");
        let preferences = StoragePreferences {
            schema_version: STORAGE_PREFERENCES_SCHEMA_VERSION,
            storage_root: target.clone(),
            previous_storage_root: Some(legacy.clone()),
            migration_id: Some("migration-staged".to_string()),
            migration_completed: false,
        };

        let layout = StorageLayout::from_preferences(legacy.clone(), Some(preferences)).unwrap();

        assert_eq!(layout.source(), StorageLayoutSource::Legacy);
        assert_eq!(layout.active_storage_root(), legacy);
        assert_eq!(layout.configured_storage_root(), Some(target.as_path()));
        assert!(!layout.migration_completed());
        assert_eq!(layout.whisper_models_dir(), legacy.join("models"));
    }

    #[test]
    fn completed_migration_routes_all_model_families_to_one_root() {
        let legacy = absolute_test_path("meetily-legacy-completed");
        let target = absolute_test_path(
            "Meetily 数据 空格 very-long-segment-012345678901234567890123456789",
        );
        let preferences = StoragePreferences {
            schema_version: STORAGE_PREFERENCES_SCHEMA_VERSION,
            storage_root: target.clone(),
            previous_storage_root: Some(legacy.clone()),
            migration_id: Some("migration-completed".to_string()),
            migration_completed: true,
        };

        let layout = StorageLayout::from_preferences(legacy, Some(preferences)).unwrap();

        assert_eq!(layout.source(), StorageLayoutSource::Migrated);
        assert!(layout.migration_completed());
        assert_eq!(layout.models_root(), target.join("models"));
        assert_eq!(layout.whisper_models_dir(), target.join("models"));
        assert_eq!(
            layout.parakeet_models_dir(),
            target.join("models").join("parakeet")
        );
        assert_eq!(
            layout.summary_models_dir(),
            target.join("models").join("summary")
        );
        assert_eq!(
            layout.paths().moss_models,
            target.join("models").join("moss")
        );
        assert_eq!(
            layout.paths().moss_runtime,
            target.join("runtime").join("moss")
        );
    }

    #[test]
    fn relative_storage_root_is_rejected() {
        let preferences = StoragePreferences {
            schema_version: STORAGE_PREFERENCES_SCHEMA_VERSION,
            storage_root: PathBuf::from("relative-storage"),
            previous_storage_root: None,
            migration_id: Some("bad-relative".to_string()),
            migration_completed: true,
        };

        let error = StorageLayout::from_preferences(
            absolute_test_path("meetily-legacy-relative"),
            Some(preferences),
        )
        .unwrap_err();

        assert!(matches!(error, StorageError::PathNotAbsolute(_)));
    }

    #[test]
    fn configured_target_inside_legacy_root_is_rejected() {
        let legacy = absolute_test_path("meetily-legacy-contained-target");
        let preferences = StoragePreferences {
            schema_version: STORAGE_PREFERENCES_SCHEMA_VERSION,
            storage_root: legacy.join("moved-models"),
            previous_storage_root: Some(legacy.clone()),
            migration_id: Some("bad-contained-target".to_string()),
            migration_completed: false,
        };

        let error = StorageLayout::from_preferences(legacy, Some(preferences)).unwrap_err();
        assert!(matches!(error, StorageError::TargetInsideLegacyData(_)));
    }

    #[test]
    fn configured_target_containing_legacy_root_is_rejected() {
        let container = absolute_test_path("meetily-container-target");
        let legacy = container.join("com.meetily.ai");
        let preferences = StoragePreferences {
            schema_version: STORAGE_PREFERENCES_SCHEMA_VERSION,
            storage_root: container,
            previous_storage_root: Some(legacy.clone()),
            migration_id: Some("bad-containing-target".to_string()),
            migration_completed: false,
        };

        let error = StorageLayout::from_preferences(legacy, Some(preferences)).unwrap_err();
        assert!(matches!(
            error,
            StorageError::TargetContainsLegacyData { .. }
        ));
    }

    #[tokio::test]
    async fn three_model_managers_receive_paths_from_the_same_layout() {
        let directory = tempfile::tempdir().unwrap();
        let legacy = directory.path().join("legacy");
        let target = directory
            .path()
            .join("统一 模型目录")
            .join("long-0123456789012345678901234567890123456789");
        let preferences = StoragePreferences {
            schema_version: STORAGE_PREFERENCES_SCHEMA_VERSION,
            storage_root: target.clone(),
            previous_storage_root: Some(legacy.clone()),
            migration_id: Some("three-engines".to_string()),
            migration_completed: true,
        };
        let layout = StorageLayout::from_preferences(legacy, Some(preferences)).unwrap();

        let whisper = crate::whisper_engine::WhisperEngine::new_with_models_dir(
            layout.whisper_models_dir().to_path_buf(),
        )
        .unwrap();
        let parakeet = crate::parakeet_engine::ParakeetEngine::new_with_models_root(
            layout.models_root().to_path_buf(),
        )
        .unwrap();
        let summary =
            crate::summary::summary_engine::model_manager::ModelManager::new_with_models_dir(
                layout.summary_models_dir().to_path_buf(),
            )
            .unwrap();

        assert_eq!(
            whisper.get_models_directory().await,
            layout.whisper_models_dir()
        );
        assert_eq!(
            parakeet.get_models_directory().await,
            layout.parakeet_models_dir()
        );
        assert_eq!(summary.get_models_directory(), layout.summary_models_dir());
    }

    #[tokio::test]
    #[ignore = "requires MEETILY_S1_LEGACY_APP_DATA pointing to the real closed-app fixture"]
    async fn s1_real_legacy_models_remain_discoverable_without_preferences() {
        let legacy = PathBuf::from(
            std::env::var("MEETILY_S1_LEGACY_APP_DATA")
                .expect("MEETILY_S1_LEGACY_APP_DATA must be set"),
        );
        assert!(legacy.is_dir(), "legacy app-data fixture is missing");
        assert!(
            !super::preferences_path(&legacy).exists(),
            "real legacy check requires storage preferences to be absent"
        );

        let layout = crate::storage::resolve_layout(legacy.clone()).unwrap();
        assert_eq!(layout.source(), StorageLayoutSource::Legacy);
        assert_eq!(layout.active_storage_root(), legacy);

        let source_files = std::fs::read_dir(layout.models_root())
            .unwrap()
            .flat_map(|entry| {
                let entry = entry.unwrap();
                if entry.path().is_dir() {
                    walk_files(entry.path())
                } else {
                    vec![entry.path()]
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(source_files.len(), 9, "real model fixture changed");

        let whisper = crate::whisper_engine::WhisperEngine::new_with_models_dir(
            layout.whisper_models_dir().to_path_buf(),
        )
        .unwrap();
        let whisper_available = whisper
            .discover_models()
            .await
            .unwrap()
            .into_iter()
            .filter(|model| matches!(model.status, crate::whisper_engine::ModelStatus::Available))
            .count();
        assert_eq!(whisper_available, 3);

        let parakeet = crate::parakeet_engine::ParakeetEngine::new_with_models_root(
            layout.models_root().to_path_buf(),
        )
        .unwrap();
        let parakeet_available = parakeet
            .discover_models()
            .await
            .unwrap()
            .into_iter()
            .filter(|model| matches!(model.status, crate::parakeet_engine::ModelStatus::Available))
            .count();
        assert_eq!(parakeet_available, 1);

        let summary =
            crate::summary::summary_engine::model_manager::ModelManager::new_with_models_dir(
                layout.summary_models_dir().to_path_buf(),
            )
            .unwrap();
        summary.init().await.unwrap();
        let summary_available = summary
            .list_models()
            .await
            .into_iter()
            .filter(|model| {
                matches!(
                    model.status,
                    crate::summary::summary_engine::model_manager::ModelStatus::Available
                )
            })
            .count();
        assert_eq!(summary_available, 2);
    }

    fn walk_files(root: PathBuf) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(walk_files(path));
            } else {
                files.push(path);
            }
        }
        files
    }
}
