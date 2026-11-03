use crate::{database::repositories::setting::SettingsRepository, state::AppState};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use tauri::{AppHandle, Emitter, Manager, Runtime};

#[derive(Serialize, Deserialize)]
struct Options {
    provider: String,
    model: String,
    language: Option<String>,
    summary_provider: String,
    summary_model: String,
    #[serde(default)]
    transcription_only: bool,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct JobView {
    meeting_id: String,
    title: String,
    status: String,
    detail: String,
    progress: Option<i64>,
}

fn db_error(e: sqlx::Error) -> String { format!("无法保存或读取录音队列：{e}") }

// Persist identity and model choices while the capture still owns its folder.
pub async fn register<R: Runtime>(app: &AppHandle<R>, folder: &str, title: &str) -> Result<String, String> {
    let state = app.state::<AppState>();
    let pool = state.db_manager.pool();
    let transcript = SettingsRepository::get_transcript_config(pool).await.map_err(db_error)?;
    let summary = SettingsRepository::get_model_config(pool).await.map_err(db_error)?;
    let options = Options {
        provider: transcript.as_ref().map(|c| c.provider.clone()).unwrap_or("sensevoice".into()),
        model: transcript.map(|c| c.model).unwrap_or(crate::config::DEFAULT_SENSEVOICE_MODEL.into()),
        language: crate::get_language_preference_internal(),
        summary_provider: summary.as_ref().map(|c| c.provider.clone()).unwrap_or_default(),
        summary_model: summary.map(|c| c.model).unwrap_or_default(),
        transcription_only: false,
    };
    insert_capture(pool, folder, title, &serde_json::to_string(&options).map_err(|e| e.to_string())?).await
}

async fn insert_capture(pool: &SqlitePool, folder: &str, title: &str, options: &str) -> Result<String, String> {
    let id = format!("meeting-{}", uuid::Uuid::new_v4());
    let now = chrono::Utc::now();
    let mut tx = pool.begin().await.map_err(db_error)?;
    sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at,folder_path) VALUES(?,?,?,?,?)")
        .bind(&id).bind(title).bind(now.to_rfc3339()).bind(now.to_rfc3339()).bind(folder)
        .execute(&mut *tx).await.map_err(db_error)?;
    sqlx::query("INSERT INTO recording_jobs(meeting_id,folder_path,status,options_json,detail,created_at,updated_at) VALUES(?,?,'capturing',?,'正在录音，结束后排队转写',?,?)")
        .bind(&id).bind(folder).bind(options).bind(now.timestamp()).bind(now.timestamp()).execute(&mut *tx).await.map_err(db_error)?;
    sqlx::query("INSERT INTO recording_transcription_checks(folder_path,status,detail,updated_at) VALUES(?,'processing','录音尚未完成转写',?)")
        .bind(folder).bind(now.timestamp()).execute(&mut *tx).await.map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    Ok(id)
}

pub async fn capture_saved(pool: &SqlitePool, folder: &str, error: Option<&str>) -> Result<String, String> {
    let id: String = sqlx::query_scalar("SELECT meeting_id FROM recording_jobs WHERE folder_path=?")
        .bind(folder).fetch_one(pool).await.map_err(db_error)?;
    // Resolve while the route is still valid. Waiting behind another recording must
    // not expire this identity or make a later binding redirect an older meeting.
    let chats: Vec<String> = sqlx::query_scalar("SELECT DISTINCT r.chat_id FROM lark_app_routes r JOIN lark_recording_links l ON r.app_id=l.app_id AND r.owner_id=l.owner_id WHERE l.folder_path=? AND l.stopped=1 AND l.blocked=0 AND l.remote_id IS NOT NULL AND r.expires_at>? AND (r.remote_id=l.remote_id OR (r.remote_id IS NULL AND r.reference_kind='number' AND r.reference=l.meeting_no))")
        .bind(folder).bind(chrono::Utc::now().timestamp()).fetch_all(pool).await.map_err(db_error)?;
    if chats.len() == 1 {
        sqlx::query("UPDATE recording_jobs SET target_chat_id=? WHERE meeting_id=?").bind(&chats[0]).bind(&id).execute(pool).await.map_err(db_error)?;
    }
    sqlx::query("UPDATE recording_jobs SET audio_ready=? WHERE meeting_id=?").bind(error.is_none()).bind(&id).execute(pool).await.map_err(db_error)?;
    set_status(pool, &id, if error.is_some() { "failed" } else { "queued" }, error.unwrap_or("音频已保存，等待转写；可以开始下一场录音")).await?;
    sqlx::query("UPDATE lark_auto_recordings SET status='completed',postprocessed=1,detail=?,updated_at=? WHERE folder_path=? AND status!='manual'")
        .bind(if error.is_some() { "录音保存失败，请检查原音频" } else { "音频已保存，后台排队处理" })
        .bind(chrono::Utc::now().timestamp()).bind(folder).execute(pool).await.map_err(db_error)?;
    Ok(id)
}

async fn set_status(pool: &SqlitePool, id: &str, status: &str, detail: &str) -> Result<(), String> {
    sqlx::query("UPDATE recording_jobs SET status=?,detail=?,updated_at=? WHERE meeting_id=?")
        .bind(status).bind(detail).bind(chrono::Utc::now().timestamp()).bind(id).execute(pool).await.map_err(db_error)?;
    Ok(())
}

pub(super) async fn register_import(pool: &SqlitePool, folder: &str, title: &str, language: Option<String>, model: Option<String>, provider: Option<String>) -> Result<String, String> {
    let saved = SettingsRepository::get_transcript_config(pool).await.map_err(db_error)?;
    let provider = provider.or_else(|| saved.as_ref().map(|s| s.provider.clone())).unwrap_or("sensevoice".into());
    if !matches!(provider.as_str(), "localWhisper" | "whisper" | "parakeet" | "sensevoice") { return Err("导入队列支持本地 SenseVoice、Whisper 或 Parakeet 模型。".into()); }
    let model = model.or_else(|| saved.filter(|s| s.provider == provider || (matches!(s.provider.as_str(), "localWhisper" | "whisper") && matches!(provider.as_str(), "localWhisper" | "whisper"))).map(|s| s.model))
        .unwrap_or_else(|| if provider == "sensevoice" {crate::config::DEFAULT_SENSEVOICE_MODEL.into()} else if provider == "parakeet" {crate::config::DEFAULT_PARAKEET_MODEL.into()} else {crate::config::DEFAULT_WHISPER_MODEL.into()});
    if model.trim().is_empty() { return Err("请选择转写模型。".into()); }
    let options = Options {provider, model, language, summary_provider:String::new(), summary_model:String::new(), transcription_only:true};
    let id = insert_capture(pool, folder, title, &serde_json::to_string(&options).map_err(|e| e.to_string())?).await?;
    set_status(pool, &id, "importing", "正在复制导入音频，完成后自动排队转写").await?;
    Ok(id)
}

pub(super) async fn import_saved(pool: &SqlitePool, id: &str, error: Option<&str>) -> Result<(), String> {
    sqlx::query("UPDATE recording_jobs SET status=CASE WHEN status='cancelling' THEN 'cancelled' ELSE ? END,resume_stage='queued',audio_ready=?,detail=CASE WHEN status='cancelling' THEN '任务已取消，原音频保留；可重新导入' ELSE ? END,updated_at=? WHERE meeting_id=? AND status IN ('importing','cancelling')")
        .bind(if error.is_none() {"queued"} else {"failed"}).bind(error.is_none())
        .bind(error.unwrap_or("音频已导入，等待后台转写；可继续导入其他音频"))
        .bind(chrono::Utc::now().timestamp()).bind(id).execute(pool).await.map_err(db_error)?;
    Ok(())
}

pub async fn enqueue_retranscription<R: Runtime>(
    app: &AppHandle<R>, id: &str, language: Option<String>,
    model: Option<String>, provider: Option<String>,
) -> Result<(), String> {
    let _storage = super::storage_migration::use_storage().await?;
    let state = app.state::<AppState>();
    let pool = state.db_manager.pool();
    let config = SettingsRepository::get_transcript_config(pool).await.map_err(db_error)?;
    let provider = provider.or_else(|| config.as_ref().map(|c| c.provider.clone())).unwrap_or("sensevoice".into());
    let model = model.or_else(|| config.filter(|c| c.provider == provider).map(|c| c.model))
        .unwrap_or_else(|| match provider.as_str() {
            "sensevoice" => crate::config::DEFAULT_SENSEVOICE_MODEL.into(),
            "parakeet" => crate::config::DEFAULT_PARAKEET_MODEL.into(),
            _ => crate::config::DEFAULT_WHISPER_MODEL.into(),
        });
    let options = Options {
        provider,
        model,
        language,
        summary_provider: String::new(),
        summary_model: String::new(),
        transcription_only: true,
    };
    if !matches!(options.provider.as_str(), "localWhisper" | "whisper" | "parakeet" | "sensevoice") || options.model.trim().is_empty() {
        return Err("请选择已下载的本地 SenseVoice、Whisper 或 Parakeet 模型。".into());
    }
    enqueue_existing(pool, id, &serde_json::to_string(&options).map_err(|e| e.to_string())?).await?;
    let _ = app.emit("recording-queue-changed", id);
    Ok(())
}

async fn enqueue_existing(pool: &SqlitePool, id: &str, options: &str) -> Result<(), String> {
    // Resolve the folder from this company's database, never from a stale UI path.
    let folder: Option<String> = sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id=?")
        .bind(id).fetch_optional(pool).await.map_err(db_error)?.flatten();
    let folder = folder.filter(|s| !s.is_empty()).ok_or("当前公司中找不到这场会议的录音目录。")?;
    super::retranscription::find_audio_file(std::path::Path::new(&folder))
        .map_err(|_| "找不到原始录音，未加入转写队列。")?;
    let now = chrono::Utc::now().timestamp();
    // One statement makes duplicate submissions and worker claims mutually exclusive.
    let mut tx = pool.begin().await.map_err(db_error)?;
    let changed = sqlx::query(
        "INSERT INTO recording_jobs(meeting_id,folder_path,status,resume_stage,options_json,audio_ready,detail,created_at,updated_at)
         SELECT ?,?,'queued','queued',?,1,'已加入后台转写队列；完成后可按需重新生成纪要',?,?
         WHERE NOT EXISTS(SELECT 1 FROM summary_processes WHERE meeting_id=? AND lower(status) IN ('pending','processing','summarizing'))
         ON CONFLICT(meeting_id) DO UPDATE SET folder_path=excluded.folder_path,status='queued',resume_stage='queued',
         options_json=excluded.options_json,audio_ready=1,detail=excluded.detail,created_at=excluded.created_at,updated_at=excluded.updated_at
         WHERE recording_jobs.status IN ('completed','failed','cancelled')")
        .bind(id).bind(&folder).bind(options).bind(now).bind(now).bind(id)
        .execute(&mut *tx).await.map_err(db_error)?.rows_affected();
    if changed == 0 { return Err("这场会议已有排队、转写或纪要任务，请等待完成后再提交。".into()); }
    sqlx::query("DELETE FROM transcription_checkpoints WHERE meeting_id=?")
        .bind(id).execute(&mut *tx).await.map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    Ok(())
}

async fn recover(pool: &SqlitePool) -> Result<(), String> {
    sqlx::query("UPDATE recording_jobs SET status='cancelled',detail='任务已取消，已有录音和结果保留' WHERE status='cancelling'")
        .execute(pool).await.map_err(db_error)?;
    sqlx::query("UPDATE recording_jobs SET status='paused',detail='任务已暂停；点击继续后恢复处理' WHERE status='pausing'")
        .execute(pool).await.map_err(db_error)?;
    // Capture never restarts by itself. Sealed audio and persisted jobs can resume.
    sqlx::query("UPDATE recording_jobs SET status=CASE WHEN status='transcribing' THEN 'queued' WHEN status='summarizing' THEN 'summary_pending' ELSE 'failed' END,detail='应用曾退出：已保存任务将继续处理；未完成保存的录音需检查原音频' WHERE status IN ('capturing','importing','transcribing','summarizing')")
        .execute(pool).await.map_err(db_error)?;
    Ok(())
}

async fn claim(pool: &SqlitePool) -> Result<Option<(String, String, String, String)>, String> {
    // Atomic claim prevents repeated processing even if another caller is added later.
    let row = sqlx::query("UPDATE recording_jobs SET resume_stage=status,status=CASE WHEN status='queued' THEN 'transcribing' ELSE 'summarizing' END,detail=CASE WHEN status='queued' THEN '正在后台转写，可同时录制下一场' ELSE '正在生成纪要' END WHERE meeting_id=(SELECT meeting_id FROM recording_jobs WHERE status IN ('queued','summary_pending') ORDER BY created_at,rowid LIMIT 1) RETURNING meeting_id,folder_path,options_json,resume_stage")
        .fetch_optional(pool).await.map_err(db_error)?;
    Ok(row.map(|r| (r.get(0),r.get(1),r.get(2),r.get(3))))
}

pub async fn run<R: Runtime>(app: AppHandle<R>, pool: SqlitePool) {
    if let Err(e) = recover(&pool).await { log::error!("{e}"); return; }
    loop {
        match claim(&pool).await {
            Ok(Some((id, folder, options, stage))) => {
                let _ = app.emit("recording-queue-changed", &id);
                let result = process(&app, &pool, &id, &folder, &options, &stage).await;
                if let Err(e) = finish_job(&pool, &id, result).await { log::error!("{e}"); }
                let _ = app.emit("recording-queue-changed", &id);
            }
            Ok(None) => (),
            Err(e) => log::error!("{e}"),
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

async fn process<R: Runtime>(app: &AppHandle<R>, pool: &SqlitePool, id: &str, folder: &str, options: &str, stage: &str) -> Result<&'static str, String> {
    check_pause(pool, id).await.map_err(|e| e.to_string())?;
    let o: Options = serde_json::from_str(options).map_err(|_| "录音任务配置损坏")?;
    if stage == "queued" {
        if !matches!(o.provider.as_str(), "localWhisper" | "whisper" | "parakeet" | "sensevoice") {
            return Err("音频已保存；后台队列目前支持本地 Whisper 或 Parakeet，请用会议详情重新转写。".into());
        }
        super::retranscription::start_retranscription(app.clone(),id.into(),folder.into(),o.language,Some(o.model),Some(o.provider))
            .await.map_err(|e| format!("转写未完成，音频已保留：{e}"))?;
        if o.transcription_only {
            return Ok("录音转写已完成；可在会议详情生成或更新纪要");
        }
        // Move the resume point even if pause arrived while final transcripts were saving.
        sqlx::query("UPDATE recording_jobs SET resume_stage='summary_pending' WHERE meeting_id=?")
            .bind(id).execute(pool).await.map_err(db_error)?;
        let changed = sqlx::query("UPDATE recording_jobs SET status='summarizing',detail='正在生成纪要' WHERE meeting_id=? AND status='transcribing'")
            .bind(id).execute(pool).await.map_err(db_error)?.rows_affected();
        if changed == 0 { return Err("任务已请求暂停".into()); }
    }
    // A crash after summary completion must not regenerate or resend its message.
    let completed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM summary_processes WHERE meeting_id=? AND lower(status)='completed'")
        .bind(id).fetch_one(pool).await.map_err(db_error)?;
    if completed == 0 {
        let text: Vec<String> = sqlx::query_scalar("SELECT transcript FROM transcripts WHERE meeting_id=? ORDER BY audio_start_time,rowid")
            .bind(id).fetch_all(pool).await.map_err(db_error)?;
        let text = text.join("\n");
        if text.trim().is_empty() { return Err("未检测到可总结的讲话，已保留录音。".into()); }
        if o.summary_provider.is_empty() || o.summary_model.is_empty() { return Err("尚未配置总结模型，转写结果已保存。".into()); }
        let language = crate::summary::metadata::read_summary_language_from_metadata(std::path::Path::new(folder)).ok().flatten();
        let state = app.state::<AppState>();
        check_pause(pool, id).await.map_err(|e| e.to_string())?;
        let summary = crate::summary::commands::api_process_transcript(app.clone(),state,text,o.summary_provider,o.summary_model,Some(id.into()),None,None,None,None,language,None).await?;
        let started_at = chrono::DateTime::parse_from_rfc3339(&summary.process_id).map_err(|e| e.to_string())?.with_timezone(&chrono::Utc);
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if cancel_requested(pool, id).await? {
                crate::summary::service::SummaryService::cancel_summary(id, started_at);
            }
            let row = sqlx::query("SELECT status,error FROM summary_processes WHERE meeting_id=?").bind(id).fetch_one(pool).await.map_err(db_error)?;
            match row.get::<String,_>("status").to_lowercase().as_str() {
                "completed" => break,
                "failed" | "error" | "cancelled" => return Err(row.get::<Option<String>,_>("error").unwrap_or("纪要生成未完成，可重试".into())),
                _ => (),
            }
        }
    }
    check_pause(pool, id).await.map_err(|e| e.to_string())?;
    // Reuses durable per-meeting delivery identity and original-group validation.
    crate::lark::app::summary_completed(pool, id).await?;
    Ok("转写和纪要已完成；请在会议详情审核后确认发群")
}

#[derive(Debug, thiserror::Error)]
#[error("任务已请求暂停")]
pub(super) struct PauseRequested;

#[derive(Debug, thiserror::Error)]
#[error("任务已取消，已有录音和结果保留")]
pub(super) struct CancelRequested;

pub(crate) async fn cancel_requested(pool: &SqlitePool, id: &str) -> Result<bool, String> {
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM recording_jobs WHERE meeting_id=?")
        .bind(id).fetch_optional(pool).await.map_err(db_error)?;
    Ok(matches!(status.as_deref(), Some("cancelling" | "cancelled")))
}

pub(super) async fn check_pause(pool: &SqlitePool, id: &str) -> anyhow::Result<()> {
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM recording_jobs WHERE meeting_id=?")
        .bind(id).fetch_optional(pool).await?;
    if matches!(status.as_deref(), Some("pausing" | "paused")) { return Err(PauseRequested.into()); }
    if matches!(status.as_deref(), Some("cancelling" | "cancelled")) { return Err(CancelRequested.into()); }
    Ok(())
}

async fn finish_job(pool: &SqlitePool, id: &str, result: Result<&str, String>) -> Result<(), String> {
    let (status, detail) = match result {
        Ok(detail) => ("completed", detail.to_string()),
        Err(e) => ("failed", e),
    };
    // Only acknowledge pause after inference and engine cleanup have actually returned.
    sqlx::query("UPDATE recording_jobs SET status=CASE WHEN status IN ('cancelling','cancelled') THEN 'cancelled' WHEN status='pausing' AND ?!='completed' THEN 'paused' ELSE ? END,detail=CASE WHEN status IN ('cancelling','cancelled') THEN '任务已取消，已有录音和结果保留；可在会议详情重新转写' WHEN status='pausing' AND ?!='completed' THEN '任务已暂停，已保存分段进度；可切换公司，点击继续后恢复' ELSE ? END,updated_at=? WHERE meeting_id=?")
        .bind(status).bind(status).bind(status).bind(detail).bind(chrono::Utc::now().timestamp()).bind(id)
        .execute(pool).await.map_err(db_error)?;
    Ok(())
}

async fn pause_job(pool: &SqlitePool, id: &str) -> Result<(), String> {
    let changed = sqlx::query("UPDATE recording_jobs SET resume_stage=CASE WHEN status IN ('queued','summary_pending','waiting_cloud') THEN status ELSE resume_stage END,status=CASE WHEN status IN ('transcribing','downloading_cloud') THEN 'pausing' ELSE 'paused' END,detail=CASE WHEN status IN ('transcribing','downloading_cloud') THEN '正在暂停：等待当前处理步骤结束并保存进度' ELSE '任务已暂停；点击继续后恢复处理' END,updated_at=? WHERE meeting_id=? AND status IN ('queued','summary_pending','transcribing','waiting_cloud','downloading_cloud')")
        .bind(chrono::Utc::now().timestamp()).bind(id).execute(pool).await.map_err(db_error)?.rows_affected();
    if changed == 0 { return Err("任务状态已变化；录音和正在生成的纪要不能暂停，请刷新后查看。".into()); }
    Ok(())
}

async fn cancel_job(pool: &SqlitePool, id: &str) -> Result<(), String> {
    let changed = sqlx::query("UPDATE recording_jobs SET status=CASE WHEN status IN ('importing','transcribing','summarizing','cloud_starting','downloading_cloud','pausing','cancelling') THEN 'cancelling' ELSE 'cancelled' END,detail=CASE WHEN status IN ('importing','transcribing','summarizing','cloud_starting','downloading_cloud','pausing','cancelling') THEN '正在取消，等待当前处理步骤退出；已有录音和结果保留' ELSE '任务已取消，已有录音和结果保留' END,updated_at=? WHERE meeting_id=? AND status IN ('importing','queued','transcribing','summary_pending','summarizing','cloud_starting','waiting_cloud','downloading_cloud','pausing','paused','failed','cancelling','cancelled')")
        .bind(chrono::Utc::now().timestamp()).bind(id).execute(pool).await.map_err(db_error)?.rows_affected();
    if changed == 0 { return Err("任务状态已变化；正在录音请先停止录音，已完成的任务无需取消。".into()); }
    Ok(())
}

#[tauri::command]
pub async fn cancel_recording_job(app: AppHandle, state: tauri::State<'_, AppState>, meeting_id: String) -> Result<(), String> {
    let _storage = super::storage_migration::use_storage().await?;
    cancel_job(state.db_manager.pool(), &meeting_id).await?;
    let _ = app.emit("recording-queue-changed", &meeting_id);
    Ok(())
}

async fn resume_job(pool: &SqlitePool, id: &str) -> Result<(), String> {
    let changed = sqlx::query("UPDATE recording_jobs SET status=resume_stage,detail='已恢复，等待后台处理',updated_at=? WHERE meeting_id=? AND status='paused' AND resume_stage IN ('queued','summary_pending','waiting_cloud')")
        .bind(chrono::Utc::now().timestamp()).bind(id).execute(pool).await.map_err(db_error)?.rows_affected();
    if changed == 0 { return Err("该任务尚未暂停或已恢复，请等待状态更新。".into()); }
    Ok(())
}

#[tauri::command]
pub async fn pause_recording_job(app: AppHandle, state: tauri::State<'_, AppState>, meeting_id: String) -> Result<(), String> {
    let _storage = super::storage_migration::use_storage().await?;
    pause_job(state.db_manager.pool(), &meeting_id).await?;
    let _ = app.emit("recording-queue-changed", &meeting_id);
    Ok(())
}

#[tauri::command]
pub async fn resume_recording_job(app: AppHandle, state: tauri::State<'_, AppState>, meeting_id: String) -> Result<(), String> {
    let _storage = super::storage_migration::use_storage().await?;
    resume_job(state.db_manager.pool(), &meeting_id).await?;
    let _ = app.emit("recording-queue-changed", &meeting_id);
    Ok(())
}

#[tauri::command]
pub async fn get_recording_jobs(state: tauri::State<'_, AppState>) -> Result<Vec<JobView>, String> {
    sqlx::query_as("SELECT j.meeting_id,m.title,j.status,j.detail,(SELECT 25 + next_segment * 55 / MAX(total_segments,1) FROM transcription_checkpoints c WHERE c.meeting_id=j.meeting_id) AS progress FROM recording_jobs j JOIN meetings m ON m.id=j.meeting_id ORDER BY CASE WHEN j.status IN ('completed','cancelled') THEN 1 ELSE 0 END,j.created_at DESC LIMIT 30")
        .fetch_all(state.db_manager.pool()).await.map_err(db_error)
}

#[tauri::command]
pub async fn retry_recording_job(state: tauri::State<'_, AppState>, meeting_id: String) -> Result<(), String> {
    let _storage = super::storage_migration::use_storage().await?;
    let row = sqlx::query("SELECT folder_path,audio_ready FROM recording_jobs WHERE meeting_id=? AND status='failed'").bind(&meeting_id)
        .fetch_optional(state.db_manager.pool()).await.map_err(db_error)?.ok_or("该任务无需重试")?;
    let folder: String = row.get(0);
    if !row.get::<bool,_>("audio_ready") {
        if crate::lark::cloud_recording::retry(state.db_manager.pool(), &meeting_id).await? { return Ok(()); }
        return Err("录音未完整保存，不能直接重试；请先检查或恢复原音频。".into());
    }
    super::retranscription::find_audio_file(std::path::Path::new(&folder))
        .map_err(|_| "完整音频尚未保存，请先恢复原音频。")?;
    sqlx::query("UPDATE recording_jobs SET status=resume_stage,detail='等待重试' WHERE meeting_id=? AND status='failed'").bind(meeting_id)
        .execute(state.db_manager.pool()).await.map_err(db_error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("CREATE TABLE meetings(id TEXT PRIMARY KEY,title TEXT,created_at TEXT,updated_at TEXT,folder_path TEXT);").execute(&pool).await.unwrap();
        sqlx::raw_sql("CREATE TABLE summary_processes(meeting_id TEXT PRIMARY KEY,status TEXT); CREATE TABLE transcripts(meeting_id TEXT,transcript TEXT);").execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261015030000_add_transcription_checks.sql")).execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261015040000_add_recording_jobs.sql")).execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261020000000_transcription_checkpoints.sql")).execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261015000000_add_lark_app.sql")).execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261015020000_add_lark_auto_recording.sql")).execute(&pool).await.unwrap();
        pool
    }
    #[tokio::test]
    async fn cancellation_is_durable_scoped_and_wins_worker_completion() {
        let pool = db().await;
        let a = insert_capture(&pool, "a", "取消目标", "{}").await.unwrap();
        let b = insert_capture(&pool, "b", "另一个任务", "{}").await.unwrap();
        for status in ["queued", "summary_pending", "waiting_cloud", "paused", "failed"] {
            set_status(&pool, &a, status, "").await.unwrap();
            cancel_job(&pool, &a).await.unwrap();
            cancel_job(&pool, &a).await.unwrap();
            recover(&pool).await.unwrap();
            assert_eq!(sqlx::query_scalar::<_, String>("SELECT status FROM recording_jobs WHERE meeting_id=?").bind(&a).fetch_one(&pool).await.unwrap(), "cancelled");
            assert!(claim(&pool).await.unwrap().is_none());
            assert!(resume_job(&pool, &a).await.is_err());
        }
        set_status(&pool, &b, "queued", "").await.unwrap();
        for status in ["transcribing", "summarizing", "importing", "cloud_starting", "downloading_cloud", "pausing"] {
            set_status(&pool, &a, status, "").await.unwrap();
            cancel_job(&pool, &a).await.unwrap();
            assert_eq!(sqlx::query_scalar::<_, String>("SELECT status FROM recording_jobs WHERE meeting_id=?").bind(&a).fetch_one(&pool).await.unwrap(), "cancelling");
            assert!(check_pause(&pool, &a).await.unwrap_err().is::<CancelRequested>());
            check_pause(&pool, &b).await.unwrap();
            finish_job(&pool, &a, Ok("worker completed at the same time")).await.unwrap();
            assert_eq!(sqlx::query_scalar::<_, String>("SELECT status FROM recording_jobs WHERE meeting_id=?").bind(&a).fetch_one(&pool).await.unwrap(), "cancelled");
        }
        assert_eq!(claim(&pool).await.unwrap().unwrap().0, b);
        set_status(&pool, &a, "cancelling", "").await.unwrap();
        recover(&pool).await.unwrap();
        assert!(cancel_requested(&pool, &a).await.unwrap());
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM meetings").fetch_one(&pool).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn import_completion_cannot_requeue_a_cancelled_copy() {
        let pool = db().await;
        let id = insert_capture(&pool, "a", "导入", "{}").await.unwrap();
        set_status(&pool, &id, "importing", "").await.unwrap();
        cancel_job(&pool, &id).await.unwrap();
        import_saved(&pool, &id, None).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_, String>("SELECT status FROM recording_jobs").fetch_one(&pool).await.unwrap(), "cancelled");
        assert!(claim(&pool).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn imported_audio_uses_selected_models_and_is_claimed_only_after_copy() {
        let pool = db().await;
        sqlx::raw_sql("CREATE TABLE transcript_settings(id TEXT,provider TEXT,model TEXT,whisperApiKey TEXT,deepgramApiKey TEXT,elevenLabsApiKey TEXT,groqApiKey TEXT,openaiApiKey TEXT); INSERT INTO transcript_settings(id,provider,model) VALUES('1','localWhisper','small');").execute(&pool).await.unwrap();
        let id = register_import(&pool, "import-a", "第一份导入", Some("zh".into()), Some("large-v3-turbo".into()), Some("whisper".into())).await.unwrap();
        let second = register_import(&pool, "import-b", "第二份导入", None, None, None).await.unwrap();
        assert!(claim(&pool).await.unwrap().is_none());
        import_saved(&pool, &id, None).await.unwrap();
        let claimed = claim(&pool).await.unwrap().unwrap();
        assert_eq!(claimed.0, id);
        let options: Options = serde_json::from_str(&claimed.2).unwrap();
        assert_eq!(options.model, "large-v3-turbo");
        assert_eq!(options.language.as_deref(), Some("zh"));
        assert!(options.transcription_only);
        assert!(claim(&pool).await.unwrap().is_none());
        import_saved(&pool, &second, Some("复制中断")).await.unwrap();
        let row = sqlx::query("SELECT status,audio_ready,target_chat_id FROM recording_jobs WHERE meeting_id=?").bind(&second).fetch_one(&pool).await.unwrap();
        assert_eq!(row.get::<String,_>(0), "failed");
        assert!(!row.get::<bool,_>(1));
        assert!(row.get::<Option<String>,_>(2).is_none());
        let third = register_import(&pool, "import-c", "复制时重启", None, None, None).await.unwrap();
        recover(&pool).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT status FROM recording_jobs WHERE meeting_id=?").bind(third).fetch_one(&pool).await.unwrap(), "failed");
    }
    #[tokio::test]
    async fn cloud_waiting_can_pause_and_never_enters_transcription_until_sealed() {
        let pool = db().await;
        let id = insert_capture(&pool, "cloud", "云录音", "{}").await.unwrap();
        set_status(&pool, &id, "waiting_cloud", "").await.unwrap();
        assert!(claim(&pool).await.unwrap().is_none());
        pause_job(&pool, &id).await.unwrap();
        recover(&pool).await.unwrap();
        resume_job(&pool, &id).await.unwrap();
        let row = sqlx::query("SELECT status,audio_ready FROM recording_jobs WHERE meeting_id=?").bind(&id).fetch_one(&pool).await.unwrap();
        assert_eq!(row.get::<String,_>(0), "waiting_cloud");
        assert!(!row.get::<bool,_>(1));
        assert!(claim(&pool).await.unwrap().is_none());
        set_status(&pool, &id, "downloading_cloud", "").await.unwrap();
        pause_job(&pool, &id).await.unwrap();
        assert!(resume_job(&pool, &id).await.is_err());
    }
    #[tokio::test]
    async fn paused_jobs_survive_restart_and_only_resume_explicitly() {
        let pool = db().await;
        let a = insert_capture(&pool, "a", "等待转写", "{}").await.unwrap();
        let b = insert_capture(&pool, "b", "等待总结", "{}").await.unwrap();
        set_status(&pool, &a, "queued", "").await.unwrap();
        set_status(&pool, &b, "summary_pending", "").await.unwrap();
        pause_job(&pool, &a).await.unwrap();
        pause_job(&pool, &b).await.unwrap();
        recover(&pool).await.unwrap();
        assert!(claim(&pool).await.unwrap().is_none());
        assert!(pause_job(&pool, &a).await.is_err());
        resume_job(&pool, &b).await.unwrap();
        assert_eq!(claim(&pool).await.unwrap().unwrap().3, "summary_pending");
        assert!(resume_job(&pool, &b).await.is_err());
        resume_job(&pool, &a).await.unwrap();
        assert_eq!(claim(&pool).await.unwrap().unwrap().3, "queued");
    }

    #[tokio::test]
    async fn active_pause_is_acknowledged_after_worker_cleanup_and_does_not_cancel_other_jobs() {
        let pool = db().await;
        let a = insert_capture(&pool, "a", "活动转写", "{}").await.unwrap();
        let b = insert_capture(&pool, "b", "录音", "{}").await.unwrap();
        set_status(&pool, &a, "queued", "").await.unwrap();
        claim(&pool).await.unwrap().unwrap();
        pause_job(&pool, &a).await.unwrap();
        assert!(check_pause(&pool, &a).await.unwrap_err().is::<PauseRequested>());
        check_pause(&pool, &b).await.unwrap();
        assert!(pause_job(&pool, &b).await.is_err());
        assert!(resume_job(&pool, &a).await.is_err());
        let status: String = sqlx::query_scalar("SELECT status FROM recording_jobs WHERE meeting_id=?")
            .bind(&a).fetch_one(&pool).await.unwrap();
        assert_eq!(status, "pausing");
        finish_job(&pool, &a, Err("任务已请求暂停".into())).await.unwrap();
        assert!(claim(&pool).await.unwrap().is_none());
        resume_job(&pool, &a).await.unwrap();
        claim(&pool).await.unwrap().unwrap();
        pause_job(&pool, &a).await.unwrap();
        recover(&pool).await.unwrap();
        assert!(claim(&pool).await.unwrap().is_none());
        resume_job(&pool, &a).await.unwrap();
    }

    #[tokio::test]
    async fn pause_at_completion_preserves_completed_work_and_summary_resume_point() {
        let pool = db().await;
        let a = insert_capture(&pool, "a", "转写", "{}").await.unwrap();
        set_status(&pool, &a, "transcribing", "").await.unwrap();
        pause_job(&pool, &a).await.unwrap();
        finish_job(&pool, &a, Ok("完成")).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_, String>("SELECT status FROM recording_jobs WHERE meeting_id=?")
            .bind(&a).fetch_one(&pool).await.unwrap(), "completed");
        set_status(&pool, &a, "transcribing", "").await.unwrap();
        pause_job(&pool, &a).await.unwrap();
        sqlx::query("UPDATE recording_jobs SET resume_stage='summary_pending' WHERE meeting_id=?")
            .bind(&a).execute(&pool).await.unwrap();
        finish_job(&pool, &a, Err("暂停".into())).await.unwrap();
        resume_job(&pool, &a).await.unwrap();
        assert_eq!(claim(&pool).await.unwrap().unwrap().3, "summary_pending");
        assert!(pause_job(&pool, &a).await.is_err());
    }
    #[tokio::test]
    async fn manual_queue_preserves_results_rejects_duplicates_and_recovers_in_order() {
        let pool = db().await;
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        for (id, dir) in [("first", first.path()), ("second", second.path())] {
            std::fs::write(dir.join("audio.wav"), b"saved audio").unwrap();
            sqlx::query("INSERT INTO meetings(id,folder_path) VALUES(?,?)")
                .bind(id).bind(dir.to_string_lossy().as_ref()).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO transcripts VALUES(?,'原有转写')").bind(id).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO summary_processes VALUES('first','completed')").execute(&pool).await.unwrap();
        let options = r#"{"provider":"localWhisper","model":"small","language":"zh","summary_provider":"","summary_model":"","transcription_only":true}"#;
        enqueue_existing(&pool, "first", options).await.unwrap();
        assert!(enqueue_existing(&pool, "first", "{}").await.is_err());
        enqueue_existing(&pool, "second", options).await.unwrap();
        let first_claim = claim(&pool).await.unwrap().unwrap();
        assert_eq!(first_claim.0, "first");
        assert!(serde_json::from_str::<Options>(&first_claim.2).unwrap().transcription_only);
        assert!(enqueue_existing(&pool, "first", "{}").await.is_err());
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT transcript FROM transcripts WHERE meeting_id='first'").fetch_one(&pool).await.unwrap(), "原有转写");
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT status FROM summary_processes WHERE meeting_id='first'").fetch_one(&pool).await.unwrap(), "completed");
        recover(&pool).await.unwrap();
        assert_eq!(claim(&pool).await.unwrap().unwrap().0, "first");
        assert_eq!(claim(&pool).await.unwrap().unwrap().0, "second");
        assert!(claim(&pool).await.unwrap().is_none());
        set_status(&pool, "first", "failed", "test").await.unwrap();
        enqueue_existing(&pool, "first", options).await.unwrap();
        assert_eq!(claim(&pool).await.unwrap().unwrap().0, "first");
    }

    #[tokio::test]
    async fn manual_queue_rejects_missing_audio_unknown_meeting_and_active_summary() {
        let pool = db().await;
        let dir = tempfile::tempdir().unwrap();
        sqlx::query("INSERT INTO meetings(id,folder_path) VALUES('meeting',?)")
            .bind(dir.path().to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        assert!(enqueue_existing(&pool, "other-company", "{}").await.is_err());
        assert!(enqueue_existing(&pool, "meeting", "{}").await.is_err());
        std::fs::create_dir(dir.path().join("audio.mp4")).unwrap();
        assert!(enqueue_existing(&pool, "meeting", "{}").await.is_err());
        std::fs::write(dir.path().join("audio.m4a"), b"saved audio").unwrap();
        sqlx::query("INSERT INTO summary_processes VALUES('meeting','PROCESSING')").execute(&pool).await.unwrap();
        assert!(enqueue_existing(&pool, "meeting", "{}").await.is_err());
        assert!(claim(&pool).await.unwrap().is_none());
        sqlx::query("UPDATE summary_processes SET status='completed'").execute(&pool).await.unwrap();
        enqueue_existing(&pool, "meeting", "{}").await.unwrap();
        assert_eq!(claim(&pool).await.unwrap().unwrap().0, "meeting");
    }

    #[test]
    fn legacy_recordings_still_generate_summaries() {
        let options: Options = serde_json::from_str(r#"{"provider":"localWhisper","model":"small","language":null,"summary_provider":"builtin-ai","summary_model":"local"}"#).unwrap();
        assert!(!options.transcription_only);
    }
    #[tokio::test]
    async fn second_capture_is_independent_of_first_transcription_and_claims_are_unique() {
        let pool = db().await;
        let first = insert_capture(&pool,"first-folder","第一场","{}").await.unwrap();
        set_status(&pool,&first,"queued","").await.unwrap();
        let claimed = claim(&pool).await.unwrap().unwrap();
        let second = insert_capture(&pool,"second-folder","第二场","{}").await.unwrap();
        assert_eq!(claimed.0,first); assert_eq!(claimed.1,"first-folder");
        assert!(claim(&pool).await.unwrap().is_none());
        set_status(&pool,&second,"queued","").await.unwrap();
        let claimed2 = claim(&pool).await.unwrap().unwrap();
        assert_eq!(claimed2.0,second); assert_eq!(claimed2.1,"second-folder");
        assert!(claim(&pool).await.unwrap().is_none());
    }
    #[tokio::test]
    async fn recovery_resumes_saved_jobs_but_never_reopens_microphone_or_retranscribes_summary() {
        let pool = db().await;
        let a = insert_capture(&pool,"a","录音中","{}").await.unwrap();
        let b = insert_capture(&pool,"b","转写中","{}").await.unwrap();
        let c = insert_capture(&pool,"c","总结中","{}").await.unwrap();
        set_status(&pool,&b,"transcribing","").await.unwrap();
        set_status(&pool,&c,"summarizing","").await.unwrap();
        recover(&pool).await.unwrap();
        let statuses: Vec<(String,String)> = sqlx::query_as("SELECT meeting_id,status FROM recording_jobs").fetch_all(&pool).await.unwrap();
        assert!(statuses.contains(&(a,"failed".into())));
        assert!(statuses.contains(&(b,"queued".into())));
        assert!(statuses.contains(&(c,"summary_pending".into())));
    }
    #[tokio::test]
    async fn capture_freezes_each_group_before_expiry_and_never_releases_incomplete_audio() {
        let pool = db().await;
        let a = insert_capture(&pool,"a","甲群会议","{}").await.unwrap();
        let b = insert_capture(&pool,"b","乙群会议","{}").await.unwrap();
        for (folder, remote, chat) in [("a","100","chat-a"),("b","200","chat-b")] {
            sqlx::query("INSERT INTO lark_recording_links(folder_path,app_id,owner_id,started_at,remote_id,stopped,blocked,auto_send) VALUES(?,'app','owner',0,?,1,0,1)")
                .bind(folder).bind(remote).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO lark_app_routes(app_id,owner_id,reference,reference_kind,chat_id,remote_id,expires_at) VALUES('app','owner',?,'id',?,?,?)")
                .bind(remote).bind(chat).bind(remote).bind(chrono::Utc::now().timestamp()+100).execute(&pool).await.unwrap();
        }
        capture_saved(&pool,"a",None).await.unwrap();
        capture_saved(&pool,"b",Some("音频保存失败")).await.unwrap();
        sqlx::query("UPDATE lark_app_routes SET expires_at=0,chat_id='changed'").execute(&pool).await.unwrap();
        let stored: String = sqlx::query_scalar("SELECT target_chat_id FROM recording_jobs WHERE meeting_id=?").bind(&a).fetch_one(&pool).await.unwrap();
        assert_eq!(stored,"chat-a");
        assert_eq!(claim(&pool).await.unwrap().unwrap().0,a);
        assert!(claim(&pool).await.unwrap().is_none());
        let status: String = sqlx::query_scalar("SELECT status FROM recording_jobs WHERE meeting_id=?").bind(b).fetch_one(&pool).await.unwrap();
        assert_eq!(status,"failed");
    }
}

