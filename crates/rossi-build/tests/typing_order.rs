//! EB020: typing predicates are read in source order, as Rodin's
//! `LabeledFormulaModule.checkAndType` does. See `rossi_build::sc::typing`.

mod common;

use rossi_build::po_view::PoView;
use rossi_build::sc_view::ScView;
use rossi_build::{BuildResult, Diagnostic, RuleId, Severity, build};

fn build_texts(sources: &[(&str, &str)]) -> BuildResult {
    build(&common::text_project("p", sources))
}

fn diagnostics_with(result: &BuildResult, rule: RuleId) -> Vec<&Diagnostic> {
    result
        .diagnostics
        .iter()
        .filter(|d| d.rule_id == Some(rule))
        .collect()
}

/// The build's only diagnostic is the EB020 reported at `origin`.
fn assert_only_unknown_type(result: &BuildResult, origin: &str) {
    let unknown = diagnostics_with(result, RuleId::UnknownType);
    assert_eq!(unknown.len(), 1, "diagnostics: {:?}", result.diagnostics);
    assert_eq!(unknown[0].origin, origin);
    assert_eq!(result.diagnostics.len(), 1, "{:?}", result.diagnostics);
}

/// Labels of the checked axioms, invariants and guards in file order.
/// `ScView` keys them by source URI, so it cannot express the order.
fn labels(contents: &str) -> Vec<String> {
    let key = "org.eventb.core.label=\"";
    contents
        .lines()
        .filter(|line| {
            ["scInvariant ", "scAxiom ", "scGuard "]
                .iter()
                .any(|marker| line.contains(marker))
        })
        .filter_map(|line| {
            let start = line.find(key)? + key.len();
            let end = line[start..].find('"')? + start;
            Some(line[start..end].to_string())
        })
        .collect()
}

fn view(result: &BuildResult, name: &str) -> ScView {
    ScView::from_xml(&result.file(name).expect(name).contents).unwrap()
}

#[test]
fn invariant_read_before_its_typing_invariants_is_dropped() {
    let result = build_texts(&[(
        "m.eventb",
        "\
machine m
variables x y
invariants
  @use x ≠ y
  @x_ty x ∈ ℕ
  @y_ty y ∈ ℕ
events
  event INITIALISATION
    then
      @a1 x ≔ 0
      @a2 y ≔ 1
  end
end
",
    )]);
    let bcm = result.file("m.bcm").expect("m.bcm");

    assert_eq!(labels(&bcm.contents), ["x_ty", "y_ty"], "{}", bcm.contents);
    assert!(
        !bcm.accurate,
        "a dropped invariant marks the file inaccurate"
    );

    // The later invariants still type both variables: no untyped-variable
    // error, no EB006, no EB018.
    assert_only_unknown_type(&result, "m.use");
    let unknown = diagnostics_with(&result, RuleId::UnknownType)[0];
    assert_eq!(unknown.severity, Severity::Error);
    assert!(unknown.span.is_some(), "anchored on the first untyped use");
    assert!(
        unknown.message.contains("'x'") && unknown.message.contains("'y'"),
        "names both untyped identifiers: {}",
        unknown.message
    );

    let bpo = PoView::from_xml(&result.file("m.bpo").expect("m.bpo").contents).unwrap();
    assert_eq!(
        bpo.sequents.keys().collect::<Vec<_>>(),
        ["INITIALISATION/x_ty/INV", "INITIALISATION/y_ty/INV"]
    );
}

/// Rodin's own regression (`testInvariantsAndTheorems_06_partialTyping`):
/// `V1=V1` cannot type `V1`, `V1∈S1` then does.
#[test]
fn rodin_partial_typing_case_keeps_the_later_invariants() {
    let result = build_texts(&[
        ("c.eventb", "context c\nsets S1\nend\n"),
        (
            "m.eventb",
            "\
machine m
sees c
variables V1
invariants
  @I1 V1 = V1
  @I2 V1 ∈ S1
  @I3 V1 ∈ {V1}
  @I4 S1 ⊆ {V1}
events
  event INITIALISATION
    then
      @a1 V1 :∈ S1
  end
end
",
        ),
    ]);
    let bcm = result.file("m.bcm").expect("m.bcm");
    assert_eq!(
        labels(&bcm.contents),
        ["I2", "I3", "I4"],
        "{}",
        bcm.contents
    );
    assert_only_unknown_type(&result, "m.I1");
}

/// The context twin (`testAxiomsAndTheorems_06_axiomPartialTyping`).
#[test]
fn axiom_read_before_its_typing_axiom_is_dropped() {
    let result = build_texts(&[(
        "c.eventb",
        "\
context c
sets S1
constants C1
axioms
  @A1 C1 = C1
  @A2 C1 ∈ S1
end
",
    )]);
    let bcc = result.file("c.bcc").expect("c.bcc");
    assert_eq!(labels(&bcc.contents), ["A2"], "{}", bcc.contents);
    assert!(!bcc.accurate);
    assert_eq!(
        view(&result, "c.bcc").constants["C1"].type_str,
        "S1",
        "C1 is still typed by A2"
    );
    assert_only_unknown_type(&result, "c.A1");
}

/// A predicate may type a name from one typed earlier: `x ≠ y` after
/// `x ∈ ℕ` gives `y` the type of `x`, so the later `y ∈ ℕ` only confirms it.
#[test]
fn predicate_types_a_name_from_an_earlier_one() {
    let result = build_texts(&[(
        "m.eventb",
        "\
machine m
variables x y
invariants
  @x_ty x ∈ ℕ
  @use x ≠ y
  @y_ty y ∈ ℕ
events
  event INITIALISATION
    then
      @a1 x ≔ 0
      @a2 y ≔ 1
  end
end
",
    )]);
    let bcm = result.file("m.bcm").expect("m.bcm");
    assert_eq!(
        labels(&bcm.contents),
        ["x_ty", "use", "y_ty"],
        "{}",
        bcm.contents
    );
    assert!(bcm.accurate);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

/// Theorems type too (Rodin `GenericPredicateTest.test_11`).
#[test]
fn theorem_invariant_types_a_variable() {
    let result = build_texts(&[(
        "m.eventb",
        "\
machine m
variables x
invariants
  @t theorem x ∈ ℕ
  @use x ≠ 1
events
  event INITIALISATION
    then
      @a1 x ≔ 0
  end
end
",
    )]);
    let bcm = result.file("m.bcm").expect("m.bcm");
    assert_eq!(labels(&bcm.contents), ["t", "use"], "{}", bcm.contents);
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
}

#[test]
fn guard_read_before_its_typing_guards_is_dropped() {
    let result = build_texts(&[(
        "m.eventb",
        "\
machine m
variables x
invariants
  @x_ty x ∈ ℕ
events
  event INITIALISATION
    then
      @a1 x ≔ 0
  end
  event evt
    any p q
    where
      @g1 p ≠ q
      @g2 p ∈ ℕ
      @g3 q ∈ ℕ
    then
      @a1 x ≔ p + q
  end
end
",
    )]);
    let bcm = result.file("m.bcm").expect("m.bcm");
    assert_eq!(
        labels(&bcm.contents),
        ["x_ty", "g2", "g3"],
        "{}",
        bcm.contents
    );
    let evt = &view(&result, "m.bcm").events["evt"];
    assert_eq!(
        evt.parameters,
        [
            ("p".to_string(), "ℤ".to_string()),
            ("q".to_string(), "ℤ".to_string())
        ]
        .into(),
        "both parameters are typed by the later guards"
    );
    assert!(!evt.accurate, "the event is kept but inaccurate");
    assert_only_unknown_type(&result, "m.evt.g1");
}

/// A genuine type clash after a typing invariant stays EB006, and a name
/// nothing declares stays EB018 whether or not the predicate also reads
/// a variable typed only later: neither is an ordering problem.
#[test]
fn other_type_failures_keep_their_rules() {
    let result = build_texts(&[(
        "m.eventb",
        "\
machine m
variables x y
invariants
  @x_ty x ∈ ℕ
  @bad x = TRUE
  @undeclared x = nothing
  @untyped_and_undeclared y = nothing
  @y_ty y ∈ ℕ
events
  event INITIALISATION
    then
      @a1 x ≔ 0
      @a2 y ≔ 0
  end
end
",
    )]);
    let bcm = result.file("m.bcm").expect("m.bcm");
    assert_eq!(labels(&bcm.contents), ["x_ty", "y_ty"], "{}", bcm.contents);
    assert!(
        diagnostics_with(&result, RuleId::UnknownType).is_empty(),
        "{:?}",
        result.diagnostics
    );
    let type_errors = diagnostics_with(&result, RuleId::TypeError);
    assert_eq!(type_errors.len(), 1, "{:?}", result.diagnostics);
    assert_eq!(type_errors[0].origin, "m.bad");
    assert_eq!(type_errors[0].message, "invariant predicate is ill-typed");
    let undeclared: Vec<&str> = diagnostics_with(&result, RuleId::UndeclaredIdentifier)
        .iter()
        .map(|d| d.origin.as_str())
        .collect();
    assert_eq!(undeclared, ["m.undeclared", "m.untyped_and_undeclared"]);
    assert_eq!(result.diagnostics.len(), 3, "{:?}", result.diagnostics);
}
