//! Unified per-element checker: free-identifier scan + typed rebuild
//! in one call.
//!
//! Every static-checker call site that handles a labeled predicate
//! (axiom, invariant, guard, witness) needs the same two verdicts —
//! "is it closed against the environment" and "does it type-check" —
//! plus the typed rebuild itself. This module gives them one entry
//! point, so the "check then record" recipe doesn't have to be
//! open-coded everywhere. Canonical text is no longer part of the
//! result: the render layer derives it from the typed rebuild at
//! emission time.
//!
//! Action checking additionally surfaces the first free identifier on
//! the action's read side (RHS of `:=`, set of `:∈`, predicate of
//! `:|`, arguments + RHS of `f(x) := …`) for callers that want to
//! emit a diagnostic. LHS variable names are *not* checked here: they
//! are the write targets, and the SC validates them via the variable
//! table.

use rossi::ast::Span;
use rossi::{Assignment, Expression, LabeledPredicate, Predicate};

use crate::sc::identifier_walker::{
    free_identifier_in_action_rhs, free_identifier_in_expression, free_identifier_in_predicate,
    usage_span_in_predicate,
};
use crate::type_env::TypeEnv;
use crate::{Diagnostic, Severity};

/// Result of checking a labeled predicate.
#[derive(Debug, Clone)]
pub struct PredicateCheck {
    /// The predicate the check ran on. Kept on guard/axiom decls, where
    /// descendant (M1+) static checks re-read it to re-derive parameter
    /// types for extended events.
    pub predicate: Predicate,
    /// First identifier in the predicate that is neither in `env` nor
    /// bound by a local quantifier / lambda / set-comprehension. `None`
    /// iff the predicate is closed against `env`.
    pub free_identifier: Option<String>,
    /// The fully typed formula-model rebuild, when the predicate
    /// type-checks against `env`. `None` is the ill-typed verdict.
    pub typed: Option<rossi::formula::Predicate>,
}

/// Result of checking a standalone expression (currently only used by
/// the variant). Same shape as [`PredicateCheck`].
#[derive(Debug, Clone)]
pub struct ExpressionCheck {
    pub expression: Expression,
    pub free_identifier: Option<String>,
    /// See [`PredicateCheck::typed`].
    pub typed: Option<rossi::formula::Expression>,
}

/// Result of checking an action.
#[derive(Debug, Clone)]
pub struct ActionCheck {
    /// The action the check ran on (see [`PredicateCheck::predicate`]).
    pub action: Assignment,
    /// First free identifier on the action's read side. `None` iff
    /// every read identifier is in `env` (or a built-in).
    pub free_identifier: Option<String>,
    /// The fully typed formula-model rebuild, when the assignment
    /// type-checks against `env`. `None` for an ill-typed assignment.
    pub typed: Option<rossi::formula::Assignment>,
}

/// Check a predicate against `env`: the free-identifier scan and the
/// typed rebuild. An unknown identifier makes the formula unverifiable,
/// so the typed rebuild is not attempted (it could only fail).
pub fn check_predicate(p: &Predicate, env: &TypeEnv) -> PredicateCheck {
    let free_identifier = free_identifier_in_predicate(p, env);
    PredicateCheck {
        typed: free_identifier
            .is_none()
            .then(|| crate::sc::typing::typed_predicate(env, p))
            .flatten(),
        free_identifier,
        predicate: p.clone(),
    }
}

/// Check an expression against `env`. Used by the variant.
pub fn check_expression(e: &Expression, env: &TypeEnv) -> ExpressionCheck {
    let free_identifier = free_identifier_in_expression(e, env);
    ExpressionCheck {
        typed: free_identifier
            .is_none()
            .then(|| crate::sc::typing::typed_expression(env, e))
            .flatten(),
        free_identifier,
        expression: e.clone(),
    }
}

/// Check an action against `env`. Walks every read-side expression and
/// (for `:|`) the becomes-such-that predicate.
pub fn check_action(a: &Assignment, env: &TypeEnv) -> ActionCheck {
    let free_identifier = free_identifier_in_action_rhs(a, env);
    ActionCheck {
        typed: free_identifier
            .is_none()
            .then(|| crate::sc::typing::typed_assignment(env, a))
            .flatten(),
        free_identifier,
        action: a.clone(),
    }
}

/// The EB018 diagnostic for a name nothing declares, in the wording every
/// path uses. `place` names where it was read, as the message ends
/// (`"axiom predicate"` → "unknown identifier 'x' in axiom predicate";
/// `"action"` → "… in action"). Shared by the SC and by the
/// component-local pass in [`crate::identifiers`] — which reaches loose
/// text and the editor, where no `TypeEnv` exists — so the same finding
/// cannot be worded two ways.
#[must_use]
pub(crate) fn undeclared_identifier(
    bad: &str,
    place: &str,
    origin: String,
    span: Option<Span>,
) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        origin,
        message: format!("unknown identifier '{bad}' in {place}"),
        rule_id: Some(crate::RuleId::UndeclaredIdentifier),
        span,
    }
}

/// The EB020 diagnostic for a predicate that reads declared names before
/// the predicate that types them. `default_label`, `kind_name` and
/// `origin` are as for [`check_labeled_predicate`]; the kind also names
/// what should have typed `names`, the still-untyped identifiers.
/// Anchored on the first of them, falling back to the predicate's span.
#[must_use]
pub(crate) fn unknown_type(
    raw: &LabeledPredicate,
    default_label: &str,
    kind_name: &str,
    names: &[String],
    origin: impl FnOnce(&str) -> String,
) -> Diagnostic {
    let quoted: Vec<String> = names.iter().map(|name| format!("'{name}'")).collect();
    let span = names
        .first()
        .and_then(|name| usage_span_in_predicate(&raw.predicate, name))
        .or(raw.span);
    Diagnostic {
        severity: Severity::Error,
        origin: origin(raw.label.as_deref().unwrap_or(default_label)),
        message: format!(
            "{kind_name} predicate reads {} before any {kind_name} types {}",
            quoted.join(", "),
            if names.len() == 1 { "it" } else { "them" }
        ),
        rule_id: Some(crate::RuleId::UnknownType),
        span,
    }
}

/// Resolve a labeled predicate against `env` and produce the effective
/// label plus the full [`PredicateCheck`], or a [`Diagnostic`] if the
/// predicate references an unknown identifier.
///
/// This is the shared shape of axiom / invariant / guard checking:
///
/// - `default_label` is what we substitute when the source had no
///   label (Rodin uses `axm` / `inv` / `grd`; we follow suit).
/// - `kind_name` is the human-readable element type used in the
///   diagnostic message (e.g. `"axiom"` → "unknown identifier 'x' in
///   axiom predicate").
/// - `origin` builds the dotted origin string from the *effective*
///   label (`{ctx}.{lbl}`, `{mach}.{lbl}`, `{mach}.{event}.{lbl}`
///   are the three current shapes).
///
/// The caller does its own URI minting and decl construction — this
/// helper owns only the bits that are common to all three sites.
pub fn check_labeled_predicate(
    raw: &LabeledPredicate,
    env: &TypeEnv,
    default_label: &str,
    kind_name: &str,
    origin: impl FnOnce(&str) -> String,
) -> std::result::Result<(String, PredicateCheck), Diagnostic> {
    let pc = check_predicate(&raw.predicate, env);
    let label = raw
        .label
        .clone()
        .unwrap_or_else(|| default_label.to_string());
    if let Some(bad) = &pc.free_identifier {
        // Anchor on the offending identifier; fall back to the labeled
        // predicate's own span.
        let span = usage_span_in_predicate(&raw.predicate, bad).or(raw.span);
        return Err(undeclared_identifier(
            bad,
            &format!("{kind_name} predicate"),
            origin(&label),
            span,
        ));
    }
    if pc.typed.is_none() {
        return Err(Diagnostic {
            severity: Severity::Error,
            origin: origin(&label),
            message: format!("{kind_name} predicate is ill-typed"),
            rule_id: Some(crate::RuleId::TypeError),
            span: raw.span,
        });
    }
    Ok((label, pc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rossi::formula::Type;
    use rossi::{parse_action_str, parse_expression_str, parse_predicate_str};

    fn env_with_users() -> TypeEnv {
        let mut env = TypeEnv::new();
        env.add_carrier_set("USERS");
        env.insert("n", Type::Int);
        env
    }

    #[test]
    fn predicate_types_and_finds_no_free_when_closed() {
        let env = env_with_users();
        let p = parse_predicate_str("n ∈ ℕ").unwrap();
        let pc = check_predicate(&p, &env);
        assert!(pc.typed.is_some());
        assert_eq!(pc.free_identifier, None);
    }

    #[test]
    fn predicate_surfaces_first_free_identifier() {
        let env = env_with_users();
        let p = parse_predicate_str("alice ∈ USERS").unwrap();
        let pc = check_predicate(&p, &env);
        assert_eq!(pc.free_identifier.as_deref(), Some("alice"));
        assert!(pc.typed.is_none(), "an unknown identifier is unverifiable");
    }

    #[test]
    fn expression_check_threads_through() {
        let env = env_with_users();
        let e = parse_expression_str("n + 1").unwrap();
        let ec = check_expression(&e, &env);
        assert_eq!(ec.free_identifier, None);
        assert!(ec.typed.is_some());
    }

    #[test]
    fn action_check_skips_lhs_variable() {
        let mut env = TypeEnv::new();
        env.insert("x", Type::pow(Type::Given("USERS".into())));
        let a = parse_action_str("x ≔ ∅").unwrap();
        let ac = check_action(&a, &env);
        // `x` is the LHS — must not be flagged. `∅` is a literal.
        assert_eq!(ac.free_identifier, None);
        assert!(ac.typed.is_some());
    }

    #[test]
    fn action_check_flags_unknown_rhs_identifier() {
        let mut env = TypeEnv::new();
        env.insert("x", Type::Int);
        let a = parse_action_str("x ≔ y + 1").unwrap();
        let ac = check_action(&a, &env);
        assert_eq!(ac.free_identifier.as_deref(), Some("y"));
    }

    #[test]
    fn action_check_flags_unknown_type_ascription_identifier() {
        let mut env = TypeEnv::new();
        env.insert("x", Type::Int);
        let a = parse_action_str("x ≔ card(∅ ⦂ ℙ(UNKNOWN))").unwrap();
        let ac = check_action(&a, &env);
        assert_eq!(ac.free_identifier.as_deref(), Some("UNKNOWN"));
    }

    #[test]
    fn action_check_binds_primed_becomes_such_that_targets() {
        let mut env = TypeEnv::new();
        env.insert("x", Type::Int);
        let a = parse_action_str("x :∣ x' = x").unwrap();
        let ac = check_action(&a, &env);
        assert_eq!(ac.free_identifier, None);
    }
}
