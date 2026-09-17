//! Locate or collect free identifiers in predicates, expressions, and actions.
//!
//! A predicate or expression is *closed* with respect to a [`TypeEnv`] if
//! every identifier it references is either:
//!
//! - declared in the environment (a carrier set, constant, variable, parameter),
//! - bound locally by a quantifier / lambda / comprehension (including a
//!   such-that assignment's primed declarations), or
//! - a recognised built-in function name (`dom`, `ran`, `card`, …).
//!
//! Two flavours of traversal share the formula model's occurrence walker:
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
//! Bound occurrences resolve by de Bruijn index in the model, so the walker
//! answers "is this name bound?" structurally. Event parameters and other
//! outer locals are *free* identifiers to the model; the `with_locals`
//! variants treat those names as in scope by filtering on them.
//!
//! These read-side consumers act only on [`Role::Usage`] occurrences —
//! binder declarations, write targets, and predicate-call names are ignored,
//! preserving the "free identifiers on the read side" semantics. `x'` is
//! canonicalised to `x` by the collector.

use std::collections::BTreeSet;
use std::ops::ControlFlow;

use rossi::ast::Span;
use rossi::formula::occurrences::{self, Occurrence, Resolution, Role};
use rossi::names::is_primed_identifier;
use rossi::{ActionBody, Expression, Predicate};

use crate::type_env::TypeEnv;

// ---------- Public API: find-first variants --------------------------------

/// Whether the formula's cached free-name superset is fully covered by
/// `env` and the built-ins. The cache includes every name a find-first
/// walk could flag (plus write targets and application names, which it
/// wouldn't), so a covered cache proves the walk would find nothing and
/// the traversal can be skipped — the common case for well-formed
/// models.
fn cache_covered(free_names: &[String], env: &TypeEnv, declared: &[String]) -> bool {
    free_names
        .iter()
        .all(|name| env.contains(name) || declared.contains(name) || is_builtin_ident(name))
}

/// Locate the first free identifier in `pred`, considering `env` plus
/// locally-bound quantifier variables.
pub fn free_identifier_in_predicate(pred: &Predicate, env: &TypeEnv) -> Option<String> {
    undeclared_identifier_in_predicate(pred, env, &[])
}

/// Like [`free_identifier_in_predicate`], also treating the `declared`
/// names as known: the typing pass's test for a name nothing declares,
/// as opposed to a declared name that merely has no type yet.
pub fn undeclared_identifier_in_predicate(
    pred: &Predicate,
    env: &TypeEnv,
    declared: &[String],
) -> Option<String> {
    if cache_covered(pred.free_identifiers(), env, declared) {
        return None;
    }
    let mut v = FreeFinder {
        env,
        declared,
        found: None,
    };
    let _ = occurrences::walk_predicate(pred, &mut Vec::new(), &mut v);
    v.found
}

/// Locate the first free identifier in `expr`.
pub fn free_identifier_in_expression(expr: &Expression, env: &TypeEnv) -> Option<String> {
    if cache_covered(expr.free_identifiers(), env, &[]) {
        return None;
    }
    let mut v = FreeFinder {
        env,
        declared: &[],
        found: None,
    };
    let _ = occurrences::walk_expression(expr, &mut Vec::new(), &mut v);
    v.found
}

/// First free identifier on an action's read side, considering `env`
/// plus locally-bound variables (a such-that assignment binds its primed
/// declarations in the model, so `x'` reads resolve there).
pub fn free_identifier_in_action_rhs(body: &ActionBody, env: &TypeEnv) -> Option<String> {
    let assignment = body.assignment()?;
    if cache_covered(assignment.free_identifiers(), env, &[]) {
        return None;
    }
    let mut v = FreeFinder {
        env,
        declared: &[],
        found: None,
    };
    let _ = occurrences::walk_assignment(assignment, &mut Vec::new(), &mut v);
    v.found
}

/// Locate the first free primed occurrence in `pred`, with its span.
///
/// A primed name can never be declared (`crate::identifiers`), so this
/// decides "not in scope" without a `TypeEnv` — which is what lets the
/// loose-text and editor paths report it at all. Find-first, like
/// [`free_identifier_in_predicate`], so a clause draws one diagnostic
/// whichever path reports it.
pub fn first_free_primed_in_predicate(pred: &Predicate) -> Option<(String, Option<Span>)> {
    if !pred
        .free_identifiers()
        .iter()
        .any(|n| is_primed_identifier(n))
    {
        return None;
    }
    let mut v = PrimedFinder { found: None };
    let _ = occurrences::walk_predicate(pred, &mut Vec::new(), &mut v);
    v.found
}

/// First free primed name on an action's read side. No span: the SC anchors
/// an action diagnostic on the whole action, so its callers have nothing to
/// do with one. The primed declarations of a such-that assignment bind in
/// the model, so an after-state read of an assigned variable resolves and is
/// not reported.
pub fn first_free_primed_in_action_rhs(body: &ActionBody) -> Option<String> {
    let assignment = body.assignment()?;
    if !assignment
        .free_identifiers()
        .iter()
        .any(|n| is_primed_identifier(n))
    {
        return None;
    }
    let mut v = PrimedFinder { found: None };
    let _ = occurrences::walk_assignment(assignment, &mut Vec::new(), &mut v);
    v.found.map(|(name, _)| name)
}

/// Locate the first identifier in `pred` that appears in `forbidden` and
/// isn't bound locally. Used to drop guards / action RHS expressions that
/// reference variables which vanished to abstract-only in this refinement
/// (Group R).
pub fn first_forbidden_identifier_in_predicate(
    pred: &Predicate,
    forbidden: &BTreeSet<String>,
) -> Option<String> {
    if !pred
        .free_identifiers()
        .iter()
        .any(|n| forbidden.contains(n))
    {
        return None;
    }
    let mut v = ForbiddenFinder {
        forbidden,
        found: None,
    };
    let _ = occurrences::walk_predicate(pred, &mut Vec::new(), &mut v);
    v.found
}

/// First identifier on an action's read side that's in `forbidden` and
/// not bound locally (Group R).
pub fn first_forbidden_identifier_in_action_rhs(
    body: &ActionBody,
    forbidden: &BTreeSet<String>,
) -> Option<String> {
    let assignment = body.assignment()?;
    if !assignment
        .free_identifiers()
        .iter()
        .any(|n| forbidden.contains(n))
    {
        return None;
    }
    let mut v = ForbiddenFinder {
        forbidden,
        found: None,
    };
    let _ = occurrences::walk_assignment(assignment, &mut Vec::new(), &mut v);
    v.found
}

/// Source span of the first free `Usage` of `name` in `pred`, for
/// anchoring a diagnostic (e.g. "unknown identifier") on the exact
/// occurrence. Uses the same resolution rule as
/// [`free_identifier_in_predicate`], so it lands on the very occurrence
/// that scan flagged. `None` if `name` does not occur free (or the
/// occurrence carries no span, as for Rodin-XML imports).
pub fn usage_span_in_predicate(pred: &Predicate, name: &str) -> Option<Span> {
    let mut v = UsageSpanFinder { name, span: None };
    let _ = occurrences::walk_predicate(pred, &mut Vec::new(), &mut v);
    v.span
}

// ---------- Public API: collect-all variants -------------------------------

/// Insert every free identifier in `pred` into `acc`. Apostrophe-suffixed
/// names (`x'` read free, as in a witness predicate) are canonicalised to
/// the unprimed form before insertion, so `x'` counts as a use of `x`.
pub fn collect_referenced_in_predicate(pred: &Predicate, acc: &mut BTreeSet<String>) {
    collect_referenced_in_predicate_with_locals(pred, &[], acc);
}

/// Insert every free identifier in `expr` into `acc`. Same
/// canonicalisation as [`collect_referenced_in_predicate`].
pub fn collect_referenced_in_expression(expr: &Expression, acc: &mut BTreeSet<String>) {
    let mut v = IdentifierCollector { locals: &[], acc };
    let _ = occurrences::walk_expression(expr, &mut Vec::new(), &mut v);
}

/// Insert every free identifier on an action's read side into `acc`.
/// For a function override lowered by the parser, the function name on
/// the Overwrite RHS is a usage and collected here.
pub fn collect_referenced_in_action_rhs(body: &ActionBody, acc: &mut BTreeSet<String>) {
    collect_referenced_in_action_rhs_with_locals(body, &[], acc);
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
    let mut v = IdentifierCollector {
        locals: initial_locals,
        acc,
    };
    let _ = occurrences::walk_predicate(pred, &mut Vec::new(), &mut v);
}

/// Same as [`collect_referenced_in_action_rhs`] with initial bound
/// identifiers (event parameters).
pub fn collect_referenced_in_action_rhs_with_locals(
    body: &ActionBody,
    initial_locals: &[&str],
    acc: &mut BTreeSet<String>,
) {
    let Some(assignment) = body.assignment() else {
        return;
    };
    let mut v = IdentifierCollector {
        locals: initial_locals,
        acc,
    };
    let _ = occurrences::walk_assignment(assignment, &mut Vec::new(), &mut v);
}

/// Event-B built-in function names that are always "in scope" even though
/// they aren't declared in any context or machine. The relational atoms
/// (`id`/`prj1`/`prj2`/`pred`/`succ`) are not here: they parse as atomic
/// operators, which the walker never reports as an identifier usage.
/// `closure`/`closure1` are deliberately absent: core Event-B has no
/// closure operator (Rodin models axiomatise their own as a declared
/// constant), so treating them as built-ins would hide every use of such
/// a constant from the reference sets and exempt an undeclared
/// `closure(x)` from the free-identifier scan.
pub fn is_builtin_ident(name: &str) -> bool {
    matches!(name, "dom" | "ran" | "card" | "min" | "max")
}

// ---------- Visitor implementations ----------------------------------------

/// Is this occurrence a free read (not resolved by an enclosing
/// declaration)?
fn free_usage(occ: &Occurrence<'_>) -> bool {
    occ.role == Role::Usage && occ.resolution == Resolution::Free
}

struct FreeFinder<'a> {
    env: &'a TypeEnv,
    /// Names known besides `env` (declared but not yet typed).
    declared: &'a [String],
    found: Option<String>,
}

impl occurrences::OccurrenceVisitor for FreeFinder<'_> {
    fn visit(&mut self, occ: Occurrence<'_>) -> ControlFlow<()> {
        if !free_usage(&occ) {
            return ControlFlow::Continue(());
        }
        if self.env.contains(occ.name)
            || self.declared.iter().any(|name| name == occ.name)
            || is_builtin_ident(occ.name)
        {
            ControlFlow::Continue(())
        } else {
            self.found = Some(occ.name.to_string());
            ControlFlow::Break(())
        }
    }
}

/// Captures the first free primed read, with its span.
struct PrimedFinder {
    found: Option<(String, Option<Span>)>,
}

impl occurrences::OccurrenceVisitor for PrimedFinder {
    fn visit(&mut self, occ: Occurrence<'_>) -> ControlFlow<()> {
        if free_usage(&occ) && is_primed_identifier(occ.name) {
            self.found = Some((occ.name.to_string(), occ.span));
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    }
}

struct ForbiddenFinder<'a> {
    forbidden: &'a BTreeSet<String>,
    found: Option<String>,
}

impl occurrences::OccurrenceVisitor for ForbiddenFinder<'_> {
    fn visit(&mut self, occ: Occurrence<'_>) -> ControlFlow<()> {
        if !free_usage(&occ) {
            return ControlFlow::Continue(());
        }
        if self.forbidden.contains(occ.name) {
            self.found = Some(occ.name.to_string());
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
}

/// Captures the span of the first free `Usage` of a specific name.
struct UsageSpanFinder<'a> {
    name: &'a str,
    span: Option<Span>,
}

impl occurrences::OccurrenceVisitor for UsageSpanFinder<'_> {
    fn visit(&mut self, occ: Occurrence<'_>) -> ControlFlow<()> {
        // Match the raw occurrence text, mirroring `FreeFinder` (which reports
        // the unstripped name), so we anchor on the same occurrence it flagged.
        if free_usage(&occ) && occ.name == self.name {
            self.span = occ.span;
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    }
}

struct IdentifierCollector<'a> {
    /// Outer local names (event parameters) that count as in scope.
    locals: &'a [&'a str],
    acc: &'a mut BTreeSet<String>,
}

impl occurrences::OccurrenceVisitor for IdentifierCollector<'_> {
    fn visit(&mut self, occ: Occurrence<'_>) -> ControlFlow<()> {
        // A read counts when it is free, or when it is an after-state
        // read bound by a such-that assignment's primed declaration —
        // `x'` in the condition is a use of the variable `x`.
        let after_state_read = occ.role == Role::Usage && occ.is_after_state_read();
        if !(free_usage(&occ) || after_state_read)
            || is_builtin_ident(occ.name)
            || self.locals.contains(&occ.name)
        {
            return ControlFlow::Continue(());
        }
        // Strip the trailing apostrophe so an after-state read (bound
        // here, or free as in a witness predicate) is recorded as a use
        // of the unprimed name. Primes only ever appear on after-state
        // reads, so unconditional stripping is safe; a local also
        // shadows its primed spelling, matching the previous
        // binder-stack behaviour.
        let canonical = occ.name.strip_suffix('\'').unwrap_or(occ.name);
        if self.locals.contains(&canonical) {
            return ControlFlow::Continue(());
        }
        self.acc.insert(canonical.to_string());
        ControlFlow::Continue(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rossi::{parse_expression_str, parse_predicate_str};

    fn env_with(names: &[&str]) -> TypeEnv {
        use rossi::formula::Type;
        let mut env = TypeEnv::new();
        for n in names {
            env.add_carrier_set(n);
        }
        // Also add an integer constant so tests can reference a non-set name.
        env.insert("n", Type::Int);
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
        // `alice` is bound inside ∀ but free in the RHS of the ∧.
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
    fn set_builder_binds_its_member_identifiers() {
        // `{E ∣ P}` binds every identifier free in E over both sides.
        let env = env_with(&["USERS"]);
        let e = parse_expression_str("{x ∣ x ∈ USERS}").unwrap();
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
        for name in ["dom", "ran", "card", "min", "max"] {
            assert!(is_builtin_ident(name), "{name} should be builtin");
        }
        // Relational atoms are atomic operators, never identifiers — so
        // is_builtin_ident (an identifier-name check) deliberately excludes
        // them. `closure`/`closure1` are not core Event-B: models declare
        // their own closure constant, which must count as an identifier.
        for name in [
            "id", "prj1", "prj2", "pred", "succ", "foo", "", "users", "closure", "closure1",
        ] {
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
        // A free `x'` (as in a witness predicate) is canonicalised to `x`
        // so it counts as a use of the unprimed variable.
        let p = parse_predicate_str("x' = 0").unwrap();
        let mut refs = BTreeSet::new();
        collect_referenced_in_predicate(&p, &mut refs);
        assert!(
            refs.contains("x"),
            "expected x (stripped from x'): {refs:?}"
        );
        assert!(!refs.contains("x'"), "raw x' should not appear: {refs:?}");
    }

    #[test]
    fn primed_finder_reports_a_free_prime_and_skips_a_bound_one() {
        let p = parse_predicate_str("x' = 0").unwrap();
        let (name, _) = first_free_primed_in_predicate(&p).expect("x' is free here");
        assert_eq!(name, "x'");

        // Unprimed reads are none of this finder's business.
        assert!(first_free_primed_in_predicate(&parse_predicate_str("x = 0").unwrap()).is_none());

        // `x :∣ x' = x + 1` binds `x'`; `y'` in the same position is free.
        let bound = rossi::parse_action_str("x :∣ x' = x + 1").unwrap();
        assert!(first_free_primed_in_action_rhs(&bound).is_none());
        let free = rossi::parse_action_str("x :∣ y' = x + 1").unwrap();
        assert_eq!(
            first_free_primed_in_action_rhs(&free).as_deref(),
            Some("y'")
        );
    }
}
