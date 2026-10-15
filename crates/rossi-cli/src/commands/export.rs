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
//! With `--build`, the export also runs the static checker and
//! proof-obligation generator, so the output additionally carries the checked
//! `.bcc`/`.bcm` files and the generated `.bpo`/`.bps` pairs — the same
//! pipeline and exit semantics as `rossi build` (error diagnostics still write
//! the output first, then fail the command). With `--proofs[=PATH]` (which
//! implies `--build`) local proof state joins the output: `.bpr` proofs are
//! carried byte-exact, and the generated `.bpo`/`.bps` are reconciled against
//! the local ones so unchanged obligations keep their stamps and recorded
//! statuses. Bare `--proofs` looks next to the text inputs first and then in
//! the LSP's shared Rodin workspace (`<root>/.rossi/rodin/<project>`); a
//! custom `rossi.rodin.workspace` setting lives in editor configuration and
//! is not visible here, so those setups pass the location as `--proofs=PATH`
//! (a Rodin project directory or `.zip`). Proof sources are read-only —
//! nothing outside the output path is modified.
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
use rossi_build::project::{
    duplicate_component_name, project_from_text_components, projects_from_text_components,
};
use rossi_build::{BuildResult, build, is_normal_path_component};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use super::build_common::{
    build_discovered, eb019_result, gate_after_write, gate_before_write, project_label,
    repack_results, report_diagnostics,
};
use super::eventb_io::{self, CmdResult, InputFamily};
use super::proofs::ProofSource;

#[derive(Args)]
pub struct ExportArgs {
    /// Event-B text inputs (.eventb, .txt) or directories containing them;
    /// `-` reads Event-B text from stdin
    #[arg(required = true, value_name = "INPUT")]
    inputs: Vec<PathBuf>,

    /// Output Rodin project: a .zip archive (path ends in .zip) or a directory
    #[arg(short, long, required = true, value_name = "OUTPUT")]
    output: PathBuf,

    /// Also static-check and generate proof obligations, including the
    /// checked `.bcc`/`.bcm` and generated `.bpo`/`.bps` files in the output
    #[arg(long)]
    build: bool,

    /// Attach local proof state (implies --build): `.bpr` byte-exact, and the
    /// generated `.bpo`/`.bps` reconciled against the local ones. PATH is a
    /// Rodin project directory or `.zip`; without `=PATH`, proofs are found
    /// next to the inputs and in the `.rossi/rodin` workspace
    #[arg(long, value_name = "PATH", num_args = 0..=1, require_equals = true)]
    proofs: Option<Option<PathBuf>>,

    /// Show detailed progress
    #[arg(short, long)]
    verbose: bool,
}

/// One project of a building export: its serialized identity plus where its
/// text inputs live (the bare `--proofs` scan locations). Whether the export
/// is flat or multi-project — and so which archive prefix each project gets —
/// is `export_built`'s `multi` argument, not per-plan state.
struct ProjectPlan {
    project: NamedProject,
    local_dirs: Vec<PathBuf>,
}

/// The plan for a flat export: one project named after the output path.
fn flat_plan(
    cli: &ExportArgs,
    components: Vec<NamedComponent>,
    local_dirs: Vec<PathBuf>,
) -> ProjectPlan {
    ProjectPlan {
        project: NamedProject {
            name: project_name_from_output(&cli.output).to_string(),
            components,
        },
        local_dirs,
    }
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
    let building = cli.build || cli.proofs.is_some();

    if eventb_io::stdin_is_sole_input(&cli.inputs)? {
        // A usage error, reported before any of stdin is consumed: there is
        // no input directory the bare form could scan.
        if matches!(cli.proofs, Some(None)) {
            return Err(
                "--proofs without a path needs file or directory inputs; use --proofs=PATH".into(),
            );
        }
        let source = eventb_io::read_stdin_to_string()?;
        let components = eventb_io::parse_text_components("<stdin>", &source)?;
        if building {
            return export_built(cli, vec![flat_plan(cli, components, Vec::new())], false);
        }
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
        if building {
            let plans = projects
                .into_iter()
                .map(|(dir, project)| ProjectPlan {
                    project,
                    local_dirs: vec![dir],
                })
                .collect();
            return export_built(cli, plans, true);
        }
        let projects: Vec<NamedProject> = projects.into_iter().map(|(_, p)| p).collect();
        return write_multi_projects(cli, &projects);
    }

    let eventb_files = eventb_io::collect_eventb_files(&cli.inputs)?;
    if eventb_files.is_empty() {
        return Err("No .eventb or .txt files found in inputs".into());
    }
    let components = parse_eventb_files(&eventb_files, cli.verbose)?;
    if building {
        let plan = flat_plan(cli, components, flat_local_dirs(&cli.inputs));
        return export_built(cli, vec![plan], false);
    }
    write_flat_project(cli, &components)
}

/// The directories a flat export's inputs live in (an input directory itself,
/// or a file's parent), deduplicated in CLI order — the bare `--proofs` scan
/// locations.
fn flat_local_dirs(inputs: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for input in inputs {
        let dir = if input.is_dir() {
            input.clone()
        } else {
            eventb_io::parent_or_cwd(input)
        };
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// Run the build pipeline over the planned projects and write the output.
///
/// The plans go through the same text→project doors as `rossi build` and the
/// LSP ([`project_from_text_components`] / [`projects_from_text_components`]),
/// any local proof files are injected as archive entries, and the result is
/// checked and repackaged exactly as `rossi build` would: the injected
/// `.bpo`/`.bps` become the reconcile baselines and the `.bpr` entries ride
/// through byte-exact. Exit semantics mirror `rossi build` — the output is
/// written first, then error diagnostics fail the command.
fn export_built(cli: &ExportArgs, plans: Vec<ProjectPlan>, multi: bool) -> CmdResult<()> {
    // A sub-project's entries live under its `<name>/` archive prefix; a flat
    // export writes at the root.
    let prefix_of = |plan: &ProjectPlan| {
        if multi {
            format!("{}/", plan.project.name)
        } else {
            String::new()
        }
    };

    // Duplicate component names cannot be serialised into a Rodin source
    // archive (a component's name is its entry filename), and the project is
    // invalid regardless. Surface the failure as the SC's EB019 diagnostic —
    // assembled directly, as `rossi build` does — and write nothing.
    let (duplicated, plans): (Vec<ProjectPlan>, Vec<ProjectPlan>) = plans
        .into_iter()
        .partition(|p| duplicate_component_name(&p.project.components).is_some());
    if !duplicated.is_empty() {
        let results: Vec<(String, BuildResult)> = duplicated
            .into_iter()
            .map(|p| {
                let prefix = prefix_of(&p);
                (prefix, eb019_result(&p.project.name, p.project.components))
            })
            .collect();
        report_diagnostics(&results);
        let labels: Vec<&str> = results.iter().map(|(p, _)| project_label(p)).collect();
        return Err(format!(
            "duplicate component names in project(s) {}; nothing was written",
            labels.join(", ")
        )
        .into());
    }

    let source = match &cli.proofs {
        None => None,
        Some(path) => Some(ProofSource::open(path.as_deref())?),
    };
    let mut proof_entries: Vec<(String, Vec<u8>)> = Vec::new();
    if let Some(source) = &source {
        for plan in &plans {
            let prefix = prefix_of(plan);
            let sub_project = multi.then_some(plan.project.name.as_str());
            let files = source.for_project(sub_project, &plan.local_dirs)?;
            if cli.verbose {
                eprintln!(
                    "Attaching {} proof file(s) for {}",
                    files.len(),
                    project_label(&prefix)
                );
            }
            for (basename, bytes) in files {
                proof_entries.push((format!("{prefix}{basename}"), bytes));
            }
        }
    }
    let proof_count = proof_entries.len();

    let projects: Vec<NamedProject> = plans.into_iter().map(|p| p.project).collect();
    let (mut src_bytes, results) = if multi {
        let (bytes, discovered) = projects_from_text_components(&projects)?;
        (bytes, build_discovered(discovered))
    } else {
        let (bytes, project) =
            project_from_text_components(&projects[0].name, &projects[0].components)?;
        (bytes, vec![(String::new(), build(&project))])
    };
    // Injecting after discovery is safe: proof entries are invisible to
    // project discovery, and repack reads them from `src_bytes` as its
    // reconcile baselines.
    if !proof_entries.is_empty() {
        src_bytes = append_zip_entries(src_bytes, proof_entries)?;
    }
    let failed = gate_before_write(&results)?;

    let bytes = repack_results(&src_bytes, &results)?;
    if is_zip_output(&cli.output) {
        eventb_io::ensure_parent_dir(&cli.output)?;
        fs::write(&cli.output, &bytes)?;
    } else {
        extract_archive_to_dir(&bytes, &cli.output)?;
    }
    report_diagnostics(&results);

    if cli.verbose {
        let components: usize = projects.iter().map(|p| p.components.len()).sum();
        let generated: usize = results.iter().map(|(_, r)| r.files.len()).sum();
        eprintln!(
            "Wrote {} component(s), {} generated file(s), and {} proof file(s) across {} project(s) to {}",
            components,
            generated,
            proof_count,
            results.len(),
            cli.output.display()
        );
    }

    gate_after_write(&results, &failed, "the output")
}

/// Append `(entry name, bytes)` pairs to a serialized zip archive, consuming
/// the entry bytes so they are freed as soon as they are written.
fn append_zip_entries(bytes: Vec<u8>, entries: Vec<(String, Vec<u8>)>) -> CmdResult<Vec<u8>> {
    use std::io::Write;
    let mut cursor = std::io::Cursor::new(bytes);
    {
        let mut writer = zip::ZipWriter::new_append(&mut cursor)?;
        for (name, data) in entries {
            // The repack step inflates injected `.bpo`/`.bps` right back (they
            // are read once as reconcile baselines and dropped), so deflating
            // them here is pure loss; only `.bpr` survives into the output
            // (raw-copied compressed), so only it is worth deflating.
            let method = if name.ends_with(".bpr") {
                zip::CompressionMethod::Deflated
            } else {
                zip::CompressionMethod::Stored
            };
            let options = zip::write::SimpleFileOptions::default().compression_method(method);
            writer.start_file(name.as_str(), options)?;
            writer.write_all(&data)?;
        }
        writer.finish()?;
    }
    Ok(cursor.into_inner())
}

/// Extract a built archive into the output directory. Every entry name comes
/// from our own writers, but each `/`-separated segment is still validated so
/// a bad name can never write outside `out_dir`.
fn extract_archive_to_dir(bytes: &[u8], out_dir: &Path) -> CmdResult<()> {
    use std::io::Read;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name()?.to_string();
        if !name.split('/').all(is_normal_path_component) {
            return Err(format!("unsafe archive entry name {name:?}").into());
        }
        let dest = out_dir.join(&name);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut contents = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut contents)?;
        fs::write(dest, contents)?;
    }
    Ok(())
}

/// Split a directory into one project per immediate subdirectory.
///
/// Returns `Some(projects)` only when the directory holds **no** Event-B text
/// of its own and at least one immediate subdirectory does (recursively); each
/// such subdirectory becomes a project named after it, paired with its source
/// directory. Returns `None` for every other shape — files directly in `dir`,
/// or no subdirectory with Event-B text — so the caller exports a single flat
/// project instead. This keeps the multi-project trigger unambiguous (it never
/// emits a root `.project` beside sub-project ones) and exactly inverts
/// multi-project import output.
fn discover_text_projects(
    dir: &Path,
    verbose: bool,
) -> CmdResult<Option<Vec<(PathBuf, NamedProject)>>> {
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
        projects.push((subdir, NamedProject { name, components }));
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
