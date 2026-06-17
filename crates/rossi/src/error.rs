//! Error types for the Event-B parser

use crate::ast::Span;
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
        /// Source byte span from pest's structured location, when available.
        /// Additive and unreferenced by `Display` (oracle-safe); lets
        /// CLI/SARIF/LSP share an accurate range instead of reconstructing one.
        /// Usually a zero-width position — pest reports a single failure point.
        span: Option<Span>,
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

    /// A kernel_lang §2.2 reserved word was used as an ordinary identifier.
    /// See [`crate::builtins::RESERVED_OPERATOR_WORDS`] /
    /// [`crate::builtins::RESERVED_ATOM_WORDS`] for the policy (exact-case,
    /// Rodin parity). `line` and `column` are 1-indexed.
    #[error(
        "reserved word `{word}` cannot be used as an identifier at line {line}, column {column}"
    )]
    ReservedWord {
        word: String,
        line: usize,
        column: usize,
        /// Byte span of the offending word (additive, oracle-safe).
        span: Option<Span>,
    },

    /// Two adjacent operators were used without the parentheses the Event-B
    /// language requires — e.g. `A ∪ B ∩ C` or `P ∧ Q ∨ R`. Both spellings are
    /// valid operators; only their bare juxtaposition is rejected (the Rodin
    /// formula parser raises `IncompatibleOperators` here). `left` is the
    /// operator binding the accumulated left operand, `right` the next operator
    /// (or, for a bare quantifier conjunct, the quantifier). `line` and `column`
    /// are 1-indexed; `span` is the byte range of the operator at which the
    /// incompatibility is detected (additive, oracle-safe).
    #[error("Operator: {left} is not compatible with: {right}, parentheses are required")]
    IncompatibleOperators {
        left: String,
        right: String,
        line: usize,
        column: usize,
        span: Option<Span>,
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
        /// Byte span of the recovered predicate that failed to parse — the
        /// whole `@label … predicate`, so consumers underline it precisely.
        span: Option<Span>,
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
        let (line, column) = match &error.line_col {
            pest::error::LineColLocation::Pos(pos) => *pos,
            pest::error::LineColLocation::Span(start, _) => *start,
        };
        // pest's `location` is byte offsets: a single point for `new_from_pos`
        // (the common case — a zero-width span) or a real span for
        // `new_from_span`.
        let span = match &error.location {
            pest::error::InputLocation::Pos(p) => Some(Span { start: *p, end: *p }),
            pest::error::InputLocation::Span((s, e)) => Some(Span { start: *s, end: *e }),
        };
        // Rewrite the `expected …` list from internal rule names (`op_in`,
        // `lbrace`) to the symbols a user types (`∈`, `{`); unmapped rules keep
        // their pest name. Renaming leaves the position fields untouched, so it
        // happens last (it consumes the pest error).
        let message = (*error)
            .renamed_rules(|rule| crate::parser::display_rule(*rule))
            .to_string();
        ParseError::PestError {
            message,
            line,
            column,
            span,
        }
    }
}

impl ParseError {
    /// 1-indexed `(line, column)` of where this error starts, when it carries a
    /// source position. Unwraps a [`ParseError::FileContext`] envelope and
    /// follows [`ParseError::MultipleErrors`] to its first entry.
    pub fn position(&self) -> Option<(usize, usize)> {
        match self {
            ParseError::PestError { line, column, .. }
            | ParseError::NestingTooDeep { line, column, .. }
            | ParseError::ReservedWord { line, column, .. }
            | ParseError::IncompatibleOperators { line, column, .. }
            | ParseError::ClauseError { line, column, .. }
            | ParseError::RecoverableError { line, column, .. } => Some((*line, *column)),
            ParseError::FileContext { source, .. } => source.position(),
            ParseError::MultipleErrors(errors) => errors.first().and_then(ParseError::position),
            _ => None,
        }
    }

    /// Source byte [`Span`] of this error, when the parser captured one that
    /// bounds the offending construct. Follows the same envelope/aggregate
    /// handling as [`position`](Self::position). Often a zero-width position for
    /// pest errors; callers that need a visible range should size an empty span
    /// themselves. A recovery error spans the whole `@label … predicate` it
    /// failed on; clause-order errors still carry no span (their pest span is
    /// the whole multi-line clause) — consumers size those from
    /// [`position`](Self::position).
    pub fn span(&self) -> Option<Span> {
        match self {
            ParseError::PestError { span, .. }
            | ParseError::ReservedWord { span, .. }
            | ParseError::IncompatibleOperators { span, .. }
            | ParseError::RecoverableError { span, .. } => *span,
            ParseError::FileContext { source, .. } => source.span(),
            ParseError::MultipleErrors(errors) => errors.first().and_then(ParseError::span),
            _ => None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_is_additive_and_display_ignores_it() {
        // The optional `span` is additive: it must not change `Display`, so the
        // rodin/import corpora and any message oracles stay byte-identical.
        let with = ParseError::PestError {
            message: "boom".to_string(),
            line: 2,
            column: 3,
            span: Some(Span { start: 5, end: 9 }),
        };
        let without = ParseError::PestError {
            message: "boom".to_string(),
            line: 2,
            column: 3,
            span: None,
        };
        assert_eq!(with.to_string(), "Pest parsing error: boom");
        assert_eq!(with.to_string(), without.to_string());
    }

    #[test]
    fn accessors_expose_position_and_span() {
        let err = ParseError::ReservedWord {
            word: "dom".to_string(),
            line: 4,
            column: 7,
            span: Some(Span { start: 10, end: 13 }),
        };
        assert_eq!(err.position(), Some((4, 7)));
        assert_eq!(err.span(), Some(Span { start: 10, end: 13 }));
    }

    #[test]
    fn incompatible_operators_message_names_both_operators() {
        let err = ParseError::IncompatibleOperators {
            left: "∪".to_string(),
            right: "∩".to_string(),
            line: 1,
            column: 7,
            span: Some(Span { start: 6, end: 7 }),
        };
        assert_eq!(
            err.to_string(),
            "Operator: ∪ is not compatible with: ∩, parentheses are required"
        );
        assert_eq!(err.position(), Some((1, 7)));
        assert_eq!(err.span(), Some(Span { start: 6, end: 7 }));
    }
}
