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
    Component, Context, Event, InitialisationEvent, LabeledAction, LabeledPredicate, Machine, Span,
};
use rossi::comments::{LexicalSpans, lexical_spans, span_containing};
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

    /// Provide semantic tokens for a document.
    ///
    /// `components` is the document's shared recovered parse (from the document
    /// manager), so highlighting reflects the same AST as every other feature
    /// and does not re-parse per request. Recovery keeps the highlight alive
    /// (rather than flickering back to the TextMate grammar with the issue-#24
    /// comment bug) through a mid-edit syntax error; comment and label tokens
    /// come from a lexical scan and are emitted even when `components` is empty.
    pub fn semantic_tokens(
        &self,
        _params: &SemanticTokensParams,
        text: &str,
        components: &[Component],
    ) -> Option<SemanticTokensResult> {
        // Extract semantic tokens from the AST, one component at a time
        let mut builder = SemanticTokensBuilder::new(text);

        for component in components {
            match component {
                Component::Context(ctx) => builder.visit_context(ctx),
                Component::Machine(mch) => builder.visit_machine(mch),
            }
        }

        builder.emit_comment_tokens();
        builder.emit_label_tokens();

        let tokens = builder.build();

        debug!("Generated {} semantic tokens", tokens.data.len() / 5);

        // Nothing to highlight (unparseable input with no comments or labels):
        // return None so the client falls back to its TextMate grammar.
        if tokens.data.is_empty() {
            return None;
        }

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
    /// One lexical scan of the source: the byte spans of all comments and all
    /// `@`-labels, each sorted and disjoint. Keyword/identifier searches must
    /// never match inside either (issue #24); label tokens are emitted directly
    /// from `lexical.labels` rather than re-found by the AST walk.
    lexical: LexicalSpans,
    /// Collected semantic tokens
    tokens: Vec<SemanticTokenData>,
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
            lexical: lexical_spans(text),
            tokens: Vec::new(),
        }
    }

    /// If `offset` falls inside a comment, the offset just past that comment
    /// (where a search should resume).
    fn comment_end_after(&self, offset: usize) -> Option<usize> {
        span_containing(&self.lexical.comments, offset).map(|s| s.end)
    }

    /// If `offset` falls inside an `@`-label, the offset just past that label.
    /// A keyword/identifier spelled inside label text (`@where`, a constant `c`
    /// inside `@abc`) is opaque, exactly as the recovery parser treats it.
    fn label_end_after(&self, offset: usize) -> Option<usize> {
        span_containing(&self.lexical.labels, offset).map(|s| s.end)
    }

    /// Find position (line, UTF-16 column) from a byte offset.
    ///
    /// LSP columns are UTF-16 code units (the protocol's default encoding); see
    /// [`crate::position`]. The line is located via the precomputed
    /// `line_offsets` table so this stays cheap when emitting many tokens.
    fn position_from_offset(&self, offset: usize) -> (u32, u32) {
        // line_offsets[0] == 0 <= offset, so the partition point is >= 1.
        let line = self.line_offsets.partition_point(|&start| start <= offset) - 1;
        let column = crate::position::utf16_len(&self.text[self.line_offsets[line]..offset]);
        (line as u32, column)
    }

    /// Find `needle` in `haystack` (a same-length view of the source, byte
    /// offsets interchangeable) starting at `start_offset`, skipping matches
    /// inside comments or `@`-labels and matches that are part of a longer word
    /// (e.g. `end` inside `extended`). `bounded` is the word-boundary rule: the
    /// structural rule (where `-` is part of a word) for keywords and hyphenated
    /// component names — so the `end` of `end-update` is not a whole word — and
    /// the math rule for plain identifiers, where `-` is the subtraction
    /// operator (mirrors `references.rs`/`keywords::word_bounded_for_name`).
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
            } else if let Some(end) = self.label_end_after(offset) {
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

    /// Find the keyword identified by `id` from `from`, emit a token for the
    /// spelling that matched, and return the offset just past it. Only a
    /// spelling that ends at or before `bound` (the enclosing construct's span
    /// end) is accepted, so a search can never cross into a sibling construct.
    /// Tries each spelling in order (so `WHERE`/`WHEN`, `THEN`/`BEGIN` are both
    /// handled), and returns `None` if no spelling is found within `bound`.
    fn mark_keyword(&mut self, id: KeywordId, from: usize, bound: usize) -> Option<usize> {
        for spelling in keywords::keyword(id).spellings {
            if let Some((offset, len)) = self.find_keyword(spelling, from)
                && offset + len <= bound
            {
                self.add_keyword(spelling, offset);
                return Some(offset + len);
            }
        }
        None
    }

    /// Emit a token for keyword `id` and advance `offset` past it in place
    /// (leaving `offset` unchanged if the keyword is not found within `bound`).
    fn advance_past_keyword(&mut self, id: KeywordId, offset: &mut usize, bound: usize) {
        if let Some(next) = self.mark_keyword(id, *offset, bound) {
            *offset = next;
        }
    }

    /// Find the position of an identifier in the text, as a whole word and
    /// never inside a comment or label. The boundary rule depends on the needle:
    /// a hyphenated component name (`do-step`) takes the structural boundary, a
    /// plain math identifier the math boundary (see
    /// [`keywords::word_bounded_for_name`]).
    fn find_identifier(&self, identifier: &str, start_offset: usize) -> Option<(usize, usize)> {
        self.find_word(
            self.text,
            identifier,
            start_offset,
            keywords::word_bounded_for_name(identifier),
        )
        .map(|offset| (offset, identifier.len()))
    }

    /// Like [`Self::find_identifier`] but only accepts a match that ends at or
    /// before `bound`, keeping the search inside its construct's span.
    fn find_identifier_within(
        &self,
        identifier: &str,
        from: usize,
        bound: usize,
    ) -> Option<(usize, usize)> {
        self.find_identifier(identifier, from)
            .filter(|&(offset, len)| offset + len <= bound)
    }

    /// Add a keyword token.
    fn add_keyword(&mut self, keyword: &str, offset: usize) {
        let (line, start) = self.position_from_offset(offset);
        self.tokens.push(SemanticTokenData {
            line,
            start,
            // UTF-16 code units, like every other token length. Keywords are
            // ASCII, so this matches their byte length, but routing through the
            // single source of truth keeps the convention from drifting.
            length: crate::position::utf16_len(keyword),
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
            // UTF-16 code units: identifiers are ASCII, but labels accept any
            // non-whitespace char.
            length: crate::position::utf16_len(identifier),
            token_type: token_type as u32,
            token_modifiers: modifiers,
        });
    }

    /// Emit an identifier token straight from its AST `span`, so its position
    /// comes from the parser rather than a text re-search. Used for the
    /// declaration names the parser already records (`name_span`,
    /// `NamedElement.span`).
    fn add_identifier_span(&mut self, span: Span, token_type: TokenType, is_declaration: bool) {
        // Copy the `&str` out so the slice borrows the local, not `self`.
        let text = self.text;
        self.add_identifier(
            &text[span.start..span.end],
            span.start,
            token_type,
            is_declaration,
        );
    }

    /// The `(cursor, bound)` to scan a construct with: its own span when the
    /// parser recorded one, else the caller's fallback region (error recovery).
    /// Centralizes the "anchor to span, degrade to a bounded text range" policy.
    fn anchored(span: Option<Span>, fallback_from: usize, fallback_bound: usize) -> (usize, usize) {
        (
            span.map_or(fallback_from, |s| s.start),
            span.map_or(fallback_bound, |s| s.end),
        )
    }

    /// Mark a clause keyword (`SEES`/`EXTENDS`/`REFINES`) and color each of its
    /// `names` as a non-declaration namespace reference. A no-op returning `cur`
    /// unchanged when `names` is empty or the keyword is not found within `bound`.
    fn mark_namespace_list(
        &mut self,
        id: KeywordId,
        names: &[String],
        mut cur: usize,
        bound: usize,
    ) -> usize {
        if names.is_empty() {
            return cur;
        }
        let Some(off) = self.mark_keyword(id, cur, bound) else {
            return cur;
        };
        cur = off;
        for name in names {
            if let Some((offset, _)) = self.find_identifier_within(name, cur, bound) {
                self.add_identifier(name, offset, TokenType::Namespace, false);
                cur = offset + name.len();
            }
        }
        cur
    }

    /// Visit a context
    fn visit_context(&mut self, ctx: &Context) {
        // Anchor the keyword/identifier scans to this component: searches start
        // at the component's own header and are bounded by its span end, so they
        // can never reach a sibling component.
        let (mut cur, bound) = Self::anchored(ctx.span, 0, self.text.len());

        // CONTEXT keyword
        self.advance_past_keyword(KeywordId::Context, &mut cur, bound);

        // Context name — straight from the parser's span when available.
        cur = self.mark_name(
            &ctx.name,
            ctx.name_span,
            cur,
            bound,
            TokenType::Namespace,
            true,
        );

        // EXTENDS clause
        cur = self.mark_namespace_list(KeywordId::Extends, &ctx.extends, cur, bound);

        // SETS clause
        if !ctx.sets.is_empty()
            && let Some(off) = self.mark_keyword(KeywordId::Sets, cur, bound)
        {
            cur = off;
            for set in &ctx.sets {
                // Sets carry no per-name span (only a whole-declaration one), so
                // mark_name's bounded search locates the name — same shape as the
                // other declaration lists.
                cur = self.mark_name(set.name(), None, cur, bound, TokenType::Set, true);
            }
        }

        // CONSTANTS clause
        if !ctx.constants.is_empty()
            && let Some(off) = self.mark_keyword(KeywordId::Constants, cur, bound)
        {
            cur = off;
            for constant in &ctx.constants {
                cur = self.mark_name(
                    &constant.name,
                    constant.span,
                    cur,
                    bound,
                    TokenType::Constant,
                    true,
                );
            }
        }

        // AXIOMS clause (theorems fold into it with `is_theorem = true`)
        if !ctx.axioms.is_empty()
            && let Some(off) = self.mark_keyword(KeywordId::Axioms, cur, bound)
        {
            cur = self.visit_predicate_section(&ctx.axioms, off);
        }

        // END keyword
        self.mark_keyword(KeywordId::End, cur, bound);
    }

    /// Walk a section of labeled predicates (a context's axioms or a machine's
    /// invariants), marking a THEOREMS header at the single point a predicate
    /// first becomes a theorem. A THEOREMS section folds into the same vec with
    /// `is_theorem = true` (Rodin models a theorem as a flagged axiom/invariant,
    /// so there is no THEOREMS node to drive the walk); searching only on that
    /// false→true transition, bounded by the next predicate's start, keeps the
    /// header search from grabbing one that belongs to a later predicate.
    fn visit_predicate_section(&mut self, preds: &[LabeledPredicate], mut cur: usize) -> usize {
        let mut prev_is_theorem = false;
        for pred in preds {
            if pred.is_theorem
                && !prev_is_theorem
                && let Some(span) = &pred.span
            {
                // Bounding the THEOREMS-header search by the next predicate's
                // start keeps it from grabbing a header for a later predicate.
                cur = self
                    .mark_keyword(KeywordId::Theorems, cur, span.start)
                    .unwrap_or(cur);
            }
            cur = self.visit_labeled_predicate(pred, cur);
            prev_is_theorem = pred.is_theorem;
        }
        cur
    }

    /// Emit a declaration/name token, preferring the parser's `span` and
    /// falling back to a bounded text search when the parser did not record one
    /// (error recovery). Returns the offset just past the name (unchanged when
    /// nothing was found).
    fn mark_name(
        &mut self,
        name: &str,
        span: Option<Span>,
        from: usize,
        bound: usize,
        token_type: TokenType,
        is_declaration: bool,
    ) -> usize {
        if let Some(span) = span {
            self.add_identifier_span(span, token_type, is_declaration);
            span.end
        } else if let Some((offset, _)) = self.find_identifier_within(name, from, bound) {
            self.add_identifier(name, offset, token_type, is_declaration);
            offset + name.len()
        } else {
            from
        }
    }

    /// Visit a machine
    fn visit_machine(&mut self, mch: &Machine) {
        // See visit_context — searches are anchored to this component and
        // bounded by its span end.
        let (mut cur, bound) = Self::anchored(mch.span, 0, self.text.len());

        // MACHINE keyword
        self.advance_past_keyword(KeywordId::Machine, &mut cur, bound);

        // Machine name — straight from the parser's span when available.
        cur = self.mark_name(
            &mch.name,
            mch.name_span,
            cur,
            bound,
            TokenType::Namespace,
            true,
        );

        // REFINES clause (a single namespace target)
        if let Some(refined) = &mch.refines {
            cur = self.mark_namespace_list(
                KeywordId::Refines,
                std::slice::from_ref(refined),
                cur,
                bound,
            );
        }

        // SEES clause
        cur = self.mark_namespace_list(KeywordId::Sees, &mch.sees, cur, bound);

        // VARIABLES clause
        if !mch.variables.is_empty()
            && let Some(off) = self.mark_keyword(KeywordId::Variables, cur, bound)
        {
            cur = off;
            for variable in &mch.variables {
                cur = self.mark_name(
                    &variable.name,
                    variable.span,
                    cur,
                    bound,
                    TokenType::Variable,
                    true,
                );
            }
        }

        // INVARIANTS clause (theorems fold into it with `is_theorem = true`)
        if !mch.invariants.is_empty()
            && let Some(off) = self.mark_keyword(KeywordId::Invariants, cur, bound)
        {
            cur = self.visit_predicate_section(&mch.invariants, off);
        }

        // VARIANT clause
        if mch.variant.is_some() {
            self.advance_past_keyword(KeywordId::Variant, &mut cur, bound);
        }

        // EVENTS header. It precedes INITIALISATION and every event in the
        // source, so mark it *before* walking them — searching for it after
        // INITIALISATION (which the cursor has already advanced past) would miss
        // it and silently drop every event's highlighting.
        if mch.initialisation.is_some() || !mch.events.is_empty() {
            self.advance_past_keyword(KeywordId::Events, &mut cur, bound);
        }

        // INITIALISATION and each event are anchored to their own spans, so a
        // drifting machine cursor cannot drop or misplace their tokens. `cur`
        // (just past the EVENTS header) and `bound` only anchor a construct that
        // is itself missing a span (recovery), keeping its scan inside the
        // machine instead of restarting at the top of the file.
        if let Some(init) = &mch.initialisation {
            self.visit_initialisation(init, cur, bound);
        }
        for event in &mch.events {
            self.visit_event(event, cur, bound);
        }

        // Machine END — search after the rightmost walked construct (the max
        // span end over init + events), bounded by the machine span end. Using
        // the max rather than the last event tolerates a span-less final event.
        let end_from = mch
            .events
            .iter()
            .filter_map(|e| e.span.map(|s| s.end))
            .chain(
                mch.initialisation
                    .as_ref()
                    .and_then(|i| i.span)
                    .map(|s| s.end),
            )
            .max()
            .unwrap_or(cur);
        self.mark_keyword(KeywordId::End, end_from, bound);
    }

    /// Visit the INITIALISATION event, anchored to its own span (falling back to
    /// `[fallback_from, fallback_bound)` — the machine's cursor and end — when
    /// the parser recorded no span).
    fn visit_initialisation(
        &mut self,
        init: &InitialisationEvent,
        fallback_from: usize,
        fallback_bound: usize,
    ) {
        let (mut cur, bound) = Self::anchored(init.span, fallback_from, fallback_bound);

        // EVENT INITIALISATION
        self.advance_past_keyword(KeywordId::Event, &mut cur, bound);
        self.advance_past_keyword(KeywordId::Initialisation, &mut cur, bound);

        // THEN/BEGIN clause (actions). Labels are emitted lexically; the action
        // span advances the cursor so the END search lands on this event's END.
        if !init.actions.is_empty() {
            self.advance_past_keyword(KeywordId::Then, &mut cur, bound);
            for action in &init.actions {
                cur = self.visit_action(action, cur);
            }
        }

        self.advance_past_keyword(KeywordId::End, &mut cur, bound);
    }

    /// Visit an event, anchored to its own span so a drifting machine cursor
    /// cannot drop or misplace its tokens (falling back to the machine's cursor
    /// and end when the parser recorded no span).
    fn visit_event(&mut self, event: &Event, fallback_from: usize, fallback_bound: usize) {
        let (mut cur, bound) = Self::anchored(event.span, fallback_from, fallback_bound);

        // EVENT keyword
        self.advance_past_keyword(KeywordId::Event, &mut cur, bound);

        // Event name — straight from the parser's span when available.
        cur = self.mark_name(
            &event.name,
            event.name_span,
            cur,
            bound,
            TokenType::Function,
            true,
        );

        // Status value (convergent, anticipated)
        if let Some(status) = event.status {
            let status_id = match status {
                rossi::ast::EventStatus::Convergent => Some(KeywordId::Convergent),
                rossi::ast::EventStatus::Anticipated => Some(KeywordId::Anticipated),
                rossi::ast::EventStatus::Ordinary => None,
            };
            if let Some(id) = status_id {
                self.advance_past_keyword(id, &mut cur, bound);
            }
        }

        // REFINES clause
        if event.refines.is_some() {
            self.advance_past_keyword(KeywordId::Refines, &mut cur, bound);
        }

        // ANY clause (parameters)
        if !event.parameters.is_empty()
            && let Some(off) = self.mark_keyword(KeywordId::Any, cur, bound)
        {
            cur = off;
            for param in &event.parameters {
                cur = self.mark_name(
                    &param.name,
                    param.span,
                    cur,
                    bound,
                    TokenType::Parameter,
                    true,
                );
            }
        }

        // WHERE/WHEN clause (guards)
        if !event.guards.is_empty() {
            self.advance_past_keyword(KeywordId::Where, &mut cur, bound);
            for guard in &event.guards {
                cur = self.visit_labeled_predicate(guard, cur);
            }
        }

        // WITH clause (labeled predicates)
        if !event.with.is_empty() {
            self.advance_past_keyword(KeywordId::With, &mut cur, bound);
            for lp in &event.with {
                cur = self.visit_labeled_predicate(lp, cur);
            }
        }

        // WITNESS clause (labeled predicates)
        if !event.witnesses.is_empty() {
            self.advance_past_keyword(KeywordId::Witness, &mut cur, bound);
            for lp in &event.witnesses {
                cur = self.visit_labeled_predicate(lp, cur);
            }
        }

        // THEN/BEGIN clause (actions)
        if !event.actions.is_empty() {
            self.advance_past_keyword(KeywordId::Then, &mut cur, bound);
            for action in &event.actions {
                cur = self.visit_action(action, cur);
            }
        }

        // END keyword
        self.advance_past_keyword(KeywordId::End, &mut cur, bound);
    }

    /// Advance past a labeled predicate. Its label is emitted lexically (see
    /// [`Self::emit_label_tokens`]) and the predicate body carries no tokens
    /// yet, so this only moves the cursor to the construct's end for a
    /// following THEOREMS-header or keyword search.
    fn visit_labeled_predicate(&mut self, lp: &LabeledPredicate, current_offset: usize) -> usize {
        lp.span.map_or(current_offset, |s| s.end)
    }

    /// Advance past a labeled action. Its label is emitted lexically; the
    /// action body carries no tokens yet.
    fn visit_action(&mut self, labeled_action: &LabeledAction, current_offset: usize) -> usize {
        labeled_action.span.map_or(current_offset, |s| s.end)
    }

    /// Emit one COMMENT token per source line covered by each comment span.
    ///
    /// Splitting per line keeps us independent of the client's
    /// `multilineTokenSupport` capability. Comment matches are excluded from
    /// every other token search, so these tokens never overlap.
    fn emit_comment_tokens(&mut self) {
        for span in &self.lexical.comments {
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
                        length: crate::position::utf16_len(segment),
                        token_type: TokenType::Comment as u32,
                        token_modifiers: 0,
                    });
                }
                start = line_end + 1;
            }
        }
    }

    /// Emit one MACRO (Label) token per `@`-label, excluding the leading `@`.
    ///
    /// Labels are placed from the single lexical scan, never re-found by the AST
    /// walk, so a label name repeated across events (`@grd1` in two events) gets
    /// the same token at each occurrence — the fix for the inconsistent-label
    /// highlighting. Running here (unconditionally, after the walk) also means
    /// labels never vanish on a broken/mid-edit document.
    fn emit_label_tokens(&mut self) {
        for span in &self.lexical.labels {
            // The span covers `@name`; color the name, leaving the `@` to the
            // TextMate `entity.name.tag` scope (matches the prior behavior). A
            // trailing `:` is dropped to match the strict parser's `extract_label`
            // (eventb-to-txt compat), which strips it from the label text.
            let name_start = span.start + 1; // `@` is ASCII, one byte
            let name = self.text[name_start..span.end].trim_end_matches(':');
            if name.is_empty() {
                continue; // a bare `@` (or `@:`) with no label text
            }
            let length = crate::position::utf16_len(name);
            let (line, col) = self.position_from_offset(name_start);
            self.tokens.push(SemanticTokenData {
                line,
                start: col,
                length,
                token_type: TokenType::Label as u32,
                token_modifiers: 0,
            });
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
