//! Expression AST nodes
//!
//! Expressions represent values in Event-B, including sets, numbers,
//! functions, relations, and arithmetic expressions.

use super::{Predicate, Span, TypedIdentifier};

/// Pattern for lambda abstraction parameters (per Event-B kernel language spec §3.3.6).
///
/// Unlike quantified predicates which use comma-separated identifier lists,
/// lambda expressions use maplet-based patterns. Each leaf identifier may
/// optionally carry a type annotation (`x⦂T`), which is what Rodin's bcc
/// emits after type-checking:
/// ```text
/// ⟨ident-pattern⟩ ::= ⟨ident-pattern⟩ { '↦' ⟨ident-pattern⟩ }
///                     | '(' ⟨ident-pattern⟩ ')'
///                     | ⟨ident⟩ [ '⦂' ⟨type⟩ ]
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum IdentPattern {
    /// A single (possibly typed) identifier
    Identifier(TypedIdentifier),
    /// A maplet pattern: left ↦ right (left-associative)
    Maplet(Box<IdentPattern>, Box<IdentPattern>),
}

impl IdentPattern {
    /// Extract all identifier names from this pattern (in left-to-right order)
    pub fn identifiers(&self) -> Vec<&str> {
        match self {
            IdentPattern::Identifier(t) => vec![t.name.as_str()],
            IdentPattern::Maplet(left, right) => {
                let mut ids = left.identifiers();
                ids.extend(right.identifiers());
                ids
            }
        }
    }
}

/// Operators for expressions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BinaryOp {
    // Arithmetic
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Exponent,
    Range,

    // Set operations
    Union,
    Intersection,
    Difference,
    CartesianProduct,

    // Relation operations
    Relation,
    TotalRelation,
    SurjectiveRelation,
    TotalSurjectiveRelation,
    TotalFunction,
    PartialFunction,
    TotalInjection,
    PartialInjection,
    TotalSurjection,
    PartialSurjection,
    Bijection,
    Composition,
    Semicolon,
    DomainRestriction,
    DomainSubtraction,
    RangeRestriction,
    RangeSubtraction,
    Overwrite,
    DirectProduct,
    ParallelProduct,

    // Typing
    OfType,

    // Other
    Maplet,
}

/// Unary operators
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum UnaryOp {
    Minus,
    PowerSet,
    PowerSet1,
    Domain,
    Range,
    Inverse,
}

/// Closed built-in expression functions: words that are only ever meaningful
/// applied to a parenthesized argument (`card(S)`, `min(S)`, …). Each takes
/// exactly one argument. The generic relational atoms (`id`/`prj1`/`prj2`/
/// `pred`/`succ`) are *not* here — they are atomic expressions, modelled by
/// [`AtomicBuiltinKind`], and "application" of them (`prj1(x)`) is an ordinary
/// [`ExpressionKind::FunctionApplication`], matching Rodin's `FUNIMAGE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BuiltinFunction {
    Card,
    Min,
    Max,
    /// Generalized union `union(S)` (prefix `⋃` over a set of sets).
    Union,
    /// Generalized intersection `inter(S)` (prefix `⋂` over a set of sets).
    Inter,
}

impl BuiltinFunction {
    /// Get the canonical name of this built-in function
    pub fn name(&self) -> &'static str {
        match self {
            BuiltinFunction::Card => "card",
            BuiltinFunction::Min => "min",
            BuiltinFunction::Max => "max",
            BuiltinFunction::Union => "union",
            BuiltinFunction::Inter => "inter",
        }
    }

    /// Look up a built-in function by name
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "card" => Some(BuiltinFunction::Card),
            "min" => Some(BuiltinFunction::Min),
            "max" => Some(BuiltinFunction::Max),
            "union" => Some(BuiltinFunction::Union),
            "inter" => Some(BuiltinFunction::Inter),
            _ => None,
        }
    }
}

/// The generic relational atoms of the Event-B mathematical language: bare
/// words that denote a built-in relation whose type the static checker infers
/// (Rodin's `KID_GEN`/`KPRJ1_GEN`/`KPRJ2_GEN` atomic expressions, plus the
/// monomorphic integer relations `KPRED`/`KSUCC`). They are *atoms* — a bare
/// `prj1` is a value, and `prj1(x)` is function application of that value
/// (`FUNIMAGE`), never a closed builtin call. These are reserved atom words
/// (see [`crate::builtins::RESERVED_ATOM_WORDS`]): legal bare, un-namable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AtomicBuiltinKind {
    /// Generic identity relation `id` (`ℙ(α × α)`).
    Id,
    /// First projection `prj1` (`ℙ((α × β) × α)`).
    Prj1,
    /// Second projection `prj2` (`ℙ((α × β) × β)`).
    Prj2,
    /// Predecessor relation `pred` (`ℙ(ℤ × ℤ)`).
    Pred,
    /// Successor relation `succ` (`ℙ(ℤ × ℤ)`).
    Succ,
}

impl AtomicBuiltinKind {
    /// Every relational atom, the canonical variant list. `name` is the single
    /// source of spellings; `from_name` and callers that need to enumerate the
    /// atoms derive from this array rather than re-listing the variants.
    pub const ALL: [AtomicBuiltinKind; 5] = [
        AtomicBuiltinKind::Id,
        AtomicBuiltinKind::Prj1,
        AtomicBuiltinKind::Prj2,
        AtomicBuiltinKind::Pred,
        AtomicBuiltinKind::Succ,
    ];

    /// Get the canonical name of this relational atom.
    pub fn name(&self) -> &'static str {
        match self {
            AtomicBuiltinKind::Id => "id",
            AtomicBuiltinKind::Prj1 => "prj1",
            AtomicBuiltinKind::Prj2 => "prj2",
            AtomicBuiltinKind::Pred => "pred",
            AtomicBuiltinKind::Succ => "succ",
        }
    }

    /// Look up a relational atom by name (derived from [`Self::name`], so the
    /// spellings can never drift between the two directions).
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// An Event-B expression together with its source location.
///
/// The expression variant lives in [`ExpressionKind`]; `span` records where the
/// expression came from in the source text. `span` is `None` for nodes that were
/// synthesized (e.g. normalisation rewrites) or built from Rodin XML, where no
/// document offset is meaningful.
///
/// Equality and hashing intentionally ignore `span`: two expressions are equal
/// iff their kinds are structurally equal, regardless of where they appear. This
/// keeps round-trip and hand-built-AST comparisons span-insensitive.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Expression {
    /// The expression variant.
    pub kind: ExpressionKind,
    /// Source span of this expression, if known.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub span: Option<Span>,
}

impl Expression {
    /// Wrap a kind with an explicit (optional) span.
    pub fn new(kind: ExpressionKind, span: Option<Span>) -> Self {
        Self { kind, span }
    }
}

/// Equality compares the kind only; the span is positional metadata.
impl PartialEq for Expression {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

impl Eq for Expression {}

impl From<ExpressionKind> for Expression {
    /// Build a span-less expression from its kind.
    fn from(kind: ExpressionKind) -> Self {
        Self { kind, span: None }
    }
}

/// The variants of an Event-B [`Expression`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ExpressionKind {
    /// Integer literal
    Integer(i64),

    /// Identifier (variable, constant, or parameter)
    Identifier(String),

    /// Boolean true
    True,

    /// Boolean false
    False,

    /// Empty set
    EmptySet,

    /// Natural numbers set (ℕ)
    Naturals,

    /// Positive natural numbers set (ℕ1)
    Naturals1,

    /// Integer numbers set (ℤ)
    Integers,

    /// Boolean type (BOOL)
    BoolType,

    /// Set enumeration: {e1, e2, ...}
    SetEnumeration(Vec<Expression>),

    /// Set comprehension: {x, y | P} or extended {x · P | E}
    SetComprehension {
        identifiers: Vec<TypedIdentifier>,
        predicate: Box<Predicate>,
        /// Expression body for extended form {x · P | E}; None for basic {x | P}
        expression: Option<Box<Expression>>,
    },

    /// Set builder notation: {E ∣ P} where E is a general expression
    ///
    /// This is the expression-form set comprehension where the member expression
    /// appears before the pipe and the predicate after. Common with maplet patterns:
    /// `{x ↦ y ∣ x ∈ S ∧ y ∈ T}`
    SetBuilder {
        member_expression: Box<Expression>,
        predicate: Box<Predicate>,
    },

    /// Relational image: r\[S\]
    RelationalImage {
        relation: Box<Expression>,
        set: Box<Expression>,
    },

    /// Quantified union: ⋃x·P ∣ E
    QuantifiedUnion {
        identifiers: Vec<TypedIdentifier>,
        predicate: Box<Predicate>,
        expression: Box<Expression>,
    },

    /// Quantified intersection: ⋂x·P ∣ E
    QuantifiedInter {
        identifiers: Vec<TypedIdentifier>,
        predicate: Box<Predicate>,
        expression: Box<Expression>,
    },

    /// Lambda expression: λ pattern · P ∣ E
    Lambda {
        pattern: IdentPattern,
        predicate: Box<Predicate>,
        expression: Box<Expression>,
    },

    /// Binary operation
    Binary {
        op: BinaryOp,
        left: Box<Expression>,
        right: Box<Expression>,
    },

    /// Unary operation
    Unary {
        op: UnaryOp,
        operand: Box<Expression>,
    },

    /// Function/relation application: f(x)
    FunctionApplication {
        function: Box<Expression>,
        arguments: Vec<Expression>,
    },

    /// Built-in function application: card(S), min(S), etc.
    BuiltinApplication {
        function: BuiltinFunction,
        arguments: Vec<Expression>,
    },

    /// A generic relational atom written bare: `id`, `prj1`, `prj2`, `pred`,
    /// `succ`. An atomic value (Rodin's generic atomic expressions); applying
    /// one (`prj1(x)`) is an ordinary [`ExpressionKind::FunctionApplication`].
    AtomicBuiltin(AtomicBuiltinKind),

    /// Boolean conversion: bool(P) — converts a predicate to a boolean expression
    Bool(Box<Predicate>),
}

impl Expression {
    /// Create an identifier expression
    pub fn identifier(name: impl Into<String>) -> Self {
        ExpressionKind::Identifier(name.into()).into()
    }

    /// Create an integer expression
    pub fn integer(value: i64) -> Self {
        ExpressionKind::Integer(value).into()
    }

    /// Create a binary operation
    pub fn binary(op: BinaryOp, left: Expression, right: Expression) -> Self {
        ExpressionKind::Binary {
            op,
            left: Box::new(left),
            right: Box::new(right),
        }
        .into()
    }

    /// Create a unary operation
    pub fn unary(op: UnaryOp, operand: Expression) -> Self {
        ExpressionKind::Unary {
            op,
            operand: Box::new(operand),
        }
        .into()
    }
}
