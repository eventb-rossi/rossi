use rossi_build::{Project, ProjectComponent, RuleId, build, sc_view::ScView};

const MACHINE: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_inv" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_init_act" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_bad" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="bad">
<org.eventb.core.action name="_bad_act" org.eventb.core.assignment="x ≔ TRUE + FALSE" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;

const CONTEXT_WITH_INVALID_AXIOM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.axiom name="_bad" org.eventb.core.label="bad" org.eventb.core.predicate="TRUE + FALSE = 0"/>
</org.eventb.core.contextFile>
"#;

const MACHINE_WITH_INVALID_INVARIANT: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_typing" org.eventb.core.label="typing" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.invariant name="_bad" org.eventb.core.label="bad" org.eventb.core.predicate="TRUE + FALSE = 0"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_init_act" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;

const MACHINE_WITH_INVALID_VARIANT: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_typing" org.eventb.core.label="typing" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.variant name="_variant" org.eventb.core.expression="TRUE + FALSE"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_init_act" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;

const MACHINE_WITH_UNDECLARED_EVENT_EXPRESSIONS: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_typing" org.eventb.core.label="typing" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_init_act" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_guard" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="bad_guard">
<org.eventb.core.guard name="_guard_bad" org.eventb.core.label="grd1" org.eventb.core.predicate="missing = 0"/>
<org.eventb.core.action name="_guard_act" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_action" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="bad_action">
<org.eventb.core.action name="_action_bad" org.eventb.core.assignment="x ≔ card(∅ ⦂ ℙ(UNKNOWN))" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;

#[test]
fn invalid_arithmetic_is_dropped_and_marks_event_inaccurate() {
    let project = Project::new(
        "invalid-arithmetic",
        vec![ProjectComponent::from_xml("M.bum", MACHINE).unwrap()],
    );
    let result = build(&project);
    let bcm = result.file("M.bcm").expect("M.bcm");
    let view = ScView::from_xml(&bcm.contents).unwrap();
    let event = view.events.get("bad").expect("bad event");

    assert!(bcm.accurate, "diagnostics: {:?}", result.diagnostics);
    assert!(!event.accurate, "diagnostics: {:?}", result.diagnostics);
    assert!(event.actions.is_empty(), "invalid action was emitted");
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.rule_id == Some(RuleId::TypeError) && diagnostic.message == "action is ill-typed"
    }));
}

#[test]
fn invalid_variant_is_diagnosed_omitted_and_marks_machine_inaccurate() {
    let project = Project::new(
        "invalid-variant",
        vec![ProjectComponent::from_xml("M.bum", MACHINE_WITH_INVALID_VARIANT).unwrap()],
    );
    let result = build(&project);
    let bcm = result.file("M.bcm").expect("M.bcm");
    let view = ScView::from_xml(&bcm.contents).unwrap();

    assert!(!bcm.accurate, "diagnostics: {:?}", result.diagnostics);
    assert!(view.variant.is_none(), "invalid variant was emitted");
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.rule_id == Some(RuleId::TypeError)
            && diagnostic.message == "variant expression is ill-typed"
    }));
}

#[test]
fn undeclared_event_expressions_report_undeclared_identifier_first() {
    let project = Project::new(
        "undeclared-event-expression",
        vec![
            ProjectComponent::from_xml("M.bum", MACHINE_WITH_UNDECLARED_EVENT_EXPRESSIONS).unwrap(),
        ],
    );
    let result = build(&project);
    let bcm = result.file("M.bcm").expect("M.bcm");
    let view = ScView::from_xml(&bcm.contents).unwrap();

    assert!(bcm.accurate, "diagnostics: {:?}", result.diagnostics);
    assert!(!view.events["bad_guard"].accurate);
    assert!(view.events["bad_guard"].guards.is_empty());
    assert!(!view.events["bad_action"].accurate);
    assert!(view.events["bad_action"].actions.is_empty());
    for (origin, name) in [
        ("M.bad_guard.grd1", "missing"),
        ("M.bad_action.act1", "UNKNOWN"),
    ] {
        assert!(result.diagnostics.iter().any(|diagnostic| {
            diagnostic.origin == origin
                && diagnostic.rule_id == Some(RuleId::UndeclaredIdentifier)
                && diagnostic.message.contains(name)
        }));
        assert!(!result.diagnostics.iter().any(|diagnostic| {
            diagnostic.origin == origin && diagnostic.rule_id == Some(RuleId::TypeError)
        }));
    }
}

#[test]
fn invalid_arithmetic_axiom_is_dropped_and_marks_context_inaccurate() {
    let project = Project::new(
        "invalid-axiom",
        vec![ProjectComponent::from_xml("C.buc", CONTEXT_WITH_INVALID_AXIOM).unwrap()],
    );
    let result = build(&project);
    let bcc = result.file("C.bcc").expect("C.bcc");

    assert!(!bcc.accurate, "diagnostics: {:?}", result.diagnostics);
    assert!(
        !bcc.contents.contains("scAxiom"),
        "invalid axiom was emitted"
    );
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.rule_id == Some(RuleId::TypeError)
            && diagnostic.message == "axiom predicate is ill-typed"
    }));
}

#[test]
fn invalid_arithmetic_invariant_is_dropped_and_marks_machine_inaccurate() {
    let project = Project::new(
        "invalid-invariant",
        vec![ProjectComponent::from_xml("M.bum", MACHINE_WITH_INVALID_INVARIANT).unwrap()],
    );
    let result = build(&project);
    let bcm = result.file("M.bcm").expect("M.bcm");

    assert!(!bcm.accurate, "diagnostics: {:?}", result.diagnostics);
    assert!(bcm.contents.contains(r#"org.eventb.core.label="typing""#));
    assert!(!bcm.contents.contains(r#"org.eventb.core.label="bad""#));
    assert!(result.diagnostics.iter().any(|diagnostic| {
        diagnostic.rule_id == Some(RuleId::TypeError)
            && diagnostic.message == "invariant predicate is ill-typed"
    }));
}
