use super::{app::{self, AppConfig, db_error, now}, oauth, payload::fingerprint};
use crate::{state::AppState, summary::meeting_settings::MeetingSummarySettings};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};
use std::collections::BTreeMap;

const DAY: i64 = 86400;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CalendarMeeting {
    pub key: String,
    pub event_id: String,
    pub series_id: String,
    pub title: String,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    pub meeting_url: Option<String>,
    pub event_url: Option<String>,
    pub revision: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selection { pub key: String, pub revision: String }

pub(crate) struct Capture {
    pub meeting: CalendarMeeting,
    app_id: String,
    owner_id: String,
    invitees: Vec<String>,
    invitation_note: String,
}

fn scope(c: &AppConfig) -> Result<(), String> {
    if !c.enabled || !c.manual_mode || !c.calendar_authorized || c.owner_id.is_empty() {
        return Err("请先连接我的 Lark 日历，启用本地录音模式。".into());
    }
    Ok(())
}

// The fixed API origin receives credentials; links are only returned for opening.
fn safe_link(value: &Value) -> Option<String> {
    let raw = value.as_str()?;
    let url = url::Url::parse(raw).ok()?;
    let host = url.host_str()?;
    (url.scheme() == "https" && (host == "larksuite.com" || host.ends_with(".larksuite.com"))
        && url.username().is_empty() && url.password().is_none() && url.port().is_none())
        .then(|| raw.to_owned())
}

fn timestamp(value: &Value) -> Option<i64> {
    value["timestamp"].as_str().and_then(|v| v.parse().ok())
        .or_else(|| value["timestamp"].as_i64())
        .or_else(|| {
            let date = chrono::NaiveDate::parse_from_str(value["date"].as_str()?, "%Y-%m-%d").ok()?;
            Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp() - 8 * 3600)
        })
}

fn normalize(c: &AppConfig, data: &Value, from: i64, until: i64) -> Result<Vec<CalendarMeeting>, String> {
    // instance_view may omit the optional items field for an empty result.
    // Only accept this after api() has checked HTTP status and code == 0.
    let object = data.as_object().ok_or("日历响应格式异常，保留上次同步结果。")?;
    let items: &[Value] = match object.get("items") {
        None | Some(Value::Null) => &[],
        Some(Value::Array(items)) => items,
        _ => return Err("日历日程列表格式异常，保留上次同步结果。".into()),
    };
    let mut result = BTreeMap::new();
    for item in items {
        if item["status"] == "cancelled" { continue; }
        let id = item["event_id"].as_str().filter(|s| !s.is_empty() && s.len() < 512).ok_or("日程缺少实例标识，请重新同步。")?;
        let start = timestamp(&item["start_time"]).ok_or("日程开始时间无效，请在 Lark 中检查。")?;
        let end = timestamp(&item["end_time"]).ok_or("日程结束时间无效，请在 Lark 中检查。")?;
        if end <= start { return Err("日程结束时间早于开始时间，请在 Lark 中检查。".into()); }
        if end <= from || start >= until { continue; }
        let title = item["summary"].as_str().filter(|s| !s.trim().is_empty()).unwrap_or("未命名日程").to_owned();
        let key = fingerprint(&[&c.app_id, &c.owner_id, "primary", id]);
        let mut meeting = CalendarMeeting {
            key: key.clone(), event_id: id.into(),
            series_id: item["recurring_event_id"].as_str().unwrap_or("").into(),
            title, start, end, all_day: item["start_time"]["date"].as_str().is_some(),
            meeting_url: safe_link(&item["vchat"]["meeting_url"]),
            event_url: safe_link(&item["app_link"]), revision: String::new(),
        };
        meeting.revision = fingerprint(&[&serde_json::to_string(&meeting).map_err(|_| "无法处理日程")?]);
        if let Some(previous) = result.insert(key, meeting.clone()) {
            if previous != meeting { return Err("Lark 返回了无法区分的重复日程实例，本次未同步，请检查日历。".into()); }
        }
    }
    let mut items: Vec<_> = result.into_values().collect();
    items.sort_by_key(|e| e.start);
    Ok(items)
}

async fn fetch(c: &AppConfig, from: i64, until: i64) -> Result<Vec<CalendarMeeting>, String> {
    let path = format!("/open-apis/calendar/v4/calendars/primary/events/instance_view?start_time={from}&end_time={until}&user_id_type=open_id");
    let data = app::api(reqwest::Method::GET, &path, &c.user_token, None).await
        .map_err(|e| format!("日程同步失败。请确认已开通并授权‘读取日程’权限；上次结果不会被清空。{e}"))?;
    normalize(c, &data, from, until)
}

async fn persist(pool: &SqlitePool, c: &AppConfig, items: &[CalendarMeeting], synced: i64) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(db_error)?;
    sqlx::query("DELETE FROM lark_calendar_instances WHERE app_id=? AND owner_id=?")
        .bind(&c.app_id).bind(&c.owner_id).execute(&mut *tx).await.map_err(db_error)?;
    for item in items {
        sqlx::query("INSERT INTO lark_calendar_instances(instance_key,app_id,owner_id,snapshot) VALUES(?,?,?,?)")
            .bind(&item.key).bind(&c.app_id).bind(&c.owner_id).bind(serde_json::to_string(item).map_err(|_| "日程保存失败")?)
            .execute(&mut *tx).await.map_err(db_error)?;
    }
    sqlx::query("INSERT INTO lark_calendar_cache(app_id,owner_id,synced_at) VALUES(?,?,?) ON CONFLICT(app_id,owner_id) DO UPDATE SET synced_at=excluded.synced_at")
        .bind(&c.app_id).bind(&c.owner_id).bind(synced).execute(&mut *tx).await.map_err(db_error)?;
    tx.commit().await.map_err(db_error)
}

async fn view(pool: &SqlitePool, c: &AppConfig) -> Result<Value, String> {
    let connected = scope(c).is_ok() && oauth::authorization_status(c).1;
    let mut events = Vec::new();
    let mut synced: Option<i64> = None;
    if connected {
        synced = sqlx::query_scalar("SELECT synced_at FROM lark_calendar_cache WHERE app_id=? AND owner_id=?")
            .bind(&c.app_id).bind(&c.owner_id).fetch_optional(pool).await.map_err(db_error)?;
        for row in sqlx::query("SELECT i.snapshot,(SELECT r.meeting_id FROM lark_calendar_recordings r JOIN meetings m ON m.id=r.meeting_id WHERE r.instance_key=i.instance_key ORDER BY m.created_at DESC LIMIT 1) AS recorded FROM lark_calendar_instances i WHERE i.app_id=? AND i.owner_id=?")
            .bind(&c.app_id).bind(&c.owner_id).fetch_all(pool).await.map_err(db_error)? {
            let mut item: Value = serde_json::from_str(&row.get::<String,_>("snapshot")).map_err(|_| "日程缓存损坏，请重新同步。")?;
            item["recordedMeetingId"] = json!(row.get::<Option<String>,_>("recorded"));
            events.push(item);
        }
        events.sort_by_key(|v| v["start"].as_i64().unwrap_or(0));
    }
    Ok(json!({"configured": !c.app_id.is_empty() && !c.secret.is_empty(), "connected": connected,
        "accountKey": fingerprint(&[&c.app_id,&c.owner_id]), "authRevision":c.calendar_auth_revision, "syncedAt": synced, "events": events}))
}

#[tauri::command]
pub async fn lark_calendar_status(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    let pool = state.db_manager.pool();
    view(pool, &app::load(pool).await?).await
}

#[tauri::command]
pub async fn lark_calendar_sync(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    let _storage = crate::audio::storage_migration::use_storage().await?;
    let _guard = app::CONFIG_LOCK.lock().await;
    let pool = state.db_manager.pool();
    scope(&app::load(pool).await?)?;
    let c = oauth::authorized_config_locked(pool).await?;
    let current = now();
    // A rolling window includes ongoing/recent meetings and stays below Lark's 40-day limit.
    let items = fetch(&c, current - DAY, current + 14 * DAY).await?;
    persist(pool, &c, &items, current).await?;
    view(pool, &c).await
}

async fn selected(pool: &SqlitePool, c: &AppConfig, selection: &Selection) -> Result<CalendarMeeting, String> {
    let saved: String = sqlx::query_scalar("SELECT snapshot FROM lark_calendar_instances WHERE instance_key=? AND app_id=? AND owner_id=?")
        .bind(&selection.key).bind(&c.app_id).bind(&c.owner_id).fetch_optional(pool).await.map_err(db_error)?
        .ok_or("这次日程已失效或账号已切换，请重新同步并选择。")?;
    let saved: CalendarMeeting = serde_json::from_str(&saved).map_err(|_| "日程缓存损坏，请重新同步。")?;
    if saved.revision != selection.revision { return Err("日程已变化，请重新同步并选择。".into()); }
    if saved.end - saved.start > 30 * DAY { return Err("请选择具体的一次会议。".into()); }
    verify_fresh(&saved, selection, fetch(c, saved.start - DAY, saved.end + DAY).await?)
}

fn verify_fresh(saved: &CalendarMeeting, selection: &Selection, items: Vec<CalendarMeeting>) -> Result<CalendarMeeting, String> {
    let fresh = items.into_iter().find(|m| m.key == saved.key)
        .ok_or("这次日程已取消、移出原时间范围或无法访问，请重新同步。")?;
    if fresh.key != selection.key || fresh.revision != selection.revision {
        return Err("日程时间、名称或链接已变化，请重新同步后再操作。".into());
    }
    Ok(fresh)
}

fn event_path(id: &str) -> String {
    let mut url = url::Url::parse("https://open.larksuite.com/open-apis/calendar/v4/calendars/primary/events/").unwrap();
    url.path_segments_mut().unwrap().pop_if_empty().push(id);
    url.path().to_owned()
}

async fn invitees(c: &AppConfig, meeting: &CalendarMeeting) -> Result<(Vec<String>, String), String> {
    let mut names = Vec::new();
    let mut unnamed = false;
    let mut cursor = String::new();
    for _ in 0..20 {
        let query = {
            let mut q = url::form_urlencoded::Serializer::new(String::new());
            q.append_pair("page_size", "100").append_pair("user_id_type", "open_id");
            if !cursor.is_empty() { q.append_pair("page_token", &cursor); }
            q.finish()
        };
        let path = format!("{}/attendees?{query}", event_path(&meeting.event_id));
        let data = app::api(reqwest::Method::GET, &path, &c.user_token, None).await?;
        for item in data["items"].as_array().ok_or("邀请名单响应无效")? {
            if item["type"] == "resource" || item["type"] == "room" { continue; }
            if let Some(name) = item["display_name"].as_str().filter(|s| !s.trim().is_empty()) {
                if !names.iter().any(|n| n == name) { names.push(name.to_owned()); }
            } else { unnamed = true; }
            if names.len() >= 100 || names.iter().map(String::len).sum::<usize>() > 6000 {
                return Ok((names, "邀请名单较长，仅带入部分姓名，请在 Lark 核对完整名单。".into()));
            }
        }
        if data["has_more"].as_bool() != Some(true) {
            return Ok((names, if unnamed { "部分邀请对象未返回姓名，请在 Lark 核对完整名单。".into() } else { String::new() }));
        }
        let next = data["page_token"].as_str().filter(|s| !s.is_empty() && *s != cursor).ok_or("邀请名单分页异常")?;
        cursor = next.into();
    }
    Err("邀请人数超出加载范围，请在 Lark 核对。".into())
}

// Caller holds CONFIG_LOCK until capture registration is finished.
pub(crate) async fn prepare_locked(pool: &SqlitePool, selection: &Selection) -> Result<Capture, String> {
    scope(&app::load(pool).await?)?;
    let c = oauth::authorized_config_locked(pool).await?;
    let meeting = selected(pool, &c, selection).await?;
    // Missing attendee access does not prevent recording a verified event.
    let (invitees, invitation_note) = invitees(&c, &meeting).await.unwrap_or_else(|_| (Vec::new(), "未能读取邀请名单，请会后手动核对人员。".into()));
    Ok(Capture { meeting, app_id: c.app_id, owner_id: c.owner_id, invitees, invitation_note })
}

pub(crate) async fn attach(pool: &SqlitePool, id: &str, capture: &Capture) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(db_error)?;
    let snapshot = json!({"event":capture.meeting,"invitees":capture.invitees,"invitationNote":capture.invitation_note});
    sqlx::query("INSERT INTO lark_calendar_recordings(meeting_id,instance_key,app_id,owner_id,snapshot) VALUES(?,?,?,?,?)")
        .bind(id).bind(&capture.meeting.key).bind(&capture.app_id).bind(&capture.owner_id).bind(snapshot.to_string())
        .execute(&mut *tx).await.map_err(db_error)?;
    let settings = MeetingSummarySettings {
        custom_prompt: format!("关联日程：{}。以下为邀请人员，不代表实际出席，不得据此推断出席或缺席：{}。{}", capture.meeting.title, capture.invitees.join("、"), capture.invitation_note),
        ..Default::default()
    };
    sqlx::query("INSERT INTO meeting_summary_settings(meeting_id,settings_json,updated_at) VALUES(?,?,?)")
        .bind(id).bind(serde_json::to_string(&settings).map_err(|_| "会议背景保存失败")?).bind(chrono::Utc::now().to_rfc3339())
        .execute(&mut *tx).await.map_err(db_error)?;
    tx.commit().await.map_err(db_error)
}

#[tauri::command]
pub async fn lark_calendar_meeting(state: tauri::State<'_,AppState>, meeting_id: String) -> Result<Option<Value>,String> {
    let value: Option<String> = sqlx::query_scalar("SELECT snapshot FROM lark_calendar_recordings WHERE meeting_id=?")
        .bind(meeting_id).fetch_optional(state.db_manager.pool()).await.map_err(db_error)?;
    value.map(|s| serde_json::from_str(&s).map_err(|_| "日程关联记录损坏".into())).transpose()
}

#[tauri::command]
pub async fn lark_calendar_link(state: tauri::State<'_,AppState>, selection: Selection, join: bool) -> Result<String,String> {
    let _guard = app::CONFIG_LOCK.lock().await;
    let pool = state.db_manager.pool();
    scope(&app::load(pool).await?)?;
    let c = oauth::authorized_config_locked(pool).await?;
    let meeting = selected(pool, &c, &selection).await?;
    (if join { meeting.meeting_url } else { meeting.event_url }).ok_or("日程未提供可用链接，请在 Lark 客户端打开。".into())
}

#[tauri::command]
pub async fn lark_calendar_start_recording(app: tauri::AppHandle, selection: Selection, mic_device_name: Option<String>, system_device_name: Option<String>) -> Result<(),String> {
    crate::audio::recording_commands::start_calendar_recording(app, mic_device_name, system_device_name, selection).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> AppConfig { AppConfig { app_id:"app".into(),owner_id:"one".into(),..Default::default() } }
    fn event(id: &str, start: i64) -> Value {
        json!({"event_id":id,"recurring_event_id":"series","summary":"周会","start_time":{"timestamp":start.to_string()},"end_time":{"timestamp":(start+3600).to_string()},"status":"confirmed","vchat":{"meeting_url":"https://vc.larksuite.com/j/123456789"}})
    }
    #[test]
    fn optional_empty_instance_list_is_valid_but_malformed_data_is_not() {
        for data in [json!({}), json!({"items": null}), json!({"items": []})] {
            assert!(normalize(&config(), &data, 0, 10000).unwrap().is_empty());
        }
        for data in [Value::Null, json!([]), json!({"items": {}}), json!({"items": "invalid"})] {
            assert!(normalize(&config(), &data, 0, 10000).is_err());
        }
    }
    #[test]
    fn recurring_instances_are_distinct_and_reschedules_keep_identity() {
        let c=config(); let first=event("series_100",100); let second=event("series_200",200);
        let items=normalize(&c,&json!({"items":[first.clone(),second]}),0,10000).unwrap();
        assert_eq!(items.len(),2); assert_ne!(items[0].key,items[1].key);
        let changed=normalize(&c,&json!({"items":[event("series_100",300)]}),0,10000).unwrap();
        assert_eq!(items[0].key,changed[0].key); assert_ne!(items[0].revision,changed[0].revision);
        let mut other=c.clone();other.owner_id="two".into();
        assert_ne!(items[0].key,normalize(&other,&json!({"items":[first]}),0,10000).unwrap()[0].key);
    }
    #[test]
    fn cancelled_and_duplicate_instances_and_untrusted_links() {
        let mut cancelled=event("cancelled",100);cancelled["status"]=json!("cancelled");
        let first=event("first",100);
        assert_eq!(normalize(&config(),&json!({"items":[first.clone(),first,cancelled]}),0,10000).unwrap().len(),1);
        assert!(normalize(&config(),&json!({"items":[event("same",100),event("same",200)]}),0,10000).is_err());
        for url in ["https://larksuite.com.evil/j/1","javascript:alert(1)","https://user@vc.larksuite.com/j/1"] { assert!(safe_link(&json!(url)).is_none()); }
        assert_eq!(timestamp(&json!({"date":"1970-01-02"})),Some(DAY-8*3600));
    }
    #[test]
    fn stale_or_cancelled_instance_cannot_start_from_an_old_selection() {
        let c=config();
        let saved=normalize(&c,&json!({"items":[event("series_100",100)]}),0,10000).unwrap().remove(0);
        let selection=Selection{key:saved.key.clone(),revision:saved.revision.clone()};
        assert!(verify_fresh(&saved,&selection,vec![]).is_err());
        let changed=normalize(&c,&json!({"items":[event("series_100",300)]}),0,10000).unwrap();
        assert!(verify_fresh(&saved,&selection,changed).is_err());
        assert!(verify_fresh(&saved,&selection,vec![saved.clone()]).is_ok());
    }
    #[tokio::test]
    async fn sync_replaces_only_current_account_and_preserves_recordings() {
        let pool=SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("CREATE TABLE meetings(id TEXT PRIMARY KEY,created_at TEXT); INSERT INTO meetings VALUES('m','now'); CREATE TABLE meeting_summary_settings(meeting_id TEXT PRIMARY KEY,settings_json TEXT,updated_at TEXT);").execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261114010000_lark_calendar.sql")).execute(&pool).await.unwrap();
        let c=config(); let items=normalize(&c,&json!({"items":[event("series_100",100)]}),0,10000).unwrap();
        persist(&pool,&c,&items,1).await.unwrap();
        let mut other=c.clone(); other.owner_id="other".into();
        let other_items=normalize(&other,&json!({"items":[event("series_100",100)]}),0,10000).unwrap();
        persist(&pool,&other,&other_items,2).await.unwrap();
        let capture=Capture{meeting:items[0].clone(),app_id:c.app_id.clone(),owner_id:c.owner_id.clone(),invitees:vec!["甲".into()],invitation_note:String::new()};
        attach(&pool,"m",&capture).await.unwrap();
        persist(&pool,&c,&[],3).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM lark_calendar_instances").fetch_one(&pool).await.unwrap(),1);
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM lark_calendar_recordings").fetch_one(&pool).await.unwrap(),1);
        let raw:String=sqlx::query_scalar("SELECT settings_json FROM meeting_summary_settings").fetch_one(&pool).await.unwrap();
        let settings:MeetingSummarySettings=serde_json::from_str(&raw).unwrap();
        assert!(settings.participants.is_empty() && settings.absent.is_empty());
        assert!(settings.custom_prompt.contains("不代表实际出席"));
    }
}
