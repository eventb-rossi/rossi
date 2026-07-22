//! Machine static checker: `.bum` → `.bcm`.
//!
//! Mirrors [`super::context`]: builds a [`MachineRecord`] of typed
//! decls (no XML), then renders to an `<scMachineFile>` via
//! [`render_machine_root`]. The render layer is shared with
//! descendants — invariants travel as already-rendered XML
//! ([`CheckedMachine::invariant_elems`]); event children travel as
//! typed [`EventDecl`] chains keyed by label.
//!
//! Event-scoped builders live in [`events`]; this module owns the
//! file-scoped wiring (env setup, REFINES/SEES/variable/invariant/
//! variant decls) and the orchestration loop.

mod events;

use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use rossi::{LabeledPredicate, Machine};

use crate::checked_predicate::{check_expression, check_labeled_predicate};
use crate::handles::HandleUri;
use crate::infer::{check_expression_type, infer_constants};
use crate::project::{Project, ProjectComponent};
use crate::rodin_ids::{Kind, RodinIds};
use crate::type_env::TypeEnv;
use crate::types::Type;
use crate::xml_out::{Element, attr, in_tag, tag};
use crate::{Diagnostic, ScFile, Severity};

use super::machine_record::{
    EventDecl, InvariantDecl, MachineRecord, RefinesMachineDecl, RenderedMachine, SeesContextDecl,
    VariableDecl, VariantDecl, render_machine_root,
};
use super::{CheckedContext, CheckedMachine};

use self::events::{EventKind, MachineCheckContext, build_event_decl};

/// Emit a `.bcm` for a single machine.
pub fn check_machine(
    project: &Project,
    pc: &ProjectComponent,
    machine: &Machine,
    checked_contexts: &HashMap<String, CheckedContext>,
    checked_machines: &HashMap<String, CheckedMachine>,
) -> (ScFile, CheckedMachine, Vec<Diagnostic>) {
    // Decomposition base machines: replace the standard SC pipeline with an
    // attribute-only stub that mirrors what the ETH Zurich Decomposition
    // plugin produces.
    //
    // The configuration `ch.ethz.eventb.decomposition.mchBase` is a real,
    // current Rodin extension defined in
    // `trunk/ModelDecomposition/ch.ethz.eventb.decomposition/plugin.xml`
    // (rodin-b-sharp SVN; core last touched 2014-11-28, feature/UI
    // 2017-07-04). The constant is `EventBUtils.DECOMPOSITION_CONFIG_SC`.
    // The plugin defines only `mchBase`; no `ctxBase` analog exists.
    //
    // The `mchBase` SC pipeline runs five modules — none of which emit any
    // `.bcm` body content:
    //   * `machineModule`  — no body output; on `decomposed=true`, sets
    //                        `pogConfig` on a temp file (POG-only, never
    //                        appears in our `.bcm`).
    //   * `contextModule`  — same pattern, for decomposed contexts.
    //   * `refinesModule`  — *validates* refinement consistency: shared
    //                        variables retained with `nature=1`, external
    //                        events preserved (`external`/`extended`, no
    //                        extra params/guards/actions), INITIALISATION
    //                        actions don't mix private and shared, abstract
    //                        shared-var init actions are present in the
    //                        concrete. Failures emit error markers, not XML.
    //   * `varModule`      — annotates each variable's symbol-table entry
    //                        with NATURE_ATTRIBUTE. No body output.
    //   * `evtModule`      — annotates each event's symbol-table entry with
    //                        EXTERNAL_ATTRIBUTE. No body output.
    //
    // So an attribute-only `<scMachineFile>` is the *correct* output. The
    // empty stub is byte-exact against every decomposition-stub `.bcm`
    // in the corpus
    // and is not a shortcut around missing emission logic.
    //
    // What we deliberately *don't* mirror: `refinesModule`'s diagnostic
    // checks. Those would surface decomposition-refinement bugs as error
    // diagnostics, but they don't change the emitted XML — so they don't
    // change cross-validation outcomes. Implementing them would also
    // require extending the parser to capture `nature` / `external` /
    // `decomposed` annotations on AST nodes, which no other consumer
    // currently needs. Revisit if a corpus model surfaces a decomposition
    // bug that this would catch, or if downstream tooling needs the
    // annotations.
    if let Some(cfg) = machine
        .metadata
        .as_ref()
        .and_then(|m| m.configuration.as_deref())
        && is_decomposition_stub_config(cfg)
    {
        return emit_decomposition_stub(pc, &machine.name, cfg);
    }

    let mut diags = Vec::new();
    let mut accurate = true;

    // -----------------------------------------------------------------
    // Environment: SEES contexts + REFINES parent.
    // -----------------------------------------------------------------
    let mut env = TypeEnv::new();
    for sees_name in &machine.sees {
        match checked_contexts.get(sees_name) {
            Some(ctx) => {
                for (k, v) in ctx.env().iter() {
                    env.insert_if_absent(k, v.clone());
                }
                // Seeing an inaccurate context makes this machine
                // inaccurate too (silent — the inaccuracy is the context's
                // own reported problem).
                if !ctx.accurate {
                    accurate = false;
                }
            }
            None => {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    origin: machine.name.clone(),
                    message: format!("SEES unknown context '{sees_name}'"),
                    rule_id: Some(crate::RuleId::CrossReferenceNotFound),
                    // SEES targets carry no per-entry span; anchor on the
                    // machine name.
                    span: machine.name_span,
                });
                accurate = false;
            }
        }
    }

    let parent: Option<&CheckedMachine> = machine
        .refines
        .as_deref()
        .and_then(|n| checked_machines.get(n));
    if let Some(parent_name) = &machine.refines
        && parent.is_none()
    {
        diags.push(Diagnostic {
            severity: Severity::Error,
            origin: machine.name.clone(),
            message: format!("REFINES unknown machine '{parent_name}'"),
            rule_id: Some(crate::RuleId::CrossReferenceNotFound),
            span: machine.name_span,
        });
        accurate = false;
    }
    if let Some(p) = parent {
        for (k, v) in p.env().iter() {
            env.insert_if_absent(k, v.clone());
        }
        // Refining an inaccurate machine makes this machine inaccurate too
        // (silent — the inaccuracy is the abstract machine's own reported
        // problem).
        if !p.accurate {
            accurate = false;
        }
    }

    // -----------------------------------------------------------------
    // Filter duplicate variables / invariant / event labels per the SC drop
    // semantics documented in `crate::duplicates` (variables and event
    // labels drop every occurrence; invariant labels keep the first).
    // -----------------------------------------------------------------
    let file_dups = crate::duplicates::machine_file_duplicates(machine);
    let dup_vars = file_dups.variables.names;
    let dup_inv_labels = file_dups.invariant_labels.names;
    let dup_event_labels = file_dups.event_labels.names;

    // -----------------------------------------------------------------
    // Type-infer variables from invariants.
    // -----------------------------------------------------------------
    let variable_names: Vec<String> = machine
        .variables
        .iter()
        .filter(|v| !dup_vars.contains(&v.name))
        .map(|v| v.name.clone())
        .collect();
    // Dropped 2nd+ occurrences of a duplicated invariant label must not
    // contribute typing either; the kept first occurrence still types.
    let mut typing_kept = crate::duplicates::FirstKept::new(&dup_inv_labels);
    let invariant_preds: Vec<_> = machine
        .invariants
        .iter()
        .filter(|inv| !typing_kept.drops(inv.label.as_deref()))
        .map(|i| i.predicate.clone())
        .collect();
    let unresolved = infer_constants(&mut env, &variable_names, &invariant_preds);
    for name in &unresolved {
        // An untyped variable is an error in Rodin (UntypedVariableError,
        // MachineCommitIdentsModule): the variable is dropped from the
        // output. The file still stays `accurate="true"` — untyped
        // variables are an event-level accuracy concern, and the
        // cascade-drop in `events.rs` already marks each event that
        // references such a variable `accurate=false` (confirmed on a
        // corpus tutorial model).
        diags.push(Diagnostic {
            severity: Severity::Error,
            origin: format!("{}.{}", machine.name, name),
            message: "could not infer variable type from invariants".to_string(),
            rule_id: Some(crate::RuleId::TypeError),
            span: crate::ast_util::named_element_span(&machine.variables, name),
        });
    }

    // -----------------------------------------------------------------
    // Build typed decls.
    // -----------------------------------------------------------------
    let file_root = HandleUri::root(
        &project.name,
        &pc.filename,
        in_tag::MACHINE_FILE,
        &machine.name,
    );
    let configuration = machine
        .metadata
        .as_ref()
        .and_then(|m| m.configuration.clone())
        .unwrap_or_else(|| "org.eventb.core.fwd".to_string());

    let refines_decl =
        build_refines_machine_decl(machine, parent, project, &pc.rodin_ids, &file_root);
    let sees_decls = build_sees_decls(
        machine,
        checked_contexts,
        project,
        &pc.rodin_ids,
        &file_root,
    );

    let mut invariant_decls: Vec<InvariantDecl> = Vec::with_capacity(machine.invariants.len());
    let mut emit_kept = crate::duplicates::FirstKept::new(&dup_inv_labels);
    for (i, inv) in machine.invariants.iter().enumerate() {
        // 2nd+ use of a duplicated invariant label: Rodin keeps the first
        // occurrence, drops the rest, and marks the file inaccurate (the
        // EB022 error is already reported).
        if emit_kept.drops(inv.label.as_deref()) {
            accurate = false;
            continue;
        }
        match build_invariant_decl(&pc.rodin_ids, &file_root, i, inv, &env, &machine.name) {
            Ok(d) => invariant_decls.push(d),
            Err(diag) => {
                diags.push(diag);
                accurate = false;
            }
        }
    }

    let (variable_decls, all_var_names, own_var_names) =
        build_variable_decls(machine, &dup_vars, &env, parent, &pc.rodin_ids, &file_root);

    // Variables inherited from the parent but not redeclared in this
    // machine vanish to abstract-only. Concrete events that reference such
    // a variable drop the offending clause and are marked inaccurate. An
    // extended INITIALISATION that would inherit a parent action on a
    // vanished variable is omitted entirely: an extended event cannot drop
    // part of its inherited action set, so Rodin marks it erroneous and
    // emits no scEvent (confirmed against a real refinement in the corpus).
    let abstract_only_var_names: BTreeSet<String> = parent
        .map(|p| {
            p.visible_variables
                .iter()
                .filter(|n| !own_var_names.contains(n.as_str()))
                .cloned()
                .collect()
        })
        .unwrap_or_default();

    // A machine without a usable variant cannot host convergent events:
    // each is downgraded to ordinary (and marked inaccurate). `variant_usable`
    // is the single machine-wide flag that feeds every event's convergence.
    let (variant_decl, variant_usable) = match &machine.variant {
        Some(expr) => {
            match build_variant_decl(&pc.rodin_ids, &file_root, expr, &env, &machine.name) {
                Ok((decl, usable)) => (Some(decl), usable),
                Err(diag) => {
                    diags.push(diag);
                    accurate = false;
                    (None, false)
                }
            }
        }
        None => (None, false),
    };

    // This machine's concrete (own-declared), typed variables, in the same
    // alphabetical order as the emitted scVariables. These are the candidates
    // the INITIALISATION repair gives a default `becomesSuchThat ⊤` when no
    // action covers them; deriving from `variable_decls` keeps the repair set
    // and the emitted variables a single source.
    let concrete_typed_vars: Vec<String> = variable_decls
        .iter()
        .filter(|d| d.is_concrete)
        .map(|d| d.name.clone())
        .collect();

    let event_context = MachineCheckContext {
        ids: &pc.rodin_ids,
        file_root: &file_root,
        project_name: &project.name,
        base_env: &env,
        parent,
        abstract_only: &abstract_only_var_names,
        declared_variable_names: &all_var_names,
        variant_usable,
        concrete_vars: &concrete_typed_vars,
        machine_name: &machine.name,
    };

    // Events — build typed decls; insert each into the per-label map
    // so descendants extending it can pick up the typed parent chain.
    let mut event_decls: Vec<Rc<EventDecl>> = Vec::new();
    let mut events_by_label: HashMap<String, Rc<EventDecl>> = HashMap::new();
    // Per-event accuracy stays inside `EventDecl::accurate`; it doesn't bubble
    // up to the file-level flag. An untyped variable, by itself, is likewise
    // not a file-level signal; each event that references it is marked
    // inaccurate instead. An event whose explicit refines target is absent
    // from the parent is dropped with an error (Rodin parity), not a
    // file-inaccuracy signal.
    // A duplicated event label drops every conflicting event — Rodin's
    // event-label-conflict rule (both symbols error out, no scEvent is
    // committed for either; the machine root stays accurate). A descendant
    // refining a dropped event hits the dropped-target path in `events.rs`.
    // The dropped event's own EB021/EB022 diagnostics were already reported
    // up front, so skipping the build here loses nothing.
    if let Some(init) = &machine.initialisation
        && !dup_event_labels.contains(crate::sc::initialisation_label())
        && !should_omit_initialisation(init, parent, &abstract_only_var_names)
        && let Some((decl, _ok)) =
            build_event_decl(event_context, EventKind::Init(init), &mut diags)
    {
        let rc = Rc::new(decl);
        events_by_label.insert(
            crate::sc::initialisation_label().to_string(),
            Rc::clone(&rc),
        );
        event_decls.push(rc);
    }
    for event in &machine.events {
        if dup_event_labels.contains(&event.name) {
            continue;
        }
        if let Some((decl, _ok)) =
            build_event_decl(event_context, EventKind::Ordinary(event), &mut diags)
        {
            let rc = Rc::new(decl);
            events_by_label.insert(event.name.clone(), Rc::clone(&rc));
            event_decls.push(rc);
        }
    }

    // Ancestors closure.
    let mut ancestors: Vec<String> = Vec::new();
    if let Some(p) = parent {
        ancestors.extend(p.ancestors().iter().cloned());
        ancestors.push(p.name().to_string());
    }
    let visible_variables: BTreeSet<String> = all_var_names
        .into_iter()
        .filter(|n| env.contains(n))
        .collect();

    // Assemble the record — the single home for the machine's name,
    // output filename, environment and ancestor closure.
    let record = MachineRecord {
        name: machine.name.clone(),
        output_filename: pc.output_filename(),
        env,
        configuration,
        refines: refines_decl,
        sees: sees_decls,
        variables: variable_decls,
        invariants: invariant_decls,
        variant: variant_decl,
        events: event_decls,
        ancestors,
    };

    // -----------------------------------------------------------------
    // Render. The internal-context list and the parent's full
    // invariant closure are external inputs to the renderer.
    // -----------------------------------------------------------------
    let internal_contexts: Vec<Rc<Element>> =
        build_internal_context_elements(machine, checked_contexts);
    let inherited_invariants: &[Rc<Element>] =
        parent.map(|p| p.invariant_elems.as_slice()).unwrap_or(&[]);

    let RenderedMachine {
        root,
        own_invariants,
        event_elems,
    } = render_machine_root(
        &record,
        accurate,
        &internal_contexts,
        inherited_invariants,
        parent.map(|p| &p.event_elems),
    );

    // -----------------------------------------------------------------
    // Cache the full invariant closure for descendants. Own invariants and
    // events come directly from the renderer so generated identities are not
    // reconstructed by scanning or replaying XML.
    // -----------------------------------------------------------------
    let mut full_invariant_elems: Vec<Rc<Element>> = parent
        .map(|p| p.invariant_elems.clone())
        .unwrap_or_default();
    full_invariant_elems.extend(own_invariants);

    let cm = CheckedMachine {
        record,
        visible_variables,
        invariant_elems: full_invariant_elems,
        events_by_label,
        event_elems,
        accurate,
    };

    (
        ScFile {
            filename: pc.output_filename(),
            contents: root.to_document(),
            accurate,
        },
        cm,
        diags,
    )
}

/// An extended child INITIALISATION inherits its parent's INIT actions
/// wholesale. If the parent assigns a variable that vanished here (declared
/// in the parent, not redeclared, no witness given), the inherited action
/// references a disappeared variable. An extended event cannot drop part of
/// its inherited action set, so Rodin marks the event erroneous and emits
/// no scEvent; we match by omitting the child INITIALISATION.
///
/// `parent_init.actions` is the parent's full effective action list
/// (inherited ++ own, plus any generated repair), so an assignment a
/// grandparent contributed up the chain is covered too.
fn should_omit_initialisation(
    init: &rossi::InitialisationEvent,
    parent: Option<&CheckedMachine>,
    abstract_only: &BTreeSet<String>,
) -> bool {
    if !init.extended {
        return false;
    }
    let Some(parent_cm) = parent else {
        return false;
    };
    let Some(parent_init) = parent_cm
        .events_by_label
        .get(crate::sc::initialisation_label())
    else {
        return false;
    };
    parent_init.actions.iter().any(|a| {
        events::lhs_variables(&a.action)
            .iter()
            .any(|v| abstract_only.contains(*v))
    })
}

// =====================================================================
// File-scoped decl builders
// =====================================================================

fn build_refines_machine_decl(
    machine: &Machine,
    parent: Option<&CheckedMachine>,
    project: &Project,
    ids: &RodinIds,
    file_root: &HandleUri,
) -> Option<RefinesMachineDecl> {
    let parent_name = machine.refines.as_deref()?;
    let parent_cm = parent?;
    let source = crate::sc::file_child_source(
        ids,
        file_root,
        Kind::RefinesMachine,
        in_tag::REFINES_MACHINE,
        parent_name,
    );
    let sc_target = HandleUri::file(&project.name, parent_cm.output_filename()).into();
    Some(RefinesMachineDecl {
        parent_name: parent_name.to_string(),
        sc_target,
        source,
    })
}

fn build_sees_decls(
    machine: &Machine,
    checked_contexts: &HashMap<String, CheckedContext>,
    project: &Project,
    ids: &RodinIds,
    file_root: &HandleUri,
) -> Vec<SeesContextDecl> {
    let mut out = Vec::with_capacity(machine.sees.len());
    for sees_name in &machine.sees {
        let Some(ctx) = checked_contexts.get(sees_name) else {
            continue;
        };
        let source = crate::sc::file_child_source(
            ids,
            file_root,
            Kind::SeesContext,
            in_tag::SEES_CONTEXT,
            sees_name,
        );
        let sc_target = HandleUri::file(&project.name, ctx.output_filename()).into();
        out.push(SeesContextDecl {
            name: sees_name.clone(),
            sc_target,
            source,
        });
    }
    out
}

fn build_invariant_decl(
    ids: &RodinIds,
    file_root: &HandleUri,
    source_index: usize,
    inv: &LabeledPredicate,
    env: &TypeEnv,
    machine_name: &str,
) -> std::result::Result<InvariantDecl, Diagnostic> {
    let (label, pc) = check_labeled_predicate(inv, env, "inv", "invariant", |lbl| {
        format!("{machine_name}.{lbl}")
    })?;
    let source =
        crate::sc::file_child_source(ids, file_root, Kind::Invariant, in_tag::INVARIANT, &label);
    Ok(InvariantDecl {
        label,
        source_index,
        predicate: pc.predicate,
        predicate_canonical: pc.canonical,
        is_theorem: inv.is_theorem,
        source,
    })
}

/// Returns `(decls, all_var_names, own_var_names)`. `all_var_names`
/// is the union of own + parent-inherited variables; `own_var_names`
/// is just the current machine's own declarations. The caller needs
/// `own_var_names` to compute `abstract_only_var_names`; returning it
/// here avoids recomputing.
fn build_variable_decls(
    machine: &Machine,
    dup_vars: &BTreeSet<String>,
    env: &TypeEnv,
    parent: Option<&CheckedMachine>,
    ids: &RodinIds,
    file_root: &HandleUri,
) -> (Vec<VariableDecl>, BTreeSet<String>, BTreeSet<String>) {
    // A duplicated variable (EB021) is dropped from the machine's own
    // declarations entirely; if an ancestor also declares the name it
    // consequently lands in `abstract_only_var_names` and referencing
    // clauses get the disappeared-variable treatment.
    let own_var_names: BTreeSet<String> = machine
        .variables
        .iter()
        .filter(|v| !dup_vars.contains(&v.name))
        .map(|v| v.name.clone())
        .collect();
    let parent_var_names: BTreeSet<String> = parent
        .map(|p| p.visible_variables.clone())
        .unwrap_or_default();
    let all_var_names: BTreeSet<String> = own_var_names
        .iter()
        .chain(parent_var_names.iter())
        .cloned()
        .collect();

    let mut decls: Vec<VariableDecl> = all_var_names
        .iter()
        .filter(|n| env.contains(n.as_str()))
        .map(|n| {
            let ty = env.get(n).cloned().unwrap_or(Type::Integer);
            let source =
                crate::sc::file_child_source(ids, file_root, Kind::Variable, in_tag::VARIABLE, n);
            VariableDecl {
                name: n.clone(),
                ty,
                source,
                is_abstract: parent_var_names.contains(n),
                // A variable is concrete in this machine iff it's
                // declared in the current machine's own variable list.
                // Inherited-only variables vanish to abstract-only;
                // rodin-docker probe confirms the rule (Group R).
                is_concrete: own_var_names.contains(n),
            }
        })
        .collect();
    decls.sort_by(|a, b| a.name.cmp(&b.name));

    (decls, all_var_names, own_var_names)
}

/// Build the variant decl and report whether it is *usable* for the
/// convergence rule. A variant naming an out-of-scope identifier is emitted
/// but unusable, matching Rodin's convergence behavior. A closed, ill-typed
/// variant is rejected. rossi does not additionally enforce Rodin's "variant
/// is an integer or a finite set" requirement; an in-scope,
/// internally-consistent expression counts as usable.
fn build_variant_decl(
    ids: &RodinIds,
    file_root: &HandleUri,
    expr: &rossi::Expression,
    env: &TypeEnv,
    machine_name: &str,
) -> std::result::Result<(VariantDecl, bool), Diagnostic> {
    // Rodin's default variant label is "vrn"; our parser drops any
    // non-default label from the .bum (only Expression is preserved).
    let label = "vrn";
    let ec = check_expression(expr, env);
    let usable = ec.free_identifier.is_none();
    if usable && check_expression_type(env, &ec.expression, None).is_err() {
        return Err(Diagnostic {
            severity: Severity::Error,
            origin: format!("{machine_name}.{label}"),
            message: "variant expression is ill-typed".to_string(),
            rule_id: Some(crate::RuleId::TypeError),
            span: expr.span,
        });
    }
    let source =
        crate::sc::file_child_source(ids, file_root, Kind::Variant, in_tag::VARIANT, label);
    Ok((
        VariantDecl {
            label,
            expression: ec.expression,
            expression_canonical: ec.canonical,
            source,
        },
        usable,
    ))
}

// =====================================================================
// Helpers
// =====================================================================

/// All contexts this machine depends on, in hoist order:
/// ancestors-first, direct SEES-last, each appearing exactly once.
fn collect_seen_contexts(
    machine: &Machine,
    checked: &HashMap<String, CheckedContext>,
) -> Vec<String> {
    use std::collections::HashSet;
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for sees_name in &machine.sees {
        let Some(ctx) = checked.get(sees_name) else {
            continue;
        };
        for a in ctx.ancestors() {
            if seen.insert(a.clone()) {
                out.push(a.clone());
            }
        }
        if seen.insert(sees_name.clone()) {
            out.push(sees_name.clone());
        }
    }
    out
}

/// True for configurations whose Rodin SC pipeline (as installed by an
/// external plugin) produces an attribute-only stub `.bcm` rather than
/// a checked body. Currently the only such configuration is the ETH
/// Zurich Decomposition plugin's `mchBase` — see the call-site comment
/// in [`check_machine`] for the plugin reference. Composite values like
/// `org.eventb.core.fwd;…` are deliberately not matched: those still
/// run the standard forward SC alongside an extra module.
fn is_decomposition_stub_config(cfg: &str) -> bool {
    cfg == "ch.ethz.eventb.decomposition.mchBase"
}

/// Build the empty-stub `ScFile` and matching empty `CheckedMachine`
/// for a decomposition base machine. The XML matches Rodin's output
/// byte-for-byte (no `accurate` attribute, self-closing root).
fn emit_decomposition_stub(
    pc: &ProjectComponent,
    machine_name: &str,
    cfg: &str,
) -> (ScFile, CheckedMachine, Vec<Diagnostic>) {
    let root = Element::new(tag::SC_MACHINE_FILE).attr(attr::CONFIGURATION, cfg);
    let cm = CheckedMachine {
        record: MachineRecord {
            name: machine_name.to_string(),
            output_filename: pc.output_filename(),
            env: TypeEnv::new(),
            configuration: cfg.to_string(),
            refines: None,
            sees: Vec::new(),
            variables: Vec::new(),
            invariants: Vec::new(),
            variant: None,
            events: Vec::new(),
            ancestors: Vec::new(),
        },
        visible_variables: BTreeSet::new(),
        invariant_elems: Vec::new(),
        events_by_label: HashMap::new(),
        event_elems: HashMap::new(),
        // The stub's file-level `accurate=false` reflects an empty body,
        // not a checking error, so it must not taint a machine that
        // refines it. (Rodin emits no `accurate` attribute on the stub at
        // all; reading it back would error rather than yield `false`.)
        accurate: true,
    };
    let file = ScFile {
        filename: pc.output_filename(),
        contents: root.to_document(),
        accurate: false,
    };
    (file, cm, Vec::new())
}

/// Pre-render the `<scInternalContext>` rows that every seen context
/// (and its ancestors) contribute to our file. Caller forwards to the
/// machine renderer.
///
/// The inner `el.clone()` calls are now Rc::clones (O(1) refcount
/// bumps) since `ctx.extends_elems` / `ctx.body` are
/// `Vec<Rc<Element>>`. Each completed `<scInternalContext>` is wrapped
/// in `Rc::new` at the collecting boundary.
fn build_internal_context_elements(
    machine: &Machine,
    checked: &HashMap<String, CheckedContext>,
) -> Vec<Rc<Element>> {
    let mut out = Vec::new();
    for name in collect_seen_contexts(machine, checked) {
        let Some(ctx) = checked.get(&name) else {
            continue;
        };
        let mut ic = Element::new(tag::SC_INTERNAL_CONTEXT).attr(attr::NAME, name.as_str());
        for el in &ctx.extends_elems {
            ic.push(el.clone());
        }
        for el in &ctx.body {
            ic.push(el.clone());
        }
        out.push(Rc::new(ic));
    }
    out
}

#[cfg(test)]
mod tests {
    use rossi::Component;

    use super::*;
    use crate::sc_view::ScView;

    const PARENT: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.event name="_init0" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION"/>
<org.eventb.core.event name="_event0" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="abstract_step"/>
</org.eventb.core.machineFile>"#;

    const CHILD: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref1" org.eventb.core.target="M0"/>
<org.eventb.core.event name="_init1" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION">
<org.eventb.core.refinesEvent name="_init_ref1" org.eventb.core.target="INITIALISATION"/>
</org.eventb.core.event>
<org.eventb.core.event name="_event1" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="concrete_step">
<org.eventb.core.refinesEvent name="_event_ref1" org.eventb.core.target="abstract_step"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;

    #[test]
    fn event_inherited_missing_render_state_is_diagnosed() {
        let parent_project = Project::new(
            "missing",
            vec![ProjectComponent::from_xml("M0.bum", PARENT).unwrap()],
        );
        let (_, mut parent_model) = crate::build_with_model(&parent_project);
        let mut parent = parent_model.machines.remove("M0").expect("M0 checked");
        parent.event_elems.remove("abstract_step");

        let child_project = Project::new(
            "missing",
            vec![ProjectComponent::from_xml("M1.bum", CHILD).unwrap()],
        );
        let child = &child_project.components[0];
        let Component::Machine(machine) = &child.component else {
            panic!("M1 is a machine");
        };
        let checked_machines = HashMap::from([("M0".to_string(), parent)]);
        let (file, checked, diagnostics) = check_machine(
            &child_project,
            child,
            machine,
            &HashMap::new(),
            &checked_machines,
        );

        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.origin == "M1.concrete_step"
                && diagnostic.message
                    == "inherited event 'abstract_step' has no rendered parent state — event is inaccurate"
        }));
        let event = checked
            .events_by_label
            .get("concrete_step")
            .expect("concrete_step checked");
        assert!(!event.accurate);
        assert!(event.inherited.is_none());

        let view = ScView::from_xml(&file.contents).unwrap();
        let event = view
            .events
            .get("concrete_step")
            .expect("concrete_step rendered");
        assert!(!event.accurate);
        assert!(event.refines_events.is_empty());
    }
}
