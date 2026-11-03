pub(crate) mod payload;
#[cfg(any(target_os = "macos", test))]
mod macos_credentials;
mod card;
mod launches;
mod ending;
mod meeting_state;
pub mod auto_recording;
pub(crate) mod cloud_recording;
pub mod app;
pub(crate) mod routing;
mod transport;
pub mod oauth;

use crate::state::AppState;
use once_cell::sync::Lazy;
use payload::{check_response, fingerprint, message_text, signed_payload, validate_webhook, BotConfig};
use serde::Serialize;
use sqlx::SqlitePool;
use std::time::Duration;
use tokio::sync::Mutex;

// Serializes config changes and sends, including rapid clicks from multiple windows.
pub(crate) static SEND_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

#[derive(Serialize)]
pub struct ConfigView {
    settings: BotConfig,
    revision: String,
}

#[derive(Serialize)]
pub struct SendResult {
    already_sent: bool,
    sent_at: String,
}

async fn read_config(pool: &SqlitePool) -> Result<BotConfig, String> {
    let json: Option<String> = sqlx::query_scalar("SELECT config_json FROM lark_bot_settings WHERE id = 1")
        .fetch_optional(pool).await.map_err(|_| "无法读取 Lark 配置。".to_owned())?;
    match json {
        Some(json) => serde_json::from_str(&json).map_err(|_| "Lark 配置损坏，请重新保存。".to_owned()),
        None => Ok(BotConfig::default()),
    }
}

#[tauri::command]
pub async fn lark_get_config(state: tauri::State<'_, AppState>) -> Result<ConfigView, String> {
    let settings = read_config(state.db_manager.pool()).await?;
    Ok(ConfigView { revision: settings.revision(), settings })
}

#[tauri::command]
pub async fn lark_save_config(state: tauri::State<'_, AppState>, config: BotConfig) -> Result<(), String> {
    let config = config.normalize()?;
    let _guard = SEND_LOCK.try_lock().map_err(|_| "正在发送纪要，请稍后再修改配置。".to_owned())?;
    let json = serde_json::to_string(&config).map_err(|_| "无法保存 Lark 配置。".to_owned())?;
    sqlx::query("INSERT INTO lark_bot_settings (id, config_json) VALUES (1, ?) ON CONFLICT(id) DO UPDATE SET config_json = excluded.config_json")
        .bind(json).execute(state.db_manager.pool()).await.map_err(|_| "无法保存 Lark 配置。".to_owned())?;
    Ok(())
}

#[tauri::command]
pub async fn lark_clear_config(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let _guard = SEND_LOCK.try_lock().map_err(|_| "正在发送纪要，请稍后再清除配置。".to_owned())?;
    sqlx::query("DELETE FROM lark_bot_settings WHERE id = 1")
        .execute(state.db_manager.pool()).await.map_err(|_| "无法清除 Lark 配置。".to_owned())?;
    Ok(())
}

#[tauri::command]
pub async fn lark_send_summary(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    title: String,
    date: String,
    summary: String,
    expected_config_revision: String,
) -> Result<SendResult, String> {
    let _guard = SEND_LOCK.try_lock().map_err(|_| "已有纪要正在发送，请等待发送结果。".to_owned())?;
    let pool = state.db_manager.pool();
    let config = read_config(pool).await?.normalize()?;
    if config.revision() != expected_config_revision {
        return Err("目标群配置已改变，请关闭预览并重新打开，确认目标群后发送。".into());
    }
    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meetings WHERE id = ?")
        .bind(&meeting_id).fetch_one(pool).await.map_err(|_| "无法读取会议信息。".to_owned())?;
    if exists == 0 { return Err("会议已不存在，请重新打开会议。".into()); }
    let url = validate_webhook(&config.webhook)?;
    let text = message_text(&title, &date, &summary)?;
    let body = signed_payload(&text, &config.secret, chrono::Utc::now().timestamp())?;
    let key = fingerprint(&[&meeting_id, url.as_str(), &text]);
    let sent: Option<String> = sqlx::query_scalar("SELECT sent_at FROM lark_summary_deliveries WHERE fingerprint = ?")
        .bind(&key).fetch_optional(pool).await.map_err(|_| "无法读取发送记录，已取消发送。".to_owned())?;
    if let Some(sent_at) = sent {
        return Ok(SendResult { already_sent: true, sent_at });
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build().map_err(|_| "无法初始化网络连接。".to_owned())?;
    let response = client.post(url).json(&body).send().await.map_err(|_| {
        "未能确认发送结果（网络异常或超时）。请先检查群内是否收到纪要，再决定是否重试。".to_owned()
    })?;
    if !response.status().is_success() {
        return Err(format!("Lark 返回 HTTP {}，未能确认发送成功。请检查群消息和网络后再重试。", response.status().as_u16()));
    }
    let value: serde_json::Value = response.json().await.map_err(|_| "Lark 响应无法解析，请先检查群内是否收到纪要，再决定是否重试。".to_owned())?;
    check_response(&value)?;
    let sent_at = chrono::Utc::now().to_rfc3339();
    sqlx::query("INSERT INTO lark_summary_deliveries (fingerprint, meeting_id, sent_at) VALUES (?, ?, ?)")
        .bind(&key).bind(&meeting_id).bind(&sent_at).execute(pool).await
        .map_err(|_| "纪要已发送，但本地发送记录保存失败。请勿立即重试，以免重复发送。".to_owned())?;
    Ok(SendResult { already_sent: false, sent_at })
}
