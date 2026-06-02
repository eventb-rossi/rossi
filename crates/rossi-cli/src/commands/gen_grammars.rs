//! `rossi gen-grammars` — regenerate the editor syntax-highlighting grammars
//! from the canonical token tables.
//!
//! Two modes, like `rossi fmt`:
//! - default: write each grammar file in place.
//! - `--check`: report any file that is out of date and exit non-zero (no
//!   writes). CI runs this so a table change that isn't regenerated fails the
//!   build, exactly as `keywords_match_grammar` guards the tables themselves.
//!
//! Whole-file targets (TextMate JSON, Sublime YAML) are pure grammar files and
//! are generated entirely. The Vim and Emacs files carry hand-maintained logic,
//! so only the marked region between their generated markers is replaced.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;

use super::grammars::{Markers, Model, emacs, paths, sublime, textmate, vim};

#[derive(Args)]
pub struct GenGrammarsArgs {
    /// Report grammars that are out of date and exit non-zero (no writes)
    #[arg(long)]
    check: bool,

    /// Show each file as it is written or checked
    #[arg(short, long)]
    verbose: bool,
}

pub fn run(args: GenGrammarsArgs) -> ExitCode {
    match run_inner(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("rossi gen-grammars: {e}");
            ExitCode::from(1)
        }
    }
}

fn run_inner(args: &GenGrammarsArgs) -> Result<ExitCode, String> {
    let root = workspace_root();
    let model = Model::build();

    // (relative path, desired full content). Whole-file targets are rendered
    // entirely; region targets are spliced into the existing file's markers.
    let mut targets: Vec<(&str, String)> = vec![
        (paths::TEXTMATE, textmate::render(&model)),
        (paths::SUBLIME, sublime::render(&model)),
    ];
    for (rel, markers, body) in [
        (paths::VIM, &vim::MARKERS, vim::render(&model)),
        (paths::EMACS, &emacs::MARKERS, emacs::render(&model)),
    ] {
        let path = root.join(rel);
        let existing = fs::read_to_string(&path).map_err(|e| io_err(&path, e))?;
        targets.push((rel, splice(&existing, markers, &body, &path)?));
    }

    let mut stale = 0usize;
    for (rel, desired) in &targets {
        let path = root.join(rel);
        let up_to_date = fs::read_to_string(&path).ok().as_deref() == Some(desired.as_str());
        match (args.check, up_to_date) {
            (true, true) => {
                if args.verbose {
                    eprintln!("ok        {rel}");
                }
            }
            (true, false) => {
                println!("{rel}");
                stale += 1;
            }
            (false, true) => {
                if args.verbose {
                    eprintln!("unchanged {rel}");
                }
            }
            (false, false) => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|e| io_err(&path, e))?;
                }
                fs::write(&path, desired).map_err(|e| io_err(&path, e))?;
                if args.verbose {
                    eprintln!("wrote     {rel}");
                }
            }
        }
    }

    if args.check && stale > 0 {
        eprintln!(
            "\n{stale} editor grammar(s) are out of date; run `rossi gen-grammars` to regenerate."
        );
        return Ok(ExitCode::from(1));
    }
    Ok(ExitCode::SUCCESS)
}

/// Replace the text between `markers.begin` and `markers.end` (exclusive of the
/// marker lines themselves) with `body`.
fn splice(existing: &str, markers: &Markers, body: &str, path: &Path) -> Result<String, String> {
    let begin_at = existing.find(markers.begin).ok_or_else(|| {
        format!(
            "{}: missing begin marker `{}`",
            path.display(),
            markers.begin
        )
    })?;
    let after_begin = begin_at + markers.begin.len();
    let end_rel = existing[after_begin..]
        .find(markers.end)
        .ok_or_else(|| format!("{}: missing end marker `{}`", path.display(), markers.end))?;
    let end_at = after_begin + end_rel;

    let head = &existing[..after_begin];
    let tail = &existing[end_at..];
    Ok(format!("{head}\n{body}{tail}"))
}

/// Workspace root, resolved from this crate's manifest dir so the command works
/// from any cwd (and from the test harness).
fn workspace_root() -> PathBuf {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = crate_dir.join("..").join("..");
    fs::canonicalize(&root).unwrap_or(root)
}

fn io_err(path: &Path, e: std::io::Error) -> String {
    format!("{}: {e}", path.display())
}
