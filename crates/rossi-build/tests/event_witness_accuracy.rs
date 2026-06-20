//! Witness accuracy: a refining event must witness every disappearing
//! abstract parameter and the after-value of each disappearing variable a
//! non-deterministic abstract action assigns. An unmet (missing or
//! ill-typed) witness marks the event `accurate="false"`; the machine root
//! stays accurate. Mirrors Rodin's `TestAccuracy.testAcc_10` / `_11`, and
//! the boundary cases were cross-checked against a real Rodin build.

use rossi_build::{Project, ProjectComponent, build, sc_view::ScView};

/// Abstract machine whose variable `e` disappears in the refinement and is
/// assigned *non-deterministically*, so its after-value `e'` must be
/// witnessed.
const ABS_NONDET: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_v" org.eventb.core.identifier="e"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="e ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a" org.eventb.core.assignment="e :∈ ℕ" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;

/// Build `M0` (`abs`) + a refinement `M1` whose INITIALISATION body is
/// `init_body`, and return `(INITIALISATION accurate, file accurate)`.
fn init_refinement(abs: &str, init_body: &str) -> (bool, bool) {
    let m1 = format!(
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.variable name="_v" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
{init_body}
</org.eventb.core.event>
</org.eventb.core.machineFile>"#
    );
    let project = Project::new(
        "witacc",
        vec![
            ProjectComponent::from_xml("M0.bum", abs).unwrap(),
            ProjectComponent::from_xml("M1.bum", &m1).unwrap(),
        ],
    );
    let r = build(&project);
    let bcm = r.file("M1.bcm").expect("M1.bcm");
    let v = ScView::from_xml(&bcm.contents).unwrap();
    let init = v.events.get("INITIALISATION").expect("INITIALISATION");
    (init.accurate, bcm.accurate)
}

#[test]
fn missing_global_witness_is_inaccurate() {
    // The disappearing `e` is assigned non-deterministically, so `e'` must
    // be witnessed; the refinement provides none.
    let (init_accurate, file_accurate) = init_refinement(
        ABS_NONDET,
        r#"<org.eventb.core.action name="_a" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>"#,
    );
    assert!(!init_accurate, "missing `e'` witness should be inaccurate");
    assert!(file_accurate, "the machine root stays accurate");
}

#[test]
fn provided_global_witness_is_accurate() {
    let (init_accurate, file_accurate) = init_refinement(
        ABS_NONDET,
        r#"<org.eventb.core.action name="_a" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
<org.eventb.core.witness name="_w" org.eventb.core.label="e'" org.eventb.core.predicate="e' = x'"/>"#,
    );
    assert!(
        init_accurate,
        "a provided `e'` witness satisfies the requirement"
    );
    assert!(file_accurate);
}

#[test]
fn ill_typed_witness_is_inaccurate() {
    // The witness label matches the required `e'`, but its predicate is
    // ill-typed (`e' ∈ ℤ` yet compared to a boolean), so it does not satisfy
    // the requirement.
    let (init_accurate, file_accurate) = init_refinement(
        ABS_NONDET,
        r#"<org.eventb.core.action name="_a" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
<org.eventb.core.witness name="_w" org.eventb.core.label="e'" org.eventb.core.predicate="e' = TRUE"/>"#,
    );
    assert!(
        !init_accurate,
        "an ill-typed `e'` witness should be inaccurate"
    );
    assert!(file_accurate);
}

#[test]
fn deterministic_abstract_assignment_needs_no_witness() {
    // A deterministic abstract `e ≔ 0` pins the after-value, so the
    // refinement needs no witness and stays accurate even without one.
    const ABS_DET: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_v" org.eventb.core.identifier="e"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="e ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a" org.eventb.core.assignment="e ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;
    let (init_accurate, file_accurate) = init_refinement(
        ABS_DET,
        r#"<org.eventb.core.action name="_a" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>"#,
    );
    assert!(
        init_accurate,
        "deterministic abstract assignment requires no witness"
    );
    assert!(file_accurate);
}

#[test]
fn missing_local_parameter_witness_is_inaccurate() {
    // The concrete event refines an abstract `op` whose parameter `p` it
    // renames to `q` and never witnesses, so `p` is an unmet local witness.
    let p0 = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_v" org.eventb.core.identifier="v"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="v ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="v ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_op" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="op">
<org.eventb.core.parameter name="_p" org.eventb.core.identifier="p"/>
<org.eventb.core.guard name="_g" org.eventb.core.label="grd1" org.eventb.core.predicate="p ∈ ℕ"/>
<org.eventb.core.action name="_a" org.eventb.core.assignment="v ≔ p" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;
    let p1 = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="P0"/>
<org.eventb.core.variable name="_v" org.eventb.core.identifier="v"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="v ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="v ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_op" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="op">
<org.eventb.core.refinesEvent name="_re" org.eventb.core.target="op"/>
<org.eventb.core.parameter name="_q" org.eventb.core.identifier="q"/>
<org.eventb.core.guard name="_g" org.eventb.core.label="grd1" org.eventb.core.predicate="q ∈ ℕ"/>
<org.eventb.core.action name="_a" org.eventb.core.assignment="v ≔ q" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;
    let project = Project::new(
        "witlocal",
        vec![
            ProjectComponent::from_xml("P0.bum", p0).unwrap(),
            ProjectComponent::from_xml("P1.bum", p1).unwrap(),
        ],
    );
    let r = build(&project);
    let bcm = r.file("P1.bcm").expect("P1.bcm");
    let v = ScView::from_xml(&bcm.contents).unwrap();
    assert!(
        !v.events.get("op").expect("op").accurate,
        "renamed abstract parameter `p` is an unmet local witness"
    );
    assert!(bcm.accurate, "the machine root stays accurate");
}

#[test]
fn inherited_abstract_parameter_is_a_required_witness() {
    // N1.op *extends* N0.op (inheriting parameter `p`) and adds `q`. N2.op
    // refines N1.op, declares no parameters, and witnesses only `q`. Both
    // `p` (inherited into the abstract through extension) and `q` are
    // required witnesses, so the unwitnessed `p` makes N2.op inaccurate.
    // Cross-checked against a real Rodin build (synthetic `⊤` for `p`).
    let n0 = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_v" org.eventb.core.identifier="v"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="v ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="v ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_op" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="op">
<org.eventb.core.parameter name="_p" org.eventb.core.identifier="p"/>
<org.eventb.core.guard name="_g" org.eventb.core.label="grd1" org.eventb.core.predicate="p ∈ ℕ"/>
<org.eventb.core.action name="_a" org.eventb.core.assignment="v ≔ p" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;
    let n1 = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="N0"/>
<org.eventb.core.variable name="_v" org.eventb.core.identifier="v"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="v ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION">
<org.eventb.core.refinesEvent name="_re0" org.eventb.core.target="INITIALISATION"/>
</org.eventb.core.event>
<org.eventb.core.event name="_op" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="op">
<org.eventb.core.refinesEvent name="_re1" org.eventb.core.target="op"/>
<org.eventb.core.parameter name="_q" org.eventb.core.identifier="q"/>
<org.eventb.core.guard name="_g2" org.eventb.core.label="grd2" org.eventb.core.predicate="q ∈ ℕ"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;
    let n2 = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="N1"/>
<org.eventb.core.variable name="_v" org.eventb.core.identifier="v"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="v ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION">
<org.eventb.core.refinesEvent name="_re0" org.eventb.core.target="INITIALISATION"/>
</org.eventb.core.event>
<org.eventb.core.event name="_op" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="op">
<org.eventb.core.refinesEvent name="_re1" org.eventb.core.target="op"/>
<org.eventb.core.witness name="_wq" org.eventb.core.label="q" org.eventb.core.predicate="q = 1"/>
<org.eventb.core.action name="_a" org.eventb.core.assignment="v ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;
    let project = Project::new(
        "witinherit",
        vec![
            ProjectComponent::from_xml("N0.bum", n0).unwrap(),
            ProjectComponent::from_xml("N1.bum", n1).unwrap(),
            ProjectComponent::from_xml("N2.bum", n2).unwrap(),
        ],
    );
    let r = build(&project);
    let bcm = r.file("N2.bcm").expect("N2.bcm");
    let v = ScView::from_xml(&bcm.contents).unwrap();
    assert!(
        !v.events.get("op").expect("op").accurate,
        "inherited abstract parameter `p` is unwitnessed, so N2.op is inaccurate"
    );
    assert!(bcm.accurate, "the machine root stays accurate");
}
