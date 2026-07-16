//! Complete well-typedness gates for guards and actions.
//!
//! Rodin drops a guard or action when any expression constraint fails and
//! marks the enclosing event inaccurate. Unknown forms are not accepted as
//! well typed: the inference engine reports them as unverified.

use rossi::{Action, ActionKind, Expression, Predicate};

use crate::infer::{check_expression_type, check_predicate_type};
use crate::type_env::TypeEnv;
use crate::types::Type;

/// `true` if every predicate and embedded expression constraint is verified.
pub fn is_well_typed_predicate(env: &TypeEnv, pred: &Predicate) -> bool {
    let enriched = crate::enrich::enrich_predicate(pred.clone(), env);
    check_predicate_type(env, &enriched).is_ok()
}

/// `true` if every assignment target and RHS satisfy Rodin's constraints.
pub fn is_well_typed_action(env: &TypeEnv, action: &Action) -> bool {
    let enriched = crate::enrich::enrich_action(action.clone(), env);
    is_well_typed_enriched_action(env, &enriched)
}

pub(crate) fn is_well_typed_enriched_action(env: &TypeEnv, action: &Action) -> bool {
    match &action.kind {
        ActionKind::Skip => true,
        ActionKind::Assignment { assignments } => assignments.iter().all(|(target, rhs)| {
            env.get(target.as_str())
                .is_some_and(|expected| check_expression_type(env, rhs, Some(expected)).is_ok())
        }),
        ActionKind::BecomesIn { variables, set } => assignment_product_type(env, variables)
            .map(Type::pow)
            .is_some_and(|expected| check_expression_type(env, set, Some(&expected)).is_ok()),
        ActionKind::BecomesSuchThat {
            variables,
            predicate,
        } => {
            let mut local = env.clone();
            for variable in variables {
                let Some(ty) = env.get(variable.as_str()) else {
                    return false;
                };
                local.insert(format!("{}'", variable.as_str()), ty.clone());
            }
            check_predicate_type(&local, predicate).is_ok()
        }
    }
}

fn assignment_product_type(env: &TypeEnv, variables: &[rossi::ast::Ident]) -> Option<Type> {
    let mut variables = variables.iter();
    let first = env.get(variables.next()?.as_str())?.clone();
    variables.try_fold(first, |product, variable| {
        Some(Type::prod(product, env.get(variable.as_str())?.clone()))
    })
}

/// `true` if every expression constraint is verified.
pub fn is_well_typed_expression(env: &TypeEnv, expr: &Expression) -> bool {
    let enriched = crate::enrich::enrich_expression(expr.clone(), env);
    check_expression_type(env, &enriched, None).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rossi::ast::expression::{BinaryOp, UnaryOp};
    use rossi::{parse_action_str, parse_expression_str, parse_predicate_str};

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

    #[test]
    fn rejects_non_integer_arithmetic_operands() {
        let env = TypeEnv::new();
        for source in [
            "TRUE + FALSE",
            "TRUE − FALSE",
            "TRUE ∗ FALSE",
            "TRUE ÷ FALSE",
            "TRUE mod FALSE",
            "TRUE ^ FALSE",
            "TRUE ‥ FALSE",
            "−TRUE",
        ] {
            let expression = parse_expression_str(source).unwrap();
            assert!(
                !is_well_typed_expression(&env, &expression),
                "accepted ill-typed arithmetic expression: {source}"
            );
        }
    }

    #[test]
    fn rejects_invalid_set_and_relation_operands() {
        let env = TypeEnv::new();
        for op in [
            BinaryOp::Union,
            BinaryOp::Intersection,
            BinaryOp::Difference,
            BinaryOp::CartesianProduct,
            BinaryOp::Relation,
            BinaryOp::TotalRelation,
            BinaryOp::SurjectiveRelation,
            BinaryOp::TotalSurjectiveRelation,
            BinaryOp::TotalFunction,
            BinaryOp::PartialFunction,
            BinaryOp::TotalInjection,
            BinaryOp::PartialInjection,
            BinaryOp::TotalSurjection,
            BinaryOp::PartialSurjection,
            BinaryOp::Bijection,
            BinaryOp::Composition,
            BinaryOp::Semicolon,
            BinaryOp::DomainRestriction,
            BinaryOp::DomainSubtraction,
            BinaryOp::RangeRestriction,
            BinaryOp::RangeSubtraction,
            BinaryOp::Overwrite,
            BinaryOp::DirectProduct,
            BinaryOp::ParallelProduct,
        ] {
            let expression = rossi::ExpressionKind::Binary {
                op,
                left: Box::new(rossi::ExpressionKind::True.into()),
                right: Box::new(rossi::ExpressionKind::False.into()),
            }
            .into();
            assert!(
                !is_well_typed_expression(&env, &expression),
                "accepted ill-typed set/relation operator: {op:?}"
            );
        }

        for op in [
            UnaryOp::PowerSet,
            UnaryOp::PowerSet1,
            UnaryOp::Domain,
            UnaryOp::Range,
            UnaryOp::Inverse,
        ] {
            let expression = rossi::ExpressionKind::Unary {
                op,
                operand: Box::new(rossi::ExpressionKind::True.into()),
            }
            .into();
            assert!(
                !is_well_typed_expression(&env, &expression),
                "accepted ill-typed unary operator: {op:?}"
            );
        }
    }

    #[test]
    fn rejects_nested_structural_operand_failures() {
        let mut env = TypeEnv::new();
        env.insert("S", Type::pow(Type::Integer));
        env.insert("r", Type::relation(Type::Integer, Type::Integer));

        for source in ["S ∪ dom(TRUE)", "dom(TRUE) ◁ r"] {
            let expression = parse_expression_str(source).unwrap();
            assert!(
                !is_well_typed_expression(&env, &expression),
                "accepted nested ill-typed expression: {source}"
            );
        }
    }

    #[test]
    fn rejects_invalid_comparison_operands() {
        let env = TypeEnv::new();
        for source in [
            "TRUE = 0",
            "TRUE ≠ 0",
            "TRUE < FALSE",
            "TRUE ≤ FALSE",
            "TRUE > FALSE",
            "TRUE ≥ FALSE",
            "0 ∈ BOOL",
            "0 ∉ BOOL",
            "BOOL ⊆ ℤ",
            "BOOL ⊂ ℤ",
            "BOOL ⊈ ℤ",
            "BOOL ⊄ ℤ",
            "finite(TRUE)",
            "partition(BOOL, {0})",
        ] {
            let predicate = parse_predicate_str(source).unwrap();
            assert!(
                !is_well_typed_predicate(&env, &predicate),
                "accepted ill-typed predicate: {source}"
            );
        }
    }

    #[test]
    fn rejects_invalid_function_applications() {
        let mut env = TypeEnv::new();
        env.insert("f", Type::relation(Type::Integer, Type::Boolean));
        for source in [
            "f(TRUE)",
            "TRUE(0)",
            "f[{TRUE}]",
            "card(TRUE)",
            "min({TRUE})",
            "max({TRUE})",
            "union({TRUE})",
            "inter({TRUE})",
        ] {
            let expression = parse_expression_str(source).unwrap();
            assert!(
                !is_well_typed_expression(&env, &expression),
                "accepted ill-typed application: {source}"
            );
        }
    }

    #[test]
    fn rejects_assignment_type_mismatches() {
        let mut env = TypeEnv::new();
        env.insert("x", Type::Integer);

        for source in ["x ≔ TRUE", "x :∈ BOOL", "x :∣ x' = TRUE"] {
            let action = parse_action_str(source).unwrap();
            assert!(
                !is_well_typed_action(&env, &action),
                "accepted ill-typed assignment: {source}"
            );
        }

        for source in ["x ≔ 0", "x :∈ ℤ", "x :∣ x' = x"] {
            let action = parse_action_str(source).unwrap();
            assert!(
                is_well_typed_action(&env, &action),
                "rejected well-typed assignment: {source}"
            );
        }
    }

    #[test]
    fn rejects_invalid_quantified_and_binder_bodies() {
        let env = TypeEnv::new();
        let predicate = parse_predicate_str("∀x⦂ℤ · x + TRUE = 0").unwrap();
        assert!(!is_well_typed_predicate(&env, &predicate));

        for source in [
            "λx⦂ℤ·x = x ∣ x + TRUE",
            "{x⦂ℤ·x = x ∣ x + TRUE}",
            "{x⦂ℤ ∣ x + TRUE = 0}",
            "{bool(x ∈ ℤ) ∣ x ∈ ℤ ∧ x + TRUE = 0}",
            "⋃x⦂ℤ·x = x ∣ x + TRUE",
            "⋂x⦂ℤ·x = x ∣ x + TRUE",
            "bool(TRUE + FALSE = 0)",
            "{TRUE, 0}",
            "TRUE ⦂ ℤ",
        ] {
            let expression = parse_expression_str(source).unwrap();
            assert!(
                !is_well_typed_expression(&env, &expression),
                "accepted ill-typed binder expression: {source}"
            );
        }
    }

    #[test]
    fn accepts_valid_quantified_binders_and_function_applications() {
        let mut env = TypeEnv::new();
        env.insert("f", Type::relation(Type::Integer, Type::Boolean));

        let predicate = parse_predicate_str("∀x⦂ℤ · x + 1 > x").unwrap();
        assert!(is_well_typed_predicate(&env, &predicate));

        for source in [
            "f(0)",
            "λx⦂ℤ·x = x ∣ x + 1",
            "{x⦂ℤ·x = x ∣ x + 1}",
            "⋃x⦂ℤ·x = x ∣ {x}",
        ] {
            let expression = parse_expression_str(source).unwrap();
            assert!(
                is_well_typed_expression(&env, &expression),
                "rejected well-typed binder expression: {source}"
            );
        }
    }

    #[test]
    fn accepts_binder_types_from_buried_and_chained_constraints() {
        let env = TypeEnv::new();
        for source in [
            "∀x·x + 1 > 0",
            "∀x·⊤ ⇒ x + 1 > 0",
            "∀x·⊥ ∨ x + 1 > 0",
            "∀x,y,z·x = y ∧ y = z ∧ z = 1",
        ] {
            let predicate = parse_predicate_str(source).unwrap();
            assert!(
                is_well_typed_predicate(&env, &predicate),
                "rejected well-typed binder predicate: {source}"
            );
        }
    }

    #[test]
    fn assignment_expected_type_resolves_polymorphic_rhs() {
        let mut env = TypeEnv::new();
        env.insert("f", Type::relation(Type::Integer, Type::Integer));
        env.insert("S", Type::pow(Type::Integer));

        for source in ["f ≔ λx·1 = 1 ∣ x + 1", "S ≔ union(∅)"] {
            let action = parse_action_str(source).unwrap();
            assert!(
                is_well_typed_action(&env, &action),
                "rejected contextually typed assignment: {source}"
            );
        }
    }

    #[test]
    fn unresolved_polymorphic_predicates_are_not_reported_as_checked() {
        let env = TypeEnv::new();
        for source in ["∅ = ∅", "id = id", "finite(∅)"] {
            let predicate = parse_predicate_str(source).unwrap();
            assert!(
                !is_well_typed_predicate(&env, &predicate),
                "accepted predicate with unresolved types: {source}"
            );
        }
    }

    #[test]
    fn unresolved_operands_are_not_reported_as_checked() {
        let mut env = TypeEnv::new();
        env.insert("S", Type::pow(Type::Integer));
        let expression = parse_expression_str("S ∪ unknown").unwrap();
        assert!(!is_well_typed_expression(&env, &expression));
    }
}
