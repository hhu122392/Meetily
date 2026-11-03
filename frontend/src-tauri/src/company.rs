use std::{io::Write, path::{Path, PathBuf}, sync::Mutex};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tauri::{AppHandle, Manager, Runtime};
use crate::{database::manager::DatabaseManager, state::AppState};

const REGISTRY_FILE: &str = "companies.json";

#[derive(Clone, Serialize, Deserialize)]
pub struct Company {
    id: String,
    name: String,
    #[serde(default)]
    app_id: String,
    #[serde(default)]
    tenant_key: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Registry {
    version: u32,
    active: String,
    recording_root: Option<PathBuf>,
    companies: Vec<Company>,
}

pub struct CompanyState {
    root: PathBuf,
    active: String,
    registry: Mutex<Registry>,
    warning: Mutex<Option<String>>,
}

pub struct StartupFailure(pub String);

fn valid_id(id: &str) -> bool { id == "legacy" || uuid::Uuid::parse_str(id).is_ok_and(|v| v.to_string() == id) }

impl Registry {
    fn validate(&self) -> Result<(), String> {
        if self.version != 1 || !self.companies.iter().any(|c| c.id == self.active) {
            return Err("公司目录配置无效，请保留数据并联系维护人员。".into());
        }
        let mut ids = std::collections::HashSet::new();
        for c in &self.companies {
            if !valid_id(&c.id) || !ids.insert(&c.id) || c.name.trim().is_empty() {
                return Err("公司目录配置损坏，已停止加载会议资料。".into());
            }
        }
        Ok(())
    }

    fn bind(&mut self, id: &str, app_id: &str, tenant: Option<&str>) -> Result<(), String> {
        let c = self.companies.iter().find(|c| c.id == id).ok_or("公司不存在")?;
        if !c.app_id.is_empty() && c.app_id != app_id {
            return Err("当前公司已绑定其他 App ID。请先在“公司资料”中新增或切换公司，再配置该公司的机器人。".into());
        }
        if let Some(tenant) = tenant {
            if tenant.trim().is_empty() { return Err("Lark 未返回公司标识，未保存本次授权。".into()); }
            if !c.tenant_key.is_empty() && c.tenant_key != tenant {
                return Err("登录账号所属公司与当前资料不一致，未保存授权；请先切换公司。".into());
            }
        }
        if let Some(other) = self.companies.iter().find(|c| c.id != id &&
            (c.app_id == app_id || tenant.is_some_and(|t| !c.tenant_key.is_empty() && c.tenant_key == t))) {
            return Err(format!("该应用或公司已属于“{}”，请切换到该公司，避免重复资料库。", other.name));
        }
        let c = self.companies.iter_mut().find(|c| c.id == id).unwrap();
        c.app_id = app_id.to_owned();
        if let Some(tenant) = tenant { c.tenant_key = tenant.to_owned(); }
        Ok(())
    }
}

fn persist(root: &Path, registry: &Registry) -> Result<(), String> {
    registry.validate()?;
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let mut tmp = tempfile::NamedTempFile::new_in(root).map_err(|e| e.to_string())?;
    tmp.write_all(&serde_json::to_vec_pretty(registry).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    tmp.as_file().sync_all().map_err(|e| e.to_string())?;
    tmp.persist(root.join(REGISTRY_FILE)).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn initialize<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let root = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let path = root.join(REGISTRY_FILE);
    let registry: Registry = if path.exists() {
        serde_json::from_slice(&std::fs::read(&path).map_err(|e| e.to_string())?)
            .map_err(|_| "无法读取公司目录配置；为保护资料，未创建空白数据库。")?
    } else {
        let registry = Registry { version: 1, active: "legacy".into(), recording_root: None,
            companies: vec![Company { id: "legacy".into(), name: "原有公司".into(), app_id: String::new(), tenant_key: String::new() }] };
        persist(&root, &registry)?;
        registry
    };
    registry.validate()?;
    if (registry.active != "legacy" || registry.recording_root.is_some()) && !directory(&root, &registry.active)?.join("meeting_minutes.sqlite").is_file() {
        return Err("当前公司的数据库不存在，已停止加载，请检查数据目录。".into());
    }
    app.manage(CompanyState { root, active: registry.active.clone(), registry: Mutex::new(registry), warning: Mutex::new(None) });
    Ok(())
}

fn directory(root: &Path, id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) { return Err("无效公司目录标识。".into()); }
    // Keep the original database in place: adoption never copies stale history back.
    Ok(if id == "legacy" { root.to_path_buf() } else { root.join("companies").join(id) })
}

pub fn data_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let state = app.try_state::<CompanyState>().ok_or("公司目录尚未初始化")?;
    directory(&state.root, &state.active)
}

pub fn bind_identity<R: Runtime>(app: &AppHandle<R>, app_id: &str, tenant: Option<&str>) -> Result<(), String> {
    let state = app.state::<CompanyState>();
    let mut registry = state.registry.lock().map_err(|_| "公司配置锁不可用")?;
    let mut next = registry.clone();
    next.bind(&state.active, app_id, tenant)?;
    persist(&state.root, &next)?;
    *registry = next;
    Ok(())
}

pub async fn prepare_database<R: Runtime>(app: &AppHandle<R>, pool: &SqlitePool) -> Result<(), String> {
    let state = app.state::<CompanyState>();
    let (config, warning) = crate::lark::app::startup_config(pool).await?;
    *state.warning.lock().map_err(|_| "公司配置锁不可用")? = warning;
    let marker: Option<String> = sqlx::query_scalar("SELECT company_id FROM company_workspace WHERE id=1")
        .fetch_optional(pool).await.map_err(|e| e.to_string())?;
    if let Some(marker) = marker {
        if marker != state.active { return Err("数据库所属公司不匹配，已停止加载。".into()); }
        return Ok(());
    }
    // SQLite snapshot includes committed WAL data. Never copy a live .sqlite alone.
    let backup = state.root.join("company-backups");
    std::fs::create_dir_all(&backup).map_err(|e| e.to_string())?;
    sqlx::query("VACUUM INTO ?").bind(backup.join(format!("{}-{}.sqlite", state.active, uuid::Uuid::new_v4())).to_string_lossy().as_ref())
        .execute(pool).await.map_err(|e| format!("公司隔离前备份失败，未修改资料：{e}"))?;
    let old_root: Option<String> = sqlx::query_scalar("SELECT directory FROM recording_storage_settings WHERE id=1")
        .fetch_optional(pool).await.map_err(|e| e.to_string())?;
    let fallback = crate::audio::recording_preferences::load_recording_preferences(app).await.map_err(|e| e.to_string())?.save_folder;
    let target = {
        let mut registry = state.registry.lock().map_err(|_| "公司配置锁不可用")?;
        let mut next = registry.clone();
        let root = next.recording_root.get_or_insert_with(|| old_root.map(PathBuf::from).unwrap_or(fallback)).clone();
        if !root.is_absolute() { return Err("录音根目录无效。".into()); }
        persist(&state.root, &next)?;
        *registry = next;
        root.join("companies").join(&state.active)
    };
    crate::audio::recording_preferences::validate_save_folder(&target).map_err(|e| e.to_string())?;
    if !config.app_id.is_empty() { bind_identity(app, &config.app_id, None)?; }
    adopt_directory(pool, &state.active, &target).await
}

async fn adopt_directory(pool: &SqlitePool, company_id: &str, target: &Path) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    sqlx::query("INSERT INTO recording_storage_settings(id,directory) VALUES(1,?) ON CONFLICT(id) DO UPDATE SET directory=excluded.directory")
        .bind(target.to_string_lossy().as_ref()).execute(&mut *tx).await.map_err(|e| e.to_string())?;
    sqlx::query("INSERT INTO company_workspace(id,company_id) VALUES(1,?)").bind(company_id)
        .execute(&mut *tx).await.map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn company_status(app: AppHandle) -> Result<serde_json::Value, String> {
    if let Some(failure) = app.try_state::<StartupFailure>() { return Err(failure.0.clone()); }
    let state = app.try_state::<CompanyState>().ok_or("公司目录尚未初始化")?;
    let registry = state.registry.lock().map_err(|_| "公司配置锁不可用")?;
    let warning = state.warning.lock().map_err(|_| "公司配置锁不可用")?.clone();
    Ok(serde_json::json!({"active":state.active,"companies":registry.companies,"recording_root":registry.recording_root,"warning":warning}))
}

fn checked_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 60 || name.chars().any(char::is_control) {
        return Err("请输入 1 至 60 字的公司名称。".into());
    }
    Ok(name.into())
}

#[tauri::command]
pub fn company_rename(app: AppHandle, name: String) -> Result<(), String> {
    let state = app.state::<CompanyState>();
    let mut registry = state.registry.lock().map_err(|_| "公司配置锁不可用")?;
    let mut next = registry.clone();
    next.companies.iter_mut().find(|c| c.id == state.active).ok_or("公司不存在")?.name = checked_name(&name)?;
    persist(&state.root, &next)?;
    *registry = next;
    Ok(())
}

#[tauri::command]
pub async fn company_create(app: AppHandle, state: tauri::State<'_, AppState>, name: String) -> Result<String, String> {
    let _storage = crate::audio::storage_migration::use_storage().await?;
    let name = checked_name(&name)?;
    let company_state = app.state::<CompanyState>();
    {
        let registry = company_state.registry.lock().map_err(|_| "公司配置锁不可用")?;
        if registry.companies.iter().any(|c| c.name == name) { return Err("该公司名称已存在，请选择已有公司。".into()); }
    }
    let id = uuid::Uuid::new_v4().to_string();
    let dir = directory(&company_state.root, &id)?;
    let db = DatabaseManager::new(&dir.join("meeting_minutes.sqlite").to_string_lossy(), &dir.join("meeting_minutes.db").to_string_lossy())
        .await.map_err(|e| e.to_string())?;
    // Reuse local model choices, never clone another company's cloud keys, endpoints or bot.
    use crate::database::repositories::setting::SettingsRepository;
    if let Some(c) = SettingsRepository::get_model_config(state.db_manager.pool()).await.map_err(|e| e.to_string())? {
        if c.provider == "builtin-ai" {
            SettingsRepository::save_model_config(db.pool(), &c.provider, &c.model, &c.whisper_model, None).await.map_err(|e| e.to_string())?;
        }
    }
    if let Some(c) = SettingsRepository::get_transcript_config(state.db_manager.pool()).await.map_err(|e| e.to_string())? {
        if c.provider == "localWhisper" || c.provider == "parakeet" || c.provider == "sensevoice" {
            SettingsRepository::save_transcript_config(db.pool(), &c.provider, &c.model).await.map_err(|e| e.to_string())?;
        }
    }
    db.cleanup().await.map_err(|e| e.to_string())?;
    let mut registry = company_state.registry.lock().map_err(|_| "公司配置锁不可用")?;
    let mut next = registry.clone();
    if next.companies.iter().any(|c| c.name == name) { return Err("该公司名称已存在，请选择已有公司。".into()); }
    next.companies.push(Company { id: id.clone(), name, app_id: String::new(), tenant_key: String::new() });
    persist(&company_state.root, &next)?;
    *registry = next;
    Ok(id)
}

pub async fn ensure_idle(pool: &SqlitePool) -> Result<(), String> {
    if crate::audio::recording_commands::is_recording().await { return Err("请先结束录音，再切换公司。".into()); }
    let busy: i64 = sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM recording_jobs WHERE status NOT IN ('completed','failed','paused','cancelled')) + (SELECT COUNT(*) FROM summary_processes WHERE LOWER(status) IN ('pending','processing','summarizing'))")
        .fetch_one(pool).await.map_err(|e| e.to_string())?;
    if busy > 0 { return Err("还有录音处理或纪要生成任务；请暂停后台转写并等待显示“已暂停”，或等待纪要生成完成后再切换公司。".into()); }
    Ok(())
}

#[tauri::command]
pub async fn company_switch(app: AppHandle, state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    let storage = crate::audio::storage_migration::STORAGE_LOCK.try_write().map_err(|_| "正在处理会议文件，请完成后再切换公司。")?;
    let config = crate::lark::app::CONFIG_LOCK.try_lock().map_err(|_| "正在更新 Lark 授权，请稍后再切换。")?;
    let delivery = crate::lark::app::DELIVERY_LOCK.try_lock().map_err(|_| "正在发送纪要，请等待发送结果。")?;
    let webhook = crate::lark::SEND_LOCK.try_lock().map_err(|_| "正在发送纪要，请等待发送结果。")?;
    ensure_idle(state.db_manager.pool()).await?;
    let company_state = app.state::<CompanyState>();
    if company_state.active == id { return Ok(()); }
    {
        let mut registry = company_state.registry.lock().map_err(|_| "公司配置锁不可用")?;
        if !registry.companies.iter().any(|c| c.id == id) { return Err("所选公司不存在。".into()); }
        if !directory(&company_state.root, &id)?.join("meeting_minutes.sqlite").is_file() {
            return Err("所选公司的数据库不存在，未切换；请检查数据目录。".into());
        }
        let mut next = registry.clone();
        next.active = id;
        persist(&company_state.root, &next)?;
        *registry = next;
    }
    // Keep all start/send gates closed until process exit. The next process opens
    // a different pool; no stale background task or React state can cross companies.
    tauri::async_runtime::spawn(async move {
        let _guards = (storage, config, delivery, webhook);
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        app.restart();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn registry() -> Registry { Registry { version: 1, active: "legacy".into(), recording_root: None, companies: vec![Company { id: "legacy".into(), name: "甲公司".into(), app_id: String::new(), tenant_key: String::new() }] } }
    #[test]
    fn company_binding_rejects_cross_tenant_and_duplicate_workspaces() {
        let mut r = registry();
        r.bind("legacy", "cli_a", Some("tenant_a")).unwrap();
        r.bind("legacy", "cli_a", Some("tenant_a")).unwrap();
        assert!(r.bind("legacy", "cli_a", Some("tenant_b")).is_err());
        assert!(r.bind("legacy", "cli_b", Some("tenant_a")).is_err());
        let id = uuid::Uuid::new_v4().to_string();
        r.companies.push(Company { id: id.clone(), name: "乙公司".into(), app_id: String::new(), tenant_key: String::new() });
        assert!(r.bind(&id, "cli_a", None).is_err());
        assert!(r.bind(&id, "cli_b", Some("tenant_a")).is_err());
        r.bind(&id, "cli_b", Some("tenant_b")).unwrap();
    }
    #[test]
    fn registry_roundtrip_and_paths_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        let r = registry(); persist(root.path(), &r).unwrap();
        let saved: Registry = serde_json::from_slice(&std::fs::read(root.path().join(REGISTRY_FILE)).unwrap()).unwrap();
        saved.validate().unwrap();
        assert_eq!(directory(root.path(), "legacy").unwrap(), root.path());
        assert!(directory(root.path(), "../other").is_err());
        assert_ne!(directory(root.path(), &uuid::Uuid::new_v4().to_string()).unwrap(), root.path());
        let mut bad = saved; bad.active = "missing".into(); assert!(persist(root.path(), &bad).is_err());
    }
    #[tokio::test]
    async fn different_company_databases_have_no_shared_meetings_or_keys() {
        let root = tempfile::tempdir().unwrap();
        let a = DatabaseManager::new(&root.path().join("a.sqlite").to_string_lossy(), "absent.db").await.unwrap();
        let b = DatabaseManager::new(&root.path().join("b.sqlite").to_string_lossy(), "absent.db").await.unwrap();
        sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at) VALUES('a','甲公司会议','now','now')").execute(a.pool()).await.unwrap();
        sqlx::query("INSERT INTO lark_app_settings(id,config_json) VALUES(1,'private')").execute(a.pool()).await.unwrap();
        for table in ["meetings", "lark_app_settings"] {
            assert_eq!(sqlx::query_scalar::<_,i64>(&format!("SELECT COUNT(*) FROM {table}")).fetch_one(b.pool()).await.unwrap(), 0);
            assert_eq!(sqlx::query_scalar::<_,i64>(&format!("SELECT COUNT(*) FROM {table}")).fetch_one(a.pool()).await.unwrap(), 1);
        }
        a.cleanup().await.unwrap(); b.cleanup().await.unwrap();
    }
    #[tokio::test]
    async fn adoption_preserves_history_and_rolls_back_if_company_already_assigned() {
        let root = tempfile::tempdir().unwrap();
        let a = DatabaseManager::new(&root.path().join("a.sqlite").to_string_lossy(), "absent.db").await.unwrap();
        let original = root.path().join("历史录音");
        std::fs::create_dir_all(&original).unwrap();
        std::fs::write(original.join("会议纪要.md"), "原有中文纪要").unwrap();
        sqlx::query("INSERT INTO meetings(id,title,created_at,updated_at,folder_path) VALUES('a','原有会议','now','now',?)")
            .bind(original.to_string_lossy().as_ref()).execute(a.pool()).await.unwrap();
        let target = root.path().join("companies/legacy");
        adopt_directory(a.pool(), "legacy", &target).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT folder_path FROM meetings WHERE id='a'").fetch_one(a.pool()).await.unwrap(), original.to_string_lossy());
        assert_eq!(std::fs::read_to_string(original.join("会议纪要.md")).unwrap(), "原有中文纪要");
        assert!(adopt_directory(a.pool(), "other", &root.path().join("other")).await.is_err());
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT directory FROM recording_storage_settings").fetch_one(a.pool()).await.unwrap(), target.to_string_lossy());
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT company_id FROM company_workspace").fetch_one(a.pool()).await.unwrap(), "legacy");
        ensure_idle(a.pool()).await.unwrap();
        sqlx::query("INSERT INTO recording_jobs(meeting_id,folder_path,status,options_json,created_at,updated_at) VALUES('a','folder','pausing','{}',0,0)")
            .execute(a.pool()).await.unwrap();
        assert!(ensure_idle(a.pool()).await.is_err());
        sqlx::query("UPDATE recording_jobs SET status='paused'").execute(a.pool()).await.unwrap();
        ensure_idle(a.pool()).await.unwrap();
        sqlx::query("INSERT INTO summary_processes(meeting_id,status,created_at,updated_at) VALUES('a','PENDING','now','now')").execute(a.pool()).await.unwrap();
        assert!(ensure_idle(a.pool()).await.is_err());
        a.cleanup().await.unwrap();
    }
}
