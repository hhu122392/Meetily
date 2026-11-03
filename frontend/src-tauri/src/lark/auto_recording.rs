use super::{
    app::{self, db_error, now, AppConfig},
    launches, meeting_state,
};
use crate::{audio::recording_commands as recording, state::AppState};
use serde_json::json;
use sqlx::SqlitePool;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Runtime};

static FRONTEND_READY: AtomicBool = AtomicBool::new(false);
pub(super) static POLICY_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tauri::command]
pub fn lark_auto_recording_ready(ready: bool) {
    FRONTEND_READY.store(ready, Ordering::SeqCst);
}

#[tauri::command]
pub async fn lark_auto_recording_finish(
    state: tauri::State<'_, AppState>,
    folder: String,
    saved: bool,
) -> Result<(), String> {
    finish(state.db_manager.pool(), &folder, saved).await
}

async fn finish(pool: &SqlitePool, folder: &str, saved: bool) -> Result<(), String> {
    sqlx::query("UPDATE lark_auto_recordings SET postprocessed=?,status=?,detail=?,updated_at=? WHERE folder_path=? AND status!='manual'")
        .bind(saved).bind(if saved {"completed"} else {"postprocess_failed"})
        .bind(if saved {"录音已保存，自动总结及发群按现有设置继续。"} else {"录音后处理未成功，已暂停后续自动录音；请先检查保存状态。"})
        .bind(now()).bind(folder).execute(pool).await.map_err(db_error)?;
    Ok(())
}

struct Owned {
    app_id: String,
    owner_id: String,
    id: String,
    folder: String,
    absent: u8,
    observation: u64,
}

// A single missing observation may be a brief reconnect; different meetings/errors stop immediately.
fn stop_reason(
    enabled: bool,
    active: Result<Option<&str>, ()>,
    id: &str,
    absent: &mut u8,
) -> Option<&'static str> {
    if !enabled {
        return Some("自动录音或企业连接已关闭，停止本次自动录音。");
    }
    match active {
        Ok(Some(current)) if current == id => {
            *absent = 0;
            None
        }
        Ok(None) => {
            *absent += 1;
            (*absent >= 2).then_some("已确认离开 Lark 会议，自动停止录音。")
        }
        Ok(Some(_)) => Some("检测到切换会议，停止原会议录音；请核对纪要。"),
        Err(()) => Some("无法确认参会状态，自动停止录音；请检查连接或重新授权。"),
    }
}

async fn update(
    pool: &SqlitePool,
    c: &AppConfig,
    id: &str,
    status: &str,
    detail: &str,
    folder: Option<&str>,
) -> Result<(), String> {
    sqlx::query("UPDATE lark_auto_recordings SET status=CASE WHEN postprocessed=1 THEN status ELSE ? END,detail=CASE WHEN postprocessed=1 THEN detail ELSE ? END,folder_path=COALESCE(?,folder_path),updated_at=? WHERE app_id=? AND owner_id=? AND remote_id=?")
        .bind(status).bind(detail).bind(folder).bind(now()).bind(&c.app_id).bind(&c.owner_id).bind(id).execute(pool).await.map_err(db_error)?;
    Ok(())
}

fn notice<R: Runtime>(handle: &AppHandle<R>, message: &str, error: bool) {
    let _ = handle.emit(
        "lark-auto-recording",
        json!({"message":message,"error":error}),
    );
}

async fn has_pending_launch(pool: &SqlitePool, c: &AppConfig) -> Result<bool, String> {
    let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM lark_meeting_launches j WHERE j.app_id=? AND j.owner_id=? AND j.created_at>=? AND j.expires_at>? AND j.reserve_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM lark_app_routes r JOIN lark_auto_recordings a ON a.app_id=r.app_id AND a.owner_id=r.owner_id AND a.remote_id=r.remote_id WHERE r.app_id=j.app_id AND r.owner_id=j.owner_id AND r.reference_kind='reserve' AND r.reference=j.reserve_id) AND NOT EXISTS (SELECT 1 FROM lark_meeting_signals s WHERE s.app_id=j.app_id AND s.owner_id=j.owner_id AND s.meeting_no=j.meeting_no AND s.ended=1 AND s.event_at>=j.created_at*1000)")
        .bind(&c.app_id).bind(&c.owner_id).bind(c.auto_record_since).bind(now()).fetch_one(pool).await.map_err(db_error)?;
    Ok(count > 0)
}

async fn claim(pool: &SqlitePool, c: &AppConfig, id: &str) -> Result<bool, String> {
    if meeting_state::ended(pool, c, id).await? { return Ok(false); }
    // Only an app-created reservation from after opt-in may trigger automatic capture.
    let chats:Vec<String>=sqlx::query_scalar("SELECT DISTINCT r.chat_id FROM lark_app_routes r JOIN lark_meeting_launches j ON j.reserve_id=r.reference AND j.app_id=r.app_id AND j.owner_id=r.owner_id AND j.chat_id=r.chat_id WHERE r.app_id=? AND r.owner_id=? AND r.reference_kind='reserve' AND r.remote_id=? AND r.expires_at>? AND j.created_at>=? AND j.expires_at>?")
        .bind(&c.app_id).bind(&c.owner_id).bind(id).bind(now()).bind(c.auto_record_since).bind(now()).fetch_all(pool).await.map_err(db_error)?;
    if chats.len() != 1 {
        return Ok(false);
    }
    let all_chats:i64=sqlx::query_scalar("SELECT COUNT(DISTINCT chat_id) FROM lark_app_routes WHERE app_id=? AND owner_id=? AND remote_id=? AND expires_at>?")
        .bind(&c.app_id).bind(&c.owner_id).bind(id).bind(now()).fetch_one(pool).await.map_err(db_error)?;
    if all_chats != 1 {
        return Ok(false);
    }
    Ok(sqlx::query("INSERT OR IGNORE INTO lark_auto_recordings(app_id,owner_id,remote_id,status,detail,updated_at) VALUES(?,?,?,'starting','已识别本人加入机器人创建的会议，正在启动录音。',?)")
        .bind(&c.app_id).bind(&c.owner_id).bind(id).bind(now()).execute(pool).await.map_err(db_error)?.rows_affected()==1)
}

pub async fn run<R: Runtime>(handle: AppHandle<R>, pool: SqlitePool) {
    // Never resume an interrupted microphone session, or retry a meeting already handled in this app.
    if sqlx::query("UPDATE lark_auto_recordings SET status='interrupted',detail='应用曾退出，本场不再自动启动；请检查已保存录音。',postprocessed=1 WHERE status IN ('starting','recording','processing','postprocess_failed')")
        .execute(&pool).await.is_err() { return; }
    let mut owned: Option<Owned> = None;
    let mut cooldown = 0;
    let mut last_error = String::new();
    // This local timer checks ownership/settings only; current() shares a 60s API cache.
    let mut timer = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        tokio::select! { _=timer.tick()=>{}, _=meeting_state::CHANGED.notified()=>{} }
        if let Err(e) = tick(&handle, &pool, &mut owned, &mut cooldown).await {
            if last_error != e {
                notice(&handle, &e, true);
                last_error = e.clone();
            }
            app::status(e).await;
        } else {
            last_error.clear();
        }
    }
}

async fn tick<R: Runtime>(
    handle: &AppHandle<R>,
    pool: &SqlitePool,
    owned: &mut Option<Owned>,
    cooldown: &mut i64,
) -> Result<(), String> {
    let config = app::load(pool).await;
    if let Some(session) = owned.as_mut() {
        if recording::is_stopping() {
            return Ok(());
        }
        let identity = AppConfig {
            app_id: session.app_id.clone(),
            owner_id: session.owner_id.clone(),
            ..Default::default()
        };
        if !recording::is_recording().await
            || recording::get_meeting_folder_path().await?.as_deref()
                != Some(session.folder.as_str())
        {
            update(
                pool,
                &identity,
                &session.id,
                "processing",
                "录音已手动停止，本场不会再次自动启动。",
                None,
            )
            .await?;
            *owned = None;
            *cooldown = now() + 15;
            return Ok(());
        }
        let enabled = config.as_ref().is_ok_and(|c| {
            c.enabled
                && c.auto_record
                && c.app_id == session.app_id
                && c.owner_id == session.owner_id
        }) && FRONTEND_READY.load(Ordering::SeqCst);
        let reason = if !enabled {
            stop_reason(false, Ok(None), &session.id, &mut session.absent)
        } else if meeting_state::ended(pool, &identity, &session.id).await? {
            Some("已收到 Lark 会议结束通知，自动停止录音。")
        } else {
            match meeting_state::current(pool).await {
                Ok((_, active, sequence)) => {
                    // Reusing a cached empty result is not a second leave confirmation.
                    if sequence == session.observation { return Ok(()); }
                    session.observation = sequence;
                    let reason = stop_reason(true, Ok(active.as_ref().map(|m| m.0.as_str())), &session.id, &mut session.absent);
                    if active.is_none() && reason.is_none() { meeting_state::confirm_absence().await; }
                    reason
                }
                Err(_) => stop_reason(true, Err(()), &session.id, &mut session.absent),
            }
        };
        if let Some(reason) = reason {
            let folder = session.folder.clone();
            update(pool, &identity, &session.id, "processing", reason, None).await?;
            let result = recording::stop_automatic(handle.clone(), &folder).await;
            *owned = None;
            *cooldown = now() + 15;
            match result {
                Ok(_) => notice(handle, reason, false),
                Err(e) => {
                    finish(pool, &folder, false).await?;
                    return Err(format!("自动停止录音失败，请手动停止并检查保存状态：{e}"));
                }
            }
        }
        return Ok(());
    }
    let c = config?;
    if !c.enabled
        || !c.auto_record
        || c.auto_record_since == 0
        || !FRONTEND_READY.load(Ordering::SeqCst)
        || now() < *cooldown
        || (c.recording_source == app::RecordingSource::Local && (recording::is_recording().await || recording::is_stopping()))
    {
        return Ok(());
    }
    if !has_pending_launch(pool, &c).await? {
        return Ok(());
    }
    let (c, active, observation) = meeting_state::current(pool).await?;
    let Some((id, no, topic)) = active else {
        return Ok(());
    };
    launches::resolve_current(pool, &c, &id, &no).await?;
    // Recheck opt-in after network calls; no stale configuration may start the microphone.
    let _policy = POLICY_LOCK.lock().await;
    let latest = app::load(pool).await?;
    if !latest.enabled
        || !latest.auto_record
        || latest.app_id != c.app_id
        || latest.owner_id != c.owner_id
        || latest.auto_record_since != c.auto_record_since
        || latest.recording_source != c.recording_source
        || !FRONTEND_READY.load(Ordering::SeqCst)
    {
        return Ok(());
    }
    if !claim(pool, &c, &id).await? {
        return Ok(());
    }
    if c.recording_source == app::RecordingSource::Cloud {
        let result = super::cloud_recording::start(handle, pool, &c, &id, &no, &topic).await;
        if let Err(ref e) = result {
            update(pool, &c, &id, "cloud_failed", e, None).await?;
        }
        return result;
    }
    match recording::start_automatic(
        handle.clone(),
        super::routing::meeting_title(&topic, &no).into(),
    )
    .await
    {
        Ok(folder) => {
            // Retain ownership before any fallible DB work, so errors cannot leave capture unattended.
            *owned = Some(Owned {
                app_id: c.app_id.clone(),
                owner_id: c.owner_id.clone(),
                id: id.clone(),
                folder: folder.clone(),
                absent: 0,
                observation,
            });
            update(
                pool,
                &c,
                &id,
                "recording",
                "已自动开始录音；离会后自动停止，手动停止后本场不再重启。",
                Some(&folder),
            )
            .await?;
            let linked:i64=sqlx::query_scalar("SELECT COUNT(*) FROM lark_recording_links WHERE folder_path=? AND remote_id=? AND app_id=? AND owner_id=? AND blocked=0")
                .bind(&folder).bind(&id).bind(&c.app_id).bind(&c.owner_id).fetch_one(pool).await.map_err(db_error)?;
            if linked == 0 {
                update(
                    pool,
                    &c,
                    &id,
                    "processing",
                    "启动时会议关联发生变化，已停止录音，请核对。",
                    None,
                )
                .await?;
                recording::stop_automatic(handle.clone(), &folder).await?;
                *owned = None;
                *cooldown = now() + 15;
                return Err(
                    "自动录音启动时未能确认同一会议，已停止；请检查授权和会议状态。".into(),
                );
            }
            notice(handle, "已加入 Lark 会议，Meetily 已自动开始录音。", false);
        }
        Err(e) => {
            update(
                pool,
                &c,
                &id,
                "failed",
                &format!("自动录音未启动：{e} 本场不会自动重试，可手动录音。"),
                None,
            )
            .await?;
            return Err(e);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn leave_requires_confirmation_but_switch_errors_and_disable_stop() {
        let mut missing = 0;
        assert!(stop_reason(true, Ok(None), "1", &mut missing).is_none());
        assert!(stop_reason(true, Ok(Some("1")), "1", &mut missing).is_none());
        assert_eq!(missing, 0);
        assert!(stop_reason(true, Ok(None), "1", &mut missing).is_none());
        assert!(stop_reason(true, Ok(None), "1", &mut missing).is_some());
        assert!(stop_reason(true, Ok(Some("2")), "1", &mut missing).is_some());
        assert!(stop_reason(true, Err(()), "1", &mut missing).is_some());
        assert!(stop_reason(false, Ok(Some("1")), "1", &mut missing).is_some());
    }
    #[tokio::test]
    async fn only_new_verified_reservations_can_claim_once_and_completion_is_folder_scoped() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        for migration in [
            include_str!("../../migrations/20261015000000_add_lark_app.sql"),
            include_str!("../../migrations/20261015010000_add_lark_launches.sql"),
            include_str!("../../migrations/20261015020000_add_lark_auto_recording.sql"),
            include_str!("../../migrations/20261016000000_add_lark_meeting_signals.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await.unwrap();
        }
        let c = AppConfig {
            app_id: "app".into(),
            owner_id: "owner".into(),
            auto_record_since: now() - 10,
            ..Default::default()
        };
        assert!(!claim(&pool, &c, "1").await.unwrap());
        sqlx::query("INSERT INTO lark_meeting_launches(request_id,app_id,owner_id,message_id,chat_id,topic,reserve_id,created_at,expires_at) VALUES('j','app','owner','msg','oc_a','test','reserve',?,?)")
            .bind(now()-20).bind(now()+1000).execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO lark_app_routes VALUES('app','owner','reserve','reserve','oc_a','1',?)",
        )
        .bind(now() + 1000)
        .execute(&pool)
        .await
        .unwrap();
        assert!(!claim(&pool, &c, "1").await.unwrap());
        sqlx::query("UPDATE lark_meeting_launches SET created_at=?")
            .bind(now())
            .execute(&pool)
            .await
            .unwrap();
        let other_owner = AppConfig {
            owner_id: "other".into(),
            ..c.clone()
        };
        assert!(!claim(&pool, &other_owner, "1").await.unwrap());
        sqlx::query(
            "INSERT INTO lark_app_routes VALUES('app','owner','1','id','oc_conflict','1',?)",
        )
        .bind(now() + 1000)
        .execute(&pool)
        .await
        .unwrap();
        assert!(!claim(&pool, &c, "1").await.unwrap());
        sqlx::query("DELETE FROM lark_app_routes WHERE chat_id='oc_conflict'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE lark_app_routes SET expires_at=0")
            .execute(&pool)
            .await
            .unwrap();
        assert!(!claim(&pool, &c, "1").await.unwrap());
        sqlx::query("UPDATE lark_app_routes SET expires_at=?")
            .bind(now() + 1000)
            .execute(&pool)
            .await
            .unwrap();
        assert!(claim(&pool, &c, "1").await.unwrap());
        assert!(!claim(&pool, &c, "1").await.unwrap());
        update(&pool, &c, "1", "processing", "stopped", Some("folder-a"))
            .await
            .unwrap();
        finish(&pool, "folder-b", true).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT postprocessed FROM lark_auto_recordings")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        finish(&pool, "folder-a", true).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT postprocessed FROM lark_auto_recordings")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        assert!(!claim(&pool, &c, "1").await.unwrap());
    }
    #[tokio::test]
    async fn manually_recorded_meetings_are_never_reclaimed_or_finished_by_automation() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        for migration in [
            include_str!("../../migrations/20261015000000_add_lark_app.sql"),
            include_str!("../../migrations/20261015010000_add_lark_launches.sql"),
            include_str!("../../migrations/20261015020000_add_lark_auto_recording.sql"),
            include_str!("../../migrations/20261016000000_add_lark_meeting_signals.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO lark_meeting_launches(request_id,app_id,owner_id,message_id,chat_id,topic,reserve_id,created_at,expires_at) VALUES('j','app','owner','msg','oc_a','test','reserve',?,?)")
            .bind(now()).bind(now()+1000).execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO lark_app_routes VALUES('app','owner','reserve','reserve','oc_a','1',?)",
        )
        .bind(now() + 1000)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO lark_auto_recordings(app_id,owner_id,remote_id,folder_path,status,postprocessed,updated_at) VALUES('app','owner','1','manual-folder','manual',1,?)")
            .bind(now()).execute(&pool).await.unwrap();
        let c = AppConfig {
            app_id: "app".into(),
            owner_id: "owner".into(),
            auto_record_since: now() - 10,
            ..Default::default()
        };
        assert!(!claim(&pool, &c, "1").await.unwrap());
        finish(&pool, "manual-folder", false).await.unwrap();
        assert!(!has_pending_launch(&pool, &c).await.unwrap());
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM lark_auto_recordings")
                .fetch_one(&pool)
                .await
                .unwrap(),
            "manual"
        );
    }

    #[tokio::test]
    async fn ended_and_processed_reservations_do_not_poll_but_new_ones_do() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        for migration in [
            include_str!("../../migrations/20261015000000_add_lark_app.sql"),
            include_str!("../../migrations/20261015010000_add_lark_launches.sql"),
            include_str!("../../migrations/20261015020000_add_lark_auto_recording.sql"),
            include_str!("../../migrations/20261016000000_add_lark_meeting_signals.sql"),
        ] { sqlx::raw_sql(migration).execute(&pool).await.unwrap(); }
        let c = AppConfig { app_id:"app".into(), owner_id:"owner".into(), auto_record_since:now()-100, ..Default::default() };
        sqlx::query("INSERT INTO lark_meeting_launches(request_id,app_id,owner_id,message_id,chat_id,topic,reserve_id,meeting_no,created_at,expires_at) VALUES('j','app','owner','msg','oc_a','test','reserve','123456789',?,?)")
            .bind(now()-5).bind(now()+1000).execute(&pool).await.unwrap();
        assert!(has_pending_launch(&pool,&c).await.unwrap());
        sqlx::query("INSERT INTO lark_app_routes VALUES('app','owner','reserve','reserve','oc_a','1',?)").bind(now()+1000).execute(&pool).await.unwrap();
        for status in ["recording","completed","failed","interrupted","manual"] {
            sqlx::query("INSERT OR REPLACE INTO lark_auto_recordings(app_id,owner_id,remote_id,status,updated_at) VALUES('app','owner','1',?,?)").bind(status).bind(now()).execute(&pool).await.unwrap();
            assert!(!has_pending_launch(&pool,&c).await.unwrap(), "status {status}");
        }
        sqlx::query("DELETE FROM lark_auto_recordings").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO lark_meeting_signals VALUES('app','owner','1','123456789','vc.meeting.meeting_ended_v1',?,1)").bind(now()*1000).execute(&pool).await.unwrap();
        assert!(!has_pending_launch(&pool,&c).await.unwrap());
        assert!(!claim(&pool,&c,"1").await.unwrap());
        sqlx::query("UPDATE lark_meeting_launches SET created_at=?").bind(now()+1).execute(&pool).await.unwrap();
        assert!(has_pending_launch(&pool,&c).await.unwrap(), "an old ended instance cannot retire a new reservation");
    }
}
