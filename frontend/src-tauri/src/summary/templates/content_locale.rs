use serde::{Deserialize, Serialize};

pub const DEFAULT_CONTENT_LOCALE: &str = "en";
pub const SIMPLIFIED_CHINESE_CONTENT_LOCALE: &str = "zh-CN";
pub const SUPPORTED_CONTENT_LOCALES: [&str; 2] =
    [DEFAULT_CONTENT_LOCALE, SIMPLIFIED_CHINESE_CONTENT_LOCALE];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentLocaleResolution {
    pub requested_locale: Option<String>,
    pub resolved_locale: String,
    pub fell_back: bool,
}

pub fn canonical_content_locale(value: Option<&str>) -> Option<&'static str> {
    let normalized = value?.trim().to_ascii_lowercase().replace('_', "-");
    if normalized == "en" || normalized.starts_with("en-") {
        Some(DEFAULT_CONTENT_LOCALE)
    } else if normalized == "zh" || normalized.starts_with("zh-") {
        Some(SIMPLIFIED_CHINESE_CONTENT_LOCALE)
    } else {
        None
    }
}

pub fn resolve_content_locale(
    requested: Option<&str>,
    available_locales: &[&str],
) -> ContentLocaleResolution {
    let canonical = canonical_content_locale(requested);
    let requested_supported = canonical.filter(|locale| {
        available_locales
            .iter()
            .any(|available| available == locale)
    });
    let resolved = requested_supported
        .or_else(|| {
            available_locales
                .iter()
                .copied()
                .find(|locale| *locale == DEFAULT_CONTENT_LOCALE)
        })
        .or_else(|| available_locales.first().copied())
        .unwrap_or(DEFAULT_CONTENT_LOCALE);

    ContentLocaleResolution {
        requested_locale: requested.map(str::to_owned),
        resolved_locale: resolved.to_owned(),
        fell_back: canonical != Some(resolved),
    }
}

/// Template content follows the explicit summary language, never the UI locale.
/// Automatic summary language uses the detected transcript language when known and
/// otherwise safely falls back to English.
pub fn content_locale_for_summary_language(
    summary_language: Option<&str>,
    detected_transcript_language: Option<&str>,
) -> &'static str {
    let summary = summary_language
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let requested = match summary {
        Some("__auto__") | None => detected_transcript_language,
        Some(value) => Some(value),
    };
    canonical_content_locale(requested).unwrap_or(DEFAULT_CONTENT_LOCALE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_supported_language_variants() {
        assert_eq!(canonical_content_locale(Some("en-GB")), Some("en"));
        assert_eq!(canonical_content_locale(Some("zh_TW")), Some("zh-CN"));
        assert_eq!(canonical_content_locale(Some("ja")), None);
    }

    #[test]
    fn missing_requested_resource_falls_back_to_english() {
        let resolution = resolve_content_locale(Some("zh-CN"), &["en"]);
        assert_eq!(resolution.resolved_locale, "en");
        assert!(resolution.fell_back);
    }

    #[test]
    fn language_matrix_is_independent_from_ui_locale() {
        let cases = [
            ("zh-CN", Some("zh"), Some("zh"), "zh-CN"),
            ("zh-CN", Some("zh"), Some("ja"), "zh-CN"),
            ("zh-CN", Some("en"), Some("en"), "en"),
            ("en", Some("zh"), Some("zh"), "zh-CN"),
            ("en", Some("__auto__"), Some("en"), "en"),
        ];
        for (ui_locale, summary_language, detected_language, expected) in cases {
            let before = ui_locale;
            assert_eq!(
                content_locale_for_summary_language(summary_language, detected_language),
                expected
            );
            assert_eq!(
                ui_locale, before,
                "content resolution must not mutate UI locale"
            );
        }
    }
}
