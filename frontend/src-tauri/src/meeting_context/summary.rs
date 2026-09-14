use super::{
    canonical_json_sha256, normalize_and_validate_container, AttendanceStatus,
    MeetingContextContainer, MeetingContextSnapshot, SnapshotPerson,
};
use crate::summary::source_binding::{
    restore_supported_owner_and_time_fields, trace_owner_and_time_fields, SummaryFieldTrace,
    SummarySourceBindingError, SummaryTraceField, SummaryTraceStatus, TranscriptEvidenceBinding,
    TranscriptVersionSnapshot,
};
use chrono::{DateTime, Datelike, NaiveDate, SecondsFormat, Utc};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedMeetingPerson {
    pub person_id: String,
    pub display_name: String,
    #[serde(default)]
    pub department: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
}

impl From<&SnapshotPerson> for VerifiedMeetingPerson {
    fn from(person: &SnapshotPerson) -> Self {
        Self {
            person_id: person.person_id.clone(),
            display_name: person.display_name.clone(),
            department: person.department.clone(),
            role: person.role.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifiedMeetingFacts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meeting_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_meeting_mechanism: Option<String>,
    #[serde(default)]
    pub attending: Vec<VerifiedMeetingPerson>,
    #[serde(default)]
    pub absent: Vec<VerifiedMeetingPerson>,
    #[serde(default)]
    pub host: Option<VerifiedMeetingPerson>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecognitionDictionaryPerson {
    pub person_id: String,
    pub display_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecognitionDictionaryTerm {
    pub term_id: String,
    pub canonical: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecognitionDictionary {
    #[serde(default)]
    pub people: Vec<RecognitionDictionaryPerson>,
    #[serde(default)]
    pub terms: Vec<RecognitionDictionaryTerm>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryMeetingContext {
    pub context_id: String,
    pub context_sha256: String,
    pub verified_meeting_facts: VerifiedMeetingFacts,
    pub recognition_dictionary: RecognitionDictionary,
}

impl SummaryMeetingContext {
    pub fn sha256(&self) -> String {
        canonical_json_sha256(self)
    }

    /// Produces one data-only prompt block. The same block is injected into
    /// chunk, combine, and final-report prompts; callers must not rebuild a
    /// second interpretation of the meeting metadata.
    pub fn to_prompt_block(&self) -> Result<String, String> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|error| format!("SUMMARY_CONTEXT_SERIALIZATION_FAILED: {error}"))?;
        Ok(format!(
            r#"<meeting_context>
{json}
</meeting_context>

MEETING CONTEXT RULES:
- `verified_meeting_facts` is authoritative for meeting name, timestamps, explicit attendees, explicit absentees, and host.
- `recognition_dictionary` controls spelling only. A person listed only in that dictionary is not proven to have attended, spoken, owned an action, or held a role.
- Never infer a missing role, department, owner, deadline, or attendance state.
- A `null` fact is an explicit absence of verified data. For example, `host: null` means no host is verified; output "not mentioned" or omit the field instead of naming a person.
- Template examples are structure examples, not facts about this meeting.
- If the transcript conflicts with verified metadata facts, preserve the verified metadata fact and do not invent a compromise."#
        ))
    }

    pub fn normalize_known_aliases(&self, input: &str) -> String {
        let mut replacements = self
            .recognition_dictionary
            .people
            .iter()
            .flat_map(|person| {
                person
                    .aliases
                    .iter()
                    .map(move |alias| (alias.as_str(), person.display_name.as_str()))
            })
            .chain(self.recognition_dictionary.terms.iter().flat_map(|term| {
                term.aliases
                    .iter()
                    .map(move |alias| (alias.as_str(), term.canonical.as_str()))
            }))
            .collect::<Vec<_>>();
        replacements.extend(
            self.recognition_dictionary
                .people
                .iter()
                .map(|person| person.display_name.as_str())
                .chain(
                    self.recognition_dictionary
                        .terms
                        .iter()
                        .map(|term| term.canonical.as_str()),
                )
                .filter(|canonical| !canonical.is_empty() && canonical.is_ascii())
                .map(|canonical| (canonical, canonical)),
        );
        replacements.sort_by(|left, right| {
            right
                .0
                .chars()
                .count()
                .cmp(&left.0.chars().count())
                .then_with(|| left.0.cmp(right.0))
        });
        let normalized =
            replacements
                .into_iter()
                .fold(input.to_owned(), |text, (alias, canonical)| {
                    if alias.is_ascii() {
                        replace_ascii_alias(&text, alias, canonical)
                    } else {
                        text.replace(alias, canonical)
                    }
                });
        self.recognition_dictionary
            .people
            .iter()
            .fold(normalized, |text, person| {
                collapse_repeated_name_annotation(&text, &person.display_name)
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryFactValidationStatus {
    Passed,
    NeedsReview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryFactWarning {
    pub code: String,
    pub message_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryFactValidation {
    pub status: SummaryFactValidationStatus,
    pub warning_count: usize,
    pub warnings: Vec<SummaryFactWarning>,
    pub aliases_normalized: bool,
    pub meeting_context_id: Option<String>,
    pub meeting_context_sha256: Option<String>,
    pub summary_context_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub field_traces: Vec<SummaryFieldTrace>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_evidence: Option<TranscriptEvidenceBinding>,
}

impl SummaryFactValidation {
    /// Use only for a payload read from native storage, never an editor request.
    /// Rechecking sanitized text cannot rediscover omissions. Keep those notices,
    /// but not old status, evidence, or caller-supplied message keys.
    pub fn retain_saved_omission_warnings(&mut self, stored_summary: &serde_json::Value) {
        let Some(warnings) = stored_summary
            .pointer("/factValidation/warnings")
            .and_then(serde_json::Value::as_array)
        else {
            return;
        };
        for warning in warnings {
            match warning.get("code").and_then(serde_json::Value::as_str) {
                Some("unsupported_year") => push_fact_warning(
                    self, "unsupported_year", "summary:factValidation.unsupportedYear",
                ),
                Some("unsupported_acronym_expansion") => push_fact_warning(
                    self, "unsupported_acronym_expansion",
                    "summary:factValidation.unsupportedAcronymExpansion",
                ),
                _ => {}
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedSummaryMarkdown {
    pub markdown: String,
    pub validation: SummaryFactValidation,
}

pub fn validate_summary_markdown(
    markdown: &str,
    context: Option<&SummaryMeetingContext>,
) -> ValidatedSummaryMarkdown {
    let Some(context) = context else {
        let warning = fact_warning(
            "missing_meeting_context",
            "summary:factValidation.missingMeetingContext",
        );
        return ValidatedSummaryMarkdown {
            markdown: markdown.to_owned(),
            validation: SummaryFactValidation {
                status: SummaryFactValidationStatus::NeedsReview,
                warning_count: 1,
                warnings: vec![warning],
                aliases_normalized: false,
                meeting_context_id: None,
                meeting_context_sha256: None,
                summary_context_sha256: None,
                field_traces: Vec::new(),
                source_evidence: None,
            },
        };
    };

    let normalized = context.normalize_known_aliases(markdown);
    let aliases_normalized = normalized != markdown;
    let mut warning_codes = BTreeSet::new();
    let mut warnings = Vec::new();
    let mut add_warning = |code: &str, message_key: &str| {
        if warning_codes.insert(code.to_owned()) {
            warnings.push(fact_warning(code, message_key));
        }
    };

    if let Some(expected_date) = context
        .verified_meeting_facts
        .started_at
        .as_deref()
        .and_then(|value| value.get(0..10))
    {
        let expected_year = expected_date
            .get(0..4)
            .and_then(|year| year.parse::<i32>().ok())
            .unwrap_or_else(|| Utc::now().year());
        for line in normalized.lines().filter(|line| is_meeting_date_line(line)) {
            let date_value = structured_field_value(line, DATE_FIELD_LABELS).unwrap_or(line);
            if extract_declared_dates(date_value, expected_year)
                .iter()
                .any(|date| date != expected_date)
            {
                add_warning(
                    "meeting_date_mismatch",
                    "summary:factValidation.meetingDateMismatch",
                );
                break;
            }
        }
    }

    let attending_ids = context
        .verified_meeting_facts
        .attending
        .iter()
        .map(|person| person.person_id.as_str())
        .collect::<BTreeSet<_>>();
    let absent_ids = context
        .verified_meeting_facts
        .absent
        .iter()
        .map(|person| person.person_id.as_str())
        .collect::<BTreeSet<_>>();
    let host_id = context
        .verified_meeting_facts
        .host
        .as_ref()
        .map(|person| person.person_id.as_str());

    for line in normalized.lines() {
        if structured_field_value(line, ATTENDEE_FIELD_LABELS).is_some_and(|value| {
            context.recognition_dictionary.people.iter().any(|person| {
                contains_known_name(value, &person.display_name)
                    && !attending_ids.contains(person.person_id.as_str())
                    && host_id != Some(person.person_id.as_str())
            })
        }) {
            add_warning(
                "attendance_conflict",
                "summary:factValidation.attendanceConflict",
            );
        }
        if structured_field_value(line, ABSENCE_FIELD_LABELS).is_some_and(|value| {
            context.recognition_dictionary.people.iter().any(|person| {
                contains_known_name(value, &person.display_name)
                    && !absent_ids.contains(person.person_id.as_str())
            })
        }) {
            add_warning("absence_conflict", "summary:factValidation.absenceConflict");
        }
        if let Some(host_value) = structured_field_value(line, HOST_FIELD_LABELS) {
            match context.verified_meeting_facts.host.as_ref() {
                Some(expected_host) => {
                    let has_expected_host =
                        contains_known_name(host_value, &expected_host.display_name);
                    let has_other_known_person = context
                        .recognition_dictionary
                        .people
                        .iter()
                        .filter(|person| person.person_id != expected_host.person_id)
                        .any(|person| contains_known_name(host_value, &person.display_name));
                    if has_other_known_person && !has_expected_host {
                        add_warning("host_conflict", "summary:factValidation.hostConflict");
                    }
                }
                None if context
                    .recognition_dictionary
                    .people
                    .iter()
                    .any(|person| contains_known_name(host_value, &person.display_name)) =>
                {
                    add_warning("host_conflict", "summary:factValidation.hostConflict");
                }
                None => {}
            }
        }
    }

    let (person_safe_markdown, removed_hybrid_person_name) =
        replace_unverified_hybrid_person_names(&normalized, context);
    if removed_hybrid_person_name {
        add_warning(
            "person_name_conflict",
            "summary:factValidation.personNameConflict",
        );
    }

    let status = if warnings.is_empty() {
        SummaryFactValidationStatus::Passed
    } else {
        SummaryFactValidationStatus::NeedsReview
    };
    // MeetingContext currently has no authoritative action-owner/deadline schema.
    // A small local model can still fill those template columns despite prompt rules,
    // so omit the unsupported values deterministically instead of presenting guesses
    // as meeting facts. The task/decision prose remains available for manual editing.
    let safe_markdown = omit_unverified_high_risk_fields(&person_safe_markdown, context);
    ValidatedSummaryMarkdown {
        markdown: safe_markdown,
        validation: SummaryFactValidation {
            status,
            warning_count: warnings.len(),
            warnings,
            aliases_normalized,
            meeting_context_id: Some(context.context_id.clone()),
            meeting_context_sha256: Some(context.context_sha256.clone()),
            summary_context_sha256: Some(context.sha256()),
            field_traces: Vec::new(),
            source_evidence: None,
        },
    }
}

/// Adds transcript-grounding checks to the structured meeting-fact checks.
///
/// `validate_summary_markdown` intentionally validates only authoritative
/// meeting metadata. Summary generation, however, also has the source
/// transcript available. This wrapper uses that evidence to flag a small set
/// of high-risk claims that a local model must not silently introduce:
/// unknown acronyms, unsupported organization names in action tables, and
/// strong status claims that do not occur in the transcript.
///
/// Most checks are review-only: lexical grounding cannot prove entailment.
/// Explicit unsupported years and acronym annotations are masked in generated
/// output. Manual-save callers preserve the authored body and retain warnings.
pub fn validate_summary_markdown_with_transcript(
    markdown: &str,
    context: Option<&SummaryMeetingContext>,
    transcript: &str,
) -> ValidatedSummaryMarkdown {
    let mut validated = validate_summary_markdown(markdown, context);
    let transcript = transcript.trim();

    if transcript.is_empty() {
        push_fact_warning(
            &mut validated.validation,
            "missing_transcript_evidence",
            "summary:factValidation.missingTranscriptEvidence",
        );
        return validated;
    }

    // The recognition dictionary is a spelling aid, never transcript evidence.
    // Normalize aliases in the actual transcript first, then ground claims only
    // against that normalized source plus verified meeting facts.
    let normalized_transcript = context
        .map(|context| context.normalize_known_aliases(transcript))
        .unwrap_or_else(|| transcript.to_owned());
    let support_corpus = build_transcript_support_corpus(&normalized_transcript, context);
    // Ground the model/user-authored claims before the deterministic sanitizer
    // removes unsupported owner, deadline, role, or status cells. Otherwise a
    // false claim could disappear from the safe rendering and incorrectly
    // leave the validation status as `passed`.
    let grounding_markdown = context
        .map(|context| context.normalize_known_aliases(markdown))
        .unwrap_or_else(|| markdown.to_owned());
    if contains_unsupported_summary_acronym(&grounding_markdown, &support_corpus) {
        push_fact_warning(
            &mut validated.validation,
            "unsupported_transcript_term",
            "summary:factValidation.unsupportedTranscriptTerm",
        );
    }
    if contains_unsupported_organization_value(&grounding_markdown, &support_corpus) {
        push_fact_warning(
            &mut validated.validation,
            "unsupported_organization",
            "summary:factValidation.unsupportedOrganization",
        );
    }
    if contains_unsupported_status_claim(&grounding_markdown, &normalized_transcript, context) {
        push_fact_warning(
            &mut validated.validation,
            "unsupported_status_claim",
            "summary:factValidation.unsupportedStatusClaim",
        );
    }
    if contains_password_security_claim(&grounding_markdown)
        && !contains_password_security_claim(&normalized_transcript)
    {
        // A custom template may contain a security example such as "do not
        // request account passwords".  The example controls structure only;
        // when the transcript never states it, remove the entire generated
        // line instead of presenting the template example as a meeting fact.
        validated.markdown = remove_password_security_claim_lines(&validated.markdown);
        push_fact_warning(
            &mut validated.validation,
            "unsupported_security_claim",
            "summary:factValidation.unsupportedSecurityClaim",
        );
    }

    validated.markdown = omit_unsupported_summary_literals(
        &validated.markdown,
        &support_corpus,
        &mut validated.validation,
    );
    validated
}

fn canonical_digits(input: &str) -> String {
    input.chars().map(|character| {
        if character == '零' { return '0'; }
        "〇一二三四五六七八九".chars().position(|digit| digit == character)
            .and_then(|digit| char::from_digit(digit as u32, 10))
            .unwrap_or(character)
    }).collect()
}

fn omit_unsupported_summary_literals(
    markdown: &str,
    support_corpus: &str,
    validation: &mut SummaryFactValidation,
) -> String {
    static NUMBER: Lazy<Regex> = Lazy::new(|| Regex::new(r"[0-9〇零一二三四五六七八九]+").unwrap());
    static YEAR: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?P<year>[0-9〇零一二三四五六七八九]+)[ \t]*年").unwrap());
    static ANNOTATION: Lazy<Regex> = Lazy::new(|| Regex::new(
        r"(?P<id>[A-Z][A-Z0-9]{1,15})(?P<bold>\*\*)?[ \t]*[（(](?P<explanation>[^（）()\n]+)[）)]"
    ).unwrap());
    let supported_years = NUMBER.find_iter(support_corpus)
        .filter(|number| number.as_str().chars().count() == 4)
        .map(|number| canonical_digits(number.as_str())).collect::<BTreeSet<_>>();
    let normalized_support = normalize_evidence_text(support_corpus);
    let mut unsupported_year = false;
    let mut unsupported_expansion = false;
    let mut fence: Option<(char, usize)> = None;
    let mut safe = String::with_capacity(markdown.len());

    // ponytail: this is a literal gate, not a semantic or translation verifier.
    // Keep code intact; ambiguous prose still needs human/source-level review.
    for line in markdown.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let marker = trimmed.chars().next().unwrap_or(' ');
        let marker_count = trimmed.chars().take_while(|character| *character == marker).count();
        if matches!(marker, '`' | '~') && marker_count >= 3 {
            match fence {
                None => fence = Some((marker, marker_count)),
                Some((opening, count)) if marker == opening && marker_count >= count
                    && trimmed[marker_count..].trim().is_empty() => fence = None,
                _ => {}
            }
            safe.push_str(line);
            continue;
        }
        if fence.is_some() || line.contains('`') {
            safe.push_str(line);
            continue;
        }
        let without_years = YEAR.replace_all(line, |captures: &regex::Captures<'_>| {
            let year = canonical_digits(&captures["year"]);
            if year.len() == 4 && !supported_years.contains(&year) {
                unsupported_year = true;
                "[年份待核对]".to_owned()
            } else { captures[0].to_owned() }
        });
        safe.push_str(&ANNOTATION.replace_all(&without_years, |captures: &regex::Captures<'_>| {
            let start = captures.get(0).unwrap().start();
            let explanation = &captures["explanation"];
            // A single ASCII token can be a function argument, not a glossary.
            let is_explanation = !explanation.is_ascii() || explanation.split_whitespace().count() > 1;
            if (start > 0 && is_ascii_word_byte(without_years.as_bytes()[start - 1]))
                || !is_explanation
                || normalized_support.contains(&normalize_evidence_text(explanation)) {
                captures[0].to_owned()
            } else {
                unsupported_expansion = true;
                format!("{}{}", &captures["id"], captures.name("bold").map_or("", |value| value.as_str()))
            }
        }));
    }
    if unsupported_year {
        push_fact_warning(validation, "unsupported_year", "summary:factValidation.unsupportedYear");
    }
    if unsupported_expansion {
        push_fact_warning(validation, "unsupported_acronym_expansion", "summary:factValidation.unsupportedAcronymExpansion");
    }
    safe
}

/// P5 entry point for generated and manually revalidated summaries.
///
/// The plain-text validator remains available for legacy callers. This version
/// additionally requires an already validated active transcript version,
/// records source segment/timestamp references for owner and time fields, and
/// restores only fields that have exact evidence after the conservative legacy
/// sanitizer has masked every high-risk cell.
pub fn validate_summary_markdown_with_source(
    markdown: &str,
    context: Option<&SummaryMeetingContext>,
    source: &TranscriptVersionSnapshot,
) -> Result<ValidatedSummaryMarkdown, SummarySourceBindingError> {
    let transcript = source.render_for_summary()?;
    let mut validated = validate_summary_markdown_with_transcript(markdown, context, &transcript);
    let evidence_markdown = context
        .map(|context| {
            let normalized = context.normalize_known_aliases(markdown);
            replace_unverified_hybrid_person_names(&normalized, context).0
        })
        .unwrap_or_else(|| markdown.to_owned());
    let traces = trace_owner_and_time_fields(&evidence_markdown, source)?;

    if traces.iter().any(|trace| {
        trace.field == SummaryTraceField::Owner && trace.status == SummaryTraceStatus::NeedsReview
    }) {
        push_fact_warning(
            &mut validated.validation,
            "untraceable_action_owner",
            "summary:factValidation.untraceableActionOwner",
        );
    }
    if traces.iter().any(|trace| {
        trace.field == SummaryTraceField::Time && trace.status == SummaryTraceStatus::NeedsReview
    }) {
        push_fact_warning(
            &mut validated.validation,
            "untraceable_action_time",
            "summary:factValidation.untraceableActionTime",
        );
    }
    if traces.iter().any(|trace| {
        trace.field == SummaryTraceField::Dependency && trace.status == SummaryTraceStatus::NeedsReview
    }) {
        push_fact_warning(
            &mut validated.validation,
            "untraceable_action_dependency",
            "summary:factValidation.untraceableActionDependency",
        );
    }
    validated.markdown =
        restore_supported_owner_and_time_fields(&evidence_markdown, &validated.markdown, &traces);
    validated.validation.field_traces = traces;
    validated.validation.source_evidence = Some(source.evidence_binding()?);
    Ok(validated)
}

fn push_fact_warning(validation: &mut SummaryFactValidation, code: &str, message_key: &str) {
    if validation
        .warnings
        .iter()
        .any(|warning| warning.code == code)
    {
        return;
    }
    validation.warnings.push(fact_warning(code, message_key));
    validation.warning_count = validation.warnings.len();
    validation.status = SummaryFactValidationStatus::NeedsReview;
}

fn build_transcript_support_corpus(
    transcript: &str,
    context: Option<&SummaryMeetingContext>,
) -> String {
    let mut parts = vec![transcript.to_owned()];
    let Some(context) = context else {
        return parts.join("\n");
    };

    let facts = &context.verified_meeting_facts;
    parts.extend(
        [
            facts.meeting_name.as_deref(),
            facts.started_at.as_deref(),
            facts.completed_at.as_deref(),
            facts.fixed_meeting_mechanism.as_deref(),
        ]
        .into_iter()
        .flatten()
        .map(str::to_owned),
    );
    for person in facts
        .attending
        .iter()
        .chain(facts.absent.iter())
        .chain(facts.host.iter())
    {
        parts.push(person.display_name.clone());
        parts.extend(person.department.iter().cloned());
        parts.extend(person.role.iter().cloned());
    }
    parts.join("\n")
}

fn contains_password_security_claim(text: &str) -> bool {
    let normalized = text.to_ascii_lowercase();
    [
        "账号密码",
        "账户密码",
        "帐户密码",
        "登录密码",
        "用戶密碼",
        "用户密码",
        "account password",
        "login password",
        "password",
    ]
    .iter()
    .any(|term| normalized.contains(term))
}

fn remove_password_security_claim_lines(markdown: &str) -> String {
    let preserved_trailing_newline = markdown.ends_with('\n');
    let mut safe = markdown
        .lines()
        .filter(|line| !contains_password_security_claim(line))
        .collect::<Vec<_>>()
        .join("\n");
    if preserved_trailing_newline && !safe.is_empty() {
        safe.push('\n');
    }
    safe
}

/// Removes meeting-identity facts that an automatically generated report has
/// no authoritative context to support. This intentionally applies only to
/// generated output; a human can still edit an old meeting manually and the
/// normal validation path will mark it for review rather than erase it.
///
/// A transcript may mention names or dates, but without the recording-time
/// snapshot we cannot establish that they are the meeting title, official
/// attendees, host, or scheduled time. In that case showing a model guess as
/// a completed meeting-info field is less safe than an explicit placeholder.
pub fn sanitize_generated_summary_without_meeting_context(markdown: &str) -> String {
    let use_chinese_placeholder = markdown.chars().any(|character| {
        matches!(character, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}')
    });
    let placeholder = if use_chinese_placeholder {
        "会议未提及"
    } else {
        "Not mentioned"
    };
    let generic_title = if use_chinese_placeholder {
        "# 会议纪要（待核对）"
    } else {
        "# Meeting Summary (Facts Need Review)"
    };

    let mut title_replaced = false;
    markdown
        .lines()
        .map(|line| {
            if !title_replaced && line.trim_start().starts_with("# ") {
                title_replaced = true;
                return generic_title.to_owned();
            }
            sanitize_unverified_generated_meeting_identity_line(line, placeholder)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Legacy identity cleanup must not erase explicitly stated meeting facts.
/// Missing calendar metadata is not evidence that a fact was absent from the recording.
pub fn sanitize_generated_summary_with_transcript(markdown: &str, transcript: &str) -> String {
    static QUOTABLE_LINE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)^\s*(?:(?:[-*+])|(?:\d+[.)]))?\s*(?:\*\*)?(?:会议名称|会议主题|会议日期|会议时间|meeting name|meeting title|meeting topic|meeting date|meeting time)(?:\*\*)?\s*[:：]")
            .expect("generated meeting identity field regex must compile")
    });
    static HOST_LINE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)^(?P<prefix>\s*(?:(?:[-*+])|(?:\d+[.)]))?\s*(?:\*\*)?(?:主持人|host)(?:\*\*)?\s*[:：]\s*)(?P<value>.+)$")
            .expect("generated host field regex must compile")
    });
    let sanitized = sanitize_generated_summary_without_meeting_context(markdown);
    markdown.lines().zip(sanitized.lines()).map(|(original, safe)| {
        const QUOTABLE_FIELDS: &[&str] = &["会议名称", "会议主题", "会议日期", "会议时间", "meeting name", "meeting title", "meeting topic", "meeting date", "meeting time"];
        if let Some(value) = structured_field_value(original, QUOTABLE_FIELDS) {
            if QUOTABLE_LINE.is_match(original) && original.trim_end().ends_with(value) && transcript_quotes_current_meeting_identity(transcript, value) {
                return original.to_owned();
            }
        }
        let Some(host) = HOST_LINE.captures(original) else { return safe.to_owned(); };
        let value = host["value"].trim_matches(|c: char| c.is_whitespace() || matches!(c, '*' | ':' | '：'));
        let name = value.split(['(', '（', ',', '，']).next().unwrap_or(value).trim();
        if transcript_explicitly_names_host(transcript, name) {
            // Hosting evidence does not establish an appended role or alias.
            // Retain the supported name, without promoting the annotation to a fact.
            format!("{}{name}", &host["prefix"])
        } else { safe.to_owned() }
    }).collect::<Vec<_>>().join("\n")
}

// Quote matching is deliberately conservative: a discussed date/title, a recording
// timestamp, or a paraphrase is not proof of the current meeting's identity.
fn transcript_quotes_current_meeting_identity(transcript: &str, value: &str) -> bool {
    let parts: Vec<_> = value.split(['，', '；', ',', ';']).map(normalize_evidence_text)
        .filter(|part| !part.is_empty()).collect();
    if parts.is_empty() || parts.iter().any(|part| part.chars().count() < 2 || is_review_placeholder(part)) {
        return false;
    }
    transcript.split(['。', '\n', '!', '！', '?', '？']).any(|sentence| {
        let lower = sentence.to_lowercase();
        let current = ["现在", "今天", "本次会议", "这次会议", "this meeting", "today"]
            .iter().any(|term| lower.contains(term));
        let declaration = ["召开", "举行", "会议日期", "会议时间", "会议主题", "会议名称", "meeting", "今天是", "现在是"]
            .iter().any(|term| lower.contains(term));
        let negated_or_other = ["不是", "并非", "是否", "能否", "昨天", "上次", "明天", "下次", "上周", "下周", "截止", "交付", "讨论", "yesterday", "previous", "tomorrow", "next meeting", "deadline", "discuss", "not "]
            .iter().any(|term| lower.contains(term));
        let support = normalize_evidence_text(sentence);
        current && declaration && !negated_or_other && parts.iter().all(|part| support.contains(part))
    })
}

fn transcript_explicitly_names_host(transcript: &str, host: &str) -> bool {
    let host = host.trim_matches(|c: char| c.is_whitespace() || matches!(c, '*' | ':' | '：'));
    if host.is_empty() || is_review_placeholder(&normalize_evidence_text(host)) { return false; }
    let name = regex::escape(host);
    let declaration = Regex::new(&format!(
        r"(?i)(?:由\s*{name}\s*(?:来)?主持|{name}\s*(?:[,，]\s*)?(?:今天|本次|这次)(?:由我)?主持|(?:host|chair)\s*(?:is|:)\s*{name}|{name}\s+(?:is (?:the )?(?:host|chair)|will (?:host|chair)))"
    )).expect("escaped host declaration must compile");
    transcript.split(['。', '\n', '!', '！', '?', '？']).any(|sentence| {
        let lower = sentence.to_lowercase();
        !["不是", "并非", "是否", "能否", "昨天", "上次", "yesterday", "previous", "not the host"]
            .iter().any(|term| lower.contains(term))
            && declaration.is_match(sentence)
    })
}

fn sanitize_unverified_generated_meeting_identity_line(line: &str, placeholder: &str) -> String {
    static CHINESE_MEETING_IDENTITY_FIELD: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"^(?P<prefix>\s*(?:(?:[-*+])|(?:\d+[.)]))?\s*(?:\*\*)?(?:会议名称|会议主题|会议时间|会议日期|参会人员|出席人员|缺席人员|主持人)(?:\*\*)?\s*[:：]\s*).*$",
        )
        .expect("Chinese generated meeting identity regex must compile")
    });
    static ENGLISH_MEETING_IDENTITY_FIELD: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?i)^(?P<prefix>\s*(?:(?:[-*+])|(?:\d+[.)]))?\s*(?:\*\*)?(?:meeting name|meeting title|meeting topic|meeting time|meeting date|attendees|participants|absent|host)(?:\*\*)?\s*:\s*).*$",
        )
        .expect("English generated meeting identity regex must compile")
    });

    if let Some(captures) = CHINESE_MEETING_IDENTITY_FIELD.captures(line) {
        return format!("{}{}", &captures["prefix"], placeholder);
    }
    if let Some(captures) = ENGLISH_MEETING_IDENTITY_FIELD.captures(line) {
        return format!("{}{}", &captures["prefix"], placeholder);
    }
    line.to_owned()
}

fn contains_unsupported_summary_acronym(markdown: &str, support_corpus: &str) -> bool {
    let supported = extract_uppercase_identifiers(support_corpus);
    extract_uppercase_identifiers(markdown)
        .into_iter()
        .any(|identifier| !supported.contains(&identifier))
}

fn extract_uppercase_identifiers(input: &str) -> BTreeSet<String> {
    static IDENTIFIER: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"[A-Z][A-Z0-9]{1,15}").expect("uppercase identifier regex must compile")
    });
    IDENTIFIER
        .find_iter(input)
        .filter(|matched| {
            let start = matched.start();
            let end = matched.end();
            let before_ok = start == 0 || !is_ascii_word_byte(input.as_bytes()[start - 1]);
            let after_ok = end == input.len() || !is_ascii_word_byte(input.as_bytes()[end]);
            before_ok && after_ok
        })
        .map(|matched| matched.as_str().to_owned())
        .collect()
}

fn contains_unsupported_organization_value(markdown: &str, support_corpus: &str) -> bool {
    let normalized_support = normalize_evidence_text(support_corpus);
    let mut organization_columns: Option<Vec<usize>> = None;

    for line in markdown.lines() {
        if !line.trim_start().starts_with('|') {
            organization_columns = None;
            continue;
        }
        let cells = parse_markdown_table_cells(line);
        if cells.is_empty() {
            organization_columns = None;
            continue;
        }
        if organization_columns.is_none() {
            let columns = cells
                .iter()
                .enumerate()
                .filter_map(|(index, header)| is_organization_table_header(header).then_some(index))
                .collect::<Vec<_>>();
            if !columns.is_empty() {
                organization_columns = Some(columns);
            }
            continue;
        }
        if is_markdown_table_separator(&cells) {
            continue;
        }
        if let Some(columns) = organization_columns.as_ref() {
            for index in columns {
                let Some(value) = cells.get(*index) else {
                    continue;
                };
                let normalized_value = normalize_evidence_text(value);
                if !normalized_value.is_empty()
                    && !is_review_placeholder(&normalized_value)
                    && !normalized_support.contains(&normalized_value)
                {
                    return true;
                }
            }
        }
    }

    false
}

fn is_organization_table_header(header: &str) -> bool {
    let normalized = header.trim().trim_matches('*').trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "部门/小组"
            | "部门／小组"
            | "部门"
            | "小组"
            | "团队"
            | "业务线"
            | "department/group"
            | "department / group"
            | "department"
            | "group"
            | "team"
            | "business unit"
    )
}

fn is_review_placeholder(normalized: &str) -> bool {
    matches!(
        normalized,
        "会议未提及" | "未提及" | "待确认" | "无" | "none" | "notmentioned" | "tobeconfirmed"
    )
}

#[derive(Debug, Clone, Copy)]
struct StatusClaimGroup {
    phrases: &'static [&'static str],
}

const HIGH_RISK_STATUS_CLAIM_GROUPS: &[StatusClaimGroup] = &[
    StatusClaimGroup {
        phrases: &[
            "已完成",
            "已经完成",
            "完成了",
            "完成完毕",
            "completed",
            "finished",
            "done",
        ],
    },
    StatusClaimGroup {
        phrases: &[
            "未完成",
            "尚未完成",
            "没有完成",
            "并未完成",
            "not completed",
            "not finished",
            "unfinished",
            "incomplete",
        ],
    },
    StatusClaimGroup {
        phrases: &["已上线", "已经上线", "正式上线", "launched", "went live"],
    },
    StatusClaimGroup {
        phrases: &["未上线", "尚未上线", "没有上线", "not launched", "not live"],
    },
    StatusClaimGroup {
        phrases: &[
            "已停止",
            "已经停止",
            "停止了",
            "已终止",
            "正式终止",
            "stopped",
            "terminated",
            "discontinued",
        ],
    },
    StatusClaimGroup {
        phrases: &["已暂停", "已经暂停", "暂停了", "paused", "on hold"],
    },
    StatusClaimGroup {
        phrases: &["已取消", "已经取消", "取消了", "cancelled", "canceled"],
    },
    StatusClaimGroup {
        phrases: &["已被放弃", "已放弃", "已经放弃", "abandoned", "dropped"],
    },
    StatusClaimGroup {
        phrases: &["未达成", "没有达成", "not achieved", "missed target"],
    },
    StatusClaimGroup {
        phrases: &["已达成", "已经达成", "达成了", "achieved", "met target"],
    },
    StatusClaimGroup {
        phrases: &["已确认", "已经确认", "确认了", "confirmed"],
    },
    StatusClaimGroup {
        phrases: &["已决定", "已经决定", "决定了", "decided"],
    },
    StatusClaimGroup {
        phrases: &["进行中", "正在进行", "推进中", "in progress", "underway"],
    },
    StatusClaimGroup {
        phrases: &["已启动", "已经启动", "启动了", "started", "initiated"],
    },
    StatusClaimGroup {
        phrases: &["必须", "must", "required"],
    },
    StatusClaimGroup {
        phrases: &[
            "禁止",
            "不得",
            "严禁",
            "prohibited",
            "forbidden",
            "must not",
        ],
    },
];

fn contains_unsupported_status_claim(
    markdown: &str,
    transcript: &str,
    context: Option<&SummaryMeetingContext>,
) -> bool {
    let anchor_groups = status_anchor_groups(context);
    let summary_units = status_fact_units(markdown, &anchor_groups);
    let transcript_units = status_fact_units(transcript, &anchor_groups);

    summary_units
        .iter()
        .filter(|unit| !is_status_section_heading(unit))
        .any(|summary_unit| {
            HIGH_RISK_STATUS_CLAIM_GROUPS.iter().any(|claim_group| {
                if !contains_status_claim(summary_unit, claim_group) {
                    return false;
                }

                let supporting_units = transcript_units
                    .iter()
                    .filter(|unit| contains_status_claim(unit, claim_group))
                    .collect::<Vec<_>>();
                if supporting_units.is_empty() {
                    return true;
                }

                let normalized_summary = normalize_evidence_text(summary_unit);
                let claimed_anchors = anchor_groups
                    .iter()
                    .filter(|variants| {
                        variants
                            .iter()
                            .any(|variant| fact_unit_contains_anchor(summary_unit, variant))
                    })
                    .collect::<Vec<_>>();
                let claimed_identifiers = extract_status_identifiers(summary_unit)
                    .into_iter()
                    // A canonical term such as `YouTube` can be supported by a
                    // configured transcript alias such as `U2B`. Once the term
                    // is covered by an anchor group, do not require the
                    // canonical spelling a second time as a raw identifier.
                    .filter(|identifier| {
                        !claimed_anchors.iter().any(|variants| {
                            variants.iter().any(|variant| {
                                variant == identifier
                                    || variant.contains(identifier)
                                    || identifier.contains(variant)
                            })
                        })
                    })
                    .collect::<BTreeSet<_>>();

                if claimed_anchors.is_empty() && claimed_identifiers.is_empty() {
                    // With no configured anchor or distinctive identifier, the
                    // business-object portion (after removing equivalent status
                    // wording) must match. A shorter transcript sentence must
                    // never bless a summary that added a new object prefix.
                    let summary_subject = status_subject_fingerprint(summary_unit, claim_group);
                    return !supporting_units.iter().any(|support| {
                        let support_subject = status_subject_fingerprint(support, claim_group);
                        !summary_subject.is_empty() && support_subject == summary_subject
                    });
                }

                let anchor_is_supported = |variants: &&Vec<String>| {
                    supporting_units.iter().any(|support| {
                        variants
                            .iter()
                            .any(|variant| fact_unit_contains_anchor(support, variant))
                    })
                };
                let identifier_is_supported = |identifier: &String| {
                    supporting_units
                        .iter()
                        .any(|support| extract_status_identifiers(support).contains(identifier))
                };

                claimed_anchors
                    .iter()
                    .any(|anchor| !anchor_is_supported(anchor))
                    || claimed_identifiers
                        .iter()
                        .any(|identifier| !identifier_is_supported(identifier))
            })
        })
}

fn status_fact_units(input: &str, anchor_groups: &[Vec<String>]) -> Vec<String> {
    input
        .lines()
        .flat_map(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with('|') {
                return vec![trimmed.to_owned()];
            }
            trimmed
                .split(['。', '.', '！', '？', '!', '?', '；', ';'])
                .flat_map(|sentence| {
                    let clauses = sentence
                        .split(['，', ','])
                        .map(str::trim)
                        .filter(|unit| !unit.is_empty())
                        .collect::<Vec<_>>();
                    clauses
                        .iter()
                        .enumerate()
                        .map(|(index, clause)| {
                            // Keep an omitted subject attached to its status:
                            // "对于 PWA，已完成测试" becomes one fact unit.
                            // Clauses that already name their subject remain
                            // separate, so "Google 已完成，PWA 观察中" cannot
                            // lend Google's status to PWA.
                            if index > 0
                                && fact_unit_starts_with_status_claim(clause)
                                && !fact_unit_has_own_post_status_subject(clause, anchor_groups)
                            {
                                format!("{}，{}", clauses[index - 1], clause)
                            } else {
                                (*clause).to_owned()
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn status_subject_fingerprint(unit: &str, group: &StatusClaimGroup) -> String {
    let mut subject = normalize_evidence_text(unit);
    let mut phrases = group
        .phrases
        .iter()
        .map(|phrase| normalize_evidence_text(phrase))
        .filter(|phrase| !phrase.is_empty())
        .collect::<Vec<_>>();
    phrases.sort_by_key(|phrase| std::cmp::Reverse(phrase.chars().count()));
    for phrase in phrases {
        subject = subject.replace(&phrase, "");
    }
    subject
}

fn fact_unit_has_own_post_status_subject(unit: &str, anchor_groups: &[Vec<String>]) -> bool {
    let normalized = normalize_evidence_text(unit);
    ["的是", "由", "负责人是", "责任人是", "归属于", "归属"]
        .iter()
        .any(|marker| normalized.contains(marker))
        || !extract_status_identifiers(unit).is_empty()
        || anchor_groups
            .iter()
            .flatten()
            .any(|variant| fact_unit_contains_anchor(unit, variant))
}

fn fact_unit_starts_with_status_claim(unit: &str) -> bool {
    let stripped = unit
        .trim_start()
        .trim_start_matches(|character: char| matches!(character, '-' | '*' | '•' | ':' | '：'))
        .trim_start();
    let normalized = normalize_evidence_text(stripped);
    HIGH_RISK_STATUS_CLAIM_GROUPS.iter().any(|group| {
        group.phrases.iter().any(|phrase| {
            let normalized_phrase = normalize_evidence_text(phrase);
            !normalized_phrase.is_empty() && normalized.starts_with(&normalized_phrase)
        })
    })
}

fn contains_status_claim(unit: &str, group: &StatusClaimGroup) -> bool {
    group
        .phrases
        .iter()
        .any(|phrase| contains_status_phrase(unit, phrase))
}

fn contains_status_phrase(input: &str, phrase: &str) -> bool {
    if phrase.is_ascii() {
        let input = normalize_ascii_status_contractions(input);
        let phrase = phrase.to_ascii_lowercase();
        return input.match_indices(&phrase).any(|(start, _)| {
            let end = start + phrase.len();
            let before_ok = start == 0 || !is_ascii_word_byte(input.as_bytes()[start - 1]);
            let after_ok = end == input.len() || !is_ascii_word_byte(input.as_bytes()[end]);
            before_ok
                && after_ok
                && (is_explicit_negative_status_phrase(&phrase)
                    || !ascii_status_match_is_negated(&input, start, end, &phrase))
        });
    }

    normalize_evidence_text(input).contains(&normalize_evidence_text(phrase))
}

fn normalize_ascii_status_contractions(input: &str) -> String {
    let mut normalized = input.to_ascii_lowercase().replace('’', "'");
    for (contraction, expanded) in [
        ("isn't", "is not"),
        ("wasn't", "was not"),
        ("aren't", "are not"),
        ("weren't", "were not"),
        ("hasn't", "has not"),
        ("haven't", "have not"),
        ("hadn't", "had not"),
        ("doesn't", "does not"),
        ("don't", "do not"),
        ("didn't", "did not"),
        ("can't", "can not"),
        ("cannot", "can not"),
    ] {
        normalized = normalized.replace(contraction, expanded);
    }
    normalized
}

fn is_explicit_negative_status_phrase(phrase: &str) -> bool {
    let phrase = phrase.trim().to_ascii_lowercase();
    phrase.starts_with("not ")
        || phrase.starts_with("no ")
        || phrase.starts_with("never ")
        || matches!(phrase.as_str(), "unfinished" | "incomplete" | "must not")
}

fn ascii_status_match_is_negated(input: &str, start: usize, end: usize, phrase: &str) -> bool {
    let before =
        input[..start].trim_end_matches(|character: char| !character.is_ascii_alphanumeric());
    let preceding_tokens = before
        .rsplit(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .take(3)
        .collect::<Vec<_>>();
    if preceding_tokens
        .iter()
        .any(|token| matches!(*token, "not" | "no" | "never" | "without"))
    {
        return true;
    }

    // `must not` contains the positive token `must`; do not let that support a
    // positive requirement claim while it is also matched by the prohibition
    // group.
    if phrase == "must" {
        let following_token = input[end..]
            .trim_start_matches(|character: char| !character.is_ascii_alphanumeric())
            .split(|character: char| !character.is_ascii_alphanumeric())
            .find(|token| !token.is_empty());
        return following_token == Some("not");
    }

    false
}

fn fact_unit_contains_anchor(unit: &str, normalized_variant: &str) -> bool {
    if normalized_variant.is_empty() {
        return false;
    }
    if normalized_variant
        .chars()
        .all(|character| character.is_ascii_alphanumeric())
    {
        static ASCII_TOKEN: Lazy<Regex> = Lazy::new(|| {
            Regex::new(r"[A-Za-z0-9]+").expect("status ASCII anchor token regex must compile")
        });
        let tokens = ASCII_TOKEN
            .find_iter(unit)
            .map(|matched| normalize_evidence_text(matched.as_str()))
            .filter(|token| !token.is_empty())
            .collect::<Vec<_>>();
        for start in 0..tokens.len() {
            let mut combined = String::new();
            for token in tokens.iter().skip(start).take(6) {
                combined.push_str(token);
                if combined == normalized_variant {
                    return true;
                }
                if combined.len() >= normalized_variant.len() {
                    break;
                }
            }
        }
        return false;
    }

    normalize_evidence_text(unit).contains(normalized_variant)
}

fn status_anchor_groups(context: Option<&SummaryMeetingContext>) -> Vec<Vec<String>> {
    let Some(context) = context else {
        return Vec::new();
    };
    let mut groups = Vec::new();
    let mut add_group = |values: Vec<&str>| {
        let variants = values
            .into_iter()
            .map(normalize_evidence_text)
            .filter(|value| value.chars().count() >= 2)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if !variants.is_empty() && !groups.contains(&variants) {
            groups.push(variants);
        }
    };

    for person in &context.recognition_dictionary.people {
        add_group(
            std::iter::once(person.display_name.as_str())
                .chain(person.aliases.iter().map(String::as_str))
                .collect(),
        );
    }
    for term in &context.recognition_dictionary.terms {
        add_group(
            std::iter::once(term.canonical.as_str())
                .chain(term.aliases.iter().map(String::as_str))
                .collect(),
        );
    }
    for person in context
        .verified_meeting_facts
        .attending
        .iter()
        .chain(context.verified_meeting_facts.absent.iter())
        .chain(context.verified_meeting_facts.host.iter())
    {
        if let Some(department) = person.department.as_deref() {
            add_group(vec![department]);
        }
        if let Some(role) = person.role.as_deref() {
            add_group(vec![role]);
        }
    }
    if let Some(meeting_name) = context.verified_meeting_facts.meeting_name.as_deref() {
        add_group(vec![meeting_name]);
    }
    groups
}

fn extract_status_identifiers(input: &str) -> BTreeSet<String> {
    static IDENTIFIER: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"[A-Za-z][A-Za-z0-9._-]{1,31}").expect("status identifier regex must compile")
    });
    const STOP_WORDS: &[&str] = &[
        "a",
        "an",
        "and",
        "are",
        "as",
        "at",
        "be",
        "been",
        "by",
        "completed",
        "decided",
        "done",
        "finished",
        "for",
        "from",
        "has",
        "have",
        "in",
        "is",
        "it",
        "must",
        "not",
        "of",
        "on",
        "or",
        "our",
        "paused",
        "required",
        "started",
        "stopped",
        "team",
        "the",
        "to",
        "was",
        "were",
        "will",
        "with",
    ];

    IDENTIFIER
        .find_iter(input)
        .filter_map(|matched| {
            let raw = matched.as_str();
            let normalized = normalize_evidence_text(raw);
            let lower = raw.to_ascii_lowercase();
            let is_distinctive = raw.chars().any(|character| character.is_ascii_digit())
                || raw.chars().all(|character| !character.is_ascii_lowercase())
                || raw
                    .chars()
                    .skip(1)
                    .any(|character| character.is_ascii_uppercase())
                || raw
                    .chars()
                    .next()
                    .is_some_and(|first| first.is_ascii_uppercase());
            (is_distinctive && !STOP_WORDS.contains(&lower.as_str())).then_some(normalized)
        })
        .collect()
}

fn is_status_section_heading(unit: &str) -> bool {
    let stripped = unit
        .trim()
        .trim_start_matches('#')
        .trim()
        .trim_matches('*')
        .trim_matches('_')
        .trim();
    matches!(
        normalize_evidence_text(stripped).as_str(),
        "已完成事项"
            | "已完成工作"
            | "进行中事项"
            | "已停止事项"
            | "已暂停事项"
            | "已取消事项"
            | "已达成目标"
            | "未达成目标"
            | "completeditems"
            | "completedwork"
            | "inprogressitems"
            | "pauseditems"
            | "cancelleditems"
            | "canceleditems"
    )
}

fn normalize_evidence_text(input: &str) -> String {
    input
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn fact_warning(code: &str, message_key: &str) -> SummaryFactWarning {
    SummaryFactWarning {
        code: code.to_owned(),
        message_key: message_key.to_owned(),
    }
}

fn is_meeting_date_line(line: &str) -> bool {
    structured_field_value(line, DATE_FIELD_LABELS).is_some()
}

const DATE_FIELD_LABELS: &[&str] = &["会议日期", "会议时间", "meeting date", "meeting time"];
const ATTENDEE_FIELD_LABELS: &[&str] = &["参会人员", "出席人员", "attendees", "participants"];
const ABSENCE_FIELD_LABELS: &[&str] = &["缺席人员", "absent"];
const HOST_FIELD_LABELS: &[&str] = &["主持人", "host:", "host："];
const ALL_FACT_FIELD_LABELS: &[&str] = &[
    "会议日期",
    "会议时间",
    "meeting date",
    "meeting time",
    "参会人员",
    "出席人员",
    "attendees",
    "participants",
    "缺席人员",
    "absent",
    "主持人",
    "host:",
    "host：",
];

fn structured_field_value<'a>(line: &'a str, labels: &[&str]) -> Option<&'a str> {
    let lower = line.to_ascii_lowercase();
    let (field_start, label_len) = labels
        .iter()
        .filter_map(|label| {
            let normalized_label = label.to_ascii_lowercase();
            lower
                .find(&normalized_label)
                .map(|position| (position, label.len()))
        })
        .min_by_key(|(position, _)| *position)?;
    let value_start = field_start + label_len;
    let value = &line[value_start..];
    let lower_value = value.to_ascii_lowercase();
    let value_end = ALL_FACT_FIELD_LABELS
        .iter()
        .filter_map(|label| lower_value.find(&label.to_ascii_lowercase()))
        .min()
        .unwrap_or(value.len());
    Some(value[..value_end].trim())
}

fn extract_declared_dates(line: &str, default_year: i32) -> BTreeSet<String> {
    static FULL_DATE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?P<year>\d{4})\s*(?:年|[-/.])\s*(?P<month>\d{1,2})\s*(?:月|[-/.])\s*(?P<day>\d{1,2})\s*日?",
        )
        .expect("full meeting date regex must compile")
    });
    static CHINESE_MONTH_DAY: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?P<month>\d{1,2})\s*月\s*(?P<day>\d{1,2})\s*日")
            .expect("Chinese meeting month/day regex must compile")
    });

    let mut dates = BTreeSet::new();
    for captures in FULL_DATE.captures_iter(line) {
        let parsed = captures
            .name("year")
            .and_then(|value| value.as_str().parse::<i32>().ok())
            .zip(
                captures
                    .name("month")
                    .and_then(|value| value.as_str().parse::<u32>().ok()),
            )
            .zip(
                captures
                    .name("day")
                    .and_then(|value| value.as_str().parse::<u32>().ok()),
            )
            .and_then(|((year, month), day)| NaiveDate::from_ymd_opt(year, month, day));
        if let Some(date) = parsed {
            dates.insert(date.format("%Y-%m-%d").to_string());
        }
    }
    for captures in CHINESE_MONTH_DAY.captures_iter(line) {
        let parsed = captures
            .name("month")
            .and_then(|value| value.as_str().parse::<u32>().ok())
            .zip(
                captures
                    .name("day")
                    .and_then(|value| value.as_str().parse::<u32>().ok()),
            )
            .and_then(|(month, day)| NaiveDate::from_ymd_opt(default_year, month, day));
        if let Some(date) = parsed {
            dates.insert(date.format("%Y-%m-%d").to_string());
        }
    }
    dates
}

fn contains_known_name(input: &str, name: &str) -> bool {
    if name.is_ascii() {
        let lower_input = input.to_ascii_lowercase();
        let lower_name = name.to_ascii_lowercase();
        let mut cursor = 0;
        while let Some(relative) = lower_input[cursor..].find(&lower_name) {
            let start = cursor + relative;
            let end = start + lower_name.len();
            let before_ok = start == 0 || !is_ascii_word_byte(input.as_bytes()[start - 1]);
            let after_ok = end == input.len() || !is_ascii_word_byte(input.as_bytes()[end]);
            if before_ok && after_ok {
                return true;
            }
            cursor = start + 1;
        }
        false
    } else {
        input.contains(name)
    }
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
    let mut cursor = 0;
    while let Some(relative) = lower_input[cursor..].find(&lower_alias) {
        let start = cursor + relative;
        let end = start + lower_alias.len();
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

fn collapse_repeated_name_annotation(input: &str, canonical: &str) -> String {
    [
        format!("{canonical} ({canonical})"),
        format!("{canonical}（{canonical}）"),
    ]
    .into_iter()
    .fold(input.to_owned(), |text, repeated| {
        text.replace(&repeated, canonical)
    })
}

fn replace_unverified_hybrid_person_names(
    markdown: &str,
    context: &SummaryMeetingContext,
) -> (String, bool) {
    let candidates = unverified_hybrid_person_names(context);
    if candidates.is_empty() {
        return (markdown.to_owned(), false);
    }

    let mut changed = false;
    let sanitized = markdown
        .split('\n')
        .map(|line| {
            if !line_has_person_identity_signal(line) {
                return line.to_owned();
            }
            candidates.iter().fold(line.to_owned(), |text, candidate| {
                if text.contains(candidate) {
                    changed = true;
                    text.replace(candidate, "待核对人员")
                } else {
                    text
                }
            })
        })
        .collect::<Vec<_>>()
        .join("\n");
    (sanitized, changed)
}

/// Small local models occasionally splice the surname of one verified attendee
/// onto the given name of another (for example 王芳 + 赵强 -> 王强). Treat only
/// these high-confidence cross-person hybrids as unsupported names. This is much
/// narrower than fuzzy matching every two-character Chinese word in the summary.
fn unverified_hybrid_person_names(context: &SummaryMeetingContext) -> Vec<String> {
    let verified_names = context
        .verified_meeting_facts
        .attending
        .iter()
        .chain(context.verified_meeting_facts.absent.iter())
        .chain(context.verified_meeting_facts.host.iter())
        .map(|person| person.display_name.trim())
        .filter(|name| is_short_chinese_person_name(name))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    if verified_names.len() < 2 {
        return Vec::new();
    }

    let recognized_people = context
        .recognition_dictionary
        .people
        .iter()
        .flat_map(|person| {
            std::iter::once(person.display_name.as_str())
                .chain(person.aliases.iter().map(String::as_str))
        })
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect::<BTreeSet<_>>();
    let recognized_terms = context
        .recognition_dictionary
        .terms
        .iter()
        .flat_map(|term| {
            std::iter::once(term.canonical.as_str()).chain(term.aliases.iter().map(String::as_str))
        })
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .collect::<BTreeSet<_>>();

    let mut hybrids = BTreeSet::new();
    for surname_source in &verified_names {
        let surname = surname_source
            .chars()
            .next()
            .expect("verified name is non-empty");
        for given_name_source in &verified_names {
            if surname_source == given_name_source {
                continue;
            }
            let candidate = std::iter::once(surname)
                .chain(given_name_source.chars().skip(1))
                .collect::<String>();
            if is_short_chinese_person_name(&candidate)
                && !recognized_people.contains(candidate.as_str())
                && !recognized_terms.contains(candidate.as_str())
            {
                hybrids.insert(candidate);
            }
        }
    }

    let mut hybrids = hybrids.into_iter().collect::<Vec<_>>();
    hybrids.sort_by(|left, right| {
        right
            .chars()
            .count()
            .cmp(&left.chars().count())
            .then_with(|| left.cmp(right))
    });
    hybrids
}

fn is_short_chinese_person_name(value: &str) -> bool {
    let characters = value.chars().collect::<Vec<_>>();
    (2..=4).contains(&characters.len())
        && characters.iter().all(|character| {
            matches!(
                character,
                '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}'
            )
        })
}

fn line_has_person_identity_signal(line: &str) -> bool {
    structured_field_value(line, ATTENDEE_FIELD_LABELS).is_some()
        || structured_field_value(line, ABSENCE_FIELD_LABELS).is_some()
        || structured_field_value(line, HOST_FIELD_LABELS).is_some()
        || [
            "参会",
            "出席",
            "缺席",
            "主持",
            "负责人",
            "责任人",
            "指派",
            "分配",
            "负责",
            "提交",
            "跟进",
            "汇报",
            "行动",
            "任务",
            "分别",
        ]
        .iter()
        .any(|signal| line.contains(signal))
}

fn omit_unverified_high_risk_fields(markdown: &str, context: &SummaryMeetingContext) -> String {
    let without_roles = remove_unverified_person_annotations(markdown, context);
    let mut active_columns: Option<(Vec<usize>, String)> = None;
    without_roles
        .split('\n')
        .map(|line| {
            if line.trim_start().starts_with('|') && line.trim_end().ends_with('|') {
                let mut cells = parse_markdown_table_cells(line);
                let high_risk_columns = cells
                    .iter()
                    .enumerate()
                    .filter_map(|(index, header)| {
                        is_high_risk_table_header(header).then_some(index)
                    })
                    .collect::<Vec<_>>();
                if !high_risk_columns.is_empty() {
                    let placeholder = if cells.iter().any(|cell| !cell.is_ascii()) {
                        "会议未提及"
                    } else {
                        "Not mentioned"
                    };
                    active_columns = Some((high_risk_columns, placeholder.to_owned()));
                    return line.to_owned();
                }
                if let Some((columns, placeholder)) = active_columns.as_ref() {
                    if is_markdown_table_separator(&cells) {
                        return line.to_owned();
                    }
                    for index in columns {
                        if let Some(cell) = cells.get_mut(*index) {
                            *cell = placeholder.clone();
                        }
                    }
                    return format!("| {} |", cells.join(" | "));
                }
                line.to_owned()
            } else {
                active_columns = None;
                omit_unverified_inline_fields(line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_markdown_table_cells(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().trim_matches('*').trim().to_owned())
        .collect()
}

fn is_markdown_table_separator(cells: &[String]) -> bool {
    !cells.is_empty()
        && cells.iter().all(|cell| {
            cell.chars()
                .all(|character| matches!(character, ':' | '-' | ' '))
        })
}

fn is_high_risk_table_header(header: &str) -> bool {
    let normalized = header.trim().trim_matches('*').trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "负责人"
            | "负责人/部门"
            | "负责人／部门"
            | "owner"
            | "owner/department"
            | "owner / department"
            | "截止时间"
            | "deadline"
            | "due date"
            | "验收标准"
            | "acceptance criteria"
            | "当前状态"
            | "status"
            | "依赖或卡点"
            | "dependencies"
            | "dependency/blocker"
            | "升级条件"
            | "escalation condition"
    )
}

fn omit_unverified_inline_fields(line: &str) -> String {
    static CHINESE_HIGH_RISK_FIELD: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"((?:负责人|判断口径|截止时间|验收标准|当前状态|依赖或卡点|升级条件)\s*(?:\*\*)?\s*[:：]\s*)[^；;\n]+",
        )
        .expect("Chinese high-risk summary field regex must compile")
    });
    static ENGLISH_HIGH_RISK_FIELD: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?i)((?:owner|criteria|deadline|due date|acceptance criteria|status|dependencies|dependency/blocker|escalation condition)\s*(?:\*\*)?\s*:\s*)[^;\n]+",
        )
        .expect("English high-risk summary field regex must compile")
    });
    let sanitized = CHINESE_HIGH_RISK_FIELD.replace_all(line, "${1}会议未提及");
    ENGLISH_HIGH_RISK_FIELD
        .replace_all(&sanitized, "${1}Not mentioned")
        .into_owned()
}

fn remove_unverified_person_annotations(markdown: &str, context: &SummaryMeetingContext) -> String {
    context
        .verified_meeting_facts
        .attending
        .iter()
        .chain(context.verified_meeting_facts.absent.iter())
        .filter(|person| person.role.is_none())
        .fold(markdown.to_owned(), |text, person| {
            let case_flag = if person.display_name.is_ascii() {
                "(?i)"
            } else {
                ""
            };
            let pattern = format!(
                r"{}{}\s*[\(（]\s*[^\)）]+\s*[\)）]",
                case_flag,
                regex::escape(&person.display_name)
            );
            Regex::new(&pattern)
                .expect("verified person annotation regex must compile")
                .replace_all(&text, person.display_name.as_str())
                .into_owned()
        })
}

#[derive(Debug, Deserialize)]
struct SummaryMetadataDocument {
    #[serde(default)]
    meeting_name: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    completed_at: Option<String>,
    #[serde(default)]
    duration_seconds: Option<f64>,
    #[serde(default)]
    meeting_context: Option<MeetingContextContainer>,
}

pub fn load_summary_meeting_context(
    meeting_folder: &Path,
) -> Result<Option<SummaryMeetingContext>, String> {
    let metadata_path = meeting_folder.join("metadata.json");
    if !metadata_path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&metadata_path)
        .map_err(|error| format!("SUMMARY_CONTEXT_METADATA_READ_FAILED: {error}"))?;
    let metadata: SummaryMetadataDocument = serde_json::from_str(&raw)
        .map_err(|error| format!("SUMMARY_CONTEXT_METADATA_INVALID: {error}"))?;
    let Some(container) = metadata.meeting_context else {
        return Ok(None);
    };
    let container = normalize_and_validate_container(container)
        .map_err(|_| "SUMMARY_CONTEXT_METADATA_CONTEXT_INVALID".to_owned())?;
    let snapshot = container
        .current_context()
        .ok_or_else(|| "SUMMARY_CONTEXT_METADATA_REFERENCE_MISSING".to_owned())?;

    build_summary_meeting_context(
        clean_optional_text(metadata.meeting_name),
        normalize_timestamp(metadata.created_at.as_deref(), "created_at")?,
        normalize_timestamp(metadata.completed_at.as_deref(), "completed_at")?,
        normalize_duration(metadata.duration_seconds)?,
        snapshot,
    )
    .map(Some)
}

pub fn build_summary_meeting_context(
    meeting_name: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
    duration_seconds: Option<f64>,
    snapshot: &MeetingContextSnapshot,
) -> Result<SummaryMeetingContext, String> {
    let attending = snapshot
        .people
        .iter()
        .filter(|person| {
            matches!(
                person.attendance,
                AttendanceStatus::Attending | AttendanceStatus::Guest
            )
        })
        .map(VerifiedMeetingPerson::from)
        .collect();
    let absent = snapshot
        .people
        .iter()
        .filter(|person| person.attendance == AttendanceStatus::Absent)
        .map(VerifiedMeetingPerson::from)
        .collect();
    let host = snapshot
        .host_person_id
        .as_deref()
        .map(|host_id| {
            snapshot
                .people
                .iter()
                .find(|person| person.person_id == host_id)
                .map(VerifiedMeetingPerson::from)
                .ok_or_else(|| "SUMMARY_CONTEXT_HOST_REFERENCE_MISSING".to_owned())
        })
        .transpose()?;
    let people = snapshot
        .people
        .iter()
        .map(|person| RecognitionDictionaryPerson {
            person_id: person.person_id.clone(),
            display_name: person.display_name.clone(),
            aliases: person.aliases.clone(),
        })
        .collect();
    let terms = snapshot
        .terms
        .iter()
        .map(|term| RecognitionDictionaryTerm {
            term_id: term.term_id.clone(),
            canonical: term.canonical.clone(),
            aliases: term.aliases.clone(),
        })
        .collect();

    Ok(SummaryMeetingContext {
        context_id: snapshot.context_id.clone(),
        context_sha256: snapshot.context_sha256.clone(),
        verified_meeting_facts: VerifiedMeetingFacts {
            meeting_name: clean_optional_text(meeting_name),
            started_at,
            completed_at,
            duration_seconds: normalize_duration(duration_seconds)?,
            fixed_meeting_mechanism: clean_optional_text(snapshot.fixed_meeting_mechanism.clone()),
            attending,
            absent,
            host,
        },
        recognition_dictionary: RecognitionDictionary { people, terms },
    })
}

fn clean_optional_text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn normalize_timestamp(value: Option<&str>, field: &str) -> Result<Option<String>, String> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let parsed = DateTime::parse_from_rfc3339(value)
        .map_err(|error| format!("SUMMARY_CONTEXT_{field}_INVALID: {error}"))?;
    Ok(Some(
        parsed
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true),
    ))
}

fn normalize_duration(value: Option<f64>) -> Result<Option<f64>, String> {
    match value {
        Some(value) if value.is_finite() && value >= 0.0 => Ok(Some(value)),
        Some(_) => Err("SUMMARY_CONTEXT_DURATION_INVALID".to_owned()),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting_context::{
        MeetingContextSource, SnapshotTerm, MEETING_CONTEXT_SCHEMA_VERSION,
    };
    use crate::summary::source_binding::TranscriptEvidenceSegment;
    use chrono::TimeZone;

    fn snapshot() -> MeetingContextSnapshot {
        let mut snapshot = MeetingContextSnapshot {
            context_id: "ctx_summary_test".to_owned(),
            revision: 1,
            reason: "recording_start".to_owned(),
            captured_at: Utc.with_ymd_and_hms(2026, 8, 27, 1, 0, 0).unwrap(),
            source: MeetingContextSource {
                template_id: "license_station_weekly".to_owned(),
                template_version: 3,
                template_file_sha256: "a".repeat(64),
                profile_sha256: "b".repeat(64),
            },
            fixed_meeting_mechanism: Some("每周三 14:00 召开牌照站周会".to_owned()),
            people: vec![
                SnapshotPerson {
                    person_id: "person_host".to_owned(),
                    display_name: "MeiL".to_owned(),
                    aliases: vec![],
                    department: None,
                    role: Some("主持人".to_owned()),
                    attendance: AttendanceStatus::Attending,
                },
                SnapshotPerson {
                    person_id: "person_expected".to_owned(),
                    display_name: "Rayson".to_owned(),
                    aliases: vec!["瑞森".to_owned()],
                    department: None,
                    role: None,
                    attendance: AttendanceStatus::Expected,
                },
                SnapshotPerson {
                    person_id: "person_absent".to_owned(),
                    display_name: "Nick".to_owned(),
                    aliases: vec![],
                    department: None,
                    role: None,
                    attendance: AttendanceStatus::Absent,
                },
                SnapshotPerson {
                    person_id: "person_guest".to_owned(),
                    display_name: "QA测试嘉宾".to_owned(),
                    aliases: vec!["测试嘉宾".to_owned()],
                    department: Some("质量保障".to_owned()),
                    role: None,
                    attendance: AttendanceStatus::Guest,
                },
            ],
            host_person_id: Some("person_host".to_owned()),
            terms: vec![
                SnapshotTerm {
                    term_id: "term_pwa".to_owned(),
                    canonical: "PWA".to_owned(),
                    aliases: vec![],
                    category: Some("产品术语".to_owned()),
                },
                SnapshotTerm {
                    term_id: "term_youtube".to_owned(),
                    canonical: "YouTube".to_owned(),
                    aliases: vec!["U2B".to_owned()],
                    category: Some("渠道".to_owned()),
                },
            ],
            context_sha256: String::new(),
        };
        snapshot.context_sha256 = super::super::snapshot_sha256(&snapshot);
        snapshot
    }

    #[test]
    fn mc_u05_expected_people_are_dictionary_only_not_attendees() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            Some("2026-08-27T01:00:00.000Z".to_owned()),
            None,
            None,
            &snapshot(),
        )
        .unwrap();

        let attendee_names: Vec<_> = context
            .verified_meeting_facts
            .attending
            .iter()
            .map(|person| person.display_name.as_str())
            .collect();
        assert_eq!(attendee_names, vec!["MeiL", "QA测试嘉宾"]);
        assert_eq!(
            context.verified_meeting_facts.absent[0].display_name,
            "Nick"
        );
        assert_eq!(
            context
                .verified_meeting_facts
                .host
                .as_ref()
                .unwrap()
                .display_name,
            "MeiL"
        );
        assert!(context
            .recognition_dictionary
            .people
            .iter()
            .any(|person| person.display_name == "Rayson"));
        assert!(!attendee_names.contains(&"Rayson"));
    }

    #[test]
    fn mc_u05_metadata_times_are_authoritative_and_missing_fields_are_not_inferred() {
        let directory = tempfile::tempdir().unwrap();
        let snapshot = snapshot();
        let container = MeetingContextContainer {
            schema_version: MEETING_CONTEXT_SCHEMA_VERSION,
            recording_context_id: snapshot.context_id.clone(),
            current_context_id: snapshot.context_id.clone(),
            contexts: vec![snapshot],
        };
        std::fs::write(
            directory.path().join("metadata.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "meeting_name": "牌照站周会",
                "created_at": "2026-08-27T09:00:00+08:00",
                "completed_at": "2026-08-27T10:30:00+08:00",
                "duration_seconds": 5400.0,
                "meeting_context": container,
            }))
            .unwrap(),
        )
        .unwrap();

        let context = load_summary_meeting_context(directory.path())
            .unwrap()
            .unwrap();
        assert_eq!(
            context.verified_meeting_facts.started_at.as_deref(),
            Some("2026-08-27T01:00:00.000Z")
        );
        assert_eq!(
            context.verified_meeting_facts.completed_at.as_deref(),
            Some("2026-08-27T02:30:00.000Z")
        );
        assert_eq!(
            context.verified_meeting_facts.duration_seconds,
            Some(5400.0)
        );
        let expected = context
            .recognition_dictionary
            .people
            .iter()
            .find(|person| person.display_name == "Rayson")
            .unwrap();
        assert_eq!(expected.aliases, vec!["瑞森"]);
        let absent = &context.verified_meeting_facts.absent[0];
        assert_eq!(absent.role, None);
        assert_eq!(absent.department, None);
    }

    #[test]
    fn mc_u05_prompt_separates_facts_from_spelling_dictionary_and_hash_is_stable() {
        let context = build_summary_meeting_context(
            Some(" 牌照站周会 ".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        let prompt = context.to_prompt_block().unwrap();

        assert!(prompt.contains("verified_meeting_facts"));
        assert!(prompt.contains("recognition_dictionary"));
        assert!(prompt.contains("is not proven to have attended"));
        assert!(prompt.contains("\"department\": null"));
        assert_eq!(context.sha256(), context.clone().sha256());
        assert_eq!(
            context.verified_meeting_facts.meeting_name.as_deref(),
            Some("牌照站周会")
        );
    }

    #[test]
    fn fact_validation_normalizes_aliases_and_accepts_matching_structured_facts() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            Some("2026-08-27T01:00:00.000Z".to_owned()),
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        let validated = validate_summary_markdown(
            "会议日期：2026-08-27\n参会人员：MeiL、QA测试嘉宾\n缺席人员：Nick\n主持人：MeiL\n讨论了瑞森的拼写。\n行动截止日期：2026-09-30",
            Some(&context),
        );

        assert_eq!(
            validated.validation.status,
            SummaryFactValidationStatus::Passed
        );
        assert!(validated.validation.aliases_normalized);
        assert!(validated.markdown.contains("Rayson的拼写"));
        assert!(validated.markdown.contains("2026-09-30"));
        assert!(validated.validation.warnings.is_empty());
    }

    #[test]
    fn alias_normalization_collapses_repeated_canonical_name_annotations() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();

        assert_eq!(
            context.normalize_known_aliases("Rayson (瑞森) 与 Rayson（瑞森）"),
            "Rayson 与 Rayson"
        );
    }

    #[test]
    fn summary_normalization_applies_aliases_and_canonical_ascii_case() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();

        let once = context.normalize_known_aliases(
            "rayson youtube YOUTUBE u2b pwa Youtube2 youtube_api myyoutube",
        );
        assert_eq!(
            once,
            "Rayson YouTube YouTube YouTube PWA Youtube2 youtube_api myyoutube"
        );
        assert_eq!(context.normalize_known_aliases(&once), once);
    }

    #[test]
    fn transcript_grounding_flags_unknown_terms_organizations_and_status_upgrades() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        let transcript = "[00:10] PWA方案暂时不会用，后续继续验证。YouTube的一个技术在做AI工具。CGS那边会再沟通。";
        let markdown = r#"**会议结论**
- PWA方案已被放弃，CPS系统已完成。

| 部门/小组 | 行动任务 |
| :--- | :--- |
| YouTube技术团队 | 建立AI工具 |"#;

        let validated =
            validate_summary_markdown_with_transcript(markdown, Some(&context), transcript);
        let codes = validated
            .validation
            .warnings
            .iter()
            .map(|warning| warning.code.as_str())
            .collect::<BTreeSet<_>>();

        assert_eq!(
            validated.validation.status,
            SummaryFactValidationStatus::NeedsReview
        );
        assert!(codes.contains("unsupported_transcript_term"));
        assert!(codes.contains("unsupported_organization"));
        assert!(codes.contains("unsupported_status_claim"));
    }

    #[test]
    fn transcript_grounding_accepts_explicitly_supported_terms_organizations_and_statuses() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        let transcript = "[00:10] 市场部确认PWA方案已暂停，CTS系统继续测试。";
        let markdown = r#"**会议结论**
- PWA方案已暂停，CTS系统继续测试。

| 部门/小组 | 行动任务 |
| :--- | :--- |
| 市场部 | 继续测试CTS系统 |"#;

        let validated =
            validate_summary_markdown_with_transcript(markdown, Some(&context), transcript);

        assert_eq!(
            validated.validation.status,
            SummaryFactValidationStatus::Passed
        );
        assert!(validated.validation.warnings.is_empty());
    }

    #[test]
    fn transcript_grounding_removes_template_only_password_rule_and_marks_review() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        let markdown = r#"**风险与安全要求**
| 风险/禁止事项 | 处理方式 |
| :--- | :--- |
| 用户账号密码泄露 | 禁止索取账号密码，通过用户 ID 排查 |
| 数据异常 | 继续核对埋点 |"#;

        let validated = validate_summary_markdown_with_transcript(
            markdown,
            Some(&context),
            "会议讨论了数据异常，需要继续核对埋点。",
        );

        assert_eq!(
            validated.validation.status,
            SummaryFactValidationStatus::NeedsReview
        );
        assert!(validated
            .validation
            .warnings
            .iter()
            .any(|warning| warning.code == "unsupported_security_claim"));
        assert!(!validated.markdown.contains("账号密码"));
        assert!(validated.markdown.contains("数据异常"));
    }

    #[test]
    fn transcript_grounding_keeps_explicit_password_security_rule() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        let markdown = "**风险与安全要求**\n- 客户问题排查禁止索取账号密码。";

        let validated = validate_summary_markdown_with_transcript(
            markdown,
            Some(&context),
            "客户问题排查不得索取账号密码，应通过用户 ID 和登录记录定位。",
        );

        assert!(validated.markdown.contains("禁止索取账号密码"));
        assert!(!validated
            .validation
            .warnings
            .iter()
            .any(|warning| warning.code == "unsupported_security_claim"));
    }

    #[test]
    fn transcript_grounding_binds_status_to_the_same_business_object() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        let transcript = "Google 包已经完成，PWA 继续观察。";
        let markdown = "**会议结论**\n- PWA 已完成。";

        let validated =
            validate_summary_markdown_with_transcript(markdown, Some(&context), transcript);

        assert!(validated
            .validation
            .warnings
            .iter()
            .any(|warning| warning.code == "unsupported_status_claim"));
    }

    #[test]
    fn transcript_grounding_accepts_status_variant_for_the_same_business_object() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        let transcript = "PWA 方案已经完成。";
        let markdown = "**会议结论**\n- PWA 已完成。";

        let validated =
            validate_summary_markdown_with_transcript(markdown, Some(&context), transcript);

        assert!(!validated
            .validation
            .warnings
            .iter()
            .any(|warning| warning.code == "unsupported_status_claim"));
    }

    #[test]
    fn transcript_grounding_binds_english_status_to_the_same_business_object() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        let transcript = "The Google package is completed. PWA remains under observation.";
        let markdown = "**Conclusion**\n- PWA is completed.";

        let validated =
            validate_summary_markdown_with_transcript(markdown, Some(&context), transcript);

        assert!(validated
            .validation
            .warnings
            .iter()
            .any(|warning| warning.code == "unsupported_status_claim"));
    }

    #[test]
    fn transcript_grounding_flags_unknown_chinese_object_with_borrowed_status() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        let validated = validate_summary_markdown_with_transcript(
            "**会议结论**\n- 赠金政策已完成。",
            Some(&context),
            "Google 包已经完成。",
        );

        assert!(validated
            .validation
            .warnings
            .iter()
            .any(|warning| warning.code == "unsupported_status_claim"));
    }

    #[test]
    fn transcript_grounding_does_not_allow_summary_to_add_an_unknown_object_prefix() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        let validated = validate_summary_markdown_with_transcript(
            "**会议结论**\n- 赠金政策项目已完成。",
            Some(&context),
            "项目已经完成。",
        );

        assert!(validated
            .validation
            .warnings
            .iter()
            .any(|warning| warning.code == "unsupported_status_claim"));
    }

    #[test]
    fn transcript_grounding_accepts_status_variant_for_same_unknown_chinese_object() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        let validated = validate_summary_markdown_with_transcript(
            "**会议结论**\n- 赠金政策已完成。",
            Some(&context),
            "赠金政策已经完成。",
        );

        assert!(!validated
            .validation
            .warnings
            .iter()
            .any(|warning| warning.code == "unsupported_status_claim"));
    }

    #[test]
    fn transcript_grounding_keeps_comma_bound_subject_with_its_status() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        let validated = validate_summary_markdown_with_transcript(
            "**会议结论**\n- 对于 PWA，已完成第一轮测试。",
            Some(&context),
            "对于 PWA，已经完成第一轮测试。",
        );

        assert!(!validated
            .validation
            .warnings
            .iter()
            .any(|warning| warning.code == "unsupported_status_claim"));
    }

    #[test]
    fn transcript_grounding_does_not_treat_negated_english_status_as_positive_support() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        let validated = validate_summary_markdown_with_transcript(
            "**Conclusion**\n- PWA is completed.",
            Some(&context),
            "PWA is not completed.",
        );

        assert!(validated
            .validation
            .warnings
            .iter()
            .any(|warning| warning.code == "unsupported_status_claim"));
    }

    #[test]
    fn transcript_grounding_understands_contracted_and_intervening_english_negation() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        for transcript in ["PWA isn't completed.", "PWA is not yet completed."] {
            let validated = validate_summary_markdown_with_transcript(
                "**Conclusion**\n- PWA is completed.",
                Some(&context),
                transcript,
            );
            assert!(validated
                .validation
                .warnings
                .iter()
                .any(|warning| warning.code == "unsupported_status_claim"));
        }
    }

    #[test]
    fn transcript_grounding_requires_ascii_anchor_word_boundaries() {
        let mut test_snapshot = snapshot();
        test_snapshot.terms.push(SnapshotTerm {
            term_id: "term_ai".to_owned(),
            canonical: "AI".to_owned(),
            aliases: Vec::new(),
            category: Some("技术术语".to_owned()),
        });
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &test_snapshot,
        )
        .unwrap();
        let validated = validate_summary_markdown_with_transcript(
            "**会议结论**\n- AI 项目已完成。",
            Some(&context),
            "PAID 项目已经完成。",
        );

        assert!(validated
            .validation
            .warnings
            .iter()
            .any(|warning| warning.code == "unsupported_status_claim"));
        assert!(validated
            .validation
            .warnings
            .iter()
            .any(|warning| warning.code == "unsupported_transcript_term"));
    }

    #[test]
    fn transcript_grounding_normalizes_aliases_from_the_source_not_the_dictionary() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        let validated = validate_summary_markdown_with_transcript(
            "**会议结论**\n- YouTube 测试已完成。",
            Some(&context),
            "U2B 测试已经完成。",
        );

        assert!(!validated
            .validation
            .warnings
            .iter()
            .any(|warning| warning.code == "unsupported_transcript_term"));
        assert!(!validated
            .validation
            .warnings
            .iter()
            .any(|warning| warning.code == "unsupported_status_claim"));
    }

    #[test]
    fn transcript_grounding_does_not_inherit_subject_when_later_clause_names_its_own() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        let validated = validate_summary_markdown_with_transcript(
            "**会议结论**\n- Google 已完成。",
            Some(&context),
            "Google 未完成，已完成的是 PWA。",
        );

        assert!(validated
            .validation
            .warnings
            .iter()
            .any(|warning| warning.code == "unsupported_status_claim"));
    }

    #[test]
    fn transcript_grounding_accepts_configured_alias_for_status_anchor() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        let validated = validate_summary_markdown_with_transcript(
            "**会议结论**\n- YouTube 测试已完成。",
            Some(&context),
            "U2B 测试已经完成。",
        );

        assert!(!validated
            .validation
            .warnings
            .iter()
            .any(|warning| warning.code == "unsupported_status_claim"));
    }

    #[test]
    fn transcript_grounding_ignores_status_only_section_heading() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        let markdown = "# 已完成事项\n- 会议未提及";

        let validated = validate_summary_markdown_with_transcript(
            markdown,
            Some(&context),
            "本周只讨论后续计划。",
        );

        assert!(!validated
            .validation
            .warnings
            .iter()
            .any(|warning| warning.code == "unsupported_status_claim"));
    }

    #[test]
    fn transcript_grounding_requires_non_empty_transcript_evidence() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();

        let validated =
            validate_summary_markdown_with_transcript("会议结论：无", Some(&context), "  ");

        assert_eq!(
            validated.validation.status,
            SummaryFactValidationStatus::NeedsReview
        );
        assert_eq!(validated.validation.warning_count, 1);
        assert_eq!(
            validated.validation.warnings[0].code,
            "missing_transcript_evidence"
        );
    }

    #[test]
    fn validation_omits_unverified_high_risk_fields_but_keeps_action_content() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        let markdown = r#"**行动计划**
| 部门/小组 | 负责人 | 行动任务 | 截止时间 | 验收标准 | 当前状态 | 依赖或卡点 |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| 产品 | Rayson | 排查用户路径 | 本周内 | 输出完整报告 | 进行中 | 等待接口 |

**下周复盘重点**
- 复盘体验会；判断口径：完成问题清单；负责人：Rayson（技术对接）"#;

        let validated = validate_summary_markdown(markdown, Some(&context));

        assert!(validated.markdown.contains("排查用户路径"));
        assert!(validated.markdown.contains(
            "| 产品 | 会议未提及 | 排查用户路径 | 会议未提及 | 会议未提及 | 会议未提及 | 会议未提及 |"
        ));
        assert!(validated
            .markdown
            .contains("判断口径：会议未提及；负责人：会议未提及"));
        assert!(!validated.markdown.contains("技术对接"));
    }

    #[test]
    fn activated_source_restores_only_traceable_owner_and_time_and_marks_conflicts() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        let source = TranscriptVersionSnapshot::legacy_whisper(
            "meeting_1",
            vec![TranscriptEvidenceSegment {
                segment_id: "segment_1".to_owned(),
                start_ms: Some(1_000),
                end_ms: Some(4_000),
                wall_clock: None,
                anonymous_speaker: None,
                bound_person_id: None,
                bound_display_name: None,
                text: "Rayson 负责整理报告，截止时间是周五。".to_owned(),
            }],
        );
        let validated = validate_summary_markdown_with_source(
            r#"| 行动项 | 负责人 | 截止时间 |
| --- | --- | --- |
| 整理报告 | Rayson | 周五 |
| 发布版本 | MeiL | 下周一 |"#,
            Some(&context),
            &source,
        )
        .unwrap();

        assert!(validated.markdown.contains("| 整理报告 | Rayson | 周五 |"));
        assert!(validated
            .markdown
            .contains("| 发布版本 | 会议未提及 | 会议未提及 |"));
        assert_eq!(
            validated.validation.status,
            SummaryFactValidationStatus::NeedsReview
        );
        assert!(validated
            .validation
            .warnings
            .iter()
            .any(|warning| warning.code == "untraceable_action_owner"));
        assert!(validated
            .validation
            .warnings
            .iter()
            .any(|warning| warning.code == "untraceable_action_time"));
        let evidence = validated.validation.source_evidence.unwrap();
        assert_eq!(evidence.transcript_version_id, "legacy_whisper_meeting_1");
        assert_eq!(evidence.transcript_sha256.len(), 64);
        assert_eq!(evidence.speaker_binding_sha256.len(), 64);
    }

    #[test]
    fn validation_localizes_combined_owner_department_columns() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            None,
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        let markdown = r#"| 负责人/部门 | 行动任务 | 截止时间 |
| :--- | :--- | :--- |
| Not mentioned | 提交测试报告 | 2026年8月28日 |"#;

        let validated = validate_summary_markdown(markdown, Some(&context));

        assert!(validated
            .markdown
            .contains("| 会议未提及 | 提交测试报告 | 会议未提及 |"));
        assert!(!validated.markdown.contains("Not mentioned"));
        assert!(!validated.markdown.contains("2026年8月28日"));
    }

    #[test]
    fn fact_validation_understands_common_chinese_meeting_date_formats() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            Some("2026-08-27T01:00:00.000Z".to_owned()),
            None,
            None,
            &snapshot(),
        )
        .unwrap();

        for markdown in [
            "会议日期：2026年8月27日",
            "会议时间：8月27日 14:00-15:30",
            "会议日期：2026/8/27",
        ] {
            let validated = validate_summary_markdown(markdown, Some(&context));
            assert_eq!(
                validated.validation.status,
                SummaryFactValidationStatus::Passed,
                "expected matching date format to pass: {markdown}"
            );
        }

        for markdown in [
            "会议日期：2025年8月27日",
            "会议时间：8月26日 14:00-15:30",
            "会议日期：2026/8/26",
        ] {
            let validated = validate_summary_markdown(markdown, Some(&context));
            assert!(
                validated
                    .validation
                    .warnings
                    .iter()
                    .any(|warning| warning.code == "meeting_date_mismatch"),
                "expected mismatching date format to be flagged: {markdown}"
            );
        }
    }

    #[test]
    fn fact_validation_rejects_inferred_host_when_snapshot_has_no_host() {
        let mut snapshot = snapshot();
        snapshot.host_person_id = None;
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            Some("2026-08-27T01:00:00.000Z".to_owned()),
            None,
            None,
            &snapshot,
        )
        .unwrap();
        let validated = validate_summary_markdown("主持人：MeiL", Some(&context));

        assert!(validated
            .validation
            .warnings
            .iter()
            .any(|warning| warning.code == "host_conflict"));
    }

    #[test]
    fn fact_validation_scopes_combined_structured_fields_to_their_own_values() {
        let mut snapshot = snapshot();
        snapshot.host_person_id = None;
        for person in &mut snapshot.people {
            person.attendance = AttendanceStatus::Attending;
        }
        snapshot.context_sha256 = super::super::snapshot_sha256(&snapshot);
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            Some("2026-08-27T01:00:00.000Z".to_owned()),
            None,
            None,
            &snapshot,
        )
        .unwrap();
        let validated = validate_summary_markdown(
            "参会人员：MeiL、Rayson、Nick、QA测试嘉宾。缺席人员：无。主持人：会议未提及",
            Some(&context),
        );

        assert_eq!(
            validated.validation.status,
            SummaryFactValidationStatus::Passed
        );
        assert!(validated.validation.warnings.is_empty());
        assert!(context
            .to_prompt_block()
            .unwrap()
            .contains("\"host\": null"));
    }

    #[test]
    fn fact_validation_marks_date_attendance_absence_and_host_conflicts_for_review() {
        let context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            Some("2026-08-27T01:00:00.000Z".to_owned()),
            None,
            None,
            &snapshot(),
        )
        .unwrap();
        let validated = validate_summary_markdown(
            "会议日期：2024-09-13\n参会人员：Rayson\n缺席人员：MeiL\n主持人：Rayson",
            Some(&context),
        );
        let codes = validated
            .validation
            .warnings
            .iter()
            .map(|warning| warning.code.as_str())
            .collect::<BTreeSet<_>>();

        assert_eq!(
            validated.validation.status,
            SummaryFactValidationStatus::NeedsReview
        );
        assert_eq!(validated.validation.warning_count, 4);
        assert!(codes.contains("meeting_date_mismatch"));
        assert!(codes.contains("attendance_conflict"));
        assert!(codes.contains("absence_conflict"));
        assert!(codes.contains("host_conflict"));
    }

    #[test]
    fn fact_validation_blocks_cross_person_hybrid_chinese_names() {
        let mut snapshot = snapshot();
        snapshot.people.extend([
            SnapshotPerson {
                person_id: "person_li_ming".to_owned(),
                display_name: "李明".to_owned(),
                aliases: Vec::new(),
                department: Some("质量保障".to_owned()),
                role: None,
                attendance: AttendanceStatus::Guest,
            },
            SnapshotPerson {
                person_id: "person_wang_fang".to_owned(),
                display_name: "王芳".to_owned(),
                aliases: Vec::new(),
                department: Some("质量保障".to_owned()),
                role: Some("测试报告负责人".to_owned()),
                attendance: AttendanceStatus::Guest,
            },
            SnapshotPerson {
                person_id: "person_zhao_qiang".to_owned(),
                display_name: "赵强".to_owned(),
                aliases: Vec::new(),
                department: Some("质量保障".to_owned()),
                role: Some("问题修复负责人".to_owned()),
                attendance: AttendanceStatus::Guest,
            },
        ]);
        snapshot.context_sha256 = super::super::snapshot_sha256(&snapshot);
        let context = build_summary_meeting_context(
            Some("核心功能验收会议".to_owned()),
            None,
            None,
            None,
            &snapshot,
        )
        .unwrap();

        let validated = validate_summary_markdown(
            "讨论要点：王芳和王强分别被指派了具体的行动任务。",
            Some(&context),
        );

        assert_eq!(
            validated.validation.status,
            SummaryFactValidationStatus::NeedsReview
        );
        assert!(validated
            .validation
            .warnings
            .iter()
            .any(|warning| warning.code == "person_name_conflict"));
        assert!(!validated.markdown.contains("王强"));
        assert!(validated.markdown.contains("王芳和待核对人员分别被指派"));
    }

    #[test]
    fn fact_validation_accepts_exact_verified_chinese_names() {
        let mut snapshot = snapshot();
        snapshot.people.extend([
            SnapshotPerson {
                person_id: "person_wang_fang".to_owned(),
                display_name: "王芳".to_owned(),
                aliases: Vec::new(),
                department: Some("质量保障".to_owned()),
                role: Some("测试报告负责人".to_owned()),
                attendance: AttendanceStatus::Guest,
            },
            SnapshotPerson {
                person_id: "person_zhao_qiang".to_owned(),
                display_name: "赵强".to_owned(),
                aliases: Vec::new(),
                department: Some("质量保障".to_owned()),
                role: Some("问题修复负责人".to_owned()),
                attendance: AttendanceStatus::Guest,
            },
        ]);
        snapshot.context_sha256 = super::super::snapshot_sha256(&snapshot);
        let context = build_summary_meeting_context(None, None, None, None, &snapshot).unwrap();

        let validated = validate_summary_markdown(
            "讨论要点：王芳和赵强分别被指派了具体的行动任务。",
            Some(&context),
        );

        assert_eq!(
            validated.validation.status,
            SummaryFactValidationStatus::Passed
        );
        assert_eq!(
            validated.markdown,
            "讨论要点：王芳和赵强分别被指派了具体的行动任务。"
        );
    }

    #[test]
    fn missing_context_is_visible_warning_not_silent_success() {
        let validated = validate_summary_markdown("# Summary\nContent", None);

        assert_eq!(validated.markdown, "# Summary\nContent");
        assert_eq!(
            validated.validation.status,
            SummaryFactValidationStatus::NeedsReview
        );
        assert_eq!(
            validated.validation.warnings[0].code,
            "missing_meeting_context"
        );
    }

    #[test]
    fn generated_summary_without_context_removes_invented_meeting_identity_fields() {
        let generated = r#"# 游戏业务与研发部门月度复盘会议

**会议信息**
- 会议名称：游戏业务与研发部门月度复盘会议
- 会议时间：2024年10月19日 星期四 16:00-17:30
- 参会人员：Miko、Annie，共 20–30 人
- 主持人：Annie

**会议结论**
- CGS 需要继续推进。"#;

        let sanitized = sanitize_generated_summary_without_meeting_context(generated);

        assert!(sanitized.starts_with("# 会议纪要（待核对）"));
        assert!(sanitized.contains("会议名称：会议未提及"));
        assert!(sanitized.contains("会议时间：会议未提及"));
        assert!(sanitized.contains("参会人员：会议未提及"));
        assert!(sanitized.contains("主持人：会议未提及"));
        assert!(sanitized.contains("CGS 需要继续推进。"));
        assert!(!sanitized.contains("2024年10月19日"));
        assert!(!sanitized.contains("Miko、Annie"));
    }

    #[test]
    fn generated_english_summary_without_context_uses_english_placeholder() {
        let generated = "# Engineering Review\n- Meeting time: Friday 4 PM\n- Attendees: MeiL";

        let sanitized = sanitize_generated_summary_without_meeting_context(generated);

        assert!(sanitized.starts_with("# Meeting Summary (Facts Need Review)"));
        assert!(sanitized.contains("Meeting time: Not mentioned"));
        assert!(sanitized.contains("Attendees: Not mentioned"));
    }

    fn markdown_table_rows(markdown: &str, required_headers: &[&str]) -> Vec<Vec<String>> {
        let lines = markdown.lines().collect::<Vec<_>>();
        for (index, line) in lines.iter().enumerate() {
            if !line.trim_start().starts_with('|') {
                continue;
            }
            let headers = markdown_table_cells(line);
            if !required_headers
                .iter()
                .all(|required| headers.iter().any(|header| header == required))
            {
                continue;
            }
            return lines
                .iter()
                .skip(index + 1)
                .take_while(|candidate| candidate.trim_start().starts_with('|'))
                .map(|candidate| markdown_table_cells(candidate))
                .filter(|cells| {
                    !cells.is_empty()
                        && !cells.iter().all(|cell| {
                            let compact = cell.replace([':', '-', ' '], "");
                            compact.is_empty()
                        })
                })
                .collect();
        }
        Vec::new()
    }

    fn markdown_table_cells(line: &str) -> Vec<String> {
        line.trim()
            .trim_matches('|')
            .split('|')
            .map(|cell| cell.trim().trim_matches('*').trim().to_owned())
            .collect()
    }

    fn is_not_mentioned_cell(value: &str) -> bool {
        let compact = value.trim().to_ascii_lowercase();
        compact.is_empty()
            || compact == "-"
            || compact == "—"
            || compact.contains("会议未提及")
            || compact.contains("未提及")
            || compact.contains("not mentioned")
            || compact.contains("none noted")
    }

    fn has_inferred_parenthetical_person_annotation(
        markdown: &str,
        context: &SummaryMeetingContext,
    ) -> bool {
        context
            .recognition_dictionary
            .people
            .iter()
            .filter(|person| {
                context
                    .verified_meeting_facts
                    .attending
                    .iter()
                    .find(|verified| verified.person_id == person.person_id)
                    .is_some_and(|verified| verified.role.is_none())
            })
            .any(|person| {
                let case_flag = if person.display_name.is_ascii() {
                    "(?i)"
                } else {
                    ""
                };
                let pattern = format!(
                    r"{}{}\s*[\(（]\s*([^\)）]+)\s*[\)）]",
                    case_flag,
                    regex::escape(&person.display_name)
                );
                Regex::new(&pattern)
                    .unwrap()
                    .captures_iter(markdown)
                    .any(|capture| {
                        capture.get(1).is_some_and(|annotation| {
                            !annotation
                                .as_str()
                                .trim()
                                .eq_ignore_ascii_case(&person.display_name)
                        })
                    })
            })
    }

    #[test]
    fn old_metadata_without_meeting_context_returns_none() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("metadata.json"),
            br#"{"version":"1.0","created_at":"2026-08-27T01:00:00Z"}"#,
        )
        .unwrap();

        assert!(load_summary_meeting_context(directory.path())
            .unwrap()
            .is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires the real Qwen 3.5 2B model and local meeting fixture"]
    async fn mc_r04_real_qwen_2b_summary_respects_verified_meeting_facts() {
        let meeting_dir = std::path::PathBuf::from(
            std::env::var("MEETILY_MC_R04_MEETING_DIR")
                .expect("MEETILY_MC_R04_MEETING_DIR must point to the real meeting folder"),
        );
        let summary_models_dir = std::path::PathBuf::from(
            std::env::var("MEETILY_MC_R04_SUMMARY_MODELS_DIR")
                .expect("MEETILY_MC_R04_SUMMARY_MODELS_DIR must contain the downloaded model"),
        );
        let template_path = std::path::PathBuf::from(
            std::env::var("MEETILY_MC_R04_TEMPLATE_PATH")
                .expect("MEETILY_MC_R04_TEMPLATE_PATH must point to license_station_weekly"),
        );
        let evidence_path = std::path::PathBuf::from(
            std::env::var("MEETILY_MC_R04_EVIDENCE_PATH")
                .expect("MEETILY_MC_R04_EVIDENCE_PATH must be provided"),
        );
        let reuse_generation_evidence = std::env::var("MEETILY_MC_R04_REUSE_GENERATION_EVIDENCE")
            .ok()
            .map(std::path::PathBuf::from);

        let transcript_json =
            std::fs::read_to_string(meeting_dir.join("transcripts.json")).unwrap();
        let transcript_value: serde_json::Value = serde_json::from_str(&transcript_json).unwrap();
        let segments = transcript_value["segments"].as_array().unwrap();
        let transcript = segments
            .iter()
            .filter_map(|segment| segment["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            segments.len(),
            139,
            "the fixed real transcript fixture changed"
        );

        let template_json = std::fs::read_to_string(&template_path).unwrap();
        let template: crate::summary::templates::TemplateV2 =
            serde_json::from_str(&template_json).unwrap();
        assert_eq!(template.id, "license_station_weekly");
        let profile: crate::meeting_context::MeetingContextProfile =
            serde_json::from_value(template.extensions["meetily_meeting_context"].clone()).unwrap();
        let profile = crate::meeting_context::normalize_and_validate_profile(profile).unwrap();
        let profile_hash = crate::meeting_context::profile_sha256(&profile);
        let attendance = profile
            .people
            .iter()
            .map(|person| crate::meeting_context::PersonAttendanceOverride {
                person_id: person.person_id.clone(),
                attendance: crate::meeting_context::AttendanceStatus::Attending,
            })
            .collect::<Vec<_>>();
        let container =
            crate::meeting_context::MeetingContextContainer::from_profile_with_recording_draft(
                profile,
                template.id.clone(),
                template.version,
                crate::summary::template_snapshot::sha256_text(&template_json),
                Utc.with_ymd_and_hms(2026, 8, 26, 5, 59, 54).unwrap(),
                Some(crate::meeting_context::RecordingMeetingContextDraft {
                    expected_profile_sha256: profile_hash,
                    attendance,
                    host_person_id: None,
                    guests: Vec::new(),
                    additional_terms: Vec::new(),
                }),
            )
            .unwrap();
        let snapshot = container.current_context().unwrap();
        let summary_context = build_summary_meeting_context(
            Some("牌照站周会".to_owned()),
            Some("2026-08-26T05:59:54.298879400Z".to_owned()),
            Some("2026-08-26T06:51:34.877514100Z".to_owned()),
            Some(3074.73),
            snapshot,
        )
        .unwrap();
        let runtime_template = template.to_runtime_template();
        let started = std::time::Instant::now();
        let (generation, elapsed_seconds) =
            if let Some(reuse_path) = reuse_generation_evidence.as_ref() {
                let saved: serde_json::Value = serde_json::from_str(
                    &std::fs::read_to_string(reuse_path)
                        .expect("saved real generation evidence must be readable"),
                )
                .expect("saved real generation evidence must be valid JSON");
                let markdown = saved["final_simplified_chinese_markdown"]
                    .as_str()
                    .expect("saved evidence must contain final_simplified_chinese_markdown")
                    .to_owned();
                let english_markdown = saved["english_markdown"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                let chunk_count = saved["chunk_count"].as_i64().unwrap_or(1);
                let saved_elapsed = saved["elapsed_seconds"].as_f64().unwrap_or_default();
                (
                    Ok::<(String, String, i64), String>((markdown, english_markdown, chunk_count)),
                    saved_elapsed,
                )
            } else {
                let generation = crate::summary::processor::generate_meeting_summary(
                    &reqwest::Client::new(),
                    &crate::summary::llm_client::LLMProvider::BuiltInAI,
                    "qwen3.5:2b",
                    "",
                    &transcript,
                    "",
                    Some(&summary_context),
                    &template.id,
                    &runtime_template,
                    32_468,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(&summary_models_dir),
                    None,
                    Some("zh-CN"),
                    Some("zh"),
                    None,
                )
                .await;
                let elapsed = started.elapsed().as_secs_f64();
                let _ = crate::summary::summary_engine::force_shutdown_sidecar().await;
                (generation, elapsed)
            };

        let (raw_markdown, english_markdown, chunk_count) = match generation {
            Ok(value) => value,
            Err(error) => {
                let evidence = serde_json::json!({
                    "test_id": "MC-R04",
                    "verdict": "FAIL",
                    "failure_stage": "real_qwen_2b_generation",
                    "error": error,
                    "elapsed_seconds": elapsed_seconds,
                    "model": "qwen3.5:2b",
                    "template_id": template.id,
                    "template_version": template.version,
                    "segment_count": segments.len(),
                    "summary_context_sha256": summary_context.sha256(),
                });
                std::fs::write(
                    &evidence_path,
                    serde_json::to_vec_pretty(&evidence).unwrap(),
                )
                .unwrap();
                panic!("real Qwen 2B summary generation failed: {error}");
            }
        };

        // Apply the same production guard to reused real-model evidence.  A
        // live generation has already passed through it; the function is
        // deliberately idempotent.
        let raw_markdown = crate::summary::processor::ensure_template_sections(
            &raw_markdown,
            &runtime_template,
            Some("zh-CN"),
        );
        let simplified_markdown =
            crate::transcript_normalization::normalize_simplified_chinese_script(
                &raw_markdown,
                Some("zh-CN"),
            );
        let raw_template_password_example_present =
            contains_password_security_claim(&simplified_markdown);
        let validated = validate_summary_markdown_with_transcript(
            &simplified_markdown,
            Some(&summary_context),
            &transcript,
        );
        let markdown = &validated.markdown;
        let compact = markdown.replace(' ', "").replace('\n', "");
        let has_correct_date = markdown.contains("2026-08-26")
            || markdown.contains("2026年8月26日")
            || markdown.contains("2026/8/26");
        let host_not_inferred = markdown
            .lines()
            .filter_map(|line| structured_field_value(line, HOST_FIELD_LABELS))
            .all(|host_value| {
                !summary_context
                    .recognition_dictionary
                    .people
                    .iter()
                    .any(|person| contains_known_name(host_value, &person.display_name))
            });
        let aliases_absent = [
            "瑞森",
            "Risa",
            "Reason",
            "Raison",
            "顺子",
            "伊丽",
            "阿牧",
            "杰克利",
        ]
        .iter()
        .all(|alias| !markdown.contains(alias));
        let required_sections = [
            "会议信息",
            "会议结论",
            "行动计划",
            "分部门工作汇报",
            "关键业务指标",
            "风险与安全要求",
            "下周复盘重点",
        ]
        .iter()
        .all(|section| markdown.contains(section));
        let pwa_not_falsely_abandoned = !compact.contains("放弃PWA")
            && !compact.contains("PWA已放弃")
            && !compact.contains("PWA项目已终止");
        let template_password_example_not_promoted = !markdown.contains("账号密码")
            && !markdown.contains("禁止索取密码")
            && !markdown.contains("禁止索取账号");
        let security_claim_warning = validated
            .validation
            .warnings
            .iter()
            .any(|warning| warning.code == "unsupported_security_claim");
        let fact_validation_status_matches_warnings = if validated.validation.warnings.is_empty() {
            validated.validation.status == SummaryFactValidationStatus::Passed
        } else {
            validated.validation.status == SummaryFactValidationStatus::NeedsReview
        };
        let structured_fact_validation_safely_handled = fact_validation_status_matches_warnings
            && if raw_template_password_example_present {
                security_claim_warning
                    && validated.validation.status == SummaryFactValidationStatus::NeedsReview
            } else {
                !security_claim_warning
            };
        let action_rows = markdown_table_rows(
            markdown,
            &[
                "部门/小组",
                "负责人",
                "行动任务",
                "截止时间",
                "验收标准",
                "当前状态",
            ],
        );
        let action_owners_grounded = !action_rows.is_empty()
            && action_rows.iter().all(|row| {
                row.get(1).is_some_and(|owner| {
                    is_not_mentioned_cell(owner)
                        || contains_known_name(owner, "Amu")
                        || contains_known_name(owner, "Rayson")
                })
            });
        let action_deadlines_grounded = !action_rows.is_empty()
            && action_rows.iter().all(|row| {
                let Some(task) = row.get(2) else { return false };
                let Some(deadline) = row.get(3) else {
                    return false;
                };
                is_not_mentioned_cell(deadline)
                    || (task.contains("产品体验") && deadline.contains("本周"))
                    || (task.contains("二次充值") && deadline.contains("今天"))
                    || ((task.contains("CGS") || task.contains("CJS") || task.contains("PWA"))
                        && (deadline.contains("9月7") || deadline.contains("09-07")))
                    || (task.contains("YouTube") && deadline.contains("下周"))
            });
        let action_acceptance_criteria_not_invented = !action_rows.is_empty()
            && action_rows
                .iter()
                .all(|row| row.get(4).is_some_and(|value| is_not_mentioned_cell(value)));
        let action_status_not_inferred = !action_rows.is_empty()
            && action_rows
                .iter()
                .all(|row| row.get(5).is_some_and(|value| is_not_mentioned_cell(value)));
        let unconfigured_roles_not_inferred =
            !has_inferred_parenthetical_person_annotation(markdown, &summary_context);
        let checks = serde_json::json!({
            "generation_completed": true,
            "summary_non_empty": markdown.chars().count() >= 500,
            "all_template_sections_present": required_sections,
            "meeting_date_is_2026_08_26": has_correct_date,
            "stale_2024_09_13_absent": !markdown.contains("2024-09-13") && !markdown.contains("2024年9月13日"),
            "structured_fact_validation_safely_handled": structured_fact_validation_safely_handled,
            "fact_conflicts_surface_needs_review": validated.validation.warnings.is_empty() || validated.validation.status == SummaryFactValidationStatus::NeedsReview,
            "unsupported_security_claim_surfaces_needs_review": !raw_template_password_example_present || security_claim_warning,
            "configured_aliases_normalized": aliases_absent,
            "unspecified_host_not_inferred": host_not_inferred,
            "pwa_not_falsely_abandoned": pwa_not_falsely_abandoned,
            "template_password_example_not_promoted": template_password_example_not_promoted,
            "unconfigured_roles_not_inferred": unconfigured_roles_not_inferred,
            "action_owners_grounded": action_owners_grounded,
            "action_deadlines_grounded": action_deadlines_grounded,
            "action_acceptance_criteria_not_invented": action_acceptance_criteria_not_invented,
            "action_status_not_inferred": action_status_not_inferred,
            "simplified_chinese_is_deterministic": crate::transcript_normalization::normalize_simplified_chinese_script(markdown, Some("zh-CN")) == *markdown,
        });
        let all_pass = checks
            .as_object()
            .unwrap()
            .values()
            .all(|value| value == &serde_json::Value::Bool(true));
        let evidence = serde_json::json!({
            "test_id": "MC-R04",
            "verdict": if all_pass { "PASS" } else { "FAIL" },
            "executed_at": Utc::now().to_rfc3339(),
            "source_meeting_directory": meeting_dir,
            "generation_source": if reuse_generation_evidence.is_some() { "saved_real_model_output_reprocessed" } else { "live_model" },
            "reused_generation_evidence": reuse_generation_evidence,
            "model": "qwen3.5:2b",
            "template_id": template.id,
            "template_version": template.version,
            "segment_count": segments.len(),
            "transcript_sha256": crate::summary::template_snapshot::sha256_text(&transcript),
            "meeting_context_id": summary_context.context_id,
            "meeting_context_sha256": summary_context.context_sha256,
            "summary_context_sha256": summary_context.sha256(),
            "verified_attendee_count": summary_context.verified_meeting_facts.attending.len(),
            "verified_absent_count": summary_context.verified_meeting_facts.absent.len(),
            "verified_host": summary_context.verified_meeting_facts.host,
            "configured_role_count": summary_context.verified_meeting_facts.attending.iter().filter(|person| person.role.is_some()).count(),
            "fixture_grounding_scope": {
                "allowed_named_action_owners": ["Amu", "Rayson"],
                "explicit_deadline_rules": [
                    "产品体验=本周",
                    "二次充值=今天",
                    "CGS/CJS/PWA=9月7日",
                    "YouTube=下周"
                ],
                "explicit_acceptance_criteria": [],
                "configured_roles": []
            },
            "chunk_count": chunk_count,
            "elapsed_seconds": elapsed_seconds,
            "raw_template_password_example_present": raw_template_password_example_present,
            "fact_validation": validated.validation,
            "checks": checks,
            "english_markdown": english_markdown,
            "final_simplified_chinese_markdown": markdown,
        });
        if let Some(parent) = evidence_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(
            &evidence_path,
            serde_json::to_vec_pretty(&evidence).unwrap(),
        )
        .unwrap();
        assert!(
            all_pass,
            "MC-R04 acceptance checks failed; inspect {evidence_path:?}"
        );
    }
}
