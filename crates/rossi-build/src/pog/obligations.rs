//! The generated obligations read back as typed values.
//!
//! The generator's product is `.bpo` text. Anything that answers a
//! question about an obligation after the build (a report, an
//! agent-facing tool) wants the obligations as values instead: the
//! nature and provenance of each one, and its sequent with the
//! hypothesis-set chain resolved the way the prover resolves it, across
//! the component files a chain crosses. This index reads the generated
//! files once and serves both, so no caller re-parses XML.

use rossi_prove::ProverSequent;
use rossi_prove::po_loader::{PoError, PoFile, PoProject};

use crate::ScFile;
use crate::error::{Error, ProjectError, Result};
use crate::po_view::PoView;

use super::natures::Nature;

/// One component's obligation file: the normalized view for metadata,
/// and the filename its sequents load under.
#[derive(Debug)]
struct ComponentFile {
    component: String,
    filename: String,
    view: PoView,
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
    /// `(role, handle)` provenance rows, handles with their project
    /// segment dropped as [`PoView`] normalizes them.
    pub sources: &'a [(String, Option<String>)],
}

impl Obligations {
    /// Indexes the `.bpo` files among `files`: the product of a build,
    /// or of a repackaging that reconciled one.
    pub fn from_files(files: &[ScFile]) -> Result<Obligations> {
        let mut index = Obligations::default();
        for file in files {
            let Some(component) = file.filename.strip_suffix(".bpo") else {
                continue;
            };
            let view = PoView::from_xml(&file.contents)?;
            let parsed = PoFile::read(file.contents.as_bytes()).map_err(|e| match e {
                PoError::Xml(e) => Error::from(e),
                PoError::Unsupported(what) => ProjectError::XmlTag(what).into(),
            })?;
            index.project.insert(file.filename.clone(), parsed);
            index.components.push(ComponentFile {
                component: component.to_string(),
                filename: file.filename.clone(),
                view,
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
        let (name, sequent) = c.view.sequents.get_key_value(name)?;
        Some(Obligation {
            component: &c.component,
            name,
            nature: Nature::from_description(&sequent.description),
            description: &sequent.description,
            accurate: sequent.accurate,
            stamp: sequent.stamp.as_deref(),
            sources: &sequent.sources,
        })
    }
}
