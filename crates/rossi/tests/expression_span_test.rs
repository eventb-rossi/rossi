//! Source-span coverage for expression / predicate / assignment nodes.
//!
//! The parser records a byte span on every node it builds (issue #68), and
//! the lowering carries them onto the formula model. These tests pin the
//! spans of identifier leaves and a few structural nodes so navigation
//! features can rely on them.

use rossi::ast::Span;
use rossi::formula::FormulaRef;
use rossi::{
    AssignmentKind, ExpressionKind, PredicateKind, parse_action_str, parse_expression_str,
    parse_predicate_str,
};

/// The source slice a span points at.
fn slice(src: &str, span: Span) -> &str {
    &src[span.start..span.end]
}

#[test]
fn recovered_component_has_absolute_formula_spans() {
    // A broken first component forces multi-component error recovery; the later
    // component is parsed from a region slice and its formula spans are lifted
    // to absolute document coordinates by the lowering. The inner identifier
    // spans must be absolute, not left relative to the region.
    let src = "CONTEXT C0\nAXIOMS\n@a xxxxx ∈\nEND\n\nMACHINE M0\nVARIABLES\ncount\nINVARIANTS\n@i1 count > 0\nEND\n";
    let parsed = rossi::parse_components_with_recovery(src);
    let components = parsed.component.expect("recovers components");
    let machine = components
        .iter()
        .find_map(|c| match c {
            rossi::Component::Machine(m) => Some(m),
            _ => None,
        })
        .expect("machine recovered");
    let PredicateKind::Relational { left, .. } = machine.invariants[0].predicate.kind() else {
        panic!("expected comparison invariant");
    };
    let span = left.span().expect("count span");
    assert_eq!(
        slice(src, span),
        "count",
        "invariant identifier span must be absolute after multi-component recovery"
    );
}

#[test]
fn recovered_clause_has_absolute_formula_spans() {
    // A broken axiom inside an otherwise-parsable context triggers clause
    // recovery: the surviving axiom is re-parsed from its segment, and its
    // formula spans must land on the document, not the segment.
    let src = "CONTEXT C0\nCONSTANTS\nk\nAXIOMS\n@a1 xxxxx ∈\n@a2 k = 1\nEND\n";
    let parsed = rossi::parse_with_recovery(src);
    let component = parsed.component.expect("recovers the context");
    let rossi::Component::Context(ctx) = &component else {
        panic!("expected a context");
    };
    let recovered = ctx
        .axioms
        .iter()
        .find_map(|ax| match ax.predicate.kind() {
            PredicateKind::Relational { left, .. } => left.span(),
            _ => None,
        })
        .expect("recovered @a2 with a spanned identifier");
    assert_eq!(
        &src[recovered.start..recovered.end],
        "k",
        "inner span must be absolute after clause recovery"
    );
}

#[test]
fn comparison_identifier_leaves_are_spanned() {
    let src = "x ∈ S";
    let pred = parse_predicate_str(src).expect("parses");
    let PredicateKind::Relational { left, right, .. } = pred.kind() else {
        panic!("expected comparison, got {:?}", pred.kind());
    };
    assert_eq!(slice(src, left.span().expect("left span")), "x");
    assert_eq!(slice(src, right.span().expect("right span")), "S");
    // The whole predicate spans the entire comparison.
    assert_eq!(slice(src, pred.span().expect("pred span")), "x ∈ S");
}

#[test]
fn nested_identifier_usages_point_at_each_occurrence() {
    // Two uses of `count` at distinct offsets must carry distinct spans.
    let src = "count = count + 1";
    let pred = parse_predicate_str(src).expect("parses");
    let PredicateKind::Relational { left, right, .. } = pred.kind() else {
        panic!("expected comparison");
    };
    assert_eq!(slice(src, left.span().unwrap()), "count");
    assert_eq!(left.span().unwrap().start, 0);
    let ExpressionKind::Associative { children, .. } = right.kind() else {
        panic!("expected sum");
    };
    assert_eq!(slice(src, children[0].span().unwrap()), "count");
    assert!(children[0].span().unwrap().start > 0);
}

#[test]
fn associative_fold_span_covers_every_operand() {
    let src = "a + b + c";
    let expr = parse_expression_str(src).expect("parses");
    // Same-operator chains flatten into one n-ary node spanning the input.
    let ExpressionKind::Associative { children, .. } = expr.kind() else {
        panic!("expected an associative sum");
    };
    assert_eq!(slice(src, expr.span().unwrap()), "a + b + c");
    assert_eq!(slice(src, children[0].span().unwrap()), "a");
    assert_eq!(slice(src, children[1].span().unwrap()), "b");
    assert_eq!(slice(src, children[2].span().unwrap()), "c");
}

#[test]
fn function_application_identifier_is_spanned() {
    let src = "f(x)";
    let expr = parse_expression_str(src).expect("parses");
    let ExpressionKind::Binary {
        left: function,
        right: argument,
        ..
    } = expr.kind()
    else {
        panic!("expected function application");
    };
    assert_eq!(slice(src, function.span().unwrap()), "f");
    assert_eq!(slice(src, argument.span().unwrap()), "x");
}

#[test]
fn predicate_application_name_is_spanned() {
    let src = "myPred(x)";
    let pred = parse_predicate_str(src).expect("parses");
    let PredicateKind::Application {
        function_span,
        args,
        ..
    } = pred.kind()
    else {
        panic!("expected application, got {:?}", pred.kind());
    };
    assert_eq!(slice(src, function_span.expect("function span")), "myPred");
    assert_eq!(function_span.unwrap().start, 0);
    assert_eq!(slice(src, args[0].span().unwrap()), "x");
}

#[test]
fn quantifier_binder_is_spanned() {
    let src = "∀ x · x ∈ S";
    let pred = parse_predicate_str(src).expect("parses");
    let PredicateKind::Quantified { decls, .. } = pred.kind() else {
        panic!("expected quantified");
    };
    // The binder declaration `x` (after the ∀) carries its own span.
    let binder = &decls[0];
    assert_eq!(slice(src, binder.span().expect("binder span")), "x");
    // ∀ is 3 bytes + space, so the binder starts at byte 4.
    assert_eq!(binder.span().unwrap().start, 4);
}

#[test]
fn lambda_pattern_binders_are_spanned() {
    let src = "λ x ↦ y · x ∈ ℤ ∧ y ∈ ℤ ∣ x";
    let expr = parse_expression_str(src).expect("parses");
    let ExpressionKind::Quantified { decls, .. } = expr.kind() else {
        panic!("expected lambda");
    };
    assert_eq!(decls.len(), 2);
    assert_eq!(slice(src, decls[0].span().expect("x binder span")), "x");
    assert_eq!(slice(src, decls[1].span().expect("y binder span")), "y");
}

#[test]
fn quantified_body_usage_is_spanned() {
    let src = "∀ x · x ∈ S";
    let pred = parse_predicate_str(src).expect("parses");
    let PredicateKind::Quantified { pred: body, .. } = pred.kind() else {
        panic!("expected quantified");
    };
    let PredicateKind::Relational { left, .. } = body.kind() else {
        panic!("expected comparison body");
    };
    // The bound usage `x` in the body points at the second `x`, not the binder.
    assert_eq!(slice(src, left.span().unwrap()), "x");
    assert!(left.span().unwrap().start > 0);
}

// ============================================================================
// Assignment spans — coverage for assignment nodes and their write targets.
// ============================================================================

#[test]
fn assignment_target_is_spanned() {
    let src = "count := count + 1";
    let assignment = parse_action_str(src).expect("parses");
    let AssignmentKind::BecomesEqualTo { idents, values } = assignment.kind() else {
        panic!("expected becomes-equal-to");
    };
    // The write target `count` carries its own exact span (offset 0), distinct
    // from the read `count` on the right-hand side.
    assert_eq!(slice(src, idents[0].span().expect("target span")), "count");
    assert_eq!(idents[0].span().unwrap().start, 0);
    assert_eq!(slice(src, values[0].span().unwrap()), "count + 1");
    assert_eq!(slice(src, assignment.span().expect("assignment span")), src);
}

#[test]
fn parallel_assignment_targets_each_spanned() {
    let src = "x, y := 1, 2";
    let assignment = parse_action_str(src).expect("parses");
    let AssignmentKind::BecomesEqualTo { idents, .. } = assignment.kind() else {
        panic!("expected becomes-equal-to");
    };
    assert_eq!(slice(src, idents[0].span().unwrap()), "x");
    assert_eq!(slice(src, idents[1].span().unwrap()), "y");
    assert_eq!(idents[1].span().unwrap().start, 3);
}

#[test]
fn function_override_target_is_spanned() {
    // `f(x) := y` is lowered by the parser to `f ≔ f\u{E103}{x ↦ y}`.
    let src = "f(x) := y";
    let assignment = parse_action_str(src).expect("parses");
    let AssignmentKind::BecomesEqualTo { idents, .. } = assignment.kind() else {
        panic!("expected becomes-equal-to, got {assignment:?}");
    };
    assert_eq!(slice(src, idents[0].span().expect("target span")), "f");
    assert_eq!(idents[0].span().unwrap().start, 0);

    // Nothing the lowering synthesizes is spanned: the override, the set
    // extension and the copy of `f` inside it are not spelled in the source,
    // and the one `f` that is belongs to the target asserted above. Two nodes
    // on one token would be reported twice by find-references.
    let AssignmentKind::BecomesEqualTo { values, .. } = assignment.kind() else {
        unreachable!("checked above");
    };
    let ExpressionKind::Associative { children, .. } = values[0].kind() else {
        panic!("expected the override, got {:?}", values[0]);
    };
    assert_eq!(values[0].span(), None, "the override node spells nothing");
    assert_eq!(children[0].span(), None, "the copied `f` spells nothing");
    assert_eq!(children[1].span(), None, "the set extension spells nothing");
}

#[test]
fn becomes_such_that_target_is_spanned() {
    let src = "x :| x' = x + 1";
    let assignment = parse_action_str(src).expect("parses");
    let AssignmentKind::BecomesSuchThat { idents, .. } = assignment.kind() else {
        panic!("expected becomes-such-that");
    };
    assert_eq!(slice(src, idents[0].span().unwrap()), "x");

    // `x'` is nowhere in the source, but it is derived from the `x` that is,
    // so it points at that identifier rather than nowhere.
    let AssignmentKind::BecomesSuchThat { primed, .. } = assignment.kind() else {
        unreachable!("checked above");
    };
    assert_eq!(slice(src, primed[0].span().expect("primed decl span")), "x");
}

#[test]
fn propositional_leaves_of_a_parsed_guard_resolve_to_their_source() {
    // The motivating case: number the atomic conditions of a guard and paint
    // each one back onto the text it came from.
    let src = "x > 0 ∧ (y = 1 ⇒ ¬ z ∈ S) ∧ (∀ n · n > y)";
    let pred = parse_predicate_str(src).expect("parses");

    let located: Vec<(usize, String, &str)> = pred
        .propositional_leaves()
        .iter()
        .enumerate()
        .map(|(index, position)| {
            let FormulaRef::Pred(leaf) = pred.sub_formula(position).expect("leaf exists") else {
                panic!("expected a predicate at {position}");
            };
            let span = leaf.span().expect("parsed leaves carry spans");
            (index, position.to_string(), slice(src, span).trim_end())
        })
        .collect();

    assert_eq!(
        located,
        [
            (0, "0".to_string(), "x > 0"),
            (1, "1.0".to_string(), "y = 1"),
            (2, "1.1.0".to_string(), "z ∈ S"),
            // The quantifier is one condition; `n > y` inside it is not a
            // condition of this guard.
            (3, "2".to_string(), "∀ n · n > y"),
        ]
    );
}

#[test]
fn variant_items_are_spanned() {
    // The variant row used to be the one machine element with no span at all,
    // so an outline could not navigate to it. The span covers the item as
    // written, label included, like a labeled predicate's.
    let source = "MACHINE m\nVARIABLES\n    x\nVARIANT\n    @vrn1 x\nEND";
    let rossi::Component::Machine(machine) = rossi::parse(source).unwrap() else {
        panic!("expected a machine");
    };
    let span = machine.variants[0].span.expect("variant carries a span");
    assert_eq!(slice(source, span), "@vrn1 x");

    // Unlabeled, the item is just the expression.
    let source = "MACHINE m\nVARIABLES\n    x\nVARIANT\n    x + 1\nEND";
    let rossi::Component::Machine(machine) = rossi::parse(source).unwrap() else {
        panic!("expected a machine");
    };
    let span = machine.variants[0].span.expect("variant carries a span");
    assert_eq!(slice(source, span), "x + 1");
}
