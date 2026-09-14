use super::repository::{
    CreateConflictPolicy, DeleteTemplateResult, DeletedTemplateListItem, RestoreConflictPolicy,
    TemplateListItem, TemplateOrigin, TemplateRecord, TemplateRepository,
    TemplateRepositoryDiagnostic, TemplateRepositoryError, TemplateRepositoryErrorKind,
};
use super::v2::{
    validate_template_v2_value, EmptyBehavior, TemplateFieldIssue, TemplateFormat,
    TemplateSectionV2, TemplateSource, TemplateSourceType, TemplateV2,
};
use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use uuid::Uuid;

pub const BUILTIN_FALLBACK_TEMPLATE_ID: &str = "standard_meeting";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateApiError {
    pub code: String,
    pub message_key: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub field_errors: Vec<TemplateFieldIssueDto>,
    pub retryable: bool,
    pub debug_id: String,
}

impl TemplateApiError {
    pub fn from_code(code: &str) -> Self {
        Self {
            code: code.to_owned(),
            message_key: message_key_for_code(code).to_owned(),
            params: BTreeMap::new(),
            field_errors: Vec::new(),
            retryable: retryable_for_code(code),
            debug_id: Uuid::new_v4().to_string(),
        }
    }

    pub fn with_param(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.params.insert(key.to_owned(), value.into());
        self
    }

    fn from_validation(errors: Vec<TemplateFieldIssueDto>) -> Self {
        let mut error = Self::from_code("TEMPLATE_INVALID");
        error.field_errors = errors;
        error
    }

    fn from_repository(error: TemplateRepositoryError) -> Self {
        let api_error = Self::from_code(error.code());
        tracing::warn!(
            debug_id = %api_error.debug_id,
            code = %api_error.code,
            detail = %error.detail,
            "template repository operation failed"
        );
        api_error
    }
}

impl From<TemplateRepositoryError> for TemplateApiError {
    fn from(error: TemplateRepositoryError) -> Self {
        Self::from_repository(error)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateFieldIssueDto {
    pub code: String,
    pub path: String,
    pub message_key: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, Value>,
}

impl From<TemplateFieldIssue> for TemplateFieldIssueDto {
    fn from(issue: TemplateFieldIssue) -> Self {
        Self {
            code: issue.code,
            path: file_pointer_to_api_pointer(&issue.path),
            message_key: issue.message_key,
            params: issue.params,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateSourceDto {
    #[serde(rename = "type")]
    pub source_type: TemplateSourceType,
    pub original_file_name: Option<String>,
    pub original_file_sha256: Option<String>,
    pub imported_at: Option<DateTime<FixedOffset>>,
    pub copied_from_template_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateSectionV2Dto {
    pub id: String,
    pub title: String,
    pub instruction: String,
    pub format: TemplateFormat,
    pub item_format: Option<String>,
    pub example_item_format: Option<String>,
    pub required: bool,
    pub empty_behavior: EmptyBehavior,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateV2Dto {
    pub schema_version: u8,
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: u64,
    pub locale: Option<String>,
    pub tags: Vec<String>,
    pub source: TemplateSourceDto,
    pub created_at: DateTime<FixedOffset>,
    pub updated_at: DateTime<FixedOffset>,
    pub sections: Vec<TemplateSectionV2Dto>,
    pub extensions: Map<String, Value>,
}

impl From<&TemplateV2> for TemplateV2Dto {
    fn from(template: &TemplateV2) -> Self {
        Self {
            schema_version: template.schema_version,
            id: template.id.clone(),
            name: template.name.clone(),
            description: template.description.clone(),
            version: template.version,
            locale: template.locale.clone(),
            tags: template.tags.clone(),
            source: TemplateSourceDto {
                source_type: template.source.source_type,
                original_file_name: template.source.original_file_name.clone(),
                original_file_sha256: template.source.original_file_sha256.clone(),
                imported_at: template.source.imported_at,
                copied_from_template_id: template.source.copied_from_template_id.clone(),
            },
            created_at: template.created_at,
            updated_at: template.updated_at,
            sections: template
                .sections
                .iter()
                .map(|section| TemplateSectionV2Dto {
                    id: section.id.clone(),
                    title: section.title.clone(),
                    instruction: section.instruction.clone(),
                    format: section.format,
                    item_format: section.item_format.clone(),
                    example_item_format: section.example_item_format.clone(),
                    required: section.required,
                    empty_behavior: section.empty_behavior,
                })
                .collect(),
            extensions: template.extensions.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationSummaryDto {
    pub error_count: usize,
    pub warning_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateListItemDto {
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
    pub is_default: bool,
    pub read_only: bool,
    pub overrides_builtin: bool,
    pub valid: bool,
    pub validation_summary: ValidationSummaryDto,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateDetailsDto {
    pub template: TemplateV2Dto,
    pub origin: TemplateOrigin,
    pub schema_version_on_disk: u8,
    pub file_sha256: String,
    pub semantic_sha256: String,
    pub is_default: bool,
    pub overrides_builtin: bool,
    pub read_only: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct ListTemplatesRequest {
    pub origin: Option<ListOrigin>,
    pub include_invalid: Option<bool>,
    pub include_trash: Option<bool>,
    pub query: Option<String>,
    pub content_locale: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListOrigin {
    All,
    Builtin,
    Bundled,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListTemplatesResponse {
    pub templates: Vec<TemplateListItemDto>,
    pub diagnostics: Vec<TemplateRepositoryDiagnostic>,
    pub deleted_templates: Vec<DeletedTemplateListItem>,
    pub default_template_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct GetTemplateRequest {
    pub template_id: String,
    pub origin: Option<TemplateOrigin>,
    pub content_locale: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationMode {
    Create,
    Update,
    Import,
    Preview,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct ValidateTemplateRequest {
    pub template: Value,
    pub mode: ValidationMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationResultDto {
    pub valid: bool,
    pub errors: Vec<TemplateFieldIssueDto>,
    pub warnings: Vec<TemplateFieldIssueDto>,
    pub normalized: Option<TemplateV2Dto>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiCreateConflictPolicy {
    Error,
    KeepBoth,
    OverrideBuiltin,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct CreateTemplateRequest {
    pub template: Value,
    pub conflict_policy: ApiCreateConflictPolicy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct UpdateTemplateRequest {
    pub template_id: String,
    pub expected_version: u64,
    pub expected_file_sha256: String,
    pub template: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct DuplicateTemplateRequest {
    pub template_id: String,
    pub origin: Option<TemplateOrigin>,
    pub new_name: String,
    pub requested_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct DeleteTemplateRequest {
    pub template_id: String,
    pub expected_file_sha256: String,
    #[serde(default)]
    pub replacement_default_template_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct GetTemplateUsageRequest {
    pub template_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateUsageDto {
    pub current_meeting_preference_count: usize,
    pub historical_snapshot_count: usize,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteTemplateResponse {
    pub trash_id: String,
    pub deleted_at: DateTime<FixedOffset>,
    pub file_sha256: String,
    pub usage: TemplateUsageDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct RestoreTemplateRequest {
    pub trash_id: String,
    pub conflict_policy: ApiRestoreConflictPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiRestoreConflictPolicy {
    Error,
    KeepBoth,
    ReplaceCustom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct PurgeTemplateRequest {
    pub trash_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplatesDirectoryInfo {
    pub path: String,
    pub exists: bool,
    pub writable: bool,
    pub custom_template_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultTemplateResolutionSource {
    UserDefault,
    BuiltinFallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultTemplatePreferenceDto {
    pub template_id: Option<String>,
    pub resolved_template_id: String,
    pub resolution_source: DefaultTemplateResolutionSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct SetDefaultTemplateRequest {
    pub template_id: Value,
}

#[derive(Clone)]
pub struct TemplateService {
    repository: TemplateRepository,
}

impl TemplateService {
    pub fn new(repository: TemplateRepository) -> Self {
        Self { repository }
    }

    pub fn repository(&self) -> &TemplateRepository {
        &self.repository
    }

    pub fn directory_info(&self) -> Result<TemplatesDirectoryInfo, TemplateApiError> {
        verify_directory_is_still_writable(self.repository.root())?;
        let listed = self.repository.list().map_err(TemplateApiError::from)?;
        let custom_template_count = listed
            .templates
            .iter()
            .filter(|item| item.origin == TemplateOrigin::Custom)
            .count();
        Ok(TemplatesDirectoryInfo {
            path: self.repository.root().to_string_lossy().into_owned(),
            exists: self.repository.root().is_dir(),
            writable: true,
            custom_template_count,
        })
    }

    pub fn list(
        &self,
        request: ListTemplatesRequest,
        default_template_id: Option<&str>,
    ) -> Result<ListTemplatesResponse, TemplateApiError> {
        let mut result = self
            .repository
            .list_for_content_locale(request.content_locale.as_deref())
            .map_err(TemplateApiError::from)?;
        let include_invalid = request.include_invalid.unwrap_or(true);
        let query = request
            .query
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_lowercase());
        result.templates.retain(|item| {
            origin_matches(request.origin, item.origin)
                && (include_invalid || item.valid)
                && query.as_ref().map_or(true, |query| {
                    item.name.to_lowercase().contains(query)
                        || item.description.to_lowercase().contains(query)
                        || item.id.to_lowercase().contains(query)
                })
        });
        let templates = result
            .templates
            .into_iter()
            .map(|item| list_item_to_dto(item, default_template_id))
            .collect();
        let deleted_templates = if request.include_trash.unwrap_or(false) {
            self.repository
                .list_deleted()
                .map_err(TemplateApiError::from)?
        } else {
            Vec::new()
        };
        Ok(ListTemplatesResponse {
            templates,
            diagnostics: result.diagnostics,
            deleted_templates,
            default_template_id: default_template_id.map(str::to_owned),
        })
    }

    pub fn get(
        &self,
        request: GetTemplateRequest,
        default_template_id: Option<&str>,
    ) -> Result<TemplateDetailsDto, TemplateApiError> {
        match self.repository.get_for_content_locale(
            &request.template_id,
            request.origin,
            request.content_locale.as_deref(),
        ) {
            Ok(record) => Ok(record_to_details(record, default_template_id)),
            Err(error) => {
                let is_invalid = error.kind == TemplateRepositoryErrorKind::InvalidTemplate;
                let mut api_error = TemplateApiError::from(error);
                if is_invalid {
                    api_error.field_errors = self.validation_issues_for_stored_template(
                        &request.template_id,
                        request.origin,
                    );
                }
                Err(api_error)
            }
        }
    }

    pub fn validate(&self, request: ValidateTemplateRequest) -> ValidationResultDto {
        validate_api_template_value(request.template, request.mode)
    }

    pub fn create(
        &self,
        request: CreateTemplateRequest,
        default_template_id: Option<&str>,
    ) -> Result<TemplateDetailsDto, TemplateApiError> {
        let validation = validate_api_template_value(request.template, ValidationMode::Create);
        let mut template = normalized_or_error(validation)?;
        let repository_policy = match request.conflict_policy {
            ApiCreateConflictPolicy::OverrideBuiltin => CreateConflictPolicy::OverrideReadOnly,
            ApiCreateConflictPolicy::Error | ApiCreateConflictPolicy::KeepBoth => {
                CreateConflictPolicy::Error
            }
        };
        let record = match self.repository.create(template.clone(), repository_policy) {
            Ok(record) => record,
            Err(error)
                if error.kind == TemplateRepositoryErrorKind::AlreadyExists
                    && request.conflict_policy == ApiCreateConflictPolicy::KeepBoth =>
            {
                template.id = self.next_available_id(&template.id)?;
                self.repository
                    .create(template, CreateConflictPolicy::Error)
                    .map_err(TemplateApiError::from)?
            }
            Err(error) => return Err(TemplateApiError::from(error)),
        };
        Ok(record_to_details(record, default_template_id))
    }

    pub fn update(
        &self,
        request: UpdateTemplateRequest,
        default_template_id: Option<&str>,
    ) -> Result<TemplateDetailsDto, TemplateApiError> {
        let validation = validate_api_template_value(request.template, ValidationMode::Update);
        let template = normalized_or_error(validation)?;
        let record = self
            .repository
            .update(
                &request.template_id,
                request.expected_version,
                &request.expected_file_sha256,
                template,
            )
            .map_err(TemplateApiError::from)?;
        Ok(record_to_details(record, default_template_id))
    }

    pub fn duplicate(
        &self,
        request: DuplicateTemplateRequest,
        default_template_id: Option<&str>,
    ) -> Result<TemplateDetailsDto, TemplateApiError> {
        if request.new_name.trim().is_empty() {
            return Err(TemplateApiError::from_validation(vec![
                TemplateFieldIssueDto {
                    code: "BLANK_VALUE".to_owned(),
                    path: "/newName".to_owned(),
                    message_key: "templates.validation.blankValue".to_owned(),
                    params: BTreeMap::new(),
                },
            ]));
        }
        let source = self
            .repository
            .get(&request.template_id, request.origin)
            .map_err(TemplateApiError::from)?;
        let mut duplicate = source.template.clone();
        duplicate.name = request.new_name;
        duplicate.id = match request.requested_id {
            Some(requested_id) => requested_id,
            None => self.next_available_id(&suggest_duplicate_id(&source.template.id))?,
        };
        duplicate.source = TemplateSource {
            source_type: if source.origin == TemplateOrigin::Custom {
                TemplateSourceType::Duplicate
            } else {
                TemplateSourceType::BuiltinCopy
            },
            original_file_name: None,
            original_file_sha256: None,
            imported_at: None,
            copied_from_template_id: Some(source.template.id),
        };
        let record = self
            .repository
            .create(duplicate, CreateConflictPolicy::Error)
            .map_err(TemplateApiError::from)?;
        Ok(record_to_details(record, default_template_id))
    }

    pub fn usage(
        &self,
        template_id: &str,
        default_template_id: Option<&str>,
    ) -> Result<TemplateUsageDto, TemplateApiError> {
        self.repository
            .get(template_id, None)
            .map_err(TemplateApiError::from)?;
        Ok(TemplateUsageDto {
            current_meeting_preference_count: 0,
            historical_snapshot_count: 0,
            is_default: default_template_id == Some(template_id),
        })
    }

    pub fn delete(
        &self,
        request: DeleteTemplateRequest,
        default_template_id: Option<&str>,
    ) -> Result<DeleteTemplateResponse, TemplateApiError> {
        let usage = self.usage(&request.template_id, default_template_id)?;
        if usage.is_default {
            return Err(TemplateApiError::from_code("TEMPLATE_IS_DEFAULT")
                .with_param("templateId", request.template_id));
        }
        if usage.current_meeting_preference_count > 0 {
            return Err(TemplateApiError::from_code("TEMPLATE_IN_USE")
                .with_param("templateId", request.template_id));
        }
        let deleted = self
            .repository
            .delete(&request.template_id, &request.expected_file_sha256)
            .map_err(TemplateApiError::from)?;
        Ok(delete_response(deleted, usage))
    }

    pub fn list_deleted(&self) -> Result<Vec<DeletedTemplateListItem>, TemplateApiError> {
        self.repository
            .list_deleted()
            .map_err(TemplateApiError::from)
    }

    pub fn restore(
        &self,
        request: RestoreTemplateRequest,
        default_template_id: Option<&str>,
    ) -> Result<TemplateDetailsDto, TemplateApiError> {
        let policy = match request.conflict_policy {
            ApiRestoreConflictPolicy::Error => RestoreConflictPolicy::Error,
            ApiRestoreConflictPolicy::KeepBoth => RestoreConflictPolicy::KeepBoth,
            ApiRestoreConflictPolicy::ReplaceCustom => RestoreConflictPolicy::ReplaceCustom,
        };
        let record = self
            .repository
            .restore(&request.trash_id, policy)
            .map_err(TemplateApiError::from)?;
        Ok(record_to_details(record, default_template_id))
    }

    pub fn purge(&self, request: PurgeTemplateRequest) -> Result<(), TemplateApiError> {
        self.repository
            .purge(&request.trash_id)
            .map_err(TemplateApiError::from)
    }

    pub fn get_default(&self, stored_template_id: Option<String>) -> DefaultTemplatePreferenceDto {
        if let Some(template_id) = stored_template_id.clone() {
            if self.repository.get(&template_id, None).is_ok() {
                return DefaultTemplatePreferenceDto {
                    template_id: stored_template_id,
                    resolved_template_id: template_id,
                    resolution_source: DefaultTemplateResolutionSource::UserDefault,
                };
            }
        }
        DefaultTemplatePreferenceDto {
            template_id: stored_template_id,
            resolved_template_id: BUILTIN_FALLBACK_TEMPLATE_ID.to_owned(),
            resolution_source: DefaultTemplateResolutionSource::BuiltinFallback,
        }
    }

    pub fn validate_default(
        &self,
        template_id: Option<String>,
    ) -> Result<DefaultTemplatePreferenceDto, TemplateApiError> {
        match template_id {
            Some(template_id) => {
                self.repository
                    .get(&template_id, None)
                    .map_err(TemplateApiError::from)?;
                Ok(DefaultTemplatePreferenceDto {
                    resolved_template_id: template_id.clone(),
                    template_id: Some(template_id),
                    resolution_source: DefaultTemplateResolutionSource::UserDefault,
                })
            }
            None => Ok(DefaultTemplatePreferenceDto {
                template_id: None,
                resolved_template_id: BUILTIN_FALLBACK_TEMPLATE_ID.to_owned(),
                resolution_source: DefaultTemplateResolutionSource::BuiltinFallback,
            }),
        }
    }

    fn next_available_id(&self, requested_base: &str) -> Result<String, TemplateApiError> {
        let listed = self.repository.list().map_err(TemplateApiError::from)?;
        let used: BTreeSet<&str> = listed
            .templates
            .iter()
            .map(|item| item.id.as_str())
            .collect();
        if !used.contains(requested_base) {
            return Ok(requested_base.to_owned());
        }
        for index in 2..=10_000 {
            let suffix = format!("_{index}");
            let keep = 64usize.saturating_sub(suffix.len());
            let base = truncate_ascii(requested_base, keep).trim_end_matches('_');
            let candidate = format!("{base}{suffix}");
            if !used.contains(candidate.as_str()) {
                return Ok(candidate);
            }
        }
        Err(TemplateApiError::from_code("TEMPLATE_ALREADY_EXISTS"))
    }

    fn validation_issues_for_stored_template(
        &self,
        template_id: &str,
        origin: Option<TemplateOrigin>,
    ) -> Vec<TemplateFieldIssueDto> {
        let Ok(bytes) = self
            .repository
            .read_bytes_for_diagnostics(template_id, origin)
        else {
            return Vec::new();
        };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            return vec![field_issue(
                "INVALID_JSON",
                "",
                "templates.validation.invalidJson",
            )];
        };
        if value.get("schema_version").is_none() {
            return vec![field_issue(
                "LEGACY_TEMPLATE_INVALID",
                "",
                "templates.validation.legacyInvalid",
            )];
        }
        validate_template_v2_value(&value)
            .errors
            .into_iter()
            .map(Into::into)
            .collect()
    }
}

fn validate_api_template_value(template: Value, mode: ValidationMode) -> ValidationResultDto {
    let transport_errors = api_transport_shape_issues(&template);
    let file_value = api_template_value_to_file_value(template);
    let validation = validate_template_v2_value(&file_value);
    let mut errors = transport_errors;
    errors.extend(
        validation
            .errors
            .into_iter()
            .map(TemplateFieldIssueDto::from),
    );
    let mut warnings: Vec<TemplateFieldIssueDto> =
        validation.warnings.into_iter().map(Into::into).collect();
    let normalized = if errors.is_empty() {
        match serde_json::from_value::<TemplateV2>(file_value) {
            Ok(template) => {
                if template.locale.is_none() {
                    warnings.push(field_issue(
                        "LOCALE_UNSET",
                        "/locale",
                        "templates.validation.localeUnset",
                    ));
                }
                if mode == ValidationMode::Create && template.version != 1 {
                    warnings.push(field_issue(
                        "VERSION_WILL_BE_NORMALIZED",
                        "/version",
                        "templates.validation.versionWillBeNormalized",
                    ));
                }
                Some(TemplateV2Dto::from(&template))
            }
            Err(_) => {
                errors.push(field_issue(
                    "DESERIALIZATION_FAILED",
                    "",
                    "templates.validation.deserializationFailed",
                ));
                None
            }
        }
    } else {
        None
    };
    ValidationResultDto {
        valid: errors.is_empty(),
        errors,
        warnings,
        normalized,
    }
}

fn api_transport_shape_issues(value: &Value) -> Vec<TemplateFieldIssueDto> {
    let mut issues = Vec::new();
    let Some(root) = value.as_object() else {
        return issues;
    };
    for file_key in ["schema_version", "created_at", "updated_at"] {
        if root.contains_key(file_key) {
            issues.push(field_issue(
                "TRANSPORT_FIELD_NAMING",
                &file_pointer_to_api_pointer(&format!("/{file_key}")),
                "templates.validation.transportFieldNaming",
            ));
        }
    }
    if let Some(source) = root.get("source").and_then(Value::as_object) {
        for file_key in [
            "original_file_name",
            "original_file_sha256",
            "imported_at",
            "copied_from_template_id",
        ] {
            if source.contains_key(file_key) {
                issues.push(field_issue(
                    "TRANSPORT_FIELD_NAMING",
                    &file_pointer_to_api_pointer(&format!("/source/{file_key}")),
                    "templates.validation.transportFieldNaming",
                ));
            }
        }
    }
    if let Some(sections) = root.get("sections").and_then(Value::as_array) {
        for (index, section) in sections.iter().enumerate() {
            let Some(section) = section.as_object() else {
                continue;
            };
            for file_key in ["item_format", "example_item_format", "empty_behavior"] {
                if section.contains_key(file_key) {
                    issues.push(field_issue(
                        "TRANSPORT_FIELD_NAMING",
                        &file_pointer_to_api_pointer(&format!("/sections/{index}/{file_key}")),
                        "templates.validation.transportFieldNaming",
                    ));
                }
            }
        }
    }
    issues
}

fn normalized_or_error(result: ValidationResultDto) -> Result<TemplateV2, TemplateApiError> {
    if !result.valid {
        return Err(TemplateApiError::from_validation(result.errors));
    }
    let normalized = result
        .normalized
        .ok_or_else(|| TemplateApiError::from_code("TEMPLATE_INVALID"))?;
    Ok(dto_to_file_template(normalized))
}

fn dto_to_file_template(dto: TemplateV2Dto) -> TemplateV2 {
    TemplateV2 {
        schema_version: dto.schema_version,
        id: dto.id,
        name: dto.name,
        description: dto.description,
        version: dto.version,
        locale: dto.locale,
        tags: dto.tags,
        source: TemplateSource {
            source_type: dto.source.source_type,
            original_file_name: dto.source.original_file_name,
            original_file_sha256: dto.source.original_file_sha256,
            imported_at: dto.source.imported_at,
            copied_from_template_id: dto.source.copied_from_template_id,
        },
        created_at: dto.created_at,
        updated_at: dto.updated_at,
        sections: dto
            .sections
            .into_iter()
            .map(|section| TemplateSectionV2 {
                id: section.id,
                title: section.title,
                instruction: section.instruction,
                format: section.format,
                item_format: section.item_format,
                example_item_format: section.example_item_format,
                required: section.required,
                empty_behavior: section.empty_behavior,
            })
            .collect(),
        extensions: dto.extensions,
    }
}

fn api_template_value_to_file_value(mut value: Value) -> Value {
    let Some(root) = value.as_object_mut() else {
        return value;
    };
    for (api_key, file_key) in [
        ("schemaVersion", "schema_version"),
        ("createdAt", "created_at"),
        ("updatedAt", "updated_at"),
    ] {
        rename_key_without_overwrite(root, api_key, file_key);
    }
    if let Some(source) = root.get_mut("source").and_then(Value::as_object_mut) {
        for (api_key, file_key) in [
            ("originalFileName", "original_file_name"),
            ("originalFileSha256", "original_file_sha256"),
            ("importedAt", "imported_at"),
            ("copiedFromTemplateId", "copied_from_template_id"),
        ] {
            rename_key_without_overwrite(source, api_key, file_key);
        }
    }
    if let Some(sections) = root.get_mut("sections").and_then(Value::as_array_mut) {
        for section in sections {
            if let Some(section) = section.as_object_mut() {
                for (api_key, file_key) in [
                    ("itemFormat", "item_format"),
                    ("exampleItemFormat", "example_item_format"),
                    ("emptyBehavior", "empty_behavior"),
                ] {
                    rename_key_without_overwrite(section, api_key, file_key);
                }
            }
        }
    }
    value
}

fn rename_key_without_overwrite(map: &mut Map<String, Value>, from: &str, to: &str) {
    let Some(value) = map.remove(from) else {
        return;
    };
    if map.contains_key(to) {
        map.insert(from.to_owned(), value);
    } else {
        map.insert(to.to_owned(), value);
    }
}

fn file_pointer_to_api_pointer(path: &str) -> String {
    path.split('/')
        .map(|segment| match segment {
            "schema_version" => "schemaVersion",
            "created_at" => "createdAt",
            "updated_at" => "updatedAt",
            "original_file_name" => "originalFileName",
            "original_file_sha256" => "originalFileSha256",
            "imported_at" => "importedAt",
            "copied_from_template_id" => "copiedFromTemplateId",
            "item_format" => "itemFormat",
            "example_item_format" => "exampleItemFormat",
            "empty_behavior" => "emptyBehavior",
            other => other,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn record_to_details(
    record: TemplateRecord,
    default_template_id: Option<&str>,
) -> TemplateDetailsDto {
    TemplateDetailsDto {
        is_default: default_template_id == Some(record.template.id.as_str()),
        template: TemplateV2Dto::from(&record.template),
        origin: record.origin,
        schema_version_on_disk: record.schema_version_on_disk,
        file_sha256: record.file_sha256,
        semantic_sha256: record.semantic_sha256,
        overrides_builtin: record.overrides_builtin,
        read_only: record.read_only,
    }
}

fn list_item_to_dto(
    item: TemplateListItem,
    default_template_id: Option<&str>,
) -> TemplateListItemDto {
    TemplateListItemDto {
        is_default: default_template_id == Some(item.id.as_str()),
        read_only: item.origin != TemplateOrigin::Custom,
        id: item.id,
        name: item.name,
        description: item.description,
        origin: item.origin,
        schema_version: item.schema_version,
        version: item.version,
        locale: item.locale,
        tags: item.tags,
        section_count: item.section_count,
        source_type: item.source_type,
        updated_at: item.updated_at,
        file_sha256: item.file_sha256,
        semantic_sha256: item.semantic_sha256,
        overrides_builtin: item.overrides_builtin,
        valid: item.valid,
        validation_summary: ValidationSummaryDto {
            error_count: item.validation_error_count,
            warning_count: 0,
        },
    }
}

fn delete_response(
    deleted: DeleteTemplateResult,
    usage: TemplateUsageDto,
) -> DeleteTemplateResponse {
    DeleteTemplateResponse {
        trash_id: deleted.trash_id,
        deleted_at: deleted.deleted_at,
        file_sha256: deleted.file_sha256,
        usage,
    }
}

fn origin_matches(requested: Option<ListOrigin>, actual: TemplateOrigin) -> bool {
    match requested.unwrap_or(ListOrigin::All) {
        ListOrigin::All => true,
        ListOrigin::Builtin => actual == TemplateOrigin::Builtin,
        ListOrigin::Bundled => actual == TemplateOrigin::Bundled,
        ListOrigin::Custom => actual == TemplateOrigin::Custom,
    }
}

fn suggest_duplicate_id(source_id: &str) -> String {
    let suffix = "_copy";
    let keep = 64usize.saturating_sub(suffix.len());
    format!(
        "{}{suffix}",
        truncate_ascii(source_id, keep).trim_end_matches('_')
    )
}

fn truncate_ascii(value: &str, max_len: usize) -> &str {
    &value[..value.len().min(max_len)]
}

fn verify_directory_is_still_writable(root: &Path) -> Result<(), TemplateApiError> {
    let probe = root
        .join(".tmp")
        .join(format!("writable-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)?;
        file.write_all(b"meetily-template-directory-probe")?;
        file.sync_all()?;
        std::fs::remove_file(&probe)
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&probe);
        let code = if error.kind() == std::io::ErrorKind::PermissionDenied {
            "TEMPLATE_DIRECTORY_NOT_WRITABLE"
        } else {
            "TEMPLATE_IO_ERROR"
        };
        let api_error = TemplateApiError::from_code(code);
        tracing::warn!(
            debug_id = %api_error.debug_id,
            code = %api_error.code,
            detail = %error,
            "templates directory write probe failed"
        );
        return Err(api_error);
    }
    Ok(())
}

fn field_issue(code: &str, path: &str, message_key: &str) -> TemplateFieldIssueDto {
    TemplateFieldIssueDto {
        code: code.to_owned(),
        path: path.to_owned(),
        message_key: message_key.to_owned(),
        params: BTreeMap::new(),
    }
}

fn message_key_for_code(code: &str) -> &'static str {
    match code {
        "TEMPLATE_NOT_FOUND" => "templates.errors.notFound",
        "TEMPLATE_INVALID" => "templates.errors.invalid",
        "TEMPLATE_ID_INVALID" => "templates.errors.invalidId",
        "TEMPLATE_ALREADY_EXISTS" => "templates.errors.alreadyExists",
        "TEMPLATE_CONFLICT" => "templates.errors.conflict",
        "TEMPLATE_READ_ONLY" => "templates.errors.readOnly",
        "TEMPLATE_IN_USE" => "templates.errors.inUse",
        "TEMPLATE_IS_DEFAULT" => "templates.errors.isDefault",
        "TEMPLATE_DIRECTORY_UNAVAILABLE" => "templates.errors.directoryUnavailable",
        "TEMPLATE_DIRECTORY_NOT_WRITABLE" => "templates.errors.directoryNotWritable",
        "TEMPLATE_DISK_FULL" => "templates.errors.diskFull",
        "TEMPLATE_PATH_REJECTED" => "templates.errors.pathRejected",
        "TEMPLATE_IMPORT_UNSUPPORTED" => "templates.errors.importUnsupported",
        "TEMPLATE_JSON_INVALID" => "templates.errors.invalidJson",
        "TEMPLATE_JSON_ENCODING_INVALID" => "templates.errors.invalidJsonEncoding",
        "TEMPLATE_EXPORT_FAILED" => "templates.errors.exportFailed",
        "TEMPLATE_DOC_CONVERTER_MISSING" => "templates.errors.docConverterMissing",
        "TEMPLATE_DOC_CONVERSION_FAILED" => "templates.errors.docConversionFailed",
        "TEMPLATE_DOCX_INVALID" => "templates.errors.docxInvalid",
        "TEMPLATE_ARCHIVE_TOO_LARGE" => "templates.errors.archiveTooLarge",
        "TEMPLATE_CANCELLED" => "templates.errors.cancelled",
        "TEMPLATE_PACK_INVALID" => "templates.errors.packInvalid",
        "TEMPLATE_PACK_VERSION_UNSUPPORTED" => "templates.errors.packVersionUnsupported",
        "TEMPLATE_PACK_INTEGRITY_FAILED" => "templates.errors.packIntegrityFailed",
        "TEMPLATE_PACK_BUDGET_EXCEEDED" => "templates.errors.packBudgetExceeded",
        "TEMPLATE_PACK_UNSAFE_ENTRY" => "templates.errors.packUnsafeEntry",
        "TEMPLATE_PACK_SENSITIVE_CONTENT" => "templates.errors.packSensitiveContent",
        "TEMPLATE_PACK_CONFLICT_UNRESOLVED" => "templates.errors.packConflictUnresolved",
        "TEMPLATE_PACK_PLAN_STALE" => "templates.errors.packPlanStale",
        "TEMPLATE_PACK_PREVIEW_PLAN_NOT_FOUND" => "templates.errors.packPreviewPlanNotFound",
        "TEMPLATE_PACK_PREVIEW_PLAN_EXPIRED" => "templates.errors.packPreviewPlanExpired",
        "TEMPLATE_PACK_PREVIEW_PLAN_CONSUMED" => "templates.errors.packPreviewPlanConsumed",
        "TEMPLATE_PACK_DECISION_MISSING" => "templates.errors.packDecisionMissing",
        "TEMPLATE_PACK_DECISION_DUPLICATE" => "templates.errors.packDecisionDuplicate",
        "TEMPLATE_PACK_DECISION_UNKNOWN_ITEM" => "templates.errors.packDecisionUnknownItem",
        "TEMPLATE_PACK_DECISION_NOT_ALLOWED" => "templates.errors.packDecisionNotAllowed",
        "TEMPLATE_PACK_KEEP_BOTH_ID_EXHAUSTED" => "templates.errors.packKeepBothIdExhausted",
        "TEMPLATE_PACK_CONFLICT_CHANGED" => "templates.errors.packConflictChanged",
        "TEMPLATE_PACK_EXECUTION_PLAN_NOT_FOUND" => "templates.errors.packExecutionPlanNotFound",
        "TEMPLATE_PACK_EXECUTION_PLAN_EXPIRED" => "templates.errors.packExecutionPlanExpired",
        "TEMPLATE_PACK_EXECUTION_PLAN_CONSUMED" => "templates.errors.packExecutionPlanConsumed",
        "TEMPLATE_PACK_EXECUTION_NOT_FOUND" => "templates.errors.packExecutionNotFound",
        "TEMPLATE_PACK_EXECUTION_ALREADY_ACTIVE" => "templates.errors.packExecutionAlreadyActive",
        "TEMPLATE_PACK_RECOVERY_FAILED" => "templates.errors.packRecoveryFailed",
        "TEMPLATE_PACK_EXPORT_FAILED" => "templates.errors.packExportFailed",
        "TEMPLATE_PACK_IMPORT_FAILED" => "templates.errors.packImportFailed",
        "MEETING_NOT_FOUND" => "meetings.errors.notFound",
        "MEETING_TEMPLATE_SNAPSHOT_FAILED" => "templates.errors.snapshotFailed",
        "SUMMARY_SOURCE_BINDING_FAILED" => "summary.errors.sourceBindingFailed",
        "STORAGE_OPERATION_BUSY" => "templates.errors.storageOperationBusy",
        _ => "templates.errors.io",
    }
}

fn retryable_for_code(code: &str) -> bool {
    matches!(
        code,
        "TEMPLATE_CONFLICT"
            | "TEMPLATE_DIRECTORY_UNAVAILABLE"
            | "TEMPLATE_DIRECTORY_NOT_WRITABLE"
            | "TEMPLATE_IO_ERROR"
            | "TEMPLATE_DISK_FULL"
            | "TEMPLATE_DOC_CONVERSION_FAILED"
            | "TEMPLATE_PACK_PLAN_STALE"
            | "TEMPLATE_PACK_PREVIEW_PLAN_NOT_FOUND"
            | "TEMPLATE_PACK_PREVIEW_PLAN_EXPIRED"
            | "TEMPLATE_PACK_PREVIEW_PLAN_CONSUMED"
            | "TEMPLATE_PACK_CONFLICT_CHANGED"
            | "TEMPLATE_PACK_EXECUTION_PLAN_NOT_FOUND"
            | "TEMPLATE_PACK_EXECUTION_PLAN_EXPIRED"
            | "TEMPLATE_PACK_EXECUTION_PLAN_CONSUMED"
            | "TEMPLATE_PACK_EXECUTION_NOT_FOUND"
            | "TEMPLATE_PACK_RECOVERY_FAILED"
            | "TEMPLATE_PACK_EXPORT_FAILED"
            | "TEMPLATE_PACK_IMPORT_FAILED"
            | "MEETING_TEMPLATE_SNAPSHOT_FAILED"
            | "SUMMARY_SOURCE_BINDING_FAILED"
            | "STORAGE_OPERATION_BUSY"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;
    use tempfile::TempDir;

    fn now() -> DateTime<FixedOffset> {
        FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 8, 23, 12, 0, 0)
            .unwrap()
    }

    fn api_template(id: &str, name: &str) -> Value {
        json!({
            "schemaVersion": 2,
            "id": id,
            "name": name,
            "description": "A service test template",
            "version": 1,
            "locale": null,
            "tags": ["test"],
            "source": {
                "type": "manual",
                "originalFileName": null,
                "originalFileSha256": null,
                "importedAt": null,
                "copiedFromTemplateId": null
            },
            "createdAt": now().to_rfc3339(),
            "updatedAt": now().to_rfc3339(),
            "sections": [{
                "id": "summary",
                "title": "Summary",
                "instruction": "Summarize the meeting",
                "format": "paragraph",
                "itemFormat": null,
                "exampleItemFormat": null,
                "required": true,
                "emptyBehavior": "show_not_mentioned"
            }],
            "extensions": {"vendorKey": {"keepCase": true}}
        })
    }

    fn service() -> (TempDir, TemplateService) {
        let temporary = TempDir::new().unwrap();
        let repository = TemplateRepository::new(temporary.path().join("templates"), None).unwrap();
        (temporary, TemplateService::new(repository))
    }

    fn create(service: &TemplateService, id: &str, name: &str) -> TemplateDetailsDto {
        service
            .create(
                CreateTemplateRequest {
                    template: api_template(id, name),
                    conflict_policy: ApiCreateConflictPolicy::Error,
                },
                None,
            )
            .unwrap()
    }

    #[test]
    fn camel_case_transport_round_trips_without_changing_extension_keys() {
        let result = validate_api_template_value(
            api_template("transport", "Transport"),
            ValidationMode::Preview,
        );
        assert!(result.valid, "{:?}", result.errors);
        let serialized = serde_json::to_value(result.normalized.unwrap()).unwrap();
        assert_eq!(serialized["schemaVersion"], 2);
        assert_eq!(
            serialized["sections"][0]["emptyBehavior"],
            "show_not_mentioned"
        );
        assert_eq!(serialized["extensions"]["vendorKey"]["keepCase"], true);
        assert!(serialized.get("schema_version").is_none());
    }

    #[test]
    fn validation_returns_camel_case_paths_and_no_normalized_value_on_error() {
        let mut value = api_template("invalid_service", "Invalid");
        value["updatedAt"] = Value::String("2020-01-01T00:00:00Z".to_owned());
        let result = validate_api_template_value(value, ValidationMode::Update);
        assert!(!result.valid);
        assert!(result.normalized.is_none());
        assert!(result.errors.iter().any(|issue| issue.path == "/updatedAt"));
    }

    #[test]
    fn transport_rejects_snake_case_aliases_instead_of_accepting_two_api_shapes() {
        let file_value = api_template_value_to_file_value(api_template("snake", "Snake"));
        let result = validate_api_template_value(file_value, ValidationMode::Preview);
        assert!(!result.valid);
        assert!(result
            .errors
            .iter()
            .any(|issue| issue.code == "TRANSPORT_FIELD_NAMING"));
    }

    #[test]
    fn service_crud_duplicate_restore_and_purge_flow() {
        let (_temporary, service) = service();
        let created = create(&service, "service_flow", "Service Flow");
        assert_eq!(created.template.version, 1);

        let listed = service
            .list(ListTemplatesRequest::default(), Some("service_flow"))
            .unwrap();
        let item = listed
            .templates
            .iter()
            .find(|item| item.id == "service_flow")
            .unwrap();
        assert!(item.is_default);

        let mut replacement = serde_json::to_value(&created.template).unwrap();
        replacement["name"] = Value::String("Updated Service Flow".to_owned());
        let updated = service
            .update(
                UpdateTemplateRequest {
                    template_id: "service_flow".to_owned(),
                    expected_version: created.template.version,
                    expected_file_sha256: created.file_sha256,
                    template: replacement,
                },
                None,
            )
            .unwrap();
        assert_eq!(updated.template.version, 2);

        let duplicated = service
            .duplicate(
                DuplicateTemplateRequest {
                    template_id: "service_flow".to_owned(),
                    origin: None,
                    new_name: "Duplicated".to_owned(),
                    requested_id: None,
                },
                None,
            )
            .unwrap();
        assert_eq!(duplicated.template.id, "service_flow_copy");

        let deleted = service
            .delete(
                DeleteTemplateRequest {
                    template_id: duplicated.template.id,
                    expected_file_sha256: duplicated.file_sha256,
                    replacement_default_template_id: None,
                },
                None,
            )
            .unwrap();
        let restored = service
            .restore(
                RestoreTemplateRequest {
                    trash_id: deleted.trash_id,
                    conflict_policy: ApiRestoreConflictPolicy::Error,
                },
                None,
            )
            .unwrap();
        let deleted_again = service
            .delete(
                DeleteTemplateRequest {
                    template_id: restored.template.id,
                    expected_file_sha256: restored.file_sha256,
                    replacement_default_template_id: None,
                },
                None,
            )
            .unwrap();
        service
            .purge(PurgeTemplateRequest {
                trash_id: deleted_again.trash_id,
            })
            .unwrap();
        assert!(service.list_deleted().unwrap().is_empty());
    }

    #[test]
    fn keep_both_and_builtin_override_are_distinct_policies() {
        let (_temporary, service) = service();
        create(&service, "same_id", "First");
        let duplicate = service
            .create(
                CreateTemplateRequest {
                    template: api_template("same_id", "Second"),
                    conflict_policy: ApiCreateConflictPolicy::KeepBoth,
                },
                None,
            )
            .unwrap();
        assert_eq!(duplicate.template.id, "same_id_2");

        let error = service
            .create(
                CreateTemplateRequest {
                    template: api_template("standard_meeting", "Override"),
                    conflict_policy: ApiCreateConflictPolicy::Error,
                },
                None,
            )
            .unwrap_err();
        assert_eq!(error.code, "TEMPLATE_ALREADY_EXISTS");
        let overridden = service
            .create(
                CreateTemplateRequest {
                    template: api_template("standard_meeting", "Override"),
                    conflict_policy: ApiCreateConflictPolicy::OverrideBuiltin,
                },
                None,
            )
            .unwrap();
        assert!(overridden.overrides_builtin);
    }

    #[test]
    fn conflicts_and_default_delete_are_structured_without_detail_leaks() {
        let (_temporary, service) = service();
        let created = create(&service, "protected", "Protected");
        let default_error = service
            .delete(
                DeleteTemplateRequest {
                    template_id: "protected".to_owned(),
                    expected_file_sha256: created.file_sha256.clone(),
                    replacement_default_template_id: None,
                },
                Some("protected"),
            )
            .unwrap_err();
        assert_eq!(default_error.code, "TEMPLATE_IS_DEFAULT");

        let mut replacement = serde_json::to_value(&created.template).unwrap();
        replacement["name"] = Value::String("Conflict".to_owned());
        let conflict = service
            .update(
                UpdateTemplateRequest {
                    template_id: "protected".to_owned(),
                    expected_version: created.template.version,
                    expected_file_sha256: "0".repeat(64),
                    template: replacement,
                },
                None,
            )
            .unwrap_err();
        assert_eq!(conflict.code, "TEMPLATE_CONFLICT");
        assert!(conflict.retryable);
        assert!(!serde_json::to_string(&conflict)
            .unwrap()
            .contains("changed"));
    }

    #[test]
    fn default_target_is_validated_and_stale_value_falls_back() {
        let (_temporary, service) = service();
        let missing = service
            .validate_default(Some("missing".to_owned()))
            .unwrap_err();
        assert_eq!(missing.code, "TEMPLATE_NOT_FOUND");
        let fallback = service.get_default(Some("missing".to_owned()));
        assert_eq!(fallback.template_id.as_deref(), Some("missing"));
        assert_eq!(fallback.resolved_template_id, BUILTIN_FALLBACK_TEMPLATE_ID);
        assert_eq!(
            fallback.resolution_source,
            DefaultTemplateResolutionSource::BuiltinFallback
        );
    }

    #[test]
    fn directory_probe_is_scoped_to_repository_and_is_cleaned() {
        let (_temporary, service) = service();
        let info = service.directory_info().unwrap();
        assert!(info.exists && info.writable);
        assert_eq!(Path::new(&info.path), service.repository().root());
        assert_eq!(
            std::fs::read_dir(service.repository().root().join(".tmp"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn lightweight_list_filters_and_never_contains_instructions() {
        let (_temporary, service) = service();
        create(&service, "filter_me", "Needle Template");
        std::fs::write(service.repository().root().join("broken.json"), b"not-json").unwrap();
        let filtered = service
            .list(
                ListTemplatesRequest {
                    origin: Some(ListOrigin::Custom),
                    include_invalid: Some(false),
                    include_trash: Some(false),
                    query: Some("needle".to_owned()),
                    content_locale: None,
                },
                None,
            )
            .unwrap();
        assert_eq!(filtered.templates.len(), 1);
        assert!(!serde_json::to_string(&filtered)
            .unwrap()
            .contains("Summarize the meeting"));
    }

    #[test]
    fn invalid_stored_template_get_returns_field_level_camel_case_issues() {
        let (_temporary, service) = service();
        let mut value = api_template("bad_stored", "Bad Stored");
        value["updatedAt"] = Value::String("2020-01-01T00:00:00Z".to_owned());
        let file_value = api_template_value_to_file_value(value);
        std::fs::write(
            service.repository().root().join("bad_stored.json"),
            serde_json::to_vec(&file_value).unwrap(),
        )
        .unwrap();
        let error = service
            .get(
                GetTemplateRequest {
                    template_id: "bad_stored".to_owned(),
                    origin: None,
                    content_locale: None,
                },
                None,
            )
            .unwrap_err();
        assert_eq!(error.code, "TEMPLATE_INVALID");
        assert!(error
            .field_errors
            .iter()
            .any(|issue| issue.path == "/updatedAt"));
    }
}
