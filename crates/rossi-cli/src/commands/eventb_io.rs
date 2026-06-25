//! Shared input handling for the Event-B conversion/formatting commands.
//!
//! `import`, `export`, and `fmt` all need to recognise the two families of
//! inputs rossi understands — Rodin files (`.zip`, `.buc`, `.bum`, or
//! directories of those) and Event-B text (`.eventb`/`.txt`) — and to collect
//! and parse them. The shared logic lives here so each command can stay focused
//! on its own direction.

use rossi::{NamedComponent, component_filename, parse_components};
use rossi_build::ProjectComponent;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub(crate) type CmdResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Whether an extension is a Rodin XML component file (`.buc` or `.bum`).
pub(crate) fn is_rodin_xml_ext(ext: &str) -> bool {
    ext.eq_ignore_ascii_case("buc") || ext.eq_ignore_ascii_case("bum")
}

/// Whether an extension is Event-B text (`.eventb` or `.txt`).
pub(crate) fn is_text_ext(ext: &str) -> bool {
    ext.eq_ignore_ascii_case("eventb") || ext.eq_ignore_ascii_case("txt")
}

/// Whether an extension is the canonical Event-B source extension (`.eventb`).
///
/// Unlike [`is_text_ext`], this excludes the generic `.txt` — used where a file
/// must be treated as a definite Event-B component (e.g. deciding a directory's
/// project layout), mirroring `rossi-build`'s "a `README.txt` is not a
/// component" convention.
pub(crate) fn is_eventb_ext(ext: &str) -> bool {
    ext.eq_ignore_ascii_case("eventb")
}

/// Whether an extension is a Rodin `.zip` archive.
pub(crate) fn is_zip_ext(ext: &str) -> bool {
    ext.eq_ignore_ascii_case("zip")
}

/// Whether a path argument denotes standard input (the `-` convention).
pub(crate) fn is_stdin(p: &Path) -> bool {
    p.as_os_str() == "-"
}

/// Read all of standard input into a string (for the `-` input convention).
pub(crate) fn read_stdin_to_string() -> CmdResult<String> {
    Ok(std::io::read_to_string(std::io::stdin())?)
}

/// Enforce the `-` (stdin) convention: `-` may only appear as the sole input.
/// Returns whether that lone input is stdin (i.e. the command should read it).
pub(crate) fn stdin_is_sole_input(inputs: &[PathBuf]) -> CmdResult<bool> {
    let has_stdin = inputs.iter().any(|p| is_stdin(p));
    if has_stdin && inputs.len() > 1 {
        return Err("'-' (stdin) must be the only input".into());
    }
    Ok(has_stdin)
}

/// Which family of inputs a command reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputFamily {
    /// Rodin: `.zip`, `.buc`, `.bum`.
    Rodin,
    /// Event-B text: `.eventb`, `.txt`.
    Text,
}

/// The kind of a single (non-directory) input file, used by `fmt` to route
/// each input to the right formatter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputKind {
    /// `.eventb` / `.txt` Event-B text.
    Text,
    /// A single `.buc` / `.bum` Rodin component file.
    RodinXml,
    /// A Rodin `.zip` archive.
    RodinZip,
}

impl InputKind {
    pub(crate) fn family(self) -> InputFamily {
        match self {
            InputKind::Text => InputFamily::Text,
            InputKind::RodinXml | InputKind::RodinZip => InputFamily::Rodin,
        }
    }
}

/// Classify a single input file by extension.
pub(crate) fn classify_file(p: &Path) -> CmdResult<InputKind> {
    match p.extension().and_then(|e| e.to_str()) {
        Some(ext) if is_text_ext(ext) => Ok(InputKind::Text),
        Some(ext) if is_rodin_xml_ext(ext) => Ok(InputKind::RodinXml),
        Some(ext) if is_zip_ext(ext) => Ok(InputKind::RodinZip),
        Some(ext) => Err(format!("Unsupported file extension '.{}': {}", ext, p.display()).into()),
        None => Err(format!("File has no extension: {}", p.display()).into()),
    }
}

/// Reject an input the command cannot read, pointing at the command that can.
///
/// Directories are accepted by either family (their contents are validated when
/// collected). A file is classified by [`classify_file`]; one from the wrong
/// family yields a "use `rossi <other>`" hint.
pub(crate) fn ensure_input(p: &Path, want: InputFamily) -> CmdResult<()> {
    if !p.exists() {
        return Err(format!("Input not found: {}", p.display()).into());
    }
    if p.is_dir() {
        return Ok(());
    }
    if classify_file(p)?.family() == want {
        return Ok(());
    }
    Err(match want {
        InputFamily::Rodin => format!(
            "import reads Rodin inputs (.zip/.buc/.bum/dir); '{}' is Event-B text \u{2014} use `rossi export`",
            p.display()
        ),
        InputFamily::Text => format!(
            "export reads Event-B text (.eventb/.txt/dir); '{}' is a Rodin file \u{2014} use `rossi import`",
            p.display()
        ),
    }
    .into())
}

/// Parse Event-B text into named components, tagging parse errors with `label`.
pub(crate) fn parse_text_components(label: &str, source: &str) -> CmdResult<Vec<NamedComponent>> {
    let parsed = parse_components(source).map_err(|e| format!("Failed to parse {label}: {e}"))?;
    Ok(parsed
        .into_iter()
        .map(|component| NamedComponent {
            filename: component_filename(&component),
            component,
        })
        .collect())
}

/// Parse a single `.buc`/`.bum` Rodin XML file into a named component.
pub(crate) fn parse_rodin_xml_file(path: &Path) -> CmdResult<NamedComponent> {
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

/// Collect `.eventb`/`.txt` files from the given inputs. Directories are walked
/// recursively; explicit file paths are taken as-is. Results are sorted.
pub(crate) fn collect_eventb_files(inputs: &[PathBuf]) -> CmdResult<Vec<PathBuf>> {
    let mut files = Vec::new();

    for input in inputs {
        if input.is_dir() {
            for entry in WalkDir::new(input).into_iter().filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.is_file()
                    && let Some(ext) = path.extension().and_then(|e| e.to_str())
                    && is_text_ext(ext)
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

/// Collect `.buc`/`.bum` files from the given inputs. Directories are walked
/// recursively; explicit file paths are taken as-is. Results are sorted.
pub(crate) fn collect_rodin_xml_files(inputs: &[PathBuf]) -> CmdResult<Vec<PathBuf>> {
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

/// Create an output file's parent directory, skipping a missing/empty parent.
///
/// Shared by the commands that write a single output file (`.zip`, `.eventb`);
/// the directory writers create their own root, so this is only needed for file
/// outputs.
pub(crate) fn ensure_parent_dir(path: &Path) -> CmdResult<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}
