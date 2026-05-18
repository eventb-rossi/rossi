//! B2: `extended="true"` events in a refined machine imply an
//! `scRefinesEvent` pointing at the abstract event with the same label,
//! even when the `.bum` has no explicit `<refinesEvent>` child.
//!
//! Rodin's SC synthesises the refinesEvent during checking. Our parser
//! only fills `event.refines` when an explicit child element is present,
//! so we must detect the implicit case and emit the `scRefinesEvent`
//! anyway.
//!
//! Covers two subpatterns:
//!
//! 1. INITIALISATION in M1 extends INITIALISATION from M0 (classic
//!    refinement pattern; most machines look like this).
//! 2. A regular named event `E` in M1 declared with `extended="true"`
//!    and no `<refinesEvent>` child.

use rossi_build::{Project, ProjectComponent, build, sc_view::ScView};

fn project() -> Project {
    let ctx = ProjectComponent::from_xml(
        "Ctx.buc",
        r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.carrierSet name="_s" org.eventb.core.identifier="USERS"/>
</org.eventb.core.contextFile>"#,
    )
    .unwrap();
    // M0: INITIALISATION with action, and event `E(u)` with guard and action.
    let m0 = ProjectComponent::from_xml(
        "M0.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_s0" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v_reg" org.eventb.core.identifier="registered"/>
<org.eventb.core.invariant name="_i0" org.eventb.core.label="inv1" org.eventb.core.predicate="registered ⊆ USERS"/>
<org.eventb.core.event name="_init0" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a_init" org.eventb.core.assignment="registered ≔ ∅" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_ev_E" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="E">
<org.eventb.core.parameter name="_p_u" org.eventb.core.identifier="u"/>
<org.eventb.core.guard name="_g_E" org.eventb.core.label="grd1" org.eventb.core.predicate="u ∈ USERS"/>
<org.eventb.core.action name="_a_E" org.eventb.core.assignment="registered ≔ registered ∪ {u}" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
    // M1: REFINES M0. BOTH events use extended="true" with no
    // <refinesEvent> child (the text-level `extends INITIALISATION`
    // sugar leaves no explicit refinesEvent in the XML).
    let m1 = ProjectComponent::from_xml(
        "M1.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.seesContext name="_s1" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v_reg" org.eventb.core.identifier="registered"/>
<org.eventb.core.event name="_init1" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION"></org.eventb.core.event>
<org.eventb.core.event name="_ev_E1" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="E"></org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
    Project::new("mb2", vec![ctx, m0, m1])
}

fn m1_view() -> ScView {
    let r = build(&project());
    assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);
    ScView::from_xml(&r.file("M1.bcm").expect("M1.bcm").contents).unwrap()
}

#[test]
fn initialisation_refines_event_synthesised() {
    // `extended="true"` INITIALISATION in M1 with no explicit
    // refinesEvent must still get an scRefinesEvent pointing at M0.
    let v = m1_view();
    let init = v.events.get("INITIALISATION").expect("INITIALISATION");
    assert_eq!(
        init.refines_events.len(),
        1,
        "INITIALISATION should have exactly one scRefinesEvent (inherited); got {:#?}",
        init.refines_events
    );
    let target = init.refines_events.values().next().unwrap();
    assert!(
        target.contains("M0.bcm") && target.contains("scMachineFile#M0"),
        "scRefinesEvent should point at M0's scMachineFile; got {target}"
    );
}

#[test]
fn regular_extended_event_refines_event_synthesised() {
    let v = m1_view();
    let e = v.events.get("E").expect("E");
    assert_eq!(
        e.refines_events.len(),
        1,
        "E should have scRefinesEvent (inherited); got {:#?}",
        e.refines_events
    );
    let target = e.refines_events.values().next().unwrap();
    assert!(
        target.contains("M0.bcm") && target.contains("scMachineFile#M0"),
        "scRefinesEvent should point at M0's scMachineFile; got {target}"
    );
}

#[test]
fn extended_event_inherits_actions() {
    // Sanity: the inherited actions show up (already tested by M5 but
    // useful to pin here so B2 doesn't accidentally re-break it).
    let v = m1_view();
    let init = v.events.get("INITIALISATION").expect("INITIALISATION");
    assert_eq!(init.actions.len(), 1);
    let e = v.events.get("E").expect("E");
    assert_eq!(e.actions.len(), 1);
}
