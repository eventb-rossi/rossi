//! Integration tests for parsing complete Event-B models

mod common;

use rossi::formula::tag::AtomicOp;
use rossi::{Component, ExpressionKind, ParseError, PredicateKind, parse};

#[test]
fn test_counter_context() {
    let source = r#"
    CONTEXT counter_ctx
    SETS
        STATUS
    CONSTANTS
        max_value
    AXIOMS
        @axm1 max_value = 100
    END
    "#;

    let ctx = common::parse_context(source);
    assert_eq!(ctx.name, "counter_ctx");
    assert_eq!(ctx.sets.len(), 1);
    assert_eq!(ctx.sets[0].name, "STATUS");
    assert_eq!(ctx.constants.len(), 1);
    assert_eq!(ctx.constants[0].name, "max_value");
    assert_eq!(ctx.axioms.len(), 1);
    assert!(ctx.extends.is_empty(), "omitted EXTENDS defaults to empty");
}

#[test]
fn test_counter_machine() {
    let source = r#"
    MACHINE counter
    SEES
        counter_ctx
    VARIABLES
        count
    INVARIANTS
        @inv1 count >= 0
    EVENTS
        EVENT INITIALISATION
        THEN
            @act1 count := 0
        END

        EVENT increment
        WHERE
            @grd1 count < 100
        THEN
            @act1 count := count + 1
        END
    END
    "#;

    let m = common::parse_machine(source);
    assert_eq!(m.name, "counter");
    assert_eq!(m.sees.len(), 1);
    assert_eq!(m.sees[0], "counter_ctx");
    assert_eq!(m.variables.len(), 1);
    assert_eq!(m.variables[0].name, "count");
    assert_eq!(m.invariants.len(), 1);
    assert!(m.initialisation.is_some());
    assert_eq!(m.events.len(), 1);
    assert_eq!(m.events[0].name, "increment");
}

#[test]
fn test_context_extends() {
    let source = r#"
    CONTEXT child
    EXTENDS
        parent1 parent2
    END
    "#;

    let ctx = common::parse_context(source);
    assert_eq!(ctx.name, "child");
    assert_eq!(ctx.extends.len(), 2);
    assert_eq!(ctx.extends[0], "parent1");
    assert_eq!(ctx.extends[1], "parent2");
    assert!(ctx.sets.is_empty(), "omitted SETS defaults to empty");
    assert!(
        ctx.constants.is_empty(),
        "omitted CONSTANTS defaults to empty"
    );
}

#[test]
fn test_machine_refines() {
    let source = r#"
    MACHINE refined
    REFINES
        abstract
    END
    "#;

    let m = common::parse_machine(source);
    assert_eq!(m.name, "refined");
    assert_eq!(m.refines, Some("abstract".to_string()));
}

#[test]
fn test_event_with_parameters() {
    let source = r#"
    MACHINE test
    VARIABLES
        x
    EVENTS
        EVENT INITIALISATION
        THEN
            @act1 x := 0
        END

        EVENT update
        ANY
            val
        WHERE
            @grd1 val > 0
        THEN
            @act1 x := val
        END
    END
    "#;

    let m = common::parse_machine(source);
    assert_eq!(m.events.len(), 1);
    let event = &m.events[0];
    assert_eq!(event.name, "update");
    assert_eq!(event.parameters.len(), 1);
    assert_eq!(event.parameters[0].name, "val");
    assert_eq!(event.guards.len(), 1);
    assert_eq!(event.guards[0].label.as_deref(), Some("grd1"));
}

#[test]
fn test_multiple_variables_and_invariants() {
    let source = r#"
    MACHINE multi
    VARIABLES
        x y z
    INVARIANTS
        @inv1 x >= 0
        @inv2 y >= 0
        @inv3 z = x + y
    END
    "#;

    let m = common::parse_machine(source);
    assert_eq!(m.variables.len(), 3);
    assert_eq!(m.variables[0].name, "x");
    assert_eq!(m.variables[1].name, "y");
    assert_eq!(m.variables[2].name, "z");
    assert_eq!(m.invariants.len(), 3);
}

#[test]
fn test_event_with_clause() {
    let source = r#"
    MACHINE refined
    REFINES
        abstract
    VARIABLES
        x
    EVENTS
        EVENT INITIALISATION
        THEN
            @act1 x := 0
        END

        EVENT set_value
        REFINES
            abstract_set
        ANY
            val
        WHERE
            @grd1 val > 0
        WITH
            @abs_val abs_val = val
        THEN
            @act1 x := val
        END
    END
    "#;

    let m = common::parse_machine(source);
    assert_eq!(m.events.len(), 1);
    let event = &m.events[0];
    assert_eq!(event.name, "set_value");
    assert_eq!(event.with.len(), 1);
    assert_eq!(event.with[0].label, Some("abs_val".to_string()));
    assert!(matches!(
        event.with[0].predicate.kind(),
        rossi::PredicateKind::Relational { .. }
    ));
}

#[test]
fn test_event_witness_clause() {
    let source = r#"
    MACHINE refined
    REFINES
        abstract
    VARIABLES
        x
    EVENTS
        EVENT INITIALISATION
        THEN
            @act1 x := 0
        END

        EVENT update
        REFINES
            abstract_update
        ANY
            val
        WHERE
            @grd1 val > 0
        WITNESS
            @abs_param val > 0
        THEN
            @act1 x := val
        END
    END
    "#;

    let m = common::parse_machine(source);
    assert_eq!(m.events.len(), 1);
    let event = &m.events[0];
    assert_eq!(event.name, "update");
    assert_eq!(event.witnesses.len(), 1);
    assert_eq!(event.witnesses[0].label, Some("abs_param".to_string()));
    assert!(matches!(
        event.witnesses[0].predicate.kind(),
        rossi::PredicateKind::Relational { .. }
    ));
}

#[test]
fn test_multiple_with_bindings() {
    let source = r#"
    MACHINE refined
    REFINES
        abstract
    VARIABLES
        x
    EVENTS
        EVENT INITIALISATION
        THEN
            @act1 x := 0
        END

        EVENT update
        REFINES
            abstract_update
        ANY
            a b
        WHERE
            @grd1 a > 0
            @grd2 b > 0
        WITH
            @abs_a abs_a = a
            @abs_b abs_b = b
        THEN
            @act1 x := a
        END
    END
    "#;

    let m = common::parse_machine(source);
    let event = &m.events[0];
    assert_eq!(event.with.len(), 2);
    assert_eq!(event.with[0].label, Some("abs_a".to_string()));
    assert_eq!(event.with[1].label, Some("abs_b".to_string()));
}

// ============================================================================
// VARIANT clause tests
// ============================================================================

#[test]
fn test_variant_clause_simple_identifier() {
    let source = r#"
    MACHINE test
    VARIABLES
        n
    INVARIANTS
        @inv1 n >= 0
    VARIANT
        n
    EVENTS
        EVENT INITIALISATION
        THEN
            @act1 n := 10
        END

        EVENT decrement
        STATUS convergent
        WHERE
            @grd1 n > 0
        THEN
            @act1 n := n - 1
        END
    END
    "#;

    let m = common::parse_machine(source);
    assert_eq!(m.variants.len(), 1, "Machine should have a variant");
    assert!(
        matches!(m.variants[0].expression.kind(), ExpressionKind::FreeIdentifier(n) if n == "n"),
        "Variant should be identifier 'n'"
    );
}

#[test]
fn test_variant_clause_arithmetic_expression() {
    let source = r#"
    MACHINE test
    VARIABLES
        x y
    INVARIANTS
        @inv1 x >= 0
        @inv2 y >= 0
    VARIANT
        x + y
    EVENTS
        EVENT INITIALISATION
        THEN
            @act1 x := 5
            @act2 y := 5
        END
    END
    "#;

    let m = common::parse_machine(source);
    assert_eq!(m.variants.len(), 1, "Machine should have a variant");
    match m.variants[0].expression.kind() {
        ExpressionKind::Associative { op, children } => {
            assert_eq!(*op, rossi::formula::tag::AssocExprOp::Plus);
            assert!(matches!(children[0].kind(), ExpressionKind::FreeIdentifier(n) if n == "x"));
            assert!(matches!(children[1].kind(), ExpressionKind::FreeIdentifier(n) if n == "y"));
        }
        other => panic!("Expected a sum for the variant, got {:?}", other),
    }
}

#[test]
fn test_variant_clause_labeled_items() {
    // Labeled variants form a list in one VARIANT clause: the first item
    // may be unlabeled, each further one starts at its @label.
    let source = r#"
    MACHINE test
    VARIABLES
        m k
    INVARIANTS
        @inv1 m >= 0
        @inv2 k >= 0
    VARIANT
        k
        @vrn1 m
        @vrn2 m - k
    EVENTS
        EVENT INITIALISATION
        THEN
            @act1 m := 5
            @act2 k := 0
        END
    END
    "#;

    let m = common::parse_machine(source);
    let shape: Vec<Option<&str>> = m.variants.iter().map(|v| v.label.as_deref()).collect();
    assert_eq!(shape, vec![None, Some("vrn1"), Some("vrn2")]);
    assert!(
        matches!(m.variants[0].expression.kind(), ExpressionKind::FreeIdentifier(n) if n == "k")
    );
}

// ============================================================================
// Rodin-compatible enumerated sets
// ============================================================================

#[test]
fn test_enumerated_sets_declaration_is_rejected() {
    let source = r#"
    CONTEXT colors
    SETS
        COLOR = {red, green, blue}
    END
    "#;

    assert!(
        parse(source).is_err(),
        "Classical-B enumerated SETS syntax must not parse as Event-B"
    );
}

#[test]
fn test_enumerated_set_uses_constants_and_partition() {
    let source = r#"
    CONTEXT colors
    SETS
        COLOR
    CONSTANTS
        red green blue
    AXIOMS
        @axm1 partition(COLOR, {red}, {green}, {blue})
    END
    "#;

    let ctx = common::parse_context(source);
    assert_eq!(ctx.sets[0].name, "COLOR");
    assert_eq!(
        ctx.constants
            .iter()
            .map(|constant| constant.name.as_str())
            .collect::<Vec<_>>(),
        ["red", "green", "blue"]
    );
    assert_eq!(ctx.axioms.len(), 1);
}

// ============================================================================
// Feature 1.4: Multiple parallel assignment
// ============================================================================

#[test]
fn test_multiple_parallel_assignment() {
    let source = r#"
    MACHINE test
    VARIABLES
        x y
    EVENTS
        EVENT INITIALISATION
        THEN
            @act1 x, y := 0, 0
        END

        EVENT swap
        THEN
            @act1 x, y := y, x
        END
    END
    "#;

    let m = common::parse_machine(source);
    // Check initialisation
    let init = m
        .initialisation
        .as_ref()
        .expect("Should have initialisation");
    assert_eq!(init.actions.len(), 1);
    match init.actions[0]
        .action
        .assignment()
        .map(rossi::Assignment::kind)
    {
        Some(rossi::AssignmentKind::BecomesEqualTo { idents, .. }) => {
            assert_eq!(idents.len(), 2);
            assert!(matches!(idents[0].kind(), ExpressionKind::FreeIdentifier(n) if n == "x"));
            assert!(matches!(idents[1].kind(), ExpressionKind::FreeIdentifier(n) if n == "y"));
        }
        other => panic!("Expected Assignment, got {:?}", other),
    }

    // Check swap event
    assert_eq!(m.events.len(), 1);
    let event = &m.events[0];
    assert_eq!(event.actions.len(), 1);
    match event.actions[0]
        .action
        .assignment()
        .map(rossi::Assignment::kind)
    {
        Some(rossi::AssignmentKind::BecomesEqualTo { idents, .. }) => {
            assert_eq!(idents.len(), 2);
            assert!(matches!(idents[0].kind(), ExpressionKind::FreeIdentifier(n) if n == "x"));
            assert!(matches!(idents[1].kind(), ExpressionKind::FreeIdentifier(n) if n == "y"));
        }
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

// ============================================================================
// Feature 2.1: Quantified union and intersection
// ============================================================================

#[test_case("s = UNION x · x ∈ ℕ | {x}", true, false ; "union_keyword_untyped")]
#[test_case("s = INTER x · x ∈ ℕ | {x}", false, false ; "inter_keyword_untyped")]
#[test_case("s = ⋃x⦂ℤ · x > 0 | {x}", true, true ; "union_glyph_typed")]
#[test_case("s = ⋂x⦂ℤ · x > 0 | {x}", false, true ; "inter_glyph_typed")]
fn test_quantified_union_inter(body: &str, is_union: bool, typed: bool) {
    let source = common::invariant_machine("s", body);
    let m = common::parse_machine(&source);
    let pred = &m.invariants[0].predicate;
    let rossi::PredicateKind::Relational { right, .. } = pred.kind() else {
        panic!("Expected Comparison predicate for {body:?}");
    };
    let decls = match (right.kind(), is_union) {
        (
            ExpressionKind::Quantified {
                op: rossi::formula::tag::QuantExprOp::QUnion,
                decls,
                ..
            },
            true,
        )
        | (
            ExpressionKind::Quantified {
                op: rossi::formula::tag::QuantExprOp::QInter,
                decls,
                ..
            },
            false,
        ) => decls,
        (other, _) => panic!("Expected quantified union/inter for {body:?}, got {other:?}"),
    };
    assert_eq!(decls.len(), 1);
    assert_eq!(decls[0].name(), "x");
    assert_eq!(decls[0].annotation().is_some(), typed);
}

// ============================================================================
// Feature 2.2: Typed bound variables (⦂) in quantifiers
// ============================================================================

#[test]
fn test_typed_bound_vars_in_forall() {
    // Exact formula pattern from a corpus model: typed bound variables in ∀
    let source = r#"
    CONTEXT test
    AXIOMS
        @axm1 ∀ti⦂ℙ(SUBSETS),pi · pi∈POLICIES ∧ ¬ TRUE = evaluable(ti↦pi) ⇒ FALSE = evaluable(ti↦pi)
    END
    "#;

    let ctx = common::parse_context(source);
    match ctx.axioms[0].predicate.kind() {
        rossi::PredicateKind::Quantified { decls, .. } => {
            assert_eq!(decls.len(), 2);
            assert_eq!(decls[0].name(), "ti");
            assert!(decls[0].annotation().is_some());
            assert_eq!(decls[1].name(), "pi");
            assert!(decls[1].annotation().is_none());
        }
        other => panic!("Expected Quantified, got {:?}", other),
    }
}

// ============================================================================
// Feature: empty set spellings
// ============================================================================

#[test]
fn test_empty_set_spellings() {
    for body in ["s = {}", "s = \u{2205}"] {
        let source = common::invariant_machine("s", body);
        let m = common::parse_machine(&source);
        let pred = &m.invariants[0].predicate;
        if let rossi::PredicateKind::Relational { right, .. } = pred.kind() {
            assert!(
                matches!(right.kind(), ExpressionKind::Atomic(AtomicOp::EmptySet)),
                "Expected EmptySet for {body:?}, got {:?}",
                right
            );
        } else {
            panic!("Expected Comparison predicate for {body:?}");
        }
    }
}

// ============================================================================
// Built-in function tests
// ============================================================================

/// The left-hand expression of a comparison predicate (panics otherwise).
fn comparison_lhs(kind: &PredicateKind) -> &ExpressionKind {
    match kind {
        PredicateKind::Relational { left, .. } => left.kind(),
        other => panic!("Expected Comparison, got {other:?}"),
    }
}

/// Assert that `left` is the single-argument application of a relational atom —
/// the V2 form `prj1(x)` = `FUNIMAGE(prj1, x)`.
fn assert_applied_atom(left: &ExpressionKind, op: AtomicOp) {
    match left {
        ExpressionKind::Binary {
            op: rossi::formula::tag::BinaryExprOp::FunImage,
            left: function,
            ..
        } => {
            assert!(matches!(function.kind(), ExpressionKind::Atomic(o) if *o == op));
        }
        other => panic!("Expected an applied {op:?} atom, got {other:?}"),
    }
}

#[test]
fn test_builtin_id_prj() {
    // V2: `id(x)`/`prj1(x)` are function application of the generic atom; a
    // projection of a pair uses a maplet argument (`prj1(S ↦ T)`), and a
    // plain identifier argument (`prj1(S)`, `prj2(cv)`) is the same form.
    let ctx = common::parse_context(
        "CONTEXT test\nAXIOMS\n    @axm1 id(S) = S\n    @axm2 prj1(S ↦ T) = S\n    @axm3 prj2(S ↦ T) = T\n    @axm4 prj1(S) = T\n    @axm5 prj2(cv) = FALSE\nEND\n",
    );
    let atom = |i: usize, k| assert_applied_atom(comparison_lhs(ctx.axioms[i].predicate.kind()), k);
    atom(0, AtomicOp::KIdGen);
    atom(1, AtomicOp::KPrj1Gen);
    atom(2, AtomicOp::KPrj2Gen);
    atom(3, AtomicOp::KPrj1Gen);
    atom(4, AtomicOp::KPrj2Gen);
}

#[test]
fn test_bare_id_is_atomic_builtin() {
    let ctx = common::parse_context("CONTEXT test\nAXIOMS\n    @axm1 id = S\nEND\n");
    if let PredicateKind::Relational { left, .. } = ctx.axioms[0].predicate.kind() {
        assert!(matches!(
            left.kind(),
            ExpressionKind::Atomic(AtomicOp::KIdGen)
        ));
    } else {
        panic!("Expected Comparison predicate");
    }
}

// ============================================================================
// Arity validation tests
// ============================================================================

#[test]
fn test_builtin_card_comma_form_rejected() {
    // A closed builtin takes exactly one argument; `card(S, T)` is rejected
    // (the comma is unexpected after the single argument), matching Rodin where
    // function application is single-argument.
    let source = "CONTEXT test\nAXIOMS\n    @axm1 card(S, T) = 5\nEND\n";
    assert!(parse(source).is_err(), "Expected error for card(S, T)");
}

#[test]
fn test_builtin_prj1_zero_args() {
    // `prj1()` has empty application parens — a syntax error (the bare atom is
    // `prj1`; application needs an argument).
    let source = "CONTEXT test\nAXIOMS\n    @axm1 prj1() = T\nEND\n";
    assert!(parse(source).is_err(), "Expected error for prj1()");
}

#[test]
fn test_nested_quantifier_in_guard() {
    // A realistic nested guard: a conjunction whose second conjunct is a
    // parenthesised quantifier (the form Event-B requires — a bare quantifier
    // as a ∧ operand is rejected, see operator_compatibility_test.rs).
    let source = r#"
    MACHINE test
    VARIABLES errcode part
    INVARIANTS
        @inv1 errcode ∈ ℤ
        @inv2 part ∈ ℤ
    EVENTS
        EVENT evt
        WHEN
            @grd1 (errcode∈dom(HM_Table(part)) ∧ (∃a·(a∈ACTIONS ∧ LEVEL↦a∈dom(HM_Table(part)(errcode)))))
        THEN
            @act1 errcode ≔ errcode
        END
    END
    "#;

    let result = parse(source);
    assert!(
        result.is_ok(),
        "nested-quantifier guard should parse: {:?}",
        result.err()
    );
}

#[test]
fn test_builtin_finite_wrong_arity() {
    let source = r#"
    CONTEXT test
    AXIOMS
        @axm1 finite(S, T)
    END
    "#;

    let result = parse(source);
    assert!(result.is_err(), "Expected arity error for finite(S, T)");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("finite") && err.contains("expected 1") && err.contains("got 2"),
        "Expected arity mismatch error, got: {}",
        err
    );
}

#[test]
fn test_builtin_partition_wrong_arity() {
    // partition needs at least 2 arguments (set + at least one block)
    let source = r#"
    CONTEXT test
    AXIOMS
        @axm1 partition(S)
    END
    "#;

    let result = parse(source);
    assert!(result.is_err(), "Expected arity error for partition(S)");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("partition") && err.contains("at least 2") && err.contains("got 1"),
        "Expected arity mismatch error, got: {}",
        err
    );
}

#[test]
fn test_builtin_partition_many_args_ok() {
    let ctx = common::parse_context("CONTEXT test\nAXIOMS\n    @axm1 partition(S, A, B, C)\nEND\n");
    match ctx.axioms[0].predicate.kind() {
        PredicateKind::Multiple(arguments) => {
            assert_eq!(arguments.len(), 4);
        }
        other => panic!("Expected the partition predicate, got {:?}", other),
    }
}

// ===========================================================================
// Clause ordering and duplicate detection tests
// ===========================================================================

#[test]
fn test_context_accepts_sets_before_extends() {
    // Event-B defines no structural syntax (Abrial) and Rodin treats sections as
    // unordered element sets, so a non-canonical order (SETS before EXTENDS) parses.
    let source = r#"
    CONTEXT test
    SETS
        S
    EXTENDS
        other_ctx
    END
    "#;

    let ctx = common::parse_context(source);
    assert_eq!(ctx.sets.len(), 1);
    assert_eq!(ctx.extends, vec!["other_ctx".to_string()]);
}

#[test_case("CONTEXT test\nSETS\n    S\nSETS\n    T\nEND\n", "SETS" ; "context_duplicate_sets")]
#[test_case("MACHINE test\nVARIABLES\n    x\nVARIABLES\n    y\nEND\n", "VARIABLES" ; "machine_duplicate_variables")]
fn test_duplicate_clause_error(source: &str, clause: &str) {
    let result = parse(source);
    assert!(result.is_err(), "Should reject duplicate {clause}");
    match result.unwrap_err() {
        ParseError::ClauseError {
            clause_type,
            message,
            line,
            column,
        } => {
            assert_eq!(clause_type, clause);
            assert!(
                message.contains("Duplicate"),
                "Error should mention 'Duplicate', got: {message}"
            );
            assert_eq!(line, 4, "duplicate {clause} clause starts on line 4");
            assert_eq!(column, 1, "duplicate {clause} clause starts at column 1");
        }
        other => panic!("Expected ClauseError, got: {:?}", other),
    }
}

#[test]
fn test_context_theorems_section_after_axioms() {
    // A THEOREMS section follows AXIOMS and lowers into `axioms` with the flag set.
    let source = r#"
    CONTEXT test
    CONSTANTS
        c
    AXIOMS
        @axm1 c > 0
    THEOREMS
        @thm1 c > -1
    END
    "#;

    let Component::Context(ctx) = parse(source).expect("should parse") else {
        panic!("expected a Context");
    };
    assert_eq!(ctx.axioms.len(), 2);
    assert!(!ctx.axioms[0].is_theorem);
    assert!(ctx.axioms[1].is_theorem);
    assert_eq!(ctx.axioms[1].label.as_deref(), Some("thm1"));
}

#[test]
fn test_context_accepts_axioms_after_theorems() {
    // Free order: AXIOMS after THEOREMS is accepted; both lower into `axioms`.
    let source = r#"
    CONTEXT test
    THEOREMS
        @thm1 1 = 1
    AXIOMS
        @axm1 2 = 2
    END
    "#;

    let ctx = common::parse_context(source);
    assert_eq!(ctx.axioms.len(), 2);
    assert!(ctx.axioms.iter().any(|a| a.is_theorem));
    assert!(ctx.axioms.iter().any(|a| !a.is_theorem));
}

#[test]
fn test_machine_theorems_between_invariants_and_variant() {
    let source = r#"
    MACHINE test
    VARIABLES
        x
    INVARIANTS
        @inv1 x > 0
    THEOREMS
        @thm1 x > -1
    VARIANT
        x
    EVENTS
        EVENT INITIALISATION
        THEN
            @act1 x := 1
        END
    END
    "#;

    let Component::Machine(mch) = parse(source).expect("should parse") else {
        panic!("expected a Machine");
    };
    assert_eq!(mch.invariants.len(), 2);
    assert!(!mch.invariants[0].is_theorem);
    assert!(mch.invariants[1].is_theorem);
    assert_eq!(mch.variants.len(), 1);
}

#[test]
fn test_machine_accepts_theorems_after_variant() {
    // Free order: THEOREMS after VARIANT is accepted; theorems lower into `invariants`.
    let source = r#"
    MACHINE test
    INVARIANTS
        @inv1 1 = 1
    VARIANT
        x
    THEOREMS
        @thm1 2 = 2
    END
    "#;

    let mch = common::parse_machine(source);
    assert_eq!(mch.invariants.len(), 2);
    assert!(mch.invariants.iter().any(|i| i.is_theorem));
    assert_eq!(mch.variants.len(), 1);
}

#[test]
fn test_machine_accepts_sees_before_refines() {
    // Free order: SEES before REFINES is accepted.
    let source = r#"
    MACHINE test
    SEES
        some_ctx
    REFINES
        abstract_m
    END
    "#;

    let mch = common::parse_machine(source);
    assert_eq!(mch.sees, vec!["some_ctx".to_string()]);
    assert_eq!(mch.refines.as_deref(), Some("abstract_m"));
}

#[test]
fn test_machine_rejects_clause_after_events() {
    // EVENTS is the terminal section: the events block is a list of `EVENT … END`
    // closed by the machine `END`, so a clause after it is a syntax error (only
    // another EVENT or END may follow).
    let source = r#"
    MACHINE test
    EVENTS
        EVENT INITIALISATION
        THEN
            @act1 x := 0
        END
    VARIABLES
        x
    END
    "#;

    let result = parse(source);
    assert!(result.is_err(), "a section after EVENTS must be rejected");
    assert!(
        matches!(result.unwrap_err(), ParseError::PestError { .. }),
        "an events-terminal violation is a plain syntax error",
    );
}

/// The individual expected-token spellings from a pest error message's
/// `= expected …` line (the list the LSP surfaces). Tokenizing avoids substring
/// coupling — e.g. matching "VARIANT" must not hit the "VARIANT" inside
/// "INVARIANTS", and the source-snippet lines must not be scanned.
fn expected_tokens(message: &str) -> Vec<String> {
    expected_line(message)
        .strip_prefix("expected ")
        .unwrap_or("")
        .split([',', ' '])
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "or")
        .map(str::to_string)
        .collect()
}

#[test]
fn misspelled_event_keyword_suggests_only_event_or_end_issue_76() {
    // EVENTS is terminal, so inside the events block the only continuations are
    // another EVENT (optionally status-prefixed) or the machine END. A misspelled
    // second EVENT keyword must surface that narrow set — not every clause keyword.
    let source = "\
MACHINE m
VARIABLES
    x
INVARIANTS
    @inv1 x > 0
EVENTS
    EVENT INITIALISATION
    THEN
        @act1 x := 0
    END
    EVNT foo
    THEN
        @act1 x := 1
    END
END
";
    let err = parse(source).expect_err("a misspelled EVENT keyword must fail");
    let ParseError::PestError { message, .. } = err else {
        panic!("expected a PestError, got: {err:?}");
    };
    let tokens = expected_tokens(&message);
    // The prior event ends with an action; the `action_list` follow-set guard stops
    // its terminating END from being parsed as a speculative next action, so the
    // list must NOT leak assignment operators (≔ :∈ :∣), comma, or lparen — nor any
    // clause keyword. The only continuations of the events block are another EVENT
    // (with an optional `convergent`/`anticipated` status prefix) or the machine END.
    // Build the allow-set from the same spellings the diagnostic renders (via
    // `display_rule` → `keywords::spell`), so it tracks any casing/spelling change.
    use rossi::keywords::{KeywordId, spell};
    let allowed = [
        spell(KeywordId::Event),
        spell(KeywordId::End),
        spell(KeywordId::Ordinary),
        spell(KeywordId::Convergent),
        spell(KeywordId::Anticipated),
    ];
    for token in &tokens {
        assert!(
            allowed.contains(&token.as_str()),
            "unexpected token {token:?} leaked into the list: {tokens:?}"
        );
    }
    // The two valid continuations are still offered.
    assert!(
        tokens.iter().any(|t| t == "EVENT"),
        "should still suggest EVENT: {tokens:?}"
    );
    assert!(
        tokens.iter().any(|t| t == "END"),
        "should still suggest END: {tokens:?}"
    );
}

#[test]
fn parse_error_names_keywords_and_dedups_issue_76() {
    // The expected-token list shows canonical keyword spellings, not raw `kw_*`
    // rule names, and lists each token once: the `event` rule and the `kw_event`
    // token both render as EVENT and collapse to a single entry.
    let source = "MACHINE m\nEVENTS\n    EVENT INITIALISATION\n    END\n    EVNT foo\nEND\n";
    let ParseError::PestError { message, .. } = parse(source).expect_err("must fail") else {
        panic!("expected a PestError");
    };
    assert!(
        !message.contains("kw_"),
        "raw rule names must be gone: {message}"
    );
    let tokens = expected_tokens(&message);
    assert_eq!(
        tokens.iter().filter(|t| *t == "EVENT").count(),
        1,
        "EVENT named exactly once (kw_event + event collapsed): {tokens:?}"
    );
    assert!(tokens.iter().any(|t| t == "END"), "names END: {tokens:?}");
}

/// The `= …` payload of a pest error message — the one line the LSP surfaces —
/// without the location header and the source snippet.
fn expected_line(message: &str) -> &str {
    message
        .lines()
        .map(str::trim_start)
        .find_map(|l| l.strip_prefix("= "))
        .unwrap_or("")
}

fn pest_message(source: &str) -> String {
    match parse(source).expect_err("must fail") {
        ParseError::PestError { message, .. } => message,
        other => panic!("expected a PestError, got: {other:?}"),
    }
}

fn machine_with_invariant(formula: &str) -> String {
    format!("MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 {formula}\nEND\n")
}

fn machine_with_action(action: &str) -> String {
    format!(
        "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT e\n    THEN\n        @act1 {action}\n    END\nEND\n"
    )
}

#[test]
fn expected_set_names_a_category_instead_of_every_formula_token() {
    // A formula position offers dozens of tokens at once — every operator after
    // an operand, every operand after an operator — and pest enumerates them
    // all (43 operators after `@inv1 x`). One category word says the same
    // thing; structural tokens (keywords, closers, separators) stay listed, and
    // a rule pest reports as a whole (`expression`, `negation_predicate`,
    // `labeled_action`) is named by its category rather than its grammar name.
    for (source, expected) in [
        (
            machine_with_invariant("x\n    @inv2 x ∈ S"),
            "expected an operator",
        ),
        (machine_with_invariant("(x ∈ S"), "expected an operator or )"),
        (machine_with_invariant("x ∈ )"), "expected an expression"),
        (machine_with_invariant("x ∈ S ∧ )"), "expected a predicate"),
        (machine_with_invariant("∀ x · )"), "expected a predicate"),
        (
            machine_with_invariant("x ∈ { ∈ }"),
            "expected an expression or }",
        ),
        (machine_with_invariant("λ )"), "expected identifier"),
        // After a complete formula at clause level: a section keyword, a
        // continuation of the formula, or another labeled predicate.
        (
            machine_with_invariant("x ∈ dom )"),
            "expected THEOREMS, END, REFINES, SEES, VARIABLES, INVARIANTS, VARIANT, EVENTS, an operator, or a predicate",
        ),
        (machine_with_action("x := )"), "expected an expression"),
        (
            "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT e\n    THEN\n        @act1 x := 1\n"
                .to_string(),
            "expected END, an operator, ,, or an action",
        ),
        (
            "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT e\n    WHERE\n        @grd1 := 3\n    THEN\n        @act1 x := 1\n    END\nEND\n"
                .to_string(),
            "expected theorem or a predicate",
        ),
        ("junk\n".to_string(), "expected CONTEXT or MACHINE"),
    ] {
        let message = pest_message(&source);
        assert_eq!(
            expected_line(&message),
            expected,
            "for source:\n{source}\nfull message:\n{message}"
        );
    }
}

/// No expected list may name a grammar rule.
///
/// Every term is a symbol the user types or a category word; a pest rule name
/// is neither, and the summarizer's classification keys on those names, so a
/// rule the classification misses shows up here as `exponent_operand` rather
/// than as `an expression`. Underscore is the tell — no spelling contains one.
#[test]
fn no_expected_term_is_a_rule_name() {
    let sources = [
        machine_with_invariant("x ^ )"),
        machine_with_invariant("x ∈ )"),
        machine_with_invariant("x ∈ S ∧ )"),
        machine_with_invariant("∀ x · )"),
        machine_with_invariant("∀ x⦂ )"),
        machine_with_invariant("λ )"),
        machine_with_invariant("{ x ∣ )"),
        machine_with_invariant("bool( )"),
        machine_with_invariant("dom( )"),
        machine_with_invariant("f{ )"),
        machine_with_invariant("x ∈ dom )"),
        machine_with_invariant("x\n    @inv2 x ∈ S"),
        machine_with_action("x := )"),
        machine_with_action("x :∣ )"),
        machine_with_action("x, )"),
        "junk\n".to_string(),
        "MACHINE m\nVARIABLES\n    x >\nEND\n".to_string(),
        "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT e\n    THEN\n        @act1 x ≔ 1\n"
            .to_string(),
    ];
    for source in sources {
        let message = pest_message(&source);
        let line = expected_line(&message);
        assert!(
            !line.contains('_'),
            "a rule name reached the user in {line:?}\nfor source:\n{source}"
        );
    }
}

#[test]
fn short_structural_expected_sets_are_listed_as_before() {
    // A short list of concrete tokens is the most useful message there is;
    // summarizing applies only to formula positions.
    for (source, expected) in [
        (machine_with_invariant("∀ x )"), "expected ⦂, ·, or ,"),
        (machine_with_action("x, )"), "expected identifier"),
        (machine_with_action("x )"), "expected ≔, :∈, :∣, ,, or ("),
        (
            "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT e\n    THEN\n        @act1 x := 1\n    END\n"
                .to_string(),
            "expected END or EVENT",
        ),
    ] {
        let message = pest_message(&source);
        assert_eq!(
            expected_line(&message),
            expected,
            "for source:\n{source}\nfull message:\n{message}"
        );
    }
}

#[test]
fn test_machine_full_valid_order() {
    let source = r#"
    MACHINE test
    REFINES
        abstract_m
    SEES
        some_ctx
    VARIABLES
        x
    INVARIANTS
        @inv1 x >= 0
        @thm1 theorem x >= 0
    VARIANT
        x
    EVENTS
        EVENT INITIALISATION
        THEN
            @act1 x := 0
        END
    END
    "#;

    let m = common::parse_machine(source);
    assert_eq!(m.name, "test");
    assert_eq!(m.refines, Some("abstract_m".to_string()));
    assert_eq!(m.sees, vec!["some_ctx"]);
    assert_eq!(m.variables.len(), 1);
    assert_eq!(m.variables[0].name, "x");
    assert_eq!(m.invariants.len(), 2);
    assert_eq!(
        m.invariants.iter().filter(|i| i.is_theorem).count(),
        1,
        "Should have exactly one invariant with is_theorem = true"
    );
    assert_eq!(m.variants.len(), 1);
    assert!(m.initialisation.is_some());
}

// ============================================================================
// Camille label and structural syntax tests
// ============================================================================

use rossi::ast::event::EventStatus;
use test_case::test_case;

// --- A colon belongs to the label -------------------------------------------

#[test_case("@axm1 1 = 1",  "axm1"  ; "without_colon")]
#[test_case("@axm1: 1 = 1", "axm1:" ; "with_colon")]
fn test_label_colon_in_axiom(predicate_text: &str, expected_label: &str) {
    let source = format!("CONTEXT test\nAXIOMS\n    {predicate_text}\nEND\n");
    let ctx = common::parse_context(&source);
    assert_eq!(ctx.axioms.len(), 1);
    assert_eq!(ctx.axioms[0].label, Some(expected_label.to_string()));
}

#[test_case("@inv1 x >= 0",  "inv1" ; "without_colon")]
#[test_case("@inv1: x >= 0", "inv1:" ; "with_colon")]
fn test_label_colon_in_invariant(predicate_text: &str, expected_label: &str) {
    let source = format!("MACHINE test\nVARIABLES\n    x\nINVARIANTS\n    {predicate_text}\nEND\n");
    let m = common::parse_machine(&source);
    assert_eq!(m.invariants.len(), 1);
    assert_eq!(m.invariants[0].label, Some(expected_label.to_string()));
}

#[test]
fn test_label_colon_in_event_guard() {
    let source = r#"
    MACHINE test
    EVENTS
        event foo
        WHERE
            @grd1: 1 = 1
        THEN
            @act1 x := 0
        END
    END
    "#;

    let m = common::parse_machine(source);
    assert_eq!(m.events[0].guards[0].label, Some("grd1:".to_string()));
}

#[test]
fn test_label_colon_in_action() {
    let source = r#"
    MACHINE test
    EVENTS
        EVENT INITIALISATION
        THEN
            @act1: x := 0
        END
    END
    "#;

    let m = common::parse_machine(source);
    let init = m
        .initialisation
        .as_ref()
        .expect("Should have initialisation");
    assert_eq!(init.actions[0].label, Some("act1:".to_string()));
}

// --- Theorem keyword ordering: both "@label theorem" and "theorem @label" ----

#[test_case("@thm1 theorem 1 = 1", "thm1" ; "label_before_theorem")]
#[test_case("theorem @thm1 1 = 1", "thm1" ; "theorem_before_label")]
#[test_case("theorem @thm1: 1 = 1", "thm1:" ; "theorem_before_label_with_colon")]
fn test_theorem_label_ordering(predicate_text: &str, expected_label: &str) {
    let source = format!("CONTEXT test\nAXIOMS\n    {predicate_text}\nEND\n");
    let ctx = common::parse_context(&source);
    assert_eq!(ctx.axioms.len(), 1);
    assert_eq!(ctx.axioms[0].label.as_deref(), Some(expected_label));
    assert!(ctx.axioms[0].is_theorem);
}

// --- Inline event status and refines -----------------------------------------

#[test_case("convergent event dec\n        END",
            "dec", Some(EventStatus::Convergent), None
            ; "convergent")]
#[test_case("anticipated event foo\n        END",
            "foo", Some(EventStatus::Anticipated), None
            ; "anticipated")]
#[test_case("event update refines abstract_update\n        END",
            "update", None, Some("abstract_update")
            ; "inline_refines")]
#[test_case("convergent event dec refines abstract_dec\n        END",
            "dec", Some(EventStatus::Convergent), Some("abstract_dec")
            ; "convergent_with_refines")]
fn test_inline_event_header(
    event_text: &str,
    expected_name: &str,
    expected_status: Option<EventStatus>,
    expected_refines: Option<&str>,
) {
    let source = format!("MACHINE test\nEVENTS\n    {event_text}\nEND\n");
    let m = common::parse_machine(&source);
    assert_eq!(m.events.len(), 1);
    let event = &m.events[0];
    assert_eq!(event.name, expected_name);
    assert_eq!(event.status, expected_status);
    assert_eq!(
        event.refines.first().map(|t| t.name.as_str()),
        expected_refines
    );
    assert!(
        !event.extended,
        "inline refines should not set extended flag"
    );
}

// --- skip action -------------------------------------------------------------

#[test]
fn test_skip_action_in_event() {
    let source = r#"
    MACHINE test
    EVENTS
        EVENT foo
        THEN
            @act1 skip
        END
    END
    "#;

    let m = common::parse_machine(source);
    assert_eq!(m.events.len(), 1);
    let event = &m.events[0];
    assert_eq!(event.actions.len(), 1);
    assert_eq!(event.actions[0].label, Some("act1".to_string()));
    assert!(event.actions[0].action.is_skip());
}

#[test]
fn test_label_with_non_identifier_chars() {
    // Labels like SAF5" appear in Rodin XML (from &quot; escapes).
    // Per TextEditor EBNF, labels accept any non-whitespace chars after '@'.
    let source = indoc::indoc! {r#"
        MACHINE test
        VARIABLES x
        INVARIANTS
            @SAF5" x ∈ ℤ
            @SAF6" x > 0
        END
    "#};
    let component = parse(source).expect("Should parse labels with double-quote chars");
    if let Component::Machine(m) = &component {
        assert_eq!(m.invariants.len(), 2);
        assert_eq!(m.invariants[0].label.as_deref(), Some("SAF5\""));
        assert_eq!(m.invariants[1].label.as_deref(), Some("SAF6\""));
    } else {
        panic!("Expected Machine");
    }
    // Roundtrip
    common::assert_roundtrip(source);
}

#[test]
fn test_label_with_colon_suffix() {
    // A trailing colon is retained as part of the Camille label.
    let source = indoc::indoc! {"
        CONTEXT test
        AXIOMS
            @axm1: 1 = 1
        END
    "};
    let component = parse(source).expect("Should parse label with colon");
    if let Component::Context(c) = &component {
        assert_eq!(c.axioms[0].label.as_deref(), Some("axm1:"));
    } else {
        panic!("Expected Context");
    }
}

#[test]
fn test_event_refines_multiple_targets() {
    // Merging syntax: several abstract event names after REFINES, in
    // both the body clause and the inline header form.
    let source = r#"
    MACHINE refined
    REFINES
        abstract
    VARIABLES
        x
    EVENTS
        EVENT INITIALISATION
        THEN
            @act1 x := 0
        END

        EVENT setBoth
        REFINES
            setHeight setWidth
        THEN
            @act1 x := 17
        END

        EVENT alt refines a b
        THEN
            @act1 x := 1
        END
    END
    "#;

    let m = common::parse_machine(source);
    let both = &m.events[0];
    let targets: Vec<&str> = both.refines.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(targets, vec!["setHeight", "setWidth"]);
    assert!(!both.extended);
    let alt = &m.events[1];
    let targets: Vec<&str> = alt.refines.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(targets, vec!["a", "b"]);
    // Each target keeps its own span for navigation.
    for t in both.refines.iter().chain(&alt.refines) {
        let span = t.span.expect("target span");
        assert_eq!(&source[span.start..span.end], t.name);
    }
}
