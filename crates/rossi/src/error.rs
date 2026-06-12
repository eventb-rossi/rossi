//! Error types for the Event-B parser

use thiserror::Error;

/// Errors that can occur during parsing
#[derive(Error, Debug, Clone)]
pub enum ParseError {
    #[error("Pest parsing error: {message}")]
    PestError {
        message: String,
        /// 1-indexed error position, from pest's structured location.
        line: usize,
        column: usize,
    },

    #[error("Unexpected rule: expected {expected}, found {found}")]
    UnexpectedRule { expected: String, found: String },

    #[error("Invalid integer: {0}")]
    InvalidInteger(String),

    /// The nesting-depth pre-scan ([`crate::nesting`]) refused the input:
    /// parsing it could overflow the stack and abort the process. `line` and
    /// `column` are 1-indexed.
    #[error("formula nesting exceeds the maximum depth of {limit} at line {line}, column {column}")]
    NestingTooDeep {
        limit: usize,
        line: usize,
        column: usize,
    },

    #[error("Empty expression")]
    EmptyExpression,

    #[error("Empty predicate")]
    EmptyPredicate,

    #[error("Missing predicate")]
    MissingPredicate,

    #[error("Missing action")]
    MissingAction,

    #[error("Missing variable")]
    MissingVariable,

    #[error("Missing operator")]
    MissingOperator,

    #[error("Missing value")]
    MissingValue,

    #[error("Invalid XML: {0}")]
    InvalidXml(String),

    /// EB002 — XML root element is neither `org.eventb.core.contextFile`
    /// nor `org.eventb.core.machineFile`. `found` is the first element name
    /// the parser actually saw (empty if the document had no Start event).
    #[error("Unexpected XML root: expected contextFile or machineFile, found `{found}`")]
    UnexpectedXmlRoot { found: String },

    /// EB003 — A required XML attribute is missing from an element.
    #[error("Missing required attribute `{attribute}` on element `{element}`")]
    MissingXmlAttribute { element: String, attribute: String },

    /// Wrapper preserving the inner [`ParseError`] variant when a per-file
    /// parse fails inside [`crate::parse_zip_with_recovery`]. The `Display`
    /// rendering matches the legacy "Failed to parse {filename}: …" string
    /// so console output stays byte-identical.
    #[error("Failed to parse {filename}: {source}")]
    FileContext {
        filename: String,
        source: Box<ParseError>,
    },

    #[error("Unsupported identifier {name:?} ({origin}): {reason}")]
    UnsupportedIdentifier {
        name: String,
        origin: String,
        reason: String,
    },

    #[error("Malformed {attr_name} in {origin}{label}: {reason}")]
    MalformedAttribute {
        origin: String,
        label: String,
        attr_name: String,
        value: String,
        reason: String,
    },

    #[error("IO error: {0}")]
    IoError(String),

    #[error("Failed to parse clause at line {line}, column {column}: {message}")]
    ClauseError {
        clause_type: String,
        line: usize,
        column: usize,
        message: String,
    },

    #[error("Recoverable error at line {line}, column {column}: {message}")]
    RecoverableError {
        line: usize,
        column: usize,
        message: String,
        source: Option<Box<ParseError>>,
    },

    #[error("Wrong number of arguments for {name}: expected {expected}, got {actual}")]
    ArityMismatch {
        name: String,
        expected: String,
        actual: usize,
    },

    #[error("Multiple parse errors ({} total): {}", .0.len(), .0.first().map(|e| e.to_string()).unwrap_or_default())]
    MultipleErrors(Vec<ParseError>),
}

impl From<std::io::Error> for ParseError {
    fn from(error: std::io::Error) -> Self {
        ParseError::IoError(error.to_string())
    }
}

impl From<Box<pest::error::Error<crate::parser::Rule>>> for ParseError {
    fn from(error: Box<pest::error::Error<crate::parser::Rule>>) -> Self {
        let (line, column) = match error.line_col {
            pest::error::LineColLocation::Pos(pos) => pos,
            pest::error::LineColLocation::Span(start, _) => start,
        };
        ParseError::PestError {
            message: error.to_string(),
            line,
            column,
        }
    }
}

/// Result type for parsing operations that may recover from errors
#[derive(Debug)]
pub struct ParseResult<T> {
    /// The parsed component (may be partial if there were recoverable errors)
    pub component: Option<T>,
    /// List of all errors encountered during parsing
    pub errors: Vec<ParseError>,
}

impl<T> ParseResult<T> {
    /// Create a new successful parse result
    pub fn ok(component: T) -> Self {
        Self {
            component: Some(component),
            errors: Vec::new(),
        }
    }

    /// Create a new parse result with errors
    pub fn with_errors(component: Option<T>, errors: Vec<ParseError>) -> Self {
        Self { component, errors }
    }

    /// Create a failed parse result
    pub fn err(error: ParseError) -> Self {
        Self {
            component: None,
            errors: vec![error],
        }
    }

    /// Check if parsing was successful (no errors)
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Check if parsing failed completely
    pub fn is_err(&self) -> bool {
        self.component.is_none()
    }

    /// Check if parsing recovered (has component but also has errors)
    pub fn has_recovered(&self) -> bool {
        self.component.is_some() && !self.errors.is_empty()
    }

    /// Get the component, consuming the result
    pub fn into_component(self) -> Option<T> {
        self.component
    }

    /// Get all errors
    pub fn get_errors(&self) -> &[ParseError] {
        &self.errors
    }

    /// Convert to a standard Result, treating any errors as failure.
    /// If there are multiple errors, they are wrapped in a MultipleErrors variant.
    pub fn into_result(self) -> Result<T> {
        if self.errors.is_empty() {
            self.component.ok_or(ParseError::MissingValue)
        } else if self.errors.len() == 1 {
            Err(self.errors.into_iter().next().unwrap())
        } else {
            Err(ParseError::MultipleErrors(self.errors))
        }
    }
}

pub type Result<T> = std::result::Result<T, ParseError>;
