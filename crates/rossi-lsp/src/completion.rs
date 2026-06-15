//! Code completion provider for Event-B
//!
//! Provides intelligent auto-completion for:
//! - Keywords (context-aware based on position)
//! - Operators (Unicode and ASCII variants)
//! - Identifiers (variables, constants, sets, parameters)
//! - Snippets (common patterns like events, axioms)

use crate::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionParams, CompletionResponse, CompletionTextEdit,
    Documentation, InsertTextFormat, MarkupContent, MarkupKind, Position, Range, TextEdit,
};
use parking_lot::RwLock;
use rossi::{Component, keywords, operators};

use crate::component_util::{component_at_offset, parse_named};
use crate::identifier_utils::position_to_offset;
use crate::position::{line_run_to_range, utf16_to_char_col};
use std::collections::HashSet;
use std::sync::Arc;

use crate::cross_references::CrossReferenceManager;
use crate::document::DocumentManager;
use crate::references::component_reference_clause;

/// Configuration for completion behavior
#[derive(Debug, Clone)]
pub struct CompletionConfig {
    /// Enable completion responses
    pub enabled: bool,
    /// Use Unicode operators (∧, ∨, ⇒) instead of ASCII (/\, \/, =>)
    pub use_unicode: bool,
    /// Enable snippet completion
    pub enable_snippets: bool,
}

impl Default for CompletionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            use_unicode: true,
            enable_snippets: true,
        }
    }
}

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
}

impl CompletionContext {
    fn new() -> Self {
        Self {
            variables: Vec::new(),
            constants: Vec::new(),
            sets: Vec::new(),
            parameters: Vec::new(),
        }
    }

    fn from_component_with_refs(
        component: &Component,
        cross_ref_manager: Option<&CrossReferenceManager>,
        document_manager: Option<&DocumentManager>,
    ) -> Self {
        let mut ctx = Self::new();

        match component {
            Component::Context(context) => {
                ctx.constants
                    .extend(context.constants.iter().map(|c| c.name.clone()));
                ctx.sets
                    .extend(context.sets.iter().map(|s| s.name().to_string()));

                // Resolve EXTENDS chain transitively
                if let Some(crm) = cross_ref_manager {
                    let mut visited = HashSet::new();
                    visited.insert(context.name.clone());
                    for parent_name in &context.extends {
                        resolve_context_symbols(
                            parent_name,
                            crm,
                            document_manager,
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
                if let Some(crm) = cross_ref_manager {
                    let mut visited = HashSet::new();
                    for ctx_name in &machine.sees {
                        resolve_context_symbols(
                            ctx_name,
                            crm,
                            document_manager,
                            &mut ctx.constants,
                            &mut ctx.sets,
                            &mut visited,
                        );
                    }
                    if let Some(ref refines_name) = machine.refines {
                        resolve_machine_symbols(
                            refines_name,
                            crm,
                            document_manager,
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
}

/// Provides code completion for Event-B documents
pub struct CompletionProvider {
    config: Arc<RwLock<CompletionConfig>>,
    /// Cross-reference manager for workspace-wide navigation
    cross_ref_manager: Option<Arc<CrossReferenceManager>>,
    /// Document manager — the source of the document's shared recovered parse
    document_manager: Option<Arc<DocumentManager>>,
}

impl CompletionProvider {
    pub fn new() -> Self {
        Self {
            config: Arc::new(RwLock::new(CompletionConfig::default())),
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

    #[allow(dead_code)]
    pub fn update_config(&self, config: CompletionConfig) {
        *self.config.write() = config;
    }

    pub fn get_config(&self) -> CompletionConfig {
        self.config.read().clone()
    }

    /// Generate completion items for the given position
    pub fn complete(&self, params: &CompletionParams, text: &str) -> Option<CompletionResponse> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let config = self.get_config();
        if !config.enabled {
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
        let (completion_ctx, self_name) = parsed
            .as_deref()
            .map(|parsed| parsed.components())
            .and_then(|components| {
                let offset = position_to_offset(text, position).unwrap_or(text.len());
                component_at_offset(components, offset).map(|component| {
                    let ctx = CompletionContext::from_component_with_refs(
                        component,
                        self.cross_ref_manager.as_deref(),
                        self.document_manager.as_deref(),
                    );
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
        items.extend(self.get_operator_completions(&config, &word_at_cursor));

        // Add identifier completions
        items.extend(self.get_identifier_completions(&completion_ctx, &word_at_cursor));

        // Add component-name completions on REFINES/SEES/EXTENDS clauses, with a
        // hyphen-aware replace range so editors match/replace across `-` (#36).
        items.extend(self.get_component_name_completions(&masked, position, self_name.as_deref()));

        // Add snippet completions
        if config.enable_snippets {
            items.extend(self.get_snippet_completions(&line_text, &word_at_cursor));
        }

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
    fn get_operator_completions(
        &self,
        config: &CompletionConfig,
        _word: &str,
    ) -> Vec<CompletionItem> {
        operators::OPERATOR_SPELLINGS
            .iter()
            .filter(|entry| entry.completion)
            .map(|entry| {
                let label = entry.text(config.use_unicode);
                let alternative = entry.text(!config.use_unicode);
                let alternative = if alternative == label {
                    ""
                } else {
                    alternative
                };
                create_operator_item(label, alternative, entry.description)
            })
            .collect()
    }

    /// Get identifier completions from the current context
    fn get_identifier_completions(
        &self,
        ctx: &CompletionContext,
        _word: &str,
    ) -> Vec<CompletionItem> {
        let mut items = Vec::new();

        // Add variables
        for var in &ctx.variables {
            items.push(CompletionItem {
                label: var.clone(),
                kind: Some(CompletionItemKind::VARIABLE),
                detail: Some("Variable".to_string()),
                documentation: Some(Documentation::String(format!("State variable `{}`", var))),
                ..Default::default()
            });
        }

        // Add constants
        for constant in &ctx.constants {
            items.push(CompletionItem {
                label: constant.clone(),
                kind: Some(CompletionItemKind::CONSTANT),
                detail: Some("Constant".to_string()),
                documentation: Some(Documentation::String(format!("Constant `{}`", constant))),
                ..Default::default()
            });
        }

        // Add sets
        for set in &ctx.sets {
            items.push(CompletionItem {
                label: set.clone(),
                kind: Some(CompletionItemKind::ENUM),
                detail: Some("Set".to_string()),
                documentation: Some(Documentation::String(format!("Carrier set `{}`", set))),
                ..Default::default()
            });
        }

        // Add parameters
        for param in &ctx.parameters {
            items.push(CompletionItem {
                label: param.clone(),
                kind: Some(CompletionItemKind::VARIABLE),
                detail: Some("Parameter".to_string()),
                documentation: Some(Documentation::String(format!(
                    "Event parameter `{}`",
                    param
                ))),
                ..Default::default()
            });
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

    /// Get snippet completions for common patterns
    fn get_snippet_completions(&self, line_text: &str, _word: &str) -> Vec<CompletionItem> {
        let mut items = Vec::new();

        // Event snippet
        if is_inside_events(line_text) {
            items.push(CompletionItem {
                label: "event".to_string(),
                kind: Some(CompletionItemKind::SNIPPET),
                detail: Some("Event template".to_string()),
                documentation: Some(Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: "Insert a complete event template with guards and actions".to_string(),
                })),
                insert_text: Some("EVENT ${1:event_name}\nWHERE\n    @${2:grd1} ${3:condition}\nTHEN\n    @${4:act1} ${5:action}\nEND".to_string()),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                ..Default::default()
            });
        }

        // Axiom/Invariant snippet (labeled predicate)
        if is_inside_context(line_text) || is_inside_machine(line_text) {
            items.push(CompletionItem {
                label: "labeled_predicate".to_string(),
                kind: Some(CompletionItemKind::SNIPPET),
                detail: Some("Labeled predicate (axiom/invariant)".to_string()),
                documentation: Some(Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: "Insert a labeled predicate for axioms or invariants".to_string(),
                })),
                insert_text: Some("@${1:label} ${2:predicate}".to_string()),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                ..Default::default()
            });
        }

        // Forall quantifier snippet
        items.push(CompletionItem {
            label: "forall".to_string(),
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some("Universal quantifier".to_string()),
            documentation: Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: "Insert a universal quantifier (∀ x · P ⇒ Q)".to_string(),
            })),
            insert_text: Some("∀ ${1:x} · ${2:x ∈ S} ⇒ ${3:predicate}".to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        });

        // Exists quantifier snippet
        items.push(CompletionItem {
            label: "exists".to_string(),
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some("Existential quantifier".to_string()),
            documentation: Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: "Insert an existential quantifier (∃ x · P)".to_string(),
            })),
            insert_text: Some("∃ ${1:x} · ${2:predicate}".to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        });

        items
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
    cross_ref_manager: &CrossReferenceManager,
    document_manager: Option<&DocumentManager>,
    constants: &mut Vec<String>,
    sets: &mut Vec<String>,
    visited: &mut HashSet<String>,
) {
    // Cycle/depth guard
    if visited.len() >= 10 || visited.contains(context_name) {
        return;
    }
    visited.insert(context_name.to_string());

    let text = match cross_ref_manager.load_component_text(context_name, document_manager) {
        Some(t) => t,
        None => return,
    };

    let component = match parse_named(&text, context_name) {
        Some(c) => c,
        None => return,
    };

    if let Component::Context(ctx) = &component {
        constants.extend(ctx.constants.iter().map(|c| c.name.clone()));
        sets.extend(ctx.sets.iter().map(|s| s.name().to_string()));

        // Recursively resolve EXTENDS parents
        for parent_name in &ctx.extends {
            resolve_context_symbols(
                parent_name,
                cross_ref_manager,
                document_manager,
                constants,
                sets,
                visited,
            );
        }
    }
}

/// Resolve variables from a refined machine (and its transitive REFINES/SEES dependencies).
fn resolve_machine_symbols(
    machine_name: &str,
    cross_ref_manager: &CrossReferenceManager,
    document_manager: Option<&DocumentManager>,
    variables: &mut Vec<String>,
    constants: &mut Vec<String>,
    sets: &mut Vec<String>,
    visited: &mut HashSet<String>,
) {
    if visited.len() >= 10 || visited.contains(machine_name) {
        return;
    }
    visited.insert(machine_name.to_string());

    let text = match cross_ref_manager.load_component_text(machine_name, document_manager) {
        Some(t) => t,
        None => return,
    };

    let component = match parse_named(&text, machine_name) {
        Some(c) => c,
        None => return,
    };

    if let Component::Machine(m) = &component {
        variables.extend(m.variables.iter().map(|v| v.name.clone()));

        // Resolve SEES contexts from the abstract machine
        for ctx_name in &m.sees {
            resolve_context_symbols(
                ctx_name,
                cross_ref_manager,
                document_manager,
                constants,
                sets,
                visited,
            );
        }

        // Recurse into further refinements
        if let Some(ref refines_name) = m.refines {
            resolve_machine_symbols(
                refines_name,
                cross_ref_manager,
                document_manager,
                variables,
                constants,
                sets,
                visited,
            );
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
/// (`keywords::is_structural_word_char`: ASCII alphanumerics, `_`, `'`, `-`) —
/// the same charset the grammar accepts — so it covers a trailing `'` and never
/// extends across a character that can't be part of a name.
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
    fn test_completion_provider_creation() {
        let provider = CompletionProvider::new();
        let config = provider.get_config();
        assert!(config.enabled);
        assert!(config.use_unicode);
        assert!(config.enable_snippets);
    }

    #[test]
    fn test_config_update() {
        let provider = CompletionProvider::new();
        let new_config = CompletionConfig {
            enabled: false,
            use_unicode: false,
            enable_snippets: false,
        };
        provider.update_config(new_config);
        let config = provider.get_config();
        assert!(!config.enabled);
        assert!(!config.use_unicode);
        assert!(!config.enable_snippets);
    }

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

        assert!(provider.complete(&params, text).is_none());
    }

    #[test]
    fn test_operator_completions_unicode() {
        let provider = CompletionProvider::new();
        let config = CompletionConfig {
            enabled: true,
            use_unicode: true,
            enable_snippets: true,
        };
        let items = provider.get_operator_completions(&config, "");

        // Should include Unicode operators
        assert!(items.iter().any(|item| item.label == "∧"));
        assert!(items.iter().any(|item| item.label == "∨"));
        assert!(items.iter().any(|item| item.label == "⇒"));
        assert!(items.iter().any(|item| item.label == "∈"));
        assert!(items.iter().any(|item| item.label == "⊈"));
        assert!(
            items
                .iter()
                .any(|item| item.label == operators::TOTAL_RELATION)
        );
        assert!(
            items
                .iter()
                .any(|item| item.label == operators::RELATIONAL_OVERRIDE)
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
        let config = CompletionConfig {
            enabled: true,
            use_unicode: false,
            enable_snippets: true,
        };
        let items = provider.get_operator_completions(&config, "");

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
        };

        let items = provider.get_identifier_completions(&ctx, "");

        assert!(items.iter().any(|item| item.label == "count"));
        assert!(items.iter().any(|item| item.label == "total"));
        assert!(items.iter().any(|item| item.label == "max_value"));
        assert!(items.iter().any(|item| item.label == "STATUS"));
        assert!(items.iter().any(|item| item.label == "x"));
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
        let items = provider.get_snippet_completions("EVENTS", "");

        assert!(items.iter().any(|item| item.label == "event"));
        assert!(items.iter().any(|item| item.label == "forall"));
        assert!(items.iter().any(|item| item.label == "exists"));
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
        dm.open(url, "rossi".to_string(), 1, abstract_source.to_string());

        crm.update_component("file:///concrete_mch.eventb".to_string(), concrete_source);
        let concrete_url = Url::parse("file:///concrete_mch.eventb").unwrap();
        dm.open(
            concrete_url.clone(),
            "rossi".to_string(),
            1,
            concrete_source.to_string(),
        );

        let mut provider = CompletionProvider::new();
        provider.set_cross_reference_manager(Arc::clone(&crm));
        provider.set_document_manager(Arc::clone(&dm));

        // Build completion context from the component in the shared parse — the
        // same source `complete` reads from.
        let parsed = dm.parse_result(&concrete_url).unwrap();
        let components = parsed.parse.component.as_deref().unwrap();
        let ctx = CompletionContext::from_component_with_refs(
            &components[0],
            provider.cross_ref_manager.as_deref(),
            provider.document_manager.as_deref(),
        );

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
