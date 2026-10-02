//! The simplification rewriter behind the `autoRewrites` reasoner
//! family, at level L5 (the latest; per-level guards are fixed at
//! their L5 values). Rules follow the reference pattern order, each
//! carrying its `SIMP_*`/`DERIV_*`/`DEF_*` name.
//!
//! It builds on the propositional simplifier: predicate node kinds
//! with no rule of their own fall through to it, and the negation
//! arm tries the simplifier first.

use rossi::formula::tag::{
    AssocExprOp, AssocPredOp, AtomicOp, BinaryExprOp, BinaryPredOp, LiteralPredOp, QuantExprOp,
    QuantPredOp, RelationalOp, UnaryExprOp,
};
use rossi::formula::{Expression, ExpressionKind, Predicate, PredicateKind, Type};

use crate::builder::{Reasoner, ReplayHints};
use crate::rule::Rule;
use crate::sequent::ProverSequent;
use crate::skeleton::StoredRule;

use super::driver::{NodeRewriter, rewrite_pred};
use super::one_point;
use super::rewrites::{auto_rewrite_rule, simplify_predicate_node};

/// The L5 rewriter as a rewriting hook.
pub(crate) struct AutoRewriter;

/// `AutoRewrites` at level L5 (`autoRewritesL5`) — automatic
/// simplification rewrites.
pub struct AutoRewritesL5;

impl Reasoner for AutoRewritesL5 {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        _hints: &ReplayHints,
    ) -> Result<Rule, String> {
        auto_rewrite_rule(
            seq,
            stored,
            &mut AutoRewriter,
            true,
            "simplification rewrites",
        )
    }
}

impl NodeRewriter for AutoRewriter {
    fn predicate(&mut self, pred: &Predicate) -> Option<Predicate> {
        match pred.kind() {
            // Handled by the propositional simplifier alone.
            PredicateKind::Associative { .. }
            | PredicateKind::Binary { .. }
            | PredicateKind::Quantified { .. } => simplify_predicate_node(pred),
            // Overridden with a super call first.
            PredicateKind::Not(_) => simplify_predicate_node(pred).or_else(|| rewrite_not(pred)),
            PredicateKind::Simple(child) => rewrite_finite(pred, child),
            PredicateKind::Multiple(children) => rewrite_partition(pred, children),
            PredicateKind::Relational { .. } => rewrite_relational(pred),
            _ => None,
        }
    }

    fn expression(&mut self, expr: &Expression) -> Option<Expression> {
        rewrite_expression(expr)
    }
}

fn btrue(pred: &Predicate) -> Option<Predicate> {
    Some(pred.factory().literal_predicate(LiteralPredOp::BTrue, None))
}

fn bfalse(pred: &Predicate) -> Option<Predicate> {
    Some(
        pred.factory()
            .literal_predicate(LiteralPredOp::BFalse, None),
    )
}

fn is_atomic(expr: &Expression, op: AtomicOp) -> bool {
    matches!(expr.kind(), ExpressionKind::Atomic(found) if *found == op)
}

/// `makeFinite`.
fn finite(expr: &Expression) -> Predicate {
    expr.factory().simple_predicate(expr.clone(), None)
}

/// `makeIsEmpty`: `E = ∅` with the empty set typed like `E`.
fn is_empty(expr: &Expression) -> Predicate {
    let ff = expr.factory();
    let empty = ff.atomic_expression(AtomicOp::EmptySet, None, expr.ty().cloned());
    ff.relational_predicate(RelationalOp::Equal, expr.clone(), empty, None)
}

/// The `finite(...)` rules.
fn rewrite_finite(pred: &Predicate, child: &Expression) -> Option<Predicate> {
    let ff = pred.factory();
    match child.kind() {
        // SIMP_SPECIAL_FINITE: finite(∅) == ⊤
        ExpressionKind::Atomic(AtomicOp::EmptySet) => btrue(pred),
        // SIMP_FINITE_NATURAL / NATURAL1 / INTEGER == ⊥ (level 2)
        ExpressionKind::Atomic(AtomicOp::Natural)
        | ExpressionKind::Atomic(AtomicOp::Natural1)
        | ExpressionKind::Atomic(AtomicOp::Integer) => bfalse(pred),
        // SIMP_FINITE_BOOL: finite(BOOL) == ⊤ (level 2)
        ExpressionKind::Atomic(AtomicOp::Bool) => btrue(pred),
        // SIMP_FINITE_ID: finite(id) == finite(S), id of type S↔S
        // (level 2)
        ExpressionKind::Atomic(AtomicOp::KIdGen) => {
            let Some(Type::Pow(pair)) = child.ty() else {
                return None;
            };
            let Type::Prod(source, _) = &**pair else {
                return None;
            };
            Some(finite(&source.to_expression(ff)))
        }
        // SIMP_FINITE_SETENUM: finite({a, …, b}) == ⊤
        ExpressionKind::SetExtension(_) => btrue(pred),
        // SIMP_FINITE_BUNION: finite(S ∪ … ∪ T) == finite(S) ∧ … ∧ finite(T)
        ExpressionKind::Associative {
            op: rossi::formula::tag::AssocExprOp::BUnion,
            children,
        } => Some(ff.associative_predicate(
            AssocPredOp::LAnd,
            children.iter().map(finite).collect(),
            None,
        )),
        // SIMP_FINITE_POW: finite(ℙ(S)) == finite(S)
        ExpressionKind::Unary {
            op: UnaryExprOp::Pow,
            child: inner,
        } => Some(finite(inner)),
        // DERIV_FINITE_CPROD:
        // finite(S × T) == S = ∅ ∨ T = ∅ ∨ (finite(S) ∧ finite(T))
        ExpressionKind::Binary {
            op: rossi::formula::tag::BinaryExprOp::CProd,
            left,
            right,
        } => Some(ff.associative_predicate(
            AssocPredOp::LOr,
            vec![
                is_empty(left),
                is_empty(right),
                ff.associative_predicate(
                    AssocPredOp::LAnd,
                    vec![finite(left), finite(right)],
                    None,
                ),
            ],
            None,
        )),
        // SIMP_FINITE_CONVERSE: finite(r∼) == finite(r)
        ExpressionKind::Unary {
            op: UnaryExprOp::Converse,
            child: inner,
        } => Some(finite(inner)),
        // SIMP_FINITE_UPTO: finite(a‥b) == ⊤
        ExpressionKind::Binary {
            op: rossi::formula::tag::BinaryExprOp::UpTo,
            ..
        } => btrue(pred),
        // SIMP_FINITE_LAMBDA: finite({x·P ∣ E ↦ F}) == finite({x·P ∣ E})
        // (level 2, when the lambda pattern is functional)
        ExpressionKind::Quantified {
            op: rossi::formula::tag::QuantExprOp::CSet,
            decls,
            pred: body,
            expr: value,
            ..
        } => {
            let ExpressionKind::Binary {
                op: rossi::formula::tag::BinaryExprOp::Mapsto,
                left,
                right,
            } = value.kind()
            else {
                return None;
            };
            if !functional_check(left, right, decls.len() as u32) {
                return None;
            }
            Some(finite(&ff.quantified_expression(
                rossi::formula::tag::QuantExprOp::CSet,
                decls.clone(),
                body.clone(),
                left.clone(),
                None,
                rossi::formula::Form::Explicit,
            )))
        }
        // SIMP_FINITE_ID_DOMRES / PRJ1_DOMRES / PRJ2_DOMRES:
        // finite(E ◁ id) == finite(E), same for prj1/prj2 (level 2)
        ExpressionKind::Binary {
            op: rossi::formula::tag::BinaryExprOp::DomRes,
            left,
            right,
        } if matches!(
            right.kind(),
            ExpressionKind::Atomic(AtomicOp::KIdGen)
                | ExpressionKind::Atomic(AtomicOp::KPrj1Gen)
                | ExpressionKind::Atomic(AtomicOp::KPrj2Gen)
        ) =>
        {
            Some(finite(left))
        }
        // SIMP_FINITE_PRJ1 / PRJ2: finite(prjN) == finite(S × T),
        // prjN of type ℙ(S×T×_) (level 2)
        ExpressionKind::Atomic(AtomicOp::KPrj1Gen) | ExpressionKind::Atomic(AtomicOp::KPrj2Gen) => {
            let Some(Type::Pow(pair)) = child.ty() else {
                return None;
            };
            let Type::Prod(source, _) = &**pair else {
                return None;
            };
            Some(finite(&source.to_expression(ff)))
        }
        _ => None,
    }
}

/// The functional check, for the lambda-shaped comprehension rules:
/// every locally bound identifier the maplet's right side uses must
/// occur in the left side at a pattern position — reached through
/// maplet operators only; an occurrence nested inside any other
/// operator does not count.
fn functional_check(left: &Expression, right: &Expression, n_bound: u32) -> bool {
    fn clear_pattern_occurrences(expr: &Expression, set: &mut std::collections::HashSet<u32>) {
        match expr.kind() {
            ExpressionKind::Binary {
                op: BinaryExprOp::Mapsto,
                left,
                right,
            } => {
                clear_pattern_occurrences(left, set);
                clear_pattern_occurrences(right, set);
            }
            ExpressionKind::BoundIdentifier(index) => {
                set.remove(index);
            }
            _ => {}
        }
    }
    let mut locally_bound_right: std::collections::HashSet<u32> = right
        .dangling_bound_indices()
        .iter()
        .copied()
        .filter(|&i| i < n_bound)
        .collect();
    clear_pattern_occurrences(left, &mut locally_bound_right);
    locally_bound_right.is_empty()
}

/// The `partition(...)` rules
/// (level 4).
fn rewrite_partition(pred: &Predicate, children: &[Expression]) -> Option<Predicate> {
    let ff = pred.factory();
    match children {
        // SIMP_EMPTY_PARTITION: partition(S) == S = ∅
        [single] => Some(is_empty(single)),
        // SIMP_SINGLE_PARTITION: partition(S, T) == S = T
        [left, right] => {
            Some(ff.relational_predicate(RelationalOp::Equal, left.clone(), right.clone(), None))
        }
        _ => None,
    }
}

/// The negation-pushing rules, after the
/// simplifier's own `¬` rules failed.
pub(crate) fn rewrite_not(pred: &Predicate) -> Option<Predicate> {
    let PredicateKind::Not(inner) = pred.kind() else {
        return None;
    };
    let ff = pred.factory();
    let PredicateKind::Relational { op, left, right } = inner.kind() else {
        return None;
    };
    let flip = |flipped: RelationalOp| {
        Some(ff.relational_predicate(flipped, left.clone(), right.clone(), None))
    };
    let bool_atom = |op: AtomicOp| ff.atomic_expression(op, None, None);
    match op {
        // SIMP_NOT_LE: ¬ a ≤ b == a > b
        RelationalOp::Le => flip(RelationalOp::Gt),
        // SIMP_NOT_GE: ¬ a ≥ b == a < b
        RelationalOp::Ge => flip(RelationalOp::Lt),
        // SIMP_NOT_GT: ¬ a > b == a ≤ b
        RelationalOp::Gt => flip(RelationalOp::Le),
        // SIMP_NOT_LT: ¬ a < b == a ≥ b
        RelationalOp::Lt => flip(RelationalOp::Ge),
        RelationalOp::Equal => {
            // SIMP_SPECIAL_NOT_EQUAL_FALSE_R: ¬(E = FALSE) == E = TRUE
            if is_atomic(right, AtomicOp::False) {
                return Some(ff.relational_predicate(
                    RelationalOp::Equal,
                    left.clone(),
                    bool_atom(AtomicOp::True),
                    None,
                ));
            }
            // SIMP_SPECIAL_NOT_EQUAL_TRUE_R: ¬(E = TRUE) == E = FALSE
            if is_atomic(right, AtomicOp::True) {
                return Some(ff.relational_predicate(
                    RelationalOp::Equal,
                    left.clone(),
                    bool_atom(AtomicOp::False),
                    None,
                ));
            }
            // SIMP_SPECIAL_NOT_EQUAL_FALSE_L: ¬(FALSE = E) == TRUE = E
            if is_atomic(left, AtomicOp::False) {
                return Some(ff.relational_predicate(
                    RelationalOp::Equal,
                    bool_atom(AtomicOp::True),
                    right.clone(),
                    None,
                ));
            }
            // SIMP_SPECIAL_NOT_EQUAL_TRUE_L: ¬(TRUE = E) == FALSE = E
            if is_atomic(left, AtomicOp::True) {
                return Some(ff.relational_predicate(
                    RelationalOp::Equal,
                    bool_atom(AtomicOp::False),
                    right.clone(),
                    None,
                ));
            }
            None
        }
        _ => None,
    }
}

/// `makeIsNotEmpty`: `¬ E = ∅`.
fn is_not_empty(expr: &Expression) -> Predicate {
    let inner = is_empty(expr);
    inner.factory().not_predicate(inner.clone(), None)
}

/// `¬ P` via `makeUnaryPredicate(NOT, …)` (no unwrapping).
fn not(pred: &Predicate) -> Predicate {
    pred.factory().not_predicate(pred.clone(), None)
}

fn rel(op: RelationalOp, left: &Expression, right: &Expression) -> Predicate {
    left.factory()
        .relational_predicate(op, left.clone(), right.clone(), None)
}

fn conj(like: &Predicate, children: Vec<Predicate>) -> Predicate {
    match children.len() {
        1 => children.into_iter().next().unwrap(),
        _ => like
            .factory()
            .associative_predicate(AssocPredOp::LAnd, children, None),
    }
}

fn disj(like: &Predicate, children: Vec<Predicate>) -> Predicate {
    match children.len() {
        1 => children.into_iter().next().unwrap(),
        _ => like
            .factory()
            .associative_predicate(AssocPredOp::LOr, children, None),
    }
}

/// `∃x·S = {x}` — the singleton existential, on the
/// shared component binder.
fn exists_singleton(set: &Expression) -> Option<Predicate> {
    let ff = set.factory();
    let (decls, member, shifted) = super::component_binder(set)?;
    let singleton = ff.set_extension(vec![member], None);
    let equal = ff.relational_predicate(RelationalOp::Equal, shifted, singleton, None);
    Some(ff.quantified_predicate(QuantPredOp::Exists, decls, equal, None))
}

/// The relational-predicate rules, in the reference pattern order.
pub(crate) fn rewrite_relational(pred: &Predicate) -> Option<Predicate> {
    use rossi::formula::tag::{AssocExprOp, BinaryExprOp};
    let PredicateKind::Relational { op, left, right } = pred.kind() else {
        return None;
    };
    // SIMP_MULTI_EQUAL/LE/GE == ⊤, SIMP_MULTI_NOTEQUAL/LT/GT == ⊥
    if left == right {
        match op {
            RelationalOp::Equal | RelationalOp::Le | RelationalOp::Ge => return btrue(pred),
            RelationalOp::NotEqual | RelationalOp::Lt | RelationalOp::Gt => return bfalse(pred),
            _ => {}
        }
    }
    match op {
        RelationalOp::Equal => {
            // SIMP_EQUAL_MAPSTO: E ↦ F = G ↦ H == E = G ∧ F = H
            if let (
                ExpressionKind::Binary {
                    op: BinaryExprOp::Mapsto,
                    left: e,
                    right: f,
                },
                ExpressionKind::Binary {
                    op: BinaryExprOp::Mapsto,
                    left: g,
                    right: h,
                },
            ) = (left.kind(), right.kind())
            {
                return Some(conj(
                    pred,
                    vec![
                        rel(RelationalOp::Equal, e, g),
                        rel(RelationalOp::Equal, f, h),
                    ],
                ));
            }
            // SIMP_SPECIAL_EQUAL_TRUE: TRUE = FALSE == ⊥ (both ways)
            if (is_atomic(left, AtomicOp::True) && is_atomic(right, AtomicOp::False))
                || (is_atomic(left, AtomicOp::False) && is_atomic(right, AtomicOp::True))
            {
                return bfalse(pred);
            }
        }
        // SIMP_NOTEQUAL: E ≠ F == ¬ E = F
        RelationalOp::NotEqual => {
            return Some(not(&rel(RelationalOp::Equal, left, right)));
        }
        // SIMP_NOTIN: E ∉ F == ¬ E ∈ F
        RelationalOp::NotIn => {
            return Some(not(&rel(RelationalOp::In, left, right)));
        }
        // SIMP_NOTSUBSET: E ⊄ F == ¬ E ⊂ F
        RelationalOp::NotSubset => {
            return Some(not(&rel(RelationalOp::Subset, left, right)));
        }
        // SIMP_NOTSUBSETEQ: E ⊈ F == ¬ E ⊆ F
        RelationalOp::NotSubsetEq => {
            return Some(not(&rel(RelationalOp::SubsetEq, left, right)));
        }
        // SIMP_SPECIAL_SUBSETEQ: ∅ ⊆ S == ⊤
        RelationalOp::SubsetEq if is_atomic(left, AtomicOp::EmptySet) => {
            return btrue(pred);
        }
        // SIMP_MULTI_SUBSETEQ: S ⊆ S == ⊤
        RelationalOp::SubsetEq if left == right => {
            return btrue(pred);
        }
        _ => {}
    }
    // Equality with the empty set (level 4): E = ∅, E ⊆ ∅, ∅ = E.
    let empty_operand = match op {
        RelationalOp::Equal | RelationalOp::SubsetEq if is_atomic(right, AtomicOp::EmptySet) => {
            Some(left)
        }
        RelationalOp::Equal if is_atomic(left, AtomicOp::EmptySet) => Some(right),
        _ => None,
    };
    if let Some(expr) = empty_operand
        && let Some(result) = rewrite_equals_empty_set(pred, expr)
    {
        return Some(result);
    }
    match op {
        RelationalOp::SubsetEq => {
            // SIMP_SUBSETEQ_BUNION: S ⊆ … ∪ S ∪ … == ⊤
            if let ExpressionKind::Associative {
                op: AssocExprOp::BUnion,
                children,
            } = right.kind()
                && children.contains(left)
            {
                return btrue(pred);
            }
            // SIMP_SUBSETEQ_BINTER: … ∩ S ∩ … ⊆ S == ⊤
            if let ExpressionKind::Associative {
                op: AssocExprOp::BInter,
                children,
            } = left.kind()
                && children.contains(right)
            {
                return btrue(pred);
            }
            // DERIV_SUBSETEQ_BUNION: A ∪ … ∪ B ⊆ S == A ⊆ S ∧ …
            if let ExpressionKind::Associative {
                op: AssocExprOp::BUnion,
                children,
            } = left.kind()
            {
                return Some(conj(
                    pred,
                    children
                        .iter()
                        .map(|child| rel(RelationalOp::SubsetEq, child, right))
                        .collect(),
                ));
            }
            // DERIV_SUBSETEQ_BINTER: S ⊆ A ∩ … ∩ B == S ⊆ A ∧ …
            if let ExpressionKind::Associative {
                op: AssocExprOp::BInter,
                children,
            } = right.kind()
            {
                return Some(conj(
                    pred,
                    children
                        .iter()
                        .map(|child| rel(RelationalOp::SubsetEq, left, child))
                        .collect(),
                ));
            }
            // SIMP_SUBSETEQ_SING: {E} ⊆ S == E ∈ S (level 2)
            if let ExpressionKind::SetExtension(members) = left.kind()
                && let [element] = members.as_slice()
            {
                return Some(rel(RelationalOp::In, element, right));
            }
        }
        RelationalOp::In => {
            // SIMP_SPECIAL_IN: E ∈ ∅ == ⊥
            if is_atomic(right, AtomicOp::EmptySet) {
                return bfalse(pred);
            }
            if let ExpressionKind::SetExtension(members) = right.kind() {
                // SIMP_MULTI_IN: B ∈ {…, B, …} == ⊤
                if members.contains(left) {
                    return btrue(pred);
                }
                // SIMP_IN_SING: E ∈ {F} == E = F
                if let [single] = members.as_slice() {
                    return Some(rel(RelationalOp::Equal, left, single));
                }
            }
            // SIMP_IN_COMPSET / SIMP_IN_COMPSET_ONEPOINT: membership
            // in a comprehension set always rewrites (existential
            // form, instantiated when a component equality names a
            // bound identifier).
            if matches!(
                right.kind(),
                ExpressionKind::Quantified {
                    op: rossi::formula::tag::QuantExprOp::CSet,
                    ..
                }
            ) {
                return one_point::rewrite_in_compset(pred);
            }
        }
        _ => {}
    }
    if *op == RelationalOp::Equal {
        // SIMP_EQUAL_SING: {E} = {F} == E = F
        if let (ExpressionKind::SetExtension(a), ExpressionKind::SetExtension(b)) =
            (left.kind(), right.kind())
            && let ([e], [f]) = (a.as_slice(), b.as_slice())
        {
            return Some(rel(RelationalOp::Equal, e, f));
        }
    }
    // SIMP_LIT_* — literal comparisons by computation.
    if let (Some(i), Some(j)) = (super::as_literal(left), super::as_literal(right)) {
        let truth = |verdict: bool| if verdict { btrue(pred) } else { bfalse(pred) };
        match op {
            RelationalOp::Equal => return truth(i == j),
            RelationalOp::Le => return truth(i <= j),
            RelationalOp::Lt => return truth(i < j),
            RelationalOp::Ge => return truth(i >= j),
            RelationalOp::Gt => return truth(i > j),
            _ => {}
        }
    }
    // Cardinality specials against the literals 0 and 1.
    let card_arg = |expr: &Expression| match expr.kind() {
        ExpressionKind::Unary {
            op: UnaryExprOp::KCard,
            child,
        } => Some(child.clone()),
        _ => None,
    };
    let zero = num_bigint::BigInt::ZERO;
    let one = num_bigint::BigInt::from(1);
    match op {
        RelationalOp::Equal => {
            // SIMP_SPECIAL_EQUAL_CARD / SIMP_LIT_EQUAL_CARD_1
            if let (Some(set), Some(value)) = (card_arg(left), super::as_literal(right)) {
                if value == zero {
                    return Some(is_empty(&set));
                }
                if value == one {
                    return exists_singleton(&set);
                }
            }
            if let (Some(value), Some(set)) = (super::as_literal(left), card_arg(right)) {
                if value == zero {
                    return Some(is_empty(&set));
                }
                if value == one {
                    return exists_singleton(&set);
                }
            }
            // SIMP_LIT_EQUAL_KBOOL_TRUE / FALSE
            if is_atomic(left, AtomicOp::True)
                && let ExpressionKind::Bool(inner) = right.kind()
            {
                return Some(inner.clone());
            }
            if is_atomic(right, AtomicOp::True)
                && let ExpressionKind::Bool(inner) = left.kind()
            {
                return Some(inner.clone());
            }
            if is_atomic(left, AtomicOp::False)
                && let ExpressionKind::Bool(inner) = right.kind()
            {
                return Some(not(inner));
            }
            if is_atomic(right, AtomicOp::False)
                && let ExpressionKind::Bool(inner) = left.kind()
            {
                return Some(not(inner));
            }
        }
        // SIMP_LIT_GT_CARD_0: card(S) > 0 == ¬ S = ∅
        RelationalOp::Gt => {
            if let (Some(set), Some(value)) = (card_arg(left), super::as_literal(right))
                && value == zero
            {
                return Some(is_not_empty(&set));
            }
        }
        // SIMP_LIT_LT_CARD_0: 0 < card(S) == ¬ S = ∅
        RelationalOp::Lt => {
            if let (Some(value), Some(set)) = (super::as_literal(left), card_arg(right))
                && value == zero
            {
                return Some(is_not_empty(&set));
            }
        }
        // SIMP_LIT_LE_CARD_0: 0 ≤ card(S) == ⊤
        // SIMP_LIT_LE_CARD_1: 1 ≤ card(S) == ¬ S = ∅
        RelationalOp::Le => {
            if let (Some(value), Some(set)) = (super::as_literal(left), card_arg(right)) {
                if value == zero {
                    return btrue(pred);
                }
                if value == one {
                    return Some(is_not_empty(&set));
                }
            }
        }
        // SIMP_LIT_GE_CARD_0: card(S) ≥ 0 == ⊤
        // SIMP_LIT_GE_CARD_1: card(S) ≥ 1 == ¬ S = ∅
        RelationalOp::Ge => {
            if let (Some(set), Some(value)) = (card_arg(left), super::as_literal(right)) {
                if value == zero {
                    return btrue(pred);
                }
                if value == one {
                    return Some(is_not_empty(&set));
                }
            }
        }
        _ => {}
    }
    batch2_relational(pred, *op, left, right)
}

/// The later relational rules (reference pattern order continues).
fn batch2_relational(
    pred: &Predicate,
    op: RelationalOp,
    left: &Expression,
    right: &Expression,
) -> Option<Predicate> {
    use rossi::formula::tag::{AssocExprOp, BinaryExprOp, QuantExprOp};
    let ff = pred.factory();
    let zero = num_bigint::BigInt::ZERO;
    let card_arg = |expr: &Expression| match expr.kind() {
        ExpressionKind::Unary {
            op: UnaryExprOp::KCard,
            child,
        } => Some(child.clone()),
        _ => None,
    };
    let is_arrow = |op: BinaryExprOp, with_inj: bool| {
        matches!(
            op,
            BinaryExprOp::Rel
                | BinaryExprOp::TRel
                | BinaryExprOp::SRel
                | BinaryExprOp::STRel
                | BinaryExprOp::PFun
                | BinaryExprOp::TFun
                | BinaryExprOp::PSur
                | BinaryExprOp::TSur
        ) || (with_inj
            && matches!(
                op,
                BinaryExprOp::PInj | BinaryExprOp::TInj | BinaryExprOp::TBij
            ))
    };
    match op {
        RelationalOp::Subset => {
            // SIMP_SPECIAL_SUBSET_R: S ⊂ ∅ == ⊥ (level 2)
            if is_atomic(right, AtomicOp::EmptySet) {
                return bfalse(pred);
            }
            // SIMP_MULTI_SUBSET: S ⊂ S == ⊥ (level 2)
            if left == right {
                return bfalse(pred);
            }
            // SIMP_SPECIAL_SUBSET_L: ∅ ⊂ S == ¬ S = ∅ (level 2)
            if is_atomic(left, AtomicOp::EmptySet) {
                return Some(not(&rel(RelationalOp::Equal, right, left)));
            }
        }
        RelationalOp::Equal => {
            // SIMP_SPECIAL_EQUAL_REL: A ↔ B = ∅, A ⇸ B = ∅,
            // A ⤔ B = ∅ == ⊥ (level 2)
            if is_atomic(right, AtomicOp::EmptySet)
                && let ExpressionKind::Binary { op: arrow, .. } = left.kind()
            {
                if matches!(
                    arrow,
                    BinaryExprOp::Rel | BinaryExprOp::PFun | BinaryExprOp::PInj
                ) {
                    return bfalse(pred);
                }
                // SIMP_SPECIAL_EQUAL_RELDOM: A  B = ∅ or A → B = ∅
                // == ¬(A = ∅) ∧ B = ∅ (level 2)
                if matches!(arrow, BinaryExprOp::TFun | BinaryExprOp::TRel) {
                    let ExpressionKind::Binary {
                        left: a, right: b, ..
                    } = left.kind()
                    else {
                        unreachable!("matched above");
                    };
                    return Some(conj(pred, vec![is_not_empty(a), is_empty(b)]));
                }
            }
            // SIMP_MULTI_EQUAL_BINTER:
            // S ∩ … ∩ T ∩ … = T == T ⊆ S ∩ … (level 2)
            if let ExpressionKind::Associative {
                op: AssocExprOp::BInter,
                children,
            } = left.kind()
                && let Some(index) = children.iter().position(|c| c == right)
            {
                let rest: Vec<Expression> = children
                    .iter()
                    .enumerate()
                    .filter(|(k, _)| *k != index)
                    .map(|(_, c)| c.clone())
                    .collect();
                let inter = if rest.len() == 1 {
                    rest.into_iter().next().unwrap()
                } else {
                    ff.associative_expression(AssocExprOp::BInter, rest, None)
                };
                return Some(rel(RelationalOp::SubsetEq, right, &inter));
            }
            // SIMP_MULTI_EQUAL_BUNION:
            // S ∪ … ∪ T ∪ … = T == S ∪ … ⊆ T (level 2)
            if let ExpressionKind::Associative {
                op: AssocExprOp::BUnion,
                children,
            } = left.kind()
                && let Some(index) = children.iter().position(|c| c == right)
            {
                let rest: Vec<Expression> = children
                    .iter()
                    .enumerate()
                    .filter(|(k, _)| *k != index)
                    .map(|(_, c)| c.clone())
                    .collect();
                let union = if rest.len() == 1 {
                    rest.into_iter().next().unwrap()
                } else {
                    ff.associative_expression(AssocExprOp::BUnion, rest, None)
                };
                return Some(rel(RelationalOp::SubsetEq, &union, right));
            }
            // SIMP_SPECIAL_EQUAL_COMPSET: {x·P ∣ E} = ∅ == ∀x·¬P
            // (level 2)
            if is_atomic(right, AtomicOp::EmptySet)
                && let ExpressionKind::Quantified {
                    op: QuantExprOp::CSet,
                    decls,
                    pred: body,
                    ..
                } = left.kind()
            {
                return Some(ff.quantified_predicate(
                    QuantPredOp::Forall,
                    decls.clone(),
                    not(body),
                    None,
                ));
            }
        }
        RelationalOp::In => {
            // SIMP_CARD_NATURAL: card(S) ∈ ℕ == ⊤ (level 2)
            if card_arg(left).is_some() && is_atomic(right, AtomicOp::Natural) {
                return btrue(pred);
            }
            // SIMP_CARD_NATURAL1: card(S) ∈ ℕ1 == ¬ S = ∅ (level 2)
            if let Some(set) = card_arg(left)
                && is_atomic(right, AtomicOp::Natural1)
            {
                return Some(is_not_empty(&set));
            }
            // SIMP_LIT_IN_NATURAL(1) and the negated-literal forms
            // (level 2) — a negative literal is −(lit) in this crate.
            if let Some(value) = super::as_literal(left) {
                if is_atomic(right, AtomicOp::Natural) {
                    return if value >= zero {
                        btrue(pred)
                    } else {
                        bfalse(pred)
                    };
                }
                if is_atomic(right, AtomicOp::Natural1) {
                    return if value > zero {
                        btrue(pred)
                    } else {
                        bfalse(pred)
                    };
                }
            }
            // SIMP_IN_FUNIMAGE: E ↦ F(E) ∈ F == ⊤ (level 2)
            if let ExpressionKind::Binary {
                op: BinaryExprOp::Mapsto,
                left: e,
                right: fe,
            } = left.kind()
            {
                if let ExpressionKind::Binary {
                    op: BinaryExprOp::FunImage,
                    left: f,
                    right: arg,
                } = fe.kind()
                    && f == right
                    && arg == e
                {
                    return btrue(pred);
                }
                // SIMP_IN_FUNIMAGE_CONVERSE_L: F∼(E) ↦ E ∈ F == ⊤
                if let ExpressionKind::Binary {
                    op: BinaryExprOp::FunImage,
                    left: conv,
                    right: arg,
                } = e.kind()
                    && let ExpressionKind::Unary {
                        op: UnaryExprOp::Converse,
                        child: f,
                    } = conv.kind()
                    && f == right
                    && arg == fe
                {
                    return btrue(pred);
                }
                // SIMP_IN_FUNIMAGE_CONVERSE_R: F(E) ↦ E ∈ F∼ == ⊤
                if let (
                    ExpressionKind::Binary {
                        op: BinaryExprOp::FunImage,
                        left: f,
                        right: arg,
                    },
                    ExpressionKind::Unary {
                        op: UnaryExprOp::Converse,
                        child: f2,
                    },
                ) = (e.kind(), right.kind())
                    && f == f2
                    && arg == fe
                {
                    return btrue(pred);
                }
            }
            // DEF_IN_MAPSTO: a ↦ b ∈ A × B == a∈A ∧ b∈B (level 3)
            if let (
                ExpressionKind::Binary {
                    op: BinaryExprOp::Mapsto,
                    left: a,
                    right: b,
                },
                ExpressionKind::Binary {
                    op: BinaryExprOp::CProd,
                    left: aa,
                    right: bb,
                },
            ) = (left.kind(), right.kind())
            {
                return Some(conj(
                    pred,
                    vec![rel(RelationalOp::In, a, aa), rel(RelationalOp::In, b, bb)],
                ));
            }
            // DERIV_MULTI_IN_SETMINUS: E ∈ S ∖ {…, E, …} == ⊥ (level 3)
            if let ExpressionKind::Binary {
                op: BinaryExprOp::SetMinus,
                right: removed,
                ..
            } = right.kind()
                && let ExpressionKind::SetExtension(members) = removed.kind()
                && members.contains(left)
            {
                return bfalse(pred);
            }
            // DERIV_MULTI_IN_BUNION: E ∈ … ∪ {…, E, …} ∪ … == ⊤
            // (level 3)
            if let ExpressionKind::Associative {
                op: AssocExprOp::BUnion,
                children,
            } = right.kind()
            {
                let hit = children.iter().any(|child| {
                    matches!(child.kind(),
                        ExpressionKind::SetExtension(members) if members.contains(left))
                });
                if hit {
                    return btrue(pred);
                }
            }
        }
        RelationalOp::SubsetEq => {
            // SIMP_SUBSETEQ_COMPSET_L:
            // {x·P ∣ E} ⊆ S == ∀x·P ⇒ E ∈ S (level 2)
            if let ExpressionKind::Quantified {
                op: QuantExprOp::CSet,
                decls,
                pred: body,
                expr: value,
                ..
            } = left.kind()
            {
                let shifted = right.shift_bound_identifiers(decls.len() as i32);
                return Some(ff.quantified_predicate(
                    QuantPredOp::Forall,
                    decls.clone(),
                    ff.binary_predicate(
                        BinaryPredOp::LImp,
                        body.clone(),
                        rel(RelationalOp::In, value, &shifted),
                        None,
                    ),
                    None,
                ));
            }
        }
        _ => {}
    }
    // SIMP_UPTO_EQUAL_NATURAL(1): i‥j against ℕ/ℕ1 == ⊥ (level 4)
    let upto = |e: &Expression| {
        matches!(
            e.kind(),
            ExpressionKind::Binary {
                op: BinaryExprOp::UpTo,
                ..
            }
        )
    };
    let nat_like =
        |e: &Expression| is_atomic(e, AtomicOp::Natural) || is_atomic(e, AtomicOp::Natural1);
    if (op == RelationalOp::Equal && upto(left) && nat_like(right))
        || (matches!(
            op,
            RelationalOp::Subset | RelationalOp::SubsetEq | RelationalOp::Equal
        ) && nat_like(left)
            && upto(right))
    {
        return bfalse(pred);
    }
    // Equality with a type expression (level 4): E = Ty, Ty ⊆ E, Ty = E.
    let type_operand = match op {
        RelationalOp::Equal if right.is_type_expression() => Some(left),
        RelationalOp::Equal | RelationalOp::SubsetEq if left.is_type_expression() => Some(right),
        _ => None,
    };
    if let Some(expr) = type_operand
        && let Some(result) = rewrite_equals_type(pred, expr)
    {
        return Some(result);
    }
    if op == RelationalOp::In {
        // DERIV_PRJ1_SURJ / DERIV_PRJ2_SURJ / DERIV_ID_BIJ (level 4)
        if let ExpressionKind::Binary {
            op: arrow,
            left: ty1,
            right: ty2,
        } = right.kind()
            && ty1.is_type_expression()
            && ty2.is_type_expression()
        {
            let projection = matches!(
                left.kind(),
                ExpressionKind::Atomic(AtomicOp::KPrj1Gen)
                    | ExpressionKind::Atomic(AtomicOp::KPrj2Gen)
            );
            if projection && is_arrow(*arrow, false) {
                return btrue(pred);
            }
            if is_atomic(left, AtomicOp::KIdGen) && is_arrow(*arrow, true) {
                return btrue(pred);
            }
        }
        // SIMP_MIN_IN / SIMP_MAX_IN: min(S)∈S, max(S)∈S == ⊤ (level 5)
        if let ExpressionKind::Unary {
            op: UnaryExprOp::KMin | UnaryExprOp::KMax,
            child,
        } = left.kind()
            && child == right
        {
            return btrue(pred);
        }
        // The id specials (level 5).
        if let ExpressionKind::Binary {
            op: BinaryExprOp::Mapsto,
            left: e1,
            right: e2,
        } = left.kind()
            && e1 == e2
        {
            // SIMP_SPECIAL_IN_ID: E ↦ E ∈ id == ⊤
            if is_atomic(right, AtomicOp::KIdGen) {
                return btrue(pred);
            }
            if let ExpressionKind::Binary {
                op: BinaryExprOp::SetMinus,
                left: r,
                right: sub,
            } = right.kind()
            {
                // SIMP_SPECIAL_IN_SETMINUS_ID: E ↦ E ∈ r ∖ id == ⊥
                if is_atomic(sub, AtomicOp::KIdGen) {
                    return bfalse(pred);
                }
                // SIMP_SPECIAL_IN_SETMINUS_DOMRES_ID:
                // E ↦ E ∈ r ∖ (S ◁ id) == E ↦ E ∈ S ⩤ r
                if let ExpressionKind::Binary {
                    op: BinaryExprOp::DomRes,
                    left: s,
                    right: id,
                } = sub.kind()
                    && is_atomic(id, AtomicOp::KIdGen)
                {
                    return Some(rel(
                        RelationalOp::In,
                        left,
                        &ff.binary_expression(BinaryExprOp::DomSub, s.clone(), r.clone(), None),
                    ));
                }
            }
            // SIMP_SPECIAL_IN_DOMRES_ID: E ↦ E ∈ S ◁ id == E ∈ S
            if let ExpressionKind::Binary {
                op: BinaryExprOp::DomRes,
                left: s,
                right: id,
            } = right.kind()
                && is_atomic(id, AtomicOp::KIdGen)
            {
                return Some(rel(RelationalOp::In, e1, s));
            }
        }
    }
    None
}

/// `rewriteEqualsType` — the level-4 type-equality family; the other
/// operand is a type expression.
fn rewrite_equals_type(pred: &Predicate, expr: &Expression) -> Option<Predicate> {
    use rossi::formula::tag::{AssocExprOp, BinaryExprOp, QuantExprOp};
    let ff = pred.factory();
    // `makeIsType`: E = (the expression of E's type).
    let is_type = |e: &Expression| -> Option<Predicate> {
        let ty = e.ty()?;
        let Type::Pow(base) = ty else {
            return None;
        };
        let _ = base;
        Some(rel(RelationalOp::Equal, e, &ty_base_expression(e)?))
    };
    let are_all_type = |exprs: &[&Expression]| -> Option<Predicate> {
        let mut out = Vec::with_capacity(exprs.len());
        for e in exprs {
            out.push(is_type(e)?);
        }
        Some(conj(pred, out))
    };
    match expr.kind() {
        // SIMP_BINTER_EQUAL_TYPE: A ∩…∩ B = Ty == A=Ty ∧…∧ B=Ty
        ExpressionKind::Associative {
            op: AssocExprOp::BInter,
            children,
        } => are_all_type(&children.iter().collect::<Vec<_>>()),
        // SIMP_SETMINUS_EQUAL_TYPE: A ∖ B = Ty == A=Ty ∧ B=∅
        ExpressionKind::Binary { op, left, right } => match op {
            BinaryExprOp::SetMinus => Some(conj(pred, vec![is_type(left)?, is_empty(right)])),
            // SIMP_CPROD_EQUAL_TYPE: S × T = Ty == S=Ta ∧ T=Tb
            BinaryExprOp::CProd => are_all_type(&[left, right]),
            // SIMP_UPTO_EQUAL_INTEGER: i‥j = ℤ == ⊥
            BinaryExprOp::UpTo => bfalse(pred),
            // SIMP_DOMRES_EQUAL_TYPE: S ◁ r = Ty == S=Ta ∧ r=Ty
            BinaryExprOp::DomRes => are_all_type(&[left, right]),
            // SIMP_DOMSUB_EQUAL_TYPE: A ⩤ r = Ty == A=∅ ∧ r=Ty
            BinaryExprOp::DomSub => Some(conj(pred, vec![is_empty(left), is_type(right)?])),
            // SIMP_RANRES_EQUAL_TYPE: r ▷ A = Ty == A=Tb ∧ r=Ty
            BinaryExprOp::RanRes => are_all_type(&[right, left]),
            // SIMP_RANSUB_EQUAL_TYPE: r ⩥ A = Ty == A=∅ ∧ r=Ty
            BinaryExprOp::RanSub => Some(conj(pred, vec![is_empty(right), is_type(left)?])),
            // SIMP_DPROD_EQUAL_TYPE / SIMP_PPROD_EQUAL_TYPE
            BinaryExprOp::DProd | BinaryExprOp::PProd => are_all_type(&[left, right]),
            _ => None,
        },
        ExpressionKind::Unary { op, child } => match op {
            // SIMP_KINTER_EQUAL_TYPE: inter(S) = Ty == S = {Ty}
            UnaryExprOp::KInter => {
                let base = ty_base_expression(expr)?;
                let singleton = ff.set_extension(vec![base], None);
                Some(rel(RelationalOp::Equal, child, &singleton))
            }
            // SIMP_CONVERSE_EQUAL_TYPE: r∼ = T×S == r = S×T
            UnaryExprOp::Converse => is_type(child),
            _ => None,
        },
        // SIMP_QINTER_EQUAL_TYPE: (⋂x·P ∣ E) = Ty == ∀x·P ⇒ E=Ty
        ExpressionKind::Quantified {
            op: QuantExprOp::QInter,
            decls,
            pred: body,
            expr: value,
            ..
        } => Some(ff.quantified_predicate(
            QuantPredOp::Forall,
            decls.clone(),
            ff.binary_predicate(BinaryPredOp::LImp, body.clone(), is_type(value)?, None),
            None,
        )),
        _ => None,
    }
}

/// `getBaseTypeExpression`: the element type of a set-typed
/// expression, as an expression.
fn ty_base_expression(expr: &Expression) -> Option<Expression> {
    let Some(Type::Pow(base)) = expr.ty() else {
        return None;
    };
    Some(base.to_expression(expr.factory()))
}

/// `rewriteEqualsEmptySet` — the level-4 empty-set equality family.
fn rewrite_equals_empty_set(pred: &Predicate, expr: &Expression) -> Option<Predicate> {
    use rossi::formula::tag::{AssocExprOp, BinaryExprOp, QuantExprOp};
    let ff = pred.factory();
    match expr.kind() {
        // SIMP_SETENUM_EQUAL_EMPTY: {A…B} = ∅ == ⊥
        ExpressionKind::SetExtension(_) => bfalse(pred),
        ExpressionKind::Associative {
            op: AssocExprOp::BInter,
            children,
        } => {
            // SIMP_BINTER_SING_EQUAL_EMPTY:
            // A ∩…∩ {a} ∩…∩ B = ∅ == ¬ a ∈ A ∩…∩ B
            let singleton_at = children.iter().position(
                |child| matches!(child.kind(), ExpressionKind::SetExtension(m) if m.len() == 1),
            );
            if let Some(index) = singleton_at {
                let rest: Vec<Expression> = children
                    .iter()
                    .enumerate()
                    .filter(|(k, _)| *k != index)
                    .map(|(_, c)| c.clone())
                    .collect();
                let more_singletons = rest.iter().any(
                    |child| matches!(child.kind(), ExpressionKind::SetExtension(m) if m.len() == 1),
                );
                if rest.len() == 1 || !more_singletons {
                    let ExpressionKind::SetExtension(members) = children[index].kind() else {
                        unreachable!("singleton found above");
                    };
                    let element = &members[0];
                    let rest_set = if rest.len() == 1 {
                        rest.into_iter().next().unwrap()
                    } else {
                        ff.associative_expression(AssocExprOp::BInter, rest, None)
                    };
                    return Some(not(&rel(RelationalOp::In, element, &rest_set)));
                }
            }
            // SIMP_BINTER_SETMINUS_EQUAL_EMPTY:
            // (A ∖ B) ∩ C ∩ (D ∖ E) = ∅ == (A ∩ C ∩ D) ⊆ B ∪ E
            if children.iter().any(|child| {
                matches!(
                    child.kind(),
                    ExpressionKind::Binary {
                        op: BinaryExprOp::SetMinus,
                        ..
                    }
                )
            }) {
                let mut lhs = Vec::new();
                let mut rhs = Vec::new();
                for child in children {
                    if let ExpressionKind::Binary {
                        op: BinaryExprOp::SetMinus,
                        left,
                        right,
                    } = child.kind()
                    {
                        lhs.push(left.clone());
                        rhs.push(right.clone());
                    } else {
                        lhs.push(child.clone());
                    }
                }
                let make = |op: AssocExprOp, mut list: Vec<Expression>| {
                    if list.len() == 1 {
                        list.pop().unwrap()
                    } else {
                        ff.associative_expression(op, list, None)
                    }
                };
                return Some(rel(
                    RelationalOp::SubsetEq,
                    &make(AssocExprOp::BInter, lhs),
                    &make(AssocExprOp::BUnion, rhs),
                ));
            }
            None
        }
        // SIMP_BUNION_EQUAL_EMPTY: A ∪…∪ B = ∅ == A=∅ ∧…∧ B=∅
        ExpressionKind::Associative {
            op: AssocExprOp::BUnion,
            children,
        } => Some(conj(pred, children.iter().map(is_empty).collect())),
        ExpressionKind::Binary { op, left, right } => match op {
            // SIMP_SETMINUS_EQUAL_EMPTY: A ∖ B = ∅ == A ⊆ B
            BinaryExprOp::SetMinus => Some(rel(RelationalOp::SubsetEq, left, right)),
            // SIMP_CPROD_EQUAL_EMPTY: S × T = ∅ == S=∅ ∨ T=∅
            BinaryExprOp::CProd => Some(disj(pred, vec![is_empty(left), is_empty(right)])),
            // SIMP_UPTO_EQUAL_EMPTY: i‥j = ∅ == i > j
            BinaryExprOp::UpTo => Some(rel(RelationalOp::Gt, left, right)),
            // SIMP_SREL_EQUAL_EMPTY: A ⤔… == A=∅ ∧ ¬ B=∅
            BinaryExprOp::SRel => Some(conj(pred, vec![is_empty(left), is_not_empty(right)])),
            // SIMP_STREL_EQUAL_EMPTY: == A=∅ ⇔ ¬ B=∅
            BinaryExprOp::STRel => Some(ff.binary_predicate(
                BinaryPredOp::LEqv,
                is_empty(left),
                is_not_empty(right),
                None,
            )),
            // SIMP_DOMRES_EQUAL_EMPTY: S ◁ r = ∅ == dom(r) ∩ S = ∅
            BinaryExprOp::DomRes => Some(is_empty(&ff.associative_expression(
                AssocExprOp::BInter,
                vec![
                    ff.unary_expression(UnaryExprOp::KDom, right.clone(), None),
                    left.clone(),
                ],
                None,
            ))),
            // SIMP_DOMSUB_EQUAL_EMPTY: S ⩤ r = ∅ == dom(r) ⊆ S
            BinaryExprOp::DomSub => Some(rel(
                RelationalOp::SubsetEq,
                &ff.unary_expression(UnaryExprOp::KDom, right.clone(), None),
                left,
            )),
            // SIMP_RANRES_EQUAL_EMPTY: r ▷ S = ∅ == ran(r) ∩ S = ∅
            BinaryExprOp::RanRes => Some(is_empty(&ff.associative_expression(
                AssocExprOp::BInter,
                vec![
                    ff.unary_expression(UnaryExprOp::KRan, left.clone(), None),
                    right.clone(),
                ],
                None,
            ))),
            // SIMP_RANSUB_EQUAL_EMPTY: r ⩥ S = ∅ == ran(r) ⊆ S
            BinaryExprOp::RanSub => Some(rel(
                RelationalOp::SubsetEq,
                &ff.unary_expression(UnaryExprOp::KRan, left.clone(), None),
                right,
            )),
            // SIMP_RELIMAGE_EQUAL_EMPTY: r[S] = ∅ == S ◁ r = ∅
            BinaryExprOp::RelImage => Some(is_empty(&ff.binary_expression(
                BinaryExprOp::DomRes,
                right.clone(),
                left.clone(),
                None,
            ))),
            // SIMP_DPROD_EQUAL_EMPTY: p ⊗ q = ∅ == dom(p) ∩ dom(q) = ∅
            BinaryExprOp::DProd => Some(is_empty(&ff.associative_expression(
                AssocExprOp::BInter,
                vec![
                    ff.unary_expression(UnaryExprOp::KDom, left.clone(), None),
                    ff.unary_expression(UnaryExprOp::KDom, right.clone(), None),
                ],
                None,
            ))),
            // SIMP_PPROD_EQUAL_EMPTY: p ∥ q = ∅ == p=∅ ∨ q=∅
            BinaryExprOp::PProd => Some(disj(pred, vec![is_empty(left), is_empty(right)])),
            _ => None,
        },
        ExpressionKind::Unary { op, child } => match op {
            // SIMP_POW_EQUAL_EMPTY: ℙ(S) = ∅ == ⊥
            UnaryExprOp::Pow => bfalse(pred),
            // SIMP_POW1_EQUAL_EMPTY: ℙ1(S) = ∅ == S=∅
            UnaryExprOp::Pow1 => Some(is_empty(child)),
            // SIMP_KUNION_EQUAL_EMPTY: union(S) = ∅ == S ⊆ {∅}
            UnaryExprOp::KUnion => {
                let empty = ff.atomic_expression(AtomicOp::EmptySet, None, expr.ty().cloned());
                let singleton = ff.set_extension(vec![empty], None);
                Some(rel(RelationalOp::SubsetEq, child, &singleton))
            }
            // SIMP_DOM_EQUAL_EMPTY / SIMP_RAN_EQUAL_EMPTY /
            // SIMP_CONVERSE_EQUAL_EMPTY: the operand must be empty.
            UnaryExprOp::KDom | UnaryExprOp::KRan | UnaryExprOp::Converse => Some(is_empty(child)),
            _ => None,
        },
        ExpressionKind::Associative {
            op: AssocExprOp::FComp,
            children,
        } => {
            // SIMP_FCOMP_EQUAL_EMPTY: p ; q = ∅ == ran(p) ∩ dom(q) = ∅
            let [p, q] = children.as_slice() else {
                return None;
            };
            Some(is_empty(&ff.associative_expression(
                AssocExprOp::BInter,
                vec![
                    ff.unary_expression(UnaryExprOp::KRan, p.clone(), None),
                    ff.unary_expression(UnaryExprOp::KDom, q.clone(), None),
                ],
                None,
            )))
        }
        ExpressionKind::Associative {
            op: AssocExprOp::BComp,
            children,
        } => {
            // SIMP_BCOMP_EQUAL_EMPTY: p ∘ q = ∅ == ran(q) ∩ dom(p) = ∅
            let [p, q] = children.as_slice() else {
                return None;
            };
            Some(is_empty(&ff.associative_expression(
                AssocExprOp::BInter,
                vec![
                    ff.unary_expression(UnaryExprOp::KRan, q.clone(), None),
                    ff.unary_expression(UnaryExprOp::KDom, p.clone(), None),
                ],
                None,
            )))
        }
        // SIMP_OVERL_EQUAL_EMPTY: r  …  s = ∅ == r=∅ ∧ … ∧ s=∅
        ExpressionKind::Associative {
            op: AssocExprOp::Ovr,
            children,
        } => Some(conj(pred, children.iter().map(is_empty).collect())),
        // SIMP_QUNION_EQUAL_EMPTY: (⋃x·P ∣ E) = ∅ == ∀x·P ⇒ E=∅
        ExpressionKind::Quantified {
            op: QuantExprOp::QUnion,
            decls,
            pred: body,
            expr: value,
            ..
        } => Some(ff.quantified_predicate(
            QuantPredOp::Forall,
            decls.clone(),
            ff.binary_predicate(BinaryPredOp::LImp, body.clone(), is_empty(value), None),
            None,
        )),
        // SIMP_NATURAL_EQUAL_EMPTY / SIMP_NATURAL1_EQUAL_EMPTY == ⊥
        ExpressionKind::Atomic(AtomicOp::Natural) | ExpressionKind::Atomic(AtomicOp::Natural1) => {
            bfalse(pred)
        }
        // SIMP_ID_EQUAL_EMPTY / SIMP_PRJ1_EQUAL_EMPTY /
        // SIMP_PRJ2_EQUAL_EMPTY == ⊥
        ExpressionKind::Atomic(AtomicOp::KIdGen | AtomicOp::KPrj1Gen | AtomicOp::KPrj2Gen) => {
            bfalse(pred)
        }
        _ => None,
    }
}

/// The expression-level rules, dispatched by node kind.
fn rewrite_expression(expr: &Expression) -> Option<Expression> {
    match expr.kind() {
        ExpressionKind::Associative { .. } => rewrite_assoc_expr(expr),
        ExpressionKind::Binary { .. } => rewrite_binary_expr(expr),
        ExpressionKind::Unary { .. } => rewrite_unary_expr(expr),
        ExpressionKind::Atomic(_) => rewrite_atomic_expr(expr),
        ExpressionKind::Bool(_) => rewrite_bool_expr(expr),
        ExpressionKind::SetExtension(_) => rewrite_setext(expr),
        ExpressionKind::Quantified { .. } => rewrite_quant_expr(expr),
        _ => None,
    }
}

/// `DEF_PRED`: pred == succ∼ (level 3).
fn rewrite_atomic_expr(expr: &Expression) -> Option<Expression> {
    let ExpressionKind::Atomic(AtomicOp::KPred) = expr.kind() else {
        return None;
    };
    let ff = expr.factory();
    let succ = ff.atomic_expression(AtomicOp::KSucc, None, expr.ty().cloned());
    Some(ff.unary_expression(UnaryExprOp::Converse, succ, None))
}

/// The `bool(...)` rules.
fn rewrite_bool_expr(expr: &Expression) -> Option<Expression> {
    let ExpressionKind::Bool(inner) = expr.kind() else {
        return None;
    };
    let ff = expr.factory();
    match inner.kind() {
        // SIMP_SPECIAL_KBOOL_BFALSE: bool(⊥) == FALSE
        PredicateKind::Literal(LiteralPredOp::BFalse) => {
            Some(ff.atomic_expression(AtomicOp::False, None, None))
        }
        // SIMP_SPECIAL_KBOOL_BTRUE: bool(⊤) == TRUE
        PredicateKind::Literal(LiteralPredOp::BTrue) => {
            Some(ff.atomic_expression(AtomicOp::True, None, None))
        }
        // SIMP_KBOOL_LIT_EQUAL_TRUE: bool(B = TRUE) == B, both ways
        // (level 5)
        PredicateKind::Relational {
            op: RelationalOp::Equal,
            left,
            right,
        } => {
            if is_atomic(right, AtomicOp::True) {
                return Some(left.clone());
            }
            if is_atomic(left, AtomicOp::True) {
                return Some(right.clone());
            }
            None
        }
        _ => None,
    }
}

/// `SIMP_MULTI_SETENUM`: duplicate members
/// dropped, first occurrences kept.
fn rewrite_setext(expr: &Expression) -> Option<Expression> {
    let ExpressionKind::SetExtension(members) = expr.kind() else {
        return None;
    };
    let mut kept: Vec<Expression> = Vec::with_capacity(members.len());
    for member in members {
        if !kept.contains(member) {
            kept.push(member.clone());
        }
    }
    (kept.len() != members.len()).then(|| expr.factory().set_extension(kept, None))
}

/// The partial-lambda pattern check: the expression is a maplet tree of
/// pairwise-distinct locally bound identifiers.
fn partial_lambda_pattern_check(expr: &Expression, n_bound: u32) -> bool {
    fn check(expr: &Expression, n_bound: u32, seen: &mut Vec<u32>) -> bool {
        match expr.kind() {
            ExpressionKind::Binary {
                op: BinaryExprOp::Mapsto,
                left,
                right,
            } => check(left, n_bound, seen) && check(right, n_bound, seen),
            ExpressionKind::BoundIdentifier(index) => {
                if *index >= n_bound || seen.contains(index) {
                    return false;
                }
                seen.push(*index);
                true
            }
            _ => false,
        }
    }
    check(expr, n_bound, &mut Vec::new())
}

/// `AbstractRewriterImpl.notLocallyBound`.
fn not_locally_bound(expr: &Expression, n_bound: u32) -> bool {
    expr.dangling_bound_indices().iter().all(|&i| i >= n_bound)
}

/// The quantified-expression rules, in the reference pattern order.
fn rewrite_quant_expr(expr: &Expression) -> Option<Expression> {
    let ExpressionKind::Quantified {
        op,
        decls,
        pred: guard,
        expr: value,
        ..
    } = expr.kind()
    else {
        return None;
    };
    let ff = expr.factory();
    let n_bound = decls.len() as u32;
    match op {
        QuantExprOp::CSet => {
            // SIMP_SPECIAL_COMPSET_BFALSE: {x · ⊥ ∣ x} == ∅ (level 2)
            if matches!(guard.kind(), PredicateKind::Literal(LiteralPredOp::BFalse)) {
                return Some(typed_empty(expr));
            }
            // SIMP_SPECIAL_COMPSET_BTRUE: {x · ⊤ ∣ x} == Ty (level 2)
            if matches!(guard.kind(), PredicateKind::Literal(LiteralPredOp::BTrue))
                && partial_lambda_pattern_check(value, n_bound)
            {
                return Some(value.ty()?.to_expression(ff));
            }
            // SIMP_COMPSET_IN: {x · x∈S ∣ x} == S (level 2)
            if let PredicateKind::Relational {
                op: RelationalOp::In,
                left,
                right,
            } = guard.kind()
                && left == value
                && not_locally_bound(right, n_bound)
                && partial_lambda_pattern_check(value, n_bound)
            {
                return Some(right.shift_bound_identifiers(-(n_bound as i32)));
            }
            // SIMP_COMPSET_SUBSETEQ: {x · x⊆S ∣ x} == ℙ(S) (level 2)
            if let PredicateKind::Relational {
                op: RelationalOp::SubsetEq,
                left,
                right,
            } = guard.kind()
                && let ExpressionKind::BoundIdentifier(index) = left.kind()
                && left == value
                && not_locally_bound(right, n_bound)
                && *index < n_bound
            {
                return Some(ff.unary_expression(
                    UnaryExprOp::Pow,
                    right.shift_bound_identifiers(-(n_bound as i32)),
                    None,
                ));
            }
            None
        }
        // SIMP_SPECIAL_QUNION: ⋃x · ⊥ ∣ E == ∅ (level 2)
        QuantExprOp::QUnion => {
            matches!(guard.kind(), PredicateKind::Literal(LiteralPredOp::BFalse))
                .then(|| typed_empty(expr))
        }
        _ => None,
    }
}

/// `convertSetextOfMapsto`: every maplet member reversed; `None`
/// when a member is not a maplet (no deduplication).
fn convert_setext_of_mapsto(members: &[Expression]) -> Option<Expression> {
    let ff = members.first()?.factory();
    let mut reversed = Vec::with_capacity(members.len());
    for member in members {
        let ExpressionKind::Binary {
            op: BinaryExprOp::Mapsto,
            left,
            right,
        } = member.kind()
        else {
            return None;
        };
        reversed.push(ff.binary_expression(
            BinaryExprOp::Mapsto,
            right.clone(),
            left.clone(),
            None,
        ));
    }
    Some(ff.set_extension(reversed, None))
}

/// `SIMP_DOM_SETENUM` / `SIMP_RAN_SETENUM`: one maplet side of every
/// member, duplicates dropped; `None` when a member is not a maplet.
fn setenum_sides(members: &[Expression], left_side: bool) -> Option<Expression> {
    let mut sides: Vec<Expression> = Vec::with_capacity(members.len());
    for member in members {
        let ExpressionKind::Binary {
            op: BinaryExprOp::Mapsto,
            left,
            right,
        } = member.kind()
        else {
            return None;
        };
        let side = if left_side { left } else { right };
        if !sides.contains(side) {
            sides.push(side.clone());
        }
    }
    Some(members[0].factory().set_extension(sides, None))
}

/// `simplifyExtremumOfUnion`: union children of the form `{min(T)}`
/// (or `{max(T)}`, matching the surrounding extremum) become `T`.
fn simplify_extremum_of_union(children: &[Expression], op: UnaryExprOp) -> Option<Expression> {
    let ff = children[0].factory();
    let mut changed = false;
    let new_children: Vec<Expression> = children
        .iter()
        .map(|child| {
            if let ExpressionKind::SetExtension(members) = child.kind()
                && let [single] = members.as_slice()
                && let ExpressionKind::Unary {
                    op: inner,
                    child: set,
                } = single.kind()
                && *inner == op
            {
                changed = true;
                return set.clone();
            }
            child.clone()
        })
        .collect();
    changed.then(|| {
        ff.unary_expression(
            op,
            ff.associative_expression(AssocExprOp::BUnion, new_children, None),
            None,
        )
    })
}

/// Set-extension min/max simplification: with at least
/// two literal members, only the extremum literal survives, inserted
/// where the non-literal prefix scanned so far left room for it.
/// Sees a unary minus of a literal as a negative literal.
fn simplify_min_max(members: &[Expression], keep_min: bool) -> Option<Expression> {
    let mut kept: Vec<Expression> = Vec::new();
    let mut extremum: Option<(Expression, num_bigint::BigInt, usize)> = None;
    let mut n_literals = 0usize;
    for member in members {
        let Some(value) = super::as_literal(member) else {
            kept.push(member.clone());
            continue;
        };
        n_literals += 1;
        let better = match &extremum {
            None => true,
            Some((_, best, _)) => {
                if keep_min {
                    *best > value
                } else {
                    *best < value
                }
            }
        };
        if better {
            extremum = Some((member.clone(), value, kept.len()));
        }
    }
    if n_literals < 2 {
        return None;
    }
    let (chosen, _, position) = extremum.expect("two literals imply an extremum");
    kept.insert(position, chosen);
    Some(members[0].factory().set_extension(kept, None))
}

/// `getExpressions`: every `size`-element combination of
/// `children[from..]`, members in child order.
fn combinations(children: &[Expression], from: usize, size: usize) -> Vec<Vec<Expression>> {
    if size == 0 {
        return vec![Vec::new()];
    }
    let mut result = Vec::new();
    let mut i = from;
    while i + size <= children.len() {
        for mut rest in combinations(children, i + 1, size - 1) {
            let mut combo = Vec::with_capacity(size);
            combo.push(children[i].clone());
            combo.append(&mut rest);
            result.push(combo);
        }
        i += 1;
    }
    result
}

/// `SIMP_CARD_BUNION`: inclusion–exclusion, alternating a binary
/// minus and a two-child sum exactly as the stored rules have them.
fn card_bunion(children: &[Expression]) -> Expression {
    let ff = children[0].factory();
    let card = |e: Expression| ff.unary_expression(UnaryExprOp::KCard, e, None);
    let length = children.len();
    let mut sub_formulas: Vec<Expression> = Vec::with_capacity(length);
    for size in 1..=length {
        let cards: Vec<Expression> = combinations(children, 0, size)
            .into_iter()
            .map(|combo| {
                card(if combo.len() == 1 {
                    combo.into_iter().next().unwrap()
                } else {
                    ff.associative_expression(AssocExprOp::BInter, combo, None)
                })
            })
            .collect();
        sub_formulas.push(if cards.len() == 1 {
            cards.into_iter().next().unwrap()
        } else {
            ff.associative_expression(AssocExprOp::Plus, cards, None)
        });
    }
    let mut terms = sub_formulas.into_iter();
    let mut result = terms.next().expect("a union has children");
    let mut positive = false;
    for term in terms {
        result = if positive {
            ff.associative_expression(AssocExprOp::Plus, vec![result, term], None)
        } else {
            ff.binary_expression(BinaryExprOp::Minus, result, term, None)
        };
        positive = !positive;
    }
    result
}

/// The unary-expression rules, in the reference pattern order per
/// operator (patterns of distinct operators never overlap).
fn rewrite_unary_expr(expr: &Expression) -> Option<Expression> {
    let ExpressionKind::Unary { op, child } = expr.kind() else {
        return None;
    };
    let ff = expr.factory();
    let card = |e: &Expression| ff.unary_expression(UnaryExprOp::KCard, e.clone(), None);
    // The source (or target) side of a relation-typed expression, as
    // an expression, taken from its solved type.
    let source_type = |e: &Expression, source_side: bool| -> Option<Expression> {
        let Some(Type::Pow(pair)) = e.ty() else {
            return None;
        };
        let Type::Prod(source, target) = &**pair else {
            return None;
        };
        Some(if source_side { source } else { target }.to_expression(ff))
    };
    let is_generic = |e: &Expression| {
        matches!(
            e.kind(),
            ExpressionKind::Atomic(AtomicOp::KIdGen)
                | ExpressionKind::Atomic(AtomicOp::KPrj1Gen)
                | ExpressionKind::Atomic(AtomicOp::KPrj2Gen)
        )
    };
    let cset_side = |left_side: bool| -> Option<Expression> {
        let ExpressionKind::Quantified {
            op: QuantExprOp::CSet,
            decls,
            pred,
            expr: value,
            ..
        } = child.kind()
        else {
            return None;
        };
        let ExpressionKind::Binary {
            op: BinaryExprOp::Mapsto,
            left,
            right,
        } = value.kind()
        else {
            return None;
        };
        Some(ff.quantified_expression(
            QuantExprOp::CSet,
            decls.clone(),
            pred.clone(),
            if left_side { left } else { right }.clone(),
            None,
            rossi::formula::Form::Explicit,
        ))
    };
    match op {
        UnaryExprOp::Converse => match child.kind() {
            // SIMP_CONVERSE_CONVERSE: r∼∼ == r
            ExpressionKind::Unary {
                op: UnaryExprOp::Converse,
                child: r,
            } => Some(r.clone()),
            // SIMP_CONVERSE_SETENUM: {x↦a, …}∼ == {a↦x, …}
            ExpressionKind::SetExtension(members) => convert_setext_of_mapsto(members),
            // SIMP_CONVERSE_ID: id∼ == id (level 2)
            ExpressionKind::Atomic(AtomicOp::KIdGen) => Some(child.clone()),
            // SIMP_SPECIAL_CONVERSE: ∅∼ == ∅ (level 2)
            ExpressionKind::Atomic(AtomicOp::EmptySet) => Some(typed_empty(expr)),
            // SIMP_CONVERSE_CPROD: (A × B)∼ == B × A (level 2)
            ExpressionKind::Binary {
                op: BinaryExprOp::CProd,
                left,
                right,
            } => Some(ff.binary_expression(BinaryExprOp::CProd, right.clone(), left.clone(), None)),
            // SIMP_CONVERSE_COMPSET: {X·P ∣ x↦y}∼ == {X·P ∣ y↦x}
            // (level 2)
            ExpressionKind::Quantified { .. } => {
                let ExpressionKind::Quantified {
                    op: QuantExprOp::CSet,
                    decls,
                    pred,
                    expr: value,
                    ..
                } = child.kind()
                else {
                    return None;
                };
                let ExpressionKind::Binary {
                    op: BinaryExprOp::Mapsto,
                    left: x,
                    right: y,
                } = value.kind()
                else {
                    return None;
                };
                Some(ff.quantified_expression(
                    QuantExprOp::CSet,
                    decls.clone(),
                    pred.clone(),
                    ff.binary_expression(BinaryExprOp::Mapsto, y.clone(), x.clone(), None),
                    None,
                    rossi::formula::Form::Explicit,
                ))
            }
            _ => None,
        },
        UnaryExprOp::KDom => match child.kind() {
            // SIMP_DOM_SETENUM: dom({x↦a, …}) == {x, …}, duplicates
            // dropped; a non-maplet member stops the rule
            ExpressionKind::SetExtension(members) => setenum_sides(members, true),
            // SIMP_SPECIAL_DOM: dom(∅) == ∅
            ExpressionKind::Atomic(AtomicOp::EmptySet) => {
                let Some(Type::Pow(pair)) = child.ty() else {
                    return None;
                };
                let Type::Prod(source, _) = &**pair else {
                    return None;
                };
                Some(ff.atomic_expression(
                    AtomicOp::EmptySet,
                    None,
                    Some(Type::Pow(source.clone())),
                ))
            }
            // SIMP_DOM_CONVERSE: dom(r∼) == ran(r) (level 2)
            ExpressionKind::Unary {
                op: UnaryExprOp::Converse,
                child: r,
            } => Some(ff.unary_expression(UnaryExprOp::KRan, r.clone(), None)),
            ExpressionKind::Binary {
                op: BinaryExprOp::CProd,
                left,
                right,
            } => {
                // SIMP_MULTI_DOM_CPROD: dom(E×E) == E (level 2)
                if left == right {
                    return Some(left.clone());
                }
                // SIMP_TYPE_DOM: dom(Ta×Tb) == Ta (level 2)
                (left.is_type_expression() && right.is_type_expression()).then(|| left.clone())
            }
            // SIMP_DOM_ID: dom(id) == S (level 2);
            // SIMP_DOM_PRJ1 / SIMP_DOM_PRJ2: dom(prjN) == S×T (level 2)
            ExpressionKind::Atomic(AtomicOp::KIdGen | AtomicOp::KPrj1Gen | AtomicOp::KPrj2Gen) => {
                source_type(child, true)
            }
            // SIMP_DOM_LAMBDA: dom({x·P ∣ E↦F}) == {x·P ∣ E} (level 2)
            ExpressionKind::Quantified { .. } => cset_side(true),
            // SIMP_MULTI_DOM_DOMSUB: dom(A⩤f) == dom(f)∖A (level 3)
            ExpressionKind::Binary {
                op: BinaryExprOp::DomSub,
                left: a,
                right: f,
            } => Some(ff.binary_expression(
                BinaryExprOp::SetMinus,
                ff.unary_expression(UnaryExprOp::KDom, f.clone(), None),
                a.clone(),
                None,
            )),
            // SIMP_MULTI_DOM_DOMRES: dom(A◁f) == dom(f)∩A (level 3)
            ExpressionKind::Binary {
                op: BinaryExprOp::DomRes,
                left: a,
                right: f,
            } => Some(ff.associative_expression(
                AssocExprOp::BInter,
                vec![
                    ff.unary_expression(UnaryExprOp::KDom, f.clone(), None),
                    a.clone(),
                ],
                None,
            )),
            // SIMP_DOM_SUCC: dom(succ) == ℤ (level 3)
            ExpressionKind::Atomic(AtomicOp::KSucc) => {
                Some(ff.atomic_expression(AtomicOp::Integer, None, None))
            }
            _ => None,
        },
        UnaryExprOp::KRan => match child.kind() {
            // SIMP_RAN_SETENUM: ran({x↦a, …}) == {a, …}
            ExpressionKind::SetExtension(members) => setenum_sides(members, false),
            // SIMP_SPECIAL_RAN: ran(∅) == ∅
            ExpressionKind::Atomic(AtomicOp::EmptySet) => {
                let Some(Type::Pow(pair)) = child.ty() else {
                    return None;
                };
                let Type::Prod(_, target) = &**pair else {
                    return None;
                };
                Some(ff.atomic_expression(
                    AtomicOp::EmptySet,
                    None,
                    Some(Type::Pow(target.clone())),
                ))
            }
            // SIMP_RAN_CONVERSE: ran(r∼) == dom(r) (level 2)
            ExpressionKind::Unary {
                op: UnaryExprOp::Converse,
                child: r,
            } => Some(ff.unary_expression(UnaryExprOp::KDom, r.clone(), None)),
            ExpressionKind::Binary {
                op: BinaryExprOp::CProd,
                left,
                right,
            } => {
                // SIMP_MULTI_RAN_CPROD: ran(E×E) == E (level 2)
                if left == right {
                    return Some(right.clone());
                }
                // SIMP_TYPE_RAN: ran(Ta×Tb) == Tb (level 2)
                (left.is_type_expression() && right.is_type_expression()).then(|| right.clone())
            }
            // SIMP_RAN_ID: ran(id) == S (the source side too);
            // SIMP_RAN_PRJ1 / SIMP_RAN_PRJ2: ran(prjN) == S or T
            // (level 2)
            ExpressionKind::Atomic(AtomicOp::KIdGen) => source_type(child, true),
            ExpressionKind::Atomic(AtomicOp::KPrj1Gen | AtomicOp::KPrj2Gen) => {
                source_type(child, false)
            }
            // SIMP_RAN_LAMBDA: ran({x·P ∣ E↦F}) == {x·P ∣ F} (level 2)
            ExpressionKind::Quantified { .. } => cset_side(false),
            // SIMP_MULTI_RAN_RANSUB: ran(f⩥A) == ran(f)∖A (level 3)
            ExpressionKind::Binary {
                op: BinaryExprOp::RanSub,
                left: f,
                right: a,
            } => Some(ff.binary_expression(
                BinaryExprOp::SetMinus,
                ff.unary_expression(UnaryExprOp::KRan, f.clone(), None),
                a.clone(),
                None,
            )),
            // SIMP_MULTI_RAN_RANRES: ran(f▷A) == ran(f)∩A (level 3)
            ExpressionKind::Binary {
                op: BinaryExprOp::RanRes,
                left: f,
                right: a,
            } => Some(ff.associative_expression(
                AssocExprOp::BInter,
                vec![
                    ff.unary_expression(UnaryExprOp::KRan, f.clone(), None),
                    a.clone(),
                ],
                None,
            )),
            // SIMP_RAN_SUCC: ran(succ) == ℤ (level 3)
            ExpressionKind::Atomic(AtomicOp::KSucc) => {
                Some(ff.atomic_expression(AtomicOp::Integer, None, None))
            }
            _ => None,
        },
        // SIMP_MINUS_MINUS: −(−E) == E
        UnaryExprOp::UnMinus => match child.kind() {
            ExpressionKind::Unary {
                op: UnaryExprOp::UnMinus,
                child: e,
            } => Some(e.clone()),
            _ => None,
        },
        UnaryExprOp::KCard => match child.kind() {
            // SIMP_SPECIAL_CARD: card(∅) == 0
            ExpressionKind::Atomic(AtomicOp::EmptySet) => Some(ff.integer_literal(0, None)),
            // SIMP_CARD_SING: card({E}) == 1
            ExpressionKind::SetExtension(members) if members.len() == 1 => {
                Some(ff.integer_literal(1, None))
            }
            // SIMP_CARD_POW: card(ℙ(S)) == 2^card(S)
            ExpressionKind::Unary {
                op: UnaryExprOp::Pow,
                child: s,
            } => Some(ff.binary_expression(
                BinaryExprOp::Expn,
                ff.integer_literal(2, None),
                card(s),
                None,
            )),
            // SIMP_CARD_BUNION: inclusion–exclusion
            ExpressionKind::Associative {
                op: AssocExprOp::BUnion,
                children,
            } => Some(card_bunion(children)),
            // SIMP_CARD_CONVERSE: card(r∼) == card(r) (level 2)
            ExpressionKind::Unary {
                op: UnaryExprOp::Converse,
                child: r,
            } => Some(card(r)),
            // SIMP_CARD_ID / PRJ1 / PRJ2: card of the source type
            // (level 2)
            ExpressionKind::Atomic(AtomicOp::KIdGen | AtomicOp::KPrj1Gen | AtomicOp::KPrj2Gen) => {
                Some(card(&source_type(child, true)?))
            }
            // SIMP_CARD_ID_DOMRES / PRJ1_DOMRES / PRJ2_DOMRES:
            // card(E ◁ generic) == card(E) (level 2)
            ExpressionKind::Binary {
                op: BinaryExprOp::DomRes,
                left: e,
                right: generic,
            } if is_generic(generic) => Some(card(e)),
            // SIMP_CARD_LAMBDA: card({x·P ∣ E↦F}) == card({x·P ∣ E})
            // (level 2, when the pattern is functional)
            ExpressionKind::Quantified {
                op: QuantExprOp::CSet,
                decls,
                expr: value,
                ..
            } => {
                let ExpressionKind::Binary {
                    op: BinaryExprOp::Mapsto,
                    left: e,
                    right: f,
                } = value.kind()
                else {
                    return None;
                };
                if !functional_check(e, f, decls.len() as u32) {
                    return None;
                }
                Some(card(&cset_side(true)?))
            }
            _ => None,
        },
        UnaryExprOp::Pow => match child.kind() {
            // SIMP_SPECIAL_POW: ℙ(∅) == {∅} (level 2)
            ExpressionKind::Atomic(AtomicOp::EmptySet) => {
                Some(ff.set_extension(vec![child.clone()], None))
            }
            _ => None,
        },
        UnaryExprOp::Pow1 => match child.kind() {
            // SIMP_SPECIAL_POW1: ℙ1(∅) == ∅ (level 2)
            ExpressionKind::Atomic(AtomicOp::EmptySet) => Some(typed_empty(expr)),
            _ => None,
        },
        UnaryExprOp::KUnion => match child.kind() {
            // SIMP_KUNION_POW / SIMP_KUNION_POW1: union(ℙ(S)) == S
            // (level 2)
            ExpressionKind::Unary {
                op: UnaryExprOp::Pow | UnaryExprOp::Pow1,
                child: s,
            } => Some(s.clone()),
            // SIMP_SPECIAL_KUNION: union({∅}) == ∅ (level 2)
            ExpressionKind::SetExtension(members) => match members.as_slice() {
                [single] if is_atomic(single, AtomicOp::EmptySet) => Some(single.clone()),
                _ => None,
            },
            _ => None,
        },
        UnaryExprOp::KInter => match child.kind() {
            // SIMP_SPECIAL_KINTER: inter({∅}) == ∅ (level 2)
            ExpressionKind::SetExtension(members) => match members.as_slice() {
                [single] if is_atomic(single, AtomicOp::EmptySet) => Some(single.clone()),
                _ => None,
            },
            // SIMP_KINTER_POW: inter(ℙ(S)) == ∅ (level 2)
            ExpressionKind::Unary {
                op: UnaryExprOp::Pow,
                child: s,
            } => Some(typed_empty(s)),
            _ => None,
        },
        UnaryExprOp::KMin => match child.kind() {
            ExpressionKind::SetExtension(members) => {
                // SIMP_MIN_SING: min({E}) == E (level 2)
                if let [single] = members.as_slice() {
                    return Some(single.clone());
                }
                // SIMP_LIT_MIN (level 2)
                simplify_min_max(members, true)
                    .map(|set| ff.unary_expression(UnaryExprOp::KMin, set, None))
            }
            // SIMP_MIN_NATURAL: min(ℕ) == 0 (level 2)
            ExpressionKind::Atomic(AtomicOp::Natural) => Some(ff.integer_literal(0, None)),
            // SIMP_MIN_NATURAL1: min(ℕ1) == 1 (level 2)
            ExpressionKind::Atomic(AtomicOp::Natural1) => Some(ff.integer_literal(1, None)),
            // SIMP_MIN_UPTO: min(E‥F) == E (level 2)
            ExpressionKind::Binary {
                op: BinaryExprOp::UpTo,
                left,
                ..
            } => Some(left.clone()),
            // SIMP_MIN_BUNION_SING (level 2)
            ExpressionKind::Associative {
                op: AssocExprOp::BUnion,
                children,
            } => simplify_extremum_of_union(children, UnaryExprOp::KMin),
            _ => None,
        },
        UnaryExprOp::KMax => match child.kind() {
            ExpressionKind::SetExtension(members) => {
                // SIMP_MAX_SING: max({E}) == E (level 2)
                if let [single] = members.as_slice() {
                    return Some(single.clone());
                }
                // SIMP_LIT_MAX (level 2)
                simplify_min_max(members, false)
                    .map(|set| ff.unary_expression(UnaryExprOp::KMax, set, None))
            }
            // SIMP_MAX_UPTO: max(E‥F) == F (level 2)
            ExpressionKind::Binary {
                op: BinaryExprOp::UpTo,
                right,
                ..
            } => Some(right.clone()),
            // SIMP_MAX_BUNION_SING (level 2)
            ExpressionKind::Associative {
                op: AssocExprOp::BUnion,
                children,
            } => simplify_extremum_of_union(children, UnaryExprOp::KMax),
            _ => None,
        },
    }
}

/// The lambda computer — `SIMP_FUNIMAGE_LAMBDA`: computes
/// `{x · P ∣ E ↦ F}(y)` by solving `∃x · y ↦ A = E ↦ F` down to
/// `A = image`. The reference runs the inner fixpoint with an L0
/// rewriter; this reuses the L5 rewriter (levels are not modelled), which
/// can only simplify further — the corpus replay gate audits the
/// difference.
fn lambda_computer(expr: &Expression) -> Option<Expression> {
    let ExpressionKind::Binary {
        op: BinaryExprOp::FunImage,
        left: cset,
        right: arg,
    } = expr.kind()
    else {
        return None;
    };
    let ExpressionKind::Quantified {
        op: QuantExprOp::CSet,
        decls,
        expr: value,
        ..
    } = cset.kind()
    else {
        return None;
    };
    if !matches!(
        value.kind(),
        ExpressionKind::Binary {
            op: BinaryExprOp::Mapsto,
            ..
        }
    ) {
        return None;
    }
    let ff = expr.factory();
    let n = decls.len() as u32;
    // A fresh variable A above the comprehension's binders: the
    // equation lives under ∃decls, with A dangling at index 0
    // outside it.
    let a = ff.bound_identifier(n, None, expr.ty().cloned());
    let inner_arg = arg.shift_bound_identifiers(1 + n as i32);
    let y_mapsto_a = ff.binary_expression(BinaryExprOp::Mapsto, inner_arg, a, None);
    let shifted_cset = cset.shift_bound_identifiers(1);
    let ExpressionKind::Quantified {
        expr: inner_value, ..
    } = shifted_cset.kind()
    else {
        return None;
    };
    let equals =
        ff.relational_predicate(RelationalOp::Equal, y_mapsto_a, inner_value.clone(), None);
    let mut pred = ff.quantified_predicate(QuantPredOp::Exists, decls.clone(), equals, None);
    let mut rewriter = AutoRewriter;
    loop {
        let mut changed = false;
        if let Some(next) = rewrite_pred(&pred, &mut rewriter) {
            pred = next;
            changed = true;
        }
        if let Some(next) = one_point::one_point_inference(&pred) {
            pred = next;
            changed = true;
        }
        if !changed {
            break;
        }
    }
    // Expected final form: A = image, with A gone from the image.
    let PredicateKind::Relational {
        op: RelationalOp::Equal,
        left,
        right,
    } = pred.kind()
    else {
        return None;
    };
    if !matches!(left.kind(), ExpressionKind::BoundIdentifier(0)) {
        return None;
    }
    if right.dangling_bound_indices().contains(&0) {
        return None;
    }
    Some(right.shift_bound_identifiers(-1))
}

/// The binary-expression rules, in the reference pattern order per
/// operator (patterns of distinct operators never overlap).
fn rewrite_binary_expr(expr: &Expression) -> Option<Expression> {
    use rossi::formula::tag::{AssocExprOp, BinaryExprOp};
    let ExpressionKind::Binary { op, left, right } = expr.kind() else {
        return None;
    };
    let ff = expr.factory();
    let lit0 = |e: &Expression| is_int_value(e, 0);
    let lit1 = |e: &Expression| is_int_value(e, 1);
    let un_minus = |e: &Expression| match e.kind() {
        ExpressionKind::Unary {
            op: UnaryExprOp::UnMinus,
            child,
        } => Some(child.clone()),
        _ => None,
    };
    let dom_of = |e: &Expression| match e.kind() {
        ExpressionKind::Unary {
            op: UnaryExprOp::KDom,
            child,
        } => Some(child.clone()),
        _ => None,
    };
    let ran_of = |e: &Expression| match e.kind() {
        ExpressionKind::Unary {
            op: UnaryExprOp::KRan,
            child,
        } => Some(child.clone()),
        _ => None,
    };
    let converse_of = |e: &Expression| match e.kind() {
        ExpressionKind::Unary {
            op: UnaryExprOp::Converse,
            child,
        } => Some(child.clone()),
        _ => None,
    };
    let domres_id = |e: &Expression| -> Option<Expression> {
        let ExpressionKind::Binary {
            op: BinaryExprOp::DomRes,
            left: s,
            right: id,
        } = e.kind()
        else {
            return None;
        };
        is_atomic(id, AtomicOp::KIdGen).then(|| s.clone())
    };
    let domsub_id = |e: &Expression| -> Option<Expression> {
        let ExpressionKind::Binary {
            op: BinaryExprOp::DomSub,
            left: s,
            right: id,
        } = e.kind()
        else {
            return None;
        };
        is_atomic(id, AtomicOp::KIdGen).then(|| s.clone())
    };
    let id_atom = || {
        // The identity keeps the type of the restricted relation.
        ff.atomic_expression(AtomicOp::KIdGen, None, expr.ty().cloned())
    };
    let binter2 = |a: &Expression, b: &Expression| {
        ff.associative_expression(AssocExprOp::BInter, vec![a.clone(), b.clone()], None)
    };
    let setminus = |a: &Expression, b: &Expression| {
        ff.binary_expression(BinaryExprOp::SetMinus, a.clone(), b.clone(), None)
    };
    match op {
        BinaryExprOp::SetMinus => {
            // SIMP_MULTI_SETMINUS: S ∖ S == ∅
            if left == right {
                return Some(typed_empty(expr));
            }
            // SIMP_SPECIAL_SETMINUS_L: ∅ ∖ S == ∅
            if is_atomic(left, AtomicOp::EmptySet) {
                return Some(left.clone());
            }
            // SIMP_SPECIAL_SETMINUS_R: S ∖ ∅ == S
            if is_atomic(right, AtomicOp::EmptySet) {
                return Some(left.clone());
            }
            // SIMP_TYPE_SETMINUS: S ∖ U == ∅ (U a type expression)
            if right.is_type_expression() {
                return Some(typed_empty(expr));
            }
            // SIMP_TYPE_SETMINUS_SETMINUS: U ∖ (U ∖ S) == S
            if left.is_type_expression()
                && let ExpressionKind::Binary {
                    op: BinaryExprOp::SetMinus,
                    left: u2,
                    right: s,
                } = right.kind()
                && u2 == left
            {
                return Some(s.clone());
            }
            None
        }
        BinaryExprOp::Minus => {
            // SIMP_MULTI_MINUS: E − E == 0
            if left == right {
                return Some(ff.integer_literal(0, None));
            }
            // SIMP_SPECIAL_MINUS_R: E − 0 == E
            if lit0(right) {
                return Some(left.clone());
            }
            // SIMP_SPECIAL_MINUS_L: 0 − E == −E
            if lit0(left) {
                return Some(ff.unary_expression(UnaryExprOp::UnMinus, right.clone(), None));
            }
            None
        }
        BinaryExprOp::Div => {
            // SIMP_MULTI_DIV: E ÷ E == 1
            if left == right {
                return Some(ff.integer_literal(1, None));
            }
            // SIMP_SPECIAL_DIV_1: E ÷ 1 == E
            if lit1(right) {
                return Some(left.clone());
            }
            // SIMP_SPECIAL_DIV_0: 0 ÷ E == 0
            if lit0(left) {
                return Some(ff.integer_literal(0, None));
            }
            // SIMP_MULTI_DIV_PROD: (X ∗ … ∗ E ∗ …) ÷ E == X ∗ …
            if let ExpressionKind::Associative {
                op: AssocExprOp::Mul,
                children,
            } = left.kind()
                && let Some(index) = children.iter().position(|c| c == right)
            {
                let rest: Vec<Expression> = children
                    .iter()
                    .enumerate()
                    .filter(|(k, _)| *k != index)
                    .map(|(_, c)| c.clone())
                    .collect();
                return Some(if rest.len() == 1 {
                    rest.into_iter().next().unwrap()
                } else {
                    ff.associative_expression(AssocExprOp::Mul, rest, None)
                });
            }
            // SIMP_DIV_MINUS: (−E) ÷ (−F) == E ÷ F — this also covers
            // The negative-literal overloads, whose values are
            // −(lit) shapes here.
            if let (Some(e), Some(f)) = (un_minus(left), un_minus(right)) {
                return Some(ff.binary_expression(BinaryExprOp::Div, e, f, None));
            }
            None
        }
        BinaryExprOp::Expn => {
            // SIMP_SPECIAL_EXPN_1_R / _0 / _1_L
            if lit1(right) {
                return Some(left.clone());
            }
            if lit0(right) {
                return Some(ff.integer_literal(1, None));
            }
            if lit1(left) {
                return Some(ff.integer_literal(1, None));
            }
            None
        }
        BinaryExprOp::FunImage => {
            // SIMP_FUNIMAGE_FUNIMAGE_CONVERSE: f(f∼(E)) == E
            if let ExpressionKind::Binary {
                op: BinaryExprOp::FunImage,
                left: inner_f,
                right: e,
            } = right.kind()
            {
                if let Some(f) = converse_of(inner_f)
                    && &f == left
                {
                    return Some(e.clone());
                }
                // SIMP_FUNIMAGE_CONVERSE_FUNIMAGE: f∼(f(E)) == E
                if let Some(f) = converse_of(left)
                    && &f == inner_f
                {
                    return Some(e.clone());
                }
            }
            // SIMP_MULTI_FUNIMAGE_OVERL_SETENUM: (f  {…, E ↦ F})(E) == F
            if let ExpressionKind::Associative {
                op: AssocExprOp::Ovr,
                children,
            } = left.kind()
                && let Some(last) = children.last()
                && let ExpressionKind::SetExtension(members) = last.kind()
            {
                for member in members {
                    if let ExpressionKind::Binary {
                        op: BinaryExprOp::Mapsto,
                        left: x,
                        right: y,
                    } = member.kind()
                        && x == right
                    {
                        return Some(y.clone());
                    }
                }
            }
            // SIMP_MULTI_FUNIMAGE_BUNION_SETENUM handled below for
            // BUnion functions.
            // SIMP_FUNIMAGE_FUNIMAGE_CONVERSE_SETENUM:
            // {x ↦ a, …}({a ↦ x, …}(E)) == E
            if let (
                ExpressionKind::SetExtension(outer),
                ExpressionKind::Binary {
                    op: BinaryExprOp::FunImage,
                    left: inner_set,
                    right: e,
                },
            ) = (left.kind(), right.kind())
                && let ExpressionKind::SetExtension(inner) = inner_set.kind()
            {
                // The reference `return`s from the whole binary
                // rewrite as soon as this shape matches, so a
                // failed inverse check decides the node: no later
                // functional-image rule may fire on it.
                if outer.len() != inner.len() {
                    return None;
                }
                let inverse =
                    outer
                        .iter()
                        .zip(inner)
                        .all(|(m1, m2)| match (m1.kind(), m2.kind()) {
                            (
                                ExpressionKind::Binary {
                                    op: BinaryExprOp::Mapsto,
                                    left: a1,
                                    right: b1,
                                },
                                ExpressionKind::Binary {
                                    op: BinaryExprOp::Mapsto,
                                    left: a2,
                                    right: b2,
                                },
                            ) => b1 == a2 && b2 == a1,
                            _ => false,
                        });
                return inverse.then(|| e.clone());
            }
            // SIMP_FUNIMAGE_CPROD: (S × {E})(x) == E
            if let ExpressionKind::Binary {
                op: BinaryExprOp::CProd,
                right: values,
                ..
            } = left.kind()
                && let ExpressionKind::SetExtension(members) = values.kind()
                && let [e] = members.as_slice()
            {
                return Some(e.clone());
            }
            // SIMP_FUNIMAGE_LAMBDA: solved by the lambda computer.
            if let Some(image) = lambda_computer(expr) {
                return Some(image);
            }
            // SIMP_FUNIMAGE_PRJ1 / PRJ2: prjN(E ↦ F)
            if let ExpressionKind::Binary {
                op: BinaryExprOp::Mapsto,
                left: e,
                right: f,
            } = right.kind()
            {
                if is_atomic(left, AtomicOp::KPrj1Gen) {
                    return Some(e.clone());
                }
                if is_atomic(left, AtomicOp::KPrj2Gen) {
                    return Some(f.clone());
                }
            }
            // SIMP_FUNIMAGE_ID: id(x) == x
            if is_atomic(left, AtomicOp::KIdGen) {
                return Some(right.clone());
            }
            // SIMP_MULTI_FUNIMAGE_SETENUM_LL: {A ↦ E, …, B ↦ E}(x) == E
            if let ExpressionKind::SetExtension(members) = left.kind() {
                let same_image = members.first().and_then(|m| match m.kind() {
                    ExpressionKind::Binary {
                        op: BinaryExprOp::Mapsto,
                        right: e,
                        ..
                    } => Some(e.clone()),
                    _ => None,
                });
                if let Some(image) = same_image {
                    let all_same = members.iter().all(|m| {
                        matches!(m.kind(),
                            ExpressionKind::Binary {
                                op: BinaryExprOp::Mapsto,
                                right: e,
                                ..
                            } if *e == image)
                    });
                    if all_same {
                        return Some(image);
                    }
                }
                // SIMP_MULTI_FUNIMAGE_SETENUM_LR: {…, x ↦ y, …}(x) == y
                for member in members {
                    if let ExpressionKind::Binary {
                        op: BinaryExprOp::Mapsto,
                        left: x,
                        right: y,
                    } = member.kind()
                        && x == right
                    {
                        return Some(y.clone());
                    }
                }
            }
            // SIMP_MULTI_FUNIMAGE_BUNION_SETENUM:
            // (r ∪ … ∪ {…, x ↦ y, …})(x) == y
            if let ExpressionKind::Associative {
                op: AssocExprOp::BUnion,
                children,
            } = left.kind()
            {
                for child in children {
                    if let ExpressionKind::SetExtension(members) = child.kind() {
                        for member in members {
                            if let ExpressionKind::Binary {
                                op: BinaryExprOp::Mapsto,
                                left: x,
                                right: y,
                            } = member.kind()
                                && x == right
                            {
                                return Some(y.clone());
                            }
                        }
                    }
                }
            }
            None
        }
        BinaryExprOp::RelImage => {
            // SIMP_SPECIAL_RELIMAGE_R: r[∅] == ∅
            // SIMP_SPECIAL_RELIMAGE_L: ∅[A] == ∅
            if is_atomic(right, AtomicOp::EmptySet) || is_atomic(left, AtomicOp::EmptySet) {
                return Some(typed_empty(expr));
            }
            // SIMP_TYPE_RELIMAGE: r[Ty] == ran(r)
            if right.is_type_expression() {
                return Some(ff.unary_expression(UnaryExprOp::KRan, left.clone(), None));
            }
            // SIMP_MULTI_RELIMAGE_DOM: r[dom(r)] == ran(r)
            if let Some(r) = dom_of(right)
                && &r == left
            {
                return Some(ff.unary_expression(UnaryExprOp::KRan, left.clone(), None));
            }
            // SIMP_RELIMAGE_ID: id[T] == T
            if is_atomic(left, AtomicOp::KIdGen) {
                return Some(right.clone());
            }
            // SIMP_MULTI_RELIMAGE_CPROD_SING: ({E}×S)[{E}] == S
            if let ExpressionKind::Binary {
                op: BinaryExprOp::CProd,
                left: dom_set,
                right: s,
            } = left.kind()
                && let (ExpressionKind::SetExtension(a), ExpressionKind::SetExtension(b)) =
                    (dom_set.kind(), right.kind())
                && let ([e1], [e2]) = (a.as_slice(), b.as_slice())
                && e1 == e2
            {
                return Some(s.clone());
            }
            // SIMP_MULTI_RELIMAGE_SING_MAPSTO: {E ↦ F}[{E}] == {F}
            if let (ExpressionKind::SetExtension(a), ExpressionKind::SetExtension(b)) =
                (left.kind(), right.kind())
                && let ([maplet], [e2]) = (a.as_slice(), b.as_slice())
                && let ExpressionKind::Binary {
                    op: BinaryExprOp::Mapsto,
                    left: e1,
                    right: f,
                } = maplet.kind()
                && e1 == e2
            {
                return Some(ff.set_extension(vec![f.clone()], None));
            }
            if let Some(conv_arg) = converse_of(left) {
                // SIMP_MULTI_RELIMAGE_CONVERSE_RANSUB: (r ⩥ S)∼[S] == ∅
                if let ExpressionKind::Binary {
                    op: BinaryExprOp::RanSub,
                    right: s,
                    ..
                } = conv_arg.kind()
                    && s == right
                {
                    return Some(typed_empty(expr));
                }
                // SIMP_MULTI_RELIMAGE_CONVERSE_RANRES:
                // (r ▷ S)∼[S] == r∼[S]
                if let ExpressionKind::Binary {
                    op: BinaryExprOp::RanRes,
                    left: r,
                    right: s,
                } = conv_arg.kind()
                    && s == right
                {
                    return Some(ff.binary_expression(
                        BinaryExprOp::RelImage,
                        ff.unary_expression(UnaryExprOp::Converse, r.clone(), None),
                        right.clone(),
                        None,
                    ));
                }
                // SIMP_RELIMAGE_CONVERSE_DOMSUB:
                // (S ⩤ r)∼[T] == r∼[T] ∖ S
                if let ExpressionKind::Binary {
                    op: BinaryExprOp::DomSub,
                    left: s,
                    right: r,
                } = conv_arg.kind()
                {
                    return Some(setminus(
                        &ff.binary_expression(
                            BinaryExprOp::RelImage,
                            ff.unary_expression(UnaryExprOp::Converse, r.clone(), None),
                            right.clone(),
                            None,
                        ),
                        s,
                    ));
                }
            }
            // SIMP_MULTI_RELIMAGE_DOMSUB: (S ⩤ r)[S] == ∅
            if let ExpressionKind::Binary {
                op: BinaryExprOp::DomSub,
                left: s,
                ..
            } = left.kind()
                && s == right
            {
                return Some(typed_empty(expr));
            }
            // SIMP_RELIMAGE_DOMRES_ID: (S ◁ id)[T] == S ∩ T
            if let Some(s) = domres_id(left) {
                return Some(binter2(&s, right));
            }
            // SIMP_RELIMAGE_DOMSUB_ID: (S ⩤ id)[T] == T ∖ S
            if let Some(s) = domsub_id(left) {
                return Some(setminus(right, &s));
            }
            None
        }
        BinaryExprOp::CProd => {
            // SIMP_SPECIAL_CPROD_R / _L (level 2)
            if is_atomic(right, AtomicOp::EmptySet) || is_atomic(left, AtomicOp::EmptySet) {
                return Some(typed_empty(expr));
            }
            None
        }
        BinaryExprOp::DomRes => {
            // SIMP_SPECIAL_DOMRES_L: ∅ ◁ r == ∅
            if is_atomic(left, AtomicOp::EmptySet) {
                return Some(typed_empty(expr));
            }
            // SIMP_SPECIAL_DOMRES_R: S ◁ ∅ == ∅
            if is_atomic(right, AtomicOp::EmptySet) {
                return Some(right.clone());
            }
            // SIMP_TYPE_DOMRES: Ty ◁ r == r
            if left.is_type_expression() {
                return Some(right.clone());
            }
            // SIMP_MULTI_DOMRES_DOM: dom(r) ◁ r == r
            if let Some(r) = dom_of(left)
                && &r == right
            {
                return Some(right.clone());
            }
            // SIMP_MULTI_DOMRES_RAN: ran(r) ◁ r∼ == r∼
            if let (Some(r), Some(c)) = (ran_of(left), converse_of(right))
                && r == c
            {
                return Some(right.clone());
            }
            // SIMP_DOMRES_DOMRES_ID: S ◁ (T ◁ id) == (S ∩ T) ◁ id
            if let Some(t) = domres_id(right) {
                return Some(ff.binary_expression(
                    BinaryExprOp::DomRes,
                    binter2(left, &t),
                    id_atom(),
                    None,
                ));
            }
            // SIMP_DOMRES_DOMSUB_ID: S ◁ (T ⩤ id) == (S ∖ T) ◁ id
            if let Some(t) = domsub_id(right) {
                return Some(ff.binary_expression(
                    BinaryExprOp::DomRes,
                    setminus(left, &t),
                    id_atom(),
                    None,
                ));
            }
            None
        }
        BinaryExprOp::RanRes => {
            // SIMP_SPECIAL_RANRES_R: r ▷ ∅ == ∅
            if is_atomic(right, AtomicOp::EmptySet) {
                return Some(typed_empty(expr));
            }
            // SIMP_SPECIAL_RANRES_L: ∅ ▷ S == ∅
            if is_atomic(left, AtomicOp::EmptySet) {
                return Some(left.clone());
            }
            // SIMP_TYPE_RANRES: r ▷ Ty == r
            if right.is_type_expression() {
                return Some(left.clone());
            }
            // SIMP_MULTI_RANRES_RAN: r ▷ ran(r) == r
            if let Some(r) = ran_of(right)
                && &r == left
            {
                return Some(left.clone());
            }
            // SIMP_MULTI_RANRES_DOM: r∼ ▷ dom(r) == r∼
            if let (Some(c), Some(r)) = (converse_of(left), dom_of(right))
                && c == r
            {
                return Some(left.clone());
            }
            // SIMP_RANRES_DOMRES_ID: (S ◁ id) ▷ T == (S ∩ T) ◁ id
            if let Some(s) = domres_id(left) {
                return Some(ff.binary_expression(
                    BinaryExprOp::DomRes,
                    binter2(&s, right),
                    id_atom(),
                    None,
                ));
            }
            // SIMP_RANRES_DOMSUB_ID: (S ⩤ id) ▷ T == (T ∖ S) ◁ id
            if let Some(s) = domsub_id(left) {
                return Some(ff.binary_expression(
                    BinaryExprOp::DomRes,
                    setminus(right, &s),
                    id_atom(),
                    None,
                ));
            }
            // SIMP_RANRES_ID: id ▷ S == S ◁ id
            if is_atomic(left, AtomicOp::KIdGen) {
                return Some(ff.binary_expression(
                    BinaryExprOp::DomRes,
                    right.clone(),
                    left.clone(),
                    None,
                ));
            }
            None
        }
        BinaryExprOp::DomSub => {
            // SIMP_SPECIAL_DOMSUB_L: ∅ ⩤ r == r
            if is_atomic(left, AtomicOp::EmptySet) {
                return Some(right.clone());
            }
            // SIMP_SPECIAL_DOMSUB_R: S ⩤ ∅ == ∅
            if is_atomic(right, AtomicOp::EmptySet) {
                return Some(right.clone());
            }
            // SIMP_TYPE_DOMSUB: Ty ⩤ r == ∅
            if left.is_type_expression() {
                return Some(typed_empty(expr));
            }
            // SIMP_MULTI_DOMSUB_DOM: dom(r) ⩤ r == ∅
            if let Some(r) = dom_of(left)
                && &r == right
            {
                return Some(typed_empty(expr));
            }
            // SIMP_DOMSUB_DOMRES_ID: S ⩤ (T ◁ id) == (T ∖ S) ◁ id
            if let Some(t) = domres_id(right) {
                return Some(ff.binary_expression(
                    BinaryExprOp::DomRes,
                    setminus(&t, left),
                    id_atom(),
                    None,
                ));
            }
            // SIMP_DOMSUB_DOMSUB_ID: S ⩤ (T ⩤ id) == (S ∪ T) ⩤ id
            if let Some(t) = domsub_id(right) {
                return Some(ff.binary_expression(
                    BinaryExprOp::DomSub,
                    ff.associative_expression(AssocExprOp::BUnion, vec![left.clone(), t], None),
                    id_atom(),
                    None,
                ));
            }
            // SIMP_MULTI_DOMSUB_RAN: ran(r) ⩤ r∼ == ∅
            if let (Some(r), Some(c)) = (ran_of(left), converse_of(right))
                && r == c
            {
                return Some(typed_empty(expr));
            }
            None
        }
        BinaryExprOp::RanSub => {
            // SIMP_SPECIAL_RANSUB_R: r ⩥ ∅ == r
            if is_atomic(right, AtomicOp::EmptySet) {
                return Some(left.clone());
            }
            // SIMP_SPECIAL_RANSUB_L: ∅ ⩥ S == ∅
            if is_atomic(left, AtomicOp::EmptySet) {
                return Some(left.clone());
            }
            // SIMP_TYPE_RANSUB: r ⩥ Ty == ∅
            if right.is_type_expression() {
                return Some(typed_empty(expr));
            }
            // SIMP_MULTI_RANSUB_RAN: r ⩥ ran(r) == ∅
            if let Some(r) = ran_of(right)
                && &r == left
            {
                return Some(typed_empty(expr));
            }
            // SIMP_RANSUB_DOMRES_ID: (S ◁ id) ⩥ T == (S ∖ T) ◁ id
            if let Some(s) = domres_id(left) {
                return Some(ff.binary_expression(
                    BinaryExprOp::DomRes,
                    setminus(&s, right),
                    id_atom(),
                    None,
                ));
            }
            // SIMP_RANSUB_DOMSUB_ID: (S ⩤ id) ⩥ T == (S ∪ T) ⩤ id
            if let Some(s) = domsub_id(left) {
                return Some(ff.binary_expression(
                    BinaryExprOp::DomSub,
                    ff.associative_expression(AssocExprOp::BUnion, vec![s, right.clone()], None),
                    id_atom(),
                    None,
                ));
            }
            // SIMP_RANSUB_ID: id ⩥ S == S ⩤ id
            if is_atomic(left, AtomicOp::KIdGen) {
                return Some(ff.binary_expression(
                    BinaryExprOp::DomSub,
                    right.clone(),
                    left.clone(),
                    None,
                ));
            }
            // SIMP_MULTI_RANSUB_DOM: r∼ ⩥ dom(r) == ∅
            if let (Some(c), Some(r)) = (converse_of(left), dom_of(right))
                && c == r
            {
                return Some(typed_empty(expr));
            }
            None
        }
        BinaryExprOp::DProd => {
            // SIMP_SPECIAL_DPROD_R / _L
            if is_atomic(right, AtomicOp::EmptySet) || is_atomic(left, AtomicOp::EmptySet) {
                return Some(typed_empty(expr));
            }
            // SIMP_DPROD_CPROD: (S × T) ⊗ (U × V) == (S ∩ U) × (T × V)
            if let (
                ExpressionKind::Binary {
                    op: BinaryExprOp::CProd,
                    left: s,
                    right: t,
                },
                ExpressionKind::Binary {
                    op: BinaryExprOp::CProd,
                    left: u,
                    right: v,
                },
            ) = (left.kind(), right.kind())
            {
                return Some(ff.binary_expression(
                    BinaryExprOp::CProd,
                    binter2(s, u),
                    ff.binary_expression(BinaryExprOp::CProd, t.clone(), v.clone(), None),
                    None,
                ));
            }
            None
        }
        BinaryExprOp::PProd => {
            // SIMP_SPECIAL_PPROD_R / _L
            if is_atomic(right, AtomicOp::EmptySet) || is_atomic(left, AtomicOp::EmptySet) {
                return Some(typed_empty(expr));
            }
            // SIMP_PPROD_CPROD: (S × T) ∥ (U × V) == (S × U) × (T × V)
            if let (
                ExpressionKind::Binary {
                    op: BinaryExprOp::CProd,
                    left: s,
                    right: t,
                },
                ExpressionKind::Binary {
                    op: BinaryExprOp::CProd,
                    left: u,
                    right: v,
                },
            ) = (left.kind(), right.kind())
            {
                return Some(ff.binary_expression(
                    BinaryExprOp::CProd,
                    ff.binary_expression(BinaryExprOp::CProd, s.clone(), u.clone(), None),
                    ff.binary_expression(BinaryExprOp::CProd, t.clone(), v.clone(), None),
                    None,
                ));
            }
            None
        }
        BinaryExprOp::Rel
        | BinaryExprOp::TRel
        | BinaryExprOp::SRel
        | BinaryExprOp::STRel
        | BinaryExprOp::PFun
        | BinaryExprOp::TFun
        | BinaryExprOp::PInj
        | BinaryExprOp::TInj
        | BinaryExprOp::PSur
        | BinaryExprOp::TSur
        | BinaryExprOp::TBij => {
            let empty_l = is_atomic(left, AtomicOp::EmptySet);
            let empty_r = is_atomic(right, AtomicOp::EmptySet);
            let singleton_empty = || -> Option<Expression> {
                let Some(Type::Pow(base)) = expr.ty() else {
                    return None;
                };
                let empty = ff.atomic_expression(AtomicOp::EmptySet, None, Some((**base).clone()));
                Some(ff.set_extension(vec![empty], None))
            };
            // SIMP_SPECIAL_EQUAL_RELDOMRAN: ∅  ∅ / ∅ ↠ ∅ / ∅ ⤖ ∅ == {∅}
            if empty_l
                && empty_r
                && matches!(
                    op,
                    BinaryExprOp::STRel | BinaryExprOp::TSur | BinaryExprOp::TBij
                )
            {
                return singleton_empty();
            }
            // SIMP_SPECIAL_REL_R: S op ∅ == {∅} for ↔, , ⇸, ⤔, ⤀
            if empty_r
                && matches!(
                    op,
                    BinaryExprOp::Rel
                        | BinaryExprOp::SRel
                        | BinaryExprOp::PFun
                        | BinaryExprOp::PInj
                        | BinaryExprOp::PSur
                )
            {
                return singleton_empty();
            }
            // SIMP_SPECIAL_REL_L: ∅ op S == {∅} for ↔, , ⇸, →, ⤔, ↣
            if empty_l
                && matches!(
                    op,
                    BinaryExprOp::Rel
                        | BinaryExprOp::TRel
                        | BinaryExprOp::PFun
                        | BinaryExprOp::TFun
                        | BinaryExprOp::PInj
                        | BinaryExprOp::TInj
                )
            {
                return singleton_empty();
            }
            None
        }
        BinaryExprOp::Mod => {
            // SIMP_SPECIAL_MOD_0: 0 mod E == 0
            if lit0(left) {
                return Some(left.clone());
            }
            // SIMP_SPECIAL_MOD_1: E mod 1 == 0
            if lit1(right) {
                return Some(ff.integer_literal(0, None));
            }
            // SIMP_MULTI_MOD: E mod E == 0
            if left == right {
                return Some(ff.integer_literal(0, None));
            }
            None
        }
        BinaryExprOp::UpTo => {
            // SIMP_LIT_UPTO: i‥j == ∅ when j < i (literals)
            if let (Some(i), Some(j)) = (super::as_literal(left), super::as_literal(right))
                && i > j
            {
                return Some(typed_empty(expr));
            }
            None
        }
        BinaryExprOp::Mapsto => {
            // SIMP_MAPSTO_PRJ1_PRJ2: prj1(E) ↦ prj2(E) == E (level 4)
            if let (
                ExpressionKind::Binary {
                    op: BinaryExprOp::FunImage,
                    left: p1,
                    right: e1,
                },
                ExpressionKind::Binary {
                    op: BinaryExprOp::FunImage,
                    left: p2,
                    right: e2,
                },
            ) = (left.kind(), right.kind())
                && is_atomic(p1, AtomicOp::KPrj1Gen)
                && is_atomic(p2, AtomicOp::KPrj2Gen)
                && e1 == e2
            {
                return Some(e1.clone());
            }
            None
        }
    }
}

/// A positive-literal test.
fn is_int_value(expr: &Expression, value: u32) -> bool {
    matches!(expr.kind(),
        ExpressionKind::IntegerLiteral(v) if *v == num_bigint::BigInt::from(value))
}

/// The typed empty set.
fn typed_empty(like: &Expression) -> Expression {
    like.factory()
        .atomic_expression(AtomicOp::EmptySet, None, like.ty().cloned())
}

/// The associative-expression rules.
fn rewrite_assoc_expr(expr: &Expression) -> Option<Expression> {
    use rossi::formula::tag::{AssocExprOp, BinaryExprOp};
    let ExpressionKind::Associative { op, children } = expr.kind() else {
        return None;
    };
    let ff = expr.factory();
    let simplified = match op {
        // SIMP_SPECIAL/TYPE/MULTI_BINTER
        AssocExprOp::BInter => simplify_set_assoc(
            expr,
            children,
            |c| c.is_type_expression(),
            |c| is_atomic(c, AtomicOp::EmptySet),
            || ty_base_expression(expr),
        ),
        // SIMP_SPECIAL/TYPE/MULTI_BUNION
        AssocExprOp::BUnion => simplify_set_assoc(
            expr,
            children,
            |c| is_atomic(c, AtomicOp::EmptySet),
            |c| c.is_type_expression(),
            || Some(typed_empty(expr)),
        ),
        // SIMP_SPECIAL_PLUS
        AssocExprOp::Plus => simplify_plus(expr, children),
        // SIMP_SPECIAL_PROD_*
        AssocExprOp::Mul => simplify_mult(expr, children),
        // SIMP_SPECIAL_FCOMP / SIMP_TYPE_FCOMP_ID / SIMP_FCOMP_ID and
        // the ∘ variants
        AssocExprOp::FComp | AssocExprOp::BComp => simplify_comp(expr, children),
        // SIMP_SPECIAL_OVERL / SIMP_TYPE_OVERL_CPROD / SIMP_MULTI_OVERL
        AssocExprOp::Ovr => simplify_ovr(expr, children),
    };
    if simplified.is_some() {
        return simplified;
    }
    // The pairwise rules fire only when the generic simplification
    // left the expression unchanged.
    match op {
        AssocExprOp::FComp => {
            if let [a, b] = children.as_slice() {
                let domres_id = |e: &Expression| -> Option<Expression> {
                    let ExpressionKind::Binary {
                        op: BinaryExprOp::DomRes,
                        left,
                        right,
                    } = e.kind()
                    else {
                        return None;
                    };
                    is_atomic(right, AtomicOp::KIdGen).then(|| left.clone())
                };
                // SIMP_FCOMP_ID_L: (S ◁ id) ; r == S ◁ r (level 2)
                if let Some(s) = domres_id(a) {
                    return Some(ff.binary_expression(BinaryExprOp::DomRes, s, b.clone(), None));
                }
                // SIMP_FCOMP_ID_R: r ; (S ◁ id) == r ▷ S (level 2)
                if let Some(s) = domres_id(b) {
                    return Some(ff.binary_expression(BinaryExprOp::RanRes, a.clone(), s, None));
                }
                // SIMP_TYPE_FCOMP_R: r ; (Ta × Tb) == dom(r) × Tb
                if let ExpressionKind::Binary {
                    op: BinaryExprOp::CProd,
                    right: tb,
                    ..
                } = b.kind()
                    && b.is_type_expression()
                {
                    return Some(ff.binary_expression(
                        BinaryExprOp::CProd,
                        ff.unary_expression(UnaryExprOp::KDom, a.clone(), None),
                        tb.clone(),
                        None,
                    ));
                }
                // SIMP_TYPE_FCOMP_L: (Ta × Tb) ; r == Ta × ran(r)
                if let ExpressionKind::Binary {
                    op: BinaryExprOp::CProd,
                    left: ta,
                    ..
                } = a.kind()
                    && a.is_type_expression()
                {
                    return Some(ff.binary_expression(
                        BinaryExprOp::CProd,
                        ta.clone(),
                        ff.unary_expression(UnaryExprOp::KRan, b.clone(), None),
                        None,
                    ));
                }
            }
            None
        }
        AssocExprOp::BComp => {
            if let [a, b] = children.as_slice() {
                // SIMP_TYPE_BCOMP_L: (Ta × Tb) ∘ r == dom(r) × Tb
                if let ExpressionKind::Binary {
                    op: BinaryExprOp::CProd,
                    right: tb,
                    ..
                } = a.kind()
                    && a.is_type_expression()
                {
                    return Some(ff.binary_expression(
                        BinaryExprOp::CProd,
                        ff.unary_expression(UnaryExprOp::KDom, b.clone(), None),
                        tb.clone(),
                        None,
                    ));
                }
                // SIMP_TYPE_BCOMP_R: r ∘ (Ta × Tb) == Ta × ran(r)
                if let ExpressionKind::Binary {
                    op: BinaryExprOp::CProd,
                    left: ta,
                    ..
                } = b.kind()
                    && b.is_type_expression()
                {
                    return Some(ff.binary_expression(
                        BinaryExprOp::CProd,
                        ta.clone(),
                        ff.unary_expression(UnaryExprOp::KRan, a.clone(), None),
                        None,
                    ));
                }
            }
            None
        }
        _ => None,
    }
}

/// Associative simplification for ∩ and ∪: neutral children drop,
/// a determinant child is the result, duplicates collapse.
fn simplify_set_assoc(
    expr: &Expression,
    children: &[Expression],
    is_neutral: impl Fn(&Expression) -> bool,
    is_determinant: impl Fn(&Expression) -> bool,
    neutral: impl Fn() -> Option<Expression>,
) -> Option<Expression> {
    let ExpressionKind::Associative { op, .. } = expr.kind() else {
        return None;
    };
    let mut out: Vec<Expression> = Vec::new();
    let mut changed = false;
    for child in children {
        if is_neutral(child) {
            changed = true;
        } else if is_determinant(child) {
            return Some(child.clone());
        } else if out.contains(child) {
            // Duplicate — dropped by the insertion-ordered set.
        } else {
            out.push(child.clone());
        }
    }
    match out.len() {
        0 => neutral(),
        1 => Some(out.into_iter().next().unwrap()),
        len if changed || len != children.len() => {
            Some(expr.factory().associative_expression(*op, out, None))
        }
        _ => None,
    }
}

/// Sum simplification: zeros drop (no dedup, no determinant).
fn simplify_plus(expr: &Expression, children: &[Expression]) -> Option<Expression> {
    let ff = expr.factory();
    let mut out: Vec<Expression> = Vec::new();
    let mut changed = false;
    for child in children {
        if is_int_value(child, 0) {
            changed = true;
        } else {
            out.push(child.clone());
        }
    }
    match out.len() {
        0 => Some(ff.integer_literal(0, None)),
        1 => Some(out.into_iter().next().unwrap()),
        len if changed || len != children.len() => {
            Some(ff.associative_expression(rossi::formula::tag::AssocExprOp::Plus, out, None))
        }
        _ => None,
    }
}

/// Product simplification: ones drop, a zero decides, signs of negated
/// factors accumulate. A negative literal is `−(lit)` here, which
/// takes the unary-minus path and lands on the same outcome as
/// a negative integer literal.
fn simplify_mult(expr: &Expression, children: &[Expression]) -> Option<Expression> {
    let ff = expr.factory();
    let mut out: Vec<Expression> = Vec::new();
    let mut changed = false;
    let mut positive = true;
    let mut zero: Option<Expression> = None;
    for child in children {
        let mut current = child.clone();
        // Strip sign layers first (−E, and a negative literal).
        while let ExpressionKind::Unary {
            op: UnaryExprOp::UnMinus,
            child: inner,
        } = current.kind()
        {
            positive = !positive;
            changed = true;
            current = inner.clone();
        }
        if is_int_value(&current, 1) {
            changed = true;
        } else if is_int_value(&current, 0) {
            zero = Some(current);
            break;
        } else {
            out.push(current);
        }
    }
    if let Some(zero) = zero {
        // The sign is irrelevant on a zero product.
        return Some(zero);
    }
    let unsigned = match out.len() {
        0 => ff.integer_literal(1, None),
        1 => out.into_iter().next().unwrap(),
        len if changed || len != children.len() => {
            ff.associative_expression(rossi::formula::tag::AssocExprOp::Mul, out, None)
        }
        _ => return None,
    };
    if positive {
        Some(unsigned)
    } else {
        Some(ff.unary_expression(UnaryExprOp::UnMinus, unsigned, None))
    }
}

/// Composition simplification for `;` and `∘`: identities drop, an empty
/// set decides, and runs of `S ◁ id` / `S ⩤ id` accumulate into one
/// restriction of the identity.
fn simplify_comp(expr: &Expression, children: &[Expression]) -> Option<Expression> {
    use rossi::formula::tag::{AssocExprOp, BinaryExprOp};
    let ExpressionKind::Associative { op, .. } = expr.kind() else {
        return None;
    };
    let ff = expr.factory();
    let mut out: Vec<Expression> = Vec::new();
    let mut changed = false;
    // The accumulator over id-restriction runs.
    let mut dom_res: Vec<Expression> = Vec::new();
    let mut dom_sub: Vec<Expression> = Vec::new();
    let mut run: Vec<Expression> = Vec::new();
    let assoc_or_single = |op: AssocExprOp, mut list: Vec<Expression>| -> Option<Expression> {
        match list.len() {
            0 => None,
            1 => Some(list.pop().unwrap()),
            _ => Some(ff.associative_expression(op, list, None)),
        }
    };
    let flush = |out: &mut Vec<Expression>,
                 dom_res: &mut Vec<Expression>,
                 dom_sub: &mut Vec<Expression>,
                 run: &mut Vec<Expression>,
                 changed: &mut bool| {
        if run.is_empty() {
            return;
        }
        if run.len() == 1 {
            out.push(run.pop().unwrap());
        } else {
            *changed = true;
            let id = match run[0].kind() {
                ExpressionKind::Binary { right, .. } => right.clone(),
                _ => unreachable!("accumulated id restrictions are binary"),
            };
            let restrictions = assoc_or_single(AssocExprOp::BInter, std::mem::take(dom_res));
            let subtractions = assoc_or_single(AssocExprOp::BUnion, std::mem::take(dom_sub));
            let result = match (restrictions, subtractions) {
                (Some(r), None) => ff.binary_expression(BinaryExprOp::DomRes, r, id, None),
                (None, Some(s)) => ff.binary_expression(BinaryExprOp::DomSub, s, id, None),
                (Some(r), Some(s)) => ff.binary_expression(
                    BinaryExprOp::DomRes,
                    ff.binary_expression(BinaryExprOp::SetMinus, r, s, None),
                    id,
                    None,
                ),
                (None, None) => unreachable!("a run holds at least one restriction"),
            };
            out.push(result);
        }
        dom_res.clear();
        dom_sub.clear();
        run.clear();
    };
    for child in children {
        if is_atomic(child, AtomicOp::KIdGen) {
            changed = true;
            continue;
        }
        if is_atomic(child, AtomicOp::EmptySet) {
            return Some(typed_empty(expr));
        }
        let accumulated = match child.kind() {
            ExpressionKind::Binary {
                op: BinaryExprOp::DomRes,
                left,
                right,
            } if is_atomic(right, AtomicOp::KIdGen) => {
                dom_res.push(left.clone());
                true
            }
            ExpressionKind::Binary {
                op: BinaryExprOp::DomSub,
                left,
                right,
            } if is_atomic(right, AtomicOp::KIdGen) => {
                dom_sub.push(left.clone());
                true
            }
            _ => false,
        };
        if accumulated {
            run.push(child.clone());
        } else {
            flush(&mut out, &mut dom_res, &mut dom_sub, &mut run, &mut changed);
            out.push(child.clone());
        }
    }
    flush(&mut out, &mut dom_res, &mut dom_sub, &mut run, &mut changed);
    match out.len() {
        0 => Some(ff.atomic_expression(AtomicOp::KIdGen, None, expr.ty().cloned())),
        1 => Some(out.into_iter().next().unwrap()),
        len if changed || len != children.len() => Some(ff.associative_expression(*op, out, None)),
        _ => None,
    }
}

/// Override simplification: empty sets drop, later duplicates win (the
/// traversal runs backwards), and a type expression cuts everything
/// overridden before it.
fn simplify_ovr(expr: &Expression, children: &[Expression]) -> Option<Expression> {
    use rossi::formula::tag::AssocExprOp;
    let ff = expr.factory();
    let mut kept_rev: Vec<Expression> = Vec::new();
    let mut changed = false;
    for child in children.iter().rev() {
        if is_atomic(child, AtomicOp::EmptySet) {
            changed = true;
            continue;
        }
        if kept_rev.contains(child) {
            // A later (already kept) occurrence overrides this one.
            continue;
        }
        kept_rev.push(child.clone());
        if child.is_type_expression() {
            // Everything to the left is overridden entirely.
            break;
        }
    }
    match kept_rev.len() {
        0 => Some(typed_empty(expr)),
        1 => Some(kept_rev.into_iter().next().unwrap()),
        len if changed || len != children.len() => {
            kept_rev.reverse();
            Some(ff.associative_expression(AssocExprOp::Ovr, kept_rev, None))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::driver::recursive_rewrite;
    use super::*;
    use crate::builder::ReplayHints;
    use crate::confidence::Confidence;
    use crate::hyp_action::HypAction;
    use crate::skeleton::{StoredInput, StoredRule};
    use crate::test_util::{desc, env, pred};
    use rossi::formula::SealedTypeEnvironment;

    /// The fixpoint of the L5 rewriter over one predicate.
    fn rewritten(env: &SealedTypeEnvironment, input: &str) -> Predicate {
        recursive_rewrite(&pred(env, input), &mut AutoRewriter).expect("a rewrite should fire")
    }

    fn assert_rewrites(env: &SealedTypeEnvironment, input: &str, expected: &str) {
        assert_eq!(rewritten(env, input), pred(env, expected), "on {input}");
    }

    #[test]
    fn relational_rules_chain_to_fixpoint() {
        let env = env(&[("x", "ℤ"), ("S", "ℙ(ℤ)")]);
        assert_rewrites(&env, "x = x", "⊤");
        // SIMP_IN_SING then the literal comparison.
        assert_rewrites(&env, "3 ∈ {5}", "⊥");
        // SIMP_IN_COMPSET_ONEPOINT through the driver.
        assert_rewrites(&env, "3 ∈ {y · y ∈ S ∣ y}", "3 ∈ S");
        // SIMP_NOT_LE
        assert_rewrites(&env, "¬ x ≤ 3", "x > 3");
    }

    #[test]
    fn simple_and_multiple_predicates_rewrite() {
        let env = env(&[("S", "ℙ(ℤ)"), ("T", "ℙ(ℤ)")]);
        // SIMP_FINITE_SETENUM
        assert_rewrites(&env, "finite({1, 2})", "⊤");
        // SIMP_SINGLE_PARTITION
        assert_rewrites(&env, "partition(S, T)", "S = T");
    }

    #[test]
    fn unary_expression_rules() {
        let env = env(&[
            ("x", "ℤ"),
            ("S", "ℙ(ℤ)"),
            ("T", "ℙ(ℤ)"),
            ("f", "ℙ(ℤ×ℤ)"),
            ("A", "ℙ(ℤ)"),
            ("r", "ℙ(ℤ×ℤ)"),
        ]);
        // SIMP_DOM_SETENUM with deduplication.
        assert_rewrites(&env, "dom({1 ↦ 2, 3 ↦ 4, 1 ↦ 5}) = S", "{1, 3} = S");
        // SIMP_CONVERSE_SETENUM
        assert_rewrites(&env, "{1 ↦ 2, 3 ↦ 4}∼ = r", "{2 ↦ 1, 4 ↦ 3} = r");
        // SIMP_CARD_POW
        assert_rewrites(&env, "card(ℙ(S)) = x", "2 ^ card(S) = x");
        // SIMP_CARD_BUNION (two children).
        assert_rewrites(
            &env,
            "card(S ∪ T) = x",
            "(card(S) + card(T)) − card(S ∩ T) = x",
        );
        // SIMP_LIT_MIN keeps the smallest literal in place.
        assert_rewrites(&env, "min({3, x, 5}) = x", "min({3, x}) = x");
        // SIMP_MIN_BUNION_SING
        assert_rewrites(&env, "min(S ∪ {min(T)}) = x", "min(S ∪ T) = x");
        // SIMP_MULTI_DOM_DOMRES
        assert_rewrites(&env, "dom(A ◁ f) = S", "dom(f) ∩ A = S");
        // SIMP_MINUS_MINUS (the unary minus of a literal survives).
        assert_rewrites(&env, "−(−x) = 3", "x = 3");
    }

    #[test]
    fn atomic_bool_setext_and_quantified_rules() {
        let env = env(&[("y", "BOOL"), ("S", "ℙ(ℤ)"), ("T", "ℙ(ℤ)"), ("r", "ℙ(ℤ×ℤ)")]);
        // DEF_PRED
        assert_rewrites(&env, "r = pred", "r = succ∼");
        // SIMP_SPECIAL_KBOOL_BTRUE
        assert_rewrites(&env, "y = bool(⊤)", "y = TRUE");
        // SIMP_MULTI_SETENUM
        assert_rewrites(&env, "{1, 2, 1} ⊆ S", "{1, 2} ⊆ S");
        // SIMP_SPECIAL_COMPSET_BFALSE
        assert_rewrites(&env, "S = {z · ⊥ ∣ z}", "S = ∅");
        // SIMP_COMPSET_IN
        assert_rewrites(&env, "T = {z · z ∈ S ∣ z}", "T = S");
    }

    #[test]
    fn card_equal_one_builds_the_singleton_existential() {
        let env = env(&[("S", "ℙ(ℤ)"), ("f", "ℙ(ℤ×ℤ)"), ("r", "ℙ(ℤ×ℤ)")]);
        // A product element type gets one declaration per component.
        assert_rewrites(&env, "card(f) = 1", "∃x,x0 · f = {x ↦ x0}");
        // The set's dangling references shift under the new binder.
        assert_rewrites(
            &env,
            "∀i · i ∈ S ⇒ card({t · i ↦ t ∈ r ∣ t}) = 1",
            "∀i · i ∈ S ⇒ (∃x · {t · i ↦ t ∈ r ∣ t} = {x})",
        );
    }

    #[test]
    fn funimage_lambda_computes_the_image() {
        let env = env(&[("x", "ℤ")]);
        // The lambda computer solves the maplet equations; the sum is
        // not folded (there is no literal-plus rule).
        assert_rewrites(
            &env,
            "{m,n · m ∈ ℕ ∧ n ∈ ℕ ∣ (m ↦ n) ↦ (m + n)}(2 ↦ 3) = x",
            "2 + 3 = x",
        );
    }

    fn stored(short: &str) -> StoredRule {
        StoredRule {
            rule: Rule {
                reasoner: desc(short),
                goal: None,
                needed_hyps: Vec::new(),
                confidence: Confidence::DISCHARGED_MAX,
                display: String::new(),
                antecedents: Vec::new(),
            },
            input: StoredInput::default(),
        }
    }

    #[test]
    fn reasoner_rewrites_hypotheses_and_goal() {
        let env = env(&[("x", "ℤ"), ("S", "ℙ(ℤ)")]);
        let hyps: Vec<Predicate> = vec![pred(&env, "x ∈ {3}")];
        let seq = crate::sequent::ProverSequent::new(
            env.clone(),
            hyps.clone(),
            [],
            hyps,
            pred(&env, "¬ x ≤ 2"),
        );
        let rule = AutoRewritesL5
            .replay(&seq, &stored("autoRewritesL5:0"), &ReplayHints::default())
            .unwrap();
        assert_eq!(rule.goal.as_ref(), Some(seq.goal()));
        let ante = &rule.antecedents[0];
        assert_eq!(ante.goal.as_ref(), Some(&pred(&env, "x > 2")));
        assert_eq!(
            ante.hyp_actions,
            vec![HypAction::Rewrite {
                hyps: vec![pred(&env, "x ∈ {3}")],
                added_idents: Vec::new(),
                inferred: vec![pred(&env, "x = 3")],
                disappearing: vec![pred(&env, "x ∈ {3}")],
            }]
        );
        assert!(rule.apply(&seq).is_some());
    }

    #[test]
    fn reasoner_fails_without_applicable_rewrites() {
        let env = env(&[("x", "ℤ")]);
        let hyps: Vec<Predicate> = vec![pred(&env, "x > 0")];
        let seq = crate::sequent::ProverSequent::new(
            env.clone(),
            hyps.clone(),
            [],
            hyps,
            pred(&env, "x > 1"),
        );
        let err = AutoRewritesL5
            .replay(&seq, &stored("autoRewritesL5:0"), &ReplayHints::default())
            .unwrap_err();
        assert_eq!(err, "No rewrites applicable");
    }
}
