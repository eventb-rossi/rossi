//! Group R: when a child machine inherits a parent's variable but
//! doesn't redeclare it, the variable vanishes to abstract-only
//! (`abstract=true concrete=false`). Concrete events that reference
//! such a variable lose those clauses and are marked
//! `accurate=false`. An extended INITIALISATION whose parent INIT
//! assigns to any vanished variable is omitted entirely.
//!
//! Rodin parity — verified by rodin-docker probes (Group R plan) and
//! against `tutorial_fx3-tut2/ITERATION.bcm`.

use rossi_build::{Project, ProjectComponent, build, sc_view::ScView};

const PARENT_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.variable name="_y" org.eventb.core.identifier="y"/>
<org.eventb.core.invariant name="_ix" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.invariant name="_iy" org.eventb.core.label="inv2" org.eventb.core.predicate="y ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_ax" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
<org.eventb.core.action name="_ay" org.eventb.core.assignment="y ≔ 1" org.eventb.core.label="act2"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;

// Child redeclares only `x`; inherits `y` without witness; declares new `w`.
// Has an own event `stepone` whose guard references `y` (now abstract-only)
// and action writes to `y` too. INITIALISATION extended=true; own action
// writes to `w`.
const CHILD_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="P"/>
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.variable name="_w" org.eventb.core.identifier="w"/>
<org.eventb.core.invariant name="_iw" org.eventb.core.label="inv3" org.eventb.core.predicate="w ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_aw" org.eventb.core.assignment="w ≔ 0" org.eventb.core.label="act3"/>
</org.eventb.core.event>
<org.eventb.core.event name="_step" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="stepone">
<org.eventb.core.guard name="_g_ok" org.eventb.core.label="grd_ok" org.eventb.core.predicate="x ≠ 0"/>
<org.eventb.core.guard name="_g_bad" org.eventb.core.label="grd_bad" org.eventb.core.predicate="y &gt; 0"/>
<org.eventb.core.action name="_a_ok" org.eventb.core.assignment="x ≔ x + 1" org.eventb.core.label="act_ok"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;

fn project() -> Project {
    Project::new(
        "r",
        vec![
            ProjectComponent::from_xml("P.bum", PARENT_BUM).unwrap(),
            ProjectComponent::from_xml("C.bum", CHILD_BUM).unwrap(),
        ],
    )
}

#[test]
fn inherited_only_variable_is_concrete_false() {
    let r = build(&project());
    let bcm = &r.file("C.bcm").expect("C.bcm").contents;
    // x is inherited AND redeclared → abstract=true concrete=true.
    assert!(
        bcm.contains(r#"name="x" org.eventb.core.abstract="true" org.eventb.core.concrete="true""#),
        "x should be abstract=true concrete=true:\n{bcm}"
    );
    // w is new → abstract=false concrete=true.
    assert!(
        bcm.contains(
            r#"name="w" org.eventb.core.abstract="false" org.eventb.core.concrete="true""#
        ),
        "w should be abstract=false concrete=true:\n{bcm}"
    );
    // y is inherited, NOT redeclared → abstract=true concrete=false.
    assert!(
        bcm.contains(
            r#"name="y" org.eventb.core.abstract="true" org.eventb.core.concrete="false""#
        ),
        "y should be abstract=true concrete=false:\n{bcm}"
    );
}

#[test]
fn initialisation_omitted_when_parent_assigns_abstract_only_var() {
    let r = build(&project());
    let v = ScView::from_xml(&r.file("C.bcm").expect("C.bcm").contents).unwrap();
    assert!(
        !v.events.contains_key("INITIALISATION"),
        "INITIALISATION should be omitted (parent INIT writes to abstract-only `y`); \
         events present: {:?}",
        v.events.keys().collect::<Vec<_>>()
    );
}

#[test]
fn event_dropping_abstract_only_guard_marks_inaccurate() {
    let r = build(&project());
    let v = ScView::from_xml(&r.file("C.bcm").expect("C.bcm").contents).unwrap();
    let step = v.events.get("stepone").expect("stepone event present");
    assert!(
        !step.accurate,
        "stepone should be inaccurate (guard `y > 0` references abstract-only `y`)"
    );
    // Only the `x ≠ 0` guard survives; `y > 0` is dropped.
    assert_eq!(
        step.guards.len(),
        1,
        "expected exactly one guard (grd_ok); got {:#?}",
        step.guards
    );
    // The action `x ≔ x + 1` doesn't touch abstract-only vars, so it
    // survives.
    assert_eq!(step.actions.len(), 1);
}
