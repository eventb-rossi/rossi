//! Operator extensions: registration, construction, typing, and WD.

use std::sync::Arc;

use rossi::formula::extension::{
    ExpressionExtension, ExtendedRef, Extension, ExtensionKind, FormulaExtension,
    PredicateExtension,
};
use rossi::formula::tag::{self, BinaryExprOp, RelationalOp};
use rossi::formula::typecheck::{TcType, TypeCheckMediator};
use rossi::formula::wd::WdMediator;
use rossi::formula::{Expression, ExpressionKind, FactoryError, FormulaFactory, Predicate, Type};

use crate::common::{env, int};

/// `dist(a, b)` — a total binary integer operator.
struct Dist;

impl FormulaExtension for Dist {
    fn symbol(&self) -> &str {
        "dist"
    }
    fn id(&self) -> &str {
        "test.dist"
    }
    fn group_id(&self) -> &str {
        "test.group"
    }
    fn kind(&self) -> ExtensionKind {
        ExtensionKind::prefix_expression(2)
    }
    fn conjoin_children_wd(&self) -> bool {
        true
    }
    fn wd_predicate(&self, _formula: ExtendedRef<'_>, wd: &WdMediator<'_>) -> Predicate {
        wd.true_wd()
    }
}

impl ExpressionExtension for Dist {
    fn synthesize_type(&self, exprs: &[Expression], _preds: &[Predicate]) -> Option<Type> {
        exprs
            .iter()
            .all(|e| e.ty() == Some(&Type::Int))
            .then_some(Type::Int)
    }
    fn verify_type(&self, proposed: &Type, _: &[Expression], _: &[Predicate]) -> bool {
        *proposed == Type::Int
    }
    fn type_check(&self, mediator: &mut TypeCheckMediator<'_, '_>, exprs: &[TcType]) -> TcType {
        let int = mediator.from_type(&Type::Int);
        for child in exprs {
            mediator.same_type(*child, int);
        }
        int
    }
}

/// `even(n)` — a predicate operator over one integer.
struct Even;

impl FormulaExtension for Even {
    fn symbol(&self) -> &str {
        "even"
    }
    fn id(&self) -> &str {
        "test.even"
    }
    fn group_id(&self) -> &str {
        "test.group"
    }
    fn kind(&self) -> ExtensionKind {
        ExtensionKind::prefix_predicate(1)
    }
    fn conjoin_children_wd(&self) -> bool {
        true
    }
    fn wd_predicate(&self, _formula: ExtendedRef<'_>, wd: &WdMediator<'_>) -> Predicate {
        wd.true_wd()
    }
}

impl PredicateExtension for Even {
    fn type_check(&self, mediator: &mut TypeCheckMediator<'_, '_>, exprs: &[TcType]) {
        let int = mediator.from_type(&Type::Int);
        for child in exprs {
            mediator.same_type(*child, int);
        }
    }
}

fn dist_ext() -> Arc<dyn ExpressionExtension> {
    static DIST: std::sync::LazyLock<Arc<dyn ExpressionExtension>> =
        std::sync::LazyLock::new(|| Arc::new(Dist));
    DIST.clone()
}

fn even_ext() -> Arc<dyn PredicateExtension> {
    static EVEN: std::sync::LazyLock<Arc<dyn PredicateExtension>> =
        std::sync::LazyLock::new(|| Arc::new(Even));
    EVEN.clone()
}

fn extended_factory() -> FormulaFactory {
    FormulaFactory::with_extensions([Extension::Expr(dist_ext()), Extension::Pred(even_ext())])
        .expect("valid extension set")
}

// --- registration ---

#[test]
fn factories_are_interned_per_extension_set() {
    assert_eq!(extended_factory(), extended_factory());
    assert_ne!(extended_factory(), FormulaFactory::default_factory());
    // The empty set is the default factory.
    assert_eq!(
        FormulaFactory::with_extensions([]).expect("empty set"),
        FormulaFactory::default_factory()
    );
}

#[test]
fn tags_are_stable_and_disjoint_from_the_core_range() {
    let ff = extended_factory();
    let tags: Vec<_> = ff.extensions().map(|(tag, _)| tag).collect();
    assert_eq!(tags.len(), 2);
    for tag in &tags {
        assert!(*tag >= tag::FIRST_EXTENSION_TAG);
    }
    // Re-requesting the factory yields the same tags.
    let again: Vec<_> = extended_factory().extensions().map(|(t, _)| t).collect();
    assert_eq!(tags, again);
}

#[test]
fn duplicate_symbols_are_rejected() {
    struct Clash;
    impl FormulaExtension for Clash {
        fn symbol(&self) -> &str {
            "dist"
        }
        fn id(&self) -> &str {
            "test.clash"
        }
        fn group_id(&self) -> &str {
            "test.group"
        }
        fn kind(&self) -> ExtensionKind {
            ExtensionKind::atomic_expression()
        }
        fn conjoin_children_wd(&self) -> bool {
            true
        }
        fn wd_predicate(&self, _: ExtendedRef<'_>, wd: &WdMediator<'_>) -> Predicate {
            wd.true_wd()
        }
    }
    impl ExpressionExtension for Clash {
        fn synthesize_type(&self, _: &[Expression], _: &[Predicate]) -> Option<Type> {
            None
        }
        fn verify_type(&self, _: &Type, _: &[Expression], _: &[Predicate]) -> bool {
            true
        }
        fn type_check(&self, mediator: &mut TypeCheckMediator<'_, '_>, _: &[TcType]) -> TcType {
            mediator.fresh()
        }
    }
    let result = FormulaFactory::with_extensions([
        Extension::Expr(dist_ext()),
        Extension::Expr(Arc::new(Clash)),
    ]);
    assert!(result.is_err());

    // A reserved core word is rejected outright.
    struct Card;
    impl FormulaExtension for Card {
        fn symbol(&self) -> &str {
            "card"
        }
        fn id(&self) -> &str {
            "test.card"
        }
        fn group_id(&self) -> &str {
            "test.group"
        }
        fn kind(&self) -> ExtensionKind {
            ExtensionKind::atomic_expression()
        }
        fn conjoin_children_wd(&self) -> bool {
            true
        }
        fn wd_predicate(&self, _: ExtendedRef<'_>, wd: &WdMediator<'_>) -> Predicate {
            wd.true_wd()
        }
    }
    impl ExpressionExtension for Card {
        fn synthesize_type(&self, _: &[Expression], _: &[Predicate]) -> Option<Type> {
            None
        }
        fn verify_type(&self, _: &Type, _: &[Expression], _: &[Predicate]) -> bool {
            true
        }
        fn type_check(&self, mediator: &mut TypeCheckMediator<'_, '_>, _: &[TcType]) -> TcType {
            mediator.fresh()
        }
    }
    assert!(FormulaFactory::with_extensions([Extension::Expr(Arc::new(Card))]).is_err());
}

// --- construction ---

#[test]
fn construction_validates_extension_and_arity() {
    let ff = extended_factory();

    // Wrong arity.
    let one_arg = ff.extended_expression(
        &dist_ext(),
        vec![ff.integer_literal(1, None)],
        vec![],
        None,
        None,
    );
    assert_eq!(one_arg.unwrap_err(), FactoryError::ArityMismatch);

    // Unknown extension on the default factory.
    let unknown = FormulaFactory::default_factory().extended_expression(
        &dist_ext(),
        vec![int(1), int(2)],
        vec![],
        None,
        None,
    );
    assert_eq!(unknown.unwrap_err(), FactoryError::UnknownExtension);

    // A misfitting explicit type is rejected.
    let misfit = ff.extended_expression(
        &dist_ext(),
        vec![ff.integer_literal(1, None), ff.integer_literal(2, None)],
        vec![],
        None,
        Some(Type::Bool),
    );
    assert_eq!(misfit.unwrap_err(), FactoryError::TypeMisfit);
}

#[test]
fn typed_children_synthesize_the_extension_type() {
    let ff = extended_factory();
    let node = ff
        .extended_expression(
            &dist_ext(),
            vec![ff.integer_literal(1, None), ff.integer_literal(2, None)],
            vec![],
            None,
            None,
        )
        .expect("fits");
    assert_eq!(node.ty(), Some(&Type::Int));
    assert!(node.is_wd_strict());
}

// --- type checking ---

#[test]
fn extensions_participate_in_type_checking() {
    let ff = extended_factory();
    // dist(x, y) = 3 infers x, y ⦂ ℤ.
    let node = ff
        .extended_expression(
            &dist_ext(),
            vec![
                ff.free_identifier("x", None, None),
                ff.free_identifier("y", None, None),
            ],
            vec![],
            None,
            None,
        )
        .expect("fits");
    let pred =
        ff.relational_predicate(RelationalOp::Equal, node, ff.integer_literal(3, None), None);
    let result = pred.type_check(&env(&[]));
    assert!(result.is_success(), "problems: {:?}", result.problems);
    assert_eq!(result.inferred.get("x"), Some(&Type::Int));
    assert_eq!(result.inferred.get("y"), Some(&Type::Int));

    // even(b) with b ⦂ BOOL fails.
    let uneven = ff
        .extended_predicate(
            &even_ext(),
            vec![ff.free_identifier("b", None, None)],
            vec![],
            None,
        )
        .expect("fits");
    let result = uneven.type_check(&env(&[("b", Type::Bool)]));
    assert!(!result.is_success());

    // even(n) with n unknown infers ℤ.
    let evens = ff
        .extended_predicate(
            &even_ext(),
            vec![ff.free_identifier("n", None, None)],
            vec![],
            None,
        )
        .expect("fits");
    let result = evens.type_check(&env(&[]));
    assert!(result.is_success());
    assert_eq!(result.inferred.get("n"), Some(&Type::Int));
}

// --- well-definedness and rewriting through extended nodes ---

#[test]
fn strict_extensions_conjoin_child_lemmas() {
    let ff = extended_factory();
    let division = ff.binary_expression(
        BinaryExprOp::Div,
        ff.integer_literal(1, None),
        ff.free_identifier("z", None, None),
        None,
    );
    let node = ff
        .extended_expression(
            &dist_ext(),
            vec![division, ff.integer_literal(2, None)],
            vec![],
            None,
            None,
        )
        .expect("fits");
    let pred =
        ff.relational_predicate(RelationalOp::Equal, node, ff.integer_literal(3, None), None);
    let typed = pred
        .type_check(&env(&[("z", Type::Int)]))
        .typed
        .expect("typed");
    let expected = ff.relational_predicate(
        RelationalOp::NotEqual,
        ff.free_identifier("z", None, Some(Type::Int)),
        ff.integer_literal(0, None),
        None,
    );
    assert_eq!(typed.wd_lemma(), expected);
}

#[test]
fn substitution_descends_into_extended_nodes() {
    let ff = extended_factory();
    let node = ff
        .extended_expression(
            &dist_ext(),
            vec![
                ff.free_identifier("x", None, None),
                ff.integer_literal(2, None),
            ],
            vec![],
            None,
            None,
        )
        .expect("fits");
    let pred =
        ff.relational_predicate(RelationalOp::Equal, node, ff.integer_literal(3, None), None);
    let map: std::collections::HashMap<String, Expression> =
        [("x".to_string(), ff.integer_literal(7, None))]
            .into_iter()
            .collect();
    let substituted = pred.substitute_free_idents(&map);
    match substituted.kind() {
        rossi::formula::PredicateKind::Relational { left, .. } => match left.kind() {
            ExpressionKind::Extended { exprs, .. } => {
                assert_eq!(exprs[0], ff.integer_literal(7, None));
            }
            other => panic!("expected extended node, got {other:?}"),
        },
        other => panic!("expected relational, got {other:?}"),
    }
    assert!(pred.free_identifiers().contains(&"x".to_string()));
    assert!(substituted.free_identifiers().is_empty());
}

// --- parsing against a factory ---

#[test]
fn parse_predicate_str_with_builds_with_the_given_factory() {
    let ff = extended_factory();
    let pred = rossi::parse_predicate_str_with("x = 1 ∧ y ∈ ℕ", &ff).expect("parses");
    assert_eq!(pred.factory(), &ff);
    let rossi::PredicateKind::Associative { children, .. } = pred.kind() else {
        panic!("expected a conjunction, got {pred:?}");
    };
    for child in children {
        assert_eq!(
            child.factory(),
            &ff,
            "every subformula belongs to the factory"
        );
    }

    let expr = rossi::parse_expression_str_with("x + 1", &ff).expect("parses");
    assert_eq!(expr.factory(), &ff);
    let action = rossi::parse_action_str_with("x ≔ x + 1", &ff).expect("parses");
    assert_eq!(action.assignment().expect("an assignment").factory(), &ff);

    // The plain entry points keep building with the default factory, and the
    // scope does not leak out of a `_with` call.
    let plain = rossi::parse_predicate_str("x = 1").expect("parses");
    assert_eq!(plain.factory(), &FormulaFactory::default_factory());
    assert_ne!(plain.factory(), &ff);

    let components =
        rossi::parse_components_with("CONTEXT C\nAXIOMS\n  @a 1 = 1\nEND\n", &ff).expect("parses");
    let rossi::Component::Context(ctx) = &components[0] else {
        panic!("expected a context");
    };
    assert_eq!(ctx.axioms[0].predicate.factory(), &ff);
}

#[test]
fn parse_predicate_at_shifts_spans() {
    let ff = extended_factory();
    let pred = rossi::parse_predicate_at("x = 1", 10, &ff).expect("parses");
    assert_eq!(pred.factory(), &ff);
    assert_eq!(
        pred.span(),
        Some(rossi::formula::Span { start: 10, end: 15 })
    );
    let rossi::PredicateKind::Relational { left, right, .. } = pred.kind() else {
        panic!("expected a relational predicate, got {pred:?}");
    };
    assert_eq!(
        left.span(),
        Some(rossi::formula::Span { start: 10, end: 11 })
    );
    assert_eq!(
        right.span(),
        Some(rossi::formula::Span { start: 14, end: 15 })
    );

    let expr = rossi::parse_expression_at("x + 1", 3, &ff).expect("parses");
    assert_eq!(expr.span(), Some(rossi::formula::Span { start: 3, end: 8 }));
}
