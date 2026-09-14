//! P3/P5 persistence adapter for authoritative summary input.
//!
//! A MOSS candidate is not a transcript version that summaries may consume.
//! Only the row currently marked `active` in P3, together with its immutable
//! `moss_activation_segments`, can become a MOSS summary source. When no MOSS
//! activation exists, the current native transcript rows remain the Whisper
//! fallback so a failed or merely completed MOSS run cannot block summaries.

use chrono::{DateTime, Utc};
use sqlx::{FromRow, SqlitePool};

use crate::summary::source_binding::{
    select_activated_summary_source, TranscriptEvidenceSegment, TranscriptVersionSnapshot,
    TranscriptSourceKind, ValidatedSummaryInput,
};

#[derive(Debug, FromRow)]
struct ActiveActivationRow {
    activation_id: String,
    meeting_id: String,
    run_id: String,
    activated_at: String,
    activation_version: i64,
}

#[derive(Debug, FromRow)]
struct ActiveActivationSegmentRow {
    transcript_id: String,
    start_ms: i64,
    end_ms: i64,
    speaker_label: String,
    resolved_person_id: Option<String>,
    resolved_person_display_name: Option<String>,
    text: String,
}

#[derive(Debug, FromRow)]
struct WhisperTranscriptRow {
    id: String,
    transcript: String,
    timestamp: String,
    audio_start_time: Option<f64>,
    audio_end_time: Option<f64>,
    duration: Option<f64>,
}

pub async fn resolve_active_summary_input(
    pool: &SqlitePool,
    meeting_id: &str,
) -> Result<ValidatedSummaryInput, String> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| format!("Failed to begin summary-source read: {error}"))?;
    let meeting_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM meetings WHERE id = ?)")
            .bind(meeting_id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(|error| format!("Failed to load summary source meeting: {error}"))?;
    if !meeting_exists {
        return Err(format!("Meeting not found: {meeting_id}"));
    }

    // Fetch all active rows and let the domain gate reject a corrupt database
    // containing more than one. Candidate, failed, cancelled, completed, and
    // rolled-back records are deliberately absent from this query.
    let activations = sqlx::query_as::<_, ActiveActivationRow>(
        r#"
        SELECT active.activation_id, active.meeting_id, active.run_id,
               active.activated_at,
               (
                   SELECT COUNT(*)
                     FROM moss_activation_snapshots history
                    WHERE history.meeting_id = active.meeting_id
               ) AS activation_version
          FROM moss_activation_snapshots active
         WHERE active.meeting_id = ? AND active.status = 'active'
         ORDER BY active.activated_at, active.activation_id
        "#,
    )
    .bind(meeting_id)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| format!("Failed to load active MOSS activation: {error}"))?;

    let mut versions = Vec::with_capacity(activations.len().max(1));
    for activation in activations {
        let segment_rows = sqlx::query_as::<_, ActiveActivationSegmentRow>(
            r#"
            SELECT transcript_id, start_ms, end_ms, speaker_label,
                   resolved_person_id, resolved_person_display_name, text
              FROM moss_activation_segments
             WHERE activation_id = ?
             ORDER BY segment_index
            "#,
        )
        .bind(&activation.activation_id)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|error| format!("Failed to load active MOSS segments: {error}"))?;
        let activated_at = DateTime::parse_from_rfc3339(&activation.activated_at)
            .map_err(|_| "Active MOSS activation timestamp is invalid".to_owned())?
            .with_timezone(&Utc);
        let activation_version = u64::try_from(activation.activation_version)
            .map_err(|_| "Active MOSS activation version is invalid".to_owned())?;
        let segments = segment_rows
            .into_iter()
            .map(|segment| {
                Ok(TranscriptEvidenceSegment {
                    segment_id: segment.transcript_id,
                    start_ms: Some(
                        u64::try_from(segment.start_ms)
                            .map_err(|_| "Active MOSS segment start is invalid")?,
                    ),
                    end_ms: Some(
                        u64::try_from(segment.end_ms)
                            .map_err(|_| "Active MOSS segment end is invalid")?,
                    ),
                    wall_clock: None,
                    anonymous_speaker: Some(segment.speaker_label),
                    bound_person_id: segment.resolved_person_id,
                    bound_display_name: segment.resolved_person_display_name,
                    text: segment.text,
                })
            })
            .collect::<Result<Vec<_>, &str>>()
            .map_err(str::to_owned)?;
        versions.push(
            TranscriptVersionSnapshot::from_moss_activation(
                activation.meeting_id,
                activation.activation_id,
                activation_version,
                activation.run_id,
                activated_at,
                segments,
            )
            .map_err(|error| format!("Active MOSS snapshot is invalid: {error}"))?,
        );
    }

    if versions.is_empty() {
        let rows = sqlx::query_as::<_, WhisperTranscriptRow>(
            r#"
            SELECT id, transcript, timestamp, audio_start_time,
                   audio_end_time, duration
              FROM transcripts
             WHERE meeting_id = ?
             ORDER BY CASE WHEN audio_start_time IS NULL THEN 1 ELSE 0 END,
                      audio_start_time, timestamp, id
            "#,
        )
        .bind(meeting_id)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|error| format!("Failed to load Whisper fallback: {error}"))?;
        let segments = rows
            .into_iter()
            .filter(|row| !row.transcript.trim().is_empty())
            .map(whisper_evidence_segment)
            .collect::<Result<Vec<_>, String>>()?;
        let source_kind =
            resolve_local_transcript_source_kind(&mut *transaction, meeting_id).await;
        versions.push(TranscriptVersionSnapshot::legacy_local(
            meeting_id,
            source_kind,
            segments,
        ));
    }

    transaction
        .commit()
        .await
        .map_err(|error| format!("Failed to finish summary-source read: {error}"))?;
    select_activated_summary_source(meeting_id, &versions)
        .map_err(|error| format!("Active summary source is invalid: {error}"))
}

/// Resolves which local engine produced the meeting's current transcript rows.
///
/// The per-meeting record written next to the audio (`metadata.json`) wins
/// because the engine can change between meetings; the global
/// `transcript_settings` row is the fallback for meetings whose transcripts
/// predate that record. Anything unreadable keeps the historical `whisper`
/// label instead of inventing a source.
async fn resolve_local_transcript_source_kind(
    executor: &mut sqlx::SqliteConnection,
    meeting_id: &str,
) -> TranscriptSourceKind {
    let folder_path: Option<String> =
        sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = ?")
            .bind(meeting_id)
            .fetch_optional(&mut *executor)
            .await
            .ok()
            .flatten();
    if let Some(provider) = folder_path.as_deref().and_then(read_metadata_provider) {
        return TranscriptSourceKind::from_provider(&provider);
    }

    let provider: Option<String> =
        sqlx::query_scalar("SELECT provider FROM transcript_settings WHERE id = '1' LIMIT 1")
            .fetch_optional(&mut *executor)
            .await
            .ok()
            .flatten();
    provider
        .as_deref()
        .map(TranscriptSourceKind::from_provider)
        .unwrap_or(TranscriptSourceKind::Whisper)
}

fn read_metadata_provider(folder_path: &str) -> Option<String> {
    let path = std::path::Path::new(folder_path).join("metadata.json");
    let raw = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value
        .get("transcription_provider")
        .and_then(|value| value.as_str())
        .map(str::to_owned)
}

fn whisper_evidence_segment(
    row: WhisperTranscriptRow,
) -> Result<TranscriptEvidenceSegment, String> {
    let start_ms = seconds_to_milliseconds(row.audio_start_time)?;
    let explicit_end = seconds_to_milliseconds(row.audio_end_time)?;
    let duration_ms = seconds_to_milliseconds(row.duration)?;
    let duration_end = start_ms.zip(duration_ms).map(|(start, duration)| {
        start
            .checked_add(duration)
            .ok_or_else(|| "Transcript timing is outside supported bounds".to_owned())
    });
    let duration_end = duration_end.transpose()?;
    let end_ms = match (explicit_end, duration_end) {
        (Some(explicit), Some(calculated)) => Some(explicit.max(calculated)),
        (explicit, calculated) => explicit.or(calculated),
    };
    Ok(TranscriptEvidenceSegment {
        segment_id: row.id,
        start_ms,
        end_ms,
        wall_clock: (!row.timestamp.trim().is_empty()).then_some(row.timestamp),
        anonymous_speaker: None,
        bound_person_id: None,
        bound_display_name: None,
        text: row.transcript,
    })
}

fn seconds_to_milliseconds(value: Option<f64>) -> Result<Option<u64>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if !value.is_finite() || value < 0.0 || value > u64::MAX as f64 / 1000.0 {
        return Err("Transcript timing is outside supported bounds".to_owned());
    }
    Ok(Some((value * 1000.0).round() as u64))
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;
    use crate::database::manager::DatabaseManager;
    use crate::database::moss::{
        BoundPerson, CandidateSegmentInput, MossCandidateRepository, MossCompletion, NewMossRun,
    };
    use crate::summary::source_binding::{
        evaluate_summary_freshness, SummaryFreshnessStatus, SummarySourceBinding,
        SummaryStaleReason, SummaryTemplateBinding, TranscriptSourceKind,
    };

    fn digest(label: &str) -> String {
        format!("{:x}", Sha256::digest(label.as_bytes()))
    }

    async fn insert_migrated_meeting(pool: &SqlitePool, meeting_id: &str, text: &str) {
        let now = "2026-08-30T00:00:00.000Z";
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES (?, ?, ?, ?)")
            .bind(meeting_id)
            .bind("P5 integration")
            .bind(now)
            .bind(now)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            r#"
            INSERT INTO transcripts (
                id, meeting_id, transcript, timestamp, summary, action_items,
                key_points, audio_start_time, audio_end_time, duration, speaker
            ) VALUES (?, ?, ?, ?, NULL, NULL, NULL, 0.0, 1.0, 1.0, 'microphone')
            "#,
        )
        .bind(format!("transcript-{meeting_id}"))
        .bind(meeting_id)
        .bind(text)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO transcript_chunks (
                meeting_id, transcript_text, model, model_name, created_at
            ) VALUES (?, ?, 'whisper', 'large-v3', ?)
            "#,
        )
        .bind(meeting_id)
        .bind(text)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
    }

    fn run_input(meeting_id: &str, suffix: &str) -> NewMossRun {
        NewMossRun {
            meeting_id: meeting_id.to_owned(),
            audio_sha256: digest(&format!("audio-{suffix}")),
            model_sha256: digest("moss-model"),
            runtime_sha256: digest("moss-runtime"),
            context_sha256: digest("meeting-context"),
            backend_name: "transcribe.cpp".to_owned(),
            backend_version: "v0.2.2".to_owned(),
            runtime_version: "c6a9257".to_owned(),
            model_revision: "openmoss-q8-fixed".to_owned(),
            language_requested: moss_helper::native::MOSS_LANGUAGE_REQUESTED.to_owned(),
            decode_parameters_json: moss_helper::native::MOSS_DECODE_PARAMETERS_JSON.to_owned(),
            decode_parameters_sha256: moss_helper::native::moss_decode_parameters_sha256(),
        }
    }

    fn completion(suffix: &str) -> MossCompletion {
        MossCompletion {
            device_name: "P5 test CPU".to_owned(),
            raw_output_sha256: digest(&format!("raw-{suffix}")),
            clean_output_sha256: digest(&format!("clean-{suffix}")),
            wall_elapsed_ms: 1,
            wall_rtf: 0.1,
            peak_memory_bytes: 1,
            language_requested: moss_helper::native::MOSS_LANGUAGE_REQUESTED.to_owned(),
            language_resolved: moss_helper::native::MOSS_LANGUAGE_RESOLVED.to_owned(),
            decode_parameters_json: moss_helper::native::MOSS_DECODE_PARAMETERS_JSON.to_owned(),
            decode_parameters_sha256: moss_helper::native::moss_decode_parameters_sha256(),
        }
    }

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        for statement in [
            "CREATE TABLE meetings (id TEXT PRIMARY KEY)",
            r#"CREATE TABLE transcripts (
                id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL, transcript TEXT NOT NULL,
                timestamp TEXT NOT NULL, audio_start_time REAL, audio_end_time REAL,
                duration REAL
            )"#,
            r#"CREATE TABLE moss_activation_snapshots (
                activation_id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL,
                run_id TEXT NOT NULL, status TEXT NOT NULL, activated_at TEXT NOT NULL
            )"#,
            r#"CREATE TABLE moss_activation_segments (
                activation_id TEXT NOT NULL, segment_index INTEGER NOT NULL,
                transcript_id TEXT NOT NULL, start_ms INTEGER NOT NULL,
                end_ms INTEGER NOT NULL, speaker_label TEXT NOT NULL,
                resolved_person_id TEXT, resolved_person_display_name TEXT,
                text TEXT NOT NULL
            )"#,
            r#"CREATE TABLE moss_transcription_runs (
                run_id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL, status TEXT NOT NULL
            )"#,
            r#"CREATE TABLE moss_candidate_segments (
                segment_id TEXT PRIMARY KEY, run_id TEXT NOT NULL,
                segment_index INTEGER NOT NULL, raw_text TEXT NOT NULL
            )"#,
        ] {
            sqlx::query(statement).execute(&pool).await.unwrap();
        }
        pool
    }

    async fn insert_meeting_with_whisper(pool: &SqlitePool, text: &str) {
        sqlx::query("INSERT INTO meetings (id) VALUES ('meeting-1')")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            r#"INSERT INTO transcripts (
                id, meeting_id, transcript, timestamp,
                audio_start_time, audio_end_time, duration
            ) VALUES ('whisper-1', 'meeting-1', ?, '2026-08-30T00:00:00Z', 1.0, 2.0, 1.0)"#,
        )
        .bind(text)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn active_moss_reads_frozen_activation_segments_not_mutable_transcripts() {
        let pool = test_pool().await;
        insert_meeting_with_whisper(&pool, "后来被人工改动的当前行").await;
        sqlx::query(
            r#"INSERT INTO moss_activation_snapshots
               (activation_id, meeting_id, run_id, status, activated_at)
               VALUES ('activation-old', 'meeting-1', 'run-old', 'rolled_back', '2026-08-29T23:00:00Z'),
                      ('activation-active', 'meeting-1', 'run-active', 'active', '2026-08-30T00:00:00Z')"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"INSERT INTO moss_activation_segments (
                activation_id, segment_index, transcript_id, start_ms, end_ms,
                speaker_label, resolved_person_id, resolved_person_display_name, text
            ) VALUES (
                'activation-active', 0, 'moss-transcript-1', 1000, 2200,
                'S01', 'person-1', '王芳', '冻结的 MOSS 激活内容'
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();

        let input = resolve_active_summary_input(&pool, "meeting-1")
            .await
            .unwrap();
        assert_eq!(input.source.source_kind, TranscriptSourceKind::Moss);
        assert_eq!(input.source.transcript_version_id, "activation-active");
        assert_eq!(input.source.transcript_version, 2);
        assert_eq!(input.source.moss_run_id.as_deref(), Some("run-active"));
        assert!(input.transcript_text.contains("王芳: 冻结的 MOSS 激活内容"));
        assert!(!input.transcript_text.contains("后来被人工改动"));
        input.source.validate_active().unwrap();
    }

    #[tokio::test]
    async fn failed_candidate_and_rolled_back_activation_cannot_enter_summary() {
        let pool = test_pool().await;
        insert_meeting_with_whisper(&pool, "Whisper 与默认摘要链保持可用").await;
        sqlx::query(
            "INSERT INTO moss_transcription_runs (run_id, meeting_id, status) VALUES ('run-failed', 'meeting-1', 'failed')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO moss_candidate_segments (segment_id, run_id, segment_index, raw_text) VALUES ('candidate-1', 'run-failed', 0, '失败候选不得摘要')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO moss_activation_snapshots (activation_id, meeting_id, run_id, status, activated_at) VALUES ('activation-rolled-back', 'meeting-1', 'run-old', 'rolled_back', '2026-08-29T23:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO moss_activation_segments (activation_id, segment_index, transcript_id, start_ms, end_ms, speaker_label, text) VALUES ('activation-rolled-back', 0, 'old-1', 0, 1000, 'S01', '已回退内容不得摘要')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let input = resolve_active_summary_input(&pool, "meeting-1")
            .await
            .unwrap();
        assert_eq!(input.source.source_kind, TranscriptSourceKind::Whisper);
        assert_eq!(input.source.moss_run_id, None);
        assert!(input
            .transcript_text
            .contains("Whisper 与默认摘要链保持可用"));
        assert!(!input.transcript_text.contains("失败候选不得摘要"));
        assert!(!input.transcript_text.contains("已回退内容不得摘要"));
    }

    #[tokio::test]
    async fn malformed_active_snapshot_fails_closed_instead_of_using_whisper() {
        let pool = test_pool().await;
        insert_meeting_with_whisper(&pool, "不能掩盖损坏激活快照").await;
        sqlx::query(
            "INSERT INTO moss_activation_snapshots (activation_id, meeting_id, run_id, status, activated_at) VALUES ('activation-empty', 'meeting-1', 'run-active', 'active', '2026-08-30T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let error = resolve_active_summary_input(&pool, "meeting-1")
            .await
            .unwrap_err();
        assert!(error.contains("SUMMARY_SOURCE_SEGMENT_INVALID"));
    }

    #[tokio::test]
    async fn multiple_active_rows_fail_closed_even_without_the_p3_unique_index() {
        let pool = test_pool().await;
        insert_meeting_with_whisper(&pool, "不能掩盖多个激活版本").await;
        for (activation, run, index) in [
            ("activation-a", "run-a", 0_i64),
            ("activation-b", "run-b", 1_i64),
        ] {
            sqlx::query(
                "INSERT INTO moss_activation_snapshots (activation_id, meeting_id, run_id, status, activated_at) VALUES (?, 'meeting-1', ?, 'active', '2026-08-30T00:00:00Z')",
            )
            .bind(activation)
            .bind(run)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO moss_activation_segments (activation_id, segment_index, transcript_id, start_ms, end_ms, speaker_label, text) VALUES (?, 0, ?, ?, ?, 'S01', '冲突激活')",
            )
            .bind(activation)
            .bind(format!("transcript-{index}"))
            .bind(index * 1000)
            .bind(index * 1000 + 500)
            .execute(&pool)
            .await
            .unwrap();
        }

        let error = resolve_active_summary_input(&pool, "meeting-1")
            .await
            .unwrap_err();
        assert!(error.contains("SUMMARY_SOURCE_MULTIPLE_ACTIVE_VERSIONS"));
    }

    #[tokio::test]
    async fn migrated_p3_repository_and_p5_resolver_share_the_authoritative_activation() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("p3-p5.sqlite");
        let legacy_path = directory.path().join("absent-legacy.db");
        let manager = DatabaseManager::new(
            database_path.to_str().unwrap(),
            legacy_path.to_str().unwrap(),
        )
        .await
        .unwrap();
        let pool = manager.pool();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(pool)
            .await
            .unwrap();
        insert_migrated_meeting(pool, "meeting-integrated", "Whisper 回退正文").await;

        let failed =
            MossCandidateRepository::start_run(pool, run_input("meeting-integrated", "failed"))
                .await
                .unwrap();
        MossCandidateRepository::mark_run_failed(pool, &failed.run_id, "MOSS_RUNTIME_FAILED")
            .await
            .unwrap();
        let fallback = resolve_active_summary_input(pool, "meeting-integrated")
            .await
            .unwrap();
        assert_eq!(fallback.source.source_kind, TranscriptSourceKind::Whisper);
        assert!(fallback.transcript_text.contains("Whisper 回退正文"));
        let template = SummaryTemplateBinding {
            template_id: "standard_meeting".to_owned(),
            template_version: 1,
            template_file_sha256: digest("template-file"),
            template_semantic_sha256: digest("template-semantic"),
        };
        let generated_from_whisper =
            SummarySourceBinding::from_active_source(&fallback.source, template.clone()).unwrap();

        let run =
            MossCandidateRepository::start_run(pool, run_input("meeting-integrated", "active"))
                .await
                .unwrap();
        MossCandidateRepository::complete_run(
            pool,
            &run.run_id,
            &[CandidateSegmentInput {
                segment_index: 0,
                start_ms: 1_000,
                end_ms: 2_000,
                speaker_label: "S01".to_owned(),
                text: "冻结激活正文".to_owned(),
            }],
            completion("active"),
        )
        .await
        .unwrap();
        MossCandidateRepository::bind_speaker(
            pool,
            &run.run_id,
            "S01",
            BoundPerson {
                person_id: "person-owner".to_owned(),
                display_name: "负责人甲".to_owned(),
            },
            &run.context_sha256,
        )
        .await
        .unwrap();
        let activation = MossCandidateRepository::activate_candidate(pool, &run.run_id)
            .await
            .unwrap();

        // Mutating the compatibility transcript table after activation must
        // not alter what P5 reads. The immutable P3 activation is authoritative.
        sqlx::query(
            "UPDATE transcripts SET transcript = '不得进入摘要的可变行' WHERE meeting_id = ?",
        )
        .bind("meeting-integrated")
        .execute(pool)
        .await
        .unwrap();
        let active = resolve_active_summary_input(pool, "meeting-integrated")
            .await
            .unwrap();
        assert_eq!(active.source.source_kind, TranscriptSourceKind::Moss);
        assert_eq!(
            active.source.transcript_version_id,
            activation.activation_id
        );
        assert_eq!(
            active.source.moss_run_id.as_deref(),
            Some(run.run_id.as_str())
        );
        assert!(active.transcript_text.contains("负责人甲: 冻结激活正文"));
        assert!(!active.transcript_text.contains("不得进入摘要的可变行"));
        active.source.validate_active().unwrap();
        let current = SummarySourceBinding::from_active_source(&active.source, template).unwrap();
        let freshness = evaluate_summary_freshness(&generated_from_whisper, &current).unwrap();
        assert_eq!(freshness.status, SummaryFreshnessStatus::Stale);
        assert!(freshness
            .reasons
            .contains(&SummaryStaleReason::TranscriptVersionChanged));
        assert!(freshness
            .reasons
            .contains(&SummaryStaleReason::TranscriptContentChanged));
        assert!(freshness
            .reasons
            .contains(&SummaryStaleReason::SpeakerBindingsChanged));
    }
}
