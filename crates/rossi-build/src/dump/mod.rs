//! The `rossi-model` document: rossi's checked model as JSON.
//!
//! # Why this exists
//!
//! Rodin has no tree export of its typed abstract syntax. Every path it
//! persists through collapses a formula back to surface syntax and stores the
//! types beside it as a table of identifier names, so a tool that wants the
//! tree has to parse and type-check the text again to recover what the
//! checker already knew. Rossi's own checked files are the same, because they
//! are Rodin's. This document is the missing half: the checked model with its
//! formulas still trees, in Rodin's own vocabulary, so a consumer can read
//! what a specification means without reimplementing the front end.
//!
//! # What a document contains
//!
//! One project, shaped like Rodin's checked files. Contexts carry their
//! carrier sets, constants and axioms; machines carry the closure of what is
//! visible in them, which is the point of the checked layer: inherited
//! invariants, the whole chain of an extended event, and the contexts seen
//! transitively. Elements record where each part was written and which
//! machine wrote it, so a consumer that wants only what one machine declared
//! can still recover that.
//!
//! # The node vocabulary
//!
//! Every expression, predicate and declaration is one node object. It states
//! Rodin's numeric `tag` and Rodin's constant name as `op`, its solved
//! `type`, its `span`, and its `children`. Fields that do not apply to an
//! operator are absent rather than null.
//!
//! Children follow Rodin's child positions, so a path through `children` is
//! a path through the formula in Rodin too, and a consumer can address one
//! and the same subformula in either.
//!
//! Bound variables are de Bruijn indices, as in Rodin: `index` counts binders
//! outward from 0, and within one declaration list index 0 is the last
//! declaration. Each occurrence also carries the `name` its declaration
//! resolves to, which is what the formula would print as, so a consumer that
//! works by name never has to compute an index.
//!
//! Assignments have no child positions in Rodin either, so instead of a
//! numbered list they name their parts: `values`, `set`, or `primed` and
//! `pred`. Each action also carries `ba`, the same action as a predicate
//! relating the before and after states, where an after-state value is an
//! ordinary primed identifier.
//!
//! # Types
//!
//! A node's `type` is Rodin's canonical string for the type, and also a key
//! into the document's `types` table, which holds each one as a nested tree.
//! One lookup yields a whole type; nothing has to be resolved step by step.
//!
//! # Positions
//!
//! A `span` gives byte offsets with an exclusive end, plus 1-based lines and
//! character columns. Spans are absent when the component came from Rodin
//! XML, which carries no source text, and on the nodes a before-after
//! predicate synthesizes.
//!
//! # Two things deliberately not in the document
//!
//! Type ascriptions are unwrapped. They spell a type the node already
//! carries, Rodin has no node for them, and keeping them would make the tree
//! depend on whether an author wrote the type out. The `text` of an element
//! still shows them wherever Rodin writes them.
//!
//! Nothing the checker rejected appears. A document is what was checked, and
//! `diagnostics` says what was dropped and why. `accurate` on a context,
//! machine or event marks one that was checked incompletely.
//!
//! # Versioning
//!
//! `version` is an integer. New optional fields, new operator names for new
//! Rodin tags, and new layers keep the version; consumers should ignore keys
//! they do not know. Renaming or removing a field, or changing what one
//! means, raises it. Tag numbers and operator names are Rodin's and do not
//! change. An extension's tag is assigned per process and is never its
//! identity; `extension.id` is.

pub mod element;
pub mod formula;
pub mod location;
mod opname;
mod origin;

use std::collections::BTreeMap;

use crate::BuildResult;
use crate::project::Project;
use crate::sc_model::ScModel;

pub use element::{
    ActionDump, ContextDump, DiagnosticDump, EventDump, IdentDump, MachineDump, PredicateDump,
    VariableDump, VariantDump,
};
pub use formula::{AssignmentNode, ExtensionInfo, ExtensionRef, Node, TypeNode};
pub use location::SpanDump;

/// The document format's name, written into every document.
pub const FORMAT: &str = "rossi-model";

/// The document format's version. See the module documentation for what a
/// change to it means.
pub const VERSION: u32 = 1;

/// The JSON Schema of a document, as a draft 2020-12 schema.
///
/// It is the contract a consumer can check against, and it is strict: no
/// object accepts a key the schema does not name, so a field added on one
/// side and not the other is caught rather than ignored. The schema also
/// carries the prose that explains the format, which the doc comments here
/// repeat for a reader of the source.
pub const JSON_SCHEMA: &str = include_str!("../../schema/rossi-model.v1.schema.json");

/// What a caller can vary about a document.
#[derive(Debug, Default, Clone)]
pub struct Options {
    /// The input as the caller named it, recorded for a reader's benefit.
    pub input: Option<String>,
    /// The archive prefix this project was found under, when it came from an
    /// archive holding several.
    pub prefix: Option<String>,
    /// The real path of a component, keyed by its Rodin filename, where the
    /// two differ. A component checked from Event-B text is named as Rodin
    /// would name it, so that handles match what a build writes, and this is
    /// what says where it actually came from.
    pub source_paths: BTreeMap<String, String>,
}

/// What produced a document.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Generator {
    pub name: &'static str,
    pub version: &'static str,
}

/// Which project a document describes.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ProjectInfo {
    /// The Rodin project name, which is the leading segment of every handle.
    pub name: String,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub input: Option<String>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub prefix: Option<String>,
}

/// A file the document's spans refer to.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct SourceInfo {
    /// What a span names as its file.
    pub id: String,
    /// Where the text actually came from, which differs from `id` when a
    /// component was checked from Event-B text under its Rodin name.
    pub path: String,
    /// `eventb`, `buc` or `bum`.
    pub kind: &'static str,
}

/// One project's checked model.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Model {
    pub format: &'static str,
    pub version: u32,
    pub generator: Generator,
    pub project: ProjectInfo,
    pub sources: Vec<SourceInfo>,
    /// Every type any node names, keyed by its canonical Rodin string.
    pub types: BTreeMap<String, TypeNode>,
    /// Every operator extension any node uses.
    pub extensions: Vec<ExtensionInfo>,
    pub contexts: Vec<ContextDump>,
    pub machines: Vec<MachineDump>,
    /// What checking and linting found, in report order.
    pub diagnostics: Vec<DiagnosticDump>,
}

impl Model {
    /// Whether anything in the document is an error.
    ///
    /// Computed from the reported findings rather than from the build result,
    /// so a document and its exit status can never disagree about what it
    /// contains.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == element::SEVERITY_ERROR)
    }
}

/// Build the document for one checked project.
///
/// Never fails. A component the checker dropped is simply absent, and
/// `diagnostics` carries the reason.
///
/// Findings come from the build result and from the lints, which is the same
/// set `rossi validate` reports, so the two cannot disagree about a project.
///
/// Order is taken from the project's own component list and from the ordered
/// parts of each record. The checked model keys its contexts and machines by
/// name in hash maps, which have no order to speak of, so they are only ever
/// looked up here, never iterated.
#[must_use]
pub fn model(project: &Project, sc: &ScModel, build: &BuildResult, options: &Options) -> Model {
    let origins = origin::Origins::build(project);
    let mut types = BTreeMap::new();
    let mut extensions = BTreeMap::new();

    let mut contexts = Vec::new();
    let mut machines = Vec::new();
    let mut sources = Vec::new();

    for component in &project.components {
        let name = component.component.name();
        let component_origin = origins.get(name);
        sources.push(SourceInfo {
            id: component.source_id().to_string(),
            path: options
                .source_paths
                .get(&component.filename)
                .cloned()
                .unwrap_or_else(|| component.filename.clone()),
            kind: source_kind(&component.filename, component.source.is_some()),
        });

        let mut builder = element::Builder {
            ctx: formula::Ctx::new(
                component_origin.and_then(|c| c.lines.as_ref()),
                &mut types,
                &mut extensions,
            ),
            origin: component_origin,
        };
        if let Some(checked) = sc.contexts.get(name) {
            contexts.push(element::context(&mut builder, checked));
        } else if let Some(checked) = sc.machines.get(name) {
            machines.push(element::machine(&mut builder, sc, checked));
        }
    }

    let diagnostics = build
        .diagnostics
        .iter()
        .chain(crate::lint::run(project).iter())
        .map(|d| element::diagnostic(d, &origins))
        .collect();

    Model {
        format: FORMAT,
        version: VERSION,
        generator: Generator {
            name: "rossi",
            version: env!("CARGO_PKG_VERSION"),
        },
        project: ProjectInfo {
            name: project.name.clone(),
            input: options.input.clone(),
            prefix: options.prefix.clone(),
        },
        sources,
        types,
        extensions: extensions.into_values().collect(),
        contexts,
        machines,
        diagnostics,
    }
}

/// What kind of file a component's spans point into.
fn source_kind(filename: &str, has_text: bool) -> &'static str {
    if has_text {
        // Checked from Event-B text, whatever Rodin name it was given so its
        // handles match a build's.
        return "eventb";
    }
    match filename.rsplit('.').next() {
        Some("buc") => "buc",
        Some("bum") => "bum",
        _ => "eventb",
    }
}
