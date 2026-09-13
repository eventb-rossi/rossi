//! The documents the tools return, and how a finding becomes one row.
//!
//! Every record is a plain struct with string enumerations, so the schema
//! the client sees is flat: no tagged unions, no references beyond the
//! nested record types.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Serialize;

use rossi_build::{Diagnostic, RuleId, Severity};

/// A 1-indexed position range in a source file, in characters.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Region {
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

impl Region {
    /// The region a byte span of `source` covers.
    pub fn of_span(source: &str, span: rossi::ast::Span) -> Region {
        let (start_line, start_column) = line_col(source, span.start);
        let (end_line, end_column) = line_col(source, span.end);
        Region {
            start_line,
            start_column,
            end_line,
            end_column,
        }
    }

    /// A point region at a 1-indexed position.
    pub fn point(line: usize, column: usize) -> Region {
        Region {
            start_line: line,
            start_column: column,
            end_line: line,
            end_column: column,
        }
    }
}

fn line_col(source: &str, byte_offset: usize) -> (usize, usize) {
    let (line, col) = rossi::ast::Span {
        start: byte_offset,
        end: byte_offset,
    }
    .to_line_col(source);
    (line + 1, col + 1)
}

/// One finding about the project.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DiagnosticRecord {
    /// `error`, `warning` or `info`.
    pub severity: String,
    /// The stable `EBnnn` rule code, when the finding has one.
    pub rule_id: Option<String>,
    /// The component the finding is about, when it is about one.
    pub component: Option<String>,
    /// The element inside the component (`inv1`, `evt/grd1`), when the
    /// finding names one.
    pub element: Option<String>,
    pub message: String,
    /// The source file, relative to the root, when the finding has one.
    pub file: Option<String>,
    /// Where in `file` the finding is, when the source text is known.
    pub region: Option<Region>,
}

impl DiagnosticRecord {
    /// A whole-file failure such as a parse error.
    pub fn for_file(rule: RuleId, file: &str, message: String, region: Option<Region>) -> Self {
        DiagnosticRecord {
            severity: Severity::Error.as_str().to_string(),
            rule_id: Some(rule.code().to_string()),
            component: None,
            element: None,
            message,
            file: Some(file.to_string()),
            region,
        }
    }

    /// A checker finding, located through `source_of`: the file and text
    /// of the component the finding names, when they are known.
    pub fn of_diagnostic(
        diagnostic: &Diagnostic,
        source_of: impl Fn(&str) -> Option<(String, Option<String>)>,
    ) -> Self {
        let component = diagnostic.component();
        let element = diagnostic
            .origin
            .strip_prefix(component)
            .and_then(|rest| rest.strip_prefix('.'))
            .filter(|rest| !rest.is_empty())
            .map(str::to_string);
        let located = source_of(component);
        let region = match (&located, diagnostic.span) {
            (Some((_, Some(text))), Some(span)) => Some(Region::of_span(text, span)),
            _ => None,
        };
        DiagnosticRecord {
            severity: diagnostic.severity.as_str().to_string(),
            rule_id: diagnostic.rule_id.map(|rule| rule.code().to_string()),
            component: located.is_some().then(|| component.to_string()),
            element,
            message: diagnostic.message.clone(),
            file: located.map(|(file, _)| file),
            region,
        }
    }

    /// Whether the record is at least `minimum` severe.
    pub fn at_least(&self, minimum: Severity) -> bool {
        rank(&self.severity) >= rank(minimum.as_str())
    }
}

fn rank(severity: &str) -> u8 {
    match severity {
        "error" => 2,
        "warning" => 1,
        _ => 0,
    }
}

/// How many findings of each severity a document carries.
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
pub struct DiagnosticCounts {
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
}

impl DiagnosticCounts {
    pub fn of(records: &[DiagnosticRecord]) -> Self {
        let mut counts = DiagnosticCounts::default();
        for record in records {
            match record.severity.as_str() {
                "error" => counts.errors += 1,
                "warning" => counts.warnings += 1,
                _ => counts.infos += 1,
            }
        }
        counts
    }
}

/// One component of the project.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ComponentRecord {
    pub name: String,
    /// `context` or `machine`.
    pub kind: String,
    /// The source file, relative to the root.
    pub file: String,
}

/// One dependency between components.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct EdgeRecord {
    pub from: String,
    pub to: String,
    /// `sees`, `extends` or `refines`.
    pub kind: String,
}

/// The `project` tool's document.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ProjectReport {
    /// The root directory the server reads.
    pub root: String,
    /// The project name every handle starts with.
    pub name: String,
    /// `text` for a folder of `.eventb` files, `rodin` for a folder of
    /// `.bum`/`.buc` files.
    pub kind: String,
    /// Where builds write their checked and proof files.
    pub output_dir: String,
    pub components: Vec<ComponentRecord>,
    pub edges: Vec<EdgeRecord>,
    pub diagnostics: DiagnosticCounts,
}

/// The `validate` tool's document.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ValidateReport {
    pub diagnostics: Vec<DiagnosticRecord>,
    pub counts: DiagnosticCounts,
}

/// One file a build wrote or would write.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FileRecord {
    pub filename: String,
    /// Whether every element in the file passed its checks.
    pub accurate: bool,
}

/// How many obligations a build generated.
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
pub struct ObligationCounts {
    pub total: usize,
    /// Obligations per nature, keyed by the nature's name
    /// (`InvariantPreservation`, `GuardStrengtheningSplit`, ...).
    pub by_nature: BTreeMap<String, usize>,
}

/// The recorded status of the obligations, as buckets.
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
pub struct ProofSummary {
    pub total: usize,
    pub discharged: usize,
    pub reviewed: usize,
    pub pending: usize,
    pub unattempted: usize,
    /// Obligations whose stored proof no longer applies; counted in the
    /// bucket their capped confidence puts them in as well.
    pub broken: usize,
}

/// The `build` tool's document.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct BuildReport {
    /// Where the files went, or would go.
    pub output_dir: String,
    /// Whether the files were written by this call.
    pub written: bool,
    pub files: Vec<FileRecord>,
    pub diagnostics: Vec<DiagnosticRecord>,
    pub counts: DiagnosticCounts,
    pub obligations: ObligationCounts,
    pub proofs: ProofSummary,
}

/// A failure the tool ran into, as the document it returns instead.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Failure {
    pub error: String,
    /// The findings that explain the failure, when there are any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<DiagnosticRecord>,
}
