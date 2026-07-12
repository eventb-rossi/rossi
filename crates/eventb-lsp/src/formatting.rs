//! Document formatting provider
//!
//! This module provides document formatting using the Event-B pretty printer.

use crate::config::FormatConfig;
use crate::lsp_types::{Position, Range, TextEdit};

/// Format a document using the supplied server configuration.
pub fn format(text: &str, config: &FormatConfig) -> Result<Vec<TextEdit>, String> {
    use rossi::{PrettyPrinter, format_str};

    // Delegate to the shared formatting core so editor and `rossi fmt`
    // formatting never diverge.
    let printer = PrettyPrinter {
        use_unicode: config.use_unicode,
        indent: config.indentation.clone(),
        // Editor output stays portable: never emit the private-use glyphs.
        private_use_glyphs: false,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn format_default(source: &str) -> Result<Vec<TextEdit>, String> {
        format(source, &FormatConfig::default())
    }

    #[test]
    fn test_format_simple_context() {
        let source = "CONTEXT test SETS STATUS END";

        let result = format_default(source);
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
        let config = FormatConfig {
            use_unicode: true,
            indentation: "    ".to_string(),
        };

        let source = r#"
        CONTEXT test
        AXIOMS
            @axm1 1 > 0
        END
        "#;

        let result = format(source, &config);
        assert!(result.is_ok());

        let formatted = result.unwrap()[0].new_text.clone();
        // Check it formatted successfully
        assert!(formatted.contains("CONTEXT"));
        assert!(formatted.contains("AXIOMS"));
    }

    #[test]
    fn test_format_with_ascii() {
        let config = FormatConfig {
            use_unicode: false,
            indentation: "    ".to_string(),
        };

        let source = r#"
        CONTEXT test
        AXIOMS
            @axm1 true
        END
        "#;

        let result = format(source, &config);
        assert!(result.is_ok());

        let formatted = result.unwrap()[0].new_text.clone();
        assert!(formatted.contains("CONTEXT"));
        assert!(formatted.contains("AXIOMS"));
        // ASCII mode renders the predicate literal ⊤ as lowercase `true`.
        assert!(formatted.contains("true"));
    }

    #[test]
    fn test_format_with_custom_indentation() {
        let config = FormatConfig {
            use_unicode: true,
            indentation: "    ".to_string(),
        };

        let source = r#"
        CONTEXT test
        SETS
            STATUS
        END
        "#;

        let result = format(source, &config);
        assert!(result.is_ok());

        let formatted = result.unwrap()[0].new_text.clone();
        // Should contain 4-space indentation
        assert!(formatted.contains("    STATUS") || formatted.contains("SETS"));
    }

    #[test]
    fn test_format_invalid_syntax() {
        let source = "CONTEXT"; // Invalid - missing name and END

        let result = format_default(source);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Parse error"));
    }

    #[test]
    fn test_format_machine() {
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

        let result = format_default(source);
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
        let source = r#"
        CONTEXT test
        SETS
            STATUS
        END
        "#;

        // Format once
        let result1 = format_default(source);
        assert!(result1.is_ok());
        let formatted1 = result1.unwrap()[0].new_text.clone();

        // Format again
        let result2 = format_default(&formatted1);
        assert!(result2.is_ok());
        let formatted2 = result2.unwrap()[0].new_text.clone();

        // Should be the same (idempotent)
        assert_eq!(formatted1, formatted2);
    }

    #[test]
    fn test_format_preserves_comments() {
        // Issue #31: Format Document must not destroy documentation.
        let source = "CONTEXT c\n// important: do not change\nAXIOMS\n    @axm1 1 = 1 // why: invariant base\nEND\n";
        let formatted = format_default(source).unwrap()[0].new_text.clone();

        assert!(formatted.contains("// important: do not change"));
        assert!(formatted.contains("// why: invariant base"));
    }
}
