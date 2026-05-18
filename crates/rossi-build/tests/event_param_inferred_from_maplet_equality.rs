//! Group N: an event parameter typed only via the equality of a
//! maplet against a function application — `m ↦ t = f(port)` — must
//! decompose the function's product codomain across the maplet's
//! leaves. Rodin parity — verified against a real-world corpus
//! machine whose guard types parameters `m` and `t` via
//! `m ↦ t = f(port)`.

use rossi_build::{Project, ProjectComponent, build, sc_view::ScView};

const CTX_BUC: &str = r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.carrierSet name="_set1" org.eventb.core.identifier="PORTS"/>
<org.eventb.core.carrierSet name="_set2" org.eventb.core.identifier="MESSAGES"/>
<org.eventb.core.constant name="_c1" org.eventb.core.identifier="msgspace"/>
<org.eventb.core.axiom name="_a1" org.eventb.core.label="axm1" org.eventb.core.predicate="msgspace ∈ PORTS ⇸ MESSAGES × ℤ"/>
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
<org.eventb.core.event name="_ev_read" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="read_message">
<org.eventb.core.parameter name="_p1" org.eventb.core.identifier="port"/>
<org.eventb.core.parameter name="_p2" org.eventb.core.identifier="m"/>
<org.eventb.core.parameter name="_p3" org.eventb.core.identifier="t"/>
<org.eventb.core.guard name="_g1" org.eventb.core.label="grd1" org.eventb.core.predicate="port ∈ dom(msgspace)"/>
<org.eventb.core.guard name="_g2" org.eventb.core.label="grd2" org.eventb.core.predicate="m ↦ t = msgspace(port)"/>
<org.eventb.core.action name="_a1" org.eventb.core.assignment="last ≔ t" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;

fn project() -> Project {
    Project::new(
        "n",
        vec![
            ProjectComponent::from_xml("Ctx.buc", CTX_BUC).unwrap(),
            ProjectComponent::from_xml("Mch.bum", MACHINE_BUM).unwrap(),
        ],
    )
}

#[test]
fn parameter_typed_via_maplet_equality_against_function_application() {
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
        .get("read_message")
        .expect("read_message event present");
    assert!(
        ev.accurate,
        "event should be accurate; diagnostics: {:?}",
        r.diagnostics
    );
    assert_eq!(
        ev.parameters.get("m").map(String::as_str),
        Some("MESSAGES"),
        "m should be MESSAGES; parameters: {:?}",
        ev.parameters
    );
    assert_eq!(
        ev.parameters.get("t").map(String::as_str),
        Some("ℤ"),
        "t should be ℤ; parameters: {:?}",
        ev.parameters
    );
}
