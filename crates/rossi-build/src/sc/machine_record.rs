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
//! - **Event children** travel along the *extended-event* chain (a
//!   separate edge, label-matched to the parent machine's events). Each
//!   [`EventDecl`] carries `inherited: Option<Rc<EventDecl>>` so passes
//!   can walk that chain in typed form, without round-tripping through
//!   `<scGuard predicate="…">` strings. Rendering copies guards, parameters,
//!   and actions from the parent's retained checked event, preserving their
//!   database identities without replaying the typed chain.

use std::collections::HashMap;
use std::rc::Rc;

use rossi::{ActionBody, EventStatus, Expression, Predicate};

use crate::handles::HandleUri;
use crate::normalize::{
    canonical_action, canonical_expression, canonical_typed_assignment, canonical_typed_expression,
    canonical_typed_predicate,
};
use crate::type_env::TypeEnv;
use crate::xml_out::{Element, RodinNameGenerator, attr, in_tag, tag};
use rossi::formula::Type;

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
    /// Machine name. Read through [`super::CheckedMachine::name`].
    pub name: String,
    /// Output `.bcm` filename. Read through
    /// [`super::CheckedMachine::output_filename`].
    pub output_filename: String,
    /// The machine's type environment (variables + seen constants). Read
    /// through [`super::CheckedMachine::env`].
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
    /// Variants in declaration order; several form a lexicographic order.
    pub variants: Vec<VariantDecl>,
    /// Events in emission order: INITIALISATION first when present,
    /// then ordinary events in source order. `Rc`-shared so the
    /// per-label lookup table on [`super::CheckedMachine`] can hand
    /// out the same decl that descendants extend.
    pub events: Vec<Rc<EventDecl>>,

    /// Transitively-refined ancestor names, oldest first. Read through
    /// [`super::CheckedMachine::ancestors`].
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
    /// Position of this invariant in the *raw* machine's `invariants`
    /// list — see [`super::context_record::AxiomDecl::source_index`].
    pub source_index: usize,
    /// Predicate AST as parsed. Retained so downstream passes do not
    /// need to re-parse the XML representation.
    pub predicate: Predicate,
    /// The fully typed formula-model form; see
    /// [`super::context_record::AxiomDecl::typed`]. The emitted
    /// predicate attribute is rendered from this.
    pub typed: rossi::formula::Predicate,
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
    /// The source label; `vrn` when the variant was unlabeled.
    pub label: String,
    /// Expression AST as parsed. Retained so downstream passes do not
    /// need to re-parse the XML representation, and the render-time
    /// fallback when `typed` is absent.
    pub expression: Expression,
    /// The fully typed formula-model form; `None` when the variant was
    /// kept despite referencing unknown identifiers (`usable = false`).
    pub typed: Option<rossi::formula::Expression>,
    pub source: HandleUri,
}

// ---------------------------------------------------------------------
// Event-scoped decls
// ---------------------------------------------------------------------

/// Event convergence, ranked `Ordinary` (weakest) through `Anticipated`
/// to `Convergent` (strongest); every static-check downgrade moves toward
/// `Ordinary`. The declaration order carries the ranking — `derive(Ord)`
/// depends on it.
///
/// The numeric `code` written to `org.eventb.core.convergence` is a
/// *separate* mapping that does not follow the ranking: `Ordinary` → `0`,
/// `Convergent` → `1`, `Anticipated` → `2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Convergence {
    Ordinary,
    Anticipated,
    Convergent,
}

impl Convergence {
    /// The convergence declared on an AST event; an absent status is
    /// ordinary.
    #[must_use]
    pub fn from_status(status: Option<EventStatus>) -> Self {
        match status {
            Some(EventStatus::Convergent) => Self::Convergent,
            Some(EventStatus::Anticipated) => Self::Anticipated,
            Some(EventStatus::Ordinary) | None => Self::Ordinary,
        }
    }

    /// The code emitted for the `org.eventb.core.convergence` attribute.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Ordinary => "0",
            Self::Convergent => "1",
            Self::Anticipated => "2",
        }
    }
}

#[derive(Debug, Clone)]
pub struct EventDecl {
    pub label: String,
    pub convergence: Convergence,
    pub extended: bool,
    pub accurate: bool,
    pub source: HandleUri,
    /// Refined abstract events in source order; several entries mean
    /// this event merges them.
    pub refines: Vec<RefinesEventDecl>,
    /// Own parameters, alphabetically sorted (Rodin's emission order).
    pub parameters: Vec<ParameterDecl>,
    /// Own guards, in source order.
    pub guards: Vec<GuardDecl>,
    /// Effective actions in render order: the inherited chain's actions
    /// (when `extended`) followed by this event's own, in source order.
    /// Unlike guards/parameters (spliced from `inherited` at render time),
    /// actions are materialised here so accuracy and the INITIALISATION
    /// repair pass read one list.
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
    /// Position of this guard in the *raw* event's `guards` list — see
    /// [`super::context_record::AxiomDecl::source_index`].
    pub source_index: usize,
    /// Predicate AST as parsed. Retained so downstream passes do not
    /// need to re-parse the XML representation.
    pub predicate: Predicate,
    /// The fully typed formula-model form; see
    /// [`super::context_record::AxiomDecl::typed`].
    pub typed: rossi::formula::Predicate,
    pub is_theorem: bool,
    pub source: HandleUri,
}

#[derive(Debug, Clone)]
pub struct ActionDecl {
    pub label: String,
    /// Position of this action in the *raw* event's `actions` list — see
    /// [`super::context_record::AxiomDecl::source_index`].
    pub source_index: usize,
    /// Action AST. Read in `machine/mod.rs` (via `lhs_variables`) to
    /// find the LHS variables an inherited INITIALISATION action
    /// assigns when deciding extended-event scope, and the render-time
    /// fallback when `typed` is absent.
    pub action: ActionBody,
    /// The fully typed formula-model form; `None` for `skip`, which
    /// has no assignment to rebuild.
    pub typed: Option<rossi::formula::Assignment>,
    pub source: HandleUri,
}

#[derive(Debug, Clone)]
pub struct WitnessDecl {
    pub label: String,
    /// Predicate AST as parsed. Retained so downstream passes do not
    /// need to re-parse the XML representation.
    pub predicate: Predicate,
    /// The fully typed formula-model form; see
    /// [`super::context_record::AxiomDecl::typed`].
    pub typed: rossi::formula::Predicate,
    pub source: HandleUri,
}

// ---------------------------------------------------------------------
// Chain helpers
// ---------------------------------------------------------------------

impl EventDecl {
    /// Actions declared by this event, excluding materialised inherited ones.
    pub fn own_actions(&self) -> &[ActionDecl] {
        let inherited_count = self
            .inherited
            .as_deref()
            .map_or(0, |parent| parent.actions.len());
        &self.actions[inherited_count..]
    }

    /// Walk the `self.inherited` chain root-first: the oldest ancestor
    /// first, the immediate parent last. This event itself is *not*
    /// included, so a caller wanting the whole chain appends it. Useful
    /// both for rendering inherited buckets and for collecting inherited
    /// typing axioms.
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

    /// Every parameter visible to this event: the inherited chain
    /// (root-first, populated only when the event is `extended`)
    /// followed by own. A name re-listed along the chain is kept once
    /// — it denotes the same parameter, so the types agree.
    ///
    /// Downstream passes use it to rebuild the event-local scope (see
    /// [`super::CheckedMachine::event_env`]); extended events inherit
    /// their scope from these declarations' solved types.
    pub fn chain_parameters(&self) -> Vec<&ParameterDecl> {
        let mut out: Vec<&ParameterDecl> = Vec::new();
        for ancestor in self.chain_root_first() {
            for p in &ancestor.parameters {
                if !out.iter().any(|q| q.name == p.name) {
                    out.push(p);
                }
            }
        }
        for p in &self.parameters {
            if !out.iter().any(|q| q.name == p.name) {
                out.push(p);
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
pub(crate) struct RenderedMachine {
    pub(crate) root: Element,
    pub(crate) own_invariants: Vec<Rc<Element>>,
    pub(crate) event_elems: HashMap<String, Rc<Element>>,
}

pub(crate) fn render_machine_root(
    record: &MachineRecord,
    accurate: bool,
    internal_contexts: &[Rc<Element>],
    inherited_invariants: &[Rc<Element>],
    inherited_events: Option<&HashMap<String, Rc<Element>>>,
) -> RenderedMachine {
    let mut names = RodinNameGenerator::default();
    let mut root = Element::new(tag::SC_MACHINE_FILE)
        .attr_bool(attr::ACCURATE, accurate)
        .attr(attr::CONFIGURATION, record.configuration.clone());
    let mut own_invariants = Vec::with_capacity(record.invariants.len());
    let mut event_elems = HashMap::with_capacity(record.events.len());

    if let Some(rm) = &record.refines {
        root.push(names.generated(|name| render_refines_machine(rm, name)));
    }
    for s in &record.sees {
        root.push(names.generated(|name| render_sees_context(s, name)));
    }
    // Hoisted internal-contexts and inherited-invariants are
    // pre-rendered and `Rc`-shared with their producing
    // CheckedContext / CheckedMachine, so this is Rc::clone.
    for ic in internal_contexts {
        root.push(names.retained(ic.clone()));
    }
    for el in inherited_invariants {
        root.push(names.retained(el.clone()));
    }
    for inv in &record.invariants {
        let element = names.generated(|name| render_invariant(inv, name));
        own_invariants.push(Rc::clone(&element));
        root.push(element);
    }
    for v in &record.variables {
        root.push(names.retained(Rc::new(render_variable(v))));
    }
    for va in &record.variants {
        root.push(names.generated(|name| render_variant(va, name)));
    }
    for e in &record.events {
        let inherited_event = e
            .inherited
            .as_deref()
            .and_then(|parent| inherited_events?.get(&parent.label).map(Rc::as_ref));
        let element = names.generated(|name| render_event(e, name, inherited_event));
        event_elems.insert(e.label.clone(), Rc::clone(&element));
        root.push(element);
    }
    RenderedMachine {
        root,
        own_invariants,
        event_elems,
    }
}

fn render_refines_machine(rm: &RefinesMachineDecl, internal_name: String) -> Element {
    Element::new(tag::SC_REFINES_MACHINE)
        .attr(attr::NAME, internal_name)
        .attr(attr::SC_TARGET, rm.sc_target.clone())
        .attr(attr::SOURCE, rm.source.as_str())
}

fn render_sees_context(s: &SeesContextDecl, internal_name: String) -> Element {
    Element::new(tag::SC_SEES_CONTEXT)
        .attr(attr::NAME, internal_name)
        .attr(attr::SC_TARGET, s.sc_target.clone())
        .attr(attr::SOURCE, s.source.as_str())
}

fn render_invariant(inv: &InvariantDecl, internal_name: String) -> Element {
    Element::new(tag::SC_INVARIANT)
        .attr(attr::NAME, internal_name)
        .attr(attr::LABEL, inv.label.clone())
        .attr(attr::PREDICATE, canonical_typed_predicate(&inv.typed))
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

fn render_variant(va: &VariantDecl, internal_name: String) -> Element {
    let expression = match &va.typed {
        Some(typed) => canonical_typed_expression(typed),
        // An unusable variant (unknown identifier) has no typed form;
        // its text is a plain rendering of the parse.
        None => canonical_expression(&va.expression),
    };
    Element::new(tag::SC_VARIANT)
        .attr(attr::NAME, internal_name)
        .attr(attr::EXPRESSION, expression)
        .attr(attr::LABEL, va.label.clone())
        .attr(attr::SOURCE, va.source.as_str())
}

/// Render an event in Rodin's module order.
///
/// Extended events copy their parent's guards, actions, and parameters with
/// their existing internal names before creating local children.
fn render_event(
    ev: &EventDecl,
    internal_name: String,
    inherited_event: Option<&Element>,
) -> Element {
    let mut names = RodinNameGenerator::default();
    let mut scev = Element::new(tag::SC_EVENT)
        .attr(attr::NAME, internal_name)
        .attr_bool(attr::ACCURATE, ev.accurate)
        .attr(attr::CONVERGENCE, ev.convergence.code())
        .attr_bool(attr::EXTENDED, ev.extended)
        .attr(attr::LABEL, ev.label.clone())
        .attr(attr::SOURCE, ev.source.as_str());

    for re in &ev.refines {
        scev.push(names.generated(|name| render_refines_event(re, name)));
    }

    if let Some(parent_element) = inherited_event {
        for copied_tag in [tag::SC_GUARD, tag::SC_ACTION, tag::SC_PARAMETER] {
            for child in parent_element
                .children
                .iter()
                .filter(|child| child.tag == copied_tag)
            {
                scev.push(names.retained(child.clone()));
            }
        }
    }

    for g in &ev.guards {
        scev.push(names.generated(|name| render_guard(g, name)));
    }
    for p in &ev.parameters {
        scev.push(names.retained(Rc::new(render_parameter(p))));
    }

    for a in ev.own_actions() {
        scev.push(names.generated(|name| render_action(a, name)));
    }

    for w in &ev.witnesses {
        scev.push(names.generated(|name| render_witness(w, name)));
    }

    scev
}

fn render_refines_event(re: &RefinesEventDecl, internal_name: String) -> Element {
    Element::new(tag::SC_REFINES_EVENT)
        .attr(attr::NAME, internal_name)
        .attr(attr::SC_TARGET, re.sc_target.clone())
        .attr(attr::SOURCE, re.source.as_str())
}

fn render_guard(g: &GuardDecl, internal_name: String) -> Element {
    Element::new(tag::SC_GUARD)
        .attr(attr::NAME, internal_name)
        .attr(attr::LABEL, g.label.clone())
        .attr(attr::PREDICATE, canonical_typed_predicate(&g.typed))
        .attr(attr::SOURCE, g.source.as_str())
        .attr_bool(attr::THEOREM, g.is_theorem)
}

fn render_parameter(p: &ParameterDecl) -> Element {
    Element::new(tag::SC_PARAMETER)
        .attr(attr::NAME, p.name.clone())
        .attr(attr::SOURCE, p.source.as_str())
        .attr(attr::TYPE, p.ty.to_rodin_canonical())
}

fn render_action(a: &ActionDecl, internal_name: String) -> Element {
    let assignment = match &a.typed {
        Some(typed) => canonical_typed_assignment(typed),
        // `skip` has no assignment to rebuild; render the parse.
        None => canonical_action(&a.action),
    };
    Element::new(tag::SC_ACTION)
        .attr(attr::NAME, internal_name)
        .attr(attr::ASSIGNMENT, assignment)
        .attr(attr::LABEL, a.label.clone())
        .attr(attr::SOURCE, a.source.as_str())
}

fn render_witness(w: &WitnessDecl, internal_name: String) -> Element {
    Element::new(tag::SC_WITNESS)
        .attr(attr::NAME, internal_name)
        .attr(attr::LABEL, w.label.clone())
        .attr(attr::PREDICATE, canonical_typed_predicate(&w.typed))
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
            variants: vec![],
            events: vec![],
            ancestors: vec![],
        }
    }

    #[test]
    fn render_root_emits_configuration_and_accurate() {
        let r = empty_record();
        let root = render_machine_root(&r, true, &[], &[], None).root;
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
            source_index: 0,
            typed: rossi::parse_predicate_str("⊤").unwrap(),
            predicate: rossi::parse_predicate_str("⊤").unwrap(),
            is_theorem: false,
            source: mk_uri().child("org.eventb.core.invariant", "inv1"),
        });
        r.variables.push(VariableDecl {
            name: "x".into(),
            ty: Type::Int,
            source: mk_uri().child("org.eventb.core.variable", "x"),
            is_abstract: false,
            is_concrete: true,
        });
        let root = render_machine_root(&r, true, &[], &[], None).root;
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
            convergence: Convergence::Ordinary,
            extended: false,
            accurate: true,
            source: mk_uri(),
            refines: Vec::new(),
            parameters: vec![],
            guards: vec![],
            actions: vec![],
            witnesses: vec![],
            inherited: None,
        });
        let parent = Rc::new(EventDecl {
            label: "e".into(),
            convergence: Convergence::Ordinary,
            extended: true,
            accurate: true,
            source: mk_uri(),
            refines: Vec::new(),
            parameters: vec![],
            guards: vec![],
            actions: vec![],
            witnesses: vec![],
            inherited: Some(Rc::clone(&grandparent)),
        });
        let own = EventDecl {
            label: "e".into(),
            convergence: Convergence::Ordinary,
            extended: true,
            accurate: true,
            source: mk_uri(),
            refines: Vec::new(),
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
