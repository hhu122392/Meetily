use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{SecondsFormat, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use super::layout::{
    ensure_absolute_non_root, ensure_no_legacy_overlap, path_is_same_or_descendant,
};
use super::preferences::{self, StoragePreferences, STORAGE_PREFERENCES_SCHEMA_VERSION};
use super::validation::{first_link_or_reparse_point, validate_target_candidate};
use super::{preferences_path, StorageError};

pub const MIGRATION_DOCUMENT_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_COPY_CHUNK_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationPhase {
    NotStarted,
    Preflight,
    InventoryCreated,
    Copying,
    Verifying,
    ReadyToSwitch,
    SwitchPendingRestart,
    ValidatingRuntime,
    Completed,
    Failed,
    RolledBack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationFileStatus {
    Pending,
    Partial,
    Verified,
    Quarantined,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationManifestEntry {
    pub relative_path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationManifest {
    pub schema_version: u32,
    pub migration_id: String,
    pub source_root: PathBuf,
    pub target_root: PathBuf,
    pub created_at_utc: String,
    pub file_count: u64,
    pub total_bytes: u64,
    pub files: Vec<MigrationManifestEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationManifestDocument {
    pub schema_version: u32,
    pub manifest_sha256: String,
    pub manifest: MigrationManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationFileProgress {
    pub relative_path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
    pub copied_bytes: u64,
    pub status: MigrationFileStatus,
    pub quarantine_path_relative: Option<PathBuf>,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationRunStats {
    pub bytes_written: u64,
    pub skipped_files: u64,
    pub resumed_from_bytes: u64,
    pub quarantined_files: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationFailure {
    pub code: String,
    pub relative_path: Option<PathBuf>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationState {
    pub schema_version: u32,
    pub migration_id: String,
    pub manifest_sha256: String,
    pub source_root: PathBuf,
    pub target_root: PathBuf,
    pub started_at_utc: String,
    pub updated_at_utc: String,
    pub phase: MigrationPhase,
    pub total_files: u64,
    pub completed_files: u64,
    pub total_bytes: u64,
    pub completed_bytes: u64,
    pub minimum_free_bytes: u64,
    pub files: Vec<MigrationFileProgress>,
    pub last_error: Option<MigrationFailure>,
    pub original_preferences: Option<StoragePreferences>,
    pub proposed_preferences: StoragePreferences,
    pub switched: bool,
    pub runtime_validated: bool,
    pub source_files_still_present: bool,
    pub rollback_result: Option<String>,
    pub last_run_stats: MigrationRunStats,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationStateDocument {
    pub schema_version: u32,
    pub state_sha256: String,
    pub state: MigrationState,
}

#[derive(Debug, Error)]
pub enum MigrationError {
    #[error("Invalid migration request: {0}")]
    InvalidRequest(String),

    #[error("Storage migration preflight failed: {0}")]
    Preflight(String),

    #[error("Storage migration already exists: {0}")]
    AlreadyExists(PathBuf),

    #[error("Storage migration document is missing: {0}")]
    MissingDocument(PathBuf),

    #[error("Storage migration document is invalid at {path}: {message}")]
    InvalidDocument { path: PathBuf, message: String },

    #[error("Storage migration source changed for {relative_path}: {reason}")]
    SourceChanged {
        relative_path: PathBuf,
        reason: String,
    },

    #[error("Storage migration target failed verification for {relative_path}: {reason}")]
    TargetMismatch {
        relative_path: PathBuf,
        reason: String,
    },

    #[error("Existing target for {relative_path} was quarantined at {quarantine_path_relative}")]
    ExistingTargetQuarantined {
        relative_path: PathBuf,
        quarantine_path_relative: PathBuf,
    },

    #[error("Partial target for {relative_path} was quarantined at {quarantine_path_relative}")]
    PartialQuarantined {
        relative_path: PathBuf,
        quarantine_path_relative: PathBuf,
    },

    #[error("Storage migration was interrupted")]
    Interrupted,

    #[error("Storage migration was cancelled by the user")]
    CancelledByUser,

    #[error("Storage migration simulated an abnormal process exit")]
    SimulatedCrash,

    #[error("Storage migration injected a target write failure")]
    InjectedWriteFailure,

    #[error("Failed to parse or serialize migration JSON at {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("Failed to {operation} migration file or directory {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl MigrationError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "invalid_request",
            Self::Preflight(_) => "preflight_failed",
            Self::AlreadyExists(_) => "migration_already_exists",
            Self::MissingDocument(_) => "migration_document_missing",
            Self::InvalidDocument { .. } => "migration_document_invalid",
            Self::SourceChanged { .. } => "source_changed",
            Self::TargetMismatch { .. } => "target_hash_mismatch",
            Self::ExistingTargetQuarantined { .. } => "existing_target_quarantined",
            Self::PartialQuarantined { .. } => "partial_quarantined",
            Self::Interrupted => "interrupted",
            Self::CancelledByUser => "cancelled_by_user",
            Self::SimulatedCrash => "simulated_crash",
            Self::InjectedWriteFailure => "target_write_failed",
            Self::Json { .. } => "migration_json_error",
            Self::Io { .. } => "migration_io_error",
            Self::Storage(_) => "storage_validation_error",
        }
    }

    fn relative_path(&self) -> Option<PathBuf> {
        match self {
            Self::SourceChanged { relative_path, .. }
            | Self::TargetMismatch { relative_path, .. }
            | Self::ExistingTargetQuarantined { relative_path, .. }
            | Self::PartialQuarantined { relative_path, .. } => Some(relative_path.clone()),
            _ => None,
        }
    }
}

pub type MigrationResult<T> = Result<T, MigrationError>;

#[derive(Debug, Clone)]
pub struct MigrationEngine {
    source_root: PathBuf,
    target_root: PathBuf,
    migration_id: String,
    chunk_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrationProgressCheckpoint {
    pub completed_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckpointAction {
    Continue,
    Interrupt,
    Cancel,
    SimulateCrash,
    FailWrite,
}

trait MigrationRunControl {
    fn after_checkpoint(&mut self, _checkpoint: MigrationProgressCheckpoint) -> CheckpointAction {
        CheckpointAction::Continue
    }
}

struct ContinueControl;

impl MigrationRunControl for ContinueControl {}

impl MigrationEngine {
    pub fn new(
        source_root: PathBuf,
        target_root: PathBuf,
        migration_id: String,
    ) -> MigrationResult<Self> {
        ensure_absolute_non_root(&source_root)?;
        ensure_absolute_non_root(&target_root)?;
        ensure_no_legacy_overlap(&target_root, &source_root)?;
        validate_migration_id(&migration_id)?;

        Ok(Self {
            source_root,
            target_root,
            migration_id,
            chunk_bytes: DEFAULT_COPY_CHUNK_BYTES,
        })
    }

    pub fn source_root(&self) -> &Path {
        &self.source_root
    }

    pub fn target_root(&self) -> &Path {
        &self.target_root
    }

    pub fn migration_id(&self) -> &str {
        &self.migration_id
    }

    pub fn control_root(&self) -> PathBuf {
        self.source_root.join("migration").join(&self.migration_id)
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.control_root().join("manifest.json")
    }

    pub fn state_path(&self) -> PathBuf {
        self.control_root().join("state.json")
    }

    pub fn prepare(&self, minimum_free_bytes: u64) -> MigrationResult<MigrationState> {
        if !self.source_root.is_dir() {
            return Err(MigrationError::InvalidRequest(format!(
                "source root is missing or is not a directory: {}",
                self.source_root.display()
            )));
        }
        if let Some(path) = first_link_or_reparse_point(&self.source_root)? {
            return Err(MigrationError::Storage(StorageError::UnsafeReparsePoint(
                path,
            )));
        }

        let manifest = build_manifest(&self.source_root, &self.target_root, &self.migration_id)?;
        let required_free_bytes = minimum_free_bytes.max(manifest.total_bytes);
        let report = validate_target_candidate(
            &self.target_root,
            &self.source_root,
            required_free_bytes,
            true,
        );
        if !report.valid {
            return Err(MigrationError::Preflight(report.errors.join("; ")));
        }

        // S2 records the old and proposed settings in migration state, but it
        // never writes the real preferences file. Resolve and validate both
        // snapshots before creating any migration control files.
        let original_preferences = preferences::load(&preferences_path(&self.source_root))?;
        let proposed_preferences = StoragePreferences {
            schema_version: STORAGE_PREFERENCES_SCHEMA_VERSION,
            storage_root: self.target_root.clone(),
            previous_storage_root: Some(self.source_root.clone()),
            migration_id: Some(self.migration_id.clone()),
            migration_completed: false,
        };
        proposed_preferences.validate()?;

        let control_root = self.control_root();
        if let Some(path) = first_link_or_reparse_point(&control_root)? {
            return Err(MigrationError::Storage(StorageError::UnsafeReparsePoint(
                path,
            )));
        }
        if path_entry_exists(&control_root)? {
            return Err(MigrationError::AlreadyExists(control_root));
        }
        let control_parent = control_root.parent().ok_or_else(|| {
            MigrationError::InvalidRequest("migration control root has no parent".into())
        })?;
        if let Some(path) = first_link_or_reparse_point(control_parent)? {
            return Err(MigrationError::Storage(StorageError::UnsafeReparsePoint(
                path,
            )));
        }
        fs::create_dir_all(control_parent).map_err(|source| MigrationError::Io {
            operation: "create migration control parent",
            path: control_parent.to_path_buf(),
            source,
        })?;
        if let Some(path) = first_link_or_reparse_point(control_parent)? {
            return Err(MigrationError::Storage(StorageError::UnsafeReparsePoint(
                path,
            )));
        }
        fs::create_dir(&control_root).map_err(|source| MigrationError::Io {
            operation: "create migration control directory",
            path: control_root.clone(),
            source,
        })?;

        let manifest_document = checked_manifest_document(manifest.clone())?;
        if let Err(error) = write_json_atomic(&self.manifest_path(), &manifest_document) {
            let _ = fs::remove_dir(&control_root);
            return Err(error);
        }

        let now = timestamp();
        let mut state = MigrationState {
            schema_version: MIGRATION_DOCUMENT_SCHEMA_VERSION,
            migration_id: self.migration_id.clone(),
            manifest_sha256: manifest_document.manifest_sha256,
            source_root: self.source_root.clone(),
            target_root: self.target_root.clone(),
            started_at_utc: now.clone(),
            updated_at_utc: now,
            phase: MigrationPhase::InventoryCreated,
            total_files: manifest.file_count,
            completed_files: 0,
            total_bytes: manifest.total_bytes,
            completed_bytes: 0,
            minimum_free_bytes,
            files: manifest
                .files
                .iter()
                .map(|entry| MigrationFileProgress {
                    relative_path: entry.relative_path.clone(),
                    bytes: entry.bytes,
                    sha256: entry.sha256.clone(),
                    copied_bytes: 0,
                    status: MigrationFileStatus::Pending,
                    quarantine_path_relative: None,
                    error_code: None,
                })
                .collect(),
            last_error: None,
            original_preferences,
            proposed_preferences,
            switched: false,
            runtime_validated: false,
            source_files_still_present: true,
            rollback_result: None,
            last_run_stats: MigrationRunStats::default(),
        };
        refresh_progress(&mut state);
        if let Err(error) = self.save_state(&state) {
            let _ = fs::remove_file(self.manifest_path());
            let _ = fs::remove_dir(&control_root);
            return Err(error);
        }
        Ok(state)
    }

    pub fn load_manifest(&self) -> MigrationResult<MigrationManifestDocument> {
        let path = self.manifest_path();
        let document: MigrationManifestDocument = read_json(&path)?;
        validate_manifest_document(&document, &path)?;
        if document.manifest.migration_id != self.migration_id
            || document.manifest.source_root != self.source_root
            || document.manifest.target_root != self.target_root
        {
            return Err(MigrationError::InvalidDocument {
                path,
                message: "manifest identity or roots do not match the engine".into(),
            });
        }
        Ok(document)
    }

    pub fn load_state(&self) -> MigrationResult<MigrationState> {
        let manifest_document = self.load_manifest()?;
        let path = self.state_path();
        let document: MigrationStateDocument = read_json(&path)?;
        validate_state_document(&document, &manifest_document, &path)?;
        if document.state.migration_id != self.migration_id
            || document.state.source_root != self.source_root
            || document.state.target_root != self.target_root
        {
            return Err(MigrationError::InvalidDocument {
                path,
                message: "state identity or roots do not match the engine".into(),
            });
        }
        Ok(document.state)
    }

    pub fn run(&self) -> MigrationResult<MigrationState> {
        self.run_with_control(&mut ContinueControl)
    }

    pub fn run_with_cancellation_and_notifier<F>(
        &self,
        cancelled: &AtomicBool,
        notify: F,
    ) -> MigrationResult<MigrationState>
    where
        F: FnMut(MigrationProgressCheckpoint),
    {
        struct CancellationControl<'a, F> {
            cancelled: &'a AtomicBool,
            notify: F,
        }

        impl<F> MigrationRunControl for CancellationControl<'_, F>
        where
            F: FnMut(MigrationProgressCheckpoint),
        {
            fn after_checkpoint(
                &mut self,
                checkpoint: MigrationProgressCheckpoint,
            ) -> CheckpointAction {
                (self.notify)(checkpoint);
                if self.cancelled.load(Ordering::SeqCst) {
                    CheckpointAction::Cancel
                } else {
                    CheckpointAction::Continue
                }
            }
        }

        self.run_with_control(&mut CancellationControl { cancelled, notify })
    }

    pub fn verify(&self) -> MigrationResult<MigrationState> {
        let manifest = self.load_manifest()?.manifest;
        let mut state = self.load_state()?;
        state.phase = MigrationPhase::Verifying;
        state.last_error = None;
        state.updated_at_utc = timestamp();
        self.save_state(&state)?;
        match self.verify_with_state(&manifest, &mut state) {
            Ok(()) => Ok(state),
            Err(error) => {
                self.persist_failure(&mut state, &error, None)?;
                Err(error)
            }
        }
    }

    fn run_with_control<C: MigrationRunControl>(
        &self,
        control: &mut C,
    ) -> MigrationResult<MigrationState> {
        let manifest_document = self.load_manifest()?;
        let manifest = manifest_document.manifest;
        let mut state = self.load_state()?;
        if matches!(
            state.phase,
            MigrationPhase::SwitchPendingRestart
                | MigrationPhase::ValidatingRuntime
                | MigrationPhase::Completed
                | MigrationPhase::RolledBack
        ) {
            return Err(MigrationError::InvalidRequest(format!(
                "migration cannot copy files while state is {:?}",
                state.phase
            )));
        }

        let remaining_bytes = state.total_bytes.saturating_sub(state.completed_bytes);
        let required_free_bytes = state.minimum_free_bytes.max(remaining_bytes);
        let report = validate_target_candidate(
            &self.target_root,
            &self.source_root,
            required_free_bytes,
            true,
        );
        if !report.valid {
            let error = MigrationError::Preflight(report.errors.join("; "));
            self.persist_failure(&mut state, &error, None)?;
            return Err(error);
        }

        state.phase = MigrationPhase::Copying;
        state.last_error = None;
        state.last_run_stats = MigrationRunStats::default();
        state.updated_at_utc = timestamp();
        self.save_state(&state)?;

        for (index, entry) in manifest.files.iter().enumerate() {
            let result = self.process_file(index, entry, &mut state, control);
            if let Err(error) = result {
                if !matches!(error, MigrationError::SimulatedCrash) {
                    if state.files[index].status != MigrationFileStatus::Quarantined {
                        state.files[index].status = MigrationFileStatus::Failed;
                        state.files[index].error_code = Some(error.code().into());
                        refresh_progress(&mut state);
                    }
                    self.persist_failure(&mut state, &error, Some(entry.relative_path.clone()))?;
                }
                return Err(error);
            }
        }

        state.phase = MigrationPhase::Verifying;
        state.updated_at_utc = timestamp();
        self.save_state(&state)?;
        match self.verify_with_state(&manifest, &mut state) {
            Ok(()) => Ok(state),
            Err(error) => {
                self.persist_failure(&mut state, &error, None)?;
                Err(error)
            }
        }
    }

    fn process_file<C: MigrationRunControl>(
        &self,
        index: usize,
        entry: &MigrationManifestEntry,
        state: &mut MigrationState,
        control: &mut C,
    ) -> MigrationResult<()> {
        let source_path = safe_join(&self.source_root, &entry.relative_path)?;
        validate_source_metadata(&source_path, entry)?;
        let target_path = safe_join(&self.target_root, &entry.relative_path)?;

        if path_entry_exists(&target_path)? {
            if regular_file_matches(&target_path, entry)? {
                let file = &mut state.files[index];
                file.copied_bytes = entry.bytes;
                file.status = MigrationFileStatus::Verified;
                file.quarantine_path_relative = None;
                file.error_code = None;
                state.last_run_stats.skipped_files += 1;
                refresh_progress(state);
                state.updated_at_utc = timestamp();
                self.save_state(state)?;
                return Ok(());
            }

            let quarantine = self.quarantine_path(&target_path, &entry.relative_path, "target")?;
            let file = &mut state.files[index];
            file.copied_bytes = 0;
            file.status = MigrationFileStatus::Quarantined;
            file.quarantine_path_relative = Some(quarantine.clone());
            file.error_code = Some("existing_target_quarantined".into());
            state.last_run_stats.quarantined_files += 1;
            refresh_progress(state);
            return Err(MigrationError::ExistingTargetQuarantined {
                relative_path: entry.relative_path.clone(),
                quarantine_path_relative: quarantine,
            });
        }

        let partial_path = partial_path(&target_path)?;
        let mut copied_bytes = 0;
        if path_entry_exists(&partial_path)? {
            let metadata =
                fs::symlink_metadata(&partial_path).map_err(|source| MigrationError::Io {
                    operation: "inspect partial",
                    path: partial_path.clone(),
                    source,
                })?;
            if !metadata.is_file() || metadata.len() > entry.bytes {
                let quarantine =
                    self.quarantine_path(&partial_path, &entry.relative_path, "partial")?;
                let file = &mut state.files[index];
                file.status = MigrationFileStatus::Quarantined;
                file.copied_bytes = 0;
                file.quarantine_path_relative = Some(quarantine.clone());
                file.error_code = Some("partial_quarantined".into());
                state.last_run_stats.quarantined_files += 1;
                refresh_progress(state);
                return Err(MigrationError::PartialQuarantined {
                    relative_path: entry.relative_path.clone(),
                    quarantine_path_relative: quarantine,
                });
            }

            copied_bytes = metadata.len();
            let source_prefix = sha256_prefix(&source_path, copied_bytes)?;
            let partial_hash = sha256_file(&partial_path)?;
            if source_prefix != partial_hash {
                let quarantine =
                    self.quarantine_path(&partial_path, &entry.relative_path, "partial")?;
                let file = &mut state.files[index];
                file.status = MigrationFileStatus::Quarantined;
                file.copied_bytes = 0;
                file.quarantine_path_relative = Some(quarantine.clone());
                file.error_code = Some("partial_quarantined".into());
                state.last_run_stats.quarantined_files += 1;
                refresh_progress(state);
                return Err(MigrationError::PartialQuarantined {
                    relative_path: entry.relative_path.clone(),
                    quarantine_path_relative: quarantine,
                });
            }
            state.last_run_stats.resumed_from_bytes += copied_bytes;
        }

        let parent = target_path.parent().ok_or_else(|| {
            MigrationError::InvalidRequest(format!(
                "target file has no parent: {}",
                target_path.display()
            ))
        })?;
        if let Some(path) = first_link_or_reparse_point(parent)? {
            return Err(MigrationError::Storage(StorageError::UnsafeReparsePoint(
                path,
            )));
        }
        fs::create_dir_all(parent).map_err(|source| MigrationError::Io {
            operation: "create target parent",
            path: parent.to_path_buf(),
            source,
        })?;
        if let Some(path) = first_link_or_reparse_point(parent)? {
            return Err(MigrationError::Storage(StorageError::UnsafeReparsePoint(
                path,
            )));
        }

        let mut source_file = File::open(&source_path).map_err(|source| MigrationError::Io {
            operation: "open source",
            path: source_path.clone(),
            source,
        })?;
        source_file
            .seek(SeekFrom::Start(copied_bytes))
            .map_err(|source| MigrationError::Io {
                operation: "seek source",
                path: source_path.clone(),
                source,
            })?;
        let mut partial_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&partial_path)
            .map_err(|source| MigrationError::Io {
                operation: "open partial target",
                path: partial_path.clone(),
                source,
            })?;
        let actual_partial_len = partial_file
            .metadata()
            .map_err(|source| MigrationError::Io {
                operation: "inspect opened partial",
                path: partial_path.clone(),
                source,
            })?
            .len();
        if actual_partial_len != copied_bytes {
            return Err(MigrationError::TargetMismatch {
                relative_path: entry.relative_path.clone(),
                reason: "partial length changed while opening".into(),
            });
        }

        let mut buffer = vec![0_u8; self.chunk_bytes.max(1)];
        while copied_bytes < entry.bytes {
            let remaining = (entry.bytes - copied_bytes) as usize;
            let read_len = remaining.min(buffer.len());
            let bytes_read = source_file
                .read(&mut buffer[..read_len])
                .map_err(|source| MigrationError::Io {
                    operation: "read source",
                    path: source_path.clone(),
                    source,
                })?;
            if bytes_read == 0 {
                return Err(MigrationError::SourceChanged {
                    relative_path: entry.relative_path.clone(),
                    reason: "source ended before the manifest byte count".into(),
                });
            }
            partial_file
                .write_all(&buffer[..bytes_read])
                .map_err(|source| MigrationError::Io {
                    operation: "write partial target",
                    path: partial_path.clone(),
                    source,
                })?;
            partial_file
                .sync_data()
                .map_err(|source| MigrationError::Io {
                    operation: "flush partial checkpoint",
                    path: partial_path.clone(),
                    source,
                })?;

            copied_bytes += bytes_read as u64;
            state.last_run_stats.bytes_written += bytes_read as u64;
            let file = &mut state.files[index];
            file.copied_bytes = copied_bytes;
            file.status = MigrationFileStatus::Partial;
            file.quarantine_path_relative = None;
            file.error_code = None;
            refresh_progress(state);
            state.updated_at_utc = timestamp();
            self.save_state(state)?;

            let checkpoint = MigrationProgressCheckpoint {
                completed_bytes: state.completed_bytes,
                total_bytes: state.total_bytes,
            };
            match control.after_checkpoint(checkpoint) {
                CheckpointAction::Continue => {}
                CheckpointAction::Interrupt => return Err(MigrationError::Interrupted),
                CheckpointAction::Cancel => return Err(MigrationError::CancelledByUser),
                CheckpointAction::SimulateCrash => return Err(MigrationError::SimulatedCrash),
                CheckpointAction::FailWrite => return Err(MigrationError::InjectedWriteFailure),
            }
        }
        partial_file
            .sync_all()
            .map_err(|source| MigrationError::Io {
                operation: "finish partial target",
                path: partial_path.clone(),
                source,
            })?;
        drop(partial_file);

        let partial_metadata =
            fs::metadata(&partial_path).map_err(|source| MigrationError::Io {
                operation: "inspect completed partial",
                path: partial_path.clone(),
                source,
            })?;
        if partial_metadata.len() != entry.bytes {
            return Err(MigrationError::TargetMismatch {
                relative_path: entry.relative_path.clone(),
                reason: format!(
                    "partial byte count {} does not match {}",
                    partial_metadata.len(),
                    entry.bytes
                ),
            });
        }
        let actual_hash = sha256_file(&partial_path)?;
        if actual_hash != entry.sha256 {
            let quarantine =
                self.quarantine_path(&partial_path, &entry.relative_path, "hash-mismatch")?;
            let file = &mut state.files[index];
            file.status = MigrationFileStatus::Quarantined;
            file.copied_bytes = 0;
            file.quarantine_path_relative = Some(quarantine.clone());
            file.error_code = Some("target_hash_mismatch".into());
            state.last_run_stats.quarantined_files += 1;
            refresh_progress(state);
            return Err(MigrationError::TargetMismatch {
                relative_path: entry.relative_path.clone(),
                reason: format!(
                    "partial SHA-256 {actual_hash} does not match {}",
                    entry.sha256
                ),
            });
        }

        if path_entry_exists(&target_path)? {
            return Err(MigrationError::TargetMismatch {
                relative_path: entry.relative_path.clone(),
                reason: "target appeared while the verified partial was being prepared".into(),
            });
        }
        fs::rename(&partial_path, &target_path).map_err(|source| MigrationError::Io {
            operation: "atomically publish verified target",
            path: target_path.clone(),
            source,
        })?;
        let file = &mut state.files[index];
        file.status = MigrationFileStatus::Verified;
        file.copied_bytes = entry.bytes;
        file.quarantine_path_relative = None;
        file.error_code = None;
        refresh_progress(state);
        state.updated_at_utc = timestamp();
        self.save_state(state)?;
        Ok(())
    }

    fn verify_with_state(
        &self,
        manifest: &MigrationManifest,
        state: &mut MigrationState,
    ) -> MigrationResult<()> {
        for (index, entry) in manifest.files.iter().enumerate() {
            let source_path = safe_join(&self.source_root, &entry.relative_path)?;
            if let Err(error) = verify_regular_file(&source_path, entry, true) {
                state.source_files_still_present = false;
                state.files[index].status = MigrationFileStatus::Failed;
                state.files[index].error_code = Some(error.code().into());
                refresh_progress(state);
                return Err(error);
            }

            let target_path = safe_join(&self.target_root, &entry.relative_path)?;
            if let Err(error) = verify_regular_file(&target_path, entry, false) {
                state.files[index].status = MigrationFileStatus::Failed;
                state.files[index].error_code = Some(error.code().into());
                refresh_progress(state);
                return Err(error);
            }
            let partial = partial_path(&target_path)?;
            if path_entry_exists(&partial)? {
                let error = MigrationError::TargetMismatch {
                    relative_path: entry.relative_path.clone(),
                    reason: "partial file remains after final publication".into(),
                };
                state.files[index].status = MigrationFileStatus::Failed;
                state.files[index].error_code = Some(error.code().into());
                refresh_progress(state);
                return Err(error);
            }

            state.files[index].status = MigrationFileStatus::Verified;
            state.files[index].copied_bytes = entry.bytes;
            state.files[index].error_code = None;
        }
        state.source_files_still_present = true;
        refresh_progress(state);
        if state.completed_files != state.total_files || state.completed_bytes != state.total_bytes
        {
            return Err(MigrationError::InvalidRequest(
                "verified progress totals do not match the manifest".into(),
            ));
        }
        state.phase = MigrationPhase::ReadyToSwitch;
        state.last_error = None;
        state.updated_at_utc = timestamp();
        self.save_state(state)?;
        Ok(())
    }

    fn quarantine_path(
        &self,
        existing_path: &Path,
        relative_path: &Path,
        label: &str,
    ) -> MigrationResult<PathBuf> {
        let base = self
            .target_root
            .join("migration")
            .join("quarantine")
            .join(&self.migration_id);
        let mut destination = safe_join(&base, relative_path)?;
        let file_name = destination.file_name().ok_or_else(|| {
            MigrationError::InvalidRequest(format!(
                "quarantine path has no file name: {}",
                destination.display()
            ))
        })?;
        let mut quarantined_name = OsString::from(file_name);
        quarantined_name.push(format!(".{label}-{}", Uuid::new_v4().simple()));
        destination.set_file_name(quarantined_name);
        let parent = destination.parent().ok_or_else(|| {
            MigrationError::InvalidRequest("quarantine path has no parent".into())
        })?;
        if let Some(path) = first_link_or_reparse_point(parent)? {
            return Err(MigrationError::Storage(StorageError::UnsafeReparsePoint(
                path,
            )));
        }
        fs::create_dir_all(parent).map_err(|source| MigrationError::Io {
            operation: "create quarantine directory",
            path: parent.to_path_buf(),
            source,
        })?;
        if let Some(path) = first_link_or_reparse_point(parent)? {
            return Err(MigrationError::Storage(StorageError::UnsafeReparsePoint(
                path,
            )));
        }
        fs::rename(existing_path, &destination).map_err(|source| MigrationError::Io {
            operation: "move invalid target to quarantine",
            path: existing_path.to_path_buf(),
            source,
        })?;
        destination
            .strip_prefix(&self.target_root)
            .map(Path::to_path_buf)
            .map_err(|_| {
                MigrationError::InvalidRequest(
                    "quarantine destination escaped the target root".into(),
                )
            })
    }

    fn persist_failure(
        &self,
        state: &mut MigrationState,
        error: &MigrationError,
        fallback_relative_path: Option<PathBuf>,
    ) -> MigrationResult<()> {
        state.phase = MigrationPhase::Failed;
        state.last_error = Some(MigrationFailure {
            code: error.code().into(),
            relative_path: error.relative_path().or(fallback_relative_path),
            message: error.code().into(),
        });
        state.updated_at_utc = timestamp();
        refresh_progress(state);
        self.save_state(state)
    }

    fn save_state(&self, state: &MigrationState) -> MigrationResult<()> {
        validate_state_invariants(state, None, &self.state_path())?;
        let state_sha256 = payload_sha256(state, &self.state_path())?;
        let document = MigrationStateDocument {
            schema_version: MIGRATION_DOCUMENT_SCHEMA_VERSION,
            state_sha256,
            state: state.clone(),
        };
        write_json_atomic(&self.state_path(), &document)
    }
}

fn build_manifest(
    source_root: &Path,
    target_root: &Path,
    migration_id: &str,
) -> MigrationResult<MigrationManifest> {
    let models_root = source_root.join("models");
    if !models_root.is_dir() {
        return Err(MigrationError::InvalidRequest(format!(
            "source models directory is missing: {}",
            models_root.display()
        )));
    }
    if let Some(path) = first_link_or_reparse_point(&models_root)? {
        return Err(MigrationError::Storage(StorageError::UnsafeReparsePoint(
            path,
        )));
    }

    let mut paths = Vec::new();
    collect_regular_files(&models_root, &mut paths)?;
    paths.sort_by(|left, right| left.to_string_lossy().cmp(&right.to_string_lossy()));
    if paths.is_empty() {
        return Err(MigrationError::InvalidRequest(
            "source models directory contains no files".into(),
        ));
    }

    let mut files = Vec::with_capacity(paths.len());
    let mut total_bytes = 0_u64;
    for path in paths {
        let relative_path = path
            .strip_prefix(source_root)
            .map(Path::to_path_buf)
            .map_err(|_| {
                MigrationError::InvalidRequest(format!(
                    "source path escaped the source root: {}",
                    path.display()
                ))
            })?;
        validate_relative_path(&relative_path)?;
        let metadata = fs::metadata(&path).map_err(|source| MigrationError::Io {
            operation: "inspect source model",
            path: path.clone(),
            source,
        })?;
        let bytes = metadata.len();
        total_bytes = total_bytes.checked_add(bytes).ok_or_else(|| {
            MigrationError::InvalidRequest("source model byte total overflowed u64".into())
        })?;
        files.push(MigrationManifestEntry {
            relative_path,
            bytes,
            sha256: sha256_file(&path)?,
        });
    }

    Ok(MigrationManifest {
        schema_version: MIGRATION_DOCUMENT_SCHEMA_VERSION,
        migration_id: migration_id.to_owned(),
        source_root: source_root.to_path_buf(),
        target_root: target_root.to_path_buf(),
        created_at_utc: timestamp(),
        file_count: files.len() as u64,
        total_bytes,
        files,
    })
}

fn collect_regular_files(directory: &Path, output: &mut Vec<PathBuf>) -> MigrationResult<()> {
    let mut entries = fs::read_dir(directory)
        .map_err(|source| MigrationError::Io {
            operation: "read source model directory",
            path: directory.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| MigrationError::Io {
            operation: "enumerate source model directory",
            path: directory.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        if let Some(reparse) = first_link_or_reparse_point(&path)? {
            return Err(MigrationError::Storage(StorageError::UnsafeReparsePoint(
                reparse,
            )));
        }
        let metadata = fs::symlink_metadata(&path).map_err(|source| MigrationError::Io {
            operation: "inspect source model entry",
            path: path.clone(),
            source,
        })?;
        if metadata.is_dir() {
            collect_regular_files(&path, output)?;
        } else if metadata.is_file() {
            output.push(path);
        } else {
            return Err(MigrationError::InvalidRequest(format!(
                "source model entry is not a regular file or directory: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn checked_manifest_document(
    manifest: MigrationManifest,
) -> MigrationResult<MigrationManifestDocument> {
    let path = manifest.source_root.join("migration-manifest-payload");
    let manifest_sha256 = payload_sha256(&manifest, &path)?;
    Ok(MigrationManifestDocument {
        schema_version: MIGRATION_DOCUMENT_SCHEMA_VERSION,
        manifest_sha256,
        manifest,
    })
}

fn validate_manifest_document(
    document: &MigrationManifestDocument,
    path: &Path,
) -> MigrationResult<()> {
    if document.schema_version != MIGRATION_DOCUMENT_SCHEMA_VERSION
        || document.manifest.schema_version != MIGRATION_DOCUMENT_SCHEMA_VERSION
    {
        return Err(MigrationError::InvalidDocument {
            path: path.to_path_buf(),
            message: "unsupported manifest schema version".into(),
        });
    }
    let expected_hash = payload_sha256(&document.manifest, path)?;
    if !document
        .manifest_sha256
        .eq_ignore_ascii_case(&expected_hash)
    {
        return Err(MigrationError::InvalidDocument {
            path: path.to_path_buf(),
            message: "manifest SHA-256 does not match its payload".into(),
        });
    }
    if document.manifest.files.len() as u64 != document.manifest.file_count {
        return Err(MigrationError::InvalidDocument {
            path: path.to_path_buf(),
            message: "manifest file count does not match its entries".into(),
        });
    }
    let mut seen = HashSet::new();
    let mut total_bytes = 0_u64;
    for entry in &document.manifest.files {
        validate_relative_path(&entry.relative_path)?;
        if !seen.insert(entry.relative_path.clone()) {
            return Err(MigrationError::InvalidDocument {
                path: path.to_path_buf(),
                message: format!("duplicate manifest path: {}", entry.relative_path.display()),
            });
        }
        if !is_sha256(&entry.sha256) {
            return Err(MigrationError::InvalidDocument {
                path: path.to_path_buf(),
                message: format!("invalid SHA-256 for {}", entry.relative_path.display()),
            });
        }
        total_bytes = total_bytes.checked_add(entry.bytes).ok_or_else(|| {
            MigrationError::InvalidDocument {
                path: path.to_path_buf(),
                message: "manifest byte total overflow".into(),
            }
        })?;
    }
    if total_bytes != document.manifest.total_bytes {
        return Err(MigrationError::InvalidDocument {
            path: path.to_path_buf(),
            message: "manifest byte total does not match its entries".into(),
        });
    }
    Ok(())
}

fn validate_state_document(
    document: &MigrationStateDocument,
    manifest: &MigrationManifestDocument,
    path: &Path,
) -> MigrationResult<()> {
    if document.schema_version != MIGRATION_DOCUMENT_SCHEMA_VERSION {
        return Err(MigrationError::InvalidDocument {
            path: path.to_path_buf(),
            message: "unsupported state document schema version".into(),
        });
    }
    let expected_hash = payload_sha256(&document.state, path)?;
    if !document.state_sha256.eq_ignore_ascii_case(&expected_hash) {
        return Err(MigrationError::InvalidDocument {
            path: path.to_path_buf(),
            message: "state SHA-256 does not match its payload".into(),
        });
    }
    validate_state_invariants(&document.state, Some(manifest), path)
}

fn validate_state_invariants(
    state: &MigrationState,
    manifest: Option<&MigrationManifestDocument>,
    path: &Path,
) -> MigrationResult<()> {
    if state.schema_version != MIGRATION_DOCUMENT_SCHEMA_VERSION {
        return Err(MigrationError::InvalidDocument {
            path: path.to_path_buf(),
            message: "unsupported state payload schema version".into(),
        });
    }
    if !matches!(
        state.phase,
        MigrationPhase::InventoryCreated
            | MigrationPhase::Copying
            | MigrationPhase::Verifying
            | MigrationPhase::ReadyToSwitch
            | MigrationPhase::Failed
    ) {
        return Err(MigrationError::InvalidDocument {
            path: path.to_path_buf(),
            message: "S2 state contains a phase reserved for a later stage".into(),
        });
    }
    if state.switched || state.runtime_validated || state.rollback_result.is_some() {
        return Err(MigrationError::InvalidDocument {
            path: path.to_path_buf(),
            message: "S2 state cannot claim a switch, runtime validation, or rollback".into(),
        });
    }
    if (state.phase == MigrationPhase::Failed) != state.last_error.is_some() {
        return Err(MigrationError::InvalidDocument {
            path: path.to_path_buf(),
            message: "failed phase and last_error must be recorded together".into(),
        });
    }
    if state.files.len() as u64 != state.total_files {
        return Err(MigrationError::InvalidDocument {
            path: path.to_path_buf(),
            message: "state file count does not match its entries".into(),
        });
    }
    if state.proposed_preferences.migration_completed {
        return Err(MigrationError::InvalidDocument {
            path: path.to_path_buf(),
            message: "S2 proposed preferences cannot mark migration completed".into(),
        });
    }
    if state.proposed_preferences.storage_root != state.target_root
        || state.proposed_preferences.previous_storage_root.as_deref()
            != Some(state.source_root.as_path())
        || state.proposed_preferences.migration_id.as_deref() != Some(state.migration_id.as_str())
    {
        return Err(MigrationError::InvalidDocument {
            path: path.to_path_buf(),
            message: "proposed preferences do not match migration identity".into(),
        });
    }
    let mut seen = HashSet::new();
    let mut calculated_total = 0_u64;
    let mut calculated_completed = 0_u64;
    let mut verified_files = 0_u64;
    for file in &state.files {
        validate_relative_path(&file.relative_path)?;
        if !seen.insert(file.relative_path.clone()) {
            return Err(MigrationError::InvalidDocument {
                path: path.to_path_buf(),
                message: format!("duplicate state path: {}", file.relative_path.display()),
            });
        }
        if !is_sha256(&file.sha256) || file.copied_bytes > file.bytes {
            return Err(MigrationError::InvalidDocument {
                path: path.to_path_buf(),
                message: format!("invalid file progress for {}", file.relative_path.display()),
            });
        }
        if file.status == MigrationFileStatus::Verified && file.copied_bytes != file.bytes {
            return Err(MigrationError::InvalidDocument {
                path: path.to_path_buf(),
                message: format!(
                    "verified file has incomplete bytes: {}",
                    file.relative_path.display()
                ),
            });
        }
        calculated_total = calculated_total.checked_add(file.bytes).ok_or_else(|| {
            MigrationError::InvalidDocument {
                path: path.to_path_buf(),
                message: "state byte total overflow".into(),
            }
        })?;
        calculated_completed = calculated_completed
            .checked_add(file.copied_bytes)
            .ok_or_else(|| MigrationError::InvalidDocument {
                path: path.to_path_buf(),
                message: "state completed byte total overflow".into(),
            })?;
        if file.status == MigrationFileStatus::Verified {
            verified_files += 1;
        }
    }
    if calculated_total != state.total_bytes
        || calculated_completed != state.completed_bytes
        || verified_files != state.completed_files
    {
        return Err(MigrationError::InvalidDocument {
            path: path.to_path_buf(),
            message: "state progress totals do not match file entries".into(),
        });
    }
    if state.phase == MigrationPhase::ReadyToSwitch
        && (state.completed_files != state.total_files
            || state.completed_bytes != state.total_bytes
            || state.files.iter().any(|file| {
                file.status != MigrationFileStatus::Verified || file.copied_bytes != file.bytes
            }))
    {
        return Err(MigrationError::InvalidDocument {
            path: path.to_path_buf(),
            message: "ready_to_switch state contains unfinished files".into(),
        });
    }
    if let Some(manifest) = manifest {
        if state.manifest_sha256 != manifest.manifest_sha256
            || state.migration_id != manifest.manifest.migration_id
            || state.source_root != manifest.manifest.source_root
            || state.target_root != manifest.manifest.target_root
            || state.total_files != manifest.manifest.file_count
            || state.total_bytes != manifest.manifest.total_bytes
        {
            return Err(MigrationError::InvalidDocument {
                path: path.to_path_buf(),
                message: "state does not match the checked manifest".into(),
            });
        }
        for (state_file, manifest_file) in state.files.iter().zip(&manifest.manifest.files) {
            if state_file.relative_path != manifest_file.relative_path
                || state_file.bytes != manifest_file.bytes
                || state_file.sha256 != manifest_file.sha256
            {
                return Err(MigrationError::InvalidDocument {
                    path: path.to_path_buf(),
                    message: "state file identity does not match the manifest".into(),
                });
            }
        }
    }
    Ok(())
}

fn validate_migration_id(value: &str) -> MigrationResult<()> {
    if value.is_empty()
        || value.len() > 80
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(MigrationError::InvalidRequest(
            "migration_id must be 1-80 ASCII letters, digits, '-' or '_'".into(),
        ));
    }
    Ok(())
}

fn validate_relative_path(relative_path: &Path) -> MigrationResult<()> {
    if relative_path.as_os_str().is_empty()
        || relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(MigrationError::InvalidRequest(format!(
            "unsafe migration relative path: {}",
            relative_path.display()
        )));
    }
    Ok(())
}

fn safe_join(root: &Path, relative_path: &Path) -> MigrationResult<PathBuf> {
    validate_relative_path(relative_path)?;
    let joined = root.join(relative_path);
    if !path_is_same_or_descendant(&joined, root) {
        return Err(MigrationError::InvalidRequest(format!(
            "migration path escaped its root: {}",
            relative_path.display()
        )));
    }
    Ok(joined)
}

fn partial_path(target_path: &Path) -> MigrationResult<PathBuf> {
    let file_name = target_path.file_name().ok_or_else(|| {
        MigrationError::InvalidRequest(format!(
            "target path has no file name: {}",
            target_path.display()
        ))
    })?;
    let mut partial_name = OsString::from(file_name);
    partial_name.push(".partial");
    Ok(target_path.with_file_name(partial_name))
}

fn path_entry_exists(path: &Path) -> MigrationResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(MigrationError::Io {
            operation: "inspect path entry",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn validate_source_metadata(
    source_path: &Path,
    entry: &MigrationManifestEntry,
) -> MigrationResult<()> {
    if let Some(path) = first_link_or_reparse_point(source_path)? {
        return Err(MigrationError::Storage(StorageError::UnsafeReparsePoint(
            path,
        )));
    }
    let metadata = fs::metadata(source_path).map_err(|source| MigrationError::Io {
        operation: "inspect source",
        path: source_path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() || metadata.len() != entry.bytes {
        return Err(MigrationError::SourceChanged {
            relative_path: entry.relative_path.clone(),
            reason: format!(
                "source byte count is {}, expected {}",
                metadata.len(),
                entry.bytes
            ),
        });
    }
    Ok(())
}

fn regular_file_matches(path: &Path, entry: &MigrationManifestEntry) -> MigrationResult<bool> {
    if let Some(reparse) = first_link_or_reparse_point(path)? {
        return Err(MigrationError::Storage(StorageError::UnsafeReparsePoint(
            reparse,
        )));
    }
    let metadata = fs::symlink_metadata(path).map_err(|source| MigrationError::Io {
        operation: "inspect existing target",
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() || metadata.len() != entry.bytes {
        return Ok(false);
    }
    Ok(sha256_file(path)? == entry.sha256)
}

fn verify_regular_file(
    path: &Path,
    entry: &MigrationManifestEntry,
    source: bool,
) -> MigrationResult<()> {
    if let Some(reparse) = first_link_or_reparse_point(path)? {
        return Err(MigrationError::Storage(StorageError::UnsafeReparsePoint(
            reparse,
        )));
    }
    if !path_entry_exists(path)? {
        return if source {
            Err(MigrationError::SourceChanged {
                relative_path: entry.relative_path.clone(),
                reason: "file is missing".into(),
            })
        } else {
            Err(MigrationError::TargetMismatch {
                relative_path: entry.relative_path.clone(),
                reason: "file is missing".into(),
            })
        };
    }
    let metadata = fs::symlink_metadata(path).map_err(|io_source| MigrationError::Io {
        operation: "inspect verified file",
        path: path.to_path_buf(),
        source: io_source,
    })?;
    if !metadata.is_file() {
        return if source {
            Err(MigrationError::SourceChanged {
                relative_path: entry.relative_path.clone(),
                reason: "path is not a regular file".into(),
            })
        } else {
            Err(MigrationError::TargetMismatch {
                relative_path: entry.relative_path.clone(),
                reason: "path is not a regular file".into(),
            })
        };
    }
    let actual_hash = sha256_file(path)?;
    if metadata.len() != entry.bytes || actual_hash != entry.sha256 {
        let reason = format!(
            "bytes={}, sha256={actual_hash}; expected bytes={}, sha256={}",
            metadata.len(),
            entry.bytes,
            entry.sha256
        );
        return if source {
            Err(MigrationError::SourceChanged {
                relative_path: entry.relative_path.clone(),
                reason,
            })
        } else {
            Err(MigrationError::TargetMismatch {
                relative_path: entry.relative_path.clone(),
                reason,
            })
        };
    }
    Ok(())
}

fn sha256_file(path: &Path) -> MigrationResult<String> {
    let mut file = File::open(path).map_err(|source| MigrationError::Io {
        operation: "open for SHA-256",
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let bytes_read = file
            .read(&mut buffer)
            .map_err(|source| MigrationError::Io {
                operation: "read for SHA-256",
                path: path.to_path_buf(),
                source,
            })?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn sha256_prefix(path: &Path, bytes: u64) -> MigrationResult<String> {
    let mut file = File::open(path).map_err(|source| MigrationError::Io {
        operation: "open source prefix",
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut remaining = bytes;
    let mut buffer = vec![0_u8; 1024 * 1024];
    while remaining > 0 {
        let read_len = (remaining as usize).min(buffer.len());
        let bytes_read =
            file.read(&mut buffer[..read_len])
                .map_err(|source| MigrationError::Io {
                    operation: "read source prefix",
                    path: path.to_path_buf(),
                    source,
                })?;
        if bytes_read == 0 {
            return Err(MigrationError::InvalidRequest(format!(
                "source prefix ended before {bytes} bytes: {}",
                path.display()
            )));
        }
        hasher.update(&buffer[..bytes_read]);
        remaining -= bytes_read as u64;
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn payload_sha256<T: Serialize>(value: &T, path: &Path) -> MigrationResult<String> {
    let bytes = serde_json::to_vec(value).map_err(|source| MigrationError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn read_json<T: DeserializeOwned>(path: &Path) -> MigrationResult<T> {
    if !path_entry_exists(path)? {
        return Err(MigrationError::MissingDocument(path.to_path_buf()));
    }
    if let Some(reparse) = first_link_or_reparse_point(path)? {
        return Err(MigrationError::Storage(StorageError::UnsafeReparsePoint(
            reparse,
        )));
    }
    let metadata = fs::symlink_metadata(path).map_err(|source| MigrationError::Io {
        operation: "inspect migration document",
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(MigrationError::InvalidDocument {
            path: path.to_path_buf(),
            message: "migration document is not a regular file".into(),
        });
    }
    let bytes = fs::read(path).map_err(|source| MigrationError::Io {
        operation: "read migration document",
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| MigrationError::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> MigrationResult<()> {
    let parent = path.parent().ok_or_else(|| {
        MigrationError::InvalidRequest(format!(
            "migration document has no parent: {}",
            path.display()
        ))
    })?;
    if let Some(reparse) = first_link_or_reparse_point(parent)? {
        return Err(MigrationError::Storage(StorageError::UnsafeReparsePoint(
            reparse,
        )));
    }
    fs::create_dir_all(parent).map_err(|source| MigrationError::Io {
        operation: "create migration document parent",
        path: parent.to_path_buf(),
        source,
    })?;
    if let Some(reparse) = first_link_or_reparse_point(parent)? {
        return Err(MigrationError::Storage(StorageError::UnsafeReparsePoint(
            reparse,
        )));
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("migration.json");
    let temporary_path = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4().simple()));

    let result = (|| -> MigrationResult<()> {
        let bytes = serde_json::to_vec_pretty(value).map_err(|source| MigrationError::Json {
            path: temporary_path.clone(),
            source,
        })?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
            .map_err(|source| MigrationError::Io {
                operation: "create temporary migration document",
                path: temporary_path.clone(),
                source,
            })?;
        file.write_all(&bytes)
            .and_then(|_| file.write_all(b"\n"))
            .map_err(|source| MigrationError::Io {
                operation: "write temporary migration document",
                path: temporary_path.clone(),
                source,
            })?;
        file.sync_all().map_err(|source| MigrationError::Io {
            operation: "flush temporary migration document",
            path: temporary_path.clone(),
            source,
        })?;
        drop(file);
        fs::rename(&temporary_path, path).map_err(|source| MigrationError::Io {
            operation: "atomically replace migration document",
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

fn refresh_progress(state: &mut MigrationState) {
    state.total_files = state.files.len() as u64;
    state.total_bytes = state.files.iter().map(|file| file.bytes).sum();
    state.completed_files = state
        .files
        .iter()
        .filter(|file| file.status == MigrationFileStatus::Verified)
        .count() as u64;
    state.completed_bytes = state.files.iter().map(|file| file.copied_bytes).sum();
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::{json, Value};
    use tempfile::TempDir;

    use super::*;

    type FileSnapshot = BTreeMap<PathBuf, (u64, String)>;

    struct Fixture {
        _temporary_root: TempDir,
        source_root: PathBuf,
        target_root: PathBuf,
        engine: MigrationEngine,
    }

    impl Fixture {
        fn standard(test_id: &str) -> Self {
            Self::with_files(
                test_id,
                "source",
                "target",
                &[
                    ("models/whisper-test.bin", 1_200, 11),
                    ("models/parakeet/parakeet-test.bin", 1_100, 29),
                    ("models/summary/summary-test.gguf", 1_700, 47),
                ],
            )
        }

        fn with_files(
            test_id: &str,
            source_name: &str,
            target_name: &str,
            files: &[(&str, usize, u8)],
        ) -> Self {
            let temporary_root = tempfile::Builder::new()
                .prefix(&format!("meetily-{test_id}-"))
                .tempdir()
                .expect("create isolated S2 temporary directory");
            let source_root = temporary_root.path().join(source_name);
            let target_root = temporary_root.path().join(target_name);
            for (relative_path, bytes, seed) in files {
                write_fixture_file(
                    &source_root.join(relative_path),
                    &patterned_bytes(*bytes, *seed),
                );
            }
            let mut engine = MigrationEngine::new(
                source_root.clone(),
                target_root.clone(),
                format!("migration-{test_id}"),
            )
            .expect("construct S2 migration engine");
            engine.chunk_bytes = 100;
            Self {
                _temporary_root: temporary_root,
                source_root,
                target_root,
                engine,
            }
        }

        fn source_snapshot(&self) -> FileSnapshot {
            snapshot_models(&self.source_root)
        }

        fn target_snapshot(&self) -> FileSnapshot {
            snapshot_models(&self.target_root)
        }

        fn assert_source_unchanged(&self, before: &FileSnapshot) {
            assert_eq!(&self.source_snapshot(), before, "source models changed");
        }
    }

    #[derive(Debug)]
    struct StopAtPercent {
        percent: u64,
        action: CheckpointAction,
        fired: bool,
        observed_bytes: u64,
    }

    impl StopAtPercent {
        fn new(percent: u64, action: CheckpointAction) -> Self {
            Self {
                percent,
                action,
                fired: false,
                observed_bytes: 0,
            }
        }
    }

    impl MigrationRunControl for StopAtPercent {
        fn after_checkpoint(
            &mut self,
            checkpoint: MigrationProgressCheckpoint,
        ) -> CheckpointAction {
            let reached = (checkpoint.completed_bytes as u128) * 100
                >= (checkpoint.total_bytes as u128) * (self.percent as u128);
            if !self.fired && reached {
                self.fired = true;
                self.observed_bytes = checkpoint.completed_bytes;
                return self.action;
            }
            CheckpointAction::Continue
        }
    }

    #[test]
    fn s2_t01_normal_copy_reaches_ready_to_switch() {
        let started = timestamp();
        let fixture = Fixture::standard("t01");
        let source_before = fixture.source_snapshot();

        let prepared = fixture.engine.prepare(0).expect("prepare migration");
        assert_eq!(prepared.phase, MigrationPhase::InventoryCreated);
        assert_eq!(prepared.total_files, 3);
        assert_eq!(prepared.completed_files, 0);

        let finished = fixture.engine.run().expect("copy and verify models");
        assert_ready(&finished);
        assert_eq!(finished.last_run_stats.bytes_written, finished.total_bytes);
        assert_eq!(fixture.target_snapshot(), source_before);
        assert_no_partial_files(&fixture.target_root);
        fixture.assert_source_unchanged(&source_before);

        emit_evidence(
            "S2-T01-normal-copy.json",
            "S2-T01",
            &started,
            json!({
                "phase": finished.phase,
                "fileCount": finished.total_files,
                "totalBytes": finished.total_bytes,
                "targetMatchesSourceSha256": true
            }),
        );
    }

    #[test]
    fn s2_t02_repeat_run_skips_verified_files_without_rewriting() {
        let started = timestamp();
        let fixture = Fixture::standard("t02");
        let source_before = fixture.source_snapshot();
        fixture.engine.prepare(0).unwrap();
        let first = fixture.engine.run().unwrap();
        let target_after_first = fixture.target_snapshot();

        let repeated = fixture.engine.run().expect("repeat verified migration");
        assert_ready(&repeated);
        assert_eq!(repeated.last_run_stats.bytes_written, 0);
        assert_eq!(repeated.last_run_stats.resumed_from_bytes, 0);
        assert_eq!(repeated.last_run_stats.skipped_files, first.total_files);
        assert_eq!(fixture.target_snapshot(), target_after_first);
        fixture.assert_source_unchanged(&source_before);

        emit_evidence(
            "S2-T02-repeat.json",
            "S2-T02",
            &started,
            json!({
                "skippedFiles": repeated.last_run_stats.skipped_files,
                "bytesWrittenOnRepeat": repeated.last_run_stats.bytes_written,
                "targetUnchanged": true
            }),
        );
    }

    #[test]
    fn s2_t03_interrupt_at_ten_percent_resumes_from_verified_prefix() {
        let started = timestamp();
        let fixture = Fixture::standard("t03");
        let source_before = fixture.source_snapshot();
        let prepared = fixture.engine.prepare(0).unwrap();
        let mut control = StopAtPercent::new(10, CheckpointAction::Interrupt);

        let error = fixture.engine.run_with_control(&mut control).unwrap_err();
        assert!(matches!(error, MigrationError::Interrupted));
        assert!(control.fired);
        assert!(control.observed_bytes * 100 >= prepared.total_bytes * 10);
        let interrupted = fixture.engine.load_state().unwrap();
        assert_eq!(interrupted.phase, MigrationPhase::Failed);
        assert!(interrupted.completed_bytes > 0);
        assert!(interrupted.completed_bytes < interrupted.total_bytes);
        assert_has_partial_file(&fixture.target_root);
        fixture.assert_source_unchanged(&source_before);

        let resumed = fixture.engine.run().expect("resume ten-percent partial");
        assert_ready(&resumed);
        assert!(resumed.last_run_stats.resumed_from_bytes > 0);
        assert!(resumed.last_run_stats.bytes_written < resumed.total_bytes);
        assert_eq!(fixture.target_snapshot(), source_before);
        assert_no_partial_files(&fixture.target_root);
        fixture.assert_source_unchanged(&source_before);

        emit_evidence(
            "S2-T03-interruption-10.json",
            "S2-T03",
            &started,
            json!({
                "stoppedAtBytes": control.observed_bytes,
                "totalBytes": prepared.total_bytes,
                "resumedFromBytes": resumed.last_run_stats.resumed_from_bytes,
                "bytesWrittenAfterResume": resumed.last_run_stats.bytes_written,
                "finalPhase": resumed.phase
            }),
        );
    }

    #[test]
    fn s2_t04_abnormal_exit_at_ninety_percent_resumes_without_recopy() {
        let started = timestamp();
        let fixture = Fixture::standard("t04");
        let source_before = fixture.source_snapshot();
        let prepared = fixture.engine.prepare(0).unwrap();
        let mut control = StopAtPercent::new(90, CheckpointAction::SimulateCrash);

        let error = fixture.engine.run_with_control(&mut control).unwrap_err();
        assert!(matches!(error, MigrationError::SimulatedCrash));
        let crashed = fixture.engine.load_state().unwrap();
        assert_eq!(crashed.phase, MigrationPhase::Copying);
        assert_eq!(crashed.last_error, None);
        assert!(crashed.completed_bytes * 100 >= prepared.total_bytes * 90);
        assert!(crashed.completed_bytes < crashed.total_bytes);
        fixture.assert_source_unchanged(&source_before);

        let remaining = crashed.total_bytes - crashed.completed_bytes;
        let resumed = fixture.engine.run().expect("resume after simulated crash");
        assert_ready(&resumed);
        assert_eq!(resumed.last_run_stats.bytes_written, remaining);
        assert!(resumed.last_run_stats.resumed_from_bytes > 0);
        assert!(resumed.last_run_stats.skipped_files > 0);
        assert_eq!(fixture.target_snapshot(), source_before);
        fixture.assert_source_unchanged(&source_before);

        emit_evidence(
            "S2-T04-interruption-90.json",
            "S2-T04",
            &started,
            json!({
                "stoppedAtBytes": crashed.completed_bytes,
                "totalBytes": crashed.total_bytes,
                "remainingBytes": remaining,
                "bytesWrittenAfterResume": resumed.last_run_stats.bytes_written,
                "skippedCompletedFiles": resumed.last_run_stats.skipped_files,
                "finalPhase": resumed.phase
            }),
        );
    }

    #[test]
    fn s2_t05_injected_write_failure_is_recoverable() {
        let started = timestamp();
        let fixture = Fixture::standard("t05");
        let source_before = fixture.source_snapshot();
        fixture.engine.prepare(0).unwrap();
        let mut control = StopAtPercent::new(25, CheckpointAction::FailWrite);

        let error = fixture.engine.run_with_control(&mut control).unwrap_err();
        assert!(matches!(error, MigrationError::InjectedWriteFailure));
        let failed = fixture.engine.load_state().unwrap();
        assert_eq!(failed.phase, MigrationPhase::Failed);
        assert_eq!(
            failed.last_error.as_ref().map(|value| value.code.as_str()),
            Some("target_write_failed")
        );
        assert!(failed.completed_bytes > 0);
        assert_has_partial_file(&fixture.target_root);
        fixture.assert_source_unchanged(&source_before);

        let resumed = fixture.engine.run().expect("resume injected write failure");
        assert_ready(&resumed);
        assert!(resumed.last_run_stats.resumed_from_bytes > 0);
        assert_eq!(fixture.target_snapshot(), source_before);
        fixture.assert_source_unchanged(&source_before);

        emit_evidence(
            "S2-T05-write-failure.json",
            "S2-T05",
            &started,
            json!({
                "failedPhase": failed.phase,
                "failureCode": failed.last_error.map(|value| value.code),
                "partialRetained": true,
                "recoveredPhase": resumed.phase
            }),
        );
    }

    #[test]
    fn s2_t06_insufficient_space_stops_before_control_or_target_files() {
        let started = timestamp();
        let fixture = Fixture::standard("t06");
        let source_before = fixture.source_snapshot();

        let error = fixture.engine.prepare(u64::MAX).unwrap_err();
        assert!(matches!(error, MigrationError::Preflight(_)));
        assert!(!fixture.engine.control_root().exists());
        assert!(snapshot_models(&fixture.target_root).is_empty());
        assert!(!fixture.target_root.join("models").exists());
        fixture.assert_source_unchanged(&source_before);

        emit_evidence(
            "S2-T06-space-failure.json",
            "S2-T06",
            &started,
            json!({
                "requiredBytes": u64::MAX,
                "preflightRejected": true,
                "migrationControlCreated": false,
                "formalTargetFilesCreated": false
            }),
        );
    }

    #[test]
    fn s2_t07_corrupt_existing_target_is_quarantined_before_retry() {
        let started = timestamp();
        let fixture = Fixture::standard("t07");
        let source_before = fixture.source_snapshot();
        fixture.engine.prepare(0).unwrap();
        let manifest = fixture.engine.load_manifest().unwrap().manifest;
        let entry = manifest.files.first().unwrap();
        let target_path = safe_join(&fixture.target_root, &entry.relative_path).unwrap();
        write_fixture_file(&target_path, &vec![0xE7; entry.bytes as usize]);

        let error = fixture.engine.run().unwrap_err();
        assert!(matches!(
            error,
            MigrationError::ExistingTargetQuarantined { .. }
        ));
        let failed = fixture.engine.load_state().unwrap();
        assert_eq!(failed.phase, MigrationPhase::Failed);
        let progress = failed
            .files
            .iter()
            .find(|file| file.relative_path == entry.relative_path)
            .unwrap();
        assert_eq!(progress.status, MigrationFileStatus::Quarantined);
        let quarantine_relative = progress.quarantine_path_relative.as_ref().unwrap();
        assert!(fixture.target_root.join(quarantine_relative).is_file());
        assert!(!target_path.exists());
        fixture.assert_source_unchanged(&source_before);

        let retried = fixture.engine.run().expect("copy after quarantine");
        assert_ready(&retried);
        assert_eq!(fixture.target_snapshot(), source_before);
        fixture.assert_source_unchanged(&source_before);

        emit_evidence(
            "S2-T07-existing-target-corruption.json",
            "S2-T07",
            &started,
            json!({
                "quarantinePathRelative": quarantine_relative,
                "quarantineExists": true,
                "sourceUnchanged": true,
                "retryPhase": retried.phase
            }),
        );
    }

    #[test]
    fn s2_t08_mutated_partial_is_quarantined_and_never_appended() {
        let started = timestamp();
        let fixture = Fixture::standard("t08");
        let source_before = fixture.source_snapshot();
        fixture.engine.prepare(0).unwrap();
        let mut control = StopAtPercent::new(10, CheckpointAction::SimulateCrash);
        assert!(matches!(
            fixture.engine.run_with_control(&mut control),
            Err(MigrationError::SimulatedCrash)
        ));
        let crashed = fixture.engine.load_state().unwrap();
        let partial_progress = crashed
            .files
            .iter()
            .find(|file| file.status == MigrationFileStatus::Partial)
            .expect("one partial file after controlled crash");
        let final_path = safe_join(&fixture.target_root, &partial_progress.relative_path).unwrap();
        let partial = partial_path(&final_path).unwrap();
        let partial_len = fs::metadata(&partial).unwrap().len();
        mutate_one_byte(&partial);

        let error = fixture.engine.run().unwrap_err();
        assert!(matches!(error, MigrationError::PartialQuarantined { .. }));
        let failed = fixture.engine.load_state().unwrap();
        let progress = failed
            .files
            .iter()
            .find(|file| file.relative_path == partial_progress.relative_path)
            .unwrap();
        let quarantine_relative = progress.quarantine_path_relative.as_ref().unwrap();
        let quarantined = fixture.target_root.join(quarantine_relative);
        assert!(quarantined.is_file());
        assert_eq!(fs::metadata(&quarantined).unwrap().len(), partial_len);
        assert!(!partial.exists());
        assert!(!final_path.exists());
        fixture.assert_source_unchanged(&source_before);

        let retried = fixture.engine.run().expect("copy after partial quarantine");
        assert_ready(&retried);
        assert_eq!(fixture.target_snapshot(), source_before);
        fixture.assert_source_unchanged(&source_before);

        emit_evidence(
            "S2-T08-partial-corruption.json",
            "S2-T08",
            &started,
            json!({
                "mutatedPartialBytes": partial_len,
                "quarantinePathRelative": quarantine_relative,
                "continuedAppendingToCorruptPartial": false,
                "retryPhase": retried.phase
            }),
        );
    }

    #[test]
    fn s2_t09_corrupt_state_hash_json_version_and_ready_invariant_are_rejected() {
        let started = timestamp();
        let fixture = Fixture::standard("t09");
        let source_before = fixture.source_snapshot();
        fixture.engine.prepare(0).unwrap();
        let state_path = fixture.engine.state_path();
        let original_state_bytes = fs::read(&state_path).unwrap();
        let original_document: MigrationStateDocument =
            serde_json::from_slice(&original_state_bytes).unwrap();

        fs::write(&state_path, b"{not-valid-json").unwrap();
        assert!(matches!(
            fixture.engine.load_state(),
            Err(MigrationError::Json { .. })
        ));

        let mut hash_mismatch = original_document.clone();
        hash_mismatch.state.phase = MigrationPhase::Copying;
        write_json_atomic(&state_path, &hash_mismatch).unwrap();
        assert!(matches!(
            fixture.engine.load_state(),
            Err(MigrationError::InvalidDocument { .. })
        ));

        let mut unknown_version = original_document.clone();
        unknown_version.schema_version = 2;
        write_json_atomic(&state_path, &unknown_version).unwrap();
        assert!(matches!(
            fixture.engine.load_state(),
            Err(MigrationError::InvalidDocument { .. })
        ));

        let mut false_ready = original_document.clone();
        false_ready.state.phase = MigrationPhase::ReadyToSwitch;
        false_ready.state_sha256 = payload_sha256(&false_ready.state, &state_path).unwrap();
        write_json_atomic(&state_path, &false_ready).unwrap();
        assert!(matches!(
            fixture.engine.load_state(),
            Err(MigrationError::InvalidDocument { .. })
        ));

        let manifest_path = fixture.engine.manifest_path();
        let mut manifest: MigrationManifestDocument = read_json(&manifest_path).unwrap();
        manifest.manifest.total_bytes += 1;
        write_json_atomic(&manifest_path, &manifest).unwrap();
        assert!(matches!(
            fixture.engine.load_manifest(),
            Err(MigrationError::InvalidDocument { .. })
        ));

        assert!(snapshot_models(&fixture.target_root).is_empty());
        fixture.assert_source_unchanged(&source_before);

        emit_evidence(
            "S2-T09-state-corruption.json",
            "S2-T09",
            &started,
            json!({
                "invalidJsonRejected": true,
                "stateHashMismatchRejected": true,
                "unknownSchemaRejected": true,
                "falseReadyStateRejected": true,
                "manifestHashMismatchRejected": true,
                "targetFilesCreated": false
            }),
        );
    }

    #[test]
    fn s2_t10_source_change_after_manifest_causes_hash_failure() {
        let started = timestamp();
        let fixture = Fixture::standard("t10");
        fixture.engine.prepare(0).unwrap();
        let manifest = fixture.engine.load_manifest().unwrap().manifest;
        let changed_entry = manifest.files.first().unwrap();
        let changed_source = safe_join(&fixture.source_root, &changed_entry.relative_path).unwrap();
        let original_hash = sha256_file(&changed_source).unwrap();
        mutate_one_byte(&changed_source);
        assert_eq!(
            fs::metadata(&changed_source).unwrap().len(),
            changed_entry.bytes
        );
        assert_ne!(sha256_file(&changed_source).unwrap(), original_hash);

        let error = fixture.engine.run().unwrap_err();
        assert!(matches!(error, MigrationError::TargetMismatch { .. }));
        let failed = fixture.engine.load_state().unwrap();
        assert_eq!(failed.phase, MigrationPhase::Failed);
        assert_eq!(
            failed.last_error.as_ref().map(|value| value.code.as_str()),
            Some("target_hash_mismatch")
        );
        let changed_target = safe_join(&fixture.target_root, &changed_entry.relative_path).unwrap();
        assert!(!changed_target.exists());

        emit_evidence(
            "S2-T10-source-change.json",
            "S2-T10",
            &started,
            json!({
                "sourceByteCountUnchanged": true,
                "sourceSha256Changed": true,
                "migrationFailed": true,
                "publishedChangedTarget": false,
                "failureCode": failed.last_error.map(|value| value.code)
            }),
        );
    }

    #[test]
    fn s2_t11_target_mutation_after_ready_fails_full_verification() {
        let started = timestamp();
        let fixture = Fixture::standard("t11");
        let source_before = fixture.source_snapshot();
        fixture.engine.prepare(0).unwrap();
        let ready = fixture.engine.run().unwrap();
        assert_ready(&ready);
        let manifest = fixture.engine.load_manifest().unwrap().manifest;
        let entry = manifest.files.first().unwrap();
        let target = safe_join(&fixture.target_root, &entry.relative_path).unwrap();
        mutate_one_byte(&target);

        let error = fixture.engine.verify().unwrap_err();
        assert!(matches!(error, MigrationError::TargetMismatch { .. }));
        let failed = fixture.engine.load_state().unwrap();
        assert_eq!(failed.phase, MigrationPhase::Failed);
        assert!(!failed.switched);
        assert!(!failed.runtime_validated);
        fixture.assert_source_unchanged(&source_before);

        emit_evidence(
            "S2-T11-ready-target-corruption.json",
            "S2-T11",
            &started,
            json!({
                "readyBeforeMutation": true,
                "oneByteMutationDetected": true,
                "finalPhase": failed.phase,
                "switched": failed.switched,
                "runtimeValidated": failed.runtime_validated
            }),
        );
    }

    #[test]
    fn s2_t12_preferences_sentinel_bytes_and_sha256_are_unchanged() {
        let started = timestamp();
        let fixture = Fixture::standard("t12");
        let source_before = fixture.source_snapshot();
        let preferences_file = preferences_path(&fixture.source_root);
        let sentinel_preferences = StoragePreferences {
            schema_version: STORAGE_PREFERENCES_SCHEMA_VERSION,
            storage_root: fixture.source_root.join("旧 配置目标"),
            previous_storage_root: None,
            migration_id: Some("sentinel-not-completed".into()),
            migration_completed: false,
        };
        let mut sentinel_bytes = serde_json::to_vec_pretty(&sentinel_preferences).unwrap();
        sentinel_bytes.extend_from_slice(b"\r\n  ");
        fs::write(&preferences_file, &sentinel_bytes).unwrap();
        let hash_before = sha256_file(&preferences_file).unwrap();

        let prepared = fixture.engine.prepare(0).unwrap();
        assert_eq!(
            prepared.original_preferences.as_ref(),
            Some(&sentinel_preferences)
        );
        let ready = fixture.engine.run().unwrap();
        assert_ready(&ready);
        let bytes_after = fs::read(&preferences_file).unwrap();
        let hash_after = sha256_file(&preferences_file).unwrap();
        assert_eq!(bytes_after, sentinel_bytes);
        assert_eq!(hash_after, hash_before);
        fixture.assert_source_unchanged(&source_before);

        emit_evidence(
            "S2-T12-preferences-sentinel.json",
            "S2-T12",
            &started,
            json!({
                "preferencesBytes": sentinel_bytes.len(),
                "sha256Before": hash_before,
                "sha256After": hash_after,
                "byteForByteUnchanged": true,
                "migrationCompletedWritten": false
            }),
        );
    }

    #[test]
    fn s2_t13_unicode_spaces_and_long_nested_paths_copy_and_resume() {
        let started = timestamp();
        let long_segment =
            "很长的模型目录-012345678901234567890123456789012345678901234567890123456789";
        let long_file =
            format!("models/中文 模型/{long_segment}/第二层 有空格/摘要 模型-最终版.gguf");
        let fixture = Fixture::with_files(
            "t13",
            "来源 数据 中文 空格",
            "目标 数据 中文 空格",
            &[
                (&long_file, 2_300, 83),
                ("models/普通目录/语音 模型.bin", 1_700, 101),
            ],
        );
        let source_before = fixture.source_snapshot();
        fixture.engine.prepare(0).unwrap();
        let mut control = StopAtPercent::new(50, CheckpointAction::SimulateCrash);
        assert!(matches!(
            fixture.engine.run_with_control(&mut control),
            Err(MigrationError::SimulatedCrash)
        ));
        let resumed = fixture.engine.run().expect("resume unicode and long path");
        assert_ready(&resumed);
        assert_eq!(fixture.target_snapshot(), source_before);
        assert_no_partial_files(&fixture.target_root);
        fixture.assert_source_unchanged(&source_before);

        emit_evidence(
            "S2-T13-unicode-long-path.json",
            "S2-T13",
            &started,
            json!({
                "unicodeSourceRoot": true,
                "unicodeTargetRoot": true,
                "longRelativePathCharacters": long_file.chars().count(),
                "resumedFromBytes": resumed.last_run_stats.resumed_from_bytes,
                "finalPhase": resumed.phase,
                "targetMatchesSourceSha256": true
            }),
        );
    }

    #[test]
    fn s2_t14_source_is_unchanged_after_interrupt_resume_and_repeat() {
        let started = timestamp();
        let fixture = Fixture::standard("t14");
        let source_before = fixture.source_snapshot();
        fixture.engine.prepare(0).unwrap();
        let mut control = StopAtPercent::new(35, CheckpointAction::SimulateCrash);
        assert!(matches!(
            fixture.engine.run_with_control(&mut control),
            Err(MigrationError::SimulatedCrash)
        ));
        fixture.assert_source_unchanged(&source_before);

        let resumed = fixture.engine.run().unwrap();
        assert_ready(&resumed);
        fixture.assert_source_unchanged(&source_before);

        let repeated = fixture.engine.run().unwrap();
        assert_ready(&repeated);
        assert_eq!(repeated.last_run_stats.bytes_written, 0);
        assert_eq!(repeated.last_run_stats.skipped_files, repeated.total_files);
        fixture.assert_source_unchanged(&source_before);
        assert_eq!(fixture.target_snapshot(), source_before);

        emit_evidence(
            "S2-T14-source-integrity.json",
            "S2-T14",
            &started,
            json!({
                "sourceFileCountBefore": source_before.len(),
                "sourceFileCountAfter": fixture.source_snapshot().len(),
                "pathsBytesAndSha256Unchanged": true,
                "bytesWrittenOnRepeat": repeated.last_run_stats.bytes_written,
                "skippedFilesOnRepeat": repeated.last_run_stats.skipped_files
            }),
        );
    }

    fn patterned_bytes(length: usize, seed: u8) -> Vec<u8> {
        (0..length)
            .map(|index| seed.wrapping_add((index % 251) as u8))
            .collect()
    }

    fn write_fixture_file(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn mutate_one_byte(path: &Path) {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        let mut first = [0_u8; 1];
        file.read_exact(&mut first).unwrap();
        first[0] ^= 0xFF;
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(&first).unwrap();
        file.sync_all().unwrap();
    }

    fn snapshot_models(storage_root: &Path) -> FileSnapshot {
        let models_root = storage_root.join("models");
        if !models_root.is_dir() {
            return BTreeMap::new();
        }
        let mut paths = Vec::new();
        collect_snapshot_files(&models_root, &mut paths);
        paths.sort();
        paths
            .into_iter()
            .map(|path| {
                let relative = path.strip_prefix(storage_root).unwrap().to_path_buf();
                let metadata = fs::metadata(&path).unwrap();
                let hash = sha256_file(&path).unwrap();
                (relative, (metadata.len(), hash))
            })
            .collect()
    }

    fn collect_snapshot_files(directory: &Path, output: &mut Vec<PathBuf>) {
        if !directory.is_dir() {
            return;
        }
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect_snapshot_files(&path, output);
            } else if path.is_file() {
                output.push(path);
            }
        }
    }

    fn partial_files(root: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        collect_snapshot_files(root, &mut paths);
        paths
            .into_iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.ends_with(".partial"))
                    .unwrap_or(false)
            })
            .collect()
    }

    fn assert_has_partial_file(root: &Path) {
        assert!(
            !partial_files(root).is_empty(),
            "expected a retained partial file under {}",
            root.display()
        );
    }

    fn assert_no_partial_files(root: &Path) {
        assert_eq!(
            partial_files(root),
            Vec::<PathBuf>::new(),
            "unexpected partial file under {}",
            root.display()
        );
    }

    fn assert_ready(state: &MigrationState) {
        assert_eq!(state.phase, MigrationPhase::ReadyToSwitch);
        assert_eq!(state.completed_files, state.total_files);
        assert_eq!(state.completed_bytes, state.total_bytes);
        assert!(state.files.iter().all(|file| {
            file.status == MigrationFileStatus::Verified && file.copied_bytes == file.bytes
        }));
        assert!(state.source_files_still_present);
        assert!(!state.switched);
        assert!(!state.runtime_validated);
        assert!(!state.proposed_preferences.migration_completed);
        assert!(state.last_error.is_none());
    }

    fn emit_evidence(file_name: &str, test_id: &str, started_at: &str, details: Value) {
        let Ok(directory) = std::env::var("MEETILY_S2_EVIDENCE_DIR") else {
            return;
        };
        let directory = PathBuf::from(directory);
        fs::create_dir_all(&directory).expect("create S2 evidence directory");
        let document = json!({
            "schemaVersion": 1,
            "stage": "S2",
            "testId": test_id,
            "status": "PASS",
            "startedAtUtc": started_at,
            "finishedAtUtc": timestamp(),
            "command": "cargo test --test app_lib_tests storage::migration::tests::s2_ -- --test-threads=1",
            "exitCode": 0,
            "testInput": "isolated tempfile directories",
            "temporaryInputOnly": true,
            "formalPathsTouched": false,
            "sourceChanged": false,
            "errors": [],
            "details": details
        });
        let mut bytes = serde_json::to_vec_pretty(&document).unwrap();
        bytes.push(b'\n');
        fs::write(directory.join(file_name), bytes).expect("write S2 test evidence");
    }
}
