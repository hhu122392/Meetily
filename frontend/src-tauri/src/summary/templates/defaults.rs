use super::content_locale::{resolve_content_locale, ContentLocaleResolution};

const BUILTIN_IDS: [&str; 6] = [
    "daily_standup",
    "project_sync",
    "psychatric_session",
    "retrospective",
    "sales_marketing_client_call",
    "standard_meeting",
];

const DAILY_STANDUP_EN: &str = include_str!("../../../templates/en/daily_standup.json");
const DAILY_STANDUP_ZH_CN: &str = include_str!("../../../templates/zh-CN/daily_standup.json");
const PROJECT_SYNC_EN: &str = include_str!("../../../templates/en/project_sync.json");
const PROJECT_SYNC_ZH_CN: &str = include_str!("../../../templates/zh-CN/project_sync.json");
const PSYCHATRIC_SESSION_EN: &str = include_str!("../../../templates/en/psychatric_session.json");
const PSYCHATRIC_SESSION_ZH_CN: &str =
    include_str!("../../../templates/zh-CN/psychatric_session.json");
const RETROSPECTIVE_EN: &str = include_str!("../../../templates/en/retrospective.json");
const RETROSPECTIVE_ZH_CN: &str = include_str!("../../../templates/zh-CN/retrospective.json");
const SALES_CALL_EN: &str = include_str!("../../../templates/en/sales_marketing_client_call.json");
const SALES_CALL_ZH_CN: &str =
    include_str!("../../../templates/zh-CN/sales_marketing_client_call.json");
const STANDARD_MEETING_EN: &str = include_str!("../../../templates/en/standard_meeting.json");
const STANDARD_MEETING_ZH_CN: &str = include_str!("../../../templates/zh-CN/standard_meeting.json");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBuiltinTemplate {
    pub id: &'static str,
    pub locale: ContentLocaleResolution,
    pub content: &'static str,
}

fn localized_source(id: &str, locale: &str) -> Option<&'static str> {
    match (id, locale) {
        ("daily_standup", "en") => Some(DAILY_STANDUP_EN),
        ("daily_standup", "zh-CN") => Some(DAILY_STANDUP_ZH_CN),
        ("project_sync", "en") => Some(PROJECT_SYNC_EN),
        ("project_sync", "zh-CN") => Some(PROJECT_SYNC_ZH_CN),
        ("psychatric_session", "en") => Some(PSYCHATRIC_SESSION_EN),
        ("psychatric_session", "zh-CN") => Some(PSYCHATRIC_SESSION_ZH_CN),
        ("retrospective", "en") => Some(RETROSPECTIVE_EN),
        ("retrospective", "zh-CN") => Some(RETROSPECTIVE_ZH_CN),
        ("sales_marketing_client_call", "en") => Some(SALES_CALL_EN),
        ("sales_marketing_client_call", "zh-CN") => Some(SALES_CALL_ZH_CN),
        ("standard_meeting", "en") => Some(STANDARD_MEETING_EN),
        ("standard_meeting", "zh-CN") => Some(STANDARD_MEETING_ZH_CN),
        _ => None,
    }
}

pub fn get_builtin_template_for_locale(
    id: &str,
    requested_locale: Option<&str>,
) -> Option<ResolvedBuiltinTemplate> {
    if !BUILTIN_IDS.contains(&id) {
        return None;
    }
    let available = ["en", "zh-CN"];
    let locale = resolve_content_locale(requested_locale, &available);
    let content =
        localized_source(id, &locale.resolved_locale).or_else(|| localized_source(id, "en"))?;
    Some(ResolvedBuiltinTemplate {
        id: BUILTIN_IDS
            .iter()
            .copied()
            .find(|candidate| *candidate == id)?,
        locale,
        content,
    })
}

/// Compatibility API: legacy callers receive the canonical English variant.
pub fn get_builtin_template(id: &str) -> Option<&'static str> {
    get_builtin_template_for_locale(id, Some("en")).map(|resolved| resolved.content)
}

pub fn get_builtin_templates() -> Vec<(&'static str, &'static str)> {
    BUILTIN_IDS
        .iter()
        .filter_map(|id| get_builtin_template(id).map(|content| (*id, content)))
        .collect()
}

pub fn get_all_localized_builtin_templates() -> Vec<(&'static str, &'static str, &'static str)> {
    BUILTIN_IDS
        .iter()
        .flat_map(|id| {
            ["en", "zh-CN"].into_iter().filter_map(|locale| {
                localized_source(id, locale).map(|content| (*id, locale, content))
            })
        })
        .collect()
}

pub fn list_builtin_template_ids() -> Vec<&'static str> {
    BUILTIN_IDS.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summary::templates::parse_and_validate_template_v2;
    use std::collections::HashSet;

    #[test]
    fn every_builtin_locale_is_valid_v2_and_identity_matches_registry() {
        for (id, locale, content) in get_all_localized_builtin_templates() {
            let template = parse_and_validate_template_v2(content)
                .unwrap_or_else(|error| panic!("{id}/{locale} failed: {:?}", error.errors));
            assert_eq!(template.id, id);
            assert_eq!(template.locale.as_deref(), Some(locale));
            assert_eq!(template.version, 1);
        }
    }

    #[test]
    fn english_and_chinese_have_identical_stable_section_structure() {
        for id in list_builtin_template_ids() {
            let en = parse_and_validate_template_v2(
                get_builtin_template_for_locale(id, Some("en"))
                    .unwrap()
                    .content,
            )
            .unwrap();
            let zh = parse_and_validate_template_v2(
                get_builtin_template_for_locale(id, Some("zh-CN"))
                    .unwrap()
                    .content,
            )
            .unwrap();
            assert_eq!(en.id, zh.id);
            assert_eq!(en.version, zh.version);
            assert_eq!(en.sections.len(), zh.sections.len());
            for (en_section, zh_section) in en.sections.iter().zip(&zh.sections) {
                assert_eq!(en_section.id, zh_section.id);
                assert_eq!(en_section.format, zh_section.format);
                assert_eq!(en_section.required, zh_section.required);
                assert_eq!(en_section.empty_behavior, zh_section.empty_behavior);
            }
        }
    }

    #[test]
    fn registry_ids_are_globally_unique_and_all_six_are_available() {
        let ids = list_builtin_template_ids();
        assert_eq!(ids.len(), 6);
        assert_eq!(ids.iter().copied().collect::<HashSet<_>>().len(), 6);
    }

    #[test]
    fn unsupported_locale_and_missing_locale_use_english_fallback() {
        let unsupported = get_builtin_template_for_locale("daily_standup", Some("fr")).unwrap();
        assert_eq!(unsupported.locale.resolved_locale, "en");
        assert!(unsupported.locale.fell_back);
        let missing = get_builtin_template_for_locale("daily_standup", None).unwrap();
        assert_eq!(missing.locale.resolved_locale, "en");
    }
}
