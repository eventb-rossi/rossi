//! Group Q: an event parameter typed only via a set-operator equality
//! where one operand of the set-op has a known type and the other is
//! the parameter. Rodin parity — verified against
//! `ertms-hl3-abz2018.zip/Env_M3.bcm`, where event `RBC_extend_ma`
//! types parameter `vss_set` via `vss_set ∩ ma[{tr}] = ∅` against
//! `ma`'s `ℙ(TRAIN × VSS)` relational-image codomain.

use rossi_build::{Project, ProjectComponent, build, sc_view::ScView};

const CTX_BUC: &str = r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.carrierSet name="_set1" org.eventb.core.identifier="TRAIN"/>
<org.eventb.core.carrierSet name="_set2" org.eventb.core.identifier="VSS"/>
<org.eventb.core.constant name="_c1" org.eventb.core.identifier="ma"/>
<org.eventb.core.axiom name="_a1" org.eventb.core.label="axm1" org.eventb.core.predicate="ma ∈ ℙ(TRAIN × VSS)"/>
</org.eventb.core.contextFile>
"#;

const MACHINE_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_s1" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v1" org.eventb.core.identifier="last"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="last ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="last ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_ev_extend" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="RBC_extend_ma_like">
<org.eventb.core.parameter name="_p1" org.eventb.core.identifier="tr"/>
<org.eventb.core.parameter name="_p2" org.eventb.core.identifier="vss_set"/>
<org.eventb.core.guard name="_g1" org.eventb.core.label="grd1" org.eventb.core.predicate="tr ∈ dom(ma)"/>
<org.eventb.core.guard name="_g2" org.eventb.core.label="grd2" org.eventb.core.predicate="vss_set ∩ ma[{tr}] = ∅"/>
<org.eventb.core.action name="_a1" org.eventb.core.assignment="last ≔ 1" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;

fn project() -> Project {
    Project::new(
        "q",
        vec![
            ProjectComponent::from_xml("Ctx.buc", CTX_BUC).unwrap(),
            ProjectComponent::from_xml("Mch.bum", MACHINE_BUM).unwrap(),
        ],
    )
}

#[test]
fn parameter_typed_via_set_op_equality_with_typed_sibling_operand() {
    let r = build(&project());
    assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);
    let bcm = r.file("Mch.bcm").expect("Mch.bcm");
    assert!(
        bcm.accurate,
        "file should remain accurate; diagnostics: {:?}",
        r.diagnostics
    );
    let v = ScView::from_xml(&bcm.contents).unwrap();
    let ev = v
        .events
        .get("RBC_extend_ma_like")
        .expect("RBC_extend_ma_like event present");
    assert!(
        ev.accurate,
        "event should be accurate; diagnostics: {:?}",
        r.diagnostics
    );
    assert_eq!(
        ev.parameters.get("tr").map(String::as_str),
        Some("TRAIN"),
        "tr should be TRAIN; parameters: {:?}",
        ev.parameters
    );
    assert_eq!(
        ev.parameters.get("vss_set").map(String::as_str),
        Some("ℙ(VSS)"),
        "vss_set should be ℙ(VSS); parameters: {:?}",
        ev.parameters
    );
}
