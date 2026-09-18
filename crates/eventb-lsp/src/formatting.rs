//! Document formatting provider
//!
//! This module provides document formatting using the Event-B pretty printer.

use crate::config::FormatConfig;
use crate::lsp_types::{Position, Range, TextEdit};
use rossi::Component;

/// Format a document using the supplied server configuration.
pub fn format(text: &str, config: &FormatConfig) -> Result<Vec<TextEdit>, String> {
    // Delegate to the shared formatting core so editor and `rossi fmt`
    // formatting never diverge; the printer comes from the one config
    // mapping ([`FormatConfig::printer`]) shared with the Rodin model sync.
    let printer = config.printer();
    let formatted = rossi::format_str(text, &printer).map_err(|e| format!("Parse error: {}", e))?;

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

/// Format the components that `range` touches, leaving the rest of the
/// document byte-for-byte alone.
///
/// The pretty printer works on whole components: a formula cannot be
/// re-laid-out without the clause it sits in, and a clause not without its
/// component. So the selection snaps outward to the components it
/// intersects, and those are printed together as one slice. A selection
/// touching no component (whitespace between two, or a file that did not
/// parse far enough) formats nothing rather than guessing.
pub fn format_range(
    text: &str,
    components: &[Component],
    range: Range,
    config: &FormatConfig,
) -> Result<Vec<TextEdit>, String> {
    let start = crate::position::position_to_offset(text, range.start)
        .ok_or_else(|| "range start is outside the document".to_string())?;
    let end = crate::position::position_to_offset(text, range.end)
        .ok_or_else(|| "range end is outside the document".to_string())?;

    let touched: Vec<rossi::ast::Span> = components
        .iter()
        .filter_map(|component| component.span())
        .filter(|span| span.start <= end && start <= span.end)
        .collect();
    let (Some(first), Some(last)) = (touched.first(), touched.last()) else {
        return Ok(Vec::new());
    };
    let slice = rossi::ast::Span {
        start: first.start,
        end: last.end,
    };

    let printer = config.printer();
    let formatted = rossi::format_str(&text[slice.start..slice.end], &printer)
        .map_err(|e| format!("Parse error: {}", e))?;
    // `format_str` ends its output with a newline; the slice ends at the
    // component's END, so drop the terminator to keep whatever followed it.
    let formatted = formatted
        .strip_suffix('\n')
        .unwrap_or(&formatted)
        .to_string();
    Ok(vec![TextEdit {
        range: crate::position::span_to_range(&slice, text),
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
        assert!(formatted.contains("context test"));
        assert!(formatted.contains("sets STATUS"));
        assert!(formatted.ends_with("end\n"));
    }

    #[test]
    fn test_format_with_unicode() {
        let config = FormatConfig {
            use_unicode: true,
            indentation: "    ".to_string(),
            ..FormatConfig::default()
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
        assert!(formatted.contains("context"));
        assert!(formatted.contains("axioms"));
    }

    #[test]
    fn test_format_with_ascii() {
        let config = FormatConfig {
            use_unicode: false,
            indentation: "    ".to_string(),
            ..FormatConfig::default()
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
        assert!(formatted.contains("context"));
        assert!(formatted.contains("axioms"));
        // ASCII mode renders the predicate literal ⊤ as lowercase `true`.
        assert!(formatted.contains("true"));
    }

    #[test]
    fn test_format_with_camille_style() {
        let config = FormatConfig {
            style: "camille".to_string(),
            ..FormatConfig::default()
        };

        let source = "CONTEXT test SETS STATUS END";
        let formatted = format(source, &config).unwrap()[0].new_text.clone();
        assert!(
            formatted.starts_with("context test\n\nsets STATUS\n"),
            "expected camille-style output, got:\n{formatted}"
        );
    }

    #[test]
    fn test_format_with_custom_indentation() {
        let config = FormatConfig {
            use_unicode: true,
            indentation: "    ".to_string(),
            ..FormatConfig::default()
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
        // The explicit 4-space indentation overrides the preset's 2 spaces
        // for indented items (the camille sets list itself stays inline).
        assert!(formatted.contains("sets STATUS"), "got:\n{formatted}");
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
                @act1 count := 0
            END
        END
        "#;

        let result = format_default(source);
        assert!(result.is_ok());

        let formatted = result.unwrap()[0].new_text.clone();
        assert!(formatted.contains("machine counter"));
        assert!(formatted.contains("variables count"));
        assert!(formatted.contains("invariants"));
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
