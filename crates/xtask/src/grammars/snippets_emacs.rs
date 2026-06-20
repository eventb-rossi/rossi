//! Emacs snippet library (yasnippet, `editors/emacs/snippets/eventb-mode/`).
//!
//! Multi-file: yasnippet stores one snippet per file under a per-mode directory
//! (`snippets/<major-mode>/<key>`). We emit one file per
//! [`rossi::snippets::SNIPPETS`] entry, each with the standard yasnippet header
//! (`# -*- mode: snippet -*-` / `# name:` / `# key:` / `# --`) followed by the
//! body.
//!
//! The file is named after the snippet's `key` (its trigger prefix): prefixes
//! are unique and filesystem-safe, which is the idiomatic yasnippet convention
//! (the human-readable name lives in the `# name:` header). yasnippet's field
//! syntax (`${1:default}`, `$0`) is the same as VS Code's tabstop syntax, so the
//! canonical bodies pass through unchanged.

use rossi::snippets::SNIPPETS;

use super::paths;

/// Render the `(relative path, content)` pairs, one yasnippet file per snippet.
pub fn render() -> Vec<(String, String)> {
    SNIPPETS
        .iter()
        .map(|snippet| {
            let rel = format!("{}/{}", paths::EMACS_SNIPPETS_DIR, snippet.prefix);
            (rel, render_one(snippet.name, snippet.prefix, snippet.body))
        })
        .collect()
}

/// A single yasnippet file: header block then the body, joined with newlines and
/// terminated with a trailing newline.
fn render_one(name: &str, key: &str, body: &[&str]) -> String {
    let mut out = String::new();
    out.push_str("# -*- mode: snippet -*-\n");
    out.push_str(&format!("# name: {name}\n"));
    out.push_str(&format!("# key: {key}\n"));
    out.push_str("# --\n");
    out.push_str(&body.join("\n"));
    out.push('\n');
    out
}
