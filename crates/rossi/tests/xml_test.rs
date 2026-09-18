//! Integration tests for XML parsing (native Event-B format)

use rossi::{AssignmentKind, Component, ExpressionKind, ParseError, parse_xml};

#[test]
fn test_parse_context_xml_from_file() {
    let xml = std::fs::read_to_string("examples/counter_ctx.buc")
        .expect("Failed to read counter_ctx.buc");

    let result = parse_xml(&xml);
    assert!(result.is_ok(), "Parse error: {:?}", result.err());

    if let Component::Context(ctx) = result.unwrap() {
        // Name comes from filename, not XML body; parse_xml alone yields "unnamed_context".
        assert_eq!(ctx.sets.len(), 1);
        assert_eq!(ctx.sets[0].name, "STATUS");
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
fn xml_rejects_parallel_assignment_arity_mismatches() {
    for (assignment, targets, expressions) in [("x, y := 1", 2, 1), ("x := 1, 2", 1, 2)] {
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.event name="evt">
        <org.eventb.core.action name="a" org.eventb.core.label="act1" org.eventb.core.assignment="{assignment}"/>
    </org.eventb.core.event>
</org.eventb.core.machineFile>"#
        );
        let error = parse_xml(&xml).expect_err("mismatched assignment must fail XML parsing");
        let ParseError::MalformedAttribute {
            origin,
            attr_name,
            value,
            reason,
            ..
        } = error
        else {
            panic!("wrong error for {assignment:?}: {error:?}");
        };
        assert!(origin.contains("<action"), "missing XML origin: {origin}");
        assert_eq!(attr_name, "assignment");
        assert_eq!(value, assignment);
        assert!(reason.contains(&format!("target count ({targets})")));
        assert!(reason.contains(&format!("expression count ({expressions})")));
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
        assert_eq!(ctx.sets[0].name, "STATUS");
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
        assert_eq!(m.variants.len(), 1);
    } else {
        panic!("Expected Machine component");
    }
}

#[test]
fn test_parse_machine_with_labeled_variants_xml() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.variable identifier="n"/>
    <org.eventb.core.variable identifier="k"/>
    <org.eventb.core.variant expression="n" label="vrn1"/>
    <org.eventb.core.variant expression="n − k" label="vrn2"/>
</org.eventb.core.machineFile>"#;

    let component = parse_xml(xml).expect("parse");
    let Component::Machine(m) = &component else {
        panic!("Expected Machine component");
    };
    assert_eq!(m.variants.len(), 2);
    assert_eq!(m.variants[0].label.as_deref(), Some("vrn1"));
    assert_eq!(m.variants[1].label.as_deref(), Some("vrn2"));

    // The writer keeps every variant with its label, so a re-parse
    // sees the same list.
    let written = rossi::xml::to_xml(&component);
    let Component::Machine(again) = parse_xml(&written).expect("reparse") else {
        panic!("Expected Machine component");
    };
    assert_eq!(again.variants, m.variants);
}

#[test]
fn an_element_without_a_label_is_named_by_its_element_name() {
    // A label is mandatory in the text format and undefined to Rodin's static
    // checker when absent, so an element that carries none is read under its
    // internal name. Printing it must produce text that parses again.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.variable org.eventb.core.identifier="n"/>
    <org.eventb.core.invariant name="_0" org.eventb.core.predicate="n ∈ ℕ"/>
    <org.eventb.core.event name="_1" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="e">
        <org.eventb.core.witness name="_3" org.eventb.core.predicate="n = 0"/>
        <org.eventb.core.action name="_2" org.eventb.core.assignment="n ≔ 0"/>
    </org.eventb.core.event>
</org.eventb.core.machineFile>"#;

    let component = parse_xml(xml).expect("parse");
    let Component::Machine(m) = &component else {
        panic!("Expected Machine component");
    };
    assert_eq!(m.invariants[0].label.as_deref(), Some("_0"));
    assert_eq!(m.events[0].with[0].label.as_deref(), Some("_3"));
    assert_eq!(m.events[0].actions[0].label.as_deref(), Some("_2"));

    let printed = rossi::to_string(&component);
    rossi::parse(&printed).unwrap_or_else(|e| panic!("must reparse: {e}\n{printed}"));
}

#[test]
fn test_unlabeled_second_variant_survives_round_trip() {
    // A non-first variant without a label (accepted from foreign XML,
    // stored as `None`) must not vanish or corrupt the round-trip: the
    // writer spells out the default `vrn` label because the textual
    // grammar requires a label on every variant after the first.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.variable identifier="n"/>
    <org.eventb.core.variant expression="n" label="v1"/>
    <org.eventb.core.variant expression="n + 1"/>
</org.eventb.core.machineFile>"#;

    let component = parse_xml(xml).expect("parse");
    let Component::Machine(m) = &component else {
        panic!("Expected Machine component");
    };
    assert_eq!(m.variants.len(), 2);
    assert_eq!(m.variants[1].label, None);

    let written = rossi::xml::to_xml(&component);
    let Component::Machine(again) = parse_xml(&written).expect("reparse") else {
        panic!("Expected Machine component");
    };
    assert_eq!(again.variants.len(), 2);
    assert_eq!(again.variants[0].label.as_deref(), Some("v1"));
    assert_eq!(again.variants[1].label.as_deref(), Some("vrn"));

    // The pretty-printer must also emit re-parseable text for the
    // `None`-labeled second variant.
    let text = rossi::pretty::PrettyPrinter::new().print_component(&component);
    let reparsed = rossi::parse(&text).expect("printed text reparses");
    let Component::Machine(back) = &reparsed else {
        panic!("Expected Machine component");
    };
    assert_eq!(back.variants.len(), 2);
    assert_eq!(back.variants[1].label.as_deref(), Some("vrn"));
}

#[test]
fn test_explicit_vrn_label_on_second_variant_round_trip() {
    // `VARIANT @v1 n @vrn n + 1`: the default label may only stay
    // implicit on the first variant; dropping it from the second would
    // re-parse as `[Some("v1"), None]` and corrupt the model.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.variable identifier="n"/>
    <org.eventb.core.variant expression="n" label="v1"/>
    <org.eventb.core.variant expression="n + 1" label="vrn"/>
</org.eventb.core.machineFile>"#;

    let component = parse_xml(xml).expect("parse");
    let written = rossi::xml::to_xml(&component);
    let Component::Machine(again) = parse_xml(&written).expect("reparse") else {
        panic!("Expected Machine component");
    };
    assert_eq!(again.variants[1].label.as_deref(), Some("vrn"));
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
            let Some(assignment) = labeled.action.assignment() else {
                panic!("Expected Assignment, got {:?}", labeled.action);
            };
            let AssignmentKind::BecomesEqualTo { values, .. } = assignment.kind() else {
                panic!("Expected becomes-equal-to, got {assignment:?}");
            };
            assert!(matches!(
                values[0].kind(),
                ExpressionKind::Associative {
                    op: rossi::formula::tag::AssocExprOp::FComp,
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

// ============================================================================
// XML structure error variants — EB002 (`UnexpectedXmlRoot`) and EB003
// (`MissingXmlAttribute`).
// ============================================================================

#[test]
fn unexpected_xml_root_returns_eb002() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<some.unknown.root version="3"/>"#;

    match parse_xml(xml) {
        Err(ParseError::UnexpectedXmlRoot { found }) => {
            assert_eq!(found, "some.unknown.root");
        }
        other => panic!("expected UnexpectedXmlRoot, got {other:?}"),
    }
}

#[test]
fn nested_supported_root_still_reports_first_root_as_eb002() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<wrapper>
    <org.eventb.core.contextFile version="3">
        <org.eventb.core.context name="C0"/>
    </org.eventb.core.contextFile>
</wrapper>"#;

    match parse_xml(xml) {
        Err(ParseError::UnexpectedXmlRoot { found }) => {
            assert_eq!(found, "wrapper");
        }
        other => panic!("expected UnexpectedXmlRoot for wrapper, got {other:?}"),
    }
}

#[test]
fn empty_xml_root_field_when_no_start_event() {
    // No Start event at all: the parser falls through with no first root,
    // so `found` is the empty string.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>"#;
    match parse_xml(xml) {
        Err(ParseError::UnexpectedXmlRoot { found }) => {
            assert_eq!(found, "");
        }
        other => panic!("expected UnexpectedXmlRoot, got {other:?}"),
    }
}

#[test]
fn missing_reference_target_returns_eb003() {
    // Every target-carrying reference element (EXTENDS/SEES/REFINES) missing
    // its target attribute must fail with MissingXmlAttribute for "target".
    for (root, version, child) in [
        (
            "org.eventb.core.contextFile",
            "3",
            "org.eventb.core.extendsContext",
        ),
        (
            "org.eventb.core.machineFile",
            "5",
            "org.eventb.core.seesContext",
        ),
        (
            "org.eventb.core.machineFile",
            "5",
            "org.eventb.core.refinesMachine",
        ),
    ] {
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<{root} version="{version}">
    <{child} name="internal"/>
</{root}>"#
        );

        match parse_xml(&xml) {
            Err(ParseError::MissingXmlAttribute { element, attribute }) => {
                assert_eq!(element, child);
                assert_eq!(attribute, "target");
            }
            other => panic!("expected MissingXmlAttribute for {child}, got {other:?}"),
        }
    }
}

#[test]
fn test_parse_event_with_multiple_refines_xml() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.refinesMachine target="M0"/>
    <org.eventb.core.variable identifier="x"/>
    <org.eventb.core.event name="_e" convergence="0" extended="false" label="setBoth">
        <org.eventb.core.refinesEvent target="setHeight"/>
        <org.eventb.core.refinesEvent target="setWidth"/>
    </org.eventb.core.event>
</org.eventb.core.machineFile>"#;

    let component = parse_xml(xml).expect("parse");
    let Component::Machine(m) = &component else {
        panic!("Expected Machine component");
    };
    let targets: Vec<&str> = m.events[0]
        .refines
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(targets, vec!["setHeight", "setWidth"]);

    // The writer emits one refinesEvent per target, so a re-parse sees
    // the same list.
    let written = rossi::xml::to_xml(&component);
    let Component::Machine(again) = parse_xml(&written).expect("reparse") else {
        panic!("Expected Machine component");
    };
    let targets: Vec<&str> = again.events[0]
        .refines
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(targets, vec!["setHeight", "setWidth"]);
}

#[test]
fn test_extended_event_keeps_only_first_refines_target() {
    // An extended event inherits its body from exactly one abstract
    // event, and the textual form (`extends <target>`) cannot name
    // more; the reader drops surplus targets so a hand-edited file
    // converts without the extra target silently vanishing later.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.refinesMachine target="M0"/>
    <org.eventb.core.variable identifier="x"/>
    <org.eventb.core.event name="_e" convergence="0" extended="true" label="setBoth">
        <org.eventb.core.refinesEvent target="setHeight"/>
        <org.eventb.core.refinesEvent target="setWidth"/>
    </org.eventb.core.event>
</org.eventb.core.machineFile>"#;

    let component = parse_xml(xml).expect("parse");
    let Component::Machine(m) = &component else {
        panic!("Expected Machine component");
    };
    let targets: Vec<&str> = m.events[0]
        .refines
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(targets, vec!["setHeight"]);

    // Both writers round-trip the truncated form unchanged.
    let written = rossi::xml::to_xml(&component);
    let Component::Machine(again) = parse_xml(&written).expect("reparse") else {
        panic!("Expected Machine component");
    };
    assert_eq!(again.events[0].refines.len(), 1);

    let text = rossi::pretty::PrettyPrinter::new().print_component(&component);
    let Component::Machine(back) = rossi::parse(&text).expect("printed text reparses") else {
        panic!("Expected Machine component");
    };
    assert_eq!(back.events[0].refines.len(), 1);
    assert!(back.events[0].extended);
}

#[test]
fn repeated_event_node_keeps_the_first_like_rodin() {
    // Rodin's element store keys children by internal name: a second DOM
    // node with the same name is logged and dropped, the first kept
    // (`Buffer.getChildren`). A file carrying such a repeat therefore holds
    // one event to Rodin, not a label conflict.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.variable name="v" identifier="x"/>
    <org.eventb.core.event name="evt1" convergence="0" extended="false" label="step">
        <org.eventb.core.action name="a" label="act1" assignment="x ≔ 1"/>
    </org.eventb.core.event>
    <org.eventb.core.event name="evt1" convergence="0" extended="false" label="step">
        <org.eventb.core.action name="a" label="act1" assignment="x ≔ 2"/>
    </org.eventb.core.event>
    <org.eventb.core.event name="evt2" convergence="0" extended="false" label="other">
    </org.eventb.core.event>
</org.eventb.core.machineFile>"#;

    let component = parse_xml(xml).expect("parse");
    let Component::Machine(m) = &component else {
        panic!("Expected Machine component");
    };
    let names: Vec<&str> = m.events.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["step", "other"]);
    assert_eq!(m.events[0].actions.len(), 1);
    assert!(
        rossi::pretty::PrettyPrinter::new()
            .print_component(&component)
            .contains("x ≔ 1"),
        "the first node's body is the one kept"
    );
}
