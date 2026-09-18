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

/// The origins of an already-filtered finding list.
fn origins<'a>(diags: &[&'a Diagnostic]) -> Vec<&'a str> {
    diags.iter().map(|d| d.origin.as_str()).collect()
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
    let machine = "machine M\nsees C\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
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
    let machine = "machine M\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  then\n    @act1 x :∈ {3}\nend\nend\n";
    assert_eq!(codes(&findings(&[("M.eventb", machine)])), Vec::new());
}

#[test]
fn eb100_stays_silent_on_an_abstract_machine() {
    // The pass checks the machine a translation consumes. `M0` is refined by
    // `M1`, so its `:∈` is an abstraction that `M1` has already resolved.
    let abstract_machine = "machine M0\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  then\n    @act1 x :∈ 0 ‥ 9\nend\nend\n";
    let leaf = "machine M1\nrefines M0\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
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
    let machine = "machine M\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
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
    let machine = "machine M\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  then\n    @act1 x :∣ x' = x + 1\nend\nend\n";
    assert_eq!(codes(&findings(&[("M.eventb", machine)])), Vec::new());
}

#[test]
fn eb101_reports_unverified_branch_conditions_at_info() {
    let machine = "machine M\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
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
    let machine = "machine M\nvariables x\n    y\ninvariants\n    @i1 x ∈ 0 ‥ 9\n    @i2 y ∈ 0 ‥ 9\nevents\n\
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
    let machine = "machine M\nvariables x\n    y\ninvariants\n    @i1 x ∈ 0 ‥ 9\n    @i2 y ∈ 0 ‥ 9\nevents\n\
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
    let machine = "machine M\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
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
    let machine = "machine M\nvariables x\n    y\ninvariants\n    @i1 x ∈ 0 ‥ 9\n    @i2 y ∈ 0 ‥ 9\nevents\n\
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
    let machine = "machine M\nsees C\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ n\nend\nend\n";
    assert_eq!(
        codes(&findings(&[("C.eventb", CTX), ("M.eventb", machine)])),
        Vec::new(),
        "a constant is not machine state, so reading it is fine"
    );
}

#[test]
fn eb102_accepts_a_canonical_such_that_initialisation() {
    let machine = "machine M\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
        event INITIALISATION\n  then\n    @act1 x :∣ x' = 7\nend\nend\n";
    assert_eq!(codes(&findings(&[("M.eventb", machine)])), Vec::new());
}

#[test]
fn eb102_flags_a_non_canonical_such_that_initialisation() {
    let machine = "machine M\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
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
    let abstract_machine = "machine M0\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  then\n    @act1 x :∈ 0 ‥ 9\nend\nend\n";
    let leaf = "machine M1\nrefines M0\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
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
    let root = "machine M0\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\nend\n";
    let left = "machine L\nrefines M0\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  then\n    @act1 x :∈ 0 ‥ 9\nend\nend\n";
    let right = "machine R\nrefines M0\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  then\n    @act1 x :∈ 0 ‥ 9\nend\nend\n";
    let diags = findings(&[("M0.eventb", root), ("L.eventb", left), ("R.eventb", right)]);
    assert_eq!(
        codes(&diags),
        vec![("EB100", "L.step/act1"), ("EB100", "R.step/act1")],
        "both leaves are reported, in project order"
    );
}

// ---------------------------------------------------------------------
// EB103 — event parameter not determined by the guards
// ---------------------------------------------------------------------

#[test]
fn eb103_flags_a_parameter_no_guard_fixes() {
    let machine = "machine M\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  any p\n  where\n    @grd1 p ∈ ℕ\n    @grd2 p > x\n  then\n    @act1 x ≔ p\nend\nend\n";
    let diags = findings(&[("M.eventb", machine)]);
    let undetermined = only(&diags, RuleId::UndeterminedParameter);
    assert_eq!(origins(&undetermined), vec!["M.step/p"]);
    assert_eq!(
        undetermined[0].severity,
        Severity::Info,
        "a trace-driven consumer supplies the parameter, so this is advisory"
    );
    assert!(
        undetermined[0]
            .message
            .contains("supplied outside the model"),
        "an unbounded parameter cannot even be enumerated: {}",
        undetermined[0].message
    );
}

#[test]
fn eb103_accepts_a_determining_equality_guard() {
    let machine = "machine M\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  any p\n  where\n    @grd1 p = x + 1\n  then\n    @act1 x ≔ p\nend\nend\n";
    let diags = findings(&[("M.eventb", machine)]);
    assert!(
        only(&diags, RuleId::UndeterminedParameter).is_empty(),
        "the equality guard fixes the parameter: {diags:#?}"
    );
}

#[test]
fn eb103_says_so_when_the_parameter_is_at_least_enumerable() {
    let context =
        "context C\nsets S\nconstants a\n    b\naxioms\n    @a1 partition(S, {a}, {b})\nend\n";
    let machine = "machine M\nsees C\nvariables x\ninvariants\n    @i1 x ∈ S\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ a\nend\n\
        event step\n  any p\n  where\n    @grd1 p ∈ S\n  then\n    @act1 x ≔ p\nend\nend\n";
    let diags = findings(&[("C.eventb", context), ("M.eventb", machine)]);
    assert_eq!(codes(&diags), vec![("EB103", "M.step/p")]);
    assert!(
        diags[0]
            .message
            .contains("domain is finite and can be enumerated"),
        "a partition makes the carrier set enumerable: {}",
        diags[0].message
    );
}

#[test]
fn eb103_counts_an_interval_guard_as_enumerable() {
    let machine = "machine M\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  any p\n  where\n    @grd1 p ∈ 1 ‥ 10\n  then\n    @act1 x ≔ p\nend\nend\n";
    let diags = findings(&[("M.eventb", machine)]);
    assert_eq!(codes(&diags), vec![("EB103", "M.step/p")]);
    assert!(
        diags[0]
            .message
            .contains("domain is finite and can be enumerated"),
        "an interval bounds an integer parameter however infinite its type: {}",
        diags[0].message
    );
}

// ---------------------------------------------------------------------
// EB104 — quantifier or comprehension without a finite domain
// ---------------------------------------------------------------------

/// The design document's own fixture, reduced: `n` is constrained only by an
/// inequality, so there is nothing to iterate.
#[test]
fn eb104_flags_a_bound_variable_with_only_an_inequality() {
    let context = "context C\nsets NAMES\nconstants here\naxioms\n    @a1 here ∈ NAMES\nend\n";
    let machine = "machine M\nsees C\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  where\n    @grd1 ∀n · n ≠ here ⇒ n ∈ NAMES\n  then\n    @act1 x ≔ 1\nend\nend\n";
    let diags = findings(&[("C.eventb", context), ("M.eventb", machine)]);
    let unbounded = only(&diags, RuleId::UnboundedQuantifier);
    assert_eq!(origins(&unbounded), vec!["M.step/grd1"]);
    assert_eq!(unbounded[0].severity, Severity::Warning);
    assert!(
        unbounded[0].message.contains("`n`"),
        "message names the bound variable: {}",
        unbounded[0].message
    );
}

#[test]
fn eb104_accepts_a_membership_antecedent() {
    let machine = "machine M\nvariables f\ninvariants\n    @i1 f ∈ ℤ ⇸ ℤ\nevents\n\
        event INITIALISATION\n  then\n    @act1 f ≔ ∅\nend\n\
        event step\n  where\n    @grd1 ∀n · n ∈ dom(f) ⇒ f(n) > 0\n  then\n    @act1 f ≔ ∅\nend\nend\n";
    assert_eq!(codes(&findings(&[("M.eventb", machine)])), Vec::new());
}

#[test]
fn eb104_accepts_a_subset_constraint() {
    // `C ⊆ S` is the same statement as `C ∈ ℙ(S)`; reading them differently
    // would report presentation rather than meaning.
    let machine = "machine M\nvariables s\ninvariants\n    @i1 s ∈ ℙ(ℤ)\nevents\n\
        event INITIALISATION\n  then\n    @act1 s ≔ ∅\nend\n\
        event step\n  where\n    @grd1 ∀c · c ⊆ s ⇒ c ⊆ s\n  then\n    @act1 s ≔ ∅\nend\nend\n";
    assert_eq!(codes(&findings(&[("M.eventb", machine)])), Vec::new());
}

#[test]
fn eb104_accepts_a_maplet_membership() {
    // A relation's pairs enumerate both bound variables at once.
    let machine = "machine M\nvariables r\ninvariants\n    @i1 r ∈ ℤ ↔ ℤ\nevents\n\
        event INITIALISATION\n  then\n    @act1 r ≔ ∅\nend\n\
        event step\n  where\n    @grd1 ∃p, q · p ↦ q ∈ r ∧ p < q\n  then\n    @act1 r ≔ ∅\nend\nend\n";
    assert_eq!(codes(&findings(&[("M.eventb", machine)])), Vec::new());
}

#[test]
fn eb104_exempts_a_finite_type() {
    // `BOOL` is its own domain, and so is a partitioned carrier set.
    let context =
        "context C\nsets S\nconstants a\n    b\naxioms\n    @a1 partition(S, {a}, {b})\nend\n";
    let machine = "machine M\nsees C\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  where\n    @grd1 (∀v · v ≠ a ⇒ v ∈ S) ∧ (∀w · w = TRUE ⇒ w = TRUE)\n  then\n    @act1 x ≔ 1\nend\nend\n";
    assert_eq!(
        codes(&findings(&[("C.eventb", context), ("M.eventb", machine)])),
        Vec::new()
    );
}

#[test]
fn eb104_covers_a_set_comprehension() {
    let machine = "machine M\nvariables s\ninvariants\n    @i1 s ∈ ℙ(ℤ)\nevents\n\
        event INITIALISATION\n  then\n    @act1 s ≔ ∅\nend\n\
        event step\n  then\n    @act1 s ≔ {n · n > 0 ∣ n}\nend\nend\n";
    let diags = findings(&[("M.eventb", machine)]);
    assert_eq!(codes(&diags), vec![("EB104", "M.step/act1")]);
}

// ---------------------------------------------------------------------
// EB105 — infinite set used as a value
// ---------------------------------------------------------------------

#[test]
fn eb105_flags_cardinality_of_an_infinite_set() {
    let machine = "machine M\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  then\n    @act1 x ≔ card(ℕ)\nend\nend\n";
    let diags = findings(&[("M.eventb", machine)]);
    assert_eq!(codes(&diags), vec![("EB105", "M.step/act1")]);
    assert_eq!(diags[0].severity, Severity::Warning);
    assert!(
        diags[0].message.contains("`ℕ`"),
        "message names the set: {}",
        diags[0].message
    );
}

#[test]
fn eb105_exempts_a_typing_position() {
    // `f ∈ ℕ → ℤ` states a type; nothing has to be built.
    let machine = "machine M\nvariables f\ninvariants\n    @i1 f ∈ ℕ → ℤ\nevents\n\
        event INITIALISATION\n  then\n    @act1 f ≔ (λn · n ∈ ℕ ∣ n)\nend\nend\n";
    assert_eq!(codes(&findings(&[("M.eventb", machine)])), Vec::new());
}

#[test]
fn eb105_flags_a_relation_space_that_must_be_built() {
    let machine = "machine M\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  then\n    @act1 x ≔ card(ℤ ↔ ℤ)\nend\nend\n";
    let diags = findings(&[("M.eventb", machine)]);
    assert_eq!(codes(&diags), vec![("EB105", "M.step/act1")]);
    assert!(
        diags[0].message.contains("relation or function space"),
        "message names the shape: {}",
        diags[0].message
    );
}

#[test]
fn eb105_flags_a_union_with_an_infinite_operand() {
    let machine = "machine M\nvariables s\ninvariants\n    @i1 s ∈ ℙ(ℤ)\nevents\n\
        event INITIALISATION\n  then\n    @act1 s ≔ ∅\nend\n\
        event step\n  then\n    @act1 s ≔ ℕ ∪ {1}\nend\nend\n";
    let diags = findings(&[("M.eventb", machine)]);
    assert_eq!(codes(&diags), vec![("EB105", "M.step/act1")]);
}

#[test]
fn eb105_accepts_an_intersection_with_one_enumerable_operand() {
    // An intersection is walked from the finite operand and each element
    // tested against the other, so `ℕ` is never built.
    let machine = "machine M\nvariables s\n    t\ninvariants\n    @i1 s ∈ ℙ(ℤ)\n    @i2 t ∈ ℙ(ℤ)\nevents\n\
        event INITIALISATION\n  then\n    @act1 s ≔ ∅\n    @act2 t ≔ ∅\nend\n\
        event step\n  then\n    @act1 s ≔ t ∩ ℕ\nend\nend\n";
    assert!(
        only(
            &findings(&[("M.eventb", machine)]),
            RuleId::InfiniteSetValue
        )
        .is_empty(),
        "one enumerable operand bounds the intersection"
    );
}

#[test]
fn eb105_flags_an_intersection_of_infinite_operands() {
    let machine = "machine M\nvariables s\ninvariants\n    @i1 s ∈ ℙ(ℤ)\nevents\n\
        event INITIALISATION\n  then\n    @act1 s ≔ ∅\nend\n\
        event step\n  then\n    @act1 s ≔ ℕ ∩ ℕ1\nend\nend\n";
    let diags = findings(&[("M.eventb", machine)]);
    assert_eq!(codes(&diags), vec![("EB105", "M.step/act1")]);
}

// ---------------------------------------------------------------------
// EB106 — deferred set with no cardinality
// ---------------------------------------------------------------------

const UNBOUNDED_SET: &str = "context C\nsets S\nconstants a\naxioms\n    @a1 a ∈ S\nend\n";

#[test]
fn eb106_flags_a_carrier_set_used_as_a_value() {
    let machine = "machine M\nsees C\nvariables x\ninvariants\n    @i1 x ∈ S\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ a\nend\n\
        event step\n  any p\n  where\n    @grd1 p ∈ S ∖ {a}\n  then\n    @act1 x ≔ p\nend\nend\n";
    let diags = findings(&[("C.eventb", UNBOUNDED_SET), ("M.eventb", machine)]);
    let set = only(&diags, RuleId::DeferredSetWithoutCardinality);
    assert_eq!(set.len(), 1, "one finding per set, at its declaration");
    assert_eq!(set[0].origin, "C.S");
    assert_eq!(set[0].severity, Severity::Warning);
    assert!(
        set[0].message.contains("M.step/grd1") && set[0].message.contains("partition"),
        "message names the first use and the evidence that would settle it: {}",
        set[0].message
    );
}

#[test]
fn eb106_is_silent_when_membership_is_all_that_is_asked() {
    // Testing membership never enumerates the set.
    let machine = "machine M\nsees C\nvariables x\ninvariants\n    @i1 x ∈ S\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ a\nend\n\
        event step\n  any p\n  where\n    @grd1 p ∈ S\n  then\n    @act1 x ≔ p\nend\nend\n";
    let diags = findings(&[("C.eventb", UNBOUNDED_SET), ("M.eventb", machine)]);
    assert!(
        only(&diags, RuleId::DeferredSetWithoutCardinality).is_empty(),
        "a typing membership is not a value use: {diags:#?}"
    );
}

#[test]
fn eb106_rejects_a_partition_into_unlisted_blocks() {
    // `partition(S, A, B)` splits `S` in two without bounding either half,
    // so it says nothing about how many elements `S` has.
    let context = "context C\nsets S\nconstants a\n    A\n    B\naxioms\n    @a1 a ∈ S\n    @a2 partition(S, A, B)\nend\n";
    let machine = "machine M\nsees C\nvariables x\ninvariants\n    @i1 x ∈ S\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ a\nend\n\
        event step\n  any p\n  where\n    @grd1 p ∈ S ∖ {a}\n  then\n    @act1 x ≔ p\nend\nend\n";
    let diags = findings(&[("C.eventb", context), ("M.eventb", machine)]);
    let set = only(&diags, RuleId::DeferredSetWithoutCardinality);
    assert_eq!(set.len(), 1, "one finding, at the declaration: {diags:#?}");
    assert_eq!(set[0].origin, "C.S");
}

#[test]
fn eb106_accepts_each_way_of_bounding_a_set() {
    for axioms in [
        "@a1 a ∈ S\n    @a2 finite(S)",
        "@a1 partition(S, {a})",
        "@a1 S = {a}",
        "@a1 a ∈ S\n    @a2 card(S) = 4",
    ] {
        let context = format!("context C\nsets S\nconstants a\naxioms\n    {axioms}\nend\n");
        let machine = "machine M\nsees C\nvariables x\ninvariants\n    @i1 x ∈ S\nevents\n\
            event INITIALISATION\n  then\n    @act1 x ≔ a\nend\n\
            event step\n  any p\n  where\n    @grd1 p ∈ S ∖ {a}\n  then\n    @act1 x ≔ p\nend\nend\n";
        let diags = findings(&[("C.eventb", &context), ("M.eventb", machine)]);
        assert!(
            only(&diags, RuleId::DeferredSetWithoutCardinality).is_empty(),
            "`{axioms}` bounds the set: {diags:#?}"
        );
    }
}

// ---------------------------------------------------------------------
// EB107 — unbounded integer
// ---------------------------------------------------------------------

#[test]
fn eb107_flags_an_integer_with_only_a_typing_constraint() {
    let context = "context C\nconstants n\naxioms\n    @a1 n ∈ ℕ\nend\n";
    let diags = findings(&[("C.eventb", context)]);
    let ints = only(&diags, RuleId::UnboundedInteger);
    assert_eq!(origins(&ints), vec!["C.n"]);
    assert_eq!(ints[0].severity, Severity::Info);
    assert!(
        ints[0].message.contains("width"),
        "message says what a translation has to invent: {}",
        ints[0].message
    );
}

#[test]
fn eb107_accepts_any_real_bound() {
    for axioms in ["@a1 n ∈ 0 ‥ 20", "@a1 n ∈ ℕ\n    @a2 n ≤ 20", "@a1 n = 20"] {
        let context = format!("context C\nconstants n\naxioms\n    {axioms}\nend\n");
        let diags = findings(&[("C.eventb", &context)]);
        assert!(
            only(&diags, RuleId::UnboundedInteger).is_empty(),
            "`{axioms}` bounds the constant: {diags:#?}"
        );
    }
}

#[test]
fn eb107_reads_an_inherited_invariant() {
    // A refinement does not restate an abstract variable's constraints, so
    // the bound one level up still counts.
    let root = "machine M0\nvariables x\ninvariants\n    @i1 x ∈ 0 ‥ 9\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\nend\n";
    let leaf = "machine M1\nrefines M0\nvariables x\nevents\n\
        event INITIALISATION extends INITIALISATION\nend\nend\n";
    let diags = findings(&[("M0.eventb", root), ("M1.eventb", leaf)]);
    assert!(
        only(&diags, RuleId::UnboundedInteger).is_empty(),
        "the inherited invariant bounds `x`: {diags:#?}"
    );
}

#[test]
fn eb107_covers_variables_and_parameters() {
    let machine = "machine M\nvariables x\ninvariants\n    @i1 x ∈ ℤ\nevents\n\
        event INITIALISATION\n  then\n    @act1 x ≔ 0\nend\n\
        event step\n  any p\n  where\n    @grd1 p ∈ ℤ\n  then\n    @act1 x ≔ p\nend\nend\n";
    let diags = findings(&[("M.eventb", machine)]);
    assert_eq!(
        origins(&only(&diags, RuleId::UnboundedInteger)),
        vec!["M.x", "M.step/p"]
    );
}

// ---------------------------------------------------------------------
// EB108 — constant not determined by an equality
// ---------------------------------------------------------------------

#[test]
fn eb108_flags_a_constant_that_is_only_constrained() {
    // The design document's worked example: a bound, a type and a
    // membership determine nothing.
    let context = "context C\nconstants n\n    f\n    v\naxioms\n\
        @a1 n ∈ ℕ\n    @a2 f ∈ (0 ‥ n − 1) → ℤ\n    @a3 v ∈ ran(f)\nend\n";
    let diags = findings(&[("C.eventb", context)]);
    assert_eq!(
        origins(&only(&diags, RuleId::UndeterminedConstant)),
        vec!["C.f", "C.n", "C.v"]
    );
}

#[test]
fn eb108_accepts_a_chain_of_definitions() {
    // A defining expression may name other constants as long as they are
    // themselves determined.
    let context = "context C\nconstants width\n    height\n    area\naxioms\n\
        @a1 width = 4\n    @a2 height = width + 1\n    @a3 area = width ∗ height\nend\n";
    let diags = findings(&[("C.eventb", context)]);
    assert!(
        only(&diags, RuleId::UndeterminedConstant).is_empty(),
        "every definition resolves: {diags:#?}"
    );
}

#[test]
fn eb108_reports_a_definition_cycle_by_what_it_waits_on() {
    let context = "context C\nconstants a\n    b\naxioms\n\
        @a1 a ∈ ℤ\n    @a2 b ∈ ℤ\n    @a3 a = b + 1\n    @a4 b = a − 1\nend\n";
    let diags = findings(&[("C.eventb", context)]);
    let undetermined = only(&diags, RuleId::UndeterminedConstant);
    assert_eq!(undetermined.len(), 2, "a cycle settles neither constant");
    assert!(
        undetermined[0].message.contains("waits on"),
        "a cycle is reported as an unmet dependency, not as a missing axiom: {}",
        undetermined[0].message
    );
}

#[test]
fn eb108_accepts_a_maplet_equality() {
    let context = "context C\nconstants pair\n    left\n    right\naxioms\n\
        @a1 pair = 2 ↦ TRUE\n    @a2 left ↦ right = 3 ↦ FALSE\nend\n";
    let diags = findings(&[("C.eventb", context)]);
    assert!(
        only(&diags, RuleId::UndeterminedConstant).is_empty(),
        "a maplet equality is a pair of equalities: {diags:#?}"
    );
}

#[test]
fn eb108_accepts_partition_and_enumeration_alike() {
    for axioms in [
        "@a1 partition(S, {a}, {b})",
        "@a1 S = {a, b}\n    @a2 a ≠ b",
    ] {
        let context = format!("context C\nsets S\nconstants a\n    b\naxioms\n    {axioms}\nend\n");
        let diags = findings(&[("C.eventb", &context)]);
        assert!(
            only(&diags, RuleId::UndeterminedConstant).is_empty(),
            "`{axioms}` names both constants: {diags:#?}"
        );
    }
}

#[test]
fn eb108_accepts_finite_plus_card_for_a_set_constant() {
    let context = "context C\nsets S\nconstants block\naxioms\n\
        @a1 block ⊆ S\n    @a2 finite(block)\n    @a3 card(block) = 4\nend\n";
    let diags = findings(&[("C.eventb", context)]);
    assert!(
        only(&diags, RuleId::UndeterminedConstant).is_empty(),
        "a size is enough for a set whose members are not named: {diags:#?}"
    );
}

// ---------------------------------------------------------------------
// EB109 — carrier-set objects not mutually distinguished
// ---------------------------------------------------------------------

#[test]
fn eb109_flags_two_constants_nothing_relates() {
    let context =
        "context C\nsets S\nconstants a\n    b\naxioms\n    @a1 a ∈ S\n    @a2 b ∈ S\nend\n";
    let diags = findings(&[("C.eventb", context)]);
    let pairs = only(&diags, RuleId::IndistinctCarrierSetConstants);
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].origin, "C.a");
    assert_eq!(pairs[0].severity, Severity::Info);
    assert!(
        pairs[0].message.contains("`a`") && pairs[0].message.contains("`b`"),
        "message names both constants: {}",
        pairs[0].message
    );
}

#[test]
fn eb109_accepts_an_inequality_or_a_partition() {
    for axioms in [
        "@a1 a ∈ S\n    @a2 b ∈ S\n    @a3 a ≠ b",
        "@a1 partition(S, {a}, {b})",
    ] {
        let context = format!("context C\nsets S\nconstants a\n    b\naxioms\n    {axioms}\nend\n");
        let diags = findings(&[("C.eventb", &context)]);
        assert!(
            only(&diags, RuleId::IndistinctCarrierSetConstants).is_empty(),
            "`{axioms}` relates the pair: {diags:#?}"
        );
    }
}

#[test]
fn eb109_reports_each_unrelated_pair() {
    // Three constants, one relation: the other two pairs stay open.
    let context = "context C\nsets S\nconstants a\n    b\n    c\naxioms\n\
        @a1 a ∈ S\n    @a2 b ∈ S\n    @a3 c ∈ S\n    @a4 a ≠ b\nend\n";
    let diags = findings(&[("C.eventb", context)]);
    assert_eq!(only(&diags, RuleId::IndistinctCarrierSetConstants).len(), 2);
}
