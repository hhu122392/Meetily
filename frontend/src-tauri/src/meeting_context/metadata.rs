use super::{normalize_and_validate_container, MeetingContextContainer, RecognitionContext};
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
struct MeetingContextMetadata {
    #[serde(default)]
    meeting_context: Option<MeetingContextContainer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecognitionContextSelection {
    Recording,
    Current,
}

pub fn load_recognition_context(
    meeting_folder: &Path,
    selection: RecognitionContextSelection,
) -> Result<Option<RecognitionContext>, String> {
    let metadata_path = meeting_folder.join("metadata.json");
    if !metadata_path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&metadata_path)
        .map_err(|error| format!("MEETING_CONTEXT_METADATA_READ_FAILED: {error}"))?;
    let metadata: MeetingContextMetadata = serde_json::from_str(&raw)
        .map_err(|error| format!("MEETING_CONTEXT_METADATA_INVALID: {error}"))?;
    let Some(container) = metadata.meeting_context else {
        return Ok(None);
    };
    let container = normalize_and_validate_container(container)
        .map_err(|_| "MEETING_CONTEXT_METADATA_CONTEXT_INVALID".to_owned())?;
    let snapshot = match selection {
        RecognitionContextSelection::Recording => container.recording_context(),
        RecognitionContextSelection::Current => container.current_context(),
    }
    .ok_or_else(|| "MEETING_CONTEXT_METADATA_REFERENCE_MISSING".to_owned())?;
    Ok(Some(RecognitionContext::from_snapshot(snapshot)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting_context::{MeetingContextContainer, MeetingContextProfile, PersonProfile};
    use chrono::Utc;
    use tempfile::tempdir;

    #[test]
    fn loads_recording_and_current_context_from_metadata() {
        let directory = tempdir().unwrap();
        let mut container = MeetingContextContainer::from_profile(
            MeetingContextProfile {
                schema_version: 1,
                fixed_meeting_mechanism: None,
                people: vec![PersonProfile {
                    person_id: "person_amu".to_owned(),
                    display_name: "Amu".to_owned(),
                    aliases: vec!["阿牧".to_owned()],
                    department: None,
                    role: None,
                    enabled: true,
                }],
                terms: vec![],
            },
            "template".to_owned(),
            1,
            "a".repeat(64),
            Utc::now(),
        )
        .unwrap();
        let mut revision = container.recording_context().unwrap().clone();
        revision.people[0].aliases.push("A Mu".to_owned());
        container
            .append_revision(revision, "manual_update", Utc::now())
            .unwrap();
        std::fs::write(
            directory.path().join("metadata.json"),
            serde_json::to_vec(&serde_json::json!({ "meeting_context": container })).unwrap(),
        )
        .unwrap();

        let recording =
            load_recognition_context(directory.path(), RecognitionContextSelection::Recording)
                .unwrap()
                .unwrap();
        let current =
            load_recognition_context(directory.path(), RecognitionContextSelection::Current)
                .unwrap()
                .unwrap();
        assert_ne!(recording.context_id, current.context_id);
        assert_eq!(
            recording.explicit_alias_map.get("阿牧"),
            Some(&"Amu".to_owned())
        );
        assert_eq!(
            current.explicit_alias_map.get("A Mu"),
            Some(&"Amu".to_owned())
        );
    }

    #[test]
    fn old_metadata_without_context_returns_none() {
        let directory = tempdir().unwrap();
        std::fs::write(
            directory.path().join("metadata.json"),
            br#"{"version":"1.0"}"#,
        )
        .unwrap();
        assert!(
            load_recognition_context(directory.path(), RecognitionContextSelection::Current)
                .unwrap()
                .is_none()
        );
    }
}
