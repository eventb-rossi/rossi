use std::io::{self, Write};
use std::process::ExitCode;

use clap::Args;
use clap_complete::Shell;

#[derive(Args)]
pub struct CompletionsArgs {
    /// Shell to generate the completion script for.
    #[arg(value_enum, value_name = "SHELL")]
    shell: Shell,
}

/// Write the completion script for `cmd` (the `rossi` clap command tree) to
/// stdout, derived from the same clap definition the CLI is parsed with — so
/// the completions can never drift from the actual command-line interface.
///
/// Redirect the output to wherever the chosen shell loads completions, e.g.
/// `rossi completions zsh > _rossi`, or source it directly with
/// `eval "$(rossi completions bash)"`.
pub fn run(args: CompletionsArgs, cmd: &mut clap::Command) -> ExitCode {
    let bin_name = cmd.get_name().to_string();
    // `clap_complete::generate` panics on a write error, so render into an
    // in-memory buffer (writes to a `Vec` cannot fail) and flush that to stdout
    // ourselves, where a write error can be reported instead of aborting.
    let mut script = Vec::new();
    clap_complete::generate(args.shell, cmd, bin_name, &mut script);
    match io::stdout().write_all(&script) {
        Ok(()) => ExitCode::SUCCESS,
        // A consumer closing the pipe early (e.g. `rossi completions bash | head`)
        // is benign — exit quietly rather than panicking on EPIPE.
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rossi completions: {e}");
            ExitCode::from(1)
        }
    }
}
