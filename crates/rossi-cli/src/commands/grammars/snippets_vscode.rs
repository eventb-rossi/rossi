//! VS Code snippet library (`editors/vscode/snippets/eventb.json`).
//!
//! Whole-file: a VS Code snippet file is pure snippet data, so the entire file
//! is generated from [`rossi::snippets::SNIPPETS`]. The same VS Code snippet
//! format is reused by the Neovim LuaSnip loader (see
//! [`super::snippets_nvim`]), so the JSON body builder lives here and is shared.

use rossi::snippets::{SNIPPETS, Snippet};

/// Render the complete `editors/vscode/snippets/eventb.json` document.
pub fn render() -> String {
    render_vscode_json(SNIPPETS)
}

/// Render a VS Code snippet object as pretty-printed JSON, matching
/// `serde_json::to_string_pretty` formatting (2-space indent, `": "` separators)
/// but with a deterministic, declaration-ordered layout: the snippet objects
/// keep table order and each carries `prefix`, `body`, `description` in that
/// order (`serde_json`'s default `Map` would sort keys alphabetically, which is
/// not the canonical VS Code layout). String escaping is delegated to
/// `serde_json::to_string` on each `&str` so it is always correct.
pub fn render_vscode_json(snippets: &[Snippet]) -> String {
    let mut out = String::new();
    out.push_str("{\n");
    for (i, snippet) in snippets.iter().enumerate() {
        let comma = if i + 1 == snippets.len() { "" } else { "," };
        out.push_str(&format!("  {}: {{\n", json_str(snippet.name)));
        out.push_str(&format!("    \"prefix\": {},\n", json_str(snippet.prefix)));
        out.push_str("    \"body\": [\n");
        for (j, line) in snippet.body.iter().enumerate() {
            let line_comma = if j + 1 == snippet.body.len() { "" } else { "," };
            out.push_str(&format!("      {}{}\n", json_str(line), line_comma));
        }
        out.push_str("    ],\n");
        out.push_str(&format!(
            "    \"description\": {}\n",
            json_str(snippet.description)
        ));
        out.push_str(&format!("  }}{comma}\n"));
    }
    out.push_str("}\n");
    out
}

/// A JSON string literal (with surrounding quotes), escaped exactly as
/// `serde_json` would. Never fails for a `&str`.
fn json_str(s: &str) -> String {
    serde_json::to_string(s).expect("a &str always serializes to JSON")
}
