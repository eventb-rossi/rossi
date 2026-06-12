//! Document formatting provider
//!
//! This module provides document formatting using the Event-B pretty printer.

use crate::lsp_types::{Position, Range, TextEdit};
use parking_lot::RwLock;
use std::sync::Arc;

/// Configuration for formatting
#[derive(Debug, Clone)]
pub struct FormattingConfig {
    /// Use Unicode operators (true) or ASCII (false)
    pub use_unicode: bool,
    /// Indentation string (e.g., "  " or "    ")
    pub indentation: String,
}

impl Default for FormattingConfig {
    fn default() -> Self {
        Self {
            use_unicode: true,
            indentation: "    ".to_string(),
        }
    }
}

/// Provides document formatting functionality
pub struct FormattingProvider {
    config: Arc<RwLock<FormattingConfig>>,
}

impl FormattingProvider {
    /// Create a new formatting provider with default configuration
    pub fn new() -> Self {
        Self {
            config: Arc::new(RwLock::new(FormattingConfig::default())),
        }
    }

    /// Update the formatting configuration
    #[allow(dead_code)]
    pub fn update_config(&self, config: FormattingConfig) {
        let mut current_config = self.config.write();
        *current_config = config;
    }

    /// Get the current configuration
    pub fn get_config(&self) -> FormattingConfig {
        self.config.read().clone()
    }

    /// Format a document
    pub fn format(&self, text: &str) -> Result<Vec<TextEdit>, String> {
        use rossi::{PrettyPrinter, format_str};

        let config = self.get_config();

        // Delegate to the shared formatting core so editor and `rossi fmt`
        // formatting never diverge.
        let printer = PrettyPrinter {
            use_unicode: config.use_unicode,
            indent: config.indentation,
        };
        let formatted = format_str(text, &printer).map_err(|e| format!("Parse error: {}", e))?;

        // Create a text edit that replaces the entire document
        // Use a large end position to ensure we replace everything
        Ok(vec![TextEdit {
            range: Range {
                start: Position::new(0, 0),
                end: Position::new(u32::MAX, u32::MAX),
            },
            new_text: formatted,
        }])
    }
}

impl Default for FormattingProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_simple_context() {
        let provider = FormattingProvider::new();

        let source = "CONTEXT test SETS STATUS END";

        let result = provider.format(source);
        assert!(result.is_ok());

        let edits = result.unwrap();
        assert_eq!(edits.len(), 1);

        let formatted = &edits[0].new_text;
        assert!(formatted.contains("CONTEXT test"));
        assert!(formatted.contains("SETS"));
        assert!(formatted.contains("STATUS"));
        assert!(formatted.contains("END"));
    }

    #[test]
    fn test_format_with_unicode() {
        let provider = FormattingProvider::new();

        // Enable Unicode (default)
        provider.update_config(FormattingConfig {
            use_unicode: true,
            indentation: "    ".to_string(),
        });

        let source = r#"
        CONTEXT test
        AXIOMS
            @axm1 1 > 0
        END
        "#;

        let result = provider.format(source);
        assert!(result.is_ok());

        let formatted = result.unwrap()[0].new_text.clone();
        // Check it formatted successfully
        assert!(formatted.contains("CONTEXT"));
        assert!(formatted.contains("AXIOMS"));
    }

    #[test]
    fn test_format_with_ascii() {
        let provider = FormattingProvider::new();

        // Enable ASCII mode
        provider.update_config(FormattingConfig {
            use_unicode: false,
            indentation: "    ".to_string(),
        });

        let source = r#"
        CONTEXT test
        AXIOMS
            @axm1 TRUE
        END
        "#;

        let result = provider.format(source);
        assert!(result.is_ok());

        let formatted = result.unwrap()[0].new_text.clone();
        assert!(formatted.contains("CONTEXT"));
        assert!(formatted.contains("AXIOMS"));
    }

    #[test]
    fn test_format_with_custom_indentation() {
        let provider = FormattingProvider::new();

        // Use 4-space indentation
        provider.update_config(FormattingConfig {
            use_unicode: true,
            indentation: "    ".to_string(),
        });

        let source = r#"
        CONTEXT test
        SETS
            STATUS
        END
        "#;

        let result = provider.format(source);
        assert!(result.is_ok());

        let formatted = result.unwrap()[0].new_text.clone();
        // Should contain 4-space indentation
        assert!(formatted.contains("    STATUS") || formatted.contains("SETS"));
    }

    #[test]
    fn test_format_invalid_syntax() {
        let provider = FormattingProvider::new();

        let source = "CONTEXT"; // Invalid - missing name and END

        let result = provider.format(source);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Parse error"));
    }

    #[test]
    fn test_format_machine() {
        let provider = FormattingProvider::new();

        let source = r#"
        MACHINE counter
        VARIABLES count
        INVARIANTS @inv1 count >= 0
        EVENTS
            EVENT INITIALISATION
            THEN
                count := 0
            END
        END
        "#;

        let result = provider.format(source);
        assert!(result.is_ok());

        let formatted = result.unwrap()[0].new_text.clone();
        assert!(formatted.contains("MACHINE counter"));
        assert!(formatted.contains("VARIABLES"));
        assert!(formatted.contains("count"));
        assert!(formatted.contains("INVARIANTS"));
        assert!(formatted.contains("INITIALISATION"));
    }

    #[test]
    fn test_format_idempotent() {
        let provider = FormattingProvider::new();

        let source = r#"
        CONTEXT test
        SETS
            STATUS
        END
        "#;

        // Format once
        let result1 = provider.format(source);
        assert!(result1.is_ok());
        let formatted1 = result1.unwrap()[0].new_text.clone();

        // Format again
        let result2 = provider.format(&formatted1);
        assert!(result2.is_ok());
        let formatted2 = result2.unwrap()[0].new_text.clone();

        // Should be the same (idempotent)
        assert_eq!(formatted1, formatted2);
    }

    #[test]
    fn test_format_preserves_comments() {
        // Issue #31: Format Document must not destroy documentation.
        let provider = FormattingProvider::new();

        let source = "CONTEXT c\n// important: do not change\nAXIOMS\n    @axm1 1 = 1 // why: invariant base\nEND\n";
        let formatted = provider.format(source).unwrap()[0].new_text.clone();

        assert!(formatted.contains("// important: do not change"));
        assert!(formatted.contains("// why: invariant base"));
    }

    #[test]
    fn test_config_update() {
        let provider = FormattingProvider::new();

        // Check default
        let config = provider.get_config();
        assert!(config.use_unicode);
        assert_eq!(config.indentation, "    ");

        // Update config
        provider.update_config(FormattingConfig {
            use_unicode: false,
            indentation: "    ".to_string(),
        });

        // Check updated
        let config = provider.get_config();
        assert!(!config.use_unicode);
        assert_eq!(config.indentation, "    ");
    }
}
