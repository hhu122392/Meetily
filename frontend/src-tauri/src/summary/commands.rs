use crate::database::repositories::{
    meeting::MeetingsRepository,
    summary::{summary_error_category, NewSummaryGenerationHistory, SummaryProcessesRepository},
    transcript_chunk::TranscriptChunksRepository,
};
use crate::meeting_context::{
    validate_summary_markdown_with_source, SummaryFactValidation, SummaryFactValidationStatus,
    SummaryFactWarning, SummaryMeetingContext,
};
use crate::state::AppState;
use crate::storage::operation_lock::{begin_storage_operation, StorageOperationKind};
use crate::summary::language_detection::{detect_summary_language, SummaryLanguageDetection};
use crate::summary::measurement::{self, MeasurementOutcome, SummaryMeasurement};
use crate::summary::metadata::{
    read_detected_summary_language_from_metadata, read_summary_language_from_metadata,
    write_detected_summary_language_to_metadata, write_summary_language_to_metadata,
};
use crate::summary::service::SummaryService;
use crate::summary::source_binding::{
    evaluate_summary_freshness, SummarySourceBinding, SummaryTemplateBinding,
    TranscriptVersionSnapshot,
};
use crate::summary::source_repository::resolve_active_summary_input;
use crate::summary::template_commands_v2::{resolve_template_for_generation, TemplateServiceState};
use crate::summary::template_snapshot::{
    capture_snapshot, remove_snapshot_after_preflight_failure,
    resolve_summary_context_for_generation, sha256_text, snapshot_link_json,
    GenerationSnapshotLink, ResolvedTemplateSummary, SnapshotGenerationContext,
    SnapshotModelContext,
};
use crate::summary::templates::TemplateApiError;
use log::{error as log_error, info as log_info, warn as log_warn};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Runtime};

#[derive(Debug, Serialize, Deserialize)]
pub struct SummaryResponse {
    pub status: String,
    #[serde(rename = "meetingName")]
    pub meeting_name: Option<String>,
    pub meeting_id: String,
    pub start: Option<String>,
    pub end: Option<String>,
    pub data: Option<serde_json::Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualSummaryRevisionDto {
    pub revision_id: String,
    pub created_at: String,
    pub source_generation_id: Option<String>,
    pub markdown: Option<String>,
    pub summary: serde_json::Value,
    pub is_current: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessTranscriptResponse {
    pub message: String,
    pub process_id: String,
    #[serde(rename = "process_id")]
    pub legacy_process_id: String,
    pub generation_id: String,
    pub resolved_template: ResolvedTemplateSummary,
    pub snapshot_path_relative: String,
    pub meeting_context_id: Option<String>,
    pub meeting_context_sha256: Option<String>,
    pub summary_context_sha256: Option<String>,
    pub source_binding: SummarySourceBinding,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BeginSummaryMeasurementResponse {
    pub generation_id: String,
    pub timed_endpoint_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SummaryLanguageStorage {
    Metadata,
    LocalFallback,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MeetingSummaryLanguagePreference {
    pub language: Option<String>,
    pub storage: SummaryLanguageStorage,
}

impl MeetingSummaryLanguagePreference {
    fn metadata(language: Option<String>) -> Self {
        Self {
            language,
            storage: SummaryLanguageStorage::Metadata,
        }
    }

    fn local_fallback() -> Self {
        Self {
            language: None,
            storage: SummaryLanguageStorage::LocalFallback,
        }
    }
}

enum MeetingFolderResolution {
    Folder(PathBuf),
    NoFolder,
}

/// Saves a meeting summary (Native SQLx implementation)
///
/// Expected format: { "markdown": "...", "summary_json": [...BlockNote blocks...] }
#[tauri::command]
pub async fn api_save_meeting_summary<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    template_state: tauri::State<'_, TemplateServiceState>,
    meeting_id: String,
    summary: serde_json::Value,
    _auth_token: Option<String>,
) -> Result<serde_json::Value, String> {
    log_info!(
        "api_save_meeting_summary (native) called for meeting_id: {}",
        meeting_id
    );
    let pool = state.db_manager.pool();
    let (summary, fact_validation) =
        match revalidate_summary_against_current_evidence(pool, &meeting_id, &summary).await {
            Ok(validated) => validated,
            Err(error) => {
                // A broken/missing evidence file must never turn a valid human
                // edit into unsavable data. Preserve the Markdown, clearly mark
                // it unverified, and let the database write be the only reason
                // the save itself can fail.
                log_warn!(
                    "Summary evidence could not be loaded while saving {}: {}",
                    meeting_id,
                    error
                );
                summary_with_unavailable_validation(&summary)
            }
        };
    let trusted_fact_validation = serde_json::to_value(&fact_validation)
        .map_err(|error| format!("Failed to serialize fact validation: {error}"))?;

    match SummaryProcessesRepository::update_meeting_summary(
        pool,
        &meeting_id,
        &summary,
        &trusted_fact_validation,
    )
    .await
    {
        Ok(true) => {
            log_info!("Summary saved successfully for meeting_id: {}", meeting_id);
            let fallback_summary = with_fact_validation(summary.clone(), &fact_validation);
            let saved_summary =
                match SummaryProcessesRepository::get_summary_data(pool, &meeting_id).await {
                    Ok(Some(process)) => process
                        .result
                        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                        .unwrap_or(fallback_summary),
                    _ => fallback_summary,
                };
            let saved_summary =
                attach_summary_freshness(&app, pool, &template_state, &meeting_id, saved_summary)
                    .await;
            Ok(serde_json::json!({
                "message": "Meeting summary saved successfully",
                "factValidation": fact_validation,
                "summary": saved_summary
            }))
        }
        Ok(false) => {
            log_warn!(
                "Meeting not found or invalid JSON for meeting_id: {}",
                meeting_id
            );
            Err("Meeting not found or can't convert the json".into())
        }
        Err(e) => {
            log_error!("Failed to save meeting summary for {}: {}", meeting_id, e);
            Err(e.to_string())
        }
    }
}

async fn revalidate_summary_against_current_evidence(
    pool: &sqlx::SqlitePool,
    meeting_id: &str,
    summary: &serde_json::Value,
) -> Result<(serde_json::Value, SummaryFactValidation), String> {
    let context = match resolve_meeting_folder(pool, meeting_id).await? {
        MeetingFolderResolution::Folder(folder) => {
            resolve_summary_context_for_generation(&folder, meeting_id, None)
                .map_err(|error| format!("Failed to load current meeting context: {error}"))?
        }
        MeetingFolderResolution::NoFolder => None,
    };
    let summary_input = resolve_active_summary_input(pool, meeting_id).await?;
    revalidate_summary_payload(summary, context.as_ref(), &summary_input.source)
}

fn revalidate_summary_payload(
    summary: &serde_json::Value,
    context: Option<&SummaryMeetingContext>,
    source: &TranscriptVersionSnapshot,
) -> Result<(serde_json::Value, SummaryFactValidation), String> {
    let mut summary = summary.clone();
    let Some(object) = summary.as_object_mut() else {
        return Ok((
            summary,
            unavailable_fact_validation(
                "summary_validation_unavailable",
                "summary:factValidation.validationUnavailable",
            ),
        ));
    };
    // Never trust a validation result supplied by the WebView. The repository
    // receives the native result through a separate parameter.
    object.remove("factValidation");
    object.remove("summaryFreshness");
    // This provenance marker belongs only to the active copy created by the
    // restore command. A subsequent human save creates a new current revision
    // and must not keep claiming that an older revision is current.
    object.remove("restoredRevisionId");
    let Some(markdown) = object
        .get("markdown")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
    else {
        return Ok((
            summary,
            unavailable_fact_validation(
                "summary_markdown_unavailable",
                "summary:factValidation.summaryMarkdownUnavailable",
            ),
        ));
    };

    // Markdown is the only native-validated body. Keeping independently
    // supplied BlockNote JSON would allow the UI to render content different
    // from what was fact-checked. Reopening reconstructs editor blocks from the
    // authoritative Markdown instead.
    object.remove("summary_json");

    let validated = validate_summary_markdown_with_source(&markdown, context, source)
        .map_err(|error| format!("Summary source evidence is invalid: {error}"))?;
    let alias_normalized_markdown = context
        .map(|context| context.normalize_known_aliases(&markdown))
        .unwrap_or_else(|| markdown.clone());
    let mut validation = validated.validation;

    // Generated summaries are stored with the deterministic sanitizer applied.
    // Human edits must be preserved verbatim, so when that sanitizer would have
    // changed a high-risk field, retain the edit but force an explicit review.
    if normalized_markdown_for_comparison(&validated.markdown)
        != normalized_markdown_for_comparison(&alias_normalized_markdown)
    {
        push_local_fact_warning(
            &mut validation,
            "manual_high_risk_fields_unverified",
            "summary:factValidation.manualHighRiskFieldsUnverified",
        );
    }
    // The saved body remains exactly what the person authored; do not claim
    // that aliases were rewritten merely because the validator used a
    // normalized comparison copy.
    validation.aliases_normalized = false;

    Ok((summary, validation))
}

fn normalized_markdown_for_comparison(markdown: &str) -> String {
    markdown
        .replace("\r\n", "\n")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

fn push_local_fact_warning(validation: &mut SummaryFactValidation, code: &str, message_key: &str) {
    if validation
        .warnings
        .iter()
        .any(|warning| warning.code == code)
    {
        return;
    }
    validation.warnings.push(SummaryFactWarning {
        code: code.to_owned(),
        message_key: message_key.to_owned(),
    });
    validation.warning_count = validation.warnings.len();
    validation.status = SummaryFactValidationStatus::NeedsReview;
}

fn summary_with_unavailable_validation(
    summary: &serde_json::Value,
) -> (serde_json::Value, SummaryFactValidation) {
    let mut summary = summary.clone();
    if let Some(object) = summary.as_object_mut() {
        object.remove("factValidation");
        object.remove("restoredRevisionId");
        if object
            .get("markdown")
            .and_then(serde_json::Value::as_str)
            .is_some()
        {
            object.remove("summary_json");
        }
    }
    (
        summary,
        unavailable_fact_validation(
            "summary_validation_unavailable",
            "summary:factValidation.validationUnavailable",
        ),
    )
}

fn with_fact_validation(
    mut summary: serde_json::Value,
    validation: &SummaryFactValidation,
) -> serde_json::Value {
    if let Some(object) = summary.as_object_mut() {
        object.insert(
            "factValidation".to_owned(),
            serde_json::to_value(validation).unwrap_or(serde_json::Value::Null),
        );
    }
    summary
}

fn unavailable_fact_validation(code: &str, message_key: &str) -> SummaryFactValidation {
    SummaryFactValidation {
        status: SummaryFactValidationStatus::NeedsReview,
        warning_count: 1,
        warnings: vec![SummaryFactWarning {
            code: code.to_owned(),
            message_key: message_key.to_owned(),
        }],
        aliases_normalized: false,
        meeting_context_id: None,
        meeting_context_sha256: None,
        summary_context_sha256: None,
        field_traces: Vec::new(),
        source_evidence: None,
    }
}

/// Lists immutable user-saved summary revisions for a meeting.
#[tauri::command]
pub async fn api_list_manual_summary_revisions<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<ManualSummaryRevisionDto>, String> {
    let pool = state.db_manager.pool();
    let current_result: Option<String> =
        sqlx::query_scalar("SELECT result FROM summary_processes WHERE meeting_id = ?")
            .bind(&meeting_id)
            .fetch_optional(pool)
            .await
            .map_err(|error| error.to_string())?
            .flatten();
    let restored_revision_id = current_result
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| {
            value
                .get("restoredRevisionId")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        });
    let rows: Vec<(String, String, Option<String>, String, Option<String>)> = sqlx::query_as(
        r#"
        SELECT revision_id, created_at, source_generation_id, summary_json, markdown
        FROM summary_manual_revisions
        WHERE meeting_id = ?
        ORDER BY created_at DESC
        LIMIT 200
        "#,
    )
    .bind(&meeting_id)
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?;

    rows.into_iter()
        .map(
            |(revision_id, created_at, source_generation_id, summary_json, markdown)| {
                let summary =
                    serde_json::from_str::<serde_json::Value>(&summary_json).map_err(|error| {
                        format!("Stored manual summary revision is invalid: {error}")
                    })?;
                let is_current = current_result.as_deref() == Some(summary_json.as_str())
                    || restored_revision_id.as_deref() == Some(revision_id.as_str());
                Ok(ManualSummaryRevisionDto {
                    revision_id,
                    created_at,
                    source_generation_id,
                    markdown,
                    is_current,
                    summary,
                })
            },
        )
        .collect()
}

/// Restores one immutable user-saved revision without deleting later history.
#[tauri::command]
pub async fn api_restore_manual_summary_revision<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    template_state: tauri::State<'_, TemplateServiceState>,
    meeting_id: String,
    revision_id: String,
) -> Result<serde_json::Value, String> {
    let pool = state.db_manager.pool();
    let summary_json: Option<String> = sqlx::query_scalar(
        "SELECT summary_json FROM summary_manual_revisions WHERE meeting_id = ? AND revision_id = ?",
    )
    .bind(&meeting_id)
    .bind(&revision_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?;
    let summary_json =
        summary_json.ok_or_else(|| "Manual summary revision was not found".to_owned())?;
    let stored_summary = serde_json::from_str::<serde_json::Value>(&summary_json)
        .map_err(|error| format!("Stored manual summary revision is invalid: {error}"))?;
    let (summary, mut fact_validation) =
        match revalidate_summary_against_current_evidence(pool, &meeting_id, &stored_summary).await
        {
            Ok(validated) => validated,
            Err(error) => {
                log_warn!(
                    "Restored summary evidence could not be loaded for {}: {}",
                    meeting_id,
                    error
                );
                summary_with_unavailable_validation(&stored_summary)
            }
        };
    fact_validation.retain_saved_omission_warnings(&stored_summary);
    let mut summary = with_fact_validation(summary, &fact_validation);
    if let Some(object) = summary.as_object_mut() {
        object.insert(
            "restoredRevisionId".to_owned(),
            serde_json::Value::String(revision_id.clone()),
        );
    }
    let summary_json = serde_json::to_string(&summary)
        .map_err(|error| format!("Revalidated manual summary revision is invalid: {error}"))?;
    let now = chrono::Utc::now();
    let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
    let update = sqlx::query(
        r#"
        UPDATE summary_processes
        SET status = 'completed', result = ?, updated_at = ?, error = NULL,
            result_backup = NULL, result_backup_timestamp = NULL
        WHERE meeting_id = ?
        "#,
    )
    .bind(&summary_json)
    .bind(now)
    .bind(&meeting_id)
    .execute(&mut *transaction)
    .await
    .map_err(|error| error.to_string())?;
    if update.rows_affected() != 1 {
        transaction
            .rollback()
            .await
            .map_err(|error| error.to_string())?;
        return Err("Meeting summary process was not found".to_owned());
    }
    sqlx::query("UPDATE meetings SET updated_at = ? WHERE id = ?")
        .bind(now)
        .bind(&meeting_id)
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    Ok(attach_summary_freshness(&app, pool, &template_state, &meeting_id, summary).await)
}

/// Gets the per-meeting summary language override from metadata.json.
#[tauri::command]
pub async fn api_get_meeting_summary_language<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<MeetingSummaryLanguagePreference, String> {
    log_info!(
        "api_get_meeting_summary_language called for meeting_id: {}",
        meeting_id
    );

    match resolve_meeting_folder(state.db_manager.pool(), &meeting_id).await? {
        MeetingFolderResolution::Folder(folder) => read_summary_language_from_metadata(&folder)
            .map(MeetingSummaryLanguagePreference::metadata)
            .map_err(|e| e.to_string()),
        MeetingFolderResolution::NoFolder => Ok(MeetingSummaryLanguagePreference::local_fallback()),
    }
}

/// Saves or clears the per-meeting summary language override in metadata.json.
#[tauri::command]
pub async fn api_save_meeting_summary_language<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    summary_language: Option<String>,
) -> Result<MeetingSummaryLanguagePreference, String> {
    log_info!(
        "api_save_meeting_summary_language called for meeting_id: {}, language: {:?}",
        meeting_id,
        summary_language
    );

    match resolve_meeting_folder(state.db_manager.pool(), &meeting_id).await? {
        MeetingFolderResolution::Folder(folder) => {
            write_summary_language_to_metadata(&folder, summary_language.as_deref())
                .map_err(|e| e.to_string())?;
            read_summary_language_from_metadata(&folder)
                .map(MeetingSummaryLanguagePreference::metadata)
                .map_err(|e| e.to_string())
        }
        MeetingFolderResolution::NoFolder => Ok(MeetingSummaryLanguagePreference::local_fallback()),
    }
}

/// Gets the cached Auto-detected summary language from metadata.json.
#[tauri::command]
pub async fn api_get_meeting_detected_summary_language<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<MeetingSummaryLanguagePreference, String> {
    log_info!(
        "api_get_meeting_detected_summary_language called for meeting_id: {}",
        meeting_id
    );

    match resolve_meeting_folder(state.db_manager.pool(), &meeting_id).await? {
        MeetingFolderResolution::Folder(folder) => {
            read_detected_summary_language_from_metadata(&folder)
                .map(MeetingSummaryLanguagePreference::metadata)
                .map_err(|e| e.to_string())
        }
        MeetingFolderResolution::NoFolder => Ok(MeetingSummaryLanguagePreference::local_fallback()),
    }
}

/// Saves or clears the cached Auto-detected summary language in metadata.json.
#[tauri::command]
pub async fn api_save_meeting_detected_summary_language<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    detected_summary_language: Option<String>,
) -> Result<MeetingSummaryLanguagePreference, String> {
    log_info!(
        "api_save_meeting_detected_summary_language called for meeting_id: {}, language: {:?}",
        meeting_id,
        detected_summary_language
    );

    match resolve_meeting_folder(state.db_manager.pool(), &meeting_id).await? {
        MeetingFolderResolution::Folder(folder) => {
            write_detected_summary_language_to_metadata(
                &folder,
                detected_summary_language.as_deref(),
            )
            .map_err(|e| e.to_string())?;
            read_detected_summary_language_from_metadata(&folder)
                .map(MeetingSummaryLanguagePreference::metadata)
                .map_err(|e| e.to_string())
        }
        MeetingFolderResolution::NoFolder => Ok(MeetingSummaryLanguagePreference::local_fallback()),
    }
}

/// Detects the dominant supported summary language from transcript segments.
#[tauri::command]
pub async fn api_detect_transcript_summary_language(
    transcript_texts: Vec<String>,
) -> Result<SummaryLanguageDetection, String> {
    Ok(detect_summary_language(&transcript_texts))
}

async fn resolve_meeting_folder(
    pool: &sqlx::SqlitePool,
    meeting_id: &str,
) -> Result<MeetingFolderResolution, String> {
    let meeting = MeetingsRepository::get_meeting_metadata(pool, meeting_id)
        .await
        .map_err(|e| format!("Failed to load meeting metadata: {}", e))?
        .ok_or_else(|| format!("Meeting not found: {}", meeting_id))?;

    let Some(folder_path) = meeting.folder_path.filter(|p| !p.trim().is_empty()) else {
        return Ok(MeetingFolderResolution::NoFolder);
    };

    Ok(MeetingFolderResolution::Folder(PathBuf::from(folder_path)))
}

/// Starts the D-12 click-to-page measurement before any transcript wait.
#[tauri::command]
pub async fn api_begin_summary_measurement(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    click_at: String,
    click_monotonic_ns: u64,
) -> Result<BeginSummaryMeasurementResponse, String> {
    use uuid::Uuid;

    let folder = match resolve_meeting_folder(state.db_manager.pool(), &meeting_id).await? {
        MeetingFolderResolution::Folder(folder) => folder,
        MeetingFolderResolution::NoFolder => {
            return Err("meeting has no controlled folder for summary measurement".to_string())
        }
    };
    let generation_id = format!("gen_{}", Uuid::new_v4().simple());
    measurement::begin_measurement(
        &folder,
        &meeting_id,
        generation_id.clone(),
        click_at,
        click_monotonic_ns,
    )?;
    Ok(BeginSummaryMeasurementResponse {
        generation_id,
        timed_endpoint_id: measurement::TIMED_ENDPOINT_ID.to_string(),
    })
}

/// Adds a WebView monotonic interval to the backend-owned generation record.
#[tauri::command]
pub async fn api_record_summary_frontend_stage(
    meeting_id: String,
    generation_id: String,
    stage: String,
    monotonic_start_ns: u64,
    monotonic_end_ns: u64,
) -> Result<(), String> {
    measurement::claim_measurement(&generation_id, &meeting_id)?;
    measurement::record_frontend_stage(&generation_id, &stage, monotonic_start_ns, monotonic_end_ns)
}

/// Excludes a failed or cancelled request from D-12 performance statistics.
#[tauri::command]
pub async fn api_finish_summary_measurement(
    meeting_id: String,
    generation_id: String,
    outcome: String,
    reason: Option<String>,
) -> Result<(), String> {
    measurement::claim_measurement(&generation_id, &meeting_id)?;
    let outcome = match outcome.as_str() {
        "failed" => MeasurementOutcome::Failed,
        "cancelled" => MeasurementOutcome::Cancelled,
        "save_failed" => MeasurementOutcome::SaveFailed,
        _ => return Err("outcome must be failed, cancelled, or save_failed".to_string()),
    };
    measurement::finish_non_success(&generation_id, outcome, reason)
}

/// Closes the fixed endpoint after the final body has reached the WebView.
#[tauri::command]
pub async fn api_record_summary_page_completion(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    generation_id: String,
    page_displayed_body: String,
    page_display_complete_at: String,
    stage_start_monotonic_ns: u64,
    stage_end_monotonic_ns: u64,
) -> Result<SummaryMeasurement, String> {
    measurement::claim_measurement(&generation_id, &meeting_id)?;
    let process =
        SummaryProcessesRepository::get_summary_data(state.db_manager.pool(), &meeting_id)
            .await
            .map_err(|error| format!("read completed summary for measurement: {error}"))?
            .ok_or_else(|| "completed summary row not found".to_string())?;
    if process.status.to_lowercase() != "completed" {
        return Err("summary row is not completed".to_string());
    }
    let result = process
        .result
        .ok_or_else(|| "completed summary body is missing".to_string())
        .and_then(|raw| {
            serde_json::from_str::<serde_json::Value>(&raw)
                .map_err(|error| format!("completed summary JSON is invalid: {error}"))
        })?;
    if recorded_generation_id(&result) != Some(generation_id.as_str()) {
        return Err("completed summary generation id does not match measurement".to_string());
    }
    let database_body = result
        .get("markdown")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "completed summary markdown is missing".to_string())?;
    measurement::record_page_completion(
        &generation_id,
        database_body,
        &page_displayed_body,
        page_display_complete_at,
        stage_start_monotonic_ns,
        stage_end_monotonic_ns,
    )
}

fn recorded_summary_source_binding(
    summary: &serde_json::Value,
) -> Result<SummarySourceBinding, String> {
    let value = summary
        .get("template_snapshot")
        .and_then(|snapshot| snapshot.get("summarySourceBinding"))
        .or_else(|| summary.get("sourceBinding"))
        .ok_or_else(|| "stored summary has no source binding".to_owned())?;
    serde_json::from_value(value.clone())
        .map_err(|error| format!("stored summary source binding is invalid: {error}"))
}

fn recorded_generation_id(summary: &serde_json::Value) -> Option<&str> {
    summary
        .get("template_snapshot")?
        .get("generationId")?
        .as_str()
}

async fn summary_language_for_recorded_generation(
    pool: &sqlx::SqlitePool,
    meeting_id: &str,
    summary: &serde_json::Value,
) -> Result<Option<String>, String> {
    let Some(generation_id) = recorded_generation_id(summary) else {
        return Ok(None);
    };
    let row: Option<Option<String>> = sqlx::query_scalar(
        r#"
        SELECT summary_language
          FROM summary_generation_history
         WHERE meeting_id = ? AND generation_id = ?
        "#,
    )
    .bind(meeting_id)
    .bind(generation_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| format!("summary generation language could not be read: {error}"))?;
    Ok(row.flatten())
}

async fn calculate_summary_freshness<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::SqlitePool,
    template_state: &TemplateServiceState,
    meeting_id: &str,
    summary: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let generated_from = recorded_summary_source_binding(summary)?;
    let current_source = resolve_active_summary_input(pool, meeting_id).await?.source;
    let summary_language =
        summary_language_for_recorded_generation(pool, meeting_id, summary).await?;
    let (_, resolved) = resolve_template_for_generation(
        app,
        pool,
        template_state,
        meeting_id,
        None,
        None,
        summary_language.as_deref(),
    )
    .await
    .map_err(|error| format!("current summary template could not be resolved: {error:?}"))?;
    let current = SummarySourceBinding::from_active_source(
        &current_source,
        SummaryTemplateBinding {
            template_id: resolved.template.id,
            template_version: resolved.template.version,
            template_file_sha256: resolved.file_sha256,
            template_semantic_sha256: resolved.semantic_sha256,
        },
    )
    .map_err(|error| format!("current summary source binding is invalid: {error}"))?;
    let freshness = evaluate_summary_freshness(&generated_from, &current)
        .map_err(|error| format!("summary freshness could not be evaluated: {error}"))?;
    serde_json::to_value(freshness)
        .map_err(|error| format!("summary freshness could not be serialized: {error}"))
}

async fn attach_summary_freshness<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::SqlitePool,
    template_state: &TemplateServiceState,
    meeting_id: &str,
    mut summary: serde_json::Value,
) -> serde_json::Value {
    let freshness =
        match calculate_summary_freshness(app, pool, template_state, meeting_id, &summary).await {
            Ok(freshness) => freshness,
            Err(error) => {
                log_warn!(
                    "Summary freshness could not be resolved for {}: {}",
                    meeting_id,
                    error
                );
                serde_json::json!({
                    "status": "unavailable",
                    "reasons": ["source_binding_unavailable"]
                })
            }
        };
    if let Some(object) = summary.as_object_mut() {
        object.insert("summaryFreshness".to_owned(), freshness);
    }
    summary
}

/// Gets summary status and data (Native SQLx implementation)
///
/// Returns summary status (pending/processing/completed/failed) and parsed result data
#[tauri::command]
pub async fn api_get_summary<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    template_state: tauri::State<'_, TemplateServiceState>,
    meeting_id: String,
    _auth_token: Option<String>,
) -> Result<SummaryResponse, String> {
    log_info!(
        "api_get_summary (native) called for meeting_id: {}",
        meeting_id
    );
    let pool = state.db_manager.pool();

    match SummaryProcessesRepository::get_summary_data_for_meeting(pool, &meeting_id).await {
        Ok(Some(process)) => {
            let status = process.status.to_lowercase();
            // Never expose provider payloads, absolute paths, tokens, or backend
            // exception strings to the WebView. The raw detail remains in native
            // logs and the UI receives only a stable diagnostic category.
            let error = process
                .error
                .as_deref()
                .map(|raw| summary_error_category(&status, raw).to_owned());

            // Parse result data if it exists (regardless of status)
            // This allows displaying restored summaries after cancellation or failure
            let data = if let Some(result_str) = process.result {
                match serde_json::from_str::<serde_json::Value>(&result_str) {
                    Ok(parsed) => {
                        let (mut refreshed, mut validation) =
                            match revalidate_summary_against_current_evidence(
                                pool,
                                &meeting_id,
                                &parsed,
                            )
                            .await
                            {
                                Ok(refreshed) => refreshed,
                                Err(error) => {
                                    log_warn!(
                                        "Summary fact validation could not be refreshed for {}: {}",
                                        meeting_id,
                                        error
                                    );
                                    summary_with_unavailable_validation(&parsed)
                                }
                            };
                        validation.retain_saved_omission_warnings(&parsed);
                        refreshed = with_fact_validation(refreshed, &validation);
                        Some(
                            attach_summary_freshness(
                                &app,
                                pool,
                                &template_state,
                                &meeting_id,
                                refreshed,
                            )
                            .await,
                        )
                    }
                    Err(e) => {
                        log_error!("Failed to parse summary result JSON: {}", e);
                        None
                    }
                }
            } else {
                None
            };

            // Fetch meeting title from database
            let meeting_name = match MeetingsRepository::get_meeting(pool, &meeting_id).await {
                Ok(Some(meeting_details)) => {
                    log_info!("Fetched meeting title: {}", &meeting_details.title);
                    Some(meeting_details.title)
                }
                Ok(None) => {
                    log_warn!("Meeting not found for meeting_id: {}", meeting_id);
                    None
                }
                Err(e) => {
                    log_error!("Failed to fetch meeting title: {}", e);
                    None
                }
            };

            let response = SummaryResponse {
                status: status.clone(),
                meeting_name,
                meeting_id: meeting_id.clone(),
                start: process.start_time.map(|t| t.to_rfc3339()),
                end: process.end_time.map(|t| t.to_rfc3339()),
                data,
                error,
            };

            log_info!(
                "Summary status for {}: {}, has_data: {}, meeting_name: {:?}",
                meeting_id,
                status,
                response.data.is_some(),
                response.meeting_name
            );
            Ok(response)
        }
        Ok(None) => {
            log_info!("No summary process found for meeting_id: {}", meeting_id);

            // Still fetch meeting title for idle state
            let meeting_name = match MeetingsRepository::get_meeting(pool, &meeting_id).await {
                Ok(Some(meeting_details)) => Some(meeting_details.title),
                _ => None,
            };

            Ok(SummaryResponse {
                status: "idle".to_string(),
                meeting_name,
                meeting_id,
                start: None,
                end: None,
                data: None,
                error: None,
            })
        }
        Err(e) => {
            log_error!("Error retrieving summary for {}: {}", meeting_id, e);
            Err(format!("Failed to retrieve summary: {}", e))
        }
    }
}

/// Processes transcript and generates summary (Native SQLx implementation)
///
/// Spawns a background task and returns immediately with process_id
#[tauri::command]
pub async fn api_process_transcript<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    template_state: tauri::State<'_, TemplateServiceState>,
    text: String,
    model: String,
    model_name: String,
    meeting_id: Option<String>,
    _chunk_size: Option<i32>,
    _overlap: Option<i32>,
    custom_prompt: Option<String>,
    template_id: Option<String>,
    historical_generation_id: Option<String>,
    summary_language: Option<String>,
    measurement_generation_id: Option<String>,
    _auth_token: Option<String>,
) -> Result<ProcessTranscriptResponse, TemplateApiError> {
    use uuid::Uuid;

    let storage_operation_guard = begin_storage_operation(StorageOperationKind::SummaryGeneration)
        .map_err(|error| {
            TemplateApiError::from_code("STORAGE_OPERATION_BUSY").with_param(
                "operations",
                serde_json::json!(error
                    .blockers
                    .iter()
                    .map(|operation| operation.as_str())
                    .collect::<Vec<_>>()),
            )
        })?;

    let m_id = meeting_id.unwrap_or_else(|| format!("meeting-{}", Uuid::new_v4()));
    let generation_id = measurement_generation_id
        .clone()
        .unwrap_or_else(|| format!("gen_{}", Uuid::new_v4().simple()));
    if measurement_generation_id.is_some() {
        measurement::claim_measurement(&generation_id, &m_id).map_err(|error| {
            generation_preflight_error("summary measurement could not be claimed", error)
        })?;
    }
    let mut preflight_terminal = measurement::TerminalOnDrop::new(generation_id.clone());
    let prepare_input_stage = measurement::stage_guard_for(&generation_id, "prepare_input");
    log_info!(
        "api_process_transcript (native) called for meeting_id: {}, model: {}",
        &m_id,
        &model
    );

    let pool = state.db_manager.pool().clone();
    // P5 treats the WebView payload as untrusted compatibility input. Rebuild
    // the prompt only from the native active transcript version so a stale tab
    // cannot summarize a candidate or an older transcript.
    drop(text);
    let summary_input = resolve_active_summary_input(&pool, &m_id)
        .await
        .map_err(summary_source_preflight_error)?;
    let text = summary_input.transcript_text;
    measurement::record_input_transcript(&generation_id, &text);
    let active_source = summary_input.source;
    let final_prompt = custom_prompt.unwrap_or_else(|| "".to_string());

    // Normalise empty / whitespace-only to None so "" and null behave identically
    let summary_language = summary_language.and_then(|s| {
        let t = s.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    });

    // The backend is authoritative: it resolves the persisted meeting preference and
    // only accepts the legacy templateId when it agrees with that persisted state.
    let (meeting_folder, resolved) = resolve_template_for_generation(
        &app,
        &pool,
        &template_state,
        &m_id,
        template_id.as_deref(),
        historical_generation_id.as_deref(),
        summary_language.as_deref(),
    )
    .await?;
    let final_template_id = resolved.template.id.clone();
    let runtime_template = resolved.runtime_template.clone();
    let template_fingerprint = resolved.semantic_sha256.clone();
    let summary_meeting_context = resolve_summary_context_for_generation(
        &meeting_folder,
        &m_id,
        historical_generation_id.as_deref(),
    )
    .map_err(|error| generation_preflight_error("meeting facts could not be loaded", error))?;
    let meeting_context_id = summary_meeting_context
        .as_ref()
        .map(|context| context.context_id.clone());
    let meeting_context_sha256 = summary_meeting_context
        .as_ref()
        .map(|context| context.context_sha256.clone());
    let summary_context_sha256 = summary_meeting_context
        .as_ref()
        .map(|context| context.sha256());
    let source_binding = SummarySourceBinding::from_active_source(
        &active_source,
        SummaryTemplateBinding {
            template_id: resolved.template.id.clone(),
            template_version: resolved.template.version,
            template_file_sha256: resolved.file_sha256.clone(),
            template_semantic_sha256: resolved.semantic_sha256.clone(),
        },
    )
    .map_err(summary_source_preflight_error)?;
    let generation_context = SnapshotGenerationContext {
        summary_language: summary_language.clone(),
        custom_prompt_sha256: (!final_prompt.trim().is_empty()).then(|| sha256_text(&final_prompt)),
        model: SnapshotModelContext {
            provider: model.clone(),
            name: model_name.clone(),
            configuration_fingerprint: None,
        },
        summary_meeting_context: summary_meeting_context.clone(),
        summary_context_sha256: summary_context_sha256.clone(),
        summary_source_binding: Some(source_binding.clone()),
    };
    let (_, snapshot_path, snapshot_path_relative) = capture_snapshot(
        &meeting_folder,
        &generation_id,
        &m_id,
        &resolved,
        generation_context,
    )
    .map_err(|error| {
        generation_preflight_error("template snapshot could not be captured", error)
    })?;
    let resolved_template = ResolvedTemplateSummary {
        id: resolved.template.id.clone(),
        version: resolved.template.version,
        file_sha256: resolved.file_sha256.clone(),
        semantic_sha256: resolved.semantic_sha256.clone(),
        resolution_source: resolved.resolution_source,
    };
    let snapshot_link = GenerationSnapshotLink {
        generation_id: generation_id.clone(),
        snapshot_path_relative: snapshot_path_relative.clone(),
        resolved_template: resolved_template.clone(),
        meeting_context_id: meeting_context_id.clone(),
        meeting_context_sha256: meeting_context_sha256.clone(),
        summary_context_sha256: summary_context_sha256.clone(),
        summary_source_binding: Some(source_binding.clone()),
    };

    // Save transcript chunks data (matching Python backend behavior)
    let chunk_size = _chunk_size.unwrap_or(40000);
    let overlap = _overlap.unwrap_or(1000);

    TranscriptChunksRepository::save_transcript_data(
        &pool,
        &m_id,
        &text,
        &model,
        &model_name,
        chunk_size,
        overlap,
    )
    .await
    .map_err(|error| {
        remove_snapshot_after_preflight_failure(&snapshot_path);
        generation_preflight_error("transcript data could not be saved", error)
    })?;

    log_info!("✓ Transcript chunks saved for meeting_id: {}", &m_id);

    // Only expose the generation as PENDING after both its immutable snapshot and
    // transcript input have been persisted. Failure rolls back the new snapshot.
    let process_metadata = snapshot_link_json(&snapshot_link);
    let resolution_source = match resolved.resolution_source {
        crate::summary::template_snapshot::SnapshotResolutionSource::MeetingOverride => {
            "meeting_override"
        }
        crate::summary::template_snapshot::SnapshotResolutionSource::GlobalDefault => {
            "global_default"
        }
        crate::summary::template_snapshot::SnapshotResolutionSource::BuiltinFallback => {
            "builtin_fallback"
        }
        crate::summary::template_snapshot::SnapshotResolutionSource::HistoricalSnapshot => {
            "historical_snapshot"
        }
    };
    let generation_history = NewSummaryGenerationHistory {
        generation_id: &generation_id,
        template_id: &resolved_template.id,
        template_version: resolved_template.version,
        file_sha256: &resolved_template.file_sha256,
        semantic_sha256: &resolved_template.semantic_sha256,
        snapshot_path_relative: &snapshot_path_relative,
        resolution_source,
        model_provider: &model,
        model_name: &model_name,
        summary_language: summary_language.as_deref(),
        source_binding_schema_version: source_binding.schema_version,
        transcript_source: source_binding.transcript_source.as_str(),
        transcript_version_id: &source_binding.transcript_version_id,
        transcript_version: source_binding.transcript_version,
        moss_run_id: source_binding.moss_run_id.as_deref(),
        transcript_activated_at: source_binding.transcript_activated_at,
        transcript_sha256: &source_binding.transcript_sha256,
        speaker_binding_snapshot_id: &source_binding.speaker_binding_snapshot_id,
        speaker_binding_version: source_binding.speaker_binding_version,
        speaker_binding_sha256: &source_binding.speaker_binding_sha256,
    };
    SummaryProcessesRepository::create_or_reset_process_with_generation_history(
        &pool,
        &m_id,
        &process_metadata,
        &generation_history,
    )
    .await
    .map_err(|error| {
        remove_snapshot_after_preflight_failure(&snapshot_path);
        generation_preflight_error("summary process could not be initialized", error)
    })?;

    log_info!("✓ Summary process initialized for meeting_id: {}", &m_id);
    prepare_input_stage.finish();

    // Register synchronously, in authoritative request order, before spawning.
    // A newer generation cancels the previous token and stale tasks cannot later
    // replace the registry entry merely because their executor started late.
    let cancellation_token = SummaryService::register_summary_generation(&m_id, &generation_id);

    // Spawn background task for actual processing
    let meeting_id_clone = m_id.clone();
    let scoped_generation_id = generation_id.clone();
    tauri::async_runtime::spawn(async move {
        let _storage_operation_guard = storage_operation_guard;
        measurement::scope_generation(scoped_generation_id, async move {
            SummaryService::process_transcript_background(
                app,
                pool,
                meeting_id_clone.clone(),
                text,
                model,
                model_name,
                final_prompt,
                final_template_id,
                runtime_template,
                template_fingerprint,
                snapshot_link,
                cancellation_token,
                summary_language,
                summary_meeting_context,
                active_source,
            )
            .await;
        })
        .await;
    });
    preflight_terminal.disarm();

    log_info!("🚀 Background task spawned for meeting_id: {}", &m_id);

    Ok(ProcessTranscriptResponse {
        message: "Summary generation started".to_string(),
        process_id: m_id.clone(),
        legacy_process_id: m_id,
        generation_id,
        resolved_template,
        snapshot_path_relative,
        meeting_context_id,
        meeting_context_sha256,
        summary_context_sha256,
        source_binding,
    })
}

fn summary_source_preflight_error(source: impl std::fmt::Display) -> TemplateApiError {
    let error = TemplateApiError::from_code("SUMMARY_SOURCE_BINDING_FAILED");
    log_error!(
        "summary source binding failed (debug_id={}, source={})",
        error.debug_id,
        source
    );
    error
}

fn generation_preflight_error(detail: &str, source: impl std::fmt::Display) -> TemplateApiError {
    let error = TemplateApiError::from_code("MEETING_TEMPLATE_SNAPSHOT_FAILED");
    log_error!(
        "{} (debug_id={}, source={})",
        detail,
        error.debug_id,
        source
    );
    error
}

/// Cancels an ongoing summary generation process
///
/// This command triggers the cancellation token for the specified meeting,
/// stopping the summary generation gracefully.
#[tauri::command]
pub async fn api_cancel_summary<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<serde_json::Value, String> {
    log_info!("api_cancel_summary called for meeting_id: {}", meeting_id);

    // Trigger cancellation via the service
    let cancelled_generation = SummaryService::cancel_summary(&meeting_id);

    if let Some(generation_id) = cancelled_generation {
        // Update database status to cancelled
        let pool = state.db_manager.pool();
        let cancellation_recorded =
            match SummaryProcessesRepository::update_process_cancelled_for_generation(
                pool,
                &meeting_id,
                &generation_id,
            )
            .await
            {
                Ok(recorded) => recorded,
                Err(e) => {
                    log_error!(
                        "Failed to update DB status to cancelled for {} generation {}: {}",
                        meeting_id,
                        generation_id,
                        e
                    );
                    return Err(format!("Failed to update cancellation status: {}", e));
                }
            };

        if cancellation_recorded {
            log_info!(
                "Successfully cancelled summary generation for meeting_id: {}",
                meeting_id
            );
        } else {
            log_warn!(
                "Cancellation arrived after generation {} was terminal or superseded for meeting {}",
                generation_id,
                meeting_id
            );
        }
        Ok(serde_json::json!({
            "message": if cancellation_recorded { "Summary generation cancelled successfully" } else { "Summary generation was already terminal or superseded" },
            "meeting_id": meeting_id,
            "generation_id": generation_id,
            "cancelled": cancellation_recorded,
        }))
    } else {
        log_warn!(
            "No active summary generation found for meeting_id: {}",
            meeting_id
        );
        Ok(serde_json::json!({
            "message": "No active summary generation to cancel",
            "meeting_id": meeting_id,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::manager::DatabaseManager;
    use crate::meeting_context::{RecognitionDictionary, VerifiedMeetingFacts};
    use crate::summary::source_binding::TranscriptEvidenceSegment;
    use crate::summary::templates::TemplateRepository;
    use tauri::Manager;

    fn source(text: &str) -> TranscriptVersionSnapshot {
        TranscriptVersionSnapshot::legacy_whisper(
            "meeting_1",
            vec![TranscriptEvidenceSegment {
                segment_id: "segment_1".to_owned(),
                start_ms: None,
                end_ms: None,
                wall_clock: None,
                anonymous_speaker: None,
                bound_person_id: None,
                bound_display_name: None,
                text: text.to_owned(),
            }],
        )
    }

    fn minimal_context() -> SummaryMeetingContext {
        SummaryMeetingContext {
            context_id: "ctx_manual_test".to_owned(),
            context_sha256: "a".repeat(64),
            verified_meeting_facts: VerifiedMeetingFacts {
                meeting_name: Some("牌照站周会".to_owned()),
                started_at: None,
                completed_at: None,
                duration_seconds: None,
                fixed_meeting_mechanism: None,
                attending: Vec::new(),
                absent: Vec::new(),
                host: None,
            },
            recognition_dictionary: RecognitionDictionary {
                people: Vec::new(),
                terms: Vec::new(),
            },
        }
    }

    #[test]
    fn manual_summary_payload_ignores_webview_validation_and_requires_current_evidence() {
        let payload = serde_json::json!({
            "markdown": "PWA 已完成。",
            "summary_json": [],
            "restoredRevisionId": "revision_old",
            "factValidation": {
                "status": "passed",
                "warningCount": 0,
                "warnings": []
            }
        });

        let (sanitized_payload, validation) =
            revalidate_summary_payload(&payload, None, &source("Google 包已经完成。")).unwrap();

        assert!(sanitized_payload.get("factValidation").is_none());
        assert!(sanitized_payload.get("summary_json").is_none());
        assert!(sanitized_payload.get("restoredRevisionId").is_none());
        assert_eq!(sanitized_payload["markdown"], "PWA 已完成。");
        assert_eq!(validation.status, SummaryFactValidationStatus::NeedsReview);
        assert!(validation
            .warnings
            .iter()
            .any(|warning| warning.code == "missing_meeting_context"));
    }

    #[test]
    fn manual_summary_without_markdown_is_never_reported_as_fact_checked() {
        let payload = serde_json::json!({
            "summary_json": [],
            "factValidation": {"status": "passed"}
        });

        let (sanitized_payload, validation) =
            revalidate_summary_payload(&payload, None, &source("现有逐字稿")).unwrap();

        assert!(sanitized_payload.get("factValidation").is_none());
        assert_eq!(validation.status, SummaryFactValidationStatus::NeedsReview);
        assert_eq!(validation.warning_count, 1);
        assert_eq!(validation.warnings[0].code, "summary_markdown_unavailable");
    }

    #[test]
    fn manual_summary_keeps_authored_high_risk_fields_but_never_marks_them_passed() {
        let markdown = r#"| 任务 | 负责人 | 截止时间 |
| --- | --- | --- |
| 上线 PWA | Mico | 明天 |"#;
        let payload = serde_json::json!({
            "markdown": markdown,
            "summary_json": [{"type": "table", "content": "different unchecked content"}],
            "factValidation": {"status": "passed"}
        });

        let (canonical_payload, validation) = revalidate_summary_payload(
            &payload,
            Some(&minimal_context()),
            &source("讨论了上线 PWA 的任务。"),
        )
        .unwrap();

        assert_eq!(canonical_payload["markdown"], markdown);
        assert!(canonical_payload.get("summary_json").is_none());
        assert_eq!(validation.status, SummaryFactValidationStatus::NeedsReview);
        assert!(validation
            .warnings
            .iter()
            .any(|warning| warning.code == "manual_high_risk_fields_unverified"));
        assert!(!validation.aliases_normalized);
    }

    #[test]
    fn evidence_failure_fallback_preserves_markdown_as_the_only_rendered_body() {
        let payload = serde_json::json!({
            "markdown": "人工修正后的正文",
            "summary_json": [{"content": "旧正文"}],
            "factValidation": {"status": "passed"}
        });

        let (canonical_payload, validation) = summary_with_unavailable_validation(&payload);

        assert_eq!(canonical_payload["markdown"], "人工修正后的正文");
        assert!(canonical_payload.get("summary_json").is_none());
        assert!(canonical_payload.get("factValidation").is_none());
        assert_eq!(validation.status, SummaryFactValidationStatus::NeedsReview);
        assert_eq!(
            validation.warnings[0].code,
            "summary_validation_unavailable"
        );
    }

    #[tokio::test]
    async fn tauri_save_history_and_restore_round_trip_recomputes_native_markers() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("summary-command.sqlite");
        let legacy_path = directory.path().join("missing-legacy.db");
        let meeting_folder = directory.path().join("meeting");
        std::fs::create_dir_all(&meeting_folder).unwrap();
        let manager = DatabaseManager::new(
            database_path.to_str().unwrap(),
            legacy_path.to_str().unwrap(),
        )
        .await
        .unwrap();
        let pool = manager.pool();
        let meeting_id = "meeting-command-round-trip";
        let now = "2026-08-30T00:00:00Z";
        sqlx::query(
            r#"
            INSERT INTO meetings (id, title, created_at, updated_at, folder_path)
            VALUES (?, 'P5 command round trip', ?, ?, ?)
            "#,
        )
        .bind(meeting_id)
        .bind(now)
        .bind(now)
        .bind(meeting_folder.to_string_lossy().as_ref())
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO transcripts (id, meeting_id, transcript, timestamp)
            VALUES ('transcript-command-round-trip', ?, '会议讨论了现有事项。', ?)
            "#,
        )
        .bind(meeting_id)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();

        let active_source = resolve_active_summary_input(pool, meeting_id)
            .await
            .unwrap()
            .source;
        let source_binding = SummarySourceBinding::from_active_source(
            &active_source,
            SummaryTemplateBinding {
                template_id: "standard_meeting".to_owned(),
                template_version: 1,
                template_file_sha256: "a".repeat(64),
                template_semantic_sha256: "b".repeat(64),
            },
        )
        .unwrap();
        let original = serde_json::json!({
            "markdown": "original generated summary",
            "template_snapshot": {
                "generationId": "gen-command-round-trip",
                "summarySourceBinding": source_binding
            }
        });
        sqlx::query(
            r#"
            INSERT INTO summary_processes (
                meeting_id, status, created_at, updated_at, result
            ) VALUES (?, 'completed', ?, ?, ?)
            "#,
        )
        .bind(meeting_id)
        .bind(now)
        .bind(now)
        .bind(original.to_string())
        .execute(pool)
        .await
        .unwrap();

        let template_repository =
            TemplateRepository::new(directory.path().join("templates"), None).unwrap();
        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_store::Builder::default().build())
            .manage(AppState {
                db_manager: manager.clone(),
            })
            .manage(TemplateServiceState::from_repository(template_repository))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();

        let first_markdown = "first saved manual summary";
        let first_save = api_save_meeting_summary(
            app.handle().clone(),
            app.state::<AppState>(),
            app.state::<TemplateServiceState>(),
            meeting_id.to_owned(),
            serde_json::json!({
                "markdown": first_markdown,
                "summaryFreshness": {"status": "current", "reasons": []},
                "factValidation": {"status": "passed", "warningCount": 0, "warnings": []}
            }),
            None,
        )
        .await
        .unwrap();
        assert_eq!(first_save["summary"]["markdown"], first_markdown);
        assert!(first_save["summary"].get("summaryFreshness").is_some());

        let persisted_after_save: String =
            sqlx::query_scalar("SELECT result FROM summary_processes WHERE meeting_id = ?")
                .bind(meeting_id)
                .fetch_one(pool)
                .await
                .unwrap();
        let persisted_after_save: serde_json::Value =
            serde_json::from_str(&persisted_after_save).unwrap();
        assert_eq!(persisted_after_save["markdown"], first_markdown);
        assert!(
            persisted_after_save.get("summaryFreshness").is_none(),
            "a WebView freshness claim must not be persisted"
        );

        api_save_meeting_summary(
            app.handle().clone(),
            app.state::<AppState>(),
            app.state::<TemplateServiceState>(),
            meeting_id.to_owned(),
            serde_json::json!({"markdown": "second saved manual summary"}),
            None,
        )
        .await
        .unwrap();
        let revisions = api_list_manual_summary_revisions(
            app.handle().clone(),
            app.state::<AppState>(),
            meeting_id.to_owned(),
        )
        .await
        .unwrap();
        assert_eq!(revisions.len(), 2);
        let first_revision = revisions
            .iter()
            .find(|revision| revision.markdown.as_deref() == Some(first_markdown))
            .unwrap();
        let first_revision_id = first_revision.revision_id.clone();

        let restored = api_restore_manual_summary_revision(
            app.handle().clone(),
            app.state::<AppState>(),
            app.state::<TemplateServiceState>(),
            meeting_id.to_owned(),
            first_revision_id.clone(),
        )
        .await
        .unwrap();
        assert_eq!(restored["markdown"], first_markdown);
        assert_eq!(restored["restoredRevisionId"], first_revision_id);
        assert!(restored.get("factValidation").is_some());
        assert!(restored.get("summaryFreshness").is_some());

        let revisions_after_restore = api_list_manual_summary_revisions(
            app.handle().clone(),
            app.state::<AppState>(),
            meeting_id.to_owned(),
        )
        .await
        .unwrap();
        assert_eq!(revisions_after_restore.len(), 2);
        assert!(revisions_after_restore
            .iter()
            .any(|revision| revision.revision_id == first_revision_id && revision.is_current));
    }
}
