//! Event-triggered invalidation with a shared, bounded polling fallback.
use super::{app::{self, AppConfig, db_error, now}, oauth, routing::active_meeting};
use serde_json::Value;
use sqlx::SqlitePool;
use std::{future::Future, sync::atomic::{AtomicI64, AtomicU64, Ordering}, time::{Duration, Instant}};
use tokio::sync::{Mutex, Notify};

pub(super) type Meeting = (String, String, String);
const FALLBACK: Duration = Duration::from_secs(60);
static REVISION: AtomicU64 = AtomicU64::new(1);
static SETTLE_UNTIL: AtomicI64 = AtomicI64::new(0);
static CACHE: Mutex<StateCache> = Mutex::const_new(StateCache { entry: None, sequence: 0 });
pub(super) static CHANGED: Notify = Notify::const_new();

struct Entry {
    identity: (String, String),
    revision: u64,
    expires: Instant,
    result: Result<Option<Meeting>, String>,
}
struct StateCache { entry: Option<Entry>, sequence: u64 }

impl StateCache {
    async fn read<F, Fut>(&mut self, identity: (String, String), revision: u64, fetch: F) -> Result<(Option<Meeting>, u64), String>
    where F: FnOnce() -> Fut, Fut: Future<Output=Result<Option<Meeting>, String>> {
        if let Some(entry) = &self.entry {
            if entry.identity == identity && entry.revision == revision && Instant::now() < entry.expires {
                return entry.result.clone().map(|m| (m, self.sequence));
            }
        }
        let result = fetch().await;
        self.sequence += 1;
        // Event delivery can precede the query API's state update. Briefly reconcile,
        // then return to the minute fallback rather than missing the first minute of audio.
        let ttl = if result.is_ok() && now() < SETTLE_UNTIL.load(Ordering::SeqCst) { Duration::from_secs(5) } else { FALLBACK };
        self.entry = Some(Entry { identity, revision, expires: Instant::now() + ttl, result: result.clone() });
        result.map(|m| (m, self.sequence))
    }
}

pub(super) fn invalidate() {
    REVISION.fetch_add(1, Ordering::SeqCst);
    CHANGED.notify_one();
}

// Confirm an empty observation promptly without counting a cached response twice.
pub(super) async fn confirm_absence() {
    if let Some(entry) = CACHE.lock().await.entry.as_mut() {
        entry.expires = entry.expires.min(Instant::now() + Duration::from_secs(5));
    }
}

pub(super) async fn current(pool: &SqlitePool) -> Result<(AppConfig, Option<Meeting>, u64), String> {
    let c = app::load(pool).await?;
    let mut cache = CACHE.lock().await;
    for _ in 0..2 {
        let revision = REVISION.load(Ordering::SeqCst);
        let (meeting, sequence) = cache.read((c.app_id.clone(), c.owner_id.clone()), revision, || async {
            // Refresh failures must also be cached; do not retry OAuth on every local tick.
            let authorized = oauth::authorized_config(pool).await?;
            if authorized.app_id != c.app_id || authorized.owner_id != c.owner_id {
                return Err("Lark 授权用户已变更，请重新核验会议。".into());
            }
            let v = app::api(reqwest::Method::GET, "/open-apis/vc/v1/bots/user_active_meeting", &authorized.user_token, None).await?;
            active_meeting(&v)
        }).await?;
        if revision == REVISION.load(Ordering::SeqCst) { return Ok((c, meeting, sequence)); }
    }
    Err("会议状态连续变化，请稍后重试。".into())
}

pub(super) fn supported(kind: &str) -> bool {
    matches!(kind, "vc.meeting.join_meeting_v1" | "vc.meeting.leave_meeting_v1" | "vc.meeting.meeting_ended_v1")
}

pub(super) async fn ended(pool: &SqlitePool, c: &AppConfig, id: &str) -> Result<bool, String> {
    Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM lark_meeting_signals WHERE app_id=? AND owner_id=? AND remote_id=? AND ended=1")
        .bind(&c.app_id).bind(&c.owner_id).bind(id).fetch_one(pool).await.map_err(db_error)? > 0)
}

// No network calls or microphone operations in the WS acknowledgement path.
pub(super) async fn receive(pool: &SqlitePool, c: &AppConfig, value: &Value) -> Result<(), String> {
    let kind = value["header"]["event_type"].as_str().unwrap_or("");
    if !c.enabled || c.owner_id.is_empty() || value["header"]["app_id"].as_str() != Some(c.app_id.as_str()) || !supported(kind) { return Ok(()); }
    let time = value["header"]["create_time"].as_str().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
    if time < (now() - 600) * 1000 || time > (now() + 60) * 1000 { return Ok(()); }
    let is_end = kind == "vc.meeting.meeting_ended_v1";
    if !is_end && value["event"]["operator"]["id"]["open_id"].as_str() != Some(c.owner_id.as_str()) { return Ok(()); }
    let id = value["event"]["meeting"]["id"].as_str().unwrap_or("");
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) { return Ok(()); }
    let event_id = value["header"]["event_id"].as_str().filter(|s| !s.is_empty()).ok_or("会议事件缺少 ID。")?;
    let number = value["event"]["meeting"]["meeting_no"].as_str().unwrap_or("");
    let mut tx = pool.begin().await.map_err(db_error)?;
    let inserted = sqlx::query("INSERT OR IGNORE INTO lark_app_events(event_id,received_at) VALUES(?,?)")
        .bind(event_id).bind(now()).execute(&mut *tx).await.map_err(db_error)?.rows_affected();
    let mut changed = false;
    if inserted > 0 {
        changed = sqlx::query("INSERT INTO lark_meeting_signals(app_id,owner_id,remote_id,meeting_no,event_kind,event_at,ended) VALUES(?,?,?,?,?,?,?) ON CONFLICT(app_id,owner_id,remote_id) DO UPDATE SET meeting_no=excluded.meeting_no,event_kind=excluded.event_kind,event_at=excluded.event_at,ended=excluded.ended WHERE lark_meeting_signals.ended=0 AND (excluded.ended=1 OR excluded.event_at>lark_meeting_signals.event_at OR (excluded.event_at=lark_meeting_signals.event_at AND excluded.event_kind='vc.meeting.leave_meeting_v1'))")
            .bind(&c.app_id).bind(&c.owner_id).bind(id).bind(number).bind(kind).bind(time).bind(is_end)
            .execute(&mut *tx).await.map_err(db_error)?.rows_affected() > 0;
    }
    tx.commit().await.map_err(db_error)?;
    if changed {
        log::info!("Lark meeting event accepted: type={kind}, meeting_id={id}");
        SETTLE_UNTIL.store(now() + 15, Ordering::SeqCst);
        invalidate();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn shared_cache_reuses_reads_and_refreshes_for_events_expiry_or_identity() {
        let mut cache = StateCache { entry:None, sequence:0 };
        let key = ("app".into(),"owner".into());
        let first = cache.read(key.clone(), 1, || async { Ok(Some(("1".into(),"123456789".into(),"test".into()))) }).await.unwrap();
        for _ in 0..100 {
            assert_eq!(cache.read(key.clone(),1,|| async { panic!("cache hit must not call API") }).await.unwrap(),first);
        }
        let changed = cache.read(key.clone(),2,|| async { Ok(None) }).await.unwrap();
        assert!(changed.0.is_none());
        assert_ne!(changed.1, first.1);
        cache.entry.as_mut().unwrap().expires = Instant::now();
        let expired = cache.read(key,2,|| async { Ok(None) }).await.unwrap();
        assert_ne!(expired.1,changed.1);
        let other = cache.read(("other-app".into(),"owner".into()),2,|| async { Ok(None) }).await.unwrap();
        assert_ne!(other.1,expired.1);
        let other_user = cache.read(("other-app".into(),"other-owner".into()),2,|| async { Ok(None) }).await.unwrap();
        assert_ne!(other.1,other_user.1);
    }

    #[tokio::test]
    async fn network_errors_are_cached_to_avoid_idle_retry_storms() {
        let mut cache = StateCache { entry:None, sequence:0 };
        let key = ("app".into(),"owner".into());
        assert!(cache.read(key.clone(),1,|| async { Err("offline".into()) }).await.is_err());
        assert!(cache.read(key.clone(),1,|| async { panic!("no immediate retry") }).await.is_err());
        assert!(cache.read(key,2,|| async { Ok(None) }).await.is_ok());
    }

    async fn fixture() -> (SqlitePool, AppConfig) {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        for migration in [include_str!("../../migrations/20261015000000_add_lark_app.sql"), include_str!("../../migrations/20261016000000_add_lark_meeting_signals.sql")] {
            sqlx::raw_sql(migration).execute(&pool).await.unwrap();
        }
        (pool, AppConfig { app_id:"app".into(), owner_id:"owner".into(), enabled:true, ..Default::default() })
    }

    fn event(kind: &str, id: &str, time: i64) -> Value {
        json!({"schema":"2.0","header":{"app_id":"app","event_id":id,"event_type":kind,"create_time":time.to_string()},
            "event":{"meeting":{"id":"123","meeting_no":"123456789"},"operator":{"id":{"open_id":"owner"}}}})
    }

    #[tokio::test]
    async fn events_deduplicate_ignore_reordering_and_keep_end_terminal() {
        let (pool,c) = fixture().await;
        let time = now()*1000;
        let join = event("vc.meeting.join_meeting_v1","a",time);
        receive(&pool,&c,&join).await.unwrap();
        receive(&pool,&c,&join).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM lark_app_events").fetch_one(&pool).await.unwrap(),1);
        receive(&pool,&c,&event("vc.meeting.leave_meeting_v1","b",time+1)).await.unwrap();
        receive(&pool,&c,&event("vc.meeting.join_meeting_v1","older",time)).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT event_kind FROM lark_meeting_signals").fetch_one(&pool).await.unwrap(),"vc.meeting.leave_meeting_v1");
        receive(&pool,&c,&event("vc.meeting.join_meeting_v1","rejoin",time+2)).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT event_kind FROM lark_meeting_signals").fetch_one(&pool).await.unwrap(),"vc.meeting.join_meeting_v1");
        let mut end = event("vc.meeting.meeting_ended_v1","end",time+3);
        end["event"]["operator"]["id"]["open_id"] = json!("other-host");
        receive(&pool,&c,&end).await.unwrap();
        receive(&pool,&c,&event("vc.meeting.join_meeting_v1","late",time+4)).await.unwrap();
        assert!(ended(&pool,&c,"123").await.unwrap());
        assert!(!ended(&pool,&AppConfig { owner_id:"other".into(),..c },"123").await.unwrap());
    }

    #[tokio::test]
    async fn rejects_other_users_apps_stale_events_and_invalid_meeting_ids() {
        let (pool,c) = fixture().await;
        let base = event("vc.meeting.join_meeting_v1","a",now()*1000);
        let mut other = base.clone(); other["event"]["operator"]["id"]["open_id"] = json!("someone-else");
        let mut app = base.clone(); app["header"]["app_id"] = json!("other-app");
        let mut invalid = base.clone(); invalid["event"]["meeting"]["id"] = json!("bad-id");
        for value in [other,app,invalid,event("vc.meeting.leave_meeting_v1","old",(now()-601)*1000),event("im.message.receive_v1","wrong",now()*1000)] {
            receive(&pool,&c,&value).await.unwrap();
        }
        receive(&pool,&AppConfig { enabled:false,..c },&base).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM lark_meeting_signals").fetch_one(&pool).await.unwrap(),0);
    }
}
