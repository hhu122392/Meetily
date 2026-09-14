use opencc_fmmseg::OpenCC;
use regex::{Captures, Regex};
use std::sync::LazyLock;

/// OpenCC ships its dictionaries inside the binary. Keep one initialized
/// converter so live transcription does not pay the dictionary setup cost for
/// every speech segment.
static CHINESE_CONVERTER: LazyLock<OpenCC> = LazyLock::new(OpenCC::new);
static CHINESE_YEAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?P<number>[0-9]{4})年").expect("valid year regex"));
static CHINESE_DATE_TIME_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?P<number>[0-9]{1,3})(?P<unit>月|日|点|分|秒)").expect("valid date/time regex")
});
fn requests_simplified_chinese(language: Option<&str>) -> bool {
    language.is_some_and(|value| {
        let normalized = value.trim().to_ascii_lowercase().replace('_', "-");
        normalized == "zh"
            || normalized == "zh-cn"
            || normalized == "zh-sg"
            || normalized == "zh-hans"
            || normalized.starts_with("zh-hans-")
    })
}

/// Normalize explicit Simplified Chinese requests before text is emitted,
/// copied, or persisted. Keep recognized names and English words as supplied;
/// a source-language hint is not permission to guess spelling corrections.
pub fn normalize_transcript(text: &str, language: Option<&str>) -> String {
    if requests_simplified_chinese(language) {
        let simplified = normalize_simplified_chinese_script(text, language);
        normalize_zh_numbers(&simplified)
    } else {
        text.to_owned()
    }
}

/// Converts Traditional Chinese glyphs to Simplified Chinese without changing
/// dates, numeric values, Markdown, or domain terms. Summary output uses this
/// narrower normalization because business numbers must remain exactly as the
/// model rendered them.
pub fn normalize_simplified_chinese_script(text: &str, language: Option<&str>) -> String {
    if requests_simplified_chinese(language) {
        CHINESE_CONVERTER.t2s(text, false)
    } else {
        text.to_owned()
    }
}

fn digit_to_chinese(digit: char) -> char {
    match digit {
        '0' => '〇',
        '1' => '一',
        '2' => '二',
        '3' => '三',
        '4' => '四',
        '5' => '五',
        '6' => '六',
        '7' => '七',
        '8' => '八',
        '9' => '九',
        _ => digit,
    }
}

fn chinese_cardinal(value: u32) -> String {
    match value {
        0..=9 => value.to_string().chars().map(digit_to_chinese).collect(),
        10 => "十".to_string(),
        11..=19 => format!(
            "十{}",
            digit_to_chinese(char::from_digit(value % 10, 10).unwrap())
        ),
        20..=99 if value % 10 == 0 => format!(
            "{}十",
            digit_to_chinese(char::from_digit(value / 10, 10).unwrap())
        ),
        20..=99 => format!(
            "{}十{}",
            digit_to_chinese(char::from_digit(value / 10, 10).unwrap()),
            digit_to_chinese(char::from_digit(value % 10, 10).unwrap())
        ),
        _ => value.to_string(),
    }
}

fn normalize_zh_numbers(text: &str) -> String {
    let years = CHINESE_YEAR.replace_all(text, |caps: &Captures<'_>| {
        let digits = &caps["number"];
        let chinese: String = digits.chars().map(digit_to_chinese).collect();
        format!("{}年", chinese)
    });
    CHINESE_DATE_TIME_NUMBER
        .replace_all(&years, |caps: &Captures<'_>| {
            let value = caps["number"].parse::<u32>().unwrap_or(0);
            format!("{}{}", chinese_cardinal(value), &caps["unit"])
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::{normalize_simplified_chinese_script, normalize_transcript};

    #[test]
    fn converts_whisper_traditional_output_to_simplified_chinese() {
        let input = "為什麼你還有緩慢，就是說會比較慢啊";
        assert_eq!(
            normalize_transcript(input, Some("zh")),
            "为什么你还有缓慢，就是说会比较慢啊"
        );
    }

    #[test]
    fn supports_simplified_chinese_locale_aliases() {
        assert_eq!(normalize_transcript("會議", Some("zh-CN")), "会议");
        assert_eq!(normalize_transcript("錄音", Some("zh_Hans")), "录音");
    }

    #[test]
    fn summary_script_normalization_preserves_business_numbers_and_markdown() {
        let input = "**會議結論**\n\n- M100 回收率 20%，日期 2026年8月26日";
        assert_eq!(
            normalize_simplified_chinese_script(input, Some("zh-CN")),
            "**会议结论**\n\n- M100 回收率 20%，日期 2026年8月26日"
        );
    }

    #[test]
    fn preserves_non_simplified_language_requests() {
        let input = "為什麼會議還沒開始";
        assert_eq!(normalize_transcript(input, Some("zh-TW")), input);
        assert_eq!(normalize_transcript(input, Some("auto")), input);
        assert_eq!(normalize_transcript(input, Some("en")), input);
    }

    #[test]
    fn normalizes_chinese_dates_times_without_touching_domain_numbers() {
        let input = "2026年8月24日19点30分，8月28日前，暂停10秒，M100，API";
        assert_eq!(
            normalize_transcript(input, Some("zh")),
            "二〇二六年八月二十四日十九点三十分，八月二十八日前，暂停十秒，M100，API"
        );
    }

    #[test]
    fn preserves_recognized_product_entities() {
        let input = "我们讨论 Midly、midly、MEDLY、Medley、medley、Medally、medally、Meetly、meetly 和 Meetily";
        assert_eq!(normalize_transcript(input, Some("zh-CN")), input);
    }

    #[test]
    fn does_not_invent_product_or_model_names() {
        let input = "Midly核心功能包含QIN3.5和large-v3-turbo";
        assert_eq!(normalize_transcript(input, Some("zh")), input);
        assert_eq!(normalize_transcript(input, Some("en")), input);
    }

    #[test]
    fn preserves_english_words_for_all_source_language_hints() {
        let input = "我们用 Medley 管理音乐，播放这首 medley，不使用 Meetily。";
        for language in [
            None,
            Some("zh"),
            Some("zh-CN"),
            Some("zh-SG"),
            Some("zh_Hans"),
            Some("zh-Hans-CN"),
            Some("zh-TW"),
            Some("auto"),
            Some("en"),
        ] {
            assert_eq!(normalize_transcript(input, language), input, "{language:?}");
        }
    }

    #[test]
    fn preserves_name_substrings_in_identifiers_and_links() {
        let input =
            "请检查 MidlyTools、medley_player、https://medley.example/medally、@meetly 和 MEDLY_v2";
        assert_eq!(normalize_transcript(input, Some("zh")), input);
    }

    #[test]
    fn keeps_english_spelling_while_normalizing_chinese_script_and_dates() {
        let input = "2026年8月26日，會議使用 Medley review 這個 API，不使用 Meetily，回收率 20%。";
        assert_eq!(
            normalize_transcript(input, Some("zh")),
            "二〇二六年八月二十六日，会议使用 Medley review 这个 API，不使用 Meetily，回收率 20%。"
        );
    }

    #[test]
    fn preserves_mixed_asr_spelling_without_guessing_corrections() {
        let input =
            "Terra和Luna，把TrackPT、TragGPT Work、XGPT Work 与 ChatGPT review API 保留原样。";
        assert_eq!(normalize_transcript(input, Some("zh")), input);
    }

    #[test]
    fn preserves_empty_and_whitespace_only_input() {
        for input in ["", " ", "\n\t"] {
            assert_eq!(normalize_transcript(input, Some("zh")), input);
        }
    }

    #[test]
    fn normalizing_mixed_text_twice_does_not_change_it_again() {
        let input = "2026年8月26日，會議用 Medley review API，回收率 20%。";
        let once = normalize_transcript(input, Some("zh"));
        assert_eq!(normalize_transcript(&once, Some("zh")), once);
    }
}
