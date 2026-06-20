//! Witness accuracy *and* emission. A refining event must witness every
//! disappearing abstract parameter and the after-value of each disappearing
//! variable a non-deterministic abstract action assigns. The required-name set
//! drives both the `accurate="false"` flag and the emitted `<scWitness>`
//! children: a not-permissible provided witness (not required, or ill-typed)
//! is dropped, and every unmet requirement gets a synthesized `⊤` placeholder
//! sourced on the event. The machine root stays accurate. Mirrors Rodin's
//! `TestAccuracy.testAcc_10` / `_11`, and the boundary cases were cross-checked
//! against a real Rodin build.

use rossi_build::sc_view::{EventRow, ScView, WitnessRow};
use rossi_build::{Project, ProjectComponent, build};

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

/// Abstract machine whose variable `e` disappears but is assigned
/// *deterministically* (`e ≔ 0`), pinning its after-value — so `e'` is not a
/// required witness.
const ABS_DET: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_v" org.eventb.core.identifier="e"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="e ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a" org.eventb.core.assignment="e ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;

/// Build `M0` (`abs`) + a refinement `M1` whose INITIALISATION body is
/// `init_body`, and return M1's parsed `.bcm` view (whose `accurate` field is
/// the machine-root flag).
fn init_refinement_view(abs: &str, init_body: &str) -> ScView {
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
    ScView::from_xml(&bcm.contents).unwrap()
}

/// `(INITIALISATION accurate, file accurate)` convenience over
/// [`init_refinement_view`].
fn init_refinement(abs: &str, init_body: &str) -> (bool, bool) {
    let v = init_refinement_view(abs, init_body);
    let init = v.events.get("INITIALISATION").expect("INITIALISATION");
    (init.accurate, v.accurate)
}

/// The `⊤` predicate, as parsed back from a `.bcm` — the placeholder Rodin
/// (and now rossi) synthesizes for an unmet required witness.
fn top() -> rossi::Predicate {
    rossi::parse_predicate_str("⊤").unwrap()
}

/// The single witness labelled `label` on `ev`. Synthesized witnesses are
/// sourced on the event element and provided ones on their own child, so
/// labels are unambiguous as long as at most one witness is synthesized.
fn witness<'a>(ev: &'a EventRow, label: &str) -> &'a WitnessRow {
    ev.witnesses
        .values()
        .find(|w| w.label == label)
        .unwrap_or_else(|| panic!("no witness labelled {label:?}; got {:?}", ev.witnesses))
}

#[test]
fn missing_global_witness_is_inaccurate() {
    // The disappearing `e` is assigned non-deterministically, so `e'` must
    // be witnessed; the refinement provides none, so a `⊤` is synthesized.
    let v = init_refinement_view(
        ABS_NONDET,
        r#"<org.eventb.core.action name="_a" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>"#,
    );
    let init = v.events.get("INITIALISATION").expect("INITIALISATION");
    assert!(!init.accurate, "missing `e'` witness should be inaccurate");
    assert!(v.accurate, "the machine root stays accurate");
    assert_eq!(init.witnesses.len(), 1, "{:?}", init.witnesses);
    assert_eq!(witness(init, "e'").predicate, top());
}

#[test]
fn provided_global_witness_is_accurate_and_kept() {
    let v = init_refinement_view(
        ABS_NONDET,
        r#"<org.eventb.core.action name="_a" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
<org.eventb.core.witness name="_w" org.eventb.core.label="e'" org.eventb.core.predicate="e' = x'"/>"#,
    );
    let init = v.events.get("INITIALISATION").expect("INITIALISATION");
    assert!(
        init.accurate,
        "a provided `e'` witness satisfies the requirement"
    );
    assert!(v.accurate);
    // The permissible provided witness is kept and emitted verbatim.
    assert_eq!(init.witnesses.len(), 1, "{:?}", init.witnesses);
    assert_eq!(
        witness(init, "e'").predicate,
        rossi::parse_predicate_str("e' = x'").unwrap()
    );
}

#[test]
fn ill_typed_witness_is_dropped_and_replaced_with_top() {
    // The witness label matches the required `e'`, but its predicate is
    // ill-typed (`e'` is an integer, compared to a boolean). It is not
    // permissible: dropped, and a `⊤` placeholder synthesized in its place.
    let v = init_refinement_view(
        ABS_NONDET,
        r#"<org.eventb.core.action name="_a" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
<org.eventb.core.witness name="_w" org.eventb.core.label="e'" org.eventb.core.predicate="e' = TRUE"/>"#,
    );
    let init = v.events.get("INITIALISATION").expect("INITIALISATION");
    assert!(
        !init.accurate,
        "an ill-typed `e'` witness should be inaccurate"
    );
    assert!(v.accurate);
    assert_eq!(init.witnesses.len(), 1, "{:?}", init.witnesses);
    // The provided `e' = TRUE` is gone; only the synthesized `⊤` remains.
    assert_eq!(witness(init, "e'").predicate, top());
}

#[test]
fn deterministic_abstract_assignment_needs_no_witness() {
    // A deterministic abstract `e ≔ 0` pins the after-value, so the
    // refinement needs no witness and stays accurate even without one.
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
fn over_witness_for_non_required_name_is_dropped() {
    // The abstract `e ≔ 0` is deterministic, so `e'` is not required. A
    // witness supplied for it anyway is not-permissible: dropped (not
    // emitted), and the event stays accurate.
    let v = init_refinement_view(
        ABS_DET,
        r#"<org.eventb.core.action name="_a" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
<org.eventb.core.witness name="_w" org.eventb.core.label="e'" org.eventb.core.predicate="e' = x'"/>"#,
    );
    let init = v.events.get("INITIALISATION").expect("INITIALISATION");
    assert!(init.accurate, "no witness is required");
    assert!(v.accurate);
    assert!(
        init.witnesses.is_empty(),
        "the not-required witness must be dropped; got {:?}",
        init.witnesses
    );
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
    let op = v.events.get("op").expect("op");
    assert!(
        !op.accurate,
        "renamed abstract parameter `p` is an unmet local witness"
    );
    assert!(bcm.accurate, "the machine root stays accurate");
    // The unmet `p` is emitted as a synthesized `⊤` placeholder.
    assert_eq!(op.witnesses.len(), 1, "{:?}", op.witnesses);
    assert_eq!(witness(op, "p").predicate, top());
}

#[test]
fn inherited_abstract_parameter_is_a_required_witness() {
    // N1.op *extends* N0.op (inheriting parameter `p`) and adds `q`. N2.op
    // refines N1.op, declares no parameters, and witnesses only `q`. Both
    // `p` (inherited into the abstract through extension) and `q` are
    // required witnesses, so the unwitnessed `p` makes N2.op inaccurate and is
    // emitted as a synthesized `⊤`; the provided `q` is kept.
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
    let op = v.events.get("op").expect("op");
    assert!(
        !op.accurate,
        "inherited abstract parameter `p` is unwitnessed, so N2.op is inaccurate"
    );
    assert!(bcm.accurate, "the machine root stays accurate");
    // `q` provided and kept; `p` unmet and synthesized as `⊤`.
    assert_eq!(op.witnesses.len(), 2, "{:?}", op.witnesses);
    assert_eq!(
        witness(op, "q").predicate,
        rossi::parse_predicate_str("q = 1").unwrap()
    );
    assert_eq!(witness(op, "p").predicate, top());
}

#[test]
fn two_unmet_parameter_witnesses_each_get_a_top_placeholder() {
    // Abstract `op` has two parameters `p`, `r`; the refinement drops both and
    // witnesses neither, so each is synthesized as a `⊤` placeholder. Both are
    // sourced on the event element, so the keyed view collapses them — count
    // the raw `<scWitness>` elements instead.
    let m0 = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_v" org.eventb.core.identifier="v"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="v ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="v ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_op" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="op">
<org.eventb.core.parameter name="_p" org.eventb.core.identifier="p"/>
<org.eventb.core.parameter name="_r" org.eventb.core.identifier="r"/>
<org.eventb.core.guard name="_g1" org.eventb.core.label="grd1" org.eventb.core.predicate="p ∈ ℕ"/>
<org.eventb.core.guard name="_g2" org.eventb.core.label="grd2" org.eventb.core.predicate="r ∈ ℕ"/>
<org.eventb.core.action name="_a" org.eventb.core.assignment="v ≔ p" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;
    let m1 = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.variable name="_v" org.eventb.core.identifier="v"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="v ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="v ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_op" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="op">
<org.eventb.core.refinesEvent name="_re" org.eventb.core.target="op"/>
<org.eventb.core.action name="_a" org.eventb.core.assignment="v ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;
    let project = Project::new(
        "wittwo",
        vec![
            ProjectComponent::from_xml("M0.bum", m0).unwrap(),
            ProjectComponent::from_xml("M1.bum", m1).unwrap(),
        ],
    );
    let r = build(&project);
    let bcm = r.file("M1.bcm").expect("M1.bcm");
    let raw = &bcm.contents;
    assert_eq!(
        raw.matches("<org.eventb.core.scWitness").count(),
        2,
        "expected two synthesized witnesses; got:\n{raw}"
    );
    assert_eq!(
        raw.matches(r#"org.eventb.core.predicate="⊤""#).count(),
        2,
        "both synthesized witnesses should be ⊤"
    );
    assert!(raw.contains(r#"org.eventb.core.label="p""#));
    assert!(raw.contains(r#"org.eventb.core.label="r""#));
    let v = ScView::from_xml(raw).unwrap();
    assert!(!v.events.get("op").expect("op").accurate);
    assert!(v.accurate, "the machine root stays accurate");
}

#[test]
fn non_refining_event_drops_a_stray_witness() {
    // A new (non-refining) event owes no witnesses, so a witness clause on it
    // is not-permissible: dropped, and the event stays accurate.
    let m = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_v" org.eventb.core.identifier="v"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="v ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="v ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_op" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="op">
<org.eventb.core.action name="_a" org.eventb.core.assignment="v ≔ 1" org.eventb.core.label="act1"/>
<org.eventb.core.witness name="_w" org.eventb.core.label="w1" org.eventb.core.predicate="v' = 1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;
    let project = Project::new(
        "witnew",
        vec![ProjectComponent::from_xml("M.bum", m).unwrap()],
    );
    let r = build(&project);
    let bcm = r.file("M.bcm").expect("M.bcm");
    let v = ScView::from_xml(&bcm.contents).unwrap();
    let op = v.events.get("op").expect("op");
    assert!(op.accurate, "a new event owes no witnesses");
    assert!(
        op.witnesses.is_empty(),
        "the stray witness must be dropped; got {:?}",
        op.witnesses
    );
    assert!(v.accurate);
}
