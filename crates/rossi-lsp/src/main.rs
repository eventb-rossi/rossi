//! Rossi Language Server
//!
//! This binary provides LSP (Language Server Protocol) support for Event-B
//! formal modeling language.

use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(
    name = "rossi-language-server",
    version,
    about = "Rossi language server for Event-B models"
)]
struct Args;

#[tokio::main]
async fn main() -> Result<()> {
    Args::parse();
    rossi_lsp::run_stdio().await
}
