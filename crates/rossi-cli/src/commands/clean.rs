//! `rossi clean` — maintenance on a project's stored proofs.
//!
//! Two independent jobs, both decisions about whole `prProof` entries
//! of a `.bpr` file, and both otherwise byte-preserving:
//!
//! * `--purge` drops the proofs whose obligation no longer exists,
//!   which is what Rodin's *Proof Purger* does. Proof files accumulate
//!   without bound because Rodin never deletes a proof on its own —
//!   it is user data — so a long-lived model ends up carrying more
//!   orphans than live proofs.
//! * the reset selectors empty a proof in place, which is what the
//!   *POCleaner* plug-in does, for the case that plug-in exists to
//!   serve: a stored proof Rodin can no longer open. The obligation
//!   goes back to unattempted and can be proved again.
//!
//! The two are complementary rather than alternatives — the purger
//! will not touch an emptied entry, because its obligation is still
//! live, and emptying does nothing for a proof that has no obligation
//! left at all.
//!
//! One deliberate deviation from Rodin: a `.bpr` with no sibling
//! `.bpo` is skipped by `--purge` rather than emptied wholesale.
//! Rodin counts every one of its proofs as unused, but Rodin also asks
//! for confirmation through a checkbox tree first; here the same rule
//! would silently discard every proof of a project whose obligations
//! merely have not been generated yet.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;
use rayon::prelude::*;
use rossi_build::pog::reconcile::reset_status_rows;
use rossi_prove::bpr::{BprError, Keep, visit_bpr};
use rossi_prove::bpr_rewrite::{ProofAction, RewriteStats, rewrite_bpr};
use rossi_prove::po_loader::{PoFile, PoProject};

use super::eventb_io::CmdResult;
use super::proofs::{FileMap, ProofStatus, split_stem};

#[derive(Args)]
pub struct CleanArgs {
    /// Input to clean: an Event-B `.zip` archive or a project directory
    /// containing `.bpr` / `.bpo` files.
    pub input: PathBuf,
    /// Drop the proofs whose obligation no longer exists.
    #[arg(long)]
    pub purge: bool,
    /// Empty the proofs of obligations matching a name pattern, where
    /// `*` stands for any run of characters (e.g. `'evt/inv*'`).
    #[arg(long, value_name = "GLOB")]
    pub reset: Vec<String>,
    /// Empty every proof that no longer applies to its obligation.
    #[arg(long)]
    pub broken: bool,
    /// Empty every proof of the named component (e.g. `M1`).
    #[arg(long, value_name = "NAME")]
    pub component: Vec<String>,
    /// Empty every stored proof.
    #[arg(long)]
    pub all: bool,
    /// Report what would change without writing anything; exit
    /// nonzero if anything would.
    #[arg(long)]
    pub check: bool,
    /// Write the cleaned project here instead of over the input.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Also list the components that come out unchanged.
    #[arg(short, long)]
    pub verbose: bool,
}

pub fn run(args: CleanArgs) -> ExitCode {
    if !args.purge
        && !args.all
        && !args.broken
        && args.reset.is_empty()
        && args.component.is_empty()
    {
        eprintln!(
            "rossi clean: nothing to do — pass --purge, or a reset selector \
             (--broken, --all, --reset, --component)"
        );
        return ExitCode::from(2);
    }
    match clean(&args) {
        Ok(report) => {
            for line in &report.lines {
                println!("{line}");
            }
            println!(
                "Clean: {} component(s) — {} purged, {} reset{}",
                report.components,
                report.purged,
                report.reset,
                if args.check { ", nothing written" } else { "" },
            );
            if report.errors > 0 || (args.check && report.changed()) {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(e) => {
            eprintln!("rossi clean: {e}");
            ExitCode::from(1)
        }
    }
}

#[derive(Default)]
struct Report {
    lines: Vec<String>,
    components: usize,
    purged: usize,
    reset: usize,
    errors: usize,
}

impl Report {
    fn changed(&self) -> bool {
        self.purged > 0 || self.reset > 0
    }
}

/// What one component's proof file should become: `None` deletes it.
type Rewritten = BTreeMap<String, Option<Vec<u8>>>;

fn clean(args: &CleanArgs) -> CmdResult<Report> {
    let files = super::proofs::collect_proof_files(&args.input)?;
    if files.bpr.is_empty() {
        return Err(format!("no .bpr files found in {}", args.input.display()).into());
    }
    // The obligations answer both jobs' questions — which proofs are
    // orphaned, and which no longer apply — so they are read once,
    // through the parser rather than an attribute scan. That the read
    // validates matters: a damaged `.bpo` has to fail the run, not
    // read as "every proof here is an orphan".
    let projects = super::proofs::load_projects(&files.bpo)?;

    // Components are independent: clean them in parallel and report
    // in stem order, exactly as `rossi prove` checks them.
    let outcomes: Vec<Outcome> = rossi_prove::thread_pool().install(|| {
        files
            .bpr
            .par_iter()
            .map(|(stem, bytes)| component(args, stem, bytes, &projects, &files.bps))
            .collect()
    });

    let mut report = Report::default();
    // `.bpr` and `.bps` files to write, keyed like the inputs;
    // anything absent here is left exactly as it was.
    let mut bpr_out = Rewritten::new();
    let mut bps_out = FileMap::new();
    for (stem, outcome) in files.bpr.keys().zip(outcomes) {
        report.components += 1;
        report.lines.extend(outcome.lines);
        report.purged += outcome.purged;
        report.reset += outcome.reset;
        report.errors += outcome.errors;
        if let Some(bpr) = outcome.bpr {
            bpr_out.insert(stem.clone(), bpr);
        }
        if let Some(bps) = outcome.bps {
            bps_out.insert(stem.clone(), bps);
        }
    }

    // With `--output` the copy is the deliverable, so it is written
    // even when no entry changed; in place there would be nothing to
    // do.
    if !args.check && (args.output.is_some() || !bpr_out.is_empty() || !bps_out.is_empty()) {
        write_out(args, &bpr_out, &bps_out)?;
    }
    Ok(report)
}

/// What cleaning one component produced.
#[derive(Default)]
struct Outcome {
    lines: Vec<String>,
    purged: usize,
    reset: usize,
    errors: usize,
    /// The rewritten proof file, `Some(None)` to delete it. `None`
    /// leaves the input alone — nothing changed, or `--check` asked
    /// for nothing to be written.
    bpr: Option<Option<Vec<u8>>>,
    /// The rewritten status file.
    bps: Option<Vec<u8>>,
}

/// Cleans one component's proof file.
fn component(
    args: &CleanArgs,
    stem: &str,
    bytes: &[u8],
    projects: &BTreeMap<&str, PoProject>,
    bpss: &FileMap,
) -> Outcome {
    let mut outcome = Outcome::default();
    let (dir, name) = split_stem(stem);
    let path = format!("{name}.bpo");
    let project = projects.get(dir);
    let po = project.and_then(|project| project.file(&path));

    // Purging needs the obligations to compare against; without them
    // every proof would read as an orphan.
    if args.purge && po.is_none() {
        outcome.lines.push(format!(
            "{name}.bpr: no .bpo alongside it — cannot tell orphans \
             from live proofs, not purged"
        ));
    }
    let live = args.purge.then_some(po).flatten();

    let stale = match (args.broken, project, po) {
        (true, Some(project), Some(po)) => broken_proofs(bytes, project, &path, po),
        _ => Stale::default(),
    };
    // Say so rather than leaving the user to assume `--broken`
    // covered everything.
    if stale.unreadable > 0 {
        outcome.lines.push(format!(
            "{name}.bpr: {} proof(s) rossi cannot read, left alone",
            stale.unreadable
        ));
    }

    // Everything the decision needs that does not vary per entry.
    let select_all = args.all || args.component.iter().any(|want| want == name);
    let has_status = bpss.contains_key(stem);
    let mut reset_names = BTreeSet::new();
    let mut sink = if args.check {
        Sink::Count(0)
    } else {
        Sink::Buffer(Vec::with_capacity(bytes.len()))
    };
    let stats = rewrite_bpr(bytes, &mut sink, |proof| {
        if let Some(po) = live
            && po.sequent(proof).is_none()
        {
            return ProofAction::Drop;
        }
        let selected = select_all
            || stale.names.contains(proof)
            || args
                .reset
                .iter()
                .any(|pattern| glob_matches(pattern, proof));
        if !selected {
            return ProofAction::Keep;
        }
        // The names are only ever consumed to rewrite status rows.
        if has_status {
            reset_names.insert(proof.to_string());
        }
        ProofAction::Reset
    });
    let stats = match stats {
        Ok(stats) => stats,
        Err(err) => {
            outcome
                .lines
                .push(format!("{name}.bpr: unreadable proof file: {err}"));
            outcome.errors += 1;
            return outcome;
        }
    };

    outcome.purged += stats.dropped;
    outcome.reset += stats.reset;
    if !stats.changed() {
        if args.verbose {
            outcome
                .lines
                .push(format!("{name}.bpr: unchanged ({} proofs)", stats.kept));
        }
        return outcome;
    }

    // Rodin removes a proof file only once it holds no proofs and has
    // no status file beside it; otherwise the emptied file stays.
    let delete = stats.remaining() == 0 && !has_status;
    outcome
        .lines
        .push(describe(name, &stats, bytes.len(), sink.len(), delete));
    outcome.bpr = sink
        .into_bytes()
        .map(|rewritten| (!delete).then_some(rewritten));

    // Emptying a proof must leave its obligation unattempted, or the
    // stale status would still claim it discharged.
    if !reset_names.is_empty()
        && let (Some(bps), Some(po)) = (bpss.get(stem), po)
    {
        let stamps: HashMap<String, String> = po
            .sequents()
            .filter(|entry| reset_names.contains(&entry.name))
            .filter_map(|entry| Some((entry.name.clone(), entry.stamp.clone()?)))
            .collect();
        match reset_status_rows(&String::from_utf8_lossy(bps), &reset_names, &stamps) {
            Ok(rows) => outcome.bps = (!args.check).then(|| rows.into_bytes()),
            Err(err) => {
                outcome.lines.push(format!("{name}.bps: {err}"));
                outcome.errors += 1;
            }
        }
    }
    outcome
}

/// Where a rewritten proof file goes.
///
/// `--check` reports the sizes a run would produce but writes nothing,
/// so it counts the bytes instead of keeping them: on a project of
/// several hundred megabytes that is the difference between holding
/// one copy of it and two.
enum Sink {
    Buffer(Vec<u8>),
    Count(usize),
}

impl Sink {
    fn len(&self) -> usize {
        match self {
            Sink::Buffer(out) => out.len(),
            Sink::Count(n) => *n,
        }
    }

    /// The rewritten document, or `None` when it was only measured.
    fn into_bytes(self) -> Option<Vec<u8>> {
        match self {
            Sink::Buffer(out) => Some(out),
            Sink::Count(_) => None,
        }
    }
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Sink::Buffer(out) => out.extend_from_slice(buf),
            Sink::Count(n) => *n += buf.len(),
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// One component's report line.
fn describe(component: &str, stats: &RewriteStats, was: usize, now: usize, delete: bool) -> String {
    let mut line = format!("{component}.bpr:");
    if stats.dropped > 0 {
        line.push_str(&format!(" {} orphaned purged,", stats.dropped));
    }
    if stats.reset > 0 {
        line.push_str(&format!(" {} reset,", stats.reset));
    }
    line.push_str(&format!(" {} kept", stats.kept));
    if delete {
        line.push_str(" — file removed");
    } else if now != was {
        line.push_str(&format!(" ({} -> {})", human(was), human(now)));
    }
    line
}

fn human(bytes: usize) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Whether `name` matches `pattern`, where `*` stands for any run of
/// characters — `/` included, so `evt/*` covers a whole event and `*`
/// covers everything.
///
/// The literal runs between the stars must appear in order, the first
/// anchored at the start of the name and the last at its end.
fn glob_matches(pattern: &str, name: &str) -> bool {
    let Some((head, rest)) = pattern.split_once('*') else {
        return pattern == name;
    };
    let Some(mut name) = name.strip_prefix(head) else {
        return false;
    };
    let mut runs: Vec<&str> = rest.split('*').collect();
    let tail = runs.pop().expect("split yields at least one run");
    for run in runs {
        match name.find(run) {
            Some(at) => name = &name[at + run.len()..],
            None => return false,
        }
    }
    name.len() >= tail.len() && name.ends_with(tail)
}

/// What `--broken` found in one component's proof file.
#[derive(Default)]
struct Stale {
    /// The proofs that no longer apply to their obligation.
    names: BTreeSet<String>,
    /// The proofs this reader cannot represent, so cannot judge —
    /// reported rather than emptied, since an unreadable proof is
    /// usually only an older storage vintage Rodin still opens.
    unreadable: usize,
}

/// The proofs of one component that no longer apply to their
/// obligation — `--broken`'s selection, judged exactly as `rossi
/// prove` judges them.
fn broken_proofs(bytes: &[u8], project: &PoProject, path: &str, po: &PoFile) -> Stale {
    let mut stale = Stale::default();
    // Only a proof that still has an obligation can be judged against
    // one. The rest go unread, which on a long-lived model is most of
    // the file — and they are the purger's business, not this one's.
    let judged = |name: &str| po.sequent(name).is_some();
    // An unreadable proof file is reported by the rewrite pass; here
    // it simply contributes no names.
    let _: Result<(), BprError> = visit_bpr(
        bytes,
        |name| if judged(name) { Keep::Deps } else { Keep::Skip },
        |proof| {
            if !judged(&proof.name) {
                return;
            }
            let status = super::proofs::classify(project, path, &proof).status;
            if status.is_stale() {
                stale.names.insert(proof.name);
            } else if status == ProofStatus::Unsupported {
                stale.unreadable += 1;
            }
        },
    );
    stale
}

fn write_out(args: &CleanArgs, bpr: &Rewritten, bps: &FileMap) -> CmdResult<()> {
    if args.input.is_dir() {
        let dir = match &args.output {
            Some(out) => {
                // Everything about to be rewritten is skipped: on a
                // project whose proof files are most of its bytes,
                // copying them first only to overwrite them doubles
                // the writing for nothing.
                let rewritten = bpr
                    .keys()
                    .map(|stem| format!("{stem}.bpr"))
                    .chain(bps.keys().map(|stem| format!("{stem}.bps")))
                    .collect();
                copy_tree(&args.input, out, &rewritten)?;
                out.clone()
            }
            None => args.input.clone(),
        };
        for (stem, contents) in bpr {
            let path = dir.join(format!("{stem}.bpr"));
            match contents {
                Some(bytes) => std::fs::write(path, bytes)?,
                None => std::fs::remove_file(path)?,
            }
        }
        for (stem, contents) in bps {
            std::fs::write(dir.join(format!("{stem}.bps")), contents)?;
        }
        return Ok(());
    }

    let source = std::fs::read(&args.input)?;
    let rebuilt = rebuild_zip(&source, bpr, bps)?;
    let path = args.output.as_ref().unwrap_or(&args.input);
    super::eventb_io::ensure_parent_dir(path)?;
    std::fs::write(path, rebuilt)?;
    Ok(())
}

/// Rebuilds an archive with the rewritten proof entries, copying every
/// other entry through — the shape `rossi fmt` uses to rewrite a
/// component in place.
fn rebuild_zip(bytes: &[u8], bpr: &Rewritten, bps: &FileMap) -> CmdResult<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    let comment = archive.comment().to_vec();
    let mut out = Vec::with_capacity(bytes.len());
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut out));
        writer.set_raw_comment(comment.into_boxed_slice())?;
        for i in 0..archive.len() {
            let entry = archive.by_index(i)?;
            let name = entry.name()?.to_string();
            let rewritten = match name.rsplit_once('.') {
                Some((stem, "bpr")) => match bpr.get(stem) {
                    // The entry is dropped along with the file.
                    Some(None) => continue,
                    Some(Some(contents)) => Some(contents),
                    None => None,
                },
                Some((stem, "bps")) => bps.get(stem),
                _ => None,
            };
            match rewritten {
                Some(contents) => {
                    writer.start_file(
                        name,
                        zip::write::SimpleFileOptions::default()
                            .compression_method(zip::CompressionMethod::Deflated),
                    )?;
                    writer.write_all(contents)?;
                }
                None if entry.is_dir() => {
                    // raw_copy_file marks directory Unix modes as
                    // regular files.
                    let mut options = zip::write::SimpleFileOptions::default()
                        .unix_permissions(entry.unix_mode().unwrap_or(0o755));
                    if let Some(modified) = entry.last_modified().filter(zip::DateTime::is_valid) {
                        options = options.last_modified_time(modified);
                    }
                    let options = options
                        .into_full_options()
                        .with_file_comment(entry.comment());
                    writer.add_directory(name, options)?;
                }
                None => writer.raw_copy_file(entry)?,
            }
        }
        writer.finish()?;
    }
    Ok(out)
}

/// Copies a project directory so `--output` leaves the input alone,
/// omitting the paths (relative to `from`) the caller is about to
/// write itself.
fn copy_tree(from: &Path, to: &Path, skip: &BTreeSet<String>) -> CmdResult<()> {
    // Copying a directory into its own subtree would walk what it is
    // still writing, so say so rather than filling the disk — and
    // before creating anything under the project. The target need not
    // exist yet, so the test is on its nearest existing ancestor.
    let anchor = to
        .ancestors()
        .find(|path| path.exists())
        .unwrap_or(Path::new("."));
    if anchor.canonicalize()?.starts_with(from.canonicalize()?) {
        return Err(format!(
            "--output {} is inside the project being cleaned",
            to.display()
        )
        .into());
    }
    std::fs::create_dir_all(to)?;
    let mut stack = vec![from.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let target = to.join(dir.strip_prefix(from).expect("walked below the root"));
        std::fs::create_dir_all(&target)?;
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let relative = path.strip_prefix(from).expect("walked below the root");
            if relative.to_str().is_some_and(|path| skip.contains(path)) {
                continue;
            }
            let name = path.file_name().expect("a directory entry has a name");
            std::fs::copy(&path, target.join(name))?;
        }
    }
    Ok(())
}
