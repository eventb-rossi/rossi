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

/// The integer operators of the test factory: `dist(a, b)` (prefix, two
/// children), `zero` (nullary), `a plus b` (associative infix) and
/// `a minus b` (non-associative infix). One shape, differing in kind.
struct IntOp {
    symbol: &'static str,
    id: &'static str,
    kind: ExtensionKind,
}

impl FormulaExtension for IntOp {
    fn symbol(&self) -> &str {
        self.symbol
    }
    fn id(&self) -> &str {
        self.id
    }
    fn group_id(&self) -> &str {
        "test.group"
    }
    fn kind(&self) -> ExtensionKind {
        self.kind
    }
    fn conjoin_children_wd(&self) -> bool {
        true
    }
    fn wd_predicate(&self, _formula: ExtendedRef<'_>, wd: &WdMediator<'_>) -> Predicate {
        wd.true_wd()
    }
}

impl ExpressionExtension for IntOp {
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

/// The integer operator spelled `symbol`; one object per symbol for the
/// process, since registration is by object identity.
fn int_op(symbol: &str) -> Arc<dyn ExpressionExtension> {
    static OPS: std::sync::LazyLock<Vec<Arc<dyn ExpressionExtension>>> =
        std::sync::LazyLock::new(|| {
            vec![
                Arc::new(IntOp {
                    symbol: "dist",
                    id: "test.dist",
                    kind: ExtensionKind::prefix_expression(2),
                }),
                Arc::new(IntOp {
                    symbol: "zero",
                    id: "test.zero",
                    kind: ExtensionKind::atomic_expression(),
                }),
                Arc::new(IntOp {
                    symbol: "plus",
                    id: "test.plus",
                    kind: ExtensionKind::infix_expression(true),
                }),
                Arc::new(IntOp {
                    symbol: "minus",
                    id: "test.minus",
                    kind: ExtensionKind::infix_expression(false),
                }),
            ]
        });
    OPS.iter()
        .find(|op| op.symbol() == symbol)
        .expect("a test operator")
        .clone()
}

fn dist_ext() -> Arc<dyn ExpressionExtension> {
    int_op("dist")
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

fn even_ext() -> Arc<dyn PredicateExtension> {
    static EVEN: std::sync::LazyLock<Arc<dyn PredicateExtension>> =
        std::sync::LazyLock::new(|| Arc::new(Even));
    EVEN.clone()
}

fn extended_factory() -> FormulaFactory {
    FormulaFactory::with_extensions([Extension::Expr(dist_ext()), Extension::Pred(even_ext())])
        .expect("valid extension set")
}

fn zero_ext() -> Arc<dyn ExpressionExtension> {
    int_op("zero")
}

fn plus_ext() -> Arc<dyn ExpressionExtension> {
    int_op("plus")
}

fn minus_ext() -> Arc<dyn ExpressionExtension> {
    int_op("minus")
}

/// The tag `ff` registered `symbol` under.
fn tag_of(ff: &FormulaFactory, symbol: &str) -> tag::Tag {
    ff.extension_by_symbol(symbol).expect("registered").0
}

/// The factory the parsing and printing tests build against: `dist`,
/// `even`, `zero`, `plus` and `minus`.
pub(crate) fn parse_factory() -> FormulaFactory {
    FormulaFactory::with_extensions([
        Extension::Expr(dist_ext()),
        Extension::Pred(even_ext()),
        Extension::Expr(zero_ext()),
        Extension::Expr(plus_ext()),
        Extension::Expr(minus_ext()),
    ])
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
    assert_eq!(action.factory(), &ff);

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

// --- lookup by symbol ---

#[test]
fn factory_finds_extension_by_symbol() {
    let ff = extended_factory();
    let (tag, ext) = ff.extension_by_symbol("dist").expect("registered");
    assert_eq!(ext.common().id(), "test.dist");
    assert_eq!(
        ff.extension(tag).map(|by_tag| by_tag.common().id()),
        Some(ext.common().id()),
        "the tag and the symbol index agree"
    );
    let (_, even) = ff.extension_by_symbol("even").expect("registered");
    assert_eq!(even.common().id(), "test.even");
    assert!(
        ff.extension_by_symbol("card").is_none(),
        "builtins are not extensions"
    );
    assert!(
        ff.extension_by_symbol("Dist").is_none(),
        "symbols are exact-case"
    );
    assert!(
        FormulaFactory::default_factory()
            .extension_by_symbol("dist")
            .is_none()
    );
}

#[test]
fn factory_identity_is_hashable() {
    use std::collections::HashSet;
    let set: HashSet<FormulaFactory> = [
        extended_factory(),
        extended_factory(),
        FormulaFactory::default_factory(),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        set.len(),
        2,
        "interned factories hash and compare by identity"
    );
}

// --- resolving prefix and nullary operators while parsing ---

fn extended_tag(expr: &Expression) -> (tag::Tag, usize) {
    match expr.kind() {
        ExpressionKind::Extended { tag, exprs, preds } => {
            assert!(preds.is_empty());
            (*tag, exprs.len())
        }
        other => panic!("expected an extended expression, got {other:?}"),
    }
}

#[test]
fn prefix_operator_call_parses_to_extended_expression() {
    let ff = parse_factory();
    let dist_tag = tag_of(&ff, "dist");
    let expr = rossi::parse_expression_str_with("dist(a, b)", &ff).expect("parses");
    assert_eq!(extended_tag(&expr), (dist_tag, 2));
    assert_eq!(
        expr.span(),
        Some(rossi::formula::Span { start: 0, end: 10 })
    );

    // Nested and as an operand.
    let expr = rossi::parse_expression_str_with("dist(dist(a, b), c) + 1", &ff).expect("parses");
    let ExpressionKind::Associative { children, .. } = expr.kind() else {
        panic!("expected a sum, got {expr:?}");
    };
    let (_, arity) = extended_tag(&children[0]);
    assert_eq!(arity, 2);

    // The same text under the default factory is not an operator call.
    let error = rossi::parse_expression_str("dist(a, b)").expect_err("no such operator");
    assert!(
        matches!(&error, rossi::ParseError::NotAPrefixOperator { name, line: 1, column: 1, .. } if name == "dist"),
        "{error:?}"
    );
}

#[test]
fn nullary_operator_parses_bare() {
    let ff = parse_factory();
    let zero_tag = tag_of(&ff, "zero");
    let expr = rossi::parse_expression_str_with("zero", &ff).expect("parses");
    assert_eq!(extended_tag(&expr), (zero_tag, 0));
    let expr = rossi::parse_expression_str_with("zero + x", &ff).expect("parses");
    let ExpressionKind::Associative { children, .. } = expr.kind() else {
        panic!("expected a sum, got {expr:?}");
    };
    assert_eq!(extended_tag(&children[0]), (zero_tag, 0));
    assert!(matches!(children[1].kind(), ExpressionKind::FreeIdentifier(name) if name == "x"));

    // Under the default factory `zero` is an ordinary identifier.
    let expr = rossi::parse_expression_str("zero").expect("parses");
    assert!(matches!(expr.kind(), ExpressionKind::FreeIdentifier(name) if name == "zero"));
}

#[test]
fn predicate_operator_call_parses_to_extended_predicate() {
    let ff = parse_factory();
    let even_tag = tag_of(&ff, "even");
    let pred = rossi::parse_predicate_str_with("even(x) ∧ x > 0", &ff).expect("parses");
    let rossi::PredicateKind::Associative { children, .. } = pred.kind() else {
        panic!("expected a conjunction, got {pred:?}");
    };
    match children[0].kind() {
        rossi::PredicateKind::Extended { tag, exprs, preds } => {
            assert_eq!(*tag, even_tag);
            assert_eq!(exprs.len(), 1);
            assert!(preds.is_empty());
        }
        other => panic!("expected an extended predicate, got {other:?}"),
    }

    // Under the default factory the application stays unresolved.
    let pred = rossi::parse_predicate_str("even(x)").expect("parses");
    assert!(
        matches!(pred.kind(), rossi::PredicateKind::Application { .. }),
        "{pred:?}"
    );
}

/// An expression operator applied in predicate position is a reserved word,
/// like `dom(x)` is — never an unresolved user predicate application, which
/// would smuggle the operator symbol back in as a free identifier.
#[test]
fn expression_operator_in_predicate_position_is_rejected() {
    let ff = parse_factory();
    for text in ["dist(1, 2)", "plus(1, 2)", "zero(1)"] {
        let error = rossi::parse_predicate_str_with(text, &ff).expect_err(text);
        assert!(
            matches!(&error, rossi::ParseError::ReservedWord { .. }),
            "{text}: {error:?}"
        );
    }
}

#[test]
fn two_argument_call_without_operator_is_rejected() {
    let ff = parse_factory();
    for (text, head) in [
        ("f(a, b)", "f"),
        ("f(x)(a, b)", "f(x)"),
        ("(g)(a, b, c)", "(g)"),
    ] {
        for factory in [&ff, &FormulaFactory::default_factory()] {
            let error = rossi::parse_expression_str_with(text, factory).expect_err(text);
            match error {
                rossi::ParseError::NotAPrefixOperator {
                    name,
                    line,
                    column,
                    span,
                } => {
                    assert_eq!(name, head, "{text}");
                    assert_eq!((line, column), (1, 1), "{text}");
                    assert_eq!(
                        span,
                        Some(rossi::formula::Span {
                            start: 0,
                            end: head.len()
                        })
                    );
                }
                other => panic!("{text}: expected a not-a-prefix-operator error, got {other:?}"),
            }
        }
    }
    // A single argument is still core function application, whatever the head.
    let expr = rossi::parse_expression_str_with("f(a ↦ b)", &ff).expect("parses");
    assert!(matches!(
        expr.kind(),
        ExpressionKind::Binary {
            op: BinaryExprOp::FunImage,
            ..
        }
    ));
}

#[test]
fn arity_mismatch_is_reported() {
    let ff = parse_factory();
    for (text, name, expected, actual) in [
        ("dist(a)", "dist", "2", 1),
        ("dist(a, b, c)", "dist", "2", 3),
        ("dist", "dist", "2", 0),
        ("dist + 1", "dist", "2", 0),
    ] {
        let error = rossi::parse_expression_str_with(text, &ff).expect_err(text);
        assert!(
            matches!(&error, rossi::ParseError::ArityMismatch { name: n, expected: e, actual: a }
                if n == name && e == expected && *a == actual),
            "{text}: {error:?}"
        );
    }
    let error = rossi::parse_predicate_str_with("even(x, y)", &ff).expect_err("arity");
    assert!(
        matches!(&error, rossi::ParseError::ArityMismatch { name, expected, actual: 2 }
            if name == "even" && expected == "1"),
        "{error:?}"
    );
}

#[test]
fn operator_symbols_cannot_name_identifiers() {
    let ff = parse_factory();
    // Binders, declarations and a predicate symbol in expression position
    // are all the reserved-word error, as for the builtin words.
    for text in ["∀dist·dist = 1", "∃zero·zero > 0"] {
        let error = rossi::parse_predicate_str_with(text, &ff).expect_err(text);
        assert!(
            matches!(&error, rossi::ParseError::ReservedWord { .. }),
            "{text}: {error:?}"
        );
    }
    let error = rossi::parse_expression_str_with("even", &ff).expect_err("predicate symbol");
    assert!(matches!(&error, rossi::ParseError::ReservedWord { word, .. } if word == "even"));
    let error = rossi::parse_components_with("MACHINE M\nVARIABLES\n    dist\nEND\n", &ff)
        .expect_err("declaration");
    assert!(matches!(&error, rossi::ParseError::ReservedWord { word, .. } if word == "dist"));
    // The same names are free under the default factory.
    rossi::parse_predicate_str("∀dist·dist = 1").expect("ordinary identifier");
}

// --- infix operators ---

#[test]
fn infix_operator_parses_between_pair_and_arrows() {
    let ff = parse_factory();
    let plus_tag = tag_of(&ff, "plus");

    // Above the pair constructor: `a ↦ b plus c` is `a ↦ (b plus c)`.
    let expr = rossi::parse_expression_str_with("a ↦ b plus c", &ff).expect("parses");
    let ExpressionKind::Binary {
        op: BinaryExprOp::Mapsto,
        left,
        right,
    } = expr.kind()
    else {
        panic!("expected a maplet, got {expr:?}");
    };
    assert!(matches!(left.kind(), ExpressionKind::FreeIdentifier(name) if name == "a"));
    assert_eq!(extended_tag(right), (plus_tag, 2));
    assert_eq!(
        right.span(),
        Some(rossi::formula::Span { start: 6, end: 14 })
    );
    let expr = rossi::parse_expression_str_with("a plus b ↦ c", &ff).expect("parses");
    let ExpressionKind::Binary {
        op: BinaryExprOp::Mapsto,
        left,
        ..
    } = expr.kind()
    else {
        panic!("expected a maplet, got {expr:?}");
    };
    assert_eq!(extended_tag(left), (plus_tag, 2));

    // Applications, images and prefix operators are ordinary operands.
    let expr =
        rossi::parse_expression_str_with("f(x) plus g[y] plus dist(a, b)", &ff).expect("parses");
    assert_eq!(extended_tag(&expr), (plus_tag, 3));
    let expr = rossi::parse_expression_str_with("−a plus card(S)", &ff).expect("parses");
    assert_eq!(extended_tag(&expr), (plus_tag, 2));

    // In predicate position and in a set.
    let pred =
        rossi::parse_predicate_str_with("a plus b ∈ S ∧ {a plus b} ⊆ S", &ff).expect("parses");
    let rossi::PredicateKind::Associative { children, .. } = pred.kind() else {
        panic!("expected a conjunction, got {pred:?}");
    };
    let rossi::PredicateKind::Relational { left, .. } = children[0].kind() else {
        panic!("expected a membership, got {:?}", children[0]);
    };
    assert_eq!(extended_tag(left), (plus_tag, 2));

    // Under the default factory the word is not an operator, and a keyword
    // is never even tried as one.
    let error = rossi::parse_expression_str("a end b").expect_err("keyword");
    assert!(
        matches!(&error, rossi::ParseError::PestError { .. }),
        "{error:?}"
    );
    for text in ["a plus b", "a card b"] {
        let error = rossi::parse_expression_str(text).expect_err(text);
        assert!(
            matches!(
                &error,
                rossi::ParseError::UnknownInfixOperator {
                    line: 1,
                    column: 3,
                    ..
                }
            ),
            "{text}: {error:?}"
        );
    }
}

#[test]
fn associative_infix_chain_folds_flat() {
    let ff = parse_factory();
    let plus_tag = tag_of(&ff, "plus");
    let expr = rossi::parse_expression_str_with("a plus b plus c plus d", &ff).expect("parses");
    assert_eq!(extended_tag(&expr), (plus_tag, 4));
    assert_eq!(
        expr.span(),
        Some(rossi::formula::Span { start: 0, end: 22 })
    );
    // Parentheses keep their nesting.
    let expr = rossi::parse_expression_str_with("a plus (b plus c)", &ff).expect("parses");
    let ExpressionKind::Extended { exprs, .. } = expr.kind() else {
        panic!("expected an extended expression, got {expr:?}");
    };
    assert_eq!(exprs.len(), 2);
    assert_eq!(extended_tag(&exprs[1]), (plus_tag, 2));
}

#[test]
fn non_associative_infix_chain_is_rejected() {
    let ff = parse_factory();
    let minus_tag = tag_of(&ff, "minus");
    let expr = rossi::parse_expression_str_with("a minus b", &ff).expect("parses");
    assert_eq!(extended_tag(&expr), (minus_tag, 2));
    let expr = rossi::parse_expression_str_with("a minus (b minus c)", &ff).expect("parses");
    assert_eq!(extended_tag(&expr), (minus_tag, 2));
    for (text, left, right) in [
        ("a minus b minus c", "minus", "minus"),
        ("a plus b minus c", "plus", "minus"),
        ("a minus b plus c", "minus", "plus"),
    ] {
        let error = rossi::parse_expression_str_with(text, &ff).expect_err(text);
        assert!(
            matches!(&error, rossi::ParseError::IncompatibleOperators { left: l, right: r, .. }
                if l == left && r == right),
            "{text}: {error:?}"
        );
    }
}

#[test]
fn infix_operand_at_set_level_needs_parentheses() {
    let ff = parse_factory();
    for text in [
        "a plus b + c",
        "a + b plus c",
        "a ∪ b plus c",
        "S plus T → U",
        "a plus b ∗ c",
        "a plus b ‥ c",
        "a plus b ^ c",
    ] {
        let error = rossi::parse_expression_str_with(text, &ff).expect_err(text);
        assert!(
            matches!(&error, rossi::ParseError::IncompatibleOperators { .. }),
            "{text}: {error:?}"
        );
    }
    for text in [
        "a plus (b + c)",
        "(a ∪ b) plus c",
        "S plus (T → U)",
        "a plus b ↦ c ∗ d",
    ] {
        rossi::parse_expression_str_with(text, &ff).unwrap_or_else(|e| panic!("{text}: {e}"));
    }
}

#[test]
fn keyword_after_operand_is_never_an_infix_operator() {
    let ff = parse_factory();
    let plus_tag = tag_of(&ff, "plus");
    let source = "\
MACHINE M
VARIABLES
    x y
INVARIANTS
    @inv1 x ∈ ℤ ∧ y ∈ ℤ
    @inv2 x = 1 or y plus x = 2
VARIANT
    x plus y
EVENTS
EVENT INITIALISATION
THEN
    @a x ≔ 0
    @b y ≔ 0
END
EVENT e
ANY
    p
WHERE
    @g p = x plus y
THEN
    @a x ≔ x plus p
END
END
";
    let components = rossi::parse_components_with(source, &ff).expect("parses");
    let rossi::Component::Machine(m) = &components[0] else {
        panic!("expected a machine");
    };
    let variant = &m.variants[0].expression;
    assert_eq!(extended_tag(variant), (plus_tag, 2));
    assert!(matches!(
        m.invariants[1].predicate.kind(),
        rossi::PredicateKind::Associative { .. }
    ));
    assert_eq!(m.events[0].parameters.len(), 1);
    assert_eq!(m.events[0].guards.len(), 1);
    assert_eq!(m.events[0].actions.len(), 1);
    // Under the default factory the same machine, minus the operator uses,
    // is unaffected: every word after an operand is the keyword it was.
    let plain = source
        .replace(" plus y", "")
        .replace("y plus x", "y")
        .replace(" plus p", "");
    rossi::parse_components(&plain).expect("keywords still close every formula");
}
