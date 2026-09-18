//! Proof-status update at the build boundary.
//!
//! [`super::reconcile`] carries `.bps` rows verbatim and leaves a
//! stale stamp as the signal that the recorded verdict no longer
//! matches its obligation. This module acts on that signal the way a
//! reference build does: a row whose stamp differs from its sequent's
//! (or whose proof is context-dependent) is recomputed from the
//! stored proof's dependencies — broken when the proof no longer
//! applies, its confidence a verbatim copy of the proof's — while
//! stamp-matched rows keep their exact bytes. Like reconciliation
//! this runs at the IO boundary (repackaging), never inside pure
//! `build()`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

use rayon::prelude::*;

use rossi_prove::Confidence;
use rossi_prove::bpr::{Keep, ProofBody, ProofEntry, read_bpr};
use rossi_prove::po_loader::{PoFile, PoProject};
use rossi_prove::status::{StatusVerdict, compute_status};

use crate::ScFile;
use crate::pog::reconcile::{
    StatusRow, assemble_status, bpo_bps_pairs, fresh_status_row, parse_status_rows, sequent_stamps,
};
use crate::xml_out::{attr, escape_attr, tag as xtag};

/// Recomputes every stale `.bps` row in `files` against the stored
/// proofs supplied by `bpr` (keyed by filename, e.g. `M0.bpr`).
///
/// A row is stale when its recorded stamp differs from its sequent's
/// stamp in the sibling `.bpo`, or when its proof is marked
/// context-dependent. A row reconciliation synthesized for an
/// obligation the recorded `.bps` never mentioned (`synthesized`,
/// keyed by `.bps` filename — the return of
/// [`super::reconcile::reconcile_build_files`]) is also recomputed
/// when its stored proof records real confidence: a missing status is
/// computed from the stored proofs, while a recorded stamp-valid row —
/// even an unattempted one — is never touched. Stale rows are
/// rewritten with a fresh verdict and the sequent's stamp; a stale
/// row without a stored proof becomes a fresh unattempted row. A
/// proof file that exists but cannot be read leaves the component's
/// rows exactly as reconciliation produced them: the stamp divergence
/// stays visible downstream (such rows are re-checked there),
/// instead of silently resetting recorded verdicts to unattempted.
/// Everything else keeps its exact bytes, so an archive whose
/// obligations did not change round-trips untouched.
pub fn update_statuses(
    files: &mut [ScFile],
    synthesized: &HashMap<String, HashSet<String>>,
    mut bpr: impl FnMut(&str) -> Option<Vec<u8>>,
) {
    // Gathering runs sequentially — it owns the proof-file callback —
    // and keeps only the components with rows whose verdict may
    // change: the stamp-stale ones, plus the synthesized unattempted
    // rows a stored proof may revive.
    let no_revivals = HashSet::new();
    let mut jobs = Vec::new();
    for (i, j) in bpo_bps_pairs(files) {
        let stamps = sequent_stamps(&files[i].contents);
        let rows = parse_status_rows(&files[j].contents);
        let revivable = synthesized.get(&files[j].filename).unwrap_or(&no_revivals);
        let candidates: HashSet<String> = rows
            .iter()
            .filter(|row| {
                stamp_stale(&stamps, row)
                    || (revivable.contains(&row.name)
                        && !row.broken
                        && !Confidence::is_attempted(row.confidence))
            })
            .map(|row| row.name.clone())
            .collect();
        if candidates.is_empty() {
            continue;
        }
        let stem = files[i].filename.trim_end_matches(".bpo");
        let proofs = bpr(&format!("{stem}.bpr"));
        jobs.push(Job {
            bpo: i,
            bps: j,
            stamps,
            rows,
            candidates,
            proofs,
        });
    }

    // The components are independent from here on. The generated
    // obligation files are parsed on the first stale row — an
    // unchanged archive never pays for it.
    let project = OnceLock::new();
    let shared: &[ScFile] = files;
    let updated: Vec<(usize, String)> = rossi_prove::thread_pool().install(|| {
        jobs.par_iter()
            .filter_map(|job| Some((job.bps, update_component(job, shared, &project)?)))
            .collect()
    });
    for (j, contents) in updated {
        files[j].contents = contents;
    }
}

/// One component with rows to recompute.
struct Job {
    /// Index of the `.bpo` in the file list.
    bpo: usize,
    /// Index of the `.bps` in the file list.
    bps: usize,
    stamps: HashMap<String, String>,
    rows: Vec<StatusRow>,
    /// Rows whose verdict may change.
    candidates: HashSet<String>,
    /// The stored proofs, `None` without a proof file.
    proofs: Option<Vec<u8>>,
}

impl Job {
    fn stamp_stale(&self, row: &StatusRow) -> bool {
        stamp_stale(&self.stamps, row)
    }

    /// The row's sequent stamp, `0` for an obligation without one.
    fn stamp(&self, row: &StatusRow) -> &str {
        self.stamps.get(&row.name).map_or("0", String::as_str)
    }
}

fn stamp_stale(stamps: &HashMap<String, String>, row: &StatusRow) -> bool {
    row.context_dependent || stamps.get(&row.name).map(String::as_str) != row.stamp.as_deref()
}

/// The component's new `.bps` contents, `None` when its rows stay.
fn update_component(job: &Job, files: &[ScFile], project: &OnceLock<PoProject>) -> Option<String> {
    let Some(bpr_contents) = &job.proofs else {
        // No proof file at all: every stamp-stale row resets to a
        // fresh unattempted one (the status update on a missing
        // proof), and there is nothing to revive.
        if !job.rows.iter().any(|row| job.stamp_stale(row)) {
            return None;
        }
        let out: Vec<String> = job
            .rows
            .iter()
            .map(|row| {
                if job.stamp_stale(row) {
                    fresh_status_row(&row.name, job.stamp(row))
                } else {
                    row.row.clone()
                }
            })
            .collect();
        return Some(assemble_status(&out));
    };
    // One pass over the proof file: dependencies for the candidate
    // rows, name and confidence for everything else.
    let Ok(entries) = read_bpr(bpr_contents.as_slice(), |name| {
        if job.candidates.contains(name) {
            Keep::Deps
        } else {
            Keep::Skip
        }
    }) else {
        // The proof file exists but cannot be read — carry the
        // rows verbatim rather than resetting recorded verdicts.
        return None;
    };
    let proofs: BTreeMap<String, ProofEntry> = entries
        .into_iter()
        .map(|entry| (entry.name.clone(), entry))
        .collect();
    let stale = |row: &StatusRow| {
        job.stamp_stale(row)
            || (job.candidates.contains(row.name.as_str())
                && Confidence::is_attempted(
                    proofs
                        .get(&row.name)
                        .and_then(|entry| entry.confidence)
                        .map(i64::from),
                ))
    };
    if !job.rows.iter().any(&stale) {
        return None;
    }
    let project = project.get_or_init(|| build_project(files));

    let out: Vec<String> = job
        .rows
        .iter()
        .map(|row| {
            if !stale(row) {
                return row.row.clone();
            }
            let stamp = job.stamp(row);
            match proofs.get(&row.name) {
                Some(entry) if !matches!(entry.body, ProofBody::Skipped) => {
                    match project.load(&files[job.bpo].filename, &row.name) {
                        Ok(seq) => status_row(&row.name, &compute_status(&seq, entry), stamp),
                        // Our own generated obligation failed to
                        // load — keep the stale row, so the stamp
                        // divergence stays visible downstream.
                        Err(_) => row.row.clone(),
                    }
                }
                _ => fresh_status_row(&row.name, stamp),
            }
        })
        .collect();
    Some(assemble_status(&out))
}

/// All generated obligation files as one project: hypothesis-set
/// chains cross component files.
pub fn build_project(files: &[ScFile]) -> PoProject {
    let mut project = PoProject::default();
    for file in files {
        if file.filename.ends_with(".bpo")
            && let Ok(parsed) = PoFile::read(file.contents.as_bytes())
        {
            project.insert(file.filename.clone(), parsed);
        }
    }
    project
}

/// Renders one recomputed status row in the generator's shape, the
/// attributes in alphabetical id order (`confidence`,
/// `contextDependent`, `poStamp`, `psBroken`, `psManual`).
fn status_row(name: &str, verdict: &StatusVerdict, stamp: &str) -> String {
    let mut row = format!("<{} {}=\"", xtag::PS_STATUS, attr::NAME);
    escape_attr(name, &mut row);
    row.push_str(&format!(
        "\" {}=\"{}\"",
        attr::CONFIDENCE,
        verdict.confidence.unwrap_or(Confidence::UNATTEMPTED.0),
    ));
    if verdict.context_dependent {
        row.push_str(&format!(" {}=\"true\"", attr::CONTEXT_DEPENDENT));
    }
    row.push_str(&format!(" {}=\"{stamp}\"", attr::PO_STAMP));
    if verdict.broken {
        row.push_str(&format!(" {}=\"true\"", attr::PS_BROKEN));
    }
    row.push_str(&format!(
        " {}=\"{}\"/>",
        attr::PS_MANUAL,
        if verdict.manual { "true" } else { "false" },
    ));
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    const BPO: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.poFile org.eventb.core.poStamp="0">
<org.eventb.core.poPredicateSet name="ALLHYP" org.eventb.core.poStamp="0">
<org.eventb.core.poIdentifier name="x" org.eventb.core.type="ℤ"/>
<org.eventb.core.poPredicate name="PRD0" org.eventb.core.predicate="x=1"/>
</org.eventb.core.poPredicateSet>
<org.eventb.core.poSequent name="evt/inv1/INV" org.eventb.core.poStamp="2">
<org.eventb.core.poPredicateSet name="SEQHYP" org.eventb.core.parentSet="/P/M0.bpo|org.eventb.core.poFile#M0|org.eventb.core.poPredicateSet#ALLHYP"/>
<org.eventb.core.poPredicate name="SEQG" org.eventb.core.predicate="x&lt;2"/>
</org.eventb.core.poSequent>
<org.eventb.core.poSequent name="evt/inv2/INV" org.eventb.core.poStamp="2">
<org.eventb.core.poPredicateSet name="SEQHYP" org.eventb.core.parentSet="/P/M0.bpo|org.eventb.core.poFile#M0|org.eventb.core.poPredicateSet#ALLHYP"/>
<org.eventb.core.poPredicate name="SEQG" org.eventb.core.predicate="x&lt;3"/>
</org.eventb.core.poSequent>
<org.eventb.core.poSequent name="evt/inv3/INV" org.eventb.core.poStamp="2">
<org.eventb.core.poPredicateSet name="SEQHYP" org.eventb.core.parentSet="/P/M0.bpo|org.eventb.core.poFile#M0|org.eventb.core.poPredicateSet#ALLHYP"/>
<org.eventb.core.poPredicate name="SEQG" org.eventb.core.predicate="x&lt;4"/>
</org.eventb.core.poSequent>
</org.eventb.core.poFile>
"#;

    /// inv1: fresh at the current stamp; inv2 and inv3: stale.
    const BPS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.psFile>
<org.eventb.core.psStatus name="evt/inv1/INV" org.eventb.core.confidence="1000" org.eventb.core.poStamp="2" org.eventb.core.psManual="false"/>
<org.eventb.core.psStatus name="evt/inv2/INV" org.eventb.core.confidence="1000" org.eventb.core.poStamp="1" org.eventb.core.psManual="true"/>
<org.eventb.core.psStatus name="evt/inv3/INV" org.eventb.core.confidence="1000" org.eventb.core.poStamp="1" org.eventb.core.psManual="false"/>
</org.eventb.core.psFile>
"#;

    /// inv2 has a proof matching its regenerated sequent; inv3 has a
    /// proof depending on a goal the sequent no longer has.
    const BPR: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.prFile version="1">
<org.eventb.core.prProof name="evt/inv2/INV" org.eventb.core.confidence="1000" org.eventb.core.prFresh="" org.eventb.core.prGoal="p0" org.eventb.core.prHyps="p1" org.eventb.core.psManual="true">
<org.eventb.core.prIdent name="x" org.eventb.core.type="ℤ"/>
<org.eventb.core.prPred name="p0" org.eventb.core.predicate="x&lt;3"/>
<org.eventb.core.prPred name="p1" org.eventb.core.predicate="x=1"/>
<org.eventb.core.prReas name="r0" org.eventb.core.prRID="org.eventb.core.seqprover.hyp"/>
</org.eventb.core.prProof>
<org.eventb.core.prProof name="evt/inv3/INV" org.eventb.core.confidence="1000" org.eventb.core.prFresh="" org.eventb.core.prGoal="p0" org.eventb.core.prHyps="">
<org.eventb.core.prIdent name="x" org.eventb.core.type="ℤ"/>
<org.eventb.core.prPred name="p0" org.eventb.core.predicate="x&lt;9"/>
<org.eventb.core.prReas name="r0" org.eventb.core.prRID="org.eventb.core.seqprover.hyp"/>
</org.eventb.core.prProof>
</org.eventb.core.prFile>
"#;

    fn files_with(bps: &str) -> Vec<ScFile> {
        vec![
            ScFile {
                filename: "M0.bpo".into(),
                contents: BPO.into(),
                accurate: true,
            },
            ScFile {
                filename: "M0.bps".into(),
                contents: bps.into(),
                accurate: true,
            },
        ]
    }

    /// Every row of `M0.bps` marked as synthesized by reconciliation.
    fn all_synthesized() -> HashMap<String, HashSet<String>> {
        HashMap::from([(
            "M0.bps".to_string(),
            ["evt/inv1/INV", "evt/inv2/INV", "evt/inv3/INV"]
                .map(str::to_string)
                .into(),
        )])
    }

    fn run(bpr: Option<&str>) -> String {
        let mut files = files_with(BPS);
        update_statuses(&mut files, &HashMap::new(), |name| {
            assert_eq!(name, "M0.bpr");
            bpr.map(|text| text.as_bytes().to_vec())
        });
        files[1].contents.clone()
    }

    #[test]
    fn stale_rows_get_fresh_verdicts_and_stamps() {
        let bps = run(Some(BPR));
        // The stamp-valid row keeps its exact bytes.
        assert!(bps.contains(
            r#"<org.eventb.core.psStatus name="evt/inv1/INV" org.eventb.core.confidence="1000" org.eventb.core.poStamp="2" org.eventb.core.psManual="false"/>"#
        ));
        // The reusable proof keeps its confidence at the new stamp,
        // manual flag copied from the proof.
        assert!(bps.contains(
            r#"<org.eventb.core.psStatus name="evt/inv2/INV" org.eventb.core.confidence="1000" org.eventb.core.poStamp="2" org.eventb.core.psManual="true"/>"#
        ));
        // The inapplicable proof is broken, confidence still the
        // cached copy; attributes in alphabetical order.
        assert!(bps.contains(
            r#"<org.eventb.core.psStatus name="evt/inv3/INV" org.eventb.core.confidence="1000" org.eventb.core.poStamp="2" org.eventb.core.psBroken="true" org.eventb.core.psManual="false"/>"#
        ));
    }

    /// The loose multi-project output path names files with a
    /// directory prefix (`proj/M0.bpo`) while every parent-set handle
    /// still resolves to the basename; recomputation must find the
    /// generated file all the same instead of silently carrying the
    /// stale rows.
    #[test]
    fn prefixed_filenames_still_get_recomputed() {
        let mut files = files_with(BPS);
        for file in &mut files {
            file.filename = format!("proj/{}", file.filename);
        }
        update_statuses(&mut files, &HashMap::new(), |name| {
            assert_eq!(name, "proj/M0.bpr");
            Some(BPR.as_bytes().to_vec())
        });
        let bps = &files[1].contents;
        assert!(bps.contains(
            r#"<org.eventb.core.psStatus name="evt/inv2/INV" org.eventb.core.confidence="1000" org.eventb.core.poStamp="2" org.eventb.core.psManual="true"/>"#
        ));
        assert!(bps.contains(
            r#"<org.eventb.core.psStatus name="evt/inv3/INV" org.eventb.core.confidence="1000" org.eventb.core.poStamp="2" org.eventb.core.psBroken="true" org.eventb.core.psManual="false"/>"#
        ));
    }

    #[test]
    fn missing_proofs_reset_to_unattempted() {
        let bps = run(None);
        assert!(bps.contains(
            r#"<org.eventb.core.psStatus name="evt/inv2/INV" org.eventb.core.confidence="-99" org.eventb.core.poStamp="2" org.eventb.core.psManual="false"/>"#
        ));
        assert!(bps.contains(
            r#"<org.eventb.core.psStatus name="evt/inv3/INV" org.eventb.core.confidence="-99" org.eventb.core.poStamp="2" org.eventb.core.psManual="false"/>"#
        ));
    }

    #[test]
    fn synthesized_unattempted_rows_revive_from_stored_proofs() {
        // Every row reads unattempted at the current stamp — the shape
        // reconciliation synthesizes when the recorded .bps had no row
        // for the obligation. The status update computes missing
        // statuses from the stored proofs, so rows with a real proof
        // revive; the proofless row keeps its exact bytes.
        let all_fresh = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.psFile>
<org.eventb.core.psStatus name="evt/inv1/INV" org.eventb.core.confidence="-99" org.eventb.core.poStamp="2" org.eventb.core.psManual="false"/>
<org.eventb.core.psStatus name="evt/inv2/INV" org.eventb.core.confidence="-99" org.eventb.core.poStamp="2" org.eventb.core.psManual="false"/>
<org.eventb.core.psStatus name="evt/inv3/INV" org.eventb.core.confidence="-99" org.eventb.core.poStamp="2" org.eventb.core.psManual="false"/>
</org.eventb.core.psFile>
"#;
        let mut files = files_with(all_fresh);
        update_statuses(&mut files, &all_synthesized(), |_| {
            Some(BPR.as_bytes().to_vec())
        });
        let bps = &files[1].contents;
        // No stored proof for inv1: its fresh row is genuinely
        // unattempted and stays byte-exact.
        assert!(bps.contains(
            r#"<org.eventb.core.psStatus name="evt/inv1/INV" org.eventb.core.confidence="-99" org.eventb.core.poStamp="2" org.eventb.core.psManual="false"/>"#
        ));
        // inv2's stored proof still applies: discharged, manual copied.
        assert!(bps.contains(
            r#"<org.eventb.core.psStatus name="evt/inv2/INV" org.eventb.core.confidence="1000" org.eventb.core.poStamp="2" org.eventb.core.psManual="true"/>"#
        ));
        // inv3's stored proof no longer applies: broken.
        assert!(bps.contains(
            r#"<org.eventb.core.psStatus name="evt/inv3/INV" org.eventb.core.confidence="1000" org.eventb.core.poStamp="2" org.eventb.core.psBroken="true" org.eventb.core.psManual="false"/>"#
        ));
    }

    /// A recorded stamp-valid unattempted row is not the reconciler's
    /// synthesized shape, even when a leftover proof sits in the
    /// `.bpr`: the status update keys only on the stamp and never
    /// touches such a row, so neither do we — and the proof file is
    /// not even consulted.
    #[test]
    fn recorded_stamp_valid_unattempted_rows_stay_untouched() {
        let all_fresh = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.psFile>
<org.eventb.core.psStatus name="evt/inv1/INV" org.eventb.core.confidence="-99" org.eventb.core.poStamp="2" org.eventb.core.psManual="false"/>
<org.eventb.core.psStatus name="evt/inv2/INV" org.eventb.core.confidence="-99" org.eventb.core.poStamp="2" org.eventb.core.psManual="false"/>
<org.eventb.core.psStatus name="evt/inv3/INV" org.eventb.core.confidence="-99" org.eventb.core.poStamp="2" org.eventb.core.psManual="false"/>
</org.eventb.core.psFile>
"#;
        let mut files = files_with(all_fresh);
        update_statuses(&mut files, &HashMap::new(), |_| {
            panic!("no candidate rows, so the proof file must not be read")
        });
        assert_eq!(files[1].contents, all_fresh);
    }

    /// A proof file that exists but cannot be parsed must not reset
    /// recorded verdicts: the stale rows are carried verbatim so the
    /// stamp divergence stays visible, exactly as a reference build
    /// re-checks
    /// them, rather than being replaced by fresh unattempted rows.
    #[test]
    fn unreadable_proof_files_carry_stale_rows_verbatim() {
        for bpr in [
            // A proof-file version this reader refuses wholesale.
            "<org.eventb.core.prFile version=\"99\"/>"
                .as_bytes()
                .to_vec(),
            // Malformed XML.
            b"<org.eventb.core.prFile version=\"1\"><org.eventb".to_vec(),
        ] {
            let mut files = files_with(BPS);
            update_statuses(&mut files, &HashMap::new(), |_| Some(bpr.clone()));
            assert_eq!(files[1].contents, BPS);
        }
    }

    #[test]
    fn untouched_components_round_trip_byte_exact() {
        let fresh_bps = BPS.replace(
            "org.eventb.core.poStamp=\"1\"",
            "org.eventb.core.poStamp=\"2\"",
        );
        let mut files = files_with(&fresh_bps);
        // With every row recorded and stamp-valid there are no
        // candidates at all, so the proof file is never consulted.
        update_statuses(&mut files, &HashMap::new(), |_| {
            panic!("no candidate rows, so the proof file must not be read")
        });
        assert_eq!(files[1].contents, fresh_bps);
    }
}
