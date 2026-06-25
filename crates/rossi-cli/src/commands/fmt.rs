//! `rossi fmt` — reformat Event-B in place, without crossing the Rodin↔text
//! boundary.
//!
//! Operates on the *same* representation it is given:
//! - `.eventb`/`.txt` text is re-emitted with a chosen operator convention
//!   (`--ascii`/`--unicode`, default Unicode) and indentation (`--indent`).
//! - Rodin `.buc`/`.bum`/`.zip` inputs are re-serialised to rossi's canonical
//!   Unicode XML. (Rodin requires Unicode, so `--ascii` is rejected for these;
//!   `--indent` does not affect XML. Non-component zip entries — e.g. proofs —
//!   are preserved verbatim.) A multi-project archive keeps its per-project
//!   directory layout: every entry is rewritten under its original path, so a
//!   bundled decomposition normalises in place without flattening.
//!
//! Three write modes, mutually exclusive: `-i`/`--in-place` rewrites inputs,
//! `--check` reports unformatted inputs and exits non-zero, and `-o`/`--output`
//! writes elsewhere. With none of these, a single text input is printed to stdout.

use clap::Args;
use rossi::{PrettyPrinter, format_str, parse_xml, to_xml};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use super::eventb_io::{self, CmdResult, InputKind};

#[derive(Args)]
pub struct FmtArgs {
    /// Files (.eventb/.txt or Rodin .zip/.buc/.bum) or directories to format;
    /// `-` reads Event-B text from stdin and writes the result to stdout
    #[arg(required = true, value_name = "INPUT")]
    inputs: Vec<PathBuf>,

    /// Rewrite each input file in place
    #[arg(short = 'i', long = "in-place", conflicts_with_all = ["check", "output"])]
    in_place: bool,

    /// Report inputs that are not already formatted and exit non-zero (no writes)
    #[arg(long, conflicts_with_all = ["in_place", "output"])]
    check: bool,

    /// Write formatted output here (a file for a single input, a directory for many)
    #[arg(short, long, value_name = "OUTPUT", conflicts_with_all = ["in_place", "check"])]
    output: Option<PathBuf>,

    /// Use ASCII operators (Event-B text only; rejected for Rodin inputs)
    #[arg(long, conflicts_with = "unicode")]
    ascii: bool,

    /// Force Unicode operators (the default)
    #[arg(long)]
    unicode: bool,

    /// Indentation string for text output (default: four spaces)
    #[arg(long, value_name = "STR")]
    indent: Option<String>,

    /// Show detailed progress
    #[arg(short, long)]
    verbose: bool,
}

enum Mode {
    Stdout,
    InPlace,
    Check,
    Output(PathBuf),
}

/// The formatted form of one input, ready to write or compare.
enum Formatted {
    /// Text content (Event-B source or canonical Rodin XML).
    Text(String),
    /// A whole Rodin `.zip` archive.
    Zip(Vec<u8>),
}

pub fn run(cli: FmtArgs) -> ExitCode {
    match run_inner(&cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("rossi fmt: {e}");
            ExitCode::from(1)
        }
    }
}

fn run_inner(cli: &FmtArgs) -> CmdResult<ExitCode> {
    let mode = if cli.in_place {
        Mode::InPlace
    } else if cli.check {
        Mode::Check
    } else if let Some(o) = &cli.output {
        Mode::Output(o.clone())
    } else {
        Mode::Stdout
    };

    let printer = PrettyPrinter {
        use_unicode: !cli.ascii,
        indent: cli.indent.clone().unwrap_or_else(|| "    ".to_string()),
        // Emitted text stays portable: never the private-use glyphs.
        private_use_glyphs: false,
    };

    // `-` reads one Event-B text stream from stdin (the lone input). It has no
    // on-disk file to rewrite or compare against, so only stdout / -o apply.
    if eventb_io::stdin_is_sole_input(&cli.inputs)? {
        return fmt_stdin(cli, &printer, &mode);
    }

    // Expand inputs into a flat worklist of (file, kind).
    let mut items: Vec<(PathBuf, InputKind)> = Vec::new();
    for input in &cli.inputs {
        if !input.exists() {
            return Err(format!("Input not found: {}", input.display()).into());
        }
        if input.is_dir() {
            for f in eventb_io::collect_eventb_files(std::slice::from_ref(input))? {
                items.push((f, InputKind::Text));
            }
            for f in eventb_io::collect_rodin_xml_files(std::slice::from_ref(input))? {
                items.push((f, InputKind::RodinXml));
            }
        } else {
            items.push((input.clone(), eventb_io::classify_file(input)?));
        }
    }

    if items.is_empty() {
        return Err("No supported files found in inputs".into());
    }

    // --ascii only makes sense for text; Rodin XML must stay Unicode.
    if cli.ascii && items.iter().any(|(_, k)| *k != InputKind::Text) {
        return Err(
            "Rodin archives require Unicode operators; --ascii applies to Event-B text only".into(),
        );
    }

    // Printing to stdout only makes sense for a single text file.
    if matches!(mode, Mode::Stdout) && (items.len() != 1 || items[0].1 != InputKind::Text) {
        return Err(
            "refusing to print formatted output to stdout; use -i (in place), -o <OUTPUT>, or --check"
                .into(),
        );
    }

    let multi = items.len() > 1;
    let mut any_unformatted = false;

    for (path, kind) in &items {
        let (formatted, changed) = render(path, *kind, &printer)?;
        match &mode {
            Mode::Stdout => {
                // The stdout guard above guarantees a single text input.
                if let Formatted::Text(s) = &formatted {
                    print!("{s}");
                    if !s.ends_with('\n') {
                        println!();
                    }
                }
            }
            Mode::Check => {
                if changed {
                    any_unformatted = true;
                    println!("{}", path.display());
                }
            }
            Mode::InPlace => {
                if changed {
                    formatted.write_to(path)?;
                    if cli.verbose {
                        eprintln!("formatted {}", path.display());
                    }
                } else if cli.verbose {
                    eprintln!("unchanged {}", path.display());
                }
            }
            Mode::Output(out) => {
                let dest = if multi {
                    fs::create_dir_all(out)?;
                    let name = path
                        .file_name()
                        .ok_or_else(|| format!("input has no file name: {}", path.display()))?;
                    out.join(name)
                } else {
                    out.clone()
                };
                formatted.write_to(&dest)?;
                if cli.verbose {
                    eprintln!("wrote {}", dest.display());
                }
            }
        }
    }

    if matches!(mode, Mode::Check) && any_unformatted {
        return Ok(ExitCode::from(1));
    }
    Ok(ExitCode::SUCCESS)
}

/// Format a single Event-B text stream read from stdin (the `-` input).
fn fmt_stdin(cli: &FmtArgs, printer: &PrettyPrinter, mode: &Mode) -> CmdResult<ExitCode> {
    match mode {
        Mode::InPlace => return Err("cannot format standard input in place; drop -i".into()),
        Mode::Check => return Err("--check needs a file path, not standard input".into()),
        Mode::Stdout | Mode::Output(_) => {}
    }
    let src = eventb_io::read_stdin_to_string()?;
    let body = format_str(&src, printer).map_err(|e| format!("Failed to parse <stdin>: {e}"))?;
    let formatted = format!("{body}\n");
    match mode {
        Mode::Output(out) => {
            Formatted::Text(formatted).write_to(out)?;
            if cli.verbose {
                eprintln!("wrote {}", out.display());
            }
        }
        _ => print!("{formatted}"),
    }
    Ok(ExitCode::SUCCESS)
}

/// Read one input, format it, and report whether the result differs from what
/// is on disk. Reads and parses the input exactly once.
fn render(path: &Path, kind: InputKind, printer: &PrettyPrinter) -> CmdResult<(Formatted, bool)> {
    match kind {
        InputKind::Text => {
            let src = fs::read_to_string(path)?;
            let body = format_str(&src, printer)
                .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;
            // Keep the trailing newline convention used elsewhere for .eventb files.
            let formatted = format!("{body}\n");
            let changed = formatted != src;
            Ok((Formatted::Text(formatted), changed))
        }
        InputKind::RodinXml => {
            let xml = fs::read_to_string(path)?;
            let component = parse_xml(&xml)
                .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;
            let formatted = to_xml(&component);
            let changed = formatted != xml;
            Ok((Formatted::Text(formatted), changed))
        }
        InputKind::RodinZip => {
            let bytes = fs::read(path)?;
            let (normalized, changed) = normalize_zip(&bytes)
                .map_err(|e| format!("Failed to normalize {}: {}", path.display(), e))?;
            Ok((Formatted::Zip(normalized), changed))
        }
    }
}

impl Formatted {
    fn write_to(&self, path: &Path) -> CmdResult<()> {
        eventb_io::ensure_parent_dir(path)?;
        match self {
            Formatted::Text(s) => fs::write(path, s)?,
            Formatted::Zip(b) => fs::write(path, b)?,
        }
        Ok(())
    }
}

fn stored_options() -> zip::write::SimpleFileOptions {
    zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)
}

/// Re-serialise every `.buc`/`.bum` entry of a Rodin zip to canonical Unicode
/// XML, copying all other entries (proofs, etc.) through unchanged. Returns the
/// rebuilt archive and whether any component entry was not already canonical.
///
/// Each entry is rewritten under its original name, so a multi-project archive's
/// `<project>/` directory layout is preserved exactly — components are
/// normalised per file regardless of which project they belong to. (Bare
/// directory entries are dropped, which is harmless: Rodin reconstructs
/// directories from the file paths on re-import.)
fn normalize_zip(bytes: &[u8]) -> CmdResult<(Vec<u8>, bool)> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    let mut out = Vec::new();
    let mut changed = false;
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut out));
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().to_string();
            if is_component_entry(&name) {
                let mut xml = String::new();
                entry.read_to_string(&mut xml)?;
                let component = parse_xml(&xml).map_err(|e| format!("{name}: {e}"))?;
                let canonical = to_xml(&component);
                changed |= canonical != xml;
                writer.start_file(name, stored_options())?;
                writer.write_all(canonical.as_bytes())?;
            } else {
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf)?;
                writer.start_file(name, stored_options())?;
                writer.write_all(&buf)?;
            }
        }
        writer.finish()?;
    }
    Ok((out, changed))
}

fn is_component_entry(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(eventb_io::is_rodin_xml_ext)
}
