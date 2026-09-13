//! The manual rewriting reasoners: a stored position — and, for a
//! hypothesis rewrite, the hypothesis recovered from the recorded
//! rule — locate one subformula, which a
//! reasoner-specific rewrite replaces.

use std::str::FromStr;

use rossi::formula::position::Position;
use rossi::formula::tag::{AssocExprOp, AssocPredOp, LiteralPredOp, RelationalOp};
use rossi::formula::{Expression, ExpressionKind, Predicate, PredicateKind, Type};

use crate::builder::{Reasoner, ReplayHints};
use crate::confidence::Confidence;
use crate::hyp_action::HypAction;
use crate::rule::{Antecedent, Rule};
use crate::sequent::ProverSequent;
use crate::skeleton::StoredRule;

use super::{break_possible_conjunct, display_pred};

/// Input recovery: the position comes from
/// the `pos` input string; a rule with a goal is a goal rewrite,
/// otherwise the hypothesis is the first predicate of the first
/// hypothesis action.
fn stored_input(
    stored: &StoredRule,
    hints: &ReplayHints,
) -> Result<(Option<Predicate>, Position), String> {
    let pos = stored
        .input
        .strings
        .get("pos")
        .ok_or("Missing position input")?;
    let position = Position::from_str(pos).map_err(|_| format!("Bad position: {pos}"))?;
    if stored.rule.goal.is_some() {
        return Ok((None, position));
    }
    let antecedent = stored
        .rule
        .antecedents
        .first()
        .ok_or("Expected exactly one antecedent!")?;
    let action = antecedent
        .hyp_actions
        .first()
        .ok_or("Expected at least one hyp action!")?;
    let hyp = match action {
        HypAction::Rewrite { hyps, .. }
        | HypAction::ForwardInf { hyps, .. }
        | HypAction::Hide(hyps) => hyps.first(),
        _ => None,
    }
    .ok_or("Expected first hyp action to be a forward or hide hyp action!")?;
    Ok((Some(hints.apply_pred(hyp)), position))
}

/// `getNeededHyps`: the hypotheses justifying a rewrite of the given
/// predicate at the given position, or `None` for inapplicability.
type NeededHyps<'a> = &'a dyn Fn(&ProverSequent, &Predicate, &Position) -> Option<Vec<Predicate>>;

/// Rewrite the goal — one antecedent
/// per conjunct of the result — or one hypothesis, whose rewritten
/// conjuncts (minus `⊤`) are inferred and selected, the original
/// hidden.
pub(crate) fn manual_rewrite_rule(
    seq: &ProverSequent,
    stored: &StoredRule,
    hints: &ReplayHints,
    rewrite: &dyn Fn(&Predicate, &Position) -> Option<Predicate>,
    needed: NeededHyps<'_>,
    display: &dyn Fn(Option<&Predicate>, &Position) -> String,
) -> Result<Rule, String> {
    let (hyp, position) = stored_input(stored, hints)?;
    let reasoner_id = stored.rule.reasoner.id().to_string();
    match hyp {
        None => {
            let goal = seq.goal();
            let failure = || {
                format!(
                    "Rewriter {reasoner_id} is inapplicable for goal {} at position {position}",
                    display_pred(goal)
                )
            };
            let needed_hyps = needed(seq, goal, &position).ok_or_else(failure)?;
            let new_goal = rewrite(goal, &position).ok_or_else(failure)?;
            let antecedents = break_possible_conjunct(&new_goal)
                .into_iter()
                .map(|conjunct| Antecedent {
                    goal: Some(conjunct),
                    added_hyps: Vec::new(),
                    unselected_added: Vec::new(),
                    added_idents: Vec::new(),
                    hyp_actions: Vec::new(),
                })
                .collect();
            Ok(Rule {
                reasoner: stored.rule.reasoner.clone(),
                goal: Some(goal.clone()),
                needed_hyps,
                confidence: Confidence::DISCHARGED_MAX,
                display: display(None, &position),
                antecedents,
            })
        }
        Some(hyp) => {
            if !seq.contains_hypothesis(&hyp) {
                return Err(format!("Nonexistent hypothesis: {}", display_pred(&hyp)));
            }
            let failure = || {
                format!(
                    "Rewriter {reasoner_id} is inapplicable for hypothesis {} at position {position}",
                    display_pred(&hyp)
                )
            };
            let needed_hyps = needed(seq, &hyp, &position).ok_or_else(failure)?;
            let inferred_hyp = rewrite(&hyp, &position).ok_or_else(failure)?;
            let mut inferred = break_possible_conjunct(&inferred_hyp);
            inferred.retain(|pred| {
                !matches!(pred.kind(), PredicateKind::Literal(LiteralPredOp::BTrue))
            });
            let hyp_actions = if inferred.is_empty() {
                vec![HypAction::Hide(vec![hyp.clone()])]
            } else {
                vec![
                    HypAction::Rewrite {
                        hyps: vec![hyp.clone()],
                        added_idents: Vec::new(),
                        inferred: inferred.clone(),
                        disappearing: vec![hyp.clone()],
                    },
                    HypAction::Select(inferred),
                ]
            };
            Ok(Rule {
                reasoner: stored.rule.reasoner.clone(),
                goal: None,
                needed_hyps,
                confidence: Confidence::DISCHARGED_MAX,
                display: display(Some(&hyp), &position),
                antecedents: vec![Antecedent {
                    goal: None,
                    added_hyps: Vec::new(),
                    unselected_added: Vec::new(),
                    added_idents: Vec::new(),
                    hyp_actions,
                }],
            })
        }
    }
}

/// Membership that simplifies against literal
/// sets.
fn smart_in(member: &Expression, set: &Expression) -> Predicate {
    let ff = member.factory();
    match set.kind() {
        ExpressionKind::SetExtension(members) if members.len() == 1 => ff.relational_predicate(
            RelationalOp::Equal,
            member.clone(),
            members[0].clone(),
            None,
        ),
        ExpressionKind::Atomic(rossi::formula::tag::AtomicOp::EmptySet) => {
            ff.literal_predicate(LiteralPredOp::BFalse, None)
        }
        _ if set.is_type_expression() => ff.literal_predicate(LiteralPredOp::BTrue, None),
        _ => ff.relational_predicate(RelationalOp::In, member.clone(), set.clone(), None),
    }
}

/// Negation with double-negation unwrapping.
fn smart_not(pred: &Predicate) -> Predicate {
    match pred.kind() {
        PredicateKind::Not(child) => child.clone(),
        _ => pred.factory().not_predicate(pred.clone(), None),
    }
}

/// An associative node in the shape its print→parse round-trip
/// yields: a same-operator first child prints without parentheses,
/// so its whole left spine merges into the run, while later
/// same-operator children keep their parentheses and stay nested.
/// Stored rules only exist post-round-trip, so produced formulas
/// must take this shape.
fn assoc_as_parsed(op: AssocExprOp, mut children: Vec<Expression>) -> Expression {
    let ff = children[0].factory().clone();
    while let Some(first) = children.first().cloned() {
        let ExpressionKind::Associative {
            op: inner,
            children: nested,
        } = first.kind()
        else {
            break;
        };
        if *inner != op {
            break;
        }
        let mut merged = nested.clone();
        merged.extend(children.drain(1..));
        children = merged;
    }
    ff.associative_expression(op, children, None)
}

/// Disjointness: a singleton side becomes a negated
/// membership, otherwise the intersection is empty.
fn smart_disjoint(left: &Expression, right: &Expression) -> Predicate {
    let ff = left.factory();
    let singleton = |e: &Expression| match e.kind() {
        ExpressionKind::SetExtension(members) if members.len() == 1 => Some(members[0].clone()),
        _ => None,
    };
    if let Some(member) = singleton(left) {
        return smart_not(&smart_in(&member, right));
    }
    if let Some(member) = singleton(right) {
        return smart_not(&smart_in(&member, left));
    }
    let empty = ff.atomic_expression(
        rossi::formula::tag::AtomicOp::EmptySet,
        None,
        left.ty().cloned(),
    );
    let inter = assoc_as_parsed(AssocExprOp::BInter, vec![left.clone(), right.clone()]);
    ff.relational_predicate(RelationalOp::Equal, inter, empty, None)
}

/// Union: set extensions merge into one (first
/// occurrences kept), anything else is a plain union.
fn smart_union(ty: Option<&Type>, components: &[Expression]) -> Expression {
    let ff = components
        .first()
        .map(|c| c.factory().clone())
        .expect("caller handles the empty case");
    match components {
        [single] => single.clone(),
        _ => {
            let mut members: Vec<Expression> = Vec::new();
            for component in components {
                let ExpressionKind::SetExtension(list) = component.kind() else {
                    let _ = ty;
                    return assoc_as_parsed(AssocExprOp::BUnion, components.to_vec());
                };
                for member in list {
                    if !members.contains(member) {
                        members.push(member.clone());
                    }
                }
            }
            ff.set_extension(members, None)
        }
    }
}

/// Partition expansion: `partition(S, s1, …, sn)` becomes
/// `S = s1 ∪ … ∪ sn` plus pairwise disjointness.
fn expand_partition(pred: &Predicate) -> Option<Predicate> {
    let PredicateKind::Multiple(children) = pred.kind() else {
        return None;
    };
    let ff = pred.factory();
    let (set, components) = children.split_first()?;
    let union = if components.is_empty() {
        ff.atomic_expression(
            rossi::formula::tag::AtomicOp::EmptySet,
            None,
            set.ty().cloned(),
        )
    } else {
        smart_union(set.ty(), components)
    };
    let mut conjuncts =
        vec![ff.relational_predicate(RelationalOp::Equal, set.clone(), union, None)];
    for (i, left) in components.iter().enumerate() {
        for right in &components[i + 1..] {
            conjuncts.push(smart_disjoint(left, right));
        }
    }
    Some(match conjuncts.len() {
        1 => conjuncts.into_iter().next().unwrap(),
        _ => ff.associative_predicate(AssocPredOp::LAnd, conjuncts, None),
    })
}

/// `PartitionRewrites` (`partitionRewrites`) — expands one
/// `partition(…)` occurrence at the stored position.
pub struct PartitionRewrites;

impl Reasoner for PartitionRewrites {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        let rewrite = |pred: &Predicate, position: &Position| -> Option<Predicate> {
            let sub = pred.sub_formula(position)?;
            let rossi::formula::position::FormulaRef::Pred(sub) = sub else {
                return None;
            };
            let expanded = expand_partition(sub)?;
            super::rewrite_at(
                pred,
                position,
                rossi::formula::position::FormulaRef::Pred(&expanded),
            )
            .ok()
        };
        let display = |hyp: Option<&Predicate>, position: &Position| match hyp {
            None => "Partition rewrites in goal".to_string(),
            Some(hyp) => format!(
                "Partition rewrites in hyp ({})",
                hyp.sub_formula(position)
                    .and_then(|sub| match sub {
                        rossi::formula::position::FormulaRef::Pred(p) => Some(display_pred(p)),
                        _ => None,
                    })
                    .unwrap_or_default()
            ),
        };
        manual_rewrite_rule(
            seq,
            stored,
            hints,
            &rewrite,
            &|_, _, _| Some(Vec::new()),
            &display,
        )
    }
}

/// The function under a simplifiable restricted application:
/// `((A ⩤ f)(…)`, `(A ◁ f)(…)`, `(f ⩥ A)(…)`, `(f ▷ A)(…)`,
/// `(f ∖ A)(…)` — the simplifiable function image.
fn fun_img_function(pred: &Predicate, position: &Position) -> Option<Expression> {
    use rossi::formula::tag::BinaryExprOp;
    let rossi::formula::position::FormulaRef::Expr(sub) = pred.sub_formula(position)? else {
        return None;
    };
    let ExpressionKind::Binary {
        op: BinaryExprOp::FunImage,
        left,
        ..
    } = sub.kind()
    else {
        return None;
    };
    match left.kind() {
        ExpressionKind::Binary {
            op: BinaryExprOp::DomSub | BinaryExprOp::DomRes,
            right: fun,
            ..
        } => Some(fun.clone()),
        ExpressionKind::Binary {
            op: BinaryExprOp::RanSub | BinaryExprOp::RanRes | BinaryExprOp::SetMinus,
            left: fun,
            ..
        } => Some(fun.clone()),
        _ => None,
    }
}

/// Whether `hyp` types `fun` as a function of any kind —
/// the functional-predicate test.
fn is_fun_pred(hyp: &Predicate, fun: &Expression) -> bool {
    use rossi::formula::tag::BinaryExprOp;
    let PredicateKind::Relational {
        op: RelationalOp::In,
        left,
        right,
    } = hyp.kind()
    else {
        return false;
    };
    matches!(
        right.kind(),
        ExpressionKind::Binary {
            op: BinaryExprOp::PFun
                | BinaryExprOp::TFun
                | BinaryExprOp::PInj
                | BinaryExprOp::TInj
                | BinaryExprOp::PSur
                | BinaryExprOp::TSur
                | BinaryExprOp::TBij,
            ..
        }
    ) && left == fun
}

/// `FunImgSimplifies` (`funImgSimplifies`, version 0) — simplifies a
/// domain- or range-restricted function application to the bare
/// function, needing a hypothesis typing it as a function.
pub struct FunImgSimplifies;

impl Reasoner for FunImgSimplifies {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        let rewrite = |pred: &Predicate, position: &Position| -> Option<Predicate> {
            use rossi::formula::tag::BinaryExprOp;
            let fun = fun_img_function(pred, position)?;
            let rossi::formula::position::FormulaRef::Expr(sub) = pred.sub_formula(position)?
            else {
                return None;
            };
            let ExpressionKind::Binary {
                op: BinaryExprOp::FunImage,
                right: arg,
                ..
            } = sub.kind()
            else {
                return None;
            };
            let replacement =
                pred.factory()
                    .binary_expression(BinaryExprOp::FunImage, fun, arg.clone(), None);
            super::rewrite_at(
                pred,
                position,
                rossi::formula::position::FormulaRef::Expr(&replacement),
            )
            .ok()
        };
        let needed = |seq: &ProverSequent,
                      pred: &Predicate,
                      position: &Position|
         -> Option<Vec<Predicate>> {
            let fun = fun_img_function(pred, position)?;
            let hyp = seq.visible_hyp_iter().find(|hyp| is_fun_pred(hyp, &fun))?;
            Some(vec![hyp.clone()])
        };
        let display = |hyp: Option<&Predicate>, _: &Position| match hyp {
            None => "Functional image simplification in goal".to_string(),
            Some(_) => "Functional image simplification in hyp".to_string(),
        };
        manual_rewrite_rule(seq, stored, hints, &rewrite, &needed, &display)
    }
}

/// Total-domain rewriting (`totalDom`, version 2) — replaces `dom(f)` by
/// a substitute justified by a visible totality hypothesis
/// `f ∈ A <total arrow> B`.
pub struct TotalDom;

impl Reasoner for TotalDom {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        use rossi::formula::tag::BinaryExprOp;
        // The substitute expression from the `subst` input.
        let substitute = stored
            .input
            .exprs
            .get("subst")
            .and_then(|list| match list.as_slice() {
                [Some(expr)] => Some(expr.clone()),
                _ => None,
            })
            .ok_or("Expected exactly one substitute!")?;
        let (hyp, position) = stored_input(stored, hints)?;

        let to_rewrite = match &hyp {
            None => seq.goal(),
            Some(hyp) => {
                if !seq.contains_hypothesis(hyp) {
                    return Err(format!("Nonexistent hypothesis: {}", display_pred(hyp)));
                }
                hyp
            }
        };
        let failure = || {
            format!(
                "Rewriter {} is inapplicable for{} {} at position {position} with parameters {}",
                stored.rule.reasoner.id(),
                if hyp.is_none() {
                    " goal"
                } else {
                    " hypothesis"
                },
                display_pred(to_rewrite),
                super::display_expr(&substitute),
            )
        };

        // `TotalDomSubstitutions`: f ∈ A <total arrow> B gives dom(f) = A.
        let function = match pred_sub_expr(to_rewrite, &position) {
            Some(sub) => match sub.kind() {
                ExpressionKind::Unary {
                    op: rossi::formula::tag::UnaryExprOp::KDom,
                    child,
                } => child.clone(),
                _ => return Err(failure()),
            },
            None => return Err(failure()),
        };
        let mut needed_hyp = None;
        for candidate in seq.visible_hyp_iter() {
            let PredicateKind::Relational {
                op: RelationalOp::In,
                left,
                right,
            } = candidate.kind()
            else {
                continue;
            };
            let ExpressionKind::Binary {
                op:
                    BinaryExprOp::TFun
                    | BinaryExprOp::TInj
                    | BinaryExprOp::TSur
                    | BinaryExprOp::TBij
                    | BinaryExprOp::TRel
                    | BinaryExprOp::STRel,
                left: domain,
                ..
            } = right.kind()
            else {
                continue;
            };
            if left == &function && domain == &substitute {
                // A later hypothesis overwrites (map semantics).
                needed_hyp = Some(candidate.clone());
            }
        }
        let needed_hyp = needed_hyp.ok_or_else(failure)?;
        let rewritten = super::rewrite_at(
            to_rewrite,
            &position,
            rossi::formula::position::FormulaRef::Expr(&substitute),
        )
        .map_err(|_| failure())?;

        let display_sub = |hyp: &Predicate| {
            format!(
                "total function dom substitution in hyp ({})",
                pred_sub_expr(hyp, &position)
                    .map(|e| super::display_expr(&e))
                    .unwrap_or_default()
            )
        };
        match hyp {
            None => Ok(Rule {
                reasoner: stored.rule.reasoner.clone(),
                goal: Some(seq.goal().clone()),
                needed_hyps: vec![needed_hyp],
                confidence: Confidence::DISCHARGED_MAX,
                display: "total function dom substitution in goal".into(),
                antecedents: vec![Antecedent {
                    goal: Some(rewritten),
                    added_hyps: Vec::new(),
                    unselected_added: Vec::new(),
                    added_idents: Vec::new(),
                    hyp_actions: Vec::new(),
                }],
            }),
            Some(hyp) => Ok(Rule {
                reasoner: stored.rule.reasoner.clone(),
                goal: None,
                needed_hyps: vec![needed_hyp],
                confidence: Confidence::DISCHARGED_MAX,
                display: display_sub(&hyp),
                antecedents: vec![Antecedent {
                    goal: None,
                    added_hyps: Vec::new(),
                    unselected_added: Vec::new(),
                    added_idents: Vec::new(),
                    hyp_actions: vec![
                        HypAction::Rewrite {
                            hyps: vec![hyp.clone()],
                            added_idents: Vec::new(),
                            inferred: vec![rewritten.clone()],
                            disappearing: vec![hyp.clone()],
                        },
                        HypAction::Select(vec![rewritten]),
                    ],
                }],
            }),
        }
    }
}

/// The expression at `position` inside `pred`, if any.
fn pred_sub_expr(pred: &Predicate, position: &Position) -> Option<Expression> {
    match pred.sub_formula(position)? {
        rossi::formula::position::FormulaRef::Expr(e) => Some(e.clone()),
        _ => None,
    }
}

/// `RemoveNegation` (`rn`) — pushes one negation at the stored
/// position: the level-0 auto-rewriter rules first, then the
/// unfolding patterns (de Morgan, negated implication and
/// quantifiers, non-emptiness as an existential).
pub struct RemoveNegation;

impl Reasoner for RemoveNegation {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        use rossi::formula::tag::{AtomicOp, BinaryPredOp, QuantPredOp};
        let unfold = |sub: &Predicate| -> Option<Predicate> {
            // super.rewrite: the auto rewriter's negation rules (all
            // level 0).
            if let Some(new) = super::rewrites::simplify_predicate_node(sub)
                .or_else(|| super::auto_rewriter::rewrite_not(sub))
            {
                return Some(new);
            }
            let PredicateKind::Not(inner) = sub.kind() else {
                return None;
            };
            let ff = sub.factory();
            let neg = |p: &Predicate| ff.not_predicate(p.clone(), None);
            let is_empty_set =
                |e: &Expression| matches!(e.kind(), ExpressionKind::Atomic(AtomicOp::EmptySet));
            match inner.kind() {
                // SIMP_NOT_NOT (already covered by the simplifier).
                PredicateKind::Not(p) => Some(p.clone()),
                // DEF_SPECIAL_NOT_EQUAL: ¬ S = ∅ == ∃x·x ∈ S
                PredicateKind::Relational {
                    op: RelationalOp::Equal,
                    left,
                    right,
                } if is_empty_set(right) || is_empty_set(left) => {
                    let set = if is_empty_set(right) { left } else { right };
                    let (decls, member, shifted) = super::component_binder(set)?;
                    let membership =
                        ff.relational_predicate(RelationalOp::In, member, shifted, None);
                    Some(ff.quantified_predicate(QuantPredOp::Exists, decls, membership, None))
                }
                // DISTRI_NOT_AND / DISTRI_NOT_OR: de Morgan.
                PredicateKind::Associative { op, children } => {
                    let dual = match op {
                        AssocPredOp::LAnd => AssocPredOp::LOr,
                        AssocPredOp::LOr => AssocPredOp::LAnd,
                    };
                    Some(ff.associative_predicate(dual, children.iter().map(neg).collect(), None))
                }
                // DERIV_NOT_IMP: ¬(P ⇒ Q) == P ∧ ¬Q
                PredicateKind::Binary {
                    op: BinaryPredOp::LImp,
                    left,
                    right,
                } => Some(ff.associative_predicate(
                    AssocPredOp::LAnd,
                    vec![left.clone(), neg(right)],
                    None,
                )),
                // DERIV_NOT_FORALL / DERIV_NOT_EXISTS.
                PredicateKind::Quantified { op, decls, pred } => {
                    let dual = match op {
                        QuantPredOp::Forall => QuantPredOp::Exists,
                        QuantPredOp::Exists => QuantPredOp::Forall,
                    };
                    Some(ff.quantified_predicate(dual, decls.clone(), neg(pred), None))
                }
                _ => None,
            }
        };
        let rewrite = |pred: &Predicate, position: &Position| -> Option<Predicate> {
            let rossi::formula::position::FormulaRef::Pred(sub) = pred.sub_formula(position)?
            else {
                return None;
            };
            if !matches!(sub.kind(), PredicateKind::Not(_)) {
                return None;
            }
            let new_sub = unfold(sub)?;
            super::rewrite_at(
                pred,
                position,
                rossi::formula::position::FormulaRef::Pred(&new_sub),
            )
            .ok()
        };
        let display = |hyp: Option<&Predicate>, position: &Position| match hyp {
            None => "remove ¬ in goal".to_string(),
            Some(hyp) => format!(
                "remove ¬ in {}",
                hyp.sub_formula(position)
                    .and_then(|sub| match sub {
                        rossi::formula::position::FormulaRef::Pred(p) => Some(display_pred(p)),
                        _ => None,
                    })
                    .unwrap_or_default()
            ),
        };
        manual_rewrite_rule(
            seq,
            stored,
            hints,
            &rewrite,
            &|_, _, _| Some(Vec::new()),
            &display,
        )
    }
}

/// `RemoveMembershipL1` (`rmL1`) — unfolds one membership at the
/// stored position, after the level-1 auto-rewriter relational rules.
pub struct RemoveMembershipL1;

/// The membership unfoldings for `rmL1`, in the reference pattern
/// order.
fn unfold_membership(sub: &Predicate) -> Option<Predicate> {
    use rossi::formula::Type;
    use rossi::formula::tag::{
        AtomicOp, BinaryExprOp, BinaryPredOp, QuantExprOp, QuantPredOp, UnaryExprOp,
    };
    let PredicateKind::Relational {
        op: RelationalOp::In,
        left: element,
        right: set,
    } = sub.kind()
    else {
        return None;
    };
    let ff = sub.factory().clone();
    let land =
        |children: Vec<Predicate>| ff.associative_predicate(AssocPredOp::LAnd, children, None);
    let is_in = |e: &Expression, s: &Expression| {
        ff.relational_predicate(RelationalOp::In, e.clone(), s.clone(), None)
    };
    let maplet_sides = |e: &Expression| match e.kind() {
        ExpressionKind::Binary {
            op: BinaryExprOp::Mapsto,
            left,
            right,
        } => Some((left.clone(), right.clone())),
        _ => None,
    };
    let base_of = |e: &Expression| -> Option<Type> {
        match e.ty()? {
            Type::Pow(base) => Some((**base).clone()),
            _ => None,
        }
    };
    let source_of = |e: &Expression| -> Option<Type> {
        match base_of(e)? {
            Type::Prod(source, _) => Some((*source).clone()),
            _ => None,
        }
    };
    let target_of = |e: &Expression| -> Option<Type> {
        match base_of(e)? {
            Type::Prod(_, target) => Some((*target).clone()),
            _ => None,
        }
    };
    match set.kind() {
        // DEF_IN_SETENUM (with the SIMP_MULTI_IN and singleton
        // shortcuts folded into the unfolding itself).
        ExpressionKind::SetExtension(members) => {
            let mut equalities = Vec::with_capacity(members.len());
            for member in members {
                if member == element {
                    return Some(ff.literal_predicate(LiteralPredOp::BTrue, None));
                }
                equalities.push(ff.relational_predicate(
                    RelationalOp::Equal,
                    element.clone(),
                    member.clone(),
                    None,
                ));
            }
            Some(match equalities.len() {
                1 => equalities.into_iter().next().unwrap(),
                _ => ff.associative_predicate(AssocPredOp::LOr, equalities, None),
            })
        }
        // DEF_IN_MAPSTO: E ↦ F ∈ S × T == E ∈ S ∧ F ∈ T
        ExpressionKind::Binary {
            op: BinaryExprOp::CProd,
            left: s,
            right: t,
        } => {
            let (e, f) = maplet_sides(element)?;
            Some(land(vec![is_in(&e, s), is_in(&f, t)]))
        }
        // DEF_IN_POW: E ∈ ℙ(S) == E ⊆ S
        ExpressionKind::Unary {
            op: UnaryExprOp::Pow,
            child,
        } => Some(ff.relational_predicate(
            RelationalOp::SubsetEq,
            element.clone(),
            child.clone(),
            None,
        )),
        // DEF_IN_BUNION / DEF_IN_BINTER.
        ExpressionKind::Associative { op, children } => {
            let (dual, is_comp) = match op {
                AssocExprOp::BUnion => (AssocPredOp::LOr, false),
                AssocExprOp::BInter => (AssocPredOp::LAnd, false),
                AssocExprOp::FComp => (AssocPredOp::LAnd, true),
                _ => return None,
            };
            if is_comp {
                // DEF_IN_FCOMP: one binder per intermediate type.
                let (e, f) = maplet_sides(element)?;
                let mut decls = Vec::new();
                for rel in &children[..children.len() - 1] {
                    let (segment, _) = super::type_binder(&ff, &target_of(rel)?);
                    decls.extend(segment);
                }
                let max = decls.len() as u32;
                let mut size = max;
                let mut conjuncts = Vec::with_capacity(children.len());
                let mut prev = e.shift_bound_identifiers(max as i32);
                for rel in &children[..children.len() - 1] {
                    let target = target_of(rel)?;
                    let pattern = super::type_pattern(&ff, &target, size - 1);
                    let (segment, _) = super::type_binder(&ff, &target);
                    size -= segment.len() as u32;
                    let map =
                        ff.binary_expression(BinaryExprOp::Mapsto, prev, pattern.clone(), None);
                    conjuncts.push(ff.relational_predicate(
                        RelationalOp::In,
                        map,
                        rel.shift_bound_identifiers(max as i32),
                        None,
                    ));
                    prev = pattern;
                }
                let last = children.last().expect("a composition has children");
                let map = ff.binary_expression(
                    BinaryExprOp::Mapsto,
                    prev,
                    f.shift_bound_identifiers(max as i32),
                    None,
                );
                conjuncts.push(ff.relational_predicate(
                    RelationalOp::In,
                    map,
                    last.shift_bound_identifiers(max as i32),
                    None,
                ));
                return Some(ff.quantified_predicate(
                    QuantPredOp::Exists,
                    decls,
                    land(conjuncts),
                    None,
                ));
            }
            Some(ff.associative_predicate(
                dual,
                children.iter().map(|child| is_in(element, child)).collect(),
                None,
            ))
        }
        // DEF_IN_SETMINUS: E ∈ S ∖ T == E ∈ S ∧ ¬ E ∈ T
        ExpressionKind::Binary {
            op: BinaryExprOp::SetMinus,
            left: s,
            right: t,
        } => Some(land(vec![
            is_in(element, s),
            ff.not_predicate(is_in(element, t), None),
        ])),
        // DEF_IN_KUNION / DEF_IN_KINTER: a single binder `s`.
        ExpressionKind::Unary {
            op: op @ (UnaryExprOp::KUnion | UnaryExprOp::KInter),
            child,
        } => {
            let base = base_of(child)?;
            let decl = ff.bound_ident_decl("s", None, None, Some(base.clone()));
            let ident = ff.bound_identifier(0, None, Some(base));
            let p = is_in(&ident, &child.shift_bound_identifiers(1));
            let q = is_in(&element.shift_bound_identifiers(1), &ident);
            let body = if *op == UnaryExprOp::KUnion {
                land(vec![p, q])
            } else {
                ff.binary_predicate(BinaryPredOp::LImp, p, q, None)
            };
            let quant = if *op == UnaryExprOp::KUnion {
                QuantPredOp::Exists
            } else {
                QuantPredOp::Forall
            };
            Some(ff.quantified_predicate(quant, vec![decl], body, None))
        }
        // DEF_IN_QUNION / DEF_IN_QINTER.
        ExpressionKind::Quantified {
            op: op @ (QuantExprOp::QUnion | QuantExprOp::QInter),
            decls,
            pred: guard,
            expr: value,
            ..
        } => {
            let q = ff.relational_predicate(
                RelationalOp::In,
                element.shift_bound_identifiers(decls.len() as i32),
                value.clone(),
                None,
            );
            let (quant, body) = if *op == QuantExprOp::QUnion {
                (QuantPredOp::Exists, land(vec![guard.clone(), q]))
            } else {
                (
                    QuantPredOp::Forall,
                    ff.binary_predicate(BinaryPredOp::LImp, guard.clone(), q, None),
                )
            };
            Some(ff.quantified_predicate(quant, decls.clone(), body, None))
        }
        // DEF_IN_DOM / DEF_IN_RAN: bind the other side's components.
        ExpressionKind::Unary {
            op: op @ (UnaryExprOp::KDom | UnaryExprOp::KRan),
            child: r,
        } => {
            let bound_ty = if *op == UnaryExprOp::KDom {
                target_of(r)?
            } else {
                source_of(r)?
            };
            let (decls, pattern) = super::type_binder(&ff, &bound_ty);
            let n = decls.len() as i32;
            let shifted = element.shift_bound_identifiers(n);
            let map = if *op == UnaryExprOp::KDom {
                ff.binary_expression(BinaryExprOp::Mapsto, shifted, pattern, None)
            } else {
                ff.binary_expression(BinaryExprOp::Mapsto, pattern, shifted, None)
            };
            let body =
                ff.relational_predicate(RelationalOp::In, map, r.shift_bound_identifiers(n), None);
            Some(ff.quantified_predicate(QuantPredOp::Exists, decls, body, None))
        }
        // DEF_IN_CONVERSE: E ↦ F ∈ r∼ == F ↦ E ∈ r
        ExpressionKind::Unary {
            op: UnaryExprOp::Converse,
            child: r,
        } => {
            let (e, f) = maplet_sides(element)?;
            let map = ff.binary_expression(BinaryExprOp::Mapsto, f, e, None);
            Some(ff.relational_predicate(RelationalOp::In, map, r.clone(), None))
        }
        // DEF_IN_DOMRES / DEF_IN_DOMSUB.
        ExpressionKind::Binary {
            op: op @ (BinaryExprOp::DomRes | BinaryExprOp::DomSub),
            left: s,
            right: r,
        } => {
            let (e, f) = maplet_sides(element)?;
            let membership = if *op == BinaryExprOp::DomRes {
                is_in(&e, s)
            } else {
                ff.relational_predicate(RelationalOp::NotIn, e.clone(), s.clone(), None)
            };
            let map = ff.binary_expression(BinaryExprOp::Mapsto, e, f, None);
            let q = ff.relational_predicate(RelationalOp::In, map, r.clone(), None);
            Some(land(vec![membership, q]))
        }
        // DEF_IN_RANRES / DEF_IN_RANSUB.
        ExpressionKind::Binary {
            op: op @ (BinaryExprOp::RanRes | BinaryExprOp::RanSub),
            left: r,
            right: t,
        } => {
            let (e, f) = maplet_sides(element)?;
            let map = ff.binary_expression(BinaryExprOp::Mapsto, e, f.clone(), None);
            let p = ff.relational_predicate(RelationalOp::In, map, r.clone(), None);
            let membership = if *op == BinaryExprOp::RanRes {
                is_in(&f, t)
            } else {
                ff.relational_predicate(RelationalOp::NotIn, f.clone(), t.clone(), None)
            };
            Some(land(vec![p, membership]))
        }
        // DEF_IN_REL (level 1): r ∈ S ↔ T == r ⊆ S × T
        ExpressionKind::Binary {
            op: BinaryExprOp::Rel,
            left: s,
            right: t,
        } => Some(ff.relational_predicate(
            RelationalOp::SubsetEq,
            element.clone(),
            ff.binary_expression(BinaryExprOp::CProd, s.clone(), t.clone(), None),
            None,
        )),
        // DEF_IN_RELIMAGE: F ∈ r[S] == ∃x·x ∈ S ∧ x ↦ F ∈ r
        ExpressionKind::Binary {
            op: BinaryExprOp::RelImage,
            left: r,
            right: s,
        } => {
            let base = base_of(s)?;
            let (decls, pattern) = super::type_binder(&ff, &base);
            let n = decls.len() as i32;
            let p = is_in(&pattern, &s.shift_bound_identifiers(n));
            let map = ff.binary_expression(
                BinaryExprOp::Mapsto,
                pattern.clone(),
                element.shift_bound_identifiers(n),
                None,
            );
            let q =
                ff.relational_predicate(RelationalOp::In, map, r.shift_bound_identifiers(n), None);
            Some(ff.quantified_predicate(QuantPredOp::Exists, decls, land(vec![p, q]), None))
        }
        // DEF_IN_ID: E ↦ F ∈ id == E = F
        ExpressionKind::Atomic(AtomicOp::KIdGen) => {
            let (e, f) = maplet_sides(element)?;
            Some(ff.relational_predicate(RelationalOp::Equal, e, f, None))
        }
        // DEF_IN_RELDOM / DEF_IN_RELRAN / DEF_IN_RELDOMRAN.
        ExpressionKind::Binary {
            op: op @ (BinaryExprOp::TRel | BinaryExprOp::SRel | BinaryExprOp::STRel),
            left: s,
            right: t,
        } => {
            let rel = ff.binary_expression(BinaryExprOp::Rel, s.clone(), t.clone(), None);
            let mut conjuncts = vec![is_in(element, &rel)];
            if matches!(op, BinaryExprOp::TRel | BinaryExprOp::STRel) {
                let dom = ff.unary_expression(UnaryExprOp::KDom, element.clone(), None);
                conjuncts.push(ff.relational_predicate(RelationalOp::Equal, dom, s.clone(), None));
            }
            if matches!(op, BinaryExprOp::SRel | BinaryExprOp::STRel) {
                let ran = ff.unary_expression(UnaryExprOp::KRan, element.clone(), None);
                conjuncts.push(ff.relational_predicate(RelationalOp::Equal, ran, t.clone(), None));
            }
            Some(land(conjuncts))
        }
        // DEF_IN_FCT: f ∈ S ⇸ T == f ∈ S ↔ T ∧ functionality.
        ExpressionKind::Binary {
            op: BinaryExprOp::PFun,
            left: s,
            right: t,
        } => {
            let rel = ff.binary_expression(BinaryExprOp::Rel, s.clone(), t.clone(), None);
            let membership = is_in(element, &rel);
            let s_base = base_of(s)?;
            let t_base = base_of(t)?;
            let (x, _) = super::type_binder(&ff, &s_base);
            let (y, _) = super::type_binder(&ff, &t_base);
            let (z, _) = super::type_binder(&ff, &t_base);
            let length = (x.len() + y.len() + z.len()) as u32;
            let f = element.shift_bound_identifiers(length as i32);
            let x_pattern = super::type_pattern(&ff, &s_base, length - 1);
            let y_pattern = super::type_pattern(&ff, &t_base, (y.len() + z.len()) as u32 - 1);
            let z_pattern = super::type_pattern(&ff, &t_base, z.len() as u32 - 1);
            let map1 = ff.binary_expression(
                BinaryExprOp::Mapsto,
                x_pattern.clone(),
                y_pattern.clone(),
                None,
            );
            let map2 =
                ff.binary_expression(BinaryExprOp::Mapsto, x_pattern, z_pattern.clone(), None);
            let functional = ff.binary_predicate(
                BinaryPredOp::LImp,
                land(vec![
                    ff.relational_predicate(RelationalOp::In, map1, f.clone(), None),
                    ff.relational_predicate(RelationalOp::In, map2, f, None),
                ]),
                ff.relational_predicate(RelationalOp::Equal, y_pattern, z_pattern, None),
                None,
            );
            let mut decls = x;
            decls.extend(y);
            decls.extend(z);
            let forall = ff.quantified_predicate(QuantPredOp::Forall, decls, functional, None);
            Some(land(vec![membership, forall]))
        }
        // DEF_IN_TFCT / DEF_IN_INJ / DEF_IN_TINJ / DEF_IN_SURJ /
        // DEF_IN_TSURJ / DEF_IN_BIJ: one weaker arrow plus a side
        // condition.
        ExpressionKind::Binary {
            op:
                op @ (BinaryExprOp::TFun
                | BinaryExprOp::PInj
                | BinaryExprOp::TInj
                | BinaryExprOp::PSur
                | BinaryExprOp::TSur
                | BinaryExprOp::TBij),
            left: s,
            right: t,
        } => {
            let arrow = |weaker: BinaryExprOp, left: &Expression, right: &Expression| {
                ff.binary_expression(weaker, left.clone(), right.clone(), None)
            };
            let dom_eq = |side: &Expression| {
                let dom = ff.unary_expression(UnaryExprOp::KDom, element.clone(), None);
                ff.relational_predicate(RelationalOp::Equal, dom, side.clone(), None)
            };
            let ran_eq = |side: &Expression| {
                let ran = ff.unary_expression(UnaryExprOp::KRan, element.clone(), None);
                ff.relational_predicate(RelationalOp::Equal, ran, side.clone(), None)
            };
            Some(match op {
                BinaryExprOp::TFun => land(vec![
                    is_in(element, &arrow(BinaryExprOp::PFun, s, t)),
                    dom_eq(s),
                ]),
                BinaryExprOp::PInj => {
                    let inv = ff.unary_expression(UnaryExprOp::Converse, element.clone(), None);
                    land(vec![
                        is_in(element, &arrow(BinaryExprOp::PFun, s, t)),
                        ff.relational_predicate(
                            RelationalOp::In,
                            inv,
                            arrow(BinaryExprOp::PFun, t, s),
                            None,
                        ),
                    ])
                }
                BinaryExprOp::TInj => land(vec![
                    is_in(element, &arrow(BinaryExprOp::PInj, s, t)),
                    dom_eq(s),
                ]),
                BinaryExprOp::PSur => land(vec![
                    is_in(element, &arrow(BinaryExprOp::PFun, s, t)),
                    ran_eq(t),
                ]),
                BinaryExprOp::TSur => land(vec![
                    is_in(element, &arrow(BinaryExprOp::PSur, s, t)),
                    dom_eq(s),
                ]),
                BinaryExprOp::TBij => land(vec![
                    is_in(element, &arrow(BinaryExprOp::TInj, s, t)),
                    ran_eq(t),
                ]),
                _ => unreachable!("matched above"),
            })
        }
        // DEF_IN_DPROD: E ↦ (F ↦ G) ∈ p ⊗ q.
        ExpressionKind::Binary {
            op: BinaryExprOp::DProd,
            left: p,
            right: q,
        } => {
            let (e, fg) = maplet_sides(element)?;
            let (f, g) = maplet_sides(&fg)?;
            let map1 = ff.binary_expression(BinaryExprOp::Mapsto, e.clone(), f, None);
            let map2 = ff.binary_expression(BinaryExprOp::Mapsto, e, g, None);
            Some(land(vec![
                ff.relational_predicate(RelationalOp::In, map1, p.clone(), None),
                ff.relational_predicate(RelationalOp::In, map2, q.clone(), None),
            ]))
        }
        // DEF_IN_PPROD: (E ↦ G) ↦ (F ↦ H) ∈ p ∥ q.
        ExpressionKind::Binary {
            op: BinaryExprOp::PProd,
            left: p,
            right: q,
        } => {
            let (eg, fh) = maplet_sides(element)?;
            let (e, g) = maplet_sides(&eg)?;
            let (f, h) = maplet_sides(&fh)?;
            let map1 = ff.binary_expression(BinaryExprOp::Mapsto, e, f, None);
            let map2 = ff.binary_expression(BinaryExprOp::Mapsto, g, h, None);
            Some(land(vec![
                ff.relational_predicate(RelationalOp::In, map1, p.clone(), None),
                ff.relational_predicate(RelationalOp::In, map2, q.clone(), None),
            ]))
        }
        // DEF_IN_POW1: S ∈ ℙ1(T) == S ∈ ℙ(T) ∧ S ≠ ∅
        ExpressionKind::Unary {
            op: UnaryExprOp::Pow1,
            child,
        } => {
            let pow = ff.unary_expression(UnaryExprOp::Pow, child.clone(), None);
            let empty = ff.atomic_expression(AtomicOp::EmptySet, None, element.ty().cloned());
            Some(land(vec![
                is_in(element, &pow),
                ff.relational_predicate(RelationalOp::NotEqual, element.clone(), empty, None),
            ]))
        }
        // DEF_IN_UPTO: E ∈ a ‥ b == a ≤ E ∧ E ≤ b
        ExpressionKind::Binary {
            op: BinaryExprOp::UpTo,
            left: a,
            right: b,
        } => Some(land(vec![
            ff.relational_predicate(RelationalOp::Le, a.clone(), element.clone(), None),
            ff.relational_predicate(RelationalOp::Le, element.clone(), b.clone(), None),
        ])),
        // DEF_IN_NATURAL / DEF_IN_NATURAL1 (level 1).
        ExpressionKind::Atomic(AtomicOp::Natural) => Some(ff.relational_predicate(
            RelationalOp::Le,
            ff.integer_literal(0, None),
            element.clone(),
            None,
        )),
        ExpressionKind::Atomic(AtomicOp::Natural1) => Some(ff.relational_predicate(
            RelationalOp::Le,
            ff.integer_literal(1, None),
            element.clone(),
            None,
        )),
        _ => None,
    }
}

impl Reasoner for RemoveMembershipL1 {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        let rewrite = |pred: &Predicate, position: &Position| -> Option<Predicate> {
            let rossi::formula::position::FormulaRef::Pred(sub) = pred.sub_formula(position)?
            else {
                return None;
            };
            if !matches!(
                sub.kind(),
                PredicateKind::Relational {
                    op: RelationalOp::In,
                    ..
                }
            ) {
                return None;
            }
            // The auto-rewriter pre-step (the reference runs it at
            // level 1; these rules are the latest level, audited by
            // the corpus gate).
            let new_sub =
                super::auto_rewriter::rewrite_relational(sub).or_else(|| unfold_membership(sub))?;
            super::rewrite_at(
                pred,
                position,
                rossi::formula::position::FormulaRef::Pred(&new_sub),
            )
            .ok()
        };
        let display = |hyp: Option<&Predicate>, position: &Position| match hyp {
            None => "remove ∈ in goal".to_string(),
            Some(hyp) => format!(
                "remove ∈ in {}",
                hyp.sub_formula(position)
                    .and_then(|sub| match sub {
                        rossi::formula::position::FormulaRef::Pred(p) => Some(display_pred(p)),
                        _ => None,
                    })
                    .unwrap_or_default()
            ),
        };
        manual_rewrite_rule(
            seq,
            stored,
            hints,
            &rewrite,
            &|_, _, _| Some(Vec::new()),
            &display,
        )
    }
}

/// `RemoveInclusion` (`ri`) — unfolds one inclusion at the stored
/// position: the level-0 auto-rewriter relational rules first, then
/// `S ⊆ T` becomes a universally quantified membership implication
/// over the element type's components.
pub struct RemoveInclusion;

impl Reasoner for RemoveInclusion {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        use rossi::formula::tag::{BinaryPredOp, QuantPredOp};
        let rewrite = |pred: &Predicate, position: &Position| -> Option<Predicate> {
            let rossi::formula::position::FormulaRef::Pred(sub) = pred.sub_formula(position)?
            else {
                return None;
            };
            let PredicateKind::Relational {
                op: RelationalOp::SubsetEq,
                left: s,
                right: t,
            } = sub.kind()
            else {
                return None;
            };
            let ff = sub.factory();
            let new_sub = super::auto_rewriter::rewrite_relational(sub).or_else(|| {
                // DEF_SUBSETEQ over the element components.
                let base = match s.ty()? {
                    rossi::formula::Type::Pow(base) => (**base).clone(),
                    _ => return None,
                };
                let (decls, pattern) = super::type_binder(ff, &base);
                let n = decls.len() as i32;
                let p = ff.relational_predicate(
                    RelationalOp::In,
                    pattern.clone(),
                    s.shift_bound_identifiers(n),
                    None,
                );
                let q = ff.relational_predicate(
                    RelationalOp::In,
                    pattern,
                    t.shift_bound_identifiers(n),
                    None,
                );
                Some(ff.quantified_predicate(
                    QuantPredOp::Forall,
                    decls,
                    ff.binary_predicate(BinaryPredOp::LImp, p, q, None),
                    None,
                ))
            })?;
            super::rewrite_at(
                pred,
                position,
                rossi::formula::position::FormulaRef::Pred(&new_sub),
            )
            .ok()
        };
        let display = |hyp: Option<&Predicate>, position: &Position| match hyp {
            None => "remove ⊆ in goal".to_string(),
            Some(hyp) => format!(
                "remove ⊆ in {}",
                hyp.sub_formula(position)
                    .and_then(|sub| match sub {
                        rossi::formula::position::FormulaRef::Pred(p) => Some(display_pred(p)),
                        _ => None,
                    })
                    .unwrap_or_default()
            ),
        };
        manual_rewrite_rule(
            seq,
            stored,
            hints,
            &rewrite,
            &|_, _, _| Some(Vec::new()),
            &display,
        )
    }
}

/// `EqvRewrites` (`eqvRewrites`) — `P ⇔ Q` at the stored position
/// becomes `(P ⇒ Q) ∧ (Q ⇒ P)`.
pub struct EqvRewrites;

impl Reasoner for EqvRewrites {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        use rossi::formula::tag::BinaryPredOp;
        let rewrite = |pred: &Predicate, position: &Position| -> Option<Predicate> {
            let rossi::formula::position::FormulaRef::Pred(sub) = pred.sub_formula(position)?
            else {
                return None;
            };
            let PredicateKind::Binary {
                op: BinaryPredOp::LEqv,
                left,
                right,
            } = sub.kind()
            else {
                return None;
            };
            let ff = sub.factory();
            let new_sub = ff.associative_predicate(
                AssocPredOp::LAnd,
                vec![
                    ff.binary_predicate(BinaryPredOp::LImp, left.clone(), right.clone(), None),
                    ff.binary_predicate(BinaryPredOp::LImp, right.clone(), left.clone(), None),
                ],
                None,
            );
            super::rewrite_at(
                pred,
                position,
                rossi::formula::position::FormulaRef::Pred(&new_sub),
            )
            .ok()
        };
        let display = |hyp: Option<&Predicate>, position: &Position| match hyp {
            None => "rewrites equivalence in goal".to_string(),
            Some(hyp) => format!(
                "rewrites equivalence in hyp ({})",
                hyp.sub_formula(position)
                    .and_then(|sub| match sub {
                        rossi::formula::position::FormulaRef::Pred(p) => Some(display_pred(p)),
                        _ => None,
                    })
                    .unwrap_or_default()
            ),
        };
        manual_rewrite_rule(
            seq,
            stored,
            hints,
            &rewrite,
            &|_, _, _| Some(Vec::new()),
            &display,
        )
    }
}

/// Relational image over a right union (`relImgUnionRightRewrites`) —
/// `r[A ∪ … ∪ B]` at the stored position distributes into
/// `r[A] ∪ … ∪ r[B]`.
pub struct RelImgUnionRight;

impl Reasoner for RelImgUnionRight {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        use rossi::formula::tag::BinaryExprOp;
        let rewrite = |pred: &Predicate, position: &Position| -> Option<Predicate> {
            let rossi::formula::position::FormulaRef::Expr(sub) = pred.sub_formula(position)?
            else {
                return None;
            };
            let ExpressionKind::Binary {
                op: BinaryExprOp::RelImage,
                left: r,
                right: set,
            } = sub.kind()
            else {
                return None;
            };
            let ExpressionKind::Associative {
                op: AssocExprOp::BUnion,
                children,
            } = set.kind()
            else {
                return None;
            };
            let ff = sub.factory();
            let images: Vec<Expression> = children
                .iter()
                .map(|child| {
                    ff.binary_expression(BinaryExprOp::RelImage, r.clone(), child.clone(), None)
                })
                .collect();
            let new_sub = ff.associative_expression(AssocExprOp::BUnion, images, None);
            super::rewrite_at(
                pred,
                position,
                rossi::formula::position::FormulaRef::Expr(&new_sub),
            )
            .ok()
        };
        let display = |hyp: Option<&Predicate>, position: &Position| match hyp {
            None => "relational image with ∪ right in goal".to_string(),
            Some(hyp) => format!(
                "relational image with ∪ right in hyp ({})",
                hyp.sub_formula(position)
                    .and_then(|sub| match sub {
                        rossi::formula::position::FormulaRef::Expr(e) =>
                            Some(super::display_expr(e)),
                        _ => None,
                    })
                    .unwrap_or_default()
            ),
        };
        manual_rewrite_rule(
            seq,
            stored,
            hints,
            &rewrite,
            &|_, _, _| Some(Vec::new()),
            &display,
        )
    }
}

/// Disjunction to implication (`disjToImplRewrites`) — every
/// disjunction in the subformula at the stored position becomes
/// `¬d1 ⇒ d2 ∨ … ∨ dn` (one bottom-up pass of the driver).
pub struct DisjToImpl;

/// The `DEF_OR` hook.
struct DisjToImplHook;

impl super::driver::NodeRewriter for DisjToImplHook {
    fn predicate(&mut self, pred: &Predicate) -> Option<Predicate> {
        use rossi::formula::tag::BinaryPredOp;
        let PredicateKind::Associative {
            op: AssocPredOp::LOr,
            children,
        } = pred.kind()
        else {
            return None;
        };
        let ff = pred.factory();
        let (first, rest) = children.split_first().expect("a disjunction has children");
        let neg = match first.kind() {
            PredicateKind::Not(inner) => inner.clone(),
            _ => ff.not_predicate(first.clone(), None),
        };
        let rest = match rest {
            [single] => single.clone(),
            _ => ff.associative_predicate(AssocPredOp::LOr, rest.to_vec(), None),
        };
        Some(ff.binary_predicate(BinaryPredOp::LImp, neg, rest, None))
    }
}

impl Reasoner for DisjToImpl {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        let rewrite = |pred: &Predicate, position: &Position| -> Option<Predicate> {
            let rossi::formula::position::FormulaRef::Pred(sub) = pred.sub_formula(position)?
            else {
                return None;
            };
            if !matches!(sub.kind(), PredicateKind::Associative { .. }) {
                return None;
            }
            let new_sub = super::driver::rewrite_pred(sub, &mut DisjToImplHook)
                .unwrap_or_else(|| sub.clone());
            super::rewrite_at(
                pred,
                position,
                rossi::formula::position::FormulaRef::Pred(&new_sub),
            )
            .ok()
        };
        let display = |hyp: Option<&Predicate>, position: &Position| match hyp {
            None => "∨ to ⇒ in goal".to_string(),
            Some(hyp) => format!(
                "∨ to ⇒ in {}",
                hyp.sub_formula(position)
                    .and_then(|sub| match sub {
                        rossi::formula::position::FormulaRef::Pred(p) => Some(display_pred(p)),
                        _ => None,
                    })
                    .unwrap_or_default()
            ),
        };
        manual_rewrite_rule(
            seq,
            stored,
            hints,
            &rewrite,
            &|_, _, _| Some(Vec::new()),
            &display,
        )
    }
}

/// `FunSingletonImg` (`funSingletonImg`) — `r[{E}]` at the stored
/// position becomes `{r(E)}`, with a well-definedness antecedent.
pub struct FunSingletonImg;

impl Reasoner for FunSingletonImg {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        use rossi::formula::tag::BinaryExprOp;
        let (hyp, position) = position_input(stored, hints)?;
        let target = match &hyp {
            None => seq.goal().clone(),
            Some(hyp) => {
                if !seq.contains_hypothesis(hyp) {
                    return Err(format!(
                        "Inference {} is not applicable for {} at position {position}",
                        stored.rule.reasoner.id(),
                        display_pred(hyp),
                    ));
                }
                hyp.clone()
            }
        };
        let failure = || {
            format!(
                "Inference {} is not applicable for {} at position {position}",
                stored.rule.reasoner.id(),
                display_pred(&target),
            )
        };
        let sub = pred_sub_expr(&target, &position).ok_or_else(failure)?;
        let (r, e) = match sub.kind() {
            ExpressionKind::Binary {
                op: BinaryExprOp::RelImage,
                left: r,
                right: set,
            } => match set.kind() {
                ExpressionKind::SetExtension(members) => match members.as_slice() {
                    [single] => (r.clone(), single.clone()),
                    _ => return Err(failure()),
                },
                _ => return Err(failure()),
            },
            _ => return Err(failure()),
        };
        let ff = target.factory();
        let image = ff.binary_expression(BinaryExprOp::FunImage, r, e, None);
        let setext = ff.set_extension(vec![image], None);
        let inferred = super::rewrite_at(
            &target,
            &position,
            rossi::formula::position::FormulaRef::Expr(&setext),
        )
        .map_err(|_| failure())?;
        let wd_antecedent = Antecedent {
            goal: Some(inferred.wd_lemma()),
            added_hyps: Vec::new(),
            unselected_added: Vec::new(),
            added_idents: Vec::new(),
            hyp_actions: Vec::new(),
        };
        let main = match &hyp {
            None => Antecedent {
                goal: Some(inferred),
                added_hyps: Vec::new(),
                unselected_added: Vec::new(),
                added_idents: Vec::new(),
                hyp_actions: Vec::new(),
            },
            Some(hyp) => Antecedent {
                goal: None,
                added_hyps: vec![inferred],
                unselected_added: Vec::new(),
                added_idents: Vec::new(),
                hyp_actions: vec![HypAction::Hide(vec![hyp.clone()])],
            },
        };
        let display = match &hyp {
            None => "fun. singleton img. in goal".to_string(),
            Some(hyp) => format!(
                "fun. singleton img. in {}",
                pred_sub_expr(hyp, &position)
                    .map(|e| super::display_expr(&e))
                    .unwrap_or_default()
            ),
        };
        Ok(Rule {
            reasoner: stored.rule.reasoner.clone(),
            goal: hyp.is_none().then(|| seq.goal().clone()),
            needed_hyps: hyp.iter().cloned().collect(),
            confidence: Confidence::DISCHARGED_MAX,
            display,
            antecedents: vec![wd_antecedent, main],
        })
    }
}

/// Local equality rewriting (`locEq`) — replaces one occurrence of a free
/// identifier, at the stored position, by the other side of an
/// equality hypothesis.
pub struct LocalEq;

impl Reasoner for LocalEq {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        let pos = stored
            .input
            .strings
            .get("pos")
            .ok_or("Missing position input")?;
        let position = Position::from_str(pos).map_err(|_| format!("Bad position: {pos}"))?;
        // Input recovery: goal mode names the equality as the needed
        // hypothesis; hypothesis mode stores a rewrite action whose
        // two required hypotheses are the equality and the target,
        // told apart by re-deriving the recorded result.
        let (target_hyp, equality) = if stored.rule.goal.is_some() {
            match stored.rule.needed_hyps.as_slice() {
                [eq] => (None, eq.clone()),
                _ => return Err("There should be only one needed hypothesis".into()),
            }
        } else {
            let action = stored
                .rule
                .antecedents
                .first()
                .and_then(|antecedent| antecedent.hyp_actions.first())
                .ok_or("There should be at least one action")?;
            let (hyps, inferred) = match action {
                HypAction::Rewrite { hyps, inferred, .. }
                | HypAction::ForwardInf { hyps, inferred, .. } => (hyps, inferred),
                _ => return Err("First action shall be a forward inference".into()),
            };
            let ([h0, h1], [result]) = (hyps.as_slice(), inferred.as_slice()) else {
                return Err("There should be exactly two hypotheses in the inference".into());
            };
            let rewrites_to = |equality: &Predicate, target: &Predicate| -> bool {
                let Some(replacement) = equality_side(equality, target, &position) else {
                    return false;
                };
                // The stored result is round-trip normal, so the
                // candidate must be normalized the same way the rule
                // construction below normalizes its product.
                super::rewrite_at(
                    target,
                    &position,
                    rossi::formula::position::FormulaRef::Expr(&replacement),
                )
                .is_ok_and(|rewritten| &rewritten == result)
            };
            // Recovery runs on the stored predicates, as Rodin's `makeInput`
            // does; only the rewritten hypothesis is then renamed. That is
            // `AbstractManualRewrites.Input.applyHints`, which `LocalEqRewrite`
            // does not override: it renames `pred` and leaves `equality`
            // alone, so a rename reaching the equality fails here exactly as
            // it does in Rodin.
            if rewrites_to(h0, h1) {
                (Some(hints.apply_pred(h1)), h0.clone())
            } else if rewrites_to(h1, h0) {
                (Some(hints.apply_pred(h0)), h1.clone())
            } else {
                return Err("Cannot proceed re-writing with the given hypotheses".into());
            }
        };
        if let Some(hyp) = &target_hyp
            && !seq.contains_hypothesis(hyp)
        {
            return Err(format!(
                "{} is not a hypothesis of the given sequent",
                display_pred(hyp)
            ));
        }
        let target = target_hyp.clone().unwrap_or_else(|| seq.goal().clone());
        if !seq.contains_hypothesis(&equality) {
            return Err(format!(
                "{} is not a hypothesis of the given sequent",
                display_pred(&equality)
            ));
        }
        let replacement = equality_side(&equality, &target, &position).ok_or_else(|| {
            format!(
                "The {} cannot be re-written with the given input.",
                if target_hyp.is_none() {
                    "goal"
                } else {
                    "hypothesis"
                }
            )
        })?;
        let result = super::rewrite_at(
            &target,
            &position,
            rossi::formula::position::FormulaRef::Expr(&replacement),
        )
        .map_err(|_| "Input position out of range".to_string())?;
        match target_hyp {
            None => Ok(Rule {
                reasoner: stored.rule.reasoner.clone(),
                goal: Some(target),
                needed_hyps: vec![equality],
                confidence: Confidence::DISCHARGED_MAX,
                display: "lae in goal".into(),
                antecedents: vec![Antecedent {
                    goal: Some(result),
                    added_hyps: Vec::new(),
                    unselected_added: Vec::new(),
                    added_idents: Vec::new(),
                    hyp_actions: Vec::new(),
                }],
            }),
            Some(hyp) => Ok(Rule {
                reasoner: stored.rule.reasoner.clone(),
                goal: None,
                needed_hyps: Vec::new(),
                confidence: Confidence::DISCHARGED_MAX,
                display: format!("lae in {}", display_pred(&hyp)),
                antecedents: vec![Antecedent {
                    goal: None,
                    added_hyps: Vec::new(),
                    unselected_added: Vec::new(),
                    added_idents: Vec::new(),
                    hyp_actions: vec![HypAction::Rewrite {
                        hyps: vec![hyp.clone(), equality],
                        added_idents: Vec::new(),
                        inferred: vec![result],
                        disappearing: vec![hyp],
                    }],
                }],
            }),
        }
    }
}

/// The replacement for the identifier at `position` inside `target`,
/// when the equality names it on either side (the named side must be
/// a free identifier).
fn equality_side(
    equality: &Predicate,
    target: &Predicate,
    position: &Position,
) -> Option<Expression> {
    let PredicateKind::Relational {
        op: RelationalOp::Equal,
        left,
        right,
    } = equality.kind()
    else {
        return None;
    };
    let ident = pred_sub_expr(target, position)?;
    if !matches!(ident.kind(), ExpressionKind::FreeIdentifier(_)) {
        return None;
    }
    if matches!(left.kind(), ExpressionKind::FreeIdentifier(_)) && left == &ident {
        return Some(right.clone());
    }
    if matches!(right.kind(), ExpressionKind::FreeIdentifier(_)) && right == &ident {
        return Some(left.clone());
    }
    None
}

/// Position-input recovery: the position from
/// the `pos` string, the hypothesis (if any) from the needed
/// hypotheses.
fn position_input(
    stored: &StoredRule,
    hints: &ReplayHints,
) -> Result<(Option<Predicate>, Position), String> {
    let pos = stored
        .input
        .strings
        .get("pos")
        .ok_or("Missing position input")?;
    let position = Position::from_str(pos).map_err(|_| format!("Bad position: {pos}"))?;
    match stored.rule.needed_hyps.as_slice() {
        [] => Ok((None, position)),
        [hyp] => Ok((Some(hints.apply_pred(hyp)), position)),
        _ => Err("Expected exactly one needed hypothesis!".into()),
    }
}

/// `FunImageGoal` (`funImgGoal`) — adds `f(E) ∈ B` for a function
/// application at a WD-strict goal position, from a hypothesis
/// `f ∈ A <arrow> B`.
pub struct FunImageGoal;

impl Reasoner for FunImageGoal {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        use rossi::formula::tag::BinaryExprOp;
        let (hyp, position) = position_input(stored, hints)?;
        let goal = seq.goal();
        if !goal.is_wd_strict_at(&position) {
            return Err("Non WD-strict position in goal".into());
        }
        let fun_image = pred_sub_expr(goal, &position)
            .filter(|e| {
                matches!(
                    e.kind(),
                    ExpressionKind::Binary {
                        op: BinaryExprOp::FunImage,
                        ..
                    }
                )
            })
            .ok_or("Position does not denote a function application")?;
        let ExpressionKind::Binary { left: fun, .. } = fun_image.kind() else {
            unreachable!("a function application was matched");
        };
        let hyp = hyp
            .filter(|hyp| seq.contains_hypothesis(hyp))
            .ok_or("Missing hypothesis")?;
        let PredicateKind::Relational {
            op: RelationalOp::In,
            left,
            right: set,
        } = hyp.kind()
        else {
            return Err(format!("Ill-formed hypothesis {}", display_pred(&hyp)));
        };
        let is_fun_or_rel = matches!(
            set.kind(),
            ExpressionKind::Binary {
                op: BinaryExprOp::PFun
                    | BinaryExprOp::TFun
                    | BinaryExprOp::PInj
                    | BinaryExprOp::TInj
                    | BinaryExprOp::PSur
                    | BinaryExprOp::TSur
                    | BinaryExprOp::TBij
                    | BinaryExprOp::Rel
                    | BinaryExprOp::TRel
                    | BinaryExprOp::SRel
                    | BinaryExprOp::STRel,
                ..
            }
        );
        if left != fun || !is_fun_or_rel {
            return Err(format!("Ill-formed hypothesis {}", display_pred(&hyp)));
        }
        let ExpressionKind::Binary { right: range, .. } = set.kind() else {
            unreachable!("an arrow was matched");
        };
        let ff = goal.factory();
        let added =
            ff.relational_predicate(RelationalOp::In, fun_image.clone(), range.clone(), None);
        Ok(Rule {
            reasoner: stored.rule.reasoner.clone(),
            goal: Some(goal.clone()),
            needed_hyps: vec![hyp],
            confidence: Confidence::DISCHARGED_MAX,
            display: format!(
                "functional image goal for {}",
                super::display_expr(&fun_image)
            ),
            antecedents: vec![Antecedent {
                goal: Some(goal.clone()),
                added_hyps: vec![added],
                unselected_added: Vec::new(),
                added_idents: Vec::new(),
                hyp_actions: Vec::new(),
            }],
        })
    }
}

/// `FunOvr` (`funOvr`, version 1) — case-splits a function
/// application whose function is an override, at a WD-strict
/// position.
pub struct FunOvr;

impl Reasoner for FunOvr {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        use rossi::formula::tag::{BinaryExprOp, UnaryExprOp};
        let (hyp, position) = position_input(stored, hints)?;
        let target = match &hyp {
            None => seq.goal().clone(),
            Some(hyp) => {
                if !seq.contains_hypothesis(hyp) {
                    return Err(format!(
                        "Inference {} is not applicable for {} at position {position}",
                        stored.rule.reasoner.id(),
                        display_pred(hyp),
                    ));
                }
                hyp.clone()
            }
        };
        let failure = || {
            format!(
                "Inference {} is not applicable for {} at position {position}",
                stored.rule.reasoner.id(),
                display_pred(&target),
            )
        };
        if !target.is_wd_strict_at(&position) {
            return Err(failure());
        }
        let sub = pred_sub_expr(&target, &position).ok_or_else(failure)?;
        let ExpressionKind::Binary {
            op: BinaryExprOp::FunImage,
            left: ovr,
            right: arg,
        } = sub.kind()
        else {
            return Err(failure());
        };
        let ExpressionKind::Associative {
            op: AssocExprOp::Ovr,
            children,
        } = ovr.kind()
        else {
            return Err(failure());
        };
        let ff = target.factory();
        let last = children.last().expect("an override has children");
        let prefix = &children[..children.len() - 1];
        let rest = match prefix {
            [single] => single.clone(),
            _ => assoc_as_parsed(AssocExprOp::Ovr, prefix.to_vec()),
        };
        let rewrite_to = |replacement: &Expression| {
            super::rewrite_at(
                &target,
                &position,
                rossi::formula::position::FormulaRef::Expr(replacement),
            )
            .ok()
        };
        let rest_image = |dom_sub_by: Expression| {
            let restricted =
                ff.binary_expression(BinaryExprOp::DomSub, dom_sub_by, rest.clone(), None);
            ff.binary_expression(BinaryExprOp::FunImage, restricted, arg.clone(), None)
        };
        let singleton_maplet = match last.kind() {
            ExpressionKind::SetExtension(members) => match members.as_slice() {
                [single] => match single.kind() {
                    ExpressionKind::Binary {
                        op: BinaryExprOp::Mapsto,
                        left,
                        right,
                    } => Some((left.clone(), right.clone())),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        };
        let (case_hyp, first, second) = match &singleton_maplet {
            // g = {E ↦ F}: the argument equals E or it does not.
            Some((e, f)) => {
                let equal =
                    ff.relational_predicate(RelationalOp::Equal, arg.clone(), e.clone(), None);
                let set_e = ff.set_extension(vec![e.clone()], None);
                (
                    equal,
                    rewrite_to(f).ok_or_else(failure)?,
                    rewrite_to(&rest_image(set_e)).ok_or_else(failure)?,
                )
            }
            // Otherwise: the argument is in dom(g) or it is not.
            None => {
                let dom_g = ff.unary_expression(UnaryExprOp::KDom, last.clone(), None);
                let membership =
                    ff.relational_predicate(RelationalOp::In, arg.clone(), dom_g.clone(), None);
                let g_image =
                    ff.binary_expression(BinaryExprOp::FunImage, last.clone(), arg.clone(), None);
                (
                    membership,
                    rewrite_to(&g_image).ok_or_else(failure)?,
                    rewrite_to(&rest_image(dom_g)).ok_or_else(failure)?,
                )
            }
        };
        let negated = ff.not_predicate(case_hyp.clone(), None);
        let antecedent = |inferred: Predicate, new_hyp: Predicate| match &hyp {
            None => Antecedent {
                goal: Some(inferred),
                added_hyps: vec![new_hyp],
                unselected_added: Vec::new(),
                added_idents: Vec::new(),
                hyp_actions: Vec::new(),
            },
            Some(hyp) => Antecedent {
                goal: None,
                added_hyps: vec![new_hyp, inferred],
                unselected_added: Vec::new(),
                added_idents: Vec::new(),
                hyp_actions: vec![HypAction::Hide(vec![hyp.clone()])],
            },
        };
        let display = match &hyp {
            None => "ovr in goal".to_string(),
            Some(hyp) => format!(
                "ovr in {}",
                pred_sub_expr(hyp, &position)
                    .map(|e| super::display_expr(&e))
                    .unwrap_or_default()
            ),
        };
        Ok(Rule {
            reasoner: stored.rule.reasoner.clone(),
            goal: hyp.is_none().then(|| seq.goal().clone()),
            needed_hyps: hyp.iter().cloned().collect(),
            confidence: Confidence::DISCHARGED_MAX,
            display,
            antecedents: vec![antecedent(first, case_hyp), antecedent(second, negated)],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skeleton::StoredInput;
    use crate::test_util::{desc, env, pred};

    fn stored(goal: Option<Predicate>, hyp: Option<Predicate>, pos: &str) -> StoredRule {
        let antecedents = match &hyp {
            Some(hyp) => vec![Antecedent {
                goal: None,
                added_hyps: Vec::new(),
                unselected_added: Vec::new(),
                added_idents: Vec::new(),
                hyp_actions: vec![HypAction::Rewrite {
                    hyps: vec![hyp.clone()],
                    added_idents: Vec::new(),
                    inferred: Vec::new(),
                    disappearing: vec![hyp.clone()],
                }],
            }],
            None => Vec::new(),
        };
        let mut input = StoredInput::default();
        input.strings.insert("pos".into(), pos.into());
        StoredRule {
            rule: Rule {
                reasoner: desc("partitionRewrites"),
                goal,
                needed_hyps: Vec::new(),
                confidence: Confidence::DISCHARGED_MAX,
                display: String::new(),
                antecedents,
            },
            input,
        }
    }

    fn sequent(
        env: &rossi::formula::SealedTypeEnvironment,
        hyps: &[&str],
        goal: &str,
    ) -> ProverSequent {
        let hyps: Vec<Predicate> = hyps.iter().map(|s| pred(env, s)).collect();
        ProverSequent::new(env.clone(), hyps.clone(), [], hyps, pred(env, goal))
    }

    #[test]
    fn goal_partition_expands_into_one_antecedent_per_conjunct() {
        let env = env(&[("S", "ℙ(ℤ)"), ("A", "ℙ(ℤ)"), ("B", "ℙ(ℤ)")]);
        let seq = sequent(&env, &[], "partition(S, A, B)");
        let rule = PartitionRewrites
            .replay(
                &seq,
                &stored(Some(pred(&env, "partition(S, A, B)")), None, ""),
                &ReplayHints::default(),
            )
            .unwrap();
        assert_eq!(rule.goal.as_ref(), Some(seq.goal()));
        let goals: Vec<Option<Predicate>> =
            rule.antecedents.iter().map(|a| a.goal.clone()).collect();
        assert_eq!(
            goals,
            vec![Some(pred(&env, "S = A ∪ B")), Some(pred(&env, "A ∩ B = ∅")),]
        );
        assert!(rule.apply(&seq).is_some());
    }

    #[test]
    fn hypothesis_partition_with_singletons_uses_memberships() {
        let env = env(&[("S", "ℙ(ℤ)"), ("x", "ℤ"), ("y", "ℤ"), ("B", "ℙ(ℤ)")]);
        let hyp = pred(&env, "partition(S, {x}, {y}, B)");
        let seq = sequent(&env, &["partition(S, {x}, {y}, B)"], "⊥");
        let rule = PartitionRewrites
            .replay(
                &seq,
                &stored(None, Some(hyp.clone()), ""),
                &ReplayHints::default(),
            )
            .unwrap();
        assert_eq!(rule.goal, None);
        let expected = vec![
            pred(&env, "S = {x} ∪ {y} ∪ B"),
            pred(&env, "¬ x = y"),
            pred(&env, "¬ x ∈ B"),
            pred(&env, "¬ y ∈ B"),
        ];
        assert_eq!(
            rule.antecedents[0].hyp_actions,
            vec![
                HypAction::Rewrite {
                    hyps: vec![hyp.clone()],
                    added_idents: Vec::new(),
                    inferred: expected.clone(),
                    disappearing: vec![hyp.clone()],
                },
                HypAction::Select(expected),
            ]
        );
        assert!(rule.apply(&seq).is_some());
    }

    #[test]
    fn nested_partition_position_is_honoured() {
        let env = env(&[("S", "ℙ(ℤ)"), ("A", "ℙ(ℤ)")]);
        // partition at child 1 of the implication.
        let seq = sequent(&env, &[], "S = A ⇒ partition(S, A)");
        let rule = PartitionRewrites
            .replay(
                &seq,
                &stored(Some(seq.goal().clone()), None, "1"),
                &ReplayHints::default(),
            )
            .unwrap();
        assert_eq!(
            rule.antecedents[0].goal.as_ref(),
            Some(&pred(&env, "S = A ⇒ S = A"))
        );
    }

    #[test]
    fn fails_at_a_non_partition_position() {
        let env = env(&[("S", "ℙ(ℤ)"), ("A", "ℙ(ℤ)")]);
        let seq = sequent(&env, &[], "S = A");
        let err = PartitionRewrites
            .replay(
                &seq,
                &stored(Some(seq.goal().clone()), None, ""),
                &ReplayHints::default(),
            )
            .unwrap_err();
        assert!(err.contains("inapplicable for goal"), "{err}");
    }
}

#[cfg(test)]
mod batch30_tests {
    use super::*;
    use crate::skeleton::StoredInput;
    use crate::test_util::{desc, env, pred};

    fn sequent(
        env: &rossi::formula::SealedTypeEnvironment,
        hyps: &[&str],
        goal: &str,
    ) -> ProverSequent {
        let hyps: Vec<Predicate> = hyps.iter().map(|s| pred(env, s)).collect();
        ProverSequent::new(env.clone(), hyps.clone(), [], hyps, pred(env, goal))
    }

    fn expr_of(env: &rossi::formula::SealedTypeEnvironment, source: &str) -> Expression {
        match pred(env, &format!("{source} = {source}")).kind() {
            PredicateKind::Relational { left, .. } => left.clone(),
            _ => unreachable!("an equality was parsed"),
        }
    }

    fn stored_goal(short: &str, goal: Predicate, pos: &str) -> StoredRule {
        let mut input = StoredInput::default();
        input.strings.insert("pos".into(), pos.into());
        StoredRule {
            rule: Rule {
                reasoner: desc(short),
                goal: Some(goal),
                needed_hyps: Vec::new(),
                confidence: Confidence::DISCHARGED_MAX,
                display: String::new(),
                antecedents: Vec::new(),
            },
            input,
        }
    }

    #[test]
    fn total_dom_substitutes_in_the_goal() {
        let env = env(&[("f", "ℙ(ℤ×ℤ)"), ("A", "ℙ(ℤ)"), ("B", "ℙ(ℤ)"), ("x", "ℤ")]);
        let seq = sequent(&env, &["f ∈ A → B"], "x ∈ dom(f)");
        let mut stored = stored_goal("totalDom:2", seq.goal().clone(), "1");
        stored
            .input
            .exprs
            .insert("subst".into(), vec![Some(expr_of(&env, "A"))]);
        let rule = TotalDom
            .replay(&seq, &stored, &ReplayHints::default())
            .unwrap();
        assert_eq!(rule.goal.as_ref(), Some(seq.goal()));
        assert_eq!(rule.needed_hyps, vec![pred(&env, "f ∈ A → B")]);
        assert_eq!(
            rule.antecedents[0].goal.as_ref(),
            Some(&pred(&env, "x ∈ A"))
        );
        assert!(rule.apply(&seq).is_some());
    }

    #[test]
    fn total_dom_fails_without_a_totality_hypothesis() {
        let env = env(&[("f", "ℙ(ℤ×ℤ)"), ("A", "ℙ(ℤ)"), ("B", "ℙ(ℤ)"), ("x", "ℤ")]);
        let seq = sequent(&env, &["f ∈ A ⇸ B"], "x ∈ dom(f)");
        let mut stored = stored_goal("totalDom:2", seq.goal().clone(), "1");
        stored
            .input
            .exprs
            .insert("subst".into(), vec![Some(expr_of(&env, "A"))]);
        let err = TotalDom
            .replay(&seq, &stored, &ReplayHints::default())
            .unwrap_err();
        assert!(err.contains("is inapplicable for goal"), "{err}");
    }

    #[test]
    fn fun_image_goal_adds_the_range_membership() {
        let env = env(&[("f", "ℙ(ℤ×ℤ)"), ("A", "ℙ(ℤ)"), ("B", "ℙ(ℤ)"), ("x", "ℤ")]);
        let seq = sequent(&env, &["f ∈ A → B"], "f(x) > 0");
        let mut stored = stored_goal("funImgGoal", seq.goal().clone(), "0");
        stored.rule.needed_hyps = vec![pred(&env, "f ∈ A → B")];
        let rule = FunImageGoal
            .replay(&seq, &stored, &ReplayHints::default())
            .unwrap();
        assert_eq!(rule.needed_hyps, vec![pred(&env, "f ∈ A → B")]);
        let ante = &rule.antecedents[0];
        assert_eq!(ante.goal.as_ref(), Some(seq.goal()));
        assert_eq!(ante.added_hyps, vec![pred(&env, "f(x) ∈ B")]);
        assert!(rule.apply(&seq).is_some());
    }

    #[test]
    fn fun_ovr_splits_on_a_singleton_override() {
        let env = env(&[
            ("f", "ℙ(ℤ×ℤ)"),
            ("a", "ℤ"),
            ("b", "ℤ"),
            ("x", "ℤ"),
            ("y", "ℤ"),
        ]);
        let seq = sequent(&env, &[], "(f  {a ↦ b})(x) = y");
        let stored = stored_goal("funOvr:1", seq.goal().clone(), "0");
        let rule = FunOvr
            .replay(&seq, &stored, &ReplayHints::default())
            .unwrap();
        assert_eq!(rule.goal.as_ref(), Some(seq.goal()));
        assert_eq!(rule.antecedents.len(), 2);
        assert_eq!(
            rule.antecedents[0].goal.as_ref(),
            Some(&pred(&env, "b = y"))
        );
        assert_eq!(rule.antecedents[0].added_hyps, vec![pred(&env, "x = a")]);
        assert_eq!(
            rule.antecedents[1].goal.as_ref(),
            Some(&pred(&env, "({a} ⩤ f)(x) = y"))
        );
        assert_eq!(rule.antecedents[1].added_hyps, vec![pred(&env, "¬ x = a")]);
        assert!(rule.apply(&seq).is_some());
    }

    #[test]
    fn remove_negation_unfolds_de_morgan_and_non_emptiness() {
        let env = env(&[("S", "ℙ(ℤ)"), ("x", "ℤ"), ("y", "ℤ")]);
        // De Morgan at a nested goal position (child 1 of ⇒).
        let seq = sequent(&env, &[], "x > 0 ⇒ ¬(x > 1 ∧ y > 1)");
        let rule = RemoveNegation
            .replay(
                &seq,
                &stored_goal("rn", seq.goal().clone(), "1"),
                &ReplayHints::default(),
            )
            .unwrap();
        assert_eq!(
            rule.antecedents[0].goal.as_ref(),
            Some(&pred(&env, "x > 0 ⇒ (¬ x > 1 ∨ ¬ y > 1)"))
        );
        // Non-emptiness as an existential, at the root.
        let seq = sequent(&env, &[], "¬ S = ∅");
        let rule = RemoveNegation
            .replay(
                &seq,
                &stored_goal("rn", seq.goal().clone(), ""),
                &ReplayHints::default(),
            )
            .unwrap();
        assert_eq!(
            rule.antecedents[0].goal.as_ref(),
            Some(&pred(&env, "∃x0·x0 ∈ S"))
        );
    }

    #[test]
    fn remove_membership_unfolds_function_and_interval() {
        let env = env(&[("f", "ℙ(ℤ×ℤ)"), ("A", "ℙ(ℤ)"), ("B", "ℙ(ℤ)"), ("x", "ℤ")]);
        // Total function at the root.
        let seq = sequent(&env, &[], "f ∈ A → B");
        let rule = RemoveMembershipL1
            .replay(
                &seq,
                &stored_goal("rmL1", seq.goal().clone(), ""),
                &ReplayHints::default(),
            )
            .unwrap();
        let goals: Vec<Option<Predicate>> =
            rule.antecedents.iter().map(|a| a.goal.clone()).collect();
        assert_eq!(
            goals,
            vec![
                Some(pred(&env, "f ∈ A ⇸ B")),
                Some(pred(&env, "dom(f) = A")),
            ]
        );
        // Interval membership at a nested position.
        let seq = sequent(&env, &[], "x > 0 ⇒ x ∈ 1 ‥ 5");
        let rule = RemoveMembershipL1
            .replay(
                &seq,
                &stored_goal("rmL1", seq.goal().clone(), "1"),
                &ReplayHints::default(),
            )
            .unwrap();
        assert_eq!(
            rule.antecedents[0].goal.as_ref(),
            Some(&pred(&env, "x > 0 ⇒ 1 ≤ x ∧ x ≤ 5"))
        );
        assert!(rule.apply(&seq).is_some());
    }

    #[test]
    fn remove_inclusion_unfolds_over_components() {
        let env = env(&[("r", "ℙ(ℤ×ℤ)"), ("q", "ℙ(ℤ×ℤ)")]);
        let seq = sequent(&env, &[], "r ⊆ q");
        let rule = RemoveInclusion
            .replay(
                &seq,
                &stored_goal("ri", seq.goal().clone(), ""),
                &ReplayHints::default(),
            )
            .unwrap();
        assert_eq!(
            rule.antecedents[0].goal.as_ref(),
            Some(&pred(&env, "∀x,x0·x ↦ x0 ∈ r ⇒ x ↦ x0 ∈ q"))
        );
        assert!(rule.apply(&seq).is_some());
    }

    #[test]
    fn eqv_and_disj_rewrites() {
        let env = env(&[("a", "ℤ"), ("b", "ℤ"), ("c", "ℤ")]);
        let seq = sequent(&env, &[], "a > 0 ⇔ b > 0");
        let rule = EqvRewrites
            .replay(
                &seq,
                &stored_goal("eqvRewrites", seq.goal().clone(), ""),
                &ReplayHints::default(),
            )
            .unwrap();
        let goals: Vec<Option<Predicate>> =
            rule.antecedents.iter().map(|a| a.goal.clone()).collect();
        assert_eq!(
            goals,
            vec![
                Some(pred(&env, "a > 0 ⇒ b > 0")),
                Some(pred(&env, "b > 0 ⇒ a > 0")),
            ]
        );
        let seq = sequent(&env, &[], "a > 0 ∨ b > 0 ∨ c > 0");
        let rule = DisjToImpl
            .replay(
                &seq,
                &stored_goal("disjToImplRewrites", seq.goal().clone(), ""),
                &ReplayHints::default(),
            )
            .unwrap();
        assert_eq!(
            rule.antecedents[0].goal.as_ref(),
            Some(&pred(&env, "¬ a > 0 ⇒ b > 0 ∨ c > 0"))
        );
    }

    #[test]
    fn rel_img_union_right_distributes() {
        let env = env(&[("r", "ℙ(ℤ×ℤ)"), ("A", "ℙ(ℤ)"), ("B", "ℙ(ℤ)"), ("S", "ℙ(ℤ)")]);
        let seq = sequent(&env, &[], "r[A ∪ B] = S");
        let rule = RelImgUnionRight
            .replay(
                &seq,
                &stored_goal("relImgUnionRightRewrites", seq.goal().clone(), "0"),
                &ReplayHints::default(),
            )
            .unwrap();
        assert_eq!(
            rule.antecedents[0].goal.as_ref(),
            Some(&pred(&env, "r[A] ∪ r[B] = S"))
        );
    }

    #[test]
    fn fun_singleton_img_rewrites_with_wd() {
        let env = env(&[("r", "ℙ(ℤ×ℤ)"), ("x", "ℤ"), ("S", "ℙ(ℤ)")]);
        let seq = sequent(&env, &[], "r[{x}] = S");
        let rule = FunSingletonImg
            .replay(
                &seq,
                &stored_goal("funSingletonImg", seq.goal().clone(), "0"),
                &ReplayHints::default(),
            )
            .unwrap();
        assert_eq!(rule.antecedents.len(), 2);
        assert_eq!(
            rule.antecedents[1].goal.as_ref(),
            Some(&pred(&env, "{r(x)} = S"))
        );
        assert!(rule.antecedents[0].goal.is_some());
    }

    #[test]
    fn local_eq_rewrites_goal_and_hypothesis() {
        let env = env(&[("x", "ℤ"), ("y", "ℤ")]);
        // Goal mode: the equality is the needed hypothesis.
        let seq = sequent(&env, &["x = y + 1"], "x > 0");
        let mut stored = stored_goal("locEq", seq.goal().clone(), "0");
        stored.rule.needed_hyps = vec![pred(&env, "x = y + 1")];
        let rule = LocalEq
            .replay(&seq, &stored, &ReplayHints::default())
            .unwrap();
        assert_eq!(rule.needed_hyps, vec![pred(&env, "x = y + 1")]);
        assert_eq!(
            rule.antecedents[0].goal.as_ref(),
            Some(&pred(&env, "y + 1 > 0"))
        );
        // Hypothesis mode: recovered from the stored rewrite action.
        let target = pred(&env, "x > 0");
        let equality = pred(&env, "x = y + 1");
        let seq = sequent(&env, &["x = y + 1", "x > 0"], "⊥");
        let mut input = crate::skeleton::StoredInput::default();
        input.strings.insert("pos".into(), "0".into());
        let stored = StoredRule {
            rule: Rule {
                reasoner: desc("locEq"),
                goal: None,
                needed_hyps: Vec::new(),
                confidence: Confidence::DISCHARGED_MAX,
                display: String::new(),
                antecedents: vec![Antecedent {
                    goal: None,
                    added_hyps: Vec::new(),
                    unselected_added: Vec::new(),
                    added_idents: Vec::new(),
                    hyp_actions: vec![HypAction::Rewrite {
                        hyps: vec![equality.clone(), target.clone()],
                        added_idents: Vec::new(),
                        inferred: vec![pred(&env, "y + 1 > 0")],
                        disappearing: vec![target.clone()],
                    }],
                }],
            },
            input,
        };
        let rule = LocalEq
            .replay(&seq, &stored, &ReplayHints::default())
            .unwrap();
        assert_eq!(
            rule.antecedents[0].hyp_actions,
            vec![HypAction::Rewrite {
                hyps: vec![target.clone(), equality],
                added_idents: Vec::new(),
                inferred: vec![pred(&env, "y + 1 > 0")],
                disappearing: vec![target],
            }]
        );
    }

    #[test]
    fn fun_img_simplifies_strips_the_restriction() {
        let env = env(&[
            ("f", "ℙ(ℤ×ℤ)"),
            ("A", "ℙ(ℤ)"),
            ("B", "ℙ(ℤ)"),
            ("C", "ℙ(ℤ)"),
            ("x", "ℤ"),
            ("y", "ℤ"),
        ]);
        let seq = sequent(&env, &["f ∈ A ⇸ B"], "(C ◁ f)(x) = y");
        let stored = stored_goal("funImgSimplifies:0", seq.goal().clone(), "0");
        let rule = FunImgSimplifies
            .replay(&seq, &stored, &ReplayHints::default())
            .unwrap();
        assert_eq!(rule.needed_hyps, vec![pred(&env, "f ∈ A ⇸ B")]);
        assert_eq!(
            rule.antecedents[0].goal.as_ref(),
            Some(&pred(&env, "f(x) = y"))
        );
        assert!(rule.apply(&seq).is_some());
    }
}
