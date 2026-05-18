//! Group L: a `<org.eventb.core.witness>` child of an `INITIALISATION`
//! event must survive into the `.bcm` as `<org.eventb.core.scWitness>`.
//! Rodin parity — verified against a real-world corpus model, where
//! the refining machine's INITIALISATION witnesses the disappearing
//! abstract variable.

use rossi_build::{Project, ProjectComponent, build, sc_view::ScView};

fn project_with_init_witness() -> Project {
    // Abstract machine: variable `e`, initialised to 0.
    let m0 = ProjectComponent::from_xml(
        "M0.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_v" org.eventb.core.identifier="e"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="e ∈ ℤ"/>
<org.eventb.core.event name="_init0" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a_init" org.eventb.core.assignment="e ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
    // Refinement: variable `x` replaces `e`. INITIALISATION assigns `x`
    // and witnesses the abstract primed variable `e'` via `e' = x'`.
    let m1 = ProjectComponent::from_xml(
        "M1.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.variable name="_v" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init1" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a_init" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
<org.eventb.core.witness name="_w1" org.eventb.core.label="e'" org.eventb.core.predicate="e' = x'"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
    Project::new("l", vec![m0, m1])
}

#[test]
fn initialisation_witness_emitted_into_bcm() {
    let r = build(&project_with_init_witness());
    assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);
    let bcm = r.file("M1.bcm").expect("M1.bcm");
    assert!(
        bcm.accurate,
        "file should remain accurate; diagnostics: {:?}",
        r.diagnostics
    );
    let v = ScView::from_xml(&bcm.contents).unwrap();
    let init = v
        .events
        .get("INITIALISATION")
        .expect("INITIALISATION must be present");
    assert_eq!(
        init.witnesses.len(),
        1,
        "expected exactly one witness on INITIALISATION; got {:?}",
        init.witnesses
    );
    let row = init.witnesses.values().next().unwrap();
    assert_eq!(row.label, "e'");
    let expected = rossi::parse_predicate_str("e' = x'").unwrap();
    assert_eq!(row.predicate, expected, "witness predicate AST mismatch");
}
