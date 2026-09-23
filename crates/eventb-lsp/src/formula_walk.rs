//! Collect identifier occurrences from a component's formulas using the
//! formula model's occurrence walker, so find-references and rename resolve
//! usage sites from the tree (with correct binder scoping) instead of
//! scanning text.
//!
//! Event parameters are locals introduced at the event level: the model
//! reads a parameter use as a free identifier, so the walker filters by
//! name against the event's parameter list — a same-named global symbol is
//! correctly shadowed within that event.

use std::ops::ControlFlow;

use rossi::ast::Span;
use rossi::formula::occurrences::{self, DeclRef, Occurrence, Resolution};
use rossi::{Component, Event, Expression, Predicate};

/// The role of an identifier occurrence, re-exported so consumers keep one
/// vocabulary with the walker.
pub use rossi::formula::occurrences::Role as IdentRole;

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

/// Event-scope locals (`ANY` parameters): unprimed name plus declaration span.
type Locals = Vec<(String, Option<Span>)>;

/// The innermost local binding a free occurrence of `name` resolves to.
fn local_binding(locals: &[(String, Option<Span>)], name: &str) -> Option<Option<Span>> {
    locals
        .iter()
        .rev()
        .find(|(local, _)| local == name)
        .map(|(_, span)| *span)
}

/// An occurrence visitor that additionally tracks the event-scope locals in
/// effect while [`drive`] traverses a component. The driver clears the
/// locals by setting an empty list.
trait WalkVisitor: occurrences::OccurrenceVisitor {
    fn set_locals(&mut self, _locals: Locals) {}
}

struct Collector<'a> {
    target: &'a str,
    locals: Locals,
    hits: Vec<Hit>,
}

impl occurrences::OccurrenceVisitor for Collector<'_> {
    fn visit(&mut self, occ: Occurrence<'_>) -> ControlFlow<()> {
        if canonical(occ.name) != self.target {
            return ControlFlow::Continue(());
        }
        let Some(span) = occ.span else {
            return ControlFlow::Continue(());
        };
        // A bound occurrence resolves to its declaration — except an
        // after-state read, which stays an occurrence of the global
        // variable. A free occurrence may still be shadowed by an event
        // parameter.
        let scope = if occ.is_after_state_read() {
            Scope::Free
        } else {
            match occ.resolution {
                Resolution::Bound { .. } => Scope::Bound(occ.bound_decl().and_then(|d| d.span)),
                Resolution::Free => match local_binding(&self.locals, self.target) {
                    Some(declaration) => Scope::Bound(declaration),
                    None => Scope::Free,
                },
            }
        };
        self.hits.push(Hit {
            span,
            role: occ.role,
            scope,
        });
        ControlFlow::Continue(())
    }
}

impl WalkVisitor for Collector<'_> {
    fn set_locals(&mut self, locals: Locals) {
        self.locals = locals;
    }
}

/// Drive the walker over every formula in `component`, with each event's
/// parameters in effect as locals for its body. The single traversal both
/// the targeted and the collect-all consumers share.
fn drive<V: WalkVisitor>(component: &Component, v: &mut V) {
    match component {
        Component::Context(ctx) => {
            for ax in &ctx.axioms {
                let _ = occurrences::walk_predicate(&ax.predicate, &mut Vec::new(), v);
            }
        }
        Component::Machine(m) => {
            for inv in &m.invariants {
                let _ = occurrences::walk_predicate(&inv.predicate, &mut Vec::new(), v);
            }
            for variant in &m.variants {
                let _ = occurrences::walk_expression(&variant.expression, &mut Vec::new(), v);
            }
            if let Some(init) = &m.initialisation {
                for lp in init.with.iter().chain(&init.witnesses) {
                    let _ = occurrences::walk_predicate(&lp.predicate, &mut Vec::new(), v);
                }
                for la in &init.actions {
                    walk_action_body(&la.action, v);
                }
            }
            for event in &m.events {
                let locals: Locals = event
                    .parameters
                    .iter()
                    .map(|p| (p.name.clone(), p.span))
                    .collect();
                v.set_locals(locals);
                walk_event_body(event, v);
                v.set_locals(Locals::new());
            }
        }
    }
}

/// Walk an action body's assignment, if any (`skip` has no formulas).
fn walk_action_body<V: occurrences::OccurrenceVisitor>(body: &rossi::ActionBody, v: &mut V) {
    if let Some(assignment) = body.assignment() {
        let _ = occurrences::walk_assignment(assignment, &mut Vec::new(), v);
    }
}

/// Walk every formula in `component` and collect every occurrence of `target`
/// (canonicalised for the `x'` form) with its role and scope.
pub fn collect_in_component(component: &Component, target: &str) -> Vec<Hit> {
    let mut c = Collector {
        target,
        locals: Vec::new(),
        hits: Vec::new(),
    };
    drive(component, &mut c);
    c.hits
}

/// Stops at the first identifier occurrence that names the target (so the walk
/// short-circuits via [`ControlFlow::Break`]).
struct MentionVisitor<'a> {
    target: &'a str,
    found: bool,
}

impl occurrences::OccurrenceVisitor for MentionVisitor<'_> {
    fn visit(&mut self, occ: Occurrence<'_>) -> ControlFlow<()> {
        // A binder *declaration* of the same name (`∀ x · …`) shadows the symbol
        // rather than referencing it, so it is not a mention of the global `x`.
        if occ.role != IdentRole::Binder && canonical(occ.name) == self.target {
            self.found = true;
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    }
}

/// True if any identifier occurrence in `predicate` names `name` (after-state
/// `name'` matches the unprimed `name`). The single tree traversal — the same
/// one that powers find-references — so "which clauses mention this symbol"
/// cannot drift from "where is this symbol used".
pub fn predicate_mentions(predicate: &Predicate, name: &str) -> bool {
    let mut v = MentionVisitor {
        target: name,
        found: false,
    };
    let _ = occurrences::walk_predicate(predicate, &mut Vec::new(), &mut v);
    v.found
}

/// True if any identifier occurrence in `expression` names `name` (after-state
/// `name'` matches the unprimed `name`). See [`predicate_mentions`].
pub fn expression_mentions(expression: &Expression, name: &str) -> bool {
    let mut v = MentionVisitor {
        target: name,
        found: false,
    };
    let _ = occurrences::walk_expression(expression, &mut Vec::new(), &mut v);
    v.found
}

/// Walk an event's guards, `with` / witness predicates, and actions with the
/// visitor's current locals in effect.
fn walk_event_body<V: occurrences::OccurrenceVisitor>(event: &Event, v: &mut V) {
    for lp in event
        .guards
        .iter()
        .chain(&event.with)
        .chain(&event.witnesses)
    {
        let _ = occurrences::walk_predicate(&lp.predicate, &mut Vec::new(), v);
    }
    for la in &event.actions {
        walk_action_body(&la.action, v);
    }
}

/// Walk one event's body **without** its parameters as locals, collecting
/// occurrences of `target`. Used to find references to a parameter: its uses
/// are free at event scope (an inner quantifier rebinding the name shadows it).
pub fn collect_in_event_body(event: &Event, target: &str) -> Vec<Hit> {
    let mut c = Collector {
        target,
        locals: Vec::new(),
        hits: Vec::new(),
    };
    walk_event_body(event, &mut c);
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

/// The binder-local resolution of a cursor sitting on a formula identifier.
///
/// Produced when the cursor is on a binder declaration or a use bound by one — a
/// quantifier (`∀`/`∃`), `λ`, set comprehension, quantified `⋃`/`⋂`, or an event
/// parameter. This is the single source of truth for "what scope does this
/// cursor bind to": rename rewrites [`spans`](Self::spans), go-to-definition
/// jumps to [`declaration`](Self::declaration), and find-references reports
/// [`spans`](Self::spans) as the in-scope locations.
#[derive(Debug, Clone)]
pub struct BoundResolution {
    /// The binder's declaration span — the go-to-definition target and the
    /// declaration entry of the reference set. `None` only for a binder the
    /// walker records without a span.
    pub declaration: Option<Span>,
    /// The binder declaration plus every use it binds, within this one component.
    pub spans: Vec<Span>,
    /// True when the binding binder is an event `ANY` parameter (a local at
    /// event scope) rather than a formula binder. Lets the definition /
    /// references / hover providers keep an event parameter on its own symbol
    /// path, while rename treats every binder uniformly.
    pub is_event_parameter: bool,
}

/// Resolve the occurrence of `identifier` at byte `offset` against the binders in
/// `component`.
///
/// Returns the binder-local scope when the cursor sits on a binder declaration or
/// a use bound by one; `None` when the occurrence is free (the name resolves to a
/// component-level / inherited symbol) or there is no occurrence at `offset`. One
/// walk of the component serves both the cursor lookup and the occurrence set.
pub fn resolve_bound_at_offset(
    component: &Component,
    identifier: &str,
    offset: usize,
) -> Option<BoundResolution> {
    resolve_bound_from_hits(
        &collect_in_component(component, identifier),
        component,
        offset,
    )
}

/// Classify the cursor at `offset` against `hits` (a [`collect_in_component`] run
/// for one identifier).
///
/// Split out of [`resolve_bound_at_offset`] so a caller that already holds the
/// walk's hits — rename, which also needs the free-use set on the fall-through —
/// reuses them instead of walking the component a second time.
pub(crate) fn resolve_bound_from_hits(
    hits: &[Hit],
    component: &Component,
    offset: usize,
) -> Option<BoundResolution> {
    // Copy the cursor occurrence out (role / span / scope are all `Copy`). A
    // caret just past the occurrence, where a double click or the end of
    // typing leaves it, names it too.
    let (role, cursor_span, scope) = hits
        .iter()
        .find(|h| h.span.contains(offset))
        .or_else(|| hits.iter().find(|h| h.span.end == offset))
        .map(|h| (h.role, h.span, h.scope))?;

    // The binder this cursor is scoped to.
    let binder_span = match (role, scope) {
        // Cursor on a binder declaration: the binder introduced here (its own
        // span, even when an outer binder of the same name shadows it).
        (IdentRole::Binder, _) => cursor_span,
        // Cursor on a use bound by a binder of the same name: scope to it.
        (_, Scope::Bound(Some(span))) => span,
        // Bound by a binder with no recorded span (the parser normally records
        // one): only the cursor token is safe to report.
        (_, Scope::Bound(None)) => {
            return Some(BoundResolution {
                declaration: None,
                spans: vec![cursor_span],
                is_event_parameter: false,
            });
        }
        // Free use, or a declaration site with no formula occurrence: not
        // binder-local — the name resolves to a component-level symbol.
        (_, Scope::Free) => return None,
    };

    let mut spans: Vec<Span> = hits
        .iter()
        .filter(|h| {
            // The binder's own declaration, plus every *use* it binds. An inner
            // binder of the same name re-declares (shadows) it: the inner
            // declaration introduces a new binding, not a use of this one, so
            // it is excluded — renaming this binder must leave the shadowing
            // inner binder and its body untouched.
            (h.role == IdentRole::Binder && h.span == binder_span)
                || (h.role != IdentRole::Binder && h.scope == Scope::Bound(Some(binder_span)))
        })
        .map(|h| h.span)
        .collect();
    // Event parameters are locals, not formula occurrences, so add the
    // declaration span explicitly.
    if !spans.contains(&binder_span) {
        spans.push(binder_span);
    }

    Some(BoundResolution {
        declaration: Some(binder_span),
        spans,
        is_event_parameter: is_event_parameter_span(component, binder_span),
    })
}

/// Whether `span` is the declaration span of an event `ANY` parameter (a local
/// at event scope), as opposed to a formula binder
/// (`∀`/`∃`/`λ`/comprehension/`⋃`/`⋂`).
fn is_event_parameter_span(component: &Component, span: Span) -> bool {
    let Component::Machine(machine) = component else {
        return false;
    };
    machine
        .events
        .iter()
        .flat_map(|event| &event.parameters)
        .any(|parameter| parameter.span == Some(span))
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
    locals: Locals,
    occurrences: Vec<AnyOccurrence>,
}

impl occurrences::OccurrenceVisitor for AllCollector {
    fn visit(&mut self, occ: Occurrence<'_>) -> ControlFlow<()> {
        if let Some(span) = occ.span {
            // An after-state read bound by its assignment's primed
            // declaration is an occurrence of the global variable, not a
            // local (see `Collector`).
            let bound = (matches!(occ.resolution, Resolution::Bound { .. })
                && !occ.is_after_state_read())
                || local_binding(&self.locals, canonical(occ.name)).is_some();
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

impl WalkVisitor for AllCollector {
    fn set_locals(&mut self, locals: Locals) {
        self.locals = locals;
    }
}

/// Every identifier occurrence in `component`'s formulas, in document order
/// modulo the walker's traversal.
pub fn collect_all_occurrences(component: &Component) -> Vec<AnyOccurrence> {
    let mut c = AllCollector {
        locals: Vec::new(),
        occurrences: Vec::new(),
    };
    drive(component, &mut c);
    c.occurrences
}

/// Collects the formula binders whose body scope contains a byte offset, for
/// offering them in completion where the cursor often sits in whitespace with no
/// identifier occurrence to resolve.
struct ScopeCollector {
    offset: usize,
    names: Vec<String>,
}

impl occurrences::OccurrenceVisitor for ScopeCollector {
    fn visit(&mut self, _occ: Occurrence<'_>) -> ControlFlow<()> {
        ControlFlow::Continue(())
    }

    fn enter_scope(&mut self, frame: &[DeclRef], scope_span: Option<Span>) -> ControlFlow<()> {
        // Inclusive end: a cursor at the very end of a half-typed body (the
        // common completion case) still counts as inside the scope.
        if let Some(span) = scope_span
            && span.start <= self.offset
            && self.offset <= span.end
        {
            for binder in frame {
                if !self.names.contains(&binder.name) {
                    self.names.push(binder.name.clone());
                }
            }
        }
        ControlFlow::Continue(())
    }
}

impl WalkVisitor for ScopeCollector {}

/// The names of every formula binder (`∀`/`∃`/`λ`/set-comprehension/`⋃`/`⋂`) in
/// scope at byte `offset`, de-duplicated. One walk of the component keeps the
/// binders whose body span contains the cursor (nested bodies nest, so an offset
/// deep inside collects every enclosing binder). Event `ANY` parameters are
/// locals rather than reported scopes, so they never appear here — they are
/// offered separately, scoped to their event.
pub fn binders_in_scope_at_offset(component: &Component, offset: usize) -> Vec<String> {
    let mut c = ScopeCollector {
        offset,
        names: Vec::new(),
    };
    drive(component, &mut c);
    c.names
}

/// Finds the frame a binder declaration belongs to, and the body it covers.
struct BinderScope {
    declaration: Span,
    found: Option<(Vec<String>, Option<Span>)>,
}

impl occurrences::OccurrenceVisitor for BinderScope {
    fn visit(&mut self, _occ: Occurrence<'_>) -> ControlFlow<()> {
        ControlFlow::Continue(())
    }

    fn enter_scope(&mut self, frame: &[DeclRef], scope_span: Option<Span>) -> ControlFlow<()> {
        if frame.iter().any(|d| d.span == Some(self.declaration)) {
            let names = frame.iter().map(|d| d.name.clone()).collect();
            self.found = Some((names, scope_span));
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    }
}

impl WalkVisitor for BinderScope {}

/// Whether renaming the formula binder declared at `declaration` to `name`
/// would capture something: `name` is another binder of the same frame, or
/// occurs anywhere in the body the binder scopes.
pub(crate) fn binder_scope_mentions(component: &Component, declaration: Span, name: &str) -> bool {
    let mut scope = BinderScope {
        declaration,
        found: None,
    };
    drive(component, &mut scope);
    let Some((frame, body)) = scope.found else {
        return false;
    };
    frame.iter().any(|binder| binder == name)
        || body.is_some_and(|body| {
            collect_in_component(component, name)
                .iter()
                .any(|hit| body.start <= hit.span.start && hit.span.end <= body.end)
        })
}

/// Declaration span of a set / constant / variable named `name`, if this
/// component declares it. (Event parameters are declared per event; see the
/// rename / references parameter paths.)
pub fn declaration_span(component: &Component, name: &str) -> Option<Span> {
    match component {
        Component::Context(ctx) => ctx
            .sets
            .iter()
            .find(|s| s.name == name)
            .and_then(|s| s.span)
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

    // ---- predicate_mentions / expression_mentions --------------------------

    #[test]
    fn predicate_mentions_only_used_identifiers() {
        let src = "MACHINE m\nVARIABLES\nx\ny\nINVARIANTS\n@i1 x ∈ ℕ\nEND\n";
        let component = parse(src).expect("parses");
        let Component::Machine(m) = &component else {
            panic!("machine");
        };
        let inv = &m.invariants[0].predicate;
        assert!(predicate_mentions(inv, "x"));
        assert!(!predicate_mentions(inv, "y"));
    }

    #[test]
    fn expression_mentions_every_operand() {
        let src = "MACHINE m\nVARIABLES\nx\ny\nVARIANT\ny − x\nEND\n";
        let component = parse(src).expect("parses");
        let Component::Machine(m) = &component else {
            panic!("machine");
        };
        let variant = &m.variants.first().expect("variant").expression;
        assert!(expression_mentions(variant, "x"));
        assert!(expression_mentions(variant, "y"));
        assert!(!expression_mentions(variant, "z"));
    }

    #[test]
    fn predicate_mentions_recognises_a_predicate_call_name() {
        // The *name* of a user-defined predicate application is a reference too:
        // `@a1 P(0)` mentions the constant `P`. (The bespoke hover matcher this
        // replaced inspected only the arguments and missed the call name.)
        let src = "CONTEXT c\nCONSTANTS\nP\nAXIOMS\n@a1 P(0)\nEND\n";
        let component = parse(src).expect("parses");
        let Component::Context(ctx) = &component else {
            panic!("context");
        };
        let axm = &ctx.axioms[0].predicate;
        assert!(predicate_mentions(axm, "P"));
    }

    #[test]
    fn predicate_mentions_ignores_a_shadowing_binder_declaration() {
        // `y` occurs only as a quantifier binder; the body uses `n`, not `y`. A
        // binder declaration shadows the global `y` and is not a mention of it.
        let src = "MACHINE m\nVARIABLES\ny\nn\nINVARIANTS\n@i1 ∀ y · n ∈ ℕ\nEND\n";
        let component = parse(src).expect("parses");
        let Component::Machine(m) = &component else {
            panic!("machine");
        };
        let inv = &m.invariants[0].predicate;
        assert!(
            !predicate_mentions(inv, "y"),
            "binder decl is not a mention"
        );
        assert!(
            predicate_mentions(inv, "n"),
            "the body use of n is a mention"
        );
    }

    // ---- resolve_bound_at_offset (the binder-scope SSOT) -------------------

    const SHADOWED: &str = "MACHINE m\nVARIABLES\nx\nINVARIANTS\n@i1 x ∈ ℕ\n@i2 ∀ x · x > 0\nEND\n";

    #[test]
    fn cursor_on_quantifier_binder_resolves_its_own_scope() {
        let component = parse(SHADOWED).expect("parses");
        // Cursor on the `∀ x` binder declaration.
        let offset = SHADOWED.find("∀ x").unwrap() + "∀ ".len();
        let res = resolve_bound_at_offset(&component, "x", offset).expect("bound");

        assert!(!res.is_event_parameter, "a formula binder, not a parameter");
        assert_eq!(texts(SHADOWED, &[res.declaration.unwrap()]), vec!["x"]);
        // The binder declaration and the bound use, both inside @i2.
        assert_eq!(texts(SHADOWED, &res.spans), vec!["x", "x"]);
        let i2 = SHADOWED.find("@i2").unwrap();
        assert!(
            res.spans.iter().all(|s| s.start >= i2),
            "scope stays within the quantifier, never the @i1 global use"
        );
    }

    #[test]
    fn cursor_on_bound_use_resolves_to_its_binder() {
        let component = parse(SHADOWED).expect("parses");
        // Cursor on the bound use `x` after the `·`.
        let offset = SHADOWED.find("· x").unwrap() + "· ".len();
        let res = resolve_bound_at_offset(&component, "x", offset).expect("bound");

        assert_eq!(res.spans.len(), 2, "binder declaration + the one bound use");
        let i2 = SHADOWED.find("@i2").unwrap();
        assert!(res.spans.iter().all(|s| s.start >= i2));
    }

    #[test]
    fn cursor_right_after_a_bound_use_resolves_to_its_binder() {
        let component = parse(SHADOWED).expect("parses");
        // The caret a double click or the end of typing leaves: just past the
        // bound use, before the space.
        let offset = SHADOWED.find("· x").unwrap() + "· x".len();
        let res = resolve_bound_at_offset(&component, "x", offset).expect("bound");

        assert_eq!(res.spans.len(), 2, "binder declaration + the one bound use");
        let i2 = SHADOWED.find("@i2").unwrap();
        assert!(res.spans.iter().all(|s| s.start >= i2));
    }

    #[test]
    fn cursor_on_free_global_use_is_not_bound() {
        let component = parse(SHADOWED).expect("parses");
        // Cursor on the free use `x` in @i1 (no enclosing binder of that name).
        let offset = SHADOWED.find("@i1 x").unwrap() + "@i1 ".len();
        assert!(resolve_bound_at_offset(&component, "x", offset).is_none());
    }

    #[test]
    fn nested_same_name_binder_excludes_the_inner_declaration() {
        // `∀ x · (∃ x · x > 0)`: the inner `∃ x` re-declares (shadows) `x`. A
        // cursor on the OUTER binder scopes to the outer `x` alone — it has no
        // body uses (the inner shadows them), and the inner declaration belongs
        // to a *different* binding, so it must not be captured here.
        let src = "MACHINE m\nINVARIANTS\n@i1 ∀ x · (∃ x · x > 0)\nEND\n";
        let component = parse(src).expect("parses");

        let outer = src.find("∀ x").unwrap() + "∀ ".len();
        let res = resolve_bound_at_offset(&component, "x", outer).expect("bound");
        assert_eq!(
            res.spans.len(),
            1,
            "outer binder alone, not the inner `∃ x`: {:?}",
            texts(src, &res.spans)
        );
        assert_eq!(res.declaration, Some(res.spans[0]));

        // A cursor on the inner binder scopes to the inner `∃ x` plus its body use.
        let inner = src.find("∃ x").unwrap() + "∃ ".len();
        let inner_res = resolve_bound_at_offset(&component, "x", inner).expect("bound");
        assert_eq!(
            inner_res.spans.len(),
            2,
            "inner binder + its bound use: {:?}",
            texts(src, &inner_res.spans)
        );
    }

    #[test]
    fn cursor_on_event_parameter_is_flagged() {
        let src = "MACHINE m\nVARIABLES\nv\nEVENTS\nEVENT e\nANY\nq\nWHERE\n@grd1 q > 0\nTHEN\n@act1 v ≔ q\nEND\nEND\n";
        let component = parse(src).expect("parses");
        // Cursor on the use of the ANY parameter `q` in the guard.
        let offset = src.find("@grd1 q").unwrap() + "@grd1 ".len();
        let res = resolve_bound_at_offset(&component, "q", offset).expect("bound");
        assert!(
            res.is_event_parameter,
            "an event ANY parameter is flagged so callers keep its symbol path"
        );
    }

    // ---- binders_in_scope_at_offset (the completion scope query) -----------

    #[test]
    fn binders_in_scope_inside_a_quantifier_body() {
        // Inside the `∀ x · x > 0` body, `x` is in scope.
        let component = parse(SHADOWED).expect("parses");
        let inside = SHADOWED.find("· x").unwrap() + "· ".len();
        assert_eq!(
            binders_in_scope_at_offset(&component, inside),
            vec!["x".to_string()]
        );
    }

    #[test]
    fn no_binders_in_scope_outside_a_body() {
        // The free use `x` in @i1 sits in no binder body.
        let component = parse(SHADOWED).expect("parses");
        let outside = SHADOWED.find("@i1 x").unwrap();
        assert!(binders_in_scope_at_offset(&component, outside).is_empty());
    }

    #[test]
    fn nested_binders_collect_the_whole_stack() {
        // `∀ x · (∃ y · x > y)`: deep inside the inner body both binders apply.
        let src = "MACHINE m\nINVARIANTS\n@i1 ∀ x · (∃ y · x > y)\nEND\n";
        let component = parse(src).expect("parses");
        let inner = src.rfind('y').unwrap();
        let mut names = binders_in_scope_at_offset(&component, inner);
        names.sort();
        assert_eq!(names, vec!["x".to_string(), "y".to_string()]);
    }
}
