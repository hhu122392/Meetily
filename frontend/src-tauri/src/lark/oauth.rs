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
fn auth_scope(meeting_control: bool) -> String {
    if meeting_control { format!("{AUTH_SCOPE} vc:meeting") } else { AUTH_SCOPE.into() }
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
pub async fn lark_app_authorize(handle: tauri::AppHandle, state: tauri::State<'_, AppState>, include_meeting_control: Option<bool>, include_cloud_recording: Option<bool>) -> Result<String, String> {
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
        .append_pair("scope", &requested_scope(include_meeting_control.unwrap_or(false), include_cloud_recording.unwrap_or(false)))
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
                    else { Some(complete(&handle,&pool,&config,query.get("code").map(String::as_str).unwrap_or("")).await) }
                }else{None};
                let message=match &result {Some(Ok(()))=>"授权完成，请返回 Meetily。",Some(Err(_))=>"授权未完成，请返回 Meetily 查看原因。",None=>"无效授权回调。"};
                let response=format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",message.len(),message);
                let _=socket.write_all(response.as_bytes()).await;
                if let Some(result)=result{return result;}
            }
        }).await;
        match result {
            Ok(Ok(())) => app::status("用户授权完成，等待长连接和会议识别。").await,
            Ok(Err(e)) => app::status(e).await,
            Err(_) => app::status("Lark 用户授权等待超时，请重试。").await,
        }
    });
    Ok(url.to_string())
}
async fn complete(handle: &tauri::AppHandle, pool: &SqlitePool, original: &AppConfig, code: &str) -> Result<(), String> {
    if code.is_empty() {
        return Err("用户未授予会议访问权限。".into());
    }
    let token = exchange(
        original,
        json!({"grant_type":"authorization_code","code":code,"redirect_uri":REDIRECT}),
    )
    .await?;
    let _lock = CONFIG_LOCK.lock().await;
    let mut c = app::load(pool).await?;
    if c.app_id != original.app_id || c.secret != original.secret {
        return Err("授权期间应用配置改变，请重新授权。".into());
    }
    // A fresh authorization must never reuse a previous user's refresh token.
    c.refresh_token.clear();
    apply_token(&mut c, &token)?;
    let user = app::api(
        reqwest::Method::GET,
        "/open-apis/authen/v1/user_info",
        &c.user_token,
        None,
    )
    .await?;
    c.owner_id = user["open_id"]
        .as_str()
        .filter(|s| s.starts_with("ou_"))
        .ok_or("未取得授权用户身份。")?
        .into();
    let tenant = user["tenant_key"].as_str().filter(|t| !t.trim().is_empty()).ok_or("Lark 未返回公司标识，未保存本次授权。")?;
    crate::company::bind_identity(handle, &c.app_id, Some(tenant))?;
    app::save(pool, &c).await?;
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
