use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use url::Url;

// Leave room below the webhook's message size limit; never silently truncate.
const MAX_PAYLOAD_BYTES: usize = 20_000;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct BotConfig {
    pub webhook: String,
    pub secret: String,
    pub target_name: String,
}

impl BotConfig {
    pub fn normalize(mut self) -> Result<Self, String> {
        self.webhook = self.webhook.trim().to_owned();
        self.secret = self.secret.trim().to_owned();
        self.target_name = self.target_name.trim().to_owned();
        validate_webhook(&self.webhook)?;
        if self.target_name.is_empty() || self.target_name.chars().count() > 80 {
            return Err("请填写 1～80 字的目标群备注。".into());
        }
        if self.secret.len() > 512 {
            return Err("签名密钥过长，请检查是否误填了其他内容。".into());
        }
        Ok(self)
    }

    pub fn revision(&self) -> String {
        fingerprint(&[&self.webhook, &self.secret, &self.target_name])
    }
}

pub fn validate_webhook(value: &str) -> Result<Url, String> {
    let invalid = || "请填写国际版 Lark 群机器人的 HTTPS Webhook：https://open.larksuite.com/open-apis/bot/v2/hook/…".to_owned();
    let url = Url::parse(value).map_err(|_| invalid())?;
    let token = url.path().strip_prefix("/open-apis/bot/v2/hook/").unwrap_or("");
    if url.scheme() != "https"
        || url.host_str() != Some("open.larksuite.com")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || token.is_empty()
        || token.len() > 128
        || !token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(invalid());
    }
    Ok(url)
}

pub fn fingerprint(parts: &[&str]) -> String {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    format!("{:x}", hash.finalize())
}

pub fn message_text(title: &str, date: &str, summary: &str) -> Result<String, String> {
    if summary.trim().is_empty() {
        return Err("纪要内容为空，请先生成或编辑纪要。".into());
    }
    if title.chars().count() > 300 || date.len() > 100 {
        return Err("会议标题或时间过长，请检查后重试。".into());
    }
    Ok(format!("会议纪要：{}\n会议时间：{}\n\n{}", title.trim(), date.trim(), summary.trim()))
}

// Identity comes from the matched Lark meeting, never from the generated summary title.
pub fn meeting_message_text(
    topic: Option<&str>, meeting_no: Option<&str>, started_at: i64,
    meeting_url: Option<&str>, summary_title: &str, summary: &str,
) -> Result<String, String> {
    if summary.trim().is_empty() { return Err("纪要内容为空，请先生成或编辑纪要。".into()); }
    let single_line = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let topic = topic.filter(|s| !s.trim().is_empty()).unwrap_or("未记录原会议名称");
    let number = meeting_no.filter(|s| !s.trim().is_empty()).unwrap_or("未记录");
    let time = chrono::DateTime::from_timestamp(started_at, 0)
        .ok_or("录音开始时间无效，无法生成会议标识。")?
        .with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).unwrap())
        .format("%Y-%m-%d %H:%M:%S").to_string();
    let link = meeting_url.filter(|s| !s.trim().is_empty()).unwrap_or("未记录，请通过会议号核对");
    Ok(format!("会议纪要：{}\n会议号：{}\n录音开始时间：{}（北京时间）\n会议链接：{}\n\n摘要标题：{}\n\n{}",
        single_line(topic), single_line(number), time, single_line(link), single_line(summary_title), summary.trim()))
}

pub fn signed_payload(text: &str, secret: &str, timestamp: i64) -> Result<Value, String> {
    let mut body = json!({ "msg_type": "text", "content": { "text": text } });
    if !secret.is_empty() {
        // Lark signs an empty message using "timestamp\nsecret" as the HMAC key.
        let key = format!("{}\n{}", timestamp, secret);
        let mac = Hmac::<Sha256>::new_from_slice(key.as_bytes())
            .map_err(|_| "无法生成 Lark 签名。".to_owned())?;
        body["timestamp"] = json!(timestamp.to_string());
        body["sign"] = json!(STANDARD.encode(mac.finalize().into_bytes()));
    }
    let size = serde_json::to_vec(&body).map_err(|_| "无法编码消息。".to_owned())?.len();
    if size > MAX_PAYLOAD_BYTES {
        return Err("纪要超过单条消息的 20 KB 限制。请在预览中精简内容后发送；没有内容被截断或发送。".into());
    }
    Ok(body)
}

pub fn check_response(value: &Value) -> Result<(), String> {
    let codes: Vec<i64> = [value.get("code"), value.get("StatusCode")]
        .into_iter().flatten().map(|code| code.as_i64().ok_or_else(||
            "Lark 返回了无法确认的结果，请先检查群内是否收到消息，再决定是否重试。".to_owned()
        )).collect::<Result<_, _>>()?;
    if codes.is_empty() {
        return Err("Lark 返回了无法确认的结果，请先检查群内是否收到消息，再决定是否重试。".into());
    }
    if let Some(code) = codes.into_iter().find(|code| *code != 0) {
        let hint = match code {
            19021 => "签名验证失败，请检查签名密钥及电脑时间",
            19024 => "未匹配安全关键词，请在机器人中配置关键词“会议纪要”",
            19022 => "IP 白名单校验失败，请检查机器人安全配置",
            9499 | 99991400 => "请求过于频繁，请稍后重试",
            _ => "请检查机器人是否有效，以及关键词、签名和 IP 白名单配置",
        };
        // Do not echo remote messages: they may contain credentials or submitted text.
        return Err(format!("Lark 拒绝发送（错误码 {}）：{}。", code, hint));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meeting_identity_survives_ai_title_changes_and_uses_beijing_time() {
        let text = meeting_message_text(Some("研发周会"), Some("982467845"), 1792057722,
            Some("https://vc-jp.larksuite.com/j/982467845"), "会议摘要转录分析", "讨论内容").unwrap();
        assert!(text.starts_with("会议纪要：研发周会\n会议号：982467845\n"));
        assert!(text.contains("录音开始时间：2026-10-15 17:48:42（北京时间）"));
        assert!(text.contains("会议链接：https://vc-jp.larksuite.com/j/982467845\n\n摘要标题：会议摘要转录分析"));
    }

    #[test]
    fn missing_identity_is_explicit_and_timezone_rolls_over_the_date() {
        let timestamp = chrono::DateTime::parse_from_rfc3339("2026-10-15T20:00:00Z").unwrap().timestamp();
        let text = meeting_message_text(None, None, timestamp, None, "AI 标题", "正文").unwrap();
        assert!(text.starts_with("会议纪要：未记录原会议名称\n会议号：未记录"));
        assert!(text.contains("2026-10-16 04:00:00（北京时间）"));
        assert!(text.contains("会议链接：未记录，请通过会议号核对"));
    }

    #[test]
    fn restricts_destination_to_lark_webhooks() {
        assert!(validate_webhook("https://open.larksuite.com/open-apis/bot/v2/hook/abc-123").is_ok());
        for value in [
            "http://open.larksuite.com/open-apis/bot/v2/hook/a",
            "https://open.larksuite.com.evil.test/open-apis/bot/v2/hook/a",
            "https://127.0.0.1/open-apis/bot/v2/hook/a",
            "https://user@open.larksuite.com/open-apis/bot/v2/hook/a",
            "https://open.larksuite.com:8443/open-apis/bot/v2/hook/a",
            "https://open.larksuite.com/open-apis/bot/v2/hook/a?redirect=x",
            "https://open.larksuite.com/open-apis/bot/v2/hook/a/extra",
            "https://open.larksuite.com/open-apis/bot/v2/hook/",
        ] {
            assert!(validate_webhook(value).is_err());
        }
    }

    #[test]
    fn checks_business_result_not_just_http_status() {
        assert!(check_response(&json!({"code": 0})).is_ok());
        assert!(check_response(&json!({"StatusCode": 0})).is_ok());
        assert!(check_response(&json!({"code": 19021})).is_err());
        assert!(check_response(&json!({"code": 0, "StatusCode": 1})).is_err());
        assert!(check_response(&json!({"message": "success"})).is_err());
        assert!(check_response(&json!({"code": "19021", "StatusCode": 0})).is_err());
    }

    #[test]
    fn size_check_counts_utf8_and_json_escaping() {
        assert!(signed_payload(&"中".repeat(7000), "", 1).is_err());
        assert!(signed_payload(&"\"".repeat(11000), "", 1).is_err());
        assert!(signed_payload("中文纪要", "", 1).is_ok());
        assert!(message_text("title", "date", " \n ").is_err());
    }

    #[test]
    fn fingerprints_separate_content_and_destinations() {
        assert_ne!(fingerprint(&["ab", "c"]), fingerprint(&["a", "bc"]));
        assert_ne!(fingerprint(&["meeting", "bot-a", "text"]), fingerprint(&["meeting", "bot-b", "text"]));
    }

    #[test]
    fn signs_empty_message_with_timestamp_and_secret_as_key() {
        let body = signed_payload("中文纪要", "example-secret", 1700000000).unwrap();
        assert_eq!(body["timestamp"], "1700000000");
        assert_eq!(body["sign"], "Gs3YRutIJpVN2FvEXKXHU6bEj2pVdSriQ0niig9EbUk=");
        assert_eq!(body["content"]["text"], "中文纪要");
        let unsigned = signed_payload("中文纪要", "", 1700000000).unwrap();
        assert!(unsigned.get("sign").is_none());
        assert!(unsigned.get("timestamp").is_none());
    }
}
