//! `rossi import` — convert Rodin archives into Event-B text.
//!
//! Reads Rodin inputs (`.zip` archives, individual `.buc`/`.bum` files, or
//! directories containing them) and writes human-readable `.eventb` text:
//! one file per component, or a single merged file with `--merge`.
//!
//! A Rodin `.zip` may bundle several top-level projects (an Eclipse "Archive
//! File" export of a decomposition). When more than one project is present
//! across the inputs, each is written under its own `<output>/<project>/`
//! subdirectory (or, with `--merge`, its own `<output>/<project>.eventb`) so
//! sibling components sharing a basename never overwrite. A single project keeps
//! the flat output unchanged.

use clap::Args;
use rossi::{FormulaSpacing, NamedComponent, NamedProject, PrettyPrinter};
use rossi_build::project::discover_projects;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use super::eventb_io::{self, CmdResult, InputFamily};

#[derive(Args)]
pub struct ImportArgs {
    /// Rodin inputs (.zip, .buc, .bum) or directories containing supported files
    #[arg(required = true, value_name = "INPUT")]
    inputs: Vec<PathBuf>,

    /// Output file (with --merge) or directory (one .eventb per component)
    #[arg(short, long, required = true, value_name = "OUTPUT")]
    output: PathBuf,

    /// Use ASCII operators in the text output
    #[arg(long)]
    ascii: bool,

    /// Indentation string for the text output (default: four spaces)
    #[arg(long, value_name = "STR")]
    indent: Option<String>,

    /// Merge all components into a single file, optionally specifying order
    /// (e.g., --merge=M1,C1,M2). Unmentioned components are appended at the end.
    #[arg(long, num_args = 0..=1, default_missing_value = "", require_equals = true, value_name = "ORDER")]
    merge: Option<String>,

    /// Show detailed progress
    #[arg(short, long)]
    verbose: bool,
}

pub fn run(cli: ImportArgs) -> ExitCode {
    match run_inner(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rossi import: {e}");
            ExitCode::from(1)
        }
    }
}

fn run_inner(cli: &ImportArgs) -> CmdResult<()> {
    for input in &cli.inputs {
        eventb_io::ensure_input(input, InputFamily::Rodin)?;
    }

    let projects = collect_projects(cli)?;
    let total: usize = projects.iter().map(|p| p.components.len()).sum();
    if total == 0 {
        return Err("No Event-B components found in input files".into());
    }
    let project_count = projects.len();

    let printer = PrettyPrinter {
        use_unicode: !cli.ascii,
        indent: cli.indent.clone().unwrap_or_else(|| "    ".to_string()),
        // Emitted text stays portable: never the private-use glyphs.
        private_use_glyphs: false,
        formula_spacing: FormulaSpacing::Readable,
    };

    // Multiple projects (a multi-project archive, or several inputs) are kept
    // apart under their own subdirectory / file; a single project writes flat,
    // exactly as before.
    let multi = project_count > 1;
    match (cli.merge.as_deref(), multi) {
        (Some(order), false) => write_merged_flat(cli, &printer, projects, order)?,
        (Some(order), true) => write_merged_per_project(cli, &printer, projects, order)?,
        (None, false) => write_files_flat(cli, &printer, projects)?,
        (None, true) => write_files_per_project(cli, &printer, projects)?,
    }

    if cli.verbose {
        eprintln!(
            "Wrote {total} component(s) across {project_count} project(s) to {}",
            cli.output.display()
        );
    }

    Ok(())
}

/// Read every input into one [`NamedProject`] per project: a `.zip` yields one
/// project per discovered project (keyed on its unique archive prefix), a
/// directory yields one project, and all loose `.buc`/`.bum` files fold into a
/// single project (so they never explode into a subdirectory each). Projects
/// with no components (a source-only `.project` dir, or a directory holding no
/// component files) are dropped — there is no text to write for them. Every
/// project name is reduced to a safe single path segment so an untrusted
/// archive cannot escape the output directory.
fn collect_projects(cli: &ImportArgs) -> CmdResult<Vec<NamedProject>> {
    let mut projects: Vec<NamedProject> = Vec::new();
    let mut loose: Vec<NamedComponent> = Vec::new();

    for input in &cli.inputs {
        if cli.verbose {
            eprintln!("Reading Rodin input: {}", input.display());
        }
        if input.is_dir() {
            let components = parse_rodin_directory(input)?;
            if cli.verbose {
                eprintln!("  Found {} component(s)", components.len());
            }
            if components.is_empty() {
                continue;
            }
            projects.push(NamedProject {
                name: eventb_io::safe_path_segment(&path_name(input)),
                components,
            });
        } else {
            match input.extension().and_then(|e| e.to_str()) {
                Some(ext) if ext.eq_ignore_ascii_case("zip") => {
                    let bytes = fs::read(input)?;
                    let fallback = file_stem(input);
                    for dp in discover_projects(&bytes, &fallback)? {
                        if dp.components.is_empty() {
                            continue;
                        }
                        let components = dp
                            .components
                            .into_iter()
                            .map(|pc| NamedComponent {
                                filename: pc.filename,
                                component: pc.component,
                            })
                            .collect::<Vec<_>>();
                        if cli.verbose {
                            eprintln!(
                                "  Found {} component(s) in project {}",
                                components.len(),
                                dp.name
                            );
                        }
                        // Key the output subdirectory on the unique archive
                        // prefix (the SSOT guarantees prefixes are distinct),
                        // not the resolved `.project` name, which can collide
                        // between sibling projects or carry path traversal.
                        projects.push(NamedProject {
                            name: eventb_io::safe_path_segment(dp.prefix.trim_end_matches('/')),
                            components,
                        });
                    }
                }
                Some(ext) if eventb_io::is_rodin_xml_ext(ext) => {
                    loose.push(eventb_io::parse_rodin_xml_file(input)?);
                }
                _ => return Err(format!("Unsupported Rodin input: {}", input.display()).into()),
            }
        }
    }

    if !loose.is_empty() {
        // The loose group is only ever namespaced (given a subdirectory) when it
        // sits beside another project; a neutral name avoids doubling the output
        // directory's own basename.
        projects.push(NamedProject {
            name: "components".to_string(),
            components: loose,
        });
    }

    Ok(projects)
}

/// Merge all components into the single output file (single-project default).
fn write_merged_flat(
    cli: &ImportArgs,
    printer: &PrettyPrinter,
    projects: Vec<NamedProject>,
    order: &str,
) -> CmdResult<()> {
    let mut components = into_single(projects);
    if !order.is_empty() {
        warn_unmatched_order(&component_names(&components), order);
        reorder_components(&mut components, order);
    }
    eventb_io::ensure_parent_dir(&cli.output)?;
    fs::write(&cli.output, render_merged(printer, &components))?;
    Ok(())
}

/// Merge each project into its own `<output>/<project>.eventb` file.
fn write_merged_per_project(
    cli: &ImportArgs,
    printer: &PrettyPrinter,
    projects: Vec<NamedProject>,
    order: &str,
) -> CmdResult<()> {
    fs::create_dir_all(&cli.output)?;
    if !order.is_empty() {
        // A name in the order list belongs to exactly one project, so warn once
        // over the whole archive — not once per project (which would flag every
        // other project's components as "not found").
        let all: Vec<String> = projects
            .iter()
            .flat_map(|p| component_names(&p.components))
            .collect();
        warn_unmatched_order(&all, order);
    }
    for mut project in projects {
        if !order.is_empty() {
            reorder_components(&mut project.components, order);
        }
        let path = cli.output.join(format!("{}.eventb", project.name));
        fs::write(&path, render_merged(printer, &project.components))?;
        if cli.verbose {
            eprintln!("  Wrote {}", path.display());
        }
    }
    Ok(())
}

/// Write one `.eventb` per component into the output directory (single project).
fn write_files_flat(
    cli: &ImportArgs,
    printer: &PrettyPrinter,
    projects: Vec<NamedProject>,
) -> CmdResult<()> {
    fs::create_dir_all(&cli.output)?;
    let components = into_single(projects);
    write_component_files(printer, &components, &cli.output, cli.verbose)
}

/// Write each project's components under its own `<output>/<project>/` directory.
fn write_files_per_project(
    cli: &ImportArgs,
    printer: &PrettyPrinter,
    projects: Vec<NamedProject>,
) -> CmdResult<()> {
    for project in projects {
        let dir = cli.output.join(&project.name);
        fs::create_dir_all(&dir)?;
        write_component_files(printer, &project.components, &dir, cli.verbose)?;
    }
    Ok(())
}

/// Flatten the (single) project's components.
fn into_single(projects: Vec<NamedProject>) -> Vec<NamedComponent> {
    projects
        .into_iter()
        .next()
        .map(|p| p.components)
        .unwrap_or_default()
}

/// The Event-B names of `components`.
fn component_names(components: &[NamedComponent]) -> Vec<String> {
    components
        .iter()
        .map(|c| c.component.name().to_string())
        .collect()
}

/// Concatenate components into one `.eventb` text body (blank-line separated).
fn render_merged(printer: &PrettyPrinter, components: &[NamedComponent]) -> String {
    let mut out = String::new();
    for (i, named) in components.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&printer.print_component(&named.component));
        out.push('\n');
    }
    out
}

/// Write each component as `<name>.eventb` in `dir`.
fn write_component_files(
    printer: &PrettyPrinter,
    components: &[NamedComponent],
    dir: &Path,
    verbose: bool,
) -> CmdResult<()> {
    for named in components {
        let path = dir.join(format!("{}.eventb", named.component.name()));
        let text = printer.print_component(&named.component);
        fs::write(&path, format!("{text}\n"))?;
        if verbose {
            eprintln!("  Wrote {}", path.display());
        }
    }
    Ok(())
}

/// A path's final component as a `String` (directory or file name).
fn path_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string()
}

/// A path's file stem as a `String`, used as the project-name fallback for a
/// flat archive carrying neither checked files nor a `.project`.
fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("project")
        .to_string()
}

fn parse_rodin_directory(dir: &Path) -> CmdResult<Vec<NamedComponent>> {
    let mut components = Vec::new();
    for file in eventb_io::collect_rodin_xml_files(&[dir.to_path_buf()])? {
        components.push(eventb_io::parse_rodin_xml_file(&file)?);
    }
    Ok(components)
}

/// Warn about names in an explicit `--merge` order that match no component.
/// Called once over all imported components, so a multi-project merge does not
/// flag a name as missing merely because it belongs to a different project.
fn warn_unmatched_order(all_names: &[String], order: &str) {
    for name in order.split(',').map(|s| s.trim()) {
        if !all_names.iter().any(|n| n == name) {
            eprintln!("Warning: '{name}' does not match any component");
        }
    }
}

/// Stable-sort `components` so those named in `order` come first, in list order;
/// unmentioned components keep their original relative order at the end.
/// Unmatched-name warnings are emitted separately by [`warn_unmatched_order`].
fn reorder_components(components: &mut [NamedComponent], order: &str) {
    let names: Vec<&str> = order.split(',').map(|s| s.trim()).collect();
    let order_pos = |c: &NamedComponent| -> usize {
        let n = c.component.name();
        names
            .iter()
            .position(|&name| name == n)
            .unwrap_or(usize::MAX)
    };
    components.sort_by_key(order_pos);
}
