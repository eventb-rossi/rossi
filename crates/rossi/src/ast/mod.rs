//! Abstract Syntax Tree (AST) definitions for Event-B
//!
//! This module contains the data structures representing the parsed Event-B components.

pub mod context;
pub mod event;
pub mod machine;
pub(crate) mod visit_mut;

pub use context::Context;
pub use event::{Event, EventStatus, InitialisationEvent};
pub use machine::{DEFAULT_VARIANT_LABEL, Machine, Variant};
pub(crate) use visit_mut::VisitMut;

use crate::keywords::KeywordId;

/// An identifier occurrence with its source span.
///
/// Used for identifier leaves that are not part of a formula — component
/// name occurrences and similar structural references. Equality,
/// ordering, and hashing are by name only; the span is positional metadata, so
/// two occurrences of the same name compare equal regardless of where they
/// appear.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Ident {
    /// The identifier text.
    pub name: String,
    /// Source span of this occurrence, if known.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub span: Option<Span>,
}

impl Ident {
    /// Create an identifier occurrence with an explicit (optional) span.
    pub fn new(name: impl Into<String>, span: Option<Span>) -> Self {
        Self {
            name: name.into(),
            span,
        }
    }

    /// The identifier text.
    pub fn as_str(&self) -> &str {
        &self.name
    }
}

impl PartialEq for Ident {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for Ident {}

impl std::hash::Hash for Ident {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl PartialEq<str> for Ident {
    fn eq(&self, other: &str) -> bool {
        self.name == other
    }
}

impl PartialEq<&str> for Ident {
    fn eq(&self, other: &&str) -> bool {
        self.name == *other
    }
}

impl AsRef<str> for Ident {
    fn as_ref(&self) -> &str {
        &self.name
    }
}

impl From<String> for Ident {
    fn from(name: String) -> Self {
        Self { name, span: None }
    }
}

impl From<&str> for Ident {
    fn from(name: &str) -> Self {
        Self {
            name: name.to_string(),
            span: None,
        }
    }
}

/// A named element (identifier) with an optional comment from Rodin XML
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NamedElement {
    pub name: String,
    pub comment: Option<String>,
    /// Source location of the identifier (textual parse only)
    pub span: Option<Span>,
}

impl NamedElement {
    /// Create a new named element with no comment
    pub fn new(name: String) -> Self {
        Self {
            name,
            comment: None,
            span: None,
        }
    }

    /// Create a new named element with a comment
    pub fn with_comment(name: String, comment: Option<String>) -> Self {
        Self {
            name,
            comment,
            span: None,
        }
    }

    /// Create a new named element located at `span` (used by error recovery,
    /// which records each declared name's source span so navigation and symbol
    /// providers can resolve it even in a component the strict parse rejected).
    pub fn with_span(name: String, span: Span) -> Self {
        Self {
            name,
            comment: None,
            span: Some(span),
        }
    }
}

impl From<String> for NamedElement {
    fn from(name: String) -> Self {
        Self::new(name)
    }
}

impl AsRef<str> for NamedElement {
    fn as_ref(&self) -> &str {
        &self.name
    }
}

/// File-level metadata from Rodin XML root elements
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FileMetadata {
    pub version: Option<String>,
    pub configuration: Option<String>,
}

/// A labeled predicate with an optional label identifier
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabeledPredicate {
    pub label: Option<String>,
    pub is_theorem: bool,
    pub predicate: crate::formula::Predicate,
    /// Source location of the entire labeled predicate
    pub span: Option<Span>,
    /// Comment from Rodin XML
    pub comment: Option<String>,
}

/// An action's body: either the explicit no-op or an assignment.
///
/// `skip` is a surface-language construct with no assignment to model,
/// so it lives here rather than in the formula layer.
#[derive(Debug, Clone, Eq)]
pub enum ActionBody {
    Skip {
        /// Source location of the `skip` keyword.
        span: Option<Span>,
    },
    Assignment(crate::formula::Assignment),
}

// Written out rather than derived so `skip`'s span stays out of equality, the
// way an assignment's does. Both arms carry a span the lowering already lifted
// into document coordinates, so neither is structural metadata that a layout
// change should be able to alter.
impl PartialEq for ActionBody {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ActionBody::Skip { .. }, ActionBody::Skip { .. }) => true,
            (ActionBody::Assignment(a), ActionBody::Assignment(b)) => a == b,
            _ => false,
        }
    }
}

impl ActionBody {
    /// The assignment, unless this is `skip`.
    pub fn assignment(&self) -> Option<&crate::formula::Assignment> {
        match self {
            ActionBody::Skip { .. } => None,
            ActionBody::Assignment(assignment) => Some(assignment),
        }
    }

    /// `true` iff this is the explicit no-op.
    pub fn is_skip(&self) -> bool {
        matches!(self, ActionBody::Skip { .. })
    }
}

/// A labeled action with an optional label identifier
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabeledAction {
    pub label: Option<String>,
    pub action: ActionBody,
    /// Source location of the entire labeled action
    pub span: Option<Span>,
    /// Comment from Rodin XML
    pub comment: Option<String>,
}

/// An Event-B component (either a Context or a Machine)
///
/// A `Machine` is inherently larger than a `Context` (it carries events,
/// invariants, and the initialisation), so the variants differ in size. This is
/// the parser's top-level result, held and matched ubiquitously; boxing the
/// `Machine` variant would add an allocation and a layer of indirection to every
/// component for a heuristic size delta, so the lint is allowed here.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum Component {
    Context(Context),
    Machine(Machine),
}

impl Component {
    /// The component's name, whichever kind it is.
    pub fn name(&self) -> &str {
        match self {
            Component::Context(ctx) => &ctx.name,
            Component::Machine(m) => &m.name,
        }
    }

    /// The component's source span, whichever kind it is.
    ///
    /// `None` for components built without location info (Rodin XML import,
    /// error recovery).
    pub fn span(&self) -> Option<Span> {
        match self {
            Component::Context(ctx) => ctx.span,
            Component::Machine(m) => m.span,
        }
    }

    /// The span of the component's name token, whichever kind it is.
    ///
    /// `None` for components built without location info (Rodin XML import,
    /// error recovery).
    pub fn name_span(&self) -> Option<Span> {
        match self {
            Component::Context(ctx) => ctx.name_span,
            Component::Machine(m) => m.name_span,
        }
    }

    /// The component's clause regions (textual parse only), whichever kind it is.
    pub fn clauses(&self) -> &[ClauseRegion] {
        match self {
            Component::Context(ctx) => &ctx.clauses,
            Component::Machine(m) => &m.clauses,
        }
    }

    /// Drop every source position from the component: spans, name spans and
    /// clause regions.
    ///
    /// Spans are positional metadata but they take part in equality, so two
    /// components that differ only in layout do not compare equal. Clearing
    /// them on both sides is what makes an AST-level round-trip comparison
    /// (parse → print → parse) meaningful. Formula nodes already ignore their
    /// spans in `PartialEq`, so only the structural AST needs this.
    pub fn clear_spans(&mut self) {
        SpanEraser.visit_component(self);
    }
}

/// Erases every span the structural AST carries.
///
/// Written as a visitor so it walks the same traversal every other AST
/// transform does: a component that grows a span-bearing field is covered
/// without touching this code.
struct SpanEraser;

impl VisitMut for SpanEraser {
    fn visit_optional_span(&mut self, span: &mut Option<Span>) {
        *span = None;
    }

    fn visit_context(&mut self, context: &mut Context) {
        // A clause region's span is not optional, so the region cannot be
        // emptied — and it is span-derived metadata anyway, whose offsets
        // shift whenever the source is reformatted. Drop the regions whole.
        context.clauses.clear();
        visit_mut::walk_context(self, context);
    }

    fn visit_machine(&mut self, machine: &mut Machine) {
        machine.clauses.clear();
        visit_mut::walk_machine(self, machine);
    }
}

/// Source location information for error reporting and LSP features
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Span {
    /// Start byte offset in the source text
    pub start: usize,
    /// End byte offset in the source text
    pub end: usize,
}

impl Span {
    /// Create a span from a pest::Span
    pub fn from_pest(span: pest::Span) -> Self {
        Self {
            start: span.start(),
            end: span.end(),
        }
    }

    /// Check if this span contains the given byte offset
    pub fn contains(&self, offset: usize) -> bool {
        offset >= self.start && offset < self.end
    }

    /// Shift both endpoints by `delta` bytes (error recovery lifts a span parsed
    /// from a region slice into absolute document coordinates).
    pub fn shift(&mut self, delta: usize) {
        self.start += delta;
        self.end += delta;
    }

    /// The (line, column) of `byte_offset` in `source`, both 1-indexed —
    /// the convention SARIF and the Camille reader report positions in,
    /// and what every human-facing report prints.
    ///
    /// Rescans from byte zero, so a report resolving many positions in
    /// one file should build a [`crate::ast::LineIndex`] instead.
    #[must_use]
    pub fn line_col_1_indexed(source: &str, byte_offset: usize) -> (usize, usize) {
        let (line, column) = Span {
            start: byte_offset,
            end: byte_offset,
        }
        .to_line_col(source);
        (line + 1, column + 1)
    }

    /// Convert the start byte offset to (line, column), both 0-indexed.
    ///
    /// Line 0 is the first line, column 0 is the first character on that line.
    /// This convention is suitable for LSP (which also uses 0-indexed positions).
    pub fn to_line_col(&self, source: &str) -> (usize, usize) {
        let mut line = 0;
        let mut col = 0;
        for (i, c) in source.char_indices() {
            if i >= self.start {
                break;
            }
            if c == '\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        (line, col)
    }
}

/// The line starts of one source, so a byte offset becomes a line and column
/// in logarithmic time.
///
/// [`Span::to_line_col`] rescans from byte zero on every call. That is fine
/// for a handful of diagnostics and quadratic for a dump, which positions
/// every node of every formula. Building the table once per component turns
/// the whole document's position mapping into a binary search plus one walk
/// of the containing line.
pub struct LineIndex<'a> {
    text: &'a str,
    /// Byte offset of the first character of each line. Always starts at 0,
    /// so it is never empty and the search below cannot underflow.
    starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    #[must_use]
    pub fn new(text: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(
            text.bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i + 1),
        );
        LineIndex { text, starts }
    }

    /// The 1-based line and character column of a byte offset.
    ///
    /// An offset past the end of the text clamps to the end rather than
    /// panicking: a span is only as trustworthy as the source it was taken
    /// against, and a position is not worth aborting a document over.
    #[must_use]
    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let offset = self.floor_char_boundary(offset.min(self.text.len()));
        // `starts[0]` is 0 and `offset` is non-negative, so at least one
        // element satisfies the predicate and the subtraction is safe.
        let line = self.starts.partition_point(|start| *start <= offset) - 1;
        let col = self.text[self.starts[line]..offset].chars().count() + 1;
        (line + 1, col)
    }
    /// The largest character boundary at or below `offset`.
    ///
    /// Slicing the text to compute a column would panic on an offset that
    /// splits a multi-byte character. Formula spans come from the lexer and
    /// land on boundaries, but a span is data and the mapping must be total.
    fn floor_char_boundary(&self, mut offset: usize) -> usize {
        while offset > 0 && !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }
}

/// Identity of the text a [`Span`] indexes — a filename, path, or URI.
///
/// A span is a pair of byte offsets and says nothing about what it offsets
/// into. That is enough while a consumer holds one document, which is why
/// nothing in the AST carries a `SourceId`. It stops being enough as soon as
/// formulas from several components meet: a flattened machine draws guards
/// along its `REFINES`/`EXTENDS`/`SEES` chain, and resolving one of those
/// spans against the wrong component's text yields a wrong region rather than
/// an error. Pair the span with its source at that boundary — see
/// [`Located`].
///
/// The identity is whatever string the producer already uses to name a
/// source; rossi does not interpret it. Cloning is O(1), so one handle can be
/// shared across every span of a component.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceId(std::sync::Arc<str>);

impl SourceId {
    /// Name a source.
    pub fn new(name: &str) -> Self {
        SourceId(std::sync::Arc::from(name))
    }

    /// The name this source was created with.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// Written out rather than derived: `Arc<str>` is only serializable under
// serde's `rc` feature, which would have to be turned on for every crate
// sharing the workspace dependency. Going through the string also puts a
// plain name on the wire instead of a one-element tuple.
#[cfg(feature = "serde")]
impl serde::Serialize for SourceId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for SourceId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Ok(SourceId::new(&name))
    }
}

/// A value paired with the source its positions are relative to.
///
/// Used for [`Span`]s that have left the document they were parsed from.
/// Single-document paths keep the bare `Span`, so nothing that never crosses
/// a component boundary pays for this.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Located<T> {
    /// The text `value` is relative to.
    pub source: SourceId,
    /// The located value, typically a [`Span`].
    pub value: T,
}

impl<T> Located<T> {
    /// Pair `value` with the source it is relative to.
    pub fn new(source: SourceId, value: T) -> Self {
        Located { source, value }
    }
}

/// The source region of one clause section: its header keyword through its last
/// member (the span of the clause's grammar rule).
///
/// `keyword` is the clause's header keyword (`SETS`, `INVARIANTS`, `EVENTS`, …),
/// identifying the section so consumers (folding, outline) can tell them apart.
/// Recorded for textual parses — both the strict parse and error recovery — so
/// structural consumers can span a clause without re-deriving its bounds by line
/// scanning. Absent for components built without location info (Rodin XML import).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ClauseRegion {
    pub keyword: KeywordId,
    pub span: Span,
}

impl ClauseRegion {
    /// Create a clause region introduced by `keyword`, covering `span`.
    pub fn new(keyword: KeywordId, span: Span) -> Self {
        Self { keyword, span }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_ids_compare_by_name() {
        let a = SourceId::new("AuctionMachine.eventb");
        // A separately constructed id for the same source is the same id, so
        // a consumer need not thread one handle everywhere to compare them.
        assert_eq!(a, SourceId::new("AuctionMachine.eventb"));
        assert_ne!(a, SourceId::new("AuctionContext.eventb"));
        assert_eq!(a.as_str(), "AuctionMachine.eventb");
        assert_eq!(a.to_string(), "AuctionMachine.eventb");
    }

    #[test]
    fn equal_spans_in_different_sources_stay_distinct() {
        // The point of the type: two components can carry the same byte
        // range and mean different regions.
        let span = Span { start: 0, end: 4 };
        assert_ne!(
            Located::new(SourceId::new("M0.eventb"), span),
            Located::new(SourceId::new("M1.eventb"), span)
        );
    }
}

#[cfg(test)]
mod line_index_tests {
    use super::{LineIndex, Span};

    /// The index must agree with the model's own conversion, which is
    /// 0-based, for every offset in a source. That function is the shared
    /// oracle: as long as this holds, the dump's positions and the
    /// diagnostics' regions cannot drift apart.
    fn agrees_with_model(text: &str) {
        let index = LineIndex::new(text);
        for offset in 0..=text.len() {
            if !text.is_char_boundary(offset) {
                continue;
            }
            let (line, col) = Span {
                start: offset,
                end: offset,
            }
            .to_line_col(text);
            assert_eq!(
                index.line_col(offset),
                (line + 1, col + 1),
                "offset {offset} of {text:?}"
            );
        }
    }

    #[test]
    fn agrees_with_the_model_on_plain_text() {
        agrees_with_model("machine m\nvariables x\nend\n");
    }

    #[test]
    fn agrees_with_the_model_on_multibyte_text() {
        agrees_with_model("@inv1: x ∈ ℙ(ℤ)\n@inv2: y ⊆ S\n");
    }

    #[test]
    fn agrees_with_the_model_on_crlf_text() {
        // A carriage return is an ordinary character on its line, so the
        // column of the newline that follows it counts it.
        agrees_with_model("axioms\r\n  @axm1: ⊤\r\n");
    }

    #[test]
    fn agrees_with_the_model_without_a_trailing_newline() {
        agrees_with_model("end");
    }

    #[test]
    fn an_empty_source_is_one_line() {
        assert_eq!(LineIndex::new("").line_col(0), (1, 1));
    }

    #[test]
    fn an_offset_past_the_end_clamps() {
        let index = LineIndex::new("ab\ncd");
        assert_eq!(index.line_col(99), index.line_col(5));
    }

    #[test]
    fn an_offset_inside_a_character_clamps_to_its_start() {
        // `ℤ` is three bytes at offset 0; offsets 1 and 2 split it.
        let index = LineIndex::new("ℤ ∈ S");
        assert_eq!(index.line_col(1), (1, 1));
        assert_eq!(index.line_col(2), (1, 1));
        assert_eq!(index.line_col(3), (1, 2));
    }
}
