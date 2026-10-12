//! The tools, and the server that routes to them.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use eventb_animate_driver::report::{Verdict, classify_check, classify_po, po_counts};
use eventb_animate_driver::{AnimateConfig, ProbSettings, Run};
use rossi::pretty::PrettyPrinter;
use rossi_build::pog::obligations::{Obligation, ObligationStatus, Obligations};
use rossi_build::pog::sources::SourceIndex;
use rossi_prove::confidence::Bucket;

use crate::animate;
use crate::report::{
    BindingRecord, BuildReport, CheckRecord, CheckRunReport, ComponentProofs, CounterexampleRecord,
    DiagnosticCounts, DiagnosticRecord, DisproofRecord, DisproveReport, DisproveVerdict, Failure,
    FileRecord, HypothesisRecord, ListPosReport, ModelCheckReport, ObligationCounts,
    ObligationRecord, ObligationReport, ProjectReport, ProofStatusReport, ProofSummary,
    SourceRecord, StatusRecord, ValidateReport, VerdictRecord,
};
use crate::workspace::{
    Loaded, Statuses, Workspace, obligation_counts, severity_floor, write_build,
};

/// What the client is told at the handshake.
const INSTRUCTIONS: &str = "Rossi verifies the Event-B model under the server's root directory. \
Author or edit the `.eventb` files there with your own tools; the server only reads them. \
`project` describes the components and their SEES/EXTENDS/REFINES edges. \
`validate` reports syntax, scoping, typing, refinement and style findings with EBnnn rule codes \
and source regions; a finding of severity `error` means the model does not check. \
`build` static-checks the model, generates its proof obligations, reconciles their recorded \
status with the previous build, and writes the checked files under `.rossi/build/<project>/`. \
`list_pos` pages through the proof obligations with their nature, the elements they come from \
and their recorded status; `get_po` shows one obligation's sequent; `proof_status` sums up the \
recorded statuses. \
`model_check` explores the reachable states of a machine with ProB for invariant violations and \
deadlocks and returns the counterexample trace; `check_invariants_cbc` searches, event by event, \
for a state from which one step breaks an invariant; `check_wd` asks ProB to discharge the \
well-definedness obligations; `disprove_po` searches for a counterexample to each open proof \
obligation. `model_check` and `check_invariants_cbc` take `bounds`, predicates over the constants \
(`n < 5`) that constrain the run without changing the model. \
Every result is one JSON document; a failed call returns a document with an `error` field.";

/// The MCP server over one root directory.
#[derive(Clone)]
pub struct RossiServer {
    workspace: Arc<Workspace>,
    animate: AnimateConfig,
    tool_router: ToolRouter<Self>,
}

/// A model-checking run, written out and ready to start.
struct Prepared {
    machine: String,
    /// The throwaway project; removed when dropped.
    dir: tempfile::TempDir,
    /// The machine's obligations, for the disprover's deadline.
    po_count: usize,
    bounds: Vec<String>,
}

/// Arguments of `model_check`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ModelCheckArgs {
    /// The machine to check (default: the most refined one, when the
    /// project has exactly one).
    #[serde(default)]
    pub machine: Option<String>,
    /// Wall-clock limit of the search, in seconds (default 30).
    #[serde(default)]
    pub time_limit_secs: Option<u32>,
    /// Stop after this many explored states.
    #[serde(default)]
    pub states: Option<u64>,
    /// ProB's default size for the carrier sets (default 4).
    #[serde(default)]
    pub set_size: Option<u32>,
    /// ProB preferences, as `KEY=VALUE`.
    #[serde(default)]
    pub prefs: Vec<String>,
    /// Predicates over the constants (`n < 5`) the run is constrained by;
    /// they become axioms of a scratch context the machine sees instead
    /// of its own, so the model is not changed.
    #[serde(default)]
    pub bounds: Vec<String>,
    /// Do not search for deadlocks.
    #[serde(default)]
    pub no_deadlock: bool,
    /// Do not search for invariant violations.
    #[serde(default)]
    pub no_invariant: bool,
    /// Also check the theorems.
    #[serde(default)]
    pub assertions: bool,
    /// Also search for a reachable state satisfying this predicate; a
    /// hit is reported as a finding.
    #[serde(default)]
    pub goal: Option<String>,
}

/// Arguments of `check_invariants_cbc`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct CbcArgs {
    /// The machine to check (default: the most refined one, when the
    /// project has exactly one).
    #[serde(default)]
    pub machine: Option<String>,
    /// Only these events (default: every event).
    #[serde(default)]
    pub events: Vec<String>,
    /// Also search for a deadlocking state that satisfies the invariant.
    #[serde(default)]
    pub deadlock: bool,
    /// ProB's default size for the carrier sets (default 4).
    #[serde(default)]
    pub set_size: Option<u32>,
    /// ProB preferences, as `KEY=VALUE`.
    #[serde(default)]
    pub prefs: Vec<String>,
    /// Predicates over the constants the run is constrained by.
    #[serde(default)]
    pub bounds: Vec<String>,
}

/// Arguments of `check_wd`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct WdArgs {
    /// The machine whose obligations to check (default: the most refined
    /// one, when the project has exactly one).
    #[serde(default)]
    pub machine: Option<String>,
    /// ProB's default size for the carrier sets (default 4).
    #[serde(default)]
    pub set_size: Option<u32>,
    /// ProB preferences, as `KEY=VALUE`.
    #[serde(default)]
    pub prefs: Vec<String>,
}

/// Arguments of `disprove_po`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct DisproveArgs {
    /// The machine whose refinement chain to look at (default: the most
    /// refined one, when the project has exactly one).
    #[serde(default)]
    pub machine: Option<String>,
    /// Only these obligations, by qualified name
    /// (`<component>/<obligation>`, e.g. `M1/evt/inv1/INV`).
    #[serde(default)]
    pub names: Vec<String>,
    /// Only the obligations whose qualified name matches this glob
    /// (`M1/*`, `*/INV`).
    #[serde(default)]
    pub filter: Option<String>,
    /// Solver time per obligation, in milliseconds (default 1000).
    #[serde(default)]
    pub disprove_timeout_ms: Option<u32>,
    /// ProB's default size for the carrier sets (default 4).
    #[serde(default)]
    pub set_size: Option<u32>,
    /// ProB preferences, as `KEY=VALUE`.
    #[serde(default)]
    pub prefs: Vec<String>,
}

fn bindings(bindings: &[eventb_animate_driver::report::StateBinding]) -> Vec<BindingRecord> {
    bindings
        .iter()
        .map(|binding| BindingRecord {
            name: binding.name.clone(),
            value: binding.value.clone(),
        })
        .collect()
}

fn checks(report: &eventb_animate_driver::report::Report) -> Vec<CheckRecord> {
    report
        .checks
        .iter()
        .map(|check| CheckRecord {
            name: check.name.clone(),
            outcome: check.outcome.clone(),
            message: check.message.clone(),
            bindings: bindings(&check.bindings),
        })
        .collect()
}

/// Arguments of `validate`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ValidateArgs {
    /// Report only the findings about this component.
    #[serde(default)]
    pub component: Option<String>,
    /// The least severity to report: `error`, `warning` or `info`
    /// (default `warning`).
    #[serde(default)]
    pub severity: Option<String>,
    /// Also report the non-trivial well-definedness conditions (rule
    /// EB010, severity `info`).
    #[serde(default)]
    pub include_wd: bool,
}

/// Arguments of `build`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct BuildArgs {
    /// Write the checked and proof files under the output directory
    /// (default `true`); `false` only reports what a build produces.
    #[serde(default = "default_true")]
    pub write: bool,
}

impl Default for BuildArgs {
    fn default() -> Self {
        BuildArgs { write: true }
    }
}

fn default_true() -> bool {
    true
}

/// Arguments of `list_pos`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ListPosArgs {
    /// Only the obligations of this component.
    #[serde(default)]
    pub component: Option<String>,
    /// Only the obligations of this nature, by name
    /// (`InvariantPreservation`, `GuardStrengtheningSplit`, ...).
    #[serde(default)]
    pub nature: Option<String>,
    /// Only the obligations in this status bucket (`discharged`,
    /// `reviewed`, `pending`, `unattempted`), or `open` for every bucket
    /// but `discharged`.
    #[serde(default)]
    pub status: Option<String>,
    /// Only the obligations that trace back to this element: a label or
    /// identifier (`inv1`, `evt`), or `event/label` for an element of an
    /// event (`evt/grd1`).
    #[serde(default)]
    pub element: Option<String>,
    /// The first obligation of the page (default 0).
    #[serde(default)]
    pub offset: usize,
    /// The page size (default 100).
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    100
}

/// Arguments of `get_po`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetPoArgs {
    /// The component whose obligation file holds the obligation.
    pub component: String,
    /// The obligation's name, e.g. `evt/inv1/INV`.
    pub name: String,
    /// At most this many hypotheses (default 200); the document says how
    /// many there are in all.
    #[serde(default = "default_max_hypotheses")]
    pub max_hypotheses: usize,
}

fn default_max_hypotheses() -> usize {
    200
}

/// Arguments of `proof_status`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ProofStatusArgs {
    /// Only this component.
    #[serde(default)]
    pub component: Option<String>,
}

/// One obligation as the documents carry it, its sources resolved.
fn obligation_record(po: &Obligation<'_>, sources: &SourceIndex) -> ObligationRecord {
    ObligationRecord {
        component: po.component.to_string(),
        name: po.name.to_string(),
        nature: nature_name(po),
        description: po.description.to_string(),
        accurate: po.accurate,
        stamp: po.stamp.map(str::to_string),
        sources: po
            .sources
            .iter()
            .filter_map(|(role, handle)| {
                let element = sources.resolve(handle)?;
                Some(SourceRecord {
                    role: role.clone(),
                    component: element.component,
                    kind: element.kind.as_str().to_string(),
                    name: element.name,
                    event: element.event,
                    theorem: element.theorem,
                })
            })
            .collect(),
        status: po.status.map(|status| StatusRecord {
            bucket: status.bucket.as_str().to_string(),
            confidence: status.confidence,
            broken: status.broken,
            manual: status.manual,
            stale: status.stale,
        }),
    }
}

/// The name an obligation's nature is reported and filtered under, or
/// its description when the generator gave it one this build does not
/// know.
fn nature_name(po: &Obligation<'_>) -> String {
    po.nature.map_or_else(
        || po.description.to_string(),
        |nature| nature.name().to_string(),
    )
}

/// A tool-level failure: the document the client gets instead of the
/// result, marked as an error.
fn failure(error: impl Into<String>, diagnostics: Vec<DiagnosticRecord>) -> CallToolResult {
    let failure = Failure {
        error: error.into(),
        diagnostics,
    };
    CallToolResult::structured_error(serde_json::to_value(failure).unwrap_or_default())
}

/// Add one obligation's recorded status to a summary. An obligation
/// with no row was never attempted.
fn count_status(summary: &mut ProofSummary, status: Option<ObligationStatus>) {
    summary.total += 1;
    let Some(status) = status else {
        summary.unattempted += 1;
        return;
    };
    match status.bucket {
        Bucket::Discharged => summary.discharged += 1,
        Bucket::Reviewed => summary.reviewed += 1,
        Bucket::Pending => summary.pending += 1,
        Bucket::Unattempted => summary.unattempted += 1,
    }
    if status.broken {
        summary.broken += 1;
    }
}

/// The status buckets of every obligation.
fn proof_summary(obligations: &Obligations) -> ProofSummary {
    let mut summary = ProofSummary::default();
    for po in obligations.iter() {
        count_status(&mut summary, po.status);
    }
    summary
}

#[tool_router]
impl RossiServer {
    /// A server over the project under `root`.
    pub fn new(root: PathBuf) -> Self {
        RossiServer {
            workspace: Arc::new(Workspace::new(root)),
            animate: AnimateConfig::default(),
            tool_router: Self::tool_router(),
        }
    }

    /// The same server, running the model checker under `config`.
    pub fn with_animate(mut self, config: AnimateConfig) -> Self {
        self.animate = config;
        self
    }

    /// The loaded project written out as a project to check, bounded when
    /// `bounds` are given. A model that does not check is not run: the
    /// tool would verify a different model.
    async fn prepare(
        &self,
        machine: Option<String>,
        bounds: Vec<String>,
        run: &Run,
    ) -> Result<(Arc<Loaded>, Prepared), CallToolResult> {
        let loaded = self.loaded().await?;
        if loaded.checks().is_none() {
            return Err(failure(
                "the model does not check; fix the errors first",
                loaded.diagnostics(false),
            ));
        }
        let machine = animate::select_machine(&loaded, machine.as_deref())
            .map_err(|error| failure(error, Vec::new()))?;
        // Only the disprover's gate reads the recorded verdicts, to skip
        // an obligation already discharged; every other run must judge
        // the model on its own.
        let statuses = match run {
            Run::Po { .. } => Statuses::Recorded,
            _ => Statuses::Generated,
        };
        let shared = Arc::clone(&loaded);
        let prepared = tokio::task::spawn_blocking(move || -> Result<Prepared, CallToolResult> {
            let checked = shared.checks().expect("checked above");
            let components = animate::bounded_components(&checked.project, &machine, &bounds)
                .map_err(|error| failure(error, Vec::new()))?;
            let files = if bounds.is_empty() {
                shared.files_for(statuses)
            } else {
                // A bounded run rebuilds, so its statuses are the
                // generated ones whatever the run wanted; only the
                // disprover asks for the recorded ones and it takes no
                // bounds.
                animate::build_files(&shared.name, &components).map_err(|diagnostics| {
                    let texts = shared.text_index();
                    failure(
                        "the bounds do not check against the model",
                        diagnostics
                            .iter()
                            .map(|d| DiagnosticRecord::of_diagnostic(d, &texts))
                            .collect(),
                    )
                })?
            };
            let po_count = rossi_build::pog::sequent_count(&files, &machine);
            let dir = animate::write_project(&shared.name, &components, &files)
                .map_err(|error| failure(error, Vec::new()))?;
            Ok(Prepared {
                machine,
                dir,
                po_count,
                bounds,
            })
        })
        .await
        .map_err(|e| failure(format!("the preparation task failed: {e}"), Vec::new()))??;
        Ok((loaded, prepared))
    }

    /// Run the tool over a prepared project and read its report.
    async fn animate(
        &self,
        prepared: &Prepared,
        run: &Run,
        settings: &ProbSettings,
    ) -> Result<(eventb_animate_driver::report::Report, Option<i32>), CallToolResult> {
        animate::run(
            &self.animate,
            run,
            settings,
            &prepared.machine,
            prepared.dir.path(),
            prepared.po_count,
        )
        .await
        .map_err(|error| failure(error.to_string(), Vec::new()))
    }

    async fn loaded(&self) -> Result<Arc<Loaded>, CallToolResult> {
        self.workspace
            .load()
            .await
            .map_err(|error| failure(error, Vec::new()))
    }

    #[tool(
        name = "project",
        description = "Describe the Event-B project under the root: its components, the files they come from, their SEES/EXTENDS/REFINES edges, and how many findings it has.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn project(&self) -> Result<Json<ProjectReport>, CallToolResult> {
        let loaded = self.loaded().await?;
        Ok(Json(ProjectReport {
            root: self.workspace.root().display().to_string(),
            name: loaded.name.clone(),
            kind: loaded.kind.as_str().to_string(),
            output_dir: loaded.output_dir.display().to_string(),
            components: loaded.components.clone(),
            edges: loaded.edges.clone(),
            diagnostics: loaded.counts(false),
        }))
    }

    #[tool(
        name = "validate",
        description = "Check the model and report its findings: parse errors, undeclared identifiers, type errors, broken cross-references, refinement errors, and advisory lints, each with its EBnnn rule code, the component and element it is about, and its source region.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn validate(
        &self,
        Parameters(args): Parameters<ValidateArgs>,
    ) -> Result<Json<ValidateReport>, CallToolResult> {
        let loaded = self.loaded().await?;
        let minimum = severity_floor(args.severity.as_deref(), args.include_wd)
            .map_err(|error| failure(error, Vec::new()))?;
        let diagnostics: Vec<DiagnosticRecord> = loaded
            .diagnostics(args.include_wd)
            .into_iter()
            .filter(|record| record.at_least(minimum))
            .filter(|record| {
                args.component
                    .as_deref()
                    .is_none_or(|wanted| record.component.as_deref() == Some(wanted))
            })
            .collect();
        let counts = DiagnosticCounts::of(diagnostics.iter().map(|record| record.severity));
        Ok(Json(ValidateReport {
            diagnostics,
            counts,
        }))
    }

    #[tool(
        name = "build",
        description = "Static-check the model, generate its proof obligations, reconcile their recorded proof status with the previous build, and write the checked files (.bcc/.bcm) and proof files (.bpo/.bps) under the output directory. Reports the files, the findings, the obligation counts by nature, and the proof status summary.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn build(
        &self,
        Parameters(args): Parameters<BuildArgs>,
    ) -> Result<Json<BuildReport>, CallToolResult> {
        let loaded = self.loaded().await?;
        let diagnostics = loaded.diagnostics(false);
        let Some(checked) = &loaded.checked else {
            return Err(failure("the model does not parse", diagnostics));
        };
        if args.write {
            write_build(&loaded).map_err(|error| failure(error, Vec::new()))?;
        }
        let counts = DiagnosticCounts::of(diagnostics.iter().map(|record| record.severity));
        let by_nature = obligation_counts(&loaded);
        let proofs = loaded
            .obligations()
            .map_or_else(ProofSummary::default, proof_summary);
        Ok(Json(BuildReport {
            output_dir: loaded.output_dir.display().to_string(),
            written: args.write,
            files: checked
                .build
                .files
                .iter()
                .map(|file| FileRecord {
                    filename: file.filename.clone(),
                    accurate: file.accurate,
                })
                .collect(),
            diagnostics,
            counts,
            obligations: ObligationCounts {
                total: by_nature.values().sum(),
                by_nature,
            },
            proofs,
        }))
    }

    /// The obligations of the load, or the failure to report instead.
    fn obligations<'a>(&self, loaded: &'a Loaded) -> Result<&'a Obligations, CallToolResult> {
        loaded.obligations().ok_or_else(|| {
            failure(
                "no proof obligations: the model does not check",
                loaded.diagnostics(false),
            )
        })
    }

    #[tool(
        name = "list_pos",
        description = "List the proof obligations, one page at a time: each with its nature, the elements it traces back to (invariant, guard, action, event, axiom, ...), and its recorded proof status. Filter by component, nature, status bucket (or `open`) and element.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn list_pos(
        &self,
        Parameters(args): Parameters<ListPosArgs>,
    ) -> Result<Json<ListPosReport>, CallToolResult> {
        let loaded = self.loaded().await?;
        let obligations = self.obligations(&loaded)?;
        let checked = loaded.checked.as_ref().expect("obligations imply a check");
        let status_filter = match args.status.as_deref() {
            None => None,
            Some(name @ ("discharged" | "reviewed" | "pending" | "unattempted" | "open")) => {
                Some(name)
            }
            Some(other) => {
                return Err(failure(
                    format!(
                        "unknown status `{other}`: use discharged, reviewed, pending, unattempted or open"
                    ),
                    Vec::new(),
                ));
            }
        };
        if let Some(wanted) = args.nature.as_deref()
            && rossi_build::pog::natures::Nature::from_name(wanted).is_none()
        {
            return Err(failure(
                format!("unknown nature `{wanted}`; see a build report for the ones generated"),
                Vec::new(),
            ));
        }
        // The filters read the obligation itself, so only the page that
        // survives them is turned into records: an element filter over a
        // large refinement chain would otherwise resolve every
        // obligation's provenance to return ten rows.
        let matching: Vec<Obligation<'_>> = obligations
            .iter()
            .filter(|po| {
                args.component
                    .as_deref()
                    .is_none_or(|wanted| po.component == wanted)
            })
            .filter(|po| args.nature.as_deref().is_none_or(|n| nature_name(po) == n))
            .filter(|po| {
                status_filter.is_none_or(|wanted| {
                    let bucket = po
                        .status
                        .map_or(Bucket::Unattempted, |status| status.bucket)
                        .as_str();
                    if wanted == "open" {
                        bucket != "discharged"
                    } else {
                        bucket == wanted
                    }
                })
            })
            .filter(|po| {
                args.element.as_deref().is_none_or(|wanted| {
                    po.sources.iter().any(|(_, handle)| {
                        checked.sources.resolve(handle).is_some_and(|element| {
                            element.name == wanted
                                || element.event.is_some_and(|event| {
                                    format!("{event}/{}", element.name) == wanted
                                })
                        })
                    })
                })
            })
            .collect();
        let total = matching.len();
        let limit = args.limit.max(1);
        let page: Vec<ObligationRecord> = matching
            .iter()
            .skip(args.offset)
            .take(limit)
            .map(|po| obligation_record(po, &checked.sources))
            .collect();
        let next_offset = (args.offset + page.len() < total).then(|| args.offset + page.len());
        Ok(Json(ListPosReport {
            obligations: page,
            total,
            offset: args.offset,
            next_offset,
        }))
    }

    #[tool(
        name = "get_po",
        description = "Show one proof obligation as the prover sees it: the typed identifiers in scope, the hypotheses (marked when the obligation's hints select them), and the goal, in Event-B notation, with the obligation's nature, sources and status.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn get_po(
        &self,
        Parameters(args): Parameters<GetPoArgs>,
    ) -> Result<Json<ObligationReport>, CallToolResult> {
        let loaded = self.loaded().await?;
        let obligations = self.obligations(&loaded)?;
        let Some(po) = obligations.get(&args.component, &args.name) else {
            return Err(failure(
                format!(
                    "no obligation `{}` in component `{}`",
                    args.name, args.component
                ),
                Vec::new(),
            ));
        };
        let checked = loaded.checked.as_ref().expect("obligations imply a check");
        let record = obligation_record(&po, &checked.sources);
        let sequent = obligations
            .sequent(&args.component, &args.name)
            .map_err(|error| {
                failure(format!("the obligation does not load: {error}"), Vec::new())
            })?;
        let printer = PrettyPrinter::rodin_formula_string();
        let identifiers = sequent
            .type_env()
            .iter()
            .map(|(name, ty)| (name.to_string(), ty.to_rodin_canonical()))
            .collect();
        let hypotheses_total = sequent.hyp_iter().count();
        let hypotheses: Vec<HypothesisRecord> = sequent
            .hyp_iter()
            .take(args.max_hypotheses)
            .enumerate()
            .map(|(index, hypothesis)| HypothesisRecord {
                index,
                text: printer.print_formula_predicate(hypothesis),
                selected: sequent.is_selected(hypothesis),
            })
            .collect();
        let truncated = hypotheses.len() < hypotheses_total;
        Ok(Json(ObligationReport {
            obligation: record,
            identifiers,
            hypotheses,
            hypotheses_total,
            truncated,
            goal: printer.print_formula_predicate(sequent.goal()),
        }))
    }

    #[tool(
        name = "proof_status",
        description = "Sum up the recorded proof status of the obligations: how many are discharged, reviewed, pending, unattempted or broken, in all and per component.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn proof_status(
        &self,
        Parameters(args): Parameters<ProofStatusArgs>,
    ) -> Result<Json<ProofStatusReport>, CallToolResult> {
        let loaded = self.loaded().await?;
        let obligations = self.obligations(&loaded)?;
        let wanted = args.component.as_deref();
        // One pass: a summary per component and the total together, so a
        // long refinement chain is not walked once per component.
        let mut per_component: BTreeMap<&str, ProofSummary> = BTreeMap::new();
        let mut summary = ProofSummary::default();
        for po in obligations
            .iter()
            .filter(|po| wanted.is_none_or(|wanted| po.component == wanted))
        {
            count_status(&mut summary, po.status);
            count_status(per_component.entry(po.component).or_default(), po.status);
        }
        Ok(Json(ProofStatusReport {
            summary,
            components: per_component
                .into_iter()
                .map(|(component, summary)| ComponentProofs {
                    component: component.to_string(),
                    summary,
                })
                .collect(),
        }))
    }

    #[tool(
        name = "model_check",
        description = "Model-check a machine with ProB: explore its reachable states for invariant violations and deadlocks (and a goal state or theorem violations when asked) under a time limit, and report the verdict with the counterexample trace, the violating state and the violated invariants mapped back to their labels.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn model_check(
        &self,
        Parameters(args): Parameters<ModelCheckArgs>,
    ) -> Result<Json<ModelCheckReport>, CallToolResult> {
        let run = Run::Check {
            time_limit_secs: args.time_limit_secs.unwrap_or(30),
            states: args.states,
            no_deadlock: args.no_deadlock,
            no_invariant: args.no_invariant,
            assertions: args.assertions,
            goal: args.goal,
        };
        let settings = ProbSettings {
            set_size: args.set_size,
            prefs: args.prefs,
        };
        let (loaded, prepared) = self.prepare(args.machine, args.bounds, &run).await?;
        let (report, code) = self.animate(&prepared, &run, &settings).await?;
        let transitions = report
            .counterexample
            .as_ref()
            .map(|cx| cx.transitions.clone())
            .unwrap_or_default();
        let verdict = |kind: &str| VerdictRecord {
            kind: kind.to_string(),
            reason: None,
            states: None,
            category: None,
            message: None,
        };
        let (verdict, counterexample) = match classify_check(&report, code) {
            Verdict::CheckOk { reason, states } => (
                VerdictRecord {
                    reason: Some(reason),
                    states: Some(states),
                    ..verdict("ok")
                },
                None,
            ),
            Verdict::CheckIncomplete { reason } => (
                VerdictRecord {
                    reason: Some(reason),
                    ..verdict("incomplete")
                },
                None,
            ),
            Verdict::InvariantViolation {
                violated,
                state,
                bindings: state_bindings,
                steps,
            } => (
                verdict("invariant_violation"),
                Some(CounterexampleRecord {
                    transitions,
                    violating_state: state,
                    violated_invariants: animate::violated_invariants(
                        &violated,
                        &loaded,
                        &prepared.machine,
                    ),
                    bindings: bindings(&state_bindings),
                    steps,
                }),
            ),
            Verdict::Deadlock {
                state,
                bindings: state_bindings,
                steps,
            } => (
                verdict("deadlock"),
                Some(CounterexampleRecord {
                    transitions,
                    violating_state: state,
                    violated_invariants: Vec::new(),
                    bindings: bindings(&state_bindings),
                    steps,
                }),
            ),
            Verdict::OtherFinding { category, message } => (
                VerdictRecord {
                    category: Some(category),
                    message: Some(message),
                    ..verdict("finding")
                },
                None,
            ),
            Verdict::LoadError { message } => (
                VerdictRecord {
                    message: Some(message),
                    ..verdict("load_error")
                },
                None,
            ),
            Verdict::EngineError { message } => (
                VerdictRecord {
                    message: Some(message),
                    ..verdict("engine_error")
                },
                None,
            ),
            // Only a po run classifies into these.
            Verdict::PoDisproved { .. }
            | Verdict::PoNoCounterexample { .. }
            | Verdict::PoOk { .. }
            | Verdict::PoError { .. } => (verdict("engine_error"), None),
        };
        Ok(Json(ModelCheckReport {
            machine: prepared.machine.clone(),
            bounds: prepared.bounds.clone(),
            status: report.status.clone(),
            message: report.message.clone().unwrap_or_default(),
            verdict,
            counterexample,
            exit_code: code,
        }))
    }

    #[tool(
        name = "check_invariants_cbc",
        description = "Check invariant preservation event by event with ProB's constraint solver, without exploring the state space: for each event, search for a state satisfying the invariant (reachable or not) from which one step violates it. A hit is a two-step counterexample; no hit is a preservation proof for the event (initialisation is not checked).",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn check_invariants_cbc(
        &self,
        Parameters(args): Parameters<CbcArgs>,
    ) -> Result<Json<CheckRunReport>, CallToolResult> {
        self.check_run(
            args.machine,
            args.bounds,
            Run::Cbc {
                events: args.events,
                deadlock: args.deadlock,
            },
            ProbSettings {
                set_size: args.set_size,
                prefs: args.prefs,
            },
        )
        .await
    }

    #[tool(
        name = "check_wd",
        description = "Ask ProB's well-definedness prover to discharge the machine's well-definedness obligations (partial function applications, divisions, minimum and maximum of sets); reports how many it discharged. An undischarged obligation is unproven, not disproven.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn check_wd(
        &self,
        Parameters(args): Parameters<WdArgs>,
    ) -> Result<Json<CheckRunReport>, CallToolResult> {
        self.check_run(
            args.machine,
            Vec::new(),
            Run::Wd,
            ProbSettings {
                set_size: args.set_size,
                prefs: args.prefs,
            },
        )
        .await
    }

    /// One run whose report is a list of checks: the constraint-based
    /// preservation check and the well-definedness prover both answer in
    /// that shape.
    async fn check_run(
        &self,
        machine: Option<String>,
        bounds: Vec<String>,
        run: Run,
        settings: ProbSettings,
    ) -> Result<Json<CheckRunReport>, CallToolResult> {
        let (_, prepared) = self.prepare(machine, bounds, &run).await?;
        let (report, code) = self.animate(&prepared, &run, &settings).await?;
        Ok(Json(CheckRunReport {
            machine: prepared.machine.clone(),
            bounds: prepared.bounds.clone(),
            status: report.status.clone(),
            message: report.message.clone().unwrap_or_default(),
            checks: checks(&report),
            exit_code: code,
        }))
    }

    #[tool(
        name = "disprove_po",
        description = "Run ProB's constraint solver against each open proof obligation of the machine's refinement chain, looking for a counterexample to its sequent. A counterexample is a definite refutation (the obligation cannot be proved as the model stands); an obligation the solver proves passes; a timeout keeps it open.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn disprove_po(
        &self,
        Parameters(args): Parameters<DisproveArgs>,
    ) -> Result<Json<DisproveReport>, CallToolResult> {
        let mut filters = args.names;
        filters.extend(args.filter);
        let run = Run::Po {
            disprove_timeout_ms: args.disprove_timeout_ms.unwrap_or(1000),
            filters,
        };
        let settings = ProbSettings {
            set_size: args.set_size,
            prefs: args.prefs,
        };
        let (_, prepared) = self.prepare(args.machine, Vec::new(), &run).await?;
        let (report, code) = self.animate(&prepared, &run, &settings).await?;
        let rows = checks(&report);
        let counts = po_counts(&report);
        let (open, spurious) = (counts.open, counts.spurious);
        let verdict = match classify_po(&report) {
            Verdict::PoDisproved { disproved, total } => DisproveVerdict {
                kind: "disproved".to_string(),
                disproved: disproved
                    .into_iter()
                    .map(|po| DisproofRecord {
                        name: po.name,
                        message: po.message,
                        bindings: bindings(&po.bindings),
                    })
                    .collect(),
                open,
                total,
                spurious,
                message: None,
            },
            Verdict::PoNoCounterexample {
                open,
                total,
                spurious,
            } => DisproveVerdict {
                kind: "no_counterexample".to_string(),
                disproved: Vec::new(),
                open,
                total,
                spurious,
                message: None,
            },
            Verdict::PoOk { message } => DisproveVerdict {
                kind: "ok".to_string(),
                disproved: Vec::new(),
                open: 0,
                total: rows.len(),
                spurious: 0,
                message: Some(message),
            },
            Verdict::PoError { message } => DisproveVerdict {
                kind: "error".to_string(),
                disproved: Vec::new(),
                open,
                total: rows.len(),
                spurious,
                message: Some(message),
            },
            // Only a check run classifies into these.
            other => DisproveVerdict {
                kind: "error".to_string(),
                disproved: Vec::new(),
                open,
                total: rows.len(),
                spurious,
                message: Some(format!("unexpected verdict {other:?}")),
            },
        };
        Ok(Json(DisproveReport {
            machine: prepared.machine.clone(),
            status: report.status.clone(),
            message: report.message.clone().unwrap_or_default(),
            verdict,
            checks: rows,
            exit_code: code,
        }))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for RossiServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("rossi", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS.to_string())
    }
}
