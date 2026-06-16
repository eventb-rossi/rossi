//! Group S: a context axiom of the shape
//! `c = (λ x · x = ∅ ∣ 0) ∪ (λ x⦂T · …)` must type the first
//! lambda's binder by lifting the function type from the typed
//! sibling across the `∪`. Rodin parity — verified against a
//! real-world corpus context whose constant is
//! `ℙ(ℙ(ℤ×ℤ)×ℤ)` and both lambdas end up with `x⦂ℙ(ℤ×ℤ)` binders.

use rossi_build::{Project, ProjectComponent, build, sc_view::ScView};

const CTX_BUC: &str = r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.constant name="_c1" org.eventb.core.identifier="integral"/>
<org.eventb.core.axiom name="_a1" org.eventb.core.label="f_integral" org.eventb.core.predicate="integral = (λ x · x = ∅ ∣ 0) ∪ (λ x⦂ℙ(ℤ×ℤ) · x ∈ ℤ ⇸ ℤ ∣ 1)"/>
</org.eventb.core.contextFile>
"#;

fn project() -> Project {
    Project::new(
        "s",
        vec![ProjectComponent::from_xml("Ctx.buc", CTX_BUC).unwrap()],
    )
}

#[test]
fn lambda_binder_typed_via_typed_sibling_across_union() {
    let r = build(&project());
    let bcc = r.file("Ctx.bcc").expect("Ctx.bcc");
    assert!(
        bcc.accurate,
        "context file should be accurate; diagnostics: {:?}",
        r.diagnostics
    );
    let v = ScView::from_xml(&bcc.contents).unwrap();
    let axiom = v
        .axioms
        .values()
        .find(|a| a.label == "f_integral")
        .expect("f_integral axiom present");

    // Walk the parsed predicate AST to find both lambdas inside the
    // union — both binders should now be typed.
    use rossi::{ExpressionKind, IdentPattern, PredicateKind, ast::predicate::ComparisonOp};
    let PredicateKind::Comparison {
        op: ComparisonOp::Equal,
        right,
        ..
    } = axiom.predicate.clone().kind
    else {
        panic!(
            "expected `integral = …` Comparison; got {:?}",
            axiom.predicate
        );
    };
    let ExpressionKind::Binary {
        op: rossi::ast::expression::BinaryOp::Union,
        left,
        right,
    } = right.kind
    else {
        panic!("expected union on RHS")
    };
    for (label, lambda) in [("first", *left), ("second", *right)] {
        let ExpressionKind::Lambda { pattern, .. } = lambda.kind else {
            panic!("{label} operand should be a Lambda; got {lambda:?}")
        };
        let IdentPattern::Identifier(ti) = pattern else {
            panic!("{label} lambda should have a single-identifier pattern")
        };
        assert!(
            ti.type_expr.is_some(),
            "{label} lambda's binder should be typed after enrich: {ti:?}"
        );
    }
}
