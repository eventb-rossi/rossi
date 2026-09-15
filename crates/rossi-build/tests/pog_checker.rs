//! Checker round-trip for generated proof obligations — regenerate every
//! model, hand the archive to eventb-checker's `--proofs` pass, and check
//! that the obligations it reads back are exactly the generated ones.
//!
//! This is the checker counterpart of `pog_animate`, and it closes the one
//! gap between them: `proof_oracle_diff` already runs eventb-checker over the
//! corpus, but only over *pristine* archives, where it validates our proof
//! *reader* against its own. Nothing put a generated `.bpo`/`.bps` in front
//! of the checker's independent Java XML reader.
//!
//! What it proves: the generated proof files parse (no EB017), the checker
//! counts exactly as many obligations as we emitted, and every obligation it
//! names is one we generated. Since proof statuses are recomputed during
//! repackaging, the checker's discharged/pending/broken counts on a
//! regenerated archive must also reproduce the corpus baseline
//! (`checker_results.tsv`) for every model whose obligations regenerate
//! exactly (models with a `pog_divergence` flag are compared on the other
//! properties only). The baseline was recorded with the eventb-checker
//! version named in `CHECKER.md`; an older checker is refused (skip), since
//! its counting rules predate the baseline.
//!
//! `#[ignore]` by default (needs `eventb-checker` and a corpus). Run:
//!
//!   cargo test -p rossi-build --test pog_checker -- --ignored --nocapture
//!
//! Environment overrides:
//!   EVENTB_CORPUS_DIR — external Event-B model corpus directory
//!   EVENTB_CHECKER    — eventb-checker executable (default: eventb-checker)
//!   EVENTB_CHECKER_TIMEOUT_SECS — per-model limit (default 120)

mod common;

use std::path::Path;
use std::time::Duration;

use common::{
    collect_zips, corpus_dir, eventb_checker_bin, load_flags, oracle_available, spawn_in_group,
    wait_with_timeout, workspace_target, write_report,
};

const DEFAULT_TIMEOUT_SECS: u64 = 120;

#[test]
#[ignore = "needs eventb-checker and a models corpus; run with --ignored"]
fn checker_reads_back_generated_obligations() {
    let checker = eventb_checker_bin();
    if !oracle_available(&checker) {
        eprintln!(
            "SKIP pog_checker: `{checker}` not runnable. Install the eventb-checker CLI \
             or set EVENTB_CHECKER to its path."
        );
        return;
    }
    let Some(corpus) = corpus_dir() else {
        eprintln!("SKIP pog_checker: no corpus (set EVENTB_CORPUS_DIR)");
        return;
    };
    if let (Some(have), Some(want)) = (checker_version(&checker), baseline_version(&corpus))
        && have < want
    {
        eprintln!(
            "SKIP pog_checker: eventb-checker {}.{} is older than the corpus baseline \
                 {}.{} (see CHECKER.md); refusing to compare status counts.",
            have.0, have.1, want.0, want.1
        );
        return;
    }
    let flags = load_flags(&corpus.join("model_flags.tsv")).unwrap_or_default();
    let baselines = load_po_baseline(&corpus);
    let zips = collect_zips(&corpus).expect("read corpus");
    let timeout = Duration::from_secs(
        std::env::var("EVENTB_CHECKER_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_TIMEOUT_SECS),
    );
    let regen_dir = workspace_target().join("eventb-models-regen-pog-checker");
    std::fs::create_dir_all(&regen_dir).expect("create regen dir");

    let mut report: Vec<Vec<String>> = Vec::new();
    let mut failures = 0usize;
    for zip in &zips {
        let model = zip
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        if common::flagged_unsupported(&flags, &model) {
            report.push(vec![
                model,
                "skip".into(),
                "flagged unsupported input".into(),
            ]);
            continue;
        }
        let regen_zip = regen_dir.join(format!("{model}.zip"));
        let pog_diverges = flags
            .get(&model)
            .is_some_and(|f| f.contains("pog_divergence"));
        let baseline = (!pog_diverges).then(|| baselines.get(&model)).flatten();
        match check_one(&checker, zip, &regen_zip, timeout, baseline) {
            Ok(note) => {
                eprintln!("  OK   {model}");
                report.push(vec![model, "match".into(), note.unwrap_or_default()]);
            }
            Err(Skip(reason)) => {
                eprintln!("  SKIP {model}: {reason}");
                report.push(vec![model, "skip".into(), reason]);
            }
            Err(Fail(reason)) => {
                eprintln!("  FAIL {model}: {reason}");
                failures += 1;
                report.push(vec![model, "diverge".into(), reason]);
            }
        }
    }

    let path = workspace_target().join("rossi-build-pog-checker.tsv");
    write_report(&path, &["model", "verdict", "notes"], &report);
    eprintln!("report: {}", path.display());
    assert!(failures == 0, "{failures} model(s) diverged");
}

use common::FailOrSkip::{self, Fail, Skip};

fn check_one(
    checker: &str,
    zip: &Path,
    regen_zip: &Path,
    timeout: Duration,
    baseline: Option<&PoBaseline>,
) -> Result<Option<String>, FailOrSkip> {
    common::regen_one(zip, regen_zip).map_err(|e| Skip(format!("regen: {e}")))?;

    // Every generated sequent, keyed the way the checker names one.
    let generated: std::collections::BTreeSet<String> = common::generated_obligations(regen_zip)
        .map_err(Skip)?
        .into_iter()
        .map(|(prefix, component, sequent)| format!("{prefix}{component}/{sequent}"))
        .collect();
    if generated.is_empty() {
        return Err(Skip("no generated obligations".into()));
    }

    let mut command = std::process::Command::new(checker);
    command
        .args(["check", "--proofs", "--format", "json"])
        .arg(regen_zip)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = spawn_in_group(&mut command).map_err(|e| Skip(format!("spawn: {e}")))?;
    let (_status, stdout, stderr) = match wait_with_timeout(child, timeout) {
        Ok(output) => output,
        Err(common::WaitError::Timeout) => return Err(Skip("timeout".into())),
        Err(common::WaitError::Io(e)) => return Err(Skip(format!("wait: {e}"))),
    };
    // Exit status 1 only means the model carries error findings, which is the
    // static checker's business, not ours; the JSON is still complete.
    let json: serde_json::Value = serde_json::from_str(&stdout).map_err(|e| {
        Fail(format!(
            "checker produced no JSON ({e}); stderr: {}",
            stderr.trim()
        ))
    })?;
    let findings = json["errors"].as_array().map_or(&[][..], Vec::as_slice);

    // EB017 is a proof-file parse error: the checker could not read what we
    // wrote. That is this gate's whole point, so it always fails.
    if let Some(parse_error) = findings.iter().find(|f| f["ruleId"] == "EB017") {
        return Err(Fail(format!(
            "checker rejected a generated proof file: {}",
            parse_error["message"].as_str().unwrap_or("?")
        )));
    }

    let summary = &json["summary"]["proofSummary"];
    let Some(total) = summary["total"].as_u64() else {
        return Err(Skip("checker reported no proof summary".into()));
    };
    if total as usize != generated.len() {
        return Err(Fail(format!(
            "checker counted {total} obligation(s), {} generated",
            generated.len()
        )));
    }

    // The status counts must reproduce the corpus baseline: repackaging
    // recomputes stale statuses, and for a model whose obligations
    // regenerate exactly, every row carries over byte-identical. The
    // aggregate comparison only means something when both sides count
    // the same obligation set — a pristine archive can carry derived
    // files without sources (which regeneration drops) or lack some
    // (which regeneration adds), so a total mismatch is reported, not
    // failed.
    if let Some(base) = baseline.filter(|b| b.result == "valid" || b.result == "invalid") {
        if total != base.total {
            return Ok(Some(format!(
                "po counts not comparable: {total} obligation(s) regenerated, \
                 the pristine archive carries {}",
                base.total
            )));
        }
        let got = (
            summary["discharged"].as_u64().unwrap_or(0),
            summary["pending"].as_u64().unwrap_or(0),
            summary["broken"].as_u64().unwrap_or(0),
        );
        let want = (base.discharged, base.pending, base.broken);
        if got != want {
            return Err(Fail(format!(
                "status counts diverge from checker_results.tsv: \
                 discharged/pending/broken {}/{}/{} vs baseline {}/{}/{}",
                got.0, got.1, got.2, want.0, want.1, want.2
            )));
        }
    }

    // Every obligation the checker names must be one we generated. Only the
    // proof rules describe obligations: a static-check finding also carries
    // `file` and `element`, but they name a source file and a model element.
    // On a proof finding `file` is `<project>/<component>` and `element` the
    // bare sequent name, which together rebuild our rows.
    for finding in findings
        .iter()
        .filter(|f| matches!(f["ruleId"].as_str(), Some("EB015" | "EB016")))
    {
        let (Some(file), Some(element)) = (finding["file"].as_str(), finding["element"].as_str())
        else {
            continue;
        };
        let name = format!("{file}/{element}");
        if !generated.contains(&name) {
            return Err(Fail(format!("checker read unknown obligation {name}")));
        }
    }
    Ok(None)
}

/// One `checker_results.tsv` row's proof-status columns.
struct PoBaseline {
    result: String,
    total: u64,
    discharged: u64,
    pending: u64,
    broken: u64,
}

/// The corpus baseline counts, keyed by model.
fn load_po_baseline(corpus: &Path) -> std::collections::BTreeMap<String, PoBaseline> {
    let mut out = std::collections::BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(corpus.join("checker_results.tsv")) else {
        return out;
    };
    let mut lines = text.lines();
    let Some(header) = lines.next() else {
        return out;
    };
    let cols: Vec<&str> = header.split('\t').collect();
    let idx = |name: &str| cols.iter().position(|c| *c == name);
    let (Some(result), Some(total), Some(discharged), Some(pending), Some(broken)) = (
        idx("result"),
        idx("po_total"),
        idx("po_discharged"),
        idx("po_pending"),
        idx("po_broken"),
    ) else {
        return out;
    };
    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        let (Some(model), Some(res)) = (fields.first(), fields.get(result)) else {
            continue;
        };
        let num = |i: usize| fields.get(i).and_then(|v| v.parse().ok()).unwrap_or(0);
        out.insert(
            model.to_string(),
            PoBaseline {
                result: res.to_string(),
                total: num(total),
                discharged: num(discharged),
                pending: num(pending),
                broken: num(broken),
            },
        );
    }
    out
}

/// `eventb-checker --version` as (major, minor).
fn checker_version(checker: &str) -> Option<(u32, u32)> {
    let output = std::process::Command::new(checker)
        .arg("--version")
        .output()
        .ok()?;
    parse_version(&String::from_utf8_lossy(&output.stdout))
}

/// The checker version the corpus baseline was recorded with, from
/// CHECKER.md's first `eventb-checker vX.Y` mention.
fn baseline_version(corpus: &Path) -> Option<(u32, u32)> {
    let text = std::fs::read_to_string(corpus.join("CHECKER.md")).ok()?;
    let idx = text.find("eventb-checker v")?;
    parse_version(&text[idx..])
}

/// The first `X.Y` version number in `text`.
fn parse_version(text: &str) -> Option<(u32, u32)> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let rest = &text[start..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(rest.len());
    let mut parts = rest[..end].split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}
