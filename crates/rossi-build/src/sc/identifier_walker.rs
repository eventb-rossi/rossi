//! Locate or collect free identifiers in predicates, expressions, and actions.
//!
//! A predicate or expression is *closed* with respect to a [`TypeEnv`] if
//! every identifier it references is either:
//!
//! - declared in the environment (a carrier set, constant, variable, parameter),
//! - bound locally by a quantifier / lambda / set-comprehension, or
//! - a recognised built-in function name (`dom`, `ran`, `card`, …).
//!
//! Two flavours of traversal share the same AST walker:
//!
//! 1. **Find-first** — [`free_identifier_in_predicate`] /
//!    [`free_identifier_in_expression`] / [`free_identifier_in_action_rhs`],
//!    plus [`first_forbidden_identifier_in_predicate`] /
//!    [`first_forbidden_identifier_in_action_rhs`]. Short-circuits on the
//!    first hit. Used by the SC pipeline.
//! 2. **Collect-all** — [`collect_referenced_in_predicate`] /
//!    [`collect_referenced_in_expression`] /
//!    [`collect_referenced_in_action_rhs`]. Walks the whole tree and inserts
//!    every free identifier into a [`BTreeSet`]. Used by the lint module.
//!
//! Both flavours implement the [`Visitor`] trait. The walker threads a
//! binder stack through the tree, so callers only need to provide the
//! *outer* type environment. The env is used for membership checks only;
//! binder types coming from quantifiers / lambdas don't need to be
//! inferred to answer "is this name bound?".
//!
//! Identifiers in binder type annotations (`∀x⦂T·…`) are visited in the
//! enclosing scope (before the binder is pushed), so a carrier set or
//! constant used only as a bound-variable type is correctly recorded.

use std::collections::BTreeSet;
use std::ops::ControlFlow;

use rossi::{Action, Expression, IdentPattern, Predicate, TypedIdentifier};

use crate::type_env::TypeEnv;

/// Visitor invoked for every free identifier the walker encounters.
///
/// `name` is the identifier's textual form (apostrophe-suffixed for the
/// after-state form `x'` in BeforeAfter predicates). `locals` is the
/// snapshot of binder names currently in scope, innermost last.
///
/// Returning [`ControlFlow::Break`] aborts the rest of the traversal —
/// useful for first-hit finders. Returning [`ControlFlow::Continue`] keeps
/// walking.
pub trait Visitor {
    fn visit_ident(&mut self, name: &str, locals: &[String]) -> ControlFlow<()>;
}

// ---------- Public API: find-first variants --------------------------------

/// Locate the first free identifier in `pred`, considering `env` plus
/// locally-bound quantifier variables.
pub fn free_identifier_in_predicate(pred: &Predicate, env: &TypeEnv) -> Option<String> {
    let mut v = FreeFinder { env, found: None };
    let _ = walk_pred(pred, &mut Vec::new(), &mut v);
    v.found
}

/// Locate the first free identifier in `expr`.
pub fn free_identifier_in_expression(expr: &Expression, env: &TypeEnv) -> Option<String> {
    let mut v = FreeFinder { env, found: None };
    let _ = walk_expr(expr, &mut Vec::new(), &mut v);
    v.found
}

/// First free identifier on an action's read side, considering `env`
/// plus locally-bound quantifier variables.
pub fn free_identifier_in_action_rhs(a: &Action, env: &TypeEnv) -> Option<String> {
    let mut v = FreeFinder { env, found: None };
    let _ = walk_action_rhs(a, &mut Vec::new(), &mut v);
    v.found
}

/// Locate the first identifier in `pred` that appears in `forbidden` and
/// isn't shadowed by a local binder. Used to drop guards / action RHS
/// expressions that reference variables which vanished to abstract-only
/// in this refinement (Group R).
pub fn first_forbidden_identifier_in_predicate(
    pred: &Predicate,
    forbidden: &BTreeSet<String>,
) -> Option<String> {
    let mut v = ForbiddenFinder {
        forbidden,
        found: None,
    };
    let _ = walk_pred(pred, &mut Vec::new(), &mut v);
    v.found
}

/// First identifier on an action's read side that's in `forbidden`
/// and not shadowed by a local binder (Group R).
pub fn first_forbidden_identifier_in_action_rhs(
    a: &Action,
    forbidden: &BTreeSet<String>,
) -> Option<String> {
    let mut v = ForbiddenFinder {
        forbidden,
        found: None,
    };
    let _ = walk_action_rhs(a, &mut Vec::new(), &mut v);
    v.found
}

// ---------- Public API: collect-all variants -------------------------------

/// Insert every free identifier in `pred` into `acc`. Apostrophe-suffixed
/// names (`x'` from BeforeAfter predicates) are canonicalised to the
/// unprimed form before insertion, so `x'` counts as a use of `x`.
pub fn collect_referenced_in_predicate(pred: &Predicate, acc: &mut BTreeSet<String>) {
    let mut v = IdentifierCollector { acc };
    let _ = walk_pred(pred, &mut Vec::new(), &mut v);
}

/// Insert every free identifier in `expr` into `acc`. Same
/// canonicalisation as [`collect_referenced_in_predicate`].
pub fn collect_referenced_in_expression(expr: &Expression, acc: &mut BTreeSet<String>) {
    let mut v = IdentifierCollector { acc };
    let _ = walk_expr(expr, &mut Vec::new(), &mut v);
}

/// Insert every free identifier on an action's read side into `acc`.
/// `function: f` in `Action::FunctionOverride` is **not** added here —
/// callers that consider override targets as reads must insert `f`
/// themselves (the walker treats action RHS as a pure read of expressions
/// and predicates).
pub fn collect_referenced_in_action_rhs(a: &Action, acc: &mut BTreeSet<String>) {
    collect_referenced_in_action_rhs_with_locals(a, &[], acc);
}

/// Same as [`collect_referenced_in_predicate`] but treats `initial_locals`
/// as already-bound identifiers — used to thread event parameters into the
/// scope of guards / witnesses / actions so a parameter name doesn't leak
/// into the machine-level reference set.
pub fn collect_referenced_in_predicate_with_locals(
    pred: &Predicate,
    initial_locals: &[&str],
    acc: &mut BTreeSet<String>,
) {
    let mut locals: Vec<String> = initial_locals.iter().map(|s| s.to_string()).collect();
    let mut v = IdentifierCollector { acc };
    let _ = walk_pred(pred, &mut locals, &mut v);
}

/// Same as [`collect_referenced_in_action_rhs`] with initial bound
/// identifiers (event parameters).
pub fn collect_referenced_in_action_rhs_with_locals(
    a: &Action,
    initial_locals: &[&str],
    acc: &mut BTreeSet<String>,
) {
    let mut locals: Vec<String> = initial_locals.iter().map(|s| s.to_string()).collect();
    let mut v = IdentifierCollector { acc };
    let _ = walk_action_rhs(a, &mut locals, &mut v);
}

/// Event-B built-in function names that are always "in scope" even though
/// they aren't declared in any context or machine.
pub fn is_builtin_ident(name: &str) -> bool {
    matches!(
        name,
        "dom" | "ran" | "id" | "prj1" | "prj2" | "card" | "min" | "max" | "closure" | "closure1"
    )
}

// ---------- Visitor implementations ----------------------------------------

struct FreeFinder<'a> {
    env: &'a TypeEnv,
    found: Option<String>,
}

impl Visitor for FreeFinder<'_> {
    fn visit_ident(&mut self, name: &str, locals: &[String]) -> ControlFlow<()> {
        if locals.iter().any(|l| l == name) || self.env.contains(name) || is_builtin_ident(name) {
            ControlFlow::Continue(())
        } else {
            self.found = Some(name.to_string());
            ControlFlow::Break(())
        }
    }
}

struct ForbiddenFinder<'a> {
    forbidden: &'a BTreeSet<String>,
    found: Option<String>,
}

impl Visitor for ForbiddenFinder<'_> {
    fn visit_ident(&mut self, name: &str, locals: &[String]) -> ControlFlow<()> {
        if locals.iter().any(|l| l == name) {
            ControlFlow::Continue(())
        } else if self.forbidden.contains(name) {
            self.found = Some(name.to_string());
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
}

struct IdentifierCollector<'a> {
    acc: &'a mut BTreeSet<String>,
}

impl Visitor for IdentifierCollector<'_> {
    fn visit_ident(&mut self, name: &str, locals: &[String]) -> ControlFlow<()> {
        if locals.iter().any(|l| l == name) || is_builtin_ident(name) {
            return ControlFlow::Continue(());
        }
        // Strip the trailing apostrophe so `x'` in a BecomesSuchThat
        // before-after predicate is recorded as a use of `x`. The grammar
        // (rossi/src/grammar.pest:12) parses `x'` as a single identifier;
        // primes only ever appear in BeforeAfter predicates, so
        // unconditional stripping is safe.
        let canonical = name.strip_suffix('\'').unwrap_or(name);
        self.acc.insert(canonical.to_string());
        ControlFlow::Continue(())
    }
}

// ---------- Internal walkers -----------------------------------------------

fn walk_action_rhs<V: Visitor>(
    action: &Action,
    locals: &mut Vec<String>,
    v: &mut V,
) -> ControlFlow<()> {
    match action {
        Action::Skip => ControlFlow::Continue(()),
        Action::Assignment { expressions, .. } => {
            for e in expressions {
                walk_expr(e, locals, v)?;
            }
            ControlFlow::Continue(())
        }
        Action::BecomesIn { set, .. } => walk_expr(set, locals, v),
        Action::BecomesSuchThat { predicate, .. } => walk_pred(predicate, locals, v),
        Action::FunctionOverride {
            arguments,
            expression,
            ..
        } => {
            for a in arguments {
                walk_expr(a, locals, v)?;
            }
            walk_expr(expression, locals, v)
        }
    }
}

fn walk_pred<V: Visitor>(p: &Predicate, locals: &mut Vec<String>, v: &mut V) -> ControlFlow<()> {
    use Predicate as P;
    match p {
        P::True | P::False => ControlFlow::Continue(()),
        P::Comparison { left, right, .. } => {
            walk_expr(left, locals, v)?;
            walk_expr(right, locals, v)
        }
        P::Not(inner) => walk_pred(inner, locals, v),
        P::Logical { left, right, .. } => {
            walk_pred(left, locals, v)?;
            walk_pred(right, locals, v)
        }
        P::Quantified {
            identifiers,
            predicate,
            ..
        } => {
            walk_binder_types(identifiers, locals, v)?;
            with_binders(
                locals,
                identifiers.iter().map(|ti| ti.name.as_str()),
                |locals| walk_pred(predicate, locals, v),
            )
        }
        P::Application { arguments, .. } | P::BuiltinApplication { arguments, .. } => {
            for a in arguments {
                walk_expr(a, locals, v)?;
            }
            ControlFlow::Continue(())
        }
    }
}

fn walk_expr<V: Visitor>(e: &Expression, locals: &mut Vec<String>, v: &mut V) -> ControlFlow<()> {
    use Expression as E;
    match e {
        E::Identifier(n) => v.visit_ident(n, locals),
        E::Integer(_)
        | E::True
        | E::False
        | E::EmptySet
        | E::Naturals
        | E::Naturals1
        | E::Integers
        | E::BoolType
        | E::StringLiteral(_) => ControlFlow::Continue(()),
        E::Binary { left, right, .. } => {
            walk_expr(left, locals, v)?;
            walk_expr(right, locals, v)
        }
        E::Unary { operand, .. } => walk_expr(operand, locals, v),
        E::FunctionApplication {
            function,
            arguments,
        } => {
            walk_expr(function, locals, v)?;
            for a in arguments {
                walk_expr(a, locals, v)?;
            }
            ControlFlow::Continue(())
        }
        E::BuiltinApplication { arguments, .. } => {
            for a in arguments {
                walk_expr(a, locals, v)?;
            }
            ControlFlow::Continue(())
        }
        E::SetEnumeration(items) => {
            for a in items {
                walk_expr(a, locals, v)?;
            }
            ControlFlow::Continue(())
        }
        E::Bool(p) => walk_pred(p, locals, v),
        E::RelationalImage { relation, set } => {
            walk_expr(relation, locals, v)?;
            walk_expr(set, locals, v)
        }
        E::IfThenElse {
            condition,
            then_expr,
            else_expr,
        } => {
            walk_pred(condition, locals, v)?;
            walk_expr(then_expr, locals, v)?;
            walk_expr(else_expr, locals, v)
        }
        E::SetComprehension {
            identifiers,
            predicate,
            expression,
        } => {
            walk_binder_types(identifiers, locals, v)?;
            with_binders(
                locals,
                identifiers.iter().map(|ti| ti.name.as_str()),
                |locals| {
                    walk_pred(predicate, locals, v)?;
                    if let Some(e) = expression {
                        walk_expr(e, locals, v)?;
                    }
                    ControlFlow::Continue(())
                },
            )
        }
        E::SetBuilder {
            member_expression,
            predicate,
        } => {
            walk_expr(member_expression, locals, v)?;
            walk_pred(predicate, locals, v)
        }
        E::Lambda {
            pattern,
            predicate,
            expression,
        } => {
            walk_pattern_types(pattern, locals, v)?;
            let names = pattern.identifiers();
            with_binders(locals, names, |locals| {
                walk_pred(predicate, locals, v)?;
                walk_expr(expression, locals, v)
            })
        }
        E::QuantifiedUnion {
            identifiers,
            predicate,
            expression,
        }
        | E::QuantifiedInter {
            identifiers,
            predicate,
            expression,
        } => {
            walk_binder_types(identifiers, locals, v)?;
            with_binders(
                locals,
                identifiers.iter().map(|ti| ti.name.as_str()),
                |locals| {
                    walk_pred(predicate, locals, v)?;
                    walk_expr(expression, locals, v)
                },
            )
        }
    }
}

/// Walk the optional `type_expr` of each binder in the *outer* scope —
/// type annotations like `∀x⦂T·…` can reference identifiers that must be
/// declared in the enclosing environment, not introduced by the binder.
fn walk_binder_types<V: Visitor>(
    binders: &[TypedIdentifier],
    locals: &mut Vec<String>,
    v: &mut V,
) -> ControlFlow<()> {
    for ti in binders {
        if let Some(t) = &ti.type_expr {
            walk_expr(t, locals, v)?;
        }
    }
    ControlFlow::Continue(())
}

/// Walk the `type_expr` of every leaf `TypedIdentifier` in a lambda
/// pattern, in the *outer* scope.
fn walk_pattern_types<V: Visitor>(
    pattern: &IdentPattern,
    locals: &mut Vec<String>,
    v: &mut V,
) -> ControlFlow<()> {
    match pattern {
        IdentPattern::Identifier(ti) => {
            if let Some(t) = &ti.type_expr {
                walk_expr(t, locals, v)?;
            }
            ControlFlow::Continue(())
        }
        IdentPattern::Maplet(l, r) => {
            walk_pattern_types(l, locals, v)?;
            walk_pattern_types(r, locals, v)
        }
    }
}

/// Scope guard: push `names` onto `locals`, run `body`, then truncate back.
fn with_binders<'a, I, F>(locals: &mut Vec<String>, names: I, body: F) -> ControlFlow<()>
where
    I: IntoIterator<Item = &'a str>,
    F: FnOnce(&mut Vec<String>) -> ControlFlow<()>,
{
    let depth = locals.len();
    for n in names {
        locals.push(n.to_string());
    }
    let r = body(locals);
    locals.truncate(depth);
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use rossi::{parse_expression_str, parse_predicate_str};

    fn env_with(names: &[&str]) -> TypeEnv {
        use crate::types::Type;
        let mut env = TypeEnv::new();
        for n in names {
            env.add_carrier_set(n);
        }
        // Also add an integer constant so tests can reference a non-set name.
        env.insert("n", Type::Integer);
        env
    }

    #[test]
    fn plain_membership_all_resolved() {
        let env = env_with(&["USERS"]);
        let p = parse_predicate_str("n ∈ USERS").unwrap();
        assert_eq!(free_identifier_in_predicate(&p, &env), None);
    }

    #[test]
    fn catches_free_identifier() {
        let env = env_with(&["USERS"]);
        let p = parse_predicate_str("alice ∈ USERS").unwrap();
        assert_eq!(
            free_identifier_in_predicate(&p, &env).as_deref(),
            Some("alice")
        );
    }

    #[test]
    fn quantified_binder_shadows_free() {
        // `alice` is not in env, but is bound by the quantifier.
        let env = env_with(&["USERS"]);
        let p = parse_predicate_str("∀alice · alice ∈ USERS").unwrap();
        assert_eq!(free_identifier_in_predicate(&p, &env), None);
    }

    #[test]
    fn quantifier_scope_restored_after_body() {
        // `alice` is bound inside ∀ but free in the RHS of the ⇒.
        let env = env_with(&["USERS"]);
        let p = parse_predicate_str("(∀alice · alice ∈ USERS) ∧ (alice ∈ USERS)").unwrap();
        assert_eq!(
            free_identifier_in_predicate(&p, &env).as_deref(),
            Some("alice")
        );
    }

    #[test]
    fn builtin_functions_are_in_scope() {
        let env = env_with(&["USERS"]);
        let p = parse_predicate_str("∀f · card(f) ≥ 0").unwrap();
        assert_eq!(free_identifier_in_predicate(&p, &env), None);
    }

    #[test]
    fn set_comprehension_binders_scope() {
        let env = env_with(&["USERS"]);
        let e = parse_expression_str("{x · x ∈ USERS | x}").unwrap();
        assert_eq!(free_identifier_in_expression(&e, &env), None);
    }

    #[test]
    fn nested_quantifiers_stack_correctly() {
        let env = env_with(&["USERS"]);
        let p = parse_predicate_str("∀a · (∀b · a ∈ USERS ∧ b ∈ USERS)").unwrap();
        assert_eq!(free_identifier_in_predicate(&p, &env), None);
    }

    #[test]
    fn lambda_pattern_binders() {
        let env = env_with(&["USERS"]);
        let e = parse_expression_str("λx ↦ y · x ∈ USERS ∧ y ∈ USERS | x").unwrap();
        assert_eq!(free_identifier_in_expression(&e, &env), None);
    }

    #[test]
    fn builtin_recognition() {
        for name in ["dom", "ran", "card", "min", "max", "id", "prj1", "prj2"] {
            assert!(is_builtin_ident(name), "{name} should be builtin");
        }
        for name in ["foo", "", "users", "unknown"] {
            assert!(!is_builtin_ident(name), "{name} should not be builtin");
        }
    }

    #[test]
    fn type_annotation_keeps_set_alive() {
        // ∀x⦂SET · x ∈ SET — collector should report `SET` even though it
        // only appears in the binder's type annotation.
        let env = env_with(&["SET"]);
        let p = parse_predicate_str("∀x⦂SET · x ∈ SET").unwrap();

        // FreeFinder should report no free idents (SET is in env).
        assert_eq!(free_identifier_in_predicate(&p, &env), None);

        // Collector should record SET (from both the annotation and the body).
        let mut refs = BTreeSet::new();
        collect_referenced_in_predicate(&p, &mut refs);
        assert!(refs.contains("SET"), "expected SET in refs: {refs:?}");
    }

    #[test]
    fn type_annotation_in_isolation_is_detected() {
        // ∀x⦂T · true — body doesn't mention T, but the annotation does.
        // Collector should still record T.
        let p = parse_predicate_str("∀x⦂T · ⊤").unwrap();
        let mut refs = BTreeSet::new();
        collect_referenced_in_predicate(&p, &mut refs);
        assert!(
            refs.contains("T"),
            "expected T in refs from annotation: {refs:?}"
        );
    }

    #[test]
    fn collector_strips_primed_apostrophe() {
        // `x' = 0` in a BSU before-after predicate. Collector canonicalises
        // `x'` to `x` so it counts as a use of the unprimed variable.
        let p = parse_predicate_str("x' = 0").unwrap();
        let mut refs = BTreeSet::new();
        collect_referenced_in_predicate(&p, &mut refs);
        assert!(
            refs.contains("x"),
            "expected x (stripped from x'): {refs:?}"
        );
        assert!(!refs.contains("x'"), "raw x' should not appear: {refs:?}");
    }
}
