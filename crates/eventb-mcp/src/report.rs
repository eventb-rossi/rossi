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

/// One element an obligation traces back to.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SourceRecord {
    /// `DEFAULT`, `ABSTRACT` or `CONCRETE`: the part the element plays.
    pub role: String,
    /// The component that declares the element.
    pub component: String,
    /// `invariant`, `guard`, `action`, `event`, `axiom`, ...
    pub kind: String,
    /// The element's label, identifier or target.
    pub name: String,
    /// The event a guard, action, parameter or witness belongs to.
    pub event: Option<String>,
    /// Whether a labelled predicate is a theorem.
    pub theorem: bool,
}

/// The recorded proof status of one obligation.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct StatusRecord {
    /// `discharged`, `reviewed`, `pending` or `unattempted`.
    pub bucket: String,
    /// The recorded confidence, when the row carries one.
    pub confidence: Option<i32>,
    /// Whether the stored proof no longer applies to the obligation.
    pub broken: bool,
    /// Whether the proof is marked as made by hand.
    pub manual: bool,
    /// Whether the verdict is due for recomputation.
    pub stale: bool,
}

/// One proof obligation.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ObligationRecord {
    pub component: String,
    /// The obligation's name, e.g. `evt/inv1/INV`.
    pub name: String,
    /// The nature's name (`InvariantPreservation`, `GuardStrengtheningSplit`,
    /// ...), or the description verbatim when it is not one of the generator's.
    pub nature: String,
    /// The human description (`Invariant  preservation`).
    pub description: String,
    /// Whether every element the obligation depends on passed its checks.
    pub accurate: bool,
    pub stamp: Option<String>,
    pub sources: Vec<SourceRecord>,
    pub status: Option<StatusRecord>,
}

/// The `list_pos` tool's document: one page of obligations.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ListPosReport {
    pub obligations: Vec<ObligationRecord>,
    /// How many obligations match the filters in all.
    pub total: usize,
    pub offset: usize,
    /// The offset of the next page, when there is one.
    pub next_offset: Option<usize>,
}

/// One hypothesis of a sequent.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct HypothesisRecord {
    /// The hypothesis's position among all of the sequent's hypotheses.
    pub index: usize,
    /// The predicate, in Event-B notation.
    pub text: String,
    /// Whether the obligation's selection hints select it for the prover.
    pub selected: bool,
}

/// The `get_po` tool's document: one obligation with its sequent.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ObligationReport {
    pub obligation: ObligationRecord,
    /// The typed identifiers in scope, name to type.
    pub identifiers: BTreeMap<String, String>,
    /// The hypotheses, up to the requested cap, with the well-definedness
    /// conjuncts the prover adds.
    pub hypotheses: Vec<HypothesisRecord>,
    /// How many hypotheses the sequent has in all.
    pub hypotheses_total: usize,
    /// Whether `hypotheses` stops short of `hypotheses_total`.
    pub truncated: bool,
    /// The goal, in Event-B notation.
    pub goal: String,
}

/// The status summary of one component.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ComponentProofs {
    pub component: String,
    pub summary: ProofSummary,
}

/// The `proof_status` tool's document.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ProofStatusReport {
    pub summary: ProofSummary,
    pub components: Vec<ComponentProofs>,
}

/// One identifier of a state with the tool's rendering of its value.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct BindingRecord {
    pub name: String,
    pub value: String,
}

/// An invariant the model checker found violated, mapped back to its
/// declaration when the rendering or the label matched one.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ViolatedInvariant {
    /// The predicate as the tool printed it.
    pub text: String,
    pub component: Option<String>,
    pub label: Option<String>,
}

/// The trace to a violating state.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CounterexampleRecord {
    /// The events fired, the constants setup and INITIALISATION first.
    pub transitions: Vec<String>,
    /// The violating state as the tool printed it.
    pub violating_state: String,
    pub violated_invariants: Vec<ViolatedInvariant>,
    /// The violating state, one binding per variable.
    pub bindings: Vec<BindingRecord>,
    /// How many transitions the trace has.
    pub steps: usize,
}

/// What a model check concluded.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct VerdictRecord {
    /// `ok`, `incomplete`, `invariant_violation`, `deadlock`, `finding`,
    /// `load_error` or `engine_error`.
    pub kind: String,
    /// Why the search ended (`exhaustive`, `state_limit`, `time_limit`, ...)
    /// for `ok` and `incomplete`.
    pub reason: Option<String>,
    /// How many states an `ok` search explored.
    pub states: Option<u64>,
    /// The tool's finding category for `finding`.
    pub category: Option<String>,
    /// The tool's message for a finding or an error.
    pub message: Option<String>,
}

/// The `model_check` tool's document.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ModelCheckReport {
    pub machine: String,
    /// The bounds the run was constrained by.
    pub bounds: Vec<String>,
    /// The tool's status: `ok`, `violation`, `incomplete` or `error`.
    pub status: String,
    pub message: String,
    pub verdict: VerdictRecord,
    pub counterexample: Option<CounterexampleRecord>,
    /// The tool's exit code; absent when a signal killed it.
    pub exit_code: Option<i32>,
}

/// One check of a run: an event's preservation, an obligation, the
/// well-definedness gate.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CheckRecord {
    pub name: String,
    /// `passed`, `failed`, `error` or `skipped`.
    pub outcome: String,
    pub message: Option<String>,
    /// The counterexample state, when the check found one.
    pub bindings: Vec<BindingRecord>,
}

/// The document of `check_invariants_cbc` and `check_wd`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CheckRunReport {
    pub machine: String,
    pub bounds: Vec<String>,
    /// The tool's status: `ok`, `violation`, `incomplete` or `error`.
    pub status: String,
    pub message: String,
    pub checks: Vec<CheckRecord>,
    pub exit_code: Option<i32>,
}

/// One obligation the disprover refuted.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DisproofRecord {
    /// The qualified name, `<component>/<obligation>`.
    pub name: String,
    pub message: String,
    pub bindings: Vec<BindingRecord>,
}

/// What a disprover run concluded.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DisproveVerdict {
    /// `disproved`, `no_counterexample`, `ok` or `error`.
    pub kind: String,
    pub disproved: Vec<DisproofRecord>,
    /// Obligations still open after the run.
    pub open: usize,
    /// Obligations the run looked at.
    pub total: usize,
    /// Counterexamples under the selected hypotheses only, which may be
    /// spurious.
    pub spurious: usize,
    pub message: Option<String>,
}

/// The `disprove_po` tool's document.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DisproveReport {
    pub machine: String,
    pub status: String,
    pub message: String,
    pub verdict: DisproveVerdict,
    pub checks: Vec<CheckRecord>,
    pub exit_code: Option<i32>,
}
