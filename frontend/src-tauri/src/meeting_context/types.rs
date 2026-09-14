use super::canonical::{profile_sha256, snapshot_sha256};
use super::validation::{
    normalize_and_validate_container, normalize_and_validate_profile,
    normalize_and_validate_snapshot, MeetingContextValidationIssue,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

pub const MEETING_CONTEXT_SCHEMA_VERSION: u8 = 1;

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttendanceStatus {
    Attending,
    Absent,
    Expected,
    Guest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonProfile {
    pub person_id: String,
    pub display_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub department: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermProfile {
    pub term_id: String,
    pub canonical: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingContextProfile {
    pub schema_version: u8,
    #[serde(default)]
    pub fixed_meeting_mechanism: Option<String>,
    #[serde(default)]
    pub people: Vec<PersonProfile>,
    #[serde(default)]
    pub terms: Vec<TermProfile>,
}

impl Default for MeetingContextProfile {
    fn default() -> Self {
        Self {
            schema_version: MEETING_CONTEXT_SCHEMA_VERSION,
            fixed_meeting_mechanism: None,
            people: Vec::new(),
            terms: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingContextSource {
    pub template_id: String,
    pub template_version: u64,
    pub template_file_sha256: String,
    pub profile_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotPerson {
    pub person_id: String,
    pub display_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub department: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    pub attendance: AttendanceStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotTerm {
    pub term_id: String,
    pub canonical: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingContextSnapshot {
    pub context_id: String,
    pub revision: u64,
    pub reason: String,
    pub captured_at: DateTime<Utc>,
    pub source: MeetingContextSource,
    #[serde(default)]
    pub fixed_meeting_mechanism: Option<String>,
    #[serde(default)]
    pub people: Vec<SnapshotPerson>,
    #[serde(default)]
    pub host_person_id: Option<String>,
    #[serde(default)]
    pub terms: Vec<SnapshotTerm>,
    pub context_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingContextContainer {
    pub schema_version: u8,
    pub recording_context_id: String,
    pub current_context_id: String,
    pub contexts: Vec<MeetingContextSnapshot>,
}

impl MeetingContextContainer {
    pub fn from_profile(
        profile: MeetingContextProfile,
        template_id: String,
        template_version: u64,
        template_file_sha256: String,
        captured_at: DateTime<Utc>,
    ) -> Result<Self, Vec<MeetingContextValidationIssue>> {
        let profile = normalize_and_validate_profile(profile)?;
        let source = MeetingContextSource {
            template_id,
            template_version,
            template_file_sha256,
            profile_sha256: profile_sha256(&profile),
        };
        let people = profile
            .people
            .into_iter()
            .filter(|person| person.enabled)
            .map(|person| SnapshotPerson {
                person_id: person.person_id,
                display_name: person.display_name,
                aliases: person.aliases,
                department: person.department,
                role: person.role,
                attendance: AttendanceStatus::Expected,
            })
            .collect();
        let terms = profile
            .terms
            .into_iter()
            .filter(|term| term.enabled)
            .map(|term| SnapshotTerm {
                term_id: term.term_id,
                canonical: term.canonical,
                aliases: term.aliases,
                category: term.category,
            })
            .collect();
        let context_id = format!("ctx_recording_{}", Uuid::new_v4().simple());
        let snapshot = MeetingContextSnapshot {
            context_id: context_id.clone(),
            revision: 1,
            reason: "recording_start".to_owned(),
            captured_at,
            source,
            fixed_meeting_mechanism: profile.fixed_meeting_mechanism,
            people,
            host_person_id: None,
            terms,
            context_sha256: String::new(),
        };
        let mut snapshot = normalize_and_validate_snapshot(snapshot)?;
        snapshot.context_sha256 = snapshot_sha256(&snapshot);
        normalize_and_validate_container(Self {
            schema_version: MEETING_CONTEXT_SCHEMA_VERSION,
            recording_context_id: context_id.clone(),
            current_context_id: context_id,
            contexts: vec![snapshot],
        })
    }

    pub fn recording_context(&self) -> Option<&MeetingContextSnapshot> {
        self.contexts
            .iter()
            .find(|context| context.context_id == self.recording_context_id)
    }

    pub fn current_context(&self) -> Option<&MeetingContextSnapshot> {
        self.contexts
            .iter()
            .find(|context| context.context_id == self.current_context_id)
    }

    pub fn append_revision(
        &mut self,
        mut snapshot: MeetingContextSnapshot,
        reason: impl Into<String>,
        captured_at: DateTime<Utc>,
    ) -> Result<&MeetingContextSnapshot, Vec<MeetingContextValidationIssue>> {
        let mut candidate = normalize_and_validate_container(self.clone())?;
        let next_revision = self
            .contexts
            .iter()
            .map(|context| context.revision)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        snapshot.context_id = format!("ctx_revision_{}", Uuid::new_v4().simple());
        snapshot.revision = next_revision;
        snapshot.reason = reason.into();
        snapshot.captured_at = captured_at;
        snapshot.context_sha256.clear();
        let mut snapshot = normalize_and_validate_snapshot(snapshot)?;
        snapshot.context_sha256 = snapshot_sha256(&snapshot);
        candidate.current_context_id = snapshot.context_id.clone();
        candidate.contexts.push(snapshot);
        candidate = normalize_and_validate_container(candidate)?;
        *self = candidate;
        Ok(self.contexts.last().expect("revision was just appended"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingSummaryTemplatePreference {
    pub schema_version: u8,
    pub mode: String,
    pub template_id: Option<String>,
    pub template_version: Option<u64>,
    pub template_file_sha256: Option<String>,
    pub selected_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordingTemplateSelection {
    pub template_id: String,
    pub template_version: u64,
    pub template_file_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonAttendanceOverride {
    pub person_id: String,
    pub attendance: AttendanceStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordingGuestDraft {
    pub person_id: String,
    pub display_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub department: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordingTermDraft {
    pub term_id: String,
    pub canonical: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordingMeetingContextDraft {
    pub expected_profile_sha256: String,
    #[serde(default)]
    pub attendance: Vec<PersonAttendanceOverride>,
    #[serde(default)]
    pub host_person_id: Option<String>,
    #[serde(default)]
    pub guests: Vec<RecordingGuestDraft>,
    #[serde(default)]
    pub additional_terms: Vec<RecordingTermDraft>,
}

impl MeetingContextContainer {
    pub fn from_profile_with_recording_draft(
        profile: MeetingContextProfile,
        template_id: String,
        template_version: u64,
        template_file_sha256: String,
        captured_at: DateTime<Utc>,
        draft: Option<RecordingMeetingContextDraft>,
    ) -> Result<Self, Vec<MeetingContextValidationIssue>> {
        let mut container = Self::from_profile(
            profile,
            template_id,
            template_version,
            template_file_sha256,
            captured_at,
        )?;
        let Some(draft) = draft else {
            return Ok(container);
        };

        let snapshot = container
            .contexts
            .first_mut()
            .expect("from_profile always creates one context");
        let mut issues = Vec::new();
        let mut overridden = BTreeSet::new();
        for (index, attendance_override) in draft.attendance.into_iter().enumerate() {
            if !overridden.insert(attendance_override.person_id.clone()) {
                issues.push(MeetingContextValidationIssue::new(
                    "DUPLICATE_ATTENDANCE_OVERRIDE",
                    format!("/attendance/{index}/personId"),
                ));
                continue;
            }
            if attendance_override.attendance == AttendanceStatus::Guest {
                issues.push(MeetingContextValidationIssue::new(
                    "INVALID_TEMPLATE_PERSON_ATTENDANCE",
                    format!("/attendance/{index}/attendance"),
                ));
                continue;
            }
            match snapshot
                .people
                .iter_mut()
                .find(|person| person.person_id == attendance_override.person_id)
            {
                Some(person) => person.attendance = attendance_override.attendance,
                None => issues.push(MeetingContextValidationIssue::new(
                    "ATTENDANCE_PERSON_NOT_FOUND",
                    format!("/attendance/{index}/personId"),
                )),
            }
        }

        for guest in draft.guests {
            snapshot.people.push(SnapshotPerson {
                person_id: guest.person_id,
                display_name: guest.display_name,
                aliases: guest.aliases,
                department: guest.department,
                role: guest.role,
                attendance: AttendanceStatus::Guest,
            });
        }
        for term in draft.additional_terms {
            snapshot.terms.push(SnapshotTerm {
                term_id: term.term_id,
                canonical: term.canonical,
                aliases: term.aliases,
                category: term.category,
            });
        }
        snapshot.host_person_id = draft.host_person_id;
        if let Some(host_id) = snapshot.host_person_id.as_deref() {
            if let Some(host) = snapshot
                .people
                .iter_mut()
                .find(|person| person.person_id == host_id)
            {
                if host.attendance == AttendanceStatus::Absent {
                    issues.push(MeetingContextValidationIssue::new(
                        "HOST_MARKED_ABSENT",
                        "/hostPersonId",
                    ));
                } else if host.attendance == AttendanceStatus::Expected {
                    host.attendance = AttendanceStatus::Attending;
                }
            }
        }

        if !issues.is_empty() {
            return Err(issues);
        }
        snapshot.context_sha256.clear();
        let mut normalized = normalize_and_validate_snapshot(snapshot.clone())?;
        normalized.context_sha256 = snapshot_sha256(&normalized);
        *snapshot = normalized;
        normalize_and_validate_container(container)
    }
}
