//! Event-scoped decl builders for the machine static checker.
//!
//! Splits out the heavy `build_event_decl` and its per-bucket
//! sub-builders so [`super::check_machine`] reads as the orchestration
//! layer it is. Nothing in here is exported to callers outside the
//! `machine` module.

use std::collections::BTreeSet;
use std::rc::Rc;

use rossi::{
    Event, EventStatus, InitialisationEvent, LabeledAction, LabeledPredicate, NamedElement,
};

use crate::checked_predicate::{check_action, check_labeled_predicate};
use crate::handles::HandleUri;
use crate::infer::infer_constants;
use crate::rodin_ids::{Kind, RodinIds, Scope};
use crate::sc::CheckedMachine;
use crate::sc::machine_record::{
    ActionDecl, EventDecl, GuardDecl, ParameterDecl, RefinesEventDecl, WitnessDecl,
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
            EventKind::Init(_) => "INITIALISATION",
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
    fn convergence(&self) -> &'static str {
        match self {
            EventKind::Init(_) => "0",
            EventKind::Ordinary(e) => convergence_code(e.status),
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

fn convergence_code(status: Option<EventStatus>) -> &'static str {
    match status {
        Some(EventStatus::Convergent) => "1",
        Some(EventStatus::Anticipated) => "2",
        Some(EventStatus::Ordinary) | None => "0",
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_event_decl(
    ids: &RodinIds,
    file_root: &HandleUri,
    kind: EventKind<'_>,
    base_env: &TypeEnv,
    parent: Option<&CheckedMachine>,
    abstract_only: &BTreeSet<String>,
    diags: &mut Vec<Diagnostic>,
    machine_name: &str,
) -> Option<(EventDecl, bool)> {
    let label = kind.label();
    let source = crate::sc::file_child_source(ids, file_root, Kind::Event, in_tag::EVENT, label);

    let (effective_refines, parent_event_decl) = resolve_effective_refines(kind, parent);

    // Explicit refines target missing from parent — drop to match Rodin's
    // silent-drop behaviour. (Implicit and INIT are already gated upstream.)
    if let Some(refines) = kind.explicit_refines()
        && parent_event_decl.is_none()
    {
        diags.push(Diagnostic {
            severity: Severity::Warning,
            origin: format!("{machine_name}.{label}"),
            message: format!("refines target '{refines}' not found in parent — event dropped"),
            rule_id: Some(crate::RuleId::CrossReferenceNotFound),
            span: kind.name_span(),
        });
        return None;
    }

    let inherited_chain: Option<Rc<EventDecl>> = if kind.extended() {
        parent_event_decl.map(Rc::clone)
    } else {
        None
    };

    let (mut scope, scope_accurate) = build_event_scope(
        base_env,
        inherited_chain.as_deref(),
        kind,
        diags,
        machine_name,
        label,
    );

    let (buckets, buckets_accurate) = build_event_buckets(
        ids,
        file_root,
        kind,
        &scope,
        abstract_only,
        diags,
        machine_name,
        label,
    );

    scope.pop_scope();

    let refines_decl = effective_refines.zip(parent).map(|(abs_label, parent_cm)| {
        build_refines_event_decl(
            ids,
            file_root,
            label,
            abs_label,
            parent_cm,
            kind.explicit_refines().is_some(),
        )
    });

    let accurate = scope_accurate && buckets_accurate;
    let decl = EventDecl {
        label: label.to_string(),
        convergence: kind.convergence(),
        extended: kind.extended(),
        accurate,
        source,
        refines: refines_decl,
        parameters: buckets.parameters,
        guards: buckets.guards,
        actions: buckets.actions,
        witnesses: buckets.witnesses,
        inherited: inherited_chain,
    };

    Some((decl, accurate))
}

/// Resolve `(effective_refines_label, parent_event_decl)` for `kind`.
/// INIT events implicitly refine the parent's INITIALISATION when one
/// exists; ordinary events prefer the explicit `refines` annotation but
/// fall back to an implicit same-label match when extended.
fn resolve_effective_refines<'a, 'b>(
    kind: EventKind<'a>,
    parent: Option<&'b CheckedMachine>,
) -> (Option<&'a str>, Option<&'b Rc<EventDecl>>) {
    let effective_refines: Option<&str> = match kind {
        EventKind::Init(_) => parent
            .filter(|p| p.events_by_label.contains_key("INITIALISATION"))
            .map(|_| "INITIALISATION"),
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
/// parameter could not be typed). The caller is responsible for the
/// matching `pop_scope`.
fn build_event_scope(
    base_env: &TypeEnv,
    inherited_chain: Option<&EventDecl>,
    kind: EventKind<'_>,
    diags: &mut Vec<Diagnostic>,
    machine_name: &str,
    label: &str,
) -> (TypeEnv, bool) {
    let mut scope = base_env.clone();
    scope.push_scope();
    if let Some(pe) = inherited_chain {
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
    if let Some(pe) = inherited_chain {
        for p in pe.typing_guard_predicates() {
            axioms.push(p.clone());
        }
    }
    for g in kind.guards() {
        axioms.push(g.predicate.clone());
    }
    let param_names: Vec<String> = kind.parameters().iter().map(|p| p.name.clone()).collect();
    let unresolved = infer_constants(&mut scope, &param_names, &axioms);
    let mut accurate = true;
    for name in &unresolved {
        diags.push(Diagnostic {
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

/// Per-event decl buckets produced by [`build_event_buckets`].
struct EventBuckets {
    parameters: Vec<ParameterDecl>,
    guards: Vec<GuardDecl>,
    actions: Vec<ActionDecl>,
    witnesses: Vec<WitnessDecl>,
}

/// Build the four per-event decl buckets (guards, parameters, actions,
/// witnesses), running per-clause checks (abstract-only references,
/// well-typedness, LHS-declared) and dropping any clause that fails. The
/// returned `bool` is the `accurate` flag for the event — `false` if any
/// clause was dropped.
#[allow(clippy::too_many_arguments)]
fn build_event_buckets(
    ids: &RodinIds,
    file_root: &HandleUri,
    kind: EventKind<'_>,
    scope: &TypeEnv,
    abstract_only: &BTreeSet<String>,
    diags: &mut Vec<Diagnostic>,
    machine_name: &str,
    label: &str,
) -> (EventBuckets, bool) {
    let mut accurate = true;

    let mut guards: Vec<GuardDecl> = Vec::with_capacity(kind.guards().len());
    for (i, g) in kind.guards().iter().enumerate() {
        // Abstract-only-reference drop: guard reads a variable that
        // vanished in this refinement (inherited from parent but not
        // redeclared, no witness). Rodin drops the guard and marks
        // the event `accurate=false` — see `ITERATION.bcm`'s
        // `stepone`/`steptwo` referencing `n`, `t` (Group R).
        if !abstract_only.is_empty()
            && let Some(bad) = crate::sc::identifier_walker::first_forbidden_identifier_in_predicate(
                &g.predicate,
                abstract_only,
            )
        {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                origin: format!(
                    "{}.{}.{}",
                    machine_name,
                    label,
                    g.label.as_deref().unwrap_or("grd"),
                ),
                message: format!("guard references abstract-only variable '{bad}' — dropped"),
                rule_id: Some(crate::RuleId::UndeclaredIdentifier),
                span: g.span,
            });
            accurate = false;
            continue;
        }
        // Per-clause well-typedness drop: catches things like
        // `a ∈ AUCTIONS ↦ item` where `AUCTIONS ↦ item` is a pair, not
        // a set. Rodin emits the event `accurate=false` and skips the
        // guard.
        if !crate::wellformed::is_well_typed_predicate(scope, &g.predicate) {
            diags.push(Diagnostic {
                severity: Severity::Error,
                origin: format!(
                    "{}.{}.{}",
                    machine_name,
                    label,
                    g.label.as_deref().unwrap_or("grd"),
                ),
                message: "guard predicate is ill-typed".to_string(),
                rule_id: Some(crate::RuleId::TypeError),
                span: g.span,
            });
            accurate = false;
            continue;
        }
        match build_guard_decl(ids, file_root, label, i, g, scope, machine_name) {
            Ok(d) => guards.push(d),
            Err(diag) => {
                diags.push(diag);
                accurate = false;
            }
        }
    }

    let mut params_sorted: Vec<&NamedElement> = kind
        .parameters()
        .iter()
        .filter(|p| scope.contains(&p.name))
        .collect();
    params_sorted.sort_by(|a, b| a.name.cmp(&b.name));
    let mut parameters: Vec<ParameterDecl> = Vec::with_capacity(params_sorted.len());
    for p in params_sorted {
        parameters.push(build_parameter_decl(ids, file_root, label, p, scope));
    }

    // Cascade-drop: an action whose LHS targets a variable that wasn't
    // typed (and so dropped from env) is meaningless — Rodin emits the
    // enclosing event without it and marks the event `accurate=false`.
    // We mirror that.
    let mut actions: Vec<ActionDecl> = Vec::with_capacity(kind.actions().len());
    for (i, act) in kind.actions().iter().enumerate() {
        let bad_lhs = lhs_variables(&act.action)
            .iter()
            .find(|v| !scope.contains(v))
            .map(|v| v.to_string());
        if let Some(bad) = bad_lhs {
            diags.push(Diagnostic {
                severity: Severity::Error,
                origin: format!(
                    "{}.{}.{}",
                    machine_name,
                    label,
                    act.label.as_deref().unwrap_or("act"),
                ),
                message: format!("LHS variable '{bad}' is not declared"),
                rule_id: Some(crate::RuleId::UndeclaredIdentifier),
                span: act.span,
            });
            accurate = false;
            continue;
        }
        // Abstract-only-reference drop: action writes to a vanished
        // variable, or reads one in its RHS / generalised-assignment
        // predicate. Rodin drops the action and marks the event
        // `accurate=false` (Group R).
        if !abstract_only.is_empty()
            && let Some(bad) = first_abstract_only_in_action(&act.action, abstract_only)
        {
            diags.push(Diagnostic {
                severity: Severity::Warning,
                origin: format!(
                    "{}.{}.{}",
                    machine_name,
                    label,
                    act.label.as_deref().unwrap_or("act"),
                ),
                message: format!("action references abstract-only variable '{bad}' — dropped"),
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
        if !crate::wellformed::is_well_typed_action(scope, &act.action) {
            diags.push(Diagnostic {
                severity: Severity::Error,
                origin: format!(
                    "{}.{}.{}",
                    machine_name,
                    label,
                    act.label.as_deref().unwrap_or("act"),
                ),
                message: "action is ill-typed".to_string(),
                rule_id: Some(crate::RuleId::TypeError),
                span: act.span,
            });
            accurate = false;
            continue;
        }
        actions.push(build_action_decl(ids, file_root, label, i, act, scope));
    }

    // Witnesses in `witnesses`-then-`with` order; the index is the source
    // position the well-definedness pass uses to pair these back up.
    let mut witnesses: Vec<WitnessDecl> = Vec::new();
    for (i, w) in kind
        .witnesses_primary()
        .iter()
        .chain(kind.witnesses_with().iter())
        .enumerate()
    {
        witnesses.push(build_witness_decl(ids, file_root, label, i, w));
    }

    (
        EventBuckets {
            parameters,
            guards,
            actions,
            witnesses,
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
    env: &TypeEnv,
) -> ActionDecl {
    let label = act.label.clone().unwrap_or_else(|| "act".to_string());
    // `check_action` returns a free-ident slot we intentionally drop:
    // emitting it would fire false positives for `:|` (becomes-such-
    // that) actions, whose predicates reference primed forms (`x'`)
    // that the identifier walker doesn't yet recognise as bound.
    // Wiring up primed-name handling and switching this to a real
    // diagnostic is a separate task.
    let ac = check_action(&act.action, env);
    let _ = ac.free_identifier;
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
        action: ac.action,
        canonical: ac.canonical,
        source,
    }
}

fn build_witness_decl(
    ids: &RodinIds,
    file_root: &HandleUri,
    event_label: &str,
    source_index: usize,
    w: &LabeledPredicate,
) -> WitnessDecl {
    let label = w.label.clone().unwrap_or_else(|| "wit".to_string());
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
        &label,
    );
    WitnessDecl {
        label,
        source_index,
        predicate_canonical: canonical,
        source,
    }
}

fn build_refines_event_decl(
    ids: &RodinIds,
    file_root: &HandleUri,
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
    // Defensive escape: in practice user-visible event labels don't
    // contain `|`/`\`/`/`, but apply the same escape rules as
    // `HandleUri::child` so anything weird round-trips correctly.
    let sc_target = format!(
        "/{proj}/{file}|org.eventb.core.scMachineFile#{mach}|org.eventb.core.scEvent#{abs}",
        proj = file_root_project(file_root),
        file = parent.output_filename(),
        mach = parent.name(),
        abs = crate::handles::escape_handle_id_owned(abstract_event_label),
    );
    RefinesEventDecl {
        abstract_label: abstract_event_label.to_string(),
        sc_target,
        source: re_source,
    }
}

/// Extract the project name from a file-root URI
/// (`/PROJECT/File.bum|...`).
fn file_root_project(file_root: &HandleUri) -> &str {
    let s = file_root.as_str();
    let rest = s.strip_prefix('/').unwrap_or(s);
    rest.split('/').next().unwrap_or("proj")
}

/// First name in `action` that hits a variable in `forbidden`, checked
/// across both the LHS targets and the RHS / generalised-assignment
/// predicate. Returns `None` if the action is clean. Drives the
/// abstract-only-reference cascade drop in [`build_event_decl`].
fn first_abstract_only_in_action(
    action: &rossi::Action,
    forbidden: &BTreeSet<String>,
) -> Option<String> {
    if let Some(v) = lhs_variables(action)
        .into_iter()
        .find(|v| forbidden.contains(*v))
    {
        return Some(v.to_string());
    }
    crate::sc::identifier_walker::first_forbidden_identifier_in_action_rhs(action, forbidden)
}

pub(super) use crate::ast_util::lhs_variables;
