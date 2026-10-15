//! Reader coverage over real proof archives.
//!
//! The hermetic test parses every `.bpr` inside the in-repo example
//! archives and runs in CI. The corpus scan (`--ignored`) sweeps every
//! model of the models collection, accounts outcomes per model, and
//! cross-checks the recomputed skeleton confidence against the stored
//! proof confidence for proofs whose reasoners are all trusted; its
//! report lands in `target/rossi-prove-bpr-corpus.tsv`.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Write};
use std::path::PathBuf;

use rayon::prelude::*;
use rossi_prove::bpr::{Keep, ProofBody, ProofEntry, read_bpr};
use rossi_prove::confidence::Confidence;
use rossi_prove::skeleton::Skeleton;

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

/// The confidence the skeleton computes bottom-up: the minimum over
/// its rules, an open leaf pending.
fn skeleton_confidence(skel: &Skeleton) -> Confidence {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || match &skel.rule {
        None => Confidence::PENDING,
        Some(stored) => skel
            .children
            .iter()
            .map(skeleton_confidence)
            .fold(stored.rule.confidence, Confidence::min),
    })
}

fn any_untrusted(skel: &Skeleton) -> bool {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        skel.rule
            .as_ref()
            .is_some_and(|stored| !stored.rule.reasoner.is_trusted())
            || skel.children.iter().any(any_untrusted)
    })
}

#[derive(Default)]
struct Tally {
    files: usize,
    /// Files the reader refuses wholesale — the pre-versioning
    /// vintage without a `version` attribute on the file root.
    legacy_files: usize,
    proofs: usize,
    loaded: usize,
    unattempted: usize,
    unsupported: BTreeMap<String, usize>,
    conf_match: usize,
    conf_capped: usize,
    conf_diff: usize,
}

impl Tally {
    fn unsupported_total(&self) -> usize {
        self.unsupported.values().sum()
    }

    /// Coarse classification of an unsupported reason, so the corpus
    /// report aggregates by cause rather than by message.
    fn reason_class(reason: &str) -> &'static str {
        if reason.contains("old-vintage") {
            "old_vintage"
        } else if reason.contains("extended") {
            "extended_lang"
        } else if reason.contains("unexpected element") {
            "unexpected_element"
        } else if reason.contains("does not type-check") || reason.contains("identifier type") {
            "type_error"
        } else if reason.contains("predicate") || reason.contains("expression") {
            "parse_error"
        } else {
            "other"
        }
    }

    fn record(&mut self, entry: &ProofEntry) {
        self.proofs += 1;
        match &entry.body {
            ProofBody::Skipped => unreachable!("scan keeps everything"),
            ProofBody::Unsupported(reason) => {
                *self
                    .unsupported
                    .entry(Self::reason_class(reason).to_string())
                    .or_default() += 1;
            }
            ProofBody::Loaded(proof) => {
                self.loaded += 1;
                if entry.confidence.is_none() {
                    self.unattempted += 1;
                }
                if let (Some(stored), Some(skel)) = (entry.confidence, proof.skeleton.as_ref())
                    && skel.rule.is_some()
                {
                    // Rules of untrusted reasoners were capped at
                    // uncertain on load, so the recomputed confidence
                    // legitimately drops below the recorded one.
                    if any_untrusted(skel) {
                        self.conf_capped += 1;
                    } else if skeleton_confidence(skel) == Confidence(stored) {
                        self.conf_match += 1;
                    } else {
                        self.conf_diff += 1;
                    }
                }
            }
        }
    }
}

fn scan_zip(path: &PathBuf, tally: &mut Tally) -> Result<(), String> {
    let file = File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|err| format!("{}: {err}", path.display()))?;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|err| format!("{}: {err}", path.display()))?;
        let entry_name = entry
            .name()
            .map_err(|err| format!("{}: {err}", path.display()))?
            .into_owned();
        if !entry_name.ends_with(".bpr") {
            continue;
        }
        tally.files += 1;
        match read_bpr(BufReader::new(entry), |_| Keep::Full) {
            Ok(proofs) => {
                for proof in &proofs {
                    tally.record(proof);
                }
            }
            // Pre-versioning files are a legacy vintage, accounted
            // rather than failed; malformed XML still fails the scan.
            Err(rossi_prove::bpr::BprError::Unsupported(_)) => tally.legacy_files += 1,
            Err(err) => return Err(format!("{}!{entry_name}: {err}", path.display())),
        }
    }
    Ok(())
}

#[test]
fn example_archives_parse() {
    let dir = workspace_root().join("crates/rossi/examples");
    let mut zips: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("examples directory")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "zip"))
        .collect();
    zips.sort();
    assert!(!zips.is_empty());

    let mut tally = Tally::default();
    for path in &zips {
        scan_zip(path, &mut tally).expect("readable archive");
    }
    assert!(tally.files > 0);
    assert!(tally.proofs > 0);
    println!(
        "example archives: {} .bpr files, {} proofs, {} loaded, {} unsupported {:?}, \
         confidence {}/{}/{} (match/capped/diff)",
        tally.files,
        tally.proofs,
        tally.loaded,
        tally.unsupported_total(),
        tally.unsupported,
        tally.conf_match,
        tally.conf_capped,
        tally.conf_diff,
    );
}

#[test]
#[ignore = "needs a models corpus; run with --ignored"]
fn corpus_bpr_scan() {
    let Some(corpus) = corpus_dir() else {
        eprintln!("corpus not found; set EVENTB_CORPUS_DIR");
        return;
    };
    let mut zips: Vec<PathBuf> = std::fs::read_dir(&corpus)
        .expect("corpus directory")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "zip"))
        .collect();
    zips.sort();

    let mut report = String::from(
        "model\tbpr_files\tlegacy_files\tproofs\tloaded\tunsupported\tclasses\tconf_match\tconf_capped\tconf_diff\n",
    );
    // Archives are independent: scan them in parallel, then fold the
    // results in archive order so the report stays stable.
    let scanned: Vec<Result<Tally, String>> = rossi_prove::thread_pool().install(|| {
        zips.par_iter()
            .map(|path| {
                let mut tally = Tally::default();
                scan_zip(path, &mut tally).map(|()| tally)
            })
            .collect()
    });
    let mut failures = Vec::new();
    let mut total = Tally::default();
    for (path, scanned) in zips.iter().zip(scanned) {
        let model = path.file_stem().unwrap_or_default().to_string_lossy();
        let tally = match scanned {
            Ok(tally) => tally,
            Err(err) => {
                failures.push(err);
                continue;
            }
        };
        let classes = tally
            .unsupported
            .iter()
            .map(|(class, count)| format!("{class}:{count}"))
            .collect::<Vec<_>>()
            .join(",");
        report.push_str(&format!(
            "{model}\t{}\t{}\t{}\t{}\t{}\t{classes}\t{}\t{}\t{}\n",
            tally.files,
            tally.legacy_files,
            tally.proofs,
            tally.loaded,
            tally.unsupported_total(),
            tally.conf_match,
            tally.conf_capped,
            tally.conf_diff,
        ));
        total.files += tally.files;
        total.legacy_files += tally.legacy_files;
        total.proofs += tally.proofs;
        total.loaded += tally.loaded;
        total.unattempted += tally.unattempted;
        for (class, count) in tally.unsupported {
            *total.unsupported.entry(class).or_default() += count;
        }
        total.conf_match += tally.conf_match;
        total.conf_capped += tally.conf_capped;
        total.conf_diff += tally.conf_diff;
    }

    let out = workspace_root().join("target/rossi-prove-bpr-corpus.tsv");
    File::create(&out)
        .and_then(|mut f| f.write_all(report.as_bytes()))
        .expect("report written");
    println!(
        "corpus: {} .bpr files ({} legacy), {} proofs, {} loaded ({} unattempted), {} unsupported {:?}, \
         confidence {}/{}/{} (match/capped/diff); report: {}",
        total.files,
        total.legacy_files,
        total.proofs,
        total.loaded,
        total.unattempted,
        total.unsupported_total(),
        total.unsupported,
        total.conf_match,
        total.conf_capped,
        total.conf_diff,
        out.display(),
    );
    assert!(
        failures.is_empty(),
        "{} archives failed to scan:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
