//! Property-based roundtrip tests for the Event-B parser and printer.
//!
//! Core invariant: for any generated formula-model tree, pretty-printing and
//! re-parsing produce an equal tree (structural equality — spans are
//! positional metadata and declaration names compare alpha-structurally).
//! This catches edge cases in precedence, parenthesization, operator
//! rendering, and binder-name resolution.
//!
//! Generation is canonical by construction: same-operator associative
//! children are spliced flat (the parser never produces a nested
//! same-operator chain), set extensions are non-empty, and comprehension
//! values take the shapes their print forms require.

mod common;

use proptest::prelude::*;
use rossi::formula::tag::{
    AssocExprOp, AssocPredOp, AtomicOp, BinaryExprOp, BinaryPredOp, LiteralPredOp, QuantExprOp,
    QuantPredOp, RelationalOp, UnaryExprOp,
};
use rossi::formula::{BoundIdentDecl, FormulaFactory};
use rossi::{
    ActionBody, Assignment, Component, Context, Event, EventStatus, Expression, ExpressionKind,
    Form, InitialisationEvent, LabeledAction, LabeledPredicate, Machine, NamedElement, Predicate,
    PredicateKind, PrettyPrinter, Style, Variant, parse,
};

fn ff() -> FormulaFactory {
    FormulaFactory::default_factory()
}

// =============================================================================
// Identifier strategies — fixed pools of safe names (no keyword collisions)
// =============================================================================

fn arb_identifier() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("aa".into()),
        Just("bb".into()),
        Just("cc".into()),
        Just("dd".into()),
        Just("ee".into()),
        Just("ff".into()),
        Just("gg".into()),
        Just("hh".into()),
        Just("xx".into()),
        Just("yy".into()),
        Just("zz".into()),
        Just("v1".into()),
        Just("v2".into()),
        Just("v3".into()),
        Just("s1".into()),
        Just("s2".into()),
    ]
}

/// The type spellings a `⦂` may carry: `ℤ`, `BOOL`, a name read as a given
/// set, `ℙ(τ)`, `τ × τ` and `τ ↔ τ`. The parser refuses anything else on the
/// right of a `⦂`, so both the annotation and the ascription draw from here.
///
/// Spelled out rather than derived from `Type::to_expression`, which cannot
/// write `↔` (there is no `Type::Rel` — `Type::relation` folds to `ℙ(·×·)`)
/// and stamps its identifiers with a solved type these trees leave `None`.
fn arb_type_expression() -> impl Strategy<Value = Expression> {
    let leaf = prop_oneof![
        Just(ff().atomic_expression(AtomicOp::Integer, None, None)),
        Just(ff().atomic_expression(AtomicOp::Bool, None, None)),
        arb_identifier().prop_map(|name| ff().free_identifier(&name, None, None)),
    ];
    leaf.prop_recursive(3, 8, 2, |inner| {
        prop_oneof![
            inner
                .clone()
                .prop_map(|child| ff().unary_expression(UnaryExprOp::Pow, child, None)),
            (
                prop_oneof![Just(BinaryExprOp::CProd), Just(BinaryExprOp::Rel)],
                inner.clone(),
                inner,
            )
                .prop_map(|(op, left, right)| ff().binary_expression(op, left, right, None)),
        ]
    })
}

/// Disjoint pool for bound declarations in quantified constructs, with both
/// bare (`p1`) and annotated (`p1⦂ℤ`) declarations. Bodies are drawn from
/// the free pool, which never mentions these names, so binders are unused
/// and scoping is trivially correct; the with-replacement vectors below can
/// repeat a name, giving the printer's hint freshening real coverage.
fn arb_bound_decl() -> impl Strategy<Value = BoundIdentDecl> {
    fn bare(name: &str) -> BoundIdentDecl {
        ff().bound_ident_decl(name, None, None, None)
    }
    // One type strategy, shared by the annotated arms: `prop_recursive`
    // rebuilds its closure per sample, so building it three times would
    // triple that work for no extra coverage.
    let ty = arb_type_expression().boxed();
    let annotated = |name: &'static str| {
        ty.clone()
            .prop_map(move |annotation| ff().bound_ident_decl(name, None, Some(annotation), None))
    };
    prop_oneof![
        Just(bare("p1")),
        Just(bare("p2")),
        Just(bare("p3")),
        Just(bare("q1")),
        Just(bare("q2")),
        Just(bare("q3")),
        annotated("p1"),
        annotated("p2"),
        annotated("q1"),
    ]
}

// =============================================================================
// Operator strategies
// =============================================================================

/// True binary operators. The associative family generates n-ary nodes
/// instead, `⦂` generates an ascription node, and FunImage/RelImage have
/// dedicated application branches.
fn arb_binary_op() -> impl Strategy<Value = BinaryExprOp> {
    prop_oneof![
        Just(BinaryExprOp::Mapsto),
        Just(BinaryExprOp::Rel),
        Just(BinaryExprOp::TRel),
        Just(BinaryExprOp::SRel),
        Just(BinaryExprOp::STRel),
        Just(BinaryExprOp::PFun),
        Just(BinaryExprOp::TFun),
        Just(BinaryExprOp::PInj),
        Just(BinaryExprOp::TInj),
        Just(BinaryExprOp::PSur),
        Just(BinaryExprOp::TSur),
        Just(BinaryExprOp::TBij),
        Just(BinaryExprOp::SetMinus),
        Just(BinaryExprOp::CProd),
        Just(BinaryExprOp::DProd),
        Just(BinaryExprOp::PProd),
        Just(BinaryExprOp::DomRes),
        Just(BinaryExprOp::DomSub),
        Just(BinaryExprOp::RanRes),
        Just(BinaryExprOp::RanSub),
        Just(BinaryExprOp::UpTo),
        Just(BinaryExprOp::Minus),
        Just(BinaryExprOp::Div),
        Just(BinaryExprOp::Mod),
        Just(BinaryExprOp::Expn),
    ]
}

fn arb_assoc_op() -> impl Strategy<Value = AssocExprOp> {
    prop_oneof![
        Just(AssocExprOp::Plus),
        Just(AssocExprOp::Mul),
        Just(AssocExprOp::BUnion),
        Just(AssocExprOp::BInter),
        Just(AssocExprOp::Ovr),
        Just(AssocExprOp::FComp),
        Just(AssocExprOp::BComp),
    ]
}

fn arb_unary_op() -> impl Strategy<Value = UnaryExprOp> {
    prop_oneof![
        Just(UnaryExprOp::UnMinus),
        Just(UnaryExprOp::Pow),
        Just(UnaryExprOp::Pow1),
        Just(UnaryExprOp::KDom),
        Just(UnaryExprOp::KRan),
        Just(UnaryExprOp::Converse),
        // The closed builtins are unary operators in the model.
        Just(UnaryExprOp::KCard),
        Just(UnaryExprOp::KMin),
        Just(UnaryExprOp::KMax),
        Just(UnaryExprOp::KUnion),
        Just(UnaryExprOp::KInter),
    ]
}

fn arb_relational_op() -> impl Strategy<Value = RelationalOp> {
    prop_oneof![
        Just(RelationalOp::Equal),
        Just(RelationalOp::NotEqual),
        Just(RelationalOp::Lt),
        Just(RelationalOp::Le),
        Just(RelationalOp::Gt),
        Just(RelationalOp::Ge),
        Just(RelationalOp::In),
        Just(RelationalOp::NotIn),
        Just(RelationalOp::SubsetEq),
        Just(RelationalOp::Subset),
    ]
}

/// Atomic expressions. `TRUE`/`FALSE` are excluded: their printed form is
/// ambiguous with the predicate literals at parse boundaries (e.g. as a
/// comparison operand).
fn arb_atomic() -> impl Strategy<Value = AtomicOp> {
    prop_oneof![
        Just(AtomicOp::EmptySet),
        Just(AtomicOp::Natural),
        Just(AtomicOp::Natural1),
        Just(AtomicOp::Integer),
        Just(AtomicOp::Bool),
        Just(AtomicOp::KIdGen),
        Just(AtomicOp::KPrj1Gen),
        Just(AtomicOp::KPrj2Gen),
        Just(AtomicOp::KPred),
        Just(AtomicOp::KSucc),
    ]
}

// =============================================================================
// Canonical constructors — splice same-operator associative children so a
// generated tree always has the shape the parser itself produces.
// =============================================================================

fn assoc_expr(op: AssocExprOp, children: Vec<Expression>) -> Expression {
    let mut flat: Vec<Expression> = Vec::new();
    for child in children {
        match child.kind() {
            ExpressionKind::Associative {
                op: nested_op,
                children: nested,
            } if *nested_op == op => flat.extend(nested.iter().cloned()),
            _ => flat.push(child.clone()),
        }
    }
    ff().associative_expression(op, flat, None)
}

fn assoc_pred(op: AssocPredOp, children: Vec<Predicate>) -> Predicate {
    let mut flat: Vec<Predicate> = Vec::new();
    for child in children {
        match child.kind() {
            PredicateKind::Associative {
                op: nested_op,
                children: nested,
            } if *nested_op == op => flat.extend(nested.iter().cloned()),
            _ => flat.push(child.clone()),
        }
    }
    ff().associative_predicate(op, flat, None)
}

// =============================================================================
// Expression strategy (recursive, depth-limited)
// =============================================================================

/// Leaf expressions without the `bool(...)` branch — also used as comparison
/// operands inside `bool` below, to avoid unbounded mutual recursion between
/// the expression and predicate strategies.
fn arb_bool_free_leaf_expression() -> impl Strategy<Value = Expression> {
    prop_oneof![
        (0i64..1000).prop_map(|n| ff().integer_literal(n, None)),
        arb_identifier().prop_map(|name| ff().free_identifier(&name, None, None)),
        arb_atomic().prop_map(|op| ff().atomic_expression(op, None, None)),
    ]
}

fn arb_leaf_expression() -> impl Strategy<Value = Expression> {
    prop_oneof![
        8 => arb_bool_free_leaf_expression(),
        // bool(P) — leaf predicates only, to keep recursion bounded.
        1 => prop_oneof![
            Just(ff().literal_predicate(LiteralPredOp::BTrue, None)),
            Just(ff().literal_predicate(LiteralPredOp::BFalse, None)),
            (
                arb_relational_op(),
                arb_bool_free_leaf_expression(),
                arb_bool_free_leaf_expression()
            )
                .prop_map(|(op, left, right)| ff().relational_predicate(op, left, right, None)),
        ]
        .prop_map(|pred| ff().bool_expression(pred, None)),
    ]
}

fn arb_leaf_predicate() -> impl Strategy<Value = Predicate> {
    prop_oneof![
        Just(ff().literal_predicate(LiteralPredOp::BTrue, None)),
        Just(ff().literal_predicate(LiteralPredOp::BFalse, None)),
        (
            arb_relational_op(),
            arb_leaf_expression(),
            arb_leaf_expression()
        )
            .prop_map(|(op, left, right)| ff().relational_predicate(op, left, right, None)),
    ]
}

fn arb_expression_impl(depth: u32, desired_size: u32) -> impl Strategy<Value = Expression> {
    arb_leaf_expression().prop_recursive(depth, desired_size, 8, |inner| {
        prop_oneof![
            // True binary operators.
            (arb_binary_op(), inner.clone(), inner.clone())
                .prop_map(|(op, left, right)| ff().binary_expression(op, left, right, None)),
            // Associative operators, canonical by construction.
            (
                arb_assoc_op(),
                proptest::collection::vec(inner.clone(), 2..4)
            )
                .prop_map(|(op, children)| assoc_expr(op, children)),
            // Unary operators (including the closed builtins).
            (arb_unary_op(), inner.clone())
                .prop_map(|(op, child)| ff().unary_expression(op, child, None)),
            // Set extension (non-empty; the empty set is the ∅ atom).
            proptest::collection::vec(inner.clone(), 1..4)
                .prop_map(|members| ff().set_extension(members, None)),
            // Function application f(x): the function side is a plain name.
            (arb_identifier(), inner.clone()).prop_map(|(name, argument)| {
                ff().binary_expression(
                    BinaryExprOp::FunImage,
                    ff().free_identifier(&name, None, None),
                    argument,
                    None,
                )
            }),
            // Relational image r[S].
            (inner.clone(), inner.clone()).prop_map(|(relation, set)| {
                ff().binary_expression(BinaryExprOp::RelImage, relation, set, None)
            }),
            // Type ascription e ⦂ T, whose right operand must denote a type.
            (inner.clone(), arb_type_expression())
                .prop_map(|(expr, ty)| ff().ascription(expr, ty, None)),
            // Ident-list comprehension {ids ∣ P}: the value is the binder
            // chain in declaration order.
            (
                proptest::collection::vec(arb_bound_decl(), 1..3),
                arb_leaf_predicate(),
            )
                .prop_map(|(decls, pred)| {
                    let value = ff().bound_ident_chain(decls.len());
                    ff().quantified_expression(
                        QuantExprOp::CSet,
                        decls,
                        pred,
                        value,
                        None,
                        Form::IdentList,
                    )
                }),
            // Explicit comprehension {ids · P ∣ E}.
            (
                proptest::collection::vec(arb_bound_decl(), 1..3),
                arb_leaf_predicate(),
                inner.clone(),
            )
                .prop_map(|(decls, pred, value)| {
                    ff().quantified_expression(
                        QuantExprOp::CSet,
                        decls,
                        pred,
                        value,
                        None,
                        Form::Explicit,
                    )
                }),
            // Lambda λ pattern · P ∣ body: the value pairs the binder chain
            // with the body.
            (
                proptest::collection::vec(arb_bound_decl(), 1..3),
                arb_leaf_predicate(),
                inner.clone(),
            )
                .prop_map(|(decls, pred, body)| {
                    let pattern = ff().bound_ident_chain(decls.len());
                    let value = ff().binary_expression(BinaryExprOp::Mapsto, pattern, body, None);
                    ff().quantified_expression(
                        QuantExprOp::CSet,
                        decls,
                        pred,
                        value,
                        None,
                        Form::Lambda,
                    )
                }),
            // Quantified union / intersection.
            (
                prop_oneof![Just(QuantExprOp::QUnion), Just(QuantExprOp::QInter)],
                proptest::collection::vec(arb_bound_decl(), 1..3),
                arb_leaf_predicate(),
                inner,
            )
                .prop_map(|(op, decls, pred, value)| {
                    ff().quantified_expression(op, decls, pred, value, None, Form::Explicit)
                }),
        ]
    })
}

fn arb_expression() -> impl Strategy<Value = Expression> {
    arb_expression_impl(4, 64)
}

// =============================================================================
// Predicate strategy (recursive, depth-limited)
// =============================================================================

fn arb_predicate() -> impl Strategy<Value = Predicate> {
    arb_leaf_predicate().prop_recursive(3, 32, 4, |inner| {
        prop_oneof![
            // Conjunction / disjunction, canonical by construction.
            (
                prop_oneof![Just(AssocPredOp::LAnd), Just(AssocPredOp::LOr)],
                proptest::collection::vec(inner.clone(), 2..4)
            )
                .prop_map(|(op, children)| assoc_pred(op, children)),
            // Implication / equivalence.
            (
                prop_oneof![Just(BinaryPredOp::LImp), Just(BinaryPredOp::LEqv)],
                inner.clone(),
                inner.clone()
            )
                .prop_map(|(op, left, right)| ff().binary_predicate(op, left, right, None)),
            // Negation.
            inner
                .clone()
                .prop_map(|child| ff().not_predicate(child, None)),
            // Comparison over recursive expressions.
            (arb_relational_op(), arb_expression(), arb_expression())
                .prop_map(|(op, left, right)| ff().relational_predicate(op, left, right, None)),
            // Quantified predicate.
            (
                prop_oneof![Just(QuantPredOp::Forall), Just(QuantPredOp::Exists)],
                proptest::collection::vec(arb_bound_decl(), 1..3),
                inner.clone(),
            )
                .prop_map(|(op, decls, pred)| ff().quantified_predicate(op, decls, pred, None)),
            // finite(S).
            arb_leaf_expression().prop_map(|expr| ff().simple_predicate(expr, None)),
            // partition(S, A, B, …).
            proptest::collection::vec(arb_leaf_expression(), 2..5)
                .prop_map(|args| ff().multiple_predicate(args, None)),
            // User-defined predicate application foo(x, y).
            (
                arb_identifier(),
                proptest::collection::vec(arb_leaf_expression(), 1..3)
            )
                .prop_map(|(function, args)| {
                    ff().predicate_application(&function, None, args, None)
                }),
        ]
    })
}

// =============================================================================
// Action strategy
// =============================================================================

/// Expression strategy for assignment right-hand sides. Forward composition
/// `;` stays in the pool: the printer parenthesizes any action sub-part
/// containing a top-level `;`, which this exercises.
fn arb_action_expression() -> impl Strategy<Value = Expression> {
    arb_expression_impl(3, 32)
}

fn arb_action_predicate() -> impl Strategy<Value = Predicate> {
    prop_oneof![
        Just(ff().literal_predicate(LiteralPredOp::BTrue, None)),
        Just(ff().literal_predicate(LiteralPredOp::BFalse, None)),
        (
            arb_relational_op(),
            arb_action_expression(),
            arb_action_expression()
        )
            .prop_map(|(op, left, right)| ff().relational_predicate(op, left, right, None)),
    ]
}

/// Distinct assignment targets (the machine wrapper declares them).
fn arb_targets() -> impl Strategy<Value = Vec<String>> {
    proptest::sample::subsequence(
        vec![
            "aa".to_string(),
            "bb".to_string(),
            "cc".to_string(),
            "dd".to_string(),
        ],
        1..4,
    )
}

fn target_idents(names: &[String]) -> Vec<Expression> {
    names
        .iter()
        .map(|name| ff().free_identifier(name, None, None))
        .collect()
}

fn arb_assignment() -> impl Strategy<Value = (Assignment, Vec<String>)> {
    prop_oneof![
        // x, y ≔ E, F
        (
            arb_targets(),
            proptest::collection::vec(arb_action_expression(), 3)
        )
            .prop_map(|(names, values)| {
                let values = values.into_iter().take(names.len()).collect();
                (
                    ff().becomes_equal_to(target_idents(&names), values, None),
                    names,
                )
            }),
        // x, y :∈ S
        (arb_targets(), arb_action_expression()).prop_map(|(names, set)| {
            (
                ff().becomes_member_of(target_idents(&names), set, None),
                names,
            )
        }),
        // x, y :∣ P — one primed declaration per target.
        (arb_targets(), arb_action_predicate()).prop_map(|(names, pred)| {
            let primed = names
                .iter()
                .map(|name| ff().bound_ident_decl(format!("{name}'"), None, None, None))
                .collect();
            (
                ff().becomes_such_that(target_idents(&names), primed, pred, None),
                names,
            )
        }),
    ]
}

fn arb_action() -> impl Strategy<Value = (ActionBody, Vec<String>)> {
    arb_assignment().prop_map(|(assignment, names)| (ActionBody::Assignment(assignment), names))
}

// =============================================================================
// Component-level strategies
// =============================================================================

fn arb_carrier_set() -> impl Strategy<Value = NamedElement> {
    prop_oneof![Just("SS"), Just("TT"), Just("UU")].prop_map(|name| NamedElement::new(name.into()))
}

/// Generate a label from a fixed pool. Always returns `Some(label)`: the
/// grammar requires one on every predicate and action, so a `None` here would
/// print text the parser refuses to read back.
fn arb_label() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        Just(Some("axm1".into())),
        Just(Some("axm2".into())),
        Just(Some("inv1".into())),
        Just(Some("inv2".into())),
        Just(Some("thm1".into())),
        Just(Some("grd1".into())),
        Just(Some("act1".into())),
        Just(Some("act2".into())),
    ]
}

fn arb_axiom() -> impl Strategy<Value = LabeledPredicate> {
    (arb_label(), arb_leaf_predicate()).prop_map(|(label, predicate)| LabeledPredicate {
        label,
        is_theorem: false,
        predicate,
        span: None,
        comment: None,
    })
}

fn arb_theorem() -> impl Strategy<Value = LabeledPredicate> {
    (arb_label(), arb_leaf_predicate()).prop_map(|(label, predicate)| LabeledPredicate {
        label,
        is_theorem: true,
        predicate,
        span: None,
        comment: None,
    })
}

fn arb_labeled_action() -> impl Strategy<Value = LabeledAction> {
    (arb_label(), arb_action()).prop_map(|(label, (action, _vars))| LabeledAction {
        label,
        action,
        span: None,
        comment: None,
    })
}

fn arb_event_name() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("evt1".into()),
        Just("evt2".into()),
        Just("evt3".into()),
    ]
}

fn arb_event_status() -> impl Strategy<Value = Option<EventStatus>> {
    // Note: Some(EventStatus::Ordinary) is excluded because the pretty printer
    // omits STATUS for ordinary events (it's the default), so roundtrip would
    // produce None instead of Some(Ordinary).
    prop_oneof![
        Just(None),
        Just(Some(EventStatus::Convergent)),
        Just(Some(EventStatus::Anticipated)),
    ]
}

/// Generate a labeled predicate for WITH/WITNESS clauses.
fn arb_witness_predicate() -> impl Strategy<Value = LabeledPredicate> {
    (arb_label(), arb_leaf_predicate()).prop_map(|(label, predicate)| LabeledPredicate {
        label,
        is_theorem: false,
        predicate,
        span: None,
        comment: None,
    })
}

fn arb_event() -> impl Strategy<Value = Event> {
    (
        arb_event_name(),
        arb_event_status(),
        // Refines target count: 0 = a new event, 2+ = a merging one.
        0usize..3,
        proptest::sample::subsequence(
            vec!["p1".to_string(), "p2".to_string(), "p3".to_string()],
            0..3,
        ),
        proptest::collection::vec(arb_axiom(), 0..3),
        proptest::collection::vec(arb_witness_predicate(), 0..2),
        proptest::collection::vec(arb_witness_predicate(), 0..2),
        proptest::collection::vec(arb_labeled_action(), 0..3),
    )
        .prop_map(
            |(name, status, refines_targets, parameters, guards, with, witnesses, actions)| {
                let mut event = Event::new(name.clone());
                event.status = status;
                if refines_targets > 0 {
                    event.refines = (0..refines_targets)
                        .map(|i| NamedElement::new(format!("{name}_abs{i}")))
                        .collect();
                    event.with = with;
                }
                event.parameters = parameters.into_iter().map(NamedElement::new).collect();
                event.guards = guards;
                event.witnesses = witnesses;
                event.actions = actions;
                event
            },
        )
}

fn arb_initialisation() -> impl Strategy<Value = InitialisationEvent> {
    proptest::collection::vec(arb_labeled_action(), 1..3).prop_map(|actions| InitialisationEvent {
        actions,
        comment: None,
        extended: false,
        with: Vec::new(),
        witnesses: Vec::new(),
        span: None,
        name_span: None,
    })
}

fn arb_context() -> impl Strategy<Value = Component> {
    (
        proptest::collection::vec(arb_carrier_set(), 0..3),
        proptest::collection::vec(arb_identifier(), 0..4),
        proptest::collection::vec(arb_axiom(), 0..3),
        proptest::collection::vec(arb_theorem(), 0..2),
    )
        .prop_map(|(sets, constants, mut axioms, theorems)| {
            let mut ctx = Context::new("PropCtx".into());
            ctx.sets = sets;
            ctx.constants = constants.into_iter().map(NamedElement::new).collect();
            axioms.extend(theorems);
            ctx.axioms = axioms;
            Component::Context(ctx)
        })
}

fn arb_machine() -> impl Strategy<Value = Component> {
    (
        // REFINES is a single optional machine name (Machine::refines is
        // Option<String>), never a list.
        proptest::option::of(prop_oneof![
            Just("abs1".to_string()),
            Just("abs2".to_string())
        ]),
        // SEES is a list of 0..3 distinct context names.
        proptest::sample::subsequence(
            vec!["ctx1".to_string(), "ctx2".to_string(), "ctx3".to_string()],
            0..3,
        ),
        proptest::collection::vec(arb_identifier(), 0..4),
        proptest::collection::vec(arb_axiom(), 0..3),
        proptest::collection::vec(arb_theorem(), 0..2),
        // VARIANT items: an optional unlabeled first item, then labeled
        // ones — the only shapes the text grammar can round-trip.
        (
            proptest::option::of(arb_leaf_expression()),
            proptest::collection::vec((arb_label(), arb_leaf_expression()), 0..2),
        ),
        proptest::option::of(arb_initialisation()),
        proptest::collection::vec(arb_event(), 0..3),
    )
        .prop_map(
            |(
                refines,
                sees,
                variables,
                mut invariants,
                theorems,
                (first_variant, labeled_variants),
                initialisation,
                events,
            )| {
                let mut machine = Machine::new("PropMch".into());
                machine.refines = refines;
                machine.sees = sees;
                machine.variables = variables.into_iter().map(NamedElement::new).collect();
                invariants.extend(theorems);
                machine.invariants = invariants;
                machine.variants = first_variant
                    .into_iter()
                    .map(|expression| Variant {
                        label: None,
                        expression,
                        span: None,
                        comment: None,
                    })
                    .chain(
                        labeled_variants
                            .into_iter()
                            .map(|(label, expression)| Variant {
                                label,
                                expression,
                                span: None,
                                comment: None,
                            }),
                    )
                    .collect();
                machine.initialisation = initialisation;
                machine.events = events;
                Component::Machine(machine)
            },
        )
}

// =============================================================================
// Wrappers — embed generated trees in minimal parseable Components
// =============================================================================

/// Wrap an expression in a Context axiom: `axm1: propvar = <expr>`
fn wrap_expression_in_context(expr: &Expression) -> Component {
    let mut ctx = Context::new("proptest".into());
    ctx.constants = vec![NamedElement::new("propvar".to_string())];
    let axiom = ff().relational_predicate(
        RelationalOp::Equal,
        ff().free_identifier("propvar", None, None),
        expr.clone(),
        None,
    );
    ctx.axioms = vec![LabeledPredicate {
        label: Some("axm1".into()),
        is_theorem: false,
        predicate: axiom,
        span: None,
        comment: None,
    }];
    Component::Context(ctx)
}

/// Wrap a predicate in a Context axiom: `axm1: <pred>`
fn wrap_predicate_in_context(pred: &Predicate) -> Component {
    let mut ctx = Context::new("proptest".into());
    ctx.axioms = vec![LabeledPredicate {
        label: Some("axm1".into()),
        is_theorem: false,
        predicate: pred.clone(),
        span: None,
        comment: None,
    }];
    Component::Context(ctx)
}

/// Wrap an action in a Machine event.
fn wrap_action_in_machine(action: &ActionBody, variables: &[String]) -> Component {
    let mut machine = Machine::new("proptest".into());
    machine.variables = variables
        .iter()
        .map(|v| NamedElement::new(v.clone()))
        .collect();
    machine.events = vec![Event::new("test_event".into())];
    machine.events[0].actions = vec![LabeledAction {
        label: Some("act1".into()),
        action: action.clone(),
        span: None,
        comment: None,
    }];
    Component::Machine(machine)
}

// =============================================================================
// Roundtrip assertion helpers
// =============================================================================

/// Run a property over `cases` random values of the built strategy, with
/// everything — strategy construction, value generation, shrinking, and the
/// property body — on a thread with a 16 MiB stack.
///
/// The `proptest!` macro would run all of that on the default 2 MiB libtest
/// thread, which can overflow in debug builds: strategy generation recurses
/// through the whole combinator chain per AST level, and the
/// recursive-descent reparse has a comparable per-level cost (cf.
/// `test_deep_predicate_fits_in_small_stack` in simple_predicate_test.rs).
fn check_roundtrip_property<S: Strategy>(
    cases: u32,
    make_strategy: impl FnOnce() -> S + Send,
    property: impl Fn(&S::Value) + Send,
) {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn_scoped(scope, move || {
                let mut config = ProptestConfig::with_cases(cases);
                config.source_file = Some(file!());
                let mut runner = proptest::test_runner::TestRunner::new(config);
                runner
                    .run(&make_strategy(), |value| {
                        property(&value);
                        Ok(())
                    })
                    .unwrap_or_else(|failure| panic!("{failure}"));
            })
            .expect("failed to spawn property thread")
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    });
}

/// Print a Component with the given printer, re-parse, and assert ASTs match.
fn assert_component_roundtrip(original: &Component, printer: &PrettyPrinter) {
    let mode = if printer.use_unicode {
        "Unicode"
    } else {
        "ASCII"
    };
    let printed = printer.print_component(original);
    let mut reparsed = match parse(&printed) {
        Ok(c) => c,
        Err(e) => panic!(
            "Failed to parse printed output ({mode}):\n{e}\n\nPrinted:\n{printed}\n\nOriginal AST:\n{original:#?}"
        ),
    };
    let mut expected = original.clone();
    common::clear_spans(&mut expected);
    common::clear_spans(&mut reparsed);
    assert_eq!(
        expected, reparsed,
        "{mode} roundtrip mismatch.\nPrinted:\n{printed}\n\nOriginal AST:\n{original:#?}"
    );
}

// =============================================================================
// Property tests (via check_roundtrip_property — see its doc comment for why
// these do not use the proptest! macro)
// =============================================================================

// --- Expression roundtrips ---

#[test]
fn expression_roundtrip_unicode() {
    check_roundtrip_property(500, arb_expression, |expr| {
        let component = wrap_expression_in_context(expr);
        assert_component_roundtrip(&component, &PrettyPrinter::new());
    });
}

#[test]
fn expression_roundtrip_ascii() {
    check_roundtrip_property(500, arb_expression, |expr| {
        let component = wrap_expression_in_context(expr);
        assert_component_roundtrip(&component, &PrettyPrinter::ascii());
    });
}

// --- Predicate roundtrips ---

#[test]
fn predicate_roundtrip_unicode() {
    check_roundtrip_property(500, arb_predicate, |pred| {
        let component = wrap_predicate_in_context(pred);
        assert_component_roundtrip(&component, &PrettyPrinter::new());
    });
}

#[test]
fn predicate_roundtrip_ascii() {
    check_roundtrip_property(500, arb_predicate, |pred| {
        let component = wrap_predicate_in_context(pred);
        assert_component_roundtrip(&component, &PrettyPrinter::ascii());
    });
}

// --- Action roundtrips ---

#[test]
fn action_roundtrip_unicode() {
    check_roundtrip_property(500, arb_action, |(action, vars)| {
        let component = wrap_action_in_machine(action, vars);
        assert_component_roundtrip(&component, &PrettyPrinter::new());
    });
}

#[test]
fn action_roundtrip_ascii() {
    check_roundtrip_property(500, arb_action, |(action, vars)| {
        let component = wrap_action_in_machine(action, vars);
        assert_component_roundtrip(&component, &PrettyPrinter::ascii());
    });
}

// Standalone-action roundtrip: guards the `action ⊆ standalone_action`
// superset property (a new action form added to the text grammar but
// forgotten in `standalone_action` would fail here, not first in a
// Rodin-import regression) and gives parse_action_str + the printer's
// `;`-guard generative coverage.
#[test]
fn action_roundtrip_standalone() {
    check_roundtrip_property(500, arb_action, |(action, _vars)| {
        let printed = PrettyPrinter::new().print_action_body(action);
        let reparsed = rossi::parse_action_str(&printed).unwrap_or_else(|e| {
            panic!(
                "Failed to parse printed action:\n{e}\n\nPrinted:\n{printed}\n\nOriginal AST:\n{action:#?}"
            )
        });
        assert_eq!(
            *action, reparsed,
            "standalone roundtrip mismatch.\nPrinted:\n{printed}"
        );
    });
}

// --- Context roundtrips ---

#[test]
fn context_roundtrip_unicode() {
    check_roundtrip_property(200, arb_context, |component| {
        assert_component_roundtrip(component, &PrettyPrinter::new());
    });
}

#[test]
fn context_roundtrip_ascii() {
    check_roundtrip_property(200, arb_context, |component| {
        assert_component_roundtrip(component, &PrettyPrinter::ascii());
    });
}

// --- Machine roundtrips ---

#[test]
fn machine_roundtrip_unicode() {
    check_roundtrip_property(200, arb_machine, |component| {
        assert_component_roundtrip(component, &PrettyPrinter::new());
    });
}

#[test]
fn machine_roundtrip_ascii() {
    check_roundtrip_property(200, arb_machine, |component| {
        assert_component_roundtrip(component, &PrettyPrinter::ascii());
    });
}

// --- Camille-style roundtrips ---
//
// The Camille preset changes the structural layout (inline header clauses,
// hanging declaration lists, the deeper event ladder), so the component
// generators re-run against it; formula printing is shared with the
// default printers above.

fn camille_printer(use_unicode: bool) -> PrettyPrinter {
    let mut printer = PrettyPrinter::styled(Style::Camille);
    printer.use_unicode = use_unicode;
    printer
}

#[test]
fn context_roundtrip_camille_unicode() {
    check_roundtrip_property(200, arb_context, |component| {
        assert_component_roundtrip(component, &camille_printer(true));
    });
}

#[test]
fn context_roundtrip_camille_ascii() {
    check_roundtrip_property(200, arb_context, |component| {
        assert_component_roundtrip(component, &camille_printer(false));
    });
}

#[test]
fn machine_roundtrip_camille_unicode() {
    check_roundtrip_property(200, arb_machine, |component| {
        assert_component_roundtrip(component, &camille_printer(true));
    });
}

#[test]
fn machine_roundtrip_camille_ascii() {
    check_roundtrip_property(200, arb_machine, |component| {
        assert_component_roundtrip(component, &camille_printer(false));
    });
}

// --- Wrapped roundtrips ---
//
// A narrow width forces breaks into nearly every generated formula, so
// these fuzz the wrap layer's safety rule: any break whose continuation
// could start a new element shows up as an AST mismatch.

#[test]
fn context_roundtrip_wrapped() {
    check_roundtrip_property(200, arb_context, |component| {
        assert_component_roundtrip(component, &camille_printer(true).with_max_line_width(40));
    });
}

#[test]
fn machine_roundtrip_wrapped() {
    check_roundtrip_property(200, arb_machine, |component| {
        assert_component_roundtrip(component, &camille_printer(true).with_max_line_width(40));
    });
}

#[test]
fn machine_roundtrip_wrapped_ascii_tiny() {
    // Width 12 keeps the hang cap and best-effort overflow paths hot.
    check_roundtrip_property(200, arb_machine, |component| {
        assert_component_roundtrip(component, &camille_printer(false).with_max_line_width(12));
    });
}
