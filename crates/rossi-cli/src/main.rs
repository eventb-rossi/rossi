use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};

/// The formula parser and the proof checker allocate and free small
/// tree nodes by the hundred million; mimalloc serves that 15-25 %
/// faster than the system allocators measured (glibc, and the parallel
/// case), so the binary opts in. Libraries stay allocator-agnostic;
/// `--no-default-features` keeps the system allocator.
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod commands {
    pub mod build;
    pub mod build_common;
    pub mod clean;
    pub mod completions;
    pub mod dump;
    pub mod eventb_io;
    pub mod export;
    pub mod fmt;
    pub mod import;
    pub mod proofs;
    pub mod prove;
    pub mod report;
    pub mod sarif;
    pub mod style;
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
    /// Write a checked project as a rossi-model JSON document.
    #[command(about = "Write a checked project as a rossi-model JSON document")]
    Dump(commands::dump::DumpArgs),
    /// Drop orphaned proofs and empty broken ones.
    #[command(about = "Clean orphaned and broken proofs from a project")]
    Clean(commands::clean::CleanArgs),
    /// Check the stored proofs of an Event-B project against its
    /// obligations.
    #[command(about = "Check stored proofs against their proof obligations")]
    Prove(commands::prove::ProveArgs),
    /// Generate a shell completion script (bash, zsh, fish, …).
    #[command(about = "Generate a shell completion script")]
    Completions(commands::completions::CompletionsArgs),
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Validate(args) => commands::validate::run(args),
        Command::Import(args) => commands::import::run(args),
        Command::Export(args) => commands::export::run(args),
        Command::Fmt(args) => commands::fmt::run(args),
        Command::Build(args) => commands::build::run_build_command(args),
        Command::Dump(args) => commands::dump::run(args),
        Command::Clean(args) => commands::clean::run(args),
        Command::Prove(args) => commands::prove::run(args),
        // Derive the completion script from the same clap command tree the CLI
        // parses with, so it can never drift from the real interface.
        Command::Completions(args) => commands::completions::run(args, &mut Cli::command()),
    }
}
