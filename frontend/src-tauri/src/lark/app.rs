use super::{
    card::{delivery_body, meeting_card},
    payload::{fingerprint, meeting_message_text},
    routing::{parse_binding, MeetingReference},
};
use crate::state::AppState;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};
use std::time::Duration;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub(super) const BASE: &str = "https://open.larksuite.com";
pub(crate) static CONFIG_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
static WORKER: Lazy<Mutex<Option<CancellationToken>>> = Lazy::new(|| Mutex::new(None));
static STATUS: Lazy<Mutex<String>> = Lazy::new(|| Mutex::new("未连接".into()));
pub(crate) static DELIVERY_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
static MATCH_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingSource {
    #[default]
    Cloud,
    Local,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct AppConfig {
    pub app_id: String,
    pub secret: String,
    pub enabled: bool,
    pub auto_send: bool,
    pub auto_record: bool,
    pub recording_source: RecordingSource,
    pub auto_record_since: i64,
    pub allow_group_launch: bool,
    pub owner_id: String,
    pub user_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
}

pub(super) fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
pub(super) fn db_error(_: sqlx::Error) -> String {
    "Lark 本地数据读写失败。".into()
}
pub(super) fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| "无法创建 Lark 网络连接。".into())
}

// Credentials stay out of the renderer, logs, source tree and plaintext DB backups.
#[cfg(windows)]
fn protect(value: &str, decrypt: bool) -> Result<String, String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
    };
    let mut bytes = if decrypt {
        STANDARD.decode(value).map_err(|_| "凭证格式异常。")?
    } else {
        value.as_bytes().to_vec()
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let success = unsafe {
        if decrypt {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if success == 0 {
        return Err("Windows 凭证加解密失败；请使用保存配置的 Windows 用户。".into());
    }
    let data =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData as *mut _);
    }
    if decrypt {
        String::from_utf8(data).map_err(|_| "凭证编码异常。".into())
    } else {
        Ok(STANDARD.encode(data))
    }
}
#[cfg(target_os = "macos")]
fn protect(value: &str, decrypt: bool) -> Result<String, String> {
    super::macos_credentials::protect(value, decrypt)
}
#[cfg(not(any(windows, target_os = "macos")))]
fn protect(_: &str, _: bool) -> Result<String, String> {
    Err("企业应用凭证保护目前仅支持 Windows 和 macOS。".into())
}

fn decode_config(raw: Option<String>) -> Result<AppConfig, String> {
    match raw {
        None => Ok(AppConfig::default()),
        Some(raw) => serde_json::from_str(&protect(&raw, true)?)
            .map_err(|_| "Lark 企业应用配置损坏。".into()),
    }
}

async fn stored_config(pool: &SqlitePool) -> Result<Option<String>, String> {
    sqlx::query_scalar("SELECT config_json FROM lark_app_settings WHERE id=1")
        .fetch_optional(pool).await.map_err(db_error)
}

// A credential decoding failure disables integration, not access to local meetings.
// Database read failures remain fatal; never replace the persisted credentials here.
pub(crate) async fn startup_config(pool: &SqlitePool) -> Result<(AppConfig, Option<String>), String> {
    let raw = stored_config(pool).await?;
    Ok(match decode_config(raw) {
        Ok(config) => (config, None),
        Err(error) => (AppConfig::default(), Some(format!(
            "Lark 配置无法读取：{error} 自动会议识别与发群暂不可用，本地会议仍可查看。请恢复有效的 Lark 配置备份；原配置已保留。"
        ))),
    })
}

pub(crate) async fn load(pool: &SqlitePool) -> Result<AppConfig, String> {
    decode_config(stored_config(pool).await?)
}
pub(super) async fn save(pool: &SqlitePool, config: &AppConfig) -> Result<(), String> {
    let raw = protect(
        &serde_json::to_string(config).map_err(|_| "无法编码配置。")?,
        false,
    )?;
    sqlx::query("INSERT INTO lark_app_settings(id,config_json) VALUES(1,?) ON CONFLICT(id) DO UPDATE SET config_json=excluded.config_json")
        .bind(raw).execute(pool).await.map_err(db_error)?;
    Ok(())
}
pub(super) async fn api(
    method: reqwest::Method,
    path: &str,
    token: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let mut request = client()?
        .request(method, format!("{BASE}{path}"))
        .bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .await
        .map_err(|_| "Lark 网络请求失败，请检查连接。")?;
    let status = response.status();
    let data: Value = response.json().await.map_err(|_| format!("Lark HTTP {}，响应无法解析。", status.as_u16()))?;
    if let Some(code) = data["code"].as_i64().filter(|code| *code != 0) {
        return Err(lark_api_error(code));
    }
    if !status.is_success() || data["code"].as_i64() != Some(0) {
        return Err(format!("Lark HTTP {}，请检查权限或连接。", status.as_u16()));
    }
    Ok(data["data"].clone())
}
fn lark_api_error(code: i64) -> String {
    match code {
        99991403 => "Lark 本月 API 调用额度已用完（99991403）。请联系管理员处理额度，恢复后重新预览并确认发送；不会自动补发。".into(),
        99991400 | 99991401 | 230020 => format!("Lark 调用过于频繁，已被限流（{code}）。请稍后重新确认发送；不会自动重试。"),
        _ => format!("Lark 拒绝请求，错误码 {code}。请检查应用权限、发布状态及配置。"),
    }
}

fn send_outcome(http: u16, body: Option<&Value>) -> (&'static str, String, Option<String>) {
    if let Some(v) = body {
        if let Some(code) = v["code"].as_i64().filter(|code| *code != 0) {
            return ("failed", lark_api_error(code), None);
        }
        if (200..300).contains(&http) && v["code"].as_i64() == Some(0) {
            if let Some(id) = v["data"]["message_id"].as_str().filter(|id| !id.is_empty()) {
                return ("sent", "发送成功".into(), Some(id.to_owned()));
            }
        }
    }
    if http == 429 { return ("failed", "Lark 请求被限流（HTTP 429）。请稍后重新确认发送；不会自动重试。".into(), None); }
    ("unknown", format!("发送结果不确定（HTTP {http}，未收到明确结果）。请核对群消息后重新确认；不会自动重试。"), None)
}

pub(super) async fn tenant_token(config: &AppConfig) -> Result<String, String> {
    let data: Value = client()?
        .post(format!(
            "{BASE}/open-apis/auth/v3/tenant_access_token/internal"
        ))
        .json(&json!({"app_id":config.app_id,"app_secret":config.secret}))
        .send()
        .await
        .map_err(|_| "Lark 应用认证网络失败。")?
        .json()
        .await
        .map_err(|_| "Lark 应用认证响应异常。")?;
    if data["code"].as_i64() != Some(0) {
        return Err(format!("Lark 应用认证失败，错误码 {}。", data["code"]));
    }
    data["tenant_access_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or("Lark 未返回应用凭证。".into())
}
pub(super) async fn status(text: impl Into<String>) {
    *STATUS.lock().await = text.into();
}

#[tauri::command]
pub async fn lark_app_status(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    let pool = state.db_manager.pool();
    let c = load(pool).await?;
    let routes = sqlx::query("SELECT reference,reference_kind,chat_id,remote_id,expires_at FROM lark_app_routes WHERE app_id=? AND owner_id=? AND expires_at>? ORDER BY expires_at DESC LIMIT 20")
        .bind(&c.app_id).bind(&c.owner_id).bind(now()).fetch_all(pool).await.map_err(db_error)?;
    let deliveries = sqlx::query("SELECT meeting_id,chat_id,status,detail,updated_at FROM lark_app_deliveries WHERE app_id=? ORDER BY updated_at DESC LIMIT 20")
        .bind(&c.app_id).fetch_all(pool).await.map_err(db_error)?;
    let recording = sqlx::query("SELECT remote_id,meeting_no,topic,blocked,detail,stopped FROM lark_recording_links WHERE app_id=? AND owner_id=? ORDER BY started_at DESC LIMIT 1")
        .bind(&c.app_id).bind(&c.owner_id).fetch_optional(pool).await.map_err(db_error)?;
    let launches = sqlx::query("SELECT request_id,chat_id,topic,status,detail,meeting_no,meeting_url FROM lark_meeting_launches WHERE app_id=? AND owner_id=? ORDER BY created_at DESC LIMIT 20")
        .bind(&c.app_id).bind(&c.owner_id).fetch_all(pool).await.map_err(db_error)?;
    let automation: Option<String> = sqlx::query_scalar("SELECT detail FROM lark_auto_recordings WHERE app_id=? AND owner_id=? ORDER BY updated_at DESC LIMIT 1")
        .bind(&c.app_id).bind(&c.owner_id).fetch_optional(pool).await.map_err(db_error)?;
    Ok(
        json!({"app_id":c.app_id,"has_secret":!c.secret.is_empty(),"enabled":c.enabled,"auto_send":c.auto_send,
            "auto_record":c.auto_record,"recording_source":c.recording_source,"automation":automation,"allow_group_launch":c.allow_group_launch,
            "authorized":super::oauth::authorization_status(&c).1,"authorization_status":super::oauth::authorization_status(&c).0,"owner_id":c.owner_id,"connection":STATUS.lock().await.clone(),
            "launches":launches.iter().map(|r|json!({"request_id":r.get::<String,_>("request_id"),"chat_id":r.get::<String,_>("chat_id"),"topic":r.get::<String,_>("topic"),"status":r.get::<String,_>("status"),"detail":r.get::<String,_>("detail"),"meeting_no":r.get::<Option<String>,_>("meeting_no"),"meeting_url":r.get::<Option<String>,_>("meeting_url")})).collect::<Vec<_>>(),
            "routes":routes.iter().map(|r| json!({"reference":r.get::<String,_>("reference"),"chat_id":r.get::<String,_>("chat_id"),"remote_id":r.get::<Option<String>,_>("remote_id")})).collect::<Vec<_>>(),
            "deliveries":deliveries.iter().map(|r| json!({"meeting_id":r.get::<String,_>("meeting_id"),"chat_id":r.get::<String,_>("chat_id"),"status":r.get::<String,_>("status"),"detail":r.get::<String,_>("detail")})).collect::<Vec<_>>(),
            "recording":recording.map(|r| json!({"remote_id":r.get::<Option<String>,_>("remote_id"),"meeting_no":r.get::<Option<String>,_>("meeting_no"),"topic":r.get::<Option<String>,_>("topic"),"blocked":r.get::<i64,_>("blocked")!=0,"detail":r.get::<String,_>("detail"),"stopped":r.get::<i64,_>("stopped")!=0}))
        }),
    )
}

#[tauri::command]
pub async fn lark_app_save(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    app_id: String,
    secret: String,
    enabled: bool,
    auto_send: bool,
    auto_record: Option<bool>,
    allow_group_launch: Option<bool>,
    recording_source: Option<RecordingSource>,
) -> Result<(), String> {
    let _policy = super::auto_recording::POLICY_LOCK.lock().await;
    let _lock = CONFIG_LOCK.lock().await;
    let _send = DELIVERY_LOCK.lock().await;
    let pool = state.db_manager.pool();
    let mut c = load(pool).await?;
    let id = app_id.trim();
    if !id.starts_with("cli_")
        || id.len() > 80
        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err("请输入有效 App ID。".into());
    }
    if c.app_id != id {
        c = AppConfig::default();
    }
    c.app_id = id.into();
    if !secret.trim().is_empty() {
        c.secret = secret.trim().into();
    }
    if c.secret.is_empty() || c.secret.len() > 512 {
        return Err("请输入有效 App Secret。".into());
    }
    c.enabled = enabled;
    c.auto_send = auto_send;
    if let Some(source) = recording_source { c.recording_source = source; }
    if let Some(value) = allow_group_launch {
        c.allow_group_launch = value;
    }
    if let Some(value) = auto_record {
        if value && !c.auto_record {
            c.auto_record_since = now();
        }
        c.auto_record = value;
    }
    crate::company::bind_identity(&app, id, None)?;
    save(pool, &c).await?;
    restart(pool.clone()).await;
    Ok(())
}

pub async fn restart(pool: SqlitePool) {
    let mut worker = WORKER.lock().await;
    if let Some(old) = worker.take() {
        old.cancel();
    }
    if let Err(e) = super::launches::recover(&pool).await {
        status(e).await;
        return;
    }
    let _ = sqlx::query("UPDATE lark_recording_links SET blocked=1,detail='应用或连接配置已重启，本次录音不再自动发群。' WHERE stopped=0")
        .execute(&pool).await;
    let Ok(c) = load(&pool).await else {
        status("无法读取企业应用配置").await;
        return;
    };
    if !c.enabled {
        status("已停用").await;
        return;
    }
    let cancel = CancellationToken::new();
    *worker = Some(cancel.clone());
    tokio::spawn(async move {
        super::transport::run(pool, c, cancel).await;
    });
}

// Called only after successful local audio start. A folder uniquely identifies the recording.
pub async fn recording_started(pool: &SqlitePool, folder: &str) -> Result<(), String> {
    let _tracking = MATCH_LOCK.lock().await;
    let c = load(pool).await?;
    if !c.enabled {
        return Ok(());
    }
    sqlx::query("INSERT OR IGNORE INTO lark_recording_links(folder_path,app_id,owner_id,started_at,auto_send,detail) VALUES(?,?,?,?,?,?)")
        .bind(folder).bind(&c.app_id).bind(&c.owner_id).bind(now()).bind(c.auto_send)
        .bind("等待识别当前会议；请先完成 Lark 用户授权并加入会议。").execute(pool).await.map_err(db_error)?;
    // Capture boundaries must use a fresh observation even when a manual/native
    // meeting has no subscribed event; never attach audio using stale routing.
    super::meeting_state::invalidate();
    if let Err(e) = match_current_inner(pool).await {
        sqlx::query("UPDATE lark_recording_links SET blocked=1,detail=? WHERE folder_path=?")
            .bind(e)
            .bind(folder)
            .execute(pool)
            .await
            .map_err(db_error)?;
    }
    sqlx::query("UPDATE lark_recording_links SET blocked=1,detail='录音开始时未能识别唯一会议；请先登录授权、加入 Lark 会议，再开始新录音。' WHERE folder_path=? AND remote_id IS NULL AND blocked=0")
        .bind(folder).execute(pool).await.map_err(db_error)?;
    // Persist the folder before recording-started is emitted, including a fast manual stop.
    sqlx::query("UPDATE lark_auto_recordings SET folder_path=?,updated_at=? WHERE status='starting' AND (app_id,owner_id,remote_id) IN (SELECT app_id,owner_id,remote_id FROM lark_recording_links WHERE folder_path=?)")
        .bind(folder).bind(now()).bind(folder).execute(pool).await.map_err(db_error)?;
    // Manual recording takes precedence: never restart it automatically after the user stops.
    sqlx::query("INSERT OR IGNORE INTO lark_auto_recordings(app_id,owner_id,remote_id,folder_path,status,detail,postprocessed,updated_at) SELECT app_id,owner_id,remote_id,folder_path,'manual','本场已手动录音，不再自动启动。',1,? FROM lark_recording_links WHERE folder_path=? AND remote_id IS NOT NULL")
        .bind(now()).bind(folder).execute(pool).await.map_err(db_error)?;
    Ok(())
}
pub async fn recording_stopped(pool: &SqlitePool, folder: &str) -> Result<(), String> {
    let _tracking = MATCH_LOCK.lock().await;
    let tracked: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM lark_recording_links WHERE folder_path=? AND stopped=0 AND blocked=0 AND remote_id IS NOT NULL")
        .bind(folder).fetch_one(pool).await.map_err(db_error)?;
    if tracked > 0 {
        super::meeting_state::invalidate();
        if let Err(e) = match_current_inner(pool).await {
            sqlx::query("UPDATE lark_recording_links SET blocked=1,detail=? WHERE folder_path=?")
                .bind(e)
                .bind(folder)
                .execute(pool)
                .await
                .map_err(db_error)?;
        }
    }
    sqlx::query("UPDATE lark_recording_links SET stopped=1 WHERE folder_path=?")
        .bind(folder)
        .execute(pool)
        .await
        .map_err(db_error)?;
    Ok(())
}

pub(super) async fn match_current(pool: &SqlitePool) -> Result<(), String> {
    let _tracking = MATCH_LOCK.lock().await;
    let result = match_current_inner(pool).await;
    if let Err(e) = &result {
        // Keep failure handling inside the same lock as start/stop and polling.
        sqlx::query("UPDATE lark_recording_links SET blocked=1,detail=? WHERE stopped=0 AND blocked=0 AND remote_id IS NOT NULL")
            .bind(e).execute(pool).await.map_err(db_error)?;
    }
    result
}

async fn match_current_inner(pool: &SqlitePool) -> Result<(), String> {
    let active_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM lark_recording_links WHERE stopped=0 AND blocked=0",
    )
    .fetch_one(pool)
    .await
    .map_err(db_error)?;
    if active_count == 0 {
        return Ok(());
    }
    let (c, active, _) = super::meeting_state::current(pool).await?;
    let parsed: Result<_, String> = Ok(active);
    if let Ok(Some((id, no, _))) = &parsed {
        super::launches::resolve_current(pool, &c, id, no).await?;
    }
    let _routing = DELIVERY_LOCK.lock().await;
    let rows = sqlx::query("SELECT folder_path,remote_id FROM lark_recording_links WHERE stopped=0 AND blocked=0 AND app_id=? AND owner_id=?")
        .bind(&c.app_id).bind(&c.owner_id).fetch_all(pool).await.map_err(db_error)?;
    for row in rows {
        let folder: String = row.get("folder_path");
        let previous: Option<String> = row.get("remote_id");
        match &parsed {
            Ok(Some((id, no, topic))) => {
                if previous.as_ref().is_some_and(|p| p != id) {
                    sqlx::query("UPDATE lark_recording_links SET blocked=1,detail='录音跨越多个 Lark 会议，禁止自动发送。' WHERE folder_path=?")
                        .bind(folder).execute(pool).await.map_err(db_error)?;
                } else {
                    sqlx::query("UPDATE lark_recording_links SET remote_id=?,meeting_no=?,topic=?,detail='已匹配当前会议，停止录音并生成纪要后发送。' WHERE folder_path=? AND stopped=0 AND blocked=0")
                        .bind(id).bind(no).bind(topic).bind(folder).execute(pool).await.map_err(db_error)?;
                    // Consume a meeting-number binding for this concrete meeting instance.
                    sqlx::query("UPDATE lark_app_routes SET remote_id=? WHERE app_id=? AND owner_id=? AND reference_kind='number' AND reference=? AND remote_id IS NULL AND expires_at>?")
                        .bind(id).bind(&c.app_id).bind(&c.owner_id).bind(no).bind(now()).execute(pool).await.map_err(db_error)?;
                }
            }
            Ok(None) => {
                if previous.is_some() {
                    // User left, or API no longer sees the meeting: do not silently append another meeting.
                    sqlx::query("UPDATE lark_recording_links SET detail='当前已不在会议中，请停止 Meetily 录音。' WHERE folder_path=? AND stopped=0 AND blocked=0")
                        .bind(folder).execute(pool).await.map_err(db_error)?;
                }
            }
            Err(message) => {
                sqlx::query(
                    "UPDATE lark_recording_links SET blocked=1,detail=? WHERE folder_path=?",
                )
                .bind(message)
                .bind(folder)
                .execute(pool)
                .await
                .map_err(db_error)?;
            }
        }
    }
    parsed.map(|_| ())
}

pub(super) async fn receive(
    pool: &SqlitePool,
    config: &AppConfig,
    value: &Value,
) -> Result<(), String> {
    if value["header"]["app_id"].as_str() != Some(config.app_id.as_str()) {
        return Ok(());
    }
    if super::meeting_state::supported(value["header"]["event_type"].as_str().unwrap_or("")) {
        let current = load(pool).await?;
        return super::meeting_state::receive(pool, &current, value).await;
    }
    if value["header"]["event_type"] != "im.message.receive_v1" {
        return Ok(());
    }
    let _routing = DELIVERY_LOCK.lock().await;
    let current = load(pool).await?;
    if !current.enabled || current.app_id != config.app_id || current.owner_id.is_empty() {
        return Ok(());
    }
    let event = &value["event"];
    let sender = event["sender"]["sender_id"]["open_id"].as_str().unwrap_or("");
    let is_owner = sender == current.owner_id;
    let message = &event["message"];
    if message["chat_type"] != "group"
        || message["message_type"] != "text"
        || message["mentions"].as_array().is_none_or(|a| a.is_empty())
    {
        return Ok(());
    }
    let timestamp_ms = message["create_time"]
        .as_str()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let timestamp = timestamp_ms / 1000;
    if timestamp < now() - 600 || timestamp > now() + 60 {
        return Ok(());
    }
    let content: Value =
        serde_json::from_str(message["content"].as_str().unwrap_or("")).unwrap_or(Value::Null);
    let text = content["text"].as_str().unwrap_or("");
    let launch_topic = super::launches::parse_command(text);
    let end_command = super::ending::parse_command(text);
    let binding = parse_binding(text);
    if launch_topic.is_none() && binding.is_none() && end_command.is_none() {
        return Ok(());
    }
    // Group members may create a new meeting, but cannot reroute an existing meeting's notes.
    if !is_owner && !(current.allow_group_launch
        && (launch_topic.is_some() || end_command.is_some())
        && !sender.is_empty()
        && event["sender"]["sender_type"].as_str() == Some("user")) {
        return Ok(());
    }
    let chat = message["chat_id"]
        .as_str()
        .filter(|s| s.starts_with("oc_"))
        .ok_or("群 ID 无效。")?;
    let event_id = value["header"]["event_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("事件 ID 缺失。")?;
    let mut tx = pool.begin().await.map_err(db_error)?;
    let inserted =
        sqlx::query("INSERT OR IGNORE INTO lark_app_events(event_id,received_at) VALUES(?,?)")
            .bind(event_id)
            .bind(now())
            .execute(&mut *tx)
            .await
            .map_err(db_error)?
            .rows_affected();
    if inserted == 1 {
        if let Some(topic) = launch_topic {
            let message_id = message["message_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("消息 ID 缺失。")?;
            let request_id = fingerprint(&[&current.app_id, message_id]);
            // Persist before acknowledging the WS event. Network mutations run outside this handler.
            sqlx::query("INSERT OR IGNORE INTO lark_meeting_launches(request_id,app_id,owner_id,message_id,chat_id,topic,created_at,expires_at,requester_id,command_at) VALUES(?,?,?,?,?,?,?,?,?,?)")
                .bind(request_id).bind(&current.app_id).bind(&current.owner_id).bind(message_id).bind(chat).bind(topic).bind(now()).bind(now()+21600)
                .bind(sender).bind(timestamp_ms)
                .execute(&mut *tx).await.map_err(db_error)?;
        } else if let Some(command) = end_command {
            let message_id = message["message_id"].as_str().filter(|s| !s.is_empty()).ok_or("消息 ID 缺失。")?;
            super::ending::enqueue(&mut tx, &current, sender, chat, message_id, timestamp_ms, &command).await?;
        } else if let Some(reference) = binding {
            let (kind, reference, remote) = match reference {
                MeetingReference::Id(id) => ("id", id.clone(), Some(id)),
                MeetingReference::Number(no) => ("number", no, None),
            };
            sqlx::query("INSERT INTO lark_app_routes(app_id,owner_id,reference,reference_kind,chat_id,remote_id,expires_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(app_id,owner_id,reference,reference_kind,chat_id) DO UPDATE SET remote_id=excluded.remote_id,expires_at=excluded.expires_at")
            .bind(&current.app_id).bind(&current.owner_id).bind(reference).bind(kind).bind(chat).bind(remote).bind(now()+21600)
            .execute(&mut *tx).await.map_err(db_error)?;
        }
    }
    tx.commit().await.map_err(db_error)?;
    Ok(())
}

// Runs from the backend after successful generation; independent of the details page lifecycle.
pub async fn summary_completed(pool: &SqlitePool, meeting_id: &str) -> Result<(), String> {
    let _guard = DELIVERY_LOCK.lock().await;
    prepare_summary_draft(pool, meeting_id).await
}

async fn prepare_summary_draft(pool: &SqlitePool, meeting_id: &str) -> Result<(), String> {
    let c = load(pool).await?;
    if !c.enabled || !c.auto_send {
        return Ok(());
    }
    crate::audio::transcription::completion::ensure_meeting_complete(pool, meeting_id).await?;
    let row = sqlx::query("SELECT m.title,l.topic,l.remote_id,l.meeting_no,l.owner_id,l.started_at,s.result,j.meeting_id AS queued_id,j.target_chat_id FROM meetings m LEFT JOIN recording_jobs j ON j.meeting_id=m.id JOIN lark_recording_links l ON m.folder_path=l.folder_path JOIN summary_processes s ON s.meeting_id=m.id WHERE m.id=? AND l.app_id=? AND l.stopped=1 AND l.blocked=0 AND l.auto_send=1 AND l.owner_id=? AND l.remote_id IS NOT NULL AND LOWER(s.status)='completed'")
        .bind(meeting_id).bind(&c.app_id).bind(&c.owner_id).fetch_optional(pool).await.map_err(db_error)?;
    let Some(row) = row else {
        return Ok(());
    };
    let remote: String = row.get("remote_id");
    let no: Option<String> = row.get("meeting_no");
    let mut chats: Vec<String> = if row.get::<Option<String>,_>("queued_id").is_some() {
        row.get::<Option<String>,_>("target_chat_id").into_iter().collect()
    } else { sqlx::query_scalar("SELECT DISTINCT chat_id FROM lark_app_routes WHERE app_id=? AND owner_id=? AND expires_at>? AND (remote_id=? OR (remote_id IS NULL AND reference_kind='number' AND reference=?))")
        .bind(&c.app_id).bind(&c.owner_id).bind(now()).bind(&remote).bind(&no).fetch_all(pool).await.map_err(db_error)? };
    // A review can happen after the original six-hour route expires. Keep its frozen destination.
    if chats.is_empty() && row.get::<Option<String>,_>("queued_id").is_none() {
        chats = sqlx::query_scalar("SELECT chat_id FROM lark_app_deliveries WHERE meeting_id=? AND app_id=? AND remote_id=?")
            .bind(meeting_id).bind(&c.app_id).bind(&remote).fetch_all(pool).await.map_err(db_error)?;
    }
    if chats.len() != 1 {
        return Err(if chats.is_empty() {
            "纪要已生成，但未找到对应群。机器人创建的会议需先入会再录音；原生会议可在目标群发送绑定指令后点击检查并发送。"
        } else {
            "同一会议绑定了多个群，已暂停自动发送。请在设置中清除多余绑定。"
        }
        .into());
    }
    let result: Value =
        serde_json::from_str(&row.get::<String, _>("result")).map_err(|_| "纪要格式异常。")?;
    sqlx::query("UPDATE lark_app_routes SET remote_id=? WHERE app_id=? AND owner_id=? AND reference_kind='number' AND reference=? AND remote_id IS NULL AND chat_id=?")
        .bind(&remote).bind(&c.app_id).bind(&c.owner_id).bind(row.get::<Option<String>,_>("meeting_no")).bind(&chats[0])
        .execute(pool).await.map_err(db_error)?;
    let detailed = crate::summary::markdown_export::preview(pool, meeting_id).await?
        .and_then(|v| v["content"].as_str().map(str::to_owned))
        .unwrap_or(super::routing::summary_text(&result)?);
    let summary = format!("{}\n\n完整纪要保存在 Meetily，可按需附送 Markdown 文件。", super::card::summary_overview(&detailed));
    let meeting_url: Option<String> = sqlx::query_scalar("SELECT meeting_url FROM lark_meeting_launches WHERE app_id=? AND owner_id=? AND chat_id=? AND meeting_no=? AND meeting_url IS NOT NULL ORDER BY created_at DESC LIMIT 1")
        .bind(&c.app_id).bind(&c.owner_id).bind(&chats[0]).bind(&no)
        .fetch_optional(pool).await.map_err(db_error)?;
    let original_topic: Option<String> = row.get("topic");
    let original_title: String = row.get("title");
    let started_at: i64 = row.get("started_at");
    let base_card = meeting_card(original_topic.as_deref(), no.as_deref(), started_at,
        meeting_url.as_deref(), &original_title, &summary)?;
    let base_card_text = base_card.to_string();
    let source_revision = fingerprint(&[&detailed, &base_card_text, &chats[0], &c.owner_id]);
    let previous = sqlx::query("SELECT draft_title,draft_summary,source_revision,message_edited FROM lark_app_deliveries WHERE meeting_id=? AND app_id=? AND remote_id=?")
        .bind(meeting_id).bind(&c.app_id).bind(&remote).fetch_optional(pool).await.map_err(db_error)?;
    let edited = previous.as_ref().is_some_and(|r| r.get::<i64,_>("message_edited") == 1
        && r.get::<Option<String>,_>("source_revision").as_deref() == Some(&source_revision));
    let (draft_title, draft_summary, card) = if edited {
        let previous = previous.as_ref().unwrap();
        let title: String = previous.get("draft_title");
        let body: String = previous.get("draft_summary");
        super::card::validate_message_edit(&title, &body)?;
        let card = meeting_card(Some(&title), no.as_deref(), started_at, meeting_url.as_deref(), "", &body)?;
        (title, body, card.to_string())
    } else {
        (base_card["header"]["title"]["content"].as_str().unwrap_or("会议纪要").trim_start_matches("会议纪要｜").to_owned(), summary, base_card_text)
    };
    let text = meeting_message_text(Some(&draft_title), no.as_deref(), started_at,
        meeting_url.as_deref(), if edited { "" } else { &original_title }, &draft_summary)?;
    let uuid = uuid::Uuid::new_v4().simple().to_string();
    sqlx::query("INSERT INTO lark_app_deliveries(meeting_id,app_id,remote_id,chat_id,message_uuid,message_text,message_type,message_content,status,updated_at,detail,draft_title,draft_summary,source_revision,message_edited) VALUES(?,?,?,?,?,?,'interactive',?,'awaiting_confirmation',?,'纪要待审核，请预览并确认发送',?,?,?,?) ON CONFLICT(meeting_id) DO UPDATE SET chat_id=excluded.chat_id,message_text=excluded.message_text,message_content=excluded.message_content,message_type=excluded.message_type,status='awaiting_confirmation',detail=excluded.detail,updated_at=excluded.updated_at,draft_title=excluded.draft_title,draft_summary=excluded.draft_summary,source_revision=excluded.source_revision,message_edited=excluded.message_edited WHERE lark_app_deliveries.status='awaiting_confirmation' AND lark_app_deliveries.app_id=excluded.app_id AND lark_app_deliveries.remote_id=excluded.remote_id")
        .bind(meeting_id).bind(&c.app_id).bind(&remote).bind(&chats[0]).bind(&uuid[..32]).bind(&text).bind(&card).bind(now()).bind(&draft_title).bind(&draft_summary).bind(&source_revision).bind(edited as i64).execute(pool).await.map_err(db_error)?.rows_affected();
    Ok(()) // Generation and queue recovery only prepare a draft; neither sends messages.
}

async fn reviewed_draft(pool: &SqlitePool, meeting_id: &str) -> Result<Value, String> {
    let c = load(pool).await?;
    if !c.enabled || !c.auto_send { return Err("请先在设置中启用企业应用连接和纪要审核发群。".into()); }
    prepare_summary_draft(pool, meeting_id).await?;
    let row = sqlx::query("SELECT chat_id,message_text,message_content,status,remote_id,message_uuid,last_sent_at,requires_duplicate_ack,draft_title,draft_summary,message_edited FROM lark_app_deliveries WHERE meeting_id=? AND app_id=?")
        .bind(meeting_id).bind(&c.app_id).fetch_optional(pool).await.map_err(db_error)?.ok_or("未找到可审核纪要，请先完成录音、生成纪要并匹配唯一群。")?;
    match row.get::<String,_>("status").as_str() {
        "awaiting_confirmation" => (),
        "sent" => return Err("本场纪要已经发送，请重新打开预览后确认再次发送。".into()),
        _ => return Err("已有发送尝试，请先核对群消息和发送状态；不会重复发送。".into()),
    }
    // Revalidate eligibility even if a draft predates regeneration, a blocked link or account changes.
    let eligible: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meetings m JOIN lark_recording_links l ON l.folder_path=m.folder_path JOIN summary_processes s ON s.meeting_id=m.id WHERE m.id=? AND l.app_id=? AND l.owner_id=? AND l.stopped=1 AND l.blocked=0 AND l.auto_send=1 AND LOWER(s.status)='completed'")
        .bind(meeting_id).bind(&c.app_id).bind(&c.owner_id).fetch_one(pool).await.map_err(db_error)?;
    if eligible != 1 { return Err("会议或纪要状态已变化，请完成生成并重新检查绑定。".into()); }
    let chat: String = row.get("chat_id");
    let text: String = row.get("message_text");
    let content: String = row.get("message_content");
    let markdown = crate::summary::markdown_export::preview(pool, meeting_id).await?;
    let markdown_revision = markdown.as_ref().map(Value::to_string).unwrap_or_default();
    let revision = fingerprint(&[&c.app_id, &c.owner_id, meeting_id, &chat, &content, &markdown_revision, &row.get::<String,_>("message_uuid"), &row.get::<i64,_>("requires_duplicate_ack").to_string()]);
    let card: Value = serde_json::from_str(&content).map_err(|_| "纪要卡片格式损坏，请重新生成预览。")?;
    Ok(json!({"chat_id":chat,"text":text,"card":card,"message_title":row.get::<Option<String>,_>("draft_title"),"message_summary":row.get::<Option<String>,_>("draft_summary"),"message_edited":row.get::<i64,_>("message_edited")!=0,"revision":revision,"remote_id":row.get::<String,_>("remote_id"),"last_sent_at":row.get::<Option<i64>,_>("last_sent_at"),"requires_duplicate_ack":row.get::<i64,_>("requires_duplicate_ack")!=0,"markdown":markdown}))
}

#[tauri::command]
pub async fn lark_app_preview_summary(state: tauri::State<'_, AppState>, meeting_id: String) -> Result<Value, String> {
    let _storage = crate::audio::storage_migration::use_storage().await?;
    let _guard = DELIVERY_LOCK.lock().await;
    let pool = state.db_manager.pool();
    let mut draft = begin_review(pool, &meeting_id).await?;
    let c = load(pool).await?;
    let chat = draft["chat_id"].as_str().ok_or("目标群异常。")?;
    match resolve_chat_name(pool, &c, chat).await {
        Ok(name) => draft["chat_name"] = json!(name),
        Err(error) => draft["chat_name_error"] = json!(error),
    }
    if draft["markdown"].is_null() { return Err("请先保存 Markdown 文件，再打开发送预览。".into()); }
    Ok(draft)
}

// Editing only updates the local review draft. Existing approval/revision guards still apply.
async fn edit_message_draft(pool: &SqlitePool, meeting_id: &str, expected_revision: &str, title: &str, summary: &str) -> Result<Value, String> {
    super::card::validate_message_edit(title, summary)?;
    let draft = reviewed_draft(pool, meeting_id).await?;
    if draft["revision"].as_str() != Some(expected_revision) {
        return Err("纪要内容或目标群已变化，请重新打开消息编辑；修改尚未保存，未发送。".into());
    }
    let changed = sqlx::query("UPDATE lark_app_deliveries SET draft_title=?,draft_summary=?,message_edited=1 WHERE meeting_id=? AND status='awaiting_confirmation'")
        .bind(title.trim()).bind(summary.trim()).bind(meeting_id).execute(pool).await.map_err(db_error)?.rows_affected();
    if changed != 1 { return Err("消息状态已变化，请重新打开编辑。".into()); }
    reviewed_draft(pool, meeting_id).await
}

#[tauri::command]
pub async fn lark_app_edit_message(state: tauri::State<'_, AppState>, meeting_id: String,
    expected_revision: String, title: String, summary: String) -> Result<Value, String> {
    let _storage = crate::audio::storage_migration::use_storage().await?;
    let _guard = DELIVERY_LOCK.lock().await;
    edit_message_draft(state.db_manager.pool(), &meeting_id, &expected_revision, &title, &summary).await
}

// Only an explicit preview request reopens an attempt. DELIVERY_LOCK ensures no send is active.
// Interrupted/uncertain attempts require a separate duplicate-risk acknowledgment.
// A fresh UUID invalidates old approvals even if the summary text is unchanged.
async fn begin_review(pool: &SqlitePool, meeting_id: &str) -> Result<Value, String> {
    let c = load(pool).await?;
    sqlx::query("UPDATE lark_app_deliveries SET status='awaiting_confirmation',message_uuid=?,requires_duplicate_ack=CASE WHEN status IN ('unknown','sending','queued') OR (status='sent' AND attachment_status IN ('pending','sending','unknown')) THEN 1 ELSE 0 END,detail='请确认后再次发送' WHERE meeting_id=? AND app_id=? AND status IN ('sent','failed','unknown','sending','queued')")
        .bind(uuid::Uuid::new_v4().simple().to_string()).bind(meeting_id).bind(&c.app_id)
        .execute(pool).await.map_err(db_error)?;
    reviewed_draft(pool, meeting_id).await
}

async fn resolve_chat_name(pool: &SqlitePool, c: &AppConfig, chat: &str) -> Result<Option<String>, String> {
    if !chat.starts_with("oc_") || !chat.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err("目标群标识无效。".into());
    }
    let cached = sqlx::query("SELECT name,checked_at,error FROM lark_chat_names WHERE app_id=? AND chat_id=?")
        .bind(&c.app_id).bind(chat).fetch_optional(pool).await.map_err(db_error)?;
    if let Some(ref row) = cached {
        let name: Option<String> = row.get("name");
        let ttl = if name.is_some() { 3600 } else { 300 };
        if row.get::<i64,_>("checked_at") > now() - ttl {
            if let Some(error) = row.get::<Option<String>,_>("error") { return Err(error); }
            return Ok(name);
        }
    }
    let result = async {
        let token = tenant_token(c).await?;
        let data = api(reqwest::Method::GET, &format!("/open-apis/im/v1/chats/{chat}"), &token, None).await?;
        data["name"].as_str().filter(|name| !name.trim().is_empty()).map(str::to_owned)
            .ok_or_else(|| "Lark 未返回群名。".to_string())
    }.await;
    // Cache failed lookups too; local status polling never calls this API.
    let name = result.as_ref().ok().cloned();
    let error = result.as_ref().err().cloned();
    sqlx::query("INSERT INTO lark_chat_names(app_id,chat_id,name,checked_at,error) VALUES(?,?,?,?,?) ON CONFLICT(app_id,chat_id) DO UPDATE SET name=excluded.name,checked_at=excluded.checked_at,error=excluded.error")
        .bind(&c.app_id).bind(chat).bind(&name).bind(now()).bind(&error).execute(pool).await.map_err(db_error)?;
    result.map(Some)
}

async fn approve_draft(pool: &SqlitePool, meeting_id: &str, expected_revision: &str, acknowledge_duplicate_risk: bool, include_markdown: bool) -> Result<(AppConfig, Value), String> {
    let mut draft = reviewed_draft(pool, meeting_id).await?;
    if draft["revision"].as_str() != Some(expected_revision) {
        return Err("纪要内容或目标群已变化，请关闭预览并重新审核；本次没有发送。".into());
    }
    if draft["requires_duplicate_ack"].as_bool() == Some(true) && !acknowledge_duplicate_risk {
        return Err("上次发送可能已送达，请核对群消息并勾选可能重复发送的确认项；本次没有发送。".into());
    }
    if include_markdown && draft["markdown"].is_null() { return Err("请先保存 Markdown 文件，或取消勾选发送附件。".into()); }
    draft["include_markdown"] = json!(include_markdown);
    sqlx::query("UPDATE lark_app_deliveries SET status='queued',detail='已确认，准备发送',attachment_status=?,attachment_detail=? WHERE meeting_id=? AND status='awaiting_confirmation'")
        .bind(if include_markdown { "pending" } else { "skipped" })
        .bind(if include_markdown { "等待上传 Markdown" } else { "本次未勾选发送文件" })
        .bind(meeting_id).execute(pool).await.map_err(db_error)?;
    Ok((load(pool).await?, draft))
}

async fn deliver(pool: &SqlitePool, c: &AppConfig, meeting_id: &str, token: &str) -> Result<(), String> {
    let row = sqlx::query("SELECT chat_id,message_uuid,message_text,message_type,message_content FROM lark_app_deliveries WHERE meeting_id=? AND app_id=? AND status='queued'")
        .bind(meeting_id).bind(&c.app_id).fetch_optional(pool).await.map_err(db_error)?;
    let Some(row) = row else {
        return Ok(());
    };
    let body = match delivery_body(
        &row.get::<String,_>("chat_id"), &row.get::<String,_>("message_uuid"),
        &row.get::<String,_>("message_type"), row.get::<Option<String>,_>("message_content").as_deref(),
        &row.get::<String,_>("message_text"),
    ) {
        Ok(body) => body,
        Err(error) => {
            set_delivery(pool, meeting_id, "failed", &error, None).await?;
            return Err(error);
        }
    };
    set_delivery(pool, meeting_id, "sending", "正在发送", None).await?;
    let response = client()?.post(format!("{BASE}/open-apis/im/v1/messages?receive_id_type=chat_id"))
        .bearer_auth(token).json(&body).send().await;
    match response {
        Ok(response) => {
            let http = response.status().as_u16();
            let body = response.json::<Value>().await.ok();
            let (status, detail, message) = send_outcome(http, body.as_ref());
            set_delivery(pool, meeting_id, status, &detail, message.as_deref()).await
        }
        Err(error) => {
            let detail = if error.is_timeout() { "发送请求超时，结果不确定。请核对群消息后重新确认；不会自动重试。" }
                else { "发送连接失败，结果不确定。请核对群消息后重新确认；不会自动重试。" };
            set_delivery(pool, meeting_id, "unknown", detail, None).await
        }
    }
}

async fn set_attachment(pool: &SqlitePool, id: &str, status: &str, detail: &str) -> Result<(), String> {
    sqlx::query("UPDATE lark_app_deliveries SET attachment_status=?,attachment_detail=?,attachment_last_sent_at=CASE WHEN ?='sent' THEN ? ELSE attachment_last_sent_at END WHERE meeting_id=?")
        .bind(status).bind(detail).bind(status).bind(now()).bind(id).execute(pool).await.map_err(db_error)?;
    Ok(())
}

async fn upload_markdown(token: &str, markdown: &Value) -> Result<String, String> {
    let name = markdown["name"].as_str().ok_or("Markdown 文件名缺失。")?;
    let content = markdown["content"].as_str().ok_or("Markdown 文件内容缺失。")?;
    let part = reqwest::multipart::Part::bytes(content.as_bytes().to_vec())
        .file_name(name.to_owned()).mime_str("text/markdown; charset=utf-8").map_err(|_| "Markdown 附件格式无效。")?;
    let form = reqwest::multipart::Form::new().text("file_type", "stream").text("file_name", name.to_owned()).part("file", part);
    let response = client()?.post(format!("{BASE}/open-apis/im/v1/files")).bearer_auth(token).multipart(form).send().await
        .map_err(|_| "Markdown 上传连接失败，尚未开始发群。请稍后重新确认。")?;
    let http = response.status();
    let data: Value = response.json().await.map_err(|_| format!("Markdown 上传响应无法解析（HTTP {}），尚未开始发群。", http.as_u16()))?;
    if let Some(code) = data["code"].as_i64().filter(|code| *code != 0) { return Err(lark_api_error(code)); }
    if !http.is_success() { return Err(format!("Markdown 上传失败（HTTP {}），尚未开始发群。", http.as_u16())); }
    data["data"]["file_key"].as_str().filter(|key| !key.is_empty()).map(str::to_owned).ok_or("Lark 未返回附件标识，尚未开始发群。".into())
}

// Upload and both message mutations occur only after approve_draft, using the exact approved bytes.
async fn deliver_bundle(pool: &SqlitePool, c: &AppConfig, id: &str, draft: &Value) -> Result<(), String> {
    let include_markdown = draft["include_markdown"].as_bool().unwrap_or(false);
    let uploaded = async {
        let token = tenant_token(c).await?;
        let key = if include_markdown { Some(upload_markdown(&token, &draft["markdown"]).await?) } else { None };
        Ok::<_,String>((token, key))
    }.await;
    let (token, key) = match uploaded {
        Ok(value) => value,
        Err(error) => {
            if include_markdown { set_attachment(pool, id, "failed", &error).await?; }
            set_delivery(pool, id, "failed", &error, None).await?;
            return Err(error);
        }
    };
    deliver(pool, c, id, &token).await?;
    let row = sqlx::query("SELECT status,detail,chat_id,message_uuid FROM lark_app_deliveries WHERE meeting_id=?")
        .bind(id).fetch_one(pool).await.map_err(db_error)?;
    if row.get::<String,_>("status") != "sent" {
        if include_markdown { set_attachment(pool, id, "not_sent", "卡片未确认成功，未发送附件消息").await?; }
        return Err(row.get("detail"));
    }
    let Some(key) = key else { return Ok(()); };
    set_attachment(pool, id, "sending", "卡片已发送，正在发送 Markdown 附件").await?;
    let uuid = fingerprint(&[&row.get::<String,_>("message_uuid"), "markdown"]);
    let body = json!({"receive_id":row.get::<String,_>("chat_id"),"msg_type":"file","content":json!({"file_key":key}).to_string(),"uuid":&uuid[..32]});
    let response = client()?.post(format!("{BASE}/open-apis/im/v1/messages?receive_id_type=chat_id"))
        .bearer_auth(token).json(&body).send().await;
    let (status, detail, _) = match response {
        Ok(response) => {
            let http = response.status().as_u16();
            let body = response.json::<Value>().await.ok();
            send_outcome(http, body.as_ref())
        }
        Err(_) => ("unknown", "附件消息请求超时或连接失败，请核对群内文件。".into(), None),
    };
    set_attachment(pool, id, status, &detail).await?;
    if status != "sent" { return Err(format!("纪要卡片已发送，Markdown 附件未确认成功：{detail} 再次发送前请核对群消息。")); }
    Ok(())
}

async fn set_delivery(
    pool: &SqlitePool,
    id: &str,
    status: &str,
    detail: &str,
    message: Option<&str>,
) -> Result<(), String> {
    sqlx::query("UPDATE lark_app_deliveries SET status=?,detail=?,message_id=?,updated_at=?,last_sent_at=CASE WHEN ?='sent' THEN ? ELSE last_sent_at END WHERE meeting_id=?")
        .bind(status).bind(detail).bind(message).bind(now()).bind(status).bind(now()).bind(id).execute(pool).await.map_err(db_error)?;
    Ok(())
}

#[tauri::command]
pub async fn lark_app_check_send(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    expected_revision: String,
    acknowledge_duplicate_risk: Option<bool>,
    include_markdown: Option<bool>,
) -> Result<i64, String> {
    let _storage = crate::audio::storage_migration::use_storage().await?;
    let pool = state.db_manager.pool();
    let _guard = DELIVERY_LOCK.lock().await;
    let (c, draft) = approve_draft(pool, &meeting_id, &expected_revision, acknowledge_duplicate_risk.unwrap_or(false), include_markdown.unwrap_or(true)).await?;
    deliver_bundle(pool, &c, &meeting_id, &draft).await?;
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM lark_app_deliveries WHERE meeting_id=?")
            .bind(&meeting_id)
            .fetch_optional(pool)
            .await
            .map_err(db_error)?;
    match status.as_deref(){
        Some("sent")=>sqlx::query_scalar("SELECT last_sent_at FROM lark_app_deliveries WHERE meeting_id=?")
            .bind(&meeting_id).fetch_one(pool).await.map_err(db_error),
        Some("unknown"|"sending")=>Err("上次发送结果不确定，请核对群消息；没有再次发送。".into()),
        Some(_)=>Err(sqlx::query_scalar::<_,String>("SELECT detail FROM lark_app_deliveries WHERE meeting_id=?").bind(&meeting_id).fetch_one(pool).await.map_err(db_error)?),
        None=>Err("此录音尚不满足自动发送条件：需在启用自动发送后录制、匹配唯一会议、停止录音并生成纪要。".into()),
    }
}

#[tauri::command]
pub async fn lark_app_meeting_status(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<Value, String> {
    let pool = state.db_manager.pool();
    let c = load(pool).await?;
    let row=sqlx::query("SELECT l.remote_id,l.detail,l.blocked,d.status,d.chat_id,d.last_sent_at,d.attachment_status,d.attachment_detail,d.attachment_last_sent_at,n.name AS chat_name,d.detail AS delivery_detail FROM meetings m JOIN lark_recording_links l ON m.folder_path=l.folder_path LEFT JOIN lark_app_deliveries d ON d.meeting_id=m.id LEFT JOIN lark_chat_names n ON n.app_id=d.app_id AND n.chat_id=d.chat_id WHERE m.id=?")
        .bind(meeting_id).fetch_optional(pool).await.map_err(db_error)?;
    Ok(match row {
        Some(r) => {
            json!({"enabled":c.enabled,"linked":true,"remote_id":r.get::<Option<String>,_>("remote_id"),"detail":r.get::<String,_>("detail"),"blocked":r.get::<i64,_>("blocked")!=0,"status":r.get::<Option<String>,_>("status"),"chat_id":r.get::<Option<String>,_>("chat_id"),"delivery_detail":r.get::<Option<String>,_>("delivery_detail"),"last_sent_at":r.get::<Option<i64>,_>("last_sent_at"),"chat_name":r.get::<Option<String>,_>("chat_name"),"attachment_status":r.get::<Option<String>,_>("attachment_status"),"attachment_detail":r.get::<Option<String>,_>("attachment_detail"),"attachment_last_sent_at":r.get::<Option<i64>,_>("attachment_last_sent_at")})
        }
        None => json!({"enabled":c.enabled,"linked":false}),
    })
}
#[tauri::command]
pub async fn lark_app_remove_route(
    state: tauri::State<'_, AppState>,
    reference: String,
    chat_id: String,
) -> Result<(), String> {
    let _guard = DELIVERY_LOCK.lock().await;
    let c = load(state.db_manager.pool()).await?;
    sqlx::query(
        "DELETE FROM lark_app_routes WHERE app_id=? AND owner_id=? AND reference=? AND chat_id=?",
    )
    .bind(c.app_id)
    .bind(c.owner_id)
    .bind(reference)
    .bind(chat_id)
    .execute(state.db_manager.pool())
    .await
    .map_err(db_error)?;
    Ok(())
}

pub async fn report_summary_result(pool: &SqlitePool, id: &str) {
    if let Err(e) = summary_completed(pool, id).await {
        // Persist a user-visible reason on the recording, without logging transcript content.
        let _=sqlx::query("UPDATE lark_recording_links SET detail=? WHERE folder_path=(SELECT folder_path FROM meetings WHERE id=?)")
            .bind(&e).bind(id).execute(pool).await;
        status(e).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn invalid_credentials_do_not_block_local_startup_or_get_overwritten() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE lark_app_settings(id INTEGER PRIMARY KEY, config_json TEXT NOT NULL)")
            .execute(&pool).await.unwrap();
        sqlx::query("CREATE TABLE meetings(id TEXT PRIMARY KEY, title TEXT)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO meetings VALUES('history','历史会议')").execute(&pool).await.unwrap();
        let (empty, warning) = startup_config(&pool).await.unwrap();
        assert!(!empty.enabled && warning.is_none());
        let mut invalid = vec!["invalid\0credential".to_owned(), "YWJj".to_owned()];
        #[cfg(windows)]
        invalid.push(protect("not-json", false).unwrap());
        for raw in invalid {
            sqlx::query("INSERT OR REPLACE INTO lark_app_settings VALUES(1,?)").bind(&raw).execute(&pool).await.unwrap();
            let (config, warning) = startup_config(&pool).await.unwrap();
            assert!(warning.unwrap().contains("本地会议仍可查看"));
            assert!(!config.enabled && !config.auto_record && !config.auto_send);
            assert!(load(&pool).await.is_err());
            assert_eq!(sqlx::query_scalar::<_,String>("SELECT config_json FROM lark_app_settings WHERE id=1").fetch_one(&pool).await.unwrap(), raw);
            assert_eq!(sqlx::query_scalar::<_,String>("SELECT title FROM meetings WHERE id='history'").fetch_one(&pool).await.unwrap(), "历史会议");
        }
        #[cfg(windows)]
        {
            save(&pool, &AppConfig { app_id: "cli_existing".into(), enabled: true, ..Default::default() }).await.unwrap();
            let (config, warning) = startup_config(&pool).await.unwrap();
            assert!(config.enabled && warning.is_none());
            assert_eq!(config.app_id, "cli_existing");
        }
        pool.close().await;
    }

    #[tokio::test]
    async fn startup_does_not_hide_database_read_failures() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        assert!(startup_config(&pool).await.is_err());
        pool.close().await;
    }

    #[test]
    fn quota_errors_from_non_success_http_are_not_ambiguous() {
        let quota = json!({"code":99991403,"msg":"This month's API call quota has been exceeded"});
        let (status, detail, _) = send_outcome(400, Some(&quota));
        assert_eq!(status, "failed");
        assert!(detail.contains("本月 API 调用额度已用完"));
        assert_eq!(send_outcome(429, None).0, "failed");
        assert_eq!(send_outcome(502, None).0, "unknown");
        assert_eq!(send_outcome(200, Some(&json!({"code":0,"data":{}}))).0, "unknown");
        assert_eq!(send_outcome(200, Some(&json!({"code":0,"data":{"message_id":"test"}}))).0, "sent");
    }
    #[tokio::test]
    async fn markdown_preview_bytes_are_bound_to_approval() {
        let pool = review_fixture().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("会议纪要.md");
        std::fs::write(&path, "# 中文纪要\n决定使用 Lark\n").unwrap();
        sqlx::query("INSERT INTO summary_markdown_exports(meeting_id,path) VALUES('review',?)")
            .bind(path.to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        let draft = begin_review(&pool, "review").await.unwrap();
        assert_eq!(draft["markdown"]["name"], "会议纪要.md");
        assert_eq!(draft["markdown"]["content"], "# 中文纪要\n决定使用 Lark\n");
        std::fs::write(&path, "文件已修改").unwrap();
        assert!(approve_draft(&pool, "review", draft["revision"].as_str().unwrap(), false, true).await.is_err());
        let fresh = begin_review(&pool, "review").await.unwrap();
        let (_, approved) = approve_draft(&pool, "review", fresh["revision"].as_str().unwrap(), false, true).await.unwrap();
        assert_eq!(approved["markdown"]["content"], "文件已修改");
        set_delivery(&pool, "review", "sent", "卡片成功", Some("card-id")).await.unwrap();
        set_attachment(&pool, "review", "unknown", "附件结果不确定").await.unwrap();
        let retry = begin_review(&pool, "review").await.unwrap();
        assert_eq!(retry["requires_duplicate_ack"], true);
        assert!(approve_draft(&pool, "review", retry["revision"].as_str().unwrap(), false, true).await.is_err());
        std::fs::remove_file(path).unwrap();
        assert!(begin_review(&pool, "review").await.is_err());
    }
    async fn review_fixture() -> SqlitePool {
        let pool = database().await;
        let c = load(&pool).await.unwrap();
        receive(&pool, &c, &event("review", "ou_owner", "oc_review", now())).await.unwrap();
        sqlx::raw_sql("INSERT INTO meetings VALUES('review','中文会议','2026-10-19','review-folder'); INSERT INTO summary_processes VALUES('review','completed','{\"markdown\":\"第一版纪要\"}'); INSERT INTO lark_recording_links(folder_path,app_id,owner_id,remote_id,meeting_no,started_at,stopped,auto_send) VALUES('review-folder','cli_test','ou_owner','1234567890123','123456789',1,1,1);")
            .execute(&pool).await.unwrap();
        pool
    }
    #[tokio::test]
    async fn message_edits_persist_without_changing_minutes_and_invalidate_old_confirmation() {
        let pool = review_fixture().await;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("会议纪要.md");
        let detailed = "# 完整纪要\n\n| 事项 | 负责人 |\n| --- | --- |\n| 验收 | 小明 |\n";
        std::fs::write(&file, detailed).unwrap();
        sqlx::query("INSERT INTO summary_markdown_exports(meeting_id,path) VALUES('review',?)")
            .bind(file.to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        let original_summary: String = sqlx::query_scalar("SELECT result FROM summary_processes WHERE meeting_id='review'").fetch_one(&pool).await.unwrap();
        let first = reviewed_draft(&pool, "review").await.unwrap();
        let edited = edit_message_draft(&pool, "review", first["revision"].as_str().unwrap(), "发布评审结论", "## 概要\n- 周五发布。\n- 小明负责验收。").await.unwrap();
        assert_eq!(edited["message_edited"], true);
        assert_eq!(edited["card"]["header"]["title"]["content"], "会议纪要｜发布评审结论");
        assert!(edited["card"].to_string().contains("小明负责验收"));
        assert_eq!(edited["chat_id"], first["chat_id"]);
        assert_eq!(edited["markdown"], first["markdown"]);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), detailed);
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT result FROM summary_processes WHERE meeting_id='review'").fetch_one(&pool).await.unwrap(), original_summary);
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT status FROM lark_app_deliveries WHERE meeting_id='review'").fetch_one(&pool).await.unwrap(), "awaiting_confirmation");
        assert!(approve_draft(&pool, "review", first["revision"].as_str().unwrap(), false, false).await.is_err());
        assert!(edit_message_draft(&pool, "review", first["revision"].as_str().unwrap(), "旧窗口", "不应覆盖").await.is_err());
        let reopened = begin_review(&pool, "review").await.unwrap();
        assert_eq!(reopened["message_summary"], edited["message_summary"]);
        let (_, approved) = approve_draft(&pool, "review", reopened["revision"].as_str().unwrap(), false, false).await.unwrap();
        let stored: String = sqlx::query_scalar("SELECT message_content FROM lark_app_deliveries WHERE meeting_id='review'").fetch_one(&pool).await.unwrap();
        assert_eq!(approved["card"], serde_json::from_str::<Value>(&stored).unwrap());
    }

    #[tokio::test]
    async fn source_changes_reset_message_edits_and_table_input_is_rejected() {
        let pool = review_fixture().await;
        let first = reviewed_draft(&pool, "review").await.unwrap();
        for (title, body) in [("", "概要"), ("标题", ""), ("标题", "事项 | 负责人\n--- | ---\n验收 | 小明")] {
            assert!(edit_message_draft(&pool, "review", first["revision"].as_str().unwrap(), title, body).await.is_err());
        }
        let edited = edit_message_draft(&pool, "review", first["revision"].as_str().unwrap(), "旧标题", "旧的概要").await.unwrap();
        sqlx::query("UPDATE summary_processes SET result=?").bind(json!({"markdown":"重新讨论后的结论"}).to_string()).execute(&pool).await.unwrap();
        assert!(approve_draft(&pool, "review", edited["revision"].as_str().unwrap(), false, false).await.is_err());
        let fresh = reviewed_draft(&pool, "review").await.unwrap();
        assert_eq!(fresh["message_edited"], false);
        assert!(fresh["message_summary"].as_str().unwrap().contains("重新讨论后的结论"));
        assert!(!fresh["card"].to_string().contains("旧的概要"));
    }

    #[tokio::test]
    async fn generation_and_queue_recovery_only_create_review_drafts() {
        let pool = review_fixture().await;
        summary_completed(&pool, "review").await.unwrap();
        summary_completed(&pool, "review").await.unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM lark_app_deliveries").fetch_one(&pool).await.unwrap();
        assert_eq!(status, "awaiting_confirmation");
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM lark_app_deliveries").fetch_one(&pool).await.unwrap(), 1);
        // Invalid test credentials would fail if this path attempted any network send.
        let draft = reviewed_draft(&pool, "review").await.unwrap();
        assert!(draft["text"].as_str().unwrap().contains("第一版纪要"));
        let stored: String = sqlx::query_scalar("SELECT message_content FROM lark_app_deliveries WHERE meeting_id='review'").fetch_one(&pool).await.unwrap();
        assert_eq!(draft["card"], serde_json::from_str::<Value>(&stored).unwrap());
        assert_eq!(draft["chat_id"], "oc_review");
    }

    #[tokio::test]
    async fn card_only_approval_explicitly_skips_the_attachment() {
        let pool = review_fixture().await;
        let draft = begin_review(&pool, "review").await.unwrap();
        let (_, approved) = approve_draft(&pool, "review", draft["revision"].as_str().unwrap(), false, false).await.unwrap();
        assert_eq!(approved["include_markdown"], false);
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT attachment_status FROM lark_app_deliveries WHERE meeting_id='review'").fetch_one(&pool).await.unwrap(), "skipped");
    }
    #[tokio::test]
    async fn changes_after_preview_require_a_new_confirmation() {
        let pool = review_fixture().await;
        let first = reviewed_draft(&pool, "review").await.unwrap();
        sqlx::query("UPDATE summary_processes SET result=?").bind(json!({"markdown":"修订后的纪要"}).to_string()).execute(&pool).await.unwrap();
        assert!(approve_draft(&pool, "review", first["revision"].as_str().unwrap(), false, false).await.is_err());
        let updated = reviewed_draft(&pool, "review").await.unwrap();
        assert!(updated["text"].as_str().unwrap().contains("修订后的纪要"));
        sqlx::query("UPDATE lark_app_routes SET chat_id='oc_changed'").execute(&pool).await.unwrap();
        assert!(approve_draft(&pool, "review", updated["revision"].as_str().unwrap(), false, false).await.is_err());
        let final_draft = reviewed_draft(&pool, "review").await.unwrap();
        approve_draft(&pool, "review", final_draft["revision"].as_str().unwrap(), false, false).await.unwrap();
        assert!(approve_draft(&pool, "review", final_draft["revision"].as_str().unwrap(), false, false).await.is_err());
    }
    #[tokio::test]
    async fn blocked_pending_and_sent_meetings_cannot_be_confirmed() {
        let pool = review_fixture().await;
        let draft = reviewed_draft(&pool, "review").await.unwrap();
        sqlx::query("UPDATE summary_processes SET status='pending'").execute(&pool).await.unwrap();
        assert!(approve_draft(&pool, "review", draft["revision"].as_str().unwrap(), false, false).await.is_err());
        sqlx::query("UPDATE summary_processes SET status='completed'").execute(&pool).await.unwrap();
        sqlx::query("UPDATE lark_recording_links SET blocked=1").execute(&pool).await.unwrap();
        assert!(approve_draft(&pool, "review", draft["revision"].as_str().unwrap(), false, false).await.is_err());
        sqlx::query("UPDATE lark_recording_links SET blocked=0").execute(&pool).await.unwrap();
        sqlx::query("UPDATE lark_app_deliveries SET status='sent'").execute(&pool).await.unwrap();
        summary_completed(&pool, "review").await.unwrap();
        assert!(reviewed_draft(&pool, "review").await.unwrap_err().contains("已经发送"));
    }
    #[tokio::test]
    async fn upgrade_retracts_unapproved_queue_but_preserves_sent_history() {
        let pool = review_fixture().await;
        summary_completed(&pool, "review").await.unwrap();
        sqlx::query("UPDATE lark_app_deliveries SET status='queued'").execute(&pool).await.unwrap();
        sqlx::query("DROP TABLE summary_markdown_exports").execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261019000000_summary_review.sql")).execute(&pool).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT status FROM lark_app_deliveries").fetch_one(&pool).await.unwrap(), "awaiting_confirmation");
    }
    #[tokio::test]
    async fn resend_requires_fresh_review_and_preserves_last_success_on_failure() {
        let pool = review_fixture().await;
        let first = begin_review(&pool, "review").await.unwrap();
        approve_draft(&pool, "review", first["revision"].as_str().unwrap(), false, false).await.unwrap();
        let uuid: String = sqlx::query_scalar("SELECT message_uuid FROM lark_app_deliveries").fetch_one(&pool).await.unwrap();
        set_delivery(&pool, "review", "sent", "成功", Some("message-one")).await.unwrap();
        set_attachment(&pool, "review", "sent", "附件成功").await.unwrap();
        sqlx::query("UPDATE lark_app_deliveries SET last_sent_at=1234567890").execute(&pool).await.unwrap();
        summary_completed(&pool, "review").await.unwrap();
        assert!(approve_draft(&pool, "review", first["revision"].as_str().unwrap(), false, false).await.is_err());
        sqlx::query("UPDATE summary_processes SET result=?").bind(json!({"markdown":"第二版纪要"}).to_string()).execute(&pool).await.unwrap();
        let second = begin_review(&pool, "review").await.unwrap();
        assert_eq!(second["last_sent_at"], 1234567890_i64);
        assert!(second["text"].as_str().unwrap().contains("第二版纪要"));
        assert_ne!(first["revision"], second["revision"]);
        assert_ne!(uuid, sqlx::query_scalar::<_,String>("SELECT message_uuid FROM lark_app_deliveries").fetch_one(&pool).await.unwrap());
        assert!(approve_draft(&pool, "review", first["revision"].as_str().unwrap(), false, false).await.is_err());
        approve_draft(&pool, "review", second["revision"].as_str().unwrap(), false, false).await.unwrap();
        assert!(approve_draft(&pool, "review", second["revision"].as_str().unwrap(), false, false).await.is_err());
        set_delivery(&pool, "review", "failed", "拒绝", None).await.unwrap();
        let retry = begin_review(&pool, "review").await.unwrap();
        assert_eq!(retry["last_sent_at"], 1234567890_i64);
        assert_ne!(retry["revision"], second["revision"]);
        set_delivery(&pool, "review", "unknown", "结果不确定", None).await.unwrap();
        let uncertain = begin_review(&pool, "review").await.unwrap();
        assert_eq!(uncertain["requires_duplicate_ack"], true);
        assert!(approve_draft(&pool, "review", uncertain["revision"].as_str().unwrap(), false, false).await.is_err());
        approve_draft(&pool, "review", uncertain["revision"].as_str().unwrap(), true, false).await.unwrap();
        assert!(approve_draft(&pool, "review", uncertain["revision"].as_str().unwrap(), true, false).await.is_err());
    }
    #[tokio::test]
    async fn resend_migration_backfills_only_successful_deliveries() {
        let pool = review_fixture().await;
        summary_completed(&pool, "review").await.unwrap();
        sqlx::query("ALTER TABLE lark_app_deliveries DROP COLUMN last_sent_at").execute(&pool).await.unwrap();
        sqlx::query("DROP TABLE lark_chat_names").execute(&pool).await.unwrap();
        sqlx::query("UPDATE lark_app_deliveries SET status='sent',updated_at=1234567890").execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261019020000_lark_resend.sql")).execute(&pool).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT last_sent_at FROM lark_app_deliveries").fetch_one(&pool).await.unwrap(),1234567890);
    }
    #[tokio::test]
    async fn group_names_are_app_scoped_and_cached_without_network_polling() {
        let pool = database().await;
        let c = load(&pool).await.unwrap();
        sqlx::query("INSERT INTO lark_chat_names(app_id,chat_id,name,checked_at) VALUES('cli_other','oc_review','其他公司',?),('cli_test','oc_review','研发组',?),('cli_test','oc_missing',NULL,?)")
            .bind(now()).bind(now()).bind(now()).execute(&pool).await.unwrap();
        assert_eq!(resolve_chat_name(&pool, &c, "oc_review").await.unwrap().as_deref(),Some("研发组"));
        assert_eq!(resolve_chat_name(&pool, &c, "oc_missing").await.unwrap(),None);
        assert!(resolve_chat_name(&pool, &c, "oc_/../x").await.is_err());
    }
    async fn database() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql("PRAGMA foreign_keys=ON; CREATE TABLE meetings(id TEXT PRIMARY KEY,title TEXT,created_at TEXT,folder_path TEXT); CREATE TABLE summary_processes(meeting_id TEXT PRIMARY KEY,status TEXT,result TEXT);")
            .execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!(
            "../../migrations/20261015000000_add_lark_app.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!(
            "../../migrations/20261015010000_add_lark_launches.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261020010000_lark_end_meeting.sql")).execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!(
            "../../migrations/20261015020000_add_lark_auto_recording.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261015040000_add_recording_jobs.sql")).execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261015030000_add_transcription_checks.sql"))
            .execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261015050000_add_lark_delivery_card.sql"))
            .execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261019020000_lark_resend.sql"))
            .execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261019030000_lark_uncertain_retry.sql"))
            .execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261019060000_lark_message_editor.sql")).execute(&pool).await.unwrap();
        sqlx::raw_sql("CREATE TABLE summary_markdown_exports(meeting_id TEXT PRIMARY KEY,path TEXT,error TEXT,updated_at TEXT)").execute(&pool).await.unwrap();
        let c = AppConfig {
            app_id: "cli_test".into(),
            secret: "never-a-real-secret".into(),
            enabled: true,
            auto_send: true,
            owner_id: "ou_owner".into(),
            ..Default::default()
        };
        save(&pool, &c).await.unwrap();
        pool
    }
    fn event(id: &str, sender: &str, chat: &str, time: i64) -> Value {
        json!({"header":{"app_id":"cli_test","event_type":"im.message.receive_v1","event_id":id},"event":{"sender":{"sender_id":{"open_id":sender}},"message":{"chat_type":"group","chat_id":chat,"message_type":"text","mentions":[{"key":"@_user_1"}],"create_time":(time*1000).to_string(),"content":json!({"text":"@_user_1 绑定会议 123456789"}).to_string()}}})
    }
    #[tokio::test]
    async fn group_end_commands_are_persisted_once_with_original_target_and_requester() {
        let pool=database().await;
        let mut c=load(&pool).await.unwrap(); c.allow_group_launch=true; save(&pool,&c).await.unwrap();
        let mut launch=event("launch","ou_alice","oc_a",now()-1);
        launch["event"]["sender"]["sender_type"]=json!("user");
        launch["event"]["message"]["message_id"]=json!("om_launch");
        launch["event"]["message"]["content"]=json!(json!({"text":"@_user_1 开会"}).to_string());
        receive(&pool,&c,&launch).await.unwrap();
        sqlx::query("UPDATE lark_meeting_launches SET meeting_no='123456789'").execute(&pool).await.unwrap();
        let mut end=event("end","ou_alice","oc_a",now());
        end["event"]["sender"]["sender_type"]=json!("user");
        end["event"]["message"]["message_id"]=json!("om_end");
        end["event"]["message"]["content"]=json!(json!({"text":"@_user_1 结束会议：123456789"}).to_string());
        receive(&pool,&c,&end).await.unwrap();
        receive(&pool,&c,&end).await.unwrap();
        end["header"]["event_id"]=json!("redelivery");
        receive(&pool,&c,&end).await.unwrap();
        let rows=sqlx::query("SELECT e.requester_id,e.chat_id,j.requester_id AS creator FROM lark_meeting_end_requests e JOIN lark_meeting_launches j ON j.request_id=e.launch_id").fetch_all(&pool).await.unwrap();
        assert_eq!(rows.len(),1);
        assert_eq!(rows[0].get::<String,_>("requester_id"),"ou_alice");
        assert_eq!(rows[0].get::<String,_>("creator"),"ou_alice");
        assert_eq!(rows[0].get::<String,_>("chat_id"),"oc_a");
    }
    #[tokio::test]
    async fn authenticated_owner_only_and_durable_event_dedup() {
        let pool = database().await;
        let c = load(&pool).await.unwrap();
        receive(&pool, &c, &event("foreign", "ou_other", "oc_wrong", now()))
            .await
            .unwrap();
        receive(
            &pool,
            &c,
            &event("stale", "ou_owner", "oc_wrong", now() - 601),
        )
        .await
        .unwrap();
        let e = event("same", "ou_owner", "oc_right", now());
        receive(&pool, &c, &e).await.unwrap();
        receive(&pool, &c, &e).await.unwrap();
        let chats: Vec<String> = sqlx::query_scalar("SELECT chat_id FROM lark_app_routes")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(chats, vec!["oc_right"]);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM lark_app_events")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
    }
    #[tokio::test]
    async fn conflicting_groups_never_enqueue() {
        let pool = database().await;
        let c = load(&pool).await.unwrap();
        for (id, chat) in [("a", "oc_a"), ("b", "oc_b")] {
            receive(&pool, &c, &event(id, "ou_owner", chat, now()))
                .await
                .unwrap();
        }
        sqlx::raw_sql("INSERT INTO meetings VALUES('local','周会','2026-10-15','folder'); INSERT INTO summary_processes VALUES('local','completed','{\"markdown\":\"会议结论\"}'); INSERT INTO lark_recording_links(folder_path,app_id,owner_id,remote_id,meeting_no,started_at,stopped,auto_send) VALUES('folder','cli_test','ou_owner','1234567890123','123456789',1,1,1);")
            .execute(&pool).await.unwrap();
        assert!(summary_completed(&pool, "local")
            .await
            .unwrap_err()
            .contains("多个群"));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM lark_app_deliveries")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        sqlx::query("UPDATE lark_app_routes SET expires_at=0")
            .execute(&pool)
            .await
            .unwrap();
        assert!(summary_completed(&pool, "local")
            .await
            .unwrap_err()
            .contains("未找到"));
    }
    #[tokio::test]
    async fn unlinked_and_incomplete_recordings_never_send() {
        let pool = database().await;
        sqlx::raw_sql("INSERT INTO meetings VALUES('old','旧纪要','2026-10-15',NULL); INSERT INTO summary_processes VALUES('old','completed','{\"markdown\":\"旧纪要\"}');")
            .execute(&pool).await.unwrap();
        summary_completed(&pool, "old").await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM lark_app_deliveries")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
    }
    #[tokio::test]
    async fn credentials_are_encrypted_at_rest() {
        let pool = database().await;
        let raw: String = sqlx::query_scalar("SELECT config_json FROM lark_app_settings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(!raw.contains("never-a-real-secret"));
        assert_eq!(load(&pool).await.unwrap().secret, "never-a-real-secret");
    }

    #[tokio::test]
    async fn stopped_routing_is_not_invalidated_by_later_authorization_expiry() {
        let pool = database().await;
        sqlx::query("INSERT INTO lark_recording_links(folder_path,app_id,owner_id,started_at,remote_id,stopped,blocked,detail) VALUES('folder','cli_test','ou_owner',1,'123',1,0,'已确认离会')")
            .execute(&pool).await.unwrap();
        // Fixture has no user token. Rechecking it during transcription would fail.
        match_current(&pool).await.unwrap();
        recording_stopped(&pool,"folder").await.unwrap();
        let blocked:i64=sqlx::query_scalar("SELECT blocked FROM lark_recording_links WHERE folder_path='folder'").fetch_one(&pool).await.unwrap();
        assert_eq!(blocked,0);
    }

    #[tokio::test]
    async fn members_can_launch_in_each_source_group_but_cannot_bind_or_spoof_senders() {
        let pool = database().await;
        let stale = load(&pool).await.unwrap();
        let mut c = stale.clone();
        c.allow_group_launch = true;
        save(&pool, &c).await.unwrap();
        for (id, sender, chat) in [("a", "ou_alice", "oc_a"), ("b", "ou_bob", "oc_b")] {
            let mut e = event(id, sender, chat, now());
            e["event"]["sender"]["sender_type"] = json!("user");
            e["event"]["message"]["message_id"] = json!(format!("om_{id}"));
            e["event"]["message"]["content"] = json!(json!({"text":"@_user_1 开会 团队会议"}).to_string());
            receive(&pool, &stale, &e).await.unwrap();
            receive(&pool, &stale, &e).await.unwrap();
            e["header"]["event_id"] = json!(format!("redelivered_{id}"));
            receive(&pool, &stale, &e).await.unwrap();
        }
        let rows = sqlx::query("SELECT chat_id,owner_id FROM lark_meeting_launches ORDER BY chat_id").fetch_all(&pool).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get::<String,_>("chat_id"), "oc_a");
        assert_eq!(rows[1].get::<String,_>("chat_id"), "oc_b");
        assert!(rows.iter().all(|r| r.get::<String,_>("owner_id") == "ou_owner"));
        let mut binding = event("binding", "ou_alice", "oc_other", now());
        binding["event"]["sender"]["sender_type"] = json!("user");
        receive(&pool, &c, &binding).await.unwrap();
        binding["header"]["event_id"] = json!("binding_id");
        binding["event"]["message"]["content"] = json!(json!({"text":"@_user_1 绑定会议ID 1234567890123456789"}).to_string());
        receive(&pool, &c, &binding).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM lark_app_routes").fetch_one(&pool).await.unwrap(),0);
        for (id, sender_type, sender, chat_type, mentions, age) in [
            ("bot", "app", "cli_bot", "group", true, 0),
            ("missing", "user", "", "group", true, 0),
            ("dm", "user", "ou_alice", "p2p", true, 0),
            ("mention", "user", "ou_alice", "group", false, 0),
            ("old", "user", "ou_alice", "group", true, 601),
        ] {
            let mut e = event(id,sender,"oc_c",now()-age);
            e["event"]["sender"]["sender_type"] = json!(sender_type);
            e["event"]["message"]["message_id"] = json!(id);
            e["event"]["message"]["chat_type"] = json!(chat_type);
            if !mentions { e["event"]["message"]["mentions"] = json!([]); }
            e["event"]["message"]["content"] = json!(json!({"text":"@_user_1 开会"}).to_string());
            receive(&pool,&c,&e).await.unwrap();
        }
        c.allow_group_launch = false;
        save(&pool,&c).await.unwrap();
        let mut e = event("disabled","ou_alice","oc_c",now());
        e["event"]["sender"]["sender_type"] = json!("user");
        e["event"]["message"]["message_id"] = json!("disabled");
        e["event"]["message"]["content"] = json!(json!({"text":"@_user_1 开会"}).to_string());
        receive(&pool,&stale,&e).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM lark_meeting_launches").fetch_one(&pool).await.unwrap(),2);
    }

    #[tokio::test]
    async fn launches_are_owner_only_durable_and_message_deduplicated() {
        let pool = database().await;
        let c = load(&pool).await.unwrap();
        let mut e = event("launch-1", "ou_other", "oc_right", now());
        e["event"]["message"]["message_id"] = json!("om_launch");
        e["event"]["message"]["content"] =
            json!(json!({"text":"@_user_1 开会 研发周会"}).to_string());
        receive(&pool, &c, &e).await.unwrap();
        e["event"]["sender"]["sender_id"]["open_id"] = json!("ou_owner");
        receive(&pool, &c, &e).await.unwrap();
        receive(&pool, &c, &e).await.unwrap();
        e["header"]["event_id"] = json!("launch-redelivered");
        receive(&pool, &c, &e).await.unwrap();
        let rows = sqlx::query("SELECT chat_id,topic,status FROM lark_meeting_launches")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get::<String, _>("chat_id"), "oc_right");
        assert_eq!(rows[0].get::<String, _>("topic"), "研发周会");
        assert_eq!(rows[0].get::<String, _>("status"), "queued");
        sqlx::query("UPDATE lark_meeting_launches SET status='creating'")
            .execute(&pool)
            .await
            .unwrap();
        super::super::launches::recover(&pool).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM lark_meeting_launches")
                .fetch_one(&pool)
                .await
                .unwrap(),
            "unknown"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM lark_app_routes")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
    }
}
