//! The checked model as document elements.
//!
//! The shape follows Rodin's checked files rather than the source: a machine
//! carries the closure of what is visible in it, not just what was written in
//! it. Its invariants include the ones it inherits, its events carry the full
//! chain of an extended event's guards and actions, and the contexts it sees
//! are listed transitively. A consumer that wants only what a machine wrote
//! can filter on the recorded owner; a consumer that wants the whole picture
//! already has it, and neither has to reimplement the checker's closure rules.

use std::collections::BTreeMap;

use rossi::formula::{Assignment, Expression, Predicate};

use crate::Diagnostic;
use crate::normalize;
use crate::sc::machine_record::{ActionDecl, Convergence, EventDecl};
use crate::sc_model::{CheckedContext, CheckedMachine, ScModel};

use super::formula::{AssignmentNode, Ctx, ExtensionInfo, Node, TypeNode};
use super::location::SpanDump;
use super::origin::{ComponentOrigin, Origin, Origins, is_synthesized};

/// An identifier declaration: a carrier set, constant or event parameter.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct IdentDump {
    pub name: String,
    /// The declared type, as a key into the document's type table.
    #[cfg_attr(feature = "serde", serde(rename = "type"))]
    pub ty: String,
    /// The Rodin handle of the checked declaration.
    pub source: String,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub span: Option<SpanDump>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub comment: Option<String>,
    /// The machine an inherited parameter was written in.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub inherited_from: Option<String>,
}

/// A machine variable.
///
/// There is no owner to record: the checked model keeps the visible set with
/// no note of which ancestor declared each name, and Rodin's checked file
/// says the same. `abstract` is the inheritance marker.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct VariableDump {
    pub name: String,
    #[cfg_attr(feature = "serde", serde(rename = "type"))]
    pub ty: String,
    pub source: String,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub span: Option<SpanDump>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub comment: Option<String>,
    /// Declared by a machine this one refines.
    #[cfg_attr(feature = "serde", serde(rename = "abstract"))]
    pub is_abstract: bool,
    /// Declared by this machine.
    #[cfg_attr(feature = "serde", serde(rename = "concrete"))]
    pub is_concrete: bool,
}

/// An axiom, invariant, guard or witness.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct PredicateDump {
    /// For a witness this is the identifier being witnessed, which for an
    /// after-state is a primed name, as in Rodin.
    pub label: String,
    /// Absent on a witness, which cannot be a theorem.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub theorem: Option<bool>,
    pub source: String,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub span: Option<SpanDump>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub comment: Option<String>,
    /// The machine an inherited invariant or guard was written in.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub inherited_from: Option<String>,
    /// The predicate as Rodin's checked file spells it, so a consumer can
    /// hand it to a Rodin parser and get the same formula back.
    pub text: String,
    pub predicate: Node,
}

/// A machine variant.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct VariantDump {
    pub label: String,
    pub source: String,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub span: Option<SpanDump>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub comment: Option<String>,
    pub text: String,
    /// Absent when the variant was kept despite naming something unknown, so
    /// there is no typed expression to report.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub expression: Option<Node>,
}

/// One action of an event.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ActionDump {
    pub label: String,
    pub source: String,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub span: Option<SpanDump>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub comment: Option<String>,
    /// The machine an inherited action was written in.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub inherited_from: Option<String>,
    pub text: String,
    /// Absent for `skip`, which has no assignment.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub assignment: Option<AssignmentNode>,
    /// The before-after predicate: the same action written as a relation
    /// between the states, with each after-state value a primed identifier.
    /// Absent for `skip`.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub ba: Option<Node>,
}

/// An event, with the whole chain an extended event inherits.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct EventDump {
    pub label: String,
    /// `ordinary`, `convergent` or `anticipated`.
    pub convergence: &'static str,
    /// Whether the event extends its abstract counterpart rather than
    /// restating it.
    pub extended: bool,
    /// Whether checking this event was complete. A false value means some
    /// part of it was dropped, and `diagnostics` says why.
    pub accurate: bool,
    pub source: String,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub span: Option<SpanDump>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub comment: Option<String>,
    /// The abstract events this one refines. More than one is a merge.
    pub refines: Vec<String>,
    pub parameters: Vec<IdentDump>,
    pub guards: Vec<PredicateDump>,
    pub witnesses: Vec<PredicateDump>,
    pub actions: Vec<ActionDump>,
}

/// A checked context.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ContextDump {
    pub name: String,
    pub source: String,
    pub accurate: bool,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub span: Option<SpanDump>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub comment: Option<String>,
    /// Directly extended contexts.
    pub extends: Vec<String>,
    /// Every extended context, transitively, oldest first.
    pub ancestors: Vec<String>,
    pub carrier_sets: Vec<IdentDump>,
    pub constants: Vec<IdentDump>,
    pub axioms: Vec<PredicateDump>,
}

/// A checked machine.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct MachineDump {
    pub name: String,
    pub source: String,
    pub accurate: bool,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub span: Option<SpanDump>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub comment: Option<String>,
    /// The machine this one refines, if any.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub refines: Option<String>,
    /// Every refined machine, transitively, oldest first.
    pub ancestors: Vec<String>,
    /// Directly seen contexts.
    pub sees: Vec<String>,
    /// Every context visible here, transitively, in the order the checker
    /// makes them visible.
    pub internal_contexts: Vec<String>,
    pub variables: Vec<VariableDump>,
    /// Inherited invariants first, oldest machine first, then this machine's.
    pub invariants: Vec<PredicateDump>,
    pub variants: Vec<VariantDump>,
    /// Initialisation first, then the events in source order.
    pub events: Vec<EventDump>,
}

/// A finding about the model.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct DiagnosticDump {
    /// `error`, `warning` or `info`.
    pub severity: &'static str,
    /// The rule that produced it, where one did.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub rule_id: Option<&'static str>,
    /// What it is about, as a component name optionally followed by a label.
    pub origin: String,
    pub message: String,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub span: Option<SpanDump>,
}

fn convergence_name(convergence: Convergence) -> &'static str {
    match convergence {
        Convergence::Ordinary => "ordinary",
        Convergence::Convergent => "convergent",
        Convergence::Anticipated => "anticipated",
    }
}

/// Where an element was written: the component that wrote it, and the clause
/// within that component.
///
/// The two travel together because neither answers the question alone. The
/// clause gives the position and the comment; the component says which text
/// that position indexes, which for anything inherited is not the component
/// being converted.
#[derive(Default)]
pub(crate) struct Placement<'a> {
    owner: Option<&'a ComponentOrigin<'a>>,
    clause: Origin<'a>,
}

/// Everything the assembly needs while it converts one component.
///
/// An element is not always written where it appears. A machine carries the
/// invariants, guards and actions it inherits, and those were written in the
/// machine it refines, so their spans index that machine's text. The builder
/// therefore resolves each element against the component that *wrote* it,
/// which is what makes an inherited position reportable at all rather than
/// merely omitted.
pub(crate) struct Builder<'a, 'm> {
    origins: &'a Origins<'a>,
    /// The component whose elements are being converted.
    component: &'a str,
    types: &'m mut BTreeMap<String, TypeNode>,
    extensions: &'m mut BTreeMap<String, ExtensionInfo>,
}

impl<'a, 'm> Builder<'a, 'm> {
    pub(crate) fn new(
        origins: &'a Origins<'a>,
        component: &'a str,
        types: &'m mut BTreeMap<String, TypeNode>,
        extensions: &'m mut BTreeMap<String, ExtensionInfo>,
    ) -> Self {
        Builder {
            origins,
            component,
            types,
            extensions,
        }
    }

    /// The source of an element written by `owner`, which is this component
    /// unless the element was inherited.
    fn owner(&self, inherited_from: Option<&str>) -> Option<&'a ComponentOrigin<'a>> {
        self.origins.get(inherited_from.unwrap_or(self.component))
    }

    /// A converter for a formula written by `owner`.
    ///
    /// Node spans name the owning file only when it is not this component's,
    /// so the common case stays compact and the inherited case stays
    /// unambiguous.
    fn ctx(&mut self, owner: Option<&'a ComponentOrigin<'a>>, foreign: bool) -> Ctx<'_> {
        let file = foreign
            .then(|| owner.map(|o| o.source_id.clone()))
            .flatten();
        Ctx::new(
            owner.and_then(|o| o.lines.as_ref()),
            file,
            self.types,
            self.extensions,
        )
    }

    /// The document form of an element's position and comment, resolved
    /// against the component that wrote it.
    fn place(&self, at: &Placement<'_>) -> (Option<SpanDump>, Option<String>) {
        let span = at.owner.and_then(|o| o.place(at.clause.span));
        (span, at.clause.comment.map(str::to_string))
    }

    /// Look an element up in the component that wrote it.
    fn lookup(
        &self,
        inherited_from: Option<&str>,
        f: impl FnOnce(&ComponentOrigin<'a>) -> Origin<'a>,
    ) -> Placement<'a> {
        let owner = self.owner(inherited_from);
        Placement {
            owner,
            clause: owner.map(f).unwrap_or_default(),
        }
    }

    /// The placement of a component itself, which is always its own.
    fn own_placement(&self) -> Placement<'a> {
        let owner = self.owner(None);
        Placement {
            owner,
            clause: owner.map(|c| c.own.clone()).unwrap_or_default(),
        }
    }

    fn register_type(&mut self, ty: &rossi::formula::Type) -> String {
        let key = ty.to_rodin_canonical();
        if !self.types.contains_key(&key) {
            self.types.insert(key.clone(), TypeNode::of(ty));
        }
        key
    }

    fn ident(
        &mut self,
        name: &str,
        ty: &rossi::formula::Type,
        source: &str,
        at: &Placement<'_>,
        inherited_from: Option<String>,
    ) -> IdentDump {
        let (span, comment) = self.place(at);
        IdentDump {
            name: name.to_string(),
            ty: self.register_type(ty),
            source: source.to_string(),
            span,
            comment,
            inherited_from,
        }
    }

    fn predicate(
        &mut self,
        label: &str,
        theorem: Option<bool>,
        typed: &Predicate,
        source: &str,
        at: &Placement<'a>,
        inherited_from: Option<String>,
    ) -> PredicateDump {
        let (span, comment) = self.place(at);
        // Ascriptions are unwrapped first: they are a spelling of a type the
        // node already carries, and Rodin's tree has no node for them, so
        // keeping them would make the tree depend on how the author wrote
        // the formula rather than on what it means.
        let stripped = typed.strip_ascriptions();
        let foreign = inherited_from.is_some();
        let predicate = {
            let mut ctx = self.ctx(at.owner, foreign);
            super::formula::predicate(&mut ctx, &stripped, &mut Vec::new())
        };
        PredicateDump {
            label: label.to_string(),
            theorem,
            source: source.to_string(),
            span,
            comment,
            inherited_from,
            text: normalize::canonical_typed_predicate(typed),
            predicate,
        }
    }

    fn expression(&mut self, owner: Option<&'a ComponentOrigin<'a>>, typed: &Expression) -> Node {
        let stripped = typed.strip_ascriptions();
        let mut ctx = self.ctx(owner, false);
        super::formula::expression(&mut ctx, &stripped, &mut Vec::new())
    }

    fn assignment(
        &mut self,
        owner: Option<&'a ComponentOrigin<'a>>,
        foreign: bool,
        typed: &Assignment,
    ) -> (AssignmentNode, Option<Node>) {
        let stripped = typed.strip_ascriptions();
        let mut ctx = self.ctx(owner, foreign);
        let node = super::formula::assignment(&mut ctx, &stripped);
        // The before-after predicate is only defined on a checked
        // assignment, and asking for one otherwise is a panic rather than an
        // error, so an unchecked action simply reports none.
        let ba = stripped.is_type_checked().then(|| {
            let predicate = stripped.ba_predicate();
            super::formula::predicate(&mut ctx, &predicate, &mut Vec::new())
        });
        (node, ba)
    }
}

/// Convert one checked context.
pub(crate) fn context(
    builder: &mut Builder<'_, '_>,
    source: String,
    checked: &CheckedContext,
) -> ContextDump {
    let record = &checked.record;
    let own = builder.own_placement();
    let (span, comment) = builder.place(&own);

    let carrier_sets = record
        .carrier_sets
        .iter()
        .map(|set| {
            let at = builder.lookup(None, |c| c.carrier_set(&set.name));
            builder.ident(&set.name, &set.ty, set.source.as_str(), &at, None)
        })
        .collect();
    let constants = record
        .constants
        .iter()
        .map(|constant| {
            let at = builder.lookup(None, |c| c.constant(&constant.name));
            builder.ident(
                &constant.name,
                &constant.ty,
                constant.source.as_str(),
                &at,
                None,
            )
        })
        .collect();
    let axioms = record
        .axioms
        .iter()
        .map(|axiom| {
            let at = builder.lookup(None, |c| c.axiom(axiom.source_index, &axiom.label));
            builder.predicate(
                &axiom.label,
                Some(axiom.is_theorem),
                &axiom.typed,
                axiom.source.as_str(),
                &at,
                None,
            )
        })
        .collect();

    ContextDump {
        name: record.name.clone(),
        source,
        accurate: checked.accurate,
        span,
        comment,
        extends: record
            .extends
            .iter()
            .map(|e| e.parent_name.clone())
            .collect(),
        ancestors: record.ancestors.clone(),
        carrier_sets,
        constants,
        axioms,
    }
}

/// The chain an extended event inherits, oldest level first, each paired with
/// the machine that wrote it.
///
/// The chain and the refinement line advance together: an event's inherited
/// level was written in the machine this one refines. Walking both at once is
/// what lets an inherited guard say where it came from.
fn chain_with_owners<'m>(
    model: &'m ScModel,
    machine: &'m CheckedMachine,
    event: &'m EventDecl,
) -> Vec<(&'m EventDecl, &'m str)> {
    let mut levels = Vec::new();
    let mut current_event = Some(event);
    let mut current_machine = Some(machine);
    while let Some(level) = current_event {
        levels.push((level, current_machine.map_or("", CheckedMachine::name)));
        current_event = level.inherited.as_deref();
        current_machine = current_machine.and_then(|m| model.refined_machine(m));
    }
    levels.reverse();
    levels
}

/// Convert one checked machine.
pub(crate) fn machine(
    builder: &mut Builder<'_, '_>,
    source: String,
    model: &ScModel,
    checked: &CheckedMachine,
) -> MachineDump {
    let record = &checked.record;
    let own = builder.own_placement();
    let (span, comment) = builder.place(&own);

    let variables = record
        .variables
        .iter()
        .map(|variable| {
            let at = builder.lookup(None, |c| c.variable(&variable.name));
            let (span, comment) = builder.place(&at);
            VariableDump {
                name: variable.name.clone(),
                ty: builder.register_type(&variable.ty),
                source: variable.source.as_str().to_string(),
                span,
                comment,
                is_abstract: variable.is_abstract,
                is_concrete: variable.is_concrete,
            }
        })
        .collect();

    // Inherited invariants come from the ancestors directly rather than
    // through the flattened helper, which drops the machine each one was
    // written in.
    let mut invariants = Vec::new();
    for ancestor in checked.ancestors() {
        let Some(parent) = model.machines.get(ancestor) else {
            continue;
        };
        for invariant in &parent.record.invariants {
            let at = builder.lookup(Some(ancestor), |c| {
                c.invariant(invariant.source_index, &invariant.label)
            });
            invariants.push(builder.predicate(
                &invariant.label,
                Some(invariant.is_theorem),
                &invariant.typed,
                invariant.source.as_str(),
                &at,
                Some(ancestor.clone()),
            ));
        }
    }
    for invariant in &record.invariants {
        let at = builder.lookup(None, |c| {
            c.invariant(invariant.source_index, &invariant.label)
        });
        invariants.push(builder.predicate(
            &invariant.label,
            Some(invariant.is_theorem),
            &invariant.typed,
            invariant.source.as_str(),
            &at,
            None,
        ));
    }

    let variants = record
        .variants
        .iter()
        .map(|variant| {
            let at = builder.lookup(None, |c| c.variant(&variant.label));
            let (span, comment) = builder.place(&at);
            VariantDump {
                label: variant.label.clone(),
                source: variant.source.as_str().to_string(),
                span,
                comment,
                text: variant
                    .typed
                    .as_ref()
                    .map(normalize::canonical_typed_expression)
                    .unwrap_or_else(|| normalize::canonical_expression(&variant.expression)),
                expression: variant
                    .typed
                    .as_ref()
                    .map(|e| builder.expression(at.owner, e)),
            }
        })
        .collect();

    let events = record
        .events
        .iter()
        .map(|event| self_event(builder, model, checked, event))
        .collect();

    MachineDump {
        name: record.name.clone(),
        source,
        accurate: checked.accurate,
        span,
        comment,
        refines: record.refines.as_ref().map(|r| r.parent_name.clone()),
        ancestors: record.ancestors.clone(),
        sees: record.sees.iter().map(|s| s.name.clone()).collect(),
        internal_contexts: model
            .seen_contexts(checked)
            .iter()
            .map(|c| c.name().to_string())
            .collect(),
        variables,
        invariants,
        variants,
        events,
    }
}

fn self_event(
    builder: &mut Builder<'_, '_>,
    model: &ScModel,
    machine: &CheckedMachine,
    event: &EventDecl,
) -> EventDump {
    let own_machine = machine.name();
    let levels = chain_with_owners(model, machine, event);
    let inherited_from = |owner: &str| (owner != own_machine).then(|| owner.to_string());

    let at = builder.lookup(None, |c| c.event(&event.label));
    let (span, comment) = builder.place(&at);

    // Parameters, guards and actions all read the same way: the chain oldest
    // first, then this event's own. A parameter re-listed along the chain
    // names the same thing, so it is kept once.
    let mut parameters: Vec<IdentDump> = Vec::new();
    for (level, owner) in &levels {
        for parameter in &level.parameters {
            if parameters.iter().any(|p| p.name == parameter.name) {
                continue;
            }
            let from = inherited_from(owner);
            let at = builder.lookup(from.as_deref(), |c| {
                c.parameter(&event.label, &parameter.name)
            });
            parameters.push(builder.ident(
                &parameter.name,
                &parameter.ty,
                parameter.source.as_str(),
                &at,
                from,
            ));
        }
    }

    let mut guards = Vec::new();
    for (level, owner) in &levels {
        for guard in &level.guards {
            let from = inherited_from(owner);
            let at = builder.lookup(from.as_deref(), |c| {
                c.guard(&event.label, guard.source_index, &guard.label)
            });
            guards.push(builder.predicate(
                &guard.label,
                Some(guard.is_theorem),
                &guard.typed,
                guard.source.as_str(),
                &at,
                from,
            ));
        }
    }

    let witnesses = event
        .witnesses
        .iter()
        .map(|witness| {
            // A witness the checker supplied for an unmet name has no
            // clause to read from, and is told from a written one by being
            // sourced on the event itself.
            let at = if is_synthesized(&witness.source, &event.source) {
                Placement {
                    owner: builder.owner(None),
                    clause: Origin::default(),
                }
            } else {
                builder.lookup(None, |c| c.witness(&event.label, &witness.label))
            };
            builder.predicate(
                &witness.label,
                None,
                &witness.typed,
                witness.source.as_str(),
                &at,
                None,
            )
        })
        .collect();

    // Actions are already materialised on the record in chain order, so the
    // chain is used only to say who owns each stretch of that list.
    let mut actions = Vec::new();
    for (level, owner) in &levels {
        for action in level.own_actions() {
            let from = inherited_from(owner);
            let at = action_origin(builder, &event.label, level, action, from.as_deref());
            actions.push(self_action(builder, action, &at, from));
        }
    }

    EventDump {
        label: event.label.clone(),
        convergence: convergence_name(event.convergence),
        extended: event.extended,
        accurate: event.accurate,
        source: event.source.as_str().to_string(),
        span,
        comment,
        refines: event
            .refines
            .iter()
            .map(|r| r.abstract_label.clone())
            .collect(),
        parameters,
        guards,
        witnesses,
        actions,
    }
}

/// Where an action was written, or nothing when the checker wrote it.
///
/// The initialisation repair pass adds an action whose recorded position is a
/// placeholder. That position is a real index into the written clauses, so
/// pairing on it would hand the synthesized action another action's comment.
fn action_origin<'a>(
    builder: &Builder<'a, '_>,
    event_label: &str,
    level: &EventDecl,
    action: &ActionDecl,
    inherited_from: Option<&str>,
) -> Placement<'a> {
    if is_synthesized(&action.source, &level.source) {
        return Placement {
            owner: builder.owner(inherited_from),
            clause: Origin::default(),
        };
    }
    builder.lookup(inherited_from, |c| {
        c.action(event_label, action.source_index, &action.label)
    })
}

fn self_action<'a>(
    builder: &mut Builder<'a, '_>,
    action: &ActionDecl,
    at: &Placement<'a>,
    inherited_from: Option<String>,
) -> ActionDump {
    let (span, comment) = builder.place(at);
    let foreign = inherited_from.is_some();
    let converted = action
        .typed
        .as_ref()
        .map(|a| builder.assignment(at.owner, foreign, a));
    let (assignment, ba) = match converted {
        Some((assignment, ba)) => (Some(assignment), ba),
        None => (None, None),
    };
    ActionDump {
        label: action.label.clone(),
        source: action.source.as_str().to_string(),
        span,
        comment,
        inherited_from,
        text: action
            .typed
            .as_ref()
            .map(normalize::canonical_typed_assignment)
            .unwrap_or_else(|| normalize::canonical_action(&action.action)),
        assignment,
        ba,
    }
}

/// Convert one finding, anchoring it in the component its origin names.
pub(crate) fn diagnostic(
    diagnostic: &Diagnostic,
    origins: &super::origin::Origins<'_>,
) -> DiagnosticDump {
    let span = origins.get(diagnostic.component()).and_then(|c| {
        let lines = c.lines.as_ref()?;
        let span = diagnostic.span?;
        Some(lines.span(span, Some(c.source_id.clone())))
    });

    DiagnosticDump {
        severity: diagnostic.severity.as_str(),
        rule_id: diagnostic.rule_id.map(|rule| rule.code()),
        origin: diagnostic.origin.clone(),
        message: diagnostic.message.clone(),
        span,
    }
}
