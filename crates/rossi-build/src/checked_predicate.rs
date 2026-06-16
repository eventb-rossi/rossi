//! Unified per-element checker: free-identifier scan + canonical form
//! in one call.
//!
//! Every static-checker call site that handles a labeled predicate
//! (axiom, invariant, guard, witness) used to invoke
//! `free_identifier_in_predicate` (in `sc::identifier_walker`) and
//! [`canonical_predicate`] in succession. This module gives them one
//! entry point, so the "check then emit" recipe doesn't have to be
//! open-coded everywhere.
//!
//! The current implementation is a thin façade — it calls the two
//! existing functions internally. Fusing them into a single tree walk
//! (drop the regex tightening pass, stream tightened tokens straight
//! out of a custom pretty-printer) is a follow-up; the public shape
//! here is designed to absorb that change without touching call sites.
//!
//! Action checking is split:
//! - [`crate::normalize::canonical_action`] always returns a string (matches today's
//!   behaviour — actions are never dropped on a free-ident error;
//!   the diagnostic surfaces but the row still emits).
//! - [`check_action`] additionally surfaces the first free identifier
//!   on the action's read side (RHS of `:=`, set of `:∈`, predicate
//!   of `:|`, arguments + RHS of `f(x) := …`) for callers that want
//!   to emit a diagnostic. LHS variable names are *not* checked here:
//!   they are the write targets, and Rodin's SC already validates
//!   them via the variable table.

use rossi::{Action, Expression, LabeledPredicate, Predicate};

use crate::enrich::{enrich_action, enrich_expression, enrich_predicate};
use crate::normalize::{canonical_action_with_env, canonical_expression, canonical_predicate};
use crate::sc::identifier_walker::{
    free_identifier_in_action_rhs, free_identifier_in_expression, free_identifier_in_predicate,
    usage_span_in_predicate,
};
use crate::type_env::TypeEnv;
use crate::{Diagnostic, Severity};

/// Result of checking a labeled predicate.
#[derive(Debug, Clone)]
pub struct PredicateCheck {
    /// The [enriched](crate::enrich::enrich_predicate) predicate the
    /// check ran on: binder types stamped, short-form comprehensions
    /// lowered. `canonical` is a rendering of exactly this AST. Kept on
    /// guard/axiom decls, where descendant (M1+) static checks re-read it
    /// to re-derive parameter types for extended events.
    pub predicate: Predicate,
    /// Rodin-canonical formatting of the predicate.
    pub canonical: String,
    /// First identifier in the predicate that is neither in `env` nor
    /// bound by a local quantifier / lambda / set-comprehension. `None`
    /// iff the predicate is closed against `env`.
    pub free_identifier: Option<String>,
}

/// Result of checking a standalone expression (currently only used by
/// the variant). Same shape as [`PredicateCheck`].
#[derive(Debug, Clone)]
pub struct ExpressionCheck {
    pub canonical: String,
    pub free_identifier: Option<String>,
}

/// Result of checking an action.
#[derive(Debug, Clone)]
pub struct ActionCheck {
    /// The enriched action the check ran on (see
    /// [`PredicateCheck::predicate`]).
    pub action: Action,
    /// Rodin-canonical assignment text, with empty-set RHS annotated
    /// against the LHS type when known (see
    /// [`canonical_action_with_env`]).
    pub canonical: String,
    /// First free identifier on the action's read side. `None` iff
    /// every read identifier is in `env` (or a built-in).
    pub free_identifier: Option<String>,
}

/// Check a predicate against `env`, producing both the canonical form
/// and the first free identifier (if any).
///
/// Both canonicalisation and free-identifier scanning run on the
/// [enriched](crate::enrich::enrich_predicate) form so that:
///
/// 1. Quantifier and lambda binders carry their inferred types in the
///    emitted text (matching Rodin's bcc form).
/// 2. Short-form set comprehensions `{E ∣ P}` are lowered to the long
///    form `{x⦂T · P ∣ E}` before scoping is resolved, so identifiers
///    bound implicitly by the short form are not flagged as free.
pub fn check_predicate(p: &Predicate, env: &TypeEnv) -> PredicateCheck {
    let enriched = enrich_predicate(p.clone(), env);
    PredicateCheck {
        free_identifier: free_identifier_in_predicate(&enriched, env),
        canonical: canonical_predicate(&enriched),
        predicate: enriched,
    }
}

/// Check an expression against `env`. Used by the variant.
pub fn check_expression(e: &Expression, env: &TypeEnv) -> ExpressionCheck {
    let enriched = enrich_expression(e.clone(), env);
    ExpressionCheck {
        free_identifier: free_identifier_in_expression(&enriched, env),
        canonical: canonical_expression(&enriched),
    }
}

/// Check an action against `env`. Walks every read-side expression and
/// (for `:|`) the becomes-such-that predicate.
pub fn check_action(a: &Action, env: &TypeEnv) -> ActionCheck {
    let enriched = enrich_action(a.clone(), env);
    ActionCheck {
        free_identifier: free_identifier_in_action_rhs(&enriched, env),
        canonical: canonical_action_with_env(&enriched, env),
        action: enriched,
    }
}

/// Resolve a labeled predicate against `env` and produce the effective
/// label plus the full [`PredicateCheck`] (enriched AST + canonical
/// text), or a [`Diagnostic`] if the predicate references an unknown
/// identifier.
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
        // Anchor on the offending identifier in the *source* predicate (the
        // enriched form may have rebuilt nodes without spans); fall back to the
        // labeled predicate's own span.
        let span = usage_span_in_predicate(&raw.predicate, bad).or(raw.span);
        return Err(Diagnostic {
            severity: Severity::Error,
            origin: origin(&label),
            message: format!("unknown identifier '{bad}' in {kind_name} predicate"),
            rule_id: Some(crate::RuleId::UndeclaredIdentifier),
            span,
        });
    }
    Ok((label, pc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Type;
    use rossi::{parse_action_str, parse_expression_str, parse_predicate_str};

    fn env_with_users() -> TypeEnv {
        let mut env = TypeEnv::new();
        env.add_carrier_set("USERS");
        env.insert("n", Type::Integer);
        env
    }

    #[test]
    fn predicate_returns_canonical_and_no_free_when_closed() {
        let env = env_with_users();
        let p = parse_predicate_str("n ∈ ℕ").unwrap();
        let pc = check_predicate(&p, &env);
        assert_eq!(pc.canonical, "n∈ℕ");
        assert_eq!(pc.free_identifier, None);
    }

    #[test]
    fn predicate_surfaces_first_free_identifier() {
        let env = env_with_users();
        let p = parse_predicate_str("alice ∈ USERS").unwrap();
        let pc = check_predicate(&p, &env);
        assert_eq!(pc.free_identifier.as_deref(), Some("alice"));
        assert_eq!(pc.canonical, "alice∈USERS");
    }

    #[test]
    fn expression_check_threads_through() {
        let env = env_with_users();
        let e = parse_expression_str("n + 1").unwrap();
        let ec = check_expression(&e, &env);
        assert_eq!(ec.free_identifier, None);
        assert!(!ec.canonical.is_empty());
    }

    #[test]
    fn action_check_skips_lhs_variable() {
        let mut env = TypeEnv::new();
        env.insert("x", Type::pow(Type::GivenSet("USERS".into())));
        let a = parse_action_str("x ≔ ∅").unwrap();
        let ac = check_action(&a, &env);
        // `x` is the LHS — must not be flagged. `∅` is a literal.
        assert_eq!(ac.free_identifier, None);
        assert_eq!(ac.canonical, "x ≔ ∅ ⦂ ℙ(USERS)");
    }

    #[test]
    fn action_check_flags_unknown_rhs_identifier() {
        let mut env = TypeEnv::new();
        env.insert("x", Type::Integer);
        let a = parse_action_str("x ≔ y + 1").unwrap();
        let ac = check_action(&a, &env);
        assert_eq!(ac.free_identifier.as_deref(), Some("y"));
    }
}
