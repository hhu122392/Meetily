use super::app::{self, AppConfig, BASE, CONFIG_LOCK};
use crate::state::AppState;
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use sqlx::SqlitePool;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Mutex,
};

pub const REDIRECT: &str = "http://127.0.0.1:18576/lark/callback";
const AUTH_SCOPE: &str = "vc:meeting.meetingevent:read offline_access";
const GROUP_SCOPE: &str = "im:chat:read im:message:readonly im:message.group_msg:get_as_user";
const CALENDAR_SCOPE: &str = "calendar:calendar.event:read offline_access";
fn document_scope(calendar:bool)->String {
    let mut scope="drive:drive:readonly docx:document docx:document.block:convert offline_access".to_owned();
    if calendar {scope.push_str(" calendar:calendar.event:read");}
    scope
}
fn document_scope_for(c:&AppConfig)->String{
    let mut scope=document_scope(c.calendar_authorized);
    if !c.manual_mode {scope.push(' ');scope.push_str(&requested_scope(true,c.recording_source==app::RecordingSource::Cloud));}
    let mut seen=std::collections::HashSet::new();scope.split_whitespace().filter(|s|seen.insert(*s)).collect::<Vec<_>>().join(" ")
}
fn combined_scope(c: &AppConfig, calendar: bool, documents: bool, tasks: bool, meeting: bool, cloud: bool) -> String {
    let mut scope = if tasks { "task:task:write offline_access".to_owned() }
        else if documents { document_scope_for(c) }
        else { authorization_scope(calendar, meeting, cloud) };
    if c.calendar_authorized { scope.push_str(" calendar:calendar.event:read"); }
    if c.documents_authorized { scope.push(' '); scope.push_str(&document_scope(false)); }
    if c.tasks_authorized { scope.push_str(" task:task:write"); }
    if tasks && !c.manual_mode && !c.user_token.is_empty() { scope.push(' '); scope.push_str(&requested_scope(true,c.recording_source==app::RecordingSource::Cloud)); }
    let mut seen=std::collections::HashSet::new();
    scope.split_whitespace().filter(|s|seen.insert(*s)).collect::<Vec<_>>().join(" ")
}
fn apply_authorization_mode(c:&mut AppConfig,calendar_only:bool,documents_only:bool){
    if calendar_only&&!documents_only {apply_calendar_mode(c);}
    else if documents_only {c.calendar_auth_revision=uuid::Uuid::new_v4().to_string();}
}
fn authorization_scope(calendar_only: bool, meeting_control: bool, cloud: bool) -> String {
    if calendar_only { CALENDAR_SCOPE.into() } else { requested_scope(meeting_control, cloud) }
}
fn apply_calendar_mode(c: &mut AppConfig) {
    c.enabled = true;
    c.manual_mode = true;
    c.auto_record = false;
    c.auto_send = false;
    c.allow_group_launch = false;
    c.recording_source = app::RecordingSource::Local;
    c.calendar_auth_revision = uuid::Uuid::new_v4().to_string();
}
fn auth_scope(meeting_control: bool) -> String {
    if meeting_control { format!("{AUTH_SCOPE} vc:meeting calendar:calendar.event:read {GROUP_SCOPE}") } else { AUTH_SCOPE.into() }
}
fn requested_scope(meeting_control: bool, cloud: bool) -> String {
    let mut scope = auth_scope(meeting_control);
    if cloud { scope.push_str(" vc:record vc:record:readonly vc:meeting:readonly minutes:minutes.media:export"); }
    scope
}
static AUTH_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

async fn exchange(c: &AppConfig, mut body: Value) -> Result<Value, String> {
    body["client_id"] = json!(c.app_id);
    body["client_secret"] = json!(c.secret);
    let response = app::client()?
        .post(format!("{BASE}/open-apis/authen/v2/oauth/token"))
        .json(&body)
        .send()
        .await
        .map_err(|_| "Lark 用户授权网络失败。")?;
    if !response.status().is_success() {
        return Err(format!(
            "Lark 用户授权 HTTP {}。",
            response.status().as_u16()
        ));
    }
    let v: Value = response.json().await.map_err(|_| "用户授权响应异常。")?;
    if v["code"].as_i64().is_some_and(|x| x != 0) || v["access_token"].as_str().is_none() {
        return Err(format!(
            "用户授权失败，错误码 {}；请检查应用发布、回调地址和用户权限。",
            v["code"]
        ));
    }
    Ok(v)
}
fn apply_token(c: &mut AppConfig, v: &Value) -> Result<(), String> {
    c.user_token = v["access_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("未返回用户凭证。")?
        .into();
    if let Some(refresh) = v["refresh_token"].as_str().filter(|v| !v.is_empty()) {
        c.refresh_token = refresh.into();
    }
    c.expires_at = app::now() + v["expires_in"].as_i64().unwrap_or(3600);
    Ok(())
}
pub(super) async fn authorized_config(pool: &SqlitePool) -> Result<AppConfig, String> {
    let _lock = CONFIG_LOCK.lock().await;
    authorized_config_locked(pool).await
}

// Caller must hold CONFIG_LOCK across the authorized operation.
pub(super) async fn authorized_config_locked(pool: &SqlitePool) -> Result<AppConfig, String> {
    let mut c = app::load(pool).await?;
    if !c.enabled {
        return Err("企业应用已停用。".into());
    }
    if c.user_token.is_empty() {
        return Err("请先登录 Lark，授权识别你正在参加的会议。".into());
    }
    if c.expires_at < app::now() + 120 {
        if c.refresh_token.is_empty() {
            return Err("Lark 用户授权已到期，请重新登录授权。".into());
        }
        let v = exchange(
            &c,
            json!({"grant_type":"refresh_token","refresh_token":c.refresh_token}),
        )
        .await?;
        apply_token(&mut c, &v)?;
        app::save(pool, &c).await?;
    }
    Ok(c)
}

#[tauri::command]
pub async fn lark_app_authorize(handle: tauri::AppHandle, state: tauri::State<'_, AppState>, include_meeting_control: Option<bool>, include_cloud_recording: Option<bool>, calendar_only: Option<bool>, documents_only: Option<bool>, tasks_only: Option<bool>) -> Result<String, String> {
    let calendar_only = calendar_only.unwrap_or(false);
    if calendar_only {
        return Err("日历同步功能已停用，请使用会议助手指定会议号。".into());
    }
    let documents_only=documents_only.unwrap_or(false);
    let tasks_only=tasks_only.unwrap_or(false);
    if [calendar_only,documents_only,tasks_only].iter().filter(|v|**v).count()>1 {return Err("一次只能选择一种授权入口。".into());}
    let guard = AUTH_LOCK
        .try_lock()
        .map_err(|_| "已有授权窗口，请先完成授权或等待五分钟后重试。")?;
    let pool = state.db_manager.pool().clone();
    let config = app::load(&pool).await?;
    if config.secret.is_empty() {
        return Err("请先保存 App ID 和 App Secret。".into());
    }
    let listener = TcpListener::bind("127.0.0.1:18576")
        .await
        .map_err(|_| "本机授权端口 18576 被占用。")?;
    let nonce = uuid::Uuid::new_v4().to_string();
    let mut url =
        url::Url::parse("https://accounts.larksuite.com/open-apis/authen/v1/authorize").unwrap();
    url.query_pairs_mut()
        .append_pair("client_id", &config.app_id)
        .append_pair("redirect_uri", REDIRECT)
        .append_pair("response_type", "code")
        .append_pair("scope", &combined_scope(&config,calendar_only,documents_only,tasks_only,include_meeting_control.unwrap_or(false),include_cloud_recording.unwrap_or(false)))
        .append_pair("state", &nonce);
    tokio::spawn(async move {
        let _guard = guard;
        let result=tokio::time::timeout(Duration::from_secs(300),async {
            loop {
                let (mut socket,peer)=listener.accept().await.map_err(|_|"无法接收本机授权回调。")?;
                if !peer.ip().is_loopback(){continue;}
                let mut bytes=vec![0u8;8192];
                let n=match tokio::time::timeout(Duration::from_secs(5),socket.read(&mut bytes)).await {Ok(Ok(n))=>n,_=>continue};
                let raw=String::from_utf8_lossy(&bytes[..n]);
                let path=raw.lines().next().and_then(|line|line.strip_prefix("GET ")).and_then(|s|s.split_whitespace().next()).unwrap_or("");
                let parsed=url::Url::parse(&format!("http://127.0.0.1:18576{path}"));
                let result=if let Ok(parsed)=parsed {
                    let query:std::collections::HashMap<_,_>=parsed.query_pairs().into_owned().collect();
                    if parsed.path()!="/lark/callback" || query.get("state")!=Some(&nonce) {None}
                    else { Some(complete(&handle,&pool,&config,query.get("code").map(String::as_str).unwrap_or(""),calendar_only,documents_only,tasks_only).await) }
                }else{None};
                let message=match &result {Some(Ok(()))=>"授权完成，请返回 Meetily。",Some(Err(_))=>"授权未完成，请返回 Meetily 查看原因。",None=>"无效授权回调。"};
                let response=format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",message.len(),message);
                let _=socket.write_all(response.as_bytes()).await;
                if let Some(result)=result{return result;}
            }
        }).await;
        match result {
            Ok(Ok(())) => {
                app::restart(pool.clone()).await;
                app::status(if tasks_only {"Lark 任务已连接，仅在人工确认后创建任务。"} else if documents_only {"个人云空间已连接，保存文档不会自动发群。"} else { "本人账号已登录，请选择会议群并保存；群同步使用本人的读取权限。" }).await;
            }
            Ok(Err(e)) => app::status(e).await,
            Err(_) => app::status("Lark 用户授权等待超时，请重试。").await,
        }
    });
    Ok(url.to_string())
}
async fn complete(handle: &tauri::AppHandle, pool: &SqlitePool, original: &AppConfig, code: &str, calendar_only: bool, documents_only:bool, tasks_only:bool) -> Result<(), String> {
    if code.is_empty() {
        return Err("用户未授予会议访问权限。".into());
    }
    let token = exchange(
        original,
        json!({"grant_type":"authorization_code","code":code,"redirect_uri":REDIRECT}),
    )
    .await?;
    let _policy = super::auto_recording::POLICY_LOCK.lock().await;
    let _lock = CONFIG_LOCK.lock().await;
    let _send = app::DELIVERY_LOCK.lock().await;
    let mut c = app::load(pool).await?;
    if c.app_id != original.app_id || c.secret != original.secret {
        return Err("授权期间应用配置改变，请重新授权。".into());
    }
    // A fresh authorization must never reuse a previous user's refresh token.
    c.refresh_token.clear();
    apply_token(&mut c, &token)?;
    c.calendar_authorized = calendar_only || original.calendar_authorized;
    c.documents_authorized=documents_only || original.documents_authorized;
    c.tasks_authorized=tasks_only || original.tasks_authorized;
    if let Some(scopes)=token["scope"].as_str() {
        if c.tasks_authorized && !scopes.split_whitespace().any(|s|s=="task:task:write") {c.tasks_authorized=false;}
        if c.calendar_authorized && !scopes.split_whitespace().any(|s|s=="calendar:calendar.event:read") {c.calendar_authorized=false;}
        if c.documents_authorized && !scopes.split_whitespace().any(|s|s=="docx:document") {c.documents_authorized=false;}
    }
    apply_authorization_mode(&mut c,calendar_only,documents_only);
    let user = app::api(
        reqwest::Method::GET,
        "/open-apis/authen/v1/user_info",
        &c.user_token,
        None,
    )
    .await?;
    let previous_owner = c.owner_id.clone();
    c.owner_id = user["open_id"]
        .as_str()
        .filter(|s| s.starts_with("ou_"))
        .ok_or("未取得授权用户身份。")?
        .into();
    if c.owner_id != previous_owner {
        c.sync_groups.clear();
        c.manual_sync_groups = None;
        c.auto_record_since = app::now();
    }
    let tenant = user["tenant_key"].as_str().filter(|t| !t.trim().is_empty()).ok_or("Lark 未返回公司标识，未保存本次授权。")?;
    crate::company::bind_identity(handle, &c.app_id, Some(tenant))?;
    app::save(pool, &c).await?;
    if tasks_only && !c.tasks_authorized { return Err("本次登录未授予 task:task:write，请核对应用权限发布状态后重新连接任务。".into()); }
    Ok(())
}

pub(super) fn authorization_status(c: &AppConfig) -> (&'static str, bool) {
    if c.user_token.is_empty() { return ("未授权", false); }
    if c.expires_at <= app::now() && c.refresh_token.is_empty() {
        return ("授权已到期，请重新登录并允许离线访问以便续期", false);
    }
    if c.refresh_token.is_empty() { return ("已授权，但缺少续期凭证；请重新登录并允许离线访问", true); }
    ("已授权，支持自动续期", true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn task_authorization_preserves_existing_scopes_without_enabling_recording_or_delivery() {
        assert!(!combined_scope(&AppConfig::default(),false,false,true,false,false).contains("vc:"));
        let mut c=AppConfig{manual_mode:true,calendar_authorized:true,documents_authorized:true,..Default::default()};
        let scope=combined_scope(&c,false,false,true,false,false);
        for required in ["task:task:write","calendar:calendar.event:read","drive:drive:readonly","docx:document","offline_access"] {assert!(scope.split_whitespace().any(|s|s==required));}
        assert!(!scope.contains("vc:"));apply_authorization_mode(&mut c,false,false);
        assert!(c.manual_mode&&!c.auto_send&&!c.auto_record&&c.recording_source==app::RecordingSource::Local);
        c.tasks_authorized=true;
        assert!(combined_scope(&c,true,false,false,false,false).contains("task:task:write"));
        assert!(combined_scope(&c,false,true,false,false,false).contains("task:task:write"));
    }
    #[test]fn document_authorization_never_changes_recording_or_delivery_settings(){
        let mut c=AppConfig{manual_mode:false,auto_record:true,auto_send:false,allow_group_launch:true,recording_source:app::RecordingSource::Cloud,..Default::default()};
        assert!(document_scope_for(&c).contains("vc:record"));apply_authorization_mode(&mut c,false,true);
        assert!(!c.manual_mode&&c.auto_record&&!c.auto_send&&c.allow_group_launch&&c.recording_source==app::RecordingSource::Cloud);
        c.manual_mode=true;c.recording_source=app::RecordingSource::Local;assert!(!document_scope_for(&c).contains("vc:"));
    }
    #[test]
    fn calendar_login_never_requests_recording_or_meeting_control() {
        let scope = authorization_scope(true, true, true);
        assert_eq!(scope, "calendar:calendar.event:read offline_access");
        assert!(!scope.contains("vc:"));
        assert!(!scope.contains("minutes:"));
        let mut c = AppConfig { auto_send:true,auto_record:true,allow_group_launch:true,..Default::default() };
        apply_calendar_mode(&mut c);
        assert!(c.enabled && c.manual_mode);
        assert!(!c.auto_send && !c.auto_record && !c.allow_group_launch);
        assert!(c.recording_source == app::RecordingSource::Local);
        assert!(!c.calendar_auth_revision.is_empty());
    }
    #[test]fn drive_login_only_requests_document_access_and_preserves_existing_calendar_scope(){
        let scope=document_scope(false);assert!(scope.contains("drive:drive:readonly"));assert!(scope.contains("docx:document.block:convert"));assert!(!scope.contains("calendar:"));assert!(!scope.contains("vc:"));assert!(!scope.contains("im:"));
        assert!(document_scope(true).contains("calendar:calendar.event:read"));
    }
    #[test]
    fn renewal_scope_rotation_and_expiry_are_explicit() {
        assert!(AUTH_SCOPE.split_whitespace().any(|s| s == "offline_access"));
        assert!(!auth_scope(false).split_whitespace().any(|s| s == "vc:meeting"));
        assert!(auth_scope(true).split_whitespace().any(|s| s == "vc:meeting"));
        let mut c = AppConfig { refresh_token: "refresh-old".into(), ..Default::default() };
        apply_token(&mut c,&json!({"access_token":"token","expires_in":7200,"refresh_token":"refresh-new"})).unwrap();
        assert_eq!(c.refresh_token,"refresh-new");
        apply_token(&mut c,&json!({"access_token":"token-2","expires_in":7200})).unwrap();
        assert_eq!(c.refresh_token,"refresh-new");
        c.refresh_token.clear(); c.expires_at=0;
        assert!(!authorization_status(&c).1);
    }
}
