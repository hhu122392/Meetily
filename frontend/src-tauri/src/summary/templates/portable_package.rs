use super::repository::semantic_sha256;
use super::{
    parse_and_validate_template_v2, validate_template_v2, TemplateApiError, TemplateOrigin,
    TemplateService, TemplateSource, TemplateSourceType, TemplateV2,
};
use chrono::{Duration as ChronoDuration, SecondsFormat, Utc};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub const PORTABLE_PACK_EXTENSION: &str = "meetily-template-pack";
pub const MAX_ARCHIVE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
pub const MAX_TEMPLATE_BYTES: usize = 1024 * 1024;
pub const MAX_TOTAL_UNCOMPRESSED_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_TEMPLATES: usize = 200;
pub const MAX_ARCHIVE_ENTRIES: usize = MAX_TEMPLATES + 1;
pub const MAX_COMPRESSION_RATIO: u64 = 100;

const EXPORT_PLAN_TTL: Duration = Duration::from_secs(15 * 60);
const MAX_EXPORT_PLANS: usize = 128;
const IMPORT_TRANSACTION_PREFIX: &str = "portable-import-";
const IMPORT_TRANSACTION_JOURNAL: &str = "journal.json";
const IMPORT_TRANSACTION_COMMITTING: &str = "COMMITTING";
const IMPORT_TRANSACTION_COMMITTED: &str = "COMMITTED";
const PORTABLE_PACK_MANIFEST_SCHEMA_SOURCE: &str =
    include_str!("../../../schemas/portable-template-package-manifest-v1.schema.json");

static EXPORT_PLANS: Lazy<Mutex<HashMap<String, PortablePackExportPlan>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static IMPORT_PLANS: Lazy<Mutex<HashMap<String, PortablePackImportPlan>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static IMPORT_PLAN_TOMBSTONES: Lazy<Mutex<HashMap<String, PortablePackImportPlanTombstone>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static IMPORT_EXECUTION_PLANS: Lazy<Mutex<HashMap<String, PortablePackExecutionPlan>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static IMPORT_EXECUTION_PLAN_TOMBSTONES: Lazy<
    Mutex<HashMap<String, PortablePackImportPlanTombstone>>,
> = Lazy::new(|| Mutex::new(HashMap::new()));
static ACTIVE_IMPORT_EXECUTIONS: Lazy<Mutex<HashMap<String, Arc<AtomicBool>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static PORTABLE_PACK_MANIFEST_VALIDATOR: Lazy<Result<jsonschema::Validator, String>> =
    Lazy::new(|| {
        let schema: Value = serde_json::from_str(PORTABLE_PACK_MANIFEST_SCHEMA_SOURCE)
            .map_err(|error| format!("portable package schema is invalid JSON: {error}"))?;
        jsonschema::draft202012::options()
            .should_validate_formats(true)
            .build(&schema)
            .map_err(|error| format!("portable package schema failed to compile: {error}"))
    });
static BEARER_SECRET: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\bbearer\s+[a-z0-9._~+/=-]{16,}\b").expect("valid bearer-token regex")
});
static OPENAI_STYLE_SECRET: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\bsk-[a-z0-9_-]{16,}\b").expect("valid secret-token regex"));
static ASSIGNED_SECRET: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:api[_ -]?key|access[_ -]?token|client[_ -]?secret)\s*[:=]\s*[^\s,;]{8,}")
        .expect("valid assigned-secret regex")
});
static WINDOWS_ABSOLUTE_PATH: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)(?:^|[\s\"'(])(?:[a-z]:[\\/]|\\\\[^\s\\/]+[\\/])"#)
        .expect("valid Windows absolute-path regex")
});
static POSIX_ABSOLUTE_PATH: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?:^|[\s\"'(])/(?:Users|home|tmp|var|etc|opt|Volumes)/"#)
        .expect("valid POSIX absolute-path regex")
});

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewTemplatePackExportRequest {
    pub template_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExportTemplatePackRequest {
    pub plan_token: String,
    pub destination_path: String,
    pub overwrite: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewTemplatePackImportRequest {
    pub source_path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanTemplatePackImportRequest {
    pub preview_plan_token: String,
    pub decisions: Vec<PortablePackImportDecision>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecuteTemplatePackImportRequest {
    pub execution_plan_token: String,
    pub execution_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CancelTemplatePackImportRequest {
    pub execution_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortablePackTemplateFingerprint {
    pub id: String,
    pub version: u64,
    pub file_sha256: String,
    pub semantic_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortablePackExportTemplate {
    #[serde(flatten)]
    pub fingerprint: PortablePackTemplateFingerprint,
    pub name: String,
    pub byte_size: usize,
    pub overrides_builtin: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortablePackWarning {
    pub code: String,
    pub message_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewTemplatePackExportResponse {
    pub plan_token: String,
    pub templates: Vec<PortablePackExportTemplate>,
    pub template_count: usize,
    pub estimated_uncompressed_bytes: usize,
    pub warnings: Vec<PortablePackWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortablePackAuditSummary {
    pub package_id: String,
    pub package_schema_version: u8,
    pub package_file_name: String,
    pub archive_sha256: String,
    pub template_count: usize,
    pub created_at: String,
    pub application_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportTemplatePackResponse {
    pub package: PortablePackAuditSummary,
    pub byte_size: usize,
    pub exported_templates: Vec<PortablePackTemplateFingerprint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortablePackConflictKind {
    None,
    Custom,
    Readonly,
    DuplicateInPackage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortablePackConflictStrategy {
    Skip,
    KeepBoth,
    ReplaceCustom,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortablePackImportDecision {
    pub item_id: String,
    pub strategy: PortablePackConflictStrategy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortablePackImportOperationKind {
    Create,
    Skip,
    KeepBoth,
    ReplaceCustom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortablePackImportTemplate {
    #[serde(flatten)]
    pub fingerprint: PortablePackTemplateFingerprint,
    pub name: String,
    pub byte_size: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortablePackImportItem {
    pub item_id: String,
    pub template: PortablePackImportTemplate,
    pub conflict_kind: PortablePackConflictKind,
    pub allowed_strategies: Vec<PortablePackConflictStrategy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub existing: Option<PortablePackTemplateFingerprint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewTemplatePackImportResponse {
    pub plan_token: String,
    pub package: PortablePackAuditSummary,
    pub items: Vec<PortablePackImportItem>,
    pub total_uncompressed_bytes: usize,
    pub warnings: Vec<PortablePackWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortablePackPlannedImportOperation {
    pub item_id: String,
    pub operation: PortablePackImportOperationKind,
    pub source: PortablePackImportTemplate,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<PortablePackImportTemplate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_existing: Option<PortablePackTemplateFingerprint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortablePackImportPlanSummary {
    pub create_count: usize,
    pub replace_count: usize,
    pub skip_count: usize,
    pub transformed_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanTemplatePackImportResponse {
    pub execution_plan_token: String,
    pub expires_at: String,
    pub package: PortablePackAuditSummary,
    pub operations: Vec<PortablePackPlannedImportOperation>,
    pub summary: PortablePackImportPlanSummary,
    pub warnings: Vec<PortablePackWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortablePackImportExecutionItem {
    pub item_id: String,
    pub operation: PortablePackImportOperationKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<PortablePackImportTemplate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortablePackRecoverySummary {
    pub rolled_back_transactions: usize,
    pub finalized_transactions: usize,
    pub cleaned_staging_directories: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteTemplatePackImportResponse {
    pub execution_id: String,
    pub package: PortablePackAuditSummary,
    pub summary: PortablePackImportPlanSummary,
    pub results: Vec<PortablePackImportExecutionItem>,
    pub recovery: PortablePackRecoverySummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelTemplatePackImportResponse {
    pub execution_id: String,
    pub cancellation_requested: bool,
}

#[derive(Debug, Clone)]
struct PortablePackExportPlan {
    created: Instant,
    templates: Vec<PortablePackExportTemplate>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct PortablePackImportPlan {
    created: Instant,
    source_path: PathBuf,
    archive_sha256: String,
    package: PortablePackAuditSummary,
    templates: Vec<PortablePackImportTemplate>,
    items: Vec<PortablePackImportItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortablePackImportPlanTombstoneKind {
    Consumed,
    Expired,
}

#[derive(Debug, Clone)]
struct PortablePackImportPlanTombstone {
    created: Instant,
    kind: PortablePackImportPlanTombstoneKind,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct PortablePackExecutionPlan {
    created: Instant,
    planned_at: chrono::DateTime<chrono::FixedOffset>,
    source_path: PathBuf,
    archive_sha256: String,
    package: PortablePackAuditSummary,
    operations: Vec<PortablePackPlannedImportOperation>,
}

#[derive(Debug, Clone)]
struct MaterializedImportOperation {
    item_id: String,
    operation: PortablePackImportOperationKind,
    target: PortablePackImportTemplate,
    bytes: Vec<u8>,
    expected_existing: Option<PortablePackTemplateFingerprint>,
}

#[derive(Debug, Clone, Copy, Default)]
struct ImportTransactionInterrupt {
    cancel_after_applied: Option<usize>,
    fail_after_applied: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ImportTransactionOperationKind {
    Create,
    Replace,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImportTransactionOperation {
    item_id: String,
    operation: ImportTransactionOperationKind,
    target_file_name: String,
    staged_file_name: String,
    backup_file_name: Option<String>,
    new_sha256: String,
    original_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImportTransactionJournal {
    schema_version: u8,
    execution_id: String,
    operations: Vec<ImportTransactionOperation>,
}

struct PendingImportTransactionDirectory {
    repository_root: PathBuf,
    transaction_root: PathBuf,
    armed: bool,
}

impl PendingImportTransactionDirectory {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PendingImportTransactionDirectory {
    fn drop(&mut self) {
        if self.armed {
            let _ =
                remove_import_transaction_directory(&self.repository_root, &self.transaction_root);
        }
    }
}

#[derive(Debug, Clone)]
struct ResolvedExportTemplate {
    preview: PortablePackExportTemplate,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct ResolvedImportTemplate {
    preview: PortablePackImportTemplate,
    template: TemplateV2,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PortablePackManifest {
    package_schema_version: u8,
    package_type: String,
    package_id: String,
    created_at: String,
    created_by: PortablePackCreatedBy,
    hash_algorithm: String,
    semantic_hash_algorithm: String,
    template_count: usize,
    templates: Vec<PortablePackManifestEntry>,
    extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PortablePackCreatedBy {
    application: String,
    application_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PortablePackManifestEntry {
    id: String,
    version: u64,
    path: String,
    byte_size: usize,
    file_sha256: String,
    semantic_sha256: String,
    exported_origin: String,
    overrides_builtin: bool,
}

pub fn preview_template_pack_export(
    service: &TemplateService,
    request: PreviewTemplatePackExportRequest,
) -> Result<PreviewTemplatePackExportResponse, TemplateApiError> {
    validate_template_ids(&request.template_ids)?;
    let resolved = resolve_export_templates(service, &request.template_ids)?;
    let templates: Vec<_> = resolved.iter().map(|item| item.preview.clone()).collect();
    let estimated_manifest_bytes = estimated_manifest_bytes(&templates)?;
    let estimated_uncompressed_bytes = templates
        .iter()
        .try_fold(estimated_manifest_bytes, |total, item| {
            total.checked_add(item.byte_size)
        })
        .ok_or_else(budget_error)?;
    if estimated_uncompressed_bytes > MAX_TOTAL_UNCOMPRESSED_BYTES {
        return Err(budget_error());
    }
    // Build once in memory during preview so the exact stored-ZIP overhead and
    // archive-size budget are validated without creating a temporary file.
    build_portable_archive(
        &resolved,
        "pack_00000000000000000000000000000000",
        "2000-01-01T00:00:00.000Z",
    )?;

    let plan_token = Uuid::new_v4().to_string();
    store_export_plan(
        plan_token.clone(),
        PortablePackExportPlan {
            created: Instant::now(),
            templates: templates.clone(),
        },
    );
    Ok(PreviewTemplatePackExportResponse {
        plan_token,
        template_count: templates.len(),
        templates,
        estimated_uncompressed_bytes,
        warnings: Vec::new(),
    })
}

pub fn export_template_pack(
    service: &TemplateService,
    request: ExportTemplatePackRequest,
) -> Result<ExportTemplatePackResponse, TemplateApiError> {
    let plan = take_export_plan(&request.plan_token)?;
    let ids: Vec<String> = plan
        .templates
        .iter()
        .map(|item| item.fingerprint.id.clone())
        .collect();
    let resolved = resolve_export_templates(service, &ids)?;
    let current: Vec<_> = resolved.iter().map(|item| item.preview.clone()).collect();
    if current != plan.templates {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"));
    }

    let destination = validate_destination(&request.destination_path, request.overwrite)?;
    let package_id = format!("pack_{}", Uuid::new_v4().simple());
    let created_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let archive = build_portable_archive(&resolved, &package_id, &created_at)?;
    let archive_sha256 = sha256_hex(&archive);
    persist_archive_atomically(&destination, &archive, request.overwrite)?;

    let package_file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("templates.meetily-template-pack")
        .to_owned();
    Ok(ExportTemplatePackResponse {
        package: PortablePackAuditSummary {
            package_id,
            package_schema_version: 1,
            package_file_name,
            archive_sha256,
            template_count: resolved.len(),
            created_at,
            application_version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        byte_size: archive.len(),
        exported_templates: resolved
            .into_iter()
            .map(|item| item.preview.fingerprint)
            .collect(),
    })
}

fn validate_template_ids(template_ids: &[String]) -> Result<(), TemplateApiError> {
    if template_ids.is_empty() || template_ids.len() > MAX_TEMPLATES {
        return Err(budget_error());
    }
    let unique: HashSet<&str> = template_ids.iter().map(String::as_str).collect();
    if unique.len() != template_ids.len() || template_ids.iter().any(|id| id.trim().is_empty()) {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_INVALID"));
    }
    Ok(())
}

fn resolve_export_templates(
    service: &TemplateService,
    template_ids: &[String],
) -> Result<Vec<ResolvedExportTemplate>, TemplateApiError> {
    let mut ids = template_ids.to_vec();
    ids.sort();
    let mut resolved = Vec::with_capacity(ids.len());
    let mut total_bytes = 0usize;
    for id in ids {
        let record = service
            .repository()
            .get(&id, None)
            .map_err(TemplateApiError::from)?;
        if record.origin != TemplateOrigin::Custom || record.read_only {
            return Err(
                TemplateApiError::from_code("TEMPLATE_READ_ONLY").with_param("templateId", id)
            );
        }
        if record.schema_version_on_disk != 2 {
            return Err(
                TemplateApiError::from_code("TEMPLATE_PACK_VERSION_UNSUPPORTED")
                    .with_param("templateId", id),
            );
        }
        let bytes = service
            .repository()
            .read_bytes_for_diagnostics(&id, Some(TemplateOrigin::Custom))
            .map_err(TemplateApiError::from)?;
        if bytes.is_empty() || bytes.len() > MAX_TEMPLATE_BYTES {
            return Err(budget_error().with_param("templateId", id));
        }
        let actual_file_sha256 = sha256_hex(&bytes);
        if actual_file_sha256 != record.file_sha256 {
            return Err(
                TemplateApiError::from_code("TEMPLATE_PACK_INTEGRITY_FAILED")
                    .with_param("templateId", id),
            );
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_INVALID"))?;
        if contains_sensitive_content(&value) {
            return Err(
                TemplateApiError::from_code("TEMPLATE_PACK_SENSITIVE_CONTENT")
                    .with_param("templateId", id),
            );
        }
        total_bytes = total_bytes
            .checked_add(bytes.len())
            .ok_or_else(budget_error)?;
        if total_bytes > MAX_TOTAL_UNCOMPRESSED_BYTES {
            return Err(budget_error());
        }
        resolved.push(ResolvedExportTemplate {
            preview: PortablePackExportTemplate {
                fingerprint: PortablePackTemplateFingerprint {
                    id: record.template.id,
                    version: record.template.version,
                    file_sha256: record.file_sha256,
                    semantic_sha256: record.semantic_sha256,
                },
                name: record.template.name,
                byte_size: bytes.len(),
                overrides_builtin: record.overrides_builtin,
            },
            bytes,
        });
    }
    Ok(resolved)
}

fn contains_sensitive_content(value: &Value) -> bool {
    match value {
        Value::Object(object) => object
            .iter()
            .any(|(key, value)| forbidden_sensitive_key(key) || contains_sensitive_content(value)),
        Value::Array(items) => items.iter().any(contains_sensitive_content),
        Value::String(text) => string_has_secret_or_absolute_path(text),
        _ => false,
    }
}

fn forbidden_sensitive_key(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    matches!(
        normalized.as_str(),
        "apikey"
            | "accesstoken"
            | "authtoken"
            | "authorization"
            | "clientsecret"
            | "password"
            | "secret"
            | "transcript"
            | "transcripttext"
            | "meetingcontent"
            | "audiopath"
            | "recordingpath"
            | "recordingfile"
            | "localpath"
    )
}

fn string_has_secret_or_absolute_path(text: &str) -> bool {
    BEARER_SECRET.is_match(text)
        || OPENAI_STYLE_SECRET.is_match(text)
        || ASSIGNED_SECRET.is_match(text)
        || WINDOWS_ABSOLUTE_PATH.is_match(text)
        || POSIX_ABSOLUTE_PATH.is_match(text)
}

fn estimated_manifest_bytes(
    templates: &[PortablePackExportTemplate],
) -> Result<usize, TemplateApiError> {
    let manifest = manifest_for(
        templates,
        "pack_00000000000000000000000000000000",
        "2000-01-01T00:00:00.000Z",
    );
    serialized_manifest(&manifest).map(|bytes| bytes.len())
}

fn manifest_for(
    templates: &[PortablePackExportTemplate],
    package_id: &str,
    created_at: &str,
) -> PortablePackManifest {
    PortablePackManifest {
        package_schema_version: 1,
        package_type: "meetily.template-pack".to_owned(),
        package_id: package_id.to_owned(),
        created_at: created_at.to_owned(),
        created_by: PortablePackCreatedBy {
            application: "Meetily".to_owned(),
            application_version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        hash_algorithm: "sha256".to_owned(),
        semantic_hash_algorithm: "meetily-template-semantic-v1".to_owned(),
        template_count: templates.len(),
        templates: templates
            .iter()
            .map(|item| PortablePackManifestEntry {
                id: item.fingerprint.id.clone(),
                version: item.fingerprint.version,
                path: format!("templates/{}.json", item.fingerprint.id),
                byte_size: item.byte_size,
                file_sha256: item.fingerprint.file_sha256.clone(),
                semantic_sha256: item.fingerprint.semantic_sha256.clone(),
                exported_origin: "custom".to_owned(),
                overrides_builtin: item.overrides_builtin,
            })
            .collect(),
        extensions: BTreeMap::new(),
    }
}

fn serialized_manifest(manifest: &PortablePackManifest) -> Result<Vec<u8>, TemplateApiError> {
    let value = serde_json::to_value(manifest)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_EXPORT_FAILED"))?;
    let validator = PORTABLE_PACK_MANIFEST_VALIDATOR
        .as_ref()
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_EXPORT_FAILED"))?;
    if !validator.is_valid(&value) {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_INVALID"));
    }
    let mut bytes = serde_json::to_vec_pretty(&value)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_EXPORT_FAILED"))?;
    bytes.push(b'\n');
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(budget_error());
    }
    Ok(bytes)
}

fn build_portable_archive(
    templates: &[ResolvedExportTemplate],
    package_id: &str,
    created_at: &str,
) -> Result<Vec<u8>, TemplateApiError> {
    if templates.is_empty() || templates.len() > MAX_TEMPLATES {
        return Err(budget_error());
    }
    for item in templates {
        if item.bytes.is_empty()
            || item.bytes.len() > MAX_TEMPLATE_BYTES
            || item.preview.byte_size != item.bytes.len()
            || item.preview.fingerprint.file_sha256 != sha256_hex(&item.bytes)
        {
            return Err(TemplateApiError::from_code(
                "TEMPLATE_PACK_INTEGRITY_FAILED",
            ));
        }
    }
    let previews: Vec<_> = templates.iter().map(|item| item.preview.clone()).collect();
    let manifest_bytes = serialized_manifest(&manifest_for(&previews, package_id, created_at))?;
    let total_uncompressed = templates
        .iter()
        .try_fold(manifest_bytes.len(), |total, item| {
            total.checked_add(item.bytes.len())
        })
        .ok_or_else(budget_error)?;
    if total_uncompressed > MAX_TOTAL_UNCOMPRESSED_BYTES
        || templates.len() + 1 > MAX_ARCHIVE_ENTRIES
    {
        return Err(budget_error());
    }

    let cursor = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(cursor);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(0o600)
        .large_file(false);
    writer
        .start_file("manifest.json", options)
        .and_then(|_| writer.write_all(&manifest_bytes).map_err(Into::into))
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_EXPORT_FAILED"))?;
    for item in templates {
        writer
            .start_file(
                format!("templates/{}.json", item.preview.fingerprint.id),
                options,
            )
            .and_then(|_| writer.write_all(&item.bytes).map_err(Into::into))
            .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_EXPORT_FAILED"))?;
    }
    let archive = writer
        .finish()
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_EXPORT_FAILED"))?
        .into_inner();
    if archive.len() > MAX_ARCHIVE_BYTES {
        return Err(budget_error());
    }
    verify_built_archive(&archive, &manifest_bytes, templates)?;
    Ok(archive)
}

fn verify_built_archive(
    bytes: &[u8],
    expected_manifest: &[u8],
    templates: &[ResolvedExportTemplate],
) -> Result<(), TemplateApiError> {
    if !bytes.starts_with(b"PK") {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_INTEGRITY_FAILED",
        ));
    }
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_INTEGRITY_FAILED"))?;
    if archive.len() != templates.len() + 1 || archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_INTEGRITY_FAILED",
        ));
    }
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_INTEGRITY_FAILED"))?;
        let expected_name = if index == 0 {
            "manifest.json".to_owned()
        } else {
            format!(
                "templates/{}.json",
                templates[index - 1].preview.fingerprint.id
            )
        };
        if entry.name() != expected_name
            || entry.is_dir()
            || entry.compression() != CompressionMethod::Stored
            || entry.size() as usize > MAX_TEMPLATE_BYTES.max(MAX_MANIFEST_BYTES)
        {
            return Err(TemplateApiError::from_code(
                "TEMPLATE_PACK_INTEGRITY_FAILED",
            ));
        }
        let mut actual = Vec::new();
        entry
            .read_to_end(&mut actual)
            .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_INTEGRITY_FAILED"))?;
        let expected = if index == 0 {
            expected_manifest
        } else {
            templates[index - 1].bytes.as_slice()
        };
        if actual != expected {
            return Err(TemplateApiError::from_code(
                "TEMPLATE_PACK_INTEGRITY_FAILED",
            ));
        }
    }
    Ok(())
}

pub fn preview_template_pack_import(
    service: &TemplateService,
    request: PreviewTemplatePackImportRequest,
) -> Result<PreviewTemplatePackImportResponse, TemplateApiError> {
    let source = validate_import_source(&request.source_path)?;
    let package_file_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("templates.meetily-template-pack")
        .to_owned();
    let archive_bytes = read_archive_bounded(&source)?;
    validate_classic_single_disk_zip(&archive_bytes)?;
    let archive_sha256 = sha256_hex(&archive_bytes);
    let (manifest, templates, total_uncompressed_bytes) = inspect_portable_archive(&archive_bytes)?;
    let items = resolve_import_conflicts(service, &templates)?;
    let package = PortablePackAuditSummary {
        package_id: manifest.package_id,
        package_schema_version: manifest.package_schema_version,
        package_file_name,
        archive_sha256: archive_sha256.clone(),
        template_count: templates.len(),
        created_at: manifest.created_at,
        application_version: manifest.created_by.application_version,
    };
    let plan_token = Uuid::new_v4().to_string();
    store_import_plan(
        plan_token.clone(),
        PortablePackImportPlan {
            created: Instant::now(),
            source_path: source,
            archive_sha256,
            package: package.clone(),
            templates: templates
                .iter()
                .map(|template| template.preview.clone())
                .collect(),
            items: items.clone(),
        },
    );
    Ok(PreviewTemplatePackImportResponse {
        plan_token,
        package,
        items,
        total_uncompressed_bytes,
        warnings: Vec::new(),
    })
}

pub fn plan_template_pack_import(
    service: &TemplateService,
    request: PlanTemplatePackImportRequest,
) -> Result<PlanTemplatePackImportResponse, TemplateApiError> {
    if request.decisions.len() > MAX_TEMPLATES {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_INVALID"));
    }
    let preview_plan = load_import_plan(&request.preview_plan_token)?;
    let templates = revalidate_import_plan(&preview_plan)?;
    let current_items = resolve_import_conflicts(service, &templates)?;
    if !import_conflict_snapshots_match(&preview_plan.items, &current_items) {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_CONFLICT_CHANGED",
        ));
    }

    let planned_at = Utc::now();
    let operations = build_import_operations(
        service,
        &preview_plan,
        &templates,
        &request.decisions,
        planned_at.fixed_offset(),
    )?;
    let final_items = resolve_import_conflicts(service, &templates)?;
    if !import_conflict_snapshots_match(&preview_plan.items, &final_items) {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_CONFLICT_CHANGED",
        ));
    }
    let summary = summarize_import_operations(&operations);
    consume_import_plan(&request.preview_plan_token)?;

    let execution_plan_token = Uuid::new_v4().to_string();
    store_import_execution_plan(
        execution_plan_token.clone(),
        PortablePackExecutionPlan {
            created: Instant::now(),
            planned_at: planned_at.fixed_offset(),
            source_path: preview_plan.source_path,
            archive_sha256: preview_plan.archive_sha256,
            package: preview_plan.package.clone(),
            operations: operations.clone(),
        },
    );
    let expires_at = (planned_at
        + ChronoDuration::seconds(EXPORT_PLAN_TTL.as_secs().try_into().unwrap_or(900)))
    .to_rfc3339_opts(SecondsFormat::Millis, true);
    Ok(PlanTemplatePackImportResponse {
        execution_plan_token,
        expires_at,
        package: preview_plan.package,
        operations,
        summary,
        warnings: Vec::new(),
    })
}

pub fn execute_template_pack_import(
    service: &TemplateService,
    request: ExecuteTemplatePackImportRequest,
) -> Result<ExecuteTemplatePackImportResponse, TemplateApiError> {
    let cancellation = register_import_execution(&request.execution_id)?;
    let result = execute_template_pack_import_inner(service, &request, &cancellation);
    unregister_import_execution(&request.execution_id);
    result
}

pub fn cancel_template_pack_import(
    request: CancelTemplatePackImportRequest,
) -> Result<CancelTemplatePackImportResponse, TemplateApiError> {
    if !valid_plan_token(&request.execution_id) {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_EXECUTION_NOT_FOUND",
        ));
    }
    let executions = ACTIVE_IMPORT_EXECUTIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let cancellation = executions
        .get(&request.execution_id)
        .ok_or_else(|| TemplateApiError::from_code("TEMPLATE_PACK_EXECUTION_NOT_FOUND"))?;
    cancellation.store(true, Ordering::SeqCst);
    Ok(CancelTemplatePackImportResponse {
        execution_id: request.execution_id,
        cancellation_requested: true,
    })
}

pub fn recover_template_pack_imports(
    service: &TemplateService,
) -> Result<PortablePackRecoverySummary, TemplateApiError> {
    let _guard = service
        .repository()
        .lock_writes()
        .map_err(TemplateApiError::from)?;
    recover_import_transactions_locked(service.repository().root())
}

fn execute_template_pack_import_inner(
    service: &TemplateService,
    request: &ExecuteTemplatePackImportRequest,
    cancellation: &AtomicBool,
) -> Result<ExecuteTemplatePackImportResponse, TemplateApiError> {
    check_import_cancellation(cancellation)?;
    let plan = take_import_execution_plan(&request.execution_plan_token)?;
    let templates = revalidate_execution_plan(&plan)?;
    let materialized = materialize_import_operations(&plan, &templates)?;
    check_import_cancellation(cancellation)?;

    let _guard = service
        .repository()
        .lock_writes()
        .map_err(TemplateApiError::from)?;
    let recovery = recover_import_transactions_locked(service.repository().root())?;
    validate_execution_conflicts(service, &plan, &templates)?;
    check_import_cancellation(cancellation)?;
    execute_import_transaction_locked(
        service.repository().root(),
        &request.execution_id,
        &materialized,
        cancellation,
        ImportTransactionInterrupt::default(),
    )?;

    Ok(ExecuteTemplatePackImportResponse {
        execution_id: request.execution_id.clone(),
        package: plan.package,
        summary: summarize_import_operations(&plan.operations),
        results: plan
            .operations
            .into_iter()
            .map(|operation| PortablePackImportExecutionItem {
                item_id: operation.item_id,
                operation: operation.operation,
                target: operation.target,
            })
            .collect(),
        recovery,
    })
}

fn register_import_execution(execution_id: &str) -> Result<Arc<AtomicBool>, TemplateApiError> {
    if !valid_plan_token(execution_id) {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_EXECUTION_NOT_FOUND",
        ));
    }
    let mut executions = ACTIVE_IMPORT_EXECUTIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if executions.contains_key(execution_id) {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_EXECUTION_ALREADY_ACTIVE",
        ));
    }
    let cancellation = Arc::new(AtomicBool::new(false));
    executions.insert(execution_id.to_owned(), cancellation.clone());
    Ok(cancellation)
}

fn unregister_import_execution(execution_id: &str) {
    ACTIVE_IMPORT_EXECUTIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(execution_id);
}

fn check_import_cancellation(cancellation: &AtomicBool) -> Result<(), TemplateApiError> {
    if cancellation.load(Ordering::SeqCst) {
        Err(TemplateApiError::from_code("TEMPLATE_CANCELLED"))
    } else {
        Ok(())
    }
}

fn revalidate_execution_plan(
    plan: &PortablePackExecutionPlan,
) -> Result<Vec<ResolvedImportTemplate>, TemplateApiError> {
    let source_path = plan
        .source_path
        .to_str()
        .ok_or_else(|| TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"))?;
    let source = validate_import_source(source_path)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"))?;
    if source != plan.source_path {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"));
    }
    let archive_bytes = read_archive_bounded(&source)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"))?;
    if sha256_hex(&archive_bytes) != plan.archive_sha256 {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"));
    }
    validate_classic_single_disk_zip(&archive_bytes)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"))?;
    let (manifest, templates, _) = inspect_portable_archive(&archive_bytes)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"))?;
    if manifest.package_id != plan.package.package_id
        || manifest.package_schema_version != plan.package.package_schema_version
        || manifest.created_at != plan.package.created_at
        || manifest.created_by.application_version != plan.package.application_version
        || templates.len() != plan.package.template_count
        || templates.len() != plan.operations.len()
    {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"));
    }
    let resolved_by_id: HashMap<_, _> = templates
        .iter()
        .map(|template| (template.preview.fingerprint.id.as_str(), template))
        .collect();
    if plan.operations.iter().any(|operation| {
        resolved_by_id
            .get(operation.source.fingerprint.id.as_str())
            .is_none_or(|resolved| resolved.preview != operation.source)
    }) {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"));
    }
    Ok(templates)
}

fn materialize_import_operations(
    plan: &PortablePackExecutionPlan,
    templates: &[ResolvedImportTemplate],
) -> Result<Vec<MaterializedImportOperation>, TemplateApiError> {
    let resolved_by_id: HashMap<_, _> = templates
        .iter()
        .map(|template| (template.preview.fingerprint.id.as_str(), template))
        .collect();
    let mut target_ids = HashSet::new();
    let mut materialized = Vec::new();
    for operation in &plan.operations {
        if operation.operation == PortablePackImportOperationKind::Skip {
            if operation.target.is_some() {
                return Err(TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"));
            }
            continue;
        }
        let source = resolved_by_id
            .get(operation.source.fingerprint.id.as_str())
            .ok_or_else(|| TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"))?;
        let target = operation
            .target
            .as_ref()
            .ok_or_else(|| TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"))?;
        let (resolved_target, expected_existing) = match operation.operation {
            PortablePackImportOperationKind::Create
            | PortablePackImportOperationKind::ReplaceCustom => {
                if target != &source.preview {
                    return Err(TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"));
                }
                ((*source).clone(), operation.expected_existing.clone())
            }
            PortablePackImportOperationKind::KeepBoth => (
                materialize_keep_both_template(
                    &source.template,
                    &target.fingerprint.id,
                    plan.planned_at,
                )?,
                operation.expected_existing.clone(),
            ),
            PortablePackImportOperationKind::Skip => unreachable!(),
        };
        if resolved_target.preview != *target
            || sha256_hex(&resolved_target.bytes) != target.fingerprint.file_sha256
            || !target_ids.insert(target.fingerprint.id.clone())
        {
            return Err(TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"));
        }
        materialized.push(MaterializedImportOperation {
            item_id: operation.item_id.clone(),
            operation: operation.operation,
            target: target.clone(),
            bytes: resolved_target.bytes,
            expected_existing,
        });
    }
    Ok(materialized)
}

fn validate_execution_conflicts(
    service: &TemplateService,
    plan: &PortablePackExecutionPlan,
    templates: &[ResolvedImportTemplate],
) -> Result<(), TemplateApiError> {
    let current_items = resolve_import_conflicts(service, templates)?;
    let current_by_id: HashMap<_, _> = current_items
        .iter()
        .map(|item| (item.template.fingerprint.id.as_str(), item))
        .collect();
    let listed_ids: HashSet<_> = service
        .repository()
        .list()
        .map_err(TemplateApiError::from)?
        .templates
        .into_iter()
        .map(|template| template.id)
        .collect();
    for operation in &plan.operations {
        let current = current_by_id
            .get(operation.source.fingerprint.id.as_str())
            .ok_or_else(|| TemplateApiError::from_code("TEMPLATE_PACK_CONFLICT_CHANGED"))?;
        let conflict_matches = match operation.operation {
            PortablePackImportOperationKind::Create => {
                current.conflict_kind == PortablePackConflictKind::None
                    && operation.expected_existing.is_none()
            }
            PortablePackImportOperationKind::Skip => {
                current.existing == operation.expected_existing
                    && current.conflict_kind != PortablePackConflictKind::None
            }
            PortablePackImportOperationKind::KeepBoth => {
                current.existing == operation.expected_existing
                    && current.conflict_kind != PortablePackConflictKind::None
                    && operation
                        .target
                        .as_ref()
                        .is_some_and(|target| !listed_ids.contains(&target.fingerprint.id))
            }
            PortablePackImportOperationKind::ReplaceCustom => {
                current.conflict_kind == PortablePackConflictKind::Custom
                    && current.existing == operation.expected_existing
            }
        };
        if !conflict_matches {
            return Err(TemplateApiError::from_code(
                "TEMPLATE_PACK_CONFLICT_CHANGED",
            ));
        }
    }
    Ok(())
}

fn revalidate_import_plan(
    plan: &PortablePackImportPlan,
) -> Result<Vec<ResolvedImportTemplate>, TemplateApiError> {
    let source_path = plan
        .source_path
        .to_str()
        .ok_or_else(|| TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"))?;
    let source = validate_import_source(source_path)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"))?;
    if source != plan.source_path {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"));
    }
    let archive_bytes = read_archive_bounded(&source)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"))?;
    if sha256_hex(&archive_bytes) != plan.archive_sha256 {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"));
    }
    validate_classic_single_disk_zip(&archive_bytes)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"))?;
    let (manifest, templates, _) = inspect_portable_archive(&archive_bytes)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"))?;
    if manifest.package_id != plan.package.package_id
        || manifest.package_schema_version != plan.package.package_schema_version
        || manifest.created_at != plan.package.created_at
        || manifest.created_by.application_version != plan.package.application_version
        || templates.len() != plan.package.template_count
        || templates
            .iter()
            .map(|template| &template.preview)
            .ne(plan.templates.iter())
    {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"));
    }
    Ok(templates)
}

fn import_conflict_snapshots_match(
    expected: &[PortablePackImportItem],
    current: &[PortablePackImportItem],
) -> bool {
    if expected.len() != current.len() {
        return false;
    }
    let current_by_template_id: HashMap<_, _> = current
        .iter()
        .map(|item| (item.template.fingerprint.id.as_str(), item))
        .collect();
    expected.iter().all(|item| {
        current_by_template_id
            .get(item.template.fingerprint.id.as_str())
            .is_some_and(|current| {
                item.template == current.template
                    && item.conflict_kind == current.conflict_kind
                    && item.allowed_strategies == current.allowed_strategies
                    && item.existing == current.existing
            })
    })
}

fn build_import_operations(
    service: &TemplateService,
    plan: &PortablePackImportPlan,
    templates: &[ResolvedImportTemplate],
    decisions: &[PortablePackImportDecision],
    planned_at: chrono::DateTime<chrono::FixedOffset>,
) -> Result<Vec<PortablePackPlannedImportOperation>, TemplateApiError> {
    let mut decision_by_item_id = HashMap::new();
    for decision in decisions {
        if decision.item_id.trim().is_empty() || decision.item_id.len() > 128 {
            return Err(TemplateApiError::from_code(
                "TEMPLATE_PACK_DECISION_UNKNOWN_ITEM",
            ));
        }
        if decision_by_item_id
            .insert(decision.item_id.as_str(), decision.strategy)
            .is_some()
        {
            return Err(TemplateApiError::from_code(
                "TEMPLATE_PACK_DECISION_DUPLICATE",
            ));
        }
    }
    let plan_item_ids: HashSet<_> = plan
        .items
        .iter()
        .map(|item| item.item_id.as_str())
        .collect();
    if decision_by_item_id
        .keys()
        .any(|item_id| !plan_item_ids.contains(item_id))
    {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_DECISION_UNKNOWN_ITEM",
        ));
    }

    let listed = service
        .repository()
        .list()
        .map_err(TemplateApiError::from)?;
    let mut reserved_ids: HashSet<String> = listed
        .templates
        .into_iter()
        .map(|template| template.id)
        .chain(
            plan.items
                .iter()
                .map(|item| item.template.fingerprint.id.clone()),
        )
        .collect();
    let resolved_by_id: HashMap<_, _> = templates
        .iter()
        .map(|template| (template.preview.fingerprint.id.as_str(), template))
        .collect();
    let mut operations = Vec::with_capacity(plan.items.len());
    for item in &plan.items {
        let strategy = decision_by_item_id.get(item.item_id.as_str()).copied();
        let operation = match item.conflict_kind {
            PortablePackConflictKind::None => {
                if strategy.is_some() {
                    return Err(TemplateApiError::from_code(
                        "TEMPLATE_PACK_DECISION_NOT_ALLOWED",
                    ));
                }
                PortablePackPlannedImportOperation {
                    item_id: item.item_id.clone(),
                    operation: PortablePackImportOperationKind::Create,
                    source: item.template.clone(),
                    target: Some(item.template.clone()),
                    expected_existing: None,
                }
            }
            PortablePackConflictKind::Custom | PortablePackConflictKind::Readonly => {
                let strategy = strategy.ok_or_else(|| {
                    TemplateApiError::from_code("TEMPLATE_PACK_DECISION_MISSING")
                        .with_param("itemId", item.item_id.as_str())
                })?;
                if !item.allowed_strategies.contains(&strategy) {
                    return Err(
                        TemplateApiError::from_code("TEMPLATE_PACK_DECISION_NOT_ALLOWED")
                            .with_param("itemId", item.item_id.as_str()),
                    );
                }
                match strategy {
                    PortablePackConflictStrategy::Skip => PortablePackPlannedImportOperation {
                        item_id: item.item_id.clone(),
                        operation: PortablePackImportOperationKind::Skip,
                        source: item.template.clone(),
                        target: None,
                        expected_existing: item.existing.clone(),
                    },
                    PortablePackConflictStrategy::ReplaceCustom => {
                        if item.conflict_kind != PortablePackConflictKind::Custom {
                            return Err(TemplateApiError::from_code(
                                "TEMPLATE_PACK_DECISION_NOT_ALLOWED",
                            )
                            .with_param("itemId", item.item_id.as_str()));
                        }
                        PortablePackPlannedImportOperation {
                            item_id: item.item_id.clone(),
                            operation: PortablePackImportOperationKind::ReplaceCustom,
                            source: item.template.clone(),
                            target: Some(item.template.clone()),
                            expected_existing: item.existing.clone(),
                        }
                    }
                    PortablePackConflictStrategy::KeepBoth => {
                        let new_id =
                            next_available_import_id(&item.template.fingerprint.id, &reserved_ids)?;
                        reserved_ids.insert(new_id.clone());
                        let resolved = resolved_by_id
                            .get(item.template.fingerprint.id.as_str())
                            .ok_or_else(|| {
                                TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE")
                            })?;
                        let transformed =
                            transform_keep_both_template(&resolved.template, &new_id, planned_at)?;
                        PortablePackPlannedImportOperation {
                            item_id: item.item_id.clone(),
                            operation: PortablePackImportOperationKind::KeepBoth,
                            source: item.template.clone(),
                            target: Some(transformed),
                            expected_existing: item.existing.clone(),
                        }
                    }
                }
            }
            PortablePackConflictKind::DuplicateInPackage => {
                return Err(TemplateApiError::from_code(
                    "TEMPLATE_PACK_DECISION_NOT_ALLOWED",
                ));
            }
        };
        operations.push(operation);
    }
    Ok(operations)
}

fn next_available_import_id(
    source_id: &str,
    reserved_ids: &HashSet<String>,
) -> Result<String, TemplateApiError> {
    for index in 1..=10_000u32 {
        let suffix = if index == 1 {
            "_imported".to_owned()
        } else {
            format!("_imported_{index}")
        };
        let base_length = 80usize.saturating_sub(suffix.len());
        let base = source_id
            .get(..source_id.len().min(base_length))
            .unwrap_or(source_id)
            .trim_end_matches(['_', '-']);
        let candidate = format!("{base}{suffix}");
        if !reserved_ids.contains(&candidate) {
            return Ok(candidate);
        }
    }
    Err(TemplateApiError::from_code(
        "TEMPLATE_PACK_KEEP_BOTH_ID_EXHAUSTED",
    ))
}

fn transform_keep_both_template(
    source: &TemplateV2,
    new_id: &str,
    planned_at: chrono::DateTime<chrono::FixedOffset>,
) -> Result<PortablePackImportTemplate, TemplateApiError> {
    Ok(materialize_keep_both_template(source, new_id, planned_at)?.preview)
}

fn materialize_keep_both_template(
    source: &TemplateV2,
    new_id: &str,
    planned_at: chrono::DateTime<chrono::FixedOffset>,
) -> Result<ResolvedImportTemplate, TemplateApiError> {
    let mut transformed = source.clone();
    transformed.id = new_id.to_owned();
    transformed.version = 1;
    transformed.created_at = planned_at;
    transformed.updated_at = planned_at;
    transformed.source = TemplateSource {
        source_type: TemplateSourceType::Duplicate,
        original_file_name: None,
        original_file_sha256: None,
        imported_at: None,
        copied_from_template_id: Some(source.id.clone()),
    };
    if !validate_template_v2(&transformed).valid {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_INVALID"));
    }
    let semantic_sha256 = semantic_sha256(&transformed)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_INVALID"))?;
    let mut bytes = serde_json::to_vec_pretty(&transformed)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_INVALID"))?;
    bytes.push(b'\n');
    let preview = PortablePackImportTemplate {
        fingerprint: PortablePackTemplateFingerprint {
            id: transformed.id.clone(),
            version: transformed.version,
            file_sha256: sha256_hex(&bytes),
            semantic_sha256,
        },
        name: transformed.name.clone(),
        byte_size: bytes.len(),
    };
    Ok(ResolvedImportTemplate {
        preview,
        template: transformed,
        bytes,
    })
}

fn summarize_import_operations(
    operations: &[PortablePackPlannedImportOperation],
) -> PortablePackImportPlanSummary {
    let mut summary = PortablePackImportPlanSummary {
        create_count: 0,
        replace_count: 0,
        skip_count: 0,
        transformed_count: 0,
    };
    for operation in operations {
        match operation.operation {
            PortablePackImportOperationKind::Create => summary.create_count += 1,
            PortablePackImportOperationKind::Skip => summary.skip_count += 1,
            PortablePackImportOperationKind::KeepBoth => {
                summary.create_count += 1;
                summary.transformed_count += 1;
            }
            PortablePackImportOperationKind::ReplaceCustom => summary.replace_count += 1,
        }
    }
    summary
}

fn execute_import_transaction_locked(
    repository_root: &Path,
    execution_id: &str,
    operations: &[MaterializedImportOperation],
    cancellation: &AtomicBool,
    interrupt: ImportTransactionInterrupt,
) -> Result<(), TemplateApiError> {
    if operations.is_empty() {
        return check_import_cancellation(cancellation);
    }
    let (transaction_root, journal) =
        prepare_import_transaction(repository_root, execution_id, operations)?;
    let commit_result = commit_import_transaction(
        repository_root,
        &transaction_root,
        &journal,
        cancellation,
        interrupt,
    );
    match commit_result {
        Ok(()) => {
            if let Err(error) =
                remove_import_transaction_directory(repository_root, &transaction_root)
            {
                tracing::warn!(
                    execution_id,
                    detail = %error.code,
                    "portable template import committed but transaction cleanup was deferred"
                );
            }
            Ok(())
        }
        Err(operation_error) => {
            if let Err(recovery_error) =
                rollback_import_transaction(repository_root, &transaction_root, &journal)
            {
                tracing::error!(
                    execution_id,
                    operation_code = %operation_error.code,
                    recovery_code = %recovery_error.code,
                    "portable template import rollback failed"
                );
                return Err(recovery_error);
            }
            remove_import_transaction_directory(repository_root, &transaction_root)?;
            Err(operation_error)
        }
    }
}

fn prepare_import_transaction(
    repository_root: &Path,
    execution_id: &str,
    operations: &[MaterializedImportOperation],
) -> Result<(PathBuf, ImportTransactionJournal), TemplateApiError> {
    if !valid_plan_token(execution_id) || operations.len() > MAX_TEMPLATES {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_IMPORT_FAILED"));
    }
    let temporary_root = repository_root.join(".tmp");
    let transaction_root =
        temporary_root.join(format!("{IMPORT_TRANSACTION_PREFIX}{execution_id}"));
    if fs::symlink_metadata(&transaction_root).is_ok() {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_EXECUTION_ALREADY_ACTIVE",
        ));
    }
    fs::create_dir(&transaction_root).map_err(import_io_error)?;
    let mut pending = PendingImportTransactionDirectory {
        repository_root: repository_root.to_path_buf(),
        transaction_root: transaction_root.clone(),
        armed: true,
    };

    let mut journal_operations = Vec::with_capacity(operations.len());
    for (index, operation) in operations.iter().enumerate() {
        let target_file_name = format!("{}.json", operation.target.fingerprint.id);
        let target = repository_root.join(&target_file_name);
        let staged_file_name = format!("{index:03}.new");
        let staged = transaction_root.join(&staged_file_name);
        let new_sha256 = sha256_hex(&operation.bytes);
        if new_sha256 != operation.target.fingerprint.file_sha256
            || operation.bytes.len() != operation.target.byte_size
        {
            let _ = remove_import_transaction_directory(repository_root, &transaction_root);
            return Err(TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"));
        }
        write_new_synced(&staged, &operation.bytes)?;

        let (transaction_operation, backup_file_name, original_sha256) = match operation.operation {
            PortablePackImportOperationKind::Create | PortablePackImportOperationKind::KeepBoth => {
                if fs::symlink_metadata(&target).is_ok() {
                    let _ = remove_import_transaction_directory(repository_root, &transaction_root);
                    return Err(TemplateApiError::from_code(
                        "TEMPLATE_PACK_CONFLICT_CHANGED",
                    ));
                }
                (ImportTransactionOperationKind::Create, None, None)
            }
            PortablePackImportOperationKind::ReplaceCustom => {
                let expected = operation
                    .expected_existing
                    .as_ref()
                    .ok_or_else(|| TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"))?;
                let metadata = fs::symlink_metadata(&target)
                    .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_CONFLICT_CHANGED"))?;
                if is_reparse_metadata(&metadata) || !metadata.is_file() {
                    let _ = remove_import_transaction_directory(repository_root, &transaction_root);
                    return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
                }
                if bounded_file_sha256(&target)? != expected.file_sha256 {
                    let _ = remove_import_transaction_directory(repository_root, &transaction_root);
                    return Err(TemplateApiError::from_code(
                        "TEMPLATE_PACK_CONFLICT_CHANGED",
                    ));
                }
                (
                    ImportTransactionOperationKind::Replace,
                    Some(format!("{index:03}.old")),
                    Some(expected.file_sha256.clone()),
                )
            }
            PortablePackImportOperationKind::Skip => {
                let _ = remove_import_transaction_directory(repository_root, &transaction_root);
                return Err(TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"));
            }
        };
        journal_operations.push(ImportTransactionOperation {
            item_id: operation.item_id.clone(),
            operation: transaction_operation,
            target_file_name,
            staged_file_name,
            backup_file_name,
            new_sha256,
            original_sha256,
        });
    }

    let journal = ImportTransactionJournal {
        schema_version: 1,
        execution_id: execution_id.to_owned(),
        operations: journal_operations,
    };
    let mut journal_bytes = serde_json::to_vec_pretty(&journal)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_IMPORT_FAILED"))?;
    journal_bytes.push(b'\n');
    write_new_synced(
        &transaction_root.join(IMPORT_TRANSACTION_JOURNAL),
        &journal_bytes,
    )?;
    sync_parent_directory(&transaction_root)?;
    sync_parent_directory(&temporary_root)?;
    pending.disarm();
    Ok((transaction_root, journal))
}

fn commit_import_transaction(
    repository_root: &Path,
    transaction_root: &Path,
    journal: &ImportTransactionJournal,
    cancellation: &AtomicBool,
    interrupt: ImportTransactionInterrupt,
) -> Result<(), TemplateApiError> {
    check_import_cancellation(cancellation)?;
    write_transaction_marker(transaction_root, IMPORT_TRANSACTION_COMMITTING)?;
    let mut applied = 0usize;
    for operation in &journal.operations {
        if interrupt
            .cancel_after_applied
            .is_some_and(|threshold| applied >= threshold)
        {
            cancellation.store(true, Ordering::SeqCst);
        }
        check_import_cancellation(cancellation)?;
        if interrupt
            .fail_after_applied
            .is_some_and(|threshold| applied >= threshold)
        {
            return Err(TemplateApiError::from_code("TEMPLATE_PACK_IMPORT_FAILED"));
        }
        let target = repository_root.join(&operation.target_file_name);
        let staged = transaction_root.join(&operation.staged_file_name);
        match operation.operation {
            ImportTransactionOperationKind::Create => {
                if fs::symlink_metadata(&target).is_ok() {
                    return Err(TemplateApiError::from_code(
                        "TEMPLATE_PACK_CONFLICT_CHANGED",
                    ));
                }
                fs::rename(&staged, &target).map_err(import_io_error)?;
            }
            ImportTransactionOperationKind::Replace => {
                let backup_name = operation
                    .backup_file_name
                    .as_deref()
                    .ok_or_else(|| TemplateApiError::from_code("TEMPLATE_PACK_RECOVERY_FAILED"))?;
                let expected_original = operation
                    .original_sha256
                    .as_deref()
                    .ok_or_else(|| TemplateApiError::from_code("TEMPLATE_PACK_RECOVERY_FAILED"))?;
                if bounded_file_sha256(&target)? != expected_original {
                    return Err(TemplateApiError::from_code(
                        "TEMPLATE_PACK_CONFLICT_CHANGED",
                    ));
                }
                let backup = transaction_root.join(backup_name);
                fs::rename(&target, &backup).map_err(import_io_error)?;
                if let Err(error) = fs::rename(&staged, &target) {
                    let _ = fs::rename(&backup, &target);
                    return Err(import_io_error(error));
                }
            }
        }
        sync_parent_directory(repository_root)?;
        applied += 1;
    }
    if interrupt
        .cancel_after_applied
        .is_some_and(|threshold| applied >= threshold)
    {
        cancellation.store(true, Ordering::SeqCst);
    }
    check_import_cancellation(cancellation)?;
    for operation in &journal.operations {
        let target = repository_root.join(&operation.target_file_name);
        if bounded_file_sha256(&target)? != operation.new_sha256 {
            return Err(TemplateApiError::from_code("TEMPLATE_PACK_IMPORT_FAILED"));
        }
    }
    write_transaction_marker(transaction_root, IMPORT_TRANSACTION_COMMITTED)?;
    sync_parent_directory(repository_root)?;
    Ok(())
}

fn rollback_import_transaction(
    repository_root: &Path,
    transaction_root: &Path,
    journal: &ImportTransactionJournal,
) -> Result<(), TemplateApiError> {
    for operation in journal.operations.iter().rev() {
        let target_path = repository_root.join(&operation.target_file_name);
        let target_hash = optional_bounded_file_sha256(&target_path)?;
        match operation.operation {
            ImportTransactionOperationKind::Create => match target_hash.as_deref() {
                None => {}
                Some(hash) if hash == operation.new_sha256 => {
                    fs::remove_file(&target_path).map_err(import_io_error)?;
                }
                Some(_) => {
                    return Err(TemplateApiError::from_code("TEMPLATE_PACK_RECOVERY_FAILED"));
                }
            },
            ImportTransactionOperationKind::Replace => {
                let backup_name = operation
                    .backup_file_name
                    .as_deref()
                    .ok_or_else(|| TemplateApiError::from_code("TEMPLATE_PACK_RECOVERY_FAILED"))?;
                let expected_original = operation
                    .original_sha256
                    .as_deref()
                    .ok_or_else(|| TemplateApiError::from_code("TEMPLATE_PACK_RECOVERY_FAILED"))?;
                let backup = transaction_root.join(backup_name);
                let backup_hash = optional_bounded_file_sha256(&backup)?;
                match (target_hash.as_deref(), backup_hash.as_deref()) {
                    (Some(current_hash), _) if current_hash == expected_original => {
                        if backup_hash.is_some() {
                            fs::remove_file(&backup).map_err(import_io_error)?;
                        }
                    }
                    (Some(current_hash), Some(backup_hash))
                        if current_hash == operation.new_sha256
                            && backup_hash == expected_original =>
                    {
                        fs::remove_file(&target_path).map_err(import_io_error)?;
                        fs::rename(&backup, &target_path).map_err(import_io_error)?;
                    }
                    (None, Some(backup_hash)) if backup_hash == expected_original => {
                        fs::rename(&backup, &target_path).map_err(import_io_error)?;
                    }
                    _ => {
                        return Err(TemplateApiError::from_code("TEMPLATE_PACK_RECOVERY_FAILED"));
                    }
                }
            }
        }
    }
    sync_parent_directory(repository_root)?;
    Ok(())
}

fn recover_import_transactions_locked(
    repository_root: &Path,
) -> Result<PortablePackRecoverySummary, TemplateApiError> {
    let temporary_root = repository_root.join(".tmp");
    let mut recovery = PortablePackRecoverySummary {
        rolled_back_transactions: 0,
        finalized_transactions: 0,
        cleaned_staging_directories: 0,
    };
    for entry in fs::read_dir(&temporary_root).map_err(import_io_error)? {
        let entry = entry.map_err(import_io_error)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(IMPORT_TRANSACTION_PREFIX) {
            continue;
        }
        let transaction_root = entry.path();
        validate_import_transaction_directory(repository_root, &transaction_root)?;
        let journal_path = transaction_root.join(IMPORT_TRANSACTION_JOURNAL);
        if !safe_transaction_regular_file(&journal_path)? {
            if safe_transaction_regular_file(&transaction_root.join(IMPORT_TRANSACTION_COMMITTING))?
                || safe_transaction_regular_file(
                    &transaction_root.join(IMPORT_TRANSACTION_COMMITTED),
                )?
            {
                return Err(TemplateApiError::from_code("TEMPLATE_PACK_RECOVERY_FAILED"));
            }
            remove_import_transaction_directory(repository_root, &transaction_root)?;
            recovery.cleaned_staging_directories += 1;
            continue;
        }
        let bytes = fs::read(&journal_path).map_err(import_io_error)?;
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(TemplateApiError::from_code("TEMPLATE_PACK_RECOVERY_FAILED"));
        }
        let journal: ImportTransactionJournal = serde_json::from_slice(&bytes)
            .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_RECOVERY_FAILED"))?;
        validate_transaction_journal(&transaction_root, &journal)?;
        if safe_transaction_regular_file(&transaction_root.join(IMPORT_TRANSACTION_COMMITTED))? {
            for operation in &journal.operations {
                if bounded_file_sha256(&repository_root.join(&operation.target_file_name))?
                    != operation.new_sha256
                {
                    return Err(TemplateApiError::from_code("TEMPLATE_PACK_RECOVERY_FAILED"));
                }
            }
            remove_import_transaction_directory(repository_root, &transaction_root)?;
            recovery.finalized_transactions += 1;
        } else {
            let was_committing = safe_transaction_regular_file(
                &transaction_root.join(IMPORT_TRANSACTION_COMMITTING),
            )?;
            rollback_import_transaction(repository_root, &transaction_root, &journal)?;
            remove_import_transaction_directory(repository_root, &transaction_root)?;
            if was_committing {
                recovery.rolled_back_transactions += 1;
            } else {
                recovery.cleaned_staging_directories += 1;
            }
        }
    }
    Ok(recovery)
}

fn validate_import_transaction_directory(
    repository_root: &Path,
    transaction_root: &Path,
) -> Result<(), TemplateApiError> {
    if transaction_root.parent() != Some(repository_root.join(".tmp").as_path()) {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_RECOVERY_FAILED"));
    }
    let name = transaction_root
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| TemplateApiError::from_code("TEMPLATE_PACK_RECOVERY_FAILED"))?;
    let execution_id = name
        .strip_prefix(IMPORT_TRANSACTION_PREFIX)
        .ok_or_else(|| TemplateApiError::from_code("TEMPLATE_PACK_RECOVERY_FAILED"))?;
    if !valid_plan_token(execution_id) {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_RECOVERY_FAILED"));
    }
    let metadata = fs::symlink_metadata(transaction_root).map_err(import_io_error)?;
    if !metadata.is_dir() || is_reparse_metadata(&metadata) {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
    }
    Ok(())
}

fn validate_transaction_journal(
    transaction_root: &Path,
    journal: &ImportTransactionJournal,
) -> Result<(), TemplateApiError> {
    let directory_execution_id = transaction_root
        .file_name()
        .and_then(|value| value.to_str())
        .and_then(|value| value.strip_prefix(IMPORT_TRANSACTION_PREFIX));
    if journal.schema_version != 1
        || directory_execution_id != Some(journal.execution_id.as_str())
        || !valid_plan_token(&journal.execution_id)
        || journal.operations.is_empty()
        || journal.operations.len() > MAX_TEMPLATES
    {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_RECOVERY_FAILED"));
    }
    let mut targets = HashSet::new();
    let mut staging = HashSet::new();
    for (index, operation) in journal.operations.iter().enumerate() {
        let expected_staged = format!("{index:03}.new");
        let target = Path::new(&operation.target_file_name);
        let valid_target = target.components().count() == 1
            && target.extension().and_then(|value| value.to_str()) == Some("json")
            && target
                .file_stem()
                .and_then(|value| value.to_str())
                .is_some_and(is_safe_import_template_id);
        let hashes_valid = is_sha256_hex(&operation.new_sha256)
            && operation
                .original_sha256
                .as_deref()
                .is_none_or(is_sha256_hex);
        let operation_shape_valid = match operation.operation {
            ImportTransactionOperationKind::Create => {
                operation.backup_file_name.is_none() && operation.original_sha256.is_none()
            }
            ImportTransactionOperationKind::Replace => {
                operation.backup_file_name.as_deref() == Some(format!("{index:03}.old").as_str())
                    && operation.original_sha256.is_some()
            }
        };
        if !valid_target
            || operation.item_id.trim().is_empty()
            || operation.item_id.len() > 128
            || operation.staged_file_name != expected_staged
            || !hashes_valid
            || !operation_shape_valid
            || !targets.insert(operation.target_file_name.as_str())
            || !staging.insert(operation.staged_file_name.as_str())
        {
            return Err(TemplateApiError::from_code("TEMPLATE_PACK_RECOVERY_FAILED"));
        }
    }
    Ok(())
}

fn remove_import_transaction_directory(
    repository_root: &Path,
    transaction_root: &Path,
) -> Result<(), TemplateApiError> {
    validate_import_transaction_directory(repository_root, transaction_root)?;
    fs::remove_dir_all(transaction_root).map_err(import_io_error)
}

fn write_new_synced(path: &Path, bytes: &[u8]) -> Result<(), TemplateApiError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(import_io_error)?;
    file.write_all(bytes).map_err(import_io_error)?;
    file.sync_all().map_err(import_io_error)?;
    drop(file);
    if fs::read(path).map_err(import_io_error)? != bytes {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_IMPORT_FAILED"));
    }
    Ok(())
}

fn write_transaction_marker(transaction_root: &Path, marker: &str) -> Result<(), TemplateApiError> {
    write_new_synced(&transaction_root.join(marker), b"1\n")?;
    sync_parent_directory(transaction_root)
}

fn optional_bounded_file_sha256(path: &Path) -> Result<Option<String>, TemplateApiError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if is_reparse_metadata(&metadata) || !metadata.is_file() {
                return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
            }
            Ok(Some(bounded_file_sha256(path)?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(import_io_error(error)),
    }
}

fn safe_transaction_regular_file(path: &Path) -> Result<bool, TemplateApiError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if is_reparse_metadata(&metadata) || !metadata.is_file() {
                return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(import_io_error(error)),
    }
}

fn bounded_file_sha256(path: &Path) -> Result<String, TemplateApiError> {
    let metadata = fs::symlink_metadata(path).map_err(import_io_error)?;
    if is_reparse_metadata(&metadata)
        || !metadata.is_file()
        || metadata.len() > MAX_TEMPLATE_BYTES as u64
    {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_RECOVERY_FAILED"));
    }
    Ok(sha256_hex(&fs::read(path).map_err(import_io_error)?))
}

fn is_safe_import_template_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    let reserved = ["con", "prn", "aux", "nul", "clock$"];
    let numbered_reserved = value.len() == 4
        && (value.starts_with("com") || value.starts_with("lpt"))
        && matches!(value.as_bytes()[3], b'1'..=b'9');
    (3..=80).contains(&bytes.len())
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_' || *byte == b'-'
        })
        && !reserved.contains(&value)
        && !numbered_reserved
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn import_io_error(error: std::io::Error) -> TemplateApiError {
    if error.raw_os_error() == Some(112) || error.raw_os_error() == Some(28) {
        TemplateApiError::from_code("TEMPLATE_DISK_FULL")
    } else {
        TemplateApiError::from_code("TEMPLATE_PACK_IMPORT_FAILED")
    }
}

fn validate_import_source(path: &str) -> Result<PathBuf, TemplateApiError> {
    if path.trim().is_empty() {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_IMPORT_FAILED"));
    }
    let source = PathBuf::from(path);
    if !source
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(PORTABLE_PACK_EXTENSION))
    {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_INVALID"));
    }
    let parent = source
        .parent()
        .ok_or_else(|| TemplateApiError::from_code("TEMPLATE_PACK_IMPORT_FAILED"))?;
    for ancestor in parent.ancestors() {
        let metadata = fs::symlink_metadata(ancestor)
            .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_IMPORT_FAILED"))?;
        if is_reparse_metadata(&metadata) {
            return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
        }
    }
    let metadata = fs::symlink_metadata(&source)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_IMPORT_FAILED"))?;
    if is_reparse_metadata(&metadata) || !metadata.is_file() {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
    }
    if metadata.len() > MAX_ARCHIVE_BYTES as u64 {
        return Err(budget_error());
    }
    Ok(source)
}

fn read_archive_bounded(path: &Path) -> Result<Vec<u8>, TemplateApiError> {
    let file =
        File::open(path).map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_IMPORT_FAILED"))?;
    let metadata = file
        .metadata()
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_IMPORT_FAILED"))?;
    if !metadata.is_file() || metadata.len() > MAX_ARCHIVE_BYTES as u64 {
        return Err(budget_error());
    }
    let mut bytes = Vec::with_capacity((metadata.len() as usize).min(MAX_ARCHIVE_BYTES));
    file.take(MAX_ARCHIVE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_IMPORT_FAILED"))?;
    if bytes.len() > MAX_ARCHIVE_BYTES {
        return Err(budget_error());
    }
    if bytes.len() < 22 || !bytes.starts_with(b"PK\x03\x04") {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_INVALID"));
    }
    Ok(bytes)
}

fn validate_classic_single_disk_zip(bytes: &[u8]) -> Result<(), TemplateApiError> {
    let eocd = bytes
        .windows(4)
        .rposition(|window| window == b"PK\x05\x06")
        .ok_or_else(|| TemplateApiError::from_code("TEMPLATE_PACK_INVALID"))?;
    if eocd + 22 > bytes.len() {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_INVALID"));
    }
    let zip64_probe_start = eocd.saturating_sub(76);
    if bytes[zip64_probe_start..eocd]
        .windows(4)
        .any(|window| window == b"PK\x06\x06" || window == b"PK\x06\x07")
    {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
    }
    let u16_at =
        |offset: usize| u16::from_le_bytes([bytes[eocd + offset], bytes[eocd + offset + 1]]);
    let u32_at = |offset: usize| {
        u32::from_le_bytes([
            bytes[eocd + offset],
            bytes[eocd + offset + 1],
            bytes[eocd + offset + 2],
            bytes[eocd + offset + 3],
        ])
    };
    let disk_number = u16_at(4);
    let central_disk = u16_at(6);
    let entries_on_disk = u16_at(8);
    let total_entries = u16_at(10);
    let central_size = u32_at(12);
    let central_offset = u32_at(16);
    let comment_length = u16_at(20) as usize;
    if eocd + 22 + comment_length != bytes.len() {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_INVALID"));
    }
    if disk_number != 0
        || central_disk != 0
        || entries_on_disk != total_entries
        || total_entries == u16::MAX
        || central_size == u32::MAX
        || central_offset == u32::MAX
    {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
    }
    if total_entries == 0 || total_entries as usize > MAX_ARCHIVE_ENTRIES {
        return Err(budget_error());
    }
    let central_end = (central_offset as u64)
        .checked_add(central_size as u64)
        .ok_or_else(budget_error)?;
    if central_end > eocd as u64 {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_INVALID"));
    }
    validate_raw_central_directory(
        bytes,
        central_offset as usize,
        central_end as usize,
        total_entries as usize,
    )?;
    Ok(())
}

fn validate_raw_central_directory(
    bytes: &[u8],
    start: usize,
    end: usize,
    expected_entries: usize,
) -> Result<(), TemplateApiError> {
    if start > end || end > bytes.len() {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_INVALID"));
    }
    let mut cursor = start;
    let mut count = 0usize;
    let mut raw_names = HashSet::new();
    let mut normalized_names = HashSet::new();
    while cursor < end {
        if cursor + 46 > end || &bytes[cursor..cursor + 4] != b"PK\x01\x02" {
            return Err(TemplateApiError::from_code("TEMPLATE_PACK_INVALID"));
        }
        let u16_at = |offset: usize| {
            u16::from_le_bytes([bytes[cursor + offset], bytes[cursor + offset + 1]]) as usize
        };
        let u32_at = |offset: usize| {
            u32::from_le_bytes([
                bytes[cursor + offset],
                bytes[cursor + offset + 1],
                bytes[cursor + offset + 2],
                bytes[cursor + offset + 3],
            ])
        };
        let flags = u16_at(8) as u16;
        let compression = u16_at(10) as u16;
        let name_length = u16_at(28);
        let extra_length = u16_at(30);
        let comment_length = u16_at(32);
        let external_attributes = u32_at(38);
        let local_header_offset = u32_at(42) as usize;
        let record_end = cursor
            .checked_add(46)
            .and_then(|value| value.checked_add(name_length))
            .and_then(|value| value.checked_add(extra_length))
            .and_then(|value| value.checked_add(comment_length))
            .ok_or_else(budget_error)?;
        if record_end > end {
            return Err(TemplateApiError::from_code("TEMPLATE_PACK_INVALID"));
        }
        if flags & 1 != 0
            || !matches!(compression, 0 | 8)
            || extra_length != 0
            || comment_length != 0
        {
            return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
        }
        let unix_file_type = (external_attributes >> 16) & 0o170000;
        if unix_file_type != 0 && unix_file_type != 0o100000 {
            return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
        }
        let raw_name = &bytes[cursor + 46..cursor + 46 + name_length];
        let name = std::str::from_utf8(raw_name)
            .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"))?;
        let normalized = name.nfc().collect::<String>().to_lowercase();
        if !raw_names.insert(raw_name.to_vec()) || !normalized_names.insert(normalized) {
            return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
        }
        validate_archive_entry_name(name)?;
        validate_matching_local_header(
            bytes,
            local_header_offset,
            start,
            raw_name,
            flags,
            compression,
        )?;
        count = count.checked_add(1).ok_or_else(budget_error)?;
        cursor = record_end;
    }
    if cursor != end || count != expected_entries {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_INVALID"));
    }
    Ok(())
}

fn validate_matching_local_header(
    bytes: &[u8],
    offset: usize,
    central_start: usize,
    expected_name: &[u8],
    expected_flags: u16,
    expected_compression: u16,
) -> Result<(), TemplateApiError> {
    if offset.checked_add(30).is_none_or(|end| end > central_start)
        || &bytes[offset..offset + 4] != b"PK\x03\x04"
    {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_INVALID"));
    }
    let u16_at = |field_offset: usize| {
        u16::from_le_bytes([
            bytes[offset + field_offset],
            bytes[offset + field_offset + 1],
        ])
    };
    let local_flags = u16_at(6);
    let local_compression = u16_at(8);
    let name_length = u16_at(26) as usize;
    let extra_length = u16_at(28) as usize;
    let header_end = offset
        .checked_add(30)
        .and_then(|value| value.checked_add(name_length))
        .and_then(|value| value.checked_add(extra_length))
        .ok_or_else(budget_error)?;
    if header_end > central_start
        || local_flags != expected_flags
        || local_compression != expected_compression
        || extra_length != 0
        || &bytes[offset + 30..offset + 30 + name_length] != expected_name
    {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct ScannedZipEntry {
    index: usize,
    name: String,
    size: usize,
}

fn inspect_portable_archive(
    bytes: &[u8],
) -> Result<(PortablePackManifest, Vec<ResolvedImportTemplate>, usize), TemplateApiError> {
    validate_classic_single_disk_zip(bytes)?;
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_INVALID"))?;
    if archive.len() == 0 || archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(budget_error());
    }
    let mut scanned = Vec::with_capacity(archive.len());
    let mut raw_names = HashSet::new();
    let mut normalized_names = HashSet::new();
    let mut total_metadata_bytes = 0usize;
    for index in 0..archive.len() {
        let entry = archive
            .by_index_raw(index)
            .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_INVALID"))?;
        let raw_name = std::str::from_utf8(entry.name_raw())
            .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"))?;
        validate_archive_entry_name(raw_name)?;
        let normalized = raw_name.nfc().collect::<String>().to_lowercase();
        if !raw_names.insert(raw_name.to_owned()) || !normalized_names.insert(normalized) {
            return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
        }
        if entry.encrypted()
            || entry.is_dir()
            || entry.is_symlink()
            || !entry.comment().is_empty()
            || entry.extra_data().is_some_and(|data| !data.is_empty())
            || !matches!(
                entry.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            )
        {
            return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
        }
        if let Some(mode) = entry.unix_mode() {
            let file_type = mode & 0o170000;
            if file_type != 0 && file_type != 0o100000 {
                return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
            }
        }
        let size = usize::try_from(entry.size()).map_err(|_| budget_error())?;
        let compressed_size = entry.compressed_size();
        let limit = if raw_name == "manifest.json" {
            MAX_MANIFEST_BYTES
        } else {
            MAX_TEMPLATE_BYTES
        };
        if size == 0 || size > limit {
            return Err(budget_error());
        }
        if entry.compression() == CompressionMethod::Stored && compressed_size != entry.size() {
            return Err(TemplateApiError::from_code(
                "TEMPLATE_PACK_INTEGRITY_FAILED",
            ));
        }
        if compressed_size == 0
            || entry.size() > compressed_size.saturating_mul(MAX_COMPRESSION_RATIO)
        {
            return Err(budget_error());
        }
        total_metadata_bytes = total_metadata_bytes
            .checked_add(size)
            .ok_or_else(budget_error)?;
        if total_metadata_bytes > MAX_TOTAL_UNCOMPRESSED_BYTES {
            return Err(budget_error());
        }
        scanned.push(ScannedZipEntry {
            index,
            name: raw_name.to_owned(),
            size,
        });
    }
    if scanned.first().map(|entry| entry.name.as_str()) != Some("manifest.json") {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_INVALID"));
    }
    let manifest_bytes = read_scanned_entry(&mut archive, &scanned[0], MAX_MANIFEST_BYTES)?;
    let manifest = parse_import_manifest(&manifest_bytes)?;
    validate_manifest_entries(&manifest)?;
    if manifest.template_count != manifest.templates.len()
        || manifest.template_count + 1 != scanned.len()
    {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_INVALID"));
    }
    for (archive_entry, manifest_entry) in scanned.iter().skip(1).zip(&manifest.templates) {
        if archive_entry.name != manifest_entry.path {
            return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
        }
    }

    let mut resolved = Vec::with_capacity(manifest.templates.len());
    let mut actual_total = manifest_bytes.len();
    for (scanned_entry, manifest_entry) in scanned.iter().skip(1).zip(&manifest.templates) {
        let template_bytes = read_scanned_entry(&mut archive, scanned_entry, MAX_TEMPLATE_BYTES)?;
        actual_total = actual_total
            .checked_add(template_bytes.len())
            .ok_or_else(budget_error)?;
        if actual_total > MAX_TOTAL_UNCOMPRESSED_BYTES {
            return Err(budget_error());
        }
        let resolved_template = validate_import_template(manifest_entry, template_bytes)?;
        resolved.push(resolved_template);
    }
    if actual_total != total_metadata_bytes {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_INTEGRITY_FAILED",
        ));
    }
    Ok((manifest, resolved, actual_total))
}

fn validate_archive_entry_name(name: &str) -> Result<(), TemplateApiError> {
    if name.is_empty()
        || name.len() > 255
        || name.starts_with('/')
        || name.starts_with('\\')
        || name.ends_with('/')
        || name.contains('\\')
        || name.contains(':')
        || name
            .chars()
            .any(|character| character == '\0' || character.is_control())
    {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
    }
    let segments: Vec<_> = name.split('/').collect();
    if segments.iter().any(|segment| {
        segment.is_empty()
            || *segment == "."
            || *segment == ".."
            || segment.ends_with(' ')
            || segment.ends_with('.')
    }) {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
    }
    if name != "manifest.json" {
        if !(segments.len() == 2 && segments[0] == "templates" && segments[1].ends_with(".json")) {
            return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
        }
        let id = &segments[1][..segments[1].len() - ".json".len()];
        let id_bytes = id.as_bytes();
        if !(3..=80).contains(&id_bytes.len())
            || !id_bytes
                .first()
                .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            || !id_bytes
                .last()
                .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            || !id_bytes.iter().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_' || *byte == b'-'
            })
        {
            return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
        }
    }
    Ok(())
}

fn read_scanned_entry(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    scanned: &ScannedZipEntry,
    limit: usize,
) -> Result<Vec<u8>, TemplateApiError> {
    let entry = archive
        .by_index(scanned.index)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_INTEGRITY_FAILED"))?;
    let mut actual = Vec::with_capacity(scanned.size.min(limit));
    entry
        .take(limit as u64 + 1)
        .read_to_end(&mut actual)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_INTEGRITY_FAILED"))?;
    if actual.len() > limit {
        return Err(budget_error());
    }
    if actual.len() != scanned.size {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_INTEGRITY_FAILED",
        ));
    }
    Ok(actual)
}

fn parse_import_manifest(bytes: &[u8]) -> Result<PortablePackManifest, TemplateApiError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_INVALID"))?;
    if value.get("package_schema_version").and_then(Value::as_u64) != Some(1) {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_VERSION_UNSUPPORTED",
        ));
    }
    let validator = PORTABLE_PACK_MANIFEST_VALIDATOR
        .as_ref()
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_IMPORT_FAILED"))?;
    if !validator.is_valid(&value) {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_INVALID"));
    }
    if contains_sensitive_content(&value) {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_SENSITIVE_CONTENT",
        ));
    }
    serde_json::from_value(value).map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_INVALID"))
}

fn validate_manifest_entries(manifest: &PortablePackManifest) -> Result<(), TemplateApiError> {
    if manifest.templates.is_empty() || manifest.templates.len() > MAX_TEMPLATES {
        return Err(budget_error());
    }
    let mut previous_id: Option<&str> = None;
    let mut ids = HashSet::new();
    let mut paths = HashSet::new();
    for entry in &manifest.templates {
        if !ids.insert(entry.id.as_str())
            || !paths.insert(entry.path.as_str())
            || entry.path != format!("templates/{}.json", entry.id)
            || previous_id.is_some_and(|previous| previous >= entry.id.as_str())
        {
            return Err(TemplateApiError::from_code("TEMPLATE_PACK_INVALID"));
        }
        previous_id = Some(&entry.id);
    }
    Ok(())
}

fn validate_import_template(
    manifest: &PortablePackManifestEntry,
    bytes: Vec<u8>,
) -> Result<ResolvedImportTemplate, TemplateApiError> {
    if bytes.len() != manifest.byte_size || sha256_hex(&bytes) != manifest.file_sha256 {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_INTEGRITY_FAILED",
        ));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_INVALID"))?;
    let value: Value = serde_json::from_str(text)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_INVALID"))?;
    if value.get("schema_version").and_then(Value::as_u64) != Some(2) {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_VERSION_UNSUPPORTED",
        ));
    }
    if contains_sensitive_content(&value) {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_SENSITIVE_CONTENT",
        ));
    }
    let template: TemplateV2 = parse_and_validate_template_v2(text)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_INVALID"))?;
    if template.id != manifest.id || template.version != manifest.version {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_INTEGRITY_FAILED",
        ));
    }
    let actual_semantic = semantic_sha256(&template)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_INVALID"))?;
    if actual_semantic != manifest.semantic_sha256 {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_INTEGRITY_FAILED",
        ));
    }
    Ok(ResolvedImportTemplate {
        preview: PortablePackImportTemplate {
            fingerprint: PortablePackTemplateFingerprint {
                id: template.id.clone(),
                version: template.version,
                file_sha256: manifest.file_sha256.clone(),
                semantic_sha256: actual_semantic,
            },
            name: template.name.clone(),
            byte_size: bytes.len(),
        },
        template,
        bytes,
    })
}

fn resolve_import_conflicts(
    service: &TemplateService,
    templates: &[ResolvedImportTemplate],
) -> Result<Vec<PortablePackImportItem>, TemplateApiError> {
    let listed = service
        .repository()
        .list()
        .map_err(TemplateApiError::from)?;
    let existing_by_id: HashMap<_, _> = listed
        .templates
        .into_iter()
        .map(|template| (template.id.clone(), template))
        .collect();
    templates
        .iter()
        .map(|resolved| {
            let imported = &resolved.preview.fingerprint;
            let (conflict_kind, allowed_strategies, existing) =
                match existing_by_id.get(&imported.id) {
                    None => (PortablePackConflictKind::None, Vec::new(), None),
                    Some(item) if !item.valid || item.semantic_sha256.is_none() => {
                        return Err(TemplateApiError::from_code(
                            "TEMPLATE_PACK_CONFLICT_UNRESOLVED",
                        )
                        .with_param("templateId", imported.id.as_str()));
                    }
                    Some(item) => {
                        let fingerprint = PortablePackTemplateFingerprint {
                            id: item.id.clone(),
                            version: item.version,
                            file_sha256: item.file_sha256.clone(),
                            semantic_sha256: item.semantic_sha256.clone().unwrap_or_default(),
                        };
                        if item.origin == TemplateOrigin::Custom {
                            (
                                PortablePackConflictKind::Custom,
                                vec![
                                    PortablePackConflictStrategy::Skip,
                                    PortablePackConflictStrategy::KeepBoth,
                                    PortablePackConflictStrategy::ReplaceCustom,
                                ],
                                Some(fingerprint),
                            )
                        } else {
                            (
                                PortablePackConflictKind::Readonly,
                                vec![
                                    PortablePackConflictStrategy::Skip,
                                    PortablePackConflictStrategy::KeepBoth,
                                ],
                                Some(fingerprint),
                            )
                        }
                    }
                };
            Ok(PortablePackImportItem {
                item_id: format!("item_{}", Uuid::new_v4().simple()),
                template: resolved.preview.clone(),
                conflict_kind,
                allowed_strategies,
                existing,
            })
        })
        .collect()
}

fn validate_destination(path: &str, overwrite: bool) -> Result<PathBuf, TemplateApiError> {
    if path.trim().is_empty() {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_EXPORT_FAILED"));
    }
    let destination = PathBuf::from(path);
    if !destination
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(PORTABLE_PACK_EXTENSION))
    {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_EXPORT_FAILED"));
    }
    let parent = destination
        .parent()
        .filter(|parent| parent.is_dir())
        .ok_or_else(|| TemplateApiError::from_code("TEMPLATE_PACK_EXPORT_FAILED"))?;
    reject_reparse_ancestors(parent)?;
    if let Ok(metadata) = fs::symlink_metadata(&destination) {
        if is_reparse_metadata(&metadata) || !metadata.is_file() {
            return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
        }
        if !overwrite {
            return Err(
                TemplateApiError::from_code("TEMPLATE_PACK_EXPORT_FAILED").with_param(
                    "fileName",
                    destination
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("templates.meetily-template-pack"),
                ),
            );
        }
    }
    Ok(destination)
}

fn persist_archive_atomically(
    destination: &Path,
    bytes: &[u8],
    overwrite: bool,
) -> Result<(), TemplateApiError> {
    let parent = destination
        .parent()
        .ok_or_else(|| TemplateApiError::from_code("TEMPLATE_PACK_EXPORT_FAILED"))?;
    reject_reparse_ancestors(parent)?;
    if let Ok(metadata) = fs::symlink_metadata(destination) {
        if is_reparse_metadata(&metadata) || !metadata.is_file() {
            return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
        }
    }
    let mut staging = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_EXPORT_FAILED"))?;
    staging
        .write_all(bytes)
        .and_then(|_| staging.as_file().sync_all())
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_EXPORT_FAILED"))?;
    let staged = fs::read(staging.path())
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_EXPORT_FAILED"))?;
    if staged != bytes || sha256_hex(&staged) != sha256_hex(bytes) {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_INTEGRITY_FAILED",
        ));
    }
    if overwrite {
        staging
            .persist(destination)
            .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_EXPORT_FAILED"))?;
    } else {
        staging
            .persist_noclobber(destination)
            .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_EXPORT_FAILED"))?;
    }
    let persisted = fs::read(destination)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_EXPORT_FAILED"))?;
    if persisted != bytes || sha256_hex(&persisted) != sha256_hex(bytes) {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_INTEGRITY_FAILED",
        ));
    }
    sync_parent_directory(parent)?;
    Ok(())
}

fn reject_reparse_ancestors(path: &Path) -> Result<(), TemplateApiError> {
    for ancestor in path.ancestors() {
        let metadata = fs::symlink_metadata(ancestor)
            .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_EXPORT_FAILED"))?;
        if is_reparse_metadata(&metadata) {
            return Err(TemplateApiError::from_code("TEMPLATE_PACK_UNSAFE_ENTRY"));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse_metadata(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_metadata(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> Result<(), TemplateApiError> {
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_PACK_EXPORT_FAILED"))
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> Result<(), TemplateApiError> {
    Ok(())
}

fn store_export_plan(token: String, plan: PortablePackExportPlan) {
    let mut plans = EXPORT_PLANS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    plans.retain(|_, item| item.created.elapsed() <= EXPORT_PLAN_TTL);
    if plans.len() >= MAX_EXPORT_PLANS {
        if let Some(oldest) = plans
            .iter()
            .min_by_key(|(_, item)| item.created)
            .map(|(token, _)| token.clone())
        {
            plans.remove(&oldest);
        }
    }
    plans.insert(token, plan);
}

fn take_export_plan(token: &str) -> Result<PortablePackExportPlan, TemplateApiError> {
    if token.trim().is_empty() || token.len() > 128 {
        return Err(TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"));
    }
    let mut plans = EXPORT_PLANS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    plans.retain(|_, item| item.created.elapsed() <= EXPORT_PLAN_TTL);
    plans
        .remove(token)
        .ok_or_else(|| TemplateApiError::from_code("TEMPLATE_PACK_PLAN_STALE"))
}

fn store_import_plan(token: String, plan: PortablePackImportPlan) {
    let mut plans = IMPORT_PLANS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let expired_tokens: Vec<_> = plans
        .iter()
        .filter(|(_, item)| item.created.elapsed() > EXPORT_PLAN_TTL)
        .map(|(token, _)| token.clone())
        .collect();
    plans.retain(|_, item| item.created.elapsed() <= EXPORT_PLAN_TTL);
    if plans.len() >= MAX_EXPORT_PLANS {
        if let Some(oldest) = plans
            .iter()
            .min_by_key(|(_, item)| item.created)
            .map(|(token, _)| token.clone())
        {
            plans.remove(&oldest);
        }
    }
    plans.insert(token, plan);
    drop(plans);
    for expired_token in expired_tokens {
        record_import_plan_tombstone(expired_token, PortablePackImportPlanTombstoneKind::Expired);
    }
}

fn load_import_plan(token: &str) -> Result<PortablePackImportPlan, TemplateApiError> {
    if !valid_plan_token(token) {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_PREVIEW_PLAN_NOT_FOUND",
        ));
    }
    let mut plans = IMPORT_PLANS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(plan) = plans.get(token).cloned() {
        if plan.created.elapsed() <= EXPORT_PLAN_TTL {
            return Ok(plan);
        }
        plans.remove(token);
        drop(plans);
        record_import_plan_tombstone(
            token.to_owned(),
            PortablePackImportPlanTombstoneKind::Expired,
        );
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_PREVIEW_PLAN_EXPIRED",
        ));
    }
    drop(plans);
    Err(import_plan_lookup_error(token))
}

fn consume_import_plan(token: &str) -> Result<(), TemplateApiError> {
    if !valid_plan_token(token) {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_PREVIEW_PLAN_NOT_FOUND",
        ));
    }
    let mut plans = IMPORT_PLANS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(plan) = plans.remove(token) else {
        drop(plans);
        return Err(import_plan_lookup_error(token));
    };
    drop(plans);
    if plan.created.elapsed() > EXPORT_PLAN_TTL {
        record_import_plan_tombstone(
            token.to_owned(),
            PortablePackImportPlanTombstoneKind::Expired,
        );
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_PREVIEW_PLAN_EXPIRED",
        ));
    }
    record_import_plan_tombstone(
        token.to_owned(),
        PortablePackImportPlanTombstoneKind::Consumed,
    );
    Ok(())
}

fn valid_plan_token(token: &str) -> bool {
    !token.trim().is_empty() && token.len() <= 128 && Uuid::parse_str(token).is_ok()
}

fn import_plan_lookup_error(token: &str) -> TemplateApiError {
    let mut tombstones = IMPORT_PLAN_TOMBSTONES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    tombstones.retain(|_, tombstone| tombstone.created.elapsed() <= EXPORT_PLAN_TTL);
    match tombstones.get(token).map(|tombstone| tombstone.kind) {
        Some(PortablePackImportPlanTombstoneKind::Consumed) => {
            TemplateApiError::from_code("TEMPLATE_PACK_PREVIEW_PLAN_CONSUMED")
        }
        Some(PortablePackImportPlanTombstoneKind::Expired) => {
            TemplateApiError::from_code("TEMPLATE_PACK_PREVIEW_PLAN_EXPIRED")
        }
        None => TemplateApiError::from_code("TEMPLATE_PACK_PREVIEW_PLAN_NOT_FOUND"),
    }
}

fn record_import_plan_tombstone(token: String, kind: PortablePackImportPlanTombstoneKind) {
    let mut tombstones = IMPORT_PLAN_TOMBSTONES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    tombstones.retain(|_, tombstone| tombstone.created.elapsed() <= EXPORT_PLAN_TTL);
    if tombstones.len() >= MAX_EXPORT_PLANS {
        if let Some(oldest) = tombstones
            .iter()
            .min_by_key(|(_, tombstone)| tombstone.created)
            .map(|(token, _)| token.clone())
        {
            tombstones.remove(&oldest);
        }
    }
    tombstones.insert(
        token,
        PortablePackImportPlanTombstone {
            created: Instant::now(),
            kind,
        },
    );
}

fn store_import_execution_plan(token: String, plan: PortablePackExecutionPlan) {
    let mut plans = IMPORT_EXECUTION_PLANS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let expired_tokens: Vec<_> = plans
        .iter()
        .filter(|(_, item)| item.created.elapsed() > EXPORT_PLAN_TTL)
        .map(|(token, _)| token.clone())
        .collect();
    plans.retain(|_, item| item.created.elapsed() <= EXPORT_PLAN_TTL);
    if plans.len() >= MAX_EXPORT_PLANS {
        if let Some(oldest) = plans
            .iter()
            .min_by_key(|(_, item)| item.created)
            .map(|(token, _)| token.clone())
        {
            plans.remove(&oldest);
        }
    }
    plans.insert(token, plan);
    drop(plans);
    for expired_token in expired_tokens {
        record_execution_plan_tombstone(
            expired_token,
            PortablePackImportPlanTombstoneKind::Expired,
        );
    }
}

fn take_import_execution_plan(token: &str) -> Result<PortablePackExecutionPlan, TemplateApiError> {
    if !valid_plan_token(token) {
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_EXECUTION_PLAN_NOT_FOUND",
        ));
    }
    let mut plans = IMPORT_EXECUTION_PLANS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(plan) = plans.remove(token) else {
        drop(plans);
        return Err(execution_plan_lookup_error(token));
    };
    drop(plans);
    if plan.created.elapsed() > EXPORT_PLAN_TTL {
        record_execution_plan_tombstone(
            token.to_owned(),
            PortablePackImportPlanTombstoneKind::Expired,
        );
        return Err(TemplateApiError::from_code(
            "TEMPLATE_PACK_EXECUTION_PLAN_EXPIRED",
        ));
    }
    record_execution_plan_tombstone(
        token.to_owned(),
        PortablePackImportPlanTombstoneKind::Consumed,
    );
    Ok(plan)
}

fn execution_plan_lookup_error(token: &str) -> TemplateApiError {
    let mut tombstones = IMPORT_EXECUTION_PLAN_TOMBSTONES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    tombstones.retain(|_, tombstone| tombstone.created.elapsed() <= EXPORT_PLAN_TTL);
    match tombstones.get(token).map(|tombstone| tombstone.kind) {
        Some(PortablePackImportPlanTombstoneKind::Consumed) => {
            TemplateApiError::from_code("TEMPLATE_PACK_EXECUTION_PLAN_CONSUMED")
        }
        Some(PortablePackImportPlanTombstoneKind::Expired) => {
            TemplateApiError::from_code("TEMPLATE_PACK_EXECUTION_PLAN_EXPIRED")
        }
        None => TemplateApiError::from_code("TEMPLATE_PACK_EXECUTION_PLAN_NOT_FOUND"),
    }
}

fn record_execution_plan_tombstone(token: String, kind: PortablePackImportPlanTombstoneKind) {
    let mut tombstones = IMPORT_EXECUTION_PLAN_TOMBSTONES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    tombstones.retain(|_, tombstone| tombstone.created.elapsed() <= EXPORT_PLAN_TTL);
    if tombstones.len() >= MAX_EXPORT_PLANS {
        if let Some(oldest) = tombstones
            .iter()
            .min_by_key(|(_, tombstone)| tombstone.created)
            .map(|(token, _)| token.clone())
        {
            tombstones.remove(&oldest);
        }
    }
    tombstones.insert(
        token,
        PortablePackImportPlanTombstone {
            created: Instant::now(),
            kind,
        },
    );
}

fn budget_error() -> TemplateApiError {
    TemplateApiError::from_code("TEMPLATE_PACK_BUDGET_EXCEEDED")
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summary::templates::{
        CreateConflictPolicy, EmptyBehavior, TemplateFormat, TemplateRepository, TemplateSectionV2,
        TemplateSource, TemplateSourceType, TemplateV2,
    };
    use chrono::{FixedOffset, TimeZone};
    use tempfile::TempDir;

    fn test_service() -> (TempDir, TemplateService) {
        let temporary = TempDir::new().unwrap();
        let repository = TemplateRepository::new(temporary.path().join("templates"), None).unwrap();
        (temporary, TemplateService::new(repository))
    }

    fn template(id: &str, instruction: &str) -> TemplateV2 {
        let now = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 8, 24, 0, 0, 0)
            .unwrap();
        TemplateV2 {
            schema_version: 2,
            id: id.to_owned(),
            name: format!("Template {id}"),
            description: "Portable package test".to_owned(),
            version: 1,
            locale: Some("en".to_owned()),
            tags: vec!["test".to_owned()],
            source: TemplateSource {
                source_type: TemplateSourceType::Manual,
                original_file_name: None,
                original_file_sha256: None,
                imported_at: None,
                copied_from_template_id: None,
            },
            created_at: now,
            updated_at: now,
            sections: vec![TemplateSectionV2 {
                id: "summary".to_owned(),
                title: "Summary".to_owned(),
                instruction: instruction.to_owned(),
                format: TemplateFormat::Paragraph,
                item_format: None,
                example_item_format: None,
                required: true,
                empty_behavior: EmptyBehavior::ShowNotMentioned,
            }],
            extensions: Default::default(),
        }
    }

    fn create(service: &TemplateService, id: &str, instruction: &str) {
        service
            .repository()
            .create(template(id, instruction), CreateConflictPolicy::Error)
            .unwrap();
    }

    fn portable_archive_for(service: &TemplateService, ids: &[&str]) -> Vec<u8> {
        let ids: Vec<_> = ids.iter().map(|id| (*id).to_owned()).collect();
        let resolved = resolve_export_templates(service, &ids).unwrap();
        build_portable_archive(
            &resolved,
            "pack_0123456789abcdef0123456789abcdef",
            "2026-08-24T00:00:00.000Z",
        )
        .unwrap()
    }

    fn write_pack(directory: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = directory.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn arbitrary_zip(entries: &[(&str, &[u8])], method: CompressionMethod) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default()
            .compression_method(method)
            .last_modified_time(zip::DateTime::default())
            .unix_permissions(0o600);
        for (name, bytes) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn rewrite_archive_entry(original: &[u8], target: &str, replacement: &[u8]) -> Vec<u8> {
        let mut input = ZipArchive::new(Cursor::new(original)).unwrap();
        let mut entries = Vec::new();
        for index in 0..input.len() {
            let mut entry = input.by_index(index).unwrap();
            let name = entry.name().to_owned();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            if name == target {
                bytes = replacement.to_vec();
            }
            entries.push((name, bytes));
        }
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .last_modified_time(zip::DateTime::default())
            .unix_permissions(0o600);
        for (name, bytes) in entries {
            writer.start_file(name, options).unwrap();
            writer.write_all(&bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn preview_sorts_templates_and_returns_exact_repository_hashes() {
        let (_temporary, service) = test_service();
        create(&service, "template_b", "Summarize the transcript");
        create(&service, "template_a", "Summarize the meeting");
        let response = preview_template_pack_export(
            &service,
            PreviewTemplatePackExportRequest {
                template_ids: vec!["template_b".to_owned(), "template_a".to_owned()],
            },
        )
        .unwrap();
        assert_eq!(response.template_count, 2);
        assert_eq!(response.templates[0].fingerprint.id, "template_a");
        let record = service
            .repository()
            .get("template_a", Some(TemplateOrigin::Custom))
            .unwrap();
        assert_eq!(
            response.templates[0].fingerprint.file_sha256,
            record.file_sha256
        );
        assert_eq!(
            response.templates[0].fingerprint.semantic_sha256,
            record.semantic_sha256
        );
    }

    #[test]
    fn preview_rejects_empty_duplicate_and_readonly_selections() {
        let (_temporary, service) = test_service();
        create(&service, "custom_one", "Summarize");
        assert_eq!(
            preview_template_pack_export(
                &service,
                PreviewTemplatePackExportRequest {
                    template_ids: vec![]
                }
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_BUDGET_EXCEEDED"
        );
        assert_eq!(
            preview_template_pack_export(
                &service,
                PreviewTemplatePackExportRequest {
                    template_ids: vec!["custom_one".to_owned(), "custom_one".to_owned()]
                }
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_INVALID"
        );
        assert_eq!(
            preview_template_pack_export(
                &service,
                PreviewTemplatePackExportRequest {
                    template_ids: vec!["daily_standup".to_owned()]
                }
            )
            .unwrap_err()
            .code,
            "TEMPLATE_READ_ONLY"
        );
    }

    #[test]
    fn preview_rejects_more_than_two_hundred_template_ids_before_repository_reads() {
        let (_temporary, service) = test_service();
        let template_ids = (0..=MAX_TEMPLATES)
            .map(|index| format!("template_{index:03}"))
            .collect();
        assert_eq!(
            preview_template_pack_export(
                &service,
                PreviewTemplatePackExportRequest { template_ids }
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_BUDGET_EXCEEDED"
        );
    }

    #[test]
    fn preview_accepts_exactly_two_hundred_small_custom_templates() {
        let (_temporary, service) = test_service();
        let template_ids: Vec<_> = (0..MAX_TEMPLATES)
            .map(|index| format!("template_{index:03}"))
            .collect();
        for id in &template_ids {
            create(&service, id, "Summarize");
        }
        let preview = preview_template_pack_export(
            &service,
            PreviewTemplatePackExportRequest { template_ids },
        )
        .unwrap();
        assert_eq!(preview.template_count, MAX_TEMPLATES);
        assert_eq!(preview.templates.len(), MAX_TEMPLATES);
    }

    #[test]
    fn privacy_scan_allows_security_words_but_rejects_real_secret_shapes() {
        let (_temporary, service) = test_service();
        create(
            &service,
            "safe_security_terms",
            "State whether the transcript exposed an API key, but never reproduce it",
        );
        assert!(preview_template_pack_export(
            &service,
            PreviewTemplatePackExportRequest {
                template_ids: vec!["safe_security_terms".to_owned()]
            }
        )
        .is_ok());

        create(
            &service,
            "unsafe_secret",
            "Use Authorization: Bearer abcdefghijklmnopqrstuvwxyz0123456789",
        );
        assert_eq!(
            preview_template_pack_export(
                &service,
                PreviewTemplatePackExportRequest {
                    template_ids: vec!["unsafe_secret".to_owned()]
                }
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_SENSITIVE_CONTENT"
        );
    }

    #[test]
    fn privacy_scan_rejects_sensitive_extension_keys_and_absolute_paths() {
        let (_temporary, service) = test_service();
        let mut sensitive_key = template("sensitive_key", "Summarize");
        sensitive_key
            .extensions
            .insert("api_key".to_owned(), Value::String("redacted".to_owned()));
        service
            .repository()
            .create(sensitive_key, CreateConflictPolicy::Error)
            .unwrap();
        assert_eq!(
            preview_template_pack_export(
                &service,
                PreviewTemplatePackExportRequest {
                    template_ids: vec!["sensitive_key".to_owned()]
                }
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_SENSITIVE_CONTENT"
        );

        create(
            &service,
            "absolute_path",
            r"Read C:\Users\alice\recording.wav before summarizing",
        );
        assert_eq!(
            preview_template_pack_export(
                &service,
                PreviewTemplatePackExportRequest {
                    template_ids: vec!["absolute_path".to_owned()]
                }
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_SENSITIVE_CONTENT"
        );
    }

    #[test]
    fn real_zip_has_manifest_first_then_sorted_exact_template_bytes() {
        let (_temporary, service) = test_service();
        create(&service, "zip_b", "B");
        create(&service, "zip_a", "A");
        let resolved =
            resolve_export_templates(&service, &["zip_b".to_owned(), "zip_a".to_owned()]).unwrap();
        let bytes = build_portable_archive(
            &resolved,
            "pack_0123456789abcdef0123456789abcdef",
            "2026-08-24T00:00:00.000Z",
        )
        .unwrap();
        let mut zip = ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert_eq!(zip.len(), 3);
        assert_eq!(zip.by_index(0).unwrap().name(), "manifest.json");
        assert_eq!(zip.by_index(1).unwrap().name(), "templates/zip_a.json");
        let mut exact = Vec::new();
        zip.by_index(1).unwrap().read_to_end(&mut exact).unwrap();
        assert_eq!(exact, resolved[0].bytes);
        assert_eq!(
            sha256_hex(&exact),
            resolved[0].preview.fingerprint.file_sha256
        );
    }

    #[test]
    fn manifest_matches_frozen_v1_contract_and_contains_no_machine_path() {
        let (temporary, service) = test_service();
        create(&service, "manifest_contract", "Summarize");
        let resolved =
            resolve_export_templates(&service, &["manifest_contract".to_owned()]).unwrap();
        let bytes = build_portable_archive(
            &resolved,
            "pack_0123456789abcdef0123456789abcdef",
            "2026-08-24T00:00:00.000Z",
        )
        .unwrap();
        let mut zip = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut manifest_bytes = Vec::new();
        zip.by_name("manifest.json")
            .unwrap()
            .read_to_end(&mut manifest_bytes)
            .unwrap();
        let manifest: Value = serde_json::from_slice(&manifest_bytes).unwrap();
        assert_eq!(manifest["package_schema_version"], 1);
        assert_eq!(manifest["package_type"], "meetily.template-pack");
        assert_eq!(manifest["hash_algorithm"], "sha256");
        assert_eq!(
            manifest["semantic_hash_algorithm"],
            "meetily-template-semantic-v1"
        );
        assert_eq!(manifest["templates"][0]["exported_origin"], "custom");
        assert_eq!(
            manifest["templates"][0]["path"],
            "templates/manifest_contract.json"
        );
        let serialized = String::from_utf8(manifest_bytes).unwrap();
        assert!(!serialized.contains(temporary.path().to_string_lossy().as_ref()));
        assert!(!serialized.contains("destination"));
    }

    #[test]
    fn archive_builder_rejects_tampered_bytes_and_oversized_template() {
        let (_temporary, service) = test_service();
        create(&service, "builder_guard", "Summarize");
        let mut resolved =
            resolve_export_templates(&service, &["builder_guard".to_owned()]).unwrap();
        resolved[0].bytes.push(b' ');
        assert_eq!(
            build_portable_archive(
                &resolved,
                "pack_0123456789abcdef0123456789abcdef",
                "2026-08-24T00:00:00.000Z"
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_INTEGRITY_FAILED"
        );

        resolved[0].bytes = vec![b' '; MAX_TEMPLATE_BYTES + 1];
        resolved[0].preview.byte_size = resolved[0].bytes.len();
        resolved[0].preview.fingerprint.file_sha256 = sha256_hex(&resolved[0].bytes);
        assert_eq!(
            build_portable_archive(
                &resolved,
                "pack_0123456789abcdef0123456789abcdef",
                "2026-08-24T00:00:00.000Z"
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_INTEGRITY_FAILED"
        );
    }

    #[test]
    fn archive_builder_enforces_thirty_two_mib_archive_budget() {
        let bytes = vec![b' '; MAX_TEMPLATE_BYTES];
        let file_sha256 = sha256_hex(&bytes);
        let templates: Vec<_> = (0..32)
            .map(|index| ResolvedExportTemplate {
                preview: PortablePackExportTemplate {
                    fingerprint: PortablePackTemplateFingerprint {
                        id: format!("large_{index:02}"),
                        version: 1,
                        file_sha256: file_sha256.clone(),
                        semantic_sha256: "0".repeat(64),
                    },
                    name: format!("Large {index}"),
                    byte_size: bytes.len(),
                    overrides_builtin: false,
                },
                bytes: bytes.clone(),
            })
            .collect();
        assert_eq!(
            build_portable_archive(
                &templates,
                "pack_0123456789abcdef0123456789abcdef",
                "2026-08-24T00:00:00.000Z"
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_BUDGET_EXCEEDED"
        );
    }

    #[test]
    fn transport_shapes_use_camel_case_and_reject_unknown_request_fields() {
        let request: PreviewTemplatePackExportRequest =
            serde_json::from_value(serde_json::json!({"templateIds": ["portable_shape"]})).unwrap();
        assert_eq!(request.template_ids, vec!["portable_shape"]);
        assert!(serde_json::from_value::<PreviewTemplatePackExportRequest>(
            serde_json::json!({"templateIds": ["portable_shape"], "extra": true})
        )
        .is_err());

        let response = PreviewTemplatePackExportResponse {
            plan_token: "token".to_owned(),
            templates: Vec::new(),
            template_count: 0,
            estimated_uncompressed_bytes: 0,
            warnings: Vec::new(),
        };
        let serialized = serde_json::to_value(response).unwrap();
        assert!(serialized.get("planToken").is_some());
        assert!(serialized.get("plan_token").is_none());
    }

    #[test]
    fn fixed_inputs_produce_byte_identical_archives() {
        let (_temporary, service) = test_service();
        create(&service, "stable_zip", "Stable");
        let resolved = resolve_export_templates(&service, &["stable_zip".to_owned()]).unwrap();
        let first = build_portable_archive(
            &resolved,
            "pack_0123456789abcdef0123456789abcdef",
            "2026-08-24T00:00:00.000Z",
        )
        .unwrap();
        let second = build_portable_archive(
            &resolved,
            "pack_0123456789abcdef0123456789abcdef",
            "2026-08-24T00:00:00.000Z",
        )
        .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn execution_rejects_stale_or_reused_plan_tokens() {
        let (temporary, service) = test_service();
        create(&service, "stale_plan", "Before");
        let preview = preview_template_pack_export(
            &service,
            PreviewTemplatePackExportRequest {
                template_ids: vec!["stale_plan".to_owned()],
            },
        )
        .unwrap();
        let current = service
            .repository()
            .get("stale_plan", Some(TemplateOrigin::Custom))
            .unwrap();
        let mut changed = current.template;
        changed.name = "Changed".to_owned();
        service
            .repository()
            .update("stale_plan", changed.version, &current.file_sha256, changed)
            .unwrap();
        let destination = temporary.path().join("stale.meetily-template-pack");
        let request = ExportTemplatePackRequest {
            plan_token: preview.plan_token.clone(),
            destination_path: destination.to_string_lossy().into_owned(),
            overwrite: false,
        };
        assert_eq!(
            export_template_pack(&service, request).unwrap_err().code,
            "TEMPLATE_PACK_PLAN_STALE"
        );
        assert_eq!(
            take_export_plan(&preview.plan_token).unwrap_err().code,
            "TEMPLATE_PACK_PLAN_STALE"
        );
        assert!(!destination.exists());
    }

    #[test]
    fn export_writes_verified_archive_and_respects_explicit_overwrite() {
        let (temporary, service) = test_service();
        create(&service, "export_one", "Export");
        let destination = temporary.path().join("portable.meetily-template-pack");
        fs::write(&destination, b"existing").unwrap();
        let preview = preview_template_pack_export(
            &service,
            PreviewTemplatePackExportRequest {
                template_ids: vec!["export_one".to_owned()],
            },
        )
        .unwrap();
        let error = export_template_pack(
            &service,
            ExportTemplatePackRequest {
                plan_token: preview.plan_token,
                destination_path: destination.to_string_lossy().into_owned(),
                overwrite: false,
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "TEMPLATE_PACK_EXPORT_FAILED");
        assert_eq!(fs::read(&destination).unwrap(), b"existing");

        let preview = preview_template_pack_export(
            &service,
            PreviewTemplatePackExportRequest {
                template_ids: vec!["export_one".to_owned()],
            },
        )
        .unwrap();
        let response = export_template_pack(
            &service,
            ExportTemplatePackRequest {
                plan_token: preview.plan_token,
                destination_path: destination.to_string_lossy().into_owned(),
                overwrite: true,
            },
        )
        .unwrap();
        let bytes = fs::read(&destination).unwrap();
        assert_eq!(response.byte_size, bytes.len());
        assert_eq!(response.package.archive_sha256, sha256_hex(&bytes));
        assert_eq!(
            response.package.package_file_name,
            "portable.meetily-template-pack"
        );
        assert!(!serde_json::to_string(&response)
            .unwrap()
            .contains(temporary.path().to_string_lossy().as_ref()));
        assert!(ZipArchive::new(Cursor::new(bytes)).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn locked_windows_destination_preserves_original_and_cleans_staging_file() {
        use std::os::windows::fs::OpenOptionsExt;

        let (temporary, service) = test_service();
        create(&service, "locked_export", "Export");
        let destination = temporary.path().join("locked.meetily-template-pack");
        fs::write(&destination, b"original-package").unwrap();
        let locked = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(&destination)
            .unwrap();
        let preview = preview_template_pack_export(
            &service,
            PreviewTemplatePackExportRequest {
                template_ids: vec!["locked_export".to_owned()],
            },
        )
        .unwrap();
        let error = export_template_pack(
            &service,
            ExportTemplatePackRequest {
                plan_token: preview.plan_token,
                destination_path: destination.to_string_lossy().into_owned(),
                overwrite: true,
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "TEMPLATE_PACK_EXPORT_FAILED");
        drop(locked);
        assert_eq!(fs::read(&destination).unwrap(), b"original-package");
        let unexpected_staging_files: Vec<_> = fs::read_dir(temporary.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .filter(|entry| entry.path() != destination)
            .collect();
        assert!(unexpected_staging_files.is_empty());
    }

    #[test]
    fn destination_extension_and_non_file_targets_are_rejected() {
        let temporary = TempDir::new().unwrap();
        assert_eq!(
            validate_destination(
                temporary.path().join("pack.zip").to_string_lossy().as_ref(),
                false
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_EXPORT_FAILED"
        );
        let directory_target = temporary.path().join("dir.meetily-template-pack");
        fs::create_dir(&directory_target).unwrap();
        assert_eq!(
            validate_destination(directory_target.to_string_lossy().as_ref(), true)
                .unwrap_err()
                .code,
            "TEMPLATE_PACK_UNSAFE_ENTRY"
        );
    }

    #[test]
    fn import_preview_accepts_exported_pack_and_keeps_repository_zero_write() {
        let temporary = TempDir::new().unwrap();
        let source_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("source"), None).unwrap(),
        );
        let target_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("target"), None).unwrap(),
        );
        create(&source_service, "portable_alpha", "Summarize safely");
        let bytes = portable_archive_for(&source_service, &["portable_alpha"]);
        let source = write_pack(temporary.path(), "valid.meetily-template-pack", &bytes);
        let before = target_service.repository().list().unwrap();
        let response = preview_template_pack_import(
            &target_service,
            PreviewTemplatePackImportRequest {
                source_path: source.to_string_lossy().into_owned(),
            },
        )
        .unwrap();
        let after = target_service.repository().list().unwrap();
        assert_eq!(before, after);
        assert_eq!(
            response.package.package_file_name,
            "valid.meetily-template-pack"
        );
        assert_eq!(response.package.archive_sha256, sha256_hex(&bytes));
        assert_eq!(response.items.len(), 1);
        assert_eq!(
            response.items[0].conflict_kind,
            PortablePackConflictKind::None
        );
        assert!(response.items[0].allowed_strategies.is_empty());
        assert_eq!(response.items[0].template.fingerprint.id, "portable_alpha");
        assert_eq!(response.total_uncompressed_bytes, {
            let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
            (0..archive.len())
                .map(|index| archive.by_index_raw(index).unwrap().size() as usize)
                .sum::<usize>()
        });
        assert!(!serde_json::to_string(&response)
            .unwrap()
            .contains(temporary.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn import_preview_classifies_custom_and_readonly_conflicts() {
        let temporary = TempDir::new().unwrap();
        let source_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("source"), None).unwrap(),
        );
        // Read-only conflicts use a bundled file; seeded built-ins are editable copies.
        let bundled = temporary.path().join("bundled");
        fs::create_dir(&bundled).unwrap();
        fs::write(
            bundled.join("qa_readonly.json"),
            serde_json::to_vec(&template("qa_readonly", "Existing read-only template")).unwrap(),
        )
        .unwrap();
        let target_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("target"), Some(bundled)).unwrap(),
        );
        create(&source_service, "conflict_custom", "Incoming");
        create(&target_service, "conflict_custom", "Existing");
        source_service
            .repository()
            .create(
                template("qa_readonly", "Incoming read-only override"),
                CreateConflictPolicy::OverrideReadOnly,
            )
            .unwrap();
        let bytes = portable_archive_for(&source_service, &["conflict_custom", "qa_readonly"]);
        let source = write_pack(temporary.path(), "conflicts.meetily-template-pack", &bytes);
        let response = preview_template_pack_import(
            &target_service,
            PreviewTemplatePackImportRequest {
                source_path: source.to_string_lossy().into_owned(),
            },
        )
        .unwrap();
        assert_eq!(
            response.items[0].conflict_kind,
            PortablePackConflictKind::Custom
        );
        assert_eq!(
            response.items[0].allowed_strategies,
            vec![
                PortablePackConflictStrategy::Skip,
                PortablePackConflictStrategy::KeepBoth,
                PortablePackConflictStrategy::ReplaceCustom,
            ]
        );
        assert!(response.items[0].existing.is_some());
        assert_eq!(
            response.items[1].conflict_kind,
            PortablePackConflictKind::Readonly
        );
        assert_eq!(
            response.items[1].allowed_strategies,
            vec![
                PortablePackConflictStrategy::Skip,
                PortablePackConflictStrategy::KeepBoth,
            ]
        );
    }

    #[test]
    fn import_source_rejects_wrong_extension_directory_and_oversized_file() {
        let (temporary, service) = test_service();
        let wrong_extension = temporary.path().join("pack.zip");
        fs::write(&wrong_extension, b"PK\x03\x04").unwrap();
        assert_eq!(
            validate_import_source(wrong_extension.to_string_lossy().as_ref())
                .unwrap_err()
                .code,
            "TEMPLATE_PACK_INVALID"
        );
        let directory = temporary.path().join("directory.meetily-template-pack");
        fs::create_dir(&directory).unwrap();
        assert_eq!(
            validate_import_source(directory.to_string_lossy().as_ref())
                .unwrap_err()
                .code,
            "TEMPLATE_PACK_UNSAFE_ENTRY"
        );
        let oversized = temporary.path().join("oversized.meetily-template-pack");
        File::create(&oversized)
            .unwrap()
            .set_len(MAX_ARCHIVE_BYTES as u64 + 1)
            .unwrap();
        assert_eq!(
            preview_template_pack_import(
                &service,
                PreviewTemplatePackImportRequest {
                    source_path: oversized.to_string_lossy().into_owned()
                }
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_BUDGET_EXCEEDED"
        );
    }

    #[test]
    fn zip_scan_rejects_traversal_absolute_backslash_ads_and_dot_segments() {
        let malicious = [
            "../evil.json",
            "/absolute.json",
            r"templates\evil.json",
            "C:evil.json",
            "templates/../evil.json",
            "templates/./evil.json",
            "templates/evil.json/",
            "templates/evil .json",
            "templates/evil.json. ",
        ];
        for name in malicious {
            let bytes = arbitrary_zip(
                &[("manifest.json", b"{}"), (name, b"{}")],
                CompressionMethod::Stored,
            );
            assert_eq!(
                inspect_portable_archive(&bytes).unwrap_err().code,
                "TEMPLATE_PACK_UNSAFE_ENTRY",
                "unsafe name was not rejected: {name}"
            );
        }
    }

    #[test]
    fn zip_scan_rejects_duplicate_casefold_and_unknown_entries() {
        let mut duplicate = arbitrary_zip(
            &[
                ("manifest.json", b"{}"),
                ("templates/same1.json", b"{}"),
                ("templates/same2.json", b"{}"),
            ],
            CompressionMethod::Stored,
        );
        let old_name = b"templates/same2.json";
        let new_name = b"templates/same1.json";
        let positions: Vec<_> = duplicate
            .windows(old_name.len())
            .enumerate()
            .filter_map(|(index, window)| (window == old_name).then_some(index))
            .collect();
        assert_eq!(positions.len(), 2);
        for position in positions {
            duplicate[position..position + new_name.len()].copy_from_slice(new_name);
        }
        assert_eq!(
            inspect_portable_archive(&duplicate).unwrap_err().code,
            "TEMPLATE_PACK_UNSAFE_ENTRY"
        );

        for entries in [
            vec![
                ("manifest.json", b"{}".as_slice()),
                ("templates/Case.json", b"{}".as_slice()),
                ("templates/case.json", b"{}".as_slice()),
            ],
            vec![
                ("manifest.json", b"{}".as_slice()),
                ("README.txt", b"unexpected".as_slice()),
            ],
        ] {
            let bytes = arbitrary_zip(&entries, CompressionMethod::Stored);
            assert_eq!(
                inspect_portable_archive(&bytes).unwrap_err().code,
                "TEMPLATE_PACK_UNSAFE_ENTRY"
            );
        }
    }

    #[test]
    fn zip_scan_requires_manifest_first_and_enforces_entry_budget() {
        let wrong_order = arbitrary_zip(
            &[("templates/first.json", b"{}"), ("manifest.json", b"{}")],
            CompressionMethod::Stored,
        );
        assert_eq!(
            inspect_portable_archive(&wrong_order).unwrap_err().code,
            "TEMPLATE_PACK_INVALID"
        );
        let names: Vec<_> = (0..MAX_ARCHIVE_ENTRIES + 1)
            .map(|index| (format!("templates/item_{index:03}.json"), b"{}".to_vec()))
            .collect();
        let refs: Vec<_> = names
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
            .collect();
        let too_many = arbitrary_zip(&refs, CompressionMethod::Stored);
        assert_eq!(
            validate_classic_single_disk_zip(&too_many)
                .unwrap_err()
                .code,
            "TEMPLATE_PACK_BUDGET_EXCEEDED"
        );
    }

    #[test]
    fn zip_scan_rejects_unsupported_compression_and_high_ratio_entries() {
        let unsupported = arbitrary_zip(&[("manifest.json", b"{}")], CompressionMethod::Bzip2);
        assert_eq!(
            inspect_portable_archive(&unsupported).unwrap_err().code,
            "TEMPLATE_PACK_UNSAFE_ENTRY"
        );
        let compressible = vec![b'0'; 200_000];
        let bomb = arbitrary_zip(
            &[("manifest.json", compressible.as_slice())],
            CompressionMethod::Deflated,
        );
        assert_eq!(
            inspect_portable_archive(&bomb).unwrap_err().code,
            "TEMPLATE_PACK_BUDGET_EXCEEDED"
        );
    }

    #[test]
    fn zip_scan_rejects_encryption_symlink_and_zip64_markers() {
        let base = arbitrary_zip(&[("manifest.json", b"{}")], CompressionMethod::Stored);
        let mut mismatched_local_name = base.clone();
        let local_name = mismatched_local_name
            .windows("manifest.json".len())
            .position(|window| window == b"manifest.json")
            .unwrap();
        mismatched_local_name[local_name] = b'n';
        assert_eq!(
            validate_classic_single_disk_zip(&mismatched_local_name)
                .unwrap_err()
                .code,
            "TEMPLATE_PACK_UNSAFE_ENTRY"
        );

        let mut encrypted = base.clone();
        encrypted[6] |= 1;
        let central = encrypted
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .unwrap();
        encrypted[central + 8] |= 1;
        assert_eq!(
            inspect_portable_archive(&encrypted).unwrap_err().code,
            "TEMPLATE_PACK_UNSAFE_ENTRY"
        );

        let mut symlink = base.clone();
        let central = symlink
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .unwrap();
        symlink[central + 5] = 3;
        let external_attributes = (0o120777_u32 << 16).to_le_bytes();
        symlink[central + 38..central + 42].copy_from_slice(&external_attributes);
        assert_eq!(
            inspect_portable_archive(&symlink).unwrap_err().code,
            "TEMPLATE_PACK_UNSAFE_ENTRY"
        );

        let mut zip64 = base;
        let eocd = zip64
            .windows(4)
            .rposition(|window| window == b"PK\x05\x06")
            .unwrap();
        zip64.splice(eocd..eocd, *b"PK\x06\x07");
        assert_eq!(
            validate_classic_single_disk_zip(&zip64).unwrap_err().code,
            "TEMPLATE_PACK_UNSAFE_ENTRY"
        );
    }

    #[test]
    fn import_rejects_manifest_version_and_template_hash_tampering() {
        let temporary = TempDir::new().unwrap();
        let source_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("source"), None).unwrap(),
        );
        let target_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("target"), None).unwrap(),
        );
        create(&source_service, "tamper_guard", "Original");
        let original = portable_archive_for(&source_service, &["tamper_guard"]);

        let mut archive = ZipArchive::new(Cursor::new(&original)).unwrap();
        let mut manifest_bytes = Vec::new();
        archive
            .by_name("manifest.json")
            .unwrap()
            .read_to_end(&mut manifest_bytes)
            .unwrap();
        let mut manifest: Value = serde_json::from_slice(&manifest_bytes).unwrap();
        manifest["package_schema_version"] = Value::from(2);
        let unsupported = rewrite_archive_entry(
            &original,
            "manifest.json",
            &serde_json::to_vec_pretty(&manifest).unwrap(),
        );
        let path = write_pack(
            temporary.path(),
            "unsupported.meetily-template-pack",
            &unsupported,
        );
        assert_eq!(
            preview_template_pack_import(
                &target_service,
                PreviewTemplatePackImportRequest {
                    source_path: path.to_string_lossy().into_owned()
                }
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_VERSION_UNSUPPORTED"
        );

        let tampered = rewrite_archive_entry(
            &original,
            "templates/tamper_guard.json",
            br#"{"schema_version":2}"#,
        );
        let path = write_pack(
            temporary.path(),
            "tampered.meetily-template-pack",
            &tampered,
        );
        assert_eq!(
            preview_template_pack_import(
                &target_service,
                PreviewTemplatePackImportRequest {
                    source_path: path.to_string_lossy().into_owned()
                }
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_INTEGRITY_FAILED"
        );
    }

    #[test]
    fn import_rejects_semantic_hash_mismatch_and_sensitive_template_content() {
        let temporary = TempDir::new().unwrap();
        let target_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("target"), None).unwrap(),
        );
        let sensitive_template = template(
            "sensitive_import",
            "Authorization: Bearer abcdefghijklmnopqrstuvwxyz0123456789",
        );
        let mut template_bytes = serde_json::to_vec_pretty(&sensitive_template).unwrap();
        template_bytes.push(b'\n');
        let sensitive = ResolvedExportTemplate {
            preview: PortablePackExportTemplate {
                fingerprint: PortablePackTemplateFingerprint {
                    id: sensitive_template.id.clone(),
                    version: sensitive_template.version,
                    file_sha256: sha256_hex(&template_bytes),
                    semantic_sha256: semantic_sha256(&sensitive_template).unwrap(),
                },
                name: sensitive_template.name.clone(),
                byte_size: template_bytes.len(),
                overrides_builtin: false,
            },
            bytes: template_bytes,
        };
        let sensitive_pack = build_portable_archive(
            &[sensitive],
            "pack_0123456789abcdef0123456789abcdef",
            "2026-08-24T00:00:00.000Z",
        )
        .unwrap();
        let path = write_pack(
            temporary.path(),
            "sensitive.meetily-template-pack",
            &sensitive_pack,
        );
        assert_eq!(
            preview_template_pack_import(
                &target_service,
                PreviewTemplatePackImportRequest {
                    source_path: path.to_string_lossy().into_owned()
                }
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_SENSITIVE_CONTENT"
        );

        let source_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("source"), None).unwrap(),
        );
        create(&source_service, "semantic_guard", "Safe");
        let original = portable_archive_for(&source_service, &["semantic_guard"]);
        let mut archive = ZipArchive::new(Cursor::new(&original)).unwrap();
        let mut manifest_bytes = Vec::new();
        archive
            .by_name("manifest.json")
            .unwrap()
            .read_to_end(&mut manifest_bytes)
            .unwrap();
        let mut manifest: Value = serde_json::from_slice(&manifest_bytes).unwrap();
        manifest["templates"][0]["semantic_sha256"] = Value::String("0".repeat(64));
        let mismatched = rewrite_archive_entry(
            &original,
            "manifest.json",
            &serde_json::to_vec_pretty(&manifest).unwrap(),
        );
        let path = write_pack(
            temporary.path(),
            "semantic-mismatch.meetily-template-pack",
            &mismatched,
        );
        assert_eq!(
            preview_template_pack_import(
                &target_service,
                PreviewTemplatePackImportRequest {
                    source_path: path.to_string_lossy().into_owned()
                }
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_INTEGRITY_FAILED"
        );
    }

    #[test]
    fn import_transport_shape_is_camel_case_and_rejects_unknown_fields() {
        let request: PreviewTemplatePackImportRequest =
            serde_json::from_value(serde_json::json!({"sourcePath": "pack.meetily-template-pack"}))
                .unwrap();
        assert_eq!(request.source_path, "pack.meetily-template-pack");
        assert!(
            serde_json::from_value::<PreviewTemplatePackImportRequest>(serde_json::json!({
                "sourcePath": "pack.meetily-template-pack",
                "extra": true
            }))
            .is_err()
        );
    }

    fn preview_import(
        target_service: &TemplateService,
        source: &Path,
    ) -> PreviewTemplatePackImportResponse {
        preview_template_pack_import(
            target_service,
            PreviewTemplatePackImportRequest {
                source_path: source.to_string_lossy().into_owned(),
            },
        )
        .unwrap()
    }

    fn import_decision(
        preview: &PreviewTemplatePackImportResponse,
        template_id: &str,
        strategy: PortablePackConflictStrategy,
    ) -> PortablePackImportDecision {
        PortablePackImportDecision {
            item_id: preview
                .items
                .iter()
                .find(|item| item.template.fingerprint.id == template_id)
                .unwrap()
                .item_id
                .clone(),
            strategy,
        }
    }

    #[test]
    fn import_plan_builds_create_keep_both_and_skip_without_repository_writes() {
        let temporary = TempDir::new().unwrap();
        let source_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("source-plan"), None).unwrap(),
        );
        // Read-only conflicts use a bundled file; seeded built-ins are editable copies.
        let bundled = temporary.path().join("bundled");
        fs::create_dir(&bundled).unwrap();
        fs::write(
            bundled.join("qa_readonly.json"),
            serde_json::to_vec(&template("qa_readonly", "Existing read-only template")).unwrap(),
        )
        .unwrap();
        let target_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("target-plan"), Some(bundled)).unwrap(),
        );
        create(&source_service, "plan_create", "Create");
        create(&source_service, "plan_conflict", "Incoming");
        create(&target_service, "plan_conflict", "Existing");
        source_service
            .repository()
            .create(
                template("qa_readonly", "Incoming read-only override"),
                CreateConflictPolicy::OverrideReadOnly,
            )
            .unwrap();
        let bytes = portable_archive_for(
            &source_service,
            &["plan_create", "plan_conflict", "qa_readonly"],
        );
        let source = write_pack(
            temporary.path(),
            "plan-success.meetily-template-pack",
            &bytes,
        );
        let preview = preview_import(&target_service, &source);
        let before = target_service.repository().list().unwrap();
        let request = PlanTemplatePackImportRequest {
            preview_plan_token: preview.plan_token.clone(),
            decisions: vec![
                import_decision(
                    &preview,
                    "plan_conflict",
                    PortablePackConflictStrategy::KeepBoth,
                ),
                import_decision(
                    &preview,
                    "qa_readonly",
                    PortablePackConflictStrategy::Skip,
                ),
            ],
        };
        let planned = plan_template_pack_import(&target_service, request.clone()).unwrap();
        let after = target_service.repository().list().unwrap();
        assert_eq!(before, after);
        assert_eq!(planned.operations.len(), 3);
        assert_eq!(planned.summary.create_count, 2);
        assert_eq!(planned.summary.replace_count, 0);
        assert_eq!(planned.summary.skip_count, 1);
        assert_eq!(planned.summary.transformed_count, 1);
        let keep_both = planned
            .operations
            .iter()
            .find(|operation| operation.source.fingerprint.id == "plan_conflict")
            .unwrap();
        assert_eq!(
            keep_both.operation,
            PortablePackImportOperationKind::KeepBoth
        );
        let target = keep_both.target.as_ref().unwrap();
        assert_eq!(target.fingerprint.id, "plan_conflict_imported");
        assert_eq!(target.fingerprint.version, 1);
        assert_ne!(
            target.fingerprint.semantic_sha256,
            keep_both.source.fingerprint.semantic_sha256
        );
        assert!(keep_both.expected_existing.is_some());
        assert!(!serde_json::to_string(&planned)
            .unwrap()
            .contains(temporary.path().to_string_lossy().as_ref()));
        assert_eq!(
            plan_template_pack_import(&target_service, request)
                .unwrap_err()
                .code,
            "TEMPLATE_PACK_PREVIEW_PLAN_CONSUMED"
        );
    }

    #[test]
    fn import_plan_requires_exact_nonduplicated_compatible_decisions() {
        let temporary = TempDir::new().unwrap();
        let source_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("source-decisions"), None).unwrap(),
        );
        // Read-only conflicts use a bundled file; seeded built-ins are editable copies.
        let bundled = temporary.path().join("bundled");
        fs::create_dir(&bundled).unwrap();
        fs::write(
            bundled.join("qa_readonly.json"),
            serde_json::to_vec(&template("qa_readonly", "Existing read-only template")).unwrap(),
        )
        .unwrap();
        let target_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("target-decisions"), Some(bundled)).unwrap(),
        );
        create(&source_service, "decision_conflict", "Incoming");
        create(&target_service, "decision_conflict", "Existing");
        create(&source_service, "decision_create", "Create");
        source_service
            .repository()
            .create(
                template("qa_readonly", "Incoming read-only override"),
                CreateConflictPolicy::OverrideReadOnly,
            )
            .unwrap();
        let bytes = portable_archive_for(
            &source_service,
            &["decision_conflict", "decision_create", "qa_readonly"],
        );
        let source = write_pack(
            temporary.path(),
            "decision-errors.meetily-template-pack",
            &bytes,
        );
        let preview = preview_import(&target_service, &source);
        let base = |decisions| PlanTemplatePackImportRequest {
            preview_plan_token: preview.plan_token.clone(),
            decisions,
        };
        assert_eq!(
            plan_template_pack_import(&target_service, base(Vec::new()))
                .unwrap_err()
                .code,
            "TEMPLATE_PACK_DECISION_MISSING"
        );
        let duplicate = import_decision(
            &preview,
            "decision_conflict",
            PortablePackConflictStrategy::Skip,
        );
        assert_eq!(
            plan_template_pack_import(
                &target_service,
                base(vec![duplicate.clone(), duplicate.clone()])
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_DECISION_DUPLICATE"
        );
        assert_eq!(
            plan_template_pack_import(
                &target_service,
                base(vec![PortablePackImportDecision {
                    item_id: format!("item_{}", Uuid::new_v4().simple()),
                    strategy: PortablePackConflictStrategy::Skip,
                }])
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_DECISION_UNKNOWN_ITEM"
        );
        let create_decision = import_decision(
            &preview,
            "decision_create",
            PortablePackConflictStrategy::Skip,
        );
        let readonly_skip = import_decision(
            &preview,
            "qa_readonly",
            PortablePackConflictStrategy::Skip,
        );
        assert_eq!(
            plan_template_pack_import(
                &target_service,
                base(vec![duplicate.clone(), readonly_skip, create_decision])
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_DECISION_NOT_ALLOWED"
        );
        let readonly_replace = import_decision(
            &preview,
            "qa_readonly",
            PortablePackConflictStrategy::ReplaceCustom,
        );
        assert_eq!(
            plan_template_pack_import(&target_service, base(vec![duplicate, readonly_replace]))
                .unwrap_err()
                .code,
            "TEMPLATE_PACK_DECISION_NOT_ALLOWED"
        );
    }

    #[test]
    fn import_plan_rejects_changed_source_and_repository_snapshot() {
        let temporary = TempDir::new().unwrap();
        let source_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("source-stale"), None).unwrap(),
        );
        let target_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("target-stale"), None).unwrap(),
        );
        create(&source_service, "stale_source", "Source");
        let bytes = portable_archive_for(&source_service, &["stale_source"]);
        let source = write_pack(
            temporary.path(),
            "stale-source.meetily-template-pack",
            &bytes,
        );
        let preview = preview_import(&target_service, &source);
        let mut changed = bytes.clone();
        changed.push(0);
        fs::write(&source, changed).unwrap();
        assert_eq!(
            plan_template_pack_import(
                &target_service,
                PlanTemplatePackImportRequest {
                    preview_plan_token: preview.plan_token,
                    decisions: Vec::new(),
                }
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_PLAN_STALE"
        );

        create(&source_service, "stale_repository", "Source");
        let bytes = portable_archive_for(&source_service, &["stale_repository"]);
        let source = write_pack(
            temporary.path(),
            "stale-repository.meetily-template-pack",
            &bytes,
        );
        let preview = preview_import(&target_service, &source);
        create(&target_service, "stale_repository", "Created after preview");
        assert_eq!(
            plan_template_pack_import(
                &target_service,
                PlanTemplatePackImportRequest {
                    preview_plan_token: preview.plan_token,
                    decisions: Vec::new(),
                }
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_CONFLICT_CHANGED"
        );
    }

    #[test]
    fn import_plan_distinguishes_missing_expired_and_consumed_preview_tokens() {
        let (_temporary, service) = test_service();
        assert_eq!(
            plan_template_pack_import(
                &service,
                PlanTemplatePackImportRequest {
                    preview_plan_token: Uuid::new_v4().to_string(),
                    decisions: Vec::new(),
                }
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_PREVIEW_PLAN_NOT_FOUND"
        );

        let temporary = TempDir::new().unwrap();
        let source_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("source-expired"), None).unwrap(),
        );
        let target_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("target-expired"), None).unwrap(),
        );
        create(&source_service, "expired_plan", "Expired");
        let bytes = portable_archive_for(&source_service, &["expired_plan"]);
        let source = write_pack(temporary.path(), "expired.meetily-template-pack", &bytes);
        let preview = preview_import(&target_service, &source);
        {
            let mut plans = IMPORT_PLANS
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            plans.get_mut(&preview.plan_token).unwrap().created =
                Instant::now() - EXPORT_PLAN_TTL - Duration::from_secs(1);
        }
        assert_eq!(
            plan_template_pack_import(
                &target_service,
                PlanTemplatePackImportRequest {
                    preview_plan_token: preview.plan_token,
                    decisions: Vec::new(),
                }
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_PREVIEW_PLAN_EXPIRED"
        );
    }

    #[test]
    fn keep_both_reserves_package_ids_and_replace_binds_existing_fingerprint() {
        let temporary = TempDir::new().unwrap();
        let source_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("source-map"), None).unwrap(),
        );
        let target_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("target-map"), None).unwrap(),
        );
        create(&source_service, "mapped", "Incoming");
        create(&target_service, "mapped", "Existing");
        create(&target_service, "mapped_imported", "Reserved");
        let bytes = portable_archive_for(&source_service, &["mapped"]);
        let source = write_pack(temporary.path(), "mapped.meetily-template-pack", &bytes);
        let preview = preview_import(&target_service, &source);
        let keep = plan_template_pack_import(
            &target_service,
            PlanTemplatePackImportRequest {
                preview_plan_token: preview.plan_token.clone(),
                decisions: vec![import_decision(
                    &preview,
                    "mapped",
                    PortablePackConflictStrategy::KeepBoth,
                )],
            },
        )
        .unwrap();
        assert_eq!(
            keep.operations[0].target.as_ref().unwrap().fingerprint.id,
            "mapped_imported_2"
        );

        let preview = preview_import(&target_service, &source);
        let replace = plan_template_pack_import(
            &target_service,
            PlanTemplatePackImportRequest {
                preview_plan_token: preview.plan_token.clone(),
                decisions: vec![import_decision(
                    &preview,
                    "mapped",
                    PortablePackConflictStrategy::ReplaceCustom,
                )],
            },
        )
        .unwrap();
        let expected = preview.items[0].existing.as_ref().unwrap();
        assert_eq!(
            replace.operations[0].expected_existing.as_ref(),
            Some(expected)
        );
        assert_eq!(
            replace.operations[0].target.as_ref().unwrap(),
            &replace.operations[0].source
        );
    }

    #[test]
    fn import_plan_transport_is_strict_camel_case() {
        let request: PlanTemplatePackImportRequest = serde_json::from_value(serde_json::json!({
            "previewPlanToken": Uuid::new_v4().to_string(),
            "decisions": [{"itemId": "item_1", "strategy": "keep_both"}]
        }))
        .unwrap();
        assert_eq!(request.decisions.len(), 1);
        assert!(
            serde_json::from_value::<PlanTemplatePackImportRequest>(serde_json::json!({
                "previewPlanToken": Uuid::new_v4().to_string(),
                "decisions": [],
                "extra": true
            }))
            .is_err()
        );
    }

    fn execution_request(
        execution_plan_token: String,
        execution_id: String,
    ) -> ExecuteTemplatePackImportRequest {
        ExecuteTemplatePackImportRequest {
            execution_plan_token,
            execution_id,
        }
    }

    fn custom_bytes(service: &TemplateService, template_id: &str) -> Option<Vec<u8>> {
        service
            .repository()
            .read_bytes_for_diagnostics(template_id, Some(TemplateOrigin::Custom))
            .ok()
    }

    fn portable_transaction_directories(service: &TemplateService) -> Vec<PathBuf> {
        fs::read_dir(service.repository().root().join(".tmp"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.starts_with(IMPORT_TRANSACTION_PREFIX))
            })
            .collect()
    }

    #[test]
    fn execution_applies_create_replace_keep_both_and_skip_with_exact_bytes() {
        let temporary = TempDir::new().unwrap();
        let source_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("source-execute"), None).unwrap(),
        );
        // Read-only conflicts use a bundled file; seeded built-ins are editable copies.
        let bundled = temporary.path().join("bundled");
        fs::create_dir(&bundled).unwrap();
        fs::write(
            bundled.join("qa_readonly.json"),
            serde_json::to_vec(&template("qa_readonly", "Existing read-only template")).unwrap(),
        )
        .unwrap();
        let target_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("target-execute"), Some(bundled)).unwrap(),
        );
        for id in ["exec_create", "exec_replace", "exec_keep"] {
            create(&source_service, id, &format!("Incoming {id}"));
        }
        source_service
            .repository()
            .create(
                template("qa_readonly", "Incoming readonly conflict"),
                CreateConflictPolicy::OverrideReadOnly,
            )
            .unwrap();
        create(
            &target_service,
            "exec_replace",
            "Existing replacement target",
        );
        create(&target_service, "exec_keep", "Existing keep-both target");
        let original_keep = custom_bytes(&target_service, "exec_keep").unwrap();
        let expected_create = custom_bytes(&source_service, "exec_create").unwrap();
        let expected_replace = custom_bytes(&source_service, "exec_replace").unwrap();
        let bytes = portable_archive_for(
            &source_service,
            &["exec_create", "exec_replace", "exec_keep", "qa_readonly"],
        );
        let source = write_pack(
            temporary.path(),
            "execute-success.meetily-template-pack",
            &bytes,
        );
        let preview = preview_import(&target_service, &source);
        let planned = plan_template_pack_import(
            &target_service,
            PlanTemplatePackImportRequest {
                preview_plan_token: preview.plan_token.clone(),
                decisions: vec![
                    import_decision(
                        &preview,
                        "exec_replace",
                        PortablePackConflictStrategy::ReplaceCustom,
                    ),
                    import_decision(
                        &preview,
                        "exec_keep",
                        PortablePackConflictStrategy::KeepBoth,
                    ),
                    import_decision(
                        &preview,
                        "qa_readonly",
                        PortablePackConflictStrategy::Skip,
                    ),
                ],
            },
        )
        .unwrap();
        let execution_id = Uuid::new_v4().to_string();
        let response = execute_template_pack_import(
            &target_service,
            execution_request(planned.execution_plan_token.clone(), execution_id.clone()),
        )
        .unwrap();

        assert_eq!(response.execution_id, execution_id);
        assert_eq!(response.summary.create_count, 2);
        assert_eq!(response.summary.replace_count, 1);
        assert_eq!(response.summary.skip_count, 1);
        assert_eq!(response.summary.transformed_count, 1);
        assert_eq!(
            custom_bytes(&target_service, "exec_create"),
            Some(expected_create)
        );
        assert_eq!(
            custom_bytes(&target_service, "exec_replace"),
            Some(expected_replace)
        );
        assert_eq!(
            custom_bytes(&target_service, "exec_keep"),
            Some(original_keep)
        );
        let kept = response
            .results
            .iter()
            .find(|result| result.operation == PortablePackImportOperationKind::KeepBoth)
            .and_then(|result| result.target.as_ref())
            .unwrap();
        let kept_record = target_service
            .repository()
            .get(&kept.fingerprint.id, Some(TemplateOrigin::Custom))
            .unwrap();
        assert_eq!(kept_record.file_sha256, kept.fingerprint.file_sha256);
        assert_eq!(
            kept_record.template.source.source_type,
            TemplateSourceType::Duplicate
        );
        assert_eq!(
            kept_record
                .template
                .source
                .copied_from_template_id
                .as_deref(),
            Some("exec_keep")
        );
        assert!(custom_bytes(&target_service, "qa_readonly").is_none());
        assert!(portable_transaction_directories(&target_service).is_empty());
        assert_eq!(
            execute_template_pack_import(
                &target_service,
                execution_request(planned.execution_plan_token, Uuid::new_v4().to_string(),),
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_EXECUTION_PLAN_CONSUMED"
        );
    }

    #[test]
    fn execution_rebuilds_source_and_repository_state_before_writing() {
        let temporary = TempDir::new().unwrap();
        let source_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("source-revalidate"), None).unwrap(),
        );
        let target_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("target-revalidate"), None).unwrap(),
        );
        create(&source_service, "execute_stale_source", "Source");
        let bytes = portable_archive_for(&source_service, &["execute_stale_source"]);
        let source = write_pack(
            temporary.path(),
            "execute-stale-source.meetily-template-pack",
            &bytes,
        );
        let preview = preview_import(&target_service, &source);
        let planned = plan_template_pack_import(
            &target_service,
            PlanTemplatePackImportRequest {
                preview_plan_token: preview.plan_token,
                decisions: Vec::new(),
            },
        )
        .unwrap();
        let mut changed = bytes;
        changed.push(0);
        fs::write(&source, changed).unwrap();
        assert_eq!(
            execute_template_pack_import(
                &target_service,
                execution_request(planned.execution_plan_token, Uuid::new_v4().to_string()),
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_PLAN_STALE"
        );
        assert!(custom_bytes(&target_service, "execute_stale_source").is_none());

        create(&source_service, "execute_stale_repository", "Source");
        let bytes = portable_archive_for(&source_service, &["execute_stale_repository"]);
        let source = write_pack(
            temporary.path(),
            "execute-stale-repository.meetily-template-pack",
            &bytes,
        );
        let preview = preview_import(&target_service, &source);
        let planned = plan_template_pack_import(
            &target_service,
            PlanTemplatePackImportRequest {
                preview_plan_token: preview.plan_token,
                decisions: Vec::new(),
            },
        )
        .unwrap();
        create(
            &target_service,
            "execute_stale_repository",
            "Concurrent local write",
        );
        let before = custom_bytes(&target_service, "execute_stale_repository").unwrap();
        assert_eq!(
            execute_template_pack_import(
                &target_service,
                execution_request(planned.execution_plan_token, Uuid::new_v4().to_string()),
            )
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_CONFLICT_CHANGED"
        );
        assert_eq!(
            custom_bytes(&target_service, "execute_stale_repository"),
            Some(before)
        );
    }

    fn planned_materialized_create_batch(
        temporary: &TempDir,
        label: &str,
    ) -> (
        TemplateService,
        PortablePackExecutionPlan,
        Vec<MaterializedImportOperation>,
    ) {
        let source_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join(format!("source-{label}")), None)
                .unwrap(),
        );
        let target_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join(format!("target-{label}")), None)
                .unwrap(),
        );
        let first = format!("{label}_first");
        let second = format!("{label}_second");
        create(&source_service, &first, "First");
        create(&source_service, &second, "Second");
        let bytes = portable_archive_for(&source_service, &[&first, &second]);
        let source = write_pack(
            temporary.path(),
            &format!("{label}.meetily-template-pack"),
            &bytes,
        );
        let preview = preview_import(&target_service, &source);
        let planned = plan_template_pack_import(
            &target_service,
            PlanTemplatePackImportRequest {
                preview_plan_token: preview.plan_token,
                decisions: Vec::new(),
            },
        )
        .unwrap();
        let plan = take_import_execution_plan(&planned.execution_plan_token).unwrap();
        let templates = revalidate_execution_plan(&plan).unwrap();
        let materialized = materialize_import_operations(&plan, &templates).unwrap();
        (target_service, plan, materialized)
    }

    #[test]
    fn mid_batch_failure_and_cancellation_roll_back_all_writes_and_staging() {
        for (label, interrupt, expected_code) in [
            (
                "rollback_failure",
                ImportTransactionInterrupt {
                    fail_after_applied: Some(1),
                    ..Default::default()
                },
                "TEMPLATE_PACK_IMPORT_FAILED",
            ),
            (
                "rollback_cancel",
                ImportTransactionInterrupt {
                    cancel_after_applied: Some(1),
                    ..Default::default()
                },
                "TEMPLATE_CANCELLED",
            ),
        ] {
            let temporary = TempDir::new().unwrap();
            let (target_service, plan, materialized) =
                planned_materialized_create_batch(&temporary, label);
            let cancellation = AtomicBool::new(false);
            let _guard = target_service.repository().lock_writes().unwrap();
            let error = execute_import_transaction_locked(
                target_service.repository().root(),
                &Uuid::new_v4().to_string(),
                &materialized,
                &cancellation,
                interrupt,
            )
            .unwrap_err();
            assert_eq!(error.code, expected_code);
            for operation in &plan.operations {
                assert!(custom_bytes(
                    &target_service,
                    &operation.target.as_ref().unwrap().fingerprint.id
                )
                .is_none());
            }
            assert!(portable_transaction_directories(&target_service).is_empty());
        }
    }

    #[test]
    fn failed_batch_restores_replaced_template_byte_for_byte() {
        let temporary = TempDir::new().unwrap();
        let source_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("source-replace-rollback"), None)
                .unwrap(),
        );
        let target_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("target-replace-rollback"), None)
                .unwrap(),
        );
        create(&source_service, "a_replace", "Incoming replacement");
        create(&source_service, "z_create", "Created after replacement");
        create(&target_service, "a_replace", "Original bytes must survive");
        let original = custom_bytes(&target_service, "a_replace").unwrap();
        let bytes = portable_archive_for(&source_service, &["a_replace", "z_create"]);
        let source = write_pack(
            temporary.path(),
            "replace-rollback.meetily-template-pack",
            &bytes,
        );
        let preview = preview_import(&target_service, &source);
        let planned = plan_template_pack_import(
            &target_service,
            PlanTemplatePackImportRequest {
                preview_plan_token: preview.plan_token.clone(),
                decisions: vec![import_decision(
                    &preview,
                    "a_replace",
                    PortablePackConflictStrategy::ReplaceCustom,
                )],
            },
        )
        .unwrap();
        let plan = take_import_execution_plan(&planned.execution_plan_token).unwrap();
        let templates = revalidate_execution_plan(&plan).unwrap();
        let materialized = materialize_import_operations(&plan, &templates).unwrap();
        assert_eq!(
            materialized[0].operation,
            PortablePackImportOperationKind::ReplaceCustom
        );
        let _guard = target_service.repository().lock_writes().unwrap();
        let error = execute_import_transaction_locked(
            target_service.repository().root(),
            &Uuid::new_v4().to_string(),
            &materialized,
            &AtomicBool::new(false),
            ImportTransactionInterrupt {
                fail_after_applied: Some(1),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "TEMPLATE_PACK_IMPORT_FAILED");
        assert_eq!(custom_bytes(&target_service, "a_replace"), Some(original));
        assert!(custom_bytes(&target_service, "z_create").is_none());
        assert!(portable_transaction_directories(&target_service).is_empty());
    }

    #[test]
    fn crash_recovery_rolls_back_incomplete_commit_and_finalizes_committed_data() {
        let temporary = TempDir::new().unwrap();
        let (target_service, _plan, materialized) =
            planned_materialized_create_batch(&temporary, "crash_rollback");
        let execution_id = Uuid::new_v4().to_string();
        let (transaction_root, journal) = prepare_import_transaction(
            target_service.repository().root(),
            &execution_id,
            &materialized,
        )
        .unwrap();
        write_transaction_marker(&transaction_root, IMPORT_TRANSACTION_COMMITTING).unwrap();
        let first = &journal.operations[0];
        fs::rename(
            transaction_root.join(&first.staged_file_name),
            target_service
                .repository()
                .root()
                .join(&first.target_file_name),
        )
        .unwrap();
        let recovery = recover_template_pack_imports(&target_service).unwrap();
        assert_eq!(recovery.rolled_back_transactions, 1);
        assert_eq!(recovery.finalized_transactions, 0);
        for operation in &journal.operations {
            assert!(!target_service
                .repository()
                .root()
                .join(&operation.target_file_name)
                .exists());
        }
        assert!(!transaction_root.exists());

        let temporary = TempDir::new().unwrap();
        let (target_service, _plan, materialized) =
            planned_materialized_create_batch(&temporary, "crash_finalize");
        let execution_id = Uuid::new_v4().to_string();
        let (transaction_root, journal) = prepare_import_transaction(
            target_service.repository().root(),
            &execution_id,
            &materialized,
        )
        .unwrap();
        commit_import_transaction(
            target_service.repository().root(),
            &transaction_root,
            &journal,
            &AtomicBool::new(false),
            ImportTransactionInterrupt::default(),
        )
        .unwrap();
        assert!(transaction_root.exists());
        let recovery = recover_template_pack_imports(&target_service).unwrap();
        assert_eq!(recovery.finalized_transactions, 1);
        assert_eq!(recovery.rolled_back_transactions, 0);
        for operation in &journal.operations {
            assert_eq!(
                bounded_file_sha256(
                    &target_service
                        .repository()
                        .root()
                        .join(&operation.target_file_name)
                )
                .unwrap(),
                operation.new_sha256
            );
        }
        assert!(!transaction_root.exists());
    }

    #[test]
    fn cancellation_registry_is_strict_and_cancelled_preflight_writes_nothing() {
        let temporary = TempDir::new().unwrap();
        let source_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("source-cancel-api"), None).unwrap(),
        );
        let target_service = TemplateService::new(
            TemplateRepository::new(temporary.path().join("target-cancel-api"), None).unwrap(),
        );
        create(&source_service, "cancel_api", "Cancel");
        let bytes = portable_archive_for(&source_service, &["cancel_api"]);
        let source = write_pack(temporary.path(), "cancel-api.meetily-template-pack", &bytes);
        let preview = preview_import(&target_service, &source);
        let planned = plan_template_pack_import(
            &target_service,
            PlanTemplatePackImportRequest {
                preview_plan_token: preview.plan_token,
                decisions: Vec::new(),
            },
        )
        .unwrap();
        let execution_id = Uuid::new_v4().to_string();
        let cancellation = register_import_execution(&execution_id).unwrap();
        let cancelled = cancel_template_pack_import(CancelTemplatePackImportRequest {
            execution_id: execution_id.clone(),
        })
        .unwrap();
        assert!(cancelled.cancellation_requested);
        assert_eq!(
            execute_template_pack_import_inner(
                &target_service,
                &execution_request(planned.execution_plan_token.clone(), execution_id.clone()),
                &cancellation,
            )
            .unwrap_err()
            .code,
            "TEMPLATE_CANCELLED"
        );
        unregister_import_execution(&execution_id);
        assert!(custom_bytes(&target_service, "cancel_api").is_none());
        execute_template_pack_import(
            &target_service,
            execution_request(planned.execution_plan_token, Uuid::new_v4().to_string()),
        )
        .unwrap();
        assert!(custom_bytes(&target_service, "cancel_api").is_some());
        assert_eq!(
            cancel_template_pack_import(CancelTemplatePackImportRequest {
                execution_id: Uuid::new_v4().to_string(),
            })
            .unwrap_err()
            .code,
            "TEMPLATE_PACK_EXECUTION_NOT_FOUND"
        );
    }

    #[test]
    fn execution_transport_shapes_are_strict_camel_case() {
        let execution_id = Uuid::new_v4().to_string();
        let request: ExecuteTemplatePackImportRequest = serde_json::from_value(serde_json::json!({
            "executionPlanToken": Uuid::new_v4().to_string(),
            "executionId": execution_id,
        }))
        .unwrap();
        assert!(valid_plan_token(&request.execution_id));
        assert!(
            serde_json::from_value::<ExecuteTemplatePackImportRequest>(serde_json::json!({
                "executionPlanToken": Uuid::new_v4().to_string(),
                "executionId": Uuid::new_v4().to_string(),
                "extra": true,
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<CancelTemplatePackImportRequest>(serde_json::json!({
                "execution_id": Uuid::new_v4().to_string(),
            }))
            .is_err()
        );
    }
}
