//! Collect identifier occurrences from a component's formulas using the shared
//! [`rossi::ast::walk`] walker, so find-references and rename resolve usage
//! sites from the AST (with correct binder scoping) instead of scanning text.
//!
//! Event parameters are modelled as binders introduced at the event level: a
//! usage inside the event is "bound" by the parameter, and a same-named global
//! symbol is therefore correctly shadowed within that event.

use std::ops::ControlFlow;

use rossi::ast::Span;
use rossi::ast::walk::{self, Binder, IdentOccurrence, IdentRole, IdentVisitor};
use rossi::{Component, Event};

/// How an occurrence of the target name resolves at its position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Not bound by any enclosing binder of the same name — a use of the
    /// component-level (global) symbol.
    Free,
    /// Bound by the innermost enclosing binder of the same name, identified by
    /// that binder's declaration span (`None` if the binder had no span).
    Bound(Option<Span>),
}

/// One occurrence of the target name found in a formula.
#[derive(Debug, Clone)]
pub struct Hit {
    /// Source span of the occurrence (a primed read `x'` keeps the full span).
    pub span: Span,
    /// Whether the occurrence reads, declares, writes, or calls the name.
    pub role: IdentRole,
    /// How the occurrence resolves (free vs bound by a binder of the same name).
    pub scope: Scope,
}

/// Strip a single trailing apostrophe so the after-state form `x'` is matched
/// against the unprimed declaration `x`.
fn canonical(name: &str) -> &str {
    name.strip_suffix('\'').unwrap_or(name)
}

struct Collector<'a> {
    target: &'a str,
    hits: Vec<Hit>,
}

impl IdentVisitor for Collector<'_> {
    fn visit(&mut self, occ: IdentOccurrence<'_>) -> ControlFlow<()> {
        if canonical(occ.name) != self.target {
            return ControlFlow::Continue(());
        }
        let Some(span) = occ.span else {
            return ControlFlow::Continue(());
        };
        // The innermost enclosing binder of the same (unprimed) name shadows the
        // global symbol; binder names are always unprimed.
        let scope = match occ.binders.iter().rev().find(|b| b.name == self.target) {
            Some(b) => Scope::Bound(b.span),
            None => Scope::Free,
        };
        self.hits.push(Hit {
            span,
            role: occ.role,
            scope,
        });
        ControlFlow::Continue(())
    }
}

/// Walk every formula in `component`, seeding each event's parameters as
/// binders, and collect every occurrence of `target` (canonicalised for the
/// `x'` form) with its role and scope.
pub fn collect_in_component(component: &Component, target: &str) -> Vec<Hit> {
    let mut c = Collector {
        target,
        hits: Vec::new(),
    };
    let mut binders: Vec<Binder> = Vec::new();
    match component {
        Component::Context(ctx) => {
            for ax in &ctx.axioms {
                let _ = walk::walk_predicate(&ax.predicate, &mut binders, &mut c);
            }
        }
        Component::Machine(m) => {
            for inv in &m.invariants {
                let _ = walk::walk_predicate(&inv.predicate, &mut binders, &mut c);
            }
            if let Some(variant) = &m.variant {
                let _ = walk::walk_expression(variant, &mut binders, &mut c);
            }
            if let Some(init) = &m.initialisation {
                for lp in init.with.iter().chain(&init.witnesses) {
                    let _ = walk::walk_predicate(&lp.predicate, &mut binders, &mut c);
                }
                for la in &init.actions {
                    let _ = walk::walk_action(&la.action, &mut binders, &mut c);
                }
            }
            for event in &m.events {
                collect_in_event(event, &mut binders, &mut c);
            }
        }
    }
    c.hits
}

/// Walk a single event's formulas with `outer` binders already in scope, seeding
/// the event's own parameters on top.
fn collect_in_event(event: &Event, outer: &mut Vec<Binder>, c: &mut Collector<'_>) {
    let depth = outer.len();
    outer.extend(event.parameters.iter().map(|p| Binder {
        name: p.name.clone(),
        span: p.span,
    }));
    for lp in event
        .guards
        .iter()
        .chain(&event.with)
        .chain(&event.witnesses)
    {
        let _ = walk::walk_predicate(&lp.predicate, outer, c);
    }
    for la in &event.actions {
        let _ = walk::walk_action(&la.action, outer, c);
    }
    outer.truncate(depth);
}

/// Walk one event's formulas **without** seeding its parameters, collecting
/// occurrences of `target`. Used to find references to a parameter: its uses
/// are free at event scope (an inner quantifier rebinding the name shadows it).
pub fn collect_in_event_body(event: &Event, target: &str) -> Vec<Hit> {
    let mut c = Collector {
        target,
        hits: Vec::new(),
    };
    let mut binders: Vec<Binder> = Vec::new();
    for lp in event
        .guards
        .iter()
        .chain(&event.with)
        .chain(&event.witnesses)
    {
        let _ = walk::walk_predicate(&lp.predicate, &mut binders, &mut c);
    }
    for la in &event.actions {
        let _ = walk::walk_action(&la.action, &mut binders, &mut c);
    }
    c.hits
}

/// Spans of every **free** occurrence of `target` in the component — uses,
/// write targets, and predicate-call names that resolve to the component-level
/// symbol (binder declarations and binder-shadowed uses are excluded). This is
/// the reference set for a global variable / constant / set.
pub fn free_occurrence_spans(component: &Component, target: &str) -> Vec<Span> {
    collect_in_component(component, target)
        .into_iter()
        .filter(|h| h.scope == Scope::Free && h.role != IdentRole::Binder)
        .map(|h| h.span)
        .collect()
}

/// Spans of every free occurrence of a parameter `target` within `event`.
pub fn parameter_occurrence_spans(event: &Event, target: &str) -> Vec<Span> {
    collect_in_event_body(event, target)
        .into_iter()
        .filter(|h| h.scope == Scope::Free && h.role != IdentRole::Binder)
        .map(|h| h.span)
        .collect()
}

/// Declaration span of a set / constant / variable named `name`, if this
/// component declares it. (Event parameters are declared per event; see the
/// rename / references parameter paths.)
pub fn declaration_span(component: &Component, name: &str) -> Option<Span> {
    match component {
        Component::Context(ctx) => ctx
            .sets
            .iter()
            .find(|s| s.name() == name)
            .and_then(|s| s.span())
            .or_else(|| {
                ctx.constants
                    .iter()
                    .find(|c| c.name == name)
                    .and_then(|c| c.span)
            }),
        Component::Machine(m) => m
            .variables
            .iter()
            .find(|v| v.name == name)
            .and_then(|v| v.span),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rossi::parse;

    fn texts<'a>(src: &'a str, spans: &[Span]) -> Vec<&'a str> {
        spans.iter().map(|s| &src[s.start..s.end]).collect()
    }

    #[test]
    fn binder_shadows_global_in_references() {
        // `x` is a machine variable; the inner `∀ x` rebinds it. A global
        // reference search for `x` must skip the bound declaration and its use.
        let src = "MACHINE m\nVARIABLES\nx\nINVARIANTS\n@i1 x > 0\n@i2 ∀ x · x > 0\nEND\n";
        let component = parse(src).expect("parses");
        let spans = free_occurrence_spans(&component, "x");
        assert_eq!(texts(src, &spans), vec!["x"], "only the free x in @i1");
    }

    #[test]
    fn primed_after_state_counts_as_the_variable() {
        let src = "MACHINE m\nVARIABLES\nx\nEVENTS\nEVENT e\nTHEN\n@a1 x :∣ x' = x + 1\nEND\nEND\n";
        let component = parse(src).expect("parses");
        let spans = free_occurrence_spans(&component, "x");
        // write target `x`, the primed `x'`, and the read `x` on the RHS.
        assert_eq!(texts(src, &spans), vec!["x", "x'", "x"]);
    }

    #[test]
    fn predicate_call_name_is_a_reference() {
        let src = "CONTEXT c\nCONSTANTS\nP\nAXIOMS\n@a1 P(0)\nEND\n";
        let component = parse(src).expect("parses");
        let spans = free_occurrence_spans(&component, "P");
        assert_eq!(texts(src, &spans), vec!["P"]);
    }

    #[test]
    fn variant_expression_usage_is_found() {
        let src = "MACHINE m\nVARIABLES\nv\nINVARIANTS\n@i1 v ∈ ℕ\nVARIANT\nv\nEND\n";
        let component = parse(src).expect("parses");
        let spans = free_occurrence_spans(&component, "v");
        // The invariant use and the VARIANT expression use.
        assert_eq!(texts(src, &spans), vec!["v", "v"]);
    }
}
