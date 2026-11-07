use super::{catalog, types::Template};
use std::path::PathBuf;

// All shipped locale variants are embedded; keep the startup API for older callers.
pub fn set_bundled_templates_dir(_path: PathBuf) {}
pub fn get_template(id: &str) -> Result<Template, String> {
    catalog::runtime(id)
}
pub fn validate_and_parse_template(text: &str) -> Result<Template, String> {
    Ok(catalog::Document::parse(text, "preview_template")?.runtime())
}
pub fn list_templates() -> Vec<(String, String, String)> {
    catalog::with_store(|repo| repo.list())
        .map(|list| {
            list.templates
                .into_iter()
                .map(|entry| (entry.id, entry.name, entry.description))
                .collect()
        })
        .unwrap_or_else(|_| {
            super::defaults::get_builtin_templates()
                .into_iter()
                .filter_map(|(id, _)| {
                    get_template(id)
                        .ok()
                        .map(|doc| (id.into(), doc.name, doc.description))
                })
                .collect()
        })
}
pub fn list_template_ids() -> Vec<String> {
    list_templates().into_iter().map(|(id, _, _)| id).collect()
}
