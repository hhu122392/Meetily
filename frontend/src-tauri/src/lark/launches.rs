use super::app::{self, api, client, db_error, now, tenant_token, AppConfig, BASE};
use super::payload::fingerprint;
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};
use tokio_util::sync::CancellationToken;

// Accept a short, explicit command only; ordinary chat is never interpreted as a request.
pub(super) fn parse_command(text: &str) -> Option<String> {
    let cleaned = regex::Regex::new(r"@_user_\d+").ok()?.replace_all(text, "");
    let text = cleaned.trim();
    if text == "开会" {
        return Some("群会议".into());
    }
    let topic = text.strip_prefix("开会 ")?.trim();
    (!topic.is_empty() && topic.chars().count() <= 80 && !topic.chars().any(char::is_control))
        .then(|| topic.to_owned())
}

pub(super) async fn recover(pool: &SqlitePool) -> Result<(), String> {
    sqlx::query("UPDATE lark_meeting_launches SET status='unknown',detail='上次创建或链接发送中断，结果不确定；不会自动重试，请核对群消息和会议。' WHERE status IN ('creating','link_sending')")
        .execute(pool).await.map_err(db_error)?;
    super::ending::recover(pool).await?;
    Ok(())
}

pub(super) async fn run(pool: SqlitePool, cancel: CancellationToken) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
    loop {
        tokio::select! { _=cancel.cancelled()=>break, _=interval.tick()=>{} }
        // Serialize with credential/config changes. Never cancel a submitted mutation mid-flight.
        let _config = app::CONFIG_LOCK.lock().await;
        if cancel.is_cancelled() {
            break;
        }
        if let Err(e) = process_one(&pool).await {
            app::status(e).await;
        }
        if let Err(e) = super::ending::process_one(&pool).await {
            app::status(e).await;
        }
    }
}

async fn set_status(pool: &SqlitePool, id: &str, status: &str, detail: &str) -> Result<(), String> {
    sqlx::query("UPDATE lark_meeting_launches SET status=?,detail=? WHERE request_id=?")
        .bind(status)
        .bind(detail)
        .bind(id)
        .execute(pool)
        .await
        .map_err(db_error)?;
    Ok(())
}

fn digits(value: &Value) -> Option<&str> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_digit()))
}

fn reservation(value: &Value) -> Result<(String, String, String), String> {
    let r = &value["reserve"];
    let id = digits(&r["id"]).ok_or("预约响应缺少有效 ID。")?;
    let no = digits(&r["meeting_no"])
        .filter(|s| s.len() == 9)
        .ok_or("预约响应缺少有效会议号。")?;
    let raw = r["url"].as_str().ok_or("预约响应缺少会议链接。")?;
    let url = url::Url::parse(raw).map_err(|_| "预约会议链接无效。")?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || !url
            .host_str()
            .is_some_and(|h| h == "larksuite.com" || h.ends_with(".larksuite.com"))
    {
        return Err("预约会议链接不属于 Lark。".into());
    }
    Ok((id.into(), no.into(), raw.into()))
}

// An ambiguous mutation is never automatically retried, even after restarting the app.
async fn mutation(path: &str, token: &str, body: Value) -> Result<Value, (&'static str, String)> {
    let http = client().map_err(|e| ("failed", e))?;
    let response = http
        .post(format!("{BASE}{path}"))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|_| ("unknown", "网络请求中断，结果不确定；不会自动重试。".into()))?;
    let status = response.status();
    let value: Value = response
        .json()
        .await
        .map_err(|_| ("unknown", "响应无法解析，结果不确定；不会自动重试。".into()))?;
    match value["code"].as_i64() {
        Some(0) if status.is_success() => Ok(value["data"].clone()),
        Some(code) if code != 0 && !status.is_server_error() => Err((
            "failed",
            format!("Lark 拒绝请求，错误码 {code}；请检查会议预约权限和应用发布状态。"),
        )),
        _ => Err(("unknown", "平台响应异常，结果不确定；不会自动重试。".into())),
    }
}

async fn process_one(pool: &SqlitePool) -> Result<(), String> {
    process_one_with(pool, &ProductionApi).await
}

trait LaunchApi {
    async fn get(&self, path: &str, token: &str) -> Result<Value, String>;
    async fn token(&self, config: &AppConfig) -> Result<String, String>;
    async fn post(
        &self,
        path: &str,
        token: &str,
        body: Value,
    ) -> Result<Value, (&'static str, String)>;
}
struct ProductionApi;
impl LaunchApi for ProductionApi {
    async fn get(&self, path: &str, token: &str) -> Result<Value, String> {
        api(reqwest::Method::GET, path, token, None).await
    }
    async fn token(&self, config: &AppConfig) -> Result<String, String> {
        tenant_token(config).await
    }
    async fn post(
        &self,
        path: &str,
        token: &str,
        body: Value,
    ) -> Result<Value, (&'static str, String)> {
        mutation(path, token, body).await
    }
}

async fn process_one_with(pool: &SqlitePool, api: &impl LaunchApi) -> Result<(), String> {
    let c = app::load(pool).await?;
    if !c.enabled || c.owner_id.is_empty() {
        return Ok(());
    }
    let row = sqlx::query("SELECT * FROM lark_meeting_launches WHERE app_id=? AND owner_id=? AND status IN ('queued','ready') ORDER BY created_at LIMIT 1")
        .bind(&c.app_id).bind(&c.owner_id).fetch_optional(pool).await.map_err(db_error)?;
    let Some(row) = row else {
        return Ok(());
    };
    let id: String = row.get("request_id");
    let state: String = row.get("status");
    if row.get::<i64, _>("expires_at") <= now()
        || (state == "queued" && row.get::<i64, _>("created_at") < now() - 600)
    {
        return set_status(pool, &id, "expired", "开会请求已过期，请重新发送开会指令。").await;
    }
    let token = match api.token(&c).await {
        Ok(t) => t,
        Err(e) => {
            return set_status(
                pool,
                &id,
                if state == "ready" {
                    "link_failed"
                } else {
                    "failed"
                },
                &e,
            )
            .await
        }
    };
    if state == "queued" {
        set_status(pool, &id, "creating", "正在创建会议").await?;
        let data = match api.post("/open-apis/vc/v1/reserves/apply?user_id_type=open_id", &token,
            json!({"owner_id":c.owner_id,"end_time":row.get::<i64,_>("expires_at").to_string(),
                "meeting_settings":{"topic":row.get::<String,_>("topic"),"meeting_initial_type":1,"auto_record":false}})).await {
            Ok(v) => v,
            Err((state, e)) => return set_status(pool, &id, state, &e).await,
        };
        let (reserve, no, url) = match reservation(&data) {
            Ok(r) => r,
            Err(e) => {
                return set_status(
                    pool,
                    &id,
                    "unknown",
                    &format!("会议可能已创建：{e} 不会自动重试。"),
                )
                .await
            }
        };
        let mut tx = pool.begin().await.map_err(db_error)?;
        let topic: String = row.get("topic");
        sqlx::query("UPDATE lark_meeting_launches SET status='ready',detail='会议已创建，等待发送入会链接',reserve_id=?,meeting_no=?,meeting_url=?,topic=? WHERE request_id=?")
            .bind(&reserve).bind(&no).bind(&url).bind(super::routing::meeting_title(&topic, &no)).bind(&id).execute(&mut *tx).await.map_err(db_error)?;
        sqlx::query("INSERT INTO lark_app_routes(app_id,owner_id,reference,reference_kind,chat_id,expires_at) VALUES(?,?,?,'reserve',?,?)")
            .bind(&c.app_id).bind(&c.owner_id).bind(&reserve).bind(row.get::<String,_>("chat_id")).bind(row.get::<i64,_>("expires_at"))
            .execute(&mut *tx).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        // The next iteration sends the persisted result. A crash cannot create a second reservation.
        return Ok(());
    }
    let text = format!("{}\n会议号：{}\n{}\n\n已自动关联本群。请使用授权账号加入会议，再开始 Meetily 录音；会后停止录音，纪要生成后发回本群。此链接六小时内有效，仅关联本次会议。",
        row.get::<String,_>("topic"), row.get::<String,_>("meeting_no"), row.get::<String,_>("meeting_url"));
    let uuid = fingerprint(&["lark-meeting-link", &id]);
    set_status(pool, &id, "link_sending", "会议已创建，正在发送入会链接").await?;
    let data = api.post("/open-apis/im/v1/messages?receive_id_type=chat_id", &token,
        json!({"receive_id":row.get::<String,_>("chat_id"),"msg_type":"text","content":json!({"text":text}).to_string(),"uuid":&uuid[..32]})).await;
    match data {
        Ok(v) if v["message_id"].as_str().is_some_and(|s| !s.is_empty()) => {
            sqlx::query("UPDATE lark_meeting_launches SET status='sent',detail='入会链接已发回来源群，加入后即可开始录音',reply_id=? WHERE request_id=?")
                .bind(v["message_id"].as_str()).bind(&id).execute(pool).await.map_err(db_error)?;
            Ok(())
        }
        Ok(_) => {
            set_status(
                pool,
                &id,
                "unknown",
                "会议已创建，但入会链接送达结果不确定；请核对群消息。",
            )
            .await
        }
        Err((state, e)) => {
            set_status(
                pool,
                &id,
                if state == "failed" {
                    "link_failed"
                } else {
                    state
                },
                &format!("会议已创建；入会链接发送：{e}"),
            )
            .await
        }
    }
}

// Verify the reservation's actual instance; a meeting number alone is not enough.
pub(super) async fn resolve_current(
    pool: &SqlitePool,
    c: &AppConfig,
    id: &str,
    no: &str,
) -> Result<(), String> {
    resolve_with(pool, c, id, no, &ProductionApi).await
}

async fn resolve_with(
    pool: &SqlitePool,
    c: &AppConfig,
    id: &str,
    no: &str,
    api: &impl LaunchApi,
) -> Result<(), String> {
    let rows = sqlx::query("SELECT j.reserve_id,j.chat_id FROM lark_meeting_launches j JOIN lark_app_routes r ON r.app_id=j.app_id AND r.owner_id=j.owner_id AND r.reference=j.reserve_id AND r.reference_kind='reserve' AND r.chat_id=j.chat_id WHERE j.app_id=? AND j.owner_id=? AND j.meeting_no=? AND j.expires_at>? AND r.expires_at>? AND r.remote_id IS NULL")
        .bind(&c.app_id).bind(&c.owner_id).bind(no).bind(now()).bind(now()).fetch_all(pool).await.map_err(db_error)?;
    if rows.is_empty() {
        return Ok(());
    }
    if rows.len() != 1 {
        return Err("多个预约对应同一会议号，暂停自动关联。".into());
    }
    let reserve: String = rows[0].get("reserve_id");
    let token = api.token(c).await?;
    let data = api.get(&format!("/open-apis/vc/v1/reserves/{reserve}/get_active_meeting?with_participants=false&user_id_type=open_id"), &token).await?;
    verify_instance(&data, id, no)?;
    sqlx::query("UPDATE lark_app_routes SET remote_id=?,expires_at=? WHERE app_id=? AND owner_id=? AND reference_kind='reserve' AND reference=? AND chat_id=? AND remote_id IS NULL AND expires_at>?")
        .bind(id).bind(now()+7*86400).bind(&c.app_id).bind(&c.owner_id).bind(&reserve).bind(rows[0].get::<String,_>("chat_id")).bind(now())
        .execute(pool).await.map_err(db_error)?;
    Ok(())
}

fn verify_instance(data: &Value, id: &str, no: &str) -> Result<(), String> {
    if data["meeting"]["id"].as_str() != Some(id)
        || data["meeting"]["meeting_no"].as_str() != Some(no)
    {
        return Err("预约会议与当前参会实例不一致，暂停自动关联。".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    struct FakeApi {
        calls: Mutex<Vec<(String, Value)>>,
        results: Mutex<std::collections::VecDeque<Result<Value, (&'static str, String)>>>,
    }
    impl LaunchApi for FakeApi {
        async fn get(&self, path: &str, token: &str) -> Result<Value, String> {
            self.post(path, token, Value::Null)
                .await
                .map_err(|(_, e)| e)
        }
        async fn token(&self, _: &AppConfig) -> Result<String, String> {
            Ok("test-token".into())
        }
        async fn post(
            &self,
            path: &str,
            _: &str,
            body: Value,
        ) -> Result<Value, (&'static str, String)> {
            self.calls.lock().unwrap().push((path.into(), body));
            self.results
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected extra network mutation")
        }
    }
    async fn database() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
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
        app::save(
            &pool,
            &AppConfig {
                app_id: "cli_test".into(),
                owner_id: "ou_owner".into(),
                enabled: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO lark_meeting_launches(request_id,app_id,owner_id,message_id,chat_id,topic,created_at,expires_at) VALUES('request','cli_test','ou_owner','om_1','oc_source','研发周会',?,?)")
            .bind(now()).bind(now()+21600).execute(&pool).await.unwrap();
        pool
    }
    fn created() -> Value {
        json!({"reserve":{"id":"1234567890123","meeting_no":"123456789","url":"https://vc.larksuite.com/j/123456789"}})
    }
    async fn state(pool: &SqlitePool) -> String {
        sqlx::query_scalar("SELECT status FROM lark_meeting_launches")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn unnamed_group_reply_uses_persisted_meeting_number() {
        let pool = database().await;
        sqlx::query("UPDATE lark_meeting_launches SET topic='群会议'").execute(&pool).await.unwrap();
        let api = FakeApi {
            calls: Mutex::new(vec![]),
            results: Mutex::new(vec![Ok(created()), Ok(json!({"message_id":"om_reply"}))].into()),
        };
        process_one_with(&pool, &api).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_, String>("SELECT topic FROM lark_meeting_launches").fetch_one(&pool).await.unwrap(), "123456789");
        recover(&pool).await.unwrap();
        process_one_with(&pool, &api).await.unwrap();
        let calls = api.calls.lock().unwrap();
        let content: Value = serde_json::from_str(calls[1].1["content"].as_str().unwrap()).unwrap();
        assert!(content["text"].as_str().unwrap().starts_with("123456789\n"));
        assert_eq!(calls.len(), 2);
    }
    #[tokio::test]
    async fn create_persists_source_group_before_reply_and_resumes_without_recreating() {
        let pool = database().await;
        let api = FakeApi {
            calls: Mutex::new(vec![]),
            results: Mutex::new(vec![Ok(created()), Ok(json!({"message_id":"om_reply"}))].into()),
        };
        process_one_with(&pool, &api).await.unwrap();
        assert_eq!(state(&pool).await, "ready");
        assert_eq!(sqlx::query_scalar::<_, String>("SELECT topic FROM lark_meeting_launches").fetch_one(&pool).await.unwrap(), "研发周会");
        let route =
            sqlx::query("SELECT reference_kind,reference,chat_id,remote_id FROM lark_app_routes")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(route.get::<String, _>("reference_kind"), "reserve");
        assert_eq!(route.get::<String, _>("reference"), "1234567890123");
        assert_eq!(route.get::<String, _>("chat_id"), "oc_source");
        assert!(route.get::<Option<String>, _>("remote_id").is_none());
        recover(&pool).await.unwrap();
        process_one_with(&pool, &api).await.unwrap();
        process_one_with(&pool, &api).await.unwrap();
        assert_eq!(state(&pool).await, "sent");
        let calls = api.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].0.contains("reserves/apply"));
        assert_eq!(calls[0].1["owner_id"], "ou_owner");
        assert_eq!(calls[0].1["meeting_settings"]["auto_record"], false);
        assert_eq!(calls[1].1["receive_id"], "oc_source");
        assert_eq!(calls[1].1["uuid"].as_str().unwrap().len(), 32);
    }
    #[tokio::test]
    async fn uncertain_creation_and_interrupted_reply_are_never_retried() {
        let pool = database().await;
        let api = FakeApi {
            calls: Mutex::new(vec![]),
            results: Mutex::new(vec![Err(("unknown", "timeout".into()))].into()),
        };
        process_one_with(&pool, &api).await.unwrap();
        recover(&pool).await.unwrap();
        process_one_with(&pool, &api).await.unwrap();
        assert_eq!(state(&pool).await, "unknown");
        assert_eq!(api.calls.lock().unwrap().len(), 1);
        sqlx::query("UPDATE lark_meeting_launches SET status='link_sending'")
            .execute(&pool)
            .await
            .unwrap();
        recover(&pool).await.unwrap();
        process_one_with(&pool, &api).await.unwrap();
        assert_eq!(state(&pool).await, "unknown");
        assert_eq!(api.calls.lock().unwrap().len(), 1);
    }
    #[tokio::test]
    async fn expired_requests_and_changed_owner_never_call_lark() {
        let pool = database().await;
        let api = FakeApi {
            calls: Mutex::new(vec![]),
            results: Mutex::new(vec![].into()),
        };
        sqlx::query("UPDATE lark_meeting_launches SET owner_id='ou_someone_else'")
            .execute(&pool)
            .await
            .unwrap();
        process_one_with(&pool, &api).await.unwrap();
        assert_eq!(state(&pool).await, "queued");
        sqlx::query("UPDATE lark_meeting_launches SET owner_id='ou_owner',created_at=0")
            .execute(&pool)
            .await
            .unwrap();
        process_one_with(&pool, &api).await.unwrap();
        assert_eq!(state(&pool).await, "expired");
        assert!(api.calls.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn reservation_locks_only_to_verified_instance_and_does_not_follow_reused_number() {
        let pool = database().await;
        let api = FakeApi {
            calls: Mutex::new(vec![]),
            results: Mutex::new(
                vec![
                    Ok(created()),
                    Ok(json!({"meeting":{"id":"9999999999999","meeting_no":"123456789"}})),
                    Ok(json!({"meeting":{"id":"1111111111111","meeting_no":"123456789"}})),
                ]
                .into(),
            ),
        };
        process_one_with(&pool, &api).await.unwrap();
        let c = app::load(&pool).await.unwrap();
        assert!(resolve_with(&pool, &c, "1111111111111", "123456789", &api)
            .await
            .is_err());
        assert!(
            sqlx::query_scalar::<_, Option<String>>("SELECT remote_id FROM lark_app_routes")
                .fetch_one(&pool)
                .await
                .unwrap()
                .is_none()
        );
        resolve_with(&pool, &c, "1111111111111", "123456789", &api)
            .await
            .unwrap();
        resolve_with(&pool, &c, "2222222222222", "123456789", &api)
            .await
            .unwrap();
        let route = sqlx::query("SELECT remote_id,chat_id,expires_at FROM lark_app_routes")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(route.get::<String, _>("remote_id"), "1111111111111");
        assert_eq!(route.get::<String, _>("chat_id"), "oc_source");
        assert!(route.get::<i64, _>("expires_at") > now() + 6 * 86400);
        assert_eq!(api.calls.lock().unwrap().len(), 3);
    }
    #[test]
    fn explicit_launch_command() {
        assert_eq!(parse_command("@_user_1 开会"), Some("群会议".into()));
        assert_eq!(
            parse_command("@_user_1 开会 研发周会"),
            Some("研发周会".into())
        );
        for text in ["今晚开会吗", "开会吧", "开会 A\nB"] {
            assert!(parse_command(text).is_none());
        }
        assert!(parse_command(&format!("开会 {}", "会".repeat(81))).is_none());
    }
    #[test]
    fn validates_reservation_and_instance() {
        let mut v = json!({"reserve":{"id":"1234567890123","meeting_no":"123456789","url":"https://vc.larksuite.com/j/123456789"}});
        assert!(reservation(&v).is_ok());
        v["reserve"]["url"] = json!("https://larksuite.com.evil.test/j/123456789");
        assert!(reservation(&v).is_err());
        let instance = json!({"meeting":{"id":"1234567890123","meeting_no":"123456789"}});
        assert!(verify_instance(&instance, "1234567890123", "123456789").is_ok());
        assert!(verify_instance(&instance, "9999999999999", "123456789").is_err());
        assert!(verify_instance(&json!({}), "1234567890123", "123456789").is_err());
    }
}
