use serde::{Deserialize, Serialize};

/// Represents a single section in a meeting template
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateSection {
    /// Section title (e.g., "Summary", "Action Items")
    pub title: String,

    /// Instruction for the LLM on what to extract/include
    pub instruction: String,

    /// Format type: "paragraph", "list", or "string"
    pub format: String,

    /// Optional markdown formatting hint for list items (e.g., table structure)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_format: Option<String>,

    /// Alternative formatting hint
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example_item_format: Option<String>,
}

/// Represents a complete meeting template
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Template {
    /// Template display name
    pub name: String,

    /// Brief description of the template's purpose
    pub description: String,

    /// List of sections in the template
    pub sections: Vec<TemplateSection>,
}

impl Template {
    /// Validates the template structure
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("Template name cannot be empty".to_string());
        }

        if self.description.is_empty() {
            return Err("Template description cannot be empty".to_string());
        }

        if self.sections.is_empty() {
            return Err("Template must have at least one section".to_string());
        }

        for (i, section) in self.sections.iter().enumerate() {
            if section.title.is_empty() {
                return Err(format!("Section {} has empty title", i));
            }

            if section.instruction.is_empty() {
                return Err(format!("Section '{}' has empty instruction", section.title));
            }

            match section.format.as_str() {
                "paragraph" | "list" | "string" => {},
                other => return Err(format!(
                    "Section '{}' has invalid format '{}'. Must be 'paragraph', 'list', or 'string'",
                    section.title, other
                )),
            }
        }

        Ok(())
    }

    /// Generates a clean markdown template structure
    pub fn to_markdown_structure(&self) -> String {
        let mut markdown = String::from("# <Add Title here>\n\n");

        for section in &self.sections {
            markdown.push_str(&format!("**{}**\n\n", section.title));
        }

        markdown
    }

    /// Generates section-specific instructions for the LLM
    pub fn to_section_instructions(&self) -> String {
        let mut instructions = String::from(
            "- **For the main title (`# [AI-Generated Title]`):** Analyze the entire transcript and create a concise, descriptive title for the meeting.\n"
        );

        for section in &self.sections {
            instructions.push_str(&format!(
                "- **For the '{}' section:** {}.\n",
                section.title, section.instruction
            ));

            // A concrete item/example format takes precedence over the general layout.
            let item_format = section
                .item_format
                .as_deref()
                .filter(|format| !format.trim().is_empty())
                .or_else(|| {
                    section
                        .example_item_format
                        .as_deref()
                        .filter(|format| !format.trim().is_empty())
                });

            if let Some(format) = item_format {
                instructions.push_str(&format!(
                    "  - Items in this section should follow the format: `{}`.\n",
                    format
                ));
            } else {
                let layout = match section.format.as_str() {
                    "paragraph" => "Use prose paragraphs, not a table or bullet list.",
                    "list" => "Use a Markdown bullet list with one distinct item per bullet. Do not convert it into a table.",
                    "string" => "Use a single text value, not a bullet list or table.",
                    _ => continue, // validate() rejects unsupported layouts.
                };
                instructions.push_str(&format!("  - {}\n", layout));
            }
        }

        instructions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout_template(format: &str, item_format: Option<&str>, example: Option<&str>) -> Template {
        Template {
            name: "Layout".to_owned(),
            description: "Only the layout differs".to_owned(),
            sections: vec![TemplateSection {
                title: "Facts".to_owned(),
                instruction: "Keep the source facts".to_owned(),
                format: format.to_owned(),
                item_format: item_format.map(str::to_owned),
                example_item_format: example.map(str::to_owned),
            }],
        }
    }

    #[test]
    fn changing_declared_format_changes_generation_instructions() {
        let paragraph = layout_template("paragraph", None, None).to_section_instructions();
        let list = layout_template("list", None, None).to_section_instructions();
        let string = layout_template("string", None, None).to_section_instructions();
        assert_ne!(paragraph, list, "The selected template format must reach the model");
        assert_ne!(paragraph, string);
        assert_ne!(list, string);
    }

    #[test]
    fn unformatted_items_use_the_declared_layout() {
        assert!(layout_template("paragraph", None, None).to_section_instructions().contains("prose paragraph"));
        assert!(layout_template("list", None, None).to_section_instructions().contains("Markdown bullet list"));
        assert!(layout_template("string", None, None).to_section_instructions().contains("single text value"));
    }

    #[test]
    fn explicit_item_format_remains_authoritative_including_tables() {
        let table = "| Task | Owner | Deadline |";
        let prompt = layout_template("list", Some(table), None).to_section_instructions();
        assert!(prompt.contains(table));
        assert!(!prompt.contains("Markdown bullet list"), "Do not contradict an explicit table format");
        let fallback = layout_template("list", Some("  "), Some(table)).to_section_instructions();
        assert!(fallback.contains(table), "A blank item format must not hide the example format");
    }

    #[test]
    fn blank_item_formats_do_not_hide_the_declared_layout() {
        let prompt = layout_template("list", Some("\n"), Some(" \t")).to_section_instructions();
        assert!(prompt.contains("Markdown bullet list"));
        assert!(!prompt.contains("Items in this section should follow the format"));
    }

    #[test]
    fn test_validate_valid_template() {
        let template = Template {
            name: "Test Template".to_string(),
            description: "A test template".to_string(),
            sections: vec![TemplateSection {
                title: "Summary".to_string(),
                instruction: "Provide a summary".to_string(),
                format: "paragraph".to_string(),
                item_format: None,
                example_item_format: None,
            }],
        };

        assert!(template.validate().is_ok());
    }

    #[test]
    fn test_validate_empty_name() {
        let template = Template {
            name: "".to_string(),
            description: "A test template".to_string(),
            sections: vec![],
        };

        assert!(template.validate().is_err());
    }

    #[test]
    fn test_validate_invalid_format() {
        let template = Template {
            name: "Test".to_string(),
            description: "Test".to_string(),
            sections: vec![TemplateSection {
                title: "Test".to_string(),
                instruction: "Test".to_string(),
                format: "invalid".to_string(),
                item_format: None,
                example_item_format: None,
            }],
        };

        assert!(template.validate().is_err());
    }
}
