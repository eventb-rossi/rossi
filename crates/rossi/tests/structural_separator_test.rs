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
fn parallel_assignment_requires_matching_target_and_expression_counts() {
    for (src, targets, expressions, operator) in [
        ("x, y := 1", 2, 1, ":="),
        ("x := 1, 2", 1, 2, ":="),
        ("x, y ≔ 1", 2, 1, "≔"),
        ("x ≔ 1, 2", 1, 2, "≔"),
    ] {
        let error = parse_action_str(src)
            .expect_err("mismatched parallel assignment must not produce an action");
        let (actual_targets, actual_expressions, line, column, span) = match error {
            ParseError::AssignmentArityMismatch {
                targets,
                expressions,
                line,
                column,
                span: Some(span),
            } => (targets, expressions, line, column, span),
            other => panic!("expected assignment arity error for {src:?}, got {other:?}"),
        };
        assert_eq!((actual_targets, actual_expressions), (targets, expressions));
        assert_eq!(line, 1);
        assert_eq!(column, src[..span.start].chars().count() + 1);
        assert_eq!(&src[span.start..span.end], operator);
    }
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
