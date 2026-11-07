//! Adapt main's correction pipeline to the custom version's company-scoped meeting settings.
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};

pub struct RecognitionContext {
    pub canonical_names: Vec<String>,
    pub canonical_terms: Vec<String>,
}

impl RecognitionContext {
    pub fn normalize_transcript(&self, input: &str) -> String {
        // Names provide AI context, never inferred deterministic substitutions.
        input.to_owned()
    }
}

pub async fn load_context(pool: &SqlitePool, meeting_id: &str) -> Result<Option<RecognitionContext>, String> {
    let settings = crate::summary::meeting_settings::read(pool, meeting_id).await?;
    let mut names: Vec<String> = [&settings.participants, &settings.host, &settings.absent]
        .into_iter().flat_map(|text| text.split(|c: char| c.is_whitespace() || matches!(c, ',' | '，' | '、' | ';' | '；')))
        .map(str::trim).filter(|name| !name.is_empty()).map(str::to_owned).collect();
    names.sort(); names.dedup();
    Ok(Some(RecognitionContext { canonical_names: names, canonical_terms: Vec::new() }))
}

pub async fn checked_folder(pool: &SqlitePool, meeting_id: &str, supplied: &str) -> Result<PathBuf, String> {
    let stored: Option<Option<String>> = sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id=?")
        .bind(meeting_id).fetch_optional(pool).await.map_err(|e| e.to_string())?;
    let stored = stored.ok_or("当前公司中不存在该会议")?.ok_or("会议没有录音目录")?;
    let expected = std::fs::canonicalize(stored).map_err(|_| "会议录音目录不存在")?;
    let actual = std::fs::canonicalize(Path::new(supplied)).map_err(|_| "会议录音目录不存在")?;
    if expected != actual || !actual.is_dir() { return Err("录音目录不属于当前会议".into()); }
    if crate::audio::retranscription::is_retranscription_in_progress() {
        return Err("正在重新转写，请完成后再纠错".into());
    }
    crate::audio::transcription::completion::ensure_meeting_complete(pool, meeting_id).await?;
    Ok(actual)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn folder_must_belong_to_meeting_in_current_database() {
        let pool=SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("CREATE TABLE meetings(id TEXT PRIMARY KEY,folder_path TEXT);CREATE TABLE recording_transcription_checks(folder_path TEXT,status TEXT,detail TEXT);").execute(&pool).await.unwrap();
        let own=tempfile::tempdir().unwrap(); let other=tempfile::tempdir().unwrap();
        sqlx::query("INSERT INTO meetings VALUES('own',?)").bind(own.path().to_str().unwrap()).execute(&pool).await.unwrap();
        assert!(checked_folder(&pool,"own",own.path().to_str().unwrap()).await.is_ok());
        assert!(checked_folder(&pool,"other-company",own.path().to_str().unwrap()).await.is_err());
        assert!(checked_folder(&pool,"own",other.path().to_str().unwrap()).await.is_err());
    }
}
