//! EB032 in the static checker: an axiom, invariant, guard, action or
//! witness that a Rodin file stores without a label, or with a blank one.
//! Rodin's `LabeledElementModule.fetchLabel` reports it, and
//! `LabeledFormulaModule.checkAndType` then never type-checks the formula
//! and drops it, marking its container inaccurate: the context for an axiom,
//! the machine for an invariant, the event for the rest.

use rossi_build::sc_view::ScView;
use rossi_build::{BuildResult, Diagnostic, Project, ProjectComponent, RuleId, Severity, build};

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

fn build_all(components: Vec<ProjectComponent>) -> BuildResult {
    build(&Project::new("p", components))
}

fn errors(result: &BuildResult) -> Vec<&Diagnostic> {
    result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect()
}

fn with_rule(result: &BuildResult, rule: RuleId) -> Vec<&Diagnostic> {
    result
        .diagnostics
        .iter()
        .filter(|d| d.rule_id == Some(rule))
        .collect()
}

fn view(result: &BuildResult, file: &str) -> ScView {
    ScView::from_xml(&result.file(file).unwrap().contents).unwrap()
}

/// Labels of a checked map's rows, sorted.
fn labels<'a>(rows: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut labels: Vec<&str> = rows.collect();
    labels.sort_unstable();
    labels
}

const INIT_X: &str = r#"<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_ix" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_ia" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="init1"/>
</org.eventb.core.event>"#;

#[test]
fn an_unlabeled_action_takes_no_part_in_a_conflict() {
    // The blank-labelled action assigns `x` like `act2`. Rodin drops it
    // before judging assignments, so `act2` is kept and EB037 stays silent.
    let m = machine(
        "M.bum",
        &format!(
            r#"{INIT_X}
<org.eventb.core.event name="_e" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="x ≔ 1" org.eventb.core.label="  "/>
<org.eventb.core.action name="_a2" org.eventb.core.assignment="x ≔ 2" org.eventb.core.label="act2"/>
</org.eventb.core.event>"#
        ),
    );
    let result = build_all(vec![m]);

    let eb032 = with_rule(&result, RuleId::MissingLabel);
    assert_eq!(eb032.len(), 1, "{:?}", result.diagnostics);
    assert_eq!(eb032[0].origin, "M.evt");
    assert_eq!(eb032[0].severity, Severity::Error);
    assert_eq!(
        eb032[0].message,
        "the action at position 1 has no label, so it was dropped"
    );
    assert!(with_rule(&result, RuleId::ConflictingAssignments).is_empty());

    let v = view(&result, "M.bcm");
    assert!(v.accurate);
    let evt = &v.events["evt"];
    assert!(!evt.accurate);
    assert_eq!(
        labels(evt.actions.values().map(|a| a.label.as_str())),
        ["act2"]
    );
}

#[test]
fn an_unlabeled_initialisation_action_leaves_its_variable_to_the_repair() {
    let m = machine(
        "M.bum",
        r#"<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.variable name="_y" org.eventb.core.identifier="y"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ ∧ y ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="x ≔ 0"/>
<org.eventb.core.action name="_a2" org.eventb.core.assignment="y ≔ 0" org.eventb.core.label="act2"/>
</org.eventb.core.event>"#,
    );
    let result = build_all(vec![m]);

    let origins = |rule| {
        with_rule(&result, rule)
            .iter()
            .map(|d| d.origin.as_str())
            .collect::<Vec<_>>()
    };
    assert_eq!(origins(RuleId::MissingLabel), ["M.INITIALISATION"]);
    assert_eq!(
        origins(RuleId::IncompleteInitialisation),
        ["M.INITIALISATION"]
    );

    // The unlabeled action takes no label, so the repair is `GEN`.
    let init = &view(&result, "M.bcm").events["INITIALISATION"];
    assert!(!init.accurate);
    let mut actions: Vec<String> = init
        .actions
        .values()
        .map(|a| {
            format!(
                "{}: {}",
                a.label,
                rossi_build::normalize::canonical_action(&a.action)
            )
        })
        .collect();
    actions.sort();
    assert_eq!(actions, ["GEN: x :∣ ⊤", "act2: y ≔ 0"]);
}

#[test]
fn an_unlabeled_guard_is_never_checked() {
    // The guard reads an undeclared `z`, which Rodin never reports: the
    // guard is dropped for its missing label first.
    let m = machine(
        "M.bum",
        &format!(
            r#"{INIT_X}
<org.eventb.core.event name="_e" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt">
<org.eventb.core.guard name="_g1" org.eventb.core.predicate="z = 0"/>
<org.eventb.core.guard name="_g2" org.eventb.core.label="grd2" org.eventb.core.predicate="x ≥ 0"/>
</org.eventb.core.event>"#
        ),
    );
    let result = build_all(vec![m]);

    let errors = errors(&result);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].rule_id, Some(RuleId::MissingLabel));
    assert_eq!(
        errors[0].message,
        "the guard at position 1 has no label, so it was dropped"
    );

    let evt = &view(&result, "M.bcm").events["evt"];
    assert!(!evt.accurate);
    assert_eq!(
        labels(evt.guards.values().map(|g| g.label.as_str())),
        ["grd2"]
    );
}

#[test]
fn an_unlabeled_invariant_types_nothing() {
    // The only typing of `y` is unlabeled, so `y` stays untyped (EB006).
    let m = machine(
        "M.bum",
        r#"<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.variable name="_y" org.eventb.core.identifier="y"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.invariant name="_i2" org.eventb.core.predicate="y ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>"#,
    );
    let result = build_all(vec![m]);

    let eb032 = with_rule(&result, RuleId::MissingLabel);
    assert_eq!(eb032.len(), 1, "{:?}", result.diagnostics);
    assert_eq!(eb032[0].origin, "M");
    assert_eq!(
        eb032[0].message,
        "the invariant at position 2 has no label, so it was dropped"
    );
    assert!(
        with_rule(&result, RuleId::TypeError)
            .iter()
            .any(|d| d.origin == "M.y"),
        "{:?}",
        result.diagnostics
    );

    let v = view(&result, "M.bcm");
    assert!(!v.accurate);
    assert_eq!(
        labels(v.invariants.values().map(|i| i.label.as_str())),
        ["inv1"]
    );
    assert!(!v.variables.contains_key("y"));
}

#[test]
fn an_unlabeled_axiom_is_dropped() {
    let c = context(
        "C.buc",
        r#"<org.eventb.core.constant name="_k" org.eventb.core.identifier="k"/>
<org.eventb.core.axiom name="_a1" org.eventb.core.label="axm1" org.eventb.core.predicate="k ∈ ℤ"/>
<org.eventb.core.axiom name="_a2" org.eventb.core.label="" org.eventb.core.predicate="k &gt; 0"/>"#,
    );
    let result = build_all(vec![c]);

    let errors = errors(&result);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].rule_id, Some(RuleId::MissingLabel));
    assert_eq!(errors[0].origin, "C");

    let v = view(&result, "C.bcc");
    assert!(!v.accurate);
    assert_eq!(
        labels(v.axioms.values().map(|a| a.label.as_str())),
        ["axm1"]
    );
}

/// The abstract machine of the witness tests: `evt` takes a parameter `p`.
fn m0_with_parameter() -> ProjectComponent {
    machine(
        "M0.bum",
        &format!(
            r#"{INIT_X}
<org.eventb.core.event name="_e" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt">
<org.eventb.core.parameter name="_p" org.eventb.core.identifier="p"/>
<org.eventb.core.guard name="_g" org.eventb.core.label="grd1" org.eventb.core.predicate="p ∈ ℤ"/>
<org.eventb.core.action name="_a" org.eventb.core.assignment="x ≔ p" org.eventb.core.label="act1"/>
</org.eventb.core.event>"#
        ),
    )
}

#[test]
fn an_unlabeled_witness_leaves_its_parameter_unwitnessed() {
    // `evt` drops the abstract parameter `p`. Its only witness is unlabeled,
    // so the default witness (EB035) stands in for it.
    let m0 = m0_with_parameter();
    let m1 = machine(
        "M1.bum",
        r#"<org.eventb.core.refinesMachine name="_r" org.eventb.core.target="M0"/>
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION"/>
<org.eventb.core.event name="_e" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt">
<org.eventb.core.refinesEvent name="_re" org.eventb.core.target="evt"/>
<org.eventb.core.witness name="_w" org.eventb.core.predicate="p = 0"/>
<org.eventb.core.action name="_a" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>"#,
    );
    let result = build_all(vec![m0, m1]);

    let eb032 = with_rule(&result, RuleId::MissingLabel);
    assert_eq!(eb032.len(), 1, "{:?}", result.diagnostics);
    assert_eq!(eb032[0].origin, "M1.evt");
    assert_eq!(
        eb032[0].message,
        "the witness at position 1 has no label, so it was dropped"
    );
    assert!(
        !with_rule(&result, RuleId::MissingWitness).is_empty(),
        "{:?}",
        result.diagnostics
    );
    let evt = &view(&result, "M1.bcm").events["evt"];
    assert!(!evt.accurate);
}

#[test]
fn an_unlabeled_witness_on_a_new_event_still_makes_it_inaccurate() {
    let m0 = m0_with_parameter();
    let m1 = machine(
        "M1.bum",
        r#"<org.eventb.core.refinesMachine name="_r" org.eventb.core.target="M0"/>
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION"/>
<org.eventb.core.event name="_e" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="evt">
<org.eventb.core.refinesEvent name="_re" org.eventb.core.target="evt"/>
</org.eventb.core.event>
<org.eventb.core.event name="_n" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="fresh">
<org.eventb.core.witness name="_w" org.eventb.core.predicate="x = 0"/>
</org.eventb.core.event>"#,
    );
    let result = build_all(vec![m0, m1]);

    let eb032 = with_rule(&result, RuleId::MissingLabel);
    assert_eq!(eb032.len(), 1, "{:?}", result.diagnostics);
    assert_eq!(eb032[0].origin, "M1.fresh");
    let fresh = &view(&result, "M1.bcm").events["fresh"];
    assert!(!fresh.accurate);
    assert!(fresh.witnesses.is_empty());
}
