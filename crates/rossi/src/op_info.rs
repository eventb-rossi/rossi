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

/// Whether two adjacent set-level operators may stand without parentheses
/// in Event-B source — the *acceptance* relation enforced at parse time.
///
/// In a flat sequence `… child … parent …` (left-associative fold), `child`
/// is the root operator of the already-folded left operand and `parent` is
/// the next operator. The relation is asymmetric and does **not** reduce to a
/// precedence ladder: e.g. `∩ ▷` is accepted but `▷ ∩` is not, and `▷` is not
/// even self-associative (`a ▷ b ▷ c` requires parentheses).
///
/// This is the complete kernel_lang §3.3.7 set-operator compatibility table,
/// derived from the Rodin formula parser's accept/reject decision for every
/// ordered operator pair. It is distinct from [`set_ops_compatible`], which
/// answers the pretty-printer's narrower "must I add a paren?" question and is
/// deliberately conservative (it may omit accepted pairs — over-parenthesising
/// is harmless for printing but would *over-reject* as an acceptance gate).
/// Invariant: every [`set_ops_compatible`] pair is also accepted here
/// (checked in tests).
#[must_use]
pub fn set_ops_acceptable(child: BinaryOp, parent: BinaryOp) -> bool {
    use BinaryOp::*;
    matches!(
        (child, parent),
        // Self-associative set operators.
        (Union, Union)
            | (Intersection, Intersection)
            | (CartesianProduct, CartesianProduct)
            | (Overwrite, Overwrite)
            | (Semicolon, Semicolon)
            | (Composition, Composition)
            // ∩ as left operand.
            | (Intersection, Difference)
            | (Intersection, RangeRestriction)
            | (Intersection, RangeSubtraction)
            // ; (forward composition) as left operand.
            | (Semicolon, RangeRestriction)
            | (Semicolon, RangeSubtraction)
            // ◁ (domain restriction) as left operand.
            | (DomainRestriction, Intersection)
            | (DomainRestriction, Difference)
            | (DomainRestriction, Semicolon)
            | (DomainRestriction, RangeRestriction)
            | (DomainRestriction, RangeSubtraction)
            | (DomainRestriction, DirectProduct)
            // ⩤ (domain subtraction) as left operand.
            | (DomainSubtraction, Intersection)
            | (DomainSubtraction, Difference)
            | (DomainSubtraction, Semicolon)
            | (DomainSubtraction, RangeRestriction)
            | (DomainSubtraction, RangeSubtraction)
            | (DomainSubtraction, DirectProduct)
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
/// Two tiers: `⇒`/`⇔` form the looser tier and `∧`/`∨` the tighter one (so
/// `P ⇒ Q ∧ R` needs no parentheses). Operators within a tier share one level;
/// Camille compatibility classes (see [`logical_compat_class`]) then decide
/// whether parentheses are needed. `⇒` and `⇔` deliberately share a level even
/// though they bind nothing in common — both are class-0 singletons, so any
/// nesting of one inside the other is always parenthesised.
#[must_use]
pub fn logical_precedence(op: LogicalOp) -> u8 {
    match op {
        LogicalOp::Implies | LogicalOp::Equivalent => 1,
        LogicalOp::Or | LogicalOp::And => 2,
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

/// Whether two adjacent same-precedence logical operators may stand without
/// parentheses — the *acceptance* relation enforced at parse time, mirroring
/// the parenthesisation decision in [`logical_compat_class`].
///
/// Two operators mix bare only when both belong to the same non-singleton
/// compatibility class: `∧ ∧` and `∨ ∨` are accepted; `∧ ∨` / `∨ ∧` are not.
/// Class-0 operators (`⇒`, `⇔`) never mix bare, not even with themselves.
#[must_use]
pub fn logical_ops_compatible(child: LogicalOp, parent: LogicalOp) -> bool {
    let c = logical_compat_class(child);
    let p = logical_compat_class(parent);
    c != 0 && p != 0 && c == p
}

#[cfg(test)]
mod tests {
    use super::*;
    use BinaryOp::*;

    /// The 13 binary set-level operators (precedence level of `Union`).
    const SET_OPS: [BinaryOp; 13] = [
        Union,
        Intersection,
        Difference,
        CartesianProduct,
        Overwrite,
        Semicolon,
        Composition,
        DomainRestriction,
        DomainSubtraction,
        RangeRestriction,
        RangeSubtraction,
        DirectProduct,
        ParallelProduct,
    ];

    #[test]
    fn set_ops_acceptable_is_a_superset_of_printer_compatibility() {
        // The printer table may be conservative, but it must never claim a
        // pair compatible that the acceptance gate would reject — otherwise
        // `rossi fmt` could print a formula the parser then refuses.
        for &c in &SET_OPS {
            for &p in &SET_OPS {
                if set_ops_compatible(c, p) {
                    assert!(
                        set_ops_acceptable(c, p),
                        "{c:?} as child of {p:?}: printer-compatible but not acceptable"
                    );
                }
            }
        }
    }

    #[test]
    fn set_ops_acceptable_matches_rodin_matrix() {
        // Exactly 23 ordered pairs are accepted bare (Rodin formula parser).
        let count = SET_OPS
            .iter()
            .flat_map(|&c| SET_OPS.iter().map(move |&p| (c, p)))
            .filter(|&(c, p)| set_ops_acceptable(c, p))
            .count();
        assert_eq!(count, 23, "accepted set-operator pair count");

        // Spot checks against the oracle, including the asymmetric and
        // non-self-associative cases that distinguish this from a precedence
        // ladder.
        assert!(set_ops_acceptable(Intersection, Difference)); // ∩ ∖
        assert!(set_ops_acceptable(Intersection, RangeRestriction)); // ∩ ▷
        assert!(set_ops_acceptable(DomainRestriction, DirectProduct)); // ◁ ⊗
        assert!(!set_ops_acceptable(Union, Intersection)); // ∪ ∩
        assert!(!set_ops_acceptable(Difference, Intersection)); // ∖ ∩
        assert!(!set_ops_acceptable(RangeRestriction, RangeRestriction)); // ▷ ▷
        assert!(!set_ops_acceptable(Semicolon, DomainRestriction)); // ; ◁
        assert!(!set_ops_acceptable(ParallelProduct, ParallelProduct)); // ∥ ∥
    }

    #[test]
    fn logical_ops_compatible_only_pairs_same_associative_operator() {
        use LogicalOp::*;
        assert!(logical_ops_compatible(And, And));
        assert!(logical_ops_compatible(Or, Or));
        assert!(!logical_ops_compatible(And, Or));
        assert!(!logical_ops_compatible(Or, And));
        // ⇒ / ⇔ are singletons: never bare, not even with themselves.
        assert!(!logical_ops_compatible(Implies, Implies));
        assert!(!logical_ops_compatible(Equivalent, Equivalent));
        assert!(!logical_ops_compatible(Implies, Equivalent));
    }
}
