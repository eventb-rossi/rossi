//! # rossi-build
//!
//! Static checker / builder for Event-B models.
//!
//! Reads Rodin's unchecked `.buc` / `.bum` files and emits the corresponding
//! checked `.bcc` / `.bcm` files. Output is intended to be semantically
//! equivalent to what Rodin's own Static Checker produces, and is accepted
//! by downstream tools (ProB animator, Proof Obligation Generator).
//!
//! ## Quick start
//!
//! ```no_run
//! use rossi_build::{Project, build};
//!
//! let project = Project::from_zip_file("auction.zip").unwrap();
//! let result = build(&project);
//!
//! for diag in &result.diagnostics {
//!     eprintln!("{diag}");
//! }
//! for file in &result.files {
//!     std::fs::write(&file.filename, &file.contents).unwrap();
//! }
//! ```

pub mod checked_predicate;
pub mod dump;
pub mod duplicates;
pub mod error;
pub mod handles;
pub mod identifiers;
pub mod lint;
pub mod normalize;
pub mod po_view;
pub mod pog;
pub mod project;
pub mod proofs;
pub mod repack;
pub mod rodin_ids;
pub mod rules;
pub mod sc_model;
pub mod sc_view;
pub mod type_env;
pub mod walk;
pub mod wd;
pub mod workspace;
pub mod xml_out;

mod ast_util;
mod sc;

pub use error::Error;
pub use handles::HandleUri;
pub use project::{Project, ProjectComponent};
pub use rossi::formula::Type;
pub use rules::RuleId;

/// Every component-local *semantic* check, for one component: the
/// duplicate-name errors ([`duplicates::component_duplicate_diagnostics`])
/// and the primed-declaration and primed-use errors
/// ([`identifiers`]). These need no project, no cross-component resolution
/// and no type inference, so they are the errors a lone `.eventb` file and
/// an open editor document can decide.
///
/// This is the `--no-semantic` half of the component-local surface; the
/// advisory half is [`lint::run_component`] plus [`lint::run_source`],
/// gated on `--no-lints`. Both `rossi validate`'s loose-text path and the
/// LSP compose exactly these, so a new component-local error joins the set
/// here rather than in each caller.
#[must_use]
pub fn component_semantic_diagnostics(component: &rossi::Component) -> Vec<Diagnostic> {
    let mut diags = duplicates::component_duplicate_diagnostics(component);
    diags.extend(identifiers::component_primed_name_diagnostics(component));
    diags.extend(identifiers::component_undeclarable_prime_diagnostics(
        component,
    ));
    diags
}

/// Static-check a whole project and emit one `.bcc` / `.bcm` per component.
///
/// Loading a [`Project`] owns fatal I/O and XML errors. Once loaded, static-check
/// issues (type errors that result in an element being dropped from the output,
/// missing SEES targets, dependency cycles, etc.) appear in
/// [`BuildResult::diagnostics`]. Rodin's SC has the same "drop but continue"
/// philosophy, except for project-integrity failures that stop checked-file
/// emission.
pub fn build(project: &Project) -> BuildResult {
    build_with_model(project).0
}

/// Like [`build`], additionally returning the typed model of every
/// successfully-checked component (type environments, axiom/event records).
/// Passes that analyse formulas after the static check — well-definedness,
/// IDE tooling — start from this instead of re-deriving state from the
/// emitted XML. See [`sc_model`] for the record types.
pub fn build_with_model(project: &Project) -> (BuildResult, sc_model::ScModel) {
    let (mut result, model) = sc::build_project(project);
    // Proof obligations follow the static check for every component it
    // successfully checked: one `.bpo` (the sequents) and one `.bps`
    // (their unattempted statuses) per component.
    result.files.extend(pog::generate(project, &model));
    (result, model)
}

/// Like [`build_with_model`] but static-check only: the typed model without
/// proof-obligation generation. IDE features (inlay hints) need the type
/// environments but never the `.bpo`/`.bps` files, so they skip that cost.
pub fn check_with_model(project: &Project) -> (BuildResult, sc_model::ScModel) {
    sc::build_project(project)
}

/// The output of a build: emitted files plus diagnostics collected along the way.
#[derive(Debug, Default, Clone)]
pub struct BuildResult {
    /// Emitted files: `.bcc` / `.bcm` in the order they were produced
    /// (topological order on SEES/REFINES/EXTENDS), then the generated
    /// `.bpo` / `.bps` proof-obligation files in project order.
    pub files: Vec<ScFile>,

    /// All diagnostics emitted during the build.
    pub diagnostics: Vec<Diagnostic>,
}

impl BuildResult {
    /// Returns true iff no diagnostics at [`Severity::Error`] were recorded.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    /// Returns true iff the build reported errors and emitted nothing —
    /// the report-and-stop failures of the static checker (duplicate
    /// component names, dependency cycles), as opposed to the drop-but-
    /// continue diagnostics that still leave checked output.
    #[must_use]
    pub fn failed_outright(&self) -> bool {
        self.files.is_empty() && !self.is_ok()
    }

    /// Find an emitted file by name (e.g. `"AuctionContext.bcc"`).
    pub fn file(&self, name: &str) -> Option<&ScFile> {
        self.files.iter().find(|f| f.filename == name)
    }
}

/// A single emitted statically-checked file.
#[derive(Debug, Clone)]
pub struct ScFile {
    /// Target file name, e.g. `"AuctionContext.bcc"` or `"AuctionMachine.bcm"`.
    pub filename: String,
    /// The XML payload, UTF-8 encoded.
    pub contents: String,
    /// True iff every element in the file passed its checks (maps to
    /// Rodin's `org.eventb.core.accurate` on the root element).
    pub accurate: bool,
}

/// Whether `value` is exactly one normal path component — no separators, no
/// `..`, no NUL, not absolute — so writing it under a chosen output directory
/// can never escape that directory. The guard every consumer of
/// [`ScFile::filename`] applies before touching the filesystem.
#[must_use]
pub fn is_normal_path_component(value: &str) -> bool {
    if value.contains('\0') {
        return false;
    }
    let path = std::path::Path::new(value);
    let mut parts = path.components();
    matches!(parts.next(), Some(std::path::Component::Normal(part)) if path.as_os_str() == part)
        && parts.next().is_none()
}

/// A single diagnostic — a type error, a missing reference, a cycle, etc.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Origin of the diagnostic: the component name, optionally scoped by
    /// element label (`"AuctionContext"`, `"AuctionMachine.inv3"`). For an
    /// element-scoped diagnostic the leading dot-separated segment is the
    /// component name; project-level diagnostics (e.g. a dependency cycle) use a
    /// non-component origin such as `"project"` instead.
    pub origin: String,
    pub message: String,
    /// Stable rule identifier (e.g. [`RuleId::CrossReferenceNotFound`]) when
    /// the diagnostic corresponds to a documented rule in `crate::rules`.
    /// `None` for internal catch-all sites that have no stable contract.
    pub rule_id: Option<RuleId>,
    /// Source span of the offending element, as a byte range into the owning
    /// component's `.eventb` text, so a caller can resolve a precise
    /// line/column. `None` for Rodin-XML imports (which carry no source) and for
    /// project-level diagnostics with no single element. Ignored by equality —
    /// it is positional metadata, not identity (the same treatment
    /// [`rossi::Predicate`] gives its own span).
    pub span: Option<rossi::ast::Span>,
}

/// Equality compares everything but the span — two diagnostics that differ only
/// in source position are the same finding.
impl PartialEq for Diagnostic {
    fn eq(&self, other: &Self) -> bool {
        self.severity == other.severity
            && self.origin == other.origin
            && self.message == other.message
            && self.rule_id == other.rule_id
    }
}

impl Eq for Diagnostic {}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.rule_id {
            Some(rid) => write!(
                f,
                "[{}] [{}] {}: {}",
                self.severity, rid, self.origin, self.message
            ),
            None => write!(f, "[{}] {}: {}", self.severity, self.origin, self.message),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Error => write!(f, "error"),
            Severity::Warning => write!(f, "warning"),
            Severity::Info => write!(f, "info"),
        }
    }
}
