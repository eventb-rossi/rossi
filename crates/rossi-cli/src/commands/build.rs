//! `rossi-build` — static-check Rodin Event-B projects and emit `.bcc` / `.bcm`.
//!
//! Process one project (a `.zip` archive or a directory of `.buc` / `.bum`
//! files). Writes either a repackaged `.zip` (when `<out>` ends in `.zip`) or
//! loose files into a directory.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;

use rossi_build::project::{
    infer_project_name_from_archive_bytes, infer_project_name_from_checked_xml,
};
use rossi_build::repack::repackage_zip_bytes;
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
    report_diagnostics(&outcome.result);

    let errors = outcome
        .result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    eprintln!(
        "rossi build: wrote {} -> {} ({} file(s), {} error diagnostic(s))",
        input.display(),
        out_path.display(),
        outcome.result.files.len(),
        errors
    );
    Ok(())
}

struct BuildOutcome {
    result: BuildResult,
    /// Original archive bytes when the input was a `.zip` (needed for repack).
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
            return Ok(BuildOutcome {
                result,
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
    let project = Project::from_zip_bytes(name, &bytes)?;
    let result = build(&project);
    Ok(BuildOutcome {
        result,
        archive_bytes: Some(bytes),
    })
}

/// Build a project from a Rodin `.zip` archive on disk.
fn build_from_zip(input: &Path) -> Result<BuildOutcome, Box<dyn std::error::Error>> {
    let bytes = std::fs::read(input)?;
    // Use Rodin's project name when the archive carries one.
    let name =
        infer_project_name_from_archive_bytes(&bytes).unwrap_or_else(|| file_project_name(input));
    let project = Project::from_zip_bytes(&name, &bytes)?;
    let result = build(&project);
    // Sanity check: also confirm the inferred name against our own
    // generated bcc/bcm (so users see a hint if names diverge).
    if let Some(f) = result.files.first()
        && let Some(rodin) = infer_project_name_from_checked_xml(&f.contents)
        && rodin != name
    {
        eprintln!(
            "rossi build: warning: emitted project name {name:?} differs from \
             internal handle {rodin:?}"
        );
    }
    Ok(BuildOutcome {
        result,
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
        write_dir(out_path, &outcome.result)
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
        Some(b) => repackage_zip_bytes(b, &outcome.result)?,
        // Directory input → no source archive to repack, so just emit
        // our checked files into a fresh flat archive.
        None => synthesize_flat_zip(input, &outcome.result)?,
    };
    std::fs::write(out_path, bytes)?;
    Ok(())
}

fn write_dir(out_dir: &Path, result: &BuildResult) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(out_dir)?;
    for f in &result.files {
        let p = out_dir.join(&f.filename);
        std::fs::write(&p, &f.contents)?;
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

fn report_diagnostics(result: &BuildResult) {
    for d in &result.diagnostics {
        eprintln!("{d}");
    }
}
