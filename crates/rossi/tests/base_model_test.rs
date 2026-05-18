use rossi::{Component, parse};

#[test]
fn test_base_model_context() {
    // base-model.eventb contains both a context and a machine
    // We'll extract just the context part (lines 1-63) for parsing
    let full_source = std::fs::read_to_string("examples/base-model.eventb")
        .expect("Failed to read base-model.eventb");

    // Extract just the context C1 (up to "end" on line 63)
    let context_source = full_source
        .lines()
        .take_while(|line| !line.starts_with("machine"))
        .collect::<Vec<_>>()
        .join("\n");

    let result = parse(&context_source);

    if let Err(e) = &result {
        eprintln!("Parse error: {}", e);
    }

    assert!(
        result.is_ok(),
        "Failed to parse context C1 from base-model.eventb: {:?}",
        result.err()
    );

    match result.unwrap() {
        Component::Context(ctx) => {
            assert_eq!(ctx.name, "C1");
            assert_eq!(ctx.sets.len(), 4, "Expected 4 sets");
            let set_names: Vec<&str> = ctx.sets.iter().map(|s| s.name()).collect();
            assert_eq!(
                set_names,
                vec!["Union", "Names", "Accesses", "AccessRights"]
            );
            assert_eq!(ctx.constants.len(), 15, "Expected 15 constants");
            assert!(!ctx.axioms.is_empty(), "Expected axioms");
            println!(
                "✓ Successfully parsed context C1 with {} sets, {} constants, {} axioms",
                ctx.sets.len(),
                ctx.constants.len(),
                ctx.axioms.len()
            );
        }
        Component::Machine(_) => {
            panic!("Expected Context component, got Machine");
        }
    }
}
