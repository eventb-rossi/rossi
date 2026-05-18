//! `rossi-build` — static-check Rodin Event-B projects and emit `.bcc` / `.bcm`.
//!
//! Two modes:
//!
//! * `build <input> [-o <out>]` — process one project (a `.zip` archive or a
//!   directory of `.buc` / `.bum` files). Writes either a repackaged `.zip`
//!   (when `<out>` ends in `.zip`) or loose files into a directory.
//! * `regen-corpus <models-dir> -o <out-dir>` — bulk: walk every `.zip` in
//!   `<models-dir>`, regenerate, and dump a TSV summary on stdout.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;

use rossi_build::project::{
    infer_project_name_from_archive_bytes, infer_project_name_from_checked_xml,
};
use rossi_build::repack::repackage_zip_bytes;
use rossi_build::{BuildResult, Project, Severity, build};

#[derive(Args)]
pub struct BuildArgs {
    /// Input `.zip` or directory.
    pub input: PathBuf,
    /// Output path. If it ends in `.zip`, writes a repackaged archive
    /// (sources + our generated `.bcc`/`.bcm`, proof artifacts dropped).
    /// Otherwise, treated as a directory and loose files are written in.
    /// Defaults to `<input-stem>.regen.zip` next to the input.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

#[derive(Args)]
pub struct RegenCorpusArgs {
    /// Directory containing `.zip` archives.
    pub models_dir: PathBuf,
    /// Output directory for regenerated archives (created if missing).
    #[arg(short, long)]
    pub output: PathBuf,
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

pub fn run_regen_corpus_command(args: RegenCorpusArgs) -> ExitCode {
    match run_regen_corpus(&args.models_dir, &args.output) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(failed) => {
            eprintln!("rossi regen-corpus: {failed} archive(s) failed");
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("rossi regen-corpus: {e}");
            ExitCode::from(2)
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

fn run_regen_corpus(
    models_dir: &Path,
    out_dir: &Path,
) -> Result<usize, Box<dyn std::error::Error>> {
    if !models_dir.is_dir() {
        return Err(format!("not a directory: {}", models_dir.display()).into());
    }
    std::fs::create_dir_all(out_dir)?;

    let mut zips: Vec<PathBuf> = std::fs::read_dir(models_dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("zip"))
        .collect();
    zips.sort();

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out, "model\tfiles\terrors\tstatus")?;

    let mut failed = 0usize;
    for zip in &zips {
        let model = zip.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
        let out_zip = out_dir.join(format!("{model}.zip"));
        match build_one(zip).and_then(|o| {
            write_zip(zip, &out_zip, &o)?;
            Ok(o)
        }) {
            Ok(o) => {
                let errs = o
                    .result
                    .diagnostics
                    .iter()
                    .filter(|d| d.severity == Severity::Error)
                    .count();
                writeln!(out, "{model}\t{}\t{errs}\tok", o.result.files.len())?;
            }
            Err(e) => {
                failed += 1;
                writeln!(out, "{model}\t0\t0\tfail: {e}")?;
            }
        }
    }
    Ok(failed)
}

struct BuildOutcome {
    result: BuildResult,
    /// Original archive bytes when the input was a `.zip` (needed for repack).
    archive_bytes: Option<Vec<u8>>,
}

fn build_one(input: &Path) -> Result<BuildOutcome, Box<dyn std::error::Error>> {
    if input.is_dir() {
        let project = Project::from_directory(input)?;
        let result = build(&project);
        Ok(BuildOutcome {
            result,
            archive_bytes: None,
        })
    } else {
        let bytes = std::fs::read(input)?;
        // Use Rodin's project name when the archive carries one.
        let name = infer_project_name_from_archive_bytes(&bytes).unwrap_or_else(|| {
            input
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("project")
                .to_string()
        });
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
