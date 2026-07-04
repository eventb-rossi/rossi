//! Advisory lint passes over a parsed [`Project`].
//!
//! Unlike the SC pipeline in `crate::sc`, these passes don't read or write
//! the on-disk `.bcc`/`.bcm` representation — they walk the AST and emit
//! advisory [`Diagnostic`]s. `crate::build()` does **not** invoke [`run`];
//! callers (today: `rossi validate`) opt in explicitly so the existing
//! `rossi build` output stays stable.
//!
//! Coverage (stable `EBnnn` rule IDs):
//!
//! - **EB011** dead variable     — no use outside typing invariants, no
//!   event writes it
//! - **EB012** unmodified var    — never assigned outside INITIALISATION;
//!   a constant in disguise
//! - **EB014** incomplete INIT   — variable not assigned by INITIALISATION
//! - **EB021** duplicate identifier — variable/constant/set/parameter declared twice
//! - **EB022** duplicate label   — invariant/event/guard/action/axiom/witness used twice
//! - **EB023** shadowed name     — declared name re-lexes as a textual token
//! - **EB024** new event assigns inherited variable — a non-refining event
//!   modifies state inherited from an abstract machine
//!
//! EB019 (duplicate component names) is a project-integrity failure, not
//! advice: it is checked by the SC build itself (`crate::sc::build_project`),
//! so `rossi build` fails on it too. EB010 (well-definedness) and EB015–17
//! (proof status) are deliberately out of scope here; they need their own
//! modules.

use std::collections::{BTreeMap, BTreeSet};

use rossi::ast::Span;
use rossi::ast::predicate::{ComparisonOp, LogicalOp};
use rossi::{
    Component, Context, Event, ExpressionKind, InitialisationEvent, LabeledAction,
    LabeledPredicate, Machine, Predicate, PredicateKind,
};

use crate::ast_util::lhs_variables;
use crate::project::Project;
use crate::sc::identifier_walker::{
    collect_referenced_in_action_rhs, collect_referenced_in_action_rhs_with_locals,
    collect_referenced_in_expression, collect_referenced_in_predicate,
    collect_referenced_in_predicate_with_locals,
};
use crate::{Diagnostic, RuleId};

/// Run every available lint over `project` and collect the diagnostics.
#[must_use]
pub fn run(project: &Project) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let index = ProjectIndex::build(project);
    // `component_mach_ids` is parallel to `project.components` (both walk
    // the list once, in order), so every machine is judged against its own
    // arena entry even when another component shares its name (EB019).
    for (pc, mach_id) in project.components.iter().zip(&index.component_mach_ids) {
        diags.extend(run_component(&pc.component));
        let Some(id) = *mach_id else {
            continue; // contexts carry no cross-component lint data
        };
        let m = index.machs[id];
        // `None` when the REFINES walk didn't reach a root — see
        // `effective_refs_for_machine`.
        let inherited = index.inherited_vars_for_machine(id);
        if let Some(referenced) = index.effective_refs_for_machine(id) {
            let event_assigned = index.effective_event_assigned_for_machine(id);
            diags.extend(lint_dead_variable(
                m,
                &referenced,
                &event_assigned,
                &inherited,
            ));
            diags.extend(lint_unmodified_variable(
                m,
                &referenced,
                &event_assigned,
                &inherited,
            ));
        }
        diags.extend(lint_incomplete_init(
            m,
            index.init_inherited_assigned[id].as_ref(),
        ));
        diags.extend(lint_new_event_assigns_inherited(m, &inherited));
    }
    diags
}

/// Run the lints that need no cross-component context over one component.
/// Loose `.eventb` text files have no [`Project`] (a single file's SEES /
/// EXTENDS parents are usually absent, so the reference-based lints would
/// false-positive); these local passes are safe to run anywhere.
#[must_use]
pub fn run_component(component: &Component) -> Vec<Diagnostic> {
    match component {
        Component::Machine(m) => [
            lint_shadowed_names_machine(m),
            lint_duplicate_names_machine(m),
        ]
        .concat(),
        Component::Context(c) => [
            lint_shadowed_names_context(c),
            lint_duplicate_names_context(c),
        ]
        .concat(),
    }
}

// ---------- individual lint passes -----------------------------------------

/// EB011: a variable nothing reads (typing invariants aside — every Rodin
/// variable has one just to be typed) and no event writes serves no
/// purpose. A variable that IS written but never read is exempt: it is an
/// output, and deleting it would break the events that assign it. Like
/// EB012, fired once per variable, at the machine that declares it — a
/// kept copy down the chain would only repeat the same verdict.
fn lint_dead_variable(
    m: &Machine,
    referenced: &BTreeSet<&str>,
    event_assigned: &BTreeSet<&str>,
    inherited: &BTreeSet<&str>,
) -> Vec<Diagnostic> {
    m.variables
        .iter()
        .filter(|v| !inherited.contains(v.name.as_str()))
        .filter(|v| {
            !referenced.contains(v.name.as_str()) && !event_assigned.contains(v.name.as_str())
        })
        .map(|v| Diagnostic {
            severity: RuleId::DeadVariable.default_severity(),
            origin: format!("{}.{}", m.name, v.name),
            message: format!(
                "variable `{}` is never used — no reference outside typing \
                 invariants and no event assigns it",
                v.name
            ),
            rule_id: Some(RuleId::DeadVariable),
            span: v.span,
        })
        .collect()
}

/// EB012: a variable assigned by INITIALISATION and never modified by any
/// event is a constant in disguise — and provably stays one: a new event
/// refines `skip` and must not modify kept abstract state (EB024's proof
/// obligation), and a refining event must simulate an abstract event that
/// does not touch the variable, so no refinement that keeps it can ever
/// change it either. The rewrite always exists: Rodin rejects INIT actions
/// whose right-hand side reads a variable, so the initial value is a
/// constant expression (an axiom, after the move).
///
/// Fired once per variable, at the machine that declares it — kept copies
/// down the chain inherit the verdict. Requires a use beyond typing: a
/// never-used variable is EB011's dead case instead, so each variable
/// draws at most one of the two advisories.
fn lint_unmodified_variable(
    m: &Machine,
    referenced: &BTreeSet<&str>,
    event_assigned: &BTreeSet<&str>,
    inherited: &BTreeSet<&str>,
) -> Vec<Diagnostic> {
    let init_lhs = m
        .initialisation
        .as_ref()
        .map(init_lhs_names)
        .unwrap_or_default();
    m.variables
        .iter()
        .filter(|v| !inherited.contains(v.name.as_str()))
        .filter(|v| init_lhs.contains(v.name.as_str()))
        .filter(|v| !event_assigned.contains(v.name.as_str()))
        .filter(|v| referenced.contains(v.name.as_str()))
        .map(|v| Diagnostic {
            severity: RuleId::UnmodifiedVariable.default_severity(),
            origin: format!("{}.{}", m.name, v.name),
            message: format!(
                "variable `{}` is never assigned outside INITIALISATION — no \
                 event here or in any refinement can change it; consider a \
                 CONSTANT with the initialisation as an axiom",
                v.name
            ),
            rule_id: Some(RuleId::UnmodifiedVariable),
            span: v.span,
        })
        .collect()
}

/// The variable names an INITIALISATION's own actions assign — the single
/// definition of "assigned by INIT" that EB012 and EB014 both judge
/// against.
fn init_lhs_names(init: &InitialisationEvent) -> BTreeSet<&str> {
    init.actions
        .iter()
        .flat_map(|la| lhs_variables(&la.action))
        .collect()
}

fn lint_incomplete_init(m: &Machine, inherited_init: Option<&BTreeSet<String>>) -> Vec<Diagnostic> {
    let Some(init) = &m.initialisation else {
        // No INITIALISATION at all: report once per declared variable.
        return m
            .variables
            .iter()
            .map(|v| Diagnostic {
                severity: RuleId::IncompleteInitialisation.default_severity(),
                origin: format!("{}.INITIALISATION", m.name),
                message: format!(
                    "variable `{}` is not assigned by INITIALISATION (no INITIALISATION event)",
                    v.name
                ),
                rule_id: Some(RuleId::IncompleteInitialisation),
                span: v.span,
            })
            .collect();
    };

    // An `extended` INIT inherits the ancestor chain's assignments; its map
    // entry exists only when that chain fully resolved. Without one there
    // is no set to judge completeness against — stay silent; EB008/EB009
    // report the broken chain itself.
    if init.extended && inherited_init.is_none() {
        return Vec::new();
    }

    let lhs = init_lhs_names(init);
    m.variables
        .iter()
        .filter(|v| !lhs.contains(v.name.as_str()))
        .filter(|v| !inherited_init.is_some_and(|s| s.contains(&v.name)))
        .map(|v| Diagnostic {
            severity: RuleId::IncompleteInitialisation.default_severity(),
            origin: format!("{}.INITIALISATION", m.name),
            message: format!("variable `{}` is not assigned by INITIALISATION", v.name),
            rule_id: Some(RuleId::IncompleteInitialisation),
            span: v.span,
        })
        .collect()
}

/// EB024: a *new* event — one that neither REFINES nor EXTENDS an abstract
/// event — must not assign a variable inherited from an abstract machine and
/// *retained* in this refinement. A new event is implicitly a refinement of
/// `skip`, which changes no state, so modifying inherited state leaves the
/// refinement proof obligation unprovable. `inherited` is the set of variable
/// names visible from this machine's REFINES chain; it is empty for root
/// machines, so this pass never fires there.
///
/// The check is restricted to variables this machine still declares (`inherited
/// ∩ own`). An inherited variable the refinement *dropped* (data-refined away)
/// is the build-time [`RuleId::DisappearedVariable`] (EB025) error's domain;
/// flagging it here too would double-report it with contradictory advice.
/// INITIALISATION is excluded by construction — rossi stores it apart from
/// `m.events`, and it legitimately assigns inherited variables.
fn lint_new_event_assigns_inherited(m: &Machine, inherited: &BTreeSet<&str>) -> Vec<Diagnostic> {
    if inherited.is_empty() {
        return Vec::new();
    }
    // Variables inherited *and* kept here: assigning a dropped one is EB025.
    let retained: BTreeSet<&str> = m
        .variables
        .iter()
        .map(|v| v.name.as_str())
        .filter(|n| inherited.contains(n))
        .collect();
    if retained.is_empty() {
        return Vec::new();
    }
    let mut diags = Vec::new();
    for e in &m.events {
        // A refining or extending event legitimately refines an abstract
        // event that may change the variable; only genuinely new events are
        // constrained to leave inherited state untouched.
        if e.refines.is_some() || e.extended {
            continue;
        }
        // Report each inherited variable at most once per event, anchored on
        // the first action that assigns it.
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for la in &e.actions {
            for v in lhs_variables(&la.action) {
                if retained.contains(&v) && seen.insert(v) {
                    diags.push(Diagnostic {
                        severity: RuleId::NewEventAssignsInheritedVariable.default_severity(),
                        origin: format!("{}.{}", m.name, e.name),
                        message: format!(
                            "new event `{}` assigns inherited variable `{v}`; a new event \
                             refines skip and must not modify inherited state — REFINES the \
                             abstract event that changes `{v}`, or data-refine it",
                            e.name
                        ),
                        rule_id: Some(RuleId::NewEventAssignsInheritedVariable),
                        span: la.span,
                    });
                }
            }
        }
    }
    diags
}

/// EB023: a declared name that rossi's *textual* syntax can re-lex as a
/// token. The parser hard-rejects the kernel_lang §2.2 reserved words
/// ([`rossi::builtins::is_reserved_word`]) but deliberately accepts the rest
/// — Rodin allows them as identifiers, so imported models must load. The
/// trap is silent: a constant `POW` declares fine and `POW = f` works, but
/// `POW(f)` parses as the powerset `ℙ(f)`; a constant `NAT` can never be
/// referenced at all (`NAT` lexes as `ℕ`). Warn at the declaration.
fn shadowed_name_diag(
    component: &str,
    kind: &str,
    name: &str,
    span: Option<Span>,
) -> Option<Diagnostic> {
    if !rossi::builtins::is_reserved_name(name) || rossi::builtins::is_reserved_word(name) {
        return None;
    }
    Some(Diagnostic {
        severity: RuleId::ShadowedName.default_severity(),
        origin: format!("{component}.{name}"),
        message: format!(
            "{kind} `{name}` collides with rossi's textual operator vocabulary; \
             uses can silently parse as the built-in token instead of this \
             identifier (e.g. `POW(S)` is the powerset, `NAT` is ℕ) — rename it"
        ),
        rule_id: Some(RuleId::ShadowedName),
        span,
    })
}

/// Byte span of a set's *name*. The declaration span starts at the name but
/// runs through any trailing comment to the next declaration, which would
/// over-underline a name-level diagnostic — clip it to the name's length.
fn set_name_span(set: &rossi::SetDeclaration) -> Option<Span> {
    set.span().map(|s| Span {
        start: s.start,
        end: s.start + set.name().len(),
    })
}

fn lint_shadowed_names_context(c: &Context) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for set in &c.sets {
        diags.extend(shadowed_name_diag(
            &c.name,
            "carrier set",
            set.name(),
            set_name_span(set),
        ));
        // Enumerated elements have no per-element span; anchor on the set.
        for e in set.elements() {
            diags.extend(shadowed_name_diag(&c.name, "set element", e, set.span()));
        }
    }
    for k in &c.constants {
        diags.extend(shadowed_name_diag(&c.name, "constant", &k.name, k.span));
    }
    diags
}

fn lint_shadowed_names_machine(m: &Machine) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for v in &m.variables {
        diags.extend(shadowed_name_diag(&m.name, "variable", &v.name, v.span));
    }
    for event in &m.events {
        for p in &event.parameters {
            diags.extend(shadowed_name_diag(
                &format!("{}.{}", m.name, event.name),
                "parameter",
                &p.name,
                p.span,
            ));
        }
    }
    diags
}

// ---------- duplicate identifiers / labels (EB021 / EB022) ------------------
//
// Within the single scope where Event-B requires uniqueness, report
// identifiers (EB021) and labels (EB022) that occur more than once. Identifiers
// and labels are separate namespaces, so a variable `x` and an invariant
// labelled `x` do not collide. Cross-component shadowing is out of scope —
// that is EB023 / the type checker's scope rules.

/// `(label, span)` for each labelled predicate (invariant / guard / witness /
/// axiom); unlabelled clauses are skipped. Feeds [`duplicate_diags`].
fn pred_labels(preds: &[LabeledPredicate]) -> impl Iterator<Item = (&str, Option<Span>)> {
    preds
        .iter()
        .filter_map(|p| p.label.as_deref().map(|l| (l, p.span)))
}

/// `(label, span)` for each labelled action; unlabelled actions are skipped.
fn action_labels(actions: &[LabeledAction]) -> impl Iterator<Item = (&str, Option<Span>)> {
    actions
        .iter()
        .filter_map(|a| a.label.as_deref().map(|l| (l, a.span)))
}

/// One `Error` diagnostic per name that occurs more than once in `names`
/// (blank and whitespace-only names are skipped). Output is sorted by name for
/// determinism. The verb in the message follows the rule: identifiers are
/// "declared", labels "used".
fn duplicate_diags<'a>(
    names: impl IntoIterator<Item = (&'a str, Option<Span>)>,
    rule: RuleId,
    kind: &str,
    scope: &str,
    origin_prefix: &str,
) -> Vec<Diagnostic> {
    let verb = if rule == RuleId::DuplicateIdentifier {
        "declared"
    } else {
        "used"
    };
    // Count occurrences per name, remembering the first source span seen (the
    // declaration the reader should jump to). A later occurrence upgrades a
    // still-unknown span so a name spanned only on its second mention is still
    // located.
    let mut counts: BTreeMap<&str, (usize, Option<Span>)> = BTreeMap::new();
    for (n, span) in names {
        if n.trim().is_empty() {
            continue;
        }
        let entry = counts.entry(n).or_insert((0, None));
        entry.0 += 1;
        if entry.1.is_none() {
            entry.1 = span;
        }
    }
    counts
        .into_iter()
        .filter(|(_, (count, _))| *count > 1)
        .map(|(name, (count, span))| Diagnostic {
            severity: rule.default_severity(),
            origin: format!("{origin_prefix}.{name}"),
            message: format!("duplicate {kind} `{name}` in {scope} ({verb} {count} times)"),
            rule_id: Some(rule),
            span,
        })
        .collect()
}

fn lint_duplicate_names_machine(m: &Machine) -> Vec<Diagnostic> {
    let scope = format!("machine `{}`", m.name);
    let mut diags = Vec::new();

    // EB021 — variable identifiers.
    diags.extend(duplicate_diags(
        m.variables.iter().map(|v| (v.name.as_str(), v.span)),
        RuleId::DuplicateIdentifier,
        "variable identifier",
        &scope,
        &m.name,
    ));

    // EB022 — invariant labels.
    diags.extend(duplicate_diags(
        pred_labels(&m.invariants),
        RuleId::DuplicateLabel,
        "invariant label",
        &scope,
        &m.name,
    ));

    // EB022 — event labels. rossi stores INITIALISATION apart from `events`,
    // but Event-B treats it as an event sharing the label namespace.
    diags.extend(duplicate_diags(
        m.events.iter().map(|e| (e.name.as_str(), e.span)).chain(
            m.initialisation
                .as_ref()
                .map(|i| (crate::sc::initialisation_label(), i.span)),
        ),
        RuleId::DuplicateLabel,
        "event label",
        &scope,
        &m.name,
    ));

    // Per-event identifier / label namespaces.
    for e in &m.events {
        diags.extend(duplicate_names_in_event(
            &m.name,
            &e.name,
            e.parameters.iter().map(|p| (p.name.as_str(), p.span)),
            // Event-B shares one label namespace across guards and actions.
            pred_labels(&e.guards).chain(action_labels(&e.actions)),
            // rossi splits witnesses into `with` (abstract vars) + `witnesses`
            // (abstract params); Event-B treats them as one witness namespace.
            pred_labels(&e.with).chain(pred_labels(&e.witnesses)),
        ));
    }

    // INITIALISATION as an event: no parameters, no guards.
    if let Some(init) = &m.initialisation {
        diags.extend(duplicate_names_in_event(
            &m.name,
            crate::sc::initialisation_label(),
            std::iter::empty(),
            action_labels(&init.actions),
            pred_labels(&init.with).chain(pred_labels(&init.witnesses)),
        ));
    }

    diags
}

/// Check the three per-event namespaces (parameters; the shared guard+action
/// label space; the shared witness label space) for duplicates.
fn duplicate_names_in_event<'a>(
    machine: &str,
    event: &str,
    parameters: impl IntoIterator<Item = (&'a str, Option<Span>)>,
    guard_action_labels: impl IntoIterator<Item = (&'a str, Option<Span>)>,
    witness_labels: impl IntoIterator<Item = (&'a str, Option<Span>)>,
) -> Vec<Diagnostic> {
    let scope = format!("event `{event}` of machine `{machine}`");
    let origin = format!("{machine}.{event}");
    let mut diags = duplicate_diags(
        parameters,
        RuleId::DuplicateIdentifier,
        "parameter identifier",
        &scope,
        &origin,
    );
    diags.extend(duplicate_diags(
        guard_action_labels,
        RuleId::DuplicateLabel,
        "guard or action label",
        &scope,
        &origin,
    ));
    diags.extend(duplicate_diags(
        witness_labels,
        RuleId::DuplicateLabel,
        "witness label",
        &scope,
        &origin,
    ));
    diags
}

fn lint_duplicate_names_context(c: &Context) -> Vec<Diagnostic> {
    let scope = format!("context `{}`", c.name);
    let mut diags = Vec::new();

    // EB021 — carrier sets, their enumerated elements, and constants share one
    // identifier namespace, so a set and a constant with the same name collide.
    // (In Event-B, enumerated set elements are constants.) Enumerated elements
    // have no per-element span, so they anchor on the set declaration.
    let mut ids: Vec<(&str, Option<Span>)> = Vec::new();
    for set in &c.sets {
        ids.push((set.name(), set_name_span(set)));
        ids.extend(set.elements().iter().map(|e| (e.as_str(), set.span())));
    }
    ids.extend(c.constants.iter().map(|k| (k.name.as_str(), k.span)));
    diags.extend(duplicate_diags(
        ids,
        RuleId::DuplicateIdentifier,
        "carrier set or constant identifier",
        &scope,
        &c.name,
    ));

    // EB022 — axiom labels.
    diags.extend(duplicate_diags(
        pred_labels(&c.axioms),
        RuleId::DuplicateLabel,
        "axiom label",
        &scope,
        &c.name,
    ));

    diags
}

// ---------- reference collection -------------------------------------------
//
// Traversal lives in `crate::sc::identifier_walker`; this module wires the
// shared collectors through the machine/context AST. Event parameters are
// passed as initial bound names so a guard mentioning a parameter doesn't
// leak that name into the machine-level reference set.

/// Collect the identifiers `pred` references, except that a top-level
/// `∧`-conjunct of the typing shape `v ∈ E` / `v ⊆ E` (bare identifier on
/// the left, `E` free of machine variables) does not count as a reference
/// of `v` — only `E`'s identifiers are collected. Every Rodin variable
/// needs such an invariant just to be typed, so counting it as a use would
/// make a variable that exists only to be typed look alive. The shape
/// mirrors the bare-ident-LHS predicates the type checker derives types
/// from (`infer_constant_from_predicate`), deliberately narrowed: strict
/// subset, negations, and equality constrain the value beyond its type,
/// and a bound that mentions another VARIABLE (`cur ∈ dom(routes)`, a
/// gluing invariant's `abs ∈ ran(conc)`) relates dynamic state — those are
/// real uses, even though the type checker can also read a type out of
/// them. `vars` is the owning machine's variable set.
fn collect_non_typing_refs(pred: &Predicate, vars: &BTreeSet<&str>, acc: &mut BTreeSet<String>) {
    match &pred.kind {
        PredicateKind::Logical {
            op: LogicalOp::And,
            left,
            right,
        } => {
            collect_non_typing_refs(left, vars, acc);
            collect_non_typing_refs(right, vars, acc);
        }
        PredicateKind::Comparison {
            op: ComparisonOp::In | ComparisonOp::Subset,
            left,
            right,
        } if matches!(&left.kind, ExpressionKind::Identifier(_)) => {
            let mut bound_refs = BTreeSet::new();
            collect_referenced_in_expression(right, &mut bound_refs);
            let bound_is_static = bound_refs.iter().all(|r| !vars.contains(r.as_str()));
            acc.extend(bound_refs);
            if !bound_is_static {
                // The bound reads machine state: a constraint between
                // variables, not typing — the LHS occurrence counts too.
                collect_referenced_in_predicate(pred, acc);
            }
        }
        _ => collect_referenced_in_predicate(pred, acc),
    }
}

/// References appearing in `m`'s own invariants, typing conjuncts excluded
/// (see [`collect_non_typing_refs`]). Theorem invariants are collected in
/// full: a theorem is a derived property, not a typing declaration — even
/// a membership-shaped one states a fact about the variable, i.e. uses it.
/// Kept separate from [`machine_body_refs`] because descendants inherit
/// exactly this set (the SC splices ancestor invariants into every
/// concrete `.bcm`), so the upward pass reuses the per-machine result
/// instead of re-walking ancestor ASTs once per descendant.
fn invariant_refs(m: &Machine) -> BTreeSet<String> {
    let vars: BTreeSet<&str> = m.variables.iter().map(|v| v.name.as_str()).collect();
    let mut acc = BTreeSet::new();
    for inv in &m.invariants {
        if inv.is_theorem {
            collect_referenced_in_predicate(&inv.predicate, &mut acc);
        } else {
            collect_non_typing_refs(&inv.predicate, &vars, &mut acc);
        }
    }
    acc
}

/// References appearing in `m`'s variant, INITIALISATION, and events —
/// everything [`invariant_refs`] doesn't cover.
fn machine_body_refs(m: &Machine, acc: &mut BTreeSet<String>) {
    if let Some(v) = &m.variant {
        collect_referenced_in_expression(v, acc);
    }
    if let Some(init) = &m.initialisation {
        for la in &init.actions {
            collect_referenced_in_action_rhs(&la.action, acc);
        }
        for w in &init.with {
            collect_referenced_in_predicate(&w.predicate, acc);
        }
        for w in &init.witnesses {
            collect_referenced_in_predicate(&w.predicate, acc);
        }
    }
    for e in &m.events {
        let params: Vec<&str> = e.parameters.iter().map(|p| p.name.as_str()).collect();
        for g in &e.guards {
            collect_referenced_in_predicate_with_locals(&g.predicate, &params, acc);
        }
        for w in &e.with {
            collect_referenced_in_predicate_with_locals(&w.predicate, &params, acc);
        }
        for w in &e.witnesses {
            collect_referenced_in_predicate_with_locals(&w.predicate, &params, acc);
        }
        for la in &e.actions {
            collect_referenced_in_action_rhs_with_locals(&la.action, &params, acc);
        }
    }
}

/// Variable names assigned by `m`'s own events. INITIALISATION is
/// deliberately excluded: giving a variable its initial value is not
/// *modifying* it, and the split is what lets EB012 recognise a
/// constant-in-disguise (INIT-assigned, never modified) while EB011 treats
/// an initialised-but-never-used variable as dead all the same.
fn event_assigned_in_machine(m: &Machine) -> BTreeSet<String> {
    let mut acc = BTreeSet::new();
    for la in m.events.iter().flat_map(|e| &e.actions) {
        for v in lhs_variables(&la.action) {
            acc.insert(v.to_string());
        }
    }
    acc
}

// ---------- cross-chain index ----------------------------------------------
//
// A naive per-component lint produces false positives for any identifier
// declared in one component and used only in a transitive descendant: a
// constant `k` in context A used solely by `B extends A`, or a variable `v`
// in machine M1 read only by `M2 refines M1`. `ProjectIndex` builds, once
// per `lint::run` call, the inverted indexes needed to union references and
// assignments across the refinement/extension chain.
//
// The chain contributes in both directions. Downward, a descendant's own
// references keep an ancestor's declaration alive (`effective_*`). Upward,
// a machine's emitted `.bcm` materialises clauses it inherits from its
// REFINES ancestors — extended events splice in the abstract event's
// parameters/guards/actions, and an extended INITIALISATION splices in the
// abstract INIT actions (see `sc::machine_record::render_event`) — so those
// count as the machine's own references/assignments even though they are
// absent from its literal text.
//
// Chain links are names, and a name may be carried by several components
// (EB019's domain). The two directions treat that ambiguity differently.
// Downward data only ever SUPPRESSES warnings, so a child attaches to every
// same-name candidate — over-approximating is conservative. The upward walk
// becomes part of the judged machine's own materialised text, where guessing
// a duplicate could fabricate or mask warnings — so an ambiguous parent
// truncates the chain exactly like a missing one, and a machine whose walk
// truncated is exempt from the variable reference lints altogether (see
// [`run`]): its materialised sets are unknowable, and the broken link is
// already an Error (EB009 missing, EB007/EB008 circular, EB019 duplicated).

/// How a REFINES ancestor walk ended: at a true root machine, or truncated
/// by a parent name resolving to zero or several machines or by a circular
/// chain (EB009's, EB019's and EB008's domains — reported elsewhere).
#[derive(Clone, Copy, PartialEq)]
enum ChainEnd {
    Root,
    Truncated,
}

/// REFINES ancestors of the machine at `m_id`, nearest-first, cycle-guarded,
/// together with how the walk ended. Every link must resolve to exactly one
/// machine; see [`unique_id`].
fn ancestor_machines(
    m_id: MachId,
    machs: &[&Machine],
    mach_ids_by_name: &BTreeMap<&str, Vec<MachId>>,
) -> (Vec<MachId>, ChainEnd) {
    let mut out = Vec::new();
    let mut visited: BTreeSet<MachId> = BTreeSet::new();
    visited.insert(m_id);
    let mut cur = machs[m_id];
    loop {
        let Some(parent) = cur.refines.as_deref() else {
            return (out, ChainEnd::Root);
        };
        let Some(next) = unique_id(mach_ids_by_name, parent) else {
            return (out, ChainEnd::Truncated);
        };
        if !visited.insert(next) {
            return (out, ChainEnd::Truncated);
        }
        out.push(next);
        cur = machs[next];
    }
}

/// The ancestor-event chain an extended event materialises, root-first.
///
/// The refinement target at each level is the explicit `refines` name or,
/// when absent, the event's own name — mirroring
/// `sc::machine::events::resolve_effective_refines` (Rodin XML leaves
/// `refines` unset on a self-closing extended event). Only `extended` links
/// are followed: a plain-refines ancestor's body is self-contained, so it
/// terminates the chain and contributes its own clauses. Lookup takes the
/// first name match; the SC's `events_by_label` map is last-wins, but the
/// two diverge only on duplicate event labels (EB022).
fn extends_chain_root_first<'a>(e: &'a Event, ancestors: &[&'a Machine]) -> Vec<&'a Event> {
    let mut chain = Vec::new();
    let mut cur = e;
    for anc in ancestors {
        let target = cur.refines.as_deref().unwrap_or(cur.name.as_str());
        let Some(ev) = anc.events.iter().find(|ae| ae.name == target) else {
            break;
        };
        chain.push(ev);
        if !ev.extended {
            break;
        }
        cur = ev;
    }
    chain.reverse();
    chain
}

/// Collect the names machine `m`'s extended events inherit from above:
/// the ancestor chain's guards and action right-hand sides. Only the
/// REFERENCE side matters: inherited action left-hand sides could only
/// assign ancestor-visible names, and both EB011 and EB012 judge a
/// variable at its declaring machine, where no ancestor action can touch
/// it.
///
/// Parameters accumulate down the chain, so each level's clauses are walked
/// with the locals of every level at or above it — an inherited parameter
/// must not leak into the machine-level sets. Ancestor `with`/`witnesses`
/// are NOT inherited by the SC and are not collected. Unlike ancestor
/// invariants (memoized per machine because every descendant inherits
/// them), each descendant re-walks its event chains directly — an accepted
/// cost, since chains are short and only extended events pay it.
fn upward_refs(m: &Machine, ancestors: &[&Machine]) -> BTreeSet<String> {
    let mut refs = BTreeSet::new();

    for e in m.events.iter().filter(|e| e.extended) {
        let mut locals: Vec<&str> = Vec::new();
        for ev in extends_chain_root_first(e, ancestors) {
            locals.extend(ev.parameters.iter().map(|p| p.name.as_str()));
            for g in &ev.guards {
                collect_referenced_in_predicate_with_locals(&g.predicate, &locals, &mut refs);
            }
            for la in &ev.actions {
                collect_referenced_in_action_rhs_with_locals(&la.action, &locals, &mut refs);
            }
        }
    }

    refs
}

/// The INIT-action chain an extended INITIALISATION inherits.
struct InitChain {
    /// Referenced by inherited INIT-action right-hand sides.
    refs: BTreeSet<String>,
    /// Assigned by inherited INIT actions (left-hand sides).
    assigned: BTreeSet<String>,
    /// Whether every extended link found an ancestor INIT to inherit from.
    /// `false` means the chain is abnormal — the parent name resolves to
    /// zero or several machines, the REFINES chain is circular, or an
    /// ancestor has no INITIALISATION at all — so `assigned` may be
    /// incomplete, and EB014 must not judge completeness against it (a
    /// broken walk is EB008/EB009/EB019's report; a missing ancestor INIT
    /// already gets its own per-variable EB014 on the ancestor, and
    /// re-flagging every kept variable on the child would only duplicate
    /// that noise). The collected refs/assigns feed EB011/EB012 only in the
    /// missing-ancestor-INIT case — when the walk itself truncated, those
    /// lints don't run at all (see [`run`]).
    fully_resolved: bool,
}

/// Walk the INIT chain an extended INITIALISATION materialises: the
/// ancestor's INIT actions, continuing up while that INIT is itself
/// extended. A non-extended ancestor INIT is self-contained and closes the
/// chain resolved; a chain still extended when the ancestors run out is
/// resolved only if the walk genuinely reached a root machine.
fn inherited_init_chain(ancestors: &[&Machine], chain_end: ChainEnd) -> InitChain {
    let mut refs = BTreeSet::new();
    let mut assigned = BTreeSet::new();
    for anc in ancestors {
        let Some(init) = &anc.initialisation else {
            return InitChain {
                refs,
                assigned,
                fully_resolved: false,
            };
        };
        for la in &init.actions {
            collect_referenced_in_action_rhs(&la.action, &mut refs);
            assigned.extend(lhs_variables(&la.action).into_iter().map(String::from));
        }
        if !init.extended {
            return InitChain {
                refs,
                assigned,
                fully_resolved: true,
            };
        }
    }
    InitChain {
        refs,
        assigned,
        fully_resolved: chain_end == ChainEnd::Root,
    }
}

/// Everything machine `m`'s emitted `.bcm` inherits from its REFINES
/// ancestors as a reference set: the extended events' chain clauses, every
/// ancestor invariant, and the extended-INIT chain. The second value is
/// the inherited INIT assignment set when that chain fully resolved —
/// EB014's completeness input; `None` means an extended INIT whose chain
/// can't be judged.
fn inherited_contributions(
    m: &Machine,
    ancestor_ids: &[MachId],
    machs: &[&Machine],
    chain_end: ChainEnd,
    mach_inv_refs: &[BTreeSet<String>],
) -> (BTreeSet<String>, Option<BTreeSet<String>>) {
    let ancestors: Vec<&Machine> = ancestor_ids.iter().map(|&a| machs[a]).collect();
    let mut refs = upward_refs(m, &ancestors);

    // Abstract invariants are inherited unconditionally — the SC splices
    // every ancestor's invariants into the concrete `.bcm` (see
    // `render_machine_root`) — so a kept variable referenced only by an
    // abstract invariant is referenced here too. (Such a variable can't
    // even be dropped: its inherited events would trip EB025.) Variants
    // are per-machine and not inherited.
    for &anc in ancestor_ids {
        refs.extend(mach_inv_refs[anc].iter().cloned());
    }

    let mut init_assigned = None;
    if m.initialisation.as_ref().is_some_and(|i| i.extended) {
        let chain = inherited_init_chain(&ancestors, chain_end);
        refs.extend(chain.refs);
        // The chain's assignments are INIT assignments: they feed EB014's
        // completeness set below, not the event-write set.
        if chain.fully_resolved {
            init_assigned = Some(chain.assigned);
        }
    }

    (refs, init_assigned)
}

/// A machine's position in [`ProjectIndex::machs`] — the key of every
/// `mach_*` collection in the index. The index is keyed by id, not name:
/// two components may share a name (EB019's domain), and ids keep their
/// data apart.
type MachId = usize;

/// All arena ids declared under `name` (empty when the project has none).
fn candidate_ids<'m>(by_name: &'m BTreeMap<&str, Vec<usize>>, name: &str) -> &'m [usize] {
    by_name.get(name).map_or(&[][..], Vec::as_slice)
}

/// Resolve `name` to an arena id iff exactly one component declares it.
/// Zero candidates (EB009's domain) and several (EB019's) are equally
/// unresolvable.
fn unique_id(by_name: &BTreeMap<&str, Vec<usize>>, name: &str) -> Option<usize> {
    match candidate_ids(by_name, name) {
        &[id] => Some(id),
        _ => None,
    }
}

struct ProjectIndex<'a> {
    /// Machine arena, in `project.components` encounter order.
    machs: Vec<&'a Machine>,
    /// Per-component machine arena id, parallel to `project.components` —
    /// how [`run`] finds each judged machine's own entries. `None` for
    /// contexts, which carry no cross-component lint data.
    component_mach_ids: Vec<Option<MachId>>,
    /// Per-machine, references appearing in its OWN text (invariants/
    /// variant/events). Inherited names live in [`Self::mach_inherited_refs`]
    /// and are unioned in only by [`Self::effective_refs_for_machine`], so a
    /// machine's inherited names don't echo into its ancestors' entries
    /// through the downward descendant unions.
    mach_refs: Vec<BTreeSet<String>>,
    /// Per-machine, the variable names assigned by its OWN events,
    /// INITIALISATION excluded (see [`event_assigned_in_machine`]).
    mach_event_assigned: Vec<BTreeSet<String>>,
    /// Per-machine, names its emitted `.bcm` additionally references via
    /// inheritance: extended events' ancestor guards/action-RHS, the
    /// extended-INIT chain, and every ancestor invariant.
    mach_inherited_refs: Vec<BTreeSet<String>>,
    /// Per-machine, its uniquely-resolved REFINES ancestors, nearest-first
    /// (the single cycle-guarded walk, computed once).
    mach_ancestors: Vec<Vec<MachId>>,
    /// Per-machine, how that ancestor walk ended. A truncated walk stores
    /// the resolved prefix in [`Self::mach_ancestors`], so truncation can't
    /// be inferred from an empty list — this flag is the source of truth.
    mach_chain_end: Vec<ChainEnd>,
    /// Per-machine, the machines that REFINE it transitively (excluding
    /// itself; empty for leaves).
    mach_refines_descendants: Vec<BTreeSet<MachId>>,
    /// Per-machine, the INIT-action LHS names an extended INITIALISATION
    /// inherits from its ancestor chain. `Some` only when every extended
    /// link resolved (see [`InitChain::fully_resolved`]); consulted by
    /// EB014.
    init_inherited_assigned: Vec<Option<BTreeSet<String>>>,
}

impl<'a> ProjectIndex<'a> {
    fn build(project: &'a Project) -> Self {
        // Arena pass: give every machine an id in encounter order and index
        // the (possibly duplicated) names.
        let mut machs: Vec<&Machine> = Vec::new();
        let mut component_mach_ids: Vec<Option<MachId>> = Vec::new();
        let mut mach_ids_by_name: BTreeMap<&str, Vec<MachId>> = BTreeMap::new();
        for pc in &project.components {
            match &pc.component {
                Component::Machine(m) => {
                    component_mach_ids.push(Some(machs.len()));
                    mach_ids_by_name
                        .entry(m.name.as_str())
                        .or_default()
                        .push(machs.len());
                    machs.push(m);
                }
                Component::Context(_) => component_mach_ids.push(None),
            }
        }

        // Own-text reference/assignment sets, per id.
        let mach_inv_refs: Vec<_> = machs.iter().copied().map(invariant_refs).collect();
        let mach_refs: Vec<_> = machs
            .iter()
            .zip(&mach_inv_refs)
            .map(|(m, inv_refs)| {
                let mut refs = inv_refs.clone();
                machine_body_refs(m, &mut refs);
                refs
            })
            .collect();
        let mach_event_assigned: Vec<_> = machs
            .iter()
            .copied()
            .map(event_assigned_in_machine)
            .collect();

        // Upward pass: what each machine's emitted `.bcm` inherits from its
        // REFINES ancestors, kept apart from the own-text sets (see the
        // `mach_refs` field doc).
        let mut mach_inherited_refs = Vec::with_capacity(machs.len());
        let mut mach_ancestors = Vec::with_capacity(machs.len());
        let mut mach_chain_end = Vec::with_capacity(machs.len());
        let mut init_inherited_assigned = Vec::with_capacity(machs.len());
        for (id, &m) in machs.iter().enumerate() {
            let (ancestors, chain_end) = ancestor_machines(id, &machs, &mach_ids_by_name);
            let (inherited_refs, init_assigned) =
                inherited_contributions(m, &ancestors, &machs, chain_end, &mach_inv_refs);
            mach_inherited_refs.push(inherited_refs);
            init_inherited_assigned.push(init_assigned);
            mach_ancestors.push(ancestors);
            mach_chain_end.push(chain_end);
        }

        // Downward edges. A parent name attaches the child to EVERY
        // machine carrying that name — this data only ever suppresses
        // warnings, so over-approximating across duplicates is conservative,
        // while dropping a candidate would false-positive on it (see the
        // module comment above `ChainEnd`).
        let mut mach_children: BTreeMap<MachId, Vec<MachId>> = BTreeMap::new();
        for (child, m) in machs.iter().enumerate() {
            if let Some(parent_name) = &m.refines {
                for &parent in candidate_ids(&mach_ids_by_name, parent_name) {
                    mach_children.entry(parent).or_default().push(child);
                }
            }
        }

        let mach_refines_descendants = transitive_descendants(&mach_children, machs.len());

        Self {
            machs,
            component_mach_ids,
            mach_refs,
            mach_event_assigned,
            mach_inherited_refs,
            mach_ancestors,
            mach_chain_end,
            mach_refines_descendants,
            init_inherited_assigned,
        }
    }

    /// Variable names inherited by the machine at `id`: the union of the own
    /// variables of every REFINES ancestor (the machine's own variables are
    /// excluded). Empty for a root machine. Derived from the ancestor lists
    /// [`ancestor_machines`] computed once during `build`, so this and the
    /// upward reference/assignment pass can't drift apart.
    fn inherited_vars_for_machine(&self, id: MachId) -> BTreeSet<&'a str> {
        self.mach_ancestors[id]
            .iter()
            .flat_map(|&a| self.machs[a].variables.iter().map(|v| v.name.as_str()))
            .collect()
    }

    /// The materialised machine's full reference set — own text, REFINES
    /// descendants, and inherited names — or `None` when the ancestor walk
    /// didn't reach a root (missing, duplicated, or circular parent — each
    /// an Error in its own right): the set would be speculation, and EB011
    /// must stay silent rather than judge against it.
    fn effective_refs_for_machine(&self, id: MachId) -> Option<BTreeSet<&str>> {
        if self.mach_chain_end[id] != ChainEnd::Root {
            return None;
        }
        let mut out: BTreeSet<&str> = self.mach_refs[id].iter().map(String::as_str).collect();
        for &d in &self.mach_refines_descendants[id] {
            out.extend(self.mach_refs[d].iter().map(String::as_str));
        }
        out.extend(self.mach_inherited_refs[id].iter().map(String::as_str));
        Some(out)
    }

    /// The machine's event-write set — own events plus REFINES
    /// descendants' events; INITIALISATION assignments are excluded
    /// throughout. No inherited component: both consumers judge a variable
    /// at its declaring machine, which no ancestor action can assign.
    fn effective_event_assigned_for_machine(&self, id: MachId) -> BTreeSet<&str> {
        let mut out: BTreeSet<&str> = self.mach_event_assigned[id]
            .iter()
            .map(String::as_str)
            .collect();
        for &d in &self.mach_refines_descendants[id] {
            out.extend(self.mach_event_assigned[d].iter().map(String::as_str));
        }
        out
    }
}

/// For each machine id in `0..n`, the transitive closure of `children`
/// excluding the machine itself (empty for leaves).
fn transitive_descendants(
    children: &BTreeMap<MachId, Vec<MachId>>,
    n: usize,
) -> Vec<BTreeSet<MachId>> {
    (0..n)
        .map(|root| {
            let mut descs = BTreeSet::new();
            let mut stack: Vec<MachId> = children.get(&root).cloned().unwrap_or_default();
            while let Some(node) = stack.pop() {
                if !descs.insert(node) {
                    continue;
                }
                if let Some(cs) = children.get(&node) {
                    stack.extend(cs.iter().copied());
                }
            }
            descs
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Severity;
    use rossi::ast::expression::BinaryOp;
    use rossi::{
        Action, ActionKind, Component, Context, Event, Expression, ExpressionKind,
        InitialisationEvent, LabeledAction, LabeledPredicate, Machine, NamedElement, Predicate,
        PredicateKind,
    };

    use crate::project::ProjectComponent;
    use crate::rodin_ids::RodinIds;

    fn pc(filename: &str, component: Component) -> ProjectComponent {
        ProjectComponent {
            filename: filename.into(),
            component,
            rodin_ids: RodinIds::default(),
            source: None,
        }
    }

    fn proj(components: Vec<ProjectComponent>) -> Project {
        Project {
            name: "test".into(),
            components,
        }
    }

    fn lp(label: &str, predicate: Predicate) -> LabeledPredicate {
        LabeledPredicate {
            label: Some(label.into()),
            is_theorem: false,
            predicate,
            span: None,
            comment: None,
        }
    }

    fn la(action: Action) -> LabeledAction {
        LabeledAction {
            label: None,
            action,
            span: None,
            comment: None,
        }
    }

    fn ident(n: &str) -> Expression {
        ExpressionKind::Identifier(n.into()).into()
    }

    fn cmp_pred(op: ComparisonOp, lhs: Expression, rhs: Expression) -> Predicate {
        PredicateKind::Comparison {
            op,
            left: lhs,
            right: rhs,
        }
        .into()
    }

    fn eq_pred(lhs: Expression, rhs: Expression) -> Predicate {
        cmp_pred(ComparisonOp::Equal, lhs, rhs)
    }

    fn nv(n: &str) -> NamedElement {
        NamedElement::new(n.into())
    }

    #[test]
    fn dead_variable_is_flagged() {
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x"), nv("y")];
        m.invariants = vec![lp(
            "inv1",
            eq_pred(ident("x"), ExpressionKind::Integer(0).into()),
        )];
        m.initialisation = Some(InitialisationEvent {
            actions: vec![
                la(Action::assignment("x", ExpressionKind::Integer(0).into())),
                la(Action::assignment("y", ExpressionKind::Integer(0).into())),
            ],
            comment: None,
            extended: false,
            with: Vec::new(),
            witnesses: Vec::new(),
            span: None,
            name_span: None,
        });

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        let dead: Vec<_> = diags
            .iter()
            .filter(|d| d.rule_id == Some(RuleId::DeadVariable))
            .collect();
        assert_eq!(
            dead.len(),
            1,
            "expected exactly one dead-var diag: {diags:#?}"
        );
        assert!(dead[0].message.contains('`'));
        assert!(dead[0].message.contains('y'));
    }

    #[test]
    fn unmodified_variable_is_flagged() {
        // `x` is initialised, read by a real (non-typing) invariant, and no
        // event ever modifies it — a constant in disguise.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        m.invariants = vec![lp(
            "inv1",
            eq_pred(ident("x"), ExpressionKind::Integer(0).into()),
        )];
        m.initialisation = Some(init_event(&["x"], false));

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        let unmod: Vec<_> = diags
            .iter()
            .filter(|d| d.rule_id == Some(RuleId::UnmodifiedVariable))
            .collect();
        assert_eq!(unmod.len(), 1, "expected one EB012: {diags:#?}");
        assert!(unmod[0].message.contains("CONSTANT"), "{diags:#?}");
    }

    #[test]
    fn shadowed_names_are_flagged() {
        // `POW` (exact ASCII operator spelling) and `NAT` (the exact-case ℕ
        // token) warn; `Dom`, `pow`, `Nat`, `OR` are ordinary identifiers and
        // stay silent. The §2.2 reserved words never reach the lint — the
        // parser rejects their declarations outright.
        let mut c = Context::new("C".into());
        c.constants = vec![
            nv("POW"),
            nv("Dom"),
            nv("pow"),
            nv("Nat"),
            nv("OR"),
            nv("price"),
        ];
        c.sets = vec![rossi::SetDeclaration::Deferred {
            name: "NAT".into(),
            comment: None,
            span: None,
        }];
        c.axioms = vec![lp(
            "ax1",
            eq_pred(ident("price"), ExpressionKind::Integer(0).into()),
        )];

        let diags = run(&proj(vec![pc("C.buc", Component::Context(c))]));
        let shadowed: Vec<_> = diags
            .iter()
            .filter(|d| d.rule_id == Some(RuleId::ShadowedName))
            .collect();
        assert_eq!(shadowed.len(), 2, "{shadowed:?}");
        assert!(shadowed.iter().any(|d| d.origin == "C.POW"));
        assert!(shadowed.iter().any(|d| d.origin == "C.NAT"));
    }

    #[test]
    fn shadowed_name_diag_carries_the_declaration_span() {
        // The EB023 diagnostic must anchor on the declaring element's span so a
        // caller can place it on the right line. A carrier set takes the set's
        // span; a constant takes the identifier's span.
        let set_span = Span { start: 11, end: 14 };
        let const_span = Span { start: 30, end: 33 };
        let mut c = Context::new("C".into());
        c.sets = vec![rossi::SetDeclaration::Deferred {
            name: "POW".into(),
            comment: None,
            span: Some(set_span),
        }];
        c.constants = vec![rossi::NamedElement::with_span("NAT".into(), const_span)];

        let diags = run_component(&Component::Context(c));
        let shadowed: Vec<_> = diags
            .iter()
            .filter(|d| d.rule_id == Some(RuleId::ShadowedName))
            .collect();
        assert_eq!(shadowed.len(), 2, "{shadowed:?}");
        assert_eq!(
            shadowed.iter().find(|d| d.origin == "C.POW").unwrap().span,
            Some(set_span)
        );
        assert_eq!(
            shadowed.iter().find(|d| d.origin == "C.NAT").unwrap().span,
            Some(const_span)
        );
    }

    #[test]
    fn duplicate_identifier_diag_carries_first_occurrence_span() {
        // EB021 anchors on the first declaration of the duplicated name.
        let first = Span { start: 4, end: 5 };
        let second = Span { start: 9, end: 10 };
        let mut m = Machine::new("M".into());
        m.variables = vec![
            rossi::NamedElement::with_span("x".into(), first),
            rossi::NamedElement::with_span("x".into(), second),
            nv("y"),
        ];
        let diags = run_component(&Component::Machine(m));
        let ids: Vec<_> = diags
            .iter()
            .filter(|d| d.rule_id == Some(RuleId::DuplicateIdentifier))
            .collect();
        assert_eq!(ids.len(), 1, "{ids:?}");
        assert_eq!(ids[0].span, Some(first));
    }

    #[test]
    fn shadowed_machine_names_are_flagged() {
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("or"), nv("count")];
        let mut e = Event::new("evt".into());
        e.parameters = vec![nv("circ"), nv("p")];
        m.events = vec![e];

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        let shadowed: Vec<_> = diags
            .iter()
            .filter(|d| d.rule_id == Some(RuleId::ShadowedName))
            .collect();
        assert_eq!(shadowed.len(), 2, "{shadowed:?}");
        assert!(shadowed.iter().any(|d| d.origin == "M.or"));
        assert!(shadowed.iter().any(|d| d.origin == "M.evt.circ"));
    }

    #[test]
    fn quantifier_binder_does_not_count_as_use() {
        // ∀x · TRUE — the `x` binder shouldn't count as a use of the machine
        // variable named `x`, so the variable should still be flagged dead.
        use rossi::TypedIdentifier;
        use rossi::ast::predicate::Quantifier;
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        m.invariants = vec![lp(
            "inv1",
            PredicateKind::Quantified {
                quantifier: Quantifier::ForAll,
                identifiers: vec![TypedIdentifier {
                    name: "x".into(),
                    type_expr: None,
                    span: None,
                }],
                predicate: Box::new(PredicateKind::True.into()),
            }
            .into(),
        )];
        m.initialisation = Some(InitialisationEvent {
            actions: vec![la(Action::assignment(
                "x",
                ExpressionKind::Integer(0).into(),
            ))],
            comment: None,
            extended: false,
            with: Vec::new(),
            witnesses: Vec::new(),
            span: None,
            name_span: None,
        });

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        let dead: Vec<_> = diags
            .iter()
            .filter(|d| d.rule_id == Some(RuleId::DeadVariable))
            .collect();
        assert_eq!(
            dead.len(),
            1,
            "binder should not satisfy reference: {diags:#?}"
        );
    }

    #[test]
    fn incomplete_init_is_flagged() {
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x"), nv("y")];
        m.initialisation = Some(InitialisationEvent {
            actions: vec![la(Action::assignment(
                "x",
                ExpressionKind::Integer(0).into(),
            ))],
            comment: None,
            extended: false,
            with: Vec::new(),
            witnesses: Vec::new(),
            span: None,
            name_span: None,
        });

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        let missing: Vec<_> = diags
            .iter()
            .filter(|d| d.rule_id == Some(RuleId::IncompleteInitialisation))
            .collect();
        assert_eq!(missing.len(), 1);
        assert!(missing[0].message.contains('y'));
    }

    #[test]
    fn bsu_primed_reference_counts_as_use() {
        // INITIALISATION uses `x :| x' = 0` — the predicate references
        // `x'`, which after prime-stripping is a use of `x`: no EB011.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        m.initialisation = Some(InitialisationEvent {
            actions: vec![la(ActionKind::BecomesSuchThat {
                variables: vec!["x".into()],
                predicate: eq_pred(ident("x'"), ExpressionKind::Integer(0).into()),
            }
            .into())],
            comment: None,
            extended: false,
            with: Vec::new(),
            witnesses: Vec::new(),
            span: None,
            name_span: None,
        });

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        assert!(
            !diags
                .iter()
                .any(|d| d.rule_id == Some(RuleId::DeadVariable)),
            "x' should count as a use of x: {diags:#?}"
        );
    }

    #[test]
    fn function_position_reference_counts_as_use() {
        // `closure` is used only as the FUNCTION of an application — the
        // shape Rodin models give their own axiomatised closure constants
        // (core Event-B has no closure operator). The name must not be
        // treated as a built-in, or the reference would be invisible and
        // `closure` would be falsely dead.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("closure"), nv("v")];
        let mut e = Event::new("e".into());
        e.guards = vec![lp(
            "grd1",
            eq_pred(
                ExpressionKind::FunctionApplication {
                    function: Box::new(ident("closure")),
                    arguments: vec![ident("v")],
                }
                .into(),
                ident("v"),
            ),
        )];
        m.events = vec![e];
        m.initialisation = Some(init_event(&["closure", "v"], false));

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        assert!(eb011_on(&diags, "M.closure").is_empty(), "{diags:#?}");
    }

    // ---------- duplicate identifiers / labels (EB021 / EB022) --------------

    fn dups_of(diags: &[Diagnostic], rule: RuleId) -> Vec<&Diagnostic> {
        diags.iter().filter(|d| d.rule_id == Some(rule)).collect()
    }

    fn labeled_action(label: &str) -> LabeledAction {
        LabeledAction {
            label: Some(label.into()),
            action: Action::assignment("x", ExpressionKind::Integer(0).into()),
            span: None,
            comment: None,
        }
    }

    #[test]
    fn duplicate_variable_identifier_is_flagged() {
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x"), nv("x"), nv("y")];
        let diags = run_component(&Component::Machine(m));
        let ids = dups_of(&diags, RuleId::DuplicateIdentifier);
        assert_eq!(ids.len(), 1, "{diags:#?}");
        assert_eq!(ids[0].severity, Severity::Error);
        assert_eq!(ids[0].origin, "M.x");
        assert!(
            ids[0].message.contains("variable identifier `x`"),
            "{}",
            ids[0].message
        );
        assert!(ids[0].message.contains("(declared 2 times)"));
    }

    #[test]
    fn duplicate_invariant_label_is_flagged() {
        let mut m = Machine::new("M".into());
        m.invariants = vec![
            lp(
                "inv1",
                eq_pred(ident("x"), ExpressionKind::Integer(0).into()),
            ),
            lp(
                "inv1",
                eq_pred(ident("y"), ExpressionKind::Integer(0).into()),
            ),
        ];
        let diags = run_component(&Component::Machine(m));
        let labels = dups_of(&diags, RuleId::DuplicateLabel);
        assert_eq!(labels.len(), 1, "{diags:#?}");
        assert_eq!(labels[0].origin, "M.inv1");
        assert!(labels[0].message.contains("invariant label `inv1`"));
        assert!(labels[0].message.contains("(used 2 times)"));
    }

    #[test]
    fn duplicate_event_label_is_flagged() {
        let mut m = Machine::new("M".into());
        m.events = vec![Event::new("evt".into()), Event::new("evt".into())];
        let diags = run_component(&Component::Machine(m));
        let labels = dups_of(&diags, RuleId::DuplicateLabel);
        assert_eq!(labels.len(), 1, "{diags:#?}");
        assert_eq!(labels[0].origin, "M.evt");
        assert!(labels[0].message.contains("event label `evt`"));
    }

    #[test]
    fn guard_and_action_sharing_label_is_flagged() {
        // Event-B shares one label namespace across guards and actions, so a
        // guard `lbl` and an action `lbl` in the same event collide.
        let mut e = Event::new("evt".into());
        e.guards = vec![lp("lbl", PredicateKind::True.into())];
        e.actions = vec![labeled_action("lbl")];
        let mut m = Machine::new("M".into());
        m.events = vec![e];
        let diags = run_component(&Component::Machine(m));
        let labels = dups_of(&diags, RuleId::DuplicateLabel);
        assert_eq!(labels.len(), 1, "{diags:#?}");
        assert_eq!(labels[0].origin, "M.evt.lbl");
        assert!(labels[0].message.contains("guard or action label `lbl`"));
        assert!(labels[0].message.contains("event `evt` of machine `M`"));
    }

    #[test]
    fn duplicate_parameter_identifier_is_flagged() {
        let mut e = Event::new("evt".into());
        e.parameters = vec![nv("p"), nv("p")];
        let mut m = Machine::new("M".into());
        m.events = vec![e];
        let diags = run_component(&Component::Machine(m));
        let ids = dups_of(&diags, RuleId::DuplicateIdentifier);
        assert_eq!(ids.len(), 1, "{diags:#?}");
        assert_eq!(ids[0].origin, "M.evt.p");
        assert!(ids[0].message.contains("parameter identifier `p`"));
    }

    #[test]
    fn carrier_set_and_constant_sharing_name_is_flagged_once() {
        // Carrier sets and constants share one identifier namespace.
        let mut c = Context::new("C".into());
        c.sets = vec![rossi::SetDeclaration::Deferred {
            name: "S".into(),
            comment: None,
            span: None,
        }];
        c.constants = vec![nv("S")];
        let diags = run_component(&Component::Context(c));
        let ids = dups_of(&diags, RuleId::DuplicateIdentifier);
        assert_eq!(ids.len(), 1, "{diags:#?}");
        assert_eq!(ids[0].origin, "C.S");
        assert!(
            ids[0]
                .message
                .contains("carrier set or constant identifier `S`")
        );
        assert!(ids[0].message.contains("context `C`"));
    }

    #[test]
    fn duplicate_axiom_label_is_flagged() {
        let mut c = Context::new("C".into());
        c.axioms = vec![
            lp(
                "axm1",
                eq_pred(ident("k"), ExpressionKind::Integer(0).into()),
            ),
            lp(
                "axm1",
                eq_pred(ident("k"), ExpressionKind::Integer(1).into()),
            ),
        ];
        let diags = run_component(&Component::Context(c));
        let labels = dups_of(&diags, RuleId::DuplicateLabel);
        assert_eq!(labels.len(), 1, "{diags:#?}");
        assert_eq!(labels[0].origin, "C.axm1");
        assert!(labels[0].message.contains("axiom label `axm1`"));
    }

    #[test]
    fn duplicate_witness_label_across_with_and_witnesses_is_flagged() {
        // `with` (abstract var) and `witnesses` (abstract param) share one
        // witness-label namespace in Event-B; the same label in each collides.
        let mut e = Event::new("evt".into());
        e.with = vec![lp("w", PredicateKind::True.into())];
        e.witnesses = vec![lp("w", PredicateKind::True.into())];
        let mut m = Machine::new("M".into());
        m.events = vec![e];
        let diags = run_component(&Component::Machine(m));
        let labels = dups_of(&diags, RuleId::DuplicateLabel);
        assert_eq!(labels.len(), 1, "{diags:#?}");
        assert_eq!(labels[0].origin, "M.evt.w");
        assert!(labels[0].message.contains("witness label `w`"));
    }

    #[test]
    fn initialisation_duplicate_action_label_is_flagged() {
        // INITIALISATION is treated as an event sharing the guard/action label
        // namespace, even though rossi stores it apart from `events`.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        m.initialisation = Some(InitialisationEvent {
            actions: vec![labeled_action("act1"), labeled_action("act1")],
            comment: None,
            extended: false,
            with: Vec::new(),
            witnesses: Vec::new(),
            span: None,
            name_span: None,
        });
        let diags = run_component(&Component::Machine(m));
        let labels = dups_of(&diags, RuleId::DuplicateLabel);
        assert_eq!(labels.len(), 1, "{diags:#?}");
        assert_eq!(labels[0].origin, "M.INITIALISATION.act1");
        assert!(labels[0].message.contains("guard or action label `act1`"));
        assert!(
            labels[0]
                .message
                .contains("event `INITIALISATION` of machine `M`")
        );
    }

    #[test]
    fn identifier_and_label_in_separate_namespaces_do_not_conflict() {
        // A variable `x` and an invariant labelled `x` must NOT be reported:
        // identifiers and labels are distinct namespaces.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        m.invariants = vec![lp(
            "x",
            eq_pred(ident("x"), ExpressionKind::Integer(0).into()),
        )];
        let diags = run_component(&Component::Machine(m));
        assert!(
            dups_of(&diags, RuleId::DuplicateIdentifier).is_empty()
                && dups_of(&diags, RuleId::DuplicateLabel).is_empty(),
            "{diags:#?}"
        );
    }

    #[test]
    fn context_identifier_and_label_in_separate_namespaces_do_not_conflict() {
        // The symmetric context case: a constant `S` and an axiom labelled `S`
        // must NOT collide — identifiers and labels are distinct namespaces.
        let mut c = Context::new("C".into());
        c.constants = vec![nv("S")];
        c.axioms = vec![lp(
            "S",
            eq_pred(ident("S"), ExpressionKind::Integer(0).into()),
        )];
        let diags = run_component(&Component::Context(c));
        assert!(
            dups_of(&diags, RuleId::DuplicateIdentifier).is_empty()
                && dups_of(&diags, RuleId::DuplicateLabel).is_empty(),
            "{diags:#?}"
        );
    }

    #[test]
    fn unlabeled_guards_are_not_duplicates() {
        // Two guards with no explicit label must not be reported — blank names
        // are skipped.
        let blank = || LabeledPredicate {
            label: None,
            is_theorem: false,
            predicate: PredicateKind::True.into(),
            span: None,
            comment: None,
        };
        let mut e = Event::new("evt".into());
        e.guards = vec![blank(), blank()];
        let mut m = Machine::new("M".into());
        m.events = vec![e];
        let diags = run_component(&Component::Machine(m));
        assert!(
            dups_of(&diags, RuleId::DuplicateLabel).is_empty(),
            "{diags:#?}"
        );
    }

    #[test]
    fn whitespace_only_labels_are_not_duplicates() {
        // Whitespace-only labels are blank and must be skipped, not just the
        // empty string.
        let ws = || LabeledPredicate {
            label: Some("   ".into()),
            is_theorem: false,
            predicate: PredicateKind::True.into(),
            span: None,
            comment: None,
        };
        let mut e = Event::new("evt".into());
        e.guards = vec![ws(), ws()];
        let mut m = Machine::new("M".into());
        m.events = vec![e];
        let diags = run_component(&Component::Machine(m));
        assert!(
            dups_of(&diags, RuleId::DuplicateLabel).is_empty(),
            "{diags:#?}"
        );
    }

    #[test]
    fn clean_model_produces_no_duplicate_findings() {
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x"), nv("y")];
        m.invariants = vec![
            lp("inv1", PredicateKind::True.into()),
            lp("inv2", PredicateKind::True.into()),
        ];
        let mut e = Event::new("evt".into());
        e.parameters = vec![nv("p"), nv("q")];
        e.guards = vec![lp("grd1", PredicateKind::True.into())];
        e.actions = vec![labeled_action("act1")];
        m.events = vec![e];
        let diags = run_component(&Component::Machine(m));
        assert!(
            dups_of(&diags, RuleId::DuplicateIdentifier).is_empty()
                && dups_of(&diags, RuleId::DuplicateLabel).is_empty(),
            "{diags:#?}"
        );
    }

    #[test]
    fn cross_refines_keeps_variable_alive() {
        // M1 declares v but doesn't reference it; M2 refines M1 and uses v
        // in an invariant. M1 should not flag v as dead.
        let mut m1 = Machine::new("M1".into());
        m1.variables = vec![nv("v")];
        m1.initialisation = Some(InitialisationEvent {
            actions: vec![la(Action::assignment(
                "v",
                ExpressionKind::Integer(0).into(),
            ))],
            comment: None,
            extended: false,
            with: Vec::new(),
            witnesses: Vec::new(),
            span: None,
            name_span: None,
        });
        let mut m2 = Machine::new("M2".into());
        m2.refines = Some("M1".into());
        m2.invariants = vec![lp(
            "inv1",
            eq_pred(ident("v"), ExpressionKind::Integer(0).into()),
        )];

        let project = proj(vec![
            pc("M1.bum", Component::Machine(m1)),
            pc("M2.bum", Component::Machine(m2)),
        ]);
        let diags = run(&project);
        assert!(
            !diags
                .iter()
                .any(|d| d.rule_id == Some(RuleId::DeadVariable) && d.origin.starts_with("M1.")),
            "v in M1 is alive via REFINES: {diags:#?}"
        );
    }

    #[test]
    fn cross_refines_keeps_variable_assigned() {
        // M1 initialises and reads v but never modifies it; M2 refines M1
        // and assigns v in an event. The descendant's write must reach M1's
        // event-write set (conservative downward union), or M1 would get a
        // false constant-like verdict.
        let mut m1 = Machine::new("M1".into());
        m1.variables = vec![nv("v")];
        m1.invariants = vec![lp(
            "inv1",
            eq_pred(ident("v"), ExpressionKind::Integer(0).into()),
        )];
        m1.initialisation = Some(init_event(&["v"], false));
        let mut m2 = Machine::new("M2".into());
        m2.refines = Some("M1".into());
        m2.events = vec![Event {
            name: "step".into(),
            status: None,
            refines: None,
            parameters: Vec::new(),
            guards: Vec::new(),
            with: Vec::new(),
            witnesses: Vec::new(),
            actions: vec![la(Action::assignment(
                "v",
                ExpressionKind::Integer(1).into(),
            ))],
            span: None,
            name_span: None,
            refines_span: None,
            comment: None,
            extended: false,
        }];

        let project = proj(vec![
            pc("M1.bum", Component::Machine(m1)),
            pc("M2.bum", Component::Machine(m2)),
        ]);
        let diags = run(&project);
        assert!(
            !diags
                .iter()
                .any(|d| d.rule_id == Some(RuleId::UnmodifiedVariable)
                    && d.origin.starts_with("M1.")),
            "v is assigned in M2: {diags:#?}"
        );
    }

    // ---------- extends-chain inheritance (upward) --------------------------
    //
    // Extended events materialise their abstract event's parameters/guards/
    // actions into the machine's `.bcm`; the reference/assignment sets must
    // see those inherited clauses or EB011/EB012 false-positive on kept
    // variables (the `extends` idiom leaves event bodies empty).

    /// An extended event named `name` refining `target` (`None` = implicit
    /// same-name match, the form Rodin-XML import produces).
    fn extends_event(name: &str, target: Option<&str>) -> Event {
        let mut e = Event::new(name.into());
        e.extended = true;
        e.refines = target.map(Into::into);
        e
    }

    /// An event named `name` whose sole guard is `var = 0`.
    fn guarded_event(name: &str, var: &str) -> Event {
        let mut e = Event::new(name.into());
        e.guards = vec![lp(
            "grd1",
            eq_pred(ident(var), ExpressionKind::Integer(0).into()),
        )];
        e
    }

    /// An INITIALISATION assigning `0` to each of `assigns`.
    fn init_event(assigns: &[&str], extended: bool) -> InitialisationEvent {
        InitialisationEvent {
            actions: assigns
                .iter()
                .map(|v| la(Action::assignment(*v, ExpressionKind::Integer(0).into())))
                .collect(),
            comment: None,
            extended,
            with: Vec::new(),
            witnesses: Vec::new(),
            span: None,
            name_span: None,
        }
    }

    /// The diagnostics for `rule` attributed to exactly `origin`.
    fn diags_on<'d>(diags: &'d [Diagnostic], rule: RuleId, origin: &str) -> Vec<&'d Diagnostic> {
        diags
            .iter()
            .filter(|d| d.rule_id == Some(rule) && d.origin == origin)
            .collect()
    }

    fn eb011_on<'d>(diags: &'d [Diagnostic], origin: &str) -> Vec<&'d Diagnostic> {
        diags_on(diags, RuleId::DeadVariable, origin)
    }

    #[test]
    fn kept_variable_is_judged_only_at_declaring_machine() {
        // `v` is dead everywhere; the verdict fires once, at declaring M0 —
        // the kept copy at M1 would only repeat it (plain or extended
        // refinement alike).
        let mut m0 = Machine::new("M0".into());
        m0.variables = vec![nv("v")];
        m0.initialisation = Some(init_event(&[], false));
        let mut m1 = Machine::new("M1".into());
        m1.refines = Some("M0".into());
        m1.variables = vec![nv("v")];
        let mut bar = Event::new("bar".into());
        bar.refines = Some("foo".into());
        m1.events = vec![bar];

        let diags = run(&proj(vec![
            pc("M0.bum", Component::Machine(m0)),
            pc("M1.bum", Component::Machine(m1)),
        ]));
        assert_eq!(eb011_on(&diags, "M0.v").len(), 1, "{diags:#?}");
        assert!(eb011_on(&diags, "M1.v").is_empty(), "{diags:#?}");
    }

    #[test]
    fn extends_chain_cycle_terminates() {
        // A circular REFINES chain (EB008's domain) with mutually extended
        // events must not hang the upward walk.
        let mut m1 = Machine::new("M1".into());
        m1.refines = Some("M2".into());
        m1.variables = vec![nv("v")];
        m1.events = vec![extends_event("e", Some("e"))];
        let mut m2 = Machine::new("M2".into());
        m2.refines = Some("M1".into());
        m2.variables = vec![nv("v")];
        m2.events = vec![extends_event("e", Some("e"))];

        let diags = run(&proj(vec![
            pc("M1.bum", Component::Machine(m1)),
            pc("M2.bum", Component::Machine(m2)),
        ]));
        assert!(!diags.is_empty(), "run() terminated on a cyclic chain");
    }

    #[test]
    fn inherited_parameter_shadows_variable_in_chain() {
        // The abstract guard references the abstract event's own parameter
        // `p`; M1's variable of the same name must not be kept alive by it.
        let mut m0 = Machine::new("M0".into());
        let mut e = Event::new("e".into());
        e.parameters = vec![nv("p")];
        e.guards = vec![lp(
            "grd1",
            eq_pred(ident("p"), ExpressionKind::Integer(0).into()),
        )];
        m0.events = vec![e];
        let mut m1 = Machine::new("M1".into());
        m1.refines = Some("M0".into());
        m1.variables = vec![nv("p")];
        m1.events = vec![extends_event("e", Some("e"))];

        let diags = run(&proj(vec![
            pc("M0.bum", Component::Machine(m0)),
            pc("M1.bum", Component::Machine(m1)),
        ]));
        assert_eq!(
            eb011_on(&diags, "M1.p").len(),
            1,
            "the inherited guard binds the parameter, not the variable: {diags:#?}"
        );
    }

    fn eb014_on<'d>(diags: &'d [Diagnostic], machine: &str) -> Vec<&'d Diagnostic> {
        diags_on(
            diags,
            RuleId::IncompleteInitialisation,
            &format!("{machine}.INITIALISATION"),
        )
    }

    #[test]
    fn extended_init_inheriting_all_assignments_is_complete() {
        // A two-level extended-INIT chain: M0 assigns v, M1 (extended) adds
        // w, M2 (extended) adds u. Every variable of every machine is
        // covered once the chain is folded in — no EB014 anywhere.
        let m0 = abstract_machine("M0", "v");
        let mut m1 = Machine::new("M1".into());
        m1.refines = Some("M0".into());
        m1.variables = vec![nv("v"), nv("w")];
        m1.initialisation = Some(init_event(&["w"], true));
        let mut m2 = Machine::new("M2".into());
        m2.refines = Some("M1".into());
        m2.variables = vec![nv("v"), nv("w"), nv("u")];
        m2.initialisation = Some(init_event(&["u"], true));

        let diags = run(&proj(vec![
            pc("M0.bum", Component::Machine(m0)),
            pc("M1.bum", Component::Machine(m1)),
            pc("M2.bum", Component::Machine(m2)),
        ]));
        assert!(
            eb014_on(&diags, "M1").is_empty() && eb014_on(&diags, "M2").is_empty(),
            "the extended-INIT chain covers every variable: {diags:#?}"
        );
    }

    #[test]
    fn extended_init_missing_new_variable_is_flagged() {
        // M1's extended INIT inherits the assignment of the kept `v` but
        // forgets its own new variable `w` — EB014 fires for `w` only.
        // (The old blanket bail on extended INITs missed this entirely.)
        let m0 = abstract_machine("M0", "v");
        let mut m1 = Machine::new("M1".into());
        m1.refines = Some("M0".into());
        m1.variables = vec![nv("v"), nv("w")];
        m1.initialisation = Some(init_event(&[], true));

        let diags = run(&proj(vec![
            pc("M0.bum", Component::Machine(m0)),
            pc("M1.bum", Component::Machine(m1)),
        ]));
        let eb014 = eb014_on(&diags, "M1");
        assert_eq!(
            eb014.len(),
            1,
            "exactly one EB014, for the new variable: {diags:#?}"
        );
        assert!(
            eb014[0].message.contains("`w`"),
            "the unassigned variable is w: {:?}",
            eb014[0]
        );
    }

    #[test]
    fn extended_init_with_missing_parent_stays_silent() {
        // The extended INIT names a parent that isn't in the project — the
        // chain is unresolvable, so EB014 keeps the old bail-out behaviour
        // (EB009 reports the unknown REFINES target).
        let mut m1 = Machine::new("M1".into());
        m1.refines = Some("Absent".into());
        m1.variables = vec![nv("v")];
        m1.initialisation = Some(init_event(&[], true));

        let diags = run(&proj(vec![pc("M1.bum", Component::Machine(m1))]));
        assert!(
            eb014_on(&diags, "M1").is_empty(),
            "an unresolvable chain must not produce EB014 noise: {diags:#?}"
        );
    }

    #[test]
    fn extended_init_with_uninitialised_parent_stays_silent() {
        // The parent has no INITIALISATION at all — it already gets one
        // EB014 per variable ('no INITIALISATION event'). Judging the
        // child's extended INIT against the empty inherited set would only
        // duplicate that noise onto every kept variable, so the chain
        // counts as unresolvable and the child stays silent.
        let mut m0 = Machine::new("M0".into());
        m0.variables = vec![nv("v")];
        let mut m1 = Machine::new("M1".into());
        m1.refines = Some("M0".into());
        m1.variables = vec![nv("v"), nv("w")];
        m1.initialisation = Some(init_event(&[], true));

        let diags = run(&proj(vec![
            pc("M0.bum", Component::Machine(m0)),
            pc("M1.bum", Component::Machine(m1)),
        ]));
        assert!(
            eb014_on(&diags, "M1").is_empty(),
            "the missing INIT is M0's problem, already reported there: {diags:#?}"
        );
        assert_eq!(
            eb014_on(&diags, "M0").len(),
            1,
            "M0 still gets its own no-INITIALISATION report: {diags:#?}"
        );
    }

    #[test]
    fn inherited_invariant_reference_without_assignment_is_unmodified() {
        // `v` is read by M0's (non-typing) invariant and assigned only by
        // M0's INIT: the constant-like verdict fires once, at the declaring
        // machine — the kept copy at M1 inherits it and stays silent.
        let mut m0 = abstract_machine("M0", "v");
        m0.invariants = vec![lp(
            "inv1",
            eq_pred(ident("v"), ExpressionKind::Integer(0).into()),
        )];
        let mut m1 = Machine::new("M1".into());
        m1.refines = Some("M0".into());
        m1.variables = vec![nv("v")];

        let diags = run(&proj(vec![
            pc("M0.bum", Component::Machine(m0)),
            pc("M1.bum", Component::Machine(m1)),
        ]));
        assert!(
            eb011_on(&diags, "M1.v").is_empty(),
            "v is referenced by the inherited invariant: {diags:#?}"
        );
        assert_eq!(
            diags_on(&diags, RuleId::UnmodifiedVariable, "M0.v").len(),
            1,
            "constant-like v reports at its declaring machine: {diags:#?}"
        );
        assert!(
            diags_on(&diags, RuleId::UnmodifiedVariable, "M1.v").is_empty(),
            "kept copies do not re-report: {diags:#?}"
        );
    }

    #[test]
    fn becomes_in_lhs_marks_variable_as_assigned() {
        // `x :∈ S` in an event is a modification via BecomesIn: despite the
        // INIT assignment, x is not constant-like — EB012 must not fire.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        m.invariants = vec![lp(
            "inv1",
            eq_pred(ident("x"), ExpressionKind::Integer(0).into()),
        )];
        m.initialisation = Some(init_event(&["x"], false));
        m.events = vec![Event {
            name: "evt".into(),
            status: None,
            refines: None,
            parameters: Vec::new(),
            guards: Vec::new(),
            with: Vec::new(),
            witnesses: Vec::new(),
            actions: vec![la(ActionKind::BecomesIn {
                variables: vec!["x".into()],
                set: ExpressionKind::Naturals.into(),
            }
            .into())],
            span: None,
            name_span: None,
            refines_span: None,
            comment: None,
            extended: false,
        }];

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        assert!(
            !diags
                .iter()
                .any(|d| d.rule_id == Some(RuleId::UnmodifiedVariable)),
            "x is assigned via BecomesIn: {diags:#?}"
        );
    }

    #[test]
    fn function_override_lhs_marks_variable_as_assigned() {
        // `f(1) := 0` lowered to `f ≔ f\u{E103}{1 ↦ 0}` — an event
        // modification of f, so f is not constant-like.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("f")];
        m.invariants = vec![lp(
            "inv1",
            eq_pred(ident("f"), ExpressionKind::Integer(0).into()),
        )];
        m.initialisation = Some(init_event(&["f"], false));
        let overwrite_rhs: Expression = ExpressionKind::Binary {
            op: BinaryOp::Overwrite,
            left: Box::new(ExpressionKind::Identifier("f".into()).into()),
            right: Box::new(
                ExpressionKind::SetEnumeration(vec![
                    ExpressionKind::Binary {
                        op: BinaryOp::Maplet,
                        left: Box::new(ExpressionKind::Integer(1).into()),
                        right: Box::new(ExpressionKind::Integer(0).into()),
                    }
                    .into(),
                ])
                .into(),
            ),
        }
        .into();
        m.events = vec![Event {
            name: "evt".into(),
            status: None,
            refines: None,
            parameters: Vec::new(),
            guards: Vec::new(),
            with: Vec::new(),
            witnesses: Vec::new(),
            actions: vec![la(ActionKind::Assignment {
                variables: vec!["f".into()],
                expressions: vec![overwrite_rhs],
            }
            .into())],
            span: None,
            name_span: None,
            refines_span: None,
            comment: None,
            extended: false,
        }];

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        assert!(
            !diags
                .iter()
                .any(|d| d.rule_id == Some(RuleId::UnmodifiedVariable)),
            "f is assigned via overwrite assignment: {diags:#?}"
        );
    }

    #[test]
    fn lambda_binder_does_not_count_as_use() {
        // The only mention of machine variable `x` is as a lambda parameter.
        // The lambda introduces a fresh binder, so the machine variable
        // should still be flagged dead.
        use rossi::IdentPattern;
        use rossi::TypedIdentifier;

        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        let lambda = Expression::from(ExpressionKind::Lambda {
            pattern: IdentPattern::Identifier(TypedIdentifier {
                name: "x".into(),
                type_expr: None,
                span: None,
            }),
            predicate: Box::new(PredicateKind::True.into()),
            expression: Box::new(ident("x")),
        });
        m.invariants = vec![lp(
            "inv1",
            eq_pred(lambda, ExpressionKind::Integer(0).into()),
        )];
        m.initialisation = Some(InitialisationEvent {
            actions: vec![la(Action::assignment(
                "x",
                ExpressionKind::Integer(0).into(),
            ))],
            comment: None,
            extended: false,
            with: Vec::new(),
            witnesses: Vec::new(),
            span: None,
            name_span: None,
        });

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        assert!(
            diags
                .iter()
                .any(|d| d.rule_id == Some(RuleId::DeadVariable) && d.message.contains("`x`")),
            "lambda binder must not satisfy reference to machine variable: {diags:#?}"
        );
    }

    #[test]
    fn no_initialisation_at_all_flags_every_variable() {
        // No INITIALISATION event present: each declared variable should
        // produce one EB014 diagnostic.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("a"), nv("b"), nv("c")];

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        let missing: Vec<_> = diags
            .iter()
            .filter(|d| d.rule_id == Some(RuleId::IncompleteInitialisation))
            .collect();
        assert_eq!(
            missing.len(),
            3,
            "expected one EB014 per variable: {diags:#?}"
        );
    }

    #[test]
    fn event_parameter_shadows_variable() {
        // Event has parameter named `x`; guard `x = 0` uses the parameter,
        // not the machine variable. The variable should still be flagged
        // unmodified (referenced=false, assigned=false → dead, not unmod).
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        m.events = vec![Event {
            name: "evt".into(),
            status: None,
            refines: None,
            parameters: vec![nv("x")],
            guards: vec![lp(
                "g1",
                eq_pred(ident("x"), ExpressionKind::Integer(0).into()),
            )],
            with: Vec::new(),
            witnesses: Vec::new(),
            actions: Vec::new(),
            span: None,
            name_span: None,
            refines_span: None,
            comment: None,
            extended: false,
        }];

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        // x is dead — the only mention is shadowed by the parameter.
        assert!(
            diags
                .iter()
                .any(|d| d.rule_id == Some(RuleId::DeadVariable) && d.message.contains('x')),
            "expected EB011 for shadowed variable: {diags:#?}"
        );
    }

    // ---------- EB024: new event assigns inherited variable -----------------

    /// A machine declaring `var` and initialising it (a plausible abstract
    /// machine to refine).
    fn abstract_machine(name: &str, var: &str) -> Machine {
        let mut m = Machine::new(name.into());
        m.variables = vec![nv(var)];
        m.initialisation = Some(init_event(&[var], false));
        m
    }

    /// A new (non-refining) event named `name` whose sole action assigns `var`.
    fn assigning_event(name: &str, var: &str) -> Event {
        let mut e = Event::new(name.into());
        e.actions = vec![la(Action::assignment(
            var,
            ExpressionKind::Integer(1).into(),
        ))];
        e
    }

    fn eb024(diags: &[Diagnostic]) -> Vec<&Diagnostic> {
        diags
            .iter()
            .filter(|d| d.rule_id == Some(RuleId::NewEventAssignsInheritedVariable))
            .collect()
    }

    #[test]
    fn new_event_assigning_inherited_variable_is_flagged() {
        // M2 refines M1; M1 owns `v` and M2 keeps it. A *new* event `step` in
        // M2 assigns the retained inherited `v` — an unprovable skip-refinement.
        // EB024 fires.
        let m1 = abstract_machine("M1", "v");
        let mut m2 = Machine::new("M2".into());
        m2.refines = Some("M1".into());
        m2.variables = vec![nv("v")];
        m2.events = vec![assigning_event("step", "v")];

        let diags = run(&proj(vec![
            pc("M1.bum", Component::Machine(m1)),
            pc("M2.bum", Component::Machine(m2)),
        ]));
        let found = eb024(&diags);
        assert_eq!(found.len(), 1, "{diags:#?}");
        assert_eq!(found[0].origin, "M2.step");
        assert_eq!(found[0].severity, Severity::Error);
        assert!(found[0].message.contains("`v`"), "{}", found[0].message);
    }

    #[test]
    fn new_event_assigning_own_variable_is_not_flagged() {
        // `w` is introduced at M2's level, so a new event may assign it.
        let m1 = abstract_machine("M1", "v");
        let mut m2 = Machine::new("M2".into());
        m2.refines = Some("M1".into());
        m2.variables = vec![nv("w")];
        m2.events = vec![assigning_event("step", "w")];

        let diags = run(&proj(vec![
            pc("M1.bum", Component::Machine(m1)),
            pc("M2.bum", Component::Machine(m2)),
        ]));
        assert!(eb024(&diags).is_empty(), "{diags:#?}");
    }

    #[test]
    fn refining_event_assigning_inherited_is_not_flagged() {
        // A refining event legitimately refines an abstract event that may
        // change `v` — not a new event, so EB024 must stay silent.
        let m1 = abstract_machine("M1", "v");
        let mut m2 = Machine::new("M2".into());
        m2.refines = Some("M1".into());
        m2.variables = vec![nv("v")];
        let mut e = assigning_event("step", "v");
        e.refines = Some("abstract_step".into());
        m2.events = vec![e];

        let diags = run(&proj(vec![
            pc("M1.bum", Component::Machine(m1)),
            pc("M2.bum", Component::Machine(m2)),
        ]));
        assert!(eb024(&diags).is_empty(), "{diags:#?}");
    }

    #[test]
    fn extended_event_assigning_inherited_is_not_flagged() {
        // An extended event copies and extends its abstract counterpart; it is
        // not a new event.
        let m1 = abstract_machine("M1", "v");
        let mut m2 = Machine::new("M2".into());
        m2.refines = Some("M1".into());
        m2.variables = vec![nv("v")];
        let mut e = assigning_event("step", "v");
        e.extended = true;
        m2.events = vec![e];

        let diags = run(&proj(vec![
            pc("M1.bum", Component::Machine(m1)),
            pc("M2.bum", Component::Machine(m2)),
        ]));
        assert!(eb024(&diags).is_empty(), "{diags:#?}");
    }

    #[test]
    fn initialisation_assigning_inherited_is_not_flagged() {
        // INITIALISATION must assign inherited variables; it is stored apart
        // from `events`, so the pass never sees it. No EB024.
        let m1 = abstract_machine("M1", "v");
        let mut m2 = Machine::new("M2".into());
        m2.refines = Some("M1".into());
        m2.variables = vec![nv("v")];
        m2.initialisation = Some(InitialisationEvent {
            actions: vec![la(Action::assignment(
                "v",
                ExpressionKind::Integer(0).into(),
            ))],
            comment: None,
            extended: false,
            with: Vec::new(),
            witnesses: Vec::new(),
            span: None,
            name_span: None,
        });

        let diags = run(&proj(vec![
            pc("M1.bum", Component::Machine(m1)),
            pc("M2.bum", Component::Machine(m2)),
        ]));
        assert!(eb024(&diags).is_empty(), "{diags:#?}");
    }

    #[test]
    fn root_machine_new_event_is_not_flagged() {
        // A root machine has no inherited variables, so every variable a new
        // event assigns is introduced at its own level.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        m.events = vec![assigning_event("step", "x")];

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        assert!(eb024(&diags).is_empty(), "{diags:#?}");
    }

    #[test]
    fn new_event_assigning_grandparent_variable_is_flagged() {
        // M3 refines M2 refines M1; `v` is owned by the grandparent M1 and kept
        // down the chain. The ancestor walk must still recognise `v` as a
        // retained inherited variable in M3.
        let m1 = abstract_machine("M1", "v");
        let mut m2 = Machine::new("M2".into());
        m2.refines = Some("M1".into());
        m2.variables = vec![nv("v")];
        let mut m3 = Machine::new("M3".into());
        m3.refines = Some("M2".into());
        m3.variables = vec![nv("v")];
        m3.events = vec![assigning_event("step", "v")];

        let diags = run(&proj(vec![
            pc("M1.bum", Component::Machine(m1)),
            pc("M2.bum", Component::Machine(m2)),
            pc("M3.bum", Component::Machine(m3)),
        ]));
        let found = eb024(&diags);
        assert_eq!(found.len(), 1, "{diags:#?}");
        assert_eq!(found[0].origin, "M3.step");
    }

    #[test]
    fn new_event_assigning_dropped_variable_is_not_flagged() {
        // M2 refines M1 (owns `v`) but does NOT redeclare `v` — it is data-
        // refined away. A new event assigning the dropped `v` is the EB025
        // (disappeared-variable) build error's domain, NOT EB024; the lint must
        // not double-report it here.
        let m1 = abstract_machine("M1", "v");
        let mut m2 = Machine::new("M2".into());
        m2.refines = Some("M1".into());
        m2.events = vec![assigning_event("step", "v")];

        let diags = run(&proj(vec![
            pc("M1.bum", Component::Machine(m1)),
            pc("M2.bum", Component::Machine(m2)),
        ]));
        assert!(eb024(&diags).is_empty(), "{diags:#?}");
    }

    // ---------- duplicate component names (EB019) ----------------------------
    //
    // Component names need not be unique — EB019 reports the duplication
    // itself. The index keys everything by arena id so the other lints stay
    // deterministic: each duplicate is judged against its own text, an
    // ambiguous REFINES target truncates like a missing one, and the
    // suppression-feeding edges attach to every same-name candidate. Every
    // test asserts both component orders — a name-keyed index is last-wins,
    // so its verdicts flipped with load order.

    /// Run the same project in the given component order and in reverse.
    fn run_both_orders(components: Vec<ProjectComponent>) -> [Vec<Diagnostic>; 2] {
        let mut reversed = components.clone();
        reversed.reverse();
        [run(&proj(components)), run(&proj(reversed))]
    }

    /// A machine whose variable `var` is fully accounted for by its own
    /// text: declared, referenced by an invariant, and initialised.
    fn self_contained_machine(name: &str, var: &str) -> Machine {
        let mut m = abstract_machine(name, var);
        m.invariants = vec![lp(
            "inv1",
            eq_pred(ident(var), ExpressionKind::Integer(0).into()),
        )];
        m
    }

    #[test]
    fn duplicate_machines_are_judged_on_their_own_text() {
        // Two machines named `M`: the first fully accounts for its variable
        // `x`, the second is empty. A name-keyed index judged the first
        // against the second's (empty) sets in one of the two orders,
        // producing false EB011/EB012/EB014 on `x`.
        let m1 = self_contained_machine("M", "x");
        let m2 = Machine::new("M".into());

        for diags in run_both_orders(vec![
            pc("a/M.bum", Component::Machine(m1)),
            pc("b/M.bum", Component::Machine(m2)),
        ]) {
            assert!(eb011_on(&diags, "M.x").is_empty(), "{diags:#?}");
            assert!(eb014_on(&diags, "M").is_empty(), "{diags:#?}");
        }
    }

    #[test]
    fn extended_init_through_duplicated_ancestor_stays_silent() {
        // C's extended INIT inherits from `X` — but two machines carry that
        // name, so the chain is unresolvable and EB014 must not judge
        // completeness against either candidate's assignments.
        let x1 = abstract_machine("X", "a");
        let x2 = abstract_machine("X", "b");
        let mut c = Machine::new("C".into());
        c.refines = Some("X".into());
        c.variables = vec![nv("a")];
        c.initialisation = Some(init_event(&[], true));

        for diags in run_both_orders(vec![
            pc("a/X.bum", Component::Machine(x1)),
            pc("b/X.bum", Component::Machine(x2)),
            pc("C.bum", Component::Machine(c)),
        ]) {
            assert!(eb014_on(&diags, "C").is_empty(), "{diags:#?}");
        }
    }

    #[test]
    fn duplicate_children_both_suppress_ancestor_dead_variable() {
        // R's `v` is referenced only by one of two refining machines that
        // share the name `X`. Each child's own-text references must survive
        // in the downward union — a name-keyed index kept only the last
        // `X`, so `v` went dead in one component order.
        let r = abstract_machine("R", "v");
        let mut x1 = Machine::new("X".into());
        x1.refines = Some("R".into());
        x1.events = vec![guarded_event("e", "v")];
        let mut x2 = Machine::new("X".into());
        x2.refines = Some("R".into());

        for diags in run_both_orders(vec![
            pc("R.bum", Component::Machine(r)),
            pc("a/X.bum", Component::Machine(x1)),
            pc("b/X.bum", Component::Machine(x2)),
        ]) {
            assert!(eb011_on(&diags, "R.v").is_empty(), "{diags:#?}");
        }
    }

    #[test]
    fn child_of_duplicated_parent_is_not_reference_linted() {
        // Only one `X` carries the invariant referencing C's kept `w`, so
        // C's materialised reference set is unknowable. EB011/EB012 stay
        // silent in both orders; the EB019 Error owns the report.
        let x1 = self_contained_machine("X", "w");
        let x2 = Machine::new("X".into());
        let mut c = abstract_machine("C", "w");
        c.refines = Some("X".into());

        for diags in run_both_orders(vec![
            pc("a/X.bum", Component::Machine(x1)),
            pc("b/X.bum", Component::Machine(x2)),
            pc("C.bum", Component::Machine(c)),
        ]) {
            assert!(eb011_on(&diags, "C.w").is_empty(), "{diags:#?}");
        }
    }

    #[test]
    fn machine_with_missing_parent_is_not_reference_linted() {
        // M refines an absent machine, so its materialised reference set is
        // unknowable (EB009's domain): neither the unreferenced `d` (EB011)
        // nor the referenced-but-never-assigned `u` (EB012) is judged. The
        // ancestry-independent EB014 still fires — the machine is not
        // exempt from linting wholesale.
        let mut m = Machine::new("M".into());
        m.refines = Some("Absent".into());
        m.variables = vec![nv("d"), nv("u")];
        m.invariants = vec![lp(
            "inv1",
            eq_pred(ident("u"), ExpressionKind::Integer(0).into()),
        )];
        m.initialisation = Some(init_event(&[], false));

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        assert!(eb011_on(&diags, "M.d").is_empty(), "{diags:#?}");
        assert!(
            diags_on(&diags, RuleId::UnmodifiedVariable, "M.u").is_empty(),
            "{diags:#?}"
        );
        assert_eq!(eb014_on(&diags, "M").len(), 2, "{diags:#?}");
    }

    #[test]
    fn machines_in_a_refines_cycle_are_not_reference_linted() {
        // A refines B refines A: the walk truncates (EB007/EB008's domain),
        // so A's unreferenced `v` is not judged.
        let mut a = abstract_machine("A", "v");
        a.refines = Some("B".into());
        let mut b = Machine::new("B".into());
        b.refines = Some("A".into());

        for diags in run_both_orders(vec![
            pc("A.bum", Component::Machine(a)),
            pc("B.bum", Component::Machine(b)),
        ]) {
            assert!(eb011_on(&diags, "A.v").is_empty(), "{diags:#?}");
        }
    }

    #[test]
    fn duplicate_refining_its_own_name_terminates() {
        // One of two machines named `X` refines `X`: resolution is
        // ambiguous immediately (both carriers are candidates), so the
        // walk truncates instead of looping, and the extended INIT is not
        // judged.
        let mut x1 = Machine::new("X".into());
        x1.refines = Some("X".into());
        x1.variables = vec![nv("v")];
        x1.initialisation = Some(init_event(&[], true));
        let x2 = Machine::new("X".into());

        for diags in run_both_orders(vec![
            pc("a/X.bum", Component::Machine(x1)),
            pc("b/X.bum", Component::Machine(x2)),
        ]) {
            assert!(eb014_on(&diags, "X").is_empty(), "{diags:#?}");
        }
    }

    #[test]
    fn eb024_stays_silent_under_a_duplicated_parent_name() {
        // C refines the duplicated name `X` and a new event assigns C's
        // kept `w`. Whether `w` is inherited depends on WHICH `X` — with
        // the chain unresolvable the inherited-variable set is empty, so
        // EB024 (an Error) must not guess. The name-keyed index fired it
        // in one of the two orders.
        let x1 = abstract_machine("X", "w");
        let x2 = Machine::new("X".into());
        let mut c = abstract_machine("C", "w");
        c.refines = Some("X".into());
        c.events = vec![assigning_event("step", "w")];

        for diags in run_both_orders(vec![
            pc("a/X.bum", Component::Machine(x1)),
            pc("b/X.bum", Component::Machine(x2)),
            pc("C.bum", Component::Machine(c)),
        ]) {
            assert!(eb024(&diags).is_empty(), "{diags:#?}");
        }
    }

    #[test]
    fn machine_and_context_sharing_a_name_do_not_interfere() {
        // Machines and contexts resolve in separate namespaces: the context
        // `N` is no REFINES candidate, so `C refines N` still resolves
        // uniquely to the machine and C's kept `v` stays alive through the
        // inherited invariant. (The cross-kind collision itself is EB019,
        // reported by the SC build, not by the lints.)
        let n_machine = self_contained_machine("N", "v");
        let n_context = Context::new("N".into());
        let mut c = abstract_machine("C", "v");
        c.refines = Some("N".into());

        for diags in run_both_orders(vec![
            pc("N.bum", Component::Machine(n_machine)),
            pc("N.buc", Component::Context(n_context)),
            pc("C.bum", Component::Machine(c)),
        ]) {
            assert!(eb011_on(&diags, "C.v").is_empty(), "{diags:#?}");
        }
    }

    #[test]
    fn component_order_does_not_change_the_diagnostic_set() {
        // Umbrella: a duplicate-heavy project produces the same diagnostic
        // set regardless of load order. (EB019 itself is the SC build's
        // report, not a lint, so it does not appear here.)
        let m1 = self_contained_machine("M", "x");
        let mut m2 = Machine::new("M".into());
        m2.variables = vec![nv("y")]; // genuinely dead in every order
        let x1 = self_contained_machine("X", "w");
        let x2 = abstract_machine("X", "b");
        let mut c = Machine::new("C".into());
        c.refines = Some("X".into());
        c.variables = vec![nv("w")];
        c.initialisation = Some(init_event(&[], true));
        let mut k1 = Context::new("K".into());
        k1.constants = vec![nv("k1")];
        let mut k2 = Context::new("K".into());
        k2.constants = vec![nv("k2")];
        let mut s = Machine::new("S".into());
        s.sees = vec!["K".into()];
        s.events = vec![guarded_event("e1", "k1"), guarded_event("e2", "k2")];

        let [fwd, rev] = run_both_orders(vec![
            pc("a/M.bum", Component::Machine(m1)),
            pc("b/M.bum", Component::Machine(m2)),
            pc("a/X.bum", Component::Machine(x1)),
            pc("b/X.bum", Component::Machine(x2)),
            pc("C.bum", Component::Machine(c)),
            pc("a/K.buc", Component::Context(k1)),
            pc("b/K.buc", Component::Context(k2)),
            pc("S.bum", Component::Machine(s)),
        ]);
        let key_set = |diags: &[Diagnostic]| -> BTreeSet<String> {
            // Keyed on the canonical rendering, severity included.
            diags.iter().map(ToString::to_string).collect()
        };
        let (fwd_set, rev_set) = (key_set(&fwd), key_set(&rev));
        assert_eq!(fwd_set, rev_set, "fwd: {fwd:#?}\nrev: {rev:#?}");
        // The genuinely dead variable is still reported (the arena keeps
        // duplicate judgment deterministic, not silent).
        assert_eq!(eb011_on(&fwd, "M.y").len(), 1, "{fwd:#?}");
    }

    // ---------- EB011/EB012 repartition: typing shapes, constant-like -------

    /// A typing invariant `var ∈ set` for the machine's own variable.
    fn typing_inv(var: &str, set: Expression) -> LabeledPredicate {
        lp("typ", cmp_pred(ComparisonOp::In, ident(var), set))
    }

    fn eb012_on<'d>(diags: &'d [Diagnostic], origin: &str) -> Vec<&'d Diagnostic> {
        diags_on(diags, RuleId::UnmodifiedVariable, origin)
    }

    #[test]
    fn typing_only_reference_does_not_keep_variable_alive() {
        // `x ∈ ℤ` merely types x; nothing else mentions or assigns it.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        m.invariants = vec![typing_inv("x", ExpressionKind::Integers.into())];
        m.initialisation = Some(init_event(&[], false));

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        assert_eq!(eb011_on(&diags, "M.x").len(), 1, "{diags:#?}");
    }

    #[test]
    fn typed_initialised_unused_variable_is_dead_not_constant_like() {
        // Fully groomed — typed and initialised — but nothing uses x and no
        // event writes it: EB011's dead case, not EB012's constant-like.
        // Each variable draws at most one of the two advisories.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        m.invariants = vec![typing_inv("x", ExpressionKind::Integers.into())];
        m.initialisation = Some(init_event(&["x"], false));

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        assert_eq!(eb011_on(&diags, "M.x").len(), 1, "{diags:#?}");
        assert!(eb012_on(&diags, "M.x").is_empty(), "{diags:#?}");
    }

    #[test]
    fn non_typing_conjunct_keeps_variable_alive() {
        // `x ∈ ℤ ∧ x > 0`: the second conjunct constrains the value — a
        // real use. Being INIT-only, x is constant-like rather than dead.
        let conj: Predicate = PredicateKind::Logical {
            op: LogicalOp::And,
            left: Box::new(cmp_pred(
                ComparisonOp::In,
                ident("x"),
                ExpressionKind::Integers.into(),
            )),
            right: Box::new(cmp_pred(
                ComparisonOp::GreaterThan,
                ident("x"),
                ExpressionKind::Integer(0).into(),
            )),
        }
        .into();
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        m.invariants = vec![lp("inv1", conj)];
        m.initialisation = Some(init_event(&["x"], false));

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        assert!(eb011_on(&diags, "M.x").is_empty(), "{diags:#?}");
        assert_eq!(eb012_on(&diags, "M.x").len(), 1, "{diags:#?}");
    }

    #[test]
    fn variable_bound_makes_membership_a_constraint() {
        // `y ⊆ x` relates two pieces of machine state — a constraint, not
        // typing: BOTH variables count as used.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x"), nv("y")];
        m.invariants = vec![lp(
            "inv1",
            cmp_pred(ComparisonOp::Subset, ident("y"), ident("x")),
        )];
        m.initialisation = Some(init_event(&["x", "y"], false));

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        assert!(eb011_on(&diags, "M.x").is_empty(), "{diags:#?}");
        assert!(eb011_on(&diags, "M.y").is_empty(), "{diags:#?}");
    }

    #[test]
    fn static_bound_membership_is_typing() {
        // `y ⊆ s` with a variable-free bound is the typing idiom: the
        // bound's identifiers count as used (s), the typed variable does
        // not — y is dead.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("y")];
        m.invariants = vec![lp(
            "inv1",
            cmp_pred(ComparisonOp::Subset, ident("y"), ident("s")),
        )];
        m.initialisation = Some(init_event(&["y"], false));

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        assert_eq!(eb011_on(&diags, "M.y").len(), 1, "{diags:#?}");
    }

    #[test]
    fn theorem_membership_counts_as_use() {
        // A theorem is a derived property, not a typing declaration: even
        // a membership-shaped one uses the variable.
        let mut thm = lp(
            "thm1",
            cmp_pred(
                ComparisonOp::In,
                ident("x"),
                ExpressionKind::Naturals.into(),
            ),
        );
        thm.is_theorem = true;
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        m.invariants = vec![typing_inv("x", ExpressionKind::Integers.into()), thm];
        m.initialisation = Some(init_event(&["x"], false));

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        assert!(eb011_on(&diags, "M.x").is_empty(), "{diags:#?}");
    }

    #[test]
    fn variable_bound_gluing_invariant_counts_as_use() {
        // The corpus shape from the review: `cur ∈ dom(routes)` constrains
        // cur against another variable — cur is used, not dead.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("cur"), nv("routes")];
        m.invariants = vec![lp(
            "inv1",
            cmp_pred(
                ComparisonOp::In,
                ident("cur"),
                ExpressionKind::FunctionApplication {
                    function: Box::new(ident("dom")),
                    arguments: vec![ident("routes")],
                }
                .into(),
            ),
        )];
        m.initialisation = Some(init_event(&["cur"], false));
        m.events = vec![assigning_event("step", "routes")];

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        assert!(eb011_on(&diags, "M.cur").is_empty(), "{diags:#?}");
        // And it is not \"constant-like\" either: the constraint reads state.
        assert_eq!(eb012_on(&diags, "M.cur").len(), 1, "{diags:#?}");
    }

    #[test]
    fn strict_subset_counts_as_use() {
        // `x ⊂ s` is a proper-subset constraint, not mere typing.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        m.invariants = vec![lp(
            "inv1",
            cmp_pred(ComparisonOp::SubsetStrict, ident("x"), ident("s")),
        )];
        m.initialisation = Some(init_event(&["x"], false));

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        assert!(eb011_on(&diags, "M.x").is_empty(), "{diags:#?}");
    }

    #[test]
    fn write_only_variable_is_neither_dead_nor_constant_like() {
        // An output: events write w, nothing reads it. Deleting it would
        // break the writing event, and it is certainly no constant.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("w")];
        m.initialisation = Some(init_event(&["w"], false));
        m.events = vec![assigning_event("emit", "w")];

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        assert!(eb011_on(&diags, "M.w").is_empty(), "{diags:#?}");
        assert!(eb012_on(&diags, "M.w").is_empty(), "{diags:#?}");
    }

    #[test]
    fn nondeterministic_init_is_still_constant_like() {
        // `x :∈ ℕ` at INIT — chosen once, never modified. A CONSTANT with
        // axiom `x ∈ ℕ` has identical traces.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        m.invariants = vec![lp(
            "inv1",
            eq_pred(ident("x"), ExpressionKind::Integer(0).into()),
        )];
        m.initialisation = Some(InitialisationEvent {
            actions: vec![la(ActionKind::BecomesIn {
                variables: vec!["x".into()],
                set: ExpressionKind::Naturals.into(),
            }
            .into())],
            comment: None,
            extended: false,
            with: Vec::new(),
            witnesses: Vec::new(),
            span: None,
            name_span: None,
        });

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        assert_eq!(eb012_on(&diags, "M.x").len(), 1, "{diags:#?}");
    }

    #[test]
    fn truncated_chain_skips_constant_like() {
        // C refines an absent machine: its write-set is speculation, so the
        // constant advice stays silent (the same gate as EB011).
        let mut c = abstract_machine("C", "v");
        c.refines = Some("Absent".into());
        c.invariants = vec![lp(
            "inv1",
            eq_pred(ident("v"), ExpressionKind::Integer(0).into()),
        )];

        let diags = run(&proj(vec![pc("C.bum", Component::Machine(c))]));
        assert!(eb012_on(&diags, "C.v").is_empty(), "{diags:#?}");
    }

    #[test]
    fn inherited_clause_reference_suppresses_same_named_new_variable() {
        // Deliberate conservatism in the inherited reference set: M0's
        // abstract guard references the name `c` (say, a seen constant);
        // M1 declares a NEW variable also named `c` and extends the event.
        // References are name-based, so the inherited guard keeps M1's `c`
        // out of EB011 — the collision itself is EB021/EB023's report.
        let mut m0 = Machine::new("M0".into());
        m0.initialisation = Some(init_event(&[], false));
        m0.events = vec![guarded_event("e", "c")];
        let mut m1 = Machine::new("M1".into());
        m1.refines = Some("M0".into());
        m1.variables = vec![nv("c")];
        m1.initialisation = Some(init_event(&["c"], false));
        m1.events = vec![extends_event("e", Some("e"))];

        let diags = run(&proj(vec![
            pc("M0.bum", Component::Machine(m0)),
            pc("M1.bum", Component::Machine(m1)),
        ]));
        assert!(eb011_on(&diags, "M1.c").is_empty(), "{diags:#?}");
    }
}
