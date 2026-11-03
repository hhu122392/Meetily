//! Explicit group commands only. Resolve and persist one target before acknowledging
//! the event; never select a newer meeting when processing or retrying a request.
use super::{app::{self, AppConfig, db_error, now}, payload::fingerprint};
use serde_json::{json, Value};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

#[derive(Debug, PartialEq)]
pub(super) struct EndCommand { meeting_no: Option<String> }

pub(super) fn parse_command(text: &str) -> Option<EndCommand> {
    let text = regex::Regex::new(r"@_user_\d+").ok()?.replace_all(text, "");
    let suffix = text.trim().strip_prefix("结束会议")?.trim();
    if suffix.is_empty() { return Some(EndCommand { meeting_no: None }); }
    let number = suffix.strip_prefix(':').or_else(|| suffix.strip_prefix('：'))?.trim();
    (number.len() == 9 && number.bytes().all(|b| b.is_ascii_digit()))
        .then(|| EndCommand { meeting_no: Some(number.into()) })
}

pub(super) async fn enqueue(
    tx: &mut Transaction<'_, Sqlite>, c: &AppConfig, sender: &str,
    chat: &str, message_id: &str, timestamp_ms: i64, command: &EndCommand,
) -> Result<(), String> {
    // Do not filter out failed/ended latest launches and accidentally fall back to older ones.
    let target: Option<String> = sqlx::query_scalar("SELECT request_id FROM lark_meeting_launches WHERE app_id=? AND owner_id=? AND chat_id=? AND command_at<=? AND (? IS NULL OR meeting_no=?) ORDER BY command_at DESC,rowid DESC LIMIT 1")
        .bind(&c.app_id).bind(&c.owner_id).bind(chat).bind(timestamp_ms)
        .bind(&command.meeting_no).bind(&command.meeting_no).fetch_optional(&mut **tx).await.map_err(db_error)?;
    sqlx::query("INSERT OR IGNORE INTO lark_meeting_end_requests(request_id,app_id,owner_id,requester_id,chat_id,launch_id,created_at) VALUES(?,?,?,?,?,?,?)")
        .bind(fingerprint(&["end-meeting",&c.app_id,message_id])).bind(&c.app_id).bind(&c.owner_id)
        .bind(sender).bind(chat).bind(target).bind(timestamp_ms / 1000).execute(&mut **tx).await.map_err(db_error)?;
    Ok(())
}

pub(super) async fn recover(pool: &SqlitePool) -> Result<(), String> {
    sqlx::query("UPDATE lark_meeting_end_requests SET status='unknown',detail='结束会议请求曾被中断，结果不确定；请核对 Lark 会议状态，不会自动再次结束。' WHERE status='ending'")
        .execute(pool).await.map_err(db_error)?;
    sqlx::query("UPDATE lark_meeting_end_requests SET reply_status='unknown' WHERE reply_status='sending'")
        .execute(pool).await.map_err(db_error)?;
    Ok(())
}

trait EndApi {
    async fn tenant(&self, c: &AppConfig) -> Result<String, String>;
    async fn user(&self, pool: &SqlitePool, c: &AppConfig) -> Result<String, String>;
    async fn get(&self, path: &str, token: &str) -> Result<Value, String>;
    async fn end(&self, id: &str, token: &str) -> Result<Value, (&'static str, String)>;
    async fn reply(&self, chat: &str, text: &str, uuid: &str, token: &str) -> Result<Value, (&'static str, String)>;
}
struct ProductionApi;
impl EndApi for ProductionApi {
    async fn tenant(&self, c: &AppConfig) -> Result<String, String> { app::tenant_token(c).await }
    async fn user(&self, pool: &SqlitePool, c: &AppConfig) -> Result<String, String> {
        let current = super::oauth::authorized_config_locked(pool).await?;
        if current.app_id != c.app_id || current.owner_id != c.owner_id { return Err("登录身份已改变，请重新发送指令。".into()); }
        Ok(current.user_token)
    }
    async fn get(&self, path: &str, token: &str) -> Result<Value, String> {
        app::api(reqwest::Method::GET, path, token, None).await
    }
    async fn end(&self, id: &str, token: &str) -> Result<Value, (&'static str, String)> {
        mutation(reqwest::Method::PATCH, &format!("/open-apis/vc/v1/meetings/{id}/end"), token, None).await
    }
    async fn reply(&self, chat: &str, text: &str, uuid: &str, token: &str) -> Result<Value, (&'static str, String)> {
        mutation(reqwest::Method::POST, "/open-apis/im/v1/messages?receive_id_type=chat_id", token,
            Some(json!({"receive_id":chat,"msg_type":"text","content":json!({"text":text}).to_string(),"uuid":uuid}))).await
    }
}

fn mutation_outcome(http: u16, value: Option<&Value>) -> Result<Value, (&'static str, String)> {
    let Some(value) = value else { return Err(("unknown", "平台响应无法解析，请核对会议状态；不会自动重试。".into())); };
    match value["code"].as_i64() {
        Some(0) if (200..300).contains(&http) => Ok(value["data"].clone()),
        Some(code) if code != 0 && http < 500 => {
            let hint = match code {
                121005 | 99991672 | 99991679 => "请给应用开通用户身份“更新会议信息”权限并重新授权；授权账号还须在会中且是当前主持人。",
                122003 => "Meetily 授权账号必须在这场会议中，请先入会。",
                122001 => "会议已结束或不在进行中，请核对 Lark 状态。",
                122002 => "会议不存在，请核对会议号。",
                99991403 => "本月 API 调用额度已用完。",
                _ => "请检查应用权限、登录授权及会议状态。",
            };
            Err(("failed", format!("Lark 拒绝操作（{code}）：{hint}")))
        }
        _ => Err(("unknown", "平台响应异常，结果不确定；请核对会议状态，不会自动重试。".into())),
    }
}

async fn mutation(method: reqwest::Method, path: &str, token: &str, body: Option<Value>) -> Result<Value, (&'static str, String)> {
    let client = app::client().map_err(|e| ("failed", e))?;
    let mut request = client.request(method, format!("{}{path}", app::BASE))
        .bearer_auth(token).header("Content-Type", "application/json; charset=utf-8");
    if let Some(body) = body { request = request.json(&body); }
    let response = request.send().await.map_err(|_| ("unknown", "请求连接中断，结果不确定；请核对会议状态，不会自动重试。".into()))?;
    let http = response.status().as_u16();
    let value = response.json::<Value>().await.ok();
    mutation_outcome(http, value.as_ref())
}

async fn finish(pool: &SqlitePool, request: &str, status: &str, detail: &str) -> Result<(), String> {
    sqlx::query("UPDATE lark_meeting_end_requests SET status=?,detail=? WHERE request_id=?")
        .bind(status).bind(detail).bind(request).execute(pool).await.map_err(db_error)?;
    app::status(detail).await;
    Ok(())
}

pub(super) async fn process_one(pool: &SqlitePool) -> Result<(), String> { process_with(pool, &ProductionApi).await }

// Called by launches::run while holding CONFIG_LOCK, so credentials cannot switch mid-operation.
async fn process_with(pool: &SqlitePool, api: &impl EndApi) -> Result<(), String> {
    let c = app::load(pool).await?;
    if !c.enabled || c.owner_id.is_empty() { return Ok(()); }
    let row = sqlx::query("SELECT * FROM lark_meeting_end_requests WHERE app_id=? AND owner_id=? AND (status='queued' OR (status IN ('ended','failed','unknown') AND reply_status='pending')) ORDER BY created_at,rowid LIMIT 1")
        .bind(&c.app_id).bind(&c.owner_id).fetch_optional(pool).await.map_err(db_error)?;
    let Some(row) = row else { return Ok(()); };
    let request: String = row.get("request_id");
    let chat: String = row.get("chat_id");
    if row.get::<String,_>("status") != "queued" {
        let token = match api.tenant(&c).await {
            Ok(t) => t,
            Err(e) => {
                sqlx::query("UPDATE lark_meeting_end_requests SET reply_status='failed' WHERE request_id=?").bind(&request).execute(pool).await.map_err(db_error)?;
                return Err(format!("结束会议结果未能回复到群：{e}"));
            }
        };
        sqlx::query("UPDATE lark_meeting_end_requests SET reply_status='sending' WHERE request_id=?").bind(&request).execute(pool).await.map_err(db_error)?;
        let text: String = row.get("detail");
        let uuid = fingerprint(&["end-meeting-reply", &request]);
        let result = api.reply(&chat, &text, &uuid[..32], &token).await;
        let state = match &result {
            Ok(v) if v["message_id"].as_str().is_some_and(|v| !v.is_empty()) => "sent",
            Ok(_) => "unknown",
            Err((s, _)) => s,
        };
        sqlx::query("UPDATE lark_meeting_end_requests SET reply_status=? WHERE request_id=?").bind(state).bind(&request).execute(pool).await.map_err(db_error)?;
        if state != "sent" { app::status(format!("{text}（群回复未确认送达，请核对）")).await; }
        return Ok(());
    }
    if row.get::<i64,_>("created_at") < now() - 600 {
        return finish(pool,&request,"failed","结束会议指令已过期，未执行；请重新发送。").await;
    }
    let target: Option<String> = row.get("launch_id");
    let launch = sqlx::query("SELECT * FROM lark_meeting_launches WHERE request_id=? AND app_id=? AND owner_id=? AND chat_id=?")
        .bind(target).bind(&c.app_id).bind(&c.owner_id).bind(&chat).fetch_optional(pool).await.map_err(db_error)?;
    let Some(launch) = launch else {
        return finish(pool,&request,"failed","未找到本群对应的机器人会议。请先 @会议助手 开会，或核对指定会议号。").await;
    };
    let sender: String = row.get("requester_id");
    if sender != c.owner_id && (!c.allow_group_launch || sender.is_empty() || sender != launch.get::<String,_>("requester_id")) {
        return finish(pool,&request,"failed","只有这场会议的发起人或 Meetily 授权账号可以结束会议。").await;
    }
    let reserve: Option<String> = launch.get("reserve_id");
    let number: Option<String> = launch.get("meeting_no");
    let (Some(reserve), Some(number)) = (reserve.filter(|s| valid_id(s)), number) else {
        return finish(pool,&request,"failed","目标会议尚未成功创建，未执行结束操作；不会改为结束其他会议。").await;
    };
    let known: Option<String> = sqlx::query_scalar("SELECT remote_id FROM lark_app_routes WHERE app_id=? AND owner_id=? AND chat_id=? AND reference_kind='reserve' AND reference=?")
        .bind(&c.app_id).bind(&c.owner_id).bind(&chat).bind(&reserve).fetch_optional(pool).await.map_err(db_error)?.flatten();
    if let Some(id) = &known {
        if super::meeting_state::ended(pool, &c, id).await? {
            return finish(pool,&request,"ended",&format!("会议 {number} 已结束，无需重复操作。")).await;
        }
    }
    let token = match api.tenant(&c).await {
        Ok(t) => t, Err(e) => return finish(pool,&request,"failed",&e).await,
    };
    let active = match api.get(&format!("/open-apis/vc/v1/reserves/{reserve}/get_active_meeting?with_participants=false&user_id_type=open_id"), &token).await {
        Ok(v) => v,
        Err(e) => return finish(pool,&request,"failed",&format!("未能确认会议 {number} 正在进行，未结束任何会议：{e}")).await,
    };
    let meeting = &active["meeting"];
    let id = meeting["id"].as_str().unwrap_or("");
    if !valid_id(id) || meeting["meeting_no"].as_str() != Some(number.as_str())
        || known.as_ref().is_some_and(|k| k != id) {
        return finish(pool,&request,"failed","预约未返回对应的进行中会议，或会议实例已改变；未结束任何会议。").await;
    }
    if super::meeting_state::ended(pool, &c, id).await? {
        return finish(pool,&request,"ended",&format!("会议 {number} 已结束，无需重复操作。")).await;
    }
    if meeting["start_time"].as_str().and_then(|v| v.parse::<i64>().ok())
        .is_some_and(|start| start > row.get::<i64,_>("created_at")) {
        return finish(pool,&request,"failed","该会议在指令发送后才开始，未执行结束操作；请重新发送。").await;
    }
    let user_token = match api.user(pool, &c).await {
        Ok(t) => t, Err(e) => return finish(pool,&request,"failed",&e).await,
    };
    sqlx::query("UPDATE lark_meeting_end_requests SET status='ending',meeting_id=? WHERE request_id=?")
        .bind(id).bind(&request).execute(pool).await.map_err(db_error)?;
    match api.end(id, &user_token).await {
        Ok(_) => {
            // Persist API-confirmed completion before replying, even if the event arrives late.
            let mut tx = pool.begin().await.map_err(db_error)?;
            sqlx::query("INSERT INTO lark_meeting_signals(app_id,owner_id,remote_id,meeting_no,event_kind,event_at,ended) VALUES(?,?,?,?,'meetily.end_confirmed',?,1) ON CONFLICT(app_id,owner_id,remote_id) DO UPDATE SET ended=1,event_at=excluded.event_at,event_kind=excluded.event_kind")
                .bind(&c.app_id).bind(&c.owner_id).bind(id).bind(&number).bind(now()*1000).execute(&mut *tx).await.map_err(db_error)?;
            let detail = format!("会议 {number} 已结束。对应的 Meetily 自动录音会停止并保存，后台继续处理；纪要仍需确认后发送。");
            sqlx::query("UPDATE lark_meeting_end_requests SET status='ended',detail=? WHERE request_id=?").bind(&detail).bind(&request).execute(&mut *tx).await.map_err(db_error)?;
            tx.commit().await.map_err(db_error)?;
            super::meeting_state::invalidate();
            app::status(detail).await;
            Ok(())
        }
        Err((state, e)) => finish(pool,&request,state,&format!("会议 {number}：{e}")).await,
    }
}

fn valid_id(id: &str) -> bool { !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_digit()) }

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeApi {
        calls: Mutex<Vec<String>>,
        active: Value,
        outcome: Result<Value, (&'static str, String)>,
    }
    impl FakeApi {
        fn success() -> Self {
            Self { calls: Mutex::new(vec![]), active: json!({"meeting":{"id":"10001","meeting_no":"123456789","start_time":(now()-60).to_string()}}), outcome: Ok(json!({})) }
        }
    }
    impl EndApi for FakeApi {
        async fn tenant(&self, _: &AppConfig) -> Result<String, String> { Ok("tenant".into()) }
        async fn user(&self, _: &SqlitePool, _: &AppConfig) -> Result<String, String> { Ok("user".into()) }
        async fn get(&self, path: &str, token: &str) -> Result<Value, String> {
            assert_eq!(token, "tenant");
            self.calls.lock().unwrap().push(format!("GET {path}"));
            Ok(self.active.clone())
        }
        async fn end(&self, id: &str, token: &str) -> Result<Value, (&'static str, String)> {
            assert_eq!(token, "user");
            self.calls.lock().unwrap().push(format!("PATCH {id}"));
            self.outcome.clone()
        }
        async fn reply(&self, chat: &str, _: &str, _: &str, token: &str) -> Result<Value, (&'static str, String)> {
            assert_eq!(token, "tenant");
            self.calls.lock().unwrap().push(format!("REPLY {chat}"));
            Ok(json!({"message_id":"reply"}))
        }
    }
    async fn db() -> (SqlitePool, AppConfig) {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        for sql in [
            include_str!("../../migrations/20261015000000_add_lark_app.sql"),
            include_str!("../../migrations/20261015010000_add_lark_launches.sql"),
            include_str!("../../migrations/20261016000000_add_lark_meeting_signals.sql"),
            include_str!("../../migrations/20261020010000_lark_end_meeting.sql"),
        ] { sqlx::raw_sql(sql).execute(&pool).await.unwrap(); }
        let c = AppConfig { app_id:"app".into(), owner_id:"owner".into(), enabled:true, allow_group_launch:true, ..Default::default() };
        app::save(&pool, &c).await.unwrap();
        (pool, c)
    }
    async fn launch(pool: &SqlitePool, id: &str, chat: &str, sender: &str, no: &str, time: i64) {
        sqlx::query("INSERT INTO lark_meeting_launches(request_id,app_id,owner_id,message_id,chat_id,topic,status,reserve_id,meeting_no,created_at,expires_at,requester_id,command_at) VALUES(?,'app','owner',?,?,'主题','sent',?,?,?, ?,?,?)")
            .bind(id).bind(id).bind(chat).bind(if no == "123456789" { "9001" } else { "9002" })
            .bind(no).bind(time/1000).bind(now()+21600).bind(sender).bind(time).execute(pool).await.unwrap();
    }
    async fn queue(pool: &SqlitePool, c: &AppConfig, sender: &str, chat: &str, message: &str, text: &str, time: i64) {
        let mut tx = pool.begin().await.unwrap();
        enqueue(&mut tx,c,sender,chat,message,time,&parse_command(text).unwrap()).await.unwrap();
        tx.commit().await.unwrap();
    }
    async fn state(pool: &SqlitePool) -> String {
        sqlx::query_scalar("SELECT status FROM lark_meeting_end_requests LIMIT 1").fetch_one(pool).await.unwrap()
    }

    #[test]
    fn command_requires_exact_verb_and_nine_digit_number() {
        assert_eq!(parse_command("@_user_1 结束会议"),Some(EndCommand{meeting_no:None}));
        for s in ["结束会议:123456789","@_user_1 结束会议： 123456789"] {
            assert_eq!(parse_command(s).unwrap().meeting_no.as_deref(),Some("123456789"));
        }
        for s in ["请结束会议","结束会议吗","结束会议123456789","结束会议:123","结束会议:１２３４５６７８９","结束会议:123456789 其他群"] {
            assert!(parse_command(s).is_none(),"{s}");
        }
    }

    #[tokio::test]
    async fn freezes_latest_group_target_and_deduplicates_messages() {
        let (pool,c)=db().await;
        let time=now()*1000;
        launch(&pool,"old","oc_a","alice","123456789",time-2000).await;
        launch(&pool,"other","oc_b","bob","987654321",time-1000).await;
        queue(&pool,&c,"alice","oc_a","end1","结束会议",time).await;
        launch(&pool,"later","oc_a","alice","987654321",time+1000).await;
        queue(&pool,&c,"alice","oc_a","end1","结束会议",time+2000).await;
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT launch_id FROM lark_meeting_end_requests").fetch_one(&pool).await.unwrap(),"old");
        let api=FakeApi::success();
        process_with(&pool,&api).await.unwrap();
        assert_eq!(state(&pool).await,"ended");
        assert!(super::super::meeting_state::ended(&pool,&c,"10001").await.unwrap());
        process_with(&pool,&api).await.unwrap();
        recover(&pool).await.unwrap();
        process_with(&pool,&api).await.unwrap();
        assert_eq!(*api.calls.lock().unwrap(),vec![
            "GET /open-apis/vc/v1/reserves/9001/get_active_meeting?with_participants=false&user_id_type=open_id",
            "PATCH 10001","REPLY oc_a"]);
    }

    #[tokio::test]
    async fn explicit_number_can_choose_older_meeting_but_never_another_group() {
        let (pool,c)=db().await; let time=now()*1000;
        launch(&pool,"old","oc_a","alice","123456789",time-3000).await;
        launch(&pool,"new","oc_a","alice","987654321",time-2000).await;
        launch(&pool,"other","oc_b","bob","555555555",time-1000).await;
        queue(&pool,&c,"owner","oc_a","one","结束会议：123456789",time).await;
        queue(&pool,&c,"owner","oc_a","two","结束会议:555555555",time).await;
        let targets: Vec<Option<String>>=sqlx::query_scalar("SELECT launch_id FROM lark_meeting_end_requests ORDER BY rowid").fetch_all(&pool).await.unwrap();
        assert_eq!(targets,vec![Some("old".into()),None]);
        let api=FakeApi::success();
        for _ in 0..4 { process_with(&pool,&api).await.unwrap(); }
        assert_eq!(api.calls.lock().unwrap().iter().filter(|x|x.starts_with("PATCH")).count(),1);
    }

    #[tokio::test]
    async fn unauthorized_sender_failed_latest_and_mismatched_instance_never_end() {
        for scenario in ["stranger","failed-latest","mismatch","future","changed-instance"] {
            let (pool,c)=db().await; let time=now()*1000;
            launch(&pool,"old","oc_a","alice","123456789",time-2000).await;
            let mut api=FakeApi::success();
            if scenario=="failed-latest" {
                launch(&pool,"failed","oc_a","alice","987654321",time-1000).await;
                sqlx::query("UPDATE lark_meeting_launches SET reserve_id=NULL,status='failed' WHERE request_id='failed'").execute(&pool).await.unwrap();
            }
            if scenario=="mismatch" { api.active["meeting"]["meeting_no"]=json!("111111111"); }
            if scenario=="future" { api.active["meeting"]["start_time"]=json!((now()+30).to_string()); }
            if scenario=="changed-instance" {
                sqlx::query("INSERT INTO lark_app_routes VALUES('app','owner','9001','reserve','oc_a','99999',9999999999)").execute(&pool).await.unwrap();
            }
            queue(&pool,&c,if scenario=="stranger" {"bob"}else{"alice"},"oc_a","one","结束会议",time).await;
            process_with(&pool,&api).await.unwrap();
            assert_eq!(state(&pool).await,"failed","{scenario}");
            assert!(!api.calls.lock().unwrap().iter().any(|x|x.starts_with("PATCH")),"{scenario}");
        }
    }

    #[tokio::test]
    async fn unknown_outcome_and_restart_never_repeat_end_mutation() {
        let (pool,c)=db().await; let time=now()*1000;
        launch(&pool,"old","oc_a","owner","123456789",time-2000).await;
        queue(&pool,&c,"owner","oc_a","one","结束会议",time).await;
        let mut api=FakeApi::success(); api.outcome=Err(("unknown","连接中断".into()));
        process_with(&pool,&api).await.unwrap();
        assert_eq!(state(&pool).await,"unknown");
        recover(&pool).await.unwrap();
        for _ in 0..3 { process_with(&pool,&api).await.unwrap(); }
        assert_eq!(api.calls.lock().unwrap().iter().filter(|x|x.starts_with("PATCH")).count(),1);
        sqlx::query("UPDATE lark_meeting_end_requests SET status='ending',reply_status='sending'").execute(&pool).await.unwrap();
        recover(&pool).await.unwrap();
        process_with(&pool,&api).await.unwrap();
        assert_eq!(state(&pool).await,"unknown");
        assert_eq!(api.calls.lock().unwrap().iter().filter(|x|x.starts_with("PATCH")).count(),1);
    }

    #[test]
    fn endpoint_errors_are_not_reported_as_success() {
        let error=mutation_outcome(403,Some(&json!({"code":121005}))).unwrap_err();
        assert_eq!(error.0,"failed"); assert!(error.1.contains("主持人"));
        assert_eq!(mutation_outcome(500,Some(&json!({"code":121001}))).unwrap_err().0,"unknown");
        assert!(mutation_outcome(200,Some(&json!({"code":0,"data":{}}))).is_ok());
    }
}
