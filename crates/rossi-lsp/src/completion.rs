//! Code completion provider for Event-B
//!
//! Provides intelligent auto-completion for:
//! - Keywords (context-aware based on position)
//! - Operators (Unicode and ASCII variants)
//! - Identifiers (variables, constants, sets, parameters)
//! - Snippets (common patterns like events, axioms)

use crate::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionParams, CompletionResponse, Documentation,
    InsertTextFormat, MarkupContent, MarkupKind, Position,
};
use dashmap::DashMap;
use parking_lot::RwLock;
use rossi::{Component, keywords, operators};

use crate::component_util::{component_at_offset, parse_all, parse_named};
use crate::identifier_utils::position_to_offset;
use std::collections::HashSet;
use std::sync::Arc;

use crate::cross_references::CrossReferenceManager;
use crate::document::DocumentManager;

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
    /// Cache of parsed components for quick completion
    component_cache: Arc<DashMap<String, Vec<Component>>>,
    /// Cross-reference manager for workspace-wide navigation
    cross_ref_manager: Option<Arc<CrossReferenceManager>>,
    /// Document manager to access open documents
    document_manager: Option<Arc<DocumentManager>>,
}

impl CompletionProvider {
    pub fn new() -> Self {
        Self {
            config: Arc::new(RwLock::new(CompletionConfig::default())),
            component_cache: Arc::new(DashMap::new()),
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

    /// Update the cached components for a document
    pub fn update_component(&self, uri: String, text: &str) {
        let components = parse_all(text);
        if !components.is_empty() {
            self.component_cache.insert(uri, components);
        } else {
            // Remove from cache if parsing fails
            self.component_cache.remove(&uri);
        }
    }

    /// Generate completion items for the given position
    pub fn complete(&self, params: &CompletionParams, text: &str) -> Option<CompletionResponse> {
        let uri = params.text_document_position.text_document.uri.to_string();
        let position = params.text_document_position.position;
        let config = self.get_config();
        if !config.enabled {
            return None;
        }

        // Get completion context from the cached component under the cursor
        let completion_ctx = self
            .component_cache
            .get(&uri)
            .and_then(|entry| {
                let offset = position_to_offset(text, position).unwrap_or(text.len());
                component_at_offset(&entry, offset).map(|component| {
                    CompletionContext::from_component_with_refs(
                        component,
                        self.cross_ref_manager.as_deref(),
                        self.document_manager.as_deref(),
                    )
                })
            })
            .unwrap_or_else(CompletionContext::new);

        // Determine what to complete based on context
        let mut items = Vec::new();

        // Analyze the text to determine context
        let line_text = get_line_text(text, position);
        let word_at_cursor = get_word_at_position(&line_text, position.character as usize);

        // Add keyword completions
        items.extend(self.get_keyword_completions(&line_text, &word_at_cursor));

        // Add operator completions
        items.extend(self.get_operator_completions(&config, &word_at_cursor));

        // Add identifier completions
        items.extend(self.get_identifier_completions(&completion_ctx, &word_at_cursor));

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
    let before_cursor = &line[..char_pos.min(line.len())];
    before_cursor
        .split_whitespace()
        .last()
        .unwrap_or("")
        .to_string()
}

// Context detection functions

fn is_top_level_context(line_text: &str) -> bool {
    let trimmed = line_text.trim();
    trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with("/*")
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
    fn test_component_caching() {
        let provider = CompletionProvider::new();
        let source = "CONTEXT test\nCONSTANTS\n    max_value\nEND";

        provider.update_component("file:///test.eventb".to_string(), source);

        assert!(provider.component_cache.contains_key("file:///test.eventb"));
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
        let url = Url::parse("file:///concrete_mch.eventb").unwrap();
        dm.open(url, "rossi".to_string(), 1, concrete_source.to_string());

        let mut provider = CompletionProvider::new();
        provider.set_cross_reference_manager(Arc::clone(&crm));
        provider.set_document_manager(Arc::clone(&dm));
        provider.update_component("file:///concrete_mch.eventb".to_string(), concrete_source);

        // Build completion context from the cached component
        let ctx = provider
            .component_cache
            .get("file:///concrete_mch.eventb")
            .map(|entry| {
                CompletionContext::from_component_with_refs(
                    &entry[0],
                    provider.cross_ref_manager.as_deref(),
                    provider.document_manager.as_deref(),
                )
            })
            .unwrap();

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
}
