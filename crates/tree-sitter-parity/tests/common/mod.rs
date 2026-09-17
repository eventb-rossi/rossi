//! Shared scaffolding for the two parity gates.
//!
//! Both read a directory named by an environment variable, feed text to the
//! tree-sitter grammar, and ask whether it derived cleanly. Keeping the answer
//! to "did tree-sitter accept this" in one place matters more than the line
//! count: the two gates would otherwise be free to disagree about what
//! acceptance means.

#![allow(dead_code)]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tree_sitter::{Node, Parser};

/// The repository root (two levels up from this crate's manifest).
pub fn repository_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

/// The directory named by `variable`, or `None` when it is unset or does not
/// name one.
///
/// A relative value resolves from the repository root, matching how the
/// in-workspace corpus harnesses read the same kind of variable. There is no
/// default: these directories live outside the repository and only the caller
/// knows where.
pub fn directory_from_env(variable: &str) -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var(variable).ok()?);
    let path = if path.is_absolute() {
        path
    } else {
        repository_root().join(path)
    };
    path.is_dir().then_some(path)
}

/// The files in `dir` with the given extension, sorted for deterministic
/// iteration.
pub fn collect_files(dir: &Path, extension: &str) -> std::io::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|found| found == extension))
        .collect();
    files.sort();
    Ok(files)
}

/// A parser loaded with the Event-B grammar.
pub fn eventb_parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_eventb::LANGUAGE.into())
        .expect("load tree-sitter Event-B grammar");
    parser
}

/// Whether tree-sitter derives `text` with no `ERROR` or `MISSING` node — the
/// two ways it reports text it cannot derive.
pub fn tree_sitter_accepts(parser: &mut Parser, text: &str) -> bool {
    let tree = parser.parse(text, None).expect("parser was cancelled");
    first_issue(tree.root_node()).is_none()
}

/// The first `ERROR` or `MISSING` node, for a caller that wants to report
/// where the derivation failed.
pub fn first_issue(node: Node<'_>) -> Option<Node<'_>> {
    if node.is_error() || node.is_missing() {
        return Some(node);
    }
    (0..node.child_count() as u32).find_map(|index| first_issue(node.child(index)?))
}

/// Serialises the panic-hook swap in [`without_panicking`]: `set_hook` is
/// process-global, so two tests in one binary swapping it concurrently could
/// leave the silencing hook installed and hide a later genuine failure.
static PANIC_HOOK: Mutex<()> = Mutex::new(());

/// Run `body`, turning a panic into `None` and silencing its message.
///
/// A corpus of adversarial input is exactly where a parser crash lives, so a
/// gate pointed at one has to survive it and keep going; the crash is a
/// verdict, not the end of the run.
pub fn without_panicking<T>(body: impl FnOnce() -> T) -> Option<T> {
    let guard = PANIC_HOOK.lock().unwrap_or_else(|error| error.into_inner());
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = catch_unwind(AssertUnwindSafe(body));
    std::panic::set_hook(previous);
    drop(guard);
    outcome.ok()
}

/// Rewrite every separator rossi accepts to a plain space, leaving newlines
/// and everything else alone.
///
/// Takes whitespace out of a cross-tool comparison: rossi's separator set is
/// Rodin's, still wider than Camille's `layout_char` and once wider than the
/// tree-sitter grammar's too, so without this a whitespace disagreement is
/// indistinguishable from a grammar disagreement. Newlines survive because
/// structure and `//` comments are line-terminated.
pub fn normalize_whitespace(text: &str) -> String {
    text.replace(
        |c: char| c != '\n' && rossi::keywords::is_whitespace(c),
        " ",
    )
}

/// Whether `text` holds a separator [`normalize_whitespace`] would rewrite.
///
/// Allocation-free, so the report can count every input without building a
/// normalised copy of each one.
pub fn has_exotic_separator(text: &str) -> bool {
    text.chars()
        .any(|c| c != '\n' && c != ' ' && rossi::keywords::is_whitespace(c))
}
