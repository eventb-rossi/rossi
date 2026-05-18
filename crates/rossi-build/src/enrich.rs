//! Stamp inferred types onto quantifier and lambda binders so the emitted
//! `.bcc`/`.bcm` matches Rodin's canonical form.
//!
//! Rodin's static checker rewrites every bound variable to carry its
//! inferred type before serialising:
//!
//! - source:  `∀x · x ∈ ℤ ⇒ P`
//! - emitted: `∀x⦂ℤ · x ∈ ℤ ⇒ P`
//!
//! - source:  `λx ↦ y · x ∈ ℤ ∧ y ∈ ℤ ∣ E`
//! - emitted: `λx⦂ℤ ↦ y⦂ℤ · x ∈ ℤ ∧ y ∈ ℤ ∣ E`
//!
//! Without this pass, our output would diverge from Rodin on any source
//! that uses untyped binders.
//!
//! Inference reuses `crate::infer::collect_binder_types` (private),
//! which already recognises the `x ∈ T` and `x = expr` shapes and
//! descends through `∧` and `⇒`-antecedents.

use std::collections::BTreeMap;

use rossi::ast::expression::BinaryOp;
use rossi::ast::predicate::ComparisonOp;
use rossi::{Action, Expression, IdentPattern, Predicate, TypedIdentifier};

use crate::ast_util::left_assoc_maplet;
use crate::infer::{
    collect_binder_types, collect_free_identifiers, parse_type_from_expression,
    pattern_to_binder_types, type_of_expression,
};
use crate::normalize::type_to_expression;
use crate::type_env::TypeEnv;
use crate::types::Type;

/// Walk `pred` and stamp each untyped binder (quantifier identifier,
/// lambda pattern leaf, set-comprehension binder) with its inferred type
/// drawn from the surrounding body and `env`. Returns a new predicate;
/// `pred` is consumed.
pub fn enrich_predicate(pred: Predicate, env: &TypeEnv) -> Predicate {
    let mut env_local = env.clone();
    enrich_predicate_in(pred, &mut env_local)
}

/// Same as [`enrich_predicate`], for an [`Expression`].
pub fn enrich_expression(expr: Expression, env: &TypeEnv) -> Expression {
    let mut env_local = env.clone();
    enrich_expression_in(expr, &mut env_local)
}

/// Same as [`enrich_predicate`], for an [`Action`].
pub fn enrich_action(action: Action, env: &TypeEnv) -> Action {
    let mut env_local = env.clone();
    enrich_action_in(action, &mut env_local)
}

// ---------------------------------------------------------------------
// Internals — operate on a mutable env so we can extend it within a
// scope when descending into a binder.
// ---------------------------------------------------------------------

fn enrich_predicate_in(pred: Predicate, env: &mut TypeEnv) -> Predicate {
    match pred {
        Predicate::Quantified {
            quantifier,
            identifiers,
            predicate,
        } => {
            let new_identifiers = enrich_typed_identifiers(&identifiers, &predicate, env);
            let predicate = env.scoped(|env| {
                bind_typed_identifiers(env, &new_identifiers);
                Box::new(enrich_predicate_in(*predicate, env))
            });
            Predicate::Quantified {
                quantifier,
                identifiers: new_identifiers,
                predicate,
            }
        }
        Predicate::Logical { op, left, right } => Predicate::Logical {
            op,
            left: Box::new(enrich_predicate_in(*left, env)),
            right: Box::new(enrich_predicate_in(*right, env)),
        },
        Predicate::Not(inner) => Predicate::Not(Box::new(enrich_predicate_in(*inner, env))),
        Predicate::Comparison { op, left, right } => {
            // Bidirectional binder typing (Group S): for `=` and `≠`, if
            // one side has a resolvable type, pass it as the expected
            // type when recursing into the other side. This lets an
            // untyped λ binder pick up the function type of a typed
            // sibling lambda across `∪` / `∩` / `∖` — `type_of_expression`
            // already descends those chains (Group Q). For other
            // comparisons (∈, ⊆, <, …) the two sides don't share a
            // single type, so expected-type plumbing isn't meaningful.
            let (left_expected, right_expected) = match op {
                ComparisonOp::Equal | ComparisonOp::NotEqual => (
                    type_of_expression(env, &right),
                    type_of_expression(env, &left),
                ),
                _ => (None, None),
            };
            Predicate::Comparison {
                op,
                left: enrich_expression_in_with_expected(left, env, left_expected.as_ref()),
                right: enrich_expression_in_with_expected(right, env, right_expected.as_ref()),
            }
        }
        Predicate::Application {
            function,
            arguments,
        } => Predicate::Application {
            function,
            arguments: arguments
                .into_iter()
                .map(|e| enrich_expression_in(e, env))
                .collect(),
        },
        Predicate::BuiltinApplication {
            predicate,
            arguments,
        } => Predicate::BuiltinApplication {
            predicate,
            arguments: arguments
                .into_iter()
                .map(|e| enrich_expression_in(e, env))
                .collect(),
        },
        Predicate::True | Predicate::False => pred,
    }
}

fn enrich_expression_in(expr: Expression, env: &mut TypeEnv) -> Expression {
    enrich_expression_in_with_expected(expr, env, None)
}

fn enrich_expression_in_with_expected(
    expr: Expression,
    env: &mut TypeEnv,
    expected: Option<&Type>,
) -> Expression {
    match expr {
        Expression::Lambda {
            pattern,
            predicate,
            expression,
        } => {
            // If the surrounding context expects a function type
            // `ℙ(α × β)`, hand `α` to `enrich_ident_pattern` as a
            // fallback source for binder types (Group S). The
            // lambda's own predicate still wins when it provides
            // a typing constraint.
            let expected_domain = expected.and_then(|t| match t {
                Type::PowerSet(pair) => match pair.as_ref() {
                    Type::Product(dom, _) => Some(dom.as_ref()),
                    _ => None,
                },
                _ => None,
            });
            let new_pattern = enrich_ident_pattern(pattern, &predicate, env, expected_domain);
            let (predicate, expression) = env.scoped(|env| {
                bind_ident_pattern(env, &new_pattern);
                (
                    Box::new(enrich_predicate_in(*predicate, env)),
                    Box::new(enrich_expression_in(*expression, env)),
                )
            });
            Expression::Lambda {
                pattern: new_pattern,
                predicate,
                expression,
            }
        }
        Expression::QuantifiedUnion {
            identifiers,
            predicate,
            expression,
        } => {
            let new_identifiers = enrich_typed_identifiers(&identifiers, &predicate, env);
            let (predicate, expression) = env.scoped(|env| {
                bind_typed_identifiers(env, &new_identifiers);
                (
                    Box::new(enrich_predicate_in(*predicate, env)),
                    Box::new(enrich_expression_in(*expression, env)),
                )
            });
            Expression::QuantifiedUnion {
                identifiers: new_identifiers,
                predicate,
                expression,
            }
        }
        Expression::QuantifiedInter {
            identifiers,
            predicate,
            expression,
        } => {
            let new_identifiers = enrich_typed_identifiers(&identifiers, &predicate, env);
            let (predicate, expression) = env.scoped(|env| {
                bind_typed_identifiers(env, &new_identifiers);
                (
                    Box::new(enrich_predicate_in(*predicate, env)),
                    Box::new(enrich_expression_in(*expression, env)),
                )
            });
            Expression::QuantifiedInter {
                identifiers: new_identifiers,
                predicate,
                expression,
            }
        }
        Expression::SetComprehension {
            identifiers,
            predicate,
            expression,
        } => {
            let new_identifiers = enrich_typed_identifiers(&identifiers, &predicate, env);
            let (predicate, expression) = env.scoped(|env| {
                bind_typed_identifiers(env, &new_identifiers);
                let p = Box::new(enrich_predicate_in(*predicate, env));
                let e = expression.map(|e| Box::new(enrich_expression_in(*e, env)));
                (p, e)
            });
            // ProB's predicate parser rejects the short form
            // (`Expected: · but was: ∣`); Rodin's SC lowers it
            // unconditionally, so do we.
            let expression = expression.or_else(|| {
                let names: Vec<Expression> = new_identifiers
                    .iter()
                    .map(|t| Expression::Identifier(t.name.clone()))
                    .collect();
                // Grammar guarantees ≥1 binder, but `SetComprehension`
                // is `pub`, so guard `left_assoc_maplet`'s panic anyway.
                (!names.is_empty()).then(|| Box::new(left_assoc_maplet(&names)))
            });
            Expression::SetComprehension {
                identifiers: new_identifiers,
                predicate,
                expression,
            }
        }
        Expression::SetBuilder {
            member_expression,
            predicate,
        } => {
            // Lower `{E ∣ P}` to the long form `{x₁⦂T₁,…,xₙ⦂Tₙ · P ∣ E}`
            // per the Event-B spec (and what Rodin's bcc/bcm emits): the
            // binders are the free identifiers of E in left-to-right
            // order, even when they would shadow a same-named global.
            let free_names: Vec<String> = {
                let mut acc: Vec<&str> = Vec::new();
                collect_free_identifiers(&member_expression, &mut acc);
                acc.into_iter().map(String::from).collect()
            };
            let untyped_idents: Vec<TypedIdentifier> = free_names
                .iter()
                .map(|n| TypedIdentifier::untyped(n.clone()))
                .collect();
            let new_identifiers = enrich_typed_identifiers(&untyped_idents, &predicate, env);
            let (predicate, member_expression) = env.scoped(|env| {
                bind_typed_identifiers(env, &new_identifiers);
                (
                    Box::new(enrich_predicate_in(*predicate, env)),
                    Box::new(enrich_expression_in(*member_expression, env)),
                )
            });
            Expression::SetComprehension {
                identifiers: new_identifiers,
                predicate,
                expression: Some(member_expression),
            }
        }
        Expression::SetEnumeration(items) => Expression::SetEnumeration(
            items
                .into_iter()
                .map(|e| enrich_expression_in(e, env))
                .collect(),
        ),
        Expression::Binary { op, left, right } => {
            // `∪`/`∩`/`∖` are type-preserving: both operands share the
            // chain's expected type. Pass it through so a Lambda nested
            // inside one of these ops can pick up the binder type from
            // its sibling (Group S). Other binary ops have asymmetric
            // typing — don't propagate.
            let operand_expected = match op {
                BinaryOp::Union | BinaryOp::Intersection | BinaryOp::Difference => expected,
                _ => None,
            };
            Expression::Binary {
                op,
                left: Box::new(enrich_expression_in_with_expected(
                    *left,
                    env,
                    operand_expected,
                )),
                right: Box::new(enrich_expression_in_with_expected(
                    *right,
                    env,
                    operand_expected,
                )),
            }
        }
        Expression::Unary { op, operand } => Expression::Unary {
            op,
            operand: Box::new(enrich_expression_in(*operand, env)),
        },
        Expression::FunctionApplication {
            function,
            arguments,
        } => Expression::FunctionApplication {
            function: Box::new(enrich_expression_in(*function, env)),
            arguments: arguments
                .into_iter()
                .map(|e| enrich_expression_in(e, env))
                .collect(),
        },
        Expression::BuiltinApplication {
            function,
            arguments,
        } => Expression::BuiltinApplication {
            function,
            arguments: arguments
                .into_iter()
                .map(|e| enrich_expression_in(e, env))
                .collect(),
        },
        Expression::RelationalImage { relation, set } => Expression::RelationalImage {
            relation: Box::new(enrich_expression_in(*relation, env)),
            set: Box::new(enrich_expression_in(*set, env)),
        },
        // Leaves: nothing to recurse into.
        _ => expr,
    }
}

fn enrich_action_in(action: Action, env: &mut TypeEnv) -> Action {
    match action {
        Action::Skip => Action::Skip,
        Action::Assignment {
            variables,
            expressions,
        } => Action::Assignment {
            variables,
            expressions: expressions
                .into_iter()
                .map(|e| enrich_expression_in(e, env))
                .collect(),
        },
        Action::BecomesIn { variables, set } => Action::BecomesIn {
            variables,
            set: enrich_expression_in(set, env),
        },
        Action::BecomesSuchThat {
            variables,
            predicate,
        } => Action::BecomesSuchThat {
            variables,
            predicate: enrich_predicate_in(predicate, env),
        },
        Action::FunctionOverride {
            function,
            arguments,
            expression,
        } => Action::FunctionOverride {
            function,
            arguments: arguments
                .into_iter()
                .map(|e| enrich_expression_in(e, env))
                .collect(),
            expression: enrich_expression_in(expression, env),
        },
    }
}

// ---------------------------------------------------------------------
// Binder helpers
// ---------------------------------------------------------------------

/// Stamp inferred types onto a `Vec<TypedIdentifier>` (quantifiers,
/// quantified-union/inter, long-form set comprehension). Identifiers
/// that already carry a type, or whose type can't be inferred, are
/// kept unchanged.
fn enrich_typed_identifiers(
    identifiers: &[TypedIdentifier],
    body: &Predicate,
    env: &TypeEnv,
) -> Vec<TypedIdentifier> {
    let untyped_names: Vec<&str> = identifiers
        .iter()
        .filter(|t| t.type_expr.is_none())
        .map(|t| t.name.as_str())
        .collect();
    if untyped_names.is_empty() {
        return identifiers.to_vec();
    }
    let mut bound: BTreeMap<String, Type> = BTreeMap::new();
    collect_binder_types(env, body, &untyped_names, &mut bound);
    identifiers
        .iter()
        .map(|t| match (&t.type_expr, bound.get(&t.name)) {
            (None, Some(ty)) => TypedIdentifier::typed(t.name.clone(), type_to_expression(ty)),
            _ => t.clone(),
        })
        .collect()
}

/// Stamp inferred types onto every leaf of a lambda's [`IdentPattern`].
/// Constraints from the lambda's own `body` are consulted first;
/// `expected_domain` (the function-domain type the surrounding context
/// expects, if any) is a fallback used for leaves that body-derived
/// inference can't pin down — the corpus `(λ x · x = ∅ ∣ 0) ∪ …`
/// case (Group S).
fn enrich_ident_pattern(
    pattern: IdentPattern,
    body: &Predicate,
    env: &TypeEnv,
    expected_domain: Option<&Type>,
) -> IdentPattern {
    let untyped_names: Vec<&str> = collect_untyped_pattern_names(&pattern);
    if untyped_names.is_empty() {
        return pattern;
    }
    let mut bound: BTreeMap<String, Type> = BTreeMap::new();
    collect_binder_types(env, body, &untyped_names, &mut bound);
    if let Some(dom) = expected_domain
        && let Some(extra) = pattern_to_binder_types(&pattern, dom)
    {
        // Body-derived entries already in `bound` win — only fill
        // gaps from the expected-type source.
        for (name, ty) in extra {
            bound.entry(name).or_insert(ty);
        }
    }
    rewrite_ident_pattern(pattern, &bound)
}

fn collect_untyped_pattern_names(pat: &IdentPattern) -> Vec<&str> {
    let mut out = Vec::new();
    walk_untyped(pat, &mut out);
    out
}

fn walk_untyped<'a>(pat: &'a IdentPattern, out: &mut Vec<&'a str>) {
    match pat {
        IdentPattern::Identifier(t) if t.type_expr.is_none() => out.push(t.name.as_str()),
        IdentPattern::Identifier(_) => {}
        IdentPattern::Maplet(left, right) => {
            walk_untyped(left, out);
            walk_untyped(right, out);
        }
    }
}

fn rewrite_ident_pattern(pat: IdentPattern, bound: &BTreeMap<String, Type>) -> IdentPattern {
    match pat {
        IdentPattern::Identifier(t) => match (&t.type_expr, bound.get(&t.name)) {
            (None, Some(ty)) => {
                IdentPattern::Identifier(TypedIdentifier::typed(t.name, type_to_expression(ty)))
            }
            _ => IdentPattern::Identifier(t),
        },
        IdentPattern::Maplet(left, right) => IdentPattern::Maplet(
            Box::new(rewrite_ident_pattern(*left, bound)),
            Box::new(rewrite_ident_pattern(*right, bound)),
        ),
    }
}

/// Add every binder's type to `env` (so nested binders can reference
/// outer ones during inference).
fn bind_typed_identifiers(env: &mut TypeEnv, identifiers: &[TypedIdentifier]) {
    for t in identifiers {
        if let Some(ty_expr) = &t.type_expr
            && let Some(ty) = parse_type_from_expression(ty_expr)
        {
            env.insert(t.name.clone(), ty);
        }
    }
}

fn bind_ident_pattern(env: &mut TypeEnv, pat: &IdentPattern) {
    match pat {
        IdentPattern::Identifier(t) => {
            if let Some(ty_expr) = &t.type_expr
                && let Some(ty) = parse_type_from_expression(ty_expr)
            {
                env.insert(t.name.clone(), ty);
            }
        }
        IdentPattern::Maplet(left, right) => {
            bind_ident_pattern(env, left);
            bind_ident_pattern(env, right);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rossi::{parse_expression_str, parse_predicate_str};

    fn empty_env() -> TypeEnv {
        TypeEnv::new()
    }

    #[test]
    fn quantifier_binder_typed_from_membership_in_body() {
        let p = parse_predicate_str("∀x·x∈ℤ⇒x>0").unwrap();
        let enriched = enrich_predicate(p, &empty_env());
        match enriched {
            Predicate::Quantified { identifiers, .. } => {
                assert_eq!(identifiers.len(), 1);
                assert!(identifiers[0].type_expr.is_some());
            }
            _ => panic!("expected Quantified"),
        }
    }

    #[test]
    fn lambda_pattern_typed_from_membership_in_body() {
        let e = parse_expression_str("λx ↦ y·x∈ℤ∧y∈ℤ ∣ x + y").unwrap();
        let enriched = enrich_expression(e, &empty_env());
        match enriched {
            Expression::Lambda { pattern, .. } => {
                let mut all_typed = true;
                walk_check_typed(&pattern, &mut all_typed);
                assert!(all_typed, "all leaves should be typed: {:?}", pattern);
            }
            _ => panic!("expected Lambda"),
        }
    }

    fn walk_check_typed(pat: &IdentPattern, all: &mut bool) {
        match pat {
            IdentPattern::Identifier(t) => {
                if t.type_expr.is_none() {
                    *all = false;
                }
            }
            IdentPattern::Maplet(l, r) => {
                walk_check_typed(l, all);
                walk_check_typed(r, all);
            }
        }
    }

    #[test]
    fn already_typed_identifiers_unchanged() {
        let p = parse_predicate_str("∀x⦂ℤ·x>0").unwrap();
        let original = match &p {
            Predicate::Quantified { identifiers, .. } => identifiers.clone(),
            _ => panic!(),
        };
        let enriched = enrich_predicate(p, &empty_env());
        match enriched {
            Predicate::Quantified { identifiers, .. } => {
                assert_eq!(identifiers, original);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn no_inferable_type_leaves_binder_untyped() {
        let p = parse_predicate_str("∀x·x>0").unwrap();
        let enriched = enrich_predicate(p, &empty_env());
        match enriched {
            Predicate::Quantified { identifiers, .. } => {
                assert!(identifiers[0].type_expr.is_none());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn lambda_binder_typed_via_typed_sibling_across_union() {
        // corpus shape (Group S): the first lambda's binder is
        // constrained only by `x = ∅` (polymorphic) but the second
        // lambda's binder is explicitly `x⦂ℙ(ℤ×ℤ)`. The union forces
        // both lambdas to share a function type, so the first binder
        // should pick up `ℙ(ℤ×ℤ)` too.
        let mut env = TypeEnv::new();
        env.insert(
            "integral",
            Type::pow(Type::prod(
                Type::pow(Type::prod(Type::Integer, Type::Integer)),
                Type::Integer,
            )),
        );
        let p =
            parse_predicate_str("integral = (λ x · x = ∅ ∣ 0) ∪ (λ x⦂ℙ(ℤ×ℤ) · x = ∅ ∣ 1)").unwrap();
        let enriched = enrich_predicate(p, &env);
        let Predicate::Comparison { right, .. } = enriched else {
            panic!("expected Comparison")
        };
        let Expression::Binary { left, .. } = right else {
            panic!("expected Binary union")
        };
        let Expression::Lambda { pattern, .. } = *left else {
            panic!("expected Lambda on the left of the union")
        };
        let IdentPattern::Identifier(ti) = pattern else {
            panic!("expected single-identifier pattern")
        };
        assert!(
            ti.type_expr.is_some(),
            "first lambda's binder should now be typed: {ti:?}",
        );
    }

    #[test]
    fn setcomp_short_form_is_promoted_to_long_form() {
        // `{x ∣ x ∈ ℤ}` → `{x⦂ℤ · x ∈ ℤ ∣ x}`: ProB only accepts the
        // long form. The promotion is structural; type stamping is the
        // existing Group J pass.
        let e = parse_expression_str("{x ∣ x ∈ ℤ}").unwrap();
        let enriched = enrich_expression(e, &empty_env());
        let Expression::SetComprehension {
            identifiers,
            expression,
            ..
        } = enriched
        else {
            panic!("expected SetComprehension");
        };
        assert_eq!(identifiers.len(), 1);
        assert!(identifiers[0].type_expr.is_some(), "binder should be typed");
        let member = expression.expect("expression should be promoted to Some");
        assert_eq!(*member, Expression::Identifier("x".into()));
    }

    #[test]
    fn setcomp_short_form_multi_binder_uses_left_assoc_maplet() {
        let e = parse_expression_str("{x, y ∣ x ∈ ℤ ∧ y ∈ ℤ}").unwrap();
        let enriched = enrich_expression(e, &empty_env());
        let Expression::SetComprehension { expression, .. } = enriched else {
            panic!("expected SetComprehension");
        };
        let member = expression.expect("expression should be promoted to Some");
        let Expression::Binary { op, left, right } = *member else {
            panic!("expected Binary maplet");
        };
        assert_eq!(op, BinaryOp::Maplet);
        assert_eq!(*left, Expression::Identifier("x".into()));
        assert_eq!(*right, Expression::Identifier("y".into()));
    }

    #[test]
    fn setcomp_long_form_member_survives_untouched() {
        let e = parse_expression_str("{x ⦂ ℤ · x > 0 ∣ x + 1}").unwrap();
        let enriched = enrich_expression(e, &empty_env());
        let Expression::SetComprehension { expression, .. } = enriched else {
            panic!("expected SetComprehension");
        };
        let member = expression.expect("explicit member must survive");
        // `x + 1` parses to a Binary Add — the key point is that it is
        // NOT just `Identifier("x")`, which is what the promotion would
        // produce if it incorrectly fired here.
        assert!(matches!(
            *member,
            Expression::Binary {
                op: BinaryOp::Add,
                ..
            }
        ));
    }

    #[test]
    fn lambda_binder_typing_does_not_overwrite_explicit_type() {
        // If the binder already carries an explicit type, expected-type
        // propagation must not override it.
        let mut env = TypeEnv::new();
        env.insert("f", Type::pow(Type::prod(Type::Integer, Type::Integer)));
        let p = parse_predicate_str("f = (λ x⦂BOOL · x = TRUE ∣ 0)").unwrap();
        let enriched = enrich_predicate(p, &env);
        let Predicate::Comparison { right, .. } = enriched else {
            panic!("expected Comparison")
        };
        let Expression::Lambda { pattern, .. } = right else {
            panic!("expected Lambda")
        };
        let IdentPattern::Identifier(ti) = pattern else {
            panic!("expected single-identifier pattern")
        };
        // The explicit type was `BOOL`; it must survive even though the
        // expected type from `f` would say `ℤ`.
        let ty_expr = ti.type_expr.unwrap();
        let ty = parse_type_from_expression(&ty_expr).expect("type parseable");
        assert_eq!(ty, Type::Boolean);
    }
}
