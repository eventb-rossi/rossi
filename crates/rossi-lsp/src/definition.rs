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
use rossi::Component;
use std::collections::HashMap;
use std::sync::Arc;

use crate::component_loader::ComponentLoader;
use crate::component_util::component_line_window;
use crate::cross_references::CrossReferenceManager;
use crate::document::DocumentManager;
use crate::position::{offset_to_position, span_to_range, utf16_len};
use crate::references::component_reference_clause;
use crate::symbols::SymbolKind;

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

    /// Update the definition cache for a document, parsing `text` first.
    ///
    /// For callers that hold only the source text (the unit tests). The edit
    /// path calls [`Self::index_components`] with the document manager's stored
    /// parse, so the file is not parsed again to refresh this cache.
    ///
    /// Parses with error recovery so a local syntax error does not drop every
    /// definition in the file: the healthy components keep their real spans and
    /// still resolve. A recovered component's own declarations only become
    /// navigable once recovery records their spans.
    pub fn update_definitions(&self, uri: String, text: &str) {
        let components = crate::component_util::parse_all(text);
        self.index_components(uri, &components, text);
    }

    /// Refresh the definition cache from a document's already-parsed components.
    /// `text` must be the source those component spans index into.
    pub fn index_components(&self, uri: String, components: &[Component], text: &str) {
        if components.is_empty() {
            // Nothing recovered — drop any stale definitions for this document.
            self.definition_cache.remove(&uri);
            return;
        }

        let mut ctx = DefinitionContext::new();
        // Sibling components of a merged file typically see the same
        // contexts/machines — extract each visible component once per
        // update, not once per sibling. The loader parses each visible file at
        // most once across the whole update and reuses open documents' parses.
        let mut cross_cache = HashMap::new();
        let loader = ComponentLoader::optional(
            self.cross_ref_manager.as_deref(),
            self.document_manager.as_deref(),
        );
        for component in components {
            ctx.definitions.extend(
                self.extract_definitions(component, text, &uri, &mut cross_cache, loader.as_ref())
                    .definitions,
            );
        }
        self.definition_cache.insert(uri, ctx);
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

        // Load the target component (open document or disk, parsed at most once).
        let loader = ComponentLoader::new(cross_ref_manager, self.document_manager.as_deref());
        let loaded = loader.load(word)?;

        // Locate the component's name via the parser (the source of truth) and
        // map its span to a range — exact for any casing or spacing, unlike a
        // textual keyword scan.
        let name_span = loaded.component().name_span()?;
        let range = span_to_range(&name_span, loaded.text());

        Some(Location::new(loaded.uri().clone(), range))
    }

    /// Extract all definitions visible from a component: those declared locally
    /// plus those reachable through SEES/EXTENDS/REFINES.
    fn extract_definitions(
        &self,
        component: &Component,
        text: &str,
        uri_str: &str,
        cross_cache: &mut HashMap<String, Vec<DefinitionInfo>>,
        loader: Option<&ComponentLoader>,
    ) -> DefinitionContext {
        let window = component_line_window(component, text);
        let mut ctx = DefinitionContext::new();
        ctx.definitions = self.extract_local_definitions(component, text, uri_str, window);

        // Add cross-file definitions from SEES/EXTENDS/REFINES contexts and
        // machines. They are scoped to the REQUESTING component's lines: that
        // is where this component's visibility applies, so in a
        // multi-component document the cursor picks the resolution belonging
        // to the component it sits in.
        if let Some(loader) = loader {
            let mut cross = self.resolve_cross_file_definitions(component, loader, cross_cache);
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

        // The parser records each declared name's span, so the definition site
        // is read straight from the AST. This is exact for any casing or spacing
        // and never matches a name spelled in a comment (the parser does not
        // tokenise comment text), so no source re-scan or comment masking is
        // needed. A `None` span only occurs for components built without
        // location info (Rodin XML import), which never reach this provider.
        let def_at = |name: &str, kind: SymbolKind, start: usize| {
            let pos = offset_to_position(text, start);
            DefinitionInfo {
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
            }
        };

        match component {
            Component::Context(context) => {
                for set in &context.sets {
                    if let Some(span) = set.span() {
                        definitions.push(def_at(set.name(), SymbolKind::Set, span.start));
                    }
                }

                for constant in &context.constants {
                    if let Some(span) = constant.span {
                        definitions.push(def_at(&constant.name, SymbolKind::Constant, span.start));
                    }
                }
            }
            Component::Machine(machine) => {
                for variable in &machine.variables {
                    if let Some(span) = variable.span {
                        definitions.push(def_at(&variable.name, SymbolKind::Variable, span.start));
                    }
                }

                for event in &machine.events {
                    if let Some(span) = event.name_span {
                        definitions.push(def_at(&event.name, SymbolKind::Event, span.start));
                    }

                    // ANY-clause parameters: the AST holds exactly the declared
                    // parameters, each with its own name span.
                    for param in &event.parameters {
                        if let Some(span) = param.span {
                            definitions.push(def_at(
                                &param.name,
                                SymbolKind::Parameter,
                                span.start,
                            ));
                        }
                    }
                }

                if let Some(init) = &machine.initialisation
                    && let Some(span) = init.name_span
                {
                    definitions.push(def_at("INITIALISATION", SymbolKind::Event, span.start));
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
        loader: &ComponentLoader,
        cache: &mut HashMap<String, Vec<DefinitionInfo>>,
    ) -> Vec<DefinitionInfo> {
        let manager = loader.manager();
        let component_names = match component {
            Component::Machine(machine) => {
                let mut names = manager.refinement_chain(&machine.name);
                names.extend(manager.ordered_visible_contexts(&machine.name));
                names
            }
            Component::Context(context) => manager.ordered_extends_chain(&context.name),
        };

        let mut results = Vec::new();
        for name in component_names {
            let definitions = cache.entry(name).or_insert_with_key(|name| {
                self.extract_visible_definitions(name, loader)
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
        loader: &ComponentLoader,
    ) -> Option<Vec<DefinitionInfo>> {
        let loaded = loader.load(name)?;
        let window = component_line_window(loaded.component(), loaded.text());
        Some(self.extract_local_definitions(
            loaded.component(),
            loaded.text(),
            loaded.uri().as_str(),
            window,
        ))
    }
}

impl Default for DefinitionProvider {
    fn default() -> Self {
        Self::new()
    }
}

// Helper functions

use crate::identifier_utils::identifier_at_position;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp_types::{TextDocumentIdentifier, TextDocumentPositionParams};

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
    fn any_parameter_resolves_but_a_guard_only_name_does_not() {
        // The AST holds exactly the ANY-clause parameters, so a parameter (`q`)
        // resolves to its declaration while a name that only appears in a guard
        // (`k`) is not a definition at all — the property the old bounded text
        // scan had to enforce by hand.
        let provider = DefinitionProvider::new();
        let source = "MACHINE m\nVARIABLES\n    v\nEVENTS\n  EVENT e\n  ANY\n    q\n  WHERE\n    q > k\n  THEN\n    v := 0\n  END\nEND";
        provider.update_definitions("file:///m.eventb".to_string(), source);
        let cache = provider.definition_cache.get("file:///m.eventb").unwrap();

        let q = cache.find_definition("q").expect("q is an ANY parameter");
        assert_eq!(q.kind, SymbolKind::Parameter);
        assert_eq!(q.location.range.start, Position::new(6, 4));

        assert!(
            cache.find_definition("k").is_none(),
            "k only appears in a guard, so it is not a definition"
        );
    }

    #[test]
    fn event_name_that_is_a_substring_of_a_keyword_resolves_exactly() {
        // An event named `ent` is a substring of `EVENT`; reading the name span
        // from the AST lands on the name, never inside the keyword.
        let provider = DefinitionProvider::new();
        let source = "MACHINE m\nEVENTS\n    EVENT ent\n    END\nEND";
        provider.update_definitions("file:///m.eventb".to_string(), source);
        let cache = provider.definition_cache.get("file:///m.eventb").unwrap();

        let ent = cache.find_definition("ent").expect("event ent resolves");
        assert_eq!(ent.kind, SymbolKind::Event);
        assert_eq!(ent.location.range.start, Position::new(2, 10)); // after "    EVENT "
    }

    #[test]
    fn inline_sets_header_resolves_every_member() {
        // `SETS s1` then `s2` on the next line: both are set declarations in the
        // AST, each with its own span, so both resolve to their own positions.
        let provider = DefinitionProvider::new();
        let source = "CONTEXT c\nSETS s1\n    s2\nEND";
        provider.update_definitions("file:///c.eventb".to_string(), source);
        let cache = provider.definition_cache.get("file:///c.eventb").unwrap();

        let s1 = cache.find_definition("s1").expect("s1 resolves");
        assert_eq!(s1.kind, SymbolKind::Set);
        assert_eq!(s1.location.range.start, Position::new(1, 5)); // after "SETS "
        let s2 = cache.find_definition("s2").expect("s2 resolves");
        assert_eq!(s2.location.range.start, Position::new(2, 4));
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

    #[test]
    fn definitions_survive_a_broken_sibling_component() {
        // A broken axiom in C0 must not wipe M0's variable definition: recovery
        // parses the healthy machine region with real spans, so goto-definition
        // keeps working everywhere except inside the broken clause itself.
        let provider = DefinitionProvider::new();
        let source =
            "CONTEXT C0\nAXIOMS\n    @a k ∈\nEND\n\nMACHINE M0\nVARIABLES\n    counter\nEND\n";
        provider.update_definitions("file:///m.eventb".to_string(), source);

        let cache = provider.definition_cache.get("file:///m.eventb").unwrap();
        let def = cache
            .find_definition("counter")
            .expect("counter resolves despite C0's broken axiom");
        assert_eq!(def.kind, SymbolKind::Variable);
        assert_eq!(def.location.range.start, Position::new(7, 4)); // after "VARIABLES\n    "
    }

    #[test]
    fn definition_resolves_inside_a_broken_component() {
        // The component under the cursor is itself broken (trailing `∈`), yet
        // its own variable still resolves — recovery records the declaration's
        // span (L2), so goto works right next to the error.
        let provider = DefinitionProvider::new();
        let source = "MACHINE m\nVARIABLES\n    counter\nINVARIANTS\n    @i counter ∈\nEND\n";
        provider.update_definitions("file:///m.eventb".to_string(), source);

        let cache = provider.definition_cache.get("file:///m.eventb").unwrap();
        let def = cache
            .find_definition("counter")
            .expect("counter resolves despite the broken invariant");
        assert_eq!(def.kind, SymbolKind::Variable);
        assert_eq!(def.location.range.start, Position::new(2, 4));
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
