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
pub(crate) fn canonical(name: &str) -> &str {
    name.strip_suffix('\'').unwrap_or(name)
}

/// True if `span` slices to `name` (or its after-state form `name'`) in `text`.
///
/// Guards the find-references / rename consumers against a span that does not
/// map to the served document — a deeper recovery bug could leave a formula
/// span relative to its region rather than absolute — so a stale span can never
/// panic a slice or produce an edit over unrelated text.
pub fn span_matches(text: &str, span: Span, name: &str) -> bool {
    span.end <= text.len()
        && text.is_char_boundary(span.start)
        && text.is_char_boundary(span.end)
        && canonical(&text[span.start..span.end]) == name
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

/// Drive the shared walker over every formula in `component`, seeding each
/// event's parameters as binders. The single traversal both the targeted and
/// the collect-all consumers share.
fn drive<V: IdentVisitor>(component: &Component, v: &mut V) {
    let mut binders: Vec<Binder> = Vec::new();
    match component {
        Component::Context(ctx) => {
            for ax in &ctx.axioms {
                let _ = walk::walk_predicate(&ax.predicate, &mut binders, v);
            }
        }
        Component::Machine(m) => {
            for inv in &m.invariants {
                let _ = walk::walk_predicate(&inv.predicate, &mut binders, v);
            }
            if let Some(variant) = &m.variant {
                let _ = walk::walk_expression(variant, &mut binders, v);
            }
            if let Some(init) = &m.initialisation {
                for lp in init.with.iter().chain(&init.witnesses) {
                    let _ = walk::walk_predicate(&lp.predicate, &mut binders, v);
                }
                for la in &init.actions {
                    let _ = walk::walk_action(&la.action, &mut binders, v);
                }
            }
            for event in &m.events {
                collect_in_event(event, &mut binders, v);
            }
        }
    }
}

/// Walk every formula in `component` and collect every occurrence of `target`
/// (canonicalised for the `x'` form) with its role and scope.
pub fn collect_in_component(component: &Component, target: &str) -> Vec<Hit> {
    let mut c = Collector {
        target,
        hits: Vec::new(),
    };
    drive(component, &mut c);
    c.hits
}

/// Walk an event's guards, `with` / witness predicates, and actions with the
/// given binders in scope.
fn walk_event_body<V: IdentVisitor>(event: &Event, binders: &mut Vec<Binder>, v: &mut V) {
    for lp in event
        .guards
        .iter()
        .chain(&event.with)
        .chain(&event.witnesses)
    {
        let _ = walk::walk_predicate(&lp.predicate, binders, v);
    }
    for la in &event.actions {
        let _ = walk::walk_action(&la.action, binders, v);
    }
}

/// Walk a single event's body with `outer` binders already in scope, seeding the
/// event's own parameters on top.
fn collect_in_event<V: IdentVisitor>(event: &Event, outer: &mut Vec<Binder>, c: &mut V) {
    let depth = outer.len();
    outer.extend(event.parameters.iter().map(|p| Binder {
        name: p.name.clone(),
        span: p.span,
    }));
    walk_event_body(event, outer, c);
    outer.truncate(depth);
}

/// Walk one event's body **without** seeding its parameters, collecting
/// occurrences of `target`. Used to find references to a parameter: its uses
/// are free at event scope (an inner quantifier rebinding the name shadows it).
pub fn collect_in_event_body(event: &Event, target: &str) -> Vec<Hit> {
    let mut c = Collector {
        target,
        hits: Vec::new(),
    };
    walk_event_body(event, &mut Vec::new(), &mut c);
    c.hits
}

/// The binder-shadowing exclusion rule shared by find-references and rename: a
/// free use / write target / predicate-call name of the component-level symbol,
/// dropping binder declarations and binder-shadowed uses.
pub(crate) fn free_spans(hits: Vec<Hit>) -> Vec<Span> {
    hits.into_iter()
        .filter(|h| h.scope == Scope::Free && h.role != IdentRole::Binder)
        .map(|h| h.span)
        .collect()
}

/// Spans of every **free** occurrence of `target` in the component — uses,
/// write targets, and predicate-call names that resolve to the component-level
/// symbol (binder declarations and binder-shadowed uses are excluded). This is
/// the reference set for a global variable / constant / set.
pub fn free_occurrence_spans(component: &Component, target: &str) -> Vec<Span> {
    free_spans(collect_in_component(component, target))
}

/// Spans of every free occurrence of a parameter `target` within `event`.
pub fn parameter_occurrence_spans(event: &Event, target: &str) -> Vec<Span> {
    free_spans(collect_in_event_body(event, target))
}

/// Any identifier occurrence in a component's formulas (used to colour every
/// formula identifier as a semantic token).
#[derive(Debug, Clone)]
pub struct AnyOccurrence {
    /// The identifier text, verbatim (a primed read keeps its `'`).
    pub name: String,
    /// Source span of the occurrence.
    pub span: Span,
    /// What this occurrence is.
    pub role: IdentRole,
    /// True if bound by an enclosing binder of the same name (a local / parameter).
    pub bound: bool,
}

struct AllCollector {
    occurrences: Vec<AnyOccurrence>,
}

impl IdentVisitor for AllCollector {
    fn visit(&mut self, occ: IdentOccurrence<'_>) -> ControlFlow<()> {
        if let Some(span) = occ.span {
            let base = canonical(occ.name);
            let bound = occ.binders.iter().any(|b| b.name == base);
            self.occurrences.push(AnyOccurrence {
                name: occ.name.to_string(),
                span,
                role: occ.role,
                bound,
            });
        }
        ControlFlow::Continue(())
    }
}

/// Every identifier occurrence in `component`'s formulas, in document order
/// modulo the walker's traversal.
pub fn collect_all_occurrences(component: &Component) -> Vec<AnyOccurrence> {
    let mut c = AllCollector {
        occurrences: Vec::new(),
    };
    drive(component, &mut c);
    c.occurrences
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
