//! Edge case tests covering AST variants with zero or weak existing coverage.

mod common;

use rossi::formula::tag::{AssocExprOp, BinaryExprOp, QuantExprOp, QuantPredOp, UnaryExprOp};
use rossi::{
    AssignmentKind, EventStatus, ExpressionKind, ParseError, PredicateKind, parse,
    parse_action_str, parse_expression_str, parse_predicate_str, to_string,
};
use test_case::test_case;

// ============================================================================
// HIGH priority: Action::BecomesIn
// ============================================================================

// Both spellings of becomes-in — Unicode `:∈` and ASCII `::` — must parse to
// the same BecomesIn action.
#[test_case(":∈" ; "unicode")]
#[test_case("::" ; "ascii")]
fn test_becomes_in(op: &str) {
    let source = format!(
        r#"
    MACHINE test
    VARIABLES
        x
    EVENTS
        EVENT INITIALISATION
        THEN
            @act1 x := 0
        END

        EVENT choose
        THEN
            @act1 x {op} {{1, 2, 3}}
        END
    END
    "#
    );

    let m = common::parse_machine(&source);
    let event = &m.events[0];
    assert_eq!(event.actions.len(), 1);
    match event.actions[0].action.kind() {
        AssignmentKind::BecomesMemberOf { idents, set } => {
            assert!(matches!(idents[0].kind(), ExpressionKind::FreeIdentifier(n) if n == "x"));
            assert!(
                matches!(set.kind(), ExpressionKind::SetExtension(_)),
                "Expected SetExtension, got {:?}",
                set
            );
        }
        other => panic!("Expected BecomesMemberOf, got {:?}", other),
    }
}

// ============================================================================
// HIGH priority: Action::BecomesSuchThat
// ============================================================================

#[test]
fn test_becomes_such_that() {
    let source = r#"
    MACHINE test
    VARIABLES
        x
    EVENTS
        EVENT INITIALISATION
        THEN
            @act1 x := 0
        END

        EVENT pick
        THEN
            @act1 x :| x > 0
        END
    END
    "#;

    let m = common::parse_machine(source);
    let event = &m.events[0];
    assert_eq!(event.actions.len(), 1);
    match event.actions[0].action.kind() {
        AssignmentKind::BecomesSuchThat { idents, pred, .. } => {
            assert!(matches!(idents[0].kind(), ExpressionKind::FreeIdentifier(n) if n == "x"));
            assert!(
                matches!(pred.kind(), PredicateKind::Relational { .. }),
                "Expected Comparison predicate, got {:?}",
                pred
            );
        }
        other => panic!("Expected BecomesSuchThat, got {:?}", other),
    }
}

// ============================================================================
// HIGH priority: UnaryOp::Inverse
// ============================================================================

// Postfix ∼ (U+223C) is the only spec-defined form; the ASCII ~ (U+007E)
// must parse identically to it.
#[test_case("r = f\u{223C}" ; "unicode")]
#[test_case("r = f~" ; "ascii")]
fn test_inverse_tilde(axiom: &str) {
    let source = common::axiom_context("f, r", axiom);
    let rhs = common::parse_axiom_rhs(&source);
    match rhs.kind() {
        ExpressionKind::Unary { op, child } => {
            assert_eq!(*op, UnaryExprOp::Converse);
            assert!(matches!(child.kind(), ExpressionKind::FreeIdentifier(n) if n == "f"));
        }
        other => panic!("Expected Unary Inverse, got {:?}", other),
    }
}

#[test]
fn test_inverse_repeated() {
    // r∼∼ should parse as (r∼)∼
    let source = common::axiom_context("r, s", "s = r\u{223C}\u{223C}");
    let rhs = common::parse_axiom_rhs(&source);
    match rhs.kind() {
        ExpressionKind::Unary {
            op: UnaryExprOp::Converse,
            child,
        } => match child.kind() {
            ExpressionKind::Unary {
                op: UnaryExprOp::Converse,
                child: inner,
            } => {
                assert!(matches!(inner.kind(), ExpressionKind::FreeIdentifier(n) if n == "r"));
            }
            other => panic!("Expected nested Inverse, got {:?}", other),
        },
        other => panic!("Expected Unary Inverse, got {:?}", other),
    }
}

#[test]
fn test_inverse_relational_image() {
    // r∼[S] should parse as (r∼)[S]
    let source = common::axiom_context("r, S, T", "T = r\u{223C}[S]");
    let rhs = common::parse_axiom_rhs(&source);
    assert!(
        matches!(rhs.kind(), ExpressionKind::Binary { op: BinaryExprOp::RelImage, left: relation, .. }
            if matches!(relation.kind(), ExpressionKind::Unary { op: UnaryExprOp::Converse, .. })),
        "Expected RelationalImage with Inverse relation, got {:?}",
        rhs
    );
}

#[test]
fn test_inverse_function_application() {
    // f∼(x) should parse as (f∼)(x)
    let source = common::axiom_context("f, x, y", "y = f\u{223C}(x)");
    let rhs = common::parse_axiom_rhs(&source);
    assert!(
        matches!(rhs.kind(), ExpressionKind::Binary { op: BinaryExprOp::FunImage, left: function, .. }
            if matches!(function.kind(), ExpressionKind::Unary { op: UnaryExprOp::Converse, .. })),
        "Expected FunctionApplication with Inverse function, got {:?}",
        rhs
    );
}

// ============================================================================
// HIGH priority: BinaryOp::Semicolon (forward composition)
// ============================================================================

#[test]
fn test_forward_composition() {
    // In expression context (not action), semicolon is forward composition
    let source = common::axiom_context("f, g, r", "r = f ; g");
    let rhs = common::parse_axiom_rhs(&source);
    match rhs.kind() {
        ExpressionKind::Associative { op, children } => {
            assert_eq!(*op, AssocExprOp::FComp);
            assert!(matches!(children[0].kind(), ExpressionKind::FreeIdentifier(n) if n == "f"));
            assert!(matches!(children[1].kind(), ExpressionKind::FreeIdentifier(n) if n == "g"));
        }
        other => panic!("Expected forward composition, got {:?}", other),
    }
}

#[test]
fn test_forward_composition_parenthesized_in_action() {
    // In an action RHS, forward composition must be parenthesized
    let source = r#"
    MACHINE test
    VARIABLES
        x
    EVENTS
        EVENT INITIALISATION
        THEN
            @act1 x := 0
        END

        EVENT apply
        THEN
            @act1 x := (f ; g)
        END
    END
    "#;

    let m = common::parse_machine(source);
    let event = &m.events[0];
    assert_eq!(event.actions.len(), 1);
    match event.actions[0].action.kind() {
        AssignmentKind::BecomesEqualTo { idents, values } => {
            assert_eq!(idents.len(), 1);
            assert!(matches!(idents[0].kind(), ExpressionKind::FreeIdentifier(n) if n == "x"));
            // The parenthesized (f ; g) should parse as forward composition
            assert!(
                matches!(
                    values[0].kind(),
                    ExpressionKind::Associative {
                        op: AssocExprOp::FComp,
                        ..
                    }
                ),
                "Expected forward composition in parens, got {:?}",
                values[0]
            );
        }
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_standalone_action_forward_composition_unparenthesized() {
    // A standalone action string (one action, as in a Rodin XML assignment
    // attribute) has no following action to separate, so a bare semicolon
    // is forward composition.
    let assignment = rossi::parse_action_str("x ≔ f;g").expect("standalone action parses");
    let AssignmentKind::BecomesEqualTo { idents, values } = assignment.kind() else {
        panic!("Expected becomes-equal-to, got {assignment:?}");
    };
    assert_eq!(idents.len(), 1);
    assert!(matches!(idents[0].kind(), ExpressionKind::FreeIdentifier(n) if n == "x"));
    let ExpressionKind::Associative { op, children } = values[0].kind() else {
        panic!("Expected composition, got {:?}", values[0]);
    };
    assert_eq!(*op, AssocExprOp::FComp);
    assert!(matches!(children[0].kind(), ExpressionKind::FreeIdentifier(n) if n == "f"));
    assert!(matches!(children[1].kind(), ExpressionKind::FreeIdentifier(n) if n == "g"));
}

#[test]
fn test_standalone_action_chained_composition_with_inverse() {
    // Left-associative chain mixing inverse and a parenthesized set
    // expression: h∼;(s ∪ t);h parses as (h∼;(s ∪ t));h.
    let assignment = rossi::parse_action_str("x ≔ h∼;(s ∪ t);h").expect("standalone action parses");
    let AssignmentKind::BecomesEqualTo { values, .. } = assignment.kind() else {
        panic!("Expected becomes-equal-to, got {assignment:?}");
    };
    // The chain flattens into one n-ary composition: h∼ ; (s ∪ t) ; h.
    let ExpressionKind::Associative { op, children } = values[0].kind() else {
        panic!("Expected composition, got {:?}", values[0]);
    };
    assert_eq!(*op, AssocExprOp::FComp);
    assert_eq!(children.len(), 3);
    assert!(matches!(
        children[0].kind(),
        ExpressionKind::Unary {
            op: UnaryExprOp::Converse,
            ..
        }
    ));
    assert!(matches!(children[2].kind(), ExpressionKind::FreeIdentifier(n) if n == "h"));
}

#[test]
fn test_standalone_becomes_such_that_with_composition() {
    let assignment = rossi::parse_action_str("x :∣ x' = f;g").expect("standalone action parses");
    let AssignmentKind::BecomesSuchThat { pred, .. } = assignment.kind() else {
        panic!("Expected becomes-such-that, got {assignment:?}");
    };
    let PredicateKind::Relational { right, .. } = pred.kind() else {
        panic!("Expected Comparison, got {:?}", pred);
    };
    assert!(matches!(
        right.kind(),
        ExpressionKind::Associative {
            op: AssocExprOp::FComp,
            ..
        }
    ));
}

// ============================================================================
// HIGH priority: EventStatus::Anticipated
// ============================================================================

#[test]
fn test_anticipated_event() {
    let source = r#"
    MACHINE test
    VARIABLES
        x
    VARIANT
        x
    EVENTS
        EVENT INITIALISATION
        THEN
            @act1 x := 10
        END

        EVENT step
        STATUS anticipated
        WHERE
            @grd1 x > 0
        THEN
            @act1 x := x - 1
        END
    END
    "#;

    let m = common::parse_machine(source);
    assert_eq!(m.events.len(), 1);
    assert_eq!(m.events[0].status, Some(EventStatus::Anticipated));
}

// ============================================================================
// MEDIUM priority: BinaryOp::Composition via circ
// ============================================================================

#[test]
fn test_composition_circ_ascii() {
    let source = common::axiom_context("f, g, r", "r = f circ g");
    let rhs = common::parse_axiom_rhs(&source);
    assert!(
        matches!(
            rhs.kind(),
            ExpressionKind::Associative {
                op: AssocExprOp::BComp,
                ..
            }
        ),
        "Expected Composition via 'circ', got {:?}",
        rhs
    );
}

// ============================================================================
// MEDIUM priority: Quantifiers with multiple variables
// ============================================================================

#[test_case("∀x, y · x > 0 ∧ y > 0 ⇒ x + y > 0", QuantPredOp::Forall ; "forall")]
#[test_case("∃x, y · x > 0 ∧ y > 0", QuantPredOp::Exists ; "exists")]
fn test_quantifier_multiple_variables(axiom: &str, expected: QuantPredOp) {
    let source = format!("\n    CONTEXT test\n    AXIOMS\n        @axm1 {axiom}\n    END\n    ");

    let ctx = common::parse_context(&source);
    match ctx.axioms[0].predicate.kind() {
        PredicateKind::Quantified { op, decls, pred } => {
            assert_eq!(*op, expected);
            let names: Vec<&str> = decls.iter().map(|d| d.name()).collect();
            assert_eq!(names, ["x", "y"]);
            assert!(
                matches!(
                    pred.kind(),
                    PredicateKind::Binary { .. } | PredicateKind::Associative { .. }
                ),
                "Expected a compound predicate body, got {:?}",
                pred
            );
        }
        other => panic!("Expected Quantified {expected:?}, got {:?}", other),
    }
}

#[test]
fn test_nested_quantifiers() {
    let source = r#"
    CONTEXT test
    AXIOMS
        @axm1 ∀x · (∃y · x + y = 0)
    END
    "#;

    let ctx = common::parse_context(source);
    match ctx.axioms[0].predicate.kind() {
        PredicateKind::Quantified { op, decls, pred } => {
            assert_eq!(*op, QuantPredOp::Forall);
            let names: Vec<&str> = decls.iter().map(|d| d.name()).collect();
            assert_eq!(names, ["x"]);
            assert!(
                matches!(
                    pred.kind(),
                    PredicateKind::Quantified {
                        op: QuantPredOp::Exists,
                        ..
                    }
                ),
                "Expected Quantified Exists inside ForAll, got {:?}",
                pred
            );
        }
        other => panic!("Expected Quantified ForAll, got {:?}", other),
    }
}

// ============================================================================
// MEDIUM priority: Lambda with ident-pattern
// ============================================================================

#[test]
fn test_lambda_maplet_pattern() {
    let source = r#"
    CONTEXT test
    CONSTANTS
        f
    AXIOMS
        @axm1 f = λx ↦ y · x ∈ ℕ ∧ y ∈ ℕ ∣ x + y
    END
    "#;

    let ctx = common::parse_context(source);
    if let PredicateKind::Relational { right, .. } = ctx.axioms[0].predicate.kind() {
        match right.kind() {
            ExpressionKind::Quantified {
                op: QuantExprOp::CSet,
                decls,
                pred,
                expr,
                form: rossi::Form::Lambda,
            } => {
                let names: Vec<&str> = decls.iter().map(|d| d.name()).collect();
                assert_eq!(names, ["x", "y"]);
                // The value is `pattern ↦ body`; the pattern is `x ↦ y`.
                let ExpressionKind::Binary {
                    op: BinaryExprOp::Mapsto,
                    left: pattern,
                    right: body,
                } = expr.kind()
                else {
                    panic!("Expected the lambda value pair, got {:?}", expr);
                };
                assert!(matches!(
                    pattern.kind(),
                    ExpressionKind::Binary {
                        op: BinaryExprOp::Mapsto,
                        ..
                    }
                ));
                assert!(matches!(pred.kind(), PredicateKind::Associative { .. }));
                assert!(matches!(body.kind(), ExpressionKind::Associative { .. }));
            }
            other => panic!("Expected Lambda, got {:?}", other),
        }
    } else {
        panic!("Expected Comparison predicate");
    }
}

#[test]
fn test_lambda_parenthesized_maplet_pattern() {
    // This is a real-world corpus pattern that originally failed
    let source = r#"
    CONTEXT test
    CONSTANTS
        DIST
    AXIOMS
        @axm1 DIST = λ(x↦y) · x ∈ ℤ ∧ y ∈ ℤ ∣ max({y − x, x − y})
    END
    "#;

    let ctx = common::parse_context(source);
    if let PredicateKind::Relational { right, .. } = ctx.axioms[0].predicate.kind() {
        match right.kind() {
            ExpressionKind::Quantified {
                op: QuantExprOp::CSet,
                decls,
                form: rossi::Form::Lambda,
                ..
            } => {
                let names: Vec<&str> = decls.iter().map(|d| d.name()).collect();
                assert_eq!(names, ["x", "y"]);
            }
            other => panic!("Expected Lambda, got {:?}", other),
        }
    } else {
        panic!("Expected Comparison predicate");
    }
}

#[test]
fn test_lambda_triple_maplet_left_assoc() {
    let source = r#"
    CONTEXT test
    CONSTANTS
        f
    AXIOMS
        @axm1 f = λx ↦ y ↦ z · x ∈ ℤ ∣ x
    END
    "#;

    let ctx = common::parse_context(source);
    if let PredicateKind::Relational { right, .. } = ctx.axioms[0].predicate.kind() {
        match right.kind() {
            ExpressionKind::Quantified {
                op: QuantExprOp::CSet,
                decls,
                expr,
                form: rossi::Form::Lambda,
                ..
            } => {
                let names: Vec<&str> = decls.iter().map(|d| d.name()).collect();
                assert_eq!(names, ["x", "y", "z"]);
                // The value's pattern side is left-assoc: (x ↦ y) ↦ z.
                let ExpressionKind::Binary {
                    op: BinaryExprOp::Mapsto,
                    left: pattern,
                    ..
                } = expr.kind()
                else {
                    panic!("Expected the lambda value pair, got {:?}", expr);
                };
                let ExpressionKind::Binary {
                    op: BinaryExprOp::Mapsto,
                    left: inner,
                    ..
                } = pattern.kind()
                else {
                    panic!("Expected the outer pattern maplet, got {:?}", pattern);
                };
                assert!(matches!(
                    inner.kind(),
                    ExpressionKind::Binary {
                        op: BinaryExprOp::Mapsto,
                        ..
                    }
                ));
            }
            other => panic!("Expected Lambda, got {:?}", other),
        }
    } else {
        panic!("Expected Comparison predicate");
    }
}

// ============================================================================
// MEDIUM priority: Set comprehension with multiple variables
// ============================================================================

#[test]
fn test_set_comprehension_multiple_vars() {
    let source = common::invariant_machine("s", "s = {x, y · x ∈ ℕ ∧ y ∈ ℕ | x + y}");
    let m = common::parse_machine(&source);
    if let PredicateKind::Relational { right, .. } = m.invariants[0].predicate.kind() {
        match right.kind() {
            ExpressionKind::Quantified {
                op: QuantExprOp::CSet,
                decls,
                pred,
                form: rossi::Form::Explicit,
                ..
            } => {
                let names: Vec<&str> = decls.iter().map(|d| d.name()).collect();
                assert_eq!(names, ["x", "y"]);
                assert!(matches!(pred.kind(), PredicateKind::Associative { .. }));
            }
            other => panic!("Expected SetComprehension, got {:?}", other),
        }
    } else {
        panic!("Expected Comparison predicate");
    }
}

// ============================================================================
// Primed identifiers (after-state variables like x')
// ============================================================================

#[test]
fn test_primed_identifier_in_becomes_such_that() {
    let source = std::fs::read_to_string("examples/refinement_abstract.eventb")
        .expect("Failed to read refinement_abstract.eventb");

    let m = common::parse_machine(&source);
    // Find the "decrease" event
    let decrease = m
        .events
        .iter()
        .find(|e| e.name == "decrease")
        .expect("Expected 'decrease' event");
    assert_eq!(decrease.actions.len(), 1);
    match decrease.actions[0].action.kind() {
        AssignmentKind::BecomesSuchThat {
            idents,
            pred,
            primed,
        } => {
            assert!(
                matches!(idents[0].kind(), ExpressionKind::FreeIdentifier(n) if n == "abstract_state")
            );
            // The condition binds the primed after-state declarations:
            // walking the whole assignment resolves `abstract_state'`
            // reads to the primed declaration.
            fn contains_primed_ident(assignment: &rossi::Assignment) -> bool {
                use std::ops::ControlFlow;
                struct Finder(bool);
                impl rossi::formula::occurrences::OccurrenceVisitor for Finder {
                    fn visit(
                        &mut self,
                        occ: rossi::formula::occurrences::Occurrence<'_>,
                    ) -> ControlFlow<()> {
                        if occ.role == rossi::formula::occurrences::Role::Usage
                            && occ.name.ends_with('\'')
                        {
                            self.0 = true;
                            return ControlFlow::Break(());
                        }
                        ControlFlow::Continue(())
                    }
                }
                let mut finder = Finder(false);
                let _ = rossi::formula::occurrences::walk_assignment(
                    assignment,
                    &mut Vec::new(),
                    &mut finder,
                );
                finder.0
            }
            assert_eq!(primed.len(), 1);
            assert_eq!(primed[0].name(), "abstract_state'");
            let _ = pred;
            assert!(
                contains_primed_ident(&decrease.actions[0].action),
                "Expected the condition to bind the primed after-state"
            );
        }
        other => panic!("Expected BecomesSuchThat, got {:?}", other),
    }
}

// ============================================================================
// Regression: a `VARIANT` whose expression starts with `(`, sitting right
// after an `INVARIANTS`/`THEOREMS` section, must parse and round-trip.
//
// Before the fix, the section's `(labeled_predicate)*` repetition was
// unguarded: it speculatively parsed the following `VARIANT` keyword as a
// permissive `identifier`, absorbed the `(…)` as a function-application
// argument, and only failed at `EVENTS`. The follow-set guard
// (`!machine_section_kw` / `!context_section_kw`) stops the list at the
// section boundary — the textual counterpart of how Camille/ProB/CamilleX
// bound a formula against structural keywords.
//
// Originally surfaced by the import→re-parse round-trip on corpus models
// whose machines declare such a variant (see rossi-build `import_corpus`).
// ============================================================================

/// `parse` succeeds and `print → re-parse → re-print` is byte-stable — the
/// same property `import_corpus` checks against the corpus.
fn assert_stable_roundtrip(source: &str) {
    let component = parse(source).expect("parse should succeed");
    let printed = to_string(&component);
    let reparsed = parse(&printed).expect("printed form must re-parse");
    assert_eq!(
        printed,
        to_string(&reparsed),
        "round-trip must be byte-stable"
    );
}

#[test]
fn variant_paren_expr_after_invariants_roundtrips() {
    assert_stable_roundtrip(
        r#"MACHINE m
VARIABLES
    a
    b
    c
INVARIANTS
    @inv1 a ∈ ℙ(b)
VARIANT
    (a × b) ∖ c
EVENTS
    EVENT INITIALISATION
    THEN
        @act1 a ≔ a
    END
END
"#,
    );
}

#[test]
fn variant_paren_expr_after_theorems_roundtrips() {
    assert_stable_roundtrip(
        r#"MACHINE m
VARIABLES
    a
    b
    c
THEOREMS
    @thm1 a ∈ ℙ(b)
VARIANT
    (a × b) ∖ c
EVENTS
    EVENT INITIALISATION
    THEN
        @act1 a ≔ a
    END
END
"#,
    );
}

#[test]
fn context_theorems_paren_predicate_after_axioms_parses() {
    // Symmetric context case: a `THEOREMS` section whose first predicate is a
    // bare parenthesized predicate, following an `AXIOMS` section. Before the
    // `!context_section_kw` guard, the AXIOMS `(labeled_predicate)*` swallowed
    // the `THEOREMS` keyword + `(…)` and failed at `END`.
    let source = r#"CONTEXT c
SETS
    S
CONSTANTS
    x
AXIOMS
    @axm1 x ∈ S
THEOREMS
    @thm1 (x ∈ S)
END
"#;
    parse(source).expect("AXIOMS list must not swallow the following THEOREMS section");
}

// ============================================================================
// Structural separators (whitespace, not comma) — EXTENDS, SETS, CONSTANTS,
// SEES, VARIABLES, event ANY.
//
// Declared identifiers and component references are separated by whitespace,
// never by a comma — that mirrors how the real Event-B text tools work, and
// how Rodin stores each as its own model element. A comma is meaningful only
// inside formulas (set extension, quantifier lists, `partition`, parallel
// assignment, function-update sets), where it stays a separator.
// ============================================================================

/// Every clause spelled with a comma between items must fail to parse.
const COMMA_FORMS: &[&str] = &[
    "CONTEXT c EXTENDS a, b END",
    "CONTEXT c SETS S, T END",
    "CONTEXT c CONSTANTS a, b END",
    "MACHINE m SEES c1, c2 END",
    "MACHINE m VARIABLES x, y END",
    "MACHINE m EVENTS EVENT e ANY a, b END END",
];

/// The same clauses with whitespace separation must parse. "Whitespace" is
/// any run of space / tab / CR / newline, so the common
/// one-identifier-per-line block and tab separation parse identically.
const WHITESPACE_FORMS: &[&str] = &[
    "CONTEXT c EXTENDS a b END",
    "CONTEXT c SETS S T END",
    "CONTEXT c CONSTANTS a b END",
    "MACHINE m SEES c1 c2 END",
    "MACHINE m VARIABLES x y END",
    "MACHINE m EVENTS EVENT e ANY a b END END",
    // Newline / tab separated multi-line forms.
    "CONTEXT c\nSETS\n    S\n    T\nEND",
    "CONTEXT c\nCONSTANTS\n\ta\n\tb\nEND",
    "MACHINE m\nVARIABLES\n    x\n    y\nEND",
    "MACHINE m\nEVENTS\n    EVENT e\n    ANY\n        a\n        b\n    END\nEND",
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

// ============================================================================
// Set comprehension: `{E ∣ P}` with nothing to bind
// ============================================================================

// The implicit form takes its declarations from the identifiers free in `E`.
// With none free there is nothing to bind, and the model cannot hold a
// quantifier with no declaration — so this has to be refused while parsing.
// Rodin refuses the same text with "Expression not binding any variable in
// quantified expression", reported on the expression part alone.
#[test]
fn implicit_comprehension_binding_nothing_is_rejected() {
    for (src, member) in [
        ("{2 ∣ ⊤}", "2"),
        ("{ℕ ∣ ⊤}", "ℕ"),
        ("{∅ ∣ ⊤}", "∅"),
        ("{{} ∣ ⊤}", "{}"),
        ("{1 ↦ 2 ∣ ⊤}", "1 ↦ 2"),
    ] {
        let error = parse_expression_str(src)
            .expect_err("a comprehension binding nothing must not produce an expression");
        let ParseError::ExpressionNotBinding {
            line,
            column,
            span: Some(span),
        } = error
        else {
            panic!("expected a not-binding error for {src:?}, got {error:?}");
        };
        // Every member here starts immediately after `{`, so the report lands
        // on `E` rather than on the brace.
        assert_eq!((line, column), (1, 2));
        assert_eq!(src[span.start..span.end].trim(), member);
    }
}

// The point of the guard: a component holding such an axiom is rejected, not
// aborted on. The parser is hosted by `rossi validate`, the language server and
// any library caller, none of which survive a panic. Checking which error comes
// out keeps this from passing for some unrelated reason.
#[test]
fn a_comprehension_binding_nothing_does_not_abort_the_parser() {
    let source = "context C\naxioms\n    @a {2 ∣ ⊤} = 1\nend\n";
    let error = parse(source).expect_err("the axiom must be rejected, not crash the parser");
    assert_eq!(error.position(), Some((3, 9)), "reported at {error:?}");
}

// A member naming only what an enclosing binder also spells declares a fresh,
// shadowing name rather than leaving nothing to bind, so it parses — this shape
// used to abort the parser for the same reason `{2 ∣ ⊤}` did.
// `implicit_set_builder_binds_every_name_its_member_writes` pins the
// declarations that result.
#[test]
fn a_comprehension_naming_only_an_enclosing_binder_parses() {
    parse_predicate_str("∀x·{x + 0 ∣ ⊤} ⊆ s")
        .expect("a member naming an enclosing binder must bind a shadowing name");
}

// The guard stays reachable under that reading: an enclosing binder no longer
// empties the declaration list, but an `E` naming no identifier at all still
// does, wherever it is written.
#[test]
fn a_comprehension_binding_nothing_is_rejected_under_a_quantifier() {
    let error = parse_predicate_str("∀x·{2 ∣ ⊤} ⊆ s")
        .expect_err("a comprehension binding nothing must not produce a predicate");
    assert!(
        matches!(error, ParseError::ExpressionNotBinding { .. }),
        "expected a not-binding error, got {error:?}"
    );
}
