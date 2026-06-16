//! Predicate AST nodes
//!
//! Predicates represent logical formulas in Event-B, including
//! comparisons, logical connectives, and quantifiers.

use super::{Expression, Ident, Span, TypedIdentifier};

/// Comparison operators
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ComparisonOp {
    Equal,
    NotEqual,
    LessThan,
    LessEqual,
    GreaterThan,
    GreaterEqual,
    In,
    NotIn,
    Subset,
    SubsetStrict,
    NotSubset,
    NotSubsetStrict,
}

/// Logical operators
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LogicalOp {
    And,
    Or,
    Implies,
    Equivalent,
}

/// Quantifiers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Quantifier {
    ForAll,
    Exists,
}

/// Built-in predicate functions recognized by the parser
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BuiltinPredicate {
    Finite,
    Partition,
}

impl BuiltinPredicate {
    /// Get the canonical name of this built-in predicate
    pub fn name(&self) -> &'static str {
        match self {
            BuiltinPredicate::Finite => "finite",
            BuiltinPredicate::Partition => "partition",
        }
    }

    /// Get the minimum number of arguments for this built-in predicate
    pub fn min_arity(&self) -> usize {
        match self {
            BuiltinPredicate::Finite => 1,
            BuiltinPredicate::Partition => 2,
        }
    }

    /// Check whether the given argument count is valid for this built-in predicate
    pub fn check_arity(&self, n: usize) -> bool {
        match self {
            BuiltinPredicate::Finite => n == 1,
            // partition(S, A, B, ...) requires the set plus at least one block
            BuiltinPredicate::Partition => n >= 2,
        }
    }

    /// Look up a built-in predicate by name
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "finite" => Some(BuiltinPredicate::Finite),
            "partition" => Some(BuiltinPredicate::Partition),
            _ => None,
        }
    }
}

/// An Event-B predicate (logical formula) together with its source location.
///
/// The predicate variant lives in [`PredicateKind`]; `span` records where the
/// predicate came from in the source text, or `None` for synthesized / Rodin-XML
/// nodes. Equality ignores `span` (see [`Expression`] for the rationale).
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Predicate {
    /// The predicate variant.
    pub kind: PredicateKind,
    /// Source span of this predicate, if known.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub span: Option<Span>,
}

impl Predicate {
    /// Wrap a kind with an explicit (optional) span.
    pub fn new(kind: PredicateKind, span: Option<Span>) -> Self {
        Self { kind, span }
    }
}

/// Equality compares the kind only; the span is positional metadata.
impl PartialEq for Predicate {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

impl Eq for Predicate {}

impl From<PredicateKind> for Predicate {
    /// Build a span-less predicate from its kind.
    fn from(kind: PredicateKind) -> Self {
        Self { kind, span: None }
    }
}

/// The variants of an Event-B [`Predicate`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PredicateKind {
    /// Boolean true
    True,

    /// Boolean false
    False,

    /// Comparison between two expressions
    Comparison {
        op: ComparisonOp,
        left: Expression,
        right: Expression,
    },

    /// Logical negation
    Not(Box<Predicate>),

    /// Binary logical operation
    Logical {
        op: LogicalOp,
        left: Box<Predicate>,
        right: Box<Predicate>,
    },

    /// Quantified predicate: ∀x·P or ∃x·P
    Quantified {
        quantifier: Quantifier,
        identifiers: Vec<TypedIdentifier>,
        predicate: Box<Predicate>,
    },

    /// User-defined predicate function application
    Application {
        function: Ident,
        arguments: Vec<Expression>,
    },

    /// Built-in predicate application: finite(S), partition(S, A, B)
    BuiltinApplication {
        predicate: BuiltinPredicate,
        arguments: Vec<Expression>,
    },
}

impl Predicate {
    /// Create a comparison predicate
    pub fn comparison(op: ComparisonOp, left: Expression, right: Expression) -> Self {
        PredicateKind::Comparison { op, left, right }.into()
    }

    /// Create a logical operation
    pub fn logical(op: LogicalOp, left: Predicate, right: Predicate) -> Self {
        PredicateKind::Logical {
            op,
            left: Box::new(left),
            right: Box::new(right),
        }
        .into()
    }

    /// Create a negation
    pub fn negation(predicate: Predicate) -> Self {
        PredicateKind::Not(Box::new(predicate)).into()
    }

    /// Create a quantified predicate
    pub fn quantified(
        quantifier: Quantifier,
        identifiers: Vec<TypedIdentifier>,
        predicate: Predicate,
    ) -> Self {
        PredicateKind::Quantified {
            quantifier,
            identifiers,
            predicate: Box::new(predicate),
        }
        .into()
    }
}
