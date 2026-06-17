//! Edge case tests covering AST variants with zero or weak existing coverage.

mod common;

use rossi::ast::expression::{BinaryOp, UnaryOp};
use rossi::ast::predicate::Quantifier;
use rossi::{ActionKind, EventStatus, Expression, ExpressionKind, Predicate, PredicateKind};

// ============================================================================
// HIGH priority: Action::BecomesIn
// ============================================================================

#[test]
fn test_becomes_in_unicode() {
    let source = r#"
    MACHINE test
    VARIABLES
        x
    EVENTS
        EVENT INITIALISATION
        THEN
            x := 0
        END

        EVENT choose
        THEN
            x :∈ {1, 2, 3}
        END
    END
    "#;

    let m = common::parse_machine(source);
    let event = &m.events[0];
    assert_eq!(event.actions.len(), 1);
    match &event.actions[0].action.kind {
        ActionKind::BecomesIn { variables, set } => {
            assert_eq!(variables, &["x"]);
            assert!(
                matches!(set.kind, ExpressionKind::SetEnumeration(_)),
                "Expected SetEnumeration, got {:?}",
                set
            );
        }
        other => panic!("Expected BecomesIn, got {:?}", other),
    }
}

#[test]
fn test_becomes_in_ascii() {
    let source = r#"
    MACHINE test
    VARIABLES
        x
    EVENTS
        EVENT INITIALISATION
        THEN
            x := 0
        END

        EVENT choose
        THEN
            x :: {1, 2, 3}
        END
    END
    "#;

    let m = common::parse_machine(source);
    let event = &m.events[0];
    assert_eq!(event.actions.len(), 1);
    match &event.actions[0].action.kind {
        ActionKind::BecomesIn { variables, set } => {
            assert_eq!(variables, &["x"]);
            assert!(
                matches!(set.kind, ExpressionKind::SetEnumeration(_)),
                "Expected SetEnumeration, got {:?}",
                set
            );
        }
        other => panic!("Expected BecomesIn, got {:?}", other),
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
            x := 0
        END

        EVENT pick
        THEN
            x :| x > 0
        END
    END
    "#;

    let m = common::parse_machine(source);
    let event = &m.events[0];
    assert_eq!(event.actions.len(), 1);
    match &event.actions[0].action.kind {
        ActionKind::BecomesSuchThat {
            variables,
            predicate,
        } => {
            assert_eq!(variables, &["x"]);
            assert!(
                matches!(predicate.kind, PredicateKind::Comparison { .. }),
                "Expected Comparison predicate, got {:?}",
                predicate
            );
        }
        other => panic!("Expected BecomesSuchThat, got {:?}", other),
    }
}

// ============================================================================
// HIGH priority: UnaryOp::Inverse
// ============================================================================

#[test]
fn test_inverse_tilde() {
    // Postfix ∼ (U+223C) — the only spec-defined form
    let source = common::axiom_context("f, r", "r = f\u{223C}");
    let rhs = common::parse_axiom_rhs(&source);
    match &rhs.kind {
        ExpressionKind::Unary { op, operand } => {
            assert_eq!(*op, UnaryOp::Inverse);
            assert!(matches!(&operand.kind, ExpressionKind::Identifier(n) if n == "f"));
        }
        other => panic!("Expected Unary Inverse, got {:?}", other),
    }
}

#[test]
fn test_inverse_repeated() {
    // r∼∼ should parse as (r∼)∼
    let source = common::axiom_context("r, s", "s = r\u{223C}\u{223C}");
    let rhs = common::parse_axiom_rhs(&source);
    match &rhs.kind {
        ExpressionKind::Unary {
            op: UnaryOp::Inverse,
            operand,
        } => match &operand.kind {
            ExpressionKind::Unary {
                op: UnaryOp::Inverse,
                operand: inner,
            } => {
                assert!(matches!(&inner.kind, ExpressionKind::Identifier(n) if n == "r"));
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
        matches!(&rhs.kind, ExpressionKind::RelationalImage { relation, .. }
            if matches!(&relation.kind, ExpressionKind::Unary { op: UnaryOp::Inverse, .. })),
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
        matches!(&rhs.kind, ExpressionKind::FunctionApplication { function, .. }
            if matches!(&function.kind, ExpressionKind::Unary { op: UnaryOp::Inverse, .. })),
        "Expected FunctionApplication with Inverse function, got {:?}",
        rhs
    );
}

#[test]
fn test_inverse_tilde_ascii() {
    // Postfix ASCII ~ (U+007E) must parse identically to the Unicode ∼ form.
    let source = common::axiom_context("f, r", "r = f~");
    let rhs = common::parse_axiom_rhs(&source);
    match &rhs.kind {
        ExpressionKind::Unary { op, operand } => {
            assert_eq!(*op, UnaryOp::Inverse);
            assert!(matches!(&operand.kind, ExpressionKind::Identifier(n) if n == "f"));
        }
        other => panic!("Expected Unary Inverse, got {:?}", other),
    }
}

// ============================================================================
// HIGH priority: BinaryOp::Semicolon (forward composition)
// ============================================================================

#[test]
fn test_forward_composition() {
    // In expression context (not action), semicolon is forward composition
    let source = common::axiom_context("f, g, r", "r = f ; g");
    let rhs = common::parse_axiom_rhs(&source);
    match &rhs.kind {
        ExpressionKind::Binary { op, left, right } => {
            assert_eq!(*op, BinaryOp::Semicolon);
            assert!(matches!(&left.kind, ExpressionKind::Identifier(n) if n == "f"));
            assert!(matches!(&right.kind, ExpressionKind::Identifier(n) if n == "g"));
        }
        other => panic!("Expected Binary Semicolon, got {:?}", other),
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
            x := 0
        END

        EVENT apply
        THEN
            x := (f ; g)
        END
    END
    "#;

    let m = common::parse_machine(source);
    let event = &m.events[0];
    assert_eq!(event.actions.len(), 1);
    match &event.actions[0].action.kind {
        ActionKind::Assignment {
            variables,
            expressions,
        } => {
            assert_eq!(variables, &["x"]);
            assert_eq!(expressions.len(), 1);
            // The parenthesized (f ; g) should parse as forward composition
            assert!(
                matches!(
                    &expressions[0].kind,
                    ExpressionKind::Binary {
                        op: BinaryOp::Semicolon,
                        ..
                    }
                ),
                "Expected Semicolon composition in parens, got {:?}",
                expressions[0]
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
    let action = rossi::parse_action_str("x ≔ f;g").expect("standalone action parses");
    let ActionKind::Assignment {
        variables,
        expressions,
    } = &action.kind
    else {
        panic!("Expected Assignment, got {:?}", action);
    };
    assert_eq!(variables, &["x"]);
    assert_eq!(expressions.len(), 1);
    let ExpressionKind::Binary { op, left, right } = &expressions[0].kind else {
        panic!("Expected Binary, got {:?}", expressions[0]);
    };
    assert_eq!(*op, BinaryOp::Semicolon);
    assert!(matches!(&left.kind, ExpressionKind::Identifier(n) if n == "f"));
    assert!(matches!(&right.kind, ExpressionKind::Identifier(n) if n == "g"));
}

#[test]
fn test_standalone_action_chained_composition_with_inverse() {
    // Left-associative chain mixing inverse and a parenthesized set
    // expression: h∼;(s ∪ t);h parses as (h∼;(s ∪ t));h.
    let action = rossi::parse_action_str("x ≔ h∼;(s ∪ t);h").expect("standalone action parses");
    let ActionKind::Assignment { expressions, .. } = &action.kind else {
        panic!("Expected Assignment, got {:?}", action);
    };
    let ExpressionKind::Binary { op, left, right } = &expressions[0].kind else {
        panic!("Expected Binary, got {:?}", expressions[0]);
    };
    assert_eq!(*op, BinaryOp::Semicolon);
    assert!(matches!(&right.kind, ExpressionKind::Identifier(n) if n == "h"));
    let ExpressionKind::Binary { op, left, .. } = &left.kind else {
        panic!("Expected nested Binary, got {:?}", left);
    };
    assert_eq!(*op, BinaryOp::Semicolon);
    assert!(matches!(
        &left.kind,
        ExpressionKind::Unary {
            op: UnaryOp::Inverse,
            ..
        }
    ));
}

#[test]
fn test_standalone_becomes_such_that_with_composition() {
    let action = rossi::parse_action_str("x :∣ x' = f;g").expect("standalone action parses");
    let ActionKind::BecomesSuchThat { predicate, .. } = &action.kind else {
        panic!("Expected BecomesSuchThat, got {:?}", action);
    };
    let PredicateKind::Comparison { right, .. } = &predicate.kind else {
        panic!("Expected Comparison, got {:?}", predicate);
    };
    assert!(matches!(
        right.kind,
        ExpressionKind::Binary {
            op: BinaryOp::Semicolon,
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
            x := 10
        END

        EVENT step
        STATUS anticipated
        WHERE
            @grd1 x > 0
        THEN
            x := x - 1
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
            &rhs.kind,
            ExpressionKind::Binary {
                op: BinaryOp::Composition,
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

#[test]
fn test_forall_multiple_variables() {
    let source = r#"
    CONTEXT test
    AXIOMS
        @axm1 ∀x, y · x > 0 ∧ y > 0 ⇒ x + y > 0
    END
    "#;

    let ctx = common::parse_context(source);
    match &ctx.axioms[0].predicate.kind {
        PredicateKind::Quantified {
            quantifier,
            identifiers,
            predicate,
        } => {
            assert_eq!(*quantifier, Quantifier::ForAll);
            assert_eq!(identifiers.len(), 2);
            assert_eq!(identifiers[0], "x");
            assert_eq!(identifiers[1], "y");
            assert!(
                matches!(&predicate.kind, PredicateKind::Logical { .. }),
                "Expected Logical predicate body, got {:?}",
                predicate
            );
        }
        other => panic!("Expected Quantified ForAll, got {:?}", other),
    }
}

#[test]
fn test_exists_multiple_variables() {
    let source = r#"
    CONTEXT test
    AXIOMS
        @axm1 ∃x, y · x > 0 ∧ y > 0
    END
    "#;

    let ctx = common::parse_context(source);
    match &ctx.axioms[0].predicate.kind {
        PredicateKind::Quantified {
            quantifier,
            identifiers,
            predicate,
        } => {
            assert_eq!(*quantifier, Quantifier::Exists);
            assert_eq!(identifiers.len(), 2);
            assert_eq!(identifiers[0], "x");
            assert_eq!(identifiers[1], "y");
            assert!(matches!(&predicate.kind, PredicateKind::Logical { .. }));
        }
        other => panic!("Expected Quantified Exists, got {:?}", other),
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
    match &ctx.axioms[0].predicate.kind {
        PredicateKind::Quantified {
            quantifier,
            identifiers,
            predicate,
        } => {
            assert_eq!(*quantifier, Quantifier::ForAll);
            assert_eq!(identifiers, &["x"]);
            assert!(
                matches!(
                    &predicate.kind,
                    PredicateKind::Quantified {
                        quantifier: Quantifier::Exists,
                        ..
                    }
                ),
                "Expected Quantified Exists inside ForAll, got {:?}",
                predicate
            );
        }
        other => panic!("Expected Quantified ForAll, got {:?}", other),
    }
}

// ============================================================================
// MEDIUM priority: @label form for guards and actions
// ============================================================================

#[test]
fn test_at_label_guards_and_actions() {
    let source = r#"
    MACHINE test
    VARIABLES
        x
    EVENTS
        EVENT INITIALISATION
        THEN
            x := 0
        END

        EVENT inc
        WHERE
            @grd1 x < 100
        THEN
            @act1 x := x + 1
        END
    END
    "#;

    let m = common::parse_machine(source);
    let event = &m.events[0];
    assert_eq!(event.guards.len(), 1);
    assert_eq!(
        event.guards[0].label.as_deref(),
        Some("grd1"),
        "Guard should have @-label"
    );
    assert_eq!(event.actions.len(), 1);
    assert_eq!(
        event.actions[0].label.as_deref(),
        Some("act1"),
        "Action should have @-label"
    );
}

// ============================================================================
// MEDIUM priority: Lambda with ident-pattern
// ============================================================================

#[test]
fn test_lambda_maplet_pattern() {
    use rossi::IdentPattern;

    let source = r#"
    CONTEXT test
    CONSTANTS
        f
    AXIOMS
        @axm1 f = λx ↦ y · x ∈ ℕ ∧ y ∈ ℕ ∣ x + y
    END
    "#;

    let ctx = common::parse_context(source);
    if let PredicateKind::Comparison { right, .. } = &ctx.axioms[0].predicate.kind {
        match &right.kind {
            ExpressionKind::Lambda {
                pattern,
                predicate,
                expression,
            } => {
                let ids = pattern.identifiers();
                assert_eq!(ids, vec!["x", "y"]);
                assert!(matches!(
                    pattern,
                    IdentPattern::Maplet(l, r)
                        if matches!(l.as_ref(), IdentPattern::Identifier(n) if n == "x")
                        && matches!(r.as_ref(), IdentPattern::Identifier(n) if n == "y")
                ));
                assert!(matches!(&predicate.kind, PredicateKind::Logical { .. }));
                assert!(matches!(&expression.kind, ExpressionKind::Binary { .. }));
            }
            other => panic!("Expected Lambda, got {:?}", other),
        }
    } else {
        panic!("Expected Comparison predicate");
    }
}

#[test]
fn test_lambda_parenthesized_maplet_pattern() {
    use rossi::IdentPattern;

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
    if let PredicateKind::Comparison { right, .. } = &ctx.axioms[0].predicate.kind {
        match &right.kind {
            ExpressionKind::Lambda { pattern, .. } => {
                let ids = pattern.identifiers();
                assert_eq!(ids, vec!["x", "y"]);
                assert!(matches!(
                    pattern,
                    IdentPattern::Maplet(l, r)
                        if matches!(l.as_ref(), IdentPattern::Identifier(n) if n == "x")
                        && matches!(r.as_ref(), IdentPattern::Identifier(n) if n == "y")
                ));
            }
            other => panic!("Expected Lambda, got {:?}", other),
        }
    } else {
        panic!("Expected Comparison predicate");
    }
}

#[test]
fn test_lambda_triple_maplet_left_assoc() {
    use rossi::IdentPattern;

    let source = r#"
    CONTEXT test
    CONSTANTS
        f
    AXIOMS
        @axm1 f = λx ↦ y ↦ z · x ∈ ℤ ∣ x
    END
    "#;

    let ctx = common::parse_context(source);
    if let PredicateKind::Comparison { right, .. } = &ctx.axioms[0].predicate.kind {
        match &right.kind {
            ExpressionKind::Lambda { pattern, .. } => {
                let ids = pattern.identifiers();
                assert_eq!(ids, vec!["x", "y", "z"]);
                // Left-assoc: (x ↦ y) ↦ z
                match pattern {
                    IdentPattern::Maplet(left, right) => {
                        assert!(matches!(right.as_ref(), IdentPattern::Identifier(n) if n == "z"));
                        match left.as_ref() {
                            IdentPattern::Maplet(ll, lr) => {
                                assert!(
                                    matches!(ll.as_ref(), IdentPattern::Identifier(n) if n == "x")
                                );
                                assert!(
                                    matches!(lr.as_ref(), IdentPattern::Identifier(n) if n == "y")
                                );
                            }
                            other => panic!("Expected inner Maplet, got {:?}", other),
                        }
                    }
                    other => panic!("Expected outer Maplet, got {:?}", other),
                }
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
    if let PredicateKind::Comparison { right, .. } = &m.invariants[0].predicate.kind {
        match &right.kind {
            ExpressionKind::SetComprehension {
                identifiers,
                predicate,
                expression,
            } => {
                assert_eq!(identifiers.len(), 2);
                assert_eq!(identifiers[0], "x");
                assert_eq!(identifiers[1], "y");
                assert!(matches!(&predicate.kind, PredicateKind::Logical { .. }));
                assert!(expression.is_some());
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
    match &decrease.actions[0].action.kind {
        ActionKind::BecomesSuchThat {
            variables,
            predicate,
        } => {
            assert_eq!(variables, &["abstract_state"]);
            // The predicate should contain abstract_state' as an identifier
            fn contains_primed_ident(pred: &Predicate) -> bool {
                match &pred.kind {
                    PredicateKind::Comparison { left, right, .. } => {
                        has_primed_expr(left) || has_primed_expr(right)
                    }
                    PredicateKind::Logical { left, right, .. } => {
                        contains_primed_ident(left) || contains_primed_ident(right)
                    }
                    _ => false,
                }
            }
            fn has_primed_expr(expr: &Expression) -> bool {
                match &expr.kind {
                    ExpressionKind::Identifier(name) => name.ends_with('\''),
                    ExpressionKind::Binary { left, right, .. } => {
                        has_primed_expr(left) || has_primed_expr(right)
                    }
                    _ => false,
                }
            }
            assert!(
                contains_primed_ident(predicate),
                "Expected predicate to contain primed identifier (abstract_state'), got {:?}",
                predicate
            );
        }
        other => panic!("Expected BecomesSuchThat, got {:?}", other),
    }
}
