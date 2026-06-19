//! Event-B Language Server
//!
//! This binary provides LSP (Language Server Protocol) support for the Event-B
//! formal modeling language.

use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(
    name = "eventb-language-server",
    version,
    about = "Event-B language server (LSP) over stdio"
)]
struct Args;

fn main() -> Result<()> {
    Args::parse();
    eventb_lsp::run_stdio_blocking()
}
