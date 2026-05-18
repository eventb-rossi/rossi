//! Pure-data model of a statically-checked machine.
//!
//! [`MachineRecord`] is the typed result of running [`super::machine`]
//! on a `.bum`. It mirrors [`super::context_record::ContextRecord`]:
//! own decls only, no XML. The `.bcm` is a *rendering* of this record
//! (see [`render_machine_root`]).
//!
//! # Inheritance shape
//!
//! Two axes of inheritance are encoded differently:
//!
//! - **Invariants** travel along the machine's REFINES chain. The
//!   render layer takes the parent's full closure (`Vec<Rc<Element>>`)
//!   as an external argument; we don't store the parent record on
//!   every child. The `Rc` wrapping makes the per-element clone cheap.
//! - **Event children** (parameters / guards / actions) travel along
//!   the *extended-event* chain (a separate edge, label-matched to
//!   the parent machine's events). Each [`EventDecl`] carries
//!   `inherited: Option<Rc<EventDecl>>` so the render and parameter-
//!   inference passes can walk that chain in typed form, without
//!   round-tripping through `<scGuard predicate="…">` strings.

use std::rc::Rc;

use rossi::{Action, Predicate};

use crate::handles::HandleUri;
use crate::type_env::TypeEnv;
use crate::types::Type;
use crate::xml_out::{Element, attr, in_tag, tag};

// ---------------------------------------------------------------------
// Top-level record
// ---------------------------------------------------------------------

/// The typed record produced by checking one `.bum`.
///
/// Some metadata fields (`name`, `output_filename`, `env`, `ancestors`)
/// duplicate state already cached on [`super::CheckedMachine`] for the
/// downstream code paths that need them; the record carries them too
/// so it remains a self-describing typed snapshot.
#[derive(Debug, Clone)]
pub struct MachineRecord {
    #[allow(dead_code)]
    pub name: String,
    #[allow(dead_code)]
    pub output_filename: String,
    #[allow(dead_code)]
    pub env: TypeEnv,
    /// `org.eventb.core.fwd` unless the source file overrides it.
    pub configuration: String,

    pub refines: Option<RefinesMachineDecl>,
    pub sees: Vec<SeesContextDecl>,
    /// Every variable visible at the end of checking, in alphabetical
    /// order. `is_abstract` marks the inherited subset.
    pub variables: Vec<VariableDecl>,
    /// Own invariants only — the parent closure travels via
    /// [`super::CheckedMachine::invariant_elems`].
    pub invariants: Vec<InvariantDecl>,
    pub variant: Option<VariantDecl>,
    /// Events in emission order: INITIALISATION first when present,
    /// then ordinary events in source order. `Rc`-shared so the
    /// per-label lookup table on [`super::CheckedMachine`] can hand
    /// out the same decl that descendants extend.
    pub events: Vec<Rc<EventDecl>>,

    /// Transitively-refined ancestor names, oldest first.
    #[allow(dead_code)]
    pub ancestors: Vec<String>,
}

// ---------------------------------------------------------------------
// File-scoped decls
// ---------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RefinesMachineDecl {
    pub parent_name: String,
    pub sc_target: String,
    pub source: HandleUri,
}

#[derive(Debug, Clone)]
pub struct SeesContextDecl {
    pub name: String,
    pub sc_target: String,
    pub source: HandleUri,
}

#[derive(Debug, Clone)]
pub struct InvariantDecl {
    pub label: String,
    /// Parsed predicate AST — kept for descendants and for downstream
    /// analyses. Today only the canonical string is rendered.
    #[allow(dead_code)]
    pub predicate: Predicate,
    pub predicate_canonical: String,
    pub is_theorem: bool,
    pub source: HandleUri,
}

#[derive(Debug, Clone)]
pub struct VariableDecl {
    pub name: String,
    pub ty: Type,
    pub source: HandleUri,
    pub is_abstract: bool,
    pub is_concrete: bool,
}

#[derive(Debug, Clone)]
pub struct VariantDecl {
    pub label: &'static str,
    pub expression_canonical: String,
    pub source: HandleUri,
}

// ---------------------------------------------------------------------
// Event-scoped decls
// ---------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct EventDecl {
    pub label: String,
    pub convergence: &'static str,
    pub extended: bool,
    pub accurate: bool,
    pub source: HandleUri,
    pub refines: Option<RefinesEventDecl>,
    /// Own parameters, alphabetically sorted (Rodin's emission order).
    pub parameters: Vec<ParameterDecl>,
    /// Own guards, in source order.
    pub guards: Vec<GuardDecl>,
    /// Own actions, in source order.
    pub actions: Vec<ActionDecl>,
    /// Own witnesses (`with` and `witnesses` clauses, merged).
    pub witnesses: Vec<WitnessDecl>,
    /// Parent in the extended-event chain. `None` unless this event is
    /// `extended=true` with a same-labelled parent. Each ancestor in
    /// turn carries its own `inherited`, so a single chain walk yields
    /// the full closure.
    pub inherited: Option<Rc<EventDecl>>,
}

#[derive(Debug, Clone)]
pub struct RefinesEventDecl {
    pub abstract_label: String,
    pub sc_target: String,
    pub source: HandleUri,
}

#[derive(Debug, Clone)]
pub struct ParameterDecl {
    pub name: String,
    pub ty: Type,
    pub source: HandleUri,
}

#[derive(Debug, Clone)]
pub struct GuardDecl {
    pub label: String,
    pub predicate: Predicate,
    pub predicate_canonical: String,
    pub is_theorem: bool,
    pub source: HandleUri,
}

#[derive(Debug, Clone)]
pub struct ActionDecl {
    pub label: String,
    /// The original action AST, kept so future passes can re-analyse
    /// without re-parsing the canonical string.
    #[allow(dead_code)]
    pub action: Action,
    pub canonical: String,
    pub source: HandleUri,
}

#[derive(Debug, Clone)]
pub struct WitnessDecl {
    pub label: String,
    /// Original predicate AST. Witnesses reference dropped abstract
    /// names which intentionally aren't resolved against env, so
    /// keeping the AST lets downstream tools perform their own
    /// analyses.
    #[allow(dead_code)]
    pub predicate: Predicate,
    pub predicate_canonical: String,
    pub source: HandleUri,
}

// ---------------------------------------------------------------------
// Chain helpers
// ---------------------------------------------------------------------

impl EventDecl {
    /// Walk `self.inherited` chain root-first (oldest ancestor first,
    /// own EventDecl last). Useful both for rendering inherited
    /// buckets and for collecting inherited typing axioms.
    pub fn chain_root_first(&self) -> Vec<&EventDecl> {
        let mut out: Vec<&EventDecl> = Vec::new();
        let mut cur = self.inherited.as_deref();
        while let Some(p) = cur {
            out.push(p);
            cur = p.inherited.as_deref();
        }
        out.reverse();
        out
    }

    /// Every guard predicate visible to this event's parameter
    /// inference: the inherited chain (in chain order, when extended)
    /// followed by own.
    pub fn typing_guard_predicates(&self) -> Vec<&Predicate> {
        let mut out: Vec<&Predicate> = Vec::new();
        if self.extended {
            for ancestor in self.chain_root_first() {
                for g in &ancestor.guards {
                    out.push(&g.predicate);
                }
            }
        }
        for g in &self.guards {
            out.push(&g.predicate);
        }
        out
    }

    /// Inherited parameter bindings, root-first then deduplicated by
    /// later occurrence (matches today's `Rc<Vec<(String, Type)>>`
    /// semantics, where own-types appended). Used by descendants
    /// when building scope.
    pub fn inherited_parameter_types(&self) -> Vec<(String, Type)> {
        let mut out: Vec<(String, Type)> = Vec::new();
        let mut cur = self.inherited.as_deref();
        // Walk ancestors root-first by collecting then reversing.
        let mut chain: Vec<&EventDecl> = Vec::new();
        while let Some(p) = cur {
            chain.push(p);
            cur = p.inherited.as_deref();
        }
        chain.reverse();
        for ancestor in chain {
            for p in &ancestor.parameters {
                if !out.iter().any(|(n, _)| n == &p.name) {
                    out.push((p.name.clone(), p.ty.clone()));
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------------
// Rendering — record → XML Element
// ---------------------------------------------------------------------

/// Render the root `<scMachineFile>` element for `record`. Caller
/// supplies the externally-tracked pieces:
///
/// - `accurate`: aggregate of every per-element accuracy flag
///   collected during checking.
/// - `internal_contexts`: scInternalContext rows already rendered for
///   each transitively-seen context (in hoist order).
/// - `inherited_invariants`: parent-machine's full invariant closure,
///   pre-rendered to splice verbatim.
pub fn render_machine_root(
    record: &MachineRecord,
    accurate: bool,
    internal_contexts: &[Rc<Element>],
    inherited_invariants: &[Rc<Element>],
) -> Element {
    let mut root = Element::new(tag::SC_MACHINE_FILE)
        .attr_bool(attr::ACCURATE, accurate)
        .attr(attr::CONFIGURATION, record.configuration.clone());

    if let Some(rm) = &record.refines {
        root.push(render_refines_machine(rm));
    }
    for s in &record.sees {
        root.push(render_sees_context(s));
    }
    // Hoisted internal-contexts and inherited-invariants are
    // pre-rendered and `Rc`-shared with their producing
    // CheckedContext / CheckedMachine, so this is Rc::clone.
    for ic in internal_contexts {
        root.push(ic.clone());
    }
    for el in inherited_invariants {
        root.push(el.clone());
    }
    for inv in &record.invariants {
        root.push(render_invariant(inv));
    }
    for v in &record.variables {
        root.push(render_variable(v));
    }
    if let Some(va) = &record.variant {
        root.push(render_variant(va));
    }
    for e in &record.events {
        root.push(render_event(e));
    }
    root
}

/// Render every own invariant in `record`. Used when emitting a child
/// machine's full invariant closure (parent's already-rendered
/// elements + this record's own).
pub fn render_own_invariants(record: &MachineRecord) -> Vec<Rc<Element>> {
    record
        .invariants
        .iter()
        .map(|inv| Rc::new(render_invariant(inv)))
        .collect()
}

fn render_refines_machine(rm: &RefinesMachineDecl) -> Element {
    Element::new(tag::SC_REFINES_MACHINE)
        .attr(attr::NAME, rm.parent_name.clone())
        .attr(attr::SC_TARGET, rm.sc_target.clone())
        .attr(attr::SOURCE, rm.source.as_str())
}

fn render_sees_context(s: &SeesContextDecl) -> Element {
    Element::new(tag::SC_SEES_CONTEXT)
        .attr(attr::NAME, s.name.clone())
        .attr(attr::SC_TARGET, s.sc_target.clone())
        .attr(attr::SOURCE, s.source.as_str())
}

fn render_invariant(inv: &InvariantDecl) -> Element {
    Element::new(tag::SC_INVARIANT)
        .attr(attr::NAME, inv.label.clone())
        .attr(attr::LABEL, inv.label.clone())
        .attr(attr::PREDICATE, inv.predicate_canonical.clone())
        .attr(attr::SOURCE, inv.source.as_str())
        .attr_bool(attr::THEOREM, inv.is_theorem)
}

fn render_variable(v: &VariableDecl) -> Element {
    Element::new(tag::SC_VARIABLE)
        .attr(attr::NAME, v.name.clone())
        .attr_bool(attr::ABSTRACT, v.is_abstract)
        .attr_bool(attr::CONCRETE, v.is_concrete)
        .attr(attr::SOURCE, v.source.as_str())
        .attr(attr::TYPE, v.ty.to_rodin_canonical())
}

fn render_variant(va: &VariantDecl) -> Element {
    Element::new(tag::SC_VARIANT)
        .attr(attr::NAME, va.label)
        .attr(attr::EXPRESSION, va.expression_canonical.clone())
        .attr(attr::LABEL, va.label)
        .attr(attr::SOURCE, va.source.as_str())
}

/// Render an event, splicing its inherited-event chain (when
/// `extended=true`) into each child bucket: guards first, then
/// parameters, then actions; ancestors before own at every level.
fn render_event(ev: &EventDecl) -> Element {
    let mut scev = Element::new(tag::SC_EVENT)
        .attr(attr::NAME, ev.label.clone())
        .attr_bool(attr::ACCURATE, ev.accurate)
        .attr(attr::CONVERGENCE, ev.convergence)
        .attr_bool(attr::EXTENDED, ev.extended)
        .attr(attr::LABEL, ev.label.clone())
        .attr(attr::SOURCE, ev.source.as_str());

    if let Some(re) = &ev.refines {
        scev.push(render_refines_event(re));
    }

    let inherited = if ev.extended {
        ev.chain_root_first()
    } else {
        Vec::new()
    };

    for ancestor in &inherited {
        for g in &ancestor.guards {
            scev.push(render_guard(g));
        }
    }
    for g in &ev.guards {
        scev.push(render_guard(g));
    }

    for ancestor in &inherited {
        for p in &ancestor.parameters {
            scev.push(render_parameter(p));
        }
    }
    for p in &ev.parameters {
        scev.push(render_parameter(p));
    }

    for ancestor in &inherited {
        for a in &ancestor.actions {
            scev.push(render_action(a));
        }
    }
    for a in &ev.actions {
        scev.push(render_action(a));
    }

    for w in &ev.witnesses {
        scev.push(render_witness(w));
    }

    scev
}

fn render_refines_event(re: &RefinesEventDecl) -> Element {
    Element::new(tag::SC_REFINES_EVENT)
        .attr(attr::NAME, re.abstract_label.clone())
        .attr(attr::SC_TARGET, re.sc_target.clone())
        .attr(attr::SOURCE, re.source.as_str())
}

fn render_guard(g: &GuardDecl) -> Element {
    Element::new(tag::SC_GUARD)
        .attr(attr::NAME, g.label.clone())
        .attr(attr::LABEL, g.label.clone())
        .attr(attr::PREDICATE, g.predicate_canonical.clone())
        .attr(attr::SOURCE, g.source.as_str())
        .attr_bool(attr::THEOREM, g.is_theorem)
}

fn render_parameter(p: &ParameterDecl) -> Element {
    Element::new(tag::SC_PARAMETER)
        .attr(attr::NAME, p.name.clone())
        .attr(attr::SOURCE, p.source.as_str())
        .attr(attr::TYPE, p.ty.to_rodin_canonical())
}

fn render_action(a: &ActionDecl) -> Element {
    Element::new(tag::SC_ACTION)
        .attr(attr::NAME, a.label.clone())
        .attr(attr::ASSIGNMENT, a.canonical.clone())
        .attr(attr::LABEL, a.label.clone())
        .attr(attr::SOURCE, a.source.as_str())
}

fn render_witness(w: &WitnessDecl) -> Element {
    Element::new(tag::SC_WITNESS)
        .attr(attr::NAME, w.label.clone())
        .attr(attr::LABEL, w.label.clone())
        .attr(attr::PREDICATE, w.predicate_canonical.clone())
        .attr(attr::SOURCE, w.source.as_str())
}

// Used by in-tag constants `in_tag::EVENT`, `in_tag::GUARD`, etc.
// in builders elsewhere; nothing exported from this module needs
// `in_tag` directly today.
#[allow(unused_imports)]
use in_tag as _;

#[cfg(test)]
mod tests {
    use super::*;
    use rossi::parse_predicate_str;

    fn mk_uri() -> HandleUri {
        HandleUri::root("proj", "M.bum", "org.eventb.core.machineFile", "M")
    }

    fn empty_record() -> MachineRecord {
        MachineRecord {
            name: "M".into(),
            output_filename: "M.bcm".into(),
            env: TypeEnv::new(),
            configuration: "org.eventb.core.fwd".into(),
            refines: None,
            sees: vec![],
            variables: vec![],
            invariants: vec![],
            variant: None,
            events: vec![],
            ancestors: vec![],
        }
    }

    #[test]
    fn render_root_emits_configuration_and_accurate() {
        let r = empty_record();
        let root = render_machine_root(&r, true, &[], &[]);
        assert_eq!(root.tag, tag::SC_MACHINE_FILE);
        let attrs: Vec<_> = root.attrs.iter().map(|(k, _)| k.as_str()).collect();
        assert!(attrs.contains(&attr::ACCURATE));
        assert!(attrs.contains(&attr::CONFIGURATION));
    }

    #[test]
    fn render_emits_in_canonical_order() {
        let mut r = empty_record();
        r.sees.push(SeesContextDecl {
            name: "Ctx".into(),
            sc_target: "/proj/Ctx.bcc|tag#Ctx".into(),
            source: mk_uri().child("org.eventb.core.seesContext", "Ctx"),
        });
        r.invariants.push(InvariantDecl {
            label: "inv1".into(),
            predicate: parse_predicate_str("⊤").unwrap(),
            predicate_canonical: "⊤".into(),
            is_theorem: false,
            source: mk_uri().child("org.eventb.core.invariant", "inv1"),
        });
        r.variables.push(VariableDecl {
            name: "x".into(),
            ty: Type::Integer,
            source: mk_uri().child("org.eventb.core.variable", "x"),
            is_abstract: false,
            is_concrete: true,
        });
        let root = render_machine_root(&r, true, &[], &[]);
        let tags: Vec<&str> = root.children.iter().map(|c| c.tag.as_str()).collect();
        assert_eq!(
            tags,
            vec![tag::SC_SEES_CONTEXT, tag::SC_INVARIANT, tag::SC_VARIABLE]
        );
    }

    #[test]
    fn event_chain_root_first_walks_oldest_to_youngest() {
        let grandparent = Rc::new(EventDecl {
            label: "e".into(),
            convergence: "0",
            extended: false,
            accurate: true,
            source: mk_uri(),
            refines: None,
            parameters: vec![],
            guards: vec![],
            actions: vec![],
            witnesses: vec![],
            inherited: None,
        });
        let parent = Rc::new(EventDecl {
            label: "e".into(),
            convergence: "0",
            extended: true,
            accurate: true,
            source: mk_uri(),
            refines: None,
            parameters: vec![],
            guards: vec![],
            actions: vec![],
            witnesses: vec![],
            inherited: Some(Rc::clone(&grandparent)),
        });
        let own = EventDecl {
            label: "e".into(),
            convergence: "0",
            extended: true,
            accurate: true,
            source: mk_uri(),
            refines: None,
            parameters: vec![],
            guards: vec![],
            actions: vec![],
            witnesses: vec![],
            inherited: Some(Rc::clone(&parent)),
        };
        let chain = own.chain_root_first();
        assert_eq!(chain.len(), 2);
        // Root-first ordering: grandparent (no inherited) precedes parent.
        assert!(chain[0].inherited.is_none());
        assert!(chain[1].inherited.is_some());
        assert!(std::ptr::eq(chain[0], grandparent.as_ref()));
        assert!(std::ptr::eq(chain[1], parent.as_ref()));
    }
}
