//! Semantic tokens provider for enhanced syntax highlighting
//!
//! This module implements `textDocument/semanticTokens/full` for Event-B files.
//! Semantic tokens provide more accurate syntax highlighting by analyzing the AST
//! rather than relying solely on TextMate grammars.

use crate::lsp_types::{
    SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens, SemanticTokensLegend,
    SemanticTokensParams, SemanticTokensResult,
};
use rossi::ast::{
    Component, Context, Event, LabeledAction, LabeledPredicate, Machine, Predicate, Span,
};
use rossi::comments::comment_spans;
use rossi::keywords::{self, KeywordId};
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
        // Parse the document, falling back to error recovery: highlighting
        // must not vanish (and flicker back to the TextMate grammar with the
        // issue-#24 comment bug) on every mid-edit keystroke.
        let parsed = rossi::parse_components_with_recovery(text);
        let Some(components) = parsed.component else {
            debug!(
                "Failed to parse document for semantic tokens: {:?}",
                parsed.errors.first()
            );
            return None;
        };

        // Extract semantic tokens from the AST, one component at a time
        let mut builder = SemanticTokensBuilder::new(text);

        for component in &components {
            match component {
                Component::Context(ctx) => builder.visit_context(ctx),
                Component::Machine(mch) => builder.visit_machine(mch),
            }
        }

        builder.emit_comment_tokens();

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
    /// ASCII-lowercased copy of `text` for case-insensitive keyword search.
    /// ASCII-lowercasing preserves byte length, so offsets match `text`.
    text_lower: String,
    /// Line offsets for quick position calculation
    line_offsets: Vec<usize>,
    /// Byte spans of all comments, sorted and disjoint. Keyword/identifier
    /// searches must never match inside these (issue #24).
    comment_spans: Vec<Span>,
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
            text_lower: text.to_ascii_lowercase(),
            line_offsets,
            comment_spans: comment_spans(text),
            tokens: Vec::new(),
            variables: Vec::new(),
            constants: Vec::new(),
            sets: Vec::new(),
            parameters: Vec::new(),
        }
    }

    /// If `offset` falls inside a comment, the offset just past that comment
    /// (where a search should resume).
    fn comment_end_after(&self, offset: usize) -> Option<usize> {
        rossi::comments::span_containing(&self.comment_spans, offset).map(|s| s.end)
    }

    /// Find position (line, column) from byte offset.
    ///
    /// The column is in characters, not bytes — the convention used by the
    /// rest of this crate, and equal to UTF-16 code units (the LSP default
    /// encoding) for all of Event-B's BMP symbols. Byte columns would place
    /// tokens after a Unicode operator (`x ∈ ℕ // note`) too far right.
    fn position_from_offset(&self, offset: usize) -> (u32, u32) {
        // line_offsets[0] == 0 <= offset, so the partition point is >= 1.
        let line = self.line_offsets.partition_point(|&start| start <= offset) - 1;
        let column = self.text[self.line_offsets[line]..offset].chars().count();
        (line as u32, column as u32)
    }

    /// Find `needle` in `haystack` (a same-length view of the source, byte
    /// offsets interchangeable) starting at `start_offset`, skipping matches
    /// inside comments and matches that are part of a longer word (e.g. `end`
    /// inside `extended`). `bounded` is the word-boundary rule: the structural
    /// rule (where `-` is part of a word) for keywords and hyphenated component
    /// names — so the `end` of `end-update` is not a whole word — and the math
    /// rule for plain identifiers, where `-` is the subtraction operator. This
    /// mirrors the per-needle choice in `references.rs` (`WordBoundary::for_name`).
    fn find_word(
        &self,
        haystack: &str,
        needle: &str,
        start_offset: usize,
        bounded: fn(&str, usize, usize) -> bool,
    ) -> Option<usize> {
        // A needle may start with a non-ASCII char (`label_text` accepts any
        // non-whitespace), so advance by whole chars to stay on boundaries.
        let step = needle.chars().next().map_or(1, char::len_utf8);
        let mut from = start_offset;
        while let Some(pos) = haystack[from..].find(needle) {
            let offset = from + pos;
            if let Some(end) = self.comment_end_after(offset) {
                from = end;
            } else if !bounded(self.text, offset, needle.len()) {
                from = offset + step;
            } else {
                return Some(offset);
            }
        }
        None
    }

    /// Find the position of a keyword in the text (case-insensitive).
    ///
    /// Searches the precomputed `text_lower`; since ASCII-lowercasing preserves
    /// byte offsets, the result maps back onto `text` and the match length
    /// equals `keyword.len()`.
    fn find_keyword(&self, keyword: &str, start_offset: usize) -> Option<(usize, usize)> {
        let needle = keyword.to_ascii_lowercase();
        // Keywords are structural: a `-` next to a keyword only ever occurs
        // inside a component name (`end-update`), never as a real keyword, so
        // the structural boundary keeps the scan off those fragments.
        self.find_word(
            &self.text_lower,
            &needle,
            start_offset,
            keywords::is_structural_word_bounded,
        )
        .map(|offset| (offset, keyword.len()))
    }

    /// Find the keyword identified by `id` from `current_offset`, emit a token
    /// for the spelling that matched, and return the offset just past it.
    /// Tries each spelling in order (so `WHERE`/`WHEN`, `THEN`/`BEGIN` are both
    /// handled), and returns `None` if no spelling is found.
    fn mark_keyword(&mut self, id: KeywordId, current_offset: usize) -> Option<usize> {
        for spelling in keywords::keyword(id).spellings {
            if let Some((offset, len)) = self.find_keyword(spelling, current_offset) {
                self.add_keyword(spelling, offset);
                return Some(offset + len);
            }
        }
        None
    }

    /// Emit a token for keyword `id` and advance `offset` past it in place
    /// (leaving `offset` unchanged if the keyword is not found).
    fn advance_past_keyword(&mut self, id: KeywordId, offset: &mut usize) {
        if let Some(next) = self.mark_keyword(id, *offset) {
            *offset = next;
        }
    }

    /// Like [`Self::mark_keyword`] but only marks a spelling that ends at or before
    /// `bound`, returning the offset just past it (or `from` unchanged if none).
    ///
    /// Used to color a `THEOREMS` header that sits between two predicates: a
    /// THEOREMS section folds into the axioms/invariants vec with `is_theorem = true`
    /// (Rodin models a theorem as a flagged axiom/invariant, so there is no THEOREMS
    /// AST node to drive the walk). Bounding by the next predicate's start keeps the
    /// search from grabbing a header that belongs to a later predicate.
    fn mark_keyword_within(&mut self, id: KeywordId, from: usize, bound: usize) -> usize {
        for spelling in keywords::keyword(id).spellings {
            if let Some((offset, len)) = self.find_keyword(spelling, from)
                && offset + len <= bound
            {
                self.add_keyword(spelling, offset);
                return offset + len;
            }
        }
        from
    }

    /// Find the position of an identifier in the text, as a whole word and
    /// never inside a comment. The boundary rule depends on the needle: a
    /// hyphenated component name (`do-step`) takes the structural boundary, a
    /// plain math identifier the math boundary (see [`keywords::word_bounded_for_name`]).
    fn find_identifier(&self, identifier: &str, start_offset: usize) -> Option<(usize, usize)> {
        self.find_word(
            self.text,
            identifier,
            start_offset,
            keywords::word_bounded_for_name(identifier),
        )
        .map(|offset| (offset, identifier.len()))
    }

    /// Add a keyword token (keywords are ASCII, so byte length == char length)
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
            // In characters: identifiers are ASCII, but labels accept any
            // non-whitespace char.
            length: identifier.chars().count() as u32,
            token_type: token_type as u32,
            token_modifiers: modifiers,
        });
    }

    /// Clear the per-component declared-name tracking so identifiers declared
    /// in one component are not colored as such inside a sibling component.
    fn reset_component_state(&mut self) {
        self.variables.clear();
        self.constants.clear();
        self.sets.clear();
        self.parameters.clear();
    }

    /// Visit a context
    fn visit_context(&mut self, ctx: &Context) {
        // Scope the declared-name tracking and the keyword/identifier scans
        // to this component: in a multi-component document, searches start at
        // the component's own header, not at the top of the file.
        self.reset_component_state();
        let mut current_offset = ctx.span.map_or(0, |s| s.start);

        // CONTEXT keyword
        self.advance_past_keyword(KeywordId::Context, &mut current_offset);

        // Context name
        if let Some((offset, _)) = self.find_identifier(&ctx.name, current_offset) {
            self.add_identifier(&ctx.name, offset, TokenType::Namespace, true);
            current_offset = offset + ctx.name.len();
        }

        // EXTENDS clause
        if !ctx.extends.is_empty()
            && let Some(off) = self.mark_keyword(KeywordId::Extends, current_offset)
        {
            current_offset = off;

            for extended in &ctx.extends {
                if let Some((offset, _)) = self.find_identifier(extended, current_offset) {
                    self.add_identifier(extended, offset, TokenType::Namespace, false);
                    current_offset = offset + extended.len();
                }
            }
        }

        // SETS clause
        if !ctx.sets.is_empty()
            && let Some(off) = self.mark_keyword(KeywordId::Sets, current_offset)
        {
            current_offset = off;

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
            && let Some(off) = self.mark_keyword(KeywordId::Constants, current_offset)
        {
            current_offset = off;

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
            && let Some(off) = self.mark_keyword(KeywordId::Axioms, current_offset)
        {
            current_offset = off;

            // A THEOREMS header can only sit where a predicate first becomes a
            // theorem (theorems fold into `axioms`, so there is no node to drive the
            // walk). Only search on that transition, not once per predicate.
            let mut prev_is_theorem = false;
            for axiom in &ctx.axioms {
                if axiom.is_theorem
                    && !prev_is_theorem
                    && let Some(span) = &axiom.span
                {
                    current_offset =
                        self.mark_keyword_within(KeywordId::Theorems, current_offset, span.start);
                }
                current_offset = self.visit_labeled_predicate(axiom, current_offset);
                prev_is_theorem = axiom.is_theorem;
            }
        }

        // END keyword
        self.mark_keyword(KeywordId::End, current_offset);
    }

    /// Visit a machine
    fn visit_machine(&mut self, mch: &Machine) {
        // See visit_context — searches are anchored to this component.
        self.reset_component_state();
        let mut current_offset = mch.span.map_or(0, |s| s.start);

        // MACHINE keyword
        self.advance_past_keyword(KeywordId::Machine, &mut current_offset);

        // Machine name
        if let Some((offset, _)) = self.find_identifier(&mch.name, current_offset) {
            self.add_identifier(&mch.name, offset, TokenType::Namespace, true);
            current_offset = offset + mch.name.len();
        }

        // REFINES clause
        if let Some(ref refined) = mch.refines
            && let Some(off) = self.mark_keyword(KeywordId::Refines, current_offset)
        {
            current_offset = off;

            if let Some((offset, _)) = self.find_identifier(refined, current_offset) {
                self.add_identifier(refined, offset, TokenType::Namespace, false);
                current_offset = offset + refined.len();
            }
        }

        // SEES clause
        if !mch.sees.is_empty()
            && let Some(off) = self.mark_keyword(KeywordId::Sees, current_offset)
        {
            current_offset = off;

            for seen in &mch.sees {
                if let Some((offset, _)) = self.find_identifier(seen, current_offset) {
                    self.add_identifier(seen, offset, TokenType::Namespace, false);
                    current_offset = offset + seen.len();
                }
            }
        }

        // VARIABLES clause
        if !mch.variables.is_empty()
            && let Some(off) = self.mark_keyword(KeywordId::Variables, current_offset)
        {
            current_offset = off;

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
            && let Some(off) = self.mark_keyword(KeywordId::Invariants, current_offset)
        {
            current_offset = off;

            // See `visit_context`: only search for the THEOREMS header where a
            // predicate first becomes a theorem, not once per invariant.
            let mut prev_is_theorem = false;
            for invariant in &mch.invariants {
                if invariant.is_theorem
                    && !prev_is_theorem
                    && let Some(span) = &invariant.span
                {
                    current_offset =
                        self.mark_keyword_within(KeywordId::Theorems, current_offset, span.start);
                }
                current_offset = self.visit_labeled_predicate(invariant, current_offset);
                prev_is_theorem = invariant.is_theorem;
            }
        }

        // VARIANT clause
        if mch.variant.is_some() {
            self.advance_past_keyword(KeywordId::Variant, &mut current_offset);
        }

        // INITIALISATION event
        if let Some(init) = &mch.initialisation
            && let Some(off) = self.mark_keyword(KeywordId::Initialisation, current_offset)
        {
            current_offset = off;

            // Actions
            for action in &init.actions {
                current_offset = self.visit_action(action, current_offset);
            }

            self.advance_past_keyword(KeywordId::End, &mut current_offset);
        }

        // EVENTS clause
        if !mch.events.is_empty()
            && let Some(off) = self.mark_keyword(KeywordId::Events, current_offset)
        {
            current_offset = off;

            for event in &mch.events {
                current_offset = self.visit_event(event, current_offset);
            }
        }

        // END keyword
        self.mark_keyword(KeywordId::End, current_offset);
    }

    /// Visit an event
    fn visit_event(&mut self, event: &Event, mut current_offset: usize) -> usize {
        // Clear event-specific parameters
        self.parameters.clear();

        // EVENT keyword
        self.advance_past_keyword(KeywordId::Event, &mut current_offset);

        // Event name
        if let Some((offset, _)) = self.find_identifier(&event.name, current_offset) {
            self.add_identifier(&event.name, offset, TokenType::Function, true);
            current_offset = offset + event.name.len();
        }

        // Status value (convergent, anticipated)
        if let Some(status) = event.status {
            let status_id = match status {
                rossi::ast::EventStatus::Convergent => Some(KeywordId::Convergent),
                rossi::ast::EventStatus::Anticipated => Some(KeywordId::Anticipated),
                rossi::ast::EventStatus::Ordinary => None,
            };
            if let Some(id) = status_id {
                self.advance_past_keyword(id, &mut current_offset);
            }
        }

        // REFINES clause
        if event.refines.is_some() {
            self.advance_past_keyword(KeywordId::Refines, &mut current_offset);
        }

        // ANY clause (parameters)
        if !event.parameters.is_empty()
            && let Some(off) = self.mark_keyword(KeywordId::Any, current_offset)
        {
            current_offset = off;

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
            self.advance_past_keyword(KeywordId::Where, &mut current_offset);

            for guard in &event.guards {
                current_offset = self.visit_labeled_predicate(guard, current_offset);
            }
        }

        // WITH clause (labeled predicates)
        if !event.with.is_empty() {
            self.advance_past_keyword(KeywordId::With, &mut current_offset);

            for lp in &event.with {
                current_offset = self.visit_labeled_predicate(lp, current_offset);
            }
        }

        // WITNESS clause (labeled predicates)
        if !event.witnesses.is_empty() {
            self.advance_past_keyword(KeywordId::Witness, &mut current_offset);

            for lp in &event.witnesses {
                current_offset = self.visit_labeled_predicate(lp, current_offset);
            }
        }

        // THEN/BEGIN clause (actions)
        if !event.actions.is_empty() {
            self.advance_past_keyword(KeywordId::Then, &mut current_offset);

            for action in &event.actions {
                current_offset = self.visit_action(action, current_offset);
            }
        }

        // END keyword
        self.advance_past_keyword(KeywordId::End, &mut current_offset);

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

    /// Emit one COMMENT token per source line covered by each comment span.
    ///
    /// Splitting per line keeps us independent of the client's
    /// `multilineTokenSupport` capability. Comment matches are excluded from
    /// every other token search, so these tokens never overlap.
    fn emit_comment_tokens(&mut self) {
        for span in &self.comment_spans {
            let mut start = span.start;
            while start < span.end {
                let line_end = self.text[start..span.end]
                    .find('\n')
                    .map_or(span.end, |pos| start + pos);
                let segment = self.text[start..line_end].trim_end_matches('\r');
                if !segment.is_empty() {
                    let (line, col) = self.position_from_offset(start);
                    self.tokens.push(SemanticTokenData {
                        line,
                        start: col,
                        length: segment.chars().count() as u32,
                        token_type: TokenType::Comment as u32,
                        token_modifiers: 0,
                    });
                }
                start = line_end + 1;
            }
        }
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
