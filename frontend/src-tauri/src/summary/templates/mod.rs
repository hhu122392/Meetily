//! 纪要模板：内置中英文 v2 模板、自定义模板与旧 JSON 兼容。
//! 公司目录下的 summary-templates 保存自定义内容；原公司独占读取旧 Meetily/templates 目录。
//! 生成任务取得独立的运行时模板副本，后续编辑不会改变已启动任务。

pub mod catalog;
mod defaults;
mod loader;
mod types;

// Re-export public API
pub use loader::{
    get_template, list_template_ids, list_templates, set_bundled_templates_dir,
    validate_and_parse_template,
};
pub use types::{Template, TemplateSection};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_integration() {
        // Test that we can load all built-in templates
        let ids = list_template_ids();
        assert!(!ids.is_empty());

        for id in ids {
            let result = get_template(&id);
            assert!(
                result.is_ok(),
                "Failed to load template '{}': {:?}",
                id,
                result.err()
            );
        }
    }

    #[test]
    fn test_template_metadata() {
        let templates = list_templates();
        assert!(!templates.is_empty());

        for (id, name, description) in templates {
            assert!(!id.is_empty());
            assert!(!name.is_empty());
            assert!(!description.is_empty());
        }
    }
}
