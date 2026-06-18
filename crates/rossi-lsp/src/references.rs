//! Find all references to symbols functionality
//!
//! This module provides the ability to find all references to Event-B symbols
//! (variables, constants, sets, events, parameters) throughout the document
//! and across the workspace.

use crate::lsp_types::*;
use rossi::Component;
use rossi::ast::Span;
use rossi::keywords::KeywordId;

use crate::component_loader::ComponentLoader;
use crate::formula_walk;
use crate::position::span_to_range;
use std::collections::HashSet;
use std::sync::Arc;
use tracing::debug;

use crate::cross_references::{CrossReferenceManager, ReferenceKind};
use crate::document::{DocumentManager, ParsedDocument};
use crate::identifier_utils;
use crate::symbols::{
    SymbolIdentity, SymbolKind, candidate_components_for_symbol, resolve_symbol_identity,
    resolve_symbol_identity_in_component,
};
use crate::text_utils;

/// Provider for finding all references to symbols
pub struct ReferenceProvider {
    /// Cross-reference manager for workspace-wide navigation
    cross_ref_manager: Option<Arc<CrossReferenceManager>>,
    /// Document manager to access open documents
    document_manager: Option<Arc<DocumentManager>>,
}

impl Default for ReferenceProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ReferenceProvider {
    /// Create a new reference provider
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

    /// Find all references to the symbol at the given position
    pub fn find_references(&self, params: &ReferenceParams, text: &str) -> Option<Vec<Location>> {
        let position = params.text_document_position.position;
        let uri = &params.text_document_position.text_document.uri;

        // Prefer the document's own parsed snapshot when it is open: masking,
        // offsets, and the AST then all index one consistent string. The
        // handler's `text` is a separate copy a concurrent edit can desync from
        // the stored parse; reusing the snapshot also avoids re-parsing the
        // cursor document. Fall back to the handler text when the document is
        // not open (or there is no document manager, as in the unit tests).
        let cursor = self
            .document_manager
            .as_ref()
            .and_then(|dm| dm.parse_result(uri));
        let text = cursor
            .as_deref()
            .map_or(text, |parsed| parsed.text.as_str());

        // Mask comments once for the whole request: every line/identifier scan
        // on the cursor document reads this instead of re-masking. The mask
        // preserves byte/line/char layout, so positions and offsets computed on
        // it stay valid against `text`.
        let masked = rossi::comments::mask_comments_chars(text);

        // Get the identifier at the cursor position
        let identifier = identifier_at_masked_position(&masked, position)?;

        debug!(
            "Finding references for identifier '{}' at {:?}",
            identifier, position
        );

        let locations = self.find_references_for_identifier(
            text,
            &masked,
            uri,
            position,
            &identifier,
            cursor.as_deref(),
        );
        debug!("Found {} references to '{}'", locations.len(), identifier);
        non_empty(locations)
    }

    /// Find all references in the current document
    fn find_references_in_text(&self, text: &str, uri: &Url, identifier: &str) -> Vec<Location> {
        self.find_references_in_text_range(text, uri, identifier, None)
    }

    fn find_references_in_text_range(
        &self,
        text: &str,
        uri: &Url,
        identifier: &str,
        line_range: Option<(usize, usize)>,
    ) -> Vec<Location> {
        identifier_utils::find_whole_word_locations(
            text,
            identifier,
            uri,
            line_range,
            // A hyphenated needle is necessarily a component name, so it gets
            // the component boundary; a hyphen-free symbol keeps the math one.
            identifier_utils::WordBoundary::for_name(identifier),
        )
    }

    fn find_symbol_references_in_text_range(
        &self,
        text: &str,
        uri: &Url,
        identifier: &str,
        line_range: Option<(usize, usize)>,
    ) -> Vec<Location> {
        // Mask once for the whole filter rather than re-masking per location.
        let masked = rossi::comments::mask_comments_chars(text);
        self.find_references_in_text_range(text, uri, identifier, line_range)
            .into_iter()
            .filter(|location| !is_component_reference_position(&masked, location.range.start))
            .collect()
    }

    fn find_references_for_identifier(
        &self,
        text: &str,
        masked: &str,
        uri: &Url,
        position: Position,
        identifier: &str,
        cursor: Option<&ParsedDocument>,
    ) -> Vec<Location> {
        let Some(manager) = &self.cross_ref_manager else {
            return self.find_references_in_text(text, uri, identifier);
        };
        let is_component_name = manager.find_component_uri(identifier).is_some();

        // One loader per request: every component the lookup touches is parsed
        // at most once, and open documents are read from the store, not
        // re-parsed.
        let loader = ComponentLoader::new(manager, self.document_manager.as_deref());

        if is_component_name && is_component_reference_position(masked, position) {
            return find_references_in_workspace(&loader, identifier);
        }

        if let Some(symbol) =
            resolve_symbol_identity(text, masked, position, identifier, &loader, cursor)
        {
            return self.find_references_for_symbol(&symbol, &loader);
        }

        if is_component_name {
            return find_references_in_workspace(&loader, identifier);
        }

        self.find_references_in_text(text, uri, identifier)
    }

    fn find_references_for_symbol(
        &self,
        symbol: &SymbolIdentity,
        loader: &ComponentLoader,
    ) -> Vec<Location> {
        let mut locations = Vec::new();
        let mut seen = HashSet::new();

        if symbol.kind == SymbolKind::Parameter {
            let Some(event_name) = symbol.event.as_deref() else {
                return locations;
            };

            let Some(loaded) = loader.load(&symbol.owner) else {
                return locations;
            };

            push_unique_locations(
                &mut locations,
                &mut seen,
                ast_parameter_references(
                    loaded.component(),
                    loaded.text(),
                    loaded.uri(),
                    event_name,
                    &symbol.name,
                ),
            );

            return locations;
        }

        for component_name in candidate_components_for_symbol(symbol, loader.manager()) {
            let Some(loaded) = loader.load(&component_name) else {
                continue;
            };

            if resolve_symbol_identity_in_component(loaded.component(), &symbol.name, loader)
                == Some(symbol.clone())
            {
                // Event names are not formula identifiers, so they stay on the
                // text scan; variables / constants / sets resolve from the AST.
                let refs = if symbol.kind == SymbolKind::Event {
                    self.find_symbol_references_in_text_range(
                        loaded.text(),
                        loaded.uri(),
                        &symbol.name,
                        None,
                    )
                } else {
                    ast_symbol_references(
                        loaded.component(),
                        loaded.text(),
                        loaded.uri(),
                        &symbol.name,
                    )
                };
                push_unique_locations(&mut locations, &mut seen, refs);
            }
        }

        locations
    }
}

/// Find all references across the workspace.
///
/// A free function (it needs only the loader): every workspace component is
/// loaded through the shared cache, so a name appearing in many files is parsed
/// at most once.
fn find_references_in_workspace(loader: &ComponentLoader, identifier: &str) -> Vec<Location> {
    let mut locations = Vec::new();
    let mut seen = HashSet::new();

    let mut component_names = loader.manager().all_component_names();
    component_names.sort();

    for component_name in component_names {
        if let Some(loaded) = loader.load(&component_name) {
            // Component references use the component word boundary so a name
            // like `ENV_C` does not match inside a sibling component `ENV_C-1`
            // (consistent with rename's cross-file path).
            push_unique_locations(
                &mut locations,
                &mut seen,
                identifier_utils::find_whole_word_locations(
                    loaded.text(),
                    identifier,
                    loaded.uri(),
                    None,
                    identifier_utils::WordBoundary::ComponentName,
                ),
            );
        }
    }

    locations
}

fn push_unique_locations(
    locations: &mut Vec<Location>,
    seen: &mut HashSet<(String, u32, u32, u32, u32)>,
    new_locations: Vec<Location>,
) {
    for location in new_locations {
        let key = (
            location.uri.to_string(),
            location.range.start.line,
            location.range.start.character,
            location.range.end.line,
            location.range.end.character,
        );
        if seen.insert(key) {
            locations.push(location);
        }
    }
}

/// References to a global symbol (variable / constant / set) in one component:
/// its declaration site plus every free formula occurrence (reads and write
/// targets), resolved from the AST so binder-shadowed names of the same spelling
/// are excluded and matches never land in comments or string literals.
fn ast_symbol_references(
    component: &Component,
    text: &str,
    uri: &Url,
    name: &str,
) -> Vec<Location> {
    let mut spans: Vec<Span> = Vec::new();
    if let Some(decl) = formula_walk::declaration_span(component, name) {
        spans.push(decl);
    }
    spans.extend(formula_walk::free_occurrence_spans(component, name));
    spans_to_locations(spans, text, uri, name)
}

/// References to an event parameter: its declaration plus every free occurrence
/// within that event's guards, witnesses, `with` predicates, and actions.
fn ast_parameter_references(
    component: &Component,
    text: &str,
    uri: &Url,
    event_name: &str,
    name: &str,
) -> Vec<Location> {
    let Component::Machine(machine) = component else {
        return Vec::new();
    };
    let Some(event) = machine.events.iter().find(|e| e.name == event_name) else {
        return Vec::new();
    };
    let mut spans: Vec<Span> = Vec::new();
    if let Some(s) = crate::symbols::event_parameter_span(event, name) {
        spans.push(s);
    }
    spans.extend(formula_walk::parameter_occurrence_spans(event, name));
    spans_to_locations(spans, text, uri, name)
}

/// Convert spans to locations, dropping any span that does not slice to `name`
/// (or its `x'` form) in `text` — a deeper recovery bug could leave a span
/// relative to its region, and a reference at the wrong position is worse than a
/// missing one.
fn spans_to_locations(spans: Vec<Span>, text: &str, uri: &Url, name: &str) -> Vec<Location> {
    spans
        .into_iter()
        .filter(|span| formula_walk::span_matches(text, *span, name))
        .map(|span| Location {
            uri: uri.clone(),
            range: span_to_range(&span, text),
        })
        .collect()
}

fn non_empty(locations: Vec<Location>) -> Option<Vec<Location>> {
    if locations.is_empty() {
        None
    } else {
        Some(locations)
    }
}

#[cfg(test)]
fn event_line_range(text: &str, event_name: &str) -> Option<(usize, usize)> {
    // Scan comment-masked lines: an `EVENT foo` or `END` inside a comment
    // must not open or close the event's line range. The terminator is matched
    // through the keyword table ([`text_utils::line_keyword_is`]), so a labelled
    // action whose label spells a keyword (`@end x := 0`) is not read as `END`.
    let masked = rossi::comments::mask_comments_chars(text);
    text_utils::event_line_range_in(&masked, event_name)
}

/// The SEES/REFINES/EXTENDS clause `position` sits in, if any, as the dependency
/// edge it introduces (`ReferenceKind::target_kind` then says whether the target
/// is a machine or a context). `masked` is the comment-masked document text, so
/// keywords spelled inside comments are already blanked out and cannot be
/// mistaken for clause boundaries.
pub(crate) fn component_reference_clause(
    masked: &str,
    position: Position,
) -> Option<ReferenceKind> {
    let mut current_clause = None;
    let mut in_event = false;
    let mut reached = false;

    for (idx, line) in masked.lines().enumerate() {
        if idx > position.line as usize {
            break;
        }
        reached |= idx == position.line as usize;

        if text_utils::event_name_from_line(line).is_some() {
            in_event = true;
            current_clause = None;
            continue;
        }

        // Each line's leading keyword is resolved through the keyword table
        // ([`text_utils::line_keyword_is`] / [`is_declaration_scan_boundary`]),
        // not a `@`-stripped first word: a labelled clause line such as
        // `@end x := 0` or `@sees y` must not be read as the keyword it spells.
        if text_utils::line_keyword_is(line, KeywordId::End) && in_event {
            in_event = false;
            current_clause = None;
        } else if text_utils::line_keyword_is(line, KeywordId::Sees) && !in_event {
            current_clause = Some(ReferenceKind::Sees);
        } else if text_utils::line_keyword_is(line, KeywordId::Extends) && !in_event {
            current_clause = Some(ReferenceKind::Extends);
        } else if text_utils::line_keyword_is(line, KeywordId::Refines) && !in_event {
            current_clause = Some(ReferenceKind::Refines);
        } else if text_utils::is_declaration_scan_boundary(line) {
            current_clause = None;
        }
    }

    // A position past the last line isn't in any clause.
    reached.then_some(current_clause).flatten()
}

/// Whether `position` sits in a SEES/REFINES/EXTENDS clause.
fn is_component_reference_position(masked: &str, position: Position) -> bool {
    component_reference_clause(masked, position).is_some()
}

/// Get the identifier at `position` in already comment-masked text. A cursor
/// inside a comment is not on an identifier (the comment is blanked in `masked`).
fn identifier_at_masked_position(masked: &str, position: Position) -> Option<String> {
    identifier_utils::identifier_at_position(masked, position).map(|(identifier, _)| identifier)
}

/// Get the identifier at the given position in the text.
/// Comment-masked: a cursor inside a comment is not on an identifier.
#[cfg(test)]
fn get_identifier_at_position(text: &str, position: Position) -> Option<String> {
    let masked = rossi::comments::mask_comments_chars(text);
    identifier_at_masked_position(&masked, position)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_uri() -> Url {
        Url::parse("file:///test.eventb").unwrap()
    }

    fn make_params(line: u32, character: u32, uri: Url) -> ReferenceParams {
        ReferenceParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position: Position::new(line, character),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: ReferenceContext {
                include_declaration: true,
            },
        }
    }

    #[test]
    fn test_reference_provider_creation() {
        let _provider = ReferenceProvider::new();
    }

    #[test]
    fn test_get_identifier_at_position() {
        let text = "count := count + 1";
        let identifier = get_identifier_at_position(text, Position::new(0, 0));
        assert_eq!(identifier, Some("count".to_string()));

        let identifier = get_identifier_at_position(text, Position::new(0, 9));
        assert_eq!(identifier, Some("count".to_string()));
    }

    #[test]
    fn test_find_variable_references() {
        let provider = ReferenceProvider::new();
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

        // Find references to 'count' (clicking on the declaration)
        let params = make_params(3, 4, uri.clone());
        let refs = provider.find_references(&params, source);

        assert!(refs.is_some());
        let refs = refs.unwrap();
        // Should find: declaration, invariant, initialisation action, guard, action (twice)
        assert!(refs.len() >= 5);

        // Verify all references contain 'count'
        for location in refs {
            assert_eq!(location.uri, uri);
        }
    }

    #[test]
    fn test_find_constant_references() {
        let provider = ReferenceProvider::new();
        let uri = make_uri();

        let source = r#"
CONTEXT ctx
CONSTANTS
    max_value
AXIOMS
    @axm1 max_value = 100
    @axm2 max_value > 0
END
"#;

        // Find references to 'max_value'
        let params = make_params(3, 4, uri.clone());
        let refs = provider.find_references(&params, source);

        assert!(refs.is_some());
        let refs = refs.unwrap();
        // Should find: declaration + 2 axiom references
        assert_eq!(refs.len(), 3);
    }

    #[test]
    fn test_find_set_references() {
        let provider = ReferenceProvider::new();
        let uri = make_uri();

        let source = r#"
CONTEXT ctx
SETS
    STATUS
CONSTANTS
    all_status
AXIOMS
    @axm1 all_status ⊆ STATUS
END
"#;

        // Find references to 'STATUS'
        let params = make_params(3, 4, uri.clone());
        let refs = provider.find_references(&params, source);

        assert!(refs.is_some());
        let refs = refs.unwrap();
        // Should find: declaration + axiom reference
        assert_eq!(refs.len(), 2);
    }

    #[test]
    fn test_no_references_found() {
        let provider = ReferenceProvider::new();
        let uri = make_uri();

        let source = r#"
MACHINE test
VARIABLES
    unused
END
"#;

        // Find references to 'unused' - should only find declaration
        let params = make_params(3, 4, uri);
        let refs = provider.find_references(&params, source);

        assert!(refs.is_some());
        let refs = refs.unwrap();
        // Should find at least the declaration
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn test_find_event_references() {
        let provider = ReferenceProvider::new();
        let uri = make_uri();

        let source = r#"
MACHINE machine1
EVENTS
    EVENT start
    THEN
        skip
    END
END

MACHINE machine2
REFINES machine1
EVENTS
    EVENT start
    REFINES start
    END
END
"#;

        // Find references to 'start'
        let params = make_params(3, 10, uri.clone());
        let refs = provider.find_references(&params, source);

        assert!(refs.is_some());
        let refs = refs.unwrap();
        // Should find: first EVENT declaration, second EVENT declaration, REFINES clause
        assert_eq!(refs.len(), 3);
    }

    #[test]
    fn test_position_outside_bounds() {
        let provider = ReferenceProvider::new();
        let uri = make_uri();

        let source = "MACHINE test\nEND";

        // Position beyond end of line
        let params = make_params(0, 100, uri.clone());
        let refs = provider.find_references(&params, source);
        assert!(refs.is_none());

        // Position on empty line
        let params = make_params(10, 0, uri);
        let refs = provider.find_references(&params, source);
        assert!(refs.is_none());
    }

    #[test]
    fn test_get_identifier_at_position_unicode() {
        // Line with Unicode operators before the identifier
        let text = "    @inv1 x ∈ ℕ";
        // 'x' is at char index 10
        let identifier = get_identifier_at_position(text, Position::new(0, 10));
        assert_eq!(identifier, Some("x".to_string()));
    }

    #[test]
    fn test_find_references_with_unicode_operators() {
        let provider = ReferenceProvider::new();
        let uri = make_uri();

        // "@inv1 count ∈ ℕ ∧ count ≥ 0"
        //  chars: i(0) n(1) v(2) 1(3) :(4) (5) c(6) o(7) u(8) n(9) t(10) (11) ∈(12) (13) ℕ(14) (15) ∧(16) (17) c(18) ...
        let source = "@inv1 count ∈ ℕ ∧ count ≥ 0";
        let refs = provider.find_references_in_text(source, &uri, "count");
        // Should find 'count' twice despite multi-byte Unicode operators
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].range.start.character, 6);
        assert_eq!(refs[1].range.start.character, 18);
    }

    #[test]
    fn test_references_skip_line_comments() {
        let provider = ReferenceProvider::new();
        let uri = make_uri();

        let source = "count := 0 // count is reset\ncount := count + 1";
        let refs = provider.find_references_in_text(source, &uri, "count");
        // Line 0: 'count' at col 0 (code), 'count' at col 14 is in comment (skipped)
        // Line 1: 'count' at col 0 and col 9
        assert_eq!(refs.len(), 3);
    }

    #[test]
    fn test_references_skip_block_comments() {
        let provider = ReferenceProvider::new();
        let uri = make_uri();

        let source = "count := 0 /* count */ + count";
        let refs = provider.find_references_in_text(source, &uri, "count");
        // 'count' at col 0 (code), 'count' at col 15 is in block comment (skipped),
        // 'count' at col 25 (code)
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].range.start.character, 0);
        assert_eq!(refs[1].range.start.character, 25);
    }

    #[test]
    fn test_event_line_range_ignores_comment_keywords() {
        // `// END here` and `/* EVENT ghost */` must not terminate or open
        // the event's line range.
        let source = "\
MACHINE m
EVENTS
    EVENT step // END here
    WHERE
        @grd1 x > 0 /* EVENT ghost */
    THEN
        @act1 x ≔ 1
    END
END";
        assert_eq!(event_line_range(source, "step"), Some((2, 7)));
        assert_eq!(event_line_range(source, "ghost"), None);
    }

    #[test]
    fn test_event_line_range_anchors_hyphenated_name() {
        // A hyphenated event name (issue #36) must be recognized as the region
        // anchor: `event_name_from_line` returns the whole whitespace-delimited
        // token, so the `end` segment before the hyphen is not a separate event.
        let source = "\
MACHINE m
EVENTS
    EVENT end-update
    THEN
        @act1 x ≔ 1
    END
END";
        assert_eq!(event_line_range(source, "end-update"), Some((2, 5)));
        assert_eq!(event_line_range(source, "end"), None);
    }

    #[test]
    fn test_event_line_range_ignores_labelled_end_action() {
        // An action labelled `end` (`@end x ≔ 0`) is not the `END` keyword: the
        // range must extend to the real `END` on line 5, not stop at the action
        // on line 4. `line_keyword_is` reads the whole `@end` token, which the
        // keyword table does not resolve to `END`.
        let source = "\
MACHINE m
EVENTS
    EVENT step
    THEN
        @end x ≔ 0
    END
END";
        assert_eq!(event_line_range(source, "step"), Some((2, 5)));
    }

    #[test]
    fn test_component_reference_clause_ignores_labelled_sees() {
        // `@sees x > 0` is an invariant labelled `sees`, not a SEES clause. The
        // clause scanner resolves each line's leading keyword through the
        // keyword table, so the `@sees` label is not mistaken for SEES and the
        // cursor on it is not reported as a component-reference position.
        let source = "\
MACHINE m
INVARIANTS
    @sees x > 0
EVENTS
    EVENT e
    THEN
        @act1 x ≔ 1
    END
END";
        assert!(!is_component_reference_position(
            source,
            Position::new(2, 4)
        ));
    }

    #[test]
    fn test_cursor_in_comment_finds_no_identifier() {
        // `count` inside the comment is prose; references from there resolve
        // to nothing.
        let source = "count := 0 // count is reset";
        assert_eq!(
            get_identifier_at_position(source, Position::new(0, 14)),
            None
        );
    }
}
