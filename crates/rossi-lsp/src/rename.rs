//! Symbol rename functionality
//!
//! This module provides the ability to rename Event-B symbols (variables, constants,
//! sets, events) safely by updating all references throughout the document and
//! across the workspace.

use crate::lsp_types::*;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::debug;

use rossi::Component;
use rossi::ast::Span;

use crate::component_util::{component_at_offset, parse_all};
use crate::cross_references::CrossReferenceManager;
use crate::document::DocumentManager;
use crate::formula_walk;
use crate::identifier_utils;
use crate::identifier_utils::position_to_offset;
use crate::position::span_to_range;

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

        // Prefer the open document's stored parse: the served text, the cursor
        // offset, and the AST then index one consistent snapshot, and the cursor
        // file is not re-parsed for the rename. Fall back to the handler text
        // when the document is not open (closed docs, unit tests).
        let cursor = self
            .document_manager
            .as_ref()
            .and_then(|dm| dm.parse_result(uri));
        let text = cursor
            .as_deref()
            .map_or(text, |parsed| parsed.text.as_str());

        // Get the identifier at the cursor position
        let (identifier, _) = get_identifier_and_range_at_position(text, position)?;

        debug!(
            "Renaming identifier '{}' to '{}' at {:?}",
            identifier, new_name, position
        );

        // Check if this is a component name that should be renamed across files
        let is_component = self.is_component_name(&identifier);

        // A structural name may be hyphenated (Rodin labels/file names);
        // mathematical symbols may not (kernel_lang §2.2). Beyond tracked
        // components, an old name that is itself hyphenated can only be a
        // structural name (e.g. an event named `do-step`, which the cross-ref
        // manager does not track), so allow the new name to be hyphenated too.
        let allow_component_name =
            is_component || !rossi::names::is_valid_math_identifier(&identifier);
        if !is_valid_new_name(new_name, allow_component_name) {
            debug!("Invalid new name: '{}'", new_name);
            return None;
        }

        // Check if new name is a keyword
        if is_keyword(new_name) {
            debug!("Cannot rename to keyword: '{}'", new_name);
            return None;
        }

        let mut changes = HashMap::new();

        if is_component {
            // Rename across all workspace files
            debug!("Renaming component '{}' across workspace", identifier);
            self.rename_across_workspace(&identifier, new_name, &mut changes);
        } else {
            // Rename only in the current document. A hyphenated symbol (an
            // event name) gets the component boundary; a math symbol the math one.
            debug!("Renaming symbol '{}' in current document", identifier);
            // Resolve the rename from the AST: a binder of the same name keeps
            // its own scope, and the after-state form `x'` is renamed at its
            // base. Fall back to a whole-word scan when the document doesn't
            // parse far enough to resolve the cursor.
            //
            // Reuse the open document's components when we have them; otherwise
            // recover them from the served text. `text` and these components are
            // the same snapshot, so the offset and the spans agree.
            let owned;
            let components: &[Component] = match cursor.as_deref() {
                Some(parsed) => parsed.components(),
                None => {
                    owned = parse_all(text);
                    &owned
                }
            };
            let edits = ast_rename_edits(text, components, position, &identifier, new_name)
                .or_else(|| text_rename_edits(text, &identifier, uri, new_name))?;

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
                && let Some(locations) = find_all_references(
                    &text,
                    old_name,
                    &url,
                    // Component boundary: renaming component `ENV_C` must not
                    // rewrite the prefix of a sibling named `ENV_C-1`.
                    identifier_utils::WordBoundary::ComponentName,
                )
            {
                let mut edits: Vec<TextEdit> = locations
                    .into_iter()
                    .map(|loc| TextEdit {
                        range: loc.range,
                        new_text: new_name.to_string(),
                    })
                    .collect();

                sort_edits_reverse(&mut edits);

                changes.insert(url, edits);
            }
        }
    }
}

/// Get the identifier and its range at the given position
fn get_identifier_and_range_at_position(text: &str, position: Position) -> Option<(String, Range)> {
    identifier_utils::identifier_at_position(text, position)
}

/// Check if a string can be the new name of a renamed symbol. Components
/// (machines/contexts/events) accept hyphenated names, mathematical symbols
/// do not — both per `rossi::names`, the same source of truth the parser and
/// importer use, so a rename can never produce unparseable text.
fn is_valid_new_name(s: &str, is_component: bool) -> bool {
    if is_component {
        rossi::names::is_valid_component_name(s)
    } else {
        rossi::names::is_valid_math_identifier(s)
    }
}

/// Check if a string is reserved vocabulary that cannot name an identifier:
/// structural keywords (case-insensitive, like their grammar tokens) plus the
/// mathematical-language words under each word's own case rule
/// ([`rossi::builtins::is_reserved_name`]) — `dom`/`card`/`POW`/`TRUE` are
/// blocked, while `Dom`, `Card`, `pow` are ordinary identifiers the parser
/// accepts and rename must not refuse.
fn is_keyword(s: &str) -> bool {
    rossi::keywords::is_keyword(s) || rossi::builtins::is_reserved_name(s)
}

/// Find all references to an identifier in the text, skipping comments.
///
/// Returns `None` when there are no matches.
fn find_all_references(
    text: &str,
    identifier: &str,
    uri: &Url,
    boundary: identifier_utils::WordBoundary,
) -> Option<Vec<Location>> {
    let locations =
        identifier_utils::find_whole_word_locations(text, identifier, uri, None, boundary);
    if locations.is_empty() {
        None
    } else {
        Some(locations)
    }
}

/// Fallback rename edits via a whole-word text scan, for documents the parser
/// cannot resolve a symbol in.
fn text_rename_edits(
    text: &str,
    identifier: &str,
    uri: &Url,
    new_name: &str,
) -> Option<Vec<TextEdit>> {
    let locations = find_all_references(
        text,
        identifier,
        uri,
        identifier_utils::WordBoundary::for_name(identifier),
    )?;
    let mut edits: Vec<TextEdit> = locations
        .into_iter()
        .map(|loc| TextEdit {
            range: loc.range,
            new_text: new_name.to_string(),
        })
        .collect();
    sort_edits_reverse(&mut edits);
    Some(edits)
}

/// AST-driven rename of the symbol at `position` within the current document.
///
/// The cursor occurrence is resolved through the shared walker: if it is (or is
/// bound by) a quantifier / lambda / comprehension binder or an event
/// parameter, only that binder's declaration and its in-scope uses are renamed;
/// otherwise the component-level symbol's declaration and free uses are renamed.
/// A binder of the same name in another scope, and same-named globals, are left
/// untouched. The after-state form `x'` is renamed at its base, preserving `'`.
///
/// `components` are the document's already-parsed components (the open
/// document's stored parse, or a recovery parse of `text` when it is not open),
/// so the rename never re-parses the cursor file. `text` must be the source
/// those component spans index into.
fn ast_rename_edits(
    text: &str,
    components: &[Component],
    position: Position,
    identifier: &str,
    new_name: &str,
) -> Option<Vec<TextEdit>> {
    let offset = position_to_offset(text, position)?;
    let component = component_at_offset(components, offset)?;

    let spans = rename_spans(component, identifier, offset);
    if spans.is_empty() {
        return None;
    }

    // Every span must slice to the identifier (or its `x'` form) in the served
    // text. If any does not — e.g. a span left relative by a deeper recovery
    // bug — abandon the AST rename rather than corrupt unrelated source or panic
    // on a non-char-boundary slice; the caller falls back to the text scan.
    if !spans
        .iter()
        .all(|s| formula_walk::span_matches(text, *s, identifier))
    {
        return None;
    }

    let mut edits: Vec<TextEdit> = spans
        .into_iter()
        .map(|span| TextEdit {
            range: span_to_range(&base_span(text, span), text),
            new_text: new_name.to_string(),
        })
        .collect();
    // A binder declaration and a use can coincide in degenerate inputs; dedup by
    // range after sorting so an edit is never applied twice.
    sort_edits_reverse(&mut edits);
    edits.dedup_by(|a, b| a.range == b.range);
    Some(edits)
}

/// The byte spans to rewrite when renaming the identifier at `offset`.
///
/// A cursor on (or bound by) a binder — a quantifier / lambda / comprehension
/// binder, or an event `ANY` parameter — renames only that binder's own scope,
/// resolved through the shared [`formula_walk::resolve_bound_at_offset`] that
/// go-to-definition and find-references also use, so the features cannot drift.
/// Otherwise the cursor names a component-level symbol and its declaration plus
/// every free use is renamed.
fn rename_spans(component: &Component, identifier: &str, offset: usize) -> Vec<Span> {
    // One walk of the component serves both the cursor lookup and the global
    // occurrence set: collect the hits once, then reuse them for the free-use
    // set when the cursor is not on a binder.
    let hits = formula_walk::collect_in_component(component, identifier);
    if let Some(bound) = formula_walk::resolve_bound_from_hits(&hits, component, offset) {
        return bound.spans;
    }

    // Global symbol: its declaration plus every free use.
    let mut spans = formula_walk::free_spans(hits);
    if let Some(decl) = formula_walk::declaration_span(component, identifier) {
        spans.push(decl);
    }
    spans
}

/// Trim a trailing apostrophe so renaming `x'` rewrites only the base `x`.
fn base_span(text: &str, span: Span) -> Span {
    if text[span.start..span.end].ends_with('\'') {
        Span {
            start: span.start,
            end: span.end - 1,
        }
    } else {
        span
    }
}

/// Sort edits bottom-to-top, right-to-left so applying them never shifts a
/// not-yet-applied edit's offsets.
fn sort_edits_reverse(edits: &mut [TextEdit]) {
    edits.sort_by(|a, b| {
        b.range
            .start
            .line
            .cmp(&a.range.start.line)
            .then(b.range.start.character.cmp(&a.range.start.character))
    });
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
    fn test_is_valid_new_name() {
        for kind in [false, true] {
            assert!(is_valid_new_name("count", kind));
            assert!(is_valid_new_name("_count", kind));
            assert!(is_valid_new_name("count_1", kind));
            assert!(is_valid_new_name("MAX_VALUE", kind));

            assert!(!is_valid_new_name("", kind));
            assert!(!is_valid_new_name("1count", kind)); // starts with digit
            assert!(!is_valid_new_name("count.var", kind)); // contains dot
            assert!(!is_valid_new_name("count-", kind)); // trailing hyphen
        }

        // Hyphenated names are valid only for components (machines,
        // contexts, events) — Rodin labels/file names, not math identifiers.
        assert!(is_valid_new_name("count-1", true));
        assert!(!is_valid_new_name("count-1", false));
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

        // Function operators (exact-case tokens — see the exact-case test
        // below for the case variants that stay renameable).
        assert!(is_keyword("dom"));
        assert!(is_keyword("ran"));
        assert!(is_keyword("POW"));
        assert!(is_keyword("POW1"));
        assert!(is_keyword("mod"));

        // Built-in functions
        assert!(is_keyword("finite"));
        assert!(is_keyword("partition"));
        assert!(is_keyword("card"));
        assert!(is_keyword("min"));
        assert!(is_keyword("max"));
        assert!(is_keyword("id"));
        assert!(is_keyword("prj1"));
        assert!(is_keyword("prj2"));

        // Quantified (case-insensitive tokens — any spelling lexes as one)
        assert!(is_keyword("UNION"));
        assert!(is_keyword("INTER"));
        assert!(is_keyword("union"));
        assert!(is_keyword("inter"));
    }

    #[test]
    fn test_is_keyword_math_words_are_exact_case() {
        // The parser reserves the math words exact-case (Rodin parity):
        // `Dom`, `Card`, `pow` parse as ordinary identifiers, so rename must
        // allow them — both as rename targets and as new names.
        for ok in [
            "Dom", "DOM", "Card", "FINITE", "Ran", "pow", "Pow", "OR", "Circ",
        ] {
            assert!(!is_keyword(ok), "{ok:?} is an ordinary identifier");
        }
        // The exact token spellings stay blocked, including the rossi-only
        // ASCII operator words that would shadow in operator position.
        for blocked in ["dom", "card", "or", "not", "circ", "oftype", "POW"] {
            assert!(is_keyword(blocked), "{blocked:?} must stay blocked");
        }
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
    fn test_rename_hyphenated_event_to_hyphenated_name() {
        // A hyphenated old name can only be a structural name (here an event,
        // which the cross-ref manager does not track), so a hyphenated new
        // name must be allowed (issue #28).
        let provider = RenameProvider::new();
        let uri = make_uri();
        let source = "\
MACHINE m1
EVENTS
EVENT do-step
THEN
    @act1 skip
END
END
";
        let params = make_rename_params(2, 6, uri.clone(), "do-step2".to_string());
        let edit = provider.rename(&params, source);
        assert!(edit.is_some(), "hyphenated event rename should succeed");
        let edits = edit.unwrap().changes.unwrap();
        assert!(edits.get(&uri).is_some_and(|e| !e.is_empty()));
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

    // ---- scope-aware rename (AST-driven) -----------------------------------

    fn pos_at(text: &str, byte: usize) -> Position {
        crate::position::offset_to_position(text, byte)
    }

    fn apply(text: &str, edits: &[TextEdit]) -> String {
        // The provider returns edits sorted bottom-to-top, right-to-left, so
        // applying them in order never invalidates a later edit's offsets.
        let mut out = text.to_string();
        for e in edits {
            let start = position_to_offset(&out, e.range.start).unwrap();
            let end = position_to_offset(&out, e.range.end).unwrap();
            out.replace_range(start..end, &e.new_text);
        }
        out
    }

    fn rename_at(source: &str, byte: usize, new_name: &str) -> String {
        let provider = RenameProvider::new();
        let uri = make_uri();
        let pos = pos_at(source, byte);
        let params = make_rename_params(pos.line, pos.character, uri.clone(), new_name.to_string());
        let edit = provider
            .rename(&params, source)
            .expect("rename produces edits");
        let edits = edit.changes.unwrap().remove(&uri).unwrap();
        apply(source, &edits)
    }

    #[test]
    fn rename_global_skips_shadowing_binder() {
        let source = "MACHINE m\nVARIABLES\nx\nINVARIANTS\n@i1 x ∈ ℕ\n@i2 ∀ x · x ∈ ℕ\nEND\n";
        // Cursor on the free use in @i1.
        let byte = source.find("@i1 x").unwrap() + "@i1 ".len();
        let out = rename_at(source, byte, "y");
        // The quantifier and its bound body keep `x`; the free use becomes `y`.
        assert!(out.contains("@i1 y ∈ ℕ"), "{out}");
        assert!(out.contains("∀ x · x ∈ ℕ"), "bound x untouched: {out}");
    }

    #[test]
    fn rename_bound_local_keeps_global() {
        let source = "MACHINE m\nVARIABLES\nx\nINVARIANTS\n@i1 x ∈ ℕ\n@i2 ∀ x · x ∈ ℕ\nEND\n";
        // Cursor on the quantifier binder `x`.
        let byte = source.find("∀ x").unwrap() + "∀ ".len();
        let out = rename_at(source, byte, "y");
        // Only the binder and its bound body use are renamed.
        assert!(out.contains("∀ y · y ∈ ℕ"), "{out}");
        assert!(out.contains("@i1 x ∈ ℕ"), "global x untouched: {out}");
    }

    #[test]
    fn rename_lambda_leaf_keeps_sibling() {
        let source =
            "CONTEXT c\nCONSTANTS\nf\nAXIOMS\n@a1 f = (λ x ↦ y · x ∈ ℕ ∧ y ∈ ℕ ∣ x)\nEND\n";
        // Cursor on the lambda binder `x`.
        let byte = source.find("λ x").unwrap() + "λ ".len();
        let out = rename_at(source, byte, "z");
        assert!(out.contains("λ z ↦ y · z ∈ ℕ ∧ y ∈ ℕ ∣ z"), "{out}");
    }

    #[test]
    fn rename_in_later_component_after_broken_one_is_safe() {
        // A broken first component forces multi-component recovery. Renaming in
        // the healthy later component must not panic (a stale slice-relative
        // span would slice into the multibyte ∈) and must rewrite the right
        // text — inner formula spans are absolute after recovery.
        let source = "CONTEXT C0\nAXIOMS\n@a xxxxx ∈\nEND\n\nMACHINE M0\nVARIABLES\ncount\nINVARIANTS\n@i1 count > 0\nEND\n";
        let byte = source.rfind("count").unwrap(); // the use in @i1
        let out = rename_at(source, byte, "total");
        assert!(out.contains("@i1 total > 0"), "{out}");
        assert!(
            out.contains("VARIABLES\ntotal"),
            "declaration renamed: {out}"
        );
    }

    #[test]
    fn rename_outer_binder_leaves_shadowing_inner_untouched() {
        // `∀ x · (∃ x · x > 0)`: the inner `∃ x` shadows the outer `x`, which has
        // no body uses. Renaming the outer binder must touch only its own
        // declaration — the inner quantifier and its body stay `x`.
        let source = "MACHINE m\nINVARIANTS\n@i1 ∀ x · (∃ x · x > 0)\nEND\n";
        let byte = source.find("∀ x").unwrap() + "∀ ".len();
        let out = rename_at(source, byte, "y");
        assert!(out.contains("∀ y · (∃ x · x > 0)"), "{out}");
    }

    #[test]
    fn rename_primed_after_state_preserves_prime() {
        let source =
            "MACHINE m\nVARIABLES\nx\nEVENTS\nEVENT e\nTHEN\n@a1 x :∣ x' = x + 1\nEND\nEND\n";
        // Cursor on the write target.
        let byte = source.find("@a1 x").unwrap() + "@a1 ".len();
        let out = rename_at(source, byte, "y");
        // The base of `x'` is renamed; the prime is preserved.
        assert!(out.contains("y :∣ y' = y + 1"), "{out}");
    }

    #[test]
    fn rename_through_open_document_uses_stored_parse() {
        // With the document open, the rename resolves against the document
        // manager's stored parse rather than re-parsing the handler text. The
        // handler text below is deliberately stale and too short to contain the
        // cursor position, so the rename can only succeed — and land on `count`
        // — if it reads the stored snapshot instead.
        let uri = make_uri();
        let stored = "MACHINE m\nVARIABLES\ncount\nINVARIANTS\n@i1 count > 0\nEND\n";

        let documents = Arc::new(DocumentManager::new());
        documents.open(uri.clone(), "eventb".to_string(), 1, stored.to_string());

        let mut provider = RenameProvider::new();
        provider.set_document_manager(Arc::clone(&documents));

        // Cursor on the use of `count` in @i1 (offset into the stored text).
        let pos = pos_at(stored, stored.rfind("count").unwrap());
        let params = make_rename_params(pos.line, pos.character, uri.clone(), "total".to_string());

        let edit = provider
            .rename(&params, "MACHINE m\nVARIABLES\nother\nEND\n")
            .expect("rename resolves from the stored parse");
        let edits = edit.changes.unwrap().remove(&uri).unwrap();
        let out = apply(stored, &edits);

        assert!(out.contains("@i1 total > 0"), "{out}");
        assert!(
            out.contains("VARIABLES\ntotal"),
            "declaration renamed: {out}"
        );
    }
}
