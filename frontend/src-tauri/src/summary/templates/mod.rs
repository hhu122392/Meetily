//! Meeting summary template management
//!
//! This module provides a flexible template system for generating meeting summaries.
//! It supports both built-in templates (embedded in the binary) and custom user templates
//! (loaded from the application data directory).
//!
//! # Architecture
//!
//! - **Built-in templates**: JSON files in `frontend/src-tauri/templates/` embedded at compile time
//! - **Custom templates**: JSON files in platform-specific app data directory
//! - **Fallback strategy**: Custom templates override built-in templates with the same ID
//!
//! # Usage
//!
//! ```rust
//! use app_lib::summary::templates;
//!
//! // Load a specific template
//! let template = templates::get_template("daily_standup").expect("daily standup template exists");
//!
//! // Generate markdown structure
//! let markdown = template.to_markdown_structure();
//!
//! // Generate LLM instructions
//! let instructions = template.to_section_instructions();
//!
//! // List available templates
//! let available = templates::list_templates();
//! ```
//!
//! # Custom Templates
//!
//! Users can add custom templates to:
//! - macOS: `~/Library/Application Support/Meetily/templates/`
//! - Windows: `%APPDATA%\Meetily\templates\`
//! - Linux: `~/.config/Meetily/templates/`
//!
//! Custom templates must follow the JSON schema defined in `types::Template`.

mod content_locale;
mod defaults;
mod document_import;
mod loader;
mod portable_package;
mod repository;
mod service;
mod types;
mod v2;

// Re-export public API
pub use content_locale::{
    canonical_content_locale, content_locale_for_summary_language, resolve_content_locale,
    ContentLocaleResolution, DEFAULT_CONTENT_LOCALE, SIMPLIFIED_CHINESE_CONTENT_LOCALE,
    SUPPORTED_CONTENT_LOCALES,
};
pub use document_import::{
    preview_template_document, preview_template_document_cancellable, DocumentImportCancellation,
    DocumentImportConfidence, DocumentImportError, DocumentImportPreview, DocumentImportWarning,
    DocumentOutlineKind, DocumentOutlineNode,
};
pub use loader::{
    get_template, list_template_ids, list_templates, set_bundled_templates_dir,
    validate_and_parse_template,
};
pub use portable_package::{
    cancel_template_pack_import, execute_template_pack_import, export_template_pack,
    plan_template_pack_import, preview_template_pack_export, preview_template_pack_import,
    recover_template_pack_imports, CancelTemplatePackImportRequest,
    CancelTemplatePackImportResponse, ExecuteTemplatePackImportRequest,
    ExecuteTemplatePackImportResponse, ExportTemplatePackRequest, ExportTemplatePackResponse,
    PlanTemplatePackImportRequest, PlanTemplatePackImportResponse, PortablePackAuditSummary,
    PortablePackConflictKind, PortablePackConflictStrategy, PortablePackExportTemplate,
    PortablePackImportDecision, PortablePackImportExecutionItem, PortablePackImportItem,
    PortablePackImportOperationKind, PortablePackImportPlanSummary, PortablePackImportTemplate,
    PortablePackPlannedImportOperation, PortablePackRecoverySummary,
    PortablePackTemplateFingerprint, PortablePackWarning, PreviewTemplatePackExportRequest,
    PreviewTemplatePackExportResponse, PreviewTemplatePackImportRequest,
    PreviewTemplatePackImportResponse, PORTABLE_PACK_EXTENSION,
};
pub use repository::{
    templates_root_from_data_dir, CreateConflictPolicy, DeleteTemplateResult,
    DeletedTemplateListItem, RestoreConflictPolicy, TemplateListItem, TemplateListResult,
    TemplateOrigin, TemplateRecord, TemplateRepository, TemplateRepositoryDiagnostic,
    TemplateRepositoryError, TemplateRepositoryErrorKind, TemplateRepositoryResult,
};
pub use service::*;
pub use types::{Template, TemplateSection};
pub use v2::{
    migrate_v1_to_v2, parse_and_validate_template_v2, validate_template_v2,
    validate_template_v2_value, EmptyBehavior, TemplateFieldIssue, TemplateFormat,
    TemplateSectionV2, TemplateSource, TemplateSourceType, TemplateV2, TemplateValidationResult,
};

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
