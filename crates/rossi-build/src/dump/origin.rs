//! Where a checked declaration was written, and what was written beside it.
//!
//! The checked model keeps what checking produced: names, types, formulas and
//! Rodin handles. It does not keep where an identifier was spelled, nor the
//! comment attached to it. Both live on the source syntax tree, so both are
//! recovered the same way, by pairing each checked declaration back to the
//! clause it came from.
//!
//! Pairing is positional where the model records a position, and by name or
//! label everywhere else. A position alone is not enough: an unlabelled
//! clause is given a constant default label by the checker, so several
//! clauses can end up sharing one, and the pairing is confirmed against the
//! label before it is trusted.
//!
//! Two checked records have no clause at all, because the checker synthesized
//! them. Both are recognisable without guessing: a synthesized record is
//! sourced on its event rather than on a child of it.

use std::collections::HashMap;

use rossi::Component;
use rossi::ast::{
    Context, Event, InitialisationEvent, LabeledAction, LabeledPredicate, Machine, NamedElement,
    Span,
};

use crate::handles::HandleUri;
use crate::project::Project;

use super::location::LineIndex;

/// What a checked declaration inherits from the clause that produced it.
#[derive(Debug, Default, Clone)]
pub(crate) struct Origin<'a> {
    pub(crate) comment: Option<&'a str>,
    pub(crate) span: Option<Span>,
}

impl<'a> Origin<'a> {
    fn of_named(element: &'a NamedElement) -> Origin<'a> {
        Origin {
            comment: element.comment.as_deref(),
            span: element.span,
        }
    }

    fn of_predicate(clause: &'a LabeledPredicate) -> Origin<'a> {
        Origin {
            comment: clause.comment.as_deref(),
            span: clause.span,
        }
    }

    fn of_action(clause: &'a LabeledAction) -> Origin<'a> {
        Origin {
            comment: clause.comment.as_deref(),
            span: clause.span,
        }
    }
}

/// An event's source clause, whichever of the two shapes it has.
///
/// Initialisation is a distinct kind of event in the syntax tree: it has no
/// name, no parameters and no guards. Reaching it by label and reading it
/// through one view keeps that difference out of the assembly.
#[derive(Clone, Copy)]
enum EventSyntax<'a> {
    Ordinary(&'a Event),
    Initialisation(&'a InitialisationEvent),
}

impl<'a> EventSyntax<'a> {
    fn own(self) -> Origin<'a> {
        match self {
            EventSyntax::Ordinary(e) => Origin {
                comment: e.comment.as_deref(),
                span: e.span,
            },
            EventSyntax::Initialisation(e) => Origin {
                comment: e.comment.as_deref(),
                span: e.span,
            },
        }
    }

    fn parameters(self) -> &'a [NamedElement] {
        match self {
            EventSyntax::Ordinary(e) => &e.parameters,
            EventSyntax::Initialisation(_) => &[],
        }
    }

    fn guards(self) -> &'a [LabeledPredicate] {
        match self {
            EventSyntax::Ordinary(e) => &e.guards,
            EventSyntax::Initialisation(_) => &[],
        }
    }

    fn actions(self) -> &'a [LabeledAction] {
        match self {
            EventSyntax::Ordinary(e) => &e.actions,
            EventSyntax::Initialisation(e) => &e.actions,
        }
    }

    /// The witness clauses, in the order the checker merges them.
    fn witnesses(self) -> impl Iterator<Item = &'a LabeledPredicate> {
        let (with, witnesses) = match self {
            EventSyntax::Ordinary(e) => (&e.with, &e.witnesses),
            EventSyntax::Initialisation(e) => (&e.with, &e.witnesses),
        };
        with.iter().chain(witnesses.iter())
    }
}

/// The source syntax of one component, indexed for lookup.
pub(crate) struct ComponentOrigin<'a> {
    /// The source identity a span in this component refers to.
    pub(crate) source_id: String,
    /// The component's line table, when it has text. `None` for a component
    /// imported from Rodin XML, whose formulas carry offsets into attribute
    /// strings the reader of a document never sees.
    pub(crate) lines: Option<LineIndex<'a>>,
    /// The component's own comment and span.
    pub(crate) own: Origin<'a>,
    syntax: Syntax<'a>,
}

enum Syntax<'a> {
    Context(&'a Context),
    Machine(&'a Machine),
}

impl<'a> ComponentOrigin<'a> {
    /// The initialisation label, which reaches a different syntax node from
    /// every other event.
    fn machine(&self) -> Option<&'a Machine> {
        match self.syntax {
            Syntax::Machine(m) => Some(m),
            Syntax::Context(_) => None,
        }
    }

    fn context(&self) -> Option<&'a Context> {
        match self.syntax {
            Syntax::Context(c) => Some(c),
            Syntax::Machine(_) => None,
        }
    }

    pub(crate) fn carrier_set(&self, name: &str) -> Origin<'a> {
        self.context()
            .and_then(|c| named(&c.sets, name))
            .unwrap_or_default()
    }

    pub(crate) fn constant(&self, name: &str) -> Origin<'a> {
        self.context()
            .and_then(|c| named(&c.constants, name))
            .unwrap_or_default()
    }

    pub(crate) fn axiom(&self, index: usize, label: &str) -> Origin<'a> {
        self.context()
            .and_then(|c| predicate_at(&c.axioms, index, label))
            .unwrap_or_default()
    }

    pub(crate) fn variable(&self, name: &str) -> Origin<'a> {
        self.machine()
            .and_then(|m| named(&m.variables, name))
            .unwrap_or_default()
    }

    pub(crate) fn invariant(&self, index: usize, label: &str) -> Origin<'a> {
        self.machine()
            .and_then(|m| predicate_at(&m.invariants, index, label))
            .unwrap_or_default()
    }

    pub(crate) fn variant(&self, label: &str) -> Origin<'a> {
        self.machine()
            .and_then(|m| m.variants.iter().find(|v| v.effective_label() == label))
            .map(|v| Origin {
                comment: v.comment.as_deref(),
                span: v.span,
            })
            .unwrap_or_default()
    }

    pub(crate) fn event(&self, label: &str) -> Origin<'a> {
        self.event_syntax(label)
            .map(EventSyntax::own)
            .unwrap_or_default()
    }

    pub(crate) fn parameter(&self, event: &str, name: &str) -> Origin<'a> {
        self.event_syntax(event)
            .and_then(|e| named(e.parameters(), name))
            .unwrap_or_default()
    }

    pub(crate) fn guard(&self, event: &str, index: usize, label: &str) -> Origin<'a> {
        self.event_syntax(event)
            .and_then(|e| predicate_at(e.guards(), index, label))
            .unwrap_or_default()
    }

    pub(crate) fn action(&self, event: &str, index: usize, label: &str) -> Origin<'a> {
        self.event_syntax(event)
            .and_then(|e| {
                let clause = e.actions().get(index)?;
                label_matches(clause.label.as_deref(), label).then(|| Origin::of_action(clause))
            })
            .unwrap_or_default()
    }

    pub(crate) fn witness(&self, event: &str, label: &str) -> Origin<'a> {
        self.event_syntax(event)
            .and_then(|e| {
                e.witnesses()
                    .find(|w| w.label.as_deref() == Some(label))
                    .map(Origin::of_predicate)
            })
            .unwrap_or_default()
    }

    fn event_syntax(&self, label: &str) -> Option<EventSyntax<'a>> {
        let machine = self.machine()?;
        if label == crate::sc::initialisation_label() {
            return machine
                .initialisation
                .as_ref()
                .map(EventSyntax::Initialisation);
        }
        machine
            .events
            .iter()
            .find(|e| e.name == label)
            .map(EventSyntax::Ordinary)
    }
}

fn named<'a>(elements: &'a [NamedElement], name: &str) -> Option<Origin<'a>> {
    elements
        .iter()
        .find(|e| e.name == name)
        .map(Origin::of_named)
}

/// The clause at `index`, if its label agrees with the checked one.
///
/// The checker keeps clause order, so the index is the reliable half of the
/// pairing. The label is the guard: a clause written without one is given a
/// constant default, so a mismatch means the lists no longer correspond and
/// the position must not be trusted.
fn predicate_at<'a>(
    clauses: &'a [LabeledPredicate],
    index: usize,
    label: &str,
) -> Option<Origin<'a>> {
    let clause = clauses.get(index)?;
    label_matches(clause.label.as_deref(), label).then(|| Origin::of_predicate(clause))
}

/// Whether a source clause corresponds to a checked declaration's label.
///
/// An unlabelled clause carries no evidence either way, so the position
/// stands on its own; a labelled one has to agree.
fn label_matches(written: Option<&str>, checked: &str) -> bool {
    written.is_none_or(|label| label == checked)
}

/// Every component's source syntax, keyed by component name.
pub(crate) struct Origins<'a> {
    by_name: HashMap<&'a str, ComponentOrigin<'a>>,
}

impl<'a> Origins<'a> {
    pub(crate) fn build(project: &'a Project) -> Origins<'a> {
        let mut by_name = HashMap::new();
        for component in &project.components {
            let lines = component.source.as_deref().map(LineIndex::new);
            let source_id = component.source_id().to_string();
            let origin = match &component.component {
                Component::Context(context) => ComponentOrigin {
                    source_id,
                    lines,
                    own: Origin {
                        comment: context.comment.as_deref(),
                        span: context.span,
                    },
                    syntax: Syntax::Context(context),
                },
                Component::Machine(machine) => ComponentOrigin {
                    source_id,
                    lines,
                    own: Origin {
                        comment: machine.comment.as_deref(),
                        span: machine.span,
                    },
                    syntax: Syntax::Machine(machine),
                },
            };
            by_name.insert(component.component.name(), origin);
        }
        Origins { by_name }
    }

    pub(crate) fn get(&self, component: &str) -> Option<&ComponentOrigin<'a>> {
        self.by_name.get(component)
    }
}

/// Whether a checked declaration was synthesized by the checker rather than
/// written.
///
/// The initialisation repair pass adds an action, and an unmet witness name
/// gets a trivial one. Neither has a source clause, and the repair action
/// carries a placeholder position that would otherwise pair it with a real
/// clause and steal that clause's comment. Both are sourced on the event
/// itself instead of on a child of it, which says so without guessing at
/// labels.
pub(crate) fn is_synthesized(declaration: &HandleUri, event: &HandleUri) -> bool {
    declaration == event
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::ProjectComponent;

    fn project(source: &str) -> Project {
        let components =
            ProjectComponent::from_eventb("m.eventb", source).expect("the source parses");
        Project::new("p", components)
    }

    const MACHINE: &str = "\
machine m
sees c
variables
    v
invariants
    @inv1 v ∈ ℕ // first
    @inv2 v ≥ 0 // second
events
event INITIALISATION
then
    @act1 v ≔ 0 // initial
end
event step
any
    p
where
    @grd1 p ∈ ℕ // a guard
then
    @act1 v ≔ p
end
end
";

    #[test]
    fn a_guard_is_paired_by_position_and_confirmed_by_label() {
        let project = project(MACHINE);
        let origins = Origins::build(&project);
        let component = origins.get("m").expect("the machine is indexed");

        assert_eq!(component.guard("step", 0, "grd1").comment, Some("a guard"));
        // The same position under a different label is a mismatch, so the
        // pairing is refused rather than reported against the wrong clause.
        assert_eq!(component.guard("step", 0, "grd9").comment, None);
    }

    #[test]
    fn each_invariant_keeps_its_own_comment() {
        let project = project(MACHINE);
        let origins = Origins::build(&project);
        let component = origins.get("m").expect("the machine is indexed");

        assert_eq!(component.invariant(0, "inv1").comment, Some("first"));
        assert_eq!(component.invariant(1, "inv2").comment, Some("second"));
    }

    #[test]
    fn initialisation_is_reached_through_its_own_syntax() {
        let project = project(MACHINE);
        let origins = Origins::build(&project);
        let component = origins.get("m").expect("the machine is indexed");

        let label = crate::sc::initialisation_label();
        assert_eq!(component.action(label, 0, "act1").comment, Some("initial"));
        // It has no parameters or guards to reach, and asking must not
        // fall through to another event's.
        assert_eq!(component.parameter(label, "p").span, None);
        assert_eq!(component.guard(label, 0, "grd1").comment, None);
    }

    #[test]
    fn an_identifier_is_paired_by_name() {
        let project = project(MACHINE);
        let origins = Origins::build(&project);
        let component = origins.get("m").expect("the machine is indexed");

        assert!(component.variable("v").span.is_some());
        assert_eq!(component.variable("absent").span, None);
        assert!(component.parameter("step", "p").span.is_some());
    }

    #[test]
    fn an_index_past_the_written_clauses_pairs_with_nothing() {
        let project = project(MACHINE);
        let origins = Origins::build(&project);
        let component = origins.get("m").expect("the machine is indexed");

        assert_eq!(component.invariant(99, "inv1").comment, None);
        assert_eq!(component.action("step", 99, "act1").comment, None);
    }

    #[test]
    fn a_component_from_text_has_a_line_table() {
        let project = project(MACHINE);
        let origins = Origins::build(&project);
        let component = origins.get("m").expect("the machine is indexed");

        assert!(component.lines.is_some());
        assert_eq!(component.source_id, "m.eventb");
    }

    #[test]
    fn a_synthesized_declaration_is_told_from_a_written_one() {
        let event = HandleUri::root("p", "m.bum", "org.eventb.core.machineFile", "m")
            .child("org.eventb.core.event", "step");
        let written = event.child("org.eventb.core.action", "act1");

        assert!(is_synthesized(&event, &event));
        assert!(!is_synthesized(&written, &event));
    }
}
