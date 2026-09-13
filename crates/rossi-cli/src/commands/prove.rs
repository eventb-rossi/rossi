//! `rossi prove` — check the stored proofs of an Event-B project
//! against its proof obligations.
//!
//! For every obligation in the input's `.bpo` files, the stored proof
//! in the sibling `.bpr` is checked by the dependency-based reuse
//! rule (the status-update decision): a proof that no longer applies to
//! its regenerated obligation reports as broken. The recorded `.bps`
//! statuses are not consulted — this command recomputes the verdicts.
//!
//! Any broken or uncheckable proof makes the exit code nonzero;
//! pending and unattempted obligations do not: open proofs are work
//! in progress rather than errors.
//!
//! With `--replay`, every checkable proof whose reasoners are all
//! implemented is additionally re-derived: each reasoner is re-run on
//! its recorded input and the reconstructed tree must complete
//! (the replay mode). Proofs using reasoners without a Rust
//! implementation are skipped, not failed — coverage grows with the
//! reasoner batches — while a replay that fails on an implemented
//! proof is an error.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;
use rayon::prelude::*;

use rossi_prove::bpr::{Keep, ProofBody, ProofEntry, visit_bpr};
use rossi_prove::confidence::Bucket;
use rossi_prove::po_loader::PoProject;
use rossi_prove::{ProofTreeNode, ReasonerProvider, RegistryProvider, Skeleton};

use super::proofs::ProofStatus;

#[derive(Args)]
pub struct ProveArgs {
    /// Input to check: an Event-B `.zip` archive or a project directory
    /// containing `.bpo` / `.bpr` files.
    pub input: PathBuf,
    /// Also list every checked obligation, not only the problematic
    /// ones.
    #[arg(short, long)]
    pub verbose: bool,
    /// Re-run each proof's reasoners on their recorded inputs and
    /// require the reconstructed tree to complete (proofs using
    /// unimplemented reasoners are skipped).
    #[arg(long)]
    pub replay: bool,
}

#[derive(Default)]
struct Summary {
    discharged: usize,
    reviewed: usize,
    pending: usize,
    unattempted: usize,
    broken: usize,
    unsupported: usize,
    errors: usize,
    replayed: usize,
    replay_skipped: usize,
    replay_failed: usize,
}

impl Summary {
    fn add(&mut self, other: &Summary) {
        self.discharged += other.discharged;
        self.reviewed += other.reviewed;
        self.pending += other.pending;
        self.unattempted += other.unattempted;
        self.broken += other.broken;
        self.unsupported += other.unsupported;
        self.errors += other.errors;
        self.replayed += other.replayed;
        self.replay_skipped += other.replay_skipped;
        self.replay_failed += other.replay_failed;
    }
}

pub fn run(args: ProveArgs) -> ExitCode {
    match prove(&args.input, args.verbose, args.replay) {
        Ok(summary) => {
            let total = summary.discharged
                + summary.reviewed
                + summary.pending
                + summary.unattempted
                + summary.broken
                + summary.unsupported
                + summary.errors;
            println!(
                "Proofs: {total} obligation(s) — {} discharged, {} reviewed, {} pending, \
                 {} unattempted, {} broken, {} unsupported, {} error(s)",
                summary.discharged,
                summary.reviewed,
                summary.pending,
                summary.unattempted,
                summary.broken,
                summary.unsupported,
                summary.errors,
            );
            if args.replay {
                println!(
                    "Replay: {} replayed, {} skipped (unimplemented reasoners), {} failed",
                    summary.replayed, summary.replay_skipped, summary.replay_failed,
                );
            }
            if summary.broken + summary.unsupported + summary.errors + summary.replay_failed > 0 {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(e) => {
            eprintln!("rossi prove: {e}");
            ExitCode::from(1)
        }
    }
}

/// The first stored reasoner in the skeleton without an implementation,
/// making the proof unreplayable for now.
fn missing_reasoner(skel: &Skeleton) -> Option<String> {
    let mut stack = vec![skel];
    while let Some(node) = stack.pop() {
        if let Some(stored) = &node.rule
            && RegistryProvider
                .implementation(&stored.rule.reasoner)
                .is_none()
        {
            return Some(stored.rule.reasoner.id().to_string());
        }
        stack.extend(node.children.iter());
    }
    None
}

fn prove(input: &Path, verbose: bool, replay: bool) -> Result<Summary, Box<dyn std::error::Error>> {
    let files = super::proofs::collect_proof_files(input)?;
    let (bpos, bprs) = (&files.bpo, &files.bpr);
    if bpos.is_empty() {
        return Err(format!("no .bpo files found in {}", input.display()).into());
    }
    let keep = if replay { Keep::Full } else { Keep::Deps };

    let pool = rossi_prove::thread_pool();
    let projects = super::proofs::load_projects(bpos)?;

    // Components are independent once the projects exist: check them
    // in parallel, reporting in stem order.
    let checked: Vec<(Vec<String>, Summary)> = pool.install(|| {
        bpos.par_iter()
            .map(|(stem, _)| {
                let (dir, file) = super::proofs::split_stem(stem);
                let bpr = bprs.get(stem).map(Vec::as_slice);
                check_component(
                    stem,
                    &projects[dir],
                    &format!("{file}.bpo"),
                    bpr,
                    keep,
                    replay,
                    verbose,
                )
            })
            .collect()
    });
    let mut summary = Summary::default();
    for (lines, counts) in &checked {
        for line in lines {
            println!("{line}");
        }
        summary.add(counts);
    }
    Ok(summary)
}

/// Checks one component's obligations against its proof file: the
/// lines to report and the verdict counts.
fn check_component(
    stem: &str,
    project: &PoProject,
    path: &str,
    bpr: Option<&[u8]>,
    keep: Keep,
    replay: bool,
    verbose: bool,
) -> (Vec<String>, Summary) {
    let mut lines = Vec::new();
    let mut summary = Summary::default();
    let Some(po) = project.file(path) else {
        return (lines, summary);
    };
    // Proofs are checked as they stream off the file, so one proof's
    // tree is in memory at a time; a later proof of the same name
    // replaces an earlier one.
    let mut verdicts: BTreeMap<String, ProofVerdict> = BTreeMap::new();
    if let Some(bytes) = bpr
        && let Err(err) = visit_bpr(
            bytes,
            |_| keep,
            |proof| {
                if po.sequent(&proof.name).is_some() {
                    verdicts.insert(
                        proof.name.clone(),
                        check_proof(project, path, &proof, replay),
                    );
                }
            },
        )
    {
        lines.push(format!("{stem}: unreadable proof file: {err}"));
        summary.errors += po.sequents().count();
        return (lines, summary);
    }
    for entry in po.sequents() {
        let name = &entry.name;
        let verdict = verdicts.get(name);
        // An obligation with no stored proof was never attempted.
        let status = verdict.map_or(ProofStatus::Checked(Bucket::Unattempted), |verdict| {
            verdict.status
        });
        let replay = verdict.and_then(|verdict| verdict.replay.as_ref());
        let note = match replay {
            None => String::new(),
            Some(Replay::Skipped(id)) => {
                summary.replay_skipped += 1;
                format!(" (replay skipped: {id})")
            }
            Some(Replay::Replayed) => {
                summary.replayed += 1;
                " (replayed)".into()
            }
            Some(Replay::Failed) => {
                summary.replay_failed += 1;
                " (replay FAILED)".into()
            }
        };
        let replay_failed = matches!(replay, Some(Replay::Failed));
        match status {
            ProofStatus::Checked(Bucket::Discharged) => summary.discharged += 1,
            ProofStatus::Checked(Bucket::Reviewed) => summary.reviewed += 1,
            ProofStatus::Checked(Bucket::Pending) => summary.pending += 1,
            ProofStatus::Checked(Bucket::Unattempted) => summary.unattempted += 1,
            ProofStatus::Broken => summary.broken += 1,
            ProofStatus::Unsupported => summary.unsupported += 1,
            ProofStatus::Error => summary.errors += 1,
        }
        let problem = matches!(
            status,
            ProofStatus::Broken | ProofStatus::Unsupported | ProofStatus::Error
        );
        if verbose || problem || replay_failed {
            lines.push(format!("{stem} {name}: {}{note}", status.label()));
        }
    }
    (lines, summary)
}

/// One proof's verdict against its obligation.
struct ProofVerdict {
    status: ProofStatus,
    replay: Option<Replay>,
}

/// The replay outcome of a proof whose status allowed one.
enum Replay {
    /// The named reasoner is not implemented.
    Skipped(String),
    Replayed,
    Failed,
}

fn check_proof(project: &PoProject, path: &str, proof: &ProofEntry, replay: bool) -> ProofVerdict {
    let classified = super::proofs::classify(project, path, proof);
    let mut outcome = None;
    // Only a proof that still applies to its obligation is worth
    // re-deriving, and it reuses the sequent the verdict was
    // computed against.
    if replay
        && let ProofStatus::Checked(_) = classified.status
        && let Some(seq) = classified.sequent
        && let ProofBody::Loaded(stored) = &proof.body
    {
        let skel = stored.skeleton.as_ref().expect("full parse");
        outcome = Some(match missing_reasoner(skel) {
            Some(id) => Replay::Skipped(id),
            None => {
                let mut node = ProofTreeNode::open(seq);
                if rossi_prove::replay(&mut node, skel, &RegistryProvider) {
                    Replay::Replayed
                } else {
                    Replay::Failed
                }
            }
        });
    }
    ProofVerdict {
        status: classified.status,
        replay: outcome,
    }
}
