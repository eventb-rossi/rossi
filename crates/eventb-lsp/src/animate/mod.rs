//! eventb-animate integration: model-check a machine or disprove its proof
//! obligations straight from the editor.
//!
//! Exposed as two CodeLenses on every `MACHINE` header ([`COMMAND_CHECK`] and
//! [`COMMAND_PO`]), whose commands the server executes. A click collects the
//! machine's dependency closure from the live buffers (refinement ancestors
//! and visible contexts, unsaved edits included), statically checks it with
//! `rossi-build`, writes the result into a throwaway Rodin project directory,
//! and runs `eventb-animate` there with `--json -`. The format-4 JSON report
//! is classified into a verdict; violations and tool-reported errors become
//! diagnostics anchored back onto the live sources, and every outcome ends
//! in a `window/showMessage`.

pub mod closure;
pub mod diagnostics;
pub mod report;

use std::sync::Arc;

use crate::config::AnimateConfig;
use crate::cross_references::CrossReferenceManager;
use crate::document::DocumentManager;
use crate::lsp_types::*;
use crate::progress::Progress;
pub use eventb_animate_driver::AnimateMode;
use eventb_animate_driver::ToolError;
use tower_lsp::Client;

/// The `workspace/executeCommand` command behind the "Model-check" lens.
pub const COMMAND_CHECK: &str = "rossi.animate.check";

/// The `workspace/executeCommand` command behind the "Disprove POs" lens.
pub const COMMAND_PO: &str = "rossi.animate.po";

/// The flow title shown in progress and messages.
pub(crate) fn mode_title(mode: AnimateMode) -> &'static str {
    match mode {
        AnimateMode::Check => "Model-check",
        AnimateMode::Po => "Disprove POs",
    }
}

/// Everything that can end an animate run early, with a user-facing message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnimateError {
    /// The clicked document is neither open nor readable.
    SourceUnavailable(String),
    /// The lens argument names a component that is not a machine.
    NotAMachine(String),
    /// A closure member could not be resolved anywhere.
    MissingComponent(String),
    /// A closure member has syntax errors — a recovered AST silently drops
    /// elements, so running the tool on it would verify a different model.
    ParseFailed(String),
    /// The static check reported error diagnostics, already shaped into
    /// findings anchored on the offending elements.
    BuildFailed(Vec<diagnostics::Finding>),
    /// Filesystem failure while writing the temporary project.
    Io(String),
    /// The tool is not installed where the configuration points.
    ToolMissing(String),
    /// The tool ran but failed or produced no parseable report.
    ToolFailed(String),
    /// The watchdog killed a run that outlived its deadline (seconds).
    Timeout(u64),
}

/// A tool failure keeps its kind; the message gains the setting to fix.
impl From<ToolError> for AnimateError {
    fn from(error: ToolError) -> Self {
        match error {
            ToolError::Missing(program) => AnimateError::ToolMissing(program),
            ToolError::Failed(message) => AnimateError::ToolFailed(message),
            ToolError::Timeout(secs) => AnimateError::Timeout(secs),
        }
    }
}

impl std::fmt::Display for AnimateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnimateError::SourceUnavailable(uri) => write!(f, "cannot read {uri}"),
            AnimateError::NotAMachine(name) => write!(f, "'{name}' is not a machine"),
            AnimateError::MissingComponent(name) => write!(
                f,
                "component '{name}' was not found in the workspace or next to the file"
            ),
            AnimateError::ParseFailed(name) => {
                write!(f, "fix the syntax errors in '{name}' first")
            }
            AnimateError::BuildFailed(findings) => write!(
                f,
                "the model has {} static error(s) — see diagnostics.",
                findings.len()
            ),
            AnimateError::Io(message) => {
                write!(f, "cannot write the temporary project: {message}")
            }
            AnimateError::ToolMissing(program) => write!(
                f,
                "eventb-animate was not found ({program}). Install it or point the \
                 rossi.animate.path setting at it."
            ),
            AnimateError::ToolFailed(message) => write!(f, "eventb-animate failed: {message}"),
            AnimateError::Timeout(secs) => write!(
                f,
                "eventb-animate timed out after {secs} s \
                 (rossi.animate.timeLimitSecs / disproveTimeoutMs bound the run)"
            ),
        }
    }
}

/// The animate lenses for a document: "Model-check" and "Disprove POs" on
/// every `MACHINE` header (contexts get none — only a machine can be run),
/// anchored on the machine's name. When the parse yields no components
/// (mid-edit breakage), fall back to scanning `MACHINE <name>` header lines;
/// a header whose name is still missing gets no lens, because the commands
/// need the machine name as an argument.
pub fn code_lenses(components: &[rossi::Component], text: &str, uri: &Url) -> Vec<CodeLens> {
    let lenses: Vec<CodeLens> = components
        .iter()
        .filter_map(|component| {
            let rossi::Component::Machine(machine) = component else {
                return None;
            };
            let span = component.name_span().or_else(|| component.span())?;
            Some(machine_lenses(
                crate::position::span_to_range(&span, text),
                uri,
                &machine.name,
            ))
        })
        .flatten()
        .collect();
    if !lenses.is_empty() {
        return lenses;
    }
    header_line_scan(text, uri)
}

fn machine_lenses(range: Range, uri: &Url, machine: &str) -> [CodeLens; 2] {
    let lens = |title: &str, command: &str| CodeLens {
        range,
        command: Some(Command {
            title: title.to_string(),
            command: command.to_string(),
            arguments: Some(vec![
                serde_json::json!(uri.to_string()),
                serde_json::json!(machine),
            ]),
        }),
        data: None,
    };
    [
        lens("Model-check", COMMAND_CHECK),
        lens("Disprove POs", COMMAND_PO),
    ]
}

fn header_line_scan(text: &str, uri: &Url) -> Vec<CodeLens> {
    crate::text_utils::header_lines(text)
        .filter(|header| header.is_machine)
        .filter_map(|header| {
            let name = header.name?;
            Some(machine_lenses(
                crate::position::full_line_range(header.text, header.line as u32),
                uri,
                name,
            ))
        })
        .flatten()
        .collect()
}

/// Everything the spawned animate task needs, resolved up front by the
/// request handler so the task itself never touches server state.
pub struct AnimateRequest {
    /// The client-free pipeline inputs.
    pub input: ExecuteInput,
    /// Whether the client advertised `window.workDoneProgress`.
    pub progress_supported: bool,
    /// Analysis handles, for refreshing the findings overlay after the run.
    pub(crate) analyzer: crate::server::Analyzer,
}

/// The client-free inputs of one run — what [`execute`] consumes. Split from
/// [`AnimateRequest`] so integration tests can drive the full pipeline
/// without an LSP client.
pub struct ExecuteInput {
    pub mode: AnimateMode,
    /// The clicked document.
    pub uri: Url,
    /// The machine named by the lens arguments.
    pub machine: String,
    pub documents: Arc<DocumentManager>,
    pub cross_references: Arc<CrossReferenceManager>,
    /// The `rossi.animate` configuration, snapshotted at click time.
    pub config: AnimateConfig,
    /// The shared Rodin workspace project for the clicked file's directory,
    /// when it exists. Po mode reconciles the generated proof files against
    /// it (and against proof files next to the sources) so obligations
    /// Rodin already discharged skip the disprover. Ignored in Check mode.
    pub rodin_project_dir: Option<std::path::PathBuf>,
}

/// The classified result of one run.
#[derive(Debug)]
pub struct AnimateOutcome {
    pub verdict: report::Verdict,
    /// Diagnostics-to-be, anchored by label/event name and resolved against
    /// the live buffers at publish time. Empty on clean verdicts — which
    /// retracts the previous run's findings.
    pub findings: Vec<diagnostics::Finding>,
}

/// Run the full flow: preflight, closure, build, temp project, tool run,
/// report classification, findings-overlay refresh, and the verdict message.
/// All outcomes are reported through the client; the caller only spawns.
pub async fn run(client: Client, request: AnimateRequest) {
    let AnimateRequest {
        input,
        progress_supported,
        analyzer,
    } = request;
    let mode = input.mode;
    let title = mode_title(mode);
    let machine = input.machine.clone();
    let documents = Arc::clone(&input.documents);
    let disprove_timeout_ms = input.config.effective_disprove_timeout_ms();
    let progress = Progress::begin(&client, progress_supported, title).await;
    match execute_with_progress(input, Some(&progress)).await {
        Ok(outcome) => {
            // Findings anchored in files that are not open never surface as
            // diagnostics (the server only publishes for open documents), so
            // a "see diagnostics" message alone would send the user looking
            // for something that isn't there. Name the files instead.
            let unopened = unopened_finding_files(&outcome.findings, &documents);
            analyzer
                .refresh_animate_findings(machine.clone(), mode, outcome.findings)
                .await;
            let (kind, mut message) =
                verdict_message(&machine, &outcome.verdict, disprove_timeout_ms);
            if !unopened.is_empty() {
                message.push_str(&format!(
                    " Some findings are in files not currently open: {}.",
                    unopened.join(", ")
                ));
            }
            progress.finish(kind, message).await;
        }
        Err(error) => {
            // A failed build/parse means the on-screen model is no longer
            // the one the stored findings were computed from. A build
            // failure carries the static errors as findings of its own; a
            // parse failure retracts. Tool/infrastructure failures keep
            // the last-known findings.
            let mut message = error_toast(title, &error);
            match &error {
                AnimateError::BuildFailed(findings) => {
                    let unopened = unopened_finding_files(findings, &documents);
                    if !unopened.is_empty() {
                        message.push_str(&format!(
                            " Some findings are in files not currently open: {}.",
                            unopened.join(", ")
                        ));
                    }
                    analyzer
                        .refresh_animate_findings(machine, mode, findings.clone())
                        .await;
                }
                AnimateError::ParseFailed(_) => {
                    analyzer
                        .refresh_animate_findings(machine, mode, Vec::new())
                        .await;
                }
                _ => {}
            }
            // The toast vanishes on dismissal; the log line survives in the
            // client's output channel for diagnosis.
            client
                .log_message(MessageType::ERROR, format!("{title}: {error}"))
                .await;
            progress.finish(MessageType::ERROR, message).await;
        }
    }
}

/// The distinct files referenced by `findings` that are not open in the
/// editor, as display names (file name when the URI is a file, full URI
/// otherwise).
fn unopened_finding_files(
    findings: &[diagnostics::Finding],
    documents: &DocumentManager,
) -> Vec<String> {
    let mut names = Vec::new();
    for finding in findings {
        if documents.version(&finding.uri).is_some() {
            continue;
        }
        let name = finding
            .uri
            .to_file_path()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| finding.uri.to_string());
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// The client-free pipeline: everything [`run`] does except progress,
/// overlay refresh, and messaging. The `#[ignore]` integration tests drive
/// the real tool through here.
pub async fn execute(input: ExecuteInput) -> Result<AnimateOutcome, AnimateError> {
    execute_with_progress(input, None).await
}

async fn execute_with_progress(
    input: ExecuteInput,
    progress: Option<&Progress>,
) -> Result<AnimateOutcome, AnimateError> {
    // Preflight before any build work: a configured concrete path that does
    // not exist fails fast with the setting's name. Bare names are left to
    // the spawn — only PATH resolution can tell whether they exist.
    let program = eventb_animate_driver::effective_tool(&input.config.path);
    if let Some(concrete) = eventb_animate_driver::concrete_path(&program)
        && !concrete.exists()
    {
        return Err(AnimateError::ToolMissing(program));
    }

    if let Some(progress) = progress {
        progress.report("collecting the model").await;
    }
    let prepared = {
        let documents = Arc::clone(&input.documents);
        let cross_references = Arc::clone(&input.cross_references);
        let uri = input.uri.clone();
        let machine = input.machine.clone();
        let mode = input.mode;
        // Moved, not cloned: the field has no use after this block.
        let rodin_project_dir = input.rodin_project_dir;
        // The closure walk parses files and the static check is CPU-bound;
        // both stay off the async workers. The ComponentLoader is `!Sync`,
        // so it is constructed and dropped inside this one blocking task.
        tokio::task::spawn_blocking(move || {
            closure::prepare(
                &cross_references,
                &documents,
                &uri,
                &machine,
                mode,
                rodin_project_dir.as_deref(),
            )
        })
        .await
        .map_err(|join_error| AnimateError::Io(format!("the build task failed: {join_error}")))??
    };

    if let Some(progress) = progress {
        match input.mode {
            AnimateMode::Check => progress.report("running eventb-animate").await,
            AnimateMode::Po => {
                progress
                    .report(&format!(
                        "disproving {} open obligation(s)",
                        prepared.po_count
                    ))
                    .await;
            }
        }
    }
    let args = eventb_animate_driver::command_args(
        input.mode,
        &input.config,
        &input.machine,
        prepared.temp_dir.path(),
    );
    let watchdog = eventb_animate_driver::watchdog(input.mode, &input.config, prepared.po_count);
    let output = eventb_animate_driver::run_tool(&program, &args, watchdog).await;
    // The temp project must outlive the tool run; drop it before the
    // (allocation-heavy) classification, not after.
    let closure = prepared.closure;
    drop(prepared.temp_dir);
    let output = output?;

    let report = report::parse(&output.stdout, &output.stderr)?;
    let verdict = match input.mode {
        AnimateMode::Check => report::classify_check(&report, output.code),
        AnimateMode::Po => report::classify_po(&report),
    };
    let findings = diagnostics::findings(&verdict, &closure);
    Ok(AnimateOutcome { verdict, findings })
}

/// The `window/showMessage` verdict every run ends with.
/// `disprove_timeout_ms` is the effective per-obligation solver budget the
/// run used, quoted in the po no-counterexample message.
pub fn verdict_message(
    machine: &str,
    verdict: &report::Verdict,
    disprove_timeout_ms: u32,
) -> (MessageType, String) {
    use report::Verdict;
    match verdict {
        Verdict::CheckOk { reason, states } if reason == "exhaustive" => (
            MessageType::INFO,
            format!(
                "Model check of {machine}: no invariant violations or deadlocks \
                 ({states} states, exhaustive)."
            ),
        ),
        Verdict::CheckOk { reason, states } => (
            MessageType::INFO,
            format!(
                "Model check of {machine}: no violations found within the bound \
                 ({reason}, {states} states explored)."
            ),
        ),
        Verdict::CheckIncomplete { reason } => (
            MessageType::WARNING,
            format!("Model check of {machine} gave no verdict ({reason})."),
        ),
        Verdict::InvariantViolation { steps, .. } => (
            MessageType::WARNING,
            format!(
                "Model check of {machine}: invariant violation after {steps} step(s) — \
                 see diagnostics."
            ),
        ),
        Verdict::Deadlock { .. } => (
            MessageType::WARNING,
            format!("Model check of {machine}: deadlock found — see diagnostics."),
        ),
        Verdict::OtherFinding { category, message } => (
            MessageType::WARNING,
            format!("Model check of {machine}: {category}: {message}"),
        ),
        Verdict::LoadError { .. } => (
            MessageType::ERROR,
            format!(
                "Model check of {machine}: eventb-animate could not load the model — \
                 see diagnostics."
            ),
        ),
        Verdict::EngineError { .. } => (
            MessageType::ERROR,
            format!("Model check of {machine} failed — see diagnostics."),
        ),
        Verdict::PoDisproved {
            disproved, total, ..
        } => (
            MessageType::WARNING,
            format!(
                "{} of {total} proof obligations disproved for {machine} — see diagnostics.",
                disproved.len()
            ),
        ),
        Verdict::PoNoCounterexample {
            open,
            total,
            spurious,
        } => {
            let spurious_part = if *spurious > 0 {
                format!(", {spurious} spurious candidate(s) under selected hypotheses")
            } else {
                String::new()
            };
            (
                MessageType::INFO,
                format!(
                    "No PO counterexamples found for {machine} ({open} of {total} unproven \
                     within {disprove_timeout_ms} ms each; unproven ≠ wrong{spurious_part})."
                ),
            )
        }
        Verdict::PoOk { message } => (MessageType::INFO, format!("{machine}: {message}")),
        Verdict::PoError { .. } => (
            MessageType::ERROR,
            format!("Disprove POs for {machine}: the po gate failed — see diagnostics."),
        ),
    }
}

/// The toast for a failed run. Every variant's Display is short and
/// actionable except [`AnimateError::ToolFailed`], whose output excerpt
/// belongs in the log (`window/logMessage`), not a toast.
fn error_toast(title: &str, error: &AnimateError) -> String {
    match error {
        AnimateError::ToolFailed(_) => format!(
            "{title}: eventb-animate failed — see the Rossi Language Server log for details."
        ),
        _ => format!("{title}: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_lenses_only_and_carry_the_machine_name() {
        let uri = Url::parse("file:///m.eventb").unwrap();
        let text = "CONTEXT c\nEND\n\nMACHINE counters\nSEES c\nEND\n";
        let components = rossi::parse_components(text).unwrap();
        let lenses = code_lenses(&components, text, &uri);
        assert_eq!(
            lenses.len(),
            2,
            "two lenses on the machine, none on the context"
        );
        for (lens, (title, command)) in lenses
            .iter()
            .zip([("Model-check", COMMAND_CHECK), ("Disprove POs", COMMAND_PO)])
        {
            let cmd = lens.command.as_ref().unwrap();
            assert_eq!(cmd.title, title);
            assert_eq!(cmd.command, command);
            assert_eq!(
                cmd.arguments.as_ref().unwrap().as_slice(),
                &[
                    serde_json::json!("file:///m.eventb"),
                    serde_json::json!("counters")
                ]
            );
            // Anchored on the name token of the MACHINE header (line 3).
            assert_eq!(lens.range.start.line, 3);
            assert_eq!(lens.range.start.character, 8);
        }
    }

    #[test]
    fn header_scan_extracts_machine_names_and_skips_nameless_headers() {
        let uri = Url::parse("file:///m.eventb").unwrap();
        let text = "CONTEXT c\nEND\nMACHINE m\nMACHINE\nMACHINERY x\n";
        let lenses = header_line_scan(text, &uri);
        assert_eq!(lenses.len(), 2, "only the named MACHINE header gets lenses");
        for lens in &lenses {
            assert_eq!(lens.range.start.line, 2);
            let cmd = lens.command.as_ref().unwrap();
            assert_eq!(cmd.arguments.as_ref().unwrap()[1], serde_json::json!("m"));
        }
    }

    #[test]
    fn verdict_messages_name_the_machine_and_the_outcome() {
        let (kind, message) = verdict_message(
            "m",
            &report::Verdict::CheckOk {
                reason: "exhaustive".into(),
                states: 15,
            },
            1000,
        );
        assert_eq!(kind, MessageType::INFO);
        assert!(message.contains("15 states, exhaustive"), "{message}");

        let (kind, message) = verdict_message(
            "m",
            &report::Verdict::PoNoCounterexample {
                open: 3,
                total: 5,
                spurious: 1,
            },
            1000,
        );
        assert_eq!(kind, MessageType::INFO);
        assert!(message.contains("3 of 5 unproven"), "{message}");
        assert!(message.contains("1 spurious"), "{message}");
    }

    #[test]
    fn tool_failure_toasts_are_short_and_point_at_the_log() {
        let toast = error_toast(
            "Model-check",
            &AnimateError::ToolFailed("java.lang.Whatever stack tail".into()),
        );
        assert!(toast.contains("log"), "{toast}");
        assert!(!toast.contains("stack tail"), "{toast}");

        // Every other variant's Display is already short and stays the toast.
        let toast = error_toast("Model-check", &AnimateError::Timeout(60));
        assert!(toast.contains("timed out after 60 s"), "{toast}");
    }

    #[test]
    fn error_verdict_toasts_are_short_and_point_at_diagnostics() {
        // The tool's message lives in the machine-header finding; the toast
        // only points there.
        let payload = "Error loading model: long ProB loader exception text";
        for verdict in [
            report::Verdict::LoadError {
                message: payload.into(),
            },
            report::Verdict::EngineError {
                message: payload.into(),
            },
            report::Verdict::PoError {
                message: payload.into(),
            },
        ] {
            let (kind, message) = verdict_message("m", &verdict, 1000);
            assert_eq!(kind, MessageType::ERROR);
            assert!(message.contains("see diagnostics"), "{message}");
            assert!(message.contains('m'), "{message}");
            assert!(!message.contains(payload), "{message}");
        }
    }
}
