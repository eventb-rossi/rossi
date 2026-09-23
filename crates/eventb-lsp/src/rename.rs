//! Symbol rename.
//!
//! A rename rewrites what the cursor names wherever it is named. The resolver
//! go-to-definition and find-references use places the cursor, and the
//! occurrences find-references reports are rewritten, closed files included.
//! Along the refinement chain, a variable or parameter a refinement declares
//! again, and an event refining one of its own name, are the same entity and
//! rename together. A formula binder renames within its own scope, a
//! component wherever a declaration or dependency clause names it. A
//! position the resolver cannot place, such as a label, a comment or a name
//! nothing declares, is refused rather than guessed at.

use crate::lsp_types::*;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::debug;

use rossi::Component;
use rossi::ast::Span;

use crate::component_loader::ComponentLoader;
use crate::component_util::covers;
use crate::cross_references::CrossReferenceManager;
use crate::document::DocumentManager;
use crate::formula_walk;
use crate::identifier_utils;
use crate::identifier_utils::position_to_offset;
use crate::position::span_to_range;
use crate::references::{SymbolOccurrences, symbol_candidates, symbol_occurrences};
use crate::resolved_environment::ResolvedEnvironments;
use crate::symbols::{
    INITIALISATION_EVENT_NAME, Resolution, SymbolIdentity, SymbolKind, abstract_event_identity,
    abstract_parameter_identities, resolve_cursor,
};

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

/// What a rename at the cursor rewrites.
enum Target {
    /// A component name, renamed wherever a declaration or dependency clause
    /// names it.
    Component(String),
    /// A formula binder: its declaration and the uses it binds, all in the
    /// cursor document, whose text they index.
    Binder {
        name: String,
        text: String,
        spans: Vec<Span>,
    },
    /// A symbol, with the declarations renamed along with it.
    Symbol {
        symbol: SymbolIdentity,
        occurrences: Vec<SymbolOccurrences>,
    },
}

impl Target {
    /// Whether the new name may be hyphenated: a structural name (a
    /// component or an event, as Rodin labels and file names) may be, a
    /// mathematical identifier may not (kernel_lang §2.2).
    fn is_structural(&self) -> bool {
        match self {
            Target::Component(_) => true,
            Target::Binder { .. } => false,
            Target::Symbol { symbol, .. } => symbol.kind == SymbolKind::Event,
        }
    }

    /// Whether an occurrence this rewrites sits at `offset` in the cursor
    /// document `uri`, so the cursor is on a name rather than on a label or
    /// comment spelled like it.
    fn is_at(&self, uri: &Uri, offset: usize) -> bool {
        match self {
            // Resolved from the cursor's own dependency clause or header.
            Target::Component(_) => true,
            Target::Binder { spans, .. } => spans.iter().any(|span| covers(*span, offset)),
            Target::Symbol { occurrences, .. } => occurrences
                .iter()
                .filter(|occurrence| occurrence.loaded.uri() == uri)
                .any(|occurrence| occurrence.spans.iter().any(|span| covers(*span, offset))),
        }
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

    /// Prepare for rename: the range of the name at the cursor, when a rename
    /// there would rewrite it.
    pub fn prepare_rename(&self, params: &TextDocumentPositionParams, text: &str) -> Option<Range> {
        let (range, _) = self.target_at(&params.text_document.uri, params.position, text)?;
        Some(range)
    }

    /// Perform the rename operation
    pub fn rename(&self, params: &RenameParams, text: &str) -> Option<WorkspaceEdit> {
        let position = params.text_document_position.position;
        let uri = &params.text_document_position.text_document.uri;
        let new_name = &params.new_name;

        // Check if new name is a keyword
        if is_keyword(new_name) {
            debug!("Cannot rename to keyword: '{}'", new_name);
            return None;
        }

        let (_, target) = self.target_at(uri, position, text)?;
        if !is_valid_new_name(new_name, target.is_structural()) {
            debug!("Invalid new name: '{}'", new_name);
            return None;
        }

        let changes = match &target {
            Target::Component(name) => {
                debug!("Renaming component '{}' across workspace", name);
                let mut changes = HashMap::new();
                self.rename_across_workspace(name, new_name, &mut changes);
                changes
            }
            Target::Binder { name, text, spans } => {
                occurrence_edits([(uri, text.as_str(), spans.as_slice())], name, new_name)
            }
            Target::Symbol {
                symbol,
                occurrences,
            } => occurrence_edits(
                occurrences.iter().map(|occurrence| {
                    (
                        occurrence.loaded.uri(),
                        occurrence.loaded.text(),
                        occurrence.spans.as_slice(),
                    )
                }),
                &symbol.name,
                new_name,
            ),
        };
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

    /// The name at `position` and what renaming it rewrites, or `None` when
    /// the resolver places nothing renameable there: a keyword, a label, a
    /// comment, a name nothing declares, or INITIALISATION.
    fn target_at(&self, uri: &Uri, position: Position, text: &str) -> Option<(Range, Target)> {
        let manager = self.cross_ref_manager.as_ref()?;
        // Prefer the open document's stored parse: the served text, the cursor
        // offset, and the AST then index one consistent snapshot, and the cursor
        // file is not re-parsed for the rename. Fall back to the handler text
        // when the document is not open.
        let cursor = self
            .document_manager
            .as_ref()
            .and_then(|dm| dm.parse_result(uri));
        let text = cursor.as_deref().map_or(text, |parsed| parsed.text());
        let masked = rossi::comments::mask_comments_chars(text);
        let (identifier, range) = identifier_utils::identifier_at_position(&masked, position)?;
        let offset = position_to_offset(text, position)?;
        debug!("Resolving '{}' at {:?} for rename", identifier, position);

        let loader = ComponentLoader::new(manager, self.document_manager.as_deref());
        let target = match resolve_cursor(
            text,
            &masked,
            position,
            &identifier,
            &loader,
            cursor.as_deref(),
        )? {
            Resolution::Component(_) => Target::Component(identifier),
            Resolution::Bound(bound) => Target::Binder {
                name: identifier,
                text: text.to_string(),
                spans: bound.spans,
            },
            Resolution::Symbol(symbol) => {
                if symbol.kind == SymbolKind::Event && symbol.name == INITIALISATION_EVENT_NAME {
                    debug!("Cannot rename INITIALISATION");
                    return None;
                }
                let mut environments = ResolvedEnvironments::new();
                let occurrences = rename_class(&symbol, &loader)
                    .iter()
                    .flat_map(|member| symbol_occurrences(member, &loader, &mut environments))
                    .collect();
                Target::Symbol {
                    symbol,
                    occurrences,
                }
            }
        };
        target.is_at(uri, offset).then_some((range, target))
    }

    /// Rename a component across all workspace files
    fn rename_across_workspace(
        &self,
        old_name: &str,
        new_name: &str,
        changes: &mut HashMap<Uri, Vec<TextEdit>>,
    ) {
        let manager = match &self.cross_ref_manager {
            Some(m) => m,
            None => return,
        };

        let loader = ComponentLoader::new(manager, self.document_manager.as_deref());
        for occurrence in loader.component_occurrences(old_name) {
            changes.entry(occurrence.uri).or_default().push(TextEdit {
                range: occurrence.range,
                new_text: new_name.to_string(),
            });
        }

        for edits in changes.values_mut() {
            sort_edits_reverse(edits);
            edits.dedup_by(|a, b| a.range == b.range);
        }
    }
}

/// The symbols a rename of `start` changes together with it: the
/// declarations the refinement chain links to it as one entity, up and down.
/// A variable or parameter a refinement declares again is the one it
/// retains, and an event refining one of its own name is that event, so
/// renaming either renames both. The abstract events one event merges share
/// their parameters, so those rename together too.
fn rename_class(start: &SymbolIdentity, loader: &ComponentLoader) -> Vec<SymbolIdentity> {
    let mut class = vec![start.clone()];
    let mut next = 0;
    while let Some(member) = class.get(next).cloned() {
        next += 1;
        let mut linked = retained_parents(&member, loader);
        for name in symbol_candidates(&member, loader) {
            let Some(loaded) = loader.load(&name) else {
                continue;
            };
            linked.extend(
                local_declarations(loaded.component(), &member)
                    .into_iter()
                    .filter(|declared| retained_parents(declared, loader).contains(&member)),
            );
            if let (SymbolKind::Parameter, Component::Machine(machine)) =
                (member.kind, loaded.component())
            {
                for event in &machine.events {
                    let merged = abstract_parameter_identities(
                        loaded.component(),
                        event,
                        &member.name,
                        loader,
                    );
                    if merged.contains(&member) {
                        linked.extend(merged);
                    }
                }
            }
        }
        for symbol in linked {
            if !class.contains(&symbol) {
                class.push(symbol);
            }
        }
    }
    class
}

/// The declarations up the refinement chain that `symbol` retains: the
/// abstract machine's variable of the same name a refinement declares again,
/// the abstract event of the same name an event refines, or the abstract
/// event's parameter an event declares again.
fn retained_parents(symbol: &SymbolIdentity, loader: &ComponentLoader) -> Vec<SymbolIdentity> {
    let Some(loaded) = loader.load(&symbol.owner) else {
        return Vec::new();
    };
    let Component::Machine(machine) = loaded.component() else {
        return Vec::new();
    };
    match symbol.kind {
        SymbolKind::Variable => machine
            .refines
            .as_deref()
            .and_then(|parent| loader.load(parent))
            .map(|parent| local_declarations(parent.component(), symbol))
            .unwrap_or_default(),
        SymbolKind::Event => machine
            .events
            .iter()
            .filter(|event| event.name == symbol.name)
            .flat_map(|event| &event.refines)
            .filter(|target| target.name == symbol.name)
            .filter_map(|target| abstract_event_identity(loaded.component(), &target.name, loader))
            .collect(),
        SymbolKind::Parameter => machine
            .events
            .iter()
            .filter(|event| symbol.event.as_deref() == Some(event.name.as_str()))
            .flat_map(|event| {
                abstract_parameter_identities(loaded.component(), event, &symbol.name, loader)
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// What `component` itself declares under `like`'s name and kind.
fn local_declarations(component: &Component, like: &SymbolIdentity) -> Vec<SymbolIdentity> {
    match (component, like.kind) {
        (Component::Machine(machine), SymbolKind::Variable)
            if machine.variables.iter().any(|v| v.name == like.name) =>
        {
            vec![SymbolIdentity {
                name: like.name.clone(),
                kind: SymbolKind::Variable,
                owner: machine.name.clone(),
                event: None,
            }]
        }
        (Component::Machine(machine), SymbolKind::Event)
            if machine.events.iter().any(|e| e.name == like.name) =>
        {
            vec![SymbolIdentity::event(&like.name, &machine.name)]
        }
        (Component::Machine(machine), SymbolKind::Parameter) => machine
            .events
            .iter()
            .filter(|event| event.parameters.iter().any(|p| p.name == like.name))
            .map(|event| SymbolIdentity::parameter(&like.name, &machine.name, &event.name))
            .collect(),
        _ => Vec::new(),
    }
}

/// The edits rewriting every span of `files` to `new_name`, grouped by file,
/// each `x'` renamed at its base. Empty when a span does not slice to `name`
/// in its text: a rename that would rewrite unrelated text is refused whole.
fn occurrence_edits<'a>(
    files: impl IntoIterator<Item = (&'a Uri, &'a str, &'a [Span])>,
    name: &str,
    new_name: &str,
) -> HashMap<Uri, Vec<TextEdit>> {
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for (uri, text, spans) in files {
        if !spans
            .iter()
            .all(|span| formula_walk::span_matches(text, *span, name))
        {
            return HashMap::new();
        }
        changes
            .entry(uri.clone())
            .or_default()
            .extend(spans.iter().map(|span| TextEdit {
                range: span_to_range(&base_span(text, *span), text),
                new_text: new_name.to_string(),
            }));
    }
    for edits in changes.values_mut() {
        sort_edits_reverse(edits);
        edits.dedup_by(|a, b| a.range == b.range);
    }
    changes
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
pub(crate) fn sort_edits_reverse(edits: &mut [TextEdit]) {
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

    fn make_uri() -> Uri {
        ("file:///test.eventb").parse::<Uri>().unwrap()
    }

    fn make_position_params(line: u32, character: u32, uri: Uri) -> TextDocumentPositionParams {
        TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri },
            position: Position::new(line, character),
        }
    }

    fn make_rename_params(line: u32, character: u32, uri: Uri, new_name: String) -> RenameParams {
        RenameParams {
            text_document_position: make_position_params(line, character, uri),
            new_name,
            work_done_progress_params: WorkDoneProgressParams::default(),
        }
    }

    /// A provider over a workspace holding `source` alone, open as
    /// [`make_uri`].
    fn provider_for(source: &str) -> RenameProvider {
        workspace_provider(&[("test.eventb", source)])
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
        let uri = make_uri();

        let source = r#"
MACHINE test
VARIABLES
    count
INVARIANTS
    @inv1 count ∈ ℕ
END
"#;
        let provider = provider_for(source);

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
        let uri = make_uri();

        let source = r#"
MACHINE test
VARIABLES
    count
END
"#;
        let provider = provider_for(source);

        // Try to rename 'VARIABLES' keyword - should fail
        let params = make_position_params(2, 0, uri);
        let range = provider.prepare_rename(&params, source);

        assert!(range.is_none());
    }

    #[test]
    fn test_rename_variable() {
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
        let provider = provider_for(source);

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
        let provider = provider_for(source);

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

    /// A context whose carrier set, and a machine whose variable, event and
    /// parameter, are all spelled like structural keywords — exactly what
    /// EB028 flags and asks the user to rename.
    const KEYWORD_CONTEXT: &str = "CONTEXT c\nSETS\n    end\nAXIOMS\n    @axm1 end ≠ ∅\nEND\n";

    #[test]
    fn prepare_rename_allows_a_keyword_spelled_declaration() {
        // EB028 tells the user to rename a declared name that spells a
        // keyword; prepare_rename must let them start. The carrier set `end`
        // is on line 2, columns 4..7.
        let provider = provider_for(KEYWORD_CONTEXT);
        let params = make_position_params(2, 4, make_uri());
        assert_eq!(
            provider.prepare_rename(&params, KEYWORD_CONTEXT),
            Some(Range::new(Position::new(2, 4), Position::new(2, 7)))
        );

        // And the rename itself goes through, declaration and use alike.
        let params = make_rename_params(2, 4, make_uri(), "finish".to_string());
        let edits = provider
            .rename(&params, KEYWORD_CONTEXT)
            .expect("keyword-spelled set rename")
            .changes
            .unwrap()
            .remove(&make_uri())
            .unwrap();
        assert_eq!(
            apply(KEYWORD_CONTEXT, &edits),
            "CONTEXT c\nSETS\n    finish\nAXIOMS\n    @axm1 finish ≠ ∅\nEND\n"
        );
    }

    #[test]
    fn prepare_rename_allows_a_keyword_spelled_reference() {
        // The rename may be started from a use, not just the declaration:
        // `end` inside the axiom on line 4, columns 10..13.
        let provider = provider_for(KEYWORD_CONTEXT);
        let params = make_position_params(4, 10, make_uri());
        assert_eq!(
            provider.prepare_rename(&params, KEYWORD_CONTEXT),
            Some(Range::new(Position::new(4, 10), Position::new(4, 13)))
        );
    }

    #[test]
    fn prepare_rename_allows_keyword_spelled_machine_names() {
        // `status` is a legal variable and `then` a legal event name in
        // rossi; both are refused by the spelling-based `is_keyword`, so the
        // offset check is what admits them.
        let source = "MACHINE m\nVARIABLES\n    status\nINVARIANTS\n    @inv1 status ∈ ℕ\nEVENTS\n    EVENT then\n    ANY\n        with\n    WHERE\n        @grd1 with ∈ ℕ\n    THEN\n        @act1 status ≔ with\n    END\nEND\n";
        let provider = provider_for(source);
        for (line, character, len) in [(2, 4, 6), (6, 10, 4), (8, 8, 4)] {
            let params = make_position_params(line, character, make_uri());
            assert_eq!(
                provider.prepare_rename(&params, source),
                Some(Range::new(
                    Position::new(line, character),
                    Position::new(line, character + len)
                )),
                "line {line}"
            );
        }
    }

    #[test]
    fn prepare_rename_still_refuses_a_real_keyword() {
        // The offset decides, not the spelling: in the very file that
        // declares a set named `end`, the SETS header and the closing END
        // token are keyword tokens with no name behind them.
        let provider = provider_for(KEYWORD_CONTEXT);
        for (line, character) in [(1, 0), (5, 0)] {
            let params = make_position_params(line, character, make_uri());
            assert_eq!(
                provider.prepare_rename(&params, KEYWORD_CONTEXT),
                None,
                "line {line}"
            );
        }
    }

    #[test]
    fn test_rename_to_keyword_fails() {
        let uri = make_uri();

        let source = r#"
MACHINE test
VARIABLES
    count
END
"#;
        let provider = provider_for(source);

        // Try to rename 'count' to 'VARIABLES' - should fail
        let params = make_rename_params(3, 4, uri, "VARIABLES".to_string());
        let edit = provider.rename(&params, source);

        assert!(edit.is_none());
    }

    #[test]
    fn test_rename_hyphenated_event_to_hyphenated_name() {
        // An event is a structural name, so a hyphenated new name must be
        // allowed (issue #28).
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
        let provider = provider_for(source);
        let params = make_rename_params(2, 6, uri.clone(), "do-step2".to_string());
        let edit = provider.rename(&params, source);
        assert!(edit.is_some(), "hyphenated event rename should succeed");
        let edits = edit.unwrap().changes.unwrap();
        assert!(edits.get(&uri).is_some_and(|e| !e.is_empty()));
    }

    #[test]
    fn test_rename_to_invalid_name_fails() {
        let uri = make_uri();

        let source = r#"
MACHINE test
VARIABLES
    count
END
"#;
        let provider = provider_for(source);

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
        let uri = make_uri();

        let source = r#"
MACHINE test
VARIABLES
    count
    counter
END
"#;
        let provider = provider_for(source);

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
    fn test_rename_edits_sorted() {
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
        let provider = provider_for(source);

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
        let source = "MACHINE m\nVARIABLES\n    count // count is reset\nINVARIANTS\n    @inv1 count ∈ ℕ\nEND\n";
        let out = rename_at(source, source.find("count").unwrap(), "val");
        // The declaration and the invariant's use, not the `count` in the
        // comment.
        assert_eq!(
            out,
            "MACHINE m\nVARIABLES\n    val // count is reset\nINVARIANTS\n    @inv1 val ∈ ℕ\nEND\n"
        );
    }

    fn collision_provider(context: &str, machine: &str) -> (RenameProvider, Uri, Uri) {
        let context_uri = ("file:///c.eventb").parse::<Uri>().unwrap();
        let machine_uri = ("file:///m.eventb").parse::<Uri>().unwrap();
        let crm = Arc::new(CrossReferenceManager::new());
        crm.update_component(context_uri.as_str().to_owned(), context);
        crm.update_component(machine_uri.as_str().to_owned(), machine);
        let documents = Arc::new(DocumentManager::new());
        documents.open(context_uri.clone(), 1, context.to_string());
        documents.open(machine_uri.clone(), 1, machine.to_string());
        let mut provider = RenameProvider::new();
        provider.set_cross_reference_manager(crm);
        provider.set_document_manager(documents);
        (provider, context_uri, machine_uri)
    }

    #[test]
    fn component_rename_excludes_same_spelling_in_formulas_and_labels() {
        let context = "CONTEXT C\nCONSTANTS\n    C\nAXIOMS\n    @C C =\nEND";
        let machine = "MACHINE M\nSEES C\nVARIABLES\n    C\nINVARIANTS\n    @C C ∈\nEND";
        let (provider, context_uri, machine_uri) = collision_provider(context, machine);
        let params = make_rename_params(0, 8, context_uri.clone(), "D".to_string());
        let mut changes = provider
            .rename(&params, context)
            .expect("component rename")
            .changes
            .unwrap();

        assert_eq!(
            apply(context, changes.remove(&context_uri).as_deref().unwrap()),
            "CONTEXT D\nCONSTANTS\n    C\nAXIOMS\n    @C C =\nEND"
        );
        assert_eq!(
            apply(machine, changes.remove(&machine_uri).as_deref().unwrap()),
            "MACHINE M\nSEES D\nVARIABLES\n    C\nINVARIANTS\n    @C C ∈\nEND"
        );
    }

    #[test]
    fn keyword_spelled_component_can_be_prepared_and_renamed() {
        let context = "CONTEXT MACHINE\nEND";
        let machine = "MACHINE M\nSEES MACHINE\nEND";
        let (provider, context_uri, machine_uri) = collision_provider(context, machine);
        let prepare = make_position_params(1, 5, machine_uri.clone());
        assert_eq!(
            provider.prepare_rename(&prepare, machine),
            Some(Range::new(Position::new(1, 5), Position::new(1, 12)))
        );

        let params = make_rename_params(1, 5, machine_uri.clone(), "C".to_string());
        let mut changes = provider
            .rename(&params, machine)
            .expect("keyword-spelled component rename")
            .changes
            .unwrap();

        assert_eq!(
            apply(context, changes.remove(&context_uri).as_deref().unwrap()),
            "CONTEXT C\nEND"
        );
        assert_eq!(
            apply(machine, changes.remove(&machine_uri).as_deref().unwrap()),
            "MACHINE M\nSEES C\nEND"
        );
    }

    #[test]
    fn local_rename_does_not_become_component_rename_on_name_collision() {
        let context = "CONTEXT C\nEND";
        let machine = "MACHINE M\nSEES C\nVARIABLES\n    C\nINVARIANTS\n    @i C ∈ ℕ\nEND";
        let (provider, context_uri, machine_uri) = collision_provider(context, machine);
        let params = make_rename_params(3, 4, machine_uri.clone(), "x".to_string());
        let mut changes = provider
            .rename(&params, machine)
            .expect("local rename")
            .changes
            .unwrap();

        assert!(!changes.contains_key(&context_uri));
        assert_eq!(
            apply(machine, changes.remove(&machine_uri).as_deref().unwrap()),
            "MACHINE M\nSEES C\nVARIABLES\n    x\nINVARIANTS\n    @i x ∈ ℕ\nEND"
        );
    }

    #[test]
    fn component_rename_accepts_the_name_trailing_edge() {
        let context = "CONTEXT C \nEND";
        let machine = "MACHINE M\nSEES C\nEND";
        let (provider, context_uri, machine_uri) = collision_provider(context, machine);
        let params = make_rename_params(0, 9, context_uri.clone(), "D".to_string());
        let mut changes = provider
            .rename(&params, context)
            .expect("component rename from trailing edge")
            .changes
            .unwrap();

        assert_eq!(
            apply(context, changes.remove(&context_uri).as_deref().unwrap()),
            "CONTEXT D \nEND"
        );
        assert_eq!(
            apply(machine, changes.remove(&machine_uri).as_deref().unwrap()),
            "MACHINE M\nSEES D\nEND"
        );
    }

    #[test]
    fn component_rename_includes_a_repeated_dependency_clause() {
        let context = "CONTEXT D\nEND";
        let machine = "MACHINE M\nSEES C\nSEES D\nEND";
        let (provider, context_uri, machine_uri) = collision_provider(context, machine);
        let params = make_rename_params(0, 8, context_uri.clone(), "E".to_string());
        let mut changes = provider
            .rename(&params, context)
            .expect("component rename through repeated SEES")
            .changes
            .unwrap();

        assert_eq!(
            apply(context, changes.remove(&context_uri).as_deref().unwrap()),
            "CONTEXT E\nEND"
        );
        assert_eq!(
            apply(machine, changes.remove(&machine_uri).as_deref().unwrap()),
            "MACHINE M\nSEES C\nSEES E\nEND"
        );
    }

    #[test]
    fn component_rename_includes_a_headerless_open_dependent() {
        let context_uri = ("file:///c.eventb").parse::<Uri>().unwrap();
        let machine_uri = ("file:///m.eventb").parse::<Uri>().unwrap();
        let context = "CONTEXT C\nEND";
        let indexed_machine = "MACHINE M\nSEES C\nEND";
        let current_machine = "SEES C\nEND";
        let crm = Arc::new(CrossReferenceManager::new());
        crm.update_component(context_uri.as_str().to_owned(), context);
        crm.update_component(machine_uri.as_str().to_owned(), indexed_machine);
        let documents = Arc::new(DocumentManager::new());
        documents.open(context_uri.clone(), 1, context.to_string());
        documents.open(machine_uri.clone(), 1, indexed_machine.to_string());
        documents.change(
            &machine_uri,
            2,
            vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: current_machine.to_string(),
            }],
        );
        let mut provider = RenameProvider::new();
        provider.set_cross_reference_manager(crm);
        provider.set_document_manager(documents);
        let params = make_rename_params(0, 8, context_uri.clone(), "D".to_string());
        let mut changes = provider
            .rename(&params, context)
            .expect("component rename with headerless dependent")
            .changes
            .unwrap();

        assert_eq!(
            apply(
                current_machine,
                changes.remove(&machine_uri).as_deref().unwrap()
            ),
            "SEES D\nEND"
        );
    }

    #[test]
    fn component_rename_visits_every_duplicate_declaration_file() {
        let first_uri = ("file:///a.eventb").parse::<Uri>().unwrap();
        let second_uri = ("file:///b.eventb").parse::<Uri>().unwrap();
        let machine_uri = ("file:///m.eventb").parse::<Uri>().unwrap();
        let context = "CONTEXT C\nEND";
        let machine = "MACHINE M\nSEES C\nEND";
        let crm = Arc::new(CrossReferenceManager::new());
        let documents = Arc::new(DocumentManager::new());
        for uri in [&first_uri, &second_uri] {
            crm.update_component(uri.as_str().to_owned(), context);
            documents.open(uri.clone(), 1, context.to_string());
        }
        crm.update_component(machine_uri.as_str().to_owned(), machine);
        documents.open(machine_uri.clone(), 1, machine.to_string());
        let mut provider = RenameProvider::new();
        provider.set_cross_reference_manager(crm);
        provider.set_document_manager(documents);
        let params = make_rename_params(0, 8, first_uri.clone(), "D".to_string());
        let changes = provider
            .rename(&params, context)
            .expect("component rename with duplicate declarations")
            .changes
            .unwrap();

        assert_eq!(changes.get(&first_uri).map(Vec::len), Some(1));
        assert_eq!(changes.get(&second_uri).map(Vec::len), Some(1));
        assert_eq!(changes.get(&machine_uri).map(Vec::len), Some(1));
    }

    #[test]
    fn component_rename_loads_a_closed_dependent_from_disk() {
        let root = std::env::temp_dir().join(format!(
            "eventb-lsp-component-rename-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let context_path = root.join("c.eventb");
        let machine_path = root.join("m.eventb");
        let context = "CONTEXT C\nEND";
        let machine = "MACHINE M\nSEES C\nEND";
        std::fs::write(&context_path, context).unwrap();
        std::fs::write(&machine_path, machine).unwrap();
        let context_uri = Uri::from_file_path(&context_path).unwrap();
        let machine_uri = Uri::from_file_path(&machine_path).unwrap();

        let crm = Arc::new(CrossReferenceManager::new());
        crm.scan_workspace(&root).unwrap();
        let documents = Arc::new(DocumentManager::new());
        documents.open(context_uri.clone(), 1, context.to_string());
        let mut provider = RenameProvider::new();
        provider.set_cross_reference_manager(crm);
        provider.set_document_manager(documents);
        let params = make_rename_params(0, 8, context_uri.clone(), "D".to_string());
        let mut changes = provider
            .rename(&params, context)
            .expect("component rename")
            .changes
            .unwrap();

        assert_eq!(
            apply(context, changes.remove(&context_uri).as_deref().unwrap()),
            "CONTEXT D\nEND"
        );
        assert_eq!(
            apply(machine, changes.remove(&machine_uri).as_deref().unwrap()),
            "MACHINE M\nSEES D\nEND"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn test_rename_to_builtin_keyword_fails() {
        let uri = make_uri();

        let source = r#"
MACHINE test
VARIABLES
    count
END
"#;
        let provider = provider_for(source);

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
        let provider = provider_for(source);
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
    fn rename_bound_use_at_its_trailing_edge_keeps_the_global() {
        let source = "MACHINE m\nVARIABLES\nx\nINVARIANTS\n@i1 x ∈ ℕ\n@i2 ∀ x · x ∈ ℕ\nEND\n";
        // The caret right after the bound use still names the binder.
        let byte = source.find("· x").unwrap() + "· x".len();
        let out = rename_at(source, byte, "y");
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

    // ---- rename across files ------------------------------------------------

    fn file_uri(name: &str) -> Uri {
        format!("file:///{name}").parse::<Uri>().unwrap()
    }

    /// A provider over a workspace where every one of `sources`, given as
    /// `(file name, text)`, is open.
    fn workspace_provider(sources: &[(&str, &str)]) -> RenameProvider {
        let crm = Arc::new(CrossReferenceManager::new());
        let documents = Arc::new(DocumentManager::new());
        for (name, text) in sources {
            let uri = file_uri(name);
            crm.update_component(uri.as_str().to_owned(), text);
            documents.open(uri, 1, text.to_string());
        }
        let mut provider = RenameProvider::new();
        provider.set_cross_reference_manager(crm);
        provider.set_document_manager(documents);
        provider
    }

    /// Every file of `sources` after renaming, to `new_name`, what the first
    /// `needle` in `file` names; `None` when the rename is refused.
    fn renamed_workspace(
        sources: &[(&str, &str)],
        file: &str,
        needle: &str,
        new_name: &str,
    ) -> Option<Vec<String>> {
        let provider = workspace_provider(sources);
        let text = sources.iter().find(|(name, _)| *name == file).unwrap().1;
        let pos = pos_at(text, text.find(needle).expect("the needle is in the file"));
        let params = make_rename_params(pos.line, pos.character, file_uri(file), new_name.into());
        let mut changes = provider.rename(&params, text)?.changes.unwrap();
        let renamed = sources
            .iter()
            .map(|(name, text)| match changes.remove(&file_uri(name)) {
                Some(edits) => apply(text, &edits),
                None => text.to_string(),
            })
            .collect();
        assert!(
            changes.is_empty(),
            "edits outside the workspace: {changes:?}"
        );
        Some(renamed)
    }

    /// `sources`' texts with every whole-word `old` replaced by `new`.
    fn replaced(sources: &[(&str, &str)], old: &str, new: &str) -> Vec<String> {
        sources
            .iter()
            .map(|(name, text)| {
                let uri = file_uri(name);
                let mut edits: Vec<TextEdit> = identifier_utils::find_whole_word_locations(
                    text,
                    old,
                    &uri,
                    None,
                    identifier_utils::WordBoundary::for_name(old),
                )
                .into_iter()
                .map(|location| TextEdit {
                    range: location.range,
                    new_text: new.to_string(),
                })
                .collect();
                sort_edits_reverse(&mut edits);
                apply(text, &edits)
            })
            .collect()
    }

    #[test]
    fn constant_rename_follows_extending_contexts_and_seeing_machines() {
        let sources = [
            (
                "C0.eventb",
                "CONTEXT C0\nCONSTANTS\n    max_value\nAXIOMS\n    @axm1 max_value ∈ ℕ\nEND\n",
            ),
            (
                "C1.eventb",
                "CONTEXT C1\nEXTENDS C0\nAXIOMS\n    @axm2 max_value > 0\nEND\n",
            ),
            (
                "M1.eventb",
                "MACHINE M1\nSEES C1\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ≤ max_value\nEND\n",
            ),
        ];
        let expected = replaced(&sources, "max_value", "limit");
        // From the declaration, and from a use two files away.
        for file in ["C0.eventb", "M1.eventb"] {
            assert_eq!(
                renamed_workspace(&sources, file, "max_value", "limit"),
                Some(expected.clone()),
                "renamed from {file}"
            );
        }
    }

    #[test]
    fn carrier_set_rename_reaches_a_seeing_machine() {
        let sources = [
            ("C0.eventb", "CONTEXT C0\nSETS\n    COLOURS\nEND\n"),
            (
                "M0.eventb",
                "MACHINE M0\nSEES C0\nVARIABLES\n    v\nINVARIANTS\n    @inv1 v ∈ COLOURS\nEND\n",
            ),
        ];
        assert_eq!(
            renamed_workspace(&sources, "C0.eventb", "COLOURS", "PALETTE"),
            Some(replaced(&sources, "COLOURS", "PALETTE"))
        );
    }

    #[test]
    fn constant_rename_leaves_a_same_named_variable_alone() {
        // The machine's own `x` is another symbol, whatever it shadows.
        let sources = [
            (
                "C1.eventb",
                "CONTEXT C1\nCONSTANTS\n    x\nAXIOMS\n    @axm1 x ∈ ℕ\nEND\n",
            ),
            (
                "M1.eventb",
                "MACHINE M1\nSEES C1\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ∈ ℕ\nEND\n",
            ),
        ];
        let renamed = renamed_workspace(&sources, "C1.eventb", "x\n", "y").unwrap();
        assert_eq!(renamed[0], replaced(&sources[..1], "x", "y")[0]);
        assert_eq!(renamed[1], sources[1].1, "the machine keeps its variable");
    }

    /// Neither prepare nor rename accepts `position` in `source`.
    fn assert_refused(source: &str, position: Position, what: &str) {
        let provider = workspace_provider(&[("m.eventb", source)]);
        let prepare = make_position_params(position.line, position.character, file_uri("m.eventb"));
        assert_eq!(
            provider.prepare_rename(&prepare, source),
            None,
            "prepare {what}"
        );
        let params = make_rename_params(
            position.line,
            position.character,
            file_uri("m.eventb"),
            "renamed".to_string(),
        );
        assert!(provider.rename(&params, source).is_none(), "rename {what}");
    }

    #[test]
    fn rename_refuses_what_the_resolver_cannot_place() {
        let source = "MACHINE m\nVARIABLES\n    x // x counts\nINVARIANTS\n    @inv1 x ∈ ℕ\n    @inv2 w ∈ ℕ\nEND\n";
        // A name nothing declares: renaming every `w` in the file would be a
        // guess.
        assert_refused(source, Position::new(5, 10), "an undeclared name");
        // A label is not a symbol.
        assert_refused(source, Position::new(4, 5), "a label");
        // Nor is a word in a comment, even one spelled like a variable.
        assert_refused(source, Position::new(2, 9), "a word in a comment");
    }

    const REFINEMENT_ABSTRACT: &str =
        include_str!("../../rossi/examples/refinement_abstract.eventb");
    const REFINEMENT_CONCRETE: &str =
        include_str!("../../rossi/examples/refinement_concrete.eventb");

    #[test]
    fn variable_rename_reaches_the_gluing_invariant_and_the_witness() {
        // The concrete machine drops `abstract_state`: it glues it in @inv3
        // and witnesses its after-state in `decrease`.
        let sources = [
            ("abstract.eventb", REFINEMENT_ABSTRACT),
            ("concrete.eventb", REFINEMENT_CONCRETE),
        ];
        let expected = replaced(&sources, "abstract_state", "level");
        for (file, needle) in [
            ("abstract.eventb", "abstract_state"),
            ("concrete.eventb", "abstract_state'"),
            ("concrete.eventb", "abstract_state ="),
        ] {
            assert_eq!(
                renamed_workspace(&sources, file, needle, "level"),
                Some(expected.clone()),
                "renamed from `{needle}` in {file}"
            );
        }
    }

    #[test]
    fn variable_rename_follows_a_retained_variable_both_ways() {
        let sources = [
            (
                "M0.eventb",
                "MACHINE M0\nVARIABLES\n    state\nINVARIANTS\n    @inv1 state ∈ ℕ\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 state ≔ 0\n    END\nEND\n",
            ),
            (
                "M1.eventb",
                "MACHINE M1\nREFINES M0\nVARIABLES\n    state\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 state ≔ 0\n    END\nEND\n",
            ),
            (
                "M2.eventb",
                "MACHINE M2\nREFINES M1\nVARIABLES\n    state\nINVARIANTS\n    @inv1 state ≤ 10\nEND\n",
            ),
        ];
        // Declared again in each refinement, `state` is one variable: renamed
        // from the middle, it changes above and below.
        assert_eq!(
            renamed_workspace(&sources, "M1.eventb", "state", "level"),
            Some(replaced(&sources, "state", "level"))
        );
    }

    #[test]
    fn parameter_rename_reaches_the_witness_of_a_refinement() {
        // The concrete `increase` drops `delta` and witnesses it.
        let sources = [
            ("abstract.eventb", REFINEMENT_ABSTRACT),
            ("concrete.eventb", REFINEMENT_CONCRETE),
        ];
        let expected = replaced(&sources, "delta", "amount");
        for (file, needle) in [
            ("abstract.eventb", "delta"),
            ("concrete.eventb", "delta delta"),
            ("concrete.eventb", "delta ="),
        ] {
            assert_eq!(
                renamed_workspace(&sources, file, needle, "amount"),
                Some(expected.clone()),
                "renamed from `{needle}` in {file}"
            );
        }
    }

    /// A machine with a variable `x` and an event `set` taking `amount`.
    const SET_ABSTRACT: &str = "MACHINE M0\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ∈ ℕ\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x ≔ 0\n    END\n\n    EVENT set\n    ANY\n        amount\n    WHERE\n        @grd1 amount ∈ ℕ\n    THEN\n        @act1 x ≔ amount\n    END\nEND\n";

    #[test]
    fn parameter_rename_follows_a_retained_parameter() {
        let sources = [
            ("M0.eventb", SET_ABSTRACT),
            (
                "M1.eventb",
                "MACHINE M1\nREFINES M0\nVARIABLES\n    x\nEVENTS\n    EVENT set\n    REFINES set\n    ANY\n        amount\n    WHERE\n        @grd1 amount ≤ 10\n    THEN\n        @act1 x ≔ amount\n    END\nEND\n",
            ),
        ];
        assert_eq!(
            renamed_workspace(&sources, "M1.eventb", "amount", "value"),
            Some(replaced(&sources, "amount", "value"))
        );
    }

    #[test]
    fn parameter_rename_follows_an_extending_event() {
        let sources = [
            ("M0.eventb", SET_ABSTRACT),
            (
                "M1.eventb",
                "MACHINE M1\nREFINES M0\nVARIABLES\n    x\nEVENTS\n    EVENT set extends set\n    WHERE\n        @grd2 amount < 10\n    END\nEND\n",
            ),
        ];
        assert_eq!(
            renamed_workspace(&sources, "M1.eventb", "amount", "value"),
            Some(replaced(&sources, "amount", "value"))
        );
    }

    #[test]
    fn parameter_rename_in_a_later_machine_of_a_merged_file() {
        // Both machines have an event `set`; the second machine's parameter
        // is found in its own event, not in the first machine's.
        let refinement = "MACHINE M1\nREFINES M0\nVARIABLES\n    x\nEVENTS\n    EVENT set\n    REFINES set\n    ANY\n        amount\n    WHERE\n        @grd1 amount ≤ 10\n    THEN\n        @act1 x ≔ amount\n    END\n\n";
        let other = "    EVENT other\n    ANY\n        amount\n    WHERE\n        @grd1 amount ∈ ℕ\n    THEN\n        @act1 x ≔ amount\n    END\nEND\n";
        let merged = format!("{SET_ABSTRACT}\n{refinement}{other}");
        let sources = [("merged.eventb", merged.as_str())];
        let renamed = renamed_workspace(
            &sources,
            "merged.eventb",
            "amount\n    WHERE\n        @grd1 amount ≤",
            "value",
        );
        let expected = format!(
            "{}\n{}{other}",
            SET_ABSTRACT.replace("amount", "value"),
            refinement.replace("amount", "value")
        );
        assert_eq!(renamed, Some(vec![expected]));
    }

    const STEP_CHAIN: [(&str, &str); 3] = [
        (
            "M0.eventb",
            "MACHINE M0\nEVENTS\n    EVENT step\n    THEN\n        skip\n    END\nEND\n",
        ),
        (
            "M1.eventb",
            "MACHINE M1\nREFINES M0\nEVENTS\n    EVENT step\n    REFINES step\n    THEN\n        skip\n    END\n\n    EVENT pause\n    REFINES step\n    THEN\n        skip\n    END\nEND\n",
        ),
        (
            "M2.eventb",
            "MACHINE M2\nREFINES M1\nEVENTS\n    EVENT step extends step\n    END\nEND\n",
        ),
    ];

    #[test]
    fn event_rename_follows_same_named_refining_events() {
        // Each `step` refines the one above it and keeps its name, so all
        // three are one event; `pause` refines it too but is another event,
        // whose target follows the rename.
        let expected = replaced(&STEP_CHAIN, "step", "advance");
        for (file, needle) in [
            ("M0.eventb", "step"),
            ("M2.eventb", "step extends"),
            ("M2.eventb", "step\n    END"),
        ] {
            assert_eq!(
                renamed_workspace(&STEP_CHAIN, file, needle, "advance"),
                Some(expected.clone()),
                "renamed from `{needle}` in {file}"
            );
        }
    }

    #[test]
    fn event_rename_accepts_a_hyphenated_name() {
        let renamed = renamed_workspace(&STEP_CHAIN, "M0.eventb", "step", "do-step");
        assert_eq!(renamed, Some(replaced(&STEP_CHAIN, "step", "do-step")));
    }

    #[test]
    fn rename_of_an_event_named_like_a_variable_renames_the_event() {
        let source = "MACHINE m\nVARIABLES\n    tick\nINVARIANTS\n    @inv1 tick ∈ ℕ\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 tick ≔ 0\n    END\n\n    EVENT tick\n    THEN\n        @act1 tick ≔ tick + 1\n    END\nEND\n";
        let sources = [("m.eventb", source)];
        assert_eq!(
            renamed_workspace(&sources, "m.eventb", "tick\n    THEN", "step"),
            Some(vec![source.replace("EVENT tick", "EVENT step")])
        );
    }

    #[test]
    fn event_rename_from_a_target_trailing_edge_past_a_same_named_event() {
        // `pause` refines the abstract `step`; the concrete `step` is a new
        // event. The caret just past the target still names the abstract one.
        let sources = [
            (
                "M0.eventb",
                "MACHINE M0\nEVENTS\n    EVENT step\n    THEN\n        skip\n    END\nEND\n",
            ),
            (
                "M1.eventb",
                "MACHINE M1\nREFINES M0\nEVENTS\n    EVENT pause\n    REFINES step\n    THEN\n        skip\n    END\n\n    EVENT step\n    THEN\n        skip\n    END\nEND\n",
            ),
        ];
        let text = sources[1].1;
        let pos = pos_at(
            text,
            text.find("REFINES step").unwrap() + "REFINES step".len(),
        );
        let provider = workspace_provider(&sources);
        let params =
            make_rename_params(pos.line, pos.character, file_uri("M1.eventb"), "go".into());
        let mut changes = provider
            .rename(&params, text)
            .expect("rename from the target's trailing edge")
            .changes
            .unwrap();
        assert_eq!(
            apply(
                sources[0].1,
                &changes.remove(&file_uri("M0.eventb")).unwrap()
            ),
            sources[0].1.replace("step", "go")
        );
        assert_eq!(
            apply(text, &changes.remove(&file_uri("M1.eventb")).unwrap()),
            text.replace("REFINES step", "REFINES go")
        );
    }

    #[test]
    fn rename_refuses_initialisation() {
        let source = "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x ≔ 0\n    END\nEND\n";
        let sources = [("m.eventb", source)];
        assert_eq!(
            renamed_workspace(&sources, "m.eventb", "INITIALISATION", "START"),
            None
        );
    }

    #[test]
    fn constant_rename_edits_a_closed_seeing_machine() {
        let root = std::env::temp_dir().join(format!(
            "eventb-lsp-constant-rename-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let context_path = root.join("c.eventb");
        let machine_path = root.join("m.eventb");
        let context = "CONTEXT C\nCONSTANTS\n    k\nAXIOMS\n    @axm1 k ∈ ℕ\nEND\n";
        let machine = "MACHINE M\nSEES C\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ≤ k\nEND\n";
        std::fs::write(&context_path, context).unwrap();
        std::fs::write(&machine_path, machine).unwrap();
        let context_uri = Uri::from_file_path(&context_path).unwrap();
        let machine_uri = Uri::from_file_path(&machine_path).unwrap();

        // Only the context is open.
        let crm = Arc::new(CrossReferenceManager::new());
        crm.scan_workspace(&root).unwrap();
        let documents = Arc::new(DocumentManager::new());
        documents.open(context_uri.clone(), 1, context.to_string());
        let mut provider = RenameProvider::new();
        provider.set_cross_reference_manager(crm);
        provider.set_document_manager(documents);
        let params = make_rename_params(2, 4, context_uri.clone(), "bound".to_string());
        let mut changes = provider
            .rename(&params, context)
            .expect("constant rename")
            .changes
            .unwrap();

        assert_eq!(
            apply(context, changes.remove(&context_uri).as_deref().unwrap()),
            context.replace('k', "bound")
        );
        assert_eq!(
            apply(machine, changes.remove(&machine_uri).as_deref().unwrap()),
            machine.replace("≤ k", "≤ bound")
        );
        std::fs::remove_dir_all(root).unwrap();
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

        let crm = Arc::new(CrossReferenceManager::new());
        crm.update_component(uri.as_str().to_owned(), stored);
        let documents = Arc::new(DocumentManager::new());
        documents.open(uri.clone(), 1, stored.to_string());

        let mut provider = RenameProvider::new();
        provider.set_cross_reference_manager(crm);
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
