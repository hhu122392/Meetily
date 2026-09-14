use super::defaults;
use super::types::Template;
use super::v2::{
    migrate_v1_to_v2, parse_and_validate_template_v2, validate_template_v2, TemplateSource,
    TemplateSourceType, TemplateV2,
};
use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};
use uuid::Uuid;

const TEMPLATE_EXTENSION: &str = "json";
/// 内置模板落成自定义模板的"只做一次"标记文件（放在模板根目录，无 .json 后缀不会被当成模板）
const BUILTIN_SEED_MARKER: &str = ".builtin-seed-v1";
const TRASH_SEPARATOR: &str = "--";
const MAX_TRASH_ID_LENGTH: usize = 180;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateOrigin {
    Builtin,
    Bundled,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateRepositoryErrorKind {
    InvalidId,
    PathRejected,
    DirectoryUnavailable,
    DirectoryNotWritable,
    NotFound,
    AlreadyExists,
    Conflict,
    ReadOnly,
    InvalidTemplate,
    DiskFull,
    Io,
}

impl TemplateRepositoryErrorKind {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidId => "TEMPLATE_ID_INVALID",
            Self::PathRejected => "TEMPLATE_PATH_REJECTED",
            Self::DirectoryUnavailable => "TEMPLATE_DIRECTORY_UNAVAILABLE",
            Self::DirectoryNotWritable => "TEMPLATE_DIRECTORY_NOT_WRITABLE",
            Self::NotFound => "TEMPLATE_NOT_FOUND",
            Self::AlreadyExists => "TEMPLATE_ALREADY_EXISTS",
            Self::Conflict => "TEMPLATE_CONFLICT",
            Self::ReadOnly => "TEMPLATE_READ_ONLY",
            Self::InvalidTemplate => "TEMPLATE_INVALID",
            Self::DiskFull => "TEMPLATE_DISK_FULL",
            Self::Io => "TEMPLATE_IO_ERROR",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateRepositoryError {
    pub kind: TemplateRepositoryErrorKind,
    pub detail: String,
}

impl TemplateRepositoryError {
    fn new(kind: TemplateRepositoryErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        self.kind.code()
    }
}

impl fmt::Display for TemplateRepositoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code(), self.detail)
    }
}

impl std::error::Error for TemplateRepositoryError {}

pub type TemplateRepositoryResult<T> = Result<T, TemplateRepositoryError>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateRecord {
    pub template: TemplateV2,
    pub origin: TemplateOrigin,
    pub schema_version_on_disk: u8,
    pub file_sha256: String,
    pub semantic_sha256: String,
    pub read_only: bool,
    pub overrides_builtin: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateListItem {
    pub id: String,
    pub name: String,
    pub description: String,
    pub origin: TemplateOrigin,
    pub schema_version: u8,
    pub version: u64,
    pub locale: Option<String>,
    pub tags: Vec<String>,
    pub section_count: usize,
    pub source_type: Option<TemplateSourceType>,
    pub updated_at: Option<DateTime<FixedOffset>>,
    pub file_sha256: String,
    pub semantic_sha256: Option<String>,
    pub overrides_builtin: bool,
    pub valid: bool,
    pub validation_error_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateRepositoryDiagnostic {
    pub file_name: String,
    pub code: String,
    pub message_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateListResult {
    pub templates: Vec<TemplateListItem>,
    pub diagnostics: Vec<TemplateRepositoryDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateConflictPolicy {
    Error,
    OverrideReadOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreConflictPolicy {
    Error,
    KeepBoth,
    ReplaceCustom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedTemplateListItem {
    pub trash_id: String,
    pub original_template_id: String,
    pub deleted_at: DateTime<FixedOffset>,
    pub name: String,
    pub file_sha256: String,
    pub valid: bool,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteTemplateResult {
    pub trash_id: String,
    pub deleted_at: DateTime<FixedOffset>,
    pub file_sha256: String,
}

#[derive(Clone)]
pub struct TemplateRepository {
    root: PathBuf,
    bundled_root: Option<PathBuf>,
    write_lock: Arc<Mutex<()>>,
}

struct LoadedTemplate {
    record: TemplateRecord,
}

struct PendingFile {
    path: PathBuf,
}

impl Drop for PendingFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl TemplateRepository {
    pub fn for_current_user(bundled_root: Option<PathBuf>) -> TemplateRepositoryResult<Self> {
        let data_dir = dirs::data_dir().ok_or_else(|| {
            TemplateRepositoryError::new(
                TemplateRepositoryErrorKind::DirectoryUnavailable,
                "the operating system did not provide an application data directory",
            )
        })?;
        Self::from_data_dir(data_dir, bundled_root)
    }

    pub fn from_data_dir(
        data_dir: impl AsRef<Path>,
        bundled_root: Option<PathBuf>,
    ) -> TemplateRepositoryResult<Self> {
        Self::new(templates_root_from_data_dir(data_dir), bundled_root)
    }

    pub fn new(
        custom_root: impl AsRef<Path>,
        bundled_root: Option<PathBuf>,
    ) -> TemplateRepositoryResult<Self> {
        let root = ensure_repository_directory(custom_root.as_ref())?;
        for child in [".trash", ".backup", ".tmp"] {
            ensure_child_directory(&root, child)?;
        }
        verify_writable(&root.join(".tmp"))?;

        let bundled_root = match bundled_root {
            Some(path) if path.exists() => {
                reject_reparse_point(&path)?;
                if !path.is_dir() {
                    return Err(TemplateRepositoryError::new(
                        TemplateRepositoryErrorKind::DirectoryUnavailable,
                        "the bundled templates path is not a directory",
                    ));
                }
                Some(fs::canonicalize(path).map_err(map_directory_error)?)
            }
            _ => None,
        };

        let repository = Self {
            root,
            bundled_root,
            write_lock: Arc::new(Mutex::new(())),
        };
        repository.cleanup_stale_temp_files(Duration::from_secs(24 * 60 * 60))?;
        Ok(repository)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn trash_root(&self) -> PathBuf {
        self.root.join(".trash")
    }

    pub fn backup_root(&self) -> PathBuf {
        self.root.join(".backup")
    }

    pub fn list(&self) -> TemplateRepositoryResult<TemplateListResult> {
        self.list_for_content_locale(None)
    }

    pub fn list_for_content_locale(
        &self,
        content_locale: Option<&str>,
    ) -> TemplateRepositoryResult<TemplateListResult> {
        self.ensure_safe_directories()?;
        self.seed_builtins_as_custom_once(content_locale)?;
        let mut diagnostics = Vec::new();
        let mut ids = BTreeSet::new();

        self.collect_file_ids(
            self.bundled_root.as_deref(),
            TemplateOrigin::Bundled,
            &mut ids,
            &mut diagnostics,
        )?;
        self.collect_file_ids(
            Some(&self.root),
            TemplateOrigin::Custom,
            &mut ids,
            &mut diagnostics,
        )?;

        let mut templates = Vec::new();
        for id in ids {
            let origin = self.effective_origin(&id)?;
            let overrides_builtin =
                origin == TemplateOrigin::Custom && self.read_only_origin_exists(&id)?;
            match self.load_from_origin_for_content_locale(&id, origin, content_locale) {
                Ok(mut loaded) => {
                    loaded.record.overrides_builtin = overrides_builtin;
                    let template = loaded.record.template;
                    templates.push(TemplateListItem {
                        id: template.id,
                        name: template.name,
                        description: template.description,
                        origin,
                        schema_version: loaded.record.schema_version_on_disk,
                        version: template.version,
                        locale: template.locale,
                        tags: template.tags,
                        section_count: template.sections.len(),
                        source_type: Some(template.source.source_type),
                        updated_at: if loaded.record.schema_version_on_disk == 1 {
                            None
                        } else {
                            Some(template.updated_at)
                        },
                        file_sha256: loaded.record.file_sha256,
                        semantic_sha256: Some(loaded.record.semantic_sha256),
                        overrides_builtin,
                        valid: true,
                        validation_error_count: 0,
                    });
                }
                Err(error) => {
                    let invalid_bytes = self.read_origin_bytes(&id, origin).ok();
                    let file_sha256 = invalid_bytes.as_deref().map(sha256_hex).unwrap_or_default();
                    let schema_version = invalid_bytes
                        .as_deref()
                        .map(detect_schema_version)
                        .unwrap_or(1);
                    diagnostics.push(TemplateRepositoryDiagnostic {
                        file_name: format!("{id}.json"),
                        code: error.code().to_owned(),
                        message_key: message_key_for_error(error.kind).to_owned(),
                    });
                    templates.push(TemplateListItem {
                        id: id.clone(),
                        name: id,
                        description: String::new(),
                        origin,
                        schema_version,
                        version: 0,
                        locale: None,
                        tags: Vec::new(),
                        section_count: 0,
                        source_type: None,
                        updated_at: None,
                        file_sha256,
                        semantic_sha256: None,
                        overrides_builtin,
                        valid: false,
                        validation_error_count: 1,
                    });
                }
            }
        }

        templates.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        diagnostics.sort_by(|left, right| left.file_name.cmp(&right.file_name));
        diagnostics
            .dedup_by(|left, right| left.file_name == right.file_name && left.code == right.code);

        Ok(TemplateListResult {
            templates,
            diagnostics,
        })
    }

    pub fn get(
        &self,
        template_id: &str,
        origin: Option<TemplateOrigin>,
    ) -> TemplateRepositoryResult<TemplateRecord> {
        self.get_for_content_locale(template_id, origin, None)
    }

    pub fn get_for_content_locale(
        &self,
        template_id: &str,
        origin: Option<TemplateOrigin>,
        content_locale: Option<&str>,
    ) -> TemplateRepositoryResult<TemplateRecord> {
        validate_template_id(template_id)?;
        self.ensure_safe_directories()?;
        let origin = match origin {
            Some(origin) => origin,
            None => self.effective_origin(template_id)?,
        };
        let mut loaded =
            self.load_from_origin_for_content_locale(template_id, origin, content_locale)?;
        loaded.record.overrides_builtin =
            origin == TemplateOrigin::Custom && self.read_only_origin_exists(template_id)?;
        Ok(loaded.record)
    }

    /// Returns the exact source bytes for diagnostics after applying the same ID,
    /// origin, root and reparse-point checks as a normal repository read.
    pub fn read_bytes_for_diagnostics(
        &self,
        template_id: &str,
        origin: Option<TemplateOrigin>,
    ) -> TemplateRepositoryResult<Vec<u8>> {
        validate_template_id(template_id)?;
        self.ensure_safe_directories()?;
        let origin = match origin {
            Some(origin) => origin,
            None => self.effective_origin(template_id)?,
        };
        self.read_origin_bytes(template_id, origin)
    }

    pub fn create(
        &self,
        template: TemplateV2,
        conflict_policy: CreateConflictPolicy,
    ) -> TemplateRepositoryResult<TemplateRecord> {
        self.create_at(template, conflict_policy, Utc::now().fixed_offset())
    }

    pub fn create_at(
        &self,
        mut template: TemplateV2,
        conflict_policy: CreateConflictPolicy,
        now: DateTime<FixedOffset>,
    ) -> TemplateRepositoryResult<TemplateRecord> {
        let _guard = self.lock_writes()?;
        self.ensure_safe_directories()?;
        validate_template_id(&template.id)?;
        let target = self.custom_template_path(&template.id)?;
        if path_entry_exists(&target) {
            return Err(TemplateRepositoryError::new(
                TemplateRepositoryErrorKind::AlreadyExists,
                "a custom template with this ID already exists",
            ));
        }
        let overrides_builtin = self.read_only_origin_exists(&template.id)?;
        if overrides_builtin && conflict_policy == CreateConflictPolicy::Error {
            return Err(TemplateRepositoryError::new(
                TemplateRepositoryErrorKind::AlreadyExists,
                "a read-only template with this ID already exists",
            ));
        }

        template.schema_version = 2;
        template.version = 1;
        template.created_at = now;
        template.updated_at = now;
        ensure_valid_template(&template)?;
        self.write_template_atomic(&target, &template, None)?;

        let mut record = self
            .load_from_origin(&template.id, TemplateOrigin::Custom)?
            .record;
        record.overrides_builtin = overrides_builtin;
        Ok(record)
    }

    pub fn update(
        &self,
        template_id: &str,
        expected_version: u64,
        expected_file_sha256: &str,
        replacement: TemplateV2,
    ) -> TemplateRepositoryResult<TemplateRecord> {
        self.update_at(
            template_id,
            expected_version,
            expected_file_sha256,
            replacement,
            Utc::now().fixed_offset(),
        )
    }

    pub fn update_at(
        &self,
        template_id: &str,
        expected_version: u64,
        expected_file_sha256: &str,
        mut replacement: TemplateV2,
        now: DateTime<FixedOffset>,
    ) -> TemplateRepositoryResult<TemplateRecord> {
        let _guard = self.lock_writes()?;
        self.ensure_safe_directories()?;
        validate_template_id(template_id)?;
        if replacement.id != template_id {
            return Err(TemplateRepositoryError::new(
                TemplateRepositoryErrorKind::InvalidId,
                "a template ID cannot be changed during update",
            ));
        }

        let target = self.custom_template_path(template_id)?;
        if !path_entry_exists(&target) {
            if self.read_only_origin_exists(template_id)? {
                return Err(TemplateRepositoryError::new(
                    TemplateRepositoryErrorKind::ReadOnly,
                    "read-only templates must be duplicated before editing",
                ));
            }
            return Err(not_found("the custom template does not exist"));
        }
        let current = self.load_from_origin(template_id, TemplateOrigin::Custom)?;
        require_expected_state(&current.record, expected_version, expected_file_sha256)?;

        replacement.schema_version = 2;
        replacement.id = template_id.to_owned();
        replacement.created_at = if current.record.schema_version_on_disk == 1 {
            now
        } else {
            current.record.template.created_at
        };
        replacement.updated_at = now;
        replacement.source = current.record.template.source.clone();
        replacement.version = if current.record.schema_version_on_disk == 1 {
            1
        } else {
            current
                .record
                .template
                .version
                .checked_add(1)
                .ok_or_else(|| {
                    TemplateRepositoryError::new(
                        TemplateRepositoryErrorKind::InvalidTemplate,
                        "the template version cannot be incremented",
                    )
                })?
        };
        ensure_valid_template(&replacement)?;

        let bytes_before_replace = self.read_custom_file(template_id)?;
        if sha256_hex(&bytes_before_replace) != expected_file_sha256 {
            return Err(conflict("the template changed before it could be replaced"));
        }
        self.write_template_atomic(
            &target,
            &replacement,
            Some((current.record.template.version, &bytes_before_replace)),
        )?;

        let mut record = self
            .load_from_origin(template_id, TemplateOrigin::Custom)?
            .record;
        record.overrides_builtin = self.read_only_origin_exists(template_id)?;
        Ok(record)
    }

    pub fn delete(
        &self,
        template_id: &str,
        expected_file_sha256: &str,
    ) -> TemplateRepositoryResult<DeleteTemplateResult> {
        self.delete_at(template_id, expected_file_sha256, Utc::now().fixed_offset())
    }

    pub fn delete_at(
        &self,
        template_id: &str,
        expected_file_sha256: &str,
        deleted_at: DateTime<FixedOffset>,
    ) -> TemplateRepositoryResult<DeleteTemplateResult> {
        let _guard = self.lock_writes()?;
        self.delete_locked(template_id, expected_file_sha256, deleted_at)
    }

    pub fn list_deleted(&self) -> TemplateRepositoryResult<Vec<DeletedTemplateListItem>> {
        self.ensure_safe_directories()?;
        let trash_root = self.trash_root();
        let mut deleted = Vec::new();

        for entry in fs::read_dir(&trash_root).map_err(map_directory_error)? {
            let entry = entry.map_err(map_io_error)?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some(TEMPLATE_EXTENSION) {
                continue;
            }
            reject_reparse_point(&path)?;
            if !entry.file_type().map_err(map_io_error)?.is_file() {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            let Ok((original_template_id, deleted_at)) = parse_trash_id(stem) else {
                continue;
            };
            let bytes = fs::read(&path).map_err(map_io_error)?;
            let file_sha256 = sha256_hex(&bytes);
            match load_template_bytes(&original_template_id, TemplateOrigin::Custom, &bytes) {
                Ok(loaded) => deleted.push(DeletedTemplateListItem {
                    trash_id: stem.to_owned(),
                    original_template_id,
                    deleted_at,
                    name: loaded.record.template.name,
                    file_sha256,
                    valid: true,
                    error_code: None,
                }),
                Err(error) => deleted.push(DeletedTemplateListItem {
                    trash_id: stem.to_owned(),
                    original_template_id: original_template_id.clone(),
                    deleted_at,
                    name: original_template_id,
                    file_sha256,
                    valid: false,
                    error_code: Some(error.code().to_owned()),
                }),
            }
        }

        deleted.sort_by(|left, right| {
            right
                .deleted_at
                .cmp(&left.deleted_at)
                .then_with(|| left.trash_id.cmp(&right.trash_id))
        });
        Ok(deleted)
    }

    pub fn restore(
        &self,
        trash_id: &str,
        conflict_policy: RestoreConflictPolicy,
    ) -> TemplateRepositoryResult<TemplateRecord> {
        self.restore_at(trash_id, conflict_policy, Utc::now().fixed_offset())
    }

    pub fn restore_at(
        &self,
        trash_id: &str,
        conflict_policy: RestoreConflictPolicy,
        now: DateTime<FixedOffset>,
    ) -> TemplateRepositoryResult<TemplateRecord> {
        let _guard = self.lock_writes()?;
        self.ensure_safe_directories()?;
        let (original_template_id, _) = parse_trash_id(trash_id)?;
        let trash_path = self.trash_path(trash_id)?;
        if !path_entry_exists(&trash_path) {
            return Err(not_found("the deleted template does not exist"));
        }
        reject_reparse_point(&trash_path)?;
        let trash_bytes = fs::read(&trash_path).map_err(map_io_error)?;
        let deleted_template =
            load_template_bytes(&original_template_id, TemplateOrigin::Custom, &trash_bytes)?;
        let target = self.custom_template_path(&original_template_id)?;

        if !path_entry_exists(&target) {
            fs::rename(&trash_path, &target).map_err(map_io_error)?;
            sync_parent_directory(&self.root)?;
            return self
                .load_from_origin(&original_template_id, TemplateOrigin::Custom)
                .map(|loaded| loaded.record);
        }

        match conflict_policy {
            RestoreConflictPolicy::Error => Err(TemplateRepositoryError::new(
                TemplateRepositoryErrorKind::AlreadyExists,
                "a custom template with the original ID already exists",
            )),
            RestoreConflictPolicy::KeepBoth => {
                let new_id = self.next_available_restored_id(&original_template_id)?;
                let mut replacement = deleted_template.record.template;
                replacement.id = new_id.clone();
                replacement.version = 1;
                replacement.created_at = now;
                replacement.updated_at = now;
                replacement.source = TemplateSource {
                    source_type: TemplateSourceType::Duplicate,
                    original_file_name: None,
                    original_file_sha256: None,
                    imported_at: None,
                    copied_from_template_id: Some(original_template_id),
                };
                ensure_valid_template(&replacement)?;
                let destination = self.custom_template_path(&new_id)?;
                self.write_template_atomic(&destination, &replacement, None)?;
                if let Err(error) = fs::remove_file(&trash_path) {
                    let _ = fs::remove_file(&destination);
                    return Err(map_io_error(error));
                }
                self.load_from_origin(&new_id, TemplateOrigin::Custom)
                    .map(|loaded| loaded.record)
            }
            RestoreConflictPolicy::ReplaceCustom => {
                let replacement_deleted_at = now;
                let replacement_trash_id = make_trash_id(
                    &original_template_id,
                    replacement_deleted_at,
                    Uuid::new_v4(),
                );
                let replacement_trash_path = self.trash_path(&replacement_trash_id)?;
                fs::rename(&target, &replacement_trash_path).map_err(map_io_error)?;
                if let Err(error) = fs::rename(&trash_path, &target) {
                    let _ = fs::rename(&replacement_trash_path, &target);
                    return Err(map_io_error(error));
                }
                sync_parent_directory(&self.root)?;
                let restored = self
                    .load_from_origin(&original_template_id, TemplateOrigin::Custom)
                    .map(|loaded| loaded.record);
                if restored.is_err() {
                    let _ = fs::rename(&target, &trash_path);
                    let _ = fs::rename(&replacement_trash_path, &target);
                }
                restored
            }
        }
    }

    pub fn purge(&self, trash_id: &str) -> TemplateRepositoryResult<()> {
        let _guard = self.lock_writes()?;
        self.ensure_safe_directories()?;
        parse_trash_id(trash_id)?;
        let path = self.trash_path(trash_id)?;
        if !path_entry_exists(&path) {
            return Err(not_found("the deleted template does not exist"));
        }
        reject_reparse_point(&path)?;
        fs::remove_file(path).map_err(map_io_error)
    }

    pub fn cleanup_stale_temp_files(&self, max_age: Duration) -> TemplateRepositoryResult<usize> {
        let temporary_root = ensure_child_directory(&self.root, ".tmp")?;
        let now = SystemTime::now();
        let mut removed = 0;
        for entry in fs::read_dir(&temporary_root).map_err(map_directory_error)? {
            let entry = entry.map_err(map_io_error)?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("tmp") {
                continue;
            }
            reject_reparse_point(&path)?;
            if !entry.file_type().map_err(map_io_error)?.is_file() {
                continue;
            }
            let modified = entry
                .metadata()
                .map_err(map_io_error)?
                .modified()
                .map_err(map_io_error)?;
            let age = now.duration_since(modified).unwrap_or_default();
            if age >= max_age {
                fs::remove_file(path).map_err(map_io_error)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    fn delete_locked(
        &self,
        template_id: &str,
        expected_file_sha256: &str,
        deleted_at: DateTime<FixedOffset>,
    ) -> TemplateRepositoryResult<DeleteTemplateResult> {
        self.ensure_safe_directories()?;
        validate_template_id(template_id)?;
        let target = self.custom_template_path(template_id)?;
        if !path_entry_exists(&target) {
            if self.read_only_origin_exists(template_id)? {
                return Err(TemplateRepositoryError::new(
                    TemplateRepositoryErrorKind::ReadOnly,
                    "read-only templates cannot be deleted",
                ));
            }
            return Err(not_found("the custom template does not exist"));
        }
        let bytes = self.read_custom_file(template_id)?;
        let file_sha256 = sha256_hex(&bytes);
        if file_sha256 != expected_file_sha256 {
            return Err(conflict("the template changed before deletion"));
        }

        let trash_id = make_trash_id(template_id, deleted_at, Uuid::new_v4());
        let trash_path = self.trash_path(&trash_id)?;
        fs::rename(&target, &trash_path).map_err(map_io_error)?;
        sync_parent_directory(&self.root)?;
        Ok(DeleteTemplateResult {
            trash_id,
            deleted_at,
            file_sha256,
        })
    }

    fn write_template_atomic(
        &self,
        target: &Path,
        template: &TemplateV2,
        backup_source: Option<(u64, &[u8])>,
    ) -> TemplateRepositoryResult<()> {
        ensure_valid_template(template)?;
        let mut bytes = serde_json::to_vec_pretty(template).map_err(|error| {
            TemplateRepositoryError::new(
                TemplateRepositoryErrorKind::InvalidTemplate,
                format!("the template could not be serialized: {error}"),
            )
        })?;
        bytes.push(b'\n');

        let temporary_path =
            self.root
                .join(".tmp")
                .join(format!("{}.{}.tmp", template.id, Uuid::new_v4()));
        let pending = PendingFile {
            path: temporary_path.clone(),
        };
        let mut temporary_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
            .map_err(map_write_error)?;
        temporary_file.write_all(&bytes).map_err(map_write_error)?;
        temporary_file.sync_all().map_err(map_write_error)?;
        drop(temporary_file);

        let read_back = fs::read(&temporary_path).map_err(map_io_error)?;
        if read_back != bytes {
            return Err(TemplateRepositoryError::new(
                TemplateRepositoryErrorKind::Io,
                "the temporary template failed byte-for-byte verification",
            ));
        }
        parse_and_validate_template_v2(std::str::from_utf8(&read_back).map_err(|_| {
            TemplateRepositoryError::new(
                TemplateRepositoryErrorKind::InvalidTemplate,
                "the serialized template is not UTF-8",
            )
        })?)
        .map_err(|_| invalid_template("the temporary template failed validation"))?;

        let backup_path = if let Some((version, original_bytes)) = backup_source {
            let path = self.backup_root().join(format!(
                "{}-v{}-{}.json",
                template.id,
                version,
                Uuid::new_v4()
            ));
            let mut backup = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(map_write_error)?;
            backup.write_all(original_bytes).map_err(map_write_error)?;
            backup.sync_all().map_err(map_write_error)?;
            Some(path)
        } else {
            None
        };

        fs::rename(&temporary_path, target).map_err(map_write_error)?;
        drop(pending);
        sync_parent_directory(&self.root)?;

        let persisted_is_valid = fs::read(target)
            .ok()
            .filter(|persisted| persisted == &bytes)
            .and_then(|persisted| std::str::from_utf8(&persisted).ok().map(str::to_owned))
            .is_some_and(|persisted| parse_and_validate_template_v2(&persisted).is_ok());
        if !persisted_is_valid {
            if let Some(backup_path) = backup_path {
                fs::rename(backup_path, target).map_err(map_write_error)?;
            } else {
                let _ = fs::remove_file(target);
            }
            return Err(TemplateRepositoryError::new(
                TemplateRepositoryErrorKind::Io,
                "the persisted template failed post-write verification",
            ));
        }
        Ok(())
    }

    fn load_from_origin(
        &self,
        template_id: &str,
        origin: TemplateOrigin,
    ) -> TemplateRepositoryResult<LoadedTemplate> {
        self.load_from_origin_for_content_locale(template_id, origin, None)
    }

    fn load_from_origin_for_content_locale(
        &self,
        template_id: &str,
        origin: TemplateOrigin,
        content_locale: Option<&str>,
    ) -> TemplateRepositoryResult<LoadedTemplate> {
        let bytes =
            self.read_origin_bytes_for_content_locale(template_id, origin, content_locale)?;
        load_template_bytes(template_id, origin, &bytes)
    }

    fn read_origin_bytes(
        &self,
        template_id: &str,
        origin: TemplateOrigin,
    ) -> TemplateRepositoryResult<Vec<u8>> {
        self.read_origin_bytes_for_content_locale(template_id, origin, None)
    }

    fn read_origin_bytes_for_content_locale(
        &self,
        template_id: &str,
        origin: TemplateOrigin,
        content_locale: Option<&str>,
    ) -> TemplateRepositoryResult<Vec<u8>> {
        validate_template_id(template_id)?;
        match origin {
            TemplateOrigin::Custom => self.read_custom_file(template_id),
            TemplateOrigin::Bundled => {
                let root = self
                    .bundled_root
                    .as_ref()
                    .ok_or_else(|| not_found("the bundled templates directory is unavailable"))?;
                let path = safe_existing_template_path(root, template_id)?;
                fs::read(path).map_err(map_io_error)
            }
            TemplateOrigin::Builtin => {
                defaults::get_builtin_template_for_locale(template_id, content_locale)
                    .map(|resolved| resolved.content.as_bytes().to_vec())
                    .ok_or_else(|| not_found("the built-in template does not exist"))
            }
        }
    }

    fn read_custom_file(&self, template_id: &str) -> TemplateRepositoryResult<Vec<u8>> {
        let path = self.custom_template_path(template_id)?;
        if !path_entry_exists(&path) {
            return Err(not_found("the custom template does not exist"));
        }
        reject_reparse_point(&path)?;
        if !path.is_file() {
            return Err(TemplateRepositoryError::new(
                TemplateRepositoryErrorKind::PathRejected,
                "the template path is not a regular file",
            ));
        }
        fs::read(path).map_err(map_io_error)
    }

    fn effective_origin(&self, template_id: &str) -> TemplateRepositoryResult<TemplateOrigin> {
        let custom_path = self.custom_template_path(template_id)?;
        if path_entry_exists(&custom_path) {
            return Ok(TemplateOrigin::Custom);
        }
        // Localized built-ins supersede legacy flat bundled resources with the
        // same stable ID. This keeps upgrades from pinning users to the old
        // English-only V1 copy while preserving custom templates as the
        // authoritative override.
        if defaults::get_builtin_template(template_id).is_some() {
            return Ok(TemplateOrigin::Builtin);
        }
        if let Some(root) = self.bundled_root.as_ref() {
            let bundled_path = root.join(format!("{template_id}.json"));
            if path_entry_exists(&bundled_path) {
                return Ok(TemplateOrigin::Bundled);
            }
        }
        Err(not_found("the template does not exist"))
    }

    fn read_only_origin_exists(&self, template_id: &str) -> TemplateRepositoryResult<bool> {
        validate_template_id(template_id)?;
        let bundled_exists = self
            .bundled_root
            .as_ref()
            .map(|root| path_entry_exists(&root.join(format!("{template_id}.json"))))
            .unwrap_or(false);
        Ok(bundled_exists || defaults::get_builtin_template(template_id).is_some())
    }

    /// 内置模板不再作为单独的只读类别出现：首次运行时把它们**原样**落成普通自定义模板文件，
    /// 之后内置注册表只保留"按 id 兜底解析"的作用（老会议偏好、默认模板仍能解析）。
    ///
    /// 用标记文件保证只做一次：用户把某个内置模板删进回收站后，重启不会再被塞回来。
    fn seed_builtins_as_custom_once(
        &self,
        content_locale: Option<&str>,
    ) -> TemplateRepositoryResult<()> {
        // 只有拿到界面内容语言时才落盘：内置模板是分语言的，先落英文再改就晚了。
        // 没有语言（内部调用）时跳过，等真正带语言的那次列表请求来落地。
        if content_locale.is_none() {
            return Ok(());
        }
        let marker = self.root.join(BUILTIN_SEED_MARKER);
        if path_entry_exists(&marker) {
            return Ok(());
        }

        for id in defaults::list_builtin_template_ids() {
            let target = self.custom_template_path(id)?;
            if path_entry_exists(&target) {
                continue;
            }
            let Some(resolved) = defaults::get_builtin_template_for_locale(id, content_locale) else {
                continue;
            };
            // 直接写原始内容：与随应用发布的那份完全一致，读回来仍会走完整校验。
            fs::write(&target, resolved.content.as_bytes()).map_err(map_io_error)?;
        }

        fs::write(&marker, b"seeded").map_err(map_io_error)?;
        Ok(())
    }

    fn custom_template_path(&self, template_id: &str) -> TemplateRepositoryResult<PathBuf> {
        validate_template_id(template_id)?;
        let path = self.root.join(format!("{template_id}.json"));
        ensure_parent_is_root(&path, &self.root)?;
        if path_entry_exists(&path) {
            reject_reparse_point(&path)?;
        }
        Ok(path)
    }

    fn trash_path(&self, trash_id: &str) -> TemplateRepositoryResult<PathBuf> {
        validate_trash_id(trash_id)?;
        let root = self.trash_root();
        let path = root.join(format!("{trash_id}.json"));
        ensure_parent_is_root(&path, &root)?;
        Ok(path)
    }

    fn next_available_restored_id(&self, original_id: &str) -> TemplateRepositoryResult<String> {
        let suffix = "_restored";
        let base_length = 80usize.saturating_sub(suffix.len());
        let base: String = original_id.chars().take(base_length).collect();
        let first = format!("{base}{suffix}");
        if !self.any_origin_exists(&first)? {
            return Ok(first);
        }
        for index in 2..=10_000u32 {
            let suffix = format!("_restored_{index}");
            let base: String = original_id
                .chars()
                .take(80usize.saturating_sub(suffix.len()))
                .collect();
            let candidate = format!("{base}{suffix}");
            if !self.any_origin_exists(&candidate)? {
                return Ok(candidate);
            }
        }
        Err(TemplateRepositoryError::new(
            TemplateRepositoryErrorKind::AlreadyExists,
            "no available restored template ID could be generated",
        ))
    }

    fn any_origin_exists(&self, template_id: &str) -> TemplateRepositoryResult<bool> {
        validate_template_id(template_id)?;
        if path_entry_exists(&self.custom_template_path(template_id)?) {
            return Ok(true);
        }
        self.read_only_origin_exists(template_id)
    }

    fn collect_file_ids(
        &self,
        root: Option<&Path>,
        _origin: TemplateOrigin,
        ids: &mut BTreeSet<String>,
        diagnostics: &mut Vec<TemplateRepositoryDiagnostic>,
    ) -> TemplateRepositoryResult<()> {
        let Some(root) = root else {
            return Ok(());
        };
        for entry in fs::read_dir(root).map_err(map_directory_error)? {
            let entry = entry.map_err(map_io_error)?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some(TEMPLATE_EXTENSION) {
                continue;
            }
            let file_name = entry.file_name().to_string_lossy().to_string();
            let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
                diagnostics.push(path_diagnostic(file_name));
                continue;
            };
            if validate_template_id(stem).is_err() {
                diagnostics.push(path_diagnostic(file_name));
                continue;
            }
            ids.insert(stem.to_owned());
        }
        Ok(())
    }

    fn ensure_safe_directories(&self) -> TemplateRepositoryResult<()> {
        reject_reparse_point(&self.root)?;
        for child in [".trash", ".backup", ".tmp"] {
            ensure_child_directory(&self.root, child)?;
        }
        Ok(())
    }

    pub(super) fn lock_writes(&self) -> TemplateRepositoryResult<MutexGuard<'_, ()>> {
        self.write_lock.lock().map_err(|_| {
            TemplateRepositoryError::new(
                TemplateRepositoryErrorKind::Io,
                "the template repository write lock is poisoned",
            )
        })
    }
}

fn load_template_bytes(
    template_id: &str,
    origin: TemplateOrigin,
    bytes: &[u8],
) -> TemplateRepositoryResult<LoadedTemplate> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| invalid_template("the template file is not UTF-8"))?;
    let value: Value = serde_json::from_str(text)
        .map_err(|_| invalid_template("the template file is not valid JSON"))?;
    let (template, schema_version_on_disk) = match value.get("schema_version") {
        None => {
            let legacy: Template = serde_json::from_value(value)
                .map_err(|_| invalid_template("the legacy template shape is invalid"))?;
            legacy
                .validate()
                .map_err(|_| invalid_template("the legacy template is invalid"))?;
            (
                migrate_v1_to_v2(template_id, &legacy, legacy_normalization_time())
                    .map_err(|_| invalid_template("the legacy template cannot be normalized"))?,
                1,
            )
        }
        Some(version) if version.as_u64() == Some(2) => (
            parse_and_validate_template_v2(text)
                .map_err(|_| invalid_template("the v2 template is invalid"))?,
            2,
        ),
        Some(_) => {
            return Err(invalid_template(
                "the template schema version is unsupported",
            ));
        }
    };
    if template.id != template_id {
        return Err(invalid_template(
            "the template ID does not match its file name",
        ));
    }

    let file_sha256 = sha256_hex(bytes);
    let semantic_sha256 = semantic_sha256(&template)?;
    Ok(LoadedTemplate {
        record: TemplateRecord {
            template,
            origin,
            schema_version_on_disk,
            file_sha256,
            semantic_sha256,
            read_only: origin != TemplateOrigin::Custom,
            overrides_builtin: false,
        },
    })
}

pub fn templates_root_from_data_dir(data_dir: impl AsRef<Path>) -> PathBuf {
    data_dir.as_ref().join("Meetily").join("templates")
}

fn detect_schema_version(bytes: &[u8]) -> u8 {
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|value| value.get("schema_version").and_then(Value::as_u64))
        .filter(|version| *version == 2)
        .map(|_| 2)
        .unwrap_or(1)
}

fn ensure_valid_template(template: &TemplateV2) -> TemplateRepositoryResult<()> {
    let validation = validate_template_v2(template);
    if validation.valid {
        Ok(())
    } else {
        Err(invalid_template("the template failed validation"))
    }
}

pub(super) fn semantic_sha256(template: &TemplateV2) -> TemplateRepositoryResult<String> {
    let value = serde_json::to_value(template)
        .map_err(|_| invalid_template("the template cannot be normalized"))?;
    let canonical = canonicalize_json(&value);
    let bytes = serde_json::to_vec(&canonical)
        .map_err(|_| invalid_template("the canonical template cannot be serialized"))?;
    Ok(sha256_hex(&bytes))
}

fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonicalize_json).collect()),
        Value::Object(object) => {
            let mut keys: Vec<&String> = object.keys().collect();
            keys.sort();
            let mut canonical = Map::new();
            for key in keys {
                canonical.insert(key.clone(), canonicalize_json(&object[key]));
            }
            Value::Object(canonical)
        }
        other => other.clone(),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn require_expected_state(
    current: &TemplateRecord,
    expected_version: u64,
    expected_file_sha256: &str,
) -> TemplateRepositoryResult<()> {
    if current.template.version != expected_version || current.file_sha256 != expected_file_sha256 {
        return Err(conflict(
            "the template version or file hash no longer matches",
        ));
    }
    Ok(())
}

fn validate_template_id(template_id: &str) -> TemplateRepositoryResult<()> {
    let bytes = template_id.as_bytes();
    let valid_length = (3..=80).contains(&bytes.len());
    let valid_first = bytes.first().is_some_and(u8::is_ascii_alphanumeric);
    let valid_last = bytes.last().is_some_and(u8::is_ascii_alphanumeric);
    let valid_characters = bytes.iter().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_' || *byte == b'-'
    });
    if !valid_length || !valid_first || !valid_last || !valid_characters {
        return Err(TemplateRepositoryError::new(
            TemplateRepositoryErrorKind::InvalidId,
            "the template ID does not match the safe identifier policy",
        ));
    }

    let reserved = ["con", "prn", "aux", "nul", "clock$"];
    let is_numbered_reserved = (template_id.len() == 4)
        && (template_id.starts_with("com") || template_id.starts_with("lpt"))
        && matches!(template_id.as_bytes()[3], b'1'..=b'9');
    if reserved.contains(&template_id) || is_numbered_reserved {
        return Err(TemplateRepositoryError::new(
            TemplateRepositoryErrorKind::InvalidId,
            "the template ID is a reserved Windows device name",
        ));
    }
    Ok(())
}

fn validate_trash_id(trash_id: &str) -> TemplateRepositoryResult<()> {
    if trash_id.is_empty()
        || trash_id.len() > MAX_TRASH_ID_LENGTH
        || !trash_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(TemplateRepositoryError::new(
            TemplateRepositoryErrorKind::PathRejected,
            "the trash ID is unsafe",
        ));
    }
    Ok(())
}

fn make_trash_id(template_id: &str, deleted_at: DateTime<FixedOffset>, uuid: Uuid) -> String {
    format!(
        "{template_id}{TRASH_SEPARATOR}{}{TRASH_SEPARATOR}{uuid}",
        deleted_at.timestamp_millis()
    )
}

fn parse_trash_id(trash_id: &str) -> TemplateRepositoryResult<(String, DateTime<FixedOffset>)> {
    validate_trash_id(trash_id)?;
    let mut parts = trash_id.rsplitn(3, TRASH_SEPARATOR);
    let uuid = parts.next().unwrap_or_default();
    let millis = parts.next().unwrap_or_default();
    let template_id = parts.next().unwrap_or_default();
    Uuid::parse_str(uuid).map_err(|_| {
        TemplateRepositoryError::new(
            TemplateRepositoryErrorKind::PathRejected,
            "the trash ID UUID is invalid",
        )
    })?;
    validate_template_id(template_id)?;
    let millis: i64 = millis.parse().map_err(|_| {
        TemplateRepositoryError::new(
            TemplateRepositoryErrorKind::PathRejected,
            "the trash ID timestamp is invalid",
        )
    })?;
    let deleted_at = Utc
        .timestamp_millis_opt(millis)
        .single()
        .ok_or_else(|| {
            TemplateRepositoryError::new(
                TemplateRepositoryErrorKind::PathRejected,
                "the trash ID timestamp is out of range",
            )
        })?
        .fixed_offset();
    Ok((template_id.to_owned(), deleted_at))
}

fn legacy_normalization_time() -> DateTime<FixedOffset> {
    Utc.timestamp_opt(0, 0)
        .single()
        .expect("the Unix epoch is a valid timestamp")
        .fixed_offset()
}

fn ensure_repository_directory(path: &Path) -> TemplateRepositoryResult<PathBuf> {
    if path_entry_exists(path) {
        reject_reparse_point(path)?;
        if !path.is_dir() {
            return Err(TemplateRepositoryError::new(
                TemplateRepositoryErrorKind::DirectoryUnavailable,
                "the templates path is occupied by a non-directory",
            ));
        }
    } else {
        fs::create_dir_all(path).map_err(map_directory_error)?;
    }
    fs::canonicalize(path).map_err(map_directory_error)
}

fn ensure_child_directory(root: &Path, name: &str) -> TemplateRepositoryResult<PathBuf> {
    let path = root.join(name);
    if path_entry_exists(&path) {
        reject_reparse_point(&path)?;
        if !path.is_dir() {
            return Err(TemplateRepositoryError::new(
                TemplateRepositoryErrorKind::DirectoryUnavailable,
                "a repository support path is not a directory",
            ));
        }
    } else {
        fs::create_dir(&path).map_err(map_directory_error)?;
    }
    let canonical = fs::canonicalize(&path).map_err(map_directory_error)?;
    if canonical.parent() != Some(root) {
        return Err(TemplateRepositoryError::new(
            TemplateRepositoryErrorKind::PathRejected,
            "a repository support directory escaped the templates root",
        ));
    }
    Ok(canonical)
}

fn verify_writable(directory: &Path) -> TemplateRepositoryResult<()> {
    let probe_path = directory.join(format!("write-probe-{}.tmp", Uuid::new_v4()));
    let result = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe_path)
        .and_then(|file| file.sync_all());
    let _ = fs::remove_file(&probe_path);
    result.map_err(|error| {
        if is_disk_full(&error) {
            TemplateRepositoryError::new(
                TemplateRepositoryErrorKind::DiskFull,
                "the templates directory has no available space",
            )
        } else {
            TemplateRepositoryError::new(
                TemplateRepositoryErrorKind::DirectoryNotWritable,
                "the templates directory is not writable",
            )
        }
    })
}

fn safe_existing_template_path(
    root: &Path,
    template_id: &str,
) -> TemplateRepositoryResult<PathBuf> {
    validate_template_id(template_id)?;
    let path = root.join(format!("{template_id}.json"));
    if !path_entry_exists(&path) {
        return Err(not_found("the template file does not exist"));
    }
    reject_reparse_point(&path)?;
    if !path.is_file() {
        return Err(TemplateRepositoryError::new(
            TemplateRepositoryErrorKind::PathRejected,
            "the template path is not a regular file",
        ));
    }
    ensure_parent_is_root(&path, root)?;
    Ok(path)
}

fn ensure_parent_is_root(path: &Path, root: &Path) -> TemplateRepositoryResult<()> {
    let parent = path.parent().ok_or_else(|| {
        TemplateRepositoryError::new(
            TemplateRepositoryErrorKind::PathRejected,
            "the generated template path has no parent",
        )
    })?;
    if parent != root {
        return Err(TemplateRepositoryError::new(
            TemplateRepositoryErrorKind::PathRejected,
            "the generated path escaped the repository root",
        ));
    }
    Ok(())
}

fn reject_reparse_point(path: &Path) -> TemplateRepositoryResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(map_io_error)?;
    if metadata.file_type().is_symlink() {
        return Err(TemplateRepositoryError::new(
            TemplateRepositoryErrorKind::PathRejected,
            "symbolic links are not allowed in the template repository",
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(TemplateRepositoryError::new(
                TemplateRepositoryErrorKind::PathRejected,
                "Windows reparse points are not allowed in the template repository",
            ));
        }
    }
    Ok(())
}

fn path_entry_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> TemplateRepositoryResult<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(map_write_error)
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> TemplateRepositoryResult<()> {
    Ok(())
}

fn path_diagnostic(file_name: String) -> TemplateRepositoryDiagnostic {
    TemplateRepositoryDiagnostic {
        file_name,
        code: "TEMPLATE_PATH_REJECTED".to_owned(),
        message_key: "templates.errors.pathRejected".to_owned(),
    }
}

fn message_key_for_error(kind: TemplateRepositoryErrorKind) -> &'static str {
    match kind {
        TemplateRepositoryErrorKind::InvalidId => "templates.errors.invalidId",
        TemplateRepositoryErrorKind::PathRejected => "templates.errors.pathRejected",
        TemplateRepositoryErrorKind::DirectoryUnavailable => {
            "templates.errors.directoryUnavailable"
        }
        TemplateRepositoryErrorKind::DirectoryNotWritable => {
            "templates.errors.directoryNotWritable"
        }
        TemplateRepositoryErrorKind::NotFound => "templates.errors.notFound",
        TemplateRepositoryErrorKind::AlreadyExists => "templates.errors.alreadyExists",
        TemplateRepositoryErrorKind::Conflict => "templates.errors.conflict",
        TemplateRepositoryErrorKind::ReadOnly => "templates.errors.readOnly",
        TemplateRepositoryErrorKind::InvalidTemplate => "templates.errors.invalid",
        TemplateRepositoryErrorKind::DiskFull => "templates.errors.diskFull",
        TemplateRepositoryErrorKind::Io => "templates.errors.io",
    }
}

fn not_found(detail: impl Into<String>) -> TemplateRepositoryError {
    TemplateRepositoryError::new(TemplateRepositoryErrorKind::NotFound, detail)
}

fn invalid_template(detail: impl Into<String>) -> TemplateRepositoryError {
    TemplateRepositoryError::new(TemplateRepositoryErrorKind::InvalidTemplate, detail)
}

fn conflict(detail: impl Into<String>) -> TemplateRepositoryError {
    TemplateRepositoryError::new(TemplateRepositoryErrorKind::Conflict, detail)
}

fn map_directory_error(error: std::io::Error) -> TemplateRepositoryError {
    if is_disk_full(&error) {
        TemplateRepositoryError::new(
            TemplateRepositoryErrorKind::DiskFull,
            "a repository directory operation failed because the disk is full",
        )
    } else if error.kind() == std::io::ErrorKind::PermissionDenied {
        TemplateRepositoryError::new(
            TemplateRepositoryErrorKind::DirectoryNotWritable,
            "a repository directory operation was denied",
        )
    } else {
        TemplateRepositoryError::new(
            TemplateRepositoryErrorKind::DirectoryUnavailable,
            "a repository directory operation failed",
        )
    }
}

fn map_write_error(error: std::io::Error) -> TemplateRepositoryError {
    if is_disk_full(&error) {
        TemplateRepositoryError::new(
            TemplateRepositoryErrorKind::DiskFull,
            "a template write failed because the disk is full",
        )
    } else if error.kind() == std::io::ErrorKind::PermissionDenied {
        TemplateRepositoryError::new(
            TemplateRepositoryErrorKind::DirectoryNotWritable,
            "a template write was denied",
        )
    } else {
        TemplateRepositoryError::new(
            TemplateRepositoryErrorKind::Io,
            "a template write operation failed",
        )
    }
}

fn map_io_error(error: std::io::Error) -> TemplateRepositoryError {
    if is_disk_full(&error) {
        TemplateRepositoryError::new(
            TemplateRepositoryErrorKind::DiskFull,
            "a template I/O operation failed because the disk is full",
        )
    } else {
        TemplateRepositoryError::new(
            TemplateRepositoryErrorKind::Io,
            "a template I/O operation failed",
        )
    }
}

fn is_disk_full(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(28) | Some(112))
}

#[cfg(test)]
mod tests {
    use super::super::v2::{EmptyBehavior, TemplateFormat, TemplateSectionV2};
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    fn at(second: u32) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(&format!("2026-08-23T10:00:{second:02}+08:00")).unwrap()
    }

    fn template(id: &str, name: &str) -> TemplateV2 {
        TemplateV2 {
            schema_version: 2,
            id: id.to_owned(),
            name: name.to_owned(),
            description: "Repository test template".to_owned(),
            version: 999,
            locale: Some("zh-CN".to_owned()),
            tags: vec!["test".to_owned()],
            source: TemplateSource {
                source_type: TemplateSourceType::Manual,
                original_file_name: None,
                original_file_sha256: None,
                imported_at: None,
                copied_from_template_id: None,
            },
            created_at: at(0),
            updated_at: at(0),
            sections: vec![TemplateSectionV2 {
                id: "summary".to_owned(),
                title: "会议摘要".to_owned(),
                instruction: "总结会议内容".to_owned(),
                format: TemplateFormat::Paragraph,
                item_format: None,
                example_item_format: None,
                required: true,
                empty_behavior: EmptyBehavior::ShowNotMentioned,
            }],
            extensions: Map::new(),
        }
    }

    fn repository() -> (TempDir, TemplateRepository) {
        let temporary = TempDir::new().unwrap();
        let repository = TemplateRepository::new(temporary.path().join("templates"), None).unwrap();
        (temporary, repository)
    }

    #[test]
    fn creates_repository_support_directories_without_touching_appdata() {
        let (temporary, repository) = repository();
        assert_eq!(
            repository.root(),
            fs::canonicalize(temporary.path().join("templates")).unwrap()
        );
        assert!(repository.trash_root().is_dir());
        assert!(repository.backup_root().is_dir());
        assert!(repository.root().join(".tmp").is_dir());
    }

    #[test]
    fn data_directory_mapping_is_pure_and_platform_independent() {
        let data_dir = PathBuf::from("C:/Users/liuxin/AppData/Roaming");
        assert_eq!(
            templates_root_from_data_dir(&data_dir),
            data_dir.join("Meetily").join("templates")
        );
    }

    #[test]
    fn stale_temp_cleanup_only_removes_tmp_files() {
        let (_temporary, repository) = repository();
        let temporary_root = repository.root().join(".tmp");
        fs::write(temporary_root.join("abandoned.tmp"), b"pending").unwrap();
        fs::write(temporary_root.join("keep.txt"), b"keep").unwrap();
        let removed = repository.cleanup_stale_temp_files(Duration::ZERO).unwrap();
        assert_eq!(removed, 1);
        assert!(!temporary_root.join("abandoned.tmp").exists());
        assert!(temporary_root.join("keep.txt").exists());
    }

    #[test]
    fn rejects_non_directory_root() {
        let temporary = TempDir::new().unwrap();
        let occupied = temporary.path().join("templates");
        fs::write(&occupied, b"not a directory").unwrap();
        let error = TemplateRepository::new(&occupied, None).err().unwrap();
        assert_eq!(
            error.kind,
            TemplateRepositoryErrorKind::DirectoryUnavailable
        );
    }

    #[test]
    fn rejects_malicious_and_reserved_ids_without_writing_files() {
        let (_temporary, repository) = repository();
        let malicious = [
            "../escape",
            "..\\escape",
            "C:drive",
            "//server",
            "NUL",
            "nul",
            "con",
            "com1",
            "lpt9",
            "ab",
            "ends_",
            "中文",
            "a\0b",
        ];
        for id in malicious {
            let mut candidate = template("safe_template", "Safe");
            candidate.id = id.to_owned();
            let error = repository
                .create_at(candidate, CreateConflictPolicy::Error, at(0))
                .unwrap_err();
            assert_eq!(error.kind, TemplateRepositoryErrorKind::InvalidId, "{id}");
        }
        assert_eq!(fs::read_dir(repository.root()).unwrap().count(), 3);
    }

    #[cfg(windows)]
    #[test]
    fn existing_template_reparse_point_is_rejected_when_supported_by_host() {
        use std::os::windows::fs::symlink_file;

        let (temporary, repository) = repository();
        let outside = temporary.path().join("outside.json");
        fs::write(
            &outside,
            serde_json::to_vec(&template("linked_template", "Outside")).unwrap(),
        )
        .unwrap();
        let link = repository.root().join("linked_template.json");
        match symlink_file(&outside, &link) {
            Ok(()) => {
                let error = repository.get("linked_template", None).unwrap_err();
                assert_eq!(error.kind, TemplateRepositoryErrorKind::PathRejected);
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::PermissionDenied
                    || error.raw_os_error() == Some(1314) =>
            {
                eprintln!("symlink test skipped: host has no symlink privilege");
            }
            Err(error) => panic!("unexpected symlink creation failure: {error}"),
        }
    }

    #[test]
    fn create_get_and_hashes_are_stable() {
        let (_temporary, repository) = repository();
        let created = repository
            .create_at(
                template("customer_review", "客户评审"),
                CreateConflictPolicy::Error,
                at(1),
            )
            .unwrap();
        assert_eq!(created.template.version, 1);
        assert_eq!(created.template.created_at, at(1));
        assert_eq!(created.file_sha256.len(), 64);
        assert_eq!(created.semantic_sha256.len(), 64);
        let loaded = repository.get("customer_review", None).unwrap();
        assert_eq!(created, loaded);

        let path = repository.root().join("customer_review.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let root = value.as_object_mut().unwrap();
        root.remove("extensions");
        let source = root["source"].as_object_mut().unwrap();
        source.remove("original_file_name");
        source.remove("original_file_sha256");
        source.remove("imported_at");
        source.remove("copied_from_template_id");
        let section = root["sections"][0].as_object_mut().unwrap();
        section.remove("item_format");
        section.remove("example_item_format");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let reformatted = repository.get("customer_review", None).unwrap();
        assert_eq!(reformatted.template, created.template);
        assert_ne!(reformatted.file_sha256, created.file_sha256);
        assert_eq!(reformatted.semantic_sha256, created.semantic_sha256);
    }

    #[test]
    fn explicit_policy_is_required_to_override_a_builtin() {
        let (_temporary, repository) = repository();
        let error = repository
            .create_at(
                template("daily_standup", "自定义站会"),
                CreateConflictPolicy::Error,
                at(1),
            )
            .unwrap_err();
        assert_eq!(error.kind, TemplateRepositoryErrorKind::AlreadyExists);

        let created = repository
            .create_at(
                template("daily_standup", "自定义站会"),
                CreateConflictPolicy::OverrideReadOnly,
                at(1),
            )
            .unwrap();
        assert!(created.overrides_builtin);
        assert_eq!(
            repository.get("daily_standup", None).unwrap().origin,
            TemplateOrigin::Custom
        );
    }

    #[test]
    fn custom_overrides_localized_builtin_and_legacy_bundle_with_stable_list_order() {
        let temporary = TempDir::new().unwrap();
        let bundled = temporary.path().join("bundled");
        fs::create_dir(&bundled).unwrap();
        fs::write(
            bundled.join("daily_standup.json"),
            defaults::get_builtin_template("daily_standup")
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        fs::write(
            bundled.join("project_sync.json"),
            include_bytes!("../../../templates/project_sync.json"),
        )
        .unwrap();
        let repository =
            TemplateRepository::new(temporary.path().join("custom"), Some(bundled)).unwrap();
        let english = repository
            .get_for_content_locale("daily_standup", None, Some("en"))
            .unwrap();
        let chinese = repository
            .get_for_content_locale("daily_standup", None, Some("zh-CN"))
            .unwrap();
        assert_eq!(english.origin, TemplateOrigin::Builtin);
        assert_eq!(english.template.locale.as_deref(), Some("en"));
        assert_eq!(chinese.origin, TemplateOrigin::Builtin);
        assert_eq!(chinese.template.locale.as_deref(), Some("zh-CN"));
        assert_ne!(english.template.name, chinese.template.name);
        repository
            .create_at(
                template("daily_standup", "AA Custom"),
                CreateConflictPolicy::OverrideReadOnly,
                at(1),
            )
            .unwrap();
        let first = repository.list().unwrap();
        let second = repository.list().unwrap();
        assert_eq!(first, second);
        let item = first
            .templates
            .iter()
            .find(|item| item.id == "daily_standup")
            .unwrap();
        assert_eq!(item.origin, TemplateOrigin::Custom);
        assert!(item.overrides_builtin);
    }

    #[test]
    fn invalid_custom_shadows_fallback_but_does_not_break_list() {
        let (_temporary, repository) = repository();
        repository.list_for_content_locale(Some("en")).unwrap();
        fs::write(repository.root().join("daily_standup.json"), b"{broken").unwrap();
        let list = repository.list().unwrap();
        let invalid = list
            .templates
            .iter()
            .find(|item| item.id == "daily_standup")
            .unwrap();
        assert_eq!(invalid.origin, TemplateOrigin::Custom);
        assert!(!invalid.valid);
        assert!(list
            .templates
            .iter()
            .any(|item| item.id == "standard_meeting" && item.valid));
        assert!(list
            .diagnostics
            .iter()
            .any(|item| item.file_name == "daily_standup.json"));
        assert_eq!(
            repository.get("daily_standup", None).unwrap_err().kind,
            TemplateRepositoryErrorKind::InvalidTemplate
        );
    }

    #[test]
    fn update_increments_version_creates_backup_and_detects_conflicts() {
        let (_temporary, repository) = repository();
        let created = repository
            .create_at(
                template("weekly_review", "周会"),
                CreateConflictPolicy::Error,
                at(1),
            )
            .unwrap();
        let original_bytes = fs::read(repository.root().join("weekly_review.json")).unwrap();
        let mut replacement = created.template.clone();
        replacement.name = "周会 2".to_owned();
        replacement.version = 88;
        replacement.created_at = at(9);
        let updated = repository
            .update_at(
                "weekly_review",
                created.template.version,
                &created.file_sha256,
                replacement,
                at(2),
            )
            .unwrap();
        assert_eq!(updated.template.version, 2);
        assert_eq!(updated.template.created_at, at(1));
        assert_eq!(updated.template.updated_at, at(2));
        let backups: Vec<_> = fs::read_dir(repository.backup_root())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read(&backups[0]).unwrap(), original_bytes);
        assert_ne!(
            fs::read(&backups[0]).unwrap(),
            fs::read(repository.root().join("weekly_review.json")).unwrap()
        );

        let mut stale = updated.template.clone();
        stale.name = "stale".to_owned();
        let error = repository
            .update_at("weekly_review", 1, &created.file_sha256, stale, at(3))
            .unwrap_err();
        assert_eq!(error.kind, TemplateRepositoryErrorKind::Conflict);
        assert_eq!(repository.get("weekly_review", None).unwrap(), updated);
    }

    #[test]
    fn editing_a_v1_file_migrates_it_to_v2_version_one() {
        let (_temporary, repository) = repository();
        fs::write(
            repository.root().join("legacy_custom.json"),
            include_bytes!("../../../templates/standard_meeting.json"),
        )
        .unwrap();
        let loaded = repository.get("legacy_custom", None).unwrap();
        assert_eq!(loaded.schema_version_on_disk, 1);
        let mut replacement = loaded.template.clone();
        replacement.name = "Migrated".to_owned();
        let updated = repository
            .update_at("legacy_custom", 1, &loaded.file_sha256, replacement, at(2))
            .unwrap();
        assert_eq!(updated.schema_version_on_disk, 2);
        assert_eq!(updated.template.version, 1);
        assert_eq!(updated.template.created_at, at(2));
        assert_eq!(updated.template.updated_at, at(2));
        let value: Value = serde_json::from_slice(
            &fs::read(repository.root().join("legacy_custom.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(value["schema_version"], 2);
    }

    #[test]
    fn external_modification_causes_hash_conflict() {
        let (_temporary, repository) = repository();
        let created = repository
            .create_at(
                template("external_edit", "Original"),
                CreateConflictPolicy::Error,
                at(1),
            )
            .unwrap();
        let path = repository.root().join("external_edit.json");
        let mut external = created.template.clone();
        external.name = "External".to_owned();
        fs::write(&path, serde_json::to_vec_pretty(&external).unwrap()).unwrap();
        let mut replacement = created.template.clone();
        replacement.name = "Editor".to_owned();
        let error = repository
            .update_at("external_edit", 1, &created.file_sha256, replacement, at(2))
            .unwrap_err();
        assert_eq!(error.kind, TemplateRepositoryErrorKind::Conflict);
        assert_eq!(
            repository.get("external_edit", None).unwrap().template.name,
            "External"
        );
    }

    #[test]
    fn create_and_delete_conflicts_never_overwrite_or_remove_current_file() {
        let (_temporary, repository) = repository();
        let created = repository
            .create_at(
                template("collision_test", "First"),
                CreateConflictPolicy::Error,
                at(1),
            )
            .unwrap();
        let duplicate_error = repository
            .create_at(
                template("collision_test", "Second"),
                CreateConflictPolicy::OverrideReadOnly,
                at(2),
            )
            .unwrap_err();
        assert_eq!(
            duplicate_error.kind,
            TemplateRepositoryErrorKind::AlreadyExists
        );
        assert_eq!(repository.get("collision_test", None).unwrap(), created);

        let delete_error = repository
            .delete_at("collision_test", &"0".repeat(64), at(3))
            .unwrap_err();
        assert_eq!(delete_error.kind, TemplateRepositoryErrorKind::Conflict);
        assert_eq!(repository.get("collision_test", None).unwrap(), created);
        assert!(repository.list_deleted().unwrap().is_empty());

        let path_error = repository.purge("../../collision_test").unwrap_err();
        assert_eq!(path_error.kind, TemplateRepositoryErrorKind::PathRejected);
        assert_eq!(repository.get("collision_test", None).unwrap(), created);
    }

    #[test]
    fn filename_and_embedded_v2_id_must_match() {
        let (_temporary, repository) = repository();
        fs::write(
            repository.root().join("file_name_id.json"),
            serde_json::to_vec(&template("different_id", "Mismatch")).unwrap(),
        )
        .unwrap();
        let error = repository.get("file_name_id", None).unwrap_err();
        assert_eq!(error.kind, TemplateRepositoryErrorKind::InvalidTemplate);
        let list = repository.list().unwrap();
        let item = list
            .templates
            .iter()
            .find(|item| item.id == "file_name_id")
            .unwrap();
        assert!(!item.valid);
    }

    #[cfg(windows)]
    #[test]
    fn readonly_replace_failure_preserves_original_and_cleans_pending_file() {
        let (_temporary, repository) = repository();
        let created = repository
            .create_at(
                template("readonly_edit", "Original"),
                CreateConflictPolicy::Error,
                at(1),
            )
            .unwrap();
        let path = repository.root().join("readonly_edit.json");
        let original_bytes = fs::read(&path).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).unwrap();

        let mut replacement = created.template.clone();
        replacement.name = "Replacement".to_owned();
        let result = repository.update_at(
            "readonly_edit",
            created.template.version,
            &created.file_sha256,
            replacement,
            at(2),
        );

        let mut cleanup_permissions = fs::metadata(&path).unwrap().permissions();
        cleanup_permissions.set_readonly(false);
        fs::set_permissions(&path, cleanup_permissions).unwrap();
        let error = result.unwrap_err();
        assert!(matches!(
            error.kind,
            TemplateRepositoryErrorKind::DirectoryNotWritable | TemplateRepositoryErrorKind::Io
        ));
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
        assert_eq!(
            fs::read_dir(repository.root().join(".tmp"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn concurrent_updates_allow_only_one_winner() {
        let (_temporary, repository) = repository();
        let repository = Arc::new(repository);
        let created = repository
            .create_at(
                template("concurrent_edit", "Initial"),
                CreateConflictPolicy::Error,
                at(1),
            )
            .unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();
        for name in ["Editor A", "Editor B"] {
            let repository = Arc::clone(&repository);
            let barrier = Arc::clone(&barrier);
            let mut replacement = created.template.clone();
            replacement.name = name.to_owned();
            let expected_hash = created.file_sha256.clone();
            handles.push(thread::spawn(move || {
                barrier.wait();
                repository.update_at("concurrent_edit", 1, &expected_hash, replacement, at(2))
            }));
        }
        barrier.wait();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| {
                    matches!(
                        result,
                        Err(error)
                            if error.kind == TemplateRepositoryErrorKind::Conflict
                    )
                })
                .count(),
            1
        );
    }

    #[test]
    fn observers_never_see_missing_or_partial_json_during_replacements() {
        let (_temporary, repository) = repository();
        let repository = Arc::new(repository);
        let mut current = repository
            .create_at(
                template("observed_updates", "Version 0"),
                CreateConflictPolicy::Error,
                at(1),
            )
            .unwrap();
        let target = repository.root().join("observed_updates.json");
        let stop = Arc::new(AtomicBool::new(false));
        let observer_stop = Arc::clone(&stop);
        let observer = thread::spawn(move || {
            let mut failures = Vec::new();
            while !observer_stop.load(Ordering::Relaxed) {
                match fs::read(&target) {
                    Ok(bytes) => {
                        if serde_json::from_slice::<Value>(&bytes).is_err() {
                            failures.push("partial JSON".to_owned());
                        }
                    }
                    Err(error) => failures.push(format!("read failed: {error}")),
                }
            }
            failures
        });

        for index in 1..=50u32 {
            let mut replacement = current.template.clone();
            replacement.name = format!("Version {index}");
            current = repository
                .update_at(
                    "observed_updates",
                    current.template.version,
                    &current.file_sha256,
                    replacement,
                    at(2),
                )
                .unwrap();
        }
        stop.store(true, Ordering::Relaxed);
        let failures = observer.join().unwrap();
        assert!(failures.is_empty(), "observer failures: {failures:?}");
    }

    #[test]
    fn delete_restore_and_purge_preserve_content_and_hash() {
        let (_temporary, repository) = repository();
        let created = repository
            .create_at(
                template("trash_test", "Trash"),
                CreateConflictPolicy::Error,
                at(1),
            )
            .unwrap();
        let deleted = repository
            .delete_at("trash_test", &created.file_sha256, at(2))
            .unwrap();
        assert_eq!(
            repository
                .get("trash_test", Some(TemplateOrigin::Custom))
                .unwrap_err()
                .kind,
            TemplateRepositoryErrorKind::NotFound
        );
        let trash = repository.list_deleted().unwrap();
        assert_eq!(trash.len(), 1);
        assert_eq!(trash[0].trash_id, deleted.trash_id);
        assert_eq!(trash[0].file_sha256, created.file_sha256);
        let restored = repository
            .restore_at(&deleted.trash_id, RestoreConflictPolicy::Error, at(3))
            .unwrap();
        assert_eq!(restored.file_sha256, created.file_sha256);

        let deleted_again = repository
            .delete_at("trash_test", &restored.file_sha256, at(4))
            .unwrap();
        repository.purge(&deleted_again.trash_id).unwrap();
        assert!(repository.list_deleted().unwrap().is_empty());
    }

    #[test]
    fn restore_conflict_supports_error_keep_both_and_replace() {
        let (_temporary, repository) = repository();
        let original = repository
            .create_at(
                template("restore_conflict", "Deleted"),
                CreateConflictPolicy::Error,
                at(1),
            )
            .unwrap();
        let deleted = repository
            .delete_at("restore_conflict", &original.file_sha256, at(2))
            .unwrap();
        let current = repository
            .create_at(
                template("restore_conflict", "Current"),
                CreateConflictPolicy::Error,
                at(3),
            )
            .unwrap();
        assert_eq!(
            repository
                .restore_at(&deleted.trash_id, RestoreConflictPolicy::Error, at(4))
                .unwrap_err()
                .kind,
            TemplateRepositoryErrorKind::AlreadyExists
        );
        let kept = repository
            .restore_at(&deleted.trash_id, RestoreConflictPolicy::KeepBoth, at(4))
            .unwrap();
        assert!(kept.template.id.starts_with("restore_conflict_restored"));
        assert_eq!(repository.get("restore_conflict", None).unwrap(), current);

        let kept_deleted = repository
            .delete_at(&kept.template.id, &kept.file_sha256, at(5))
            .unwrap();
        repository
            .create_at(
                template(&kept.template.id, "Current replacement"),
                CreateConflictPolicy::Error,
                at(5),
            )
            .unwrap();
        let replaced = repository
            .restore_at(
                &kept_deleted.trash_id,
                RestoreConflictPolicy::ReplaceCustom,
                at(6),
            )
            .unwrap();
        assert_eq!(replaced.template.id, kept.template.id);
        assert!(repository
            .list_deleted()
            .unwrap()
            .iter()
            .any(|item| item.original_template_id == kept.template.id));
    }

    #[test]
    fn read_only_templates_cannot_be_updated_or_deleted() {
        let (_temporary, repository) = repository();
        let builtin = repository.get("daily_standup", None).unwrap();
        let error = repository
            .update_at(
                "daily_standup",
                builtin.template.version,
                &builtin.file_sha256,
                builtin.template.clone(),
                at(2),
            )
            .unwrap_err();
        assert_eq!(error.kind, TemplateRepositoryErrorKind::ReadOnly);
        let error = repository
            .delete_at("daily_standup", &builtin.file_sha256, at(2))
            .unwrap_err();
        assert_eq!(error.kind, TemplateRepositoryErrorKind::ReadOnly);
    }

    #[test]
    fn localized_builtin_resolution_preserves_stable_id_and_structure() {
        let (_temporary, repository) = repository();
        let en = repository
            .get_for_content_locale("project_sync", None, Some("en"))
            .unwrap();
        let zh = repository
            .get_for_content_locale("project_sync", None, Some("zh-CN"))
            .unwrap();
        assert_eq!(en.template.id, zh.template.id);
        assert_eq!(en.template.version, zh.template.version);
        assert_eq!(en.template.locale.as_deref(), Some("en"));
        assert_eq!(zh.template.locale.as_deref(), Some("zh-CN"));
        assert_eq!(en.template.sections.len(), zh.template.sections.len());
        assert_ne!(en.file_sha256, zh.file_sha256);
    }

    #[test]
    fn locale_resolution_never_overwrites_or_substitutes_custom_template_content() {
        let (_temporary, repository) = repository();
        let mut custom = template("daily_standup", "My protected standup");
        custom.locale = Some("en".to_owned());
        custom.description = "User-owned content must survive locale changes".to_owned();
        let created = repository
            .create_at(custom, CreateConflictPolicy::OverrideReadOnly, at(1))
            .unwrap();
        let resolved = repository
            .get_for_content_locale("daily_standup", None, Some("zh-CN"))
            .unwrap();
        assert_eq!(resolved.origin, TemplateOrigin::Custom);
        assert_eq!(resolved.template.name, "My protected standup");
        assert_eq!(resolved.template.locale.as_deref(), Some("en"));
        assert_eq!(resolved.file_sha256, created.file_sha256);
        assert_eq!(
            fs::read(repository.root().join("daily_standup.json")).unwrap(),
            repository
                .read_bytes_for_diagnostics("daily_standup", Some(TemplateOrigin::Custom))
                .unwrap()
        );
    }

    #[test]
    fn list_isolates_unsafe_file_names_and_scales_to_one_thousand_templates() {
        let (_temporary, repository) = repository();
        fs::write(repository.root().join("BAD NAME.json"), b"{}").unwrap();
        let source = serde_json::to_vec(&template("bulk_0000", "Bulk")).unwrap();
        for index in 0..1_000u32 {
            let id = format!("bulk_{index:04}");
            let mut value: Value = serde_json::from_slice(&source).unwrap();
            value["id"] = Value::String(id.clone());
            value["name"] = Value::String(format!("Bulk {index:04}"));
            fs::write(
                repository.root().join(format!("{id}.json")),
                serde_json::to_vec(&value).unwrap(),
            )
            .unwrap();
        }
        let mut durations = Vec::new();
        let mut list = None;
        for _ in 0..20 {
            let started = Instant::now();
            list = Some(repository.list().unwrap());
            durations.push(started.elapsed());
        }
        durations.sort();
        let p95 = durations[18];
        let list = list.unwrap();
        assert_eq!(
            list.templates
                .iter()
                .filter(|item| item.id.starts_with("bulk_"))
                .count(),
            1_000
        );
        assert!(list
            .diagnostics
            .iter()
            .any(|item| item.file_name == "BAD NAME.json"));
        println!("repository 1000-template list P95: {p95:?}");
        // The product acceptance budget applies to optimized application code.
        // Debug builds retain the functional and sample-size checks above, but
        // their instrumentation and cold-cache variance are not a release metric.
        if !cfg!(debug_assertions) {
            let budget = Duration::from_millis(500);
            assert!(p95 <= budget, "P95 listing took {p95:?}; budget={budget:?}");
        }
    }
}
