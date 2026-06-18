//! Separator handling in the structural list clauses (EXTENDS, SETS,
//! CONSTANTS, SEES, VARIABLES, event ANY).
//!
//! Declared identifiers and component references are separated by whitespace,
//! never by a comma — that mirrors how the real Event-B text tools work, and
//! how Rodin stores each as its own model element. A comma is meaningful only
//! inside formulas (set extension, quantifier lists, `partition`, parallel
//! assignment, function-update sets), where it stays a separator.

use rossi::*;

/// Every clause spelled with a comma between items must fail to parse.
const COMMA_FORMS: &[&str] = &[
    "CONTEXT c EXTENDS a, b END",
    "CONTEXT c SETS S, T END",
    "CONTEXT c CONSTANTS a, b END",
    "MACHINE m SEES c1, c2 END",
    "MACHINE m VARIABLES x, y END",
    "MACHINE m EVENTS EVENT e ANY a, b END END",
];

/// The same clauses with whitespace separation must parse.
const WHITESPACE_FORMS: &[&str] = &[
    "CONTEXT c EXTENDS a b END",
    "CONTEXT c SETS S T END",
    "CONTEXT c CONSTANTS a b END",
    "MACHINE m SEES c1 c2 END",
    "MACHINE m VARIABLES x y END",
    "MACHINE m EVENTS EVENT e ANY a b END END",
];

#[test]
fn comma_rejected_in_every_structural_clause() {
    for src in COMMA_FORMS {
        assert!(
            parse(src).is_err(),
            "a comma must not separate structural list items: {src}"
        );
    }
}

#[test]
fn whitespace_accepted_in_every_structural_clause() {
    for src in WHITESPACE_FORMS {
        parse(src).unwrap_or_else(|e| panic!("whitespace form must parse: {e:?}\n{src}"));
    }
}

#[test]
fn newline_and_tab_separate_structural_lists() {
    // "Whitespace" is any run of space / tab / CR / newline, so the common
    // one-identifier-per-line block and tab separation parse identically.
    for src in [
        "CONTEXT c\nSETS\n    S\n    T\nEND",
        "CONTEXT c\nCONSTANTS\n\ta\n\tb\nEND",
        "MACHINE m\nVARIABLES\n    x\n    y\nEND",
        "MACHINE m\nEVENTS\n    EVENT e\n    ANY\n        a\n        b\n    END\nEND",
    ] {
        parse(src).unwrap_or_else(|e| panic!("multi-line form must parse: {e:?}\n{src}"));
    }
}

#[test]
fn commas_still_parse_inside_formulas() {
    // Set extension, quantifier ident-list, partition, function-update set.
    for src in ["{a, b, c}", "f{x ↦ y, u ↦ v}"] {
        parse_expression_str(src)
            .unwrap_or_else(|e| panic!("formula comma must parse: {e:?}\n{src}"));
    }
    for src in ["∀x, y · x = y", "partition(S, a, b)"] {
        parse_predicate_str(src)
            .unwrap_or_else(|e| panic!("formula comma must parse: {e:?}\n{src}"));
    }
    // Parallel assignment: comma separates both targets and values.
    parse_action_str("x, y := 1, 2")
        .unwrap_or_else(|e| panic!("parallel assignment must parse: {e:?}"));
}

#[test]
fn any_parameters_round_trip_without_commas() {
    let machine = parse("MACHINE m EVENTS EVENT e ANY p q r END END").unwrap();
    let printed = to_string(&machine);
    assert!(
        printed.contains("p q r") && !printed.contains("p, q"),
        "ANY parameters must print whitespace-separated:\n{printed}"
    );
    parse(&printed).unwrap_or_else(|e| panic!("pretty output must reparse: {e:?}\n{printed}"));
}
