//! EB022 across an extended-event boundary.
//!
//! Rodin installs inherited guard/action labels in the concrete event's
//! shared label table before checking the concrete clauses. The inherited
//! element wins every collision; the concrete clause is dropped and the
//! checked event becomes inaccurate.

use rossi_build::normalize::{canonical_action, canonical_predicate};
use rossi_build::sc_view::ScView;
use rossi_build::{Diagnostic, Project, ProjectComponent, RuleId, Severity, build};

fn component(filename: &str, body: &str) -> ProjectComponent {
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

fn build_project(components: Vec<ProjectComponent>) -> rossi_build::BuildResult {
    build(&Project::new("labels", components))
}

fn eb022(diags: &[Diagnostic]) -> Vec<&Diagnostic> {
    diags
        .iter()
        .filter(|d| d.rule_id == Some(RuleId::DuplicateLabel))
        .collect()
}

fn event_view(result: &rossi_build::BuildResult, file: &str, event: &str) -> ScView {
    let checked = result
        .file(file)
        .unwrap_or_else(|| panic!("missing {file}"));
    let view = ScView::from_xml(&checked.contents).unwrap();
    assert!(view.events.contains_key(event), "missing {event} in {file}");
    view
}

fn base_machine() -> ProjectComponent {
    component(
        "M0.bum",
        r#"<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_ix" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_init_x" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="init1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_evt" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt">
<org.eventb.core.guard name="_g1" org.eventb.core.label="grd1" org.eventb.core.predicate="x ≥ 0"/>
<org.eventb.core.action name="_a1" org.eventb.core.assignment="x ≔ x + 1" org.eventb.core.label="act1"/>
</org.eventb.core.event>"#,
    )
}

#[test]
fn inherited_guard_label_is_error_and_local_guard_is_dropped_before_checking() {
    let child = component(
        "M1.bum",
        r#"<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_ix" org.eventb.core.label="inv2" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION">
<org.eventb.core.refinesEvent name="_ri" org.eventb.core.target="INITIALISATION"/>
</org.eventb.core.event>
<org.eventb.core.event name="_evt" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="evt">
<org.eventb.core.refinesEvent name="_re" org.eventb.core.target="evt"/>
<org.eventb.core.guard name="_bad" org.eventb.core.label="grd1" org.eventb.core.predicate="missing ≥ 0"/>
<org.eventb.core.guard name="_fresh" org.eventb.core.label="grd2" org.eventb.core.predicate="x ≥ 1"/>
</org.eventb.core.event>"#,
    );
    let result = build_project(vec![base_machine(), child]);

    let conflicts = eb022(&result.diagnostics);
    assert_eq!(conflicts.len(), 1, "{:?}", result.diagnostics);
    assert_eq!(conflicts[0].severity, Severity::Error);
    assert_eq!(conflicts[0].origin, "M1.evt.grd1");
    assert!(conflicts[0].message.contains("inherited guard label"));
    assert_eq!(
        result.diagnostics.len(),
        1,
        "the dropped guard must not cascade into an undeclared-name error: {:?}",
        result.diagnostics
    );

    let view = event_view(&result, "M1.bcm", "evt");
    let evt = &view.events["evt"];
    assert!(!evt.accurate);
    let labels: Vec<&str> = evt.guards.values().map(|g| g.label.as_str()).collect();
    assert_eq!(labels, ["grd1", "grd2"]);
    assert!(
        evt.guards
            .values()
            .any(|g| canonical_predicate(&g.predicate) == "x≥0")
    );
    assert!(
        evt.guards
            .values()
            .any(|g| canonical_predicate(&g.predicate) == "x≥1")
    );
    assert!(
        !evt.guards
            .values()
            .any(|g| canonical_predicate(&g.predicate).contains("missing"))
    );
}

#[test]
fn inherited_guards_and_actions_share_the_namespace_with_local_clauses() {
    let child = component(
        "M1.bum",
        r#"<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_ix" org.eventb.core.label="inv2" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION"/>
<org.eventb.core.event name="_evt" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="evt">
<org.eventb.core.refinesEvent name="_re" org.eventb.core.target="evt"/>
<org.eventb.core.guard name="_guard_vs_action" org.eventb.core.label="act1" org.eventb.core.predicate="x ≥ 2"/>
<org.eventb.core.guard name="_fresh_g" org.eventb.core.label="grd2" org.eventb.core.predicate="x ≥ 1"/>
<org.eventb.core.action name="_action_vs_guard" org.eventb.core.label="grd1" org.eventb.core.assignment="x ≔ x + 2"/>
<org.eventb.core.action name="_fresh_a" org.eventb.core.label="act2" org.eventb.core.assignment="x ≔ x + 3"/>
</org.eventb.core.event>"#,
    );
    let result = build_project(vec![base_machine(), child]);

    let conflicts = eb022(&result.diagnostics);
    assert_eq!(conflicts.len(), 2, "{:?}", result.diagnostics);
    assert!(
        conflicts
            .iter()
            .any(|d| d.origin == "M1.evt.act1" && d.message.contains("inherited action"))
    );
    assert!(
        conflicts
            .iter()
            .any(|d| d.origin == "M1.evt.grd1" && d.message.contains("inherited guard"))
    );

    let view = event_view(&result, "M1.bcm", "evt");
    let evt = &view.events["evt"];
    let guard_labels: Vec<&str> = evt.guards.values().map(|g| g.label.as_str()).collect();
    let action_labels: Vec<&str> = evt.actions.values().map(|a| a.label.as_str()).collect();
    assert_eq!(guard_labels, ["grd1", "grd2"]);
    assert_eq!(action_labels, ["act1", "act2"]);
    assert!(!evt.accurate);
}

#[test]
fn locally_repeated_inherited_label_reports_once_and_drops_every_local_clause() {
    let child = component(
        "M1.bum",
        r#"<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_ix" org.eventb.core.label="inv2" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION"/>
<org.eventb.core.event name="_evt" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="evt">
<org.eventb.core.guard name="_g1" org.eventb.core.label="grd1" org.eventb.core.predicate="x ≥ 5"/>
<org.eventb.core.guard name="_g2" org.eventb.core.label="grd1" org.eventb.core.predicate="x ≥ 6"/>
</org.eventb.core.event>"#,
    );
    let result = build_project(vec![base_machine(), child]);

    // The ordinary component-local duplicate report already covers this
    // label, so the inherited-label pass must not add a second EB022 row.
    let conflicts = eb022(&result.diagnostics);
    assert_eq!(conflicts.len(), 1, "{:?}", result.diagnostics);
    assert_eq!(conflicts[0].origin, "M1.evt.grd1");
    let view = event_view(&result, "M1.bcm", "evt");
    let evt = &view.events["evt"];
    assert_eq!(evt.guards.len(), 1, "only the inherited guard remains");
    assert!(
        evt.guards
            .values()
            .any(|g| canonical_predicate(&g.predicate) == "x≥0")
    );
}

#[test]
fn transitive_inherited_label_conflict_is_detected() {
    let middle = component(
        "M1.bum",
        r#"<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_ix" org.eventb.core.label="inv2" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION"/>
<org.eventb.core.event name="_evt" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="evt">
<org.eventb.core.guard name="_mid" org.eventb.core.label="grd2" org.eventb.core.predicate="x ≥ 1"/>
</org.eventb.core.event>"#,
    );
    let leaf = component(
        "M2.bum",
        r#"<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M1"/>
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_ix" org.eventb.core.label="inv3" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION"/>
<org.eventb.core.event name="_evt" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="evt">
<org.eventb.core.guard name="_root_again" org.eventb.core.label="grd1" org.eventb.core.predicate="x ≥ 99"/>
<org.eventb.core.guard name="_leaf" org.eventb.core.label="grd3" org.eventb.core.predicate="x ≥ 2"/>
</org.eventb.core.event>"#,
    );
    let result = build_project(vec![base_machine(), middle, leaf]);

    let conflicts = eb022(&result.diagnostics);
    assert_eq!(conflicts.len(), 1, "{:?}", result.diagnostics);
    assert_eq!(conflicts[0].origin, "M2.evt.grd1");
    let view = event_view(&result, "M2.bcm", "evt");
    let evt = &view.events["evt"];
    let labels: Vec<&str> = evt.guards.values().map(|g| g.label.as_str()).collect();
    assert_eq!(labels, ["grd1", "grd2", "grd3"]);
}

#[test]
fn extended_initialisation_rejects_inherited_action_label() {
    let root = component(
        "M0.bum",
        r#"<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_ix" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>"#,
    );
    let child = component(
        "M1.bum",
        r#"<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.variable name="_y" org.eventb.core.identifier="y"/>
<org.eventb.core.invariant name="_ix" org.eventb.core.label="inv2" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.invariant name="_iy" org.eventb.core.label="inv3" org.eventb.core.predicate="y ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_collision" org.eventb.core.assignment="y ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>"#,
    );
    let result = build_project(vec![root, child]);

    let conflicts = eb022(&result.diagnostics);
    assert_eq!(conflicts.len(), 1, "{:?}", result.diagnostics);
    assert_eq!(conflicts[0].origin, "M1.INITIALISATION.act1");
    let view = event_view(&result, "M1.bcm", "INITIALISATION");
    let init = &view.events["INITIALISATION"];
    let labels: Vec<&str> = init.actions.values().map(|a| a.label.as_str()).collect();
    assert!(
        labels.contains(&"act1"),
        "inherited action retained: {labels:?}"
    );
    assert!(
        labels.contains(&"GEN"),
        "dropped y action is repaired under a fresh label: {labels:?}"
    );
    assert!(!init.accurate);
}

#[test]
fn plain_refinement_may_reuse_abstract_labels() {
    let child = component(
        "M1.bum",
        r#"<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_ix" org.eventb.core.label="inv2" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_i" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="init1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_evt" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt">
<org.eventb.core.refinesEvent name="_re" org.eventb.core.target="evt"/>
<org.eventb.core.guard name="_g" org.eventb.core.label="grd1" org.eventb.core.predicate="x ≥ 1"/>
<org.eventb.core.action name="_a" org.eventb.core.label="act1" org.eventb.core.assignment="x ≔ x + 2"/>
</org.eventb.core.event>"#,
    );
    let result = build_project(vec![base_machine(), child]);

    assert!(
        eb022(&result.diagnostics).is_empty(),
        "{:?}",
        result.diagnostics
    );
    let view = event_view(&result, "M1.bcm", "evt");
    let evt = &view.events["evt"];
    assert!(evt.accurate, "{:?}", result.diagnostics);
    assert_eq!(evt.guards.len(), 1);
    assert!(
        evt.guards
            .values()
            .any(|g| canonical_predicate(&g.predicate) == "x≥1")
    );
    assert_eq!(evt.actions.len(), 1);
    assert!(
        evt.actions
            .values()
            .any(|a| canonical_action(&a.action) == "x ≔ x+2")
    );
}
