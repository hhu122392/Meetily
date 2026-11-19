//! Each desktop reads selected groups using its own user token. No shared event consumer.
use super::{
    app::{self, db_error, now, AppConfig},
    invitations, oauth,
    routing::{self, MeetingReference},
};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const DISCOVERY_INTERVAL: Duration = Duration::from_secs(300);
const MESSAGE_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupSyncMode {
    #[default]
    Automatic,
    Selected,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncGroup {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub since: i64,
}

pub(super) fn configure_groups(c: &mut AppConfig, mode: Option<GroupSyncMode>, groups: Option<Vec<SyncGroup>>, clock: i64) -> Result<(), String> {
    if c.manual_sync_groups.is_none() {
        c.manual_sync_groups = Some(c.sync_groups.clone());
    }
    if let Some(mode) = mode { c.group_sync_mode = mode; }
    if c.group_sync_mode == GroupSyncMode::Selected {
        let input = groups.unwrap_or_else(|| c.manual_sync_groups.clone().unwrap_or_default());
        c.sync_groups = selected_groups(&c.sync_groups, input, clock)?;
        c.manual_sync_groups = Some(c.sync_groups.clone());
    }
    Ok(())
}

#[derive(Default)]
struct DiscoverySchedule {
    identity: (String, String),
    last: Option<Instant>,
    since: i64,
}
impl DiscoverySchedule {
    fn due(&mut self, c: &AppConfig, at: Instant, clock: i64) -> bool {
        let identity = (c.app_id.clone(), c.owner_id.clone());
        if self.identity != identity {
            self.identity = identity;
            self.last = None;
            self.since = clock;
        }
        self.last.is_none_or(|last| at.duration_since(last) >= DISCOVERY_INTERVAL)
    }
    fn completed(&mut self, at: Instant, started: i64) {
        self.last = Some(at);
        self.since = started;
    }
}

fn valid_chat(id: &str) -> bool {
    id.starts_with("oc_")
        && id.len() <= 100
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

pub(super) fn selected_groups(
    old: &[SyncGroup],
    input: Vec<SyncGroup>,
    clock: i64,
) -> Result<Vec<SyncGroup>, String> {
    let mut out = Vec::<SyncGroup>::new();
    for g in input {
        if !valid_chat(&g.id) || g.name.len() > 500 {
            return Err("同步群信息无效，请重新加载群列表。".into());
        }
        if out.iter().any(|v| v.id == g.id) {
            continue;
        }
        let since = old
            .iter()
            .find(|v| v.id == g.id)
            .map(|v| v.since)
            .filter(|v| *v > 0)
            .unwrap_or(clock);
        out.push(SyncGroup {
            id: g.id,
            name: g.name,
            since,
        });
    }
    Ok(out)
}

async fn groups(token: &str) -> Result<Vec<SyncGroup>, String> {
    let mut result = Vec::new();
    let mut cursor = String::new();
    for _ in 0..50 {
        let q = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("page_size", "100")
            .append_pair("page_token", &cursor)
            .finish();
        let v = app::api(
            reqwest::Method::GET,
            &format!("/open-apis/im/v1/chats?{q}"),
            token,
            None,
        )
        .await?;
        for item in v["items"]
            .as_array()
            .ok_or("群列表响应缺少列表，请重新加载。")?
        {
            if let Some(id) = item["chat_id"].as_str().filter(|v| valid_chat(v)) {
                result.push(SyncGroup {
                    id: id.into(),
                    name: item["name"].as_str().unwrap_or("未命名群").into(),
                    since: 0,
                });
            }
        }
        if !v["has_more"].as_bool().ok_or("群列表分页标识缺失。")? {
            return Ok(result);
        }
        cursor = next_page(&v, &cursor)?;
    }
    Err("群列表过长，未加载完整，请联系管理员缩小可用范围。".into())
}

#[tauri::command]
pub async fn lark_sync_groups(state: tauri::State<'_, AppState>) -> Result<Vec<SyncGroup>, String> {
    let _lock = app::CONFIG_LOCK.lock().await;
    let c = oauth::authorized_config_locked(state.db_manager.pool()).await?;
    discover_groups(&c).await
}

async fn discover_groups(c: &AppConfig) -> Result<Vec<SyncGroup>, String> {
    let mine = groups(&c.user_token)
        .await
        .map_err(|e| format!("无法读取本人所在群：{e} 请重新登录并授予群列表权限。"))?;
    let bot = groups(&app::tenant_token(c).await?).await?;
    Ok(mine
        .into_iter()
        .filter(|g| bot.iter().any(|b| b.id == g.id))
        .collect())
}

// Fetch outside the config lock, then recheck policy and identity before persisting.
async fn apply_discovered_groups(pool: &SqlitePool, snapshot: &AppConfig, groups: Vec<SyncGroup>, since: i64, cancel: &CancellationToken) -> Result<Option<AppConfig>, String> {
    let _policy = super::auto_recording::POLICY_LOCK.lock().await;
    let _lock = app::CONFIG_LOCK.lock().await;
    let mut latest = app::load(pool).await?;
    if cancel.is_cancelled() || !latest.enabled || latest.manual_mode
        || latest.group_sync_mode != GroupSyncMode::Automatic
        || latest.app_id != snapshot.app_id || latest.owner_id != snapshot.owner_id || latest.secret != snapshot.secret {
        return Ok(None);
    }
    let updated = selected_groups(&latest.sync_groups, groups, since)?;
    let needs_save = updated != latest.sync_groups || latest.manual_sync_groups.is_none();
    if needs_save {
        if latest.manual_sync_groups.is_none() { latest.manual_sync_groups = Some(latest.sync_groups.clone()); }
        for removed in latest.sync_groups.iter().filter(|g| !updated.iter().any(|v| v.id == g.id)) {
            sqlx::query("UPDATE lark_app_routes SET local_record_bound_at=NULL WHERE app_id=? AND owner_id=? AND chat_id=?")
                .bind(&latest.app_id).bind(&latest.owner_id).bind(&removed.id).execute(pool).await.map_err(db_error)?;
        }
        latest.sync_groups = updated;
        app::save(pool, &latest).await?;
    }
    Ok(Some(latest))
}

fn next_page(v: &Value, previous: &str) -> Result<String, String> {
    v["page_token"]
        .as_str()
        .filter(|s| !s.is_empty() && *s != previous)
        .map(str::to_owned)
        .ok_or("群消息分页异常，未跳过未读消息。".into())
}

struct Command {
    id: String,
    sender: String,
    time: i64,
    text: String,
}
fn command(item: &Value, bot: &str, since: i64, clock: i64) -> Option<Command> {
    if item["deleted"] == true
        || item["sender"]["sender_type"] != "user"
        || item["sender"]["id_type"] != "open_id"
    {
        return None;
    }
    let time = item["create_time"].as_str()?.parse::<i64>().ok()? / 1000;
    if time < since || time < clock - 600 || time > clock + 60 {
        return None;
    }
    if !item["mentions"]
        .as_array()?
        .iter()
        .any(|m| m["id"].as_str() == Some(bot) && m["id_type"] == "open_id")
    {
        return None;
    }
    let body: Value = serde_json::from_str(item["body"]["content"].as_str()?).ok()?;
    let mut text = invitations::message_text(item["msg_type"].as_str()?, &body)?;
    // Strip only real mention placeholders, never names typed as plain text.
    for mention in item["mentions"].as_array()? {
        if let Some(key) = mention["key"].as_str().filter(|k| !k.is_empty()) {
            text = text.replace(key, "");
        }
    }
    if text.len() > 16000 {
        return None;
    }
    Some(Command {
        id: item["message_id"].as_str()?.into(),
        sender: item["sender"]["id"].as_str()?.into(),
        time,
        text: text.trim().into(),
    })
}

async fn number(c: &AppConfig, cmd: &Command) -> Option<Result<String, String>> {
    if let Some(invitation) = invitations::parse(&cmd.text, cmd.time) {
        return Some(match invitation {
            Err(e) => Err(e),
            Ok(i) => {
                if let Some(no) = i.number {
                    Ok(no)
                } else {
                    match super::calendar::resolve_invitation(c, &i, None).await {
                        Ok(m) => m
                            .meeting_url
                            .as_deref()
                            .and_then(invitations::meeting_link)
                            .map(|v| v.0)
                            .ok_or("日程中没有可核对的会议号，请改发会议号或入会链接。".into()),
                        Err(e) => Err(e),
                    }
                }
            }
        });
    }
    match routing::parse_binding(&cmd.text) {
        Some(MeetingReference::Number(no)) => Some(Ok(no)),
        Some(MeetingReference::Id(_)) => {
            Some(Err("请发送九位会议号，如：会议ID:123 456 789。".into()))
        }
        None if cmd.text.starts_with("会议") || cmd.text.starts_with("开会") => Some(Err(
            "请发送已有会议号、入会链接或完整邀请；已停用创建预约。".into(),
        )),
        _ => None,
    }
}

// Caller holds CONFIG_LOCK then DELIVERY_LOCK; account/config changes cannot race a binding or reply.
async fn bind(
    pool: &SqlitePool,
    c: &AppConfig,
    chat: &str,
    cmd: &Command,
    no: &str,
) -> Result<bool, String> {
    let mut tx = pool.begin().await.map_err(db_error)?;
    let inserted=sqlx::query("INSERT OR IGNORE INTO lark_group_commands(app_id,owner_id,message_id,chat_id,command_at,meeting_no,detail) VALUES(?,?,?,?,?,?,?)")
        .bind(&c.app_id).bind(&c.owner_id).bind(&cmd.id).bind(chat).bind(cmd.time).bind(no)
        .bind("已同步，等待本人入会；本机录音开关须提前开启。").execute(&mut *tx).await.map_err(db_error)?.rows_affected()>0;
    if inserted {
        // Reposting a number cannot detach a route already attached to a concrete meeting instance.
        let bound = (c.auto_record && cmd.time >= c.auto_record_since).then_some(cmd.time);
        sqlx::query("INSERT INTO lark_app_routes(app_id,owner_id,reference,reference_kind,chat_id,expires_at,local_record_bound_at) VALUES(?,?,?,'number',?,?,?) ON CONFLICT(app_id,owner_id,reference,reference_kind,chat_id) DO UPDATE SET remote_id=CASE WHEN lark_app_routes.expires_at<? OR EXISTS(SELECT 1 FROM lark_auto_recordings a WHERE a.app_id=lark_app_routes.app_id AND a.owner_id=lark_app_routes.owner_id AND a.remote_id=lark_app_routes.remote_id AND a.status NOT IN ('starting','recording')) THEN NULL ELSE lark_app_routes.remote_id END,expires_at=excluded.expires_at,local_record_bound_at=excluded.local_record_bound_at,invitation_id=NULL WHERE excluded.expires_at>=lark_app_routes.expires_at")
            .bind(&c.app_id).bind(&c.owner_id).bind(no).bind(chat).bind(cmd.time+21600).bind(bound).bind(cmd.time)
            .execute(&mut *tx).await.map_err(db_error)?;
    }
    tx.commit().await.map_err(db_error)?;
    if inserted {
        super::meeting_state::invalidate();
    }
    Ok(inserted)
}

trait ReplyApi {
    async fn send(&self, c: &AppConfig, body: Value) -> Result<Value, String>;
}
trait SyncApi: ReplyApi {
    async fn history(&self, path: &str, token: &str) -> Result<Value, String>;
}
struct Production;
// Per-process pacing also covers pagination and overlapping workers during a settings restart.
struct ReadPacer(tokio::sync::Mutex<Option<tokio::time::Instant>>);
impl ReadPacer {
    const fn new() -> Self {
        Self(tokio::sync::Mutex::const_new(None))
    }
    async fn wait(&self) {
        let mut last = self.0.lock().await;
        if let Some(previous) = *last {
            tokio::time::sleep_until(previous + Duration::from_secs(1)).await;
        }
        *last = Some(tokio::time::Instant::now());
    }
}
static HISTORY_PACER: ReadPacer = ReadPacer::new();
impl SyncApi for Production {
    async fn history(&self, path: &str, token: &str) -> Result<Value, String> {
        HISTORY_PACER.wait().await;
        app::api(reqwest::Method::GET, path, token, None).await
    }
}
impl ReplyApi for Production {
    async fn send(&self, c: &AppConfig, body: Value) -> Result<Value, String> {
        let token = app::tenant_token(c).await?;
        app::api(
            reqwest::Method::POST,
            "/open-apis/im/v1/messages?receive_id_type=chat_id",
            &token,
            Some(body),
        )
        .await
    }
}

async fn accept(
    pool: &SqlitePool,
    c: &AppConfig,
    chat: &str,
    cmd: Command,
    api: &impl ReplyApi,
) -> Result<(), String> {
    let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM lark_group_commands WHERE app_id=? AND owner_id=? AND message_id=?)")
        .bind(&c.app_id).bind(&c.owner_id).bind(&cmd.id).fetch_one(pool).await.map_err(db_error)?;
    if exists {
        return Ok(());
    }
    let Some(result) = number(c, &cmd).await else {
        return Ok(());
    };
    let _config = app::CONFIG_LOCK.lock().await;
    let latest = app::load(pool).await?;
    if cmd.time < now() - 600
        || !latest.enabled
        || latest.manual_mode
        || latest.app_id != c.app_id
        || latest.owner_id != c.owner_id
        || !latest
            .sync_groups
            .iter()
            .any(|g| g.id == chat && cmd.time >= g.since)
    {
        return Ok(());
    }
    let c = &latest;
    let _delivery = app::DELIVERY_LOCK.lock().await;
    let (text, inserted) = match result {
        Ok(no) => {
            let inserted = bind(pool, c, chat, &cmd, &no).await?;
            (format!("已登记会议 {} {} {}，现在可以入会。\n已登录、同步本群并开启自动录音的 Meetily，会在检测到各自账号入会后开始本地录音。",&no[..3],&no[3..6],&no[6..]),inserted)
        }
        Err(e) => {
            let inserted=sqlx::query("INSERT OR IGNORE INTO lark_group_commands(app_id,owner_id,message_id,chat_id,command_at,detail) VALUES(?,?,?,?,?,?)")
                .bind(&c.app_id).bind(&c.owner_id).bind(&cmd.id).bind(chat).bind(cmd.time).bind(&e).execute(pool).await.map_err(db_error)?.rows_affected()>0;
            (format!("会议指令未绑定：{e}"), inserted)
        }
    };
    if !inserted || cmd.sender != c.owner_id {
        return Ok(());
    }
    // Persist uncertainty before the external mutation. Never retry an uncertain send on restart.
    sqlx::query("UPDATE lark_group_commands SET ack_state='unknown' WHERE app_id=? AND owner_id=? AND message_id=?")
        .bind(&c.app_id).bind(&c.owner_id).bind(&cmd.id).execute(pool).await.map_err(db_error)?;
    // The accepted command and uncertainty are durable. Do not block microphone state queries on reply I/O.
    drop(_delivery);
    drop(_config);
    let uuid = super::payload::fingerprint(&["group-command-reply", &c.app_id, chat, &cmd.id]);
    let reply=api.send(c,json!({"receive_id":chat,"msg_type":"text","content":json!({"text":text}).to_string(),"uuid":&uuid[..32]})).await;
    let sent = reply
        .as_ref()
        .is_ok_and(|v| v["message_id"].as_str().is_some_and(|s| !s.is_empty()));
    sqlx::query(
        "UPDATE lark_group_commands SET ack_state=? WHERE app_id=? AND owner_id=? AND message_id=?",
    )
    .bind(if sent { "sent" } else { "unknown" })
    .bind(&c.app_id)
    .bind(&c.owner_id)
    .bind(&cmd.id)
    .execute(pool)
    .await
    .map_err(db_error)?;
    if !sent {
        return Err("会议绑定已处理，群回执送达不确定；请查看指令记录，不会自动补发。".into());
    }
    Ok(())
}

async fn sync_chat(
    pool: &SqlitePool,
    c: &AppConfig,
    g: &SyncGroup,
    bot: &str,
    api: &impl SyncApi,
) -> Result<(), String> {
    let saved=sqlx::query("SELECT through_time,window_start,window_end,page_token FROM lark_group_sync_cursors WHERE app_id=? AND owner_id=? AND chat_id=?")
        .bind(&c.app_id).bind(&c.owner_id).bind(&g.id).fetch_optional(pool).await.map_err(db_error)?;
    let clock = now();
    let mut end = clock - 2;
    let through = saved
        .as_ref()
        .map(|r| r.get::<i64, _>("through_time"))
        .unwrap_or(g.since);
    let mut start = through.saturating_sub(2).max(g.since).max(clock - 600);
    let mut cursor = String::new();
    if let Some(r) = saved.as_ref() {
        let previous = r.get::<i64, _>("window_start");
        let token = r.get::<String, _>("page_token");
        if !token.is_empty() && previous >= g.since && previous >= clock - 600 {
            start = previous;
            end = r.get("window_end");
            cursor = token;
        }
    }
    if start >= end {
        return Ok(());
    }
    for _ in 0..4 {
        let q = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("container_id_type", "chat")
            .append_pair("container_id", &g.id)
            .append_pair("start_time", &start.to_string())
            .append_pair("end_time", &end.to_string())
            .append_pair("sort_type", "ByCreateTimeAsc")
            .append_pair("page_size", "50")
            .append_pair("page_token", &cursor)
            .finish();
        let v = api
            .history(&format!("/open-apis/im/v1/messages?{q}"), &c.user_token)
            .await
            .map_err(|e| {
                format!(
                    "{}：{e} 请确认本人仍在群内，并已授予以用户身份读取群消息权限。",
                    g.name
                )
            })?;
        for item in v["items"]
            .as_array()
            .ok_or("群消息响应缺少列表，保留同步进度。")?
        {
            if item["chat_id"].as_str().is_some_and(|id| id != g.id) {
                continue;
            }
            if let Some(cmd) = command(item, bot, g.since, clock) {
                accept(pool, c, &g.id, cmd, api).await?;
            }
        }
        if !v["has_more"]
            .as_bool()
            .ok_or("群消息分页标识缺失，保留同步进度。")?
        {
            sqlx::query("INSERT INTO lark_group_sync_cursors(app_id,owner_id,chat_id,through_time) VALUES(?,?,?,?) ON CONFLICT(app_id,owner_id,chat_id) DO UPDATE SET through_time=excluded.through_time,window_start=0,window_end=0,page_token=''")
                .bind(&c.app_id).bind(&c.owner_id).bind(&g.id).bind(end).execute(pool).await.map_err(db_error)?;
            return Ok(());
        }
        cursor = next_page(&v, &cursor)?;
        sqlx::query("INSERT INTO lark_group_sync_cursors VALUES(?,?,?,?,?,?,?) ON CONFLICT(app_id,owner_id,chat_id) DO UPDATE SET window_start=excluded.window_start,window_end=excluded.window_end,page_token=excluded.page_token")
            .bind(&c.app_id).bind(&c.owner_id).bind(&g.id).bind(through).bind(start).bind(end).bind(&cursor).execute(pool).await.map_err(db_error)?;
    }
    Err("群消息较多，已保存分页进度，下轮继续读取。".into())
}

pub(super) async fn run(pool: SqlitePool, cancel: CancellationToken) {
    let track_pool = pool.clone();
    let track_cancel = cancel.clone();
    let tracker = tokio::spawn(async move {
        loop {
            tokio::select! {_=track_cancel.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(15))=>{}}
            if let Err(e) = app::match_current(&track_pool).await {
                app::status(e).await;
            }
        }
    });
    let mut bot_cache: Option<(String, String, String)> = None;
    let mut discovery = DiscoverySchedule::default();
    let mut delay = MESSAGE_INTERVAL.as_secs();
    loop {
        if cancel.is_cancelled() {
            break;
        }
        let round_started = std::time::Instant::now();
        let result: Result<String, String> = async {
            if cancel.is_cancelled() {
                return Ok(String::new());
            }
            let config = app::load(&pool).await?;
            if !config.enabled || config.manual_mode {
                return Ok("群同步已关闭".into());
            }
            if config.group_sync_mode == GroupSyncMode::Selected && config.sync_groups.is_empty() {
                return Ok("请选择要同步的会议群并保存。".into());
            }
            let mut c = oauth::authorized_config(&pool).await?;
            if c.group_sync_mode == GroupSyncMode::Automatic {
                if discovery.due(&c, Instant::now(), now()) {
                    let started = now();
                    let groups = discover_groups(&c).await.map_err(|e| format!("自动发现会议群失败，保留原有群信息：{e}"))?;
                    let Some(updated) = apply_discovered_groups(&pool, &c, groups, discovery.since, &cancel).await? else { return Ok(String::new()); };
                    c = updated;
                    discovery.completed(Instant::now(), started);
                }
                if c.sync_groups.is_empty() {
                    return Ok("暂未发现你和会议助手都在的群；每 5 分钟自动更新，请先将助手加入会议群。".into());
                }
            }
            if bot_cache
                .as_ref()
                .is_none_or(|b| b.0 != c.app_id || b.1 != c.secret)
            {
                let token = app::tenant_token(&c).await?;
                // Bot info has a top-level `bot` object, unlike standard APIs' `data` envelope.
                let response = app::client()?
                    .get(format!("{}/open-apis/bot/v3/info", app::BASE))
                    .bearer_auth(token)
                    .send()
                    .await
                    .map_err(|_| "无法核对会议助手身份。")?;
                let v: Value = response
                    .json()
                    .await
                    .map_err(|_| "会议助手身份响应异常。")?;
                if v["code"] != 0 {
                    return Err("无法核对会议助手身份，请检查应用机器人能力和权限。".into());
                }
                let id = v["bot"]["open_id"]
                    .as_str()
                    .filter(|s| s.starts_with("ou_"))
                    .ok_or("会议助手没有返回有效身份。")?;
                bot_cache = Some((c.app_id.clone(), c.secret.clone(), id.into()));
            }
            let mut errors = Vec::new();
            for g in &c.sync_groups {
                if cancel.is_cancelled() {
                    break;
                }
                if let Err(e) =
                    sync_chat(&pool, &c, g, &bot_cache.as_ref().unwrap().2, &Production).await
                {
                    errors.push(e);
                }
            }
            if errors.is_empty() {
                Ok(format!(
                    "已完成 {} 个群的本轮同步；逐群读取，每轮至少 15 秒，群多或翻页时耗时会增加。",
                    c.sync_groups.len()
                ))
            } else {
                Err(errors.join("\n"))
            }
        }
        .await;
        let failed = result.is_err();
        match result {
            Ok(message) => {
                delay = MESSAGE_INTERVAL.as_secs();
                if !message.is_empty() {
                    app::status(message).await;
                }
            }
            Err(e) => {
                delay = (delay * 2).min(300);
                app::status(format!("{e} {delay} 秒后重试读取。不会重发不确定的回执。")).await;
            }
        }
        let pause = if failed {
            Duration::from_secs(delay)
        } else {
            MESSAGE_INTERVAL
                .saturating_sub(round_started.elapsed())
                .max(Duration::from_secs(1))
        };
        tokio::select! {_=cancel.cancelled()=>break,_=tokio::time::sleep(pause)=>{}}
    }
    tracker.abort();
}

pub(super) async fn recent(pool: &SqlitePool, c: &AppConfig) -> Result<Vec<Value>, String> {
    let rows=sqlx::query("SELECT message_id,chat_id,meeting_no,detail,ack_state FROM lark_group_commands WHERE app_id=? AND owner_id=? ORDER BY command_at DESC LIMIT 20")
        .bind(&c.app_id).bind(&c.owner_id).fetch_all(pool).await.map_err(db_error)?;
    Ok(rows.iter().map(|r|json!({"id":r.get::<String,_>("message_id"),"chat_id":r.get::<String,_>("chat_id"),"meeting_no":r.get::<Option<String>,_>("meeting_no"),"detail":r.get::<String,_>("detail"),"ack_state":r.get::<String,_>("ack_state")})).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn automatic_is_default_and_discovery_runs_every_five_minutes_per_account() {
        let c: AppConfig = serde_json::from_value(json!({"app_id":"cli_test","owner_id":"ou_test"})).unwrap();
        assert_eq!(c.group_sync_mode, GroupSyncMode::Automatic);
        let mut schedule = DiscoverySchedule::default();
        let start = Instant::now();
        assert!(schedule.due(&c, start, 1000));
        assert_eq!(schedule.since, 1000);
        // A failed discovery is not marked completed and cannot advance its history boundary.
        assert!(schedule.due(&c, start + Duration::from_secs(60), 1060));
        assert_eq!(schedule.since, 1000);
        schedule.completed(start + Duration::from_secs(60), 1060);
        assert!(!schedule.due(&c, start + Duration::from_secs(359), 1359));
        assert!(schedule.due(&c, start + Duration::from_secs(360), 1360));
        assert_eq!(schedule.since, 1060);
        let other = AppConfig { owner_id: "ou_other".into(), ..c };
        assert!(schedule.due(&other, start + Duration::from_secs(361), 1361));
        assert_eq!(schedule.since, 1361);
        assert_eq!(MESSAGE_INTERVAL, Duration::from_secs(15));
    }

    #[tokio::test]
    async fn discovery_preserves_manual_selection_times_and_disarms_removed_groups() {
        let pool = database().await;
        let mut c = config("ou_sender");
        configure_groups(&mut c, Some(GroupSyncMode::Selected), None, now()).unwrap();
        let selected = c.sync_groups.clone();
        configure_groups(&mut c, Some(GroupSyncMode::Automatic), Some(vec![]), now()).unwrap();
        assert_eq!(c.manual_sync_groups.as_ref().unwrap(), &selected);
        app::save(&pool, &c).await.unwrap();
        let cancel = CancellationToken::new();
        let since = now() - 100;
        let new_group = SyncGroup { id: "oc_new".into(), name: "新群".into(), since: 1 };
        let mut found = selected.clone(); found.push(new_group.clone());
        let updated = apply_discovered_groups(&pool, &c, found, since, &cancel).await.unwrap().unwrap();
        assert_eq!(updated.sync_groups[0].since, selected[0].since);
        assert_eq!(updated.sync_groups[1].since, since);
        assert_eq!(updated.manual_sync_groups.as_ref().unwrap(), &selected);
        sqlx::query("INSERT INTO lark_app_routes(app_id,owner_id,reference,reference_kind,chat_id,expires_at,local_record_bound_at) VALUES('cli_test','ou_sender','123456789','number','oc_test',9999999999,1)").execute(&pool).await.unwrap();
        let mut updated = apply_discovered_groups(&pool, &updated, vec![new_group], now(), &cancel).await.unwrap().unwrap();
        let bound: Option<i64> = sqlx::query_scalar("SELECT local_record_bound_at FROM lark_app_routes WHERE chat_id='oc_test'").fetch_one(&pool).await.unwrap();
        assert!(bound.is_none());
        assert_eq!(updated.sync_groups[0].since, since);
        // Returning to selected mode restores the explicit list, not all discovered groups.
        configure_groups(&mut updated, Some(GroupSyncMode::Selected), None, now()).unwrap();
        assert_eq!(updated.sync_groups.len(), 1);
        assert_eq!(updated.sync_groups[0].id, "oc_test");
    }

    #[tokio::test]
    async fn late_discovery_cannot_overwrite_account_policy_or_cancellation() {
        let pool = database().await;
        let c = config("ou_sender");
        let cancel = CancellationToken::new();
        for latest in [
            AppConfig { owner_id:"ou_other".into(), ..c.clone() },
            AppConfig { app_id:"cli_other".into(), ..c.clone() },
            AppConfig { secret:"changed".into(), ..c.clone() },
            AppConfig { enabled:false, ..c.clone() },
            AppConfig { manual_mode:true, ..c.clone() },
            AppConfig { group_sync_mode:GroupSyncMode::Selected, ..c.clone() },
        ] {
            app::save(&pool, &latest).await.unwrap();
            assert!(apply_discovered_groups(&pool, &c, vec![], now(), &cancel).await.unwrap().is_none());
            assert_eq!(app::load(&pool).await.unwrap().sync_groups, latest.sync_groups);
        }
        app::save(&pool, &c).await.unwrap();
        let invalid = SyncGroup { id:"bad_chat".into(), ..Default::default() };
        assert!(apply_discovered_groups(&pool, &c, vec![invalid], now(), &cancel).await.is_err());
        assert_eq!(app::load(&pool).await.unwrap().sync_groups, c.sync_groups);
        cancel.cancel();
        assert!(apply_discovered_groups(&pool, &c, vec![], now(), &cancel).await.unwrap().is_none());
        assert_eq!(app::load(&pool).await.unwrap().sync_groups, c.sync_groups);
    }
    struct Replies {
        bodies: Mutex<Vec<Value>>,
        fail: bool,
        pages: Mutex<std::collections::VecDeque<Value>>,
        reads: Mutex<Vec<String>>,
    }
    impl Replies {
        fn new(fail: bool) -> Self {
            Self {
                bodies: Mutex::new(Vec::new()),
                fail,
                pages: Mutex::new(Default::default()),
                reads: Mutex::new(Vec::new()),
            }
        }
    }
    impl SyncApi for Replies {
        async fn history(&self, path: &str, _: &str) -> Result<Value, String> {
            self.reads.lock().unwrap().push(path.into());
            self.pages
                .lock()
                .unwrap()
                .pop_front()
                .ok_or("测试页面不足".into())
        }
    }
    impl ReplyApi for Replies {
        async fn send(&self, _: &AppConfig, body: Value) -> Result<Value, String> {
            self.bodies.lock().unwrap().push(body);
            if self.fail {
                Err("超时".into())
            } else {
                Ok(json!({"message_id":"om_reply"}))
            }
        }
    }
    async fn database() -> SqlitePool {
        let pool = before_upgrade().await;
        sqlx::raw_sql(include_str!(
            "../../migrations/20261119010000_lark_group_sync.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        pool
    }
    async fn before_upgrade() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE meetings(id TEXT PRIMARY KEY);")
            .execute(&pool)
            .await
            .unwrap();
        for migration in [
            include_str!("../../migrations/20261015000000_add_lark_app.sql"),
            include_str!("../../migrations/20261015010000_add_lark_launches.sql"),
            include_str!("../../migrations/20261015020000_add_lark_auto_recording.sql"),
            include_str!("../../migrations/20261117000000_bound_meeting_recording.sql"),
            include_str!("../../migrations/20261119000000_lark_invitations.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await.unwrap();
        }
        pool
    }
    fn config(owner: &str) -> AppConfig {
        AppConfig {
            app_id: "cli_test".into(),
            owner_id: owner.into(),
            enabled: true,
            auto_record: true,
            auto_record_since: now() - 30,
            sync_groups: vec![SyncGroup {
                id: "oc_test".into(),
                name: "测试群".into(),
                since: now() - 30,
            }],
            ..Default::default()
        }
    }
    fn cmd(id: &str) -> Command {
        Command {
            id: id.into(),
            sender: "ou_sender".into(),
            time: now(),
            text: "会议ID:123 456 789".into(),
        }
    }

    #[test]
    fn only_recent_real_bot_mentions_from_users_are_commands() {
        let clock = now();
        let m = json!({"message_id":"om_1","msg_type":"text","create_time":(clock*1000).to_string(),"sender":{"id":"ou_sender","id_type":"open_id","sender_type":"user"},"mentions":[{"id":"ou_bot","id_type":"open_id","key":"@_user_1"}],"body":{"content":json!({"text":"@_user_1 会议ID:550 544 175"}).to_string()}});
        assert_eq!(
            command(&m, "ou_bot", clock - 1, clock).unwrap().text,
            "会议ID:550 544 175"
        );
        assert!(command(&m, "ou_other_bot", clock - 1, clock).is_none());
        assert!(command(&m, "ou_bot", clock + 1, clock).is_none());
        assert!(command(&m, "ou_bot", clock - 1000, clock + 601).is_none());
        let mut bot = m.clone();
        bot["sender"]["sender_type"] = json!("app");
        assert!(command(&bot, "ou_bot", clock - 1, clock).is_none());
        let mut deleted = m;
        deleted["deleted"] = json!(true);
        assert!(command(&deleted, "ou_bot", clock - 1, clock).is_none());
    }
    #[tokio::test]
    async fn separate_desktops_bind_the_same_command_but_only_sender_replies_once() {
        let a = database().await;
        let b = database().await;
        let ca = config("ou_sender");
        let cb = config("ou_colleague");
        app::save(&a, &ca).await.unwrap();
        app::save(&b, &cb).await.unwrap();
        let api = Replies::new(false);
        accept(&a, &ca, "oc_test", cmd("om_1"), &api).await.unwrap();
        accept(&b, &cb, "oc_test", cmd("om_1"), &api).await.unwrap();
        accept(&a, &ca, "oc_test", cmd("om_1"), &api).await.unwrap();
        assert_eq!(api.bodies.lock().unwrap().len(), 1);
        for (pool, owner) in [(&a, "ou_sender"), (&b, "ou_colleague")] {
            let row = sqlx::query("SELECT owner_id,local_record_bound_at FROM lark_app_routes")
                .fetch_one(pool)
                .await
                .unwrap();
            assert_eq!(row.get::<String, _>("owner_id"), owner);
            assert!(row.get::<Option<i64>, _>("local_record_bound_at").is_some());
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM lark_invitations")
                    .fetch_one(pool)
                    .await
                    .unwrap(),
                0
            );
        }
    }
    #[tokio::test]
    async fn uncertain_reply_is_not_retried_and_binding_survives() {
        let pool = database().await;
        let c = config("ou_sender");
        app::save(&pool, &c).await.unwrap();
        let api = Replies::new(true);
        assert!(accept(&pool, &c, "oc_test", cmd("om_1"), &api)
            .await
            .is_err());
        accept(&pool, &c, "oc_test", cmd("om_1"), &api)
            .await
            .unwrap();
        assert_eq!(api.bodies.lock().unwrap().len(), 1);
        assert_eq!(recent(&pool, &c).await.unwrap()[0]["ack_state"], "unknown");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM lark_app_routes")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
    }
    #[tokio::test]
    async fn changed_identity_removed_group_or_disabled_mode_cannot_bind_or_reply() {
        let pool = database().await;
        let c = config("ou_sender");
        let api = Replies::new(false);
        for latest in [
            config("ou_new"),
            AppConfig {
                sync_groups: vec![],
                ..c.clone()
            },
            AppConfig {
                manual_mode: true,
                ..c.clone()
            },
            AppConfig {
                enabled: false,
                ..c.clone()
            },
        ] {
            app::save(&pool, &latest).await.unwrap();
            accept(&pool, &c, "oc_test", cmd("om_1"), &api)
                .await
                .unwrap();
        }
        assert_eq!(api.bodies.lock().unwrap().len(), 0);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM lark_app_routes")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
    }
    #[tokio::test]
    async fn repost_preserves_active_instance_but_can_arm_next_instance_after_stop() {
        let pool = database().await;
        let c = config("ou_colleague");
        app::save(&pool, &c).await.unwrap();
        let api = Replies::new(false);
        accept(&pool, &c, "oc_test", cmd("om_1"), &api)
            .await
            .unwrap();
        sqlx::query("UPDATE lark_app_routes SET remote_id='instance1'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO lark_auto_recordings(app_id,owner_id,remote_id,status,updated_at) VALUES('cli_test','ou_colleague','instance1','recording',0)").execute(&pool).await.unwrap();
        accept(&pool, &c, "oc_test", cmd("om_2"), &api)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, Option<String>>("SELECT remote_id FROM lark_app_routes")
                .fetch_one(&pool)
                .await
                .unwrap(),
            Some("instance1".into())
        );
        sqlx::query("UPDATE lark_auto_recordings SET status='processing'")
            .execute(&pool)
            .await
            .unwrap();
        accept(&pool, &c, "oc_test", cmd("om_3"), &api)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, Option<String>>("SELECT remote_id FROM lark_app_routes")
                .fetch_one(&pool)
                .await
                .unwrap(),
            None
        );
    }
    #[tokio::test]
    async fn no_retroactive_microphone_opt_in_and_invalid_invites_never_bind() {
        let pool = database().await;
        let mut c = config("ou_colleague");
        c.auto_record_since = now() + 10;
        app::save(&pool, &c).await.unwrap();
        let api = Replies::new(false);
        accept(&pool, &c, "oc_test", cmd("om_1"), &api)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, Option<i64>>(
                "SELECT local_record_bound_at FROM lark_app_routes"
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            None
        );
        let mut invalid = cmd("om_bad");
        invalid.text = "会议ID:1234".into();
        accept(&pool, &c, "oc_test", invalid, &api).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM lark_app_routes")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
    }
    #[test]
    fn adding_groups_starts_now_and_repeated_save_does_not_replay_history() {
        let g = SyncGroup {
            id: "oc_test".into(),
            name: "测试".into(),
            since: 1,
        };
        let first = selected_groups(&[], vec![g.clone()], 100).unwrap();
        assert_eq!(first[0].since, 100);
        assert_eq!(
            selected_groups(&first, vec![g.clone()], 200).unwrap()[0].since,
            100
        );
        assert_eq!(selected_groups(&[], vec![g; 11], 100).unwrap().len(), 1);
        assert!(next_page(&json!({"has_more":true,"page_token":"old"}), "old").is_err());
    }
    #[test]
    fn more_than_ten_distinct_groups_can_be_saved_without_changing_existing_cursors() {
        let input: Vec<_> = (0..35)
            .map(|i| SyncGroup {
                id: format!("oc_{i}"),
                name: format!("会议群{i}"),
                since: 1,
            })
            .collect();
        let saved = selected_groups(&[], input.clone(), 100).unwrap();
        assert_eq!(saved.len(), 35);
        assert!(saved.iter().all(|g| g.since == 100));
        let mut edited = input;
        edited.remove(0);
        let again = selected_groups(&saved, edited, 200).unwrap();
        assert_eq!(again.len(), 34);
        assert!(again.iter().all(|g| g.since == 100));
    }
    #[tokio::test]
    async fn concurrent_history_reads_and_pages_share_one_paced_queue() {
        let pacer = ReadPacer::new();
        let request = || async {
            pacer.wait().await;
            tokio::time::Instant::now()
        };
        let (a, b, c) = tokio::join!(request(), request(), request());
        let mut observed = [a, b, c];
        observed.sort();
        assert!(observed[1].duration_since(observed[0]) >= Duration::from_millis(950));
        assert!(observed[2].duration_since(observed[1]) >= Duration::from_millis(950));
    }
    #[tokio::test]
    async fn pagination_resumes_after_budget_and_malformed_response_never_advances_cursor() {
        let pool = database().await;
        let c = config("ou_colleague");
        app::save(&pool, &c).await.unwrap();
        let api = Replies::new(false);
        for i in 1..=4 {
            api.pages
                .lock()
                .unwrap()
                .push_back(json!({"items":[],"has_more":true,"page_token":format!("page{i}")}));
        }
        assert!(sync_chat(&pool, &c, &c.sync_groups[0], "ou_bot", &api)
            .await
            .is_err());
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT page_token FROM lark_group_sync_cursors")
                .fetch_one(&pool)
                .await
                .unwrap(),
            "page4"
        );
        api.pages
            .lock()
            .unwrap()
            .push_back(json!({"has_more":false}));
        assert!(sync_chat(&pool, &c, &c.sync_groups[0], "ou_bot", &api)
            .await
            .is_err());
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT page_token FROM lark_group_sync_cursors")
                .fetch_one(&pool)
                .await
                .unwrap(),
            "page4"
        );
        api.pages
            .lock()
            .unwrap()
            .push_back(json!({"items":[],"has_more":false}));
        sync_chat(&pool, &c, &c.sync_groups[0], "ou_bot", &api)
            .await
            .unwrap();
        let reads = api.reads.lock().unwrap();
        assert!(reads[4].contains("page_token=page4"));
        assert_eq!(reads[4], reads[5]);
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT page_token FROM lark_group_sync_cursors")
                .fetch_one(&pool)
                .await
                .unwrap(),
            ""
        );
    }
    #[tokio::test]
    async fn history_sync_processes_peer_command_and_skips_deleted_or_other_bot_messages() {
        let pool = database().await;
        let c = config("ou_colleague");
        app::save(&pool, &c).await.unwrap();
        let api = Replies::new(false);
        let message = json!({"message_id":"om_1","msg_type":"text","create_time":((now()-10)*1000).to_string(),"chat_id":"oc_test","sender":{"id":"ou_sender","id_type":"open_id","sender_type":"user"},"mentions":[{"id":"ou_bot","id_type":"open_id","key":"@_user_1"}],"body":{"content":json!({"text":"@_user_1 https://vc-jp.larksuite.com/j/550544175"}).to_string()}});
        let mut other = message.clone();
        other["message_id"] = json!("om_other");
        other["mentions"][0]["id"] = json!("ou_else");
        let mut deleted = message.clone();
        deleted["message_id"] = json!("om_deleted");
        deleted["deleted"] = json!(true);
        api.pages
            .lock()
            .unwrap()
            .push_back(json!({"items":[message,other,deleted],"has_more":false}));
        sync_chat(&pool, &c, &c.sync_groups[0], "ou_bot", &api)
            .await
            .unwrap();
        assert_eq!(recent(&pool, &c).await.unwrap().len(), 1);
        assert!(api.bodies.lock().unwrap().is_empty());
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT reference FROM lark_app_routes")
                .fetch_one(&pool)
                .await
                .unwrap(),
            "550544175"
        );
    }
    #[tokio::test]
    async fn upgrade_cancels_old_schedules_without_deleting_completed_meeting_links() {
        let pool = before_upgrade().await;
        sqlx::raw_sql("INSERT INTO lark_invitations(id,app_id,owner_id,chat_id,source_key,invitation,topic,starts_at,ends_at,command_at,status) VALUES('i','a','u','c','s','{}','旧预约',1,2,1,'ready'); INSERT INTO lark_meeting_launches(request_id,app_id,owner_id,message_id,chat_id,topic,created_at,expires_at,status) VALUES('l','a','u','m','c','旧会议',1,9999999999,'sent'); INSERT INTO lark_app_routes(app_id,owner_id,reference,reference_kind,chat_id,expires_at,local_record_bound_at,remote_id) VALUES('a','u','1','number','c',9999999999,1,NULL),('a','u','2','number','c',9999999999,1,'instance');").execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!(
            "../../migrations/20261119010000_lark_group_sync.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM lark_invitations")
                .fetch_one(&pool)
                .await
                .unwrap(),
            "cancelled"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT expires_at FROM lark_meeting_launches")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        let rows = sqlx::query("SELECT remote_id,local_record_bound_at FROM lark_app_routes")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get::<String, _>("remote_id"), "instance");
        assert_eq!(rows[0].get::<Option<i64>, _>("local_record_bound_at"), None);
    }
}
