//! Default-recursing mutable traversal over the Event-B AST.
//!
//! Override only the node kinds a transform needs, and call the corresponding
//! `walk_*` function when the transform should continue into that node's
//! children. The defaults recurse through the complete AST.

use super::{
    Action, ActionKind, ClauseRegion, Component, Context, Event, Expression, ExpressionKind,
    FileMetadata, Ident, IdentPattern, InitialisationEvent, LabeledAction, LabeledPredicate,
    Machine, NamedElement, Predicate, PredicateKind, SetDeclaration, Span, TypedIdentifier,
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

    fn visit_set_declaration(&mut self, set: &mut SetDeclaration) {
        walk_set_declaration(self, set);
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

    fn visit_event(&mut self, event: &mut Event) {
        walk_event(self, event);
    }

    fn visit_initialisation(&mut self, initialisation: &mut InitialisationEvent) {
        walk_initialisation(self, initialisation);
    }

    fn visit_expression(&mut self, expression: &mut Expression) {
        walk_expression(self, expression);
    }

    fn visit_predicate(&mut self, predicate: &mut Predicate) {
        walk_predicate(self, predicate);
    }

    fn visit_action(&mut self, action: &mut Action) {
        walk_action(self, action);
    }

    fn visit_typed_identifier(&mut self, identifier: &mut TypedIdentifier) {
        walk_typed_identifier(self, identifier);
    }

    fn visit_ident_pattern(&mut self, pattern: &mut IdentPattern) {
        walk_ident_pattern(self, pattern);
    }

    fn visit_ident(&mut self, ident: &mut Ident) {
        walk_ident(self, ident);
    }

    fn visit_clause_region(&mut self, clause: &mut ClauseRegion) {
        walk_clause_region(self, clause);
    }

    fn visit_file_metadata(&mut self, _metadata: &mut FileMetadata) {}

    fn visit_span(&mut self, _span: &mut Span) {}
}

fn visit_optional_span<V: VisitMut + ?Sized>(visitor: &mut V, span: &mut Option<Span>) {
    if let Some(span) = span {
        visitor.visit_span(span);
    }
}

pub fn walk_component<V: VisitMut + ?Sized>(visitor: &mut V, component: &mut Component) {
    match component {
        Component::Context(context) => visitor.visit_context(context),
        Component::Machine(machine) => visitor.visit_machine(machine),
    }
}

pub fn walk_context<V: VisitMut + ?Sized>(visitor: &mut V, context: &mut Context) {
    for set in &mut context.sets {
        visitor.visit_set_declaration(set);
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
    if let Some(variant) = &mut machine.variant {
        visitor.visit_expression(variant);
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

pub fn walk_set_declaration<V: VisitMut + ?Sized>(visitor: &mut V, set: &mut SetDeclaration) {
    visit_optional_span(visitor, set.span_mut());
}

pub fn walk_named_element<V: VisitMut + ?Sized>(visitor: &mut V, element: &mut NamedElement) {
    visit_optional_span(visitor, &mut element.span);
}

pub fn walk_labeled_predicate<V: VisitMut + ?Sized>(
    visitor: &mut V,
    predicate: &mut LabeledPredicate,
) {
    visitor.visit_predicate(&mut predicate.predicate);
    visit_optional_span(visitor, &mut predicate.span);
}

pub fn walk_labeled_action<V: VisitMut + ?Sized>(visitor: &mut V, action: &mut LabeledAction) {
    visitor.visit_action(&mut action.action);
    visit_optional_span(visitor, &mut action.span);
}

pub fn walk_event<V: VisitMut + ?Sized>(visitor: &mut V, event: &mut Event) {
    for parameter in &mut event.parameters {
        visitor.visit_named_element(parameter);
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
    visit_optional_span(visitor, &mut event.refines_span);
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

pub fn walk_expression<V: VisitMut + ?Sized>(visitor: &mut V, expression: &mut Expression) {
    visit_optional_span(visitor, &mut expression.span);
    match &mut expression.kind {
        ExpressionKind::Integer(_)
        | ExpressionKind::Identifier(_)
        | ExpressionKind::AtomicBuiltin(_)
        | ExpressionKind::True
        | ExpressionKind::False
        | ExpressionKind::EmptySet
        | ExpressionKind::Naturals
        | ExpressionKind::Naturals1
        | ExpressionKind::Integers
        | ExpressionKind::BoolType => {}
        ExpressionKind::SetEnumeration(expressions) => {
            for expression in expressions {
                visitor.visit_expression(expression);
            }
        }
        ExpressionKind::SetComprehension {
            identifiers,
            predicate,
            expression,
        } => {
            for identifier in identifiers {
                visitor.visit_typed_identifier(identifier);
            }
            visitor.visit_predicate(predicate);
            if let Some(expression) = expression {
                visitor.visit_expression(expression);
            }
        }
        ExpressionKind::SetBuilder {
            member_expression,
            predicate,
        } => {
            visitor.visit_expression(member_expression);
            visitor.visit_predicate(predicate);
        }
        ExpressionKind::RelationalImage { relation, set } => {
            visitor.visit_expression(relation);
            visitor.visit_expression(set);
        }
        ExpressionKind::QuantifiedUnion {
            identifiers,
            predicate,
            expression,
        }
        | ExpressionKind::QuantifiedInter {
            identifiers,
            predicate,
            expression,
        } => {
            for identifier in identifiers {
                visitor.visit_typed_identifier(identifier);
            }
            visitor.visit_predicate(predicate);
            visitor.visit_expression(expression);
        }
        ExpressionKind::Lambda {
            pattern,
            predicate,
            expression,
        } => {
            visitor.visit_ident_pattern(pattern);
            visitor.visit_predicate(predicate);
            visitor.visit_expression(expression);
        }
        ExpressionKind::Binary { left, right, .. } => {
            visitor.visit_expression(left);
            visitor.visit_expression(right);
        }
        ExpressionKind::Unary { operand, .. } => visitor.visit_expression(operand),
        ExpressionKind::FunctionApplication { function, argument } => {
            visitor.visit_expression(function);
            visitor.visit_expression(argument);
        }
        ExpressionKind::BuiltinApplication { argument, .. } => {
            visitor.visit_expression(argument);
        }
        ExpressionKind::Bool(predicate) => visitor.visit_predicate(predicate),
    }
}

pub fn walk_predicate<V: VisitMut + ?Sized>(visitor: &mut V, predicate: &mut Predicate) {
    visit_optional_span(visitor, &mut predicate.span);
    match &mut predicate.kind {
        PredicateKind::True | PredicateKind::False => {}
        PredicateKind::Comparison { left, right, .. } => {
            visitor.visit_expression(left);
            visitor.visit_expression(right);
        }
        PredicateKind::Not(predicate) => visitor.visit_predicate(predicate),
        PredicateKind::Logical { left, right, .. } => {
            visitor.visit_predicate(left);
            visitor.visit_predicate(right);
        }
        PredicateKind::Quantified {
            identifiers,
            predicate,
            ..
        } => {
            for identifier in identifiers {
                visitor.visit_typed_identifier(identifier);
            }
            visitor.visit_predicate(predicate);
        }
        PredicateKind::Application {
            function,
            arguments,
        } => {
            visitor.visit_ident(function);
            for argument in arguments {
                visitor.visit_expression(argument);
            }
        }
        PredicateKind::BuiltinApplication { arguments, .. } => {
            for argument in arguments {
                visitor.visit_expression(argument);
            }
        }
    }
}

pub fn walk_action<V: VisitMut + ?Sized>(visitor: &mut V, action: &mut Action) {
    visit_optional_span(visitor, &mut action.span);
    match &mut action.kind {
        ActionKind::Skip => {}
        ActionKind::Assignment {
            variables,
            expressions,
        } => {
            for variable in variables {
                visitor.visit_ident(variable);
            }
            for expression in expressions {
                visitor.visit_expression(expression);
            }
        }
        ActionKind::BecomesIn { variables, set } => {
            for variable in variables {
                visitor.visit_ident(variable);
            }
            visitor.visit_expression(set);
        }
        ActionKind::BecomesSuchThat {
            variables,
            predicate,
        } => {
            for variable in variables {
                visitor.visit_ident(variable);
            }
            visitor.visit_predicate(predicate);
        }
    }
}

pub fn walk_typed_identifier<V: VisitMut + ?Sized>(
    visitor: &mut V,
    identifier: &mut TypedIdentifier,
) {
    visit_optional_span(visitor, &mut identifier.span);
    if let Some(expression) = &mut identifier.type_expr {
        visitor.visit_expression(expression);
    }
}

pub fn walk_ident_pattern<V: VisitMut + ?Sized>(visitor: &mut V, pattern: &mut IdentPattern) {
    match pattern {
        IdentPattern::Identifier(identifier) => visitor.visit_typed_identifier(identifier),
        IdentPattern::Maplet(left, right) => {
            visitor.visit_ident_pattern(left);
            visitor.visit_ident_pattern(right);
        }
    }
}

pub fn walk_ident<V: VisitMut + ?Sized>(visitor: &mut V, ident: &mut Ident) {
    visit_optional_span(visitor, &mut ident.span);
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
    fn default_recursion_reaches_component_and_formula_spans() {
        let source = "CONTEXT C\nSETS\n  S\nCONSTANTS\n  k\nAXIOMS\n  @a1 k ∈ S\nEND\n\
                      MACHINE M\nSEES C\nVARIABLES\n  x\nINVARIANTS\n  @i1 x ∈ ℤ\n\
                      EVENTS\nEVENT INITIALISATION\nTHEN\n  @a1 x ≔ 0\nEND\n\
                      EVENT tick\nANY p\nWHERE\n  @g1 p ∈ ℤ\nTHEN\n  @a1 x ≔ p\nEND\nEND\n";
        let mut components = parse_components(source).expect("components parse");
        let original_machine_span = components[1].span().expect("machine span");
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
        let Component::Machine(machine) = &components[1] else {
            panic!("expected machine");
        };
        assert!(machine.events[0].actions[0].action.span.unwrap().start > 11);
        assert!(machine.invariants[0].predicate.span.unwrap().start > 11);
    }
}
