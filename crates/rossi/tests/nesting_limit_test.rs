//! Regression tests for the nesting-depth guard.
//!
//! Deeply nested formulas used to overflow the stack and abort the whole
//! process (fatal for the language server, which parses every workspace
//! file). The parser must now reject over-deep input with
//! [`ParseError::NestingTooDeep`] and parse at-limit input successfully.
//!
//! The at-limit cases double as a stack-headroom proof: they run in debug CI
//! on default test threads, so if the guaranteed parser stack ever stops
//! covering [`MAX_NESTING_DEPTH`], they crash rather than silently regress.

use rossi::{
    MAX_NESTING_DEPTH, ParseError, enclosing_spans, parse, parse_expression_str,
    parse_predicate_str, parse_with_recovery, to_string,
};

/// `((((…x…)))) = 1` with `n` parens, wrapped in a context axiom.
fn deep_paren_context(n: usize) -> String {
    format!(
        "context C axioms @a {}x{} = 1 end",
        "(".repeat(n),
        ")".repeat(n)
    )
}

/// `¬¬…¬(1=1)` with `n` negations (plus one paren level), as a context axiom.
fn deep_negation_context(n: usize) -> String {
    format!("context C axioms @a {}(1=1) end", "¬".repeat(n))
}

/// `∀x0 · ∀x1 · … · 1=1` with `n` quantifiers, as a context axiom.
fn deep_forall_context(n: usize) -> String {
    let chain: String = (0..n).map(|k| format!("∀ x{k} · ")).collect();
    format!("context C axioms @a {chain}1=1 end")
}

fn assert_too_deep<T: std::fmt::Debug>(result: Result<T, ParseError>) {
    match result {
        Err(ParseError::NestingTooDeep { limit, .. }) => {
            assert_eq!(limit, MAX_NESTING_DEPTH);
        }
        other => panic!("expected NestingTooDeep, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Over the limit: clean error, no abort
// ---------------------------------------------------------------------------

#[test]
fn over_limit_parens_rejected() {
    assert_too_deep(parse(&deep_paren_context(MAX_NESTING_DEPTH + 1)));
    // Far beyond the limit (the original crash was a fuzz file; 5000 levels
    // would overflow even release builds without the pre-scan).
    assert_too_deep(parse(&deep_paren_context(5000)));
}

#[test]
fn over_limit_negations_rejected() {
    assert_too_deep(parse(&deep_negation_context(MAX_NESTING_DEPTH + 1)));
}

#[test]
fn over_limit_quantifiers_rejected() {
    assert_too_deep(parse(&deep_forall_context(MAX_NESTING_DEPTH + 1)));
}

#[test]
fn over_limit_predicate_and_expression_strs_rejected() {
    let pred = format!("{}(1=1)", "¬".repeat(MAX_NESTING_DEPTH + 1));
    assert_too_deep(parse_predicate_str(&pred));

    let n = MAX_NESTING_DEPTH + 1;
    let expr = format!("{}x{}", "(".repeat(n), ")".repeat(n));
    assert_too_deep(parse_expression_str(&expr));
}

#[test]
fn over_limit_recovery_returns_single_error_without_panicking() {
    let result = parse_with_recovery(&deep_paren_context(5000));
    assert!(result.component.is_none());
    assert_eq!(result.errors.len(), 1);
    assert!(matches!(
        result.errors[0],
        ParseError::NestingTooDeep { .. }
    ));
}

#[test]
fn over_limit_enclosing_spans_returns_empty() {
    let source = deep_paren_context(5000);
    assert!(enclosing_spans(&source, source.len() / 2).is_empty());
}

// ---------------------------------------------------------------------------
// At the limit: parses successfully (stack-headroom proof in debug CI)
// ---------------------------------------------------------------------------

#[test]
fn at_limit_parens_parse() {
    let component = parse(&deep_paren_context(MAX_NESTING_DEPTH)).expect("at-limit parens");
    // Parens collapse in the AST; downstream consumers must handle the result.
    let printed = to_string(&component);
    parse(&printed).expect("pretty output reparses");
    let _ = rossi::to_xml(&component);
}

#[test]
fn at_limit_negations_parse() {
    // One paren level is part of the input, so stay one under the limit.
    let source = deep_negation_context(MAX_NESTING_DEPTH - 1);
    let component = parse(&source).expect("at-limit negations");
    // The negation chain is retained in the AST — exercise the recursive
    // consumers (pretty printer, XML export) at depth in debug builds.
    let _ = to_string(&component);
    let _ = rossi::to_xml(&component);

    // Round-trip at half depth: the pretty printer parenthesizes each
    // negation level (`¬(¬(…))`), doubling the counted depth, so an at-limit
    // chain legitimately reparses as over-limit.
    let component = parse(&deep_negation_context(MAX_NESTING_DEPTH / 2 - 1)).expect("half depth");
    parse(&to_string(&component)).expect("pretty output reparses");
}

#[test]
fn at_limit_quantifiers_parse() {
    let source = deep_forall_context(MAX_NESTING_DEPTH);
    let component = parse(&source).expect("at-limit quantifiers");
    let printed = to_string(&component);
    parse(&printed).expect("pretty output reparses");
    let _ = rossi::to_xml(&component);
}

#[test]
fn at_limit_mixed_parens_and_negations_parse() {
    let half = MAX_NESTING_DEPTH / 2;
    let source = format!(
        "context C axioms @a {}{}1=1{} end",
        "¬".repeat(half),
        "(".repeat(half),
        ")".repeat(half)
    );
    parse(&source).expect("at-limit mixed");
}

/// Every recursion driver the pre-scan counts (see crates/rossi/src/nesting.rs)
/// must parse at the limit — this pins the scanner's token set to the
/// grammar: a driver whose per-level stack cost outgrows the budget, or a
/// new grammar construct the scanner forgets, shows up here as a debug-CI
/// crash instead of a production abort.
#[test]
fn at_limit_every_driver_construct_parses() {
    let n = MAX_NESTING_DEPTH;

    // Lambda pattern parens: λ counts 1, each '(' counts 1.
    let k = n - 1;
    let lambda = format!(
        "context C axioms @a f = λ{}x{} · 1=1 ∣ x end",
        "(".repeat(k),
        ")".repeat(k)
    );
    parse(&lambda).expect("at-limit lambda pattern");

    // Existential chain.
    let exists: String = (0..n).map(|i| format!("∃ y{i} · ")).collect();
    parse(&format!("context C axioms @a {exists}1=1 end")).expect("at-limit ∃ chain");

    // Quantified union chain (keyword and symbol forms count alike); the
    // terminal set literal costs one brace level.
    let k = n - 1;
    let union: String = (0..k).map(|i| format!("⋃ z{i} · 1=1 ∣ ")).collect();
    parse(&format!("context C axioms @a s = {union}{{1}} end")).expect("at-limit ⋃ chain");

    // Unary keyword chains (dom/ran/ℙ) and unary minus. dom/ran require the
    // parenthesized form, so each level costs 2 (prefix word + bracket); the
    // chain only sits exactly at the limit while the limit is even.
    assert_eq!(n % 2, 0, "dom chain no longer exercises the exact limit");
    let k = n / 2;
    let dom = "dom(".repeat(k);
    parse(&format!(
        "context C constants r axioms @a x = {dom}r{} end",
        ")".repeat(k)
    ))
    .expect("at-limit dom chain");
    let pow = "ℙ".repeat(n);
    parse(&format!("context C constants S axioms @a T = {pow}S end")).expect("at-limit ℙ chain");
    let minus = "−".repeat(n);
    parse(&format!("context C axioms @a x = {minus}1 end")).expect("at-limit − chain");
}

// ---------------------------------------------------------------------------
// False-positive guards: realistic shapes must not trip the limit
// ---------------------------------------------------------------------------

#[test]
fn many_small_formulas_pass() {
    // 1000 labeled axioms, each with a shallow negation — the per-formula
    // reset must keep these from accumulating.
    let axioms: String = (0..1000).map(|k| format!("@a{k} ¬(c{k} = 1)\n")).collect();
    let source = format!("context C constants {} axioms {axioms} end", {
        (0..1000).map(|k| format!("c{k} ")).collect::<String>()
    });
    parse(&source).expect("many small formulas");
}

#[test]
fn long_conjunction_of_bracketed_negations_passes() {
    // (¬a) ∧ (¬a) ∧ … in ONE formula: prefix operators unwind at brackets.
    let chain = "(¬(a = 1)) ∧ ".repeat(MAX_NESTING_DEPTH * 2);
    let source = format!("context C constants a axioms @a {chain}(a = 1) end");
    parse(&source).expect("long conjunction");
}

#[test]
fn long_conjunction_of_unwrapped_negations_passes() {
    let chain = "not (a = 0) ∧ ".repeat(MAX_NESTING_DEPTH);
    let source = format!("context C constants a axioms @a {chain}not (a = 0) end");
    parse(&source).expect("long conjunction of shallow negations");
}

#[test]
fn ascii_arrows_and_subtraction_chains_pass() {
    let arrows = (0..MAX_NESTING_DEPTH * 2)
        .map(|k| format!("@t{k} f ∈ A --> B ∧ g ∈ A +-> B ∧ m = a |-> b\n"))
        .collect::<String>();
    let source = format!("context C constants f g m a b A B axioms {arrows} end");
    parse(&source).expect("ascii arrows");

    let subtraction = "a - ".repeat(MAX_NESTING_DEPTH * 2);
    let source = format!("context C constants a b axioms @a b = {subtraction}a end");
    parse(&source).expect("subtraction chain");
}
