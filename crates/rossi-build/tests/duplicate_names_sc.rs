//! EB021/EB022 in the static checker: duplicate identifiers / labels are
//! reported as errors and filtered out of the emitted `.bcc`/`.bcm` with
//! Rodin's drop semantics — duplicated identifiers and event labels drop
//! every occurrence; duplicated formula labels (invariants, axioms, guards,
//! actions) keep the first occurrence and drop the rest, marking the
//! container inaccurate. Identifier and event-label drops don't flip file
//! accuracy directly: dependent formulas fail their own checks and cascade.

use rossi_build::sc_view::ScView;
use rossi_build::{Diagnostic, Project, ProjectComponent, RuleId, Severity, build};

fn machine(filename: &str, body: &str) -> ProjectComponent {
    ProjectComponent::from_xml(
        filename,
        &format!(
            r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
{body}
</org.eventb.core.machineFile>"#
        ),
    )
    .unwrap()
}

fn context(filename: &str, body: &str) -> ProjectComponent {
    ProjectComponent::from_xml(
        filename,
        &format!(
            r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
{body}
</org.eventb.core.contextFile>"#
        ),
    )
    .unwrap()
}

fn diags_with_rule(diags: &[Diagnostic], rule: RuleId) -> Vec<&Diagnostic> {
    diags.iter().filter(|d| d.rule_id == Some(rule)).collect()
}

/// Build a single-component project (the common shape here).
fn build_one(component: ProjectComponent) -> rossi_build::BuildResult {
    build(&Project::new("p", vec![component]))
}

#[test]
fn duplicate_variable_is_dropped_and_reported() {
    let m = machine(
        "M.bum",
        r#"<org.eventb.core.variable name="_v1" org.eventb.core.identifier="x"/>
<org.eventb.core.variable name="_v2" org.eventb.core.identifier="x"/>
<org.eventb.core.variable name="_v3" org.eventb.core.identifier="y"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.invariant name="_i2" org.eventb.core.label="inv2" org.eventb.core.predicate="y ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
<org.eventb.core.action name="_a2" org.eventb.core.assignment="y ≔ 0" org.eventb.core.label="act2"/>
</org.eventb.core.event>"#,
    );
    let r = build_one(m);

    assert!(!r.is_ok(), "EB021 is an error: {:?}", r.diagnostics);
    let eb021 = diags_with_rule(&r.diagnostics, RuleId::DuplicateIdentifier);
    assert_eq!(eb021.len(), 1, "{:?}", r.diagnostics);
    assert_eq!(eb021[0].origin, "M.x");
    assert_eq!(eb021[0].severity, Severity::Error);

    // Both `x` declarations are gone; the typing invariant that references
    // the dropped name cascades (EB018) and flips the file inaccurate.
    let bcm = r.file("M.bcm").expect("M.bcm is still emitted");
    assert!(
        !bcm.accurate,
        "typing-invariant cascade: {:?}",
        r.diagnostics
    );
    let v = ScView::from_xml(&bcm.contents).unwrap();
    assert!(!v.variables.contains_key("x"), "{:?}", v.variables.keys());
    assert!(v.variables.contains_key("y"));
    assert!(!v.invariants.contains_key("inv1"));
    assert!(
        !diags_with_rule(&r.diagnostics, RuleId::UndeclaredIdentifier).is_empty(),
        "the invariant referencing the dropped variable reports EB018: {:?}",
        r.diagnostics
    );
}

#[test]
fn duplicate_invariant_label_keeps_first_occurrence() {
    // The 2nd `inv1` references an undeclared `z`: if the checker wrongly
    // kept it (or checked it), an EB018 would surface. A properly dropped
    // clause is never checked, so EB022 must be the only error.
    let m = machine(
        "M.bum",
        r#"<org.eventb.core.variable name="_v1" org.eventb.core.identifier="x"/>
<org.eventb.core.variable name="_v2" org.eventb.core.identifier="y"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.invariant name="_i2" org.eventb.core.label="inv1" org.eventb.core.predicate="z ∈ ℤ"/>
<org.eventb.core.invariant name="_i3" org.eventb.core.label="inv2" org.eventb.core.predicate="y ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
<org.eventb.core.action name="_a2" org.eventb.core.assignment="y ≔ 0" org.eventb.core.label="act2"/>
</org.eventb.core.event>"#,
    );
    let r = build_one(m);

    let eb022 = diags_with_rule(&r.diagnostics, RuleId::DuplicateLabel);
    assert_eq!(eb022.len(), 1, "{:?}", r.diagnostics);
    assert_eq!(eb022[0].origin, "M.inv1");
    assert!(
        diags_with_rule(&r.diagnostics, RuleId::UndeclaredIdentifier).is_empty(),
        "the dropped 2nd occurrence must never be checked: {:?}",
        r.diagnostics
    );

    let bcm = r.file("M.bcm").unwrap();
    assert!(
        !bcm.accurate,
        "a dropped invariant marks the file inaccurate"
    );
    let v = ScView::from_xml(&bcm.contents).unwrap();
    let mut labels: Vec<&str> = v.invariants.values().map(|r| r.label.as_str()).collect();
    labels.sort_unstable();
    assert_eq!(
        labels,
        ["inv1", "inv2"],
        "first inv1 kept, duplicate dropped"
    );
}

#[test]
fn duplicate_event_label_drops_every_occurrence() {
    let m = machine(
        "M.bum",
        r#"<org.eventb.core.variable name="_v1" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_e1" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt">
<org.eventb.core.action name="_a2" org.eventb.core.assignment="x ≔ 1" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_e2" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt">
<org.eventb.core.action name="_a3" org.eventb.core.assignment="x ≔ 2" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_e3" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="other">
<org.eventb.core.action name="_a4" org.eventb.core.assignment="x ≔ 3" org.eventb.core.label="act1"/>
</org.eventb.core.event>"#,
    );
    let r = build_one(m);

    assert!(!r.is_ok());
    let eb022 = diags_with_rule(&r.diagnostics, RuleId::DuplicateLabel);
    assert_eq!(eb022.len(), 1, "{:?}", r.diagnostics);
    assert_eq!(eb022[0].origin, "M.evt");

    // Both `evt`s vanish; the machine root stays accurate (Rodin's
    // event-label conflict does not touch machine accuracy).
    let bcm = r.file("M.bcm").unwrap();
    assert!(bcm.accurate, "{:?}", r.diagnostics);
    let v = ScView::from_xml(&bcm.contents).unwrap();
    assert!(!v.events.contains_key("evt"), "{:?}", v.events.keys());
    assert!(v.events.contains_key("other"));
    assert!(v.events.contains_key("INITIALISATION"));
}

#[test]
fn descendant_refining_dropped_duplicate_event_hits_missing_target_path() {
    let m0 = machine(
        "M0.bum",
        r#"<org.eventb.core.variable name="_v1" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_e1" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt">
<org.eventb.core.action name="_a2" org.eventb.core.assignment="x ≔ 1" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_e2" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt">
<org.eventb.core.action name="_a3" org.eventb.core.assignment="x ≔ 2" org.eventb.core.label="act1"/>
</org.eventb.core.event>"#,
    );
    let m1 = machine(
        "M1.bum",
        r#"<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.variable name="_v1" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION"></org.eventb.core.event>
<org.eventb.core.event name="_e1" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="c">
<org.eventb.core.refinesEvent name="_re" org.eventb.core.target="evt"/>
<org.eventb.core.action name="_a2" org.eventb.core.assignment="x ≔ x + 1" org.eventb.core.label="act1"/>
</org.eventb.core.event>"#,
    );
    let r = build(&Project::new("p", vec![m0, m1]));

    // The duplicated `evt` never enters M0's event map, so M1's explicit
    // refines finds no target and `c` is dropped by the existing path.
    let dropped: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.origin == "M1.c" && d.message.contains("refines target 'evt' not found"))
        .collect();
    assert_eq!(dropped.len(), 1, "{:?}", r.diagnostics);
    let v = ScView::from_xml(&r.file("M1.bcm").unwrap().contents).unwrap();
    assert!(!v.events.contains_key("c"), "{:?}", v.events.keys());
}

#[test]
fn duplicate_parameter_is_dropped_and_guard_cascades() {
    let m = machine(
        "M.bum",
        r#"<org.eventb.core.variable name="_v1" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_e1" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt">
<org.eventb.core.parameter name="_p1" org.eventb.core.identifier="p"/>
<org.eventb.core.parameter name="_p2" org.eventb.core.identifier="p"/>
<org.eventb.core.parameter name="_p3" org.eventb.core.identifier="q"/>
<org.eventb.core.guard name="_g1" org.eventb.core.label="grd1" org.eventb.core.predicate="p &gt; 0"/>
<org.eventb.core.guard name="_g2" org.eventb.core.label="grd2" org.eventb.core.predicate="q ∈ ℤ"/>
<org.eventb.core.action name="_a2" org.eventb.core.assignment="x ≔ q" org.eventb.core.label="act1"/>
</org.eventb.core.event>"#,
    );
    let r = build_one(m);

    let eb021 = diags_with_rule(&r.diagnostics, RuleId::DuplicateIdentifier);
    assert_eq!(eb021.len(), 1, "{:?}", r.diagnostics);
    assert_eq!(eb021[0].origin, "M.evt.p");

    let bcm = r.file("M.bcm").unwrap();
    // Parameter drops don't touch file accuracy; the event pays via its
    // guard cascade (grd1 references the dropped `p` and is ill-typed).
    assert!(bcm.accurate, "{:?}", r.diagnostics);
    let v = ScView::from_xml(&bcm.contents).unwrap();
    let evt = &v.events["evt"];
    assert_eq!(evt.parameters.keys().collect::<Vec<_>>(), ["q"]);
    assert!(!evt.accurate);
    let guard_labels: Vec<&str> = evt.guards.values().map(|g| g.label.as_str()).collect();
    assert_eq!(guard_labels, ["grd2"], "grd1 cascaded away");
}

#[test]
fn duplicate_guard_action_label_keeps_the_guard() {
    // Guards and actions share one label namespace; the guard comes first,
    // so it is kept and the action is dropped. The 2nd guard of `evt2`
    // references an undeclared `z`: a properly dropped clause is never
    // checked, so no other error may surface for it.
    let m = machine(
        "M.bum",
        r#"<org.eventb.core.variable name="_v1" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_e1" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt1">
<org.eventb.core.guard name="_g1" org.eventb.core.label="lbl" org.eventb.core.predicate="x &gt; 0"/>
<org.eventb.core.action name="_a2" org.eventb.core.assignment="x ≔ 1" org.eventb.core.label="lbl"/>
</org.eventb.core.event>
<org.eventb.core.event name="_e2" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt2">
<org.eventb.core.guard name="_g2" org.eventb.core.label="grd1" org.eventb.core.predicate="x &gt; 0"/>
<org.eventb.core.guard name="_g3" org.eventb.core.label="grd1" org.eventb.core.predicate="z &gt; 0"/>
<org.eventb.core.action name="_a3" org.eventb.core.assignment="x ≔ 2" org.eventb.core.label="act1"/>
</org.eventb.core.event>"#,
    );
    let r = build_one(m);

    let eb022 = diags_with_rule(&r.diagnostics, RuleId::DuplicateLabel);
    assert_eq!(eb022.len(), 2, "{:?}", r.diagnostics);
    assert!(eb022.iter().any(|d| d.origin == "M.evt1.lbl"));
    assert!(eb022.iter().any(|d| d.origin == "M.evt2.grd1"));
    assert_eq!(
        r.diagnostics.len(),
        2,
        "the dropped duplicates must not cascade into other errors: {:?}",
        r.diagnostics
    );

    let bcm = r.file("M.bcm").unwrap();
    assert!(bcm.accurate);
    let v = ScView::from_xml(&bcm.contents).unwrap();
    let evt1 = &v.events["evt1"];
    let guard_labels: Vec<&str> = evt1.guards.values().map(|g| g.label.as_str()).collect();
    assert_eq!(guard_labels, ["lbl"], "the guard wins the label");
    assert!(
        !evt1.actions.values().any(|a| a.label == "lbl"),
        "the action is dropped"
    );
    assert!(!evt1.accurate);
    let evt2 = &v.events["evt2"];
    let guard_labels: Vec<&str> = evt2.guards.values().map(|g| g.label.as_str()).collect();
    assert_eq!(guard_labels, ["grd1"], "first grd1 kept, duplicate dropped");
    assert!(!evt2.accurate);
}

#[test]
fn duplicate_witness_label_marks_event_inaccurate() {
    let m = machine(
        "M.bum",
        r#"<org.eventb.core.variable name="_v1" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_e1" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt">
<org.eventb.core.witness name="_w1" org.eventb.core.label="w" org.eventb.core.predicate="x = 0"/>
<org.eventb.core.witness name="_w2" org.eventb.core.label="w" org.eventb.core.predicate="x = 1"/>
<org.eventb.core.action name="_a2" org.eventb.core.assignment="x ≔ 1" org.eventb.core.label="act1"/>
</org.eventb.core.event>"#,
    );
    let r = build_one(m);

    let eb022 = diags_with_rule(&r.diagnostics, RuleId::DuplicateLabel);
    assert_eq!(eb022.len(), 1, "{:?}", r.diagnostics);
    assert_eq!(eb022[0].origin, "M.evt.w");
    assert!(eb022[0].message.contains("witness label"));

    let v = ScView::from_xml(&r.file("M.bcm").unwrap().contents).unwrap();
    assert!(!v.events["evt"].accurate);
}

#[test]
fn initialisation_duplicate_action_label_keeps_first_and_repairs() {
    let m = machine(
        "M.bum",
        r#"<org.eventb.core.variable name="_v1" org.eventb.core.identifier="x"/>
<org.eventb.core.variable name="_v2" org.eventb.core.identifier="y"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.invariant name="_i2" org.eventb.core.label="inv2" org.eventb.core.predicate="y ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
<org.eventb.core.action name="_a2" org.eventb.core.assignment="y ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>"#,
    );
    let r = build_one(m);

    let eb022 = diags_with_rule(&r.diagnostics, RuleId::DuplicateLabel);
    assert_eq!(eb022.len(), 1, "{:?}", r.diagnostics);
    assert_eq!(eb022[0].origin, "M.INITIALISATION.act1");

    // The dropped 2nd action left `y` uninitialised, so the INIT repair
    // kicks in: a synthesized default assignment plus event inaccuracy.
    let v = ScView::from_xml(&r.file("M.bcm").unwrap().contents).unwrap();
    let init = &v.events["INITIALISATION"];
    let action_labels: Vec<&str> = init.actions.values().map(|a| a.label.as_str()).collect();
    assert!(action_labels.contains(&"act1"), "{action_labels:?}");
    assert!(
        action_labels.contains(&"GEN"),
        "the dropped 2nd action leaves y to the INIT repair: {action_labels:?}"
    );
    assert!(!init.accurate);
}

#[test]
fn duplicate_constant_is_dropped_and_axiom_cascades() {
    let c = context(
        "C.buc",
        r#"<org.eventb.core.constant name="_c1" org.eventb.core.identifier="k"/>
<org.eventb.core.constant name="_c2" org.eventb.core.identifier="k"/>
<org.eventb.core.axiom name="_a1" org.eventb.core.label="axm1" org.eventb.core.predicate="k ∈ ℤ"/>"#,
    );
    let r = build_one(c);

    assert!(!r.is_ok());
    let eb021 = diags_with_rule(&r.diagnostics, RuleId::DuplicateIdentifier);
    assert_eq!(eb021.len(), 1, "{:?}", r.diagnostics);
    assert_eq!(eb021[0].origin, "C.k");

    let bcc = r.file("C.bcc").expect("C.bcc is still emitted");
    assert!(!bcc.accurate, "axiom cascade: {:?}", r.diagnostics);
    let v = ScView::from_xml(&bcc.contents).unwrap();
    assert!(!v.constants.contains_key("k"), "{:?}", v.constants.keys());
    assert!(v.axioms.is_empty(), "typing axiom cascaded away");
}

#[test]
fn carrier_set_and_constant_collision_drops_both_without_accuracy_flip() {
    let c = context(
        "C.buc",
        r#"<org.eventb.core.carrierSet name="_s1" org.eventb.core.identifier="S"/>
<org.eventb.core.constant name="_c1" org.eventb.core.identifier="S"/>"#,
    );
    let r = build_one(c);

    assert!(!r.is_ok());
    let eb021 = diags_with_rule(&r.diagnostics, RuleId::DuplicateIdentifier);
    assert_eq!(eb021.len(), 1, "{:?}", r.diagnostics);
    assert_eq!(eb021[0].origin, "C.S");

    // Nothing references the dropped name, so no formula drop occurs and
    // the file stays accurate — identifier conflicts alone don't flip it.
    let bcc = r.file("C.bcc").unwrap();
    assert!(bcc.accurate, "{:?}", r.diagnostics);
    let v = ScView::from_xml(&bcc.contents).unwrap();
    assert!(!v.carrier_sets.contains_key("S"));
    assert!(!v.constants.contains_key("S"));
}

#[test]
fn duplicate_axiom_label_keeps_first_occurrence() {
    let c = context(
        "C.buc",
        r#"<org.eventb.core.constant name="_c1" org.eventb.core.identifier="k"/>
<org.eventb.core.axiom name="_a1" org.eventb.core.label="axm1" org.eventb.core.predicate="k ∈ ℤ"/>
<org.eventb.core.axiom name="_a2" org.eventb.core.label="axm1" org.eventb.core.predicate="z ∈ ℤ"/>"#,
    );
    let r = build_one(c);

    let eb022 = diags_with_rule(&r.diagnostics, RuleId::DuplicateLabel);
    assert_eq!(eb022.len(), 1, "{:?}", r.diagnostics);
    assert_eq!(eb022[0].origin, "C.axm1");
    assert!(
        diags_with_rule(&r.diagnostics, RuleId::UndeclaredIdentifier).is_empty(),
        "the dropped 2nd occurrence must never be checked: {:?}",
        r.diagnostics
    );

    let bcc = r.file("C.bcc").unwrap();
    assert!(
        !bcc.accurate,
        "a dropped axiom marks the context inaccurate"
    );
    let v = ScView::from_xml(&bcc.contents).unwrap();
    let labels: Vec<&str> = v.axioms.values().map(|a| a.label.as_str()).collect();
    assert_eq!(labels, ["axm1"], "first axm1 kept, duplicate dropped");
    assert!(v.constants.contains_key("k"), "first axm1 still types k");
}

#[test]
fn constant_typed_only_by_dropped_duplicate_axiom_reports_eb006() {
    let c = context(
        "C.buc",
        r#"<org.eventb.core.constant name="_c1" org.eventb.core.identifier="k"/>
<org.eventb.core.axiom name="_a1" org.eventb.core.label="axm1" org.eventb.core.predicate="1 = 1"/>
<org.eventb.core.axiom name="_a2" org.eventb.core.label="axm1" org.eventb.core.predicate="k ∈ ℤ"/>"#,
    );
    let r = build_one(c);

    // The typing axiom is the dropped 2nd occurrence, so `k` ends up
    // untyped: the existing EB006 error fires and `k` is not emitted.
    let eb006 = diags_with_rule(&r.diagnostics, RuleId::TypeError);
    assert_eq!(eb006.len(), 1, "{:?}", r.diagnostics);
    assert_eq!(eb006[0].origin, "C.k");
    assert_eq!(eb006[0].severity, Severity::Error);
    let v = ScView::from_xml(&r.file("C.bcc").unwrap().contents).unwrap();
    assert!(!v.constants.contains_key("k"));
}

#[test]
fn duplicates_still_reported_when_refines_cycle_stops_the_sc() {
    // A dependency cycle is a report-and-stop, but the duplicate variable
    // is a component-local fact the user must still see in the same pass
    // (the lint pass used to report it regardless of the SC outcome).
    let components = ProjectComponent::from_eventb(
        "m.eventb",
        "MACHINE M\nREFINES M\nVARIABLES\n    x x\nEND\n",
    )
    .unwrap();
    let r = build(&Project::new("p", components));
    assert_eq!(
        diags_with_rule(&r.diagnostics, RuleId::CircularRefines).len(),
        1,
        "{:?}",
        r.diagnostics
    );
    assert_eq!(
        diags_with_rule(&r.diagnostics, RuleId::DuplicateIdentifier).len(),
        1,
        "{:?}",
        r.diagnostics
    );
}

#[test]
fn duplicates_still_reported_when_extends_cycle_stops_the_sc() {
    let components = ProjectComponent::from_eventb(
        "c.eventb",
        "CONTEXT C\nEXTENDS C\nCONSTANTS\n    k k\nEND\n",
    )
    .unwrap();
    let r = build(&Project::new("p", components));
    assert_eq!(
        diags_with_rule(&r.diagnostics, RuleId::CircularExtends).len(),
        1,
        "{:?}",
        r.diagnostics
    );
    assert_eq!(
        diags_with_rule(&r.diagnostics, RuleId::DuplicateIdentifier).len(),
        1,
        "{:?}",
        r.diagnostics
    );
}

#[test]
fn duplicates_still_reported_on_duplicate_component_names() {
    let mut components =
        ProjectComponent::from_eventb("a.eventb", "MACHINE M\nVARIABLES\n    x x\nEND\n").unwrap();
    components.extend(
        ProjectComponent::from_eventb("b.eventb", "MACHINE M\nVARIABLES\n    x x\nEND\n").unwrap(),
    );
    let r = build(&Project::new("p", components));
    assert_eq!(
        diags_with_rule(&r.diagnostics, RuleId::DuplicateComponent).len(),
        1,
        "{:?}",
        r.diagnostics
    );
    // One EB021 per colliding component, exactly as the per-component lint
    // pass used to report before the check moved into the SC.
    assert_eq!(
        diags_with_rule(&r.diagnostics, RuleId::DuplicateIdentifier).len(),
        2,
        "{:?}",
        r.diagnostics
    );
}

#[test]
fn dropped_duplicate_event_still_reports_inner_duplicates() {
    // Both `evt`s are dropped for the label clash, but the duplicated
    // parameter inside the first one is still an error — fixing the label
    // clash must not surface a brand-new EB021, and the SC path must agree
    // with the loose-text/LSP shared core, which walks every event.
    let m = machine(
        "M.bum",
        r#"<org.eventb.core.variable name="_v1" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_e1" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt">
<org.eventb.core.parameter name="_p1" org.eventb.core.identifier="p"/>
<org.eventb.core.parameter name="_p2" org.eventb.core.identifier="p"/>
<org.eventb.core.guard name="_g1" org.eventb.core.label="grd1" org.eventb.core.predicate="p ∈ ℤ"/>
</org.eventb.core.event>
<org.eventb.core.event name="_e2" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt">
<org.eventb.core.action name="_a2" org.eventb.core.assignment="x ≔ 1" org.eventb.core.label="act1"/>
</org.eventb.core.event>"#,
    );
    let r = build_one(m);

    let eb022 = diags_with_rule(&r.diagnostics, RuleId::DuplicateLabel);
    assert_eq!(eb022.len(), 1, "{:?}", r.diagnostics);
    assert_eq!(eb022[0].origin, "M.evt");
    let eb021 = diags_with_rule(&r.diagnostics, RuleId::DuplicateIdentifier);
    assert_eq!(eb021.len(), 1, "{:?}", r.diagnostics);
    assert_eq!(eb021[0].origin, "M.evt.p");

    let v = ScView::from_xml(&r.file("M.bcm").unwrap().contents).unwrap();
    assert!(!v.events.contains_key("evt"), "{:?}", v.events.keys());
}

#[test]
fn omitted_extended_initialisation_still_reports_inner_duplicates() {
    // M1's extended INITIALISATION inherits M0's `v ≔ 0`, but `v` has
    // disappeared in M1, so the event is omitted from the .bcm entirely.
    // Its own two `act1` actions are still a source-level duplicate label
    // (EB022) that the build must report — the diagnostic comes from the
    // up-front pass, not the (skipped) event build.
    let m0 = machine(
        "M0.bum",
        r#"<org.eventb.core.variable name="_v1" org.eventb.core.identifier="v"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="v ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="v ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>"#,
    );
    let m1 = machine(
        "M1.bum",
        r#"<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.variable name="_w1" org.eventb.core.identifier="w"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="w ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="w ≔ 0" org.eventb.core.label="act1"/>
<org.eventb.core.action name="_a2" org.eventb.core.assignment="w ≔ 1" org.eventb.core.label="act1"/>
</org.eventb.core.event>"#,
    );
    let r = build(&Project::new("p", vec![m0, m1]));

    let eb022: Vec<_> = diags_with_rule(&r.diagnostics, RuleId::DuplicateLabel)
        .into_iter()
        .filter(|d| d.origin == "M1.INITIALISATION.act1")
        .collect();
    assert_eq!(eb022.len(), 1, "{:?}", r.diagnostics);
    // The event really is omitted from the emitted machine.
    let v = ScView::from_xml(&r.file("M1.bcm").unwrap().contents).unwrap();
    assert!(
        !v.events.contains_key("INITIALISATION"),
        "extended INIT should be omitted: {:?}",
        v.events.keys()
    );
}

#[test]
fn build_reports_duplicates_once_and_lint_not_at_all() {
    // validate folds `build()` diagnostics *and* `lint::run` — EB021/EB022
    // must come from exactly one of them (the SC) or every duplicate would
    // be double-reported.
    let m = machine(
        "M.bum",
        r#"<org.eventb.core.variable name="_v1" org.eventb.core.identifier="x"/>
<org.eventb.core.variable name="_v2" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>"#,
    );
    let project = Project::new("p", vec![m]);
    let r = build(&project);
    assert_eq!(
        diags_with_rule(&r.diagnostics, RuleId::DuplicateIdentifier).len(),
        1,
        "{:?}",
        r.diagnostics
    );
    let lint = rossi_build::lint::run(&project);
    assert!(
        diags_with_rule(&lint, RuleId::DuplicateIdentifier).is_empty()
            && diags_with_rule(&lint, RuleId::DuplicateLabel).is_empty(),
        "lint must no longer report duplicates: {lint:?}"
    );
}
