use sqlx::SqlitePool;
use std::time::Duration;
use tokio::task::JoinHandle;

// A slow worker still owns its model. Never detach it and unload the model underneath it.
pub async fn wait_for_worker(
    mut task: JoinHandle<Result<(), String>>,
    slow_after: Duration,
    on_slow: impl FnOnce(),
) -> Result<(), String> {
    let result = match tokio::time::timeout(slow_after, &mut task).await {
        Ok(result) => result,
        Err(_) => { on_slow(); task.await }
    };
    result.map_err(|_| "转写任务异常退出，请使用保存的音频重新转写。".to_string())?
}

pub fn validate_counts(queued: u64, completed: u64, failures: u64) -> Result<(), String> {
    if failures > 0 || queued != completed {
        Err(format!("转写不完整：已处理 {completed}/{queued} 个片段，失败 {failures} 个；请重新转写，暂不生成或发送纪要。"))
    } else { Ok(()) }
}

pub async fn set_status(pool: &SqlitePool, folder: &str, status: &str, detail: &str) -> Result<(), String> {
    sqlx::query("INSERT INTO recording_transcription_checks(folder_path,status,detail,updated_at) VALUES(?,?,?,?) ON CONFLICT(folder_path) DO UPDATE SET status=excluded.status,detail=excluded.detail,updated_at=excluded.updated_at")
        .bind(folder).bind(status).bind(detail).bind(chrono::Utc::now().timestamp())
        .execute(pool).await.map_err(|_| "无法保存转写完整性状态。".to_string())?;
    Ok(())
}

pub async fn ensure_meeting_complete(pool: &SqlitePool, meeting: &str) -> Result<(), String> {
    let problem: Option<String> = sqlx::query_scalar("SELECT c.detail FROM meetings m JOIN recording_transcription_checks c ON c.folder_path=m.folder_path WHERE m.id=? AND c.status!='completed'")
        .bind(meeting).fetch_optional(pool).await.map_err(|_| "无法检查转写完整性。".to_string())?;
    match problem {
        Some(detail) => Err(if detail.is_empty() { "转写尚未完整完成，暂不生成或发送纪要。".into() } else { detail }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn slow_worker_remains_owned_and_is_awaited() {
        let (send, receive) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(async move { receive.await.unwrap(); Ok(()) });
        let wait = tokio::spawn(wait_for_worker(worker, Duration::from_millis(1), || {}));
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!wait.is_finished());
        send.send(()).unwrap();
        assert!(wait.await.unwrap().is_ok());
        assert!(validate_counts(22,22,19).is_err());
        assert!(validate_counts(22,3,0).is_err());
        assert!(validate_counts(22,22,0).is_ok());
    }
    #[tokio::test]
    async fn incomplete_meeting_is_blocked_until_successful_retranscription() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("CREATE TABLE meetings(id TEXT PRIMARY KEY,folder_path TEXT);").execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../../migrations/20261015030000_add_transcription_checks.sql")).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO meetings VALUES('a','folder-a'),('b','folder-b')").execute(&pool).await.unwrap();
        set_status(&pool,"folder-a","processing","正在转写").await.unwrap();
        assert!(ensure_meeting_complete(&pool,"a").await.is_err());
        assert!(ensure_meeting_complete(&pool,"b").await.is_ok());
        set_status(&pool,"folder-a","failed","转写失败").await.unwrap();
        assert!(ensure_meeting_complete(&pool,"a").await.is_err());
        set_status(&pool,"folder-a","completed","").await.unwrap();
        assert!(ensure_meeting_complete(&pool,"a").await.is_ok());
    }
}
