//! `rossi_build::runtime` — the EB1xx runtime-translation suitability pass.
//!
//! Each rule gets a model that trips it and a model that must not, because
//! the whole point of the pass is that it stays quiet on the constructs a
//! translation can already handle.

mod common;

use rossi_build::{Diagnostic, RuleId, Severity, build_with_model};

/// Build a project from `.eventb` texts and run the pass over it.
fn findings(files: &[(&str, &str)]) -> Vec<Diagnostic> {
    let project = common::text_project("runtime", files);
    let (build, model) = build_with_model(&project);
    assert!(
        !build
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error),
        "fixture must type-check: {:?}",
        build.diagnostics
    );
    rossi_build::runtime::run(&project, &model)
}

/// The `(code, origin)` pairs a run reported, for compact assertions.
fn codes(diags: &[Diagnostic]) -> Vec<(&str, &str)> {
    diags
        .iter()
        .map(|d| {
            (
                d.rule_id.expect("every runtime finding is tagged").code(),
                d.origin.as_str(),
            )
        })
        .collect()
}

fn only(diags: &[Diagnostic], rule: RuleId) -> Vec<&Diagnostic> {
    diags.iter().filter(|d| d.rule_id == Some(rule)).collect()
}

// ---------------------------------------------------------------------
// EB100 — becomes-member-of in an ordinary event
// ---------------------------------------------------------------------

const CTX: &str = "context C\nconstants n\naxioms\n    @a1 n = 5\nend\n";

#[test]
fn eb100_flags_set_choice_in_an_ordinary_event() {
    let machine = "machine M\nsees C\nvariables x\ninvariants\n    @i1 x ∈ ℤ\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  then\n    @act1 x :∈ 0 ‥ n\nend\nend\n";
    let diags = findings(&[("C.eventb", CTX), ("M.eventb", machine)]);
    assert_eq!(codes(&diags), vec![("EB100", "M.step/act1")]);
    let found = only(&diags, RuleId::BecomesMemberOfInEvent);
    assert_eq!(found[0].severity, Severity::Warning);
    assert!(
        found[0].message.contains("`x`") && found[0].message.contains("singleton"),
        "message names the variable and the missing evidence: {}",
        found[0].message
    );
    assert!(found[0].span.is_some(), "own action keeps its span");
}

#[test]
fn eb100_exempts_a_singleton_set() {
    let machine = "machine M\nvariables x\ninvariants\n    @i1 x ∈ ℤ\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  then\n    @act1 x :∈ {3}\nend\nend\n";
    assert_eq!(codes(&findings(&[("M.eventb", machine)])), Vec::new());
}

#[test]
fn eb100_stays_silent_on_an_abstract_machine() {
    // The pass checks the machine a translation consumes. `M0` is refined by
    // `M1`, so its `:∈` is an abstraction that `M1` has already resolved.
    let abstract_machine = "machine M0\nvariables x\ninvariants\n    @i1 x ∈ ℤ\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  then\n    @act1 x :∈ 0 ‥ 9\nend\nend\n";
    let leaf = "machine M1\nrefines M0\nvariables x\ninvariants\n    @i1 x ∈ ℤ\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step refines step\n  then\n    @act1 x ≔ 4\nend\nend\n";
    let diags = findings(&[("M0.eventb", abstract_machine), ("M1.eventb", leaf)]);
    assert_eq!(codes(&diags), Vec::new());
}

// ---------------------------------------------------------------------
// EB101 — becomes-such-that outside the canonical form
// ---------------------------------------------------------------------

#[test]
fn eb101_flags_a_condition_that_only_constrains() {
    let machine = "machine M\nvariables x\ninvariants\n    @i1 x ∈ ℤ\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  then\n    @act1 x :∣ x' > x\nend\nend\n";
    let diags = findings(&[("M.eventb", machine)]);
    assert_eq!(codes(&diags), vec![("EB101", "M.step/act1")]);
    let found = only(&diags, RuleId::NonCanonicalBecomesSuchThat);
    assert_eq!(found[0].severity, Severity::Warning);
    assert!(
        found[0].message.contains("equality over its primed name"),
        "message names the missing shape: {}",
        found[0].message
    );
}

#[test]
fn eb101_is_silent_on_a_single_determining_equality() {
    let machine = "machine M\nvariables x\ninvariants\n    @i1 x ∈ ℤ\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  then\n    @act1 x :∣ x' = x + 1\nend\nend\n";
    assert_eq!(codes(&findings(&[("M.eventb", machine)])), Vec::new());
}

#[test]
fn eb101_reports_unverified_branch_conditions_at_info() {
    let machine = "machine M\nvariables x\ninvariants\n    @i1 x ∈ ℤ\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  then\n    @act1 x :∣ (x > 0 ∧ x' = 1) ∨ (x ≤ 0 ∧ x' = 2)\nend\nend\n";
    let diags = findings(&[("M.eventb", machine)]);
    assert_eq!(codes(&diags), vec![("EB101", "M.step/act1")]);
    let found = only(&diags, RuleId::NonCanonicalBecomesSuchThat);
    assert_eq!(
        found[0].severity,
        Severity::Info,
        "a readable shape is advisory, not a warning"
    );
    assert!(
        found[0].message.contains("2 branch conditions")
            && found[0]
                .message
                .contains("exhaustive and mutually exclusive"),
        "message counts the branches and names what was not verified: {}",
        found[0].message
    );
}

#[test]
fn eb101_rejects_a_branch_that_leaves_a_variable_open() {
    // The second branch fixes `x` but only bounds `y`, so a translation
    // taking it still has to choose.
    let machine = "machine M\nvariables x\n    y\ninvariants\n    @i1 x ∈ ℤ\n    @i2 y ∈ ℤ\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\n    @act2 y ≔ 0\nend\n\
        event step\n  then\n    @act1 x, y :∣ (x > 0 ∧ x' = 1 ∧ y' = 2) ∨ (x ≤ 0 ∧ x' = 3 ∧ y' > 0)\nend\nend\n";
    let diags = findings(&[("M.eventb", machine)]);
    assert_eq!(codes(&diags), vec![("EB101", "M.step/act1")]);
    assert_eq!(diags[0].severity, Severity::Warning);
}

#[test]
fn eb101_rejects_an_after_state_read_on_the_defining_side() {
    // `x' = y' + 1` defines `x'` in terms of another after-state value, which
    // is a simultaneous constraint, not a value to read off.
    let machine = "machine M\nvariables x\n    y\ninvariants\n    @i1 x ∈ ℤ\n    @i2 y ∈ ℤ\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\n    @act2 y ≔ 0\nend\n\
        event step\n  then\n    @act1 x, y :∣ x' = y' + 1 ∧ y' = 2\nend\nend\n";
    let diags = findings(&[("M.eventb", machine)]);
    assert_eq!(codes(&diags), vec![("EB101", "M.step/act1")]);
    assert_eq!(diags[0].severity, Severity::Warning);
}

// ---------------------------------------------------------------------
// EB102 — nondeterministic or state-reading initialisation
// ---------------------------------------------------------------------

#[test]
fn eb102_flags_set_choice_in_initialisation() {
    let machine = "machine M\nvariables x\ninvariants\n    @i1 x ∈ ℤ\nevents\n\
        event INITIALISATION\n  then\n    @act1 x :∈ 0 ‥ 9\nend\nend\n";
    let diags = findings(&[("M.eventb", machine)]);
    assert_eq!(codes(&diags), vec![("EB102", "M.INITIALISATION/act1")]);
    assert_eq!(diags[0].severity, Severity::Warning);
    assert!(
        diags[0].message.contains("supplied outside the model"),
        "message says who has to provide the value: {}",
        diags[0].message
    );
}

#[test]
fn eb102_flags_a_read_of_the_variable_being_assigned() {
    // `x ≔ x + 1` reads the before-state of `x`, which INITIALISATION is
    // what establishes: the target being read does not make the read safe.
    let machine = "machine M\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ x + 1\nend\nend\n";
    let diags = findings(&[("M.eventb", machine)]);
    assert_eq!(codes(&diags), vec![("EB102", "M.INITIALISATION/act1")]);
    assert!(
        diags[0].message.contains("`x`") && diags[0].message.contains("no value before"),
        "message names the variable read too early: {}",
        diags[0].message
    );
}

#[test]
fn eb102_flags_a_read_of_uninitialised_state() {
    // `y` has no value yet, so `x`'s initial value is whatever the
    // translation leaves in the slot.
    let machine = "machine M\nvariables x\n    y\ninvariants\n    @i1 x ∈ ℤ\n    @i2 y ∈ ℤ\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ y + 1\n    @act2 y ≔ 0\nend\nend\n";
    let diags = findings(&[("M.eventb", machine)]);
    assert_eq!(codes(&diags), vec![("EB102", "M.INITIALISATION/act1")]);
    assert!(
        diags[0].message.contains("`y`") && diags[0].message.contains("no value before"),
        "message names the variable read too early: {}",
        diags[0].message
    );
}

#[test]
fn eb102_accepts_a_deterministic_initialisation() {
    let machine = "machine M\nsees C\nvariables x\ninvariants\n    @i1 x ∈ ℤ\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ n\nend\nend\n";
    assert_eq!(
        codes(&findings(&[("C.eventb", CTX), ("M.eventb", machine)])),
        Vec::new(),
        "a constant is not machine state, so reading it is fine"
    );
}

#[test]
fn eb102_accepts_a_canonical_such_that_initialisation() {
    let machine = "machine M\nvariables x\ninvariants\n    @i1 x ∈ ℤ\nevents\n\
        event INITIALISATION\n  then\n    @act1 x :∣ x' = 7\nend\nend\n";
    assert_eq!(codes(&findings(&[("M.eventb", machine)])), Vec::new());
}

#[test]
fn eb102_flags_a_non_canonical_such_that_initialisation() {
    let machine = "machine M\nvariables x\ninvariants\n    @i1 x ∈ ℤ\nevents\n\
        event INITIALISATION\n  then\n    @act1 x :∣ x' ∈ ℕ\nend\nend\n";
    let diags = findings(&[("M.eventb", machine)]);
    assert_eq!(codes(&diags), vec![("EB102", "M.INITIALISATION/act1")]);
    assert!(
        diags[0]
            .message
            .contains("does not fix each variable with an equality"),
        "message names the missing shape: {}",
        diags[0].message
    );
}

// ---------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------

#[test]
fn inherited_actions_are_checked_without_a_span() {
    // `M1.step` extends `M0.step`, so the abstract `:∈` runs in the leaf and
    // is reported there — but its text lives in `M0.eventb`, and a span
    // would index the wrong file.
    let abstract_machine = "machine M0\nvariables x\ninvariants\n    @i1 x ∈ ℤ\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  then\n    @act1 x :∈ 0 ‥ 9\nend\nend\n";
    let leaf = "machine M1\nrefines M0\nvariables x\ninvariants\n    @i1 x ∈ ℤ\nevents\n\
        event INITIALISATION extends INITIALISATION\nend\n\
        event step extends step\nend\nend\n";
    let diags = findings(&[("M0.eventb", abstract_machine), ("M1.eventb", leaf)]);
    assert_eq!(codes(&diags), vec![("EB100", "M1.step/act1")]);
    assert!(
        diags[0].span.is_none(),
        "an inherited action carries no span into the refining component"
    );
}

#[test]
fn every_leaf_of_a_forked_refinement_is_checked() {
    let root = "machine M0\nvariables x\ninvariants\n    @i1 x ∈ ℤ\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\nend\n";
    let left = "machine L\nrefines M0\nvariables x\ninvariants\n    @i1 x ∈ ℤ\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  then\n    @act1 x :∈ 0 ‥ 9\nend\nend\n";
    let right = "machine R\nrefines M0\nvariables x\ninvariants\n    @i1 x ∈ ℤ\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  then\n    @act1 x :∈ 0 ‥ 9\nend\nend\n";
    let diags = findings(&[("M0.eventb", root), ("L.eventb", left), ("R.eventb", right)]);
    assert_eq!(
        codes(&diags),
        vec![("EB100", "L.step/act1"), ("EB100", "R.step/act1")],
        "both leaves are reported, in project order"
    );
}
