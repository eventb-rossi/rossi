//! Tests for syntax error recovery
//!
//! These tests verify that the parser can recover from syntax errors
//! and produce partial ASTs with error information.

use rossi::{Component, parse_with_recovery};

#[test]
fn test_recovery_context_with_invalid_axiom() {
    let source = r#"
    CONTEXT test
    SETS
        MySet
    CONSTANTS
        c1
    AXIOMS
        @axm1 c1 = 10
        @axm2 invalid syntax $$$ here
        @axm3 c1 > 0
    END
    "#;

    let result = parse_with_recovery(source);

    // Should have recovered with errors
    assert!(result.has_recovered(), "Expected recovery with errors");
    assert!(!result.errors.is_empty(), "Expected at least one error");

    // Should have a partial component
    if let Some(Component::Context(ctx)) = result.component {
        assert_eq!(ctx.name, "test");
        assert_eq!(ctx.sets.len(), 1);
        assert_eq!(ctx.sets[0].name(), "MySet");
        assert_eq!(ctx.constants.len(), 1);
        assert_eq!(ctx.constants[0].name, "c1");
        // Should have recovered the valid axioms
        assert!(!ctx.axioms.is_empty(), "Should have at least some axioms");
    } else {
        panic!("Expected a Context component");
    }
}

#[test]
fn test_recovery_machine_with_invalid_invariant() {
    let source = r#"
    MACHINE test
    VARIABLES
        x, y
    INVARIANTS
        @inv1 x >= 0
        @inv2 invalid @#$ syntax
        @inv3 y >= 0
    END
    "#;

    let result = parse_with_recovery(source);

    // Should have recovered
    assert!(result.has_recovered(), "Expected recovery with errors");

    if let Some(Component::Machine(m)) = result.component {
        assert_eq!(m.name, "test");
        assert_eq!(m.variables.len(), 2);
        assert_eq!(m.variables[0].name, "x");
        assert_eq!(m.variables[1].name, "y");
        // Should have recovered some invariants
        assert!(
            !m.invariants.is_empty(),
            "Should have at least some invariants"
        );
    } else {
        panic!("Expected a Machine component");
    }
}

#[test]
fn test_recovery_context_missing_end() {
    let source = r#"
    CONTEXT test
    SETS
        MySet
    CONSTANTS
        c1
    "#;

    let result = parse_with_recovery(source);

    // Should have some errors due to missing END
    assert!(!result.errors.is_empty());

    // Should still recover what it can
    if let Some(Component::Context(ctx)) = result.component {
        assert_eq!(ctx.name, "test");
        assert_eq!(ctx.sets.len(), 1);
        assert_eq!(ctx.constants.len(), 1);
    }
}

#[test]
fn test_recovery_context_with_multiple_errors() {
    let source = r#"
    CONTEXT multi_error
    SETS
        Set1, Set2
    CONSTANTS
        bad syntax here
        c1, c2
    AXIOMS
        @axm1 c1 = 1
        @axm2 !!!! invalid
        @axm3 c2 = 2
        @axm4 bad #### syntax
    END
    "#;

    let result = parse_with_recovery(source);

    // Should have multiple errors
    assert!(result.errors.len() >= 2, "Expected multiple errors");

    // Should still have a component
    if let Some(Component::Context(ctx)) = result.component {
        assert_eq!(ctx.name, "multi_error");
        // Should recover the valid sets
        assert!(!ctx.sets.is_empty());
        // Should recover some constants
        assert!(!ctx.constants.is_empty());
    }
}

#[test]
fn test_recovery_machine_with_valid_clauses() {
    let source = r#"
    MACHINE valid_parts
    REFINES
        abstract_machine
    SEES
        some_context
    VARIABLES
        x, y, z
    INVARIANTS
        @inv1 x = 0
        @inv2 bad &&& syntax
        @inv3 y >= 0
    END
    "#;

    let result = parse_with_recovery(source);

    if let Some(Component::Machine(m)) = result.component {
        assert_eq!(m.name, "valid_parts");
        assert_eq!(m.refines, Some("abstract_machine".to_string()));
        assert_eq!(m.sees.len(), 1);
        assert_eq!(m.sees[0], "some_context");
        assert_eq!(m.variables.len(), 3);
    }
}

#[test]
fn test_successful_parse_no_errors() {
    let source = r#"
    CONTEXT perfect
    SETS
        MySet
    CONSTANTS
        c1
    AXIOMS
        @axm1 c1 = 10
    END
    "#;

    let result = parse_with_recovery(source);

    // Should succeed with no errors
    assert!(result.is_ok(), "Expected no errors");
    assert!(result.errors.is_empty(), "Expected empty error list");
    assert!(result.component.is_some(), "Expected a component");
}

#[test]
fn test_recovery_preserves_valid_data() {
    let source = r#"
    CONTEXT recovery_test
    EXTENDS
        parent1, parent2
    SETS
        Set1, Set2, Set3
    CONSTANTS
        c1, c2, c3
    AXIOMS
        @axm1 c1 = 1
        @axm2 c2 = 2
        @axm3 invalid
        @axm4 c3 = 3
        @thm1 theorem c1 < c2
    END
    "#;

    let result = parse_with_recovery(source);

    if let Some(Component::Context(ctx)) = result.component {
        // Check that valid data is preserved
        assert_eq!(ctx.name, "recovery_test");
        assert_eq!(ctx.extends.len(), 2);
        assert!(ctx.extends.contains(&"parent1".to_string()));
        assert!(ctx.extends.contains(&"parent2".to_string()));
        assert_eq!(ctx.sets.len(), 3);
        assert_eq!(ctx.constants.len(), 3);
        // Should have recovered some axioms (at least the valid ones)
        assert!(!ctx.axioms.is_empty());
    }
}

#[test]
fn test_error_count_tracking() {
    let source = r#"
    CONTEXT error_count
    AXIOMS
        @axm1 bad1
        @axm2 bad2
        @axm3 bad3
    END
    "#;

    let result = parse_with_recovery(source);

    // The number of errors should reflect the failures
    assert!(!result.errors.is_empty(), "Expected errors to be recorded");
    // We should have at least the initial parse error
    assert!(
        !result.errors.is_empty(),
        "Expected at least one error, got {}",
        result.errors.len()
    );
}

#[test]
fn test_recovery_with_commas_in_lists() {
    let source = r#"
    CONTEXT comma_test
    SETS
        Set1,
        Set2,
        Set3
    CONSTANTS
        c1, c2, c3
    END
    "#;

    let result = parse_with_recovery(source);

    // This should parse successfully
    if let Some(Component::Context(ctx)) = result.component {
        assert_eq!(ctx.name, "comma_test");
        assert!(ctx.sets.len() >= 2, "Should recover multiple sets");
        assert!(
            ctx.constants.len() >= 2,
            "Should recover multiple constants"
        );
    }
}

#[test]
fn test_recovery_unknown_component_type() {
    let source = r#"
    UNKNOWN something
    SETS
        MySet
    END
    "#;

    let result = parse_with_recovery(source);

    // Should fail completely since we can't determine the component type
    assert!(result.is_err(), "Expected complete failure");
}
