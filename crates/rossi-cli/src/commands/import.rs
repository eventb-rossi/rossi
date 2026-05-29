//! `rossi import` — convert Rodin archives into Event-B text.
//!
//! Reads Rodin inputs (`.zip` archives, individual `.buc`/`.bum` files, or
//! directories containing them) and writes human-readable `.eventb` text:
//! one file per component, or a single merged file with `--merge`.

use clap::Args;
use rossi::{Component, NamedComponent, PrettyPrinter, parse_zip_file};
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

    let mut all_components = Vec::new();
    for input in &cli.inputs {
        if cli.verbose {
            eprintln!("Reading Rodin input: {}", input.display());
        }
        let components = parse_rodin_input(input)?;
        if cli.verbose {
            eprintln!("  Found {} component(s)", components.len());
        }
        all_components.extend(components);
    }

    if all_components.is_empty() {
        return Err("No Event-B components found in input files".into());
    }

    let printer = PrettyPrinter {
        use_unicode: !cli.ascii,
        indent: cli.indent.clone().unwrap_or_else(|| "    ".to_string()),
    };

    if let Some(ref order) = cli.merge {
        // Reorder components if an explicit order was provided
        if !order.is_empty() {
            reorder_components(&mut all_components, order);
        }

        // Write all components into a single file
        let mut output = String::new();
        for (i, named) in all_components.iter().enumerate() {
            if i > 0 {
                output.push('\n');
            }
            output.push_str(&printer.print_component(&named.component));
            output.push('\n');
        }
        if let Some(parent) = cli.output.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(&cli.output, &output)?;
        if cli.verbose {
            eprintln!(
                "Wrote {} component(s) to {}",
                all_components.len(),
                cli.output.display()
            );
        }
    } else {
        // Write each component to a separate file in the output directory
        fs::create_dir_all(&cli.output)?;
        for named in &all_components {
            let filename = format!("{}.eventb", component_name(named));
            let path = cli.output.join(&filename);
            let text = printer.print_component(&named.component);
            fs::write(&path, format!("{}\n", text))?;
            if cli.verbose {
                eprintln!("  Wrote {}", path.display());
            }
        }
        if cli.verbose {
            eprintln!(
                "Wrote {} file(s) to {}",
                all_components.len(),
                cli.output.display()
            );
        }
    }

    Ok(())
}

fn parse_rodin_input(path: &Path) -> CmdResult<Vec<NamedComponent>> {
    if path.is_dir() {
        return parse_rodin_directory(path);
    }

    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("zip") => Ok(parse_zip_file(path)?),
        Some(ext) if eventb_io::is_rodin_xml_ext(ext) => {
            eventb_io::parse_rodin_xml_file(path).map(|c| vec![c])
        }
        _ => Err(format!("Unsupported Rodin input: {}", path.display()).into()),
    }
}

fn parse_rodin_directory(dir: &Path) -> CmdResult<Vec<NamedComponent>> {
    let mut components = Vec::new();
    for file in eventb_io::collect_rodin_xml_files(&[dir.to_path_buf()])? {
        components.push(eventb_io::parse_rodin_xml_file(&file)?);
    }
    Ok(components)
}

fn component_name(named: &NamedComponent) -> &str {
    match &named.component {
        Component::Context(c) => &c.name,
        Component::Machine(m) => &m.name,
    }
}

fn reorder_components(components: &mut [NamedComponent], order: &str) {
    let names: Vec<&str> = order.split(',').map(|s| s.trim()).collect();

    // Warn about names that don't match any component
    for name in &names {
        if !components.iter().any(|c| component_name(c) == *name) {
            eprintln!("Warning: '{}' does not match any component", name);
        }
    }

    // Stable sort: components whose name appears in the order list come first,
    // sorted by their position in the list. Unmentioned components keep their
    // original relative order and appear after the named ones.
    let order_pos = |c: &NamedComponent| -> usize {
        let n = component_name(c);
        names
            .iter()
            .position(|&name| name == n)
            .unwrap_or(usize::MAX)
    };
    components.sort_by_key(order_pos);
}
