//! An EXTENDED event inherits its immediate abstract event's inaccuracy.
//!
//! Rodin marks an extended concrete event `accurate="false"` whenever the
//! abstract event it copies is itself inaccurate — the concrete event is no
//! longer a lossless reflection of the source. A plain (non-extended)
//! refinement does NOT inherit that flag: it re-states its own clauses.
//!
//! Isolation: the abstract event `INITIALISATION` is made inaccurate by the
//! untyped-variable lever (its action assigns an untyped variable, so the
//! action is dropped). The refining machine adds a typing invariant for the
//! same variable, so its own recomputation of the inherited action is clean.
//! Thus the concrete event is inaccurate *only* via inheritance.

use rossi_build::{Project, ProjectComponent, build, sc_view::ScView};

// Abstract machine: `x` has no typing invariant, so the INITIALISATION
// action `x ≔ 0` is dropped and INITIALISATION is inaccurate. The file
// itself stays accurate (event-level signal only).
const M0_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_v_x" org.eventb.core.identifier="x"/>
<org.eventb.core.event name="_init0" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;

// Refines M0, redeclares `x` and types it via `x ∈ ℤ`, and extends
// INITIALISATION. M1's own recomputation of the inherited `x ≔ 0` succeeds
// (x is typed here), so any inaccuracy must come from inheriting M0.
const M1_EXTENDED_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.variable name="_v_x1" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_i0" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init1" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION">
<org.eventb.core.refinesEvent name="_re_init" org.eventb.core.target="INITIALISATION"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;

// Same isolation, but a NON-extended INITIALISATION that re-states its own
// (typed) action. It must NOT inherit M0's inaccuracy.
const M1_PLAIN_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.variable name="_v_x1" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_i0" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init1" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;

fn extended_project() -> Project {
    Project::new(
        "ext",
        vec![
            ProjectComponent::from_xml("M0.bum", M0_BUM).unwrap(),
            ProjectComponent::from_xml("M1.bum", M1_EXTENDED_BUM).unwrap(),
        ],
    )
}

fn plain_project() -> Project {
    Project::new(
        "pln",
        vec![
            ProjectComponent::from_xml("M0.bum", M0_BUM).unwrap(),
            ProjectComponent::from_xml("M1.bum", M1_PLAIN_BUM).unwrap(),
        ],
    )
}

#[test]
fn abstract_event_is_inaccurate_baseline() {
    // Sanity: the abstract INITIALISATION is inaccurate, but M0 the file
    // stays accurate.
    let r = build(&extended_project());
    let m0 = r.file("M0.bcm").expect("M0.bcm");
    assert!(
        m0.accurate,
        "M0 file should stay accurate; {:?}",
        r.diagnostics
    );
    let v = ScView::from_xml(&m0.contents).unwrap();
    let init = v
        .events
        .get("INITIALISATION")
        .expect("INITIALISATION present");
    assert!(
        !init.accurate,
        "M0 INITIALISATION should be inaccurate (untyped LHS); {:?}",
        r.diagnostics
    );
}

#[test]
fn extended_event_inherits_abstract_inaccuracy() {
    let r = build(&extended_project());
    let m1 = r.file("M1.bcm").expect("M1.bcm");
    let v = ScView::from_xml(&m1.contents).unwrap();
    let init = v
        .events
        .get("INITIALISATION")
        .expect("INITIALISATION present");
    assert!(
        !init.accurate,
        "extended INITIALISATION must inherit M0's inaccuracy; {:?}",
        r.diagnostics
    );
}

#[test]
fn plain_refinement_does_not_inherit_inaccuracy() {
    let r = build(&plain_project());
    let m1 = r.file("M1.bcm").expect("M1.bcm");
    let v = ScView::from_xml(&m1.contents).unwrap();
    let init = v
        .events
        .get("INITIALISATION")
        .expect("INITIALISATION present");
    assert!(
        init.accurate,
        "non-extended INITIALISATION re-states its own typed action and must \
         stay accurate; {:?}",
        r.diagnostics
    );
}
