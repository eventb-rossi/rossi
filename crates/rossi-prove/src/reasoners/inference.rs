//! The input-bearing inference reasoners: quantifier instantiation,
//! lemma introduction, case analysis, implication elimination, and
//! equality rewriting.

use rossi::formula::tag::{
    AssocExprOp, AssocPredOp, BinaryPredOp, LiteralPredOp, QuantPredOp, RelationalOp,
};
use rossi::formula::{BoundIdentDecl, Expression, ExpressionKind, Predicate, PredicateKind};

use crate::builder::{Reasoner, ReplayHints};
use crate::confidence::Confidence;
use crate::hyp_action::HypAction;
use crate::rule::{Antecedent, Rule};
use crate::sequent::ProverSequent;
use crate::skeleton::StoredRule;

use super::driver::{self, NodeRewriter};
use super::{break_possible_conjunct, display_expr, display_pred, fresh_instantiation};

/// An antecedent with the given goal and nothing else.
fn goal_antecedent(goal: Option<Predicate>) -> Antecedent {
    Antecedent {
        goal,
        added_hyps: Vec::new(),
        unselected_added: Vec::new(),
        added_idents: Vec::new(),
        hyp_actions: Vec::new(),
    }
}

/// One predicate stored under the key `pred`.
fn single_pred_input(stored: &StoredRule, hints: &ReplayHints) -> Result<Predicate, String> {
    match stored.input.preds.get("pred").map(Vec::as_slice) {
        Some([Some(pred)]) => Ok(hints.apply_pred(pred)),
        _ => Err("Expected exactly one predicate".into()),
    }
}

/// Expressions stored under the key `exprs`,
/// holes preserved.
fn exprs_input(
    stored: &StoredRule,
    hints: &ReplayHints,
) -> Result<Vec<Option<Expression>>, String> {
    let exprs = stored
        .input
        .exprs
        .get("exprs")
        .ok_or("Expected expression input")?;
    Ok(exprs
        .iter()
        .map(|slot| slot.as_ref().map(|expr| hints.apply_expr(expr)))
        .collect())
}

/// Hypothesis-input recovery: the input hypothesis is the
/// stored rule's single needed hypothesis.
fn hypothesis_input(stored: &StoredRule, hints: &ReplayHints) -> Result<Predicate, String> {
    match stored.rule.needed_hyps.as_slice() {
        [hyp] => Ok(hints.apply_pred(hyp)),
        [] => Err("Null hypothesis".into()),
        _ => Err("Expected at most one needed hypothesis!".into()),
    }
}

/// Instantiation computation: expressions must match
/// the declarations' types exactly; holes stay holes.
fn compute_instantiations(
    exprs: &[Option<Expression>],
    decls: &[BoundIdentDecl],
) -> Option<Vec<Option<Expression>>> {
    let mut out = Vec::with_capacity(decls.len());
    for (index, decl) in decls.iter().enumerate() {
        match exprs.get(index).and_then(|slot| slot.as_ref()) {
            Some(expr) => {
                if expr.ty() != decl.ty() {
                    return None;
                }
                out.push(Some(expr.clone()));
            }
            None => out.push(None),
        }
    }
    Some(out)
}

/// Conjunction: `⊤` for none, the predicate itself for one, a
/// conjunction otherwise.
fn make_conj(like: &Predicate, preds: &[Predicate]) -> Predicate {
    match preds {
        [] => like.factory().literal_predicate(LiteralPredOp::BTrue, None),
        [single] => single.clone(),
        _ => like
            .factory()
            .associative_predicate(AssocPredOp::LAnd, preds.to_vec(), None),
    }
}

/// Well-definedness over instantiations: the deduplicated conjunction
/// of the lemmas of every present expression. (The reference builds
/// this through a hash set; insertion order is kept here.)
fn wd_of_instantiations(like: &Predicate, exprs: &[Option<Expression>]) -> Predicate {
    let mut lemmas: Vec<Predicate> = Vec::new();
    for expr in exprs.iter().flatten() {
        let lemma = expr.wd_lemma();
        if !lemmas.contains(&lemma) {
            lemmas.push(lemma);
        }
    }
    make_conj(like, &lemmas)
}

/// Drops `⊤` from a broken-up conjunct set.
fn remove_true(preds: &mut Vec<Predicate>) {
    preds.retain(|pred| !matches!(pred.kind(), PredicateKind::Literal(LiteralPredOp::BTrue)));
}

/// The display fragment for instantiations: `a,_,b`.
fn display_instantiations(exprs: &[Option<Expression>]) -> String {
    exprs
        .iter()
        .map(|slot| match slot {
            Some(expr) => display_expr(expr),
            None => "_".to_string(),
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// `ExI` — instantiates an existential goal:
/// `⊢ WD(E)` and `WD(E) ⊢ [x≔E]P` prove `⊢ ∃x·P`.
pub struct ExI;

impl Reasoner for ExI {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        let goal = seq.goal();
        let PredicateKind::Quantified {
            op: QuantPredOp::Exists,
            decls,
            ..
        } = goal.kind()
        else {
            return Err("Goal is not existentially quantified".into());
        };
        let exprs = exprs_input(stored, hints)?;
        let instantiations = compute_instantiations(&exprs, decls)
            .ok_or("Type error when trying to instantiate bound identifiers")?;

        let wd_pred = wd_of_instantiations(goal, &instantiations);
        let mut wd_preds = break_possible_conjunct(&wd_pred);
        remove_true(&mut wd_preds);
        let instantiated = parse_normal(&goal.instantiate(&instantiations));

        let mut second = goal_antecedent(Some(instantiated));
        second.added_hyps = wd_preds;
        Ok(Rule {
            reasoner: stored.rule.reasoner.clone(),
            goal: Some(goal.clone()),
            needed_hyps: Vec::new(),
            confidence: Confidence::DISCHARGED_MAX,
            display: format!("∃ goal (inst {})", display_instantiations(&instantiations)),
            antecedents: vec![goal_antecedent(Some(wd_pred)), second],
        })
    }
}

/// `Cut` — introduces a lemma: prove its well-definedness, prove the
/// lemma, then use it.
pub struct Cut;

impl Reasoner for Cut {
    fn replay(
        &self,
        _seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        let lemma = single_pred_input(stored, hints)?;
        let lemma_wd = lemma.wd_lemma();
        let mut wd_preds = break_possible_conjunct(&lemma_wd);
        remove_true(&mut wd_preds);

        let mut prove = goal_antecedent(Some(lemma.clone()));
        prove.added_hyps = wd_preds.clone();
        let mut use_it = goal_antecedent(None);
        use_it.added_hyps = wd_preds;
        use_it.added_hyps.push(lemma.clone());
        Ok(Rule {
            reasoner: stored.rule.reasoner.clone(),
            goal: None,
            needed_hyps: Vec::new(),
            confidence: Confidence::DISCHARGED_MAX,
            display: format!("ah ({})", display_pred(&lemma)),
            antecedents: vec![goal_antecedent(Some(lemma_wd)), prove, use_it],
        })
    }
}

/// `DoCase` — case distinction on an arbitrary predicate.
pub struct DoCase;

impl Reasoner for DoCase {
    fn replay(
        &self,
        _seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        let true_case = single_pred_input(stored, hints)?;
        let wd = true_case.wd_lemma();
        let negated = super::make_neg(&true_case);

        let mut on_true = goal_antecedent(None);
        on_true.added_hyps = vec![true_case.clone()];
        let mut on_false = goal_antecedent(None);
        on_false.added_hyps = vec![negated];
        Ok(Rule {
            reasoner: stored.rule.reasoner.clone(),
            goal: None,
            needed_hyps: Vec::new(),
            confidence: Confidence::DISCHARGED_MAX,
            display: format!("dc ({})", display_pred(&true_case)),
            antecedents: vec![goal_antecedent(Some(wd)), on_true, on_false],
        })
    }
}

/// `DisjE` — case analysis on a disjunctive hypothesis, or on a
/// membership in a union or a set extension.
pub struct DisjE;

impl Reasoner for DisjE {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        let hyp = hypothesis_input(stored, hints)?;
        if !seq.contains_hypothesis(&hyp) {
            return Err(format!("Nonexistent hypothesis: {}", display_pred(&hyp)));
        }
        let deselect = HypAction::Deselect(vec![hyp.clone()]);
        let cases: Vec<Vec<Predicate>> = match hyp.kind() {
            PredicateKind::Associative {
                op: AssocPredOp::LOr,
                children,
            } => children.iter().map(break_possible_conjunct).collect(),
            PredicateKind::Relational {
                op: RelationalOp::In,
                left,
                right,
            } => {
                let ff = hyp.factory();
                match right.kind() {
                    ExpressionKind::Associative {
                        op: AssocExprOp::BUnion,
                        children,
                    } => children
                        .iter()
                        .map(|child| {
                            vec![ff.relational_predicate(
                                RelationalOp::In,
                                left.clone(),
                                child.clone(),
                                None,
                            )]
                        })
                        .collect(),
                    ExpressionKind::SetExtension(members) if members.len() > 1 => members
                        .iter()
                        .map(|member| {
                            vec![ff.relational_predicate(
                                RelationalOp::Equal,
                                left.clone(),
                                member.clone(),
                                None,
                            )]
                        })
                        .collect(),
                    _ => {
                        return Err(format!(
                            "Case analysis not possible on hypothesis: {}",
                            display_pred(&hyp)
                        ));
                    }
                }
            }
            _ => {
                return Err(format!(
                    "Case analysis not possible on hypothesis: {}",
                    display_pred(&hyp)
                ));
            }
        };
        let antecedents = cases
            .into_iter()
            .map(|added| Antecedent {
                goal: None,
                added_hyps: added,
                unselected_added: Vec::new(),
                added_idents: Vec::new(),
                hyp_actions: vec![deselect.clone()],
            })
            .collect();
        Ok(Rule {
            reasoner: stored.rule.reasoner.clone(),
            goal: None,
            needed_hyps: vec![hyp.clone()],
            confidence: Confidence::DISCHARGED_MAX,
            display: format!("case distinction ({})", display_pred(&hyp)),
            antecedents,
        })
    }
}

/// The shared shape of `ImpE` and `ModusTollens`: an implication
/// hypothesis is split and hidden.
fn imp_hypothesis_rule(
    seq: &ProverSequent,
    stored: &StoredRule,
    hints: &ReplayHints,
    display: impl Fn(&Predicate) -> String,
    antecedents: impl Fn(&Predicate, &Predicate, HypAction) -> Vec<Antecedent>,
) -> Result<Rule, String> {
    let hyp = hypothesis_input(stored, hints)?;
    if !seq.contains_hypothesis(&hyp) {
        return Err(format!("Nonexistent hypothesis: {}", display_pred(&hyp)));
    }
    let PredicateKind::Binary {
        op: BinaryPredOp::LImp,
        left,
        right,
    } = hyp.kind()
    else {
        return Err(format!(
            "Hypothesis is not an implication: {}",
            display_pred(&hyp)
        ));
    };
    let hide = HypAction::Hide(vec![hyp.clone()]);
    Ok(Rule {
        reasoner: stored.rule.reasoner.clone(),
        goal: None,
        needed_hyps: vec![hyp.clone()],
        confidence: Confidence::DISCHARGED_MAX,
        display: display(&hyp),
        antecedents: antecedents(left, right, hide),
    })
}

/// `ImpE` — modus ponens on an implication hypothesis.
pub struct ImpE;

impl Reasoner for ImpE {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        imp_hypothesis_rule(
            seq,
            stored,
            hints,
            |hyp| format!("⇒ hyp mp ({})", display_pred(hyp)),
            |left, right, hide| {
                let mut prove_left = goal_antecedent(Some(left.clone()));
                prove_left.hyp_actions = vec![hide.clone()];
                let mut use_right = goal_antecedent(None);
                use_right.added_hyps = break_possible_conjunct(right);
                use_right.hyp_actions = vec![hide];
                vec![prove_left, use_right]
            },
        )
    }
}

/// `ModusTollens` — modus tollens on an implication hypothesis.
pub struct ModusTollens;

impl Reasoner for ModusTollens {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        imp_hypothesis_rule(
            seq,
            stored,
            hints,
            |hyp| format!("⇒ hyp mt ({})", display_pred(hyp)),
            |left, right, hide| {
                let mut prove_neg_right = goal_antecedent(Some(super::make_neg(right)));
                prove_neg_right.hyp_actions = vec![hide.clone()];
                let mut use_neg_left = goal_antecedent(None);
                use_neg_left.added_hyps = break_possible_conjunct(&super::make_neg(left));
                use_neg_left.hyp_actions = vec![hide];
                vec![prove_neg_right, use_neg_left]
            },
        )
    }
}

/// Forward-inference input recovery: the input hypothesis is the
/// single required hypothesis of the first (forward-inference) action
/// of the single antecedent; `names` recovers `ExF`'s suggestions.
fn forward_inf_input(stored: &StoredRule) -> Result<(&Predicate, Vec<&str>), String> {
    let [antecedent] = stored.rule.antecedents.as_slice() else {
        return Err("Expected exactly one antecedent.".into());
    };
    let Some(first) = antecedent.hyp_actions.first() else {
        return Err("Expected at least one hyp action.".into());
    };
    let (hyps, idents) = match first {
        HypAction::ForwardInf {
            hyps, added_idents, ..
        }
        | HypAction::Rewrite {
            hyps, added_idents, ..
        } => (hyps, added_idents),
        _ => return Err("Expected the first hyp action to be a forward inference.".into()),
    };
    let [pred] = hyps.as_slice() else {
        return Err("Expected exactly one required hypothesis.".into());
    };
    Ok((pred, idents.iter().map(|i| i.name.as_str()).collect()))
}

/// `ExF` — frees an existential hypothesis in place: a rewrite action
/// replaces it by its instantiated conjuncts.
pub struct ExF;

impl Reasoner for ExF {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        let (pred, names) = forward_inf_input(stored)?;
        let pred = hints.apply_pred(pred);
        let PredicateKind::Quantified {
            op: QuantPredOp::Exists,
            decls,
            ..
        } = pred.kind()
        else {
            return Err(format!(
                "Predicate is not existentially quantified: {}",
                display_pred(&pred)
            ));
        };
        let (idents, instantiated) = fresh_instantiation(decls, &pred, seq.type_env(), &names)?;
        let inferred = break_possible_conjunct(&instantiated);
        let rewrite = HypAction::Rewrite {
            hyps: vec![pred.clone()],
            added_idents: idents,
            inferred: inferred.clone(),
            disappearing: vec![pred.clone()],
        };
        Ok(Rule {
            reasoner: stored.rule.reasoner.clone(),
            goal: None,
            needed_hyps: Vec::new(),
            confidence: Confidence::DISCHARGED_MAX,
            display: format!("∃ hyp ({})", display_pred(&pred)),
            antecedents: vec![Antecedent {
                goal: None,
                added_hyps: Vec::new(),
                unselected_added: Vec::new(),
                added_idents: Vec::new(),
                hyp_actions: vec![rewrite, HypAction::Select(inferred)],
            }],
        })
    }
}

/// `ExE` — the deprecated existential elimination: the instantiated
/// conjuncts become antecedent hypotheses instead of a rewrite.
pub struct ExE;

impl Reasoner for ExE {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        let hyp = hypothesis_input(stored, hints)?;
        if !seq.contains_hypothesis(&hyp) {
            return Err(format!("Nonexistent hypothesis: {}", display_pred(&hyp)));
        }
        let PredicateKind::Quantified {
            op: QuantPredOp::Exists,
            decls,
            ..
        } = hyp.kind()
        else {
            return Err(format!(
                "Hypothesis is not existentially quantified: {}",
                display_pred(&hyp)
            ));
        };
        let (idents, instantiated) = fresh_instantiation(decls, &hyp, seq.type_env(), &[])?;
        Ok(Rule {
            reasoner: stored.rule.reasoner.clone(),
            goal: Some(seq.goal().clone()),
            needed_hyps: vec![hyp.clone()],
            confidence: Confidence::DISCHARGED_MAX,
            display: "∃ hyp".into(),
            antecedents: vec![Antecedent {
                goal: Some(seq.goal().clone()),
                added_hyps: break_possible_conjunct(&instantiated),
                unselected_added: Vec::new(),
                added_idents: idents,
                hyp_actions: vec![HypAction::Deselect(vec![hyp.clone()])],
            }],
        })
    }
}

/// Equality rewriting: bottom-up substitution of `from` by `to` on
/// the shared rewrite driver.
/// A matching child run inside a larger same-operator associative
/// expression is spliced; `None` means no occurrence.
struct EqualitySubst<'a> {
    from: &'a Expression,
    to: &'a Expression,
}

impl NodeRewriter for EqualitySubst<'_> {
    fn expression(&mut self, expr: &Expression) -> Option<Expression> {
        if let (
            ExpressionKind::Associative { op, children },
            ExpressionKind::Associative {
                op: from_op,
                children: from_children,
            },
        ) = (expr.kind(), self.from.kind())
            && op == from_op
        {
            // Splice the first run of `from`'s children; a full
            // match reduces to `to` through the single-child path.
            let window = from_children.len();
            let start = children.iter().position(|c| c == &from_children[0])?;
            if start + window > children.len()
                || (1..window).any(|k| children[start + k] != from_children[k])
            {
                return None;
            }
            let mut new_children: Vec<Expression> = children[..start].to_vec();
            new_children.push(self.to.clone());
            new_children.extend(children[start + window..].iter().cloned());
            let flat = driver::flatten_once(*op, new_children);
            if flat.len() == 1 {
                return Some(flat.into_iter().next().unwrap());
            }
            return Some(expr.factory().associative_expression(*op, flat, None));
        }
        (expr == self.from).then(|| self.to.clone())
    }
}

/// Normalizes freshly instantiated predicates to the shape the
/// constructors give them: EVERY nested same-operator associative
/// expression merges into its parent, because the n-ary constructors
/// flatten at construction and instantiation builds through them.
/// Deliberately stronger than [`super::as_parsed_pred`], which models
/// the print→parse round
/// trip instead (only a first child splices there); an instantiation
/// product needs the factory form, a rewrite product the parsed one.
fn parse_normal(pred: &Predicate) -> Predicate {
    struct MergeAssoc;
    impl rossi::formula::FormulaRewriter for MergeAssoc {
        fn rewrite_expression(&mut self, expr: &Expression) -> Expression {
            if let ExpressionKind::Associative { op, children } = expr.kind()
                && children.iter().any(|c| {
                    matches!(c.kind(),
                        ExpressionKind::Associative { op: inner, .. } if inner == op)
                })
            {
                return expr.factory().associative_expression(
                    *op,
                    driver::flatten_once(*op, children.clone()),
                    None,
                );
            }
            expr.clone()
        }
    }
    pred.rewrite(&mut MergeAssoc)
}

/// The predicate half of the substitution; `None` means unchanged.
fn subst_pred(pred: &Predicate, from: &Expression, to: &Expression) -> Option<Predicate> {
    driver::rewrite_pred(pred, &mut EqualitySubst { from, to })
}

/// The `EqHe` level-2 body shared by `EqL2` and `HeL2`: rewrite the
/// selected hypotheses and the goal with an equality hypothesis.
fn eq_he_rule(
    seq: &ProverSequent,
    stored: &StoredRule,
    hints: &ReplayHints,
    swap: bool,
    display: impl Fn(&Predicate) -> String,
) -> Result<Rule, String> {
    let hyp = hypothesis_input(stored, hints)?;
    if !seq.contains_hypothesis(&hyp) {
        return Err(format!("Nonexistent hypothesis: {}", display_pred(&hyp)));
    }
    let PredicateKind::Relational {
        op: RelationalOp::Equal,
        left,
        right,
    } = hyp.kind()
    else {
        return Err(format!("Unsupported hypothesis: {}", display_pred(&hyp)));
    };
    let (from, to) = if swap { (right, left) } else { (left, right) };

    // A rewrite answers None for the equality itself and for unchanged
    // predicates.
    let rewrite = |pred: &Predicate| -> Option<Predicate> {
        if *pred == hyp {
            return None;
        }
        subst_pred(pred, from, to)
    };

    let mut actions: Vec<HypAction> = Vec::new();
    let mut to_deselect: Vec<Predicate> = Vec::new();
    let mut to_select: Vec<Predicate> = Vec::new();
    for selected in seq.selected_hyp_iter() {
        let Some(rewritten) = rewrite(selected) else {
            continue;
        };
        if seq.contains_hypothesis(&rewritten) {
            if !seq.is_selected(&rewritten) {
                if !to_select.contains(&rewritten) {
                    to_select.push(rewritten);
                }
                if !to_deselect.contains(selected) {
                    to_deselect.push(selected.clone());
                }
            }
        } else {
            actions.push(HypAction::ForwardInf {
                hyps: vec![selected.clone()],
                added_idents: Vec::new(),
                inferred: vec![rewritten],
            });
            if !to_deselect.contains(selected) {
                to_deselect.push(selected.clone());
            }
        }
    }
    if !to_deselect.is_empty() {
        actions.push(HypAction::Deselect(to_deselect));
    }
    if !to_select.is_empty() {
        actions.push(HypAction::Select(to_select));
    }

    // Level 2: a rewritten-away identifier makes the equality useless —
    // hide it, or only deselect it when the identifier still occurs in
    // the unselected visible hypotheses; keep it when the replacement
    // still mentions it.
    if let ExpressionKind::FreeIdentifier(name) = from.kind()
        && !to_free_idents_contain(to, name)
    {
        let in_default = seq
            .visible_hyp_iter()
            .filter(|visible| !seq.is_selected(visible))
            .any(|pred| pred.free_identifiers().iter().any(|ident| ident == name));
        if in_default {
            actions.push(HypAction::Deselect(vec![hyp.clone()]));
        } else {
            actions.push(HypAction::Hide(vec![hyp.clone()]));
        }
    }

    let new_goal = rewrite(seq.goal());
    let goal_dependent = new_goal.is_some();
    if actions.is_empty() && !goal_dependent {
        return Err("Nothing to rewrite".into());
    }
    Ok(Rule {
        reasoner: stored.rule.reasoner.clone(),
        goal: goal_dependent.then(|| seq.goal().clone()),
        needed_hyps: vec![hyp.clone()],
        confidence: Confidence::DISCHARGED_MAX,
        display: display(&hyp),
        antecedents: vec![Antecedent {
            goal: new_goal,
            added_hyps: Vec::new(),
            unselected_added: Vec::new(),
            added_idents: Vec::new(),
            hyp_actions: actions,
        }],
    })
}

fn to_free_idents_contain(expr: &Expression, name: &str) -> bool {
    expr.free_identifiers().iter().any(|ident| ident == name)
}

/// `EqL2` — rewrites left-to-right with an equality hypothesis.
pub struct EqL2;

impl Reasoner for EqL2 {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        eq_he_rule(seq, stored, hints, false, |hyp| {
            format!("eh with {}", display_pred(hyp))
        })
    }
}

/// `HeL2` — rewrites right-to-left with an equality hypothesis.
pub struct HeL2;

impl Reasoner for HeL2 {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        eq_he_rule(seq, stored, hints, true, |hyp| {
            format!("he with {}", display_pred(hyp))
        })
    }
}

/// `AutoImpF` (`autoImpE`) — simplifies every visible implication
/// whose left side is partly discharged by selected hypotheses.
pub struct AutoImpE;

impl Reasoner for AutoImpE {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        _hints: &ReplayHints,
    ) -> Result<Rule, String> {
        let mut actions: Vec<HypAction> = Vec::new();
        for hyp in seq.visible_hyp_iter() {
            let PredicateKind::Binary {
                op: BinaryPredOp::LImp,
                left,
                right,
            } = hyp.kind()
            else {
                continue;
            };
            let left_conjuncts = break_possible_conjunct(left);
            if !left_conjuncts.iter().any(|pred| seq.is_selected(pred)) {
                continue;
            }
            let mut source_hyps: Vec<Predicate> = left_conjuncts
                .iter()
                .filter(|pred| seq.contains_hypothesis(pred))
                .cloned()
                .collect();
            let new_lhs: Vec<Predicate> = left_conjuncts
                .iter()
                .filter(|pred| !source_hyps.contains(pred))
                .cloned()
                .collect();
            source_hyps.push(hyp.clone());

            let inferred: Vec<Predicate> = if new_lhs.is_empty() {
                break_possible_conjunct(right)
            } else {
                vec![hyp.factory().binary_predicate(
                    BinaryPredOp::LImp,
                    make_conj(hyp, &new_lhs),
                    right.clone(),
                    None,
                )]
            };
            if seq.contains_hypotheses(&inferred) {
                actions.push(HypAction::Hide(vec![hyp.clone()]));
                continue;
            }
            actions.push(HypAction::Rewrite {
                hyps: source_hyps,
                added_idents: Vec::new(),
                inferred,
                disappearing: vec![hyp.clone()],
            });
        }
        if actions.is_empty() {
            return Err("Auto ImpE no more applicable".into());
        }
        Ok(Rule {
            reasoner: stored.rule.reasoner.clone(),
            goal: None,
            needed_hyps: Vec::new(),
            confidence: Confidence::DISCHARGED_MAX,
            display: "auto ImpE".into(),
            antecedents: vec![Antecedent {
                goal: None,
                added_hyps: Vec::new(),
                unselected_added: Vec::new(),
                added_idents: Vec::new(),
                hyp_actions: actions,
            }],
        })
    }
}

/// `NegEnum` — removes an enumerated value contradicted by a
/// disequality: `E∈{a,b}` and `¬E=b` infer `E∈{a}`.
pub struct NegEnum;

impl Reasoner for NegEnum {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        // `ForwardInfHypsReasoner` deserialization: the predicates are
        // the forward-inference action's required hypotheses.
        let [antecedent] = stored.rule.antecedents.as_slice() else {
            return Err("Expected exactly one antecedent.".into());
        };
        let hyps = match antecedent.hyp_actions.first() {
            Some(HypAction::ForwardInf { hyps, .. }) => hyps,
            _ => return Err("Expected the first hyp action to be a forward inference.".into()),
        };
        let predicates: Vec<Predicate> = hyps.iter().map(|hyp| hints.apply_pred(hyp)).collect();
        let [first, second] = predicates.as_slice() else {
            return Err("Invalid number of predicate input".into());
        };
        for pred in [first, second] {
            if !seq.contains_hypothesis(pred) {
                return Err(format!("Input {} is not an hypothesis", display_pred(pred)));
            }
        }
        let (inclusion, negation) = if matches!(
            first.kind(),
            PredicateKind::Relational {
                op: RelationalOp::In,
                ..
            }
        ) {
            (first, second)
        } else if matches!(
            second.kind(),
            PredicateKind::Relational {
                op: RelationalOp::In,
                ..
            }
        ) {
            (second, first)
        } else {
            return Err(format!(
                "Hypothesis {} is not an inclusion",
                display_pred(first)
            ));
        };
        let PredicateKind::Relational {
            op: RelationalOp::In,
            left: element,
            right: set,
        } = inclusion.kind()
        else {
            unreachable!("checked above");
        };
        let ExpressionKind::SetExtension(members) = set.kind() else {
            return Err(format!(
                "Predicate {} is not a set extension",
                display_expr(set)
            ));
        };
        let PredicateKind::Not(child) = negation.kind() else {
            return Err(format!(
                "Hypothesis {} is not a negation",
                display_pred(negation)
            ));
        };
        let PredicateKind::Relational {
            op: RelationalOp::Equal,
            left: eq_left,
            right: eq_right,
        } = child.kind()
        else {
            return Err(format!(
                "Predicate {} is not an equality",
                display_pred(child)
            ));
        };
        let excluded = if element == eq_left {
            Some(eq_right)
        } else if element == eq_right {
            Some(eq_left)
        } else {
            None
        };
        let new_members: Vec<Expression> = match excluded {
            Some(excluded) => members
                .iter()
                .filter(|member| *member != excluded)
                .cloned()
                .collect(),
            None => members.clone(),
        };
        if new_members.len() == members.len() || new_members.is_empty() {
            return Err(format!(
                "Negation enumeration is not applicable for hypotheses {} and {}",
                display_pred(inclusion),
                display_pred(negation)
            ));
        }
        let ff = inclusion.factory();
        let inferred = ff.relational_predicate(
            RelationalOp::In,
            element.clone(),
            ff.set_extension(new_members, None),
            None,
        );
        let sources = vec![inclusion.clone(), negation.clone()];
        Ok(Rule {
            reasoner: stored.rule.reasoner.clone(),
            goal: None,
            needed_hyps: sources.clone(),
            confidence: Confidence::DISCHARGED_MAX,
            display: format!(
                "negEnum ({},{})",
                display_pred(inclusion),
                display_pred(negation)
            ),
            antecedents: vec![Antecedent {
                goal: None,
                added_hyps: Vec::new(),
                unselected_added: Vec::new(),
                added_idents: Vec::new(),
                hyp_actions: vec![
                    HypAction::ForwardInf {
                        hyps: sources.clone(),
                        added_idents: Vec::new(),
                        inferred: vec![inferred],
                    },
                    HypAction::Deselect(sources),
                ],
            }],
        })
    }
}

/// The shared `AbstractAllD` body: instantiate a universal hypothesis.
fn all_d_rule(
    seq: &ProverSequent,
    stored: &StoredRule,
    hints: &ReplayHints,
    require_total: bool,
    name: &str,
    antecedents: impl Fn(&Predicate, &Predicate, Vec<Predicate>) -> Result<Vec<Antecedent>, String>,
) -> Result<Rule, String> {
    let univ = hypothesis_input(stored, hints)?;
    if !seq.contains_hypothesis(&univ) {
        return Err(format!("Nonexistent hypothesis:{}", display_pred(&univ)));
    }
    let PredicateKind::Quantified {
        op: QuantPredOp::Forall,
        decls,
        ..
    } = univ.kind()
    else {
        return Err(format!(
            "Hypothesis is not universally quantified:{}",
            display_pred(&univ)
        ));
    };
    let exprs = exprs_input(stored, hints)?;
    let instantiations = compute_instantiations(&exprs, decls)
        .ok_or("Type error when trying to instantiate bound identifiers")?;
    if require_total && instantiations.iter().any(Option::is_none) {
        return Err("Missing instantiation".into());
    }

    let wd_pred = wd_of_instantiations(&univ, &instantiations);
    let mut wd_preds = break_possible_conjunct(&wd_pred);
    remove_true(&mut wd_preds);
    let instantiated = parse_normal(&univ.instantiate(&instantiations));

    Ok(Rule {
        reasoner: stored.rule.reasoner.clone(),
        goal: None,
        needed_hyps: vec![univ.clone()],
        confidence: Confidence::DISCHARGED_MAX,
        display: format!("{name} (inst {})", display_instantiations(&instantiations)),
        antecedents: antecedents(&univ, &instantiated, wd_preds)?,
    })
}

fn deselect(univ: &Predicate) -> HypAction {
    HypAction::Deselect(vec![univ.clone()])
}

/// `AllD` — plain universal instantiation.
pub struct AllD;

impl Reasoner for AllD {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        all_d_rule(
            seq,
            stored,
            hints,
            false,
            "∀ hyp",
            |univ, inst, wd_preds| {
                let wd_goal = goal_antecedent(Some(make_conj(univ, &wd_preds)));
                let mut use_inst = goal_antecedent(None);
                use_inst.added_hyps = wd_preds.clone();
                for conjunct in break_possible_conjunct(inst) {
                    if !use_inst.added_hyps.contains(&conjunct) {
                        use_inst.added_hyps.push(conjunct);
                    }
                }
                use_inst.unselected_added = wd_preds;
                use_inst.hyp_actions = vec![deselect(univ)];
                Ok(vec![wd_goal, use_inst])
            },
        )
    }
}

/// `AllmpD` — universal instantiation with immediate modus ponens.
pub struct AllmpD;

impl Reasoner for AllmpD {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        all_d_rule(
            seq,
            stored,
            hints,
            true,
            "∀ hyp mp",
            |univ, inst, wd_preds| {
                let PredicateKind::Binary {
                    op: BinaryPredOp::LImp,
                    left,
                    right,
                } = inst.kind()
                else {
                    return Err("instantiation is not an implication".into());
                };
                let mut wd_goal = goal_antecedent(Some(make_conj(univ, &wd_preds)));
                wd_goal.hyp_actions = vec![deselect(univ)];
                let mut prove_left = goal_antecedent(Some(left.clone()));
                prove_left.added_hyps = wd_preds.clone();
                prove_left.unselected_added = wd_preds.clone();
                prove_left.hyp_actions = vec![deselect(univ)];
                let mut use_right = goal_antecedent(None);
                use_right.added_hyps = wd_preds.clone();
                for conjunct in break_possible_conjunct(right) {
                    if !use_right.added_hyps.contains(&conjunct) {
                        use_right.added_hyps.push(conjunct);
                    }
                }
                use_right.unselected_added = wd_preds;
                use_right.hyp_actions = vec![deselect(univ)];
                Ok(vec![wd_goal, prove_left, use_right])
            },
        )
    }
}

/// `AllmtD` — universal instantiation with immediate modus tollens.
pub struct AllmtD;

impl Reasoner for AllmtD {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        all_d_rule(
            seq,
            stored,
            hints,
            true,
            "∀ hyp mt",
            |univ, inst, wd_preds| {
                let PredicateKind::Binary {
                    op: BinaryPredOp::LImp,
                    left,
                    right,
                } = inst.kind()
                else {
                    return Err("instantiation is not an implication".into());
                };
                let mut wd_goal = goal_antecedent(Some(make_conj(univ, &wd_preds)));
                wd_goal.hyp_actions = vec![deselect(univ)];
                let mut prove_neg_right = goal_antecedent(Some(super::make_neg(right)));
                prove_neg_right.added_hyps = wd_preds.clone();
                prove_neg_right.unselected_added = wd_preds.clone();
                prove_neg_right.hyp_actions = vec![deselect(univ)];
                let mut use_neg_left = goal_antecedent(None);
                use_neg_left.added_hyps = wd_preds.clone();
                use_neg_left.added_hyps.push(super::make_neg(left));
                use_neg_left.unselected_added = wd_preds;
                use_neg_left.hyp_actions = vec![deselect(univ)];
                Ok(vec![wd_goal, prove_neg_right, use_neg_left])
            },
        )
    }
}

/// `OnePointRule` (`onePointRule`, version 2) — applies the
/// one-point rule to a quantified goal or hypothesis, with a second
/// antecedent for the replacement's well-definedness.
pub struct OnePointRule;

impl Reasoner for OnePointRule {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        // Hypothesis input: at most one needed hypothesis.
        let hyp = match stored.rule.needed_hyps.as_slice() {
            [] => None,
            [hyp] => {
                let hyp = hints.apply_pred(hyp);
                if !seq.contains_hypothesis(&hyp) {
                    return Err(format!("Nonexistent hypothesis: {}", display_pred(&hyp)));
                }
                Some(hyp)
            }
            _ => return Err("Expected at most one needed hypothesis!".into()),
        };
        let apply_to = hyp.clone().unwrap_or_else(|| seq.goal().clone());
        if !matches!(apply_to.kind(), PredicateKind::Quantified { .. }) {
            return Err(format!(
                "One point rule applied to not quantified predicate {}",
                display_pred(&apply_to)
            ));
        }
        let (simplified, replacement) =
            super::one_point::one_point_inference_with_replacement(&apply_to).ok_or_else(|| {
                format!(
                    "One point processing unsuccessful for predicate {}",
                    display_pred(&apply_to)
                )
            })?;
        let simplified = super::as_parsed_pred(&simplified).unwrap_or(simplified);
        let replacement_wd = replacement.wd_lemma();
        let display = match &hyp {
            None => "One Point Rule in goal".to_string(),
            Some(hyp) => format!("One Point Rule in {}", display_pred(hyp)),
        };
        let (goal, antecedents) = match &hyp {
            None => (
                Some(seq.goal().clone()),
                vec![
                    Antecedent {
                        goal: Some(simplified),
                        added_hyps: Vec::new(),
                        unselected_added: Vec::new(),
                        added_idents: Vec::new(),
                        hyp_actions: Vec::new(),
                    },
                    Antecedent {
                        goal: Some(replacement_wd),
                        added_hyps: Vec::new(),
                        unselected_added: Vec::new(),
                        added_idents: Vec::new(),
                        hyp_actions: Vec::new(),
                    },
                ],
            ),
            Some(hyp) => (
                None,
                vec![
                    Antecedent {
                        goal: None,
                        added_hyps: Vec::new(),
                        unselected_added: Vec::new(),
                        added_idents: Vec::new(),
                        hyp_actions: vec![HypAction::Rewrite {
                            hyps: vec![hyp.clone()],
                            added_idents: Vec::new(),
                            inferred: vec![simplified],
                            disappearing: vec![hyp.clone()],
                        }],
                    },
                    Antecedent {
                        goal: Some(replacement_wd),
                        added_hyps: Vec::new(),
                        unselected_added: Vec::new(),
                        added_idents: Vec::new(),
                        hyp_actions: vec![HypAction::Hide(vec![hyp.clone()])],
                    },
                ],
            ),
        };
        Ok(Rule {
            reasoner: stored.rule.reasoner.clone(),
            goal,
            needed_hyps: hyp.iter().cloned().collect(),
            confidence: Confidence::DISCHARGED_MAX,
            display,
            antecedents,
        })
    }
}

/// `FiniteSet` (`finiteSet`, version 0) — the goal `finite(S)`
/// reduces to the input superset `T`'s well-definedness, finiteness,
/// and `S ⊆ T`.
pub struct FiniteSet;

impl Reasoner for FiniteSet {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        _hints: &ReplayHints,
    ) -> Result<Rule, String> {
        let goal = seq.goal();
        let PredicateKind::Simple(s) = goal.kind() else {
            return Err("Goal is not a finiteness".into());
        };
        let t = stored
            .input
            .exprs
            .get("expr")
            .and_then(|list| match list.as_slice() {
                [Some(expr)] => Some(expr.clone()),
                _ => None,
            })
            .ok_or("Expected a single expression input")?;
        if s.ty() != t.ty() {
            return Err("Incorrect input type".into());
        }
        let ff = goal.factory();
        let antecedent = |goal: Predicate| Antecedent {
            goal: Some(goal),
            added_hyps: Vec::new(),
            unselected_added: Vec::new(),
            added_idents: Vec::new(),
            hyp_actions: Vec::new(),
        };
        Ok(Rule {
            reasoner: stored.rule.reasoner.clone(),
            goal: Some(goal.clone()),
            needed_hyps: Vec::new(),
            confidence: Confidence::DISCHARGED_MAX,
            display: "finite set".into(),
            antecedents: vec![
                antecedent(t.wd_lemma()),
                antecedent(ff.simple_predicate(t.clone(), None)),
                antecedent(ff.relational_predicate(RelationalOp::SubsetEq, s.clone(), t, None)),
            ],
        })
    }
}

/// `ConjF` (`conjF`) — splits a conjunctive hypothesis forward: the
/// conjunction is rewritten into its conjuncts and they are selected.
pub struct ConjF;

impl Reasoner for ConjF {
    fn replay(
        &self,
        seq: &ProverSequent,
        stored: &StoredRule,
        hints: &ReplayHints,
    ) -> Result<Rule, String> {
        let _ = seq;
        // Forward-inference input: the first rewrite action's single
        // required hypothesis.
        let hyp = stored
            .rule
            .antecedents
            .first()
            .and_then(|antecedent| antecedent.hyp_actions.first())
            .and_then(|action| match action {
                HypAction::Rewrite { hyps, .. } | HypAction::ForwardInf { hyps, .. } => {
                    hyps.first()
                }
                _ => None,
            })
            .map(|hyp| hints.apply_pred(hyp))
            .ok_or("Null hypothesis")?;
        if !matches!(
            hyp.kind(),
            PredicateKind::Associative {
                op: AssocPredOp::LAnd,
                ..
            }
        ) {
            return Err(format!(
                "Predicate is not a conjunction: {}",
                display_pred(&hyp)
            ));
        }
        let inferred = break_possible_conjunct(&hyp);
        Ok(Rule {
            reasoner: stored.rule.reasoner.clone(),
            goal: None,
            needed_hyps: Vec::new(),
            confidence: Confidence::DISCHARGED_MAX,
            display: format!("∧ hyp ({})", display_pred(&hyp)),
            antecedents: vec![Antecedent {
                goal: None,
                added_hyps: Vec::new(),
                unselected_added: Vec::new(),
                added_idents: Vec::new(),
                hyp_actions: vec![
                    HypAction::Rewrite {
                        hyps: vec![hyp.clone()],
                        added_idents: Vec::new(),
                        inferred: inferred.clone(),
                        disappearing: vec![hyp],
                    },
                    HypAction::Select(inferred),
                ],
            }],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{desc, env, pred};
    use rossi::formula::SealedTypeEnvironment;
    use rossi::parse_expression_str;

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
            input: crate::skeleton::StoredInput::default(),
        }
    }

    fn no_hints() -> ReplayHints {
        ReplayHints::default()
    }

    fn expr(env: &SealedTypeEnvironment, source: &str) -> Expression {
        parse_expression_str(source)
            .expect("test expression")
            .type_check(env)
            .typed
            .expect("test expression types")
    }

    fn sequent(env: &SealedTypeEnvironment, hyps: &[&str], goal: &str) -> ProverSequent {
        let hyps: Vec<Predicate> = hyps.iter().map(|s| pred(env, s)).collect();
        ProverSequent::new(env.clone(), hyps.clone(), [], hyps, pred(env, goal))
    }

    #[test]
    fn ex_i_instantiates_with_wd_antecedents() {
        let env = env(&[("f", "ℙ(ℤ×ℤ)"), ("y", "ℤ")]);
        let seq = sequent(&env, &[], "∃x·x=f(y)");
        let mut with_input = stored("exI");
        with_input
            .input
            .exprs
            .insert("exprs".into(), vec![Some(expr(&env, "f(y)"))]);
        let rule = ExI.replay(&seq, &with_input, &no_hints()).unwrap();
        assert_eq!(rule.antecedents.len(), 2);
        // The WD of f(y) is the first antecedent's goal, and its
        // conjuncts feed the instantiated second antecedent.
        let wd = rule.antecedents[0].goal.as_ref().unwrap();
        assert_eq!(wd, &expr(&env, "f(y)").wd_lemma());
        assert_eq!(
            rule.antecedents[1].goal.as_ref(),
            Some(&pred(&env, "f(y)=f(y)"))
        );
        assert!(!rule.antecedents[1].added_hyps.is_empty());
        assert!(rule.apply(&seq).is_some());

        // A missing instantiation keeps the quantifier.
        let mut hole = stored("exI");
        hole.input.exprs.insert("exprs".into(), vec![None]);
        let seq2 = sequent(&env, &[], "∃x,z·x=f(z)");
        let mut hole2 = hole.clone();
        hole2
            .input
            .exprs
            .get_mut("exprs")
            .unwrap()
            .push(Some(expr(&env, "y")));
        let rule = ExI.replay(&seq2, &hole2, &no_hints()).unwrap();
        let wide = crate::test_util::env(&[("f", "ℙ(ℤ×ℤ)"), ("y", "ℤ")]);
        assert_eq!(
            rule.antecedents[1].goal.as_ref(),
            Some(&pred(&wide, "∃x·x=f(y)"))
        );
    }

    #[test]
    fn cut_introduces_a_lemma_with_wildcard_use() {
        let env = env(&[("x", "ℤ")]);
        let seq = sequent(&env, &["x=1"], "x<2");
        let mut with_lemma = stored("cut");
        with_lemma
            .input
            .preds
            .insert("pred".into(), vec![Some(pred(&env, "x<9"))]);
        let rule = Cut.replay(&seq, &with_lemma, &no_hints()).unwrap();
        assert_eq!(rule.goal, None);
        assert_eq!(rule.antecedents.len(), 3);
        assert_eq!(rule.antecedents[1].goal.as_ref(), Some(&pred(&env, "x<9")));
        assert_eq!(rule.antecedents[2].goal, None);
        assert_eq!(rule.antecedents[2].added_hyps, vec![pred(&env, "x<9")]);
        let children = rule.apply(&seq).unwrap();
        assert_eq!(children.len(), 3);
        assert!(children[2].contains_hypothesis(&pred(&env, "x<9")));
    }

    #[test]
    fn do_case_splits_on_the_predicate_and_its_negation() {
        let env = env(&[("x", "ℤ")]);
        let seq = sequent(&env, &[], "x<2∨x≥2");
        let mut with_case = stored("doCase");
        with_case
            .input
            .preds
            .insert("pred".into(), vec![Some(pred(&env, "x<2"))]);
        let rule = DoCase.replay(&seq, &with_case, &no_hints()).unwrap();
        assert_eq!(rule.antecedents.len(), 3);
        assert_eq!(rule.antecedents[1].added_hyps, vec![pred(&env, "x<2")]);
        assert_eq!(rule.antecedents[2].added_hyps, vec![pred(&env, "¬x<2")]);
        assert!(rule.apply(&seq).is_some());
    }

    #[test]
    fn disj_e_cases_on_disjuncts_and_memberships() {
        let env = env(&[("x", "ℤ")]);
        let seq = sequent(&env, &["x=1∨x=2"], "x<3");
        let mut on_disj = stored("disjE");
        on_disj.rule.needed_hyps = vec![pred(&env, "x=1∨x=2")];
        let rule = DisjE.replay(&seq, &on_disj, &no_hints()).unwrap();
        assert_eq!(rule.goal, None);
        assert_eq!(rule.antecedents.len(), 2);
        assert_eq!(rule.antecedents[0].added_hyps, vec![pred(&env, "x=1")]);
        assert_eq!(
            rule.antecedents[0].hyp_actions,
            vec![HypAction::Deselect(vec![pred(&env, "x=1∨x=2")])]
        );

        // Membership in a set extension cases on equalities.
        let seq = sequent(&env, &["x∈{1,2}"], "x<3");
        let mut on_member = stored("disjE");
        on_member.rule.needed_hyps = vec![pred(&env, "x∈{1,2}")];
        let rule = DisjE.replay(&seq, &on_member, &no_hints()).unwrap();
        assert_eq!(rule.antecedents[0].added_hyps, vec![pred(&env, "x=1")]);
        assert_eq!(rule.antecedents[1].added_hyps, vec![pred(&env, "x=2")]);

        // A singleton set is not case-splittable.
        let seq = sequent(&env, &["x∈{1}"], "x<3");
        let mut on_singleton = stored("disjE");
        on_singleton.rule.needed_hyps = vec![pred(&env, "x∈{1}")];
        assert!(DisjE.replay(&seq, &on_singleton, &no_hints()).is_err());
    }

    #[test]
    fn imp_e_and_mt_split_and_hide_the_implication() {
        let env = env(&[("x", "ℤ")]);
        let imp = "x=1⇒x<2∧x<9";
        let seq = sequent(&env, &[imp], "x<3");
        let mut on_imp = stored("impE");
        on_imp.rule.needed_hyps = vec![pred(&env, imp)];
        let rule = ImpE.replay(&seq, &on_imp, &no_hints()).unwrap();
        assert_eq!(rule.goal, None);
        assert_eq!(rule.antecedents[0].goal.as_ref(), Some(&pred(&env, "x=1")));
        assert_eq!(
            rule.antecedents[1].added_hyps,
            vec![pred(&env, "x<2"), pred(&env, "x<9")]
        );
        assert_eq!(
            rule.antecedents[1].hyp_actions,
            vec![HypAction::Hide(vec![pred(&env, imp)])]
        );

        let mut on_mt = stored("mt");
        on_mt.rule.needed_hyps = vec![pred(&env, imp)];
        let rule = ModusTollens.replay(&seq, &on_mt, &no_hints()).unwrap();
        assert_eq!(
            rule.antecedents[0].goal.as_ref(),
            Some(&pred(&env, "¬(x<2∧x<9)"))
        );
        assert_eq!(rule.antecedents[1].added_hyps, vec![pred(&env, "¬x=1")]);
    }

    #[test]
    fn ex_f_frees_the_hypothesis_in_place() {
        let env = env(&[("y", "ℤ")]);
        let hyp = "∃x·x=y∧x<9";
        let seq = sequent(&env, &[hyp], "y<9");
        let mut on_ex = stored("exF");
        on_ex.rule.antecedents = vec![Antecedent {
            goal: None,
            added_hyps: Vec::new(),
            unselected_added: Vec::new(),
            added_idents: Vec::new(),
            hyp_actions: vec![HypAction::Rewrite {
                hyps: vec![pred(&env, hyp)],
                added_idents: vec![crate::sequent::TypedIdent::new(
                    "x",
                    rossi::formula::Type::Int,
                )],
                inferred: Vec::new(),
                disappearing: vec![pred(&env, hyp)],
            }],
        }];
        let rule = ExF.replay(&seq, &on_ex, &no_hints()).unwrap();
        assert_eq!(rule.goal, None);
        let wide = crate::test_util::env(&[("y", "ℤ"), ("x", "ℤ")]);
        let [rewrite, select] = rule.antecedents[0].hyp_actions.as_slice() else {
            panic!("expected rewrite and select actions");
        };
        assert_eq!(
            *rewrite,
            HypAction::Rewrite {
                hyps: vec![pred(&env, hyp)],
                added_idents: vec![crate::sequent::TypedIdent::new(
                    "x",
                    rossi::formula::Type::Int,
                )],
                inferred: vec![pred(&wide, "x=y"), pred(&wide, "x<9")],
                disappearing: vec![pred(&env, hyp)],
            }
        );
        assert_eq!(
            *select,
            HypAction::Select(vec![pred(&wide, "x=y"), pred(&wide, "x<9")])
        );
        let children = rule.apply(&seq).unwrap();
        assert!(children[0].contains_hypothesis(&pred(&wide, "x=y")));
        assert!(children[0].is_hidden(&pred(&env, hyp)));
    }

    #[test]
    fn ex_e_adds_the_freed_conjuncts_as_hypotheses() {
        let env = env(&[("y", "ℤ")]);
        let hyp = "∃x·x=y";
        let seq = sequent(&env, &[hyp], "y<9");
        let mut on_ex = stored("exE");
        on_ex.rule.needed_hyps = vec![pred(&env, hyp)];
        let rule = ExE.replay(&seq, &on_ex, &no_hints()).unwrap();
        let wide = crate::test_util::env(&[("y", "ℤ"), ("x", "ℤ")]);
        assert_eq!(rule.goal.as_ref(), Some(seq.goal()));
        assert_eq!(rule.antecedents[0].added_hyps, vec![pred(&wide, "x=y")]);
        assert_eq!(
            rule.antecedents[0].added_idents,
            vec![crate::sequent::TypedIdent::new(
                "x",
                rossi::formula::Type::Int
            )]
        );
        assert!(rule.apply(&seq).is_some());
    }

    #[test]
    fn eq_l2_rewrites_goal_and_selected_hypotheses() {
        let env = env(&[("x", "ℤ"), ("y", "ℤ")]);
        let seq = sequent(&env, &["x=y", "x<9"], "x>0");
        let mut on_eq = stored("eqL2");
        on_eq.rule.needed_hyps = vec![pred(&env, "x=y")];
        let rule = EqL2.replay(&seq, &on_eq, &no_hints()).unwrap();
        // Goal is rewritten, the rewritten hypothesis is inferred, the
        // original deselected, and the spent equality (x not used in
        // any default hypothesis) hidden.
        assert_eq!(rule.goal.as_ref(), Some(seq.goal()));
        assert_eq!(rule.antecedents[0].goal.as_ref(), Some(&pred(&env, "y>0")));
        assert_eq!(
            rule.antecedents[0].hyp_actions,
            vec![
                HypAction::ForwardInf {
                    hyps: vec![pred(&env, "x<9")],
                    added_idents: Vec::new(),
                    inferred: vec![pred(&env, "y<9")],
                },
                HypAction::Deselect(vec![pred(&env, "x<9")]),
                HypAction::Hide(vec![pred(&env, "x=y")]),
            ]
        );
        let children = rule.apply(&seq).unwrap();
        assert_eq!(children[0].goal(), &pred(&env, "y>0"));

        // he rewrites the other way; nothing mentions y, so nothing
        // rewrites — but level 2 still hides the spent equality, which
        // counts as an action, exactly as the stored rules have it.
        let mut on_he = stored("heL2");
        on_he.rule.needed_hyps = vec![pred(&env, "x=y")];
        let rule = HeL2.replay(&seq, &on_he, &no_hints()).unwrap();
        assert_eq!(rule.goal, None);
        assert_eq!(
            rule.antecedents[0].hyp_actions,
            vec![HypAction::Hide(vec![pred(&env, "x=y")])]
        );
    }

    #[test]
    fn eq_l2_splices_inside_associative_expressions() {
        let env = env(&[("a", "ℤ"), ("b", "ℤ"), ("c", "ℤ")]);
        // a+b rewritten to c inside the wider sum a+b+1.
        let seq = sequent(&env, &["a+b=c"], "a+b+1=c+1");
        let mut on_eq = stored("eqL2");
        on_eq.rule.needed_hyps = vec![pred(&env, "a+b=c")];
        let rule = EqL2.replay(&seq, &on_eq, &no_hints()).unwrap();
        assert_eq!(
            rule.antecedents[0].goal.as_ref(),
            Some(&pred(&env, "c+1=c+1"))
        );
    }

    #[test]
    fn auto_imp_e_discharges_selected_left_sides() {
        let env = env(&[("x", "ℤ")]);
        let seq = sequent(&env, &["x=1", "x=1∧x<9⇒x<2"], "x<3");
        let rule = AutoImpE
            .replay(&seq, &stored("autoImpE"), &no_hints())
            .unwrap();
        let [action] = rule.antecedents[0].hyp_actions.as_slice() else {
            panic!("expected one rewrite action");
        };
        // x=1 is discharged; x<9 remains on the left.
        assert_eq!(
            *action,
            HypAction::Rewrite {
                hyps: vec![pred(&env, "x=1"), pred(&env, "x=1∧x<9⇒x<2")],
                added_idents: Vec::new(),
                inferred: vec![pred(&env, "x<9⇒x<2")],
                disappearing: vec![pred(&env, "x=1∧x<9⇒x<2")],
            }
        );

        // Nothing to do without a selected left conjunct.
        let seq = sequent(&env, &["x=1⇒x<2"], "x<3");
        assert!(
            AutoImpE
                .replay(&seq, &stored("autoImpE"), &no_hints())
                .is_err()
        );
    }

    #[test]
    fn neg_enum_removes_the_contradicted_member() {
        let env = env(&[("x", "ℤ")]);
        let seq = sequent(&env, &["x∈{1,2}", "¬x=2"], "x=1");
        let mut on_enum = stored("negEnum");
        on_enum.rule.antecedents = vec![Antecedent {
            goal: None,
            added_hyps: Vec::new(),
            unselected_added: Vec::new(),
            added_idents: Vec::new(),
            hyp_actions: vec![HypAction::ForwardInf {
                hyps: vec![pred(&env, "x∈{1,2}"), pred(&env, "¬x=2")],
                added_idents: Vec::new(),
                inferred: Vec::new(),
            }],
        }];
        let rule = NegEnum.replay(&seq, &on_enum, &no_hints()).unwrap();
        let [fwd, deselect] = rule.antecedents[0].hyp_actions.as_slice() else {
            panic!("expected forward inference and deselect");
        };
        assert_eq!(
            *fwd,
            HypAction::ForwardInf {
                hyps: vec![pred(&env, "x∈{1,2}"), pred(&env, "¬x=2")],
                added_idents: Vec::new(),
                inferred: vec![pred(&env, "x∈{1}")],
            }
        );
        assert_eq!(
            *deselect,
            HypAction::Deselect(vec![pred(&env, "x∈{1,2}"), pred(&env, "¬x=2")])
        );
        let children = rule.apply(&seq).unwrap();
        assert!(children[0].contains_hypothesis(&pred(&env, "x∈{1}")));
    }

    #[test]
    fn all_d_family_instantiates_a_universal_hypothesis() {
        let env = env(&[("y", "ℤ")]);
        let univ = "∀x·x>0⇒x+y>y";
        let seq = sequent(&env, &[univ], "y+y>y");
        let mut on_all = stored("allD");
        on_all.rule.needed_hyps = vec![pred(&env, univ)];
        on_all
            .input
            .exprs
            .insert("exprs".into(), vec![Some(expr(&env, "y"))]);
        let rule = AllD.replay(&seq, &on_all, &no_hints()).unwrap();
        assert_eq!(rule.goal, None);
        assert_eq!(rule.antecedents.len(), 2);
        // Trivial WD: the first antecedent proves ⊤, the second gains
        // the instantiated implication.
        assert_eq!(rule.antecedents[0].goal.as_ref(), Some(&pred(&env, "⊤")));
        assert_eq!(
            rule.antecedents[1].added_hyps,
            vec![pred(&env, "y>0⇒y+y>y")]
        );
        assert!(rule.apply(&seq).is_some());

        // allmpD splits the instantiated implication.
        let mut on_mp = stored("allmpD");
        on_mp.rule.needed_hyps = vec![pred(&env, univ)];
        on_mp
            .input
            .exprs
            .insert("exprs".into(), vec![Some(expr(&env, "y"))]);
        let rule = AllmpD.replay(&seq, &on_mp, &no_hints()).unwrap();
        assert_eq!(rule.antecedents.len(), 3);
        assert_eq!(rule.antecedents[1].goal.as_ref(), Some(&pred(&env, "y>0")));
        assert_eq!(rule.antecedents[2].added_hyps, vec![pred(&env, "y+y>y")]);

        // allmtD negates both sides.
        let mut on_mt = stored("allmtD");
        on_mt.rule.needed_hyps = vec![pred(&env, univ)];
        on_mt
            .input
            .exprs
            .insert("exprs".into(), vec![Some(expr(&env, "y"))]);
        let rule = AllmtD.replay(&seq, &on_mt, &no_hints()).unwrap();
        assert_eq!(
            rule.antecedents[1].goal.as_ref(),
            Some(&pred(&env, "¬y+y>y"))
        );
        assert_eq!(rule.antecedents[2].added_hyps, vec![pred(&env, "¬y>0")]);

        // allmpD refuses a missing instantiation.
        let mut hole = stored("allmpD");
        hole.rule.needed_hyps = vec![pred(&env, univ)];
        hole.input.exprs.insert("exprs".into(), vec![None]);
        assert!(AllmpD.replay(&seq, &hole, &no_hints()).is_err());
    }
}

#[cfg(test)]
mod one_point_rule_tests {
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

    fn stored(needed: Vec<Predicate>) -> StoredRule {
        StoredRule {
            rule: Rule {
                reasoner: desc("onePointRule:2"),
                goal: None,
                needed_hyps: needed,
                confidence: Confidence::DISCHARGED_MAX,
                display: String::new(),
                antecedents: Vec::new(),
            },
            input: StoredInput::default(),
        }
    }

    #[test]
    fn finite_set_produces_the_three_antecedents() {
        let env = env(&[("S", "ℙ(ℤ)"), ("a", "ℤ"), ("b", "ℤ")]);
        let seq = sequent(&env, &[], "finite(S)");
        let mut stored = stored(Vec::new());
        stored.rule.reasoner = desc("finiteSet:0");
        stored.input.exprs.insert(
            "expr".into(),
            vec![Some(match pred(&env, "a ‥ b = a ‥ b").kind() {
                PredicateKind::Relational { left, .. } => left.clone(),
                _ => unreachable!(),
            })],
        );
        let rule = FiniteSet
            .replay(&seq, &stored, &ReplayHints::default())
            .unwrap();
        let goals: Vec<Option<Predicate>> =
            rule.antecedents.iter().map(|a| a.goal.clone()).collect();
        assert_eq!(
            goals,
            vec![
                Some(pred(&env, "⊤")),
                Some(pred(&env, "finite(a ‥ b)")),
                Some(pred(&env, "S ⊆ a ‥ b")),
            ]
        );
        assert!(rule.apply(&seq).is_some());
    }

    #[test]
    fn conj_f_splits_a_conjunctive_hypothesis() {
        let env = env(&[("a", "ℤ"), ("b", "ℤ")]);
        let hyp = pred(&env, "a > 0 ∧ b > 0");
        let seq = sequent(&env, &["a > 0 ∧ b > 0"], "⊥");
        let mut stored = stored(Vec::new());
        stored.rule.reasoner = desc("conjF");
        stored.rule.antecedents = vec![Antecedent {
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
        }];
        let rule = ConjF
            .replay(&seq, &stored, &ReplayHints::default())
            .unwrap();
        assert_eq!(
            rule.antecedents[0].hyp_actions,
            vec![
                HypAction::Rewrite {
                    hyps: vec![hyp.clone()],
                    added_idents: Vec::new(),
                    inferred: vec![pred(&env, "a > 0"), pred(&env, "b > 0")],
                    disappearing: vec![hyp],
                },
                HypAction::Select(vec![pred(&env, "a > 0"), pred(&env, "b > 0")]),
            ]
        );
        assert!(rule.apply(&seq).is_some());
    }

    #[test]
    fn one_point_rule_on_the_goal_adds_the_wd_antecedent() {
        let env = env(&[("S", "ℙ(ℤ)"), ("g", "ℙ(ℤ×ℤ)"), ("x", "ℤ")]);
        let seq = sequent(&env, &[], "∃z·z = g(x) ∧ z ∈ S");
        let rule = OnePointRule
            .replay(&seq, &stored(Vec::new()), &ReplayHints::default())
            .unwrap();
        assert_eq!(rule.goal.as_ref(), Some(seq.goal()));
        assert_eq!(
            rule.antecedents[0].goal.as_ref(),
            Some(&pred(&env, "g(x) ∈ S"))
        );
        assert_eq!(
            rule.antecedents[1].goal.as_ref(),
            Some(&pred(&env, "x ∈ dom(g) ∧ g ∈ ℤ ⇸ ℤ"))
        );
        assert!(rule.apply(&seq).is_some());
    }

    #[test]
    fn one_point_rule_on_a_hypothesis_rewrites_and_hides() {
        let env = env(&[("S", "ℙ(ℤ)")]);
        let hyp = pred(&env, "∀z·z = 3 ⇒ z ∈ S");
        let seq = sequent(&env, &["∀z·z = 3 ⇒ z ∈ S"], "⊥");
        let rule = OnePointRule
            .replay(&seq, &stored(vec![hyp.clone()]), &ReplayHints::default())
            .unwrap();
        assert_eq!(rule.goal, None);
        assert_eq!(rule.needed_hyps, vec![hyp.clone()]);
        assert_eq!(
            rule.antecedents[0].hyp_actions,
            vec![HypAction::Rewrite {
                hyps: vec![hyp.clone()],
                added_idents: Vec::new(),
                inferred: vec![pred(&env, "3 ∈ S")],
                disappearing: vec![hyp.clone()],
            }]
        );
        assert_eq!(rule.antecedents[1].goal.as_ref(), Some(&pred(&env, "⊤")));
        assert_eq!(
            rule.antecedents[1].hyp_actions,
            vec![HypAction::Hide(vec![hyp])]
        );
        assert!(rule.apply(&seq).is_some());
    }
}
