use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod commands {
    pub mod build;
    pub mod eventb_io;
    pub mod export;
    pub mod fmt;
    pub mod gen_grammars;
    pub mod grammars;
    pub mod import;
    pub mod sarif;
    pub mod validate;
}

#[derive(Parser)]
#[command(
    name = "rossi",
    version,
    propagate_version = true,
    about = "Rossi command-line tools for Event-B models"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Validate Event-B text files or Rodin ZIP archives.
    #[command(about = "Validate Event-B model files")]
    Validate(commands::validate::ValidateArgs),
    /// Import Rodin archives (.zip/.buc/.bum/dir) into Event-B text.
    #[command(about = "Import Rodin archives into Event-B text")]
    Import(commands::import::ImportArgs),
    /// Export Event-B text (.eventb/.txt/dir) into a Rodin .zip archive.
    #[command(about = "Export Event-B text into a Rodin .zip archive")]
    Export(commands::export::ExportArgs),
    /// Reformat Event-B text/archives in place (operator convention, indentation).
    #[command(about = "Reformat Event-B text/archives in place")]
    Fmt(commands::fmt::FmtArgs),
    /// Static-check a Rodin project and emit `.bcc` / `.bcm` output.
    #[command(about = "Static-check a Rodin project and emit .bcc/.bcm output")]
    Build(commands::build::BuildArgs),
    /// Regenerate editor syntax grammars from the canonical token tables.
    #[command(about = "Regenerate editor syntax grammars from the canonical token tables")]
    GenGrammars(commands::gen_grammars::GenGrammarsArgs),
    /// Run the Rossi language server over stdio.
    #[command(about = "Run the Rossi language server over stdio")]
    Lsp,
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Validate(args) => commands::validate::run(args),
        Command::Import(args) => commands::import::run(args),
        Command::Export(args) => commands::export::run(args),
        Command::Fmt(args) => commands::fmt::run(args),
        Command::Build(args) => commands::build::run_build_command(args),
        Command::GenGrammars(args) => commands::gen_grammars::run(args),
        // The LSP brings its own runtime (with sized handler stacks); the
        // other commands are fully synchronous.
        Command::Lsp => match eventb_lsp::run_stdio_blocking() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("rossi lsp: {e}");
                ExitCode::from(1)
            }
        },
    }
}
