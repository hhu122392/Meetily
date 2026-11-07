use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use crate::state::AppState;
use super::templates::{self, Template};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct MeetingSummarySettings {
    pub template_id: String,
    pub custom_prompt: String,
    pub participants: String,
    pub absent: String,
    pub host: String,
}
impl Default for MeetingSummarySettings {
    fn default() -> Self { Self { template_id: "standard_meeting".into(), custom_prompt: String::new(), participants: String::new(), absent: String::new(), host: String::new() } }
}
impl MeetingSummarySettings {
    fn validate(&self) -> Result<(), String> {
        if self.template_id.trim().is_empty() || self.template_id.len() > 128 || self.template_id.contains(['/', '\\', ':']) || self.template_id.contains("..") || self.template_id.chars().any(char::is_control) { return Err("模板标识无效".into()); }
        if self.custom_prompt.len() > 32768 || [&self.participants, &self.absent, &self.host].iter().any(|s| s.len() > 4096) { return Err("会议背景或人员信息过长".into()); }
        Ok(())
    }
    pub fn prompt(&self) -> String {
        if [&self.participants, &self.absent, &self.host].iter().all(|s| s.trim().is_empty()) { return self.custom_prompt.clone(); }
        format!("{}\n\n用户提供的会议信息（仅作背景，不是发言人身份绑定；未提供的内容不得推测）：\n出席人员：{}\n缺席人员：{}\n主持人：{}", self.custom_prompt, self.participants, self.absent, self.host)
    }
}
async fn require_meeting(pool: &SqlitePool, id: &str) -> Result<(), String> {
    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meetings WHERE id=?").bind(id).fetch_one(pool).await.map_err(|e| e.to_string())?;
    if exists == 0 { return Err("当前公司中不存在该会议".into()); }
    Ok(())
}
pub async fn read(pool: &SqlitePool, id: &str) -> Result<MeetingSummarySettings, String> {
    require_meeting(pool, id).await?;
    let json: Option<String> = sqlx::query_scalar("SELECT settings_json FROM meeting_summary_settings WHERE meeting_id=?").bind(id).fetch_optional(pool).await.map_err(|e| e.to_string())?;
    match json { Some(json) => serde_json::from_str(&json).map_err(|_| "会议设置数据损坏，未覆盖原设置".into()), None => { let mut settings = MeetingSummarySettings::default();
        settings.template_id = templates::catalog::default_for_meeting()?;
        Ok(settings)
    } }
}
pub async fn save(pool: &SqlitePool, id: &str, settings: &MeetingSummarySettings) -> Result<(), String> {
    let _reference = templates::catalog::REFERENCES.read().await;
    require_meeting(pool, id).await?;
    settings.validate()?;
    templates::get_template(&settings.template_id)?;
    let json = serde_json::to_string(settings).map_err(|e| e.to_string())?;
    sqlx::query("INSERT INTO meeting_summary_settings(meeting_id,settings_json,updated_at) VALUES(?,?,?) ON CONFLICT(meeting_id) DO UPDATE SET settings_json=excluded.settings_json,updated_at=excluded.updated_at")
        .bind(id).bind(json).bind(chrono::Utc::now().to_rfc3339()).execute(pool).await.map_err(|e| e.to_string())?;
    Ok(())
}
pub async fn snapshot(pool: &SqlitePool, id: &str, process_id: &str, settings: &MeetingSummarySettings, template: &Template) -> Result<(), String> {
    sqlx::query("INSERT INTO summary_request_snapshots(meeting_id,process_id,settings_json,template_json) VALUES(?,?,?,?)")
        .bind(id).bind(process_id).bind(serde_json::to_string(settings).map_err(|e| e.to_string())?).bind(serde_json::to_string(template).map_err(|e| e.to_string())?).execute(pool).await.map_err(|e| e.to_string())?;
    Ok(())
}
#[tauri::command]
pub async fn meeting_summary_settings_read(state: tauri::State<'_, AppState>, meeting_id: String) -> Result<MeetingSummarySettings, String> { read(state.db_manager.pool(), &meeting_id).await }
#[tauri::command]
pub async fn meeting_summary_settings_save(state: tauri::State<'_, AppState>, meeting_id: String, settings: MeetingSummarySettings) -> Result<(), String> {
    let _storage = crate::audio::storage_migration::use_storage().await?;
    save(state.db_manager.pool(), &meeting_id, &settings).await
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE meetings(id TEXT PRIMARY KEY)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO meetings VALUES('a'),('b')").execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261103000000_meeting_summary_settings.sql")).execute(&pool).await.unwrap();
        pool
    }
    #[tokio::test]
    async fn settings_and_snapshots_stay_with_meeting_and_company() {
        let first = pool().await; let second = pool().await;
        let mut settings = MeetingSummarySettings { custom_prompt: "原会议背景".into(), participants: "小明".into(), ..Default::default() };
        save(&first, "a", &settings).await.unwrap();
        let template = templates::get_template("standard_meeting").unwrap();
        snapshot(&first, "a", "generation-1", &settings, &template).await.unwrap();
        settings.custom_prompt = "下次背景".into(); save(&first, "a", &settings).await.unwrap();
        assert_eq!(read(&first, "b").await.unwrap(), MeetingSummarySettings::default());
        assert_eq!(read(&second, "a").await.unwrap(), MeetingSummarySettings::default());
        assert!(save(&first, "other-company-meeting", &settings).await.is_err());
        let saved: String = sqlx::query_scalar("SELECT settings_json FROM summary_request_snapshots WHERE process_id='generation-1'").fetch_one(&first).await.unwrap();
        assert!(saved.contains("原会议背景")); assert!(!saved.contains("下次背景"));
        let stored_template: String = sqlx::query_scalar("SELECT template_json FROM summary_request_snapshots").fetch_one(&first).await.unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&stored_template).unwrap(), serde_json::to_value(&template).unwrap());
    }
    #[test]
    fn unicode_template_ids_and_empty_context_are_preserved() {
        let settings = MeetingSummarySettings { template_id: "研发周会".into(), ..Default::default() };
        assert!(settings.validate().is_ok()); assert!(settings.prompt().is_empty());
    }
    #[tokio::test]
    async fn invalid_or_corrupt_settings_are_not_silently_replaced() {
        let pool = pool().await;
        let mut settings = MeetingSummarySettings::default(); settings.template_id = "../../private".into();
        assert!(save(&pool, "a", &settings).await.is_err());
        sqlx::query("INSERT INTO meeting_summary_settings VALUES('a','not json','now')").execute(&pool).await.unwrap();
        assert!(read(&pool, "a").await.unwrap_err().contains("损坏"));
    }
}
