//! `rossi-build` — static-check Rodin Event-B projects and emit `.bcc` /
//! `.bcm` plus the generated `.bpo` / `.bps` proof-obligation files.
//!
//! Process one project (a `.zip` archive or a directory of `.buc` / `.bum`
//! files). Writes either a repackaged `.zip` (when `<out>` ends in `.zip`) or
//! loose files into a directory.
//!
//! Any error diagnostic makes the build exit nonzero. Matching Rodin's static
//! checker, the filtered output is still written first: erroneous elements are
//! dropped and their files marked inaccurate.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;

use rossi_build::pog::reconcile::reconcile_build_files;
use rossi_build::pog::status::update_statuses;
use rossi_build::project::{duplicate_component_name, project_from_text_components};
use rossi_build::{BuildResult, Project, ScFile, build, is_normal_path_component};

use rossi::NamedComponent;

use super::build_common::{
    build_archive_projects, eb019_result, error_diagnostic_count, gate_after_write,
    gate_before_write, repack_results, report_diagnostics,
};
use super::eventb_io::{self, InputKind};

#[derive(Args)]
pub struct BuildArgs {
    /// Input to check: a Rodin `.zip`, a directory (a Rodin project, or a
    /// folder of `.eventb`/`.txt`), or an Event-B text / `.buc` / `.bum` file.
    pub input: PathBuf,
    /// Output path. If it ends in `.zip`, writes a repackaged archive
    /// (sources and `.bpr` proofs plus our generated `.bcc`/`.bcm` and
    /// `.bpo`/`.bps`, reconciled with the input's so unchanged
    /// obligations keep their stamps and statuses). Otherwise, treated
    /// as a directory and loose files are written in.
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

    let failed = gate_before_write(&outcome.results)?;

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
    report_diagnostics(&outcome.results);

    let errors = error_diagnostic_count(&outcome.results);
    let files: usize = outcome.results.iter().map(|(_, r)| r.files.len()).sum();
    eprintln!(
        "rossi build: wrote {} -> {} ({} file(s) across {} project(s), {} error diagnostic(s))",
        input.display(),
        out_path.display(),
        files,
        outcome.results.len(),
        errors
    );
    gate_after_write(&outcome.results, &failed, "checked output")
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
        // loose folder of Event-B text. Ask the loader itself what it would
        // pick up, rather than re-deciding: a recursive, case-insensitive
        // gate used to admit directories holding only `M.BUM`, or only
        // `sub/M.bum`, that `Project::from_directory` then read as zero
        // components — an empty project, built in silence.
        let components = rossi_build::project::component_files(input)?;
        let is_rodin_project = components
            .iter()
            .any(|path| rossi_build::project::is_xml_component(path));
        if is_rodin_project {
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
        return build_from_text_files(&eventb_io::dir_project_name(input), &text_files);
    }

    match eventb_io::classify_file(input)? {
        InputKind::Text => {
            build_from_text_files(&eventb_io::file_project_name(input), &[input.to_path_buf()])
        }
        InputKind::RodinXml => build_from_components(
            &eventb_io::file_project_name(input),
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
    // Duplicate component names cannot be serialised into a Rodin source
    // archive (a component's name is its entry filename), and the project
    // is invalid regardless. Assemble the project directly so the failure
    // surfaces as the SC's EB019 diagnostic instead of a zip-writer error.
    if duplicate_component_name(&components).is_some() {
        return Ok(BuildOutcome {
            results: vec![(String::new(), eb019_result(name, components))],
            archive_bytes: None,
        });
    }
    let (bytes, project) = project_from_text_components(name, &components)?;
    Ok(BuildOutcome {
        results: vec![(String::new(), build(&project))],
        archive_bytes: Some(bytes),
    })
}

/// Build a project from a Rodin `.zip` archive on disk.
fn build_from_zip(input: &Path) -> Result<BuildOutcome, Box<dyn std::error::Error>> {
    let bytes = std::fs::read(input)?;
    build_zip_bytes(&eventb_io::file_project_name(input), bytes)
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
    let results = build_archive_projects(&bytes, fallback_name)?;
    // No project (no `.buc`/`.bum`, no `.project`) would otherwise repackage to
    // a zip stripped of its checked/proof files with nothing regenerated — a
    // silently destructive "success". Fail loudly instead.
    if results.is_empty() {
        return Err("no Event-B projects found in archive".into());
    }
    Ok(BuildOutcome {
        results,
        archive_bytes: Some(bytes),
    })
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
        Some(b) => repack_results(b, &outcome.results)?,
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
    // A single project writes its files flat into `out_dir` (unchanged loose
    // output); a multi-project archive writes each under its own subdirectory
    // so colliding component filenames across projects don't overwrite.
    let multi = outcome.results.len() > 1;
    let mut pending = Vec::new();
    let mut relative_paths = std::collections::BTreeSet::new();
    for (prefix, result) in &outcome.results {
        let project_dir = loose_project_dir(prefix)?;
        for f in &result.files {
            if !is_normal_path_component(&f.filename) {
                return Err(format!(
                    "unsafe generated filename {:?}; loose output filenames must be one normal path component",
                    f.filename
                )
                .into());
            }
            let relative = if multi {
                project_dir
                    .as_ref()
                    .map_or_else(|| PathBuf::from(&f.filename), |dir| dir.join(&f.filename))
            } else {
                PathBuf::from(&f.filename)
            };
            if !relative_paths.insert(relative.clone()) {
                return Err(
                    format!("duplicate loose output destination {}", relative.display()).into(),
                );
            }
            pending.push((relative, f.contents.clone()));
        }
    }

    std::fs::create_dir_all(out_dir)?;
    let canonical_root = std::fs::canonicalize(out_dir)?;
    if !std::fs::metadata(&canonical_root)?.is_dir() {
        return Err(format!("output path is not a directory: {}", out_dir.display()).into());
    }
    let parents: std::collections::BTreeSet<PathBuf> = pending
        .iter()
        .filter_map(|(relative, _)| out_dir.join(relative).parent().map(Path::to_path_buf))
        .collect();

    // Check existing paths before creating any project directories, so an
    // escaping symlink prevents even safe sibling directories being created.
    visit_resolved_output_paths(out_dir, &canonical_root, &parents, &pending, |_| {})?;
    for parent in &parents {
        std::fs::create_dir_all(parent)?;
    }

    // Creating directories can reveal aliases on case-insensitive filesystems;
    // resolve and de-duplicate again before the first checked file is written.
    let mut destinations = Vec::with_capacity(pending.len());
    visit_resolved_output_paths(out_dir, &canonical_root, &parents, &pending, |path| {
        destinations.push(path)
    })?;
    reconcile_pending(&mut pending, &destinations);
    for ((_, contents), destination) in pending.iter().zip(destinations) {
        std::fs::write(destination, contents)?;
    }
    Ok(())
}

/// Reconcile each pending `.bpo` / `.bps` pair against the destination
/// files it is about to overwrite, so proof stamps and statuses carry
/// across rebuilds. Previous state comes from the destination — the
/// files actually being replaced — which for loose output may differ
/// from the input; `.bpr` files on disk are never touched. Unreadable
/// or missing destinations count as absent.
fn reconcile_pending(pending: &mut [(PathBuf, String)], destinations: &[PathBuf]) {
    // Wrap each pending entry as an `ScFile` keyed by its relative path
    // (path-prefixed, so sibling projects stay separate) and let
    // `reconcile_build_files` do the `.bpo` / `.bps` pairing — one
    // implementation of that rule, shared with the archive paths.
    let destination_of: std::collections::HashMap<String, &PathBuf> = pending
        .iter()
        .zip(destinations)
        .map(|((relative, _), destination)| (relative.to_string_lossy().into_owned(), destination))
        .collect();
    let mut files: Vec<ScFile> = pending
        .iter_mut()
        .map(|(relative, contents)| ScFile {
            filename: relative.to_string_lossy().into_owned(),
            contents: std::mem::take(contents),
            accurate: true,
        })
        .collect();
    let synthesized = reconcile_build_files(&mut files, |name| {
        destination_of
            .get(name)
            .and_then(|destination| std::fs::read_to_string(destination).ok())
    });
    // The same status pass the archive repack runs, so a loose-output
    // rebuild and a zip rebuild of the same project agree on the
    // `.bps` bytes. The `.bpr` sits beside the `.bps` it belongs to.
    update_statuses(&mut files, &synthesized, |name| {
        let stem = name.strip_suffix(".bpr")?;
        let sibling = destination_of.get(&format!("{stem}.bps"))?;
        let path = sibling.with_file_name(std::path::Path::new(name).file_name()?);
        std::fs::read(path).ok()
    });
    for ((_, contents), file) in pending.iter_mut().zip(files) {
        *contents = file.contents;
    }
}

/// Convert an archive prefix to the one directory component allowed for loose
/// output. The raw prefix remains untouched for archive repacking.
fn loose_project_dir(prefix: &str) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    if prefix.is_empty() {
        return Ok(None);
    }
    let segment = prefix.strip_suffix('/').unwrap_or_default();
    if !is_normal_path_component(segment) {
        return Err(format!(
            "unsafe archive prefix {prefix:?}; loose output requires an empty prefix or exactly one normal path component"
        )
        .into());
    }
    Ok(Some(PathBuf::from(segment)))
}

/// Resolve every pending destination against the canonical output root.
/// Existing symlinks are allowed only when their targets remain contained.
fn visit_resolved_output_paths(
    out_dir: &Path,
    canonical_root: &Path,
    parents: &std::collections::BTreeSet<PathBuf>,
    pending: &[(PathBuf, String)],
    mut visit: impl FnMut(PathBuf),
) -> Result<(), Box<dyn std::error::Error>> {
    let mut resolved_parents = std::collections::BTreeMap::new();
    for parent in parents {
        resolved_parents.insert(
            parent.clone(),
            resolve_output_parent(out_dir, canonical_root, parent)?,
        );
    }
    let mut resolved_paths = std::collections::BTreeSet::new();
    for (relative, _) in pending {
        let lexical = out_dir.join(relative);
        let parent = lexical
            .parent()
            .ok_or_else(|| format!("output destination has no parent: {}", lexical.display()))?;
        let resolved_parent = resolved_parents
            .get(parent)
            .ok_or_else(|| format!("output destination has no parent: {}", lexical.display()))?;
        if !resolved_parent.starts_with(canonical_root) {
            return Err(format!(
                "output destination {} escapes output directory {}",
                lexical.display(),
                out_dir.display()
            )
            .into());
        }

        let resolved = match std::fs::symlink_metadata(&lexical) {
            Ok(_) => {
                let path = std::fs::canonicalize(&lexical)?;
                if !std::fs::metadata(&path)?.is_file() {
                    return Err(
                        format!("output destination is not a file: {}", lexical.display()).into(),
                    );
                }
                path
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                resolved_parent.join(lexical.file_name().ok_or_else(|| {
                    format!("output destination has no filename: {}", lexical.display())
                })?)
            }
            Err(e) => return Err(e.into()),
        };
        if !resolved.starts_with(canonical_root) {
            return Err(format!(
                "output destination {} escapes output directory {}",
                lexical.display(),
                out_dir.display()
            )
            .into());
        }
        if !resolved_paths.insert(resolved.clone()) {
            return Err(format!("duplicate loose output destination {}", lexical.display()).into());
        }
        visit(resolved);
    }
    Ok(())
}

fn resolve_output_parent(
    out_dir: &Path,
    canonical_root: &Path,
    parent: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    match std::fs::symlink_metadata(parent) {
        Ok(_) => {
            let resolved = std::fs::canonicalize(parent)?;
            if !std::fs::metadata(&resolved)?.is_dir() {
                return Err(
                    format!("output parent is not a directory: {}", parent.display()).into(),
                );
            }
            Ok(resolved)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let relative = parent.strip_prefix(out_dir)?;
            Ok(canonical_root.join(relative))
        }
        Err(e) => Err(e.into()),
    }
}

/// Emit a flat zip from `BuildResult` alone (no source archive to merge
/// with). A project-directory input contributes its previous proof
/// state: generated `.bpo` / `.bps` pairs are reconciled against the
/// directory's files and its top-level `.bpr` proofs are copied in
/// byte-exact.
fn synthesize_flat_zip(
    input: &Path,
    result: &BuildResult,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    use std::io::Write;
    use zip::write::{SimpleFileOptions, ZipWriter};

    let prefix = input
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| format!("{s}/"))
        .unwrap_or_default();

    // A directory input contributes its previous proof state: the
    // generated pairs reconcile against its files, and its top-level
    // `.bpr` proofs ride along byte-exact after the generated entries.
    let mut files = result.files.clone();
    let mut proofs: Vec<PathBuf> = Vec::new();
    if input.is_dir() {
        let synthesized = reconcile_build_files(&mut files, |name| {
            std::fs::read_to_string(input.join(name)).ok()
        });
        // The same status pass the archive repack runs, reading the
        // directory's proofs, so both flows agree on the `.bps` bytes.
        update_statuses(&mut files, &synthesized, |name| {
            std::fs::read(input.join(name)).ok()
        });
        proofs = std::fs::read_dir(input)?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension().and_then(|s| s.to_str()) == Some("bpr") && path.is_file()
            })
            .collect();
        proofs.sort();
    }

    let mut cursor = std::io::Cursor::new(Vec::<u8>::new());
    let mut w = ZipWriter::new(&mut cursor);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for f in &files {
        w.start_file(format!("{prefix}{}", f.filename), opts)?;
        w.write_all(f.contents.as_bytes())?;
    }
    for path in proofs {
        let Some(filename) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        w.start_file(format!("{prefix}{filename}"), opts)?;
        w.write_all(&std::fs::read(&path)?)?;
    }
    w.finish()?;
    Ok(cursor.into_inner())
}
