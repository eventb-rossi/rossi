use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod commands {
    pub mod build;
    pub mod print;
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
    /// Convert between Rodin ZIP archives and Event-B text files.
    #[command(about = "Convert between Rodin ZIP archives and Event-B text files")]
    Print(commands::print::PrintArgs),
    /// Static-check a Rodin project and emit `.bcc` / `.bcm` output.
    #[command(about = "Static-check a Rodin project and emit .bcc/.bcm output")]
    Build(commands::build::BuildArgs),
    /// Run the Rossi language server over stdio.
    #[command(about = "Run the Rossi language server over stdio")]
    Lsp,
}

#[tokio::main]
async fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Validate(args) => commands::validate::run(args),
        Command::Print(args) => match commands::print::run(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("rossi print: {e}");
                ExitCode::from(1)
            }
        },
        Command::Build(args) => commands::build::run_build_command(args),
        Command::Lsp => match rossi_lsp::run_stdio().await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("rossi lsp: {e}");
                ExitCode::from(1)
            }
        },
    }
}
