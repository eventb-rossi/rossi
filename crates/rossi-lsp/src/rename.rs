//! Symbol rename functionality
//!
//! This module provides the ability to rename Event-B symbols (variables, constants,
//! sets, events) safely by updating all references throughout the document and
//! across the workspace.

use lsp_types::*;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::debug;

use crate::cross_references::CrossReferenceManager;
use crate::document::DocumentManager;
use crate::text_utils;

/// Provider for renaming symbols
pub struct RenameProvider {
    /// Cross-reference manager for workspace-wide navigation
    cross_ref_manager: Option<Arc<CrossReferenceManager>>,
    /// Document manager to access open documents
    document_manager: Option<Arc<DocumentManager>>,
}

impl Default for RenameProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl RenameProvider {
    /// Create a new rename provider
    pub fn new() -> Self {
        Self {
            cross_ref_manager: None,
            document_manager: None,
        }
    }

    /// Set the cross-reference manager for workspace-wide navigation
    pub fn set_cross_reference_manager(&mut self, manager: Arc<CrossReferenceManager>) {
        self.cross_ref_manager = Some(manager);
    }

    /// Set the document manager for accessing open documents
    pub fn set_document_manager(&mut self, manager: Arc<DocumentManager>) {
        self.document_manager = Some(manager);
    }

    /// Prepare for rename: validate the position and return the range of the symbol
    pub fn prepare_rename(&self, params: &TextDocumentPositionParams, text: &str) -> Option<Range> {
        let position = params.position;

        // Get the identifier at the cursor position
        let (identifier, range) = get_identifier_and_range_at_position(text, position)?;

        debug!(
            "Prepare rename for identifier '{}' at {:?}",
            identifier, position
        );

        // Check if this identifier is a keyword (keywords cannot be renamed)
        if is_keyword(&identifier) {
            debug!("Cannot rename keyword '{}'", identifier);
            return None;
        }

        Some(range)
    }

    /// Perform the rename operation
    pub fn rename(&self, params: &RenameParams, text: &str) -> Option<WorkspaceEdit> {
        let position = params.text_document_position.position;
        let uri = &params.text_document_position.text_document.uri;
        let new_name = &params.new_name;

        // Get the identifier at the cursor position
        let (identifier, _) = get_identifier_and_range_at_position(text, position)?;

        debug!(
            "Renaming identifier '{}' to '{}' at {:?}",
            identifier, new_name, position
        );

        // Validate new name
        if !is_valid_identifier(new_name) {
            debug!("Invalid new name: '{}'", new_name);
            return None;
        }

        // Check if new name is a keyword
        if is_keyword(new_name) {
            debug!("Cannot rename to keyword: '{}'", new_name);
            return None;
        }

        // Check if this is a component name that should be renamed across files
        let is_component = self.is_component_name(&identifier);

        let mut changes = HashMap::new();

        if is_component {
            // Rename across all workspace files
            debug!("Renaming component '{}' across workspace", identifier);
            self.rename_across_workspace(&identifier, new_name, &mut changes);
        } else {
            // Rename only in the current document
            debug!("Renaming symbol '{}' in current document", identifier);
            let locations = find_all_references(text, &identifier, uri)?;

            if locations.is_empty() {
                return None;
            }

            // Create text edits for all references
            let mut edits: Vec<TextEdit> = locations
                .into_iter()
                .map(|loc| TextEdit {
                    range: loc.range,
                    new_text: new_name.clone(),
                })
                .collect();

            // Sort edits in reverse order (bottom to top, right to left)
            edits.sort_by(|a, b| {
                b.range
                    .start
                    .line
                    .cmp(&a.range.start.line)
                    .then(b.range.start.character.cmp(&a.range.start.character))
            });

            changes.insert(uri.clone(), edits);
        }

        if changes.is_empty() {
            return None;
        }

        let total_edits: usize = changes.values().map(|v| v.len()).sum();
        debug!(
            "Rename will update {} locations across {} files",
            total_edits,
            changes.len()
        );

        Some(WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        })
    }

    /// Check if an identifier is a component name (context or machine)
    fn is_component_name(&self, identifier: &str) -> bool {
        if let Some(ref manager) = self.cross_ref_manager {
            manager.find_component_uri(identifier).is_some()
        } else {
            false
        }
    }

    /// Rename a component across all workspace files
    fn rename_across_workspace(
        &self,
        old_name: &str,
        new_name: &str,
        changes: &mut HashMap<Url, Vec<TextEdit>>,
    ) {
        let manager = match &self.cross_ref_manager {
            Some(m) => m,
            None => return,
        };

        // Get all component URIs in the workspace
        let component_uris = manager.all_component_uris();

        for uri_str in component_uris {
            // Try to get the document content
            let text = if let Some(doc_mgr) = &self.document_manager {
                // First try to get from open documents
                if let Ok(url) = Url::parse(&uri_str) {
                    doc_mgr.get_text(&url)
                } else {
                    None
                }
            } else {
                None
            };

            // If not in open documents, read from file
            let text = text.or_else(|| {
                if let Ok(url) = Url::parse(&uri_str) {
                    if let Ok(path) = url.to_file_path() {
                        std::fs::read_to_string(path).ok()
                    } else {
                        None
                    }
                } else {
                    None
                }
            });

            if let Some(text) = text
                && let Ok(url) = Url::parse(&uri_str)
                && let Some(locations) = find_all_references(&text, old_name, &url)
            {
                let mut edits: Vec<TextEdit> = locations
                    .into_iter()
                    .map(|loc| TextEdit {
                        range: loc.range,
                        new_text: new_name.to_string(),
                    })
                    .collect();

                // Sort edits in reverse order
                edits.sort_by(|a, b| {
                    b.range
                        .start
                        .line
                        .cmp(&a.range.start.line)
                        .then(b.range.start.character.cmp(&a.range.start.character))
                });

                changes.insert(url, edits);
            }
        }
    }
}

/// Get the identifier and its range at the given position
fn get_identifier_and_range_at_position(text: &str, position: Position) -> Option<(String, Range)> {
    let lines: Vec<&str> = text.lines().collect();
    let line_idx = position.line as usize;
    let col_idx = position.character as usize;

    if line_idx >= lines.len() {
        return None;
    }

    let line = lines[line_idx];
    let chars: Vec<char> = line.chars().collect();

    if col_idx >= chars.len() {
        return None;
    }

    // Find the start of the identifier
    let mut start = col_idx;
    while start > 0 && is_identifier_char(chars[start - 1]) {
        start -= 1;
    }

    // Find the end of the identifier
    let mut end = col_idx;
    while end < chars.len() && is_identifier_char(chars[end]) {
        end += 1;
    }

    if start < end {
        let identifier: String = chars[start..end].iter().collect();
        let range = Range::new(
            Position::new(line_idx as u32, start as u32),
            Position::new(line_idx as u32, end as u32),
        );
        Some((identifier, range))
    } else {
        None
    }
}

/// Check if a character can be part of an identifier
fn is_identifier_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Check if a string is a valid Event-B identifier
fn is_valid_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }

    // Must start with a letter or underscore
    let chars: Vec<char> = s.chars().collect();
    if !chars[0].is_alphabetic() && chars[0] != '_' {
        return false;
    }

    // Rest must be alphanumeric or underscore
    chars.iter().all(|&c| is_identifier_char(c))
}

/// Check if a string is an Event-B keyword (case-insensitive)
fn is_keyword(s: &str) -> bool {
    matches!(
        s.to_lowercase().as_str(),
        // Structural keywords
        "context"
            | "machine"
            | "end"
            | "extends"
            | "sets"
            | "constants"
            | "axioms"
            | "theorem"
            | "refines"
            | "sees"
            | "variables"
            | "invariants"
            | "variant"
            | "events"
            | "event"
            | "initialisation"
            | "any"
            | "where"
            | "when"
            | "with"
            | "witness"
            | "then"
            | "begin"
            // Event status keywords
            | "ordinary"
            | "convergent"
            | "anticipated"
            // Built-in types and constants
            | "true"
            | "false"
            | "bool"
            | "nat"
            | "nat1"
            | "int"
            // Logic operators
            | "not"
            | "or"
            // Function/set operators
            | "dom"
            | "ran"
            | "pow"
            | "pow1"
            | "mod"
            | "finite"
            | "partition"
            | "card"
            | "min"
            | "max"
            | "id"
            | "prj1"
            | "prj2"
            | "closure"
            | "closure1"
            // Quantified operators
            | "union"
            | "inter"
            // Other
            | "oftype"
            | "skip"
    )
}

/// Find all references to an identifier in the text, skipping comments
fn find_all_references(text: &str, identifier: &str, uri: &Url) -> Option<Vec<Location>> {
    let mut locations = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let id_chars: Vec<char> = identifier.chars().collect();
    let mut tracker = text_utils::CommentTracker::new();

    for (line_idx, line) in lines.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let code_spans = tracker.code_spans(&chars);

        for span in &code_spans {
            let mut col = span.start;
            while col + id_chars.len() <= span.end {
                let matches = chars[col..col + id_chars.len()] == id_chars;
                if matches {
                    let before_ok = col == 0 || !is_identifier_char(chars[col - 1]);
                    let after_ok = col + id_chars.len() >= chars.len()
                        || !is_identifier_char(chars[col + id_chars.len()]);

                    if before_ok && after_ok {
                        locations.push(Location::new(
                            uri.clone(),
                            Range::new(
                                Position::new(line_idx as u32, col as u32),
                                Position::new(line_idx as u32, (col + id_chars.len()) as u32),
                            ),
                        ));
                    }
                }
                col += 1;
            }
        }
    }

    if locations.is_empty() {
        None
    } else {
        Some(locations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_uri() -> Url {
        Url::parse("file:///test.eventb").unwrap()
    }

    fn make_position_params(line: u32, character: u32, uri: Url) -> TextDocumentPositionParams {
        TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri },
            position: Position::new(line, character),
        }
    }

    fn make_rename_params(line: u32, character: u32, uri: Url, new_name: String) -> RenameParams {
        RenameParams {
            text_document_position: make_position_params(line, character, uri),
            new_name,
            work_done_progress_params: WorkDoneProgressParams::default(),
        }
    }

    #[test]
    fn test_rename_provider_creation() {
        let _provider = RenameProvider::new();
    }

    #[test]
    fn test_is_valid_identifier() {
        assert!(is_valid_identifier("count"));
        assert!(is_valid_identifier("_count"));
        assert!(is_valid_identifier("count_1"));
        assert!(is_valid_identifier("MAX_VALUE"));

        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("1count")); // starts with digit
        assert!(!is_valid_identifier("count-1")); // contains hyphen
        assert!(!is_valid_identifier("count.var")); // contains dot
    }

    #[test]
    fn test_is_keyword() {
        assert!(is_keyword("CONTEXT"));
        assert!(is_keyword("MACHINE"));
        assert!(is_keyword("VARIABLES"));
        assert!(is_keyword("END"));

        assert!(!is_keyword("count"));
        assert!(!is_keyword("my_variable"));
    }

    #[test]
    fn test_is_keyword_case_insensitive() {
        assert!(is_keyword("context"));
        assert!(is_keyword("Context"));
        assert!(is_keyword("CONTEXT"));
        assert!(is_keyword("machine"));
        assert!(is_keyword("Machine"));
        assert!(is_keyword("MACHINE"));
        assert!(is_keyword("Variables"));
        assert!(is_keyword("End"));
    }

    #[test]
    fn test_is_keyword_builtins() {
        // Built-in types
        assert!(is_keyword("true"));
        assert!(is_keyword("TRUE"));
        assert!(is_keyword("false"));
        assert!(is_keyword("FALSE"));
        assert!(is_keyword("BOOL"));
        assert!(is_keyword("NAT"));
        assert!(is_keyword("NAT1"));
        assert!(is_keyword("INT"));

        // Function operators
        assert!(is_keyword("dom"));
        assert!(is_keyword("DOM"));
        assert!(is_keyword("ran"));
        assert!(is_keyword("pow"));
        assert!(is_keyword("POW"));
        assert!(is_keyword("POW1"));
        assert!(is_keyword("mod"));

        // Built-in functions
        assert!(is_keyword("finite"));
        assert!(is_keyword("FINITE"));
        assert!(is_keyword("partition"));
        assert!(is_keyword("card"));
        assert!(is_keyword("min"));
        assert!(is_keyword("max"));
        assert!(is_keyword("id"));
        assert!(is_keyword("prj1"));
        assert!(is_keyword("prj2"));
        assert!(is_keyword("closure"));
        assert!(is_keyword("closure1"));

        // Quantified
        assert!(is_keyword("UNION"));
        assert!(is_keyword("INTER"));
        assert!(is_keyword("union"));
        assert!(is_keyword("inter"));
    }

    #[test]
    fn test_prepare_rename_valid() {
        let provider = RenameProvider::new();
        let uri = make_uri();

        let source = r#"
MACHINE test
VARIABLES
    count
INVARIANTS
    @inv1 count ∈ ℕ
END
"#;

        // Prepare rename on 'count' variable
        let params = make_position_params(3, 4, uri);
        let range = provider.prepare_rename(&params, source);

        assert!(range.is_some());
        let range = range.unwrap();
        assert_eq!(range.start.line, 3);
        assert_eq!(range.start.character, 4);
        assert_eq!(range.end.character, 9); // "count" is 5 characters
    }

    #[test]
    fn test_prepare_rename_keyword() {
        let provider = RenameProvider::new();
        let uri = make_uri();

        let source = r#"
MACHINE test
VARIABLES
    count
END
"#;

        // Try to rename 'VARIABLES' keyword - should fail
        let params = make_position_params(2, 0, uri);
        let range = provider.prepare_rename(&params, source);

        assert!(range.is_none());
    }

    #[test]
    fn test_rename_variable() {
        let provider = RenameProvider::new();
        let uri = make_uri();

        let source = r#"
MACHINE counter
VARIABLES
    count
INVARIANTS
    @inv1 count ∈ ℕ
EVENTS
    EVENT INITIALISATION
    THEN
        count := 0
    END

    EVENT increment
    WHEN
        count < 10
    THEN
        count := count + 1
    END
END
"#;

        // Rename 'count' to 'counter_value'
        let params = make_rename_params(3, 4, uri.clone(), "counter_value".to_string());
        let edit = provider.rename(&params, source);

        assert!(edit.is_some());
        let edit = edit.unwrap();

        let changes = edit.changes.unwrap();
        let text_edits = changes.get(&uri).unwrap();

        // Should have multiple edits (declaration + all references)
        assert!(text_edits.len() >= 5);

        // All edits should replace 'count' with 'counter_value'
        for text_edit in text_edits {
            assert_eq!(text_edit.new_text, "counter_value");
        }
    }

    #[test]
    fn test_rename_constant() {
        let provider = RenameProvider::new();
        let uri = make_uri();

        let source = r#"
CONTEXT ctx
CONSTANTS
    max_val
AXIOMS
    @axm1 max_val = 100
    @axm2 max_val > 0
END
"#;

        // Rename 'max_val' to 'MAX_VALUE'
        let params = make_rename_params(3, 4, uri.clone(), "MAX_VALUE".to_string());
        let edit = provider.rename(&params, source);

        assert!(edit.is_some());
        let edit = edit.unwrap();

        let changes = edit.changes.unwrap();
        let text_edits = changes.get(&uri).unwrap();

        // Should have 3 edits (declaration + 2 axiom references)
        assert_eq!(text_edits.len(), 3);
    }

    #[test]
    fn test_rename_to_keyword_fails() {
        let provider = RenameProvider::new();
        let uri = make_uri();

        let source = r#"
MACHINE test
VARIABLES
    count
END
"#;

        // Try to rename 'count' to 'VARIABLES' - should fail
        let params = make_rename_params(3, 4, uri, "VARIABLES".to_string());
        let edit = provider.rename(&params, source);

        assert!(edit.is_none());
    }

    #[test]
    fn test_rename_to_invalid_name_fails() {
        let provider = RenameProvider::new();
        let uri = make_uri();

        let source = r#"
MACHINE test
VARIABLES
    count
END
"#;

        // Try to rename to invalid identifier
        let params = make_rename_params(3, 4, uri.clone(), "123invalid".to_string());
        let edit = provider.rename(&params, source);
        assert!(edit.is_none());

        // Try to rename to identifier with invalid characters
        let params = make_rename_params(3, 4, uri, "count-value".to_string());
        let edit = provider.rename(&params, source);
        assert!(edit.is_none());
    }

    #[test]
    fn test_rename_preserves_other_identifiers() {
        let provider = RenameProvider::new();
        let uri = make_uri();

        let source = r#"
MACHINE test
VARIABLES
    count
    counter
END
"#;

        // Rename 'count' to 'value'
        let params = make_rename_params(3, 4, uri.clone(), "value".to_string());
        let edit = provider.rename(&params, source);

        assert!(edit.is_some());
        let edit = edit.unwrap();

        let changes = edit.changes.unwrap();
        let text_edits = changes.get(&uri).unwrap();

        // Should only rename 'count', not 'counter'
        assert_eq!(text_edits.len(), 1);
    }

    #[test]
    fn test_get_identifier_and_range_at_position() {
        let text = "VARIABLES count";
        let position = Position::new(0, 10); // On 'count'

        let result = get_identifier_and_range_at_position(text, position);
        assert!(result.is_some());

        let (identifier, range) = result.unwrap();
        assert_eq!(identifier, "count");
        assert_eq!(range.start.character, 10);
        assert_eq!(range.end.character, 15);
    }

    #[test]
    fn test_rename_edits_sorted() {
        let provider = RenameProvider::new();
        let uri = make_uri();

        let source = r#"
MACHINE test
VARIABLES
    x
INVARIANTS
    @inv1 x = 0
    @inv2 x > 0
END
"#;

        let params = make_rename_params(3, 4, uri.clone(), "y".to_string());
        let edit = provider.rename(&params, source);

        assert!(edit.is_some());
        let edit = edit.unwrap();

        let changes = edit.changes.unwrap();
        let text_edits = changes.get(&uri).unwrap();

        // Edits should be sorted in reverse order (bottom to top)
        for i in 1..text_edits.len() {
            let prev = &text_edits[i - 1];
            let curr = &text_edits[i];

            // Previous edit should be on same or later line
            assert!(prev.range.start.line >= curr.range.start.line);

            // If on same line, previous should be at same or later column
            if prev.range.start.line == curr.range.start.line {
                assert!(prev.range.start.character >= curr.range.start.character);
            }
        }
    }

    #[test]
    fn test_rename_skips_comments() {
        let provider = RenameProvider::new();
        let uri = make_uri();

        let source = "count := 0 // count reset\ncount := count + 1";

        // Rename 'count' to 'val'
        let params = make_rename_params(0, 0, uri.clone(), "val".to_string());
        let edit = provider.rename(&params, source);

        assert!(edit.is_some());
        let edit = edit.unwrap();
        let changes = edit.changes.unwrap();
        let text_edits = changes.get(&uri).unwrap();

        // Should have 3 edits: line 0 col 0, line 1 col 0, line 1 col 9
        // Should NOT include the 'count' inside the comment
        assert_eq!(text_edits.len(), 3);
    }

    #[test]
    fn test_rename_to_builtin_keyword_fails() {
        let provider = RenameProvider::new();
        let uri = make_uri();

        let source = r#"
MACHINE test
VARIABLES
    count
END
"#;

        // Renaming to built-in keywords should fail
        let params = make_rename_params(3, 4, uri.clone(), "dom".to_string());
        assert!(provider.rename(&params, source).is_none());

        let params = make_rename_params(3, 4, uri.clone(), "POW".to_string());
        assert!(provider.rename(&params, source).is_none());

        let params = make_rename_params(3, 4, uri.clone(), "finite".to_string());
        assert!(provider.rename(&params, source).is_none());

        let params = make_rename_params(3, 4, uri, "TRUE".to_string());
        assert!(provider.rename(&params, source).is_none());
    }
}
