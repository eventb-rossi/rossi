//! Corpus gate: reasoner replay against recorded proof rules.
//!
//! For every rule node of every stored proof, when the reasoner has a
//! Rust implementation, re-run it on its recorded input against the
//! sequent the stored proof path produces there, and deep-compare the
//! produced rule with the recorded one: goal, needed hypotheses (as a
//! set — some are serialized from hash-ordered collections),
//! confidence, and each antecedent's goal, added hypotheses, added
//! identifiers, and hypothesis actions in order. This is deliberately
//! stronger than the reference replay, which only checks antecedent
//! arity.
//! The display string is never compared.
//!
//! Nodes whose reasoners are external provers (oracle), unimplemented,
//! or untrusted are measured, not failed; descent stops where the
//! stored rule no longer applies (broken proofs). Stored rewrites
//! whose recorded output round-trips to their input — only an
//! in-memory shape the printer hides changed, such as an associative
//! nesting built by the proof-obligation generator — are measured as
//! `invisible_rewrite`: no replay from serialized formulas can
//! reproduce them. Any `replayed_diff` or reasoner error on an
//! unflagged model gates. When the corpus
//! carries a `replay_results.tsv` baseline, per-model `replayed_eq`
//! must also not regress.
//!
//! Run with `--ignored`; the per-model report lands in
//! `target/rossi-build-proof-replay-corpus.tsv` and the per-reasoner
//! coverage table in `target/rossi-build-proof-replay-reasoners.tsv`.

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use rayon::prelude::*;

use common::{
    collect_zips, corpus_dir, flagged_unsupported, load_flags, prove_known_divergence,
    workspace_root, write_report,
};
use rossi_prove::bpr::{self, Keep, ProofBody, ProofEntry};
use rossi_prove::bps::read_bps;
use rossi_prove::po_loader::PoProject;
use rossi_prove::{
    Antecedent, HypAction, ReasonerProvider, Registration, RegistryProvider, ReplayHints, Rule,
    Skeleton,
};

#[derive(Default)]
struct Counts {
    nodes: usize,
    replayed_eq: usize,
    replayed_diff: usize,
    oracle: usize,
    unimplemented: usize,
    untrusted: usize,
    error: usize,
    apply_stop: usize,
    stale_selection: usize,
    kept_drift: usize,
    invisible_rewrite: usize,
}

impl Counts {
    fn add(&mut self, other: &Counts) {
        self.nodes += other.nodes;
        self.replayed_eq += other.replayed_eq;
        self.replayed_diff += other.replayed_diff;
        self.oracle += other.oracle;
        self.unimplemented += other.unimplemented;
        self.untrusted += other.untrusted;
        self.error += other.error;
        self.apply_stop += other.apply_stop;
        self.stale_selection += other.stale_selection;
        self.kept_drift += other.kept_drift;
        self.invisible_rewrite += other.invisible_rewrite;
    }
}

/// Per-reasoner-id coverage: how many nodes exist and how they fared.
#[derive(Default)]
struct ReasonerCounts {
    nodes: usize,
    replayed_eq: usize,
    replayed_diff: usize,
}

impl ReasonerCounts {
    fn add(&mut self, other: &ReasonerCounts) {
        self.nodes += other.nodes;
        self.replayed_eq += other.replayed_eq;
        self.replayed_diff += other.replayed_diff;
    }
}

/// Set equality on needed hypotheses: some (contrHyps) are built
/// from hash-ordered collections, so their serialization order is not
/// meaningful.
fn hyp_set_eq(a: &[rossi::formula::Predicate], b: &[rossi::formula::Predicate]) -> bool {
    a.iter().all(|p| b.contains(p)) && b.iter().all(|p| a.contains(p))
}

/// Hypothesis-action equality: a bijective multiset comparison — an
/// auto-rewrite rule emits one action per visible hypothesis in the
/// sequent's iteration order, and a reusable-but-kept proof may have
/// been recorded against an obligation listing the same hypotheses in
/// another order. Each action's predicate lists also compare as sets
/// (the proof serializer emits them in hash order).
fn actions_eq(a: &[HypAction], b: &[HypAction]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut used = vec![false; b.len()];
    a.iter().all(|x| {
        b.iter().enumerate().any(|(index, y)| {
            if !used[index] && action_eq(x, y) {
                used[index] = true;
                true
            } else {
                false
            }
        })
    })
}

fn action_eq(a: &HypAction, b: &HypAction) -> bool {
    match (a, b) {
        (HypAction::Select(x), HypAction::Select(y))
        | (HypAction::Deselect(x), HypAction::Deselect(y))
        | (HypAction::Hide(x), HypAction::Hide(y))
        | (HypAction::Show(x), HypAction::Show(y)) => hyp_set_eq(x, y),
        (
            HypAction::ForwardInf {
                hyps: h1,
                added_idents: i1,
                inferred: f1,
            },
            HypAction::ForwardInf {
                hyps: h2,
                added_idents: i2,
                inferred: f2,
            },
        ) => hyp_set_eq(h1, h2) && i1 == i2 && hyp_set_eq(f1, f2),
        (
            HypAction::Rewrite {
                hyps: h1,
                added_idents: i1,
                inferred: f1,
                disappearing: d1,
            },
            HypAction::Rewrite {
                hyps: h2,
                added_idents: i2,
                inferred: f2,
                disappearing: d2,
            },
        ) => hyp_set_eq(h1, h2) && i1 == i2 && hyp_set_eq(f1, f2) && hyp_set_eq(d1, d2),
        _ => false,
    }
}

fn antecedents_eq(a: &Antecedent, b: &Antecedent) -> bool {
    // Added hypotheses (and the unselected subset) compare as sets:
    // The proof serializer emits them in hash order.
    a.goal == b.goal
        && hyp_set_eq(&a.added_hyps, &b.added_hyps)
        && hyp_set_eq(&a.unselected_added, &b.unselected_added)
        && a.added_idents == b.added_idents
        && actions_eq(&a.hyp_actions, &b.hyp_actions)
}

/// The deep rule comparison: everything but the display string.
fn rules_eq(produced: &Rule, stored: &Rule) -> bool {
    produced.goal == stored.goal
        && hyp_set_eq(&produced.needed_hyps, &stored.needed_hyps)
        && produced.confidence == stored.confidence
        && produced.antecedents.len() == stored.antecedents.len()
        && produced
            .antecedents
            .iter()
            .zip(&stored.antecedents)
            .all(|(x, y)| antecedents_eq(x, y))
}

/// A conjunction's conjuncts (or the predicate itself), duplicates
/// and `⊤` dropped — what an auto-rewrite records as the inferred
/// hypotheses of a rewritten hypothesis.
fn split_conjuncts(pred: &rossi::formula::Predicate) -> Vec<rossi::formula::Predicate> {
    use rossi::formula::PredicateKind;
    use rossi::formula::tag::{AssocPredOp, LiteralPredOp};
    let conjuncts: Vec<rossi::formula::Predicate> = match pred.kind() {
        PredicateKind::Associative {
            op: AssocPredOp::LAnd,
            children,
        } => children.clone(),
        _ => vec![pred.clone()],
    };
    let mut out: Vec<rossi::formula::Predicate> = Vec::with_capacity(conjuncts.len());
    for conjunct in conjuncts {
        if matches!(
            conjunct.kind(),
            PredicateKind::Literal(LiteralPredOp::BTrue)
        ) || out.contains(&conjunct)
        {
            continue;
        }
        out.push(conjunct);
    }
    out
}

/// An auto-rewrite hypothesis action whose output round-trips to its
/// input: the rewritten hypothesis, split into conjuncts, is the
/// hypothesis itself. These are recorded when only an in-memory
/// normalization (associative flattening of a shape the printer hides,
/// negative-literal folding) changed the formula — the serialized
/// proof cannot expose what changed, so no replay from strings can
/// reproduce the action.
fn invisible_action(action: &HypAction) -> bool {
    let HypAction::Rewrite {
        hyps,
        added_idents,
        inferred,
        disappearing,
    } = action
    else {
        return false;
    };
    added_idents.is_empty()
        && hyps.len() == 1
        && disappearing == hyps
        && hyp_set_eq(inferred, &split_conjuncts(&hyps[0]))
}

/// The stored rule with its serialization-invisible components
/// removed: a goal antecedent equal to the sequent goal, and
/// hypothesis rewrites that round-trip to their input.
fn strip_invisible(stored: &Rule, seq_goal: &rossi::formula::Predicate) -> Rule {
    let mut rule = stored.clone();
    if rule.antecedents.len() == 1
        && rule.goal.as_ref() == Some(seq_goal)
        && rule.antecedents[0].goal.as_ref() == Some(seq_goal)
    {
        rule.goal = None;
        rule.antecedents[0].goal = None;
    }
    for antecedent in &mut rule.antecedents {
        antecedent
            .hyp_actions
            .retain(|action| !invisible_action(action));
    }
    rule
}

/// Nothing visible left: the stored rule recorded only invisible
/// normalization, so a faithful replay finds no rewrite at all.
fn strips_to_nothing(stripped: &Rule) -> bool {
    stripped.goal.is_none()
        && stripped.antecedents.len() == 1
        && stripped.antecedents[0].goal.is_none()
        && stripped.antecedents[0].added_hyps.is_empty()
        && stripped.antecedents[0].hyp_actions.is_empty()
}

/// What differs, for the report.
fn diff_summary(produced: &Rule, stored: &Rule) -> String {
    if produced.goal != stored.goal {
        return "goal".into();
    }
    if !hyp_set_eq(&produced.needed_hyps, &stored.needed_hyps) {
        return "needed_hyps".into();
    }
    if produced.confidence != stored.confidence {
        return "confidence".into();
    }
    if produced.antecedents.len() != stored.antecedents.len() {
        return format!(
            "antecedent arity {} vs {}",
            produced.antecedents.len(),
            stored.antecedents.len()
        );
    }
    for (index, (x, y)) in produced
        .antecedents
        .iter()
        .zip(&stored.antecedents)
        .enumerate()
    {
        if !antecedents_eq(x, y) {
            let what = if x.goal != y.goal {
                "goal"
            } else if !hyp_set_eq(&x.added_hyps, &y.added_hyps) {
                "added_hyps"
            } else if x.added_idents != y.added_idents {
                "added_idents"
            } else if !actions_eq(&x.hyp_actions, &y.hyp_actions) {
                "hyp_actions"
            } else {
                "unselected_added"
            };
            return format!("antecedent {index} {what}");
        }
    }
    "equal".into()
}

/// Walks one proof: replay-and-compare every implemented node, then
/// descend along the *stored* rule's application so one node's diff
/// cannot cascade into its children.
fn walk_proof(
    root: rossi_prove::ProverSequent,
    skel: &Skeleton,
    manual: bool,
    counts: &mut Counts,
    per_reasoner: &mut BTreeMap<String, ReasonerCounts>,
    problems: &mut Vec<String>,
    context: &str,
) {
    let mut stack: Vec<(rossi_prove::ProverSequent, &Skeleton, bool)> = vec![(root, skel, false)];
    while let Some((seq, node, drifted)) = stack.pop() {
        let Some(stored) = &node.rule else {
            continue;
        };
        counts.nodes += 1;
        let desc = &stored.rule.reasoner;
        let reasoner_row = per_reasoner.entry(desc.id().to_string()).or_default();
        reasoner_row.nodes += 1;

        if !desc.is_trusted() {
            counts.untrusted += 1;
        } else if desc.registration() == Some(Registration::Oracle) {
            counts.oracle += 1;
        } else {
            match RegistryProvider.implementation(desc) {
                None => counts.unimplemented += 1,
                Some(imp) => match imp.replay(&seq, stored, &ReplayHints::default()) {
                    Err(err) if drifted => {
                        counts.stale_selection += 1;
                        let _ = err;
                    }
                    Err(err) if manual => {
                        counts.kept_drift += 1;
                        let _ = err;
                    }
                    Err(err) => {
                        if strips_to_nothing(&strip_invisible(&stored.rule, seq.goal())) {
                            counts.invisible_rewrite += 1;
                        } else {
                            counts.error += 1;
                            problems.push(format!("{context}: {} failed: {err}", desc.id()));
                        }
                    }
                    Ok(produced) => {
                        if rules_eq(&produced, &stored.rule) {
                            counts.replayed_eq += 1;
                            reasoner_row.replayed_eq += 1;
                        } else if rules_eq(&produced, &strip_invisible(&stored.rule, seq.goal())) {
                            counts.invisible_rewrite += 1;
                        } else if drifted {
                            // The recorded rule speaks about a selection
                            // state this obligation no longer produces;
                            // the comparison is meaningless below the
                            // drift point.
                            counts.stale_selection += 1;
                        } else if manual {
                            // A manually kept proof — after a
                            // recalculate refresh, exactly the proofs
                            // the auto-prover could not re-derive. They
                            // may predate their regenerated obligation
                            // (extra typing hypotheses, another
                            // hypothesis order), so a content diff is
                            // recording-time drift, not a replay bug.
                            counts.kept_drift += 1;
                        } else {
                            counts.replayed_diff += 1;
                            reasoner_row.replayed_diff += 1;
                            problems.push(format!(
                                "{context}: {} differs: {}",
                                desc.id(),
                                diff_summary(&produced, &stored.rule)
                            ));
                        }
                    }
                },
            }
        }

        // Selection drift: a stored hypothesis action referencing a
        // hypothesis this sequent does not have proves the proof was
        // recorded against a differently shaped obligation. Such
        // actions apply on the intersection and the dependency
        // check ignores selection entirely, so the proof stays
        // reusable — but every selection-sensitive rule below compares
        // against a different sequent than the recorded one.
        let drift_here = stored.rule.antecedents.iter().any(|antecedent| {
            antecedent.hyp_actions.iter().any(|action| {
                matches!(
                    action,
                    HypAction::Select(_)
                        | HypAction::Deselect(_)
                        | HypAction::Hide(_)
                        | HypAction::Show(_)
                ) && action
                    .hyps()
                    .iter()
                    .any(|hyp| !seq.contains_hypothesis(hyp))
            })
        });

        match stored.rule.apply(&seq) {
            Some(children) if children.len() == node.children.len() => {
                for (child_seq, child_skel) in children.into_iter().zip(&node.children) {
                    stack.push((child_seq, child_skel, drifted || drift_here));
                }
            }
            // The stored rule no longer applies here (a broken proof):
            // nothing below has a well-defined sequent.
            _ => counts.apply_stop += 1,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn check_component(
    project: &PoProject,
    component: &str,
    path: &str,
    bpr: Option<&[u8]>,
    bps: Option<&str>,
    counts: &mut Counts,
    per_reasoner: &mut BTreeMap<String, ReasonerCounts>,
    problems: &mut Vec<String>,
) {
    let Some(po) = project.file(path) else {
        return;
    };
    // The recalculate refresh marks kept proofs manual on the STATUS
    // row; the stored proof's own flag keeps its historical value.
    let manual_rows: std::collections::HashSet<String> = bps
        .and_then(|text| read_bps(text.as_bytes()).ok())
        .map(|rows| {
            rows.into_iter()
                .filter(|row| row.manual)
                .map(|row| row.name)
                .collect()
        })
        .unwrap_or_default();
    let proofs: BTreeMap<String, ProofEntry> = match bpr {
        Some(bytes) => match bpr::read_bpr(bytes, |_| Keep::Full) {
            Ok(entries) => entries
                .into_iter()
                .map(|entry| (entry.name.clone(), entry))
                .collect(),
            // Pre-versioning or unreadable proof files carry nothing to
            // replay; the reuse gate accounts for them.
            Err(_) => return,
        },
        None => return,
    };

    for entry in po.sequents() {
        let name = &entry.name;
        let Some(proof) = proofs.get(name) else {
            continue;
        };
        let ProofBody::Loaded(loaded) = &proof.body else {
            continue;
        };
        let Some(skel) = &loaded.skeleton else {
            continue;
        };
        let Ok(seq) = project.load(path, name) else {
            continue;
        };
        // A broken proof was recorded against an obligation that no
        // longer exists; the stored path cannot reconstruct its
        // sequents (wildcard-goal prefixes slip through unchecked), so
        // there is nothing meaningful to replay against.
        if rossi_prove::status::compute_status(&seq, proof).broken {
            continue;
        }
        walk_proof(
            seq,
            skel,
            proof.manual || manual_rows.contains(name),
            counts,
            per_reasoner,
            problems,
            &format!("{component} {name}"),
        );
    }
}

fn check_model(
    path: &Path,
    counts: &mut Counts,
    per_reasoner: &mut BTreeMap<String, ReasonerCounts>,
    problems: &mut Vec<String>,
) -> Result<(), String> {
    // Legacy-vintage components have nothing to replay against, and a
    // `.bpo` that fails to parse never reaches the walkable set — both
    // are accounted by the reuse gate, so this one just skips them.
    let mut po = common::PoArchive::load(path)?;
    for stem in std::mem::take(&mut po.checked) {
        let (dir, file) = common::stem_parts(&stem);
        let bpr = po.entry(&format!("{stem}.bpr"));
        let bps = po.entry(&format!("{stem}.bps"));
        let bps = bps.as_deref().map(String::from_utf8_lossy);
        check_component(
            &po.projects[dir],
            &stem,
            &format!("{file}.bpo"),
            bpr.as_deref(),
            bps.as_deref(),
            counts,
            per_reasoner,
            problems,
        );
    }
    Ok(())
}

/// Per-model `replayed_eq` from a committed corpus baseline, when one
/// exists (`replay_results.tsv`, columns `model` and `replayed_eq`).
fn load_baseline(corpus: &Path) -> BTreeMap<String, usize> {
    let Ok(text) = std::fs::read_to_string(corpus.join("replay_results.tsv")) else {
        return BTreeMap::new();
    };
    let mut lines = text.lines();
    let Some(header) = lines.next() else {
        return BTreeMap::new();
    };
    let cols: Vec<&str> = header.split('\t').collect();
    let Some(eq_col) = cols.iter().position(|c| *c == "replayed_eq") else {
        return BTreeMap::new();
    };
    lines
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            Some((
                fields.first()?.to_string(),
                fields.get(eq_col)?.parse().ok()?,
            ))
        })
        .collect()
}

#[test]
#[ignore = "needs a models corpus; run with --ignored"]
fn replay_reproduces_recorded_rules() {
    let Some(corpus) = corpus_dir() else {
        eprintln!("corpus not found; set EVENTB_CORPUS_DIR");
        return;
    };
    let flags = load_flags(&corpus.join("model_flags.tsv")).unwrap_or_default();
    let baseline = load_baseline(&corpus);
    let zips = collect_zips(&corpus).expect("corpus listing");

    // Archives are independent: check them in parallel, then fold the
    // results in archive order so the reports stay stable.
    type Checked = (Counts, BTreeMap<String, ReasonerCounts>, Vec<String>);
    let checked: Vec<Result<Checked, String>> = rossi_prove::thread_pool().install(|| {
        zips.par_iter()
            .map(|path| {
                let model = path.file_stem().unwrap_or_default().to_string_lossy();
                if flagged_unsupported(&flags, &model) {
                    return Err("flagged".to_string());
                }
                let mut counts = Counts::default();
                let mut per_reasoner = BTreeMap::new();
                let mut problems = Vec::new();
                check_model(path, &mut counts, &mut per_reasoner, &mut problems)?;
                Ok((counts, per_reasoner, problems))
            })
            .collect()
    });

    let mut rows = Vec::new();
    let mut failures = Vec::new();
    let mut total = Counts::default();
    let mut per_reasoner: BTreeMap<String, ReasonerCounts> = BTreeMap::new();
    for (path, checked) in zips.iter().zip(checked) {
        let model = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let (counts, model_reasoners, mut problems) = match checked {
            Ok(checked) => checked,
            Err(err) => {
                rows.push(report_row(&model, &Counts::default(), "skip", &err));
                continue;
            }
        };
        for (id, model_counts) in model_reasoners {
            per_reasoner.entry(id).or_default().add(&model_counts);
        }
        if let Some(base) = baseline.get(&model)
            && counts.replayed_eq < *base
        {
            problems.push(format!(
                "replayed_eq regressed: {} < baseline {base}",
                counts.replayed_eq
            ));
        }
        let verdict = if counts.replayed_diff > 0 || counts.error > 0 || !problems.is_empty() {
            match prove_known_divergence(&corpus, &model) {
                Some(reason) => ("known", reason),
                None => {
                    failures.push(format!(
                        "{model}: {} problems, first: {}",
                        problems.len(),
                        problems.first().map(String::as_str).unwrap_or("?")
                    ));
                    ("diverge", problems.first().cloned().unwrap_or_default())
                }
            }
        } else {
            ("match", String::new())
        };
        rows.push(report_row(&model, &counts, verdict.0, &verdict.1));
        total.add(&counts);
    }

    let out = workspace_root().join("target/rossi-build-proof-replay-corpus.tsv");
    write_report(
        &out,
        &[
            "model",
            "nodes",
            "replayed_eq",
            "replayed_diff",
            "oracle",
            "unimplemented",
            "untrusted",
            "error",
            "apply_stop",
            "stale_selection",
            "kept_drift",
            "invisible_rewrite",
            "verdict",
            "notes",
        ],
        &rows,
    );
    let coverage = workspace_root().join("target/rossi-build-proof-replay-reasoners.tsv");
    let reasoner_rows: Vec<Vec<String>> = per_reasoner
        .iter()
        .map(|(id, counts)| {
            vec![
                id.clone(),
                counts.nodes.to_string(),
                counts.replayed_eq.to_string(),
                counts.replayed_diff.to_string(),
            ]
        })
        .collect();
    write_report(
        &coverage,
        &["reasoner", "nodes", "replayed_eq", "replayed_diff"],
        &reasoner_rows,
    );
    println!(
        "replay: {} nodes — {} replayed_eq, {} replayed_diff, {} oracle, {} unimplemented, \
         {} untrusted, {} error, {} apply_stop, {} stale_selection, {} kept_drift, \
         {} invisible_rewrite; reports: {} and {}",
        total.nodes,
        total.replayed_eq,
        total.replayed_diff,
        total.oracle,
        total.unimplemented,
        total.untrusted,
        total.error,
        total.apply_stop,
        total.stale_selection,
        total.kept_drift,
        total.invisible_rewrite,
        out.display(),
        coverage.display(),
    );
    assert!(
        failures.is_empty(),
        "{} models diverged:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn report_row(model: &str, counts: &Counts, verdict: &str, notes: &str) -> Vec<String> {
    vec![
        model.to_string(),
        counts.nodes.to_string(),
        counts.replayed_eq.to_string(),
        counts.replayed_diff.to_string(),
        counts.oracle.to_string(),
        counts.unimplemented.to_string(),
        counts.untrusted.to_string(),
        counts.error.to_string(),
        counts.apply_stop.to_string(),
        counts.stale_selection.to_string(),
        counts.kept_drift.to_string(),
        counts.invisible_rewrite.to_string(),
        verdict.to_string(),
        common::sanitize(notes),
    ]
}
