//! Well-typedness checks for guards and actions.
//!
//! Rodin's static checker drops guards / actions whose types don't line
//! up (e.g. `a ∈ AUCTIONS ↦ item` — RHS of `∈` is a pair, not a set —
//! or `auctions ≔ auctions ∪ {a ↦ i}` where the two sides of `∪` have
//! different power-set types). The enclosing event is then marked
//! `accurate=false`. We mirror that for the cases that show up in the
//! corpus.
//!
//! The check is conservative: it only flags an element as ill-typed
//! when both sides have a known type that disagrees. Anything we can't
//! resolve is accepted. That keeps us from regressing on under-typed
//! sources where Rodin's full inference would do better than ours.

use rossi::ast::expression::BinaryOp;
use rossi::ast::predicate::ComparisonOp;
use rossi::{Action, ActionKind, Expression, ExpressionKind, Predicate, PredicateKind};

use crate::infer::type_of_expression;
use crate::type_env::TypeEnv;
use crate::types::Type;

/// `true` if every comparison and every set operation in `pred` has
/// matching types on both sides (modulo the conservative "accept on
/// missing type" rule). Recurses through logical connectives,
/// quantifiers, and into the embedded expressions.
pub fn is_well_typed_predicate(env: &TypeEnv, pred: &Predicate) -> bool {
    match &pred.kind {
        PredicateKind::True | PredicateKind::False => true,
        PredicateKind::Not(inner) => is_well_typed_predicate(env, inner),
        PredicateKind::Logical { left, right, .. } => {
            is_well_typed_predicate(env, left) && is_well_typed_predicate(env, right)
        }
        // Quantifier bodies may reference binders that aren't in env;
        // a strict check would need to plumb the binder types through,
        // and the corpus doesn't depend on it yet.
        PredicateKind::Quantified { .. } => true,
        PredicateKind::Comparison { op, left, right } => {
            is_well_typed_expression(env, left)
                && is_well_typed_expression(env, right)
                && check_comparison(env, *op, left, right)
        }
        PredicateKind::Application { arguments, .. }
        | PredicateKind::BuiltinApplication { arguments, .. } => {
            arguments.iter().all(|a| is_well_typed_expression(env, a))
        }
    }
}

/// `true` if every assignment's LHS variable type matches its RHS, and
/// every set-op operand pair agrees, etc.
pub fn is_well_typed_action(env: &TypeEnv, action: &Action) -> bool {
    match &action.kind {
        ActionKind::Skip => true,
        ActionKind::Assignment { expressions, .. } => {
            // Only verify each RHS expression is itself well-typed.
            // We deliberately skip the LHS-vs-RHS type-equality check:
            // `type_of_expression` is the relaxed inference, which falls
            // back to one side's type when the other isn't in the env
            // (e.g. `mapping ◁ prj1` where `prj1` isn't a user-declared
            // identifier — seen in a real-world corpus model). That
            // produces false positives. Type-class mismatches in the
            // RHS — which is what `auction` actually exhibits — are
            // already caught by `is_well_typed_expression`.
            expressions.iter().all(|e| is_well_typed_expression(env, e))
        }
        ActionKind::BecomesIn { set, .. } => is_well_typed_expression(env, set),
        // Becomes-such-that uses primed forms (`x'`) the identifier
        // walker doesn't yet recognise; skip rather than flag false.
        ActionKind::BecomesSuchThat { .. } => true,
        ActionKind::FunctionOverride {
            arguments,
            expression,
            ..
        } => {
            arguments.iter().all(|a| is_well_typed_expression(env, a))
                && is_well_typed_expression(env, expression)
        }
    }
}

/// Recurse through an expression checking that every operand of
/// `Union`/`Intersection`/`Difference` (the symmetric set ops, where
/// both sides should share the same power-set type) actually agrees.
pub fn is_well_typed_expression(env: &TypeEnv, expr: &Expression) -> bool {
    match &expr.kind {
        ExpressionKind::Binary {
            op: BinaryOp::Union | BinaryOp::Intersection | BinaryOp::Difference,
            left,
            right,
        } => {
            if !is_well_typed_expression(env, left) || !is_well_typed_expression(env, right) {
                return false;
            }
            match (
                type_of_expression(env, left),
                type_of_expression(env, right),
            ) {
                (Some(lt), Some(rt)) => lt == rt,
                _ => true,
            }
        }
        ExpressionKind::Binary { left, right, .. } => {
            is_well_typed_expression(env, left) && is_well_typed_expression(env, right)
        }
        ExpressionKind::Unary { operand, .. } => is_well_typed_expression(env, operand),
        ExpressionKind::FunctionApplication {
            function,
            arguments,
        } => {
            is_well_typed_expression(env, function)
                && arguments.iter().all(|a| is_well_typed_expression(env, a))
        }
        ExpressionKind::BuiltinApplication { arguments, .. } => {
            arguments.iter().all(|a| is_well_typed_expression(env, a))
        }
        ExpressionKind::SetEnumeration(items) => {
            items.iter().all(|i| is_well_typed_expression(env, i))
        }
        ExpressionKind::RelationalImage { relation, set } => {
            is_well_typed_expression(env, relation) && is_well_typed_expression(env, set)
        }
        // Lambda / set-comp / quantifier-union/inter — bodies reference
        // binders not in env, would need a richer check. Skip for now.
        _ => true,
    }
}

/// Comparison-specific shape check (called after both sides are known
/// to be well-typed expressions).
fn check_comparison(
    env: &TypeEnv,
    op: ComparisonOp,
    left: &Expression,
    right: &Expression,
) -> bool {
    use ComparisonOp::*;
    match op {
        // `e ∈ S` and `e ∉ S` require S : ℙ(τ) with typeof(e) = τ.
        // Catches `a ∈ AUCTIONS ↦ item` (RHS is a pair, not a set).
        In | NotIn => match type_of_expression(env, right) {
            Some(Type::PowerSet(elem)) => match type_of_expression(env, left) {
                Some(t) => t == *elem,
                None => true,
            },
            Some(_) => false,
            None => true,
        },
        // `S ⊆ T`, `S ⊂ T` etc.: both sides must be ℙ(τ) for the same τ.
        Subset | SubsetStrict | NotSubset | NotSubsetStrict => {
            match (
                type_of_expression(env, left),
                type_of_expression(env, right),
            ) {
                (Some(lt), Some(rt)) => lt == rt && matches!(lt, Type::PowerSet(_)),
                _ => true,
            }
        }
        // `e₁ = e₂` and `e₁ ≠ e₂`: both sides must have the same type.
        Equal | NotEqual => match (
            type_of_expression(env, left),
            type_of_expression(env, right),
        ) {
            (Some(lt), Some(rt)) => lt == rt,
            _ => true,
        },
        // `e₁ < e₂` etc. require both sides to be ℤ.
        LessThan | LessEqual | GreaterThan | GreaterEqual => {
            match (
                type_of_expression(env, left),
                type_of_expression(env, right),
            ) {
                (Some(Type::Integer), Some(Type::Integer)) => true,
                (Some(_), Some(_)) => false,
                _ => true,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rossi::{parse_action_str, parse_predicate_str};

    fn auction_env() -> TypeEnv {
        let mut env = TypeEnv::new();
        env.add_carrier_set("AUCTIONS");
        env.add_carrier_set("ITEMS");
        env.insert(
            "a",
            Type::prod(Type::carrier_elem("AUCTIONS"), Type::carrier_elem("ITEMS")),
        );
        env.insert("i", Type::carrier_elem("ITEMS"));
        env.insert("item", Type::pow(Type::carrier_elem("ITEMS")));
        env.insert(
            "auctions",
            Type::relation(Type::carrier_elem("AUCTIONS"), Type::carrier_elem("ITEMS")),
        );
        env
    }

    impl Type {
        fn carrier_elem(name: &str) -> Type {
            Type::GivenSet(name.to_string())
        }
    }

    #[test]
    fn rejects_membership_with_pair_rhs() {
        let env = auction_env();
        let p = parse_predicate_str("a ∈ AUCTIONS ↦ item").unwrap();
        assert!(!is_well_typed_predicate(&env, &p));
    }

    #[test]
    fn rejects_assignment_with_mismatched_union_operands() {
        let env = auction_env();
        let a = parse_action_str("auctions ≔ auctions ∪ {a ↦ i}").unwrap();
        assert!(!is_well_typed_action(&env, &a));
    }

    #[test]
    fn accepts_membership_with_set_rhs() {
        let env = auction_env();
        let p = parse_predicate_str("a ∈ auctions").unwrap();
        assert!(is_well_typed_predicate(&env, &p));
    }

    #[test]
    fn accepts_assignment_with_consistent_types() {
        let env = auction_env();
        let a = parse_action_str("auctions ≔ auctions ∪ {a}").unwrap();
        assert!(is_well_typed_action(&env, &a));
    }
}
