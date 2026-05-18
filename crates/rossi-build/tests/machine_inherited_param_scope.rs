//! C1+C2: an `extended="true"` concrete event inherits parameters
//! from the abstract chain. Both:
//!
//! 1. Type-inference for any new own parameters must see inherited
//!    guards as typing axioms (C1).
//! 2. When checking the event's own guards and actions, inherited
//!    parameter names must be in the type env (C2).
//!
//! The common real-world pattern: a
//! concrete event `E` is extended="true" without redeclaring the
//! abstract parameter, and adds its own guard that references the
//! inherited parameter. Our current code drops that guard because the
//! identifier resolution walker can't see the inherited parameter.

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
    // M0: `E(u)` with guard `u ∈ USERS`, action `registered := registered ∪ {u}`.
    let m0 = ProjectComponent::from_xml(
        "M0.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_s0" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v_reg" org.eventb.core.identifier="registered"/>
<org.eventb.core.invariant name="_i0" org.eventb.core.label="inv1" org.eventb.core.predicate="registered ⊆ USERS"/>
<org.eventb.core.event name="_init0" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="registered ≔ ∅" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_e" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="E">
<org.eventb.core.parameter name="_p_u" org.eventb.core.identifier="u"/>
<org.eventb.core.guard name="_g1" org.eventb.core.label="grd1" org.eventb.core.predicate="u ∈ USERS"/>
<org.eventb.core.action name="_a1" org.eventb.core.assignment="registered ≔ registered ∪ {u}" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
    // M1 REFINES M0. `E` is extended="true" and adds its own guard
    // `u ∉ registered` referencing the INHERITED parameter u. No
    // explicit <parameter> redeclaration — that's the implicit pattern.
    let m1 = ProjectComponent::from_xml(
        "M1.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.seesContext name="_s1" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v_reg" org.eventb.core.identifier="registered"/>
<org.eventb.core.event name="_init1" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION"></org.eventb.core.event>
<org.eventb.core.event name="_e1" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="E">
<org.eventb.core.guard name="_g_own" org.eventb.core.label="grd_own" org.eventb.core.predicate="u ∉ registered"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
    Project::new("mc12", vec![ctx, m0, m1])
}

fn m1_view() -> ScView {
    let r = build(&project());
    assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);
    ScView::from_xml(&r.file("M1.bcm").expect("M1.bcm").contents).unwrap()
}

#[test]
fn own_guard_referencing_inherited_param_survives() {
    // The concrete `E`'s own guard `u ∉ registered` names the
    // inherited parameter `u`. It must appear in M1.bcm — the
    // identifier walker must see `u` bound through the extended chain.
    let v = m1_view();
    let e = v.events.get("E").expect("E");
    let own_guard = e
        .guards
        .values()
        .find(|g| g.label == "grd_own")
        .expect("own guard grd_own");
    assert_eq!(
        own_guard.predicate,
        rossi::parse_predicate_str("u ∉ registered").unwrap(),
    );
}

#[test]
fn extended_event_carries_all_guards() {
    // Inherited guard `u ∈ USERS` PLUS own guard `u ∉ registered`.
    let v = m1_view();
    let e = v.events.get("E").expect("E");
    assert_eq!(
        e.guards.len(),
        2,
        "expected both inherited and own guard; got {:#?}",
        e.guards
    );
}

#[test]
fn event_stays_accurate() {
    let v = m1_view();
    let e = v.events.get("E").expect("E");
    assert!(
        e.accurate,
        "event should be accurate; something was dropped"
    );
}

#[test]
fn parameter_inherited() {
    // `u` should still show up as an scParameter of E (inherited).
    let v = m1_view();
    let e = v.events.get("E").expect("E");
    assert_eq!(e.parameters.get("u").map(String::as_str), Some("USERS"));
}
