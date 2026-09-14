use crate::database::repositories::{
    meeting::MeetingsRepository, summary::SummaryProcessesRepository,
};
use crate::state::AppState;
use crate::summary::template_snapshot::{
    inspect_snapshot_files, quarantine_snapshot, read_snapshot, restore_quarantined_snapshot,
    SnapshotFileInspection, SnapshotFileState,
};
use crate::summary::templates::TemplateApiError;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{Sqlite, Transaction};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Runtime};

const DEFAULT_RETAIN_LATEST: usize = 20;
const DEFAULT_RETAIN_DAYS: i64 = 90;
const DEFAULT_MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotRetentionPolicy {
    #[serde(default = "default_retain_latest")]
    pub retain_latest: usize,
    #[serde(default = "default_retain_days")]
    pub retain_days: i64,
    #[serde(default = "default_max_total_bytes")]
    pub max_total_bytes: u64,
}

impl Default for SnapshotRetentionPolicy {
    fn default() -> Self {
        Self {
            retain_latest: DEFAULT_RETAIN_LATEST,
            retain_days: DEFAULT_RETAIN_DAYS,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
        }
    }
}

impl SnapshotRetentionPolicy {
    fn validate(&self) -> Result<(), String> {
        if self.retain_latest > 10_000
            || !(1..=36500).contains(&self.retain_days)
            || self.max_total_bytes > 1024 * 1024 * 1024 * 1024
        {
            return Err("snapshot retention policy is outside supported bounds".to_owned());
        }
        Ok(())
    }
}

fn default_retain_latest() -> usize {
    DEFAULT_RETAIN_LATEST
}
fn default_retain_days() -> i64 {
    DEFAULT_RETAIN_DAYS
}
fn default_max_total_bytes() -> u64 {
    DEFAULT_MAX_TOTAL_BYTES
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingLifecycleRequest {
    pub meeting_id: String,
    #[serde(default)]
    pub policy: SnapshotRetentionPolicy,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteSnapshotCleanupRequest {
    pub meeting_id: String,
    #[serde(default)]
    pub policy: SnapshotRetentionPolicy,
    pub preview_token: String,
    pub expected_generation_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationSnapshotRequest {
    pub meeting_id: String,
    pub generation_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationHistoryItemDto {
    pub generation_id: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub template_id: String,
    pub template_version: i64,
    pub resolution_source: String,
    pub model_provider: String,
    pub model_name: String,
    pub summary_language: Option<String>,
    pub source_binding_schema_version: Option<i64>,
    pub transcript_source: Option<String>,
    pub transcript_version_id: Option<String>,
    pub transcript_version: Option<i64>,
    pub moss_run_id: Option<String>,
    pub transcript_activated_at: Option<DateTime<Utc>>,
    pub transcript_sha256: Option<String>,
    pub speaker_binding_snapshot_id: Option<String>,
    pub speaker_binding_version: Option<i64>,
    pub speaker_binding_sha256: Option<String>,
    pub error_category: Option<String>,
    pub snapshot_state: String,
    pub is_current_summary: bool,
    pub is_active_generation: bool,
    pub can_retry_with_snapshot: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationSnapshotDetailsDto {
    pub generation_id: String,
    pub captured_at: DateTime<Utc>,
    pub template: crate::summary::templates::TemplateV2,
    pub file_sha256: String,
    pub semantic_sha256: String,
    pub summary_language: Option<String>,
    pub model_provider: String,
    pub model_name: String,
    pub meeting_context_id: Option<String>,
    pub meeting_context_sha256: Option<String>,
    pub summary_context_sha256: Option<String>,
    pub summary_source_binding: Option<crate::summary::source_binding::SummarySourceBinding>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotInventoryItemDto {
    pub generation_id: String,
    pub byte_size: u64,
    pub captured_at: DateTime<Utc>,
    pub file_state: String,
    pub history_status: Option<String>,
    pub template_id: Option<String>,
    pub template_version: Option<u64>,
    pub protected_reasons: Vec<String>,
    pub cleanup_reasons: Vec<String>,
    pub cleanup_candidate: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotCleanupPreviewDto {
    pub meeting_id: String,
    pub policy: SnapshotRetentionPolicy,
    pub preview_token: String,
    pub total_file_count: usize,
    pub total_bytes: u64,
    pub candidate_file_count: usize,
    pub candidate_bytes: u64,
    pub protected_file_count: usize,
    pub corrupt_file_count: usize,
    pub orphan_file_count: usize,
    pub items: Vec<SnapshotInventoryItemDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotCleanupResultDto {
    pub cleanup_batch_id: String,
    pub quarantined_generation_ids: Vec<String>,
    pub quarantined_file_count: usize,
    pub quarantined_bytes: u64,
    pub skipped_generation_ids: Vec<String>,
    pub plan_changed: bool,
}

#[derive(Debug, Clone, Copy)]
enum CleanupFaultInjection {
    None,
    #[cfg(test)]
    DiskFullAfterMoves(usize),
    #[cfg(test)]
    FailBeforeCommit,
}

fn lifecycle_error(detail: &str, source: impl std::fmt::Display) -> TemplateApiError {
    let error = TemplateApiError::from_code("MEETING_TEMPLATE_SNAPSHOT_FAILED");
    tracing::warn!(
        debug_id = %error.debug_id,
        detail,
        source = %source,
        "summary generation lifecycle operation failed"
    );
    error
}

fn validate_meeting_id(value: &str) -> Result<(), String> {
    if value.trim().is_empty()
        || value.len() > 200
        || value
            .bytes()
            .any(|byte| matches!(byte, b'\r' | b'\n' | b'\0'))
    {
        Err("meeting id failed validation".to_owned())
    } else {
        Ok(())
    }
}

async fn meeting_folder(pool: &sqlx::SqlitePool, meeting_id: &str) -> Result<PathBuf, String> {
    validate_meeting_id(meeting_id)?;
    MeetingsRepository::get_meeting_metadata(pool, meeting_id)
        .await
        .map_err(|error| format!("meeting lookup failed: {error}"))?
        .and_then(|meeting| meeting.folder_path)
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "meeting folder is unavailable".to_owned())
}

fn generation_from_json(raw: Option<&str>) -> Option<String> {
    let value: Value = serde_json::from_str(raw?).ok()?;
    value
        .get("template_snapshot")
        .or_else(|| Some(&value))?
        .get("generationId")?
        .as_str()
        .map(str::to_owned)
}

async fn current_references(
    pool: &sqlx::SqlitePool,
    meeting_id: &str,
) -> Result<(Option<String>, Option<String>), sqlx::Error> {
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT result, metadata FROM summary_processes WHERE meeting_id = ?")
            .bind(meeting_id)
            .fetch_optional(pool)
            .await?;
    let Some((result, metadata)) = row else {
        return Ok((None, None));
    };
    Ok((
        generation_from_json(result.as_deref()),
        generation_from_json(metadata.as_deref()),
    ))
}

async fn backfill_legacy_snapshot_history(
    pool: &sqlx::SqlitePool,
    folder: &Path,
    meeting_id: &str,
) -> Result<(), String> {
    let files = inspect_snapshot_files(folder, meeting_id)?;
    let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT status, result, metadata FROM summary_processes WHERE meeting_id = ?",
    )
    .bind(meeting_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| format!("legacy process state could not be read: {error}"))?;
    let (process_status, current_summary, active_generation) = match row {
        Some((status, result, metadata)) => (
            status.to_ascii_lowercase(),
            generation_from_json(result.as_deref()),
            generation_from_json(metadata.as_deref()),
        ),
        None => ("legacy".to_owned(), None, None),
    };

    for file in files {
        let Some(snapshot) = file.snapshot else {
            continue;
        };
        let is_current = current_summary.as_deref() == Some(&file.generation_id)
            || active_generation.as_deref() == Some(&file.generation_id);
        let status = if is_current {
            match process_status.as_str() {
                "pending" | "processing" => "pending",
                "completed" => "completed",
                _ => "legacy",
            }
        } else {
            "legacy"
        };
        let completed_at = (status != "pending").then_some(snapshot.captured_at);
        let source = snapshot.generation_context.summary_source_binding.as_ref();
        let transcript_version = source
            .map(|value| {
                i64::try_from(value.transcript_version)
                    .map_err(|_| "snapshot transcript version exceeds SQLite integer range")
            })
            .transpose()?;
        let speaker_binding_version = source
            .map(|value| {
                i64::try_from(value.speaker_binding_version)
                    .map_err(|_| "snapshot speaker version exceeds SQLite integer range")
            })
            .transpose()?;
        sqlx::query(
            r#"
            INSERT INTO summary_generation_history (
                generation_id, meeting_id, status, created_at, updated_at,
                completed_at, template_id, template_version, file_sha256,
                semantic_sha256, snapshot_path_relative, resolution_source,
                model_provider, model_name, summary_language,
                source_binding_schema_version, transcript_source,
                transcript_version_id, transcript_version, moss_run_id,
                transcript_activated_at, transcript_sha256,
                speaker_binding_snapshot_id, speaker_binding_version,
                speaker_binding_sha256, snapshot_state
            ) VALUES (
                ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                ?, ?, ?, ?, ?, 'available'
            )
            ON CONFLICT(generation_id) DO UPDATE SET
                source_binding_schema_version = COALESCE(
                    summary_generation_history.source_binding_schema_version,
                    excluded.source_binding_schema_version
                ),
                transcript_source = COALESCE(
                    summary_generation_history.transcript_source,
                    excluded.transcript_source
                ),
                transcript_version_id = COALESCE(
                    summary_generation_history.transcript_version_id,
                    excluded.transcript_version_id
                ),
                transcript_version = COALESCE(
                    summary_generation_history.transcript_version,
                    excluded.transcript_version
                ),
                moss_run_id = COALESCE(
                    summary_generation_history.moss_run_id,
                    excluded.moss_run_id
                ),
                transcript_activated_at = COALESCE(
                    summary_generation_history.transcript_activated_at,
                    excluded.transcript_activated_at
                ),
                transcript_sha256 = COALESCE(
                    summary_generation_history.transcript_sha256,
                    excluded.transcript_sha256
                ),
                speaker_binding_snapshot_id = COALESCE(
                    summary_generation_history.speaker_binding_snapshot_id,
                    excluded.speaker_binding_snapshot_id
                ),
                speaker_binding_version = COALESCE(
                    summary_generation_history.speaker_binding_version,
                    excluded.speaker_binding_version
                ),
                speaker_binding_sha256 = COALESCE(
                    summary_generation_history.speaker_binding_sha256,
                    excluded.speaker_binding_sha256
                )
            "#,
        )
        .bind(&snapshot.generation_id)
        .bind(meeting_id)
        .bind(status)
        .bind(snapshot.captured_at)
        .bind(snapshot.captured_at)
        .bind(completed_at)
        .bind(&snapshot.template_ref.id)
        .bind(i64::try_from(snapshot.template_ref.version).map_err(|_| {
            "legacy snapshot template version exceeds SQLite integer range".to_owned()
        })?)
        .bind(&snapshot.hashes.file_sha256)
        .bind(&snapshot.hashes.semantic_sha256)
        .bind(format!(
            "summary-template-snapshots/{}.json",
            snapshot.generation_id
        ))
        .bind(match snapshot.resolution_source {
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
        })
        .bind(&snapshot.generation_context.model.provider)
        .bind(&snapshot.generation_context.model.name)
        .bind(snapshot.generation_context.summary_language.as_deref())
        .bind(source.map(|value| i64::from(value.schema_version)))
        .bind(source.map(|value| value.transcript_source.as_str()))
        .bind(source.map(|value| value.transcript_version_id.as_str()))
        .bind(transcript_version)
        .bind(source.and_then(|value| value.moss_run_id.as_deref()))
        .bind(source.and_then(|value| value.transcript_activated_at.as_ref()))
        .bind(source.map(|value| value.transcript_sha256.as_str()))
        .bind(source.map(|value| value.speaker_binding_snapshot_id.as_str()))
        .bind(speaker_binding_version)
        .bind(source.map(|value| value.speaker_binding_sha256.as_str()))
        .execute(pool)
        .await
        .map_err(|error| format!("legacy snapshot history could not be imported: {error}"))?;
    }
    Ok(())
}

async fn current_references_in_transaction(
    transaction: &mut Transaction<'_, Sqlite>,
    meeting_id: &str,
) -> Result<(Option<String>, Option<String>), sqlx::Error> {
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT result, metadata FROM summary_processes WHERE meeting_id = ?")
            .bind(meeting_id)
            .fetch_optional(&mut **transaction)
            .await?;
    let Some((result, metadata)) = row else {
        return Ok((None, None));
    };
    Ok((
        generation_from_json(result.as_deref()),
        generation_from_json(metadata.as_deref()),
    ))
}

fn actual_snapshot_state(file: Option<&SnapshotFileInspection>) -> &'static str {
    match file.map(|item| item.state) {
        Some(SnapshotFileState::Available) => "available",
        Some(SnapshotFileState::Corrupt) => "corrupt",
        None => "missing",
    }
}

#[tauri::command]
pub async fn api_list_summary_generation_history<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<GenerationHistoryItemDto>, TemplateApiError> {
    let pool = state.db_manager.pool();
    let folder = meeting_folder(pool, &meeting_id)
        .await
        .map_err(|error| lifecycle_error("meeting folder could not be resolved", error))?;
    backfill_legacy_snapshot_history(pool, &folder, &meeting_id)
        .await
        .map_err(|error| lifecycle_error("legacy snapshot history could not be upgraded", error))?;
    let histories = SummaryProcessesRepository::list_generation_history(pool, &meeting_id, 200)
        .await
        .map_err(|error| lifecycle_error("generation history could not be listed", error))?;
    let files = inspect_snapshot_files(&folder, &meeting_id)
        .map_err(|error| lifecycle_error("snapshot files could not be inspected", error))?;
    let files: HashMap<_, _> = files
        .iter()
        .map(|file| (file.generation_id.as_str(), file))
        .collect();
    let (current_summary, active_generation) = current_references(pool, &meeting_id)
        .await
        .map_err(|error| lifecycle_error("snapshot references could not be resolved", error))?;

    Ok(histories
        .into_iter()
        .map(|history| {
            let file = files.get(history.generation_id.as_str()).copied();
            let snapshot_state = if history.snapshot_state == "quarantined" {
                "quarantined".to_owned()
            } else {
                actual_snapshot_state(file).to_owned()
            };
            let is_current_summary = current_summary.as_deref() == Some(&history.generation_id);
            let is_active_generation = history.status == "pending"
                && active_generation.as_deref() == Some(&history.generation_id);
            GenerationHistoryItemDto {
                can_retry_with_snapshot: snapshot_state == "available"
                    && history.status != "pending",
                generation_id: history.generation_id,
                status: history.status,
                created_at: history.created_at,
                updated_at: history.updated_at,
                completed_at: history.completed_at,
                template_id: history.template_id,
                template_version: history.template_version,
                resolution_source: history.resolution_source,
                model_provider: history.model_provider,
                model_name: history.model_name,
                summary_language: history.summary_language,
                source_binding_schema_version: history.source_binding_schema_version,
                transcript_source: history.transcript_source,
                transcript_version_id: history.transcript_version_id,
                transcript_version: history.transcript_version,
                moss_run_id: history.moss_run_id,
                transcript_activated_at: history.transcript_activated_at,
                transcript_sha256: history.transcript_sha256,
                speaker_binding_snapshot_id: history.speaker_binding_snapshot_id,
                speaker_binding_version: history.speaker_binding_version,
                speaker_binding_sha256: history.speaker_binding_sha256,
                error_category: history.error_category,
                snapshot_state,
                is_current_summary,
                is_active_generation,
            }
        })
        .collect())
}

#[tauri::command]
pub async fn api_get_summary_generation_snapshot<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    request: GenerationSnapshotRequest,
) -> Result<GenerationSnapshotDetailsDto, TemplateApiError> {
    let pool = state.db_manager.pool();
    let folder = meeting_folder(pool, &request.meeting_id)
        .await
        .map_err(|error| lifecycle_error("meeting folder could not be resolved", error))?;
    let snapshot = read_snapshot(&folder, &request.meeting_id, &request.generation_id)
        .map_err(|error| lifecycle_error("historical snapshot could not be opened", error))?;
    let meeting_context_id = snapshot
        .generation_context
        .summary_meeting_context
        .as_ref()
        .map(|context| context.context_id.clone());
    let meeting_context_sha256 = snapshot
        .generation_context
        .summary_meeting_context
        .as_ref()
        .map(|context| context.context_sha256.clone());
    Ok(GenerationSnapshotDetailsDto {
        generation_id: snapshot.generation_id,
        captured_at: snapshot.captured_at,
        template: snapshot.template,
        file_sha256: snapshot.hashes.file_sha256,
        semantic_sha256: snapshot.hashes.semantic_sha256,
        summary_language: snapshot.generation_context.summary_language,
        model_provider: snapshot.generation_context.model.provider,
        model_name: snapshot.generation_context.model.name,
        meeting_context_id,
        meeting_context_sha256,
        summary_context_sha256: snapshot.generation_context.summary_context_sha256,
        summary_source_binding: snapshot.generation_context.summary_source_binding,
    })
}

async fn build_cleanup_preview(
    pool: &sqlx::SqlitePool,
    folder: &Path,
    meeting_id: &str,
    policy: SnapshotRetentionPolicy,
) -> Result<SnapshotCleanupPreviewDto, String> {
    policy.validate()?;
    backfill_legacy_snapshot_history(pool, folder, meeting_id).await?;
    let files = inspect_snapshot_files(folder, meeting_id)?;
    let histories = SummaryProcessesRepository::list_generation_history(pool, meeting_id, 200)
        .await
        .map_err(|error| format!("generation history lookup failed: {error}"))?;
    let history_by_id: HashMap<_, _> = histories
        .iter()
        .map(|history| (history.generation_id.as_str(), history))
        .collect();
    let (current_summary, active_generation) = current_references(pool, meeting_id)
        .await
        .map_err(|error| format!("snapshot reference lookup failed: {error}"))?;
    let cutoff = Utc::now() - Duration::days(policy.retain_days);
    let total_bytes = files.iter().map(|file| file.byte_size).sum::<u64>();
    let mut projected_bytes = total_bytes;
    let mut items = Vec::with_capacity(files.len());

    for (rank, file) in files.iter().enumerate() {
        let history = history_by_id.get(file.generation_id.as_str()).copied();
        let mut protected_reasons = Vec::new();
        let mut cleanup_reasons = Vec::new();
        if current_summary.as_deref() == Some(&file.generation_id) {
            protected_reasons.push("current_summary".to_owned());
        }
        if active_generation.as_deref() == Some(&file.generation_id)
            || history.is_some_and(|item| item.status == "pending")
        {
            protected_reasons.push("active_generation".to_owned());
        }
        if file.state == SnapshotFileState::Available && rank < policy.retain_latest {
            protected_reasons.push("latest_count".to_owned());
        }
        if file.state == SnapshotFileState::Available && file.modified_at >= cutoff {
            protected_reasons.push("retention_age".to_owned());
        }

        if file.state == SnapshotFileState::Corrupt {
            cleanup_reasons.push("corrupt".to_owned());
        }
        if history.is_none() {
            cleanup_reasons.push("orphan".to_owned());
        }
        if rank >= policy.retain_latest {
            cleanup_reasons.push("exceeds_count".to_owned());
        }
        if file.modified_at < cutoff {
            cleanup_reasons.push("exceeds_age".to_owned());
        }
        if projected_bytes > policy.max_total_bytes {
            cleanup_reasons.push("exceeds_capacity".to_owned());
        }

        let cleanup_candidate = protected_reasons.is_empty() && !cleanup_reasons.is_empty();
        if cleanup_candidate {
            projected_bytes = projected_bytes.saturating_sub(file.byte_size);
        }
        items.push(SnapshotInventoryItemDto {
            generation_id: file.generation_id.clone(),
            byte_size: file.byte_size,
            captured_at: file
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.captured_at)
                .unwrap_or(file.modified_at),
            file_state: match file.state {
                SnapshotFileState::Available => "available",
                SnapshotFileState::Corrupt => "corrupt",
            }
            .to_owned(),
            history_status: history.map(|item| item.status.clone()),
            template_id: file
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.template_ref.id.clone()),
            template_version: file
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.template_ref.version),
            protected_reasons,
            cleanup_reasons,
            cleanup_candidate,
        });
    }

    let candidate_file_count = items.iter().filter(|item| item.cleanup_candidate).count();
    let candidate_bytes = items
        .iter()
        .filter(|item| item.cleanup_candidate)
        .map(|item| item.byte_size)
        .sum();
    let protected_file_count = items
        .iter()
        .filter(|item| !item.protected_reasons.is_empty())
        .count();
    let corrupt_file_count = items
        .iter()
        .filter(|item| item.file_state == "corrupt")
        .count();
    let orphan_file_count = items
        .iter()
        .filter(|item| item.history_status.is_none())
        .count();
    let preview_token = preview_token(meeting_id, &policy, &items);

    Ok(SnapshotCleanupPreviewDto {
        meeting_id: meeting_id.to_owned(),
        policy,
        preview_token,
        total_file_count: items.len(),
        total_bytes,
        candidate_file_count,
        candidate_bytes,
        protected_file_count,
        corrupt_file_count,
        orphan_file_count,
        items,
    })
}

fn preview_token(
    meeting_id: &str,
    policy: &SnapshotRetentionPolicy,
    items: &[SnapshotInventoryItemDto],
) -> String {
    let mut digest = Sha256::new();
    digest.update(meeting_id.as_bytes());
    digest.update(policy.retain_latest.to_le_bytes());
    digest.update(policy.retain_days.to_le_bytes());
    digest.update(policy.max_total_bytes.to_le_bytes());
    for item in items {
        digest.update(item.generation_id.as_bytes());
        digest.update(item.byte_size.to_le_bytes());
        digest.update([u8::from(item.cleanup_candidate)]);
        for reason in &item.protected_reasons {
            digest.update(reason.as_bytes());
        }
    }
    format!("{:x}", digest.finalize())
}

#[tauri::command]
pub async fn api_preview_template_snapshot_cleanup<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    request: MeetingLifecycleRequest,
) -> Result<SnapshotCleanupPreviewDto, TemplateApiError> {
    let pool = state.db_manager.pool();
    let folder = meeting_folder(pool, &request.meeting_id)
        .await
        .map_err(|error| lifecycle_error("meeting folder could not be resolved", error))?;
    build_cleanup_preview(pool, &folder, &request.meeting_id, request.policy)
        .await
        .map_err(|error| lifecycle_error("snapshot cleanup preview failed", error))
}

#[tauri::command]
pub async fn api_execute_template_snapshot_cleanup<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    request: ExecuteSnapshotCleanupRequest,
) -> Result<SnapshotCleanupResultDto, TemplateApiError> {
    let pool = state.db_manager.pool();
    let folder = meeting_folder(pool, &request.meeting_id)
        .await
        .map_err(|error| lifecycle_error("meeting folder could not be resolved", error))?;

    execute_snapshot_cleanup(pool, &folder, request, CleanupFaultInjection::None).await
}

async fn execute_snapshot_cleanup(
    pool: &sqlx::SqlitePool,
    folder: &Path,
    request: ExecuteSnapshotCleanupRequest,
    _fault: CleanupFaultInjection,
) -> Result<SnapshotCleanupResultDto, TemplateApiError> {
    let current_plan =
        build_cleanup_preview(pool, folder, &request.meeting_id, request.policy.clone())
            .await
            .map_err(|error| {
                lifecycle_error("snapshot cleanup plan could not be revalidated", error)
            })?;
    let expected: BTreeSet<_> = request.expected_generation_ids.iter().cloned().collect();
    if expected.len() != request.expected_generation_ids.len() {
        return Err(lifecycle_error(
            "snapshot cleanup request was rejected",
            "duplicate generation ids",
        ));
    }
    let current_candidates: BTreeMap<_, _> = current_plan
        .items
        .iter()
        .filter(|item| item.cleanup_candidate)
        .map(|item| (item.generation_id.clone(), item.byte_size))
        .collect();
    let plan_changed = request.preview_token != current_plan.preview_token
        || expected != current_candidates.keys().cloned().collect::<BTreeSet<_>>();
    let current_candidate_ids: BTreeSet<_> = current_candidates.keys().cloned().collect();
    let mut selected: Vec<_> = expected
        .intersection(&current_candidate_ids)
        .cloned()
        .collect();
    selected.sort();
    let selected_set: BTreeSet<_> = selected.iter().cloned().collect();
    let mut skipped: Vec<_> = expected.difference(&selected_set).cloned().collect();

    let cleanup_batch_id = format!("cleanup_{}", uuid::Uuid::new_v4().simple());
    if selected.is_empty() {
        skipped.sort();
        return Ok(SnapshotCleanupResultDto {
            cleanup_batch_id,
            quarantined_generation_ids: Vec::new(),
            quarantined_file_count: 0,
            quarantined_bytes: 0,
            skipped_generation_ids: skipped,
            plan_changed,
        });
    }

    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| lifecycle_error("snapshot cleanup transaction could not start", error))?;
    let candidate_json = serde_json::to_string(&selected).map_err(|error| {
        lifecycle_error("snapshot cleanup audit could not be serialized", error)
    })?;
    let planned_bytes: u64 = selected
        .iter()
        .filter_map(|generation_id| current_candidates.get(generation_id))
        .sum();
    // This first write acquires SQLite's writer reservation before filesystem
    // mutation, preventing a concurrent generation from changing references.
    sqlx::query(
        r#"
        INSERT INTO summary_snapshot_cleanup_audit (
            cleanup_batch_id, meeting_id, created_at, status, file_count,
            byte_count, plan_changed, candidate_generation_ids
        ) VALUES (?, ?, ?, 'preparing', ?, ?, ?, ?)
        "#,
    )
    .bind(&cleanup_batch_id)
    .bind(&request.meeting_id)
    .bind(Utc::now())
    .bind(i64::try_from(selected.len()).unwrap_or(i64::MAX))
    .bind(i64::try_from(planned_bytes).unwrap_or(i64::MAX))
    .bind(i64::from(plan_changed))
    .bind(candidate_json)
    .execute(&mut *transaction)
    .await
    .map_err(|error| lifecycle_error("snapshot cleanup audit could not be initialized", error))?;

    let (current_summary, active_generation) =
        current_references_in_transaction(&mut transaction, &request.meeting_id)
            .await
            .map_err(|error| {
                lifecycle_error("snapshot references could not be revalidated", error)
            })?;
    selected.retain(|generation_id| {
        let protected = current_summary.as_deref() == Some(generation_id)
            || active_generation.as_deref() == Some(generation_id);
        if protected {
            skipped.push(generation_id.clone());
        }
        !protected
    });

    let mut moved = Vec::new();
    for generation_id in &selected {
        match quarantine_snapshot(folder, generation_id, &cleanup_batch_id) {
            Ok((source, quarantined)) => {
                moved.push((generation_id.clone(), source, quarantined));
                #[cfg(test)]
                if matches!(_fault, CleanupFaultInjection::DiskFullAfterMoves(limit) if moved.len() >= limit)
                {
                    for (_, source, quarantined) in moved.iter().rev() {
                        let _ = restore_quarantined_snapshot(source, quarantined);
                    }
                    let _ = transaction.rollback().await;
                    return Err(lifecycle_error(
                        "snapshot cleanup move failed",
                        "injected disk-full failure after quarantine move",
                    ));
                }
            }
            Err(error) if error.contains("could not be read") || error.contains("not found") => {
                skipped.push(generation_id.clone());
            }
            Err(error) => {
                for (_, source, quarantined) in moved.iter().rev() {
                    let _ = restore_quarantined_snapshot(source, quarantined);
                }
                let _ = transaction.rollback().await;
                return Err(lifecycle_error("snapshot cleanup move failed", error));
            }
        }
    }

    let now = Utc::now();
    for (generation_id, _, _) in &moved {
        let update_result = sqlx::query(
            r#"
            UPDATE summary_generation_history
            SET snapshot_state = 'quarantined', cleanup_batch_id = ?, updated_at = ?
            WHERE generation_id = ? AND meeting_id = ? AND status != 'pending'
            "#,
        )
        .bind(&cleanup_batch_id)
        .bind(now)
        .bind(generation_id)
        .bind(&request.meeting_id)
        .execute(&mut *transaction)
        .await;
        if let Err(error) = update_result {
            for (_, source, quarantined) in moved.iter().rev() {
                let _ = restore_quarantined_snapshot(source, quarantined);
            }
            let _ = transaction.rollback().await;
            return Err(lifecycle_error(
                "snapshot cleanup history update failed",
                error,
            ));
        }
    }
    let audit_completion = sqlx::query(
        "UPDATE summary_snapshot_cleanup_audit SET status = 'completed', file_count = ?, byte_count = ? WHERE cleanup_batch_id = ?",
    )
    .bind(i64::try_from(moved.len()).unwrap_or(i64::MAX))
    .bind(
        i64::try_from(
            moved
                .iter()
                .filter_map(|(generation_id, _, _)| current_candidates.get(generation_id))
                .sum::<u64>(),
        )
        .unwrap_or(i64::MAX),
    )
    .bind(&cleanup_batch_id)
    .execute(&mut *transaction)
    .await;
    if let Err(error) = audit_completion {
        for (_, source, quarantined) in moved.iter().rev() {
            let _ = restore_quarantined_snapshot(source, quarantined);
        }
        let _ = transaction.rollback().await;
        return Err(lifecycle_error(
            "snapshot cleanup audit could not be completed",
            error,
        ));
    }

    #[cfg(test)]
    if matches!(_fault, CleanupFaultInjection::FailBeforeCommit) {
        for (_, source, quarantined) in moved.iter().rev() {
            let _ = restore_quarantined_snapshot(source, quarantined);
        }
        let _ = transaction.rollback().await;
        return Err(lifecycle_error(
            "snapshot cleanup transaction could not commit",
            "injected pre-commit failure",
        ));
    }

    if let Err(error) = transaction.commit().await {
        for (_, source, quarantined) in moved.iter().rev() {
            let _ = restore_quarantined_snapshot(source, quarantined);
        }
        return Err(lifecycle_error(
            "snapshot cleanup transaction could not commit",
            error,
        ));
    }

    let quarantined_generation_ids: Vec<_> = moved
        .iter()
        .map(|(generation_id, _, _)| generation_id.clone())
        .collect();
    let quarantined_bytes = quarantined_generation_ids
        .iter()
        .filter_map(|generation_id| current_candidates.get(generation_id))
        .sum();
    skipped.sort();
    skipped.dedup();
    Ok(SnapshotCleanupResultDto {
        cleanup_batch_id,
        quarantined_file_count: quarantined_generation_ids.len(),
        quarantined_generation_ids,
        quarantined_bytes,
        skipped_generation_ids: skipped,
        plan_changed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use tempfile::tempdir;

    #[test]
    fn retention_defaults_are_conservative_and_bounded() {
        let policy = SnapshotRetentionPolicy::default();
        assert_eq!(policy.retain_latest, 20);
        assert_eq!(policy.retain_days, 90);
        assert_eq!(policy.max_total_bytes, 512 * 1024 * 1024);
        assert!(policy.validate().is_ok());
    }

    #[test]
    fn generation_reference_parser_accepts_result_and_metadata_shapes() {
        let link = r#"{"generationId":"gen_123"}"#;
        let result = r#"{"markdown":"ok","template_snapshot":{"generationId":"gen_456"}}"#;
        assert_eq!(generation_from_json(Some(link)).as_deref(), Some("gen_123"));
        assert_eq!(
            generation_from_json(Some(result)).as_deref(),
            Some("gen_456")
        );
        assert!(generation_from_json(Some("not-json")).is_none());
    }

    #[test]
    fn preview_token_changes_with_candidate_state() {
        let policy = SnapshotRetentionPolicy::default();
        let mut item = SnapshotInventoryItemDto {
            generation_id: "gen_1".to_owned(),
            byte_size: 100,
            captured_at: Utc::now(),
            file_state: "available".to_owned(),
            history_status: Some("completed".to_owned()),
            template_id: Some("standard_meeting".to_owned()),
            template_version: Some(1),
            protected_reasons: Vec::new(),
            cleanup_reasons: vec!["exceeds_count".to_owned()],
            cleanup_candidate: true,
        };
        let before = preview_token("meeting-1", &policy, &[item.clone()]);
        item.cleanup_candidate = false;
        item.protected_reasons.push("current_summary".to_owned());
        let after = preview_token("meeting-1", &policy, &[item]);
        assert_ne!(before, after);
    }

    async fn lifecycle_pool() -> sqlx::SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE summary_processes (
                meeting_id TEXT PRIMARY KEY,
                status TEXT NOT NULL,
                result TEXT,
                metadata TEXT
            );
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE summary_generation_history (
                generation_id TEXT PRIMARY KEY,
                meeting_id TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                completed_at TEXT,
                template_id TEXT NOT NULL,
                template_version INTEGER NOT NULL,
                file_sha256 TEXT NOT NULL,
                semantic_sha256 TEXT NOT NULL,
                snapshot_path_relative TEXT NOT NULL,
                resolution_source TEXT NOT NULL,
                model_provider TEXT NOT NULL,
                model_name TEXT NOT NULL,
                summary_language TEXT,
                source_binding_schema_version INTEGER,
                transcript_source TEXT,
                transcript_version_id TEXT,
                transcript_version INTEGER,
                moss_run_id TEXT,
                transcript_activated_at TEXT,
                transcript_sha256 TEXT,
                speaker_binding_snapshot_id TEXT,
                speaker_binding_version INTEGER,
                speaker_binding_sha256 TEXT,
                error_category TEXT,
                snapshot_state TEXT NOT NULL,
                cleanup_batch_id TEXT
            );
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE summary_snapshot_cleanup_audit (
                cleanup_batch_id TEXT PRIMARY KEY,
                meeting_id TEXT NOT NULL,
                created_at TEXT NOT NULL,
                status TEXT NOT NULL,
                file_count INTEGER NOT NULL,
                byte_count INTEGER NOT NULL,
                plan_changed INTEGER NOT NULL DEFAULT 0,
                candidate_generation_ids TEXT NOT NULL
            );
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn strict_cleanup_policy() -> SnapshotRetentionPolicy {
        SnapshotRetentionPolicy {
            retain_latest: 0,
            retain_days: 1,
            max_total_bytes: 1,
        }
    }

    fn cleanup_request(preview: &SnapshotCleanupPreviewDto) -> ExecuteSnapshotCleanupRequest {
        ExecuteSnapshotCleanupRequest {
            meeting_id: preview.meeting_id.clone(),
            policy: preview.policy.clone(),
            preview_token: preview.preview_token.clone(),
            expected_generation_ids: preview
                .items
                .iter()
                .filter(|item| item.cleanup_candidate)
                .map(|item| item.generation_id.clone())
                .collect(),
        }
    }

    async fn insert_history(pool: &sqlx::SqlitePool, generation_id: &str) {
        sqlx::query(
            r#"
            INSERT INTO summary_generation_history (
                generation_id, meeting_id, status, created_at, updated_at,
                completed_at, template_id, template_version, file_sha256,
                semantic_sha256, snapshot_path_relative, resolution_source,
                model_provider, model_name, snapshot_state
            ) VALUES (?, 'meeting-1', 'completed', '2026-08-24T00:00:00Z',
                '2026-08-24T00:01:00Z', '2026-08-24T00:01:00Z', 'standard_meeting',
                1, ?, ?, ?, 'meeting_override', 'test', 'test', 'available')
            "#,
        )
        .bind(generation_id)
        .bind("a".repeat(64))
        .bind("b".repeat(64))
        .bind(format!("summary-template-snapshots/{generation_id}.json"))
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn cleanup_preview_never_selects_current_corrupt_snapshot() {
        let pool = lifecycle_pool().await;
        let folder = tempdir().unwrap();
        let snapshot_dir = folder
            .path()
            .join(crate::summary::template_snapshot::SNAPSHOT_DIRECTORY);
        std::fs::create_dir(&snapshot_dir).unwrap();
        std::fs::write(snapshot_dir.join("gen_current.json"), "corrupt-current").unwrap();
        std::fs::write(snapshot_dir.join("gen_old.json"), "corrupt-old").unwrap();
        insert_history(&pool, "gen_current").await;
        insert_history(&pool, "gen_old").await;
        sqlx::query(
            "INSERT INTO summary_processes (meeting_id, status, result, metadata) VALUES ('meeting-1', 'completed', ?, ?)",
        )
        .bind(r#"{"template_snapshot":{"generationId":"gen_current"}}"#)
        .bind(r#"{"generationId":"gen_current"}"#)
        .execute(&pool)
        .await
        .unwrap();

        let preview = build_cleanup_preview(
            &pool,
            folder.path(),
            "meeting-1",
            SnapshotRetentionPolicy {
                retain_latest: 0,
                retain_days: 1,
                max_total_bytes: 1,
            },
        )
        .await
        .unwrap();
        let current = preview
            .items
            .iter()
            .find(|item| item.generation_id == "gen_current")
            .unwrap();
        let old = preview
            .items
            .iter()
            .find(|item| item.generation_id == "gen_old")
            .unwrap();
        assert!(!current.cleanup_candidate);
        assert!(current
            .protected_reasons
            .contains(&"current_summary".to_owned()));
        assert!(old.cleanup_candidate);
        assert!(old.cleanup_reasons.contains(&"corrupt".to_owned()));
    }

    #[test]
    fn quarantine_move_is_recoverable() {
        let folder = tempdir().unwrap();
        let snapshot_dir = folder
            .path()
            .join(crate::summary::template_snapshot::SNAPSHOT_DIRECTORY);
        std::fs::create_dir(&snapshot_dir).unwrap();
        let original = snapshot_dir.join("gen_old.json");
        std::fs::write(&original, "corrupt-old").unwrap();
        let (source, quarantined) =
            quarantine_snapshot(folder.path(), "gen_old", "cleanup_test").unwrap();
        assert_eq!(
            source,
            std::fs::canonicalize(&snapshot_dir)
                .unwrap()
                .join("gen_old.json")
        );
        assert!(!source.exists());
        assert!(quarantined.is_file());
        restore_quarantined_snapshot(&source, &quarantined).unwrap();
        assert!(source.is_file());
        assert!(!quarantined.exists());
    }

    #[tokio::test]
    async fn cleanup_reference_race_only_shrinks_the_previewed_set() {
        let pool = lifecycle_pool().await;
        let folder = tempdir().unwrap();
        let snapshot_dir = folder
            .path()
            .join(crate::summary::template_snapshot::SNAPSHOT_DIRECTORY);
        std::fs::create_dir(&snapshot_dir).unwrap();
        std::fs::write(snapshot_dir.join("gen_protected.json"), "corrupt").unwrap();
        std::fs::write(snapshot_dir.join("gen_removable.json"), "corrupt").unwrap();

        let preview =
            build_cleanup_preview(&pool, folder.path(), "meeting-1", strict_cleanup_policy())
                .await
                .unwrap();
        assert_eq!(preview.candidate_file_count, 2);

        sqlx::query(
            "INSERT INTO summary_processes (meeting_id, status, result, metadata) VALUES ('meeting-1', 'completed', ?, NULL)",
        )
        .bind(r#"{"template_snapshot":{"generationId":"gen_protected"}}"#)
        .execute(&pool)
        .await
        .unwrap();

        let result = execute_snapshot_cleanup(
            &pool,
            folder.path(),
            cleanup_request(&preview),
            CleanupFaultInjection::None,
        )
        .await
        .unwrap();
        assert!(result.plan_changed);
        assert_eq!(result.quarantined_generation_ids, vec!["gen_removable"]);
        assert_eq!(result.skipped_generation_ids, vec!["gen_protected"]);
        assert!(snapshot_dir.join("gen_protected.json").is_file());
        assert!(!snapshot_dir.join("gen_removable.json").exists());
    }

    #[tokio::test]
    async fn injected_disk_full_restores_files_and_rolls_back_audit() {
        let pool = lifecycle_pool().await;
        let folder = tempdir().unwrap();
        let snapshot_dir = folder
            .path()
            .join(crate::summary::template_snapshot::SNAPSHOT_DIRECTORY);
        std::fs::create_dir(&snapshot_dir).unwrap();
        std::fs::write(snapshot_dir.join("gen_disk_a.json"), "corrupt-a").unwrap();
        std::fs::write(snapshot_dir.join("gen_disk_b.json"), "corrupt-b").unwrap();
        let preview =
            build_cleanup_preview(&pool, folder.path(), "meeting-1", strict_cleanup_policy())
                .await
                .unwrap();

        let error = execute_snapshot_cleanup(
            &pool,
            folder.path(),
            cleanup_request(&preview),
            CleanupFaultInjection::DiskFullAfterMoves(1),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "MEETING_TEMPLATE_SNAPSHOT_FAILED");
        assert!(snapshot_dir.join("gen_disk_a.json").is_file());
        assert!(snapshot_dir.join("gen_disk_b.json").is_file());
        let audit_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM summary_snapshot_cleanup_audit")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(audit_count, 0);
    }

    #[tokio::test]
    async fn injected_precommit_failure_restores_file_and_history_state() {
        let pool = lifecycle_pool().await;
        let folder = tempdir().unwrap();
        let snapshot_dir = folder
            .path()
            .join(crate::summary::template_snapshot::SNAPSHOT_DIRECTORY);
        std::fs::create_dir(&snapshot_dir).unwrap();
        std::fs::write(snapshot_dir.join("gen_commit.json"), "corrupt").unwrap();
        insert_history(&pool, "gen_commit").await;
        let preview =
            build_cleanup_preview(&pool, folder.path(), "meeting-1", strict_cleanup_policy())
                .await
                .unwrap();

        execute_snapshot_cleanup(
            &pool,
            folder.path(),
            cleanup_request(&preview),
            CleanupFaultInjection::FailBeforeCommit,
        )
        .await
        .unwrap_err();
        assert!(snapshot_dir.join("gen_commit.json").is_file());
        let state: String = sqlx::query_scalar(
            "SELECT snapshot_state FROM summary_generation_history WHERE generation_id = 'gen_commit'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(state, "available");
        let audit_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM summary_snapshot_cleanup_audit")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(audit_count, 0);
    }

    #[tokio::test]
    async fn committed_quarantine_survives_database_restart_consistently() {
        let database_directory = tempdir().unwrap();
        let database_path = database_directory.path().join("lifecycle.sqlite");
        let options = SqliteConnectOptions::new()
            .filename(&database_path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options.clone())
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE summary_processes (meeting_id TEXT PRIMARY KEY, status TEXT NOT NULL, result TEXT, metadata TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"CREATE TABLE summary_generation_history (
                generation_id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL, status TEXT NOT NULL,
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL, completed_at TEXT,
                template_id TEXT NOT NULL, template_version INTEGER NOT NULL,
                file_sha256 TEXT NOT NULL, semantic_sha256 TEXT NOT NULL,
                snapshot_path_relative TEXT NOT NULL, resolution_source TEXT NOT NULL,
                model_provider TEXT NOT NULL, model_name TEXT NOT NULL, summary_language TEXT,
                source_binding_schema_version INTEGER, transcript_source TEXT,
                transcript_version_id TEXT, transcript_version INTEGER, moss_run_id TEXT,
                transcript_activated_at TEXT, transcript_sha256 TEXT,
                speaker_binding_snapshot_id TEXT, speaker_binding_version INTEGER,
                speaker_binding_sha256 TEXT,
                error_category TEXT, snapshot_state TEXT NOT NULL, cleanup_batch_id TEXT
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"CREATE TABLE summary_snapshot_cleanup_audit (
                cleanup_batch_id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL, created_at TEXT NOT NULL,
                status TEXT NOT NULL, file_count INTEGER NOT NULL, byte_count INTEGER NOT NULL,
                plan_changed INTEGER NOT NULL DEFAULT 0, candidate_generation_ids TEXT NOT NULL
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();

        let meeting_folder = tempdir().unwrap();
        let snapshot_dir = meeting_folder
            .path()
            .join(crate::summary::template_snapshot::SNAPSHOT_DIRECTORY);
        std::fs::create_dir(&snapshot_dir).unwrap();
        std::fs::write(snapshot_dir.join("gen_restart.json"), "corrupt").unwrap();
        insert_history(&pool, "gen_restart").await;
        let preview = build_cleanup_preview(
            &pool,
            meeting_folder.path(),
            "meeting-1",
            strict_cleanup_policy(),
        )
        .await
        .unwrap();
        let result = execute_snapshot_cleanup(
            &pool,
            meeting_folder.path(),
            cleanup_request(&preview),
            CleanupFaultInjection::None,
        )
        .await
        .unwrap();
        let batch_id = result.cleanup_batch_id.clone();
        drop(pool);

        let restarted = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        let state: String = sqlx::query_scalar(
            "SELECT snapshot_state FROM summary_generation_history WHERE generation_id = 'gen_restart'",
        )
        .fetch_one(&restarted)
        .await
        .unwrap();
        let audit_status: String = sqlx::query_scalar(
            "SELECT status FROM summary_snapshot_cleanup_audit WHERE cleanup_batch_id = ?",
        )
        .bind(&batch_id)
        .fetch_one(&restarted)
        .await
        .unwrap();
        assert_eq!(state, "quarantined");
        assert_eq!(audit_status, "completed");
        assert!(!snapshot_dir.join("gen_restart.json").exists());
        assert!(snapshot_dir
            .join(".quarantine")
            .join(batch_id)
            .join("gen_restart.json")
            .is_file());
    }
}
