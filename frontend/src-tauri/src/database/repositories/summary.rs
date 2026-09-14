use crate::database::models::{SummaryGenerationHistory, SummaryProcess};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::SqlitePool;
use tracing::{error, info as log_info};
use uuid::Uuid;

pub struct SummaryProcessesRepository;

pub const SUMMARY_INTERRUPTED_BY_RESTART: &str =
    "Summary generation was interrupted by application restart";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OrphanedSummaryRecovery {
    pub processes_failed: u64,
    pub histories_failed: u64,
}

#[derive(Debug, Clone)]
pub struct NewSummaryGenerationHistory<'a> {
    pub generation_id: &'a str,
    pub template_id: &'a str,
    pub template_version: u64,
    pub file_sha256: &'a str,
    pub semantic_sha256: &'a str,
    pub snapshot_path_relative: &'a str,
    pub resolution_source: &'a str,
    pub model_provider: &'a str,
    pub model_name: &'a str,
    pub summary_language: Option<&'a str>,
    pub source_binding_schema_version: u8,
    pub transcript_source: &'a str,
    pub transcript_version_id: &'a str,
    pub transcript_version: u64,
    pub moss_run_id: Option<&'a str>,
    pub transcript_activated_at: Option<DateTime<Utc>>,
    pub transcript_sha256: &'a str,
    pub speaker_binding_snapshot_id: &'a str,
    pub speaker_binding_version: u64,
    pub speaker_binding_sha256: &'a str,
}

impl SummaryProcessesRepository {
    /// Settles summary work that belonged to a previous application process.
    ///
    /// This runs synchronously after database migrations and before the app can
    /// accept a new generation request. At that point no summary worker from a
    /// previous process can still be authoritative. The transition is atomic,
    /// restores the last durable summary when regeneration had created a
    /// backup, and is idempotent on every later startup.
    pub async fn recover_orphaned_generations_from_previous_run(
        pool: &SqlitePool,
    ) -> Result<OrphanedSummaryRecovery, sqlx::Error> {
        let mut transaction = pool.begin().await?;
        let orphaned_processes: Vec<(String, Option<String>, Option<String>, Option<String>)> =
            sqlx::query_as(
                r#"
            SELECT meeting_id, metadata, result, result_backup
            FROM summary_processes
            WHERE lower(status) IN ('pending', 'processing')
            "#,
            )
            .fetch_all(&mut *transaction)
            .await?;

        let now = Utc::now();
        let mut processes_failed = 0_u64;
        for (meeting_id, metadata, current_result, result_backup) in orphaned_processes {
            let had_backup = result_backup.is_some();
            let restored_result = result_backup.or(current_result);
            let restored_metadata = if had_backup {
                restored_result
                    .as_deref()
                    .and_then(template_snapshot_metadata_from_result)
            } else {
                metadata
            };
            let update = sqlx::query(
                r#"
                UPDATE summary_processes
                SET status = 'failed',
                    error = ?,
                    updated_at = ?,
                    end_time = ?,
                    result = ?,
                    metadata = ?,
                    result_backup = NULL,
                    result_backup_timestamp = NULL
                WHERE meeting_id = ?
                  AND lower(status) IN ('pending', 'processing')
                "#,
            )
            .bind(SUMMARY_INTERRUPTED_BY_RESTART)
            .bind(now)
            .bind(now)
            .bind(restored_result)
            .bind(restored_metadata)
            .bind(meeting_id)
            .execute(&mut *transaction)
            .await?;
            processes_failed += update.rows_affected();
        }

        // Also settle an orphaned audit row whose process row is already gone
        // or terminal. Since this executes before new work is accepted, every
        // remaining pending history row belongs to the previous process.
        let histories_failed = sqlx::query(
            r#"
            UPDATE summary_generation_history
            SET status = 'failed',
                error_category = 'interrupted_by_restart',
                updated_at = ?,
                completed_at = ?
            WHERE status = 'pending'
            "#,
        )
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?
        .rows_affected();

        transaction.commit().await?;
        Ok(OrphanedSummaryRecovery {
            processes_failed,
            histories_failed,
        })
    }

    /// Retrieves the current summary process state for a given meeting ID.
    pub async fn get_summary_data(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<SummaryProcess>, sqlx::Error> {
        sqlx::query_as::<_, SummaryProcess>("SELECT * FROM summary_processes WHERE meeting_id = ?")
            .bind(meeting_id)
            .fetch_optional(pool)
            .await
    }

    pub async fn update_meeting_summary(
        pool: &SqlitePool,
        meeting_id: &str,
        summary: &Value,
        trusted_fact_validation: &Value,
    ) -> Result<bool, sqlx::Error> {
        let mut transaction = pool.begin().await?;

        let meeting_exists: bool = sqlx::query("SELECT 1 FROM meetings WHERE id = ?")
            .bind(meeting_id)
            .fetch_optional(&mut *transaction)
            .await?
            .is_some();

        if !meeting_exists {
            log_info!(
                "Attempted to save summary for a non-existent meeting_id: {}",
                meeting_id
            );
            transaction.rollback().await?;
            return Ok(false);
        }

        // Editing summary content must not detach it from the immutable template
        // snapshot that produced it. The existing link is authoritative: callers
        // cannot replace it with a forged path or generation id.
        let existing_result: Option<String> =
            sqlx::query_scalar("SELECT result FROM summary_processes WHERE meeting_id = ?")
                .bind(meeting_id)
                .fetch_optional(&mut *transaction)
                .await?
                .flatten();
        let existing_summary = existing_result
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
        let authoritative_snapshot = existing_summary
            .as_ref()
            .and_then(|value| value.get("template_snapshot").cloned());
        let mut summary_to_save = summary.clone();
        let source_generation_id = authoritative_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.get("generationId"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(snapshot) = authoritative_snapshot {
            let Some(object) = summary_to_save.as_object_mut() else {
                error!("Can't preserve template snapshot on a non-object summary value");
                transaction.rollback().await?;
                return Ok(false);
            };
            object.insert("template_snapshot".to_owned(), snapshot);
        } else if let Some(object) = summary_to_save.as_object_mut() {
            // Legacy summaries have no authoritative link; reject a caller-supplied
            // lookalike instead of blessing it as trusted history.
            object.remove("template_snapshot");
        }
        let Some(object) = summary_to_save.as_object_mut() else {
            error!("Can't attach trusted fact validation to a non-object summary value");
            transaction.rollback().await?;
            return Ok(false);
        };
        // A human edit replaces the report, not the model's original output.
        // Keep only the database copy, including its pipeline version; the
        // WebView cannot supply or replace generation provenance.
        if let Some(draft) = existing_summary
            .as_ref()
            .and_then(|value| value.get("generationDraft"))
        {
            object.insert("generationDraft".to_owned(), draft.clone());
        } else {
            object.remove("generationDraft");
        }
        // `summary` originates in the WebView and is therefore not trusted to
        // declare its own validation status. Only the native command may pass
        // the freshly computed validation through this separate parameter.
        object.remove("factValidation");
        object.insert("factValidation".to_owned(), trusted_fact_validation.clone());

        let result_json = match serde_json::to_string(&summary_to_save) {
            Ok(result_json) => result_json,
            Err(error) => {
                error!("Can't convert the json to string for saving to Database: {error}");
                transaction.rollback().await?;
                return Ok(false);
            }
        };
        let now = Utc::now();

        let revision_id = format!("revision_{}", Uuid::new_v4().simple());
        let markdown = summary_to_save.get("markdown").and_then(Value::as_str);
        sqlx::query(
            r#"
            INSERT INTO summary_manual_revisions (
                revision_id, meeting_id, created_at, source_generation_id,
                summary_json, markdown
            ) VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&revision_id)
        .bind(meeting_id)
        .bind(now)
        .bind(source_generation_id.as_deref())
        .bind(&result_json)
        .bind(markdown)
        .execute(&mut *transaction)
        .await?;

        sqlx::query("UPDATE summary_processes SET result = ?, updated_at = ? WHERE meeting_id = ?")
            .bind(&result_json)
            .bind(now)
            .bind(meeting_id)
            .execute(&mut *transaction)
            .await?;

        sqlx::query("UPDATE meetings SET updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(meeting_id)
            .execute(&mut *transaction)
            .await?;

        transaction.commit().await?;

        log_info!(
            "Successfully updated summary and timestamp for meeting_id: {}",
            meeting_id
        );
        Ok(true)
    }

    pub async fn get_summary_data_for_meeting(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<SummaryProcess>, sqlx::Error> {
        sqlx::query_as::<_, SummaryProcess>(
            "SELECT p.* FROM summary_processes p JOIN transcript_chunks t ON p.meeting_id = t.meeting_id WHERE p.meeting_id = ?",
        )
        .bind(meeting_id)
        .fetch_optional(pool)
        .await
    }

    pub async fn create_or_reset_process(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<(), sqlx::Error> {
        Self::create_or_reset_process_with_metadata(pool, meeting_id, None).await
    }

    pub async fn create_or_reset_process_with_metadata(
        pool: &SqlitePool,
        meeting_id: &str,
        metadata: Option<&Value>,
    ) -> Result<(), sqlx::Error> {
        log_info!(
            "Creating or resetting summary process for meeting_id: {}",
            meeting_id
        );
        let now = Utc::now();
        let metadata_json = metadata
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| {
                sqlx::Error::Protocol(format!("Failed to serialize process metadata: {error}"))
            })?;
        sqlx::query(
            r#"
            INSERT INTO summary_processes (meeting_id, status, created_at, updated_at, start_time, result, error, metadata)
            VALUES (?, 'PENDING', ?, ?, ?, NULL, NULL, ?)
            ON CONFLICT(meeting_id) DO UPDATE SET
                status = 'PENDING',
                updated_at = excluded.updated_at,
                start_time = excluded.start_time,
                result_backup = result,
                result_backup_timestamp = excluded.updated_at,
                result = result,
                error = NULL,
                metadata = excluded.metadata
            "#
        )
        .bind(meeting_id)
        .bind(now)
        .bind(now)
        .bind(now)
        .bind(metadata_json)
        .execute(pool)
        .await?;
        log_info!(
            "Backed up existing summary before regeneration for meeting_id: {}",
            meeting_id
        );
        Ok(())
    }

    /// Starts a generation and appends its durable audit record in the same
    /// SQLite transaction. Any older pending generation becomes superseded;
    /// its immutable snapshot remains available for diagnosis or retry.
    pub async fn create_or_reset_process_with_generation_history(
        pool: &SqlitePool,
        meeting_id: &str,
        metadata: &Value,
        generation: &NewSummaryGenerationHistory<'_>,
    ) -> Result<(), sqlx::Error> {
        let metadata_json = serde_json::to_string(metadata).map_err(|error| {
            sqlx::Error::Protocol(format!("Failed to serialize process metadata: {error}"))
        })?;
        let now = Utc::now();
        let mut transaction = pool.begin().await?;

        sqlx::query(
            r#"
            UPDATE summary_generation_history
            SET status = 'superseded', updated_at = ?, completed_at = ?
            WHERE meeting_id = ? AND status = 'pending'
            "#,
        )
        .bind(now)
        .bind(now)
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO summary_processes (meeting_id, status, created_at, updated_at, start_time, result, error, metadata)
            VALUES (?, 'PENDING', ?, ?, ?, NULL, NULL, ?)
            ON CONFLICT(meeting_id) DO UPDATE SET
                status = 'PENDING',
                updated_at = excluded.updated_at,
                start_time = excluded.start_time,
                end_time = NULL,
                result_backup = result,
                result_backup_timestamp = excluded.updated_at,
                result = result,
                error = NULL,
                metadata = excluded.metadata
            "#,
        )
        .bind(meeting_id)
        .bind(now)
        .bind(now)
        .bind(now)
        .bind(&metadata_json)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO summary_generation_history (
                generation_id, meeting_id, status, created_at, updated_at,
                template_id, template_version, file_sha256, semantic_sha256,
                snapshot_path_relative, resolution_source, model_provider,
                model_name, summary_language, source_binding_schema_version,
                transcript_source, transcript_version_id, transcript_version,
                moss_run_id, transcript_activated_at, transcript_sha256,
                speaker_binding_snapshot_id, speaker_binding_version,
                speaker_binding_sha256
            ) VALUES (
                ?, ?, 'pending', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                ?, ?, ?, ?, ?, ?, ?
            )
            "#,
        )
        .bind(generation.generation_id)
        .bind(meeting_id)
        .bind(now)
        .bind(now)
        .bind(generation.template_id)
        .bind(i64::try_from(generation.template_version).map_err(|_| {
            sqlx::Error::Protocol("Template version exceeds SQLite integer range".to_owned())
        })?)
        .bind(generation.file_sha256)
        .bind(generation.semantic_sha256)
        .bind(generation.snapshot_path_relative)
        .bind(generation.resolution_source)
        .bind(generation.model_provider)
        .bind(generation.model_name)
        .bind(generation.summary_language)
        .bind(i64::from(generation.source_binding_schema_version))
        .bind(generation.transcript_source)
        .bind(generation.transcript_version_id)
        .bind(i64::try_from(generation.transcript_version).map_err(|_| {
            sqlx::Error::Protocol("Transcript version exceeds SQLite integer range".to_owned())
        })?)
        .bind(generation.moss_run_id)
        .bind(generation.transcript_activated_at.as_ref())
        .bind(generation.transcript_sha256)
        .bind(generation.speaker_binding_snapshot_id)
        .bind(
            i64::try_from(generation.speaker_binding_version).map_err(|_| {
                sqlx::Error::Protocol(
                    "Speaker binding version exceeds SQLite integer range".to_owned(),
                )
            })?,
        )
        .bind(generation.speaker_binding_sha256)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(())
    }

    pub async fn list_generation_history(
        pool: &SqlitePool,
        meeting_id: &str,
        limit: i64,
    ) -> Result<Vec<SummaryGenerationHistory>, sqlx::Error> {
        let bounded_limit = limit.clamp(1, 10_000);
        sqlx::query_as::<_, SummaryGenerationHistory>(
            r#"
            SELECT generation_id, meeting_id, status, created_at, updated_at,
                   completed_at, template_id, template_version, file_sha256,
                   semantic_sha256, snapshot_path_relative, resolution_source,
                   model_provider, model_name, summary_language,
                   source_binding_schema_version, transcript_source,
                   transcript_version_id, transcript_version, moss_run_id,
                   transcript_activated_at, transcript_sha256,
                   speaker_binding_snapshot_id, speaker_binding_version,
                   speaker_binding_sha256, error_category, snapshot_state,
                   cleanup_batch_id
            FROM summary_generation_history
            WHERE meeting_id = ?
            ORDER BY created_at DESC
            LIMIT ?
            "#,
        )
        .bind(meeting_id)
        .bind(bounded_limit)
        .fetch_all(pool)
        .await
    }

    pub async fn update_process_completed(
        pool: &SqlitePool,
        meeting_id: &str,
        result: Value, // Keep this as Value to handle both old and new formats if needed
        chunk_count: i64,
        processing_time: f64,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now();
        let result_str = serde_json::to_string(&result)
            .map_err(|e| sqlx::Error::Protocol(format!("Failed to serialize result: {}", e)))?;

        sqlx::query(
            r#"
            UPDATE summary_processes
            SET status = 'completed', result = ?, updated_at = ?, end_time = ?, chunk_count = ?, processing_time = ?, error = NULL, result_backup = NULL, result_backup_timestamp = NULL
            WHERE meeting_id = ?
            "#
        )
        .bind(result_str)
        .bind(now)
        .bind(now)
        .bind(chunk_count)
        .bind(processing_time)
        .bind(meeting_id)
        .execute(pool)
        .await?;
        log_info!(
            "Summary completed and backup cleared for meeting_id: {}",
            meeting_id
        );
        Ok(())
    }

    /// Completes a generation only when it is still the authoritative pending
    /// generation for this meeting. This prevents an older background task from
    /// overwriting a newer regeneration that reused the same meeting row.
    pub async fn update_process_completed_for_generation(
        pool: &SqlitePool,
        meeting_id: &str,
        generation_id: &str,
        result: Value,
        chunk_count: i64,
        processing_time: f64,
    ) -> Result<bool, sqlx::Error> {
        let mut transaction = pool.begin().await?;
        let row: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT status, metadata FROM summary_processes WHERE meeting_id = ?")
                .bind(meeting_id)
                .fetch_optional(&mut *transaction)
                .await?;
        let Some((status, Some(metadata_raw))) = row else {
            transaction.rollback().await?;
            return Ok(false);
        };
        if status != "PENDING" || !metadata_generation_matches(&metadata_raw, generation_id) {
            transaction.rollback().await?;
            return Ok(false);
        }
        let now = Utc::now();
        let result_str = serde_json::to_string(&result).map_err(|error| {
            sqlx::Error::Protocol(format!("Failed to serialize result: {error}"))
        })?;
        let update = sqlx::query(
            r#"
            UPDATE summary_processes
            SET status = 'completed', result = ?, updated_at = ?, end_time = ?, chunk_count = ?, processing_time = ?, error = NULL, result_backup = NULL, result_backup_timestamp = NULL
            WHERE meeting_id = ? AND status = 'PENDING' AND metadata = ?
            "#,
        )
        .bind(result_str)
        .bind(now)
        .bind(now)
        .bind(chunk_count)
        .bind(processing_time)
        .bind(meeting_id)
        .bind(metadata_raw)
        .execute(&mut *transaction)
        .await?;
        if update.rows_affected() != 1 {
            transaction.rollback().await?;
            return Ok(false);
        }
        let history = sqlx::query(
            r#"
            UPDATE summary_generation_history
            SET status = 'completed', updated_at = ?, completed_at = ?, error_category = NULL
            WHERE generation_id = ? AND meeting_id = ? AND status = 'pending'
            "#,
        )
        .bind(now)
        .bind(now)
        .bind(generation_id)
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;
        if history.rows_affected() != 1 {
            transaction.rollback().await?;
            return Err(sqlx::Error::Protocol(
                "Generation history was missing during completion".to_owned(),
            ));
        }
        transaction.commit().await?;
        Ok(true)
    }

    pub async fn update_process_failed(
        pool: &SqlitePool,
        meeting_id: &str,
        error: &str,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now();

        // Restore from backup if it exists, otherwise keep current result
        sqlx::query(
            r#"
            UPDATE summary_processes
            SET
                status = 'failed',
                error = ?,
                updated_at = ?,
                end_time = ?,
                result = COALESCE(result_backup, result),
                result_backup = NULL,
                result_backup_timestamp = NULL
            WHERE meeting_id = ?
            "#,
        )
        .bind(error)
        .bind(now)
        .bind(now)
        .bind(meeting_id)
        .execute(pool)
        .await?;
        log_info!(
            "Summary generation failed and backup restored for meeting_id: {}",
            meeting_id
        );
        Ok(())
    }

    pub async fn update_process_cancelled(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now();

        // Restore from backup if it exists, otherwise keep current result
        sqlx::query(
            r#"
            UPDATE summary_processes
            SET
                status = 'cancelled',
                updated_at = ?,
                end_time = ?,
                error = 'Generation was cancelled by user',
                result = COALESCE(result_backup, result),
                result_backup = NULL,
                result_backup_timestamp = NULL
            WHERE meeting_id = ?
            "#,
        )
        .bind(now)
        .bind(now)
        .bind(meeting_id)
        .execute(pool)
        .await?;
        log_info!(
            "Marked summary process as cancelled and restored backup for meeting_id: {}",
            meeting_id
        );
        Ok(())
    }

    /// Marks a generation as failed only if it is still current. When a prior
    /// summary is restored, its template snapshot link is restored with it.
    pub async fn update_process_failed_for_generation(
        pool: &SqlitePool,
        meeting_id: &str,
        generation_id: &str,
        error: &str,
    ) -> Result<bool, sqlx::Error> {
        Self::terminate_generation(pool, meeting_id, generation_id, "failed", error).await
    }

    /// Marks a generation as cancelled only if it is still current. This is
    /// deliberately generation-scoped so a late cancellation cannot cancel a
    /// newer request for the same meeting.
    pub async fn update_process_cancelled_for_generation(
        pool: &SqlitePool,
        meeting_id: &str,
        generation_id: &str,
    ) -> Result<bool, sqlx::Error> {
        Self::terminate_generation(
            pool,
            meeting_id,
            generation_id,
            "cancelled",
            "Generation was cancelled by user",
        )
        .await
    }

    async fn terminate_generation(
        pool: &SqlitePool,
        meeting_id: &str,
        generation_id: &str,
        terminal_status: &str,
        terminal_error: &str,
    ) -> Result<bool, sqlx::Error> {
        let mut transaction = pool.begin().await?;
        let row: Option<(String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT status, metadata, result, result_backup FROM summary_processes WHERE meeting_id = ?",
        )
        .bind(meeting_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((status, metadata_raw, current_result, result_backup)) = row else {
            transaction.rollback().await?;
            return Ok(false);
        };
        let Some(metadata_raw) = metadata_raw else {
            transaction.rollback().await?;
            return Ok(false);
        };
        if status != "PENDING" || !metadata_generation_matches(&metadata_raw, generation_id) {
            transaction.rollback().await?;
            return Ok(false);
        }

        let had_backup = result_backup.is_some();
        let restored_result = result_backup.or(current_result);
        let restored_metadata = if had_backup {
            restored_result
                .as_deref()
                .and_then(template_snapshot_metadata_from_result)
        } else {
            Some(metadata_raw.clone())
        };
        let now = Utc::now();
        let update = sqlx::query(
            r#"
            UPDATE summary_processes
            SET status = ?, error = ?, updated_at = ?, end_time = ?, result = ?, metadata = ?, result_backup = NULL, result_backup_timestamp = NULL
            WHERE meeting_id = ? AND status = 'PENDING' AND metadata = ?
            "#,
        )
        .bind(terminal_status)
        .bind(terminal_error)
        .bind(now)
        .bind(now)
        .bind(restored_result)
        .bind(restored_metadata)
        .bind(meeting_id)
        .bind(metadata_raw)
        .execute(&mut *transaction)
        .await?;
        if update.rows_affected() != 1 {
            transaction.rollback().await?;
            return Ok(false);
        }
        let history = sqlx::query(
            r#"
            UPDATE summary_generation_history
            SET status = ?, error_category = ?, updated_at = ?, completed_at = ?
            WHERE generation_id = ? AND meeting_id = ? AND status = 'pending'
            "#,
        )
        .bind(terminal_status)
        .bind(summary_error_category(terminal_status, terminal_error))
        .bind(now)
        .bind(now)
        .bind(generation_id)
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;
        if history.rows_affected() != 1 {
            transaction.rollback().await?;
            return Err(sqlx::Error::Protocol(
                "Generation history was missing during terminal transition".to_owned(),
            ));
        }
        transaction.commit().await?;
        Ok(true)
    }
}

pub(crate) fn summary_error_category(status: &str, raw: &str) -> &'static str {
    if status == "cancelled" || raw.to_ascii_lowercase().contains("cancel") {
        return "cancelled_by_user";
    }
    let normalized = raw.to_ascii_lowercase();
    if normalized.contains("auth")
        || normalized.contains("api key")
        || normalized.contains("unauthorized")
    {
        "model_authentication"
    } else if normalized.contains("connect")
        || normalized.contains("network")
        || normalized.contains("timeout")
    {
        "model_connection"
    } else if normalized.contains("model")
        && (normalized.contains("missing")
            || normalized.contains("required")
            || normalized.contains("not found")
            || normalized.contains("unavailable"))
    {
        "model_unavailable"
    } else if normalized.contains("json") || normalized.contains("response") {
        "invalid_model_response"
    } else if normalized.contains("disk")
        || normalized.contains("database")
        || normalized.contains("write")
    {
        "local_storage"
    } else {
        "generation_failed"
    }
}

fn metadata_generation_matches(raw: &str, expected: &str) -> bool {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|value| {
            value
                .get("generationId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .is_some_and(|generation_id| generation_id == expected)
}

fn template_snapshot_metadata_from_result(raw: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let snapshot = value.get("template_snapshot")?;
    if !snapshot.is_object() {
        return None;
    }
    serde_json::to_string(snapshot).ok()
}

#[cfg(test)]
mod generation_tests {
    use super::*;
    use crate::database::manager::DatabaseManager;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    async fn pool() -> SqlitePool {
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
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                error TEXT,
                result TEXT,
                start_time TEXT,
                end_time TEXT,
                chunk_count INTEGER DEFAULT 0,
                processing_time REAL DEFAULT 0.0,
                metadata TEXT,
                result_backup TEXT,
                result_backup_timestamp TEXT
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE meetings (
                id TEXT PRIMARY KEY,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE summary_manual_revisions (
                revision_id TEXT PRIMARY KEY,
                meeting_id TEXT NOT NULL,
                created_at TEXT NOT NULL,
                source_generation_id TEXT,
                summary_json TEXT NOT NULL,
                markdown TEXT
            )
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
                snapshot_state TEXT NOT NULL DEFAULT 'available',
                cleanup_batch_id TEXT
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn file_pool(path: &std::path::Path) -> SqlitePool {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        for statement in [
            r#"CREATE TABLE IF NOT EXISTS summary_processes (
                meeting_id TEXT PRIMARY KEY, status TEXT NOT NULL,
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
                error TEXT, result TEXT, start_time TEXT, end_time TEXT,
                chunk_count INTEGER DEFAULT 0, processing_time REAL DEFAULT 0.0,
                metadata TEXT, result_backup TEXT, result_backup_timestamp TEXT
            )"#,
            r#"CREATE TABLE IF NOT EXISTS meetings (
                id TEXT PRIMARY KEY, updated_at TEXT NOT NULL
            )"#,
            r#"CREATE TABLE IF NOT EXISTS summary_manual_revisions (
                revision_id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL,
                created_at TEXT NOT NULL, source_generation_id TEXT,
                summary_json TEXT NOT NULL, markdown TEXT
            )"#,
            r#"CREATE TABLE IF NOT EXISTS summary_generation_history (
                generation_id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL,
                status TEXT NOT NULL, created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL, completed_at TEXT,
                template_id TEXT NOT NULL, template_version INTEGER NOT NULL,
                file_sha256 TEXT NOT NULL, semantic_sha256 TEXT NOT NULL,
                snapshot_path_relative TEXT NOT NULL, resolution_source TEXT NOT NULL,
                model_provider TEXT NOT NULL, model_name TEXT NOT NULL,
                summary_language TEXT, source_binding_schema_version INTEGER,
                transcript_source TEXT, transcript_version_id TEXT,
                transcript_version INTEGER, moss_run_id TEXT,
                transcript_activated_at TEXT, transcript_sha256 TEXT,
                speaker_binding_snapshot_id TEXT, speaker_binding_version INTEGER,
                speaker_binding_sha256 TEXT, error_category TEXT,
                snapshot_state TEXT NOT NULL DEFAULT 'available', cleanup_batch_id TEXT
            )"#,
        ] {
            sqlx::query(statement).execute(&pool).await.unwrap();
        }
        pool
    }

    fn link(generation_id: &str) -> Value {
        serde_json::json!({
            "generationId": generation_id,
            "snapshotPathRelative": format!("summary-template-snapshots/{generation_id}.json"),
            "resolvedTemplate": {
                "id": "daily_standup",
                "version": 1,
                "fileSha256": "a".repeat(64),
                "semanticSha256": "b".repeat(64),
                "resolutionSource": "meeting_override"
            }
        })
    }

    #[tokio::test]
    async fn manual_edit_preserves_snapshot_and_replaces_fact_validation_with_trusted_result() {
        let pool = pool().await;
        sqlx::query("INSERT INTO meetings (id, updated_at) VALUES ('m-edit', 'now')")
            .execute(&pool)
            .await
            .unwrap();
        let old_fact_validation = serde_json::json!({
            "status": "passed",
            "warningCount": 0,
            "warnings": []
        });
        let trusted_fact_validation = serde_json::json!({
            "status": "needs_review",
            "warningCount": 1,
            "warnings": [{
                "code": "unsupported_status_claim",
                "messageKey": "summary:factValidation.unsupportedStatusClaim"
            }]
        });
        let original = serde_json::json!({
            "markdown": "before edit",
            "template_snapshot": link("gen_edit"),
            "factValidation": old_fact_validation,
        });
        sqlx::query(
            "INSERT INTO summary_processes (meeting_id, status, created_at, updated_at, result) VALUES ('m-edit', 'completed', 'now', 'now', ?)",
        )
        .bind(original.to_string())
        .execute(&pool)
        .await
        .unwrap();

        let accepted = SummaryProcessesRepository::update_meeting_summary(
            &pool,
            "m-edit",
            &serde_json::json!({
                "markdown": "after edit",
                "template_snapshot": {"generationId": "forged"},
                "factValidation": {"status": "passed"},
            }),
            &trusted_fact_validation,
        )
        .await
        .unwrap();
        assert!(accepted);
        let stored: String =
            sqlx::query_scalar("SELECT result FROM summary_processes WHERE meeting_id = 'm-edit'")
                .fetch_one(&pool)
                .await
                .unwrap();
        let stored: Value = serde_json::from_str(&stored).unwrap();

        assert_eq!(stored["markdown"], "after edit");
        assert_eq!(stored["template_snapshot"], link("gen_edit"));
        assert_eq!(stored["factValidation"], trusted_fact_validation);
        assert_ne!(stored["factValidation"], old_fact_validation);
        let revision_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM summary_manual_revisions WHERE meeting_id = 'm-edit'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(revision_count, 1);
        let revision: String = sqlx::query_scalar(
            "SELECT summary_json FROM summary_manual_revisions WHERE meeting_id = 'm-edit'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let revision: Value = serde_json::from_str(&revision).unwrap();
        assert_eq!(revision["factValidation"], trusted_fact_validation);
    }

    #[tokio::test]
    async fn manual_edit_retains_only_the_stored_generation_draft() {
        let pool = pool().await;
        let original_draft = serde_json::json!({
            "markdown": "original model output, before cleanup",
            "pipelineVersion": 2026091306
        });
        for (meeting_id, has_stored_draft, supplied_draft) in [
            ("draft-omitted", true, None),
            ("draft-forged", true, Some(serde_json::json!({"markdown": "forged"}))),
            ("draft-legacy", false, Some(serde_json::json!({"markdown": "forged"}))),
        ] {
            sqlx::query("INSERT INTO meetings (id, updated_at) VALUES (?, 'now')")
                .bind(meeting_id).execute(&pool).await.unwrap();
            let mut original = serde_json::json!({
                "markdown": "cleaned saved report", "template_snapshot": link("gen_draft")
            });
            if has_stored_draft {
                original["generationDraft"] = original_draft.clone();
            }
            sqlx::query(
                "INSERT INTO summary_processes (meeting_id, status, created_at, updated_at, result) VALUES (?, 'completed', 'now', 'now', ?)",
            ).bind(meeting_id).bind(original.to_string()).execute(&pool).await.unwrap();
            // Saving a second edit must not turn the first human edit into the
            // model's original draft.
            for markdown in ["first human edit", "second human edit"] {
                let mut edit = serde_json::json!({"markdown": markdown});
                if let Some(draft) = &supplied_draft {
                    edit["generationDraft"] = draft.clone();
                }
                assert!(SummaryProcessesRepository::update_meeting_summary(
                    &pool, meeting_id, &edit, &serde_json::json!({"status": "needs_review"})
                ).await.unwrap());
                let raw: String = sqlx::query_scalar(
                    "SELECT result FROM summary_processes WHERE meeting_id = ?"
                ).bind(meeting_id).fetch_one(&pool).await.unwrap();
                let saved: Value = serde_json::from_str(&raw).unwrap();
                assert_eq!(saved["markdown"], markdown);
                assert_eq!(saved.get("generationDraft"),
                    has_stored_draft.then_some(&original_draft), "{meeting_id}");
            }
            let revisions: Vec<String> = sqlx::query_scalar(
                "SELECT summary_json FROM summary_manual_revisions WHERE meeting_id = ?"
            ).bind(meeting_id).fetch_all(&pool).await.unwrap();
            assert_eq!(revisions.len(), 2);
            for raw in revisions {
                let revision: Value = serde_json::from_str(&raw).unwrap();
                assert_eq!(revision.get("generationDraft"),
                    has_stored_draft.then_some(&original_draft), "{meeting_id}");
            }
        }
    }

    #[tokio::test]
    async fn mc_r05_manual_revision_survives_restart_context_reload_and_regeneration() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("mc-r05.sqlite");
        let pool = file_pool(&database_path).await;
        sqlx::query("INSERT INTO meetings (id, updated_at) VALUES ('m-r05', 'before-edit')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            r#"INSERT INTO summary_generation_history (
                generation_id, meeting_id, status, created_at, updated_at, completed_at,
                template_id, template_version, file_sha256, semantic_sha256,
                snapshot_path_relative, resolution_source, model_provider, model_name
            ) VALUES ('gen_r05_original', 'm-r05', 'completed', 't0', 't0', 't0',
                'license_station_weekly', 3, ?, ?,
                'summary-template-snapshots/gen_r05_original.json',
                'meeting_override', 'builtin-ai', 'qwen3.5:2b')"#,
        )
        .bind("a".repeat(64))
        .bind("b".repeat(64))
        .execute(&pool)
        .await
        .unwrap();
        let fact_validation = serde_json::json!({
            "status": "passed",
            "warningCount": 0,
            "warnings": [],
            "meetingContextId": "ctx_r05_recording",
            "meetingContextSha256": "c".repeat(64),
            "summaryContextSha256": "d".repeat(64)
        });
        let original = serde_json::json!({
            "markdown": "generated summary",
            "template_snapshot": link("gen_r05_original"),
            "factValidation": fact_validation,
        });
        sqlx::query(
            "INSERT INTO summary_processes (meeting_id, status, created_at, updated_at, result, metadata) VALUES ('m-r05', 'completed', 't0', 't0', ?, ?)",
        )
        .bind(original.to_string())
        .bind(link("gen_r05_original").to_string())
        .execute(&pool)
        .await
        .unwrap();

        let manual_marker = "【MC-R05 摘要人工编辑】";
        assert!(SummaryProcessesRepository::update_meeting_summary(
            &pool,
            "m-r05",
            &serde_json::json!({
                "markdown": format!("generated summary{manual_marker}"),
                "template_snapshot": link("forged_generation"),
                "factValidation": {"status": "needs_review"}
            }),
            &fact_validation,
        )
        .await
        .unwrap());
        pool.close().await;

        // Reopening the file-backed database represents a full application restart.
        let reopened = file_pool(&database_path).await;
        let restarted_result: String =
            sqlx::query_scalar("SELECT result FROM summary_processes WHERE meeting_id = 'm-r05'")
                .fetch_one(&reopened)
                .await
                .unwrap();
        let restarted_result: Value = serde_json::from_str(&restarted_result).unwrap();
        let edit_survived_restart = restarted_result["markdown"]
            .as_str()
            .is_some_and(|markdown| markdown.contains(manual_marker));
        let authoritative_context_survived = restarted_result["factValidation"]["meetingContextId"]
            == "ctx_r05_recording"
            && restarted_result["template_snapshot"] == link("gen_r05_original");

        // A meeting-context reload changes only meeting metadata and must not rewrite
        // the current summary or its append-only manual revision.
        sqlx::query("UPDATE meetings SET updated_at = 'context-reloaded' WHERE id = 'm-r05'")
            .execute(&reopened)
            .await
            .unwrap();
        let after_context_reload: String =
            sqlx::query_scalar("SELECT result FROM summary_processes WHERE meeting_id = 'm-r05'")
                .fetch_one(&reopened)
                .await
                .unwrap();
        let context_reload_did_not_overwrite = after_context_reload == restarted_result.to_string();

        let second_link = link("gen_r05_second");
        SummaryProcessesRepository::create_or_reset_process_with_generation_history(
            &reopened,
            "m-r05",
            &second_link,
            &NewSummaryGenerationHistory {
                generation_id: "gen_r05_second",
                template_id: "license_station_weekly",
                template_version: 3,
                file_sha256: &"e".repeat(64),
                semantic_sha256: &"f".repeat(64),
                snapshot_path_relative: "summary-template-snapshots/gen_r05_second.json",
                resolution_source: "meeting_override",
                model_provider: "builtin-ai",
                model_name: "qwen3.5:2b",
                summary_language: Some("zh-CN"),
                source_binding_schema_version: 1,
                transcript_source: "whisper",
                transcript_version_id: "legacy-whisper-m-r05",
                transcript_version: 1,
                moss_run_id: None,
                transcript_activated_at: None,
                transcript_sha256: concat!(
                    "11111111", "11111111", "11111111", "11111111", "11111111", "11111111",
                    "11111111", "11111111"
                ),
                speaker_binding_snapshot_id: "legacy-binding-m-r05",
                speaker_binding_version: 1,
                speaker_binding_sha256: concat!(
                    "22222222", "22222222", "22222222", "22222222", "22222222", "22222222",
                    "22222222", "22222222"
                ),
            },
        )
        .await
        .unwrap();
        let pending_result: String =
            sqlx::query_scalar("SELECT result FROM summary_processes WHERE meeting_id = 'm-r05'")
                .fetch_one(&reopened)
                .await
                .unwrap();
        let pending_kept_manual_summary = pending_result.contains(manual_marker);
        let regenerated = serde_json::json!({
            "markdown": "second generation summary",
            "template_snapshot": second_link,
            "factValidation": {"status": "passed", "warningCount": 0, "warnings": []}
        });
        assert!(
            SummaryProcessesRepository::update_process_completed_for_generation(
                &reopened,
                "m-r05",
                "gen_r05_second",
                regenerated.clone(),
                1,
                0.2,
            )
            .await
            .unwrap()
        );

        let (revision_summary, revision_markdown, source_generation_id): (
            String,
            Option<String>,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT summary_json, markdown, source_generation_id FROM summary_manual_revisions WHERE meeting_id = 'm-r05'",
        )
        .fetch_one(&reopened)
        .await
        .unwrap();
        let revision_summary: Value = serde_json::from_str(&revision_summary).unwrap();
        let revision_survived_regeneration = revision_markdown
            .as_deref()
            .is_some_and(|markdown| markdown.contains(manual_marker))
            && revision_summary["template_snapshot"] == link("gen_r05_original")
            && revision_summary["factValidation"]["meetingContextId"] == "ctx_r05_recording"
            && source_generation_id.as_deref() == Some("gen_r05_original");
        let active_result: String =
            sqlx::query_scalar("SELECT result FROM summary_processes WHERE meeting_id = 'm-r05'")
                .fetch_one(&reopened)
                .await
                .unwrap();
        let active_result: Value = serde_json::from_str(&active_result).unwrap();
        let new_generation_is_distinct = active_result == regenerated
            && !active_result["markdown"]
                .as_str()
                .unwrap_or_default()
                .contains(manual_marker);
        let generation_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM summary_generation_history WHERE meeting_id = 'm-r05'",
        )
        .fetch_one(&reopened)
        .await
        .unwrap();
        let checks = serde_json::json!({
            "manual_edit_survived_database_restart": edit_survived_restart,
            "authoritative_context_and_template_snapshot_survived": authoritative_context_survived,
            "context_reload_did_not_overwrite_manual_summary": context_reload_did_not_overwrite,
            "pending_regeneration_kept_manual_summary_visible": pending_kept_manual_summary,
            "manual_revision_survived_regeneration": revision_survived_regeneration,
            "new_generation_is_distinct_from_manual_revision": new_generation_is_distinct,
            "two_generation_history_records_exist": generation_count == 2,
        });
        assert!(checks
            .as_object()
            .unwrap()
            .values()
            .all(|value| value == &Value::Bool(true)));

        if let Ok(path) = std::env::var("MEETILY_MC_R05_EVIDENCE_PATH") {
            let path = std::path::PathBuf::from(path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            let evidence = serde_json::json!({
                "test_id": "MC-R05",
                "verdict": "PASS",
                "executed_at": Utc::now().to_rfc3339(),
                "storage": "file-backed SQLite closed and reopened",
                "manual_marker": manual_marker,
                "original_generation_id": "gen_r05_original",
                "new_generation_id": "gen_r05_second",
                "generation_history_count": generation_count,
                "checks": checks,
            });
            std::fs::write(path, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
        }
    }

    async fn insert_pending(
        pool: &SqlitePool,
        meeting_id: &str,
        current_generation: &str,
        result: Option<Value>,
        backup: Option<Value>,
    ) {
        sqlx::query(
            "INSERT INTO summary_processes (meeting_id, status, created_at, updated_at, result, result_backup, metadata) VALUES (?, 'PENDING', 'now', 'now', ?, ?, ?)",
        )
        .bind(meeting_id)
        .bind(result.map(|value| value.to_string()))
        .bind(backup.map(|value| value.to_string()))
        .bind(link(current_generation).to_string())
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO summary_generation_history (
                generation_id, meeting_id, status, created_at, updated_at,
                template_id, template_version, file_sha256, semantic_sha256,
                snapshot_path_relative, resolution_source, model_provider, model_name
            ) VALUES (?, ?, 'pending', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, 'daily_standup', 1, ?, ?, ?, 'meeting_override', 'test', 'test')
            "#,
        )
        .bind(current_generation)
        .bind(meeting_id)
        .bind("a".repeat(64))
        .bind("b".repeat(64))
        .bind(format!(
            "summary-template-snapshots/{current_generation}.json"
        ))
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn startup_recovery_fails_orphaned_generation_and_restores_backup() {
        let pool = pool().await;
        let old_link = link("gen_old");
        let old_result = serde_json::json!({
            "markdown": "saved before restart",
            "template_snapshot": old_link
        });
        insert_pending(
            &pool,
            "m-restart",
            "gen_interrupted",
            Some(old_result.clone()),
            Some(old_result.clone()),
        )
        .await;

        let recovered =
            SummaryProcessesRepository::recover_orphaned_generations_from_previous_run(&pool)
                .await
                .unwrap();
        assert_eq!(recovered.processes_failed, 1);
        assert_eq!(recovered.histories_failed, 1);

        let (status, error, result, metadata, backup, end_time): (
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT status, error, result, metadata, result_backup, end_time FROM summary_processes WHERE meeting_id = 'm-restart'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "failed");
        assert_eq!(error.as_deref(), Some(SUMMARY_INTERRUPTED_BY_RESTART));
        assert_eq!(
            serde_json::from_str::<Value>(result.as_deref().unwrap()).unwrap(),
            old_result
        );
        assert_eq!(
            serde_json::from_str::<Value>(metadata.as_deref().unwrap()).unwrap(),
            old_link
        );
        assert!(backup.is_none());
        assert!(end_time.is_some());

        let (history_status, category, completed_at): (String, Option<String>, Option<String>) =
            sqlx::query_as(
                "SELECT status, error_category, completed_at FROM summary_generation_history WHERE generation_id = 'gen_interrupted'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(history_status, "failed");
        assert_eq!(category.as_deref(), Some("interrupted_by_restart"));
        assert!(completed_at.is_some());

        let second =
            SummaryProcessesRepository::recover_orphaned_generations_from_previous_run(&pool)
                .await
                .unwrap();
        assert_eq!(second, OrphanedSummaryRecovery::default());
    }

    #[tokio::test]
    async fn startup_recovery_settles_orphan_history_without_touching_completed_summary() {
        let pool = pool().await;
        let completed_result = serde_json::json!({"markdown": "completed"}).to_string();
        sqlx::query(
            "INSERT INTO summary_processes (meeting_id, status, created_at, updated_at, result) VALUES ('m-completed', 'completed', 'now', 'now', ?)",
        )
        .bind(&completed_result)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO summary_generation_history (
                generation_id, meeting_id, status, created_at, updated_at,
                template_id, template_version, file_sha256, semantic_sha256,
                snapshot_path_relative, resolution_source, model_provider, model_name
            ) VALUES ('gen_orphan', 'm-missing-process', 'pending', 'now', 'now',
                      'daily_standup', 1, ?, ?, 'summary-template-snapshots/gen_orphan.json',
                      'meeting_override', 'test', 'test')
            "#,
        )
        .bind("a".repeat(64))
        .bind("b".repeat(64))
        .execute(&pool)
        .await
        .unwrap();

        let recovered =
            SummaryProcessesRepository::recover_orphaned_generations_from_previous_run(&pool)
                .await
                .unwrap();
        assert_eq!(recovered.processes_failed, 0);
        assert_eq!(recovered.histories_failed, 1);

        let (completed_status, preserved_result): (String, String) = sqlx::query_as(
            "SELECT status, result FROM summary_processes WHERE meeting_id = 'm-completed'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(completed_status, "completed");
        assert_eq!(preserved_result, completed_result);

        let orphan_status: String = sqlx::query_scalar(
            "SELECT status FROM summary_generation_history WHERE generation_id = 'gen_orphan'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(orphan_status, "failed");
    }

    #[tokio::test]
    async fn stale_completion_cannot_overwrite_newer_generation() {
        let pool = pool().await;
        insert_pending(&pool, "m1", "gen_new", None, None).await;
        let accepted = SummaryProcessesRepository::update_process_completed_for_generation(
            &pool,
            "m1",
            "gen_old",
            serde_json::json!({"template_snapshot": link("gen_old")}),
            1,
            0.1,
        )
        .await
        .unwrap();
        assert!(!accepted);
        let status: String =
            sqlx::query_scalar("SELECT status FROM summary_processes WHERE meeting_id = 'm1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "PENDING");
    }

    #[tokio::test]
    async fn current_generation_completion_is_committed_and_clears_backup() {
        let pool = pool().await;
        let old_result = serde_json::json!({"markdown": "old"});
        insert_pending(
            &pool,
            "m-current",
            "gen_current",
            Some(old_result.clone()),
            Some(old_result),
        )
        .await;
        let new_result = serde_json::json!({
            "markdown": "new",
            "template_snapshot": link("gen_current")
        });
        assert!(
            SummaryProcessesRepository::update_process_completed_for_generation(
                &pool,
                "m-current",
                "gen_current",
                new_result.clone(),
                2,
                0.5,
            )
            .await
            .unwrap()
        );
        let (status, result, backup): (String, String, Option<String>) = sqlx::query_as(
            "SELECT status, result, result_backup FROM summary_processes WHERE meeting_id = 'm-current'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "completed");
        assert_eq!(serde_json::from_str::<Value>(&result).unwrap(), new_result);
        assert!(backup.is_none());
    }

    #[tokio::test]
    async fn stale_cancellation_cannot_cancel_newer_generation() {
        let pool = pool().await;
        insert_pending(&pool, "m-cancel", "gen_new", None, None).await;
        assert!(
            !SummaryProcessesRepository::update_process_cancelled_for_generation(
                &pool, "m-cancel", "gen_old",
            )
            .await
            .unwrap()
        );
        let status: String = sqlx::query_scalar(
            "SELECT status FROM summary_processes WHERE meeting_id = 'm-cancel'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "PENDING");
    }

    #[tokio::test]
    async fn failed_regeneration_restores_result_and_its_snapshot_metadata() {
        let pool = pool().await;
        let old_link = link("gen_old");
        let old_result = serde_json::json!({
            "markdown": "previous summary",
            "template_snapshot": old_link
        });
        insert_pending(
            &pool,
            "m2",
            "gen_new",
            Some(old_result.clone()),
            Some(old_result.clone()),
        )
        .await;
        let accepted = SummaryProcessesRepository::update_process_failed_for_generation(
            &pool,
            "m2",
            "gen_new",
            "model unavailable",
        )
        .await
        .unwrap();
        assert!(accepted);
        let (status, result, metadata): (String, String, String) = sqlx::query_as(
            "SELECT status, result, metadata FROM summary_processes WHERE meeting_id = 'm2'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "failed");
        assert_eq!(serde_json::from_str::<Value>(&result).unwrap(), old_result);
        assert_eq!(
            serde_json::from_str::<Value>(&metadata).unwrap(),
            link("gen_old")
        );
    }

    #[tokio::test]
    async fn failed_regeneration_of_legacy_result_does_not_forge_metadata() {
        let pool = pool().await;
        let old_result = serde_json::json!({"markdown": "legacy summary"});
        insert_pending(
            &pool,
            "m3",
            "gen_new",
            Some(old_result.clone()),
            Some(old_result),
        )
        .await;
        assert!(
            SummaryProcessesRepository::update_process_cancelled_for_generation(
                &pool, "m3", "gen_new",
            )
            .await
            .unwrap()
        );
        let metadata: Option<String> =
            sqlx::query_scalar("SELECT metadata FROM summary_processes WHERE meeting_id = 'm3'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(metadata.is_none());
    }

    #[tokio::test]
    async fn starting_new_generation_supersedes_old_history_atomically() {
        let pool = pool().await;
        insert_pending(&pool, "m-start", "gen_old", None, None).await;
        let metadata = link("gen_new");
        let file_sha256 = "c".repeat(64);
        let semantic_sha256 = "d".repeat(64);
        SummaryProcessesRepository::create_or_reset_process_with_generation_history(
            &pool,
            "m-start",
            &metadata,
            &NewSummaryGenerationHistory {
                generation_id: "gen_new",
                template_id: "daily_standup",
                template_version: 2,
                file_sha256: &file_sha256,
                semantic_sha256: &semantic_sha256,
                snapshot_path_relative: "summary-template-snapshots/gen_new.json",
                resolution_source: "meeting_override",
                model_provider: "test",
                model_name: "test",
                summary_language: Some("zh-CN"),
                source_binding_schema_version: 1,
                transcript_source: "moss",
                transcript_version_id: "activation-test",
                transcript_version: 2,
                moss_run_id: Some("run-test"),
                transcript_activated_at: Some(
                    chrono::DateTime::parse_from_rfc3339("2026-08-30T00:00:00Z")
                        .unwrap()
                        .with_timezone(&Utc),
                ),
                transcript_sha256: concat!(
                    "33333333", "33333333", "33333333", "33333333", "33333333", "33333333",
                    "33333333", "33333333"
                ),
                speaker_binding_snapshot_id: "activation-test",
                speaker_binding_version: 2,
                speaker_binding_sha256: concat!(
                    "44444444", "44444444", "44444444", "44444444", "44444444", "44444444",
                    "44444444", "44444444"
                ),
            },
        )
        .await
        .unwrap();

        let old_status: String = sqlx::query_scalar(
            "SELECT status FROM summary_generation_history WHERE generation_id = 'gen_old'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let new_status: String = sqlx::query_scalar(
            "SELECT status FROM summary_generation_history WHERE generation_id = 'gen_new'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(old_status, "superseded");
        assert_eq!(new_status, "pending");
        let new_generation_rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM summary_generation_history WHERE generation_id = 'gen_new'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            new_generation_rows, 1,
            "one request must append one generation"
        );
        let history = SummaryProcessesRepository::list_generation_history(&pool, "m-start", 20)
            .await
            .unwrap()
            .into_iter()
            .find(|item| item.generation_id == "gen_new")
            .unwrap();
        assert_eq!(history.template_version, 2);
        assert_eq!(history.transcript_source.as_deref(), Some("moss"));
        assert_eq!(
            history.transcript_version_id.as_deref(),
            Some("activation-test")
        );
        assert_eq!(history.transcript_version, Some(2));
        assert_eq!(history.moss_run_id.as_deref(), Some("run-test"));
        assert_eq!(
            history.transcript_sha256.as_deref(),
            Some(concat!(
                "33333333", "33333333", "33333333", "33333333", "33333333", "33333333", "33333333",
                "33333333"
            ))
        );
        assert_eq!(
            history.speaker_binding_sha256.as_deref(),
            Some(concat!(
                "44444444", "44444444", "44444444", "44444444", "44444444", "44444444", "44444444",
                "44444444"
            ))
        );
        let current_metadata: String = sqlx::query_scalar(
            "SELECT metadata FROM summary_processes WHERE meeting_id = 'm-start'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(metadata_generation_matches(&current_metadata, "gen_new"));
    }

    #[tokio::test]
    async fn migrated_database_persists_and_queries_complete_source_lineage() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("p5-history.sqlite");
        let legacy_path = directory.path().join("absent-legacy.db");
        let manager = DatabaseManager::new(
            database_path.to_str().unwrap(),
            legacy_path.to_str().unwrap(),
        )
        .await
        .unwrap();
        let pool = manager.pool();
        sqlx::query(
            "INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('m-migrated', 'P5', '2026-08-30T00:00:00Z', '2026-08-30T00:00:00Z')",
        )
        .execute(pool)
        .await
        .unwrap();

        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('summary_generation_history')")
                .fetch_all(pool)
                .await
                .unwrap();
        for required in [
            "transcript_source",
            "moss_run_id",
            "transcript_sha256",
            "speaker_binding_sha256",
            "template_version",
        ] {
            assert!(
                columns.iter().any(|column| column == required),
                "{required}"
            );
        }

        let transcript_hash = "5".repeat(64);
        let binding_hash = "6".repeat(64);
        let file_hash = "7".repeat(64);
        let semantic_hash = "8".repeat(64);
        let metadata = link("gen-migrated");
        SummaryProcessesRepository::create_or_reset_process_with_generation_history(
            pool,
            "m-migrated",
            &metadata,
            &NewSummaryGenerationHistory {
                generation_id: "gen-migrated",
                template_id: "standard_meeting",
                template_version: 9,
                file_sha256: &file_hash,
                semantic_sha256: &semantic_hash,
                snapshot_path_relative: "summary-template-snapshots/gen-migrated.json",
                resolution_source: "meeting_override",
                model_provider: "builtin-ai",
                model_name: "qwen3.5:2b",
                summary_language: Some("zh-CN"),
                source_binding_schema_version: 1,
                transcript_source: "moss",
                transcript_version_id: "moss-activation-migrated",
                transcript_version: 3,
                moss_run_id: Some("moss-run-migrated"),
                transcript_activated_at: Some(
                    DateTime::parse_from_rfc3339("2026-08-30T00:01:00Z")
                        .unwrap()
                        .with_timezone(&Utc),
                ),
                transcript_sha256: &transcript_hash,
                speaker_binding_snapshot_id: "moss-activation-migrated",
                speaker_binding_version: 3,
                speaker_binding_sha256: &binding_hash,
            },
        )
        .await
        .unwrap();

        let history = SummaryProcessesRepository::list_generation_history(pool, "m-migrated", 10)
            .await
            .unwrap();
        assert_eq!(history.len(), 1, "one click creates one generation row");
        let row = &history[0];
        assert_eq!(row.model_name, "qwen3.5:2b");
        assert_eq!(row.template_version, 9);
        assert_eq!(row.transcript_source.as_deref(), Some("moss"));
        assert_eq!(row.moss_run_id.as_deref(), Some("moss-run-migrated"));
        assert_eq!(
            row.transcript_sha256.as_deref(),
            Some(transcript_hash.as_str())
        );
        assert_eq!(
            row.speaker_binding_sha256.as_deref(),
            Some(binding_hash.as_str())
        );
    }
}
