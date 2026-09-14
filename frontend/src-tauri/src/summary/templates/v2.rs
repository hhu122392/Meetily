use super::types::{Template, TemplateSection};
use crate::meeting_context::{normalize_and_validate_profile, MeetingContextProfile};
use chrono::{DateTime, FixedOffset};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};

const TEMPLATE_V2_SCHEMA_SOURCE: &str = include_str!("../../../schemas/template-v2.schema.json");
const MAX_TEMPLATE_CHARACTERS: usize = 200_000;
const MEETING_CONTEXT_EXTENSION_KEY: &str = "meetily_meeting_context";

static TEMPLATE_V2_SCHEMA: Lazy<Result<Value, String>> = Lazy::new(|| {
    serde_json::from_str(TEMPLATE_V2_SCHEMA_SOURCE)
        .map_err(|error| format!("Template v2 schema is invalid JSON: {error}"))
});

static TEMPLATE_V2_VALIDATOR: Lazy<Result<jsonschema::Validator, String>> = Lazy::new(|| {
    let schema = TEMPLATE_V2_SCHEMA.as_ref().map_err(Clone::clone)?;
    jsonschema::draft202012::options()
        .should_validate_formats(true)
        .build(schema)
        .map_err(|error| format!("Template v2 schema failed to compile: {error}"))
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateSourceType {
    Builtin,
    Manual,
    JsonImport,
    DocxImport,
    DocImport,
    BuiltinCopy,
    Duplicate,
    LegacyMigration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateSource {
    #[serde(rename = "type")]
    pub source_type: TemplateSourceType,
    #[serde(default)]
    pub original_file_name: Option<String>,
    #[serde(default)]
    pub original_file_sha256: Option<String>,
    #[serde(default)]
    pub imported_at: Option<DateTime<FixedOffset>>,
    #[serde(default)]
    pub copied_from_template_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateFormat {
    Paragraph,
    List,
    String,
}

impl TemplateFormat {
    fn as_runtime_str(self) -> &'static str {
        match self {
            Self::Paragraph => "paragraph",
            Self::List => "list",
            Self::String => "string",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmptyBehavior {
    Omit,
    ShowNotMentioned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateSectionV2 {
    pub id: String,
    pub title: String,
    pub instruction: String,
    pub format: TemplateFormat,
    #[serde(default)]
    pub item_format: Option<String>,
    #[serde(default)]
    pub example_item_format: Option<String>,
    pub required: bool,
    pub empty_behavior: EmptyBehavior,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateV2 {
    pub schema_version: u8,
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: u64,
    pub locale: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub source: TemplateSource,
    pub created_at: DateTime<FixedOffset>,
    pub updated_at: DateTime<FixedOffset>,
    pub sections: Vec<TemplateSectionV2>,
    #[serde(default)]
    pub extensions: Map<String, Value>,
}

impl TemplateV2 {
    pub fn to_runtime_template(&self) -> Template {
        Template {
            name: self.name.clone(),
            description: self.description.clone(),
            sections: self
                .sections
                .iter()
                .map(|section| TemplateSection {
                    title: section.title.clone(),
                    instruction: section.instruction.clone(),
                    format: section.format.as_runtime_str().to_owned(),
                    item_format: section.item_format.clone(),
                    example_item_format: section.example_item_format.clone(),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateFieldIssue {
    pub code: String,
    pub path: String,
    pub message_key: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, Value>,
}

impl TemplateFieldIssue {
    fn new(code: &str, path: impl Into<String>, message_key: &str) -> Self {
        Self {
            code: code.to_owned(),
            path: path.into(),
            message_key: message_key.to_owned(),
            params: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateValidationResult {
    pub valid: bool,
    pub errors: Vec<TemplateFieldIssue>,
    pub warnings: Vec<TemplateFieldIssue>,
}

pub fn parse_and_validate_template_v2(json: &str) -> Result<TemplateV2, TemplateValidationResult> {
    let value: Value = match serde_json::from_str(json) {
        Ok(value) => value,
        Err(_) => {
            return Err(TemplateValidationResult {
                valid: false,
                errors: vec![TemplateFieldIssue::new(
                    "INVALID_JSON",
                    "",
                    "templates.validation.invalidJson",
                )],
                warnings: Vec::new(),
            });
        }
    };

    let validation = validate_template_v2_value(&value);
    if !validation.valid {
        return Err(validation);
    }

    serde_json::from_value(value).map_err(|_| TemplateValidationResult {
        valid: false,
        errors: vec![TemplateFieldIssue::new(
            "DESERIALIZATION_FAILED",
            "",
            "templates.validation.deserializationFailed",
        )],
        warnings: Vec::new(),
    })
}

pub fn validate_template_v2(template: &TemplateV2) -> TemplateValidationResult {
    match serde_json::to_value(template) {
        Ok(value) => validate_template_v2_value(&value),
        Err(_) => TemplateValidationResult {
            valid: false,
            errors: vec![TemplateFieldIssue::new(
                "SERIALIZATION_FAILED",
                "",
                "templates.validation.serializationFailed",
            )],
            warnings: Vec::new(),
        },
    }
}

pub fn validate_template_v2_value(value: &Value) -> TemplateValidationResult {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    match TEMPLATE_V2_VALIDATOR.as_ref() {
        Ok(validator) => {
            if let Err(validation_errors) = validator.validate(value) {
                for error in validation_errors {
                    errors.push(TemplateFieldIssue::new(
                        "SCHEMA_VIOLATION",
                        error.instance_path.to_string(),
                        "templates.validation.schemaViolation",
                    ));
                }
            }
        }
        Err(_) => errors.push(TemplateFieldIssue::new(
            "SCHEMA_UNAVAILABLE",
            "",
            "templates.validation.schemaUnavailable",
        )),
    }

    let Ok(template) = serde_json::from_value::<TemplateV2>(value.clone()) else {
        return TemplateValidationResult {
            valid: false,
            errors,
            warnings,
        };
    };

    validate_non_blank(&template.name, "/name", &mut errors);
    validate_non_blank(&template.description, "/description", &mut errors);

    let mut section_ids = HashSet::new();
    for (index, section) in template.sections.iter().enumerate() {
        validate_non_blank(
            &section.title,
            &format!("/sections/{index}/title"),
            &mut errors,
        );
        validate_non_blank(
            &section.instruction,
            &format!("/sections/{index}/instruction"),
            &mut errors,
        );
        if !section_ids.insert(section.id.as_str()) {
            errors.push(TemplateFieldIssue::new(
                "DUPLICATE_SECTION_ID",
                format!("/sections/{index}/id"),
                "templates.validation.duplicateSectionId",
            ));
        }
    }

    if template.updated_at < template.created_at {
        errors.push(TemplateFieldIssue::new(
            "UPDATED_BEFORE_CREATED",
            "/updated_at",
            "templates.validation.updatedBeforeCreated",
        ));
    }

    if let Some(value) = template.extensions.get(MEETING_CONTEXT_EXTENSION_KEY) {
        match serde_json::from_value::<MeetingContextProfile>(value.clone()) {
            Ok(profile) => {
                if let Err(profile_issues) = normalize_and_validate_profile(profile) {
                    errors.extend(profile_issues.into_iter().map(|issue| {
                        TemplateFieldIssue::new(
                            &format!("MEETING_CONTEXT_{}", issue.code),
                            format!("/extensions/{MEETING_CONTEXT_EXTENSION_KEY}{}", issue.path),
                            "templates.validation.meetingContextInvalid",
                        )
                    }));
                }
            }
            Err(_) => errors.push(TemplateFieldIssue::new(
                "MEETING_CONTEXT_DESERIALIZATION_FAILED",
                format!("/extensions/{MEETING_CONTEXT_EXTENSION_KEY}"),
                "templates.validation.meetingContextInvalid",
            )),
        }
    }

    let total_characters = count_template_characters(&template);
    if total_characters > MAX_TEMPLATE_CHARACTERS {
        let mut issue =
            TemplateFieldIssue::new("TEMPLATE_TOO_LARGE", "", "templates.validation.tooLarge");
        issue.params.insert(
            "maxCharacters".to_owned(),
            Value::from(MAX_TEMPLATE_CHARACTERS as u64),
        );
        errors.push(issue);
    } else if total_characters > MAX_TEMPLATE_CHARACTERS * 9 / 10 {
        warnings.push(TemplateFieldIssue::new(
            "TEMPLATE_NEAR_SIZE_LIMIT",
            "",
            "templates.validation.nearSizeLimit",
        ));
    }

    TemplateValidationResult {
        valid: errors.is_empty(),
        errors,
        warnings,
    }
}

fn validate_non_blank(value: &str, path: &str, errors: &mut Vec<TemplateFieldIssue>) {
    if value.trim().is_empty() {
        errors.push(TemplateFieldIssue::new(
            "BLANK_VALUE",
            path,
            "templates.validation.blankValue",
        ));
    }
}

fn count_template_characters(template: &TemplateV2) -> usize {
    template.name.chars().count()
        + template.description.chars().count()
        + template
            .tags
            .iter()
            .map(|tag| tag.chars().count())
            .sum::<usize>()
        + template
            .sections
            .iter()
            .map(|section| {
                section.title.chars().count()
                    + section.instruction.chars().count()
                    + section
                        .item_format
                        .as_deref()
                        .map(|value| value.chars().count())
                        .unwrap_or(0)
                    + section
                        .example_item_format
                        .as_deref()
                        .map(|value| value.chars().count())
                        .unwrap_or(0)
            })
            .sum::<usize>()
}

pub fn migrate_v1_to_v2(
    id_hint: &str,
    template: &Template,
    migrated_at: DateTime<FixedOffset>,
) -> Result<TemplateV2, TemplateValidationResult> {
    if template.validate().is_err() {
        return Err(TemplateValidationResult {
            valid: false,
            errors: vec![TemplateFieldIssue::new(
                "LEGACY_TEMPLATE_INVALID",
                "",
                "templates.validation.legacyInvalid",
            )],
            warnings: Vec::new(),
        });
    }

    let id = safe_identifier(id_hint, &template.name, "template");
    let mut used_section_ids = HashSet::new();
    let sections = template
        .sections
        .iter()
        .map(|section| {
            let base = safe_identifier(&section.title, &section.title, "section");
            let section_id = unique_identifier(base, &mut used_section_ids);
            TemplateSectionV2 {
                id: section_id,
                title: section.title.clone(),
                instruction: section.instruction.clone(),
                format: match section.format.as_str() {
                    "paragraph" => TemplateFormat::Paragraph,
                    "list" => TemplateFormat::List,
                    _ => TemplateFormat::String,
                },
                item_format: section.item_format.clone(),
                example_item_format: section.example_item_format.clone(),
                required: true,
                empty_behavior: EmptyBehavior::ShowNotMentioned,
            }
        })
        .collect();

    let migrated = TemplateV2 {
        schema_version: 2,
        id,
        name: template.name.clone(),
        description: template.description.clone(),
        version: 1,
        locale: None,
        tags: Vec::new(),
        source: TemplateSource {
            source_type: TemplateSourceType::LegacyMigration,
            original_file_name: None,
            original_file_sha256: None,
            imported_at: None,
            copied_from_template_id: None,
        },
        created_at: migrated_at,
        updated_at: migrated_at,
        sections,
        extensions: Map::new(),
    };

    let validation = validate_template_v2(&migrated);
    if validation.valid {
        Ok(migrated)
    } else {
        Err(validation)
    }
}

fn safe_identifier(primary: &str, fallback_source: &str, fallback_prefix: &str) -> String {
    let mut slug = String::new();
    let mut previous_separator = false;
    for character in primary.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
            previous_separator = false;
        } else if matches!(character, '_' | '-' | ' ' | '.')
            && !previous_separator
            && !slug.is_empty()
        {
            slug.push('_');
            previous_separator = true;
        }
        if slug.len() >= 72 {
            break;
        }
    }
    while slug.ends_with('_') {
        slug.pop();
    }

    if slug.len() < 3 {
        let digest = Sha256::digest(fallback_source.as_bytes());
        let suffix = digest[..6]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        slug = format!("{fallback_prefix}_{suffix}");
    }
    slug.truncate(80);
    while slug.ends_with('_') || slug.ends_with('-') {
        slug.pop();
    }
    slug
}

fn unique_identifier(mut candidate: String, used: &mut HashSet<String>) -> String {
    if used.insert(candidate.clone()) {
        return candidate;
    }

    let base = candidate.clone();
    let mut suffix = 2_u32;
    loop {
        let suffix_text = format!("_{suffix}");
        candidate = base
            .chars()
            .take(80_usize.saturating_sub(suffix_text.len()))
            .collect();
        candidate.push_str(&suffix_text);
        if used.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migrated_at() -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339("2026-08-23T10:00:00+08:00").unwrap()
    }

    fn parse_v1(source: &str) -> Template {
        serde_json::from_str(source).unwrap()
    }

    #[test]
    fn migrates_all_bundled_v1_templates_without_losing_section_content() {
        let fixtures = [
            (
                "daily_standup",
                include_str!("../../../templates/daily_standup.json"),
            ),
            (
                "project_sync",
                include_str!("../../../templates/project_sync.json"),
            ),
            (
                "psychatric_session",
                include_str!("../../../templates/psychatric_session.json"),
            ),
            (
                "retrospective",
                include_str!("../../../templates/retrospective.json"),
            ),
            (
                "sales_marketing_client_call",
                include_str!("../../../templates/sales_marketing_client_call.json"),
            ),
            (
                "standard_meeting",
                include_str!("../../../templates/standard_meeting.json"),
            ),
        ];

        for (id, source) in fixtures {
            let legacy = parse_v1(source);
            let migrated = migrate_v1_to_v2(id, &legacy, migrated_at()).unwrap();
            assert_eq!(migrated.sections.len(), legacy.sections.len());
            for (new, old) in migrated.sections.iter().zip(legacy.sections.iter()) {
                assert_eq!(new.title, old.title);
                assert_eq!(new.instruction, old.instruction);
                assert_eq!(new.item_format, old.item_format);
                assert_eq!(new.example_item_format, old.example_item_format);
            }
            assert!(validate_template_v2(&migrated).valid);
        }
    }

    #[test]
    fn duplicate_chinese_titles_receive_deterministic_unique_ids() {
        let legacy = Template {
            name: "中文模板".to_owned(),
            description: "用于测试".to_owned(),
            sections: vec![
                TemplateSection {
                    title: "行动项".to_owned(),
                    instruction: "提取行动项".to_owned(),
                    format: "list".to_owned(),
                    item_format: None,
                    example_item_format: None,
                },
                TemplateSection {
                    title: "行动项".to_owned(),
                    instruction: "再次提取行动项".to_owned(),
                    format: "list".to_owned(),
                    item_format: None,
                    example_item_format: None,
                },
            ],
        };
        let migrated = migrate_v1_to_v2("客户 会议.docx", &legacy, migrated_at()).unwrap();
        assert_ne!(migrated.sections[0].id, migrated.sections[1].id);
        assert!(migrated.sections[1].id.ends_with("_2"));
        assert!(validate_template_v2(&migrated).valid);
    }

    #[test]
    fn v2_round_trip_preserves_fixed_offset_time_and_extensions() {
        let legacy = parse_v1(include_str!("../../../templates/standard_meeting.json"));
        let mut migrated = migrate_v1_to_v2("standard_meeting", &legacy, migrated_at()).unwrap();
        migrated.extensions.insert(
            "vendor.example".to_owned(),
            serde_json::json!({"enabled": true}),
        );
        let json = serde_json::to_string(&migrated).unwrap();
        let reparsed = parse_and_validate_template_v2(&json).unwrap();
        assert_eq!(reparsed, migrated);
        assert_eq!(reparsed.created_at.offset().local_minus_utc(), 8 * 60 * 60);
    }

    #[test]
    fn duplicate_section_ids_and_reversed_times_are_rejected() {
        let legacy = parse_v1(include_str!("../../../templates/standard_meeting.json"));
        let mut migrated = migrate_v1_to_v2("standard_meeting", &legacy, migrated_at()).unwrap();
        migrated.sections[1].id = migrated.sections[0].id.clone();
        migrated.updated_at = DateTime::parse_from_rfc3339("2026-08-22T10:00:00+08:00").unwrap();
        let validation = validate_template_v2(&migrated);
        assert!(!validation.valid);
        assert!(validation
            .errors
            .iter()
            .any(|issue| issue.code == "DUPLICATE_SECTION_ID"));
        assert!(validation
            .errors
            .iter()
            .any(|issue| issue.code == "UPDATED_BEFORE_CREATED"));
    }

    #[test]
    fn unknown_root_field_does_not_crash_serde_but_fails_authoritative_schema() {
        let legacy = parse_v1(include_str!("../../../templates/standard_meeting.json"));
        let migrated = migrate_v1_to_v2("standard_meeting", &legacy, migrated_at()).unwrap();
        let mut value = serde_json::to_value(migrated).unwrap();
        value["future_field"] = Value::Bool(true);
        assert!(serde_json::from_value::<TemplateV2>(value.clone()).is_ok());
        assert!(!validate_template_v2_value(&value).valid);
    }

    #[test]
    fn valid_meeting_context_extension_and_unknown_vendor_extension_are_preserved() {
        let legacy = parse_v1(include_str!("../../../templates/standard_meeting.json"));
        let mut migrated = migrate_v1_to_v2("standard_meeting", &legacy, migrated_at()).unwrap();
        migrated.extensions.insert(
            MEETING_CONTEXT_EXTENSION_KEY.to_owned(),
            serde_json::json!({
                "schema_version": 1,
                "fixed_meeting_mechanism": "每周三 14:00",
                "people": [{
                    "person_id": "person_rayson",
                    "display_name": "Rayson",
                    "aliases": ["瑞森"],
                    "enabled": true
                }],
                "terms": []
            }),
        );
        migrated.extensions.insert(
            "vendor.example".to_owned(),
            serde_json::json!({"keep": true}),
        );

        let json = serde_json::to_string(&migrated).unwrap();
        let restored = parse_and_validate_template_v2(&json).unwrap();
        assert_eq!(
            restored.extensions["vendor.example"],
            serde_json::json!({"keep": true})
        );
        assert!(restored
            .extensions
            .contains_key(MEETING_CONTEXT_EXTENSION_KEY));
    }

    #[test]
    fn invalid_meeting_context_extension_is_rejected_with_precise_path() {
        let legacy = parse_v1(include_str!("../../../templates/standard_meeting.json"));
        let mut migrated = migrate_v1_to_v2("standard_meeting", &legacy, migrated_at()).unwrap();
        migrated.extensions.insert(
            MEETING_CONTEXT_EXTENSION_KEY.to_owned(),
            serde_json::json!({
                "schema_version": 1,
                "people": [
                    {
                        "person_id": "person_1",
                        "display_name": "Rayson",
                        "aliases": ["瑞森"]
                    },
                    {
                        "person_id": "person_2",
                        "display_name": "Amu",
                        "aliases": ["瑞森"]
                    }
                ]
            }),
        );

        let result = validate_template_v2(&migrated);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|issue| {
            issue.code == "MEETING_CONTEXT_AMBIGUOUS_PERSON_ALIAS"
                && issue.path == "/extensions/meetily_meeting_context/people/1/aliases/0"
        }));
    }
}
