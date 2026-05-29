//! `rossi export` — convert Event-B text into a Rodin `.zip` archive.
//!
//! Reads Event-B text (`.eventb`/`.txt` files or directories of them) and
//! packs the parsed components into a single Rodin-compatible `.zip`. The
//! archive's XML always uses Unicode operators, which is what Rodin expects,
//! so there is no operator-convention option here — see `rossi fmt` for that.

use clap::Args;
use rossi::write_zip_file;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use super::eventb_io::{self, CmdResult, InputFamily};

#[derive(Args)]
pub struct ExportArgs {
    /// Event-B text inputs (.eventb, .txt) or directories containing them;
    /// `-` reads Event-B text from stdin
    #[arg(required = true, value_name = "INPUT")]
    inputs: Vec<PathBuf>,

    /// Output Rodin .zip archive
    #[arg(short, long, required = true, value_name = "OUTPUT")]
    output: PathBuf,

    /// Show detailed progress
    #[arg(short, long)]
    verbose: bool,
}

pub fn run(cli: ExportArgs) -> ExitCode {
    match run_inner(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rossi export: {e}");
            ExitCode::from(1)
        }
    }
}

fn run_inner(cli: &ExportArgs) -> CmdResult<()> {
    let components = if eventb_io::stdin_is_sole_input(&cli.inputs)? {
        let source = eventb_io::read_stdin_to_string()?;
        eventb_io::parse_text_components("<stdin>", &source)?
    } else {
        for input in &cli.inputs {
            eventb_io::ensure_input(input, InputFamily::Text)?;
        }

        let eventb_files = eventb_io::collect_eventb_files(&cli.inputs)?;
        if eventb_files.is_empty() {
            return Err("No .eventb or .txt files found in inputs".into());
        }

        let mut components = Vec::new();
        for path in &eventb_files {
            if cli.verbose {
                eprintln!("Parsing: {}", path.display());
            }
            let source = fs::read_to_string(path)?;
            components.extend(eventb_io::parse_text_components(
                &path.display().to_string(),
                &source,
            )?);
        }
        components
    };

    if let Some(parent) = cli.output.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    write_zip_file(&cli.output, &components)?;

    if cli.verbose {
        eprintln!(
            "Wrote {} component(s) to {}",
            components.len(),
            cli.output.display()
        );
    }

    Ok(())
}
