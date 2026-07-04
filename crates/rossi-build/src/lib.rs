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
pub mod duplicates;
pub mod enrich;
pub mod error;
pub mod handles;
pub mod infer;
pub mod lint;
pub mod normalize;
pub mod project;
pub mod repack;
pub mod rodin_ids;
pub mod rules;
pub mod sc_model;
pub mod sc_view;
pub mod type_env;
pub mod types;
pub mod wellformed;
pub mod xml_out;

mod ast_util;
mod sc;

pub use error::Error;
pub use handles::HandleUri;
pub use project::{Project, ProjectComponent};
pub use rules::RuleId;
pub use types::Type;

/// Static-check a whole project and emit one `.bcc` / `.bcm` per component.
///
/// Returns on the first fatal error (bad I/O, unparseable XML). Non-fatal
/// issues (type errors that result in an element being dropped from the
/// output, missing SEES target, etc.) appear in [`BuildResult::diagnostics`]
/// and do not abort the build — Rodin's SC has the same "drop but continue"
/// philosophy and downstream tools tolerate it.
pub fn build(project: &Project) -> BuildResult {
    sc::build_project(project).0
}

/// Like [`build`], additionally returning the typed model of every
/// successfully-checked component (type environments, axiom/event records).
/// Passes that analyse formulas after the static check — well-definedness,
/// IDE tooling — start from this instead of re-deriving state from the
/// emitted XML. See [`sc_model`] for the record types.
pub fn build_with_model(project: &Project) -> (BuildResult, sc_model::ScModel) {
    sc::build_project(project)
}

/// The output of a build: emitted files plus diagnostics collected along the way.
#[derive(Debug, Default, Clone)]
pub struct BuildResult {
    /// Emitted `.bcc` and `.bcm` files, in the order they were produced
    /// (topological order on SEES/REFINES/EXTENDS).
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
