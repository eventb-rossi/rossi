//! Guards documentation and the canonical snippet table against invalid
//! Event-B syntax.
//!
//! Every complete component shown in a ```eventb fenced block of a git-tracked
//! markdown file must parse, and every snippet in [`rossi::snippets::SNIPPETS`]
//! must expand to parseable Event-B. The editor snippet libraries (VS Code,
//! Neovim, Zed, Emacs) are generated from that table by `rossi gen-grammars`,
//! whose `--check` mode guards file↔table sync — so validating the table covers
//! all of them at the source. Doc blocks that intentionally show errors or UI
//! annotations (←, ⮟, ▶, "<- ") and fragments that don't start with
//! CONTEXT/MACHINE are skipped.
//!
//! The VS Code extension's New Event-B Project starter files are the one piece
//! of user-facing Event-B text not rendered from the snippet table, so they are
//! extracted from the TypeScript source and parsed here as well.

use std::path::{Path, PathBuf};
use std::process::Command;

const ANNOTATION_MARKERS: &[&str] = &["←", "⮟", "▶", "<- "];

/// Repo root per `git rev-parse`, or None outside a git checkout of this repo
/// (e.g. a published crate tarball or a vendored copy), where the doc sweep
/// has nothing meaningful to scan and is skipped.
fn git_repo_root() -> Option<PathBuf> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let root = PathBuf::from(String::from_utf8(out.stdout).ok()?.trim_end());
    // A vendored copy inside some other project's repo would resolve to that
    // repo's root; only our own checkout has this crate at crates/rossi.
    root.join("crates/rossi/Cargo.toml")
        .exists()
        .then_some(root)
}

/// Markdown files tracked by git. Compared to a directory walk this respects
/// .gitignore and .git/info/exclude, and leaves out submodule checkouts and
/// stray worktrees without a hardcoded skip list.
fn markdown_files(root: &Path) -> Vec<PathBuf> {
    let out = Command::new("git")
        .args(["ls-files", "-z", "--", "*.md"])
        .current_dir(root)
        .output()
        .expect("run git ls-files");
    assert!(
        out.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .expect("utf-8 paths from git ls-files")
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| root.join(s))
        .collect()
}

/// Extracts the contents of ```eventb fenced blocks (with their 1-based start
/// line), handling fences indented inside lists by stripping the fence's own
/// indentation from each block line.
fn eventb_blocks(text: &str) -> Vec<(usize, String)> {
    let mut blocks = Vec::new();
    let mut body = String::new();
    let mut start = 0;
    let mut fence_indent: Option<&str> = None;
    for (lineno, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some(indent) = fence_indent {
            if trimmed.starts_with("```") {
                blocks.push((start + 1, std::mem::take(&mut body)));
                fence_indent = None;
            } else {
                body.push_str(line.strip_prefix(indent).unwrap_or(trimmed));
                body.push('\n');
            }
        } else if trimmed == "```eventb" {
            start = lineno;
            fence_indent = Some(&line[..line.len() - trimmed.len()]);
        }
    }
    blocks
}

fn is_complete_component(block: &str) -> bool {
    let first = block.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let first = first.trim_start();
    first.starts_with("CONTEXT") || first.starts_with("MACHINE")
}

fn has_annotations(block: &str) -> bool {
    ANNOTATION_MARKERS.iter().any(|m| block.contains(m))
}

#[test]
fn doc_eventb_blocks_parse() {
    let Some(root) = git_repo_root() else {
        eprintln!("skipping: not inside a rossi git checkout");
        return;
    };
    let mut checked = 0;
    let mut skipped = 0;
    let mut failures = Vec::new();

    for file in markdown_files(&root) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for (line, block) in eventb_blocks(&text) {
            if !is_complete_component(&block) || has_annotations(&block) {
                skipped += 1;
                continue;
            }
            checked += 1;
            if let Err(e) = rossi::parse_components(&block) {
                failures.push(format!(
                    "{}:{} failed to parse:\n{}\n--- block ---\n{}",
                    file.display(),
                    line,
                    e,
                    block
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} invalid ```eventb doc block(s):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    // Sanity check that extraction still finds the known examples; if this
    // fires, the fence detection above broke, not the docs. The floor sits
    // one below the count of complete-component ```eventb blocks in tracked
    // markdown (the editor READMEs/INSTALL) so a routine doc edit doesn't trip
    // it, while a fence-detection collapse to ~0 still does.
    assert!(
        checked >= 3,
        "only {checked} eventb blocks checked ({skipped} skipped) — extraction is broken"
    );
}

/// Default placeholder texts in snippets (e.g. `predicate`) are not valid
/// Event-B on their own, so expansion substitutes a minimal valid value per
/// placeholder name before parsing.
fn placeholder_value(name: &str) -> &str {
    match name {
        "predicate" | "guard" | "condition" => "x = 1",
        "action" => "x := 1",
        "witness" => "abs_param = x",
        "expression" => "x + 1",
        "value" => "0",
        "set" | "domain" => "{1}",
        "SET_NAME" => "S",
        "context_name" | "machine_name" | "event_name" => "n",
        "concrete_event" | "abstract_event" => "e",
        "const_name" | "var_name" | "var" | "new_param" => "x",
        other => other, // labels (axm1, …) and identifiers stay as-is
    }
}

fn expand_placeholders(template: &str) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[at..]);
            return out;
        };
        let inner = &after[..end];
        let name = inner.split_once(':').map_or(inner, |(_, n)| n);
        out.push_str(placeholder_value(name));
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Wraps snippet fragments into a minimal component so they can be parsed.
/// Keyed on the snippet's trigger prefix; a new snippet with an unknown prefix
/// fails the test with an explicit "add a wrapper" message.
fn wrap_snippet(prefix: &str, body: &str) -> Option<String> {
    const INIT: &str = "EVENT INITIALISATION\nBEGIN\nx := 0\nEND";
    match prefix {
        "ctx" | "mch" => Some(body.to_string()),
        "evt" => Some(format!(
            "MACHINE m\nVARIABLES x\nEVENTS\n{INIT}\n{body}\nEND\n"
        )),
        "init" => Some(format!("MACHINE m\nVARIABLES x\nEVENTS\n{body}\nEND\n")),
        "refines" => Some(format!(
            "MACHINE m\nREFINES a\nVARIABLES x\nEVENTS\n{INIT}\n{body}\nEND\n"
        )),
        "axm" => Some(format!("CONTEXT c\nCONSTANTS x\nAXIOMS\n{body}\nEND\n")),
        "inv" => Some(format!(
            "MACHINE m\nVARIABLES x\nINVARIANTS\n{body}\nEVENTS\n{INIT}\nEND\n"
        )),
        "grd" => Some(format!(
            "MACHINE m\nVARIABLES x\nEVENTS\n{INIT}\nEVENT e\nWHERE\n{body}\nTHEN\nx := 1\nEND\nEND\n"
        )),
        "act" | "actnd" | "actst" => Some(format!(
            "MACHINE m\nVARIABLES x\nEVENTS\n{INIT}\nEVENT e\nBEGIN\n{body}\nEND\nEND\n"
        )),
        "forall" | "exists" => Some(format!(
            "CONTEXT c\nCONSTANTS x\nAXIOMS\n@axm1 {body}\nEND\n"
        )),
        "lambda" | "setcomp" => Some(format!(
            "CONTEXT c\nCONSTANTS x\nAXIOMS\n@axm1 x = {body}\nEND\n"
        )),
        _ => None,
    }
}

/// Validates the canonical snippet table itself — the single source the editor
/// snippet libraries are rendered from. File↔table drift is separately gated
/// by `gen-grammars --check` (CI and crates/rossi-cli/tests/gen_grammars_test.rs).
#[test]
fn canonical_snippets_expand_to_valid_eventb() {
    let mut failures = Vec::new();

    for snippet in rossi::snippets::SNIPPETS {
        let expanded = expand_placeholders(&snippet.body.join("\n"));
        let Some(source) = wrap_snippet(snippet.prefix, &expanded) else {
            failures.push(format!(
                "snippet '{}' (prefix '{}') has no wrapper — add one to wrap_snippet()",
                snippet.name, snippet.prefix
            ));
            continue;
        };
        if let Err(e) = rossi::parse_components(&source) {
            failures.push(format!(
                "snippet '{}' expands to invalid Event-B:\n{}\n--- source ---\n{}",
                snippet.name, e, source
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} invalid snippet(s) in rossi::snippets::SNIPPETS:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// Validates the starter files the VS Code New Event-B Project command writes
/// (`starterContext()`/`starterMachine()` in rossiCommands.ts). Each file must
/// hold a single valid component — `rossi::parse` is the same entry point the
/// language server runs per document. Extraction failures are reported
/// explicitly so a refactor of the TypeScript file can't silently drop this
/// coverage.
#[test]
fn vscode_starter_project_files_parse() {
    let Some(root) = git_repo_root() else {
        eprintln!("skipping: not inside a rossi git checkout");
        return;
    };
    let ts_path = root.join("editors/vscode/src/rossiCommands.ts");
    let source = std::fs::read_to_string(&ts_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", ts_path.display()));

    for function_name in ["starterContext", "starterMachine"] {
        let function = source
            .split_once(&format!("function {function_name}"))
            .unwrap_or_else(|| {
                panic!("rossiCommands.ts no longer defines {function_name} — update this test")
            })
            .1;
        let template = function
            .split_once('`')
            .and_then(|(_, rest)| rest.split_once("`;"))
            .unwrap_or_else(|| {
                panic!("{function_name} no longer returns a template literal — update this test")
            })
            .0;
        let component = template.replace("${name}", "my_model");

        if let Err(e) = rossi::parse(&component) {
            panic!(
                "VS Code starter file from {function_name}() is invalid Event-B:\n{e}\n--- component ---\n{component}"
            );
        }
    }
}
