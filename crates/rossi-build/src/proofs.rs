//! Proof-status checks (EB015–EB017) from Rodin proof files.
//!
//! Rodin keeps proof evidence next to a project's `.buc`/`.bum` sources:
//! `.bpo` (generated proof obligations), `.bpr` (the proofs themselves) and
//! `.bps` (per-obligation status). This module joins the three and reports
//! undischarged obligations (EB015), broken proofs (EB016) and unparseable
//! proof files (EB017), mirroring eventb-checker's `ProofStatusChecker` so
//! findings stay byte-identical with its output.
//!
//! Following eventb-checker 1.6, each obligation's confidence is taken from
//! its `.bps` status entry (the `.bpr` proof confidence is only a fallback
//! when no `.bps` mentions it), and a *broken* obligation — which keeps the
//! stale, often discharged-level confidence of its now-invalid proof — is
//! counted `pending` rather than discharged/reviewed and is reported once,
//! as EB016. Reading the status rather than the proof also sidesteps the
//! deeply-nested `.bpr` proof trees that earlier oracles choked on.
//!
//! The pass is independent of [`crate::build`]: proof files are scraped
//! directly from the zip / directory, so it still runs when component
//! parsing failed. When the input contains no proof files at all,
//! [`ProofReport::summary`] is `None` and no diagnostics are produced —
//! callers can treat that as "nothing to check" without a flag.

use std::collections::HashMap;
use std::io::{BufRead, Read, Seek};
use std::path::Path;

use quick_xml::events::{BytesStart, Event as XmlEvent};
use quick_xml::{Reader, XmlVersion};

use crate::rules::RuleId;
use crate::{Diagnostic, Severity, error::Result};

/// Proof-obligation counts across the whole input, mirroring
/// eventb-checker's `proofSummary` JSON object.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProofSummary {
    pub total: usize,
    pub discharged: usize,
    pub reviewed: usize,
    pub pending: usize,
    pub unattempted: usize,
    /// Obligations whose proof script is stale (`psBroken="true"`). A broken
    /// proof can still be discharged, so this is not disjoint from the
    /// other counts.
    pub broken: usize,
}

/// The outcome of the proof-status pass.
#[derive(Debug, Default)]
pub struct ProofReport {
    /// EB017 parse errors first, then EB015 per undischarged obligation,
    /// then EB016 per broken proof — eventb-checker's emission order.
    pub diagnostics: Vec<Diagnostic>,
    /// `None` when the input contained no `.bpr`/`.bpo`/`.bps` files.
    pub summary: Option<ProofSummary>,
}

/// Rodin's proof confidence buckets. Thresholds are eventb-checker's
/// (`>500` discharged, `101..=500` reviewed, `0..=100` pending, absent or
/// negative unattempted).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Confidence {
    Discharged,
    Reviewed,
    Pending,
    Unattempted,
}

impl Confidence {
    fn classify(confidence: Option<i64>) -> Self {
        match confidence {
            None => Confidence::Unattempted,
            Some(c) if c > 500 => Confidence::Discharged,
            Some(c) if c >= 101 => Confidence::Reviewed,
            Some(c) if c >= 0 => Confidence::Pending,
            Some(_) => Confidence::Unattempted,
        }
    }

    /// A broken proof keeps its stale (often high) confidence in the
    /// `.bps`, but eventb-checker 1.6 does not count it as discharged or
    /// reviewed: such an obligation is reported `pending`. Lower
    /// classifications (already pending / unattempted) are left untouched.
    fn cap_if_broken(self, broken: bool) -> Self {
        if broken && matches!(self, Confidence::Discharged | Confidence::Reviewed) {
            Confidence::Pending
        } else {
            self
        }
    }

    fn label(self) -> &'static str {
        match self {
            Confidence::Discharged => "discharged",
            Confidence::Reviewed => "reviewed",
            Confidence::Pending => "pending",
            Confidence::Unattempted => "unattempted",
        }
    }
}

/// Check proof status of a Rodin `.zip` archive on disk.
pub fn check_zip_file(path: &Path) -> Result<ProofReport> {
    let file = std::fs::File::open(path)?;
    check_zip_archive(std::io::BufReader::new(file))
}

/// Check proof status of a zip archive already in memory.
pub fn check_zip_bytes(data: &[u8]) -> Result<ProofReport> {
    check_zip_archive(std::io::Cursor::new(data))
}

/// Check proof status of a directory of Rodin files (shallow, mirroring
/// [`crate::Project::from_directory`]).
pub fn check_directory(dir: &Path) -> Result<ProofReport> {
    let mut names: Vec<String> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .collect();
    // read_dir order is OS-dependent; sort for deterministic output.
    names.sort();

    let mut data = ProofData::default();
    // Process grouped by kind (.bpr, .bpo, .bps) like eventb-checker: the
    // group order fixes both EB017 ordering and obligation order.
    for kind in [FileKind::Proof, FileKind::Obligations, FileKind::Status] {
        for name in names.iter().filter(|n| FileKind::of(n) == Some(kind)) {
            match std::fs::File::open(dir.join(name)) {
                Ok(f) => data.ingest(kind, name, std::io::BufReader::new(f)),
                Err(e) => data.parse_error(name, &e.to_string()),
            }
        }
    }
    Ok(data.finish())
}

fn check_zip_archive<R: Read + Seek>(reader: R) -> Result<ProofReport> {
    let mut archive = zip::ZipArchive::new(reader)?;
    // Classify every entry in one pass, then ingest grouped by kind (.bpr,
    // .bpo, .bps) — the same grouping that fixes EB017 and obligation
    // ordering, but without re-scanning the whole archive once per kind.
    let mut by_kind: [Vec<usize>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for i in 0..archive.len() {
        let name = archive.by_index(i)?.name().to_string();
        if let Some(kind) = FileKind::of(&name) {
            by_kind[kind as usize].push(i);
        }
    }
    let mut data = ProofData::default();
    for kind in [FileKind::Proof, FileKind::Obligations, FileKind::Status] {
        for &i in &by_kind[kind as usize] {
            let entry = archive.by_index(i)?;
            let entry_path = entry.name().to_string();
            data.ingest(kind, &entry_path, std::io::BufReader::new(entry));
        }
    }
    Ok(data.finish())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    /// `.bpr` — proofs with their confidence.
    Proof,
    /// `.bpo` — generated proof obligations.
    Obligations,
    /// `.bps` — per-obligation status (broken flag).
    Status,
}

impl FileKind {
    fn of(path: &str) -> Option<Self> {
        if path.ends_with(".bpr") {
            Some(FileKind::Proof)
        } else if path.ends_with(".bpo") {
            Some(FileKind::Obligations)
        } else if path.ends_with(".bps") {
            Some(FileKind::Status)
        } else {
            None
        }
    }

    /// The root-level child element carrying one obligation entry.
    fn entry_tag(self) -> &'static [u8] {
        match self {
            FileKind::Proof => b"org.eventb.core.prProof",
            FileKind::Obligations => b"org.eventb.core.poSequent",
            FileKind::Status => b"org.eventb.core.psStatus",
        }
    }
}

/// One root-level entry scraped from a proof file: the obligation name plus
/// the kind-specific payload.
struct ProofEntry {
    component: String,
    name: String,
    /// `.bpr` only: `org.eventb.core.confidence`.
    confidence: Option<i64>,
    /// `.bps` only: `org.eventb.core.psBroken`.
    broken: bool,
}

impl ProofEntry {
    fn key(&self) -> String {
        format!("{}/{}", self.component, self.name)
    }
}

#[derive(Default)]
struct ProofData {
    pr: Vec<ProofEntry>,
    po: Vec<ProofEntry>,
    ps: Vec<ProofEntry>,
    parse_errors: Vec<Diagnostic>,
    saw_proof_file: bool,
}

impl ProofData {
    /// Parse one proof file. On XML failure the file contributes an EB017
    /// diagnostic and none of its entries (all-or-nothing, like
    /// eventb-checker's DOM parse).
    fn ingest<R: BufRead>(&mut self, kind: FileKind, path: &str, reader: R) {
        self.saw_proof_file = true;
        let component = component_of(path);
        match parse_proof_xml(reader, kind, &component) {
            Ok(mut entries) => match kind {
                FileKind::Proof => self.pr.append(&mut entries),
                FileKind::Obligations => self.po.append(&mut entries),
                FileKind::Status => self.ps.append(&mut entries),
            },
            Err(detail) => self.parse_error(path, &detail),
        }
    }

    fn parse_error(&mut self, path: &str, detail: &str) {
        self.parse_errors.push(Diagnostic {
            severity: Severity::Warning,
            origin: path.to_string(),
            message: format!("Failed to parse proof file: {detail}"),
            rule_id: Some(RuleId::ProofFileParseError),
        });
    }

    fn finish(self) -> ProofReport {
        if !self.saw_proof_file {
            return ProofReport::default();
        }

        // `.bps` is authoritative for both confidence and the broken flag;
        // `.bpr` confidence is only a fallback for obligations no `.bps`
        // entry mentions (a degenerate proofs-only project).
        let ps_status: HashMap<String, (Option<i64>, bool)> = self
            .ps
            .iter()
            .map(|e| (e.key(), (e.confidence, e.broken)))
            .collect();
        let pr_confidence: HashMap<String, Option<i64>> =
            self.pr.iter().map(|e| (e.key(), e.confidence)).collect();

        // Obligation list: `.bpo`, else the `.bps` statuses, else the `.bpr`
        // proofs — the first kind the project actually shipped.
        let sources = if !self.po.is_empty() {
            &self.po
        } else if !self.ps.is_empty() {
            &self.ps
        } else {
            &self.pr
        };
        let obligations: Vec<(String, String, Confidence, bool)> = sources
            .iter()
            .map(|src| {
                let key = src.key();
                let (raw_confidence, broken) = match ps_status.get(&key) {
                    Some(&(conf, broken)) => (conf, broken),
                    None => (pr_confidence.get(&key).copied().flatten(), false),
                };
                let confidence = Confidence::classify(raw_confidence).cap_if_broken(broken);
                (src.component.clone(), src.name.clone(), confidence, broken)
            })
            .collect();

        let mut summary = ProofSummary {
            total: obligations.len(),
            ..ProofSummary::default()
        };
        for (_, _, confidence, broken) in &obligations {
            match confidence {
                Confidence::Discharged => summary.discharged += 1,
                Confidence::Reviewed => summary.reviewed += 1,
                Confidence::Pending => summary.pending += 1,
                Confidence::Unattempted => summary.unattempted += 1,
            }
            if *broken {
                summary.broken += 1;
            }
        }

        let mut diagnostics = self.parse_errors;
        for (component, name, confidence, broken) in &obligations {
            // A broken obligation is reported once, as EB016 below — not
            // also as EB015.
            if !*broken && *confidence != Confidence::Discharged {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    origin: component.clone(),
                    message: format!(
                        "Proof obligation not discharged: {name} ({})",
                        confidence.label()
                    ),
                    rule_id: Some(RuleId::UndischargedProof),
                });
            }
        }
        for (component, name, _, broken) in &obligations {
            if *broken {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    origin: component.clone(),
                    message: format!("Broken proof: {name}"),
                    rule_id: Some(RuleId::BrokenProof),
                });
            }
        }

        ProofReport {
            diagnostics,
            summary: Some(summary),
        }
    }
}

/// Component name = file basename minus extension (`a/b/M1.bpr` → `M1`).
fn component_of(path: &str) -> String {
    let basename = crate::project::basename(path);
    basename
        .rsplit_once('.')
        .map_or(basename, |(stem, _)| stem)
        .to_string()
}

/// Stream-parse one proof file, returning its root-level entries.
///
/// Only depth-1 children matching the kind's entry tag are read; their
/// subtrees are skipped without buffering — `.bpr` files reach tens of
/// megabytes and the payload we need is all in root-child attributes.
fn parse_proof_xml<R: BufRead>(
    input: R,
    kind: FileKind,
    component: &str,
) -> std::result::Result<Vec<ProofEntry>, String> {
    let mut reader = Reader::from_reader(input);
    let mut buf = Vec::new();
    let mut skip = Vec::new();
    let mut entries = Vec::new();
    let mut in_root = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(e)) => {
                if !in_root {
                    // The document element; its direct children are the entries.
                    in_root = true;
                } else {
                    // A root-level child: extract it if it is an entry, then
                    // skip its whole subtree in one go. `read_to_end_into`
                    // still parses (so a malformed subtree is reported EB017)
                    // but never materializes the deep proof script — the
                    // payload we need is all in root-child attributes.
                    if e.name().as_ref() == kind.entry_tag() {
                        extract_entry(&e, kind, component, &mut entries)?;
                    }
                    let end = e.to_end().into_owned();
                    reader
                        .read_to_end_into(end.name(), &mut skip)
                        .map_err(|e| e.to_string())?;
                }
            }
            Ok(XmlEvent::Empty(e)) => {
                if in_root && e.name().as_ref() == kind.entry_tag() {
                    extract_entry(&e, kind, component, &mut entries)?;
                }
            }
            Ok(XmlEvent::Eof) => break,
            Err(e) => return Err(e.to_string()),
            _ => {}
        }
        buf.clear();
    }
    Ok(entries)
}

fn extract_entry(
    e: &BytesStart,
    kind: FileKind,
    component: &str,
    entries: &mut Vec<ProofEntry>,
) -> std::result::Result<(), String> {
    let mut name = String::new();
    let mut confidence = None;
    let mut broken = false;

    for attr in e.attributes() {
        let attr = attr.map_err(|e| e.to_string())?;
        let value = || -> std::result::Result<String, String> {
            attr.normalized_value(XmlVersion::Implicit1_0)
                .map(|v| v.into_owned())
                .map_err(|e| e.to_string())
        };
        match attr.key.as_ref() {
            b"name" => name = value()?,
            // eventb-checker 1.6 takes each obligation's confidence from its
            // `.bps` status entry; the `.bpr` proof confidence is only a
            // fallback for obligations a `.bps` never mentions.
            b"org.eventb.core.confidence"
                if kind == FileKind::Status || kind == FileKind::Proof =>
            {
                confidence = value()?.parse::<i64>().ok();
            }
            b"org.eventb.core.psBroken" if kind == FileKind::Status => {
                broken = value()? == "true";
            }
            _ => {}
        }
    }

    // Entries without a name can't be joined to anything; skip them.
    if !name.is_empty() {
        entries.push(ProofEntry {
            component: component.to_string(),
            name,
            confidence,
            broken,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bps(body: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<org.eventb.core.psFile>{body}</org.eventb.core.psFile>"
        )
    }

    fn bpr(body: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<org.eventb.core.prFile version=\"1\">{body}</org.eventb.core.prFile>"
        )
    }

    fn bpo(body: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<org.eventb.core.poFile>{body}</org.eventb.core.poFile>"
        )
    }

    fn zip_of(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        let mut zw = zip::ZipWriter::new(&mut cursor);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
        for (name, body) in entries {
            zw.start_file(*name, opts).unwrap();
            std::io::Write::write_all(&mut zw, body.as_bytes()).unwrap();
        }
        zw.finish().unwrap();
        cursor.into_inner()
    }

    #[test]
    fn classification_thresholds_match_eventb_checker() {
        assert_eq!(Confidence::classify(Some(501)), Confidence::Discharged);
        assert_eq!(Confidence::classify(Some(1000)), Confidence::Discharged);
        assert_eq!(Confidence::classify(Some(500)), Confidence::Reviewed);
        assert_eq!(Confidence::classify(Some(101)), Confidence::Reviewed);
        assert_eq!(Confidence::classify(Some(100)), Confidence::Pending);
        assert_eq!(Confidence::classify(Some(0)), Confidence::Pending);
        assert_eq!(Confidence::classify(Some(-1)), Confidence::Unattempted);
        assert_eq!(Confidence::classify(None), Confidence::Unattempted);
    }

    #[test]
    fn no_proof_files_means_no_summary() {
        let data = zip_of(&[("M0.bum", "<org.eventb.core.machineFile/>")]);
        let report = check_zip_bytes(&data).unwrap();
        assert!(report.summary.is_none());
        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn bpo_is_the_obligation_source_when_present() {
        // Two obligations in the .bpo; only one has a proof in the .bpr.
        let data = zip_of(&[
            (
                "p/M0.bpo",
                &bpo(r#"<org.eventb.core.poSequent name="inv1/INV"/>
                       <org.eventb.core.poSequent name="inv2/INV"/>"#),
            ),
            (
                "p/M0.bpr",
                &bpr(
                    r#"<org.eventb.core.prProof name="inv1/INV" org.eventb.core.confidence="1000"/>"#,
                ),
            ),
        ]);
        let report = check_zip_bytes(&data).unwrap();
        let summary = report.summary.unwrap();
        assert_eq!(summary.total, 2);
        assert_eq!(summary.discharged, 1);
        assert_eq!(summary.unattempted, 1);
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(
            report.diagnostics[0].message,
            "Proof obligation not discharged: inv2/INV (unattempted)"
        );
        assert_eq!(report.diagnostics[0].origin, "M0");
        assert_eq!(
            report.diagnostics[0].rule_id,
            Some(RuleId::UndischargedProof)
        );
    }

    #[test]
    fn bpr_only_projects_use_proofs_as_obligations() {
        let data = zip_of(&[(
            "M0.bpr",
            &bpr(
                r#"<org.eventb.core.prProof name="inv1/INV" org.eventb.core.confidence="1000"/>
                   <org.eventb.core.prProof name="grd1/GRD" org.eventb.core.confidence="42"/>"#,
            ),
        )]);
        let report = check_zip_bytes(&data).unwrap();
        let summary = report.summary.unwrap();
        assert_eq!(summary.total, 2);
        assert_eq!(summary.discharged, 1);
        assert_eq!(summary.pending, 1);
        assert_eq!(
            report.diagnostics[0].message,
            "Proof obligation not discharged: grd1/GRD (pending)"
        );
    }

    #[test]
    fn broken_high_confidence_is_capped_to_pending() {
        // A broken obligation keeps its stale high `.bps` confidence, but
        // 1.6 counts it pending (not discharged) and reports it once as
        // EB016 — never also as EB015.
        let data = zip_of(&[
            (
                "M0.bpo",
                &bpo(r#"<org.eventb.core.poSequent name="inv1/INV"/>"#),
            ),
            (
                "M0.bps",
                &bps(
                    r#"<org.eventb.core.psStatus name="inv1/INV" org.eventb.core.confidence="1000" org.eventb.core.psBroken="true" org.eventb.core.psManual="false"/>"#,
                ),
            ),
        ]);
        let report = check_zip_bytes(&data).unwrap();
        let summary = report.summary.unwrap();
        assert_eq!(summary.discharged, 0);
        assert_eq!(summary.pending, 1);
        assert_eq!(summary.broken, 1);
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.diagnostics[0].message, "Broken proof: inv1/INV");
        assert_eq!(report.diagnostics[0].rule_id, Some(RuleId::BrokenProof));
    }

    #[test]
    fn malformed_proof_file_reports_eb017_and_continues() {
        let data = zip_of(&[
            ("Bad.bps", "<org.eventb.core.psFile><unclosed"),
            (
                "M0.bpo",
                &bpo(r#"<org.eventb.core.poSequent name="inv1/INV"/>"#),
            ),
        ]);
        let report = check_zip_bytes(&data).unwrap();
        assert_eq!(report.summary.unwrap().total, 1);
        let eb017: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.rule_id == Some(RuleId::ProofFileParseError))
            .collect();
        assert_eq!(eb017.len(), 1);
        assert!(eb017[0].message.starts_with("Failed to parse proof file: "));
        assert_eq!(eb017[0].origin, "Bad.bps");
    }

    #[test]
    fn entries_without_name_are_skipped() {
        let data = zip_of(&[(
            "M0.bpo",
            &bpo(r#"<org.eventb.core.poSequent/>
                   <org.eventb.core.poSequent name="inv1/INV"/>"#),
        )]);
        let report = check_zip_bytes(&data).unwrap();
        assert_eq!(report.summary.unwrap().total, 1);
    }

    #[test]
    fn nested_elements_are_not_obligations() {
        // poSequent must be a root child; deeper occurrences are skipped.
        let data = zip_of(&[(
            "M0.bpo",
            &bpo(r#"<org.eventb.core.poSequent name="inv1/INV">
                       <org.eventb.core.poSequent name="nested/INV"/>
                   </org.eventb.core.poSequent>"#),
        )]);
        let report = check_zip_bytes(&data).unwrap();
        assert_eq!(report.summary.unwrap().total, 1);
    }

    #[test]
    fn confidence_comes_from_bps_status() {
        // eventb-checker 1.6 classifies by the `.bps` confidence: a high
        // status confidence discharges the obligation even with no `.bpr`.
        let data = zip_of(&[
            (
                "M0.bpo",
                &bpo(r#"<org.eventb.core.poSequent name="inv1/INV"/>"#),
            ),
            (
                "M0.bps",
                &bps(
                    r#"<org.eventb.core.psStatus name="inv1/INV" org.eventb.core.confidence="1000" org.eventb.core.psBroken="false"/>"#,
                ),
            ),
        ]);
        let report = check_zip_bytes(&data).unwrap();
        assert_eq!(report.summary.unwrap().discharged, 1);
        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn bps_confidence_overrides_bpr() {
        // The `.bpr` proof says discharged (1000) but the `.bps` status says
        // pending (50): the status wins.
        let data = zip_of(&[
            ("M0.bpo", &bpo(r#"<org.eventb.core.poSequent name="po1"/>"#)),
            (
                "M0.bpr",
                &bpr(r#"<org.eventb.core.prProof name="po1" org.eventb.core.confidence="1000"/>"#),
            ),
            (
                "M0.bps",
                &bps(
                    r#"<org.eventb.core.psStatus name="po1" org.eventb.core.confidence="50" org.eventb.core.psBroken="false"/>"#,
                ),
            ),
        ]);
        let report = check_zip_bytes(&data).unwrap();
        let summary = report.summary.unwrap();
        assert_eq!(summary.pending, 1);
        assert_eq!(summary.discharged, 0);
    }

    #[test]
    fn bpr_confidence_is_fallback_without_a_bps_entry() {
        // No `.bps` entry for the obligation: fall back to the `.bpr` proof
        // confidence so proofs-only projects still classify.
        let data = zip_of(&[
            ("M0.bpo", &bpo(r#"<org.eventb.core.poSequent name="po1"/>"#)),
            (
                "M0.bpr",
                &bpr(r#"<org.eventb.core.prProof name="po1" org.eventb.core.confidence="1000"/>"#),
            ),
        ]);
        let report = check_zip_bytes(&data).unwrap();
        assert_eq!(report.summary.unwrap().discharged, 1);
    }

    #[test]
    fn component_join_is_per_file_stem() {
        // Same obligation name in two components must not cross-join.
        let data = zip_of(&[
            (
                "M0.bpo",
                &bpo(r#"<org.eventb.core.poSequent name="inv1/INV"/>"#),
            ),
            (
                "M1.bpr",
                &bpr(
                    r#"<org.eventb.core.prProof name="inv1/INV" org.eventb.core.confidence="1000"/>"#,
                ),
            ),
        ]);
        let report = check_zip_bytes(&data).unwrap();
        let summary = report.summary.unwrap();
        assert_eq!(summary.total, 1);
        assert_eq!(summary.unattempted, 1);
        assert_eq!(summary.discharged, 0);
    }
}
