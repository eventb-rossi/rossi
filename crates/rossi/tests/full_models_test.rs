//! Integration tests for parsing complete Event-B models

mod common;

use rossi::ast::expression::{AtomicBuiltinKind, BuiltinFunction};
use rossi::ast::predicate::BuiltinPredicate;
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
    assert_eq!(ctx.sets[0].name(), "STATUS");
    assert_eq!(ctx.constants.len(), 1);
    assert_eq!(ctx.constants[0].name, "max_value");
    assert_eq!(ctx.axioms.len(), 1);
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
            count := 0
        END

        EVENT increment
        WHERE
            @grd1 count < 100
        THEN
            count := count + 1
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
        parent1, parent2
    END
    "#;

    let ctx = common::parse_context(source);
    assert_eq!(ctx.name, "child");
    assert_eq!(ctx.extends.len(), 2);
    assert_eq!(ctx.extends[0], "parent1");
    assert_eq!(ctx.extends[1], "parent2");
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
            x := 0
        END

        EVENT update
        ANY
            val
        WHERE
            @grd1 val > 0
        THEN
            x := val
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
}

#[test]
fn test_multiple_variables_and_invariants() {
    let source = r#"
    MACHINE multi
    VARIABLES
        x, y, z
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
            x := 0
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
            x := val
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
        &event.with[0].predicate.kind,
        rossi::PredicateKind::Comparison { .. }
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
            x := 0
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
            x := val
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
        &event.witnesses[0].predicate.kind,
        rossi::PredicateKind::Comparison { .. }
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
            x := 0
        END

        EVENT update
        REFINES
            abstract_update
        ANY
            a, b
        WHERE
            @grd1 a > 0
            @grd2 b > 0
        WITH
            @abs_a abs_a = a
            @abs_b abs_b = b
        THEN
            x := a
        END
    END
    "#;

    let m = common::parse_machine(source);
    let event = &m.events[0];
    assert_eq!(event.with.len(), 2);
    assert_eq!(event.with[0].label, Some("abs_a".to_string()));
    assert_eq!(event.with[1].label, Some("abs_b".to_string()));
}

#[test]
fn test_with_where_then_together() {
    let source = r#"
    MACHINE refined
    REFINES
        abstract
    VARIABLES
        x
    EVENTS
        EVENT INITIALISATION
        THEN
            x := 0
        END

        EVENT inc
        REFINES
            abstract_inc
        WHERE
            @grd1 x < 100
        WITH
            @abs_x abs_x = x
        THEN
            x := x + 1
        END
    END
    "#;

    let m = common::parse_machine(source);
    let event = &m.events[0];
    assert_eq!(event.guards.len(), 1);
    assert_eq!(event.with.len(), 1);
    assert_eq!(event.actions.len(), 1);
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
            n := 10
        END

        EVENT decrement
        STATUS convergent
        WHERE
            @grd1 n > 0
        THEN
            n := n - 1
        END
    END
    "#;

    let m = common::parse_machine(source);
    assert!(m.variant.is_some(), "Machine should have a variant");
    assert_eq!(
        m.variant.unwrap(),
        ExpressionKind::Identifier("n".to_string()).into(),
        "Variant should be identifier 'n'"
    );
}

#[test]
fn test_variant_clause_arithmetic_expression() {
    use rossi::ast::expression::BinaryOp;

    let source = r#"
    MACHINE test
    VARIABLES
        x, y
    INVARIANTS
        @inv1 x >= 0
        @inv2 y >= 0
    VARIANT
        x + y
    EVENTS
        EVENT INITIALISATION
        THEN
            x := 5
            y := 5
        END
    END
    "#;

    let m = common::parse_machine(source);
    assert!(m.variant.is_some(), "Machine should have a variant");
    match m.variant.unwrap().kind {
        ExpressionKind::Binary { op, left, right } => {
            assert_eq!(op, BinaryOp::Add);
            assert!(matches!(left.kind, ExpressionKind::Identifier(ref n) if n == "x"));
            assert!(matches!(right.kind, ExpressionKind::Identifier(ref n) if n == "y"));
        }
        other => panic!("Expected Binary expression for variant, got {:?}", other),
    }
}

// ============================================================================
// parse_with_recovery() tests
// ============================================================================

#[test]
fn test_recovery_valid_input_no_errors() {
    use rossi::parse_with_recovery;

    let source = r#"
    CONTEXT test
    SETS
        S
    CONSTANTS
        x
    AXIOMS
        @axm1 x = 5
    END
    "#;

    let result = parse_with_recovery(source);
    assert!(result.is_ok(), "Valid input should have no errors");
    assert!(result.component.is_some());

    let ctx = match result.component {
        Some(Component::Context(ctx)) => ctx,
        _ => panic!("Expected Context component"),
    };
    assert_eq!(ctx.name, "test");
    assert_eq!(ctx.sets.len(), 1);
    assert_eq!(ctx.constants.len(), 1);
    assert_eq!(ctx.axioms.len(), 1);
}

#[test]
fn test_recovery_context_with_bad_axiom() {
    use rossi::parse_with_recovery;

    let source = r#"
    CONTEXT test
    SETS
        MySet
    CONSTANTS
        x
    AXIOMS
        @axm1 @@@ invalid syntax
        @axm2 x = 5
    END
    "#;

    let result = parse_with_recovery(source);
    assert!(
        result.has_recovered(),
        "Should recover with partial results"
    );
    assert!(!result.errors.is_empty(), "Should have errors");

    if let Some(Component::Context(ctx)) = result.component {
        assert_eq!(ctx.name, "test", "Should recover context name");
        assert!(
            !ctx.sets.is_empty() || !ctx.constants.is_empty(),
            "Should recover some declarations"
        );
        // At least one axiom should be recovered (axm2: x = 5)
        // The bad axiom may or may not be recovered depending on error recovery heuristics
    } else {
        panic!("Expected recovered Context component");
    }
}

#[test]
fn test_recovery_machine_with_bad_invariant() {
    use rossi::parse_with_recovery;

    let source = r#"
    MACHINE test
    VARIABLES
        x y
    INVARIANTS
        @inv1 @@@ bad predicate
        @inv2 x >= 0
    END
    "#;

    let result = parse_with_recovery(source);
    assert!(
        result.has_recovered(),
        "Should recover with partial results"
    );
    assert!(!result.errors.is_empty(), "Should have errors");

    if let Some(Component::Machine(m)) = result.component {
        assert_eq!(m.name, "test", "Should recover machine name");
        assert!(
            !m.variables.is_empty(),
            "Should recover variable declarations"
        );
    } else {
        panic!("Expected recovered Machine component");
    }
}

#[test]
fn test_recovery_unrecognizable_input() {
    use rossi::parse_with_recovery;

    let source = "this is not Event-B at all";

    let result = parse_with_recovery(source);
    assert!(
        result.is_err(),
        "Unrecognizable input should fail completely"
    );
    assert!(result.component.is_none());
}

#[test]
fn test_recovery_result_api() {
    use rossi::parse_with_recovery;

    // Test ParseResult API methods
    let valid_source = r#"
    CONTEXT test
    END
    "#;

    let result = parse_with_recovery(valid_source);
    assert!(result.is_ok());
    assert!(!result.is_err());
    assert!(!result.has_recovered());
    assert!(result.get_errors().is_empty());

    let component = result.into_component();
    assert!(component.is_some());
}

#[test]
fn test_recovery_into_result_valid() {
    use rossi::parse_with_recovery;

    let source = r#"
    CONTEXT test
    END
    "#;

    let result = parse_with_recovery(source).into_result();
    assert!(result.is_ok(), "Valid input should convert to Ok result");
}

// ============================================================================
// Mixed labeled/unlabeled actions test
// ============================================================================

#[test]
fn test_mixed_labeled_unlabeled_actions() {
    let source = r#"
    MACHINE test
    VARIABLES
        x
    EVENTS
        EVENT INITIALISATION
        THEN
            x := 0
        END

        EVENT update
        THEN
            @act1 x := x + 1
            x := x + 2
            @act3 x := x + 3
        END
    END
    "#;

    let m = common::parse_machine(source);
    assert_eq!(m.events.len(), 1);
    let event = &m.events[0];
    assert_eq!(event.actions.len(), 3);
    assert_eq!(event.actions[0].label, Some("act1".to_string()));
    assert_eq!(event.actions[1].label, None);
    assert_eq!(event.actions[2].label, Some("act3".to_string()));
}

// ============================================================================
// Feature 1.1: Enumerated sets
// ============================================================================

#[test]
fn test_enumerated_set_declaration() {
    let source = r#"
    CONTEXT colors
    SETS
        COLOR = {red, green, blue}
    END
    "#;

    let ctx = common::parse_context(source);
    assert_eq!(ctx.sets.len(), 1);
    assert_eq!(ctx.sets[0].name(), "COLOR");
    match &ctx.sets[0] {
        rossi::SetDeclaration::Enumerated { name, elements, .. } => {
            assert_eq!(name, "COLOR");
            assert_eq!(elements, &["red", "green", "blue"]);
        }
        _ => panic!("Expected Enumerated set declaration"),
    }
}

#[test]
fn test_mixed_deferred_and_enumerated_sets() {
    let source = r#"
    CONTEXT mixed
    SETS
        PERSON
        STATUS = {active, inactive}
    END
    "#;

    let ctx = common::parse_context(source);
    assert_eq!(ctx.sets.len(), 2);
    assert_eq!(ctx.sets[0].name(), "PERSON");
    assert!(matches!(
        &ctx.sets[0],
        rossi::SetDeclaration::Deferred { .. }
    ));
    assert_eq!(ctx.sets[1].name(), "STATUS");
    match &ctx.sets[1] {
        rossi::SetDeclaration::Enumerated { elements, .. } => {
            assert_eq!(elements, &["active", "inactive"]);
        }
        _ => panic!("Expected Enumerated set"),
    }
}

// ============================================================================
// Feature 1.4: Multiple parallel assignment
// ============================================================================

#[test]
fn test_multiple_parallel_assignment() {
    let source = r#"
    MACHINE test
    VARIABLES
        x, y
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
    "#;

    let m = common::parse_machine(source);
    // Check initialisation
    let init = m
        .initialisation
        .as_ref()
        .expect("Should have initialisation");
    assert_eq!(init.actions.len(), 1);
    match &init.actions[0].action.kind {
        rossi::ActionKind::Assignment {
            variables,
            expressions,
        } => {
            assert_eq!(variables, &["x", "y"]);
            assert_eq!(expressions.len(), 2);
        }
        other => panic!("Expected Assignment, got {:?}", other),
    }

    // Check swap event
    assert_eq!(m.events.len(), 1);
    let event = &m.events[0];
    assert_eq!(event.actions.len(), 1);
    match &event.actions[0].action.kind {
        rossi::ActionKind::Assignment {
            variables,
            expressions,
        } => {
            assert_eq!(variables, &["x", "y"]);
            assert_eq!(expressions.len(), 2);
        }
        other => panic!("Expected Assignment, got {:?}", other),
    }
}

// ============================================================================
// Feature 1.2: Extended set comprehension
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
    if let rossi::PredicateKind::Comparison { right, .. } = &pred.kind {
        match &right.kind {
            ExpressionKind::SetComprehension {
                identifiers,
                expression,
                ..
            } => {
                assert_eq!(identifiers, &["x"]);
                assert!(
                    expression.is_some(),
                    "Extended form should have expression body"
                );
            }
            other => panic!("Expected SetComprehension, got {:?}", other),
        }
    } else {
        panic!("Expected Comparison predicate");
    }
}

// ============================================================================
// Feature 1.3: Relational image
// ============================================================================

#[test]
fn test_relational_image() {
    let source = r#"
    MACHINE test
    VARIABLES
        r, s
    INVARIANTS
        @inv1 r[s] = s
    END
    "#;

    let m = common::parse_machine(source);
    let pred = &m.invariants[0].predicate;
    if let rossi::PredicateKind::Comparison { left, .. } = &pred.kind {
        match &left.kind {
            ExpressionKind::RelationalImage { relation, set } => {
                assert!(matches!(&relation.kind, ExpressionKind::Identifier(n) if n == "r"));
                assert!(matches!(&set.kind, ExpressionKind::Identifier(n) if n == "s"));
            }
            other => panic!("Expected RelationalImage, got {:?}", other),
        }
    } else {
        panic!("Expected Comparison predicate");
    }
}

// ============================================================================
// Feature 2.1: Quantified union and intersection
// ============================================================================

#[test]
fn test_quantified_union() {
    let source = r#"
    MACHINE test
    VARIABLES
        s
    INVARIANTS
        @inv1 s = UNION x · x ∈ ℕ | {x}
    END
    "#;

    let m = common::parse_machine(source);
    let pred = &m.invariants[0].predicate;
    if let rossi::PredicateKind::Comparison { right, .. } = &pred.kind {
        match &right.kind {
            ExpressionKind::QuantifiedUnion { identifiers, .. } => {
                assert_eq!(identifiers, &["x"]);
            }
            other => panic!("Expected QuantifiedUnion, got {:?}", other),
        }
    } else {
        panic!("Expected Comparison predicate");
    }
}

#[test]
fn test_quantified_inter() {
    let source = r#"
    MACHINE test
    VARIABLES
        s
    INVARIANTS
        @inv1 s = INTER x · x ∈ ℕ | {x}
    END
    "#;

    let m = common::parse_machine(source);
    let pred = &m.invariants[0].predicate;
    if let rossi::PredicateKind::Comparison { right, .. } = &pred.kind {
        match &right.kind {
            ExpressionKind::QuantifiedInter { identifiers, .. } => {
                assert_eq!(identifiers, &["x"]);
            }
            other => panic!("Expected QuantifiedInter, got {:?}", other),
        }
    } else {
        panic!("Expected Comparison predicate");
    }
}

// ============================================================================
// Feature 2.2: Typed bound variables (⦂) in quantifiers
// ============================================================================

#[test]
fn test_typed_forall_single() {
    use rossi::ast::predicate::Quantifier;

    let source = r#"
    CONTEXT test
    AXIOMS
        @axm1 ∀x⦂ℤ · x > 0
    END
    "#;

    let ctx = common::parse_context(source);
    match &ctx.axioms[0].predicate.kind {
        rossi::PredicateKind::Quantified {
            quantifier,
            identifiers,
            ..
        } => {
            assert_eq!(*quantifier, Quantifier::ForAll);
            assert_eq!(identifiers.len(), 1);
            assert_eq!(identifiers[0].name, "x");
            assert!(identifiers[0].type_expr.is_some());
            assert!(matches!(
                identifiers[0].type_expr.as_deref().map(|e| &e.kind),
                Some(ExpressionKind::Integers)
            ));
        }
        other => panic!("Expected Quantified ForAll, got {:?}", other),
    }
}

#[test]
fn test_typed_forall_multiple() {
    use rossi::ast::predicate::Quantifier;

    let source = r#"
    CONTEXT test
    AXIOMS
        @axm1 ∀ti⦂ℙ(SUBSETS), pi · pi ∈ POLICIES
    END
    "#;

    let ctx = common::parse_context(source);
    match &ctx.axioms[0].predicate.kind {
        rossi::PredicateKind::Quantified {
            quantifier,
            identifiers,
            ..
        } => {
            assert_eq!(*quantifier, Quantifier::ForAll);
            assert_eq!(identifiers.len(), 2);
            assert_eq!(identifiers[0].name, "ti");
            assert!(identifiers[0].type_expr.is_some());
            assert_eq!(identifiers[1].name, "pi");
            assert!(identifiers[1].type_expr.is_none());
        }
        other => panic!("Expected Quantified ForAll, got {:?}", other),
    }
}

#[test]
fn test_typed_exists() {
    use rossi::ast::predicate::Quantifier;

    let source = r#"
    CONTEXT test
    AXIOMS
        @axm1 ∃x⦂ℤ · x = 0
    END
    "#;

    let ctx = common::parse_context(source);
    match &ctx.axioms[0].predicate.kind {
        rossi::PredicateKind::Quantified {
            quantifier,
            identifiers,
            ..
        } => {
            assert_eq!(*quantifier, Quantifier::Exists);
            assert_eq!(identifiers[0].name, "x");
            assert!(identifiers[0].type_expr.is_some());
        }
        other => panic!("Expected Quantified Exists, got {:?}", other),
    }
}

#[test]
fn test_typed_forall_mixed() {
    use rossi::ast::predicate::Quantifier;

    let source = r#"
    CONTEXT test
    AXIOMS
        @axm1 ∀x⦂ℤ, y · x > y
    END
    "#;

    let ctx = common::parse_context(source);
    match &ctx.axioms[0].predicate.kind {
        rossi::PredicateKind::Quantified {
            quantifier,
            identifiers,
            ..
        } => {
            assert_eq!(*quantifier, Quantifier::ForAll);
            assert_eq!(identifiers.len(), 2);
            assert_eq!(identifiers[0].name, "x");
            assert!(identifiers[0].type_expr.is_some());
            assert_eq!(identifiers[1].name, "y");
            assert!(identifiers[1].type_expr.is_none());
        }
        other => panic!("Expected Quantified ForAll, got {:?}", other),
    }
}

#[test]
fn test_typed_quantified_union() {
    let source = common::invariant_machine("s", "s = ⋃x⦂ℤ · x > 0 | {x}");
    let m = common::parse_machine(&source);
    let pred = &m.invariants[0].predicate;
    if let rossi::PredicateKind::Comparison { right, .. } = &pred.kind {
        match &right.kind {
            ExpressionKind::QuantifiedUnion { identifiers, .. } => {
                assert_eq!(identifiers[0].name, "x");
                assert!(identifiers[0].type_expr.is_some());
            }
            other => panic!("Expected QuantifiedUnion, got {:?}", other),
        }
    } else {
        panic!("Expected Comparison predicate");
    }
}

#[test]
fn test_typed_quantified_inter() {
    let source = common::invariant_machine("s", "s = ⋂x⦂ℤ · x > 0 | {x}");
    let m = common::parse_machine(&source);
    let pred = &m.invariants[0].predicate;
    if let rossi::PredicateKind::Comparison { right, .. } = &pred.kind {
        match &right.kind {
            ExpressionKind::QuantifiedInter { identifiers, .. } => {
                assert_eq!(identifiers[0].name, "x");
                assert!(identifiers[0].type_expr.is_some());
            }
            other => panic!("Expected QuantifiedInter, got {:?}", other),
        }
    } else {
        panic!("Expected Comparison predicate");
    }
}

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
    match &ctx.axioms[0].predicate.kind {
        rossi::PredicateKind::Quantified { identifiers, .. } => {
            assert_eq!(identifiers.len(), 2);
            assert_eq!(identifiers[0].name, "ti");
            assert!(identifiers[0].type_expr.is_some());
            assert_eq!(identifiers[1].name, "pi");
            assert!(identifiers[1].type_expr.is_none());
        }
        other => panic!("Expected Quantified, got {:?}", other),
    }
}

// ============================================================================
// Feature 3.1: Additional relation types
// ============================================================================

#[test]
fn test_total_relation() {
    use rossi::ast::expression::BinaryOp;

    let source = common::invariant_machine("r", "r \u{2208} A <<-> B");
    let m = common::parse_machine(&source);
    let pred = &m.invariants[0].predicate;
    if let rossi::PredicateKind::Comparison { right, .. } = &pred.kind {
        assert!(
            matches!(&right.kind, ExpressionKind::Binary { op, .. } if *op == BinaryOp::TotalRelation),
            "Expected TotalRelation, got {:?}",
            right
        );
    } else {
        panic!("Expected Comparison predicate");
    }
}

#[test]
fn test_surjective_relation() {
    use rossi::ast::expression::BinaryOp;

    let source = common::invariant_machine("r", "r \u{2208} A <->> B");
    let m = common::parse_machine(&source);
    let pred = &m.invariants[0].predicate;
    if let rossi::PredicateKind::Comparison { right, .. } = &pred.kind {
        assert!(
            matches!(&right.kind, ExpressionKind::Binary { op, .. } if *op == BinaryOp::SurjectiveRelation),
            "Expected SurjectiveRelation, got {:?}",
            right
        );
    } else {
        panic!("Expected Comparison predicate");
    }
}

#[test]
fn test_total_surjective_relation() {
    use rossi::ast::expression::BinaryOp;

    let source = common::invariant_machine("r", "r \u{2208} A <<->> B");
    let m = common::parse_machine(&source);
    let pred = &m.invariants[0].predicate;
    if let rossi::PredicateKind::Comparison { right, .. } = &pred.kind {
        assert!(
            matches!(&right.kind, ExpressionKind::Binary { op, .. } if *op == BinaryOp::TotalSurjectiveRelation),
            "Expected TotalSurjectiveRelation, got {:?}",
            right
        );
    } else {
        panic!("Expected Comparison predicate");
    }
}

// ============================================================================
// Feature 4.1: Empty set ,, ASCII alias
// ============================================================================

#[test]
fn test_empty_set_comma_comma() {
    let source = common::invariant_machine("s", "s = ,,");
    let m = common::parse_machine(&source);
    let pred = &m.invariants[0].predicate;
    if let rossi::PredicateKind::Comparison { right, .. } = &pred.kind {
        assert!(matches!(right.kind, ExpressionKind::EmptySet));
    } else {
        panic!("Expected Comparison predicate");
    }
}

// ============================================================================
// Feature: oftype typing operator
// ============================================================================

#[test]
fn test_oftype_ascii() {
    use rossi::ast::expression::BinaryOp;

    let source = common::invariant_machine("x", "x \u{2208} \u{2115} oftype \u{2124}");
    let m = common::parse_machine(&source);
    let pred = &m.invariants[0].predicate;
    if let rossi::PredicateKind::Comparison { right, .. } = &pred.kind {
        assert!(
            matches!(&right.kind, ExpressionKind::Binary { op, .. } if *op == BinaryOp::OfType),
            "Expected OfType, got {:?}",
            right
        );
    } else {
        panic!("Expected Comparison predicate");
    }
}

#[test]
fn test_oftype_unicode() {
    use rossi::ast::expression::BinaryOp;

    let source = common::invariant_machine("x", "x \u{2208} \u{2115} \u{2982} \u{2124}");
    let m = common::parse_machine(&source);
    let pred = &m.invariants[0].predicate;
    if let rossi::PredicateKind::Comparison { right, .. } = &pred.kind {
        assert!(
            matches!(&right.kind, ExpressionKind::Binary { op, .. } if *op == BinaryOp::OfType),
            "Expected OfType, got {:?}",
            right
        );
    } else {
        panic!("Expected Comparison predicate");
    }
}

// ============================================================================
// Built-in function tests
// ============================================================================

#[test]
fn test_builtin_card() {
    let ctx = common::parse_context("CONTEXT test\nAXIOMS\n    @axm1 card(S) = 5\nEND\n");
    let pred = &ctx.axioms[0].predicate;
    if let PredicateKind::Comparison { left, .. } = &pred.kind {
        match &left.kind {
            ExpressionKind::BuiltinApplication {
                function,
                arguments,
            } => {
                assert_eq!(*function, BuiltinFunction::Card);
                assert_eq!(arguments.len(), 1);
                assert_eq!(
                    arguments[0],
                    ExpressionKind::Identifier("S".to_string()).into()
                );
            }
            other => panic!("Expected BuiltinApplication(Card), got {:?}", other),
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
    if let PredicateKind::Comparison { left, .. } = &ctx.axioms[0].predicate.kind {
        match &left.kind {
            ExpressionKind::BuiltinApplication { function, .. } => {
                assert_eq!(*function, BuiltinFunction::Min);
            }
            other => panic!("Expected BuiltinApplication(Min), got {:?}", other),
        }
    }
    if let PredicateKind::Comparison { left, .. } = &ctx.axioms[1].predicate.kind {
        match &left.kind {
            ExpressionKind::BuiltinApplication { function, .. } => {
                assert_eq!(*function, BuiltinFunction::Max);
            }
            other => panic!("Expected BuiltinApplication(Max), got {:?}", other),
        }
    }
}

/// The left-hand expression of a comparison predicate (panics otherwise).
fn comparison_lhs(kind: &PredicateKind) -> &ExpressionKind {
    match kind {
        PredicateKind::Comparison { left, .. } => &left.kind,
        other => panic!("Expected Comparison, got {other:?}"),
    }
}

/// Assert that `left` is the single-argument application of a relational atom —
/// the V2 form `prj1(x)` = `FUNIMAGE(prj1, x)`.
fn assert_applied_atom(left: &ExpressionKind, kind: AtomicBuiltinKind) {
    match left {
        ExpressionKind::FunctionApplication {
            function,
            arguments,
        } => {
            assert_eq!(function.kind, ExpressionKind::AtomicBuiltin(kind));
            assert_eq!(arguments.len(), 1);
        }
        other => panic!("Expected FunctionApplication(AtomicBuiltin({kind:?})), got {other:?}"),
    }
}

#[test]
fn test_builtin_id_prj() {
    // V2: `id(x)`/`prj1(x)` are function application of the generic atom; a
    // projection of a pair uses a maplet argument (`prj1(S ↦ T)`).
    let ctx = common::parse_context(
        "CONTEXT test\nAXIOMS\n    @axm1 id(S) = S\n    @axm2 prj1(S ↦ T) = S\n    @axm3 prj2(S ↦ T) = T\nEND\n",
    );
    let atom = |i: usize, k| assert_applied_atom(comparison_lhs(&ctx.axioms[i].predicate.kind), k);
    atom(0, AtomicBuiltinKind::Id);
    atom(1, AtomicBuiltinKind::Prj1);
    atom(2, AtomicBuiltinKind::Prj2);
}

#[test]
fn test_bare_id_is_atomic_builtin() {
    let ctx = common::parse_context("CONTEXT test\nAXIOMS\n    @axm1 id = S\nEND\n");
    if let PredicateKind::Comparison { left, .. } = &ctx.axioms[0].predicate.kind {
        assert_eq!(
            *left,
            ExpressionKind::AtomicBuiltin(AtomicBuiltinKind::Id).into()
        );
    } else {
        panic!("Expected Comparison predicate");
    }
}

// ============================================================================
// Built-in predicate tests
// ============================================================================

#[test]
fn test_builtin_finite() {
    let ctx = common::parse_context("CONTEXT test\nAXIOMS\n    @axm1 finite(S)\nEND\n");
    match &ctx.axioms[0].predicate.kind {
        PredicateKind::BuiltinApplication {
            predicate,
            arguments,
        } => {
            assert_eq!(*predicate, BuiltinPredicate::Finite);
            assert_eq!(arguments.len(), 1);
            assert_eq!(
                arguments[0],
                ExpressionKind::Identifier("S".to_string()).into()
            );
        }
        other => panic!("Expected BuiltinApplication(Finite), got {:?}", other),
    }
}

#[test]
fn test_builtin_partition() {
    let ctx = common::parse_context("CONTEXT test\nAXIOMS\n    @axm1 partition(S, A, B)\nEND\n");
    match &ctx.axioms[0].predicate.kind {
        PredicateKind::BuiltinApplication {
            predicate,
            arguments,
        } => {
            assert_eq!(*predicate, BuiltinPredicate::Partition);
            assert_eq!(arguments.len(), 3);
        }
        other => panic!("Expected BuiltinApplication(Partition), got {:?}", other),
    }
}

#[test]
fn test_user_defined_predicate() {
    let ctx = common::parse_context("CONTEXT test\nAXIOMS\n    @axm1 myPred(x)\nEND\n");
    match &ctx.axioms[0].predicate.kind {
        PredicateKind::Application {
            function,
            arguments,
        } => {
            assert_eq!(function, "myPred");
            assert_eq!(arguments.len(), 1);
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
    if let PredicateKind::Comparison { left, .. } = &ctx.axioms[0].predicate.kind {
        match &left.kind {
            ExpressionKind::Bool(pred) => {
                assert!(
                    matches!(&pred.kind, PredicateKind::Comparison { .. }),
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
    if let PredicateKind::Comparison { right, .. } = &ctx.axioms[0].predicate.kind {
        assert_eq!(*right, ExpressionKind::BoolType.into());
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
fn test_builtin_prj1_single_arg() {
    // V2: prj1(S) is function application of the generic projection atom.
    let ctx = common::parse_context("CONTEXT test\nAXIOMS\n    @axm1 prj1(S) = T\nEND\n");
    assert_applied_atom(
        comparison_lhs(&ctx.axioms[0].predicate.kind),
        AtomicBuiltinKind::Prj1,
    );
}

#[test]
fn test_builtin_prj2_single_arg() {
    // V2: prj2(cv) is function application of the generic projection atom.
    let ctx = common::parse_context("CONTEXT test\nAXIOMS\n    @axm1 prj2(cv) = FALSE\nEND\n");
    assert_applied_atom(
        comparison_lhs(&ctx.axioms[0].predicate.kind),
        AtomicBuiltinKind::Prj2,
    );
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
    match &ctx.axioms[0].predicate.kind {
        PredicateKind::BuiltinApplication {
            predicate,
            arguments,
        } => {
            assert_eq!(*predicate, BuiltinPredicate::Partition);
            assert_eq!(arguments.len(), 4);
        }
        other => panic!("Expected BuiltinApplication(Partition), got {:?}", other),
    }
}

// ===========================================================================
// Clause ordering and duplicate detection tests
// ===========================================================================

#[test]
fn test_context_clause_order_sets_before_extends() {
    let source = r#"
    CONTEXT test
    SETS
        S
    EXTENDS
        other_ctx
    END
    "#;

    let result = parse(source);
    assert!(result.is_err(), "Should reject SETS before EXTENDS");
    let err = result.unwrap_err();
    match &err {
        ParseError::ClauseError {
            clause_type,
            message,
            ..
        } => {
            assert_eq!(clause_type, "EXTENDS");
            assert!(
                message.contains("EXTENDS") && message.contains("SETS"),
                "Error should mention both EXTENDS and SETS, got: {}",
                message
            );
        }
        other => panic!("Expected ClauseError, got: {:?}", other),
    }
}

#[test]
fn test_context_duplicate_sets_clause() {
    let source = r#"
    CONTEXT test
    SETS
        S
    SETS
        T
    END
    "#;

    let result = parse(source);
    assert!(result.is_err(), "Should reject duplicate SETS");
    let err = result.unwrap_err();
    match &err {
        ParseError::ClauseError {
            clause_type,
            message,
            ..
        } => {
            assert_eq!(clause_type, "SETS");
            assert!(
                message.contains("Duplicate"),
                "Error should mention 'Duplicate', got: {}",
                message
            );
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
fn test_context_rejects_axioms_after_theorems() {
    let source = r#"
    CONTEXT test
    THEOREMS
        @thm1 1 = 1
    AXIOMS
        @axm1 2 = 2
    END
    "#;

    let result = parse(source);
    assert!(result.is_err(), "AXIOMS after THEOREMS must be rejected");
    match result.unwrap_err() {
        ParseError::ClauseError {
            clause_type,
            message,
            ..
        } => {
            assert_eq!(clause_type, "AXIOMS");
            assert!(message.contains("AXIOMS") && message.contains("THEOREMS"));
        }
        other => panic!("Expected ClauseError, got: {:?}", other),
    }
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
            x := 1
        END
    END
    "#;

    let Component::Machine(mch) = parse(source).expect("should parse") else {
        panic!("expected a Machine");
    };
    assert_eq!(mch.invariants.len(), 2);
    assert!(!mch.invariants[0].is_theorem);
    assert!(mch.invariants[1].is_theorem);
    assert!(mch.variant.is_some());
}

#[test]
fn test_machine_rejects_theorems_after_variant() {
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

    let result = parse(source);
    assert!(result.is_err(), "THEOREMS after VARIANT must be rejected");
    match result.unwrap_err() {
        ParseError::ClauseError {
            clause_type,
            message,
            ..
        } => {
            assert_eq!(clause_type, "THEOREMS");
            assert!(message.contains("THEOREMS") && message.contains("VARIANT"));
        }
        other => panic!("Expected ClauseError, got: {:?}", other),
    }
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
fn test_machine_clause_order_sees_before_refines() {
    let source = r#"
    MACHINE test
    SEES
        some_ctx
    REFINES
        abstract_m
    END
    "#;

    let result = parse(source);
    assert!(result.is_err(), "Should reject SEES before REFINES");
    let err = result.unwrap_err();
    match &err {
        ParseError::ClauseError {
            clause_type,
            message,
            ..
        } => {
            assert_eq!(clause_type, "REFINES");
            assert!(
                message.contains("REFINES") && message.contains("SEES"),
                "Error should mention both REFINES and SEES, got: {}",
                message
            );
        }
        other => panic!("Expected ClauseError, got: {:?}", other),
    }
}

#[test]
fn test_machine_duplicate_variables_clause() {
    let source = r#"
    MACHINE test
    VARIABLES
        x
    VARIABLES
        y
    END
    "#;

    let result = parse(source);
    assert!(result.is_err(), "Should reject duplicate VARIABLES");
    let err = result.unwrap_err();
    match &err {
        ParseError::ClauseError {
            clause_type,
            message,
            ..
        } => {
            assert_eq!(clause_type, "VARIABLES");
            assert!(
                message.contains("Duplicate"),
                "Error should mention 'Duplicate', got: {}",
                message
            );
        }
        other => panic!("Expected ClauseError, got: {:?}", other),
    }
}

#[test]
fn test_machine_clause_order_events_before_variables() {
    let source = r#"
    MACHINE test
    EVENTS
        EVENT INITIALISATION
        THEN
            x := 0
        END
    VARIABLES
        x
    END
    "#;

    let result = parse(source);
    assert!(result.is_err(), "Should reject EVENTS before VARIABLES");
    let err = result.unwrap_err();
    match &err {
        ParseError::ClauseError {
            clause_type,
            message,
            ..
        } => {
            assert_eq!(clause_type, "VARIABLES");
            assert!(
                message.contains("VARIABLES") && message.contains("EVENTS"),
                "Error should mention both VARIABLES and EVENTS, got: {}",
                message
            );
        }
        other => panic!("Expected ClauseError, got: {:?}", other),
    }
}

#[test]
fn test_clause_error_has_line_info() {
    let source = "CONTEXT test\nSETS\n    S\nSETS\n    T\nEND\n";

    let result = parse(source);
    assert!(result.is_err());
    let err = result.unwrap_err();
    match &err {
        ParseError::ClauseError { line, column, .. } => {
            assert_eq!(*line, 4, "Duplicate SETS clause starts on line 4");
            assert_eq!(*column, 1, "Duplicate SETS clause starts at column 1");
        }
        other => panic!("Expected ClauseError, got: {:?}", other),
    }
}

#[test]
fn test_context_sparse_valid_order() {
    let source = r#"
    CONTEXT test
    SETS
        S
    AXIOMS
        @axm1 S = S
    END
    "#;

    let ctx = common::parse_context(source);
    assert_eq!(ctx.name, "test");
    assert_eq!(ctx.sets.len(), 1);
    assert_eq!(ctx.axioms.len(), 1);
    assert!(ctx.extends.is_empty());
    assert!(ctx.constants.is_empty());
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
            x := 0
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
    assert!(m.variant.is_some());
    assert!(m.initialisation.is_some());
}

// ============================================================================
// eventb-to-txt reference format compatibility tests
// ============================================================================

use rossi::ast::event::EventStatus;
use test_case::test_case;

// --- Label with optional colon: parse succeeds and label is extracted --------

#[test_case("@axm1 1 = 1",  "axm1"  ; "without_colon")]
#[test_case("@axm1: 1 = 1", "axm1"  ; "with_colon")]
fn test_label_colon_in_axiom(predicate_text: &str, expected_label: &str) {
    let source = format!("CONTEXT test\nAXIOMS\n    {predicate_text}\nEND\n");
    let ctx = common::parse_context(&source);
    assert_eq!(ctx.axioms.len(), 1);
    assert_eq!(ctx.axioms[0].label, Some(expected_label.to_string()));
}

#[test_case("@inv1 x >= 0",  "inv1" ; "without_colon")]
#[test_case("@inv1: x >= 0", "inv1" ; "with_colon")]
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
            x := 0
        END
    END
    "#;

    let m = common::parse_machine(source);
    assert_eq!(m.events[0].guards[0].label, Some("grd1".to_string()));
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
    assert_eq!(init.actions[0].label, Some("act1".to_string()));
}

// --- Theorem keyword ordering: both "@label theorem" and "theorem @label" ----

#[test_case("@thm1 theorem 1 = 1"  ; "label_before_theorem")]
#[test_case("theorem @thm1 1 = 1"  ; "theorem_before_label")]
#[test_case("theorem @thm1: 1 = 1" ; "theorem_before_label_with_colon")]
fn test_theorem_label_ordering(predicate_text: &str) {
    let source = format!("CONTEXT test\nAXIOMS\n    {predicate_text}\nEND\n");
    let ctx = common::parse_context(&source);
    assert_eq!(ctx.axioms.len(), 1);
    assert_eq!(ctx.axioms[0].label, Some("thm1".to_string()));
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
    assert_eq!(event.refines.as_deref(), expected_refines);
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
    assert_eq!(event.actions[0].action, rossi::ActionKind::Skip.into());
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
    // Labels with trailing colon (eventb-to-txt format) should still work
    let source = indoc::indoc! {"
        CONTEXT test
        AXIOMS
            @axm1: 1 = 1
        END
    "};
    let component = parse(source).expect("Should parse label with colon");
    if let Component::Context(c) = &component {
        assert_eq!(c.axioms[0].label.as_deref(), Some("axm1"));
    } else {
        panic!("Expected Context");
    }
}
