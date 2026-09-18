//! `EB1xx` runtime-translation suitability checks over the typed
//! static-checker model.
//!
//! A runtime translation of an Event-B model — a code generator, an
//! animator, a trace checker — has to turn every construct into something
//! that runs. Two properties stop it from doing that faithfully:
//! *uncertainty*, where a construct admits more than one value and the model
//! does not say which, and *non-finiteness*, where a construct ranges over a
//! domain no terminating evaluation can cover. Neither is an error: both are
//! ordinary, proof-friendly Event-B. They are reported because translators
//! that meet them rarely refuse — they pick a value, truncate a domain, or
//! emit nothing for the construct, and the divergence from the model is
//! silent.
//!
//! The checks run over **leaf machines** (those no other machine refines)
//! and the contexts those see, because that is the machine a translation
//! consumes: the abstract levels are discharged by proof and either flattened
//! into the leaf or discarded. An abstract machine may freely use `:∈` —
//! `crates/rossi/examples/binary-search.zip` does — and this pass stays
//! silent about it.
//!
//! Messages say what could not be verified and name the missing evidence.
//! They never say a model is untranslatable: certainty is undecidable in
//! general, and each translation draws the line in a different place.

use std::collections::BTreeSet;

use rossi::ast::Span;
use rossi::formula::{
    Assignment, AssignmentKind, Expression, ExpressionKind, Predicate, PredicateKind,
    tag::{AssocPredOp, RelationalOp},
};

use crate::ast_util::lhs_variables;
use crate::sc_model::{ActionDecl, CheckedContext, CheckedMachine, EventDecl, ScModel};
use crate::{Diagnostic, Project, RuleId, Severity};
#[must_use]
pub fn run(project: &Project, model: &ScModel) -> Vec<Diagnostic> {
    let scope = Scope::build(project, model);
    let mut diags = Vec::new();

    for machine in &scope.machines {
        check_machine(machine, &mut diags);
    }

    diags
}

// ---------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------

/// The leaf machines of a project and the contexts they see.
///
/// Both lists follow `project.components` order rather than the checked
/// model's `HashMap` order, so a run's diagnostics come out in a stable
/// sequence.
struct Scope<'a> {
    machines: Vec<&'a CheckedMachine>,
    #[allow(dead_code)]
    contexts: Vec<&'a CheckedContext>,
}

impl<'a> Scope<'a> {
    fn build(project: &Project, model: &'a ScModel) -> Self {
        // A machine is abstract when some *checked* machine refines it. An
        // unchecked refinement leaves no record, so its parent stays a leaf
        // and is checked: the alternative is silently skipping the only
        // machine a broken project has.
        let refined: BTreeSet<&str> = model
            .machines
            .values()
            .filter_map(|m| m.record.refines.as_ref())
            .map(|r| r.parent_name.as_str())
            .collect();

        let mut machines = Vec::new();
        let mut contexts = Vec::new();
        let mut seen_context_names: BTreeSet<&str> = BTreeSet::new();
        let mut any_machine = false;

        for pc in &project.components {
            let rossi::Component::Machine(_) = &pc.component else {
                continue;
            };
            any_machine = true;
            let Some(machine) = model.machines.get(pc.component.name()) else {
                continue;
            };
            if refined.contains(machine.name()) {
                continue;
            }
            for context in model.seen_contexts(machine) {
                seen_context_names.insert(context.name());
            }
            machines.push(machine);
        }

        for pc in &project.components {
            let rossi::Component::Context(_) = &pc.component else {
                continue;
            };
            let Some(context) = model.contexts.get(pc.component.name()) else {
                continue;
            };
            // With no machine at all there is nothing to translate a context
            // *for*, but the context is the whole model, so check it.
            if any_machine && !seen_context_names.contains(context.name()) {
                continue;
            }
            contexts.push(context);
        }

        Self { machines, contexts }
    }
}

// ---------------------------------------------------------------------
// Event access
// ---------------------------------------------------------------------

/// Every typed action of `event`, oldest inherited first, paired with
/// whether it was declared by this event. `skip` has no typed form and is
/// left out.
///
/// `actions` carries the inherited extended-event prefix first; those
/// actions run in this machine but their text lives in an ancestor, so a
/// finding on one is reported here without a span, which would index the
/// wrong component's source.
fn event_actions(event: &EventDecl) -> impl Iterator<Item = (bool, &ActionDecl, &Assignment)> {
    let inherited = event.actions.len() - event.own_actions().len();
    event
        .actions
        .iter()
        .enumerate()
        .filter_map(move |(index, action)| {
            action
                .typed
                .as_ref()
                .map(|typed| (index >= inherited, action, typed))
        })
}

/// The span to report a formula of an event under: its own when the event
/// declares it, none when an ancestor does.
fn own_span(own: bool, span: Option<Span>) -> Option<Span> {
    if own { span } else { None }
}

// ---------------------------------------------------------------------
// Machine-level checks
// ---------------------------------------------------------------------

fn check_machine(machine: &CheckedMachine, diags: &mut Vec<Diagnostic>) {
    let variables: BTreeSet<&str> = machine
        .visible_variables
        .iter()
        .map(String::as_str)
        .collect();

    for event in &machine.record.events {
        let is_init = event.label == crate::sc::initialisation_label();
        for (own, action, typed) in event_actions(event) {
            let span = own_span(own, typed.span());
            let origin = || format!("{}.{}/{}", machine.name(), event.label, action.label);
            let targets = quoted_list(&lhs_variables(&action.action));

            if is_init {
                check_init_action(typed, &targets, &variables, origin, span, diags);
            } else {
                check_event_action(typed, &targets, origin, span, diags);
            }
        }
    }
}

/// EB100 and EB101 over one action of an ordinary event. `targets` is the
/// action's assigned identifiers, rendered for a message.
fn check_event_action(
    action: &Assignment,
    targets: &str,
    origin: impl Fn() -> String,
    span: Option<Span>,
    diags: &mut Vec<Diagnostic>,
) {
    match action.kind() {
        AssignmentKind::BecomesEqualTo { .. } => {}
        AssignmentKind::BecomesMemberOf { set, .. } => {
            if is_singleton(set) {
                return;
            }
            diags.push(Diagnostic {
                severity: RuleId::BecomesMemberOfInEvent.default_severity(),
                origin: origin(),
                message: format!(
                    "Could not verify the value {targets} takes: `:∈` chooses from a set that is not a singleton"
                ),
                rule_id: Some(RuleId::BecomesMemberOfInEvent),
                span,
            });
        }
        AssignmentKind::BecomesSuchThat { primed, pred, .. } => {
            diags.extend(becomes_such_that_diagnostic(
                targets,
                primed.len(),
                pred,
                &origin,
                span,
            ));
        }
    }
}

/// EB102 over one action of INITIALISATION.
fn check_init_action(
    action: &Assignment,
    targets: &str,
    variables: &BTreeSet<&str>,
    origin: impl Fn() -> String,
    span: Option<Span>,
    diags: &mut Vec<Diagnostic>,
) {
    // A read of machine state is a defect of its own: before initialisation
    // no variable has a value, so the action's result is whatever the
    // translation happens to leave in the slot.
    let read: BTreeSet<&str> = read_identifiers(action)
        .into_iter()
        .flat_map(|names| names.iter().map(String::as_str))
        .filter(|name| variables.contains(name))
        .collect();
    if !read.is_empty() {
        let read: Vec<&str> = read.into_iter().collect();
        diags.push(Diagnostic {
            severity: RuleId::NondeterministicInitialisation.default_severity(),
            origin: origin(),
            message: format!(
                "Could not verify the initial value: the action reads {}, which has no value before INITIALISATION",
                quoted_list(&read)
            ),
            rule_id: Some(RuleId::NondeterministicInitialisation),
            span,
        });
    }

    match action.kind() {
        AssignmentKind::BecomesEqualTo { .. } => {}
        AssignmentKind::BecomesMemberOf { set, .. } => {
            if is_singleton(set) {
                return;
            }
            diags.push(Diagnostic {
                severity: RuleId::NondeterministicInitialisation.default_severity(),
                origin: origin(),
                message: format!(
                    "Could not verify the initial value of {targets}: `:∈` chooses from a set that is not a singleton, so the initial state must be supplied outside the model"
                ),
                rule_id: Some(RuleId::NondeterministicInitialisation),
                span,
            });
        }
        AssignmentKind::BecomesSuchThat { primed, pred, .. } => {
            if canonical_branches(pred, primed.len()).is_some() {
                // A canonical `:|` in INITIALISATION reads as an ordinary
                // definition; EB101's branch caveat is an ordinary-event
                // concern and is not repeated here.
                return;
            }
            diags.push(Diagnostic {
                severity: RuleId::NondeterministicInitialisation.default_severity(),
                origin: origin(),
                message: format!(
                    "Could not verify the initial value of {targets}: the condition of `:∣` does not fix each variable with an equality, so the initial state must be supplied outside the model"
                ),
                rule_id: Some(RuleId::NondeterministicInitialisation),
                span,
            });
        }
    }
}

/// EB101's finding for one `:∣` action, at the severity its shape earns, or
/// `None` when the action is certain.
fn becomes_such_that_diagnostic(
    targets: &str,
    primed: usize,
    pred: &Predicate,
    origin: impl Fn() -> String,
    span: Option<Span>,
) -> Option<Diagnostic> {
    match canonical_branches(pred, primed) {
        // A single branch fixes every variable outright with nothing to
        // choose between: deterministic, and not reported.
        Some(1) => None,
        // The shape is readable, but `[acomp]` and `[acons]` — that the
        // branch conditions cover every case and overlap in none — are a
        // tautology and an unsatisfiability check. Both need a solver, so
        // the honest report is that the shape was recognised and the
        // conditions were not verified.
        Some(branches) => Some(Diagnostic {
            severity: Severity::Info,
            origin: origin(),
            message: format!(
                "`:∣` assigns {targets} in canonical form, but could not verify that its {branches} branch conditions are exhaustive and mutually exclusive"
            ),
            rule_id: Some(RuleId::NonCanonicalBecomesSuchThat),
            span,
        }),
        None => Some(Diagnostic {
            severity: RuleId::NonCanonicalBecomesSuchThat.default_severity(),
            origin: origin(),
            message: format!(
                "Could not verify the value {targets} takes: the condition of `:∣` does not fix each variable with an equality over its primed name"
            ),
            rule_id: Some(RuleId::NonCanonicalBecomesSuchThat),
            span,
        }),
    }
}

// ---------------------------------------------------------------------
// Canonical `:∣` form
// ---------------------------------------------------------------------

/// The number of branches of `pred` when it is a before-after predicate a
/// translation can read a value out of, `None` otherwise.
///
/// The form is a disjunction of branches, each of which fixes every one of
/// the `primed` after-state variables with exactly one equality
/// `x′ = E`, where `E` and every other conjunct of the branch read no
/// after-state at all. `primed` declarations are bound innermost-last, so
/// after-state variable `j` of `n` is de Bruijn index `n - 1 - j` at the
/// condition's own depth — but the condition is walked as a whole, so what
/// matters is only which indices below `n` dangle out of a sub-formula.
fn canonical_branches(pred: &Predicate, primed: usize) -> Option<usize> {
    if primed == 0 {
        return None;
    }
    let branches = disjuncts(pred);
    for branch in &branches {
        let mut fixed = vec![false; primed];
        for conjunct in conjuncts(branch) {
            match primed_equality(conjunct, primed) {
                Some(index) => {
                    // Two equalities for one variable are two values, which
                    // is exactly the uncertainty being checked for.
                    if std::mem::replace(&mut fixed[index as usize], true) {
                        return None;
                    }
                }
                // Any other conjunct is a branch condition and must read no
                // after-state: `x′ > 0` constrains without determining.
                None if reads_after_state(conjunct.dangling_bound_indices(), primed) => {
                    return None;
                }
                None => {}
            }
        }
        if !fixed.iter().all(|f| *f) {
            return None;
        }
    }
    Some(branches.len())
}

/// The after-state index this conjunct fixes, when it is an equality with a
/// bare after-state variable on one side and an after-state-free expression
/// on the other.
fn primed_equality(pred: &Predicate, primed: usize) -> Option<u32> {
    let PredicateKind::Relational {
        op: RelationalOp::Equal,
        left,
        right,
    } = pred.kind()
    else {
        return None;
    };
    for (target, value) in [(left, right), (right, left)] {
        let ExpressionKind::BoundIdentifier(index) = target.kind() else {
            continue;
        };
        if u64::from(*index) < primed as u64
            && !reads_after_state(value.dangling_bound_indices(), primed)
        {
            return Some(*index);
        }
    }
    None
}

/// Whether a sub-formula with these dangling indices reads one of the
/// `primed` after-state variables.
///
/// The after-state declarations are the outermost binders of the condition,
/// so an index below `primed` that dangles out of the sub-formula refers to
/// one of them; deeper indices belong to quantifiers inside it.
fn reads_after_state(dangling: &[u32], primed: usize) -> bool {
    dangling
        .iter()
        .any(|index| u64::from(*index) < primed as u64)
}

// ---------------------------------------------------------------------
// Shared shape helpers
// ---------------------------------------------------------------------

/// The top-level conjuncts of `pred`, flattening nested `∧`.
fn conjuncts(pred: &Predicate) -> Vec<&Predicate> {
    let mut out = Vec::new();
    push_associative(pred, AssocPredOp::LAnd, &mut out);
    out
}

/// The top-level disjuncts of `pred`, flattening nested `∨`.
fn disjuncts(pred: &Predicate) -> Vec<&Predicate> {
    let mut out = Vec::new();
    push_associative(pred, AssocPredOp::LOr, &mut out);
    out
}

fn push_associative<'a>(pred: &'a Predicate, op: AssocPredOp, out: &mut Vec<&'a Predicate>) {
    match pred.kind() {
        PredicateKind::Associative {
            op: found,
            children,
        } if *found == op => {
            for child in children {
                push_associative(child, op, out);
            }
        }
        _ => out.push(pred),
    }
}

/// The free identifiers an action reads, as one name list per read position.
///
/// Only the positions that are evaluated count: the assignment targets are
/// written, not read. A target occurring in a read position is a read like
/// any other, which is why the whole assignment's free identifiers are not
/// what this asks — `x ≔ x + 1` reads `x`.
fn read_identifiers(action: &Assignment) -> Vec<&[String]> {
    match action.kind() {
        AssignmentKind::BecomesEqualTo { values, .. } => {
            values.iter().map(Expression::free_identifiers).collect()
        }
        AssignmentKind::BecomesMemberOf { set, .. } => vec![set.free_identifiers()],
        AssignmentKind::BecomesSuchThat { pred, .. } => vec![pred.free_identifiers()],
    }
}

/// Whether `expr` is a one-element set extension. The empty set is
/// `Atomic(EmptySet)` and a set extension is never empty, so the length test
/// is the whole check.
fn is_singleton(expr: &Expression) -> bool {
    matches!(expr.kind(), ExpressionKind::SetExtension(members) if members.len() == 1)
}

fn quoted_list(names: &[&str]) -> String {
    names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}
