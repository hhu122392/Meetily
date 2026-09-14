use super::{AttendanceStatus, MeetingContextSnapshot};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const WHISPER_INITIAL_PROMPT_MAX_CHARS: usize = 256;

// This remains false until MC-R02 has passed with the installed model. The
// deterministic alias normalizer is safe and stays enabled independently.
pub const LIVE_WHISPER_PROMPT_ENABLED: bool = false;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecognitionContext {
    pub context_id: String,
    pub context_sha256: String,
    pub canonical_names: Vec<String>,
    pub explicit_alias_map: BTreeMap<String, String>,
    pub canonical_terms: Vec<String>,
    pub explicit_term_alias_map: BTreeMap<String, String>,
    pub whisper_initial_prompt: Option<String>,
    pub prompt_sha256: Option<String>,
    pub prompt_truncated: bool,
    pub live_prompt_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecognitionContextDiagnostics {
    pub context_id: String,
    pub context_sha256: String,
    pub canonical_name_count: usize,
    pub person_alias_count: usize,
    pub canonical_term_count: usize,
    pub term_alias_count: usize,
    pub prompt_sha256: Option<String>,
    pub prompt_chars: usize,
    pub prompt_truncated: bool,
    pub live_prompt_enabled: bool,
}

impl RecognitionContext {
    pub fn from_snapshot(snapshot: &MeetingContextSnapshot) -> Self {
        let mut high_priority_names = Vec::new();
        let mut other_names = Vec::new();
        let mut explicit_alias_map = BTreeMap::new();
        for person in &snapshot.people {
            if snapshot.host_person_id.as_deref() == Some(person.person_id.as_str())
                || matches!(
                    person.attendance,
                    AttendanceStatus::Attending | AttendanceStatus::Guest
                )
            {
                high_priority_names.push(person.display_name.clone());
            } else {
                other_names.push(person.display_name.clone());
            }
            for alias in &person.aliases {
                explicit_alias_map.insert(alias.clone(), person.display_name.clone());
            }
        }

        let canonical_names = high_priority_names
            .iter()
            .chain(other_names.iter())
            .cloned()
            .collect::<Vec<_>>();
        let canonical_terms = snapshot
            .terms
            .iter()
            .map(|term| term.canonical.clone())
            .collect::<Vec<_>>();
        let mut explicit_term_alias_map = BTreeMap::new();
        for term in &snapshot.terms {
            for alias in &term.aliases {
                explicit_term_alias_map.insert(alias.clone(), term.canonical.clone());
            }
        }

        let prompt_items = high_priority_names
            .iter()
            .chain(other_names.iter())
            .chain(canonical_terms.iter())
            .cloned()
            .collect::<Vec<_>>();
        let (whisper_initial_prompt, prompt_truncated) = build_prompt(&prompt_items);
        let prompt_sha256 = whisper_initial_prompt
            .as_ref()
            .map(|prompt| format!("{:x}", Sha256::digest(prompt.as_bytes())));

        Self {
            context_id: snapshot.context_id.clone(),
            context_sha256: snapshot.context_sha256.clone(),
            canonical_names,
            explicit_alias_map,
            canonical_terms,
            explicit_term_alias_map,
            whisper_initial_prompt,
            prompt_sha256,
            prompt_truncated,
            live_prompt_enabled: LIVE_WHISPER_PROMPT_ENABLED,
        }
    }

    pub fn diagnostics(&self) -> RecognitionContextDiagnostics {
        RecognitionContextDiagnostics {
            context_id: self.context_id.clone(),
            context_sha256: self.context_sha256.clone(),
            canonical_name_count: self.canonical_names.len(),
            person_alias_count: self.explicit_alias_map.len(),
            canonical_term_count: self.canonical_terms.len(),
            term_alias_count: self.explicit_term_alias_map.len(),
            prompt_sha256: self.prompt_sha256.clone(),
            prompt_chars: self
                .whisper_initial_prompt
                .as_ref()
                .map_or(0, |prompt| prompt.chars().count()),
            prompt_truncated: self.prompt_truncated,
            live_prompt_enabled: self.live_prompt_enabled,
        }
    }

    pub fn active_whisper_prompt(&self) -> Option<String> {
        self.live_prompt_enabled
            .then(|| self.whisper_initial_prompt.clone())
            .flatten()
    }

    /// Build an offline-only prompt from canonical terms. Names and aliases are
    /// deliberately excluded. The caller must keep this out of the live path.
    pub fn term_only_whisper_prompt(&self) -> (Option<String>, bool) {
        build_prompt(&self.canonical_terms)
    }

    pub fn normalize_transcript(&self, input: &str) -> String {
        let mut replacements = self
            .explicit_alias_map
            .iter()
            .chain(self.explicit_term_alias_map.iter())
            .map(|(alias, canonical)| (alias.as_str(), canonical.as_str()))
            .collect::<Vec<_>>();
        // ASCII entities are matched case-insensitively. Include their canonical
        // spellings as self-mappings so variants such as `youtube` and `YOUTUBE`
        // are normalized to the configured `YouTube` spelling even when no alias
        // was explicitly configured. Explicit aliases stay ahead of self-mappings
        // when two keys are identical, preserving the user's deterministic rule.
        replacements.extend(
            self.canonical_names
                .iter()
                .chain(self.canonical_terms.iter())
                .filter(|canonical| !canonical.is_empty() && canonical.is_ascii())
                .map(|canonical| (canonical.as_str(), canonical.as_str())),
        );
        replacements.sort_by(|left, right| {
            right
                .0
                .chars()
                .count()
                .cmp(&left.0.chars().count())
                .then_with(|| left.0.cmp(right.0))
        });

        replacements
            .into_iter()
            .fold(input.to_owned(), |text, (alias, canonical)| {
                if alias.is_ascii() {
                    replace_ascii_alias(&text, alias, canonical)
                } else {
                    text.replace(alias, canonical)
                }
            })
    }
}

fn build_prompt(items: &[String]) -> (Option<String>, bool) {
    const PREFIX: &str = "专有名词：";
    if items.is_empty() {
        return (None, false);
    }
    let mut prompt = PREFIX.to_owned();
    let mut included = 0_usize;
    for item in items {
        let separator = if included == 0 { "" } else { "，" };
        let suffix = "。";
        let candidate_chars = prompt.chars().count()
            + separator.chars().count()
            + item.chars().count()
            + suffix.chars().count();
        if candidate_chars > WHISPER_INITIAL_PROMPT_MAX_CHARS {
            break;
        }
        prompt.push_str(separator);
        prompt.push_str(item);
        included += 1;
    }
    if included == 0 {
        return (None, true);
    }
    prompt.push('。');
    (Some(prompt), included < items.len())
}

fn is_ascii_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn replace_ascii_alias(input: &str, alias: &str, canonical: &str) -> String {
    if alias.is_empty() {
        return input.to_owned();
    }
    let lower_input = input.to_ascii_lowercase();
    let lower_alias = alias.to_ascii_lowercase();
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0_usize;
    while let Some(relative) = lower_input[cursor..].find(&lower_alias) {
        let start = cursor + relative;
        let end = start + alias.len();
        let before_ok = start == 0 || !is_ascii_word_byte(input.as_bytes()[start - 1]);
        let after_ok = end == input.len() || !is_ascii_word_byte(input.as_bytes()[end]);
        if before_ok && after_ok {
            output.push_str(&input[cursor..start]);
            output.push_str(canonical);
            cursor = end;
        } else {
            let next = start + 1;
            output.push_str(&input[cursor..next]);
            cursor = next;
        }
    }
    output.push_str(&input[cursor..]);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting_context::{MeetingContextSource, SnapshotPerson, SnapshotTerm};
    use crate::whisper_engine::WhisperEngine;
    use chrono::Utc;
    use serde_json::json;
    use std::path::PathBuf;
    use std::time::Instant;

    fn snapshot() -> MeetingContextSnapshot {
        MeetingContextSnapshot {
            context_id: "ctx_recording_test".to_owned(),
            revision: 1,
            reason: "recording_start".to_owned(),
            captured_at: Utc::now(),
            source: MeetingContextSource {
                template_id: "license_station_weekly".to_owned(),
                template_version: 3,
                template_file_sha256: "a".repeat(64),
                profile_sha256: "b".repeat(64),
            },
            fixed_meeting_mechanism: None,
            people: vec![
                SnapshotPerson {
                    person_id: "person_rayson".to_owned(),
                    display_name: "Rayson".to_owned(),
                    aliases: vec!["瑞森".to_owned(), "Reason".to_owned(), "Risa".to_owned()],
                    department: None,
                    role: None,
                    attendance: AttendanceStatus::Attending,
                },
                SnapshotPerson {
                    person_id: "person_amu".to_owned(),
                    display_name: "Amu".to_owned(),
                    aliases: vec!["阿牧".to_owned()],
                    department: None,
                    role: None,
                    attendance: AttendanceStatus::Expected,
                },
                SnapshotPerson {
                    person_id: "person_yili".to_owned(),
                    display_name: "伊犁".to_owned(),
                    aliases: vec!["伊丽".to_owned()],
                    department: None,
                    role: None,
                    attendance: AttendanceStatus::Expected,
                },
            ],
            host_person_id: Some("person_rayson".to_owned()),
            terms: vec![
                SnapshotTerm {
                    term_id: "term_pwa".to_owned(),
                    canonical: "PWA".to_owned(),
                    aliases: vec!["P W A".to_owned()],
                    category: None,
                },
                SnapshotTerm {
                    term_id: "term_youtube".to_owned(),
                    canonical: "YouTube".to_owned(),
                    aliases: vec!["U2B".to_owned()],
                    category: None,
                },
            ],
            context_sha256: "c".repeat(64),
        }
    }

    #[test]
    fn explicit_aliases_are_normalized_without_guessing_unknown_words() {
        let context = RecognitionContext::from_snapshot(&snapshot());
        let normalized = context
            .normalize_transcript("瑞森和阿牧提到伊丽，Reason 同意；老学、小比、思域保持原样。");
        assert_eq!(
            normalized,
            "Rayson和Amu提到伊犁，Rayson 同意；老学、小比、思域保持原样。"
        );
    }

    #[test]
    fn ascii_aliases_require_word_boundaries_and_are_case_insensitive() {
        let context = RecognitionContext::from_snapshot(&snapshot());
        assert_eq!(
            context.normalize_transcript("reason REASON unreasonable Risa Risa2"),
            "Rayson Rayson unreasonable Rayson Risa2"
        );
    }

    #[test]
    fn canonical_ascii_entities_are_case_normalized_with_word_boundaries() {
        let context = RecognitionContext::from_snapshot(&snapshot());
        assert_eq!(
            context.normalize_transcript(
                "rayson RAYSON rayson2 youtube YOUTUBE u2b Youtube2 pwa PWA2"
            ),
            "Rayson Rayson rayson2 YouTube YouTube YouTube Youtube2 PWA PWA2"
        );
    }

    #[test]
    fn canonical_ascii_normalization_preserves_embedded_identifiers_and_is_idempotent() {
        let context = RecognitionContext::from_snapshot(&snapshot());
        let once = context.normalize_transcript(
            "myyoutube youtube_api youtube-ads （youtube）中文youtube中文 PWA_api",
        );
        assert_eq!(
            once,
            "myyoutube youtube_api YouTube-ads （YouTube）中文YouTube中文 PWA_api"
        );
        assert_eq!(context.normalize_transcript(&once), once);
    }

    #[test]
    fn prompt_is_stable_bounded_and_excludes_aliases() {
        let context = RecognitionContext::from_snapshot(&snapshot());
        let prompt = context.whisper_initial_prompt.as_deref().unwrap();
        assert!(prompt.starts_with("专有名词：Rayson"));
        assert!(prompt.contains("Amu"));
        assert!(prompt.contains("PWA"));
        assert!(!prompt.contains("瑞森"));
        assert!(!prompt.contains("Reason"));
        assert!(prompt.chars().count() <= WHISPER_INITIAL_PROMPT_MAX_CHARS);
        assert!(!context.live_prompt_enabled);
        assert_eq!(context, RecognitionContext::from_snapshot(&snapshot()));
    }

    #[test]
    fn term_only_prompt_is_bounded_and_excludes_people_and_aliases() {
        let context = RecognitionContext::from_snapshot(&snapshot());
        let (prompt, truncated) = context.term_only_whisper_prompt();
        let prompt = prompt.unwrap();
        assert!(prompt.contains("PWA"));
        assert!(prompt.contains("YouTube"));
        assert!(!prompt.contains("Rayson"));
        assert!(!prompt.contains("Amu"));
        assert!(!prompt.contains("瑞森"));
        assert!(!prompt.contains("U2B"));
        assert!(!truncated);
        assert!(prompt.chars().count() <= WHISPER_INITIAL_PROMPT_MAX_CHARS);
        assert_eq!(
            (Some(prompt), truncated),
            context.term_only_whisper_prompt()
        );
        assert!(!context.live_prompt_enabled);
    }

    #[test]
    fn prompt_truncates_on_whole_items_in_priority_order() {
        let items = (0..100)
            .map(|index| format!("candidate_{index:03}"))
            .collect::<Vec<_>>();
        let (prompt, truncated) = build_prompt(&items);
        let prompt = prompt.unwrap();
        assert!(truncated);
        assert!(prompt.chars().count() <= WHISPER_INITIAL_PROMPT_MAX_CHARS);
        assert!(prompt.contains("candidate_000"));
        assert!(!prompt.contains("candidate_099"));
        assert!(prompt.ends_with('。'));
    }

    #[test]
    fn diagnostics_never_expose_prompt_or_names() {
        let context = RecognitionContext::from_snapshot(&snapshot());
        let diagnostics = serde_json::to_string(&context.diagnostics()).unwrap();
        assert!(!diagnostics.contains("Rayson"));
        assert!(!diagnostics.contains("瑞森"));
        assert!(diagnostics.contains("prompt_sha256"));
        assert!(diagnostics.contains("context_id"));
    }

    fn contains_prompt_token(text: &str, token: &str) -> bool {
        if token.is_ascii() {
            let lower_text = text.to_ascii_lowercase();
            let lower_token = token.to_ascii_lowercase();
            let mut cursor = 0_usize;
            while let Some(relative) = lower_text[cursor..].find(&lower_token) {
                let start = cursor + relative;
                let end = start + token.len();
                let before_ok = start == 0 || !is_ascii_word_byte(text.as_bytes()[start - 1]);
                let after_ok = end == text.len() || !is_ascii_word_byte(text.as_bytes()[end]);
                if before_ok && after_ok {
                    return true;
                }
                cursor = start + 1;
            }
            false
        } else {
            text.contains(token)
        }
    }

    /// Heavy, explicit acceptance gate. It is ignored in ordinary test runs
    /// because it requires the installed Whisper model and ten prepared clips.
    #[tokio::test]
    #[ignore = "requires MEETILY_MC_R02_* real-audio acceptance inputs"]
    async fn mc_r02_real_audio_prompt_gate() {
        let manifest_path = PathBuf::from(
            std::env::var("MEETILY_MC_R02_MANIFEST").expect("MEETILY_MC_R02_MANIFEST is required"),
        );
        let template_path = PathBuf::from(
            std::env::var("MEETILY_MC_R02_TEMPLATE").expect("MEETILY_MC_R02_TEMPLATE is required"),
        );
        let models_dir = PathBuf::from(
            std::env::var("MEETILY_MC_R02_MODELS_DIR")
                .expect("MEETILY_MC_R02_MODELS_DIR is required"),
        );
        let output_path = PathBuf::from(
            std::env::var("MEETILY_MC_R02_REPORT").expect("MEETILY_MC_R02_REPORT is required"),
        );
        let model_name = std::env::var("MEETILY_MC_R02_MODEL")
            .unwrap_or_else(|_| "large-v3-turbo-q5_0".to_owned());

        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        let clips = manifest["clips"]
            .as_array()
            .expect("manifest clips must be an array");
        assert_eq!(
            clips.len(),
            10,
            "MC-R02 requires exactly ten prepared clips"
        );
        let template: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&template_path).unwrap()).unwrap();
        let profile = &template["extensions"]["meetily_meeting_context"];
        let canonical_names = profile["people"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|person| person["enabled"].as_bool().unwrap_or(true))
            .map(|person| person["display_name"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let canonical_terms = profile["terms"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|term| term["enabled"].as_bool().unwrap_or(true))
            .map(|term| term["canonical"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let prompt_items = canonical_names
            .iter()
            .chain(canonical_terms.iter())
            .cloned()
            .collect::<Vec<_>>();
        let (prompt, prompt_truncated) = build_prompt(&prompt_items);
        let prompt = prompt.expect("real template should produce a prompt");
        let prompt_sha256 = format!("{:x}", Sha256::digest(prompt.as_bytes()));

        let engine = WhisperEngine::new_with_models_dir(models_dir).unwrap();
        engine.discover_models().await.unwrap();
        engine.load_model(&model_name).await.unwrap();
        let mut clip_reports = Vec::new();
        let mut total_new_tokens = 0_usize;
        for clip in clips {
            let clip_path = PathBuf::from(clip["path"].as_str().unwrap());
            let bytes = std::fs::read(&clip_path).unwrap();
            assert_eq!(bytes.len() % 4, 0, "clip must contain f32le samples");
            let samples = bytes
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
                .collect::<Vec<_>>();

            let baseline_started = Instant::now();
            let (baseline_text, _, _) = engine
                .transcribe_audio_with_confidence_and_prompt(
                    samples.clone(),
                    Some("zh".to_owned()),
                    None,
                )
                .await
                .unwrap();
            let baseline_elapsed_ms = baseline_started.elapsed().as_millis();
            let prompted_started = Instant::now();
            let (prompted_text, _, _) = engine
                .transcribe_audio_with_confidence_and_prompt(
                    samples,
                    Some("zh".to_owned()),
                    Some(prompt.clone()),
                )
                .await
                .unwrap();
            let prompted_elapsed_ms = prompted_started.elapsed().as_millis();

            let new_prompt_tokens = prompt_items
                .iter()
                .filter(|token| {
                    contains_prompt_token(&prompted_text, token)
                        && !contains_prompt_token(&baseline_text, token)
                })
                .cloned()
                .collect::<Vec<_>>();
            total_new_tokens += new_prompt_tokens.len();
            clip_reports.push(json!({
                "clip_id": clip["clipId"],
                "sequence_id": clip["sequenceId"],
                "clip_sha256": clip["sha256"],
                "baseline_text": baseline_text,
                "prompted_text": prompted_text,
                "baseline_elapsed_ms": baseline_elapsed_ms,
                "prompted_elapsed_ms": prompted_elapsed_ms,
                "new_prompt_tokens": new_prompt_tokens,
                "passed": new_prompt_tokens.is_empty(),
            }));
        }

        let passed = total_new_tokens == 0;
        let report = json!({
            "stage": "C",
            "gate": "MC-R02 no prompted person or term hallucination",
            "audited_at": Utc::now(),
            "model": model_name,
            "manifest_path": manifest_path,
            "template_path": template_path,
            "clip_count": clips.len(),
            "prompt_sha256": prompt_sha256,
            "prompt_chars": prompt.chars().count(),
            "prompt_truncated": prompt_truncated,
            "live_prompt_enabled_in_product": LIVE_WHISPER_PROMPT_ENABLED,
            "new_prompt_token_count": total_new_tokens,
            "clips": clip_reports,
            "result": if passed { "PASS" } else { "FAIL" },
        });
        std::fs::create_dir_all(output_path.parent().unwrap()).unwrap();
        std::fs::write(
            &output_path,
            format!("{}\n", serde_json::to_string_pretty(&report).unwrap()),
        )
        .unwrap();
        assert!(passed, "MC-R02 failed; see {}", output_path.display());
    }
}
