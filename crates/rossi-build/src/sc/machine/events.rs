//! Event-scoped decl builders for the machine static checker.
//!
//! Splits out the heavy `build_event_decl` and its per-bucket
//! sub-builders so [`super::check_machine`] reads as the orchestration
//! layer it is. Nothing in here is exported to callers outside the
//! `machine` module.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use rossi::{
    Action, ActionKind, Event, Ident, InitialisationEvent, LabeledAction, LabeledPredicate,
    NamedElement, PredicateKind,
};

use crate::checked_predicate::{ActionCheck, check_action, check_labeled_predicate};
use crate::handles::HandleUri;
use crate::infer::infer_constants;
use crate::rodin_ids::{Kind, RodinIds, Scope};
use crate::sc::CheckedMachine;
use crate::sc::machine_record::{
    ActionDecl, Convergence, EventDecl, GuardDecl, ParameterDecl, RefinesEventDecl, WitnessDecl,
};
use crate::type_env::TypeEnv;
use crate::types::Type;
use crate::xml_out::in_tag;
use crate::{Diagnostic, Severity};

/// Unified view over INIT and ordinary events. Keeps [`build_event_decl`]
/// free of `match` noise. Copy-cheap since both variants are just
/// references.
#[derive(Clone, Copy)]
pub(super) enum EventKind<'a> {
    Init(&'a InitialisationEvent),
    Ordinary(&'a Event),
}

impl<'a> EventKind<'a> {
    fn label(&self) -> &'a str {
        match self {
            EventKind::Init(_) => crate::sc::initialisation_label(),
            EventKind::Ordinary(e) => e.name.as_str(),
        }
    }
    fn parameters(&self) -> &'a [NamedElement] {
        match self {
            EventKind::Init(_) => &[],
            EventKind::Ordinary(e) => &e.parameters,
        }
    }
    fn guards(&self) -> &'a [LabeledPredicate] {
        match self {
            EventKind::Init(_) => &[],
            EventKind::Ordinary(e) => &e.guards,
        }
    }
    fn actions(&self) -> &'a [LabeledAction] {
        match self {
            EventKind::Init(i) => &i.actions,
            EventKind::Ordinary(e) => &e.actions,
        }
    }
    fn witnesses_primary(&self) -> &'a [LabeledPredicate] {
        match self {
            EventKind::Init(i) => &i.witnesses,
            EventKind::Ordinary(e) => &e.witnesses,
        }
    }
    fn witnesses_with(&self) -> &'a [LabeledPredicate] {
        match self {
            EventKind::Init(i) => &i.with,
            EventKind::Ordinary(e) => &e.with,
        }
    }
    fn extended(&self) -> bool {
        match self {
            EventKind::Init(i) => i.extended,
            EventKind::Ordinary(e) => e.extended,
        }
    }
    fn convergence(&self) -> Convergence {
        match self {
            EventKind::Init(_) => Convergence::Ordinary,
            EventKind::Ordinary(e) => Convergence::from_status(e.status),
        }
    }
    fn explicit_refines(&self) -> Option<&'a str> {
        match self {
            EventKind::Init(_) => None,
            EventKind::Ordinary(e) => e.refines.as_deref(),
        }
    }
    /// Span of the event's name token, for diagnostics about the event itself.
    fn name_span(&self) -> Option<rossi::ast::Span> {
        match self {
            EventKind::Init(i) => i.name_span,
            EventKind::Ordinary(e) => e.name_span,
        }
    }
}

/// Machine-wide state shared by every event check.
#[derive(Clone, Copy)]
pub(super) struct MachineCheckContext<'a> {
    pub(super) ids: &'a RodinIds,
    pub(super) file_root: &'a HandleUri,
    pub(super) project_name: &'a str,
    pub(super) base_env: &'a TypeEnv,
    pub(super) parent: Option<&'a CheckedMachine>,
    pub(super) abstract_only: &'a BTreeSet<String>,
    pub(super) declared_variable_names: &'a BTreeSet<String>,
    pub(super) variant_usable: bool,
    pub(super) concrete_vars: &'a [String],
    pub(super) machine_name: &'a str,
}

/// State shared by the helper passes for one event.
struct EventCheckContext<'machine, 'event, 'diagnostics> {
    machine: MachineCheckContext<'machine>,
    kind: EventKind<'event>,
    diagnostics: &'diagnostics mut Vec<Diagnostic>,
}

impl<'machine, 'event, 'diagnostics> EventCheckContext<'machine, 'event, 'diagnostics> {
    fn label(&self) -> &'event str {
        self.kind.label()
    }
}

/// The two clause kinds that share an Event-B event label namespace.
#[derive(Clone, Copy)]
enum EventClauseKind {
    Guard,
    Action,
}

impl EventClauseKind {
    fn noun(self) -> &'static str {
        match self {
            Self::Guard => "guard",
            Self::Action => "action",
        }
    }
}

/// Labels already occupied by the event body an extended event inherits.
///
/// Guards are kept on the [`EventDecl`] that introduced them and spliced from
/// the full chain at render time. Actions are materialised as an effective
/// inherited-plus-own list on the immediate parent. These borrowed keys index
/// the complete effective namespace without copying label strings.
struct InheritedEvent<'a> {
    decl: Option<&'a EventDecl>,
    labels: BTreeMap<&'a str, EventClauseKind>,
}

impl<'a> InheritedEvent<'a> {
    fn new(decl: Option<&'a EventDecl>) -> Self {
        let mut labels = BTreeMap::new();
        if let Some(parent) = decl {
            let mut event = Some(parent);
            while let Some(current) = event {
                for guard in &current.guards {
                    labels
                        .entry(guard.label.as_str())
                        .or_insert(EventClauseKind::Guard);
                }
                event = current.inherited.as_deref();
            }
            for action in &parent.actions {
                labels
                    .entry(action.label.as_str())
                    .or_insert(EventClauseKind::Action);
            }
        }
        Self { decl, labels }
    }

    fn label_kind(&self, label: &str) -> Option<EventClauseKind> {
        self.labels.get(label).copied()
    }

    fn contains_label(&self, label: Option<&str>) -> bool {
        label.is_some_and(|label| self.labels.contains_key(label))
    }
}

/// Report one EB022 per concrete label that is already occupied by an
/// inherited guard or action. Rodin also attaches a second marker to the
/// event's `extended` attribute; Rossi deliberately keeps the actionable,
/// label-anchored error only.
fn report_inherited_label_conflicts(
    context: &mut EventCheckContext<'_, '_, '_>,
    inherited: &InheritedEvent<'_>,
    local_duplicate_labels: &BTreeSet<String>,
) {
    let kind = context.kind;
    let machine_name = context.machine.machine_name;
    let event_label = context.label();
    let clauses = crate::duplicates::pred_labels(kind.guards())
        .map(|(label, span)| (label, EventClauseKind::Guard, span))
        .chain(
            crate::duplicates::action_labels(kind.actions())
                .map(|(label, span)| (label, EventClauseKind::Action, span)),
        );

    for (label, local_kind, span) in clauses {
        let Some(inherited_kind) = inherited.label_kind(label) else {
            continue;
        };
        // The component-local duplicate pass has already emitted EB022 for a
        // name used more than once locally. Do not add a second diagnostic for
        // that same label, but still let the filtering passes below drop every
        // local occurrence because the inherited clause wins.
        if local_duplicate_labels.contains(label) {
            continue;
        }
        context.diagnostics.push(Diagnostic {
            severity: crate::RuleId::DuplicateLabel.default_severity(),
            origin: clause_origin(machine_name, event_label, Some(label), local_kind.noun()),
            message: format!(
                "{} label `{label}` conflicts with inherited {} label in extended event `{}`",
                local_kind.noun(),
                inherited_kind.noun(),
                event_label,
            ),
            rule_id: Some(crate::RuleId::DuplicateLabel),
            span,
        });
    }
}

/// Resolve an event's *effective* convergence. The second tuple element is
/// the reason it was downgraded, or `None` when the declared convergence is
/// honoured as-is.
///
/// The static checker downgrades a declared convergence toward `Ordinary`
/// when it cannot honour it. The downgraded value is what gets emitted, and
/// any downgrade marks the event inaccurate — it is no longer a lossless
/// reflection of the source.
///
/// Two downgrade rules, in order:
/// * A concrete event refining an *ordinary* abstract event may not claim a
///   stronger convergence.
/// * A convergent event must decrease a variant: without a usable machine
///   variant it is downgraded, unless an abstract event is already
///   convergent (whose variant then covers it).
///
/// INITIALISATION needs no special-casing: its declared convergence is
/// always `Ordinary` (see [`EventKind::convergence`]), and both rules are
/// no-ops for an ordinary declaration, so an INIT event never downgrades.
fn resolve_convergence(
    declared: Convergence,
    abstract_cvg: Option<Convergence>,
    variant_usable: bool,
) -> (Convergence, Option<&'static str>) {
    if abstract_cvg == Some(Convergence::Ordinary) && declared != Convergence::Ordinary {
        return (
            Convergence::Ordinary,
            Some(
                "event declares a stronger convergence than the ordinary event it refines — \
                 downgraded to ordinary",
            ),
        );
    }
    if declared == Convergence::Convergent
        && abstract_cvg != Some(Convergence::Convergent)
        && !variant_usable
    {
        return (
            Convergence::Ordinary,
            Some("convergent event has no usable variant — downgraded to ordinary"),
        );
    }
    (declared, None)
}

/// A non-deterministic assignment (`x :∈ S` or `x :∣ P`) leaves the
/// assigned variable's after-value open, so a refinement that drops that
/// variable must witness it. A deterministic `x ≔ e` already pins the
/// after-value and needs no witness.
fn is_nondeterministic_assignment(action: &rossi::Action) -> bool {
    matches!(
        action.kind,
        rossi::ActionKind::BecomesIn { .. } | rossi::ActionKind::BecomesSuchThat { .. }
    )
}

/// The witness names a refining event must provide: (a) every abstract
/// parameter it does not itself (re)declare, and (b) the primed after-value
/// `v'` of every disappearing variable a *non-deterministic* abstract action
/// assigns. This is the single set both the event's accuracy flag and its
/// emitted witness elements are derived from.
///
/// `abstract_decl` is the refined (abstract) event. Its full parameter and
/// action sets — own plus any inherited through its own extension — are
/// considered, since an extended abstract event carries its ancestors'
/// parameters and actions just as Rodin's statically-checked event does.
fn required_witness_names(
    abstract_decl: &EventDecl,
    concrete_param_names: &BTreeSet<String>,
    abstract_only: &BTreeSet<String>,
) -> BTreeSet<String> {
    let abstract_params = abstract_decl.chain_parameters();
    // `chain_root_first` excludes the event itself, so its own actions are
    // chained on.
    let abstract_actions = abstract_decl
        .chain_root_first()
        .into_iter()
        .flat_map(|e| e.actions.iter())
        .chain(abstract_decl.actions.iter());

    let mut required: BTreeSet<String> = BTreeSet::new();
    // Local: abstract parameters the concrete event does not (re)declare.
    for p in &abstract_params {
        if !concrete_param_names.contains(&p.name) {
            required.insert(p.name.clone());
        }
    }
    // Global: the primed after-value of each disappearing variable a
    // non-deterministic abstract action assigns.
    for a in abstract_actions {
        if !is_nondeterministic_assignment(&a.action) {
            continue;
        }
        for v in crate::ast_util::lhs_variables(&a.action) {
            if abstract_only.contains(v) {
                required.insert(format!("{v}'"));
            }
        }
    }
    required
}

/// Type-check scope for witness predicates: the concrete environment plus the
/// abstract parameters, concrete after-values, and the disappearing variables
/// (and their primed forms) the witnesses are about.
fn witness_scope(
    event_env: &TypeEnv,
    abstract_decl: &EventDecl,
    abstract_only: &BTreeSet<String>,
    concrete_vars: &[String],
    parent: Option<&CheckedMachine>,
) -> TypeEnv {
    let mut wscope = event_env.clone();
    for variable in concrete_vars {
        if let Some(ty) = event_env.get(variable) {
            wscope.insert(format!("{variable}'"), ty.clone());
        }
    }
    for p in &abstract_decl.chain_parameters() {
        wscope.insert(p.name.clone(), p.ty.clone());
    }
    if let Some(parent) = parent {
        for v in abstract_only {
            if let Some(ty) = parent.env().get(v) {
                wscope.insert(v.clone(), ty.clone());
                wscope.insert(format!("{v}'"), ty.clone());
            }
        }
    }
    wscope
}

/// Resolve a refining event's emitted witnesses *and* its accuracy flag from
/// one [`required_witness_names`] set — the single source both are derived
/// from, mirroring Rodin's witness module.
///
/// A provided WITNESS/WITH clause is *permissible* — kept and emitted — only
/// when its label is a required name and its predicate type-checks; every
/// other provided witness (not required, ill-typed, or unlabelled) is dropped.
/// Each remaining unmet requirement gets a synthesized `⊤` placeholder sourced
/// on the event element, and the event is marked inaccurate (with a warning
/// per unmet name).
///
/// `abstract_decl` is `None` for a new (non-refining) event: nothing is
/// required, so any provided witness is not-permissible and dropped.
fn resolve_witnesses(
    context: &mut EventCheckContext<'_, '_, '_>,
    event_env: &TypeEnv,
    abstract_decl: Option<&EventDecl>,
    inherited_chain: Option<&EventDecl>,
) -> (Vec<WitnessDecl>, bool) {
    let Some(abstract_decl) = abstract_decl else {
        return (Vec::new(), true);
    };
    // The concrete parameter set the requirements are weighed against: own
    // plus any inherited through extension (an extended event re-declares
    // nothing but inherits its abstract's parameters).
    let kind = context.kind;
    let machine = context.machine;
    let label = context.label();
    let mut concrete_param_names: BTreeSet<String> =
        kind.parameters().iter().map(|p| p.name.clone()).collect();
    if let Some(ic) = inherited_chain {
        for p in ic.chain_parameters() {
            concrete_param_names.insert(p.name.clone());
        }
    }
    let mut required =
        required_witness_names(abstract_decl, &concrete_param_names, machine.abstract_only);
    if required.is_empty() {
        // A refining event with nothing to witness: any provided witness is
        // not-permissible and dropped.
        return (Vec::new(), true);
    }

    let wscope = witness_scope(
        event_env,
        abstract_decl,
        machine.abstract_only,
        machine.concrete_vars,
        machine.parent,
    );

    // Keep each *permissible* provided witness, in source order, and clear its
    // requirement: its label is a required name and its predicate type-checks.
    // Everything else is not-permissible and dropped — an ill-typed witness
    // for a required name leaves the requirement unmet, so a `⊤` placeholder
    // is synthesized for it below.
    let mut witnesses: Vec<WitnessDecl> = Vec::new();
    for w in kind
        .witnesses_primary()
        .iter()
        .chain(kind.witnesses_with().iter())
    {
        if let Some(wl) = w.label.as_deref()
            && required.contains(wl)
            && crate::wellformed::is_well_typed_predicate(&wscope, &w.predicate)
        {
            required.remove(wl);
            witnesses.push(build_witness_decl(
                machine.ids,
                machine.file_root,
                label,
                wl,
                w,
            ));
        }
    }

    // Synthesize a `⊤` placeholder for each remaining unmet requirement, in
    // sorted (BTreeSet) order, and warn. The event is inaccurate iff any
    // requirement is still unmet.
    for name in &required {
        context.diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            origin: format!("{}.{}", machine.machine_name, label),
            message: format!("missing or ill-typed witness for '{name}' — event is inaccurate"),
            rule_id: None,
            span: kind.name_span(),
        });
        witnesses.push(synthesize_witness(
            machine.ids,
            machine.file_root,
            label,
            name,
        ));
    }

    (witnesses, required.is_empty())
}

/// Build a synthesized `<scWitness>` placeholder for an unmet required name:
/// predicate `⊤` (the maximally-permissive witness Rodin writes for a missing
/// one), sourced on the event element itself since it has no source clause of
/// its own — the same convention as the INITIALISATION-repair action.
fn synthesize_witness(
    ids: &RodinIds,
    file_root: &HandleUri,
    event_label: &str,
    name: &str,
) -> WitnessDecl {
    WitnessDecl {
        label: name.to_string(),
        predicate_canonical: crate::normalize::canonical_predicate(&PredicateKind::True.into()),
        source: crate::sc::file_child_source(
            ids,
            file_root,
            Kind::Event,
            in_tag::EVENT,
            event_label,
        ),
    }
}

pub(super) fn build_event_decl(
    machine: MachineCheckContext<'_>,
    kind: EventKind<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<(EventDecl, bool)> {
    let mut context = EventCheckContext {
        machine,
        kind,
        diagnostics,
    };
    let label = context.label();
    let source = crate::sc::file_child_source(
        context.machine.ids,
        context.machine.file_root,
        Kind::Event,
        in_tag::EVENT,
        label,
    );

    // Filter duplicate parameters / labels per the SC drop semantics
    // documented in `crate::duplicates` (parameters drop every occurrence;
    // guard/action + witness labels keep the first). Witnesses are
    // enumerated in resolve_witnesses' keep order (`witnesses` before
    // `with`) so the duplicate name-set matches the occurrence the checker
    // retains.
    let event_dups = crate::duplicates::event_duplicates(
        context.machine.machine_name,
        label,
        context
            .kind
            .parameters()
            .iter()
            .map(|p| (p.name.as_str(), p.span)),
        crate::duplicates::pred_labels(context.kind.guards())
            .chain(crate::duplicates::action_labels(context.kind.actions())),
        crate::duplicates::pred_labels(context.kind.witnesses_primary()).chain(
            crate::duplicates::pred_labels(context.kind.witnesses_with()),
        ),
    );
    // A duplicated witness label always drops at least one witness (the
    // keep-loop in `resolve_witnesses` honours only the first per required
    // name — a witness's label is its required name), so the event is no
    // longer a lossless reflection of the source.
    let witness_dup_accurate = event_dups.witness_labels.names.is_empty();

    let (effective_refines, parent_event_decl) =
        resolve_effective_refines(context.kind, context.machine.parent);

    // Explicit refines target missing from parent — an error in Rodin
    // (AbstractEventNotFoundError + EventRefinementError, both Error
    // markers), and the whole concrete event is dropped from the output.
    // (Implicit and INIT are already gated upstream.)
    if let Some(refines) = context.kind.explicit_refines()
        && parent_event_decl.is_none()
    {
        context.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            origin: format!("{}.{}", context.machine.machine_name, label),
            message: format!("refines target '{refines}' not found in parent — event dropped"),
            rule_id: Some(crate::RuleId::CrossReferenceNotFound),
            span: context.kind.name_span(),
        });
        return None;
    }

    let inherited_chain: Option<Rc<EventDecl>> = if context.kind.extended() {
        parent_event_decl.map(Rc::clone)
    } else {
        None
    };
    let inherited = InheritedEvent::new(inherited_chain.as_deref());
    report_inherited_label_conflicts(
        &mut context,
        &inherited,
        &event_dups.guard_action_labels.names,
    );

    // A local event parameter may not reuse any identifier already visible
    // in the machine environment. Rodin reports ParameterNameConflictError
    // and filters the parameter, leaving formulas to resolve the outer name.
    // Names duplicated within the event are already diagnosed and filtered.
    let mut invalid_parameter_names = event_dups.parameters.names.clone();
    let inherited_parameter_names: BTreeSet<&str> = inherited
        .decl
        .into_iter()
        .flat_map(EventDecl::chain_parameters)
        .map(|parameter| parameter.name.as_str())
        .collect();
    for parameter in context.kind.parameters() {
        if invalid_parameter_names.contains(&parameter.name) {
            continue;
        }
        let message = if inherited_parameter_names.contains(parameter.name.as_str()) {
            format!(
                "parameter `{}` conflicts with an inherited parameter and was dropped",
                parameter.name
            )
        } else if context
            .machine
            .declared_variable_names
            .contains(&parameter.name)
            || context.machine.base_env.contains(&parameter.name)
        {
            format!(
                "parameter `{}` conflicts with a visible identifier and was dropped",
                parameter.name
            )
        } else {
            continue;
        };
        context.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            origin: format!(
                "{}.{}.{}",
                context.machine.machine_name, label, parameter.name
            ),
            message,
            rule_id: Some(crate::RuleId::TypeError),
            span: parameter.span,
        });
        invalid_parameter_names.insert(parameter.name.clone());
    }

    let (scope, scope_accurate) = build_event_scope(
        &mut context,
        &inherited,
        &event_dups,
        &invalid_parameter_names,
    );

    let (buckets, buckets_accurate) = build_event_buckets(
        &mut context,
        &scope,
        &inherited,
        &event_dups,
        &invalid_parameter_names,
    );

    let refines_decl =
        effective_refines
            .zip(context.machine.parent)
            .map(|(abs_label, parent_cm)| {
                build_refines_event_decl(
                    context.machine.ids,
                    context.machine.file_root,
                    context.machine.project_name,
                    label,
                    abs_label,
                    parent_cm,
                    context.kind.explicit_refines().is_some(),
                )
            });

    // An extended event inherits its immediate abstract event's inaccuracy:
    // because it copies the abstract clauses verbatim, an inaccurate parent
    // means this event is no longer a lossless reflection of the source.
    // `inherited_chain` is `Some` only for extended events, so a plain
    // refinement does not propagate. The immediate parent's flag already
    // folds in the rest of the chain (parents are checked first).
    let inherited_accurate = inherited_chain.as_deref().is_none_or(|p| p.accurate);

    // Convergence: a declared convergence the checker cannot honour is
    // downgraded toward ordinary, and the downgrade itself marks the event
    // inaccurate. The abstract convergence comes from the refined event
    // (resolved for both plain and extended refinements).
    let abstract_cvg = parent_event_decl.map(|p| p.convergence);
    let (convergence, downgrade_reason) = resolve_convergence(
        context.kind.convergence(),
        abstract_cvg,
        context.machine.variant_usable,
    );
    if let Some(reason) = downgrade_reason {
        context.diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            origin: format!("{}.{}", context.machine.machine_name, label),
            message: reason.to_string(),
            rule_id: None,
            span: context.kind.name_span(),
        });
    }
    let convergence_accurate = downgrade_reason.is_none();

    // Witnesses: resolve the emitted set and the accuracy flag from one
    // required-name computation. Only a refining event can owe witnesses (a new
    // event has no abstract decl, so nothing is required and any provided
    // witness is dropped).
    let (witnesses, witness_accurate) = resolve_witnesses(
        &mut context,
        &scope,
        parent_event_decl.map(|p| &**p),
        inherited_chain.as_deref(),
    );

    // Effective actions = the inherited chain (the parent decl already
    // carries its full root-first closure) ++ own. Materialised here rather
    // than spliced at render time so the accuracy and INITIALISATION-repair
    // passes read one list. Inherited actions are valid in this scope: an
    // extended INITIALISATION that would inherit an action on a vanished
    // variable is dropped upstream by `should_omit_initialisation`.
    let mut actions: Vec<ActionDecl> = Vec::new();
    if let Some(parent_ev) = inherited_chain.as_deref() {
        actions.extend(parent_ev.actions.iter().cloned());
    }
    actions.extend(buckets.actions);

    let mut accurate = scope_accurate
        && buckets_accurate
        && inherited_accurate
        && convergence_accurate
        && witness_accurate
        && witness_dup_accurate;

    // INITIALISATION repair: in Event-B every concrete variable must be
    // initialised, so any concrete, typed variable that no action (inherited
    // or own) assigns is given a default `becomesSuchThat ⊤`. All such
    // variables are gathered into one combined action and the event (not the
    // machine) is marked inaccurate.
    if matches!(context.kind, EventKind::Init(_)) {
        let unassigned: Vec<Ident> = {
            let assigned: std::collections::HashSet<&str> = actions
                .iter()
                .flat_map(|a| lhs_variables(&a.action))
                .collect();
            context
                .machine
                .concrete_vars
                .iter()
                .filter(|v| !assigned.contains(v.as_str()))
                .map(|v| Ident::from(v.clone()))
                .collect()
        };
        if !unassigned.is_empty() {
            let repair_label = fresh_gen_label(&actions);
            actions.push(build_repair_action(
                repair_label,
                &source,
                unassigned,
                context.machine.base_env,
            ));
            accurate = false;
        }
    }
    let decl = EventDecl {
        label: label.to_string(),
        convergence,
        extended: context.kind.extended(),
        accurate,
        source,
        refines: refines_decl,
        parameters: buckets.parameters,
        guards: buckets.guards,
        actions,
        witnesses,
        inherited: inherited_chain,
    };

    Some((decl, accurate))
}

/// Fresh label for the synthetic INITIALISATION-repair action: `GEN`, then
/// `GEN1`, `GEN12`, and so on — each collision appends the running index to
/// the label until it is free among the event's existing actions. The append
/// form only triggers when the model already declares an action named `GEN`.
fn fresh_gen_label(actions: &[ActionDecl]) -> String {
    let used: std::collections::HashSet<&str> = actions.iter().map(|a| a.label.as_str()).collect();
    let mut label = "GEN".to_string();
    let mut index = 1;
    while used.contains(label.as_str()) {
        label.push_str(&index.to_string());
        index += 1;
    }
    label
}

/// Build the synthetic `<vars> :∣ ⊤` repair action. The assignment text is
/// produced through the shared [`check_action`] canonicaliser, and the
/// `source` points at the INITIALISATION event element (the generated action
/// has no source clause of its own).
fn build_repair_action(
    label: String,
    source: &HandleUri,
    variables: Vec<Ident>,
    env: &TypeEnv,
) -> ActionDecl {
    let action = Action::from(ActionKind::BecomesSuchThat {
        variables,
        predicate: PredicateKind::True.into(),
    });
    let checked = check_action(&action, env);
    ActionDecl {
        label,
        // ActionDecl.source_index is never read for actions; a generated
        // action has no source clause, so use the same 0 placeholder as
        // other synthetic decls.
        source_index: 0,
        action: checked.action,
        canonical: checked.canonical,
        source: source.clone(),
    }
}

/// Resolve `(effective_refines_label, parent_event_decl)` for `kind`.
/// INIT events implicitly refine the parent's INITIALISATION when one
/// exists; ordinary events prefer the explicit `refines` annotation but
/// fall back to an implicit same-label match when extended.
///
/// `lint::extends_chain_root_first` mirrors the Ordinary half of this rule
/// on the raw AST, and `lint::inherited_init_chain` mirrors the Init half —
/// keep them in sync when changing the resolution.
fn resolve_effective_refines<'a, 'b>(
    kind: EventKind<'a>,
    parent: Option<&'b CheckedMachine>,
) -> (Option<&'a str>, Option<&'b Rc<EventDecl>>) {
    let effective_refines: Option<&str> = match kind {
        EventKind::Init(_) => parent
            .filter(|p| {
                p.events_by_label
                    .contains_key(crate::sc::initialisation_label())
            })
            .map(|_| crate::sc::initialisation_label()),
        EventKind::Ordinary(e) => {
            let explicit = e.refines.as_deref();
            let implicit = if e.refines.is_none() && e.extended {
                parent
                    .filter(|p| p.events_by_label.contains_key(&e.name))
                    .map(|_| e.name.as_str())
            } else {
                None
            };
            explicit.or(implicit)
        }
    };
    let parent_event_decl =
        effective_refines.and_then(|l| parent.and_then(|p| p.events_by_label.get(l)));
    (effective_refines, parent_event_decl)
}

/// Build the event-local type scope: outer env + inherited parameter
/// types (when extended) + own-parameter inference from inherited+own
/// guards. Returns the scope and an `accurate` flag (false when any
/// parameter could not be typed).
fn build_event_scope(
    context: &mut EventCheckContext<'_, '_, '_>,
    inherited: &InheritedEvent<'_>,
    dups: &crate::duplicates::EventDuplicates,
    invalid_parameter_names: &BTreeSet<String>,
) -> (TypeEnv, bool) {
    let kind = context.kind;
    let machine_name = context.machine.machine_name;
    let label = context.label();
    let mut scope = context.machine.base_env.clone();
    scope.push_scope();
    if let Some(pe) = inherited.decl {
        // The parent's full parameter chain — ancestors root-first then the
        // parent's own, deduped by name — is the scope an extended event
        // inherits. This is the same set `chain_parameters` builds for
        // `CheckedMachine::event_env`.
        for p in pe.chain_parameters() {
            scope.insert(p.name.clone(), p.ty.clone());
        }
    }

    // Inherited typing axioms (when extended) + own guard predicates.
    // `typing_guard_predicates` already walks the parent chain
    // root-first and includes parent's own guards, gated on parent's
    // own `extended` flag.
    let mut axioms: Vec<rossi::Predicate> = Vec::new();
    if let Some(pe) = inherited.decl {
        for p in pe.typing_guard_predicates() {
            axioms.push(p.clone());
        }
    }
    // 2nd+ occurrences of a duplicated guard label are dropped from the
    // event (see `build_event_buckets`), so they must not contribute typing
    // either; the kept first occurrence still types its parameters.
    let mut typing_kept = crate::duplicates::FirstKept::new(&dups.guard_action_labels.names);
    for g in kind.guards() {
        if inherited.contains_label(g.label.as_deref()) {
            continue;
        }
        if typing_kept.drops(g.label.as_deref()) {
            continue;
        }
        axioms.push(g.predicate.clone());
    }
    // Invalid parameters are dropped entirely, so they are not typed again.
    // Duplicate and outer-name conflicts already have their own diagnostics.
    let param_names: Vec<String> = kind
        .parameters()
        .iter()
        .filter(|p| !invalid_parameter_names.contains(&p.name))
        .map(|p| p.name.clone())
        .collect();
    let unresolved = infer_constants(&mut scope, &param_names, &axioms);
    let mut accurate = true;
    for name in &unresolved {
        context.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            origin: format!("{machine_name}.{label}.{name}"),
            message: "could not infer parameter type from guards".to_string(),
            rule_id: Some(crate::RuleId::TypeError),
            span: crate::ast_util::named_element_span(kind.parameters(), name),
        });
        accurate = false;
    }
    (scope, accurate)
}

/// Per-event decl buckets produced by [`build_event_buckets`]. Witnesses are
/// not here — they are resolved separately by [`resolve_witnesses`], which
/// needs the abstract event the buckets pass doesn't see.
struct EventBuckets {
    parameters: Vec<ParameterDecl>,
    guards: Vec<GuardDecl>,
    actions: Vec<ActionDecl>,
}

/// `machine.event.clause` origin for a per-clause diagnostic, falling back to
/// `fallback` (e.g. `"grd"` / `"act"`) when the clause carries no label.
fn clause_origin(machine: &str, event: &str, clause_label: Option<&str>, fallback: &str) -> String {
    format!("{machine}.{event}.{}", clause_label.unwrap_or(fallback))
}

/// Build the three per-event decl buckets (guards, parameters, actions),
/// running per-clause checks (abstract-only references, well-typedness,
/// LHS-declared) and dropping any clause that fails. The returned `bool` is
/// the `accurate` flag for the event — `false` if any clause was dropped.
fn build_event_buckets(
    context: &mut EventCheckContext<'_, '_, '_>,
    scope: &TypeEnv,
    inherited: &InheritedEvent<'_>,
    dups: &crate::duplicates::EventDuplicates,
    invalid_parameter_names: &BTreeSet<String>,
) -> (EventBuckets, bool) {
    let machine = context.machine;
    let kind = context.kind;
    let label = context.label();
    let mut accurate = true;

    // Guards and actions share one label namespace, so the first-kept rule
    // for duplicated labels (EB022) tracks occurrences across both loops:
    // a guard `lbl` followed by an action `lbl` keeps the guard.
    let mut label_kept = crate::duplicates::FirstKept::new(&dups.guard_action_labels.names);

    let mut guards: Vec<GuardDecl> = Vec::with_capacity(kind.guards().len());
    for (i, g) in kind.guards().iter().enumerate() {
        // Imported labels are installed before concrete clauses in Rodin's
        // event label table. The inherited guard/action therefore wins and
        // every colliding concrete clause is dropped.
        if inherited.contains_label(g.label.as_deref()) {
            accurate = false;
            continue;
        }
        // 2nd+ use of a duplicated guard/action label: Rodin keeps the first
        // occurrence, drops the rest, and marks the event inaccurate (the
        // EB022 error is already reported).
        if label_kept.drops(g.label.as_deref()) {
            accurate = false;
            continue;
        }
        // Disappeared-variable reference: the guard reads a variable that
        // vanished in this refinement (inherited from the parent but not
        // redeclared, no witness). Reading a disappeared variable in a guard is
        // an error (EB025) — except in a *theorem* guard, where it is permitted
        // and reported only as the softer warning. Rodin allows the theorem
        // case with no problem marker at all (MachineEventGuardFreeIdentsModule
        // gates VariableHasDisappearedError on `!isTheorem`), so the Warning
        // is deliberately stricter than Rodin and must not be "aligned" to an
        // Error. Either way the guard is dropped and the event marked
        // `accurate=false` — see `ITERATION.bcm`'s `stepone`/`steptwo`
        // referencing `n`, `t` (Group R).
        if !machine.abstract_only.is_empty()
            && let Some(bad) = crate::sc::identifier_walker::first_forbidden_identifier_in_predicate(
                &g.predicate,
                machine.abstract_only,
            )
        {
            let (severity, message, rule) = if g.is_theorem {
                (
                    Severity::Warning,
                    format!("theorem guard references abstract-only variable '{bad}' — dropped"),
                    crate::RuleId::UndeclaredIdentifier,
                )
            } else {
                (
                    Severity::Error,
                    format!(
                        "guard references variable '{bad}', which has disappeared in this \
                         refinement (declared in an abstract machine but not kept here)"
                    ),
                    crate::RuleId::DisappearedVariable,
                )
            };
            context.diagnostics.push(Diagnostic {
                severity,
                origin: clause_origin(machine.machine_name, label, g.label.as_deref(), "grd"),
                message,
                rule_id: Some(rule),
                span: g.span,
            });
            accurate = false;
            continue;
        }
        match build_guard_decl(
            machine.ids,
            machine.file_root,
            label,
            i,
            g,
            scope,
            machine.machine_name,
        ) {
            Ok(d) => guards.push(d),
            Err(diag) => {
                context.diagnostics.push(diag);
                accurate = false;
            }
        }
    }

    // Invalid parameters are dropped entirely (every duplicate occurrence,
    // or the one declaration that conflicts with a visible outer name). The
    // scope filter alone is not enough because that outer name is present.
    let mut params_sorted: Vec<&NamedElement> = kind
        .parameters()
        .iter()
        .filter(|p| scope.contains(&p.name) && !invalid_parameter_names.contains(&p.name))
        .collect();
    params_sorted.sort_by(|a, b| a.name.cmp(&b.name));
    let mut parameters: Vec<ParameterDecl> = Vec::with_capacity(params_sorted.len());
    for p in params_sorted {
        parameters.push(build_parameter_decl(
            machine.ids,
            machine.file_root,
            label,
            p,
            scope,
        ));
    }

    // Cascade-drop: an action whose LHS targets a variable that wasn't
    // typed (and so dropped from env) is meaningless — Rodin emits the
    // enclosing event without it and marks the event `accurate=false`.
    // We mirror that.
    let mut actions: Vec<ActionDecl> = Vec::with_capacity(kind.actions().len());
    for (i, act) in kind.actions().iter().enumerate() {
        if inherited.contains_label(act.label.as_deref()) {
            accurate = false;
            continue;
        }
        // Same first-kept rule as the guards loop, over the shared
        // guard+action label namespace.
        if label_kept.drops(act.label.as_deref()) {
            accurate = false;
            continue;
        }
        let bad_lhs = lhs_variables(&act.action)
            .iter()
            .find(|v| !scope.contains(v))
            .map(|v| v.to_string());
        if let Some(bad) = bad_lhs {
            context.diagnostics.push(Diagnostic {
                severity: Severity::Error,
                origin: clause_origin(machine.machine_name, label, act.label.as_deref(), "act"),
                message: format!("LHS variable '{bad}' is not declared"),
                rule_id: Some(crate::RuleId::UndeclaredIdentifier),
                span: act.span,
            });
            accurate = false;
            continue;
        }
        // Disappeared-variable reference: the action touches a variable the
        // abstract machine declared but this refinement dropped (data-refined
        // away) — either by *assigning* it (its LHS) or by *reading* it in its
        // RHS / generalised-assignment predicate. The variable no longer exists
        // in the concrete state, so either way it is rejected (EB025). Rodin
        // drops the action and marks the event `accurate=false` (Group R); we
        // mirror that and report an error. The LHS is checked first so an
        // illegal write is described as such.
        let disappeared = if machine.abstract_only.is_empty() {
            None
        } else {
            lhs_variables(&act.action)
                .into_iter()
                .find(|v| machine.abstract_only.contains(*v))
                .map(|v| (v.to_string(), "assigns"))
                .or_else(|| {
                    crate::sc::identifier_walker::first_forbidden_identifier_in_action_rhs(
                        &act.action,
                        machine.abstract_only,
                    )
                    .map(|v| (v, "references"))
                })
        };
        if let Some((bad, verb)) = disappeared {
            context.diagnostics.push(Diagnostic {
                severity: Severity::Error,
                origin: clause_origin(machine.machine_name, label, act.label.as_deref(), "act"),
                message: format!(
                    "action {verb} variable '{bad}', which has disappeared in this \
                     refinement (declared in an abstract machine but not kept here)"
                ),
                rule_id: Some(crate::RuleId::DisappearedVariable),
                span: act.span,
            });
            accurate = false;
            continue;
        }
        let checked = check_action(&act.action, scope);
        if let Some(bad) = &checked.free_identifier {
            context.diagnostics.push(Diagnostic {
                severity: Severity::Error,
                origin: clause_origin(machine.machine_name, label, act.label.as_deref(), "act"),
                message: format!("unknown identifier '{bad}' in action"),
                rule_id: Some(crate::RuleId::UndeclaredIdentifier),
                span: act.span,
            });
            accurate = false;
            continue;
        }
        // Per-clause well-typedness drop: catches things like
        // `auctions ≔ auctions ∪ {a ↦ i}` where the two operands of `∪`
        // are at different power-set levels. Rodin emits the event
        // `accurate=false` and skips the action.
        if !crate::wellformed::is_well_typed_enriched_action(scope, &checked.action) {
            context.diagnostics.push(Diagnostic {
                severity: Severity::Error,
                origin: clause_origin(machine.machine_name, label, act.label.as_deref(), "act"),
                message: "action is ill-typed".to_string(),
                rule_id: Some(crate::RuleId::TypeError),
                span: act.span,
            });
            accurate = false;
            continue;
        }
        actions.push(build_action_decl(
            machine.ids,
            machine.file_root,
            label,
            i,
            act,
            checked,
        ));
    }

    (
        EventBuckets {
            parameters,
            guards,
            actions,
        },
        accurate,
    )
}

/// Build the `source=` URI for an event child element (guard, parameter,
/// action, witness): `<file_root>/event/<child_tag>`. Centralises the
/// `Scope::Event { label } / Kind::X / child_label` id lookup pattern so
/// each per-bucket builder stays a thin wrapper.
fn build_event_child_source(
    ids: &RodinIds,
    file_root: &HandleUri,
    event_label: &str,
    kind: Kind,
    child_tag: &str,
    child_label: &str,
) -> HandleUri {
    let child_id = ids.get_or(
        Scope::Event {
            label: event_label.to_string(),
        },
        kind,
        child_label,
    );
    let event_source =
        crate::sc::file_child_source(ids, file_root, Kind::Event, in_tag::EVENT, event_label);
    event_source.child(child_tag, child_id)
}

fn build_guard_decl(
    ids: &RodinIds,
    file_root: &HandleUri,
    event_label: &str,
    source_index: usize,
    g: &LabeledPredicate,
    env: &TypeEnv,
    machine_name: &str,
) -> std::result::Result<GuardDecl, Diagnostic> {
    let (label, pc) = check_labeled_predicate(g, env, "grd", "guard", |lbl| {
        format!("{machine_name}.{event_label}.{lbl}")
    })?;
    let source = build_event_child_source(
        ids,
        file_root,
        event_label,
        Kind::Guard,
        in_tag::GUARD,
        &label,
    );
    Ok(GuardDecl {
        label,
        source_index,
        predicate: pc.predicate,
        predicate_canonical: pc.canonical,
        is_theorem: g.is_theorem,
        source,
    })
}

fn build_parameter_decl(
    ids: &RodinIds,
    file_root: &HandleUri,
    event_label: &str,
    p: &NamedElement,
    env: &TypeEnv,
) -> ParameterDecl {
    let ty = env.get(&p.name).cloned().unwrap_or(Type::Integer);
    let source = build_event_child_source(
        ids,
        file_root,
        event_label,
        Kind::Parameter,
        in_tag::PARAMETER,
        &p.name,
    );
    ParameterDecl {
        name: p.name.clone(),
        ty,
        source,
    }
}

fn build_action_decl(
    ids: &RodinIds,
    file_root: &HandleUri,
    event_label: &str,
    source_index: usize,
    act: &LabeledAction,
    checked: ActionCheck,
) -> ActionDecl {
    let label = act.label.clone().unwrap_or_else(|| "act".to_string());
    let source = build_event_child_source(
        ids,
        file_root,
        event_label,
        Kind::Action,
        in_tag::ACTION,
        &label,
    );
    ActionDecl {
        label,
        source_index,
        action: checked.action,
        canonical: checked.canonical,
        source,
    }
}

fn build_witness_decl(
    ids: &RodinIds,
    file_root: &HandleUri,
    event_label: &str,
    witness_label: &str,
    w: &LabeledPredicate,
) -> WitnessDecl {
    // Witnesses reference dropped abstract names that aren't in env;
    // skip the free-ident check and only canonicalise. Enrich with an
    // empty env first so structural lowerings (e.g. SetComprehension
    // short → long form) still fire — binder-type stamping degrades to
    // best-effort, which is pre-existing behaviour for this path.
    let enriched = crate::enrich::enrich_predicate(w.predicate.clone(), &TypeEnv::new());
    let canonical = crate::normalize::canonical_predicate(&enriched);
    let source = build_event_child_source(
        ids,
        file_root,
        event_label,
        Kind::Witness,
        in_tag::WITNESS,
        witness_label,
    );
    WitnessDecl {
        label: witness_label.to_string(),
        predicate_canonical: canonical,
        source,
    }
}

fn build_refines_event_decl(
    ids: &RodinIds,
    file_root: &HandleUri,
    project_name: &str,
    own_event_label: &str,
    abstract_event_label: &str,
    parent: &CheckedMachine,
    has_explicit_refines_child: bool,
) -> RefinesEventDecl {
    let event_source =
        crate::sc::file_child_source(ids, file_root, Kind::Event, in_tag::EVENT, own_event_label);
    let re_source = if has_explicit_refines_child {
        let refines_id = ids.get_or(
            Scope::Event {
                label: own_event_label.to_string(),
            },
            Kind::RefinesEvent,
            abstract_event_label,
        );
        event_source.child(in_tag::REFINES_EVENT, refines_id)
    } else {
        event_source
    };
    let abstract_internal_name = parent
        .event_internal_name(abstract_event_label)
        .expect("resolved parent event has an internal name");
    let sc_target = HandleUri::root(
        project_name,
        parent.output_filename(),
        crate::xml_out::tag::SC_MACHINE_FILE,
        parent.name(),
    )
    .child(crate::xml_out::tag::SC_EVENT, abstract_internal_name)
    .into();
    RefinesEventDecl {
        abstract_label: abstract_event_label.to_string(),
        sc_target,
        source: re_source,
    }
}

pub(super) use crate::ast_util::lhs_variables;
