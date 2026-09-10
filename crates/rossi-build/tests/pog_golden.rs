//! Golden proof obligations — the `.bpo` files rossi generates for two
//! in-repo example archives, locked against the reference output for
//! the same sources.
//!
//! Every other reference comparison (`pog_corpus`, `rodin_corpus`) is
//! `#[ignore]`d because it needs the external model corpus or a
//! toolchain runtime. This one carries its reference in
//! `tests/fixtures/pog_golden/`,
//! so it runs under a plain `cargo test` and is the only proof-obligation
//! gate CI executes. See that directory's `README.md` for how the reference
//! was produced and how to regenerate it after a deliberate POG change.
//!
//! The comparison runs at two levels. The normalized [`PoView`] diff reports
//! a divergence in the obligations themselves in readable terms; the verbatim
//! comparison then catches a change in attribute order, element order or
//! spacing that the semantic view forgives.

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use rossi_build::po_view::PoView;
use rossi_build::pog::reconcile::preserve_child_order;
use rossi_build::project::discover_projects;

/// The example archives locked here. Their components are whatever the
/// fixture directory holds, so the checked-in references stay the single
/// source of truth for what gets compared.
const MODELS: &[&str] = &["traffic-light", "binary-search"];

/// Problems reported before truncation.
const MAX_PROBLEMS: usize = 10;

fn fixtures_dir(model: &str) -> PathBuf {
    common::workspace_root()
        .join("crates/rossi-build/tests/fixtures/pog_golden")
        .join(model)
}

/// Build one example archive and return its generated `.bpo` files keyed by
/// component name. This is the pure build path — no reconciliation, so every
/// stamp is 0, exactly as in a fresh reference build.
fn generated_bpos(model: &str) -> BTreeMap<String, String> {
    let zip = common::workspace_root()
        .join("crates/rossi/examples")
        .join(format!("{model}.zip"));
    let bytes = std::fs::read(&zip).unwrap_or_else(|e| panic!("read {}: {e}", zip.display()));
    let projects =
        discover_projects(&bytes, model).unwrap_or_else(|e| panic!("{model}: discovery: {e}"));

    let mut out = BTreeMap::new();
    for dp in projects {
        let build = rossi_build::build(&dp.into_project());
        assert!(
            build.is_ok(),
            "{model}: build reported errors: {:?}",
            build.diagnostics
        );
        for file in build.files {
            if let Some(component) = file.filename.strip_suffix(".bpo") {
                out.insert(component.to_string(), file.contents);
            }
        }
    }
    out
}

/// The checked-in reference `.bpo` files, keyed by component name.
fn reference_bpos(model: &str) -> BTreeMap<String, String> {
    let dir = fixtures_dir(model);
    let entries = std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
    let mut out = BTreeMap::new();
    for entry in entries {
        let path = entry.expect("fixture entry").path();
        if path.extension().is_some_and(|e| e == "bpo") {
            let component = path
                .file_stem()
                .expect("stem")
                .to_string_lossy()
                .into_owned();
            let contents = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            out.insert(component, contents);
        }
    }
    assert!(!out.is_empty(), "{model}: no reference .bpo fixtures");
    out
}

#[test]
fn generated_obligations_match_rodin() {
    let mut problems = Vec::new();
    let mut verbatim = Vec::new();
    for model in MODELS {
        let ours = generated_bpos(model);
        let theirs = reference_bpos(model);
        assert_eq!(
            ours.keys().collect::<Vec<_>>(),
            theirs.keys().collect::<Vec<_>>(),
            "{model}: the generated components differ from the reference set"
        );
        for (component, reference) in &theirs {
            let file = format!("{model}/{component}.bpo");
            let generated = &ours[component];
            let our_view =
                PoView::from_xml(generated).unwrap_or_else(|e| panic!("{file}: parse ours: {e}"));
            let their_view = PoView::from_xml(reference)
                .unwrap_or_else(|e| panic!("{file}: parse reference: {e}"));
            common::diff_po_views(&file, &their_view, &our_view, MAX_PROBLEMS, &mut problems);
            verbatim.push((
                file,
                strip_indent(reference),
                strip_indent(&preserve_child_order(generated, reference)),
            ));
        }
    }
    problems.truncate(MAX_PROBLEMS);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    for (file, theirs, ours) in verbatim {
        assert_eq!(theirs, ours, "{file} diverges from the reference");
    }
}

/// Erase the one difference between the reference `.bpo` and ours that the
/// verbatim comparison is not meant to police: the reference writer
/// indents four spaces per level, rossi's emitter writes each element flush
/// left. Every other byte is compared as written, the ascribed generic
/// atoms and the identifier order included: Rodin's static checker and
/// generator write identifiers in the iteration order of their hash
/// tables, and rossi reproduces it the only way it can, by keeping the
/// order of the previous copy of the file, which is what the reference
/// stands in for above. Rodin re-stamps a sequent over any character that
/// differs, so the reference decides everything.
///
/// Line splitting also erases a trailing-newline or CRLF difference, neither
/// of which either writer produces.
fn strip_indent(xml: &str) -> String {
    xml.lines()
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n")
}
