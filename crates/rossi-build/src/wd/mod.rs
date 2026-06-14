//! Well-definedness conditions (EB010).
//!
//! Mirrors eventb-checker's `WellDefinednessChecker`: for every formula
//! the static check accepted, compute its WD lemma (Rodin's L-operator,
//! [`computer`]), simplify it (Rodin's `WDImprover`, [`mod@improve`]), and
//! report the survivors as INFO diagnostics with the message
//! `Well-definedness condition: <lemma>` — byte-identical to
//! eventb-checker, which prints Rodin's `Predicate#toString()`
//! ([`render`]).
//!
//! Formulas come from the *raw* component ASTs (the lemma embeds
//! verbatim fragments of the source, including its comprehension forms);
//! the [`ScModel`] supplies the type environments and decides which
//! formulas the static check kept. Coverage matches eventb-checker:
//! context axioms and theorems; machine invariants, theorems, and
//! variant; event guards, actions, and witnesses (including
//! INITIALISATION).

pub mod builder;
pub mod computer;
pub mod improve;
pub mod normal;
pub mod render;

use std::collections::HashSet;

use rossi::{Action, Component, Expression, Predicate};

use crate::project::Project;
use crate::sc::{CheckedContext, CheckedMachine, ScModel};
use crate::{Diagnostic, RuleId, Severity};

use computer::WdComputer;
use improve::improve;
use normal::{flatten, resolve_binders};
use render::render_predicate;

/// Compute WD findings for every successfully-checked component of the
/// project, in declaration order.
pub fn run(project: &Project, model: &ScModel) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    // The [`ScModel`] is keyed by component name, so a project with two
    // components of the same name (a duplicate already flagged EB019) has
    // a single model entry for both. Check each name once to avoid pairing
    // a raw component against another's environment and emitting duplicate
    // EB010 rows.
    let mut seen: HashSet<&str> = HashSet::new();
    for pc in &project.components {
        match &pc.component {
            Component::Context(ctx) => {
                if seen.insert(&ctx.name)
                    && let Some(cc) = model.contexts.get(&ctx.name)
                {
                    check_context(ctx, cc, &mut out);
                }
            }
            Component::Machine(m) => {
                if seen.insert(&m.name)
                    && let Some(cm) = model.machines.get(&m.name)
                {
                    check_machine(m, cm, &mut out);
                }
            }
        }
    }
    out
}

/// Yields the raw clauses the SC kept, paired by source *position* rather
/// than label: two clauses can share an (effective) label, so label-set
/// membership would WD-check a clause the SC actually dropped. `idx` reads
/// each kept decl's `source_index`; `raw` is the verbatim source list.
/// Centralized so all seven call sites share one pairing implementation.
fn kept_clauses<'a, D, R: 'a>(
    decls: &[D],
    idx: impl Fn(&D) -> usize,
    raw: impl Iterator<Item = &'a R> + 'a,
) -> impl Iterator<Item = &'a R> + 'a {
    let kept: HashSet<usize> = decls.iter().map(idx).collect();
    raw.enumerate()
        .filter_map(move |(i, item)| kept.contains(&i).then_some(item))
}

fn check_context(
    ctx: &rossi::ast::context::Context,
    cc: &CheckedContext,
    out: &mut Vec<Diagnostic>,
) {
    // One computer per environment: the env is restored after each formula
    // (every binder scope is popped), so reuse avoids re-cloning it.
    let mut computer = WdComputer::new(cc.env().clone());
    for ax in kept_clauses(&cc.record.axioms, |a| a.source_index, ctx.axioms.iter()) {
        let label = effective(&ax.label, "axm");
        check(
            &mut computer,
            Formula::Pred(&ax.predicate),
            format!("{}.{}", ctx.name, label),
            out,
        );
    }
}

fn check_machine(m: &rossi::Machine, cm: &CheckedMachine, out: &mut Vec<Diagnostic>) {
    // Invariants and the variant share the machine environment; one computer
    // serves both (it self-restores after each formula).
    let mut mc = WdComputer::new(cm.env().clone());
    for inv in kept_clauses(
        &cm.record.invariants,
        |i| i.source_index,
        m.invariants.iter(),
    ) {
        let label = effective(&inv.label, "inv");
        check(
            &mut mc,
            Formula::Pred(&inv.predicate),
            format!("{}.{}", m.name, label),
            out,
        );
    }

    if let (Some(variant), Some(decl)) = (&m.variant, &cm.record.variant) {
        check(
            &mut mc,
            Formula::Expr(variant),
            format!("{}.{}", m.name, decl.label),
            out,
        );
    }

    if let (Some(init), Some(decl)) = (&m.initialisation, cm.events_by_label.get("INITIALISATION"))
    {
        // `event_env` builds an owned env per event — move it into the
        // computer (no clone) and reuse it for the INIT actions + witnesses.
        let mut ec = WdComputer::new(cm.event_env(decl));
        for act in kept_clauses(&decl.actions, |a| a.source_index, init.actions.iter()) {
            let label = effective(&act.label, "act");
            check(
                &mut ec,
                Formula::Act(&act.action),
                format!("{}.INITIALISATION/{}", m.name, label),
                out,
            );
        }
        // Witnesses are paired in the SC's build order (`witnesses` then
        // `with`); see [`super::sc::machine_record::WitnessDecl::source_index`].
        for wit in kept_clauses(
            &decl.witnesses,
            |w| w.source_index,
            init.witnesses.iter().chain(&init.with),
        ) {
            let label = effective(&wit.label, "wit");
            check(
                &mut ec,
                Formula::Pred(&wit.predicate),
                format!("{}.INITIALISATION/{}", m.name, label),
                out,
            );
        }
    }

    for event in &m.events {
        let Some(decl) = cm.events_by_label.get(&event.name) else {
            continue;
        };
        let mut ec = WdComputer::new(cm.event_env(decl));

        for guard in kept_clauses(&decl.guards, |g| g.source_index, event.guards.iter()) {
            let label = effective(&guard.label, "grd");
            check(
                &mut ec,
                Formula::Pred(&guard.predicate),
                format!("{}.{}/{}", m.name, event.name, label),
                out,
            );
        }

        for act in kept_clauses(&decl.actions, |a| a.source_index, event.actions.iter()) {
            let label = effective(&act.label, "act");
            check(
                &mut ec,
                Formula::Act(&act.action),
                format!("{}.{}/{}", m.name, event.name, label),
                out,
            );
        }

        // Witnesses paired in `witnesses`-then-`with` order (see INIT).
        for wit in kept_clauses(
            &decl.witnesses,
            |w| w.source_index,
            event.witnesses.iter().chain(&event.with),
        ) {
            let label = effective(&wit.label, "wit");
            check(
                &mut ec,
                Formula::Pred(&wit.predicate),
                format!("{}.{}/{}", m.name, event.name, label),
                out,
            );
        }
    }
}

enum Formula<'a> {
    Pred(&'a Predicate),
    Expr(&'a Expression),
    Act(&'a Action),
}

/// The eventb-checker recipe: compute, drop trivially-true lemmas,
/// improve, drop again, report. A formula whose lemma cannot be fully
/// built (untypeable function application) is skipped rather than
/// reported partially.
fn check(
    computer: &mut WdComputer,
    formula: Formula<'_>,
    origin: String,
    out: &mut Vec<Diagnostic>,
) {
    computer.reset();
    let lemma = match formula {
        Formula::Pred(p) => computer.wd_predicate(p),
        Formula::Expr(e) => computer.wd_expression(e),
        Formula::Act(a) => computer.wd_action(a),
    };
    if computer.failed.is_some() || lemma == Predicate::True {
        return;
    }
    // Rodin: getWDLemma() flattens, improve() flattens again at the end,
    // and toString resolves bound-name collisions.
    let improved = flatten(improve(flatten(lemma)));
    if improved == Predicate::True {
        return;
    }
    out.push(Diagnostic {
        severity: Severity::Info,
        origin,
        message: format!(
            "Well-definedness condition: {}",
            render_predicate(&resolve_binders(&improved))
        ),
        rule_id: Some(RuleId::WellDefinedness),
    });
}

fn effective<'a>(label: &'a Option<String>, default: &'static str) -> &'a str {
    label.as_deref().unwrap_or(default)
}
