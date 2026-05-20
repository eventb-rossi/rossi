//! Semantic tokens provider for enhanced syntax highlighting
//!
//! This module implements `textDocument/semanticTokens/full` for Event-B files.
//! Semantic tokens provide more accurate syntax highlighting by analyzing the AST
//! rather than relying solely on TextMate grammars.

use crate::lsp_types::{
    SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens, SemanticTokensLegend,
    SemanticTokensParams, SemanticTokensResult,
};
use rossi::ast::{Component, Context, Event, LabeledAction, LabeledPredicate, Machine, Predicate};
use tracing::debug;

/// Semantic tokens provider
pub struct SemanticTokensProvider;

impl Default for SemanticTokensProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SemanticTokensProvider {
    /// Create a new semantic tokens provider
    pub fn new() -> Self {
        Self
    }

    /// Get semantic tokens legend (token types and modifiers)
    pub fn legend() -> SemanticTokensLegend {
        SemanticTokensLegend {
            token_types: vec![
                SemanticTokenType::KEYWORD,
                SemanticTokenType::VARIABLE,
                SemanticTokenType::PARAMETER,
                SemanticTokenType::PROPERTY,
                SemanticTokenType::FUNCTION,
                SemanticTokenType::OPERATOR,
                SemanticTokenType::TYPE,
                SemanticTokenType::NAMESPACE,
                SemanticTokenType::MACRO, // Used for labels
                SemanticTokenType::COMMENT,
                SemanticTokenType::STRING,
                SemanticTokenType::NUMBER,
            ],
            token_modifiers: vec![
                SemanticTokenModifier::DECLARATION,
                SemanticTokenModifier::READONLY,
                SemanticTokenModifier::DEFINITION,
            ],
        }
    }

    /// Provide semantic tokens for a document
    pub fn semantic_tokens(
        &self,
        _params: &SemanticTokensParams,
        text: &str,
    ) -> Option<SemanticTokensResult> {
        // Parse the document
        let component = match rossi::parse(text) {
            Ok(comp) => comp,
            Err(e) => {
                debug!("Failed to parse document for semantic tokens: {}", e);
                return None;
            }
        };

        // Extract semantic tokens from the AST
        let mut builder = SemanticTokensBuilder::new(text);

        match &component {
            Component::Context(ctx) => builder.visit_context(ctx),
            Component::Machine(mch) => builder.visit_machine(mch),
        }

        let tokens = builder.build();

        debug!("Generated {} semantic tokens", tokens.data.len() / 5);

        Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data: tokens.data,
        }))
    }
}

/// Semantic token builder that traverses the AST and generates tokens
struct SemanticTokensBuilder<'a> {
    /// Source text for position calculation
    text: &'a str,
    /// Line offsets for quick position calculation
    line_offsets: Vec<usize>,
    /// Collected semantic tokens
    tokens: Vec<SemanticTokenData>,
    /// Declared variables for tracking
    variables: Vec<String>,
    /// Declared constants for tracking
    constants: Vec<String>,
    /// Declared sets for tracking
    sets: Vec<String>,
    /// Event parameters for tracking
    parameters: Vec<String>,
}

/// Internal representation of a semantic token with absolute position
#[derive(Debug, Clone)]
struct SemanticTokenData {
    line: u32,
    start: u32,
    length: u32,
    token_type: u32,
    token_modifiers: u32,
}

impl<'a> SemanticTokensBuilder<'a> {
    fn new(text: &'a str) -> Self {
        // Calculate line offsets for quick position lookup
        let mut line_offsets = vec![0];
        for (i, c) in text.char_indices() {
            if c == '\n' {
                line_offsets.push(i + 1);
            }
        }

        Self {
            text,
            line_offsets,
            tokens: Vec::new(),
            variables: Vec::new(),
            constants: Vec::new(),
            sets: Vec::new(),
            parameters: Vec::new(),
        }
    }

    /// Find position (line, column) from byte offset
    fn position_from_offset(&self, offset: usize) -> (u32, u32) {
        for (line_num, &line_start) in self.line_offsets.iter().enumerate() {
            if offset < line_start {
                let prev_line = line_num.saturating_sub(1);
                let prev_start = self.line_offsets[prev_line];
                let column = offset - prev_start;
                return (prev_line as u32, column as u32);
            }
        }
        // Last line
        let last_line = self.line_offsets.len().saturating_sub(1);
        let last_start = self.line_offsets[last_line];
        let column = offset - last_start;
        (last_line as u32, column as u32)
    }

    /// Find the position of a keyword in the text
    fn find_keyword(&self, keyword: &str, start_offset: usize) -> Option<(usize, usize)> {
        if let Some(pos) = self.text[start_offset..].find(keyword) {
            let offset = start_offset + pos;
            Some((offset, keyword.len()))
        } else {
            None
        }
    }

    /// Find the position of an identifier in the text
    fn find_identifier(&self, identifier: &str, start_offset: usize) -> Option<(usize, usize)> {
        // Look for the identifier as a whole word
        let search_text = &self.text[start_offset..];
        if let Some(pos) = search_text.find(identifier) {
            let offset = start_offset + pos;
            Some((offset, identifier.len()))
        } else {
            None
        }
    }

    /// Add a keyword token
    fn add_keyword(&mut self, keyword: &str, offset: usize) {
        let (line, start) = self.position_from_offset(offset);
        self.tokens.push(SemanticTokenData {
            line,
            start,
            length: keyword.len() as u32,
            token_type: TokenType::Keyword as u32,
            token_modifiers: 0,
        });
    }

    /// Add an identifier token
    fn add_identifier(
        &mut self,
        identifier: &str,
        offset: usize,
        token_type: TokenType,
        is_declaration: bool,
    ) {
        let (line, start) = self.position_from_offset(offset);
        let mut modifiers = 0;
        if is_declaration {
            modifiers |= 1 << TokenModifier::Declaration as u32;
        }
        if matches!(token_type, TokenType::Constant | TokenType::Set) {
            modifiers |= 1 << TokenModifier::Readonly as u32;
        }

        self.tokens.push(SemanticTokenData {
            line,
            start,
            length: identifier.len() as u32,
            token_type: token_type as u32,
            token_modifiers: modifiers,
        });
    }

    /// Visit a context
    fn visit_context(&mut self, ctx: &Context) {
        let mut current_offset = 0;

        // CONTEXT keyword
        if let Some((offset, _)) = self.find_keyword("CONTEXT", current_offset) {
            self.add_keyword("CONTEXT", offset);
            current_offset = offset + 7;
        }

        // Context name
        if let Some((offset, _)) = self.find_identifier(&ctx.name, current_offset) {
            self.add_identifier(&ctx.name, offset, TokenType::Namespace, true);
            current_offset = offset + ctx.name.len();
        }

        // EXTENDS clause
        if !ctx.extends.is_empty()
            && let Some((offset, _)) = self.find_keyword("EXTENDS", current_offset)
        {
            self.add_keyword("EXTENDS", offset);
            current_offset = offset + 7;

            for extended in &ctx.extends {
                if let Some((offset, _)) = self.find_identifier(extended, current_offset) {
                    self.add_identifier(extended, offset, TokenType::Namespace, false);
                    current_offset = offset + extended.len();
                }
            }
        }

        // SETS clause
        if !ctx.sets.is_empty()
            && let Some((offset, _)) = self.find_keyword("SETS", current_offset)
        {
            self.add_keyword("SETS", offset);
            current_offset = offset + 4;

            for set in &ctx.sets {
                let set_name = set.name().to_string();
                self.sets.push(set_name.clone());
                if let Some((offset, _)) = self.find_identifier(&set_name, current_offset) {
                    self.add_identifier(&set_name, offset, TokenType::Set, true);
                    current_offset = offset + set_name.len();
                }
            }
        }

        // CONSTANTS clause
        if !ctx.constants.is_empty()
            && let Some((offset, _)) = self.find_keyword("CONSTANTS", current_offset)
        {
            self.add_keyword("CONSTANTS", offset);
            current_offset = offset + 9;

            for constant in &ctx.constants {
                self.constants.push(constant.name.clone());
                if let Some((offset, _)) = self.find_identifier(&constant.name, current_offset) {
                    self.add_identifier(&constant.name, offset, TokenType::Constant, true);
                    current_offset = offset + constant.name.len();
                }
            }
        }

        // AXIOMS clause
        if !ctx.axioms.is_empty()
            && let Some((offset, _)) = self.find_keyword("AXIOMS", current_offset)
        {
            self.add_keyword("AXIOMS", offset);
            current_offset = offset + 6;

            for axiom in &ctx.axioms {
                current_offset = self.visit_labeled_predicate(axiom, current_offset);
            }
        }

        // END keyword
        if let Some((offset, _)) = self.find_keyword("END", current_offset) {
            self.add_keyword("END", offset);
        }
    }

    /// Visit a machine
    fn visit_machine(&mut self, mch: &Machine) {
        let mut current_offset = 0;

        // MACHINE keyword
        if let Some((offset, _)) = self.find_keyword("MACHINE", current_offset) {
            self.add_keyword("MACHINE", offset);
            current_offset = offset + 7;
        }

        // Machine name
        if let Some((offset, _)) = self.find_identifier(&mch.name, current_offset) {
            self.add_identifier(&mch.name, offset, TokenType::Namespace, true);
            current_offset = offset + mch.name.len();
        }

        // REFINES clause
        if let Some(ref refined) = mch.refines
            && let Some((offset, _)) = self.find_keyword("REFINES", current_offset)
        {
            self.add_keyword("REFINES", offset);
            current_offset = offset + 7;

            if let Some((offset, _)) = self.find_identifier(refined, current_offset) {
                self.add_identifier(refined, offset, TokenType::Namespace, false);
                current_offset = offset + refined.len();
            }
        }

        // SEES clause
        if !mch.sees.is_empty()
            && let Some((offset, _)) = self.find_keyword("SEES", current_offset)
        {
            self.add_keyword("SEES", offset);
            current_offset = offset + 4;

            for seen in &mch.sees {
                if let Some((offset, _)) = self.find_identifier(seen, current_offset) {
                    self.add_identifier(seen, offset, TokenType::Namespace, false);
                    current_offset = offset + seen.len();
                }
            }
        }

        // VARIABLES clause
        if !mch.variables.is_empty()
            && let Some((offset, _)) = self.find_keyword("VARIABLES", current_offset)
        {
            self.add_keyword("VARIABLES", offset);
            current_offset = offset + 9;

            for variable in &mch.variables {
                self.variables.push(variable.name.clone());
                if let Some((offset, _)) = self.find_identifier(&variable.name, current_offset) {
                    self.add_identifier(&variable.name, offset, TokenType::Variable, true);
                    current_offset = offset + variable.name.len();
                }
            }
        }

        // INVARIANTS clause
        if !mch.invariants.is_empty()
            && let Some((offset, _)) = self.find_keyword("INVARIANTS", current_offset)
        {
            self.add_keyword("INVARIANTS", offset);
            current_offset = offset + 10;

            for invariant in &mch.invariants {
                current_offset = self.visit_labeled_predicate(invariant, current_offset);
            }
        }

        // VARIANT clause
        if mch.variant.is_some()
            && let Some((offset, _)) = self.find_keyword("VARIANT", current_offset)
        {
            self.add_keyword("VARIANT", offset);
            current_offset = offset + 7;
        }

        // INITIALISATION event
        if let Some(init) = &mch.initialisation
            && let Some((offset, _)) = self.find_keyword("INITIALISATION", current_offset)
        {
            self.add_keyword("INITIALISATION", offset);
            current_offset = offset + 14;

            // Actions
            for action in &init.actions {
                current_offset = self.visit_action(action, current_offset);
            }

            if let Some((offset, _)) = self.find_keyword("END", current_offset) {
                self.add_keyword("END", offset);
                current_offset = offset + 3;
            }
        }

        // EVENTS clause
        if !mch.events.is_empty()
            && let Some((offset, _)) = self.find_keyword("EVENTS", current_offset)
        {
            self.add_keyword("EVENTS", offset);
            current_offset = offset + 6;

            for event in &mch.events {
                current_offset = self.visit_event(event, current_offset);
            }
        }

        // END keyword
        if let Some((offset, _)) = self.find_keyword("END", current_offset) {
            self.add_keyword("END", offset);
        }
    }

    /// Visit an event
    fn visit_event(&mut self, event: &Event, mut current_offset: usize) -> usize {
        // Clear event-specific parameters
        self.parameters.clear();

        // EVENT keyword
        if let Some((offset, _)) = self.find_keyword("EVENT", current_offset) {
            self.add_keyword("EVENT", offset);
            current_offset = offset + 5;
        }

        // Event name
        if let Some((offset, _)) = self.find_identifier(&event.name, current_offset) {
            self.add_identifier(&event.name, offset, TokenType::Function, true);
            current_offset = offset + event.name.len();
        }

        // Status keywords (convergent, anticipated)
        if let Some(status) = event.status {
            match status {
                rossi::ast::EventStatus::Convergent => {
                    if let Some((offset, _)) = self.find_keyword("CONVERGENT", current_offset) {
                        self.add_keyword("CONVERGENT", offset);
                        current_offset = offset + 10;
                    }
                }
                rossi::ast::EventStatus::Anticipated => {
                    if let Some((offset, _)) = self.find_keyword("ANTICIPATED", current_offset) {
                        self.add_keyword("ANTICIPATED", offset);
                        current_offset = offset + 11;
                    }
                }
                _ => {}
            }
        }

        // REFINES clause
        if event.refines.is_some()
            && let Some((offset, _)) = self.find_keyword("REFINES", current_offset)
        {
            self.add_keyword("REFINES", offset);
            current_offset = offset + 7;
        }

        // ANY clause (parameters)
        if !event.parameters.is_empty()
            && let Some((offset, _)) = self.find_keyword("ANY", current_offset)
        {
            self.add_keyword("ANY", offset);
            current_offset = offset + 3;

            for param in &event.parameters {
                self.parameters.push(param.name.clone());
                if let Some((offset, _)) = self.find_identifier(&param.name, current_offset) {
                    self.add_identifier(&param.name, offset, TokenType::Parameter, true);
                    current_offset = offset + param.name.len();
                }
            }
        }

        // WHERE/WHEN clause (guards)
        if !event.guards.is_empty() {
            // Try WHERE first, then WHEN
            if let Some((offset, _)) = self.find_keyword("WHERE", current_offset) {
                self.add_keyword("WHERE", offset);
                current_offset = offset + 5;
            } else if let Some((offset, _)) = self.find_keyword("WHEN", current_offset) {
                self.add_keyword("WHEN", offset);
                current_offset = offset + 4;
            }

            for guard in &event.guards {
                current_offset = self.visit_labeled_predicate(guard, current_offset);
            }
        }

        // WITH clause (labeled predicates)
        if !event.with.is_empty() {
            if let Some((offset, _)) = self.find_keyword("WITH", current_offset) {
                self.add_keyword("WITH", offset);
                current_offset = offset + 4;
            }

            for lp in &event.with {
                current_offset = self.visit_labeled_predicate(lp, current_offset);
            }
        }

        // WITNESS clause (labeled predicates)
        if !event.witnesses.is_empty() {
            if let Some((offset, _)) = self.find_keyword("WITNESS", current_offset) {
                self.add_keyword("WITNESS", offset);
                current_offset = offset + 7;
            }

            for lp in &event.witnesses {
                current_offset = self.visit_labeled_predicate(lp, current_offset);
            }
        }

        // THEN/BEGIN clause (actions)
        if !event.actions.is_empty() {
            // Try THEN first, then BEGIN
            if let Some((offset, _)) = self.find_keyword("THEN", current_offset) {
                self.add_keyword("THEN", offset);
                current_offset = offset + 4;
            } else if let Some((offset, _)) = self.find_keyword("BEGIN", current_offset) {
                self.add_keyword("BEGIN", offset);
                current_offset = offset + 5;
            }

            for action in &event.actions {
                current_offset = self.visit_action(action, current_offset);
            }
        }

        // END keyword
        if let Some((offset, _)) = self.find_keyword("END", current_offset) {
            self.add_keyword("END", offset);
            current_offset = offset + 3;
        }

        current_offset
    }

    /// Visit a labeled predicate
    fn visit_labeled_predicate(
        &mut self,
        lp: &LabeledPredicate,
        mut current_offset: usize,
    ) -> usize {
        // Label
        if let Some(label) = &lp.label
            && let Some((offset, _)) = self.find_identifier(label, current_offset)
        {
            self.add_identifier(label, offset, TokenType::Label, false);
            current_offset = offset + label.len();
        }

        // Predicate - we'd need to traverse it to find identifiers
        current_offset = self.visit_predicate(&lp.predicate, current_offset);

        current_offset
    }

    /// Visit a predicate (simplified - just looks for identifiers)
    fn visit_predicate(&mut self, _predicate: &Predicate, current_offset: usize) -> usize {
        // This is a simplified implementation
        // A full implementation would recursively traverse the predicate AST
        // and mark all identifiers based on their context
        // For now, we just return the current offset as we don't have a simple
        // Identifier variant in the Predicate enum
        current_offset
    }

    /// Visit a labeled action (simplified)
    fn visit_action(&mut self, labeled_action: &LabeledAction, mut current_offset: usize) -> usize {
        // Label
        if let Some(label) = &labeled_action.label
            && let Some((offset, _)) = self.find_identifier(label, current_offset)
        {
            self.add_identifier(label, offset, TokenType::Label, false);
            current_offset = offset + label.len();
        }

        // Simplified implementation for the action itself
        // A full implementation would traverse the action AST to mark variables
        current_offset
    }

    /// Build the final semantic tokens
    fn build(mut self) -> SemanticTokens {
        // Sort tokens by position (line, then column)
        self.tokens.sort_by_key(|t| (t.line, t.start));

        // Convert to LSP delta-encoded format using SemanticToken structs
        let mut data = Vec::new();
        let mut prev_line = 0;
        let mut prev_start = 0;

        for token in self.tokens {
            let delta_line = token.line - prev_line;
            let delta_start = if delta_line == 0 {
                token.start - prev_start
            } else {
                token.start
            };

            data.push(SemanticToken {
                delta_line,
                delta_start,
                length: token.length,
                token_type: token.token_type,
                token_modifiers_bitset: token.token_modifiers,
            });

            prev_line = token.line;
            prev_start = token.start;
        }

        SemanticTokens {
            result_id: None,
            data,
        }
    }
}

/// Token type indices (must match the legend order)
#[repr(u32)]
#[allow(dead_code)]
enum TokenType {
    Keyword = 0,
    Variable = 1,
    Parameter = 2,
    Property = 3,
    Function = 4,
    Operator = 5,
    Set = 6, // Using TYPE semantic token
    Namespace = 7,
    Label = 8, // Using MACRO semantic token
    Comment = 9,
    String = 10,
    Constant = 11, // Using NUMBER semantic token
}

/// Token modifier bit indices
#[repr(u32)]
#[allow(dead_code)]
enum TokenModifier {
    Declaration = 0,
    Readonly = 1,
    Definition = 2,
}
