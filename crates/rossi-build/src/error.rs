//! Fatal-error types that abort a build.
//!
//! Non-fatal diagnostics (dropped elements, missing references) are reported
//! via [`crate::Diagnostic`] inside [`crate::BuildResult`].

use thiserror::Error;

/// A fatal error from project loading or IO. Non-fatal type-check issues
/// are surfaced as [`crate::Diagnostic`] values instead.
#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("parse error: {0}")]
    Parse(#[from] rossi::ParseError),

    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("project error: {0}")]
    Project(Box<ProjectError>),
}

impl From<ProjectError> for Error {
    fn from(err: ProjectError) -> Self {
        Error::Project(Box::new(err))
    }
}

impl From<quick_xml::Error> for Error {
    fn from(err: quick_xml::Error) -> Self {
        Error::Project(Box::new(ProjectError::Xml(err)))
    }
}

/// Structured cause of an [`Error::Project`]. Variants preserve the
/// underlying type when possible so callers can introspect rather than
/// re-parse a `format!`'d string.
#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("not a directory: {0}")]
    NotADirectory(std::path::PathBuf),

    #[error("invalid XML: {0}")]
    Xml(#[from] quick_xml::Error),

    #[error("invalid XML attribute: {0}")]
    XmlAttribute(String),

    #[error("invalid XML tag: {0}")]
    XmlTag(String),

    #[error("could not parse {kind} {input:?}: {err}")]
    ReparseFormula {
        kind: &'static str,
        input: String,
        #[source]
        err: rossi::ParseError,
    },
}

pub type Result<T> = std::result::Result<T, Error>;
