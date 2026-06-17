//! Source-span coverage for action AST nodes and their write targets.

use rossi::ast::{ActionKind, Span};
use rossi::parse_action_str;

fn slice(src: &str, span: Span) -> &str {
    &src[span.start..span.end]
}

#[test]
fn assignment_target_is_spanned() {
    let src = "count := count + 1";
    let action = parse_action_str(src).expect("parses");
    let ActionKind::Assignment {
        variables,
        expressions,
    } = &action.kind
    else {
        panic!("expected assignment, got {:?}", action.kind);
    };
    // The write target `count` carries its own exact span (offset 0), distinct
    // from the read `count` on the right-hand side.
    let target = &variables[0];
    assert_eq!(slice(src, target.span.expect("target span")), "count");
    assert_eq!(target.span.unwrap().start, 0);
    assert_eq!(slice(src, expressions[0].span.unwrap()), "count + 1");
    assert_eq!(slice(src, action.span.expect("action span")), src);
}

#[test]
fn parallel_assignment_targets_each_spanned() {
    let src = "x, y := 1, 2";
    let action = parse_action_str(src).expect("parses");
    let ActionKind::Assignment { variables, .. } = &action.kind else {
        panic!("expected assignment");
    };
    assert_eq!(slice(src, variables[0].span.unwrap()), "x");
    assert_eq!(slice(src, variables[1].span.unwrap()), "y");
    assert_eq!(variables[1].span.unwrap().start, 3);
}

#[test]
fn function_override_target_is_spanned() {
    // `f(x) := y` is lowered by the parser to `f ≔ f\u{E103}{x ↦ y}`.
    let src = "f(x) := y";
    let action = parse_action_str(src).expect("parses");
    let ActionKind::Assignment { variables, .. } = &action.kind else {
        panic!("expected assignment, got {:?}", action.kind);
    };
    let target = &variables[0];
    assert_eq!(slice(src, target.span.expect("target span")), "f");
    assert_eq!(target.span.unwrap().start, 0);
}

#[test]
fn becomes_such_that_target_is_spanned() {
    let src = "x :| x' = x + 1";
    let action = parse_action_str(src).expect("parses");
    let ActionKind::BecomesSuchThat { variables, .. } = &action.kind else {
        panic!("expected becomes-such-that");
    };
    assert_eq!(slice(src, variables[0].span.unwrap()), "x");
}
