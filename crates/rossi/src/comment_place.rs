//! Where each source comment sits, so formatting can put it back.
//!
//! [`crate::comment_attach`] answers a different question: which *model
//! element* owns a comment, because that is the only thing Rodin can store
//! (`org.eventb.core.comment` is a per-element string attribute, and Rodin has
//! no text parser at all). That mapping is lossy about position by
//! construction — it keeps text and throws the span away — which is correct for
//! `rossi export` and unavoidable for `rossi import`, but wrong for text ->
//! text formatting, where the file itself is the artifact.
//!
//! This module keeps the position instead. Each comment is filed under the
//! printed line it belongs to (an [`Anchor`]) and re-emitted there, in the
//! marker style it was written in, so `rossi fmt` moves nothing.
//!
//! [`collect_anchors`] therefore walks the same elements as
//! [`crate::comment_attach`], and the two must stay in step: an element with a
//! comment slot there and no `Item` anchor here has its comments dropped by
//! `fmt`. It walks them for a different purpose, though, so it deliberately
//! differs in three ways — it excludes `init.with`/`init.witnesses` (the
//! printer never emits them), and it adds `Header`, `Clause` and `End` anchors,
//! which are lines rather than elements.
//!
//! The anchor is a key, not an offset to scan past: the printer does **not**
//! emit in source order. `print_context_into` always emits SETS, CONSTANTS then
//! AXIOMS whatever order they were written in, `THEOREMS` is folded into the
//! invariants and never printed as a clause, and INITIALISATION is printed
//! first even when it sits between events. A cursor that drained "everything
//! before offset X" would dump unrelated comments at the first clause printed
//! out of order.
//!
//! # What is preserved
//!
//! Text, marker (a run of `//` lines stays a run of `//` lines, a `/* */` block
//! stays a block), trailing-versus-own-line, the element or keyword line the
//! comment sits against, and one blank line after a comment group when the
//! source had one. A bare `//` separator line inside a banner is kept.
//!
//! # What is not, and why
//!
//! - **An event's inner clause keywords.** `ANY`, `WHERE`, `WITH`, `WITNESS`
//!   and `THEN` carry no [`crate::ast::ClauseRegion`] (see `keywords.rs`), so a
//!   comment written above `WHERE` lands above the first guard instead — one
//!   line lower. Only top-level clauses can be anchored.
//! - **A comment between a `refines`/`sees`/`extends` keyword and its target.**
//!   Those targets are bare strings in the AST with no spans, so they can never
//!   be anchors. Rodin cannot represent such a comment either — none of
//!   `IRefinesMachine`, `ISeesContext`, `IRefinesEvent` or `IExtendsContext` is
//!   an `ICommentedElement`.
//! - **A comment inside a formula**, e.g. between two lines of a wrapped
//!   predicate: it moves above the next element. The move happens once and the
//!   result is a fixed point.
//! - **Blank lines other than the single one after a comment group**, and
//!   indentation of the comment itself, which follows the anchor's.
//!
//! Every one of these is a *stable move*, never a loss:
//! [`CommentPlacements::assert_fully_rendered`] fails the build if the printer
//! ever skips an anchor that carries comments.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::ast::{ClauseRegion, Component, Span};
use crate::comments;
use crate::keywords::KeywordId;

/// The printed line a comment belongs to, identified by a source offset.
///
/// A plain offset would not do: a component's `End` line and its `Header` line
/// are different lines that can be derived from the same span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Anchor {
    /// The `machine`/`context` header line, keyed by the component's start.
    Header(usize),
    /// A top-level clause keyword line, keyed by its [`ClauseRegion`] start.
    Clause(usize),
    /// A printed element line, keyed by the element's span start.
    Item(usize),
    /// A closing `end` line, keyed by its container's span **end**.
    End(usize),
    /// After the last `end` in the file.
    Eof,
}

impl Anchor {
    /// The source offset this anchor is looked up on.
    ///
    /// `End` keys on its container's span **end**, everything else on a start —
    /// which is the whole reason this is an enum and not a bare offset. `Eof`
    /// has no span of its own and is filed at the source length instead, so it
    /// answers with the largest offset there is and always sorts last.
    fn offset(self) -> usize {
        match self {
            Anchor::Header(at) | Anchor::Clause(at) | Anchor::Item(at) | Anchor::End(at) => at,
            Anchor::Eof => usize::MAX,
        }
    }
}

/// How a comment was written, so it can be written back the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Marker {
    /// `// text` — always a single line.
    Line,
    /// `/* text */`, possibly spanning lines.
    Block,
}

/// One comment, ready to re-emit.
#[derive(Debug, Clone)]
struct Placed {
    marker: Marker,
    /// The text to put back after the marker. For a `//` comment this is the
    /// raw remainder of the line with trailing whitespace dropped, so a banner
    /// of slashes (`//////`) is restored byte for byte rather than gaining a
    /// space; for a block it is [`comments::normalize_comment`] of the body,
    /// because the block is re-indented as it is written. Never blank.
    text: String,
    /// The source left a blank line after this comment, so one is written back.
    ///
    /// Exactly one, however many the source had — that is what makes a second
    /// format pass a fixed point.
    blank_after: bool,
}

/// Every comment in a source file, filed under the line that will carry it.
#[derive(Debug, Default)]
pub(crate) struct CommentPlacements {
    /// Comments written on their own line(s), above their anchor, in source
    /// order.
    above: HashMap<Anchor, Vec<Placed>>,
    /// The comment written after code on the anchor's own line.
    trailing: HashMap<Anchor, Placed>,
    /// Anchors the printer has already rendered.
    ///
    /// An anchor the printer never visits would swallow its comments silently,
    /// which is the failure this whole module exists to prevent — so in debug
    /// builds [`Self::assert_fully_rendered`] names the one that was missed.
    /// Recording the set rather than a count also makes rendering idempotent,
    /// so a caller that has already emitted an anchor's comments can hand that
    /// anchor to the printer again without them coming out twice.
    rendered: RefCell<HashSet<Anchor>>,
}

impl CommentPlacements {
    /// File every comment in `source` under the line that will carry it.
    ///
    /// `components` must have been parsed from `source`; `layout` says which
    /// clause keywords the printer will actually give a line of their own.
    pub(crate) fn new(source: &str, components: &[Component], layout: ClauseLayout) -> Self {
        let spans = comments::comment_spans(source);
        let mut placements = Self::default();
        if spans.is_empty() {
            return placements;
        }

        let anchors = collect_anchors(source, components, layout);
        if anchors.is_empty() {
            return placements;
        }

        for span in &spans {
            let Some((is_line, text)) = comments::comment_body(&source[span.start..span.end])
            else {
                continue; // blank block comment — nothing worth keeping
            };
            let placed = Placed {
                marker: if is_line { Marker::Line } else { Marker::Block },
                text,
                blank_after: blank_line_follows(source, span.end),
            };

            // Owned by the line it sits on: the nearest anchor at or before
            // it. A line carries at most one trailing comment, so a second one
            // on the same line (`x ∈ ℕ /* a */ /* b */`) falls through to the
            // own-line case rather than being dropped.
            let trailing_slot = trailing_on_its_line(source, span.start)
                .then(|| {
                    let i = anchors
                        .partition_point(|(start, _)| *start <= span.start)
                        .saturating_sub(1);
                    anchors[i].1
                })
                .filter(|anchor| !placements.trailing.contains_key(anchor));

            if let Some(anchor) = trailing_slot {
                placements.trailing.insert(anchor, placed);
            } else {
                // Own-line: the comment introduces whatever comes next.
                let i = anchors.partition_point(|(start, _)| *start < span.end);
                let introduces = anchors.get(i).map_or(Anchor::Eof, |(_, anchor)| *anchor);
                placements.above.entry(introduces).or_default().push(placed);
            }
        }
        placements
    }

    /// Whether any comment was written above `anchor`.
    pub(crate) fn has_above(&self, anchor: Option<Anchor>) -> bool {
        anchor.is_some_and(|a| self.above.contains_key(&a))
    }

    /// Panic if the printer never visited an anchor that carries comments.
    pub(crate) fn assert_fully_rendered(&self) {
        if cfg!(debug_assertions) {
            let rendered = self.rendered.borrow();
            let missed = self
                .above
                .keys()
                .chain(self.trailing.keys())
                .find(|anchor| !rendered.contains(anchor));
            assert!(
                missed.is_none(),
                "the printer skipped {:?}, dropping the comments filed under it",
                missed.unwrap(),
            );
        }
    }

    /// Whether `anchor`'s line ends with a comment.
    ///
    /// The layout rules that today ask "does this element render a comment?"
    /// ask this instead when formatting from source.
    pub(crate) fn has_trailing(&self, anchor: Option<Anchor>) -> bool {
        anchor.is_some_and(|a| self.trailing.contains_key(&a))
    }

    /// Write the comments the source left above `anchor` into `out`, at
    /// `indent`.
    ///
    /// Idempotent: a caller that had to emit an anchor's comments early can
    /// hand the same anchor to the printer again without them coming out twice.
    pub(crate) fn write_above(&self, out: &mut String, anchor: Option<Anchor>, indent: &str) {
        let (Some(anchor), Some(placed)) = (anchor, anchor.and_then(|a| self.above.get(&a))) else {
            return;
        };
        if !self.rendered.borrow_mut().insert(anchor) {
            return;
        }
        for comment in placed {
            write_own_line(out, comment, indent);
        }
    }

    /// Write the comment trailing `anchor` on a line of its own at `indent`.
    ///
    /// For a line that has no room left for a trailing comment because the
    /// keyword shares it with what follows: left in place the next format pass
    /// would read it as belonging to the last thing on the line.
    pub(crate) fn write_trailing_alone(
        &self,
        out: &mut String,
        anchor: Option<Anchor>,
        indent: &str,
    ) {
        let (Some(anchor), Some(comment)) = (anchor, anchor.and_then(|a| self.trailing.get(&a)))
        else {
            return;
        };
        self.rendered.borrow_mut().insert(anchor);
        write_own_line(out, comment, indent);
    }

    /// Append the comment the source left trailing `anchor` to `out`, which
    /// must already hold the anchor's line.
    ///
    /// `indent` is that line's own indentation; a block comment's later lines
    /// hang one level deeper.
    pub(crate) fn write_trailing(&self, out: &mut String, anchor: Option<Anchor>, indent: &str) {
        let (Some(anchor), Some(comment)) = (anchor, anchor.and_then(|a| self.trailing.get(&a)))
        else {
            return;
        };
        self.rendered.borrow_mut().insert(anchor);
        out.push(' ');
        match comment.marker {
            Marker::Line => {
                out.push_str("//");
                out.push_str(&comment.text);
            }
            // The `/*` stays on the element's line: moving it to the next line
            // would make the second format pass read it as an own-line comment
            // and shift it.
            Marker::Block => write_block(out, &comment.text, "", indent),
        }
    }
}

/// Write one comment on a line of its own at `indent`, keeping the blank line
/// the source left after it.
fn write_own_line(out: &mut String, comment: &Placed, indent: &str) {
    match comment.marker {
        Marker::Line => {
            out.push_str(indent);
            out.push_str("//");
            out.push_str(&comment.text);
        }
        Marker::Block => write_block(out, &comment.text, indent, indent),
    }
    out.push('\n');
    if comment.blank_after {
        out.push('\n');
    }
}

/// Write a `/* … */` block into `out`: the first line at `first_indent`, later
/// lines at `continuation` plus three spaces so they hang under the opening
/// `/* `.
///
/// Shared with [`crate::pretty`], which renders the same block from an element's
/// `comment` attribute — including the `*/` escape below, which is the only
/// thing keeping an imported Rodin comment from closing the block early.
pub(crate) fn write_block(out: &mut String, text: &str, first_indent: &str, continuation: &str) {
    // `*/` inside the text would close the block early; break it up, losing
    // one byte of fidelity. Only reachable for text that came from Rodin XML,
    // so the copy is skipped on the common path.
    let escaped;
    let text = if text.contains("*/") {
        escaped = text.replace("*/", "* /");
        escaped.as_str()
    } else {
        text
    };
    let mut lines = text.split('\n');
    out.push_str(first_indent);
    out.push_str("/* ");
    out.push_str(lines.next().unwrap_or_default());
    for line in lines {
        out.push('\n');
        if !line.is_empty() {
            out.push_str(continuation);
            out.push_str("   ");
            out.push_str(line);
        }
    }
    out.push_str(" */");
}

/// Whether a blank line separates `offset` from whatever comes next.
fn blank_line_follows(source: &str, offset: usize) -> bool {
    source[offset..]
        .split_once(|c: char| !c.is_whitespace())
        .map_or(&source[offset..], |(gap, _)| gap)
        .matches('\n')
        .count()
        > 1
}

/// Whether the comment starting at `offset` has code before it on its line.
fn trailing_on_its_line(source: &str, offset: usize) -> bool {
    source[..offset]
        .rsplit('\n')
        .next()
        .is_some_and(|line| !line.trim().is_empty())
}

/// Every line the printer will emit that can carry a comment, sorted by offset.
fn collect_anchors(
    source: &str,
    components: &[Component],
    layout: ClauseLayout,
) -> Vec<(usize, Anchor)> {
    let mut anchors = Anchors::default();

    for component in components {
        anchors.header(component.span().or(component.name_span()));
        for clause in component.clauses() {
            if layout.prints_keyword_line(clause) {
                anchors.clause(clause.span);
            } else {
                // The keyword has no line of its own but is printed as part of
                // another one: file its line under that line, so a comment
                // written on it follows the keyword instead of drifting onto a
                // neighbouring element.
                anchors.at(
                    clause.span.start,
                    folded_clause_anchor(clause, component, layout),
                );
            }
        }
        match component {
            Component::Context(ctx) => {
                for set in &ctx.sets {
                    anchors.item(set.span);
                }
                for constant in &ctx.constants {
                    anchors.item(constant.span);
                }
                for axiom in &ctx.axioms {
                    anchors.item(axiom.span);
                }
            }
            Component::Machine(machine) => {
                for variable in &machine.variables {
                    anchors.item(variable.span);
                }
                for invariant in &machine.invariants {
                    anchors.item(invariant.span);
                }
                for variant in &machine.variants {
                    anchors.item(variant.span);
                }
                if let Some(init) = &machine.initialisation {
                    anchors.item(init.span);
                    for action in &init.actions {
                        anchors.item(action.span);
                    }
                    // `init.with` and `init.witnesses` are deliberately absent:
                    // print_initialisation never emits them, so a comment filed
                    // there would never be printed.
                    anchors.end(init.span);
                }
                for event in &machine.events {
                    anchors.item(event.span.or(event.name_span));
                    for parameter in &event.parameters {
                        anchors.item(parameter.span);
                    }
                    for predicate in event
                        .guards
                        .iter()
                        .chain(&event.with)
                        .chain(&event.witnesses)
                    {
                        anchors.item(predicate.span);
                    }
                    for action in &event.actions {
                        anchors.item(action.span);
                    }
                    anchors.end(event.span);
                }
            }
        }
        anchors.end(component.span());
    }

    anchors.at(source.len(), Some(Anchor::Eof));
    anchors.finish()
}

/// The anchors of one file, keyed by the offset each is looked up on.
#[derive(Default)]
struct Anchors(Vec<(usize, Anchor)>);

impl Anchors {
    /// File `anchor` under its own offset, which is where a comment search
    /// will look for it. `at` takes an explicit key for the two anchors whose
    /// lookup offset is not their own: `Eof`, and a folded clause keyword,
    /// which is looked up at the keyword but rendered on the line that
    /// absorbed it.
    fn at(&mut self, key: usize, anchor: Option<Anchor>) {
        self.0.extend(anchor.map(|anchor| (key, anchor)));
    }

    fn header(&mut self, span: Option<Span>) {
        self.push(header_anchor(span));
    }

    fn clause(&mut self, span: Span) {
        self.push(Some(Anchor::Clause(span.start)));
    }

    fn item(&mut self, span: Option<Span>) {
        self.push(item_anchor(span));
    }

    fn end(&mut self, span: Option<Span>) {
        self.push(end_anchor(span));
    }

    fn push(&mut self, anchor: Option<Anchor>) {
        if let Some(anchor) = anchor {
            self.at(anchor.offset(), Some(anchor));
        }
    }

    fn finish(mut self) -> Vec<(usize, Anchor)> {
        self.0.sort_by_key(|(offset, _)| *offset);
        self.0.dedup();
        self.0
    }
}

/// The printed line that absorbs a clause keyword the printer does not give a
/// line of its own.
///
/// `THEOREMS` is absent on purpose: its members keep their own spans, so a
/// comment written on the keyword line already lands above the first of them.
fn folded_clause_anchor(
    clause: &ClauseRegion,
    component: &Component,
    layout: ClauseLayout,
) -> Option<Anchor> {
    match clause.keyword {
        KeywordId::Refines | KeywordId::Sees | KeywordId::Extends
            if layout.inline_header_clauses =>
        {
            header_anchor(component.span().or(component.name_span()))
        }
        KeywordId::Variant if layout.inline_variant => match component {
            Component::Machine(machine) => item_anchor(machine.variants.first()?.span),
            Component::Context(_) => None,
        },
        _ => None,
    }
}

/// Which clause keywords the printer will give a line of their own.
///
/// An anchor the printer never visits would silently swallow the comments filed
/// under it, so this must match the printer's own layout decisions exactly.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ClauseLayout {
    /// `refines`/`sees`/`extends` are folded onto the component header line.
    pub(crate) inline_header_clauses: bool,
    /// `variant` shares its line with the first variant expression.
    pub(crate) inline_variant: bool,
}

impl ClauseLayout {
    /// Whether the printer emits a standalone keyword line for `clause`.
    ///
    /// `THEOREMS` is normalized into the invariants/axioms and never printed as
    /// a clause of its own.
    fn prints_keyword_line(self, clause: &ClauseRegion) -> bool {
        match clause.keyword {
            KeywordId::Theorems => false,
            KeywordId::Refines | KeywordId::Sees | KeywordId::Extends => {
                !self.inline_header_clauses
            }
            KeywordId::Variant => !self.inline_variant,
            _ => true,
        }
    }
}

/// The anchor for an element's own printed line.
pub(crate) fn item_anchor(span: Option<Span>) -> Option<Anchor> {
    span.map(|span| Anchor::Item(span.start))
}

/// The anchor for a component's header line.
pub(crate) fn header_anchor(span: Option<Span>) -> Option<Anchor> {
    span.map(|span| Anchor::Header(span.start))
}

/// The anchor for the `end` line closing `span`.
pub(crate) fn end_anchor(span: Option<Span>) -> Option<Anchor> {
    span.map(|span| Anchor::End(span.end))
}

/// The anchor for the clause introduced by `keyword`, if the source had one.
pub(crate) fn clause_anchor(clauses: &[ClauseRegion], keyword: KeywordId) -> Option<Anchor> {
    clauses
        .iter()
        .find(|clause| clause.keyword == keyword)
        .map(|clause| Anchor::Clause(clause.span.start))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailing_needs_code_before_it_on_the_line() {
        let src = "a\n  // note\nb // trailing\n";
        assert!(!trailing_on_its_line(src, src.find("// note").unwrap()));
        assert!(trailing_on_its_line(src, src.find("// trailing").unwrap()));
    }

    fn block(text: &str, first_indent: &str, continuation: &str) -> String {
        let mut out = String::new();
        write_block(&mut out, text, first_indent, continuation);
        out
    }

    #[test]
    fn block_hangs_its_continuation_lines() {
        assert_eq!(block("one\ntwo", "  ", "  "), "  /* one\n     two */");
    }

    #[test]
    fn block_close_inside_the_text_is_broken_up() {
        assert_eq!(block("a */ b", "", ""), "/* a * / b */");
    }
}
