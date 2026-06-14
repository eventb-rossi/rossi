//! Operator metadata: precedence, associativity, and compatibility.
//!
//! Companion to [`crate::operators`] (which owns the *spellings*): this
//! module is the shared reference for how Event-B operators *bind* —
//! precedence levels, associativity, and the Camille/Rodin compatibility
//! classes that decide when same-precedence operators may mix without
//! parentheses.
//!
//! Consumed by the pretty-printer ([`crate::pretty`]) and by downstream
//! renderers that must parenthesize formulas the way Rodin does (e.g.
//! the well-definedness lemma renderer in rossi-build). Keeping one
//! table prevents the parser, printer, and renderers from drifting
//! apart when the grammar evolves.

use crate::ast::expression::BinaryOp;
use crate::ast::predicate::LogicalOp;

/// Precedence level of a binary expression operator (higher = binds
/// tighter).
#[must_use]
pub fn binary_precedence(op: BinaryOp) -> u8 {
    match op {
        // Maplet / pair constructor (lowest binary precedence per
        // kernel_lang Table 3.1: `a ↦ b ↔ c = a ↦ (b ↔ c)`)
        BinaryOp::Maplet => 1,

        // Relation types (bind tighter than maplet, looser than set ops)
        BinaryOp::Relation
        | BinaryOp::TotalRelation
        | BinaryOp::SurjectiveRelation
        | BinaryOp::TotalSurjectiveRelation
        | BinaryOp::TotalFunction
        | BinaryOp::PartialFunction
        | BinaryOp::TotalInjection
        | BinaryOp::PartialInjection
        | BinaryOp::TotalSurjection
        | BinaryOp::PartialSurjection
        | BinaryOp::Bijection
        | BinaryOp::OfType => 2,

        // Binary set operators
        BinaryOp::Union
        | BinaryOp::Intersection
        | BinaryOp::Difference
        | BinaryOp::CartesianProduct
        | BinaryOp::Overwrite
        | BinaryOp::Semicolon
        | BinaryOp::Composition
        | BinaryOp::DomainRestriction
        | BinaryOp::DomainSubtraction
        | BinaryOp::RangeRestriction
        | BinaryOp::RangeSubtraction
        | BinaryOp::DirectProduct
        | BinaryOp::ParallelProduct => 3,

        // Interval
        BinaryOp::Range => 4,

        // Additive (arithmetic only)
        BinaryOp::Add | BinaryOp::Subtract => 5,

        // Multiplicative (arithmetic only)
        BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Modulo => 6,

        // Exponent — highest arithmetic precedence per spec §3.3.6
        BinaryOp::Exponent => 7,
    }
}

/// Precedence of the unary minus prefix `−e`, for Rodin-faithful
/// rendering of mixed arithmetic.
///
/// Empirically (eventb-checker's `Predicate#toString()`), unary minus
/// binds at the additive level and left-associatively: `−a∗b`
/// parenthesizes its operand to `(−a)∗b`, while `−a+b` and `−a − b` stay
/// bare. Equal to [`binary_precedence`] of `Add`/`Subtract`.
#[must_use]
pub fn unary_minus_precedence() -> u8 {
    binary_precedence(BinaryOp::Add)
}

#[must_use]
pub fn is_right_associative(_op: BinaryOp) -> bool {
    // Event-B has no right-associative binary operators at expression
    // level. Maplet is left-associative per spec p.18 (`a ↦ b ↦ c =
    // (a ↦ b) ↦ c`). Kept as a function for symmetry with
    // `is_non_associative`.
    false
}

#[must_use]
pub fn is_non_associative(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Range
            | BinaryOp::Exponent
            | BinaryOp::Relation
            | BinaryOp::TotalRelation
            | BinaryOp::SurjectiveRelation
            | BinaryOp::TotalSurjectiveRelation
            | BinaryOp::TotalFunction
            | BinaryOp::PartialFunction
            | BinaryOp::TotalInjection
            | BinaryOp::PartialInjection
            | BinaryOp::TotalSurjection
            | BinaryOp::PartialSurjection
            | BinaryOp::Bijection
            | BinaryOp::OfType
    )
}

/// Check whether two set-level operators are compatible for mixing
/// without parentheses. The `child` operator appears as the left operand
/// of the `parent` operator in a flat sequence: `... child ... parent ...`.
///
/// This is an asymmetric relation derived empirically from the Rodin
/// formula parser's actual behaviour.
#[must_use]
pub fn set_ops_compatible(child: BinaryOp, parent: BinaryOp) -> bool {
    use BinaryOp::*;
    matches!(
        (child, parent),
        (Union, Union)
            | (Intersection, Intersection)
            | (Intersection, Difference)
            | (Composition, Composition)
            | (Semicolon, Semicolon)
            | (Overwrite, Overwrite)
            // Cartesian product is left-associative: Rodin renders a
            // left-nested `a × b × c` bare and only parenthesizes a
            // right-nested child `a × (b × c)`. Verified against
            // eventb-checker's `Predicate#toString()`.
            | (CartesianProduct, CartesianProduct)
            | (DomainRestriction, Intersection)
            | (DomainRestriction, Difference)
            | (DomainRestriction, Semicolon)
            | (DomainSubtraction, Intersection)
            | (DomainSubtraction, Difference)
            | (DomainSubtraction, Semicolon)
    )
}

/// Check whether two same-precedence operators are compatible (can mix
/// without parentheses). For arithmetic and other non-set levels, uses
/// simple same-operator grouping.
#[must_use]
pub fn binary_ops_compatible(child: BinaryOp, parent: BinaryOp) -> bool {
    let prec = binary_precedence(child);
    debug_assert_eq!(prec, binary_precedence(parent));

    match prec {
        // Set operator level — use the asymmetric compatibility matrix
        p if p == binary_precedence(BinaryOp::Union) => set_ops_compatible(child, parent),
        // Additive: + and - can freely mix (left-associative)
        p if p == binary_precedence(BinaryOp::Add) => true,
        // Multiplicative: *, ÷, mod can freely mix (left-associative)
        p if p == binary_precedence(BinaryOp::Multiply) => true,
        // Maplet: left-associative, self-compatible
        p if p == binary_precedence(BinaryOp::Maplet) => child == parent,
        // Everything else (arrows, range, exponent): incompatible
        _ => false,
    }
}

/// Precedence of a logical operator (higher = binds tighter).
///
/// And/Or share the same precedence level; Camille compatibility classes
/// (see [`logical_compat_class`]) decide whether parentheses are needed.
#[must_use]
pub fn logical_precedence(op: LogicalOp) -> u8 {
    match op {
        LogicalOp::Equivalent => 1,
        LogicalOp::Implies => 2,
        LogicalOp::Or | LogicalOp::And => 3,
    }
}

/// Camille compatibility class for predicate logical operators.
///
/// Operators at the same precedence level but in different classes always
/// require explicit parentheses. Class 0 means "singleton" — incompatible
/// with everything, including itself.
#[must_use]
pub fn logical_compat_class(op: LogicalOp) -> u8 {
    match op {
        LogicalOp::And => 1,
        LogicalOp::Or => 2,
        LogicalOp::Implies | LogicalOp::Equivalent => 0, // non-associative singletons
    }
}
