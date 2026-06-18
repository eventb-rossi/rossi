mod common;

use rossi::*;
use test_case::test_case;

// ============================================================================
// Pretty-print assertion tests (individual — custom assertions)
// ============================================================================

#[test]
fn test_pretty_print_simple_context() {
    let source = r#"CONTEXT simple
END
"#;

    let component = parse(source).expect("Failed to parse");
    let output = to_string(&component);

    assert!(output.contains("CONTEXT simple"));
    assert!(output.contains("END"));
}

#[test]
fn test_pretty_print_inverse_operator() {
    // ASCII `~` input round-trips: Unicode mode emits ∼ (U+223C), ASCII mode
    // emits ~ (U+007E).
    let source = common::axiom_context("f, r", "r = f~");
    let component = parse(&source).expect("Failed to parse ASCII ~ inverse");

    let unicode = to_string(&component);
    assert!(
        unicode.contains("f\u{223C}"),
        "Unicode output should use ∼, got: {unicode}"
    );

    let ascii = to_string_ascii(&component);
    assert!(
        ascii.contains("f~"),
        "ASCII output should use ~, got: {ascii}"
    );
    assert!(
        !ascii.contains('\u{223C}'),
        "ASCII output must not contain U+223C"
    );
}

#[test]
fn test_pretty_print_cartesian_product_left_associative() {
    // `×` is left-associative: Rodin renders a left-nested chain bare and
    // only parenthesizes a right-nested child. Regression test for the
    // shared `op_info::set_ops_compatible` table (a missing `(×, ×)` entry
    // made `rossi fmt` emit `(ℤ × ℤ) × ℤ`, diverging from Rodin).
    let left_nested = common::axiom_context("S", "S = ℤ × ℤ × ℤ");
    let out = to_string(&parse(&left_nested).expect("parse left-nested ×"));
    assert!(
        out.contains("ℤ × ℤ × ℤ") && !out.contains("(ℤ × ℤ)"),
        "left-nested × must print bare, got: {out}"
    );

    let right_nested = common::axiom_context("S", "S = ℤ × (ℤ × ℤ)");
    let out = to_string(&parse(&right_nested).expect("parse right-nested ×"));
    assert!(
        out.contains("ℤ × (ℤ × ℤ)"),
        "right-nested × must keep its parentheses, got: {out}"
    );
}

#[test]
fn test_pretty_print_context_with_all_clauses() {
    let source = r#"CONTEXT test_ctx
EXTENDS base_ctx
SETS
    STATUS
CONSTANTS
    max_value
AXIOMS
    @axm1 max_value = 100
    @axm2 max_value > 0
    @thm1 theorem max_value >= 0
END
"#;

    let component = parse(source).expect("Failed to parse");
    let output = to_string(&component);

    assert!(output.contains("CONTEXT test_ctx"));
    assert!(output.contains("EXTENDS"));
    assert!(output.contains("base_ctx"));
    assert!(output.contains("SETS"));
    assert!(output.contains("STATUS"));
    assert!(output.contains("CONSTANTS"));
    assert!(output.contains("max_value"));
    assert!(output.contains("AXIOMS"));
    assert!(output.contains("@axm1"));
    assert!(
        !output.contains("THEOREMS"),
        "Output should not contain THEOREMS keyword — theorems are inline within AXIOMS"
    );
    assert!(output.contains("@thm1"));
    assert!(output.contains("theorem"));
    assert!(output.contains("END"));
}

#[test]
fn test_pretty_print_simple_machine() {
    let source = r#"MACHINE simple
END
"#;

    let component = parse(source).expect("Failed to parse");
    let output = to_string(&component);

    assert!(output.contains("MACHINE simple"));
    assert!(output.contains("END"));
}

#[test]
fn test_pretty_print_machine_with_variables() {
    let source = r#"MACHINE counter
VARIABLES
    count
INVARIANTS
    @inv1 count >= 0
EVENTS
    EVENT INITIALISATION
    THEN
        count := 0
    END
END
"#;

    let component = parse(source).expect("Failed to parse");
    let output = to_string(&component);

    assert!(output.contains("MACHINE counter"));
    assert!(output.contains("VARIABLES"));
    assert!(output.contains("count"));
    assert!(output.contains("INVARIANTS"));
    assert!(output.contains("@inv1"));
    assert!(output.contains("EVENTS"));
    assert!(output.contains("EVENT INITIALISATION"));
    assert!(output.contains("THEN"));
    assert!(output.contains("\u{2254}")); // ≔ COLON EQUALS (Unicode mode)
    assert!(output.contains("END"));
}

#[test]
fn test_pretty_print_event_with_guards() {
    let source = r#"MACHINE counter
VARIABLES
    count
EVENTS
    EVENT INITIALISATION
    THEN
        count := 0
    END

    EVENT increment
    WHERE
        @grd1 count < 100
    THEN
        @act1 count := count + 1
    END
END
"#;

    let component = parse(source).expect("Failed to parse");
    let output = to_string(&component);

    assert!(output.contains("EVENT increment"));
    assert!(output.contains("WHERE"));
    assert!(output.contains("@grd1"));
    assert!(output.contains("THEN"));
    assert!(output.contains("@act1"));
}

#[test]
fn test_pretty_print_event_with_parameters() {
    let source = r#"MACHINE test
VARIABLES
    x
EVENTS
    EVENT INITIALISATION
    THEN
        x := 0
    END

    EVENT add_value
    ANY
        val
    WHERE
        @grd1 val > 0
    THEN
        x := x + val
    END
END
"#;

    let component = parse(source).expect("Failed to parse");
    let output = to_string(&component);

    assert!(output.contains("EVENT add_value"));
    assert!(output.contains("ANY"));
    assert!(output.contains("val"));
    assert!(output.contains("WHERE"));
    assert!(output.contains("THEN"));
}

#[test]
fn test_pretty_print_convergent_event() {
    let source = r#"MACHINE test
VARIABLES
    x
VARIANT
    x
EVENTS
    EVENT INITIALISATION
    THEN
        x := 10
    END

    EVENT decrement
    STATUS convergent
    WHERE
        @grd1 x > 0
    THEN
        x := x - 1
    END
END
"#;

    let component = parse(source).expect("Failed to parse");
    let output = to_string(&component);

    assert!(output.contains("VARIANT"));
    assert!(output.contains("EVENT decrement"));
    assert!(output.contains("convergent EVENT decrement"));
}

#[test]
fn test_pretty_print_expressions() {
    let source = "MACHINE test\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x = 5\nEND\n";

    let component = parse(source).expect("Failed to parse");
    let output = to_string(&component);

    assert!(output.contains("x = 5"));
}

#[test]
fn test_pretty_print_set_operations() {
    let source = r#"CONTEXT test
CONSTANTS
    s1 s2
AXIOMS
    @axm1 s1 = {1, 2, 3}
END
"#;

    let component = parse(source).expect("Failed to parse");
    let output = to_string(&component);

    assert!(output.contains("{"));
    assert!(output.contains("}"));
}

#[test]
fn test_pretty_print_logical_operators() {
    let source = r#"CONTEXT test
CONSTANTS
    x y
AXIOMS
    @axm1 x > 0
    @axm2 y > 0
END
"#;

    let component = parse(source).expect("Failed to parse");
    let output = to_string(&component);

    assert!(output.contains("x > 0"));
    assert!(output.contains("y > 0"));
}

#[test]
fn test_ascii_mode() {
    let source = r#"CONTEXT test
CONSTANTS
    x
AXIOMS
    @axm1 x = 5
END
"#;

    let component = parse(source).expect("Failed to parse");
    let output = to_string_ascii(&component);

    assert!(output.contains("CONTEXT"));
    assert!(output.contains("x = 5"));
}

#[test]
fn test_pretty_print_action_types() {
    let source = r#"MACHINE test
VARIABLES
    x
EVENTS
    EVENT INITIALISATION
    THEN
        x := 0
    END
END
"#;

    let component = parse(source).expect("Failed to parse");
    let output = to_string(&component);

    assert!(output.contains("x \u{2254} 0")); // ≔ COLON EQUALS (Unicode mode)
}

#[test]
fn test_pretty_print_sees_and_refines() {
    let source = r#"MACHINE refined
REFINES
    abstract
SEES
    ctx1
    ctx2
END
"#;

    let component = parse(source).expect("Failed to parse");
    let output = to_string(&component);

    assert!(output.contains("REFINES"));
    assert!(output.contains("abstract"));
    assert!(output.contains("SEES"));
    assert!(output.contains("ctx1"));
    assert!(output.contains("ctx2"));
}

#[test]
fn test_pretty_printer_custom_indent() {
    let source = r#"CONTEXT test
SETS
    STATUS
END
"#;

    let component = parse(source).expect("Failed to parse");
    let printer = PrettyPrinter::new().with_indent("  ".to_string());
    let output = printer.print_component(&component);

    assert!(output.contains("  STATUS"));
}

#[test]
fn test_empty_machine_with_events() {
    let source = r#"MACHINE simple
VARIABLES
    x
EVENTS
    EVENT INITIALISATION
    THEN
        x := 0
    END
END
"#;

    let component = parse(source).expect("Failed to parse");
    let output = to_string(&component);

    assert!(output.contains("MACHINE simple"));
    assert!(output.contains("EVENTS"));
    assert!(output.contains("INITIALISATION"));
}

#[test]
fn test_pretty_print_machine_no_events() {
    let source = r#"MACHINE simple
VARIABLES
    x
INVARIANTS
    @inv1 x >= 0
END
"#;

    let component = parse(source).expect("Failed to parse");
    let output = to_string(&component);

    assert!(output.contains("MACHINE simple"));
    assert!(output.contains("VARIABLES"));
    assert!(output.contains("INVARIANTS"));
    assert!(
        !output.contains("EVENTS"),
        "Output should not contain EVENTS when there are no events"
    );
    assert!(output.contains("END"));
}

// ============================================================================
// Precedence-aware parenthesization tests (individual — AST construction)
// ============================================================================

#[test]
fn test_pretty_print_no_unnecessary_parens_same_prec_left_assoc() {
    // (a + b) + c should print as "a + b + c" (left-associative, no parens needed)
    use rossi::ast::expression::BinaryOp;
    let expr: Expression = ExpressionKind::Binary {
        op: BinaryOp::Add,
        left: Box::new(
            ExpressionKind::Binary {
                op: BinaryOp::Add,
                left: Box::new(ExpressionKind::Identifier("a".into()).into()),
                right: Box::new(ExpressionKind::Identifier("b".into()).into()),
            }
            .into(),
        ),
        right: Box::new(ExpressionKind::Identifier("c".into()).into()),
    }
    .into();
    let output = PrettyPrinter::new().print_expression(&expr);
    assert_eq!(output, "a + b + c");
}

#[test]
fn test_pretty_print_parens_right_child_same_prec() {
    // a + (b + c) must keep parens (right child, left-associative)
    use rossi::ast::expression::BinaryOp;
    let expr: Expression = ExpressionKind::Binary {
        op: BinaryOp::Add,
        left: Box::new(ExpressionKind::Identifier("a".into()).into()),
        right: Box::new(
            ExpressionKind::Binary {
                op: BinaryOp::Add,
                left: Box::new(ExpressionKind::Identifier("b".into()).into()),
                right: Box::new(ExpressionKind::Identifier("c".into()).into()),
            }
            .into(),
        ),
    }
    .into();
    let output = PrettyPrinter::new().print_expression(&expr);
    assert_eq!(output, "a + (b + c)");
}

#[test]
fn test_pretty_print_no_parens_higher_prec_child() {
    // a + b * c should print without parens (multiply is higher precedence)
    use rossi::ast::expression::BinaryOp;
    let expr: Expression = ExpressionKind::Binary {
        op: BinaryOp::Add,
        left: Box::new(ExpressionKind::Identifier("a".into()).into()),
        right: Box::new(
            ExpressionKind::Binary {
                op: BinaryOp::Multiply,
                left: Box::new(ExpressionKind::Identifier("b".into()).into()),
                right: Box::new(ExpressionKind::Identifier("c".into()).into()),
            }
            .into(),
        ),
    }
    .into();
    let output = PrettyPrinter::new().print_expression(&expr);
    assert_eq!(output, "a + b \u{2217} c"); // ∗ ASTERISK OPERATOR
}

#[test]
fn test_pretty_print_parens_lower_prec_child() {
    // (a + b) * c must keep parens (add is lower precedence than multiply)
    use rossi::ast::expression::BinaryOp;
    let expr: Expression = ExpressionKind::Binary {
        op: BinaryOp::Multiply,
        left: Box::new(
            ExpressionKind::Binary {
                op: BinaryOp::Add,
                left: Box::new(ExpressionKind::Identifier("a".into()).into()),
                right: Box::new(ExpressionKind::Identifier("b".into()).into()),
            }
            .into(),
        ),
        right: Box::new(ExpressionKind::Identifier("c".into()).into()),
    }
    .into();
    let output = PrettyPrinter::new().print_expression(&expr);
    assert_eq!(output, "(a + b) \u{2217} c"); // ∗ ASTERISK OPERATOR
}

#[test]
fn test_pretty_print_maplet_right_grouped_needs_parens() {
    // a ↦ (b ↦ c): right child is itself a Maplet, so keep parens
    // (left-associative — same-level Maplet on the right is non-default grouping).
    use rossi::ast::expression::BinaryOp;
    let expr: Expression = ExpressionKind::Binary {
        op: BinaryOp::Maplet,
        left: Box::new(ExpressionKind::Identifier("a".into()).into()),
        right: Box::new(
            ExpressionKind::Binary {
                op: BinaryOp::Maplet,
                left: Box::new(ExpressionKind::Identifier("b".into()).into()),
                right: Box::new(ExpressionKind::Identifier("c".into()).into()),
            }
            .into(),
        ),
    }
    .into();
    let output = PrettyPrinter::new().print_expression(&expr);
    assert_eq!(output, "a \u{21A6} (b \u{21A6} c)");
}

#[test]
fn test_pretty_print_maplet_left_grouped_no_parens() {
    // (a ↦ b) ↦ c: this is the natural left-associative grouping
    // (`a ↦ b ↦ c = (a ↦ b) ↦ c` per spec p.18), so emit flat.
    use rossi::ast::expression::BinaryOp;
    let expr: Expression = ExpressionKind::Binary {
        op: BinaryOp::Maplet,
        left: Box::new(
            ExpressionKind::Binary {
                op: BinaryOp::Maplet,
                left: Box::new(ExpressionKind::Identifier("a".into()).into()),
                right: Box::new(ExpressionKind::Identifier("b".into()).into()),
            }
            .into(),
        ),
        right: Box::new(ExpressionKind::Identifier("c".into()).into()),
    }
    .into();
    let output = PrettyPrinter::new().print_expression(&expr);
    assert_eq!(output, "a \u{21A6} b \u{21A6} c");
}

#[test]
fn test_pretty_print_arrow_inside_maplet_no_parens() {
    // a ↦ (b ↔ c): arrows bind tighter than maplet (kernel_lang Table 3.1),
    // so this is the natural grouping — emit flat.
    use rossi::ast::expression::BinaryOp;
    let expr: Expression = ExpressionKind::Binary {
        op: BinaryOp::Maplet,
        left: Box::new(ExpressionKind::Identifier("a".into()).into()),
        right: Box::new(
            ExpressionKind::Binary {
                op: BinaryOp::Relation,
                left: Box::new(ExpressionKind::Identifier("b".into()).into()),
                right: Box::new(ExpressionKind::Identifier("c".into()).into()),
            }
            .into(),
        ),
    }
    .into();
    let output = PrettyPrinter::new().print_expression(&expr);
    assert_eq!(output, "a \u{21A6} b \u{2194} c");
}

#[test]
fn test_pretty_print_maplet_inside_arrow_needs_parens() {
    // (a ↦ b) ↔ c: maplet binds looser than the arrow, so dropping the
    // parens would re-bind as a ↦ (b ↔ c) — a different AST.
    use rossi::ast::expression::BinaryOp;
    let expr: Expression = ExpressionKind::Binary {
        op: BinaryOp::Relation,
        left: Box::new(
            ExpressionKind::Binary {
                op: BinaryOp::Maplet,
                left: Box::new(ExpressionKind::Identifier("a".into()).into()),
                right: Box::new(ExpressionKind::Identifier("b".into()).into()),
            }
            .into(),
        ),
        right: Box::new(ExpressionKind::Identifier("c".into()).into()),
    }
    .into();
    let output = PrettyPrinter::new().print_expression(&expr);
    assert_eq!(output, "(a \u{21A6} b) \u{2194} c");
}

#[test]
fn test_pretty_print_maplet_arrow_roundtrip() {
    // Both groupings survive a print → reparse cycle unchanged.
    for src in ["a \u{21A6} b \u{2194} c", "(a \u{21A6} b) \u{2194} c"] {
        let parsed = rossi::parse_expression_str(src).expect("parses");
        let printed = PrettyPrinter::new().print_expression(&parsed);
        let reparsed = rossi::parse_expression_str(&printed).expect("reparses");
        assert_eq!(
            parsed, reparsed,
            "roundtrip changed AST for {src} (printed as {printed})"
        );
    }
}

#[test]
fn test_pretty_print_function_application_binary_function_keeps_parens() {
    // (mapping ◁ prj1)(x): the function side is a Binary, so
    // dropping the parens would re-bind as `mapping ◁ prj1(x)`,
    // a different AST. Regression seen on a real-world corpus model.
    use rossi::ast::expression::{AtomicBuiltinKind, BinaryOp};
    let expr: Expression = ExpressionKind::FunctionApplication {
        function: Box::new(
            ExpressionKind::Binary {
                op: BinaryOp::DomainRestriction,
                left: Box::new(ExpressionKind::Identifier("mapping".into()).into()),
                right: Box::new(ExpressionKind::AtomicBuiltin(AtomicBuiltinKind::Prj1).into()),
            }
            .into(),
        ),
        arguments: vec![ExpressionKind::Identifier("x".into()).into()],
    }
    .into();
    let output = PrettyPrinter::new().print_expression(&expr);
    assert_eq!(output, "(mapping \u{25C1} prj1)(x)");
}

#[test]
fn test_pretty_print_inverse_of_atomic_builtin_no_parens() {
    // A bare relational atom is a primary expression, so postfix inverse needs
    // no parens: `prj1∼`, not `(prj1)∼` (it would round-trip either way, but
    // the canonical form must match the minimal one).
    use rossi::ast::expression::{AtomicBuiltinKind, UnaryOp};
    let expr: Expression = ExpressionKind::Unary {
        op: UnaryOp::Inverse,
        operand: Box::new(ExpressionKind::AtomicBuiltin(AtomicBuiltinKind::Prj1).into()),
    }
    .into();
    assert_eq!(PrettyPrinter::new().print_expression(&expr), "prj1\u{223C}");
}

#[test]
fn test_pretty_print_function_application_identifier_function_no_parens() {
    // f(x): the function side is an Identifier, so no parens needed.
    let expr: Expression = ExpressionKind::FunctionApplication {
        function: Box::new(ExpressionKind::Identifier("f".into()).into()),
        arguments: vec![ExpressionKind::Identifier("x".into()).into()],
    }
    .into();
    let output = PrettyPrinter::new().print_expression(&expr);
    assert_eq!(output, "f(x)");
}

#[test]
fn test_pretty_print_mixed_same_prec_left_child() {
    // (a - b) + c should print as "a - b + c" (left child, same prec, left-assoc)
    use rossi::ast::expression::BinaryOp;
    let expr: Expression = ExpressionKind::Binary {
        op: BinaryOp::Add,
        left: Box::new(
            ExpressionKind::Binary {
                op: BinaryOp::Subtract,
                left: Box::new(ExpressionKind::Identifier("a".into()).into()),
                right: Box::new(ExpressionKind::Identifier("b".into()).into()),
            }
            .into(),
        ),
        right: Box::new(ExpressionKind::Identifier("c".into()).into()),
    }
    .into();
    let output = PrettyPrinter::new().print_expression(&expr);
    assert_eq!(output, "a \u{2212} b + c"); // − MINUS SIGN
}

#[test]
fn test_pretty_print_mixed_same_prec_right_child() {
    // a + (b - c) must keep parens (right child, same prec, left-assoc)
    use rossi::ast::expression::BinaryOp;
    let expr: Expression = ExpressionKind::Binary {
        op: BinaryOp::Add,
        left: Box::new(ExpressionKind::Identifier("a".into()).into()),
        right: Box::new(
            ExpressionKind::Binary {
                op: BinaryOp::Subtract,
                left: Box::new(ExpressionKind::Identifier("b".into()).into()),
                right: Box::new(ExpressionKind::Identifier("c".into()).into()),
            }
            .into(),
        ),
    }
    .into();
    let output = PrettyPrinter::new().print_expression(&expr);
    assert_eq!(output, "a + (b \u{2212} c)"); // − MINUS SIGN
}

// ============================================================================
// Camille compatibility class tests (parenthesization)
// ============================================================================

#[test]
fn test_camille_and_or_left_child() {
    // (a ∧ b) ∨ c — And inside Or must keep parens (different compat classes)
    use rossi::ast::predicate::LogicalOp;
    let pred: Predicate = PredicateKind::Logical {
        op: LogicalOp::Or,
        left: Box::new(
            PredicateKind::Logical {
                op: LogicalOp::And,
                left: Box::new(
                    PredicateKind::Comparison {
                        op: rossi::ast::predicate::ComparisonOp::GreaterThan,
                        left: ExpressionKind::Identifier("a".into()).into(),
                        right: ExpressionKind::Integer(0).into(),
                    }
                    .into(),
                ),
                right: Box::new(
                    PredicateKind::Comparison {
                        op: rossi::ast::predicate::ComparisonOp::GreaterThan,
                        left: ExpressionKind::Identifier("b".into()).into(),
                        right: ExpressionKind::Integer(0).into(),
                    }
                    .into(),
                ),
            }
            .into(),
        ),
        right: Box::new(
            PredicateKind::Comparison {
                op: rossi::ast::predicate::ComparisonOp::GreaterThan,
                left: ExpressionKind::Identifier("c".into()).into(),
                right: ExpressionKind::Integer(0).into(),
            }
            .into(),
        ),
    }
    .into();
    let output = PrettyPrinter::new().print_predicate(&pred);
    assert_eq!(output, "(a > 0 ∧ b > 0) ∨ c > 0");
}

#[test]
fn test_camille_or_inside_and() {
    // a ∧ (b ∨ c) — Or inside And must keep parens
    use rossi::ast::predicate::LogicalOp;
    let pred: Predicate = PredicateKind::Logical {
        op: LogicalOp::And,
        left: Box::new(
            PredicateKind::Comparison {
                op: rossi::ast::predicate::ComparisonOp::GreaterThan,
                left: ExpressionKind::Identifier("a".into()).into(),
                right: ExpressionKind::Integer(0).into(),
            }
            .into(),
        ),
        right: Box::new(
            PredicateKind::Logical {
                op: LogicalOp::Or,
                left: Box::new(
                    PredicateKind::Comparison {
                        op: rossi::ast::predicate::ComparisonOp::GreaterThan,
                        left: ExpressionKind::Identifier("b".into()).into(),
                        right: ExpressionKind::Integer(0).into(),
                    }
                    .into(),
                ),
                right: Box::new(
                    PredicateKind::Comparison {
                        op: rossi::ast::predicate::ComparisonOp::GreaterThan,
                        left: ExpressionKind::Identifier("c".into()).into(),
                        right: ExpressionKind::Integer(0).into(),
                    }
                    .into(),
                ),
            }
            .into(),
        ),
    }
    .into();
    let output = PrettyPrinter::new().print_predicate(&pred);
    assert_eq!(output, "a > 0 ∧ (b > 0 ∨ c > 0)");
}

#[test]
fn test_camille_and_chain_same_class() {
    // a ∧ b ∧ c — same class, left-assoc: left child no parens, right child gets parens
    use rossi::ast::predicate::LogicalOp;
    let pred: Predicate = PredicateKind::Logical {
        op: LogicalOp::And,
        left: Box::new(
            PredicateKind::Logical {
                op: LogicalOp::And,
                left: Box::new(
                    PredicateKind::Comparison {
                        op: rossi::ast::predicate::ComparisonOp::GreaterThan,
                        left: ExpressionKind::Identifier("a".into()).into(),
                        right: ExpressionKind::Integer(0).into(),
                    }
                    .into(),
                ),
                right: Box::new(
                    PredicateKind::Comparison {
                        op: rossi::ast::predicate::ComparisonOp::GreaterThan,
                        left: ExpressionKind::Identifier("b".into()).into(),
                        right: ExpressionKind::Integer(0).into(),
                    }
                    .into(),
                ),
            }
            .into(),
        ),
        right: Box::new(
            PredicateKind::Comparison {
                op: rossi::ast::predicate::ComparisonOp::GreaterThan,
                left: ExpressionKind::Identifier("c".into()).into(),
                right: ExpressionKind::Integer(0).into(),
            }
            .into(),
        ),
    }
    .into();
    let output = PrettyPrinter::new().print_predicate(&pred);
    assert_eq!(output, "a > 0 ∧ b > 0 ∧ c > 0");
}

#[test]
fn test_camille_union_difference_incompatible() {
    // (S ∪ T) ∖ U — Union and Difference are in different compat classes
    use rossi::ast::expression::BinaryOp;
    let expr: Expression = ExpressionKind::Binary {
        op: BinaryOp::Difference,
        left: Box::new(
            ExpressionKind::Binary {
                op: BinaryOp::Union,
                left: Box::new(ExpressionKind::Identifier("S".into()).into()),
                right: Box::new(ExpressionKind::Identifier("T".into()).into()),
            }
            .into(),
        ),
        right: Box::new(ExpressionKind::Identifier("U".into()).into()),
    }
    .into();
    let output = PrettyPrinter::new().print_expression(&expr);
    assert_eq!(output, "(S ∪ T) ∖ U");
}

#[test]
fn test_camille_difference_self_incompatible() {
    // S ∖ (T ∖ U) — Difference is class 0, incompatible even with itself
    use rossi::ast::expression::BinaryOp;
    let expr: Expression = ExpressionKind::Binary {
        op: BinaryOp::Difference,
        left: Box::new(ExpressionKind::Identifier("S".into()).into()),
        right: Box::new(
            ExpressionKind::Binary {
                op: BinaryOp::Difference,
                left: Box::new(ExpressionKind::Identifier("T".into()).into()),
                right: Box::new(ExpressionKind::Identifier("U".into()).into()),
            }
            .into(),
        ),
    }
    .into();
    let output = PrettyPrinter::new().print_expression(&expr);
    assert_eq!(output, "S ∖ (T ∖ U)");
}

#[test]
fn test_camille_difference_left_child_also_parens() {
    // (S ∖ T) ∖ U — Difference is class 0 (incompatible with everything, including itself)
    // Per Table 3.2: ∖ row is completely empty, so parens are always required
    use rossi::ast::expression::BinaryOp;
    let expr: Expression = ExpressionKind::Binary {
        op: BinaryOp::Difference,
        left: Box::new(
            ExpressionKind::Binary {
                op: BinaryOp::Difference,
                left: Box::new(ExpressionKind::Identifier("S".into()).into()),
                right: Box::new(ExpressionKind::Identifier("T".into()).into()),
            }
            .into(),
        ),
        right: Box::new(ExpressionKind::Identifier("U".into()).into()),
    }
    .into();
    let output = PrettyPrinter::new().print_expression(&expr);
    assert_eq!(output, "(S ∖ T) ∖ U");
}

#[test]
fn test_camille_mixed_and_or_roundtrip() {
    // Roundtrip: (a ∧ b) ∨ c ∨ d ∨ (e ∧ f)
    let source = r#"CONTEXT test
AXIOMS
    @axm1 (a > 0 ∧ b > 0) ∨ c > 0 ∨ d > 0 ∨ (e > 0 ∧ f > 0)
END
"#;
    common::assert_roundtrip(source);
}

// ============================================================================
// Special roundtrip tests (individual — custom logic, not assert_roundtrip)
// ============================================================================

#[test]
fn test_roundtrip_maplet_comma_comma() {
    // `,,` is an accepted alternative input spelling for the maplet ↦; it
    // round-trips to the canonical ↦, so verify that glyph appears and that
    // the output parses back.
    let source = r#"
MACHINE test
VARIABLES
    r x y
INVARIANTS
    @inv1 r = x ,, y
END
"#;
    let component = parse(source).unwrap();
    let output = to_string(&component);
    assert!(
        output.contains('\u{21A6}'),
        "expected canonical maplet ↦ in output, got:\n{output}"
    );
    let _component2 = parse(&output).unwrap();
}

// ============================================================================
// Roundtrip operator tests (parametrized)
// ============================================================================

#[test_case(r#"CONTEXT simple
SETS
    STATUS
CONSTANTS
    maximum
AXIOMS
    @axm1 maximum = 100
END
"# ; "simple_context")]
#[test_case(r#"MACHINE counter
SEES
    counter_ctx
VARIABLES
    count
INVARIANTS
    @inv1 count >= 0
    @inv2 count <= max_value
EVENTS
    EVENT INITIALISATION
    THEN
        count := 0
    END

    EVENT increment
    WHERE
        @grd1 count < max_value
    THEN
        count := count + 1
    END

    EVENT decrement
    WHERE
        @grd1 count > 0
    THEN
        count := count - 1
    END
END
"# ; "counter_example")]
#[test_case(r#"CONTEXT test
CONSTANTS
    x y z
AXIOMS
    @axm1 x = 5
    @axm2 x > 0
    @axm3 x < 10
    @axm4 x >= 0
    @axm5 x <= 10
END
"# ; "complex_predicates")]
#[test_case(r#"CONTEXT test
CONSTANTS
    a b c d
AXIOMS
    @axm1 a = b + c
    @axm2 a = b - c
    @axm3 a = b * c
END
"# ; "arithmetic_expressions")]
#[test_case(r#"MACHINE simple
VARIABLES
    x
INVARIANTS
    @inv1 x >= 0
END
"# ; "machine_no_events")]
#[test_case("MACHINE test\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x \u{2208} \u{2115} \u{2982} \u{2124}\nEND\n" ; "oftype_unicode")]
fn test_roundtrip_operator(source: &str) {
    common::assert_roundtrip(source);
}

// ============================================================================
// Roundtrip feature tests (parametrized)
// ============================================================================

#[test_case(r#"MACHINE test
REFINES
    abs
VARIABLES
    x
EVENTS
    EVENT INITIALISATION
    THEN
        x := 0
    END

    EVENT update
    REFINES
        abs_update
    WHERE
        @grd1 x < 100
    WITH
        @abs_x abs_x = x
    THEN
        x := x + 1
    END
END
"# ; "with_clause")]
#[test_case(r#"MACHINE test
REFINES
    abs
VARIABLES
    x
EVENTS
    EVENT INITIALISATION
    THEN
        x := 0
    END

    EVENT update
    REFINES
        abs_update
    ANY
        val
    WHERE
        @grd1 val > 0
    WITNESS
        @abs_param val > 0
    THEN
        x := val
    END
END
"# ; "witness_clause")]
#[test_case(r#"
CONTEXT colors
SETS
    COLOR = {red, green, blue}
END
"# ; "enumerated_set")]
#[test_case(r#"
CONTEXT mixed
SETS
    PERSON
    STATUS = {active, inactive}
END
"# ; "mixed_sets")]
#[test_case(r#"
MACHINE test
VARIABLES
    x y
EVENTS
    EVENT INITIALISATION
    THEN
        x, y := 0, 0
    END

    EVENT swap
    THEN
        x, y := y, x
    END
END
"# ; "multiple_parallel_assignment")]
#[test_case(r#"
MACHINE test
VARIABLES
    s
INVARIANTS
    @inv1 s = {x · x ∈ ℕ | x * x}
END
"# ; "extended_set_comprehension")]
#[test_case(r#"
MACHINE test
VARIABLES
    s
INVARIANTS
    @inv1 s = ⋃x · x ∈ ℕ | {x}
END
"# ; "quantified_union")]
#[test_case(r#"
MACHINE test
VARIABLES
    s
INVARIANTS
    @inv1 s = ⋂x · x ∈ ℕ | {x}
END
"# ; "quantified_inter")]
#[test_case("CONTEXT test\nAXIOMS\n    \u{2200}x\u{2982}\u{2124}\u{00B7}x > 0\nEND\n" ; "typed_forall")]
#[test_case("CONTEXT test\nAXIOMS\n    \u{2200}x\u{2982}\u{2124}, y\u{00B7}x > y\nEND\n" ; "typed_forall_mixed")]
#[test_case("CONTEXT test\nAXIOMS\n    \u{2203}x\u{2982}\u{2124}\u{00B7}x = 0\nEND\n" ; "typed_exists")]
fn test_roundtrip_feature(source: &str) {
    common::assert_roundtrip(source);
}

// ============================================================================
// Roundtrip builtin tests (parametrized)
// ============================================================================

#[test_case("CONTEXT test\nAXIOMS\n    @axm1 bool(x > 0) = TRUE\nEND\n" ; "bool_expr")]
fn test_roundtrip_builtin(source: &str) {
    common::assert_roundtrip(source);
}

// ============================================================================
// ASCII roundtrip tests (parametrized)
// ============================================================================

// Variant clause
#[test_case(r#"MACHINE test
VARIABLES
    n
INVARIANTS
    @inv1 n >= 0
VARIANT
    n
EVENTS
    EVENT INITIALISATION
    THEN
        n := 10
    END

    EVENT dec
    STATUS convergent
    WHERE
        @grd1 n > 0
    THEN
        n := n - 1
    END
END
"# ; "variant_clause")]
// Oftype
#[test_case("MACHINE test\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x \u{2208} \u{2115} \u{2982} \u{2124}\nEND\n" ; "oftype")]
// Typed identifiers in quantifiers
#[test_case("CONTEXT test\nAXIOMS\n    \u{2200}x\u{2982}\u{2124}\u{00B7}x > 0\nEND\n" ; "typed_forall")]
// Bool (not in proptest)
#[test_case("CONTEXT test\nAXIOMS\n    @axm1 bool(x > 0) = TRUE\nEND\n" ; "bool_expr")]
fn test_roundtrip_ascii(source: &str) {
    common::assert_roundtrip_ascii(source);
}

#[test]
fn test_set_comprehension_basic_unicode_bar() {
    let source = "MACHINE M\nEVENTS\n    EVENT e\n    THEN\n        @act1 v ≔ {x ∣ x ∈ S ∧ x ≠ 0}\n    END\nEND\n";
    let component = parse(source).unwrap();
    let output = to_string(&component);
    assert!(
        output.contains("∣"),
        "Basic set comprehension should use Unicode ∣, got: {}",
        output
    );
    assert!(
        !output.contains('|'),
        "Basic set comprehension should not contain ASCII |, got: {}",
        output
    );
}
