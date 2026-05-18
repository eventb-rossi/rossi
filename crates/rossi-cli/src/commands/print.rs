use clap::Args;
use rossi::{
    NamedComponent, component_filename, parse_components, parse_zip_file, to_string,
    to_string_ascii, write_zip_file,
};
use rossi_build::ProjectComponent;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Args)]
pub struct PrintArgs {
    /// Input files (.zip, .buc, .bum, .eventb) or directories containing supported files
    #[arg(required = true, value_name = "INPUT")]
    inputs: Vec<PathBuf>,

    /// Output file or directory
    #[arg(short, long, required = true, value_name = "OUTPUT")]
    output: PathBuf,

    /// Use ASCII operators in text output (Rodin-to-text only)
    #[arg(long)]
    ascii: bool,

    /// Merge all components into a single file, optionally specifying order
    /// (e.g., --merge M1,C1,M2). Unmentioned components are appended at the end.
    #[arg(long, num_args = 0..=1, default_missing_value = "", require_equals = true, value_name = "ORDER")]
    merge: Option<String>,

    /// Show detailed progress
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    RodinToText,
    TextToZip,
}

pub fn run(cli: PrintArgs) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mode = detect_mode(&cli.inputs)?;

    match mode {
        Mode::RodinToText => rodin_to_text(&cli),
        Mode::TextToZip => text_to_zip(&cli),
    }
}

fn detect_mode(inputs: &[PathBuf]) -> std::result::Result<Mode, Box<dyn std::error::Error>> {
    let mut has_rodin = false;
    let mut has_text = false;

    for input in inputs {
        if !input.exists() {
            return Err(format!("Input not found: {}", input.display()).into());
        }
        if input.is_dir() {
            let kinds = detect_directory_kinds(input)?;
            has_rodin |= kinds.has_rodin;
            has_text |= kinds.has_text;
        } else {
            match input.extension().and_then(|e| e.to_str()) {
                Some(ext) if ext.eq_ignore_ascii_case("zip") => has_rodin = true,
                Some(ext) if is_rodin_xml_ext(ext) => has_rodin = true,
                Some(ext) if ext.eq_ignore_ascii_case("eventb") => has_text = true,
                Some(ext) if ext.eq_ignore_ascii_case("txt") => has_text = true,
                Some(ext) => {
                    return Err(format!(
                        "Unsupported file extension '.{}': {}",
                        ext,
                        input.display()
                    )
                    .into());
                }
                None => {
                    return Err(format!("File has no extension: {}", input.display()).into());
                }
            }
        }
    }

    if has_rodin && has_text {
        return Err(
            "Cannot mix Rodin inputs (.zip/.buc/.bum/directories) with .eventb/.txt inputs".into(),
        );
    }

    if has_rodin {
        Ok(Mode::RodinToText)
    } else {
        Ok(Mode::TextToZip)
    }
}

fn rodin_to_text(cli: &PrintArgs) -> std::result::Result<(), Box<dyn std::error::Error>> {
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

    let printer: fn(&rossi::Component) -> String = if cli.ascii {
        to_string_ascii
    } else {
        to_string
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
            output.push_str(&printer(&named.component));
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
            let name = match &named.component {
                rossi::Component::Context(c) => &c.name,
                rossi::Component::Machine(m) => &m.name,
            };
            let filename = format!("{}.eventb", name);
            let path = cli.output.join(&filename);
            let text = printer(&named.component);
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

fn text_to_zip(cli: &PrintArgs) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let eventb_files = collect_eventb_files(&cli.inputs)?;

    if eventb_files.is_empty() {
        return Err("No .eventb or .txt files found in inputs".into());
    }

    let mut components = Vec::new();

    for path in &eventb_files {
        if cli.verbose {
            eprintln!("Parsing: {}", path.display());
        }
        let source = fs::read_to_string(path)?;
        let parsed = parse_components(&source)
            .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;
        for component in parsed {
            let filename = component_filename(&component);
            components.push(NamedComponent {
                filename,
                component,
            });
        }
    }

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

#[derive(Default)]
struct DirectoryKinds {
    has_rodin: bool,
    has_text: bool,
}

fn detect_directory_kinds(
    dir: &Path,
) -> std::result::Result<DirectoryKinds, Box<dyn std::error::Error>> {
    let mut kinds = DirectoryKinds::default();

    for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if is_rodin_xml_ext(ext) {
                kinds.has_rodin = true;
            } else if ext.eq_ignore_ascii_case("eventb") || ext.eq_ignore_ascii_case("txt") {
                kinds.has_text = true;
            }
        }
    }

    if !kinds.has_rodin && !kinds.has_text {
        return Err(format!(
            "Directory contains no .buc, .bum, .eventb, or .txt inputs: {}",
            dir.display()
        )
        .into());
    }

    Ok(kinds)
}

fn parse_rodin_input(
    path: &Path,
) -> std::result::Result<Vec<NamedComponent>, Box<dyn std::error::Error>> {
    if path.is_dir() {
        return parse_rodin_directory(path);
    }

    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("zip") => Ok(parse_zip_file(path)?),
        Some(ext) if is_rodin_xml_ext(ext) => parse_rodin_xml_file(path).map(|c| vec![c]),
        _ => Err(format!("Unsupported Rodin input: {}", path.display()).into()),
    }
}

fn parse_rodin_directory(
    dir: &Path,
) -> std::result::Result<Vec<NamedComponent>, Box<dyn std::error::Error>> {
    let mut components = Vec::new();
    for file in collect_rodin_xml_files(&[dir.to_path_buf()])? {
        components.push(parse_rodin_xml_file(&file)?);
    }
    Ok(components)
}

fn parse_rodin_xml_file(
    path: &Path,
) -> std::result::Result<NamedComponent, Box<dyn std::error::Error>> {
    let xml = fs::read_to_string(path)?;
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Invalid filename: {}", path.display()))?;
    let component = ProjectComponent::from_xml(filename, &xml)?;
    Ok(NamedComponent {
        filename: component.filename,
        component: component.component,
    })
}

fn is_rodin_xml_ext(ext: &str) -> bool {
    ext.eq_ignore_ascii_case("buc") || ext.eq_ignore_ascii_case("bum")
}

fn component_name(named: &NamedComponent) -> &str {
    match &named.component {
        rossi::Component::Context(c) => &c.name,
        rossi::Component::Machine(m) => &m.name,
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

fn collect_eventb_files(
    inputs: &[PathBuf],
) -> std::result::Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut files = Vec::new();

    for input in inputs {
        if input.is_dir() {
            for entry in WalkDir::new(input).into_iter().filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.is_file()
                    && let Some(ext) = path.extension()
                    && (ext.eq_ignore_ascii_case("eventb") || ext.eq_ignore_ascii_case("txt"))
                {
                    files.push(path.to_path_buf());
                }
            }
        } else {
            files.push(input.clone());
        }
    }

    files.sort();
    Ok(files)
}

fn collect_rodin_xml_files(
    inputs: &[PathBuf],
) -> std::result::Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut files = Vec::new();

    for input in inputs {
        if input.is_dir() {
            for entry in WalkDir::new(input).into_iter().filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.is_file()
                    && let Some(ext) = path.extension().and_then(|e| e.to_str())
                    && is_rodin_xml_ext(ext)
                {
                    files.push(path.to_path_buf());
                }
            }
        } else {
            files.push(input.clone());
        }
    }

    files.sort();
    Ok(files)
}
