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

use clap::{Args, ValueEnum};
use rayon::prelude::*;
use serde::Serialize;

use rossi_prove::bpr::{Keep, ProofBody, ProofEntry, visit_bpr};
use rossi_prove::confidence::Bucket;
use rossi_prove::po_loader::PoProject;
use rossi_prove::{ProofTreeNode, ReasonerProvider, RegistryProvider, Skeleton};

use super::proofs::ProofStatus;
use super::report::{Report, finish_structured_output};

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
    /// Report format: the human lines, or one JSON document on standard
    /// output with the summary and every obligation's verdict.
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: ProveFormat,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum ProveFormat {
    /// Human-readable text output
    Text,
    /// JSON output
    Json,
}

#[derive(Default, Serialize)]
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

/// One obligation's verdict, as reported.
#[derive(Serialize)]
struct ObligationRow {
    /// The component stem, prefixed by its archive directory when the
    /// archive nests projects.
    component: String,
    name: String,
    status: ProofStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    replay: Option<Replay>,
}

impl ObligationRow {
    fn replay_note(&self) -> String {
        match &self.replay {
            None => String::new(),
            Some(Replay::Skipped { reasoner }) => format!(" (replay skipped: {reasoner})"),
            Some(Replay::Replayed) => " (replayed)".into(),
            Some(Replay::Failed) => " (replay FAILED)".into(),
        }
    }

    /// Whether the row is reported even without `--verbose`.
    fn is_problem(&self) -> bool {
        matches!(
            self.status,
            ProofStatus::Broken | ProofStatus::Unsupported | ProofStatus::Error
        ) || matches!(self.replay, Some(Replay::Failed))
    }
}

/// One component's obligations checked: file-level messages, one row
/// per obligation, and the counts.
struct ComponentReport {
    messages: Vec<String>,
    obligations: Vec<ObligationRow>,
    summary: Summary,
}

/// The whole input checked.
struct ProveReport {
    summary: Summary,
    components: Vec<ComponentReport>,
}

/// The JSON document of a run.
#[derive(Serialize)]
struct ProveDocument<'a> {
    input: String,
    summary: &'a Summary,
    obligations: Vec<&'a ObligationRow>,
    /// File-level problems, such as an unreadable proof file.
    messages: Vec<&'a str>,
}

pub fn run(args: ProveArgs) -> ExitCode {
    let report = match prove(&args.input, args.replay) {
        Ok(report) => report,
        Err(e) => {
            eprintln!("rossi prove: {e}");
            return ExitCode::from(1);
        }
    };
    let summary = &report.summary;
    let exit = if summary.broken + summary.unsupported + summary.errors + summary.replay_failed > 0
    {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    };
    match args.format {
        ProveFormat::Text => {
            print_text(&report, args.verbose, args.replay);
            exit
        }
        ProveFormat::Json => {
            let document = ProveDocument {
                input: args.input.display().to_string(),
                summary,
                obligations: report
                    .components
                    .iter()
                    .flat_map(|c| c.obligations.iter())
                    .collect(),
                messages: report
                    .components
                    .iter()
                    .flat_map(|c| c.messages.iter().map(String::as_str))
                    .collect(),
            };
            let written = Report::Console.structured(|out| {
                serde_json::to_writer_pretty(&mut *out, &document)?;
                writeln!(out)
            });
            finish_structured_output("prove", written, exit, &mut std::io::stderr())
        }
    }
}

fn print_text(report: &ProveReport, verbose: bool, replay: bool) {
    for component in &report.components {
        for message in &component.messages {
            println!("{message}");
        }
        for row in &component.obligations {
            if verbose || row.is_problem() {
                println!(
                    "{} {}: {}{}",
                    row.component,
                    row.name,
                    row.status.label(),
                    row.replay_note()
                );
            }
        }
    }
    let summary = &report.summary;
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
    if replay {
        println!(
            "Replay: {} replayed, {} skipped (unimplemented reasoners), {} failed",
            summary.replayed, summary.replay_skipped, summary.replay_failed,
        );
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

fn prove(input: &Path, replay: bool) -> Result<ProveReport, Box<dyn std::error::Error>> {
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
    let components: Vec<ComponentReport> = pool.install(|| {
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
                )
            })
            .collect()
    });
    let mut summary = Summary::default();
    for component in &components {
        summary.add(&component.summary);
    }
    Ok(ProveReport {
        summary,
        components,
    })
}

/// Checks one component's obligations against its proof file.
fn check_component(
    stem: &str,
    project: &PoProject,
    path: &str,
    bpr: Option<&[u8]>,
    keep: Keep,
    replay: bool,
) -> ComponentReport {
    let mut report = ComponentReport {
        messages: Vec::new(),
        obligations: Vec::new(),
        summary: Summary::default(),
    };
    let Some(po) = project.file(path) else {
        return report;
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
        report
            .messages
            .push(format!("{stem}: unreadable proof file: {err}"));
        report.summary.errors += po.sequents().count();
        return report;
    }
    let summary = &mut report.summary;
    for entry in po.sequents() {
        let name = &entry.name;
        let verdict = verdicts.get(name);
        // An obligation with no stored proof was never attempted.
        let status = verdict.map_or(ProofStatus::Checked(Bucket::Unattempted), |verdict| {
            verdict.status
        });
        let replay = verdict.and_then(|verdict| verdict.replay.clone());
        match &replay {
            None => {}
            Some(Replay::Skipped { .. }) => summary.replay_skipped += 1,
            Some(Replay::Replayed) => summary.replayed += 1,
            Some(Replay::Failed) => summary.replay_failed += 1,
        }
        match status {
            ProofStatus::Checked(Bucket::Discharged) => summary.discharged += 1,
            ProofStatus::Checked(Bucket::Reviewed) => summary.reviewed += 1,
            ProofStatus::Checked(Bucket::Pending) => summary.pending += 1,
            ProofStatus::Checked(Bucket::Unattempted) => summary.unattempted += 1,
            ProofStatus::Broken => summary.broken += 1,
            ProofStatus::Unsupported => summary.unsupported += 1,
            ProofStatus::Error => summary.errors += 1,
        }
        report.obligations.push(ObligationRow {
            component: stem.to_string(),
            name: name.clone(),
            status,
            replay,
        });
    }
    report
}

/// One proof's verdict against its obligation.
struct ProofVerdict {
    status: ProofStatus,
    replay: Option<Replay>,
}

/// The replay outcome of a proof whose status allowed one.
#[derive(Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum Replay {
    /// The named reasoner is not implemented.
    Skipped {
        reasoner: String,
    },
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
            Some(reasoner) => Replay::Skipped { reasoner },
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
