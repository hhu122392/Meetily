pub mod canonical;
pub mod metadata;
pub mod recognition;
pub mod recording;
pub mod summary;
pub mod types;
pub mod validation;

pub use canonical::{canonical_json_sha256, profile_sha256, snapshot_sha256};
pub use metadata::{load_recognition_context, RecognitionContextSelection};
pub use recognition::{RecognitionContext, RecognitionContextDiagnostics};
pub use recording::{resolve_recording_metadata, ResolvedRecordingMetadata};
pub use summary::{
    build_summary_meeting_context, load_summary_meeting_context,
    sanitize_generated_summary_without_meeting_context, sanitize_generated_summary_with_transcript,
    validate_summary_markdown,
    validate_summary_markdown_with_source, validate_summary_markdown_with_transcript,
    RecognitionDictionary, RecognitionDictionaryPerson, RecognitionDictionaryTerm,
    SummaryFactValidation, SummaryFactValidationStatus, SummaryFactWarning, SummaryMeetingContext,
    ValidatedSummaryMarkdown, VerifiedMeetingFacts, VerifiedMeetingPerson,
};
pub use types::{
    AttendanceStatus, MeetingContextContainer, MeetingContextProfile, MeetingContextSnapshot,
    MeetingContextSource, PersonAttendanceOverride, PersonProfile, RecordingGuestDraft,
    RecordingMeetingContextDraft, RecordingSummaryTemplatePreference, RecordingTemplateSelection,
    RecordingTermDraft, SnapshotPerson, SnapshotTerm, TermProfile, MEETING_CONTEXT_SCHEMA_VERSION,
};
pub use validation::{
    normalize_and_validate_container, normalize_and_validate_profile,
    normalize_and_validate_snapshot, MeetingContextValidationIssue,
};
