//! Keep generated headings and task rows separate when Markdown is parsed by the editor.
//! This changes block separators only; it does not rewrite meeting facts.

/// Keep a template section from being mistaken for the report's removable H1 title.
pub fn protect_template_headings(markdown: &str, titles: &[&str]) -> String {
    let mut fence: Option<(char, usize)> = None;
    markdown.split('\n').map(|line| {
        let trimmed = line.trim();
        let marker = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'));
        let count = marker.map(|c| trimmed.chars().take_while(|ch| *ch == c).count()).unwrap_or(0);
        if let Some((character, length)) = fence {
            if marker == Some(character) && count >= length && trimmed.chars().all(|c| c == character) {
                fence = None;
            }
            return line.to_owned();
        }
        if line.starts_with("    ") || line.starts_with('\t') { return line.to_owned(); }
        if count >= 3 {
            fence = Some((marker.unwrap(), count));
            return line.to_owned();
        }
        if let Some(title) = trimmed.strip_prefix("# ") {
            if titles.iter().any(|expected| expected.trim().eq_ignore_ascii_case(title.trim())) {
                return format!("## {}", title.trim());
            }
        }
        line.to_owned()
    }).collect::<Vec<_>>().join("\n")
}

pub fn preserve_report_blocks(markdown: &str) -> String {
    let mut output: Vec<String> = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut after_heading = false;
    for line in markdown.lines() {
        if line.starts_with("    ") || line.starts_with('\t') {
            output.push(line.to_owned());
            after_heading = false;
            continue;
        }
        let trimmed = line.trim();
        let marker = trimmed.chars().next().filter(|ch| matches!(ch, '`' | '~'));
        let marker_count = marker.map(|ch| trimmed.chars().take_while(|c| *c == ch).count()).unwrap_or(0);
        if let Some((character, length)) = fence {
            output.push(line.to_owned());
            if marker == Some(character) && marker_count >= length &&
                trimmed.chars().all(|ch| ch == character) { fence = None; }
            continue;
        }
        if marker_count >= 3 {
            fence = Some((marker.unwrap(), marker_count));
            output.push(line.to_owned());
            after_heading = false;
            continue;
        }
        let heading = (trimmed.starts_with("**") && trimmed.ends_with("**") && trimmed.len() > 4)
            || (trimmed.starts_with("__") && trimmed.ends_with("__") && trimmed.len() > 4)
            || (trimmed.starts_with('#') && trimmed.trim_start_matches('#').starts_with(' '));
        let task = ["任务：", "任务:", "Task:", "task:", "Action item:", "action item:"]
            .iter().any(|prefix| trimmed.starts_with(prefix));
        let field = trimmed.find([':', '：']).is_some_and(|index| {
            let label = &trimmed[..index];
            !label.starts_with(['-', '*', '|', '>']) && label.chars().count() <= 24
                && !label.contains(['。', '！', '？', '.', '!', '?'])
        });
        if !trimmed.is_empty() && (heading || after_heading || (field && !task))
            && output.last().is_some_and(|previous| !previous.is_empty()) {
            output.push(String::new());
        }
        output.push(if task { format!("- {trimmed}") } else { line.to_owned() });
        after_heading = heading;
    }
    output.join("\n").trim_matches(['\r', '\n']).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_section_h1_is_not_a_document_title() {
        let markdown = "# 会议信息\n主持人：林岚\n\n# 核心结论\n采用小范围试点";
        assert_eq!(protect_template_headings(markdown, &["会议信息", "核心结论"]),
            "## 会议信息\n主持人：林岚\n\n## 核心结论\n采用小范围试点");
    }

    #[test]
    fn keeps_real_title_and_code_examples() {
        let markdown = "# 星桥评审\n\n## 会议信息\n林岚\n\n```text\n# 会议信息\n```\n\n    # 会议信息";
        assert_eq!(protect_template_headings(markdown, &["会议信息"]), markdown);
    }

    #[test]
    fn separates_real_generated_heading_fields_and_adjacent_tasks() {
        let source = "**会议信息**\n主题：评审\n主持人：林岚\n\n**行动项与责任**\n任务：修复；负责人：周宁；截止：9月15日18点\n任务：回归；负责人：陈悦；依赖：修复";
        let formatted = preserve_report_blocks(source);
        assert!(formatted.contains("**会议信息**\n\n主题：评审\n\n主持人：林岚"));
        assert!(formatted.contains("**行动项与责任**\n\n- 任务：修复"));
        assert!(formatted.contains("9月15日18点\n- 任务：回归"));
        assert!(formatted.contains("负责人：陈悦；依赖：修复"));
        assert_eq!(preserve_report_blocks(&formatted), formatted);
    }

    #[test]
    fn keeps_code_samples_and_existing_lists_unchanged() {
        let source = "### Example\n\n~~~text\n**标题**\n任务：代码示例\n~~~\n\n- Task: release; Owner: Jo\n- Task: review; Owner: Lee";
        assert_eq!(preserve_report_blocks(source), source);
        assert_eq!(preserve_report_blocks("    任务：代码示例"), "    任务：代码示例");
    }
}
