//! Durable file synchronization after a transcript database transaction.
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use std::{io::Write, path::Path};

pub static WRITE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub fn read_metadata(folder: &Path) -> Result<Value> {
    match std::fs::read(folder.join("metadata.json")) {
        Ok(bytes) => {
            let value: Value = serde_json::from_slice(&bytes)?;
            if !value.is_object() { return Err(anyhow!("metadata must be an object")); }
            Ok(value)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({"version":"1.0"})),
        Err(error) => Err(error.into()),
    }
}

pub fn write_new_backup(folder: &Path, prefix: &str, value: &Value) -> Result<String> {
    let name = format!("{}{}-{}.json", prefix, chrono::Local::now().format("%Y%m%d-%H%M%S"), uuid::Uuid::new_v4());
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(folder.join(&name))?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    Ok(name)
}

/// Old retranscription backups recorded the requested engine, not the previous one.
pub fn backup_source(value: &Value) -> (Option<String>, Option<String>) {
    if value.get("kind").and_then(Value::as_str) == Some("pre-retranscription-backup") && value["version"] != "1.1" {
        return (None, None);
    }
    (value["transcription_provider"].as_str().map(str::to_owned), value["transcription_model"].as_str().map(str::to_owned))
}

pub async fn ensure_table(pool: &SqlitePool) -> Result<()> {
    sqlx::query("CREATE TABLE IF NOT EXISTS transcript_file_sync (meeting_id TEXT PRIMARY KEY, folder TEXT NOT NULL, metadata_patch TEXT NOT NULL)")
        .execute(pool).await?;
    Ok(())
}

pub async fn pending(pool: &SqlitePool, meeting_id: &str) -> Result<bool> {
    ensure_table(pool).await?;
    Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM transcript_file_sync WHERE meeting_id = ?").bind(meeting_id).fetch_one(pool).await? > 0)
}

/// Call in the same transaction that changes transcript rows.
pub async fn stage(tx: &mut Transaction<'_, Sqlite>, meeting_id: &str, folder: &Path, metadata_patch: Value) -> Result<()> {
    sqlx::query("INSERT INTO transcript_file_sync (meeting_id, folder, metadata_patch) VALUES (?, ?, ?) ON CONFLICT(meeting_id) DO UPDATE SET folder=excluded.folder, metadata_patch=excluded.metadata_patch")
        .bind(meeting_id).bind(folder.to_string_lossy().as_ref()).bind(serde_json::to_string(&metadata_patch)?)
        .execute(&mut **tx).await?;
    Ok(())
}

fn atomic_json(path: &Path, value: &Value) -> Result<()> {
    let mut temp = tempfile::NamedTempFile::new_in(path.parent().ok_or_else(|| anyhow!("missing parent"))?)?;
    temp.write_all(&serde_json::to_vec_pretty(value)?)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

/// Caller holds WRITE_LOCK. Retry reads current database text so later manual edits survive.
pub async fn flush(pool: &SqlitePool, meeting_id: &str) -> Result<()> {
    ensure_table(pool).await?;
    let Some(record) = sqlx::query("SELECT folder, metadata_patch FROM transcript_file_sync WHERE meeting_id = ?")
        .bind(meeting_id).fetch_optional(pool).await? else { return Ok(()); };
    let folder: String = record.try_get("folder")?;
    let folder = Path::new(&folder);
    let patch: Value = serde_json::from_str(record.try_get::<&str, _>("metadata_patch")?)?;
    let rows = sqlx::query("SELECT id, transcript, timestamp, audio_start_time, audio_end_time, duration FROM transcripts WHERE meeting_id = ? ORDER BY COALESCE(audio_start_time, 0.0), timestamp")
        .bind(meeting_id).fetch_all(pool).await?;
    let segments: Vec<Value> = rows.iter().enumerate().map(|(i, row)| -> Result<Value> { Ok(json!({
        "id": row.try_get::<String, _>("id")?, "text": row.try_get::<String, _>("transcript")?,
        "timestamp": row.try_get::<String, _>("timestamp")?, "sequence_id": i,
        "audio_start_time": row.try_get::<Option<f64>, _>("audio_start_time")?,
        "audio_end_time": row.try_get::<Option<f64>, _>("audio_end_time")?, "duration": row.try_get::<Option<f64>, _>("duration")?,
    })) }).collect::<Result<_>>()?;
    atomic_json(&folder.join("transcripts.json"), &json!({"version":"1.0", "last_updated": chrono::Utc::now().to_rfc3339(), "total_segments":segments.len(), "segments":segments}))?;
    if let Some(patch) = patch.as_object().filter(|patch| !patch.is_empty()) {
        let mut metadata = read_metadata(folder)?;
        let object = metadata.as_object_mut().ok_or_else(|| anyhow!("invalid metadata"))?;
        for (key, value) in patch { object.insert(key.clone(), value.clone()); }
        object.remove("detected_summary_language");
        atomic_json(&folder.join("metadata.json"), &metadata)?;
    }
    sqlx::query("DELETE FROM transcript_file_sync WHERE meeting_id = ?").bind(meeting_id).execute(pool).await?;
    Ok(())
}

/// Complete earlier writes before taking another backup or changing model provenance.
pub async fn ensure_ready(pool: &SqlitePool, meeting_id: &str) -> Result<(), String> {
    flush(pool, meeting_id).await.map_err(|error| {
        log::warn!("Transcript files pending for {meeting_id}: {error}");
        "transcript_files_pending".into()
    })
}

pub async fn finish(pool: &SqlitePool, meeting_id: &str) -> bool {
    if let Err(error) = flush(pool, meeting_id).await {
        log::warn!("Database saved; transcript files pending for {meeting_id}: {error}");
        true
    } else { false }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rapid_backups_are_unique_and_old_retranscription_source_is_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let value = json!({"text":"original"});
        let a = write_new_backup(dir.path(), "before-", &value).unwrap();
        let b = write_new_backup(dir.path(), "before-", &json!({"text":"changed"})).unwrap();
        assert_ne!(a, b);
        assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(dir.path().join(a)).unwrap()).unwrap(), value);
        assert_eq!(backup_source(&json!({"kind":"pre-retranscription-backup","version":"1.0","transcription_provider":"new-engine"})), (None, None));
        assert_eq!(backup_source(&json!({"kind":"pre-retranscription-backup","version":"1.1","transcription_provider":"old-engine"})).0.as_deref(), Some("old-engine"));
    }
    #[tokio::test]
    async fn file_failure_survives_restart_and_retry_uses_current_text() {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite:{}?mode=rwc", dir.path().join("db.sqlite").display());
        let pool = SqlitePool::connect(&url).await.unwrap();
        sqlx::query("CREATE TABLE transcripts(id TEXT, meeting_id TEXT, transcript TEXT, timestamp TEXT, audio_start_time REAL, audio_end_time REAL, duration REAL)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO transcripts VALUES('s','m','old','0',0,1,1)").execute(&pool).await.unwrap();
        ensure_table(&pool).await.unwrap();
        std::fs::write(dir.path().join("metadata.json"), r#"{"transcription_provider":"wrong","recording_context":{"name":"preserved"}}"#).unwrap();
        std::fs::create_dir(dir.path().join("transcripts.json")).unwrap(); // Force file replacement to fail.
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("UPDATE transcripts SET transcript='committed'").execute(&mut *tx).await.unwrap();
        stage(&mut tx, "m", dir.path(), json!({"transcription_provider":null,"transcription_model":null})).await.unwrap();
        tx.commit().await.unwrap();
        assert!(finish(&pool, "m").await);
        assert!(pending(&pool, "m").await.unwrap());
        pool.close().await;
        let pool = SqlitePool::connect(&url).await.unwrap();
        assert!(pending(&pool, "m").await.unwrap());
        std::fs::remove_dir(dir.path().join("transcripts.json")).unwrap();
        sqlx::query("UPDATE transcripts SET transcript='manual edit'").execute(&pool).await.unwrap();
        flush(&pool, "m").await.unwrap();
        assert!(!pending(&pool, "m").await.unwrap());
        let data: Value = serde_json::from_slice(&std::fs::read(dir.path().join("transcripts.json")).unwrap()).unwrap();
        assert_eq!(data["segments"][0]["text"], "manual edit");
        let metadata = read_metadata(dir.path()).unwrap();
        assert!(metadata["transcription_provider"].is_null());
        assert_eq!(metadata["recording_context"]["name"], "preserved");
    }
}
