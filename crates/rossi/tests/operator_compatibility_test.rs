//! Parse-time operator-compatibility rejection.
//!
//! The Event-B language requires explicit parentheses when adjacent operators
//! of the same level are incompatible; the Rodin formula parser rejects the
//! bare form with `IncompatibleOperators`. These tests pin rossi's matching
//! rejection. The accept/reject decisions and the exact set-operator pairs are
//! the Rodin formula parser's.

use rossi::{parse_expression_str, parse_predicate_str};

/// Assert a parse `result` is the incompatible-operators rejection naming both
/// operators. Generic over the Ok type so the expression and predicate entry
/// points share one body.
#[track_caller]
fn assert_incompatible<T: std::fmt::Debug, E: std::fmt::Display>(
    result: Result<T, E>,
    src: &str,
    left: &str,
    right: &str,
) {
    let err = result.expect_err(&format!("expected `{src}` to be rejected as incompatible"));
    let msg = err.to_string();
    assert!(
        msg.contains(&format!(
            "Operator: {left} is not compatible with: {right}, parentheses are required"
        )),
        "`{src}` rejected, but message did not name {left}/{right}: {msg}"
    );
}

#[track_caller]
fn expr_incompatible(src: &str, left: &str, right: &str) {
    assert_incompatible(parse_expression_str(src), src, left, right);
}

#[track_caller]
fn pred_incompatible(src: &str, left: &str, right: &str) {
    assert_incompatible(parse_predicate_str(src), src, left, right);
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

// ---------------------------------------------------------------------------
// ∧ / ∨ may not be mixed without parentheses
// ---------------------------------------------------------------------------

#[test]
fn and_then_or_is_rejected() {
    pred_incompatible("x > 0 ∧ y > 0 ∨ z > 0", "∧", "∨");
}

#[test]
fn or_then_and_is_rejected() {
    pred_incompatible("x > 0 ∨ y > 0 ∧ z > 0", "∨", "∧");
}

#[test]
fn mixing_fires_before_a_trailing_quantifier_operand() {
    // Reached at the operator, before its right operand is parsed.
    pred_incompatible("x > 0 ∧ y > 0 ∨ ∃ w · w > 0", "∧", "∨");
}

#[test]
fn same_connective_chains_parse() {
    pred_ok("x > 0 ∧ y > 0 ∧ z > 0");
    pred_ok("x > 0 ∨ y > 0 ∨ z > 0");
}

#[test]
fn parenthesised_connectives_parse() {
    pred_ok("(x > 0 ∧ y > 0) ∨ z > 0");
    pred_ok("x > 0 ∧ (y > 0 ∨ z > 0)");
}

#[test]
fn connectives_below_implication_keep_their_precedence() {
    // ∧/∨ bind tighter than ⇒, so these need no parentheses.
    pred_ok("x > 0 ⇒ y > 0 ∨ z > 0");
    pred_ok("x > 0 ∧ y > 0 ⇒ z > 0");
}

// ---------------------------------------------------------------------------
// A bare quantifier may not be a ∧ / ∨ operand
// ---------------------------------------------------------------------------

#[test]
fn bare_exists_as_conjunct_is_rejected() {
    pred_incompatible("x > 0 ∧ ∃ w · w > 0", "∧", "∃");
}

#[test]
fn bare_forall_as_disjunct_is_rejected() {
    pred_incompatible("x > 0 ∨ ∀ w · w > 0", "∨", "∀");
}

#[test]
fn parenthesised_quantifier_operand_parses() {
    pred_ok("x > 0 ∧ (∃ w · w > 0)");
    pred_ok("x > 0 ∨ (∀ w · w > 0)");
}

#[test]
fn negated_quantifier_operand_parses() {
    // `¬` wraps the quantifier, so the operand's leading token is `¬`, not `∃`.
    pred_ok("x > 0 ∧ ¬(∃ w · w > 0)");
}

#[test]
fn leading_quantifier_absorbs_the_connective_body() {
    // `∀w·w>0 ∧ x>0` is `∀w·(w>0 ∧ x>0)`: the quantifier body extends maximally,
    // so the conjunction is inside it, not the other way around.
    pred_ok("∀ w · w > 0 ∧ x > 0");
    pred_ok("∃ w · w > 0 ∨ x > 0");
}

// A trailing bare quantifier under ∧/∨ is permitted when a closing bracket
// bounds it (the bracket stands in for the required parentheses). The rule
// propagates into quantifier bodies but resets at ∣-bounded such-that clauses.

#[test]
fn quantifier_conjunct_bounded_by_a_bracket_parses() {
    pred_ok("(x > 0 ∧ ∃ w · w > 0)"); // parentheses
    pred_ok("(x > 0 ∨ ∃ w · w > 0)"); // disjunction, parenthesised
    pred_ok("x > 1 ⇒ (x > 0 ∧ ∃ w · w > 0)"); // parenthesised right operand
    pred_ok("z ∈ {x ∣ x > 0 ∧ ∃ w · w > 0}"); // simple comprehension
    pred_ok("z ∈ {x ∣ x > 0 ∧ ∃ w · w > 0 ∧ x > 1}"); // quantifier mid-chain (absorbs)
    pred_ok("(∀ x · x > 0 ∧ ∃ w · w > 0)"); // propagates into the ∀ body
}

#[test]
fn quantifier_conjunct_not_bounded_by_a_bracket_is_rejected() {
    // Top level, a ∀ body, and ∣-bounded such-that clauses are not bracketed.
    pred_incompatible("x > 0 ∧ ∃ w · w > 0", "∧", "∃");
    pred_incompatible("∀ x · x > 0 ∧ ∃ w · w > 0", "∧", "∃");
    expr_incompatible("{x · x > 0 ∧ ∃ w · w > 0 ∣ x}", "∧", "∃"); // explicit comprehension
    expr_incompatible("(λ x · x > 0 ∧ ∃ w · w > 0 ∣ x)", "∧", "∃"); // lambda such-that
    expr_incompatible("⋃ z · z > 0 ∧ ∃ w · w > 0 ∣ {z}", "∧", "∃"); // ⋃ such-that
}

// ---------------------------------------------------------------------------
// ⇒ / ⇔ may neither be chained nor mixed without parentheses
// ---------------------------------------------------------------------------
//
// Each is a non-associative singleton, and the two are mutually incompatible —
// so every adjacent pair of them needs explicit parentheses, unlike the ∧/∨
// level where same-operator chains are fine.

#[test]
fn implication_chain_is_rejected() {
    pred_incompatible("x > 0 ⇒ y > 0 ⇒ z > 0", "⇒", "⇒");
}

#[test]
fn equivalence_chain_is_rejected() {
    pred_incompatible("x > 0 ⇔ y > 0 ⇔ z > 0", "⇔", "⇔");
}

#[test]
fn implication_then_equivalence_is_rejected() {
    pred_incompatible("x > 0 ⇒ y > 0 ⇔ z > 0", "⇒", "⇔");
}

#[test]
fn equivalence_then_implication_is_rejected() {
    pred_incompatible("x > 0 ⇔ y > 0 ⇒ z > 0", "⇔", "⇒");
}

#[test]
fn a_surrounding_bracket_does_not_license_an_implication_chain() {
    // Unlike a bare quantifier operand, binary-operator chaining is never
    // licensed by a closing bracket — only explicit grouping parentheses are.
    pred_incompatible("(x > 0 ⇒ y > 0 ⇒ z > 0)", "⇒", "⇒");
}

#[test]
fn explicitly_grouped_implication_and_equivalence_parse() {
    pred_ok("x > 0 ⇒ (y > 0 ⇒ z > 0)");
    pred_ok("(x > 0 ⇒ y > 0) ⇒ z > 0");
    pred_ok("x > 0 ⇔ (y > 0 ⇔ z > 0)");
    pred_ok("(x > 0 ⇒ y > 0) ⇔ z > 0");
    pred_ok("x > 0 ⇒ (y > 0 ⇔ z > 0)");
}

// A bare quantifier may not be a ⇒/⇔ operand either, with the same
// closing-bracket exception as the ∧/∨ level.

#[test]
fn bare_quantifier_as_an_implication_operand_is_rejected() {
    pred_incompatible("x > 0 ⇒ ∃ w · w > 0", "⇒", "∃");
    pred_incompatible("x > 0 ⇔ ∃ w · w > 0", "⇔", "∃");
    pred_incompatible("x > 0 ⇒ ∀ w · w > 0", "⇒", "∀");
}

#[test]
fn quantifier_implication_operand_bounded_by_a_bracket_parses() {
    pred_ok("x > 0 ⇒ (∃ w · w > 0)"); // quantifier parenthesised
    pred_ok("(x > 0 ⇒ ∃ w · w > 0)"); // whole predicate bracketed
    pred_ok("x > 0 ⇔ (∃ w · w > 0)");
}
