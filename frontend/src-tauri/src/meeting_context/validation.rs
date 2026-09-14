use super::canonical::snapshot_sha256;
use super::types::{
    MeetingContextContainer, MeetingContextProfile, MeetingContextSnapshot, PersonProfile,
    SnapshotPerson, SnapshotTerm, TermProfile, MEETING_CONTEXT_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use unicode_normalization::UnicodeNormalization;

const MAX_PEOPLE: usize = 100;
const MAX_TERMS: usize = 200;
const MAX_ALIASES: usize = 20;
const MAX_NAME_CHARS: usize = 80;
const MAX_OPTIONAL_TEXT_CHARS: usize = 120;
const MAX_MECHANISM_CHARS: usize = 1_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingContextValidationIssue {
    pub code: String,
    pub path: String,
}

impl MeetingContextValidationIssue {
    pub(super) fn new(code: &str, path: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            path: path.into(),
        }
    }
}

pub(crate) fn comparison_key(value: &str) -> String {
    value
        .trim()
        .nfkc()
        .map(|character| {
            if character.is_ascii() {
                character.to_ascii_lowercase()
            } else {
                character
            }
        })
        .collect()
}

fn contains_forbidden_character(value: &str) -> bool {
    value.chars().any(|character| {
        character.is_control()
            || matches!(
                character,
                '\u{202A}'
                    ..='\u{202E}' | '\u{2066}'
                    ..='\u{2069}' | '\u{200E}' | '\u{200F}'
            )
    })
}

fn normalize_required(
    value: &mut String,
    path: &str,
    max_chars: usize,
    issues: &mut Vec<MeetingContextValidationIssue>,
) {
    *value = value.trim().to_owned();
    if value.is_empty() {
        issues.push(MeetingContextValidationIssue::new("EMPTY_VALUE", path));
    } else if value.chars().count() > max_chars {
        issues.push(MeetingContextValidationIssue::new("VALUE_TOO_LONG", path));
    }
    if contains_forbidden_character(value) {
        issues.push(MeetingContextValidationIssue::new(
            "FORBIDDEN_CHARACTER",
            path,
        ));
    }
}

fn normalize_optional(
    value: &mut Option<String>,
    path: &str,
    max_chars: usize,
    issues: &mut Vec<MeetingContextValidationIssue>,
) {
    let Some(current) = value else {
        return;
    };
    *current = current.trim().to_owned();
    if current.is_empty() {
        *value = None;
        return;
    }
    if current.chars().count() > max_chars {
        issues.push(MeetingContextValidationIssue::new("VALUE_TOO_LONG", path));
    }
    if contains_forbidden_character(current) {
        issues.push(MeetingContextValidationIssue::new(
            "FORBIDDEN_CHARACTER",
            path,
        ));
    }
}

fn normalize_sha256(
    value: &mut String,
    path: &str,
    issues: &mut Vec<MeetingContextValidationIssue>,
) {
    normalize_required(value, path, 64, issues);
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        issues.push(MeetingContextValidationIssue::new("INVALID_SHA256", path));
    }
}

fn normalize_aliases(
    aliases: &mut Vec<String>,
    path: &str,
    issues: &mut Vec<MeetingContextValidationIssue>,
) {
    if aliases.len() > MAX_ALIASES {
        issues.push(MeetingContextValidationIssue::new("TOO_MANY_ALIASES", path));
    }
    let mut seen = BTreeSet::new();
    for (index, alias) in aliases.iter_mut().enumerate() {
        let alias_path = format!("{path}/{index}");
        normalize_required(alias, &alias_path, MAX_NAME_CHARS, issues);
        let key = comparison_key(alias);
        if !key.is_empty() && !seen.insert(key) {
            issues.push(MeetingContextValidationIssue::new(
                "DUPLICATE_ALIAS",
                alias_path,
            ));
        }
    }
}

fn normalize_person_profile(
    person: &mut PersonProfile,
    path: &str,
    issues: &mut Vec<MeetingContextValidationIssue>,
) {
    normalize_required(
        &mut person.person_id,
        &format!("{path}/person_id"),
        MAX_NAME_CHARS,
        issues,
    );
    normalize_required(
        &mut person.display_name,
        &format!("{path}/display_name"),
        MAX_NAME_CHARS,
        issues,
    );
    normalize_aliases(&mut person.aliases, &format!("{path}/aliases"), issues);
    normalize_optional(
        &mut person.department,
        &format!("{path}/department"),
        MAX_OPTIONAL_TEXT_CHARS,
        issues,
    );
    normalize_optional(
        &mut person.role,
        &format!("{path}/role"),
        MAX_OPTIONAL_TEXT_CHARS,
        issues,
    );
}

fn normalize_term_profile(
    term: &mut TermProfile,
    path: &str,
    issues: &mut Vec<MeetingContextValidationIssue>,
) {
    normalize_required(
        &mut term.term_id,
        &format!("{path}/term_id"),
        MAX_NAME_CHARS,
        issues,
    );
    normalize_required(
        &mut term.canonical,
        &format!("{path}/canonical"),
        MAX_NAME_CHARS,
        issues,
    );
    normalize_aliases(&mut term.aliases, &format!("{path}/aliases"), issues);
    normalize_optional(
        &mut term.category,
        &format!("{path}/category"),
        MAX_OPTIONAL_TEXT_CHARS,
        issues,
    );
}

fn validate_person_uniqueness(
    people: &[PersonProfile],
    issues: &mut Vec<MeetingContextValidationIssue>,
) {
    let mut ids = BTreeMap::new();
    let mut names = BTreeMap::new();
    let mut active_names = BTreeMap::new();
    for (index, person) in people.iter().enumerate() {
        let id_key = comparison_key(&person.person_id);
        if ids.insert(id_key, index).is_some() {
            issues.push(MeetingContextValidationIssue::new(
                "DUPLICATE_PERSON_ID",
                format!("/people/{index}/person_id"),
            ));
        }
        let name_key = comparison_key(&person.display_name);
        if names.insert(name_key.clone(), index).is_some() {
            issues.push(MeetingContextValidationIssue::new(
                "DUPLICATE_PERSON_NAME",
                format!("/people/{index}/display_name"),
            ));
        }
        if person.enabled {
            active_names.insert(name_key, index);
        }
    }

    let mut active_aliases: BTreeMap<String, usize> = BTreeMap::new();
    for (index, person) in people
        .iter()
        .enumerate()
        .filter(|(_, person)| person.enabled)
    {
        for (alias_index, alias) in person.aliases.iter().enumerate() {
            let key = comparison_key(alias);
            if let Some(owner) = active_aliases.insert(key.clone(), index) {
                if owner != index {
                    issues.push(MeetingContextValidationIssue::new(
                        "AMBIGUOUS_PERSON_ALIAS",
                        format!("/people/{index}/aliases/{alias_index}"),
                    ));
                }
            }
            if let Some(owner) = active_names.get(&key) {
                if *owner != index {
                    issues.push(MeetingContextValidationIssue::new(
                        "ALIAS_MATCHES_PERSON_NAME",
                        format!("/people/{index}/aliases/{alias_index}"),
                    ));
                }
            }
        }
    }
}

fn validate_term_uniqueness(
    terms: &[TermProfile],
    issues: &mut Vec<MeetingContextValidationIssue>,
) {
    let mut ids = BTreeMap::new();
    let mut names = BTreeMap::new();
    let mut active_names = BTreeMap::new();
    for (index, term) in terms.iter().enumerate() {
        let id_key = comparison_key(&term.term_id);
        if ids.insert(id_key, index).is_some() {
            issues.push(MeetingContextValidationIssue::new(
                "DUPLICATE_TERM_ID",
                format!("/terms/{index}/term_id"),
            ));
        }
        let name_key = comparison_key(&term.canonical);
        if names.insert(name_key.clone(), index).is_some() {
            issues.push(MeetingContextValidationIssue::new(
                "DUPLICATE_TERM_NAME",
                format!("/terms/{index}/canonical"),
            ));
        }
        if term.enabled {
            active_names.insert(name_key, index);
        }
    }

    let mut active_aliases: BTreeMap<String, usize> = BTreeMap::new();
    for (index, term) in terms.iter().enumerate().filter(|(_, term)| term.enabled) {
        for (alias_index, alias) in term.aliases.iter().enumerate() {
            let key = comparison_key(alias);
            if let Some(owner) = active_aliases.insert(key.clone(), index) {
                if owner != index {
                    issues.push(MeetingContextValidationIssue::new(
                        "AMBIGUOUS_TERM_ALIAS",
                        format!("/terms/{index}/aliases/{alias_index}"),
                    ));
                }
            }
            if let Some(owner) = active_names.get(&key) {
                if *owner != index {
                    issues.push(MeetingContextValidationIssue::new(
                        "ALIAS_MATCHES_TERM_NAME",
                        format!("/terms/{index}/aliases/{alias_index}"),
                    ));
                }
            }
        }
    }
}

pub fn normalize_and_validate_profile(
    mut profile: MeetingContextProfile,
) -> Result<MeetingContextProfile, Vec<MeetingContextValidationIssue>> {
    let mut issues = Vec::new();
    if profile.schema_version != MEETING_CONTEXT_SCHEMA_VERSION {
        issues.push(MeetingContextValidationIssue::new(
            "UNSUPPORTED_SCHEMA_VERSION",
            "/schema_version",
        ));
    }
    if profile.people.len() > MAX_PEOPLE {
        issues.push(MeetingContextValidationIssue::new(
            "TOO_MANY_PEOPLE",
            "/people",
        ));
    }
    if profile.terms.len() > MAX_TERMS {
        issues.push(MeetingContextValidationIssue::new(
            "TOO_MANY_TERMS",
            "/terms",
        ));
    }
    normalize_optional(
        &mut profile.fixed_meeting_mechanism,
        "/fixed_meeting_mechanism",
        MAX_MECHANISM_CHARS,
        &mut issues,
    );
    for (index, person) in profile.people.iter_mut().enumerate() {
        normalize_person_profile(person, &format!("/people/{index}"), &mut issues);
    }
    for (index, term) in profile.terms.iter_mut().enumerate() {
        normalize_term_profile(term, &format!("/terms/{index}"), &mut issues);
    }
    validate_person_uniqueness(&profile.people, &mut issues);
    validate_term_uniqueness(&profile.terms, &mut issues);
    if issues.is_empty() {
        Ok(profile)
    } else {
        Err(issues)
    }
}

fn normalize_snapshot_person(
    person: &mut SnapshotPerson,
    path: &str,
    issues: &mut Vec<MeetingContextValidationIssue>,
) {
    normalize_required(
        &mut person.person_id,
        &format!("{path}/person_id"),
        MAX_NAME_CHARS,
        issues,
    );
    normalize_required(
        &mut person.display_name,
        &format!("{path}/display_name"),
        MAX_NAME_CHARS,
        issues,
    );
    normalize_aliases(&mut person.aliases, &format!("{path}/aliases"), issues);
    normalize_optional(
        &mut person.department,
        &format!("{path}/department"),
        MAX_OPTIONAL_TEXT_CHARS,
        issues,
    );
    normalize_optional(
        &mut person.role,
        &format!("{path}/role"),
        MAX_OPTIONAL_TEXT_CHARS,
        issues,
    );
}

fn normalize_snapshot_term(
    term: &mut SnapshotTerm,
    path: &str,
    issues: &mut Vec<MeetingContextValidationIssue>,
) {
    normalize_required(
        &mut term.term_id,
        &format!("{path}/term_id"),
        MAX_NAME_CHARS,
        issues,
    );
    normalize_required(
        &mut term.canonical,
        &format!("{path}/canonical"),
        MAX_NAME_CHARS,
        issues,
    );
    normalize_aliases(&mut term.aliases, &format!("{path}/aliases"), issues);
    normalize_optional(
        &mut term.category,
        &format!("{path}/category"),
        MAX_OPTIONAL_TEXT_CHARS,
        issues,
    );
}

pub fn normalize_and_validate_snapshot(
    mut snapshot: MeetingContextSnapshot,
) -> Result<MeetingContextSnapshot, Vec<MeetingContextValidationIssue>> {
    let mut issues = Vec::new();
    normalize_required(
        &mut snapshot.context_id,
        "/context_id",
        MAX_NAME_CHARS,
        &mut issues,
    );
    normalize_required(
        &mut snapshot.reason,
        "/reason",
        MAX_OPTIONAL_TEXT_CHARS,
        &mut issues,
    );
    normalize_required(
        &mut snapshot.source.template_id,
        "/source/template_id",
        MAX_NAME_CHARS,
        &mut issues,
    );
    normalize_sha256(
        &mut snapshot.source.template_file_sha256,
        "/source/template_file_sha256",
        &mut issues,
    );
    normalize_sha256(
        &mut snapshot.source.profile_sha256,
        "/source/profile_sha256",
        &mut issues,
    );
    if snapshot.revision == 0 {
        issues.push(MeetingContextValidationIssue::new(
            "INVALID_REVISION",
            "/revision",
        ));
    }
    if snapshot.people.len() > MAX_PEOPLE {
        issues.push(MeetingContextValidationIssue::new(
            "TOO_MANY_PEOPLE",
            "/people",
        ));
    }
    if snapshot.terms.len() > MAX_TERMS {
        issues.push(MeetingContextValidationIssue::new(
            "TOO_MANY_TERMS",
            "/terms",
        ));
    }
    normalize_optional(
        &mut snapshot.fixed_meeting_mechanism,
        "/fixed_meeting_mechanism",
        MAX_MECHANISM_CHARS,
        &mut issues,
    );
    normalize_optional(
        &mut snapshot.host_person_id,
        "/host_person_id",
        MAX_NAME_CHARS,
        &mut issues,
    );
    for (index, person) in snapshot.people.iter_mut().enumerate() {
        normalize_snapshot_person(person, &format!("/people/{index}"), &mut issues);
    }
    for (index, term) in snapshot.terms.iter_mut().enumerate() {
        normalize_snapshot_term(term, &format!("/terms/{index}"), &mut issues);
    }

    let profile_projection = MeetingContextProfile {
        schema_version: MEETING_CONTEXT_SCHEMA_VERSION,
        fixed_meeting_mechanism: snapshot.fixed_meeting_mechanism.clone(),
        people: snapshot
            .people
            .iter()
            .map(|person| PersonProfile {
                person_id: person.person_id.clone(),
                display_name: person.display_name.clone(),
                aliases: person.aliases.clone(),
                department: person.department.clone(),
                role: person.role.clone(),
                enabled: true,
            })
            .collect(),
        terms: snapshot
            .terms
            .iter()
            .map(|term| TermProfile {
                term_id: term.term_id.clone(),
                canonical: term.canonical.clone(),
                aliases: term.aliases.clone(),
                category: term.category.clone(),
                enabled: true,
            })
            .collect(),
    };
    if let Err(projected_issues) = normalize_and_validate_profile(profile_projection) {
        issues.extend(projected_issues);
    }

    if let Some(host_id) = snapshot.host_person_id.as_deref() {
        let host_key = comparison_key(host_id);
        if !snapshot
            .people
            .iter()
            .any(|person| comparison_key(&person.person_id) == host_key)
        {
            issues.push(MeetingContextValidationIssue::new(
                "HOST_PERSON_NOT_FOUND",
                "/host_person_id",
            ));
        }
    }

    if issues.is_empty() {
        Ok(snapshot)
    } else {
        Err(issues)
    }
}

pub fn normalize_and_validate_container(
    mut container: MeetingContextContainer,
) -> Result<MeetingContextContainer, Vec<MeetingContextValidationIssue>> {
    let mut issues = Vec::new();
    if container.schema_version != MEETING_CONTEXT_SCHEMA_VERSION {
        issues.push(MeetingContextValidationIssue::new(
            "UNSUPPORTED_SCHEMA_VERSION",
            "/schema_version",
        ));
    }
    normalize_required(
        &mut container.recording_context_id,
        "/recording_context_id",
        MAX_NAME_CHARS,
        &mut issues,
    );
    normalize_required(
        &mut container.current_context_id,
        "/current_context_id",
        MAX_NAME_CHARS,
        &mut issues,
    );
    if container.contexts.is_empty() {
        issues.push(MeetingContextValidationIssue::new(
            "EMPTY_CONTEXTS",
            "/contexts",
        ));
    }

    let mut context_ids = BTreeSet::new();
    let mut revisions = BTreeSet::new();
    let mut previous_revision = 0;
    for (index, snapshot) in container.contexts.iter_mut().enumerate() {
        let stored_hash = snapshot.context_sha256.trim().to_owned();
        match normalize_and_validate_snapshot(snapshot.clone()) {
            Ok(normalized) => *snapshot = normalized,
            Err(snapshot_issues) => {
                issues.extend(snapshot_issues.into_iter().map(|issue| {
                    MeetingContextValidationIssue::new(
                        &issue.code,
                        format!("/contexts/{index}{}", issue.path),
                    )
                }));
            }
        }

        let id_key = comparison_key(&snapshot.context_id);
        if !id_key.is_empty() && !context_ids.insert(id_key) {
            issues.push(MeetingContextValidationIssue::new(
                "DUPLICATE_CONTEXT_ID",
                format!("/contexts/{index}/context_id"),
            ));
        }
        if snapshot.revision > 0 && !revisions.insert(snapshot.revision) {
            issues.push(MeetingContextValidationIssue::new(
                "DUPLICATE_REVISION",
                format!("/contexts/{index}/revision"),
            ));
        }
        if snapshot.revision <= previous_revision {
            issues.push(MeetingContextValidationIssue::new(
                "REVISION_ORDER_INVALID",
                format!("/contexts/{index}/revision"),
            ));
        }
        previous_revision = snapshot.revision;

        let mut normalized_hash = stored_hash;
        normalize_sha256(
            &mut normalized_hash,
            &format!("/contexts/{index}/context_sha256"),
            &mut issues,
        );
        snapshot.context_sha256 = normalized_hash;
        if snapshot.context_sha256.len() == 64
            && snapshot.context_sha256 != snapshot_sha256(snapshot)
        {
            issues.push(MeetingContextValidationIssue::new(
                "CONTEXT_HASH_MISMATCH",
                format!("/contexts/{index}/context_sha256"),
            ));
        }
    }

    if container
        .contexts
        .first()
        .is_some_and(|snapshot| snapshot.context_id != container.recording_context_id)
    {
        issues.push(MeetingContextValidationIssue::new(
            "RECORDING_CONTEXT_NOT_FIRST",
            "/recording_context_id",
        ));
    }
    if !container
        .contexts
        .iter()
        .any(|snapshot| snapshot.context_id == container.recording_context_id)
    {
        issues.push(MeetingContextValidationIssue::new(
            "RECORDING_CONTEXT_NOT_FOUND",
            "/recording_context_id",
        ));
    }
    if !container
        .contexts
        .iter()
        .any(|snapshot| snapshot.context_id == container.current_context_id)
    {
        issues.push(MeetingContextValidationIssue::new(
            "CURRENT_CONTEXT_NOT_FOUND",
            "/current_context_id",
        ));
    }

    if issues.is_empty() {
        Ok(container)
    } else {
        Err(issues)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting_context::canonical::{profile_sha256, snapshot_sha256};
    use crate::meeting_context::types::{
        AttendanceStatus, MeetingContextContainer, MeetingContextSource,
    };
    use chrono::{TimeZone, Utc};

    fn person(id: &str, name: &str, aliases: &[&str]) -> PersonProfile {
        PersonProfile {
            person_id: id.to_owned(),
            display_name: name.to_owned(),
            aliases: aliases.iter().map(|value| (*value).to_owned()).collect(),
            department: None,
            role: None,
            enabled: true,
        }
    }

    fn profile() -> MeetingContextProfile {
        MeetingContextProfile {
            schema_version: MEETING_CONTEXT_SCHEMA_VERSION,
            fixed_meeting_mechanism: Some(" 每周三 14:00 ".to_owned()),
            people: vec![person(
                "person_rayson",
                " Rayson ",
                &["瑞森", "Reason", "Risa", "Raison"],
            )],
            terms: vec![TermProfile {
                term_id: "term_m100".to_owned(),
                canonical: "M100".to_owned(),
                aliases: Vec::new(),
                category: Some("product".to_owned()),
                enabled: true,
            }],
        }
    }

    #[test]
    fn normalizes_valid_profile_and_produces_stable_hash() {
        let normalized = normalize_and_validate_profile(profile()).unwrap();
        let normalized_again = normalize_and_validate_profile(profile()).unwrap();
        assert_eq!(normalized.people[0].display_name, "Rayson");
        assert_eq!(
            normalized.fixed_meeting_mechanism.as_deref(),
            Some("每周三 14:00")
        );
        assert_eq!(
            profile_sha256(&normalized),
            profile_sha256(&normalized_again)
        );
    }

    #[test]
    fn rejects_duplicate_names_after_nfkc_and_ascii_case_normalization() {
        let mut value = profile();
        value.people.push(person("person_2", "ＲＡＹＳＯＮ", &[]));
        let issues = normalize_and_validate_profile(value).unwrap_err();
        assert!(issues
            .iter()
            .any(|issue| issue.code == "DUPLICATE_PERSON_NAME"));
    }

    #[test]
    fn rejects_alias_mapped_to_two_people() {
        let mut value = profile();
        value.people.push(person("person_amu", "Amu", &["瑞森"]));
        let issues = normalize_and_validate_profile(value).unwrap_err();
        assert!(issues
            .iter()
            .any(|issue| issue.code == "AMBIGUOUS_PERSON_ALIAS"));
    }

    #[test]
    fn rejects_duplicate_person_ids() {
        let mut value = profile();
        value.people.push(person("PERSON_RAYSON", "Amu", &[]));
        let issues = normalize_and_validate_profile(value).unwrap_err();
        assert!(issues
            .iter()
            .any(|issue| issue.code == "DUPLICATE_PERSON_ID"));
    }

    #[test]
    fn rejects_alias_mapped_to_two_terms() {
        let mut value = profile();
        value.terms[0].aliases.push("产品一百".to_owned());
        value.terms.push(TermProfile {
            term_id: "term_m200".to_owned(),
            canonical: "M200".to_owned(),
            aliases: vec!["产品一百".to_owned()],
            category: None,
            enabled: true,
        });
        let issues = normalize_and_validate_profile(value).unwrap_err();
        assert!(issues
            .iter()
            .any(|issue| issue.code == "AMBIGUOUS_TERM_ALIAS"));
    }

    #[test]
    fn rejects_people_over_limit() {
        let mut value = profile();
        value.people = (0..=MAX_PEOPLE)
            .map(|index| person(&format!("person_{index}"), &format!("Person {index}"), &[]))
            .collect();
        let issues = normalize_and_validate_profile(value).unwrap_err();
        assert!(issues.iter().any(|issue| issue.code == "TOO_MANY_PEOPLE"));
    }

    #[test]
    fn rejects_control_and_bidi_characters() {
        let mut value = profile();
        value.people[0].aliases.push("bad\nname".to_owned());
        value.terms[0].canonical.push('\u{202E}');
        let issues = normalize_and_validate_profile(value).unwrap_err();
        assert!(
            issues
                .iter()
                .filter(|issue| issue.code == "FORBIDDEN_CHARACTER")
                .count()
                >= 2
        );
    }

    #[test]
    fn snapshot_requires_existing_host() {
        let snapshot = MeetingContextSnapshot {
            context_id: "ctx_1".to_owned(),
            revision: 1,
            reason: "recording_start".to_owned(),
            captured_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            source: MeetingContextSource {
                template_id: "license_station_weekly".to_owned(),
                template_version: 1,
                template_file_sha256: "a".repeat(64),
                profile_sha256: "b".repeat(64),
            },
            fixed_meeting_mechanism: None,
            people: vec![SnapshotPerson {
                person_id: "person_rayson".to_owned(),
                display_name: "Rayson".to_owned(),
                aliases: vec!["瑞森".to_owned()],
                department: None,
                role: None,
                attendance: AttendanceStatus::Expected,
            }],
            host_person_id: Some("person_missing".to_owned()),
            terms: Vec::new(),
            context_sha256: String::new(),
        };
        let issues = normalize_and_validate_snapshot(snapshot).unwrap_err();
        assert!(issues
            .iter()
            .any(|issue| issue.code == "HOST_PERSON_NOT_FOUND"));
    }

    #[test]
    fn profile_creates_expected_recording_snapshot_and_append_only_revision() {
        let at = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let mut container = MeetingContextContainer::from_profile(
            profile(),
            "license_station_weekly".to_owned(),
            1,
            "a".repeat(64),
            at,
        )
        .unwrap();
        let original = container.recording_context().unwrap().clone();
        assert_eq!(original.people[0].attendance, AttendanceStatus::Expected);
        assert_eq!(original.context_sha256, snapshot_sha256(&original));

        let mut edited = original.clone();
        edited.people[0].attendance = AttendanceStatus::Attending;
        let revision_id = container
            .append_revision(edited, "attendance_correction", at)
            .unwrap()
            .context_id
            .clone();

        assert_eq!(container.recording_context().unwrap(), &original);
        assert_eq!(container.current_context_id, revision_id);
        assert_eq!(container.contexts.len(), 2);
        assert_eq!(container.contexts[1].revision, 2);
        assert_eq!(
            container.contexts[1].people[0].attendance,
            AttendanceStatus::Attending
        );
    }

    #[test]
    fn snapshot_hash_does_not_hash_itself() {
        let at = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let container = MeetingContextContainer::from_profile(
            profile(),
            "license_station_weekly".to_owned(),
            1,
            "a".repeat(64),
            at,
        )
        .unwrap();
        let mut snapshot = container.recording_context().unwrap().clone();
        let baseline = snapshot_sha256(&snapshot);
        snapshot.context_sha256 = "f".repeat(64);
        assert_eq!(snapshot_sha256(&snapshot), baseline);
    }

    #[test]
    fn profile_and_container_round_trip_without_data_loss() {
        let normalized = normalize_and_validate_profile(profile()).unwrap();
        let profile_json = serde_json::to_string(&normalized).unwrap();
        let restored_profile: MeetingContextProfile = serde_json::from_str(&profile_json).unwrap();
        assert_eq!(restored_profile, normalized);

        let at = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let container = MeetingContextContainer::from_profile(
            restored_profile,
            "license_station_weekly".to_owned(),
            1,
            "a".repeat(64),
            at,
        )
        .unwrap();
        let container_json = serde_json::to_string(&container).unwrap();
        let restored_container: MeetingContextContainer =
            serde_json::from_str(&container_json).unwrap();
        assert_eq!(
            normalize_and_validate_container(restored_container).unwrap(),
            container
        );
    }

    #[test]
    fn container_rejects_broken_references_duplicate_revisions_and_tampering() {
        let at = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let mut container = MeetingContextContainer::from_profile(
            profile(),
            "license_station_weekly".to_owned(),
            1,
            "a".repeat(64),
            at,
        )
        .unwrap();
        let mut duplicate = container.contexts[0].clone();
        duplicate.context_id = "ctx_duplicate".to_owned();
        duplicate.context_sha256 = snapshot_sha256(&duplicate);
        container.contexts.push(duplicate);
        container.current_context_id = "ctx_missing".to_owned();
        container.contexts[0].people[0].display_name = "Tampered".to_owned();

        let issues = normalize_and_validate_container(container).unwrap_err();
        assert!(issues
            .iter()
            .any(|issue| issue.code == "DUPLICATE_REVISION"));
        assert!(issues
            .iter()
            .any(|issue| issue.code == "REVISION_ORDER_INVALID"));
        assert!(issues
            .iter()
            .any(|issue| issue.code == "CURRENT_CONTEXT_NOT_FOUND"));
        assert!(issues
            .iter()
            .any(|issue| issue.code == "CONTEXT_HASH_MISMATCH"));
    }

    #[test]
    fn snapshot_rejects_non_canonical_sha256_values() {
        let at = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let mut container = MeetingContextContainer::from_profile(
            profile(),
            "license_station_weekly".to_owned(),
            1,
            "a".repeat(64),
            at,
        )
        .unwrap();
        container.contexts[0].source.template_file_sha256 = "A".repeat(64);
        let issues = normalize_and_validate_container(container).unwrap_err();
        assert!(issues.iter().any(|issue| {
            issue.code == "INVALID_SHA256"
                && issue.path == "/contexts/0/source/template_file_sha256"
        }));
    }
}
