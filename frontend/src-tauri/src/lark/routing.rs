use serde_json::Value;

#[derive(Debug, PartialEq)]
pub enum MeetingReference {
    Id(String),
    Number(String),
}

// Only explicit routing commands are accepted. Meeting content is never a command.
pub fn parse_binding(text: &str) -> Option<MeetingReference> {
    let cleaned = regex::Regex::new(r"@_user_\d+").ok()?.replace_all(text, "");
    let text = cleaned.trim();
    if let Some(value) = text.strip_prefix("绑定会议ID ") {
        let id = value.trim();
        return (id.len() >= 10 && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_digit()))
            .then(|| MeetingReference::Id(id.into()));
    }
    let value = text.strip_prefix("绑定会议 ")?.trim();
    let number: String = value.chars().filter(|c| *c != ' ' && *c != '-').collect();
    (number.len() == 9 && number.bytes().all(|b| b.is_ascii_digit()))
        .then(|| MeetingReference::Number(number))
}

pub fn summary_text(value: &Value) -> Result<String, String> {
    // Generation uses sanitized markdown. Do not send metadata/reasoning or stringify JSON.
    let text = value
        .get("markdown")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if text.is_empty() {
        return Err("没有可自动发送的 Markdown 纪要。".into());
    }
    let lower = text.to_lowercase();
    if lower.contains("<think") || lower.contains("</think") {
        return Err("纪要含模型推理标记，已暂停自动发送。".into());
    }
    Ok(text.into())
}

pub(super) fn meeting_title<'a>(topic: &'a str, number: &'a str) -> &'a str {
    let topic = topic.trim();
    let number = number.trim();
    if matches!(topic, "" | "群会议" | "Lark 会议") && !number.is_empty() {
        number
    } else if topic.is_empty() {
        "Lark 会议"
    } else {
        topic
    }
}

pub fn active_meeting(value: &Value) -> Result<Option<(String, String, String)>, String> {
    let list = value
        .get("meetings")
        .and_then(Value::as_array)
        .ok_or("当前会议接口返回格式异常，未匹配。")?;
    if list.len() > 1 {
        return Err("检测到多个正在参加的会议，已暂停自动匹配。".into());
    }
    let Some(m) = list.first() else {
        return Ok(None);
    };
    let id = m.get("meeting_id").and_then(Value::as_str).unwrap_or("");
    let no = m.get("meeting_no").and_then(Value::as_str).unwrap_or("");
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err("当前会议缺少有效会议 ID，未匹配。".into());
    }
    Ok(Some((
        id.into(),
        no.into(),
        meeting_title(m.get("meeting_title")
            .and_then(Value::as_str)
            .unwrap_or(""), no)
            .into(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn untitled_group_meetings_use_the_number_and_preserve_real_titles() {
        for topic in ["", "  ", "群会议", " 群会议 ", "Lark 会议"] {
            let meeting = active_meeting(&json!({"meetings":[{
                "meeting_id":"12345678901", "meeting_no":"123456789", "meeting_title":topic
            }]})).unwrap().unwrap();
            assert_eq!(meeting.2, "123456789");
        }
        assert_eq!(meeting_title("研发周会", "123456789"), "研发周会");
        assert_eq!(meeting_title("", ""), "Lark 会议");
        assert_eq!(meeting_title("群会议", ""), "群会议");
    }
    #[test]
    fn only_explicit_commands() {
        assert_eq!(
            parse_binding("@_user_1 绑定会议 123 456 789"),
            Some(MeetingReference::Number("123456789".into()))
        );
        assert_eq!(
            parse_binding("绑定会议ID 1234567890123456789"),
            Some(MeetingReference::Id("1234567890123456789".into()))
        );
        for s in [
            "讨论123456789",
            "绑定会议 123456789和987654321",
            "绑定会议 https://evil.test/123456789",
            "绑定会议ID 123456789",
            "绑定会议 1234567890",
        ] {
            assert_eq!(parse_binding(s), None);
        }
    }
    #[test]
    fn fail_closed_on_multiple_or_malformed_meetings() {
        assert!(active_meeting(&json!({"meetings":[{},{}]})).is_err());
        assert!(active_meeting(&json!({})).is_err());
        assert!(active_meeting(&json!({"meetings":[{}]})).is_err());
        assert_eq!(active_meeting(&json!({"meetings":[]})).unwrap(), None);
        assert_eq!(
            active_meeting(
                &json!({"meetings":[{"meeting_id":"12345678901","meeting_no":"123456789"}]})
            )
            .unwrap()
            .unwrap()
            .0,
            "12345678901"
        );
    }
    #[test]
    fn never_send_reasoning_or_raw_json() {
        assert!(summary_text(&json!({"markdown":" <think>secret</think>"})).is_err());
        assert!(summary_text(&json!({"MeetingName":"only a title"})).is_err());
        assert_eq!(
            summary_text(&json!({"markdown":"## 决议\n通过"})).unwrap(),
            "## 决议\n通过"
        );
    }
}
