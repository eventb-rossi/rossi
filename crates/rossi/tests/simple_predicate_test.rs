//! Test for debugging predicate parsing

mod common;

use rossi::formula::tag::AssocPredOp;
use rossi::{
    ExpressionKind, ParseError, PredicateKind, parse, parse_action_str, parse_expression_str,
    parse_predicate_str,
};

#[test]
fn test_binary_addition_ast_structure() {
    use rossi::formula::tag::RelationalOp;
    use rossi::{ExpressionKind, PredicateKind};

    let source = r#"
    CONTEXT test
    CONSTANTS
        a b c
    AXIOMS
        @axm1 c = a + b
    END
    "#;

    let ctx = common::parse_context(source);
    assert_eq!(ctx.axioms.len(), 1);
    let pred = &ctx.axioms[0].predicate;

    // The predicate should be: c = (a + b)
    match pred.kind() {
        PredicateKind::Relational { op, left, right } => {
            assert_eq!(*op, RelationalOp::Equal);
            assert!(matches!(left.kind(), ExpressionKind::FreeIdentifier(name) if name == "c"));
            // right should be the sum a + b
            match right.kind() {
                ExpressionKind::Associative { op, children } => {
                    assert_eq!(*op, rossi::formula::tag::AssocExprOp::Plus);
                    assert!(
                        matches!(children[0].kind(), ExpressionKind::FreeIdentifier(name) if name == "a")
                    );
                    assert!(
                        matches!(children[1].kind(), ExpressionKind::FreeIdentifier(name) if name == "b")
                    );
                }
                other => panic!("Expected a sum, got {:?}", other),
            }
        }
        other => panic!("Expected Comparison predicate, got {:?}", other),
    }
}

#[test]
fn test_chained_binary_operations() {
    use rossi::ExpressionKind;

    let source = r#"
    CONTEXT test
    CONSTANTS
        a b c d
    AXIOMS
        @axm1 d = a + b + c
    END
    "#;

    let ctx = common::parse_context(source);
    if let rossi::PredicateKind::Relational { right, .. } = ctx.axioms[0].predicate.kind() {
        // a + b + c should be ((a + b) + c) - left associative
        match right.kind() {
            ExpressionKind::Associative { op, children } => {
                assert_eq!(*op, rossi::formula::tag::AssocExprOp::Plus);
                // Same-operator chains flatten: a + b + c is one node.
                assert_eq!(children.len(), 3);
                assert!(
                    matches!(children[2].kind(), ExpressionKind::FreeIdentifier(name) if name == "c")
                );
                assert!(
                    matches!(children[0].kind(), ExpressionKind::FreeIdentifier(name) if name == "a")
                );
                assert!(
                    matches!(children[1].kind(), ExpressionKind::FreeIdentifier(name) if name == "b")
                );
            }
            other => panic!("Expected a sum, got {:?}", other),
        }
    }
}

// All spellings of the binary logical operators: Unicode ∧ plus the ASCII
// forms & (AND) and `or` (OR).
#[test_case::test_case("x > 0 ∧ y > 0", AssocPredOp::LAnd ; "conjunction_unicode")]
#[test_case::test_case("x > 0 & y > 0", AssocPredOp::LAnd ; "conjunction_ascii_ampersand")]
#[test_case::test_case("x > 0 or y > 0", AssocPredOp::LOr ; "disjunction_ascii_or")]
fn test_logical_operator_spellings(invariant_body: &str, expected: AssocPredOp) {
    use rossi::PredicateKind;

    let source = common::invariant_machine("x y", invariant_body);
    let m = common::parse_machine(&source);
    let pred = &m.invariants[0].predicate;
    match pred.kind() {
        PredicateKind::Associative { op, children } => {
            assert_eq!(*op, expected);
            assert!(matches!(
                children[0].kind(),
                PredicateKind::Relational { .. }
            ));
            assert!(matches!(
                children[1].kind(),
                PredicateKind::Relational { .. }
            ));
        }
        other => panic!("Expected a logical connective, got {:?}", other),
    }
}

#[test]
fn test_chained_binary_predicates() {
    use rossi::PredicateKind;
    use rossi::formula::tag::AssocPredOp;

    let m = common::parse_machine(
        r#"
    MACHINE test
    VARIABLES
        x y z
    INVARIANTS
        @inv1 x > 0 ∧ y > 0 ∧ z > 0
    END
    "#,
    );
    let pred = &m.invariants[0].predicate;

    // Should be ((x > 0) ∧ (y > 0)) ∧ (z > 0) - left associative
    match pred.kind() {
        PredicateKind::Associative { op, children } => {
            assert_eq!(*op, AssocPredOp::LAnd);
            // Same-operator chains flatten into one n-ary conjunction.
            assert_eq!(children.len(), 3);
            assert!(matches!(
                children[2].kind(),
                PredicateKind::Relational { .. }
            ));
            assert!(matches!(
                children[0].kind(),
                PredicateKind::Relational { .. }
            ));
            assert!(matches!(
                children[1].kind(),
                PredicateKind::Relational { .. }
            ));
        }
        other => panic!("Expected Logical predicate, got {:?}", other),
    }
}

#[test]
fn test_lambda_expression() {
    use rossi::ExpressionKind;
    use rossi::formula::tag::{BinaryExprOp, QuantExprOp};

    let source = r#"
    CONTEXT test
    CONSTANTS
        f
    AXIOMS
        @axm1 f = λx·x ∈ ℕ ∣ x + 1
    END
    "#;

    let result = parse(source);
    if let Err(e) = &result {
        eprintln!("Parse error: {:?}", e);
    }
    assert!(result.is_ok(), "Lambda expression should parse correctly");

    let ctx = common::parse_context(source);
    if let rossi::PredicateKind::Relational { right, .. } = ctx.axioms[0].predicate.kind() {
        match right.kind() {
            ExpressionKind::Quantified {
                op: QuantExprOp::CSet,
                decls,
                pred,
                expr,
                form: rossi::Form::Lambda,
            } => {
                assert_eq!(decls.len(), 1);
                assert_eq!(decls[0].name(), "x");
                assert!(matches!(
                    pred.kind(),
                    rossi::PredicateKind::Relational { .. }
                ));
                // The value is `x ↦ x + 1`.
                assert!(matches!(
                    expr.kind(),
                    ExpressionKind::Binary {
                        op: BinaryExprOp::Mapsto,
                        ..
                    }
                ));
            }
            other => panic!("Expected Lambda expression, got {:?}", other),
        }
    }
}

// ============================================================================
// Unary expression parsing tests
// ============================================================================

macro_rules! test_unary_op {
    ($name:ident, $constants:expr, $axiom:expr, $expected:expr) => {
        #[test]
        fn $name() {
            use rossi::formula::tag::UnaryExprOp;
            use rossi::ExpressionKind;

            let source = common::axiom_context($constants, $axiom);
            let rhs = common::parse_axiom_rhs(&source);
            assert!(
                matches!(rhs.kind(), ExpressionKind::Unary { op, .. } if *op == $expected),
                "Expected {:?}, got {:?}",
                $expected,
                rhs
            );
        }
    };
}

test_unary_op!(test_unary_domain, "f, d", "d = dom(f)", UnaryExprOp::KDom);
test_unary_op!(test_unary_range, "f, r", "r = ran(f)", UnaryExprOp::KRan);
test_unary_op!(test_unary_powerset, "S, P", "P = POW(S)", UnaryExprOp::Pow);
test_unary_op!(
    test_unary_powerset1,
    "S, P",
    "P = POW1(S)",
    UnaryExprOp::Pow1
);
test_unary_op!(test_unary_minus, "x, y", "y = -x", UnaryExprOp::UnMinus);

#[test]
fn test_nested_unary() {
    use rossi::ExpressionKind;
    use rossi::formula::tag::UnaryExprOp;

    let source = common::axiom_context("f, r", "r = dom(ran(f))");
    let rhs = common::parse_axiom_rhs(&source);
    match rhs.kind() {
        ExpressionKind::Unary { op, child: operand } => {
            assert_eq!(*op, UnaryExprOp::KDom);
            match operand.kind() {
                ExpressionKind::Unary {
                    op: inner_op,
                    child: inner_operand,
                } => {
                    assert_eq!(*inner_op, UnaryExprOp::KRan);
                    assert!(matches!(
                        inner_operand.kind(),
                        ExpressionKind::FreeIdentifier(name) if name == "f"
                    ));
                }
                other => panic!("Expected inner Unary, got {:?}", other),
            }
        }
        other => panic!("Expected Unary expression, got {:?}", other),
    }
}

#[test]
fn test_unary_in_binary() {
    use rossi::ExpressionKind;
    use rossi::formula::tag::{AssocExprOp, UnaryExprOp};

    let source = common::axiom_context("f, g, result", "result = dom(f) \u{222A} ran(g)");
    let rhs = common::parse_axiom_rhs(&source);
    match rhs.kind() {
        ExpressionKind::Associative { op, children } => {
            assert_eq!(*op, AssocExprOp::BUnion);
            let (left, right) = (&children[0], &children[1]);
            assert!(matches!(
                left.kind(),
                ExpressionKind::Unary {
                    op: UnaryExprOp::KDom,
                    ..
                }
            ));
            assert!(matches!(
                right.kind(),
                ExpressionKind::Unary {
                    op: UnaryExprOp::KRan,
                    ..
                }
            ));
        }
        other => panic!("Expected Binary expression, got {:?}", other),
    }
}

// ============================================================================
// Negation predicate parsing tests
// ============================================================================

#[test_case::test_case("¬(x > 0)" ; "unicode")]
#[test_case::test_case("not(x > 0)" ; "ascii")]
fn test_negation(invariant_body: &str) {
    use rossi::PredicateKind;

    let source = common::invariant_machine("x", invariant_body);
    let m = common::parse_machine(&source);
    let pred = &m.invariants[0].predicate;
    match pred.kind() {
        PredicateKind::Not(inner) => {
            assert!(matches!(inner.kind(), PredicateKind::Relational { .. }));
        }
        other => panic!("Expected Not predicate, got {:?}", other),
    }
}

#[test]
fn test_double_negation() {
    use rossi::PredicateKind;

    let m = common::parse_machine(
        r#"
    MACHINE test
    VARIABLES
        x
    INVARIANTS
        @inv1 ¬(¬(x > 0))
    END
    "#,
    );
    let pred = &m.invariants[0].predicate;
    match pred.kind() {
        PredicateKind::Not(inner) => match inner.kind() {
            PredicateKind::Not(inner2) => {
                assert!(matches!(inner2.kind(), PredicateKind::Relational { .. }));
            }
            other => panic!("Expected inner Not, got {:?}", other),
        },
        other => panic!("Expected Not predicate, got {:?}", other),
    }
}

// ============================================================================
// Binary operator tests
// ============================================================================

macro_rules! test_binary_op {
    ($name:ident, $constants:expr, $axiom:expr, $expected:expr) => {
        #[test]
        fn $name() {
            use rossi::formula::tag::BinaryExprOp;
            use rossi::ExpressionKind;

            let source = common::axiom_context($constants, $axiom);
            let rhs = common::parse_axiom_rhs(&source);
            assert!(
                matches!(rhs.kind(), ExpressionKind::Binary { op, .. } if *op == $expected),
                "Expected {:?}, got {:?}",
                $expected,
                rhs
            );
        }
    };
}

#[test]
fn test_maplet_ascii() {
    use rossi::ExpressionKind;
    use rossi::formula::tag::BinaryExprOp;

    let source = common::axiom_context("x, y, r", "r = x |-> y");
    let rhs = common::parse_axiom_rhs(&source);
    match rhs.kind() {
        ExpressionKind::Binary { op, left, right } => {
            assert_eq!(*op, BinaryExprOp::Mapsto);
            assert!(matches!(left.kind(), ExpressionKind::FreeIdentifier(n) if n == "x"));
            assert!(matches!(right.kind(), ExpressionKind::FreeIdentifier(n) if n == "y"));
        }
        other => panic!("Expected Maplet, got {:?}", other),
    }
}

test_binary_op!(
    test_maplet_unicode,
    "x, y, r",
    "r = x \u{21A6} y",
    BinaryExprOp::Mapsto
);

#[test]
fn test_maplet_left_associative() {
    use rossi::ExpressionKind;
    use rossi::formula::tag::BinaryExprOp;

    let source = common::axiom_context("a, b, c, r", "r = a |-> b |-> c");
    let rhs = common::parse_axiom_rhs(&source);
    // Left-associative per spec p.18: a |-> b |-> c = (a |-> b) |-> c
    match rhs.kind() {
        ExpressionKind::Binary { op, left, right } => {
            assert_eq!(*op, BinaryExprOp::Mapsto);
            assert!(matches!(right.kind(), ExpressionKind::FreeIdentifier(n) if n == "c"));
            match left.kind() {
                ExpressionKind::Binary {
                    op: inner_op,
                    left: inner_left,
                    right: inner_right,
                } => {
                    assert_eq!(*inner_op, BinaryExprOp::Mapsto);
                    assert!(
                        matches!(inner_left.kind(), ExpressionKind::FreeIdentifier(n) if n == "a")
                    );
                    assert!(
                        matches!(inner_right.kind(), ExpressionKind::FreeIdentifier(n) if n == "b")
                    );
                }
                other => panic!("Expected inner Maplet, got {:?}", other),
            }
        }
        other => panic!("Expected Maplet, got {:?}", other),
    }
}

#[test]
fn test_maplet_binds_looser_than_relation_arrow() {
    use rossi::ExpressionKind;
    use rossi::formula::tag::BinaryExprOp;

    // kernel_lang Table 3.1: pair constructor binds looser than relation
    // arrows, so a ↦ b ↔ c = a ↦ (b ↔ c).
    let source = common::axiom_context("a, b, c, r", "r = a \u{21A6} b \u{2194} c");
    let rhs = common::parse_axiom_rhs(&source);
    match rhs.kind() {
        ExpressionKind::Binary { op, left, right } => {
            assert_eq!(*op, BinaryExprOp::Mapsto);
            assert!(matches!(left.kind(), ExpressionKind::FreeIdentifier(n) if n == "a"));
            match right.kind() {
                ExpressionKind::Binary {
                    op: inner_op,
                    left: inner_left,
                    right: inner_right,
                } => {
                    assert_eq!(*inner_op, BinaryExprOp::Rel);
                    assert!(
                        matches!(inner_left.kind(), ExpressionKind::FreeIdentifier(n) if n == "b")
                    );
                    assert!(
                        matches!(inner_right.kind(), ExpressionKind::FreeIdentifier(n) if n == "c")
                    );
                }
                other => panic!("Expected inner Relation, got {:?}", other),
            }
        }
        other => panic!("Expected Maplet, got {:?}", other),
    }
}

#[test]
fn test_maplet_binds_looser_than_total_fn_arrow() {
    use rossi::ExpressionKind;
    use rossi::formula::tag::BinaryExprOp;

    // a ↦ b → c = a ↦ (b → c)
    let source = common::axiom_context("a, b, c, r", "r = a \u{21A6} b \u{2192} c");
    let rhs = common::parse_axiom_rhs(&source);
    match rhs.kind() {
        ExpressionKind::Binary { op, right, .. } => {
            assert_eq!(*op, BinaryExprOp::Mapsto);
            assert!(matches!(
                right.kind(),
                ExpressionKind::Binary {
                    op: BinaryExprOp::TFun,
                    ..
                }
            ));
        }
        other => panic!("Expected Maplet, got {:?}", other),
    }
}

#[test]
fn test_maplet_binds_looser_than_relation_arrow_ascii() {
    // ASCII spellings parse to the same AST as the Unicode form.
    let unicode = common::parse_axiom_rhs(&common::axiom_context(
        "a, b, c, r",
        "r = a \u{21A6} b \u{2194} c",
    ));
    let ascii = common::parse_axiom_rhs(&common::axiom_context("a, b, c, r", "r = a |-> b <-> c"));
    assert_eq!(unicode, ascii);
}

#[test]
fn test_parenthesized_maplet_keeps_grouping_under_arrow() {
    use rossi::ExpressionKind;
    use rossi::formula::tag::BinaryExprOp;

    // Explicit parens override precedence: (a ↦ b) ↔ c stays a Relation.
    let source = common::axiom_context("a, b, c, r", "r = (a \u{21A6} b) \u{2194} c");
    let rhs = common::parse_axiom_rhs(&source);
    match rhs.kind() {
        ExpressionKind::Binary { op, left, right } => {
            assert_eq!(*op, BinaryExprOp::Rel);
            assert!(matches!(
                left.kind(),
                ExpressionKind::Binary {
                    op: BinaryExprOp::Mapsto,
                    ..
                }
            ));
            assert!(matches!(right.kind(), ExpressionKind::FreeIdentifier(n) if n == "c"));
        }
        other => panic!("Expected Relation, got {:?}", other),
    }
}

#[test]
fn test_maplet_chain_with_arrow_operands() {
    use rossi::ExpressionKind;
    use rossi::formula::tag::BinaryExprOp;

    // Pair-expression operands may each contain one (non-associative) arrow:
    // a ↔ b ↦ c ↔ d = (a ↔ b) ↦ (c ↔ d). Rejected by the old
    // (inverted-precedence) grammar, which allowed only one arrow per chain.
    let source = common::axiom_context("a, b, c, d, r", "r = a \u{2194} b \u{21A6} c \u{2194} d");
    let rhs = common::parse_axiom_rhs(&source);
    match rhs.kind() {
        ExpressionKind::Binary { op, left, right } => {
            assert_eq!(*op, BinaryExprOp::Mapsto);
            assert!(matches!(
                left.kind(),
                ExpressionKind::Binary {
                    op: BinaryExprOp::Rel,
                    ..
                }
            ));
            assert!(matches!(
                right.kind(),
                ExpressionKind::Binary {
                    op: BinaryExprOp::Rel,
                    ..
                }
            ));
        }
        other => panic!("Expected Maplet, got {:?}", other),
    }
}

#[test]
fn test_maplet_binds_looser_than_arrow_in_action() {
    use rossi::ExpressionKind;
    use rossi::formula::tag::BinaryExprOp;

    // Same precedence through the _no_semi expression twins used in actions.
    let m = common::parse_machine(
        r#"
    MACHINE test
    VARIABLES
        x a b c
    EVENTS
        EVENT update
        THEN
            @act1 x ≔ a ↦ b ↔ c
        END
    END
    "#,
    );
    let event = &m.events[0];
    match event.actions[0].action.kind() {
        rossi::AssignmentKind::BecomesEqualTo { values, .. } => match values[0].kind() {
            ExpressionKind::Binary { op, right, .. } => {
                assert_eq!(*op, BinaryExprOp::Mapsto);
                assert!(matches!(
                    right.kind(),
                    ExpressionKind::Binary {
                        op: BinaryExprOp::Rel,
                        ..
                    }
                ));
            }
            other => panic!("Expected Maplet, got {:?}", other),
        },
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

test_binary_op!(
    test_total_function,
    "S, T, f",
    "f = S --> T",
    BinaryExprOp::TFun
);
test_binary_op!(
    test_partial_function,
    "S, T, f",
    "f = S +-> T",
    BinaryExprOp::PFun
);
test_binary_op!(
    test_relation_type,
    "S, T, r",
    "r = S <-> T",
    BinaryExprOp::Rel
);
test_binary_op!(
    test_domain_restriction,
    "S, f, r",
    "r = S <| f",
    BinaryExprOp::DomRes
);
test_binary_op!(
    test_range_restriction,
    "f, S, r",
    "r = f |> S",
    BinaryExprOp::RanRes
);
test_binary_op!(
    test_domain_subtraction,
    "S, f, r",
    "r = S <<| f",
    BinaryExprOp::DomSub
);
test_binary_op!(
    test_range_subtraction,
    "f, S, r",
    "r = f |>> S",
    BinaryExprOp::RanSub
);
#[test]
fn test_overwrite() {
    let source = common::axiom_context("f, g, r", "r = f <+ g");
    let rhs = common::parse_axiom_rhs(&source);
    assert!(
        matches!(
            rhs.kind(),
            rossi::ExpressionKind::Associative {
                op: rossi::formula::tag::AssocExprOp::Ovr,
                ..
            }
        ),
        "Expected override, got {rhs:?}"
    );
}
#[test]
fn test_overwrite_pua() {
    let source = common::axiom_context("f, g, r", "r = f \u{E103} g");
    let rhs = common::parse_axiom_rhs(&source);
    assert!(
        matches!(
            rhs.kind(),
            rossi::ExpressionKind::Associative {
                op: rossi::formula::tag::AssocExprOp::Ovr,
                ..
            }
        ),
        "Expected override, got {rhs:?}"
    );
}
test_binary_op!(test_exponent, "a, b, r", "r = a ^ b", BinaryExprOp::Expn);

/// Per spec §3.3.6: exponent binds tighter than additive and multiplicative.
/// `2 ^ 3 + 4` must parse as `(2 ^ 3) + 4`, not `2 ^ (3 + 4)`.
#[test]
fn test_exponent_precedence_vs_additive() {
    use rossi::formula::tag::BinaryExprOp;

    // 2 ^ 3 + 4 should be (2^3) + 4, i.e. Add at the top
    let source = common::axiom_context("r", "r = 2 ^ 3 + 4");
    let rhs = common::parse_axiom_rhs(&source);
    assert!(
        matches!(
            rhs.kind(),
            ExpressionKind::Associative {
                op: rossi::formula::tag::AssocExprOp::Plus,
                children,
            } if matches!(
                children[0].kind(),
                ExpressionKind::Binary { op: BinaryExprOp::Expn, .. }
            ) && matches!(children[1].kind(), ExpressionKind::IntegerLiteral(n) if *n == 4.into())
        ),
        "2 ^ 3 + 4 should parse as (2^3)+4, got {:?}",
        rhs
    );
}

/// `a * b ^ c` must parse as `a * (b ^ c)`, not `(a * b) ^ c`.
#[test]
fn test_exponent_precedence_vs_multiplicative() {
    use rossi::formula::tag::BinaryExprOp;

    let source = common::axiom_context("a, b, c, r", "r = a * b ^ c");
    let rhs = common::parse_axiom_rhs(&source);
    assert!(
        matches!(
            rhs.kind(),
            ExpressionKind::Associative {
                op: rossi::formula::tag::AssocExprOp::Mul,
                children,
            } if matches!(children[0].kind(), ExpressionKind::FreeIdentifier(id) if id == "a")
              && matches!(children[1].kind(), ExpressionKind::Binary { op: BinaryExprOp::Expn, .. })
        ),
        "a * b ^ c should parse as a*(b^c), got {:?}",
        rhs
    );
}

test_binary_op!(
    test_direct_product,
    "f, g, r",
    "r = f >< g",
    BinaryExprOp::DProd
);

#[test]
fn test_multiply_vs_cartesian_product() {
    use rossi::ExpressionKind;
    use rossi::formula::tag::BinaryExprOp;

    // ASCII "*" should parse as multiplication
    let source = common::axiom_context("a, b, r", "r = a * b");
    let rhs = common::parse_axiom_rhs(&source);
    assert!(
        matches!(
            rhs.kind(),
            ExpressionKind::Associative {
                op: rossi::formula::tag::AssocExprOp::Mul,
                ..
            }
        ),
        "Single * should parse as Multiply, got {:?}",
        rhs
    );

    // ASCII "**" should parse as CartesianProduct
    let source = common::axiom_context("S, T, r", "r = S ** T");
    let rhs = common::parse_axiom_rhs(&source);
    assert!(
        matches!(
            rhs.kind(),
            ExpressionKind::Binary {
                op: BinaryExprOp::CProd,
                ..
            }
        ),
        "Double ** should parse as CartesianProduct, got {:?}",
        rhs
    );

    // Unicode "×" should parse as CartesianProduct
    let source = common::axiom_context("S, T, r", "r = S \u{00D7} T");
    let rhs = common::parse_axiom_rhs(&source);
    assert!(
        matches!(
            rhs.kind(),
            ExpressionKind::Binary {
                op: BinaryExprOp::CProd,
                ..
            }
        ),
        "Unicode × should parse as CartesianProduct, got {:?}",
        rhs
    );
}

// ASCII and Unicode type-set tokens parse as the builtin type sets, not as
// identifiers.
#[test_case::test_case("NAT1", rossi::formula::tag::AtomicOp::Natural1 ; "nat1_ascii")]
#[test_case::test_case("\u{2115}1", rossi::formula::tag::AtomicOp::Natural1 ; "nat1_unicode")]
#[test_case::test_case("INT", rossi::formula::tag::AtomicOp::Integer ; "int_ascii")]
#[test_case::test_case("\u{2124}", rossi::formula::tag::AtomicOp::Integer ; "int_unicode")]
#[test_case::test_case("NAT", rossi::formula::tag::AtomicOp::Natural ; "nat_ascii")]
fn type_set_token_parses_as_type_set(token: &str, expected: rossi::formula::tag::AtomicOp) {
    let source = common::invariant_machine("x", &format!("x ∈ {token}"));
    let m = common::parse_machine(&source);
    let rossi::PredicateKind::Relational { right, .. } = m.invariants[0].predicate.kind() else {
        panic!("Expected Comparison predicate");
    };
    assert!(
        matches!(right.kind(), ExpressionKind::Atomic(op) if *op == expected),
        "{token} should parse as the builtin type set, not Identifier"
    );
}

// ASCII type-set spellings are exact-case (uppercase NAT1/INT); the lowercase
// form is an ordinary identifier, not the type set.
#[test_case::test_case("nat1", "Naturals1" ; "nat1")]
#[test_case::test_case("int", "Integers" ; "int")]
fn lowercase_type_keyword_is_identifier(word: &str, type_set: &str) {
    use rossi::ExpressionKind;

    let source = common::invariant_machine("x", &format!("x ∈ {word}"));
    let m = common::parse_machine(&source);
    let rossi::PredicateKind::Relational { right, .. } = m.invariants[0].predicate.kind() else {
        panic!("Expected Comparison predicate");
    };
    assert!(
        matches!(right.kind(), ExpressionKind::FreeIdentifier(n) if n == word),
        "lowercase {word} is an ordinary identifier, not {type_set}"
    );
}

#[test]
fn test_negation_in_conjunction() {
    use rossi::PredicateKind;
    use rossi::formula::tag::AssocPredOp;

    let m = common::parse_machine(
        r#"
    MACHINE test
    VARIABLES
        x y
    INVARIANTS
        @inv1 ¬(x > 0) ∧ y > 0
    END
    "#,
    );
    let pred = &m.invariants[0].predicate;
    match pred.kind() {
        PredicateKind::Associative { op, children } => {
            assert_eq!(*op, AssocPredOp::LAnd);
            assert!(matches!(children[0].kind(), PredicateKind::Not(_)));
            assert!(matches!(
                children[1].kind(),
                PredicateKind::Relational { .. }
            ));
        }
        other => panic!("Expected Logical predicate, got {:?}", other),
    }
}

// --- becomes-such-that: Unicode :∣ and ASCII :| produce the same AST ----------

#[test]
fn test_becomes_such_that_unicode_and_ascii_same_ast() {
    let source_unicode = r#"
    MACHINE test
    EVENTS
        EVENT INITIALISATION
        THEN
            @act1 x :∣ x > 0
        END
    END
    "#;
    let source_ascii = r#"
    MACHINE test
    EVENTS
        EVENT INITIALISATION
        THEN
            @act1 x :| x > 0
        END
    END
    "#;

    let result_unicode = parse(source_unicode);
    assert!(
        result_unicode.is_ok(),
        "Unicode :∣ should parse: {:?}",
        result_unicode.err()
    );
    let result_ascii = parse(source_ascii);
    assert!(
        result_ascii.is_ok(),
        "ASCII :| should parse: {:?}",
        result_ascii.err()
    );

    // Both should produce identical AST (clear spans since byte offsets differ)
    let mut component_unicode = result_unicode.unwrap();
    let mut component_ascii = result_ascii.unwrap();
    common::clear_spans(&mut component_unicode);
    common::clear_spans(&mut component_ascii);
    assert_eq!(
        component_unicode, component_ascii,
        "Unicode :∣ and ASCII :| should produce the same AST"
    );
}

// ============================================================================
// Expression precedence tests (spec §3.3.4 Table 3.1)
// ============================================================================

use rossi::formula::tag::BinaryExprOp;

#[test]
fn test_range_cartesian_product() {
    // 1‥2 × 1‥3 should parse as (1‥2) × (1‥3), not 1 ‥ (2×1) ‥ 3
    let source = common::axiom_context("S", "S = 1‥2 × 1‥3");
    let rhs = common::parse_axiom_rhs(&source);
    match rhs.kind() {
        ExpressionKind::Binary {
            op: BinaryExprOp::CProd,
            left,
            right,
        } => {
            assert!(
                matches!(
                    left.kind(),
                    ExpressionKind::Binary {
                        op: BinaryExprOp::UpTo,
                        ..
                    }
                ),
                "Left should be Range, got {:?}",
                left
            );
            assert!(
                matches!(
                    right.kind(),
                    ExpressionKind::Binary {
                        op: BinaryExprOp::UpTo,
                        ..
                    }
                ),
                "Right should be Range, got {:?}",
                right
            );
        }
        other => panic!("Expected CartesianProduct of two Ranges, got {:?}", other),
    }
}

#[test]
fn test_arithmetic_before_range() {
    // a + b .. c should parse as (a + b) .. c
    let source = common::axiom_context("a, b, c, S", "S = a + b .. c");
    let rhs = common::parse_axiom_rhs(&source);
    match rhs.kind() {
        ExpressionKind::Binary {
            op: BinaryExprOp::UpTo,
            left,
            ..
        } => {
            assert!(
                matches!(
                    left.kind(),
                    ExpressionKind::Associative {
                        op: rossi::formula::tag::AssocExprOp::Plus,
                        ..
                    }
                ),
                "Left of Range should be Add, got {:?}",
                left
            );
        }
        other => panic!("Expected Range with Add left, got {:?}", other),
    }
}

#[test]
fn test_range_before_union() {
    // a .. b ∪ C should parse as (a .. b) ∪ C
    let source = common::axiom_context("a, b, C, S", "S = a .. b ∪ C");
    let rhs = common::parse_axiom_rhs(&source);
    match rhs.kind() {
        ExpressionKind::Associative {
            op: rossi::formula::tag::AssocExprOp::BUnion,
            children,
            ..
        } => {
            let left = &children[0];
            assert!(
                matches!(
                    left.kind(),
                    ExpressionKind::Binary {
                        op: BinaryExprOp::UpTo,
                        ..
                    }
                ),
                "Left of Union should be Range, got {:?}",
                left
            );
        }
        other => panic!("Expected Union with Range left, got {:?}", other),
    }
}

// ── TRUE / FALSE in expressions vs predicate constants ──────────────

// TRUE/FALSE in expression position (here: element of a set) parse as
// expressions, not as the predicate constants.
#[test_case::test_case("TRUE", rossi::formula::tag::AtomicOp::True ; "true_literal")]
#[test_case::test_case("FALSE", rossi::formula::tag::AtomicOp::False ; "false_literal")]
fn bool_literal_in_set(token: &str, expected: rossi::formula::tag::AtomicOp) {
    use rossi::PredicateKind;
    use rossi::formula::tag::RelationalOp;
    let source = format!("CONTEXT test\nAXIOMS\n    @axm1 {token} ∈ {{queue_1, queue_2}}\nEND\n");
    let ctx = common::parse_context(&source);
    match ctx.axioms[0].predicate.kind() {
        PredicateKind::Relational {
            op: RelationalOp::In,
            left,
            ..
        } => {
            assert!(
                matches!(left.kind(), ExpressionKind::Atomic(op) if *op == expected),
                "for {token}"
            );
        }
        other => panic!("Expected {token} ∈ comparison, got {other:?}"),
    }
}

// The bare ⊤/⊥ constants parse as predicates.
#[test_case::test_case("⊤", rossi::formula::tag::LiteralPredOp::BTrue ; "true_constant")]
#[test_case::test_case("⊥", rossi::formula::tag::LiteralPredOp::BFalse ; "false_constant")]
fn bare_bool_predicate(axiom_body: &str, expected: rossi::formula::tag::LiteralPredOp) {
    let source = format!("CONTEXT test\nAXIOMS\n    @axm1 {axiom_body}\nEND\n");
    let ctx = common::parse_context(&source);
    assert!(
        matches!(ctx.axioms[0].predicate.kind(), rossi::PredicateKind::Literal(op) if *op == expected),
        "for {axiom_body}"
    );
}

// TRUE parses as an expression on either side of `=` — the RHS position
// (immediately before END) is the token-boundary position issue #30 was about.
#[test_case::test_case("TRUE = x", true ; "true_on_lhs")]
#[test_case::test_case("x = TRUE", false ; "true_on_rhs")]
fn test_true_eq_comparison(axiom_body: &str, true_on_left: bool) {
    use rossi::PredicateKind;
    use rossi::formula::tag::RelationalOp;
    let source = common::axiom_context("x", axiom_body);
    let ctx = common::parse_context(&source);
    match ctx.axioms[0].predicate.kind() {
        PredicateKind::Relational {
            op: RelationalOp::Equal,
            left,
            right,
        } => {
            let operand = if true_on_left { left } else { right };
            assert!(
                matches!(
                    operand.kind(),
                    ExpressionKind::Atomic(rossi::formula::tag::AtomicOp::True)
                ),
                "Expected the TRUE literal in {axiom_body:?}, got {:?}",
                operand
            );
        }
        other => panic!("Expected TRUE equality comparison, got {:?}", other),
    }
}

// ===== Postfix function update: f{x ↦ y} == f <+ {x ↦ y} =====
//
// Rodin's parser accepts this compact form (well-formed under its
// FormulaFactory.OVR tag); our grammar lowers it to the same AST as the
// explicit <+ form so consumers don't need to handle a new variant. The
// canonical static-checker emission uses U+E103.

#[test]
fn test_postfix_function_update_lowers_to_overwrite() {
    let postfix = parse_expression_str("f{x ↦ y}").expect("postfix update parses");
    let explicit = parse_expression_str("f <+ {x ↦ y}").expect("explicit overwrite parses");
    assert_eq!(postfix, explicit);

    let postfix = parse_expression_str("f{x ↦ y, a ↦ b}").expect("multi-element postfix parses");
    let explicit =
        parse_expression_str("f <+ {x ↦ y, a ↦ b}").expect("multi-element explicit parses");
    assert_eq!(postfix, explicit);
}

#[test]
fn test_postfix_function_update_in_action() {
    let action = parse_action_str("currentFloor ≔ currentFloor{c ↦ f}").expect("action parses");
    let equivalent = parse_action_str("currentFloor ≔ currentFloor <+ {c ↦ f}").expect("explicit");
    assert_eq!(action, equivalent);
    match action.kind() {
        rossi::AssignmentKind::BecomesEqualTo { idents, .. } => {
            assert_eq!(idents.len(), 1);
            assert!(
                matches!(idents[0].kind(), ExpressionKind::FreeIdentifier(n) if n == "currentFloor")
            );
        }
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

#[test]
fn test_postfix_function_update_set_enumeration_unaffected() {
    // A bare set enumeration is still set_enumeration — only postfix
    // application after a primary_expr triggers the new branch.
    let bare = parse_expression_str("{x ↦ y}").expect("bare set enum parses");
    match bare.kind() {
        ExpressionKind::SetExtension(_) => {}
        other => panic!("Expected SetExtension, got {:?}", other),
    }
}

// Regression: `parse_expression` and `parse_predicate` recurse through every
// precedence-wrapper rule on the way down, which previously consumed enough
// stack on this file-system invariant to overflow a 2 MB test thread.
// `parse_expression` and `parse_predicate` now unwrap single-child wrappers
// in a loop, so this should fit comfortably in 1 MB.
#[test]
fn test_deep_predicate_fits_in_small_stack() {
    use rossi::parse_predicate_str;
    let input = "C ∖ {x ↦ y ∣ y ∈ dom(f(x))}[C] ≠ ∅";
    let s: String = input.into();
    let parsed = std::thread::Builder::new()
        .stack_size(1024 * 1024) // 1 MB — was failing at 2 MB before the fix
        .spawn(move || parse_predicate_str(&s).is_ok())
        .unwrap()
        .join()
        .unwrap_or(false);
    assert!(parsed, "file-system invariant must parse on a 1 MB stack");
}

// ===== Surjection alias spellings parse at the grammar level =====
//
// `+->>`/`-->>` are accepted alternative ASCII input spellings for the
// surjection arrows (Rodin's keyboard). Eager input converts them to ⤀/↠
// before the parser sees them, but raw text (pasted, or with eager input off)
// must still parse — and as the *surjection*, not the `+->`/`-->` function
// arrow they extend with a dangling `>`. These pin the `op_partial_surj` /
// `op_total_surj` grammar alternatives so a future reorder can't regress them.

fn binary_op_of(src: &str) -> rossi::formula::tag::BinaryExprOp {
    let parsed =
        parse_expression_str(src).unwrap_or_else(|e| panic!("expected `{src}` to parse, got: {e}"));
    match parsed.kind() {
        ExpressionKind::Binary { op, .. } => *op,
        other => panic!("expected `{src}` to be a binary expression, got: {other:?}"),
    }
}

/// `true` iff `src` parses to a type ascription (`E ⦂ T`).
fn parses_as_ascription(src: &str) -> bool {
    let parsed =
        parse_expression_str(src).unwrap_or_else(|e| panic!("expected `{src}` to parse, got: {e}"));
    matches!(parsed.kind(), ExpressionKind::Ascription { .. })
}

#[test]
fn test_surjection_alias_spellings_parse_as_surjections() {
    use rossi::formula::tag::BinaryExprOp;
    assert_eq!(binary_op_of("S +->> T"), BinaryExprOp::PSur);
    assert_eq!(binary_op_of("S -->> T"), BinaryExprOp::TSur);
    // The canonical forms still parse identically.
    assert_eq!(binary_op_of("S +>> T"), BinaryExprOp::PSur);
    assert_eq!(binary_op_of("S ->> T"), BinaryExprOp::TSur);
    // …and the shorter function arrows they extend are unaffected (no
    // dangling `>` left behind by a greedy surjection match).
    assert_eq!(binary_op_of("S +-> T"), BinaryExprOp::PFun);
    assert_eq!(binary_op_of("S --> T"), BinaryExprOp::TFun);
}

#[test]
fn test_relation_arrow_and_typing_operator_spellings() {
    use rossi::formula::tag::BinaryExprOp;
    assert_eq!(binary_op_of("A <<-> B"), BinaryExprOp::TRel);
    assert_eq!(binary_op_of("A <->> B"), BinaryExprOp::SRel);
    assert_eq!(binary_op_of("A <<->> B"), BinaryExprOp::STRel);
    // Both spellings of the oftype typing operator (an ascription node).
    assert!(parses_as_ascription("\u{2115} oftype \u{2124}"));
    assert!(parses_as_ascription("\u{2115} \u{2982} \u{2124}"));
    // Regression (d581fd2): `,,` is a maplet spelling, not the empty set.
    assert_eq!(binary_op_of("x ,, y"), BinaryExprOp::Mapsto);
}

/// Every code point Rodin's math lexer treats as whitespace.
///
/// `LexicalClass.isWhitespace(cp)` is
/// `Character.isWhitespace(cp) || FormulaFactory.isEventBWhiteSpace(cp)`
/// (RodinCore `org.eventb.core.ast`), and `isEventBWhiteSpace(cp)` is
/// `Character.isSpaceChar(cp) || 0x09..=0x0D || 0x1C..=0x1F`. OR-ing
/// `isSpaceChar` in cancels Java's usual NBSP / U+2007 / U+202F carve-out, so
/// the union is exactly Zs u Zl u Zp u U+0009..=U+000D u U+001C..=U+001F --
/// the 28 code points below. `grammar.pest`'s `WHITESPACE` enumerates the same
/// set; this is the list that pins it.
const RODIN_WHITESPACE: &[char] = &[
    '\u{09}', '\u{0A}', '\u{0B}', '\u{0C}', '\u{0D}', // U+0009..U+000D
    '\u{1C}', '\u{1D}', '\u{1E}', '\u{1F}',   // U+001C..U+001F
    '\u{20}',   // SPACE (Zs)
    '\u{A0}',   // NO-BREAK SPACE (Zs)
    '\u{1680}', // OGHAM SPACE MARK (Zs)
    '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}', '\u{2007}',
    '\u{2008}', '\u{2009}', '\u{200A}', // EN QUAD..HAIR SPACE (Zs)
    '\u{2028}', // LINE SEPARATOR (Zl)
    '\u{2029}', // PARAGRAPH SEPARATOR (Zp)
    '\u{202F}', // NARROW NO-BREAK SPACE (Zs)
    '\u{205F}', // MEDIUM MATHEMATICAL SPACE (Zs)
    '\u{3000}', // IDEOGRAPHIC SPACE (Zs)
];

/// Code points that read as blank but are **not** separators to Rodin, so they
/// must not be separators here either.
///
/// U+0085 NEL is the one worth calling out: both `Character.isWhitespace` and
/// `Character.isSpaceChar` exclude it, so Rodin's math lexer rejects it. (It is
/// Camille's `layout_char` -- a different lexer, in a different project -- that
/// accepts U+0085, which is why a survey that merges the two disagrees here.)
/// The rest are category Cf, not a space separator.
const NOT_RODIN_WHITESPACE: &[char] = &[
    '\u{85}',   // NEXT LINE (Cc)
    '\u{200B}', // ZERO WIDTH SPACE (Cf)
    '\u{FEFF}', // ZERO WIDTH NO-BREAK SPACE / BOM (Cf)
    '\u{180E}', // MONGOLIAN VOWEL SEPARATOR (Cf since Unicode 6.3)
];

#[test]
fn test_unicode_whitespace_matches_rodin() {
    // Real Rodin XML in the wild puts U+00A0 around operators like `=` and
    // `<=`, which Rodin accepts -- so the wide set is not academic.
    let ascii =
        rossi::parse_predicate_str("sense_ev = TRUE").expect("ASCII-spaced predicate should parse");

    for &separator in RODIN_WHITESPACE {
        let source = format!("sense_ev{separator}={separator}TRUE");
        let parsed = rossi::parse_predicate_str(&source).unwrap_or_else(|e| {
            panic!(
                "U+{:04X} is Rodin whitespace and must parse: {e:?}",
                separator as u32
            )
        });
        assert_eq!(
            parsed, ascii,
            "U+{:04X} must separate tokens exactly like an ASCII space",
            separator as u32
        );
        // `keywords::is_whitespace` is a second enumeration of the same set,
        // for callers that scan text rather than parse it. Holding both to one
        // table is what stops the scanner and the parser drifting apart.
        assert!(
            rossi::keywords::is_whitespace(separator),
            "U+{:04X} parses as a separator but is_whitespace says otherwise",
            separator as u32
        );
    }

    for &glyph in NOT_RODIN_WHITESPACE {
        let source = format!("sense_ev{glyph}={glyph}TRUE");
        assert!(
            rossi::parse_predicate_str(&source).is_err(),
            "U+{:04X} is not whitespace in Rodin; it must not glue tokens together",
            glyph as u32
        );
        assert!(
            !rossi::keywords::is_whitespace(glyph),
            "U+{:04X} does not parse as a separator but is_whitespace claims it does",
            glyph as u32
        );
    }
}

#[test]
fn eb031_flags_exactly_the_rodin_separators_camille_cannot_read() {
    // `camille_unreadable_separator` is defined as `is_whitespace` minus
    // `camille_layout_char`, so the two lexers' sets cannot drift apart by
    // construction. What is worth pinning is the boundary that decides whether
    // EB031 is usable in CI at all.
    //
    // U+00A0: Camille's second range ends exactly on it, and it is the
    // separator real Rodin XML contains — a warning here would fire on the
    // common case, and `--deny-warnings` would make that a build failure.
    assert!(rossi::keywords::is_whitespace('\u{a0}'));
    assert!(!rossi::keywords::camille_unreadable_separator('\u{a0}'));

    // U+3000: unreadable to Camille (`EB004 Unknown token`), read by rossi.
    assert!(rossi::keywords::camille_unreadable_separator('\u{3000}'));

    // U+200B: unreadable to Camille, but not a separator to rossi either, so
    // it is not an EB031 case — rossi rejects that file outright.
    assert!(!rossi::keywords::is_whitespace('\u{200b}'));
    assert!(!rossi::keywords::camille_unreadable_separator('\u{200b}'));

    // Every EB031 code point is a separator Camille's ranges exclude.
    for separator in RODIN_WHITESPACE
        .iter()
        .filter(|&&c| rossi::keywords::camille_unreadable_separator(c))
    {
        assert!(!rossi::keywords::camille_layout_char(*separator));
    }
}

#[test]
fn test_label_terminates_on_unicode_whitespace() {
    // A label is "@" then non-whitespace up to the next whitespace. The grammar's
    // whitespace set now includes U+00A0, and the `label_text` rule references
    // WHITESPACE, so a label followed by a U+00A0 must terminate at it — not
    // swallow the following predicate into the label. With a U+00A0 between the
    // label and the predicate, the label is still `axm1` and `1 = 1` parses as
    // the axiom predicate.
    let ctx = common::parse_context("CONTEXT c\nAXIOMS\n@axm1\u{a0}1 = 1\nEND\n");
    assert_eq!(ctx.axioms.len(), 1);
    assert_eq!(ctx.axioms[0].label.as_deref(), Some("axm1"));
}

// ============================================================================
// Built-in function tests
// ============================================================================

#[test]
fn test_builtin_card() {
    let ctx = common::parse_context("CONTEXT test\nAXIOMS\n    @axm1 card(S) = 5\nEND\n");
    let pred = &ctx.axioms[0].predicate;
    if let PredicateKind::Relational { left, .. } = pred.kind() {
        match left.kind() {
            ExpressionKind::Unary {
                op: rossi::formula::tag::UnaryExprOp::KCard,
                child,
            } => {
                assert!(matches!(child.kind(), ExpressionKind::FreeIdentifier(n) if n == "S"));
            }
            other => panic!("Expected card application, got {:?}", other),
        }
    } else {
        panic!("Expected Comparison predicate");
    }
}

#[test]
fn test_builtin_min_max() {
    let ctx = common::parse_context(
        "CONTEXT test\nAXIOMS\n    @axm1 min(S) = 0\n    @axm2 max(S) = 100\nEND\n",
    );
    if let PredicateKind::Relational { left, .. } = ctx.axioms[0].predicate.kind() {
        assert!(matches!(
            left.kind(),
            ExpressionKind::Unary {
                op: rossi::formula::tag::UnaryExprOp::KMin,
                ..
            }
        ));
    }
    if let PredicateKind::Relational { left, .. } = ctx.axioms[1].predicate.kind() {
        assert!(matches!(
            left.kind(),
            ExpressionKind::Unary {
                op: rossi::formula::tag::UnaryExprOp::KMax,
                ..
            }
        ));
    }
}

// ============================================================================
// Built-in predicate tests
// ============================================================================

#[test]
fn test_builtin_finite() {
    let ctx = common::parse_context("CONTEXT test\nAXIOMS\n    @axm1 finite(S)\nEND\n");
    match ctx.axioms[0].predicate.kind() {
        PredicateKind::Simple(argument) => {
            assert!(matches!(argument.kind(), ExpressionKind::FreeIdentifier(n) if n == "S"));
        }
        other => panic!("Expected the finite predicate, got {:?}", other),
    }
}

#[test]
fn test_builtin_partition() {
    let ctx = common::parse_context("CONTEXT test\nAXIOMS\n    @axm1 partition(S, A, B)\nEND\n");
    match ctx.axioms[0].predicate.kind() {
        PredicateKind::Multiple(arguments) => {
            assert_eq!(arguments.len(), 3);
        }
        other => panic!("Expected the partition predicate, got {:?}", other),
    }
}

#[test]
fn test_user_defined_predicate() {
    let ctx = common::parse_context("CONTEXT test\nAXIOMS\n    @axm1 myPred(x)\nEND\n");
    match ctx.axioms[0].predicate.kind() {
        PredicateKind::Application { function, args, .. } => {
            assert_eq!(function, "myPred");
            assert_eq!(args.len(), 1);
        }
        other => panic!("Expected Application(myPred), got {:?}", other),
    }
}

// ============================================================================
// bool(P) expression tests
// ============================================================================

#[test]
fn test_bool_expr() {
    let ctx = common::parse_context("CONTEXT test\nAXIOMS\n    @axm1 bool(x > 0) = TRUE\nEND\n");
    if let PredicateKind::Relational { left, .. } = ctx.axioms[0].predicate.kind() {
        match left.kind() {
            ExpressionKind::Bool(pred) => {
                assert!(
                    matches!(pred.kind(), PredicateKind::Relational { .. }),
                    "Expected Comparison inside Bool, got {:?}",
                    pred
                );
            }
            other => panic!("Expected Bool expression, got {:?}", other),
        }
    } else {
        panic!("Expected Comparison predicate");
    }
}

#[test]
fn test_bool_vs_bool_type() {
    let ctx = common::parse_context("CONTEXT test\nAXIOMS\n    @axm1 x : BOOL\nEND\n");
    if let PredicateKind::Relational { right, .. } = ctx.axioms[0].predicate.kind() {
        assert!(matches!(
            right.kind(),
            ExpressionKind::Atomic(rossi::formula::tag::AtomicOp::Bool)
        ));
    } else {
        panic!("Expected Comparison predicate");
    }
}

// ============================================================================
// Extended set comprehension
// ============================================================================

#[test]
fn test_extended_set_comprehension() {
    let source = r#"
    MACHINE test
    VARIABLES
        s
    INVARIANTS
        @inv1 s = {x · x ∈ ℕ | x * x}
    END
    "#;

    let m = common::parse_machine(source);
    let pred = &m.invariants[0].predicate;
    if let rossi::PredicateKind::Relational { right, .. } = pred.kind() {
        match right.kind() {
            ExpressionKind::Quantified {
                op: rossi::formula::tag::QuantExprOp::CSet,
                decls,
                form: rossi::Form::Explicit,
                ..
            } => {
                assert_eq!(decls.len(), 1);
                assert_eq!(decls[0].name(), "x");
            }
            other => panic!("Expected SetComprehension, got {:?}", other),
        }
    } else {
        panic!("Expected Comparison predicate");
    }
}

// ============================================================================
// Relational image
// ============================================================================

#[test]
fn test_relational_image() {
    let source = r#"
    MACHINE test
    VARIABLES
        r s
    INVARIANTS
        @inv1 r[s] = s
    END
    "#;

    let m = common::parse_machine(source);
    let pred = &m.invariants[0].predicate;
    if let rossi::PredicateKind::Relational { left, .. } = pred.kind() {
        match left.kind() {
            ExpressionKind::Binary {
                op: rossi::formula::tag::BinaryExprOp::RelImage,
                left: relation,
                right: set,
            } => {
                assert!(matches!(relation.kind(), ExpressionKind::FreeIdentifier(n) if n == "r"));
                assert!(matches!(set.kind(), ExpressionKind::FreeIdentifier(n) if n == "s"));
            }
            other => panic!("Expected RelationalImage, got {:?}", other),
        }
    } else {
        panic!("Expected Comparison predicate");
    }
}

// ============================================================================
// Typed bound variables (⦂) in quantifiers
// ============================================================================

#[test]
fn test_typed_forall_single() {
    use rossi::formula::tag::QuantPredOp;

    let source = r#"
    CONTEXT test
    AXIOMS
        @axm1 ∀x⦂ℤ · x > 0
    END
    "#;

    let ctx = common::parse_context(source);
    match ctx.axioms[0].predicate.kind() {
        rossi::PredicateKind::Quantified { op, decls, .. } => {
            assert_eq!(*op, QuantPredOp::Forall);
            assert_eq!(decls.len(), 1);
            assert_eq!(decls[0].name(), "x");
            assert!(matches!(
                decls[0].annotation().map(rossi::Expression::kind),
                Some(ExpressionKind::Atomic(
                    rossi::formula::tag::AtomicOp::Integer
                ))
            ));
        }
        other => panic!("Expected Quantified ForAll, got {:?}", other),
    }
}

#[test]
fn test_typed_exists() {
    use rossi::formula::tag::QuantPredOp;

    let source = r#"
    CONTEXT test
    AXIOMS
        @axm1 ∃x⦂ℤ · x = 0
    END
    "#;

    let ctx = common::parse_context(source);
    match ctx.axioms[0].predicate.kind() {
        rossi::PredicateKind::Quantified { op, decls, .. } => {
            assert_eq!(*op, QuantPredOp::Exists);
            assert_eq!(decls[0].name(), "x");
            assert!(decls[0].annotation().is_some());
        }
        other => panic!("Expected Quantified Exists, got {:?}", other),
    }
}

#[test]
fn test_typed_forall_mixed() {
    use rossi::formula::tag::QuantPredOp;

    let source = r#"
    CONTEXT test
    AXIOMS
        @axm1 ∀x⦂ℤ, y · x > y
    END
    "#;

    let ctx = common::parse_context(source);
    match ctx.axioms[0].predicate.kind() {
        rossi::PredicateKind::Quantified { op, decls, .. } => {
            assert_eq!(*op, QuantPredOp::Forall);
            assert_eq!(decls.len(), 2);
            assert_eq!(decls[0].name(), "x");
            assert!(decls[0].annotation().is_some());
            assert_eq!(decls[1].name(), "y");
            assert!(decls[1].annotation().is_none());
        }
        other => panic!("Expected Quantified ForAll, got {:?}", other),
    }
}

// ============================================================================
// A ⦂ annotation must denote a type
// ============================================================================

// Rodin parses the annotation with `MainParsers.TYPE_PARSER`, which reads a
// full expression and then demands `Expression.isATypeExpression`. Inside a
// type every bare name is a given set, so the accepted grammar is `ℤ`, `BOOL`,
// an identifier, `ℙ(τ)`, `τ × τ`, `τ ↔ τ` and parentheses.
#[test_case::test_case("ℤ" ; "integer")]
#[test_case::test_case("BOOL" ; "boolean")]
#[test_case::test_case("S" ; "carrier_set")]
#[test_case::test_case("ℙ(ℤ)" ; "power_set")]
#[test_case::test_case("ℤ×BOOL" ; "product")]
#[test_case::test_case("ℤ↔ℤ" ; "relation")]
#[test_case::test_case("ℙ(S×T)" ; "power_set_of_product")]
#[test_case::test_case("(ℤ×ℤ)" ; "parenthesized")]
fn type_annotations_are_accepted(spelling: &str) {
    parse_predicate_str(&format!("∀x⦂{spelling} · x = x")).expect("a type annotation parses");
}

// Everything else Rodin reports as "Expression doesn't denote a type": `ℕ` and
// `ℙ1` are sets rather than types, the function arrows are not type
// constructors (only `↔` is), and the rest never reach `isATypeExpression`.
#[test_case::test_case("ℤ(1)" ; "application")]
#[test_case::test_case("ℕ" ; "natural")]
#[test_case::test_case("ℙ1(ℤ)" ; "non_empty_power_set")]
#[test_case::test_case("ℤ→ℤ" ; "total_function")]
#[test_case::test_case("ℤ⇸ℤ" ; "partial_function")]
#[test_case::test_case("ℤ∪ℤ" ; "union")]
#[test_case::test_case("{1}" ; "set_extension")]
#[test_case::test_case("1" ; "integer_literal")]
#[test_case::test_case("card(S)" ; "cardinality")]
fn non_type_annotations_are_rejected(spelling: &str) {
    assert_not_a_type(&format!("∀x⦂{spelling} · x = x"));
}

/// Assert that `predicate` is refused for spelling a `⦂` annotation that is
/// not a type. `#[track_caller]` keeps a failure pointing at the case.
#[track_caller]
fn assert_not_a_type(predicate: &str) {
    let error = parse_predicate_str(predicate).expect_err("a non-type annotation is refused");
    assert!(
        matches!(error, ParseError::InvalidTypeExpression { .. }),
        "{predicate}: {error:?}"
    );
}

// `parse_bound_decl` is the one funnel for every binder, so the rejection has
// to reach the λ pattern, the comprehension and the quantified set operators
// as well as ∀/∃.
#[test_case::test_case("∃x⦂ℕ · x = x" ; "exists")]
#[test_case::test_case("(λx⦂ℕ · x = x ∣ x) = f" ; "lambda_pattern")]
#[test_case::test_case("{x⦂ℕ · x = x ∣ x} = s" ; "comprehension")]
#[test_case::test_case("(⋃x⦂ℕ · x = x ∣ {x}) = s" ; "quantified_union")]
#[test_case::test_case("(⋂x⦂ℕ · x = x ∣ {x}) = s" ; "quantified_inter")]
fn every_binder_site_rejects_a_non_type(predicate: &str) {
    assert_not_a_type(predicate);
}

// The ascription operator takes the same type grammar — Rodin runs the very
// same `TYPE_PARSER` from `SubParsers.OftypeParser`. Its *left* operand stays
// lenient, which is a separate divergence.
#[test_case::test_case("∅ ⦂ ℙ(ℤ)" ; "power_set_of_integer")]
#[test_case::test_case("prj1 ⦂ ℙ(ℤ×BOOL×ℤ)" ; "generic_atom")]
fn type_ascriptions_are_accepted(expression: &str) {
    assert!(parses_as_ascription(expression), "{expression}");
}

#[test_case::test_case("∅ ⦂ {1}" ; "set_extension")]
#[test_case::test_case("∅ ⦂ ℕ" ; "natural")]
fn non_type_ascriptions_are_rejected(expression: &str) {
    let error = parse_expression_str(expression).expect_err("a non-type ascription is refused");
    assert!(
        matches!(error, ParseError::InvalidTypeExpression { .. }),
        "{expression}: {error:?}"
    );
}

// The caret sits on the annotation, which is where Rodin reports it too.
#[test]
fn the_rejection_points_at_the_annotation() {
    let source = "∀ x ⦂ ℤ(1) · x = x";
    let error = parse_predicate_str(source).expect_err("refused");
    let ParseError::InvalidTypeExpression { line, column, span } = error else {
        panic!("expected InvalidTypeExpression, got {error:?}");
    };
    assert_eq!((line, column), (1, 7));
    let span = span.expect("the annotation is located");
    assert_eq!(&source[span.start..span.end], "ℤ(1)");
}
