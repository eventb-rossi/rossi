//! EB037: the actions of an event must assign disjoint sets of variables.
//! Rodin reports `ActionDisjointLHSError` on every action involved and drops
//! it from the checked event (`MachineEventActionModule`). An extended
//! event's inherited actions take part: the own action that re-assigns one
//! of their variables is dropped, the inherited action kept. The event stays
//! accurate unless INITIALISATION has to repair a variable. The first case
//! follows Rodin's `TestEvents.testEvents_16_actionMultipleLHSConflict`; the
//! checked events here match what Rodin 3.10 builds from the same models.

mod common;

use rossi_build::normalize::canonical_action;
use rossi_build::sc_view::{EventRow, ScView};
use rossi_build::{BuildResult, Diagnostic, RuleId, Severity, build};

fn build_text(files: &[(&str, &str)]) -> BuildResult {
    build(&common::text_project("p", files))
}

/// The EB037 findings, by origin.
fn eb037(result: &BuildResult) -> Vec<&Diagnostic> {
    result
        .diagnostics
        .iter()
        .filter(|d| d.rule_id == Some(RuleId::ConflictingAssignments))
        .collect()
}

fn origins<'a>(findings: &[&'a Diagnostic]) -> Vec<&'a str> {
    findings.iter().map(|d| d.origin.as_str()).collect()
}

/// The checked event `event` of machine `machine`.
fn checked_event(result: &BuildResult, machine: &str, event: &str) -> EventRow {
    let file = result
        .file(&format!("{machine}.bcm"))
        .unwrap_or_else(|| panic!("missing {machine}.bcm"));
    let mut view = ScView::from_xml(&file.contents).unwrap();
    view.events
        .remove(event)
        .unwrap_or_else(|| panic!("missing {event} in {machine}.bcm"))
}

/// The checked event's actions as `label: assignment`, sorted by label.
fn actions(event: &EventRow) -> Vec<String> {
    let mut actions: Vec<String> = event
        .actions
        .values()
        .map(|a| format!("{}: {}", a.label, canonical_action(&a.action)))
        .collect();
    actions.sort();
    actions
}

#[test]
fn every_action_sharing_a_variable_is_dropped() {
    let result = build_text(&[(
        "mac",
        "\
machine mac
variables u v w x y z
invariants
  @I1 u ∈ ℕ ∧ v ∈ ℕ ∧ w ∈ ℕ ∧ x ∈ ℕ ∧ y ∈ ℕ ∧ z ∈ ℕ
events
  event INITIALISATION
    then
      @init u, v, w, x, y, z ≔ 0, 0, 0, 0, 0, 0
  end
  event evt
    then
      @A1 u ≔ 1
      @A2 v, w :∣ ⊤
      @A3 v ≔ 1
      @A4 x ≔ u
      @A5 y, z, w ≔ v, w, 1
  end
end
",
    )]);

    let findings = eb037(&result);
    assert_eq!(
        origins(&findings),
        ["mac.evt.A2", "mac.evt.A3", "mac.evt.A5"],
        "{:?}",
        result.diagnostics
    );
    assert!(findings.iter().all(|d| d.severity == Severity::Error));
    // A3 assigns only `v` and A5 only `w`, so each is named with its own.
    assert!(
        findings[0].message.starts_with(
            "action `A2` assigns `v`, which action `A3` also assigns, and `w`, which action `A5` \
             also assigns;"
        ),
        "{}",
        findings[0].message
    );

    let evt = checked_event(&result, "mac", "evt");
    assert!(evt.accurate);
    assert_eq!(actions(&evt), ["A1: u ≔ 1", "A4: x ≔ u"]);
}

#[test]
fn variables_shared_with_the_same_actions_are_named_together() {
    let result = build_text(&[(
        "m",
        "\
machine m
variables x y
invariants
  @t x ∈ ℤ ∧ y ∈ ℤ
events
  event INITIALISATION
    then
      @init x, y ≔ 0, 0
  end
  event e
    then
      @a x, y ≔ 1, 1
      @b y, x ≔ 2, 2
  end
end
",
    )]);

    let findings = eb037(&result);
    assert_eq!(origins(&findings), ["m.e.a", "m.e.b"]);
    assert!(
        findings[0]
            .message
            .starts_with("action `a` assigns `x` and `y`, which action `b` also assigns;"),
        "{}",
        findings[0].message
    );
}

#[test]
fn two_updates_of_one_function_conflict() {
    let text = "\
machine m
variables f
invariants
  @t f ∈ ℤ → ℤ
events
  event INITIALISATION
    then
      @a f ≔ ℤ × {0}
  end
  event set
    then
      @a1 f(1) ≔ 2
      @a2 f(3) ≔ 4
  end
end
";
    let result = build_text(&[("m", text)]);

    let findings = eb037(&result);
    assert_eq!(origins(&findings), ["m.set.a1", "m.set.a2"]);
    assert!(
        findings[0]
            .message
            .contains("which action `a2` also assigns")
    );
    // Each finding points at the function its own action assigns.
    let anchors: Vec<&str> = findings
        .iter()
        .map(|d| {
            let span = d.span.expect("anchored");
            &text[span.start..span.end]
        })
        .collect();
    assert_eq!(anchors, ["f", "f"]);
    assert_ne!(findings[0].span, findings[1].span);

    let set = checked_event(&result, "m", "set");
    assert!(set.accurate);
    assert!(actions(&set).is_empty());
}

const M0: &str = "\
machine M0
variables x
invariants
  @t x ∈ ℤ
events
  event INITIALISATION
    then
      @init x ≔ 0
  end
  event inc
    then
      @act1 x ≔ x + 1
  end
end
";

#[test]
fn an_own_action_loses_to_an_inherited_one() {
    let m1 = "\
machine M1 refines M0
variables x y
invariants
  @t2 y ∈ ℤ
events
  event INITIALISATION extends INITIALISATION
    then
      @init2 y ≔ 0
  end
  event inc extends inc
    then
      @act2 x ≔ 0
      @act3 y ≔ y + 1
  end
end
";
    let result = build_text(&[("M0", M0), ("M1", m1)]);

    let findings = eb037(&result);
    assert_eq!(
        origins(&findings),
        ["M1.inc.act2"],
        "{:?}",
        result.diagnostics
    );
    assert!(
        findings[0]
            .message
            .contains("which inherited action `act1` also assigns"),
        "{}",
        findings[0].message
    );

    let inc = checked_event(&result, "M1", "inc");
    assert!(inc.accurate);
    assert_eq!(actions(&inc), ["act1: x ≔ x+1", "act3: y ≔ y+1"]);
}

#[test]
fn initialisation_reinitialises_a_variable_in_conflict() {
    // Rodin counts a variable in conflict as unassigned, so its repair
    // action initialises it even though the inherited action still does.
    let m1 = "\
machine M1 refines M0
variables x y
invariants
  @t2 y ∈ ℤ
events
  event INITIALISATION extends INITIALISATION
    then
      @init2 x ≔ 1
      @init3 y ≔ 0
  end
  event inc extends inc
  end
end
";
    let result = build_text(&[("M0", M0), ("M1", m1)]);

    assert_eq!(
        origins(&eb037(&result)),
        ["M1.INITIALISATION.init2"],
        "{:?}",
        result.diagnostics
    );
    let init = checked_event(&result, "M1", "INITIALISATION");
    assert!(!init.accurate);
    assert_eq!(
        actions(&init),
        ["GEN: x :∣ ⊤", "init3: y ≔ 0", "init: x ≔ 0"]
    );
}

#[test]
fn initialisation_repairs_only_the_variables_in_conflict() {
    // A dropped action still counts for Rodin's repair: a variable it alone
    // assigns (`x`) is not repaired, and its label is taken, so the repair
    // action is `GEN1`.
    let result = build_text(&[(
        "m",
        "\
machine m
variables x y z
invariants
  @t x ∈ ℤ ∧ y ∈ ℤ ∧ z ∈ ℤ
events
  event INITIALISATION
    then
      @GEN x, y ≔ 0, 0
      @b y ≔ 1
      @c z ≔ 0
  end
end
",
    )]);

    assert_eq!(
        origins(&eb037(&result)),
        ["m.INITIALISATION.GEN", "m.INITIALISATION.b"],
        "{:?}",
        result.diagnostics
    );
    let init = checked_event(&result, "m", "INITIALISATION");
    assert!(!init.accurate);
    assert_eq!(actions(&init), ["GEN1: y :∣ ⊤", "c: z ≔ 0"]);
}

#[test]
fn an_action_naming_a_variable_twice_conflicts_with_itself() {
    let result = build_text(&[(
        "m",
        "\
machine m
variables x
invariants
  @t x ∈ ℤ
events
  event INITIALISATION
    then
      @a x ≔ 0
  end
  event e
    then
      @a1 x, x ≔ 1, 2
  end
end
",
    )]);

    let findings = eb037(&result);
    assert_eq!(origins(&findings), ["m.e.a1"]);
    assert!(
        findings[0]
            .message
            .starts_with("action `a1` assigns `x` more than once;"),
        "{}",
        findings[0].message
    );
    let e = checked_event(&result, "m", "e");
    assert!(e.accurate);
    assert!(actions(&e).is_empty());
}

#[test]
fn disjoint_actions_and_one_multiple_assignment_are_accepted() {
    let result = build_text(&[(
        "m",
        "\
machine m
variables x y z
invariants
  @t x ∈ ℤ ∧ y ∈ ℤ ∧ z ∈ ℤ
events
  event INITIALISATION
    then
      @a x ≔ 0
      @b y ≔ 0
      @c z ≔ 0
  end
  event swap
    then
      @a1 x, y ≔ y, x
      @a2 z ≔ x + y
  end
end
",
    )]);

    assert!(eb037(&result).is_empty(), "{:?}", result.diagnostics);
    let swap = checked_event(&result, "m", "swap");
    assert!(swap.accurate);
    assert_eq!(actions(&swap).len(), 2);
}
