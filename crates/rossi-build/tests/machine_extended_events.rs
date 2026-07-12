//! M5: extended events and witnesses.
//!
//! Two patterns covered:
//!
//! 1. **Non-extended refinement** (`extended=false`) — concrete event has
//!    its own guards/actions but is tied to abstract event via
//!    `scRefinesEvent`. If the concrete event drops an abstract parameter,
//!    a `scWitness` carries the witnessing predicate.
//!
//! 2. **Extended refinement** (`extended=true`) — concrete event inherits
//!    *all* parameters/guards/actions from the abstract chain, emitted
//!    under the concrete `scEvent` with `source=` URIs pointing at the
//!    originating `.bum`.

use rossi_build::{Project, ProjectComponent, build};

fn project() -> Project {
    let ctx = ProjectComponent::from_xml(
        "Ctx.buc",
        r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.carrierSet name="_s" org.eventb.core.identifier="USERS"/>
</org.eventb.core.contextFile>"#,
    )
    .unwrap();
    // M0 — abstract machine.
    //   INITIALISATION: register := ∅
    //   found(e): guard e ∈ register → act1 r := e
    let m0 = ProjectComponent::from_xml(
        "M0.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_s0" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v_r" org.eventb.core.identifier="r"/>
<org.eventb.core.variable name="_v_reg" org.eventb.core.identifier="register"/>
<org.eventb.core.invariant name="_i0" org.eventb.core.label="inv1" org.eventb.core.predicate="register ⊆ USERS"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv2" org.eventb.core.predicate="r ∈ USERS"/>
<org.eventb.core.event name="_init0" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0_1" org.eventb.core.assignment="register ≔ ∅" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_found" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="found">
<org.eventb.core.parameter name="_p_e" org.eventb.core.identifier="e"/>
<org.eventb.core.guard name="_g0" org.eventb.core.label="grd1" org.eventb.core.predicate="e ∈ register"/>
<org.eventb.core.action name="_a0" org.eventb.core.assignment="r ≔ e" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
    // M1 refines M0.
    //   Adds variable `k`.
    //   INITIALISATION is extended=true — inherits `register := ∅`,
    //     adds `k := whatever`.
    //   found: not extended; has grd1 and action r := k; witnesses e = k.
    let m1 = ProjectComponent::from_xml(
        "M1.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.seesContext name="_s1" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v_k" org.eventb.core.identifier="k"/>
<org.eventb.core.variable name="r" org.eventb.core.identifier="r"/>
<org.eventb.core.variable name="register" org.eventb.core.identifier="register"/>
<org.eventb.core.invariant name="_i2" org.eventb.core.label="inv1" org.eventb.core.predicate="k ∈ register"/>
<org.eventb.core.event name="_init1" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION">
<org.eventb.core.refinesEvent name="_re_init" org.eventb.core.target="INITIALISATION"/>
<org.eventb.core.action name="_a1_init" org.eventb.core.assignment="k ≔ r" org.eventb.core.label="act2"/>
</org.eventb.core.event>
<org.eventb.core.event name="_found1" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="found">
<org.eventb.core.refinesEvent name="_re_found" org.eventb.core.target="found"/>
<org.eventb.core.guard name="_g_found" org.eventb.core.label="grd1" org.eventb.core.predicate="k ∈ register"/>
<org.eventb.core.action name="_a_found" org.eventb.core.assignment="r ≔ k" org.eventb.core.label="act1"/>
<org.eventb.core.witness name="_w_e" org.eventb.core.label="e" org.eventb.core.predicate="e = k"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
    Project::new("m5", vec![ctx, m0, m1])
}

fn m1_bcm() -> String {
    let r = build(&project());
    assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);
    r.file("M1.bcm").expect("M1.bcm").contents.clone()
}

#[test]
fn refines_event_emitted_on_non_extended() {
    let bcm = m1_bcm();
    assert!(
        bcm.contains(r#"<org.eventb.core.scRefinesEvent"#),
        "expected scRefinesEvent in:\n{bcm}"
    );
    assert!(
        bcm.contains("scTarget=") && bcm.contains("/m5/M0.bcm"),
        "scRefinesEvent should point at M0's scEvent:\n{bcm}"
    );
}

#[test]
fn witness_emitted_for_dropped_parameter() {
    let bcm = m1_bcm();
    // `found` in M1 drops the abstract parameter `e` — witness `e = k`
    // must appear inside that event.
    assert!(
        bcm.contains("<org.eventb.core.scWitness"),
        "expected scWitness in:\n{bcm}"
    );
    assert!(
        bcm.contains(r#"org.eventb.core.predicate="e=k""#),
        "witness predicate `e=k` missing:\n{bcm}"
    );
}

#[test]
fn extended_event_inherits_parent_actions() {
    let bcm = m1_bcm();
    // INITIALISATION is extended — it should inline M0's
    // `register ≔ ∅ ⦂ ℙ(USERS)` action AND its own `k ≔ r`.
    assert!(
        bcm.contains(r#"org.eventb.core.assignment="register ≔ ∅ ⦂ ℙ(USERS)""#),
        "expected inherited action from M0's INITIALISATION:\n{bcm}"
    );
    assert!(
        bcm.contains(r#"org.eventb.core.assignment="k ≔ r""#),
        "expected own action from M1's INITIALISATION:\n{bcm}"
    );
    // The inherited action's source should be M0.bum, not M1.bum.
    assert!(
        bcm.contains(r#"source="/m5/M0.bum"#) && bcm.contains(r#"action#_a0_1""#),
        "inherited action should carry M0.bum source:\n{bcm}"
    );
}

#[test]
fn non_extended_event_does_not_inherit_parent_guards() {
    // M1's `found` has its own `grd1` referring to `k`; the abstract's
    // `grd1: e ∈ register` should NOT appear (extended=false).
    let bcm = m1_bcm();
    assert!(
        !bcm.contains(r#"predicate="e∈register""#),
        "non-extended event should not carry abstract guards; got:\n{bcm}"
    );
}

#[test]
fn extended_event_redeclaring_inherited_parameter_is_diagnosed() {
    let m0 = ProjectComponent::from_xml(
        "M0.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION"/>
<org.eventb.core.event name="_event" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt">
<org.eventb.core.parameter name="_param" org.eventb.core.identifier="p"/>
<org.eventb.core.guard name="_guard" org.eventb.core.label="grd1" org.eventb.core.predicate="p ∈ ℤ"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
    let m1 = ProjectComponent::from_xml(
        "M1.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref_m" org.eventb.core.target="M0"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION">
<org.eventb.core.refinesEvent name="_ref_i" org.eventb.core.target="INITIALISATION"/>
</org.eventb.core.event>
<org.eventb.core.event name="_event" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="evt">
<org.eventb.core.refinesEvent name="_ref_e" org.eventb.core.target="evt"/>
<org.eventb.core.parameter name="_param" org.eventb.core.identifier="p"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
    let result = build(&Project::new("conflict", vec![m0, m1]));
    assert!(
        result.diagnostics.iter().any(|diagnostic| {
            diagnostic.origin == "M1.evt.p"
                && diagnostic
                    .message
                    .contains("parameter `p` conflicts with an inherited parameter")
        }),
        "expected inherited parameter conflict: {:?}",
        result.diagnostics
    );
}
