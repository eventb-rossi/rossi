//! The dump document, locked against a checked-in reference.
//!
//! Unlike the proof-obligation goldens, which compare rossi against Rodin's
//! own output, nothing else produces this format, so these lock rossi against
//! itself. They therefore say nothing about whether the format is right; that
//! is what `dump_model.rs` and `dump_schema.rs` are for. What they catch is
//! drift: any change to a field, an order, a name or a position shows up here
//! as a diff, so a change to the format is always a deliberate one with the
//! reference updated in the same commit.
//!
//! See `tests/fixtures/dump_golden/README.md` for how to regenerate them.
#![cfg(feature = "serde")]

use std::path::PathBuf;

use rossi_build::dump;
use rossi_build::project::{Project, discover_projects};

mod common;

/// The text models locked here. Shared with the other dump tests, so the
/// three binaries cannot drift on which examples they cover.
use common::DUMP_MODELS as TEXT_MODELS;

/// One archive is enough, and it is the one that exercises the most: an
/// extended event chain, inherited guards, actions and invariants, a variant,
/// a witness, and both non-ordinary convergences. A second archive was tried
/// and dropped; it more than doubled the reference for one feature the schema
/// tests already cover, over fewer operators.
const ARCHIVE_MODELS: &[&str] = &["binary-search"];

fn fixtures_dir() -> PathBuf {
    common::workspace_root().join("crates/rossi-build/tests/fixtures/dump_golden")
}

fn document(project: &Project) -> String {
    let (build, sc) = rossi_build::check_with_model(project);
    let model = dump::model(project, &sc, &build, &dump::Options::default());
    let mut json = serde_json::to_string_pretty(&model).expect("the document serializes");
    json.push('\n');
    json
}

fn archive_project(name: &str) -> Project {
    let path = common::workspace_root()
        .join("crates/rossi/examples")
        .join(format!("{name}.zip"));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let mut projects =
        discover_projects(&bytes, name).unwrap_or_else(|e| panic!("{name}: discovery: {e}"));
    assert_eq!(projects.len(), 1, "{name} should hold one project");
    projects.remove(0).into_project()
}

fn generated() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (name, files) in TEXT_MODELS {
        out.push((
            (*name).to_string(),
            document(&common::example_project(name, files)),
        ));
    }
    for name in ARCHIVE_MODELS {
        out.push(((*name).to_string(), document(&archive_project(name))));
    }
    out
}

#[test]
fn documents_match_the_reference() {
    // Regenerating on demand keeps the reference honest: it is produced by
    // the same code path the assertion compares against, so there is no
    // second way of writing it that could disagree.
    let regenerate = std::env::var_os("ROSSI_DUMP_GOLDEN_REGENERATE").is_some();
    let dir = fixtures_dir();
    if regenerate {
        std::fs::create_dir_all(&dir).expect("the fixture directory");
    }

    let mut differing = Vec::new();
    for (name, generated) in generated() {
        let path = dir.join(format!("{name}.json"));
        if regenerate {
            std::fs::write(&path, &generated)
                .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
            continue;
        }
        let reference = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "read {}: {e}. Regenerate with ROSSI_DUMP_GOLDEN_REGENERATE=1",
                path.display()
            )
        });
        if reference != generated {
            differing.push(name);
        }
    }

    assert!(
        !regenerate,
        "the reference was regenerated; unset ROSSI_DUMP_GOLDEN_REGENERATE to compare"
    );
    assert!(
        differing.is_empty(),
        "these documents diverge from the reference: {}. \
         If the change is intended, regenerate with ROSSI_DUMP_GOLDEN_REGENERATE=1 \
         and review the diff.",
        differing.join(", ")
    );
}

#[test]
fn the_reference_covers_every_locked_model() {
    let dir = fixtures_dir();
    let mut stored: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .filter_map(|entry| {
            let path = entry.expect("a fixture entry").path();
            (path.extension()? == "json").then(|| {
                path.file_stem()
                    .expect("a stem")
                    .to_string_lossy()
                    .into_owned()
            })
        })
        .collect();
    stored.sort();

    let mut expected: Vec<String> = TEXT_MODELS
        .iter()
        .map(|(name, _)| (*name).to_string())
        .chain(ARCHIVE_MODELS.iter().map(|name| (*name).to_string()))
        .collect();
    expected.sort();

    assert_eq!(
        stored, expected,
        "the fixture directory and the locked model list disagree"
    );
}
