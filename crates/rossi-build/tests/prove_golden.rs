//! Golden proof statuses — the `.bps` rows rossi computes for the
//! in-repo example archives, locked against the output a reference
//! build wrote for exactly the same input.
//!
//! The scenario is the missing-status one: the archives' `.bps` files
//! are set aside, so every row must be derived from the stored `.bpr`
//! proofs against the obligations — the pure status-update pass.
//! The fixtures under `tests/fixtures/prove_golden/` are the `.bps`
//! files `rodin-headless build --auto-tactics off` produced from the
//! archives with their `.bps` entries stripped; see that directory's
//! `README.md` for the exact command and toolchain version.
//!
//! Like `pog_golden`, this gate is hermetic and runs under a plain
//! `cargo test`: rossi's side synthesizes the fresh unattempted rows
//! reconciliation produces for unmentioned obligations, runs
//! `update_statuses` with the archives' proofs, and compares every
//! row field — name order, confidence, broken, manual, and stamp —
//! against the fixture.

mod common;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::PathBuf;

use rossi_build::ScFile;
use rossi_build::pog::status::update_statuses;
use rossi_prove::bps::{PsStatus, read_bps};
use rossi_prove::po_loader::PoFile;

const MODELS: &[&str] = &[
    "base-model",
    "binary-search",
    "cars-on-bridge",
    "file-system",
    "traffic-light",
];

/// Problems reported before truncation.
const MAX_PROBLEMS: usize = 10;

fn fixtures_dir(model: &str) -> PathBuf {
    common::workspace_root()
        .join("crates/rossi-build/tests/fixtures/prove_golden")
        .join(model)
}

/// The archive's `.bpo` and `.bpr` files, keyed by component basename.
fn archive_proof_files(model: &str) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let zip = common::workspace_root()
        .join("crates/rossi/examples")
        .join(format!("{model}.zip"));
    let file = std::fs::File::open(&zip).unwrap_or_else(|e| panic!("open {}: {e}", zip.display()));
    let mut archive = zip::ZipArchive::new(file).expect("zip");
    let names: Vec<String> = archive
        .file_names()
        .map(|name| name.expect("entry name").into_owned())
        .collect();
    let mut bpos = BTreeMap::new();
    let mut bprs = BTreeMap::new();
    for name in names {
        let Some((_, base)) = name.rsplit_once('/').map(|(d, b)| (d, b.to_string())) else {
            continue;
        };
        let ext_is = |ext: &str| base.ends_with(ext);
        if !ext_is(".bpo") && !ext_is(".bpr") {
            continue;
        }
        let mut text = String::new();
        archive
            .by_name(&name)
            .expect("entry")
            .read_to_string(&mut text)
            .expect("utf8");
        if ext_is(".bpo") {
            bpos.insert(base, text);
        } else {
            bprs.insert(base, text);
        }
    }
    (bpos, bprs)
}

/// The fresh unattempted row reconciliation synthesizes for an
/// obligation the recorded `.bps` does not mention — fabricated
/// through the library's own row synthesis so a format change there
/// cannot leave this gate comparing a stale shape.
fn fresh_rows(bpo: &str) -> String {
    let parsed = PoFile::read(bpo.as_bytes()).expect("bpo");
    let rows: Vec<String> = parsed
        .sequents()
        .map(|entry| {
            rossi_build::pog::reconcile::fresh_status_row(
                &entry.name,
                entry.stamp.as_deref().unwrap_or("0"),
            )
        })
        .collect();
    rossi_build::pog::reconcile::assemble_status(&rows)
}

fn row_summary(row: &PsStatus) -> String {
    format!(
        "confidence={:?} broken={} manual={} stamp={:?}",
        row.confidence, row.broken, row.manual, row.po_stamp
    )
}

#[test]
fn statuses_match_the_rodin_oracle() {
    let mut problems = Vec::new();
    let mut compared = 0usize;
    for model in MODELS {
        let (bpos, bprs) = archive_proof_files(model);
        let mut files: Vec<ScFile> = Vec::new();
        // Every row is one reconciliation would synthesize (the
        // recorded `.bps` mentions no obligation at all).
        let mut synthesized: HashMap<String, HashSet<String>> = HashMap::new();
        for (base, contents) in &bpos {
            files.push(ScFile {
                filename: base.clone(),
                contents: contents.clone(),
                accurate: true,
            });
            let bps_name = format!("{}.bps", base.trim_end_matches(".bpo"));
            let parsed = PoFile::read(contents.as_bytes()).expect("bpo");
            synthesized.insert(
                bps_name.clone(),
                parsed.sequents().map(|entry| entry.name.clone()).collect(),
            );
            files.push(ScFile {
                filename: bps_name,
                contents: fresh_rows(contents),
                accurate: true,
            });
        }
        update_statuses(&mut files, &synthesized, |name| {
            bprs.get(name).map(|text| text.as_bytes().to_vec())
        });

        for file in &files {
            let Some(component) = file.filename.strip_suffix(".bps") else {
                continue;
            };
            let fixture_path = fixtures_dir(model).join(&file.filename);
            let fixture = std::fs::read_to_string(&fixture_path)
                .unwrap_or_else(|e| panic!("read {}: {e}", fixture_path.display()));
            let computed = read_bps(file.contents.as_bytes()).expect("computed rows");
            let expected = read_bps(fixture.as_bytes()).expect("fixture rows");

            if computed.len() != expected.len() {
                problems.push(format!(
                    "{model}/{component}: {} rows computed, {} in the oracle",
                    computed.len(),
                    expected.len()
                ));
                continue;
            }
            for (got, want) in computed.iter().zip(&expected) {
                compared += 1;
                if got.name != want.name {
                    problems.push(format!(
                        "{model}/{component}: row order {} vs {}",
                        got.name, want.name
                    ));
                } else if got.confidence != want.confidence
                    || got.broken != want.broken
                    || got.manual != want.manual
                    || got.po_stamp != want.po_stamp
                {
                    problems.push(format!(
                        "{model}/{component} {}: computed {} — oracle {}",
                        got.name,
                        row_summary(got),
                        row_summary(want)
                    ));
                }
            }
        }
    }
    assert!(compared > 200, "suspiciously few rows compared: {compared}");
    assert!(
        problems.is_empty(),
        "{} status divergences (showing up to {MAX_PROBLEMS}):\n{}",
        problems.len(),
        problems
            .iter()
            .take(MAX_PROBLEMS)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
