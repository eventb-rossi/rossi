//! Integration tests for working Event-B model examples

use rossi::{Component, parse};
use std::fs;

#[test]
fn test_basic_context() {
    let source =
        fs::read_to_string("examples/basic_ctx.eventb").expect("Failed to read basic_ctx.eventb");

    let result = parse(&source);
    assert!(
        result.is_ok(),
        "Failed to parse basic_ctx: {:?}",
        result.err()
    );

    if let Component::Context(ctx) = result.unwrap() {
        assert_eq!(ctx.name, "basic_ctx");
        assert_eq!(ctx.constants.len(), 3);
        assert!(ctx.constants.iter().any(|c| c.name == "min_value"));
        assert!(ctx.constants.iter().any(|c| c.name == "max_value"));
        assert!(ctx.constants.iter().any(|c| c.name == "default_value"));
        assert!(ctx.axioms.len() >= 7);
        assert!(
            ctx.axioms.iter().any(|a| a.is_theorem),
            "Should have at least one axiom with is_theorem = true"
        );
    } else {
        panic!("Expected Context component");
    }
}

#[test]
fn test_basic_machine() {
    let source = fs::read_to_string("examples/basic_machine.eventb")
        .expect("Failed to read basic_machine.eventb");

    let result = parse(&source);
    assert!(
        result.is_ok(),
        "Failed to parse basic_machine: {:?}",
        result.err()
    );

    if let Component::Machine(m) = result.unwrap() {
        assert_eq!(m.name, "basic_machine");
        assert_eq!(m.sees.len(), 1);
        assert_eq!(m.sees[0], "basic_ctx");
        assert_eq!(m.variables.len(), 1);
        assert!(m.variables.iter().any(|v| v.name == "current_value"));
        assert!(m.invariants.len() >= 2);
        assert!(m.initialisation.is_some());
        assert_eq!(m.events.len(), 3);

        // Check event names
        let event_names: Vec<&str> = m.events.iter().map(|e| e.name.as_str()).collect();
        assert!(event_names.contains(&"increase"));
        assert!(event_names.contains(&"decrease"));
        assert!(event_names.contains(&"reset"));
    } else {
        panic!("Expected Machine component");
    }
}

#[test]
fn test_all_working_examples_parse() {
    // Test that all working example files parse without errors
    let example_files = vec![
        "examples/counter.eventb",
        "examples/counter_machine.eventb",
        "examples/basic_ctx.eventb",
        "examples/basic_machine.eventb",
        // Files exercising a real THEOREMS section.
        "examples/simple_sets_ctx.eventb",
        "examples/sets_and_relations_ctx.eventb",
        "examples/library_ctx.eventb",
        "examples/library_machine.eventb",
    ];

    for file in example_files {
        let source = fs::read_to_string(file).unwrap_or_else(|_| panic!("Failed to read {}", file));

        let result = parse(&source);
        assert!(
            result.is_ok(),
            "Failed to parse {}: {:?}",
            file,
            result.err()
        );
    }
}

#[test]
fn test_theorems_section_lowers_to_flagged_axioms() {
    // A THEOREMS section is parsed into `Context.axioms` with `is_theorem = true`,
    // matching Rodin's model (a theorem is a flagged axiom, not a separate section).
    let source = fs::read_to_string("examples/simple_sets_ctx.eventb")
        .expect("Failed to read simple_sets_ctx.eventb");
    let Component::Context(ctx) = parse(&source).expect("should parse") else {
        panic!("expected a Context");
    };

    let theorems: Vec<_> = ctx.axioms.iter().filter(|a| a.is_theorem).collect();
    assert_eq!(theorems.len(), 1, "THEOREMS member should be flagged");
    assert_eq!(theorems[0].label.as_deref(), Some("thm1"));
    // The plain axioms remain non-theorems in the same vec.
    assert_eq!(ctx.axioms.iter().filter(|a| !a.is_theorem).count(), 4);
}

#[test]
fn test_machine_theorems_section_lowers_to_flagged_invariants() {
    let source = fs::read_to_string("examples/library_machine.eventb")
        .expect("Failed to read library_machine.eventb");
    let Component::Machine(mch) = parse(&source).expect("should parse") else {
        panic!("expected a Machine");
    };

    let theorems: Vec<_> = mch.invariants.iter().filter(|i| i.is_theorem).collect();
    assert_eq!(theorems.len(), 1, "THEOREMS member should be flagged");
    assert_eq!(theorems[0].label.as_deref(), Some("thm1"));
}
