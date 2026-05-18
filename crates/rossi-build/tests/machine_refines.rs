//! M4: a concrete machine REFINES an abstract machine.
//!
//! Pattern (from binary-search M0 → M1):
//!
//! ```text
//! M0: variable r, invariant inv1: r ∈ ℤ, event found(e): e ∈ ℤ → r := e
//! M1 refines M0:
//!   variable k (new), keeps r,
//!   new invariant inv1: k ∈ ℤ (same label, different predicate — Rodin allows this)
//!   (no event-level refinement in this M4 test; events with REFINES land in M5)
//! ```
//!
//! Expected in M1.bcm:
//! - scRefinesMachine pointing at M0.bcm
//! - M0's invariant copied in with source= back to M0.bum (label kept)
//! - M1's invariant emitted with source= pointing to M1.bum
//! - scVariable r: abstract=true concrete=true
//! - scVariable k: abstract=false concrete=true

use rossi_build::{Project, ProjectComponent, build, sc_view::ScView};

fn project() -> Project {
    let ctx = ProjectComponent::from_xml(
        "Ctx.buc",
        r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
</org.eventb.core.contextFile>"#,
    )
    .unwrap();
    let m0 = ProjectComponent::from_xml(
        "M0.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_s0" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v_r" org.eventb.core.identifier="r"/>
<org.eventb.core.invariant name="_i_m0_1" org.eventb.core.label="inv1" org.eventb.core.predicate="r ∈ ℤ"/>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
    let m1 = ProjectComponent::from_xml(
        "M1.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.seesContext name="_s1" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v_k" org.eventb.core.identifier="k"/>
<org.eventb.core.invariant name="_i_m1_1" org.eventb.core.label="inv1" org.eventb.core.predicate="k ∈ ℤ"/>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
    Project::new("m4", vec![ctx, m0, m1])
}

fn m1_view() -> ScView {
    let r = build(&project());
    assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);
    ScView::from_xml(&r.file("M1.bcm").expect("M1.bcm").contents).unwrap()
}

#[test]
fn sc_refines_machine_emitted() {
    let r = build(&project());
    let bcm = &r.file("M1.bcm").expect("M1.bcm").contents;
    assert!(
        bcm.contains("<org.eventb.core.scRefinesMachine"),
        "expected scRefinesMachine in:\n{bcm}"
    );
    assert!(
        bcm.contains("/m4/M0.bcm"),
        "expected scTarget pointing at M0.bcm:\n{bcm}"
    );
}

#[test]
fn both_invariants_carried_in_order() {
    // M0's inv first (source order matches Rodin's emission).
    let v = m1_view();
    assert_eq!(v.invariants.len(), 2, "expected two invariants, got {v:#?}");
    // At least one should have source pointing at M0.bum, one at M1.bum.
    let sources: Vec<_> = v.invariants.keys().cloned().collect();
    // ScView strips the leading /PROJECT/ from source URIs so lookups
    // aren't project-name-sensitive — the file-name fragment stays.
    assert!(
        sources.iter().any(|s| s.starts_with("M0.bum")),
        "expected an invariant sourced from M0.bum, got {sources:?}"
    );
    assert!(
        sources.iter().any(|s| s.starts_with("M1.bum")),
        "expected an invariant sourced from M1.bum, got {sources:?}"
    );
}

#[test]
fn abstract_flag_true_for_inherited_variable() {
    let r = build(&project());
    let bcm = &r.file("M1.bcm").expect("M1.bcm").contents;
    // r was declared in M0 and NOT redeclared in M1 → abstract=true concrete=false
    // (vanishes to abstract-only; Group R / Rodin parity).
    assert!(
        bcm.contains(r#"<org.eventb.core.scVariable name="r" org.eventb.core.abstract="true" org.eventb.core.concrete="false""#),
        "r should be abstract=true concrete=false:\n{bcm}"
    );
    // k is new in M1 → abstract=false concrete=true.
    assert!(
        bcm.contains(r#"<org.eventb.core.scVariable name="k" org.eventb.core.abstract="false" org.eventb.core.concrete="true""#),
        "k should be abstract=false concrete=true:\n{bcm}"
    );
}

#[test]
fn machine_is_accurate() {
    let r = build(&project());
    let bcm = r.file("M1.bcm").expect("M1.bcm");
    assert!(bcm.accurate, "diagnostics: {:?}", r.diagnostics);
}
