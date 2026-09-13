//! The tools, and the server that routes to them.

use std::path::PathBuf;
use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use rossi::pretty::PrettyPrinter;
use rossi_build::Severity;
use rossi_build::pog::obligations::{Obligation, Obligations};
use rossi_build::pog::sources::{ElementKind, SourceIndex};
use rossi_prove::confidence::Bucket;

use crate::report::{
    BuildReport, ComponentProofs, DiagnosticCounts, DiagnosticRecord, Failure, FileRecord,
    HypothesisRecord, ListPosReport, ObligationCounts, ObligationRecord, ObligationReport,
    ProjectReport, ProofStatusReport, ProofSummary, SourceRecord, StatusRecord, ValidateReport,
};
use crate::workspace::{Loaded, Workspace, obligation_counts, write_build};

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
Every result is one JSON document; a failed call returns a document with an `error` field.";

/// The MCP server over one root directory.
#[derive(Clone)]
pub struct RossiServer {
    workspace: Arc<Workspace>,
    tool_router: ToolRouter<Self>,
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

fn bucket_name(bucket: Bucket) -> &'static str {
    match bucket {
        Bucket::Discharged => "discharged",
        Bucket::Reviewed => "reviewed",
        Bucket::Pending => "pending",
        Bucket::Unattempted => "unattempted",
    }
}

fn kind_name(kind: ElementKind) -> &'static str {
    match kind {
        ElementKind::Context => "context",
        ElementKind::Machine => "machine",
        ElementKind::CarrierSet => "carrier_set",
        ElementKind::Constant => "constant",
        ElementKind::Axiom => "axiom",
        ElementKind::Extends => "extends",
        ElementKind::Sees => "sees",
        ElementKind::Refines => "refines",
        ElementKind::Variable => "variable",
        ElementKind::Invariant => "invariant",
        ElementKind::Variant => "variant",
        ElementKind::Event => "event",
        ElementKind::RefinesEvent => "refines_event",
        ElementKind::Parameter => "parameter",
        ElementKind::Guard => "guard",
        ElementKind::Action => "action",
        ElementKind::Witness => "witness",
    }
}

/// One obligation as the documents carry it, its sources resolved.
fn obligation_record(po: &Obligation<'_>, sources: Option<&SourceIndex>) -> ObligationRecord {
    ObligationRecord {
        component: po.component.to_string(),
        name: po.name.to_string(),
        nature: po.nature.map_or_else(
            || po.description.to_string(),
            |nature| format!("{nature:?}"),
        ),
        description: po.description.to_string(),
        accurate: po.accurate,
        stamp: po.stamp.map(str::to_string),
        sources: po
            .sources
            .iter()
            .filter_map(|(role, handle)| {
                let element = sources?.resolve(handle.as_deref()?)?;
                Some(SourceRecord {
                    role: role.clone(),
                    component: element.component,
                    kind: kind_name(element.kind).to_string(),
                    name: element.name,
                    event: element.event,
                    theorem: element.theorem,
                })
            })
            .collect(),
        status: po.status.map(|status| StatusRecord {
            bucket: bucket_name(status.bucket).to_string(),
            confidence: status.confidence,
            broken: status.broken,
            manual: status.manual,
            stale: status.stale,
        }),
    }
}

/// The status buckets of the obligations `select` admits.
fn proof_summary<'a>(
    obligations: &'a Obligations,
    select: impl Fn(&Obligation<'a>) -> bool,
) -> ProofSummary {
    let mut proofs = ProofSummary::default();
    for po in obligations.iter().filter(select) {
        proofs.total += 1;
        match po.status {
            Some(status) => {
                match status.bucket {
                    Bucket::Discharged => proofs.discharged += 1,
                    Bucket::Reviewed => proofs.reviewed += 1,
                    Bucket::Pending => proofs.pending += 1,
                    Bucket::Unattempted => proofs.unattempted += 1,
                }
                if status.broken {
                    proofs.broken += 1;
                }
            }
            None => proofs.unattempted += 1,
        }
    }
    proofs
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

#[tool_router]
impl RossiServer {
    /// A server over the project under `root`.
    pub fn new(root: PathBuf) -> Self {
        RossiServer {
            workspace: Arc::new(Workspace::new(root)),
            tool_router: Self::tool_router(),
        }
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
        let diagnostics = DiagnosticCounts::of(&loaded.diagnostics(false));
        Ok(Json(ProjectReport {
            root: self.workspace.root().display().to_string(),
            name: loaded.name.clone(),
            kind: loaded.kind.as_str().to_string(),
            output_dir: loaded.output_dir.display().to_string(),
            components: loaded.components.clone(),
            edges: loaded.edges.clone(),
            diagnostics,
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
        let minimum = match args.severity.as_deref() {
            // The well-definedness conditions are `info`, so asking for
            // them without naming a severity would otherwise filter every
            // one of them back out.
            None if args.include_wd => Severity::Info,
            None | Some("warning") => Severity::Warning,
            Some("error") => Severity::Error,
            Some("info") => Severity::Info,
            Some(other) => {
                return Err(failure(
                    format!("unknown severity `{other}`: use error, warning or info"),
                    Vec::new(),
                ));
            }
        };
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
        let counts = DiagnosticCounts::of(&diagnostics);
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
        let Some(build) = &loaded.build else {
            return Err(failure("the model does not parse", diagnostics));
        };
        if args.write {
            write_build(&loaded).map_err(|error| failure(error, Vec::new()))?;
        }
        let counts = DiagnosticCounts::of(&diagnostics);
        let by_nature = obligation_counts(&loaded);
        let proofs = loaded
            .obligations
            .as_ref()
            .map_or_else(ProofSummary::default, |obligations| {
                proof_summary(obligations, |_| true)
            });
        Ok(Json(BuildReport {
            output_dir: loaded.output_dir.display().to_string(),
            written: args.write,
            files: build
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
        loaded.obligations.as_ref().ok_or_else(|| {
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
        let sources = loaded.sources.as_ref();
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
        let matching: Vec<ObligationRecord> = obligations
            .iter()
            .filter(|po| {
                args.component
                    .as_deref()
                    .is_none_or(|wanted| po.component == wanted)
            })
            .map(|po| obligation_record(&po, sources))
            .filter(|record| args.nature.as_deref().is_none_or(|n| record.nature == n))
            .filter(|record| {
                status_filter.is_none_or(|wanted| {
                    let bucket = record
                        .status
                        .as_ref()
                        .map_or("unattempted", |s| s.bucket.as_str());
                    if wanted == "open" {
                        bucket != "discharged"
                    } else {
                        bucket == wanted
                    }
                })
            })
            .filter(|record| {
                args.element.as_deref().is_none_or(|wanted| {
                    record.sources.iter().any(|source| {
                        source.name == wanted
                            || source
                                .event
                                .as_deref()
                                .is_some_and(|event| format!("{event}/{}", source.name) == wanted)
                    })
                })
            })
            .collect();
        let total = matching.len();
        let limit = args.limit.max(1);
        let page: Vec<ObligationRecord> =
            matching.into_iter().skip(args.offset).take(limit).collect();
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
        let record = obligation_record(&po, loaded.sources.as_ref());
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
        let components: Vec<ComponentProofs> = obligations
            .components()
            .filter(|component| {
                args.component
                    .as_deref()
                    .is_none_or(|wanted| *component == wanted)
            })
            .map(|component| ComponentProofs {
                component: component.to_string(),
                summary: proof_summary(obligations, |po| po.component == component),
            })
            .collect();
        let summary = proof_summary(obligations, |po| {
            args.component
                .as_deref()
                .is_none_or(|wanted| po.component == wanted)
        });
        Ok(Json(ProofStatusReport {
            summary,
            components,
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
