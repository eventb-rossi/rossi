//! `rossi export` — convert Event-B text into a Rodin project.
//!
//! Reads Event-B text (`.eventb`/`.txt` files or directories of them) and packs
//! the parsed components into a complete Rodin project: a `.project` descriptor
//! (named after the output path) plus each component's native Rodin XML. The
//! output is written as a `.zip` archive when the output path ends in `.zip`,
//! and as a loose project directory otherwise. The archive's XML always uses
//! Unicode operators, which is what Rodin expects, so there is no
//! operator-convention option here — see `rossi fmt` for that.
//!
//! When the sole input is a directory whose Event-B files live entirely under
//! immediate subdirectories (and none directly in it), each such subdirectory
//! is exported as its own Rodin project under a `<name>/` prefix — the inverse
//! of a multi-project `rossi import`, so a decomposition round-trips. Any other
//! shape (files in the directory itself, several inputs, a single file, stdin)
//! is exported as one flat project named after the output path.

use clap::Args;
use rossi::{
    NamedComponent, NamedProject, to_multi_project_zip, write_multi_project_directory,
    write_project_directory, write_project_zip_file,
};
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
    if eventb_io::stdin_is_sole_input(&cli.inputs)? {
        let source = eventb_io::read_stdin_to_string()?;
        let components = eventb_io::parse_text_components("<stdin>", &source)?;
        return write_flat_project(cli, &components);
    }

    for input in &cli.inputs {
        eventb_io::ensure_input(input, InputFamily::Text)?;
    }

    // A single directory whose Event-B text lives only under immediate
    // subdirectories exports as one Rodin project per subdirectory (the inverse
    // of a multi-project import). Any other shape falls through to one flat
    // project below.
    if let [only] = cli.inputs.as_slice()
        && only.is_dir()
        && let Some(projects) = discover_text_projects(only, cli.verbose)?
    {
        return write_multi_projects(cli, &projects);
    }

    let eventb_files = eventb_io::collect_eventb_files(&cli.inputs)?;
    if eventb_files.is_empty() {
        return Err("No .eventb or .txt files found in inputs".into());
    }
    let components = parse_eventb_files(&eventb_files, cli.verbose)?;
    write_flat_project(cli, &components)
}

/// Split a directory into one project per immediate subdirectory.
///
/// Returns `Some(projects)` only when the directory holds **no** Event-B text
/// of its own and at least one immediate subdirectory does (recursively); each
/// such subdirectory becomes a project named after it. Returns `None` for every
/// other shape — files directly in `dir`, or no subdirectory with Event-B text —
/// so the caller exports a single flat project instead. This keeps the
/// multi-project trigger unambiguous (it never emits a root `.project` beside
/// sub-project ones) and exactly inverts multi-project import output.
fn discover_text_projects(dir: &Path, verbose: bool) -> CmdResult<Option<Vec<NamedProject>>> {
    let mut subdirs = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_file() {
            // A definite Event-B source (`.eventb`) directly under `dir` ⇒ flat
            // single project. A generic `.txt` (README/LICENSE/notes) does not
            // disqualify the split — matching the "a README.txt is not a
            // component" convention used elsewhere.
            if entry
                .path()
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(eventb_io::is_eventb_ext)
            {
                return Ok(None);
            }
        } else if file_type.is_dir() {
            subdirs.push(entry.path());
        }
    }

    subdirs.sort();
    let mut projects = Vec::new();
    for subdir in subdirs {
        let files = eventb_io::collect_eventb_files(std::slice::from_ref(&subdir))?;
        // A subdirectory with no Event-B text (docs, proofs, …) is not a project.
        if files.is_empty() {
            continue;
        }
        let name = subdir
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("invalid project directory name: {}", subdir.display()))?
            .to_string();
        let components = parse_eventb_files(&files, verbose)?;
        projects.push(NamedProject { name, components });
    }

    Ok((!projects.is_empty()).then_some(projects))
}

/// Parse each `.eventb`/`.txt` file into its components, flattened in order.
fn parse_eventb_files(files: &[PathBuf], verbose: bool) -> CmdResult<Vec<NamedComponent>> {
    let mut components = Vec::new();
    for path in files {
        if verbose {
            eprintln!("Parsing: {}", path.display());
        }
        let source = fs::read_to_string(path)?;
        components.extend(eventb_io::parse_text_components(
            &path.display().to_string(),
            &source,
        )?);
    }
    Ok(components)
}

/// Write `components` as one flat Rodin project named after the output path.
fn write_flat_project(cli: &ExportArgs, components: &[NamedComponent]) -> CmdResult<()> {
    let project_name = project_name_from_output(&cli.output);
    if is_zip_output(&cli.output) {
        eventb_io::ensure_parent_dir(&cli.output)?;
        write_project_zip_file(&cli.output, components, project_name)?;
    } else {
        write_project_directory(&cli.output, components, project_name)?;
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

/// Write each [`NamedProject`] under its own `<name>/` directory in the output.
fn write_multi_projects(cli: &ExportArgs, projects: &[NamedProject]) -> CmdResult<()> {
    if is_zip_output(&cli.output) {
        eventb_io::ensure_parent_dir(&cli.output)?;
        let bytes = to_multi_project_zip(projects)?;
        fs::write(&cli.output, bytes)?;
    } else {
        write_multi_project_directory(&cli.output, projects)?;
    }

    if cli.verbose {
        let total: usize = projects.iter().map(|p| p.components.len()).sum();
        eprintln!(
            "Wrote {} component(s) across {} project(s) to {}",
            total,
            projects.len(),
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
