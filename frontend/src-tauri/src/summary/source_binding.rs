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
pub enum SummaryTraceField {
    Owner,
    Time,
    Dependency,
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
    /// Nearby task evidence is a review aid, never proof of the field value.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_evidence: Vec<SummaryEvidenceReference>,
}

/// Finds exact, reviewable evidence for owner and time fields. This is not a
/// semantic-entailment claim: unsupported values are explicitly marked for
/// review, while supported values carry stable segment/timestamp references.
pub fn trace_owner_and_time_fields(
    markdown: &str,
    source: &TranscriptVersionSnapshot,
) -> Result<Vec<SummaryFieldTrace>, SummarySourceBindingError> {
    source.validate_active()?;
    let mut traces = Vec::new();
    let mut table_schema: Option<TableTraceSchema> = None;

    for (line_index, line) in markdown.lines().enumerate() {
        let markdown_line = line_index + 1;
        if line.trim_start().starts_with('|') && line.trim_end().ends_with('|') {
            let cells = markdown_table_cells(line);
            if cells.is_empty() {
                table_schema = None;
                continue;
            }
            if table_schema.is_none() {
                let fields = cells
                    .iter()
                    .enumerate()
                    .filter_map(|(index, header)| {
                        trace_field_for_header(header).map(|field| (index, field))
                    })
                    .collect::<Vec<_>>();
                let anchors = cells
                    .iter()
                    .enumerate()
                    .filter_map(|(index, header)| is_action_anchor_header(header).then_some(index))
                    .collect::<Vec<_>>();
                table_schema = (!fields.is_empty()).then_some(TableTraceSchema {
                    fields,
                    action_anchor_columns: anchors,
                });
                continue;
            }
            if markdown_table_separator(&cells) {
                continue;
            }
            if let Some(schema) = table_schema.as_ref() {
                let action_anchors = schema
                    .action_anchor_columns
                    .iter()
                    .filter_map(|column| cells.get(*column))
                    .filter(|value| !is_review_placeholder(value))
                    .cloned()
                    .collect::<Vec<_>>();
                for (column, field) in &schema.fields {
                    if let Some(value) = cells.get(*column) {
                        if !is_review_placeholder(value) {
                            traces.push(trace_field_value(
                                *field,
                                value,
                                markdown_line,
                                Some(*column),
                                &action_anchors,
                                source,
                            ));
                        }
                    }
                }
            }
            continue;
        }

        table_schema = None;
        let action_anchors = inline_action_anchors(line);
        for (field, labels) in [
            (SummaryTraceField::Owner, OWNER_INLINE_LABELS),
            (SummaryTraceField::Time, TIME_INLINE_LABELS),
            (SummaryTraceField::Dependency, DEPENDENCY_INLINE_LABELS),
        ] {
            for value in inline_field_values(line, labels) {
                if !is_review_placeholder(value) {
                    traces.push(trace_field_value(
                        field,
                        value,
                        markdown_line,
                        None,
                        &action_anchors,
                        source,
                    ));
                }
            }
        }
    }
    Ok(traces)
}

/// Restores only evidence-supported owner/time fields after the existing
/// safety sanitizer has replaced high-risk values with placeholders.
/// Unsupported fields remain masked and carry `needs_review` traces.
pub fn restore_supported_owner_and_time_fields(
    evidence_normalized_markdown: &str,
    sanitized_markdown: &str,
    traces: &[SummaryFieldTrace],
) -> String {
    let original = evidence_normalized_markdown.lines().collect::<Vec<_>>();
    let mut sanitized = sanitized_markdown
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for trace in traces
        .iter()
        .filter(|trace| trace.status == SummaryTraceStatus::Supported)
    {
        let line_index = trace.markdown_line.saturating_sub(1);
        let (Some(original_line), Some(sanitized_line)) =
            (original.get(line_index), sanitized.get_mut(line_index))
        else {
            continue;
        };
        if let Some(column) = trace.markdown_column {
            let original_cells = markdown_table_cells(original_line);
            let mut sanitized_cells = markdown_table_cells(sanitized_line);
            let (Some(original_cell), Some(sanitized_cell)) =
                (original_cells.get(column), sanitized_cells.get_mut(column))
            else {
                continue;
            };
            *sanitized_cell = original_cell.clone();
            *sanitized_line = format!("| {} |", sanitized_cells.join(" | "));
        } else {
            *sanitized_line =
                restore_first_inline_placeholder(sanitized_line, trace.field, trace.value.as_str());
        }
    }
    let mut result = sanitized.join("\n");
    if sanitized_markdown.ends_with('\n') {
        result.push('\n');
    }
    result
}

fn trace_field_value(
    field: SummaryTraceField,
    value: &str,
    markdown_line: usize,
    markdown_column: Option<usize>,
    action_anchors: &[String],
    source: &TranscriptVersionSnapshot,
) -> SummaryFieldTrace {
    let normalized_value = normalize_evidence(value);
    let normalized_anchors = action_anchors
        .iter()
        .map(|anchor| normalize_evidence(anchor))
        .filter(|anchor| anchor.chars().count() >= 2)
        .collect::<Vec<_>>();
    let evidence = source
        .segments
        .iter()
        .filter(|segment| {
            // An audio segment can contain several unrelated tasks and clock times.
            // Require the task and value in the same sentence, not just the same segment.
            evidence_sentences(&segment.text).iter().any(|sentence| {
                let normalized_text = normalize_evidence(sentence);
                let action_matches = normalized_anchors.iter().any(|anchor| normalized_text.contains(anchor));
                let text_matches = !normalized_value.is_empty() && normalized_text.contains(&normalized_value);
                let speaker_matches = field == SummaryTraceField::Owner
                    && segment.effective_speaker()
                        .is_some_and(|speaker| normalize_evidence(speaker) == normalized_value)
                    && contains_first_person_commitment(sentence);
                if !action_matches { return false; }
                match field {
                    SummaryTraceField::Owner => speaker_matches || (text_matches
                        && explicit_owner_statement(sentence, value)),
                    SummaryTraceField::Time => text_matches && !has_multiple_deadlines(&normalized_text),
                    SummaryTraceField::Dependency => text_matches && normalized_anchors.iter().any(|task| {
                        ["依赖", "的前提是", "需要先", "dependson", "requires"]
                            .iter().any(|relation| normalized_text.contains(&format!("{task}{relation}{normalized_value}")))
                    }),
                }
            })
        })
        .map(|segment| SummaryEvidenceReference {
            segment_id: segment.segment_id.clone(),
            start_ms: segment.start_ms,
            end_ms: segment.end_ms,
            excerpt_sha256: sha256_text(segment.text.trim()),
        })
        .collect::<Vec<_>>();
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
        value: value.trim().to_owned(),
        markdown_line,
        markdown_column,
        status: if evidence.is_empty() {
            SummaryTraceStatus::NeedsReview
        } else {
            SummaryTraceStatus::Supported
        },
        evidence,
        task: action_anchors.join(" / "),
        related_evidence,
    }
}

const OWNER_INLINE_LABELS: &[&str] = &["负责人", "责任人", "owner", "assignee"];
const TIME_INLINE_LABELS: &[&str] = &["截止时间", "截止日期", "完成时间", "截止", "deadline", "due date"];
const DEPENDENCY_INLINE_LABELS: &[&str] = &["依赖", "前提条件", "dependency", "prerequisite"];
const ACTION_INLINE_LABELS: &[&str] =
    &["行动项", "行动任务", "任务", "事项", "action item", "task"];

struct TableTraceSchema {
    fields: Vec<(usize, SummaryTraceField)>,
    action_anchor_columns: Vec<usize>,
}

fn trace_field_for_header(header: &str) -> Option<SummaryTraceField> {
    let normalized = header.trim().trim_matches('*').trim().to_ascii_lowercase();
    match normalized.as_str() {
        "负责人" | "负责人/部门" | "负责人／部门" | "责任人" | "owner" | "owner/department"
        | "owner / department" | "assignee" => Some(SummaryTraceField::Owner),
        "时间" | "截止时间" | "截止日期" | "完成时间" | "截止" | "deadline" | "due date" | "time" => {
            Some(SummaryTraceField::Time)
        }
        "依赖" | "前提条件" | "dependency" | "prerequisite" => Some(SummaryTraceField::Dependency),
        _ => None,
    }
}

fn is_action_anchor_header(header: &str) -> bool {
    matches!(
        header
            .trim()
            .trim_matches('*')
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "行动项"
            | "行动任务"
            | "任务"
            | "事项"
            | "决策"
            | "结论"
            | "action item"
            | "action"
            | "task"
            | "decision"
    )
}

fn inline_action_anchors(line: &str) -> Vec<String> {
    inline_field_values(line, ACTION_INLINE_LABELS)
        .into_iter()
        .map(str::to_owned)
        .collect()
}

fn contains_first_person_commitment(text: &str) -> bool {
    let normalized = text.to_ascii_lowercase();
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

fn restore_first_inline_placeholder(
    line: &str,
    field: SummaryTraceField,
    original_value: &str,
) -> String {
    let labels = match field {
        SummaryTraceField::Owner => OWNER_INLINE_LABELS,
        SummaryTraceField::Time => TIME_INLINE_LABELS,
        SummaryTraceField::Dependency => DEPENDENCY_INLINE_LABELS,
    };
    for (start, end) in inline_field_ranges(line, labels) {
        if is_review_placeholder(&line[start..end]) {
            let mut restored = String::with_capacity(line.len() + original_value.len());
            restored.push_str(&line[..start]);
            restored.push_str(original_value);
            restored.push_str(&line[end..]);
            return restored;
        }
    }
    line.to_owned()
}

fn inline_field_values<'a>(line: &'a str, labels: &[&str]) -> Vec<&'a str> {
    inline_field_ranges(line, labels)
        .into_iter()
        .map(|(start, end)| line[start..end].trim().trim_matches('*').trim())
        .collect()
}

fn inline_field_ranges(line: &str, labels: &[&str]) -> Vec<(usize, usize)> {
    let lowercase = line.to_ascii_lowercase();
    let mut ranges = Vec::new();
    for label in labels {
        let needle = label.to_ascii_lowercase();
        let mut offset = 0;
        while let Some(relative) = lowercase[offset..].find(&needle) {
            let start = offset + relative + needle.len();
            let remainder = &line[start..];
            let Some(colon_offset) = remainder.find([':', '：']) else {
                break;
            };
            if !remainder[..colon_offset]
                .chars()
                .all(|character| character.is_whitespace() || character == '*')
            {
                offset = start;
                continue;
            }
            let value_start = start
                + colon_offset
                + remainder[colon_offset..]
                    .chars()
                    .next()
                    .map(char::len_utf8)
                    .unwrap_or(1);
            let value_remainder = &line[value_start..];
            let value_end = value_remainder
                .find([';', '；', '|'])
                .unwrap_or(value_remainder.len());
            let raw_value = &value_remainder[..value_end];
            let leading = raw_value.len() - raw_value.trim_start().len();
            let trailing = raw_value.trim_end().len();
            let range_start = value_start + leading;
            let range_end = value_start + trailing;
            if range_start < range_end {
                ranges.push((range_start, range_end));
            }
            offset = value_start + value_end;
            if offset >= line.len() {
                break;
            }
        }
    }
    ranges.sort_unstable();
    ranges.dedup();
    ranges
}

fn markdown_table_cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().trim_matches('*').trim().to_owned())
        .collect()
}

fn markdown_table_separator(cells: &[String]) -> bool {
    !cells.is_empty()
        && cells.iter().all(|cell| {
            cell.chars()
                .all(|character| matches!(character, ':' | '-' | ' '))
        })
}

fn is_review_placeholder(value: &str) -> bool {
    matches!(
        normalize_evidence(value).trim_matches(['。', '，', '；', '：']),
        "会议未提及"
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

fn normalize_evidence(value: &str) -> String {
    static CLOCK: Lazy<Regex> = Lazy::new(|| Regex::new(
        r"(?P<h>\d{1,2})(?:(?:[:：](?P<m>\d{2}))|(?:[点时](?:(?P<cm>\d{1,2})分?)?))"
    ).unwrap());
    let width_normalized: String = value.nfkc().collect();
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
    let mut sentences = Vec::new();
    let mut start = 0;
    for (offset, ch) in text.char_indices() {
        let end = offset + ch.len_utf8();
        let separator = matches!(ch, '。' | '！' | '？' | '\n' | '；' | ';')
            || (matches!(ch, '.' | '!' | '?') &&
                text[end..].chars().next().map_or(true, char::is_whitespace));
        if separator {
            sentences.push(&text[start..end]);
            start = end;
        }
    }
    if start < text.len() { sentences.push(&text[start..]); }
    sentences
}

fn explicit_owner_statement(sentence: &str, owner: &str) -> bool {
    let text = normalize_evidence(sentence);
    let name = normalize_evidence(owner);
    if ["不负责", "不是负责人", "notresponsible"].iter().any(|negative| text.contains(negative)) {
        return false;
    }
    let assigned = ["负责", "承担", "will", "isresponsible", "owns"].iter()
        .any(|role| text.contains(&format!("{name}{role}")));
    let named_speaker = sentence.trim_start().starts_with(&format!("{}：", owner.trim()))
        || sentence.trim_start().starts_with(&format!("{}:", owner.trim()));
    assigned || (named_speaker && contains_first_person_commitment(sentence))
        || text.contains(&format!("交给{name}"))
        || text.contains(&format!("assignedto{name}"))
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
    use chrono::TimeZone;

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 29, 4, 0, 0).unwrap()
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
        assert!(restored.contains("| 发布版本 | 会议未提及 | 会议未提及 |"));
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
