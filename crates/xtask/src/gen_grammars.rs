//! `rossi gen-grammars` — regenerate the editor syntax-highlighting grammars
//! from the canonical token tables.
//!
//! Two modes, like `rossi fmt`:
//! - default: write each grammar file in place.
//! - `--check`: report any file that is out of date and exit non-zero (no
//!   writes). CI runs this so a table change that isn't regenerated fails the
//!   build, exactly as `keywords_match_grammar` guards the tables themselves.
//!
//! Whole-file targets (TextMate JSON, Sublime YAML, the tree-sitter token
//! manifest, and the verbatim copies: Zed's snippets and highlights, the
//! standalone grammar repo's examples) are pure generated files. The Vim and
//! Emacs files, the tree-sitter `grammar.js`, and the standalone grammar's
//! `highlights.scm` carry hand-maintained scaffolding (and may be
//! hand-extended), so only the marked region between their generated markers is
//! replaced — the rest is preserved.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;

use crate::grammars::{
    Markers, Model, emacs, input_emacs, operators_nvim, operators_sublime, paths, snippets_emacs,
    snippets_nvim, snippets_vscode, sublime, textmate, vim, zed,
};

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

    // (relative path, desired full content), one entry per concrete file on
    // disk. Whole-file producers contribute one entry; multi-file producers
    // (the yasnippet directory, the Neovim snippet package) contribute several.
    // Region targets are spliced into the existing file's markers below.
    let mut targets: Vec<(String, String)> = vec![
        (paths::TEXTMATE.to_string(), textmate::render(&model)),
        (paths::SUBLIME.to_string(), sublime::render(&model)),
        (
            paths::SNIPPETS_VSCODE.to_string(),
            snippets_vscode::render(),
        ),
        (paths::NVIM_OPERATORS.to_string(), operators_nvim::render()),
        (paths::EMACS_INPUT.to_string(), input_emacs::render()),
        (
            paths::SUBLIME_OPERATORS.to_string(),
            operators_sublime::render(),
        ),
        (paths::TS_TOKENS.to_string(), zed::tokens_manifest(&model)),
    ];
    // Multi-file producers: each returns its own list of (rel path, content).
    targets.extend(snippets_nvim::render());
    targets.extend(snippets_emacs::render());
    for (rel, markers, body) in [
        (paths::VIM, &vim::MARKERS, vim::render(&model)),
        (paths::EMACS, &emacs::MARKERS, emacs::render(&model)),
        (
            paths::TS_GRAMMAR,
            &zed::MARKERS,
            zed::render_grammar_region(&model),
        ),
        (
            paths::TS_HIGHLIGHTS,
            &zed::MARKERS_SCM,
            zed::render_highlights_region(&model),
        ),
    ] {
        let path = root.join(rel);
        let existing = fs::read_to_string(&path).map_err(|e| io_err(&path, e))?;
        targets.push((rel.to_string(), splice(&existing, markers, &body, &path)?));
    }

    // Verbatim copies (see [`paths::COPIES`] for why each exists). A generated
    // source is copied from its just-computed desired content — never from disk
    // — so a copy cannot lag its source within one run, not even in content
    // outside a spliced region (e.g. hand-written captures in the standalone
    // highlights.scm flow into Zed's bundled copy).
    for (src, dst) in paths::COPIES {
        let content = match targets.iter().find(|(rel, _)| rel == src) {
            Some((_, content)) => content.clone(),
            None => {
                let path = root.join(src);
                fs::read_to_string(&path).map_err(|e| io_err(&path, e))?
            }
        };
        targets.push((dst.to_string(), content));
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

    // Prune orphans in fully-generated directories: a file left behind when its
    // source row is removed (e.g. a snippet dropped from the canonical table)
    // would silently disagree with the source of truth, the very drift this
    // command exists to prevent. `--check` reports it; a write removes it.
    // Dotfiles (e.g. yasnippet's `.yas-parents`) are left alone so any
    // hand-maintained directory metadata survives.
    let wanted: std::collections::HashSet<&str> =
        targets.iter().map(|(rel, _)| rel.as_str()).collect();
    for dir in [paths::EMACS_SNIPPETS_DIR, paths::TS_EXAMPLES_DIR] {
        let entries = match fs::read_dir(root.join(dir)) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || !path.is_file() {
                continue;
            }
            let rel = format!("{dir}/{name}");
            if wanted.contains(rel.as_str()) {
                continue;
            }
            if args.check {
                println!("{rel}");
                stale += 1;
            } else {
                fs::remove_file(&path).map_err(|e| io_err(&path, e))?;
                if args.verbose {
                    eprintln!("removed   {rel}");
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
