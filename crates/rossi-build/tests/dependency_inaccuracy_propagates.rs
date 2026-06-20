//! Inaccuracy propagates up from a present-but-inaccurate dependency.
//!
//! A component computes its own accuracy from its own clauses, then folds in
//! the accuracy of each dependency it builds on: extending or seeing an
//! inaccurate context, or refining an inaccurate machine, makes the dependent
//! inaccurate too. This mirrors the Event-B static-checker reference, where
//! the propagation is silent (no new problem marker — the inaccuracy is the
//! dependency's own reported problem).
//!
//! Each dependent below has clean own-clauses, so its inaccuracy comes
//! *only* through propagation. The missing-target case is unaffected: it keeps
//! its existing diagnostic and is exercised elsewhere.

use rossi_build::{Project, ProjectComponent, build};

// Inaccurate context: constant `c` has no typing axiom, so it is unresolved
// and the context file is `accurate="false"`.
const C1_INACCURATE: &str = r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.constant name="_c" org.eventb.core.identifier="c"/>
</org.eventb.core.contextFile>"#;

// Extends the inaccurate C1; own carrier set is fine.
const C2_EXTENDS_C1: &str = r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.extendsContext name="_e" org.eventb.core.target="C1"/>
<org.eventb.core.carrierSet name="_s" org.eventb.core.identifier="S"/>
</org.eventb.core.contextFile>"#;

// Extends C2, which is inaccurate only via propagation — proves transitivity.
const C3_EXTENDS_C2: &str = r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.extendsContext name="_e2" org.eventb.core.target="C2"/>
</org.eventb.core.contextFile>"#;

// Sees the inaccurate C1; no own clauses.
const MS_SEES_C1: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_s" org.eventb.core.target="C1"/>
</org.eventb.core.machineFile>"#;

fn context_project() -> Project {
    Project::new(
        "ctxprop",
        vec![
            ProjectComponent::from_xml("C1.buc", C1_INACCURATE).unwrap(),
            ProjectComponent::from_xml("C2.buc", C2_EXTENDS_C1).unwrap(),
            ProjectComponent::from_xml("C3.buc", C3_EXTENDS_C2).unwrap(),
            ProjectComponent::from_xml("MS.bum", MS_SEES_C1).unwrap(),
        ],
    )
}

#[test]
fn inaccurate_context_baseline() {
    // Sanity: the lever actually produces an inaccurate context file.
    let r = build(&context_project());
    assert!(
        !r.file("C1.bcc").expect("C1.bcc").accurate,
        "C1 should be inaccurate (unresolved constant); {:?}",
        r.diagnostics
    );
}

#[test]
fn extends_inaccurate_context_propagates() {
    let r = build(&context_project());
    assert!(
        !r.file("C2.bcc").expect("C2.bcc").accurate,
        "C2 EXTENDS inaccurate C1 ⇒ inaccurate; {:?}",
        r.diagnostics
    );
}

#[test]
fn extends_propagation_is_transitive() {
    let r = build(&context_project());
    assert!(
        !r.file("C3.bcc").expect("C3.bcc").accurate,
        "C3 EXTENDS C2 (inaccurate via propagation) ⇒ inaccurate; {:?}",
        r.diagnostics
    );
}

#[test]
fn sees_inaccurate_context_propagates() {
    let r = build(&context_project());
    assert!(
        !r.file("MS.bcm").expect("MS.bcm").accurate,
        "MS SEES inaccurate C1 ⇒ inaccurate; {:?}",
        r.diagnostics
    );
}

// Inaccurate machine: invariant references an undeclared identifier, so the
// invariant is dropped and the machine file is `accurate="false"`.
const M0_INACCURATE: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="undeclared ∈ ℤ"/>
</org.eventb.core.machineFile>"#;

// Refines the inaccurate M0; no own clauses.
const M1_REFINES_M0: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_r" org.eventb.core.target="M0"/>
</org.eventb.core.machineFile>"#;

fn machine_project() -> Project {
    Project::new(
        "machprop",
        vec![
            ProjectComponent::from_xml("M0.bum", M0_INACCURATE).unwrap(),
            ProjectComponent::from_xml("M1.bum", M1_REFINES_M0).unwrap(),
        ],
    )
}

#[test]
fn inaccurate_machine_baseline() {
    let r = build(&machine_project());
    assert!(
        !r.file("M0.bcm").expect("M0.bcm").accurate,
        "M0 should be inaccurate (dropped invariant); {:?}",
        r.diagnostics
    );
}

#[test]
fn refines_inaccurate_machine_propagates() {
    let r = build(&machine_project());
    assert!(
        !r.file("M1.bcm").expect("M1.bcm").accurate,
        "M1 REFINES inaccurate M0 ⇒ inaccurate; {:?}",
        r.diagnostics
    );
}
