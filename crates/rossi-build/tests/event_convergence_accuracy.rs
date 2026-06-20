//! Convergence-refinement accuracy: a concrete event may not claim a
//! stronger convergence than the ordinary abstract event it refines.
//! Rodin downgrades such an event's convergence to ordinary, emits the
//! *downgraded* code, and marks the event `accurate="false"` (the SC
//! output no longer reflects the declared source). The machine file itself
//! stays accurate — per-event inaccuracy does not bubble up.
//!
//! Mirrors Rodin's `TestAccuracy.testAcc_19`.

use rossi_build::{Project, ProjectComponent, build, sc_view::ScView};

/// Abstract machine: variable `n`, variant `n`, an ordinary event `ord`
/// and a convergent event `conv`.
const ABSTRACT_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_v" org.eventb.core.identifier="n"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="n ∈ ℕ"/>
<org.eventb.core.variant name="_vr" org.eventb.core.expression="n"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="n ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_ord" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="ord">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="n ≔ n + 1" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_conv" org.eventb.core.convergence="1" org.eventb.core.extended="false" org.eventb.core.label="conv">
<org.eventb.core.action name="_a2" org.eventb.core.assignment="n ≔ n − 1" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;

/// Concrete machine refining the abstract one. Keeps `n` (no disappearing
/// variables, so no witnesses are involved). Three refining events:
///   `bad`    convergent, refines the ordinary `ord`  → downgraded + inaccurate
///   `badant` anticipated, refines the ordinary `ord` → downgraded + inaccurate
///   `good`   convergent, refines the convergent `conv` → kept + accurate
const CONCRETE_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.variable name="_v" org.eventb.core.identifier="n"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="n ∈ ℕ"/>
<org.eventb.core.variant name="_vr" org.eventb.core.expression="n"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="n ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_bad" org.eventb.core.convergence="1" org.eventb.core.extended="false" org.eventb.core.label="bad">
<org.eventb.core.refinesEvent name="_re" org.eventb.core.target="ord"/>
<org.eventb.core.action name="_a1" org.eventb.core.assignment="n ≔ n + 1" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_badant" org.eventb.core.convergence="2" org.eventb.core.extended="false" org.eventb.core.label="badant">
<org.eventb.core.refinesEvent name="_re" org.eventb.core.target="ord"/>
<org.eventb.core.action name="_a1" org.eventb.core.assignment="n ≔ n + 1" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_good" org.eventb.core.convergence="1" org.eventb.core.extended="false" org.eventb.core.label="good">
<org.eventb.core.refinesEvent name="_re" org.eventb.core.target="conv"/>
<org.eventb.core.action name="_a2" org.eventb.core.assignment="n ≔ n − 1" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;

fn project() -> Project {
    Project::new(
        "convergence_refinement",
        vec![
            ProjectComponent::from_xml("M0.bum", ABSTRACT_BUM).unwrap(),
            ProjectComponent::from_xml("M1.bum", CONCRETE_BUM).unwrap(),
        ],
    )
}

fn concrete_view() -> ScView {
    let r = build(&project());
    ScView::from_xml(&r.file("M1.bcm").expect("M1.bcm").contents).unwrap()
}

#[test]
fn convergent_refining_ordinary_is_downgraded_and_inaccurate() {
    let v = concrete_view();
    let bad = v.events.get("bad").expect("bad");
    // Declared convergent ("1"), emitted as ordinary ("0").
    assert_eq!(bad.convergence.as_deref(), Some("0"));
    assert!(
        !bad.accurate,
        "bad should be inaccurate (convergence downgraded)"
    );
}

#[test]
fn anticipated_refining_ordinary_is_downgraded_and_inaccurate() {
    let v = concrete_view();
    let badant = v.events.get("badant").expect("badant");
    // Declared anticipated ("2"), emitted as ordinary ("0").
    assert_eq!(badant.convergence.as_deref(), Some("0"));
    assert!(
        !badant.accurate,
        "badant should be inaccurate (convergence downgraded)"
    );
}

#[test]
fn convergent_refining_convergent_stays_accurate() {
    let v = concrete_view();
    let good = v.events.get("good").expect("good");
    assert_eq!(good.convergence.as_deref(), Some("1"));
    assert!(
        good.accurate,
        "good keeps its convergence and stays accurate"
    );
}

#[test]
fn machine_file_stays_accurate_despite_downgraded_events() {
    let r = build(&project());
    let bcm = r.file("M1.bcm").expect("M1.bcm");
    assert!(
        bcm.accurate,
        "per-event convergence downgrades do not taint the file; diagnostics: {:?}",
        r.diagnostics
    );
}
