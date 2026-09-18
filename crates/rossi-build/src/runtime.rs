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

use std::collections::{BTreeMap, BTreeSet};

use rossi::ast::Span;
use rossi::formula::{
    Assignment, AssignmentKind, BoundIdentDecl, Expression, ExpressionKind, Predicate,
    PredicateKind, Type,
    tag::{
        AssocExprOp, AssocPredOp, AtomicOp, BinaryExprOp, BinaryPredOp, QuantPredOp, RelationalOp,
        UnaryExprOp,
    },
};

use crate::ast_util::lhs_variables;
use crate::sc_model::{ActionDecl, CheckedContext, CheckedMachine, EventDecl, GuardDecl, ScModel};
use crate::{Diagnostic, Project, RuleId, Severity};

/// Run every runtime-translation suitability check over the leaf machines of
/// `project` and the contexts they see.
///
/// The severities are mixed: a construct whose value a translation cannot
/// read is a `Warning`, one it can read but only under an assumption the
/// model leaves open is `Info`. Callers that hide INFO findings (the CLI does
/// unless `--show-info` is given) still see the certain half.
#[must_use]
pub fn run(project: &Project, model: &ScModel) -> Vec<Diagnostic> {
    let scope = Scope::build(project, model);
    let mut diags = Vec::new();

    for machine in &scope.machines {
        check_machine(machine, &mut diags);
        check_parameters(machine, &scope, &mut diags);
        check_parallel_actions(machine, &mut diags);
        check_guard_order(machine, &scope, &mut diags);
        check_dropped_variables(machine, &scope, &mut diags);
        check_witnesses(machine, &mut diags);
    }
    check_formulas(&scope, &mut diags);
    check_contexts(&scope, &mut diags);
    check_integers(&scope, &mut diags);

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
    model: &'a ScModel,
    machines: Vec<&'a CheckedMachine>,
    contexts: Vec<&'a CheckedContext>,
    /// Every axiom conjunct visible in the scope, in context then source
    /// order. Several rules read the same axioms; they are flattened once.
    axioms: Vec<&'a Predicate>,
    /// Carrier sets a visible axiom gives a finite cardinality. The type
    /// alone never says so: a given set may be infinite.
    finite_sets: BTreeSet<String>,
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

        let axioms: Vec<&Predicate> = contexts
            .iter()
            .flat_map(|context| context.record.axioms.iter())
            .flat_map(|axiom| conjuncts(&axiom.typed))
            .collect();
        let finite_sets = axioms
            .iter()
            .filter_map(|axiom| finitely_bounded_set(axiom))
            .map(str::to_string)
            .collect();

        Self {
            model,
            machines,
            contexts,
            axioms,
            finite_sets,
        }
    }

    /// Every invariant that constrains `machine`'s state: those it inherits
    /// from its refinement ancestors, oldest first, then its own.
    ///
    /// A refinement keeps an abstract variable's constraints without
    /// restating them, so reading only the machine's own invariants would
    /// report an inherited variable as unconstrained when its bound is one
    /// level up.
    fn invariants(&self, machine: &'a CheckedMachine) -> Vec<&'a Predicate> {
        self.model
            .inherited_invariants(machine)
            .into_iter()
            .chain(machine.record.invariants.iter())
            .flat_map(|invariant| conjuncts(&invariant.typed))
            .collect()
    }

    /// Whether every value of `ty` can be enumerated: the structural test
    /// plus the carrier sets a visible axiom bounds.
    fn is_finite(&self, ty: &Type) -> bool {
        ty.is_finite_with(&|name| self.finite_sets.contains(name))
    }
}

/// The carrier set this axiom conjunct gives a finite cardinality, if any.
///
/// Four spellings say it: `finite(S)`, a `partition` of `S` into listed
/// blocks, an enumeration `S = {a, b}`, and `card(S) = n`. Rodin has no
/// enumerated carrier-set declaration, so every one of them is an ordinary
/// axiom.
fn finitely_bounded_set(pred: &Predicate) -> Option<&str> {
    if let PredicateKind::Simple(expr) = pred.kind() {
        return carrier_set_name(expr);
    }
    // `card(S) = n` — `card` is well-defined only on finite sets, so the
    // axiom asserts finiteness by being well-defined.
    if let PredicateKind::Relational {
        op: RelationalOp::Equal,
        left,
        right,
    } = pred.kind()
        && let Some(name) = [left, right]
            .into_iter()
            .find_map(|side| card_operand(side).and_then(carrier_set_name))
    {
        return Some(name);
    }
    listing(pred)
        .filter(|listing| listing.finite)
        .and_then(|listing| carrier_set_name(listing.set))
}

/// What an axiom says about the members of a set, when it lists them.
///
/// Two spellings list members: `partition(S, {a}, {b})` and `S = {a, b}`.
/// They carry the same information about which constants are elements and
/// about the set being finite; which one a model uses is a matter of style,
/// and every rule that reads one reads the other through this. They differ
/// in one respect only, recorded in `disjoint`.
struct Listing<'a> {
    /// The set listed — a carrier set or a set-valued constant.
    set: &'a Expression,
    /// Constants the axiom names as single elements.
    named: Vec<&'a str>,
    /// Whether every block is a literal set extension, so that the whole set
    /// is finite. `partition(S, A, B)` splits `S` in two without bounding
    /// either half and says nothing about its size.
    finite: bool,
    /// Whether the named constants are known to differ. A partition's blocks
    /// are disjoint by definition; an enumeration's members may coincide.
    disjoint: bool,
}

fn listing(pred: &Predicate) -> Option<Listing<'_>> {
    match pred.kind() {
        PredicateKind::Multiple(exprs) => {
            let (set, blocks) = exprs.split_first()?;
            Some(Listing {
                set,
                named: blocks
                    .iter()
                    .filter_map(|block| match block.kind() {
                        ExpressionKind::SetExtension(members) if members.len() == 1 => {
                            free_identifier(&members[0])
                        }
                        _ => None,
                    })
                    .collect(),
                finite: blocks
                    .iter()
                    .all(|block| matches!(block.kind(), ExpressionKind::SetExtension(_))),
                disjoint: true,
            })
        }
        PredicateKind::Relational {
            op: RelationalOp::Equal,
            left,
            right,
        } => [(left, right), (right, left)]
            .into_iter()
            .find_map(|(set, members)| {
                let ExpressionKind::SetExtension(members) = members.kind() else {
                    return None;
                };
                Some(Listing {
                    set,
                    named: members.iter().filter_map(free_identifier).collect(),
                    finite: true,
                    disjoint: false,
                })
            }),
        _ => None,
    }
}

/// The name of the carrier set `expr` denotes, when it is one.
///
/// A carrier set `S` is the free identifier whose own type is `ℙ(S)`; a
/// constant of the same type is a subset of it and is not the set itself.
fn carrier_set_name(expr: &Expression) -> Option<&str> {
    let name = free_identifier(expr)?;
    match expr.ty() {
        Some(Type::Pow(base)) => match base.as_ref() {
            Type::Given(given) if given == name => Some(name),
            _ => None,
        },
        _ => None,
    }
}

fn free_identifier(expr: &Expression) -> Option<&str> {
    match expr.kind() {
        ExpressionKind::FreeIdentifier(name) => Some(name.as_str()),
        _ => None,
    }
}

/// The set `expr` counts, when it is a `card(…)`.
fn card_operand(expr: &Expression) -> Option<&Expression> {
    match expr.kind() {
        ExpressionKind::Unary {
            op: UnaryExprOp::KCard,
            child,
        } => Some(child),
        _ => None,
    }
}

// ---------------------------------------------------------------------
// Event access
// ---------------------------------------------------------------------

/// Every guard in scope for `event`, oldest inherited first, paired with
/// whether it was declared by this event (and so whether its span indexes
/// this component's text).
fn event_guards(event: &EventDecl) -> impl Iterator<Item = (bool, &GuardDecl)> {
    let guards = event.chain_guards();
    let inherited = guards.len() - event.guards.len();
    guards
        .into_iter()
        .enumerate()
        .map(move |(index, guard)| (index >= inherited, guard))
}

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

/// Whether `op` builds a relation or function space, which is infinite as
/// soon as either side is and is the shape a declaration gives a type with.
fn is_relation_space_op(op: BinaryExprOp) -> bool {
    matches!(
        op,
        BinaryExprOp::Rel
            | BinaryExprOp::TRel
            | BinaryExprOp::SRel
            | BinaryExprOp::STRel
            | BinaryExprOp::PFun
            | BinaryExprOp::TFun
            | BinaryExprOp::PInj
            | BinaryExprOp::TInj
            | BinaryExprOp::PSur
            | BinaryExprOp::TSur
            | BinaryExprOp::TBij
    )
}

fn quoted_list(names: &[&str]) -> String {
    names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------
// EB103 — event parameter not determined by the guards
// ---------------------------------------------------------------------

/// Report every parameter of a leaf machine's events that the guards do not
/// pin to one value.
///
/// This is INFO rather than a warning because it is not always a defect: in
/// trace-driven checking the parameter comes from the trace, and its absence
/// from the model is correct. The rule reports what the consumer will have to
/// supply.
fn check_parameters(machine: &CheckedMachine, scope: &Scope, diags: &mut Vec<Diagnostic>) {
    for event in &machine.record.events {
        if event.label == crate::sc::initialisation_label() {
            continue; // INITIALISATION takes no parameters.
        }
        let guards: Vec<&Predicate> = event_guards(event)
            .flat_map(|(_, guard)| conjuncts(&guard.typed))
            .collect();

        for parameter in event.chain_parameters() {
            if guards
                .iter()
                .any(|conjunct| determines(conjunct, &parameter.name))
            {
                continue;
            }
            // A parameter confined to a finite domain is still a choice,
            // but a translation can at least enumerate it; that is a
            // materially different position to be in, so the message says
            // which. The type settles it for a bounded carrier set or
            // `BOOL`; otherwise a guard may still confine it.
            let enumerable = scope.is_finite(&parameter.ty)
                || guards.iter().any(|conjunct| {
                    membership_domain(conjunct, &parameter.name)
                        .is_some_and(|set| is_enumerable_set(set, scope))
                });
            let message = if enumerable {
                format!(
                    "Could not verify the value of parameter `{}`: no guard of the form `{} = …`, though its domain is finite and can be enumerated",
                    parameter.name, parameter.name
                )
            } else {
                format!(
                    "Could not verify the value of parameter `{}`: no guard of the form `{} = …`, so it must be supplied outside the model",
                    parameter.name, parameter.name
                )
            };
            diags.push(Diagnostic {
                severity: RuleId::UndeterminedParameter.default_severity(),
                origin: format!("{}.{}/{}", machine.name(), event.label, parameter.name),
                message,
                rule_id: Some(RuleId::UndeterminedParameter),
                // The parameter's own declaration span is not carried on the
                // checked record, and the guards that should have pinned it
                // are exactly what is missing, so there is nothing to anchor.
                span: None,
            });
        }
    }
}

/// Whether this conjunct fixes `name` to one value: an equality with `name`
/// bare on one side and an expression that does not read it on the other.
fn determines(pred: &Predicate, name: &str) -> bool {
    let PredicateKind::Relational {
        op: RelationalOp::Equal,
        left,
        right,
    } = pred.kind()
    else {
        return false;
    };
    [(left, right), (right, left)]
        .iter()
        .any(|(target, value)| {
            free_identifier(target) == Some(name)
                && !value.free_identifiers().iter().any(|free| free == name)
        })
}

/// Whether a translation could enumerate this set.
///
/// An enumerated set and an integer interval are finite however infinite
/// their element type is, which is exactly how a model bounds an integer;
/// anything else is judged by its element type.
fn is_enumerable_set(set: &Expression, scope: &Scope) -> bool {
    match set.kind() {
        ExpressionKind::SetExtension(_) => true,
        ExpressionKind::Binary {
            op: BinaryExprOp::UpTo,
            ..
        } => true,
        _ => set
            .ty()
            .and_then(Type::base_type)
            .is_some_and(|element| scope.is_finite(element)),
    }
}

/// The set `name` is required to belong to by this conjunct, if any.
fn membership_domain<'a>(pred: &'a Predicate, name: &str) -> Option<&'a Expression> {
    let PredicateKind::Relational {
        op: RelationalOp::In,
        left,
        right,
    } = pred.kind()
    else {
        return None;
    };
    (free_identifier(left) == Some(name)).then_some(right)
}

// ---------------------------------------------------------------------
// Formula walk — EB104, EB105, EB106
// ---------------------------------------------------------------------

/// One type-checked formula of the scope, as the walk over it wants to see
/// it: a predicate, or an expression that stands in value position.
///
/// Every expression a component declares is evaluated — a variant, an
/// assigned value, the set of a `:∈` — so there is no third case.
enum Formula<'a> {
    Pred(&'a Predicate),
    Value(&'a Expression),
}

/// Walk every formula the scope's leaf machines and visible contexts
/// declare, reporting the rules that read whole formulas.
///
/// EB106 is collected rather than emitted inline: a carrier set used in ten
/// places is one under-specified declaration, so it is reported once, at the
/// declaration, naming the first use.
fn check_formulas(scope: &Scope, diags: &mut Vec<Diagnostic>) {
    let mut unbounded_sets: BTreeMap<String, String> = BTreeMap::new();

    for_each_formula(scope, &mut |origin, span, formula| match formula {
        Formula::Pred(pred) => {
            check_quantifiers_in_predicate(pred, scope, origin, span, diags);
            check_values_in_predicate(pred, scope, origin, span, diags, &mut unbounded_sets);
        }
        Formula::Value(expr) => {
            check_quantifiers_in_expression(expr, scope, origin, span, diags);
            check_values_in_expression(expr, scope, origin, span, diags, &mut unbounded_sets, true);
        }
    });

    emit_unbounded_sets(scope, &unbounded_sets, diags);
}

/// EB106, one finding per under-specified carrier set, at its declaration.
fn emit_unbounded_sets(
    scope: &Scope,
    unbounded_sets: &BTreeMap<String, String>,
    diags: &mut Vec<Diagnostic>,
) {
    for (set, first_use) in unbounded_sets {
        let Some(context) = scope
            .contexts
            .iter()
            .find(|context| context.record.carrier_sets.iter().any(|s| &s.name == set))
        else {
            continue;
        };
        diags.push(Diagnostic {
            severity: RuleId::DeferredSetWithoutCardinality.default_severity(),
            origin: format!("{}.{set}", context.name()),
            message: format!(
                "Could not verify the size of carrier set `{set}`, used as a value by `{first_use}`: no `finite({set})`, `partition({set}, …)`, `{set} = {{…}}` or `card({set}) = …` axiom is visible"
            ),
            rule_id: Some(RuleId::DeferredSetWithoutCardinality),
            // The declaration's span lives on the untyped AST; the checked
            // record keeps only its Rodin handle, so the finding is
            // anchored by origin alone.
            span: None,
        });
    }
}

/// How a formula is reported: the origin it is filed under, built only when
/// a rule has something to say, and the span of its own text.
type Origin<'a> = &'a dyn Fn() -> String;

/// What [`for_each_formula`] hands each formula to.
type FormulaVisitor<'a> = &'a mut dyn FnMut(Origin<'_>, Option<Span>, Formula);

/// Call `visit` for every type-checked formula of the scope, with the origin
/// it is reported under and the span of its own text.
///
/// Origins are built lazily: most formulas produce no finding, and the
/// closure is only called by a rule that has one.
fn for_each_formula(scope: &Scope, visit: FormulaVisitor<'_>) {
    for context in &scope.contexts {
        let name = context.name();
        for axiom in &context.record.axioms {
            visit(
                &|| format!("{name}.{}", axiom.label),
                axiom.typed.span(),
                Formula::Pred(&axiom.typed),
            );
        }
    }

    for machine in &scope.machines {
        let name = machine.name();
        for invariant in &machine.record.invariants {
            visit(
                &|| format!("{name}.{}", invariant.label),
                invariant.typed.span(),
                Formula::Pred(&invariant.typed),
            );
        }
        for variant in &machine.record.variants {
            if let Some(typed) = &variant.typed {
                visit(
                    &|| format!("{name}.{}", variant.label),
                    typed.span(),
                    Formula::Value(typed),
                );
            }
        }
        for event in &machine.record.events {
            for (own, guard) in event_guards(event) {
                visit(
                    &|| format!("{name}.{}/{}", event.label, guard.label),
                    own_span(own, guard.typed.span()),
                    Formula::Pred(&guard.typed),
                );
            }
            for (own, action, typed) in event_actions(event) {
                let origin = || format!("{name}.{}/{}", event.label, action.label);
                let span = own_span(own, typed.span());
                // An assignment's right-hand side is a value position by
                // definition: the value is what gets stored.
                match typed.kind() {
                    AssignmentKind::BecomesEqualTo { values, .. } => {
                        for value in values {
                            visit(&origin, span, Formula::Value(value));
                        }
                    }
                    AssignmentKind::BecomesMemberOf { set, .. } => {
                        visit(&origin, span, Formula::Value(set));
                    }
                    AssignmentKind::BecomesSuchThat { pred, .. } => {
                        visit(&origin, span, Formula::Pred(pred));
                    }
                }
            }
            for witness in &event.witnesses {
                visit(
                    &|| format!("{name}.{}/{}", event.label, witness.label),
                    witness.typed.span(),
                    Formula::Pred(&witness.typed),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------
// EB104 — quantifier or comprehension without a finite domain
// ---------------------------------------------------------------------

fn check_quantifiers_in_predicate(
    pred: &Predicate,
    scope: &Scope,
    origin: Origin<'_>,
    span: Option<Span>,
    diags: &mut Vec<Diagnostic>,
) {
    if let PredicateKind::Quantified {
        op,
        decls,
        pred: body,
    } = pred.kind()
    {
        // `∀x · P ⇒ Q` restricts `x` in the antecedent; `∃x · P` restricts
        // it in the body itself. Rodin imposes neither shape, so both are
        // read the same way: the conjuncts that must hold for the bound
        // variable to matter.
        let generators = match (op, body.kind()) {
            (
                QuantPredOp::Forall,
                PredicateKind::Binary {
                    op: BinaryPredOp::LImp,
                    left,
                    ..
                },
            ) => conjuncts(left),
            _ => conjuncts(body),
        };
        report_unbounded_decls(decls, &generators, scope, origin, span, diags);
    }

    let (preds, exprs) = predicate_children(pred);
    for child in preds {
        check_quantifiers_in_predicate(child, scope, origin, span, diags);
    }
    for child in exprs {
        check_quantifiers_in_expression(child, scope, origin, span, diags);
    }
}

fn check_quantifiers_in_expression(
    expr: &Expression,
    scope: &Scope,
    origin: Origin<'_>,
    span: Option<Span>,
    diags: &mut Vec<Diagnostic>,
) {
    if let ExpressionKind::Quantified { decls, pred, .. } = expr.kind() {
        // A comprehension, a lambda and `⋃`/`⋂` all restrict their bound
        // variables in the same predicate slot. The print form differs; the
        // meaning does not.
        report_unbounded_decls(decls, &conjuncts(pred), scope, origin, span, diags);
    }

    let (preds, exprs) = expression_children(expr);
    for child in preds {
        check_quantifiers_in_predicate(child, scope, origin, span, diags);
    }
    for child in exprs {
        check_quantifiers_in_expression(child, scope, origin, span, diags);
    }
}

/// Report each declaration of one quantifier that has neither a finite type
/// nor a generator among `generators`.
fn report_unbounded_decls(
    decls: &[BoundIdentDecl],
    generators: &[&Predicate],
    scope: &Scope,
    origin: Origin<'_>,
    span: Option<Span>,
    diags: &mut Vec<Diagnostic>,
) {
    for (position, decl) in decls.iter().enumerate() {
        // A finite type is its own domain: `BOOL` needs no generator, and
        // neither does a carrier set an axiom has bounded.
        if decl.ty().is_some_and(|ty| scope.is_finite(ty)) {
            continue;
        }
        // Declarations bind innermost-last, so the declaration at position
        // `i` of `n` is de Bruijn index `n - 1 - i` inside the body.
        let index = (decls.len() - 1 - position) as u32;
        if generators
            .iter()
            .any(|generator| restricts(generator, index))
        {
            continue;
        }
        diags.push(Diagnostic {
            severity: RuleId::UnboundedQuantifier.default_severity(),
            origin: origin(),
            message: format!(
                "Could not verify a finite domain for bound variable `{}`: no membership, equality or maplet membership restricts it, and its type is not finite",
                decl.name()
            ),
            rule_id: Some(RuleId::UnboundedQuantifier),
            span,
        });
    }
}

/// Whether this conjunct gives the bound variable at `index` a domain to be
/// iterated over.
///
/// Four shapes do: a membership `x ∈ S`, a subset constraint `x ⊆ S`, which
/// is the same statement as `x ∈ ℙ(S)` and would be perverse to read
/// differently, an equality `x = E` (one value is a domain of one), and a
/// maplet membership `x ↦ y ∈ r`, which is how a relation's domain is
/// written.
fn restricts(pred: &Predicate, index: u32) -> bool {
    let PredicateKind::Relational { op, left, right } = pred.kind() else {
        return false;
    };
    match op {
        RelationalOp::In | RelationalOp::Subset | RelationalOp::SubsetEq => {
            mentions_bound_at_top(left, index)
        }
        RelationalOp::Equal => {
            (is_bound(left, index) && !reads_bound(right, index))
                || (is_bound(right, index) && !reads_bound(left, index))
        }
        _ => false,
    }
}

/// Whether `expr` is the bound variable at `index`, possibly inside a maplet
/// tree — `x`, `x ↦ y`, `(x ↦ y) ↦ z`. A deeper position does not count:
/// `f(x) ∈ S` says nothing about which `x` to try.
fn mentions_bound_at_top(expr: &Expression, index: u32) -> bool {
    match expr.kind() {
        ExpressionKind::Binary {
            op: BinaryExprOp::Mapsto,
            left,
            right,
        } => mentions_bound_at_top(left, index) || mentions_bound_at_top(right, index),
        _ => is_bound(expr, index),
    }
}

fn is_bound(expr: &Expression, index: u32) -> bool {
    matches!(expr.kind(), ExpressionKind::BoundIdentifier(found) if *found == index)
}

fn reads_bound(expr: &Expression, index: u32) -> bool {
    expr.dangling_bound_indices().contains(&index)
}

// ---------------------------------------------------------------------
// EB105 / EB106 — infinite sets in value position
// ---------------------------------------------------------------------

/// Why a set cannot be enumerated.
enum Unbounded {
    /// An infinite set built into the language: `ℤ`, a power set, a
    /// function space. The string names it for the message.
    Structural(&'static str),
    /// A carrier set no visible axiom bounds.
    CarrierSet(String),
}

fn check_values_in_predicate(
    pred: &Predicate,
    scope: &Scope,
    origin: Origin<'_>,
    span: Option<Span>,
    diags: &mut Vec<Diagnostic>,
    sets: &mut BTreeMap<String, String>,
) {
    let (preds, exprs) = predicate_children(pred);
    for child in preds {
        check_values_in_predicate(child, scope, origin, span, diags, sets);
    }
    // A predicate never puts its operands in value position by itself:
    // `x ∈ ℕ` is a typing statement, not a request to build `ℕ`. Only the
    // expression operators below do, so each child restarts the walk.
    for child in exprs {
        check_values_in_expression(child, scope, origin, span, diags, sets, false);
    }
}

/// Walk `expr`, testing every operand that must be built rather than merely
/// described.
///
/// The distinction is the whole of these two rules: `f ∈ ℕ → ℤ` gives `f` a
/// type and is fine, `card(ℕ)` asks for a computation and is not. So a node
/// is never tested for what it *is*, only for the position its parent puts
/// it in — `in_value` carries that down.
///
/// A node found unbounded is reported and not descended into: its operands
/// are why it is unbounded, and naming them too would report one defect
/// several times.
fn check_values_in_expression(
    expr: &Expression,
    scope: &Scope,
    origin: Origin<'_>,
    span: Option<Span>,
    diags: &mut Vec<Diagnostic>,
    sets: &mut BTreeMap<String, String>,
    in_value: bool,
) {
    if in_value {
        match unbounded_reason(expr, scope) {
            Some(Unbounded::Structural(what)) => {
                diags.push(Diagnostic {
                    severity: RuleId::InfiniteSetValue.default_severity(),
                    origin: origin(),
                    message: format!(
                        "Could not enumerate {what} here: an infinite set is used as a value, not to give a type"
                    ),
                    rule_id: Some(RuleId::InfiniteSetValue),
                    span,
                });
                return;
            }
            // Recorded, not emitted: the defect is the declaration, and one
            // finding there beats one per use.
            Some(Unbounded::CarrierSet(set)) => {
                sets.entry(set).or_insert_with(origin);
                return;
            }
            None => {}
        }
    }

    let (preds, exprs) = expression_children(expr);
    for child in preds {
        check_values_in_predicate(child, scope, origin, span, diags, sets);
    }
    // Only a handful of operators put their operands in value position; any
    // other node's children are merely described.
    let children = value_children(expr)
        .unwrap_or_else(|| exprs.into_iter().map(|child| (child, false)).collect());
    for (child, child_in_value) in children {
        check_values_in_expression(child, scope, origin, span, diags, sets, child_in_value);
    }
}

/// Each expression child of `expr`, paired with whether `expr` puts it in
/// value position — where the set has to be built rather than described.
/// `None` for an operator that puts none of them there.
fn value_children(expr: &Expression) -> Option<Vec<(&Expression, bool)>> {
    let children = match expr.kind() {
        // Counting, bounding and generalised union all consume the set.
        ExpressionKind::Unary {
            op:
                UnaryExprOp::KCard
                | UnaryExprOp::KMin
                | UnaryExprOp::KMax
                | UnaryExprOp::KUnion
                | UnaryExprOp::KInter,
            child,
        } => vec![(child, true)],
        // Listing a set builds every member.
        ExpressionKind::SetExtension(members) => {
            members.iter().map(|member| (member, true)).collect()
        }
        ExpressionKind::Associative {
            op: AssocExprOp::BUnion,
            children,
        } => children.iter().map(|child| (child, true)).collect(),
        // An intersection is walked from whichever operand can be
        // enumerated, testing each element against the others, so no single
        // operand has to be buildable. The node itself is still tested, and
        // is out of reach only when none of them is.
        ExpressionKind::Associative {
            op: AssocExprOp::BInter,
            children,
        } => children.iter().map(|child| (child, false)).collect(),
        // `A ∖ B` walks `A` and tests each element against `B`, so only the
        // left operand has to be enumerable.
        ExpressionKind::Binary {
            op: BinaryExprOp::SetMinus,
            left,
            right,
        } => vec![(left, true), (right, false)],
        _ => return None,
    };
    Some(children)
}

/// Why `expr` cannot be enumerated, if it cannot.
///
/// Only the set-building spine is followed. An operand that computes with a
/// set (`dom(r)`, `r[s]`) is left alone: its result may well be finite even
/// when an operand is not, and claiming otherwise would report `dom(f)` for
/// every total function on `ℤ`.
fn unbounded_reason(expr: &Expression, scope: &Scope) -> Option<Unbounded> {
    match expr.kind() {
        ExpressionKind::Atomic(AtomicOp::Integer) => Some(Unbounded::Structural("`ℤ`")),
        ExpressionKind::Atomic(AtomicOp::Natural) => Some(Unbounded::Structural("`ℕ`")),
        ExpressionKind::Atomic(AtomicOp::Natural1) => Some(Unbounded::Structural("`ℕ1`")),
        // `ℙ(S)` is infinite exactly when `S` is, and so is `ℙ1(S)`.
        ExpressionKind::Unary {
            op: UnaryExprOp::Pow | UnaryExprOp::Pow1,
            child,
        } => unbounded_reason(child, scope).map(|_| Unbounded::Structural("a power set")),
        // Every relation and function space, and the product they are built
        // over, is infinite as soon as either side is.
        ExpressionKind::Binary { op, left, right }
            if is_relation_space_op(*op) || *op == BinaryExprOp::CProd =>
        {
            unbounded_reason(left, scope)
                .or_else(|| unbounded_reason(right, scope))
                .map(|_| Unbounded::Structural("a relation or function space"))
        }
        ExpressionKind::Binary {
            op: BinaryExprOp::SetMinus,
            left,
            ..
        } => unbounded_reason(left, scope),
        ExpressionKind::Associative {
            op: AssocExprOp::BUnion,
            children,
        } => children
            .iter()
            .find_map(|child| unbounded_reason(child, scope)),
        // An intersection is no larger than its smallest operand, so one
        // enumerable operand is enough to build it.
        ExpressionKind::Associative {
            op: AssocExprOp::BInter,
            children,
        } => {
            let mut reasons = children.iter().map(|child| unbounded_reason(child, scope));
            let first = reasons.next()??;
            reasons.all(|reason| reason.is_some()).then_some(first)
        }
        ExpressionKind::FreeIdentifier(_) => {
            let name = carrier_set_name(expr)?;
            (!scope.finite_sets.contains(name)).then(|| Unbounded::CarrierSet(name.to_string()))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------
// Generic child access
// ---------------------------------------------------------------------

/// The immediate sub-formulas of a predicate.
///
/// Bound declarations' type annotations are deliberately skipped: an
/// annotation spells a type, never a value, and reporting one would
/// contradict the typing exemption these rules rest on.
fn predicate_children(pred: &Predicate) -> (Vec<&Predicate>, Vec<&Expression>) {
    match pred.kind() {
        PredicateKind::Literal(_) | PredicateKind::PredicateVariable(_) => (Vec::new(), Vec::new()),
        PredicateKind::Relational { left, right, .. } => (Vec::new(), vec![left, right]),
        PredicateKind::Binary { left, right, .. } => (vec![left, right], Vec::new()),
        PredicateKind::Associative { children, .. } => (children.iter().collect(), Vec::new()),
        PredicateKind::Not(child) => (vec![child], Vec::new()),
        PredicateKind::Quantified { pred, .. } => (vec![pred], Vec::new()),
        PredicateKind::Simple(expr) => (Vec::new(), vec![expr]),
        PredicateKind::Multiple(exprs) => (Vec::new(), exprs.iter().collect()),
        PredicateKind::Application { args, .. } => (Vec::new(), args.iter().collect()),
        PredicateKind::Extended { exprs, preds, .. } => {
            (preds.iter().collect(), exprs.iter().collect())
        }
    }
}

/// The immediate sub-formulas of an expression.
fn expression_children(expr: &Expression) -> (Vec<&Predicate>, Vec<&Expression>) {
    match expr.kind() {
        ExpressionKind::FreeIdentifier(_)
        | ExpressionKind::BoundIdentifier(_)
        | ExpressionKind::IntegerLiteral(_)
        | ExpressionKind::Atomic(_) => (Vec::new(), Vec::new()),
        ExpressionKind::SetExtension(members) => (Vec::new(), members.iter().collect()),
        ExpressionKind::Bool(pred) => (vec![pred], Vec::new()),
        ExpressionKind::Binary { left, right, .. } => (Vec::new(), vec![left, right]),
        ExpressionKind::Associative { children, .. } => (Vec::new(), children.iter().collect()),
        ExpressionKind::Unary { child, .. } => (Vec::new(), vec![child]),
        ExpressionKind::Quantified { pred, expr, .. } => (vec![pred], vec![expr]),
        // The ascribed type is a type expression by construction.
        ExpressionKind::Ascription { expr, .. } => (Vec::new(), vec![expr]),
        ExpressionKind::Extended { exprs, preds, .. } => {
            (preds.iter().collect(), exprs.iter().collect())
        }
    }
}

// ---------------------------------------------------------------------
// EB107, EB108, EB109 — what the contexts leave open
// ---------------------------------------------------------------------

/// EB108 and EB109 over the contexts the leaf machines see.
fn check_contexts(scope: &Scope, diags: &mut Vec<Diagnostic>) {
    check_constant_definitions(scope, diags);
    check_carrier_set_constants(scope, diags);
}

/// EB108 — every constant a visible axiom does not pin to a value.
///
/// A definition may name other constants, so this is a reachability question
/// over the definition graph rather than a per-axiom test: `SIZE = WIDTH *
/// HEIGHT` determines `SIZE` exactly when `WIDTH` and `HEIGHT` are
/// themselves determined. A cycle never reaches a fixed point and is
/// reported like any other unmet dependency, naming what the definition
/// waits on.
fn check_constant_definitions(scope: &Scope, diags: &mut Vec<Diagnostic>) {
    let constants: BTreeSet<&str> = scope
        .contexts
        .iter()
        .flat_map(|context| context.record.constants.iter())
        .map(|constant| constant.name.as_str())
        .collect();
    if constants.is_empty() {
        return;
    }
    fn constant_name<'e>(expr: &'e Expression, constants: &BTreeSet<&str>) -> Option<&'e str> {
        free_identifier(expr).filter(|name| constants.contains(name))
    }

    // Alternative definitions per constant, each carrying what it depends
    // on. Several axioms may define one constant; any one of them suffices.
    let mut definitions: BTreeMap<&str, Vec<BTreeSet<&str>>> = BTreeMap::new();
    let mut finite_constants: BTreeSet<&str> = BTreeSet::new();
    let mut sized_constants: BTreeSet<&str> = BTreeSet::new();

    for axiom in &scope.axioms {
        collect_definitions(axiom, &constants, &mut definitions);
        // `finite(c)` and `card(c) = n` together bound a set-valued constant
        // without naming its members, which is the shape the cardinality
        // restriction asks for.
        match axiom.kind() {
            PredicateKind::Simple(expr) => {
                finite_constants.extend(constant_name(expr, &constants));
            }
            PredicateKind::Relational {
                op: RelationalOp::Equal,
                left,
                right,
            } => {
                for side in [left, right] {
                    sized_constants
                        .extend(card_operand(side).and_then(|set| constant_name(set, &constants)));
                }
            }
            _ => {}
        }
    }
    for name in finite_constants.intersection(&sized_constants) {
        definitions.entry(name).or_default().push(BTreeSet::new());
    }

    // Least fixed point: start with nothing determined and keep admitting
    // constants all of whose dependencies are already determined.
    let mut determined: BTreeSet<&str> = BTreeSet::new();
    loop {
        let mut grew = false;
        for (name, alternatives) in &definitions {
            if determined.contains(name) {
                continue;
            }
            let settled = alternatives.iter().any(|deps| {
                deps.iter()
                    // A name that is not a constant is a carrier set or a
                    // literal: nothing to determine.
                    .all(|dep| !constants.contains(dep) || determined.contains(dep))
            });
            if settled {
                determined.insert(name);
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }

    for context in &scope.contexts {
        for constant in &context.record.constants {
            let name = constant.name.as_str();
            if determined.contains(name) {
                continue;
            }
            let waiting: Vec<&str> = definitions
                .get(name)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|dep| constants.contains(*dep) && !determined.contains(*dep))
                .copied()
                .collect();
            let message = if waiting.is_empty() {
                format!(
                    "Could not verify the value of constant `{name}`: no axiom of the form `{name} = …`, and no `partition` block or `finite` plus `card` pair naming it"
                )
            } else {
                format!(
                    "Could not verify the value of constant `{name}`: its defining axiom waits on {}, which {} not determined either",
                    quoted_list(&waiting),
                    if waiting.len() == 1 { "is" } else { "are" }
                )
            };
            diags.push(Diagnostic {
                severity: RuleId::UndeterminedConstant.default_severity(),
                origin: format!("{}.{name}", context.name()),
                message,
                rule_id: Some(RuleId::UndeterminedConstant),
                span: None,
            });
        }
    }
}

/// Record what each constant this conjunct defines depends on.
///
/// A listing fixes each constant it names outright: a partition's blocks
/// are disjoint, so its singleton constants are determined up to naming
/// whatever set is split, and an enumeration of a carrier set names the
/// whole universe. An enumeration of a set-valued constant only says who its
/// members are and defines the constant itself, through the equality. An
/// equality defines the constant on one side in terms of the other.
fn collect_definitions<'a>(
    pred: &'a Predicate,
    constants: &BTreeSet<&str>,
    out: &mut BTreeMap<&'a str, Vec<BTreeSet<&'a str>>>,
) {
    if let Some(listing) = listing(pred)
        && (listing.disjoint || carrier_set_name(listing.set).is_some())
    {
        for name in listing.named {
            if constants.contains(name) {
                out.entry(name).or_default().push(BTreeSet::new());
            }
        }
    }
    if let PredicateKind::Relational {
        op: RelationalOp::Equal,
        left,
        right,
    } = pred.kind()
    {
        define_from_equality(left, right, constants, out);
    }
}

/// Record the definitions an equality between `target` and `value` yields.
///
/// A maplet equality is a pair of equalities written as one, so
/// `a ↦ b = 2 ↦ TRUE` defines `a` and `b` separately. The two sides are
/// walked together and each leaf pair is read in both orientations, since
/// which side names the constant is a matter of spelling.
fn define_from_equality<'a>(
    target: &'a Expression,
    value: &'a Expression,
    constants: &BTreeSet<&str>,
    out: &mut BTreeMap<&'a str, Vec<BTreeSet<&'a str>>>,
) {
    if let (
        ExpressionKind::Binary {
            op: BinaryExprOp::Mapsto,
            left: target_left,
            right: target_right,
        },
        ExpressionKind::Binary {
            op: BinaryExprOp::Mapsto,
            left: value_left,
            right: value_right,
        },
    ) = (target.kind(), value.kind())
    {
        define_from_equality(target_left, value_left, constants, out);
        define_from_equality(target_right, value_right, constants, out);
        return;
    }

    for (defined, definition) in [(target, value), (value, target)] {
        let Some(name) = free_identifier(defined) else {
            continue;
        };
        if !constants.contains(name) {
            continue;
        }
        // A self-reference is not a definition: `c = c + 1` determines
        // nothing, and admitting it would let a cycle look settled.
        if definition
            .free_identifiers()
            .iter()
            .any(|free| free == name)
        {
            continue;
        }
        let deps = definition
            .free_identifiers()
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<&str>>();
        out.entry(name).or_default().push(deps);
    }
}

/// EB109 — pairs of same-carrier-set constants no axiom tells apart.
fn check_carrier_set_constants(scope: &Scope, diags: &mut Vec<Diagnostic>) {
    // Group by carrier set, keeping the declaring context so each finding
    // can be filed where the constants are written.
    let mut groups: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
    for context in &scope.contexts {
        for constant in &context.record.constants {
            if let Type::Given(set) = &constant.ty {
                groups
                    .entry(set.as_str())
                    .or_default()
                    .push((context.name(), constant.name.as_str()));
            }
        }
    }
    if groups.values().all(|members| members.len() < 2) {
        return;
    }

    // One pass over the axioms answers every pair: which constants a
    // `partition` of each set gives a block of their own — a `partition`
    // relates every pair at once, which is why models with four constants
    // write one predicate instead of six inequalities — and which pairs an
    // axiom relates by `=` or `≠` outright.
    let mut partitioned: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut related: BTreeSet<(&str, &str)> = BTreeSet::new();
    for axiom in &scope.axioms {
        if let Some(listing) = listing(axiom)
            && listing.disjoint
            && let Some(set) = carrier_set_name(listing.set)
        {
            partitioned.entry(set).or_default().extend(listing.named);
        }
        if let Some((first, second)) = related_pair(axiom) {
            related.insert((first.min(second), first.max(second)));
        }
    }

    for (set, members) in groups {
        let partitioned = partitioned.get(set);
        for (index, (context, first)) in members.iter().enumerate() {
            for (_, second) in &members[index + 1..] {
                if partitioned.is_some_and(|names| names.contains(first) && names.contains(second))
                {
                    continue;
                }
                if related.contains(&(*first.min(second), *first.max(second))) {
                    continue;
                }
                diags.push(Diagnostic {
                    severity: RuleId::IndistinctCarrierSetConstants.default_severity(),
                    origin: format!("{context}.{first}"),
                    message: format!(
                        "Could not verify whether `{first}` and `{second}` denote different elements of `{set}`: no axiom relates them by `=` or `≠`, and no `partition` of `{set}` names both"
                    ),
                    rule_id: Some(RuleId::IndistinctCarrierSetConstants),
                    span: None,
                });
            }
        }
    }
}

/// The two identifiers this conjunct states to be equal or different.
fn related_pair(pred: &Predicate) -> Option<(&str, &str)> {
    let PredicateKind::Relational {
        op: RelationalOp::Equal | RelationalOp::NotEqual,
        left,
        right,
    } = pred.kind()
    else {
        return None;
    };
    Some((free_identifier(left)?, free_identifier(right)?))
}

// ---------------------------------------------------------------------
// EB107 — unbounded integer
// ---------------------------------------------------------------------

/// Report every integer identifier whose only constraints give it its type.
///
/// Reported at INFO: an unbounded integer is usually harmless, it becomes a
/// machine integer and only matters at the edges. The edges are where the
/// surveyed translations differ from each other, which is why it is reported
/// at all.
fn check_integers(scope: &Scope, diags: &mut Vec<Diagnostic>) {
    for context in &scope.contexts {
        for constant in &context.record.constants {
            if constant.ty != Type::Int {
                continue;
            }
            if bounded(&constant.name, &scope.axioms) {
                continue;
            }
            diags.push(integer_diagnostic(
                format!("{}.{}", context.name(), constant.name),
                &constant.name,
                "constant",
            ));
        }
    }

    for machine in &scope.machines {
        let invariants = scope.invariants(machine);
        for variable in &machine.record.variables {
            if variable.ty != Type::Int {
                continue;
            }
            if bounded(&variable.name, &invariants) {
                continue;
            }
            diags.push(integer_diagnostic(
                format!("{}.{}", machine.name(), variable.name),
                &variable.name,
                "variable",
            ));
        }

        for event in &machine.record.events {
            let guards: Vec<&Predicate> = event_guards(event)
                .flat_map(|(_, guard)| conjuncts(&guard.typed))
                .collect();
            for parameter in event.chain_parameters() {
                if parameter.ty != Type::Int || bounded(&parameter.name, &guards) {
                    continue;
                }
                diags.push(integer_diagnostic(
                    format!("{}.{}/{}", machine.name(), event.label, parameter.name),
                    &parameter.name,
                    "parameter",
                ));
            }
        }
    }
}

fn integer_diagnostic(origin: String, name: &str, kind: &str) -> Diagnostic {
    Diagnostic {
        severity: RuleId::UnboundedInteger.default_severity(),
        origin,
        message: format!(
            "Could not verify a range for integer {kind} `{name}`: its only constraints give it its type, so the width a translation gives it is not the model's choice"
        ),
        rule_id: Some(RuleId::UnboundedInteger),
        span: None,
    }
}

/// Whether anything among `constraints` says more about `name` than that it
/// is an integer.
///
/// A typing conjunct over `ℤ`, `ℕ` or `ℕ1` is exactly the declaration Rodin
/// requires and says nothing about range; `ℕ` bounds one end and is counted
/// as typing for that reason. Anything else that mentions the name at all —
/// an interval membership, a comparison, an equality — is treated as a
/// bound, deliberately generously: this rule guesses at intent, and the
/// costly mistake is the false positive.
fn bounded(name: &str, constraints: &[&Predicate]) -> bool {
    constraints
        .iter()
        .filter(|constraint| {
            constraint
                .free_identifiers()
                .iter()
                .any(|free| free == name)
        })
        .any(|constraint| !is_integer_typing(constraint, name))
}

fn is_integer_typing(pred: &Predicate, name: &str) -> bool {
    let Some((typed_name, bound)) = crate::lint::typing_conjunct(pred) else {
        return false;
    };
    typed_name == name
        && matches!(
            bound.kind(),
            ExpressionKind::Atomic(AtomicOp::Integer | AtomicOp::Natural | AtomicOp::Natural1)
        )
}

// ---------------------------------------------------------------------
// EB110 — actions that only agree with the model in parallel
// ---------------------------------------------------------------------

/// Report the events whose actions cannot be applied one after another.
///
/// Not an uncertainty: the model says exactly what happens. It is a
/// mistranslation trap, so the finding is INFO and the help names the three
/// ways translations get it right.
fn check_parallel_actions(machine: &CheckedMachine, diags: &mut Vec<Diagnostic>) {
    for event in &machine.record.events {
        let actions: Vec<(bool, &ActionDecl, &Assignment)> = event_actions(event).collect();
        let targets: Vec<Vec<&str>> = actions
            .iter()
            .map(|(_, action, _)| lhs_variables(&action.action))
            .collect();

        for (position, (own, action, typed)) in actions.iter().enumerate() {
            let origin = || format!("{}.{}/{}", machine.name(), event.label, action.label);
            let span = own_span(*own, typed.span());

            if matches!(typed.kind(), AssignmentKind::BecomesEqualTo { .. })
                && targets[position].len() > 1
            {
                diags.push(Diagnostic {
                    severity: RuleId::ParallelAssignmentHazard.default_severity(),
                    origin: origin(),
                    message: format!(
                        "{} are assigned simultaneously; applying the assignments in sequence would not give the same result",
                        quoted_list(&targets[position])
                    ),
                    rule_id: Some(RuleId::ParallelAssignmentHazard),
                    span,
                });
            }

            // A read of what a *different* action writes is the same hazard
            // spread over two clauses. Reading what this action itself
            // assigns is the ordinary before-state read every assignment
            // makes and is not reported.
            let clashes: Vec<&str> = typed
                .free_identifiers()
                .iter()
                .map(String::as_str)
                .filter(|name| !targets[position].contains(name))
                .filter(|name| {
                    targets
                        .iter()
                        .enumerate()
                        .any(|(other, assigned)| other != position && assigned.contains(name))
                })
                .collect();
            if !clashes.is_empty() {
                diags.push(Diagnostic {
                    severity: RuleId::ParallelAssignmentHazard.default_severity(),
                    origin: origin(),
                    message: format!(
                        "This action reads {}, which another action of the same event assigns; every action reads the before-state, so the two must not be applied in sequence",
                        quoted_list(&clashes)
                    ),
                    rule_id: Some(RuleId::ParallelAssignmentHazard),
                    span,
                });
            }
        }
    }
}

// ---------------------------------------------------------------------
// EB111 — guard depends on the evaluation order
// ---------------------------------------------------------------------

/// Report guards whose well-definedness nothing earlier establishes.
///
/// The conservative half of the question: a conjunct counts as established
/// when it appears verbatim among the earlier guards, the invariants or the
/// visible axioms. Real implication needs a prover, so a guard whose
/// condition merely *follows* from them is reported too. That is why this is
/// INFO, and why the message says the condition was not found rather than
/// that it does not hold.
fn check_guard_order(machine: &CheckedMachine, scope: &Scope, diags: &mut Vec<Diagnostic>) {
    let mut established: Vec<&Predicate> = scope.axioms.clone();
    established.extend(scope.invariants(machine));
    let context_depth = established.len();

    for event in &machine.record.events {
        established.truncate(context_depth);
        for (own, guard) in event_guards(event) {
            let lemma = guard.typed.wd_lemma();
            let missing: Vec<&Predicate> = essential_conditions(&lemma)
                .into_iter()
                .filter(|condition| !established.contains(condition))
                .collect();

            if !missing.is_empty() {
                diags.push(Diagnostic {
                    severity: RuleId::GuardEvaluationOrder.default_severity(),
                    origin: format!("{}.{}/{}", machine.name(), event.label, guard.label),
                    message: format!(
                        "Could not find {} among the earlier guards, the invariants or the axioms; a translation that evaluates guards in order may apply this one where it is not defined",
                        render_conditions(&missing)
                    ),
                    rule_id: Some(RuleId::GuardEvaluationOrder),
                    span: own_span(own, guard.typed.span()),
                });
            }

            // The guard itself is available to those below it: guards are a
            // conjunction, and a translation evaluating them in order has
            // already established this one.
            established.extend(conjuncts(&guard.typed));
        }
    }
}

/// The conditions of a well-definedness lemma that a guard could establish.
///
/// A lemma of a quantified guard carries its conditions under the
/// quantifier. One that does not mention the bound variable — `f` applied to
/// something fixed, inside a `∀` over something else — is a plain condition
/// and is reported as itself, which is both readable and comparable against
/// the earlier guards. One that does mention it cannot be lifted out, so the
/// whole quantified lemma is reported instead.
fn essential_conditions(lemma: &Predicate) -> Vec<&Predicate> {
    let mut out = Vec::new();
    push_essential(lemma, &mut out);
    out
}

fn push_essential<'a>(pred: &'a Predicate, out: &mut Vec<&'a Predicate>) {
    if is_discharged_by_typing(pred) {
        return;
    }
    match pred.kind() {
        PredicateKind::Associative {
            op: AssocPredOp::LAnd,
            children,
        } => {
            for child in children {
                push_essential(child, out);
            }
        }
        PredicateKind::Quantified { pred: body, .. }
        | PredicateKind::Binary {
            op: BinaryPredOp::LImp,
            right: body,
            ..
        } => {
            let mut inner = Vec::new();
            push_essential(body, &mut inner);
            let (closed, open): (Vec<_>, Vec<_>) = inner
                .into_iter()
                .partition(|condition| condition.dangling_bound_indices().is_empty());
            out.extend(closed);
            if !open.is_empty() {
                out.push(pred);
            }
        }
        _ => out.push(pred),
    }
}

/// Whether this well-definedness conjunct says nothing about evaluation
/// order.
///
/// Two kinds do not. The trivial `⊤`, and the functionality half of a
/// function application: applying `f` requires both that the argument is in
/// `dom(f)`, which is an ordering concern, and that `f` is a function at
/// all, which its type already settles and no guard would ever restate.
/// Reporting the second would bury the first — every application in a model
/// carries one.
///
/// A quantified conjunct is judged by its body, so a lemma that only
/// requires functionality under a quantifier is dropped like a bare one.
fn is_discharged_by_typing(pred: &Predicate) -> bool {
    match pred.kind() {
        PredicateKind::Literal(rossi::formula::tag::LiteralPredOp::BTrue) => true,
        PredicateKind::Relational {
            op: RelationalOp::In,
            right,
            ..
        } => is_function_space(right),
        PredicateKind::Quantified { pred: body, .. } => {
            conjuncts(body).into_iter().all(is_discharged_by_typing)
        }
        PredicateKind::Binary {
            op: BinaryPredOp::LImp,
            right,
            ..
        } => conjuncts(right).into_iter().all(is_discharged_by_typing),
        PredicateKind::Associative {
            op: AssocPredOp::LAnd,
            children,
        } => children.iter().all(is_discharged_by_typing),
        _ => false,
    }
}

/// Whether `expr` is a function or relation space between two types — the
/// shape a declaration gives, not one a guard establishes.
fn is_function_space(expr: &Expression) -> bool {
    let ExpressionKind::Binary { op, left, right } = expr.kind() else {
        return false;
    };
    is_relation_space_op(*op) && left.is_type_expression() && right.is_type_expression()
}

fn render_conditions(conditions: &[&Predicate]) -> String {
    let printer = rossi::pretty::PrettyPrinter::rodin_formula_string();
    conditions
        .iter()
        .map(|condition| format!("`{}`", printer.print_formula_predicate(condition)))
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------
// EB112 / EB113 — what the refinement leaves behind
// ---------------------------------------------------------------------

/// EB112 — variables the abstraction declares that the leaf does not keep.
///
/// A refinement inherits its abstract variables rather than re-declaring
/// them, so they stay visible whether or not it keeps them; what marks one
/// as dropped is that the checked machine has it as abstract only, with no
/// concrete counterpart.
fn check_dropped_variables(machine: &CheckedMachine, scope: &Scope, diags: &mut Vec<Diagnostic>) {
    for variable in &machine.record.variables {
        if !variable.is_abstract || variable.is_concrete {
            continue;
        }
        let declared_by = machine
            .ancestors()
            .iter()
            .rev()
            .find(|name| {
                scope.model.machines.get(*name).is_some_and(|ancestor| {
                    ancestor
                        .record
                        .variables
                        .iter()
                        .any(|v| v.name == variable.name && v.is_concrete)
                })
            })
            .map_or_else(|| "its abstraction".to_string(), |name| format!("`{name}`"));

        diags.push(Diagnostic {
            severity: RuleId::DroppedAbstractVariable.default_severity(),
            origin: format!("{}.{}", machine.name(), variable.name),
            message: format!(
                "`{}` is declared by {declared_by} and not kept here, so nothing this machine runs can reproduce what the model states about it",
                variable.name
            ),
            rule_id: Some(RuleId::DroppedAbstractVariable),
            span: None,
        });
    }
}

/// EB113 — witnesses that constrain the abstract value without fixing it.
fn check_witnesses(machine: &CheckedMachine, diags: &mut Vec<Diagnostic>) {
    for event in &machine.record.events {
        for witness in &event.witnesses {
            // The label names what is being witnessed; a primed label
            // witnesses an abstract variable's after-value.
            if determines(&witness.typed, &witness.label) {
                continue;
            }
            diags.push(Diagnostic {
                severity: RuleId::NonEqualityWitness.default_severity(),
                origin: format!("{}.{}/{}", machine.name(), event.label, witness.label),
                message: format!(
                    "Could not verify the abstract value of `{}`: the witness constrains it rather than fixing it with `{} = …`",
                    witness.label, witness.label
                ),
                rule_id: Some(RuleId::NonEqualityWitness),
                span: witness.typed.span(),
            });
        }
    }
}
