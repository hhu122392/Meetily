use serde_json::{json, Value};
use url::Url;

// Keep the existing conservative automatic-delivery budget, including JSON escaping.
pub(super) const MAX_CARD_BYTES: usize = 19_000;

fn single_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn heading(line: &str) -> Option<String> {
    let line = line.trim();
    let hashes = line.bytes().take_while(|c| *c == b'#').count();
    if (1..=6).contains(&hashes) && line.as_bytes().get(hashes) == Some(&b' ') {
        return Some(line[hashes..].trim().trim_end_matches('#').trim().to_owned());
    }
    line.strip_prefix("**").and_then(|s| s.strip_suffix("**"))
        // A standalone bold decision is content, not necessarily an empty section heading.
        .filter(|s| matches!(*s,
            "摘要" | "会议摘要" | "概述" | "关键决策" | "关键决定" | "决议" | "会议决议" |
            "行动项目" | "行动项" | "待办事项" | "讨论要点" | "讨论内容" | "下一步" |
            "日期" | "出席人员" | "参会人员" | "昨天" | "今天" | "阻碍事项" | "风险" |
            "备注" | "其他事项"))
        .map(str::to_owned)
}

fn empty_placeholder(line: &str) -> bool {
    matches!(line.trim(), "本部分未注明任何内容。" | "本部分未注明任何内容" | "No content noted for this section.")
}

fn sections(summary: &str) -> Vec<(String, String)> {
    let mut result = Vec::new();
    let mut title = String::new();
    let mut lines = Vec::new();
    let mut fence: Option<char> = None;
    for line in summary.lines() {
        let trimmed = line.trim();
        let marker = if trimmed.starts_with("```") { Some('`') }
            else if trimmed.starts_with("~~~") { Some('~') } else { None };
        if let Some(marker) = marker {
            if fence == Some(marker) { fence = None; }
            else if fence.is_none() { fence = Some(marker); }
            lines.push(line.to_owned());
            continue;
        }
        if fence.is_none() {
            if let Some(next_title) = heading(line) {
                let body = lines.join("\n").trim().to_owned();
                if !body.is_empty() { result.push((title, body)); }
                title = next_title;
                lines.clear();
                continue;
            }
            if empty_placeholder(line) { continue; }
        }
        // Keep at most one blank line between paragraphs.
        if !trimmed.is_empty() || lines.last().is_some_and(|s: &String| !s.trim().is_empty()) {
            lines.push(line.to_owned());
        }
    }
    let body = lines.join("\n").trim().to_owned();
    if !body.is_empty() { result.push((title, body)); }
    result
}

fn safe_markdown(text: &str) -> String {
    // AI/transcript text must never become a Lark mention or other executable card tag.
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

// Parse only unambiguous pipe tables. Escaped pipes and code spans are cell content.
fn table_cells(line: &str) -> Option<Vec<String>> {
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut chars = line.trim().chars().peekable();
    let mut code_ticks = 0;
    let mut separators = 0;
    while let Some(c) = chars.next() {
        if c == '\\' {
            cell.push(c);
            if let Some(next) = chars.next() { cell.push(next); }
        } else if c == '`' {
            let mut count = 1;
            while chars.peek() == Some(&'`') { chars.next(); count += 1; }
            if code_ticks == 0 { code_ticks = count; }
            else if code_ticks == count { code_ticks = 0; }
            cell.extend(std::iter::repeat('`').take(count));
        } else if c == '|' && code_ticks == 0 {
            cells.push(cell.trim().to_owned());
            cell.clear();
            separators += 1;
        } else {
            cell.push(c);
        }
    }
    if separators == 0 || code_ticks != 0 { return None; }
    cells.push(cell.trim().to_owned());
    if line.trim().starts_with('|') { cells.remove(0); }
    // An escaped final pipe belongs to the last cell.
    if cells.last().is_some_and(String::is_empty) && line.trim().ends_with('|') { cells.pop(); }
    (!cells.is_empty()).then_some(cells)
}

fn table_separator(cells: &[String]) -> bool {
    cells.iter().all(|cell| {
        let dashes = cell.trim().trim_start_matches(':').trim_end_matches(':');
        dashes.len() >= 3 && dashes.bytes().all(|c| c == b'-')
    })
}

fn tables_as_lists(summary: &str) -> String {
    let lines: Vec<_> = summary.lines().collect();
    let mut output = Vec::new();
    let mut i = 0;
    let mut fence: Option<(char, usize)> = None;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        let first = trimmed.chars().next().unwrap_or(' ');
        let run = trimmed.chars().take_while(|c| *c == first).count();
        if matches!(first, '`' | '~') && run >= 3 {
            if let Some((marker, length)) = fence {
                if first == marker && run >= length && trimmed[run..].trim().is_empty() { fence = None; }
            } else { fence = Some((first, run)); }
            output.push(lines[i].to_owned());
            i += 1;
            continue;
        }
        if fence.is_none() && !lines[i].starts_with("    ") && !lines[i].starts_with('\t') && i + 1 < lines.len() {
            if let (Some(headers), Some(separator)) = (table_cells(lines[i]), table_cells(lines[i + 1])) {
                if headers.len() == separator.len() && table_separator(&separator) {
                    let mut end = i + 2;
                    let mut rows = Vec::new();
                    while end < lines.len() {
                        let Some(row) = table_cells(lines[end]) else { break; };
                        rows.push(row);
                        end += 1;
                    }
                    // Never guess a field mapping or drop surplus cells in malformed output.
                    if !rows.is_empty() && rows.iter().all(|row| row.len() == headers.len()) {
                        output.push(String::new());
                        for row in rows {
                            for (column, (header, value)) in headers.iter().zip(row).enumerate() {
                                let header = header.trim_matches('*').trim_matches('`');
                                let label = if header.is_empty() { format!("字段 {}", column + 1) } else { header.to_owned() };
                                let value = value.replace("<br />", " / ").replace("<br/>", " / ").replace("<br>", " / ");
                                let value = if value.is_empty() { "—" } else { &value };
                                output.push(format!("{}**{}**：{}", if column == 0 { "• " } else { "  " }, label, value));
                            }
                            output.push(String::new());
                        }
                    } else {
                        output.extend(lines[i..end].iter().map(|s| (*s).to_owned()));
                    }
                    i = end;
                    continue;
                }
            }
        }
        output.push(lines[i].to_owned());
        i += 1;
    }
    output.join("\n")
}

fn safe_meeting_url(value: &str) -> Option<&str> {
    let value = value.trim();
    let url = Url::parse(value).ok()?;
    let host = url.host_str()?;
    (url.scheme() == "https" && url.username().is_empty() && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && (host == "larksuite.com" || host.ends_with(".larksuite.com")))
        .then_some(value)
}

// Extract a small, source-faithful overview without another model or remote API call.
fn overview_lines(body: &str) -> Vec<String> {
    let lines: Vec<_> = body.lines().collect();
    let mut result = Vec::new();
    let mut i = 0;
    let mut fence: Option<(char, usize)> = None;
    while i < lines.len() {
        let line = lines[i].trim();
        let marker = line.chars().next().unwrap_or(' ');
        let run = line.chars().take_while(|c| *c == marker).count();
        if matches!(marker, '`' | '~') && run >= 3 {
            if let Some((open, count)) = fence {
                if marker == open && run >= count && line[run..].trim().is_empty() { fence = None; }
            } else { fence = Some((marker, run)); }
            i += 1;
            continue;
        }
        if fence.is_some() || line.is_empty() { i += 1; continue; }
        if i + 1 < lines.len() {
            if let (Some(headers), Some(separator)) = (table_cells(line), table_cells(lines[i + 1])) {
                if headers.len() == separator.len() && table_separator(&separator) {
                    i += 2;
                    while i < lines.len() {
                        let Some(row) = table_cells(lines[i]) else { break; };
                        if !table_separator(&row) {
                            let fields = row.iter().enumerate().filter(|(_, value)| !value.is_empty()).map(|(column, value)| {
                                let label = headers.get(column).filter(|h| !h.trim().is_empty()).cloned().unwrap_or_else(|| format!("字段 {}", column + 1));
                                format!("{}：{}", label.trim_matches('*'), value)
                            }).collect::<Vec<_>>().join("；");
                            if !fields.is_empty() { result.push(format!("- {fields}")); }
                        }
                        i += 1;
                    }
                    continue;
                }
            }
        }
        // A malformed model-generated table must not leak pipe-table syntax to Lark.
        if !table_cells(line).is_some_and(|cells| table_separator(&cells)) {
            result.push(line.to_owned());
        }
        i += 1;
    }
    result.into_iter().map(|s| s.replace("\\|", " / ").replace('|', " / ")
        .replace("<br />", "；").replace("<br/>", "；").replace("<br>", "；")).collect()
}

pub(super) fn summary_overview(markdown: &str) -> String {
    let without_metadata = markdown.lines().filter(|line| {
        let line = line.trim();
        !line.starts_with("会议时间：") && !line.starts_with("本地会议 ID：")
    }).collect::<Vec<_>>().join("\n");
    let mut parts = sections(&without_metadata);
    parts.retain(|(title, _)| !matches!(title.as_str(), "日期" | "出席人员" | "参会人员"));
    parts.sort_by_key(|(title, _)| {
        let title = title.to_lowercase();
        if ["摘要", "概述", "总结", "summary", "overview"].iter().any(|s| title.contains(s)) { 0 }
        else if ["决策", "决议", "决定", "decision"].iter().any(|s| title.contains(s)) { 1 }
        else if ["行动", "待办", "下一步", "action", "next step"].iter().any(|s| title.contains(s)) { 2 }
        else { 3 }
    });
    let selected = parts.into_iter().filter_map(|(title, body)| {
        let lines = overview_lines(&body).into_iter().take(2).collect::<Vec<_>>().join("\n");
        if lines.is_empty() { return None; }
        let short: String = lines.chars().take(220).collect();
        let short = if short.len() < lines.len() { format!("{short}…") } else { short };
        Some(if title.is_empty() { short } else { format!("## {}\n{short}", title.replace('|', " / ").chars().take(48).collect::<String>()) })
    }).take(3).collect::<Vec<_>>().join("\n\n");
    if selected.is_empty() { "未提取到明确要点，请检查详细纪要。".into() } else { selected }
}

pub(super) fn validate_message_edit(title: &str, summary: &str) -> Result<(), String> {
    if title.trim().is_empty() || title.chars().count() > 100 || title.contains(['\n', '\r']) {
        return Err("消息标题不能为空，最多 100 字，且不能换行。".into());
    }
    if summary.trim().is_empty() || summary.chars().count() > 1500 {
        return Err("消息概要不能为空，最多 1500 字；详细内容请保留在 Markdown 文件中。".into());
    }
    if summary.lines().any(|line| table_cells(line).is_some_and(|cells| table_separator(&cells))
        || line.trim().starts_with('|') || line.to_lowercase().contains("<table")) {
        return Err("消息概要不使用表格，请改为文字或分项列表。".into());
    }
    Ok(())
}

pub(super) fn meeting_card(
    topic: Option<&str>, meeting_no: Option<&str>, started_at: i64,
    meeting_url: Option<&str>, summary_title: &str, summary: &str,
) -> Result<Value, String> {
    if summary.trim().is_empty() { return Err("纪要内容为空，请先生成纪要。".into()); }
    let topic = single_line(topic.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| if summary_title.trim().is_empty() { "未记录原会议名称" } else { summary_title }));
    let number = single_line(meeting_no.filter(|s| !s.trim().is_empty()).unwrap_or("未记录"));
    let time = chrono::DateTime::from_timestamp(started_at, 0)
        .ok_or("录音开始时间无效，无法生成会议标识。")?
        .with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).unwrap())
        .format("%Y-%m-%d %H:%M").to_string();
    let mut elements = vec![json!({"tag":"div","fields":[
        {"is_short":true,"text":{"tag":"plain_text","content":format!("录音开始（北京时间）\n{time}")}},
        {"is_short":true,"text":{"tag":"plain_text","content":format!("会议号\n{number}")}}
    ]})];
    let summary_title = single_line(summary_title);
    if !summary_title.is_empty() && summary_title != topic {
        elements.push(json!({"tag":"div","text":{"tag":"plain_text","content":format!("摘要主题：{summary_title}")}}));
    }
    elements.push(json!({"tag":"hr"}));
    let sections = sections(&tables_as_lists(summary));
    if sections.is_empty() {
        elements.push(json!({"tag":"div","text":{"tag":"plain_text","content":"本次纪要没有提取到有效内容，请检查转写结果。"}}));
    }
    for (title, body) in sections {
        let body = safe_markdown(&body);
        let content = if title.is_empty() { body } else {
            // Heading markers are not supported consistently by Lark's lightweight Markdown.
            format!("**{}**\n{}", safe_markdown(&title).replace('*', ""), body)
        };
        elements.push(json!({"tag":"div","text":{"tag":"lark_md","content":content}}));
    }
    elements.push(json!({"tag":"hr"}));
    if let Some(link) = meeting_url.and_then(safe_meeting_url) {
        elements.push(json!({"tag":"action","actions":[{
            "tag":"button","text":{"tag":"plain_text","content":"打开原会议链接"},"type":"default","url":link
        }]}));
    } else {
        elements.push(json!({"tag":"note","elements":[{"tag":"plain_text","content":"原会议链接未记录，请通过会议号核对。"}]}));
    }
    elements.push(json!({"tag":"note","elements":[{"tag":"plain_text","content":"由 Meetily 根据转写生成"}]}));
    let card = json!({
        "config":{"wide_screen_mode":true},
        "header":{"template":"blue","title":{"tag":"plain_text","content":format!("会议纪要｜{topic}")}},
        "elements":elements
    });
    if serde_json::to_vec(&card).map_err(|_| "无法编码纪要卡片。")?.len() > MAX_CARD_BYTES {
        return Err("纪要卡片超过自动发送的 19 KB 限制，请精简后手动发送；内容未截断。".into());
    }
    Ok(card)
}

// Persist the exact content/type together; legacy queued messages stay plain text.
pub(super) fn delivery_body(
    chat: &str, uuid: &str, message_type: &str, content: Option<&str>, legacy_text: &str,
) -> Result<Value, String> {
    let content = match message_type {
        "text" => json!({"text":legacy_text}).to_string(),
        "interactive" => {
            let content = content.ok_or("纪要卡片内容缺失，已停止发送。")?;
            let card: Value = serde_json::from_str(content).map_err(|_| "纪要卡片格式损坏，已停止发送。")?;
            if !card["header"].is_object() || !card["elements"].is_array() || content.len() > MAX_CARD_BYTES {
                return Err("纪要卡片结构或大小异常，已停止发送。".into());
            }
            content.to_owned()
        }
        _ => return Err("不支持的纪要消息格式，已停止发送。".into()),
    };
    Ok(json!({"receive_id":chat,"uuid":uuid,"msg_type":message_type,"content":content}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overview_compacts_table_rows_and_never_contains_tables() {
        let source = "## 待办事项\n| 事项 | 负责人 | 截止 |\n| --- | --- | --- |\n| 接口联调 | 张三 | 周五 |\n| 回归测试 | 李四 | 下周 |\n| 详细任务 | 王五 | 下月 |\n\n## 附录\n```text\n| 不应发送 | 代码 |\n```";
        let overview = summary_overview(source);
        assert!(overview.contains("事项：接口联调；负责人：张三；截止：周五"));
        assert!(overview.contains("事项：回归测试；负责人：李四；截止：下周"));
        assert!(!overview.contains("详细任务"));
        assert!(!overview.contains("不应发送"));
        assert!(!overview.contains('|'));
        let malformed = summary_overview("## 摘要\n| A | B |\n| --- | --- |\n| 值 | 内容 | 多余字段 |\n\n普通 | 段落");
        assert!(malformed.contains("多余字段"));
        assert!(!malformed.contains('|'));
        let card = meeting_card(None, None, 0, None, "迭代发布评审", &overview).unwrap();
        assert_eq!(card["header"]["title"]["content"], "会议纪要｜迭代发布评审");
        assert!(!card.to_string().contains('|'));
    }

    #[test]
    fn overview_prioritizes_key_points_and_omits_metadata_and_long_detail() {
        let text = format!("# 项目周会\n会议时间：2026-10-19\n本地会议 ID：private-id\n## 详细讨论\n{}\n## 摘要\n交付日期已确定。\n## 决议\n本周五发布。\n## 待办事项\n小明负责验收。", "背景讨论".repeat(1000));
        let result = summary_overview(&text);
        assert!(result.contains("交付日期已确定"));
        assert!(result.contains("本周五发布"));
        assert!(result.contains("小明负责验收"));
        assert!(!result.contains("private-id"));
        assert!(!result.contains("背景讨论"));
        assert!(result.chars().count() < 900);
        let long = summary_overview(&"长内容".repeat(1000));
        assert!(long.chars().count() <= 221);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn tables_become_readable_fields_without_losing_rows() {
        let summary = "## 行动项目\n| 事项 | 负责人 | 截止时间 |\n| :--- | :---: | ---: |\n| 接口联调 | 张三 | 明天 |\n| 回归测试 | 李四 | |\n\n## 决议\n按计划发布。";
        let card = meeting_card(None, None, 0, None, "", summary).unwrap();
        let encoded = card.to_string();
        for text in ["• **事项**：接口联调", "**负责人**：张三", "**截止时间**：明天", "• **事项**：回归测试", "**负责人**：李四", "**截止时间**：—", "按计划发布。"] {
            assert!(encoded.contains(text), "missing {text}");
        }
        assert!(!encoded.contains("| :---"));
        assert!(!encoded.contains("| 事项 |"));
    }

    #[test]
    fn table_conversion_preserves_pipes_links_and_code() {
        let converted = tables_as_lists("字段 | 内容\n--- | ---\n条件 | `a|b` 与 a\\|b\n链接 | [文档](https://example.com) <br/> 下一行");
        assert!(converted.contains("`a|b` 与 a\\|b"));
        assert!(converted.contains("[文档](https://example.com)  /  下一行"));
        for unchanged in [
            "```text\n| 列 | 值 |\n| --- | --- |\n| A | B |\n```",
            "~~~~text\n~~~\n| 列 | 值 |\n| --- | --- |\n| A | B |\n~~~~",
            "    A | B\n    --- | ---\n    C | D",
            "A | B\n--- | ---\nC | D | 多余数据",
            "A | B\n--- | ---",
            "普通段落 a | b\n不是表格",
        ] {
            assert_eq!(tables_as_lists(unchanged), unchanged);
        }
    }

    #[test]
    fn converted_tables_still_escape_tags_and_enforce_size() {
        let card = meeting_card(None, None, 0, None, "", "| 项目 | 内容 |\n| --- | --- |\n| 通知 | <at id=all></at> |\n\n| 状态 | 结果 |\n| --- | --- |\n| 测试 | 通过 |").unwrap();
        let text = card.to_string();
        assert!(!text.contains("<at"));
        assert!(text.contains("&lt;at"));
        assert!(text.contains("• **状态**：测试"));
        let huge = format!("事项 | 负责人\n--- | ---\n{}", "测试 | 张三\n".repeat(500));
        assert!(meeting_card(None, None, 0, None, "", &huge).is_err());
    }

    #[test]
    fn card_preserves_identity_sections_and_original_url() {
        let card = meeting_card(Some("研发周会"), Some("501260381"), 1792059534,
            Some("https://vc-jp.larksuite.com/j/501260381"), "项目进展",
            "**摘要**\n已完成讨论。\n\n## 行动项目\n- 张三：明天提交。\n\n**阻碍事项**\n本部分未注明任何内容。").unwrap();
        assert_eq!(card["header"]["title"]["content"], "会议纪要｜研发周会");
        let encoded = card.to_string();
        assert!(encoded.contains("2026-10-15 18:18"));
        assert!(encoded.contains("501260381"));
        assert!(encoded.contains("张三：明天提交。"));
        assert!(!encoded.contains("阻碍事项"));
        assert!(!encoded.contains("## 行动项目"));
        let button = card["elements"].as_array().unwrap().iter().find(|v| v["tag"] == "action").unwrap();
        assert_eq!(button["actions"][0]["url"], "https://vc-jp.larksuite.com/j/501260381");
    }

    #[test]
    fn missing_identity_does_not_invent_a_link_or_participants() {
        let card = meeting_card(None, None, 0, None, "", "**摘要**\n本部分未注明任何内容。").unwrap();
        assert!(card.to_string().contains("没有提取到有效内容"));
        assert!(card["elements"].as_array().unwrap().iter().all(|e| e["tag"] != "action"));
        for url in ["javascript:alert(1)", "https://larksuite.com.evil.test/x", "https://user@vc.larksuite.com/x", "http://vc.larksuite.com/x"] {
            assert!(safe_meeting_url(url).is_none());
        }
    }

    #[test]
    fn sections_preserve_code_and_meaningful_negative_statements() {
        let result = sections("## 讨论\n```text\n## 保留代码中的标题\n本部分未注明任何内容。\n```\n\n**决议**\n无新增决议。");
        assert_eq!(result.len(), 2);
        assert!(result[0].1.contains("## 保留代码中的标题"));
        assert!(result[0].1.contains("本部分未注明任何内容。"));
        assert_eq!(result[1].1, "无新增决议。");
        assert_eq!(sections("**决定本周上线**\n\n**行动项目**\n- 完成测试")[0].1, "**决定本周上线**");
    }

    #[test]
    fn generated_content_cannot_mention_members_and_oversize_is_not_truncated() {
        let card = meeting_card(None, None, 0, None, "", "<at id=all></at> **重点** & 2 < 3").unwrap();
        let encoded = card.to_string();
        assert!(!encoded.contains("<at"));
        assert!(encoded.contains("**重点**"));
        assert!(meeting_card(None, None, 0, None, "", &"中".repeat(7000)).is_err());
    }

    #[test]
    fn delivery_uses_exact_saved_card_and_keeps_legacy_text_compatible() {
        let card = meeting_card(None, None, 0, None, "", "## 决议\n通过").unwrap().to_string();
        let body = delivery_body("oc_expected", "same-uuid", "interactive", Some(&card), "fallback").unwrap();
        assert_eq!(body["msg_type"], "interactive");
        assert_eq!(body["content"], card);
        assert_eq!(body["receive_id"], "oc_expected");
        assert_eq!(body["uuid"], "same-uuid");
        assert_eq!(delivery_body("c", "u", "text", None, "旧纪要").unwrap()["content"], "{\"text\":\"旧纪要\"}");
        assert!(delivery_body("c", "u", "interactive", None, "").is_err());
        assert!(delivery_body("c", "u", "interactive", Some("not json"), "").is_err());
    }

    #[tokio::test]
    async fn migration_preserves_historical_delivery_and_defaults_to_text() {
        use sqlx::Row;
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("CREATE TABLE meetings(id TEXT PRIMARY KEY); INSERT INTO meetings VALUES('old');").execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261015000000_add_lark_app.sql")).execute(&pool).await.unwrap();
        sqlx::raw_sql("INSERT INTO lark_app_deliveries(meeting_id,app_id,remote_id,chat_id,message_uuid,message_text,status,updated_at) VALUES('old','app','remote','original-group','original-uuid','旧纪要','sent',1);").execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261015050000_add_lark_delivery_card.sql")).execute(&pool).await.unwrap();
        let row = sqlx::query("SELECT * FROM lark_app_deliveries WHERE meeting_id='old'").fetch_one(&pool).await.unwrap();
        assert_eq!(row.get::<String,_>("status"), "sent");
        assert_eq!(row.get::<String,_>("chat_id"), "original-group");
        assert_eq!(row.get::<String,_>("message_type"), "text");
        assert!(row.get::<Option<String>,_>("message_content").is_none());
        assert_eq!(row.get::<String,_>("message_text"), "旧纪要");
    }

    // Writes a synthetic preview only when explicitly requested; never sends a message.
    #[test]
    #[ignore]
    fn export_card_preview() {
        let output = std::env::var("MEETILY_CARD_PREVIEW_PATH").expect("set MEETILY_CARD_PREVIEW_PATH");
        let card = meeting_card(Some("研发周会 · 格式预览"), Some("123456789"), 1792059534,
            None, "本周进展与下周安排",
            "**摘要**\n本次会议回顾了迭代进展，确认了发布前的验证安排。\n\n**关键决策**\n- 完成回归测试后发布。\n- 会议纪要统一使用卡片展示。\n\n**行动项目**\n- **张三**：明天下午前完成接口联调。\n- **李四**：周五前补充测试用例。\n\n**讨论要点**\n- 重点检查录音、转写和纪要自动发送流程。\n\n**其他事项**\n本部分未注明任何内容。").unwrap();
        std::fs::write(output, serde_json::to_string_pretty(&card).unwrap()).unwrap();
    }
}
