//! Workspace-wide symbol search functionality
//!
//! This module provides workspace symbol search across all Event-B files in a workspace.
//! It maintains an index of all symbols (variables, constants, sets, events) and supports
//! fuzzy search for quick navigation.

use crate::lsp_types::*;
use crate::position::offset_to_position;
use dashmap::DashMap;
use rossi::ast::*;
use std::sync::Arc;
use tracing::debug;

/// Information about a symbol in the workspace
#[derive(Debug, Clone)]
struct SymbolEntry {
    /// Symbol name
    name: String,
    /// Symbol kind (variable, constant, etc.)
    kind: SymbolKind,
    /// Container name (e.g., machine/context name)
    container: String,
    /// Location in the document
    location: Location,
}

/// Provider for workspace-wide symbol search
pub struct WorkspaceSymbolProvider {
    /// Index of all symbols across all documents
    /// Key: document URI, Value: list of symbols in that document
    symbol_index: Arc<DashMap<String, Vec<SymbolEntry>>>,
}

impl Default for WorkspaceSymbolProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceSymbolProvider {
    /// Create a new workspace symbol provider
    pub fn new() -> Self {
        Self {
            symbol_index: Arc::new(DashMap::new()),
        }
    }

    /// Update symbol index for a specific document
    ///
    /// This should be called whenever a document is opened or changed to keep
    /// the workspace symbol index up-to-date.
    pub fn update_symbols(&self, uri: String, text: &str) {
        debug!("Updating workspace symbols for: {}", uri);

        // Parse with error recovery (via the shared helper) so a local syntax
        // error does not drop every symbol in the file — the healthy components
        // keep their spans and stay indexed.
        let components = crate::component_util::parse_all(text);
        if components.is_empty() {
            // Nothing recovered — remove any stale symbols for this document.
            self.symbol_index.remove(&uri);
            return;
        }

        // Extract symbols from every component in the document
        let symbols = components
            .iter()
            .flat_map(|component| self.extract_symbols_from_component(component, &uri, text))
            .collect();

        // Update index
        self.symbol_index.insert(uri, symbols);
    }

    /// Remove symbols for a document (when it's closed)
    pub fn remove_document(&self, uri: &str) {
        debug!("Removing workspace symbols for: {}", uri);
        self.symbol_index.remove(uri);
    }

    /// Search for symbols matching the query
    ///
    /// Returns a list of symbols that match the query string. The search is
    /// case-insensitive and supports substring matching.
    pub fn search(&self, query: &str) -> Vec<SymbolInformation> {
        let query_lower = query.to_lowercase();
        let mut results = Vec::new();

        // Search through all indexed documents
        for entry in self.symbol_index.iter() {
            for symbol in entry.value() {
                // Match against symbol name (case-insensitive substring match)
                if symbol.name.to_lowercase().contains(&query_lower) {
                    results.push(SymbolInformation {
                        name: symbol.name.clone(),
                        kind: symbol.kind,
                        tags: None,
                        #[allow(deprecated)]
                        deprecated: None,
                        location: symbol.location.clone(),
                        container_name: Some(symbol.container.clone()),
                    });
                }
            }
        }

        debug!(
            "Workspace symbol search for '{}' returned {} results",
            query,
            results.len()
        );

        results
    }

    /// Extract all symbols from a component, locating each at the span the
    /// parser recorded for its declared name. Spans are absolute offsets into
    /// `text`, so symbols land in the right component of a multi-component
    /// document without bounding the search to a line window, and a name
    /// spelled in a comment is never indexed (the parser does not tokenise it).
    fn extract_symbols_from_component(
        &self,
        component: &Component,
        uri: &str,
        text: &str,
    ) -> Vec<SymbolEntry> {
        let mut symbols = Vec::new();

        let entry = |name: &str, kind: SymbolKind, container: &str, span: Option<Span>| {
            Some(SymbolEntry {
                name: name.to_string(),
                kind,
                container: container.to_string(),
                location: self.locate_at(uri, text, span?.start)?,
            })
        };

        match component {
            Component::Context(ctx) => {
                symbols.extend(
                    ctx.sets
                        .iter()
                        .filter_map(|s| entry(s.name(), SymbolKind::ENUM, &ctx.name, s.span())),
                );
                symbols.extend(
                    ctx.constants
                        .iter()
                        .filter_map(|c| entry(&c.name, SymbolKind::CONSTANT, &ctx.name, c.span)),
                );
            }
            Component::Machine(mch) => {
                symbols.extend(
                    mch.variables
                        .iter()
                        .filter_map(|v| entry(&v.name, SymbolKind::VARIABLE, &mch.name, v.span)),
                );
                symbols.extend(
                    mch.events
                        .iter()
                        .filter_map(|e| entry(&e.name, SymbolKind::EVENT, &mch.name, e.name_span)),
                );
                if let Some(init) = &mch.initialisation {
                    symbols.extend(entry(
                        "INITIALISATION",
                        SymbolKind::EVENT,
                        &mch.name,
                        init.name_span,
                    ));
                }
            }
        }

        debug!(
            "Extracted {} symbols from {} ({})",
            symbols.len(),
            component.name(),
            uri
        );

        symbols
    }

    /// Wrap the position at byte offset `start` in `text` into a zero-width
    /// [`Location`] in `uri`.
    fn locate_at(&self, uri: &str, text: &str, start: usize) -> Option<Location> {
        let pos = offset_to_position(text, start);
        Some(Location::new(Url::parse(uri).ok()?, Range::new(pos, pos)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_symbols_context() {
        let provider = WorkspaceSymbolProvider::new();

        let source = r#"
CONTEXT counter
SETS
    STATUS
    PRIORITY
CONSTANTS
    max_count
AXIOMS
    @axm1 max_count ∈ ℕ
    @axm2 max_count = 100
END
"#;

        provider.update_symbols("file:///test.eventb".to_string(), source);

        // Search for STATUS set
        let results = provider.search("STATUS");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "STATUS");
        assert_eq!(results[0].kind, SymbolKind::ENUM);
        assert_eq!(results[0].container_name, Some("counter".to_string()));

        // Search for PRIORITY set
        let results = provider.search("PRIORITY");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "PRIORITY");
        assert_eq!(results[0].kind, SymbolKind::ENUM);

        // Search for max_count constant
        let results = provider.search("max_count");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "max_count");
        assert_eq!(results[0].kind, SymbolKind::CONSTANT);

        // Case-insensitive search
        let results = provider.search("status");
        assert_eq!(results.len(), 1);

        // Partial match (matches both PRIORITY and STATUS)
        let results = provider.search("T");
        assert!(results.len() >= 2);
    }

    #[test]
    fn test_workspace_symbols_machine() {
        let provider = WorkspaceSymbolProvider::new();

        let source = r#"
MACHINE counter_machine
VARIABLES
    count
    active
EVENTS
    EVENT INITIALISATION
    THEN
        count := 0
    END

    EVENT increment
    WHERE
        count < 10
    THEN
        count := count + 1
    END
END
"#;

        provider.update_symbols("file:///machine.eventb".to_string(), source);

        // Search for variable
        let results = provider.search("count");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "count");
        assert_eq!(results[0].kind, SymbolKind::VARIABLE);

        // Search for event
        let results = provider.search("increment");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "increment");
        assert_eq!(results[0].kind, SymbolKind::EVENT);

        // Search for INITIALISATION
        let results = provider.search("INITIALISATION");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "INITIALISATION");
    }

    #[test]
    fn test_workspace_symbols_lowercase_keywords() {
        let provider = WorkspaceSymbolProvider::new();

        // Lowercase keywords (Camille style). The set named STATUS — a
        // contextual keyword — is followed by another set, exercising the
        // declaration-scan carve-out that keeps STATUS a name, not a boundary.
        let source = r#"
context types
sets
    STATUS
    Colours
constants
    ceiling
axioms
    @axm1 ceiling ∈ ℕ
end

machine counter
sees types
variables
    tally
events
    event initialisation
    then
        tally := 0
    end

    event increment
    where
        tally < ceiling
    then
        tally := tally + 1
    end
end
"#;

        provider.update_symbols("file:///model.eventb".to_string(), source);

        for (name, kind) in [
            ("STATUS", SymbolKind::ENUM),
            ("Colours", SymbolKind::ENUM),
            ("ceiling", SymbolKind::CONSTANT),
            ("tally", SymbolKind::VARIABLE),
            ("increment", SymbolKind::EVENT),
            ("INITIALISATION", SymbolKind::EVENT),
        ] {
            let results = provider.search(name);
            assert_eq!(results.len(), 1, "expected exactly one `{name}` symbol");
            assert_eq!(results[0].name, name);
            assert_eq!(results[0].kind, kind);
        }
    }

    #[test]
    fn test_workspace_symbols_multiple_documents() {
        let provider = WorkspaceSymbolProvider::new();

        let ctx = r#"
CONTEXT ctx
SETS
    DATA
CONSTANTS
    value
END
"#;

        let mch = r#"
MACHINE mch
VARIABLES
    data
    value
END
"#;

        provider.update_symbols("file:///ctx.eventb".to_string(), ctx);
        provider.update_symbols("file:///mch.eventb".to_string(), mch);

        // Search for 'value' - should find both constant and variable
        let results = provider.search("value");
        assert_eq!(results.len(), 2);

        // Verify one is constant and one is variable
        let kinds: Vec<_> = results.iter().map(|r| r.kind).collect();
        assert!(kinds.contains(&SymbolKind::CONSTANT));
        assert!(kinds.contains(&SymbolKind::VARIABLE));
    }

    #[test]
    fn test_remove_document() {
        let provider = WorkspaceSymbolProvider::new();

        let source = r#"
CONTEXT test
CONSTANTS
    foo
END
"#;

        let uri = "file:///test.eventb".to_string();
        provider.update_symbols(uri.clone(), source);

        // Should find the symbol
        let results = provider.search("foo");
        assert_eq!(results.len(), 1);

        // Remove the document
        provider.remove_document(&uri);

        // Should no longer find the symbol
        let results = provider.search("foo");
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_empty_query() {
        let provider = WorkspaceSymbolProvider::new();

        let source = r#"
CONTEXT test
CONSTANTS
    alpha
    beta
END
"#;

        provider.update_symbols("file:///test.eventb".to_string(), source);

        // Empty query should match everything
        let results = provider.search("");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_no_match() {
        let provider = WorkspaceSymbolProvider::new();

        let source = r#"
CONTEXT test
CONSTANTS
    foo
END
"#;

        provider.update_symbols("file:///test.eventb".to_string(), source);

        // Search for non-existent symbol
        let results = provider.search("nonexistent");
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_update_symbols_on_change() {
        let provider = WorkspaceSymbolProvider::new();
        let uri = "file:///test.eventb".to_string();

        let source1 = r#"
CONTEXT test
CONSTANTS
    old_symbol
END
"#;

        provider.update_symbols(uri.clone(), source1);
        let results = provider.search("old_symbol");
        assert_eq!(results.len(), 1);

        // Update with new content
        let source2 = r#"
CONTEXT test
CONSTANTS
    new_symbol
END
"#;

        provider.update_symbols(uri, source2);

        // Old symbol should be gone
        let results = provider.search("old_symbol");
        assert_eq!(results.len(), 0);

        // New symbol should be found
        let results = provider.search("new_symbol");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn symbols_survive_a_broken_sibling_component() {
        // A broken axiom in C0 must not drop M0's variable from the index:
        // recovery indexes the healthy machine region (which keeps its spans).
        let provider = WorkspaceSymbolProvider::new();
        let source =
            "CONTEXT C0\nAXIOMS\n    @a k ∈\nEND\n\nMACHINE M0\nVARIABLES\n    counter\nEND\n";
        provider.update_symbols("file:///m.eventb".to_string(), source);

        let results = provider.search("counter");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, SymbolKind::VARIABLE);
    }
}
