//! The tools, and the server that routes to them.

use std::path::PathBuf;
use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use rossi_build::Severity;
use rossi_prove::confidence::Bucket;

use crate::report::{
    BuildReport, DiagnosticCounts, DiagnosticRecord, Failure, FileRecord, ObligationCounts,
    ProjectReport, ProofSummary, ValidateReport,
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
        let mut proofs = ProofSummary::default();
        if let Some(obligations) = &loaded.obligations {
            for po in obligations.iter() {
                proofs.total += 1;
                if let Some(status) = po.status {
                    match status.bucket {
                        Bucket::Discharged => proofs.discharged += 1,
                        Bucket::Reviewed => proofs.reviewed += 1,
                        Bucket::Pending => proofs.pending += 1,
                        Bucket::Unattempted => proofs.unattempted += 1,
                    }
                    if status.broken {
                        proofs.broken += 1;
                    }
                } else {
                    proofs.unattempted += 1;
                }
            }
        }
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
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for RossiServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("rossi", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS.to_string())
    }
}
