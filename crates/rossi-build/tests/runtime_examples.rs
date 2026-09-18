//! What the runtime-translation pass reports on the bundled example
//! archives, pinned exactly.
//!
//! These are not cleanliness assertions. Every example here is a correct,
//! proof-carrying Event-B model, and most of the findings below are things a
//! modeller has no reason to change — the point of pinning them is that the
//! set only moves when the pass changes, and then a reviewer sees which
//! model moved and why.
//!
//! To regenerate after an intended change, run
//! `cargo run -p rossi-cli -- validate --runtime --show-info
//! crates/rossi/examples/<name>.zip` and read the origins out of the report.

use std::collections::BTreeMap;

use rossi_build::{Project, build_with_model};

/// The `(code, origin)` pairs the pass reports for one example, sorted.
fn findings(name: &str) -> Vec<(&'static str, String)> {
    let path = format!("../rossi/examples/{name}.zip");
    let project = Project::from_zip_file(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let (_, model) = build_with_model(&project);
    let mut findings: Vec<(&'static str, String)> = rossi_build::runtime::run(&project, &model)
        .into_iter()
        .map(|diag| {
            (
                diag.rule_id.expect("every finding is tagged").code(),
                diag.origin,
            )
        })
        .collect();
    findings.sort();
    findings
}

fn assert_findings(name: &str, expected: &[(&str, &str)]) {
    let actual = findings(name);
    let actual: Vec<(&str, &str)> = actual
        .iter()
        .map(|(code, origin)| (*code, origin.as_str()))
        .collect();
    assert_eq!(actual, expected, "{name}");
}

/// The leaf is deterministic: the abstract levels choose `k` from `dom(f)`
/// and it computes the midpoint, so nothing about its actions is reported.
/// Its context is the design document's worked example of constants that
/// determine nothing, and both leaf search events read the midpoint one
/// action overwrites.
#[test]
fn binary_search() {
    assert_findings(
        "binary-search",
        &[
            ("EB108", "C0.f"),
            ("EB108", "C0.n"),
            ("EB108", "C0.v"),
            ("EB110", "M3.search_dec/act2"),
            ("EB110", "M3.search_inc/act2"),
        ],
    );
}

/// The `partition` of `COLOURS` settles finiteness, the constants' values
/// and their distinctness at once, so the only findings are the parameter no
/// guard fixes — enumerable, because the partition bounds its type — and the
/// two booleans `M2` data-refines into colours.
#[test]
fn traffic_light() {
    assert_findings(
        "traffic-light",
        &[
            ("EB103", "M2.set_cars_colours/new_value_colours"),
            ("EB112", "M2.cars_go"),
            ("EB112", "M2.peds_go"),
        ],
    );
}

/// One context bounds its carrier set with a `partition` and another with an
/// enumeration plus an inequality; both are accepted, so only the unbounded
/// natural number is left.
#[test]
fn cars_on_bridge() {
    assert_findings(
        "cars-on-bridge",
        &[("EB107", "C0.cars_limit"), ("EB108", "C0.cars_limit")],
    );
}

/// The richest of the four. Every event parameter is free, `delete_file`'s
/// fifth guard quantifies a name restricted only by an inequality,
/// `create_file` subtracts from a carrier set nothing bounds, and four
/// guards apply a map to a file no earlier guard puts in its domain.
#[test]
fn file_system() {
    assert_findings(
        "file-system",
        &[
            ("EB103", "M0.create_file/file"),
            ("EB103", "M0.create_file/name"),
            ("EB103", "M0.create_file/parent"),
            ("EB103", "M0.create_folder/folder"),
            ("EB103", "M0.create_folder/name"),
            ("EB103", "M0.create_folder/parent"),
            ("EB103", "M0.create_hard_link/file"),
            ("EB103", "M0.create_hard_link/name"),
            ("EB103", "M0.create_hard_link/parent"),
            ("EB103", "M0.delete_file/file"),
            ("EB103", "M0.delete_file/name"),
            ("EB103", "M0.delete_file/parent"),
            ("EB103", "M0.delete_hard_link/file"),
            ("EB103", "M0.delete_hard_link/name"),
            ("EB103", "M0.delete_hard_link/parent"),
            ("EB103", "M0.rename_file/file"),
            ("EB103", "M0.rename_file/name"),
            ("EB103", "M0.rename_file/oldName"),
            ("EB103", "M0.rename_file/parent"),
            ("EB104", "M0.delete_file/grd5"),
            ("EB106", "C0.FilesType"),
            ("EB108", "C0.Root"),
            ("EB111", "M0.delete_file/grd4"),
            ("EB111", "M0.delete_file/grd5"),
            ("EB111", "M0.delete_file/grd7"),
            ("EB111", "M0.delete_hard_link/grd4"),
            ("EB111", "M0.delete_hard_link/grd5"),
        ],
    );
}

/// The one large model in the corpus, pinned by count rather than by origin:
/// listing 434 findings would be unreadable and would say less than the
/// shape does. The shape is the point — a model of this size leans on free
/// parameters and function application throughout, which is what a
/// translation of it would have to be given or be careful about.
#[test]
fn base_model_counts() {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for (code, _) in findings("base-model") {
        *counts.entry(code).or_default() += 1;
    }
    assert_eq!(
        counts,
        BTreeMap::from([
            ("EB103", 149),
            ("EB104", 1),
            ("EB108", 4),
            ("EB109", 28),
            ("EB110", 2),
            ("EB111", 250),
        ])
    );
}

/// Warnings are what `--deny-warnings` would gate on, so they are worth
/// separating from the advisory bulk. Only two rules reach that severity on
/// the bundled models, and one example raises nothing at all: a `partition`
/// is enough to keep a whole model clear of them.
#[test]
fn warnings_are_the_small_half() {
    let warnings = |name: &str| -> Vec<(&'static str, String)> {
        let path = format!("../rossi/examples/{name}.zip");
        let project = Project::from_zip_file(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let (_, model) = build_with_model(&project);
        let mut found: Vec<(&'static str, String)> = rossi_build::runtime::run(&project, &model)
            .into_iter()
            .filter(|diag| diag.severity == rossi_build::Severity::Warning)
            .map(|diag| (diag.rule_id.expect("tagged").code(), diag.origin))
            .collect();
        found.sort();
        found
    };

    assert!(warnings("traffic-light").is_empty());
    assert_eq!(
        warnings("binary-search"),
        vec![
            ("EB108", "C0.f".to_string()),
            ("EB108", "C0.n".to_string()),
            ("EB108", "C0.v".to_string()),
        ]
    );
    assert_eq!(
        warnings("cars-on-bridge"),
        vec![("EB108", "C0.cars_limit".to_string())]
    );
    assert_eq!(
        warnings("file-system"),
        vec![
            ("EB104", "M0.delete_file/grd5".to_string()),
            ("EB106", "C0.FilesType".to_string()),
            ("EB108", "C0.Root".to_string()),
        ]
    );
}
