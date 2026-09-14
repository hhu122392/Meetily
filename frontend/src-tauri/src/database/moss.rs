use std::collections::BTreeSet;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Sqlite, SqlitePool, Transaction};
use thiserror::Error;
use uuid::Uuid;

use crate::moss_audio_token_alignment::{
    validate_audio_token_track, AudioTokenTrack, CandidateAudioTokenBoundary,
    MachineTermSuggestion, ALIGNMENT_METHOD_AUDIO_TOKEN, R5_MIN_GLOBAL_MATCH_COVERAGE,
};
use crate::moss_helper::manager::ManagedTranscription;

pub const MOSS_INTERRUPTED_BY_RESTART: &str = "MOSS_INTERRUPTED_BY_RESTART";

#[derive(Debug, Error)]
pub enum MossStoreError {
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("MOSS store serialization failed")]
    Serialization(#[from] serde_json::Error),
    #[error("invalid MOSS store input: {0}")]
    InvalidInput(&'static str),
    #[error("meeting was not found")]
    MeetingNotFound,
    #[error("MOSS run was not found")]
    RunNotFound,
    #[error("another MOSS run is already active for this meeting")]
    ActiveRunExists,
    #[error("MOSS run is not in the required state")]
    InvalidRunState,
    #[error("MOSS candidate was not found")]
    CandidateNotFound,
    #[error("another MOSS activation is already active for this meeting")]
    ActiveActivationExists,
    #[error("active MOSS activation was not found")]
    ActivationNotFound,
    #[error("transcript changed after the MOSS run started")]
    TranscriptConflict { expected: String, actual: String },
    #[error("activated transcript changed before rollback")]
    RollbackConflict { expected: String, actual: String },
    #[error("injected transaction failure")]
    InjectedFailure,
}

#[derive(Debug, Clone)]
pub struct NewMossRun {
    pub meeting_id: String,
    pub audio_sha256: String,
    pub model_sha256: String,
    pub runtime_sha256: String,
    pub context_sha256: String,
    pub backend_name: String,
    pub backend_version: String,
    pub runtime_version: String,
    pub model_revision: String,
    pub language_requested: String,
    pub decode_parameters_json: String,
    pub decode_parameters_sha256: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize, PartialEq)]
pub struct MossRunRecord {
    pub run_id: String,
    pub meeting_id: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub source_transcript_sha256: String,
    pub audio_sha256: String,
    pub model_sha256: String,
    pub runtime_sha256: String,
    pub context_sha256: String,
    pub backend_name: String,
    pub backend_version: String,
    pub runtime_version: String,
    pub model_revision: String,
    pub device_name: Option<String>,
    pub raw_output_sha256: Option<String>,
    pub clean_output_sha256: Option<String>,
    pub candidate_sha256: Option<String>,
    pub segment_count: i64,
    pub wall_elapsed_ms: Option<i64>,
    pub wall_rtf: Option<f64>,
    pub peak_memory_bytes: Option<i64>,
    pub error_code: Option<String>,
    pub language_requested: Option<String>,
    pub language_resolved: Option<String>,
    pub decode_parameters_json: Option<String>,
    pub decode_parameters_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CandidateSegmentInput {
    pub segment_index: u32,
    pub start_ms: i64,
    pub end_ms: i64,
    pub speaker_label: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct MossCompletion {
    pub device_name: String,
    pub raw_output_sha256: String,
    pub clean_output_sha256: String,
    pub wall_elapsed_ms: u64,
    pub wall_rtf: f64,
    pub peak_memory_bytes: u64,
    pub language_requested: String,
    pub language_resolved: String,
    pub decode_parameters_json: String,
    pub decode_parameters_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CandidateAlignmentInput {
    pub segment_index: u32,
    pub raw_segment_index: u32,
    pub raw_start_ms: i64,
    pub raw_end_ms: i64,
    pub raw_text_sha256: String,
    pub alignment_method: String,
    pub confidence: Option<f64>,
    pub source_anchor_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MossRunDiagnosticsInput {
    pub audio_duration_ms: u64,
    pub activity_frame_ms: u32,
    pub activity_threshold_dbfs: f64,
    pub first_active_ms: Option<u64>,
    pub last_active_ms: Option<u64>,
    pub model_last_timestamp_ms: i64,
    pub aligned_segment_count: u32,
    pub fallback_segment_count: u32,
    pub source_anchor_count: u32,
    pub source_hash_verified: bool,
    pub source_expected_sha256: String,
    pub source_actual_sha256: String,
    pub fallback_reason: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize, PartialEq)]
pub struct MossRunDiagnosticsRecord {
    pub run_id: String,
    pub audio_duration_ms: i64,
    pub activity_frame_ms: i64,
    pub activity_threshold_dbfs: f64,
    pub first_active_ms: Option<i64>,
    pub last_active_ms: Option<i64>,
    pub model_last_timestamp_ms: i64,
    pub tail_delta_ms: Option<i64>,
    pub aligned_segment_count: i64,
    pub fallback_segment_count: i64,
    pub source_anchor_count: i64,
    pub source_hash_verified: bool,
    pub source_expected_sha256: String,
    pub source_actual_sha256: String,
    pub fallback_reason: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct AudioTokenAlignmentInput {
    pub track: Option<AudioTokenTrack>,
    pub boundaries: Vec<CandidateAudioTokenBoundary>,
    pub global_match_coverage: Option<f64>,
    pub token_aligned_segment_count: u32,
    pub fallback_raw_segment_count: u32,
    pub fallback_reason: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize, PartialEq)]
pub struct AudioTokenAlignmentRecord {
    pub run_id: String,
    pub status: String,
    pub audio_sha256: String,
    pub audio_duration_ms: Option<i64>,
    pub model_name: Option<String>,
    pub model_sha256: Option<String>,
    pub program_sha256: Option<String>,
    pub parameters_sha256: Option<String>,
    pub token_track_sha256: Option<String>,
    pub backend: Option<String>,
    pub global_match_coverage: Option<f64>,
    pub token_aligned_segment_count: i64,
    pub fallback_raw_segment_count: i64,
    pub fallback_reason: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceTranscriptAnchor {
    pub anchor_id: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceTranscriptAnchorSnapshot {
    pub expected_sha256: String,
    pub actual_sha256: String,
    pub hash_verified: bool,
    pub invalid_timed_rows: u32,
    pub anchors: Vec<SourceTranscriptAnchor>,
}

#[derive(Debug, Clone)]
pub struct TermCorrectionInput {
    pub start_char: usize,
    pub end_char: usize,
    pub original_text: String,
    pub replacement_text: String,
    pub rule_id: String,
    pub expected_context_sha256: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize, PartialEq, Eq)]
pub struct TermCorrectionRecord {
    pub correction_id: String,
    pub segment_id: String,
    pub revision: i64,
    pub original_text: String,
    pub replacement_text: String,
    pub result_text: String,
    pub start_char: i64,
    pub end_char: i64,
    pub rule_id: String,
    pub context_sha256: String,
    pub created_at: String,
    pub reverted_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundPerson {
    pub person_id: String,
    pub display_name: String,
}

#[derive(Debug, Clone)]
pub struct SegmentOverrideInput {
    pub replacement_text: Option<String>,
    pub person: Option<BoundPerson>,
    pub reason_code: String,
    pub expected_context_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivationResult {
    pub activation_id: String,
    pub meeting_id: String,
    pub run_id: String,
    pub previous_transcript_sha256: String,
    pub activated_transcript_sha256: String,
    pub segment_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RollbackResult {
    pub activation_id: String,
    pub meeting_id: String,
    pub restored_transcript_sha256: String,
    pub segment_count: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InterruptedRunRecovery {
    pub runs_failed: u64,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize, PartialEq)]
struct StoredTranscript {
    id: String,
    meeting_id: String,
    transcript: String,
    timestamp: String,
    summary: Option<String>,
    action_items: Option<String>,
    key_points: Option<String>,
    audio_start_time: Option<f64>,
    audio_end_time: Option<f64>,
    duration: Option<f64>,
    speaker: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
struct CandidateSegmentRow {
    segment_id: String,
    segment_index: i64,
    start_ms: i64,
    end_ms: i64,
    speaker_label: String,
    raw_text: String,
}

#[derive(Debug, Clone, FromRow)]
struct SegmentOverrideRow {
    override_id: String,
    replacement_text: Option<String>,
    person_id: Option<String>,
    person_display_name: Option<String>,
}

#[derive(Debug, Clone)]
struct MaterializedCandidateSegment {
    candidate_segment_id: String,
    segment_index: i64,
    start_ms: i64,
    end_ms: i64,
    speaker_label: String,
    text: String,
    resolved_person: Option<BoundPerson>,
    correction_id: Option<String>,
    override_id: Option<String>,
    binding_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MutationFault {
    None,
    #[cfg(test)]
    AfterTranscriptDelete,
}

pub struct MossCandidateRepository;

impl MossCandidateRepository {
    pub async fn current_transcript_sha256(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<String, MossStoreError> {
        ensure_meeting_exists(pool, meeting_id).await?;
        let (_, hash) = transcript_snapshot(pool, meeting_id).await?;
        Ok(hash)
    }

    pub async fn start_run(
        pool: &SqlitePool,
        input: NewMossRun,
    ) -> Result<MossRunRecord, MossStoreError> {
        let meeting_id = required(input.meeting_id, "meeting_id")?;
        let audio_sha256 = normalized_sha256(&input.audio_sha256)?;
        let model_sha256 = normalized_sha256(&input.model_sha256)?;
        let runtime_sha256 = normalized_sha256(&input.runtime_sha256)?;
        let context_sha256 = normalized_sha256(&input.context_sha256)?;
        let backend_name = required(input.backend_name, "backend_name")?;
        let backend_version = required(input.backend_version, "backend_version")?;
        let runtime_version = required(input.runtime_version, "runtime_version")?;
        let model_revision = required(input.model_revision, "model_revision")?;
        let language_requested = required(input.language_requested, "language_requested")?;
        let decode_parameters_json =
            required(input.decode_parameters_json, "decode_parameters_json")?;
        let decode_parameters_sha256 = normalized_sha256(&input.decode_parameters_sha256)?;
        if language_requested != moss_helper::native::MOSS_LANGUAGE_REQUESTED
            || decode_parameters_json != moss_helper::native::MOSS_DECODE_PARAMETERS_JSON
            || !decode_parameters_sha256
                .eq_ignore_ascii_case(&moss_helper::native::moss_decode_parameters_sha256())
        {
            return Err(MossStoreError::InvalidInput("native_decode_contract"));
        }
        let run_id = format!("moss-run-{}", Uuid::new_v4());
        let now = timestamp();
        let mut tx = pool.begin().await?;
        let result = async {
            ensure_meeting_exists(&mut *tx, &meeting_id).await?;
            sqlx::query("UPDATE meetings SET updated_at = updated_at WHERE id = ?")
                .bind(&meeting_id)
                .execute(&mut *tx)
                .await?;
            let active_activation: bool = sqlx::query_scalar(
                r#"
                SELECT EXISTS(
                    SELECT 1 FROM moss_activation_snapshots
                    WHERE meeting_id = ? AND status = 'active'
                )
                "#,
            )
            .bind(&meeting_id)
            .fetch_one(&mut *tx)
            .await?;
            if active_activation {
                return Err(MossStoreError::ActiveActivationExists);
            }
            let (_, source_transcript_sha256) = transcript_snapshot(&mut *tx, &meeting_id).await?;
            let insert = sqlx::query(
                r#"
                INSERT INTO moss_transcription_runs (
                    run_id, meeting_id, status, created_at, updated_at, started_at,
                    source_transcript_sha256, audio_sha256, model_sha256,
                    runtime_sha256, context_sha256, backend_name, backend_version,
                    runtime_version, model_revision, language_requested,
                    decode_parameters_json, decode_parameters_sha256
                ) VALUES (?, ?, 'running', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&run_id)
            .bind(&meeting_id)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .bind(&source_transcript_sha256)
            .bind(&audio_sha256)
            .bind(&model_sha256)
            .bind(&runtime_sha256)
            .bind(&context_sha256)
            .bind(&backend_name)
            .bind(&backend_version)
            .bind(&runtime_version)
            .bind(&model_revision)
            .bind(&language_requested)
            .bind(&decode_parameters_json)
            .bind(&decode_parameters_sha256)
            .execute(&mut *tx)
            .await;
            if let Err(error) = insert {
                if is_unique_violation(&error) {
                    return Err(MossStoreError::ActiveRunExists);
                }
                return Err(error.into());
            }
            get_run_in_transaction(&mut tx, &run_id).await
        }
        .await;
        finish_transaction(tx, result).await
    }

    pub async fn get_run(pool: &SqlitePool, run_id: &str) -> Result<MossRunRecord, MossStoreError> {
        sqlx::query_as::<_, MossRunRecord>(
            r#"
            SELECT run_id, meeting_id, status, created_at, updated_at, started_at,
                   completed_at, source_transcript_sha256, audio_sha256,
                   model_sha256, runtime_sha256, context_sha256, backend_name,
                   backend_version, runtime_version, model_revision, device_name,
                   raw_output_sha256, clean_output_sha256, candidate_sha256,
                   segment_count, wall_elapsed_ms, wall_rtf, peak_memory_bytes,
                   error_code, language_requested, language_resolved,
                   decode_parameters_json, decode_parameters_sha256
            FROM moss_transcription_runs
            WHERE run_id = ?
            "#,
        )
        .bind(run_id)
        .fetch_optional(pool)
        .await?
        .ok_or(MossStoreError::RunNotFound)
    }

    pub async fn source_transcript_anchors(
        pool: &SqlitePool,
        run_id: &str,
    ) -> Result<SourceTranscriptAnchorSnapshot, MossStoreError> {
        let run = Self::get_run(pool, run_id).await?;
        if run.status != "running" {
            return Err(MossStoreError::InvalidRunState);
        }
        let (rows, actual_sha256) = transcript_snapshot(pool, &run.meeting_id).await?;
        let hash_verified = actual_sha256 == run.source_transcript_sha256;
        let mut invalid_timed_rows = 0u32;
        let mut anchors = Vec::new();
        if hash_verified {
            for row in rows {
                if row.transcript.trim().is_empty() {
                    continue;
                }
                let Some(start_seconds) = row.audio_start_time else {
                    invalid_timed_rows = invalid_timed_rows.saturating_add(1);
                    continue;
                };
                let Some(end_seconds) = row.audio_end_time else {
                    invalid_timed_rows = invalid_timed_rows.saturating_add(1);
                    continue;
                };
                if !start_seconds.is_finite()
                    || !end_seconds.is_finite()
                    || start_seconds < 0.0
                    || end_seconds <= start_seconds
                    || end_seconds > i64::MAX as f64 / 1_000.0
                {
                    invalid_timed_rows = invalid_timed_rows.saturating_add(1);
                    continue;
                }
                let start_ms = (start_seconds * 1_000.0).round() as i64;
                let end_ms = (end_seconds * 1_000.0).round() as i64;
                if end_ms <= start_ms {
                    invalid_timed_rows = invalid_timed_rows.saturating_add(1);
                    continue;
                }
                anchors.push(SourceTranscriptAnchor {
                    anchor_id: row.id,
                    start_ms,
                    end_ms,
                    text: row.transcript,
                });
            }
        }
        if !hash_verified || invalid_timed_rows > 0 {
            anchors.clear();
        }
        Ok(SourceTranscriptAnchorSnapshot {
            expected_sha256: run.source_transcript_sha256,
            actual_sha256,
            hash_verified,
            invalid_timed_rows,
            anchors,
        })
    }

    pub async fn complete_run_from_managed(
        pool: &SqlitePool,
        run_id: &str,
        result: &ManagedTranscription,
    ) -> Result<MossRunRecord, MossStoreError> {
        let segments = result
            .segments
            .iter()
            .map(|segment| {
                let speaker_number = segment
                    .speaker_id
                    .checked_add(1)
                    .ok_or(MossStoreError::InvalidInput("speaker_id"))?;
                Ok(CandidateSegmentInput {
                    segment_index: segment.segment_index,
                    start_ms: segment.t0_ms,
                    end_ms: segment.t1_ms,
                    speaker_label: format!("S{speaker_number:02}"),
                    text: segment.text.clone(),
                })
            })
            .collect::<Result<Vec<_>, MossStoreError>>()?;
        Self::complete_run(
            pool,
            run_id,
            &segments,
            MossCompletion {
                device_name: result.completed.device_description.clone(),
                raw_output_sha256: result.completed.raw_text_sha256.clone(),
                clean_output_sha256: result.completed.clean_text_sha256.clone(),
                wall_elapsed_ms: result.supervisor_wall_elapsed_ms,
                wall_rtf: result.supervisor_wall_rtf,
                peak_memory_bytes: result.helper_peak_job_memory_bytes,
                language_requested: result.language_requested.clone(),
                language_resolved: result.language_resolved.clone(),
                decode_parameters_json: result.decode_parameters_json.clone(),
                decode_parameters_sha256: result.decode_parameters_sha256.clone(),
            },
        )
        .await
    }

    pub async fn complete_run(
        pool: &SqlitePool,
        run_id: &str,
        segments: &[CandidateSegmentInput],
        completion: MossCompletion,
    ) -> Result<MossRunRecord, MossStoreError> {
        Self::complete_run_internal(pool, run_id, segments, completion, None, None, None).await
    }

    pub async fn complete_run_with_provenance(
        pool: &SqlitePool,
        run_id: &str,
        segments: &[CandidateSegmentInput],
        completion: MossCompletion,
        alignments: &[CandidateAlignmentInput],
        diagnostics: MossRunDiagnosticsInput,
    ) -> Result<MossRunRecord, MossStoreError> {
        Self::complete_run_internal(
            pool,
            run_id,
            segments,
            completion,
            Some(alignments),
            Some(diagnostics),
            None,
        )
        .await
    }

    pub async fn complete_run_with_r5_provenance(
        pool: &SqlitePool,
        run_id: &str,
        segments: &[CandidateSegmentInput],
        completion: MossCompletion,
        alignments: &[CandidateAlignmentInput],
        diagnostics: MossRunDiagnosticsInput,
        audio_tokens: AudioTokenAlignmentInput,
    ) -> Result<MossRunRecord, MossStoreError> {
        Self::complete_run_internal(
            pool,
            run_id,
            segments,
            completion,
            Some(alignments),
            Some(diagnostics),
            Some(audio_tokens),
        )
        .await
    }

    async fn complete_run_internal(
        pool: &SqlitePool,
        run_id: &str,
        segments: &[CandidateSegmentInput],
        completion: MossCompletion,
        alignments: Option<&[CandidateAlignmentInput]>,
        diagnostics: Option<MossRunDiagnosticsInput>,
        audio_tokens: Option<AudioTokenAlignmentInput>,
    ) -> Result<MossRunRecord, MossStoreError> {
        let prepared = validate_candidate_segments(segments)?;
        let prepared_alignments = alignments
            .map(|values| validate_candidate_alignments(values, &prepared))
            .transpose()?;
        let prepared_diagnostics = diagnostics
            .map(|value| validate_run_diagnostics(value, prepared.len()))
            .transpose()?;
        let prepared_audio_tokens = audio_tokens
            .map(|value| {
                validate_audio_token_alignment(value, &prepared, prepared_alignments.as_deref())
            })
            .transpose()?;
        if prepared_alignments.is_some() != prepared_diagnostics.is_some() {
            return Err(MossStoreError::InvalidInput("alignment_diagnostics"));
        }
        if let (Some(alignments), Some(diagnostics)) =
            (prepared_alignments.as_ref(), prepared_diagnostics.as_ref())
        {
            let aligned_segment_count = alignments
                .iter()
                .filter(|value| value.alignment_method != "moss_segment")
                .count();
            let fallback_segment_count = alignments.len().saturating_sub(aligned_segment_count);
            let referenced_anchor_count = alignments
                .iter()
                .filter(|value| value.alignment_method == "source_transcript_segment")
                .flat_map(|value| value.source_anchor_ids.iter())
                .collect::<std::collections::BTreeSet<_>>()
                .len();
            let source_aligned_segment_count = alignments
                .iter()
                .filter(|value| value.alignment_method == "source_transcript_segment")
                .count();
            if usize::try_from(diagnostics.aligned_segment_count).ok()
                != Some(aligned_segment_count)
                || usize::try_from(diagnostics.fallback_segment_count).ok()
                    != Some(fallback_segment_count)
                || referenced_anchor_count
                    > usize::try_from(diagnostics.source_anchor_count).unwrap_or(usize::MAX)
                || (!diagnostics.source_hash_verified && source_aligned_segment_count > 0)
            {
                return Err(MossStoreError::InvalidInput("alignment_diagnostics"));
            }
        }
        let candidate_sha256 = sha256_json(&prepared)?;
        let raw_output_sha256 = normalized_sha256(&completion.raw_output_sha256)?;
        let clean_output_sha256 = normalized_sha256(&completion.clean_output_sha256)?;
        let device_name = required(completion.device_name, "device_name")?;
        let language_requested = required(completion.language_requested, "language_requested")?;
        let language_resolved = required(completion.language_resolved, "language_resolved")?;
        let decode_parameters_json =
            required(completion.decode_parameters_json, "decode_parameters_json")?;
        let decode_parameters_sha256 = normalized_sha256(&completion.decode_parameters_sha256)?;
        if language_requested != moss_helper::native::MOSS_LANGUAGE_REQUESTED
            || language_resolved != moss_helper::native::MOSS_LANGUAGE_RESOLVED
            || decode_parameters_json != moss_helper::native::MOSS_DECODE_PARAMETERS_JSON
            || !decode_parameters_sha256
                .eq_ignore_ascii_case(&moss_helper::native::moss_decode_parameters_sha256())
        {
            return Err(MossStoreError::InvalidInput("native_decode_contract"));
        }
        if !completion.wall_rtf.is_finite() || completion.wall_rtf < 0.0 {
            return Err(MossStoreError::InvalidInput("wall_rtf"));
        }
        let wall_elapsed_ms = i64::try_from(completion.wall_elapsed_ms)
            .map_err(|_| MossStoreError::InvalidInput("wall_elapsed_ms"))?;
        let peak_memory_bytes = i64::try_from(completion.peak_memory_bytes)
            .map_err(|_| MossStoreError::InvalidInput("peak_memory_bytes"))?;
        let mut tx = pool.begin().await?;
        let result = async {
            lock_run(&mut tx, run_id).await?;
            let run = get_run_in_transaction(&mut tx, run_id).await?;
            if run.status != "running" {
                return Err(MossStoreError::InvalidRunState);
            }
            if run.language_requested.as_deref() != Some(language_requested.as_str())
                || run.decode_parameters_json.as_deref() != Some(decode_parameters_json.as_str())
                || !run
                    .decode_parameters_sha256
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case(&decode_parameters_sha256))
            {
                return Err(MossStoreError::InvalidInput("native_decode_contract"));
            }
            if let Some(value) = prepared_diagnostics.as_ref() {
                if value.source_expected_sha256 != run.source_transcript_sha256 {
                    return Err(MossStoreError::InvalidInput("source_expected_sha256"));
                }
            }
            let now = timestamp();
            if let Some(audio_tokens) = prepared_audio_tokens.as_ref() {
                if let Some(track) = audio_tokens.track.as_ref() {
                    if track.audio_sha256 != run.audio_sha256 {
                        return Err(MossStoreError::InvalidInput("audio_token_audio_sha256"));
                    }
                    sqlx::query(
                        r#"
                        INSERT INTO moss_audio_token_runs (
                            run_id, status, audio_sha256, audio_duration_ms,
                            model_name, model_sha256, program_sha256,
                            parameters_sha256, token_track_sha256, backend,
                            parameters_json, global_match_coverage,
                            token_aligned_segment_count, fallback_raw_segment_count,
                            fallback_reason, created_at
                        ) VALUES (?, 'verified', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                        "#,
                    )
                    .bind(run_id)
                    .bind(&track.audio_sha256)
                    .bind(track.audio_duration_ms)
                    .bind(&track.model_name)
                    .bind(&track.model_sha256)
                    .bind(&track.program_sha256)
                    .bind(&track.parameters_sha256)
                    .bind(&track.token_track_sha256)
                    .bind(&track.backend)
                    .bind(serde_json::to_string(&track.parameters)?)
                    .bind(audio_tokens.global_match_coverage)
                    .bind(i64::from(audio_tokens.token_aligned_segment_count))
                    .bind(i64::from(audio_tokens.fallback_raw_segment_count))
                    .bind(&audio_tokens.fallback_reason)
                    .bind(&now)
                    .execute(&mut *tx)
                    .await?;
                    for chunk in &track.source_chunks {
                        sqlx::query(
                            r#"
                            INSERT INTO moss_audio_token_source_chunks (
                                run_id, chunk_index, start_ms, end_ms, sample_count,
                                text_sha256, first_global_token_index, token_count
                            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                            "#,
                        )
                        .bind(run_id)
                        .bind(i64::from(chunk.chunk_index))
                        .bind(chunk.start_ms)
                        .bind(chunk.end_ms)
                        .bind(i64::try_from(chunk.sample_count).map_err(|_| {
                            MossStoreError::InvalidInput("audio_token_sample_count")
                        })?)
                        .bind(&chunk.text_sha256)
                        .bind(i64::from(chunk.first_global_token_index))
                        .bind(i64::from(chunk.token_count))
                        .execute(&mut *tx)
                        .await?;
                    }
                    for token in &track.tokens {
                        sqlx::query(
                            r#"
                            INSERT INTO moss_audio_tokens (
                                run_id, global_token_index, chunk_index,
                                whisper_segment_index, whisper_token_index,
                                start_ms, end_ms, token_text, probability
                            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                            "#,
                        )
                        .bind(run_id)
                        .bind(i64::from(token.global_token_index))
                        .bind(i64::from(token.chunk_index))
                        .bind(i64::from(token.whisper_segment_index))
                        .bind(i64::from(token.whisper_token_index))
                        .bind(token.start_ms)
                        .bind(token.end_ms)
                        .bind(&token.text)
                        .bind(f64::from(token.probability))
                        .execute(&mut *tx)
                        .await?;
                    }
                } else {
                    sqlx::query(
                        r#"
                        INSERT INTO moss_audio_token_runs (
                            run_id, status, audio_sha256, global_match_coverage,
                            token_aligned_segment_count, fallback_raw_segment_count,
                            fallback_reason, created_at
                        ) VALUES (?, 'fallback', ?, NULL, 0, ?, ?, ?)
                        "#,
                    )
                    .bind(run_id)
                    .bind(&run.audio_sha256)
                    .bind(i64::from(audio_tokens.fallback_raw_segment_count))
                    .bind(&audio_tokens.fallback_reason)
                    .bind(&now)
                    .execute(&mut *tx)
                    .await?;
                }
            }
            for (position, segment) in prepared.iter().enumerate() {
                let segment_id = format!("moss-segment-{}-{:06}", run_id, segment.segment_index);
                sqlx::query(
                    r#"
                    INSERT INTO moss_candidate_segments (
                        segment_id, run_id, segment_index, start_ms, end_ms,
                        speaker_label, raw_text, created_at
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                    "#,
                )
                .bind(&segment_id)
                .bind(run_id)
                .bind(i64::from(segment.segment_index))
                .bind(segment.start_ms)
                .bind(segment.end_ms)
                .bind(&segment.speaker_label)
                .bind(&segment.text)
                .bind(&now)
                .execute(&mut *tx)
                .await?;
                if let (Some(alignments), Some(diagnostics)) =
                    (prepared_alignments.as_ref(), prepared_diagnostics.as_ref())
                {
                    let alignment = &alignments[position];
                    sqlx::query(
                        r#"
                        INSERT INTO moss_candidate_segment_alignment (
                            segment_id, raw_segment_index, raw_start_ms, raw_end_ms,
                            raw_text_sha256, alignment_method, confidence,
                            source_anchor_ids_json, source_transcript_sha256, created_at
                        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                        "#,
                    )
                    .bind(&segment_id)
                    .bind(i64::from(alignment.raw_segment_index))
                    .bind(alignment.raw_start_ms)
                    .bind(alignment.raw_end_ms)
                    .bind(&alignment.raw_text_sha256)
                    .bind(&alignment.alignment_method)
                    .bind(alignment.confidence)
                    .bind(serde_json::to_string(&alignment.source_anchor_ids)?)
                    .bind(diagnostics.source_expected_sha256.as_str())
                    .bind(&now)
                    .execute(&mut *tx)
                    .await?;
                }
                if let Some(audio_tokens) = prepared_audio_tokens.as_ref() {
                    if let Some(boundary) = audio_tokens
                        .boundaries
                        .iter()
                        .find(|boundary| boundary.segment_index == segment.segment_index)
                    {
                        sqlx::query(
                            r#"
                            INSERT INTO moss_candidate_audio_token_boundary (
                                segment_id, run_id, first_token_index,
                                last_token_index, token_track_sha256,
                                confidence, created_at
                            ) VALUES (?, ?, ?, ?, ?, ?, ?)
                            "#,
                        )
                        .bind(&segment_id)
                        .bind(run_id)
                        .bind(i64::from(boundary.first_token_index))
                        .bind(i64::from(boundary.last_token_index))
                        .bind(&boundary.token_track_sha256)
                        .bind(boundary.confidence)
                        .bind(&now)
                        .execute(&mut *tx)
                        .await?;
                    }
                }
            }
            if let Some(diagnostics) = prepared_diagnostics.as_ref() {
                let last_active_ms = diagnostics
                    .last_active_ms
                    .map(i64::try_from)
                    .transpose()
                    .map_err(|_| MossStoreError::InvalidInput("last_active_ms"))?;
                let first_active_ms = diagnostics
                    .first_active_ms
                    .map(i64::try_from)
                    .transpose()
                    .map_err(|_| MossStoreError::InvalidInput("first_active_ms"))?;
                let tail_delta_ms =
                    last_active_ms.map(|value| diagnostics.model_last_timestamp_ms - value);
                sqlx::query(
                    r#"
                    INSERT INTO moss_run_diagnostics (
                        run_id, audio_duration_ms, activity_frame_ms,
                        activity_threshold_dbfs, first_active_ms, last_active_ms,
                        model_last_timestamp_ms, tail_delta_ms, aligned_segment_count,
                        fallback_segment_count, source_anchor_count, source_hash_verified,
                        source_expected_sha256, source_actual_sha256, fallback_reason, created_at
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    "#,
                )
                .bind(run_id)
                .bind(
                    i64::try_from(diagnostics.audio_duration_ms)
                        .map_err(|_| MossStoreError::InvalidInput("audio_duration_ms"))?,
                )
                .bind(i64::from(diagnostics.activity_frame_ms))
                .bind(diagnostics.activity_threshold_dbfs)
                .bind(first_active_ms)
                .bind(last_active_ms)
                .bind(diagnostics.model_last_timestamp_ms)
                .bind(tail_delta_ms)
                .bind(i64::from(diagnostics.aligned_segment_count))
                .bind(i64::from(diagnostics.fallback_segment_count))
                .bind(i64::from(diagnostics.source_anchor_count))
                .bind(diagnostics.source_hash_verified)
                .bind(&diagnostics.source_expected_sha256)
                .bind(&diagnostics.source_actual_sha256)
                .bind(&diagnostics.fallback_reason)
                .bind(&now)
                .execute(&mut *tx)
                .await?;
            }
            let updated = sqlx::query(
                r#"
                UPDATE moss_transcription_runs
                SET status = 'completed', updated_at = ?, completed_at = ?,
                    device_name = ?, raw_output_sha256 = ?,
                    clean_output_sha256 = ?, candidate_sha256 = ?,
                    segment_count = ?, wall_elapsed_ms = ?, wall_rtf = ?,
                    peak_memory_bytes = ?, error_code = NULL,
                    language_requested = ?, language_resolved = ?,
                    decode_parameters_json = ?, decode_parameters_sha256 = ?
                WHERE run_id = ? AND status = 'running'
                "#,
            )
            .bind(&now)
            .bind(&now)
            .bind(&device_name)
            .bind(&raw_output_sha256)
            .bind(&clean_output_sha256)
            .bind(&candidate_sha256)
            .bind(i64::try_from(prepared.len()).unwrap_or(i64::MAX))
            .bind(wall_elapsed_ms)
            .bind(completion.wall_rtf)
            .bind(peak_memory_bytes)
            .bind(&language_requested)
            .bind(&language_resolved)
            .bind(&decode_parameters_json)
            .bind(&decode_parameters_sha256)
            .bind(run_id)
            .execute(&mut *tx)
            .await?;
            if updated.rows_affected() != 1 {
                return Err(MossStoreError::InvalidRunState);
            }
            get_run_in_transaction(&mut tx, run_id).await
        }
        .await;
        finish_transaction(tx, result).await
    }

    pub async fn run_diagnostics(
        pool: &SqlitePool,
        run_id: &str,
    ) -> Result<Option<MossRunDiagnosticsRecord>, MossStoreError> {
        sqlx::query_as::<_, MossRunDiagnosticsRecord>(
            r#"
            SELECT run_id, audio_duration_ms, activity_frame_ms,
                   activity_threshold_dbfs, first_active_ms, last_active_ms,
                   model_last_timestamp_ms, tail_delta_ms, aligned_segment_count,
                   fallback_segment_count, source_anchor_count, source_hash_verified,
                   source_expected_sha256, source_actual_sha256, fallback_reason, created_at
              FROM moss_run_diagnostics WHERE run_id = ?
            "#,
        )
        .bind(run_id)
        .fetch_optional(pool)
        .await
        .map_err(Into::into)
    }

    pub async fn audio_token_alignment(
        pool: &SqlitePool,
        run_id: &str,
    ) -> Result<Option<AudioTokenAlignmentRecord>, MossStoreError> {
        sqlx::query_as::<_, AudioTokenAlignmentRecord>(
            r#"
            SELECT run_id, status, audio_sha256, audio_duration_ms,
                   model_name, model_sha256, program_sha256,
                   parameters_sha256, token_track_sha256, backend,
                   global_match_coverage, token_aligned_segment_count,
                   fallback_raw_segment_count, fallback_reason, created_at
              FROM moss_audio_token_runs
             WHERE run_id = ?
            "#,
        )
        .bind(run_id)
        .fetch_optional(pool)
        .await
        .map_err(Into::into)
    }

    pub async fn mark_run_failed(
        pool: &SqlitePool,
        run_id: &str,
        error_code: &str,
    ) -> Result<MossRunRecord, MossStoreError> {
        Self::mark_run_terminal(pool, run_id, "failed", error_code).await
    }

    pub async fn mark_run_cancelled(
        pool: &SqlitePool,
        run_id: &str,
    ) -> Result<MossRunRecord, MossStoreError> {
        Self::mark_run_terminal(pool, run_id, "cancelled", "MOSS_CANCELLED").await
    }

    async fn mark_run_terminal(
        pool: &SqlitePool,
        run_id: &str,
        status: &str,
        error_code: &str,
    ) -> Result<MossRunRecord, MossStoreError> {
        let error_code = fixed_code(error_code)?;
        let now = timestamp();
        let updated = sqlx::query(
            r#"
            UPDATE moss_transcription_runs
            SET status = ?, updated_at = ?, completed_at = ?, error_code = ?
            WHERE run_id = ? AND status = 'running'
            "#,
        )
        .bind(status)
        .bind(&now)
        .bind(&now)
        .bind(&error_code)
        .bind(run_id)
        .execute(pool)
        .await?;
        if updated.rows_affected() != 1 {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM moss_transcription_runs WHERE run_id = ?)",
            )
            .bind(run_id)
            .fetch_one(pool)
            .await?;
            return Err(if exists {
                MossStoreError::InvalidRunState
            } else {
                MossStoreError::RunNotFound
            });
        }
        Self::get_run(pool, run_id).await
    }

    pub async fn recover_interrupted_runs(
        pool: &SqlitePool,
    ) -> Result<InterruptedRunRecovery, MossStoreError> {
        let now = timestamp();
        let result = sqlx::query(
            r#"
            UPDATE moss_transcription_runs
            SET status = 'failed', updated_at = ?, completed_at = ?,
                error_code = ?
            WHERE status = 'running'
            "#,
        )
        .bind(&now)
        .bind(&now)
        .bind(MOSS_INTERRUPTED_BY_RESTART)
        .execute(pool)
        .await?;
        Ok(InterruptedRunRecovery {
            runs_failed: result.rows_affected(),
        })
    }

    pub async fn add_term_correction(
        pool: &SqlitePool,
        segment_id: &str,
        input: TermCorrectionInput,
    ) -> Result<TermCorrectionRecord, MossStoreError> {
        Self::add_term_correction_internal(pool, segment_id, input, None).await
    }

    pub async fn add_machine_term_correction(
        pool: &SqlitePool,
        segment_id: &str,
        source: MachineTermSuggestion,
    ) -> Result<TermCorrectionRecord, MossStoreError> {
        let input = TermCorrectionInput {
            start_char: source.start_char,
            end_char: source.end_char,
            original_text: source.original_text.clone(),
            replacement_text: source.replacement_text.clone(),
            rule_id: format!("R5_AUDIO_TOKEN_CONTEXT:{}", source.term_id),
            expected_context_sha256: source.context_sha256.clone(),
        };
        Self::add_term_correction_internal(pool, segment_id, input, Some(source)).await
    }

    async fn add_term_correction_internal(
        pool: &SqlitePool,
        segment_id: &str,
        input: TermCorrectionInput,
        machine_source: Option<MachineTermSuggestion>,
    ) -> Result<TermCorrectionRecord, MossStoreError> {
        let original_text = required(input.original_text, "original_text")?;
        let replacement_text = required(input.replacement_text, "replacement_text")?;
        let rule_id = required(input.rule_id, "rule_id")?;
        let expected_context_sha256 = normalized_sha256(&input.expected_context_sha256)?;
        let start_char = input.start_char;
        let end_char = input.end_char;
        let mut tx = pool.begin().await?;
        let result = async {
            let segment = lock_and_load_segment(&mut tx, segment_id).await?;
            if segment.run_status != "completed" {
                return Err(MossStoreError::InvalidRunState);
            }
            if segment.context_sha256 != expected_context_sha256 {
                return Err(MossStoreError::InvalidInput("context_sha256"));
            }
            let machine_binding = if let Some(source) = machine_source.as_ref() {
                let term_id = required(source.term_id.clone(), "term_id")?;
                let token_track_sha256 = normalized_sha256(&source.token_track_sha256)?;
                let model_sha256 = normalized_sha256(&source.model_sha256)?;
                if i64::from(source.segment_index) != segment.segment_index
                    || source.original_text != original_text
                    || source.replacement_text != replacement_text
                    || source.context_sha256 != expected_context_sha256
                    || source.last_token_index < source.first_token_index
                    || !source.confidence.is_finite()
                    || !(0.0..=1.0).contains(&source.confidence)
                {
                    return Err(MossStoreError::InvalidInput("machine_term_source"));
                }
                let token_binding_count: i64 = sqlx::query_scalar(
                    r#"
                    SELECT COUNT(*)
                      FROM moss_candidate_audio_token_boundary b
                      JOIN moss_audio_token_runs a ON a.run_id = b.run_id
                      JOIN moss_audio_tokens first_token
                        ON first_token.run_id = b.run_id
                       AND first_token.global_token_index = ?
                      JOIN moss_audio_tokens last_token
                        ON last_token.run_id = b.run_id
                       AND last_token.global_token_index = ?
                     WHERE b.segment_id = ?
                       AND b.run_id = ?
                       AND b.first_token_index <= ?
                       AND b.last_token_index >= ?
                       AND b.token_track_sha256 = ?
                       AND a.token_track_sha256 = ?
                       AND a.model_sha256 = ?
                    "#,
                )
                .bind(i64::from(source.first_token_index))
                .bind(i64::from(source.last_token_index))
                .bind(segment_id)
                .bind(&segment.run_id)
                .bind(i64::from(source.first_token_index))
                .bind(i64::from(source.last_token_index))
                .bind(&token_track_sha256)
                .bind(&token_track_sha256)
                .bind(&model_sha256)
                .fetch_one(&mut *tx)
                .await?;
                if token_binding_count != 1 {
                    return Err(MossStoreError::InvalidInput("machine_term_tokens"));
                }
                let (token_count, minimum_probability): (i64, Option<f64>) = sqlx::query_as(
                    r#"
                    SELECT COUNT(*), MIN(probability)
                      FROM moss_audio_tokens
                     WHERE run_id = ?
                       AND global_token_index BETWEEN ? AND ?
                    "#,
                )
                .bind(&segment.run_id)
                .bind(i64::from(source.first_token_index))
                .bind(i64::from(source.last_token_index))
                .fetch_one(&mut *tx)
                .await?;
                let expected_token_count = source
                    .last_token_index
                    .checked_sub(source.first_token_index)
                    .and_then(|value| value.checked_add(1))
                    .map(i64::from)
                    .ok_or(MossStoreError::InvalidInput("machine_term_tokens"))?;
                if token_count != expected_token_count
                    || minimum_probability.is_none_or(|probability| {
                        (probability - source.confidence).abs() > 1e-6
                    })
                {
                    return Err(MossStoreError::InvalidInput("machine_term_confidence"));
                }
                Some((term_id, token_track_sha256, model_sha256))
            } else {
                None
            };
            let latest: Option<(i64, String)> = sqlx::query_as(
                r#"
                SELECT revision, result_text
                FROM moss_term_corrections
                WHERE segment_id = ? AND reverted_at IS NULL
                ORDER BY revision DESC
                LIMIT 1
                "#,
            )
            .bind(segment_id)
            .fetch_optional(&mut *tx)
            .await?;
            let base_text = latest
                .as_ref()
                .map(|(_, text)| text.as_str())
                .unwrap_or(segment.raw_text.as_str());
            let result_text = replace_char_range(
                base_text,
                start_char,
                end_char,
                &original_text,
                &replacement_text,
            )?;
            let revision: i64 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(revision), 0) + 1 FROM moss_term_corrections WHERE segment_id = ?",
            )
            .bind(segment_id)
            .fetch_one(&mut *tx)
            .await?;
            let correction_id = format!("moss-correction-{}", Uuid::new_v4());
            let now = timestamp();
            sqlx::query(
                r#"
                INSERT INTO moss_term_corrections (
                    correction_id, segment_id, revision, original_text,
                    replacement_text, result_text, start_char, end_char,
                    rule_id, context_sha256, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&correction_id)
            .bind(segment_id)
            .bind(revision)
            .bind(&original_text)
            .bind(&replacement_text)
            .bind(&result_text)
            .bind(i64::try_from(start_char).map_err(|_| MossStoreError::InvalidInput("start_char"))?)
            .bind(i64::try_from(end_char).map_err(|_| MossStoreError::InvalidInput("end_char"))?)
            .bind(&rule_id)
            .bind(&expected_context_sha256)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
            if let (Some(source), Some((term_id, token_track_sha256, model_sha256))) =
                (machine_source.as_ref(), machine_binding.as_ref())
            {
                sqlx::query(
                    r#"
                    INSERT INTO moss_machine_term_correction_source (
                        correction_id, run_id, term_id, context_sha256,
                        token_track_sha256, model_sha256, first_token_index,
                        last_token_index, confidence, created_at
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    "#,
                )
                .bind(&correction_id)
                .bind(&segment.run_id)
                .bind(term_id)
                .bind(&expected_context_sha256)
                .bind(token_track_sha256)
                .bind(model_sha256)
                .bind(i64::from(source.first_token_index))
                .bind(i64::from(source.last_token_index))
                .bind(source.confidence)
                .bind(&now)
                .execute(&mut *tx)
                .await?;
            }
            sqlx::query_as::<_, TermCorrectionRecord>(
                r#"
                SELECT correction_id, segment_id, revision, original_text,
                       replacement_text, result_text, start_char, end_char,
                       rule_id, context_sha256, created_at, reverted_at
                FROM moss_term_corrections WHERE correction_id = ?
                "#,
            )
            .bind(correction_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(Into::into)
        }
        .await;
        finish_transaction(tx, result).await
    }

    pub async fn revert_latest_term_correction(
        pool: &SqlitePool,
        correction_id: &str,
    ) -> Result<(), MossStoreError> {
        let mut tx = pool.begin().await?;
        let result = async {
            let row: Option<(String,)> = sqlx::query_as(
                "SELECT segment_id FROM moss_term_corrections WHERE correction_id = ? AND reverted_at IS NULL",
            )
            .bind(correction_id)
            .fetch_optional(&mut *tx)
            .await?;
            let (segment_id,) = row.ok_or(MossStoreError::CandidateNotFound)?;
            let _ = lock_and_load_segment(&mut tx, &segment_id).await?;
            let latest: String = sqlx::query_scalar(
                r#"
                SELECT correction_id FROM moss_term_corrections
                WHERE segment_id = ? AND reverted_at IS NULL
                ORDER BY revision DESC LIMIT 1
                "#,
            )
            .bind(&segment_id)
            .fetch_one(&mut *tx)
            .await?;
            if latest != correction_id {
                return Err(MossStoreError::InvalidInput("correction_not_latest"));
            }
            sqlx::query(
                "UPDATE moss_term_corrections SET reverted_at = ? WHERE correction_id = ?",
            )
            .bind(timestamp())
            .bind(correction_id)
            .execute(&mut *tx)
            .await?;
            Ok(())
        }
        .await;
        finish_transaction(tx, result).await
    }

    pub async fn bind_speaker(
        pool: &SqlitePool,
        run_id: &str,
        speaker_label: &str,
        person: BoundPerson,
        expected_context_sha256: &str,
    ) -> Result<String, MossStoreError> {
        validate_speaker_label(speaker_label)?;
        let person_id = required(person.person_id, "person_id")?;
        let display_name = required(person.display_name, "person_display_name")?;
        let expected_context_sha256 = normalized_sha256(expected_context_sha256)?;
        let mut tx = pool.begin().await?;
        let result = async {
            lock_run(&mut tx, run_id).await?;
            let run = get_run_in_transaction(&mut tx, run_id).await?;
            if run.status != "completed" {
                return Err(MossStoreError::InvalidRunState);
            }
            if run.context_sha256 != expected_context_sha256 {
                return Err(MossStoreError::InvalidInput("context_sha256"));
            }
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM moss_candidate_segments WHERE run_id = ? AND speaker_label = ?)",
            )
            .bind(run_id)
            .bind(speaker_label)
            .fetch_one(&mut *tx)
            .await?;
            if !exists {
                return Err(MossStoreError::CandidateNotFound);
            }
            let now = timestamp();
            sqlx::query(
                r#"
                UPDATE moss_speaker_bindings SET revoked_at = ?
                WHERE run_id = ? AND speaker_label = ? AND revoked_at IS NULL
                "#,
            )
            .bind(&now)
            .bind(run_id)
            .bind(speaker_label)
            .execute(&mut *tx)
            .await?;
            let binding_id = format!("moss-binding-{}", Uuid::new_v4());
            sqlx::query(
                r#"
                INSERT INTO moss_speaker_bindings (
                    binding_id, run_id, speaker_label, person_id,
                    person_display_name, context_sha256, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&binding_id)
            .bind(run_id)
            .bind(speaker_label)
            .bind(person_id)
            .bind(display_name)
            .bind(expected_context_sha256)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            Ok(binding_id)
        }
        .await;
        finish_transaction(tx, result).await
    }

    pub async fn set_segment_override(
        pool: &SqlitePool,
        segment_id: &str,
        input: SegmentOverrideInput,
    ) -> Result<String, MossStoreError> {
        let replacement_text = input
            .replacement_text
            .map(|text| required(text, "replacement_text"))
            .transpose()?;
        let person = input
            .person
            .map(|person| {
                Ok::<_, MossStoreError>(BoundPerson {
                    person_id: required(person.person_id, "person_id")?,
                    display_name: required(person.display_name, "person_display_name")?,
                })
            })
            .transpose()?;
        if replacement_text.is_none() && person.is_none() {
            return Err(MossStoreError::InvalidInput("segment_override"));
        }
        let reason_code = fixed_code(&input.reason_code)?;
        let expected_context_sha256 = normalized_sha256(&input.expected_context_sha256)?;
        let mut tx = pool.begin().await?;
        let result = async {
            let segment = lock_and_load_segment(&mut tx, segment_id).await?;
            if segment.run_status != "completed" {
                return Err(MossStoreError::InvalidRunState);
            }
            if segment.context_sha256 != expected_context_sha256 {
                return Err(MossStoreError::InvalidInput("context_sha256"));
            }
            let revision: i64 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(revision), 0) + 1 FROM moss_segment_overrides WHERE segment_id = ?",
            )
            .bind(segment_id)
            .fetch_one(&mut *tx)
            .await?;
            let now = timestamp();
            sqlx::query(
                "UPDATE moss_segment_overrides SET revoked_at = ? WHERE segment_id = ? AND revoked_at IS NULL",
            )
            .bind(&now)
            .bind(segment_id)
            .execute(&mut *tx)
            .await?;
            let override_id = format!("moss-override-{}", Uuid::new_v4());
            sqlx::query(
                r#"
                INSERT INTO moss_segment_overrides (
                    override_id, segment_id, revision, replacement_text,
                    person_id, person_display_name, context_sha256,
                    reason_code, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&override_id)
            .bind(segment_id)
            .bind(revision)
            .bind(replacement_text)
            .bind(person.as_ref().map(|value| value.person_id.as_str()))
            .bind(person.as_ref().map(|value| value.display_name.as_str()))
            .bind(expected_context_sha256)
            .bind(reason_code)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            Ok(override_id)
        }
        .await;
        finish_transaction(tx, result).await
    }

    pub async fn revoke_segment_override(
        pool: &SqlitePool,
        override_id: &str,
    ) -> Result<(), MossStoreError> {
        let updated = sqlx::query(
            "UPDATE moss_segment_overrides SET revoked_at = ? WHERE override_id = ? AND revoked_at IS NULL",
        )
        .bind(timestamp())
        .bind(override_id)
        .execute(pool)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(MossStoreError::CandidateNotFound);
        }
        Ok(())
    }

    pub async fn activate_candidate(
        pool: &SqlitePool,
        run_id: &str,
    ) -> Result<ActivationResult, MossStoreError> {
        Self::activate_candidate_internal(pool, run_id, MutationFault::None).await
    }

    async fn activate_candidate_internal(
        pool: &SqlitePool,
        run_id: &str,
        fault: MutationFault,
    ) -> Result<ActivationResult, MossStoreError> {
        let mut tx = pool.begin().await?;
        let result = async {
            // This no-op write obtains SQLite's writer reservation before the
            // activation hash is read. A concurrent transcript edit therefore
            // commits before this hash (and is detected) or after activation.
            lock_run(&mut tx, run_id).await?;
            let run = get_run_in_transaction(&mut tx, run_id).await?;
            if run.status != "completed" {
                return Err(MossStoreError::InvalidRunState);
            }
            let candidate_sha256 = run
                .candidate_sha256
                .clone()
                .ok_or(MossStoreError::CandidateNotFound)?;
            let already_active: bool = sqlx::query_scalar(
                r#"
                SELECT EXISTS(
                    SELECT 1 FROM moss_activation_snapshots
                    WHERE meeting_id = ? AND status = 'active'
                )
                "#,
            )
            .bind(&run.meeting_id)
            .fetch_one(&mut *tx)
            .await?;
            if already_active {
                return Err(MossStoreError::ActiveActivationExists);
            }

            let (before, current_sha256) = transcript_snapshot(&mut *tx, &run.meeting_id).await?;
            if current_sha256 != run.source_transcript_sha256 {
                return Err(MossStoreError::TranscriptConflict {
                    expected: run.source_transcript_sha256,
                    actual: current_sha256,
                });
            }
            let candidate = materialize_candidate(&mut tx, run_id).await?;
            if candidate.is_empty()
                || i64::try_from(candidate.len()).ok() != Some(run.segment_count)
            {
                return Err(MossStoreError::CandidateNotFound);
            }

            let activation_id = format!("moss-activation-{}", Uuid::new_v4());
            let activated_at = timestamp();
            let activated_rows = candidate
                .iter()
                .map(|segment| StoredTranscript {
                    id: format!(
                        "moss-transcript-{}-{:06}",
                        activation_id, segment.segment_index
                    ),
                    meeting_id: run.meeting_id.clone(),
                    transcript: segment.text.clone(),
                    timestamp: activated_at.clone(),
                    summary: None,
                    action_items: None,
                    key_points: None,
                    audio_start_time: Some(segment.start_ms as f64 / 1_000.0),
                    audio_end_time: Some(segment.end_ms as f64 / 1_000.0),
                    duration: Some((segment.end_ms - segment.start_ms) as f64 / 1_000.0),
                    // Existing transcripts.speaker means microphone/system
                    // source. Anonymous MOSS/person identity lives only in the
                    // immutable activation segment below.
                    speaker: None,
                })
                .collect::<Vec<_>>();
            let activated_sha256 = sha256_json(&activated_rows)?;
            let before_json = serde_json::to_string(&before)?;
            let activated_json = serde_json::to_string(&activated_rows)?;

            sqlx::query(
                r#"
                INSERT INTO moss_activation_snapshots (
                    activation_id, meeting_id, run_id, status, activated_at,
                    pre_activation_transcript_sha256, activated_transcript_sha256,
                    candidate_sha256, pre_activation_transcripts_json,
                    activated_transcripts_json
                ) VALUES (?, ?, ?, 'active', ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&activation_id)
            .bind(&run.meeting_id)
            .bind(run_id)
            .bind(&activated_at)
            .bind(&current_sha256)
            .bind(&activated_sha256)
            .bind(&candidate_sha256)
            .bind(&before_json)
            .bind(&activated_json)
            .execute(&mut *tx)
            .await?;

            for segment in &candidate {
                let transcript_id = format!(
                    "moss-transcript-{}-{:06}",
                    activation_id, segment.segment_index
                );
                sqlx::query(
                    r#"
                    INSERT INTO moss_activation_segments (
                        activation_id, segment_index, transcript_id,
                        candidate_segment_id, start_ms, end_ms, speaker_label,
                        resolved_person_id, resolved_person_display_name, text,
                        correction_id, override_id, binding_id
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    "#,
                )
                .bind(&activation_id)
                .bind(segment.segment_index)
                .bind(transcript_id)
                .bind(&segment.candidate_segment_id)
                .bind(segment.start_ms)
                .bind(segment.end_ms)
                .bind(&segment.speaker_label)
                .bind(
                    segment
                        .resolved_person
                        .as_ref()
                        .map(|person| person.person_id.as_str()),
                )
                .bind(
                    segment
                        .resolved_person
                        .as_ref()
                        .map(|person| person.display_name.as_str()),
                )
                .bind(&segment.text)
                .bind(segment.correction_id.as_deref())
                .bind(segment.override_id.as_deref())
                .bind(segment.binding_id.as_deref())
                .execute(&mut *tx)
                .await?;
            }

            sqlx::query("DELETE FROM transcripts WHERE meeting_id = ?")
                .bind(&run.meeting_id)
                .execute(&mut *tx)
                .await?;
            if fault_is_after_delete(fault) {
                return Err(MossStoreError::InjectedFailure);
            }
            insert_transcripts(&mut tx, &activated_rows).await?;
            update_transcript_dependents(&mut tx, &run.meeting_id, &activated_rows, &activated_at)
                .await?;

            let (_, committed_hash) = transcript_snapshot(&mut *tx, &run.meeting_id).await?;
            if committed_hash != activated_sha256 {
                return Err(MossStoreError::InvalidInput("activated_transcript_hash"));
            }
            Ok(ActivationResult {
                activation_id,
                meeting_id: run.meeting_id,
                run_id: run_id.to_owned(),
                previous_transcript_sha256: current_sha256,
                activated_transcript_sha256: activated_sha256,
                segment_count: activated_rows.len(),
            })
        }
        .await;
        finish_transaction(tx, result).await
    }

    pub async fn rollback_active_activation(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<RollbackResult, MossStoreError> {
        Self::rollback_active_activation_internal(pool, meeting_id, MutationFault::None).await
    }

    async fn rollback_active_activation_internal(
        pool: &SqlitePool,
        meeting_id: &str,
        fault: MutationFault,
    ) -> Result<RollbackResult, MossStoreError> {
        let mut tx = pool.begin().await?;
        let result = async {
            let locked = sqlx::query(
                r#"
                UPDATE moss_activation_snapshots SET status = status
                WHERE meeting_id = ? AND status = 'active'
                "#,
            )
            .bind(meeting_id)
            .execute(&mut *tx)
            .await?;
            if locked.rows_affected() != 1 {
                return Err(MossStoreError::ActivationNotFound);
            }
            let activation: (String, String, String, String) = sqlx::query_as(
                r#"
                SELECT activation_id, pre_activation_transcript_sha256,
                       activated_transcript_sha256, pre_activation_transcripts_json
                FROM moss_activation_snapshots
                WHERE meeting_id = ? AND status = 'active'
                "#,
            )
            .bind(meeting_id)
            .fetch_one(&mut *tx)
            .await?;
            let (activation_id, previous_sha256, activated_sha256, previous_json) = activation;
            let (_, current_sha256) = transcript_snapshot(&mut *tx, meeting_id).await?;
            if current_sha256 != activated_sha256 {
                return Err(MossStoreError::RollbackConflict {
                    expected: activated_sha256,
                    actual: current_sha256,
                });
            }
            let previous: Vec<StoredTranscript> = serde_json::from_str(&previous_json)?;
            if previous
                .iter()
                .any(|transcript| transcript.meeting_id != meeting_id)
                || sha256_json(&previous)? != previous_sha256
            {
                return Err(MossStoreError::InvalidInput("activation_snapshot"));
            }

            sqlx::query("DELETE FROM transcripts WHERE meeting_id = ?")
                .bind(meeting_id)
                .execute(&mut *tx)
                .await?;
            if fault_is_after_delete(fault) {
                return Err(MossStoreError::InjectedFailure);
            }
            insert_transcripts(&mut tx, &previous).await?;
            let now = timestamp();
            update_transcript_dependents(&mut tx, meeting_id, &previous, &now).await?;
            let updated = sqlx::query(
                r#"
                UPDATE moss_activation_snapshots
                SET status = 'rolled_back', rolled_back_at = ?
                WHERE activation_id = ? AND status = 'active'
                "#,
            )
            .bind(&now)
            .bind(&activation_id)
            .execute(&mut *tx)
            .await?;
            if updated.rows_affected() != 1 {
                return Err(MossStoreError::ActivationNotFound);
            }
            let (_, restored_sha256) = transcript_snapshot(&mut *tx, meeting_id).await?;
            if restored_sha256 != previous_sha256 {
                return Err(MossStoreError::InvalidInput("rollback_transcript_hash"));
            }
            Ok(RollbackResult {
                activation_id,
                meeting_id: meeting_id.to_owned(),
                restored_transcript_sha256: restored_sha256,
                segment_count: previous.len(),
            })
        }
        .await;
        finish_transaction(tx, result).await
    }
}

async fn materialize_candidate(
    tx: &mut Transaction<'_, Sqlite>,
    run_id: &str,
) -> Result<Vec<MaterializedCandidateSegment>, MossStoreError> {
    let rows = sqlx::query_as::<_, CandidateSegmentRow>(
        r#"
        SELECT segment_id, segment_index, start_ms, end_ms, speaker_label, raw_text
        FROM moss_candidate_segments
        WHERE run_id = ?
        ORDER BY segment_index
        "#,
    )
    .bind(run_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut materialized = Vec::with_capacity(rows.len());
    for row in rows {
        let correction: Option<(String, String)> = sqlx::query_as(
            r#"
            SELECT correction_id, result_text
            FROM moss_term_corrections
            WHERE segment_id = ? AND reverted_at IS NULL
            ORDER BY revision DESC LIMIT 1
            "#,
        )
        .bind(&row.segment_id)
        .fetch_optional(&mut **tx)
        .await?;
        let segment_override: Option<SegmentOverrideRow> = sqlx::query_as(
            r#"
                SELECT override_id, replacement_text, person_id, person_display_name
                FROM moss_segment_overrides
                WHERE segment_id = ? AND revoked_at IS NULL
                "#,
        )
        .bind(&row.segment_id)
        .fetch_optional(&mut **tx)
        .await?;
        let binding: Option<(String, String, String)> = sqlx::query_as(
            r#"
            SELECT binding_id, person_id, person_display_name
            FROM moss_speaker_bindings
            WHERE run_id = ? AND speaker_label = ? AND revoked_at IS NULL
            "#,
        )
        .bind(run_id)
        .bind(&row.speaker_label)
        .fetch_optional(&mut **tx)
        .await?;

        let text = segment_override
            .as_ref()
            .and_then(|value| value.replacement_text.clone())
            .or_else(|| correction.as_ref().map(|(_, text)| text.clone()))
            .unwrap_or_else(|| row.raw_text.clone());
        if text.trim().is_empty() {
            return Err(MossStoreError::InvalidInput("materialized_text"));
        }
        let override_person = segment_override.as_ref().and_then(|value| {
            value
                .person_id
                .as_ref()
                .zip(value.person_display_name.as_ref())
                .map(|(person_id, display_name)| BoundPerson {
                    person_id: person_id.clone(),
                    display_name: display_name.clone(),
                })
        });
        let binding_person = binding
            .as_ref()
            .map(|(_, person_id, display_name)| BoundPerson {
                person_id: person_id.clone(),
                display_name: display_name.clone(),
            });
        materialized.push(MaterializedCandidateSegment {
            candidate_segment_id: row.segment_id,
            segment_index: row.segment_index,
            start_ms: row.start_ms,
            end_ms: row.end_ms,
            speaker_label: row.speaker_label,
            text,
            resolved_person: override_person.or(binding_person),
            correction_id: correction.map(|(id, _)| id),
            override_id: segment_override.map(|value| value.override_id),
            binding_id: binding.map(|(id, _, _)| id),
        });
    }
    Ok(materialized)
}

async fn insert_transcripts(
    tx: &mut Transaction<'_, Sqlite>,
    transcripts: &[StoredTranscript],
) -> Result<(), MossStoreError> {
    for transcript in transcripts {
        sqlx::query(
            r#"
            INSERT INTO transcripts (
                id, meeting_id, transcript, timestamp, summary, action_items,
                key_points, audio_start_time, audio_end_time, duration, speaker
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&transcript.id)
        .bind(&transcript.meeting_id)
        .bind(&transcript.transcript)
        .bind(&transcript.timestamp)
        .bind(&transcript.summary)
        .bind(&transcript.action_items)
        .bind(&transcript.key_points)
        .bind(transcript.audio_start_time)
        .bind(transcript.audio_end_time)
        .bind(transcript.duration)
        .bind(&transcript.speaker)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn update_transcript_dependents(
    tx: &mut Transaction<'_, Sqlite>,
    meeting_id: &str,
    transcripts: &[StoredTranscript],
    updated_at: &str,
) -> Result<(), MossStoreError> {
    let source_text = transcripts
        .iter()
        .map(|transcript| transcript.transcript.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    sqlx::query("UPDATE transcript_chunks SET transcript_text = ? WHERE meeting_id = ?")
        .bind(source_text)
        .bind(meeting_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE meetings SET updated_at = ? WHERE id = ?")
        .bind(updated_at)
        .bind(meeting_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn fault_is_after_delete(fault: MutationFault) -> bool {
    match fault {
        MutationFault::None => false,
        #[cfg(test)]
        MutationFault::AfterTranscriptDelete => true,
    }
}

#[derive(Debug, FromRow)]
struct SegmentContext {
    run_id: String,
    segment_index: i64,
    run_status: String,
    context_sha256: String,
    raw_text: String,
}

async fn lock_and_load_segment(
    tx: &mut Transaction<'_, Sqlite>,
    segment_id: &str,
) -> Result<SegmentContext, MossStoreError> {
    let updated = sqlx::query(
        r#"
        UPDATE moss_transcription_runs SET updated_at = updated_at
        WHERE run_id = (
            SELECT run_id FROM moss_candidate_segments WHERE segment_id = ?
        )
        "#,
    )
    .bind(segment_id)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(MossStoreError::CandidateNotFound);
    }
    sqlx::query_as::<_, SegmentContext>(
        r#"
        SELECT r.run_id, s.segment_index, r.status AS run_status,
               r.context_sha256, s.raw_text
        FROM moss_candidate_segments s
        JOIN moss_transcription_runs r ON r.run_id = s.run_id
        WHERE s.segment_id = ?
        "#,
    )
    .bind(segment_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(MossStoreError::CandidateNotFound)
}

async fn lock_run(tx: &mut Transaction<'_, Sqlite>, run_id: &str) -> Result<(), MossStoreError> {
    let updated =
        sqlx::query("UPDATE moss_transcription_runs SET updated_at = updated_at WHERE run_id = ?")
            .bind(run_id)
            .execute(&mut **tx)
            .await?;
    if updated.rows_affected() != 1 {
        return Err(MossStoreError::RunNotFound);
    }
    Ok(())
}

async fn get_run_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    run_id: &str,
) -> Result<MossRunRecord, MossStoreError> {
    sqlx::query_as::<_, MossRunRecord>(
        r#"
        SELECT run_id, meeting_id, status, created_at, updated_at, started_at,
               completed_at, source_transcript_sha256, audio_sha256,
               model_sha256, runtime_sha256, context_sha256, backend_name,
               backend_version, runtime_version, model_revision, device_name,
               raw_output_sha256, clean_output_sha256, candidate_sha256,
               segment_count, wall_elapsed_ms, wall_rtf, peak_memory_bytes,
               error_code, language_requested, language_resolved,
               decode_parameters_json, decode_parameters_sha256
        FROM moss_transcription_runs WHERE run_id = ?
        "#,
    )
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(MossStoreError::RunNotFound)
}

async fn finish_transaction<T>(
    tx: Transaction<'_, Sqlite>,
    result: Result<T, MossStoreError>,
) -> Result<T, MossStoreError> {
    match result {
        Ok(value) => {
            tx.commit().await?;
            Ok(value)
        }
        Err(error) => {
            let _ = tx.rollback().await;
            Err(error)
        }
    }
}

fn validate_candidate_segments(
    segments: &[CandidateSegmentInput],
) -> Result<Vec<CandidateSegmentInput>, MossStoreError> {
    if segments.is_empty() {
        return Err(MossStoreError::InvalidInput("candidate_segments"));
    }
    let mut prepared = segments.to_vec();
    prepared.sort_by_key(|segment| segment.segment_index);
    let mut previous_start = 0_i64;
    for (expected, segment) in prepared.iter_mut().enumerate() {
        let expected =
            u32::try_from(expected).map_err(|_| MossStoreError::InvalidInput("segment_index"))?;
        if segment.segment_index != expected
            || segment.start_ms < 0
            || segment.end_ms < segment.start_ms
            || segment.start_ms < previous_start
            || segment.text.trim().is_empty()
        {
            return Err(MossStoreError::InvalidInput("candidate_segment"));
        }
        validate_speaker_label(&segment.speaker_label)?;
        previous_start = segment.start_ms;
    }
    Ok(prepared)
}

fn validate_candidate_alignments(
    values: &[CandidateAlignmentInput],
    segments: &[CandidateSegmentInput],
) -> Result<Vec<CandidateAlignmentInput>, MossStoreError> {
    if values.len() != segments.len() {
        return Err(MossStoreError::InvalidInput("candidate_alignments"));
    }
    let mut prepared = values.to_vec();
    prepared.sort_by_key(|value| value.segment_index);
    for (position, (value, segment)) in prepared.iter_mut().zip(segments).enumerate() {
        let expected = u32::try_from(position)
            .map_err(|_| MossStoreError::InvalidInput("alignment_segment_index"))?;
        value.raw_text_sha256 = normalized_sha256(&value.raw_text_sha256)?;
        if value.segment_index != expected
            || segment.segment_index != expected
            || value.raw_start_ms < 0
            || value.raw_end_ms < value.raw_start_ms
            || value
                .source_anchor_ids
                .iter()
                .any(|anchor| anchor.trim().is_empty() || anchor.len() > 512)
        {
            return Err(MossStoreError::InvalidInput("candidate_alignment"));
        }
        match value.alignment_method.as_str() {
            "moss_segment" if value.confidence.is_none() && value.source_anchor_ids.is_empty() => {}
            "source_transcript_segment"
                if value.confidence.is_some_and(|confidence| {
                    confidence.is_finite() && (0.0..=1.0).contains(&confidence)
                }) && !value.source_anchor_ids.is_empty() => {}
            ALIGNMENT_METHOD_AUDIO_TOKEN
                if value.confidence.is_some_and(|confidence| {
                    confidence.is_finite() && (0.0..=1.0).contains(&confidence)
                }) && value.source_anchor_ids.len() == 1 => {}
            _ => return Err(MossStoreError::InvalidInput("alignment_method")),
        }
    }

    let mut expected_raw_segment_index = 0u32;
    let mut position = 0usize;
    while position < prepared.len() {
        let first = &prepared[position];
        if first.raw_segment_index != expected_raw_segment_index {
            return Err(MossStoreError::InvalidInput("raw_segment_index"));
        }
        let group_end = prepared[position..]
            .iter()
            .position(|value| value.raw_segment_index != first.raw_segment_index)
            .map(|offset| position + offset)
            .unwrap_or(prepared.len());
        let group = &prepared[position..group_end];
        if group.iter().any(|value| {
            value.raw_start_ms != first.raw_start_ms
                || value.raw_end_ms != first.raw_end_ms
                || value.raw_text_sha256 != first.raw_text_sha256
                || value.alignment_method != first.alignment_method
        }) {
            return Err(MossStoreError::InvalidInput("raw_segment_provenance"));
        }
        let text = segments[position..group_end]
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<String>();
        if sha256_text(&text) != first.raw_text_sha256 {
            return Err(MossStoreError::InvalidInput("raw_text_sha256"));
        }
        if first.alignment_method == "moss_segment"
            && (group.len() != 1
                || segments[position].start_ms != first.raw_start_ms
                || segments[position].end_ms != first.raw_end_ms)
        {
            return Err(MossStoreError::InvalidInput("raw_segment_fallback"));
        }
        if first.alignment_method == "source_transcript_segment"
            || first.alignment_method == ALIGNMENT_METHOD_AUDIO_TOKEN
        {
            let output_group = &segments[position..group_end];
            let anchor_ids = group
                .iter()
                .map(|value| value.source_anchor_ids.first().map(String::as_str))
                .collect::<Option<std::collections::BTreeSet<_>>>();
            let source_partition_valid = first.raw_end_ms > first.raw_start_ms
                && output_group
                    .first()
                    .is_some_and(|segment| segment.start_ms == first.raw_start_ms)
                && output_group
                    .last()
                    .is_some_and(|segment| segment.end_ms == first.raw_end_ms)
                && output_group.iter().all(|segment| {
                    segment.start_ms >= first.raw_start_ms
                        && segment.end_ms <= first.raw_end_ms
                        && segment.end_ms > segment.start_ms
                })
                && output_group
                    .windows(2)
                    .all(|pair| pair[0].end_ms == pair[1].start_ms)
                && group.iter().all(|value| value.source_anchor_ids.len() == 1)
                && anchor_ids.is_some_and(|ids| ids.len() == group.len());
            if !source_partition_valid {
                return Err(MossStoreError::InvalidInput("source_segment_partition"));
            }
        }
        expected_raw_segment_index = expected_raw_segment_index
            .checked_add(1)
            .ok_or(MossStoreError::InvalidInput("raw_segment_index"))?;
        position = group_end;
    }
    Ok(prepared)
}

fn validate_audio_token_alignment(
    mut value: AudioTokenAlignmentInput,
    segments: &[CandidateSegmentInput],
    alignments: Option<&[CandidateAlignmentInput]>,
) -> Result<AudioTokenAlignmentInput, MossStoreError> {
    let alignments = alignments.ok_or(MossStoreError::InvalidInput("audio_token_alignments"))?;
    let audio_segment_indices = alignments
        .iter()
        .filter(|alignment| alignment.alignment_method == ALIGNMENT_METHOD_AUDIO_TOKEN)
        .map(|alignment| alignment.segment_index)
        .collect::<BTreeSet<_>>();
    let raw_group_count = alignments
        .iter()
        .map(|alignment| alignment.raw_segment_index)
        .collect::<BTreeSet<_>>()
        .len();
    let audio_raw_group_count = alignments
        .iter()
        .filter(|alignment| alignment.alignment_method == ALIGNMENT_METHOD_AUDIO_TOKEN)
        .map(|alignment| alignment.raw_segment_index)
        .collect::<BTreeSet<_>>()
        .len();
    let expected_fallback_raw_groups = raw_group_count.saturating_sub(audio_raw_group_count);
    if usize::try_from(value.token_aligned_segment_count).ok() != Some(audio_segment_indices.len())
        || usize::try_from(value.fallback_raw_segment_count).ok()
            != Some(expected_fallback_raw_groups)
        || (value.fallback_raw_segment_count > 0) != value.fallback_reason.is_some()
    {
        return Err(MossStoreError::InvalidInput("audio_token_counts"));
    }
    value.fallback_reason = value
        .fallback_reason
        .map(|reason| fixed_code(&reason))
        .transpose()?;
    value
        .boundaries
        .sort_by_key(|boundary| boundary.segment_index);
    let boundary_indices = value
        .boundaries
        .iter()
        .map(|boundary| boundary.segment_index)
        .collect::<BTreeSet<_>>();
    if boundary_indices.len() != value.boundaries.len()
        || boundary_indices != audio_segment_indices
        || value.boundaries.iter().any(|boundary| {
            usize::try_from(boundary.segment_index)
                .ok()
                .and_then(|index| segments.get(index))
                .is_none()
                || boundary.last_token_index < boundary.first_token_index
                || !boundary.confidence.is_finite()
                || !(0.0..=1.0).contains(&boundary.confidence)
                || !is_sha256(&boundary.token_track_sha256)
        })
    {
        return Err(MossStoreError::InvalidInput("audio_token_boundaries"));
    }
    match value.track.as_ref() {
        Some(track) => {
            validate_audio_token_track(track, None)
                .map_err(|_| MossStoreError::InvalidInput("audio_token_track"))?;
            if !value
                .global_match_coverage
                .is_some_and(|coverage| coverage.is_finite() && (0.0..=1.0).contains(&coverage))
                || (!value.boundaries.is_empty()
                    && !value
                        .global_match_coverage
                        .is_some_and(|coverage| coverage >= R5_MIN_GLOBAL_MATCH_COVERAGE))
                || value.boundaries.iter().any(|boundary| {
                    let Some(segment_index) = usize::try_from(boundary.segment_index).ok() else {
                        return true;
                    };
                    let Some(segment) = segments.get(segment_index) else {
                        return true;
                    };
                    let Some(first_index) = usize::try_from(boundary.first_token_index).ok() else {
                        return true;
                    };
                    let Some(last_index) = usize::try_from(boundary.last_token_index).ok() else {
                        return true;
                    };
                    let Some(first_token) = track.tokens.get(first_index) else {
                        return true;
                    };
                    let Some(last_token) = track.tokens.get(last_index) else {
                        return true;
                    };
                    let expected_confidence = track.tokens[first_index..=last_index]
                        .iter()
                        .map(|token| f64::from(token.probability))
                        .reduce(f64::min);
                    boundary.token_track_sha256 != track.token_track_sha256
                        || first_token.start_ms < segment.start_ms
                        || first_token.start_ms >= segment.end_ms
                        || last_token.start_ms < segment.start_ms
                        || last_token.start_ms >= segment.end_ms
                        || expected_confidence.is_none_or(|confidence| {
                            (confidence - boundary.confidence).abs() > 1e-6
                        })
                })
            {
                return Err(MossStoreError::InvalidInput("audio_token_binding"));
            }
        }
        None => {
            if !value.boundaries.is_empty()
                || value.token_aligned_segment_count != 0
                || value.global_match_coverage.is_some()
                || value.fallback_reason.is_none()
            {
                return Err(MossStoreError::InvalidInput("audio_token_fallback"));
            }
        }
    }
    Ok(value)
}

fn validate_run_diagnostics(
    mut value: MossRunDiagnosticsInput,
    segment_count: usize,
) -> Result<MossRunDiagnosticsInput, MossStoreError> {
    value.source_expected_sha256 = normalized_sha256(&value.source_expected_sha256)?;
    value.source_actual_sha256 = normalized_sha256(&value.source_actual_sha256)?;
    let activity_range_valid = match (value.first_active_ms, value.last_active_ms) {
        (None, None) => true,
        (Some(first), Some(last)) => first <= last && last <= value.audio_duration_ms,
        _ => false,
    };
    if value.audio_duration_ms == 0
        || value.activity_frame_ms != 20
        || value.activity_threshold_dbfs != -50.0
        || value.model_last_timestamp_ms < 0
        || !activity_range_valid
        || value.source_hash_verified && value.source_expected_sha256 != value.source_actual_sha256
        || usize::try_from(
            value
                .aligned_segment_count
                .saturating_add(value.fallback_segment_count),
        )
        .unwrap_or(usize::MAX)
            != segment_count
        || (value.fallback_segment_count > 0) != value.fallback_reason.is_some()
    {
        return Err(MossStoreError::InvalidInput("run_diagnostics"));
    }
    value.fallback_reason = value
        .fallback_reason
        .map(|reason| fixed_code(&reason))
        .transpose()?;
    Ok(value)
}

fn sha256_text(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn validate_speaker_label(label: &str) -> Result<(), MossStoreError> {
    let digits = label.strip_prefix('S').unwrap_or_default();
    if digits.len() < 2 || !digits.bytes().all(|value| value.is_ascii_digit()) {
        return Err(MossStoreError::InvalidInput("speaker_label"));
    }
    Ok(())
}

fn replace_char_range(
    input: &str,
    start_char: usize,
    end_char: usize,
    expected: &str,
    replacement: &str,
) -> Result<String, MossStoreError> {
    let characters = input.chars().collect::<Vec<_>>();
    if start_char >= end_char || end_char > characters.len() {
        return Err(MossStoreError::InvalidInput("correction_range"));
    }
    let actual = characters[start_char..end_char].iter().collect::<String>();
    if actual != expected {
        return Err(MossStoreError::InvalidInput("correction_original_text"));
    }
    let mut result = String::new();
    result.extend(characters[..start_char].iter());
    result.push_str(replacement);
    result.extend(characters[end_char..].iter());
    if result.trim().is_empty() {
        return Err(MossStoreError::InvalidInput("correction_result"));
    }
    Ok(result)
}

fn normalized_sha256(value: &str) -> Result<String, MossStoreError> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.len() != 64 || !normalized.bytes().all(|value| value.is_ascii_hexdigit()) {
        return Err(MossStoreError::InvalidInput("sha256"));
    }
    Ok(normalized)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn required(value: String, field: &'static str) -> Result<String, MossStoreError> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        return Err(MossStoreError::InvalidInput(field));
    }
    Ok(value)
}

fn fixed_code(value: &str) -> Result<String, MossStoreError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(MossStoreError::InvalidInput("error_code"));
    }
    Ok(value.to_owned())
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn sha256_json<T: Serialize>(value: &T) -> Result<String, MossStoreError> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(database) if database.is_unique_violation())
}

async fn ensure_meeting_exists<'e, E>(executor: E, meeting_id: &str) -> Result<(), MossStoreError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM meetings WHERE id = ?)")
        .bind(meeting_id)
        .fetch_one(executor)
        .await?;
    if !exists {
        return Err(MossStoreError::MeetingNotFound);
    }
    Ok(())
}

async fn transcript_snapshot<'e, E>(
    executor: E,
    meeting_id: &str,
) -> Result<(Vec<StoredTranscript>, String), MossStoreError>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let rows = sqlx::query_as::<_, StoredTranscript>(
        r#"
        SELECT id, meeting_id, transcript, timestamp, summary, action_items,
               key_points, audio_start_time, audio_end_time, duration, speaker
        FROM transcripts
        WHERE meeting_id = ?
        ORDER BY CASE WHEN audio_start_time IS NULL THEN 1 ELSE 0 END,
                 audio_start_time, timestamp, id
        "#,
    )
    .bind(meeting_id)
    .fetch_all(executor)
    .await?;
    let hash = sha256_json(&rows)?;
    Ok((rows, hash))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use sqlx::migrate::Migrator;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    use super::*;
    use crate::database::manager::DatabaseManager;
    use crate::moss_audio_token_alignment::{
        sha256_audio_token_track, sha256_json as audio_token_sha256_json, AudioToken,
        AudioTokenAlignmentParameters, AudioTokenSourceChunk, R5_ALIGNMENT_MODEL_NAME,
        R5_TOKEN_TRACK_SCHEMA_VERSION,
    };

    struct TestDatabase {
        _directory: tempfile::TempDir,
        manager: DatabaseManager,
    }

    #[derive(Debug, FromRow)]
    struct ActivationAuditRow {
        speaker_label: String,
        resolved_person_id: Option<String>,
        correction_id: Option<String>,
        override_id: Option<String>,
        binding_id: Option<String>,
        resolved_person_display_name: Option<String>,
    }

    impl TestDatabase {
        fn pool(&self) -> &SqlitePool {
            self.manager.pool()
        }
    }

    async fn database() -> TestDatabase {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("moss-p3.sqlite");
        let legacy_path = directory.path().join("legacy.db");
        let manager = DatabaseManager::new(
            database_path.to_str().unwrap(),
            legacy_path.to_str().unwrap(),
        )
        .await
        .unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(manager.pool())
            .await
            .unwrap();
        TestDatabase {
            _directory: directory,
            manager,
        }
    }

    fn digest(label: &str) -> String {
        format!("{:x}", Sha256::digest(label.as_bytes()))
    }

    fn run_input(meeting_id: &str, suffix: &str) -> NewMossRun {
        NewMossRun {
            meeting_id: meeting_id.to_owned(),
            audio_sha256: digest(&format!("audio-{suffix}")),
            model_sha256: digest("model-q8"),
            runtime_sha256: digest("runtime-v0.2.2"),
            context_sha256: digest(&format!("context-{suffix}")),
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
            device_name: "Intel(R) Arc(TM) Graphics".to_owned(),
            raw_output_sha256: digest(&format!("raw-{suffix}")),
            clean_output_sha256: digest(&format!("clean-{suffix}")),
            wall_elapsed_ms: 1_234,
            wall_rtf: 0.42,
            peak_memory_bytes: 987_654,
            language_requested: moss_helper::native::MOSS_LANGUAGE_REQUESTED.to_owned(),
            language_resolved: moss_helper::native::MOSS_LANGUAGE_RESOLVED.to_owned(),
            decode_parameters_json: moss_helper::native::MOSS_DECODE_PARAMETERS_JSON.to_owned(),
            decode_parameters_sha256: moss_helper::native::moss_decode_parameters_sha256(),
        }
    }

    fn candidate(texts: &[(&str, &str)]) -> Vec<CandidateSegmentInput> {
        texts
            .iter()
            .enumerate()
            .map(|(index, (speaker, text))| CandidateSegmentInput {
                segment_index: index as u32,
                start_ms: (index as i64) * 1_000,
                end_ms: (index as i64 + 1) * 1_000,
                speaker_label: (*speaker).to_owned(),
                text: (*text).to_owned(),
            })
            .collect()
    }

    fn raw_alignments(segments: &[CandidateSegmentInput]) -> Vec<CandidateAlignmentInput> {
        segments
            .iter()
            .map(|segment| CandidateAlignmentInput {
                segment_index: segment.segment_index,
                raw_segment_index: segment.segment_index,
                raw_start_ms: segment.start_ms,
                raw_end_ms: segment.end_ms,
                raw_text_sha256: digest(&segment.text),
                alignment_method: "moss_segment".to_owned(),
                confidence: None,
                source_anchor_ids: Vec::new(),
            })
            .collect()
    }

    fn diagnostics(source_sha256: &str, segment_count: usize) -> MossRunDiagnosticsInput {
        MossRunDiagnosticsInput {
            audio_duration_ms: 4_000,
            activity_frame_ms: 20,
            activity_threshold_dbfs: -50.0,
            first_active_ms: Some(0),
            last_active_ms: Some(3_500),
            model_last_timestamp_ms: 3_550,
            aligned_segment_count: 0,
            fallback_segment_count: segment_count.try_into().unwrap(),
            source_anchor_count: 1,
            source_hash_verified: true,
            source_expected_sha256: source_sha256.to_owned(),
            source_actual_sha256: source_sha256.to_owned(),
            fallback_reason: Some("PARTIAL_ALIGNMENT_FALLBACK".to_owned()),
        }
    }

    fn r5_track(audio_sha256: &str) -> AudioTokenTrack {
        let parameters = AudioTokenAlignmentParameters::product("zh");
        let parameters_sha256 = audio_token_sha256_json(&parameters).unwrap();
        let tokens = vec![
            AudioToken {
                global_token_index: 0,
                chunk_index: 0,
                whisper_segment_index: 0,
                whisper_token_index: 0,
                start_ms: 100,
                end_ms: 400,
                text: " Google".to_owned(),
                probability: 0.9,
            },
            AudioToken {
                global_token_index: 1,
                chunk_index: 0,
                whisper_segment_index: 0,
                whisper_token_index: 1,
                start_ms: 1_200,
                end_ms: 1_500,
                text: "另外".to_owned(),
                probability: 0.8,
            },
        ];
        let chunk_text = tokens
            .iter()
            .map(|token| token.text.as_str())
            .collect::<String>();
        let source_chunks = vec![AudioTokenSourceChunk {
            chunk_index: 0,
            start_ms: 0,
            end_ms: 2_000,
            sample_count: 32_000,
            text_sha256: audio_token_sha256_json(&chunk_text).unwrap(),
            first_global_token_index: 0,
            token_count: 2,
        }];
        let token_track_sha256 = sha256_audio_token_track(&source_chunks, &tokens).unwrap();
        AudioTokenTrack {
            schema_version: R5_TOKEN_TRACK_SCHEMA_VERSION,
            audio_sha256: audio_sha256.to_owned(),
            audio_duration_ms: 4_000,
            model_name: R5_ALIGNMENT_MODEL_NAME.to_owned(),
            model_sha256: digest("whisper-r5-model"),
            program_sha256: digest("meetily-r5-program"),
            parameters_sha256,
            token_track_sha256,
            backend: "cpu".to_owned(),
            parameters,
            hotword_bias_diagnostics: None,
            source_chunks,
            tokens,
        }
    }

    fn r5_completion_inputs(
        source_sha256: &str,
        track: AudioTokenTrack,
    ) -> (
        Vec<CandidateSegmentInput>,
        Vec<CandidateAlignmentInput>,
        MossRunDiagnosticsInput,
        AudioTokenAlignmentInput,
    ) {
        let segments = candidate(&[("S01", "谷歌")]);
        let alignments = vec![CandidateAlignmentInput {
            segment_index: 0,
            raw_segment_index: 0,
            raw_start_ms: 0,
            raw_end_ms: 1_000,
            raw_text_sha256: digest("谷歌"),
            alignment_method: ALIGNMENT_METHOD_AUDIO_TOKEN.to_owned(),
            confidence: Some(0.9),
            source_anchor_ids: vec!["whisper-token-000000-000000".to_owned()],
        }];
        let mut run_diagnostics = diagnostics(source_sha256, 1);
        run_diagnostics.aligned_segment_count = 1;
        run_diagnostics.fallback_segment_count = 0;
        run_diagnostics.fallback_reason = None;
        let token_track_sha256 = track.token_track_sha256.clone();
        let audio_tokens = AudioTokenAlignmentInput {
            track: Some(track),
            boundaries: vec![CandidateAudioTokenBoundary {
                segment_index: 0,
                first_token_index: 0,
                last_token_index: 0,
                token_track_sha256,
                confidence: 0.9,
            }],
            global_match_coverage: Some(0.75),
            token_aligned_segment_count: 1,
            fallback_raw_segment_count: 0,
            fallback_reason: None,
        };
        (segments, alignments, run_diagnostics, audio_tokens)
    }

    async fn insert_meeting(pool: &SqlitePool, meeting_id: &str, text: &str) {
        let now = "2026-08-29T00:00:00.000Z";
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES (?, ?, ?, ?)")
            .bind(meeting_id)
            .bind(format!("Meeting {meeting_id}"))
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
            ) VALUES (?, ?, ?, ?, ?, ?, ?, 0.0, 1.0, 1.0, 'microphone')
            "#,
        )
        .bind(format!("transcript-{meeting_id}"))
        .bind(meeting_id)
        .bind(text)
        .bind(now)
        .bind("preserved summary")
        .bind("preserved action")
        .bind("preserved key point")
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO transcript_chunks (
                meeting_id, transcript_text, model, model_name, created_at
            ) VALUES (?, ?, 'whisper', 'baseline', ?)
            "#,
        )
        .bind(meeting_id)
        .bind(text)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn original_text(pool: &SqlitePool, meeting_id: &str) -> String {
        sqlx::query_scalar("SELECT transcript FROM transcripts WHERE meeting_id = ?")
            .bind(meeting_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn run_store_fails_closed_on_invalid_native_decode_contract() {
        let database = database().await;
        insert_meeting(database.pool(), "meeting-decode-contract", "原始文字").await;

        let mut automatic = run_input("meeting-decode-contract", "auto");
        automatic.language_requested = "auto".to_owned();
        assert!(matches!(
            MossCandidateRepository::start_run(database.pool(), automatic).await,
            Err(MossStoreError::InvalidInput("native_decode_contract"))
        ));

        let mut bad_hash = run_input("meeting-decode-contract", "bad-hash");
        bad_hash.decode_parameters_sha256 = "0".repeat(64);
        assert!(matches!(
            MossCandidateRepository::start_run(database.pool(), bad_hash).await,
            Err(MossStoreError::InvalidInput("native_decode_contract"))
        ));

        let run = MossCandidateRepository::start_run(
            database.pool(),
            run_input("meeting-decode-contract", "valid"),
        )
        .await
        .unwrap();
        let mut non_chinese = completion("non-chinese");
        non_chinese.language_resolved = "en-US".to_owned();
        assert!(matches!(
            MossCandidateRepository::complete_run(
                database.pool(),
                &run.run_id,
                &candidate(&[("S01", "候选文字")]),
                non_chinese,
            )
            .await,
            Err(MossStoreError::InvalidInput("native_decode_contract"))
        ));
    }

    async fn complete(
        pool: &SqlitePool,
        meeting_id: &str,
        suffix: &str,
        segments: &[CandidateSegmentInput],
    ) -> MossRunRecord {
        let run = MossCandidateRepository::start_run(pool, run_input(meeting_id, suffix))
            .await
            .unwrap();
        MossCandidateRepository::complete_run(pool, &run.run_id, segments, completion(suffix))
            .await
            .unwrap()
    }

    async fn segment_id(pool: &SqlitePool, run_id: &str, index: i64) -> String {
        sqlx::query_scalar(
            "SELECT segment_id FROM moss_candidate_segments WHERE run_id = ? AND segment_index = ?",
        )
        .bind(run_id)
        .bind(index)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    fn migration_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations")
    }

    #[tokio::test]
    async fn append_only_migration_upgrades_pre_p3_database_without_changing_old_data() {
        let directory = tempfile::tempdir().unwrap();
        let old_migrations = directory.path().join("old-migrations");
        std::fs::create_dir(&old_migrations).unwrap();
        for entry in std::fs::read_dir(migration_root()).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".sql")
                && name != "20260829000000_add_moss_candidate_store.sql"
                && name != "20260831000000_add_moss_alignment_provenance.sql"
                && name != "20260831010000_add_moss_audio_token_provenance.sql"
                && name != "20260905000000_add_moss_native_decode_contract.sql"
            {
                std::fs::copy(entry.path(), old_migrations.join(name)).unwrap();
            }
        }
        let database_path = directory.path().join("pre-p3.sqlite");
        let options = SqliteConnectOptions::new()
            .filename(&database_path)
            .create_if_missing(true)
            .foreign_keys(true);
        let old_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        Migrator::new(old_migrations)
            .await
            .unwrap()
            .run(&old_pool)
            .await
            .unwrap();
        insert_meeting(&old_pool, "legacy-meeting", "人工编辑不得丢失").await;
        sqlx::query(
            r#"
            INSERT INTO summary_processes (
                meeting_id, status, created_at, updated_at, result
            ) VALUES ('legacy-meeting', 'completed', 'now', 'now', '{"saved":true}')
            "#,
        )
        .execute(&old_pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO summary_generation_history (
                generation_id, meeting_id, status, created_at, updated_at,
                template_id, template_version, file_sha256, semantic_sha256,
                snapshot_path_relative, resolution_source, model_provider, model_name
            ) VALUES ('legacy-generation', 'legacy-meeting', 'completed', 'now', 'now',
                      'daily', 7, ?, ?, 'snapshots/legacy.json', 'meeting', 'ollama', 'qwen')
            "#,
        )
        .bind(digest("legacy-file"))
        .bind(digest("legacy-semantic"))
        .execute(&old_pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO summary_manual_revisions (
                revision_id, meeting_id, created_at, source_generation_id,
                summary_json, markdown
            ) VALUES ('legacy-revision', 'legacy-meeting', 'now',
                      'legacy-generation', '{"manual":true}', '人工摘要')
            "#,
        )
        .execute(&old_pool)
        .await
        .unwrap();
        old_pool.close().await;

        let manager = DatabaseManager::new(
            database_path.to_str().unwrap(),
            directory.path().join("absent.db").to_str().unwrap(),
        )
        .await
        .unwrap();
        let pool = manager.pool();
        assert_eq!(
            original_text(pool, "legacy-meeting").await,
            "人工编辑不得丢失"
        );
        let summary: String = sqlx::query_scalar(
            "SELECT result FROM summary_processes WHERE meeting_id = 'legacy-meeting'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(summary, "{\"saved\":true}");
        let template_version: i64 = sqlx::query_scalar(
            "SELECT template_version FROM summary_generation_history WHERE generation_id = 'legacy-generation'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(template_version, 7);
        let manual_markdown: String = sqlx::query_scalar(
            "SELECT markdown FROM summary_manual_revisions WHERE revision_id = 'legacy-revision'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(manual_markdown, "人工摘要");
        let moss_tables: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'moss_%' ORDER BY name",
        )
        .fetch_all(pool)
        .await
        .unwrap();
        assert_eq!(
            moss_tables,
            [
                "moss_activation_segments",
                "moss_activation_snapshots",
                "moss_audio_token_runs",
                "moss_audio_token_source_chunks",
                "moss_audio_tokens",
                "moss_candidate_audio_token_boundary",
                "moss_candidate_segment_alignment",
                "moss_candidate_segments",
                "moss_machine_term_correction_source",
                "moss_run_diagnostics",
                "moss_segment_overrides",
                "moss_speaker_bindings",
                "moss_term_corrections",
                "moss_transcription_runs",
            ]
            .map(str::to_owned)
            .to_vec()
        );
    }

    #[tokio::test]
    async fn r5_audio_token_and_machine_source_round_trip_after_database_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("r5-round-trip.sqlite");
        let legacy_path = directory.path().join("absent.db");
        let manager = DatabaseManager::new(
            database_path.to_str().unwrap(),
            legacy_path.to_str().unwrap(),
        )
        .await
        .unwrap();
        let pool = manager.pool();
        insert_meeting(pool, "r5-round-trip", "人工原文不能改变").await;
        let run =
            MossCandidateRepository::start_run(pool, run_input("r5-round-trip", "r5-round-trip"))
                .await
                .unwrap();
        let track = r5_track(&run.audio_sha256);
        let (segments, alignments, run_diagnostics, audio_tokens) =
            r5_completion_inputs(&run.source_transcript_sha256, track.clone());
        MossCandidateRepository::complete_run_with_r5_provenance(
            pool,
            &run.run_id,
            &segments,
            completion("r5-round-trip"),
            &alignments,
            run_diagnostics,
            audio_tokens,
        )
        .await
        .unwrap();
        let segment_id = segment_id(pool, &run.run_id, 0).await;

        let invalid_source = MachineTermSuggestion {
            segment_index: 0,
            start_char: 0,
            end_char: 2,
            original_text: "谷歌".to_owned(),
            replacement_text: "Google".to_owned(),
            term_id: "term-google".to_owned(),
            context_sha256: run.context_sha256.clone(),
            token_track_sha256: track.token_track_sha256.clone(),
            model_sha256: track.model_sha256.clone(),
            first_token_index: 1,
            last_token_index: 1,
            confidence: 0.8,
        };
        let rejected =
            MossCandidateRepository::add_machine_term_correction(pool, &segment_id, invalid_source)
                .await
                .unwrap_err();
        assert!(matches!(
            rejected,
            MossStoreError::InvalidInput("machine_term_tokens")
        ));
        let wrong_confidence_source = MachineTermSuggestion {
            segment_index: 0,
            start_char: 0,
            end_char: 2,
            original_text: "谷歌".to_owned(),
            replacement_text: "Google".to_owned(),
            term_id: "term-google".to_owned(),
            context_sha256: run.context_sha256.clone(),
            token_track_sha256: track.token_track_sha256.clone(),
            model_sha256: track.model_sha256.clone(),
            first_token_index: 0,
            last_token_index: 0,
            confidence: 0.7,
        };
        let rejected = MossCandidateRepository::add_machine_term_correction(
            pool,
            &segment_id,
            wrong_confidence_source,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            rejected,
            MossStoreError::InvalidInput("machine_term_confidence")
        ));
        let correction_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM moss_term_corrections")
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(correction_count, 0);

        let valid_source = MachineTermSuggestion {
            segment_index: 0,
            start_char: 0,
            end_char: 2,
            original_text: "谷歌".to_owned(),
            replacement_text: "Google".to_owned(),
            term_id: "term-google".to_owned(),
            context_sha256: run.context_sha256.clone(),
            token_track_sha256: track.token_track_sha256.clone(),
            model_sha256: track.model_sha256.clone(),
            first_token_index: 0,
            last_token_index: 0,
            confidence: 0.9,
        };
        let correction =
            MossCandidateRepository::add_machine_term_correction(pool, &segment_id, valid_source)
                .await
                .unwrap();
        assert_eq!(correction.result_text, "Google");
        assert_eq!(
            original_text(pool, "r5-round-trip").await,
            "人工原文不能改变"
        );

        pool.close().await;
        drop(manager);
        let reopened = DatabaseManager::new(
            database_path.to_str().unwrap(),
            legacy_path.to_str().unwrap(),
        )
        .await
        .unwrap();
        let reopened_pool = reopened.pool();
        let token_record =
            MossCandidateRepository::audio_token_alignment(reopened_pool, &run.run_id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(token_record.status, "verified");
        assert_eq!(
            token_record.token_track_sha256.as_deref(),
            Some(track.token_track_sha256.as_str())
        );
        assert_eq!(token_record.token_aligned_segment_count, 1);
        let persisted_counts: (i64, i64, i64, i64) = sqlx::query_as(
            r#"
            SELECT
                (SELECT COUNT(*) FROM moss_audio_token_source_chunks WHERE run_id = ?),
                (SELECT COUNT(*) FROM moss_audio_tokens WHERE run_id = ?),
                (SELECT COUNT(*) FROM moss_candidate_audio_token_boundary WHERE run_id = ?),
                (SELECT COUNT(*) FROM moss_machine_term_correction_source WHERE run_id = ?)
            "#,
        )
        .bind(&run.run_id)
        .bind(&run.run_id)
        .bind(&run.run_id)
        .bind(&run.run_id)
        .fetch_one(reopened_pool)
        .await
        .unwrap();
        assert_eq!(persisted_counts, (1, 2, 1, 1));
    }

    #[tokio::test]
    async fn r5_audio_hash_mismatch_rolls_back_every_candidate_write() {
        let db = database().await;
        let pool = db.pool();
        insert_meeting(pool, "r5-atomic", "人工原文不能改变").await;
        let run = MossCandidateRepository::start_run(pool, run_input("r5-atomic", "r5-atomic"))
            .await
            .unwrap();
        let wrong_track = r5_track(&digest("different-audio"));
        let (segments, alignments, run_diagnostics, audio_tokens) =
            r5_completion_inputs(&run.source_transcript_sha256, wrong_track);
        let error = MossCandidateRepository::complete_run_with_r5_provenance(
            pool,
            &run.run_id,
            &segments,
            completion("r5-atomic"),
            &alignments,
            run_diagnostics,
            audio_tokens,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            MossStoreError::InvalidInput("audio_token_audio_sha256")
        ));
        let counts: (i64, i64, i64) = sqlx::query_as(
            r#"
            SELECT
                (SELECT COUNT(*) FROM moss_candidate_segments WHERE run_id = ?),
                (SELECT COUNT(*) FROM moss_audio_token_runs WHERE run_id = ?),
                (SELECT COUNT(*) FROM moss_candidate_audio_token_boundary WHERE run_id = ?)
            "#,
        )
        .bind(&run.run_id)
        .bind(&run.run_id)
        .bind(&run.run_id)
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(counts, (0, 0, 0));
        let correct_track = r5_track(&run.audio_sha256);
        let (segments, alignments, run_diagnostics, mut low_coverage_tokens) =
            r5_completion_inputs(&run.source_transcript_sha256, correct_track);
        low_coverage_tokens.global_match_coverage = Some(0.49);
        let low_coverage_error = MossCandidateRepository::complete_run_with_r5_provenance(
            pool,
            &run.run_id,
            &segments,
            completion("r5-low-coverage"),
            &alignments,
            run_diagnostics,
            low_coverage_tokens,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            low_coverage_error,
            MossStoreError::InvalidInput("audio_token_binding")
        ));
        let counts_after_low_coverage: (i64, i64, i64) = sqlx::query_as(
            r#"
            SELECT
                (SELECT COUNT(*) FROM moss_candidate_segments WHERE run_id = ?),
                (SELECT COUNT(*) FROM moss_audio_token_runs WHERE run_id = ?),
                (SELECT COUNT(*) FROM moss_candidate_audio_token_boundary WHERE run_id = ?)
            "#,
        )
        .bind(&run.run_id)
        .bind(&run.run_id)
        .bind(&run.run_id)
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(counts_after_low_coverage, (0, 0, 0));
        assert_eq!(
            MossCandidateRepository::get_run(pool, &run.run_id)
                .await
                .unwrap()
                .status,
            "running"
        );
        assert_eq!(original_text(pool, "r5-atomic").await, "人工原文不能改变");
    }

    #[tokio::test]
    async fn run_trace_is_complete_and_database_enforces_one_running_run_per_meeting() {
        let db = database().await;
        insert_meeting(db.pool(), "m1", "原始人工文字").await;
        let expected_source = MossCandidateRepository::current_transcript_sha256(db.pool(), "m1")
            .await
            .unwrap();
        let run = MossCandidateRepository::start_run(db.pool(), run_input("m1", "first"))
            .await
            .unwrap();
        assert_eq!(run.source_transcript_sha256, expected_source);
        assert_eq!(run.audio_sha256, digest("audio-first"));
        assert_eq!(run.model_sha256, digest("model-q8"));
        assert_eq!(run.runtime_sha256, digest("runtime-v0.2.2"));
        assert_eq!(run.context_sha256, digest("context-first"));
        assert_eq!(run.status, "running");

        let second = MossCandidateRepository::start_run(db.pool(), run_input("m1", "second"))
            .await
            .unwrap_err();
        assert!(matches!(second, MossStoreError::ActiveRunExists));
        MossCandidateRepository::mark_run_cancelled(db.pool(), &run.run_id)
            .await
            .unwrap();
        let replacement =
            MossCandidateRepository::start_run(db.pool(), run_input("m1", "replacement"))
                .await
                .unwrap();
        assert_eq!(replacement.status, "running");
    }

    #[tokio::test]
    async fn source_transcript_anchors_are_time_valid_and_bound_to_the_run_hash() {
        let db = database().await;
        insert_meeting(db.pool(), "verified", "CGS 项目进度").await;
        let run = MossCandidateRepository::start_run(db.pool(), run_input("verified", "anchor"))
            .await
            .unwrap();
        let verified = MossCandidateRepository::source_transcript_anchors(db.pool(), &run.run_id)
            .await
            .unwrap();
        assert!(verified.hash_verified);
        assert_eq!(verified.invalid_timed_rows, 0);
        assert_eq!(verified.anchors.len(), 1);
        assert_eq!(verified.anchors[0].start_ms, 0);
        assert_eq!(verified.anchors[0].end_ms, 1_000);

        sqlx::query(
            "UPDATE transcripts SET transcript = '人工改过的内容' WHERE meeting_id = 'verified'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let changed = MossCandidateRepository::source_transcript_anchors(db.pool(), &run.run_id)
            .await
            .unwrap();
        assert!(!changed.hash_verified);
        assert!(changed.anchors.is_empty());

        insert_meeting(db.pool(), "rounded-invalid", "时间过短").await;
        sqlx::query(
            "UPDATE transcripts SET audio_start_time = 0.0001, audio_end_time = 0.0004 WHERE meeting_id = 'rounded-invalid'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let invalid_run = MossCandidateRepository::start_run(
            db.pool(),
            run_input("rounded-invalid", "rounded-invalid"),
        )
        .await
        .unwrap();
        let invalid =
            MossCandidateRepository::source_transcript_anchors(db.pool(), &invalid_run.run_id)
                .await
                .unwrap();
        assert!(invalid.hash_verified);
        assert_eq!(invalid.invalid_timed_rows, 1);
        assert!(invalid.anchors.is_empty());
    }

    #[tokio::test]
    async fn provenance_and_activity_diagnostics_commit_atomically_for_overlapping_segments() {
        let db = database().await;
        insert_meeting(db.pool(), "m1", "冻结的原转录").await;
        let run = MossCandidateRepository::start_run(db.pool(), run_input("m1", "provenance"))
            .await
            .unwrap();
        let segments = vec![
            CandidateSegmentInput {
                segment_index: 0,
                start_ms: 0,
                end_ms: 3_000,
                speaker_label: "S01".to_owned(),
                text: "CGS 项目".to_owned(),
            },
            CandidateSegmentInput {
                segment_index: 1,
                start_ms: 1_000,
                end_ms: 2_000,
                speaker_label: "S02".to_owned(),
                text: "Google 包".to_owned(),
            },
        ];
        let completed = MossCandidateRepository::complete_run_with_provenance(
            db.pool(),
            &run.run_id,
            &segments,
            completion("provenance"),
            &raw_alignments(&segments),
            diagnostics(&run.source_transcript_sha256, segments.len()),
        )
        .await
        .unwrap();
        assert_eq!(completed.status, "completed");
        let alignment_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM moss_candidate_segment_alignment WHERE segment_id IN (SELECT segment_id FROM moss_candidate_segments WHERE run_id = ?)",
        )
        .bind(&run.run_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(alignment_count, 2);
        let stored = MossCandidateRepository::run_diagnostics(db.pool(), &run.run_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.first_active_ms, Some(0));
        assert_eq!(stored.last_active_ms, Some(3_500));
        assert_eq!(stored.model_last_timestamp_ms, 3_550);
        assert_eq!(stored.tail_delta_ms, Some(50));
        assert_eq!(stored.fallback_segment_count, 2);
        let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(integrity, "ok");
    }

    #[tokio::test]
    async fn provenance_failure_rolls_back_and_legacy_completion_remains_readable() {
        let db = database().await;
        insert_meeting(db.pool(), "failed", "冻结来源").await;
        let run = MossCandidateRepository::start_run(db.pool(), run_input("failed", "bad-source"))
            .await
            .unwrap();
        let segments = candidate(&[("S01", "候选")]);
        let wrong_source = digest("wrong-source");
        let error = MossCandidateRepository::complete_run_with_provenance(
            db.pool(),
            &run.run_id,
            &segments,
            completion("bad-source"),
            &raw_alignments(&segments),
            diagnostics(&wrong_source, segments.len()),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            MossStoreError::InvalidInput("source_expected_sha256")
        ));
        let candidate_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM moss_candidate_segments WHERE run_id = ?")
                .bind(&run.run_id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(candidate_count, 0);
        assert_eq!(
            MossCandidateRepository::get_run(db.pool(), &run.run_id)
                .await
                .unwrap()
                .status,
            "running"
        );

        MossCandidateRepository::mark_run_cancelled(db.pool(), &run.run_id)
            .await
            .unwrap();
        insert_meeting(db.pool(), "legacy", "保留旧数据").await;
        let legacy = complete(
            db.pool(),
            "legacy",
            "legacy-complete",
            &candidate(&[("S01", "旧候选")]),
        )
        .await;
        assert!(
            MossCandidateRepository::run_diagnostics(db.pool(), &legacy.run_id)
                .await
                .unwrap()
                .is_none()
        );
        let legacy_alignment_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM moss_candidate_segment_alignment WHERE segment_id IN (SELECT segment_id FROM moss_candidate_segments WHERE run_id = ?)",
        )
        .bind(&legacy.run_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(legacy_alignment_count, 0);
    }

    #[tokio::test]
    async fn source_alignment_must_partition_the_original_moss_time_range() {
        let db = database().await;
        insert_meeting(db.pool(), "partition", "冻结来源").await;
        let run = MossCandidateRepository::start_run(db.pool(), run_input("partition", "gap"))
            .await
            .unwrap();
        let segments = vec![
            CandidateSegmentInput {
                segment_index: 0,
                start_ms: 0,
                end_ms: 400,
                speaker_label: "S01".to_owned(),
                text: "前半段".to_owned(),
            },
            CandidateSegmentInput {
                segment_index: 1,
                start_ms: 600,
                end_ms: 1_000,
                speaker_label: "S01".to_owned(),
                text: "后半段".to_owned(),
            },
        ];
        let raw_hash = digest("前半段后半段");
        let alignments = vec![
            CandidateAlignmentInput {
                segment_index: 0,
                raw_segment_index: 0,
                raw_start_ms: 0,
                raw_end_ms: 1_000,
                raw_text_sha256: raw_hash.clone(),
                alignment_method: "source_transcript_segment".to_owned(),
                confidence: Some(0.9),
                source_anchor_ids: vec!["anchor-1".to_owned()],
            },
            CandidateAlignmentInput {
                segment_index: 1,
                raw_segment_index: 0,
                raw_start_ms: 0,
                raw_end_ms: 1_000,
                raw_text_sha256: raw_hash,
                alignment_method: "source_transcript_segment".to_owned(),
                confidence: Some(0.9),
                source_anchor_ids: vec!["anchor-2".to_owned()],
            },
        ];
        let error = MossCandidateRepository::complete_run_with_provenance(
            db.pool(),
            &run.run_id,
            &segments,
            completion("gap"),
            &alignments,
            MossRunDiagnosticsInput {
                aligned_segment_count: 2,
                fallback_segment_count: 0,
                fallback_reason: None,
                ..diagnostics(&run.source_transcript_sha256, 0)
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            MossStoreError::InvalidInput("source_segment_partition")
        ));
        let stored_segments: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM moss_candidate_segments WHERE run_id = ?")
                .bind(&run.run_id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(stored_segments, 0);
        assert_eq!(
            MossCandidateRepository::get_run(db.pool(), &run.run_id)
                .await
                .unwrap()
                .status,
            "running"
        );
    }

    #[tokio::test]
    async fn completed_candidates_are_versioned_and_never_replace_current_transcript() {
        let db = database().await;
        insert_meeting(db.pool(), "m1", "人工保留内容").await;
        let first = complete(db.pool(), "m1", "first", &candidate(&[("S01", "候选一")])).await;
        assert_eq!(first.status, "completed");
        assert_eq!(first.segment_count, 1);
        assert!(first.candidate_sha256.is_some());
        assert_eq!(original_text(db.pool(), "m1").await, "人工保留内容");

        let second = complete(db.pool(), "m1", "second", &candidate(&[("S01", "候选二")])).await;
        assert_ne!(first.run_id, second.run_id);
        assert_ne!(first.candidate_sha256, second.candidate_sha256);
        let candidates: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM moss_transcription_runs WHERE meeting_id = 'm1' AND status = 'completed'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(candidates, 2);
        assert_eq!(original_text(db.pool(), "m1").await, "人工保留内容");
    }

    #[tokio::test]
    async fn failure_cancellation_and_restart_recovery_preserve_transcripts_and_recording() {
        let db = database().await;
        let recording_directory = tempfile::tempdir().unwrap();
        let recording_path = recording_directory.path().join("recording.wav");
        std::fs::write(&recording_path, b"synthetic recording sentinel").unwrap();
        for meeting in ["failed", "cancelled", "interrupted"] {
            insert_meeting(db.pool(), meeting, &format!("original-{meeting}")).await;
        }
        let failed = MossCandidateRepository::start_run(db.pool(), run_input("failed", "failed"))
            .await
            .unwrap();
        MossCandidateRepository::mark_run_failed(
            db.pool(),
            &failed.run_id,
            "MOSS_MODEL_LOAD_FAILED",
        )
        .await
        .unwrap();
        let cancelled =
            MossCandidateRepository::start_run(db.pool(), run_input("cancelled", "cancelled"))
                .await
                .unwrap();
        MossCandidateRepository::mark_run_cancelled(db.pool(), &cancelled.run_id)
            .await
            .unwrap();
        let interrupted =
            MossCandidateRepository::start_run(db.pool(), run_input("interrupted", "interrupted"))
                .await
                .unwrap();
        let recovery = MossCandidateRepository::recover_interrupted_runs(db.pool())
            .await
            .unwrap();
        assert_eq!(recovery.runs_failed, 1);
        assert_eq!(
            MossCandidateRepository::get_run(db.pool(), &interrupted.run_id)
                .await
                .unwrap()
                .error_code
                .as_deref(),
            Some(MOSS_INTERRUPTED_BY_RESTART)
        );
        assert_eq!(
            MossCandidateRepository::recover_interrupted_runs(db.pool())
                .await
                .unwrap()
                .runs_failed,
            0
        );
        for meeting in ["failed", "cancelled", "interrupted"] {
            assert_eq!(
                original_text(db.pool(), meeting).await,
                format!("original-{meeting}")
            );
        }
        assert_eq!(
            std::fs::read(recording_path).unwrap(),
            b"synthetic recording sentinel"
        );
    }

    #[tokio::test]
    async fn correction_binding_and_segment_override_are_audited_in_activation() {
        let db = database().await;
        insert_meeting(db.pool(), "m1", "当前 Whisper 人工编辑").await;
        let run = complete(
            db.pool(),
            "m1",
            "candidate",
            &candidate(&[("S01", "我们讨论西吉艾斯"), ("S02", "原始第二段")]),
        )
        .await;
        let first_segment = segment_id(db.pool(), &run.run_id, 0).await;
        let second_segment = segment_id(db.pool(), &run.run_id, 1).await;
        let context_sha256 = run.context_sha256.clone();
        let correction = MossCandidateRepository::add_term_correction(
            db.pool(),
            &first_segment,
            TermCorrectionInput {
                start_char: 4,
                end_char: 8,
                original_text: "西吉艾斯".to_owned(),
                replacement_text: "CGS".to_owned(),
                rule_id: "TERM_ALIAS_CGS".to_owned(),
                expected_context_sha256: context_sha256.clone(),
            },
        )
        .await
        .unwrap();
        assert_eq!(correction.result_text, "我们讨论CGS");
        let binding_id = MossCandidateRepository::bind_speaker(
            db.pool(),
            &run.run_id,
            "S01",
            BoundPerson {
                person_id: "person-alpha".to_owned(),
                display_name: "Participant Alpha".to_owned(),
            },
            &context_sha256,
        )
        .await
        .unwrap();
        let override_id = MossCandidateRepository::set_segment_override(
            db.pool(),
            &second_segment,
            SegmentOverrideInput {
                replacement_text: Some("人工覆盖第二段".to_owned()),
                person: Some(BoundPerson {
                    person_id: "person-beta".to_owned(),
                    display_name: "Participant Beta".to_owned(),
                }),
                reason_code: "MANUAL_SEGMENT_FIX".to_owned(),
                expected_context_sha256: context_sha256,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            original_text(db.pool(), "m1").await,
            "当前 Whisper 人工编辑"
        );

        let activation = MossCandidateRepository::activate_candidate(db.pool(), &run.run_id)
            .await
            .unwrap();
        assert_eq!(activation.segment_count, 2);
        let activated: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT transcript, speaker FROM transcripts WHERE meeting_id = 'm1' ORDER BY audio_start_time",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(
            activated,
            vec![
                ("我们讨论CGS".to_owned(), None),
                ("人工覆盖第二段".to_owned(), None)
            ]
        );
        let audit: Vec<ActivationAuditRow> = sqlx::query_as(
            r#"
            SELECT speaker_label, resolved_person_id, correction_id,
                   override_id, binding_id, resolved_person_display_name
            FROM moss_activation_segments
            WHERE activation_id = ? ORDER BY segment_index
            "#,
        )
        .bind(&activation.activation_id)
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(audit[0].speaker_label, "S01");
        assert_eq!(audit[0].resolved_person_id.as_deref(), Some("person-alpha"));
        assert_eq!(
            audit[0].correction_id.as_deref(),
            Some(correction.correction_id.as_str())
        );
        assert_eq!(audit[0].binding_id.as_deref(), Some(binding_id.as_str()));
        assert_eq!(
            audit[0].resolved_person_display_name.as_deref(),
            Some("Participant Alpha")
        );
        assert_eq!(audit[1].resolved_person_id.as_deref(), Some("person-beta"));
        assert_eq!(audit[1].override_id.as_deref(), Some(override_id.as_str()));
        assert_eq!(
            audit[1].resolved_person_display_name.as_deref(),
            Some("Participant Beta")
        );
        let chunk: String = sqlx::query_scalar(
            "SELECT transcript_text FROM transcript_chunks WHERE meeting_id = 'm1'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(chunk, "我们讨论CGS\n人工覆盖第二段");
    }

    #[tokio::test]
    async fn edit_during_run_causes_conflict_and_preserves_human_text() {
        let db = database().await;
        insert_meeting(db.pool(), "m1", "运行开始时的文字").await;
        let run = complete(
            db.pool(),
            "m1",
            "conflict",
            &candidate(&[("S01", "MOSS 候选")]),
        )
        .await;
        sqlx::query(
            "UPDATE transcripts SET transcript = '运行期间人工编辑' WHERE meeting_id = 'm1'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let error = MossCandidateRepository::activate_candidate(db.pool(), &run.run_id)
            .await
            .unwrap_err();
        assert!(matches!(error, MossStoreError::TranscriptConflict { .. }));
        assert_eq!(original_text(db.pool(), "m1").await, "运行期间人工编辑");
        let activations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM moss_activation_snapshots")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(activations, 0);
    }

    #[tokio::test]
    async fn activation_fault_rolls_back_delete_and_candidate_writes() {
        let db = database().await;
        insert_meeting(db.pool(), "m1", "事务前原文").await;
        let before = MossCandidateRepository::current_transcript_sha256(db.pool(), "m1")
            .await
            .unwrap();
        let run = complete(db.pool(), "m1", "fault", &candidate(&[("S01", "不应提交")])).await;
        let error = MossCandidateRepository::activate_candidate_internal(
            db.pool(),
            &run.run_id,
            MutationFault::AfterTranscriptDelete,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, MossStoreError::InjectedFailure));
        assert_eq!(original_text(db.pool(), "m1").await, "事务前原文");
        assert_eq!(
            MossCandidateRepository::current_transcript_sha256(db.pool(), "m1")
                .await
                .unwrap(),
            before
        );
        let activations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM moss_activation_snapshots")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(activations, 0);
    }

    #[tokio::test]
    async fn rollback_restores_exact_snapshot_and_can_reactivate_same_candidate() {
        let db = database().await;
        insert_meeting(db.pool(), "m1", "含摘要字段的原文").await;
        let before_hash = MossCandidateRepository::current_transcript_sha256(db.pool(), "m1")
            .await
            .unwrap();
        let run = complete(
            db.pool(),
            "m1",
            "rollback",
            &candidate(&[("S01", "激活候选")]),
        )
        .await;
        let first_activation = MossCandidateRepository::activate_candidate(db.pool(), &run.run_id)
            .await
            .unwrap();
        assert_ne!(first_activation.activated_transcript_sha256, before_hash);
        let rollback = MossCandidateRepository::rollback_active_activation(db.pool(), "m1")
            .await
            .unwrap();
        assert_eq!(rollback.restored_transcript_sha256, before_hash);
        let restored: (
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = sqlx::query_as(
            r#"
                SELECT transcript, summary, action_items, key_points, speaker
                FROM transcripts WHERE meeting_id = 'm1'
                "#,
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(restored.0, "含摘要字段的原文");
        assert_eq!(restored.1.as_deref(), Some("preserved summary"));
        assert_eq!(restored.2.as_deref(), Some("preserved action"));
        assert_eq!(restored.3.as_deref(), Some("preserved key point"));
        assert_eq!(restored.4.as_deref(), Some("microphone"));
        let second_activation = MossCandidateRepository::activate_candidate(db.pool(), &run.run_id)
            .await
            .unwrap();
        assert_ne!(
            first_activation.activation_id,
            second_activation.activation_id
        );
    }

    #[tokio::test]
    async fn rollback_conflict_and_rollback_fault_both_preserve_current_transcript() {
        let db = database().await;
        insert_meeting(db.pool(), "conflict", "回退前原文").await;
        let conflict_run = complete(
            db.pool(),
            "conflict",
            "rollback-conflict",
            &candidate(&[("S01", "激活后文字")]),
        )
        .await;
        MossCandidateRepository::activate_candidate(db.pool(), &conflict_run.run_id)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE transcripts SET transcript = '激活后的人工编辑' WHERE meeting_id = 'conflict'",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let conflict = MossCandidateRepository::rollback_active_activation(db.pool(), "conflict")
            .await
            .unwrap_err();
        assert!(matches!(conflict, MossStoreError::RollbackConflict { .. }));
        assert_eq!(
            original_text(db.pool(), "conflict").await,
            "激活后的人工编辑"
        );

        insert_meeting(db.pool(), "fault", "故障注入原文").await;
        let fault_run = complete(
            db.pool(),
            "fault",
            "rollback-fault",
            &candidate(&[("S01", "故障注入候选")]),
        )
        .await;
        let activation = MossCandidateRepository::activate_candidate(db.pool(), &fault_run.run_id)
            .await
            .unwrap();
        let error = MossCandidateRepository::rollback_active_activation_internal(
            db.pool(),
            "fault",
            MutationFault::AfterTranscriptDelete,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, MossStoreError::InjectedFailure));
        assert_eq!(original_text(db.pool(), "fault").await, "故障注入候选");
        let status: String = sqlx::query_scalar(
            "SELECT status FROM moss_activation_snapshots WHERE activation_id = ?",
        )
        .bind(activation.activation_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(status, "active");
    }

    #[tokio::test]
    async fn only_one_activation_can_be_active_but_other_candidates_remain_saved() {
        let db = database().await;
        insert_meeting(db.pool(), "m1", "共同来源").await;
        let first = complete(
            db.pool(),
            "m1",
            "first-active",
            &candidate(&[("S01", "第一个候选")]),
        )
        .await;
        let second = complete(
            db.pool(),
            "m1",
            "second-active",
            &candidate(&[("S01", "第二个候选")]),
        )
        .await;
        MossCandidateRepository::activate_candidate(db.pool(), &first.run_id)
            .await
            .unwrap();
        let error = MossCandidateRepository::activate_candidate(db.pool(), &second.run_id)
            .await
            .unwrap_err();
        assert!(matches!(error, MossStoreError::ActiveActivationExists));
        let new_run =
            MossCandidateRepository::start_run(db.pool(), run_input("m1", "after-activation"))
                .await
                .unwrap_err();
        assert!(matches!(new_run, MossStoreError::ActiveActivationExists));
        let run_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM moss_transcription_runs WHERE meeting_id = 'm1' AND status = 'completed'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(run_count, 2);
    }

    #[tokio::test]
    async fn correction_revert_is_lifo_and_restores_prior_materialization() {
        let db = database().await;
        insert_meeting(db.pool(), "m1", "原文").await;
        let run = complete(
            db.pool(),
            "m1",
            "correction-revert",
            &candidate(&[("S01", "西吉艾斯项目")]),
        )
        .await;
        let segment = segment_id(db.pool(), &run.run_id, 0).await;
        let first = MossCandidateRepository::add_term_correction(
            db.pool(),
            &segment,
            TermCorrectionInput {
                start_char: 0,
                end_char: 4,
                original_text: "西吉艾斯".to_owned(),
                replacement_text: "CGS".to_owned(),
                rule_id: "TERM_ALIAS_CGS".to_owned(),
                expected_context_sha256: run.context_sha256.clone(),
            },
        )
        .await
        .unwrap();
        let second = MossCandidateRepository::add_term_correction(
            db.pool(),
            &segment,
            TermCorrectionInput {
                start_char: 3,
                end_char: 5,
                original_text: "项目".to_owned(),
                replacement_text: "计划".to_owned(),
                rule_id: "TERM_ALIAS_PROJECT".to_owned(),
                expected_context_sha256: run.context_sha256.clone(),
            },
        )
        .await
        .unwrap();
        let non_latest =
            MossCandidateRepository::revert_latest_term_correction(db.pool(), &first.correction_id)
                .await
                .unwrap_err();
        assert!(matches!(non_latest, MossStoreError::InvalidInput(_)));
        MossCandidateRepository::revert_latest_term_correction(db.pool(), &second.correction_id)
            .await
            .unwrap();
        MossCandidateRepository::activate_candidate(db.pool(), &run.run_id)
            .await
            .unwrap();
        assert_eq!(original_text(db.pool(), "m1").await, "CGS项目");
    }

    #[tokio::test]
    async fn explicit_meeting_delete_cascades_its_moss_history_without_touching_other_meetings() {
        let db = database().await;
        insert_meeting(db.pool(), "delete-me", "待删除会议").await;
        insert_meeting(db.pool(), "keep-me", "保留会议").await;
        let run = complete(
            db.pool(),
            "delete-me",
            "delete-cascade",
            &candidate(&[("S01", "待删除候选")]),
        )
        .await;
        MossCandidateRepository::activate_candidate(db.pool(), &run.run_id)
            .await
            .unwrap();
        sqlx::query("DELETE FROM meetings WHERE id = 'delete-me'")
            .execute(db.pool())
            .await
            .unwrap();
        let run_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM moss_transcription_runs")
            .fetch_one(db.pool())
            .await
            .unwrap();
        let activation_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM moss_activation_snapshots")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(run_count, 0);
        assert_eq!(activation_count, 0);
        assert_eq!(original_text(db.pool(), "keep-me").await, "保留会议");
    }
}
