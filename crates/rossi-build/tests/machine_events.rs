//! M2: events without refinement.
//!
//! Exercises: INITIALISATION, events with parameters (ANY) / guards (WHERE) /
//! actions (THEN), parameter type inference from guards, convergence
//! encoding, and empty-set type ascription on assignment RHS.

use rossi_build::{Project, ProjectComponent, build, sc_view::ScView};

const CTX_BUC: &str = r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.carrierSet name="_set1" org.eventb.core.identifier="USERS"/>
</org.eventb.core.contextFile>
"#;

/// Machine with:
///   INITIALISATION: `registered := ∅`
///   Register(u): guard `u ∈ USERS`, action `registered := registered ∪ {u}`
///   Leave(u): convergent, guard `u ∈ registered`, action `registered := registered ∖ {u}`
const MACHINE_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_s1" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v1" org.eventb.core.identifier="registered"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="registered ⊆ USERS"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="registered ≔ ∅" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_ev_reg" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="Register">
<org.eventb.core.parameter name="_p1" org.eventb.core.identifier="u"/>
<org.eventb.core.guard name="_g1" org.eventb.core.label="grd1" org.eventb.core.predicate="u ∈ USERS"/>
<org.eventb.core.action name="_a1" org.eventb.core.assignment="registered ≔ registered ∪ {u}" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_ev_leave" org.eventb.core.convergence="1" org.eventb.core.extended="false" org.eventb.core.label="Leave">
<org.eventb.core.parameter name="_p2" org.eventb.core.identifier="u"/>
<org.eventb.core.guard name="_g2" org.eventb.core.label="grd1" org.eventb.core.predicate="u ∈ registered"/>
<org.eventb.core.action name="_a2" org.eventb.core.assignment="registered ≔ registered ∖ {u}" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;

fn make_project() -> Project {
    Project::new(
        "m2",
        vec![
            ProjectComponent::from_xml("Ctx.buc", CTX_BUC).unwrap(),
            ProjectComponent::from_xml("Mch.bum", MACHINE_BUM).unwrap(),
        ],
    )
}

fn machine_view() -> ScView {
    let r = build(&make_project());
    assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);
    ScView::from_xml(&r.file("Mch.bcm").expect("Mch.bcm").contents).unwrap()
}

#[test]
fn machine_has_three_events() {
    let v = machine_view();
    assert!(v.events.contains_key("INITIALISATION"));
    assert!(v.events.contains_key("Register"));
    assert!(v.events.contains_key("Leave"));
}

#[test]
fn initialisation_action_gets_empty_set_type_ascription() {
    // `registered ≔ ∅` should canonicalize to `registered ≔ ∅ ⦂ ℙ(USERS)`
    // because `registered : ℙ(USERS)` is known.
    let r = build(&make_project());
    let bcm = &r.file("Mch.bcm").expect("Mch.bcm").contents;
    assert!(
        bcm.contains(r#"org.eventb.core.assignment="registered ≔ ∅ ⦂ ℙ(USERS)""#),
        "expected empty-set type ascription in INITIALISATION:\n{bcm}"
    );
}

#[test]
fn parameter_type_is_inferred_from_guard() {
    // Register.u : USERS (from `u ∈ USERS`, where USERS : ℙ(USERS))
    let v = machine_view();
    let reg = v.events.get("Register").expect("Register");
    assert_eq!(reg.parameters.get("u").map(String::as_str), Some("USERS"));
}

#[test]
fn parameter_typed_via_variable_reference() {
    // Leave.u : USERS (from `u ∈ registered`, where registered : ℙ(USERS))
    let v = machine_view();
    let leave = v.events.get("Leave").expect("Leave");
    assert_eq!(leave.parameters.get("u").map(String::as_str), Some("USERS"));
}

#[test]
fn convergence_encoding() {
    let v = machine_view();
    assert_eq!(
        v.events
            .get("Register")
            .and_then(|e| e.convergence.as_deref()),
        Some("0")
    );
    assert_eq!(
        v.events.get("Leave").and_then(|e| e.convergence.as_deref()),
        Some("1")
    );
}

#[test]
fn event_guards_and_actions_captured_by_sc_view() {
    let v = machine_view();
    let reg = v.events.get("Register").expect("Register");
    assert_eq!(reg.guards.len(), 1, "Register has one guard");
    assert_eq!(reg.actions.len(), 1, "Register has one action");
    // Guard predicate is `u∈USERS` (canonical) — ScView parses it back.
    // Guards are keyed by source URI now (labels can collide across REFINES).
    let grd1 = reg
        .guards
        .values()
        .find(|g| g.label == "grd1")
        .expect("grd1 by label");
    assert!(!grd1.theorem);
    assert_eq!(
        grd1.predicate,
        rossi::parse_predicate_str("u ∈ USERS").unwrap()
    );
}

#[test]
fn all_events_are_accurate() {
    let v = machine_view();
    for (label, e) in &v.events {
        assert!(e.accurate, "{label} should be accurate");
    }
}

#[test]
fn machine_file_is_accurate_when_all_events_type_check() {
    let r = build(&make_project());
    let bcm = r.file("Mch.bcm").expect("Mch.bcm");
    assert!(bcm.accurate, "diagnostics: {:?}", r.diagnostics);
}
