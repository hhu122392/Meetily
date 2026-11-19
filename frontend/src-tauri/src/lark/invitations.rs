use chrono::{Datelike, FixedOffset, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(super) struct Invitation {
    pub title: String,
    pub start: i64,
    pub end: i64,
    pub number: Option<String>,
    pub meeting_url: Option<String>,
    pub share_url: Option<String>,
}

pub(super) fn message_text(kind: &str, content: &serde_json::Value) -> Option<String> {
    if kind == "text" {
        return content["text"].as_str().map(str::to_owned);
    }
    if kind != "post" {
        return None;
    }
    let post = if content["content"].is_array() {
        content
    } else {
        content.get("zh_cn").or_else(|| content.get("en_us"))?
    };
    let mut text = String::new();
    for line in post["content"].as_array()? {
        for node in line.as_array()? {
            match node["tag"].as_str()? {
                "text" => text.push_str(node["text"].as_str().unwrap_or("")),
                "a" => {
                    text.push(' ');
                    text.push_str(node["href"].as_str().unwrap_or(""));
                    text.push(' ');
                }
                "at" => text.push(' '),
                _ => {}
            }
        }
        text.push('\n');
    }
    Some(text)
}

fn lark_url(raw: &str) -> Option<url::Url> {
    let u = url::Url::parse(raw).ok()?;
    (u.scheme() == "https"
        && u.username().is_empty()
        && u.password().is_none()
        && u.port().is_none()
        && u.host_str()
            .is_some_and(|h| h == "larksuite.com" || h.ends_with(".larksuite.com")))
    .then_some(u)
}

pub(super) fn meeting_link(raw: &str) -> Option<(String, String)> {
    let u = lark_url(raw)?;
    let host = u.host_str()?;
    if !(host == "vc.larksuite.com" || host.starts_with("vc-") && host.ends_with(".larksuite.com"))
    {
        return None;
    }
    let no = u.path().strip_prefix("/j/")?.trim_end_matches('/');
    (no.len() == 9 && no.bytes().all(|b| b.is_ascii_digit())).then(|| (no.into(), u.to_string()))
}

pub(super) fn share_token(raw: &str) -> Option<String> {
    let u = lark_url(raw)?;
    if u.path() != "/calendar/share" {
        return None;
    }
    let tokens: Vec<_> = u.query_pairs().filter(|(k, _)| k == "token").collect();
    if tokens.len() != 1 {
        return None;
    }
    let token = &tokens[0].1;
    (!token.is_empty()
        && token.len() <= 512
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    .then(|| token.to_string())
}

fn urls(text: &str) -> Vec<&str> {
    regex::Regex::new(r#"https://[^\s<>\[\]）)\"，,]+"#)
        .unwrap()
        .find_iter(text)
        .map(|m| m.as_str())
        .collect()
}

pub(super) fn bare_link(text: &str) -> Option<String> {
    let cleaned = regex::Regex::new(r"@_user_\d+")
        .unwrap()
        .replace_all(text, "");
    meeting_link(cleaned.trim()).map(|(no, _)| no)
}

fn field(text: &str, label: &str, stops: &str) -> Option<String> {
    let re = regex::Regex::new(&format!(r"(?s){}\s*[:：]\s*(.*?)(?:{}|$)", label, stops)).ok()?;
    Some(re.captures(text)?.get(1)?.as_str().trim().to_owned())
}

pub(super) fn parse(text: &str, received: i64) -> Option<Result<Invitation, String>> {
    let is_video =
        text.contains("邀请你加入Lark视频会议") || text.contains("邀请你加入 Lark 视频会议");
    let is_calendar = text.contains("加入Lark日程") || text.contains("加入 Lark 日程");
    if !is_video && !is_calendar {
        return None;
    }
    Some(parse_inner(text, received, is_video))
}

fn parse_inner(text: &str, received: i64, video: bool) -> Result<Invitation, String> {
    if text.len() > 16000 {
        return Err("邀请过长，请直接粘贴 Lark 复制的完整邀请。".into());
    }
    let title = field(
        text,
        if video {
            "会议主题"
        } else {
            "日程主题"
        },
        r"(?:会议)?时间\s*[:：]",
    )
    .filter(|s| !s.is_empty() && s.len() <= 500)
    .ok_or("邀请缺少主题，请复制完整邀请。")?;
    let time = field(
        text,
        if video { "会议时间" } else { "时间" },
        r"会议\s*ID\s*[:：]|会议链接\s*[:：]|组织者\s*[:：]",
    )
    .ok_or("邀请缺少时间，请复制完整邀请。")?;
    let (start, end) = time_range(&time, received)?;
    if end <= received {
        return Err("邀请对应的会议已经结束，请复制下一次会议的邀请。".into());
    }
    if start > received + 366 * 86400 {
        return Err("暂只接受一年内的具体一次会议邀请。".into());
    }
    let links = urls(text);
    if video {
        let numbers: Vec<_> = regex::Regex::new(r"会议\s*ID\s*[:：]\s*([0-9][0-9 \-]*[0-9])")
            .unwrap()
            .captures_iter(text)
            .map(|c| {
                c[1].chars()
                    .filter(char::is_ascii_digit)
                    .collect::<String>()
            })
            .collect();
        let meetings: Vec<_> = links.iter().filter_map(|s| meeting_link(s)).collect();
        if numbers.len() != 1
            || numbers[0].len() != 9
            || meetings.len() != 1
            || numbers[0] != meetings[0].0
        {
            return Err("会议号与入会链接缺失、重复或不一致，请重新复制完整邀请。".into());
        }
        Ok(Invitation {
            title,
            start,
            end,
            number: Some(numbers[0].clone()),
            meeting_url: Some(meetings[0].1.clone()),
            share_url: None,
        })
    } else {
        let shares: Vec<_> = links.iter().filter(|s| share_token(s).is_some()).collect();
        if shares.len() != 1 {
            return Err("日程邀请缺少唯一有效的 Lark 分享链接。".into());
        }
        Ok(Invitation {
            title,
            start,
            end,
            number: None,
            meeting_url: None,
            share_url: Some(shares[0].to_string()),
        })
    }
}

fn time_range(raw: &str, received: i64) -> Result<(i64, i64), String> {
    // Lark's Chinese copy format explicitly includes GMT offset. Never use this PC's timezone.
    let tz = regex::Regex::new(r"GMT\s*([+-])(\d{1,2})(?::(\d{2}))?")
        .unwrap()
        .captures(raw)
        .ok_or("邀请缺少 GMT 时区，请复制完整邀请。")?;
    let hours: i32 = tz[2].parse().map_err(|_| "邀请时区无效")?;
    let minutes: i32 = tz
        .get(3)
        .map_or("0", |m| m.as_str())
        .parse()
        .map_err(|_| "邀请时区无效")?;
    if hours > 14 || minutes > 59 || hours == 14 && minutes != 0 {
        return Err("邀请时区无效。".into());
    }
    let zone =
        FixedOffset::east_opt((hours * 3600 + minutes * 60) * if &tz[1] == "-" { -1 } else { 1 })
            .ok_or("邀请时区无效")?;
    let date_re = regex::Regex::new(r"(?:(\d{4})年)?\s*(\d{1,2})月\s*(\d{1,2})日").unwrap();
    let dates: Vec<_> = date_re.captures_iter(raw).collect();
    if dates.is_empty() || dates.len() > 2 {
        return Err("邀请日期不明确，请分享具体一次日程。".into());
    }
    let current = zone
        .timestamp_opt(received, 0)
        .single()
        .ok_or("接收时间无效")?;
    let year = dates[0]
        .get(1)
        .map(|m| m.as_str().parse::<i32>())
        .transpose()
        .map_err(|_| "邀请年份无效")?
        .unwrap_or(current.year());
    let make_date = |d: &regex::Captures<'_>, fallback: i32| -> Result<NaiveDate, String> {
        let y = d
            .get(1)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(fallback);
        NaiveDate::from_ymd_opt(y, d[2].parse().unwrap_or(0), d[3].parse().unwrap_or(0))
            .ok_or("邀请日期无效。".into())
    };
    let mut first = make_date(&dates[0], year)?;
    // A yearless December invitation for January means next January. No arbitrary past-date rollover.
    if dates[0].get(1).is_none() && current.month() == 12 && first.month() == 1 {
        first = first.with_year(year + 1).ok_or("邀请日期无效")?;
    }
    let clock_re = regex::Regex::new(r"(\d{1,2}):(\d{2})(?::(\d{2}))?").unwrap();
    let without_tz = &raw[..tz.get(0).unwrap().start()];
    let clocks: Vec<_> = clock_re.captures_iter(without_tz).collect();
    if clocks.len() != 2 {
        return Err("邀请必须包含明确的开始和结束时间。".into());
    }
    let at = |date: NaiveDate, c: &regex::Captures<'_>| -> Result<i64, String> {
        let dt = date
            .and_hms_opt(
                c[1].parse().unwrap_or(99),
                c[2].parse().unwrap_or(99),
                c.get(3).and_then(|m| m.as_str().parse().ok()).unwrap_or(0),
            )
            .ok_or("邀请时刻无效")?;
        zone.from_local_datetime(&dt)
            .single()
            .map(|t| t.timestamp())
            .ok_or("邀请时间无效".into())
    };
    let start = at(first, &clocks[0])?;
    let last = if dates.len() == 2 {
        make_date(&dates[1], first.year())?
    } else {
        first
    };
    let mut end = at(last, &clocks[1])?;
    if dates.len() == 1 && end < start {
        end += 86400;
    }
    if end <= start || end - start > 7 * 86400 {
        return Err("邀请时间范围无效，请分享具体一次会议。".into());
    }
    Ok((start, end))
}

impl Invitation {
    pub fn source_key(&self) -> String {
        if let Some(url) = &self.share_url {
            return format!("calendar:{}", share_token(url).unwrap_or_default());
        }
        // Distinguish recurring instances on different days; same-day rescheduling updates the pending reminder.
        format!(
            "video:{}:{}",
            self.number.as_deref().unwrap_or(""),
            (self.start + 8 * 3600) / 86400
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn received() -> i64 {
        chrono::DateTime::parse_from_rfc3339("2026-11-19T10:00:00+08:00")
            .unwrap()
            .timestamp()
    }
    fn video() -> String {
        "@_user_1 𝑴𝒂𝒏𝒃𝒂邀请你加入Lark视频会议 会议主题：(无主题) 会议时间：11月19日 (今天) 11:30 - 12:00 (GMT+8) 会议 ID：807 981 831 会议链接：https://vc-jp.larksuite.com/j/807981831".into()
    }
    #[test]
    fn accepts_actual_chinese_video_invitation() {
        let i = parse(&video(), received()).unwrap().unwrap();
        assert_eq!(i.number.as_deref(), Some("807981831"));
        assert_eq!(i.start, received() + 5400);
        assert_eq!(i.end - i.start, 1800);
    }
    #[test]
    fn rich_text_paste_preserves_the_join_link_and_line_breaks() {
        let original = video();
        let (prefix, link) = original.split_once("https://").unwrap();
        let post = serde_json::json!({"zh_cn":{"content":[[{"tag":"at","user_id":"bot"},{"tag":"text","text":prefix},{"tag":"a","text":"加入会议","href":format!("https://{link}")}]]}});
        let text = message_text("post", &post).unwrap();
        assert_eq!(
            parse(&text, received()).unwrap().unwrap().number.as_deref(),
            Some("807981831")
        );
    }
    #[test]
    fn accepts_calendar_invitation_and_fullwidth_punctuation() {
        let i=parse("@_user_1 开会加入Lark日程: https://www.jp.larksuite.com/calendar/share?token=abc123\n日程主题: IP\n时间: 2026年11月20日（周五） 14:30 - 15:00 (GMT+8)\n组织者: Lili",received()).unwrap().unwrap();
        assert_eq!(i.title, "IP");
        assert_eq!(i.number, None);
        assert_eq!(i.end - i.start, 1800);
    }
    #[test]
    fn rejects_mismatched_expired_or_ambiguous_invites() {
        assert!(
            parse(&video().replace("/j/807981831", "/j/123456789"), received())
                .unwrap()
                .is_err()
        );
        assert!(parse(&video(), received() + 86400).unwrap().is_err());
        assert!(parse(&video().replace("(GMT+8)", ""), received())
            .unwrap()
            .is_err());
        assert!(parse(
            &(video() + " https://vc-jp.larksuite.com/j/123456789"),
            received()
        )
        .unwrap()
        .is_err());
        assert_eq!(parse("讨论 807981831", received()), None);
        assert!(meeting_link("https://vc-jp.larksuite.com.evil.test/j/807981831").is_none());
        assert!(meeting_link("https://vc-jp.larksuite.com@evil.test/j/807981831").is_none());
    }
    #[test]
    fn handles_cross_midnight_and_year_boundary() {
        let (s, e) = time_range("2026年11月19日 23:30 - 00:30 (GMT+8)", received()).unwrap();
        assert_eq!(e - s, 3600);
        let dec = chrono::DateTime::parse_from_rfc3339("2026-12-31T10:00:00+08:00")
            .unwrap()
            .timestamp();
        let (s, _) = time_range("1月1日 11:30 - 12:00 (GMT+8)", dec).unwrap();
        assert_eq!(chrono::DateTime::from_timestamp(s, 0).unwrap().year(), 2027);
    }
}
