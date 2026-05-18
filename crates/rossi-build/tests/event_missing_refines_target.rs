//! Group K: an event whose explicit `refines` target doesn't exist in
//! the parent machine is silently dropped from the `.bcm`. Rodin parity
//! — verified against a real-world corpus model, where the refining
//! events target non-existent abstract events and Rodin emits a `.bcm`
//! that contains only `INITIALISATION` while still marking the file
//! `accurate="true"`.

use rossi_build::{Project, ProjectComponent, Severity, build, sc_view::ScView};

fn project_with_missing_target() -> Project {
    // Parent M0 has events `a` and `b` (no `xyz`).
    let m0 = ProjectComponent::from_xml(
        "M0.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_v" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init0" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a_init" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_ev_a" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="a">
<org.eventb.core.action name="_a_a" org.eventb.core.assignment="x ≔ x + 1" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_ev_b" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="b">
<org.eventb.core.action name="_a_b" org.eventb.core.assignment="x ≔ x − 1" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
    // M1 refines M0. Event `c` explicitly refines `xyz` (absent in M0).
    let m1 = ProjectComponent::from_xml(
        "M1.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.variable name="_v" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init1" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION"></org.eventb.core.event>
<org.eventb.core.event name="_ev_c" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="c">
<org.eventb.core.refinesEvent name="_re" org.eventb.core.target="xyz"/>
<org.eventb.core.action name="_a_c" org.eventb.core.assignment="x ≔ x + 2" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
    Project::new("k", vec![m0, m1])
}

#[test]
fn missing_refines_target_drops_event() {
    let r = build(&project_with_missing_target());
    assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);
    let bcm = r.file("M1.bcm").expect("M1.bcm");
    assert!(
        bcm.accurate,
        "file should remain accurate; diagnostics: {:?}",
        r.diagnostics
    );
    let v = ScView::from_xml(&bcm.contents).unwrap();
    assert!(
        !v.events.contains_key("c"),
        "event `c` (refines missing target) should be dropped; events = {:?}",
        v.events.keys().collect::<Vec<_>>()
    );
    // INITIALISATION still resolves (M0 has one).
    assert!(v.events.contains_key("INITIALISATION"));
}

#[test]
fn missing_refines_target_emits_warning() {
    let r = build(&project_with_missing_target());
    let warnings: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .filter(|d| d.message.contains("refines target") && d.message.contains("xyz"))
        .collect();
    assert_eq!(
        warnings.len(),
        1,
        "expected exactly one refines-target warning; diagnostics: {:?}",
        r.diagnostics
    );
    assert_eq!(warnings[0].origin, "M1.c");
}
