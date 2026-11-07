//! 公司范围的模板库。文件格式兼容主分支 v2，旧版 JSON 在读取时转换，原文件保留。
use super::types::{Template, TemplateSection};
use chrono::Utc;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

pub const MAX_JSON_BYTES: usize = 800_000;
static STORE: Lazy<Mutex<Option<Repository>>> = Lazy::new(|| Mutex::new(None));
// 防止删除检查与会议引用写入交错。
pub static REFERENCES: Lazy<tokio::sync::RwLock<()>> = Lazy::new(|| tokio::sync::RwLock::new(()));

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    pub id: String,
    pub title: String,
    pub instruction: String,
    pub format: String,
    #[serde(default)]
    pub item_format: Option<String>,
    #[serde(default)]
    pub example_item_format: Option<String>,
    pub required: bool,
    pub empty_behavior: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub schema_version: u8,
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: u64,
    pub locale: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub source: Value,
    pub created_at: String,
    pub updated_at: String,
    pub sections: Vec<Section>,
    #[serde(default)]
    pub extensions: serde_json::Map<String, Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub locale: Option<String>,
    pub version: u64,
    pub readonly: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    pub templates: Vec<Entry>,
    pub default_template_id: String,
    pub warnings: Vec<String>,
}

pub fn valid_id(id: &str) -> Result<(), String> {
    if id.trim().is_empty()
        || id.chars().count() > 128
        || id.contains(['/', '\\', ':'])
        || id.contains("..")
        || id.ends_with(['.', ' '])
        || id.chars().any(|c| c.is_control() || "<>\"|?*".contains(c))
    {
        return Err("模板标识无效".into());
    }
    if id.eq_ignore_ascii_case("preferences") {
        return Err("模板标识已保留".into());
    }
    let name = id.split('.').next().unwrap_or("").to_ascii_uppercase();
    if ["CON", "PRN", "AUX", "NUL"].contains(&name.as_str())
        || (name.len() == 4
            && (name.starts_with("COM") || name.starts_with("LPT"))
            && name.as_bytes()[3].is_ascii_digit())
    {
        return Err("模板标识不能使用系统保留名称".into());
    }
    Ok(())
}

fn bounded(value: &str, max: usize, label: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.chars().count() > max {
        return Err(format!("{label}不能为空且不能超过 {max} 字"));
    }
    Ok(())
}

impl Document {
    pub fn parse(text: &str, legacy_id: &str) -> Result<Self, String> {
        if text.len() > MAX_JSON_BYTES {
            return Err("模板 JSON 超过 800KB".into());
        }
        let value: Value =
            serde_json::from_str(text).map_err(|e| format!("模板 JSON 无效：{e}"))?;
        let doc = if value.get("schema_version").is_some() {
            serde_json::from_value(value).map_err(|e| format!("模板 v2 格式无效：{e}"))?
        } else {
            let legacy: Template =
                serde_json::from_value(value).map_err(|e| format!("旧版模板格式无效：{e}"))?;
            legacy.validate()?;
            let now = Utc::now().to_rfc3339();
            Self {
                schema_version: 2,
                id: legacy_id.into(),
                name: legacy.name,
                description: legacy.description,
                version: 1,
                locale: None,
                tags: vec![],
                source: json!({"type":"legacy_migration"}),
                created_at: now.clone(),
                updated_at: now,
                extensions: Default::default(),
                sections: legacy
                    .sections
                    .into_iter()
                    .enumerate()
                    .map(|(i, s)| Section {
                        id: format!("section_{}", i + 1),
                        title: s.title,
                        instruction: s.instruction,
                        format: s.format,
                        item_format: s.item_format,
                        example_item_format: s.example_item_format,
                        required: true,
                        empty_behavior: "show_not_mentioned".into(),
                    })
                    .collect(),
            }
        };
        doc.validate()?;
        Ok(doc)
    }

    pub fn validate(&self) -> Result<(), String> {
        valid_id(&self.id)?;
        if self.schema_version != 2 || self.version == 0 || self.version > 9_007_199_254_740_991 {
            return Err("模板版本无效".into());
        }
        bounded(&self.name, 120, "模板名称")?;
        bounded(&self.description, 1000, "模板说明")?;
        for date in [&self.created_at, &self.updated_at] {
            chrono::DateTime::parse_from_rfc3339(date).map_err(|_| "模板日期无效")?;
        }
        if let Some(locale) = &self.locale {
            let parts: Vec<_> = locale.split('-').collect();
            if locale.len() > 35
                || !(2..=3).contains(&parts[0].len())
                || !parts[0].bytes().all(|c| c.is_ascii_alphabetic())
                || parts.iter().skip(1).any(|p| {
                    !(2..=8).contains(&p.len()) || !p.bytes().all(|c| c.is_ascii_alphanumeric())
                })
            {
                return Err("模板语言标识无效".into());
            }
        }
        if self.tags.len() > 20 || self.tags.iter().collect::<HashSet<_>>().len() != self.tags.len()
        {
            return Err("模板标签重复或超过 20 个".into());
        }
        for tag in &self.tags {
            bounded(tag, 40, "模板标签")?;
        }
        let source = self.source.as_object().ok_or("模板来源格式无效")?;
        if !matches!(
            source.get("type").and_then(Value::as_str),
            Some(
                "builtin"
                    | "manual"
                    | "json_import"
                    | "docx_import"
                    | "doc_import"
                    | "builtin_copy"
                    | "duplicate"
                    | "legacy_migration"
            )
        ) {
            return Err("模板来源类型无效".into());
        }
        let allowed = [
            "type",
            "original_file_name",
            "original_file_sha256",
            "imported_at",
            "copied_from_template_id",
        ];
        if source.keys().any(|key| !allowed.contains(&key.as_str())) {
            return Err("模板来源包含未知字段".into());
        }
        for key in &allowed[1..] {
            if source
                .get(*key)
                .is_some_and(|v| !v.is_null() && !v.is_string())
            {
                return Err(format!("模板来源 {key} 必须是文本"));
            }
        }
        if let Some(name) = source.get("original_file_name").and_then(Value::as_str) {
            bounded(name, 255, "导入文件名")?;
            if name.contains(['/', '\\']) || name.chars().any(char::is_control) {
                return Err("导入文件名无效".into());
            }
        }
        if let Some(hash) = source.get("original_file_sha256").and_then(Value::as_str) {
            if hash.len() != 64
                || !hash
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            {
                return Err("导入文件校验值无效".into());
            }
        }
        if let Some(date) = source.get("imported_at").and_then(Value::as_str) {
            chrono::DateTime::parse_from_rfc3339(date).map_err(|_| "导入日期无效")?;
        }
        match source.get("type").and_then(Value::as_str) {
            Some("json_import" | "docx_import" | "doc_import") => {
                for key in ["original_file_name", "original_file_sha256", "imported_at"] {
                    if source.get(key).and_then(Value::as_str).is_none() {
                        return Err(format!("模板来源缺少 {key}"));
                    }
                }
            }
            Some("builtin_copy" | "duplicate") => {
                valid_id(
                    source
                        .get("copied_from_template_id")
                        .and_then(Value::as_str)
                        .ok_or("缺少原模板标识")?,
                )?;
            }
            _ => {}
        }
        if self.sections.is_empty() || self.sections.len() > 50 {
            return Err("模板必须包含 1 至 50 个章节".into());
        }
        let mut ids = HashSet::new();
        let mut titles = HashSet::new();
        for section in &self.sections {
            bounded(&section.id, 80, "章节标识")?;
            if !section
                .id
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
                || !ids.insert(&section.id)
            {
                return Err("章节标识无效或重复".into());
            }
            bounded(&section.title, 120, "章节标题")?;
            if !titles.insert(section.title.trim()) {
                return Err("章节标题不能重复".into());
            }
            bounded(&section.instruction, 10_000, "章节指令")?;
            if !matches!(section.format.as_str(), "paragraph" | "list" | "string")
                || !matches!(
                    section.empty_behavior.as_str(),
                    "omit" | "show_not_mentioned"
                )
            {
                return Err("章节格式或空内容处理方式无效".into());
            }
            for hint in [&section.item_format, &section.example_item_format]
                .into_iter()
                .flatten()
            {
                if hint.chars().count() > 5000 {
                    return Err("章节格式提示超过 5000 字".into());
                }
            }
        }
        if serde_json::to_vec(self).map_err(|e| e.to_string())?.len() > MAX_JSON_BYTES {
            return Err("模板 JSON 超过 800KB".into());
        }
        Ok(())
    }

    pub fn runtime(&self) -> Template {
        Template {
            name: self.name.clone(),
            description: self.description.clone(),
            sections: self
                .sections
                .iter()
                .map(|s| TemplateSection {
                    title: s.title.clone(),
                    instruction: format!(
                        "{}\n{}",
                        s.instruction,
                        if s.empty_behavior == "omit" && !s.required {
                            "没有转写依据时省略此章节，不得编造。"
                        } else {
                            "没有转写依据时保留章节并标记为未提及，不得编造。"
                        }
                    ),
                    format: s.format.clone(),
                    item_format: s.item_format.clone(),
                    example_item_format: s.example_item_format.clone(),
                })
                .collect(),
        }
    }
}

macro_rules! builtins {
    ($($id:literal),*) => {{
        let mut result = Vec::new();
        $(result.push(($id.to_owned(), include_str!(concat!("../../../templates/en/", $id, ".json"))));
          result.push((concat!($id, "_zh_cn").to_owned(), include_str!(concat!("../../../templates/zh-CN/", $id, ".json"))));)*
        result
    }};
}
fn builtin_sources() -> Vec<(String, &'static str)> {
    builtins!(
        "standard_meeting",
        "daily_standup",
        "project_sync",
        "retrospective",
        "sales_marketing_client_call",
        "psychatric_session"
    )
}
fn builtin(id: &str) -> Result<Option<Document>, String> {
    builtin_sources()
        .into_iter()
        .find(|(key, _)| key == id)
        .map(|(_, text)| {
            let mut doc = Document::parse(text, id)?;
            doc.id = id.into();
            Ok(doc)
        })
        .transpose()
}

pub struct Repository {
    root: PathBuf,
    legacy: Option<PathBuf>,
}
impl Repository {
    pub fn new(root: PathBuf, legacy: Option<PathBuf>) -> Self {
        Self { root, legacy }
    }
    fn path(&self, id: &str) -> Result<PathBuf, String> {
        valid_id(id)?;
        Ok(self.root.join(format!("{id}.json")))
    }
    fn read_file(path: &Path, id: &str) -> Result<Option<Document>, String> {
        match std::fs::metadata(path) {
            Ok(meta) if meta.len() > MAX_JSON_BYTES as u64 => Err("模板文件超过 800KB".into()),
            Ok(_) => {
                let text =
                    std::fs::read_to_string(path).map_err(|e| format!("无法读取模板：{e}"))?;
                let mut doc = Document::parse(&text, id)?;
                // 旧目录里的文件名一直是会议引用标识，保留原有引用。
                doc.id = id.into();
                Ok(Some(doc))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("无法读取模板：{e}")),
        }
    }
    pub fn get(&self, id: &str) -> Result<(Document, bool), String> {
        valid_id(id)?;
        if self.root.join("deleted").join(id).exists() {
            return Err("模板已删除，请重新选择模板".into());
        }
        if let Some(doc) = Self::read_file(&self.path(id)?, id)? {
            return Ok((doc, false));
        }
        if let Some(legacy) = &self.legacy {
            if let Some(doc) = Self::read_file(&legacy.join(format!("{id}.json")), id)? {
                return Ok((doc, false));
            }
        }
        builtin(id)?
            .map(|doc| (doc, true))
            .ok_or_else(|| format!("模板 {id} 不存在，请重新选择模板"))
    }
    fn ids(&self) -> Result<Vec<String>, String> {
        let mut ids: HashSet<String> = builtin_sources().into_iter().map(|(id, _)| id).collect();
        for dir in std::iter::once(&self.root).chain(self.legacy.iter()) {
            if !dir.exists() {
                continue;
            }
            for entry in std::fs::read_dir(dir).map_err(|e| format!("无法读取模板目录：{e}"))?
            {
                let path = entry.map_err(|e| e.to_string())?.path();
                if path.extension().is_some_and(|e| e == "json")
                    && path.file_stem().is_some_and(|s| s != "preferences")
                {
                    if let Some(id) = path.file_stem().and_then(|s| s.to_str()) {
                        ids.insert(id.into());
                    }
                }
            }
        }
        let mut ids: Vec<_> = ids
            .into_iter()
            .filter(|id| !self.root.join("deleted").join(id).exists())
            .collect();
        ids.sort();
        Ok(ids)
    }
    pub fn default_id(&self) -> Result<String, String> {
        let path = self.root.join("preferences.json");
        if !path.exists() {
            return Ok("standard_meeting_zh_cn".into());
        }
        let value: Value = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|_| "默认模板设置损坏，未覆盖原设置")?;
        let id = value
            .get("defaultTemplateId")
            .and_then(Value::as_str)
            .ok_or("默认模板设置无效")?;
        self.get(id)?;
        Ok(id.into())
    }
    pub fn list(&self) -> Result<Library, String> {
        let mut warnings = vec![];
        let mut templates = vec![];
        for id in self.ids()? {
            match self.get(&id) {
                Ok((doc, readonly)) => templates.push(Entry {
                    id,
                    name: doc.name,
                    description: doc.description,
                    locale: doc.locale,
                    version: doc.version,
                    readonly,
                }),
                Err(e) => warnings.push(format!("{id}：{e}")),
            }
        }
        let default_template_id = match self.default_id() {
            Ok(id) => id,
            Err(e) => {
                warnings.push(e);
                String::new()
            }
        };
        Ok(Library {
            templates,
            default_template_id,
            warnings,
        })
    }
    fn write(&self, path: &Path, value: &impl Serialize) -> Result<(), String> {
        std::fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        let mut temp = tempfile::NamedTempFile::new_in(&self.root).map_err(|e| e.to_string())?;
        temp.write_all(&serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        temp.as_file().sync_all().map_err(|e| e.to_string())?;
        temp.persist(path).map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn save(
        &self,
        mut doc: Document,
        expected_version: Option<u64>,
    ) -> Result<Document, String> {
        doc.validate()?;
        if doc.id == "preferences" {
            return Err("模板标识已保留".into());
        }
        match expected_version {
            Some(version) => {
                let (old, readonly) = self.get(&doc.id)?;
                if readonly {
                    return Err("内置模板请先复制再编辑".into());
                }
                if old.version != version {
                    return Err("模板已在其他窗口修改，请重新加载后编辑".into());
                }
                doc.created_at = old.created_at;
                doc.source = old.source;
                doc.version = old.version + 1;
            }
            None => {
                if self.root.join("deleted").join(&doc.id).exists()
                    || self
                        .ids()?
                        .iter()
                        .any(|id| id.eq_ignore_ascii_case(&doc.id))
                {
                    return Err("模板标识已存在，请复制或导入为新模板".into());
                }
                doc.version = 1;
            }
        }
        doc.updated_at = Utc::now().to_rfc3339();
        doc.validate()?;
        self.write(&self.path(&doc.id)?, &doc)?;
        Ok(doc)
    }
    pub fn duplicate(&self, id: &str) -> Result<Document, String> {
        let (mut doc, readonly) = self.get(id)?;
        doc.id = format!("custom_{}", uuid::Uuid::new_v4().simple());
        doc.name = format!("{}（副本）", doc.name.chars().take(110).collect::<String>());
        doc.source = json!({"type":if readonly{"builtin_copy"}else{"duplicate"},"copied_from_template_id":id});
        doc.created_at = Utc::now().to_rfc3339();
        self.save(doc, None)
    }
    pub fn import(&self, text: &str) -> Result<Document, String> {
        let id = format!("custom_{}", uuid::Uuid::new_v4().simple());
        let mut doc = Document::parse(text, &id)?;
        // 导入永远生成新标识，不覆盖现有会议所引用的模板。
        doc.id = id;
        doc.source = json!({"type":"json_import","original_file_name":"import.json","original_file_sha256":format!("{:x}",Sha256::digest(text.as_bytes())),"imported_at":Utc::now().to_rfc3339()});
        doc.created_at = Utc::now().to_rfc3339();
        self.save(doc, None)
    }
    pub fn set_default(&self, id: &str) -> Result<(), String> {
        self.get(id)?;
        self.write(
            &self.root.join("preferences.json"),
            &json!({"defaultTemplateId":id}),
        )
    }
    pub fn delete(&self, id: &str) -> Result<(), String> {
        let (_, readonly) = self.get(id)?;
        if readonly {
            return Err("内置模板不能删除".into());
        }
        if self.default_id()? == id {
            return Err("请先选择其他默认模板，再删除此模板".into());
        }
        // 留下删除标记，保留旧目录和自定义文件，避免旧模板回退后重新出现。
        let deleted = self.root.join("deleted");
        std::fs::create_dir_all(&deleted).map_err(|e| e.to_string())?;
        self.write(
            &deleted.join(id),
            &json!({"deleted_at":Utc::now().to_rfc3339()}),
        )
    }
}

pub fn initialize(root: PathBuf, legacy: Option<PathBuf>) -> Result<(), String> {
    *STORE.lock().map_err(|_| "模板库锁不可用")? = Some(Repository::new(root, legacy));
    Ok(())
}
pub fn with_store<T>(f: impl FnOnce(&Repository) -> Result<T, String>) -> Result<T, String> {
    let store = STORE.lock().map_err(|_| "模板库锁不可用")?;
    f(store.as_ref().ok_or("公司模板库尚未初始化")?)
}
pub fn runtime(id: &str) -> Result<Template, String> {
    valid_id(id)?;
    let store = STORE.lock().map_err(|_| "模板库锁不可用")?;
    if let Some(store) = store.as_ref() {
        return Ok(store.get(id)?.0.runtime());
    }
    // 内置模板可在单元测试和启动前读取；不回退到其他公司的全局目录。
    builtin(id)?
        .map(|doc| doc.runtime())
        .ok_or_else(|| "公司模板库尚未初始化".into())
}
pub fn default_for_meeting() -> Result<String, String> {
    let store = STORE.lock().map_err(|_| "模板库锁不可用")?;
    match store.as_ref() {
        Some(repo) => repo.default_id(),
        None => Ok("standard_meeting".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_builtins_and_three_column_actions() {
        assert_eq!(builtin_sources().len(), 12);
        for (id, _) in builtin_sources() {
            builtin(&id).unwrap().unwrap().validate().unwrap();
        }
        let doc = builtin("standard_meeting_zh_cn").unwrap().unwrap();
        let action = doc
            .sections
            .iter()
            .find(|s| s.id == "action_items")
            .unwrap();
        assert_eq!(
            action
                .item_format
                .as_deref()
                .unwrap()
                .lines()
                .next()
                .unwrap()
                .matches('|')
                .count(),
            4
        );
    }
    #[test]
    fn company_isolation_default_conflicts_and_snapshot() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let a = Repository::new(a.path().into(), None);
        let b = Repository::new(b.path().into(), None);
        let mut doc = a.duplicate("standard_meeting_zh_cn").unwrap();
        a.set_default(&doc.id).unwrap();
        assert_ne!(a.default_id().unwrap(), b.default_id().unwrap());
        assert!(b.get(&doc.id).is_err());
        let snapshot = a.get(&doc.id).unwrap().0.runtime();
        doc.sections[0].instruction = "更改后的指令".into();
        let saved = a.save(doc.clone(), Some(1)).unwrap();
        assert_eq!(saved.version, 2);
        assert!(a.save(doc, Some(1)).is_err());
        assert!(!snapshot.sections[0].instruction.contains("更改后的指令"));
        assert!(a.delete(&saved.id).is_err());
        a.set_default("daily_standup").unwrap();
        a.delete(&saved.id).unwrap();
        assert!(a.get(&saved.id).is_err());
    }
    #[test]
    fn legacy_import_and_invalid_data_never_overwrite() {
        let root = tempfile::tempdir().unwrap();
        let legacy = tempfile::tempdir().unwrap();
        let json = r#"{"name":"旧模板","description":"兼容旧会议","sections":[{"title":"摘要","instruction":"概括内容","format":"paragraph"}]}"#;
        std::fs::write(legacy.path().join("研发周会.json"), json).unwrap();
        let repo = Repository::new(root.path().into(), Some(legacy.path().into()));
        assert_eq!(repo.get("研发周会").unwrap().0.name, "旧模板");
        let imported = repo.import(json).unwrap();
        assert_ne!(imported.id, "研发周会");
        repo.delete("研发周会").unwrap();
        assert!(repo.get("研发周会").is_err());
        assert!(repo.import("{}").is_err());
        assert!(repo.get("../secret").is_err());
        assert!(repo.get("CON").is_err());
        std::fs::write(root.path().join("broken.json"), "{").unwrap();
        assert!(repo.get("broken").is_err());
        assert_eq!(repo.list().unwrap().warnings.len(), 1);
        assert_eq!(
            std::fs::read_to_string(legacy.path().join("研发周会.json")).unwrap(),
            json
        );
    }
    #[test]
    fn invalid_v2_and_reserved_paths_are_rejected() {
        let mut doc = builtin("standard_meeting").unwrap().unwrap();
        doc.sections[1].id = doc.sections[0].id.clone();
        assert!(doc.validate().is_err());
        assert!(valid_id("Preferences").is_err());
        assert!(valid_id("file.").is_err());
        assert!(valid_id("LPT1").is_err());
        let root = tempfile::tempdir().unwrap();
        let repo = Repository::new(root.path().into(), None);
        std::fs::write(root.path().join("preferences.json"), "invalid").unwrap();
        assert!(repo.default_id().is_err());
        assert_eq!(repo.list().unwrap().warnings.len(), 1);
    }
}
