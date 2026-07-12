//! Code completion provider for Event-B
//!
//! Provides intelligent auto-completion for:
//! - Keywords (context-aware based on position)
//! - Operators (Unicode and ASCII variants)
//! - Identifiers (variables, constants, sets, parameters)
//! - Snippets (common patterns like events, axioms)

use crate::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionParams, CompletionResponse, CompletionTextEdit,
    Documentation, InsertTextFormat, Position, Range, TextEdit,
};
use rossi::{Component, keywords, operators};

use crate::component_loader::ComponentLoader;
use crate::component_util::component_at_offset;
use crate::config::{CompletionConfig, FormatConfig};
use crate::identifier_utils::position_to_offset;
use crate::position::{line_run_to_range, utf16_to_char_col};
use std::collections::HashSet;
use std::sync::Arc;

use crate::cross_references::CrossReferenceManager;
use crate::document::DocumentManager;
use crate::references::component_reference_clause;
use crate::text_utils;

/// Completion context - tracks what's available at the cursor position
#[derive(Debug, Clone)]
struct CompletionContext {
    /// Variables available in current scope
    variables: Vec<String>,
    /// Constants available in current scope (from seen contexts)
    constants: Vec<String>,
    /// Sets available in current scope (from seen contexts)
    sets: Vec<String>,
    /// Parameters from current event's ANY clause
    parameters: Vec<String>,
    /// Formula binders (∀/∃/λ/comprehension/⋃/⋂) in scope at the cursor
    locals: Vec<String>,
}

impl CompletionContext {
    fn new() -> Self {
        Self {
            variables: Vec::new(),
            constants: Vec::new(),
            sets: Vec::new(),
            parameters: Vec::new(),
            locals: Vec::new(),
        }
    }

    fn from_component_with_refs(component: &Component, loader: Option<&ComponentLoader>) -> Self {
        let mut ctx = Self::new();

        match component {
            Component::Context(context) => {
                ctx.constants
                    .extend(context.constants.iter().map(|c| c.name.clone()));
                ctx.sets
                    .extend(context.sets.iter().map(|s| s.name().to_string()));

                // Resolve EXTENDS chain transitively
                if let Some(loader) = loader {
                    let mut visited = HashSet::new();
                    visited.insert(context.name.clone());
                    for parent_name in &context.extends {
                        resolve_context_symbols(
                            parent_name,
                            loader,
                            &mut ctx.constants,
                            &mut ctx.sets,
                            &mut visited,
                        );
                    }
                }
            }
            Component::Machine(machine) => {
                ctx.variables
                    .extend(machine.variables.iter().map(|v| v.name.clone()));

                // Resolve SEES contexts and REFINES machines
                if let Some(loader) = loader {
                    let mut visited = HashSet::new();
                    for ctx_name in &machine.sees {
                        resolve_context_symbols(
                            ctx_name,
                            loader,
                            &mut ctx.constants,
                            &mut ctx.sets,
                            &mut visited,
                        );
                    }
                    if let Some(ref refines_name) = machine.refines {
                        resolve_machine_symbols(
                            refines_name,
                            loader,
                            &mut ctx.variables,
                            &mut ctx.constants,
                            &mut ctx.sets,
                            &mut visited,
                        );
                    }
                }
            }
        }

        ctx
    }

    /// Augment the context with the symbols scoped to the cursor: the enclosing
    /// event's `ANY` parameters and the formula binders in scope at `offset`.
    /// `masked` must be the comment-masked form, and `offset` the byte offset, of
    /// the same source the `component` was parsed from, so its event line ranges
    /// and binder spans index one snapshot.
    fn add_local_scope(
        &mut self,
        component: &Component,
        masked: &str,
        position: Position,
        offset: usize,
    ) {
        if let Component::Machine(machine) = component
            && let Some(event) = text_utils::enclosing_event(machine, masked, position)
        {
            self.parameters
                .extend(event.parameters.iter().map(|p| p.name.clone()));
        }
        // Formula binders occur in both contexts and machines.
        self.locals
            .extend(crate::formula_walk::binders_in_scope_at_offset(
                component, offset,
            ));
    }
}

/// Provides code completion for Event-B documents
pub struct CompletionProvider {
    /// Cross-reference manager for workspace-wide navigation
    cross_ref_manager: Option<Arc<CrossReferenceManager>>,
    /// Document manager — the source of the document's shared recovered parse
    document_manager: Option<Arc<DocumentManager>>,
}

impl CompletionProvider {
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

    /// Generate completion items for the given position
    pub fn complete(
        &self,
        params: &CompletionParams,
        text: &str,
        completion_config: &CompletionConfig,
        format_config: &FormatConfig,
    ) -> Option<CompletionResponse> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        if !completion_config.enabled {
            return None;
        }

        // No completions inside a comment — it's prose, not Event-B.
        let lexical = rossi::comments::lexical_spans(text);
        if let Some(offset) = position_to_offset(text, position)
            && rossi::comments::span_containing(&lexical.comments, offset).is_some()
        {
            return None;
        }
        // Structural context detection scans the comment-masked line, so an
        // `EVENT` mentioned in a trailing comment cannot change the scope.
        let masked = lexical.mask_comments_chars(text);

        // Get completion context from the component under the cursor in the
        // document's shared parse (the single source of truth maintained by the
        // document manager), along with that component's own name (to exclude it
        // from component-name completion — a component never references itself).
        let parsed = self
            .document_manager
            .as_ref()
            .and_then(|dm| dm.parse_result(uri));
        // One loader per request: each visible context/machine in the SEES /
        // EXTENDS / REFINES walk is parsed at most once, reusing open documents'
        // stored parses.
        let loader = ComponentLoader::optional(
            self.cross_ref_manager.as_deref(),
            self.document_manager.as_deref(),
        );
        // Select the cursor's component against the stored parse's own text, so
        // the offset and the component spans index one snapshot — the handler
        // `text` is a separate copy a concurrent edit can desync from the parse.
        let (completion_ctx, self_name) = parsed
            .as_deref()
            .and_then(|parsed| {
                let offset =
                    position_to_offset(&parsed.text, position).unwrap_or(parsed.text.len());
                component_at_offset(parsed.components(), offset).map(|component| {
                    let mut ctx =
                        CompletionContext::from_component_with_refs(component, loader.as_ref());
                    // Scope the event `ANY` parameters and formula binders off the
                    // same snapshot the component was parsed from, so the event
                    // line ranges and binder spans line up with the cursor. When
                    // the stored parse is the same text the handler masked (the
                    // common case — no concurrent edit), reuse that mask instead
                    // of scanning and allocating the whole document again.
                    let reparsed_mask;
                    let scope_masked = if parsed.text == text {
                        masked.as_str()
                    } else {
                        reparsed_mask = rossi::comments::mask_comments_chars(&parsed.text);
                        &reparsed_mask
                    };
                    ctx.add_local_scope(component, scope_masked, position, offset);
                    (ctx, Some(rossi::deps::kind_and_name(component).1))
                })
            })
            .unwrap_or((CompletionContext::new(), None));

        // Determine what to complete based on context
        let mut items = Vec::new();

        // Analyze the text to determine context
        let line_text = get_line_text(&masked, position);
        // `position.character` is a UTF-16 column; `get_word_at_position` slices
        // by char, so convert first or an astral char before the cursor would
        // truncate the word.
        let char_col = utf16_to_char_col(&line_text, position.character as usize);
        let word_at_cursor = get_word_at_position(&line_text, char_col);

        // Add keyword completions
        items.extend(self.get_keyword_completions(&line_text, &word_at_cursor));

        // Add operator completions
        items.extend(self.get_operator_completions(format_config.use_unicode));

        // Add identifier completions
        items.extend(self.get_identifier_completions(&completion_ctx, &word_at_cursor));

        // Add component-name completions on REFINES/SEES/EXTENDS clauses, with a
        // hyphen-aware replace range so editors match/replace across `-` (#36).
        items.extend(self.get_component_name_completions(&masked, position, self_name.as_deref()));

        // Add snippet completions
        items.extend(self.get_snippet_completions(&line_text, position));

        // Add built-in type completions
        items.extend(self.get_builtin_completions(&word_at_cursor));

        if items.is_empty() {
            None
        } else {
            Some(CompletionResponse::Array(items))
        }
    }

    /// Get keyword completions based on context
    fn get_keyword_completions(&self, line_text: &str, _word: &str) -> Vec<CompletionItem> {
        use keywords::{KeywordGroup, KeywordId, scope};
        let mut items = Vec::new();

        // Top-level keywords
        if line_text.trim().is_empty() || is_top_level_context(line_text) {
            push_keyword_items(
                &mut items,
                [KeywordId::Context, KeywordId::Machine]
                    .into_iter()
                    .map(keywords::keyword),
            );
        }

        // Context clause keywords
        if is_inside_context(line_text) {
            push_keyword_items(&mut items, keywords::iter_completion_scope(scope::CONTEXT));
        }

        // Machine clause keywords
        if is_inside_machine(line_text) {
            push_keyword_items(&mut items, keywords::iter_completion_scope(scope::MACHINE));
        }

        // Events section keywords
        if is_inside_events(line_text) {
            push_keyword_items(&mut items, keywords::iter_completion_scope(scope::EVENTS));
        }

        // Event keywords
        if is_inside_event(line_text) {
            push_keyword_items(&mut items, keywords::iter_completion_scope(scope::EVENT));
        }

        // Event status values (triggered on a STATUS line)
        if line_text.contains("STATUS") {
            push_keyword_items(&mut items, keywords::iter_group(KeywordGroup::Status));
        }

        items
    }

    /// Get operator completions (Unicode or ASCII based on config)
    fn get_operator_completions(&self, use_unicode: bool) -> Vec<CompletionItem> {
        operators::OPERATOR_SPELLINGS
            .iter()
            .filter(|entry| entry.completion)
            .map(|entry| {
                let label = entry.emit_text(use_unicode);
                let alternative = entry.emit_text(!use_unicode);
                let alternative = if alternative == label {
                    ""
                } else {
                    alternative
                };
                create_operator_item(label, alternative, entry.description)
            })
            .collect()
    }

    /// Get identifier completions from the current context.
    ///
    /// The symbol classes are offered most-local first, and a name is offered
    /// only once: an in-scope binder or event parameter shadows a same-named
    /// global symbol, so it wins the single completion item for that name rather
    /// than the editor showing the name twice with conflicting kinds.
    fn get_identifier_completions(
        &self,
        ctx: &CompletionContext,
        _word: &str,
    ) -> Vec<CompletionItem> {
        // (names, kind, detail, documentation noun) — most local first.
        let groups: [(&[String], CompletionItemKind, &str, &str); 5] = [
            (
                &ctx.locals,
                CompletionItemKind::VARIABLE,
                "Bound variable",
                "Bound variable",
            ),
            (
                &ctx.parameters,
                CompletionItemKind::VARIABLE,
                "Parameter",
                "Event parameter",
            ),
            (
                &ctx.variables,
                CompletionItemKind::VARIABLE,
                "Variable",
                "State variable",
            ),
            (
                &ctx.constants,
                CompletionItemKind::CONSTANT,
                "Constant",
                "Constant",
            ),
            (&ctx.sets, CompletionItemKind::ENUM, "Set", "Carrier set"),
        ];

        let mut items = Vec::new();
        let mut seen = HashSet::new();
        for (names, kind, detail, noun) in groups {
            for name in names {
                if !seen.insert(name.clone()) {
                    continue;
                }
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(kind),
                    detail: Some(detail.to_string()),
                    documentation: Some(Documentation::String(format!("{noun} `{name}`"))),
                    ..Default::default()
                });
            }
        }

        items
    }

    /// Component-name completions for a REFINES/SEES/EXTENDS clause. The cheap
    /// clause check runs first, so the workspace component list is only queried
    /// when the cursor is actually in such a clause. Each item carries an
    /// explicit edit spanning the whole (possibly hyphenated) word under the
    /// cursor, so the editor filters and replaces across `-` rather than only
    /// the segment after the last hyphen (issue #36). `self_name` is the
    /// enclosing component, excluded so it can't reference itself.
    fn get_component_name_completions(
        &self,
        masked: &str,
        position: Position,
        self_name: Option<&str>,
    ) -> Vec<CompletionItem> {
        let Some(clause) = component_reference_clause(masked, position) else {
            return Vec::new();
        };
        let Some(crm) = self.cross_ref_manager.as_deref() else {
            return Vec::new();
        };
        // REFINES targets a machine; SEES/EXTENDS a context (the edge's target).
        let kind = clause.target_kind();

        let range = hyphenated_word_range(masked, position);
        crm.component_names_of_kind(kind)
            .into_iter()
            .filter(|name| Some(name.as_str()) != self_name)
            .map(|name| CompletionItem {
                label: name.clone(),
                kind: Some(CompletionItemKind::MODULE),
                detail: Some("Component".to_string()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range,
                    new_text: name,
                })),
                ..Default::default()
            })
            .collect()
    }

    /// Snippet completions, sourced from the canonical [`rossi::snippets`]
    /// table — the same table the editor snippet libraries are generated from.
    /// Serving them here means the LSP (the path Sublime Text uses, since it has
    /// no native snippet files) offers exactly the snippets every other editor
    /// ships, so the two can never drift.
    ///
    /// When the cursor follows a `\name` input leader (e.g. the user typed
    /// `\exists` and pressed Tab), the editor's own word boundary excludes the
    /// leading backslash, so a plain insert leaves it stranded in front of the
    /// expanded body (`\∃ …`, which then fails to parse — issue #78). In that
    /// case each item gets an explicit edit that replaces the whole `\name`
    /// span, plus a backslashed `filter_text` so the client still matches it.
    fn get_snippet_completions(&self, line: &str, position: Position) -> Vec<CompletionItem> {
        let leader = leader_token_range(line, position);
        rossi::snippets::SNIPPETS
            .iter()
            .map(|snippet| {
                let body = snippet.body.join("\n");
                // With a `\name` leader, replace the whole `\name` span via an
                // explicit edit (so the backslash is consumed, not stranded) and
                // filter on the backslashed prefix; otherwise insert at the
                // cursor and let the editor match on the label as usual.
                let (insert_text, text_edit, filter_text) = match leader {
                    Some(range) => (
                        None,
                        Some(CompletionTextEdit::Edit(TextEdit {
                            range,
                            new_text: body,
                        })),
                        Some(format!("\\{}", snippet.prefix)),
                    ),
                    None => (Some(body), None, None),
                };
                CompletionItem {
                    label: snippet.prefix.to_string(),
                    kind: Some(CompletionItemKind::SNIPPET),
                    detail: Some(snippet.name.to_string()),
                    documentation: Some(Documentation::String(snippet.description.to_string())),
                    insert_text,
                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                    text_edit,
                    filter_text,
                    ..Default::default()
                }
            })
            .collect()
    }

    /// Get built-in type and constant completions
    fn get_builtin_completions(&self, _word: &str) -> Vec<CompletionItem> {
        vec![
            create_builtin_item("BOOL", "Boolean type {TRUE, FALSE}"),
            create_builtin_item("TRUE", "Boolean true value"),
            create_builtin_item("FALSE", "Boolean false value"),
            create_builtin_item("ℕ", "Natural numbers (0, 1, 2, ...)"),
            create_builtin_item("NAT", "Natural numbers (ASCII)"),
            create_builtin_item("ℕ1", "Positive natural numbers (1, 2, 3, ...)"),
            create_builtin_item("NAT1", "Positive natural numbers (ASCII)"),
            create_builtin_item("ℤ", "Integers (..., -1, 0, 1, ...)"),
            create_builtin_item("INT", "Integers (ASCII)"),
        ]
    }
}

impl Default for CompletionProvider {
    fn default() -> Self {
        Self::new()
    }
}

// Cross-document resolution helpers

/// Resolve constants and sets from a context (and its EXTENDS parents) transitively.
/// Uses a visited set to prevent cycles; caps depth at 10.
fn resolve_context_symbols(
    context_name: &str,
    loader: &ComponentLoader,
    constants: &mut Vec<String>,
    sets: &mut Vec<String>,
    visited: &mut HashSet<String>,
) {
    // Cycle/depth guard
    if visited.len() >= 10 || visited.contains(context_name) {
        return;
    }
    visited.insert(context_name.to_string());

    let loaded = match loader.load(context_name) {
        Some(l) => l,
        None => return,
    };

    if let Component::Context(ctx) = loaded.component() {
        constants.extend(ctx.constants.iter().map(|c| c.name.clone()));
        sets.extend(ctx.sets.iter().map(|s| s.name().to_string()));

        // Recursively resolve EXTENDS parents
        for parent_name in &ctx.extends {
            resolve_context_symbols(parent_name, loader, constants, sets, visited);
        }
    }
}

/// Resolve variables from a refined machine (and its transitive REFINES/SEES dependencies).
fn resolve_machine_symbols(
    machine_name: &str,
    loader: &ComponentLoader,
    variables: &mut Vec<String>,
    constants: &mut Vec<String>,
    sets: &mut Vec<String>,
    visited: &mut HashSet<String>,
) {
    if visited.len() >= 10 || visited.contains(machine_name) {
        return;
    }
    visited.insert(machine_name.to_string());

    let loaded = match loader.load(machine_name) {
        Some(l) => l,
        None => return,
    };

    if let Component::Machine(m) = loaded.component() {
        variables.extend(m.variables.iter().map(|v| v.name.clone()));

        // Resolve SEES contexts from the abstract machine
        for ctx_name in &m.sees {
            resolve_context_symbols(ctx_name, loader, constants, sets, visited);
        }

        // Recurse into further refinements
        if let Some(ref refines_name) = m.refines {
            resolve_machine_symbols(refines_name, loader, variables, constants, sets, visited);
        }
    }
}

// Helper functions

fn create_keyword_item(keyword: &str, description: &str) -> CompletionItem {
    CompletionItem {
        label: keyword.to_string(),
        kind: Some(CompletionItemKind::KEYWORD),
        detail: Some("Keyword".to_string()),
        documentation: Some(Documentation::String(description.to_string())),
        ..Default::default()
    }
}

/// Push a completion item for every spelling of each keyword (so alternates like
/// `WHEN`/`BEGIN` are offered alongside `WHERE`/`THEN`).
fn push_keyword_items<'a>(
    items: &mut Vec<CompletionItem>,
    iter: impl Iterator<Item = &'a keywords::Keyword>,
) {
    for kw in iter {
        for spelling in kw.spellings {
            items.push(create_keyword_item(spelling, kw.summary));
        }
    }
}

fn create_operator_item(operator: &str, alternative: &str, description: &str) -> CompletionItem {
    let detail = if alternative.is_empty() {
        "Operator".to_string()
    } else {
        format!("Operator (alternative: {})", alternative)
    };

    CompletionItem {
        label: operator.to_string(),
        kind: Some(CompletionItemKind::OPERATOR),
        detail: Some(detail),
        documentation: Some(Documentation::String(description.to_string())),
        ..Default::default()
    }
}

fn create_builtin_item(name: &str, description: &str) -> CompletionItem {
    CompletionItem {
        label: name.to_string(),
        kind: Some(CompletionItemKind::CONSTANT),
        detail: Some("Built-in".to_string()),
        documentation: Some(Documentation::String(description.to_string())),
        ..Default::default()
    }
}

fn get_line_text(text: &str, position: Position) -> String {
    text.lines()
        .nth(position.line as usize)
        .unwrap_or("")
        .to_string()
}

fn get_word_at_position(line: &str, char_pos: usize) -> String {
    // `char_pos` is a character column, not a byte offset — slice by chars so a
    // multi-byte operator (e.g. `∈`, `≤`) before the cursor can't land mid-char
    // and panic.
    let before_cursor: String = line.chars().take(char_pos).collect();
    before_cursor
        .split_whitespace()
        .last()
        .unwrap_or("")
        .to_string()
}

/// The range of the (possibly hyphenated) word ending at the cursor, used as a
/// completion edit range so a hyphenated component name is replaced whole, not
/// just its last `-` segment. Scans left over component-name characters
/// (`keywords::is_structural_word_char`: ASCII alphanumerics, `_`, `-`) — the
/// same charset the grammar's `component_name` accepts — so it never extends
/// across a character that can't be part of a name.
fn hyphenated_word_range(masked: &str, position: Position) -> Range {
    let line = masked.lines().nth(position.line as usize).unwrap_or("");
    let chars: Vec<char> = line.chars().collect();
    // The incoming `position.character` is a UTF-16 column; the scan below
    // indexes `chars` by char, and the returned range is emitted as UTF-16
    // columns — so convert in on the way down and back out on the way up.
    // `utf16_to_char_col` already clamps to the line's char count.
    let cursor = utf16_to_char_col(line, position.character as usize);
    let mut start = cursor;
    while start > 0 && keywords::is_structural_word_char(chars[start - 1]) {
        start -= 1;
    }
    // A component name can't start with `-`, so don't pull a leading hyphen
    // into the replace range.
    while start < cursor && chars[start] == '-' {
        start += 1;
    }
    line_run_to_range(line, position.line, start, cursor)
}

/// The range of a `\name` input-leader token ending at the cursor, if one is
/// present. Scans left over `[A-Za-z0-9_]` name characters; if a single `\`
/// sits immediately before that run (or right before the cursor, for a bare
/// `\`), returns the range from the backslash through the cursor — used as a
/// snippet completion's edit range so expanding `\exists` replaces the whole
/// `\exists`, not just the word after the backslash (issue #78). Returns `None`
/// when there is no leading backslash, leaving plain word typing to the
/// editor's default replace behaviour. Columns are UTF-16 in and out (LSP),
/// converted via `utf16_to_char_col` / `line_run_to_range`, as in
/// [`hyphenated_word_range`].
fn leader_token_range(line: &str, position: Position) -> Option<Range> {
    let chars: Vec<char> = line.chars().collect();
    let cursor = utf16_to_char_col(line, position.character as usize);
    let mut start = cursor;
    while start > 0 && (chars[start - 1].is_ascii_alphanumeric() || chars[start - 1] == '_') {
        start -= 1;
    }
    (start > 0 && chars[start - 1] == '\\')
        .then(|| line_run_to_range(line, position.line, start - 1, cursor))
}

// Context detection functions

fn is_top_level_context(line_text: &str) -> bool {
    // Callers pass comment-masked lines, so a comment-only line is blank
    // here (and a cursor inside a comment never reaches completion at all).
    line_text.trim().is_empty()
}

fn is_inside_context(_line_text: &str) -> bool {
    // In a real implementation, we'd track whether we're inside a CONTEXT block
    // For now, we'll use a simple heuristic
    true
}

fn is_inside_machine(_line_text: &str) -> bool {
    // In a real implementation, we'd track whether we're inside a MACHINE block
    true
}

fn is_inside_events(line_text: &str) -> bool {
    line_text.contains("EVENTS")
        || line_text.contains("EVENT")
        || line_text.contains("INITIALISATION")
}

fn is_inside_event(line_text: &str) -> bool {
    line_text.contains("EVENT") && !line_text.contains("EVENTS")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keyword_completions() {
        let provider = CompletionProvider::new();
        let items = provider.get_keyword_completions("", "");

        // Should include top-level keywords
        assert!(items.iter().any(|item| item.label == "CONTEXT"));
        assert!(items.iter().any(|item| item.label == "MACHINE"));
    }

    #[test]
    fn test_no_completions_inside_comment() {
        let provider = CompletionProvider::new();
        let text = "MACHINE m // type EVENT here\nEND\n";
        let params = CompletionParams {
            text_document_position: crate::lsp_types::TextDocumentPositionParams {
                text_document: crate::lsp_types::TextDocumentIdentifier {
                    uri: crate::lsp_types::Url::parse("file:///test.eventb").unwrap(),
                },
                position: Position::new(0, 20), // inside the comment
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: None,
        };

        assert!(
            provider
                .complete(
                    &params,
                    text,
                    &CompletionConfig::default(),
                    &FormatConfig::default(),
                )
                .is_none()
        );
    }

    #[test]
    fn test_operator_completions_unicode() {
        let provider = CompletionProvider::new();
        let items = provider.get_operator_completions(true);

        // Should include Unicode operators
        assert!(items.iter().any(|item| item.label == "∧"));
        assert!(items.iter().any(|item| item.label == "∨"));
        assert!(items.iter().any(|item| item.label == "⇒"));
        assert!(items.iter().any(|item| item.label == "∈"));
        assert!(items.iter().any(|item| item.label == "⊈"));
        // The private-use operators have no portable glyph, so even in Unicode
        // mode their completion inserts the ASCII spelling, never a tofu glyph.
        assert!(items.iter().any(|item| item.label == "<<->"));
        assert!(items.iter().any(|item| item.label == "<+"));
        assert!(
            !items
                .iter()
                .any(|item| operators::is_private_use_glyph(&item.label)),
            "no operator completion should insert a private-use glyph"
        );
        assert!(items.iter().any(|item| item.label == "‥"));
        assert!(items.iter().any(|item| item.label == "−"));
        assert!(items.iter().any(|item| item.label == ":∣"));
        assert!(items.iter().any(|item| item.label == "ℙ"));
        assert!(!items.iter().any(|item| item.label == "℘"));
    }

    #[test]
    fn test_operator_completions_ascii() {
        let provider = CompletionProvider::new();
        let items = provider.get_operator_completions(false);

        // Should include ASCII operators
        assert!(items.iter().any(|item| item.label == "&"));
        assert!(items.iter().any(|item| item.label == "or"));
        assert!(items.iter().any(|item| item.label == "=>"));
        assert!(items.iter().any(|item| item.label == ":"));
        assert!(items.iter().any(|item| item.label == "::"));
    }

    #[test]
    fn test_identifier_completions() {
        let provider = CompletionProvider::new();
        let ctx = CompletionContext {
            variables: vec!["count".to_string(), "total".to_string()],
            constants: vec!["max_value".to_string()],
            sets: vec!["STATUS".to_string()],
            parameters: vec!["x".to_string()],
            locals: vec!["bound".to_string()],
        };

        let items = provider.get_identifier_completions(&ctx, "");

        assert!(items.iter().any(|item| item.label == "count"));
        assert!(items.iter().any(|item| item.label == "total"));
        assert!(items.iter().any(|item| item.label == "max_value"));
        assert!(items.iter().any(|item| item.label == "STATUS"));
        assert!(items.iter().any(|item| item.label == "x"));
        assert!(
            items
                .iter()
                .any(|item| item.label == "bound"
                    && item.detail.as_deref() == Some("Bound variable"))
        );
    }

    #[test]
    fn identifier_completions_offer_a_shadowed_name_once_most_local_wins() {
        let provider = CompletionProvider::new();
        // `x` is both a state variable and an in-scope binder; the binder shadows
        // it, so `x` is offered once, as the bound variable.
        let ctx = CompletionContext {
            variables: vec!["x".to_string()],
            constants: Vec::new(),
            sets: Vec::new(),
            parameters: Vec::new(),
            locals: vec!["x".to_string()],
        };

        let items = provider.get_identifier_completions(&ctx, "");
        let xs: Vec<_> = items.iter().filter(|item| item.label == "x").collect();
        assert_eq!(xs.len(), 1, "a shadowed name is offered once, got {xs:?}");
        assert_eq!(xs[0].detail.as_deref(), Some("Bound variable"));
    }

    #[test]
    fn test_builtin_completions() {
        let provider = CompletionProvider::new();
        let items = provider.get_builtin_completions("");

        assert!(items.iter().any(|item| item.label == "BOOL"));
        assert!(items.iter().any(|item| item.label == "TRUE"));
        assert!(items.iter().any(|item| item.label == "FALSE"));
        assert!(items.iter().any(|item| item.label == "ℕ"));
        assert!(items.iter().any(|item| item.label == "NAT"));
        assert!(items.iter().any(|item| item.label == "ℤ"));
        assert!(items.iter().any(|item| item.label == "INT"));
    }

    #[test]
    fn test_snippet_completions() {
        let provider = CompletionProvider::new();
        // No `\` leader, so items carry a plain insert_text and no text_edit.
        let items = provider.get_snippet_completions("", Position::new(0, 0));

        // Every snippet comes from the canonical table — one item per entry.
        assert_eq!(items.len(), rossi::snippets::SNIPPETS.len());
        assert!(items.iter().any(|item| item.label == "evt"));
        assert!(items.iter().any(|item| item.label == "forall"));
        assert!(items.iter().any(|item| item.label == "exists"));
        // Every item is a snippet carrying its body, with no edit range when
        // there is no leader to consume.
        assert!(items.iter().all(|item| {
            item.kind == Some(CompletionItemKind::SNIPPET)
                && item.insert_text_format == Some(InsertTextFormat::SNIPPET)
                && item.insert_text.is_some()
                && item.text_edit.is_none()
        }));
        // The old ad-hoc labels are gone now that the table is the source.
        assert!(!items.iter().any(|item| item.label == "event"));
        assert!(!items.iter().any(|item| item.label == "labeled_predicate"));
    }

    #[test]
    fn leader_token_range_spans_backslash_and_word() {
        // `\exists`, cursor at the end (UTF-16 col 7) → the whole `\exists`.
        let range = leader_token_range("\\exists", Position::new(0, 7))
            .expect("a `\\name` leader must be detected");
        assert_eq!(range.start, Position::new(0, 0));
        assert_eq!(range.end, Position::new(0, 7));

        // A bare `\` (col 1) still counts — replacing it consumes the leader.
        let bare = leader_token_range("\\", Position::new(0, 1))
            .expect("a bare backslash is a leader too");
        assert_eq!(bare.start, Position::new(0, 0));
        assert_eq!(bare.end, Position::new(0, 1));

        // A plain word (no backslash) is not a leader — let the editor decide.
        assert!(leader_token_range("exists", Position::new(0, 6)).is_none());
    }

    #[test]
    fn snippet_completion_consumes_leader_backslash() {
        let provider = CompletionProvider::new();
        // The user typed `\exists` and triggered completion.
        let items = provider.get_snippet_completions("\\exists", Position::new(0, 7));
        // With a leader every item (single- and multi-line bodies alike) carries
        // the edit, with no leftover insert_text a client might prefer over it.
        assert!(
            items
                .iter()
                .all(|i| i.text_edit.is_some() && i.insert_text.is_none())
        );
        let item = items
            .iter()
            .find(|i| i.label == "exists")
            .expect("the exists snippet must be offered");

        // The edit must replace the whole `\exists`, backslash included, so the
        // expanded body stands alone rather than `\∃ …` (issue #78).
        let body = rossi::snippets::SNIPPETS
            .iter()
            .find(|s| s.prefix == "exists")
            .unwrap()
            .body
            .join("\n");
        match item.text_edit.as_ref().expect("leader needs a text_edit") {
            CompletionTextEdit::Edit(edit) => {
                assert_eq!(edit.range.start, Position::new(0, 0));
                assert_eq!(edit.range.end, Position::new(0, 7));
                assert_eq!(edit.new_text, body);
            }
            other => panic!("expected a plain TextEdit, got {other:?}"),
        }
        // Filter on the backslashed form so the client still surfaces the item,
        // and keep snippet semantics for the inserted tabstops.
        assert_eq!(item.filter_text.as_deref(), Some("\\exists"));
        assert_eq!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));
    }

    #[test]
    fn test_completion_refined_variables() {
        use crate::lsp_types::Url;

        let abstract_source = "MACHINE abstract_mch\nVARIABLES\n    abstract_state\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        abstract_state := 0\n    END\nEND";
        let concrete_source = "MACHINE concrete_mch\nREFINES\n    abstract_mch\nVARIABLES\n    concrete_state\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        concrete_state := 0\n    END\nEND";

        let crm = Arc::new(CrossReferenceManager::new());
        let dm = Arc::new(DocumentManager::new());

        crm.update_component("file:///abstract_mch.eventb".to_string(), abstract_source);
        let url = Url::parse("file:///abstract_mch.eventb").unwrap();
        dm.open(url, 1, abstract_source.to_string());

        crm.update_component("file:///concrete_mch.eventb".to_string(), concrete_source);
        let concrete_url = Url::parse("file:///concrete_mch.eventb").unwrap();
        dm.open(concrete_url.clone(), 1, concrete_source.to_string());

        let mut provider = CompletionProvider::new();
        provider.set_cross_reference_manager(Arc::clone(&crm));
        provider.set_document_manager(Arc::clone(&dm));

        // Build completion context from the component in the shared parse — the
        // same source `complete` reads from.
        let parsed = dm.parse_result(&concrete_url).unwrap();
        let components = parsed.parse.component.as_deref().unwrap();
        let loader = ComponentLoader::optional(
            provider.cross_ref_manager.as_deref(),
            provider.document_manager.as_deref(),
        );
        let ctx = CompletionContext::from_component_with_refs(&components[0], loader.as_ref());

        // Should include abstract_state from refined machine
        assert!(
            ctx.variables.contains(&"abstract_state".to_string()),
            "abstract_state should appear in completions, got: {:?}",
            ctx.variables
        );
        // Should also include local concrete_state
        assert!(
            ctx.variables.contains(&"concrete_state".to_string()),
            "concrete_state should appear in completions"
        );
    }

    /// Run the full completion pipeline against a single open document and return
    /// the `(label, detail)` of every produced item — the same path an editor
    /// drives, so the scope wiring (not just the helpers) is exercised.
    fn complete_labels(source: &str, position: Position) -> Vec<(String, Option<String>)> {
        use crate::lsp_types::Url;

        let crm = Arc::new(CrossReferenceManager::new());
        let dm = Arc::new(DocumentManager::new());
        let url = Url::parse("file:///m.eventb").unwrap();
        crm.update_component(url.to_string(), source);
        dm.open(url.clone(), 1, source.to_string());

        let mut provider = CompletionProvider::new();
        provider.set_cross_reference_manager(crm);
        provider.set_document_manager(dm);

        let params = CompletionParams {
            text_document_position: crate::lsp_types::TextDocumentPositionParams {
                text_document: crate::lsp_types::TextDocumentIdentifier { uri: url },
                position,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: None,
        };

        match provider.complete(
            &params,
            source,
            &CompletionConfig::default(),
            &FormatConfig::default(),
        ) {
            Some(CompletionResponse::Array(items)) => {
                items.into_iter().map(|i| (i.label, i.detail)).collect()
            }
            _ => Vec::new(),
        }
    }

    #[test]
    fn completion_offers_the_enclosing_event_parameters() {
        // Cursor on the action line (index 10), inside event `e` whose ANY
        // clause declares `amount` (issue #102).
        let source = "MACHINE m\nVARIABLES\n    v\nEVENTS\n  EVENT e\n  ANY\n    amount\n  WHERE\n    @grd1 amount > 0\n  THEN\n    @act1 v := 0\n  END\nEND";
        let labels = complete_labels(source, Position::new(10, 8));

        assert!(
            labels
                .iter()
                .any(|(label, detail)| label == "amount" && detail.as_deref() == Some("Parameter")),
            "the event's ANY parameter `amount` must be offered, got {labels:?}"
        );
    }

    #[test]
    fn completion_does_not_offer_a_sibling_events_parameters() {
        // Two events; the cursor sits in `e1` (action line, index 8). Only `e1`'s
        // parameter is in scope — `e2`'s `p2` must not be offered.
        let source = "MACHINE m\nVARIABLES\n    v\nEVENTS\n  EVENT e1\n  ANY\n    p1\n  THEN\n    @act1 v := 0\n  END\n  EVENT e2\n  ANY\n    p2\n  THEN\n    @act2 v := 1\n  END\nEND";
        let labels = complete_labels(source, Position::new(8, 8));

        assert!(
            labels.iter().any(|(label, _)| label == "p1"),
            "the enclosing event's parameter `p1` must be offered, got {labels:?}"
        );
        assert!(
            !labels.iter().any(|(label, _)| label == "p2"),
            "a sibling event's parameter `p2` must not be offered, got {labels:?}"
        );
    }

    // The invariant `@i1 ∀ k · k > 0` is on line index 4; `k` is bound over the
    // body `k > 0`. The event action `@act1 …` on line index 8 is outside it.
    const WITH_QUANTIFIER: &str = "MACHINE m\nVARIABLES\n    v\nINVARIANTS\n    @i1 ∀ k · k > 0\nEVENTS\n    EVENT e\n    THEN\n        @act1 v := 0\n    END\nEND";

    #[test]
    fn completion_offers_in_scope_formula_binders() {
        // Cursor inside the quantifier body `k > 0`, just past the bound use `k`.
        let labels = complete_labels(WITH_QUANTIFIER, Position::new(4, 15));
        assert!(
            labels
                .iter()
                .any(|(label, detail)| label == "k" && detail.as_deref() == Some("Bound variable")),
            "the in-scope binder `k` must be offered, got {labels:?}"
        );
    }

    #[test]
    fn completion_omits_binders_outside_their_body() {
        // Cursor in the event action, outside the quantifier body — `k` is gone.
        let labels = complete_labels(WITH_QUANTIFIER, Position::new(8, 16));
        assert!(
            !labels.iter().any(|(label, _)| label == "k"),
            "a binder must not be offered outside its body, got {labels:?}"
        );
    }

    /// Build a provider whose workspace holds an abstract machine, a context,
    /// and the current machine `concrete_mch`.
    fn provider_with_workspace() -> CompletionProvider {
        let abstract_source = "MACHINE abstract_mch\nVARIABLES\n    s\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        s := 0\n    END\nEND";
        let ctx_source = "CONTEXT ctx0\nCONSTANTS\n    c\nAXIOMS\n    @a1 c = 0\nEND";
        let concrete_source = "MACHINE concrete_mch\nREFINES\n    abstract_mch\nVARIABLES\n    t\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        t := 0\n    END\nEND";

        let crm = Arc::new(CrossReferenceManager::new());
        crm.update_component("file:///abstract_mch.eventb".to_string(), abstract_source);
        crm.update_component("file:///ctx0.eventb".to_string(), ctx_source);
        crm.update_component("file:///concrete_mch.eventb".to_string(), concrete_source);

        let mut provider = CompletionProvider::new();
        provider.set_cross_reference_manager(crm);
        provider
    }

    #[test]
    fn test_component_names_filtered_by_kind_excluding_self() {
        let provider = provider_with_workspace();
        // A REFINES clause in concrete_mch: offer abstract machines only,
        // exclude concrete_mch itself, never offer a context.
        let masked = "MACHINE concrete_mch\nREFINES\n    \nEND\n";
        let labels: Vec<String> = provider
            .get_component_name_completions(masked, Position::new(2, 4), Some("concrete_mch"))
            .into_iter()
            .map(|i| i.label)
            .collect();

        assert!(
            labels.contains(&"abstract_mch".to_string()),
            "REFINES should offer the abstract machine, got {labels:?}"
        );
        assert!(
            !labels.contains(&"concrete_mch".to_string()),
            "the current component must be excluded, got {labels:?}"
        );
        assert!(
            !labels.contains(&"ctx0".to_string()),
            "REFINES must not offer a context, got {labels:?}"
        );
    }

    #[test]
    fn test_component_name_completion_spans_hyphenated_word() {
        let crm = Arc::new(CrossReferenceManager::new());
        crm.update_component(
            "file:///abstract-mch.eventb".to_string(),
            "MACHINE abstract-mch\nVARIABLES\n    s\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        s := 0\n    END\nEND",
        );
        let mut provider = CompletionProvider::new();
        provider.set_cross_reference_manager(crm);

        // REFINES target on its own indented line; cursor after `abstract-`.
        let masked = "MACHINE concrete\nREFINES\n    abstract-\nEND\n";
        let items =
            provider.get_component_name_completions(masked, Position::new(2, 13), Some("concrete"));
        let item = items
            .iter()
            .find(|i| i.label == "abstract-mch")
            .expect("hyphenated machine name must be offered in a REFINES clause");
        assert_eq!(item.kind, Some(CompletionItemKind::MODULE));

        // The edit must replace the whole hyphenated prefix `abstract-`, so the
        // editor matches across `-` rather than only the empty last segment.
        match item
            .text_edit
            .as_ref()
            .expect("component item needs a text_edit")
        {
            CompletionTextEdit::Edit(edit) => {
                assert_eq!(edit.range.start, Position::new(2, 4));
                assert_eq!(edit.range.end, Position::new(2, 13));
                assert_eq!(edit.new_text, "abstract-mch");
            }
            other => panic!("expected a plain TextEdit, got {other:?}"),
        }
    }

    #[test]
    fn hyphenated_word_range_is_utf16_after_astral() {
        // An astral `𝔹` (U+1D539 — two UTF-16 code units, one `char`) before the
        // word means the incoming UTF-16 cursor column and the emitted edit
        // range must both account for the surrogate pair, not the single char it
        // spans. LSP columns are UTF-16.
        let masked = "    𝔹 abstract-";
        // Cursor just past the trailing `-`: UTF-16 column 16
        // (4 spaces + 𝔹(2) + 1 space + "abstract-"(9)).
        let range = hyphenated_word_range(masked, Position::new(0, 16));
        // The replaced `abstract-` starts at the `a` (UTF-16 col 7), ends at 16.
        assert_eq!(range.start, Position::new(0, 7));
        assert_eq!(range.end, Position::new(0, 16));
    }

    #[test]
    fn test_component_names_not_offered_outside_reference_clause() {
        let provider = provider_with_workspace();
        // VARIABLES clause is not a component-reference position.
        let masked = "MACHINE m\nVARIABLES\n    x\nEND\n";
        let items = provider.get_component_name_completions(masked, Position::new(2, 5), Some("m"));
        assert!(
            items.is_empty(),
            "component names must not be offered outside REFINES/SEES/EXTENDS, got {items:?}"
        );
    }
}
