//! Corpus integration test — semantic diff against every model in
//! an external Event-B model corpus.
//!
//! By default ignored (the corpus lives outside the repo). Run locally:
//!
//!   cargo test -p rossi-build --test corpus -- --ignored --nocapture
//!
//! Set `EVENTB_CORPUS_DIR=/some/path` to choose the corpus location.
//! Relative paths are resolved from the workspace root.
//!
//! A TSV matrix is written to `target/rossi-build-corpus.tsv`:
//!
//!   model<TAB>file<TAB>semantic<TAB>byte_exact<TAB>diag
//!
//! The test fails if any file fails semantic comparison. Byte-exact is
//! surfaced as a metric — not a gate.

use std::io::{Read, Write};
use std::path::PathBuf;

use rossi_build::{Project, build, sc_view::ScView};

fn corpus_dir() -> Option<PathBuf> {
    env_path("EVENTB_CORPUS_DIR").filter(|p| p.is_dir())
}

fn env_path(var: &str) -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var(var).ok()?);
    Some(if path.is_absolute() {
        path
    } else {
        workspace_root().join(path)
    })
}

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

/// One row of the TSV report.
struct Row {
    model: String,
    filename: String,
    semantic: Outcome,
    byte_exact: Outcome,
    diag: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Pass,
    Fail,
    Skip,
}

impl Outcome {
    fn tsv(&self) -> &'static str {
        match self {
            Outcome::Pass => "PASS",
            Outcome::Fail => "FAIL",
            Outcome::Skip => "SKIP",
        }
    }
}

#[test]
#[ignore]
fn corpus_semantic_equivalence() {
    let Some(dir) = corpus_dir() else {
        eprintln!("EVENTB_CORPUS_DIR is not set or is not a directory — nothing to do");
        return;
    };
    let mut rows = Vec::<Row>::new();
    let mut zip_count = 0usize;
    let mut load_failures = 0usize;

    for entry in std::fs::read_dir(&dir).expect("read corpus") {
        let entry = entry.expect("read dir entry");
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !name.ends_with(".zip") {
            continue;
        }
        zip_count += 1;
        match process_zip(&path) {
            Ok(model_rows) => rows.extend(model_rows),
            Err(e) => {
                load_failures += 1;
                rows.push(Row {
                    model: name.trim_end_matches(".zip").to_string(),
                    filename: String::new(),
                    semantic: Outcome::Skip,
                    byte_exact: Outcome::Skip,
                    diag: format!("load: {e}"),
                });
            }
        }
    }

    write_report(&rows);

    // Summary.
    let total = rows.iter().filter(|r| r.semantic != Outcome::Skip).count();
    let pass = rows.iter().filter(|r| r.semantic == Outcome::Pass).count();
    let byte_exact = rows
        .iter()
        .filter(|r| r.byte_exact == Outcome::Pass)
        .count();
    println!(
        "corpus: {zip_count} archives, {load_failures} load-failed, \
         {pass}/{total} files semantically equal, \
         {byte_exact}/{total} byte-exact"
    );

    let failures: Vec<&Row> = rows
        .iter()
        .filter(|r| r.semantic == Outcome::Fail)
        .collect();
    if !failures.is_empty() {
        for f in failures.iter().take(20) {
            eprintln!("  FAIL  {} / {} — {}", f.model, f.filename, f.diag);
        }
        panic!(
            "{} / {} files failed semantic equivalence (first 20 shown above)",
            failures.len(),
            total
        );
    }
}

fn process_zip(path: &std::path::Path) -> Result<Vec<Row>, Box<dyn std::error::Error>> {
    let model = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("?")
        .to_string();

    // Read Rodin's bcc/bcm into a basename-indexed map. Use BTreeMap so
    // project-name extraction below is deterministic across runs.
    let data = std::fs::read(path)?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&data))?;
    let mut rodin_files: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for i in 0..archive.len() {
        let mut e = archive.by_index(i)?;
        let n = e.name().to_string();
        if n.ends_with(".bcc") || n.ends_with(".bcm") {
            let base = basename(&n).to_string();
            let mut s = String::new();
            e.read_to_string(&mut s)?;
            rodin_files.insert(base, s);
        }
    }

    // Derive Rodin's project name from any .bcc source URI (deterministic:
    // BTreeMap iteration is sorted by basename).
    let project_name = rodin_files
        .values()
        .find_map(|xml| {
            let marker = "org.eventb.core.source=\"/";
            let i = xml.find(marker)?;
            let start = i + marker.len();
            let rest = &xml[start..];
            let slash = rest.find('/')?;
            Some(rest[..slash].to_string())
        })
        .unwrap_or_else(|| model.clone());

    let mut project = Project::from_zip_bytes(project_name, &data)?;
    let _ = &mut project; // keep builder-style option open

    let result = build(&project);

    let mut rows = Vec::new();
    for f in &result.files {
        let rodin = match rodin_files.get(&f.filename) {
            Some(r) => r.clone(),
            None => {
                rows.push(Row {
                    model: model.clone(),
                    filename: f.filename.clone(),
                    semantic: Outcome::Skip,
                    byte_exact: Outcome::Skip,
                    diag: "no Rodin reference".to_string(),
                });
                continue;
            }
        };

        let byte_exact = if rodin.trim_end() == f.contents.trim_end() {
            Outcome::Pass
        } else {
            Outcome::Fail
        };

        let (semantic, diag) = match (ScView::from_xml(&f.contents), ScView::from_xml(&rodin)) {
            (Ok(ours), Ok(theirs)) => {
                if ours == theirs {
                    (Outcome::Pass, String::new())
                } else {
                    (Outcome::Fail, describe_diff(&ours, &theirs))
                }
            }
            (Err(e), _) => (Outcome::Fail, format!("our view parse: {e}")),
            (_, Err(e)) => (Outcome::Skip, format!("rodin view parse: {e}")),
        };

        rows.push(Row {
            model: model.clone(),
            filename: f.filename.clone(),
            semantic,
            byte_exact,
            diag,
        });
    }
    Ok(rows)
}

fn basename(p: &str) -> &str {
    p.rsplit_once('/').map(|(_, b)| b).unwrap_or(p)
}

/// Produce a short human diff between two views.
fn describe_diff(ours: &ScView, theirs: &ScView) -> String {
    let mut parts = Vec::new();
    if ours.kind != theirs.kind {
        parts.push(format!("kind {:?} != {:?}", ours.kind, theirs.kind));
    }
    if ours.accurate != theirs.accurate {
        parts.push(format!("accurate {} != {}", ours.accurate, theirs.accurate));
    }
    diff_sets(
        "carrier_sets",
        ours.carrier_sets.keys(),
        theirs.carrier_sets.keys(),
        &mut parts,
    );
    diff_sets(
        "constants",
        ours.constants.keys(),
        theirs.constants.keys(),
        &mut parts,
    );
    diff_sets(
        "axioms",
        ours.axioms.keys(),
        theirs.axioms.keys(),
        &mut parts,
    );
    diff_sets(
        "invariants",
        ours.invariants.keys(),
        theirs.invariants.keys(),
        &mut parts,
    );
    diff_sets(
        "variables",
        ours.variables.keys(),
        theirs.variables.keys(),
        &mut parts,
    );
    diff_sets(
        "events",
        ours.events.keys(),
        theirs.events.keys(),
        &mut parts,
    );
    // Per-key content checks for common collections.
    for (k, o) in &ours.carrier_sets {
        if let Some(t) = theirs.carrier_sets.get(k)
            && o != t
        {
            parts.push(format!(
                "carrier_set[{k}]: type {} vs {}",
                o.type_str, t.type_str
            ));
        }
    }
    for (k, o) in &ours.constants {
        if let Some(t) = theirs.constants.get(k)
            && o != t
        {
            parts.push(format!(
                "constant[{k}]: type {} vs {}",
                o.type_str, t.type_str
            ));
        }
    }
    for k in ours.axioms.keys() {
        if let Some(t) = theirs.axioms.get(k)
            && (ours.axioms[k].theorem != t.theorem || ours.axioms[k].predicate != t.predicate)
        {
            parts.push(format!("axiom[{k}] differs"));
        }
    }
    for k in ours.invariants.keys() {
        if let Some(t) = theirs.invariants.get(k)
            && (ours.invariants[k].theorem != t.theorem
                || ours.invariants[k].predicate != t.predicate)
        {
            parts.push(format!("invariant[{k}] differs"));
        }
    }
    // Variable types (key set diff happens above; this is content diff).
    for (name, o) in &ours.variables {
        if let Some(t) = theirs.variables.get(name)
            && o.type_str != t.type_str
        {
            parts.push(format!(
                "variable[{name}] type {} vs {}",
                o.type_str, t.type_str
            ));
        }
    }

    for k in ours.events.keys() {
        if let Some(t) = theirs.events.get(k) {
            let o = &ours.events[k];
            if o.accurate != t.accurate {
                parts.push(format!(
                    "event[{k}] accurate {} != {}",
                    o.accurate, t.accurate
                ));
            }
            if o.convergence != t.convergence {
                parts.push(format!(
                    "event[{k}] convergence {:?} != {:?}",
                    o.convergence, t.convergence
                ));
            }
            if o.extended != t.extended {
                parts.push(format!(
                    "event[{k}] extended {} != {}",
                    o.extended, t.extended
                ));
            }

            // Parameters: keys + types.
            let our_params: std::collections::BTreeSet<_> = o.parameters.keys().collect();
            let their_params: std::collections::BTreeSet<_> = t.parameters.keys().collect();
            if our_params != their_params {
                parts.push(format!(
                    "event[{k}].parameters {:?} vs {:?}",
                    our_params, their_params
                ));
            } else {
                for name in &our_params {
                    let ours_ty = o.parameters.get(*name);
                    let theirs_ty = t.parameters.get(*name);
                    if ours_ty != theirs_ty {
                        parts.push(format!(
                            "event[{k}].parameter[{name}] type {:?} vs {:?}",
                            ours_ty, theirs_ty
                        ));
                    }
                }
            }

            // Guards: keys + content.
            let our_guards: std::collections::BTreeSet<_> = o.guards.keys().collect();
            let their_guards: std::collections::BTreeSet<_> = t.guards.keys().collect();
            if our_guards != their_guards {
                parts.push(format!(
                    "event[{k}].guards {:?} vs {:?}",
                    our_guards, their_guards
                ));
            } else {
                for src in &our_guards {
                    let og = &o.guards[*src];
                    let tg = &t.guards[*src];
                    if og.theorem != tg.theorem || og.predicate != tg.predicate {
                        parts.push(format!("event[{k}].guard[{}] differs", og.label));
                    }
                }
            }

            // Actions: keys + content.
            let our_actions: std::collections::BTreeSet<_> = o.actions.keys().collect();
            let their_actions: std::collections::BTreeSet<_> = t.actions.keys().collect();
            if our_actions != their_actions {
                parts.push(format!(
                    "event[{k}].actions {:?} vs {:?}",
                    our_actions, their_actions
                ));
            } else {
                for src in &our_actions {
                    if o.actions[*src].action != t.actions[*src].action {
                        parts.push(format!(
                            "event[{k}].action[{}] differs",
                            o.actions[*src].label
                        ));
                    }
                }
            }

            // Witnesses: keys + content.
            let our_wits: std::collections::BTreeSet<_> = o.witnesses.keys().collect();
            let their_wits: std::collections::BTreeSet<_> = t.witnesses.keys().collect();
            if our_wits != their_wits {
                parts.push(format!(
                    "event[{k}].witnesses {:?} vs {:?}",
                    our_wits, their_wits
                ));
            } else {
                for src in &our_wits {
                    if o.witnesses[*src].predicate != t.witnesses[*src].predicate {
                        parts.push(format!(
                            "event[{k}].witness[{}] differs",
                            o.witnesses[*src].label
                        ));
                    }
                }
            }

            // scRefinesEvent: keys + scTarget.
            let our_re: std::collections::BTreeSet<_> = o.refines_events.keys().collect();
            let their_re: std::collections::BTreeSet<_> = t.refines_events.keys().collect();
            if our_re != their_re {
                parts.push(format!(
                    "event[{k}].refines_events {:?} vs {:?}",
                    our_re, their_re
                ));
            } else {
                for src in &our_re {
                    if o.refines_events[*src] != t.refines_events[*src] {
                        parts.push(format!(
                            "event[{k}].refines_event[{src}] target {} vs {}",
                            o.refines_events[*src], t.refines_events[*src]
                        ));
                    }
                }
            }
        }
    }
    if ours.variant != theirs.variant {
        parts.push(format!(
            "variant {:?} vs {:?}",
            ours.variant, theirs.variant
        ));
    }
    if parts.is_empty() {
        "views differ in unknown way".to_string()
    } else {
        parts.join("; ")
    }
}

fn diff_sets<'a, A, B>(label: &str, a: A, b: B, out: &mut Vec<String>)
where
    A: IntoIterator<Item = &'a String>,
    B: IntoIterator<Item = &'a String>,
{
    use std::collections::HashSet;
    let a: HashSet<&str> = a.into_iter().map(String::as_str).collect();
    let b: HashSet<&str> = b.into_iter().map(String::as_str).collect();
    let only_a: Vec<&&str> = a.difference(&b).collect();
    let only_b: Vec<&&str> = b.difference(&a).collect();
    if !only_a.is_empty() {
        out.push(format!("{label} extra {:?}", only_a));
    }
    if !only_b.is_empty() {
        out.push(format!("{label} missing {:?}", only_b));
    }
}

fn write_report(rows: &[Row]) {
    let mut path = workspace_root();
    path.push("target");
    let _ = std::fs::create_dir_all(&path);
    path.push("rossi-build-corpus.tsv");

    let mut f = match std::fs::File::create(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("could not write {}: {e}", path.display());
            return;
        }
    };
    let _ = writeln!(f, "model\tfile\tsemantic\tbyte_exact\tdiag");
    for r in rows {
        let _ = writeln!(
            f,
            "{}\t{}\t{}\t{}\t{}",
            r.model,
            r.filename,
            r.semantic.tsv(),
            r.byte_exact.tsv(),
            r.diag
        );
    }
    println!("report: {}", path.display());
}
