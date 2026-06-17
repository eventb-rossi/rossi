//! Parse-time operator-compatibility rejection.
//!
//! The Event-B language requires explicit parentheses when adjacent operators
//! of the same level are incompatible; the Rodin formula parser rejects the
//! bare form with `IncompatibleOperators`. These tests pin rossi's matching
//! rejection. The accept/reject decisions and the exact set-operator pairs are
//! the Rodin formula parser's.

use rossi::{parse_expression_str, parse_predicate_str};

/// Assert an expression is rejected and the message names both operators.
#[track_caller]
fn expr_incompatible(src: &str, left: &str, right: &str) {
    let err = parse_expression_str(src)
        .expect_err(&format!("expected `{src}` to be rejected as incompatible"));
    let msg = err.to_string();
    assert!(
        msg.contains(&format!(
            "Operator: {left} is not compatible with: {right}, parentheses are required"
        )),
        "`{src}` rejected, but message did not name {left}/{right}: {msg}"
    );
}

#[track_caller]
fn expr_ok(src: &str) {
    parse_expression_str(src).unwrap_or_else(|e| panic!("expected `{src}` to parse, got: {e}"));
}

#[track_caller]
fn pred_ok(src: &str) {
    parse_predicate_str(src).unwrap_or_else(|e| panic!("expected `{src}` to parse, got: {e}"));
}

// ---------------------------------------------------------------------------
// Set-operator compatibility matrix
// ---------------------------------------------------------------------------

#[test]
fn union_intersection_mix_is_rejected() {
    expr_incompatible("a ∪ b ∩ c", "∪", "∩");
}

#[test]
fn intersection_union_mix_is_rejected() {
    expr_incompatible("a ∩ b ∪ c", "∩", "∪");
}

#[test]
fn union_difference_mix_is_rejected() {
    expr_incompatible("a ∪ b ∖ c", "∪", "∖");
}

#[test]
fn difference_intersection_mix_is_rejected() {
    expr_incompatible("a ∖ b ∩ c", "∖", "∩");
}

#[test]
fn range_restriction_is_not_self_associative() {
    // `a ▷ b ▷ c` requires parentheses (▷ is not a compatible left operand).
    expr_incompatible("a ▷ b ▷ c", "▷", "▷");
}

#[test]
fn parallel_product_is_not_self_associative() {
    expr_incompatible("a ∥ b ∥ c", "∥", "∥");
}

#[test]
fn forward_composition_then_domain_restriction_is_rejected() {
    // An earlier table wrongly listed `; ◁` as compatible; the oracle
    // rejects it.
    expr_incompatible("a ; b ◁ c", ";", "◁");
}

#[test]
fn self_associative_set_operators_parse() {
    expr_ok("a ∪ b ∪ c");
    expr_ok("a ∩ b ∩ c");
    expr_ok("a × b × c");
    expr_ok("a ; b ; c");
}

#[test]
fn compatible_set_operator_pairs_parse() {
    expr_ok("a ∩ b ∖ c"); // ∩ ∖
    expr_ok("a ∩ b ▷ c"); // ∩ ▷
    expr_ok("a ◁ b ∩ c"); // ◁ ∩
    expr_ok("a ◁ b ⊗ c"); // ◁ ⊗
    expr_ok("a ⩤ b ▷ c"); // ⩤ ▷
}

#[test]
fn longer_compatible_set_chain_parses() {
    // Consecutive pairs (◁,∩) and (∩,▷) are each compatible.
    expr_ok("a ◁ b ∩ c ▷ d");
}

#[test]
fn parenthesising_restores_acceptance() {
    expr_ok("(a ∪ b) ∩ c");
    expr_ok("a ∪ (b ∩ c)");
}

#[test]
fn other_binary_levels_are_unaffected() {
    // Arithmetic mixes freely; maplet is self-associative; relation arrows and
    // range are non-associative by grammar and never reach the set-op gate.
    expr_ok("a + b ∗ c");
    expr_ok("a + b − c");
    expr_ok("a ↦ b ↦ c");
    pred_ok("x ∈ a ‥ b");
}
