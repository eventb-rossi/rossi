//! `rossi export` — convert Event-B text into a Rodin project.
//!
//! Reads Event-B text (`.eventb`/`.txt` files or directories of them) and packs
//! the parsed components into a complete Rodin project: a `.project` descriptor
//! (named after the output path) plus each component's native Rodin XML. The
//! output is written as a `.zip` archive when the output path ends in `.zip`,
//! and as a loose project directory otherwise. The archive's XML always uses
//! Unicode operators, which is what Rodin expects, so there is no
//! operator-convention option here — see `rossi fmt` for that.

use clap::Args;
use rossi::{write_project_directory, write_project_zip_file};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use super::eventb_io::{self, CmdResult, InputFamily};

#[derive(Args)]
pub struct ExportArgs {
    /// Event-B text inputs (.eventb, .txt) or directories containing them;
    /// `-` reads Event-B text from stdin
    #[arg(required = true, value_name = "INPUT")]
    inputs: Vec<PathBuf>,

    /// Output Rodin project: a .zip archive (path ends in .zip) or a directory
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

    let project_name = project_name_from_output(&cli.output);
    if is_zip_output(&cli.output) {
        if let Some(parent) = cli.output.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        write_project_zip_file(&cli.output, &components, project_name)?;
    } else {
        write_project_directory(&cli.output, &components, project_name)?;
    }

    if cli.verbose {
        eprintln!(
            "Wrote {} component(s) to {}",
            components.len(),
            cli.output.display()
        );
    }

    Ok(())
}

/// Whether the output path denotes a `.zip` archive (vs. a project directory).
fn is_zip_output(output: &Path) -> bool {
    output
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(eventb_io::is_zip_ext)
}

/// The Rodin project name to embed, taken from the output path's file stem.
/// A missing or blank stem is normalized to a default by the project writer.
fn project_name_from_output(output: &Path) -> &str {
    output
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
}
