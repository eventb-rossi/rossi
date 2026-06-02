//! Rossi Language Server Library
//!
//! This library provides the implementation of the Language Server Protocol (LSP)
//! for Event-B formal modeling language.

use anyhow::Result;
use tower_lsp::{LspService, Server};
use tracing::info;

// Re-export tower-lsp's protocol types so this crate cannot drift to a
// different lsp-types version than the server framework uses internally.
pub use tower_lsp::lsp_types;

// Re-export modules for testing and library use
pub mod analysis;
pub mod code_actions;
pub mod completion;
pub mod config;
pub mod cross_references;
pub mod definition;
pub mod document;
pub mod document_links;
pub mod folding;
pub mod formatting;
pub mod hover;
pub mod identifier_utils;
pub mod prob;
pub mod references;
pub mod rename;
pub mod semantic_tokens;
pub mod signature_help;
pub mod symbols;
pub mod text_utils;
pub mod workspace;

// Re-export the server implementation
pub mod server;

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
