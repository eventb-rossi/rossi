//! Integration tests for working Event-B model examples

use rossi::{Component, parse, parse_components};
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
    // Parse *every* bundled example model — discovered from the directory rather
    // than a hand-maintained list — so an example that stops parsing cannot rot
    // unnoticed (every `.eventb` under `examples/` must parse cleanly).
    let dir = "examples";
    let mut parsed = 0;
    for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("Failed to read {dir}/: {e}")) {
        let path = entry.expect("directory entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("eventb") {
            continue;
        }
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()));
        // `parse_components` handles both single- and multi-component files
        // (e.g. base-model.eventb bundles a context and a machine).
        let result = parse_components(&source);
        assert!(
            result.is_ok(),
            "Failed to parse {}: {:?}",
            path.display(),
            result.err()
        );
        assert!(
            !result.unwrap().is_empty(),
            "no components parsed from {}",
            path.display()
        );
        parsed += 1;
    }
    assert!(parsed > 0, "no example models found under {dir}/");
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
