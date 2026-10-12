//! `rossi mcp` — serve the Model Context Protocol tools of one project
//! directory over standard input and output.
//!
//! An LLM host starts this command with the project as its root and
//! calls the tools: validate, build, list and show the proof
//! obligations, model-check. The protocol owns standard output, so every
//! log line goes to standard error (`RUST_LOG` selects how much).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;

use eventb_animate_driver::AnimateConfig;
use eventb_mcp::RossiServer;

#[derive(Args)]
pub struct McpArgs {
    /// The project directory: a folder of `.eventb` files, or a Rodin
    /// project directory of `.bum` / `.buc` files.
    #[arg(long, default_value = ".")]
    pub root: PathBuf,
    /// The `eventb-animate` executable the model-checking tools run
    /// (default: the `EVENTB_ANIMATE` variable, else `eventb-animate` on
    /// PATH).
    #[arg(long, value_name = "PATH")]
    pub animate: Option<String>,
}

pub fn run(args: McpArgs) -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let root = match std::fs::canonicalize(&args.root) {
        Ok(root) if root.is_dir() => root,
        Ok(root) => {
            eprintln!("rossi mcp: not a directory: {}", root.display());
            return ExitCode::from(1);
        }
        Err(error) => {
            eprintln!("rossi mcp: cannot open {}: {error}", args.root.display());
            return ExitCode::from(1);
        }
    };
    let animate = AnimateConfig {
        path: args
            .animate
            .or_else(|| std::env::var("EVENTB_ANIMATE").ok())
            .unwrap_or_default(),
        ..AnimateConfig::default()
    };
    let server = RossiServer::new(root).with_animate(animate);
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("rossi mcp: cannot start the runtime: {error}");
            return ExitCode::from(1);
        }
    };
    match runtime.block_on(eventb_mcp::serve_stdio(server)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("rossi mcp: {error}");
            ExitCode::from(1)
        }
    }
}
