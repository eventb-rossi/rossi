//! Assembling a `rossi-build` project from a document's dependency closure.
//!
//! Two features need the same thing: the open file's components plus every
//! component they refine, see or extend, taken from current buffer snapshots
//! rather than from disk, packed into a [`rossi_build::Project`] the static
//! checker can run over. Inlay hints need it for the typed model; the
//! semantic diagnostics need it for the checker's findings. Keeping one
//! assembler means the two cannot disagree about what a file's closure is,
//! and a fix to the dedup rules reaches both.

use std::collections::HashSet;

use rossi::deps::kind_and_name;

use crate::component_loader::ComponentLoader;
use crate::document::ParsedDocument;
use crate::resolved_environment::ResolvedEnvironments;

/// The closure of `doc`, roots first, as a project named `name`.
///
/// Dependencies are deduplicated against the roots by kind and name: a merged
/// file's machine may SEES a context sitting right next to it. Roots
/// themselves are never deduplicated, because genuine same-name duplicates
/// must reach the static check so its duplicate-component error empties the
/// model rather than letting a consumer join every copy to one record.
///
/// The returned project carries no `source`: spans on this file's components
/// still index `doc.text()`, and a caller must not read a dependency's spans,
/// which index their own files (and are absent entirely for a dependency
/// imported from Rodin XML).
pub(crate) fn project_for(
    doc: &ParsedDocument,
    loader: &ComponentLoader,
    name: &str,
) -> rossi_build::Project {
    let mut environments = ResolvedEnvironments::new();
    let mut seen = HashSet::new();
    let mut components = Vec::new();

    for component in doc.components() {
        seen.insert(kind_and_name(component));
        components.push(component.clone());
    }
    for component in doc.components() {
        let environment = environments.resolve(component, loader);
        for dependency in environment
            .refined_machines()
            .into_iter()
            .chain(environment.visible_contexts())
            .chain(environment.extended_contexts())
        {
            if seen.insert(kind_and_name(dependency)) {
                components.push(dependency.clone());
            }
        }
    }

    rossi_build::Project::new(
        name,
        components
            .into_iter()
            .map(|component| {
                rossi_build::ProjectComponent::from_parsed(
                    format!("{}.eventb", component.name()),
                    component,
                    None,
                )
            })
            .collect(),
    )
}

/// The names of `doc`'s own components, for telling a finding about this file
/// from one about a dependency whose spans index a different text.
pub(crate) fn local_names(doc: &ParsedDocument) -> HashSet<String> {
    doc.components()
        .iter()
        .map(|component| component.name().to_string())
        .collect()
}
