//! Integration tests for XML parsing (native Event-B format)

use rossi::ast::expression::BinaryOp;
use rossi::{Action, Component, Expression, parse_xml};

#[test]
fn test_parse_context_xml_from_file() {
    let xml = std::fs::read_to_string("examples/counter_ctx.buc")
        .expect("Failed to read counter_ctx.buc");

    let result = parse_xml(&xml);
    assert!(result.is_ok(), "Parse error: {:?}", result.err());

    if let Component::Context(ctx) = result.unwrap() {
        // Name comes from filename, not XML body; parse_xml alone yields "unnamed_context".
        assert_eq!(ctx.sets.len(), 1);
        assert_eq!(ctx.sets[0].name(), "STATUS");
        assert_eq!(ctx.constants.len(), 1);
        assert_eq!(ctx.constants[0].name, "max_value");
        assert_eq!(ctx.axioms.len(), 2);
        assert_eq!(ctx.axioms[0].label, Some("axm1".to_string()));
        assert_eq!(ctx.axioms[1].label, Some("axm2".to_string()));
    } else {
        panic!("Expected Context component");
    }
}

#[test]
fn test_parse_machine_xml_from_file() {
    let xml = std::fs::read_to_string("examples/counter.bum").expect("Failed to read counter.bum");

    let result = parse_xml(&xml);
    assert!(result.is_ok(), "Parse error: {:?}", result.err());

    if let Component::Machine(m) = result.unwrap() {
        // Name comes from filename, not XML body; parse_xml alone yields "unnamed_machine".
        assert_eq!(m.sees.len(), 1);
        assert_eq!(m.sees[0], "counter_ctx");
        assert_eq!(m.variables.len(), 1);
        assert_eq!(m.variables[0].name, "count");
        assert_eq!(m.invariants.len(), 2);
        assert!(m.initialisation.is_some());

        let init = m.initialisation.as_ref().unwrap();
        assert_eq!(init.actions.len(), 1);

        assert_eq!(m.events.len(), 2);
        assert_eq!(m.events[0].name, "increment");
        assert_eq!(m.events[1].name, "decrement");
    } else {
        panic!("Expected Machine component");
    }
}

#[test]
fn test_parse_simple_context_xml() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
    <org.eventb.core.carrierSet identifier="PERSON"/>
</org.eventb.core.contextFile>"#;

    let result = parse_xml(xml);
    assert!(result.is_ok(), "Parse error: {:?}", result.err());

    if let Component::Context(ctx) = result.unwrap() {
        assert_eq!(ctx.sets.len(), 1);
        assert_eq!(ctx.sets[0].name(), "PERSON");
    } else {
        panic!("Expected Context component");
    }
}

#[test]
fn test_parse_context_with_extends_xml() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
    <org.eventb.core.extendsContext target="base_ctx"/>
    <org.eventb.core.carrierSet identifier="STATUS"/>
</org.eventb.core.contextFile>"#;

    let result = parse_xml(xml);
    assert!(result.is_ok(), "Parse error: {:?}", result.err());

    if let Component::Context(ctx) = result.unwrap() {
        assert_eq!(ctx.extends.len(), 1);
        assert_eq!(ctx.extends[0], "base_ctx");
        assert_eq!(ctx.sets.len(), 1);
        assert_eq!(ctx.sets[0].name(), "STATUS");
    } else {
        panic!("Expected Context component");
    }
}

#[test]
fn test_parse_context_with_theorems_xml() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
    <org.eventb.core.constant identifier="x"/>
    <org.eventb.core.axiom label="axm1" predicate="x &gt; 0"/>
    <org.eventb.core.axiom label="thm1" predicate="x &gt;= 1" theorem="true"/>
</org.eventb.core.contextFile>"#;

    let result = parse_xml(xml);
    assert!(result.is_ok(), "Parse error: {:?}", result.err());

    if let Component::Context(ctx) = result.unwrap() {
        assert_eq!(ctx.constants.len(), 1);
        assert_eq!(ctx.axioms.len(), 2);
        let non_theorems: Vec<_> = ctx.axioms.iter().filter(|a| !a.is_theorem).collect();
        let theorems: Vec<_> = ctx.axioms.iter().filter(|a| a.is_theorem).collect();
        assert_eq!(non_theorems.len(), 1);
        assert_eq!(theorems.len(), 1);
        assert_eq!(non_theorems[0].label, Some("axm1".to_string()));
        assert_eq!(theorems[0].label, Some("thm1".to_string()));
    } else {
        panic!("Expected Context component");
    }
}

#[test]
fn test_parse_machine_with_refines_xml() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.refinesMachine target="abstract_machine"/>
    <org.eventb.core.variable identifier="x"/>
</org.eventb.core.machineFile>"#;

    let result = parse_xml(xml);
    assert!(result.is_ok(), "Parse error: {:?}", result.err());

    if let Component::Machine(m) = result.unwrap() {
        assert_eq!(m.refines, Some("abstract_machine".to_string()));
        assert_eq!(m.variables.len(), 1);
        assert_eq!(m.variables[0].name, "x");
    } else {
        panic!("Expected Machine component");
    }
}

#[test]
fn test_parse_machine_with_variant_xml() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.variable identifier="n"/>
    <org.eventb.core.variant expression="n"/>
</org.eventb.core.machineFile>"#;

    let result = parse_xml(xml);
    assert!(result.is_ok(), "Parse error: {:?}", result.err());

    if let Component::Machine(m) = result.unwrap() {
        assert!(m.variant.is_some());
    } else {
        panic!("Expected Machine component");
    }
}

#[test]
fn test_parse_event_with_parameters_xml() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.variable identifier="x"/>
    <org.eventb.core.event name="set_value">
        <org.eventb.core.parameter identifier="v"/>
        <org.eventb.core.guard label="grd1" predicate="v &gt; 0"/>
        <org.eventb.core.action label="act1" assignment="x := v"/>
    </org.eventb.core.event>
</org.eventb.core.machineFile>"#;

    let result = parse_xml(xml);
    assert!(result.is_ok(), "Parse error: {:?}", result.err());

    if let Component::Machine(m) = result.unwrap() {
        assert_eq!(m.events.len(), 1);
        assert_eq!(m.events[0].name, "set_value");
        assert_eq!(m.events[0].parameters.len(), 1);
        assert_eq!(m.events[0].parameters[0].name, "v");
        assert_eq!(m.events[0].guards.len(), 1);
        assert_eq!(m.events[0].actions.len(), 1);
    } else {
        panic!("Expected Machine component");
    }
}

#[test]
fn test_parse_action_with_forward_composition_xml() {
    // Rodin stores one action per attribute, where a bare semicolon is
    // forward composition (no parentheses required, unlike the text format).
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.variable identifier="g"/>
    <org.eventb.core.event name="compose">
        <org.eventb.core.action label="act1" assignment="g ≔ p;f"/>
        <org.eventb.core.action label="act2" assignment="next ≔ r∼;(({0} ⩤ f) ∪ {m − 1 ↦ m});r"/>
    </org.eventb.core.event>
</org.eventb.core.machineFile>"#;

    let result = parse_xml(xml);
    assert!(result.is_ok(), "Parse error: {:?}", result.err());

    if let Component::Machine(m) = result.unwrap() {
        let actions = &m.events[0].actions;
        assert_eq!(actions.len(), 2);
        for labeled in actions {
            let Action::Assignment { expressions, .. } = &labeled.action else {
                panic!("Expected Assignment, got {:?}", labeled.action);
            };
            assert!(matches!(
                &expressions[0],
                Expression::Binary {
                    op: BinaryOp::Semicolon,
                    ..
                }
            ));
        }
    } else {
        panic!("Expected Machine component");
    }
}

#[test]
fn test_parse_convergent_event_xml() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.variable identifier="n"/>
    <org.eventb.core.event name="decrease" convergence="1">
        <org.eventb.core.guard label="grd1" predicate="n &gt; 0"/>
        <org.eventb.core.action label="act1" assignment="n := n - 1"/>
    </org.eventb.core.event>
</org.eventb.core.machineFile>"#;

    let result = parse_xml(xml);
    assert!(result.is_ok(), "Parse error: {:?}", result.err());

    if let Component::Machine(m) = result.unwrap() {
        assert_eq!(m.events.len(), 1);
        assert_eq!(m.events[0].name, "decrease");
        assert_eq!(m.events[0].status, Some(rossi::EventStatus::Convergent));
    } else {
        panic!("Expected Machine component");
    }
}

#[test]
fn test_invalid_xml() {
    let xml = r#"<not-valid-eventb/>"#;

    let result = parse_xml(xml);
    assert!(result.is_err());
}

#[test]
fn test_empty_context_name() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
</org.eventb.core.contextFile>"#;

    let result = parse_xml(xml);
    // Should succeed with default name "unnamed_context"
    assert!(result.is_ok());
    if let Component::Context(ctx) = result.unwrap() {
        assert_eq!(ctx.name, "unnamed_context");
    } else {
        panic!("Expected Context component");
    }
}
