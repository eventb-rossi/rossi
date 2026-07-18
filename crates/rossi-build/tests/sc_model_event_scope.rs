//! The `ScModel` surface that downstream formula passes (well-
//! definedness) build on:
//!
//! - `CheckedMachine::record` carries the typed invariant / variant /
//!   event ASTs the `.bcm` was rendered from.
//! - `EventDecl::chain_parameters` exposes inherited parameters of an
//!   `extended="true"` event without redeclaration.
//! - `CheckedMachine::event_env` rebuilds the event-local type scope
//!   (machine env + chain parameters).

use rossi_build::normalize::{canonical_expression, canonical_predicate};
use rossi_build::{Project, ProjectComponent, Type, build_with_model};

fn project() -> Project {
    let ctx = ProjectComponent::from_xml(
        "Ctx.buc",
        r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.carrierSet name="_s" org.eventb.core.identifier="USERS"/>
</org.eventb.core.contextFile>"#,
    )
    .unwrap();
    // M0: `E(u)` with guard `u ∈ USERS`, plus a variant so the record
    // carries a typed variant expression.
    let m0 = ProjectComponent::from_xml(
        "M0.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_s0" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v_reg" org.eventb.core.identifier="registered"/>
<org.eventb.core.invariant name="_i0" org.eventb.core.label="inv1" org.eventb.core.predicate="registered ⊆ USERS ∧ (∀x · x ∈ ℤ ⇒ x = x)"/>
<org.eventb.core.variant name="_vr" org.eventb.core.expression="card({x ∣ x ∈ registered} ∖ registered)"/>
<org.eventb.core.event name="_init0" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="registered ≔ ∅" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_e" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="E">
<org.eventb.core.parameter name="_p_u" org.eventb.core.identifier="u"/>
<org.eventb.core.guard name="_g1" org.eventb.core.label="grd1" org.eventb.core.predicate="u ∈ USERS"/>
<org.eventb.core.action name="_a1" org.eventb.core.assignment="registered ≔ registered ∪ {u}" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_w" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="W">
<org.eventb.core.parameter name="_p" org.eventb.core.identifier="p"/>
<org.eventb.core.guard name="_g_w" org.eventb.core.label="grd1" org.eventb.core.predicate="p ∈ ℤ"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
    // M1 REFINES M0. `E` is extended="true" without redeclaring `u`.
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
<org.eventb.core.event name="_w1" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="W">
<org.eventb.core.refinesEvent name="_re" org.eventb.core.target="W"/>
<org.eventb.core.witness name="_wit" org.eventb.core.label="p" org.eventb.core.predicate="p = 0 ∧ (∀z · z = p)"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
    Project::new("scope", vec![ctx, m0, m1])
}

#[test]
fn machine_record_carries_enriched_formulas() {
    let (r, model) = build_with_model(&project());
    assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);

    let m0 = model.machines.get("M0").expect("M0 in model");
    assert_eq!(m0.record.invariants.len(), 1);
    let invariant = &m0.record.invariants[0];
    assert_eq!(invariant.label, "inv1");
    assert_eq!(
        canonical_predicate(&invariant.predicate),
        invariant.predicate_canonical
    );
    assert!(
        invariant.predicate_canonical.contains("∀x⦂ℤ·"),
        "invariant binder should be enriched: {:?}",
        invariant.predicate
    );

    let variant = m0.record.variant.as_ref().expect("variant in record");
    assert_eq!(
        canonical_expression(&variant.expression),
        variant.expression_canonical
    );
    assert_eq!(
        variant.expression_canonical,
        "card({x⦂USERS·x∈registered∣x} ∖ registered)"
    );

    let event = model.machines["M1"]
        .events_by_label
        .get("W")
        .expect("W in M1");
    let witness = event.witnesses.first().expect("p witness");
    assert_eq!(witness.label, "p");
    assert_eq!(
        canonical_predicate(&witness.predicate),
        witness.predicate_canonical
    );
    assert!(
        witness.predicate_canonical.contains("∀z⦂ℤ·"),
        "witness binder should be enriched: {:?}",
        witness.predicate
    );
}

#[test]
fn chain_parameters_sees_inherited_param() {
    let (r, model) = build_with_model(&project());
    assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);

    let m1 = model.machines.get("M1").expect("M1 in model");
    let e = m1.events_by_label.get("E").expect("event E");
    // M1's E declares no own parameters; `u` arrives via the chain.
    assert!(e.parameters.is_empty());
    let params: Vec<&str> = e
        .chain_parameters()
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(params, ["u"]);

    // A non-extended event has no chain.
    let m0 = model.machines.get("M0").expect("M0 in model");
    let e0 = m0.events_by_label.get("E").expect("event E in M0");
    let own: Vec<&str> = e0
        .chain_parameters()
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(own, ["u"]);
}

#[test]
fn event_env_resolves_chain_params_and_variables() {
    let (r, model) = build_with_model(&project());
    assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);

    let m1 = model.machines.get("M1").expect("M1 in model");
    let e = m1.events_by_label.get("E").expect("event E");
    let env = m1.event_env(e);

    let users = Type::GivenSet("USERS".into());
    assert_eq!(env.get("u"), Some(&users), "inherited parameter typed");
    assert_eq!(
        env.get("registered"),
        Some(&Type::pow(users.clone())),
        "machine variable visible"
    );
    assert!(env.get("nonexistent").is_none());
}
