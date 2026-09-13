//! Model Context Protocol server for the Rossi Event-B toolchain.
//!
//! The server is the toolchain's agent-facing surface. A client (an LLM
//! host such as Claude Code) calls typed tools and gets JSON documents
//! back: the project's shape, its diagnostics with source regions, the
//! outcome of a build, its proof obligations and their status, and the
//! verdicts of the model checker. The client edits the `.eventb` files
//! under the root with its own tools; the server only reads them.
//!
//! Every tool is a thin call into the library crates, so nothing about
//! Event-B is decided here. The project is reloaded whenever the files
//! under the root change (a content digest decides), and a load is also
//! a build: the generated proof files are reconciled against the previous
//! ones under `.rossi/build/<project>/`, so obligations keep their stamps
//! and statuses across edits whether or not a caller asks to write them.

pub mod report;
pub mod server;
pub mod workspace;

pub use server::RossiServer;

use std::path::PathBuf;

use rmcp::ServiceExt;

/// Serves the project under `root` over standard input and output until
/// the client disconnects. Logging goes to standard error; nothing else
/// may write to standard output while the server runs.
pub async fn serve_stdio(root: PathBuf) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let running = RossiServer::new(root)
        .serve(rmcp::transport::stdio())
        .await?;
    running.waiting().await?;
    Ok(())
}
