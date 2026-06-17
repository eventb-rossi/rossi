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
//! Both flavours implement [`rossi::ast::walk::IdentVisitor`] over the shared
//! AST walker in the core crate (so the static checker, lint, and the language
//! server resolve identifiers through one traversal). The walker threads a
//! binder stack through the tree, so callers only need to provide the *outer*
//! type environment. The env is used for membership checks only; binder types
//! coming from quantifiers / lambdas don't need to be inferred to answer "is
//! this name bound?".
//!
//! The shared walker reports every identifier occurrence — reads, binder
//! declarations, write targets, and predicate-call names. These read-side
//! consumers act only on [`IdentRole::Usage`] occurrences, preserving the
//! original "free identifiers on the read side" semantics: write targets and
//! binder names are ignored, and `x'` is canonicalised to `x` by the collector.
//!
//! Identifiers in binder type annotations (`∀x⦂T·…`) are reported in the
//! enclosing scope (before the binder is pushed), so a carrier set or constant
//! used only as a bound-variable type is correctly recorded.

use std::collections::BTreeSet;
use std::ops::ControlFlow;

use rossi::ast::Span;
use rossi::ast::walk::{self, Binder, IdentOccurrence, IdentRole, IdentVisitor};
use rossi::{Action, Expression, Predicate};

use crate::type_env::TypeEnv;

/// Binder frame for the shared walker built from event-parameter names; these
/// outer locals carry no span (they come from declarations, not the formula).
fn locals_from(names: &[&str]) -> Vec<Binder> {
    names
        .iter()
        .map(|n| Binder {
            name: (*n).to_string(),
            span: None,
        })
        .collect()
}

// ---------- Public API: find-first variants --------------------------------

/// Locate the first free identifier in `pred`, considering `env` plus
/// locally-bound quantifier variables.
pub fn free_identifier_in_predicate(pred: &Predicate, env: &TypeEnv) -> Option<String> {
    let mut v = FreeFinder { env, found: None };
    let _ = walk::walk_predicate(pred, &mut Vec::new(), &mut v);
    v.found
}

/// Locate the first free identifier in `expr`.
pub fn free_identifier_in_expression(expr: &Expression, env: &TypeEnv) -> Option<String> {
    let mut v = FreeFinder { env, found: None };
    let _ = walk::walk_expression(expr, &mut Vec::new(), &mut v);
    v.found
}

/// First free identifier on an action's read side, considering `env`
/// plus locally-bound quantifier variables.
pub fn free_identifier_in_action_rhs(a: &Action, env: &TypeEnv) -> Option<String> {
    let mut v = FreeFinder { env, found: None };
    let _ = walk::walk_action(a, &mut Vec::new(), &mut v);
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
    let _ = walk::walk_predicate(pred, &mut Vec::new(), &mut v);
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
    let _ = walk::walk_action(a, &mut Vec::new(), &mut v);
    v.found
}

/// Source span of the first unshadowed `Usage` of `name` in `pred`, for
/// anchoring a diagnostic (e.g. "unknown identifier") on the exact occurrence.
/// Uses the same shadowing rule as [`free_identifier_in_predicate`], so it
/// lands on the very occurrence that scan flagged. `None` if `name` does not
/// occur free (or the occurrence carries no span, as for Rodin-XML imports).
pub fn usage_span_in_predicate(pred: &Predicate, name: &str) -> Option<Span> {
    let mut v = UsageSpanFinder { name, span: None };
    let _ = walk::walk_predicate(pred, &mut Vec::new(), &mut v);
    v.span
}

// ---------- Public API: collect-all variants -------------------------------

/// Insert every free identifier in `pred` into `acc`. Apostrophe-suffixed
/// names (`x'` from BeforeAfter predicates) are canonicalised to the
/// unprimed form before insertion, so `x'` counts as a use of `x`.
pub fn collect_referenced_in_predicate(pred: &Predicate, acc: &mut BTreeSet<String>) {
    let mut v = IdentifierCollector { acc };
    let _ = walk::walk_predicate(pred, &mut Vec::new(), &mut v);
}

/// Insert every free identifier in `expr` into `acc`. Same
/// canonicalisation as [`collect_referenced_in_predicate`].
pub fn collect_referenced_in_expression(expr: &Expression, acc: &mut BTreeSet<String>) {
    let mut v = IdentifierCollector { acc };
    let _ = walk::walk_expression(expr, &mut Vec::new(), &mut v);
}

/// Insert every free identifier on an action's read side into `acc`.
/// For `f ≔ f\u{E103}{(x ↦ E)}` (function override lowered by the parser),
/// the `f` on the Overwrite RHS is emitted as a Usage and collected here.
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
    let mut locals = locals_from(initial_locals);
    let mut v = IdentifierCollector { acc };
    let _ = walk::walk_predicate(pred, &mut locals, &mut v);
}

/// Same as [`collect_referenced_in_action_rhs`] with initial bound
/// identifiers (event parameters).
pub fn collect_referenced_in_action_rhs_with_locals(
    a: &Action,
    initial_locals: &[&str],
    acc: &mut BTreeSet<String>,
) {
    let mut locals = locals_from(initial_locals);
    let mut v = IdentifierCollector { acc };
    let _ = walk::walk_action(a, &mut locals, &mut v);
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
//
// These read-side consumers act only on `IdentRole::Usage`. Binder
// declarations, write targets, and predicate-call names reported by the shared
// walker are ignored here, which keeps the "free identifiers on the read side"
// semantics these callers have always relied on.

/// Is this name bound by an enclosing binder?
fn shadowed(name: &str, binders: &[Binder]) -> bool {
    binders.iter().any(|b| b.name == name)
}

struct FreeFinder<'a> {
    env: &'a TypeEnv,
    found: Option<String>,
}

impl IdentVisitor for FreeFinder<'_> {
    fn visit(&mut self, occ: IdentOccurrence<'_>) -> ControlFlow<()> {
        if occ.role != IdentRole::Usage {
            return ControlFlow::Continue(());
        }
        let name = occ.name;
        if shadowed(name, occ.binders) || self.env.contains(name) || is_builtin_ident(name) {
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

impl IdentVisitor for ForbiddenFinder<'_> {
    fn visit(&mut self, occ: IdentOccurrence<'_>) -> ControlFlow<()> {
        if occ.role != IdentRole::Usage {
            return ControlFlow::Continue(());
        }
        let name = occ.name;
        if shadowed(name, occ.binders) {
            ControlFlow::Continue(())
        } else if self.forbidden.contains(name) {
            self.found = Some(name.to_string());
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
}

/// Captures the span of the first unshadowed `Usage` of a specific name.
struct UsageSpanFinder<'a> {
    name: &'a str,
    span: Option<Span>,
}

impl IdentVisitor for UsageSpanFinder<'_> {
    fn visit(&mut self, occ: IdentOccurrence<'_>) -> ControlFlow<()> {
        if occ.role != IdentRole::Usage {
            return ControlFlow::Continue(());
        }
        // Match the raw occurrence text, mirroring `FreeFinder` (which reports
        // the unstripped name), so we anchor on the same occurrence it flagged.
        if occ.name == self.name && !shadowed(occ.name, occ.binders) {
            self.span = occ.span;
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    }
}

struct IdentifierCollector<'a> {
    acc: &'a mut BTreeSet<String>,
}

impl IdentVisitor for IdentifierCollector<'_> {
    fn visit(&mut self, occ: IdentOccurrence<'_>) -> ControlFlow<()> {
        if occ.role != IdentRole::Usage {
            return ControlFlow::Continue(());
        }
        let name = occ.name;
        if shadowed(name, occ.binders) || is_builtin_ident(name) {
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
    fn usage_span_locates_the_free_identifier() {
        // The span returned must cover exactly the offending identifier so a
        // diagnostic anchors on it rather than the whole predicate.
        let src = "alice ∈ USERS";
        let p = parse_predicate_str(src).unwrap();
        let span = usage_span_in_predicate(&p, "alice").expect("alice occurs free");
        assert_eq!(&src[span.start..span.end], "alice");
        // A name that does not occur yields no span.
        assert_eq!(usage_span_in_predicate(&p, "bob"), None);
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
