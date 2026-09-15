//! The generated obligations read back as typed values.
//!
//! The generator's product is `.bpo` text. Anything that answers a
//! question about an obligation after the build (a report, an
//! agent-facing tool) wants the obligations as values instead: the
//! nature and provenance of each one, and its sequent with the
//! hypothesis-set chain resolved the way the prover resolves it, across
//! the component files a chain crosses. Both come out of the reader the
//! prover already uses, so a caller neither re-parses the XML nor pays
//! for the predicates of an obligation it only wanted to list.

use std::collections::HashMap;

use rossi_prove::confidence::Bucket;
use rossi_prove::po_loader::{PoError, PoFile, PoProject};
use rossi_prove::{Confidence, ProverSequent, PsStatus, read_bps};

use crate::ScFile;
use crate::error::{Error, ProjectError, Result};
use crate::proofs::cap_if_broken;

use super::natures::Nature;

/// One component's obligation file: the filename its sequents load
/// under, and the rows of its status sidecar by obligation name.
#[derive(Debug)]
struct ComponentFile {
    component: String,
    filename: String,
    statuses: HashMap<String, PsStatus>,
}

/// The recorded proof status of one obligation, as its `.bps` row
/// reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObligationStatus {
    /// The reporting bucket, a broken row's stale confidence capped to
    /// pending the way the proof-status pass reports it.
    pub bucket: Bucket,
    /// The row's confidence verbatim.
    pub confidence: Option<i32>,
    /// Whether the stored proof no longer applies to the obligation.
    pub broken: bool,
    /// Whether the proof is marked as made by hand.
    pub manual: bool,
    /// Whether the verdict is due for recomputation: the row was
    /// computed against another stamp than the obligation carries now,
    /// or its proof depends on context and is re-checked on every
    /// build.
    pub stale: bool,
}

/// The obligations of one build, components in file order and
/// obligations in document order.
#[derive(Debug, Default)]
pub struct Obligations {
    components: Vec<ComponentFile>,
    project: PoProject,
}

/// What one obligation asks to prove and where it came from.
#[derive(Debug, Clone, Copy)]
pub struct Obligation<'a> {
    /// The component whose file holds the obligation.
    pub component: &'a str,
    /// The obligation's name, e.g. `evt/inv1/INV`.
    pub name: &'a str,
    /// The nature behind the description, when it is one of the
    /// generator's.
    pub nature: Option<Nature>,
    /// The `poDesc` attribute verbatim.
    pub description: &'a str,
    /// Whether every element the obligation depends on passed its checks.
    pub accurate: bool,
    /// The obligation's stamp, when the file carries one.
    pub stamp: Option<&'a str>,
    /// The elements the obligation traces back to: `(role, handle)`
    /// rows in document order, the handles as the file spells them.
    pub sources: &'a [(String, String)],
    /// The recorded status, when the status sidecar has a row for the
    /// obligation.
    pub status: Option<ObligationStatus>,
}

impl Obligations {
    /// Indexes the `.bpo` files among `files`, each with the rows of its
    /// `.bps` sidecar: the product of a build, or of a repackaging that
    /// reconciled one.
    pub fn from_files(files: &[ScFile]) -> Result<Obligations> {
        let mut statuses = HashMap::new();
        for file in files {
            if let Some(component) = file.filename.strip_suffix(".bps") {
                let rows = read_bps(file.contents.as_bytes())?;
                let by_name: HashMap<String, PsStatus> = rows
                    .into_iter()
                    .map(|row| (row.name.clone(), row))
                    .collect();
                statuses.insert(component.to_string(), by_name);
            }
        }

        let mut index = Obligations::default();
        for file in files {
            let Some(component) = file.filename.strip_suffix(".bpo") else {
                continue;
            };
            let parsed = PoFile::read(file.contents.as_bytes()).map_err(|e| match e {
                PoError::Xml(e) => Error::from(e),
                PoError::Unsupported(what) => ProjectError::XmlTag(what).into(),
            })?;
            index.project.insert(file.filename.clone(), parsed);
            index.components.push(ComponentFile {
                component: component.to_string(),
                filename: file.filename.clone(),
                statuses: statuses.remove(component).unwrap_or_default(),
            });
        }
        Ok(index)
    }

    /// The components that have an obligation file, in file order.
    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.components.iter().map(|c| c.component.as_str())
    }

    /// Every obligation, components in file order and obligations in
    /// document order.
    pub fn iter(&self) -> impl Iterator<Item = Obligation<'_>> {
        self.components.iter().flat_map(move |c| {
            self.project
                .file(&c.filename)
                .into_iter()
                .flat_map(|file| file.sequents())
                .filter_map(move |entry| self.describe(c, &entry.name))
        })
    }

    /// The named obligation of `component`.
    pub fn get(&self, component: &str, name: &str) -> Option<Obligation<'_>> {
        self.describe(self.component(component)?, name)
    }

    /// The obligation's sequent as the prover loads it: the typed
    /// environment, the hypotheses of the resolved chain with their
    /// well-definedness conjuncts, the selection the hints make, and
    /// the goal.
    pub fn sequent(
        &self,
        component: &str,
        name: &str,
    ) -> std::result::Result<ProverSequent, String> {
        let c = self
            .component(component)
            .ok_or_else(|| format!("no component named `{component}`"))?;
        self.project.load(&c.filename, name)
    }

    fn component(&self, component: &str) -> Option<&ComponentFile> {
        self.components.iter().find(|c| c.component == component)
    }

    fn describe<'a>(&'a self, c: &'a ComponentFile, name: &str) -> Option<Obligation<'a>> {
        let sequent = self.project.file(&c.filename)?.sequent(name)?;
        let name = sequent.name.as_str();
        let status = c.statuses.get(name).map(|row| ObligationStatus {
            bucket: cap_if_broken(
                Confidence::classify(row.confidence.map(i64::from)),
                row.broken,
            ),
            confidence: row.confidence,
            broken: row.broken,
            manual: row.manual,
            stale: row.context_dependent || row.po_stamp.as_deref() != sequent.stamp.as_deref(),
        });
        Some(Obligation {
            component: &c.component,
            name,
            nature: Nature::from_description(&sequent.description),
            description: &sequent.description,
            accurate: sequent.accurate,
            stamp: sequent.stamp.as_deref(),
            sources: &sequent.sources,
            status,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Four obligations at stamp 2.
    const BPO: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.poFile org.eventb.core.poStamp="0">
<org.eventb.core.poPredicateSet name="ALLHYP" org.eventb.core.poStamp="0">
<org.eventb.core.poIdentifier name="x" org.eventb.core.type="ℤ"/>
<org.eventb.core.poPredicate name="PRD0" org.eventb.core.predicate="x=1"/>
</org.eventb.core.poPredicateSet>
<org.eventb.core.poSequent name="evt/inv1/INV" org.eventb.core.accurate="true" org.eventb.core.poDesc="Invariant  preservation" org.eventb.core.poStamp="2">
<org.eventb.core.poPredicateSet name="SEQHYP" org.eventb.core.parentSet="/P/M0.bpo|org.eventb.core.poFile#M0|org.eventb.core.poPredicateSet#ALLHYP"/>
<org.eventb.core.poPredicate name="SEQG" org.eventb.core.predicate="x&lt;2"/>
</org.eventb.core.poSequent>
<org.eventb.core.poSequent name="evt/inv2/INV" org.eventb.core.accurate="true" org.eventb.core.poDesc="Invariant  preservation" org.eventb.core.poStamp="2">
<org.eventb.core.poPredicateSet name="SEQHYP" org.eventb.core.parentSet="/P/M0.bpo|org.eventb.core.poFile#M0|org.eventb.core.poPredicateSet#ALLHYP"/>
<org.eventb.core.poPredicate name="SEQG" org.eventb.core.predicate="x&lt;3"/>
</org.eventb.core.poSequent>
<org.eventb.core.poSequent name="evt/inv3/INV" org.eventb.core.accurate="true" org.eventb.core.poDesc="Invariant  preservation" org.eventb.core.poStamp="2">
<org.eventb.core.poPredicateSet name="SEQHYP" org.eventb.core.parentSet="/P/M0.bpo|org.eventb.core.poFile#M0|org.eventb.core.poPredicateSet#ALLHYP"/>
<org.eventb.core.poPredicate name="SEQG" org.eventb.core.predicate="x&lt;4"/>
</org.eventb.core.poSequent>
<org.eventb.core.poSequent name="evt/inv4/INV" org.eventb.core.accurate="true" org.eventb.core.poDesc="Invariant  preservation" org.eventb.core.poStamp="2">
<org.eventb.core.poPredicateSet name="SEQHYP" org.eventb.core.parentSet="/P/M0.bpo|org.eventb.core.poFile#M0|org.eventb.core.poPredicateSet#ALLHYP"/>
<org.eventb.core.poPredicate name="SEQG" org.eventb.core.predicate="x&lt;5"/>
</org.eventb.core.poSequent>
</org.eventb.core.poFile>
"#;

    /// inv1 discharged at the current stamp; inv2 discharged but broken;
    /// inv3 computed against an older stamp; inv4 has no row.
    const BPS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.psFile>
<org.eventb.core.psStatus name="evt/inv1/INV" org.eventb.core.confidence="1000" org.eventb.core.poStamp="2" org.eventb.core.psManual="false"/>
<org.eventb.core.psStatus name="evt/inv2/INV" org.eventb.core.confidence="1000" org.eventb.core.poStamp="2" org.eventb.core.psBroken="true" org.eventb.core.psManual="true"/>
<org.eventb.core.psStatus name="evt/inv3/INV" org.eventb.core.confidence="1000" org.eventb.core.poStamp="1" org.eventb.core.psManual="false"/>
</org.eventb.core.psFile>
"#;

    fn files() -> Vec<ScFile> {
        // The status sidecar first: the index must not depend on the
        // build's obligations-then-status file order.
        vec![
            ScFile {
                filename: "M0.bps".into(),
                contents: BPS.into(),
                accurate: true,
            },
            ScFile {
                filename: "M0.bpo".into(),
                contents: BPO.into(),
                accurate: true,
            },
        ]
    }

    #[test]
    fn a_status_row_joins_its_obligation() {
        let index = Obligations::from_files(&files()).unwrap();
        let status = |name: &str| index.get("M0", name).unwrap().status;

        assert_eq!(
            status("evt/inv1/INV"),
            Some(ObligationStatus {
                bucket: Bucket::Discharged,
                confidence: Some(1000),
                broken: false,
                manual: false,
                stale: false,
            })
        );
        // A broken row keeps its recorded confidence but reports as
        // pending, and carries its manual flag.
        assert_eq!(
            status("evt/inv2/INV"),
            Some(ObligationStatus {
                bucket: Bucket::Pending,
                confidence: Some(1000),
                broken: true,
                manual: true,
                stale: false,
            })
        );
        // A row computed against another stamp is due for recomputation.
        assert_eq!(
            status("evt/inv3/INV"),
            Some(ObligationStatus {
                bucket: Bucket::Discharged,
                confidence: Some(1000),
                broken: false,
                manual: false,
                stale: true,
            })
        );
        assert_eq!(status("evt/inv4/INV"), None);

        let names: Vec<&str> = index.iter().map(|po| po.name).collect();
        assert_eq!(
            names,
            [
                "evt/inv1/INV",
                "evt/inv2/INV",
                "evt/inv3/INV",
                "evt/inv4/INV"
            ]
        );
    }
}
