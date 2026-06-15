//! Context AST nodes
//!
//! Contexts define the static properties of Event-B models including
//! sets, constants, and axioms.

use super::{FileMetadata, LabeledPredicate, NamedElement, Span};

/// A set declaration in an Event-B context
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SetDeclaration {
    /// Deferred (carrier) set: just a name
    Deferred {
        name: String,
        comment: Option<String>,
        /// Source location of the declaration (textual parse only)
        span: Option<Span>,
    },
    /// Enumerated set: name with explicit elements, e.g. `S = {a, b, c}`
    Enumerated {
        name: String,
        elements: Vec<String>,
        comment: Option<String>,
        /// Source location of the declaration (textual parse only)
        span: Option<Span>,
    },
}

impl SetDeclaration {
    /// Get the name of this set declaration
    pub fn name(&self) -> &str {
        match self {
            SetDeclaration::Deferred { name, .. } => name,
            SetDeclaration::Enumerated { name, .. } => name,
        }
    }

    /// The elements of an enumerated set, or an empty slice for a deferred
    /// (carrier) set. In Event-B these elements are declared constants, sharing
    /// the context's identifier namespace.
    pub fn elements(&self) -> &[String] {
        match self {
            SetDeclaration::Deferred { .. } => &[],
            SetDeclaration::Enumerated { elements, .. } => elements,
        }
    }

    /// Get the comment on this set declaration
    pub fn comment(&self) -> Option<&str> {
        match self {
            SetDeclaration::Deferred { comment, .. } => comment.as_deref(),
            SetDeclaration::Enumerated { comment, .. } => comment.as_deref(),
        }
    }

    /// Get the source span of this set declaration
    pub fn span(&self) -> Option<Span> {
        match self {
            SetDeclaration::Deferred { span, .. } => *span,
            SetDeclaration::Enumerated { span, .. } => *span,
        }
    }

    /// Mutable access to the source span of this set declaration
    pub fn span_mut(&mut self) -> &mut Option<Span> {
        match self {
            SetDeclaration::Deferred { span, .. } => span,
            SetDeclaration::Enumerated { span, .. } => span,
        }
    }

    /// Mutable access to the comment on this set declaration
    pub fn comment_mut(&mut self) -> &mut Option<String> {
        match self {
            SetDeclaration::Deferred { comment, .. } => comment,
            SetDeclaration::Enumerated { comment, .. } => comment,
        }
    }
}

/// An Event-B Context component
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Context {
    /// Name of the context
    pub name: String,

    /// Contexts that this context extends
    pub extends: Vec<String>,

    /// Carrier sets declared in this context (deferred or enumerated)
    pub sets: Vec<SetDeclaration>,

    /// Constants declared in this context
    pub constants: Vec<NamedElement>,

    /// Axioms (properties that must hold).
    /// Theorems are stored here with `is_theorem = true`.
    pub axioms: Vec<LabeledPredicate>,

    /// Source location of the entire context (CONTEXT name ... END)
    pub span: Option<Span>,

    /// Source location of the context name
    pub name_span: Option<Span>,

    /// Comment from Rodin XML
    pub comment: Option<String>,

    /// File-level metadata from Rodin XML
    pub metadata: Option<FileMetadata>,
}

impl Context {
    /// Create a new context with the given name
    pub fn new(name: String) -> Self {
        Self {
            name,
            extends: Vec::new(),
            sets: Vec::new(),
            constants: Vec::new(),
            axioms: Vec::new(),
            span: None,
            name_span: None,
            comment: None,
            metadata: None,
        }
    }
}
