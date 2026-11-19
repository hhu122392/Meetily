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
    let name=path.file_name().and_then(|n|n.to_str()).ok_or("文件名无效")?.to_owned();
    tokio::task::spawn_blocking(move || write_named_files(&dir, &name, &content, true)).await.map_err(|e| e.to_string())??;
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
    let name=current["name"].as_str().ok_or("文件名无效")?.to_owned();
    let content = content.to_owned();
    tokio::task::spawn_blocking(move || write_named_files(&dir, &name, &content, false)).await.map_err(|e| e.to_string())??;
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
    let existing:Option<String>=sqlx::query_scalar("SELECT path FROM summary_markdown_exports WHERE meeting_id=?").bind(id).fetch_optional(pool).await.map_err(|e|e.to_string())?.flatten();
    let name=existing.as_deref().and_then(|p|Path::new(p).file_name()).and_then(|n|n.to_str()).unwrap_or("会议纪要.md").to_owned();
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
        row.get::<String,_>("title").replace(['\r','\n'], " "), format_meeting_time(&row.get::<String,_>("created_at")), id, body);
    tokio::task::spawn_blocking(move || write_named_files(&dir, &name, &content, generated))
        .await.map_err(|e| e.to_string())?
}

fn format_meeting_time(raw: &str) -> String {
    let timestamp = chrono::DateTime::parse_from_rfc3339(raw.trim())
        .map(|time| time.with_timezone(&chrono::Utc)).ok()
        // Legacy SQLite meeting timestamps without an offset are stored in UTC.
        .or_else(|| chrono::NaiveDateTime::parse_from_str(raw.trim(), "%Y-%m-%d %H:%M:%S%.f")
            .ok().map(|time| time.and_utc()));
    timestamp.map(|time| format!("{}（东八区 UTC+8）",
        time.with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).unwrap())
            .format("%Y-%m-%d %H:%M:%S")))
        // Keep incomplete historical dates instead of inventing a time.
        .unwrap_or_else(|| raw.to_owned())
}

fn write_files(dir: &Path, content: &str, generated: bool) -> Result<String, String> {
    write_named_files(dir,"会议纪要.md",content,generated)
}
pub(crate) fn validate_filename(name:&str)->Result<String,String>{
    let name=name.trim();
    if name.is_empty()||name.chars().count()>120||name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])||name.chars().any(char::is_control)||name.ends_with(['.',' ']) {return Err("文件名需为 1–120 字，不能包含路径或特殊字符。".into());}
    let name=if name.to_ascii_lowercase().ends_with(".md"){name.to_owned()}else{format!("{name}.md")};
    let base=name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    if base.is_empty()||["CON","PRN","AUX","NUL","COM1","COM2","COM3","COM4","COM5","COM6","COM7","COM8","COM9","LPT1","LPT2","LPT3","LPT4","LPT5","LPT6","LPT7","LPT8","LPT9"].contains(&base.as_str()) {return Err("请换一个有效的 Markdown 文件名。".into());}
    Ok(name)
}
async fn rename_file(pool:&SqlitePool,id:&str,expected_hash:&str,expected_name:&str,name:&str)->Result<Value,String>{
    let name=validate_filename(name)?;
    let current=preview(pool,id).await?.ok_or("Markdown 文件不存在")?;
    if current["hash"]!=expected_hash||current["name"]!=expected_name{return Err("文件内容或名称已变化，请重新打开后修改。".into());}
    if current["name"]==name{return Ok(current);}
    let old=PathBuf::from(current["path"].as_str().ok_or("文件路径无效")?);
    let dest=old.parent().ok_or("文件目录无效")?.join(&name);
    // Create without replacing another file; keep the original until the DB points to the new file.
    let mut output=std::fs::OpenOptions::new().write(true).create_new(true).open(&dest).map_err(|_|"同名文件已存在或目录不可写，请换一个名称。")?;
    let write_result=output.write_all(current["content"].as_str().unwrap_or("").as_bytes()).and_then(|_|output.sync_all());drop(output);
    if let Err(e)=write_result {let _=std::fs::remove_file(&dest);return Err(format!("改名未完成，原文件保留：{e}"));}
    if let Err(e)=sqlx::query("UPDATE summary_markdown_exports SET path=?,updated_at=? WHERE meeting_id=?").bind(dest.to_string_lossy().as_ref()).bind(chrono::Utc::now().to_rfc3339()).bind(id).execute(pool).await {let _=std::fs::remove_file(&dest);return Err(e.to_string());}
    let _=std::fs::remove_file(&old);
    preview(pool,id).await?.ok_or("改名后文件无法读取".into())
}
#[tauri::command]
pub async fn summary_markdown_rename(state:tauri::State<'_,AppState>,meeting_id:String,expected_hash:String,expected_name:String,name:String)->Result<Value,String>{
    let _storage=crate::audio::storage_migration::use_storage().await?;
    let _guard=crate::lark::app::DELIVERY_LOCK.lock().await;
    rename_file(state.db_manager.pool(),&meeting_id,&expected_hash,&expected_name,&name).await
}
fn write_named_files(dir: &Path, name:&str, content: &str, generated: bool) -> Result<String, String> {
    let name=validate_filename(name)?;
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
        let latest = dir.join(&name);
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

// Resolve history through the current company's meeting record, never a client path.
async fn history_directory(pool: &SqlitePool, id: &str) -> Result<PathBuf, String> {
    let file = preview(pool, id).await?.ok_or("该会议没有 Markdown 文件")?;
    let path = PathBuf::from(file["path"].as_str().ok_or("Markdown 路径无效")?);
    let parent = path.parent().ok_or("Markdown 目录无效")?.canonicalize().map_err(|e| e.to_string())?;
    let directory = parent.join("纪要版本");
    if directory.exists() && directory.canonicalize().map_err(|e| e.to_string())?.parent() != Some(parent.as_path()) { return Err("历史目录不能指向会议目录之外".into()); }
    Ok(directory)
}
fn history_name_valid(name: &str) -> bool {
    name.starts_with("会议纪要-") && name.ends_with(".md") && !name.contains(['/', '\\', ':']) && !name.contains("..")
}
async fn read_history(pool: &SqlitePool, id: &str, name: &str) -> Result<Value, String> {
    if !history_name_valid(name) { return Err("历史版本标识无效".into()); }
    let dir = history_directory(pool, id).await?.canonicalize().map_err(|e| e.to_string())?;
    let path = dir.join(name).canonicalize().map_err(|_| "历史版本不存在")?;
    if path.parent() != Some(dir.as_path()) { return Err("历史版本不能指向会议目录之外".into()); }
    let size = tokio::fs::metadata(&path).await.map_err(|e| e.to_string())?.len();
    if size > 2 * 1024 * 1024 { return Err("历史纪要超过 2 MiB".into()); }
    let content = tokio::fs::read_to_string(path).await.map_err(|e| e.to_string())?;
    if content.len() > 2 * 1024 * 1024 { return Err("历史纪要超过 2 MiB".into()); }
    Ok(serde_json::json!({"name":name,"hash":crate::lark::payload::fingerprint(&[&content]),"content":content}))
}
#[tauri::command]
pub async fn summary_markdown_history_list(state: tauri::State<'_, AppState>, meeting_id: String) -> Result<Vec<Value>, String> {
    let _storage = crate::audio::storage_migration::use_storage().await?;
    let _guard = crate::lark::app::DELIVERY_LOCK.lock().await;
    let dir = history_directory(state.db_manager.pool(), &meeting_id).await?;
    if !dir.exists() { return Ok(Vec::new()); }
    let mut entries = tokio::fs::read_dir(dir).await.map_err(|e| e.to_string())?;
    let mut result = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
        let name = entry.file_name().to_string_lossy().to_string();
        if !history_name_valid(&name) || !entry.file_type().await.map_err(|e| e.to_string())?.is_file() { continue; }
        result.push(serde_json::json!({"name":name,"size":entry.metadata().await.map_err(|e| e.to_string())?.len()}));
    }
    result.sort_by(|a,b| b["name"].as_str().cmp(&a["name"].as_str()));
    Ok(result)
}
#[tauri::command]
pub async fn summary_markdown_history_read(state: tauri::State<'_, AppState>, meeting_id: String, version: String) -> Result<Value, String> {
    let _storage = crate::audio::storage_migration::use_storage().await?;
    let _guard = crate::lark::app::DELIVERY_LOCK.lock().await;
    read_history(state.db_manager.pool(), &meeting_id, &version).await
}
async fn restore_history(pool: &SqlitePool, id: &str, version: &str, expected_hash: &str, version_hash: &str) -> Result<Value, String> {
    let history = read_history(pool, id, version).await?;
    if history["hash"].as_str() != Some(version_hash) { return Err("历史文件已变化，请重新预览后恢复".into()); }
    save_file(pool, id, expected_hash, history["content"].as_str().ok_or("历史内容无效")?).await
}
#[tauri::command]
pub async fn summary_markdown_history_restore(state: tauri::State<'_, AppState>, meeting_id: String, version: String, expected_hash: String, version_hash: String) -> Result<Value, String> {
    let _storage = crate::audio::storage_migration::use_storage().await?;
    let _guard = crate::lark::app::DELIVERY_LOCK.lock().await;
    restore_history(state.db_manager.pool(), &meeting_id, &version, &expected_hash, &version_hash).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn renamed_file_survives_save_restore_and_regeneration_without_clobbering(){
        let pool=SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("CREATE TABLE meetings(id TEXT,title TEXT,created_at TEXT,folder_path TEXT);CREATE TABLE summary_processes(meeting_id TEXT,result TEXT);CREATE TABLE summary_markdown_exports(meeting_id TEXT PRIMARY KEY,path TEXT,error TEXT,updated_at TEXT,manually_edited INTEGER DEFAULT 0);").execute(&pool).await.unwrap();
        let dir=tempfile::tempdir().unwrap();
        sqlx::query("INSERT INTO meetings VALUES('m','会议','2026-11-18T03:44:02.706579500+00:00',?)").bind(dir.path().to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO summary_processes VALUES('m',?)").bind(serde_json::json!({"markdown":"原稿"}).to_string()).execute(&pool).await.unwrap();
        export(&pool,"m",None,true).await.unwrap();let f=preview(&pool,"m").await.unwrap().unwrap();
        assert!(f["content"].as_str().unwrap().contains("会议时间：2026-11-18 11:44:02（东八区 UTC+8）"));
        assert!(!f["content"].as_str().unwrap().contains(".706579500"));
        std::fs::write(dir.path().join("其他.md"),"不能覆盖").unwrap();
        assert!(rename_file(&pool,"m",f["hash"].as_str().unwrap(),"会议纪要.md","其他.md").await.is_err());
        assert!(rename_file(&pool,"m","stale","会议纪要.md","周会").await.is_err());
        let f=rename_file(&pool,"m",f["hash"].as_str().unwrap(),"会议纪要.md","项目周会").await.unwrap();assert_eq!(f["name"],"项目周会.md");assert!(!dir.path().join("会议纪要.md").exists());
        save_file(&pool,"m",f["hash"].as_str().unwrap(),"手动改稿").await.unwrap();
        export(&pool,"m",None,true).await.unwrap();let f=preview(&pool,"m").await.unwrap().unwrap();assert_eq!(f["name"],"项目周会.md");assert!(f["content"].as_str().unwrap().contains("原稿"));assert_eq!(std::fs::read_to_string(dir.path().join("其他.md")).unwrap(),"不能覆盖");
        for name in ["../逃逸.md","CON.md","LPT1.txt.md","A:B","a\\b",".","\n"]{assert!(validate_filename(name).is_err(),"{name}");}
    }
    #[tokio::test]
    async fn manual_edit_is_preserved_until_regeneration_and_stale_save_is_rejected() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("CREATE TABLE meetings(id TEXT,title TEXT,created_at TEXT,folder_path TEXT); CREATE TABLE summary_processes(meeting_id TEXT,result TEXT); CREATE TABLE summary_markdown_exports(meeting_id TEXT PRIMARY KEY,path TEXT,error TEXT,updated_at TEXT,manually_edited INTEGER NOT NULL DEFAULT 0);").execute(&pool).await.unwrap();
        let root = tempfile::tempdir().unwrap();
        sqlx::query("INSERT INTO meetings VALUES('test','会议','2026-11-18T23:44:02.123+09:00',?)").bind(root.path().to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO summary_processes VALUES('test',?)").bind(serde_json::json!({"markdown":"原始详细纪要"}).to_string()).execute(&pool).await.unwrap();
        export(&pool, "test", None, true).await.unwrap();
        let original = preview(&pool, "test").await.unwrap().unwrap();
        assert!(original["content"].as_str().unwrap().contains("会议时间：2026-11-18 22:44:02（东八区 UTC+8）"));
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
    #[tokio::test]
    async fn history_restore_checks_both_versions_and_archives_current() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE summary_markdown_exports(meeting_id TEXT PRIMARY KEY,path TEXT,error TEXT,updated_at TEXT,manually_edited INTEGER DEFAULT 0)").execute(&pool).await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let path = write_files(root.path(), "历史版本", true).unwrap();
        let name = std::fs::read_dir(root.path().join("纪要版本")).unwrap().next().unwrap().unwrap().file_name().to_string_lossy().to_string();
        write_files(root.path(), "当前编辑", false).unwrap();
        sqlx::query("INSERT INTO summary_markdown_exports(meeting_id,path) VALUES('a',?)").bind(path).execute(&pool).await.unwrap();
        assert!(read_history(&pool,"a","../会议纪要.md").await.is_err());
        assert!(read_history(&pool,"missing",&name).await.is_err());
        let history = read_history(&pool,"a",&name).await.unwrap();
        let current = preview(&pool,"a").await.unwrap().unwrap();
        assert!(restore_history(&pool,"a",&name,"stale",history["hash"].as_str().unwrap()).await.is_err());
        assert!(restore_history(&pool,"a",&name,current["hash"].as_str().unwrap(),"changed-history").await.is_err());
        let restored = restore_history(&pool,"a",&name,current["hash"].as_str().unwrap(),history["hash"].as_str().unwrap()).await.unwrap();
        assert_eq!(restored["content"], "历史版本");
        let backups: Vec<_> = std::fs::read_dir(root.path().join("纪要版本")).unwrap().map(|p| std::fs::read_to_string(p.unwrap().path()).unwrap()).collect();
        assert!(backups.iter().any(|s| s == "当前编辑"));
        assert_eq!(sqlx::query_scalar::<_,i64>("SELECT manually_edited FROM summary_markdown_exports").fetch_one(&pool).await.unwrap(),1);
    }

}
