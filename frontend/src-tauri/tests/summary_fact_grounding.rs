use app_lib::meeting_context::validate_summary_markdown_with_transcript;

#[test]
fn spoken_meeting_identity_is_preserved_without_calendar_metadata() {
    let source = "林岚：现在是2026年9月12日14点，线上召开星桥项目评审会，预计14点30分结束。";
    let report = "会议主题：星桥项目评审会\n会议日期：2026年9月12日\n会议时间：2026年9月12日14点，预计14点30分结束";
    assert_eq!(app_lib::meeting_context::sanitize_generated_summary_with_transcript(report, source), report);
}

#[test]
fn discussed_dates_and_negated_meeting_identity_are_not_promoted() {
    for source in [
        "今天讨论2026年9月12日的项目截止日期。",
        "昨天召开星桥项目评审会，时间是2026年9月12日14点。",
        "今天不是星桥项目评审会，也不是2026年9月12日。",
        "[2026年9月12日14点] 讨论项目进度。",
    ] {
        let result = app_lib::meeting_context::sanitize_generated_summary_with_transcript(
            "会议主题：星桥项目评审会\n会议日期：2026年9月12日", source,
        );
        assert_eq!(result, "会议主题：会议未提及\n会议日期：会议未提及", "{source}");
    }
    let mixed = app_lib::meeting_context::sanitize_generated_summary_with_transcript(
        "会议日期：2026年9月12日；主持人：王明", "现在是2026年9月12日，线上召开评审会。",
    );
    assert!(!mixed.contains("王明"), "A supported date must not restore an unsupported host: {mixed}");
    let reversed = app_lib::meeting_context::sanitize_generated_summary_with_transcript(
        "主持人：王明；会议日期：2026年9月12日", "现在是2026年9月12日，线上召开评审会。",
    );
    assert!(!reversed.contains("王明"), "A supported date must not restore a preceding unsupported host: {reversed}");
}

#[test]
fn explicit_host_is_not_erased_when_recording_has_no_calendar_context() {
    let transcript = "我是产品负责人林岚，今天主持这次评审。";
    let generated = "主持人：林岚";
    let sanitized = app_lib::meeting_context::sanitize_generated_summary_with_transcript(generated, transcript);
    let result = validate_summary_markdown_with_transcript(&sanitized, None, transcript);
    assert_eq!(result.markdown, generated);
}

#[test]
fn host_mention_without_hosting_statement_does_not_become_meeting_metadata() {
    for transcript in ["林岚提出了一个问题。", "不是由林岚主持。", "昨天由林岚主持。"] {
        let result = app_lib::meeting_context::sanitize_generated_summary_with_transcript(
            "主持人：林岚", transcript,
        );
        assert_eq!(result, "主持人：会议未提及");
    }
}

#[test]
fn explicit_host_survives_role_annotation_without_trusting_the_role() {
    for host in ["林岚（产品负责人）", "林岚(虚构职务)", "林岚，产品负责人", "林岚, 虚构职务"] {
        let result = app_lib::meeting_context::sanitize_generated_summary_with_transcript(
            &format!("主持人：{host}"), "我是产品负责人林岚，今天主持。",
        );
        assert_eq!(result, "主持人：林岚");
    }
}

#[test]
fn supported_host_does_not_restore_another_unverified_field_on_the_same_line() {
    for report in [
        "主持人：林岚；会议日期：2037年1月1日",
        "主持人：林岚，产品负责人；会议日期：2037年1月1日",
        "会议日期：2037年1月1日；主持人：林岚",
    ] {
        let result = app_lib::meeting_context::sanitize_generated_summary_with_transcript(
            report, "我是林岚，今天主持。",
        );
        assert!(!result.contains("2037"), "{result}");
    }
}

#[test]
fn generated_grounding_masks_year_added_to_ambiguous_source() {
    for (source, generated) in [
        ("68年进入危机之后。利率为0-0.25%。", "1988年危机后，利率为0-0.25%。"),
        ("讨论08年发生的变化。", "二〇〇八年发生变化。"),
        ("讨论项目进度。", "项目于 2037 年完成。"),
    ] {
        let result = validate_summary_markdown_with_transcript(generated, None, source);
        assert!(result.markdown.contains("年份待核对"), "{}", result.markdown);
        assert!(result.validation.warnings.iter().any(|w| w.code == "unsupported_year"));
    }
}

#[test]
fn generated_grounding_preserves_explicit_years_and_business_numbers() {
    for source in ["2008年危机后利率为0-0.25%。", "二〇〇八年危机后利率为0-0.25%。", "In 2008 the rate was 0-0.25%."] {
        let generated = "2008 年危机后利率为0-0.25%。";
        let result = validate_summary_markdown_with_transcript(generated, None, source);
        assert_eq!(result.markdown, generated);
        assert!(!result.validation.warnings.iter().any(|w| w.code == "unsupported_year"));
    }
    let generated = "国债价格100.5312，期限30年，利率2.814%。";
    assert_eq!(validate_summary_markdown_with_transcript(generated, None, generated).markdown, generated);
}

#[test]
fn generated_grounding_does_not_use_substrings_as_year_evidence() {
    let result = validate_summary_markdown_with_transcript("2037年完成。", None, "编号120379，年份尚未确定。");
    assert!(result.markdown.contains("年份待核对"));
}

#[test]
fn generated_grounding_removes_only_unsupported_acronym_annotation() {
    for (source, generated, expected) in [
        ("都是通过国债市场或者NBS市场来影响的。", "通过国债或NBS（货币市场）市场影响。", "通过国债或NBS市场影响。"),
        ("连接ABC。", "连接ABC (虚构的系统)。", "连接ABC。"),
    ] {
        let result = validate_summary_markdown_with_transcript(generated, None, source);
        assert_eq!(result.markdown, expected);
        assert!(result.validation.warnings.iter().any(|w| w.code == "unsupported_acronym_expansion"));
    }
}

#[test]
fn generated_grounding_preserves_explicit_acronym_annotation() {
    let generated = "使用ABC（音频处理器）和API (application programming interface)。";
    let source = "ABC是音频处理器。API means application programming interface.";
    let result = validate_summary_markdown_with_transcript(generated, None, source);
    assert_eq!(result.markdown, generated);
    assert!(!result.validation.warnings.iter().any(|w| w.code == "unsupported_acronym_expansion"));
}

#[test]
fn generated_grounding_preserves_code_and_ascii_function_arguments() {
    let generated = "代码 `ABC(参数)` 和 `2037年`。\n```text\nABC(参数) 2037年\n```\n~~~~text\n```\nABC(参数) 2037年\n~~~~\n调用ABC(value)。\n";
    assert_eq!(validate_summary_markdown_with_transcript(generated, None, "演示代码。").markdown, generated);
}

#[test]
fn generated_grounding_keeps_markdown_and_supported_year_from_verified_context() {
    let context = serde_json::from_value(serde_json::json!({
        "context_id": "test", "context_sha256": "test",
        "verified_meeting_facts": {"started_at": "2026-09-09T08:00:00Z"},
        "recognition_dictionary": {}
    })).unwrap();
    let result = validate_summary_markdown_with_transcript("2026年，使用**ABC**（虚构的系统）。\n", Some(&context), "使用ABC。");
    assert_eq!(result.markdown, "2026年，使用**ABC**。\n");
    assert!(!result.validation.warnings.iter().any(|w| w.code == "unsupported_year"));
}

#[test]
fn generated_omission_warnings_survive_read_revalidation() {
    let source = "讨论08年的ABC。";
    let generated = validate_summary_markdown_with_transcript(
        "2008年使用ABC（虚构的系统）。", None, source,
    );
    let mut reread = validate_summary_markdown_with_transcript(&generated.markdown, None, source);
    // The native read/restore paths retain omission notices from stored metadata
    // after rechecking the current text. Editor-save requests do not use this step.
    reread.validation.retain_saved_omission_warnings(&serde_json::json!({
        "factValidation": &generated.validation,
    }));
    for code in ["unsupported_year", "unsupported_acronym_expansion"] {
        assert!(generated.validation.warnings.iter().any(|warning| warning.code == code));
        assert!(reread.validation.warnings.iter().any(|warning| warning.code == code),
            "read revalidation lost {code} after generated content was omitted");
    }
}

#[test]
fn saved_omission_notices_do_not_replace_fresh_validation_or_trust_message_keys() {
    let mut current = validate_summary_markdown_with_transcript("项目进展。", None, "项目进展。").validation;
    let before = current.clone();
    let stored = serde_json::json!({"factValidation": {
        "status": "passed", "aliasesNormalized": true,
        "sourceEvidence": {"meetingId": "untrusted-old-source"},
        "warnings": [
            {"code": "unsupported_year", "messageKey": "untrusted.message"},
            {"code": "unknown_warning", "messageKey": "untrusted.message"},
            {"code": "missing_transcript_evidence", "messageKey": "untrusted.message"}
        ]
    }});
    current.retain_saved_omission_warnings(&stored);
    current.retain_saved_omission_warnings(&stored);
    assert_eq!(current.warning_count, before.warning_count + 1);
    assert_eq!(current.source_evidence, before.source_evidence);
    assert_eq!(current.aliases_normalized, before.aliases_normalized);
    assert_eq!(current.status, app_lib::meeting_context::SummaryFactValidationStatus::NeedsReview);
    assert!(before.warnings.iter().all(|warning| current.warnings.contains(warning)));
    assert!(current.warnings.iter().any(|warning| warning.code == "unsupported_year"
        && warning.message_key == "summary:factValidation.unsupportedYear"));
    assert!(!current.warnings.iter().any(|warning| warning.code == "unknown_warning"
        || warning.code == "missing_transcript_evidence"));
}
