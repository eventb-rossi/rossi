//! INITIALISATION repair: a concrete, typed variable that no action
//! assigns gets a synthetic `becomesSuchThat ⊤` default, all such
//! variables gathered into one `GEN` action, and the event (not the
//! machine) is marked inaccurate. Each repaired variable draws an EB014
//! warning, Rodin's `InitialisationIncompleteWarning`.

mod common;

use common::diags_with_rule;
use rossi_build::{
    BuildResult, Project, ProjectComponent, RuleId, Severity, build, sc_view::ScView,
};

/// The EB014 messages on `machine`'s INITIALISATION, in report order.
fn eb014<'a>(result: &'a BuildResult, machine: &str) -> Vec<&'a str> {
    let origin = format!("{machine}.INITIALISATION");
    diags_with_rule(&result.diagnostics, RuleId::IncompleteInitialisation)
        .into_iter()
        .filter(|d| d.origin == origin)
        .map(|d| d.message.as_str())
        .collect()
}

/// The EB014 message for variable `var`, initialised by the action `label`.
fn repaired(var: &str, label: &str) -> String {
    format!(
        "variable `{var}` is not assigned by INITIALISATION, so the generated action `{label}` \
         gives it an arbitrary initial value"
    )
}

// --- standalone machine, INITIALISATION has no actions ---------------
const ONE_VAR_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_v" org.eventb.core.identifier="v"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="I" org.eventb.core.predicate="v ∈ ℕ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION"/>
</org.eventb.core.machineFile>
"#;

// Byte-exact expectation for the case above: the
// machine stays accurate, INITIALISATION is inaccurate, and carries one
// generated `v :∣ ⊤` action sourced at the event element.
const ONE_VAR_BCM: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.scMachineFile org.eventb.core.accurate="true" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.scInvariant name="'" org.eventb.core.label="I" org.eventb.core.predicate="v∈ℕ" org.eventb.core.source="/r/M.bum|org.eventb.core.machineFile#M|org.eventb.core.invariant#_i" org.eventb.core.theorem="false"/>
<org.eventb.core.scVariable name="v" org.eventb.core.abstract="false" org.eventb.core.concrete="true" org.eventb.core.source="/r/M.bum|org.eventb.core.machineFile#M|org.eventb.core.variable#_v" org.eventb.core.type="ℤ"/>
<org.eventb.core.scEvent name="w" org.eventb.core.accurate="false" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION" org.eventb.core.source="/r/M.bum|org.eventb.core.machineFile#M|org.eventb.core.event#_init">
<org.eventb.core.scAction name="'" org.eventb.core.assignment="v :∣ ⊤" org.eventb.core.label="GEN" org.eventb.core.source="/r/M.bum|org.eventb.core.machineFile#M|org.eventb.core.event#_init"/>
</org.eventb.core.scEvent>
</org.eventb.core.scMachineFile>"#;

// --- standalone machine, INITIALISATION assigns only one of two ------
const PARTIAL_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_a" org.eventb.core.identifier="a"/>
<org.eventb.core.variable name="_b" org.eventb.core.identifier="b"/>
<org.eventb.core.invariant name="_ia" org.eventb.core.label="ia" org.eventb.core.predicate="a ∈ ℕ"/>
<org.eventb.core.invariant name="_ib" org.eventb.core.label="ib" org.eventb.core.predicate="b ∈ ℕ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="a ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;

// --- standalone machine, INITIALISATION empty, two variables --------
// Variables are declared b-then-a to confirm the combined GEN LHS is
// emitted in rossi's deterministic alphabetical order, not source order.
const EMPTY_TWO_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_b" org.eventb.core.identifier="b"/>
<org.eventb.core.variable name="_a" org.eventb.core.identifier="a"/>
<org.eventb.core.invariant name="_ia" org.eventb.core.label="ia" org.eventb.core.predicate="a ∈ ℕ"/>
<org.eventb.core.invariant name="_ib" org.eventb.core.label="ib" org.eventb.core.predicate="b ∈ ℕ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION"/>
</org.eventb.core.machineFile>
"#;

// --- refinement, extended INITIALISATION, both INITs incomplete ------
const ABS_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_ix" org.eventb.core.label="ix" org.eventb.core.predicate="x ∈ ℕ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION"/>
</org.eventb.core.machineFile>
"#;

const REF_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="Abs"/>
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.variable name="_y" org.eventb.core.identifier="y"/>
<org.eventb.core.invariant name="_iy" org.eventb.core.label="iy" org.eventb.core.predicate="y ∈ ℕ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION"/>
</org.eventb.core.machineFile>
"#;

// --- standalone machine, INITIALISATION drops two of its actions ----
// `GEN` is ill-typed and `GEN1` reads an undeclared `z`.
const DROPPED_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_w" org.eventb.core.identifier="w"/>
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.variable name="_y" org.eventb.core.identifier="y"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="I" org.eventb.core.predicate="w ∈ ℤ ∧ x ∈ ℤ ∧ y ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="x ≔ TRUE" org.eventb.core.label="GEN"/>
<org.eventb.core.action name="_a2" org.eventb.core.assignment="y ≔ z" org.eventb.core.label="GEN1"/>
<org.eventb.core.action name="_a3" org.eventb.core.assignment="w ≔ 0" org.eventb.core.label="a"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;

fn one_var() -> Project {
    Project::new(
        "r",
        vec![ProjectComponent::from_xml("M.bum", ONE_VAR_BUM).unwrap()],
    )
}

fn partial() -> Project {
    Project::new(
        "r",
        vec![ProjectComponent::from_xml("M.bum", PARTIAL_BUM).unwrap()],
    )
}

fn empty_two() -> Project {
    Project::new(
        "r",
        vec![ProjectComponent::from_xml("M.bum", EMPTY_TWO_BUM).unwrap()],
    )
}

fn dropped() -> Project {
    Project::new(
        "r",
        vec![ProjectComponent::from_xml("M.bum", DROPPED_BUM).unwrap()],
    )
}

fn refinement() -> Project {
    Project::new(
        "r",
        vec![
            ProjectComponent::from_xml("Abs.bum", ABS_BUM).unwrap(),
            ProjectComponent::from_xml("Ref.bum", REF_BUM).unwrap(),
        ],
    )
}

#[test]
fn standalone_empty_init_repairs_single_variable_byte_exact() {
    let r = build(&one_var());
    let bcm = r.file("M.bcm").expect("M.bcm");
    assert_eq!(bcm.contents.trim_end(), ONE_VAR_BCM.trim_end());
    assert_eq!(eb014(&r, "M"), [repaired("v", "GEN")]);
    let warnings = diags_with_rule(&r.diagnostics, RuleId::IncompleteInitialisation);
    assert_eq!(warnings[0].severity, Severity::Warning);
}

#[test]
fn partial_init_repairs_only_the_unassigned_variable() {
    let r = build(&partial());
    let bcm = &r.file("M.bcm").unwrap().contents;
    // `a` is assigned, so only `b` is repaired; the real action survives.
    assert!(bcm.contains(r#"org.eventb.core.assignment="a ≔ 0" org.eventb.core.label="act1""#));
    assert!(bcm.contains(r#"org.eventb.core.assignment="b :∣ ⊤" org.eventb.core.label="GEN""#));
    assert!(
        !bcm.contains(r#"assignment="a :∣"#),
        "assigned variable `a` must not be repaired:\n{bcm}"
    );
    let v = ScView::from_xml(bcm).unwrap();
    assert!(v.accurate);
    assert!(!v.events["INITIALISATION"].accurate);
    assert_eq!(eb014(&r, "M"), [repaired("b", "GEN")]);
}

#[test]
fn empty_init_combines_unassigned_variables_into_one_action() {
    let r = build(&empty_two());
    let bcm = &r.file("M.bcm").unwrap().contents;
    // One combined `GEN` action, variables in alphabetical order.
    assert!(
        bcm.contains(r#"org.eventb.core.assignment="a,b :∣ ⊤" org.eventb.core.label="GEN""#),
        "expected a single combined GEN action `a,b :∣ ⊤`:\n{bcm}"
    );
    // Exactly one scAction (the combined GEN), not one per variable. ScView
    // keys actions by source URI, so per-variable GEN actions would collapse
    // to a single entry there and hide the regression; count raw elements.
    assert_eq!(
        bcm.matches("<org.eventb.core.scAction ").count(),
        1,
        "all unassigned variables must share one GEN action:\n{bcm}"
    );
    // One warning per variable, in the action's (alphabetical) order.
    assert_eq!(eb014(&r, "M"), [repaired("a", "GEN"), repaired("b", "GEN")]);
}

#[test]
fn dropped_actions_keep_their_labels_taken() {
    let r = build(&dropped());
    let bcm = &r.file("M.bcm").unwrap().contents;
    // Rodin enters each action's label in the event's label table before it
    // checks the assignment, so a dropped action's label stays taken: the
    // repair of `x` and `y` skips `GEN` and `GEN1`. This matches what Rodin
    // 3.10 builds from the same machine.
    assert!(
        bcm.contains(r#"org.eventb.core.assignment="x,y :∣ ⊤" org.eventb.core.label="GEN12""#),
        "{bcm}"
    );
    assert_eq!(
        bcm.matches("<org.eventb.core.scAction ").count(),
        2,
        "{bcm}"
    );
    // The source text assigns both, but the checker initialises them.
    assert_eq!(
        eb014(&r, "M"),
        [repaired("x", "GEN12"), repaired("y", "GEN12")]
    );
}

#[test]
fn extended_init_inherits_repair_and_adds_fresh_label() {
    let r = build(&refinement());

    // Abstract INIT is repaired for its own variable `x`.
    let abs = &r.file("Abs.bcm").unwrap().contents;
    assert!(abs.contains(r#"org.eventb.core.assignment="x :∣ ⊤" org.eventb.core.label="GEN""#));

    // The extended child inherits that `GEN: x :∣ ⊤` (so `x` is already
    // assigned) and repairs only the new variable `y` under a fresh
    // `GEN1` label.
    let refn = &r.file("Ref.bcm").unwrap().contents;
    assert!(refn.contains(r#"org.eventb.core.assignment="x :∣ ⊤" org.eventb.core.label="GEN""#));
    assert!(refn.contains(r#"org.eventb.core.assignment="y :∣ ⊤" org.eventb.core.label="GEN1""#));

    let v = ScView::from_xml(refn).unwrap();
    let init = &v.events["INITIALISATION"];
    assert!(!init.accurate);
    assert_eq!(init.actions.len(), 2, "inherited x plus generated y");

    // Each machine warns about the variable it initialises itself: `Ref`
    // inherits the repair of `x` and does not repeat its warning.
    assert_eq!(eb014(&r, "Abs"), [repaired("x", "GEN")]);
    assert_eq!(eb014(&r, "Ref"), [repaired("y", "GEN1")]);
}

/// A machine whose INITIALISATION assigns its one variable `v`.
const M0_TEXT: &str = "\
machine M0
variables v
invariants
  @t v ∈ ℤ
events
  event INITIALISATION
    then
      @a v ≔ 0
  end
end
";

#[test]
fn a_fully_assigned_extended_chain_draws_no_warning() {
    let r = build(&common::text_project(
        "p",
        &[
            ("M0", M0_TEXT),
            (
                "M1",
                "\
machine M1 refines M0
variables v w
invariants
  @t w ∈ ℤ
events
  event INITIALISATION extends INITIALISATION
    then
      @b w ≔ 0
  end
end
",
            ),
            (
                "M2",
                "\
machine M2 refines M1
variables v w u
invariants
  @t u ∈ ℤ
events
  event INITIALISATION extends INITIALISATION
    then
      @c u ≔ 0
  end
end
",
            ),
        ],
    ));
    for machine in ["M0", "M1", "M2"] {
        assert!(eb014(&r, machine).is_empty(), "{:?}", r.diagnostics);
    }
}

#[test]
fn a_machine_without_initialisation_warns_for_each_variable() {
    let text = "\
machine m
variables b a c
invariants
  @t a ∈ ℤ ∧ b ∈ ℤ ∧ c ∈ ℤ
end
";
    let r = build(&common::text_project("p", &[("m", text)]));

    let warnings = diags_with_rule(&r.diagnostics, RuleId::IncompleteInitialisation);
    let messages: Vec<&str> = warnings.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(
        messages,
        ["a", "b", "c"].map(|v| format!(
            "variable `{v}` is not assigned by INITIALISATION (no INITIALISATION event)"
        )),
        "{:?}",
        r.diagnostics
    );
    // Each warning points at the variable's declaration.
    for (warning, name) in warnings.iter().zip(["a", "b", "c"]) {
        let span = warning.span.expect("anchored");
        assert_eq!(&text[span.start..span.end], name);
    }
}

#[test]
fn a_dropped_initialisation_warns_for_each_variable() {
    // `M1` drops `v`, which the INITIALISATION it extends assigns, so the
    // checker leaves INITIALISATION out and nothing initialises `w`.
    let r = build(&common::text_project(
        "p",
        &[
            ("M0", M0_TEXT),
            (
                "M1",
                "\
machine M1 refines M0
variables w
invariants
  @t w ∈ ℤ
events
  event INITIALISATION extends INITIALISATION
    then
      @b w ≔ 0
  end
end
",
            ),
        ],
    ));

    let m1 = ScView::from_xml(&r.file("M1.bcm").unwrap().contents).unwrap();
    assert!(!m1.events.contains_key("INITIALISATION"));
    assert_eq!(
        eb014(&r, "M1"),
        ["variable `w` is not assigned by INITIALISATION (INITIALISATION was dropped)"]
    );
}

#[test]
fn build_reports_incomplete_initialisation_and_lint_does_not() {
    // `rossi validate` folds `build()` diagnostics and `lint::run`, so EB014
    // must come from exactly one of them.
    let project = partial();
    assert_eq!(eb014(&build(&project), "M").len(), 1);
    let lint = rossi_build::lint::run(&project);
    assert!(
        diags_with_rule(&lint, RuleId::IncompleteInitialisation).is_empty(),
        "{lint:?}"
    );
}
