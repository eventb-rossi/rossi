//! Abstract Syntax Tree (AST) definitions for Event-B
//!
//! This module contains the data structures representing the parsed Event-B components.

pub mod action;
pub mod context;
pub mod event;
pub mod expression;
pub mod machine;
pub mod predicate;
pub mod walk;

pub use action::{Action, ActionKind};
pub use context::{Context, SetDeclaration};
pub use event::{Event, EventStatus, InitialisationEvent};
pub use expression::{
    AtomicBuiltinKind, BuiltinFunction, Expression, ExpressionKind, IdentPattern,
};
pub use machine::Machine;
pub use predicate::{BuiltinPredicate, Predicate, PredicateKind};
pub use walk::{Binder, IdentOccurrence, IdentRole, IdentVisitor};

use crate::keywords::KeywordId;

/// A bound variable with an optional type annotation (e.g., `x⦂ℤ`)
///
/// `span` locates the binder's name token in the source. Equality compares the
/// name and type annotation only — the span is positional metadata — so two
/// binders of the same name and type compare equal regardless of position.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TypedIdentifier {
    pub name: String,
    pub type_expr: Option<Box<Expression>>,
    /// Source span of the binder's name token, if known.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub span: Option<Span>,
}

impl TypedIdentifier {
    /// Create an untyped identifier (no type annotation)
    pub fn untyped(name: String) -> Self {
        Self {
            name,
            type_expr: None,
            span: None,
        }
    }

    /// Create a typed identifier with a type annotation
    pub fn typed(name: String, type_expr: Expression) -> Self {
        Self {
            name,
            type_expr: Some(Box::new(type_expr)),
            span: None,
        }
    }

    /// Set the binder's source span (builder style).
    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }
}

/// Equality compares name and type annotation; the span is positional metadata.
impl PartialEq for TypedIdentifier {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.type_expr == other.type_expr
    }
}

impl Eq for TypedIdentifier {}

impl From<String> for TypedIdentifier {
    fn from(name: String) -> Self {
        Self::untyped(name)
    }
}

impl From<&str> for TypedIdentifier {
    fn from(name: &str) -> Self {
        Self::untyped(name.to_string())
    }
}

/// Matches only if names are equal AND there is no type annotation.
/// A typed identifier `x⦂ℤ` does NOT equal `"x"`.
impl PartialEq<&str> for TypedIdentifier {
    fn eq(&self, other: &&str) -> bool {
        self.name == *other && self.type_expr.is_none()
    }
}

/// Matches only if names are equal AND there is no type annotation.
impl PartialEq<str> for TypedIdentifier {
    fn eq(&self, other: &str) -> bool {
        self.name == other && self.type_expr.is_none()
    }
}

/// An identifier occurrence with its source span.
///
/// Used for identifier leaves that are not themselves a spanned [`Expression`]
/// — assignment / `becomes` targets and predicate-application names. Equality,
/// ordering, and hashing are by name only; the span is positional metadata, so
/// two occurrences of the same name compare equal regardless of where they
/// appear (mirroring [`TypedIdentifier`]'s name-based comparisons).
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LabeledPredicate {
    pub label: Option<String>,
    pub is_theorem: bool,
    pub predicate: Predicate,
    /// Source location of the entire labeled predicate
    pub span: Option<Span>,
    /// Comment from Rodin XML
    pub comment: Option<String>,
}

/// A labeled action with an optional label identifier
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LabeledAction {
    pub label: Option<String>,
    pub action: Action,
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
