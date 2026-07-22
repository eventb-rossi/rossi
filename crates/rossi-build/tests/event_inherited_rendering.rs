//! Rendering inherited events whose concrete labels differ from their parents.

use rossi_build::{Project, ProjectComponent, build_with_model, sc_view::ScView};

const M0: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_v0" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_i0" org.eventb.core.label="type0" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init0" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_init_act0" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="init0"/>
</org.eventb.core.event>
<org.eventb.core.event name="_event0" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="abstract_step">
<org.eventb.core.parameter name="_param0" org.eventb.core.identifier="p"/>
<org.eventb.core.guard name="_guard0" org.eventb.core.label="typed" org.eventb.core.predicate="p ∈ ℤ"/>
<org.eventb.core.action name="_action0" org.eventb.core.assignment="x ≔ p" org.eventb.core.label="write"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;

const M1: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref1" org.eventb.core.target="M0"/>
<org.eventb.core.variable name="_v1" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="type1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init1" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION">
<org.eventb.core.refinesEvent name="_init_ref1" org.eventb.core.target="INITIALISATION"/>
</org.eventb.core.event>
<org.eventb.core.event name="_event1" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="middle_step">
<org.eventb.core.refinesEvent name="_event_ref1" org.eventb.core.target="abstract_step"/>
<org.eventb.core.guard name="_guard1" org.eventb.core.label="nonnegative" org.eventb.core.predicate="p ≥ 0"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;

const M2: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref2" org.eventb.core.target="M1"/>
<org.eventb.core.variable name="_v2" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_i2" org.eventb.core.label="type2" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init2" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION">
<org.eventb.core.refinesEvent name="_init_ref2" org.eventb.core.target="INITIALISATION"/>
</org.eventb.core.event>
<org.eventb.core.event name="_event2" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="concrete_step">
<org.eventb.core.refinesEvent name="_event_ref2" org.eventb.core.target="middle_step"/>
<org.eventb.core.guard name="_guard2" org.eventb.core.label="bounded" org.eventb.core.predicate="p ≤ 10"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;

fn project() -> Project {
    Project::new(
        "rename",
        vec![
            ProjectComponent::from_xml("M0.bum", M0).unwrap(),
            ProjectComponent::from_xml("M1.bum", M1).unwrap(),
            ProjectComponent::from_xml("M2.bum", M2).unwrap(),
        ],
    )
}

#[test]
fn event_inherited_rendering_uses_the_abstract_event_label() {
    let (result, model) = build_with_model(&project());
    assert!(result.is_ok(), "diagnostics: {:?}", result.diagnostics);

    let middle = model.machines["M1"].events_by_label["middle_step"].as_ref();
    assert_eq!(
        middle.inherited.as_ref().map(|event| event.label.as_str()),
        Some("abstract_step")
    );
    let concrete = model.machines["M2"].events_by_label["concrete_step"].as_ref();
    assert_eq!(
        concrete
            .inherited
            .as_ref()
            .map(|event| event.label.as_str()),
        Some("middle_step")
    );

    let m2 = result.file("M2.bcm").expect("M2.bcm");
    let view = ScView::from_xml(&m2.contents).unwrap();
    let concrete = view.events.get("concrete_step").expect("concrete_step");
    assert!(concrete.accurate);
    assert!(concrete.extended);
    assert_eq!(concrete.parameters.len(), 1);
    assert_eq!(concrete.guards.len(), 3);
    assert_eq!(concrete.actions.len(), 1);
    assert_eq!(
        concrete.refines_events.values().next().map(String::as_str),
        Some("M1.bcm|org.eventb.core.scMachineFile#M1")
    );
}

#[test]
fn event_inherited_initialisation_renders_transitively() {
    let (result, _) = build_with_model(&project());
    assert!(result.is_ok(), "diagnostics: {:?}", result.diagnostics);

    let view = ScView::from_xml(&result.file("M2.bcm").expect("M2.bcm").contents).unwrap();
    let init = view.events.get("INITIALISATION").expect("INITIALISATION");
    assert!(init.accurate);
    assert!(init.extended);
    assert_eq!(init.actions.len(), 1);
}
