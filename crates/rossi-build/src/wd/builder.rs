//! Smart constructors for WD lemmas — port of rodin-ast's
//! `org.eventb.internal.core.ast.wd.FormulaBuilder`.
//!
//! `Predicate::True` plays the role of Rodin's `BTRUE` literal: it is the
//! neutral element the constructors simplify away, and a final result of
//! `Predicate::True` means "no WD condition".
//!
//! The conjunction constructors build left-associated binary chains;
//! Rodin builds n-ary `LAND` nodes and `flatten()`s at the end, which is
//! indistinguishable after rendering and improver decomposition (both
//! flatten same-operator chains).

use rossi::ast::expression::{BinaryOp, UnaryOp};
use rossi::ast::predicate::{ComparisonOp, LogicalOp, Quantifier};
use rossi::{Expression, Predicate, TypedIdentifier};

use crate::normalize::type_to_expression;
use crate::types::Type;

/// `left ∧ right`, with `⊤` as the neutral element.
pub fn land2(left: Predicate, right: Predicate) -> Predicate {
    if left == Predicate::True {
        return right;
    }
    if right == Predicate::True {
        return left;
    }
    Predicate::logical(LogicalOp::And, left, right)
}

/// n-ary conjunction: filters `⊤` conjuncts, then chains the rest.
pub fn land(children: Vec<Predicate>) -> Predicate {
    let mut iter = children.into_iter().filter(|c| *c != Predicate::True);
    let Some(first) = iter.next() else {
        return Predicate::True;
    };
    iter.fold(first, |acc, c| Predicate::logical(LogicalOp::And, acc, c))
}

/// `left ⇒ right` with Rodin's simplifications:
/// - `⊤ ⇒ r` and `l ⇒ ⊤` collapse to `r` / `⊤`;
/// - nested implications merge their antecedents:
///   `l ⇒ (a ⇒ b)` becomes `l ∧ a ⇒ b`;
/// - `l ⇒ l` collapses to `⊤`.
pub fn limp(left: Predicate, right: Predicate) -> Predicate {
    if left == Predicate::True || right == Predicate::True {
        return right;
    }
    if let Predicate::Logical {
        op: LogicalOp::Implies,
        left: inner_left,
        right: inner_right,
    } = right
    {
        return limp(land2(left, *inner_left), *inner_right);
    }
    if left == right {
        return Predicate::True;
    }
    Predicate::logical(LogicalOp::Implies, left, right)
}

/// `left ∨ right`; `⊤` absorbs the disjunction.
pub fn lor2(left: Predicate, right: Predicate) -> Predicate {
    if left == Predicate::True {
        return left;
    }
    if right == Predicate::True {
        return right;
    }
    Predicate::logical(LogicalOp::Or, left, right)
}

/// `∀decls·pred`, skipped entirely when the body is `⊤`.
pub fn forall(decls: Vec<TypedIdentifier>, pred: Predicate) -> Predicate {
    if pred == Predicate::True {
        return pred;
    }
    Predicate::quantified(Quantifier::ForAll, decls, pred)
}

/// `∃decls·pred`, skipped entirely when the body is `⊤`.
pub fn exists(decls: Vec<TypedIdentifier>, pred: Predicate) -> Predicate {
    if pred == Predicate::True {
        return pred;
    }
    Predicate::quantified(Quantifier::Exists, decls, pred)
}

/// `expr ≠ 0` (divisor of `÷`).
pub fn not_zero(expr: Expression) -> Predicate {
    Predicate::comparison(ComparisonOp::NotEqual, expr, Expression::Integer(0))
}

/// `0 ≤ expr`.
pub fn non_negative(expr: Expression) -> Predicate {
    Predicate::comparison(ComparisonOp::LessEqual, Expression::Integer(0), expr)
}

/// `0 < expr` (divisor of `mod`).
pub fn positive(expr: Expression) -> Predicate {
    Predicate::comparison(ComparisonOp::LessThan, Expression::Integer(0), expr)
}

/// `finite(expr)` (operand of `card`).
pub fn finite(expr: Expression) -> Predicate {
    Predicate::BuiltinApplication {
        predicate: rossi::ast::predicate::BuiltinPredicate::Finite,
        arguments: vec![expr],
    }
}

/// `expr ≠ ∅`. Rodin types the empty set; the type never shows in
/// `toString` output, so the bare `∅` is byte-identical.
pub fn not_empty(expr: Expression) -> Predicate {
    Predicate::comparison(ComparisonOp::NotEqual, expr, Expression::EmptySet)
}

/// `arg ∈ dom(fun)`.
pub fn in_domain(fun: Expression, arg: Expression) -> Predicate {
    let dom = Expression::Unary {
        op: UnaryOp::Domain,
        operand: Box::new(fun),
    };
    Predicate::comparison(ComparisonOp::In, arg, dom)
}

/// `fun ∈ src ⇸ trg`, where `src`/`trg` come from `fun`'s relational
/// type. Returns `None` when the type is not a relation (ill-typed
/// input the SC nevertheless kept).
pub fn partial(fun: Expression, fun_type: &Type) -> Option<Predicate> {
    let Type::PowerSet(pair) = fun_type else {
        return None;
    };
    let Type::Product(src, trg) = pair.as_ref() else {
        return None;
    };
    let pfun = Expression::binary(
        BinaryOp::PartialFunction,
        type_to_expression(src),
        type_to_expression(trg),
    );
    Some(Predicate::comparison(ComparisonOp::In, fun, pfun))
}

/// Boundedness condition for `min` (`lower = true`) / `max`:
/// `∃b·∀x·x∈set ⇒ b ≤ x` (resp. `b ≥ x`).
///
/// Rodin builds this with fresh De Bruijn identifiers it *names* `b` and
/// `x` only at `toString`, renaming on collision. The `$`-prefixed
/// placeholders reproduce that: `$` cannot occur in an Event-B
/// identifier, so the synthesized binders can never capture names free
/// in `set`; [`super::normal::resolve_binders`] later picks the display
/// names (`b`/`x`, suffixed when taken).
pub fn bounded(set: Expression, lower: bool) -> Predicate {
    let op = if lower {
        ComparisonOp::LessEqual
    } else {
        ComparisonOp::GreaterEqual
    };
    let rel = Predicate::comparison(
        op,
        Expression::identifier("$b"),
        Expression::identifier("$x"),
    );
    let x_in_set = Predicate::comparison(ComparisonOp::In, Expression::identifier("$x"), set);
    // Rodin assembles this implication directly, bypassing the `limp`
    // simplifications.
    let body = Predicate::logical(LogicalOp::Implies, x_in_set, rel);
    Predicate::quantified(
        Quantifier::Exists,
        vec![TypedIdentifier::untyped("$b".into())],
        Predicate::quantified(
            Quantifier::ForAll,
            vec![TypedIdentifier::untyped("$x".into())],
            body,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wd::render::render_predicate;
    use rossi::parse_predicate_str;

    fn p(src: &str) -> Predicate {
        parse_predicate_str(src).unwrap()
    }

    #[test]
    fn land_is_true_neutral() {
        assert_eq!(land2(Predicate::True, p("x∈S")), p("x∈S"));
        assert_eq!(land2(p("x∈S"), Predicate::True), p("x∈S"));
        assert_eq!(land(vec![]), Predicate::True);
        assert_eq!(
            land(vec![Predicate::True, p("x∈S"), Predicate::True]),
            p("x∈S")
        );
    }

    #[test]
    fn limp_merges_nested_antecedents() {
        // l ⇒ (a ⇒ b)  ⇝  l ∧ a ⇒ b
        let merged = limp(p("l∈S"), limp(p("a∈S"), p("b∈S")));
        assert_eq!(render_predicate(&merged), "l∈S∧a∈S⇒b∈S");
    }

    #[test]
    fn limp_collapses_identity_and_true() {
        assert_eq!(limp(p("x∈S"), p("x∈S")), Predicate::True);
        assert_eq!(limp(p("x∈S"), Predicate::True), Predicate::True);
        assert_eq!(limp(Predicate::True, p("x∈S")), p("x∈S"));
    }

    #[test]
    fn lor_absorbs_true() {
        assert_eq!(lor2(Predicate::True, p("x∈S")), Predicate::True);
        assert_eq!(lor2(p("x∈S"), Predicate::True), Predicate::True);
    }

    #[test]
    fn forall_skips_true_body() {
        assert_eq!(
            forall(vec![TypedIdentifier::untyped("x".into())], Predicate::True),
            Predicate::True
        );
    }

    #[test]
    fn bounded_renders_like_rodin() {
        use crate::wd::normal::resolve_binders;
        let set = rossi::parse_expression_str("s").unwrap();
        assert_eq!(
            render_predicate(&resolve_binders(&bounded(set.clone(), true))),
            "∃b·∀x·x∈s⇒b≤x"
        );
        assert_eq!(
            render_predicate(&resolve_binders(&bounded(set, false))),
            "∃b·∀x·x∈s⇒b≥x"
        );
    }

    #[test]
    fn partial_renders_function_space_from_type() {
        let f = rossi::parse_expression_str("f").unwrap();
        let ty = Type::pow(Type::Product(
            Box::new(Type::GivenSet("S".into())),
            Box::new(Type::pow(Type::Integer)),
        ));
        let pred = partial(f, &ty).unwrap();
        assert_eq!(render_predicate(&pred), "f∈S ⇸ ℙ(ℤ)");
        assert_eq!(
            partial(rossi::parse_expression_str("g").unwrap(), &Type::Integer),
            None
        );
    }
}
