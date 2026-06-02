//! Property-based roundtrip tests for Event-B parser.
//!
//! Core invariant: for any generated AST node, pretty-printing and re-parsing
//! should produce the same AST (modulo spans). This catches edge cases in
//! precedence, parenthesization, and operator rendering.

mod common;

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use rossi::ast::TypedIdentifier;
use rossi::ast::expression::*;
use rossi::ast::predicate::*;
use rossi::*;

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

/// Disjoint identifier pool for bound variables in quantified constructs.
/// Includes both untyped (`p1`) and typed (`p1⦂ℤ`) identifiers.
fn arb_quantifier_identifier() -> impl Strategy<Value = TypedIdentifier> {
    prop_oneof![
        Just(TypedIdentifier::untyped("p1".into())),
        Just(TypedIdentifier::untyped("p2".into())),
        Just(TypedIdentifier::untyped("p3".into())),
        Just(TypedIdentifier::untyped("q1".into())),
        Just(TypedIdentifier::untyped("q2".into())),
        Just(TypedIdentifier::untyped("q3".into())),
        Just(TypedIdentifier::typed("p1".into(), Expression::Integers)),
        Just(TypedIdentifier::typed("p2".into(), Expression::Naturals)),
        Just(TypedIdentifier::typed("q1".into(), Expression::BoolType)),
    ]
}

// =============================================================================
// IdentPattern strategy (for lambda expressions)
// =============================================================================

fn arb_ident_pattern() -> impl Strategy<Value = IdentPattern> {
    arb_quantifier_identifier()
        .prop_map(IdentPattern::Identifier)
        .prop_recursive(2, 8, 2, |inner: BoxedStrategy<IdentPattern>| {
            (inner.clone(), inner)
                .prop_map(|(left, right)| IdentPattern::Maplet(Box::new(left), Box::new(right)))
        })
}

// =============================================================================
// Leaf enum strategies
// =============================================================================

fn arb_binary_op() -> impl Strategy<Value = BinaryOp> {
    prop_oneof![
        Just(BinaryOp::Add),
        Just(BinaryOp::Subtract),
        Just(BinaryOp::Multiply),
        Just(BinaryOp::Divide),
        Just(BinaryOp::Modulo),
        Just(BinaryOp::Exponent),
        Just(BinaryOp::Range),
        Just(BinaryOp::Union),
        Just(BinaryOp::Intersection),
        Just(BinaryOp::Difference),
        Just(BinaryOp::CartesianProduct),
        Just(BinaryOp::Relation),
        Just(BinaryOp::TotalRelation),
        Just(BinaryOp::SurjectiveRelation),
        Just(BinaryOp::TotalSurjectiveRelation),
        Just(BinaryOp::TotalFunction),
        Just(BinaryOp::PartialFunction),
        Just(BinaryOp::TotalInjection),
        Just(BinaryOp::PartialInjection),
        Just(BinaryOp::TotalSurjection),
        Just(BinaryOp::PartialSurjection),
        Just(BinaryOp::Bijection),
        Just(BinaryOp::Composition),
        Just(BinaryOp::Semicolon),
        Just(BinaryOp::DomainRestriction),
        Just(BinaryOp::DomainSubtraction),
        Just(BinaryOp::RangeRestriction),
        Just(BinaryOp::RangeSubtraction),
        Just(BinaryOp::Overwrite),
        Just(BinaryOp::DirectProduct),
        Just(BinaryOp::ParallelProduct),
        Just(BinaryOp::Maplet),
        Just(BinaryOp::OfType),
    ]
}

fn arb_unary_op() -> impl Strategy<Value = UnaryOp> {
    prop_oneof![
        Just(UnaryOp::Minus),
        Just(UnaryOp::PowerSet),
        Just(UnaryOp::PowerSet1),
        Just(UnaryOp::Domain),
        Just(UnaryOp::Range),
        Just(UnaryOp::Inverse),
    ]
}

fn arb_comparison_op() -> impl Strategy<Value = ComparisonOp> {
    prop_oneof![
        Just(ComparisonOp::Equal),
        Just(ComparisonOp::NotEqual),
        Just(ComparisonOp::LessThan),
        Just(ComparisonOp::LessEqual),
        Just(ComparisonOp::GreaterThan),
        Just(ComparisonOp::GreaterEqual),
        Just(ComparisonOp::In),
        Just(ComparisonOp::NotIn),
        Just(ComparisonOp::Subset),
        Just(ComparisonOp::SubsetStrict),
    ]
}

fn arb_logical_op() -> impl Strategy<Value = LogicalOp> {
    prop_oneof![
        Just(LogicalOp::And),
        Just(LogicalOp::Or),
        Just(LogicalOp::Implies),
        Just(LogicalOp::Equivalent),
    ]
}

fn arb_builtin_function() -> impl Strategy<Value = BuiltinFunction> {
    prop_oneof![
        Just(BuiltinFunction::Card),
        Just(BuiltinFunction::Min),
        Just(BuiltinFunction::Max),
        Just(BuiltinFunction::Id),
        Just(BuiltinFunction::Prj1),
        Just(BuiltinFunction::Prj2),
    ]
}

// =============================================================================
// Expression strategy (recursive, depth-limited)
// =============================================================================

fn arb_leaf_expression() -> impl Strategy<Value = Expression> {
    // NOTE: Expression::True and Expression::False are excluded because their
    // printed form (⊤/⊥ or TRUE/FALSE) is ambiguous with Predicate::True/False
    // at parse boundaries (e.g. as LHS of a comparison). They are still reachable
    // inside set enumerations, function arguments, etc. via other AST paths.
    prop_oneof![
        (0i64..1000).prop_map(Expression::Integer),
        arb_identifier().prop_map(Expression::Identifier),
        Just(Expression::EmptySet),
        Just(Expression::Naturals),
        Just(Expression::Naturals1),
        Just(Expression::Integers),
        Just(Expression::BoolType),
        // Bool(predicate) — inline simple predicates to avoid circular type dependency
        prop_oneof![Just(Predicate::True), Just(Predicate::False)]
            .prop_map(|p| Expression::Bool(Box::new(p))),
        // StringLiteral — safe characters only
        "[a-zA-Z0-9_]{0,10}".prop_map(Expression::StringLiteral),
    ]
}

/// Build a recursive expression strategy parameterized by the binary op strategy.
fn arb_expression_with(
    bin_ops: BoxedStrategy<BinaryOp>,
    depth: u32,
    desired_size: u32,
) -> impl Strategy<Value = Expression> {
    arb_leaf_expression().prop_recursive(depth, desired_size, 8, move |inner| {
        let boxed = inner.clone().boxed();
        prop_oneof![
            // Binary expression
            (bin_ops.clone(), inner.clone(), inner.clone()).prop_map(|(op, left, right)| {
                Expression::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                }
            }),
            // Unary expression
            (arb_unary_op(), inner.clone()).prop_map(|(op, operand)| Expression::Unary {
                op,
                operand: Box::new(operand),
            }),
            // SetEnumeration (non-empty to avoid EmptySet mismatch)
            proptest::collection::vec(inner.clone(), 1..4).prop_map(Expression::SetEnumeration),
            // BuiltinApplication (respecting arity)
            arb_builtin_application(boxed.clone()),
            // FunctionApplication (function must not be a builtin name)
            arb_function_application(boxed),
            // RelationalImage: r[S]
            (inner.clone(), inner.clone()).prop_map(|(relation, set)| {
                Expression::RelationalImage {
                    relation: Box::new(relation),
                    set: Box::new(set),
                }
            }),
            // SetComprehension basic: {ids | P}
            (
                proptest::collection::vec(arb_quantifier_identifier(), 1..3),
                arb_leaf_predicate(),
            )
                .prop_map(|(identifiers, predicate)| Expression::SetComprehension {
                    identifiers,
                    predicate: Box::new(predicate),
                    expression: None,
                }),
            // SetComprehension extended: {ids · P | E}
            (
                proptest::collection::vec(arb_quantifier_identifier(), 1..3),
                arb_leaf_predicate(),
                inner.clone(),
            )
                .prop_map(|(identifiers, predicate, expr)| {
                    Expression::SetComprehension {
                        identifiers,
                        predicate: Box::new(predicate),
                        expression: Some(Box::new(expr)),
                    }
                }),
            // Lambda: λ pattern · P | E
            (arb_ident_pattern(), arb_leaf_predicate(), inner.clone(),).prop_map(
                |(pattern, predicate, expression)| Expression::Lambda {
                    pattern,
                    predicate: Box::new(predicate),
                    expression: Box::new(expression),
                }
            ),
            // QuantifiedUnion: ⋃ids · P | E
            (
                proptest::collection::vec(arb_quantifier_identifier(), 1..3),
                arb_leaf_predicate(),
                inner.clone(),
            )
                .prop_map(|(identifiers, predicate, expression)| {
                    Expression::QuantifiedUnion {
                        identifiers,
                        predicate: Box::new(predicate),
                        expression: Box::new(expression),
                    }
                }),
            // QuantifiedInter: ⋂ids · P | E
            (
                proptest::collection::vec(arb_quantifier_identifier(), 1..3),
                arb_leaf_predicate(),
                inner,
            )
                .prop_map(|(identifiers, predicate, expression)| {
                    Expression::QuantifiedInter {
                        identifiers,
                        predicate: Box::new(predicate),
                        expression: Box::new(expression),
                    }
                }),
        ]
    })
}

fn arb_expression() -> impl Strategy<Value = Expression> {
    arb_expression_with(arb_binary_op().boxed(), 4, 64)
}

/// Generate a BuiltinApplication with correct arity.
fn arb_builtin_application(inner: BoxedStrategy<Expression>) -> impl Strategy<Value = Expression> {
    arb_builtin_function().prop_flat_map(move |func| {
        let arity = func.arity();
        proptest::collection::vec(inner.clone(), arity..=arity).prop_map(move |arguments| {
            Expression::BuiltinApplication {
                function: func,
                arguments,
            }
        })
    })
}

/// Generate a FunctionApplication with a safe identifier as function.
/// All names in `arb_identifier()` already avoid builtins and keywords.
fn arb_function_application(inner: BoxedStrategy<Expression>) -> impl Strategy<Value = Expression> {
    (arb_identifier(), proptest::collection::vec(inner, 1..4)).prop_map(|(name, arguments)| {
        Expression::FunctionApplication {
            function: Box::new(Expression::Identifier(name)),
            arguments,
        }
    })
}

// =============================================================================
// Predicate strategy (recursive, depth-limited)
// =============================================================================

fn arb_leaf_predicate() -> impl Strategy<Value = Predicate> {
    prop_oneof![
        Just(Predicate::True),
        Just(Predicate::False),
        (
            arb_comparison_op(),
            arb_leaf_expression(),
            arb_leaf_expression()
        )
            .prop_map(|(op, left, right)| Predicate::Comparison { op, left, right }),
    ]
}

fn arb_predicate() -> impl Strategy<Value = Predicate> {
    arb_leaf_predicate().prop_recursive(
        3,  // depth
        32, // desired size
        4,  // items per collection
        |inner| {
            prop_oneof![
                // Logical { op, left, right }
                (arb_logical_op(), inner.clone(), inner.clone()).prop_map(|(op, left, right)| {
                    Predicate::Logical {
                        op,
                        left: Box::new(left),
                        right: Box::new(right),
                    }
                }),
                // Not(pred)
                inner.clone().prop_map(|p| Predicate::Not(Box::new(p))),
                // Comparison with recursive expressions
                (arb_comparison_op(), arb_expression(), arb_expression())
                    .prop_map(|(op, left, right)| Predicate::Comparison { op, left, right }),
                // Quantified predicate
                (
                    prop_oneof![Just(Quantifier::ForAll), Just(Quantifier::Exists)],
                    proptest::collection::vec(arb_quantifier_identifier(), 1..3),
                    inner.clone(),
                )
                    .prop_map(|(quantifier, identifiers, predicate)| {
                        Predicate::Quantified {
                            quantifier,
                            identifiers,
                            predicate: Box::new(predicate),
                        }
                    }),
                // BuiltinApplication: finite(expr) or partition(expr, expr, ...)
                arb_builtin_predicate_application(),
                // User-defined predicate application: foo(x, y)
                (
                    arb_identifier(),
                    proptest::collection::vec(arb_leaf_expression(), 1..3)
                )
                    .prop_map(|(function, arguments)| Predicate::Application {
                        function,
                        arguments,
                    }),
            ]
        },
    )
}

fn arb_builtin_predicate_application() -> impl Strategy<Value = Predicate> {
    prop_oneof![
        // finite(S) — 1 argument
        arb_leaf_expression().prop_map(|expr| Predicate::BuiltinApplication {
            predicate: BuiltinPredicate::Finite,
            arguments: vec![expr],
        }),
        // partition(S, A, B, ...) — 2..4 arguments
        proptest::collection::vec(arb_leaf_expression(), 2..5).prop_map(|args| {
            Predicate::BuiltinApplication {
                predicate: BuiltinPredicate::Partition,
                arguments: args,
            }
        }),
    ]
}

// =============================================================================
// Action strategy
// =============================================================================

/// Expression strategy excluding semicolon operator (safe for action RHS).
/// Semicolons in action context are action separators, not forward composition.
fn arb_action_expression() -> impl Strategy<Value = Expression> {
    let no_semi = arb_binary_op()
        .prop_filter("exclude semicolon", |op| *op != BinaryOp::Semicolon)
        .boxed();
    arb_expression_with(no_semi, 3, 32)
}

/// Predicate strategy for action RHS (BecomesSuchThat) — uses action expressions.
fn arb_action_predicate() -> impl Strategy<Value = Predicate> {
    prop_oneof![
        Just(Predicate::True),
        Just(Predicate::False),
        (
            arb_comparison_op(),
            arb_action_expression(),
            arb_action_expression()
        )
            .prop_map(|(op, left, right)| Predicate::Comparison { op, left, right }),
    ]
}

fn arb_action() -> impl Strategy<Value = (Action, Vec<String>)> {
    prop_oneof![
        // Single-variable Assignment: v := E
        (arb_identifier(), arb_action_expression()).prop_map(|(var, expr)| {
            let vars = vec![var.clone()];
            (
                Action::Assignment {
                    variables: vec![var],
                    expressions: vec![expr],
                },
                vars,
            )
        }),
        // Multi-variable Assignment: x, y := E1, E2
        (2usize..4)
            .prop_flat_map(|n| {
                (
                    proptest::collection::vec(arb_identifier(), n..=n),
                    proptest::collection::vec(arb_action_expression(), n..=n),
                )
            })
            .prop_map(|(variables, expressions)| {
                let vars = variables.clone();
                (
                    Action::Assignment {
                        variables,
                        expressions,
                    },
                    vars,
                )
            }),
        // Single-variable BecomesIn: v :∈ S
        (arb_identifier(), arb_action_expression()).prop_map(|(var, set)| {
            let vars = vec![var.clone()];
            (
                Action::BecomesIn {
                    variables: vec![var],
                    set,
                },
                vars,
            )
        }),
        // Multi-variable BecomesIn: x, y :∈ S
        (
            proptest::collection::vec(arb_identifier(), 2..4),
            arb_action_expression(),
        )
            .prop_map(|(variables, set)| {
                let vars = variables.clone();
                (Action::BecomesIn { variables, set }, vars)
            }),
        // Single-variable BecomesSuchThat: v :| P
        (arb_identifier(), arb_action_predicate()).prop_map(|(var, pred)| {
            let vars = vec![var.clone()];
            (
                Action::BecomesSuchThat {
                    variables: vec![var],
                    predicate: pred,
                },
                vars,
            )
        }),
        // Multi-variable BecomesSuchThat: x, y :| P
        (
            proptest::collection::vec(arb_identifier(), 2..4),
            arb_action_predicate(),
        )
            .prop_map(|(variables, predicate)| {
                let vars = variables.clone();
                (
                    Action::BecomesSuchThat {
                        variables,
                        predicate,
                    },
                    vars,
                )
            }),
        // FunctionOverride: f(x) := E
        (
            arb_identifier(),
            proptest::collection::vec(arb_action_expression(), 1..3),
            arb_action_expression(),
        )
            .prop_map(|(function, arguments, expression)| {
                let vars = vec![function.clone()];
                (
                    Action::FunctionOverride {
                        function,
                        arguments,
                        expression,
                    },
                    vars,
                )
            }),
    ]
}

// =============================================================================
// Component-level strategies
// =============================================================================

fn arb_set_declaration() -> impl Strategy<Value = SetDeclaration> {
    let set_name = prop_oneof![Just("SS".into()), Just("TT".into()), Just("UU".into()),];
    prop_oneof![
        set_name.clone().prop_map(|name| SetDeclaration::Deferred {
            name,
            comment: None
        }),
        (
            set_name,
            proptest::collection::vec(
                prop_oneof![
                    Just("el1".into()),
                    Just("el2".into()),
                    Just("el3".into()),
                    Just("el4".into()),
                ],
                1..4,
            ),
        )
            .prop_map(|(name, elements)| SetDeclaration::Enumerated {
                name,
                elements,
                comment: None,
            }),
    ]
}

/// Generate a label from a fixed pool. Always returns `Some(label)` to avoid
/// a known grammar ambiguity in ASCII mode where unlabeled predicates starting
/// with keyword-like identifiers followed by `:` (the `In` operator) are
/// misinterpreted as labels.
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
        proptest::bool::ANY,
        proptest::collection::vec(arb_quantifier_identifier(), 0..3),
        proptest::collection::vec(arb_axiom(), 0..3),
        proptest::collection::vec(arb_witness_predicate(), 0..2),
        proptest::collection::vec(arb_witness_predicate(), 0..2),
        proptest::collection::vec(arb_labeled_action(), 0..3),
    )
        .prop_map(
            |(name, status, has_refines, parameters, guards, with, witnesses, actions)| {
                let mut event = Event::new(name.clone());
                event.status = status;
                if has_refines {
                    event.refines = Some(format!("{name}_abs"));
                    event.with = with;
                }
                event.parameters = parameters
                    .into_iter()
                    .map(|tid| NamedElement::new(tid.name))
                    .collect();
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
    })
}

fn arb_context() -> impl Strategy<Value = Component> {
    (
        proptest::collection::vec(arb_set_declaration(), 0..3),
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
        proptest::collection::vec(arb_identifier(), 0..4),
        proptest::collection::vec(arb_axiom(), 0..3),
        proptest::collection::vec(arb_theorem(), 0..2),
        proptest::option::of(arb_leaf_expression()),
        proptest::option::of(arb_initialisation()),
        proptest::collection::vec(arb_event(), 0..3),
    )
        .prop_map(
            |(variables, mut invariants, theorems, variant, initialisation, events)| {
                let mut machine = Machine::new("PropMch".into());
                machine.variables = variables.into_iter().map(NamedElement::new).collect();
                invariants.extend(theorems);
                machine.invariants = invariants;
                machine.variant = variant;
                machine.initialisation = initialisation;
                machine.events = events;
                Component::Machine(machine)
            },
        )
}

// =============================================================================
// Wrappers — embed generated AST in minimal parseable Components
// =============================================================================

/// Wrap an expression in a Context axiom: `axm1: propvar = <expr>`
fn wrap_expression_in_context(expr: &Expression) -> Component {
    let mut ctx = Context::new("proptest".into());
    ctx.constants = vec![NamedElement::new("propvar".to_string())];
    ctx.axioms = vec![LabeledPredicate {
        label: Some("axm1".into()),
        is_theorem: false,
        predicate: Predicate::Comparison {
            op: ComparisonOp::Equal,
            left: Expression::Identifier("propvar".into()),
            right: expr.clone(),
        },
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
fn wrap_action_in_machine(action: &Action, variables: &[String]) -> Component {
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
// proptest! tests
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    // --- Expression roundtrips ---

    #[test]
    fn expression_roundtrip_unicode(expr in arb_expression()) {
        let component = wrap_expression_in_context(&expr);
        assert_component_roundtrip(&component, &PrettyPrinter::new());
    }

    #[test]
    fn expression_roundtrip_ascii(expr in arb_expression()) {
        let component = wrap_expression_in_context(&expr);
        assert_component_roundtrip(&component, &PrettyPrinter::ascii());
    }

    // --- Predicate roundtrips ---

    #[test]
    fn predicate_roundtrip_unicode(pred in arb_predicate()) {
        let component = wrap_predicate_in_context(&pred);
        assert_component_roundtrip(&component, &PrettyPrinter::new());
    }

    #[test]
    fn predicate_roundtrip_ascii(pred in arb_predicate()) {
        let component = wrap_predicate_in_context(&pred);
        assert_component_roundtrip(&component, &PrettyPrinter::ascii());
    }

    // --- Action roundtrips ---

    #[test]
    fn action_roundtrip_unicode((action, vars) in arb_action()) {
        let component = wrap_action_in_machine(&action, &vars);
        assert_component_roundtrip(&component, &PrettyPrinter::new());
    }

    #[test]
    fn action_roundtrip_ascii((action, vars) in arb_action()) {
        let component = wrap_action_in_machine(&action, &vars);
        assert_component_roundtrip(&component, &PrettyPrinter::ascii());
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    // --- Context roundtrips ---

    #[test]
    fn context_roundtrip_unicode(component in arb_context()) {
        assert_component_roundtrip(&component, &PrettyPrinter::new());
    }

    #[test]
    fn context_roundtrip_ascii(component in arb_context()) {
        assert_component_roundtrip(&component, &PrettyPrinter::ascii());
    }

    // --- Machine roundtrips ---

    #[test]
    fn machine_roundtrip_unicode(component in arb_machine()) {
        assert_component_roundtrip(&component, &PrettyPrinter::new());
    }

    #[test]
    fn machine_roundtrip_ascii(component in arb_machine()) {
        assert_component_roundtrip(&component, &PrettyPrinter::ascii());
    }
}
