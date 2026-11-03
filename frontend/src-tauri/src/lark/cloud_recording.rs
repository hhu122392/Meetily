use super::{app::{self, AppConfig, db_error, now}, meeting_state, oauth};
use crate::audio::{audio_processing, recording_jobs, recording_preferences, storage_migration};
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};
use std::{path::Path, time::Duration};
use tauri::{AppHandle, Emitter, Runtime};
use tokio::io::AsyncWriteExt;

// Remote IDs, model choices and the original group are durable. Signed URLs and tokens are not.
pub(super) async fn start<R: Runtime>(handle: &AppHandle<R>, pool: &SqlitePool, c: &AppConfig, remote: &str, no: &str, topic: &str) -> Result<(), String> {
    let _storage = storage_migration::use_storage().await?;
    let title = super::routing::meeting_title(topic, no);
    let preferences = recording_preferences::load_recording_preferences(handle).await.map_err(|e| e.to_string())?;
    let folder = audio_processing::create_meeting_folder(&preferences.save_folder, title, false).map_err(|e| e.to_string())?;
    let folder = folder.to_string_lossy().into_owned();
    let id = recording_jobs::register(handle, &folder, title).await?;
    let prepared: Result<(), String> = async {
    let mut tx = pool.begin().await.map_err(db_error)?;
    let chats: Vec<String> = sqlx::query_scalar("SELECT DISTINCT chat_id FROM lark_app_routes WHERE app_id=? AND owner_id=? AND remote_id=? AND expires_at>?")
        .bind(&c.app_id).bind(&c.owner_id).bind(remote).bind(now()).fetch_all(&mut *tx).await.map_err(db_error)?;
    if chats.len() != 1 { return Err("会议群关联已变化，未启动云录音。".into()); }
    sqlx::query("INSERT INTO lark_cloud_recordings(meeting_id,app_id,owner_id,remote_id,meeting_no,auto_send,deadline) VALUES(?,?,?,?,?,?,?)")
        .bind(&id).bind(&c.app_id).bind(&c.owner_id).bind(remote).bind(no).bind(c.auto_send).bind(now()+86400)
        .execute(&mut *tx).await.map_err(db_error)?;
    sqlx::query("UPDATE recording_jobs SET status='cloud_starting',resume_stage='waiting_cloud',target_chat_id=?,detail='正在请求 Lark 开始云录音' WHERE meeting_id=?")
        .bind(&chats[0]).bind(&id).execute(&mut *tx).await.map_err(db_error)?;
    sqlx::query("UPDATE lark_auto_recordings SET status='cloud_waiting',folder_path=?,detail='正在请求云录音，会议结束后下载并转写',updated_at=? WHERE app_id=? AND owner_id=? AND remote_id=?")
        .bind(&folder).bind(now()).bind(&c.app_id).bind(&c.owner_id).bind(remote).execute(&mut *tx).await.map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    Ok(())
    }.await;
    if let Err(e) = prepared {
        set_status(pool, &id, "failed", &e).await?;
        return Err(e);
    }
    let _ = handle.emit("recording-queue-changed", &id);
    // Never automatically retry this mutation: a lost response may still have started recording.
    let result: Result<Value, Failure> = async {
        // current() can refresh OAuth while returning the earlier configuration snapshot.
        // Reload the credential before a privileged operation, and recheck its identity.
        let authorized = oauth::authorized_config(pool).await?;
        if authorized.app_id != c.app_id || authorized.owner_id != c.owner_id {
            return Err(Failure::fatal("Lark 授权账号已改变，未启动云录音"));
        }
        if recording_jobs::cancel_requested(pool, &id).await? { return Err(Failure::fatal("任务已取消")); }
        request(reqwest::Method::PATCH, &format!("/open-apis/vc/v1/meetings/{remote}/recording/start"), &authorized.user_token, Some(json!({"timezone":8}))).await
    }.await;
    let (status, detail) = match result {
        Ok(_) => ("waiting_cloud", "已请求云录音；等待整场会议结束后下载并转写".to_owned()),
        Err(e) => ("failed", format!("未确认云录音启动：{} 请在 Lark 确认或手动开始录制，再点击重试等待下载；重试不会再次启动录制。", e.message)),
    };
    set_status(pool, &id, status, &detail).await?;
    let _ = handle.emit("recording-queue-changed", &id);
    if status == "failed" { return Err(detail); }
    Ok(())
}

#[derive(Debug)]
struct Failure { message: String, retry: bool }
impl Failure {
    fn transient(message: impl Into<String>) -> Self { Self {message:message.into(), retry:true} }
    fn fatal(message: impl Into<String>) -> Self { Self {message:message.into(), retry:false} }
}
impl From<String> for Failure { fn from(message: String) -> Self { Self::fatal(message) } }

fn api_failure(code: i64) -> Failure {
    match code {
        124002 | 2091003 | 121004 => Failure::transient(format!("Lark 录制文件尚未就绪（{code}），稍后自动检查")),
        121001 | 99991400 | 99991401 => Failure::transient(format!("Lark 暂不可用或请求限流（{code}），稍后自动重试")),
        121005 | 2091005 => Failure::fatal(format!("Lark 无权操作录制（{code}）。启动需授权账号在会中且为主持人；下载需录制归属本人及文件下载权限")),
        99991672 | 99991679 => Failure::fatal(format!("缺少 Lark 云录音权限（{code}）。请开通用户身份的录制、录制读取、会议信息、妙记音视频下载权限，并点击“授权会议助手”")),
        99991403 => Failure::fatal("Lark API 月额度已用完（99991403），恢复额度后点击重试"),
        121002 => Failure::fatal("当前 Lark 账号或租户不支持此录制操作（121002），请检查套餐和管理员设置"),
        _ => Failure::fatal(format!("Lark 云录音请求被拒绝（{code}），请检查授权、主持人身份和录制设置")),
    }
}

async fn request(method: reqwest::Method, path: &str, token: &str, body: Option<Value>) -> Result<Value, Failure> {
    let mut req = app::client()?.request(method, format!("{}{path}", app::BASE)).bearer_auth(token);
    if let Some(body) = body { req = req.json(&body); }
    let response = req.send().await.map_err(|_| Failure::transient("Lark 网络连接失败，稍后重试"))?;
    let http = response.status();
    if http.as_u16() == 429 || http.is_server_error() { return Err(Failure::transient(format!("Lark HTTP {}，稍后重试", http.as_u16()))); }
    let v: Value = response.json().await.map_err(|_| Failure::transient("Lark 响应无法解析，稍后重试"))?;
    match v["code"].as_i64() {
        Some(0) if http.is_success() => Ok(v["data"].clone()),
        Some(code) => Err(api_failure(code)),
        _ => Err(Failure::fatal("Lark 云录音响应格式异常")),
    }
}

fn vendor_url(raw: &str) -> Result<url::Url, Failure> {
    let u = url::Url::parse(raw).map_err(|_| Failure::fatal("Lark 返回的下载地址无效"))?;
    let host = u.host_str().unwrap_or_default();
    let trusted = ["larksuite.com", "larksuitecdn.com", "byteoversea.com", "ibytedtos.com", "ibyteimg.com", "feishu.cn", "feishucdn.com"]
        .iter().any(|domain| host == *domain || host.ends_with(&format!(".{domain}")));
    if u.scheme() != "https" || !trusted || !u.username().is_empty() || u.password().is_some() || u.port().is_some_and(|p| p != 443) {
        return Err(Failure::fatal("Lark 返回了未支持的录制地址，请检查接口返回或联系维护人员"));
    }
    Ok(u)
}
fn minute_token(raw: &str) -> Result<String, Failure> {
    let u = vendor_url(raw)?;
    let path: Vec<_> = u.path().trim_end_matches('/').split('/').collect();
    if path.len() != 3 || path[1] != "minutes" || path[2].is_empty() || !path[2].bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(Failure::fatal("Lark 录制结果没有有效的妙记链接"));
    }
    Ok(path[2].into())
}

async fn set_status(pool: &SqlitePool, id: &str, status: &str, detail: &str) -> Result<(), String> {
    sqlx::query("UPDATE recording_jobs SET status=CASE WHEN status IN ('cancelling','cancelled') THEN 'cancelled' ELSE ? END,detail=CASE WHEN status IN ('cancelling','cancelled') THEN '任务已取消，已有文件保留；不会停止 Lark 云录制' ELSE ? END,updated_at=? WHERE meeting_id=?")
        .bind(status).bind(detail).bind(now()).bind(id).execute(pool).await.map_err(db_error)?;
    Ok(())
}

pub(crate) async fn retry(pool: &SqlitePool, id: &str) -> Result<bool, String> {
    let mut tx = pool.begin().await.map_err(db_error)?;
    let changed = sqlx::query("UPDATE recording_jobs SET status='waiting_cloud',resume_stage='waiting_cloud',detail='重新等待云录制文件；不会再次启动录制' WHERE meeting_id=? AND status='failed' AND audio_ready=0 AND EXISTS(SELECT 1 FROM lark_cloud_recordings c WHERE c.meeting_id=recording_jobs.meeting_id)")
        .bind(id).execute(&mut *tx).await.map_err(db_error)?.rows_affected();
    if changed == 1 {
        sqlx::query("UPDATE lark_cloud_recordings SET next_attempt=0,deadline=? WHERE meeting_id=?")
            .bind(now()+7200).bind(id).execute(&mut *tx).await.map_err(db_error)?;
    }
    tx.commit().await.map_err(db_error)?;
    Ok(changed == 1)
}

pub async fn run<R: Runtime>(handle: AppHandle<R>, pool: SqlitePool) {
    if let Err(e) = recover(&pool).await { log::error!("{e}"); return; }
    loop {
        if let Err(e) = tick(&handle, &pool).await { log::error!("云录音队列：{e}"); }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}
async fn recover(pool: &SqlitePool) -> Result<(), String> {
    sqlx::query("UPDATE recording_jobs SET status=CASE WHEN status='cloud_starting' THEN 'failed' ELSE 'waiting_cloud' END,detail=CASE WHEN status='cloud_starting' THEN '启动云录音时应用退出，请先在 Lark 确认录制，再重试下载' ELSE '应用重启，继续等待或下载云录音' END WHERE status IN ('cloud_starting','downloading_cloud')")
        .execute(pool).await.map_err(db_error)?;
    Ok(())
}

async fn tick<R: Runtime>(handle: &AppHandle<R>, pool: &SqlitePool) -> Result<(), String> {
    let _storage = storage_migration::use_storage().await?;
    // Claim only a due task. Waiting for Lark never blocks the local transcription worker.
    let row = sqlx::query("UPDATE recording_jobs SET status='downloading_cloud',resume_stage='waiting_cloud' WHERE meeting_id=(SELECT c.meeting_id FROM lark_cloud_recordings c JOIN recording_jobs j ON j.meeting_id=c.meeting_id WHERE j.status='waiting_cloud' AND c.next_attempt<=? ORDER BY c.next_attempt,j.created_at LIMIT 1) RETURNING meeting_id,folder_path")
        .bind(now()).fetch_optional(pool).await.map_err(db_error)?;
    let Some(row) = row else { return Ok(()); };
    let id: String = row.get(0);
    let folder: String = row.get(1);
    let result = process(pool, &id, Path::new(&folder)).await;
    match result {
        Ok(true) => (), // Atomic completion already made the file available to the transcription worker.
        other => {
            let (status, detail) = match other {
                Ok(false) => ("waiting_cloud", "等待会议结束及 Lark 录制文件就绪".into()),
                Err(e) => (if e.retry { "waiting_cloud" } else { "failed" }, e.message),
                _ => unreachable!(),
            };
            // A pause must win even if a network failure or completion races it.
            sqlx::query("UPDATE recording_jobs SET status=CASE WHEN status='cancelling' THEN 'cancelled' WHEN status='pausing' THEN 'paused' ELSE ? END,detail=CASE WHEN status='cancelling' THEN '任务已取消，已有文件保留；不会停止 Lark 云录制' WHEN status='pausing' THEN '云录音任务已暂停，继续后重新检查或下载' ELSE ? END,updated_at=? WHERE meeting_id=? AND status IN ('downloading_cloud','pausing','cancelling')")
                .bind(status).bind(detail).bind(now()).bind(&id).execute(pool).await.map_err(db_error)?;
            sqlx::query("UPDATE lark_cloud_recordings SET next_attempt=? WHERE meeting_id=?")
                .bind(now()+60).bind(&id).execute(pool).await.map_err(db_error)?;
        }
    }
    let _ = handle.emit("recording-queue-changed", &id);
    Ok(())
}

async fn process(pool: &SqlitePool, id: &str, folder: &Path) -> Result<bool, Failure> {
    let row = sqlx::query("SELECT * FROM lark_cloud_recordings WHERE meeting_id=?").bind(id).fetch_one(pool).await.map_err(db_error)?;
    if now() > row.get::<i64,_>("deadline") { return Err(Failure::fatal("等待云录音超时。请确认会议已结束、曾开启录制且生成了文件，处理后点击重试")); }
    let c = oauth::authorized_config(pool).await?;
    if c.app_id != row.get::<String,_>("app_id") || c.owner_id != row.get::<String,_>("owner_id") {
        return Err(Failure::fatal("Lark 授权账号已改变，请恢复本场会议的原账号后重试"));
    }
    let remote: String = row.get("remote_id");
    if row.get::<Option<i64>,_>("ended_at").is_none() {
        let ended = if meeting_state::ended(pool, &c, &remote).await? { true } else {
            let data = request(reqwest::Method::GET, &format!("/open-apis/vc/v1/meetings/{remote}"), &c.user_token, None).await?;
            data["meeting"]["status"].as_i64() == Some(3)
        };
        if !ended { return Ok(false); }
        sqlx::query("UPDATE lark_cloud_recordings SET ended_at=?,deadline=? WHERE meeting_id=? AND ended_at IS NULL")
            .bind(now()).bind(now()+7200).bind(id).execute(pool).await.map_err(db_error)?;
    }
    // Do not use absence from the meeting as proof that the whole meeting ended.
    let recording = request(reqwest::Method::GET, &format!("/open-apis/vc/v1/meetings/{remote}/recording"), &c.user_token, None).await?;
    let token = minute_token(recording["recording"]["url"].as_str().ok_or_else(|| Failure::transient("Lark 录制链接尚未生成"))?)?;
    let media = request(reqwest::Method::GET, &format!("/open-apis/minutes/v1/minutes/{token}/media"), &c.user_token, None).await?;
    let url = media["download_url"].as_str().ok_or_else(|| Failure::transient("Lark 音视频下载地址尚未生成"))?;
    sqlx::query("UPDATE recording_jobs SET detail='正在下载 Lark 录音，完成后自动排队转写' WHERE meeting_id=? AND status='downloading_cloud'")
        .bind(id).execute(pool).await.map_err(db_error)?;
    download(pool, id, folder, url).await?;
    complete(pool, id, &folder.to_string_lossy()).await?;
    Ok(true)
}

async fn download(pool: &SqlitePool, id: &str, folder: &Path, raw: &str) -> Result<(), Failure> {
    let url = vendor_url(raw)?;
    let client = reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).timeout(Duration::from_secs(3600))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 || vendor_url(attempt.url().as_str()).is_err() { attempt.stop() } else { attempt.follow() }
        })).build().map_err(|_| Failure::fatal("无法创建录音下载连接"))?;
    // No bearer credential is sent to the signed CDN address, including redirects.
    let mut response = tokio::time::timeout(Duration::from_secs(30), client.get(url).send()).await
        .map_err(|_| Failure::transient("录音下载连接超时，稍后重试"))?
        .map_err(|_| Failure::transient("录音下载连接失败，稍后重新获取链接并重试"))?;
    if response.status() != reqwest::StatusCode::OK { return Err(Failure::transient(format!("录音下载 HTTP {}，稍后重新获取链接", response.status().as_u16()))); }
    let expected = response.content_length();
    let part = folder.join("audio.mp4.part");
    let mut file = tokio::fs::File::create(&part).await.map_err(|_| Failure::fatal("无法创建云录音文件，请检查录音目录权限和磁盘空间"))?;
    let mut size = 0u64;
    let mut prefix = Vec::new();
    loop {
        let status: String = sqlx::query_scalar("SELECT status FROM recording_jobs WHERE meeting_id=?").bind(id).fetch_one(pool).await.map_err(db_error)?;
        if status != "downloading_cloud" { return Err(Failure::transient("云录音任务已请求暂停")); }
        let chunk = tokio::time::timeout(Duration::from_secs(30), response.chunk()).await
            .map_err(|_| Failure::transient("录音下载长时间无响应，稍后重试"))?
            .map_err(|_| Failure::transient("录音下载中断，稍后重试；未完整文件不会转写"))?;
        let Some(chunk) = chunk else { break; };
        if prefix.len() < 12 { prefix.extend(chunk.iter().take(12-prefix.len()).copied()); }
        size += chunk.len() as u64;
        if size > 20*1024*1024*1024 { return Err(Failure::fatal("云录音超过 20 GB，请手动导出音频后导入")); }
        file.write_all(&chunk).await.map_err(|_| Failure::fatal("录音写入失败，请检查磁盘空间后重试"))?;
    }
    if !valid_media(&prefix, size, expected) { return Err(Failure::transient("下载文件不完整或不是 MP4/M4A 录音，稍后重新下载")); }
    file.sync_all().await.map_err(|_| Failure::fatal("无法保存云录音文件"))?;
    drop(file);
    tokio::fs::rename(part, folder.join("audio.mp4")).await.map_err(|_| Failure::fatal("云录音保存失败，请检查目录权限后重试"))?;
    Ok(())
}
fn valid_media(prefix: &[u8], size: u64, expected: Option<u64>) -> bool {
    size > 12 && prefix.get(4..8) == Some(b"ftyp") && expected.map_or(true, |n| n == size)
}

async fn complete(pool: &SqlitePool, id: &str, folder: &str) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(db_error)?;
    sqlx::query("INSERT OR IGNORE INTO lark_recording_links(folder_path,app_id,owner_id,remote_id,meeting_no,topic,started_at,stopped,auto_send,detail) SELECT j.folder_path,c.app_id,c.owner_id,c.remote_id,c.meeting_no,m.title,j.created_at,1,c.auto_send,'已下载云录音，后台排队转写' FROM lark_cloud_recordings c JOIN recording_jobs j ON j.meeting_id=c.meeting_id JOIN meetings m ON m.id=c.meeting_id WHERE c.meeting_id=?")
        .bind(id).execute(&mut *tx).await.map_err(db_error)?;
    sqlx::query("UPDATE recording_jobs SET audio_ready=1,resume_stage='queued',status=CASE WHEN status='cancelling' THEN 'cancelled' WHEN status='pausing' THEN 'paused' ELSE 'queued' END,detail=CASE WHEN status='cancelling' THEN '任务已取消，已下载录音保留' WHEN status='pausing' THEN '录音已下载，任务已暂停；继续后排队转写' ELSE '云录音已完整保存，等待后台转写' END,updated_at=? WHERE meeting_id=? AND status IN ('downloading_cloud','pausing','cancelling')")
        .bind(now()).bind(id).execute(&mut *tx).await.map_err(db_error)?;
    sqlx::query("UPDATE lark_auto_recordings SET status='completed',postprocessed=1,detail='云录音已下载，后台排队转写',updated_at=? WHERE folder_path=?")
        .bind(now()).bind(folder).execute(&mut *tx).await.map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("CREATE TABLE meetings(id TEXT PRIMARY KEY,title TEXT); INSERT INTO meetings VALUES('job','测试会议');").execute(&pool).await.unwrap();
        for migration in [
            include_str!("../../migrations/20261015040000_add_recording_jobs.sql"),
            include_str!("../../migrations/20261020000000_transcription_checkpoints.sql"),
            include_str!("../../migrations/20261015000000_add_lark_app.sql"),
            include_str!("../../migrations/20261015020000_add_lark_auto_recording.sql"),
            include_str!("../../migrations/20261021000000_lark_cloud_recording.sql"),
        ] { sqlx::raw_sql(migration).execute(&pool).await.unwrap(); }
        sqlx::raw_sql("INSERT INTO recording_jobs(meeting_id,folder_path,status,resume_stage,options_json,detail,created_at,updated_at,target_chat_id) VALUES('job','folder','waiting_cloud','waiting_cloud','{}','',1,1,'original-group'); INSERT INTO lark_cloud_recordings(meeting_id,app_id,owner_id,remote_id,meeting_no,auto_send,deadline) VALUES('job','app','owner','123','456',1,9999999999); INSERT INTO lark_auto_recordings(app_id,owner_id,remote_id,folder_path,status,detail,updated_at) VALUES('app','owner','123','folder','cloud_waiting','',1);")
            .execute(&pool).await.unwrap();
        pool
    }
    #[test]
    fn defaults_and_urls_are_strict() {
        let c: AppConfig = serde_json::from_str("{}").unwrap();
        assert!(c.recording_source == app::RecordingSource::Cloud);
        assert!(!c.auto_record);
        assert!(serde_json::from_str::<AppConfig>(r#"{"recording_source":"invalid"}"#).is_err());
        assert_eq!(minute_token("https://example.jp.larksuite.com/minutes/obj123?from=meeting").unwrap(), "obj123");
        for u in ["http://example.larksuite.com/minutes/obj123", "https://larksuite.com.evil.test/minutes/obj123", "https://user:secret@example.larksuite.com/minutes/obj123", "https://example.larksuite.com/minutes/a%2Fb", "https://127.0.0.1/minutes/obj123"] { assert!(minute_token(u).is_err()); }
    }
    #[test]
    fn partial_or_error_downloads_never_become_audio() {
        assert!(valid_media(b"\0\0\0\x20ftypisom", 1024, Some(1024)));
        assert!(!valid_media(b"\0\0\0\x20ftypisom", 100, Some(1024)));
        assert!(!valid_media(b"<html>Error!", 1024, Some(1024)));
        assert!(!valid_media(b"", 0, Some(0)));
        assert!(api_failure(124002).retry);
        assert!(!api_failure(2091005).retry);
        assert!(!api_failure(99991403).retry);
    }
    #[tokio::test]
    async fn restart_recovers_download_but_never_repeats_recording_start() {
        let pool = db().await;
        set_status(&pool, "job", "cloud_starting", "").await.unwrap();
        recover(&pool).await.unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM recording_jobs").fetch_one(&pool).await.unwrap();
        assert_eq!(status, "failed");
        assert!(retry(&pool, "job").await.unwrap());
        assert!(!retry(&pool, "job").await.unwrap());
        set_status(&pool, "job", "downloading_cloud", "").await.unwrap();
        recover(&pool).await.unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM recording_jobs").fetch_one(&pool).await.unwrap();
        assert_eq!(status, "waiting_cloud");
        set_status(&pool, "job", "paused", "").await.unwrap();
        recover(&pool).await.unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM recording_jobs").fetch_one(&pool).await.unwrap();
        assert_eq!(status, "paused");
    }
    #[tokio::test]
    async fn cancellation_wins_cloud_start_response_and_download_completion() {
        let pool = db().await;
        set_status(&pool, "job", "cancelling", "").await.unwrap();
        set_status(&pool, "job", "waiting_cloud", "late start response").await.unwrap();
        assert_eq!(sqlx::query_scalar::<_, String>("SELECT status FROM recording_jobs").fetch_one(&pool).await.unwrap(), "cancelled");
        assert!(!retry(&pool, "job").await.unwrap());
        recover(&pool).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_, String>("SELECT status FROM recording_jobs").fetch_one(&pool).await.unwrap(), "cancelled");
        sqlx::query("UPDATE recording_jobs SET status='cancelling'").execute(&pool).await.unwrap();
        complete(&pool, "job", "folder").await.unwrap();
        let row = sqlx::query("SELECT status,audio_ready FROM recording_jobs").fetch_one(&pool).await.unwrap();
        assert_eq!(row.get::<String,_>(0), "cancelled");
        assert!(row.get::<bool,_>(1));
    }

    #[tokio::test]
    async fn completion_preserves_original_group_and_pause_and_is_idempotent() {
        let pool = db().await;
        set_status(&pool, "job", "pausing", "").await.unwrap();
        complete(&pool, "job", "folder").await.unwrap();
        complete(&pool, "job", "folder").await.unwrap();
        let row = sqlx::query("SELECT status,resume_stage,audio_ready,target_chat_id FROM recording_jobs").fetch_one(&pool).await.unwrap();
        assert_eq!(row.get::<String,_>(0), "paused");
        assert_eq!(row.get::<String,_>(1), "queued");
        assert!(row.get::<bool,_>(2));
        assert_eq!(row.get::<String,_>(3), "original-group");
        let row = sqlx::query("SELECT COUNT(*),stopped,auto_send FROM lark_recording_links").fetch_one(&pool).await.unwrap();
        assert_eq!(row.get::<i64,_>(0), 1);
        assert_eq!(row.get::<i64,_>(1), 1);
        assert_eq!(row.get::<i64,_>(2), 1);
    }
}
