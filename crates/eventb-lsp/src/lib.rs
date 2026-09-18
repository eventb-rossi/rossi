//! Event-B Language Server Library
//!
//! This library provides the implementation of the Language Server Protocol (LSP)
//! for the Event-B formal modeling language.

use anyhow::Result;
use tower_lsp::{LspService, Server};
use tracing::info;

// Re-export tower-lsp's protocol types so this crate cannot drift to a
// different lsp-types version than the server framework uses internally.
pub use tower_lsp::lsp_types;

// Re-export modules for testing and library use
pub mod analysis;
pub mod animate;
pub mod code_actions;
pub mod completion;
pub mod component_loader;
pub mod component_util;
pub mod config;
pub mod cross_references;
pub mod definition;
pub mod diagnostics;
pub mod document;
pub mod document_highlight;
pub mod document_links;
pub mod folding;
pub mod formatting;
pub mod formula_walk;
pub mod hierarchy;
pub mod hover;
pub mod identifier_utils;
pub mod inlay_hints;
pub mod position;
pub(crate) mod progress;
pub mod references;
pub mod rename;
mod resolved_environment;
pub mod rodin;
pub mod selection_range;
pub mod semantic_tokens;
pub mod signature_help;
pub mod symbols;
#[cfg(test)]
pub(crate) mod test_util;
pub mod text_utils;
mod uri_identity;
pub mod workspace;

// `benchmark_support` is #[path]-included both here and into the
// dependency_environment_allocations integration test, so it must spell
// this crate's items the same way in either context.
#[cfg(test)]
extern crate self as eventb_lsp;

#[cfg(test)]
mod benchmark_metrics;
#[cfg(test)]
mod dependency_benchmark;

// The dependency traversal itself stays crate-private; these two are
// public so the allocation benchmark measures the same enumeration the
// server uses instead of keeping its own copy.
pub use resolved_environment::{DependencyScope, direct_dependencies};

// Re-export the server implementation
pub mod server;

/// Stack size for the runtime threads LSP handlers run on.
///
/// Handlers walk ASTs whose formula nesting can legitimately reach
/// [`rossi::MAX_NESTING_DEPTH`]; in debug builds such walks need more than
/// tokio's default 2 MiB worker stacks. 16 MiB gives the same headroom the
/// CLI gets from the main thread (8 MiB) with margin to spare.
pub const HANDLER_THREAD_STACK_SIZE: usize = 16 * 1024 * 1024;

/// Run the language server on a dedicated multi-thread runtime whose worker
/// threads have [`HANDLER_THREAD_STACK_SIZE`] stacks.
///
/// The `eventb-language-server` binary enters through here so the stack
/// policy stays in one place.
pub fn run_stdio_blocking() -> Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(HANDLER_THREAD_STACK_SIZE)
        .build()?
        .block_on(run_stdio())
}

/// Run the Rossi language server over stdin/stdout using the LSP stdio transport.
pub async fn run_stdio() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_writer(std::io::stderr)
        .init();

    info!("Starting Rossi Language Server");

    let (service, socket) = LspService::build(server::RossiLanguageServer::new)
        .custom_method(
            "rossi/operatorTable",
            server::RossiLanguageServer::operator_table,
        )
        .finish();
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    info!("Rossi Language Server initialized, listening on stdio");
    Server::new(stdin, stdout, socket).serve(service).await;
    info!("Rossi Language Server shutting down");

    Ok(())
}
