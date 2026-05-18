//! B1: emit `scVariant` element for a machine that declares a VARIANT.
//!
//! Rodin's shape (from binary-search/M2.bcm):
//!
//!   <org.eventb.core.scVariant name="C7" org.eventb.core.expression="j − i"
//!       org.eventb.core.label="vrn"
//!       org.eventb.core.source="/binary-search/M2.bum|...|variant#7"/>
//!
//! Emission order inside `scMachineFile`: scInvariants → scVariables →
//! scVariant → scEvents (confirmed against M2.bcm).

use rossi_build::{Project, ProjectComponent, build, sc_view::ScView};

const CTX_BUC: &str = r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
</org.eventb.core.contextFile>
"#;

/// Minimal machine with a VARIANT for a convergent event.
/// - variable `n`
/// - invariant `n ∈ ℕ`
/// - variant `n` (must decrease on each convergent event)
/// - event `decrement` convergent, guard `n > 0`, action `n := n − 1`
const MACHINE_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_s" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v" org.eventb.core.identifier="n"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="n ∈ ℕ"/>
<org.eventb.core.variant name="_vr" org.eventb.core.expression="n"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="n ≔ 10" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_dec" org.eventb.core.convergence="1" org.eventb.core.extended="false" org.eventb.core.label="decrement">
<org.eventb.core.guard name="_g" org.eventb.core.label="grd1" org.eventb.core.predicate="n &gt; 0"/>
<org.eventb.core.action name="_a" org.eventb.core.assignment="n ≔ n − 1" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;

fn project() -> Project {
    Project::new(
        "mb1",
        vec![
            ProjectComponent::from_xml("Ctx.buc", CTX_BUC).unwrap(),
            ProjectComponent::from_xml("Mch.bum", MACHINE_BUM).unwrap(),
        ],
    )
}

#[test]
fn sc_variant_element_emitted() {
    let r = build(&project());
    assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);
    let bcm = &r.file("Mch.bcm").expect("Mch.bcm").contents;
    assert!(
        bcm.contains("<org.eventb.core.scVariant"),
        "expected scVariant in:\n{bcm}"
    );
    assert!(
        bcm.contains(r#"org.eventb.core.expression="n""#),
        "expected variant expression `n`:\n{bcm}"
    );
    assert!(
        bcm.contains(r#"org.eventb.core.label="vrn""#),
        "expected default label `vrn`:\n{bcm}"
    );
}

#[test]
fn variant_appears_between_variables_and_events() {
    let r = build(&project());
    let bcm = &r.file("Mch.bcm").expect("Mch.bcm").contents;
    let idx_var = bcm.find("<org.eventb.core.scVariable").unwrap();
    let idx_variant = bcm.find("<org.eventb.core.scVariant").unwrap();
    let idx_event = bcm.find("<org.eventb.core.scEvent").unwrap();
    assert!(
        idx_var < idx_variant && idx_variant < idx_event,
        "expected scVariable → scVariant → scEvent order; got {idx_var}, {idx_variant}, {idx_event}"
    );
}

#[test]
fn sc_view_captures_variant() {
    let r = build(&project());
    let bcm = &r.file("Mch.bcm").expect("Mch.bcm").contents;
    let view = ScView::from_xml(bcm).unwrap();
    assert_eq!(view.variant.as_deref(), Some("n"));
}
