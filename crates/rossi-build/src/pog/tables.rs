//! Per-event tables: actions split by determinism with the frame
//! substitutions, guard correspondences between an event and the
//! abstract events it refines, and the machine-state lookups the
//! event scope is built from.

use std::collections::{BTreeSet, HashMap};

use rossi::formula::{Assignment, AssignmentKind, Expression, FormulaFactory, Predicate, Type};

use crate::handles::HandleUri;
use crate::sc::machine_record::{ActionDecl, EventDecl, MachineRecord};
use crate::sc::{CheckedMachine, ScModel};

/// A simultaneous free-identifier substitution (one batch of parallel
/// assignments). An empty batch is a no-op.
pub(super) type Substitution = HashMap<String, Expression>;

/// Apply a substitution batch.
pub(super) fn apply(predicate: &Predicate, subst: &Substitution) -> Predicate {
    if subst.is_empty() {
        predicate.clone()
    } else {
        predicate.substitute_free_idents(subst)
    }
}

/// The primed after-value identifier of a variable.
pub(super) fn primed(ff: &FormulaFactory, name: &str, ty: &Type) -> Expression {
    ff.free_identifier(format!("{name}'"), None, Some(ty.clone()))
}

/// The concrete machine state: every concrete variable in declaration
/// order with whether it is preserved (also present in the
/// abstraction), plus a by-name type lookup over all visible variables
/// (abstract-only ones included) — the type source for primed
/// identifiers.
pub(super) struct MachineVariables {
    pub rows: Vec<(String, Type, bool)>,
    pub types: HashMap<String, Type>,
}

impl MachineVariables {
    pub fn new(record: &MachineRecord) -> Self {
        MachineVariables {
            rows: record
                .variables
                .iter()
                .filter(|v| v.is_concrete)
                .map(|v| (v.name.clone(), v.ty.clone(), v.is_abstract))
                .collect(),
            types: record
                .variables
                .iter()
                .map(|v| (v.name.clone(), v.ty.clone()))
                .collect(),
        }
    }
}

/// The names of every variable the actions assign.
fn assigned_variables(actions: &[ActionDecl]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for decl in actions {
        let Some(assignment) = &decl.typed else {
            continue;
        };
        for ident in assignment.assigned_identifiers() {
            if let rossi::formula::ExpressionKind::FreeIdentifier(name) = ident.kind() {
                out.insert(name.clone());
            }
        }
    }
    out
}

/// One action and its typed assignment.
pub(super) struct ActionInfo {
    pub label: String,
    pub source: HandleUri,
    pub assignment: Assignment,
}

/// An event's actions, split by determinism.
pub(super) struct EventActionTable {
    pub actions: Vec<ActionInfo>,
    /// Indices into `actions` of the nondeterministic ones, with their
    /// before-after predicates.
    pub nondet: Vec<(usize, Predicate)>,
    /// `x' ↦ E` for every deterministic action, as one batch.
    pub primed_det: Substitution,
    pub assigned_variables: BTreeSet<String>,
}

impl EventActionTable {
    pub fn new(action_decls: &[ActionDecl]) -> Self {
        let mut actions = Vec::new();
        let mut nondet = Vec::new();
        let mut primed_det = Substitution::new();
        for decl in action_decls {
            let Some(assignment) = &decl.typed else {
                continue;
            };
            let index = actions.len();
            actions.push(ActionInfo {
                label: decl.label.clone(),
                source: decl.source.clone(),
                assignment: assignment.clone(),
            });
            match assignment.kind() {
                AssignmentKind::BecomesEqualTo { idents, values } => {
                    for (ident, value) in idents.iter().zip(values) {
                        if let rossi::formula::ExpressionKind::FreeIdentifier(name) = ident.kind() {
                            primed_det.insert(format!("{name}'"), value.clone());
                        }
                    }
                }
                AssignmentKind::BecomesMemberOf { .. } | AssignmentKind::BecomesSuchThat { .. } => {
                    nondet.push((index, assignment.ba_predicate()));
                }
            }
        }
        EventActionTable {
            actions,
            nondet,
            primed_det,
            assigned_variables: assigned_variables(action_decls),
        }
    }
}

/// The concrete event's actions plus the frame substitutions over the
/// machine state.
pub(super) struct ConcreteEventActionTable {
    pub table: EventActionTable,
    /// `v ↦ v'` for every variable the event assigns.
    pub delta_prime: Substitution,
    /// The after-state batch: `v' ↦ v` for every concrete variable the
    /// event leaves alone, and `x' ↦ E` for every deterministic
    /// assignment — disjoint key sets, so order-independent.
    pub after_state: Substitution,
}

impl ConcreteEventActionTable {
    pub fn new(actions: &[ActionDecl], variables: &MachineVariables, ff: &FormulaFactory) -> Self {
        let table = EventActionTable::new(actions);
        let mut xi = Substitution::new();
        let mut delta = Substitution::new();
        for (name, ty, _) in &variables.rows {
            if table.assigned_variables.contains(name) {
                delta.insert(name.clone(), primed(ff, name, ty));
            } else {
                xi.insert(
                    format!("{name}'"),
                    ff.free_identifier(name, None, Some(ty.clone())),
                );
            }
        }
        let after_state = merge_batches([&xi, &table.primed_det]);
        ConcreteEventActionTable {
            table,
            delta_prime: delta,
            after_state,
        }
    }
}

/// The abstract event's actions, with the correspondence to the
/// concrete action list (structural formula equality) and the
/// disappearing-variable split.
pub(super) struct AbstractEventActionTable {
    pub table: EventActionTable,
    /// For each concrete action, the index of the identical abstract
    /// action, if any.
    pub index_of_abstract: Vec<Option<usize>>,
    /// For each abstract action, the index of the identical concrete
    /// action, if any.
    pub index_of_concrete: Vec<Option<usize>>,
    /// The abstract assignments over surviving variables — what the
    /// concrete event must simulate. A deterministic assignment mixing
    /// surviving and dropped variables splits; a nondeterministic one
    /// simulates whole.
    pub sim: Vec<ActionInfo>,
    /// `y ↦ F` for every deterministic abstract assignment to a
    /// variable this refinement dropped — an implicit witness.
    pub disappearing_witnesses: Substitution,
}

/// For each action of `from`, the index of the identical action in
/// `to`, if any.
fn correspondence(from: &[ActionInfo], to: &[ActionInfo]) -> Vec<Option<usize>> {
    from.iter()
        .map(|f| to.iter().position(|t| t.assignment == f.assignment))
        .collect()
}

impl AbstractEventActionTable {
    pub fn new(
        actions: &[ActionDecl],
        variables: &MachineVariables,
        concrete: &ConcreteEventActionTable,
    ) -> Self {
        let table = EventActionTable::new(actions);
        let index_of_abstract = correspondence(&concrete.table.actions, &table.actions);
        let index_of_concrete = correspondence(&table.actions, &concrete.table.actions);

        let concrete_names: BTreeSet<&str> = variables
            .rows
            .iter()
            .map(|(name, _, _)| name.as_str())
            .collect();
        let mut sim = Vec::new();
        let mut disappearing = Substitution::new();
        for action in &table.actions {
            match action.assignment.kind() {
                AssignmentKind::BecomesEqualTo { idents, values } => {
                    let mut surviving_idents = Vec::new();
                    let mut surviving_values = Vec::new();
                    for (ident, value) in idents.iter().zip(values) {
                        let rossi::formula::ExpressionKind::FreeIdentifier(name) = ident.kind()
                        else {
                            continue;
                        };
                        if concrete_names.contains(name.as_str()) {
                            surviving_idents.push(ident.clone());
                            surviving_values.push(value.clone());
                        } else {
                            disappearing.insert(name.clone(), value.clone());
                        }
                    }
                    if !surviving_idents.is_empty() {
                        let assignment = if surviving_idents.len() == idents.len() {
                            action.assignment.clone()
                        } else {
                            action.assignment.factory().becomes_equal_to(
                                surviving_idents,
                                surviving_values,
                                None,
                            )
                        };
                        sim.push(ActionInfo {
                            label: action.label.clone(),
                            source: action.source.clone(),
                            assignment,
                        });
                    }
                }
                AssignmentKind::BecomesMemberOf { .. } | AssignmentKind::BecomesSuchThat { .. } => {
                    sim.push(ActionInfo {
                        label: action.label.clone(),
                        source: action.source.clone(),
                        assignment: action.assignment.clone(),
                    });
                }
            }
        }

        AbstractEventActionTable {
            table,
            index_of_abstract,
            index_of_concrete,
            sim,
            disappearing_witnesses: disappearing,
        }
    }
}

/// One witness: the witnessed identifier is its label — a parameter
/// name for a dropped abstract parameter, a primed variable for a
/// dropped abstract variable.
pub(super) struct WitnessInfo {
    pub label: String,
    pub source: HandleUri,
    pub predicate: Predicate,
    pub deterministic: bool,
}

/// An event's witnesses, classified.
///
/// A witness `w` is deterministic when its predicate is `w = E` with
/// `w` not free in `E`; it then acts as a substitution. Deterministic
/// witnesses for primed variables contribute both the unprimed
/// (`v ↦ E`) and primed (`v' ↦ E`) forms; parameter witnesses
/// contribute `p ↦ E`. Nondeterministic witnesses for primed
/// variables contribute `v ↦ v'` instead.
pub(super) struct EventWitnessTable {
    pub witnesses: Vec<WitnessInfo>,
    /// `v ↦ E` for deterministic primed-variable witnesses.
    pub machine_det: Substitution,
    /// `v' ↦ E` for deterministic primed-variable witnesses.
    pub machine_primed_det: Substitution,
    /// `p ↦ E` for deterministic parameter witnesses.
    pub event_det: Substitution,
    /// `v ↦ v'` for nondeterministic primed-variable witnesses.
    pub prime_substitution: Substitution,
}

impl EventWitnessTable {
    pub fn new(event: &EventDecl, variables: &MachineVariables, ff: &FormulaFactory) -> Self {
        let mut witnesses = Vec::new();
        let mut machine_det = Substitution::new();
        let mut machine_primed_det = Substitution::new();
        let mut event_det = Substitution::new();
        let mut prime = Substitution::new();

        for witness in &event.witnesses {
            let label = witness.label.clone();
            let deterministic = deterministic_value(&label, &witness.typed);
            witnesses.push(WitnessInfo {
                label: label.clone(),
                source: witness.source.clone(),
                predicate: witness.typed.clone(),
                deterministic: deterministic.is_some(),
            });
            match (deterministic, label.strip_suffix('\'')) {
                (Some(value), Some(base)) => {
                    machine_det.insert(base.to_string(), value.clone());
                    machine_primed_det.insert(label, value);
                }
                (Some(value), None) => {
                    event_det.insert(label, value);
                }
                (None, base) => {
                    if let Some(base) = base
                        && let Some(ty) = variables.types.get(base)
                    {
                        prime.insert(base.to_string(), primed(ff, base, ty));
                    }
                }
            }
        }

        EventWitnessTable {
            witnesses,
            machine_det,
            machine_primed_det,
            event_det,
            prime_substitution: prime,
        }
    }

    /// The nondeterministic witnesses, in declaration order.
    pub fn nondet(&self) -> impl Iterator<Item = &WitnessInfo> {
        self.witnesses.iter().filter(|w| !w.deterministic)
    }
}

/// The witnessed value when the witness is deterministic: `label = E`
/// with the label identifier not free in `E`.
fn deterministic_value(label: &str, predicate: &Predicate) -> Option<Expression> {
    use rossi::formula::{PredicateKind, tag};
    let PredicateKind::Relational {
        op: tag::RelationalOp::Equal,
        left,
        right,
    } = predicate.kind()
    else {
        return None;
    };
    let rossi::formula::ExpressionKind::FreeIdentifier(name) = left.kind() else {
        return None;
    };
    if name != label || right.free_identifiers().iter().any(|n| n == label) {
        return None;
    }
    Some(right.clone())
}

/// Merge substitution batches applied together: later entries win on a
/// shared key, matching the order they are listed in.
pub(super) fn merge_batches<'a>(
    batches: impl IntoIterator<Item = &'a Substitution>,
) -> Substitution {
    let mut merged = Substitution::new();
    for batch in batches {
        for (key, value) in batch {
            merged.insert(key.clone(), value.clone());
        }
    }
    merged
}

/// One guard of the effective (inherited-then-own) guard list.
pub(super) struct GuardInfo {
    pub label: String,
    /// The checked file's internal name of the guard row.
    pub internal_name: String,
    pub predicate: Predicate,
    pub source: HandleUri,
    pub is_theorem: bool,
}

/// The effective guards of an event, in checked-file order.
pub(super) struct ConcreteEventGuardTable {
    pub guards: Vec<GuardInfo>,
}

impl ConcreteEventGuardTable {
    pub fn new(machine: &CheckedMachine, event: &EventDecl) -> Self {
        let guards = event
            .chain_guards()
            .into_iter()
            .map(|guard| GuardInfo {
                label: guard.label.clone(),
                internal_name: machine
                    .event_child_internal_name(
                        &event.label,
                        crate::xml_out::tag::SC_GUARD,
                        &guard.label,
                    )
                    .unwrap_or(&guard.label)
                    .to_string(),
                predicate: guard.typed.clone(),
                source: guard.source.clone(),
                is_theorem: guard.is_theorem,
            })
            .collect();
        ConcreteEventGuardTable { guards }
    }
}

/// An abstract event's effective guards, with the correspondence to
/// the concrete guard list (structural formula equality).
pub(super) struct AbstractEventGuardTable {
    pub guards: Vec<AbstractGuardInfo>,
    /// For each abstract guard, the index of the identical concrete
    /// guard, if any.
    pub index_of_concrete: Vec<Option<usize>>,
}

pub(super) struct AbstractGuardInfo {
    pub label: String,
    pub source: HandleUri,
    pub predicate: Predicate,
    pub is_theorem: bool,
}

impl AbstractEventGuardTable {
    pub fn new(abstract_event: &EventDecl, concrete: &ConcreteEventGuardTable) -> Self {
        let guards: Vec<AbstractGuardInfo> = abstract_event
            .chain_guards()
            .into_iter()
            .map(|guard| AbstractGuardInfo {
                label: guard.label.clone(),
                source: guard.source.clone(),
                predicate: guard.typed.clone(),
                is_theorem: guard.is_theorem,
            })
            .collect();
        let index_of_concrete = guards
            .iter()
            .map(|a| {
                concrete
                    .guards
                    .iter()
                    .position(|c| c.predicate == a.predicate)
            })
            .collect();
        AbstractEventGuardTable {
            guards,
            index_of_concrete,
        }
    }
}

/// How an event relates to its abstraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RefinementType {
    /// A new event, refining nothing.
    Intro,
    /// Refines one abstract event.
    Split,
    /// Merges several abstract events.
    Merge,
}

/// The abstract events an event refines, with their guard tables.
pub(super) struct AbstractEventGuardList {
    pub events: Vec<std::rc::Rc<EventDecl>>,
    pub tables: Vec<AbstractEventGuardTable>,
    pub refinement_type: RefinementType,
}

impl AbstractEventGuardList {
    pub fn new(
        model: &ScModel,
        machine: &CheckedMachine,
        event: &EventDecl,
        concrete: &ConcreteEventGuardTable,
    ) -> Self {
        let events: Vec<std::rc::Rc<EventDecl>> = model
            .abstract_events(machine, event)
            .into_iter()
            .cloned()
            .collect();
        let tables: Vec<AbstractEventGuardTable> = events
            .iter()
            .map(|abstract_event| AbstractEventGuardTable::new(abstract_event, concrete))
            .collect();
        let refinement_type = match events.len() {
            0 => RefinementType::Intro,
            1 => RefinementType::Split,
            _ => RefinementType::Merge,
        };
        AbstractEventGuardList {
            events,
            tables,
            refinement_type,
        }
    }

    pub fn first_abstract_event(&self) -> Option<&std::rc::Rc<EventDecl>> {
        self.events.first()
    }
}
