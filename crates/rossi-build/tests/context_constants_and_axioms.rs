//! End-to-end test: binary-search/C0.buc has constants `n`, `f`, `v` and
//! four axioms. We should emit a .bcc whose axioms and constants are
//! semantically equivalent to Rodin's, with inferred types for each
//! constant.

use rossi_build::{Project, ProjectComponent, build};

const C0_BUC: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.contextFile org.eventb.core.configuration="org.eventb.core.fwd" version="3">
    <org.eventb.core.constant name="'" org.eventb.core.identifier="n"/>
    <org.eventb.core.constant name="*" org.eventb.core.identifier="f"/>
    <org.eventb.core.constant name="(" org.eventb.core.identifier="v"/>
    <org.eventb.core.axiom name=")" org.eventb.core.label="axm1" org.eventb.core.predicate="n ∈ ℕ"/>
    <org.eventb.core.axiom name="+" org.eventb.core.label="axm2" org.eventb.core.predicate="f ∈ (0 ‥ n − 1) → ℤ"/>
    <org.eventb.core.axiom name="," org.eventb.core.label="axm3" org.eventb.core.predicate="v ∈ ran(f)"/>
    <org.eventb.core.axiom name="-" org.eventb.core.label="axm4" org.eventb.core.predicate="∀x, y · x ∈ dom(f) ∧ y ∈ dom(f) ∧ x ≤ y ⇒ f(x) ≤ f(y)"/>
</org.eventb.core.contextFile>
"#;

fn make_project() -> Project {
    let pc = ProjectComponent::from_xml("C0.buc", C0_BUC).unwrap();
    Project::new("binary-search", vec![pc])
}

#[test]
fn emits_a_bcc_file() {
    let result = build(&make_project());
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].filename, "C0.bcc");
}

#[test]
fn integer_constant_gets_integer_type() {
    let result = build(&make_project());
    let xml = &result.files[0].contents;
    // n : ℤ (inferred from n ∈ ℕ, since ℕ : ℙ(ℤ))
    assert!(
        xml.contains(r#"<org.eventb.core.scConstant name="n""#)
            && xml.contains(r#"org.eventb.core.type="ℤ""#),
        "n should get type ℤ, got:\n{xml}"
    );
}

#[test]
fn integer_relation_constant_gets_pow_int_times_int() {
    let result = build(&make_project());
    let xml = &result.files[0].contents;
    // f : ℙ(ℤ×ℤ) (inferred from f ∈ (0‥n−1) → ℤ)
    assert!(
        xml.contains(r#"name="f""#) && xml.contains("ℙ(ℤ×ℤ)"),
        "f should get type ℙ(ℤ×ℤ), got:\n{xml}"
    );
}

#[test]
fn constants_are_sorted_alphabetically() {
    let result = build(&make_project());
    let xml = &result.files[0].contents;
    let idx_f = xml.find(r#"<org.eventb.core.scConstant name="f""#).unwrap();
    let idx_n = xml.find(r#"<org.eventb.core.scConstant name="n""#).unwrap();
    let idx_v = xml.find(r#"<org.eventb.core.scConstant name="v""#).unwrap();
    assert!(idx_f < idx_n && idx_n < idx_v);
}

#[test]
fn axioms_appear_before_constants() {
    let result = build(&make_project());
    let xml = &result.files[0].contents;
    let first_axiom = xml.find("<org.eventb.core.scAxiom").unwrap();
    let first_constant = xml.find("<org.eventb.core.scConstant").unwrap();
    assert!(
        first_axiom < first_constant,
        "expected scAxiom elements before scConstant elements"
    );
}

#[test]
fn predicates_are_canonical_unicode() {
    let result = build(&make_project());
    let xml = &result.files[0].contents;
    // Simple membership + function-application axioms: byte-exact with Rodin.
    for expected in [
        r#"org.eventb.core.predicate="n∈ℕ""#,
        r#"org.eventb.core.predicate="v∈ran(f)""#,
    ] {
        assert!(
            xml.contains(expected),
            "expected {expected} in output:\n{xml}"
        );
    }
    // Quantified axm4 — binder type ascriptions (`⦂ℤ`) are now stamped
    // on by the enrich pass, matching Rodin byte-for-byte.
    assert!(
        xml.contains(r#"∀x⦂ℤ,y⦂ℤ·x∈dom(f)∧y∈dom(f)∧x≤y⇒f(x)≤f(y)"#),
        "axm4 differs from Rodin in unexpected way:\n{xml}"
    );
}

#[test]
fn predicates_are_semantically_equivalent_to_inputs() {
    // Round-trip: our emitted predicate strings parse back into the
    // expected AST. axm4's source binders were untyped (`∀x, y · …`);
    // the SC now enriches them with their inferred types (`∀x⦂ℤ, y⦂ℤ
    // · …`) to match Rodin, so the expected AST mirrors that.
    use rossi::parse_predicate_str;

    let result = build(&make_project());
    let xml = &result.files[0].contents;

    // Parse our emitted predicates from the XML.
    let inputs = [
        "n ∈ ℕ",
        "f ∈ (0 ‥ n − 1) → ℤ",
        "v ∈ ran(f)",
        "∀x⦂ℤ, y⦂ℤ · x ∈ dom(f) ∧ y ∈ dom(f) ∧ x ≤ y ⇒ f(x) ≤ f(y)",
    ];
    for (i, input) in inputs.iter().enumerate() {
        let label = format!("axm{}", i + 1);
        // Pull our emitted predicate back out by a coarse XML search.
        let marker = format!("org.eventb.core.label=\"{label}\" org.eventb.core.predicate=\"");
        let start = xml.find(&marker).unwrap() + marker.len();
        let end = start + xml[start..].find('"').unwrap();
        let ours = &xml[start..end];

        let expected = parse_predicate_str(input).unwrap();
        let ours_ast = parse_predicate_str(ours)
            .unwrap_or_else(|e| panic!("our predicate {ours:?} did not re-parse: {e}"));
        assert_eq!(
            ours_ast,
            expected,
            "axm{} differs semantically:\n ours:     {ours}\n expected: {input}",
            i + 1
        );
    }
}

#[test]
fn accurate_is_true_when_all_constants_are_inferred() {
    let result = build(&make_project());
    let xml = &result.files[0].contents;
    assert!(xml.contains("org.eventb.core.accurate=\"true\""));
    assert!(result.files[0].accurate);
    assert!(result.is_ok(), "diagnostics: {:?}", result.diagnostics);
}
