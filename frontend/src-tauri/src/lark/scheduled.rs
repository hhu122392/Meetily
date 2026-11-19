use super::{
    app::{self, db_error, now, AppConfig, RecordingSource},
    invitations::Invitation,
    payload::fingerprint,
};
use serde_json::{json, Value};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

pub(super) async fn enqueue(
    tx: &mut Transaction<'_, Sqlite>,
    c: &AppConfig,
    chat: &str,
    message: &str,
    at: i64,
    parsed: Result<Invitation, String>,
) -> Result<(), String> {
    let i = match parsed {
        Ok(i) => i,
        Err(detail) => {
            sqlx::query("INSERT OR IGNORE INTO lark_invitations(id,app_id,owner_id,chat_id,source_key,invitation,topic,starts_at,ends_at,command_at,status,detail) VALUES(?,?,?,?,?,'{}','无法识别的邀请',0,0,?,'invalid',?)")
                .bind(fingerprint(&[&c.app_id,message])).bind(&c.app_id).bind(&c.owner_id).bind(chat).bind(message).bind(at).bind(detail)
                .execute(&mut **tx).await.map_err(db_error)?;
            return Ok(());
        }
    };
    let source = i.source_key();
    let id = fingerprint(&[&c.app_id, &c.owner_id, chat, &source]);
    let bound = (!c.manual_mode
        && c.auto_record
        && c.recording_source == RecordingSource::Local
        && at / 1000 >= c.auto_record_since)
        .then_some(at / 1000);
    let serialized = serde_json::to_string(&i).map_err(|_| "邀请保存失败")?;
    let old = sqlx::query(
        "SELECT invitation,command_at,status,route_armed,record_bound_at FROM lark_invitations WHERE id=?",
    )
    .bind(&id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(db_error)?;
    if let Some(old) = old {
        let status: String = old.get("status");
        if old.get::<i64, _>("command_at") >= at
            || matches!(
                status.as_str(),
                "sent" | "sending" | "unknown" | "cancelled" | "duplicate"
            )
        {
            return Ok(());
        }
        if old.get::<String, _>("invitation") == serialized
            && matches!(status.as_str(), "ready" | "queued")
        {
            let old_bound: Option<i64> = old.get("record_bound_at");
            if bound.is_some() && old_bound.is_none_or(|v| v < c.auto_record_since) {
                sqlx::query("UPDATE lark_invitations SET command_at=?,record_bound_at=?,route_armed=0,next_attempt=0 WHERE id=?")
                    .bind(at).bind(bound).bind(&id).execute(&mut **tx).await.map_err(db_error)?;
            } else {
                sqlx::query("UPDATE lark_invitations SET command_at=? WHERE id=?")
                    .bind(at)
                    .bind(&id)
                    .execute(&mut **tx)
                    .await
                    .map_err(db_error)?;
            }
            return Ok(());
        }
        // A changed appointment must not leave its old recording window armed.
        sqlx::query("DELETE FROM lark_app_routes WHERE invitation_id=? AND remote_id IS NULL")
            .bind(&id)
            .execute(&mut **tx)
            .await
            .map_err(db_error)?;
        sqlx::query("UPDATE lark_invitations SET route_armed=0,event_id=NULL WHERE id=?")
            .bind(&id)
            .execute(&mut **tx)
            .await
            .map_err(db_error)?;
    }
    // Newer copies replace only unsent reminders. Redelivery and old/out-of-order messages cannot rewind them.
    sqlx::query("INSERT INTO lark_invitations(id,app_id,owner_id,chat_id,source_key,invitation,topic,starts_at,ends_at,meeting_no,meeting_url,command_at,record_bound_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(app_id,owner_id,chat_id,source_key) DO UPDATE SET invitation=excluded.invitation,topic=excluded.topic,starts_at=excluded.starts_at,ends_at=excluded.ends_at,meeting_no=excluded.meeting_no,meeting_url=excluded.meeting_url,command_at=excluded.command_at,record_bound_at=excluded.record_bound_at,status='queued',detail='收到更新邀请，等待核对',attempts=0,next_attempt=0 WHERE excluded.command_at>lark_invitations.command_at AND lark_invitations.status NOT IN ('sent','sending','unknown','cancelled','duplicate')")
        .bind(id).bind(&c.app_id).bind(&c.owner_id).bind(chat).bind(source).bind(serialized).bind(&i.title).bind(i.start).bind(i.end).bind(&i.number).bind(&i.meeting_url).bind(at).bind(bound)
        .execute(&mut **tx).await.map_err(db_error)?;
    Ok(())
}

pub(super) async fn recover(pool: &SqlitePool) -> Result<(), String> {
    sqlx::query("UPDATE lark_invitations SET status='unknown',detail='上次发送中断，送达结果不确定；请核对群消息，不会自动重发。' WHERE status='sending'").execute(pool).await.map_err(db_error)?;
    Ok(())
}

async fn state(pool: &SqlitePool, id: &str, status: &str, detail: &str) -> Result<(), String> {
    if matches!(status, "expired" | "cancelled" | "duplicate") {
        sqlx::query("DELETE FROM lark_app_routes WHERE invitation_id=? AND remote_id IS NULL")
            .bind(id)
            .execute(pool)
            .await
            .map_err(db_error)?;
    }
    sqlx::query("UPDATE lark_invitations SET status=?,detail=? WHERE id=?")
        .bind(status)
        .bind(detail)
        .bind(id)
        .execute(pool)
        .await
        .map_err(db_error)?;
    Ok(())
}

trait ScheduleApi {
    async fn resolve(
        &self,
        pool: &SqlitePool,
        i: &Invitation,
        event: Option<&str>,
    ) -> Result<(Invitation, Option<String>), String>;
    async fn send(
        &self,
        c: &AppConfig,
        chat: &str,
        id: &str,
        text: &str,
    ) -> Result<(), (&'static str, String)>;
}
struct Production;
impl ScheduleApi for Production {
    async fn resolve(
        &self,
        pool: &SqlitePool,
        i: &Invitation,
        event: Option<&str>,
    ) -> Result<(Invitation, Option<String>), String> {
        if i.share_url.is_none() {
            return Ok((i.clone(), None));
        }
        let c = super::oauth::authorized_config_locked(pool).await?;
        let m = super::calendar::resolve_invitation(&c, i, event).await?;
        let (number, url) = m
            .meeting_url
            .as_deref()
            .and_then(super::invitations::meeting_link)
            .ok_or(
                "日程尚未提供有效的 Lark 会议号，请在 Lark 中添加视频会议或发送视频会议邀请。",
            )?;
        Ok((
            Invitation {
                title: m.title,
                start: m.start,
                end: m.end,
                number: Some(number),
                meeting_url: Some(url),
                share_url: i.share_url.clone(),
            },
            Some(m.event_id),
        ))
    }
    async fn send(
        &self,
        c: &AppConfig,
        chat: &str,
        id: &str,
        text: &str,
    ) -> Result<(), (&'static str, String)> {
        let token = app::tenant_token(c).await.map_err(|e| ("failed", e))?;
        let uuid = fingerprint(&["scheduled-invitation", id]);
        let v=super::launches::mutation("/open-apis/im/v1/messages?receive_id_type=chat_id",&token,
            json!({"receive_id":chat,"msg_type":"text","content":json!({"text":text}).to_string(),"uuid":&uuid[..32]})).await?;
        if v["message_id"].as_str().is_none_or(str::is_empty) {
            return Err(("unknown", "提醒送达结果不确定，请核对群消息。".into()));
        }
        Ok(())
    }
}

pub(super) async fn process_one(pool: &SqlitePool) -> Result<(), String> {
    // The worker already holds CONFIG_LOCK; share the route lock with incoming messages and cancellation.
    let _routing = app::DELIVERY_LOCK.lock().await;
    process_with(pool, &Production, now()).await
}

async fn process_with(pool: &SqlitePool, api: &impl ScheduleApi, clock: i64) -> Result<(), String> {
    let c = app::load(pool).await?;
    if !c.enabled || c.manual_mode || c.owner_id.is_empty() {
        return Ok(());
    }
    let row=sqlx::query("SELECT * FROM lark_invitations WHERE app_id=? AND owner_id=? AND status IN ('queued','ready') AND next_attempt<=? ORDER BY next_attempt,starts_at LIMIT 1")
        .bind(&c.app_id).bind(&c.owner_id).bind(clock).fetch_optional(pool).await.map_err(db_error)?;
    let Some(row) = row else {
        return Ok(());
    };
    let id: String = row.get("id");
    let mut i: Invitation = serde_json::from_str(&row.get::<String, _>("invitation"))
        .map_err(|_| "预约邀请数据损坏")?;
    if i.end <= clock || clock > i.start + 600 {
        return state(
            pool,
            &id,
            "expired",
            "已错过开始时间十分钟，不再补发过期提醒。请重新发送新的邀请。",
        )
        .await;
    }
    let old_state: String = row.get("status");
    let needs_resolution = old_state == "queued" || i.share_url.is_some();
    if needs_resolution {
        let event: Option<String> = row.get("event_id");
        match api.resolve(pool, &i, event.as_deref()).await {
            Ok((fresh, event)) => {
                if fresh.start != i.start || fresh.number != i.number {
                    sqlx::query(
                        "DELETE FROM lark_app_routes WHERE invitation_id=? AND remote_id IS NULL",
                    )
                    .bind(&id)
                    .execute(pool)
                    .await
                    .map_err(db_error)?;
                    sqlx::query("UPDATE lark_invitations SET route_armed=0 WHERE id=?")
                        .bind(&id)
                        .execute(pool)
                        .await
                        .map_err(db_error)?;
                }
                i = fresh;
                if i.end <= clock || clock > i.start + 600 {
                    return state(
                        pool,
                        &id,
                        "expired",
                        "日程已经结束或错过提醒时间，不再补发。",
                    )
                    .await;
                }
                sqlx::query("UPDATE lark_invitations SET invitation=?,topic=?,starts_at=?,ends_at=?,meeting_no=?,meeting_url=?,event_id=?,status='ready',detail='邀请已核对，等待预约时间',attempts=0 WHERE id=?")
                    .bind(serde_json::to_string(&i).unwrap()).bind(&i.title).bind(i.start).bind(i.end).bind(&i.number).bind(&i.meeting_url).bind(event).bind(&id).execute(pool).await.map_err(db_error)?;
            }
            Err(e) => {
                sqlx::query(
                    "DELETE FROM lark_app_routes WHERE invitation_id=? AND remote_id IS NULL",
                )
                .bind(&id)
                .execute(pool)
                .await
                .map_err(db_error)?;
                sqlx::query("UPDATE lark_invitations SET route_armed=0 WHERE id=?")
                    .bind(&id)
                    .execute(pool)
                    .await
                    .map_err(db_error)?;
                let attempts = row.get::<i64, _>("attempts") + 1;
                sqlx::query("UPDATE lark_invitations SET attempts=?,next_attempt=?,status=?,detail=? WHERE id=?")
                    .bind(attempts).bind(clock+60).bind(if attempts>=3 {"failed"} else {"queued"}).bind(format!("邀请核对失败：{e}")).bind(&id).execute(pool).await.map_err(db_error)?;
                return Ok(());
            }
        }
    }
    let no = i.number.as_deref().ok_or("邀请缺少已核对的会议号")?;
    let url = i.meeting_url.as_deref().ok_or("邀请缺少已核对的入会链接")?;
    let chat: String = row.get("chat_id");
    // Both invitation formats may describe the same instance. Suppress a second reminder in the same group.
    let duplicate:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM lark_invitations WHERE app_id=? AND owner_id=? AND chat_id=? AND meeting_no=? AND starts_at=? AND id<>? AND (status IN ('sent','sending','unknown') OR (status='ready' AND id<?)))")
        .bind(&c.app_id).bind(&c.owner_id).bind(&chat).bind(no).bind(i.start).bind(&id).bind(&id).fetch_one(pool).await.map_err(db_error)?;
    if duplicate {
        return state(
            pool,
            &id,
            "duplicate",
            "同群同场会议已有预约，不重复提醒。包装不同的邀请已合并处理。",
        )
        .await;
    }
    if clock < i.start - 1800 {
        sqlx::query("UPDATE lark_invitations SET next_attempt=? WHERE id=?")
            .bind(i.start - 1800)
            .bind(&id)
            .execute(pool)
            .await
            .map_err(db_error)?;
        return Ok(());
    }
    let armed: i64 = sqlx::query_scalar("SELECT route_armed FROM lark_invitations WHERE id=?")
        .bind(&id)
        .fetch_one(pool)
        .await
        .map_err(db_error)?;
    if armed == 0 {
        let bound: Option<i64> = row.get("record_bound_at");
        let bound = bound
            .filter(|b| {
                c.auto_record
                    && c.recording_source == RecordingSource::Local
                    && *b >= c.auto_record_since
            })
            .map(|_| clock);
        let mut tx = pool.begin().await.map_err(db_error)?;
        // Do not rewind an already consumed instance when the same invitation arrives twice.
        sqlx::query("INSERT INTO lark_app_routes(app_id,owner_id,reference,reference_kind,chat_id,expires_at,local_record_bound_at,invitation_id) VALUES(?,?,?,'number',?,?,?,?) ON CONFLICT(app_id,owner_id,reference,reference_kind,chat_id) DO UPDATE SET remote_id=CASE WHEN lark_app_routes.expires_at<? OR (lark_app_routes.invitation_id<>excluded.invitation_id AND EXISTS(SELECT 1 FROM lark_invitations old WHERE old.id=lark_app_routes.invitation_id AND old.ends_at<=?)) THEN NULL ELSE lark_app_routes.remote_id END,expires_at=MAX(lark_app_routes.expires_at,excluded.expires_at),local_record_bound_at=excluded.local_record_bound_at,invitation_id=excluded.invitation_id")
            .bind(&c.app_id).bind(&c.owner_id).bind(no).bind(&chat).bind(i.end+600).bind(bound).bind(&id).bind(clock).bind(clock)
            .execute(&mut *tx).await.map_err(db_error)?;
        sqlx::query("UPDATE lark_invitations SET route_armed=1 WHERE id=?")
            .bind(&id)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        super::meeting_state::invalidate();
    }
    if clock < i.start {
        sqlx::query("UPDATE lark_invitations SET next_attempt=? WHERE id=?")
            .bind(i.start)
            .bind(&id)
            .execute(pool)
            .await
            .map_err(db_error)?;
        return Ok(());
    }
    let zone = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
    let time = chrono::DateTime::from_timestamp(i.start, 0)
        .ok_or("预约时间无效")?
        .with_timezone(&zone)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let text = format!(
        "会议开始提醒\n会议主题：{}\n会议时间：{}（东八区）\n会议 ID：{} {} {}\n入会链接：{}",
        i.title,
        time,
        &no[..3],
        &no[3..6],
        &no[6..],
        url
    );
    state(pool, &id, "sending", "正在向来源群发送开会提醒").await?;
    match api.send(&c, &chat, &id, &text).await {
        Ok(()) => {
            sqlx::query("UPDATE lark_invitations SET status='sent',detail='开会提醒已发送到来源群',sent_at=? WHERE id=?").bind(clock).bind(&id).execute(pool).await.map_err(db_error)?;
            Ok(())
        }
        Err((status, detail)) => state(pool, &id, status, &detail).await,
    }
}

pub(super) async fn list(pool: &SqlitePool, c: &AppConfig) -> Result<Vec<Value>, String> {
    let rows=sqlx::query("SELECT id,topic,starts_at,meeting_no,chat_id,status,detail FROM lark_invitations WHERE app_id=? AND owner_id=? ORDER BY command_at DESC LIMIT 30")
        .bind(&c.app_id).bind(&c.owner_id).fetch_all(pool).await.map_err(db_error)?;
    Ok(rows.iter().map(|r|json!({"id":r.get::<String,_>("id"),"topic":r.get::<String,_>("topic"),"start":r.get::<i64,_>("starts_at"),"meeting_no":r.get::<Option<String>,_>("meeting_no"),"chat_id":r.get::<String,_>("chat_id"),"status":r.get::<String,_>("status"),"detail":r.get::<String,_>("detail")})).collect())
}

pub(super) async fn manage(pool: &SqlitePool, id: &str, retry: bool) -> Result<(), String> {
    let _config = app::CONFIG_LOCK.lock().await;
    let _routing = app::DELIVERY_LOCK.lock().await;
    let c = app::load(pool).await?;
    let row = sqlx::query("SELECT * FROM lark_invitations WHERE id=? AND app_id=? AND owner_id=?")
        .bind(id)
        .bind(&c.app_id)
        .bind(&c.owner_id)
        .fetch_optional(pool)
        .await
        .map_err(db_error)?
        .ok_or("预约不存在或账号已切换")?;
    let status: String = row.get("status");
    if retry {
        if status != "failed" {
            return Err("仅能重试明确失败的预约；送达不确定时请先核对群消息。".into());
        }
        sqlx::query("UPDATE lark_invitations SET status='queued',attempts=0,next_attempt=0,detail='等待重新核对邀请' WHERE id=?").bind(id).execute(pool).await.map_err(db_error)?;
    } else {
        if matches!(status.as_str(), "sent" | "sending" | "unknown") {
            return Err("提醒已经发送或结果不确定，不能撤销。".into());
        }
        state(pool, id, "cancelled", "已取消本次提醒及其自动录音绑定").await?;
        if row.get::<i64, _>("route_armed") != 0 {
            sqlx::query("DELETE FROM lark_app_routes WHERE invitation_id=? AND remote_id IS NULL")
                .bind(id)
                .execute(pool)
                .await
                .map_err(db_error)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    struct Fake {
        sent: Mutex<Vec<String>>,
        uncertain: bool,
    }
    impl ScheduleApi for Fake {
        async fn resolve(
            &self,
            _: &SqlitePool,
            i: &Invitation,
            _: Option<&str>,
        ) -> Result<(Invitation, Option<String>), String> {
            Ok((i.clone(), None))
        }
        async fn send(
            &self,
            _: &AppConfig,
            chat: &str,
            _: &str,
            text: &str,
        ) -> Result<(), (&'static str, String)> {
            self.sent.lock().unwrap().push(format!("{chat}:{text}"));
            if self.uncertain {
                Err(("unknown", "模拟超时".into()))
            } else {
                Ok(())
            }
        }
    }
    async fn database() -> (SqlitePool, AppConfig) {
        let p = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        for sql in [
            include_str!("../../migrations/20261015000000_add_lark_app.sql"),
            include_str!("../../migrations/20261117000000_bound_meeting_recording.sql"),
            include_str!("../../migrations/20261119000000_lark_invitations.sql"),
        ] {
            sqlx::raw_sql(sql).execute(&p).await.unwrap();
        }
        let c = AppConfig {
            app_id: "cli_test".into(),
            owner_id: "ou_owner".into(),
            enabled: true,
            auto_record: true,
            auto_record_since: 1,
            ..Default::default()
        };
        app::save(&p, &c).await.unwrap();
        (p, c)
    }
    fn invitation(start: i64) -> Invitation {
        Invitation {
            title: "研发周会".into(),
            start,
            end: start + 1800,
            number: Some("807981831".into()),
            meeting_url: Some("https://vc-jp.larksuite.com/j/807981831".into()),
            share_url: None,
        }
    }
    async fn add(p: &SqlitePool, c: &AppConfig, i: Invitation, at: i64) {
        let mut tx = p.begin().await.unwrap();
        enqueue(&mut tx, c, "oc_source", "om_message", at, Ok(i))
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    async fn status(p: &SqlitePool) -> String {
        sqlx::query_scalar("SELECT status FROM lark_invitations")
            .fetch_one(p)
            .await
            .unwrap()
    }
    #[tokio::test]
    async fn waits_until_due_persists_and_sends_once_to_source_group() {
        let (p, c) = database().await;
        let t = 1800000000;
        add(&p, &c, invitation(t), (t - 7200) * 1000).await;
        let api = Fake {
            sent: Mutex::new(vec![]),
            uncertain: false,
        };
        process_with(&p, &api, t - 7200).await.unwrap();
        assert_eq!(status(&p).await, "ready");
        let routes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM lark_app_routes")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(routes, 0);
        recover(&p).await.unwrap();
        process_with(&p, &api, t - 1800).await.unwrap();
        let bound: Option<i64> =
            sqlx::query_scalar("SELECT local_record_bound_at FROM lark_app_routes")
                .fetch_one(&p)
                .await
                .unwrap();
        assert!(bound.is_some());
        assert!(api.sent.lock().unwrap().is_empty());
        process_with(&p, &api, t).await.unwrap();
        assert_eq!(status(&p).await, "sent");
        add(&p, &c, invitation(t), (t + 1) * 1000).await;
        process_with(&p, &api, t + 2).await.unwrap();
        let sent = api.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].starts_with("oc_source:"));
        assert!(sent[0].contains("807 981 831"));
        assert!(sent[0].contains("东八区"));
    }
    #[tokio::test]
    async fn old_messages_cannot_rewind_newer_schedule() {
        let (p, c) = database().await;
        let t = 1800000000;
        add(&p, &c, invitation(t), (t - 7200) * 1000).await;
        add(&p, &c, invitation(t + 600), (t - 7100) * 1000).await;
        add(&p, &c, invitation(t), (t - 7200) * 1000).await;
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT starts_at FROM lark_invitations")
                .fetch_one(&p)
                .await
                .unwrap(),
            t + 600
        );
    }
    #[tokio::test]
    async fn changing_an_armed_appointment_removes_the_old_window() {
        let (p, c) = database().await;
        let t = 1800000000;
        let api = Fake {
            sent: Mutex::new(vec![]),
            uncertain: false,
        };
        add(&p, &c, invitation(t), (t - 3600) * 1000).await;
        process_with(&p, &api, t - 1200).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM lark_app_routes")
                .fetch_one(&p)
                .await
                .unwrap(),
            1
        );
        add(&p, &c, invitation(t + 3600), (t - 1100) * 1000).await;
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM lark_app_routes")
                .fetch_one(&p)
                .await
                .unwrap(),
            0
        );
        process_with(&p, &api, t).await.unwrap();
        assert!(api.sent.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn both_formats_for_same_instance_send_one_reminder() {
        let (p, c) = database().await;
        let t = 1800000000;
        let api = Fake {
            sent: Mutex::new(vec![]),
            uncertain: false,
        };
        add(&p, &c, invitation(t), (t - 3600) * 1000).await;
        let mut calendar = invitation(t);
        calendar.share_url = Some("https://www.jp.larksuite.com/calendar/share?token=abc".into());
        add(&p, &c, calendar, (t - 3500) * 1000).await;
        process_with(&p, &api, t - 1200).await.unwrap();
        process_with(&p, &api, t - 1200).await.unwrap();
        process_with(&p, &api, t).await.unwrap();
        process_with(&p, &api, t).await.unwrap();
        assert_eq!(api.sent.lock().unwrap().len(), 1);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM lark_invitations WHERE status='duplicate'"
            )
            .fetch_one(&p)
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM lark_app_routes")
                .fetch_one(&p)
                .await
                .unwrap(),
            1
        );
    }
    #[tokio::test]
    async fn uncertain_delivery_is_not_retried_even_after_restart_or_repost() {
        let (p, c) = database().await;
        let t = 1800000000;
        add(&p, &c, invitation(t), (t - 30) * 1000).await;
        let api = Fake {
            sent: Mutex::new(vec![]),
            uncertain: true,
        };
        process_with(&p, &api, t).await.unwrap();
        assert_eq!(status(&p).await, "unknown");
        recover(&p).await.unwrap();
        add(&p, &c, invitation(t), (t + 1) * 1000).await;
        process_with(&p, &api, t + 2).await.unwrap();
        assert_eq!(api.sent.lock().unwrap().len(), 1);
    }
    #[tokio::test]
    async fn disabled_manual_mode_and_stale_appointments_do_not_send() {
        let (p, mut c) = database().await;
        let t = 1800000000;
        add(&p, &c, invitation(t), (t - 30) * 1000).await;
        let api = Fake {
            sent: Mutex::new(vec![]),
            uncertain: false,
        };
        c.manual_mode = true;
        app::save(&p, &c).await.unwrap();
        process_with(&p, &api, t).await.unwrap();
        assert_eq!(status(&p).await, "queued");
        c.manual_mode = false;
        app::save(&p, &c).await.unwrap();
        process_with(&p, &api, t + 601).await.unwrap();
        assert_eq!(status(&p).await, "expired");
        assert!(api.sent.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn cancelling_an_armed_appointment_removes_its_binding() {
        let (p, c) = database().await;
        let t = 1800000000;
        add(&p, &c, invitation(t), (t - 7200) * 1000).await;
        let api = Fake {
            sent: Mutex::new(vec![]),
            uncertain: false,
        };
        process_with(&p, &api, t - 60).await.unwrap();
        let id: String = sqlx::query_scalar("SELECT id FROM lark_invitations")
            .fetch_one(&p)
            .await
            .unwrap();
        manage(&p, &id, false).await.unwrap();
        assert_eq!(status(&p).await, "cancelled");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM lark_app_routes")
                .fetch_one(&p)
                .await
                .unwrap(),
            0
        );
        process_with(&p, &api, t).await.unwrap();
        assert!(api.sent.lock().unwrap().is_empty());
    }
}
