//! Find all references to symbols functionality
//!
//! This module provides the ability to find all references to Event-B symbols
//! (variables, constants, sets, events, parameters) throughout the document
//! and across the workspace.

use crate::lsp_types::*;
use rossi::{Component, parse};
use std::collections::HashSet;
use std::sync::Arc;
use tracing::debug;

use crate::cross_references::{ComponentKind, CrossReferenceManager};
use crate::document::DocumentManager;
use crate::identifier_utils;
use crate::symbols::{SymbolKind, SymbolRef, enumerate_symbols};
use crate::text_utils;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SymbolIdentity {
    name: String,
    kind: SymbolKind,
    owner: String,
    event: Option<String>,
}

impl SymbolIdentity {
    fn parameter(name: &str, machine_name: &str, event_name: &str) -> Self {
        Self {
            name: name.to_string(),
            kind: SymbolKind::Parameter,
            owner: machine_name.to_string(),
            event: Some(event_name.to_string()),
        }
    }
}

impl From<SymbolRef> for SymbolIdentity {
    fn from(symbol: SymbolRef) -> Self {
        Self {
            name: symbol.name,
            kind: symbol.kind,
            owner: symbol.owner,
            event: symbol.event,
        }
    }
}

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

        // Get the identifier at the cursor position
        let identifier = get_identifier_at_position(text, position)?;

        debug!(
            "Finding references for identifier '{}' at {:?}",
            identifier, position
        );

        let locations = self.find_references_for_identifier(text, uri, position, &identifier);
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
        identifier_utils::find_whole_word_locations(text, identifier, uri, line_range)
    }

    fn find_symbol_references_in_text_range(
        &self,
        text: &str,
        uri: &Url,
        identifier: &str,
        line_range: Option<(usize, usize)>,
    ) -> Vec<Location> {
        self.find_references_in_text_range(text, uri, identifier, line_range)
            .into_iter()
            .filter(|location| !is_component_reference_position(text, location.range.start))
            .collect()
    }

    fn find_references_for_identifier(
        &self,
        text: &str,
        uri: &Url,
        position: Position,
        identifier: &str,
    ) -> Vec<Location> {
        let Some(manager) = &self.cross_ref_manager else {
            return self.find_references_in_text(text, uri, identifier);
        };
        let is_component_name = manager.find_component_uri(identifier).is_some();

        if is_component_name && is_component_reference_position(text, position) {
            return self.find_references_in_workspace(identifier);
        }

        if let Some(symbol) = self.resolve_symbol_identity(text, position, identifier, manager) {
            return self.find_references_for_symbol(&symbol, manager);
        }

        if is_component_name {
            return self.find_references_in_workspace(identifier);
        }

        self.find_references_in_text(text, uri, identifier)
    }

    fn find_references_for_symbol(
        &self,
        symbol: &SymbolIdentity,
        manager: &CrossReferenceManager,
    ) -> Vec<Location> {
        let mut locations = Vec::new();
        let mut seen = HashSet::new();

        if symbol.kind == SymbolKind::Parameter {
            let Some(event_name) = symbol.event.as_deref() else {
                return locations;
            };

            let Some((component_uri, component_text, _)) =
                self.load_component_by_name(&symbol.owner, manager)
            else {
                return locations;
            };
            let Some(line_range) = event_line_range(&component_text, event_name) else {
                return locations;
            };

            push_unique_locations(
                &mut locations,
                &mut seen,
                self.find_references_in_text_range(
                    &component_text,
                    &component_uri,
                    &symbol.name,
                    Some(line_range),
                ),
            );

            return locations;
        }

        for component_name in self.candidate_components_for_symbol(symbol, manager) {
            let Some((component_uri, component_text, component)) =
                self.load_component_by_name(&component_name, manager)
            else {
                continue;
            };

            if self.resolve_symbol_identity_in_component(&component, &symbol.name, manager)
                == Some(symbol.clone())
            {
                push_unique_locations(
                    &mut locations,
                    &mut seen,
                    self.find_symbol_references_in_text_range(
                        &component_text,
                        &component_uri,
                        &symbol.name,
                        None,
                    ),
                );
            }
        }

        locations
    }

    fn resolve_symbol_identity(
        &self,
        text: &str,
        position: Position,
        identifier: &str,
        manager: &CrossReferenceManager,
    ) -> Option<SymbolIdentity> {
        let component = parse(text).ok()?;
        self.resolve_symbol_identity_at_position(&component, text, position, identifier, manager)
    }

    fn resolve_symbol_identity_at_position(
        &self,
        component: &Component,
        text: &str,
        position: Position,
        identifier: &str,
        manager: &CrossReferenceManager,
    ) -> Option<SymbolIdentity> {
        if let Component::Machine(machine) = component
            && let Some(parameter) =
                local_parameter_symbol_identity_at_position(machine, text, position, identifier)
        {
            return Some(parameter);
        }

        self.resolve_symbol_identity_in_component(component, identifier, manager)
    }

    fn resolve_symbol_identity_in_component(
        &self,
        component: &Component,
        identifier: &str,
        manager: &CrossReferenceManager,
    ) -> Option<SymbolIdentity> {
        if let Some(local) = local_symbol_identity(component, identifier) {
            return Some(local);
        }

        match component {
            Component::Machine(machine) => {
                for machine_name in manager.refinement_chain(&machine.name) {
                    if let Some((_, _, component)) =
                        self.load_component_by_name(&machine_name, manager)
                        && let Some(symbol) = local_symbol_identity(&component, identifier)
                    {
                        return Some(symbol);
                    }
                }

                for context_name in manager.ordered_visible_contexts(&machine.name) {
                    if let Some((_, _, component)) =
                        self.load_component_by_name(&context_name, manager)
                        && let Some(symbol) = local_symbol_identity(&component, identifier)
                    {
                        return Some(symbol);
                    }
                }
            }
            Component::Context(context) => {
                for context_name in manager.ordered_extends_chain(&context.name) {
                    if let Some((_, _, component)) =
                        self.load_component_by_name(&context_name, manager)
                        && let Some(symbol) = local_symbol_identity(&component, identifier)
                    {
                        return Some(symbol);
                    }
                }
            }
        }

        None
    }

    fn candidate_components_for_symbol(
        &self,
        symbol: &SymbolIdentity,
        manager: &CrossReferenceManager,
    ) -> Vec<String> {
        if symbol.kind == SymbolKind::Parameter {
            return vec![symbol.owner.clone()];
        }

        let mut candidates = Vec::new();
        let mut component_names = manager.all_component_names();
        component_names.sort();

        for component_name in component_names {
            if component_name == symbol.owner {
                candidates.push(component_name);
                continue;
            }

            let Some(info) = manager.get_component(&component_name) else {
                continue;
            };

            match (symbol.kind, info.kind) {
                (SymbolKind::Set | SymbolKind::Constant, ComponentKind::Context)
                    if manager
                        .ordered_extends_chain(&component_name)
                        .contains(&symbol.owner) =>
                {
                    candidates.push(component_name);
                }
                (SymbolKind::Set | SymbolKind::Constant, ComponentKind::Machine)
                    if manager
                        .ordered_visible_contexts(&component_name)
                        .contains(&symbol.owner) =>
                {
                    candidates.push(component_name);
                }
                (SymbolKind::Variable | SymbolKind::Event, ComponentKind::Machine)
                    if manager
                        .refinement_chain(&component_name)
                        .contains(&symbol.owner) =>
                {
                    candidates.push(component_name);
                }
                _ => {}
            }
        }

        candidates.sort();
        candidates.dedup();
        candidates
    }

    /// Find all references across the workspace
    fn find_references_in_workspace(&self, identifier: &str) -> Vec<Location> {
        let mut locations = Vec::new();
        let mut seen = HashSet::new();

        let manager = match &self.cross_ref_manager {
            Some(m) => m,
            None => return locations,
        };

        let mut component_names = manager.all_component_names();
        component_names.sort();

        for component_name in component_names {
            if let Some((uri, text, _)) = self.load_component_by_name(&component_name, manager) {
                push_unique_locations(
                    &mut locations,
                    &mut seen,
                    self.find_references_in_text(&text, &uri, identifier),
                );
            }
        }

        locations
    }

    fn load_component_by_name(
        &self,
        component_name: &str,
        manager: &CrossReferenceManager,
    ) -> Option<(Url, String, Component)> {
        let uri_str = manager.find_component_uri(component_name)?;
        let uri = Url::parse(&uri_str).ok()?;
        let text = manager.load_component_text(component_name, self.document_manager.as_deref())?;
        let component = parse(&text).ok()?;
        Some((uri, text, component))
    }
}

/// Resolve `identifier` to a symbol declared directly in `component`.
///
/// Parameters are excluded here — they are scoped to an event body and resolved
/// positionally by [`local_parameter_symbol_identity_at_position`].
fn local_symbol_identity(component: &Component, identifier: &str) -> Option<SymbolIdentity> {
    enumerate_symbols(component)
        .into_iter()
        .find(|symbol| symbol.name == identifier && symbol.kind != SymbolKind::Parameter)
        .map(SymbolIdentity::from)
}

fn local_parameter_symbol_identity_at_position(
    machine: &rossi::Machine,
    text: &str,
    position: Position,
    identifier: &str,
) -> Option<SymbolIdentity> {
    let line_idx = position.line as usize;

    machine.events.iter().find_map(|event| {
        let (start_line, end_line) = event_line_range(text, &event.name)?;
        if line_idx < start_line || line_idx > end_line {
            return None;
        }

        event
            .parameters
            .iter()
            .any(|parameter| parameter.name == identifier)
            .then(|| SymbolIdentity::parameter(identifier, &machine.name, &event.name))
    })
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

fn non_empty(locations: Vec<Location>) -> Option<Vec<Location>> {
    if locations.is_empty() {
        None
    } else {
        Some(locations)
    }
}

fn event_line_range(text: &str, event_name: &str) -> Option<(usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let start_line = lines
        .iter()
        .position(|line| text_utils::event_name_from_line(line).as_deref() == Some(event_name))?;

    let end_line = lines
        .iter()
        .enumerate()
        .skip(start_line + 1)
        .find_map(|(line_idx, line)| {
            text_utils::first_identifier_word(line)
                .as_deref()
                .is_some_and(|word| word.eq_ignore_ascii_case("END"))
                .then_some(line_idx)
        })
        .unwrap_or_else(|| lines.len().saturating_sub(1));

    Some((start_line, end_line))
}

fn is_component_reference_position(text: &str, position: Position) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    if position.line as usize >= lines.len() {
        return false;
    }

    let mut current_clause = None;
    let mut in_event = false;

    for line in lines.iter().take(position.line as usize + 1) {
        if text_utils::event_name_from_line(line).is_some() {
            in_event = true;
            current_clause = None;
            continue;
        }

        let first_word = text_utils::first_identifier_word(line);
        let first_word = first_word.as_deref();

        match first_word {
            Some(word) if word.eq_ignore_ascii_case("END") && in_event => {
                in_event = false;
                current_clause = None;
            }
            Some(word) if word.eq_ignore_ascii_case("SEES") && !in_event => {
                current_clause = Some("SEES")
            }
            Some(word) if word.eq_ignore_ascii_case("EXTENDS") && !in_event => {
                current_clause = Some("EXTENDS")
            }
            Some(word) if word.eq_ignore_ascii_case("REFINES") && !in_event => {
                current_clause = Some("REFINES")
            }
            Some(word) if text_utils::is_clause_boundary_keyword(word) => current_clause = None,
            _ => {}
        }
    }

    current_clause.is_some()
}

/// Get the identifier at the given position in the text
fn get_identifier_at_position(text: &str, position: Position) -> Option<String> {
    identifier_utils::identifier_at_position(text, position).map(|(identifier, _)| identifier)
}

/// Find a whole word match in a line and return its column index (in characters, not bytes)
#[cfg(test)]
fn find_whole_word_in_line(line: &str, word: &str) -> Option<usize> {
    let chars: Vec<char> = line.chars().collect();
    let word_chars: Vec<char> = word.chars().collect();

    let mut idx = 0;
    while idx + word_chars.len() <= chars.len() {
        // Check if word matches at this position
        let matches = chars[idx..idx + word_chars.len()] == word_chars;

        if matches {
            // Check word boundaries
            let before_ok = idx == 0 || !text_utils::is_identifier_char(chars[idx - 1]);
            let after_ok = idx + word_chars.len() >= chars.len()
                || !text_utils::is_identifier_char(chars[idx + word_chars.len()]);

            if before_ok && after_ok {
                return Some(idx);
            }
        }

        idx += 1;
    }
    None
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
    fn test_find_whole_word_in_line() {
        let line = "count := count + counter";

        // Should find 'count' at position 0
        assert_eq!(find_whole_word_in_line(line, "count"), Some(0));

        // Should find 'count' in "count + counter" (second occurrence)
        assert_eq!(find_whole_word_in_line(&line[9..], "count"), Some(0));

        // Should NOT find 'count' as part of 'counter'
        let result = find_whole_word_in_line(line, "counter");
        assert_eq!(result, Some(17));
    }

    #[test]
    fn test_identifier_boundaries() {
        let line = "my_var := my_var + my_variable";

        // Find 'my_var' should not match 'my_variable'
        assert_eq!(find_whole_word_in_line(line, "my_var"), Some(0));
        assert_eq!(find_whole_word_in_line(&line[10..], "my_var"), Some(0));

        // Find 'my_variable' should match at the end
        assert_eq!(find_whole_word_in_line(line, "my_variable"), Some(19));
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
}
