//! Declared-name validity that no project context can decide (EB033).
//!
//! Event-B spells the post-value of an assigned variable with a trailing
//! prime, so the prime belongs to a formula and never to a declaration.
//! Rodin enforces that in its static checker: `IdentifierModule` parses
//! every carrier set, constant, variable and event parameter with
//! `primeAllowed = false` and reports `InvalidIdentifierError` for a primed
//! one, dropping the name from the checked model. The single caller passing
//! `primeAllowed = true` is `MachineEventWitnessModule`, for a witness
//! *label*.
//!
//! The parser is deliberately not the place for it. `parser.rs`'s
//! `declared_name` gate is shared by three positions, and only one of them
//! may refuse a prime: an assignment target may already be primed (Rodin's
//! `MainParsers.makePrimedDecl` branches on `isPrimed()` precisely so `x' :∣
//! …` is not double-primed) and a quantifier binder may be (that is how the
//! model spells a such-that declaration). Rodin refuses a primed
//! declaration from an *SC module*, not from its parser or its database, so
//! checking it here is parity rather than compromise — and a Rodin
//! `.buc`/`.bum` that stores such a name still imports.
//!
//! The rule reads one name at a time, so — like [`crate::duplicates`] — it
//! has two views: this module reports the diagnostics for `rossi validate`'s
//! loose-text path, the LSP and the SC's up-front pass, while the SC's
//! per-component checks filter the offending names out of their output with
//! [`rossi::names::is_primed_identifier`] directly.
//!
//! A primed declaration being impossible is also what makes a *use* of a
//! primed name decidable without a project: no SEES / EXTENDS parent and no
//! abstract machine can ever put one in scope. That is what
//! [`component_undeclarable_prime_diagnostics`] reports (as EB018, Rodin's
//! `UndeclaredFreeIdentifierError`) for `rossi validate`'s loose-text path
//! and the editor, where the SC does not run and no `TypeEnv` exists.

use rossi::names::is_primed_identifier;
use rossi::{Component, LabeledAction, LabeledPredicate};

use crate::checked_predicate::undeclared_identifier;
use crate::sc::identifier_walker::{
    first_free_primed_in_action_rhs, first_free_primed_in_predicate,
};
use crate::{Diagnostic, RuleId};

/// EB033 — one error per declared name carrying the after-state prime.
///
/// Only the identifier sites can be primed: a component or event name is a
/// `component_name`, whose grammar has no prime at all.
#[must_use]
pub fn component_primed_name_diagnostics(component: &Component) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    crate::lint::for_each_declared_name(component, |origin, kind, name, site, span| {
        if !site.is_math_identifier() || !is_primed_identifier(name) {
            return;
        }
        diags.push(Diagnostic {
            severity: RuleId::PrimedDeclaredName.default_severity(),
            origin: format!("{origin}.{name}"),
            message: format!(
                "{kind} `{name}` is declared with the after-state prime, which \
                 only a witness label may carry — rename it without the `'`"
            ),
            rule_id: Some(RuleId::PrimedDeclaredName),
            span,
        });
    });
    diags
}

/// EB018 — one error per clause reading a primed name nothing can declare.
///
/// The SC reports these already, so this is for the paths it never reaches:
/// a lone `.eventb` file and the open editor document. Wording, origin and
/// find-first behaviour come from the same helpers the SC uses, so a file
/// and its directory never describe the same finding differently.
///
/// Two positions are deliberately out:
///
/// - **witness predicates**, where Rodin *does* resolve a primed free
///   identifier (`MachineEventWitnessFreeIdentsModule` strips the prime,
///   and rossi's `sc::machine::events::witness_scope` mirrors it). Only a
///   scope that knows the abstract machine can judge them.
/// - **the variant**, where rossi does not report a free identifier at all:
///   `sc::machine::build_variant_decl` consumes it to mark the variant
///   unusable and emits nothing. Rodin *does* report it
///   (`machineVariantFreeIdentsModule`), so that is a gap of its own; until
///   it is closed, reporting here would say something `rossi build` does not.
#[must_use]
pub fn component_undeclarable_prime_diagnostics(component: &Component) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    match component {
        Component::Context(ctx) => {
            // THEOREMS lower into `axioms`, so both are covered here.
            for axiom in &ctx.axioms {
                push_predicate(&mut diags, axiom, "axm", "axiom predicate", &ctx.name);
            }
        }
        Component::Machine(machine) => {
            for invariant in &machine.invariants {
                push_predicate(
                    &mut diags,
                    invariant,
                    "inv",
                    "invariant predicate",
                    &machine.name,
                );
            }
            if let Some(init) = &machine.initialisation {
                let prefix = format!("{}.{}", machine.name, crate::sc::initialisation_label());
                for action in &init.actions {
                    push_action(&mut diags, action, &prefix);
                }
            }
            for event in &machine.events {
                let prefix = format!("{}.{}", machine.name, event.name);
                for guard in &event.guards {
                    push_predicate(&mut diags, guard, "grd", "guard predicate", &prefix);
                }
                for action in &event.actions {
                    push_action(&mut diags, action, &prefix);
                }
            }
        }
    }
    diags
}

/// Report the first free primed read of `raw`, if any, at
/// `{origin_prefix}.{label}` — the origin shape [`crate::duplicates`] uses —
/// falling back to `default_label` when the source wrote none, as the SC does.
fn push_predicate(
    diags: &mut Vec<Diagnostic>,
    raw: &LabeledPredicate,
    default_label: &str,
    place: &str,
    origin_prefix: &str,
) {
    let Some((bad, span)) = first_free_primed_in_predicate(&raw.predicate) else {
        return;
    };
    let label = raw.label.as_deref().unwrap_or(default_label);
    diags.push(undeclared_identifier(
        &bad,
        place,
        format!("{origin_prefix}.{label}"),
        span.or(raw.span),
    ));
}

/// The action counterpart, anchored on the primed read the way
/// `sc::machine::events` anchors it, so the two paths underline the same text.
fn push_action(diags: &mut Vec<Diagnostic>, raw: &LabeledAction, origin_prefix: &str) {
    let Some((bad, span)) = first_free_primed_in_action_rhs(&raw.action) else {
        return;
    };
    let label = raw.label.as_deref().unwrap_or("act");
    diags.push(undeclared_identifier(
        &bad,
        "action",
        format!("{origin_prefix}.{label}"),
        span.or(raw.span),
    ));
}
