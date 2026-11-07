use super::templates::catalog::{self, Document, Library};
use crate::state::AppState;

#[tauri::command]
pub async fn template_library_list() -> Result<Library, String> {
    catalog::with_store(|repo| repo.list())
}

#[tauri::command]
pub async fn template_library_read(template_id: String) -> Result<Document, String> {
    catalog::with_store(|repo| Ok(repo.get(&template_id)?.0))
}

#[tauri::command]
pub async fn template_library_save(
    document: Document,
    expected_version: Option<u64>,
) -> Result<Document, String> {
    catalog::with_store(|repo| repo.save(document, expected_version))
}

#[tauri::command]
pub async fn template_library_import(template_json: String) -> Result<Document, String> {
    catalog::with_store(|repo| repo.import(&template_json))
}

#[tauri::command]
pub async fn template_library_duplicate(template_id: String) -> Result<Document, String> {
    catalog::with_store(|repo| repo.duplicate(&template_id))
}

#[tauri::command]
pub async fn template_library_set_default(template_id: String) -> Result<(), String> {
    catalog::with_store(|repo| repo.set_default(&template_id))
}

#[tauri::command]
pub async fn template_library_delete(
    state: tauri::State<'_, AppState>,
    template_id: String,
) -> Result<(), String> {
    let _storage = crate::audio::storage_migration::use_storage().await?;
    let _reference = catalog::REFERENCES.write().await;
    ensure_unused(state.db_manager.pool(), &template_id).await?;
    catalog::with_store(|repo| repo.delete(&template_id))
}

async fn ensure_unused(pool: &sqlx::SqlitePool, template_id: &str) -> Result<(), String> {
    let referenced:i64=sqlx::query_scalar("SELECT COUNT(*) FROM meeting_summary_settings WHERE CASE WHEN json_valid(settings_json) THEN json_extract(settings_json,'$.templateId') = ? ELSE 1 END")
        .bind(template_id).fetch_one(pool).await.map_err(|e|e.to_string())?;
    if referenced > 0 {
        return Err(
            "此模板被会议设置引用，或有损坏的会议设置；请先检查并为相关会议改选模板".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deletion_checks_meeting_references_and_corrupt_settings() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE meeting_summary_settings(settings_json TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO meeting_summary_settings VALUES(?)")
            .bind(r#"{"templateId":"custom_used"}"#)
            .execute(&pool)
            .await
            .unwrap();
        assert!(ensure_unused(&pool, "custom_used").await.is_err());
        ensure_unused(&pool, "custom_unused").await.unwrap();
        sqlx::query("INSERT INTO meeting_summary_settings VALUES('corrupt')")
            .execute(&pool)
            .await
            .unwrap();
        assert!(ensure_unused(&pool, "custom_unused").await.is_err());
    }
}

#[tauri::command]
pub async fn template_library_export(
    app: tauri::AppHandle,
    template_id: String,
) -> Result<Option<String>, String> {
    use std::io::Write;
    use tauri_plugin_dialog::DialogExt;
    let document = catalog::with_store(|repo| Ok(repo.get(&template_id)?.0))?;
    tauri::async_runtime::spawn_blocking(move || {
        let Some(path) = app
            .dialog()
            .file()
            .set_title("导出纪要模板")
            .add_filter("JSON 模板", &["json"])
            .set_file_name(format!("{}.json", document.id))
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = path.into_path().map_err(|e| e.to_string())?;
        let parent = path.parent().ok_or("导出目录无效")?;
        let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        temp.write_all(&serde_json::to_vec_pretty(&document).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        temp.as_file().sync_all().map_err(|e| e.to_string())?;
        temp.persist(&path).map_err(|e| e.to_string())?;
        Ok(Some(path.to_string_lossy().into_owned()))
    })
    .await
    .map_err(|e| e.to_string())?
}
