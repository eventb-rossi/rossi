//! Rodin database identities are independent from user-visible labels.

use rossi_build::{Project, ProjectComponent, build};

const M0: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile org.eventb.core.configuration="org.eventb.core.fwd" version="5">
<org.eventb.core.variable name="v0" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="i0" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="e0" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="a0" org.eventb.core.label="act1" org.eventb.core.assignment="x ≔ 0"/>
</org.eventb.core.event>
<org.eventb.core.event name="e1" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="tick">
<org.eventb.core.action name="a1" org.eventb.core.label="act1" org.eventb.core.assignment="x ≔ x+1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;

const M1: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile org.eventb.core.configuration="org.eventb.core.fwd" version="5">
<org.eventb.core.refinesMachine name="r0" org.eventb.core.target="M0"/>
<org.eventb.core.variable name="v0" org.eventb.core.identifier="x"/>
<org.eventb.core.variable name="v1" org.eventb.core.identifier="y"/>
<org.eventb.core.invariant name="i0" org.eventb.core.label="inv1" org.eventb.core.predicate="y ∈ ℤ"/>
<org.eventb.core.event name="e0" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="a0" org.eventb.core.label="act2" org.eventb.core.assignment="y ≔ 0"/>
</org.eventb.core.event>
<org.eventb.core.event name="e1" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="tick">
<org.eventb.core.action name="a1" org.eventb.core.label="act2" org.eventb.core.assignment="y ≔ y+1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;

#[test]
fn inherited_labels_keep_unique_names_and_resolvable_targets() {
    let project = Project::new(
        "p/q",
        vec![
            ProjectComponent::from_xml("M0.bum", M0).unwrap(),
            ProjectComponent::from_xml("M1.bum", M1).unwrap(),
        ],
    );
    let result = build(&project);
    assert!(result.is_ok(), "diagnostics: {:?}", result.diagnostics);

    let bcm = &result.file("M1.bcm").unwrap().contents;
    assert_eq!(bcm.matches("org.eventb.core.label=\"inv1\"").count(), 2);
    assert!(bcm.contains("scInvariant name=\"'\" org.eventb.core.label=\"inv1\""));
    assert!(bcm.contains("scInvariant name=\"(\" org.eventb.core.label=\"inv1\""));

    assert!(bcm.contains("scEvent name=\"z\" org.eventb.core.accurate=\"true\""));
    assert!(bcm.contains("scEvent name=\"{\" org.eventb.core.accurate=\"true\""));
    assert!(bcm.contains("org.eventb.core.scEvent#y\""));
    assert!(bcm.contains("org.eventb.core.scEvent#z\""));
    assert!(bcm.contains("org.eventb.core.scTarget=\"/p\\/q/M0.bcm"));

    assert!(bcm.contains("scAction name=\"'\" org.eventb.core.assignment=\"x ≔ 0\""));
    assert!(bcm.contains("scAction name=\"(\" org.eventb.core.assignment=\"y ≔ 0\""));
}
