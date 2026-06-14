//! Well-definedness oracle diff — compares `rossi_build::wd` against
//! eventb-checker's EB010 findings over a corpus of real Rodin archives.
//!
//! This is a GATE on message text: every finding both sides report for
//! the same `(component, element)` must be byte-identical (eventb-checker
//! prints Rodin's `Predicate#toString()`, so this pins the whole
//! compute → improve → render pipeline to Rodin's output).
//!
//! Coverage differences are allowed only for models the corpus flags with
//! `wd_coverage_gap` in its `model_flags.tsv` — places where the two tools
//! legitimately check different formula sets (each row's notes give the
//! reason: a component rossi's parser rejects, a decomposition stub rossi
//! does not check, an event/guard/witness one side's type check drops, a
//! variant labelled differently, …). Keeping the model list in the corpus
//! rather than this source avoids leaking corpus contents into the tree.
//! Message mismatches still fail even for flagged models — only one-sided
//! rows are tolerated.
//!
//! Ignored by default (needs the `eventb-checker` CLI and a models
//! corpus). Run:
//!
//!   cargo test -p rossi-build --test wd_oracle_diff -- --ignored --nocapture
//!
//! The corpus directory is taken from `EVENTB_CORPUS_DIR` (the test skips
//! when unset). The oracle runs from `PATH`; set `EVENTB_CHECKER` to
//! override.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use common::{collect_zips, eventb_checker_bin, load_flags, locate_corpus, oracle_available};
use rossi_build::rules::RuleId;
use rossi_build::{Project, build_with_model, wd};

#[test]
#[ignore = "needs the eventb-checker CLI and a models corpus; run with --ignored"]
fn wd_oracle_diff() {
    let oracle = eventb_checker_bin();
    if !oracle_available(&oracle) {
        eprintln!(
            "SKIP wd_oracle_diff: `{oracle}` not runnable. Install the eventb-checker CLI \
             or set EVENTB_CHECKER to its path."
        );
        return;
    }
    let Some(dir) = locate_corpus() else {
        eprintln!("SKIP wd_oracle_diff: EVENTB_CORPUS_DIR is not set");
        return;
    };
    let zips = collect_zips(&dir);
    if zips.is_empty() {
        eprintln!("SKIP wd_oracle_diff: no .zip models in {}", dir.display());
        return;
    }
    // Coverage-gap models are declared in the corpus, not here — see the
    // module docs.
    let gap_flags = load_flags(&dir.join("model_flags.tsv")).unwrap_or_default();
    eprintln!("oracle: {oracle}");
    eprintln!("corpus: {} zip(s) in {}", zips.len(), dir.display());

    let (mut matched, mut failures) = (0usize, Vec::new());
    for zip in &zips {
        let name = zip.file_name().and_then(|s| s.to_str()).unwrap_or("?");
        let model = zip.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let coverage_gap_ok = gap_flags
            .get(model)
            .is_some_and(|f| f.contains("wd_coverage_gap"));
        match diff_one(&oracle, zip, coverage_gap_ok) {
            Ok(n) => {
                matched += n;
                eprintln!("  OK   {name} ({n} finding(s))");
            }
            Err(e) => {
                eprintln!("  FAIL {name}: {e}");
                failures.push(format!("{name}: {e}"));
            }
        }
    }
    eprintln!("total byte-identical findings: {matched}");

    assert!(
        failures.is_empty(),
        "{} model(s) diverged from the oracle:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Returns the number of byte-identical findings on success. `coverage_gap_ok`
/// comes from the corpus `wd_coverage_gap` flag and tolerates one-sided rows.
fn diff_one(oracle: &str, zip: &Path, coverage_gap_ok: bool) -> Result<usize, String> {
    let theirs = rodin_wd(oracle, zip)?;
    // A rossi-side loader failure is a real divergence, not "0 findings" —
    // only swallow it for models we already know rossi can't parse.
    let mine = match rossi_wd(zip) {
        Ok(m) => m,
        Err(_) if coverage_gap_ok => BTreeMap::new(),
        Err(e) => return Err(e),
    };

    let mut matched = 0usize;
    let mut problems = Vec::new();
    let keys: Vec<&(String, String)> = mine.keys().chain(theirs.keys()).collect();
    let mut seen = std::collections::HashSet::new();
    for key in keys {
        if !seen.insert(key) {
            continue;
        }
        match (mine.get(key), theirs.get(key)) {
            (Some(a), Some(b)) if a == b => matched += 1,
            (Some(a), Some(b)) => problems.push(format!(
                "MISMATCH {}/{}: rossi `{a}` vs oracle `{b}`",
                key.0, key.1
            )),
            (Some(_), None) if coverage_gap_ok => {}
            (None, Some(_)) if coverage_gap_ok => {}
            (Some(_), None) => problems.push(format!("ROSSI_ONLY {}/{}", key.0, key.1)),
            (None, Some(_)) => problems.push(format!("ROSSI_MISSING {}/{}", key.0, key.1)),
            (None, None) => unreachable!("key came from one of the maps"),
        }
    }
    if problems.is_empty() {
        Ok(matched)
    } else {
        problems.truncate(5);
        Err(problems.join("; "))
    }
}

/// rossi's EB010 findings keyed by (component, element). Errors when the
/// model can't be loaded (the caller decides whether that's a known gap)
/// or when two findings collide on one key — a collision would otherwise
/// let a masked divergence slip through as a MATCH.
fn rossi_wd(zip: &Path) -> Result<BTreeMap<(String, String), String>, String> {
    let project = Project::from_zip_file(zip).map_err(|e| format!("rossi load: {e}"))?;
    let (_result, model) = build_with_model(&project);
    let mut out = BTreeMap::new();
    for d in wd::run(&project, &model) {
        if d.rule_id != Some(RuleId::WellDefinedness) {
            continue;
        }
        let (component, element) = d.origin.split_once('.').map_or_else(
            || (d.origin.clone(), String::new()),
            |(c, e)| (c.to_string(), e.to_string()),
        );
        let message = d
            .message
            .strip_prefix("Well-definedness condition: ")
            .unwrap_or(&d.message)
            .to_string();
        if let Some(prev) = out.insert((component.clone(), element.clone()), message) {
            return Err(format!(
                "rossi findings collide on {component}/{element}: `{prev}`"
            ));
        }
    }
    Ok(out)
}

/// eventb-checker's EB010 findings keyed by (component, element).
fn rodin_wd(oracle: &str, zip: &Path) -> Result<BTreeMap<(String, String), String>, String> {
    let output = Command::new(oracle)
        .args(["check", "--show-info", "--format", "json"])
        .arg(zip)
        .output()
        .map_err(|e| format!("spawn {oracle}: {e}"))?;
    if output.stdout.is_empty() {
        return Err(format!(
            "no oracle output (status {}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|e| format!("oracle json: {e}"))?;
    let rows = json
        .get("errors")
        .and_then(|v| v.as_array())
        .ok_or("oracle json has no errors array")?;

    let mut out = BTreeMap::new();
    for row in rows {
        if row.get("ruleId").and_then(|v| v.as_str()) != Some("EB010") {
            continue;
        }
        let file = row.get("file").and_then(|v| v.as_str()).unwrap_or("");
        let component = Path::new(file)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let mut element = row
            .get("element")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if element.is_empty() {
            // The variant carries no label attribute in Rodin XML;
            // rossi labels it "vrn".
            element = "vrn".to_string();
        }
        let message = row
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .strip_prefix("Well-definedness condition: ")
            .unwrap_or("")
            .to_string();
        if let Some(prev) = out.insert((component.clone(), element.clone()), message) {
            return Err(format!(
                "oracle findings collide on {component}/{element}: `{prev}`"
            ));
        }
    }
    Ok(out)
}
