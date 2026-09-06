mod common;

use rossi::*;
use test_case::test_case;

// ============================================================================
// Pretty-print assertion tests (individual — custom assertions)
// ============================================================================

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

    assert!(output.contains("context test_ctx extends base_ctx"));
    assert!(output.contains("sets STATUS"));
    assert!(output.contains("constants max_value"));
    assert!(output.contains("axioms"));
    assert!(output.contains("@axm1"));
    assert!(
        !output.to_lowercase().contains("theorems"),
        "Output should not contain a THEOREMS keyword — theorems are inline within AXIOMS"
    );
    assert!(output.contains("theorem @thm1"));
    assert!(output.ends_with("end\n"));
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
        @act1 x := 10
    END

    EVENT decrement
    STATUS convergent
    WHERE
        @grd1 x > 0
    THEN
        @act1 x := x - 1
    END
END
"#;

    let component = parse(source).expect("Failed to parse");
    let output = to_string(&component);

    assert!(output.contains("variant x"));
    assert!(output.contains("convergent event decrement"));
}

#[test]
fn test_pretty_print_parallel_assignment_keeps_all_pairs() {
    let action = parse_action_str("x, y := 1, 2").expect("parallel assignment parses");
    assert_eq!(
        PrettyPrinter::new().print_action_body(&action),
        "x, y ≔ 1, 2"
    );
    assert_eq!(
        PrettyPrinter::ascii().print_action_body(&action),
        "x, y := 1, 2"
    );
}

#[test]
fn rodin_canonical_binary_spacing_is_exhaustive() {
    use rossi::formula::tag::{AssocExprOp, BinaryExprOp};
    use rossi::operators::OperatorId;

    // Every infix expression operator, keyed by the operator table's id so
    // the expected spelling cannot drift from the table. FunImage/RelImage
    // print as application forms, not infix, so they have no row.
    let assoc_cases = [
        (AssocExprOp::Plus, OperatorId::Add, true),
        (AssocExprOp::Mul, OperatorId::Multiply, true),
        (AssocExprOp::BUnion, OperatorId::Union, true),
        (AssocExprOp::BInter, OperatorId::Intersection, true),
        (AssocExprOp::Ovr, OperatorId::Overwrite, true),
        (AssocExprOp::FComp, OperatorId::Semicolon, false),
        (AssocExprOp::BComp, OperatorId::Composition, false),
    ];
    let binary_cases = [
        (BinaryExprOp::Minus, OperatorId::Subtract, false),
        (BinaryExprOp::Div, OperatorId::Divide, false),
        (BinaryExprOp::Mod, OperatorId::Modulo, false),
        (BinaryExprOp::Expn, OperatorId::Exponent, false),
        (BinaryExprOp::UpTo, OperatorId::Range, false),
        (BinaryExprOp::SetMinus, OperatorId::Difference, false),
        (BinaryExprOp::CProd, OperatorId::CartesianProduct, true),
        (BinaryExprOp::Rel, OperatorId::Relation, false),
        (BinaryExprOp::TRel, OperatorId::TotalRelation, false),
        (BinaryExprOp::SRel, OperatorId::SurjectiveRelation, false),
        (
            BinaryExprOp::STRel,
            OperatorId::TotalSurjectiveRelation,
            false,
        ),
        (BinaryExprOp::TFun, OperatorId::TotalFunction, false),
        (BinaryExprOp::PFun, OperatorId::PartialFunction, false),
        (BinaryExprOp::TInj, OperatorId::TotalInjection, false),
        (BinaryExprOp::PInj, OperatorId::PartialInjection, false),
        (BinaryExprOp::TSur, OperatorId::TotalSurjection, false),
        (BinaryExprOp::PSur, OperatorId::PartialSurjection, false),
        (BinaryExprOp::TBij, OperatorId::Bijection, false),
        (BinaryExprOp::DomRes, OperatorId::DomainRestriction, false),
        (BinaryExprOp::DomSub, OperatorId::DomainSubtraction, false),
        (BinaryExprOp::RanRes, OperatorId::RangeRestriction, false),
        (BinaryExprOp::RanSub, OperatorId::RangeSubtraction, false),
        (BinaryExprOp::DProd, OperatorId::DirectProduct, false),
        (BinaryExprOp::PProd, OperatorId::ParallelProduct, false),
        (BinaryExprOp::Mapsto, OperatorId::Maplet, false),
    ];
    let printer = PrettyPrinter::rodin_canonical();
    let check = |expression: Expression, op_id: OperatorId, tight: bool| {
        let operator = operators::spell(op_id, true);
        let separator = if tight { "" } else { " " };
        assert_eq!(
            printer.print_formula_expression(&expression),
            format!("a{separator}{operator}{separator}b"),
            "wrong Rodin spacing for {op_id:?}"
        );
    };

    for (op, op_id, tight) in assoc_cases {
        check(assoc(op, vec![id("a"), id("b")]), op_id, tight);
    }
    for (op, op_id, tight) in binary_cases {
        check(bin(op, id("a"), id("b")), op_id, tight);
    }
    check(
        mff().ascription(id("a"), id("b"), None),
        OperatorId::OfType,
        false,
    );
}

#[test]
fn rodin_canonical_tightens_comparisons_and_logical_operators() {
    use rossi::formula::tag::{AssocPredOp, BinaryPredOp, RelationalOp};
    use rossi::operators::OperatorId;

    let comparison_cases = [
        (RelationalOp::Equal, OperatorId::Equal),
        (RelationalOp::NotEqual, OperatorId::NotEqual),
        (RelationalOp::Lt, OperatorId::LessThan),
        (RelationalOp::Le, OperatorId::LessEqual),
        (RelationalOp::Gt, OperatorId::GreaterThan),
        (RelationalOp::Ge, OperatorId::GreaterEqual),
        (RelationalOp::In, OperatorId::In),
        (RelationalOp::NotIn, OperatorId::NotIn),
        (RelationalOp::SubsetEq, OperatorId::Subset),
        (RelationalOp::Subset, OperatorId::SubsetStrict),
        (RelationalOp::NotSubsetEq, OperatorId::NotSubset),
        (RelationalOp::NotSubset, OperatorId::NotSubsetStrict),
    ];
    let printer = PrettyPrinter::rodin_canonical();

    for (op, op_id) in comparison_cases {
        let predicate = mff().relational_predicate(op, id("a"), id("b"), None);
        let operator = operators::spell(op_id, true);
        assert_eq!(
            printer.print_formula_predicate(&predicate),
            format!("a{operator}b"),
            "wrong Rodin spacing for {op_id:?}"
        );
    }

    let comparison = |left: &str, right: &str| {
        mff().relational_predicate(RelationalOp::Equal, id(left), id(right), None)
    };
    let logical_cases = [
        (
            passoc(
                AssocPredOp::LAnd,
                vec![comparison("a", "b"), comparison("c", "d")],
            ),
            OperatorId::And,
        ),
        (
            passoc(
                AssocPredOp::LOr,
                vec![comparison("a", "b"), comparison("c", "d")],
            ),
            OperatorId::Or,
        ),
        (
            mff().binary_predicate(
                BinaryPredOp::LImp,
                comparison("a", "b"),
                comparison("c", "d"),
                None,
            ),
            OperatorId::Implies,
        ),
        (
            mff().binary_predicate(
                BinaryPredOp::LEqv,
                comparison("a", "b"),
                comparison("c", "d"),
                None,
            ),
            OperatorId::Equivalent,
        ),
    ];
    for (predicate, op_id) in logical_cases {
        let operator = operators::spell(op_id, true);
        assert_eq!(
            printer.print_formula_predicate(&predicate),
            format!("a=b{operator}c=d"),
            "wrong Rodin spacing for {op_id:?}"
        );
    }
}

#[test]
fn rodin_canonical_ascii_keeps_word_operator_boundaries() {
    let predicate = parse_predicate_str("a = b or c = d").expect("predicate parses");
    let printer = PrettyPrinter::ascii().with_formula_spacing(FormulaSpacing::RodinCanonical);
    let printed = printer.print_formula_predicate(&predicate);

    assert_eq!(printed, "a=b or c=d");
    assert_eq!(
        parse_predicate_str(&printed).expect("printed predicate reparses"),
        predicate
    );
}

#[test]
fn rodin_canonical_preserves_root_specific_type_ascription_spacing() {
    let printer = PrettyPrinter::rodin_canonical();
    let expression = parse_expression_str("a ⦂ b").expect("expression parses");
    let predicate = parse_predicate_str("a ⦂ b = c").expect("predicate parses");
    let bool_expression = parse_expression_str("bool(a ⦂ b = c)").expect("bool parses");
    let action = parse_action_str("x ≔ a ⦂ b").expect("action parses");

    assert_eq!(printer.print_formula_expression(&expression), "a ⦂ b");
    assert_eq!(printer.print_formula_predicate(&predicate), "a⦂b=c");
    assert_eq!(
        printer.print_formula_expression(&bool_expression),
        "bool(a ⦂ b=c)"
    );
    assert_eq!(printer.print_action_body(&action), "x ≔ a ⦂ b");
}

#[test]
fn rodin_canonical_tightens_comma_separated_formula_lists() {
    let printer = PrettyPrinter::rodin_canonical();
    let expression = parse_expression_str("{1, 2, 3}").expect("enumeration parses");
    let predicate = parse_predicate_str("∀x⦂ℤ, y⦂ℤ · p(x, y)").expect("predicate parses");
    let action = parse_action_str("x, y ≔ 1, 2").expect("action parses");

    assert_eq!(printer.print_formula_expression(&expression), "{1,2,3}");
    assert_eq!(
        printer.print_formula_predicate(&predicate),
        "∀x⦂ℤ,y⦂ℤ·p(x,y)"
    );
    assert_eq!(printer.print_action_body(&action), "x,y ≔ 1,2");

    assert_eq!(
        PrettyPrinter::new().print_formula_expression(&expression),
        "{1, 2, 3}"
    );
    assert_eq!(
        PrettyPrinter::new().print_action_body(&action),
        "x, y ≔ 1, 2"
    );
}

#[test]
fn test_pretty_printer_custom_indent() {
    let source = r#"CONTEXT test
AXIOMS
    @axm1 1 = 1
END
"#;

    let component = parse(source).expect("Failed to parse");
    let printer = PrettyPrinter::new().with_indent("   ".to_string());
    let output = printer.print_component(&component);

    assert!(output.contains("\n   @axm1"), "got:\n{output}");
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

    assert!(output.contains("machine simple"));
    assert!(output.contains("variables x"));
    assert!(output.contains("invariants"));
    assert!(
        !output.contains("events"),
        "Output should not contain an events clause when there are no events"
    );
    assert!(output.ends_with("end\n"));
}

// ============================================================================
// Precedence-aware parenthesization tests (parametrized — AST construction)
//
// Each row pins the canonical *minimal* parenthesization, which the roundtrip
// properties cannot check (over-parenthesized output also roundtrips). The
// shared body additionally reparses the printed form back to the same AST, so
// every canonical form is also proven parser-stable.
// ============================================================================

// The rows build formula-model trees through the factory — the shapes the
// parser itself produces (same-operator chains are one n-ary node, so the
// legacy "right-nested same operator keeps parens" cases have no model
// equivalent). The shared body reparses the printed form and compares the
// trees directly.

fn mff() -> rossi::formula::FormulaFactory {
    rossi::formula::FormulaFactory::default_factory()
}

fn id(name: &str) -> Expression {
    mff().free_identifier(name, None, None)
}

fn bin(op: rossi::formula::tag::BinaryExprOp, left: Expression, right: Expression) -> Expression {
    mff().binary_expression(op, left, right, None)
}

fn un(op: rossi::formula::tag::UnaryExprOp, child: Expression) -> Expression {
    mff().unary_expression(op, child, None)
}

fn assoc(op: rossi::formula::tag::AssocExprOp, children: Vec<Expression>) -> Expression {
    mff().associative_expression(op, children, None)
}

/// `<name> > 0` — comparison leaf for the logical-operator rows.
fn gt0(name: &str) -> Predicate {
    mff().relational_predicate(
        rossi::formula::tag::RelationalOp::Gt,
        id(name),
        mff().integer_literal(0, None),
        None,
    )
}

fn passoc(op: rossi::formula::tag::AssocPredOp, children: Vec<Predicate>) -> Predicate {
    mff().associative_predicate(op, children, None)
}

use rossi::formula::tag::{AssocExprOp as AOp, AssocPredOp as POp, BinaryExprOp as BOp};

// a + b + c — one n-ary sum prints flat.
#[test_case(
    assoc(AOp::Plus, vec![id("a"), id("b"), id("c")]),
    "a + b + c";
    "assoc_chain_prints_flat"
)]
// a + b ∗ c prints flat: multiply is higher precedence. (∗ ASTERISK OPERATOR)
#[test_case(
    assoc(AOp::Plus, vec![id("a"), assoc(AOp::Mul, vec![id("b"), id("c")])]),
    "a + b \u{2217} c";
    "higher_prec_child_no_parens"
)]
// (a + b) ∗ c keeps parens: add is lower precedence than multiply.
#[test_case(
    assoc(AOp::Mul, vec![assoc(AOp::Plus, vec![id("a"), id("b")]), id("c")]),
    "(a + b) \u{2217} c";
    "lower_prec_child_parens"
)]
// a − b + c prints flat: leading subtraction, same precedence. (− MINUS SIGN)
#[test_case(
    assoc(AOp::Plus, vec![bin(BOp::Minus, id("a"), id("b")), id("c")]),
    "a \u{2212} b + c";
    "mixed_same_prec_left_child"
)]
// a + (b − c) keeps parens: a same-precedence non-first operand.
#[test_case(
    assoc(AOp::Plus, vec![id("a"), bin(BOp::Minus, id("b"), id("c"))]),
    "a + (b \u{2212} c)";
    "mixed_same_prec_right_child"
)]
// a ↦ (b ↦ c): right child is itself a Maplet, so keep parens
// (left-associative — same-level Maplet on the right is non-default grouping).
#[test_case(
    bin(BOp::Mapsto, id("a"), bin(BOp::Mapsto, id("b"), id("c"))),
    "a \u{21A6} (b \u{21A6} c)";
    "maplet_right_grouped_parens"
)]
// (a ↦ b) ↦ c: the natural left-associative grouping
// (`a ↦ b ↦ c = (a ↦ b) ↦ c` per spec p.18), so emit flat.
#[test_case(
    bin(BOp::Mapsto, bin(BOp::Mapsto, id("a"), id("b")), id("c")),
    "a \u{21A6} b \u{21A6} c";
    "maplet_left_grouped_no_parens"
)]
// a ↦ (b ↔ c): arrows bind tighter than maplet (kernel_lang Table 3.1;
// regression context: c3e3cae), so this is the natural grouping — emit flat.
#[test_case(
    bin(BOp::Mapsto, id("a"), bin(BOp::Rel, id("b"), id("c"))),
    "a \u{21A6} b \u{2194} c";
    "arrow_inside_maplet_no_parens"
)]
// (a ↦ b) ↔ c: maplet binds looser than the arrow (kernel_lang Table 3.1;
// regression context: c3e3cae) — dropping the parens would re-bind as
// a ↦ (b ↔ c), a different AST.
#[test_case(
    bin(BOp::Rel, bin(BOp::Mapsto, id("a"), id("b")), id("c")),
    "(a \u{21A6} b) \u{2194} c";
    "maplet_inside_arrow_parens"
)]
// (S ∪ T) ∖ U — Union and Difference are in different Camille compatibility
// classes, so parens are required.
#[test_case(
    bin(
        BOp::SetMinus,
        assoc(AOp::BUnion, vec![id("S"), id("T")]),
        id("U")
    ),
    "(S ∪ T) ∖ U";
    "union_difference_incompatible"
)]
// S ∖ (T ∖ U) — Difference is Camille class 0, incompatible even with itself.
#[test_case(
    bin(BOp::SetMinus, id("S"), bin(BOp::SetMinus, id("T"), id("U"))),
    "S ∖ (T ∖ U)";
    "difference_self_incompatible"
)]
// (S ∖ T) ∖ U — per Table 3.2 the ∖ row is completely empty, so parens are
// always required, even for the left child.
#[test_case(
    bin(BOp::SetMinus, bin(BOp::SetMinus, id("S"), id("T")), id("U")),
    "(S ∖ T) ∖ U";
    "difference_left_child_parens"
)]
// prj1∼, not (prj1)∼: a bare relational atom is a primary expression, so
// postfix inverse needs no parens. It would round-trip either way, but the
// canonical form must match the minimal one.
#[test_case(
    un(
        rossi::formula::tag::UnaryExprOp::Converse,
        mff().atomic_expression(rossi::formula::tag::AtomicOp::KPrj1Gen, None, None)
    ),
    "prj1\u{223C}";
    "inverse_of_atomic_builtin_no_parens"
)]
fn test_pretty_print_canonical_expression(expr: Expression, expected: &str) {
    let printed = PrettyPrinter::new().print_formula_expression(&expr);
    assert_eq!(printed, expected);
    assert_eq!(
        parse_expression_str(&printed).expect("canonical form reparses"),
        expr,
        "reparsing the canonical form changed the AST"
    );
}

// (a > 0 ∧ b > 0) ∨ c > 0 — And inside Or keeps parens (different Camille
// compatibility classes).
#[test_case(
    passoc(
        POp::LOr,
        vec![passoc(POp::LAnd, vec![gt0("a"), gt0("b")]), gt0("c")]
    ),
    "(a > 0 ∧ b > 0) ∨ c > 0";
    "and_or_left_child"
)]
// a > 0 ∧ (b > 0 ∨ c > 0) — Or inside And keeps parens.
#[test_case(
    passoc(
        POp::LAnd,
        vec![gt0("a"), passoc(POp::LOr, vec![gt0("b"), gt0("c")])]
    ),
    "a > 0 ∧ (b > 0 ∨ c > 0)";
    "or_inside_and"
)]
// a > 0 ∧ b > 0 ∧ c > 0 — one n-ary conjunction prints flat.
#[test_case(
    passoc(POp::LAnd, vec![gt0("a"), gt0("b"), gt0("c")]),
    "a > 0 ∧ b > 0 ∧ c > 0";
    "and_chain_same_class"
)]
fn test_pretty_print_canonical_predicate(pred: Predicate, expected: &str) {
    let printed = PrettyPrinter::new().print_formula_predicate(&pred);
    assert_eq!(printed, expected);
    assert_eq!(
        parse_predicate_str(&printed).expect("canonical form reparses"),
        pred,
        "reparsing the canonical form changed the AST"
    );
}

#[test]
fn test_pretty_print_function_application_binary_function_keeps_parens() {
    // (mapping ◁ prj1)(x): the function side is a Binary, so
    // dropping the parens would re-bind as `mapping ◁ prj1(x)`,
    // a different AST. Regression seen on a real-world corpus model.
    let expr = bin(
        BOp::FunImage,
        bin(
            BOp::DomRes,
            id("mapping"),
            mff().atomic_expression(rossi::formula::tag::AtomicOp::KPrj1Gen, None, None),
        ),
        id("x"),
    );
    let output = PrettyPrinter::new().print_formula_expression(&expr);
    assert_eq!(output, "(mapping \u{25C1} prj1)(x)");
}

#[test]
fn test_single_argument_application_ast_roundtrips() {
    let applications: [(Expression, &str); 2] = [
        (bin(BOp::FunImage, id("f"), id("x")), "f(x)"),
        (
            un(rossi::formula::tag::UnaryExprOp::KCard, id("x")),
            "card(x)",
        ),
    ];

    for (application, expected) in applications {
        let printed = PrettyPrinter::new().print_formula_expression(&application);
        assert_eq!(printed, expected);
        assert_eq!(rossi::parse_expression_str(&printed).unwrap(), application);
    }
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

#[test]
fn test_theorems_section_roundtrips_to_inline() {
    // The canonical printed form is inline `theorem @x` (Rodin parity), so a parsed
    // THEOREMS section normalizes to inline and re-parses to the same flagged rows.
    let source = r#"
    CONTEXT test
    AXIOMS
        @axm1 1 = 1
    THEOREMS
        @thm1 2 = 2
    END
    "#;

    let component = parse(source).expect("should parse");
    let printed = rossi::to_string(&component);
    assert!(!printed.contains("THEOREMS"), "output normalizes to inline");
    assert!(printed.contains("theorem @thm1"));

    let Component::Context(reparsed) = parse(&printed).expect("reparse") else {
        panic!("expected a Context");
    };
    assert_eq!(reparsed.axioms.len(), 2);
    assert!(reparsed.axioms.iter().any(|a| a.is_theorem));
}

#[test]
fn test_skip_action_roundtrip() {
    let source = r#"MACHINE test
EVENTS
    EVENT foo
    THEN
        @act1 skip
    END
END
"#;
    let mut component = rossi::parse(source).expect("Failed to parse");
    let output = rossi::to_string(&component);
    assert!(
        output.contains("skip"),
        "Pretty-printed output should contain 'skip'"
    );
    // Parse again and compare (clear spans since source positions differ after pretty-print)
    let mut component2 = rossi::parse(&output).expect("Failed to re-parse pretty output");
    common::clear_spans(&mut component);
    common::clear_spans(&mut component2);
    assert_eq!(
        component, component2,
        "Roundtrip should produce identical AST"
    );
}

#[test]
fn test_extended_initialisation_no_actions_roundtrip() {
    let source = indoc::indoc! {"
        MACHINE m1
        REFINES
            m0
        EVENTS
            EVENT INITIALISATION extends INITIALISATION
            END
        END
    "};
    let mut component = rossi::parse(source).expect("Failed to parse");
    let output = rossi::to_string(&component);
    let mut component2 = rossi::parse(&output).expect("Failed to re-parse pretty output");
    common::clear_spans(&mut component);
    common::clear_spans(&mut component2);
    assert_eq!(
        component, component2,
        "Extended init with no actions should roundtrip"
    );
}

// ============================================================================
// Roundtrip example tests
// ============================================================================

// `skip` carries the only structural span an action body can own, so a machine
// using it is the case that proves `clear_spans` reaches that span; the
// generative round-trips never emit `skip`.
#[test]
fn skip_action_roundtrips() {
    common::assert_roundtrip(
        r#"MACHINE m
VARIABLES
    x
INVARIANTS
    @inv1 x ∈ ℕ
EVENTS
    EVENT INITIALISATION
    THEN
        @act1 x ≔ 0
    END

    EVENT idle
    THEN
        @act1 skip
    END
END
"#,
    );
}

// Kept as a readable pinned example of machine-level REFINES with event-level
// REFINES/WITH; generative cover: machine_roundtrip_* in proptest_roundtrip.rs.
#[test]
fn test_roundtrip_refines_with_clause() {
    common::assert_roundtrip(
        r#"MACHINE test
REFINES
    abs
VARIABLES
    x
EVENTS
    EVENT INITIALISATION
    THEN
        @act1 x := 0
    END

    EVENT update
    REFINES
        abs_update
    WHERE
        @grd1 x < 100
    WITH
        @abs_x abs_x = x
    THEN
        @act1 x := x + 1
    END
END
"#,
    );
}

// Roundtrip cases pinned in both Unicode and ASCII printer modes (parametrized)

// Oftype (⦂)
#[test_case("MACHINE test\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x \u{2208} \u{2115} \u{2982} \u{2124}\nEND\n" ; "oftype")]
// Typed-forall axiom: `arb_label` generates no unlabeled predicate and the
// property test skips ASCII mode, so both modes stay pinned here.
#[test_case("CONTEXT test\nAXIOMS\n    @axm1 \u{2200}x\u{2982}\u{2124}\u{00B7}x > 0\nEND\n" ; "typed_forall")]
fn test_roundtrip_both_modes(source: &str) {
    common::assert_roundtrip(source);
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

#[test]
fn private_use_glyphs_flag_controls_relation_and_override_spelling() {
    let pred = parse_predicate_str("r ∈ A <<-> B ∧ s = f <+ g").expect("parses");

    // Default printer is portable: the private-use operators emit ASCII.
    let portable = PrettyPrinter::new().print_formula_predicate(&pred);
    assert!(
        portable.contains("<<->") && portable.contains("<+"),
        "default printer should emit ASCII for private-use operators, got: {portable}"
    );
    assert!(
        !portable.contains('\u{E100}') && !portable.contains('\u{E103}'),
        "default printer must not emit a private-use glyph, got: {portable}"
    );

    // Opting in reproduces Rodin's internal spelling (the raw glyphs).
    let glyphs = PrettyPrinter::new()
        .with_private_use_glyphs(true)
        .print_formula_predicate(&pred);
    assert!(
        glyphs.contains('\u{E100}') && glyphs.contains('\u{E103}'),
        "with_private_use_glyphs(true) should emit the glyphs, got: {glyphs}"
    );

    // The user-facing paths (CLI `fmt`/`import`, LSP formatting) build their
    // printer through `resolved`, so the override has to survive that.
    let resolved = PrettyPrinter::resolved(
        Style::Camille,
        &StyleOverrides {
            private_use_glyphs: true,
            ..StyleOverrides::default()
        },
    )
    .print_formula_predicate(&pred);
    assert_eq!(resolved, glyphs);

    // Under the ASCII convention the override changes nothing.
    let ascii = PrettyPrinter::resolved(
        Style::Camille,
        &StyleOverrides {
            use_unicode: false,
            private_use_glyphs: true,
            ..StyleOverrides::default()
        },
    )
    .print_formula_predicate(&pred);
    assert!(
        ascii.contains("<<->") && ascii.contains("<+"),
        "the ASCII convention keeps the ASCII spelling, got: {ascii}"
    );
}

// ============================================================================
// Legacy (rossi-style) layout regression golden
// ============================================================================

/// Byte-exact golden for `styled(Style::Rossi)` — the layout that was the
/// default before camille. `--style rossi` output must never drift.
#[test]
fn rossi_style_golden_is_byte_stable() {
    let source = "MACHINE m1 REFINES m0 SEES c1 c2\n\
                  VARIABLES a b\n\
                  INVARIANTS\n@inv1 a : NAT\ntheorem @thm1 a >= 0\n\
                  VARIANT a\n\
                  EVENTS\n\
                  EVENT INITIALISATION THEN @act1 a := 0 END\n\
                  convergent EVENT step REFINES step ANY p WHERE @grd1 p < a THEN @act1 a := p END\n\
                  END\n";
    let expected = "MACHINE m1\n\
                    REFINES\n\
                    \x20\x20\x20\x20m0\n\
                    SEES\n\
                    \x20\x20\x20\x20c1\n\
                    \x20\x20\x20\x20c2\n\
                    VARIABLES\n\
                    \x20\x20\x20\x20a\n\
                    \x20\x20\x20\x20b\n\
                    INVARIANTS\n\
                    \x20\x20\x20\x20@inv1 a ∈ ℕ\n\
                    \x20\x20\x20\x20theorem @thm1 a ≥ 0\n\
                    VARIANT\n\
                    \x20\x20\x20\x20a\n\
                    EVENTS\n\
                    \x20\x20\x20\x20EVENT INITIALISATION\n\
                    \x20\x20\x20\x20THEN\n\
                    \x20\x20\x20\x20\x20\x20\x20\x20@act1 a ≔ 0\n\
                    \x20\x20\x20\x20END\n\
                    \n\
                    \x20\x20\x20\x20convergent EVENT step\n\
                    \x20\x20\x20\x20REFINES\n\
                    \x20\x20\x20\x20\x20\x20\x20\x20step\n\
                    \x20\x20\x20\x20ANY\n\
                    \x20\x20\x20\x20\x20\x20\x20\x20p\n\
                    \x20\x20\x20\x20WHERE\n\
                    \x20\x20\x20\x20\x20\x20\x20\x20@grd1 p < a\n\
                    \x20\x20\x20\x20THEN\n\
                    \x20\x20\x20\x20\x20\x20\x20\x20@act1 a ≔ p\n\
                    \x20\x20\x20\x20END\n\
                    END\n";
    let printer = PrettyPrinter::styled(Style::Rossi);
    let output = format_str(source, &printer).expect("format");
    assert_eq!(output, expected);
}
