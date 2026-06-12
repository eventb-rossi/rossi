//! Abstract Syntax Tree (AST) definitions for Event-B
//!
//! This module contains the data structures representing the parsed Event-B components.

pub mod action;
pub mod context;
pub mod event;
pub mod expression;
pub mod machine;
pub mod predicate;

pub use action::Action;
pub use context::{Context, SetDeclaration};
pub use event::{Event, EventStatus, InitialisationEvent};
pub use expression::{BuiltinFunction, Expression, IdentPattern};
pub use machine::Machine;
pub use predicate::{BuiltinPredicate, Predicate};

/// A bound variable with an optional type annotation (e.g., `x⦂ℤ`)
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TypedIdentifier {
    pub name: String,
    pub type_expr: Option<Box<Expression>>,
}

impl TypedIdentifier {
    /// Create an untyped identifier (no type annotation)
    pub fn untyped(name: String) -> Self {
        Self {
            name,
            type_expr: None,
        }
    }

    /// Create a typed identifier with a type annotation
    pub fn typed(name: String, type_expr: Expression) -> Self {
        Self {
            name,
            type_expr: Some(Box::new(type_expr)),
        }
    }
}

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
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
