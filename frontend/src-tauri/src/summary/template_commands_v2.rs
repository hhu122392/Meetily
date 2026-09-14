use crate::database::repositories::meeting::MeetingsRepository;
use crate::state::AppState;
use crate::summary::metadata::{read_metadata_field, write_metadata_field};
use crate::summary::template_snapshot::{
    count_snapshots_for_template, list_snapshots, read_snapshot, ResolvedGenerationTemplate,
    SnapshotListItem, SnapshotResolutionSource, SnapshotTemplateOrigin,
};
use crate::summary::templates::{
    cancel_template_pack_import, content_locale_for_summary_language, execute_template_pack_import,
    export_template_pack, migrate_v1_to_v2, plan_template_pack_import, preview_template_document,
    preview_template_document_cancellable, preview_template_pack_export,
    preview_template_pack_import, recover_template_pack_imports, validate_template_v2,
    CancelTemplatePackImportRequest, CancelTemplatePackImportResponse, CreateTemplateRequest,
    DefaultTemplatePreferenceDto, DefaultTemplateResolutionSource, DeleteTemplateRequest,
    DeleteTemplateResponse, DeletedTemplateListItem, DocumentImportCancellation,
    DocumentImportConfidence, DocumentImportError, DocumentImportWarning, DocumentOutlineNode,
    DuplicateTemplateRequest, ExecuteTemplatePackImportRequest, ExecuteTemplatePackImportResponse,
    ExportTemplatePackRequest, ExportTemplatePackResponse, GetTemplateRequest,
    GetTemplateUsageRequest, ListTemplatesRequest, ListTemplatesResponse,
    PlanTemplatePackImportRequest, PlanTemplatePackImportResponse,
    PreviewTemplatePackExportRequest, PreviewTemplatePackExportResponse,
    PreviewTemplatePackImportRequest, PreviewTemplatePackImportResponse, PurgeTemplateRequest,
    RestoreTemplateRequest, SetDefaultTemplateRequest, Template, TemplateApiError,
    TemplateDetailsDto, TemplateOrigin, TemplateRepository, TemplateService, TemplateSource,
    TemplateSourceType, TemplateUsageDto, TemplateV2Dto, TemplatesDirectoryInfo,
    ValidateTemplateRequest, ValidationResultDto,
};
use chrono::Utc;
use once_cell::sync::Lazy;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tauri::{AppHandle, Runtime, State};
use tauri_plugin_store::StoreExt;

const TEMPLATE_PREFERENCE_STORE: &str = "summary-template-preferences.v1.json";
const DEFAULT_TEMPLATE_KEY: &str = "defaultTemplateId";
const MEETING_TEMPLATE_FIELD: &str = "summary_template";
const MEETING_TEMPLATE_KEY_PREFIX: &str = "meeting:";
const MAX_DOCUMENT_IMPORT_BATCH: usize = 50;
const MAX_JSON_IMPORT_BYTES: u64 = 1_048_576;
static MEETING_TEMPLATE_PREFERENCE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
static TEMPLATE_IMPORT_JOBS: Lazy<Mutex<HashMap<String, Arc<TemplateImportJobControl>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingTemplateMode {
    Inherit,
    MeetingOverride,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct PersistedMeetingTemplatePreference {
    pub schema_version: u8,
    pub mode: MeetingTemplateMode,
    pub template_id: Option<String>,
    pub template_version: Option<u64>,
    pub template_file_sha256: Option<String>,
    pub selected_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingTemplatePreferenceDto {
    pub schema_version: u8,
    pub mode: MeetingTemplateMode,
    pub template_id: Option<String>,
    pub template_version: Option<u64>,
    pub template_file_sha256: Option<String>,
    pub selected_at: String,
}

impl From<&PersistedMeetingTemplatePreference> for MeetingTemplatePreferenceDto {
    fn from(value: &PersistedMeetingTemplatePreference) -> Self {
        Self {
            schema_version: value.schema_version,
            mode: value.mode,
            template_id: value.template_id.clone(),
            template_version: value.template_version,
            template_file_sha256: value.template_file_sha256.clone(),
            selected_at: value.selected_at.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingTemplateStorage {
    Metadata,
    LocalFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingTemplateResolutionSource {
    MeetingOverride,
    GlobalDefault,
    BuiltinFallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedMeetingTemplateDto {
    pub template_id: String,
    pub name: String,
    pub version: u64,
    pub file_sha256: String,
    pub origin: TemplateOrigin,
    pub source: MeetingTemplateResolutionSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingTemplateIssueDto {
    pub code: String,
    pub selected_template_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingTemplatePreferenceResponse {
    pub preference: MeetingTemplatePreferenceDto,
    pub storage: MeetingTemplateStorage,
    pub resolved: ResolvedMeetingTemplateDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue: Option<MeetingTemplateIssueDto>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GetMeetingTemplatePreferenceRequest {
    pub meeting_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveMeetingTemplatePreferenceRequest {
    pub meeting_id: String,
    pub preference: SaveMeetingTemplatePreferenceValue,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveMeetingTemplatePreferenceValue {
    pub mode: MeetingTemplateMode,
    pub template_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListMeetingTemplateSnapshotsRequest {
    pub meeting_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewTemplateDocumentsRequest {
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default)]
    pub item_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateImportJobStatus {
    Accepted,
    Running,
    CancelRequested,
    Cancelled,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateImportItemStatus {
    Pending,
    Running,
    CancelRequested,
    Cancelled,
    Completed,
    Failed,
}

#[derive(Debug)]
struct TemplateImportJobControl {
    job_id: String,
    batch_cancelled: Arc<AtomicBool>,
    item_cancellations: HashMap<String, Arc<AtomicBool>>,
    status: Mutex<TemplateImportJobStatus>,
    item_statuses: Mutex<HashMap<String, TemplateImportItemStatus>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateImportJobItemDto {
    pub item_id: String,
    pub status: TemplateImportItemStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateImportJobDto {
    pub job_id: String,
    pub status: TemplateImportJobStatus,
    pub items: Vec<TemplateImportJobItemDto>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateImportJobRequest {
    pub job_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateImportItemCancellationRequest {
    pub job_id: String,
    pub item_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentImportPreviewDto {
    pub import_id: String,
    pub file_name: String,
    pub source_type: crate::summary::templates::TemplateSourceType,
    pub file_sha256: String,
    pub confidence: DocumentImportConfidence,
    pub outline: Vec<DocumentOutlineNode>,
    pub warnings: Vec<DocumentImportWarning>,
    pub draft: TemplateV2Dto,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewTemplateDocumentItemDto {
    pub item_id: String,
    pub file_name: String,
    pub status: TemplateImportItemStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<DocumentImportPreviewDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<TemplateApiError>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewTemplateDocumentsResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    pub status: TemplateImportJobStatus,
    pub items: Vec<PreviewTemplateDocumentItemDto>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExportTemplateJsonRequest {
    pub template_id: String,
    pub origin: Option<TemplateOrigin>,
    pub content_locale: Option<String>,
    pub destination_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportTemplateJsonResponse {
    pub file_name: String,
    pub bytes: usize,
    pub file_sha256: String,
}

pub struct TemplateServiceState {
    service: Result<TemplateService, TemplateApiError>,
}

impl TemplateServiceState {
    pub fn initialize(bundled_root: Option<PathBuf>) -> Self {
        let service = TemplateRepository::for_current_user(bundled_root)
            .map_err(TemplateApiError::from)
            .map(TemplateService::new)
            .and_then(|service| {
                recover_template_pack_imports(&service)?;
                Ok(service)
            });
        Self { service }
    }

    pub fn from_repository(repository: TemplateRepository) -> Self {
        let service = TemplateService::new(repository);
        Self {
            service: (|| {
                recover_template_pack_imports(&service)?;
                Ok(service)
            })(),
        }
    }

    pub(crate) fn service(&self) -> Result<TemplateService, TemplateApiError> {
        self.service.clone()
    }
}

async fn run_blocking<T, F>(
    state: State<'_, TemplateServiceState>,
    operation: F,
) -> Result<T, TemplateApiError>
where
    T: Send + 'static,
    F: FnOnce(TemplateService) -> Result<T, TemplateApiError> + Send + 'static,
{
    let service = state.service()?;
    tauri::async_runtime::spawn_blocking(move || operation(service))
        .await
        .map_err(|error| internal_error("template service task failed", error))?
}

#[tauri::command]
pub async fn api_get_templates_directory(
    state: State<'_, TemplateServiceState>,
) -> Result<TemplatesDirectoryInfo, TemplateApiError> {
    run_blocking(state, |service| service.directory_info()).await
}

#[tauri::command]
pub async fn api_open_templates_directory(
    state: State<'_, TemplateServiceState>,
) -> Result<(), TemplateApiError> {
    let root = state.service()?.repository().root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || open_directory(root))
        .await
        .map_err(|error| internal_error("open templates directory task failed", error))?
}

#[tauri::command]
pub async fn api_preview_template_documents(
    request: Value,
) -> Result<PreviewTemplateDocumentsResponse, TemplateApiError> {
    let request = parse_request::<PreviewTemplateDocumentsRequest>(request)?;
    let unique_paths = validate_import_paths(request.paths)?;

    tauri::async_runtime::spawn_blocking(move || {
        let items = unique_paths
            .into_iter()
            .map(|path_value| {
                let path = PathBuf::from(&path_value);
                let file_name = safe_selected_file_name(&path);
                let item_id = uuid::Uuid::new_v4().to_string();
                match preview_template_document(&path) {
                    Ok(preview) => PreviewTemplateDocumentItemDto {
                        item_id,
                        file_name,
                        status: TemplateImportItemStatus::Completed,
                        preview: Some(DocumentImportPreviewDto {
                            import_id: preview.import_id,
                            file_name: preview.file_name,
                            source_type: preview.source_type,
                            file_sha256: preview.file_sha256,
                            confidence: preview.confidence,
                            outline: preview.outline,
                            warnings: preview.warnings,
                            draft: TemplateV2Dto::from(&preview.draft),
                        }),
                        error: None,
                    },
                    Err(error) => PreviewTemplateDocumentItemDto {
                        item_id,
                        file_name,
                        status: TemplateImportItemStatus::Failed,
                        preview: None,
                        error: Some(document_import_api_error(error)),
                    },
                }
            })
            .collect();
        PreviewTemplateDocumentsResponse {
            job_id: None,
            status: TemplateImportJobStatus::Completed,
            items,
        }
    })
    .await
    .map_err(|error| internal_error("document import preview task failed", error))
}

#[tauri::command]
pub async fn api_preview_template_imports(
    request: Value,
) -> Result<PreviewTemplateDocumentsResponse, TemplateApiError> {
    let request = parse_request::<PreviewTemplateDocumentsRequest>(request)?;
    let (job_id, work_items) = prepare_template_import_job(request)?;
    let job = register_template_import_job(&job_id, &work_items)?;
    set_template_import_job_status(&job, TemplateImportJobStatus::Running);

    let task_job_id = job_id.clone();
    let task_job = Arc::clone(&job);
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut items = Vec::with_capacity(work_items.len());
        for (item_id, path_value) in work_items {
            let cancellation = template_import_item_cancellation(&task_job, &item_id);
            let path = PathBuf::from(&path_value);
            let file_name = safe_selected_file_name(&path);
            if cancellation.is_cancelled() {
                set_template_import_item_status(
                    &task_job,
                    &item_id,
                    TemplateImportItemStatus::Cancelled,
                );
                items.push(cancelled_import_item(item_id, file_name));
                continue;
            }
            set_template_import_item_status(&task_job, &item_id, TemplateImportItemStatus::Running);
            let preview = if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
            {
                preview_template_json_cancellable(&path, &cancellation)
            } else {
                preview_template_document_cancellable(&path, &cancellation)
                    .map(|preview| DocumentImportPreviewDto {
                        import_id: preview.import_id,
                        file_name: preview.file_name,
                        source_type: preview.source_type,
                        file_sha256: preview.file_sha256,
                        confidence: preview.confidence,
                        outline: preview.outline,
                        warnings: preview.warnings,
                        draft: TemplateV2Dto::from(&preview.draft),
                    })
                    .map_err(document_import_api_error)
            };

            let item = match preview {
                Ok(preview) => PreviewTemplateDocumentItemDto {
                    item_id: item_id.clone(),
                    file_name,
                    status: TemplateImportItemStatus::Completed,
                    preview: Some(preview),
                    error: None,
                },
                Err(error) if error.code == "TEMPLATE_CANCELLED" => {
                    cancelled_import_item(item_id.clone(), file_name)
                }
                Err(error) => PreviewTemplateDocumentItemDto {
                    item_id: item_id.clone(),
                    file_name,
                    status: TemplateImportItemStatus::Failed,
                    preview: None,
                    error: Some(error),
                },
            };
            set_template_import_item_status(&task_job, &item_id, item.status);
            items.push(item);
        }
        let status = if task_job.batch_cancelled.load(Ordering::Acquire) {
            TemplateImportJobStatus::Cancelled
        } else {
            TemplateImportJobStatus::Completed
        };
        set_template_import_job_status(&task_job, status);
        PreviewTemplateDocumentsResponse {
            job_id: Some(task_job_id),
            status,
            items,
        }
    })
    .await
    .map_err(|error| internal_error("template import preview task failed", error));
    if result.is_err() {
        set_template_import_job_status(&job, TemplateImportJobStatus::Completed);
    }
    result
}

#[tauri::command]
pub async fn api_get_template_import_job(
    request: Value,
) -> Result<TemplateImportJobDto, TemplateApiError> {
    let request = parse_request::<TemplateImportJobRequest>(request)?;
    let job = get_template_import_job(&request.job_id)?;
    Ok(template_import_job_snapshot(&job))
}

#[tauri::command]
pub async fn api_cancel_template_import_job(
    request: Value,
) -> Result<TemplateImportJobDto, TemplateApiError> {
    let request = parse_request::<TemplateImportJobRequest>(request)?;
    let job = get_template_import_job(&request.job_id)?;
    cancel_template_import_job(&job);
    Ok(template_import_job_snapshot(&job))
}

#[tauri::command]
pub async fn api_cancel_template_import_item(
    request: Value,
) -> Result<TemplateImportJobDto, TemplateApiError> {
    let request = parse_request::<TemplateImportItemCancellationRequest>(request)?;
    let job = get_template_import_job(&request.job_id)?;
    cancel_template_import_item(&job, &request.item_id)?;
    Ok(template_import_job_snapshot(&job))
}

#[tauri::command]
pub async fn api_export_template_json(
    state: State<'_, TemplateServiceState>,
    request: Value,
) -> Result<ExportTemplateJsonResponse, TemplateApiError> {
    let request = parse_request::<ExportTemplateJsonRequest>(request)?;
    if request.destination_path.trim().is_empty() {
        return Err(request_deserialization_error(
            "destinationPath must be a non-empty JSON file path",
        ));
    }
    let destination = PathBuf::from(&request.destination_path);
    if !destination
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
    {
        return Err(TemplateApiError::from_code("TEMPLATE_EXPORT_FAILED"));
    }
    let service = state.service()?;

    tauri::async_runtime::spawn_blocking(move || {
        export_template_json_file(&service, request, destination)
    })
    .await
    .map_err(|error| internal_error("template JSON export task failed", error))?
}

#[tauri::command]
pub async fn api_preview_template_pack_export(
    state: State<'_, TemplateServiceState>,
    request: Value,
) -> Result<PreviewTemplatePackExportResponse, TemplateApiError> {
    let request = parse_request::<PreviewTemplatePackExportRequest>(request)?;
    run_blocking(state, move |service| {
        preview_template_pack_export(&service, request)
    })
    .await
}

#[tauri::command]
pub async fn api_export_template_pack(
    state: State<'_, TemplateServiceState>,
    request: Value,
) -> Result<ExportTemplatePackResponse, TemplateApiError> {
    let request = parse_request::<ExportTemplatePackRequest>(request)?;
    run_blocking(state, move |service| {
        export_template_pack(&service, request)
    })
    .await
}

#[tauri::command]
pub async fn api_preview_template_pack_import(
    state: State<'_, TemplateServiceState>,
    request: Value,
) -> Result<PreviewTemplatePackImportResponse, TemplateApiError> {
    let request = parse_request::<PreviewTemplatePackImportRequest>(request)?;
    run_blocking(state, move |service| {
        preview_template_pack_import(&service, request)
    })
    .await
}

#[tauri::command]
pub async fn api_plan_template_pack_import(
    state: State<'_, TemplateServiceState>,
    request: Value,
) -> Result<PlanTemplatePackImportResponse, TemplateApiError> {
    let request = parse_request::<PlanTemplatePackImportRequest>(request)?;
    run_blocking(state, move |service| {
        plan_template_pack_import(&service, request)
    })
    .await
}

#[tauri::command]
pub async fn api_execute_template_pack_import(
    state: State<'_, TemplateServiceState>,
    request: Value,
) -> Result<ExecuteTemplatePackImportResponse, TemplateApiError> {
    let request = parse_request::<ExecuteTemplatePackImportRequest>(request)?;
    run_blocking(state, move |service| {
        execute_template_pack_import(&service, request)
    })
    .await
}

#[tauri::command]
pub async fn api_cancel_template_pack_import(
    request: Value,
) -> Result<CancelTemplatePackImportResponse, TemplateApiError> {
    let request = parse_request::<CancelTemplatePackImportRequest>(request)?;
    cancel_template_pack_import(request)
}

fn export_template_json_file(
    service: &TemplateService,
    request: ExportTemplateJsonRequest,
    destination: PathBuf,
) -> Result<ExportTemplateJsonResponse, TemplateApiError> {
    let parent = destination
        .parent()
        .filter(|parent| parent.is_dir())
        .ok_or_else(|| TemplateApiError::from_code("TEMPLATE_EXPORT_FAILED"))?;
    if std::fs::symlink_metadata(&destination)
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(TemplateApiError::from_code("TEMPLATE_PATH_REJECTED"));
    }
    let record = service
        .repository()
        .get_for_content_locale(
            &request.template_id,
            request.origin,
            request.content_locale.as_deref(),
        )
        .map_err(TemplateApiError::from)?;
    let validation = validate_template_v2(&record.template);
    if !validation.valid {
        let mut error = TemplateApiError::from_code("TEMPLATE_INVALID");
        error.field_errors = validation.errors.into_iter().map(Into::into).collect();
        return Err(error);
    }
    let mut bytes = serde_json::to_vec_pretty(&record.template)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_EXPORT_FAILED"))?;
    bytes.push(b'\n');
    let file_sha256 = format!("{:x}", Sha256::digest(&bytes));
    let mut output = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_EXPORT_FAILED"))?;
    output
        .write_all(&bytes)
        .and_then(|_| output.as_file().sync_all())
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_EXPORT_FAILED"))?;
    output
        .persist(&destination)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_EXPORT_FAILED"))?;
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("template.json")
        .to_owned();
    Ok(ExportTemplateJsonResponse {
        file_name,
        bytes: bytes.len(),
        file_sha256,
    })
}

fn validate_import_paths(paths: Vec<String>) -> Result<Vec<String>, TemplateApiError> {
    if paths.is_empty() || paths.len() > MAX_DOCUMENT_IMPORT_BATCH {
        return Err(request_deserialization_error(format!(
            "paths must contain between 1 and {MAX_DOCUMENT_IMPORT_BATCH} entries"
        )));
    }
    let mut unique_paths = Vec::with_capacity(paths.len());
    let mut seen = HashSet::with_capacity(paths.len());
    for path in paths {
        if path.trim().is_empty() || !seen.insert(path.clone()) {
            return Err(request_deserialization_error(
                "paths must be non-empty and must not contain duplicates",
            ));
        }
        unique_paths.push(path);
    }
    Ok(unique_paths)
}

fn prepare_template_import_job(
    request: PreviewTemplateDocumentsRequest,
) -> Result<(String, Vec<(String, String)>), TemplateApiError> {
    let paths = validate_import_paths(request.paths)?;
    let job_id = request
        .job_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    validate_import_identifier(&job_id, "jobId")?;
    let item_ids = match request.item_ids {
        Some(item_ids) if item_ids.len() == paths.len() => item_ids,
        Some(_) => {
            return Err(request_deserialization_error(
                "itemIds must have the same number of entries as paths",
            ))
        }
        None => paths
            .iter()
            .map(|_| uuid::Uuid::new_v4().to_string())
            .collect(),
    };
    let mut seen = HashSet::with_capacity(item_ids.len());
    for item_id in &item_ids {
        validate_import_identifier(item_id, "itemId")?;
        if !seen.insert(item_id.clone()) {
            return Err(request_deserialization_error(
                "itemIds must not contain duplicates",
            ));
        }
    }
    Ok((job_id, item_ids.into_iter().zip(paths).collect()))
}

fn validate_import_identifier(value: &str, field: &str) -> Result<(), TemplateApiError> {
    if value.trim().is_empty() || value.len() > 128 {
        return Err(request_deserialization_error(format!(
            "{field} must contain between 1 and 128 characters"
        )));
    }
    Ok(())
}

fn register_template_import_job(
    job_id: &str,
    work_items: &[(String, String)],
) -> Result<Arc<TemplateImportJobControl>, TemplateApiError> {
    let mut jobs = TEMPLATE_IMPORT_JOBS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(existing) = jobs.get(job_id) {
        let status = *existing
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if matches!(
            status,
            TemplateImportJobStatus::Accepted
                | TemplateImportJobStatus::Running
                | TemplateImportJobStatus::CancelRequested
        ) {
            return Err(TemplateApiError::from_code("TEMPLATE_CONFLICT"));
        }
    }
    if jobs.len() >= 128 {
        jobs.retain(|_, job| {
            matches!(
                *job.status
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
                TemplateImportJobStatus::Accepted
                    | TemplateImportJobStatus::Running
                    | TemplateImportJobStatus::CancelRequested
            )
        });
    }
    let item_cancellations = work_items
        .iter()
        .map(|(item_id, _)| (item_id.clone(), Arc::new(AtomicBool::new(false))))
        .collect::<HashMap<_, _>>();
    let item_statuses = work_items
        .iter()
        .map(|(item_id, _)| (item_id.clone(), TemplateImportItemStatus::Pending))
        .collect::<HashMap<_, _>>();
    let job = Arc::new(TemplateImportJobControl {
        job_id: job_id.to_owned(),
        batch_cancelled: Arc::new(AtomicBool::new(false)),
        item_cancellations,
        status: Mutex::new(TemplateImportJobStatus::Accepted),
        item_statuses: Mutex::new(item_statuses),
    });
    jobs.insert(job_id.to_owned(), Arc::clone(&job));
    Ok(job)
}

fn get_template_import_job(
    job_id: &str,
) -> Result<Arc<TemplateImportJobControl>, TemplateApiError> {
    TEMPLATE_IMPORT_JOBS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(job_id)
        .cloned()
        .ok_or_else(|| TemplateApiError::from_code("TEMPLATE_NOT_FOUND"))
}

fn set_template_import_job_status(job: &TemplateImportJobControl, status: TemplateImportJobStatus) {
    *job.status
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = status;
}

fn set_template_import_item_status(
    job: &TemplateImportJobControl,
    item_id: &str,
    status: TemplateImportItemStatus,
) {
    if let Some(current) = job
        .item_statuses
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get_mut(item_id)
    {
        *current = status;
    }
}

fn template_import_item_cancellation(
    job: &TemplateImportJobControl,
    item_id: &str,
) -> DocumentImportCancellation {
    let mut signals = vec![Arc::clone(&job.batch_cancelled)];
    if let Some(item) = job.item_cancellations.get(item_id) {
        signals.push(Arc::clone(item));
    }
    DocumentImportCancellation::from_signals(signals)
}

fn cancel_template_import_job(job: &TemplateImportJobControl) {
    let current_status = *job
        .status
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if matches!(
        current_status,
        TemplateImportJobStatus::Cancelled | TemplateImportJobStatus::Completed
    ) {
        return;
    }
    job.batch_cancelled.store(true, Ordering::Release);
    for signal in job.item_cancellations.values() {
        signal.store(true, Ordering::Release);
    }
    set_template_import_job_status(job, TemplateImportJobStatus::CancelRequested);
    let mut statuses = job
        .item_statuses
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for status in statuses.values_mut() {
        if matches!(
            *status,
            TemplateImportItemStatus::Pending | TemplateImportItemStatus::Running
        ) {
            *status = TemplateImportItemStatus::CancelRequested;
        }
    }
}

fn cancel_template_import_item(
    job: &TemplateImportJobControl,
    item_id: &str,
) -> Result<(), TemplateApiError> {
    let signal = job
        .item_cancellations
        .get(item_id)
        .ok_or_else(|| TemplateApiError::from_code("TEMPLATE_NOT_FOUND"))?;
    let current_status = *job
        .status
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if matches!(
        current_status,
        TemplateImportJobStatus::Cancelled | TemplateImportJobStatus::Completed
    ) {
        return Ok(());
    }
    signal.store(true, Ordering::Release);
    let mut statuses = job
        .item_statuses
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(status) = statuses.get_mut(item_id) {
        if matches!(
            *status,
            TemplateImportItemStatus::Pending | TemplateImportItemStatus::Running
        ) {
            *status = TemplateImportItemStatus::CancelRequested;
        }
    }
    Ok(())
}

fn template_import_job_snapshot(job: &TemplateImportJobControl) -> TemplateImportJobDto {
    let status = *job
        .status
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut items = job
        .item_statuses
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .map(|(item_id, status)| TemplateImportJobItemDto {
            item_id: item_id.clone(),
            status: *status,
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| left.item_id.cmp(&right.item_id));
    TemplateImportJobDto {
        job_id: job.job_id.clone(),
        status,
        items,
    }
}

fn cancelled_import_item(item_id: String, file_name: String) -> PreviewTemplateDocumentItemDto {
    PreviewTemplateDocumentItemDto {
        item_id,
        file_name,
        status: TemplateImportItemStatus::Cancelled,
        preview: None,
        error: Some(TemplateApiError::from_code("TEMPLATE_CANCELLED")),
    }
}

#[cfg(test)]
fn preview_template_json(path: &Path) -> Result<DocumentImportPreviewDto, TemplateApiError> {
    preview_template_json_cancellable(path, &DocumentImportCancellation::default())
}

fn preview_template_json_cancellable(
    path: &Path,
    cancellation: &DocumentImportCancellation,
) -> Result<DocumentImportPreviewDto, TemplateApiError> {
    cancellation
        .checkpoint()
        .map_err(document_import_api_error)?;
    let metadata =
        std::fs::metadata(path).map_err(|_| TemplateApiError::from_code("TEMPLATE_IO_ERROR"))?;
    if !metadata.is_file() {
        return Err(TemplateApiError::from_code("TEMPLATE_IO_ERROR"));
    }
    if metadata.len() > MAX_JSON_IMPORT_BYTES {
        return Err(TemplateApiError::from_code("TEMPLATE_ARCHIVE_TOO_LARGE")
            .with_param("maxBytes", MAX_JSON_IMPORT_BYTES));
    }
    let mut file =
        std::fs::File::open(path).map_err(|_| TemplateApiError::from_code("TEMPLATE_IO_ERROR"))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        cancellation
            .checkpoint()
            .map_err(document_import_api_error)?;
        let read = file
            .read(&mut buffer)
            .map_err(|_| TemplateApiError::from_code("TEMPLATE_IO_ERROR"))?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        if bytes.len() as u64 > MAX_JSON_IMPORT_BYTES {
            return Err(TemplateApiError::from_code("TEMPLATE_ARCHIVE_TOO_LARGE")
                .with_param("maxBytes", MAX_JSON_IMPORT_BYTES));
        }
    }
    cancellation
        .checkpoint()
        .map_err(document_import_api_error)?;
    let file_sha256 = format!("{:x}", Sha256::digest(&bytes));
    let content = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    let text = std::str::from_utf8(content)
        .map_err(|_| TemplateApiError::from_code("TEMPLATE_JSON_ENCODING_INVALID"))?;
    let value: Value = serde_json::from_str(text).map_err(|parse_error| {
        TemplateApiError::from_code("TEMPLATE_JSON_INVALID")
            .with_param("line", parse_error.line() as u64)
            .with_param("column", parse_error.column() as u64)
    })?;
    cancellation
        .checkpoint()
        .map_err(document_import_api_error)?;
    let now = Utc::now().fixed_offset();
    let file_name = safe_selected_file_name(path);
    let schema_version_value = value.get("schema_version");
    let explicit_schema_version = schema_version_value.and_then(Value::as_u64);
    let schema_version_on_disk = explicit_schema_version.unwrap_or(1);
    if schema_version_value.is_some() && !matches!(explicit_schema_version, Some(1) | Some(2)) {
        let validation = crate::summary::templates::validate_template_v2_value(&value);
        let mut error = TemplateApiError::from_code("TEMPLATE_INVALID");
        error.field_errors = validation.errors.into_iter().map(Into::into).collect();
        return Err(error);
    }
    let mut warnings = Vec::new();
    let mut template = if schema_version_on_disk == 2 {
        let validation = crate::summary::templates::validate_template_v2_value(&value);
        if !validation.valid {
            let mut error = TemplateApiError::from_code("TEMPLATE_INVALID");
            error.field_errors = validation.errors.into_iter().map(Into::into).collect();
            return Err(error);
        }
        warnings.extend(
            validation
                .warnings
                .into_iter()
                .map(|warning| DocumentImportWarning {
                    code: warning.code,
                    message_key: warning.message_key,
                    params: warning.params,
                }),
        );
        serde_json::from_value(value)
            .map_err(|_| TemplateApiError::from_code("TEMPLATE_JSON_INVALID"))?
    } else {
        let legacy: Template = serde_json::from_value(value)
            .map_err(|_| TemplateApiError::from_code("TEMPLATE_JSON_INVALID"))?;
        let id_hint = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("imported_template");
        warnings.push(DocumentImportWarning {
            code: "JSON_V1_MIGRATED".to_owned(),
            message_key: "templates.import.warnings.v1Migrated".to_owned(),
            params: Default::default(),
        });
        migrate_v1_to_v2(id_hint, &legacy, now).map_err(|validation| {
            let mut error = TemplateApiError::from_code("TEMPLATE_INVALID");
            error.field_errors = validation.errors.into_iter().map(Into::into).collect();
            error
        })?
    };
    template.version = 1;
    template.created_at = now;
    template.updated_at = now;
    template.source = TemplateSource {
        source_type: TemplateSourceType::JsonImport,
        original_file_name: Some(file_name.clone()),
        original_file_sha256: Some(file_sha256.clone()),
        imported_at: Some(now),
        copied_from_template_id: None,
    };

    Ok(DocumentImportPreviewDto {
        import_id: uuid::Uuid::new_v4().to_string(),
        file_name,
        source_type: TemplateSourceType::JsonImport,
        file_sha256,
        confidence: DocumentImportConfidence::High,
        outline: Vec::new(),
        warnings,
        draft: TemplateV2Dto::from(&template),
    })
}

fn safe_selected_file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("selected-document")
        .to_owned()
}

fn document_import_api_error(error: DocumentImportError) -> TemplateApiError {
    let mut api_error = TemplateApiError::from_code(error.code);
    api_error.params = error.params;
    tracing::warn!(
        debug_id = %api_error.debug_id,
        code = %api_error.code,
        detail = %error.detail,
        "template document preview failed"
    );
    api_error
}

#[tauri::command]
pub async fn api_list_templates_v2<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, TemplateServiceState>,
    request: Value,
) -> Result<ListTemplatesResponse, TemplateApiError> {
    let request = parse_request::<ListTemplatesRequest>(request)?;
    let default_template_id = load_default_template_id(&app)?;
    run_blocking(state, move |service| {
        service.list(request, default_template_id.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn api_get_template_v2<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, TemplateServiceState>,
    request: Value,
) -> Result<TemplateDetailsDto, TemplateApiError> {
    let request = parse_request::<GetTemplateRequest>(request)?;
    let default_template_id = load_default_template_id(&app)?;
    run_blocking(state, move |service| {
        service.get(request, default_template_id.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn api_validate_template_v2(
    state: State<'_, TemplateServiceState>,
    request: Value,
) -> Result<ValidationResultDto, TemplateApiError> {
    let request = parse_request::<ValidateTemplateRequest>(request)?;
    run_blocking(state, move |service| Ok(service.validate(request))).await
}

#[tauri::command]
pub async fn api_create_template<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, TemplateServiceState>,
    request: Value,
) -> Result<TemplateDetailsDto, TemplateApiError> {
    let request = parse_request::<CreateTemplateRequest>(request)?;
    let default_template_id = load_default_template_id(&app)?;
    run_blocking(state, move |service| {
        service.create(request, default_template_id.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn api_update_template<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, TemplateServiceState>,
    request: Value,
) -> Result<TemplateDetailsDto, TemplateApiError> {
    let request = parse_request::<crate::summary::templates::UpdateTemplateRequest>(request)?;
    let default_template_id = load_default_template_id(&app)?;
    run_blocking(state, move |service| {
        service.update(request, default_template_id.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn api_duplicate_template<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, TemplateServiceState>,
    request: Value,
) -> Result<TemplateDetailsDto, TemplateApiError> {
    let request = parse_request::<DuplicateTemplateRequest>(request)?;
    let default_template_id = load_default_template_id(&app)?;
    run_blocking(state, move |service| {
        service.duplicate(request, default_template_id.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn api_get_template_usage<R: Runtime>(
    app: AppHandle<R>,
    app_state: State<'_, AppState>,
    state: State<'_, TemplateServiceState>,
    request: Value,
) -> Result<TemplateUsageDto, TemplateApiError> {
    let request = parse_request::<GetTemplateUsageRequest>(request)?;
    let default_template_id = load_default_template_id(&app)?;
    let meetings = MeetingsRepository::get_meetings(app_state.db_manager.pool())
        .await
        .map_err(|error| internal_error("meeting template usage could not be loaded", error))?;
    let service = state.service()?;
    let _guard = MEETING_TEMPLATE_PREFERENCE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut usage = service.usage(&request.template_id, default_template_id.as_deref())?;
    usage.current_meeting_preference_count =
        count_current_meeting_preferences(&app, &meetings, &request.template_id)?;
    usage.historical_snapshot_count = count_historical_snapshots(&meetings, &request.template_id)?;
    Ok(usage)
}

#[tauri::command]
pub async fn api_list_meeting_template_snapshots(
    app_state: State<'_, AppState>,
    request: Value,
) -> Result<Vec<SnapshotListItem>, TemplateApiError> {
    let request = parse_request::<ListMeetingTemplateSnapshotsRequest>(request)?;
    validate_meeting_id(&request.meeting_id)?;
    let folder = meeting_folder(app_state.db_manager.pool(), &request.meeting_id)
        .await?
        .ok_or_else(|| snapshot_error("meeting folder is unavailable", None::<String>))?;
    list_snapshots(&folder, &request.meeting_id).map_err(|error| {
        snapshot_error(
            "meeting template snapshots could not be listed",
            Some(error),
        )
    })
}

#[tauri::command]
pub async fn api_delete_template<R: Runtime>(
    app: AppHandle<R>,
    app_state: State<'_, AppState>,
    state: State<'_, TemplateServiceState>,
    request: Value,
) -> Result<DeleteTemplateResponse, TemplateApiError> {
    let request = parse_request::<DeleteTemplateRequest>(request)?;
    let default_template_id = load_default_template_id(&app)?;
    let meetings = MeetingsRepository::get_meetings(app_state.db_manager.pool())
        .await
        .map_err(|error| internal_error("meeting template usage could not be loaded", error))?;
    let service = state.service()?;
    let _guard = MEETING_TEMPLATE_PREFERENCE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let current_meeting_preference_count =
        count_current_meeting_preferences(&app, &meetings, &request.template_id)?;
    if current_meeting_preference_count > 0 {
        return Err(TemplateApiError::from_code("TEMPLATE_IN_USE")
            .with_param("templateId", request.template_id));
    }
    service.delete(request, default_template_id.as_deref())
}

#[tauri::command]
pub async fn api_list_deleted_templates(
    state: State<'_, TemplateServiceState>,
) -> Result<Vec<DeletedTemplateListItem>, TemplateApiError> {
    run_blocking(state, |service| service.list_deleted()).await
}

#[tauri::command]
pub async fn api_restore_template<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, TemplateServiceState>,
    request: Value,
) -> Result<TemplateDetailsDto, TemplateApiError> {
    let request = parse_request::<RestoreTemplateRequest>(request)?;
    let default_template_id = load_default_template_id(&app)?;
    run_blocking(state, move |service| {
        service.restore(request, default_template_id.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn api_purge_template(
    state: State<'_, TemplateServiceState>,
    request: Value,
) -> Result<(), TemplateApiError> {
    let request = parse_request::<PurgeTemplateRequest>(request)?;
    run_blocking(state, move |service| service.purge(request)).await
}

#[tauri::command]
pub async fn api_get_default_template<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, TemplateServiceState>,
) -> Result<DefaultTemplatePreferenceDto, TemplateApiError> {
    let stored_template_id = load_default_template_id(&app)?;
    run_blocking(state, move |service| {
        Ok(service.get_default(stored_template_id))
    })
    .await
}

#[tauri::command]
pub async fn api_set_default_template<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, TemplateServiceState>,
    request: Value,
) -> Result<DefaultTemplatePreferenceDto, TemplateApiError> {
    let request = parse_request::<SetDefaultTemplateRequest>(request)?;
    let requested_id = match request.template_id {
        Value::Null => None,
        Value::String(template_id) => Some(template_id),
        _ => {
            return Err(request_deserialization_error(
                "templateId must be a string or null",
            ));
        }
    };
    let validated = run_blocking(state, {
        let requested_id = requested_id.clone();
        move |service| service.validate_default(requested_id)
    })
    .await?;
    save_default_template_id(&app, validated.template_id.as_deref())?;
    Ok(validated)
}

#[tauri::command]
pub async fn api_get_meeting_template_preference<R: Runtime>(
    app: AppHandle<R>,
    app_state: State<'_, AppState>,
    template_state: State<'_, TemplateServiceState>,
    request: Value,
) -> Result<MeetingTemplatePreferenceResponse, TemplateApiError> {
    let request = parse_request::<GetMeetingTemplatePreferenceRequest>(request)?;
    validate_meeting_id(&request.meeting_id)?;
    let folder = meeting_folder(app_state.db_manager.pool(), &request.meeting_id).await?;
    let service = template_state.service()?;
    let stored_default = load_default_template_id(&app)?;

    let _guard = MEETING_TEMPLATE_PREFERENCE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (preference, storage) =
        load_preference_with_migration(&app, folder.as_deref(), &request.meeting_id)?;
    resolve_meeting_template_preference(&service, stored_default, preference, storage)
}

#[tauri::command]
pub async fn api_save_meeting_template_preference<R: Runtime>(
    app: AppHandle<R>,
    app_state: State<'_, AppState>,
    template_state: State<'_, TemplateServiceState>,
    request: Value,
) -> Result<MeetingTemplatePreferenceResponse, TemplateApiError> {
    let request = parse_request::<SaveMeetingTemplatePreferenceRequest>(request)?;
    validate_meeting_id(&request.meeting_id)?;
    let folder = meeting_folder(app_state.db_manager.pool(), &request.meeting_id).await?;
    let service = template_state.service()?;
    let stored_default = load_default_template_id(&app)?;
    let preference =
        build_meeting_preference(&service, stored_default.as_deref(), request.preference)?;

    let _guard = MEETING_TEMPLATE_PREFERENCE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let storage = match folder.as_deref() {
        Some(folder) => {
            write_preference_to_metadata(folder, &preference)?;
            delete_fallback_preference(&app, &request.meeting_id)?;
            MeetingTemplateStorage::Metadata
        }
        None => {
            save_fallback_preference(&app, &request.meeting_id, &preference)?;
            MeetingTemplateStorage::LocalFallback
        }
    };

    resolve_meeting_template_preference(&service, stored_default, preference, storage)
}

async fn meeting_folder(
    pool: &sqlx::SqlitePool,
    meeting_id: &str,
) -> Result<Option<PathBuf>, TemplateApiError> {
    let meeting = MeetingsRepository::get_meeting_metadata(pool, meeting_id)
        .await
        .map_err(|error| internal_error("meeting metadata could not be loaded", error))?
        .ok_or_else(|| {
            TemplateApiError::from_code("MEETING_NOT_FOUND")
                .with_param("meetingId", meeting_id.to_owned())
        })?;
    Ok(meeting
        .folder_path
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_dir()))
}

fn count_current_meeting_preferences<R: Runtime>(
    app: &AppHandle<R>,
    meetings: &[crate::database::models::MeetingModel],
    template_id: &str,
) -> Result<usize, TemplateApiError> {
    let mut count = 0;
    for meeting in meetings {
        let folder = meeting
            .folder_path
            .as_deref()
            .filter(|path| !path.trim().is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_dir());
        let (preference, _) = load_preference_with_migration(app, folder.as_deref(), &meeting.id)?;
        if preference.mode == MeetingTemplateMode::MeetingOverride
            && preference.template_id.as_deref() == Some(template_id)
        {
            count += 1;
        }
    }
    Ok(count)
}

fn count_historical_snapshots(
    meetings: &[crate::database::models::MeetingModel],
    template_id: &str,
) -> Result<usize, TemplateApiError> {
    let mut count = 0usize;
    for meeting in meetings {
        let Some(folder) = meeting
            .folder_path
            .as_deref()
            .filter(|path| !path.trim().is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
        else {
            continue;
        };
        count +=
            count_snapshots_for_template(&folder, &meeting.id, template_id).map_err(|error| {
                snapshot_error(
                    "historical template usage could not be counted",
                    Some(error),
                )
            })?;
    }
    Ok(count)
}

pub(crate) async fn resolve_template_for_generation<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::SqlitePool,
    template_state: &TemplateServiceState,
    meeting_id: &str,
    requested_template_id: Option<&str>,
    historical_generation_id: Option<&str>,
    summary_language: Option<&str>,
) -> Result<(PathBuf, ResolvedGenerationTemplate), TemplateApiError> {
    validate_meeting_id(meeting_id)?;
    let folder = meeting_folder(pool, meeting_id)
        .await?
        .ok_or_else(|| snapshot_error("meeting folder is unavailable", None::<String>))?;

    if let Some(generation_id) = historical_generation_id {
        let snapshot = read_snapshot(&folder, meeting_id, generation_id).map_err(|error| {
            snapshot_error(
                "historical template snapshot could not be resolved",
                Some(error),
            )
            .with_param("generationId", generation_id.to_owned())
        })?;
        return Ok((folder, ResolvedGenerationTemplate::from_snapshot(&snapshot)));
    }

    let service = template_state.service()?;
    let stored_default = load_default_template_id(app)?;
    let _guard = MEETING_TEMPLATE_PREFERENCE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (preference, _) = load_preference_with_migration(app, Some(&folder), meeting_id)?;

    let (template_id, resolution_source) = match preference.mode {
        MeetingTemplateMode::MeetingOverride => (
            preference
                .template_id
                .ok_or_else(|| preference_data_error("meeting_override is missing template_id"))?,
            SnapshotResolutionSource::MeetingOverride,
        ),
        MeetingTemplateMode::Inherit => {
            let default = service.get_default(stored_default);
            let source = match default.resolution_source {
                DefaultTemplateResolutionSource::UserDefault => {
                    SnapshotResolutionSource::GlobalDefault
                }
                DefaultTemplateResolutionSource::BuiltinFallback => {
                    SnapshotResolutionSource::BuiltinFallback
                }
            };
            (default.resolved_template_id, source)
        }
    };

    if let Some(requested) = requested_template_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if requested != template_id {
            return Err(TemplateApiError::from_code("TEMPLATE_CONFLICT")
                .with_param("requestedTemplateId", requested.to_owned())
                .with_param("resolvedTemplateId", template_id));
        }
    }

    let content_locale = content_locale_for_summary_language(summary_language, None);
    let record = service
        .repository()
        .get_for_content_locale(&template_id, None, Some(content_locale))
        .map_err(TemplateApiError::from)?;
    let runtime_template = record.template.to_runtime_template();
    Ok((
        folder,
        ResolvedGenerationTemplate {
            template: record.template,
            runtime_template,
            origin: SnapshotTemplateOrigin::from(record.origin),
            resolution_source,
            file_sha256: record.file_sha256,
            semantic_sha256: record.semantic_sha256,
        },
    ))
}

fn snapshot_error(detail: &str, source: Option<impl std::fmt::Display>) -> TemplateApiError {
    let error = TemplateApiError::from_code("MEETING_TEMPLATE_SNAPSHOT_FAILED");
    match source {
        Some(source) => tracing::warn!(
            debug_id = %error.debug_id,
            code = %error.code,
            detail,
            source = %source,
            "meeting template snapshot operation failed"
        ),
        None => tracing::warn!(
            debug_id = %error.debug_id,
            code = %error.code,
            detail,
            "meeting template snapshot operation failed"
        ),
    }
    error
}

fn build_meeting_preference(
    service: &TemplateService,
    default_template_id: Option<&str>,
    requested: SaveMeetingTemplatePreferenceValue,
) -> Result<PersistedMeetingTemplatePreference, TemplateApiError> {
    let selected_at = Utc::now().to_rfc3339();
    match requested.mode {
        MeetingTemplateMode::Inherit => {
            if requested.template_id.is_some() {
                return Err(request_deserialization_error(
                    "inherit preference must have a null templateId",
                ));
            }
            Ok(PersistedMeetingTemplatePreference {
                schema_version: 1,
                mode: MeetingTemplateMode::Inherit,
                template_id: None,
                template_version: None,
                template_file_sha256: None,
                selected_at,
            })
        }
        MeetingTemplateMode::MeetingOverride => {
            let template_id = requested
                .template_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    request_deserialization_error(
                        "meeting_override preference requires a templateId",
                    )
                })?;
            let details = service.get(
                GetTemplateRequest {
                    template_id: template_id.to_owned(),
                    origin: None,
                    content_locale: None,
                },
                default_template_id,
            )?;
            Ok(PersistedMeetingTemplatePreference {
                schema_version: 1,
                mode: MeetingTemplateMode::MeetingOverride,
                template_id: Some(details.template.id.clone()),
                template_version: Some(details.template.version),
                template_file_sha256: Some(details.file_sha256),
                selected_at,
            })
        }
    }
}

fn resolve_meeting_template_preference(
    service: &TemplateService,
    stored_default: Option<String>,
    preference: PersistedMeetingTemplatePreference,
    storage: MeetingTemplateStorage,
) -> Result<MeetingTemplatePreferenceResponse, TemplateApiError> {
    let (resolved, issue) = match preference.mode {
        MeetingTemplateMode::MeetingOverride => {
            let selected_id = preference
                .template_id
                .clone()
                .ok_or_else(|| preference_data_error("meeting_override is missing template_id"))?;
            match service.get(
                GetTemplateRequest {
                    template_id: selected_id.clone(),
                    origin: None,
                    content_locale: None,
                },
                stored_default.as_deref(),
            ) {
                Ok(details) => (
                    resolved_from_details(
                        details,
                        MeetingTemplateResolutionSource::MeetingOverride,
                    ),
                    None,
                ),
                Err(error) => {
                    let issue_code = if error.code == "TEMPLATE_NOT_FOUND" {
                        "SELECTED_TEMPLATE_MISSING"
                    } else {
                        "SELECTED_TEMPLATE_INVALID"
                    };
                    let fallback = resolve_default_template(service, stored_default)?;
                    (
                        fallback,
                        Some(MeetingTemplateIssueDto {
                            code: issue_code.to_owned(),
                            selected_template_id: selected_id,
                        }),
                    )
                }
            }
        }
        MeetingTemplateMode::Inherit => (resolve_default_template(service, stored_default)?, None),
    };

    Ok(MeetingTemplatePreferenceResponse {
        preference: MeetingTemplatePreferenceDto::from(&preference),
        storage,
        resolved,
        issue,
    })
}

fn resolve_default_template(
    service: &TemplateService,
    stored_default: Option<String>,
) -> Result<ResolvedMeetingTemplateDto, TemplateApiError> {
    let default = service.get_default(stored_default);
    let source = match default.resolution_source {
        DefaultTemplateResolutionSource::UserDefault => {
            MeetingTemplateResolutionSource::GlobalDefault
        }
        DefaultTemplateResolutionSource::BuiltinFallback => {
            MeetingTemplateResolutionSource::BuiltinFallback
        }
    };
    let details = service.get(
        GetTemplateRequest {
            template_id: default.resolved_template_id,
            origin: None,
            content_locale: None,
        },
        default.template_id.as_deref(),
    )?;
    Ok(resolved_from_details(details, source))
}

fn resolved_from_details(
    details: TemplateDetailsDto,
    source: MeetingTemplateResolutionSource,
) -> ResolvedMeetingTemplateDto {
    ResolvedMeetingTemplateDto {
        template_id: details.template.id,
        name: details.template.name,
        version: details.template.version,
        file_sha256: details.file_sha256,
        origin: details.origin,
        source,
    }
}

fn load_preference_with_migration<R: Runtime>(
    app: &AppHandle<R>,
    folder: Option<&Path>,
    meeting_id: &str,
) -> Result<(PersistedMeetingTemplatePreference, MeetingTemplateStorage), TemplateApiError> {
    let fallback = load_fallback_preference(app, meeting_id)?;
    if let Some(folder) = folder {
        if let Some(metadata_preference) = read_preference_from_metadata(folder)? {
            if fallback.is_some() {
                delete_fallback_preference(app, meeting_id)?;
            }
            return Ok((metadata_preference, MeetingTemplateStorage::Metadata));
        }

        if let Some(fallback_preference) = fallback {
            match write_preference_to_metadata(folder, &fallback_preference) {
                Ok(()) => {
                    delete_fallback_preference(app, meeting_id)?;
                    return Ok((fallback_preference, MeetingTemplateStorage::Metadata));
                }
                Err(error) => {
                    tracing::warn!(
                        debug_id = %error.debug_id,
                        code = %error.code,
                        meeting_id,
                        "meeting template fallback migration was retained after metadata write failed"
                    );
                    return Ok((fallback_preference, MeetingTemplateStorage::LocalFallback));
                }
            }
        }

        return Ok((inherit_preference(), MeetingTemplateStorage::Metadata));
    }

    Ok((
        fallback.unwrap_or_else(inherit_preference),
        MeetingTemplateStorage::LocalFallback,
    ))
}

fn inherit_preference() -> PersistedMeetingTemplatePreference {
    PersistedMeetingTemplatePreference {
        schema_version: 1,
        mode: MeetingTemplateMode::Inherit,
        template_id: None,
        template_version: None,
        template_file_sha256: None,
        selected_at: Utc::now().to_rfc3339(),
    }
}

fn read_preference_from_metadata(
    folder: &Path,
) -> Result<Option<PersistedMeetingTemplatePreference>, TemplateApiError> {
    read_metadata_field(folder, MEETING_TEMPLATE_FIELD)
        .map_err(|error| internal_error("meeting template metadata could not be read", error))?
        .map(parse_persisted_preference)
        .transpose()
}

fn write_preference_to_metadata(
    folder: &Path,
    preference: &PersistedMeetingTemplatePreference,
) -> Result<(), TemplateApiError> {
    let value = serde_json::to_value(preference).map_err(|error| {
        internal_error("meeting template preference could not be serialized", error)
    })?;
    write_metadata_field(folder, MEETING_TEMPLATE_FIELD, Some(value))
        .map_err(|error| internal_error("meeting template metadata could not be saved", error))
}

fn fallback_key(meeting_id: &str) -> String {
    format!("{MEETING_TEMPLATE_KEY_PREFIX}{meeting_id}")
}

fn load_fallback_preference<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: &str,
) -> Result<Option<PersistedMeetingTemplatePreference>, TemplateApiError> {
    load_fallback_preference_at(app, TEMPLATE_PREFERENCE_STORE, meeting_id)
}

fn load_fallback_preference_at<R: Runtime>(
    app: &AppHandle<R>,
    path: impl AsRef<Path>,
    meeting_id: &str,
) -> Result<Option<PersistedMeetingTemplatePreference>, TemplateApiError> {
    let store = app.store(path).map_err(|error| {
        internal_error(
            "meeting template preference store could not be opened",
            error,
        )
    })?;
    store
        .get(fallback_key(meeting_id))
        .map(parse_persisted_preference)
        .transpose()
}

fn save_fallback_preference<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: &str,
    preference: &PersistedMeetingTemplatePreference,
) -> Result<(), TemplateApiError> {
    save_fallback_preference_at(app, TEMPLATE_PREFERENCE_STORE, meeting_id, preference)
}

fn save_fallback_preference_at<R: Runtime>(
    app: &AppHandle<R>,
    path: impl AsRef<Path>,
    meeting_id: &str,
    preference: &PersistedMeetingTemplatePreference,
) -> Result<(), TemplateApiError> {
    let store = app.store(path).map_err(|error| {
        internal_error(
            "meeting template preference store could not be opened",
            error,
        )
    })?;
    let value = serde_json::to_value(preference).map_err(|error| {
        internal_error("meeting template preference could not be serialized", error)
    })?;
    store.set(fallback_key(meeting_id), value);
    store.save().map_err(|error| {
        internal_error(
            "meeting template preference fallback could not be saved",
            error,
        )
    })
}

fn delete_fallback_preference<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: &str,
) -> Result<(), TemplateApiError> {
    delete_fallback_preference_at(app, TEMPLATE_PREFERENCE_STORE, meeting_id)
}

fn delete_fallback_preference_at<R: Runtime>(
    app: &AppHandle<R>,
    path: impl AsRef<Path>,
    meeting_id: &str,
) -> Result<(), TemplateApiError> {
    let store = app.store(path).map_err(|error| {
        internal_error(
            "meeting template preference store could not be opened",
            error,
        )
    })?;
    if store.delete(fallback_key(meeting_id)) {
        store.save().map_err(|error| {
            internal_error(
                "meeting template preference fallback could not be removed",
                error,
            )
        })?;
    }
    Ok(())
}

fn parse_persisted_preference(
    value: Value,
) -> Result<PersistedMeetingTemplatePreference, TemplateApiError> {
    let preference: PersistedMeetingTemplatePreference =
        serde_json::from_value(value).map_err(|error| preference_data_error(error))?;
    validate_persisted_preference(&preference)?;
    Ok(preference)
}

fn validate_persisted_preference(
    preference: &PersistedMeetingTemplatePreference,
) -> Result<(), TemplateApiError> {
    let valid_override = preference.mode == MeetingTemplateMode::MeetingOverride
        && preference
            .template_id
            .as_deref()
            .is_some_and(|id| !id.is_empty())
        && preference
            .template_version
            .is_some_and(|version| version >= 1)
        && preference
            .template_file_sha256
            .as_deref()
            .is_some_and(is_sha256);
    let valid_inherit = preference.mode == MeetingTemplateMode::Inherit
        && preference.template_id.is_none()
        && preference.template_version.is_none()
        && preference.template_file_sha256.is_none();
    if preference.schema_version != 1
        || (!valid_override && !valid_inherit)
        || chrono::DateTime::parse_from_rfc3339(&preference.selected_at).is_err()
    {
        return Err(preference_data_error(
            "stored meeting template preference failed schema validation",
        ));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_meeting_id(meeting_id: &str) -> Result<(), TemplateApiError> {
    if meeting_id.is_empty() || meeting_id.len() > 200 || meeting_id.chars().any(char::is_control) {
        return Err(request_deserialization_error("meetingId is invalid"));
    }
    Ok(())
}

fn preference_data_error(detail: impl std::fmt::Display) -> TemplateApiError {
    let error = TemplateApiError::from_code("TEMPLATE_INVALID");
    tracing::warn!(
        debug_id = %error.debug_id,
        code = %error.code,
        detail = %detail,
        "stored meeting template preference is invalid"
    );
    error
}

fn load_default_template_id<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<Option<String>, TemplateApiError> {
    load_default_template_id_at(app, TEMPLATE_PREFERENCE_STORE)
}

fn load_default_template_id_at<R: Runtime>(
    app: &AppHandle<R>,
    path: impl AsRef<std::path::Path>,
) -> Result<Option<String>, TemplateApiError> {
    let store = app
        .store(path)
        .map_err(|error| internal_error("default template store could not be opened", error))?;
    let Some(value) = store.get(DEFAULT_TEMPLATE_KEY) else {
        return Ok(None);
    };
    value
        .as_str()
        .map(|value| Some(value.to_owned()))
        .ok_or_else(|| {
            let error = TemplateApiError::from_code("TEMPLATE_INVALID");
            tracing::warn!(
                debug_id = %error.debug_id,
                code = %error.code,
                "default template preference has an invalid stored type"
            );
            error
        })
}

fn save_default_template_id<R: Runtime>(
    app: &AppHandle<R>,
    template_id: Option<&str>,
) -> Result<(), TemplateApiError> {
    save_default_template_id_at(app, TEMPLATE_PREFERENCE_STORE, template_id)
}

fn save_default_template_id_at<R: Runtime>(
    app: &AppHandle<R>,
    path: impl AsRef<std::path::Path>,
    template_id: Option<&str>,
) -> Result<(), TemplateApiError> {
    let store = app
        .store(path)
        .map_err(|error| internal_error("default template store could not be opened", error))?;
    match template_id {
        Some(template_id) => store.set(DEFAULT_TEMPLATE_KEY, Value::String(template_id.to_owned())),
        None => {
            store.delete(DEFAULT_TEMPLATE_KEY);
        }
    }
    store
        .save()
        .map_err(|error| internal_error("default template preference could not be saved", error))
}

fn open_directory(path: PathBuf) -> Result<(), TemplateApiError> {
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer").arg(&path).spawn();

    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(&path).spawn();

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(&path).spawn();

    result
        .map(|_| ())
        .map_err(|error| internal_error("templates directory could not be opened", error))
}

fn parse_request<T: DeserializeOwned>(value: Value) -> Result<T, TemplateApiError> {
    serde_json::from_value(value).map_err(request_deserialization_error)
}

fn request_deserialization_error(error: impl std::fmt::Display) -> TemplateApiError {
    let mut api_error = TemplateApiError::from_code("TEMPLATE_INVALID");
    api_error
        .field_errors
        .push(crate::summary::templates::TemplateFieldIssueDto {
            code: "REQUEST_DESERIALIZATION_FAILED".to_owned(),
            path: String::new(),
            message_key: "templates.validation.requestInvalid".to_owned(),
            params: Default::default(),
        });
    tracing::warn!(
        debug_id = %api_error.debug_id,
        code = %api_error.code,
        detail = %error,
        "template command request could not be deserialized"
    );
    api_error
}

fn internal_error(context: &str, error: impl std::fmt::Display) -> TemplateApiError {
    let api_error = TemplateApiError::from_code("TEMPLATE_IO_ERROR");
    tracing::warn!(
        debug_id = %api_error.debug_id,
        code = %api_error.code,
        detail = %error,
        context,
        "template API operation failed"
    );
    api_error
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_service() -> (TempDir, TemplateService) {
        let temporary = TempDir::new().unwrap();
        let repository = TemplateRepository::new(temporary.path().join("templates"), None).unwrap();
        (temporary, TemplateService::new(repository))
    }

    fn valid_override(template_id: &str) -> PersistedMeetingTemplatePreference {
        PersistedMeetingTemplatePreference {
            schema_version: 1,
            mode: MeetingTemplateMode::MeetingOverride,
            template_id: Some(template_id.to_owned()),
            template_version: Some(1),
            template_file_sha256: Some("a".repeat(64)),
            selected_at: "2026-08-23T00:00:00Z".to_owned(),
        }
    }

    fn mock_app() -> tauri::App<tauri::test::MockRuntime> {
        tauri::test::mock_builder()
            .plugin(tauri_plugin_store::Builder::default().build())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap()
    }

    #[test]
    fn default_template_store_survives_reload_and_clear() {
        let temporary = TempDir::new().unwrap();
        let path = temporary
            .path()
            .join("summary-template-preferences.v1.json");
        {
            let app = mock_app();
            save_default_template_id_at(app.handle(), &path, Some("daily_standup")).unwrap();
            assert!(path.is_file());
            assert_eq!(
                load_default_template_id_at(app.handle(), &path).unwrap(),
                Some("daily_standup".to_owned())
            );
        }
        {
            let app = mock_app();
            assert_eq!(
                load_default_template_id_at(app.handle(), &path).unwrap(),
                Some("daily_standup".to_owned())
            );
            save_default_template_id_at(app.handle(), &path, None).unwrap();
        }
        let app = mock_app();
        assert_eq!(
            load_default_template_id_at(app.handle(), &path).unwrap(),
            None
        );
    }

    #[test]
    fn meeting_fallback_store_survives_reload_and_deletes_only_its_key() {
        let temporary = TempDir::new().unwrap();
        let path = temporary.path().join("meeting-preferences.json");
        let preference = valid_override("daily_standup");
        {
            let app = mock_app();
            save_fallback_preference_at(app.handle(), &path, "meeting-one", &preference).unwrap();
            save_fallback_preference_at(app.handle(), &path, "meeting-two", &inherit_preference())
                .unwrap();
        }
        {
            let app = mock_app();
            assert_eq!(
                load_fallback_preference_at(app.handle(), &path, "meeting-one").unwrap(),
                Some(preference)
            );
            delete_fallback_preference_at(app.handle(), &path, "meeting-one").unwrap();
        }
        let app = mock_app();
        assert_eq!(
            load_fallback_preference_at(app.handle(), &path, "meeting-one").unwrap(),
            None
        );
        assert_eq!(
            load_fallback_preference_at(app.handle(), &path, "meeting-two")
                .unwrap()
                .map(|value| value.mode),
            Some(MeetingTemplateMode::Inherit)
        );
    }

    #[test]
    fn malformed_request_becomes_structured_template_error() {
        let error = parse_request::<GetTemplateRequest>(serde_json::json!({
            "templateId": 42
        }))
        .unwrap_err();
        assert_eq!(error.code, "TEMPLATE_INVALID");
        assert_eq!(error.field_errors[0].code, "REQUEST_DESERIALIZATION_FAILED");
        assert!(!serde_json::to_string(&error).unwrap().contains("integer"));
    }

    #[test]
    fn persisted_preference_schema_rejects_partial_or_unsafe_shapes() {
        assert!(validate_persisted_preference(&valid_override("daily_standup")).is_ok());
        assert!(
            validate_persisted_preference(&PersistedMeetingTemplatePreference {
                schema_version: 1,
                mode: MeetingTemplateMode::Inherit,
                template_id: None,
                template_version: None,
                template_file_sha256: None,
                selected_at: "2026-08-23T00:00:00Z".to_owned(),
            })
            .is_ok()
        );

        let mut partial = valid_override("daily_standup");
        partial.template_file_sha256 = None;
        assert_eq!(
            validate_persisted_preference(&partial).unwrap_err().code,
            "TEMPLATE_INVALID"
        );

        let mut uppercase_hash = valid_override("daily_standup");
        uppercase_hash.template_file_sha256 = Some("A".repeat(64));
        assert_eq!(
            validate_persisted_preference(&uppercase_hash)
                .unwrap_err()
                .code,
            "TEMPLATE_INVALID"
        );

        let mut bad_timestamp = valid_override("daily_standup");
        bad_timestamp.selected_at = "yesterday".to_owned();
        assert_eq!(
            validate_persisted_preference(&bad_timestamp)
                .unwrap_err()
                .code,
            "TEMPLATE_INVALID"
        );
    }

    #[test]
    fn meeting_override_captures_the_exact_resolved_version_and_hash() {
        let (_temporary, service) = test_service();
        let expected = service
            .get(
                GetTemplateRequest {
                    template_id: "daily_standup".to_owned(),
                    origin: None,
                    content_locale: None,
                },
                None,
            )
            .unwrap();
        let persisted = build_meeting_preference(
            &service,
            None,
            SaveMeetingTemplatePreferenceValue {
                mode: MeetingTemplateMode::MeetingOverride,
                template_id: Some("daily_standup".to_owned()),
            },
        )
        .unwrap();

        assert_eq!(persisted.template_id.as_deref(), Some("daily_standup"));
        assert_eq!(persisted.template_version, Some(expected.template.version));
        assert_eq!(
            persisted.template_file_sha256.as_deref(),
            Some(expected.file_sha256.as_str())
        );
        assert!(validate_persisted_preference(&persisted).is_ok());
    }

    #[test]
    fn inherit_requires_null_template_id_and_resolves_global_default() {
        let (_temporary, service) = test_service();
        let error = build_meeting_preference(
            &service,
            None,
            SaveMeetingTemplatePreferenceValue {
                mode: MeetingTemplateMode::Inherit,
                template_id: Some("daily_standup".to_owned()),
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "TEMPLATE_INVALID");

        let response = resolve_meeting_template_preference(
            &service,
            Some("daily_standup".to_owned()),
            inherit_preference(),
            MeetingTemplateStorage::Metadata,
        )
        .unwrap();
        assert_eq!(response.resolved.template_id, "daily_standup");
        assert_eq!(
            response.resolved.source,
            MeetingTemplateResolutionSource::GlobalDefault
        );
        assert!(response.issue.is_none());
    }

    #[test]
    fn unavailable_override_is_not_silently_rewritten() {
        let (_temporary, service) = test_service();
        let preference = valid_override("deleted_custom_template");
        let response = resolve_meeting_template_preference(
            &service,
            None,
            preference.clone(),
            MeetingTemplateStorage::LocalFallback,
        )
        .unwrap();

        assert_eq!(response.preference.template_id, preference.template_id);
        assert_eq!(response.resolved.template_id, "standard_meeting");
        assert_eq!(
            response.issue.as_ref().map(|issue| issue.code.as_str()),
            Some("SELECTED_TEMPLATE_MISSING")
        );
    }

    #[test]
    fn metadata_round_trip_preserves_unrelated_fields_and_uses_snake_case_on_disk() {
        let temporary = TempDir::new().unwrap();
        std::fs::write(
            temporary.path().join("metadata.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "summary_language": "zh-CN",
                "future_field": {"keep": true}
            }))
            .unwrap(),
        )
        .unwrap();
        let preference = valid_override("daily_standup");
        write_preference_to_metadata(temporary.path(), &preference).unwrap();
        assert_eq!(
            read_preference_from_metadata(temporary.path()).unwrap(),
            Some(preference)
        );

        let value: Value =
            serde_json::from_slice(&std::fs::read(temporary.path().join("metadata.json")).unwrap())
                .unwrap();
        assert_eq!(value["summary_language"], "zh-CN");
        assert_eq!(value["future_field"]["keep"], true);
        assert!(value["summary_template"].get("template_id").is_some());
        assert!(value["summary_template"].get("templateId").is_none());
    }

    #[test]
    fn current_usage_counts_only_matching_meeting_overrides() {
        let temporary = TempDir::new().unwrap();
        let first = temporary.path().join("first");
        let second = temporary.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        write_preference_to_metadata(&first, &valid_override("daily_standup")).unwrap();
        write_preference_to_metadata(&second, &inherit_preference()).unwrap();
        let now = crate::database::models::DateTimeUtc(Utc::now());
        let meetings = vec![
            crate::database::models::MeetingModel {
                id: "first-meeting".to_owned(),
                title: "First".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
                folder_path: Some(first.to_string_lossy().into_owned()),
            },
            crate::database::models::MeetingModel {
                id: "second-meeting".to_owned(),
                title: "Second".to_owned(),
                created_at: now.clone(),
                updated_at: now,
                folder_path: Some(second.to_string_lossy().into_owned()),
            },
        ];
        let app = mock_app();

        assert_eq!(
            count_current_meeting_preferences(app.handle(), &meetings, "daily_standup").unwrap(),
            1
        );
        assert_eq!(
            count_current_meeting_preferences(app.handle(), &meetings, "standard_meeting").unwrap(),
            0
        );
    }

    #[test]
    fn response_transport_is_camel_case_and_does_not_expose_paths() {
        let (_temporary, service) = test_service();
        let response = resolve_meeting_template_preference(
            &service,
            None,
            inherit_preference(),
            MeetingTemplateStorage::Metadata,
        )
        .unwrap();
        let serialized = serde_json::to_value(response).unwrap();
        assert!(serialized["preference"].get("schemaVersion").is_some());
        assert!(serialized["preference"].get("schema_version").is_none());
        assert!(serialized["resolved"].get("fileSha256").is_some());
        assert!(!serialized.to_string().contains("templates\\"));
        assert!(!serialized.to_string().contains("templates/"));
    }

    #[test]
    fn json_import_accepts_utf8_bom_and_normalizes_source_metadata() {
        let (temporary, service) = test_service();
        let record = service
            .repository()
            .get_for_content_locale("daily_standup", None, Some("en"))
            .unwrap();
        let path = temporary.path().join("standup.json");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend(serde_json::to_vec_pretty(&record.template).unwrap());
        std::fs::write(&path, bytes).unwrap();

        let preview = preview_template_json(&path).unwrap();
        assert_eq!(preview.source_type, TemplateSourceType::JsonImport);
        assert_eq!(
            preview.draft.source.source_type,
            TemplateSourceType::JsonImport
        );
        assert_eq!(
            preview.draft.source.original_file_name.as_deref(),
            Some("standup.json")
        );
        assert_eq!(
            preview.draft.source.original_file_sha256.as_deref(),
            Some(preview.file_sha256.as_str())
        );
        assert_eq!(preview.draft.version, 1);
        assert!(preview.outline.is_empty());
    }

    #[test]
    fn json_import_reports_syntax_location_without_exposing_file_contents() {
        let temporary = TempDir::new().unwrap();
        let path = temporary.path().join("broken.json");
        std::fs::write(&path, b"{\n  \"schema_version\": 2,\n  broken\n}").unwrap();

        let error = preview_template_json(&path).unwrap_err();
        assert_eq!(error.code, "TEMPLATE_JSON_INVALID");
        assert_eq!(error.params.get("line"), Some(&serde_json::json!(3)));
        assert!(error.params.get("column").is_some());
        assert!(!serde_json::to_string(&error).unwrap().contains("broken"));
    }

    #[test]
    fn json_import_rejects_unsupported_schema_encoding_and_oversized_files() {
        let temporary = TempDir::new().unwrap();
        let unsupported = temporary.path().join("unsupported.json");
        std::fs::write(&unsupported, br#"{"schema_version":3}"#).unwrap();
        let error = preview_template_json(&unsupported).unwrap_err();
        assert_eq!(error.code, "TEMPLATE_INVALID");
        assert!(error
            .field_errors
            .iter()
            .any(|issue| issue.path == "/schemaVersion"));

        let invalid_encoding = temporary.path().join("encoding.json");
        std::fs::write(&invalid_encoding, [0xFF, 0xFE, 0x00]).unwrap();
        assert_eq!(
            preview_template_json(&invalid_encoding).unwrap_err().code,
            "TEMPLATE_JSON_ENCODING_INVALID"
        );

        let oversized = temporary.path().join("oversized.json");
        std::fs::write(&oversized, vec![b' '; MAX_JSON_IMPORT_BYTES as usize + 1]).unwrap();
        assert_eq!(
            preview_template_json(&oversized).unwrap_err().code,
            "TEMPLATE_ARCHIVE_TOO_LARGE"
        );
    }

    #[test]
    fn json_import_migrates_legacy_v1_before_review() {
        let temporary = TempDir::new().unwrap();
        let path = temporary.path().join("legacy-review.json");
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "name": "Legacy review",
                "description": "Migrated locally",
                "sections": [{
                    "title": "Decisions",
                    "instruction": "Extract decisions",
                    "format": "list",
                    "item_format": "- {{decision}}",
                    "example_item_format": "- Ship it"
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let preview = preview_template_json(&path).unwrap();
        assert_eq!(preview.draft.schema_version, 2);
        assert_eq!(preview.draft.id, "legacy_review");
        assert!(preview
            .warnings
            .iter()
            .any(|warning| warning.code == "JSON_V1_MIGRATED"));
    }

    #[test]
    fn import_job_keeps_client_ids_and_validates_parallel_item_ids() {
        let request = PreviewTemplateDocumentsRequest {
            paths: vec!["first.json".to_owned(), "second.docx".to_owned()],
            job_id: Some("stable-job-id".to_owned()),
            item_ids: Some(vec!["stable-item-a".to_owned(), "stable-item-b".to_owned()]),
        };
        let (job_id, work_items) = prepare_template_import_job(request).unwrap();
        assert_eq!(job_id, "stable-job-id");
        assert_eq!(work_items[0].0, "stable-item-a");
        assert_eq!(work_items[1].0, "stable-item-b");

        let mismatched = prepare_template_import_job(PreviewTemplateDocumentsRequest {
            paths: vec!["only.json".to_owned()],
            job_id: Some("mismatched-job".to_owned()),
            item_ids: Some(vec![]),
        })
        .unwrap_err();
        assert_eq!(mismatched.code, "TEMPLATE_INVALID");
    }

    #[test]
    fn import_job_cancellation_is_item_scoped_batch_wide_and_idempotent() {
        let job_id = format!("cancel-test-{}", uuid::Uuid::new_v4());
        let work_items = vec![
            ("item-a".to_owned(), "a.docx".to_owned()),
            ("item-b".to_owned(), "b.docx".to_owned()),
        ];
        let job = register_template_import_job(&job_id, &work_items).unwrap();
        set_template_import_job_status(&job, TemplateImportJobStatus::Running);

        cancel_template_import_item(&job, "item-a").unwrap();
        cancel_template_import_item(&job, "item-a").unwrap();
        assert!(template_import_item_cancellation(&job, "item-a").is_cancelled());
        assert!(!template_import_item_cancellation(&job, "item-b").is_cancelled());
        let item_snapshot = template_import_job_snapshot(&job);
        assert_eq!(item_snapshot.status, TemplateImportJobStatus::Running);
        assert_eq!(
            item_snapshot
                .items
                .iter()
                .find(|item| item.item_id == "item-a")
                .map(|item| item.status),
            Some(TemplateImportItemStatus::CancelRequested)
        );

        cancel_template_import_job(&job);
        cancel_template_import_job(&job);
        assert!(job.batch_cancelled.load(Ordering::Acquire));
        assert!(template_import_item_cancellation(&job, "item-b").is_cancelled());
        assert_eq!(
            template_import_job_snapshot(&job).status,
            TemplateImportJobStatus::CancelRequested
        );
        set_template_import_job_status(&job, TemplateImportJobStatus::Cancelled);
    }

    #[test]
    fn json_export_is_pretty_snake_case_utf8_and_replace_safe() {
        let (temporary, service) = test_service();
        let destination = temporary.path().join("daily_standup.json");
        std::fs::write(&destination, b"old data").unwrap();
        let request = ExportTemplateJsonRequest {
            template_id: "daily_standup".to_owned(),
            origin: None,
            content_locale: Some("en".to_owned()),
            destination_path: destination.to_string_lossy().into_owned(),
        };

        let response = export_template_json_file(&service, request, destination.clone()).unwrap();
        let bytes = std::fs::read(&destination).unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(response.bytes, bytes.len());
        assert_eq!(
            response.file_sha256,
            format!("{:x}", Sha256::digest(&bytes))
        );
        assert!(bytes.ends_with(b"\n"));
        assert!(value.get("schema_version").is_some());
        assert!(value.get("schemaVersion").is_none());
        assert_eq!(value["id"], "daily_standup");
    }
}
