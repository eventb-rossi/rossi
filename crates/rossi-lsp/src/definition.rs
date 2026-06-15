//! Go-to-definition provider for Event-B
//!
//! Enables navigation to where identifiers are defined:
//! - Variables (in VARIABLES clause)
//! - Constants (in CONSTANTS clause)
//! - Sets (in SETS clause)
//! - Events (EVENT declarations)
//! - Parameters (in ANY clause)
//! - Cross-file references (SEES, REFINES, EXTENDS)

use crate::lsp_types::{
    GotoDefinitionParams, GotoDefinitionResponse, Location, Position, Range, Url,
};
use dashmap::DashMap;
use rossi::{Component, parse_components};
use std::collections::HashMap;
use std::sync::Arc;

use crate::component_util::{component_line_window, lines_in_window, parse_named};
use crate::cross_references::CrossReferenceManager;
use crate::document::DocumentManager;
use crate::position::{char_col_to_utf16, span_to_range, utf16_len};
use crate::references::component_reference_clause;
use crate::symbols::SymbolKind;
use crate::text_utils;

/// Information about where an identifier is defined
#[derive(Debug, Clone)]
struct DefinitionInfo {
    /// The name of the identifier
    name: String,
    /// The kind of definition
    kind: SymbolKind,
    /// Location in the source file
    location: Location,
    /// Inclusive line range (in the cached document) the definition is
    /// visible from: the declaring component's lines for local definitions,
    /// the requesting component's lines for cross-file resolutions. `None`
    /// means visible from anywhere.
    component_lines: Option<(usize, usize)>,
}

/// Context containing all definitions in a document
#[derive(Debug, Clone)]
struct DefinitionContext {
    definitions: Vec<DefinitionInfo>,
}

impl DefinitionContext {
    fn new() -> Self {
        Self {
            definitions: Vec::new(),
        }
    }

    /// Find definition by name, preferring local definitions over cross-file ones
    #[cfg(test)]
    fn find_definition(&self, name: &str) -> Option<&DefinitionInfo> {
        self.find_definition_at(name, None)
    }

    /// Find definition by name. With a cursor line, definitions declared by
    /// the component containing that line win over same-named definitions in
    /// sibling components of a multi-component document.
    fn find_definition_at(
        &self,
        name: &str,
        cursor_line: Option<usize>,
    ) -> Option<&DefinitionInfo> {
        if let Some(line) = cursor_line
            && let Some(found) = Self::prefer_local(self.definitions.iter().filter(|d| {
                d.name == name
                    && d.component_lines
                        .is_some_and(|(start, end)| (start..=end).contains(&line))
            }))
        {
            return Some(found);
        }
        Self::prefer_local(self.definitions.iter().filter(|d| d.name == name))
    }

    /// Prefer local definitions (Variable, Event, Parameter) over cross-file
    /// ones (Constant, Set)
    fn prefer_local<'a>(
        candidates: impl Iterator<Item = &'a DefinitionInfo> + Clone,
    ) -> Option<&'a DefinitionInfo> {
        candidates
            .clone()
            .find(|d| {
                matches!(
                    d.kind,
                    SymbolKind::Variable | SymbolKind::Event | SymbolKind::Parameter
                )
            })
            .or_else(|| candidates.into_iter().next())
    }
}

/// Provides go-to-definition functionality
pub struct DefinitionProvider {
    /// Cache of definition contexts per document
    definition_cache: Arc<DashMap<String, DefinitionContext>>,
    /// Cross-reference manager for workspace-wide navigation
    cross_ref_manager: Option<Arc<CrossReferenceManager>>,
    /// Document manager for reading open documents
    document_manager: Option<Arc<DocumentManager>>,
}

impl DefinitionProvider {
    pub fn new() -> Self {
        Self {
            definition_cache: Arc::new(DashMap::new()),
            cross_ref_manager: None,
            document_manager: None,
        }
    }

    /// Set the cross-reference manager for workspace-wide navigation
    pub fn set_cross_reference_manager(&mut self, manager: Arc<CrossReferenceManager>) {
        self.cross_ref_manager = Some(manager);
    }

    /// Set the document manager for reading open documents
    pub fn set_document_manager(&mut self, manager: Arc<DocumentManager>) {
        self.document_manager = Some(manager);
    }

    /// Update the definition cache for a document
    pub fn update_definitions(&self, uri: String, text: &str) {
        match parse_components(text) {
            Ok(components) if !components.is_empty() => {
                let mut ctx = DefinitionContext::new();
                // Sibling components of a merged file typically see the same
                // contexts/machines — extract each visible component once per
                // update, not once per sibling.
                let mut cross_cache = HashMap::new();
                for component in &components {
                    ctx.definitions.extend(
                        self.extract_definitions(component, text, &uri, &mut cross_cache)
                            .definitions,
                    );
                }
                self.definition_cache.insert(uri, ctx);
            }
            _ => {
                // Remove from cache if parsing fails
                self.definition_cache.remove(&uri);
            }
        }
    }

    /// Handle go-to-definition request
    pub fn goto_definition(
        &self,
        params: &GotoDefinitionParams,
        text: &str,
    ) -> Option<GotoDefinitionResponse> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        // All structural scans run on comment-masked text (char columns are
        // preserved). A cursor inside a comment then finds no identifier and
        // the request resolves to nothing, as it should.
        let masked = rossi::comments::mask_comments_chars(text);

        // Definitions are only ever named by identifiers, so resolve the
        // cursor straight to one (operators and punctuation have none).
        let (word, _) = identifier_at_position(&masked, position)?;

        // Stage 1: Check if this is a cross-file reference (SEES, REFINES, EXTENDS clause name)
        if let Some(cross_ref_location) = self.find_cross_file_reference(&masked, position, &word) {
            return Some(GotoDefinitionResponse::Scalar(cross_ref_location));
        }

        // Stage 2: Local + cross-file definitions from cache. The cursor line
        // scopes the lookup to the component under the cursor first.
        if let Some(def_ctx) = self.definition_cache.get(&uri.to_string())
            && let Some(def_info) = def_ctx.find_definition_at(&word, Some(position.line as usize))
        {
            return Some(GotoDefinitionResponse::Scalar(def_info.location.clone()));
        }

        None
    }

    /// Find cross-file reference location (SEES, REFINES, EXTENDS).
    /// `masked` is the comment-masked document text.
    fn find_cross_file_reference(
        &self,
        masked: &str,
        position: Position,
        word: &str,
    ) -> Option<Location> {
        let cross_ref_manager = self.cross_ref_manager.as_ref()?;

        // Check if cursor is in a SEES, REFINES, or EXTENDS clause. Reuses the
        // references provider's detector: case-insensitive (matching the
        // grammar's `^"sees"` keywords) and in-event-aware, so a `sees` spelled
        // any way resolves and one mentioned inside an event never misfires.
        component_reference_clause(masked, position)?;

        // Load the target file (prefer open documents over disk)
        let target_text =
            cross_ref_manager.load_component_text(word, self.document_manager.as_deref())?;
        let target_uri = cross_ref_manager.find_component_uri(word)?;
        let target_url = Url::parse(&target_uri).ok()?;

        // Locate the component's name via the parser (the source of truth) and
        // map its span to a range — exact for any casing or spacing, unlike a
        // textual keyword scan.
        let component = parse_named(&target_text, word)?;
        let name_span = component.name_span()?;
        let range = span_to_range(&name_span, &target_text);

        Some(Location::new(target_url, range))
    }

    /// Extract all definitions visible from a component: those declared locally
    /// plus those reachable through SEES/EXTENDS/REFINES.
    fn extract_definitions(
        &self,
        component: &Component,
        text: &str,
        uri_str: &str,
        cross_cache: &mut HashMap<String, Vec<DefinitionInfo>>,
    ) -> DefinitionContext {
        let window = component_line_window(component, text);
        let mut ctx = DefinitionContext::new();
        ctx.definitions = self.extract_local_definitions(component, text, uri_str, window);

        // Add cross-file definitions from SEES/EXTENDS/REFINES contexts and
        // machines. They are scoped to the REQUESTING component's lines: that
        // is where this component's visibility applies, so in a
        // multi-component document the cursor picks the resolution belonging
        // to the component it sits in.
        if let Some(crm) = &self.cross_ref_manager {
            let mut cross = self.resolve_cross_file_definitions(component, crm, cross_cache);
            for definition in &mut cross {
                definition.component_lines = Some(window);
            }
            ctx.definitions.extend(cross);
        }

        ctx
    }

    /// Extract definitions declared directly in `component` (no cross-file walk).
    ///
    /// All text searches are bounded to `window` (the component's line window)
    /// so that in a multi-component document a sibling component's clauses
    /// cannot shadow this component's declarations.
    fn extract_local_definitions(
        &self,
        component: &Component,
        text: &str,
        uri_str: &str,
        window: (usize, usize),
    ) -> Vec<DefinitionInfo> {
        let mut definitions = Vec::new();
        let uri = match Url::parse(uri_str) {
            Ok(u) => u,
            Err(_) => return definitions, // Return empty if URI parsing fails
        };

        // Mask comments once for all clause/event scans below: a declaration
        // name or keyword mentioned in a comment must never become the
        // definition site (char columns are unchanged by the mask).
        let masked = rossi::comments::mask_comments_chars(text);
        let text = masked.as_str();

        let def_info = |name: &str, kind: SymbolKind, pos: Position| DefinitionInfo {
            name: name.to_string(),
            kind,
            location: Location {
                uri: uri.clone(),
                range: Range {
                    start: pos,
                    end: Position::new(pos.line, pos.character + utf16_len(name)),
                },
            },
            component_lines: Some(window),
        };

        match component {
            Component::Context(context) => {
                // Extract sets
                for set in &context.sets {
                    let set_name = set.name();
                    if let Some(pos) = find_identifier_in_clause(text, "SETS", set_name, window) {
                        definitions.push(def_info(set_name, SymbolKind::Set, pos));
                    }
                }

                // Extract constants
                for constant in &context.constants {
                    if let Some(pos) =
                        find_identifier_in_clause(text, "CONSTANTS", &constant.name, window)
                    {
                        definitions.push(def_info(&constant.name, SymbolKind::Constant, pos));
                    }
                }
            }
            Component::Machine(machine) => {
                // Extract variables
                for variable in &machine.variables {
                    if let Some(pos) =
                        find_identifier_in_clause(text, "VARIABLES", &variable.name, window)
                    {
                        definitions.push(def_info(&variable.name, SymbolKind::Variable, pos));
                    }
                }

                // Extract events
                for event in &machine.events {
                    if let Some(pos) = find_event_definition(text, &event.name, window) {
                        definitions.push(def_info(&event.name, SymbolKind::Event, pos));
                    }

                    // Extract event parameters
                    for param in &event.parameters {
                        if let Some(pos) =
                            find_identifier_in_event(text, &event.name, "ANY", &param.name, window)
                        {
                            definitions.push(def_info(&param.name, SymbolKind::Parameter, pos));
                        }
                    }
                }

                // Handle INITIALISATION event
                if machine.initialisation.is_some()
                    && let Some(pos) = find_initialisation_definition(text, window)
                {
                    definitions.push(def_info("INITIALISATION", SymbolKind::Event, pos));
                }
            }
        }

        definitions
    }

    /// Resolve definitions reachable through SEES/EXTENDS/REFINES, reusing the
    /// cross-reference manager's visibility graph and extracting each visible
    /// component's local definitions with `extract_local_definitions`.
    ///
    /// `cache` memoizes the extraction per visible component name for the
    /// duration of one document update — loading and parsing a visible file
    /// once instead of once per sibling component that sees it.
    fn resolve_cross_file_definitions(
        &self,
        component: &Component,
        cross_ref_manager: &CrossReferenceManager,
        cache: &mut HashMap<String, Vec<DefinitionInfo>>,
    ) -> Vec<DefinitionInfo> {
        let component_names = match component {
            Component::Machine(machine) => {
                let mut names = cross_ref_manager.refinement_chain(&machine.name);
                names.extend(cross_ref_manager.ordered_visible_contexts(&machine.name));
                names
            }
            Component::Context(context) => cross_ref_manager.ordered_extends_chain(&context.name),
        };

        let mut results = Vec::new();
        for name in component_names {
            let definitions = cache.entry(name).or_insert_with_key(|name| {
                self.extract_visible_definitions(name, cross_ref_manager)
                    .unwrap_or_default()
            });
            results.extend(definitions.iter().cloned());
        }

        results
    }

    /// Load, parse, and extract the local definitions of one visible
    /// component. The target's own line window is overwritten by
    /// `extract_definitions` with the requesting component's window — the
    /// lines these definitions are visible from.
    fn extract_visible_definitions(
        &self,
        name: &str,
        cross_ref_manager: &CrossReferenceManager,
    ) -> Option<Vec<DefinitionInfo>> {
        let text = cross_ref_manager.load_component_text(name, self.document_manager.as_deref())?;
        let component = parse_named(&text, name)?;
        let uri_str = cross_ref_manager.find_component_uri(name)?;
        let window = component_line_window(&component, &text);
        Some(self.extract_local_definitions(&component, &text, &uri_str, window))
    }
}

impl Default for DefinitionProvider {
    fn default() -> Self {
        Self::new()
    }
}

// Helper functions

use crate::identifier_utils::identifier_at_position;

/// Find the first occurrence of `needle` (as chars) in `haystack` starting at `from`.
/// Returns the char index of the match, or `None`.
fn char_find_substr(haystack: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() || from + needle.len() > haystack.len() {
        return None;
    }
    (from..=haystack.len() - needle.len()).find(|&i| haystack[i..i + needle.len()] == *needle)
}

/// Check if a word is at a specific char position with proper word boundaries
fn is_whole_word_at(chars: &[char], pos: usize, word_len: usize) -> bool {
    if pos + word_len > chars.len() {
        return false;
    }
    let before_ok = pos == 0 || !chars[pos - 1].is_alphanumeric() && chars[pos - 1] != '_';
    let after_ok = pos + word_len >= chars.len()
        || !chars[pos + word_len].is_alphanumeric() && chars[pos + word_len] != '_';
    before_ok && after_ok
}

/// Find an identifier within a clause (e.g., VARIABLES, CONSTANTS, SETS)
fn find_identifier_in_clause(
    text: &str,
    clause: &str,
    identifier: &str,
    window: (usize, usize),
) -> Option<Position> {
    let id_chars: Vec<char> = identifier.chars().collect();

    // Find the clause line
    let mut in_clause = false;
    for (line_num, line) in lines_in_window(text, window) {
        let trimmed = line.trim();

        // Check if we're entering the clause (keywords are case-insensitive).
        if trimmed.eq_ignore_ascii_case(clause) {
            in_clause = true;
            continue;
        }

        // Check if we've left the clause (another keyword)
        if in_clause && text_utils::is_declaration_scan_boundary(trimmed) {
            break;
        }

        // Search for identifier in this clause
        if in_clause {
            let chars: Vec<char> = line.chars().collect();
            if let Some(col) = char_find_substr(&chars, &id_chars, 0)
                && is_whole_word_at(&chars, col, id_chars.len())
            {
                return Some(Position::new(line_num as u32, char_col_to_utf16(line, col)));
            }
        }
    }

    None
}

/// Find an event definition
fn find_event_definition(text: &str, event_name: &str, window: (usize, usize)) -> Option<Position> {
    let name_chars: Vec<char> = event_name.chars().collect();

    for (line_num, line) in lines_in_window(text, window) {
        // Only a header line whose `EVENT` keyword (any casing) names this
        // event — `event_name_from_line` reads the keyword case-insensitively
        // and keeps a hyphenated name whole.
        if text_utils::event_name_from_line(line).as_deref() != Some(event_name) {
            continue;
        }
        // The name itself is case-sensitive; locate its first whole-word
        // occurrence for the column. Scanning for whole words (not the first
        // substring) avoids a false hit when the name is a substring of the
        // EVENT keyword spelled before it (e.g. an event named `ent`).
        let chars: Vec<char> = line.chars().collect();
        let mut from = 0;
        while let Some(name_pos) = char_find_substr(&chars, &name_chars, from) {
            if is_whole_word_at(&chars, name_pos, name_chars.len()) {
                return Some(Position::new(
                    line_num as u32,
                    char_col_to_utf16(line, name_pos),
                ));
            }
            from = name_pos + 1;
        }
    }

    None
}

/// Find INITIALISATION event definition
fn find_initialisation_definition(text: &str, window: (usize, usize)) -> Option<Position> {
    // The init event's header is `EVENT INITIALISATION` (any casing). Detect it
    // by its event name (as `find_event_definition` does), then locate the
    // INITIALISATION token's column case-insensitively (an ASCII-uppercased copy
    // preserves char columns). Uppercasing only the matched header line.
    let kw = rossi::keywords::spell(rossi::keywords::KeywordId::Initialisation);
    let kw_chars: Vec<char> = kw.chars().collect();

    for (line_num, line) in lines_in_window(text, window) {
        if !text_utils::event_name_from_line(line).is_some_and(|name| name.eq_ignore_ascii_case(kw))
        {
            continue;
        }
        let upper: Vec<char> = line.chars().map(|c| c.to_ascii_uppercase()).collect();
        if let Some(pos) = char_find_substr(&upper, &kw_chars, 0) {
            return Some(Position::new(line_num as u32, char_col_to_utf16(line, pos)));
        }
    }

    None
}

/// Find an identifier within an event (e.g., ANY clause)
fn find_identifier_in_event(
    text: &str,
    event_name: &str,
    clause: &str,
    identifier: &str,
    window: (usize, usize),
) -> Option<Position> {
    let id_chars: Vec<char> = identifier.chars().collect();

    // Find the event first
    let mut in_event = false;
    let mut in_clause = false;

    for (line_num, line) in lines_in_window(text, window) {
        let trimmed = line.trim();

        // Walk to the target event's header (EVENT keyword in any casing).
        if !in_event {
            if text_utils::event_name_from_line(line).as_deref() == Some(event_name) {
                in_event = true;
            }
            continue;
        }

        // Check if we've left the event: its END or the next event's header.
        if trimmed.eq_ignore_ascii_case("END") || text_utils::event_name_from_line(line).is_some() {
            break;
        }

        // Check if we're entering the clause within the event
        if trimmed.eq_ignore_ascii_case(clause) {
            in_clause = true;
            continue;
        }

        // Check if we've left the clause
        if in_clause && text_utils::is_declaration_scan_boundary(trimmed) {
            in_clause = false;
        }

        // Search for identifier in this clause
        if in_clause {
            let chars: Vec<char> = line.chars().collect();
            if let Some(col) = char_find_substr(&chars, &id_chars, 0)
                && is_whole_word_at(&chars, col, id_chars.len())
            {
                return Some(Position::new(line_num as u32, char_col_to_utf16(line, col)));
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp_types::{TextDocumentIdentifier, TextDocumentPositionParams};

    /// Whole-file search window, the single-component default.
    const FULL: (usize, usize) = (0, usize::MAX);

    #[test]
    fn test_definition_provider_creation() {
        let provider = DefinitionProvider::new();
        assert!(provider.definition_cache.is_empty());
    }

    #[test]
    fn test_extract_variable_definitions() {
        let provider = DefinitionProvider::new();
        let source = "MACHINE test\nVARIABLES\n    count\n    total\nEND";

        provider.update_definitions("file:///test.eventb".to_string(), source);

        let cache = provider
            .definition_cache
            .get("file:///test.eventb")
            .unwrap();
        assert_eq!(cache.definitions.len(), 2);

        let count_def = cache.find_definition("count");
        assert!(count_def.is_some());
        assert_eq!(count_def.unwrap().kind, SymbolKind::Variable);

        let total_def = cache.find_definition("total");
        assert!(total_def.is_some());
        assert_eq!(total_def.unwrap().kind, SymbolKind::Variable);
    }

    #[test]
    fn test_definition_site_ignores_comment_mentions() {
        let provider = DefinitionProvider::new();
        // `count` is mentioned in the machine-header comment before its real
        // declaration; the definition must point at the VARIABLES clause.
        let source = "MACHINE test // count of VARIABLES\nVARIABLES\n    count // count again\nEND";

        provider.update_definitions("file:///test.eventb".to_string(), source);

        let cache = provider
            .definition_cache
            .get("file:///test.eventb")
            .unwrap();
        let count_def = cache.find_definition("count").unwrap();
        assert_eq!(count_def.location.range.start.line, 2);
        assert_eq!(count_def.location.range.start.character, 4);
    }

    #[test]
    fn test_extract_constant_definitions() {
        let provider = DefinitionProvider::new();
        let source = "CONTEXT test\nCONSTANTS\n    max_value\n    min_value\nEND";

        provider.update_definitions("file:///test.eventb".to_string(), source);

        let cache = provider
            .definition_cache
            .get("file:///test.eventb")
            .unwrap();
        assert_eq!(cache.definitions.len(), 2);

        let max_def = cache.find_definition("max_value");
        assert!(max_def.is_some());
        assert_eq!(max_def.unwrap().kind, SymbolKind::Constant);
    }

    #[test]
    fn test_extract_set_definitions() {
        let provider = DefinitionProvider::new();
        let source = "CONTEXT test\nSETS\n    STATUS\n    COLORS\nEND";

        provider.update_definitions("file:///test.eventb".to_string(), source);

        let cache = provider
            .definition_cache
            .get("file:///test.eventb")
            .unwrap();
        assert_eq!(cache.definitions.len(), 2);

        let status_def = cache.find_definition("STATUS");
        assert!(status_def.is_some());
        assert_eq!(status_def.unwrap().kind, SymbolKind::Set);
    }

    #[test]
    fn test_extract_event_definitions() {
        let provider = DefinitionProvider::new();
        let source =
            "MACHINE test\nEVENTS\n    EVENT increment\n    END\n    EVENT decrement\n    END\nEND";

        provider.update_definitions("file:///test.eventb".to_string(), source);

        let cache = provider
            .definition_cache
            .get("file:///test.eventb")
            .unwrap();

        let inc_def = cache.find_definition("increment");
        assert!(inc_def.is_some());
        assert_eq!(inc_def.unwrap().kind, SymbolKind::Event);

        let dec_def = cache.find_definition("decrement");
        assert!(dec_def.is_some());
        assert_eq!(dec_def.unwrap().kind, SymbolKind::Event);
    }

    #[test]
    fn test_find_identifier_in_clause() {
        let text = "MACHINE test\nVARIABLES\n    count\n    total\nEND";

        let pos = find_identifier_in_clause(text, "VARIABLES", "count", FULL);
        assert!(pos.is_some());
        let pos = pos.unwrap();
        assert_eq!(pos.line, 2);
        assert!(pos.character >= 4); // After indentation

        let pos = find_identifier_in_clause(text, "VARIABLES", "total", FULL);
        assert!(pos.is_some());
        let pos = pos.unwrap();
        assert_eq!(pos.line, 3);
    }

    #[test]
    fn test_find_event_definition() {
        let text = "MACHINE test\nEVENTS\n    EVENT increment\n    WHERE\n        count < 10\n    END\nEND";

        let pos = find_event_definition(text, "increment", FULL);
        assert!(pos.is_some());
        let pos = pos.unwrap();
        assert_eq!(pos.line, 2);
    }

    #[test]
    fn test_find_initialisation_definition() {
        let text = "MACHINE test\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        count := 0\n    END\nEND";

        let pos = find_initialisation_definition(text, FULL);
        assert!(pos.is_some());
        let pos = pos.unwrap();
        assert_eq!(pos.line, 2);
    }

    #[test]
    fn test_local_finders_are_case_insensitive() {
        // Lowercase keywords (Camille style) must resolve like UPPERCASE ones.
        let text = "machine test\nvariables\n    count\nevents\n    event increment\n    then\n        count := count + 1\n    end\n    event initialisation\n    then\n        count := 0\n    end\nend";

        assert_eq!(
            find_identifier_in_clause(text, "VARIABLES", "count", FULL).map(|p| p.line),
            Some(2)
        );
        assert_eq!(
            find_event_definition(text, "increment", FULL).map(|p| p.line),
            Some(4)
        );
        // The init header sits on the second `event ...` line.
        assert_eq!(
            find_initialisation_definition(text, FULL).map(|p| p.line),
            Some(8)
        );
    }

    #[test]
    fn test_clause_scan_keeps_status_as_a_set_name() {
        // STATUS is a contextual keyword but a common set name; a SETS clause
        // member named STATUS must be found, and must not end the scan early so
        // a following member is still reachable. Lowercase header to boot.
        let text = "context c\nsets\n    STATUS\n    Colours\nend";

        assert_eq!(
            find_identifier_in_clause(text, "SETS", "STATUS", FULL).map(|p| p.line),
            Some(2)
        );
        assert_eq!(
            find_identifier_in_clause(text, "SETS", "Colours", FULL).map(|p| p.line),
            Some(3)
        );
    }

    #[test]
    fn test_identifier_at_position() {
        let text = "MACHINE test_machine";

        // At 't' in test_machine
        let word = identifier_at_position(text, Position::new(0, 8)).map(|(word, _)| word);
        assert_eq!(word, Some("test_machine".to_string()));

        // At 'M' in MACHINE
        let word = identifier_at_position(text, Position::new(0, 0)).map(|(word, _)| word);
        assert_eq!(word, Some("MACHINE".to_string()));
    }

    #[test]
    fn test_identifier_at_position_unicode() {
        // Line with Unicode operators before the target word
        let text = "    @inv1 count ∈ ℕ";
        //  chars: 0123456789...
        //  '∈' is at char index 16, 'ℕ' is at char index 18

        // Hovering on 'count' (char index 10)
        let word = identifier_at_position(text, Position::new(0, 10)).map(|(word, _)| word);
        assert_eq!(word, Some("count".to_string()));

        // Hovering on 'inv1' (char index 5)
        let word = identifier_at_position(text, Position::new(0, 5)).map(|(word, _)| word);
        assert_eq!(word, Some("inv1".to_string()));
    }

    #[test]
    fn test_find_identifier_in_clause_after_unicode() {
        // Unicode characters on preceding lines shouldn't affect column positions
        let text = "MACHINE test\nINVARIANTS\n    @inv1 x ∈ ℕ\nVARIABLES\n    count\nEND";

        let pos = find_identifier_in_clause(text, "VARIABLES", "count", FULL);
        assert!(pos.is_some());
        let pos = pos.unwrap();
        assert_eq!(pos.line, 4);
        assert_eq!(pos.character, 4); // BMP `∈`/`ℕ` are one UTF-16 unit, and only on a prior line
    }

    #[test]
    fn test_find_identifier_in_clause_reports_utf16_after_astral() {
        // An astral character (`𝔹`, U+1D539) on the identifier's own line is two
        // UTF-16 code units but a single `char`. LSP columns are UTF-16, so the
        // reported column must skip the surrogate pair, not the char count. (`𝔹`
        // is code here, not a comment, so masking leaves it in place.) All four
        // local finders convert with the same `char_col_to_utf16(line, col)`.
        let text = "MACHINE test\nVARIABLES\n    𝔹 count\nEND";

        let pos = find_identifier_in_clause(text, "VARIABLES", "count", FULL).unwrap();
        assert_eq!(pos.line, 2);
        // 4 spaces + `𝔹` (2 units) + 1 space = column 7, not the char index 6.
        assert_eq!(pos.character, 7);
    }

    #[test]
    fn test_definition_cache_invalidation() {
        let provider = DefinitionProvider::new();
        let source = "MACHINE test\nVARIABLES\n    count\nEND";

        provider.update_definitions("file:///test.eventb".to_string(), source);
        assert!(
            provider
                .definition_cache
                .contains_key("file:///test.eventb")
        );

        // Update with invalid syntax
        provider.update_definitions("file:///test.eventb".to_string(), "INVALID SYNTAX");
        assert!(
            !provider
                .definition_cache
                .contains_key("file:///test.eventb")
        );
    }

    /// Helper to set up a provider with cross-ref and document managers, registering
    /// a context document so cross-file resolution works.
    fn setup_cross_file_provider(
        context_uri: &str,
        context_source: &str,
    ) -> (
        DefinitionProvider,
        Arc<CrossReferenceManager>,
        Arc<DocumentManager>,
    ) {
        let crm = Arc::new(CrossReferenceManager::new());
        let dm = Arc::new(DocumentManager::new());

        // Register context in cross-reference manager
        crm.update_component(context_uri.to_string(), context_source);

        // Open context document in document manager
        let url = Url::parse(context_uri).unwrap();
        dm.open(url, "rossi".to_string(), 1, context_source.to_string());

        let mut provider = DefinitionProvider::new();
        provider.set_cross_reference_manager(Arc::clone(&crm));
        provider.set_document_manager(Arc::clone(&dm));

        (provider, crm, dm)
    }

    #[test]
    fn test_cross_file_constant_definition() {
        let context_source =
            "CONTEXT counter_ctx\nCONSTANTS\n    max_value\nAXIOMS\n    @axm1 max_value ∈ ℕ\nEND";
        let machine_source = "MACHINE counter\nSEES\n    counter_ctx\nVARIABLES\n    count\nINVARIANTS\n    @inv1 count ≤ max_value\nEND";

        let (provider, _crm, _dm) = setup_multi_component_provider(&[
            ("file:///counter_ctx.eventb", context_source),
            ("file:///counter.eventb", machine_source),
        ]);

        // Update the machine definitions — this should also resolve cross-file defs
        provider.update_definitions("file:///counter.eventb".to_string(), machine_source);

        let cache = provider
            .definition_cache
            .get("file:///counter.eventb")
            .unwrap();

        // Should find max_value from the context
        let max_def = cache.find_definition("max_value");
        assert!(
            max_def.is_some(),
            "max_value should be resolved from context"
        );
        let max_def = max_def.unwrap();
        assert_eq!(max_def.kind, SymbolKind::Constant);
        assert_eq!(max_def.location.uri.as_str(), "file:///counter_ctx.eventb");
    }

    #[test]
    fn test_cross_file_set_definition() {
        let context_source = "CONTEXT types_ctx\nSETS\n    STATUS\nEND";
        let machine_source = "MACHINE sys\nSEES\n    types_ctx\nVARIABLES\n    state\nINVARIANTS\n    @inv1 state ∈ STATUS\nEND";

        let (provider, _crm, _dm) = setup_multi_component_provider(&[
            ("file:///types_ctx.eventb", context_source),
            ("file:///sys.eventb", machine_source),
        ]);

        provider.update_definitions("file:///sys.eventb".to_string(), machine_source);

        let cache = provider.definition_cache.get("file:///sys.eventb").unwrap();

        let set_def = cache.find_definition("STATUS");
        assert!(set_def.is_some(), "STATUS should be resolved from context");
        let set_def = set_def.unwrap();
        assert_eq!(set_def.kind, SymbolKind::Set);
        assert_eq!(set_def.location.uri.as_str(), "file:///types_ctx.eventb");
    }

    #[test]
    fn test_cross_file_extends_definition() {
        let parent_source = "CONTEXT base_ctx\nCONSTANTS\n    base_const\nEND";
        let child_source =
            "CONTEXT child_ctx\nEXTENDS\n    base_ctx\nCONSTANTS\n    child_const\nEND";

        let (provider, _crm, _dm) = setup_multi_component_provider(&[
            ("file:///base_ctx.eventb", parent_source),
            ("file:///child_ctx.eventb", child_source),
        ]);

        provider.update_definitions("file:///child_ctx.eventb".to_string(), child_source);

        let cache = provider
            .definition_cache
            .get("file:///child_ctx.eventb")
            .unwrap();

        // Should find base_const from the parent context
        let base_def = cache.find_definition("base_const");
        assert!(
            base_def.is_some(),
            "base_const should be resolved from parent context"
        );
        let base_def = base_def.unwrap();
        assert_eq!(base_def.kind, SymbolKind::Constant);
        assert_eq!(base_def.location.uri.as_str(), "file:///base_ctx.eventb");

        // Should also find child_const locally
        let child_def = cache.find_definition("child_const");
        assert!(child_def.is_some(), "child_const should be found locally");
        assert_eq!(child_def.unwrap().kind, SymbolKind::Constant);
    }

    #[test]
    fn test_local_definition_priority() {
        // If a machine has a variable with the same name as a context constant,
        // the local variable should take priority
        let context_source = "CONTEXT ctx\nCONSTANTS\n    x\nEND";
        let machine_source = "MACHINE m\nSEES\n    ctx\nVARIABLES\n    x\nEND";

        let (provider, _crm, _dm) = setup_cross_file_provider("file:///ctx.eventb", context_source);

        provider.update_definitions("file:///m.eventb".to_string(), machine_source);

        let cache = provider.definition_cache.get("file:///m.eventb").unwrap();

        // find_definition should prefer the local Variable over the cross-file Constant
        let x_def = cache.find_definition("x");
        assert!(x_def.is_some());
        let x_def = x_def.unwrap();
        assert_eq!(x_def.kind, SymbolKind::Variable);
        assert_eq!(x_def.location.uri.as_str(), "file:///m.eventb");
    }

    /// Helper to register multiple components and return a configured provider
    fn setup_multi_component_provider(
        components: &[(&str, &str)],
    ) -> (
        DefinitionProvider,
        Arc<CrossReferenceManager>,
        Arc<DocumentManager>,
    ) {
        let crm = Arc::new(CrossReferenceManager::new());
        let dm = Arc::new(DocumentManager::new());

        for (uri, source) in components {
            crm.update_component(uri.to_string(), source);
            let url = Url::parse(uri).unwrap();
            dm.open(url, "rossi".to_string(), 1, source.to_string());
        }

        let mut provider = DefinitionProvider::new();
        provider.set_cross_reference_manager(Arc::clone(&crm));
        provider.set_document_manager(Arc::clone(&dm));

        (provider, crm, dm)
    }

    #[test]
    fn test_refines_variable_definition() {
        let abstract_source = "MACHINE abstract_mch\nVARIABLES\n    abstract_state\nINVARIANTS\n    @inv1 abstract_state ∈ ℕ\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        abstract_state := 0\n    END\nEND";
        let concrete_source = "MACHINE concrete_mch\nREFINES\n    abstract_mch\nVARIABLES\n    concrete_state\nINVARIANTS\n    @inv1 abstract_state = concrete_state\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        concrete_state := 0\n    END\nEND";

        let (provider, _crm, _dm) = setup_multi_component_provider(&[
            ("file:///abstract_mch.eventb", abstract_source),
            ("file:///concrete_mch.eventb", concrete_source),
        ]);

        provider.update_definitions("file:///concrete_mch.eventb".to_string(), concrete_source);

        let cache = provider
            .definition_cache
            .get("file:///concrete_mch.eventb")
            .unwrap();

        // Should find abstract_state from the refined machine
        let abs_def = cache.find_definition("abstract_state");
        assert!(
            abs_def.is_some(),
            "abstract_state should be resolved from refined machine"
        );
        let abs_def = abs_def.unwrap();
        assert_eq!(abs_def.kind, SymbolKind::Variable);
        assert_eq!(abs_def.location.uri.as_str(), "file:///abstract_mch.eventb");
    }

    #[test]
    fn test_refines_event_definition() {
        let abstract_source = "MACHINE abstract_mch\nVARIABLES\n    state\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        state := 0\n    END\n    EVENT update\n    THEN\n        state := state + 1\n    END\nEND";
        let concrete_source = "MACHINE concrete_mch\nREFINES\n    abstract_mch\nVARIABLES\n    state\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        state := 0\n    END\n    EVENT update_v2\n    REFINES update\n    THEN\n        state := state + 1\n    END\nEND";

        let (provider, _crm, _dm) = setup_multi_component_provider(&[
            ("file:///abstract_mch.eventb", abstract_source),
            ("file:///concrete_mch.eventb", concrete_source),
        ]);

        provider.update_definitions("file:///concrete_mch.eventb".to_string(), concrete_source);

        let cache = provider
            .definition_cache
            .get("file:///concrete_mch.eventb")
            .unwrap();

        // Should find 'update' event from the abstract machine
        let evt_def = cache.find_definition("update");
        assert!(
            evt_def.is_some(),
            "update event should be resolved from refined machine"
        );
        let evt_def = evt_def.unwrap();
        assert_eq!(evt_def.kind, SymbolKind::Event);
        assert_eq!(evt_def.location.uri.as_str(), "file:///abstract_mch.eventb");
    }

    #[test]
    fn test_refines_transitive_sees() {
        // Machine B refines Machine A which SEES Context C
        let ctx_source = "CONTEXT ctx\nCONSTANTS\n    max_val\nEND";
        let abstract_source = "MACHINE abstract_mch\nSEES\n    ctx\nVARIABLES\n    state\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        state := 0\n    END\nEND";
        let concrete_source = "MACHINE concrete_mch\nREFINES\n    abstract_mch\nVARIABLES\n    state\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        state := 0\n    END\nEND";

        let (provider, _crm, _dm) = setup_multi_component_provider(&[
            ("file:///ctx.eventb", ctx_source),
            ("file:///abstract_mch.eventb", abstract_source),
            ("file:///concrete_mch.eventb", concrete_source),
        ]);

        provider.update_definitions("file:///concrete_mch.eventb".to_string(), concrete_source);

        let cache = provider
            .definition_cache
            .get("file:///concrete_mch.eventb")
            .unwrap();

        // Should find max_val from the context that the abstract machine SEES
        let const_def = cache.find_definition("max_val");
        assert!(
            const_def.is_some(),
            "max_val should be resolved transitively via REFINES → SEES"
        );
        let const_def = const_def.unwrap();
        assert_eq!(const_def.kind, SymbolKind::Constant);
        assert_eq!(const_def.location.uri.as_str(), "file:///ctx.eventb");
    }

    #[test]
    fn test_refines_local_variable_priority() {
        // Local variable should shadow same-named variable from refined machine
        let abstract_source = "MACHINE abstract_mch\nVARIABLES\n    state\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        state := 0\n    END\nEND";
        let concrete_source = "MACHINE concrete_mch\nREFINES\n    abstract_mch\nVARIABLES\n    state\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        state := 0\n    END\nEND";

        let (provider, _crm, _dm) = setup_multi_component_provider(&[
            ("file:///abstract_mch.eventb", abstract_source),
            ("file:///concrete_mch.eventb", concrete_source),
        ]);

        provider.update_definitions("file:///concrete_mch.eventb".to_string(), concrete_source);

        let cache = provider
            .definition_cache
            .get("file:///concrete_mch.eventb")
            .unwrap();

        // find_definition should prefer the local variable
        let state_def = cache.find_definition("state");
        assert!(state_def.is_some());
        let state_def = state_def.unwrap();
        assert_eq!(state_def.kind, SymbolKind::Variable);
        assert_eq!(
            state_def.location.uri.as_str(),
            "file:///concrete_mch.eventb"
        );
    }

    /// Goto-definition params at a 0-indexed `(line, character)` in `uri`.
    fn goto_params(uri: &str, line: u32, character: u32) -> GotoDefinitionParams {
        GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: Url::parse(uri).unwrap(),
                },
                position: Position::new(line, character),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        }
    }

    /// Resolve the cross-file target under `(line, character)` and assert it
    /// lands on `uri`'s `expected` range (a single document holds both the
    /// machine and the context it sees, as in `base-model.eventb`).
    fn assert_sees_target(source: &str, line: u32, character: u32, expected: Range) {
        let uri = "file:///model.eventb";
        let (provider, _crm, _dm) = setup_multi_component_provider(&[(uri, source)]);
        provider.update_definitions(uri.to_string(), source);

        let response = provider
            .goto_definition(&goto_params(uri, line, character), source)
            .expect("SEES target should resolve to the context definition");
        let location = match response {
            GotoDefinitionResponse::Scalar(location) => location,
            other => panic!("expected a scalar location, got {other:?}"),
        };
        assert_eq!(location.uri.as_str(), uri);
        assert_eq!(location.range, expected);
    }

    // `C1` on `context C1` spans cols 8..10 in every casing variant below.
    const C1_NAME: Range = Range {
        start: Position {
            line: 0,
            character: 8,
        },
        end: Position {
            line: 0,
            character: 10,
        },
    };

    #[test]
    fn goto_definition_lowercase_sees_resolves_same_file_context() {
        // The reported bug: lowercase keywords (as in base-model.eventb). The
        // `C1` in `sees C1` (line 6, col 5) must jump to `context C1` (line 0).
        let source = "context C1\nsets\n    S1\nend\n\nmachine M1\nsees C1\nvariables\n    v\nend";
        assert_sees_target(source, 6, 5, C1_NAME);
    }

    #[test]
    fn goto_definition_mixed_case_sees_resolves_same_file_context() {
        // Mixed-case keywords are equally valid per the grammar's `^"sees"`.
        let source = "Context C1\nSets\n    S1\nEnd\n\nMachine M1\nSees C1\nVariables\n    v\nEnd";
        assert_sees_target(source, 6, 5, C1_NAME);
    }

    #[test]
    fn goto_definition_uppercase_sees_still_resolves() {
        // Regression guard: the canonical UPPERCASE spelling keeps working.
        let source = "CONTEXT C1\nSETS\n    S1\nEND\n\nMACHINE M1\nSEES C1\nVARIABLES\n    v\nEND";
        assert_sees_target(source, 6, 5, C1_NAME);
    }
}
