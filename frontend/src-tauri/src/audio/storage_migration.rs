use std::{collections::BTreeSet, io::Read, path::{Path, PathBuf}};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use tauri::{AppHandle, Manager, Runtime};
use crate::state::AppState;

// Readers cover all operations that can create/write recording files. Migration never waits
// behind an active job: it fails immediately and leaves the current location unchanged.
pub(crate) static STORAGE_LOCK: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());
pub async fn use_storage() -> Result<tokio::sync::RwLockReadGuard<'static, ()>, String> {
    STORAGE_LOCK.try_read().map_err(|_| "正在迁移会议文件或切换公司，请完成后重试。".into())
}

#[tauri::command]
pub async fn migrate_recording_storage<R: Runtime>(app: AppHandle<R>, state: tauri::State<'_, AppState>, directory: String) -> Result<serde_json::Value, String> {
    let _exclusive = STORAGE_LOCK.try_write().map_err(|_| "正在录音、导入、转写或保存纪要，请任务完成后再迁移。")?;
    let _review = crate::lark::app::DELIVERY_LOCK.lock().await;
    let pool = state.db_manager.pool();
    if super::recording_commands::is_recording().await { return Err("请先停止录音再迁移。".into()); }
    let busy: i64 = sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM recording_jobs WHERE status NOT IN ('completed','failed','cancelled')) + (SELECT COUNT(*) FROM summary_processes WHERE LOWER(status) IN ('pending','processing','summarizing'))")
        .fetch_one(pool).await.map_err(|e| e.to_string())?;
    if busy != 0 { return Err("还有录音处理或纪要生成任务，请完成或取消后再迁移。".into()); }
    let preferences = super::recording_preferences::load_recording_preferences(&app).await.map_err(|e| e.to_string())?;
    let source = preferences.save_folder;
    let target = PathBuf::from(directory);
    super::recording_preferences::validate_save_folder(&target).map_err(|e| e.to_string())?;
    let target = std::fs::canonicalize(&target).map_err(|e| e.to_string())?;
    let mut existing: Vec<String> = sqlx::query_scalar("SELECT folder_path FROM meetings WHERE folder_path IS NOT NULL UNION SELECT folder_path FROM recording_jobs UNION SELECT folder_path FROM lark_recording_links UNION SELECT folder_path FROM recording_transcription_checks UNION SELECT folder_path FROM lark_auto_recordings WHERE folder_path IS NOT NULL")
        .fetch_all(pool).await.map_err(|e| e.to_string())?;
    let exports: Vec<String> = sqlx::query_scalar("SELECT path FROM summary_markdown_exports WHERE path IS NOT NULL")
        .fetch_all(pool).await.map_err(|e| e.to_string())?;
    existing.extend(exports.iter().filter_map(|p| Path::new(p).parent().map(|p| p.to_string_lossy().into_owned())));
    let plan = make_plan(&source, &target, &existing)?;
    let pairs = plan.clone();
    tokio::task::spawn_blocking(move || {
        for (old, new) in &pairs { copy_verified(old, new)?; }
        Ok::<_, String>(())
    }).await.map_err(|e| e.to_string())?
        .map_err(|e| format!("迁移未完成，仍使用原目录；原文件已保留。目标中可能有部分副本，请另选空目录重试。{e}"))?;
    // All original files remain as a recovery backup. A failed transaction cannot lose history.
    remap_paths(pool, &plan, &target).await?;
    let _ = app.emit("recording-storage-changed", target.to_string_lossy().to_string());
    Ok(serde_json::json!({"directory":target.to_string_lossy(),"backup_directory":source.to_string_lossy(),"message":"已迁移并校验，历史会议和新录音已使用新目录；原文件保留为恢复备份。"}))
}

use tauri::Emitter;

fn make_plan(source: &Path, target: &Path, historical: &[String]) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    if !source.is_absolute() || source.parent().is_none() { return Err("原录音目录无效，请先检查设置。".into()); }
    if std::fs::read_dir(target).map_err(|e| e.to_string())?.next().is_some() {
        return Err("请选择空文件夹，避免覆盖已有文件。".into());
    }
    let source = if source.exists() { std::fs::canonicalize(source).map_err(|e| e.to_string())? } else { source.to_path_buf() };
    if target.starts_with(&source) || source.starts_with(target) { return Err("新旧目录不能相同，也不能互相包含。".into()); }
    let mut plan = Vec::new();
    if source.exists() { plan.push((source.clone(), target.to_path_buf())); }
    let mut added = BTreeSet::new();
    for old in historical.iter().filter(|s| !s.trim().is_empty()) {
        let path = PathBuf::from(old);
        if !path.is_absolute() { return Err("历史会议含相对路径，无法安全迁移。".into()); }
        let canonical = std::fs::canonicalize(&path).map_err(|e| format!("历史会议文件夹不可用：{}（{}），已取消迁移。", path.display(), e))?;
        if canonical.starts_with(&source) || !added.insert(canonical.clone()) { continue; }
        if canonical.starts_with(target) || target.starts_with(&canonical) { return Err("目标目录与历史会议目录重叠。".into()); }
        let suffix = crate::lark::payload::fingerprint(&[&canonical.to_string_lossy()]);
        let name = format!("{}-{}", canonical.file_name().unwrap_or_default().to_string_lossy(), &suffix[..8]);
        plan.push((canonical, target.join("历史会议").join(name)));
    }
    Ok(plan)
}

fn digest(path: &Path) -> Result<Vec<u8>, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buf = [0u8; 65536];
    loop { let n = file.read(&mut buf).map_err(|e| e.to_string())?; if n == 0 { break; } hash.update(&buf[..n]); }
    Ok(hash.finalize().to_vec())
}

fn copy_verified(source: &Path, target: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(source).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    { use std::os::windows::fs::MetadataExt;
      if meta.file_attributes() & 0x400 != 0 { return Err(format!("录音目录包含链接或联接点，未迁移：{}", source.display())); }
    }
    if meta.file_type().is_symlink() { return Err("录音目录包含符号链接，未迁移。".into()); }
    if meta.is_dir() {
        std::fs::create_dir_all(target).map_err(|e| e.to_string())?;
        for entry in std::fs::read_dir(source).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            copy_verified(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else if meta.is_file() {
        let mut input = std::fs::File::open(source).map_err(|e| e.to_string())?;
        let mut output = std::fs::OpenOptions::new().write(true).create_new(true).open(target).map_err(|e| e.to_string())?;
        std::io::copy(&mut input, &mut output).map_err(|e| e.to_string())?;
        output.sync_all().map_err(|e| e.to_string())?;
        drop(output);
        if digest(source)? != digest(target)? { return Err(format!("文件校验失败：{}，原目录保持不变。", source.display())); }
    } else { return Err("录音目录含不支持的文件类型。".into()); }
    Ok(())
}

fn relocated(value: &str, plan: &[(PathBuf, PathBuf)]) -> Option<String> {
    let path = PathBuf::from(value);
    let canonical = std::fs::canonicalize(&path).ok()?;
    plan.iter().find_map(|(old,new)| canonical.strip_prefix(old).ok().map(|tail| {
        if tail.as_os_str().is_empty() { new.clone() } else { new.join(tail) }.to_string_lossy().into_owned()
    }))
}

async fn remap_paths(pool: &SqlitePool, plan: &[(PathBuf, PathBuf)], target: &Path) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    for (table, column) in [("meetings","folder_path"),("recording_jobs","folder_path"),("lark_recording_links","folder_path"),("lark_auto_recordings","folder_path"),("recording_transcription_checks","folder_path"),("summary_markdown_exports","path")] {
        let values = sqlx::query(&format!("SELECT DISTINCT {column} AS value FROM {table} WHERE {column} IS NOT NULL"))
            .fetch_all(&mut *tx).await.map_err(|e| e.to_string())?;
        for row in values {
            let old: String = row.get("value");
            if let Some(new) = relocated(&old, plan) {
                sqlx::query(&format!("UPDATE {table} SET {column}=? WHERE {column}=?"))
                    .bind(new).bind(old).execute(&mut *tx).await.map_err(|e| e.to_string())?;
            }
        }
    }
    sqlx::query("INSERT INTO recording_storage_settings(id,directory) VALUES(1,?) ON CONFLICT(id) DO UPDATE SET directory=excluded.directory")
        .bind(target.to_string_lossy().as_ref()).execute(&mut *tx).await.map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copy_preserves_unicode_audio_and_nested_markdown_without_overwrite() {
        let root = tempfile::tempdir().unwrap(); let old = root.path().join("原目录"); let new = root.path().join("新目录");
        std::fs::create_dir_all(old.join("会议/纪要版本")).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(old.join("会议/audio.mp4"), [0,1,2,255]).unwrap();
        std::fs::write(old.join("会议/纪要版本/原稿.md"), "# 中文纪要").unwrap();
        let plan = make_plan(&old, &std::fs::canonicalize(&new).unwrap(), &[old.join("会议").to_string_lossy().into_owned()]).unwrap();
        for (a,b) in &plan { copy_verified(a,b).unwrap(); }
        assert_eq!(digest(&old.join("会议/audio.mp4")).unwrap(), digest(&new.join("会议/audio.mp4")).unwrap());
        assert_eq!(std::fs::read_to_string(new.join("会议/纪要版本/原稿.md")).unwrap(), "# 中文纪要");
        assert!(copy_verified(&old,&new).is_err());
        assert!(old.join("会议/audio.mp4").exists());
        assert!(relocated(&old.join("会议").to_string_lossy(), &plan).unwrap().contains("新目录"));
    }
    #[test]
    fn rejects_nested_destination_and_missing_history() {
        let root = tempfile::tempdir().unwrap(); let old = root.path().join("old"); let child = old.join("child");
        std::fs::create_dir_all(&child).unwrap();
        assert!(make_plan(&old,&std::fs::canonicalize(&child).unwrap(),&[]).is_err());
        let target = root.path().join("target"); std::fs::create_dir_all(&target).unwrap();
        assert!(make_plan(&old,&target,&[root.path().join("missing").to_string_lossy().into_owned()]).is_err());
    }
    #[tokio::test]
    async fn transaction_updates_all_history_paths_and_future_location() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        let root = tempfile::tempdir().unwrap(); let old = root.path().join("old"); let new = root.path().join("new");
        std::fs::create_dir_all(&old).unwrap(); std::fs::write(old.join("会议纪要.md"), "中文").unwrap();
        for table in ["meetings","recording_jobs","lark_recording_links","lark_auto_recordings","recording_transcription_checks"] {
            sqlx::query(&format!("CREATE TABLE {table}(folder_path TEXT)")).execute(&pool).await.unwrap();
            sqlx::query(&format!("INSERT INTO {table} VALUES(?)")).bind(old.to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        }
        sqlx::raw_sql("CREATE TABLE summary_markdown_exports(path TEXT); CREATE TABLE recording_storage_settings(id INTEGER PRIMARY KEY,directory TEXT);").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO summary_markdown_exports VALUES(?)").bind(old.join("会议纪要.md").to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        let plan = vec![(std::fs::canonicalize(&old).unwrap(),new.clone())];
        remap_paths(&pool,&plan,&new).await.unwrap();
        for table in ["meetings","recording_jobs","lark_recording_links","lark_auto_recordings","recording_transcription_checks"] {
            assert_eq!(sqlx::query_scalar::<_,String>(&format!("SELECT folder_path FROM {table}")).fetch_one(&pool).await.unwrap(),new.to_string_lossy());
        }
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT directory FROM recording_storage_settings").fetch_one(&pool).await.unwrap(), new.to_string_lossy());
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT path FROM summary_markdown_exports").fetch_one(&pool).await.unwrap(),new.join("会议纪要.md").to_string_lossy());
    }
    #[tokio::test]
    async fn active_file_work_excludes_migration_and_migration_excludes_new_work() {
        let active = use_storage().await.unwrap();
        assert!(STORAGE_LOCK.try_write().is_err());
        drop(active);
        let moving = STORAGE_LOCK.write().await;
        assert!(use_storage().await.is_err());
        drop(moving);
        assert!(use_storage().await.is_ok());
    }
}

