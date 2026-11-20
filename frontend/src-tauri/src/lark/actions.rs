use super::{
    app::{self, db_error, now, AppConfig},
    oauth,
    payload::fingerprint,
};
use crate::{state::AppState, summary::markdown_export};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Member {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Action {
    pub id: String,
    pub summary: String,
    pub evidence: String,
    pub owner_text: String,
    pub due_text: String,
    pub due_date: String,
    pub due_time: String,
    pub member_chat_id: String,
    pub members: Vec<Member>,
    // Historical drafts do not record whether they were edited; preserve them conservatively.
    #[serde(default = "historical_draft_is_manual")]
    pub manually_edited: bool,
}

fn historical_draft_is_manual() -> bool { true }

fn plain(s: &str) -> String {
    s.replace("**", "").replace('`', "").trim().to_owned()
}
fn missing(s: &str) -> String {
    let s = plain(s);
    if [
        "",
        "-",
        "—",
        "无",
        "暂无",
        "未提及",
        "未明确",
        "待定",
        "待确认",
        "未指定",
        "N/A",
    ]
    .contains(&s.as_str())
    {
        String::new()
    } else {
        s
    }
}
fn key(summary: &str) -> String {
    fingerprint(&[&plain(summary)
        .split_whitespace()
        .collect::<String>()
        .to_lowercase()])
}
fn heading(s: &str) -> Option<(usize, String)> {
    let s = s.trim();
    let level = s.chars().take_while(|c| *c == '#').count();
    if level > 0 && level <= 6 && s.as_bytes().get(level) == Some(&b' ') {
        Some((
            level,
            plain(s[level..].trim())
                .trim_end_matches('#')
                .trim()
                .to_owned(),
        ))
    } else if s.starts_with("**") && s.ends_with("**") && s.len() > 4 {
        Some((2, plain(s).trim_end_matches(['：', ':']).to_owned()))
    } else {
        None
    }
}
fn action_heading(s: &str) -> bool {
    let s = s.to_lowercase();
    [
        "行动项",
        "行动项目",
        "待办事项",
        "后续行动",
        "下一步",
        "action items",
        "next steps",
        "follow-up actions",
    ]
    .iter()
    .any(|v| s == *v || s.ends_with(&format!(" {v}")))
}
fn cells(s: &str) -> Vec<String> {
    // Keep escaped pipes inside a cell; do not split them into false tasks.
    let mut result = Vec::new();
    let mut cell = String::new();
    let mut escaped = false;
    for ch in s.trim().trim_matches('|').chars() {
        if escaped {
            cell.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '|' {
            result.push(plain(&cell));
            cell.clear();
        } else {
            cell.push(ch);
        }
    }
    result.push(plain(&cell));
    result
}
fn candidate(summary: String, evidence: String, owner: String, due: String) -> Option<Action> {
    let summary = missing(&summary);
    if summary.is_empty() || summary.contains("本部分未注明") || summary == "无行动项" {
        return None;
    }
    let due = missing(&due);
    let (date, time) = if chrono::NaiveDate::parse_from_str(&due, "%Y-%m-%d").is_ok() {
        (due.clone(), String::new())
    } else if let Ok(d) =
        chrono::NaiveDateTime::parse_from_str(&due.replace(' ', "T"), "%Y-%m-%dT%H:%M")
    {
        (
            d.format("%Y-%m-%d").to_string(),
            d.format("%H:%M").to_string(),
        )
    } else {
        (String::new(), String::new())
    };
    Some(Action {
        id: String::new(),
        summary,
        evidence,
        owner_text: missing(&owner),
        due_text: due,
        due_date: date,
        due_time: time,
        member_chat_id: String::new(),
        members: vec![],
        manually_edited: false,
    })
}

fn merge_candidate(result: &mut Vec<Action>, item: Action) {
    if let Some(existing) = result.iter_mut().find(|v| key(&v.summary) == key(&item.summary)) {
        for (previous, next, separator) in [
            (&mut existing.owner_text, &item.owner_text, "、"),
            (&mut existing.due_text, &item.due_text, "；"),
        ] {
            if !next.is_empty() && next.as_str() != previous.as_str() {
                if !previous.is_empty() { previous.push_str(separator); }
                previous.push_str(next);
            }
        }
        if existing.due_text != item.due_text {
            existing.due_date.clear();
            existing.due_time.clear();
        }
        if !existing.evidence.lines().any(|line| line == item.evidence) {
            existing.evidence.push('\n');
            existing.evidence.push_str(&item.evidence);
        }
    } else {
        result.push(item);
    }
}

// Extract only explicit action sections. This is local parsing, without model/API calls.
fn extract(markdown: &str) -> Vec<Action> {
    let mut result = Vec::new();
    let mut section = None;
    let mut table: Option<(usize, Option<usize>, Option<usize>)> = None;
    let mut fence: Option<char> = None;
    let mut seen = HashSet::new();
    for line in markdown.lines() {
        let s = line.trim();
        if s.starts_with("```") || s.starts_with("~~~") {
            let marker = s.chars().next().unwrap();
            if fence == Some(marker) {
                fence = None;
            } else if fence.is_none() {
                fence = Some(marker);
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        if let Some((level, title)) = heading(s) {
            if action_heading(&title) {
                section = Some(level);
                table = None;
                continue;
            }
            if section.is_some_and(|parent| level <= parent) {
                section = None;
                table = None;
            }
            continue;
        }
        if section.is_none() {
            continue;
        }
        let item = if s.starts_with('|') {
            let row = cells(s);
            if row
                .iter()
                .all(|c| !c.is_empty() && c.chars().all(|x| x == '-' || x == ':' || x == ' '))
            {
                continue;
            }
            if let Some(task) = row.iter().position(|c| {
                [
                    "任务",
                    "事项",
                    "行动",
                    "行动项",
                    "待办事项",
                    "Task",
                    "Action",
                    "任务内容",
                ]
                .contains(&c.as_str())
            }) {
                table = Some((
                    task,
                    row.iter().position(|c| {
                        ["负责人", "责任人", "Owner", "Assignee"].contains(&c.as_str())
                    }),
                    row.iter().position(|c| {
                        [
                            "截止时间",
                            "截止日期",
                            "截止",
                            "Due",
                            "Due Date",
                            "Deadline",
                        ]
                        .contains(&c.as_str())
                    }),
                ));
                continue;
            }
            table.and_then(|(task, owner, due)| {
                candidate(
                    row.get(task)?.clone(),
                    s.into(),
                    owner.and_then(|i| row.get(i)).cloned().unwrap_or_default(),
                    due.and_then(|i| row.get(i)).cloned().unwrap_or_default(),
                )
            })
        } else {
            table = None;
            let body = s
                .strip_prefix("- ")
                .or_else(|| s.strip_prefix("* "))
                .or_else(|| s.strip_prefix("+ "))
                .or_else(|| {
                    s.split_once(". ")
                        .filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
                        .map(|(_, b)| b)
                });
            body.and_then(|b| {
                // Completed checkboxes are not pending work.
                if b.starts_with("[x]") || b.starts_with("[X]") {
                    return None;
                }
                let b = plain(b.strip_prefix("[ ] ").unwrap_or(b));
                let mut owner = String::new();
                let mut due = String::new();
                let mut task = Vec::new();
                for part in b.split(['；', ';']) {
                    let part = part.trim();
                    if let Some(v) = part
                        .strip_prefix("负责人：")
                        .or_else(|| part.strip_prefix("负责人:"))
                    {
                        owner = v.trim().into();
                    } else if let Some(v) = part
                        .strip_prefix("截止时间：")
                        .or_else(|| part.strip_prefix("截止日期："))
                        .or_else(|| part.strip_prefix("截止："))
                    {
                        due = v.trim().into();
                    } else {
                        task.push(part);
                    }
                }
                candidate(task.join("；"), s.into(), owner, due)
            })
        };
        if let Some(item) = item {
            if seen.insert(key(&item.summary)) {
                result.push(item);
            } else if let Some(existing) = result
                .iter_mut()
                .find(|v| key(&v.summary) == key(&item.summary))
            {
                // Repeated rows can carry additional owners or conflicting dates. Keep their evidence.
                if !item.owner_text.is_empty() && item.owner_text != existing.owner_text {
                    existing.owner_text = [existing.owner_text.as_str(), item.owner_text.as_str()]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join("、");
                }
                if !item.due_text.is_empty() && item.due_text != existing.due_text {
                    existing.due_text = [existing.due_text.as_str(), item.due_text.as_str()]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join("；");
                    existing.due_date.clear();
                    existing.due_time.clear();
                }
                if !existing.evidence.lines().any(|line| line == item.evidence) {
                    existing.evidence.push('\n');
                    existing.evidence.push_str(&item.evidence);
                }
            }
        }
    }
    result
}

fn config_revision(c: &AppConfig) -> String {
    fingerprint(&[
        &c.app_id,
        &c.owner_id,
        &c.secret,
        &c.enabled.to_string(),
        &c.tasks_authorized.to_string(),
    ])
}
async fn source(pool: &SqlitePool, meeting: &str) -> Result<(String, String, Vec<Action>), String> {
    let title: String = sqlx::query_scalar("SELECT title FROM meetings WHERE id=?")
        .bind(meeting)
        .fetch_optional(pool)
        .await
        .map_err(db_error)?
        .ok_or("当前公司不存在该会议。")?;
    let file = markdown_export::preview(pool, meeting)
        .await?
        .ok_or("请先保存会议纪要。")?;
    let raw: Option<Option<String>> = sqlx::query_scalar("SELECT result FROM summary_processes WHERE meeting_id=?")
        .bind(meeting).fetch_optional(pool).await.map_err(db_error)?;
    let result = raw.flatten().map(|s| serde_json::from_str::<Value>(&s))
        .transpose().map_err(|_| "会议分析数据损坏。")?;
    let independent = result.as_ref().and_then(|r| r.get(crate::summary::action_extraction::RESULT_FIELD));
    let markdown = file["content"].as_str().ok_or("纪要内容无效")?;
    let hash = file["hash"].as_str().ok_or("纪要版本无效")?;
    if let Some(value) = independent {
        let items: Vec<crate::summary::action_extraction::ExtractedAction> = serde_json::from_value(value.clone())
            .map_err(|_| "独立行动项数据损坏。")?;
        let mut actions = Vec::new();
        for item in items.into_iter().filter_map(|i| candidate(i.summary, i.evidence, i.owner_text, i.due_text)) {
            merge_candidate(&mut actions, item);
        }
        return Ok((title, fingerprint(&[hash, &value.to_string()]), actions));
    }
    // Older meetings have no independent extraction yet; retain their existing workflow.
    Ok((title, hash.into(), extract(markdown)))
}
async fn drafts(pool: &SqlitePool, meeting: &str, hash: &str) -> Result<Vec<Action>, String> {
    let rows: Vec<String> = sqlx::query_scalar("SELECT draft_json FROM lark_action_drafts WHERE meeting_id=? AND source_hash=? ORDER BY rowid")
        .bind(meeting).bind(hash).fetch_all(pool).await.map_err(db_error)?;
    rows.iter()
        .map(|s| serde_json::from_str(s).map_err(|_| "行动项草稿损坏。".into()))
        .collect()
}
fn revision(items: &[Action]) -> Result<String, String> {
    Ok(fingerprint(&[
        &serde_json::to_string(items).map_err(|_| "行动项格式错误")?
    ]))
}
async fn receipts(pool: &SqlitePool, meeting: &str, c: &AppConfig) -> Result<Vec<Value>, String> {
    let rows = sqlx::query("SELECT r.*,d.draft_json FROM lark_action_receipts r JOIN lark_action_drafts d ON d.id=r.action_id WHERE d.meeting_id=? AND r.app_id=? AND r.owner_id=? ORDER BY r.updated_at DESC")
        .bind(meeting).bind(&c.app_id).bind(&c.owner_id).fetch_all(pool).await.map_err(db_error)?;
    Ok(rows.iter().map(|r| json!({"id":r.get::<String,_>("action_id"),"status":r.get::<String,_>("status"),"detail":r.get::<String,_>("detail"),"taskGuid":r.get::<Option<String>,_>("task_guid"),"taskUrl":r.get::<Option<String>,_>("task_url"),"summary":serde_json::from_str::<Value>(&r.get::<String,_>("request_json")).ok().and_then(|v|v["summary"].as_str().map(str::to_owned))})).collect())
}
async fn context(
    pool: &SqlitePool,
    meeting: &str,
    hash: &str,
    c: &AppConfig,
) -> Result<Value, String> {
    let items = drafts(pool, meeting, hash).await?;
    Ok(
        json!({"sourceHash":hash,"revision":revision(&items)?,"items":items,"receipts":receipts(pool,meeting,c).await?,"authorized":c.enabled && c.tasks_authorized && !c.owner_id.is_empty() && oauth::authorization_status(c).1,"ownerId":c.owner_id}),
    )
}

#[tauri::command]
pub async fn lark_actions_extract(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<Value, String> {
    let _storage = crate::audio::storage_migration::use_storage().await?;
    let _guard = app::DELIVERY_LOCK.lock().await;
    let pool = state.db_manager.pool();
    let hash = refresh_drafts(pool, &meeting_id).await?;
    context(pool, &meeting_id, &hash, &app::load(pool).await?).await
}

// Caller holds DELIVERY_LOCK. This only creates local review drafts, never Lark tasks.
pub(crate) async fn refresh_drafts(pool: &SqlitePool, meeting_id: &str) -> Result<String, String> {
    let (_, hash, items) = source(pool, meeting_id).await?;
    if items.len() > 100 {
        return Err("纪要包含超过 100 条行动项，请先拆分整理。".into());
    }
    let mut tx = pool.begin().await.map_err(db_error)?;
    for mut item in items {
        let source_key = key(&item.summary);
        let old = sqlx::query(
            "SELECT id,source_hash,draft_json FROM lark_action_drafts WHERE meeting_id=? AND source_key=?",
        )
        .bind(&meeting_id)
        .bind(&source_key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        if let Some(old) = old {
            if old.get::<String, _>("source_hash") == hash {
                continue;
            }
            item.id = old.get("id");
            let previous: Action = serde_json::from_str(old.get::<&str, _>("draft_json"))
                .map_err(|_| "行动项草稿损坏。")?;
            if previous.manually_edited {
                // Regeneration may update automatic candidates, never reviewed user edits.
                sqlx::query("UPDATE lark_action_drafts SET source_hash=?,updated_at=? WHERE id=?")
                    .bind(&hash).bind(now()).bind(&item.id).execute(&mut *tx).await.map_err(db_error)?;
                continue;
            }
        } else {
            item.id = uuid::Uuid::new_v4().to_string();
        }
        sqlx::query("INSERT INTO lark_action_drafts(id,meeting_id,source_key,source_hash,draft_json,updated_at) VALUES(?,?,?,?,?,?) ON CONFLICT(meeting_id,source_key) DO UPDATE SET source_hash=excluded.source_hash,draft_json=excluded.draft_json,updated_at=excluded.updated_at")
            .bind(&item.id).bind(&meeting_id).bind(source_key).bind(&hash).bind(serde_json::to_string(&item).unwrap()).bind(now()).execute(&mut *tx).await.map_err(db_error)?;
    }
    sqlx::query("UPDATE lark_action_drafts SET source_hash=?,updated_at=? WHERE meeting_id=? AND (source_key LIKE 'new-%' OR COALESCE(json_extract(draft_json,'$.manuallyEdited'),1)=1)")
        .bind(&hash).bind(now()).bind(meeting_id).execute(&mut *tx).await.map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    Ok(hash)
}

fn valid_id(s: &str, prefix: &str) -> bool {
    s.starts_with(prefix)
        && s.len() > prefix.len()
        && s.len() < 100
        && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}
async fn members(c: &AppConfig, chat: &str) -> Result<Vec<Member>, String> {
    if !valid_id(chat, "oc_") {
        return Err("请选择用于匹配负责人的群。".into());
    }
    let token = app::tenant_token(c).await?;
    let mut result = vec![];
    let mut cursor = String::new();
    for _ in 0..50 {
        let path = {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            query
                .append_pair("member_id_type", "open_id")
                .append_pair("page_size", "100");
            if !cursor.is_empty() {
                query.append_pair("page_token", &cursor);
            }
            format!("/open-apis/im/v1/chats/{chat}/members?{}", query.finish())
        };
        let data = app::api(reqwest::Method::GET, &path, &token, None).await?;
        for item in data["items"]
            .as_array()
            .ok_or("成员列表缺失，未进行人员匹配。")?
        {
            if let (Some(id), Some(name)) = (
                item["member_id"].as_str().filter(|s| valid_id(s, "ou_")),
                item["name"].as_str(),
            ) {
                if !result.iter().any(|m: &Member| m.id == id) {
                    result.push(Member {
                        id: id.into(),
                        name: name.into(),
                    });
                }
            }
        }
        if data["has_more"] == false {
            return Ok(result);
        }
        let next = data["page_token"]
            .as_str()
            .filter(|s| !s.is_empty() && *s != cursor)
            .ok_or("成员分页响应异常。")?;
        cursor = next.into();
    }
    Err("群成员过多，请选择范围较小的会议群。".into())
}
#[tauri::command]
pub async fn lark_actions_members(
    state: tauri::State<'_, AppState>,
    chat_id: String,
) -> Result<Vec<Member>, String> {
    let _lock = app::CONFIG_LOCK.lock().await;
    let c = app::load(state.db_manager.pool()).await?;
    if !c.enabled {
        return Err("请启用会议助手。".into());
    }
    members(&c, &chat_id).await
}

fn validate_draft(item: &Action) -> Result<(), String> {
    if item.summary.trim().is_empty()
        || item.summary.chars().count() > 1000
        || item.evidence.chars().count() > 1500
        || item.owner_text.chars().count() > 200
        || item.due_text.chars().count() > 100
        || item.members.len() > 20
    {
        return Err("行动项内容为空或过长，请精简后保存。".into());
    }
    if item
        .members
        .iter()
        .any(|m| !valid_id(&m.id, "ou_") || m.name.chars().count() > 200)
    {
        return Err("负责人标识无效，请从群成员重新选择。".into());
    }
    if !item.due_date.is_empty() {
        chrono::NaiveDate::parse_from_str(&item.due_date, "%Y-%m-%d")
            .map_err(|_| "截止日期格式无效。")?;
    }
    if !item.due_time.is_empty() {
        if item.due_date.is_empty() {
            return Err("填写截止时间时也需要填写日期。".into());
        }
        chrono::NaiveTime::parse_from_str(&item.due_time, "%H:%M")
            .map_err(|_| "截止时间格式无效。")?;
    }
    Ok(())
}
async fn save_drafts(
    pool: &SqlitePool,
    meeting: &str,
    hash: &str,
    expected: &str,
    items: &[Action],
) -> Result<(), String> {
    if items.len() > 100 {
        return Err("最多保存 100 条行动项。".into());
    }
    let old = drafts(pool, meeting, hash).await?;
    if revision(&old)? != expected {
        return Err("行动项已被其他操作修改，请重新提取后核对。".into());
    }
    let by_id: HashMap<_, _> = old.iter().map(|i| (i.id.as_str(), i)).collect();
    let mut seen = HashSet::new();
    for item in items {
        validate_draft(item)?;
        if !seen.insert(&item.id) {
            return Err("行动项重复。".into());
        }
        if let Some(previous) = by_id.get(item.id.as_str()) {
            if item.evidence != previous.evidence {
                return Err("原文依据不可修改。".into());
            }
        } else if !item.id.starts_with("new-") || !item.evidence.is_empty() {
            return Err("行动项不属于当前纪要。".into());
        }
    }
    let mut tx = pool.begin().await.map_err(db_error)?;
    for item in items {
        let mut saved_item = item.clone();
        if let Some(previous) = by_id.get(item.id.as_str()) {
            saved_item.manually_edited = previous.manually_edited;
            saved_item.manually_edited |= &saved_item != *previous;
        } else {
            saved_item.manually_edited = true;
        }
        if by_id.contains_key(item.id.as_str()) {
            sqlx::query("UPDATE lark_action_drafts SET draft_json=?,updated_at=? WHERE id=? AND meeting_id=? AND source_hash=?")
                .bind(serde_json::to_string(&saved_item).unwrap()).bind(now()).bind(&item.id).bind(meeting).bind(hash).execute(&mut *tx).await.map_err(db_error)?;
        } else {
            sqlx::query("INSERT INTO lark_action_drafts(id,meeting_id,source_key,source_hash,draft_json,updated_at) VALUES(?,?,?,?,?,?)")
                .bind(&item.id).bind(meeting).bind(&item.id).bind(hash).bind(serde_json::to_string(&saved_item).unwrap()).bind(now()).execute(&mut *tx).await.map_err(db_error)?;
        }
    }
    tx.commit().await.map_err(db_error)?;
    Ok(())
}
#[tauri::command]
pub async fn lark_actions_save(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    source_hash: String,
    expected_revision: String,
    items: Vec<Action>,
) -> Result<Value, String> {
    let _storage = crate::audio::storage_migration::use_storage().await?;
    let _guard = app::DELIVERY_LOCK.lock().await;
    let pool = state.db_manager.pool();
    if source(pool, &meeting_id).await?.1 != source_hash {
        return Err("纪要已变化，请重新提取行动项。".into());
    }
    save_drafts(pool, &meeting_id, &source_hash, &expected_revision, &items).await?;
    context(pool, &meeting_id, &source_hash, &app::load(pool).await?).await
}

fn task_body(item: &Action, meeting_title: &str) -> Result<Value, String> {
    validate_draft(item)?;
    if !item.owner_text.is_empty() && item.members.is_empty() {
        return Err(format!(
            "“{}”提到了负责人，请匹配 Lark 用户；确需不指定时请清空负责人文字。",
            item.summary
        ));
    }
    if !item.due_text.is_empty() && item.due_date.is_empty() {
        return Err(format!(
            "“{}”提到了截止时间，请核对具体日期；确需不设置时请清空截止原文。",
            item.summary
        ));
    }
    let mut body = json!({"summary":item.summary.trim(),"description":format!("会议：{}\n纪要依据：{}\n负责人原文：{}\n截止原文：{}\n已在 Meetily 人工确认",meeting_title.chars().take(300).collect::<String>(),item.evidence,item.owner_text,item.due_text)});
    if !item.members.is_empty() {
        body["members"] = json!(item
            .members
            .iter()
            .map(|m| json!({"id":m.id,"type":"user","role":"assignee"}))
            .collect::<Vec<_>>());
    }
    if !item.due_date.is_empty() {
        let date = chrono::NaiveDate::parse_from_str(&item.due_date, "%Y-%m-%d").unwrap();
        let time = if item.due_time.is_empty() {
            chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap()
        } else {
            chrono::NaiveTime::parse_from_str(&item.due_time, "%H:%M").unwrap()
        };
        // The Task API interprets all-day dates in UTC+0; timed deadlines use Beijing time.
        let timestamp = date.and_time(time).and_utc().timestamp_millis()
            - if item.due_time.is_empty() {
                0
            } else {
                8 * 3600 * 1000
            };
        body["due"] =
            json!({"timestamp":timestamp.to_string(),"is_all_day":item.due_time.is_empty()});
    }
    Ok(body)
}

#[tauri::command]
pub async fn lark_actions_preview(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    source_hash: String,
    expected_revision: String,
    selected_ids: Vec<String>,
) -> Result<Value, String> {
    let _storage = crate::audio::storage_migration::use_storage().await?;
    let _config = app::CONFIG_LOCK.lock().await;
    let _guard = app::DELIVERY_LOCK.lock().await;
    let pool = state.db_manager.pool();
    let c = oauth::authorized_config_locked(pool).await?;
    if !c.tasks_authorized {
        return Err("请先连接 Lark 任务，授权 task:task:write。".into());
    }
    let (title, hash, _) = source(pool, &meeting_id).await?;
    let items = drafts(pool, &meeting_id, &hash).await?;
    if hash != source_hash || revision(&items)? != expected_revision {
        return Err("纪要或行动项已变化，请重新核对。".into());
    }
    if selected_ids.is_empty()
        || selected_ids.len() > 50
        || selected_ids.iter().collect::<HashSet<_>>().len() != selected_ids.len()
    {
        return Err("请勾选 1 至 50 条不重复的行动项。".into());
    }
    let previous = receipts(pool, &meeting_id, &c).await?;
    let mut payload = vec![];
    let mut groups = HashMap::new();
    for id in selected_ids {
        let item = items.iter().find(|i| i.id == id).ok_or("行动项已失效。")?;
        if previous
            .iter()
            .any(|r| r["id"] == id && r["status"] != "failed")
        {
            return Err("所选行动项已同步或结果待核对，不会重复创建。".into());
        }
        if !item.members.is_empty() {
            if !groups.contains_key(&item.member_chat_id) {
                groups.insert(
                    item.member_chat_id.clone(),
                    members(&c, &item.member_chat_id).await?,
                );
            }
            if item
                .members
                .iter()
                .any(|m| !groups[&item.member_chat_id].contains(m))
            {
                return Err("负责人已不在所选群或姓名已变化，请重新选择并保存。".into());
            }
        }
        let mut body = task_body(item, &title)?;
        body["client_token"] = json!(uuid::Uuid::new_v4().to_string());
        payload.push(json!({"id":id,"action":item,"body":body}));
    }
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO lark_action_previews(id,meeting_id,source_hash,config_revision,draft_revision,payload_json,created_at) VALUES(?,?,?,?,?,?,?)")
        .bind(&id).bind(&meeting_id).bind(&hash).bind(config_revision(&c)).bind(&expected_revision).bind(serde_json::to_string(&payload).unwrap()).bind(now()).execute(pool).await.map_err(db_error)?;
    Ok(
        json!({"id":id,"items":payload.iter().map(|v|v["action"].clone()).collect::<Vec<_>>(),"ownerId":c.owner_id}),
    )
}

enum Outcome {
    Sent(String, Option<String>),
    Failed(String),
    Unknown(String),
}
fn outcome(status: u16, value: Option<&Value>) -> Outcome {
    if let Some(v) = value {
        if (200..300).contains(&status) && v["code"] == 0 {
            if let Some(guid) = v["data"]["task"]["guid"].as_str().filter(|s| !s.is_empty()) {
                let url = v["data"]["task"]["url"]
                    .as_str()
                    .and_then(|s| url::Url::parse(s).ok())
                    .filter(|u| {
                        u.scheme() == "https"
                            && u.host_str().is_some_and(|h| {
                                h == "larksuite.com" || h.ends_with(".larksuite.com")
                            })
                            && u.username().is_empty()
                            && u.password().is_none()
                    })
                    .map(|u| u.to_string());
                return Outcome::Sent(guid.into(), url);
            }
        }
        if status < 500
            && v["code"].as_i64().is_some_and(|c| {
                matches!(
                    c,
                    1470400
                        | 1470403
                        | 1470404
                        | 99991400
                        | 99991401
                        | 99991403
                        | 99991663
                        | 99991668
                        | 99991672
                        | 99991679
                )
            })
        {
            return Outcome::Failed(format!(
                "Lark 拒绝创建，错误码 {}。请检查任务权限、应用发布状态或负责人可见范围。",
                v["code"]
            ));
        }
    }
    Outcome::Unknown(
        "未收到明确结果。请在 Lark 任务中核对；为避免重复，本项已停止再次创建。".into(),
    )
}
async fn create_task(token: &str, body: Value) -> Outcome {
    let client = match app::client() {
        Ok(c) => c,
        Err(e) => return Outcome::Failed(e),
    };
    match client
        .post(format!(
            "{}/open-apis/task/v2/tasks?user_id_type=open_id",
            app::BASE
        ))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status().as_u16();
            outcome(status, response.json::<Value>().await.ok().as_ref())
        }
        Err(_) => Outcome::Unknown(
            "请求中断，任务可能已经创建。请先在 Lark 中核对；不会自动重试。".into(),
        ),
    }
}
async fn send_items<F, Fut>(
    pool: &SqlitePool,
    c: &AppConfig,
    payload: &[Value],
    mut send: F,
) -> Result<(), String>
where
    F: FnMut(Value) -> Fut,
    Fut: std::future::Future<Output = Outcome>,
{
    for item in payload {
        let id = item["id"].as_str().ok_or("预览内容异常。")?;
        // A durable reservation precedes every network write. Interrupted writes stay blocked after restart.
        let claimed=sqlx::query("INSERT INTO lark_action_receipts(action_id,app_id,owner_id,status,request_json,client_token,updated_at) VALUES(?,?,?,'sending',?,?,?) ON CONFLICT(action_id,app_id,owner_id) DO UPDATE SET status='sending',request_json=excluded.request_json,client_token=excluded.client_token,detail='',updated_at=excluded.updated_at WHERE lark_action_receipts.status='failed'")
            .bind(id).bind(&c.app_id).bind(&c.owner_id).bind(item["body"].to_string()).bind(item["body"]["client_token"].as_str().ok_or("幂等标识缺失")?).bind(now()).execute(pool).await.map_err(db_error)?.rows_affected();
        if claimed == 0 {
            continue;
        }
        let (status, detail, guid, url) = match send(item["body"].clone()).await {
            Outcome::Sent(g, u) => ("sent", "已同步到 Lark 任务。".into(), Some(g), u),
            Outcome::Failed(e) => ("failed", e, None, None),
            Outcome::Unknown(e) => ("unknown", e, None, None),
        };
        sqlx::query("UPDATE lark_action_receipts SET status=?,detail=?,task_guid=?,task_url=?,updated_at=? WHERE action_id=? AND app_id=? AND owner_id=?")
            .bind(status).bind(detail).bind(guid).bind(url).bind(now()).bind(id).bind(&c.app_id).bind(&c.owner_id).execute(pool).await.map_err(db_error)?;
        if status != "sent" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    }
    Ok(())
}

#[tauri::command]
pub async fn lark_actions_sync(
    state: tauri::State<'_, AppState>,
    preview_id: String,
    confirmed: bool,
) -> Result<Value, String> {
    if !confirmed {
        return Err("请先人工核对并确认同步。".into());
    }
    let _storage = crate::audio::storage_migration::use_storage().await?;
    let _config = app::CONFIG_LOCK.lock().await;
    let _guard = app::DELIVERY_LOCK.lock().await;
    let pool = state.db_manager.pool();
    let c = oauth::authorized_config_locked(pool).await?;
    let (meeting, hash, payload) = checked_preview(pool, &c, &preview_id).await?;
    send_items(pool, &c, &payload, |body| create_task(&c.user_token, body)).await?;
    context(pool, &meeting, &hash, &c).await
}

async fn checked_preview(
    pool: &SqlitePool,
    c: &AppConfig,
    preview_id: &str,
) -> Result<(String, String, Vec<Value>), String> {
    let preview = sqlx::query("SELECT * FROM lark_action_previews WHERE id=?")
        .bind(preview_id)
        .fetch_optional(pool)
        .await
        .map_err(db_error)?
        .ok_or("预览已失效，请重新预览。")?;
    if config_revision(&c) != preview.get::<String, _>("config_revision")
        || now() - preview.get::<i64, _>("created_at") > 900
    {
        return Err("账号、权限或预览时效已变化，请重新预览。".into());
    }
    let meeting: String = preview.get("meeting_id");
    let (_, hash, _) = source(pool, &meeting).await?;
    if hash != preview.get::<String, _>("source_hash")
        || revision(&drafts(pool, &meeting, &hash).await?)?
            != preview.get::<String, _>("draft_revision")
    {
        return Err("纪要或行动项已修改，本次没有同步，请重新预览。".into());
    }
    let payload: Vec<Value> = serde_json::from_str(&preview.get::<String, _>("payload_json"))
        .map_err(|_| "预览记录损坏。")?;
    Ok((meeting, hash, payload))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    fn item(id: &str) -> Action {
        let mut a = candidate(
            "补充测试".into(),
            "- 补充测试".into(),
            String::new(),
            String::new(),
        )
        .unwrap();
        a.id = id.into();
        a
    }
    async fn db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("PRAGMA foreign_keys=ON; CREATE TABLE meetings(id TEXT PRIMARY KEY); INSERT INTO meetings VALUES('m'),('other');").execute(&pool).await.unwrap();
        sqlx::raw_sql("CREATE TABLE summary_processes(meeting_id TEXT PRIMARY KEY,result TEXT);").execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!(
            "../../migrations/20261116030000_lark_action_tasks.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        for id in ["a", "b", "c"] {
            let a = item(id);
            sqlx::query("INSERT INTO lark_action_drafts VALUES(?,'m',?,'hash',?,0)")
                .bind(id)
                .bind(id)
                .bind(serde_json::to_string(&a).unwrap())
                .execute(&pool)
                .await
                .unwrap();
        }
        pool
    }
    #[test]
    fn extraction_is_scoped_deduplicated_and_never_invents_people_or_dates() {
        let md="# 纪要\n## 讨论\n- 不应作为待办\n## 行动项\n| **负责人** | **任务** | **截止时间** |\n| --- | --- | --- |\n| 张三 | 完成接口 | 下周五 |\n| 待定 | 补充测试 | 未提及 |\n| 待定 | 补充测试 | 未提及 |\n## 风险\n- 不应提取\n**待办事项**\n- [ ] 补充文档；负责人：李四；截止日期：2026-11-20\n- [x] 已完成的事\n```\n- 代码不是待办\n```\n- 调查问题\n";
        let items = extract(md);
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].owner_text, "张三");
        assert_eq!(items[0].due_text, "下周五");
        assert!(items[0].due_date.is_empty() && items[0].members.is_empty());
        assert!(
            items[1].owner_text.is_empty()
                && items[1].due_text.is_empty()
                && items[1].due_date.is_empty()
        );
        assert_eq!(items[2].due_date, "2026-11-20");
        assert_eq!(items[2].owner_text, "李四");
        assert_eq!(items[3].summary, "调查问题");
        assert!(extract("## 摘要\n今天需要测试\n## 风险\n- 负责人：张三").is_empty());
    }
    #[tokio::test]
    async fn independent_actions_survive_missing_template_section_and_manual_minutes_edits() {
        let pool = db().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("纪要.md");
        std::fs::write(&path, "## 摘要\n会议讨论了接口交付。").unwrap();
        sqlx::raw_sql("ALTER TABLE meetings ADD COLUMN title TEXT NOT NULL DEFAULT '例会'; CREATE TABLE summary_markdown_exports(meeting_id TEXT PRIMARY KEY,path TEXT,error TEXT);").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO summary_markdown_exports VALUES('m',?,NULL)")
            .bind(path.to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        let result = json!({"markdown":"只有摘要", "meeting_action_items":[
            {"summary":"补充接口测试","evidence":"确定补充接口测试。","ownerText":"","dueText":""}
        ]});
        sqlx::query("INSERT INTO summary_processes VALUES('m',?)")
            .bind(result.to_string()).execute(&pool).await.unwrap();
        let first_hash = refresh_drafts(&pool, "m").await.unwrap();
        let first = drafts(&pool, "m", &first_hash).await.unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].summary, "补充接口测试");
        assert!(first[0].owner_text.is_empty() && first[0].due_date.is_empty());
        let mut edited = first.clone();
        edited[0].summary = "人工调整任务内容".into();
        edited[0].owner_text = "李四".into();
        save_drafts(&pool, "m", &first_hash, &revision(&first).unwrap(), &edited).await.unwrap();
        std::fs::write(&path, "## 摘要\n人工修改纪要，仍然没有行动项栏目。").unwrap();
        let next_hash = refresh_drafts(&pool, "m").await.unwrap();
        assert_ne!(first_hash, next_hash);
        let next = drafts(&pool, "m", &next_hash).await.unwrap();
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].id, first[0].id);
        assert_eq!(next[0].summary, "人工调整任务内容");
        assert_eq!(next[0].owner_text, "李四");
        let receipt_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM lark_action_receipts").fetch_one(&pool).await.unwrap();
        assert_eq!(receipt_count, 0);
        sqlx::query("UPDATE summary_processes SET result=? WHERE meeting_id='m'")
            .bind(json!({"meeting_action_items":[]}).to_string()).execute(&pool).await.unwrap();
        std::fs::write(&path, "## 行动项\n- 旧纪要中的任务").unwrap();
        assert!(source(&pool, "m").await.unwrap().2.is_empty());
        let hash = refresh_drafts(&pool, "m").await.unwrap();
        let retained = drafts(&pool, "m", &hash).await.unwrap();
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].summary, "人工调整任务内容");
        assert!(retained[0].manually_edited);
    }

    #[test]
    fn independent_duplicate_tasks_retain_owners_and_conflicting_deadlines() {
        let mut items = vec![];
        merge_candidate(&mut items, candidate("发布".into(), "张三周五发布".into(), "张三".into(), "周五".into()).unwrap());
        merge_candidate(&mut items, candidate("发布".into(), "李四周六发布".into(), "李四".into(), "周六".into()).unwrap());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].owner_text, "张三、李四");
        assert_eq!(items[0].due_text, "周五；周六");
        assert!(items[0].due_date.is_empty());
    }

    #[test]
    fn request_omits_absent_fields_and_converts_dates_without_using_today() {
        let mut a = item("a");
        let body = task_body(&a, "例会").unwrap();
        assert!(body.get("members").is_none() && body.get("due").is_none());
        a.owner_text = "张三".into();
        assert!(task_body(&a, "会").is_err());
        a.members = vec![Member {
            id: "ou_a".into(),
            name: "张三".into(),
        }];
        a.due_text = "下周五".into();
        assert!(task_body(&a, "会").is_err());
        a.due_date = "2023-05-01".into();
        let body = task_body(&a, "会").unwrap();
        assert_eq!(
            body["due"],
            json!({"timestamp":"1682899200000","is_all_day":true})
        );
        a.due_date = "2023-05-21".into();
        a.due_time = "15:00".into();
        let body = task_body(&a, "会").unwrap();
        assert_eq!(
            body["due"],
            json!({"timestamp":"1684652400000","is_all_day":false})
        );
        assert_eq!(body["members"][0]["role"], "assignee");
        a.due_date = "2026-02-30".into();
        assert!(task_body(&a, "会").is_err());
    }
    #[test]
    fn repeated_task_rows_preserve_all_owners_and_require_date_conflict_review() {
        let items=extract("## 行动项\n| 任务 | 负责人 | 截止日期 |\n| --- | --- | --- |\n| 发布 | 张三 | 2026-11-20 |\n| 发布 | 李四 | 2026-11-21 |\n");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].owner_text, "张三、李四");
        assert!(items[0].due_date.is_empty());
        assert!(items[0].due_text.contains("2026-11-21"));
        assert_eq!(items[0].evidence.lines().count(), 2);
    }
    #[tokio::test]
    async fn draft_edits_require_current_revision_and_preserve_evidence_and_meeting_boundary() {
        let pool = db().await;
        let old = drafts(&pool, "m", "hash").await.unwrap();
        let mut changed = old.clone();
        changed[0].summary = "人工确认后的任务".into();
        assert!(save_drafts(&pool, "m", "hash", "stale", &changed)
            .await
            .is_err());
        assert!(
            save_drafts(&pool, "other", "hash", &revision(&[]).unwrap(), &changed)
                .await
                .is_err()
        );
        save_drafts(&pool, "m", "hash", &revision(&old).unwrap(), &changed)
            .await
            .unwrap();
        assert_eq!(
            drafts(&pool, "m", "hash").await.unwrap()[0].summary,
            "人工确认后的任务"
        );
        let mut tampered = changed.clone();
        tampered[0].evidence = "伪造原文".into();
        assert!(
            save_drafts(&pool, "m", "hash", &revision(&changed).unwrap(), &tampered)
                .await
                .is_err()
        );
        changed.push(Action {
            id: "new-local".into(),
            evidence: String::new(),
            ..item("unused")
        });
        let current = drafts(&pool, "m", "hash").await.unwrap();
        save_drafts(&pool, "m", "hash", &revision(&current).unwrap(), &changed)
            .await
            .unwrap();
        assert_eq!(drafts(&pool, "m", "hash").await.unwrap().len(), 4);
        pool.close().await;
    }
    #[tokio::test]
    async fn success_and_uncertain_outcomes_are_durable_and_block_duplicate_calls() {
        let pool = db().await;
        let c = AppConfig {
            app_id: "cli_a".into(),
            owner_id: "ou_owner".into(),
            ..Default::default()
        };
        let payload = vec![
            json!({"id":"a","body":{"summary":"one","client_token":"stable-a"}}),
            json!({"id":"b","body":{"summary":"two","client_token":"stable-b"}}),
            json!({"id":"c","body":{"summary":"three","client_token":"stable-c"}}),
        ];
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        send_items(&pool, &c, &payload, move |body| {
            let n = count.fetch_add(1, Ordering::SeqCst);
            async move {
                assert!(body["client_token"].is_string());
                if n == 0 {
                    Outcome::Sent(
                        "guid-a".into(),
                        Some("https://applink.larksuite.com/task/a".into()),
                    )
                } else {
                    Outcome::Unknown("网络中断".into())
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let rows = receipts(&pool, "m", &c).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|r| r["id"] == "a" && r["status"] == "sent"));
        assert!(rows
            .iter()
            .any(|r| r["id"] == "b" && r["status"] == "unknown"));
        let count = calls.clone();
        send_items(&pool, &c, &payload[..2], move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            async { panic!("已发送或未知结果不可重发") }
        })
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        sqlx::query("UPDATE lark_action_receipts SET status='sending' WHERE action_id='b'")
            .execute(&pool)
            .await
            .unwrap();
        send_items(&pool, &c, &payload[..2], |_| async {
            panic!("重启后的发送中状态必须阻止再次创建")
        })
        .await
        .unwrap();
        let other = AppConfig {
            owner_id: "ou_other".into(),
            ..c.clone()
        };
        assert!(receipts(&pool, "m", &other).await.unwrap().is_empty());
        sqlx::query("DELETE FROM meetings WHERE id='m'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(receipts(&pool, "m", &c).await.unwrap().is_empty());
        pool.close().await;
    }
    #[tokio::test]
    async fn definite_failures_stop_batch_and_can_be_retried_only_by_another_call() {
        let pool = db().await;
        let c = AppConfig {
            app_id: "cli_a".into(),
            owner_id: "ou_a".into(),
            ..Default::default()
        };
        let payload = vec![
            json!({"id":"a","body":{"client_token":"once"}}),
            json!({"id":"b","body":{"client_token":"later"}}),
        ];
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        send_items(&pool, &c, &payload, move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            async { Outcome::Failed("权限未开通".into()) }
        })
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            receipts(&pool, "m", &c).await.unwrap()[0]["status"],
            "failed"
        );
        send_items(&pool, &c, &payload[..1], |_| async {
            Outcome::Sent("guid".into(), None)
        })
        .await
        .unwrap();
        assert_eq!(receipts(&pool, "m", &c).await.unwrap()[0]["status"], "sent");
        pool.close().await;
    }
    #[tokio::test]
    async fn saved_preview_rejects_account_source_draft_and_expiry_changes_before_network() {
        let pool = db().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("纪要.md");
        std::fs::write(&path, "## 行动项\n- 补充测试").unwrap();
        sqlx::raw_sql("ALTER TABLE meetings ADD COLUMN title TEXT NOT NULL DEFAULT '例会'; CREATE TABLE summary_markdown_exports(meeting_id TEXT PRIMARY KEY,path TEXT,error TEXT);").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO summary_markdown_exports VALUES('m',?,NULL)")
            .bind(path.to_string_lossy().as_ref())
            .execute(&pool)
            .await
            .unwrap();
        let (_, hash, _) = source(&pool, "m").await.unwrap();
        sqlx::query("UPDATE lark_action_drafts SET source_hash=?")
            .bind(&hash)
            .execute(&pool)
            .await
            .unwrap();
        let rev = revision(&drafts(&pool, "m", &hash).await.unwrap()).unwrap();
        let c = AppConfig {
            enabled: true,
            app_id: "cli_a".into(),
            owner_id: "ou_a".into(),
            tasks_authorized: true,
            ..Default::default()
        };
        sqlx::query("INSERT INTO lark_action_previews VALUES('p','m',?,?,?,'[]',?)")
            .bind(&hash)
            .bind(config_revision(&c))
            .bind(&rev)
            .bind(now())
            .execute(&pool)
            .await
            .unwrap();
        assert!(checked_preview(&pool, &c, "p").await.is_ok());
        assert!(checked_preview(
            &pool,
            &AppConfig {
                owner_id: "ou_other".into(),
                ..c.clone()
            },
            "p"
        )
        .await
        .is_err());
        assert!(checked_preview(
            &pool,
            &AppConfig {
                app_id: "cli_other".into(),
                ..c.clone()
            },
            "p"
        )
        .await
        .is_err());
        std::fs::write(&path, "纪要已改").unwrap();
        assert!(checked_preview(&pool, &c, "p").await.is_err());
        std::fs::write(&path, "## 行动项\n- 补充测试").unwrap();
        let mut changed = drafts(&pool, "m", &hash).await.unwrap();
        changed[0].due_date = "2026-11-30".into();
        save_drafts(&pool, "m", &hash, &rev, &changed)
            .await
            .unwrap();
        assert!(checked_preview(&pool, &c, "p").await.is_err());
        sqlx::query("UPDATE lark_action_previews SET draft_revision=?,created_at=? WHERE id='p'")
            .bind(revision(&changed).unwrap())
            .bind(now() - 901)
            .execute(&pool)
            .await
            .unwrap();
        assert!(checked_preview(&pool, &c, "p").await.is_err());
        assert!(receipts(&pool, "m", &c).await.unwrap().is_empty());
        pool.close().await;
    }
    #[test]
    fn transport_outcomes_are_conservative_and_links_are_constrained() {
        assert!(matches!(
            outcome(403, Some(&json!({"code":1470403}))),
            Outcome::Failed(_)
        ));
        for v in [
            None,
            Some(json!({"code":0,"data":{}})),
            Some(json!({"code":1470422})),
            Some(json!({"code":1470500})),
        ] {
            assert!(matches!(outcome(200, v.as_ref()), Outcome::Unknown(_)));
        }
        assert!(matches!(
            outcome(
                200,
                Some(&json!({"code":0,"data":{"task":{"guid":"a","url":"https://evil.test"}}}))
            ),
            Outcome::Sent(_, None)
        ));
        let base = AppConfig {
            app_id: "cli_a".into(),
            owner_id: "ou_a".into(),
            tasks_authorized: true,
            ..Default::default()
        };
        assert_ne!(
            config_revision(&base),
            config_revision(&AppConfig {
                owner_id: "ou_b".into(),
                ..base.clone()
            })
        );
        assert_ne!(
            config_revision(&base),
            config_revision(&AppConfig {
                tasks_authorized: false,
                ..base.clone()
            })
        );
    }
}
