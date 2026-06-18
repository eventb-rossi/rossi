//! Separator handling in the structural list clauses (SEES, EXTENDS, SETS,
//! CONSTANTS, VARIABLES, event ANY).
//!
//! Commit 1 (this revision) characterises the *current* behaviour: the comma in
//! these clauses is optional, so the comma and whitespace spellings parse to the
//! same AST. The follow-set SSOT refactor must preserve exactly this. A later
//! commit tightens the grammar to whitespace-only and flips the comma cases to
//! rejection.

use rossi::*;

/// Both spellings must parse to the same logical content. Compared through the
/// pretty-printer, which normalises formatting and carries no spans, so the
/// one-byte shift the comma introduces does not leak into the comparison.
fn assert_same_render(whitespace: &str, comma: &str) {
    let ws = to_string(
        &parse(whitespace)
            .unwrap_or_else(|e| panic!("whitespace form must parse: {e:?}\n{whitespace}")),
    );
    let cm = to_string(
        &parse(comma).unwrap_or_else(|e| panic!("comma form must parse: {e:?}\n{comma}")),
    );
    assert_eq!(
        ws, cm,
        "comma and whitespace forms must yield the same component"
    );
}

#[test]
fn context_extends_comma_equals_whitespace() {
    assert_same_render("CONTEXT c EXTENDS a b END", "CONTEXT c EXTENDS a, b END");
}

#[test]
fn context_sets_comma_equals_whitespace() {
    assert_same_render("CONTEXT c SETS S T END", "CONTEXT c SETS S, T END");
}

#[test]
fn context_constants_comma_equals_whitespace() {
    assert_same_render(
        "CONTEXT c CONSTANTS a b END",
        "CONTEXT c CONSTANTS a, b END",
    );
}

#[test]
fn machine_sees_comma_equals_whitespace() {
    assert_same_render("MACHINE m SEES c1 c2 END", "MACHINE m SEES c1, c2 END");
}

#[test]
fn machine_variables_comma_equals_whitespace() {
    assert_same_render(
        "MACHINE m VARIABLES x y END",
        "MACHINE m VARIABLES x, y END",
    );
}

#[test]
fn event_any_comma_equals_whitespace() {
    assert_same_render(
        "MACHINE m EVENTS EVENT e ANY a b END END",
        "MACHINE m EVENTS EVENT e ANY a, b END END",
    );
}
