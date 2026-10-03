//! Shared field recognition for masking, evidence checking and restoration.
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use once_cell::sync::Lazy;
use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryTraceField { Owner, Time, Dependency, Acceptance, Status, Blocker, Criteria, Escalation }

pub const ACTION_FIELDS: &[SummaryTraceField] = &[
    SummaryTraceField::Owner, SummaryTraceField::Time, SummaryTraceField::Dependency,
    SummaryTraceField::Acceptance, SummaryTraceField::Status, SummaryTraceField::Blocker,
    SummaryTraceField::Criteria, SummaryTraceField::Escalation,
];
pub const TASK_LABELS: &[&str] = &["行动项", "行动任务", "任务", "事项", "决策", "结论", "action item", "action", "task", "decision", "deliverable"];

pub fn field_labels(field: SummaryTraceField) -> &'static [&'static str] {
    match field {
        SummaryTraceField::Owner => &["负责人", "负责人/部门", "负责人／部门", "责任人", "owner", "owner/department", "assignee"],
        SummaryTraceField::Time => &["时间", "截止时间", "截止日期", "完成时间", "截止", "deadline", "due date", "time"],
        SummaryTraceField::Dependency => &["依赖", "前提条件", "dependency", "dependencies", "prerequisite", "prerequisites"],
        SummaryTraceField::Acceptance => &["验收标准", "验收条件", "acceptance criteria", "acceptance criterion"],
        SummaryTraceField::Status => &["当前状态", "任务状态", "状态", "status", "current status"],
        SummaryTraceField::Blocker => &["卡点", "阻碍", "blocker", "blockers"],
        SummaryTraceField::Criteria => &["判断口径", "criteria"],
        SummaryTraceField::Escalation => &["升级条件", "escalation condition"],
    }
}

pub fn normalize_label(label: &str) -> String {
    label.nfkc().flat_map(char::to_lowercase).filter(|c| !c.is_whitespace() && *c != '*').collect()
}
pub fn is_task_label(label: &str) -> bool {
    let normalized = normalize_label(label);
    TASK_LABELS.iter().any(|alias| normalize_label(alias) == normalized)
}
pub fn label_fields(label: &str) -> Vec<SummaryTraceField> {
    let normalized = normalize_label(label);
    if matches!(normalized.as_str(), "依赖或卡点" | "dependency/blocker" | "dependencies/blockers" | "依赖/卡点") {
        return vec![SummaryTraceField::Dependency, SummaryTraceField::Blocker];
    }
    if let Some(field) = ACTION_FIELDS.iter().find(|field| field_labels(**field).iter().any(|alias| normalize_label(alias) == normalized)) {
        return vec![*field];
    }
    // Only explicit known components qualify; an unknown metric is not an acceptance criterion.
    let parts = normalized.split('/').collect::<Vec<_>>();
    if parts.len() > 1 {
        let fields = parts.iter().map(|part| label_fields(part)).collect::<Vec<_>>();
        if fields.iter().all(|fields| fields.len() == 1) {
            return fields.into_iter().flatten().collect();
        }
    }
    Vec::new()
}

pub fn table_cells(line: &str) -> Vec<String> {
    let line = line.trim();
    if line.len() < 2 || !line.starts_with('|') || !line.ends_with('|') { return Vec::new(); }
    let inner = &line[1..line.len()-1];
    let mut cells = Vec::new();
    let mut start = 0;
    let mut escaped = false;
    for (index, c) in inner.char_indices() {
        if c == '|' && !escaped { cells.push(inner[start..index].trim().to_owned()); start = index + 1; }
        escaped = c == '\\' && !escaped;
    }
    cells.push(inner[start..].trim().to_owned());
    cells
}
pub fn table_separator(cells: &[String]) -> bool {
    !cells.is_empty() && cells.iter().all(|cell| cell.contains('-') && cell.chars().all(|c| matches!(c, '-' | ':' | ' ')))
}
pub fn table_header(lines: &[&str], index: usize) -> bool {
    let cells = table_cells(lines[index]);
    lines.get(index+1).is_some_and(|line| {
        let next = table_cells(line); !cells.is_empty() && next.len() == cells.len() && table_separator(&next)
    })
}

/// Byte ranges refer to the original line, including any value formatting.
pub fn inline_labels(line: &str) -> Vec<(&str, usize, usize)> {
    static LABEL: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?:^|[;；|])\s*(?:[-*]\s+)?(?P<label>[^:：;；|\n]+?)\s*[:：]\s*").unwrap());
    LABEL.captures_iter(line).filter_map(|capture| {
        let start = capture.get(0)?.end();
        let end = start + line[start..].find([';', '；', '|']).unwrap_or(line.len()-start);
        let value = &line[start..end];
        let leading = value.len() - value.trim_start().len();
        let trailing = value.trim_end().len();
        (leading < trailing).then_some((capture.name("label")?.as_str(), start+leading, start+trailing))
    }).collect()
}
pub fn inline_field_ranges(line: &str, labels: &[&str]) -> Vec<(usize, usize)> {
    inline_labels(line).into_iter().filter(|(label, _, _)| labels.iter().any(|alias| normalize_label(label) == normalize_label(alias)))
        .map(|(_, start, end)| (start, end)).collect()
}

pub fn cell_field_values<'a>(fields: &[SummaryTraceField], value: &'a str) -> Vec<(SummaryTraceField, &'a str)> {
    if fields.len() == 1 { return vec![(fields[0], value.trim().trim_matches('*').trim())]; }
    let explicit = inline_labels(value).into_iter().flat_map(|(label, start, end)| {
        label_fields(label).into_iter().filter(|field| fields.contains(field)).map(move |field| (field, value[start..end].trim().trim_matches('*').trim()))
    }).collect::<Vec<_>>();
    if explicit.is_empty() && fields != [SummaryTraceField::Dependency, SummaryTraceField::Blocker] {
        let parts = value.split('/').map(str::trim).collect::<Vec<_>>();
        if parts.len() == fields.len() && parts.iter().all(|part| !part.is_empty()) {
            return fields.iter().zip(parts).map(|(field, value)| (*field, value)).collect();
        }
    }
    if !explicit.is_empty() && fields == [SummaryTraceField::Dependency, SummaryTraceField::Blocker] {
        let complete = value.split([';', '；']).filter(|part| !part.trim().is_empty()).all(|part| {
            let labels = inline_labels(part);
            labels.len() == 1 && !label_fields(labels[0].0).is_empty()
                && label_fields(labels[0].0).iter().all(|field| fields.contains(field))
        });
        if complete { return explicit; }
        return fields.iter().map(|field| (*field, value)).collect();
    }
    fields.iter().map(|field| (*field, explicit.iter().find(|(found,_)| found == field).map_or(value, |(_,value)| *value))).collect()
}

pub fn ambiguous_dependency_blocker(fields: &[SummaryTraceField], value: &str) -> bool {
    fields == [SummaryTraceField::Dependency, SummaryTraceField::Blocker] && inline_labels(value).is_empty()
}

pub fn advance_fence(fence: &mut Option<(char, usize)>, line: &str) -> bool {
    let line = line.trim_start(); let marker = line.chars().next().unwrap_or(' ');
    let count = line.chars().take_while(|c| *c == marker).count();
    if !matches!(marker, '`' | '~') || count < 3 { return false; }
    match *fence {
        None => *fence = Some((marker, count)),
        Some((opening, length)) if marker == opening && count >= length && line[count..].trim().is_empty() => *fence = None,
        _ => {}
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliases_are_exact_and_keep_ambiguous_metrics_unclassified() {
        for label in ["截止时间", "截止日期", " ** Due  Date ** ", "Ｄｕｅ Ｄａｔｅ"] { assert_eq!(label_fields(label), vec![SummaryTraceField::Time]); }
        assert_eq!(label_fields(" ** 责任人 ** "), vec![SummaryTraceField::Owner]);
        assert_eq!(label_fields("依赖或卡点"), vec![SummaryTraceField::Dependency, SummaryTraceField::Blocker]);
        assert_eq!(label_fields("截止时间/验收标准"), vec![SummaryTraceField::Time, SummaryTraceField::Acceptance]);
        for label in ["Success Metric", "自定义截止时间说明", "状态说明"] { assert!(label_fields(label).is_empty()); }
        assert!(is_task_label(" ** Deliverable ** ")); assert!(!is_task_label("Deliverable Notes"));
    }
    #[test]
    fn table_header_requires_a_real_separator_and_escaped_pipes_keep_their_column() {
        let rows = ["| 行动任务 | 负责人 |", "| --- | --- |", "| 当前状态 | 未知 |", "| 负责人 | 未知 |"];
        assert!(table_header(&rows, 0)); assert!(!table_header(&rows, 2));
        assert_eq!(table_cells(r"| 甲\|乙 | **保留** |"), vec![r"甲\|乙", "**保留**"]);
        assert!(!table_separator(&["".into(), "".into()]));
        assert!(table_cells("|").is_empty());
    }
    #[test]
    fn incomplete_combined_cell_never_drops_its_unverified_tail() {
        let fields = label_fields("依赖或卡点");
        let mixed = "依赖：测试环境就绪；虚构卡点";
        let values = cell_field_values(&fields, mixed);
        assert_eq!(values.len(), 2);
        assert!(values.iter().all(|(_, value)| *value == mixed));
        let explicit = cell_field_values(&fields, "依赖：测试环境就绪；卡点：供应商审批");
        assert_eq!(explicit, vec![(SummaryTraceField::Dependency, "测试环境就绪"), (SummaryTraceField::Blocker, "供应商审批")]);
    }
    #[test]
    fn inline_labels_have_boundaries_and_do_not_match_a_prose_suffix() {
        let line = "- **任务**：整理报告； **Due  Date**: 周五; CustomOwner: 原样";
        let ranges = inline_field_ranges(line, field_labels(SummaryTraceField::Time));
        assert_eq!(&line[ranges[0].0..ranges[0].1], "周五");
        assert!(inline_field_ranges(line, field_labels(SummaryTraceField::Owner)).is_empty());
        assert!(inline_field_ranges("这不是负责人：字段", field_labels(SummaryTraceField::Owner)).is_empty());
    }
}
