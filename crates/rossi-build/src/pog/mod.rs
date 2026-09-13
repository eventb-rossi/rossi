//! Proof-obligation generation over the typed checked model.
//!
//! Consumes the [`crate::sc_model::ScModel`] a build produces and
//! emits one `.bpo` file per component, describing what must be proved
//! for the model to be correct: well-definedness of its formulas,
//! preservation of its invariants, feasibility of its actions,
//! refinement of its abstraction, and convergence of its events.

mod context;
mod event;
mod hyp;
mod machine;
mod model;
pub mod natures;
pub mod obligations;
pub mod reconcile;
pub mod status;
mod tables;

use rossi::Component;

use crate::ScFile;
use crate::project::Project;
use crate::sc::ScModel;

/// Generate the proof-obligation files of every successfully-checked
/// component, in project order.
pub fn generate(project: &Project, model: &ScModel) -> Vec<ScFile> {
    let mut files = Vec::new();
    for pc in &project.components {
        let name = pc.component.name();
        match &pc.component {
            Component::Context(_) => {
                if let Some(checked) = model.contexts.get(name) {
                    let (obligations, status) = context::generate(project, model, checked);
                    files.push(obligations);
                    files.push(status);
                }
            }
            Component::Machine(_) => {
                if let Some(checked) = model.machines.get(name) {
                    // A decomposition base machine is an attribute-only
                    // stub: its proof files are empty stubs too, carrying
                    // the same file-level inaccuracy as the checked stub.
                    let (obligations, status) = if crate::sc::machine::is_decomposition_stub_config(
                        &checked.record.configuration,
                    ) {
                        model::PoFile::new(&project.name, checked.name()).into_sc_files(false)
                    } else {
                        machine::generate(project, model, checked)
                    };
                    files.push(obligations);
                    files.push(status);
                }
            }
        }
    }
    files
}
