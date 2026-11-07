//! Deterministic domain-term correction for transcripts.
//!
//! MAIN-046. MAIN-044 measured the same 20 second windows through Whisper
//! (greedy and beam), SenseVoice-Small, streaming Zipformer and streaming
//! Paraformer: every engine mis-hears the domain terms in this content, e.g.
//! "V4 Pro" -> "vispro" / "VS Pro" / "威斯普罗" and "V4.1 Flash" ->
//! "vis一f" / "VS点E Flash" / "fsh". Contextual biasing (sherpa-onnx hotwords,
//! modified_beam_search) was measured ineffective, and "V4.1" cannot even be
//! encoded by that model's BPE vocabulary.
//!
//! So the correction is done here, as explicit and auditable substitutions.
//! Two rules of the house apply:
//!   1. Nothing is deleted. Every rule maps a wrong form onto a canonical term.
//!   2. Every rule that fires is reported by id, so a transcript can be audited
//!      and the table can be extended from real transcripts instead of guesses.

use regex::Regex;
use std::sync::OnceLock;

struct CorrectionRule {
    id: &'static str,
    pattern: &'static str,
    replacement: &'static str,
}

/// Only variants actually observed in this project's own transcripts.
const RULES: &[CorrectionRule] = &[
    CorrectionRule {
        id: "v4_pro",
        // No \b anchors: CJK characters count as word characters in Rust's
        // Unicode regex, so "发到vispro的请求" has no word boundary after "pro".
        // The patterns stay specific enough to be safe without them.
        pattern: r"(?i)(?:vis|vs|v)\s*-?\s*pro|v4\s*-?\s*pro|威斯\s*普罗|维斯\s*普罗|威斯\s*朋\s*普罗",
        replacement: "V4 Pro",
    },
    CorrectionRule {
        id: "v41_flash",
        pattern: r"(?i)vis\s*-?\s*一\s*-?\s*f|vs\s*[点件]\s*e?\s*-?\s*flash|vis(?:dom)?\s*e?\s*-?\s*flash|v4\.?1\s*e?\s*-?\s*flash|微信店\s*一一\s*[A-Za-z]*",
        replacement: "V4.1 Flash",
    },
    CorrectionRule {
        // P1-12：真实转写里出现过「转发到这个V点 / V店」，指的就是 V4.1
        id: "v41_short",
        // 不能写 \b：CJK 字符在 Rust 的 Unicode 正则里也算 word character，
        // 「这个V点」两个词之间没有边界。改用"前面不是英数字"的判断。
        pattern: r"(?i)(^|[^A-Za-z0-9.])v\s*[点店]",
        replacement: "${1}V4.1",
    },
    CorrectionRule {
        // P1-12：单独的「一flash」（前一个词不是 V4/vs）同样是 V4.1 Flash
        id: "v41_flash_short",
        pattern: r"(?i)(^|[^A-Za-z0-9.])(?:一|1)\s*[- ]?\s*flash",
        replacement: "${1}V4.1 Flash",
    },
    CorrectionRule {
        id: "v40_flash",
        pattern: r"(?i)4\.0\s*e?\s*-?\s*(?:flash|fresh)",
        replacement: "V4.0 Flash",
    },
    CorrectionRule {
        id: "v4_flash",
        pattern: r"(?i)vi?s?\s*4\s*[- ]?\s*flash|v4\s*fresh|v4f(?:lash)?",
        replacement: "V4 Flash",
    },
    CorrectionRule {
        // P1-12 续：多场会议的真实落库文本里，「比Vf的话是高了将近20分」和
        // 「比V4 Flash高了将近20分」是同一句话；同段里另有一句「比pro的话…高了12」
        // 说明 Vf 指的就是 V4 Flash（不是 V4 Pro）。
        id: "v4_flash_abbrev",
        pattern: r"(?i)(^|[^A-Za-z0-9])v\s*f([^A-Za-z0-9]|$)",
        replacement: "${1}V4 Flash${2}",
    },
    CorrectionRule {
        id: "flash_short",
        pattern: r"(?i)fsh|flsah|f1ash|flash\s*lash|flashi?lash",
        replacement: "Flash",
    },
    CorrectionRule {
        id: "gpt56",
        pattern: r"(?i)gpt\s*-?\s*5\.6\s*(?:so|soul)?",
        replacement: "GPT-5.6",
    },
    CorrectionRule {
        id: "deepseek",
        pattern: r"(?i)deep\s*-?\s*seek\s*(?:hall?is|han+is|hannes)?",
        replacement: "DeepSeek",
    },
];

fn compiled() -> &'static Vec<(Regex, &'static CorrectionRule)> {
    static COMPILED: OnceLock<Vec<(Regex, &'static CorrectionRule)>> = OnceLock::new();
    COMPILED.get_or_init(|| {
        RULES
            .iter()
            .map(|rule| {
                (
                    Regex::new(rule.pattern).expect("term correction pattern must compile"),
                    rule,
                )
            })
            .collect()
    })
}

/// Apply the term table. Returns the corrected text and the ids of the rules
/// that fired, in application order.
pub fn correct_terms(text: &str) -> (String, Vec<&'static str>) {
    if text.is_empty() {
        return (String::new(), Vec::new());
    }

    let mut current = text.to_string();
    let mut applied = Vec::new();
    for (regex, rule) in compiled() {
        if regex.is_match(&current) {
            current = regex.replace_all(&current, rule.replacement).to_string();
            applied.push(rule.id);
        }
    }
    (current, applied)
}

#[cfg(test)]
mod tests {
    use super::correct_terms;

    #[test]
    fn corrects_the_variants_seen_in_real_transcripts() {
        let cases = [
            ("发到vispro的请求", "发到V4 Pro的请求"),
            ("所有发到VS Pro的请求", "所有发到V4 Pro的请求"),
            ("所有发到威斯普罗的请求", "所有发到V4 Pro的请求"),
            ("转发到这个vis一f模型里面", "转发到这个V4.1 Flash模型里面"),
            ("转发到VS点E Flash的模型", "转发到V4.1 Flash的模型"),
            ("优于这个vis pro", "优于这个V4 Pro"),
            ("我用一个fsh模型", "我用一个Flash模型"),
            ("跟这个GPT5.6so对比", "跟这个GPT-5.6对比"),
            // P1-12：2026-09-11 落库文本里实测到的变体
            ("所有发到V4pro的请求将会转发到这个V点。", "所有发到V4 Pro的请求将会转发到这个V4.1。"),
            ("那么比vis4 flash的话是高了", "那么比V4 Flash的话是高了"),
            ("那么比V4Flash的话是高了有二十分", "那么比V4 Flash的话是高了有二十分"),
            ("那么比V4Fresh", "那么比V4 Flash"),
            ("我们看一下4.0EFresh", "我们看一下V4.0 Flash"),
            ("转发到这个V4.1 Flashlash这个模型里面", "转发到这个V4.1 Flash这个模型里面"),
            ("然后一flash这个模型里面", "然后V4.1 Flash这个模型里面"),
            // P1-12 续：Vf 也是 V4 Flash（同段还有「比pro…高了12」可区分）
            ("那么比Vf的话是高了有将近20分了", "那么比V4 Flash的话是高了有将近20分了"),
        ];
        for (input, expected) in cases {
            let (corrected, applied) = correct_terms(input);
            assert_eq!(corrected, expected, "input: {input}");
            assert!(!applied.is_empty(), "no rule fired for {input}");
        }
    }

    #[test]
    fn leaves_unrelated_text_untouched() {
        let input = "这个提升是非常大的，那么比Pro的话分数也是将近高了十二分。";
        let (corrected, applied) = correct_terms(input);
        assert_eq!(corrected, input);
        assert!(applied.is_empty());
    }

    #[test]
    fn never_deletes_content() {
        let input = "所以官方发布了个公告 所有发到VS Pro的请求 将会转发到VS点E Flash的模型里面";
        let (corrected, _) = correct_terms(input);
        assert!(corrected.contains("所以官方发布了个公告"));
        assert!(corrected.contains("将会转发到"));
        assert!(corrected.contains("的模型里面"));
    }
}
