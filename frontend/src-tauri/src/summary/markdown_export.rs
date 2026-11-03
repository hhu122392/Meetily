use std::{io::Write, path::{Path, PathBuf}};
use serde_json::Value;
use sqlx::{Row, SqlitePool};
use tauri::{AppHandle, Manager, Runtime};
use crate::state::AppState;

pub async fn export(pool: &SqlitePool, id: &str, fallback: Option<&Path>, generated: bool) -> Result<String, String> {
    let edited: bool = sqlx::query_scalar::<_,i64>("SELECT manually_edited FROM summary_markdown_exports WHERE meeting_id=?")
        .bind(id).fetch_optional(pool).await.map_err(|e| e.to_string())?.unwrap_or(0) != 0;
    if edited {
        let existing = preview(pool, id).await?.ok_or("独立编辑的 Markdown 不可用，请检查原文件。")?;
        if !generated { return existing["path"].as_str().map(str::to_owned).ok_or("Markdown 路径无效。".into()); }
        // Preserve the customized version before a newly generated summary replaces it.
        archive_current(&existing).await?;
    }
    let result = write_export(pool, id, fallback, generated).await;
    let (path, error) = match &result {
        Ok(path) => (Some(path.as_str()), None),
        Err(error) => (None, Some(error.as_str())),
    };
    sqlx::query("INSERT INTO summary_markdown_exports(meeting_id,path,error,updated_at) VALUES(?,?,?,?) ON CONFLICT(meeting_id) DO UPDATE SET path=COALESCE(excluded.path,path),error=excluded.error,updated_at=excluded.updated_at")
        .bind(id).bind(path).bind(error).bind(chrono::Utc::now().to_rfc3339())
        .execute(pool).await.map_err(|e| format!("无法保存 Markdown 导出状态：{e}"))?;
    if result.is_ok() && generated {
        sqlx::query("UPDATE summary_markdown_exports SET manually_edited=0 WHERE meeting_id=?").bind(id).execute(pool).await.map_err(|e| e.to_string())?;
    }
    result
}

async fn archive_current(file: &Value) -> Result<(), String> {
    let path = PathBuf::from(file["path"].as_str().ok_or("Markdown 路径无效。")?);
    let dir = path.parent().ok_or("Markdown 目录无效。")?.to_path_buf();
    let content = file["content"].as_str().ok_or("Markdown 内容无效。")?.to_owned();
    tokio::task::spawn_blocking(move || write_files(&dir, &content, true)).await.map_err(|e| e.to_string())??;
    Ok(())
}

async fn save_file(pool: &SqlitePool, id: &str, expected_hash: &str, content: &str) -> Result<Value, String> {
    if content.trim().is_empty() || content.len() > 2 * 1024 * 1024 {
        return Err("Markdown 内容不能为空，且不能超过 2 MiB。".into());
    }
    let current = preview(pool, id).await?.ok_or("Markdown 文件不存在，请先保存纪要。")?;
    if current["hash"].as_str() != Some(expected_hash) { return Err("Markdown 文件已被其他操作修改，请重新打开后编辑；本次没有覆盖。".into()); }
    if current["content"].as_str() == Some(content) { return Ok(current); }
    archive_current(&current).await?;
    // Mark protected before writing: a failed file replacement leaves the old file intact,
    // and subsequent page saves cannot silently overwrite a separately edited document.
    sqlx::query("UPDATE summary_markdown_exports SET manually_edited=1,error=NULL,updated_at=? WHERE meeting_id=?")
        .bind(chrono::Utc::now().to_rfc3339()).bind(id).execute(pool).await.map_err(|e| e.to_string())?;
    let dir = PathBuf::from(current["path"].as_str().unwrap()).parent().ok_or("Markdown 目录无效。")?.to_path_buf();
    let content = content.to_owned();
    tokio::task::spawn_blocking(move || write_files(&dir, &content, false)).await.map_err(|e| e.to_string())??;
    preview(pool, id).await?.ok_or("文件已保存，但无法重新读取，请检查目录。".into())
}

#[tauri::command]
pub async fn summary_markdown_read(state: tauri::State<'_, AppState>, meeting_id: String) -> Result<Value, String> {
    let _storage = crate::audio::storage_migration::use_storage().await?;
    let _guard = crate::lark::app::DELIVERY_LOCK.lock().await;
    preview(state.db_manager.pool(), &meeting_id).await?.ok_or("Markdown 文件尚未生成，请先保存纪要。".into())
}

#[tauri::command]
pub async fn summary_markdown_save(state: tauri::State<'_, AppState>, meeting_id: String, expected_hash: String, content: String) -> Result<Value, String> {
    let _storage = crate::audio::storage_migration::use_storage().await?;
    let _guard = crate::lark::app::DELIVERY_LOCK.lock().await;
    save_file(state.db_manager.pool(), &meeting_id, &expected_hash, &content).await
}

async fn write_export(pool: &SqlitePool, id: &str, fallback: Option<&Path>, generated: bool) -> Result<String, String> {
    let row = sqlx::query("SELECT m.title,m.created_at,m.folder_path,s.result FROM meetings m JOIN summary_processes s ON s.meeting_id=m.id WHERE m.id=?")
        .bind(id).fetch_optional(pool).await.map_err(|e| e.to_string())?.ok_or("会议纪要不存在")?;
    let result: Value = serde_json::from_str(&row.get::<String,_>("result")).map_err(|_| "纪要格式异常")?;
    let body = crate::lark::routing::summary_text(&result)?;
    let folder: Option<String> = row.get("folder_path");
    let dir = match folder.filter(|s| !s.trim().is_empty()) {
        Some(folder) => PathBuf::from(folder),
        None => fallback.ok_or("无法确定 Markdown 保存目录")?.join("summary-exports").join(crate::lark::payload::fingerprint(&[id])),
    };
    if !dir.is_absolute() { return Err("会议目录必须是绝对路径".into()); }
    let content = format!("# {}\n\n会议时间：{}\n\n本地会议 ID：{}\n\n{}\n",
        row.get::<String,_>("title").replace(['\r','\n'], " "), row.get::<String,_>("created_at"), id, body);
    tokio::task::spawn_blocking(move || write_files(&dir, &content, generated))
        .await.map_err(|e| e.to_string())?
}

fn write_files(dir: &Path, content: &str, generated: bool) -> Result<String, String> {
    let write = || -> std::io::Result<String> {
        std::fs::create_dir_all(dir)?;
        if generated {
            let versions = dir.join("纪要版本");
            std::fs::create_dir_all(&versions)?;
            let name = format!("会议纪要-{}-{}.md", chrono::Local::now().format("%Y%m%d-%H%M%S"), uuid::Uuid::new_v4());
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(versions.join(name))?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
        }
        let latest = dir.join("会议纪要.md");
        let mut temporary = tempfile::NamedTempFile::new_in(dir)?;
        temporary.write_all(content.as_bytes())?;
        temporary.as_file().sync_all()?;
        temporary.persist(&latest).map_err(|e| e.error)?;
        Ok(latest.to_string_lossy().into_owned())
    };
    write().map_err(|e| format!("纪要已保存在 Meetily，但 Markdown 文件保存失败：{e}。请检查目录权限或磁盘空间后重试。"))
}

pub async fn preview(pool: &SqlitePool, id: &str) -> Result<Option<Value>, String> {
    let path: Option<String> = sqlx::query_scalar("SELECT path FROM summary_markdown_exports WHERE meeting_id=? AND error IS NULL")
        .bind(id).fetch_optional(pool).await.map_err(|e| e.to_string())?.flatten();
    let Some(path) = path else { return Ok(None); };
    let metadata = tokio::fs::metadata(&path).await.map_err(|_| "Markdown 文件不存在，请重新保存。")?;
    if metadata.len() > 2 * 1024 * 1024 { return Err("Markdown 文件超过 2 MiB，请精简纪要后重新保存。".into()); }
    let content = tokio::fs::read_to_string(&path).await.map_err(|_| "无法读取 Markdown 文件，请重新保存。")?;
    if content.len() > 2 * 1024 * 1024 { return Err("Markdown 文件过大。".into()); }
    let name = Path::new(&path).file_name().and_then(|n| n.to_str()).ok_or("Markdown 文件名无效。")?;
    Ok(Some(serde_json::json!({"path":path,"name":name,"content":content,"size":content.len(),"hash":crate::lark::payload::fingerprint(&[&content])})))
}

#[tauri::command]
pub async fn summary_markdown_status(state: tauri::State<'_, AppState>, meeting_id: String) -> Result<Value, String> {
    let row = sqlx::query("SELECT path,error FROM summary_markdown_exports WHERE meeting_id=?")
        .bind(meeting_id).fetch_optional(state.db_manager.pool()).await.map_err(|e| e.to_string())?;
    Ok(match row { Some(r) => serde_json::json!({"path":r.get::<Option<String>,_>("path"),"error":r.get::<Option<String>,_>("error")}), None => serde_json::json!({}) })
}

#[tauri::command]
pub async fn summary_export_markdown<R: Runtime>(app: AppHandle<R>, state: tauri::State<'_, AppState>, meeting_id: String) -> Result<String, String> {
    let _storage = crate::audio::storage_migration::use_storage().await?;
    let _guard = crate::lark::app::DELIVERY_LOCK.lock().await;
    let fallback = crate::company::data_dir(&app).ok();
    export(state.db_manager.pool(), &meeting_id, fallback.as_deref(), false).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn manual_edit_is_preserved_until_regeneration_and_stale_save_is_rejected() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("CREATE TABLE meetings(id TEXT,title TEXT,created_at TEXT,folder_path TEXT); CREATE TABLE summary_processes(meeting_id TEXT,result TEXT); CREATE TABLE summary_markdown_exports(meeting_id TEXT PRIMARY KEY,path TEXT,error TEXT,updated_at TEXT,manually_edited INTEGER NOT NULL DEFAULT 0);").execute(&pool).await.unwrap();
        let root = tempfile::tempdir().unwrap();
        sqlx::query("INSERT INTO meetings VALUES('test','会议','2026-10-19',?)").bind(root.path().to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO summary_processes VALUES('test',?)").bind(serde_json::json!({"markdown":"原始详细纪要"}).to_string()).execute(&pool).await.unwrap();
        export(&pool, "test", None, true).await.unwrap();
        let original = preview(&pool, "test").await.unwrap().unwrap();
        let edited = save_file(&pool, "test", original["hash"].as_str().unwrap(), "# 自定义详细纪要\n- 决定使用 Lark\n- 负责人：小明").await.unwrap();
        assert!(save_file(&pool, "test", original["hash"].as_str().unwrap(), "过期编辑").await.is_err());
        export(&pool, "test", None, false).await.unwrap();
        assert_eq!(preview(&pool, "test").await.unwrap().unwrap()["content"], edited["content"]);
        sqlx::query("UPDATE summary_processes SET result=?").bind(serde_json::json!({"markdown":"重新生成的纪要"}).to_string()).execute(&pool).await.unwrap();
        export(&pool, "test", None, true).await.unwrap();
        assert!(preview(&pool, "test").await.unwrap().unwrap()["content"].as_str().unwrap().contains("重新生成的纪要"));
        let versions = std::fs::read_dir(root.path().join("纪要版本")).unwrap().map(|p| std::fs::read_to_string(p.unwrap().path()).unwrap()).collect::<Vec<_>>();
        assert!(versions.iter().any(|s| s.contains("自定义详细纪要")));
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT manually_edited FROM summary_markdown_exports").fetch_one(&pool).await.unwrap(), 0);
    }
    #[test]
    fn generated_versions_survive_edits_and_unicode_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_files(dir.path(), "# 中文纪要\n决议：使用 Lark", true).unwrap();
        write_files(dir.path(), "# 修订稿\n责任人：小明", false).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "# 修订稿\n责任人：小明");
        let versions: Vec<_> = std::fs::read_dir(dir.path().join("纪要版本")).unwrap().collect();
        assert_eq!(versions.len(), 1);
        assert_eq!(std::fs::read_to_string(versions[0].as_ref().unwrap().path()).unwrap(), "# 中文纪要\n决议：使用 Lark");
        write_files(dir.path(), "第二次生成", true).unwrap();
        assert_eq!(std::fs::read_dir(dir.path().join("纪要版本")).unwrap().count(), 2);
    }
    #[test]
    fn invalid_destination_is_reported_without_overwriting_file() {
        let file = tempfile::NamedTempFile::new().unwrap();
        assert!(write_files(file.path(), "内容", false).unwrap_err().contains("保存失败"));
        assert_eq!(std::fs::metadata(file.path()).unwrap().len(), 0);
    }
}
