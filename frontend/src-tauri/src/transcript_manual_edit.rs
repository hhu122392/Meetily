use crate::{state::AppState, transcript_proofread::{apply_edits, ProofreadApplyResponse, ProofreadEdit}};
use std::path::PathBuf;

/// Main's segment editor, using the custom version's guarded backup/sync pipeline.
#[tauri::command]
pub async fn api_update_transcript_segment(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    transcript_id: String,
    text: String,
    expected_text: String,
) -> Result<ProofreadApplyResponse, String> {
    let _storage = crate::audio::storage_migration::use_storage().await?;
    let pool = state.db_manager.pool();
    let stored: Option<Option<String>> = sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id=?")
        .bind(&meeting_id).fetch_optional(pool).await.map_err(|e| e.to_string())?;
    let stored = stored.flatten().ok_or("当前公司中不存在该会议或录音目录")?;
    let folder = crate::transcript_correction_context::checked_folder(pool, &meeting_id, &stored).await?;
    save_segment(pool, meeting_id, folder, transcript_id, text, expected_text).await
}

pub(crate) async fn save_segment(
    pool: &sqlx::SqlitePool, meeting_id: String, folder: PathBuf,
    transcript_id: String, text: String, expected_text: String,
) -> Result<ProofreadApplyResponse, String> {
    let text = text.trim().to_owned();
    if text.is_empty() { return Err("转写内容不能为空".into()); }
    if text == expected_text { return Err("转写内容没有变化".into()); }
    let end_char = expected_text.chars().count();
    apply_edits(pool, meeting_id, folder, vec![ProofreadEdit {
        segment_id: transcript_id, original: expected_text.clone(), suggested: text,
        expected_text, start_char: 0, end_char,
    }]).await
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn fixture() -> (sqlx::SqlitePool, tempfile::TempDir) {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("CREATE TABLE transcripts(id TEXT PRIMARY KEY, meeting_id TEXT, transcript TEXT, timestamp TEXT, audio_start_time REAL, audio_end_time REAL, duration REAL, speaker TEXT, summary TEXT, action_items TEXT, key_points TEXT); INSERT INTO transcripts VALUES('s','m','原文🙂','0',2,5,3,'person','summary',NULL,NULL); INSERT INTO transcripts VALUES('other','other-meeting','其他会议','0',0,1,1,NULL,NULL,NULL,NULL);").execute(&pool).await.unwrap();
        (pool, tempfile::tempdir().unwrap())
    }
    #[tokio::test]
    async fn manual_edit_persists_unicode_with_backup_and_preserves_metadata() {
        let (pool, dir) = fixture().await;
        let result = save_segment(&pool, "m".into(), dir.path().into(), "s".into(), " 修改后🙂\n第二行 ".into(), "原文🙂".into()).await.unwrap();
        assert!(!result.file_sync_pending);
        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.path().join("transcripts.json")).unwrap()).unwrap();
        assert_eq!(saved["segments"][0]["text"], "修改后🙂\n第二行");
        assert_eq!(saved["segments"][0]["audio_start_time"], 2.0);
        let backup: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.path().join(result.backup_file.unwrap())).unwrap()).unwrap();
        assert_eq!(backup["segments"][0]["text"], "原文🙂");
        let metadata: (String, String) = sqlx::query_as("SELECT speaker,summary FROM transcripts WHERE id='s'").fetch_one(&pool).await.unwrap();
        assert_eq!(metadata, ("person".into(), "summary".into()));
    }
    #[tokio::test]
    async fn manual_edit_rejects_stale_source_empty_text_and_other_meeting_segment() {
        let (pool, dir) = fixture().await;
        for (id, text, expected) in [("s", "新内容", "过期原文"), ("s", "  ", "原文🙂"), ("other", "新内容", "其他会议")] {
            assert!(save_segment(&pool, "m".into(), dir.path().into(), id.into(), text.into(), expected.into()).await.is_err());
        }
        let text: String = sqlx::query_scalar("SELECT transcript FROM transcripts WHERE id='s'").fetch_one(&pool).await.unwrap();
        assert_eq!(text, "原文🙂");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
    #[tokio::test]
    async fn manual_edit_can_fill_an_empty_segment() {
        let (pool, dir) = fixture().await;
        sqlx::query("UPDATE transcripts SET transcript='' WHERE id='s'").execute(&pool).await.unwrap();
        save_segment(&pool, "m".into(), dir.path().into(), "s".into(), "补充文字".into(), "".into()).await.unwrap();
        let text: String = sqlx::query_scalar("SELECT transcript FROM transcripts WHERE id='s'").fetch_one(&pool).await.unwrap();
        assert_eq!(text, "补充文字");
    }
}
