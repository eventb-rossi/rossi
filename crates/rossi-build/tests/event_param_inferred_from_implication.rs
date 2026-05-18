//! Group M: an event parameter constrained only by `⇒`-consequents
//! must still be typed. Rodin parity — verified against a real-world
//! corpus machine whose guard types the parameter `trigs` via
//! `(newstate = PS_Ready ⇒ trigs = {proc}) ∧ (newstate ≠ PS_Ready ⇒ trigs = ∅)`.

use rossi_build::{Project, ProjectComponent, build, sc_view::ScView};

const CTX_BUC: &str = r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.carrierSet name="_set1" org.eventb.core.identifier="PROCESSES"/>
</org.eventb.core.contextFile>
"#;

const MACHINE_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_s1" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v1" org.eventb.core.identifier="timeout_trigger"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="timeout_trigger ⊆ PROCESSES"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="timeout_trigger ≔ ∅" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_ev_resume" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="resume">
<org.eventb.core.parameter name="_p1" org.eventb.core.identifier="proc"/>
<org.eventb.core.parameter name="_p2" org.eventb.core.identifier="flag"/>
<org.eventb.core.parameter name="_p3" org.eventb.core.identifier="trigs"/>
<org.eventb.core.guard name="_g1" org.eventb.core.label="grd1" org.eventb.core.predicate="proc ∈ PROCESSES"/>
<org.eventb.core.guard name="_g2" org.eventb.core.label="grd2" org.eventb.core.predicate="flag ∈ BOOL"/>
<org.eventb.core.guard name="_g3" org.eventb.core.label="grd49" org.eventb.core.predicate="(flag = TRUE ⇒ trigs = {proc}) ∧ (flag = FALSE ⇒ trigs = ∅)"/>
<org.eventb.core.action name="_a1" org.eventb.core.assignment="timeout_trigger ≔ trigs ⩤ timeout_trigger" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;

fn project() -> Project {
    Project::new(
        "m",
        vec![
            ProjectComponent::from_xml("Ctx.buc", CTX_BUC).unwrap(),
            ProjectComponent::from_xml("Mch.bum", MACHINE_BUM).unwrap(),
        ],
    )
}

#[test]
fn parameter_typed_via_implication_pair_consequent() {
    let r = build(&project());
    assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);
    let bcm = r.file("Mch.bcm").expect("Mch.bcm");
    assert!(
        bcm.accurate,
        "file should remain accurate; diagnostics: {:?}",
        r.diagnostics
    );
    let v = ScView::from_xml(&bcm.contents).unwrap();
    let resume = v.events.get("resume").expect("resume event present");
    assert!(
        resume.accurate,
        "resume event should be accurate; diagnostics: {:?}",
        r.diagnostics
    );
    assert_eq!(
        resume.parameters.get("trigs").map(String::as_str),
        Some("ℙ(PROCESSES)"),
        "trigs should be inferred as ℙ(PROCESSES); parameters: {:?}",
        resume.parameters
    );
}
