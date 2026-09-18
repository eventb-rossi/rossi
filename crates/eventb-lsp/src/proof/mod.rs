//! Proof obligations for an open document, generated in-process and judged
//! against whatever proofs are stored beside the sources.
//!
//! `rossi_build::build` over the document's dependency closure generates the
//! obligations; that is the same generator `rossi build` runs, so the list
//! the editor shows is the list Rodin would see. A stored `.bpr` next to the
//! sources or in the shared Rodin workspace project gives each obligation a
//! status the way `rossi prove` computes it: the recorded proof is judged
//! against the freshly generated sequent, so a proof of an obligation that
//! has since changed reports as broken rather than as the discharged it
//! once was. An obligation with no stored proof is unattempted.
//!
//! None of this discharges anything. rossi has no automatic prover; the
//! status is a faithful account of what the stored proofs cover, no more.

pub(crate) mod anchor;
pub mod state;

use std::collections::{BTreeMap, HashMap};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use rossi::Component;
use rossi_build::po_view::PoView;
use rossi_prove::confidence::{Bucket, Confidence};
use rossi_prove::po_loader::PoProject;
use rossi_prove::{Keep, ProofBody, ProofEntry, compute_status, read_bpr};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::component_loader::ComponentLoader;
use crate::document::ParsedDocument;
use crate::lsp_types::*;
use crate::position::{PositionIndex, span_to_range};

/// The custom request that lists a document's obligations.
pub const REQUEST_OBLIGATIONS: &str = "rossi/proofObligations";

/// The notification pushed whenever a document's obligations are recomputed,
/// so a client that shows them does not poll.
pub const NOTIFICATION_STATUS: &str = "$/rossi/proofStatus";

/// What the stored proofs say about one obligation. The vocabulary is
/// `rossi prove`'s, so the editor and the CLI describe a proof the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProofStatus {
    /// The stored proof stands at prover-level confidence.
    Discharged,
    /// The stored proof stands, accepted by a human rather than a prover.
    Reviewed,
    /// A stored proof exists but is incomplete.
    Pending,
    /// No stored proof.
    Unattempted,
    /// The stored proof no longer applies to the regenerated obligation.
    Broken,
    /// The stored proof is in a form rossi cannot read.
    Unsupported,
    /// The obligation itself could not be loaded for judging.
    Error,
}

impl ProofStatus {
    /// Whether the obligation counts as closed: nothing left for anyone to
    /// do. `Reviewed` is closed because a human signed it off, which is how
    /// Rodin treats it too.
    pub fn is_closed(self) -> bool {
        matches!(self, ProofStatus::Discharged | ProofStatus::Reviewed)
    }
}

/// One proof obligation of a document, as the client sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Obligation {
    /// The generator's name for the sequent, e.g. `evt/inv1/INV`.
    pub name: String,
    /// The component the obligation belongs to.
    pub component: String,
    /// The generator's description of what is asked, e.g. "Invariant
    /// preservation".
    pub description: String,
    /// The source element the obligation is about, in the document.
    pub range: Range,
    pub status: ProofStatus,
    /// Whether every element the sequent was built from passed the static
    /// check. An inaccurate obligation may be missing hypotheses.
    pub accurate: bool,
}

/// `rossi/proofObligations` parameters.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofObligationsParams {
    pub text_document: TextDocumentIdentifier,
}

/// `$/rossi/proofStatus` parameters: the full obligation list for one
/// document, replacing whatever the client held for it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofStatusParams {
    pub uri: Url,
    pub obligations: Vec<Obligation>,
}

/// The `$/rossi/proofStatus` notification type, for `Client::send_notification`.
pub enum ProofStatusNotification {}

impl notification::Notification for ProofStatusNotification {
    type Params = ProofStatusParams;
    const METHOD: &'static str = NOTIFICATION_STATUS;
}

/// The obligations of every document that has been computed, read by the
/// diagnostics, the lenses and the custom request.
#[derive(Default)]
pub struct ProofOverlay {
    by_uri: HashMap<Url, Vec<Obligation>>,
}

impl ProofOverlay {
    /// Replace one document's list. Returns whether anything changed, so a
    /// caller republishes only then.
    pub(crate) fn apply(&mut self, uri: Url, obligations: Vec<Obligation>) -> bool {
        if self.by_uri.get(&uri) == Some(&obligations) {
            return false;
        }
        self.by_uri.insert(uri, obligations);
        true
    }

    pub(crate) fn remove(&mut self, uri: &Url) -> bool {
        self.by_uri.remove(uri).is_some()
    }

    pub(crate) fn get(&self, uri: &Url) -> Option<&[Obligation]> {
        self.by_uri.get(uri).map(Vec::as_slice)
    }
}

/// `obligations` with every range recomputed against the current parse of
/// `doc`, for serving a list computed before later edits moved the text.
/// An obligation whose component the current parse no longer recovers
/// keeps the range it had: stale is better than the file start.
pub(crate) fn re_anchor(doc: &ParsedDocument, obligations: &[Obligation]) -> Vec<Obligation> {
    let index = PositionIndex::new(doc.text());
    let components: HashMap<&str, &Component> = doc
        .components()
        .iter()
        .map(|component| (component.name(), component))
        .collect();
    obligations
        .iter()
        .map(|obligation| Obligation {
            range: components
                .get(obligation.component.as_str())
                .map_or(obligation.range, |component| {
                    anchor::range_for(component, &obligation.name, &index)
                }),
            ..obligation.clone()
        })
        .collect()
}

/// Generate and judge the obligations of `doc`.
///
/// A document that does not parse has no obligations: the generator needs
/// a checked model, and a mid-edit syntax error must not blank the list a
/// moment later re-fills, so callers keep the previous list until a clean
/// parse replaces it.
pub(crate) fn compute(
    doc: &ParsedDocument,
    loader: &ComponentLoader,
    sources: &[PathBuf],
) -> Option<Vec<Obligation>> {
    if !doc.parse().errors.is_empty() {
        return None;
    }
    let project = crate::closure::project_for(doc, loader, "lsp-proof");
    let result = rossi_build::build(&project);

    // Stored proofs per local component, read only as far as their
    // dependencies: judging applicability never needs the rule tree.
    let stored: HashMap<&str, HashMap<String, ProofEntry>> = doc
        .components()
        .iter()
        .filter_map(|component| {
            let name = component.name();
            Some((name, stored_proofs(name, sources)?))
        })
        .collect();

    // The regenerated obligations of the whole closure, needed only when
    // there is a proof to judge: hypothesis-set chains cross component
    // files, so a machine's sequent loads against its seen contexts' too.
    let judge =
        (!stored.is_empty()).then(|| rossi_build::pog::status::build_project(&result.files));

    let index = PositionIndex::new(doc.text());
    let mut obligations = Vec::new();
    for component in doc.components() {
        let name = component.name();
        let Some(bpo) = result.file(&format!("{name}.bpo")) else {
            // Dropped by the static check: nothing to prove yet.
            continue;
        };
        let view = match PoView::from_xml(&bpo.contents) {
            Ok(view) => view,
            Err(error) => {
                debug!("cannot read generated obligations of {name}: {error}");
                continue;
            }
        };
        let proofs = stored.get(name);
        for (sequent_name, sequent) in &view.sequents {
            let status = match (judge.as_ref(), proofs.and_then(|p| p.get(sequent_name))) {
                (Some(project), Some(entry)) => judge_proof(project, name, entry),
                _ => ProofStatus::Unattempted,
            };
            obligations.push(Obligation {
                name: sequent_name.clone(),
                component: name.to_string(),
                description: sequent.description.clone(),
                range: anchor::range_for(component, sequent_name, &index),
                status,
                accurate: sequent.accurate,
            });
        }
    }
    Some(obligations)
}

/// The stored proofs of `component`, keyed by obligation name, from the
/// first source that has a `.bpr` for it.
fn stored_proofs(component: &str, sources: &[PathBuf]) -> Option<HashMap<String, ProofEntry>> {
    let file_name = format!("{component}.bpr");
    if !rossi_build::is_normal_path_component(&file_name) {
        return None;
    }
    let path = sources
        .iter()
        .map(|dir| dir.join(&file_name))
        .find(|path| path.is_file())?;
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) => {
            debug!("cannot open {}: {error}", path.display());
            return None;
        }
    };
    match read_bpr(BufReader::new(file), |_| Keep::Deps) {
        Ok(entries) => Some(entries.into_iter().map(|e| (e.name.clone(), e)).collect()),
        Err(error) => {
            debug!("cannot read {}: {error}", path.display());
            None
        }
    }
}

/// `rossi prove`'s verdict for one stored proof: unreadable proofs are
/// unsupported, an obligation that cannot load is an error, and a readable
/// proof is broken or stands at its recorded confidence.
fn judge_proof(project: &PoProject, component: &str, entry: &ProofEntry) -> ProofStatus {
    match &entry.body {
        ProofBody::Skipped => return ProofStatus::Error,
        ProofBody::Unsupported(_) => return ProofStatus::Unsupported,
        ProofBody::Loaded(_) => {}
    }
    let Ok(sequent) = project.load(&format!("{component}.bpo"), &entry.name) else {
        return ProofStatus::Error;
    };
    let verdict = compute_status(&sequent, entry);
    if verdict.broken {
        return ProofStatus::Broken;
    }
    match Confidence::classify(verdict.confidence.map(i64::from)) {
        Bucket::Discharged => ProofStatus::Discharged,
        Bucket::Reviewed => ProofStatus::Reviewed,
        Bucket::Pending => ProofStatus::Pending,
        Bucket::Unattempted => ProofStatus::Unattempted,
    }
}

/// Diagnostics for a document's obligations, one per source element rather
/// than one per obligation, so an invariant preserved by twenty events is
/// one row and not twenty.
///
/// A broken proof is a warning: something the user had was lost to an edit.
/// Open obligations are hints, which editors show inline but keep out of
/// the problems list, because on a fresh model every obligation is open and
/// that is the normal state of affairs, not a problem. Closed obligations
/// produce nothing; the lens and the client's own view carry the count.
pub(crate) fn diagnostics(obligations: &[Obligation]) -> Vec<Diagnostic> {
    // Keyed by position so the output is ordered; `Range` itself is not `Ord`.
    type Grouped<'a> = BTreeMap<(u32, u32, u32, u32), (Range, Vec<&'a str>)>;
    let mut broken: Grouped = BTreeMap::new();
    let mut open: Grouped = BTreeMap::new();
    for obligation in obligations {
        let range = obligation.range;
        let key = (
            range.start.line,
            range.start.character,
            range.end.line,
            range.end.character,
        );
        let group = match obligation.status {
            ProofStatus::Broken => &mut broken,
            status if status.is_closed() => continue,
            _ => &mut open,
        };
        group
            .entry(key)
            .or_insert_with(|| (range, Vec::new()))
            .1
            .push(&obligation.name);
    }

    let mut diagnostics = Vec::new();
    for (range, names) in broken.values() {
        diagnostics.push(crate::diagnostics::lsp_diagnostic(
            *range,
            DiagnosticSeverity::WARNING,
            None,
            format!("stored proof no longer applies to {}", names.join(", ")),
        ));
    }
    for (range, names) in open.values() {
        let noun = if names.len() == 1 {
            "open proof obligation"
        } else {
            "open proof obligations"
        };
        diagnostics.push(crate::diagnostics::lsp_diagnostic(
            *range,
            DiagnosticSeverity::HINT,
            None,
            format!("{} {noun}: {}", names.len(), names.join(", ")),
        ));
    }
    diagnostics
}

/// One lens per component that has obligations, on its name, summarizing
/// how many are closed.
pub(crate) fn code_lenses(
    components: &[Component],
    text: &str,
    obligations: &[Obligation],
) -> Vec<CodeLens> {
    components
        .iter()
        .filter_map(|component| {
            let name = component.name();
            let (closed, total) = obligations
                .iter()
                .filter(|o| o.component == name)
                .fold((0, 0), |(closed, total), o| {
                    (closed + usize::from(o.status.is_closed()), total + 1)
                });
            if total == 0 {
                return None;
            }
            let span = component.name_span().or_else(|| component.span())?;
            Some(CodeLens {
                range: span_to_range(&span, text),
                command: Some(Command {
                    title: format!("{closed}/{total} proof obligations discharged"),
                    // An informational lens: the count is the point, and a
                    // client with an obligation view wires its own reveal
                    // command by matching the title.
                    command: String::new(),
                    arguments: None,
                }),
                data: None,
            })
        })
        .collect()
}

/// Where the stored proofs of the document at `uri` are looked for,
/// freshest first: the resolved shared Rodin workspace project (Rodin
/// writes there live while a session runs), then the document's own
/// directory (the proof mirror's session-end copies, or a plain Rodin
/// export). The same order the animate lens reads recorded proof state in.
pub(crate) fn sources_for(uri: &Url, rodin_project_dir: Option<PathBuf>) -> Vec<PathBuf> {
    rodin_project_dir
        .filter(|dir| dir.is_dir())
        .into_iter()
        .chain(
            uri.to_file_path()
                .ok()
                .and_then(|path| path.parent().map(Path::to_path_buf)),
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obligation(name: &str, line: u32, status: ProofStatus) -> Obligation {
        Obligation {
            name: name.to_string(),
            component: "m".to_string(),
            description: String::new(),
            range: Range::new(Position::new(line, 0), Position::new(line, 5)),
            status,
            accurate: true,
        }
    }

    #[test]
    fn open_obligations_group_per_element_as_hints() {
        let diags = diagnostics(&[
            obligation("e1/inv1/INV", 3, ProofStatus::Unattempted),
            obligation("e2/inv1/INV", 3, ProofStatus::Pending),
            obligation("inv1/WD", 3, ProofStatus::Discharged),
        ]);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::HINT));
        assert_eq!(
            diags[0].message,
            "2 open proof obligations: e1/inv1/INV, e2/inv1/INV"
        );
    }

    #[test]
    fn a_broken_proof_is_a_warning_and_closed_ones_are_silent() {
        let diags = diagnostics(&[
            obligation("inv1/WD", 3, ProofStatus::Broken),
            obligation("inv2/WD", 4, ProofStatus::Reviewed),
        ]);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(diags[0].range.start.line, 3);
    }

    #[test]
    fn the_lens_counts_closed_over_total() {
        let text = "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x \u{2208} \u{2115}\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x \u{2254} 0\n    END\nEND\n";
        let components = crate::component_util::parse_all(text);
        let lenses = code_lenses(
            &components,
            text,
            &[
                obligation("inv1/WD", 4, ProofStatus::Discharged),
                obligation("INITIALISATION/inv1/INV", 4, ProofStatus::Unattempted),
            ],
        );
        assert_eq!(lenses.len(), 1);
        assert_eq!(
            lenses[0].command.as_ref().unwrap().title,
            "1/2 proof obligations discharged"
        );
    }
}
