use super::{
    profile_sha256, MeetingContextContainer, MeetingContextProfile, RecordingMeetingContextDraft,
    RecordingSummaryTemplatePreference, RecordingTemplateSelection,
};
use crate::summary::templates::TemplateService;
use chrono::{DateTime, SecondsFormat, Utc};

const MEETING_CONTEXT_EXTENSION_KEY: &str = "meetily_meeting_context";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRecordingMetadata {
    pub summary_template: Option<RecordingSummaryTemplatePreference>,
    pub meeting_context: Option<MeetingContextContainer>,
}

pub fn resolve_recording_metadata(
    service: &TemplateService,
    selection: Option<RecordingTemplateSelection>,
    context_draft: Option<RecordingMeetingContextDraft>,
    captured_at: DateTime<Utc>,
) -> Result<ResolvedRecordingMetadata, String> {
    let Some(selection) = selection else {
        if context_draft.is_some() {
            return Err("MEETING_CONTEXT_TEMPLATE_SELECTION_REQUIRED".to_owned());
        }
        return Ok(ResolvedRecordingMetadata {
            summary_template: None,
            meeting_context: None,
        });
    };

    let record = service
        .repository()
        .get(&selection.template_id, None)
        .map_err(|error| format!("RECORDING_TEMPLATE_NOT_AVAILABLE: {error}"))?;
    if record.template.version != selection.template_version
        || record.file_sha256 != selection.template_file_sha256
    {
        return Err("RECORDING_TEMPLATE_STALE".to_owned());
    }

    let selected_at = captured_at.to_rfc3339_opts(SecondsFormat::Millis, true);
    let summary_template = RecordingSummaryTemplatePreference {
        schema_version: 1,
        mode: "meeting_override".to_owned(),
        template_id: Some(record.template.id.clone()),
        template_version: Some(record.template.version),
        template_file_sha256: Some(record.file_sha256.clone()),
        selected_at,
    };

    let Some(extension) = record
        .template
        .extensions
        .get(MEETING_CONTEXT_EXTENSION_KEY)
        .cloned()
    else {
        if context_draft.is_some() {
            return Err("RECORDING_TEMPLATE_CONTEXT_MISSING".to_owned());
        }
        return Ok(ResolvedRecordingMetadata {
            summary_template: Some(summary_template),
            meeting_context: None,
        });
    };

    let profile: MeetingContextProfile = serde_json::from_value(extension)
        .map_err(|_| "RECORDING_TEMPLATE_CONTEXT_INVALID".to_owned())?;
    let normalized = super::normalize_and_validate_profile(profile)
        .map_err(|_| "RECORDING_TEMPLATE_CONTEXT_INVALID".to_owned())?;
    let current_profile_sha256 = profile_sha256(&normalized);
    if context_draft
        .as_ref()
        .is_some_and(|draft| draft.expected_profile_sha256 != current_profile_sha256)
    {
        return Err("RECORDING_TEMPLATE_CONTEXT_STALE".to_owned());
    }

    let meeting_context = MeetingContextContainer::from_profile_with_recording_draft(
        normalized,
        record.template.id,
        record.template.version,
        record.file_sha256,
        captured_at,
        context_draft,
    )
    .map_err(|issues| {
        let details = issues
            .into_iter()
            .map(|issue| format!("{}@{}", issue.code, issue.path))
            .collect::<Vec<_>>()
            .join(",");
        format!("RECORDING_MEETING_CONTEXT_INVALID: {details}")
    })?;

    Ok(ResolvedRecordingMetadata {
        summary_template: Some(summary_template),
        meeting_context: Some(meeting_context),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting_context::{AttendanceStatus, PersonAttendanceOverride, RecordingGuestDraft};
    use crate::summary::templates::{CreateConflictPolicy, TemplateOrigin, TemplateRepository};
    use serde_json::json;
    use tempfile::TempDir;

    fn service_with_context() -> (TempDir, TemplateService, RecordingTemplateSelection) {
        let temporary = TempDir::new().unwrap();
        let repository = TemplateRepository::new(temporary.path().join("templates"), None).unwrap();
        let legacy: crate::summary::templates::Template =
            serde_json::from_str(include_str!("../../templates/standard_meeting.json")).unwrap();
        let mut template = crate::summary::templates::migrate_v1_to_v2(
            "recording_context_template",
            &legacy,
            Utc::now().fixed_offset(),
        )
        .unwrap();
        template.extensions.insert(
            MEETING_CONTEXT_EXTENSION_KEY.to_owned(),
            json!({
                "schema_version": 1,
                "people": [{
                    "person_id": "person_mico",
                    "display_name": "Mico",
                    "aliases": ["米可"]
                }],
                "terms": [{
                    "term_id": "term_m100",
                    "canonical": "M100",
                    "aliases": ["M一百"]
                }]
            }),
        );
        let record = repository
            .create(template, CreateConflictPolicy::Error)
            .unwrap();
        let selection = RecordingTemplateSelection {
            template_id: record.template.id.clone(),
            template_version: record.template.version,
            template_file_sha256: record.file_sha256.clone(),
        };
        (temporary, TemplateService::new(repository), selection)
    }

    fn profile_hash(service: &TemplateService, selection: &RecordingTemplateSelection) -> String {
        let record = service
            .repository()
            .get(&selection.template_id, Some(TemplateOrigin::Custom))
            .unwrap();
        let profile: MeetingContextProfile = serde_json::from_value(
            record.template.extensions[MEETING_CONTEXT_EXTENSION_KEY].clone(),
        )
        .unwrap();
        profile_sha256(&super::super::normalize_and_validate_profile(profile).unwrap())
    }

    #[test]
    fn resolves_authoritative_template_and_applies_only_structured_adjustments() {
        let (_temporary, service, selection) = service_with_context();
        let draft = RecordingMeetingContextDraft {
            expected_profile_sha256: profile_hash(&service, &selection),
            attendance: vec![PersonAttendanceOverride {
                person_id: "person_mico".to_owned(),
                attendance: AttendanceStatus::Attending,
            }],
            host_person_id: Some("person_mico".to_owned()),
            guests: vec![RecordingGuestDraft {
                person_id: "guest_meil".to_owned(),
                display_name: "MeiL".to_owned(),
                aliases: vec!["梅尔".to_owned()],
                department: None,
                role: Some("主持人".to_owned()),
            }],
            additional_terms: Vec::new(),
        };
        let resolved =
            resolve_recording_metadata(&service, Some(selection), Some(draft), Utc::now()).unwrap();
        let context = resolved.meeting_context.unwrap();
        let snapshot = context.recording_context().unwrap();
        assert_eq!(snapshot.people.len(), 2);
        assert_eq!(snapshot.host_person_id.as_deref(), Some("person_mico"));
        assert_eq!(snapshot.people[0].attendance, AttendanceStatus::Attending);
        assert_eq!(snapshot.people[1].attendance, AttendanceStatus::Guest);
        assert_eq!(snapshot.context_sha256.len(), 64);
        assert!(resolved.summary_template.is_some());
    }

    #[test]
    fn rejects_stale_template_and_stale_profile() {
        let (_temporary, service, selection) = service_with_context();
        let mut stale_selection = selection.clone();
        stale_selection.template_file_sha256 = "0".repeat(64);
        assert_eq!(
            resolve_recording_metadata(&service, Some(stale_selection), None, Utc::now())
                .unwrap_err(),
            "RECORDING_TEMPLATE_STALE"
        );

        let stale_draft = RecordingMeetingContextDraft {
            expected_profile_sha256: "0".repeat(64),
            attendance: Vec::new(),
            host_person_id: None,
            guests: Vec::new(),
            additional_terms: Vec::new(),
        };
        assert_eq!(
            resolve_recording_metadata(&service, Some(selection), Some(stale_draft), Utc::now())
                .unwrap_err(),
            "RECORDING_TEMPLATE_CONTEXT_STALE"
        );
    }
}
