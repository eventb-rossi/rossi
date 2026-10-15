//! Compare rossi's EB010 findings with eventb-checker over the model corpus.
//!
//! This ignored gate pins both coverage and byte-exact message text for models
//! both tools can load and statically check. Corpus-flagged unsupported syntax
//! is skipped, as are individual formulas rejected by either checker before WD
//! computation. One exact, audited simplifier-shape difference is tolerated;
//! every other one-sided or byte-level difference fails. Run it with:
//!
//! ```text
//! cargo test -p rossi-build --test wd_oracle_diff -- --ignored --nocapture
//! ```
//!
//! `EVENTB_CHECKER` may override the oracle executable. `EVENTB_CORPUS_DIR`
//! may override the corpus; otherwise the sibling `eventb-models-collection`
//! checkout is used when present.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{
    collect_zips, eventb_checker_bin, load_flags, locate_corpus, oracle_available, workspace_root,
};
use rossi_build::project::discover_projects;
use rossi_build::{RuleId, Severity, build_with_model, wd};

type FindingKey = (String, String);

#[test]
#[ignore = "needs the eventb-checker CLI and a models corpus; run with --ignored"]
fn wd_oracle_diff() {
    let oracle = eventb_checker_bin();
    if !oracle_available(&oracle) {
        eprintln!("SKIP wd_oracle_diff: `{oracle}` is not runnable");
        return;
    }
    let Some(corpus) = corpus_dir() else {
        eprintln!("SKIP wd_oracle_diff: no model corpus found");
        return;
    };
    let zips = collect_zips(&corpus).unwrap_or_default();
    if zips.is_empty() {
        eprintln!(
            "SKIP wd_oracle_diff: no .zip models in {}",
            corpus.display()
        );
        return;
    }

    let flags = load_flags(&corpus.join("model_flags.tsv")).unwrap_or_default();
    let mut matched = 0;
    let mut failures = Vec::new();
    for zip in &zips {
        let name = zip.file_name().and_then(|s| s.to_str()).unwrap_or("?");
        let model = zip.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let load_gap_ok = flags.get(model).is_some_and(|model_flags| {
            model_flags.iter().any(|flag| {
                matches!(
                    flag.as_str(),
                    "defective" | "keyword_identifier" | "unsupported"
                )
            })
        });
        match diff_one(&oracle, zip, load_gap_ok) {
            Ok(Some(count)) => {
                matched += count;
                eprintln!("  OK   {name} ({count} finding(s))");
            }
            Ok(None) => eprintln!("  SKIP {name} (flagged unsupported input)"),
            Err(error) => {
                eprintln!("  FAIL {name}: {error}");
                failures.push(format!("{name}: {error}"));
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

fn corpus_dir() -> Option<PathBuf> {
    locate_corpus().or_else(|| {
        let sibling = workspace_root().join("../eventb-models-collection");
        sibling.is_dir().then_some(sibling)
    })
}

fn diff_one(oracle: &str, zip: &Path, load_gap_ok: bool) -> Result<Option<usize>, String> {
    let mine = match rossi_wd(zip) {
        Ok(findings) => findings,
        Err(_) if load_gap_ok => return Ok(None),
        Err(error) => return Err(error),
    };
    let theirs = eventb_checker_wd(oracle, zip)?;
    let keys: BTreeSet<_> = mine
        .findings
        .keys()
        .chain(theirs.findings.keys())
        .cloned()
        .collect();
    let mut matched = 0;
    let mut problems = Vec::new();

    for key in keys {
        match (mine.findings.get(&key), theirs.findings.get(&key)) {
            (Some(ours), Some(theirs)) if ours == theirs => matched += 1,
            (Some(_), Some(_)) if known_simplifier_gap(&key) => {}
            (Some(ours), Some(theirs)) => problems.push(format!(
                "MISMATCH {}/{}: rossi `{ours}` vs oracle `{theirs}`",
                key.0, key.1
            )),
            (Some(_), None) if theirs.uncheckable.contains(&key) => {}
            (Some(_), None) => problems.push(format!("ROSSI_ONLY {}/{}", key.0, key.1)),
            (None, Some(_)) if mine.cannot_check(&key) => {}
            (None, Some(_)) => problems.push(format!("ROSSI_MISSING {}/{}", key.0, key.1)),
            (None, None) => unreachable!("key came from one of the maps"),
        }
    }

    if problems.is_empty() {
        Ok(Some(matched))
    } else {
        problems.truncate(5);
        Err(problems.join("; "))
    }
}

fn known_simplifier_gap(key: &FindingKey) -> bool {
    // Rossi's current subsumption tree retains a nested implication here;
    // Rodin distributes the same hypotheses into a sibling implication.
    key.0 == "Bazalt/MLSModel.bum" && key.1 == "ExecuteProcess/grd11"
}

struct RossiFindings {
    findings: BTreeMap<FindingKey, String>,
    unchecked_files: BTreeSet<String>,
    uncheckable_elements: BTreeSet<FindingKey>,
}

impl RossiFindings {
    fn cannot_check(&self, key: &FindingKey) -> bool {
        self.unchecked_files.contains(&key.0)
            || self.uncheckable_elements.iter().any(|(file, element)| {
                file == &key.0
                    && (element.is_empty()
                        || element == &key.1
                        || key
                            .1
                            .strip_prefix(element)
                            .is_some_and(|suffix| suffix.starts_with('/')))
            })
    }
}

fn rossi_wd(zip: &Path) -> Result<RossiFindings, String> {
    let bytes = std::fs::read(zip).map_err(|e| format!("rossi load: {e}"))?;
    let source_paths = archive_source_paths(&bytes)?;
    let fallback = zip
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("project");
    let projects = discover_projects(&bytes, fallback).map_err(|e| format!("rossi load: {e}"))?;
    let mut findings = BTreeMap::new();
    let mut unchecked_files = BTreeSet::new();
    let mut uncheckable_elements = BTreeSet::new();

    for discovered in projects {
        let prefix = discovered.prefix.clone();
        let project = discovered.into_project();
        let mut files = BTreeMap::new();
        for component in &project.components {
            let candidates: Vec<_> = source_paths
                .iter()
                .filter(|path| {
                    path.starts_with(&prefix)
                        && Path::new(path).file_name().and_then(|name| name.to_str())
                            == Some(component.filename.as_str())
                })
                .collect();
            let path = match candidates.as_slice() {
                [path] => (*path).clone(),
                [] => format!("{prefix}{}", component.filename),
                _ => {
                    return Err(format!(
                        "rossi source path is ambiguous for {}{}",
                        prefix, component.filename
                    ));
                }
            };
            if let rossi::Component::Machine(machine) = &component.component
                && machine
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.configuration.as_deref())
                    == Some("ch.ethz.eventb.decomposition.mchBase")
            {
                unchecked_files.insert(path.clone());
            }
            if files
                .insert(component.component.name().to_string(), path)
                .is_some()
            {
                return Err(format!(
                    "rossi components collide on {}",
                    component.component.name()
                ));
            }
        }

        let (build, model) = build_with_model(&project);
        for diagnostic in &build.diagnostics {
            if diagnostic.severity != Severity::Error {
                continue;
            }
            let (component, element) = diagnostic.origin.split_once('.').map_or_else(
                || (diagnostic.origin.as_str(), ""),
                |(component, element)| (component, element),
            );
            if let Some(file) = files.get(component) {
                uncheckable_elements.insert((file.clone(), element.replace('.', "/")));
            }
        }

        for diagnostic in wd::run(&project, &model) {
            if diagnostic.rule_id != Some(RuleId::WellDefinedness) {
                continue;
            }
            let (component, element) = diagnostic.origin.split_once('.').map_or_else(
                || (diagnostic.origin.as_str(), ""),
                |(component, element)| (component, element),
            );
            let file = files
                .get(component)
                .ok_or_else(|| format!("rossi diagnostic has unknown component {component}"))?;
            let message = strip_message_prefix(&diagnostic.message).to_string();
            insert_unique(
                &mut findings,
                (file.clone(), element.to_string()),
                message,
                "rossi",
            )?;
        }
    }

    Ok(RossiFindings {
        findings,
        unchecked_files,
        uncheckable_elements,
    })
}

fn archive_source_paths(bytes: &[u8]) -> Result<Vec<String>, String> {
    let reader = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| format!("rossi load: {e}"))?;
    let mut paths = Vec::new();
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|e| format!("rossi load: {e}"))?;
        let path = entry.name().map_err(|e| format!("rossi load: {e}"))?;
        if Path::new(&*path)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension, "buc" | "bum"))
        {
            paths.push(path.to_string());
        }
    }
    Ok(paths)
}

struct OracleFindings {
    findings: BTreeMap<FindingKey, String>,
    uncheckable: BTreeSet<FindingKey>,
}

fn eventb_checker_wd(oracle: &str, zip: &Path) -> Result<OracleFindings, String> {
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
        .and_then(|value| value.as_array())
        .ok_or("oracle json has no errors array")?;
    let mut findings = BTreeMap::new();
    let mut uncheckable = BTreeSet::new();

    for row in rows {
        let rule_id = row.get("ruleId").and_then(|value| value.as_str());
        let file = row
            .get("file")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let file = file.replace('\\', "/");
        let mut element = row
            .get("element")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        if element.is_empty() {
            element = "vrn".to_string();
        }
        let key = (file, element);
        if rule_id != Some("EB010") {
            if rule_id == Some("EB020")
                || row.get("severity").and_then(|value| value.as_str()) == Some("ERROR")
            {
                uncheckable.insert(key);
            }
            continue;
        }
        let message = row
            .get("message")
            .and_then(|value| value.as_str())
            .map(strip_message_prefix)
            .unwrap_or("")
            .to_string();
        insert_unique(&mut findings, key, message, "oracle")?;
    }
    Ok(OracleFindings {
        findings,
        uncheckable,
    })
}

fn strip_message_prefix(message: &str) -> &str {
    message
        .strip_prefix("Well-definedness condition: ")
        .unwrap_or(message)
}

fn insert_unique(
    findings: &mut BTreeMap<FindingKey, String>,
    key: FindingKey,
    message: String,
    side: &str,
) -> Result<(), String> {
    if let Some(previous) = findings.insert(key.clone(), message) {
        return Err(format!(
            "{side} findings collide on {}/{}: `{previous}`",
            key.0, key.1
        ));
    }
    Ok(())
}
