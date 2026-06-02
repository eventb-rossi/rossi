//! Expression AST nodes
//!
//! Expressions represent values in Event-B, including sets, numbers,
//! functions, relations, and arithmetic expressions.

use super::{Predicate, TypedIdentifier};

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

/// Built-in expression functions recognized by the parser
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum BuiltinFunction {
    Card,
    Min,
    Max,
    Id,
    Prj1,
    Prj2,
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
            BuiltinFunction::Id => "id",
            BuiltinFunction::Prj1 => "prj1",
            BuiltinFunction::Prj2 => "prj2",
            BuiltinFunction::Union => "union",
            BuiltinFunction::Inter => "inter",
        }
    }

    /// Get the expected number of arguments for this built-in function
    pub fn arity(&self) -> usize {
        self.max_arity()
    }

    /// Get the minimum number of arguments
    pub fn min_arity(&self) -> usize {
        match self {
            BuiltinFunction::Card
            | BuiltinFunction::Min
            | BuiltinFunction::Max
            | BuiltinFunction::Id
            | BuiltinFunction::Prj1
            | BuiltinFunction::Prj2
            | BuiltinFunction::Union
            | BuiltinFunction::Inter => 1,
        }
    }

    /// Get the maximum number of arguments
    pub fn max_arity(&self) -> usize {
        match self {
            BuiltinFunction::Card
            | BuiltinFunction::Min
            | BuiltinFunction::Max
            | BuiltinFunction::Id
            | BuiltinFunction::Union
            | BuiltinFunction::Inter => 1,
            BuiltinFunction::Prj1 | BuiltinFunction::Prj2 => 2,
        }
    }

    /// Check if the given number of arguments is valid
    pub fn check_arity(&self, n: usize) -> bool {
        n >= self.min_arity() && n <= self.max_arity()
    }

    /// Look up a built-in function by name
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "card" => Some(BuiltinFunction::Card),
            "min" => Some(BuiltinFunction::Min),
            "max" => Some(BuiltinFunction::Max),
            "id" => Some(BuiltinFunction::Id),
            "prj1" => Some(BuiltinFunction::Prj1),
            "prj2" => Some(BuiltinFunction::Prj2),
            "union" => Some(BuiltinFunction::Union),
            "inter" => Some(BuiltinFunction::Inter),
            _ => None,
        }
    }
}

/// An Event-B expression
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Expression {
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

    /// Boolean conversion: bool(P) — converts a predicate to a boolean expression
    Bool(Box<Predicate>),

    /// String literal (ProB extension): "hello"
    StringLiteral(String),

    /// Conditional expression (ProB extension): IF P THEN E1 ELSE E2 END
    IfThenElse {
        condition: Box<Predicate>,
        then_expr: Box<Expression>,
        else_expr: Box<Expression>,
    },
}

impl Expression {
    /// Create an identifier expression
    pub fn identifier(name: impl Into<String>) -> Self {
        Expression::Identifier(name.into())
    }

    /// Create an integer expression
    pub fn integer(value: i64) -> Self {
        Expression::Integer(value)
    }

    /// Create a binary operation
    pub fn binary(op: BinaryOp, left: Expression, right: Expression) -> Self {
        Expression::Binary {
            op,
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    /// Create a unary operation
    pub fn unary(op: UnaryOp, operand: Expression) -> Self {
        Expression::Unary {
            op,
            operand: Box::new(operand),
        }
    }
}
