//! `cargo xtask` — workspace maintenance tasks for the Rossi repository.
//!
//! These are source-tree tools, run by maintainers and CI rather than by users
//! of the shipped binaries. The current task, `gen-grammars`, regenerates the
//! editor syntax-highlighting grammars from the canonical token tables; it
//! writes the `editors/` tree relative to the workspace root, so it only makes
//! sense from a checkout — which is exactly why it lives here and not in the
//! published `rossi` CLI.
//!
//! Run via the workspace alias: `cargo xtask gen-grammars [--check] [-v]`.

use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod gen_grammars;
mod grammars;

#[derive(Parser)]
#[command(name = "xtask", about = "Rossi workspace maintenance tasks")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Regenerate editor syntax grammars from the canonical token tables.
    #[command(about = "Regenerate editor syntax grammars from the canonical token tables")]
    GenGrammars(gen_grammars::GenGrammarsArgs),
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::GenGrammars(args) => gen_grammars::run(args),
    }
}
