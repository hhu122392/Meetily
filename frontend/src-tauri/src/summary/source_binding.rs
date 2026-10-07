//! Authoritative summary-input binding and provenance.
//!
//! P5 deliberately keeps this module independent from the P3 persistence
//! schema. P3 only has to adapt its active transcript-version record into
//! [`TranscriptVersionSnapshot`]. Every caller then gets the same fail-closed
//! checks, canonical hashes, rendering rules, staleness evaluation, and field
//! evidence references.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;
use once_cell::sync::Lazy;
use regex::Regex;
pub use super::field_schema::SummaryTraceField;
use super::field_schema::{advance_fence, ambiguous_dependency_blocker, cell_field_values, field_labels, inline_field_ranges, inline_labels, is_task_label, label_fields, table_cells, table_header, table_separator, ACTION_FIELDS, TASK_LABELS};

pub const SUMMARY_SOURCE_BINDING_SCHEMA_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptSourceKind {
    Whisper,
    SenseVoice,
    Parakeet,
    Moss,
    Manual,
}

impl TranscriptSourceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Whisper => "whisper",
            Self::SenseVoice => "sensevoice",
            Self::Parakeet => "parakeet",
            Self::Moss => "moss",
            Self::Manual => "manual",
        }
    }

    /// Maps the configured local transcription provider onto the source kind
    /// recorded as summary provenance. Remote/unknown providers keep the
    /// historical `whisper` label so existing data and the remote Whisper
    /// pipeline keep reading the same value.
    pub fn from_provider(provider: &str) -> Self {
        match provider.trim().to_ascii_lowercase().as_str() {
            "sensevoice" => Self::SenseVoice,
            "parakeet" => Self::Parakeet,
            _ => Self::Whisper,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptVersionState {
    Candidate,
    Active,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranscriptEvidenceSegment {
    pub segment_id: String,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub wall_clock: Option<String>,
    pub anonymous_speaker: Option<String>,
    pub bound_person_id: Option<String>,
    pub bound_display_name: Option<String>,
    pub text: String,
}

impl TranscriptEvidenceSegment {
    fn effective_speaker(&self) -> Option<&str> {
        self.bound_display_name
            .as_deref()
            .or(self.anonymous_speaker.as_deref())
    }
}

/// P3-facing immutable view of one transcript version.
///
/// `transcript_sha256` hashes text, timing, segment identity, and the original
/// anonymous speaker label. Human bindings are intentionally hashed separately
/// so an old summary can say precisely which source changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranscriptVersionSnapshot {
    pub schema_version: u8,
    pub meeting_id: String,
    pub transcript_version_id: String,
    pub transcript_version: u64,
    pub source_kind: TranscriptSourceKind,
    pub moss_run_id: Option<String>,
    pub state: TranscriptVersionState,
    pub activated_at: Option<DateTime<Utc>>,
    pub transcript_sha256: String,
    pub speaker_binding_snapshot_id: String,
    pub speaker_binding_version: u64,
    pub speaker_binding_sha256: String,
    pub segments: Vec<TranscriptEvidenceSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SummaryTemplateBinding {
    pub template_id: String,
    pub template_version: u64,
    pub template_file_sha256: String,
    pub template_semantic_sha256: String,
}

/// Small, non-content lineage record persisted with a summary generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SummarySourceBinding {
    pub schema_version: u8,
    pub meeting_id: String,
    pub transcript_version_id: String,
    pub transcript_version: u64,
    pub transcript_source: TranscriptSourceKind,
    pub moss_run_id: Option<String>,
    pub transcript_activated_at: Option<DateTime<Utc>>,
    pub transcript_sha256: String,
    pub speaker_binding_snapshot_id: String,
    pub speaker_binding_version: u64,
    pub speaker_binding_sha256: String,
    pub template: SummaryTemplateBinding,
}

/// Exact transcript evidence used by one fact-validation pass. Generated
/// summaries also carry [`SummarySourceBinding`], which adds the template.
/// Manual edits use this smaller record so their field traces cannot be
/// mistaken for evidence from the original generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranscriptEvidenceBinding {
    pub schema_version: u8,
    pub meeting_id: String,
    pub transcript_version_id: String,
    pub transcript_version: u64,
    pub transcript_source: TranscriptSourceKind,
    pub moss_run_id: Option<String>,
    pub transcript_activated_at: Option<DateTime<Utc>>,
    pub transcript_sha256: String,
    pub speaker_binding_snapshot_id: String,
    pub speaker_binding_version: u64,
    pub speaker_binding_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedSummaryInput {
    pub transcript_text: String,
    pub source: TranscriptVersionSnapshot,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SummarySourceBindingError {
    #[error("SUMMARY_SOURCE_MEETING_ID_INVALID")]
    MeetingIdInvalid,
    #[error("SUMMARY_SOURCE_NO_ACTIVE_VERSION")]
    NoActiveVersion,
    #[error("SUMMARY_SOURCE_MULTIPLE_ACTIVE_VERSIONS")]
    MultipleActiveVersions,
    #[error("SUMMARY_SOURCE_VERSION_INVALID")]
    VersionInvalid,
    #[error("SUMMARY_SOURCE_ACTIVATION_INVALID")]
    ActivationInvalid,
    #[error("SUMMARY_SOURCE_MOSS_RUN_INVALID")]
    MossRunInvalid,
    #[error("SUMMARY_SOURCE_SEGMENT_INVALID")]
    SegmentInvalid,
    #[error("SUMMARY_SOURCE_TRANSCRIPT_HASH_MISMATCH")]
    TranscriptHashMismatch,
    #[error("SUMMARY_SOURCE_BINDING_HASH_MISMATCH")]
    SpeakerBindingHashMismatch,
    #[error("SUMMARY_SOURCE_TEMPLATE_INVALID")]
    TemplateInvalid,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TranscriptHashSegment<'a> {
    segment_id: &'a str,
    start_ms: Option<u64>,
    end_ms: Option<u64>,
    wall_clock: Option<&'a str>,
    anonymous_speaker: Option<&'a str>,
    text: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SpeakerBindingHashSegment<'a> {
    segment_id: &'a str,
    anonymous_speaker: Option<&'a str>,
    bound_person_id: Option<&'a str>,
    bound_display_name: Option<&'a str>,
}

impl TranscriptVersionSnapshot {
    /// Builds the P5 view of one immutable P3 activation. The activation id is
    /// the transcript and speaker-binding snapshot id because P3 freezes both
    /// in the same transaction. Hashes are recomputed from the frozen segment
    /// rows and validated before the source can be used by a summary.
    pub fn from_moss_activation(
        meeting_id: impl Into<String>,
        activation_id: impl Into<String>,
        activation_version: u64,
        moss_run_id: impl Into<String>,
        activated_at: DateTime<Utc>,
        segments: Vec<TranscriptEvidenceSegment>,
    ) -> Result<Self, SummarySourceBindingError> {
        let mut source = Self {
            schema_version: SUMMARY_SOURCE_BINDING_SCHEMA_VERSION,
            meeting_id: meeting_id.into(),
            transcript_version_id: activation_id.into(),
            transcript_version: activation_version,
            source_kind: TranscriptSourceKind::Moss,
            moss_run_id: Some(moss_run_id.into()),
            state: TranscriptVersionState::Active,
            activated_at: Some(activated_at),
            transcript_sha256: String::new(),
            speaker_binding_snapshot_id: String::new(),
            speaker_binding_version: activation_version,
            speaker_binding_sha256: String::new(),
            segments,
        };
        source.speaker_binding_snapshot_id = source.transcript_version_id.clone();
        source.transcript_sha256 = source.computed_transcript_sha256();
        source.speaker_binding_sha256 = source.computed_speaker_binding_sha256();
        source.validate_active()?;
        Ok(source)
    }

    pub fn computed_transcript_sha256(&self) -> String {
        let projection = self
            .segments
            .iter()
            .map(|segment| TranscriptHashSegment {
                segment_id: segment.segment_id.trim(),
                start_ms: segment.start_ms,
                end_ms: segment.end_ms,
                wall_clock: clean_optional(segment.wall_clock.as_deref()),
                anonymous_speaker: clean_optional(segment.anonymous_speaker.as_deref()),
                text: segment.text.trim(),
            })
            .collect::<Vec<_>>();
        canonical_json_sha256(&projection)
    }

    pub fn computed_speaker_binding_sha256(&self) -> String {
        let projection = self
            .segments
            .iter()
            .map(|segment| SpeakerBindingHashSegment {
                segment_id: segment.segment_id.trim(),
                anonymous_speaker: clean_optional(segment.anonymous_speaker.as_deref()),
                bound_person_id: clean_optional(segment.bound_person_id.as_deref()),
                bound_display_name: clean_optional(segment.bound_display_name.as_deref()),
            })
            .collect::<Vec<_>>();
        canonical_json_sha256(&projection)
    }

    /// Builds the pre-P3 compatibility source. It is still explicit and
    /// authoritative: current SQLite transcript rows become the one active
    /// Whisper version, while any WebView-supplied text is ignored by P5.
    pub fn legacy_whisper(
        meeting_id: impl Into<String>,
        segments: Vec<TranscriptEvidenceSegment>,
    ) -> Self {
        Self::legacy_local(meeting_id, TranscriptSourceKind::Whisper, segments)
    }

    /// Builds the pre-P3 compatibility source for whichever local engine
    /// actually produced the current transcript rows (Whisper / SenseVoice /
    /// Parakeet). The version id keeps the historical `legacy_` prefix so old
    /// summaries stay comparable, but names the real engine.
    pub fn legacy_local(
        meeting_id: impl Into<String>,
        source_kind: TranscriptSourceKind,
        segments: Vec<TranscriptEvidenceSegment>,
    ) -> Self {
        let meeting_id = meeting_id.into();
        let mut source = Self {
            schema_version: SUMMARY_SOURCE_BINDING_SCHEMA_VERSION,
            meeting_id: meeting_id.clone(),
            transcript_version_id: format!("legacy_{}_{meeting_id}", source_kind.as_str()),
            transcript_version: 1,
            source_kind,
            moss_run_id: None,
            state: TranscriptVersionState::Active,
            // Pre-P3 rows have no activation event. Keeping this null is more
            // accurate than inventing one from meeting creation/update time.
            activated_at: None,
            transcript_sha256: String::new(),
            speaker_binding_snapshot_id: format!("legacy_unbound_{meeting_id}"),
            speaker_binding_version: 1,
            speaker_binding_sha256: String::new(),
            segments,
        };
        source.transcript_sha256 = source.computed_transcript_sha256();
        source.speaker_binding_sha256 = source.computed_speaker_binding_sha256();
        source
    }

    pub fn validate_active(&self) -> Result<(), SummarySourceBindingError> {
        if self.schema_version != SUMMARY_SOURCE_BINDING_SCHEMA_VERSION
            || !valid_identifier(&self.meeting_id, 200)
        {
            return Err(SummarySourceBindingError::MeetingIdInvalid);
        }
        if !valid_identifier(&self.transcript_version_id, 200)
            || self.transcript_version == 0
            || !valid_identifier(&self.speaker_binding_snapshot_id, 200)
            || self.speaker_binding_version == 0
        {
            return Err(SummarySourceBindingError::VersionInvalid);
        }
        if self.state != TranscriptVersionState::Active {
            return Err(SummarySourceBindingError::ActivationInvalid);
        }
        if self.source_kind == TranscriptSourceKind::Moss && self.activated_at.is_none() {
            return Err(SummarySourceBindingError::ActivationInvalid);
        }
        match self.source_kind {
            TranscriptSourceKind::Whisper
            | TranscriptSourceKind::SenseVoice
            | TranscriptSourceKind::Parakeet
            | TranscriptSourceKind::Manual
                if self.moss_run_id.is_some() =>
            {
                return Err(SummarySourceBindingError::MossRunInvalid)
            }
            TranscriptSourceKind::Moss
                if !self
                    .moss_run_id
                    .as_deref()
                    .is_some_and(|value| valid_identifier(value, 200)) =>
            {
                return Err(SummarySourceBindingError::MossRunInvalid)
            }
            _ => {}
        }
        if self.segments.is_empty() {
            return Err(SummarySourceBindingError::SegmentInvalid);
        }
        let mut ids = BTreeSet::new();
        let mut previous_start = None;
        for segment in &self.segments {
            let id = segment.segment_id.trim();
            if !valid_identifier(id, 200)
                || !ids.insert(id.to_owned())
                || segment.text.trim().is_empty()
                || segment.text.chars().any(is_forbidden_control)
                || segment
                    .wall_clock
                    .as_deref()
                    .is_some_and(|value| value.chars().any(is_forbidden_control))
                || !paired_optional_text(
                    segment.bound_person_id.as_deref(),
                    segment.bound_display_name.as_deref(),
                )
            {
                return Err(SummarySourceBindingError::SegmentInvalid);
            }
            match (segment.start_ms, segment.end_ms) {
                (Some(start), Some(end)) if start <= end => {
                    if previous_start.is_some_and(|previous| start < previous) {
                        return Err(SummarySourceBindingError::SegmentInvalid);
                    }
                    previous_start = Some(start);
                }
                (Some(start), None) => {
                    if previous_start.is_some_and(|previous| start < previous) {
                        return Err(SummarySourceBindingError::SegmentInvalid);
                    }
                    previous_start = Some(start);
                }
                (None, None) if self.source_kind != TranscriptSourceKind::Moss => {}
                _ => return Err(SummarySourceBindingError::SegmentInvalid),
            }
            if segment
                .anonymous_speaker
                .as_deref()
                .is_some_and(|speaker| !valid_anonymous_speaker(speaker))
                || (self.source_kind == TranscriptSourceKind::Moss
                    && !segment
                        .anonymous_speaker
                        .as_deref()
                        .is_some_and(valid_anonymous_speaker))
            {
                return Err(SummarySourceBindingError::SegmentInvalid);
            }
        }
        if !is_sha256(&self.transcript_sha256)
            || self.computed_transcript_sha256() != self.transcript_sha256
        {
            return Err(SummarySourceBindingError::TranscriptHashMismatch);
        }
        if !is_sha256(&self.speaker_binding_sha256)
            || self.computed_speaker_binding_sha256() != self.speaker_binding_sha256
        {
            return Err(SummarySourceBindingError::SpeakerBindingHashMismatch);
        }
        Ok(())
    }

    pub fn render_for_summary(&self) -> Result<String, SummarySourceBindingError> {
        self.validate_active()?;
        Ok(self
            .segments
            .iter()
            .map(|segment| {
                let time = segment
                    .start_ms
                    .map(format_timestamp)
                    .or_else(|| clean_optional(segment.wall_clock.as_deref()).map(str::to_owned));
                let mut prefix = String::new();
                if let Some(time) = time {
                    prefix.push_str(&time);
                    prefix.push(' ');
                }
                if let Some(speaker) = segment.effective_speaker() {
                    prefix.push_str(speaker.trim());
                    prefix.push_str(": ");
                }
                format!("{prefix}{}", segment.text.trim())
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    pub fn evidence_binding(&self) -> Result<TranscriptEvidenceBinding, SummarySourceBindingError> {
        self.validate_active()?;
        Ok(TranscriptEvidenceBinding {
            schema_version: SUMMARY_SOURCE_BINDING_SCHEMA_VERSION,
            meeting_id: self.meeting_id.clone(),
            transcript_version_id: self.transcript_version_id.clone(),
            transcript_version: self.transcript_version,
            transcript_source: self.source_kind,
            moss_run_id: self.moss_run_id.clone(),
            transcript_activated_at: self.activated_at,
            transcript_sha256: self.transcript_sha256.clone(),
            speaker_binding_snapshot_id: self.speaker_binding_snapshot_id.clone(),
            speaker_binding_version: self.speaker_binding_version,
            speaker_binding_sha256: self.speaker_binding_sha256.clone(),
        })
    }
}

impl SummaryTemplateBinding {
    pub fn validate(&self) -> Result<(), SummarySourceBindingError> {
        if !valid_identifier(&self.template_id, 200)
            || self.template_version == 0
            || !is_sha256(&self.template_file_sha256)
            || !is_sha256(&self.template_semantic_sha256)
        {
            return Err(SummarySourceBindingError::TemplateInvalid);
        }
        Ok(())
    }
}

impl SummarySourceBinding {
    pub fn from_active_source(
        source: &TranscriptVersionSnapshot,
        template: SummaryTemplateBinding,
    ) -> Result<Self, SummarySourceBindingError> {
        source.validate_active()?;
        template.validate()?;
        Ok(Self {
            schema_version: SUMMARY_SOURCE_BINDING_SCHEMA_VERSION,
            meeting_id: source.meeting_id.clone(),
            transcript_version_id: source.transcript_version_id.clone(),
            transcript_version: source.transcript_version,
            transcript_source: source.source_kind,
            moss_run_id: source.moss_run_id.clone(),
            transcript_activated_at: source.activated_at,
            transcript_sha256: source.transcript_sha256.clone(),
            speaker_binding_snapshot_id: source.speaker_binding_snapshot_id.clone(),
            speaker_binding_version: source.speaker_binding_version,
            speaker_binding_sha256: source.speaker_binding_sha256.clone(),
            template,
        })
    }

    pub fn validate_lineage(&self) -> Result<(), SummarySourceBindingError> {
        validate_transcript_lineage(
            self.schema_version,
            &self.meeting_id,
            &self.transcript_version_id,
            self.transcript_version,
            self.transcript_source,
            self.moss_run_id.as_deref(),
            self.transcript_activated_at.as_ref(),
            &self.transcript_sha256,
            &self.speaker_binding_snapshot_id,
            self.speaker_binding_version,
            &self.speaker_binding_sha256,
        )?;
        self.template.validate()
    }
}

impl TranscriptEvidenceBinding {
    pub fn validate_lineage(&self) -> Result<(), SummarySourceBindingError> {
        validate_transcript_lineage(
            self.schema_version,
            &self.meeting_id,
            &self.transcript_version_id,
            self.transcript_version,
            self.transcript_source,
            self.moss_run_id.as_deref(),
            self.transcript_activated_at.as_ref(),
            &self.transcript_sha256,
            &self.speaker_binding_snapshot_id,
            self.speaker_binding_version,
            &self.speaker_binding_sha256,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_transcript_lineage(
    schema_version: u8,
    meeting_id: &str,
    transcript_version_id: &str,
    transcript_version: u64,
    transcript_source: TranscriptSourceKind,
    moss_run_id: Option<&str>,
    transcript_activated_at: Option<&DateTime<Utc>>,
    transcript_sha256: &str,
    speaker_binding_snapshot_id: &str,
    speaker_binding_version: u64,
    speaker_binding_sha256: &str,
) -> Result<(), SummarySourceBindingError> {
    if schema_version != SUMMARY_SOURCE_BINDING_SCHEMA_VERSION
        || !valid_identifier(meeting_id, 200)
        || !valid_identifier(transcript_version_id, 200)
        || transcript_version == 0
        || !valid_identifier(speaker_binding_snapshot_id, 200)
        || speaker_binding_version == 0
        || !is_sha256(transcript_sha256)
        || !is_sha256(speaker_binding_sha256)
    {
        return Err(SummarySourceBindingError::VersionInvalid);
    }
    if transcript_source == TranscriptSourceKind::Moss && transcript_activated_at.is_none() {
        return Err(SummarySourceBindingError::ActivationInvalid);
    }
    match transcript_source {
        TranscriptSourceKind::Whisper
        | TranscriptSourceKind::SenseVoice
        | TranscriptSourceKind::Parakeet
        | TranscriptSourceKind::Manual
            if moss_run_id.is_some() =>
        {
            Err(SummarySourceBindingError::MossRunInvalid)
        }
        TranscriptSourceKind::Moss
            if !moss_run_id.is_some_and(|value| valid_identifier(value, 200)) =>
        {
            Err(SummarySourceBindingError::MossRunInvalid)
        }
        _ => Ok(()),
    }
}

pub fn select_activated_summary_source(
    meeting_id: &str,
    versions: &[TranscriptVersionSnapshot],
) -> Result<ValidatedSummaryInput, SummarySourceBindingError> {
    if !valid_identifier(meeting_id, 200) {
        return Err(SummarySourceBindingError::MeetingIdInvalid);
    }
    let active = versions
        .iter()
        .filter(|version| {
            version.meeting_id == meeting_id && version.state == TranscriptVersionState::Active
        })
        .collect::<Vec<_>>();
    let source = match active.as_slice() {
        [] => return Err(SummarySourceBindingError::NoActiveVersion),
        [source] => *source,
        _ => return Err(SummarySourceBindingError::MultipleActiveVersions),
    };
    Ok(ValidatedSummaryInput {
        transcript_text: source.render_for_summary()?,
        source: source.clone(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SummaryStaleReason {
    TranscriptVersionChanged,
    TranscriptContentChanged,
    SpeakerBindingsChanged,
    TemplateChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryFreshnessStatus {
    Current,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryFreshness {
    pub status: SummaryFreshnessStatus,
    pub reasons: Vec<SummaryStaleReason>,
}

pub fn evaluate_summary_freshness(
    generated_from: &SummarySourceBinding,
    current: &SummarySourceBinding,
) -> Result<SummaryFreshness, SummarySourceBindingError> {
    if generated_from.meeting_id != current.meeting_id {
        return Err(SummarySourceBindingError::MeetingIdInvalid);
    }
    generated_from.validate_lineage()?;
    current.validate_lineage()?;
    let mut reasons = BTreeSet::new();
    if generated_from.transcript_version_id != current.transcript_version_id
        || generated_from.transcript_version != current.transcript_version
        || generated_from.transcript_source != current.transcript_source
        || generated_from.moss_run_id != current.moss_run_id
    {
        reasons.insert(SummaryStaleReason::TranscriptVersionChanged);
    }
    if generated_from.transcript_sha256 != current.transcript_sha256 {
        reasons.insert(SummaryStaleReason::TranscriptContentChanged);
    }
    if generated_from.speaker_binding_snapshot_id != current.speaker_binding_snapshot_id
        || generated_from.speaker_binding_version != current.speaker_binding_version
        || generated_from.speaker_binding_sha256 != current.speaker_binding_sha256
    {
        reasons.insert(SummaryStaleReason::SpeakerBindingsChanged);
    }
    if generated_from.template != current.template {
        reasons.insert(SummaryStaleReason::TemplateChanged);
    }
    let reasons = reasons.into_iter().collect::<Vec<_>>();
    Ok(SummaryFreshness {
        status: if reasons.is_empty() {
            SummaryFreshnessStatus::Current
        } else {
            SummaryFreshnessStatus::Stale
        },
        reasons,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryTraceStatus {
    Supported,
    NeedsReview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryEvidenceReference {
    pub segment_id: String,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub excerpt_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryFieldTrace {
    pub field: SummaryTraceField,
    pub value: String,
    pub markdown_line: usize,
    pub markdown_column: Option<usize>,
    pub status: SummaryTraceStatus,
    pub evidence: Vec<SummaryEvidenceReference>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub task: String,
    /// Literal task anchor; display notes never become field evidence.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_task: String,
    /// Nearby task evidence is a review aid, never proof of the field value.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_evidence: Vec<SummaryEvidenceReference>,
}

/// Finds reviewable evidence for recognized action fields. This is not a
/// semantic-entailment claim: unsupported values are explicitly marked for
/// review, while supported values carry stable segment/timestamp references.
pub fn trace_owner_and_time_fields(
    markdown: &str,
    source: &TranscriptVersionSnapshot,
) -> Result<Vec<SummaryFieldTrace>, SummarySourceBindingError> {
    trace_action_fields(markdown, source, None)
}

/// A dictionary changes spelling, never assignments. References always retain
/// the original source text/hash; the normalized comparison view is not saved.
pub fn trace_action_fields_with_spelling(
    markdown: &str,
    source: &TranscriptVersionSnapshot,
    normalize: impl Fn(&str) -> String,
) -> Result<Vec<SummaryFieldTrace>, SummarySourceBindingError> {
    source.validate_active()?;
    let mut comparison = source.clone();
    for segment in &mut comparison.segments { segment.text = normalize(&segment.text); }
    comparison.transcript_sha256 = comparison.computed_transcript_sha256();
    let mut traces = trace_owner_and_time_fields(markdown, &comparison)?;
    for trace in &mut traces {
        for reference in trace.evidence.iter_mut().chain(trace.related_evidence.iter_mut()) {
            if let Some(segment) = source.segments.iter().find(|segment| segment.segment_id == reference.segment_id) {
                *reference = source_reference(segment);
            }
        }
        if !trace.source_task.is_empty() && !source.segments.iter().any(|segment| task_matches(&segment.text, &trace.source_task)) {
            let spellings = source.segments.iter().flat_map(|segment| literal_source_actions(&segment.text))
                .filter(|task| normalize_task(&normalize(task)) == trace.source_task).collect::<BTreeSet<_>>();
            if spellings.len() == 1 { trace.source_task = spellings.into_iter().next().unwrap(); }
            else { trace.source_task.clear(); trace.status = SummaryTraceStatus::NeedsReview; trace.related_evidence.extend(trace.evidence.drain(..)); }
        }
    }
    Ok(traces)
}

/// Generation alone can recover missing slots, using the same proof as filled slots.
pub fn recover_missing_action_fields(markdown: &str, source: &TranscriptVersionSnapshot, owners: &[String]) -> Result<String, SummarySourceBindingError> {
    source.validate_active()?;
    let markdown = canonicalize_generated_action_tasks(markdown, source);
    let markdown = recover_missing_action_rows(&markdown, source, owners);
    let traces = trace_action_fields(&markdown, source, Some(owners))?;
    Ok(restore_supported_owner_and_time_fields(&markdown, &markdown, &traces))
}

static SOURCE_ACTION: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)修订|修正|提交|送交|完成|发布|发送|整理|制定|更新|交付|研发|开展|持续营造|持续沟通|协调|[与和][^，,。；;（(]+?(?:沟通|讨论|协调|说明)|\b(?:build|create|fix|test|deploy|review|send|submit|prepare|write|update)\b").unwrap());

#[cfg(test)]
mod unpunctuated_import_tests {
    use super::*;

    fn imported_source(text: &str) -> TranscriptVersionSnapshot {
        TranscriptVersionSnapshot::legacy_local("whisper-real-case", TranscriptSourceKind::Whisper, vec![TranscriptEvidenceSegment {
            segment_id:"native-whisper".into(), start_ms:Some(522440), end_ms:Some(547910), wall_clock:None,
            anonymous_speaker:None, bound_person_id:None, bound_display_name:None, text:text.into(),
        }])
    }

    #[test]
    fn budget_description_cannot_borrow_an_earlier_plan_assignment() {
        let source = imported_source("卫福部是进一步提出了调整方案 院长也提出了三项的原则 请卫福部进行计划调整 第一要增加偏箱医疗的巡回医疗量能 第二现有的预算相关预算经费中 也可以用在偏箱包含卫生所 牙科设备的更新还有采购 还有薪资的保障 第三对于这个55个无牙医的偏箱");
        let task = "更新还有采购 还有薪资的保障 第三对于这个55个无牙医的偏箱";
        assert!(!source_action_is_assigned(task, &[task.into()], &source));
        assert!(source_action_values(task, &[task.into()], &source).is_empty());
    }

    #[test]
    fn explicit_communication_keeps_owner_without_later_policy_condition() {
        let source = imported_source("好 临时动一二院长就推动学校导入国产可溯源乳品专案实施计划 在未来的规划与执行精进进行了提示 院长表示对于农业部建议新年度可以思考由中央提供预算 地方政府依据实际需求来执行的新构想 农业部要和地方政府来进行充分的说明 如果所有的地方政府都愿意承诺的话 相关政策才能够继续的推动下去");
        let task = "和地方政府来进行充分的说明";
        assert!(literal_source_actions(&source.segments[0].text).contains(&task.to_owned()));
        assert!(has_local_action_assignment(task, &source.segments[0].text));
        assert_eq!(source_action_values(task, &[task.into()], &source), vec![(SummaryTraceField::Owner, "农业部".into())]);
        let negative = imported_source("如果农业部要和地方政府来进行充分的说明 地方政府负责提交申请");
        assert!(!source_action_is_assigned("提交申请", &["提交申请".into()], &negative));
    }

    #[test]
    fn spoken_principle_spacing_does_not_name_whole_instruction_owner() {
        // Actual R08an desktop source: ASR puts spaces after ordinal markers.
        let source = imported_source("尽性 卫福部呢 是进步提出了调整方案 院长也提出了三项的原则 请卫福部进行计划调整 第一 要增加偏箱医疗的巡回医疗量能 第二 现有的预算 相关预算经费中呢 也可以用在偏箱包含卫生所 牙科设备的更新 还有采购 还有薪资的保障 第三 对于这个55个无牙医的偏箱 除了鼓励");
        let task = "更新 还有采购 还有薪资的保障";
        assert!(source_action_values(task, &[task.into()], &source).is_empty(),
            "A budget description cannot inherit the preceding plan's assignment");
        assert!(!source_action_is_assigned(task, &[task.into()], &source));
    }

    #[test]
    fn complete_relative_deadline_at_audio_cut_keeps_joint_assignment() {
        let mut source = imported_source("而另外在这个执行过程当中 各县市政府所提出包含这个乳品的品质 还有配送还有冷链等相关的问题 都要顺利找出解决方案 才可以确保学童饮用乳品的品质 让家长们能够安心 而如果执行相关的这个执行问题 无法克服的话 那这个政策是必须要检讨的 另外因为这个新年度即将要来临 院长要求农业部还有教育部 要在一周之内");
        source.segments[0].start_ms = Some(616560);
        source.segments[0].end_ms = Some(644600);
        source.segments.push(TranscriptEvidenceSegment {
            segment_id: "native-whisper-next".into(), start_ms: Some(643800), end_ms: Some(668960),
            text: "与地方政府来妥善沟通 检讨相关问题是否能够进行改善 那么届时会依据沟通还有改善的状况 来做出进一步的政策决定 以上 谢谢发言人 接下来请经济部能源署邮政委署长 说明报告事项第一案 加户屋顶设置太阳光电加速计划报告 发言人 次长 副主委 各位媒体朋友 大家午安 以下由".into(),
            ..source.segments[0].clone()
        });
        let task = "与地方政府来妥善沟通 检讨相关问题是否能够进行改善";
        let values = source_action_values(task, &[task.into()], &source);
        assert!(values.contains(&(SummaryTraceField::Owner, "农业部、教育部".into())), "{values:?}");
        assert!(values.contains(&(SummaryTraceField::Time, "一周之内".into())), "{values:?}");
        source.segments[1].anonymous_speaker = Some("other-speaker".into());
        assert!(!source_action_is_assigned(task, &[task.into()], &source));
        source.segments[1].anonymous_speaker = None;
        source.segments[1].start_ms = Some(646101);
        assert!(!source_action_is_assigned(task, &[task.into()], &source));
        assert!(join_split_relative_deadline("要求在一周之内", "如需核定才执行").is_none());
        assert!(join_split_relative_deadline("要求在一周之内", "另外请经济部提交报告").is_none());
    }
    #[test]
    fn approval_then_implementation_does_not_expand_revision_deadline() {
        // Actual R08ao text, including its unchanged ASR spelling.
        let text = "请卫福部依照上述的三项原则 在一个月内修订优化偏乡医疗计划 送院合递之后呢 至于实施 使计划可以";
        let source = imported_source(text);
        let task = "修订优化偏乡医疗计划";
        assert!(literal_source_actions(text).contains(&task.into()),
            "A later approval/implementation phase is not part of the revision task");
        let values = source_action_values(task, &[task.into()], &source);
        assert!(values.contains(&(SummaryTraceField::Owner, "卫福部".into())), "{values:?}");
        assert!(values.contains(&(SummaryTraceField::Time, "一个月内".into())), "{values:?}");
        assert!(source_action_values("实施 使计划可以", &["实施 使计划可以".into()], &source)
            .iter().all(|(field, _)| *field != SummaryTraceField::Time),
            "An earlier revision deadline cannot govern implementation after approval");
        assert!(literal_source_actions("请建设局在两周内修订方案 送审批准之后据以实施")
            .contains(&"修订方案".into()));
        // A combined assignment without a later implementation phase stays intact.
        assert!(literal_source_actions("请建设局在两周内修订方案 并送交委员会")
            .contains(&"修订方案 并送交委员会".into()));
        let hypothetical = imported_source("如果请建设局在两周内修订方案 送审批准之后据以实施");
        assert!(!source_action_is_assigned("修订方案", &["修订方案".into()], &hypothetical));
    }

}

pub fn literal_source_actions(text: &str) -> Vec<String> {
    evidence_sentences(text).into_iter().flat_map(|sentence| sentence.split(['，', ',', '。', '；', ';', '\n'])).flat_map(|clause| {
        SOURCE_ACTION.find_iter(clause).map(|found| clause[found.start()..].trim().to_owned()).collect::<Vec<_>>()
    }).collect()
}

fn canonicalize_generated_action_tasks(markdown: &str, source: &TranscriptVersionSnapshot) -> String {
    static PERIOD_PREFIX: Lazy<Regex> = Lazy::new(|| Regex::new(r"^在?[一二三四五六七八九十两\d]+(?:工作日|天|周|星期|个月|年)(?:之)?内").unwrap());
    let spoken_key = |text: &str| normalize_task(text).replace("来妥善", "妥善").replace("来进行", "进行");
    let already_anchored = |display: &str, anchor: &str| display.trim().strip_prefix(anchor)
        .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('（') || suffix.starts_with('('));
    let source_actions = source.segments.iter().flat_map(|segment| literal_source_actions(&segment.text)).collect::<BTreeSet<_>>();
    let candidate = |task: &str| {
        // Only an exact action/object, allowing the two grammatical fillers above.
        // Different verbs, objects, phases or synonymous rewrites stay unverified.
        task.split(['，', ',', '；', ';', '（', '(']).find_map(|clause| {
            let text = PERIOD_PREFIX.replace(clause.trim(), "");
            SOURCE_ACTION.find_iter(&text).find_map(|found| {
                if ["不", "未", "如果", "假如", "希望", "预计"].iter().any(|word| text[..found.start()].contains(word)) { return None; }
                let key = spoken_key(&text[found.start()..]);
                if key.chars().count() < 4 || ["持续沟通", "完成任务", "执行任务", "开展工作"].contains(&key.as_str()) { return None; }
                let matches = source_actions.iter().filter(|action| spoken_key(action) == key).collect::<Vec<_>>();
                (matches.len() == 1).then(|| matches[0].clone())
            })
        })
    };
    let mut lines = markdown.lines().map(str::to_owned).collect::<Vec<_>>();
    let mut slots = Vec::new();
    let mut columns = Vec::new();
    let mut fence = None;
    let line_refs = lines.iter().map(String::as_str).collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        if advance_fence(&mut fence, line) || fence.is_some() { columns.clear(); continue; }
        let cells = table_cells(line);
        if table_header(&line_refs, index) {
            columns = cells.iter().enumerate().filter_map(|(column, label)| is_task_label(label).then_some(column)).collect();
        } else if cells.is_empty() {
            columns.clear();
            let labels = inline_labels(line).into_iter().filter(|(label, _, _)| is_task_label(label)).collect::<Vec<_>>();
            if let [(label, start, end)] = labels.as_slice() {
                let _ = label;
                if let Some(anchor) = candidate(&line[*start..*end]) { slots.push((index, None, Some((*start, *end)), anchor)); }
            }
        }
        else if !table_separator(&cells) {
            for column in &columns {
                if let Some(task) = cells.get(*column) {
                    if let Some(anchor) = candidate(task) { slots.push((index, Some(*column), None, anchor)); }
                }
            }
        }
    }
    for (index, column, range, anchor) in &slots {
        // Two differently scoped rows must not collapse into one generic task.
        if slots.iter().filter(|(_, _, _, other)| normalize_task(other) == normalize_task(anchor)).count() != 1 { continue; }
        if let Some(column) = column {
            let mut cells = table_cells(&lines[*index]);
            if !already_anchored(&cells[*column], anchor) {
                cells[*column] = format!("{anchor}（{}）", cells[*column]);
            }
            lines[*index] = format!("| {} |", cells.join(" | "));
        } else if let Some((start, end)) = range {
            let original = &lines[*index][*start..*end];
            if !already_anchored(original, anchor) {
                let display = format!("{anchor}（{original}）");
                lines[*index].replace_range(*start..*end, &display);
            }
        }
    }
    let mut result = lines.join("\n");
    if markdown.ends_with('\n') { result.push('\n'); }
    result
}

/// Only literal, uniquely assigned tasks can add rows to one recognized action table.
/// Unknown columns/layouts and authored saves are never reconstructed.
fn recover_missing_action_rows(markdown: &str, source: &TranscriptVersionSnapshot, owners: &[String]) -> String {
    // Revoked assignments need review; do not infer which earlier task survives.
    if source.segments.iter().any(|segment| ["取消", "撤回", "作废", "暂缓", "不再负责", "不执行", "cancel", "withdrawn"].iter().any(|word| segment.text.to_lowercase().contains(word))) { return markdown.to_owned(); }
    let lines = markdown.lines().collect::<Vec<_>>();
    let mut fence = None;
    let tables = lines.iter().enumerate().filter_map(|(index, line)| {
        if advance_fence(&mut fence, line) || fence.is_some() || !table_header(&lines, index) { return None; }
        let header = table_cells(line);
        let anchors = header.iter().enumerate().filter_map(|(column, label)| is_task_label(label).then_some(column)).collect::<Vec<_>>();
        (!anchors.is_empty()).then_some((index, header, anchors))
    }).collect::<Vec<_>>();
    if tables.len() != 1 { return markdown.to_owned(); }
    let (start, header, anchors) = &tables[0];
    if anchors.len() != 1 || header.len() < 2 || header.iter().enumerate().any(|(column, label)| column != anchors[0]
        && (label_fields(label).is_empty() || label_fields(label).iter().any(|field| matches!(field, SummaryTraceField::Criteria | SummaryTraceField::Escalation)))) {
        return markdown.to_owned();
    }
    let anchor = anchors[0];
    // Decisions are not a catch-all destination for omitted action tasks.
    if ["决策", "结论", "decision"].iter().any(|label| header[anchor].trim().trim_matches('*').trim().eq_ignore_ascii_case(label)) { return markdown.to_owned(); }
    let end = (*start+2..lines.len()).find(|index| table_cells(lines[*index]).is_empty()).unwrap_or(lines.len());
    if lines[*start+2..end].iter().any(|line| table_cells(line).len() != header.len()) { return markdown.to_owned(); }
    let existing = lines[*start+2..end].iter().map(|line| normalize_task(&table_cells(line)[anchor])).collect::<Vec<_>>();
    let mut candidates = Vec::new();
    for sentence in source.segments.iter().flat_map(|segment| evidence_sentences(&segment.text)) {
        if hypothetical_assignment(sentence, &[]) || ["若", "举例", "假定", "尚未确认", "尚未确定", "未决定", "不是说", "仅供"].iter().any(|word| sentence.contains(word)) { continue; }
        for clause in sentence.split(['，', ',']) {
            for owner in owners {
                let Some(tail) = clause.trim().strip_prefix(owner.as_str()) else { continue; };
                let Some(task) = ["负责", "承担", "is responsible for ", "will ", "owns "].iter()
                    .find_map(|role| tail.trim_start().strip_prefix(role)) else { continue; };
                let task = task.trim().trim_end_matches(['。', '.', '！', '!', '；', ';']).trim();
                if !(4..=80).contains(&task.chars().count()) || task.contains(['|', ':', '：'])
                    || ["并", "以及", "和", "或", "可能", "考虑", "建议", "是否", " and ", " or "].iter().any(|word| task.contains(word))
                    || ACTION_FIELDS.iter().flat_map(|field| field_labels(*field)).any(|label| task_matches(task, label)) { continue; }
                let candidate = (task.to_owned(), owner.clone());
                if !candidates.contains(&candidate) { candidates.push(candidate); }
            }
        }
    }
    candidates.retain(|(task, _)| {
        let task = normalize_task(task); let core = task_evidence_key(&task, std::slice::from_ref(&task));
        !existing.iter().any(|task| task_evidence_key(task, std::slice::from_ref(task)) == core)
    });
    let tasks = existing.iter().cloned().chain(candidates.iter().map(|(task, _)| normalize_task(task))).collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>();
    let keys = tasks.iter().map(|task| task_evidence_key(task, &tasks)).collect::<Vec<_>>();
    let mut rows = Vec::new();
    for (task, owner) in candidates {
        let key = task_evidence_key(&normalize_task(&task), &tasks);
        if source.segments.iter().any(|segment| task_matches(&segment.text, &key)
            && ["不负责", "不承担", "没有约定", "未约定", "not responsible"].iter().any(|word| segment.text.to_lowercase().contains(word))) { continue; }
        if recover_field_value(SummaryTraceField::Owner, &[key], &keys, source, owners).as_deref() != Some(owner.as_str()) { continue; }
        let missing = if header.iter().all(|label| label.is_ascii()) { "Not mentioned" } else { "会议未提及" };
        let mut cells = vec![missing.to_owned(); header.len()]; cells[anchor] = task;
        rows.push(format!("| {} |", cells.join(" | ")));
    }
    if rows.is_empty() { return markdown.to_owned(); }
    let mut output = lines[..end].join("\n"); output.push('\n'); output.push_str(&rows.join("\n"));
    if end < lines.len() { output.push('\n'); output.push_str(&lines[end..].join("\n")); }
    if markdown.ends_with('\n') { output.push('\n'); }
    output
}

fn trace_action_fields(markdown: &str, source: &TranscriptVersionSnapshot, recover_owners: Option<&[String]>) -> Result<Vec<SummaryFieldTrace>, SummarySourceBindingError> {
    source.validate_active()?;
    let mut claims = Vec::new();
    let mut tasks = BTreeSet::new();
    let mut table_schema: Option<TableTraceSchema> = None;
    let lines = markdown.lines().collect::<Vec<_>>();
    let mut fence = None;
    for (line_index, line) in lines.iter().enumerate() {
        let markdown_line = line_index + 1;
        if advance_fence(&mut fence, line) || fence.is_some() { table_schema = None; continue; }
        let cells = table_cells(line);
        if !cells.is_empty() {
            if table_header(&lines, line_index) {
                let fields = cells.iter().enumerate().filter_map(|(index, label)| {
                    let fields = label_fields(label); (!fields.is_empty()).then_some((index, fields))
                }).collect::<Vec<_>>();
                let anchors = cells.iter().enumerate().filter_map(|(index, label)| is_task_label(label).then_some(index)).collect();
                table_schema = Some(TableTraceSchema { fields, action_anchor_columns: anchors });
                continue;
            }
            if table_separator(&cells) { continue; }
            if let Some(schema) = table_schema.as_ref() {
                let anchors = schema.action_anchor_columns.iter().filter_map(|column| cells.get(*column))
                    .filter(|value| !is_review_placeholder(value)).cloned().collect::<Vec<_>>();
                tasks.extend(anchors.iter().cloned());
                for (column, fields) in &schema.fields {
                    if let Some(value) = cells.get(*column) {
                        for (field, value) in cell_field_values(fields, value) {
                            if is_field_placeholder(field, value) == recover_owners.is_some() {
                                claims.push((field, value.to_owned(), markdown_line, Some(*column), anchors.clone(), ambiguous_dependency_blocker(fields, cells.get(*column).unwrap())));
                            }
                        }
                    }
                }
            }
            continue;
        }
        table_schema = None;
        let anchors = inline_action_anchors(line);
        tasks.extend(anchors.iter().cloned());
        for (label, start, end) in inline_labels(line) {
            for (field, value) in cell_field_values(&label_fields(label), &line[start..end]) {
                if is_field_placeholder(field, value) == recover_owners.is_some() {
                    claims.push((field, value.to_owned(), markdown_line, None, anchors.clone(), ambiguous_dependency_blocker(&label_fields(label), &line[start..end])));
                }
            }
        }
    }
    let tasks = tasks.into_iter().collect::<Vec<_>>();
    let task_keys = tasks.iter().map(|task| source_task_key(task, &tasks, source)).collect::<Vec<_>>();
    let mut checked = claims.into_iter().map(|(field, value, line, column, anchors, ambiguous)|
        {
            let keys = anchors.iter().map(|task| source_task_key(task, &tasks, source)).collect::<Vec<_>>();
            let recovered = recover_owners.and_then(|owners| recover_field_value(field, &keys, &task_keys, source, owners));
            let mut trace = trace_field_value(field, recovered.as_deref().unwrap_or(&value), line, column, &keys, &task_keys, source);
            trace.task = anchors.join(" / ");
            trace.source_task = if keys.iter().all(|key| source.segments.iter().any(|segment| task_matches(&segment.text, key))) { keys.join(" / ") } else { String::new() };
            (trace, ambiguous)
        });
    let mut traces = Vec::new();
    while let Some((mut trace, ambiguous)) = checked.next() {
        if ambiguous {
            // A bare combined cell gets its meaning from unique source evidence, never from its wording.
            let (other, _) = checked.next().expect("combined dependency/blocker pair");
            match (trace.status, other.status) {
                (SummaryTraceStatus::Supported, SummaryTraceStatus::NeedsReview) => {},
                (SummaryTraceStatus::NeedsReview, SummaryTraceStatus::Supported) => trace = other,
                _ => { trace.status = SummaryTraceStatus::NeedsReview; trace.related_evidence.extend(trace.evidence.drain(..)); },
            }
        }
        if recover_owners.is_none() || !is_field_placeholder(trace.field, &trace.value) { traces.push(trace); }
    }
    Ok(traces)
}

fn task_evidence_key(task: &str, all_tasks: &[String]) -> String {
    let core = |task: &str| ["完成", "执行", "开展"].iter().find_map(|prefix| task.strip_prefix(prefix))
        .filter(|tail| tail.chars().count() >= 4).unwrap_or(task).to_owned();
    let key = core(task);
    if all_tasks.iter().filter(|other| core(other) == key).count() == 1 { key } else { task.to_owned() }
}

fn source_task_key(task: &str, all_tasks: &[String], source: &TranscriptVersionSnapshot) -> String {
    // ponytail: a literal main task plus optional display notes, never fuzzy matching.
    let main = |task: &str| normalize_task(task.split(['（', '(']).next().unwrap_or(task));
    let mains = all_tasks.iter().map(|task| main(task)).collect::<Vec<_>>();
    let key = task_evidence_key(&main(task), &mains);
    if key.chars().count() >= 2
        && source.segments.iter().any(|segment| task_matches(&segment.text, &key))
        && all_tasks.iter().filter(|other| main(other) == main(task)).count() <= 1 {
        key
    } else { task_evidence_key(&normalize_task(task), &all_tasks.iter().map(|task| normalize_task(task)).collect::<Vec<_>>()) }
}

fn normalize_task(task: &str) -> String {
    let task = task.trim().trim_matches('*').trim().nfkc().flat_map(char::to_lowercase).collect::<String>();
    if task.is_ascii() { task.split_whitespace().collect::<Vec<_>>().join(" ") } else { normalize_evidence(&task) }
}

/// A saved review candidate belongs only to its still-masked task/field slot.
/// This never verifies a value or restores it into the authored body.
pub fn review_trace_slot_is_masked(markdown: &str, trace: &SummaryFieldTrace) -> bool {
    if trace.task.trim().is_empty() || trace.markdown_line == 0 { return false; }
    let lines = markdown.lines().collect::<Vec<_>>();
    let mut header = Vec::new();
    let mut fence = None;
    for (index, line) in lines.iter().enumerate() {
        if advance_fence(&mut fence, line) || fence.is_some() { header.clear(); continue; }
        let cells = table_cells(line);
        if table_header(&lines, index) { header = cells; continue; }
        if index + 1 == trace.markdown_line {
            let (anchors, slots) = if let Some(column) = trace.markdown_column {
                let Some(label) = header.get(column) else { return false; };
                let Some(value) = cells.get(column) else { return false; };
                let anchors = header.iter().enumerate().filter(|(_, label)| is_task_label(label))
                    .filter_map(|(i, _)| cells.get(i)).cloned().collect::<Vec<_>>();
                (anchors, cell_field_values(&label_fields(label), value))
            } else {
                (inline_action_anchors(line), inline_labels(line).into_iter()
                    .flat_map(|(label, start, end)| cell_field_values(&label_fields(label), &line[start..end])).collect())
            };
            return normalize_task(&anchors.join(" / ")) == normalize_task(&trace.task)
                && slots.iter().filter(|(field, _)| *field == trace.field).count() == 1
                && slots.iter().any(|(field, value)| *field == trace.field && is_field_placeholder(*field, value));
        }
        if cells.is_empty() { header.clear(); }
    }
    false
}

fn task_matches(text: &str, task: &str) -> bool {
    if !task.is_ascii() { return normalize_evidence(text).contains(task); }
    let pattern = task.split_whitespace().map(regex::escape).collect::<Vec<_>>().join(r"\s+");
    Regex::new(&format!(r"(?i)(?:^|[^a-z0-9_]){pattern}(?:$|[^a-z0-9_])")).unwrap().is_match(text)
}

fn recover_field_value(field: SummaryTraceField, anchors: &[String], all_tasks: &[String], source: &TranscriptVersionSnapshot, owners: &[String]) -> Option<String> {
    if field == SummaryTraceField::Owner {
        let supported = owners.iter().filter(|owner| trace_field_value(field, owner, 0, None, anchors, all_tasks, source).status == SummaryTraceStatus::Supported).collect::<Vec<_>>();
        return (supported.len() == 1).then(|| supported[0].clone());
    }
    if matches!(field, SummaryTraceField::Time | SummaryTraceField::Acceptance | SummaryTraceField::Status | SummaryTraceField::Dependency | SummaryTraceField::Blocker) {
        return stated_attribute_assertion(field, anchors, all_tasks, source).map(|(value, _)| value);
    }
    None
}

fn source_action_owners(task: &str, all_tasks: &[String], source: &TranscriptVersionSnapshot) -> Vec<String> {
    let key = normalize_task(task);
    let keys = all_tasks.iter().map(|task| normalize_task(task)).collect::<Vec<_>>();
    if keys.iter().filter(|other| **other == key).count() > 1 { return Vec::new(); }
    let anchors = vec![key.clone()];
    let mut owners = BTreeSet::new();
    for statement in scoped_source_statements(source, &keys).iter().filter(|statement| task_matches(&statement.text, &key)) {
        owners.extend(assigned_subjects(&statement.text, &key));
        for clause in statement.text.split(['，', ',', '；', ';']) {
            if let Some((action, people)) = clause.split_once("交给").or_else(|| clause.split_once("assigned to ")) {
                if task_matches(action, &key) { owners.extend(owner_members(people.trim_end_matches(['。', '.']))); }
            }
        }
        if contains_first_person_commitment(&statement.text) {
            for reference in &statement.references {
                if let Some(name) = source.segments.iter().find(|segment| segment.segment_id == reference.segment_id).and_then(|segment| segment.bound_display_name.as_ref()) { owners.insert(name.clone()); }
            }
            if let Some((name, _)) = statement.text.split_once('：').or_else(|| statement.text.split_once(':')) {
                if name.chars().count() <= 40 { owners.insert(name.trim().to_owned()); }
            }
        }
    }
    owners.into_iter().filter(|owner| !owner_assignment_evidence(owner, &anchors, &keys, source).is_empty()).collect()
}

/// Fresh report cells use source assertions, not values guessed by the writer.
pub fn source_action_values(task: &str, all_tasks: &[String], source: &TranscriptVersionSnapshot) -> Vec<(SummaryTraceField, String)> {
    let key = normalize_task(task);
    let keys = all_tasks.iter().map(|task| normalize_task(task)).collect::<Vec<_>>();
    if keys.iter().filter(|other| **other == key).count() > 1 { return Vec::new(); }
    let anchors = vec![key];
    let owners = source_action_owners(task, all_tasks, source);
    let mut values = Vec::new();
    if !owners.is_empty() { values.push((SummaryTraceField::Owner, owners.join("、"))); }
    for field in [SummaryTraceField::Time, SummaryTraceField::Acceptance, SummaryTraceField::Status, SummaryTraceField::Dependency, SummaryTraceField::Blocker] {
        if let Some(value) = recover_field_value(field, &anchors, &keys, source, &[]) { values.push((field, value)); }
    }
    values
}

pub fn has_local_action_assignment(task: &str, sentence: &str) -> bool {
    evidence_sentences(sentence).into_iter().any(|sentence| {
        let key = normalize_task(task);
        let Some(start) = normalize_evidence(sentence).find(&key) else { return false; };
        let normalized = normalize_evidence(sentence);
        let prefix = &normalized[..start];
        if hypothetical_assignment(sentence.split(['，', ',']).next().unwrap(), std::slice::from_ref(&key))
            || ["希望", "建议", "可能", "不负责", "不承担", "不执行"].iter().any(|word| prefix.contains(word)) { return false; }
        !assigned_subjects(sentence, &key).is_empty()
            || sentence.split(['，', ',', '；', ';']).any(|clause| task_matches(clause, &key) && contains_first_person_commitment(clause))
    })
}

pub fn source_action_is_assigned(task: &str, all_tasks: &[String], source: &TranscriptVersionSnapshot) -> bool {
    let unique = all_tasks.iter().cloned().collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>();
    if !source_action_owners(task, &unique, source).is_empty() { return true; }
    let key = normalize_task(task);
    let keys = all_tasks.iter().map(|task| normalize_task(task)).collect::<Vec<_>>();
    let mut committed = false;
    for statement in scoped_source_statements(source, &keys).iter().filter(|statement| task_matches(&statement.text, &key)) {
        let text = statement.text.as_str();
        if hypothetical_assignment(text, &keys) || ["希望", "建议", "可能"].iter().any(|word| text.contains(word)) { continue; }
        let qualifiers = normalize_evidence(text).replace(&key, "");
        if ["取消", "撤回", "作废", "不执行", "暂缓"].iter().any(|word| qualifiers.contains(word))
            && !["不取消", "未取消", "不撤回"].iter().any(|word| qualifiers.contains(word)) { committed = false; continue; }
        if text.split(['，', ',', '；', ';']).any(|clause| task_matches(clause, &key) && contains_first_person_commitment(clause)) { committed = true; }
    }
    committed
}

struct ScopedStatement {
    text: String,
    references: Vec<SummaryEvidenceReference>,
    speaker: Option<String>,
}

fn source_reference(segment: &TranscriptEvidenceSegment) -> SummaryEvidenceReference {
    SummaryEvidenceReference { segment_id: segment.segment_id.clone(), start_ms: segment.start_ms, end_ms: segment.end_ms, excerpt_sha256: sha256_text(segment.text.trim()) }
}

fn assignment_task<'a>(clause: &str, tasks: &'a [String]) -> Option<&'a String> {
    let lower = clause.to_lowercase();
    if ["不负责", "不承担", "not responsible", "will not", "does not own"].iter().any(|word| lower.contains(word)) { return None; }
    let role = ["负责", "承担", "will ", "is responsible", "owns "].iter().filter_map(|role| lower.find(role).map(|start| start + role.len())).min()?;
    let matches = tasks.iter().filter(|task| task_matches(&lower[role..], task)).collect::<Vec<_>>();
    (matches.len() == 1).then(|| matches[0])
}

fn hypothetical_assignment(text: &str, tasks: &[String]) -> bool {
    static CONDITIONAL: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:if|provided|example|suppose)\b").unwrap());
    let qualifiers = tasks.iter().fold(normalize_evidence(text), |text, task| text.replace(normalize_evidence(task).as_str(), ""));
    ["如果", "假如", "假设", "例如", "没说"].iter().any(|word| qualifiers.contains(word))
        || CONDITIONAL.is_match(text) || text.trim_end().ends_with(['?', '？'])
}

fn assignment_scopes<'a>(sentence: &'a str, tasks: &[String]) -> Vec<&'a str> {
    let mut start = 0;
    let mut scopes = Vec::new();
    let first = sentence.split(['，', ',']).next().unwrap_or(sentence);
    let mut task = assignment_task(first, tasks);
    // A condition/example may govern the whole paragraph, including later clauses.
    if hypothetical_assignment(first, tasks) { return vec![sentence]; }
    for (offset, separator) in sentence.char_indices().filter(|(_, ch)| matches!(ch, '，' | ',')) {
        let next = offset + separator.len_utf8();
        let clause = sentence[next..].split(['，', ',']).next().unwrap();
        if let Some(assigned) = assignment_task(clause, tasks) {
            if task.is_some_and(|prior| prior != assigned) {
                scopes.push(&sentence[start..offset]);
                start = next;
            }
            task = Some(assigned);
        }
    }
    scopes.push(&sentence[start..]);
    scopes
}

/// A native ASR cut may punctuate the middle of a relative deadline. Join only
/// that grammatical fragment for comparison; evidence stays in both originals.
/// A missing numeral may still link the assignment, but is never filled in.
pub fn join_split_relative_deadline(left: &str, right: &str) -> Option<String> {
    static SPLIT: Lazy<Regex> = Lazy::new(|| Regex::new(r"在(?:[一二三四五六七八九十两\d]+(?:(?:工作日|天|周|星期|个月|年)(?:之)?)?)?$").unwrap());
    static CONTINUATION: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(?:(?:工作日|天|周|星期|个月|年)(?:之)?内|之内|内)").unwrap());
    static COMPLETE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^在[一二三四五六七八九十两\d]*(?:工作日|天|周|星期|个月|年)(?:之)?内").unwrap());
    static COMPLETE_END: Lazy<Regex> = Lazy::new(|| Regex::new(r"在[一二三四五六七八九十两\d]+(?:工作日|天|周|星期|个月|年)(?:之)?内$").unwrap());
    let left = left.trim().trim_end_matches(['。', '.', '！', '!']);
    let right = right.trim();
    // An ASR cut can fall after the time and before the governed action.
    // Callers still reject an audio gap or a changed speaker label.
    if COMPLETE_END.is_match(left) && SOURCE_ACTION.find(right).is_some_and(|action| action.start() == 0) {
        return Some(format!("{left} {right}"));
    }
    let split = SPLIT.find(left)?;
    (CONTINUATION.is_match(right) && COMPLETE.is_match(&format!("{}{right}", &left[split.start()..])))
        .then(|| format!("{left}{right}"))
}

pub fn contiguous_evidence(left: &TranscriptEvidenceSegment, right: &TranscriptEvidenceSegment) -> bool {
    left.effective_speaker() == right.effective_speaker()
        && matches!((left.end_ms, right.start_ms), (Some(end), Some(start)) if end.abs_diff(start) <= 1500)
}

fn scoped_source_statements(source: &TranscriptVersionSnapshot, all_tasks: &[String]) -> Vec<ScopedStatement> {
    let mut statements = Vec::new();
    let mut previous: Option<(String, SummaryEvidenceReference, Option<String>)> = None;
    let mut originals: Vec<ScopedStatement> = Vec::new();
    let mut prior_segment = None;
    for segment in &source.segments {
        let speaker = segment.effective_speaker().or(segment.anonymous_speaker.as_deref()).map(str::to_owned);
        for (index, sentence) in evidence_sentences(&segment.text).into_iter().enumerate() {
            if sentence.trim().is_empty() { continue; }
            if index == 0 && prior_segment.is_some_and(|prior| contiguous_evidence(prior, segment)) {
                if let Some(prior) = originals.last_mut() {
                    if let Some(joined) = join_split_relative_deadline(&prior.text, sentence) {
                        prior.text = joined;
                        prior.references.push(source_reference(segment));
                        continue;
                    }
                }
            }
            originals.push(ScopedStatement { text:sentence.to_owned(), references:vec![source_reference(segment)], speaker:speaker.clone() });
        }
        prior_segment = Some(segment);
    }
    for original in originals {
        let speaker = original.speaker;
        for sentence in assignment_scopes(&original.text, all_tasks).into_iter().filter(|sentence| !sentence.trim().is_empty()) {
            let normalized = normalize_evidence(sentence);
            let mut subjects = all_tasks.iter().filter(|task| task_matches(sentence, task)).collect::<Vec<_>>();
            let reference = original.references.last().unwrap().clone();
            let mut references = original.references.clone();
            let continuation = normalized.strip_prefix("更正").unwrap_or(&normalized);
            let continuation = continuation.trim_start_matches(['，', ',']);
            let starts_with_field = ACTION_FIELDS.iter().flat_map(|field| field_labels(*field)).any(|label| continuation.starts_with(&normalize_evidence(label)));
            // ASR can omit punctuation between a field value and a new assignment.
            // Do not borrow its following fields or invent the missing boundary.
            if starts_with_field && assignment_task(sentence.split(['，', ',']).next().unwrap(), all_tasks).is_some()
                && sentence.contains(['，', ',']) { previous = None; continue; }
            // A task named in a field value is its object, not a new subject.
            if starts_with_field && !subjects.iter().any(|task| normalized.starts_with(normalize_evidence(task).as_str())) { subjects.clear(); }
            let mut text = sentence.to_owned();
            if subjects.is_empty() && starts_with_field {
                if let Some((task, anchor, _)) = previous.as_ref().filter(|(_, _, prior_speaker)| *prior_speaker == speaker) {
                    // Internal scope only. References and displayed excerpts remain original source text.
                    text = format!("{task}，{sentence}");
                    if anchor.segment_id != reference.segment_id { references.insert(0, anchor.clone()); }
                } else { previous = None; }
            } else {
                previous = if subjects.len() == 1 {
                    let task = subjects[0];
                    let start = normalized.find(normalize_evidence(task).as_str()).unwrap();
                    let prefix = &normalized[..start];
                    let object_only = ["依赖", "前提", "卡在", "dependson", "blockedby"].iter().any(|word| prefix.contains(word));
                    let hypothetical = hypothetical_assignment(sentence, all_tasks);
                    (!object_only && !hypothetical).then(|| (task.clone(), reference.clone(), speaker.clone()))
                } else { None };
            }
            statements.push(ScopedStatement { text, references, speaker: speaker.clone() });
        }
    }
    statements
}

/// Restores only evidence-supported action fields after the existing
/// safety sanitizer has replaced high-risk values with placeholders.
/// Unsupported fields remain masked and carry `needs_review` traces.
pub fn restore_supported_owner_and_time_fields(
    evidence_normalized_markdown: &str,
    sanitized_markdown: &str,
    traces: &[SummaryFieldTrace],
) -> String {
    let original = evidence_normalized_markdown.lines().collect::<Vec<_>>();
    let mut sanitized = sanitized_markdown.lines().map(str::to_owned).collect::<Vec<_>>();
    for (index, line) in sanitized.iter_mut().enumerate() {
        let Some(original_line) = original.get(index) else { continue; };
        let checks = traces.iter().filter(|trace| trace.markdown_line == index+1).collect::<Vec<_>>();
        if checks.is_empty() { continue; }
        let original_cells = table_cells(original_line);
        let mut sanitized_cells = table_cells(line);
        if !original_cells.is_empty() && original_cells.len() == sanitized_cells.len() {
            for column in 0..original_cells.len() {
                let cell_checks = checks.iter().filter(|trace| trace.markdown_column == Some(column)).copied().collect::<Vec<_>>();
                if !cell_checks.is_empty() { sanitized_cells[column] = checked_field_cell(&original_cells[column], &sanitized_cells[column], &cell_checks); }
            }
            *line = format!("| {} |", sanitized_cells.join(" | "));
        } else {
            let original_fields = inline_labels(original_line);
            let safe_fields = inline_labels(line);
            let replacements = original_fields.iter().zip(safe_fields.iter()).filter_map(|((label, start, end), (safe_label, safe_start, safe_end))| {
                let fields = label_fields(label);
                if fields.is_empty() || fields != label_fields(safe_label) { return None; }
                let cell_checks = checks.iter().filter(|trace| trace.markdown_column.is_none() && fields.contains(&trace.field)
                    && (fields.len() != 1 || checks.iter().filter(|other| other.field == trace.field).count() == 1
                        || is_field_placeholder(trace.field, &original_line[*start..*end])
                        || normalize_field_value(trace.field, &trace.value) == normalize_field_value(trace.field, &original_line[*start..*end])
                        || (trace.field == SummaryTraceField::Time && deadline_values_match(&original_line[*start..*end], &trace.value))))
                    .copied().collect::<Vec<_>>();
                (!cell_checks.is_empty()).then(|| (*safe_start, *safe_end, checked_field_cell(&original_line[*start..*end], &line[*safe_start..*safe_end], &cell_checks)))
            }).collect::<Vec<_>>();
            for (start, end, value) in replacements.into_iter().rev() { line.replace_range(start..end, &value); }
        }
    }
    let mut result = sanitized.join("\n");
    if sanitized_markdown.ends_with('\n') { result.push('\n'); }
    result
}

fn checked_field_cell(original: &str, masked: &str, checks: &[&SummaryFieldTrace]) -> String {
    let slots = inline_labels(original);
    let complete = !slots.is_empty() && original.split([';', '；']).filter(|part| !part.trim().is_empty()).all(|part| {
        let labels = inline_labels(part); labels.len() == 1 && !label_fields(labels[0].0).is_empty()
    });
    if complete {
        let mut result = original.to_owned();
        for (label, start, end) in slots.into_iter().rev() {
            let fields = label_fields(label);
            let checked = checks.iter().filter(|trace| fields.contains(&trace.field)).collect::<Vec<_>>();
            if checked.len() == 1 {
                let value = if checked[0].status == SummaryTraceStatus::Supported { checked[0].value.as_str() } else { review_label(masked) };
                result.replace_range(start..end, value);
            } else if !checked.is_empty() { result.replace_range(start..end, review_label(masked)); }
        }
        return result;
    }
    let parts = original.split('/').map(str::trim).collect::<Vec<_>>();
    if checks.len() > 1 && parts.len() == checks.len() && !checks.iter().all(|trace| matches!(trace.field, SummaryTraceField::Dependency | SummaryTraceField::Blocker)) {
        return checks.iter().zip(parts).map(|(trace, part)| {
            if trace.status == SummaryTraceStatus::Supported { trace.value.as_str() }
            else if is_field_placeholder(trace.field, part) { part }
            else { review_label(masked) }
        }).collect::<Vec<_>>().join(" / ");
    }
    if checks.len() == 1 && checks[0].status == SummaryTraceStatus::Supported {
        if parts.len() > 1 && !original.contains(&checks[0].value) { return original.to_owned(); }
        return checks[0].value.clone();
    }
    if checks.iter().all(|trace| trace.status == SummaryTraceStatus::Supported) { original.to_owned() }
    else { review_label(masked).to_owned() }
}

fn trace_field_value(
    field: SummaryTraceField,
    value: &str,
    markdown_line: usize,
    markdown_column: Option<usize>,
    action_anchors: &[String],
    all_tasks: &[String],
    source: &TranscriptVersionSnapshot,
) -> SummaryFieldTrace {
    let normalized_anchors = action_anchors
        .iter()
        .map(|anchor| normalize_task(anchor))
        .filter(|anchor| anchor.chars().count() >= 2)
        .collect::<Vec<_>>();
    let mut checked_value = value.trim().to_owned();
    let evidence = if field == SummaryTraceField::Owner {
        let members = owner_members(value);
        let checked = members.iter().map(|member| owner_assignment_evidence(member, &normalized_anchors, all_tasks, source)).collect::<Vec<_>>();
        if checked.is_empty() || checked.iter().any(Vec::is_empty) { Vec::new() }
        else {
            let mut seen = BTreeSet::new();
            checked.into_iter().flatten().filter(|reference| seen.insert(reference.segment_id.clone())).collect()
        }
    } else if matches!(field, SummaryTraceField::Time | SummaryTraceField::Acceptance | SummaryTraceField::Status | SummaryTraceField::Dependency | SummaryTraceField::Blocker) {
        stated_attribute_assertion(field, &normalized_anchors, all_tasks, source).filter(|(assertion, _)| {
            if field == SummaryTraceField::Time { deadline_values_match(value, assertion) }
            else { normalize_field_value(field, value) == normalize_field_value(field, assertion) }
        }).map_or_else(Vec::new, |(assertion, references)| {
            if field == SummaryTraceField::Time { checked_value = assertion; }
            references
        })
    } else { Vec::new() };
    let related_evidence = if evidence.is_empty() {
        let texts: Vec<_> = source.segments.iter().map(|segment| segment.text.as_str()).collect();
        rank_related_segments(action_anchors, &texts).into_iter().map(|index| &source.segments[index]).map(|segment| SummaryEvidenceReference {
            segment_id: segment.segment_id.clone(),
            start_ms: segment.start_ms,
            end_ms: segment.end_ms,
            excerpt_sha256: sha256_text(segment.text.trim()),
        }).collect()
    } else { Vec::new() };
    SummaryFieldTrace {
        field,
        value: checked_value,
        markdown_line,
        markdown_column,
        status: if evidence.is_empty() {
            SummaryTraceStatus::NeedsReview
        } else {
            SummaryTraceStatus::Supported
        },
        evidence,
        task: action_anchors.join(" / "),
        source_task: action_anchors.join(" / "),
        related_evidence,
    }
}

fn owner_members(value: &str) -> Vec<String> {
    static JOIN: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\s*(?:、|，|,|/|&|以及|还有|会同|联合|与|和|及|\band\b)\s*").unwrap());
    JOIN.split(value).map(str::trim).filter(|member| !member.is_empty()).map(str::to_owned).collect()
}

fn unresolved_owner(value: &str) -> bool {
    let value = normalize_evidence(value);
    matches!(value.as_str(), "我" | "i" | "we" | "they" | "someone")
        || ["我们", "咱们", "他们", "她们", "大家", "有人", "这一步", "所以"].iter().any(|word| value.contains(word))
}

fn assigned_subjects(sentence: &str, task: &str) -> Vec<String> {
    static ENGLISH: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(?:^|[,;。；])\s*(?P<people>[^,;。；]+?)\s+(?:(?:are|is)(?: not)? responsible for|will(?: not)?|owns)\s+").unwrap());
    for capture in ENGLISH.captures_iter(sentence) {
        if task_matches(&sentence[capture.get(0).unwrap().end()..], task) { return owner_members(&capture["people"]).into_iter().filter(|name| !unresolved_owner(name)).collect(); }
    }
    let text = sentence.nfkc().flat_map(char::to_lowercase).filter(|ch| !ch.is_whitespace()).collect::<String>();
    let Some(action) = text.find(task) else { return Vec::new(); };
    let prefix = &text[..action];
    let role = ["负责", "承担"].iter().filter_map(|role| prefix.rfind(role).map(|start| (start, *role, false)));
    let request = ["要求", "请"].iter().filter_map(|role| prefix.rfind(role).map(|start| (start, *role, true)));
    let explicit = role.chain(request).max_by_key(|(start, _, _)| *start);
    let imperative = prefix.rfind('要').filter(|start| !prefix[..*start].ends_with(['需', '想'])
        && explicit.map_or(true, |(prior, _, _)| *start > prior && prefix[prior..*start].contains([',', '，']))).map(|start| (start, "要", false));
    let Some((start, marker, requested)) = imperative.or(explicit) else { return Vec::new(); };
    let names = if requested {
        let tail = &prefix[start + marker.len()..];
        let end = ["要", "在", "依照", "按照", "根据", "向", "等", "一起来"].iter().filter_map(|word| tail.find(word)).min().unwrap_or(tail.len());
        &tail[..end]
    } else {
        let head = &prefix[..start];
        let begin = head.rfind(['，', ',', '；', ';', '。']).map_or(0, |offset| offset + head[offset..].chars().next().unwrap().len_utf8());
        let head = &head[begin..];
        let head = ["请", "要求", "交给"].iter().filter_map(|marker| head.rfind(marker).map(|start| (start, &head[start + marker.len()..]))).max_by_key(|(start, _)| *start).map_or(head, |(_, tail)| tail);
        head.trim_start_matches('由').trim_end_matches("不再").trim_end_matches('不').trim_end_matches("共同").trim_end_matches("一起")
    };
    static DEPARTMENTS: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^部会局署、，,]+?(?:政府|部|会|局|署)").unwrap());
    owner_members(names).into_iter().flat_map(|name| {
        let departments = DEPARTMENTS.find_iter(&name).map(|found| found.as_str().to_owned()).collect::<Vec<_>>();
        if departments.len() > 1 && departments.concat() == name { departments } else { vec![name] }
    }).filter(|name| !unresolved_owner(name)).collect()
}

fn owner_assignment_evidence(owner: &str, anchors: &[String], all_tasks: &[String], source: &TranscriptVersionSnapshot) -> Vec<SummaryEvidenceReference> {
    static UNCERTAIN: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:might|may|maybe|possibly|hope|suggest)\b").unwrap());
    let name = normalize_evidence(owner).trim_start_matches('各').to_owned();
    if unresolved_owner(&name) { return Vec::new(); }
    let mut final_assertion = None;
    for statement in scoped_source_statements(source, all_tasks) {
        let text = statement.text.as_str();
        let qualifiers = anchors.iter().fold(normalize_evidence(text), |text, task| text.replace(normalize_evidence(task).as_str(), ""));
        if hypothetical_assignment(text.split(['，', ',']).next().unwrap(), all_tasks)
            || text.trim_end().ends_with(['?', '？'])
            || UNCERTAIN.is_match(text)
            || ["希望", "建议", "可能", "预计"].iter().any(|word| qualifiers.contains(word)) { continue; }
        for task in anchors.iter().filter(|task| task_matches(text, task)) {
            if ["取消", "撤回", "作废", "未指定负责人", "尚未确定负责人"].iter().any(|word| qualifiers.contains(word))
                && !["不取消", "未取消", "不撤回"].iter().any(|word| qualifiers.contains(word)) {
                final_assertion = Some((false, statement.references.clone())); continue;
            }
            let first_person_action = text.split(['，', ',', '；', ';']).any(|clause| task_matches(clause, task)
                && contains_first_person_commitment(clause) && !hypothetical_assignment(clause, all_tasks));
            let transfer = text.split(['，', ',', '；', ';']).any(|clause| {
                ["交给", "assigned to "].iter().any(|marker| clause.to_lowercase().split_once(marker).is_some_and(|(action, person)|
                    task_matches(action, task) && owner_members(person.trim_end_matches(['。', '.'])).iter().any(|person| normalize_evidence(person) == name)))
            });
            let subjects = assigned_subjects(text, task);
            let assigned = subjects.iter().any(|person| normalize_evidence(person).trim_start_matches('各') == name)
                || (statement.speaker.as_deref().is_some_and(|speaker| normalize_evidence(speaker) == name)
                    && first_person_action)
                || (text.trim_start().starts_with(&format!("{}：", owner.trim())) || text.trim_start().starts_with(&format!("{}:", owner.trim())))
                    && first_person_action || transfer;
            if !assigned {
                if !subjects.is_empty() && (qualifiers.contains("更正") || qualifiers.contains("改为")) {
                    final_assertion = Some((false, statement.references.clone()));
                }
                continue;
            }
            let negative = ["不负责", "不再负责", "不承担", "不是负责人", "not responsible", "will not", "will never", "does not own"].iter().any(|word| text.to_lowercase().contains(word));
            final_assertion = Some((!negative, statement.references.clone()));
        }
    }
    final_assertion.filter(|(supported, _)| *supported).map_or_else(Vec::new, |(_, references)| references)
}

fn review_label(placeholder: &str) -> &'static str {
    if placeholder.is_ascii() { "Needs review" } else { "待核对" }
}

fn oral_relative_deadline(sentence: &str, anchors: &[String]) -> Option<String> {
    static WITHIN: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?P<value>[一二三四五六七八九十两\d]+(?:工作日|天|周|星期|个月|年)(?:之)?内)").unwrap());
    let periods = WITHIN.captures_iter(sentence).collect::<Vec<_>>();
    if periods.len() != 1 { return None; }
    let found = periods[0].get(0)?;
    let prefix = normalize_evidence(&sentence[..found.start()]);
    if prefix.chars().last().is_some_and(|ch| ch == '.' || ch.is_ascii_digit())
        || ["希望", "建议", "争取", "可能", "预计", "大约", "去年", "上次", "曾经", "如果", "假如", "假设", "不在", "不是", "撤回"]
            .iter().any(|word| prefix.contains(word))
        || prefix.ends_with(['到', '至', '或', '约']) { return None; }
    // The time must govern this immediate action, not a later approval or discussion.
    let action = normalize_evidence(sentence[found.end()..].split(['，', ',', '；', ';', '。']).next()?);
    let anchored = anchors.iter().any(|task| action.starts_with(task)
        || ["完成", "执行", "开展"].iter().any(|verb| action.strip_prefix(verb).is_some_and(|tail| tail.starts_with(task))));
    anchored.then(|| periods[0]["value"].to_owned())
}

/// Literal attribute assertions only; retrieval and nearby task mentions are not proof.
fn stated_attribute_assertion(
    field: SummaryTraceField,
    anchors: &[String],
    all_tasks: &[String],
    source: &TranscriptVersionSnapshot,
) -> Option<(String, Vec<SummaryEvidenceReference>)> {
    static ATTRIBUTE: Lazy<Regex> = Lazy::new(|| Regex::new(
        r"(?i)验收标准|验收条件|放行标准|当前状态|任务状态|状态|截止时间|截止日期|依赖|前提|需要先|(?:要|需要)?等|卡点|卡在|阻碍|acceptance criteria|acceptance criterion|current status|status|deadline|due date|dependencies|dependency|depends on|requires|prerequisite|blocker|blocked by"
    ).unwrap());
    static ACCEPTANCE: Lazy<Regex> = Lazy::new(|| Regex::new(
        r"(?i)(?:验收标准|验收条件|放行标准|acceptance criteri(?:a|on))\s*(?:(?:改为|是|为|[:：]|is\b|are\b)\s*)?(?P<value>[^，,。；;\n]+)"
    ).unwrap());
    static STATUS: Lazy<Regex> = Lazy::new(|| Regex::new(
        r"(?i)(?:当前状态|任务状态|状态|current status|status)\s*(?:(?:改为|是|为|[:：]|is\b)\s*)?(?P<value>[^，,。；;\n]+)"
    ).unwrap());
    static CONDITIONAL: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:if|provided|example|suppose)\b").unwrap());
    static DEPENDENCY: Lazy<Regex> = Lazy::new(|| Regex::new(
        r"(?i)(?:依赖|前提(?:条件)?|需要先|(?:要|需要)?等|depends on|dependencies|dependency|requires|prerequisites?)\s*(?:(?:改为|是|为|[:：]|is\b|are\b)\s*)?(?P<value>[^，,。；;\n]+)"
    ).unwrap());
    static BLOCKER: Lazy<Regex> = Lazy::new(|| Regex::new(
        r"(?i)(?:卡点|卡在|阻碍|blockers?|blocked by)\s*(?:(?:改为|是|为|[:：]|is\b|are\b)\s*)?(?P<value>[^，,。；;\n]+)"
    ).unwrap());
    static NEGATED: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(?:not|no|never)\b").unwrap());
    static DEADLINE: Lazy<Regex> = Lazy::new(|| Regex::new(
        r"(?i)(?:截止时间|截止日期|完成时间|截止|deadline|due date)\s*(?:(?:改为|是|为|[:：]|is\b)\s*)?(?P<value>[^，,。；;\n]+)"
    ).unwrap());
    static DUE_BEFORE: Lazy<Regex> = Lazy::new(|| Regex::new(
        r"(?P<value>(?:今天|明天|后天|(?:\d{4}年)?\d{1,2}月\d{1,2}日|(?:下|本)?周[一二三四五六日天]?|星期[一二三四五六日天])(?:\d{1,2}[点时](?:\d{1,2}分?)?|\d{1,2}:\d{2})?前)(?:提交|完成|交付|发布|发送|发给)"
    ).unwrap());
    let relation = matches!(field, SummaryTraceField::Dependency | SummaryTraceField::Blocker);
    let pattern = match field { SummaryTraceField::Acceptance => &*ACCEPTANCE, SummaryTraceField::Status => &*STATUS,
        SummaryTraceField::Dependency => &*DEPENDENCY, SummaryTraceField::Time => &*DEADLINE, _ => &*BLOCKER };
    let mut assertions = Vec::new();
    for statement in scoped_source_statements(source, all_tasks) {
        let sentence = statement.text.as_str();
            if sentence.trim_end().ends_with(['?', '？']) { continue; }
            let absence = relation && anchors.iter().any(|task| {
                let normalized = normalize_evidence(sentence);
                let Some((_, tail)) = normalized.rsplit_once(normalize_evidence(task).as_str()) else { return false; };
                let tail = tail.trim_end_matches(['。', '！', '.', '!']);
                let tail = tail.strip_prefix('的').unwrap_or(tail);
                let tail = tail.strip_prefix("目前").or_else(|| tail.strip_prefix("当前")).unwrap_or(tail);
                match field {
                    SummaryTraceField::Dependency => ["无依赖", "没有依赖", "hasnodependencies", "hasnodependency"].contains(&tail),
                    _ => ["无卡点", "没有卡点", "没有阻碍", "hasnoblockers", "hasnoblocker"].contains(&tail),
                }
            });
            let first_attribute = ATTRIBUTE.find_iter(sentence).find(|found| {
                all_tasks.iter().any(|task| task_matches(&sentence[..found.start()], task))
            }).map_or(sentence.len(), |found| found.start());
            let subject = normalize_evidence(&sentence[..first_attribute]);
            let subjects = all_tasks.iter().filter(|task| task_matches(&sentence[..first_attribute], task)).collect::<Vec<_>>();
            let subjects = subjects.iter().filter(|task| !subjects.iter().any(|other| other != *task && other.contains(task.as_str()))).collect::<Vec<_>>();
            let qualifiers = all_tasks.iter().fold(subject.clone(), |text, task| text.replace(normalize_evidence(task).as_str(), ""));
            if !subjects.iter().any(|task| anchors.contains(*task))
                || (field == SummaryTraceField::Time && ["希望", "建议", "争取"].iter().any(|word| qualifiers.contains(word)))
                || ["如果", "假如", "若", "假设", "例如", "没说", "不是说", "不要说"].iter().any(|word| qualifiers.contains(word))
                || (field == SummaryTraceField::Status && qualifiers.contains("计划"))
                || (relation && !absence && (["不", "没"].iter().any(|word| qualifiers.contains(word)) || NEGATED.is_match(&sentence[..first_attribute])))
                || CONDITIONAL.is_match(&sentence[..first_attribute]) {
                continue;
            }
            let mut values = pattern.captures_iter(sentence).filter(|capture| capture.get(0).unwrap().start() >= first_attribute).filter_map(|capture| {
                let matched = capture.get(0).unwrap();
                let correction = matched.as_str().contains("改为") || ["更正", "不对", "correction"].iter()
                    .any(|word| sentence[..matched.start()].to_lowercase().contains(word));
                let captured = capture.name("value").unwrap();
                let mut value_end = captured.end();
                if field == SummaryTraceField::Acceptance {
                    // ASR commas can divide one criterion. Keep its literal continuation,
                    // stopping at another field or task instead of weakening the condition.
                    while let Some(separator @ ('，' | ',')) = sentence[value_end..].chars().next() {
                        let tail = &sentence[value_end + separator.len_utf8()..];
                        let next = tail.split(['，', ',', '。', '；', ';', '\n']).next().unwrap();
                        if next.trim().is_empty()
                            || ATTRIBUTE.find(next.trim()).is_some_and(|found| found.start() == 0)
                            || all_tasks.iter().any(|task| normalize_evidence(next).starts_with(&normalize_evidence(task)))
                            || assignment_task(next, all_tasks).is_some() { break; }
                        value_end += separator.len_utf8() + next.len();
                    }
                }
                let mut value = sentence[captured.start()..value_end].trim().trim_end_matches(['.', '!']);
                if field == SummaryTraceField::Dependency && ["等", "要等", "需要等"].iter().any(|prefix| matched.as_str().starts_with(prefix)) {
                    for suffix in ["之后才能开始", "后才能开始", "之后", "后"] { if let Some(prefix) = value.strip_suffix(suffix) { value = prefix; break; } }
                }
                (!value.is_empty()).then(|| (matched.start(), value.to_owned(), correction))
            }).collect::<Vec<_>>();
            let fallback = (field == SummaryTraceField::Acceptance).then(|| {
                    ["尚未确定验收条件", "未约定验收条件", "尚未确定验收标准", "未约定验收标准"].iter()
                        .find(|word| sentence.contains(**word)).map(|word| (*word).to_owned())
                }).flatten()
                .or_else(|| {
                    if field != SummaryTraceField::Status { return None; }
                    let normalized = normalize_evidence(sentence);
                    let (_, tail) = normalized.rsplit_once(normalize_evidence(anchors.first()?).as_str())?;
                    let tail = tail.trim_end_matches(['。', '！', '？']);
                    let tail = tail.strip_prefix("目前").or_else(|| tail.strip_prefix("现在")).unwrap_or(tail);
                    ["进行中", "正在进行", "未开始", "已完成", "已经完成", "尚未完成", "未完成", "已暂停", "已取消", "inprogress", "notstarted", "completed", "unfinished"].contains(&tail).then(|| tail.to_owned())
                }).or_else(|| absence.then(|| if sentence.is_ascii() { "None" } else { "无" }.to_owned()))
                .or_else(|| (field == SummaryTraceField::Time).then(|| {
                    if has_multiple_deadlines(&normalize_evidence(sentence)) { return None; }
                    oral_relative_deadline(sentence, anchors).or_else(|| DUE_BEFORE.captures(sentence).map(|capture| capture["value"].to_owned())).or_else(|| {
                        ["下周", "本周", "明天", "今天"].iter().find(|period| sentence.trim_start().starts_with(**period))
                            .filter(|period| {
                                let tail = normalize_evidence(sentence.trim_start().strip_prefix(**period).unwrap()).trim_end_matches(['。', '.']).to_owned();
                                anchors.iter().any(|task| tail == normalize_evidence(task) || ["完成", "执行", "开展"].iter().any(|verb| tail == format!("{verb}{}", normalize_evidence(task))))
                            })
                            .map(|_| sentence.trim().trim_end_matches(['。', '.']).to_owned())
                    })
                }).flatten());
            if values.is_empty() {
                if let Some(value) = fallback { values.push((first_attribute, value, sentence.contains("更正"))); }
            }
            for (start, assertion, correction) in values {
                // ponytail: explicit labels and literal values; arbitrary paraphrases stay for review.
                let end = ATTRIBUTE.find_iter(sentence).find(|found| found.start() > start).map_or(sentence.len(), |found| found.start());
                let clause_start = sentence[..start].rfind(['，', ',', '；', ';']).map_or(0, |index| index + sentence[index..].chars().next().unwrap().len_utf8());
                let clause_subject = &sentence[clause_start..start];
                if all_tasks.iter().any(|task| !anchors.iter().any(|anchor| anchor.contains(task)) && task_matches(clause_subject, task)) { break; }
                let confidence = format!("{}{}", qualifiers, normalize_evidence(&sentence[clause_start..end]));
                let unknown_time = field == SummaryTraceField::Time && matches!(normalize_evidence(&assertion).as_str(), "尚未确定" | "未决定" | "未定" | "tobeconfirmed" | "notyetdecided" | "undetermined" | "notdetermined");
                let certain = subjects.len() == 1
                    && !["可能", "预计", "尚未确认", "未经确认", "未核实", "尚未核实", "unconfirmed", "notconfirmed"].iter().any(|word| confidence.contains(word))
                    && !["计划", "希望", "如果", "假设", "planned", "expected", "might", "would"].iter().any(|word| assertion.to_lowercase().contains(word))
                    && !(relation && !absence && (["不依赖", "没有卡", "并未卡", "没有阻"].iter().any(|word| confidence.contains(word)) || NEGATED.is_match(&sentence[clause_start..end]) || CONDITIONAL.is_match(&sentence[clause_start..end])))
                    && !(field == SummaryTraceField::Time && (has_multiple_deadlines(&normalize_evidence(sentence)) || CONDITIONAL.is_match(&sentence[clause_start..end]) || (!unknown_time && ["不是", "不在", "not", "never"].iter().any(|word| confidence.contains(word)))));
                assertions.push((statement.references.clone(), assertion, correction, certain));
            }
    }
    let (_, final_value, _, certain) = assertions.last()?;
    let decisive = assertions.iter().rposition(|(_, _, corrected, _)| *corrected).unwrap_or(0);
    if !certain || assertions[decisive..].iter().any(|(_, assertion, _, certain)| !certain || normalize_field_value(field, assertion) != normalize_field_value(field, final_value)) {
        return None;
    }
    let value = final_value.clone();
    let mut seen = BTreeSet::new();
    let evidence = assertions.into_iter().flat_map(|(references, _, _, _)| references).filter(|reference| seen.insert(reference.segment_id.clone())).collect();
    Some((value, evidence))
}

fn deadline_values_match(candidate: &str, source: &str) -> bool {
    if normalize_evidence(candidate).replace("之内", "内").trim_end_matches('前') == normalize_evidence(source).replace("之内", "内").trim_end_matches('前') { return true; }
    static CHINESE_DATE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(?:(\d{4})年)?(\d{1,2})月(\d{1,2})日(\d{1,2})[点时](?:(\d{1,2})分?)?前?$").unwrap());
    // Match minute precision only; render the literal source, never an inferred UTC time or year.
    static ISO_DATE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(\d{4})-(\d{2})-(\d{2})[ T](\d{2}):(\d{2})(?::00(?:\.0+)?)?Z?$").unwrap());
    let (Some(original), Some(iso)) = (CHINESE_DATE.captures(source.trim()), ISO_DATE.captures(candidate.trim())) else { return false; };
    let number = |capture: &regex::Captures<'_>, index| capture.get(index).and_then(|value| value.as_str().parse::<u32>().ok());
    let (month, day, hour, minute) = (number(&original, 2), number(&original, 3), number(&original, 4), number(&original, 5).unwrap_or(0));
    original.get(1).map_or(true, |year| year.as_str() == &iso[1])
        && month == number(&iso, 2) && day == number(&iso, 3) && hour == number(&iso, 4) && minute == number(&iso, 5).unwrap_or(0)
        && hour.is_some_and(|hour| hour < 24) && minute < 60
        && chrono::NaiveDate::from_ymd_opt(number(&original, 1).unwrap_or(2000) as i32, month.unwrap_or(0), day.unwrap_or(0)).is_some()
}

fn normalize_field_value(field: SummaryTraceField, value: &str) -> String {
    if matches!(field, SummaryTraceField::Acceptance | SummaryTraceField::Status | SummaryTraceField::Dependency | SummaryTraceField::Blocker) {
        static DEPENDENCY_PREFIX: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(?:依赖于?|需要先|depends on|requires)\s*[:：]?\s*").unwrap());
        static BLOCKER_PREFIX: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(?:(?:目前|当前)?卡在|(?:is )?(?:currently )?blocked by)\s*[:：]?\s*").unwrap());
        let value = value.trim().trim_matches('*').trim().nfkc().collect::<String>();
        let value = match field {
            SummaryTraceField::Dependency => DEPENDENCY_PREFIX.replace(&value, ""),
            SummaryTraceField::Blocker => BLOCKER_PREFIX.replace(&value, ""),
            _ => std::borrow::Cow::Borrowed(value.as_str()),
        };
        value.trim_end_matches(['。', '.', '！', '!']).chars().flat_map(char::to_lowercase)
            .filter(|character| !character.is_whitespace() && *character != '*').collect()
    } else { normalize_evidence(value) }
}

struct TableTraceSchema {
    fields: Vec<(usize, Vec<SummaryTraceField>)>,
    action_anchor_columns: Vec<usize>,
}

fn inline_action_anchors(line: &str) -> Vec<String> {
    inline_field_values(line, TASK_LABELS)
        .into_iter()
        .map(str::to_owned)
        .collect()
}

fn contains_first_person_commitment(text: &str) -> bool {
    let normalized = text.to_ascii_lowercase();
    if ["i will not", "i will never", "i'll not"].iter().any(|phrase| normalized.contains(phrase)) { return false; }
    [
        "我负责",
        "我来",
        "我会",
        "由我",
        "i'll",
        "i will",
        "i can take",
        "i am responsible",
        "i'm responsible",
    ]
    .iter()
    .any(|phrase| normalized.contains(phrase))
}

fn inline_field_values<'a>(line: &'a str, labels: &[&str]) -> Vec<&'a str> {
    inline_field_ranges(line, labels)
        .into_iter()
        .map(|(start, end)| line[start..end].trim().trim_matches('*').trim())
        .collect()
}

fn is_review_placeholder(value: &str) -> bool {
    matches!(
        normalize_evidence(value).trim_matches(['。', '，', '；', '：']),
        "会议未提及"
            | "待核对"
            | "未提及"
            | "待确认"
            | "待检查"
            | "需检查"
            | "无"
            | "无明确提及"
            | "未明确提及"
            | "原文未提及"
            | "未指定"
            | "未决定"
            | "未定"
            | "尚未确定"
            | "none"
            | "notmentioned"
            | "tobeconfirmed"
            | "needsreview"
    )
}

fn is_field_placeholder(field: SummaryTraceField, value: &str) -> bool {
    if field == SummaryTraceField::Time && matches!(normalize_evidence(value).as_str(), "尚未确定" | "未定" | "未决定" | "tobeconfirmed") { return false; }
    if matches!(field, SummaryTraceField::Dependency | SummaryTraceField::Blocker)
        && matches!(normalize_evidence(value).as_str(), "无" | "none" | "尚未确定" | "未定" | "未决定") { return false; }
    if matches!(field, SummaryTraceField::Acceptance | SummaryTraceField::Status)
        && matches!(normalize_evidence(value).as_str(), "尚未确定" | "未定" | "未决定") { return false; }
    is_review_placeholder(value)
}

fn normalize_evidence(value: &str) -> String {
    static CLOCK: Lazy<Regex> = Lazy::new(|| Regex::new(
        r"(?P<h>\d{1,2})(?:(?:[:：](?P<m>\d{2}))|(?:[点时](?:(?P<cm>\d{1,2})分?)?))"
    ).unwrap());
    let width_normalized: String = value.nfkc().filter(|character| !character.is_whitespace()).collect();
    let normalized_clock = CLOCK.replace_all(&width_normalized, |caps: &regex::Captures<'_>| {
        let hour = caps["h"].parse::<u32>().unwrap_or(99);
        let minute = caps.name("m").or_else(|| caps.name("cm"))
            .and_then(|m| m.as_str().parse::<u32>().ok()).unwrap_or(0);
        if hour > 23 || minute > 59 { caps[0].to_owned() }
        else { format!("{hour}时{minute:02}分") }
    });
    normalized_clock
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| !character.is_whitespace() && !character.is_ascii_punctuation())
        .collect()
}

#[cfg(test)]
mod source_review_regressions {
    use super::*;

    #[test]
    fn t08_clock_spacing_does_not_change_deadline_evidence() {
        assert_eq!(normalize_evidence("10 月 9 日 18 点"), normalize_evidence("10月9日18点"));
        assert_eq!(normalize_evidence("10 月 9 日 18 点 30 分"), normalize_evidence("10月9日18点30分"));
        assert_ne!(normalize_evidence("10 月 9 日 18 点 30 分"), normalize_evidence("10月9日18点"));
    }

    #[test]
    fn absent_dependencies_are_not_claims_needing_review() {
        assert!(is_review_placeholder("无明确提及。"));
        assert!(is_review_placeholder("未指定"));
        assert!(!is_review_placeholder("周宁未指定测试负责人"));
    }

    #[test]
    fn paraphrased_tasks_find_related_text_without_promoting_it_to_proof() {
        let segments = ["线上开会讨论项目进度。", "我今天18点前把纪要发给四位参会人，并抄送赵琪。", "我负责完成全部100个用例的回归测试，交付测试报告。"];
        let ranked = rank_related_segments(&["发送会议纪要".to_string()], &segments);
        assert_eq!(ranked.first(), Some(&1));
        assert_eq!(rank_related_segments(&["完成全部100个用例的回归测试并交付测试报告".into()], &segments).first(), Some(&2));
        assert!(rank_related_segments(&["购买办公室打印机".into()], &segments).is_empty());
    }
}

/// Retrieval hints only. Similar wording must never change a field to Supported.
fn rank_related_segments(anchors: &[String], texts: &[&str]) -> Vec<usize> {
    fn terms(text: &str) -> BTreeSet<String> {
        let chars: Vec<_> = normalize_evidence(text).chars().filter(|c| c.is_alphabetic()).collect();
        chars.windows(2).map(|pair| pair.iter().collect()).collect()
    }
    let query = terms(&anchors.join(" "));
    if query.is_empty() { return Vec::new(); }
    let documents: Vec<_> = texts.iter().map(|text| terms(text)).collect();
    let weights: Vec<_> = query.iter().map(|term| {
        let frequency = documents.iter().filter(|document| document.contains(term)).count();
        (term, ((documents.len() + 1) as f64 / (frequency + 1) as f64).ln() + 1.0)
    }).collect();
    let mut ranked: Vec<_> = documents.iter().enumerate().filter_map(|(index, document)| {
        let score: f64 = weights.iter().filter(|(term, _)| document.contains(*term)).map(|(_, weight)| weight).sum();
        (score > 0.0).then_some((index, score))
    }).collect();
    ranked.sort_by(|left, right| right.1.total_cmp(&left.1).then(left.0.cmp(&right.0)));
    let Some(best) = ranked.first().map(|item| item.1) else { return Vec::new(); };
    ranked.into_iter().filter(|(_, score)| *score >= best * 0.6).take(2).map(|(index, _)| index).collect()
}

fn evidence_sentences(text: &str) -> Vec<&str> {
    // Some native Chinese ASR separates clauses with spaces. Only explicit
    // subject/condition/principle or approval-then-implementation boundaries
    // may narrow a field's evidence;
    // a leading hypothetical must continue to govern its later assignments.
    static SPOKEN_BOUNDARY: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(?:如果|如需|那么届时|以上(?:[，,。]|\s|谢谢|$)|接下来请[\p{Han}]{1,24}(?:说明|报告)|第[一二三四五六七八九十]\s*(?:要|现有|对于)|送[\p{Han}]{1,12}(?:之后|后)(?:呢)?\s*(?:据以|至于|再)?实施|(?:院长|主持人)(?:要求|也表示|表示|也请)|[\p{Han}]{1,8}(?:政府|部|会|局|署)(?:要|负责|承担))").unwrap());
    static NEW_TOPIC_ASSIGNMENT: Lazy<Regex> = Lazy::new(|| Regex::new(r"^另外(?:因为[^。！？;；]{0,40})?(?:院长|主持人)要求").unwrap());
    // ponytail: only an explicit conditional conclusion closes a hypothesis
    // before a separate reported assignment; ambiguous clauses stay unassigned.
    static CONDITIONAL_CONCLUSION: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?:的话[，,\s]*那(?:么)?|则|那么)[^。！？;；]{2,80}$").unwrap());
    let mut sentences = Vec::new();
    let mut start = 0;
    for (offset, ch) in text.char_indices() {
        let end = offset + ch.len_utf8();
        let separator = matches!(ch, '。' | '！' | '？' | '\n' | '；' | ';')
            || (matches!(ch, '.' | '!' | '?') &&
                text[end..].chars().next().map_or(true, char::is_whitespace));
        let spoken_boundary = ch.is_whitespace()
            && !["还有", "以及", "及", "与", "和"].iter().any(|prefix| text[end..].starts_with(prefix))
            && {
                let hypothetical = ["如果", "假如", "假设", "例如", "若"].iter().any(|word| text[start..offset].contains(word));
                (NEW_TOPIC_ASSIGNMENT.is_match(&text[end..]) && (!hypothetical
                    || CONDITIONAL_CONCLUSION.find(&text[start..offset]).is_some_and(|conclusion|
                        !["如果", "假如", "假设", "例如", "若"].iter().any(|word| conclusion.as_str().contains(word)))))
                    || (!hypothetical && SPOKEN_BOUNDARY.is_match(&text[end..]))
            };
        if separator || spoken_boundary {
            sentences.push(&text[start..end]);
            start = end;
        }
    }
    if start < text.len() { sentences.push(&text[start..]); }
    sentences
}

fn has_multiple_deadlines(text: &str) -> bool {
    static CLOCKS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\d{1,2}时\d{2}分").unwrap());
    static DATES: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?:\d{1,2}月)?\d{1,2}[日号]|(?:周|星期)[一二三四五六日天]").unwrap());
    CLOCKS.find_iter(text).take(2).count() > 1 || DATES.find_iter(text).take(2).count() > 1
}

fn format_timestamp(milliseconds: u64) -> String {
    let total_seconds = milliseconds / 1000;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("[{hours:02}:{minutes:02}:{seconds:02}]")
    } else {
        format!("[{minutes:02}:{seconds:02}]")
    }
}

fn clean_optional(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn paired_optional_text(left: Option<&str>, right: Option<&str>) -> bool {
    match (clean_optional(left), clean_optional(right)) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            valid_identifier(left, 200)
                && right.chars().count() <= 200
                && !right.chars().any(is_forbidden_control)
        }
        _ => false,
    }
}

fn valid_identifier(value: &str, max: usize) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= max
        && !value.chars().any(is_forbidden_control)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn valid_anonymous_speaker(value: &str) -> bool {
    let value = value.trim();
    value.len() >= 3
        && value.starts_with('S')
        && value[1..].bytes().all(|byte| byte.is_ascii_digit())
}

fn is_forbidden_control(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{202A}'
                | '\u{202B}'
                | '\u{202C}'
                | '\u{202D}'
                | '\u{202E}'
                | '\u{2066}'
                | '\u{2067}'
                | '\u{2068}'
                | '\u{2069}'
        )
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256_text(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn canonical_json_sha256<T: Serialize>(value: &T) -> String {
    fn canonicalize_value(value: Value) -> Value {
        match value {
            Value::Array(values) => {
                Value::Array(values.into_iter().map(canonicalize_value).collect())
            }
            Value::Object(values) => {
                let mut entries = values.into_iter().collect::<Vec<_>>();
                entries.sort_by(|left, right| left.0.cmp(&right.0));
                let mut canonical = Map::new();
                for (key, value) in entries {
                    canonical.insert(key, canonicalize_value(value));
                }
                Value::Object(canonical)
            }
            scalar => scalar,
        }
    }

    let json = serde_json::to_value(value).expect("serializable summary source value");
    let canonical = canonicalize_value(json);
    let bytes = serde_json::to_vec(&canonical).expect("canonical summary source is serializable");
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unresolved_subject_is_not_a_named_owner() {
        for (text, owner, task) in [
            ("所以所以这一步呢我们还要再跟地方政府来来做协调。", "所以所以这一步呢我们还", "协调"),
            ("我们负责整理报告。", "我们", "整理报告"),
            ("We will prepare the report.", "We", "prepare the report"),
        ] {
            let source = t06_source(&[text]);
            assert!(source_action_values(task, &[task.into()], &source).iter().all(|(field,_)| *field != SummaryTraceField::Owner));
            assert_eq!(trace_owner_and_time_fields(&format!("任务：{task}；负责人：{owner}"), &source).unwrap()[0].status, SummaryTraceStatus::NeedsReview);
        }
        let source = t06_source(&["张三负责整理报告。"]);
        assert!(source_action_values("整理报告", &["整理报告".into()], &source).iter().any(|(field,value)| *field == SummaryTraceField::Owner && value == "张三"));
    }

    #[test]
    fn configured_spelling_requires_raw_assignment_and_keeps_raw_references() {
        let normalize = |text: &str| text.replace("卫服部", "卫福部").replace("林州", "林舟");
        let source = t06_source(&["请卫服部在一个月内修订医疗计划。林州负责整理报告。"]);
        let before = source.clone();
        let traces = trace_action_fields_with_spelling("任务：修订医疗计划；负责人：卫福部；截止时间：一个月内\n任务：整理报告；负责人：林舟", &source, normalize).unwrap();
        assert!(traces.iter().all(|trace| trace.status == SummaryTraceStatus::Supported));
        assert!(traces.iter().all(|trace| trace.evidence[0].excerpt_sha256 == sha256_text(source.segments[0].text.trim())));
        assert_eq!(source, before);
        let absent = t06_source(&["卫服部参加会议。林州收到整理报告。"]);
        assert!(trace_action_fields_with_spelling("任务：整理报告；负责人：林舟", &absent, normalize).unwrap().iter().all(|trace| trace.status == SummaryTraceStatus::NeedsReview));
        let mut corrupted = source; corrupted.segments[0].text.push_str("更正");
        assert_eq!(trace_action_fields_with_spelling("任务：整理报告；负责人：林舟", &corrupted, normalize).unwrap_err(), SummarySourceBindingError::TranscriptHashMismatch);
    }

    #[test]
    fn explicit_requested_joint_departments_have_individual_task_proof() {
        assert_eq!(assigned_subjects("请农业部参加会议，教育部负责整理报告。", "整理报告"), vec!["教育部"]);
        assert_eq!(assigned_subjects("请农业部还有教育部要在一周内整理报告。", "整理报告"), vec!["农业部", "教育部"]);
        assert_eq!(assigned_subjects("更正，农业部不再负责整理报告。", "整理报告"), vec!["农业部"]);
        assert_eq!(assigned_subjects("请农业部和教育部负责整理报告。", "整理报告"), vec!["农业部", "教育部"]);
        assert_eq!(assigned_subjects("由农业部和教育部共同负责整理报告。", "整理报告"), vec!["农业部", "教育部"]);
        for (text, owner, status) in [
            ("农业部负责整理报告。更正，教育部负责整理报告。", "农业部", SummaryTraceStatus::NeedsReview),
            ("农业部负责整理报告。更正，教育部负责整理报告。", "教育部", SummaryTraceStatus::Supported),
            ("农业部负责整理报告。整理报告取消。", "农业部", SummaryTraceStatus::NeedsReview),
            ("农业部负责整理报告。整理报告不取消。", "农业部", SummaryTraceStatus::Supported),
            ("农业部可能负责整理报告。", "农业部", SummaryTraceStatus::NeedsReview),
            ("Alice will 整理报告. Alice will not 整理报告.", "Alice", SummaryTraceStatus::NeedsReview),
        ] {
            let traces = trace_owner_and_time_fields(&format!("任务：整理报告；负责人：{owner}"), &t06_source(&[text])).unwrap();
            assert_eq!(traces[0].status, status, "{text}");
        }
    }

    #[test]
    fn generated_task_anchor_preserves_conditions_and_colliding_rows() {
        let source = t06_source(&["请在一个月内修订优化偏乡医疗计划。送院核定之后据以实施。林舟负责整理报告，截止时间：周五。"]);
        let original = "依照上述三项原则，在一个月内修订优化偏乡医疗计划，送院核定之后据以实施";
        let markdown = format!("| 任务 | 截止时间 |\n| --- | --- |\n| {original} | 一个月内 |\n");
        let narrowed = canonicalize_generated_action_tasks(&markdown, &source);
        assert!(narrowed.contains("修订优化偏乡医疗计划（"));
        assert!(narrowed.contains(original), "display must retain conditions and later stages");
        assert_eq!(canonicalize_generated_action_tasks(&narrowed, &source), narrowed, "an existing source anchor must not acquire another wrapper");
        let traces = trace_owner_and_time_fields(&narrowed, &source).unwrap();
        assert_eq!(traces[0].source_task, "修订优化偏乡医疗计划");
        assert_eq!(traces[0].status, SummaryTraceStatus::Supported);
        let collision = "| 任务 | 负责人 |\n| --- | --- |\n| 整理报告，先收集屋顶资料 | 林舟 |\n| 整理报告，先收集医疗资料 | 林舟 |\n";
        assert_eq!(canonicalize_generated_action_tasks(collision, &source), collision);
        let unknown = "任务：落实未宣布的新政策；负责人：林舟";
        assert_eq!(canonicalize_generated_action_tasks(unknown, &source), unknown);
        // Authored reads use tracing, not the generation-only canonicalizer.
        assert_eq!(trace_owner_and_time_fields(&markdown, &source).unwrap()[0].task, original);
    }

    #[test]
    fn oral_deadline_belongs_to_its_immediate_action_and_final_correction() {
        let markdown = "任务：修订优化偏乡医疗计划；截止时间：一个月内";
        let source = t06_source(&["请卫服部依照三项原则，在一个月内修订优化偏乡医疗计划。送院核定之后据以实施。"]);
        assert_eq!(trace_owner_and_time_fields(markdown, &source).unwrap()[0].status, SummaryTraceStatus::Supported);
        let milk = "任务：与地方政府沟通；截止时间：一周内";
        assert_eq!(trace_owner_and_time_fields(milk, &t06_source(&["院长要求农业部在一周之内与地方政府沟通。"])) .unwrap()[0].value, "一周之内");
        for text in [
            "希望农业部在一周内与地方政府沟通。",
            "如果农业部在一周内与地方政府沟通，就继续政策。",
            "农业部不在一周内与地方政府沟通。",
            "去年农业部在一周内与地方政府沟通。",
            "农业部在一到两周内与地方政府沟通。",
            "农业部在一周内与地方政府沟通。更正，与地方政府沟通截止时间：未定。",
        ] { assert_eq!(trace_owner_and_time_fields(milk, &t06_source(&[text])).unwrap()[0].status, SummaryTraceStatus::NeedsReview, "{text}"); }
        let decimal = "任务：完成报告；截止时间：5天内";
        assert_eq!(trace_owner_and_time_fields(decimal, &t06_source(&["请在1.5天内完成报告。"])) .unwrap()[0].status, SummaryTraceStatus::NeedsReview);
        assert_eq!(trace_owner_and_time_fields("任务：核定医疗计划；截止时间：一个月内", &source).unwrap()[0].status, SummaryTraceStatus::NeedsReview);
    }

    #[test]
    fn literal_task_anchor_keeps_display_notes_without_borrowing_fields() {
        let source = t06_source(&["农业部负责与地方政府沟通，截止时间：周五。教育部负责核定乳品计划，截止时间：周六。"]);
        let traces = trace_owner_and_time_fields("任务：与地方政府沟通（乳品政策协调）；负责人：农业部；截止时间：周五", &source).unwrap();
        assert_eq!(traces.len(), 2);
        assert!(traces.iter().all(|trace| trace.status == SummaryTraceStatus::Supported));
        assert!(traces.iter().all(|trace| trace.source_task == "与地方政府沟通"));
        assert!(traces[0].task.contains("乳品政策协调"));
        for markdown in [
            "任务：核定乳品计划（地方政府沟通）；负责人：农业部；截止时间：周五",
            "任务：妥善协调地方政府的乳品政策执行；负责人：农业部",
            "| 任务 | 负责人 |\n| --- | --- |\n| 与地方政府沟通（乳品） | 农业部 |\n| 与地方政府沟通（屋顶） | 农业部 |",
        ] {
            assert!(trace_owner_and_time_fields(markdown, &source).unwrap().iter().all(|trace| trace.status == SummaryTraceStatus::NeedsReview));
        }
        let mut changed = source;
        changed.segments[0].text.push_str("更正");
        assert_eq!(trace_owner_and_time_fields("任务：与地方政府沟通；负责人：农业部", &changed).unwrap_err(), SummarySourceBindingError::TranscriptHashMismatch);
    }

    #[test]
    fn t07_review_slot_checks_combined_fields_inline_duplicates_and_column_meaning() {
        let mut trace = SummaryFieldTrace { field: SummaryTraceField::Time, value: "周五".into(),
            markdown_line: 3, markdown_column: Some(1), status: SummaryTraceStatus::NeedsReview,
            task: "接口回归测试".into(), source_task: String::new(), evidence: Vec::new(), related_evidence: Vec::new() };
        let mixed = "| 任务 | 负责人/截止时间 |\n| --- | --- |\n| 接口回归测试 | MeiL / 待核对 |";
        assert!(review_trace_slot_is_masked(mixed, &trace));
        trace.field = SummaryTraceField::Owner;
        assert!(!review_trace_slot_is_masked(mixed, &trace));
        trace.field = SummaryTraceField::Time;
        assert!(!review_trace_slot_is_masked(&mixed.replace("负责人/截止时间", "自定义列"), &trace));
        trace.markdown_line = 1; trace.markdown_column = None;
        assert!(review_trace_slot_is_masked("任务：接口回归测试；截止时间：待核对", &trace));
        assert!(!review_trace_slot_is_masked("任务：接口回归测试；截止时间：待核对；截止时间：待核对", &trace));
        assert!(!review_trace_slot_is_masked("任务：发布邀请；截止时间：待核对", &trace));
    }
    use chrono::TimeZone;

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 29, 4, 0, 0).unwrap()
    }

    fn t06_source(texts: &[&str]) -> TranscriptVersionSnapshot {
        TranscriptVersionSnapshot::legacy_whisper("t06_virtual", texts.iter().enumerate().map(|(index, text)| TranscriptEvidenceSegment {
            segment_id: format!("t06_virtual_{index}"), start_ms: None, end_ms: None, wall_clock: None,
            anonymous_speaker: None, bound_person_id: None, bound_display_name: None, text: (*text).to_owned(),
        }).collect())
    }

    #[test]
    fn t06_adjacent_fields_keep_anchor_and_field_references_and_stop_at_switches() {
        let markdown = "任务：完成接口回归测试；截止时间：周五";
        let source = t06_source(&["林舟负责完成接口回归测试。", "截止时间为周五。"]);
        let trace = &trace_owner_and_time_fields(markdown, &source).unwrap()[0];
        assert_eq!(trace.status, SummaryTraceStatus::Supported);
        assert_eq!(trace.evidence.iter().map(|reference| reference.segment_id.as_str()).collect::<Vec<_>>(), ["t06_virtual_0", "t06_virtual_1"]);
        for lines in [vec!["如果完成接口回归测试。", "截止时间为周五。"], vec!["完成接口回归测试。", "另外安排发布邀请。", "截止时间为周五。"], vec!["完成接口回归测试。", "陈岚：截止时间为周五。"]] {
            assert_eq!(trace_owner_and_time_fields(markdown, &t06_source(&lines)).unwrap()[0].status, SummaryTraceStatus::NeedsReview);
        }
        let mut switched = source.clone();
        switched.segments[0].anonymous_speaker = Some("S01".into());
        switched.segments[1].anonymous_speaker = Some("S02".into());
        switched.transcript_sha256 = switched.computed_transcript_sha256();
        switched.speaker_binding_sha256 = switched.computed_speaker_binding_sha256();
        assert_eq!(trace_owner_and_time_fields(markdown, &switched).unwrap()[0].status, SummaryTraceStatus::NeedsReview);
        let dependency = "| 行动任务 | 依赖 |\n| --- | --- |\n| 发布试点邀请 | 完成接口回归测试 |\n| 完成接口回归测试 | 会议未提及 |";
        let traces = trace_owner_and_time_fields(dependency, &t06_source(&["发布试点邀请截止时间为周五。", "依赖完成接口回归测试。"])).unwrap();
        assert_eq!(traces[0].status, SummaryTraceStatus::Supported);
        assert_eq!(traces[0].evidence.len(), 2);
    }

    #[test]
    fn t06_controlled_task_verbs_do_not_merge_colliding_tasks() {
        let source = t06_source(&["接口回归测试截止时间为周五。"]);
        assert_eq!(trace_owner_and_time_fields("任务：完成接口回归测试；截止时间：周五", &source).unwrap()[0].status, SummaryTraceStatus::Supported);
        let table = "| 行动任务 | 截止时间 |\n| --- | --- |\n| 完成接口回归测试 | 周五 |\n| 执行接口回归测试 | 周五 |";
        assert!(trace_owner_and_time_fields(table, &source).unwrap().iter().all(|trace| trace.status == SummaryTraceStatus::NeedsReview));
        assert_eq!(trace_owner_and_time_fields("Task: Build model; Deadline: Friday", &t06_source(&["Rebuild model deadline is Friday."])).unwrap()[0].status, SummaryTraceStatus::NeedsReview);
    }

    #[test]
    fn t06_final_deadline_and_iso_render_the_literal_source_without_an_invented_year() {
        let source = t06_source(&["完成接口回归测试截止时间为10月8日18点。", "更正，截止时间改为10月9日12点。"]);
        let markdown = "任务：完成接口回归测试；截止时间：2026-10-09 12:00";
        let traces = trace_owner_and_time_fields(markdown, &source).unwrap();
        assert_eq!(traces[0].status, SummaryTraceStatus::Supported);
        assert_eq!(traces[0].value, "10月9日12点");
        assert_eq!(traces[0].evidence.len(), 2);
        for candidate in ["2026-10-09T12:00:00Z", "2026-10-09T12:00:00.000Z"] {
            let trace = &trace_owner_and_time_fields(&markdown.replace("2026-10-09 12:00", candidate), &source).unwrap()[0];
            assert_eq!(trace.status, SummaryTraceStatus::Supported);
            assert_eq!(trace.value, "10月9日12点");
        }
        for candidate in ["2026-10-09T12:00:01Z", "2026-10-09T12:00:00+08:00"] {
            assert_eq!(trace_owner_and_time_fields(&markdown.replace("2026-10-09 12:00", candidate), &source).unwrap()[0].status, SummaryTraceStatus::NeedsReview);
        }
        assert_eq!(restore_supported_owner_and_time_fields(markdown, &markdown.replace("2026-10-09 12:00", "会议未提及"), &traces), "任务：完成接口回归测试；截止时间：10月9日12点");
        assert_eq!(trace_owner_and_time_fields(&markdown.replace("09", "08"), &source).unwrap()[0].status, SummaryTraceStatus::NeedsReview);
        assert_eq!(trace_owner_and_time_fields("任务：完成接口回归测试；截止时间：14点30分", &t06_source(&["完成接口回归测试讨论到现在14点30分，会议结束。"])).unwrap()[0].status, SummaryTraceStatus::NeedsReview);
    }

    #[test]
    fn t06_recovery_requires_a_unique_assertion_and_preserves_existing_columns() {
        let source = t06_source(&["林舟负责完成接口回归测试，截止时间为周五，验收标准是阻断问题为0。"]);
        let missing = "任务：完成接口回归测试；负责人：会议未提及；截止时间：会议未提及；验收标准：会议未提及";
        let recovered = recover_missing_action_fields(missing, &source, &["林舟".into(), "陈岚".into()]).unwrap();
        assert_eq!(recovered, "任务：完成接口回归测试；负责人：林舟；截止时间：周五；验收标准：阻断问题为0");
        assert_eq!(recover_missing_action_fields("任务：完成接口回归测试；说明：会议未提及", &source, &[]).unwrap(), "任务：完成接口回归测试；说明：会议未提及");
        let conflict = t06_source(&["完成接口回归测试截止时间为周五。", "完成接口回归测试截止时间为周六。"]);
        assert_eq!(recover_missing_action_fields(missing, &conflict, &[]).unwrap(), missing);
    }

    #[test]
    fn t08_omitted_rows_require_literal_unique_assignments_and_known_layout() {
        let owners = vec!["林舟".into(), "陈岚".into()];
        let header = "| **Deliverable** | **Owner** | **Due Date** |\n| --- | --- | --- |";
        let draft = format!("{header}\n| 接口回归测试 | 林舟 | 10月9日18点 |\n");
        let source = t06_source(&["林舟负责完成接口回归测试，截止时间为10月9日18点。", "陈岚负责发布试点邀请。", "截止时间为10月10日12点。"]);
        let recovered = recover_missing_action_fields(&draft, &source, &owners).unwrap();
        assert!(recovered.contains("| 发布试点邀请 | 陈岚 | 10月10日12点 |"));
        assert_eq!(table_cells(recovered.lines().nth(0).unwrap()), table_cells(header.lines().next().unwrap()));
        assert_eq!(recovered.lines().filter(|line| line.contains("接口回归测试")).count(), 1);
        let traces = trace_owner_and_time_fields(&recovered, &source).unwrap();
        assert_eq!(traces.len(), 4); assert!(traces.iter().all(|trace| trace.status == SummaryTraceStatus::Supported));
        assert_eq!(recover_missing_action_fields(&recovered, &source, &owners).unwrap(), recovered);
        let empty = recover_missing_action_fields(header, &source, &owners).unwrap();
        assert_eq!(trace_owner_and_time_fields(&empty, &source).unwrap().len(), 4);
        for text in ["如果林舟负责完成接口回归测试，陈岚负责发布试点邀请。", "例如陈岚负责发布试点邀请。", "陈岚负责发布试点邀请？", "陈岚不负责发布试点邀请。", "陈岚负责发布试点邀请和发送纪要。", "陈岚负责发布试点邀请，只是举例。", "陈岚负责发布试点邀请，但尚未确认。", "陌生甲负责发布试点邀请。", "我负责发布试点邀请。", "若审批通过，陈岚负责发布试点邀请。"] {
            assert_eq!(recover_missing_action_fields(&draft, &t06_source(&[text]), &owners).unwrap(), draft, "{text}");
        }
        let conflict = t06_source(&["陈岚负责发布试点邀请。", "林舟负责发布试点邀请。"]);
        assert_eq!(recover_missing_action_fields(&draft, &conflict, &owners).unwrap(), draft);
        for revoked in ["发布试点邀请取消。", "陈岚不负责发布试点邀请。", "发布试点邀请没有约定。", "该任务取消。"] {
            assert_eq!(recover_missing_action_fields(&draft, &t06_source(&["陈岚负责发布试点邀请。", revoked]), &owners).unwrap(), draft);
        }
        for layout in [draft.replace("**Due Date**", "Success Metric"), format!("{draft}\n{draft}"), format!("```\n{draft}```\n"), draft.replace("**Deliverable**", "Decision")] {
            assert_eq!(recover_missing_action_fields(&layout, &source, &owners).unwrap(), layout);
        }
        assert_eq!(recover_missing_action_rows(&draft, &source, &[]), draft);
        for text in ["陈岚 will not send invitations.", "陈岚 will never send invitations.", "陈岚不承担发布试点邀请。"] {
            assert_eq!(recover_missing_action_fields(header, &t06_source(&[text]), &owners).unwrap(), header);
        }
        assert_eq!(trace_owner_and_time_fields("Task: send invitations; Owner: 陈岚", &t06_source(&["陈岚 will not send invitations."])).unwrap()[0].status, SummaryTraceStatus::NeedsReview);
        for text in ["I will not send invitations.", "I will never send invitations.", "I'll not send invitations."] {
            let mut source = t06_source(&[text]);
            source.segments[0].bound_person_id = Some("person_chen".into()); source.segments[0].bound_display_name = Some("陈岚".into());
            source.speaker_binding_sha256 = source.computed_speaker_binding_sha256();
            assert_eq!(trace_owner_and_time_fields("Task: send invitations; Owner: 陈岚", &source).unwrap()[0].status, SummaryTraceStatus::NeedsReview);
        }
    }

    #[test]
    fn t06_owner_proof_cannot_borrow_a_dependency_object_or_another_clause() {
        for text in ["陈岚负责发布邀请，依赖完成接口回归测试。", "林舟负责完成接口回归测试，陈岚负责发布邀请。"] {
            assert_eq!(trace_owner_and_time_fields("任务：完成接口回归测试；负责人：陈岚", &t06_source(&[text])).unwrap()[0].status, SummaryTraceStatus::NeedsReview);
        }
        let mut source = t06_source(&["我负责发布邀请，完成接口回归测试只是举例。"]);
        source.segments[0].bound_display_name = Some("陈岚".into());
        source.segments[0].bound_person_id = Some("person_chen".into());
        source.speaker_binding_sha256 = source.computed_speaker_binding_sha256();
        assert_eq!(trace_owner_and_time_fields("任务：完成接口回归测试；负责人：陈岚", &source).unwrap()[0].status, SummaryTraceStatus::NeedsReview);
    }

    #[test]
    fn t08_merged_asr_task_assignments_do_not_mix_attributes() {
        let markdown = "| 行动任务 | 负责人 | 截止时间 | 验收标准 | 当前状态 | 依赖或卡点 |\n| --- | --- | --- | --- | --- | --- |\n| 完成接口回归测试 | 林舟 | 10月9日18点 | 阻断问题为0 | 进行中 | 供应商审批通过 |\n| 发布试点邀请 | 陈岚 | 10月10日12点 | 20名试点客户全部收到邀请 | 未开始 | 接口回归测试通过 |";
        let source = t06_source(&["林舟负责完成接口回归测试，截止时间是10月9日18点，验收标准是阻断问题为0，当前状态是进行中，依赖供应商审批通过，陈岚负责发布试点邀请，截止时间是10月10日12点，验收标准是20名试点客户全部收到邀请，当前状态是未开始，依赖接口回归测试通过，其他任务没有约定。"]);
        let traces = trace_owner_and_time_fields(markdown, &source).unwrap();
        assert_eq!(traces.len(), 10);
        assert!(traces.iter().all(|trace| trace.status == SummaryTraceStatus::Supported));
        assert!(traces.iter().all(|trace| trace.evidence[0].segment_id == source.segments[0].segment_id));
        assert!(traces.iter().all(|trace| trace.evidence[0].excerpt_sha256 == sha256_text(source.segments[0].text.trim())));
        let swapped = "| 行动任务 | 负责人 | 截止时间 | 验收标准 | 当前状态 | 依赖或卡点 |\n| --- | --- | --- | --- | --- | --- |\n| 完成接口回归测试 | 林舟 | 10月10日12点 | 20名试点客户全部收到邀请 | 未开始 | 接口回归测试通过 |\n| 发布试点邀请 | 陈岚 | 10月9日18点 | 阻断问题为0 | 进行中 | 供应商审批通过 |";
        let swapped = trace_owner_and_time_fields(swapped, &source).unwrap();
        assert!(swapped.iter().filter(|trace| trace.field != SummaryTraceField::Owner).all(|trace| trace.status == SummaryTraceStatus::NeedsReview));
        for prefix in ["如果", "例如", "If ", "Provided ", "Suppose "] {
            let conditional = t06_source(&[&format!("{prefix}{}", source.segments[0].text)]);
            assert!(trace_owner_and_time_fields(markdown, &conditional).unwrap().iter()
                .all(|trace| trace.status == SummaryTraceStatus::NeedsReview));
        }

        // Actual E01 ASR omitted the boundary before the second assignment and
        // misheard both names. Preserve the first task's clear attributes, while
        // the ambiguous dependency/next assignment and unmatched names need review.
        let source = t06_source(&["林州负责完成接口回归测试，截止时间是10月9日18点，验收标准是阻断问题为0，当前状态是进行中，依赖供应商审批通过陈兰负责发布试点邀请，截止时间是10月10日12点，验收标准是20名试点客户全部收到邀请，当前状态是未开始，依赖接口回归测试通过，其他任务没有约定，这些都是虚构测试材料。"]);
        let traces = trace_owner_and_time_fields(markdown, &source).unwrap();
        assert_eq!(traces.iter().map(|trace| trace.status).collect::<Vec<_>>(), vec![
            SummaryTraceStatus::NeedsReview, SummaryTraceStatus::Supported,
            SummaryTraceStatus::Supported, SummaryTraceStatus::Supported,
            SummaryTraceStatus::NeedsReview, SummaryTraceStatus::NeedsReview,
            SummaryTraceStatus::NeedsReview, SummaryTraceStatus::NeedsReview,
            SummaryTraceStatus::NeedsReview, SummaryTraceStatus::NeedsReview,
        ]);
    }

    fn segments() -> Vec<TranscriptEvidenceSegment> {
        vec![
            TranscriptEvidenceSegment {
                segment_id: "seg_1".to_owned(),
                start_ms: Some(1_000),
                end_ms: Some(4_000),
                wall_clock: None,
                anonymous_speaker: Some("S01".to_owned()),
                bound_person_id: Some("person_rayson".to_owned()),
                bound_display_name: Some("Rayson".to_owned()),
                text: "Rayson 负责整理报告，截止时间是周五。".to_owned(),
            },
            TranscriptEvidenceSegment {
                segment_id: "seg_2".to_owned(),
                start_ms: Some(4_000),
                end_ms: Some(8_000),
                wall_clock: None,
                anonymous_speaker: Some("S02".to_owned()),
                bound_person_id: None,
                bound_display_name: None,
                text: "S02 表示仍需确认接口时间。".to_owned(),
            },
        ]
    }

    fn moss_source(state: TranscriptVersionState) -> TranscriptVersionSnapshot {
        let mut source = TranscriptVersionSnapshot {
            schema_version: SUMMARY_SOURCE_BINDING_SCHEMA_VERSION,
            meeting_id: "meeting_1".to_owned(),
            transcript_version_id: "version_moss_1".to_owned(),
            transcript_version: 2,
            source_kind: TranscriptSourceKind::Moss,
            moss_run_id: Some("run_1".to_owned()),
            state,
            activated_at: (state == TranscriptVersionState::Active).then_some(at()),
            transcript_sha256: String::new(),
            speaker_binding_snapshot_id: "binding_1".to_owned(),
            speaker_binding_version: 3,
            speaker_binding_sha256: String::new(),
            segments: segments(),
        };
        source.transcript_sha256 = source.computed_transcript_sha256();
        source.speaker_binding_sha256 = source.computed_speaker_binding_sha256();
        source
    }

    fn template() -> SummaryTemplateBinding {
        SummaryTemplateBinding {
            template_id: "standard_meeting".to_owned(),
            template_version: 4,
            template_file_sha256: "a".repeat(64),
            template_semantic_sha256: "b".repeat(64),
        }
    }

    #[test]
    fn candidate_is_ignored_and_only_the_active_version_can_feed_summary() {
        let candidate = moss_source(TranscriptVersionState::Candidate);
        let failed = moss_source(TranscriptVersionState::Failed);
        let whisper = TranscriptVersionSnapshot::legacy_whisper(
            "meeting_1",
            vec![TranscriptEvidenceSegment {
                segment_id: "legacy_1".to_owned(),
                start_ms: Some(0),
                end_ms: Some(1_000),
                wall_clock: None,
                anonymous_speaker: None,
                bound_person_id: None,
                bound_display_name: None,
                text: "当前 Whisper 正文".to_owned(),
            }],
        );
        let selected =
            select_activated_summary_source("meeting_1", &[candidate, failed, whisper]).unwrap();
        assert_eq!(selected.source.source_kind, TranscriptSourceKind::Whisper);
        assert_eq!(selected.source.activated_at, None);
        assert!(selected.transcript_text.contains("当前 Whisper 正文"));
    }

    #[test]
    fn an_explicit_active_manual_version_is_supported_without_being_mislabeled_as_moss() {
        let mut manual = TranscriptVersionSnapshot::legacy_whisper("meeting_1", segments());
        manual.transcript_version_id = "version_manual_1".to_owned();
        manual.transcript_version = 3;
        manual.source_kind = TranscriptSourceKind::Manual;

        let selected = select_activated_summary_source("meeting_1", &[manual]).unwrap();
        assert_eq!(selected.source.source_kind, TranscriptSourceKind::Manual);
        assert_eq!(selected.source.moss_run_id, None);
    }

    #[test]
    fn activated_moss_keeps_unbound_anonymous_speaker_and_uses_bound_name_only_where_known() {
        let selected = select_activated_summary_source(
            "meeting_1",
            &[moss_source(TranscriptVersionState::Active)],
        )
        .unwrap();
        assert!(selected.transcript_text.contains("[00:01] Rayson:"));
        assert!(selected.transcript_text.contains("[00:04] S02:"));
        assert!(!selected.transcript_text.contains("Unknown"));
    }

    #[test]
    fn tampered_content_or_binding_hash_is_rejected() {
        let mut content = moss_source(TranscriptVersionState::Active);
        content.segments[0].text.push_str("篡改");
        assert_eq!(
            content.validate_active().unwrap_err(),
            SummarySourceBindingError::TranscriptHashMismatch
        );

        let mut binding = moss_source(TranscriptVersionState::Active);
        binding.segments[0].bound_display_name = Some("Other".to_owned());
        assert_eq!(
            binding.validate_active().unwrap_err(),
            SummarySourceBindingError::SpeakerBindingHashMismatch
        );
    }

    #[test]
    fn multiple_or_missing_active_versions_fail_closed() {
        let active = moss_source(TranscriptVersionState::Active);
        assert_eq!(
            select_activated_summary_source("meeting_1", &[]).unwrap_err(),
            SummarySourceBindingError::NoActiveVersion
        );
        assert_eq!(
            select_activated_summary_source("meeting_1", &[active.clone(), active]).unwrap_err(),
            SummarySourceBindingError::MultipleActiveVersions
        );
    }

    #[test]
    fn active_moss_without_a_recorded_activation_time_fails_closed() {
        let mut source = moss_source(TranscriptVersionState::Active);
        source.activated_at = None;

        assert_eq!(
            select_activated_summary_source("meeting_1", &[source]).unwrap_err(),
            SummarySourceBindingError::ActivationInvalid
        );
    }

    #[test]
    fn lineage_distinguishes_transcript_binding_and_template_staleness() {
        let source = moss_source(TranscriptVersionState::Active);
        let generated = SummarySourceBinding::from_active_source(&source, template()).unwrap();
        assert_eq!(
            evaluate_summary_freshness(&generated, &generated)
                .unwrap()
                .status,
            SummaryFreshnessStatus::Current
        );

        let mut current = generated.clone();
        current.transcript_sha256 = "c".repeat(64);
        current.speaker_binding_sha256 = "d".repeat(64);
        current.template.template_version += 1;
        let freshness = evaluate_summary_freshness(&generated, &current).unwrap();
        assert_eq!(freshness.status, SummaryFreshnessStatus::Stale);
        assert_eq!(
            freshness.reasons,
            vec![
                SummaryStaleReason::TranscriptContentChanged,
                SummaryStaleReason::SpeakerBindingsChanged,
                SummaryStaleReason::TemplateChanged,
            ]
        );
    }

    #[test]
    fn t05_relation_words_in_a_cell_do_not_hide_literal_source_evidence() {
        let mut source = moss_source(TranscriptVersionState::Active);
        source.segments[0].text = "整理报告目前卡在供应商审批。发布版本依赖接口回归测试通过。".into(); source.transcript_sha256 = source.computed_transcript_sha256();
        let traces = trace_owner_and_time_fields("| 任务 | 依赖或卡点 |\n| --- | --- |\n| 整理报告 | 卡在供应商审批 |\n| 发布版本 | 依赖接口回归测试通过 |", &source).unwrap();
        assert_eq!(traces.len(), 2);
        assert_eq!(traces[0].field, SummaryTraceField::Blocker); assert_eq!(traces[1].field, SummaryTraceField::Dependency);
        assert!(traces.iter().all(|trace| trace.status == SummaryTraceStatus::Supported));
    }
    #[test]
    fn t05_dependency_and_blocker_use_their_own_relations() {
        let mut source = moss_source(TranscriptVersionState::Active);
        source.segments[0].text = "整理报告的前提是测试环境就绪，卡点是供应商审批。".into();
        source.transcript_sha256 = source.computed_transcript_sha256();
        let traces = trace_owner_and_time_fields("任务：整理报告；依赖或卡点：依赖：测试环境就绪；卡点：供应商审批", &source).unwrap();
        assert_eq!(traces.len(), 2);
        assert!(traces.iter().all(|t| t.status == SummaryTraceStatus::Supported));
        let blocker = trace_owner_and_time_fields("任务：整理报告；依赖或卡点：供应商审批", &source).unwrap();
        assert_eq!(blocker.len(), 1); assert_eq!(blocker[0].field, SummaryTraceField::Blocker);
        assert_eq!(blocker[0].status, SummaryTraceStatus::Supported);
    }
    #[test]
    fn t05_risk_negation_direction_and_uncertainty_do_not_prove_a_relation() {
        for text in ["整理报告不依赖供应商审批。", "整理报告存在供应商审批风险。", "如果整理报告依赖供应商审批，就通知大家。", "整理报告依赖供应商审批，但此依赖尚未确认。", "发布版本依赖整理报告。"] {
            let mut source = moss_source(TranscriptVersionState::Active);
            source.segments[0].text = text.into(); source.transcript_sha256 = source.computed_transcript_sha256();
            let traces = trace_owner_and_time_fields("任务：整理报告；依赖或卡点：供应商审批", &source).unwrap();
            assert_eq!(traces.len(), 1); assert_eq!(traces[0].status, SummaryTraceStatus::NeedsReview, "{text}");
        }
    }
    #[test]
    fn t05_none_is_a_fact_needing_proof_and_unknown_is_not_none() {
        for (text, label, value, expected) in [
            ("整理报告目前没有依赖。", "依赖", "无", SummaryTraceStatus::Supported),
            ("整理报告目前没有卡点。", "卡点", "无", SummaryTraceStatus::Supported),
            ("整理报告的依赖是尚未确定。", "依赖", "尚未确定", SummaryTraceStatus::Supported),
            ("整理报告没有卡点。", "依赖", "无", SummaryTraceStatus::NeedsReview),
            ("整理报告只分配了负责人。", "依赖", "无", SummaryTraceStatus::NeedsReview),
            ("Run regression tests has no blockers.", "Blocker", "None", SummaryTraceStatus::Supported),
        ] {
            let mut source = moss_source(TranscriptVersionState::Active);
            source.segments[0].text = text.into(); source.transcript_sha256 = source.computed_transcript_sha256();
            let task = if text.is_ascii() { "Run regression tests" } else { "整理报告" };
            let traces = trace_owner_and_time_fields(&format!("任务：{task}；{label}：{value}"), &source).unwrap();
            assert_eq!(traces.len(), 1); assert_eq!(traces[0].status, expected, "{text}");
        }
    }
    #[test]
    fn t05_known_dependency_object_does_not_replace_the_task_subject() {
        let mut source = moss_source(TranscriptVersionState::Active);
        source.segments[0].text = "发布版本依赖整理报告，卡点是供应商审批。".into(); source.transcript_sha256 = source.computed_transcript_sha256();
        let traces = trace_owner_and_time_fields("| 任务 | 依赖或卡点 |\n| --- | --- |\n| 发布版本 | 依赖：整理报告；卡点：供应商审批 |\n| 整理报告 | 会议未提及 |", &source).unwrap();
        assert_eq!(traces.len(), 2); assert!(traces.iter().all(|t| t.status == SummaryTraceStatus::Supported));
    }
    #[test]
    fn t05_unverified_combined_tail_and_ambiguous_meaning_stay_for_review() {
        let mut source = moss_source(TranscriptVersionState::Active);
        source.segments[0].text = "整理报告依赖供应商审批，卡点是供应商审批。".into(); source.transcript_sha256 = source.computed_transcript_sha256();
        let original = "| 任务 | 依赖或卡点 |\n| --- | --- |\n| 整理报告 | 供应商审批 |";
        let traces = trace_owner_and_time_fields(original, &source).unwrap();
        assert_eq!(traces.len(), 1); assert_eq!(traces[0].status, SummaryTraceStatus::NeedsReview);
        let restored = restore_supported_owner_and_time_fields(original, &original.replace("| 整理报告 | 供应商审批 |", "| 整理报告 | 会议未提及 |"), &traces);
        assert!(restored.ends_with("| 整理报告 | 待核对 |"));
    }
    #[test]
    fn t04_acceptance_and_status_use_explicit_task_attributes() {
        let source = TranscriptVersionSnapshot::legacy_whisper("attributes", vec![TranscriptEvidenceSegment {
            segment_id: "criteria_source".into(), start_ms: Some(1_000), end_ms: Some(3_000), wall_clock: None,
            anonymous_speaker: None, bound_person_id: None, bound_display_name: None,
            text: "整理报告，验收标准是100个用例通过，当前状态是进行中。发布版本，验收标准是20名客户收到邀请，当前状态是未开始。".into(),
        }]);
        let markdown = "| 行动任务 | 验收标准 | 当前状态 |\n| --- | --- | --- |\n| 整理报告 | 100个用例通过 | 进行中 |\n| 发布版本 | 20名客户收到邀请 | 未开始 |";
        let traces = trace_owner_and_time_fields(markdown, &source).unwrap();
        assert_eq!(traces.len(), 4);
        assert!(traces.iter().all(|trace| trace.status == SummaryTraceStatus::Supported && trace.evidence[0].segment_id == "criteria_source" && trace.evidence[0].excerpt_sha256.len() == 64));
    }

    #[test]
    fn t04_future_negation_uncertainty_and_other_tasks_are_not_proof() {
        for (text, value) in [
            ("整理报告计划下周开始。", "进行中"),
            ("整理报告当前状态预计为已完成。", "已完成"),
            ("整理报告尚未完成。", "已完成"),
            ("如果整理报告当前状态是已完成，就通知客户。", "已完成"),
            ("发布版本的前提是整理报告通过，当前状态是未开始。", "未开始"),
            ("整理报告和发布版本，当前状态是进行中。", "进行中"),
            ("整理报告当前状态是进行中，但尚未确认。", "进行中"),
        ] {
            let source = TranscriptVersionSnapshot::legacy_whisper("attributes", vec![TranscriptEvidenceSegment {
                segment_id: "negative_source".into(), start_ms: None, end_ms: None, wall_clock: None,
                anonymous_speaker: None, bound_person_id: None, bound_display_name: None, text: text.into(),
            }]);
            let markdown = format!("任务：整理报告；当前状态：{value}\n任务：发布版本；当前状态：会议未提及");
            let traces = trace_owner_and_time_fields(&markdown, &source).unwrap();
            assert_eq!(traces.len(), 1, "{text}");
            assert_eq!(traces[0].status, SummaryTraceStatus::NeedsReview, "{text}");
        }
    }

    #[test]
    fn t04_asr_commas_keep_complete_acceptance_and_field_boundaries() {
        for (spoken, expected) in [
            ("验收标准是20名试点，客户全部收到邀请。", "20名试点，客户全部收到邀请"),
            ("验收标准是20名试点，客户全部收到邀请，当前状态是未开始。", "20名试点，客户全部收到邀请"),
            ("验收标准是20名试点，客户全部收到邀请，整理报告当前状态是进行中。", "20名试点，客户全部收到邀请"),
            ("验收标准是20名试点，客户全部收到邀请，更正，验收标准改为10名客户收到邀请。", "10名客户收到邀请"),
        ] {
            let source = TranscriptVersionSnapshot::legacy_whisper("asr-criteria", [
                "陈兰负责发布试点邀请。", spoken,
            ].iter().enumerate().map(|(index, text)| TranscriptEvidenceSegment {
                segment_id: format!("asr_{index}"), start_ms: Some(index as u64 * 6000),
                end_ms: Some(index as u64 * 6000 + 3000), wall_clock: None,
                anonymous_speaker: None, bound_person_id: None, bound_display_name: None, text: (*text).into(),
            }).collect());
            let values = source_action_values("发布试点邀请", &["发布试点邀请".into(), "整理报告".into()], &source);
            assert_eq!(values.iter().find(|(field, _)| *field == SummaryTraceField::Acceptance).map(|(_, value)| value.as_str()), Some(expected), "{spoken}");
            let markdown = format!("任务：发布试点邀请；验收标准：{expected}\n任务：整理报告；验收标准：会议未提及");
            assert_eq!(trace_owner_and_time_fields(&markdown, &source).unwrap()[0].status, SummaryTraceStatus::Supported, "{spoken}");
            let weakened = markdown.replace(expected, "20名试点");
            assert_eq!(trace_owner_and_time_fields(&weakened, &source).unwrap()[0].status, SummaryTraceStatus::NeedsReview, "{spoken}");
        }
    }

    #[test]
    fn t04_direct_states_and_later_uncertain_corrections_are_distinct() {
        for (text, value, expected) in [
            ("整理报告目前进行中。", "进行中", SummaryTraceStatus::Supported),
            ("整理报告已完成。", "已完成", SummaryTraceStatus::Supported),
            ("整理报告尚未完成。", "尚未完成", SummaryTraceStatus::Supported),
            ("整理报告已完成后才开始下一步。", "已完成", SummaryTraceStatus::NeedsReview),
            ("我没说整理报告已完成。", "已完成", SummaryTraceStatus::NeedsReview),
            ("整理报告当前状态是进行中。更正，整理报告当前状态可能是进行中。", "进行中", SummaryTraceStatus::NeedsReview),
            ("整理报告当前状态是进行中。更正，整理报告当前状态尚未确定。", "进行中", SummaryTraceStatus::NeedsReview),
            ("整理报告当前状态是进行中。更正，整理报告当前状态尚未确定。", "尚未确定", SummaryTraceStatus::Supported),
            ("整理报告当前状态是进行中，不对，当前状态改为未开始。", "未开始", SummaryTraceStatus::Supported),
            ("整理报告当前状态是进行中，不对，当前状态改为未开始。", "进行中", SummaryTraceStatus::NeedsReview),
            ("整理报告当前状态是进行中，当前状态是未开始。", "进行中", SummaryTraceStatus::NeedsReview),
        ] {
            let source = TranscriptVersionSnapshot::legacy_whisper("attributes", vec![TranscriptEvidenceSegment {
                segment_id: "direct_source".into(), start_ms: None, end_ms: None, wall_clock: None,
                anonymous_speaker: None, bound_person_id: None, bound_display_name: None, text: text.into(),
            }]);
            let traces = trace_owner_and_time_fields(&format!("任务：整理报告；当前状态：{value}"), &source).unwrap();
            assert_eq!(traces.len(), 1, "{text}");
            assert_eq!(traces[0].status, expected, "{text}");
        }
    }

    #[test]
    fn t04_comparison_operators_decimals_and_questions_keep_their_meaning() {
        for (text, field, value, expected) in [
            ("整理报告验收标准是阻断问题=0。", "验收标准", "阻断问题>0", SummaryTraceStatus::NeedsReview),
            ("整理报告验收标准是响应时间不超过2.0秒。", "验收标准", "响应时间不超过20秒", SummaryTraceStatus::NeedsReview),
            ("整理报告验收标准是响应时间不超过2.0秒。", "验收标准", "响应时间不超过2.0秒", SummaryTraceStatus::Supported),
            ("整理报告进行中？", "当前状态", "进行中", SummaryTraceStatus::NeedsReview),
        ] {
            let source = TranscriptVersionSnapshot::legacy_whisper("attributes", vec![TranscriptEvidenceSegment {
                segment_id: "numeric_source".into(), start_ms: None, end_ms: None, wall_clock: None,
                anonymous_speaker: None, bound_person_id: None, bound_display_name: None, text: text.into(),
            }]);
            let traces = trace_owner_and_time_fields(&format!("任务：整理报告；{field}：{value}"), &source).unwrap();
            assert_eq!(traces[0].status, expected, "{text}");
        }
    }

    #[test]
    fn t04_attribute_words_inside_task_names_do_not_change_the_subject() {
        for task in ["制定行动计划", "整理状态报告"] {
            let source = TranscriptVersionSnapshot::legacy_whisper("attributes", vec![TranscriptEvidenceSegment {
                segment_id: "task_words".into(), start_ms: None, end_ms: None, wall_clock: None,
                anonymous_speaker: None, bound_person_id: None, bound_display_name: None,
                text: format!("{task}验收标准是提交1份报告，当前状态是进行中。"),
            }]);
            let traces = trace_owner_and_time_fields(&format!("任务：{task}；验收标准：提交1份报告；当前状态：进行中"), &source).unwrap();
            assert_eq!(traces.len(), 2);
            assert!(traces.iter().all(|trace| trace.status == SummaryTraceStatus::Supported), "{task}");
        }
    }

    #[test]
    fn t04_uncertainty_is_checked_for_each_field_not_the_whole_sentence() {
        for (text, acceptance) in [
            ("整理报告验收标准是阻断问题为0，当前状态是进行中，依赖尚未确认。", SummaryTraceStatus::Supported),
            ("整理报告验收标准可能是阻断问题为0，当前状态是进行中。", SummaryTraceStatus::NeedsReview),
        ] {
            let source = TranscriptVersionSnapshot::legacy_whisper("attributes", vec![TranscriptEvidenceSegment {
                segment_id: "field_scope".into(), start_ms: None, end_ms: None, wall_clock: None,
                anonymous_speaker: None, bound_person_id: None, bound_display_name: None, text: text.into(),
            }]);
            let traces = trace_owner_and_time_fields("任务：整理报告；验收标准：阻断问题为0；当前状态：进行中", &source).unwrap();
            assert_eq!(traces[0].status, acceptance);
            assert_eq!(traces[1].status, SummaryTraceStatus::Supported);
        }
    }

    #[test]
    fn t04_status_correction_keeps_decisive_and_prior_references() {
        let source = TranscriptVersionSnapshot::legacy_whisper("attributes", ["整理报告当前状态是进行中。", "更正，整理报告当前状态改为未开始。", "整理报告当前状态是未开始。"].iter().enumerate().map(|(index, text)| TranscriptEvidenceSegment {
            segment_id: format!("correction_{index}"), start_ms: None, end_ms: None, wall_clock: None,
            anonymous_speaker: None, bound_person_id: None, bound_display_name: None, text: (*text).into(),
        }).collect());
        let traces = trace_owner_and_time_fields("任务：整理报告；当前状态：未开始", &source).unwrap();
        assert_eq!(traces[0].status, SummaryTraceStatus::Supported);
        assert_eq!(traces[0].evidence.len(), 3);
        let old = trace_owner_and_time_fields("任务：整理报告；当前状态：进行中", &source).unwrap();
        assert_eq!(old[0].status, SummaryTraceStatus::NeedsReview);
    }

    #[test]
    fn t03_deliverable_reordered_bold_aliases_use_the_same_evidence() {
        let source = moss_source(TranscriptVersionState::Active);
        let markdown = "| **Due  Date** | 责任人 | Deliverable | 保留列 |
| --- | --- | --- | --- |
| 周五 | Rayson | 整理报告 | **原样** |";
        let traces = trace_owner_and_time_fields(markdown, &source).unwrap();
        assert_eq!(traces.len(), 2); assert!(traces.iter().all(|trace| trace.status == SummaryTraceStatus::Supported));
        assert!(traces.iter().all(|trace| trace.task == "整理报告"));
        let restored = restore_supported_owner_and_time_fields(markdown, &markdown.replace("| 周五 | Rayson |", "| 会议未提及 | 会议未提及 |"), &traces);
        assert_eq!(restored, markdown);
    }
    #[test]
    fn t03_unknown_task_headers_do_not_supply_an_action_anchor() {
        let source = moss_source(TranscriptVersionState::Active);
        let traces = trace_owner_and_time_fields("| Deliverable Notes | Due Date |
| --- | --- |
| 整理报告 | 周五 |", &source).unwrap();
        assert_eq!(traces.len(), 1); assert_eq!(traces[0].status, SummaryTraceStatus::NeedsReview); assert!(traces[0].task.is_empty());
    }
    #[test]
    fn t03_combined_cell_requires_every_claim_to_be_supported() {
        let source = moss_source(TranscriptVersionState::Active);
        let original = "| 行动任务 | 截止时间/验收标准 |
| --- | --- |
| 整理报告 | 截止时间：周五；验收标准：100个用例通过 |";
        let traces = trace_owner_and_time_fields(original, &source).unwrap();
        assert_eq!(traces.len(), 2);
        assert_eq!(traces.iter().find(|trace| trace.field == SummaryTraceField::Time).unwrap().status, SummaryTraceStatus::Supported);
        assert_eq!(traces.iter().find(|trace| trace.field == SummaryTraceField::Acceptance).unwrap().status, SummaryTraceStatus::NeedsReview);
        let masked = "| 行动任务 | 截止时间/验收标准 |
| --- | --- |
| 整理报告 | 会议未提及 |";
        // T06 keeps only the proven component; the other candidate still needs review.
        assert_eq!(restore_supported_owner_and_time_fields(original, masked, &traces), masked.replace("会议未提及", "截止时间：周五；验收标准：待核对"));
        let bare = original.replace("截止时间：周五；验收标准：100个用例通过", "周五 / 100个用例通过");
        let traces = trace_owner_and_time_fields(&bare, &source).unwrap();
        assert_eq!(restore_supported_owner_and_time_fields(&bare, masked, &traces), masked.replace("会议未提及", "周五 / 待核对"));
    }
    #[test]
    fn t03_inline_restoration_keeps_the_unsupported_slot_masked() {
        let source = moss_source(TranscriptVersionState::Active);
        let original = "任务：整理报告；责任人：未知人；负责人：Rayson；截止日期：周五";
        let traces = trace_owner_and_time_fields(original, &source).unwrap();
        let masked = "任务：整理报告；责任人：会议未提及；负责人：会议未提及；截止日期：会议未提及";
        let restored = restore_supported_owner_and_time_fields(original, masked, &traces);
        assert_eq!(restored, "任务：整理报告；责任人：待核对；负责人：Rayson；截止日期：周五");
    }
    #[test]
    fn t03_fenced_tables_are_examples_not_field_claims() {
        let source = moss_source(TranscriptVersionState::Active);
        assert!(trace_owner_and_time_fields("```markdown
| 行动任务 | 负责人 |
| --- | --- |
| 整理报告 | 未知人 |
```", &source).unwrap().is_empty());
    }

    #[test]
    fn owner_and_time_traces_include_segment_ids_timestamps_and_hashes() {
        let source = moss_source(TranscriptVersionState::Active);
        let markdown = r#"| 行动项 | 负责人 | 截止时间 |
| --- | --- | --- |
| 整理报告 | Rayson | 周五 |
| 发布版本 | MeiL | 下周一 |"#;
        let traces = trace_owner_and_time_fields(markdown, &source).unwrap();
        assert_eq!(traces.len(), 4);
        let owner = traces.iter().find(|trace| trace.value == "Rayson").unwrap();
        assert_eq!(owner.status, SummaryTraceStatus::Supported);
        assert_eq!(owner.evidence[0].segment_id, "seg_1");
        assert_eq!(owner.evidence[0].start_ms, Some(1_000));
        assert_eq!(owner.evidence[0].excerpt_sha256.len(), 64);
        assert!(traces
            .iter()
            .filter(|trace| matches!(trace.value.as_str(), "MeiL" | "下周一"))
            .all(|trace| trace.status == SummaryTraceStatus::NeedsReview));
    }

    #[test]
    fn matching_value_without_the_same_action_is_not_treated_as_evidence() {
        let source = moss_source(TranscriptVersionState::Active);
        let traces = trace_owner_and_time_fields(
            r#"| 行动项 | 负责人 | 截止时间 |
| --- | --- | --- |
| 发布版本 | Rayson | 周五 |"#,
            &source,
        )
        .unwrap();

        assert_eq!(traces.len(), 2);
        assert!(traces
            .iter()
            .all(|trace| trace.status == SummaryTraceStatus::NeedsReview));
        assert!(traces.iter().all(|trace| trace.evidence.is_empty()));
    }

    #[test]
    fn supported_table_fields_can_be_restored_without_restoring_unverified_cells() {
        let source = moss_source(TranscriptVersionState::Active);
        let original = r#"| 行动项 | 负责人 | 截止时间 |
| --- | --- | --- |
| 整理报告 | Rayson | 周五 |
| 发布版本 | MeiL | 下周一 |"#;
        let sanitized = r#"| 行动项 | 负责人 | 截止时间 |
| --- | --- | --- |
| 整理报告 | 会议未提及 | 会议未提及 |
| 发布版本 | 会议未提及 | 会议未提及 |"#;
        let traces = trace_owner_and_time_fields(original, &source).unwrap();
        let restored = restore_supported_owner_and_time_fields(original, sanitized, &traces);
        assert!(restored.contains("| 整理报告 | Rayson | 周五 |"));
        assert!(restored.contains("| 发布版本 | 待核对 | 待核对 |"));
    }

    #[test]
    fn short_deadline_label_is_traced() {
        let source = moss_source(TranscriptVersionState::Active);
        let traces = trace_owner_and_time_fields(
            "任务：整理报告；负责人：Rayson；截止：周五", &source,
        ).unwrap();
        assert_eq!(traces.len(), 2);
        assert!(traces.iter().any(|trace| trace.field == SummaryTraceField::Time));
    }

    #[test]
    fn related_task_retrieval_does_not_certify_a_wrong_deadline() {
        let mut source = moss_source(TranscriptVersionState::Active);
        source.segments[0].text = "我今天18点前把纪要发给四位参会人。现在14点30分，会议结束。".into();
        source.transcript_sha256 = source.computed_transcript_sha256();
        let traces = trace_owner_and_time_fields("任务：发送会议纪要；截止：14点30分", &source).unwrap();
        assert_eq!(traces[0].status, SummaryTraceStatus::NeedsReview);
        assert!(traces[0].evidence.is_empty());
        assert_eq!(traces[0].related_evidence.len(), 1);
        assert_eq!(traces[0].related_evidence[0].segment_id, source.segments[0].segment_id);
    }

    #[test]
    fn unrelated_clock_time_in_same_audio_segment_is_not_a_deadline() {
        let mut source = moss_source(TranscriptVersionState::Active);
        source.segments[0].text = "Rayson负责整理报告，今天18点前提交。现在14点30分，会议结束。".into();
        source.transcript_sha256 = source.computed_transcript_sha256();
        let traces = trace_owner_and_time_fields(
            "任务：整理报告；截止时间：14点30分", &source,
        ).unwrap();
        assert_eq!(traces[0].status, SummaryTraceStatus::NeedsReview);
    }

    #[test]
    fn person_in_another_sentence_is_not_the_task_owner() {
        let mut source = moss_source(TranscriptVersionState::Active);
        source.segments[0].text = "Rayson负责整理报告。MeiL是接收人。".into();
        source.transcript_sha256 = source.computed_transcript_sha256();
        let traces = trace_owner_and_time_fields(
            "任务：整理报告；负责人：MeiL", &source,
        ).unwrap();
        assert_eq!(traces[0].status, SummaryTraceStatus::NeedsReview);
    }

    #[test]
    fn unbound_sensevoice_commitment_does_not_identify_a_named_owner() {
        for (text, expected) in [
            ("今天参会有Rayson和MeiL。我负责整理报告，发给Rayson。", SummaryTraceStatus::NeedsReview),
            ("Rayson，我负责整理报告。", SummaryTraceStatus::NeedsReview),
            ("整理报告交给Rayson，周五前提交。", SummaryTraceStatus::Supported),
        ] {
            let mut input = segments();
            input.truncate(1);
            input[0].anonymous_speaker = None;
            input[0].bound_person_id = None;
            input[0].bound_display_name = None;
            input[0].text = text.to_owned();
            let source = TranscriptVersionSnapshot::legacy_local(
                "anonymous_meeting", TranscriptSourceKind::SenseVoice, input,
            );
            let traces = trace_owner_and_time_fields("任务：整理报告；负责人：Rayson", &source).unwrap();
            assert_eq!(traces.len(), 1);
            assert_eq!(traces[0].status, expected, "{text}");
        }
    }

    #[test]
    fn recipient_in_the_task_sentence_is_not_its_owner() {
        let mut source = moss_source(TranscriptVersionState::Active);
        source.segments[0].text = "Rayson负责整理报告并发给MeiL。".into();
        source.transcript_sha256 = source.computed_transcript_sha256();
        let traces = trace_owner_and_time_fields("任务：整理报告；负责人：MeiL", &source).unwrap();
        assert_eq!(traces[0].status, SummaryTraceStatus::NeedsReview);
    }

    #[test]
    fn later_task_is_not_a_prerequisite_of_the_earlier_task() {
        let mut source = moss_source(TranscriptVersionState::Active);
        source.segments[0].text = "先整理报告，再发布版本。".into();
        source.transcript_sha256 = source.computed_transcript_sha256();
        let traces = trace_owner_and_time_fields("任务：整理报告；依赖：发布版本", &source).unwrap();
        assert_eq!(traces[0].status, SummaryTraceStatus::NeedsReview);
    }

    #[test]
    fn multiple_deadlines_in_one_sentence_need_task_review() {
        let mut source = moss_source(TranscriptVersionState::Active);
        source.segments[0].text = "今天18点前整理报告，19点前发邀请。".into();
        source.transcript_sha256 = source.computed_transcript_sha256();
        let traces = trace_owner_and_time_fields("任务：整理报告；截止：19点前", &source).unwrap();
        assert_eq!(traces[0].status, SummaryTraceStatus::NeedsReview);
    }

    #[test]
    fn clock_formatting_does_not_lose_explicit_deadline_evidence() {
        let mut source = moss_source(TranscriptVersionState::Active);
        source.segments[0].text = "Rayson负责整理报告，今天18点前提交。".into();
        source.transcript_sha256 = source.computed_transcript_sha256();
        let traces = trace_owner_and_time_fields(
            "任务：整理报告；截止时间：今天18:00前", &source,
        ).unwrap();
        assert_eq!(traces[0].status, SummaryTraceStatus::Supported);
    }

    #[test]
    fn task_dependency_is_not_silently_skipped() {
        let source = moss_source(TranscriptVersionState::Active);
        let traces = trace_owner_and_time_fields(
            "任务：整理报告；依赖：回归测试通过", &source,
        ).unwrap();
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].value, "回归测试通过");
        assert_eq!(traces[0].status, SummaryTraceStatus::NeedsReview);
    }

    #[test]
    fn inline_owner_and_deadline_are_traced_without_storing_source_excerpt() {
        let source = moss_source(TranscriptVersionState::Active);
        let original = "行动项：整理报告；负责人：Rayson；截止时间：周五";
        let traces = trace_owner_and_time_fields(original, &source).unwrap();
        assert_eq!(traces.len(), 2);
        assert!(traces
            .iter()
            .all(|trace| trace.status == SummaryTraceStatus::Supported));
        let serialized = serde_json::to_string(&traces).unwrap();
        assert!(!serialized.contains("负责整理报告"));
        assert!(serialized.contains("excerptSha256"));

        let restored = restore_supported_owner_and_time_fields(
            original,
            "行动项：整理报告；负责人：会议未提及；截止时间：会议未提及",
            &traces,
        );
        assert_eq!(restored, original);
    }
}
