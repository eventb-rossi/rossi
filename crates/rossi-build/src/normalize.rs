//! Rodin-canonical formula formatting.
//!
//! Rodin's `.bcc`/`.bcm` attribute values use tighter spacing than readable
//! Event-B text. [`PrettyPrinter`] emits that representation directly from
//! the AST; this module adds the build-specific type annotations Rodin's
//! static checker introduces.

use rossi::ast::expression::BinaryOp;
use rossi::pretty::PrettyPrinter;
use rossi::{Action, ActionKind, Expression, ExpressionKind, Predicate};

use crate::type_env::TypeEnv;
use crate::types::Type;

/// Canonicalise a predicate to Rodin's tight form.
pub fn canonical_predicate(p: &Predicate) -> String {
    PrettyPrinter::rodin_canonical().print_predicate(p)
}

/// Canonicalise an expression to Rodin's tight form.
pub fn canonical_expression(e: &Expression) -> String {
    PrettyPrinter::rodin_canonical().print_expression(e)
}

/// Canonicalise an action (assignment).
pub fn canonical_action(a: &Action) -> String {
    PrettyPrinter::rodin_canonical().print_action(a)
}

/// Canonicalise an action, injecting `⦂ T` annotations on any bare
/// empty-set RHS expressions using the known type of the LHS variable.
///
/// Rodin's SC does this during type-checking: `x ≔ ∅` becomes
/// `x ≔ ∅ ⦂ ℙ(USERS)` when `x : ℙ(USERS)`. Only deterministic assignments
/// are affected; `:∈` (becomes-in) and `:|` (becomes-such-that) keep
/// their raw form because the RHS is a set / predicate, not a value.
pub fn canonical_action_with_env(a: &Action, env: &TypeEnv) -> String {
    let annotated = annotate_empty_sets(a, env);
    canonical_action(&annotated)
}

fn annotate_empty_sets(a: &Action, env: &TypeEnv) -> Action {
    match &a.kind {
        ActionKind::Assignment { assignments } => ActionKind::Assignment {
            assignments: assignments
                .iter()
                .map(|(variable, expression)| {
                    let expression = match (&expression.kind, env.get(variable.as_str())) {
                        (ExpressionKind::EmptySet, Some(ty)) => typed_empty_set(ty),
                        _ => expression.clone(),
                    };
                    (variable.clone(), expression)
                })
                .collect(),
        }
        .into(),
        _ => a.clone(),
    }
}

fn typed_empty_set(ty: &Type) -> Expression {
    // Rodin only annotates empty sets on set-typed LHS (ℙ(T)).
    // An assignment `n ≔ 0` to an ℤ-typed variable is never an empty set,
    // so we guard here.
    if !matches!(ty, Type::PowerSet(_)) {
        return ExpressionKind::EmptySet.into();
    }
    ExpressionKind::Binary {
        op: BinaryOp::OfType,
        left: Box::new(ExpressionKind::EmptySet.into()),
        right: Box::new(type_to_expression(ty)),
    }
    .into()
}

/// Convert a [`Type`] into the `Expression` shape used for type ascriptions
/// in predicate / action text, so the pretty-printer can emit it.
pub(crate) fn type_to_expression(ty: &Type) -> Expression {
    use ExpressionKind as E;
    match ty {
        Type::Integer => E::Integers.into(),
        Type::Boolean => E::BoolType.into(),
        Type::GivenSet(name) => E::Identifier(name.clone()).into(),
        Type::PowerSet(inner) => E::Unary {
            op: rossi::ast::expression::UnaryOp::PowerSet,
            operand: Box::new(type_to_expression(inner)),
        }
        .into(),
        Type::Product(l, r) => E::Binary {
            op: BinaryOp::CartesianProduct,
            left: Box::new(type_to_expression(l)),
            right: Box::new(type_to_expression(r)),
        }
        .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rossi::parse_predicate_str;

    fn canonical_from_str(src: &str) -> String {
        let p = parse_predicate_str(src).unwrap();
        canonical_predicate(&p)
    }

    #[test]
    fn tight_membership() {
        assert_eq!(canonical_from_str("n ∈ ℕ"), "n∈ℕ");
        assert_eq!(canonical_from_str("register ⊆ USERS"), "register⊆USERS");
    }

    #[test]
    fn arithmetic_inside_function_app() {
        // `f(x) ≤ f(y)` → `f(x)≤f(y)`
        let input = parse_predicate_str("f(x) ≤ f(y)").unwrap();
        assert_eq!(canonical_predicate(&input), "f(x)≤f(y)");
    }

    #[test]
    fn logical_chain_is_tight() {
        let p = parse_predicate_str("x ∈ dom(f) ∧ y ∈ dom(f) ∧ x ≤ y").unwrap();
        assert_eq!(canonical_predicate(&p), "x∈dom(f)∧y∈dom(f)∧x≤y");
    }

    #[test]
    fn empty_set_assignment_gets_powerset_ascription() {
        use rossi::parse_action_str;
        let mut env = TypeEnv::new();
        env.insert("x", Type::pow(Type::GivenSet("USERS".into())));
        let a = parse_action_str("x ≔ ∅").unwrap();
        assert_eq!(canonical_action_with_env(&a, &env), "x ≔ ∅ ⦂ ℙ(USERS)");
    }

    #[test]
    fn parallel_assignment_annotates_every_pair() {
        use rossi::parse_action_str;
        let mut env = TypeEnv::new();
        env.insert("x", Type::pow(Type::GivenSet("USERS".into())));
        env.insert("y", Type::pow(Type::GivenSet("ITEMS".into())));
        let action = parse_action_str("x, y ≔ ∅, ∅").unwrap();
        assert_eq!(
            canonical_action_with_env(&action, &env),
            "x,y ≔ ∅ ⦂ ℙ(USERS),∅ ⦂ ℙ(ITEMS)"
        );
    }

    #[test]
    fn integer_assignment_unchanged() {
        use rossi::parse_action_str;
        let mut env = TypeEnv::new();
        env.insert("n", Type::Integer);
        let a = parse_action_str("n ≔ 0").unwrap();
        // `0` isn't an empty set — no ascription.
        assert_eq!(canonical_action_with_env(&a, &env), "n ≔ 0");
    }

    #[test]
    fn empty_set_assignment_without_env_stays_bare() {
        use rossi::parse_action_str;
        let env = TypeEnv::new();
        let a = parse_action_str("x ≔ ∅").unwrap();
        assert_eq!(canonical_action_with_env(&a, &env), "x ≔ ∅");
    }

    #[test]
    fn quantified_predicate_matches_rodin() {
        // From binary-search/C0.bcc axm4.
        let p = parse_predicate_str("∀x⦂ℤ, y⦂ℤ · x ∈ dom(f) ∧ y ∈ dom(f) ∧ x ≤ y ⇒ f(x) ≤ f(y)")
            .unwrap();
        assert_eq!(
            canonical_predicate(&p),
            "∀x⦂ℤ,y⦂ℤ·x∈dom(f)∧y∈dom(f)∧x≤y⇒f(x)≤f(y)"
        );
    }

    #[test]
    fn function_override_canonical_form() {
        use rossi::parse_action_str;
        let env = TypeEnv::new();
        // The parser lowers `f(x) ≔ E` to `f ≔ f\u{E103}{x ↦ E}` directly.
        // canonical_action_with_env emits the lowered Assignment canonically.
        let a = parse_action_str("currentFloor(c) ≔ f").unwrap();
        assert_eq!(
            canonical_action_with_env(&a, &env),
            "currentFloor ≔ currentFloor\u{E103}{c ↦ f}"
        );
    }

    #[test]
    fn function_override_maplet_arg() {
        use rossi::parse_action_str;
        let env = TypeEnv::new();
        // Override on a pair domain uses a maplet argument `g(a ↦ b) ≔ y`
        // (function application is single-argument); it lowers to the override
        // `g ≔ g <+ {(a ↦ b) ↦ y}`, the maplet printed flat (left-associative).
        let a = parse_action_str("g(a ↦ b) ≔ y").unwrap();
        assert_eq!(
            canonical_action_with_env(&a, &env),
            "g ≔ g\u{E103}{a ↦ b ↦ y}"
        );
    }

    /// Rodin keeps relation operators spaced and spells them with private-use
    /// glyphs, while relational override uses a tight private-use glyph.
    #[test]
    fn relation_operators_stay_spaced_with_their_glyph() {
        use rossi::parse_predicate_str;
        let p = parse_predicate_str("r ∈ A <<-> B").unwrap();
        assert_eq!(canonical_predicate(&p), "r∈A \u{E100} B");
    }
}
