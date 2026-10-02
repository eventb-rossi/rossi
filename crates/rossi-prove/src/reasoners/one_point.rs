//! The one-point processors: shared
//! machinery that finds `x = E` replacements for bound identifiers of
//! a quantified predicate and instantiates them. The rewriting variant
//! backs `SIMP_IN_COMPSET`/`SIMP_IN_COMPSET_ONEPOINT` in the auto
//! rewriter; the inference variant backs the lambda computer (and,
//! later, the `onePointRule` reasoner).

use rossi::formula::tag::{
    AssocPredOp, BinaryExprOp, BinaryPredOp, LiteralPredOp, QuantExprOp, QuantPredOp, RelationalOp,
};
use rossi::formula::{BoundIdentDecl, Expression, ExpressionKind, Predicate, PredicateKind};

/// A maplet equality broken
/// into its component equalities, leaf-first; any other predicate is
/// returned alone.
fn split_maplet_equality(pred: &Predicate) -> Vec<Predicate> {
    let PredicateKind::Relational {
        op: RelationalOp::Equal,
        left,
        right,
    } = pred.kind()
    else {
        return vec![pred.clone()];
    };
    let both_maplets = |l: &Expression, r: &Expression| {
        matches!(
            (l.kind(), r.kind()),
            (
                ExpressionKind::Binary {
                    op: BinaryExprOp::Mapsto,
                    ..
                },
                ExpressionKind::Binary {
                    op: BinaryExprOp::Mapsto,
                    ..
                },
            )
        )
    };
    if !both_maplets(left, right) {
        return vec![pred.clone()];
    }
    fn split(left: &Expression, right: &Expression, out: &mut Vec<Predicate>) {
        if let (
            ExpressionKind::Binary {
                op: BinaryExprOp::Mapsto,
                left: a,
                right: b,
            },
            ExpressionKind::Binary {
                op: BinaryExprOp::Mapsto,
                left: c,
                right: d,
            },
        ) = (left.kind(), right.kind())
        {
            split(a, c, out);
            split(b, d, out);
        } else {
            out.push(left.factory().relational_predicate(
                RelationalOp::Equal,
                left.clone(),
                right.clone(),
                None,
            ));
        }
    }
    let mut out = Vec::new();
    split(left, right, &mut out);
    out
}

/// Replacement matching, patterns in the reference order: an
/// equality naming a bound identifier and its replacement expression.
fn match_replacement(pred: &Predicate) -> Option<(u32, Expression)> {
    let PredicateKind::Relational {
        op: RelationalOp::Equal,
        left,
        right,
    } = pred.kind()
    else {
        return None;
    };
    match (left.kind(), right.kind()) {
        // Two bound identifiers: the smaller index is replaced.
        (ExpressionKind::BoundIdentifier(i), ExpressionKind::BoundIdentifier(j)) => {
            if i < j {
                Some((*i, right.clone()))
            } else {
                Some((*j, left.clone()))
            }
        }
        (_, ExpressionKind::BoundIdentifier(j)) => Some((*j, left.clone())),
        (ExpressionKind::BoundIdentifier(i), _) => Some((*i, right.clone())),
        _ => None,
    }
}

/// The replacement slots of one processor run, indexed by declaration
/// position.
struct Replacements {
    n: usize,
    slots: Vec<Option<Expression>>,
}

impl Replacements {
    fn new(n: usize) -> Replacements {
        Replacements {
            n,
            slots: vec![None; n],
        }
    }

    /// Record `pred` as a replacement if it names
    /// one of the binder's identifiers, the replacement expression
    /// only references more-outer identifiers, and the slot is free.
    fn check(&mut self, pred: &Predicate) -> bool {
        let Some((index, expr)) = match_replacement(pred) else {
            return false;
        };
        if index as usize >= self.n {
            return false;
        }
        if !expr
            .dangling_bound_indices()
            .iter()
            .all(|&other| other > index)
        {
            return false;
        }
        let position = self.n - 1 - index as usize;
        if self.slots[position].is_some() {
            return false;
        }
        self.slots[position] = Some(expr);
        true
    }

    fn available(&self) -> bool {
        self.slots.iter().any(Option::is_some)
    }
}

/// Instantiates the filled slots of `∀/∃ decls · body`, keeping the
/// declarations before the first replacement outside the
/// instantiation and re-merging afterwards.
/// `None` when a replacement expression references a declaration of
/// the inner segment — the index shift fails there, so no stored
/// proof can contain the result.
fn instantiate_partial(
    op: QuantPredOp,
    decls: &[BoundIdentDecl],
    body: &Predicate,
    slots: &[Option<Expression>],
) -> Option<Predicate> {
    let ff = body.factory();
    let first = slots
        .iter()
        .position(Option::is_some)
        .expect("at least one replacement");
    let (outer, inner) = decls.split_at(first);
    let inner_len = inner.len();
    let mut inner_slots: Vec<Option<Expression>> = Vec::with_capacity(slots.len() - first);
    for slot in &slots[first..] {
        let Some(expr) = slot else {
            inner_slots.push(None);
            continue;
        };
        if expr
            .dangling_bound_indices()
            .iter()
            .any(|&i| (i as usize) < inner_len)
        {
            return None;
        }
        inner_slots.push(Some(expr.shift_bound_identifiers(-(inner_len as i32))));
    }
    let inner_pred = ff.quantified_predicate(op, inner.to_vec(), body.clone(), None);
    let instantiated = inner_pred.instantiate(&inner_slots);
    if outer.is_empty() {
        return Some(instantiated);
    }
    // Any quantified result is re-merged under the original
    // operator, whatever quantifier the instantiation left on top.
    match instantiated.kind() {
        PredicateKind::Quantified {
            decls: kept, pred, ..
        } => {
            let mut all = outer.to_vec();
            all.extend(kept.iter().cloned());
            Some(ff.quantified_predicate(op, all, pred.clone(), None))
        }
        _ => Some(ff.quantified_predicate(op, outer.to_vec(), instantiated, None)),
    }
}

/// The rewriting variant behind `SIMP_IN_COMPSET` /
/// `SIMP_IN_COMPSET_ONEPOINT`: `E ∈ {x · P ∣ F}` becomes
/// `∃x · P ∧ F = E`, with every component equality naming a bound
/// identifier consumed as an instantiation. Always rewrites when the
/// shape matches.
pub(crate) fn rewrite_in_compset(pred: &Predicate) -> Option<Predicate> {
    let PredicateKind::Relational {
        op: RelationalOp::In,
        left: element,
        right: cset,
    } = pred.kind()
    else {
        return None;
    };
    let ExpressionKind::Quantified {
        op: QuantExprOp::CSet,
        decls,
        pred: guard,
        expr: value,
        ..
    } = cset.kind()
    else {
        return None;
    };
    let ff = pred.factory();
    let n = decls.len();
    let replacement = ff.relational_predicate(
        RelationalOp::Equal,
        value.clone(),
        element.shift_bound_identifiers(n as i32),
        None,
    );
    let mut predicates = vec![guard.clone()];
    let mut replacements = Replacements::new(n);
    for eq in split_maplet_equality(&replacement) {
        if !replacements.check(&eq) {
            predicates.push(eq);
        }
    }
    let body = if predicates.len() == 1 {
        guard.clone()
    } else {
        ff.associative_predicate(AssocPredOp::LAnd, predicates, None)
    };
    if replacements.available()
        && let Some(result) =
            instantiate_partial(QuantPredOp::Exists, decls, &body, &replacements.slots)
    {
        // SIMP_IN_COMPSET_ONEPOINT
        return Some(result);
    }
    // SIMP_IN_COMPSET
    Some(ff.quantified_predicate(QuantPredOp::Exists, decls.clone(), body, None))
}

/// The inference variant: applies the one-point rule
/// to a quantified predicate, consuming the first replacement
/// equality found at the right polarity. `None` when nothing applies.
pub(crate) fn one_point_inference(pred: &Predicate) -> Option<Predicate> {
    one_point_inference_with_replacement(pred).map(|(result, _)| result)
}

/// The one-point application together with the consumed replacement
/// expression, for the
/// reasoner's well-definedness antecedent.
pub(crate) fn one_point_inference_with_replacement(
    pred: &Predicate,
) -> Option<(Predicate, Expression)> {
    let PredicateKind::Quantified {
        op,
        decls,
        pred: body,
    } = pred.kind()
    else {
        return None;
    };
    let polarity = *op == QuantPredOp::Exists;
    let mut scan = InferenceScan {
        replacements: Replacements::new(decls.len()),
        found: false,
    };
    let processing = scan.match_and_simplify(body, polarity);
    if !scan.found {
        return None;
    }
    let ff = pred.factory();
    let processing = processing.unwrap_or_else(|| {
        ff.literal_predicate(
            if polarity {
                LiteralPredOp::BTrue
            } else {
                LiteralPredOp::BFalse
            },
            None,
        )
    });
    let replacement = scan
        .replacements
        .slots
        .iter()
        .flatten()
        .next()
        .expect("a found scan holds a replacement")
        .clone();
    let result = instantiate_partial(*op, decls, &processing, &scan.replacements.slots)?;
    Some((result, replacement))
}

/// Match-and-simplify: walks the body at the binder's polarity,
/// consumes the first valid replacement equality (`Some(None)` returns
/// `None` for a removed node) and stops looking once one is found.
struct InferenceScan {
    replacements: Replacements,
    found: bool,
}

impl InferenceScan {
    /// `None` = the node was consumed; otherwise the (possibly
    /// simplified) node.
    fn match_and_simplify(&mut self, pred: &Predicate, polarity: bool) -> Option<Predicate> {
        if self.found {
            return Some(pred.clone());
        }
        let ff = pred.factory();
        match pred.kind() {
            PredicateKind::Associative {
                op: op @ AssocPredOp::LAnd,
                children,
            } if polarity => self.process_associative(pred, children, polarity, *op),
            PredicateKind::Associative {
                op: op @ AssocPredOp::LOr,
                children,
            } if !polarity => self.process_associative(pred, children, polarity, *op),
            PredicateKind::Binary {
                op: BinaryPredOp::LImp,
                left,
                right,
            } if !polarity => {
                let left = self.match_and_simplify(left, !polarity);
                let right = self.match_and_simplify(right, polarity);
                match (left, right) {
                    (None, right) => right,
                    (Some(left), None) => Some(negate(&left)),
                    (Some(left), Some(right)) => {
                        Some(ff.binary_predicate(BinaryPredOp::LImp, left, right, None))
                    }
                }
            }
            PredicateKind::Not(child) => {
                let child = self.match_and_simplify(child, !polarity)?;
                Some(negate(&child))
            }
            PredicateKind::Relational {
                op: RelationalOp::Equal,
                left,
                right,
            } if polarity => {
                let is_maplet = matches!(
                    (left.kind(), right.kind()),
                    (
                        ExpressionKind::Binary {
                            op: BinaryExprOp::Mapsto,
                            ..
                        },
                        ExpressionKind::Binary {
                            op: BinaryExprOp::Mapsto,
                            ..
                        },
                    )
                );
                if is_maplet {
                    let conjuncts = split_maplet_equality(pred);
                    let land = ff.associative_predicate(AssocPredOp::LAnd, conjuncts, None);
                    self.match_and_simplify(&land, polarity)
                } else if self.replacements.check(pred) {
                    self.found = true;
                    None
                } else {
                    Some(pred.clone())
                }
            }
            _ => Some(pred.clone()),
        }
    }

    fn process_associative(
        &mut self,
        pred: &Predicate,
        children: &[Predicate],
        polarity: bool,
        op: AssocPredOp,
    ) -> Option<Predicate> {
        let new_children: Vec<Predicate> = children
            .iter()
            .filter_map(|child| self.match_and_simplify(child, polarity))
            .collect();
        match new_children.len() {
            0 => None,
            1 => Some(new_children.into_iter().next().unwrap()),
            _ => Some(pred.factory().associative_predicate(op, new_children, None)),
        }
    }
}

/// Unwraps a negation rather than stacking a second one.
fn negate(pred: &Predicate) -> Predicate {
    match pred.kind() {
        PredicateKind::Not(child) => child.clone(),
        _ => pred.factory().not_predicate(pred.clone(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{env, pred};

    #[test]
    fn in_compset_without_replacement_builds_the_quantified_form() {
        let env = env(&[("S", "ℙ(ℤ)")]);
        // No component equality names the bound identifier alone.
        let input = pred(&env, "3 ∈ {x · x ∈ S ∣ x∗2}");
        let result = rewrite_in_compset(&input).unwrap();
        assert_eq!(result, pred(&env, "∃x · x ∈ S ∧ x∗2 = 3"));
    }

    #[test]
    fn in_compset_with_replacement_instantiates() {
        let env = env(&[("S", "ℙ(ℤ)")]);
        let input = pred(&env, "3 ∈ {x · x ∈ S ∣ x}");
        let result = rewrite_in_compset(&input).unwrap();
        assert_eq!(result, pred(&env, "3 ∈ S"));
    }

    #[test]
    fn in_compset_splits_maplet_equalities() {
        let env = env(&[("a", "ℤ"), ("b", "ℤ")]);
        let input = pred(&env, "a ↦ b ∈ {x,y · x > y ∣ x ↦ y}");
        let result = rewrite_in_compset(&input).unwrap();
        assert_eq!(result, pred(&env, "a > b"));
    }

    #[test]
    fn in_compset_keeps_unconsumed_components_as_guards() {
        let env = env(&[("a", "ℤ"), ("b", "ℤ")]);
        // The x ↦ x pattern reuses the identifier: only the first
        // component is consumed, the second stays as a guard.
        let input = pred(&env, "a ↦ b ∈ {x · x > 0 ∣ x ↦ x}");
        let result = rewrite_in_compset(&input).unwrap();
        assert_eq!(result, pred(&env, "a > 0 ∧ a = b"));
    }

    #[test]
    fn inference_consumes_one_equality_under_exists() {
        let env = env(&[("S", "ℙ(ℤ)")]);
        let input = pred(&env, "∃x · x = 3 ∧ x ∈ S");
        let result = one_point_inference(&input).unwrap();
        assert_eq!(result, pred(&env, "3 ∈ S"));
    }

    #[test]
    fn inference_applies_at_negative_polarity_under_forall() {
        let env = env(&[("S", "ℙ(ℤ)")]);
        let input = pred(&env, "∀x · x = 3 ⇒ x ∈ S");
        let result = one_point_inference(&input).unwrap();
        assert_eq!(result, pred(&env, "3 ∈ S"));
    }

    #[test]
    fn inference_keeps_outer_declarations() {
        let env = env(&[("S", "ℙ(ℤ)")]);
        let input = pred(&env, "∃x,y · y = 3 ∧ x ∈ S ∧ y ∈ S");
        let result = one_point_inference(&input).unwrap();
        assert_eq!(result, pred(&env, "∃x · x ∈ S ∧ 3 ∈ S"));
    }

    #[test]
    fn inference_without_replacement_answers_none() {
        let env = env(&[("S", "ℙ(ℤ)")]);
        assert_eq!(one_point_inference(&pred(&env, "∃x · x ∈ S")), None);
        assert_eq!(one_point_inference(&pred(&env, "∀x · x = 3 ∧ x ∈ S")), None);
    }
}
