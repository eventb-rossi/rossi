//! Source-span coverage for expression / predicate AST nodes.
//!
//! The parser records a byte span on every expression and predicate node it
//! builds (issue #68). These tests pin the spans of identifier leaves and a few
//! structural nodes so navigation features can rely on them.

use rossi::ast::{ExpressionKind, PredicateKind, Span};
use rossi::{parse_expression_str, parse_predicate_str};

/// The source slice a span points at.
fn slice(src: &str, span: Span) -> &str {
    &src[span.start..span.end]
}

#[test]
fn comparison_identifier_leaves_are_spanned() {
    let src = "x ∈ S";
    let pred = parse_predicate_str(src).expect("parses");
    let PredicateKind::Comparison { left, right, .. } = &pred.kind else {
        panic!("expected comparison, got {:?}", pred.kind);
    };
    assert_eq!(slice(src, left.span.expect("left span")), "x");
    assert_eq!(slice(src, right.span.expect("right span")), "S");
    // The whole predicate spans the entire comparison.
    assert_eq!(slice(src, pred.span.expect("pred span")), "x ∈ S");
}

#[test]
fn nested_identifier_usages_point_at_each_occurrence() {
    // Two uses of `count` at distinct offsets must carry distinct spans.
    let src = "count = count + 1";
    let pred = parse_predicate_str(src).expect("parses");
    let PredicateKind::Comparison { left, right, .. } = &pred.kind else {
        panic!("expected comparison");
    };
    assert_eq!(slice(src, left.span.unwrap()), "count");
    assert_eq!(left.span.unwrap().start, 0);

    // right is `count + 1`; its left operand is the second `count`.
    let ExpressionKind::Binary { left: inner, .. } = &right.kind else {
        panic!("expected binary on the right");
    };
    assert_eq!(slice(src, inner.span.unwrap()), "count");
    assert_eq!(inner.span.unwrap().start, 8);
}

#[test]
fn binary_fold_span_covers_both_operands() {
    let src = "a + b + c";
    let expr = parse_expression_str(src).expect("parses");
    // Left-associative: ((a + b) + c). The outer node spans the whole input.
    let ExpressionKind::Binary { left, right, .. } = &expr.kind else {
        panic!("expected binary");
    };
    assert_eq!(slice(src, expr.span.unwrap()), "a + b + c");
    assert_eq!(slice(src, left.span.unwrap()), "a + b");
    assert_eq!(slice(src, right.span.unwrap()), "c");
}

#[test]
fn function_application_identifier_is_spanned() {
    let src = "f(x)";
    let expr = parse_expression_str(src).expect("parses");
    let ExpressionKind::FunctionApplication {
        function,
        arguments,
    } = &expr.kind
    else {
        panic!("expected function application");
    };
    assert_eq!(slice(src, function.span.unwrap()), "f");
    assert_eq!(slice(src, arguments[0].span.unwrap()), "x");
}

#[test]
fn quantifier_binder_is_spanned() {
    let src = "∀ x · x ∈ S";
    let pred = parse_predicate_str(src).expect("parses");
    let PredicateKind::Quantified { identifiers, .. } = &pred.kind else {
        panic!("expected quantified");
    };
    // The binder declaration `x` (after the ∀) carries its own span.
    let binder = &identifiers[0];
    assert_eq!(slice(src, binder.span.expect("binder span")), "x");
    // ∀ is 3 bytes + space, so the binder starts at byte 4.
    assert_eq!(binder.span.unwrap().start, 4);
}

#[test]
fn lambda_pattern_binders_are_spanned() {
    use rossi::ast::IdentPattern;
    let src = "λ x ↦ y · x ∈ ℤ ∧ y ∈ ℤ ∣ x";
    let expr = parse_expression_str(src).expect("parses");
    let ExpressionKind::Lambda { pattern, .. } = &expr.kind else {
        panic!("expected lambda");
    };
    let IdentPattern::Maplet(l, r) = pattern else {
        panic!("expected maplet pattern");
    };
    let (IdentPattern::Identifier(lx), IdentPattern::Identifier(ry)) = (l.as_ref(), r.as_ref())
    else {
        panic!("expected identifier leaves");
    };
    assert_eq!(slice(src, lx.span.expect("x binder span")), "x");
    assert_eq!(slice(src, ry.span.expect("y binder span")), "y");
}

#[test]
fn quantified_body_usage_is_spanned() {
    let src = "∀ x · x ∈ S";
    let pred = parse_predicate_str(src).expect("parses");
    let PredicateKind::Quantified { predicate, .. } = &pred.kind else {
        panic!("expected quantified");
    };
    let PredicateKind::Comparison { left, .. } = &predicate.kind else {
        panic!("expected comparison body");
    };
    // The bound usage `x` in the body points at the second `x`, not the binder.
    assert_eq!(slice(src, left.span.unwrap()), "x");
    assert!(left.span.unwrap().start > 0);
}
