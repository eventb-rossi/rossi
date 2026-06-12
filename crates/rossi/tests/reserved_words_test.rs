//! Regression tests for issue #30: reserved words of the mathematical
//! language (kernel_lang §2.2) must be *consistently* rejected as user
//! identifiers, matching Rodin's lexer.
//!
//! Before the fix, `dom` parsed as an identifier in some positions
//! (`@a dom ∈ ℕ`) but `@a x = dom` followed by `end` failed with a
//! misleading pest error: `unary_expr` matched the `dom` operator token and
//! greedily consumed `end` as its operand, with no PEG backtracking
//! possible. Now `dom`/`ran` are operators only when applied to `(…)`
//! (Rodin's `ParenNudParser` rule), and reserved words are rejected as
//! identifiers at AST-build time with a located [`ParseError::ReservedWord`].

mod common;

use rossi::ast::Expression;
use rossi::ast::expression::UnaryOp;
use rossi::parser::{parse_action_str, parse_predicate_str};
use rossi::{ParseError, parse};

/// Run `parse_fn` on `input` and assert it fails with a `ReservedWord` error
/// for `word`, located at the word's first occurrence in `input`.
#[track_caller]
fn assert_reserved<T: std::fmt::Debug>(
    parse_fn: impl FnOnce(&str) -> Result<T, ParseError>,
    input: &str,
    word: &str,
) {
    let byte = input.find(word).expect("word must occur in input");
    let prefix = &input[..byte];
    let line = prefix.matches('\n').count() + 1;
    let column = prefix.rsplit('\n').next().unwrap().chars().count() + 1;
    match parse_fn(input) {
        Err(ParseError::ReservedWord {
            word: w,
            line: l,
            column: c,
        }) => {
            assert_eq!((w.as_str(), l, c), (word, line, column), "in {input:?}");
        }
        other => panic!("expected ReservedWord({word:?}) at {line}:{column}, got {other:?}"),
    }
}

// ============================================================================
// Issue #30: the two formerly-inconsistent shapes now agree
// ============================================================================

#[test]
fn issue_30_bare_dom_rejected_in_every_position() {
    // Formerly accepted (PEG backtracked to identifier).
    assert_reserved(parse_predicate_str, "dom ∈ ℕ", "dom");
    // Formerly accepted standalone…
    assert_reserved(parse_predicate_str, "x = dom", "dom");
    // …but failing with a misleading pest error when followed by `end`.
    assert_reserved(parse, "context c0 constants x axioms @a x = dom end", "dom");
    assert_reserved(parse, "context c0 constants x axioms @a x = ran end", "ran");
}

#[test]
fn reserved_operator_words_rejected_bare_in_formulas() {
    for word in ["card", "min", "max", "union", "inter", "mod", "finite"] {
        assert_reserved(parse_predicate_str, &format!("x = {word}"), word);
    }
}

#[test]
fn reserved_words_rejected_when_misapplied() {
    // Postfix inverse, relational image, and unresolvable application are
    // not the `word(…)` operator form — Rodin rejects all of these.
    assert_reserved(parse_predicate_str, "x = card∼", "card");
    assert_reserved(parse_predicate_str, "x = card[S]", "card");
    assert_reserved(parse_predicate_str, "x = mod(y)", "mod");
    // Expression-only words standing as a predicate application.
    assert_reserved(parse_predicate_str, "dom(f)", "dom");
}

#[test]
fn reserved_words_rejected_as_action_targets() {
    assert_reserved(parse_action_str, "dom ≔ 5", "dom");
    assert_reserved(parse_action_str, "x ≔ dom", "dom");
}

// ============================================================================
// Declaration sites (full §2.2 list, including the generic atoms)
// ============================================================================

#[test]
fn reserved_words_rejected_in_declarations() {
    assert_reserved(parse, "context c0 constants dom end", "dom");
    assert_reserved(parse, "context c0 sets card end", "card");
    assert_reserved(parse, "context c0 sets S = {a, succ} end", "succ");
    // Atoms and literals can't be declared either.
    assert_reserved(parse, "context c0 constants pred end", "pred");
    assert_reserved(parse, "context c0 constants TRUE end", "TRUE");
    assert_reserved(parse, "machine m0 variables ran end", "ran");
    assert_reserved(
        parse,
        "machine m0 events event e any inter then skip end end",
        "inter",
    );
}

#[test]
fn reserved_words_rejected_as_binders() {
    assert_reserved(parse_predicate_str, "∀ dom · dom ∈ ℕ", "dom");
    assert_reserved(parse_predicate_str, "∃ x, max · x = max", "max");
    assert_reserved(parse_predicate_str, "s = {prj1 · prj1 ∈ ℕ | prj1}", "prj1");
    assert_reserved(parse_predicate_str, "f = λ id · id ∈ ℕ ∣ id", "id");
}

// ============================================================================
// The operator/atom readings still parse
// ============================================================================

#[test]
fn applied_builtin_forms_still_parse() {
    let ctx = common::axiom_context("f, S", "dom(f) = S ∧ ran(f) ⊆ S ∧ card(S) = 1");
    parse(&ctx).expect("applied dom/ran/card must parse");

    // Whitespace before the paren is fine (Rodin lexes tokens, not spacing).
    parse_predicate_str("dom (f) = S").expect("spaced dom (f) must parse");

    parse_predicate_str("finite(S)").expect("finite predicate must parse");
    parse_predicate_str("partition(S, A, B)").expect("partition must parse");
    parse_predicate_str("x = min(S) + max(S)").expect("min/max must parse");
    parse_predicate_str("u = union(S) ∪ inter(S)").expect("union/inter must parse");
}

#[test]
fn generic_atoms_still_parse_bare() {
    // id, prj1, prj2, pred, succ are generic atomic expressions in V2 —
    // legal bare, exactly as in Rodin.
    for atom in ["id", "prj1", "prj2", "pred", "succ"] {
        let rhs = common::parse_axiom_rhs(&common::axiom_context("f", &format!("f = {atom}")));
        assert_eq!(rhs, Expression::Identifier(atom.to_string()));
    }
}

#[test]
fn reservation_is_exact_case() {
    // Rodin reserves exact spellings only: `Dom`, `DOM`, `Card` are
    // ordinary identifiers there.
    let ctx = common::axiom_context("Dom, DOM, Card", "Dom = DOM ∧ Card ∈ ℕ");
    parse(&ctx).expect("non-exact-case spellings are ordinary identifiers");
}

#[test]
fn dom_no_longer_swallows_a_following_identifier() {
    // `dom f` (no parens) is a syntax error in Rodin; it must not parse as
    // an application here either.
    assert!(parse_predicate_str("x = dom f").is_err());
}

// ============================================================================
// Closed-form precedence: postfix operators bind to the whole dom(…)
// ============================================================================

/// Parse `x = {formula}` in a context and return the axiom's RHS.
fn axiom_rhs(formula: &str) -> Expression {
    common::parse_axiom_rhs(&common::axiom_context(
        "x, f, g, S",
        &format!("x = {formula}"),
    ))
}

#[track_caller]
fn assert_is_unary(expr: &Expression, op: UnaryOp) {
    assert!(
        matches!(expr, Expression::Unary { op: o, .. } if *o == op),
        "expected Unary {op:?}, got {expr:?}"
    );
}

#[test]
fn postfix_operators_bind_outside_closed_dom() {
    // Rodin: dom(f)∼ = (dom(f))∼, not dom(f∼).
    let inverse = axiom_rhs("dom(f)∼");
    let Expression::Unary {
        op: UnaryOp::Inverse,
        operand,
    } = inverse
    else {
        panic!("expected (dom(f))∼, got {inverse:?}");
    };
    assert_is_unary(&operand, UnaryOp::Domain);

    // Rodin: dom(f)(x) = (dom(f))(x), not dom(f(x)).
    let applied = axiom_rhs("dom(f)(g)");
    let Expression::FunctionApplication { function, .. } = applied else {
        panic!("expected (dom(f))(g), got {applied:?}");
    };
    assert_is_unary(&function, UnaryOp::Domain);

    // Rodin: ran(f)[S] = (ran(f))[S], not ran(f[S]).
    let imaged = axiom_rhs("ran(f)[S]");
    let Expression::RelationalImage { relation, .. } = imaged else {
        panic!("expected (ran(f))[S], got {imaged:?}");
    };
    assert_is_unary(&relation, UnaryOp::Range);
}

// ============================================================================
// Generic atoms in positions that *name* an identifier
// ============================================================================

#[test]
fn atoms_rejected_as_action_targets_and_phantom_predicates() {
    // Assignment targets are uses of declared variables; the atoms can never
    // be declared, so assigning to them is as invalid as `dom ≔ 0`.
    assert_reserved(parse_action_str, "pred ≔ 5", "pred");
    assert_reserved(parse_action_str, "id ≔ 5", "id");
    assert_reserved(parse_action_str, "TRUE ≔ 5", "TRUE");

    // An atom applied where a predicate is expected is an expression, never
    // a predicate — reject instead of fabricating Predicate::Application.
    assert_reserved(parse_predicate_str, "pred(x)", "pred");
    assert_reserved(parse_predicate_str, "id(x)", "id");
    // …but the same applications are fine as expressions.
    parse_predicate_str("y = pred(x)").expect("pred(x) is a valid expression");
    parse_predicate_str("y = id(x)").expect("id(x) is a valid expression (V1 compat)");
}

// ============================================================================
// Recovery and XML import stay consistent with the strict parser
// ============================================================================

#[test]
fn recovery_does_not_readmit_reserved_declarations() {
    // Multi-line layout: the recovery scanner takes one identifier per line
    // for space-separated lists.
    let result =
        rossi::parse_with_recovery("CONTEXT c0\nCONSTANTS\n  dom\n  x\nAXIOMS\n  @a x = 1\nEND");
    assert!(
        !result.errors.is_empty(),
        "the ReservedWord error must be reported"
    );
    let Some(rossi::ast::Component::Context(ctx)) = result.component else {
        panic!("recovery must still produce a component");
    };
    let names: Vec<_> = ctx.constants.iter().map(|c| c.name.as_str()).collect();
    assert!(
        !names.contains(&"dom"),
        "recovered AST must not contain the rejected declaration: {names:?}"
    );
    assert!(names.contains(&"x"), "valid sibling declarations survive");
}

#[test]
fn xml_import_rejects_reserved_declared_names() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
  <org.eventb.core.constant name="c1" org.eventb.core.identifier="dom"/>
</org.eventb.core.contextFile>"#;
    match rossi::xml::parse_xml(xml) {
        Err(ParseError::UnsupportedIdentifier { name, reason, .. }) => {
            assert_eq!(name, "dom");
            assert!(reason.contains("reserved"), "reason: {reason}");
        }
        other => panic!("expected UnsupportedIdentifier for constant `dom`, got {other:?}"),
    }
}
