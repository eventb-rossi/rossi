//! `rossi-build` — static-check Rodin Event-B projects and emit `.bcc` / `.bcm`.
//!
//! Process one project (a `.zip` archive or a directory of `.buc` / `.bum`
//! files). Writes either a repackaged `.zip` (when `<out>` ends in `.zip`) or
//! loose files into a directory.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;

use rossi_build::project::discover_projects;
use rossi_build::repack::repackage_zip_bytes_multi;
use rossi_build::{BuildResult, Project, Severity, build};

use rossi::{NamedComponent, to_zip};

use super::eventb_io::{self, InputKind};

#[derive(Args)]
pub struct BuildArgs {
    /// Input to check: a Rodin `.zip`, a directory (a Rodin project, or a
    /// folder of `.eventb`/`.txt`), or an Event-B text / `.buc` / `.bum` file.
    pub input: PathBuf,
    /// Output path. If it ends in `.zip`, writes a repackaged archive
    /// (sources + our generated `.bcc`/`.bcm`, proof artifacts dropped).
    /// Otherwise, treated as a directory and loose files are written in.
    /// Defaults to `<input-stem>.regen.zip` next to the input.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

pub fn run_build_command(args: BuildArgs) -> ExitCode {
    match run_build(&args.input, args.output.as_deref()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rossi build: {e}");
            ExitCode::from(1)
        }
    }
}

fn run_build(input: &Path, output: Option<&Path>) -> Result<(), Box<dyn std::error::Error>> {
    let outcome = build_one(input)?;
    let default_out;
    let out_path = match output {
        Some(p) => p,
        None => {
            let stem = input
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("project");
            default_out = input
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(format!("{stem}.regen.zip"));
            &default_out
        }
    };

    write_output(input, out_path, &outcome)?;
    report_diagnostics(&outcome);

    let errors = outcome
        .results
        .iter()
        .flat_map(|(_, r)| &r.diagnostics)
        .filter(|d| d.severity == Severity::Error)
        .count();
    let files: usize = outcome.results.iter().map(|(_, r)| r.files.len()).sum();
    eprintln!(
        "rossi build: wrote {} -> {} ({} file(s) across {} project(s), {} error diagnostic(s))",
        input.display(),
        out_path.display(),
        files,
        outcome.results.len(),
        errors
    );
    Ok(())
}

struct BuildOutcome {
    /// One entry per project: (archive prefix, BuildResult). Length 1 for
    /// directory / text / single-file inputs; one per top-level project for a
    /// multi-project `.zip`.
    results: Vec<(String, BuildResult)>,
    /// Original archive bytes when the input was (or was serialized to) a
    /// `.zip` — needed to repackage. `None` for a Rodin project directory.
    archive_bytes: Option<Vec<u8>>,
}

fn build_one(input: &Path) -> Result<BuildOutcome, Box<dyn std::error::Error>> {
    if input.is_dir() {
        // A Rodin project directory always carries `.buc`/`.bum` component
        // files; prefer that path so a real project is never misread as a
        // loose folder of Event-B text.
        if !eventb_io::collect_rodin_xml_files(&[input.to_path_buf()])?.is_empty() {
            let project = Project::from_directory(input)?;
            let result = build(&project);
            // A Rodin project directory is a single project with no source
            // archive to repack against; loose-file output is written flat.
            return Ok(BuildOutcome {
                results: vec![(String::new(), result)],
                archive_bytes: None,
            });
        }
        let text_files = eventb_io::collect_eventb_files(&[input.to_path_buf()])?;
        if text_files.is_empty() {
            return Err(format!("no Event-B files found in {}", input.display()).into());
        }
        return build_from_text_files(&dir_project_name(input), &text_files);
    }

    match eventb_io::classify_file(input)? {
        InputKind::Text => build_from_text_files(&file_project_name(input), &[input.to_path_buf()]),
        InputKind::RodinXml => build_from_components(
            &file_project_name(input),
            vec![eventb_io::parse_rodin_xml_file(input)?],
        ),
        InputKind::RodinZip => build_from_zip(input),
    }
}

/// Build a project from `.eventb`/`.txt` files: parse each, then hand the
/// components to [`build_from_components`].
fn build_from_text_files(
    name: &str,
    files: &[PathBuf],
) -> Result<BuildOutcome, Box<dyn std::error::Error>> {
    let mut components = Vec::new();
    for path in files {
        let source = std::fs::read_to_string(path)?;
        components.extend(eventb_io::parse_text_components(
            &path.display().to_string(),
            &source,
        )?);
    }
    build_from_components(name, components)
}

/// Build a project from parsed components. Serialise them to a Rodin source
/// archive first, then reuse the `.zip` pipeline so the output carries both the
/// sources and our generated `.bcc`/`.bcm` (matching the old export+build path).
fn build_from_components(
    name: &str,
    components: Vec<NamedComponent>,
) -> Result<BuildOutcome, Box<dyn std::error::Error>> {
    if components.is_empty() {
        return Err("no Event-B components to build".into());
    }
    let bytes = to_zip(&components)?;
    build_zip_bytes(name, bytes)
}

/// Build a project from a Rodin `.zip` archive on disk.
fn build_from_zip(input: &Path) -> Result<BuildOutcome, Box<dyn std::error::Error>> {
    let bytes = std::fs::read(input)?;
    build_zip_bytes(&file_project_name(input), bytes)
}

/// Discover every project bundled in `bytes`, build each independently, and
/// return one `(prefix, BuildResult)` per project. A Rodin `.zip` may hold
/// several top-level projects; each is checked under its own name so handle
/// URIs stay byte-exact and sibling components never collide. `fallback_name`
/// names a flat archive that carries neither checked files nor a `.project`.
fn build_zip_bytes(
    fallback_name: &str,
    bytes: Vec<u8>,
) -> Result<BuildOutcome, Box<dyn std::error::Error>> {
    let projects = discover_projects(&bytes, fallback_name)?;
    // No project (no `.buc`/`.bum`, no `.project`) would otherwise repackage to
    // a zip stripped of its checked/proof files with nothing regenerated — a
    // silently destructive "success". Fail loudly instead.
    if projects.is_empty() {
        return Err("no Event-B projects found in archive".into());
    }
    let results = projects
        .into_iter()
        .map(|dp| {
            let prefix = dp.prefix.clone();
            (prefix, build(&dp.into_project()))
        })
        .collect();
    Ok(BuildOutcome {
        results,
        archive_bytes: Some(bytes),
    })
}

/// Project name for a single-file input (its file stem).
fn file_project_name(input: &Path) -> String {
    input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("project")
        .to_string()
}

/// Project name for a directory input (its final path component).
fn dir_project_name(input: &Path) -> String {
    input
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project")
        .to_string()
}

fn write_output(
    input: &Path,
    out_path: &Path,
    outcome: &BuildOutcome,
) -> Result<(), Box<dyn std::error::Error>> {
    let is_zip_out = out_path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    if is_zip_out {
        write_zip(input, out_path, outcome)
    } else {
        write_dir(out_path, outcome)
    }
}

fn write_zip(
    input: &Path,
    out_path: &Path,
    outcome: &BuildOutcome,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = out_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = match &outcome.archive_bytes {
        // Each project's checked files are dropped under its own prefix.
        Some(b) => repackage_zip_bytes_multi(
            b,
            outcome
                .results
                .iter()
                .map(|(prefix, result)| (prefix.as_str(), result)),
        )?,
        // Directory input → no source archive to repack, so just emit our
        // checked files into a fresh flat archive (always a single project).
        None => {
            let empty = BuildResult {
                files: vec![],
                diagnostics: vec![],
            };
            let result = outcome.results.first().map_or(&empty, |(_, r)| r);
            synthesize_flat_zip(input, result)?
        }
    };
    std::fs::write(out_path, bytes)?;
    Ok(())
}

fn write_dir(out_dir: &Path, outcome: &BuildOutcome) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(out_dir)?;
    // A single project writes its files flat into `out_dir` (unchanged loose
    // output); a multi-project archive writes each under its own subdirectory
    // so colliding component filenames across projects don't overwrite.
    let multi = outcome.results.len() > 1;
    for (prefix, result) in &outcome.results {
        let base = if multi {
            let dir = out_dir.join(prefix.trim_end_matches('/'));
            std::fs::create_dir_all(&dir)?;
            dir
        } else {
            out_dir.to_path_buf()
        };
        for f in &result.files {
            std::fs::write(base.join(&f.filename), &f.contents)?;
        }
    }
    Ok(())
}

/// Emit a flat zip from `BuildResult` alone (no source archive to merge with).
fn synthesize_flat_zip(
    input: &Path,
    result: &BuildResult,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    use zip::write::{SimpleFileOptions, ZipWriter};

    let prefix = input
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| format!("{s}/"))
        .unwrap_or_default();

    let mut cursor = std::io::Cursor::new(Vec::<u8>::new());
    let mut w = ZipWriter::new(&mut cursor);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for f in &result.files {
        w.start_file(format!("{prefix}{}", f.filename), opts)?;
        use std::io::Write;
        w.write_all(f.contents.as_bytes())?;
    }
    w.finish()?;
    Ok(cursor.into_inner())
}

fn report_diagnostics(outcome: &BuildOutcome) {
    // A diagnostic's Display carries only the bare component name, so in a
    // multi-project archive (where sibling projects can share component names)
    // print a per-project header to disambiguate which project each came from.
    let multi = outcome.results.len() > 1;
    for (prefix, result) in &outcome.results {
        if multi && !result.diagnostics.is_empty() {
            let label = if prefix.is_empty() {
                "(root)"
            } else {
                prefix.trim_end_matches('/')
            };
            eprintln!("--- {label} ---");
        }
        for d in &result.diagnostics {
            eprintln!("{d}");
        }
    }
}
