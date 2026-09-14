use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Runtime, State};

use super::coordinator::{MigrationCoordinator, MigrationCoordinatorError};
use super::migration::{
    MigrationEngine, MigrationFileStatus, MigrationPhase, MigrationState, MigrationStateDocument,
};
use super::operation_lock::{storage_operation_snapshot, StorageOperationKind};
use super::validation::{
    first_link_or_reparse_point, validate_target_candidate, StorageValidationReport,
    MIGRATION_MINIMUM_FREE_BYTES,
};
use super::{StorageLayoutState, StorageStatus};

pub const PLANNED_STORAGE_TARGET_ROOT: &str = r"E:\MeetilyData";
pub const STORAGE_MIGRATION_STATUS_CHANGED_EVENT: &str = "storage-migration-status-changed";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageMigrationCommandError {
    pub code: String,
    pub message_key: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, Value>,
    pub retryable: bool,
}

impl StorageMigrationCommandError {
    fn new(code: impl Into<String>, retryable: bool) -> Self {
        let code = code.into();
        Self {
            message_key: format!("storageMigration.errors.{code}"),
            code,
            params: BTreeMap::new(),
            retryable,
        }
    }

    fn with_param(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.params.insert(key.to_owned(), value.into());
        self
    }

    fn operation_busy(status: &StorageOperationLockStatus) -> Self {
        let operations = status
            .active_operations
            .iter()
            .map(|entry| entry.operation.clone())
            .collect::<Vec<_>>();
        Self::new("operation_busy", true).with_param("operations", operations)
    }

    fn coordinator(error: MigrationCoordinatorError) -> Self {
        let retryable = matches!(
            error,
            MigrationCoordinatorError::TaskAlreadyRunning(_)
                | MigrationCoordinatorError::TaskNotRunning
                | MigrationCoordinatorError::OperationBusy(_)
        );
        let code = error.code();
        let mut result = Self::new(code, retryable);
        if let MigrationCoordinatorError::OperationBusy(operation_error) = error {
            result = result.with_param(
                "operations",
                operation_error
                    .blockers
                    .iter()
                    .map(|operation| operation.as_str())
                    .collect::<Vec<_>>(),
            );
        }
        result
    }

    fn inventory(error: impl std::fmt::Display) -> Self {
        log::error!("Storage migration inventory failed: {error}");
        Self::new("inventory_failed", true)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageModelInventory {
    pub root: PathBuf,
    pub exists: bool,
    pub file_count: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageOperationLockEntry {
    pub operation: String,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageOperationBlockReason {
    pub code: String,
    pub operation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageOperationLockStatus {
    pub busy: bool,
    pub migration_active: bool,
    pub active_operations: Vec<StorageOperationLockEntry>,
    pub reasons: Vec<StorageOperationBlockReason>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageMigrationActions {
    pub can_recheck: bool,
    pub can_start: bool,
    pub can_cancel: bool,
    pub can_resume: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageMigrationIssue {
    pub code: String,
    pub relative_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageMigrationStatus {
    pub current_storage_root: PathBuf,
    pub source_root: PathBuf,
    pub target_root: PathBuf,
    pub source_inventory: StorageModelInventory,
    pub target_validation: StorageValidationReport,
    pub operation_lock: StorageOperationLockStatus,
    pub formal_migration_authorized: bool,
    pub active_task_id: Option<String>,
    pub migration_id: Option<String>,
    pub phase: MigrationPhase,
    pub completed_files: u64,
    pub total_files: u64,
    pub completed_bytes: u64,
    pub total_bytes: u64,
    pub progress_percent: f64,
    pub current_relative_path: Option<PathBuf>,
    pub last_error: Option<StorageMigrationIssue>,
    pub restart_required: bool,
    pub old_source_retained: bool,
    pub state_read_error: Option<String>,
    pub actions: StorageMigrationActions,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageMigrationPreflight {
    pub source_inventory: StorageModelInventory,
    pub target_validation: StorageValidationReport,
    pub operation_lock: StorageOperationLockStatus,
    pub formal_migration_authorized: bool,
    pub ready: bool,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageMigrationStatusChangedHint {
    reason: &'static str,
    task_id: Option<String>,
}

#[tauri::command]
pub fn get_storage_status(state: State<'_, StorageLayoutState>) -> StorageStatus {
    state.layout().status()
}

#[tauri::command]
pub fn validate_storage_target(
    target_root: PathBuf,
    state: State<'_, StorageLayoutState>,
) -> Result<StorageValidationReport, String> {
    Ok(validate_target_candidate(
        &target_root,
        state.layout().legacy_app_data_root(),
        MIGRATION_MINIMUM_FREE_BYTES,
        true,
    ))
}

#[tauri::command]
pub async fn get_storage_operation_lock_status() -> StorageOperationLockStatus {
    collect_operation_lock_status().await
}

#[tauri::command]
pub async fn get_storage_migration_status(
    layout_state: State<'_, StorageLayoutState>,
    coordinator: State<'_, MigrationCoordinator>,
) -> Result<StorageMigrationStatus, StorageMigrationCommandError> {
    build_storage_migration_status(&layout_state, &coordinator).await
}

#[tauri::command]
pub async fn preflight_storage_migration(
    layout_state: State<'_, StorageLayoutState>,
    coordinator: State<'_, MigrationCoordinator>,
) -> Result<StorageMigrationPreflight, StorageMigrationCommandError> {
    let inventory = inventory_models(&layout_state.layout().legacy_app_data_root().join("models"))
        .map_err(StorageMigrationCommandError::inventory)?;
    let required_free_bytes = MIGRATION_MINIMUM_FREE_BYTES.saturating_add(inventory.total_bytes);
    let target_validation = validate_target_candidate(
        Path::new(PLANNED_STORAGE_TARGET_ROOT),
        layout_state.layout().legacy_app_data_root(),
        required_free_bytes,
        false,
    );
    let operation_lock = collect_operation_lock_status().await;
    let formal_migration_authorized = coordinator.formal_authorized();
    let mut blockers = Vec::new();
    if !target_validation.valid {
        blockers.push("target_validation_failed".to_owned());
    }
    if operation_lock.busy {
        blockers.push("operation_busy".to_owned());
    }
    if !formal_migration_authorized {
        blockers.push("formal_migration_not_authorized".to_owned());
    }

    Ok(StorageMigrationPreflight {
        source_inventory: inventory,
        target_validation,
        operation_lock,
        formal_migration_authorized,
        ready: blockers.is_empty(),
        blockers,
    })
}

#[tauri::command]
pub async fn start_storage_migration(
    coordinator: State<'_, MigrationCoordinator>,
) -> Result<(), StorageMigrationCommandError> {
    let operation_lock = collect_operation_lock_status().await;
    if operation_lock.busy {
        return Err(StorageMigrationCommandError::operation_busy(
            &operation_lock,
        ));
    }
    coordinator
        .ensure_formal_authorized()
        .map_err(StorageMigrationCommandError::coordinator)?;

    // The S3 production coordinator can never pass the authorization gate.
    // S4 will provide the fixed-root background launch after its separate audit.
    Err(StorageMigrationCommandError::new(
        "formal_migration_not_authorized",
        false,
    ))
}

#[tauri::command]
pub async fn cancel_storage_migration<R: Runtime>(
    app: AppHandle<R>,
    coordinator: State<'_, MigrationCoordinator>,
) -> Result<(), StorageMigrationCommandError> {
    let task_id = coordinator
        .request_cancel()
        .map_err(StorageMigrationCommandError::coordinator)?;
    emit_status_changed(&app, "cancel_requested", Some(task_id));
    Ok(())
}

#[tauri::command]
pub async fn resume_storage_migration(
    coordinator: State<'_, MigrationCoordinator>,
) -> Result<(), StorageMigrationCommandError> {
    let operation_lock = collect_operation_lock_status().await;
    if operation_lock.busy {
        return Err(StorageMigrationCommandError::operation_busy(
            &operation_lock,
        ));
    }
    coordinator
        .ensure_formal_authorized()
        .map_err(StorageMigrationCommandError::coordinator)?;

    Err(StorageMigrationCommandError::new(
        "formal_migration_not_authorized",
        false,
    ))
}

async fn build_storage_migration_status(
    layout_state: &StorageLayoutState,
    coordinator: &MigrationCoordinator,
) -> Result<StorageMigrationStatus, StorageMigrationCommandError> {
    let source_root = layout_state.layout().legacy_app_data_root().to_path_buf();
    let target_root = PathBuf::from(PLANNED_STORAGE_TARGET_ROOT);
    let inventory = inventory_models(&source_root.join("models"))
        .map_err(StorageMigrationCommandError::inventory)?;
    let required_free_bytes = MIGRATION_MINIMUM_FREE_BYTES.saturating_add(inventory.total_bytes);
    let target_validation =
        validate_target_candidate(&target_root, &source_root, required_free_bytes, false);
    let operation_lock = collect_operation_lock_status().await;
    let (state, state_read_error) = match read_latest_planned_state(&source_root, &target_root) {
        Ok(state) => (state, None),
        Err(error) => {
            log::error!("Unable to read persisted storage migration state: {error}");
            (None, Some("migration_state_unreadable".to_owned()))
        }
    };
    let formal_migration_authorized = coordinator.formal_authorized();
    let active_task_id = coordinator.active_task_id();
    let phase = state
        .as_ref()
        .map(|state| state.phase)
        .unwrap_or(MigrationPhase::NotStarted);
    let total_files = state
        .as_ref()
        .map(|state| state.total_files)
        .unwrap_or(inventory.file_count);
    let total_bytes = state
        .as_ref()
        .map(|state| state.total_bytes)
        .unwrap_or(inventory.total_bytes);
    let completed_files = state
        .as_ref()
        .map(|state| state.completed_files)
        .unwrap_or_default();
    let completed_bytes = state
        .as_ref()
        .map(|state| state.completed_bytes)
        .unwrap_or_default();
    let progress_percent = if total_bytes == 0 {
        0.0
    } else {
        ((completed_bytes as f64 / total_bytes as f64) * 100.0).clamp(0.0, 100.0)
    };
    let current_relative_path = state.as_ref().and_then(current_relative_path);
    let last_error = state.as_ref().and_then(|state| {
        state
            .last_error
            .as_ref()
            .map(|failure| StorageMigrationIssue {
                code: failure.code.clone(),
                relative_path: failure.relative_path.clone(),
            })
    });
    let state_is_resumable = matches!(phase, MigrationPhase::Failed)
        && last_error.as_ref().map(|error| error.code.as_str()) == Some("cancelled_by_user");
    let no_runtime_operation = !operation_lock.busy;

    Ok(StorageMigrationStatus {
        current_storage_root: layout_state.layout().active_storage_root().to_path_buf(),
        source_root,
        target_root,
        source_inventory: inventory,
        target_validation: target_validation.clone(),
        operation_lock,
        formal_migration_authorized,
        active_task_id: active_task_id.clone(),
        migration_id: state.as_ref().map(|state| state.migration_id.clone()),
        phase,
        completed_files,
        total_files,
        completed_bytes,
        total_bytes,
        progress_percent,
        current_relative_path,
        last_error,
        restart_required: matches!(phase, MigrationPhase::SwitchPendingRestart),
        old_source_retained: state
            .as_ref()
            .map(|state| state.source_files_still_present)
            .unwrap_or(true),
        state_read_error,
        actions: StorageMigrationActions {
            can_recheck: true,
            can_start: formal_migration_authorized
                && no_runtime_operation
                && state.is_none()
                && target_validation.valid,
            can_cancel: active_task_id.is_some(),
            can_resume: formal_migration_authorized
                && no_runtime_operation
                && state_is_resumable
                && target_validation.valid,
        },
    })
}

async fn collect_operation_lock_status() -> StorageOperationLockStatus {
    let snapshot = storage_operation_snapshot();
    let mut operations = BTreeMap::<String, u32>::new();
    for entry in snapshot.active_operations {
        operations.insert(entry.operation.as_str().to_owned(), entry.count);
    }
    if crate::audio::recording_commands::is_recording().await {
        operations
            .entry(StorageOperationKind::Recording.as_str().to_owned())
            .and_modify(|count| *count = (*count).max(1))
            .or_insert(1);
        operations
            .entry(StorageOperationKind::LiveTranscription.as_str().to_owned())
            .and_modify(|count| *count = (*count).max(1))
            .or_insert(1);
    }
    if crate::audio::retranscription::is_retranscription_in_progress() {
        operations
            .entry(StorageOperationKind::Retranscription.as_str().to_owned())
            .and_modify(|count| *count = (*count).max(1))
            .or_insert(1);
    }

    let active_operations = operations
        .into_iter()
        .map(|(operation, count)| StorageOperationLockEntry { operation, count })
        .collect::<Vec<_>>();
    let reasons = active_operations
        .iter()
        .map(|entry| StorageOperationBlockReason {
            code: "storage_operation_active".to_owned(),
            operation: entry.operation.clone(),
        })
        .collect::<Vec<_>>();
    let migration_active = active_operations
        .iter()
        .any(|entry| entry.operation == StorageOperationKind::Migration.as_str());

    StorageOperationLockStatus {
        busy: !active_operations.is_empty(),
        migration_active,
        active_operations,
        reasons,
    }
}

fn current_relative_path(state: &MigrationState) -> Option<PathBuf> {
    state
        .files
        .iter()
        .find(|file| {
            matches!(
                file.status,
                MigrationFileStatus::Partial
                    | MigrationFileStatus::Failed
                    | MigrationFileStatus::Pending
            )
        })
        .map(|file| file.relative_path.clone())
}

fn read_latest_planned_state(
    source_root: &Path,
    target_root: &Path,
) -> Result<Option<MigrationState>, String> {
    let migration_root = source_root.join("migration");
    if !migration_root.exists() {
        return Ok(None);
    }
    if let Some(path) =
        first_link_or_reparse_point(&migration_root).map_err(|error| error.to_string())?
    {
        return Err(format!(
            "migration control path is a reparse point: {}",
            path.display()
        ));
    }

    let mut candidates = Vec::new();
    let entries = fs::read_dir(&migration_root)
        .map_err(|error| format!("read {}: {error}", migration_root.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("read migration entry: {error}"))?;
        let metadata = entry
            .metadata()
            .map_err(|error| format!("inspect {}: {error}", entry.path().display()))?;
        if !metadata.is_dir() {
            continue;
        }
        let state_path = entry.path().join("state.json");
        if !state_path.is_file() {
            continue;
        }
        let bytes = fs::read(&state_path)
            .map_err(|error| format!("read {}: {error}", state_path.display()))?;
        let document: MigrationStateDocument = serde_json::from_slice(&bytes)
            .map_err(|error| format!("parse {}: {error}", state_path.display()))?;
        if document.state.source_root != source_root || document.state.target_root != target_root {
            continue;
        }
        let engine = MigrationEngine::new(
            source_root.to_path_buf(),
            target_root.to_path_buf(),
            document.state.migration_id.clone(),
        )
        .map_err(|error| error.to_string())?;
        let state = engine.load_state().map_err(|error| error.to_string())?;
        candidates.push(state);
    }
    candidates.sort_by(|left, right| right.updated_at_utc.cmp(&left.updated_at_utc));
    Ok(candidates.into_iter().next())
}

fn inventory_models(root: &Path) -> Result<StorageModelInventory, String> {
    if !root.exists() {
        return Ok(StorageModelInventory {
            root: root.to_path_buf(),
            exists: false,
            file_count: 0,
            total_bytes: 0,
        });
    }
    if !root.is_dir() {
        return Err(format!("model root is not a directory: {}", root.display()));
    }
    if let Some(path) = first_link_or_reparse_point(root).map_err(|error| error.to_string())? {
        return Err(format!(
            "model inventory contains a reparse point: {}",
            path.display()
        ));
    }

    let mut pending = vec![root.to_path_buf()];
    let mut visited = BTreeSet::new();
    let mut file_count = 0_u64;
    let mut total_bytes = 0_u64;
    while let Some(directory) = pending.pop() {
        if !visited.insert(directory.clone()) {
            return Err(format!(
                "model inventory directory repeated: {}",
                directory.display()
            ));
        }
        for entry in fs::read_dir(&directory)
            .map_err(|error| format!("read {}: {error}", directory.display()))?
        {
            let entry = entry.map_err(|error| format!("read model entry: {error}"))?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("inspect {}: {error}", path.display()))?;
            if is_reparse_or_symlink(&metadata) {
                return Err(format!(
                    "model inventory contains a reparse point: {}",
                    path.display()
                ));
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                file_count = file_count.saturating_add(1);
                total_bytes = total_bytes.saturating_add(metadata.len());
            } else {
                return Err(format!(
                    "model inventory contains an unsupported entry: {}",
                    path.display()
                ));
            }
        }
    }

    Ok(StorageModelInventory {
        root: root.to_path_buf(),
        exists: true,
        file_count,
        total_bytes,
    })
}

fn is_reparse_or_symlink(metadata: &fs::Metadata) -> bool {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(target_os = "windows"))]
    {
        metadata.file_type().is_symlink()
    }
}

fn emit_status_changed<R: Runtime>(
    app: &AppHandle<R>,
    reason: &'static str,
    task_id: Option<String>,
) {
    if let Err(error) = app.emit(
        STORAGE_MIGRATION_STATUS_CHANGED_EVENT,
        StorageMigrationStatusChangedHint { reason, task_id },
    ) {
        log::warn!("Unable to emit storage migration status hint: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_counts_regular_files_and_bytes() {
        let temporary = tempfile::tempdir().unwrap();
        let models = temporary.path().join("models");
        fs::create_dir_all(models.join("nested")).unwrap();
        fs::write(models.join("one.bin"), b"123").unwrap();
        fs::write(models.join("nested").join("two.bin"), b"4567").unwrap();

        let inventory = inventory_models(&models).unwrap();
        assert!(inventory.exists);
        assert_eq!(inventory.file_count, 2);
        assert_eq!(inventory.total_bytes, 7);
    }

    #[test]
    fn missing_inventory_root_is_an_explicit_zero_inventory() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("missing-models");
        let inventory = inventory_models(&root).unwrap();
        assert!(!inventory.exists);
        assert_eq!(inventory.file_count, 0);
        assert_eq!(inventory.total_bytes, 0);
    }

    #[test]
    fn production_target_is_fixed_and_not_a_frontend_argument() {
        assert_eq!(PLANNED_STORAGE_TARGET_ROOT, r"E:\MeetilyData");
        let source = include_str!("commands.rs");
        assert!(!source.contains("start_storage_migration(\n    target_root"));
        assert!(!source.contains("resume_storage_migration(\n    target_root"));
    }
}
