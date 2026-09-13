//! Post-order formula rewriting.
//!
//! The single driver owns the traversal: children are rewritten first,
//! the node is rebuilt only if a child changed (otherwise the original
//! handle is reused, so an identity rewrite returns pointer-equal
//! trees), and the rewriter's hook then sees the rebuilt node. The
//! driver also owns the binding-depth protocol: it brackets every
//! quantified body with `entering_quantifier`/`leaving_quantifier`, and
//! rewrites declarations (their annotations) *outside* the bracket,
//! since annotations are scoped to the enclosing context.

use super::assignment::{Assignment, AssignmentKind};
use super::decl::BoundIdentDecl;
use super::expression::{Expression, ExpressionKind};
use super::factory::FormulaFactory;
use super::predicate::{Predicate, PredicateKind};
use super::subst::remove_unused_decls;
use super::tag::UnaryExprOp;

/// A formula transformation, driven post-order by the rewrite driver.
///
/// The hooks receive nodes whose children are already rewritten;
/// returning the argument unchanged (the default) leaves the node
/// as-is. Implementations tracking binding depth maintain it in the
/// `entering_quantifier`/`leaving_quantifier` callbacks.
pub trait FormulaRewriter {
    /// Whether the driver should flatten while rebuilding: merge nested
    /// same-operator associative nodes, drop unused quantifier
    /// declarations, merge directly nested same-operator quantified
    /// predicates, and fold the negation of an integer literal.
    fn auto_flattening(&self) -> bool {
        false
    }

    /// Called before rewriting the body of a construct binding
    /// `n_decls` declarations.
    fn entering_quantifier(&mut self, n_decls: usize) {
        let _ = n_decls;
    }

    /// Called after rewriting the body of a construct binding
    /// `n_decls` declarations.
    fn leaving_quantifier(&mut self, n_decls: usize) {
        let _ = n_decls;
    }

    /// Rewrites one expression whose children are already rewritten.
    fn rewrite_expression(&mut self, expr: &Expression) -> Expression {
        expr.clone()
    }

    /// Rewrites one predicate whose children are already rewritten.
    fn rewrite_predicate(&mut self, pred: &Predicate) -> Predicate {
        pred.clone()
    }
}

/// The identity rewriter with flattening on.
struct Flattener;

impl FormulaRewriter for Flattener {
    fn auto_flattening(&self) -> bool {
        true
    }
}

impl Expression {
    /// Rewrites this expression post-order. An identity rewrite returns
    /// a pointer-equal handle.
    pub fn rewrite(&self, rewriter: &mut dyn FormulaRewriter) -> Expression {
        rewrite_expr(self, rewriter)
    }

    /// Normalizes the expression: merges nested same-operator
    /// associative and quantified nodes, drops unused quantifier
    /// declarations, and folds negated integer literals.
    pub fn flatten(&self) -> Expression {
        self.rewrite(&mut Flattener)
    }
}

impl Predicate {
    /// Rewrites this predicate post-order. An identity rewrite returns
    /// a pointer-equal handle.
    pub fn rewrite(&self, rewriter: &mut dyn FormulaRewriter) -> Predicate {
        rewrite_pred(self, rewriter)
    }

    /// Normalizes the predicate; see [`Expression::flatten`].
    pub fn flatten(&self) -> Predicate {
        self.rewrite(&mut Flattener)
    }
}

impl Assignment {
    /// Rewrites the formulas of this assignment post-order. There is no
    /// assignment-level hook: the rewriter sees the embedded
    /// expressions and predicates.
    pub fn rewrite(&self, rewriter: &mut dyn FormulaRewriter) -> Assignment {
        rewrite_assign(self, rewriter)
    }

    /// Normalizes the assignment's formulas; see
    /// [`Expression::flatten`].
    pub fn flatten(&self) -> Assignment {
        self.rewrite(&mut Flattener)
    }
}

/// Merges a directly nested quantifier of the same kind into `decls`,
/// when its declaration annotations permit; `true` iff merged.
///
/// An inner declaration's annotation is scoped under the outer
/// binders; in the merged declaration list it must read in the
/// enclosing scope instead. A reference to an outer binder cannot be
/// expressed there — the nesting is kept in that case — and a
/// reference to an enclosing binder renumbers past the departed outer
/// frame.
fn merge_nested_quantifier(
    ff: &FormulaFactory,
    op: super::tag::QuantPredOp,
    decls: &mut Vec<BoundIdentDecl>,
    body: &mut Predicate,
) -> bool {
    let nested = match body.kind() {
        PredicateKind::Quantified {
            op: inner,
            decls: inner_decls,
            pred: inner_pred,
        } if *inner == op => Some((inner_decls.clone(), inner_pred.clone())),
        _ => None,
    };
    let Some((inner_decls, inner_pred)) = nested else {
        return false;
    };
    let n_outer = decls.len() as u32;
    let mergeable = inner_decls.iter().all(|d| {
        d.annotation()
            .is_none_or(|a| a.dangling_bound_indices().iter().all(|&i| i >= n_outer))
    });
    if !mergeable {
        return false;
    }
    decls.extend(inner_decls.iter().map(|d| match d.annotation() {
        Some(a) if !a.dangling_bound_indices().is_empty() => ff.bound_ident_decl(
            d.name(),
            d.span(),
            Some(a.shift_bound_identifiers(-(n_outer as i32))),
            d.ty().cloned(),
        ),
        _ => d.clone(),
    }));
    *body = inner_pred;
    true
}

/// The flattening normalization of one quantified-predicate node:
/// declarations the body does not reference are dropped (the
/// quantifier disappearing when none remain), and a directly nested
/// quantifier of the same kind is merged into the declaration list.
/// `None` when the node is already normal. The body itself is not
/// visited — this is the per-node effect the flattening rewriter
/// applies to every quantifier it traverses.
pub fn normalize_quantified_predicate(p: &Predicate) -> Option<Predicate> {
    let PredicateKind::Quantified { op, decls, pred } = p.kind() else {
        return None;
    };
    let ff = p.factory().clone();
    let mut decls2 = decls.clone();
    let mut body = pred.clone();
    let mut changed = false;
    match remove_unused_decls(&decls2, &body) {
        Some((kept, new_body)) if kept.is_empty() => return Some(new_body),
        Some((kept, new_body)) => {
            decls2 = kept;
            body = new_body;
            changed = true;
        }
        None => {}
    }
    if merge_nested_quantifier(&ff, *op, &mut decls2, &mut body) {
        changed = true;
    }
    changed.then(|| ff.quantified_predicate(*op, decls2, body, p.span()))
}

fn same_expr(a: &Expression, b: &Expression) -> bool {
    std::sync::Arc::ptr_eq(&a.0, &b.0)
}

fn same_pred(a: &Predicate, b: &Predicate) -> bool {
    std::sync::Arc::ptr_eq(&a.0, &b.0)
}

/// Rewrites a child list; `true` if any element changed.
fn rewrite_exprs(children: &[Expression], rw: &mut dyn FormulaRewriter) -> (Vec<Expression>, bool) {
    let mut changed = false;
    let new: Vec<Expression> = children
        .iter()
        .map(|c| {
            let c2 = rewrite_expr(c, rw);
            changed |= !same_expr(&c2, c);
            c2
        })
        .collect();
    (new, changed)
}

fn rewrite_preds(children: &[Predicate], rw: &mut dyn FormulaRewriter) -> (Vec<Predicate>, bool) {
    let mut changed = false;
    let new: Vec<Predicate> = children
        .iter()
        .map(|c| {
            let c2 = rewrite_pred(c, rw);
            changed |= !same_pred(&c2, c);
            c2
        })
        .collect();
    (new, changed)
}

/// Rewrites declaration annotations (in the enclosing scope).
fn rewrite_decls(
    decls: &[BoundIdentDecl],
    rw: &mut dyn FormulaRewriter,
) -> (Vec<BoundIdentDecl>, bool) {
    let mut changed = false;
    let new: Vec<BoundIdentDecl> = decls
        .iter()
        .map(|d| {
            let Some(annotation) = d.annotation() else {
                return d.clone();
            };
            let a2 = rewrite_expr(annotation, rw);
            if same_expr(&a2, annotation) {
                return d.clone();
            }
            changed = true;
            d.factory()
                .clone()
                .bound_ident_decl(d.name(), d.span(), Some(a2), d.ty().cloned())
        })
        .collect();
    (new, changed)
}

pub(super) fn rewrite_expr(e: &Expression, rw: &mut dyn FormulaRewriter) -> Expression {
    let ff = e.factory().clone();
    let rebuilt = match e.kind() {
        ExpressionKind::FreeIdentifier(_)
        | ExpressionKind::BoundIdentifier(_)
        | ExpressionKind::IntegerLiteral(_)
        | ExpressionKind::Atomic(_) => e.clone(),
        ExpressionKind::SetExtension(members) => {
            let (new, changed) = rewrite_exprs(members, rw);
            if changed {
                ff.set_extension(new, e.span())
            } else {
                e.clone()
            }
        }
        ExpressionKind::Bool(pred) => {
            let p2 = rewrite_pred(pred, rw);
            if same_pred(&p2, pred) {
                e.clone()
            } else {
                ff.bool_expression(p2, e.span())
            }
        }
        ExpressionKind::Binary { op, left, right } => {
            let l2 = rewrite_expr(left, rw);
            let r2 = rewrite_expr(right, rw);
            if same_expr(&l2, left) && same_expr(&r2, right) {
                e.clone()
            } else {
                ff.binary_expression(*op, l2, r2, e.span())
            }
        }
        ExpressionKind::Associative { op, children } => {
            let (mut new, mut changed) = rewrite_exprs(children, rw);
            if rw.auto_flattening() && new.iter().any(|c| c.tag() == op.tag()) {
                new = new
                    .into_iter()
                    .flat_map(|c| {
                        let inlined = match c.kind() {
                            ExpressionKind::Associative { children, .. } if c.tag() == op.tag() => {
                                Some(children.clone())
                            }
                            _ => None,
                        };
                        inlined.unwrap_or_else(|| vec![c])
                    })
                    .collect();
                changed = true;
            }
            if changed {
                ff.associative_expression(*op, new, e.span())
            } else {
                e.clone()
            }
        }
        ExpressionKind::Unary { op, child } => {
            let c2 = rewrite_expr(child, rw);
            if rw.auto_flattening()
                && *op == UnaryExprOp::UnMinus
                && let ExpressionKind::IntegerLiteral(value) = c2.kind()
            {
                let negated = ff.integer_literal(-value.clone(), e.span());
                return rw.rewrite_expression(&negated);
            }
            if same_expr(&c2, child) {
                e.clone()
            } else {
                ff.unary_expression(*op, c2, e.span())
            }
        }
        ExpressionKind::Quantified {
            op,
            decls,
            pred,
            expr,
            form,
        } => {
            let (decls2, decls_changed) = rewrite_decls(decls, rw);
            rw.entering_quantifier(decls.len());
            let p2 = rewrite_pred(pred, rw);
            let x2 = rewrite_expr(expr, rw);
            rw.leaving_quantifier(decls.len());
            // Unused declarations are deliberately kept here: only
            // quantified predicates drop them while flattening.
            if !decls_changed && same_pred(&p2, pred) && same_expr(&x2, expr) {
                e.clone()
            } else {
                ff.quantified_expression(*op, decls2, p2, x2, e.span(), *form)
            }
        }
        ExpressionKind::Ascription { expr, type_expr } => {
            let x2 = rewrite_expr(expr, rw);
            let t2 = rewrite_expr(type_expr, rw);
            if same_expr(&x2, expr) && same_expr(&t2, type_expr) {
                e.clone()
            } else {
                ff.ascription(x2, t2, e.span())
            }
        }
        ExpressionKind::Extended { tag, exprs, preds } => {
            let (exprs2, ec) = rewrite_exprs(exprs, rw);
            let (preds2, pc) = rewrite_preds(preds, rw);
            if ec || pc {
                let Some(super::extension::Extension::Expr(ext)) = ff.extension(*tag).cloned()
                else {
                    unreachable!("extended nodes carry a registered extension");
                };
                ff.extended_expression(&ext, exprs2, preds2, e.span(), None)
                    .expect("a rebuilt node keeps its shape")
            } else {
                e.clone()
            }
        }
    };
    rw.rewrite_expression(&rebuilt)
}

pub(super) fn rewrite_pred(p: &Predicate, rw: &mut dyn FormulaRewriter) -> Predicate {
    let ff = p.factory().clone();
    let rebuilt = match p.kind() {
        PredicateKind::Literal(_) | PredicateKind::PredicateVariable(_) => p.clone(),
        PredicateKind::Relational { op, left, right } => {
            let l2 = rewrite_expr(left, rw);
            let r2 = rewrite_expr(right, rw);
            if same_expr(&l2, left) && same_expr(&r2, right) {
                p.clone()
            } else {
                ff.relational_predicate(*op, l2, r2, p.span())
            }
        }
        PredicateKind::Binary { op, left, right } => {
            let l2 = rewrite_pred(left, rw);
            let r2 = rewrite_pred(right, rw);
            if same_pred(&l2, left) && same_pred(&r2, right) {
                p.clone()
            } else {
                ff.binary_predicate(*op, l2, r2, p.span())
            }
        }
        PredicateKind::Associative { op, children } => {
            let (mut new, mut changed) = rewrite_preds(children, rw);
            if rw.auto_flattening() && new.iter().any(|c| c.tag() == op.tag()) {
                new = new
                    .into_iter()
                    .flat_map(|c| {
                        let inlined = match c.kind() {
                            PredicateKind::Associative { children, .. } if c.tag() == op.tag() => {
                                Some(children.clone())
                            }
                            _ => None,
                        };
                        inlined.unwrap_or_else(|| vec![c])
                    })
                    .collect();
                changed = true;
            }
            if changed {
                ff.associative_predicate(*op, new, p.span())
            } else {
                p.clone()
            }
        }
        PredicateKind::Not(child) => {
            let c2 = rewrite_pred(child, rw);
            if same_pred(&c2, child) {
                p.clone()
            } else {
                ff.not_predicate(c2, p.span())
            }
        }
        PredicateKind::Quantified { op, decls, pred } => {
            let (mut decls2, decls_changed) = rewrite_decls(decls, rw);
            rw.entering_quantifier(decls.len());
            let mut p2 = rewrite_pred(pred, rw);
            rw.leaving_quantifier(decls.len());
            let mut changed = decls_changed || !same_pred(&p2, pred);

            if rw.auto_flattening() {
                // Drop declarations the body no longer references,
                // renumbering the survivors. If none survive, the
                // quantifier itself disappears.
                match remove_unused_decls(&decls2, &p2) {
                    Some((kept, body)) if kept.is_empty() => {
                        // The body has already been rewritten (and seen
                        // by the hook); it replaces the quantifier
                        // wholesale.
                        return body;
                    }
                    Some((kept, body)) => {
                        decls2 = kept;
                        p2 = body;
                        changed = true;
                    }
                    None => {}
                }
                if merge_nested_quantifier(&ff, *op, &mut decls2, &mut p2) {
                    changed = true;
                }
            }

            if changed {
                ff.quantified_predicate(*op, decls2, p2, p.span())
            } else {
                p.clone()
            }
        }
        PredicateKind::Simple(child) => {
            let c2 = rewrite_expr(child, rw);
            if same_expr(&c2, child) {
                p.clone()
            } else {
                ff.simple_predicate(c2, p.span())
            }
        }
        PredicateKind::Multiple(children) => {
            let (new, changed) = rewrite_exprs(children, rw);
            if changed {
                ff.multiple_predicate(new, p.span())
            } else {
                p.clone()
            }
        }
        PredicateKind::Application {
            function,
            function_span,
            args,
        } => {
            let (new, changed) = rewrite_exprs(args, rw);
            if changed {
                ff.predicate_application(function, *function_span, new, p.span())
            } else {
                p.clone()
            }
        }
        PredicateKind::Extended { tag, exprs, preds } => {
            let (exprs2, ec) = rewrite_exprs(exprs, rw);
            let (preds2, pc) = rewrite_preds(preds, rw);
            if ec || pc {
                let Some(super::extension::Extension::Pred(ext)) = ff.extension(*tag).cloned()
                else {
                    unreachable!("extended nodes carry a registered extension");
                };
                ff.extended_predicate(&ext, exprs2, preds2, p.span())
                    .expect("a rebuilt node keeps its shape")
            } else {
                p.clone()
            }
        }
    };
    rw.rewrite_predicate(&rebuilt)
}

pub(super) fn rewrite_assign(a: &Assignment, rw: &mut dyn FormulaRewriter) -> Assignment {
    let ff = a.factory().clone();
    match a.kind() {
        AssignmentKind::BecomesEqualTo { idents, values } => {
            let (idents2, ic) = rewrite_exprs(idents, rw);
            let (values2, vc) = rewrite_exprs(values, rw);
            if ic || vc {
                ff.becomes_equal_to(idents2, values2, a.span())
            } else {
                a.clone()
            }
        }
        AssignmentKind::BecomesMemberOf { idents, set } => {
            let (idents2, ic) = rewrite_exprs(idents, rw);
            let s2 = rewrite_expr(set, rw);
            if ic || !same_expr(&s2, set) {
                ff.becomes_member_of(idents2, s2, a.span())
            } else {
                a.clone()
            }
        }
        AssignmentKind::BecomesSuchThat {
            idents,
            primed,
            pred,
        } => {
            let (idents2, ic) = rewrite_exprs(idents, rw);
            let (primed2, pc) = rewrite_decls(primed, rw);
            rw.entering_quantifier(primed.len());
            let p2 = rewrite_pred(pred, rw);
            rw.leaving_quantifier(primed.len());
            if ic || pc || !same_pred(&p2, pred) {
                ff.becomes_such_that(idents2, primed2, p2, a.span())
            } else {
                a.clone()
            }
        }
    }
}

/// Unwraps `E ⦂ T` to `E` everywhere, bottom-up.
struct StripAscriptions;

impl FormulaRewriter for StripAscriptions {
    fn rewrite_expression(&mut self, expr: &Expression) -> Expression {
        match expr.kind() {
            ExpressionKind::Ascription { expr: inner, .. } => inner.clone(),
            _ => expr.clone(),
        }
    }
}

impl Predicate {
    /// This predicate with every type ascription `E ⦂ T` unwrapped to
    /// `E`.
    ///
    /// Type-inference artifacts are emitted into formula strings
    /// (`∅ ⦂ ℙ(T)`, `∀x⦂T·P`); semantic comparison and hypothesis
    /// identity need the ascription nodes dropped. On a type-checked
    /// formula the solved types survive: stripping removes only the
    /// wrapper node.
    #[must_use]
    pub fn strip_ascriptions(&self) -> Predicate {
        self.rewrite(&mut StripAscriptions)
    }
}

impl Expression {
    /// This expression with every type ascription `E ⦂ T` unwrapped to
    /// `E` — see [`Predicate::strip_ascriptions`].
    #[must_use]
    pub fn strip_ascriptions(&self) -> Expression {
        self.rewrite(&mut StripAscriptions)
    }
}

impl Assignment {
    /// This assignment with every type ascription `E ⦂ T` unwrapped to
    /// `E` — see [`Predicate::strip_ascriptions`].
    #[must_use]
    pub fn strip_ascriptions(&self) -> Assignment {
        self.rewrite(&mut StripAscriptions)
    }
}
