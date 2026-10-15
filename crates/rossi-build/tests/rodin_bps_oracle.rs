//! Live-oracle gate for computed proof statuses — hand the same
//! status-less archive to a reference build and to rossi, and require
//! both sides to derive the same `.bps` rows.
//!
//! For every corpus model carrying a `rodin_bps_subset` flag, the
//! archive's `.bps` entries are stripped (its `.bpo` and `.bpr` kept),
//! so each side must recompute every row from the stored proofs
//! against the obligations — the pure status-update pass. The
//! reference side runs `$RODIN_HEADLESS build --auto-tactics off`
//! (which repackages
//! the archive in place); the rossi side regenerates the archive,
//! whose repackaging runs `update_statuses`. Rows compare by name
//! order and by confidence, psBroken and psManual exactly; stamps are
//! only checked for self-consistency within each side (each row
//! stamped like its own sequent), since the two generators number
//! stamps independently.
//!
//! The subset is chosen corpus-side (single-project models whose
//! obligations regenerate exactly; `pog_divergence` models are
//! refused here as their sequents differ between the sides). One
//! divergence shape is reported as a note rather than failed: a
//! component that exists only as derived files with no source
//! (the reference updater still processes its surviving `.bpo` while
//! regeneration drops it).
//!
//! `#[ignore]` by default (needs a corpus and a reference build). Run:
//!
//!   RODIN_HEADLESS=/path/to/wrapper \
//!   cargo test --release -p rossi-build --test rodin_bps_oracle -- --ignored --nocapture
//!
//! `RODIN_HEADLESS` names a command invoked as
//! `$RODIN_HEADLESS build --auto-tactics off <zip>` — typically a
//! wrapper around `podman run …/rodin-headless` that mounts the zip's
//! directory as `/models`.
//!
//! Environment overrides:
//!   EVENTB_CORPUS_DIR — external Event-B model corpus directory
//!   RODIN_BPS_TIMEOUT_SECS — per-model build limit (default 3600)

mod common;

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use common::{
    corpus_dir, load_flags, regen_one, spawn_in_group, wait_with_timeout, workspace_target,
    write_report,
};
use rossi_build::pog::reconcile::sequent_stamps;
use rossi_prove::bps::{PsStatus, read_bps};

const DEFAULT_TIMEOUT_SECS: u64 = 3600;

/// Problems reported per model before truncation.
const MAX_PROBLEMS: usize = 10;

#[test]
#[ignore = "needs a reference build oracle and a models corpus; run with --ignored"]
fn rodin_build_reproduces_computed_statuses() {
    let Ok(rodin) = std::env::var("RODIN_HEADLESS") else {
        eprintln!("SKIP rodin_bps_oracle: set RODIN_HEADLESS to a rodin-headless command");
        return;
    };
    let Some(corpus) = corpus_dir() else {
        eprintln!("SKIP rodin_bps_oracle: no corpus (set EVENTB_CORPUS_DIR)");
        return;
    };
    let flags = load_flags(&corpus.join("model_flags.tsv")).unwrap_or_default();
    let subset: Vec<String> = flags
        .iter()
        .filter(|(_, f)| f.contains("rodin_bps_subset"))
        .map(|(model, _)| model.clone())
        .collect();
    if subset.is_empty() {
        eprintln!("SKIP rodin_bps_oracle: the corpus flags no rodin_bps_subset models");
        return;
    }
    let timeout = Duration::from_secs(
        std::env::var("RODIN_BPS_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_TIMEOUT_SECS),
    );
    let workdir = workspace_target().join("rodin-bps-oracle");
    std::fs::create_dir_all(&workdir).expect("create work dir");

    let mut report: Vec<Vec<String>> = Vec::new();
    let mut failures = 0usize;
    for model in &subset {
        if flags
            .get(model)
            .is_some_and(|f| f.contains("pog_divergence"))
        {
            report.push(vec![
                model.clone(),
                "skip".into(),
                "pog_divergence: the sides generate different sequents".into(),
            ]);
            continue;
        }
        let zip = corpus.join(format!("{model}.zip"));
        match check_one(&rodin, &zip, &workdir, model, timeout) {
            Ok(note) => {
                eprintln!("  OK   {model}");
                report.push(vec![
                    model.clone(),
                    "match".into(),
                    note.unwrap_or_default(),
                ]);
            }
            Err(Skip(reason)) => {
                eprintln!("  SKIP {model}: {reason}");
                report.push(vec![model.clone(), "skip".into(), reason]);
            }
            Err(Fail(reason)) => {
                eprintln!("  FAIL {model}: {reason}");
                failures += 1;
                report.push(vec![model.clone(), "diverge".into(), reason]);
            }
        }
    }

    let path = workspace_target().join("rossi-build-rodin-bps-oracle.tsv");
    write_report(&path, &["model", "verdict", "notes"], &report);
    eprintln!("report: {}", path.display());
    assert!(failures == 0, "{failures} model(s) diverged");
}

use common::FailOrSkip::{self, Fail, Skip};

fn check_one(
    rodin: &str,
    zip: &Path,
    workdir: &Path,
    model: &str,
    timeout: Duration,
) -> Result<Option<String>, FailOrSkip> {
    let bytes = std::fs::read(zip).map_err(|e| Skip(format!("read: {e}")))?;
    let stripped = strip_bps(&bytes).map_err(Skip)?;

    // The reference side builds its copy in place.
    let rodin_zip = workdir.join(format!("{model}.zip"));
    std::fs::write(&rodin_zip, &stripped).map_err(|e| Skip(format!("write: {e}")))?;
    let mut command = std::process::Command::new(rodin);
    command
        .args(["build", "--auto-tactics", "off"])
        .arg(&rodin_zip)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = spawn_in_group(&mut command).map_err(|e| Skip(format!("spawn: {e}")))?;
    let (status, _stdout, stderr) = match wait_with_timeout(child, timeout) {
        Ok(output) => output,
        Err(common::WaitError::Timeout) => return Err(Skip("rodin build timeout".into())),
        Err(common::WaitError::Io(e)) => return Err(Skip(format!("wait: {e}"))),
    };
    if !status.success() {
        return Err(Skip(format!(
            "rodin build failed ({status}): {}",
            stderr.lines().last().unwrap_or("").trim()
        )));
    }
    let rodin_bytes = std::fs::read(&rodin_zip).map_err(|e| Skip(format!("read back: {e}")))?;
    let rodin_side = proof_state(&rodin_bytes).map_err(Skip)?;
    if rodin_side.is_empty() {
        return Err(Skip("rodin build produced no status files".into()));
    }

    // The rossi side regenerates the same stripped archive.
    let stripped_zip = workdir.join(format!("{model}-stripped.zip"));
    std::fs::write(&stripped_zip, &stripped).map_err(|e| Skip(format!("write: {e}")))?;
    let rossi_zip = workdir.join(format!("{model}-rossi.zip"));
    regen_one(&stripped_zip, &rossi_zip).map_err(|e| Skip(format!("regen: {e}")))?;
    let rossi_bytes = std::fs::read(&rossi_zip).map_err(|e| Skip(format!("read back: {e}")))?;
    let rossi_side = proof_state(&rossi_bytes).map_err(Skip)?;

    let mut problems = Vec::new();
    let mut notes = Vec::new();
    for name in rodin_side.keys() {
        if !rossi_side.contains_key(name) {
            if has_source(&stripped, name) {
                problems.push(format!("missing status file {name}"));
            } else {
                notes.push(format!("{name}: sourceless derived files, not regenerated"));
            }
        }
    }
    for name in rossi_side.keys() {
        if !rodin_side.contains_key(name) {
            problems.push(format!("extra status file {name}"));
        }
    }
    for (name, rodin_comp) in &rodin_side {
        let Some(rossi_comp) = rossi_side.get(name) else {
            continue;
        };
        diff_component(name, rodin_comp, rossi_comp, &mut problems);
        if problems.len() > MAX_PROBLEMS {
            break;
        }
    }
    if problems.is_empty() {
        Ok(if notes.is_empty() {
            None
        } else {
            notes.truncate(MAX_PROBLEMS);
            Some(notes.join("; "))
        })
    } else {
        problems.truncate(MAX_PROBLEMS);
        Err(Fail(problems.join("; ")))
    }
}

/// One component's `.bps` rows and its `.bpo` sequent stamps.
struct ComponentState {
    rows: Vec<PsStatus>,
    stamps: std::collections::HashMap<String, String>,
}

/// Whether a component has a direct source file in the archive; a
/// derived file without one is a stray no build ever rebuilds and rossi
/// never regenerates.
fn has_source(zip_bytes: &[u8], bps_name: &str) -> bool {
    let Ok(archive) = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)) else {
        return false;
    };
    let stem = bps_name.trim_end_matches(".bps");
    archive
        .file_names()
        .any(|n| n.is_ok_and(|n| n == format!("{stem}.bum") || n == format!("{stem}.buc")))
}

/// The archive's proof state, keyed by `.bps` entry path.
fn proof_state(zip_bytes: &[u8]) -> Result<BTreeMap<String, ComponentState>, String> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(|e| format!("zip: {e}"))?;
    let names: Vec<String> = archive
        .file_names()
        .map(|name| name.map(String::from))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("zip: {e}"))?;
    let mut out = BTreeMap::new();
    for name in names {
        let Some(stem) = name.strip_suffix(".bps") else {
            continue;
        };
        let mut text = String::new();
        archive
            .by_name(&name)
            .map_err(|e| format!("zip: {e}"))?
            .read_to_string(&mut text)
            .map_err(|e| format!("read {name}: {e}"))?;
        let rows = read_bps(text.as_bytes()).map_err(|e| format!("parse {name}: {e}"))?;
        let mut bpo = String::new();
        let stamps = match archive.by_name(&format!("{stem}.bpo")) {
            Ok(mut entry) => {
                entry
                    .read_to_string(&mut bpo)
                    .map_err(|e| format!("read {stem}.bpo: {e}"))?;
                sequent_stamps(&bpo)
            }
            Err(_) => Default::default(),
        };
        out.insert(name, ComponentState { rows, stamps });
    }
    Ok(out)
}

fn diff_component(
    file: &str,
    rodin: &ComponentState,
    rossi: &ComponentState,
    problems: &mut Vec<String>,
) {
    let names = |state: &ComponentState| -> Vec<String> {
        state.rows.iter().map(|r| r.name.clone()).collect()
    };
    if names(rodin) != names(rossi) {
        problems.push(format!(
            "{file}: row names differ (rodin {} rows, rossi {})",
            rodin.rows.len(),
            rossi.rows.len()
        ));
        return;
    }
    for (want, got) in rodin.rows.iter().zip(&rossi.rows) {
        if want.confidence != got.confidence
            || want.broken != got.broken
            || want.manual != got.manual
        {
            problems.push(format!(
                "{file}: {}: rodin confidence={:?} broken={} manual={}, \
                 rossi confidence={:?} broken={} manual={}",
                want.name,
                want.confidence,
                want.broken,
                want.manual,
                got.confidence,
                got.broken,
                got.manual
            ));
        }
    }
    // Per-side stamp self-consistency: each row carries its own
    // sequent's stamp.
    for (label, state) in [("rodin", rodin), ("rossi", rossi)] {
        for row in &state.rows {
            let expected = state.stamps.get(&row.name);
            let row_stamp = row.po_stamp.as_deref().unwrap_or("0");
            if expected.is_some_and(|s| s != row_stamp) {
                problems.push(format!(
                    "{file}: {}: {label} row stamp {row_stamp} differs from its sequent's {}",
                    row.name,
                    expected.map(String::as_str).unwrap_or("?")
                ));
            }
        }
    }
}

/// The archive without its `.bps` entries (everything else byte-exact).
fn strip_bps(zip_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(|e| format!("zip: {e}"))?;
    let mut out = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(&mut out);
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| format!("zip: {e}"))?;
        let name = entry.name().map_err(|e| format!("zip: {e}"))?.into_owned();
        if name.ends_with(".bps") {
            continue;
        }
        if entry.is_dir() {
            let options = zip::write::SimpleFileOptions::default();
            writer
                .add_directory(name, options)
                .map_err(|e| format!("zip: {e}"))?;
            continue;
        }
        writer
            .raw_copy_file(entry)
            .map_err(|e| format!("zip: {e}"))?;
    }
    writer.finish().map_err(|e| format!("zip: {e}"))?;
    Ok(out.into_inner())
}
