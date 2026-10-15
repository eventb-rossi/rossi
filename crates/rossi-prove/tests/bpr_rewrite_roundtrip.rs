//! Rewriter coverage over real proof archives.
//!
//! The hermetic test sweeps every `.bpr` inside the in-repo example
//! archives and runs in CI; the corpus scan (`--ignored`) does the same
//! over the models collection. Both assert the two properties the
//! rewriter is trusted for:
//!
//! * copying with [`ProofAction::Keep`] reproduces the file byte for
//!   byte — nothing outside the entries a caller acts on may move;
//! * a rewritten file still reads, and after
//!   [`ProofAction::Reset`] every proof reads back as unattempted.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use rossi_prove::bpr::{BprError, Keep, read_bpr};
use rossi_prove::bpr_rewrite::{ProofAction, rewrite_bpr};

fn workspace_root() -> PathBuf {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.pop();
    root.pop();
    root
}

fn corpus_dir() -> Option<PathBuf> {
    let dir = match std::env::var_os("EVENTB_CORPUS_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => workspace_root().join("../eventb-models-collection"),
    };
    dir.is_dir().then_some(dir)
}

fn zips_in(dir: &Path) -> Vec<PathBuf> {
    let mut zips: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("readable directory")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "zip"))
        .collect();
    zips.sort();
    zips
}

#[derive(Default)]
struct Tally {
    files: usize,
    proofs: usize,
    /// Files both the reader and the rewriter refuse: the legacy
    /// pre-versioning vintage.
    legacy: usize,
}

/// Checks both properties for one `.bpr` document.
fn check(label: &str, original: &[u8], tally: &mut Tally) {
    tally.files += 1;

    let mut copied = Vec::with_capacity(original.len());
    let stats = match rewrite_bpr(original, &mut copied, |_| ProofAction::Keep) {
        Ok(stats) => stats,
        // The rewriter and the reader must agree on which vintages
        // they refuse, or a `clean` run would report a file the rest
        // of the toolchain reads fine.
        Err(BprError::Unsupported(_)) => {
            let err = read_bpr(original, |_| Keep::Skip)
                .err()
                .unwrap_or_else(|| panic!("{label}: rewriter refused a file the reader accepts"));
            assert!(
                matches!(err, BprError::Unsupported(_)),
                "{label}: reader failed differently: {err}"
            );
            tally.legacy += 1;
            return;
        }
        Err(err) => panic!("{label}: {err}"),
    };

    assert_eq!(
        copied.len(),
        original.len(),
        "{label}: a kept copy changed length"
    );
    assert!(copied == original, "{label}: a kept copy changed bytes");
    assert_eq!(stats.dropped, 0);
    assert_eq!(stats.reset, 0);
    tally.proofs += stats.kept;

    let mut emptied = Vec::new();
    let reset = rewrite_bpr(original, &mut emptied, |_| ProofAction::Reset)
        .unwrap_or_else(|err| panic!("{label}: resetting: {err}"));
    assert_eq!(reset.reset, stats.kept, "{label}: reset every entry");
    assert!(
        emptied.len() <= original.len(),
        "{label}: emptying grew the file"
    );

    // The emptied file must still read, and hold the same obligations
    // with nothing stored against them.
    let entries = read_bpr(emptied.as_slice(), |_| Keep::Full)
        .unwrap_or_else(|err| panic!("{label}: rereading the emptied file: {err}"));
    assert_eq!(entries.len(), stats.kept, "{label}: entry count changed");
    for entry in &entries {
        assert!(
            entry.confidence.is_none() && !entry.manual,
            "{label}: {} kept proof state after reset",
            entry.name
        );
    }
}

fn scan_zip(path: &Path, tally: &mut Tally) {
    let file = File::open(path).expect("readable archive");
    let mut archive = zip::ZipArchive::new(BufReader::new(file)).expect("readable archive");
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).expect("readable entry");
        let name = entry.name().expect("readable entry name").into_owned();
        if !name.ends_with(".bpr") {
            continue;
        }
        let label = format!("{}!{name}", path.display());
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes).expect("readable entry");
        check(&label, &bytes, tally);
    }
}

#[test]
fn example_archives_round_trip() {
    let zips = zips_in(&workspace_root().join("crates/rossi/examples"));
    assert!(!zips.is_empty());

    let mut tally = Tally::default();
    for path in &zips {
        scan_zip(path, &mut tally);
    }
    assert!(tally.files > 0 && tally.proofs > 0);
    println!(
        "example archives: {} .bpr files ({} legacy), {} proofs round-tripped",
        tally.files, tally.legacy, tally.proofs,
    );
}

#[test]
#[ignore = "needs a models corpus; run with --ignored"]
fn corpus_archives_round_trip() {
    let Some(dir) = corpus_dir() else {
        eprintln!("no corpus directory; set EVENTB_CORPUS_DIR");
        return;
    };
    let zips = zips_in(&dir);
    assert!(!zips.is_empty());

    let mut tally = Tally::default();
    for path in &zips {
        scan_zip(path, &mut tally);
    }
    println!(
        "corpus: {} archives, {} .bpr files ({} legacy), {} proofs round-tripped",
        zips.len(),
        tally.files,
        tally.legacy,
        tally.proofs,
    );
}
