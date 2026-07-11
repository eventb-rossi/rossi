//! Corpus integration test — regenerate every model with our static checker,
//! then build it headlessly with Rodin and confirm Rodin still accepts the
//! result. This is the Rodin counterpart of `animate_corpus` (which exercises
//! the ProB side via `eventb-animate`).
//!
//! Rodin is driven through the `rodin-headless` `rodin` wrapper, which selects a
//! native or Docker/podman runtime internally. Its `build` command runs a full
//! Rodin workspace build on a `.zip` and writes the generated `.bcc`/`.bcm`
//! back into the archive in place. We pass it the *regenerated* archive (rossi
//! sources + our freshly generated checked files) and then read the archive
//! back to see whether every component checked accurately.
//!
//! What this gates: a full Rodin build re-derives `.bcc`/`.bcm` from the
//! `.buc`/`.bum` source, so this primarily guards that rossi's regeneration
//! keeps a project *Rodin-loadable and buildable* (project layout, dropped
//! proof artifacts, valid source XML). The promise we hold is one-directional:
//! **a model the reference calls `valid` must build accurately under Rodin
//! after rossi regenerates it.** Models the reference calls `invalid` (or that
//! have no reference row) are reported for visibility but never fail the test —
//! they are already broken or are eventb-checker/Rodin disagreements, not rossi
//! regressions.
//!
//! `checker_results.tsv` is carried through as informational corpus metadata,
//! not as the oracle for this test: if Rodin builds a Rossi-regenerated archive
//! accurately, the model passes even when another checker rejects the pristine
//! source. Expected checker/Rodin disagreements belong in `model_flags.tsv`
//! under `checker_divergence`.
//!
//! `#[ignore]` by default: the corpus and the Rodin runtime live outside the
//! repo. Run locally (after preparing a Rodin runtime — `rodin-install.sh` for
//! native, or a pre-built Docker image; the per-model timeout does not cover a
//! first-run image build):
//!
//!   cargo test -p rossi-build --test rodin_corpus -- --ignored --nocapture
//!
//! Environment overrides:
//!   EVENTB_CORPUS_DIR          — external Event-B model corpus directory
//!   RODIN_HEADLESS             — path to the rodin-headless `rodin` wrapper (required)
//!   RODIN_HEADLESS_TIMEOUT_SECS — per-model build timeout (default 600)
//!   RODIN_HEADLESS_REGEN_DIR   — where regenerated archives are written
//!                                (default target/eventb-models-regen-rodin).
//!                                Set this to a path the Rodin runtime can read:
//!                                a containerised runtime only sees its working
//!                                directory, and the macOS podman VM shares
//!                                `$HOME` but not `/Volumes` or `/tmp`.
//! Relative paths are resolved from the workspace root.
//!
//! Output:
//!   target/eventb-models-regen-rodin/<model>.zip — regenerated archives
//!                                                  (mutated in place by Rodin)
//!   target/rossi-build-rodin-corpus.tsv          — model | checker_status | rodin_actual | verdict | notes
//!
//! Verdicts:
//!   match    — Rodin built every component accurately after Rossi regeneration
//!   known    — a model flagged `defective` (broken source that cannot
//!              regenerate), `unsupported` (needs an Event-B extension rossi
//!              doesn't support yet, e.g. the theory plugin), or
//!              `rodin_rejected` (Rodin's static checker has never accepted
//!              the pristine archive, so the failure predates rossi) in the
//!              corpus `model_flags.tsv`
//!   regress  — an unflagged Rossi regeneration, Rodin launch/load, timeout, or
//!              static-check failure (fails the test)

mod common;

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use common::{
    Row, WaitError, env_path, load_expected, load_flags, locate_corpus, log_hint, regen_one,
    resolve_program, spawn_in_group, wait_with_timeout, workspace_target, write_report,
};

const DEFAULT_TIMEOUT_SECS: u64 = 600;

#[test]
#[ignore]
fn rodin_builds_regenerated_corpus() {
    let Some(corpus) = locate_corpus() else {
        eprintln!("EVENTB_CORPUS_DIR is not set or is not a directory — nothing to do");
        return;
    };
    let Some(rodin) = locate_rodin() else {
        let configured = std::env::var("RODIN_HEADLESS").unwrap_or_default();
        if configured.is_empty() {
            eprintln!("RODIN_HEADLESS is not set — nothing to do");
        } else {
            eprintln!(
                "RODIN_HEADLESS command `{configured}` was not found or is not executable — nothing to do"
            );
        }
        return;
    };
    // Skip-when-unset applies to the *environment* (no corpus, no Rodin); a
    // configured corpus with a missing or malformed reference file is a loud
    // failure — silently returning here would green-light a 0-model "gate".
    let reference_tsv = corpus.join("checker_results.tsv");
    let expected = load_expected(&reference_tsv).unwrap_or_else(|| {
        panic!("{} is missing or malformed", reference_tsv.display());
    });
    let flags_tsv = corpus.join("model_flags.tsv");
    let flags = load_flags(&flags_tsv).unwrap_or_else(|| {
        panic!("{} is missing or malformed", flags_tsv.display());
    });

    // The regenerated archives must live somewhere the Rodin runtime can read.
    // The native runtime sees the whole filesystem, but a containerised one
    // (Docker/podman) only mounts the working directory — and on macOS the
    // podman VM shares `$HOME`, not `/Volumes` or `/tmp`. Allow an override so a
    // container user can place the regen dir under a VM-shared path.
    let regen_dir = env_path("RODIN_HEADLESS_REGEN_DIR")
        .unwrap_or_else(|| workspace_target().join("eventb-models-regen-rodin"));
    std::fs::create_dir_all(&regen_dir).expect("create regen dir");
    let timeout = Duration::from_secs(
        std::env::var("RODIN_HEADLESS_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_TIMEOUT_SECS),
    );

    let mut zips: Vec<PathBuf> = std::fs::read_dir(&corpus)
        .expect("read corpus")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("zip"))
        .collect();
    zips.sort();

    let known_failure: std::collections::BTreeSet<&str> = flags
        .iter()
        .filter(|(_, f)| {
            f.contains("defective") || f.contains("unsupported") || f.contains("rodin_rejected")
        })
        .map(|(m, _)| m.as_str())
        .collect();
    let checker_divergence: std::collections::BTreeSet<&str> = flags
        .iter()
        .filter(|(_, f)| f.contains("checker_divergence"))
        .map(|(m, _)| m.as_str())
        .collect();
    let mut rows = Vec::<Row>::new();
    let mut regressions = 0usize;

    for zip in &zips {
        let model = zip
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        let regen_zip = regen_dir.join(format!("{model}.zip"));
        let outcome = match regen_one(zip, &regen_zip) {
            Ok(()) => rodin_build_one(&rodin, &regen_dir, &regen_zip, timeout),
            Err(e) => Outcome::Regen(e.to_string()),
        };

        let checker_status = expected
            .get(&model)
            .cloned()
            .unwrap_or_else(|| "?".to_string());
        let actual_str = outcome.label();
        let built = actual_str == "built";
        let verdict = if built {
            "match"
        } else if known_failure.contains(model.as_str()) {
            "known"
        } else {
            regressions += 1;
            "regress"
        };
        let notes = if built && checker_divergence.contains(model.as_str()) {
            "checker_divergence: eventb-checker rejects this archive, but Rodin builds it cleanly"
                .to_string()
        } else {
            outcome.notes().to_string()
        };
        rows.push(Row {
            model: model.clone(),
            expected: checker_status,
            actual: actual_str.to_string(),
            verdict: verdict.to_string(),
            notes,
        });
    }

    let report = workspace_target().join("rossi-build-rodin-corpus.tsv");
    write_report(
        &report,
        &[
            "model",
            "checker_status",
            "rodin_actual",
            "verdict",
            "notes",
        ],
        &rows.iter().map(Row::to_fields).collect::<Vec<_>>(),
    );
    println!(
        "rodin-corpus: {} archives, {} regressions (report: {})",
        zips.len(),
        regressions,
        report.display()
    );
    for r in rows.iter().filter(|r| r.verdict == "regress").take(20) {
        eprintln!(
            "  REGRESS  {}: checker {}, rodin {} — {}",
            r.model, r.expected, r.actual, r.notes
        );
    }
    assert!(
        regressions == 0,
        "{regressions} valid model(s) failed to build under Rodin (first 20 shown above)"
    );
}

#[derive(Debug, Clone)]
enum Outcome {
    /// Rodin built every component accurately.
    Built,
    /// Rodin built but at least one component carries static-check errors.
    ScError(String),
    /// Rodin could not load/build the project at all (import crash, no output).
    LoadError(String),
    /// The build exceeded the timeout and was killed.
    Timeout,
    /// rossi could not regenerate the archive in the first place.
    Regen(String),
}

impl Outcome {
    fn label(&self) -> &'static str {
        match self {
            Outcome::Built => "built",
            Outcome::ScError(_) => "sc_error",
            Outcome::LoadError(_) => "load_error",
            Outcome::Timeout => "timeout",
            Outcome::Regen(_) => "regen_error",
        }
    }

    fn notes(&self) -> &str {
        match self {
            Outcome::ScError(s) | Outcome::LoadError(s) | Outcome::Regen(s) => s.as_str(),
            _ => "",
        }
    }
}

fn locate_rodin() -> Option<PathBuf> {
    let configured = std::env::var("RODIN_HEADLESS").ok()?;
    if configured.trim().is_empty() {
        return None;
    }
    resolve_program(&configured)
}

/// Run `<rodin> build <model>.zip` from inside `regen_dir`. We `cd` into the
/// archive's directory and pass a bare filename so both runtimes resolve it:
/// the Docker wrapper mounts the current directory into the container, and the
/// native path operates relative to it.
fn rodin_build_one(rodin: &Path, regen_dir: &Path, zip: &Path, timeout: Duration) -> Outcome {
    let file_name = match zip.file_name().and_then(|s| s.to_str()) {
        Some(n) => n.to_string(),
        None => return Outcome::LoadError("bad regen path".into()),
    };

    let mut cmd = Command::new(rodin);
    cmd.current_dir(regen_dir)
        .arg("build")
        .arg(&file_name)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = match spawn_in_group(&mut cmd) {
        Ok(c) => c,
        Err(e) => return Outcome::LoadError(format!("spawn: {e}")),
    };

    match wait_with_timeout(child, timeout) {
        Ok((status, stdout, stderr)) => classify(status.code(), &stdout, &stderr, zip),
        Err(WaitError::Timeout) => Outcome::Timeout,
        Err(WaitError::Io(e)) => Outcome::LoadError(format!("wait: {e}")),
    }
}

/// Classify a Rodin build. The exit code gates first: a non-zero exit means
/// the wrapper/JVM pipeline failed (launch error, internal timeout,
/// interrupt) — possibly *before* Rodin touched the archive, whose checked
/// files would then still be rossi's own pre-seeded output, so their accuracy
/// proves nothing. A zero exit can still accompany Rodin database exceptions,
/// so those logs gate before the rewritten archive. Only then is the accuracy
/// of the `.bcc`/`.bcm` authoritative: a clean build marks every component
/// `org.eventb.core.accurate="true"`. (A static-check failure is *not*
/// signalled by the exit code — non-strict builds exit 0 and record
/// `accurate="false"` in the rewritten archive.)
fn classify(exit: Option<i32>, stdout: &str, stderr: &str, zip: &Path) -> Outcome {
    if exit != Some(0) {
        let mut hint = log_hint(stderr);
        if hint.is_empty() {
            hint = log_hint(stdout);
        }
        if hint.is_empty() {
            hint = format!("rodin build failed (exit {exit:?})");
        }
        return Outcome::LoadError(hint);
    }
    if let Some(error) = rodin_database_error(stdout, stderr) {
        return Outcome::LoadError(error);
    }
    match inspect_accuracy(zip) {
        Accuracy::Inaccurate(detail) => Outcome::ScError(detail),
        Accuracy::AllAccurate => Outcome::Built,
        Accuracy::Unknown => Outcome::LoadError("no checked files produced (exit 0)".into()),
    }
}

fn rodin_database_error(stdout: &str, stderr: &str) -> Option<String> {
    stderr
        .lines()
        .chain(stdout.lines())
        .find(|line| {
            line.contains("Duplicate child")
                || line.contains("Rodin Database Exception")
                || (line.contains("[org.eventb.core.sc") && line.contains("does not exist"))
        })
        .map(|line| line.trim().to_string())
}

enum Accuracy {
    /// At least one source component is missing a checked file or is inaccurate.
    Inaccurate(String),
    /// Every source component has an accurate checked file.
    AllAccurate,
    /// The archive carries no `.bcc`/`.bcm` to judge by.
    Unknown,
}

/// Read the (Rodin-rewritten) archive and decide whether every `.buc`/`.bum`
/// source has a matching accurate `.bcc`/`.bcm` beside it. Entries are keyed
/// by full archive path with the checked extension (`p/X.buc` owes `p/X.bcc`,
/// `p/X.bum` owes `p/X.bcm`), so a context and a machine sharing a component
/// name are judged independently — basename-stem keying would let an accurate
/// `X.bcm` silently overwrite an inaccurate `X.bcc` verdict.
fn inspect_accuracy(zip: &Path) -> Accuracy {
    let Ok(file) = std::fs::File::open(zip) else {
        return Accuracy::Unknown;
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        return Accuracy::Unknown;
    };

    let mut owed: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut checked: BTreeMap<String, bool> = BTreeMap::new();

    for i in 0..archive.len() {
        let Ok(mut entry) = archive.by_index(i) else {
            continue;
        };
        let name = entry.name().to_string();
        if let Some(stem) = name.strip_suffix(".buc") {
            owed.insert(format!("{stem}.bcc"));
        } else if let Some(stem) = name.strip_suffix(".bum") {
            owed.insert(format!("{stem}.bcm"));
        } else if name.ends_with(".bcc") || name.ends_with(".bcm") {
            let mut content = String::new();
            // An unreadable checked file is a failed check, not a pass. A
            // readable one with no accurate attribute at all counts as
            // accurate on purpose: Rodin writes an attribute-less stub for
            // components whose configuration needs a plugin the headless
            // install lacks, and the reference treats those builds as clean.
            let accurate = entry.read_to_string(&mut content).is_ok()
                && !content.contains("org.eventb.core.accurate=\"false\"");
            checked.insert(name, accurate);
        }
    }

    if checked.is_empty() {
        return Accuracy::Unknown;
    }

    let mut bad = Vec::new();
    for name in &owed {
        match checked.get(name) {
            Some(true) => {}
            Some(false) => bad.push(format!("{name}: inaccurate")),
            None => bad.push(format!("{name}: not checked")),
        }
    }
    // An inaccurate checked file with no matching source (unlikely) still counts.
    for (name, accurate) in &checked {
        if !accurate && !owed.contains(name) {
            bad.push(format!("{name}: inaccurate"));
        }
    }

    if bad.is_empty() {
        Accuracy::AllAccurate
    } else {
        Accuracy::Inaccurate(bad.join("; "))
    }
}

#[test]
fn database_integrity_logs_fail_successful_builds() {
    let cases = [
        "!MESSAGE Duplicate child inv1[org.eventb.core.scInvariant]",
        "Rodin Database Exception: Rodin Database Status",
        "INITIALISATION[org.eventb.core.scEvent] does not exist",
    ];

    for message in cases {
        assert!(matches!(
            classify(Some(0), message, "", Path::new("unused.zip")),
            Outcome::LoadError(detail) if detail == message
        ));
    }
    assert!(rodin_database_error("Build complete.", "").is_none());
}
