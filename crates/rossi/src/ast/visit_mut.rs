//! Default-recursing mutable traversal over the Event-B AST.
//!
//! Override only the node kinds a transform needs, and call the corresponding
//! `walk_*` function when the transform should continue into that node's
//! children. The defaults recurse through the component structure only:
//! formulas are immutable formula-model trees and are not visited.

use super::{
    ClauseRegion, Component, Context, Event, FileMetadata, InitialisationEvent, LabeledAction,
    LabeledPredicate, Machine, NamedElement, Span, Variant,
};

/// A mutable AST visitor whose methods recurse by default.
pub trait VisitMut {
    fn visit_component(&mut self, component: &mut Component) {
        walk_component(self, component);
    }

    fn visit_context(&mut self, context: &mut Context) {
        walk_context(self, context);
    }

    fn visit_machine(&mut self, machine: &mut Machine) {
        walk_machine(self, machine);
    }

    fn visit_named_element(&mut self, element: &mut NamedElement) {
        walk_named_element(self, element);
    }

    fn visit_labeled_predicate(&mut self, predicate: &mut LabeledPredicate) {
        walk_labeled_predicate(self, predicate);
    }

    fn visit_labeled_action(&mut self, action: &mut LabeledAction) {
        walk_labeled_action(self, action);
    }

    fn visit_variant(&mut self, variant: &mut Variant) {
        walk_variant(self, variant);
    }

    fn visit_event(&mut self, event: &mut Event) {
        walk_event(self, event);
    }

    fn visit_initialisation(&mut self, initialisation: &mut InitialisationEvent) {
        walk_initialisation(self, initialisation);
    }

    fn visit_clause_region(&mut self, clause: &mut ClauseRegion) {
        walk_clause_region(self, clause);
    }

    fn visit_file_metadata(&mut self, _metadata: &mut FileMetadata) {}

    fn visit_span(&mut self, _span: &mut Span) {}

    /// Visit a span that may be absent.
    ///
    /// Overriding this is the only way to *remove* a span rather than adjust
    /// one, which is what an AST comparison needs; the default forwards a
    /// present span to [`VisitMut::visit_span`] so a transform that only cares
    /// about positions can ignore the distinction.
    fn visit_optional_span(&mut self, span: &mut Option<Span>) {
        if let Some(span) = span {
            self.visit_span(span);
        }
    }
}

fn visit_optional_span<V: VisitMut + ?Sized>(visitor: &mut V, span: &mut Option<Span>) {
    visitor.visit_optional_span(span);
}

pub fn walk_component<V: VisitMut + ?Sized>(visitor: &mut V, component: &mut Component) {
    match component {
        Component::Context(context) => visitor.visit_context(context),
        Component::Machine(machine) => visitor.visit_machine(machine),
    }
}

pub fn walk_context<V: VisitMut + ?Sized>(visitor: &mut V, context: &mut Context) {
    for set in &mut context.sets {
        visitor.visit_named_element(set);
    }
    for constant in &mut context.constants {
        visitor.visit_named_element(constant);
    }
    for axiom in &mut context.axioms {
        visitor.visit_labeled_predicate(axiom);
    }
    visit_optional_span(visitor, &mut context.span);
    visit_optional_span(visitor, &mut context.name_span);
    for clause in &mut context.clauses {
        visitor.visit_clause_region(clause);
    }
    if let Some(metadata) = &mut context.metadata {
        visitor.visit_file_metadata(metadata);
    }
}

pub fn walk_machine<V: VisitMut + ?Sized>(visitor: &mut V, machine: &mut Machine) {
    for variable in &mut machine.variables {
        visitor.visit_named_element(variable);
    }
    for invariant in &mut machine.invariants {
        visitor.visit_labeled_predicate(invariant);
    }
    for variant in &mut machine.variants {
        visitor.visit_variant(variant);
    }
    if let Some(initialisation) = &mut machine.initialisation {
        visitor.visit_initialisation(initialisation);
    }
    for event in &mut machine.events {
        visitor.visit_event(event);
    }
    visit_optional_span(visitor, &mut machine.span);
    visit_optional_span(visitor, &mut machine.name_span);
    for clause in &mut machine.clauses {
        visitor.visit_clause_region(clause);
    }
    if let Some(metadata) = &mut machine.metadata {
        visitor.visit_file_metadata(metadata);
    }
}

pub fn walk_named_element<V: VisitMut + ?Sized>(visitor: &mut V, element: &mut NamedElement) {
    visit_optional_span(visitor, &mut element.span);
}

pub fn walk_labeled_predicate<V: VisitMut + ?Sized>(
    visitor: &mut V,
    predicate: &mut LabeledPredicate,
) {
    // The formula itself is a formula-model tree — immutable, out of
    // this legacy visitor's reach. Only the structural span is visited.
    visit_optional_span(visitor, &mut predicate.span);
}

pub fn walk_labeled_action<V: VisitMut + ?Sized>(visitor: &mut V, action: &mut LabeledAction) {
    // See `walk_labeled_predicate` — the assignment is a formula-model tree,
    // whose spans the lowering already lifts. Only the structural span is
    // visited; a shifter reaching a lifted span would move it a second time.
    visit_optional_span(visitor, &mut action.span);
}

pub fn walk_variant<V: VisitMut + ?Sized>(visitor: &mut V, variant: &mut Variant) {
    // See `walk_labeled_predicate` — the expression is a formula-model
    // tree; only the structural span is visited.
    visit_optional_span(visitor, &mut variant.span);
}

pub fn walk_event<V: VisitMut + ?Sized>(visitor: &mut V, event: &mut Event) {
    for named in event.refines.iter_mut().chain(&mut event.parameters) {
        visitor.visit_named_element(named);
    }
    for predicate in event
        .guards
        .iter_mut()
        .chain(&mut event.with)
        .chain(&mut event.witnesses)
    {
        visitor.visit_labeled_predicate(predicate);
    }
    for action in &mut event.actions {
        visitor.visit_labeled_action(action);
    }
    visit_optional_span(visitor, &mut event.span);
    visit_optional_span(visitor, &mut event.name_span);
}

pub fn walk_initialisation<V: VisitMut + ?Sized>(
    visitor: &mut V,
    initialisation: &mut InitialisationEvent,
) {
    for action in &mut initialisation.actions {
        visitor.visit_labeled_action(action);
    }
    for predicate in initialisation
        .with
        .iter_mut()
        .chain(&mut initialisation.witnesses)
    {
        visitor.visit_labeled_predicate(predicate);
    }
    visit_optional_span(visitor, &mut initialisation.span);
    visit_optional_span(visitor, &mut initialisation.name_span);
}

pub fn walk_clause_region<V: VisitMut + ?Sized>(visitor: &mut V, clause: &mut ClauseRegion) {
    visitor.visit_span(&mut clause.span);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_components;

    struct SpanShifter {
        delta: usize,
        visited: usize,
    }

    impl VisitMut for SpanShifter {
        fn visit_span(&mut self, span: &mut Span) {
            self.visited += 1;
            span.shift(self.delta);
        }
    }

    #[test]
    fn default_recursion_reaches_component_structural_spans() {
        let source = "CONTEXT C\nSETS\n  S\nCONSTANTS\n  k\nAXIOMS\n  @a1 k ∈ S\nEND\n\
                      MACHINE M\nSEES C\nVARIABLES\n  x\nINVARIANTS\n  @i1 x ∈ ℤ\n\
                      EVENTS\nEVENT INITIALISATION\nTHEN\n  @a1 x ≔ 0\nEND\n\
                      EVENT tick\nANY p\nWHERE\n  @g1 p ∈ ℤ\nTHEN\n  @a1 x ≔ p\nEND\nEND\n";
        let mut components = parse_components(source).expect("components parse");
        let original_machine_span = components[1].span().expect("machine span");
        let original_invariant_span = {
            let Component::Machine(machine) = &components[1] else {
                panic!("expected machine");
            };
            machine.invariants[0].span.expect("invariant span")
        };
        let mut shifter = SpanShifter {
            delta: 11,
            visited: 0,
        };
        for component in &mut components {
            shifter.visit_component(component);
        }

        assert!(
            shifter.visited >= 20,
            "visited only {} spans",
            shifter.visited
        );
        assert_eq!(
            components[1].span(),
            Some(Span {
                start: original_machine_span.start + 11,
                end: original_machine_span.end + 11,
            })
        );
        // Formula-model spans are immutable and out of this visitor's
        // reach; the labeled element's own span is structural and moves.
        let Component::Machine(machine) = &components[1] else {
            panic!("expected machine");
        };
        assert_eq!(
            machine.invariants[0].span,
            Some(Span {
                start: original_invariant_span.start + 11,
                end: original_invariant_span.end + 11,
            })
        );
    }
}
