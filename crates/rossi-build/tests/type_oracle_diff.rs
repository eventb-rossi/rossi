//! Type-inference oracle diff — compares rossi-build's inferred types against
//! Rodin's, construct by construct.
//!
//! Both sides are driven from the same `.eventb` snippets in
//! `tests/fixtures/type_propagation/`. rossi's types come from
//! `rossi_build::build` (read back via [`ScView`]); Rodin's come from the
//! `eventb-checker info --types --format json` oracle, which is built on the
//! real Rodin AST library. Each declared constant / variable / event parameter is
//! aligned by name and classified.
//!
//! Ignored by default (needs the `eventb-checker` CLI). Run:
//!
//!   cargo test -p rossi-build --test type_oracle_diff -- --ignored --nocapture
//!
//! The harness runs `eventb-checker` from `PATH`; set `EVENTB_CHECKER` to
//! override it with a specific binary. A TSV matrix is written to
//! `target/type-oracle-diff.tsv`:
//!
//!   snippet<TAB>component<TAB>identifier<TAB>class<TAB>rossi<TAB>rodin
//!
//! Classes:
//!   - MATCH         — both infer the same type
//!   - MISMATCH      — both type it but differ (wrong propagation)
//!   - ROSSI_MISSING — Rodin types it, rossi does not (incomplete inference)
//!   - ROSSI_ONLY    — rossi types it, Rodin does not
//!
//! This is a CATALOG, not a gate: discrepancies do not fail the test. Only
//! harness errors (parse/build/oracle failures) are surfaced as ERROR rows and
//! fail the run, so a broken snippet or oracle can't masquerade as "all match".

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use rossi::{NamedComponent, component_filename, parse_components, to_zip};
use rossi_build::sc_view::{RootKind, ScView};
use rossi_build::{Project, build};

/// (component, scoped-identifier) -> canonical type string. Event parameters
/// are scoped as `"<event>/<param>"`; constants and variables use their bare
/// name.
type TypeMap = BTreeMap<(String, String), String>;

struct Row {
    snippet: String,
    component: String,
    identifier: String,
    class: &'static str,
    rossi: String,
    rodin: String,
}

#[test]
#[ignore = "needs the eventb-checker CLI; run with --ignored"]
fn type_oracle_diff() {
    let oracle = eventb_checker_bin();
    if !oracle_available(&oracle) {
        eprintln!(
            "SKIP type_oracle_diff: `{oracle}` not runnable. Install the eventb-checker CLI \
             or set EVENTB_CHECKER to its path."
        );
        return;
    }

    let dir = fixtures_dir();
    let snippets = collect_eventb(&dir);
    if snippets.is_empty() {
        eprintln!(
            "SKIP type_oracle_diff: no .eventb fixtures in {}",
            dir.display()
        );
        return;
    }
    eprintln!("oracle: {oracle}");
    eprintln!(
        "fixtures: {} snippet(s) in {}",
        snippets.len(),
        dir.display()
    );

    let mut rows = Vec::new();
    for snippet in &snippets {
        let name = snippet
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        let src = match std::fs::read_to_string(snippet) {
            Ok(s) => s,
            Err(e) => {
                rows.push(error_row(&name, format!("read: {e}")));
                continue;
            }
        };
        // The oracle is the authority: if Rodin can't evaluate the snippet, it
        // is malformed Event-B — that's a harness ERROR (fails the run). If only
        // rossi fails, that's a real rossi gap, recorded but non-fatal.
        let rodin = match rodin_types(&oracle, snippet) {
            Ok(r) => r,
            Err(e) => {
                rows.push(error_row(&name, format!("rodin oracle: {e}")));
                continue;
            }
        };
        match rossi_types(&name, &src) {
            Ok(rossi) => classify(&name, &rossi, &rodin, &mut rows),
            Err(e) => {
                rows.push(Row {
                    snippet: name.clone(),
                    component: String::new(),
                    identifier: String::new(),
                    class: "ROSSI_BUILD_FAIL",
                    rossi: e,
                    rodin: String::new(),
                });
                // Surface every type Rodin inferred as a rossi gap.
                classify(&name, &TypeMap::new(), &rodin, &mut rows);
            }
        }
    }

    write_tsv(&rows);
    let report = target_dir().join("type-oracle-diff.tsv");
    print_summary(&rows, &report);

    let errors: Vec<&Row> = rows.iter().filter(|r| r.class == "ERROR").collect();
    assert!(
        errors.is_empty(),
        "{} snippet(s) failed to evaluate (see ERROR rows in {})",
        errors.len(),
        report.display(),
    );
}

/// Build the snippet with rossi-build and read back each declared identifier's
/// inferred type from the emitted `.bcc`/`.bcm`.
fn rossi_types(name: &str, src: &str) -> Result<TypeMap, String> {
    let components = parse_components(src).map_err(|e| e.to_string())?;
    let named: Vec<NamedComponent> = components
        .into_iter()
        .map(|c| NamedComponent {
            filename: component_filename(&c),
            component: c,
        })
        .collect();
    let bytes = to_zip(&named).map_err(|e| e.to_string())?;
    let project = Project::from_zip_bytes(name.to_string(), &bytes).map_err(|e| e.to_string())?;
    let result = build(&project);

    let mut map = TypeMap::new();
    for file in &result.files {
        let component = strip_ext(&file.filename).to_string();
        let view = ScView::from_xml(&file.contents).map_err(|e| e.to_string())?;
        match view.kind {
            RootKind::Context => {
                for (n, row) in &view.constants {
                    map.insert((component.clone(), n.clone()), row.type_str.clone());
                }
            }
            RootKind::Machine => {
                for (n, row) in &view.variables {
                    map.insert((component.clone(), n.clone()), row.type_str.clone());
                }
                for (event, row) in &view.events {
                    for (param, ty) in &row.parameters {
                        map.insert((component.clone(), format!("{event}/{param}")), ty.clone());
                    }
                }
            }
            RootKind::Unknown => {}
        }
    }
    Ok(map)
}

/// Run the Rodin oracle on the same snippet and parse its `info --types` JSON.
fn rodin_types(oracle: &str, snippet: &Path) -> Result<TypeMap, String> {
    let output = Command::new(oracle)
        .arg("info")
        .arg("--types")
        .arg("--format")
        .arg("json")
        .arg(snippet)
        .output()
        .map_err(|e| format!("spawn {oracle}: {e}"))?;
    if output.stdout.is_empty() {
        return Err(format!(
            "no output (status {}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("json: {e}: {}", String::from_utf8_lossy(&output.stdout)))?;

    // `info --types` nests the type map under a top-level "types" key.
    let types = json.get("types").ok_or_else(|| {
        format!(
            "missing \"types\" in oracle output: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })?;

    let mut map = TypeMap::new();
    if let Some(contexts) = types.get("contexts").and_then(|v| v.as_object()) {
        for (ctx, constants) in contexts {
            if let Some(obj) = constants.as_object() {
                for (id, ty) in obj {
                    if let Some(s) = ty.as_str() {
                        map.insert((ctx.clone(), id.clone()), s.to_string());
                    }
                }
            }
        }
    }
    if let Some(machines) = types.get("machines").and_then(|v| v.as_object()) {
        for (machine, body) in machines {
            if let Some(vars) = body.get("variables").and_then(|v| v.as_object()) {
                for (id, ty) in vars {
                    if let Some(s) = ty.as_str() {
                        map.insert((machine.clone(), id.clone()), s.to_string());
                    }
                }
            }
            if let Some(events) = body.get("events").and_then(|v| v.as_object()) {
                for (event, params) in events {
                    if let Some(obj) = params.as_object() {
                        for (param, ty) in obj {
                            if let Some(s) = ty.as_str() {
                                map.insert(
                                    (machine.clone(), format!("{event}/{param}")),
                                    s.to_string(),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(map)
}

fn classify(snippet: &str, rossi: &TypeMap, rodin: &TypeMap, rows: &mut Vec<Row>) {
    let keys: BTreeSet<&(String, String)> = rossi.keys().chain(rodin.keys()).collect();
    for key in keys {
        let (class, rs, rd) = match (rossi.get(key), rodin.get(key)) {
            (Some(a), Some(b)) if normalize(a) == normalize(b) => ("MATCH", a.clone(), b.clone()),
            (Some(a), Some(b)) => ("MISMATCH", a.clone(), b.clone()),
            (None, Some(b)) => ("ROSSI_MISSING", String::new(), b.clone()),
            (Some(a), None) => ("ROSSI_ONLY", a.clone(), String::new()),
            (None, None) => continue,
        };
        rows.push(Row {
            snippet: snippet.to_string(),
            component: key.0.clone(),
            identifier: key.1.clone(),
            class,
            rossi: rs,
            rodin: rd,
        });
    }
}

/// Canonical type strings from both sides use the same alphabet (ℙ, ×, ℤ,
/// BOOL, given-set names); strip whitespace so incidental spacing never reads
/// as a mismatch.
fn normalize(ty: &str) -> String {
    ty.chars().filter(|c| !c.is_whitespace()).collect()
}

fn error_row(snippet: &str, message: String) -> Row {
    Row {
        snippet: snippet.to_string(),
        component: String::new(),
        identifier: String::new(),
        class: "ERROR",
        rossi: String::new(),
        rodin: message,
    }
}

fn print_summary(rows: &[Row], report: &Path) {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for row in rows {
        *counts.entry(row.class).or_default() += 1;
    }
    eprintln!("\n=== type-oracle-diff summary ===");
    for (class, n) in &counts {
        eprintln!("  {class:<14} {n}");
    }
    // The discrepancies are the point — list them inline for quick scanning.
    for row in rows
        .iter()
        .filter(|r| matches!(r.class, "MISMATCH" | "ROSSI_MISSING" | "ROSSI_ONLY"))
    {
        eprintln!(
            "  {:<13} {}/{}::{}  rossi={:?} rodin={:?}",
            row.class, row.snippet, row.component, row.identifier, row.rossi, row.rodin
        );
    }
    for row in rows.iter().filter(|r| r.class == "ROSSI_BUILD_FAIL") {
        eprintln!("  ROSSI_BUILD_FAIL {}: {}", row.snippet, row.rossi);
    }
    for row in rows.iter().filter(|r| r.class == "ERROR") {
        eprintln!("  ERROR         {}: {}", row.snippet, row.rodin);
    }
    eprintln!("report: {}", report.display());
}

fn write_tsv(rows: &[Row]) {
    use std::fmt::Write as _;
    let mut out = String::from("snippet\tcomponent\tidentifier\tclass\trossi\trodin\n");
    for row in rows {
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}",
            row.snippet, row.component, row.identifier, row.class, row.rossi, row.rodin
        );
    }
    let path = target_dir().join("type-oracle-diff.tsv");
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let _ = std::fs::write(&path, out);
}

fn strip_ext(filename: &str) -> &str {
    filename
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(filename)
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/type_propagation")
}

fn target_dir() -> PathBuf {
    // crates/rossi-build -> workspace root -> target
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.join("target")
}

fn collect_eventb(dir: &Path) -> Vec<PathBuf> {
    let mut snippets: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("eventb"))
        .collect();
    snippets.sort();
    snippets
}

/// The `eventb-checker` command to run: `EVENTB_CHECKER` if set, else the CLI
/// resolved from `PATH`.
fn eventb_checker_bin() -> String {
    std::env::var("EVENTB_CHECKER").unwrap_or_else(|_| "eventb-checker".to_string())
}

/// Whether the oracle CLI is runnable (`<oracle> --version` succeeds).
fn oracle_available(oracle: &str) -> bool {
    Command::new(oracle)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
