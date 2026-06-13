//! Corpus integration test — the textual import→validate round-trip
//! (issue #28): import every `.bum`/`.buc` from every corpus model, pretty-
//! print it to the rossi textual format, and re-parse that text. `rossi
//! validate` must always accept `rossi import`'s own output, hyphenated
//! Rodin component/event names included.
//!
//! This is the textual counterpart of `corpus` (which diffs regenerated
//! `.bcc`/`.bcm` and never exercises the text grammar).
//!
//! `#[ignore]` by default (the corpus lives outside the repo). Run locally:
//!
//!   EVENTB_CORPUS_DIR=../eventb-models-collection \
//!     cargo test -p rossi-build --test import_corpus -- --ignored --nocapture
//!
//! A TSV report is written to `target/rossi-build-import-corpus.tsv`
//! (model | expected | actual | verdict | notes).
//!
//! Verdicts:
//!   match — every component imports, prints, re-parses, and re-prints
//!           identically (a stable round-trip)
//!   known — a model flagged `defective` (broken source), `unsupported`
//!           (needs an Event-B extension rossi doesn't have, e.g. the theory
//!           plugin), or `keyword_identifier` (declares an identifier the
//!           textual grammar cannot express, e.g. a constant named `end`)
//!           in the corpus `model_flags.tsv`; reported, never fails
//!   fail  — an unflagged model whose import, re-parse, or print-stability
//!           failed (fails the test)

mod common;

use std::path::Path;

use common::{Row, load_flags, locate_corpus, sanitize, workspace_target, write_report};

#[test]
#[ignore]
fn import_output_reparses_for_corpus() {
    let Some(corpus) = locate_corpus() else {
        eprintln!("EVENTB_CORPUS_DIR is not set or is not a directory — nothing to do");
        return;
    };
    let flags = load_flags(&corpus.join("model_flags.tsv")).unwrap_or_default();

    let mut rows = Vec::<Row>::new();
    for entry in std::fs::read_dir(&corpus).expect("read corpus") {
        let path = entry.expect("read dir entry").path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(model) = name.strip_suffix(".zip") else {
            continue;
        };

        let excused = flags.get(model).is_some_and(|f| {
            f.contains("defective") || f.contains("unsupported") || f.contains("keyword_identifier")
        });

        let (actual, notes) = match round_trip_zip(&path) {
            Ok(files) => ("round_trip".to_string(), format!("{files} files")),
            Err(e) => ("fail".to_string(), e),
        };
        let verdict = match (actual.as_str(), excused) {
            ("round_trip", _) => "match",
            (_, true) => "known",
            (_, false) => "fail",
        };
        rows.push(Row {
            model: model.to_string(),
            expected: if excused {
                "known-broken"
            } else {
                "round_trip"
            }
            .to_string(),
            actual,
            verdict: verdict.to_string(),
            notes,
        });
    }
    rows.sort_by(|a, b| a.model.cmp(&b.model));

    let report = workspace_target().join("rossi-build-import-corpus.tsv");
    write_report(
        &report,
        &["model", "expected", "actual", "verdict", "notes"],
        &rows.iter().map(Row::to_fields).collect::<Vec<_>>(),
    );

    let total = rows.len();
    let matched = rows.iter().filter(|r| r.verdict == "match").count();
    let known = rows.iter().filter(|r| r.verdict == "known").count();
    println!(
        "import corpus: {matched}/{total} models round-trip, {known} known-broken; \
         report: {}",
        report.display()
    );

    let failures: Vec<&Row> = rows.iter().filter(|r| r.verdict == "fail").collect();
    if !failures.is_empty() {
        for f in &failures {
            eprintln!("  FAIL  {} — {}", f.model, f.notes);
        }
        panic!(
            "{} / {total} models failed the import→validate round-trip",
            failures.len()
        );
    }
}

/// Import every component of one archive and round-trip it through the
/// textual format. Returns the number of files checked, or a description of
/// the first failure.
fn round_trip_zip(path: &Path) -> Result<usize, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read: {e}"))?;
    let components =
        rossi::parse_zip(&bytes).map_err(|e| format!("import: {}", sanitize(&e.to_string())))?;

    for named in &components {
        let file = &named.filename;
        // The pretty printer debug_asserts that structural names are
        // re-lexable; a panic here is a real import/grammar disagreement —
        // contain it to a per-file failure so the whole run still reports.
        let text = std::panic::catch_unwind(|| rossi::to_string(&named.component))
            .map_err(|_| format!("{file}: print panicked (unparseable name reached the AST)"))?;
        let reparsed = rossi::parse(&text)
            .map_err(|e| format!("{file}: re-parse: {}", sanitize(&e.to_string())))?;
        let reprinted = rossi::to_string(&reparsed);
        if reprinted != text {
            let diff_at = text
                .lines()
                .zip(reprinted.lines())
                .position(|(a, b)| a != b)
                .map(|i| format!("first differing line {}", i + 1))
                .unwrap_or_else(|| "different line counts".to_string());
            return Err(format!("{file}: unstable round-trip ({diff_at})"));
        }
    }
    Ok(components.len())
}
