//! The WD L-operator — port of rodin-ast's
//! `org.eventb.internal.core.ast.wd.WDComputer`.
//!
//! Computes the well-definedness lemma of a predicate, expression, or
//! assignment over the raw (source-form) AST. Runs on the *unenriched*
//! formula because the lemma embeds verbatim fragments of the original —
//! Rodin's `toString` preserves the source's comprehension forms, so
//! rendering the SC-lowered AST would diverge from the oracle.
//!
//! Types are needed in exactly one rule (`f(x)` ⇒ `f ∈ src ⇸ trg`): the
//! computer carries a scoped [`TypeEnv`] seeded with the component
//! environment and extended at each binder using the same inference the
//! enrichment pass uses. When a function's type cannot be resolved, the
//! whole formula is flagged ([`WdComputer::failed`]) and the caller skips
//! its finding rather than emit a partial lemma.

use rossi::ast::expression::{BinaryOp, BuiltinFunction};
use rossi::ast::predicate::LogicalOp;
use rossi::{Action, Expression, Predicate, TypedIdentifier};

use crate::infer::{collect_binder_types, parse_type_from_expression, type_of_expression};
use crate::type_env::TypeEnv;
use crate::wd::builder as fb;

/// One WD computation over a single formula.
pub struct WdComputer {
    env: TypeEnv,
    /// Set when a sub-lemma could not be built (untypeable function in a
    /// function application). The formula's finding must be skipped.
    pub failed: Option<String>,
}

impl WdComputer {
    pub fn new(env: TypeEnv) -> Self {
        Self { env, failed: None }
    }

    /// Clear the per-formula state so one computer can serve every formula
    /// sharing a base environment. `scoped` always pops the binder scope it
    /// pushes, so the env is already back at its base after a computation —
    /// only [`Self::failed`] needs resetting.
    pub fn reset(&mut self) {
        self.failed = None;
    }

    // -----------------------------------------------------------------
    // Predicates
    // -----------------------------------------------------------------

    pub fn wd_predicate(&mut self, p: &Predicate) -> Predicate {
        match p {
            Predicate::True | Predicate::False => Predicate::True,
            Predicate::Comparison { left, right, .. } => {
                fb::land2(self.wd_expression(left), self.wd_expression(right))
            }
            Predicate::Not(inner) => self.wd_predicate(inner),
            Predicate::Logical {
                op: op @ (LogicalOp::And | LogicalOp::Or),
                ..
            } => {
                let mut children: Vec<&Predicate> = Vec::new();
                flatten_chain(p, *op, &mut children);
                if *op == LogicalOp::And {
                    self.land_wd(&children)
                } else {
                    self.lor_wd(&children)
                }
            }
            Predicate::Logical {
                op: LogicalOp::Implies,
                left,
                right,
            } => {
                let wd_left = self.wd_predicate(left);
                let wd_right = self.wd_predicate(right);
                fb::land2(wd_left, fb::limp((**left).clone(), wd_right))
            }
            Predicate::Logical {
                op: LogicalOp::Equivalent,
                left,
                right,
            } => fb::land2(self.wd_predicate(left), self.wd_predicate(right)),
            Predicate::Quantified {
                identifiers,
                predicate,
                ..
            } => {
                // Both ∀ and ∃ produce a universally quantified lemma.
                let body = self.scoped(identifiers, Some(predicate), |s| s.wd_predicate(predicate));
                fb::forall(untyped(identifiers), body)
            }
            Predicate::Application { arguments, .. }
            | Predicate::BuiltinApplication { arguments, .. } => self.wd_expressions(arguments),
        }
    }

    /// `landWD` — right-to-left fold over the flattened conjunction:
    /// each conjunct's WD is asserted under the hypothesis of the
    /// conjuncts before it.
    fn land_wd(&mut self, children: &[&Predicate]) -> Predicate {
        let mut result = Predicate::True;
        for child in children.iter().rev() {
            let wd = self.wd_predicate(child);
            result = fb::land2(wd, fb::limp((*child).clone(), result));
        }
        result
    }

    /// `lorWD` — dual of [`Self::land_wd`] with disjunction.
    fn lor_wd(&mut self, children: &[&Predicate]) -> Predicate {
        let mut result = Predicate::True;
        for child in children.iter().rev() {
            let wd = self.wd_predicate(child);
            result = fb::land2(wd, fb::lor2((*child).clone(), result));
        }
        result
    }

    // -----------------------------------------------------------------
    // Expressions
    // -----------------------------------------------------------------

    pub fn wd_expression(&mut self, e: &Expression) -> Predicate {
        match e {
            Expression::Integer(_)
            | Expression::Identifier(_)
            | Expression::True
            | Expression::False
            | Expression::EmptySet
            | Expression::Naturals
            | Expression::Naturals1
            | Expression::Integers
            | Expression::BoolType
            | Expression::StringLiteral(_) => Predicate::True,

            Expression::SetEnumeration(items) => self.wd_expressions(items),

            Expression::Binary { op, left, right } => {
                let wd_l = self.wd_expression(left);
                let wd_r = self.wd_expression(right);
                let extra = match op {
                    BinaryOp::Divide => fb::not_zero((**right).clone()),
                    BinaryOp::Modulo => fb::land2(
                        fb::non_negative((**left).clone()),
                        fb::positive((**right).clone()),
                    ),
                    BinaryOp::Exponent => fb::land2(
                        fb::non_negative((**left).clone()),
                        fb::non_negative((**right).clone()),
                    ),
                    _ => Predicate::True,
                };
                fb::land(vec![wd_l, wd_r, extra])
            }

            Expression::Unary { operand, .. } => self.wd_expression(operand),

            Expression::BuiltinApplication {
                function,
                arguments,
            } => {
                let wd_children = self.wd_expressions(arguments);
                let extra = match (function, arguments.as_slice()) {
                    (BuiltinFunction::Card, [arg]) => fb::finite(arg.clone()),
                    (BuiltinFunction::Min, [arg]) => {
                        fb::land2(fb::not_empty(arg.clone()), fb::bounded(arg.clone(), true))
                    }
                    (BuiltinFunction::Max, [arg]) => {
                        fb::land2(fb::not_empty(arg.clone()), fb::bounded(arg.clone(), false))
                    }
                    (BuiltinFunction::Inter, [arg]) => fb::not_empty(arg.clone()),
                    _ => Predicate::True,
                };
                fb::land2(wd_children, extra)
            }

            Expression::FunctionApplication {
                function,
                arguments,
            } => {
                let mut parts = vec![self.wd_expression(function)];
                parts.push(self.wd_expressions(arguments));
                if !self.is_builtin_total(function) {
                    // A multi-argument application `f(a, b)` denotes
                    // application to the maplet tuple `a ↦ b`.
                    let Some(arg) = (!arguments.is_empty())
                        .then(|| crate::ast_util::left_assoc_maplet(arguments))
                    else {
                        self.fail("function application without argument");
                        return Predicate::True;
                    };
                    let domain = fb::in_domain((**function).clone(), arg);
                    match type_of_expression(&self.env, function)
                        .and_then(|ty| fb::partial((**function).clone(), &ty))
                    {
                        Some(partial) => parts.push(fb::land2(domain, partial)),
                        None => {
                            self.fail("cannot type function in application");
                            return Predicate::True;
                        }
                    }
                }
                fb::land(parts)
            }

            Expression::RelationalImage { relation, set } => {
                fb::land2(self.wd_expression(relation), self.wd_expression(set))
            }

            Expression::SetComprehension {
                identifiers,
                predicate,
                expression,
            } => {
                self.comprehension_wd(identifiers.clone(), predicate, expression.as_deref(), false)
            }

            Expression::SetBuilder {
                member_expression,
                predicate,
            } => {
                // Rodin's implicit comprehension `{E ∣ P}` binds every
                // identifier occurring in E.
                let mut names: Vec<&str> = Vec::new();
                crate::infer::collect_free_identifiers(member_expression, &mut names);
                let mut decls: Vec<TypedIdentifier> = Vec::new();
                for n in names {
                    if decls.iter().all(|d| d.name != n) {
                        decls.push(TypedIdentifier::untyped(n.to_string()));
                    }
                }
                self.comprehension_wd(decls, predicate, Some(member_expression), false)
            }

            Expression::QuantifiedUnion {
                identifiers,
                predicate,
                expression,
            } => self.comprehension_wd(identifiers.clone(), predicate, Some(expression), false),

            Expression::QuantifiedInter {
                identifiers,
                predicate,
                expression,
            } => self.comprehension_wd(identifiers.clone(), predicate, Some(expression), true),

            Expression::Lambda {
                pattern,
                predicate,
                expression,
            } => {
                let decls: Vec<TypedIdentifier> = pattern
                    .identifiers()
                    .into_iter()
                    .map(|n| TypedIdentifier::untyped(n.to_string()))
                    .collect();
                self.comprehension_wd(decls, predicate, Some(expression), false)
            }

            Expression::Bool(p) => self.wd_predicate(p),

            Expression::IfThenElse {
                condition,
                then_expr,
                else_expr,
            } => {
                // ProB extension, absent from Rodin models.
                let c = self.wd_predicate(condition);
                let t = self.wd_expression(then_expr);
                let f = self.wd_expression(else_expr);
                fb::land(vec![c, t, f])
            }
        }
    }

    /// Shared rule for CSET / λ / ⋃ / ⋂:
    /// `∀decls·wd(P) ∧ (P ⇒ wd(E))`, plus `∃decls·P` for ⋂.
    fn comprehension_wd(
        &mut self,
        decls: Vec<TypedIdentifier>,
        predicate: &Predicate,
        expression: Option<&Expression>,
        require_nonempty: bool,
    ) -> Predicate {
        let body = self.scoped(&decls, Some(predicate), |s| {
            let wd_p = s.wd_predicate(predicate);
            let wd_e = expression.map_or(Predicate::True, |e| s.wd_expression(e));
            fb::land2(wd_p, fb::limp(predicate.clone(), wd_e))
        });
        let children_wd = fb::forall(untyped(&decls), body);
        let local_wd = if require_nonempty {
            fb::exists(untyped(&decls), predicate.clone())
        } else {
            Predicate::True
        };
        fb::land2(children_wd, local_wd)
    }

    // -----------------------------------------------------------------
    // Assignments
    // -----------------------------------------------------------------

    pub fn wd_action(&mut self, a: &Action) -> Predicate {
        match a {
            Action::Skip => Predicate::True,
            Action::Assignment { expressions, .. } => self.wd_expressions(expressions),
            Action::BecomesIn { set, .. } => self.wd_expression(set),
            Action::BecomesSuchThat {
                variables,
                predicate,
            } => {
                // ∀x'·wd(P) — the primed identifiers are bound, typed
                // like their unprimed counterparts.
                let decls: Vec<TypedIdentifier> = variables
                    .iter()
                    .map(|v| TypedIdentifier::untyped(format!("{v}'")))
                    .collect();
                self.env.push_scope();
                for v in variables {
                    if let Some(ty) = self.env.get(v).cloned() {
                        self.env.insert(format!("{v}'"), ty);
                    }
                }
                let body = self.wd_predicate(predicate);
                self.env.pop_scope();
                fb::forall(decls, body)
            }
            Action::FunctionOverride {
                arguments,
                expression,
                ..
            } => {
                // `f(x) ≔ E` is sugar for `f ≔ f  {x ↦ E}`: the WD is
                // that of the desugared right-hand side — children only,
                // no domain condition.
                let wd_args = self.wd_expressions(arguments);
                fb::land2(wd_args, self.wd_expression(expression))
            }
        }
    }

    // -----------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------

    fn wd_expressions(&mut self, items: &[Expression]) -> Predicate {
        let parts: Vec<Predicate> = items.iter().map(|e| self.wd_expression(e)).collect();
        fb::land(parts)
    }

    /// Rodin skips the domain condition for the built-in total functions
    /// (`KPRED`, `KSUCC`, and the generic `id`/`prj1`/`prj2`). These are
    /// reserved identifiers, so they always denote the built-in — the same
    /// shared membership set ([`rossi::builtins::is_reserved_relational_atom`])
    /// that [`crate::infer`]'s `reserved_atom` consults to type them ahead of
    /// the environment, keeping the two passes in agreement by construction.
    fn is_builtin_total(&self, function: &Expression) -> bool {
        match function {
            Expression::Identifier(name) => rossi::builtins::is_reserved_relational_atom(name),
            _ => false,
        }
    }

    /// Push a scope typing `decls` (explicit `x⦂T` annotation first,
    /// otherwise inferred from the typing shapes of `typing_pred` — the
    /// same source the enrichment pass uses), run `body`, pop.
    fn scoped<F>(
        &mut self,
        decls: &[TypedIdentifier],
        typing_pred: Option<&Predicate>,
        body: F,
    ) -> Predicate
    where
        F: FnOnce(&mut Self) -> Predicate,
    {
        self.env.push_scope();
        let names: Vec<&str> = decls.iter().map(|d| d.name.as_str()).collect();
        let mut inferred = std::collections::BTreeMap::new();
        if let Some(pred) = typing_pred {
            collect_binder_types(&self.env, pred, &names, &mut inferred);
        }
        for decl in decls {
            let ty = decl
                .type_expr
                .as_deref()
                .and_then(parse_type_from_expression)
                .or_else(|| inferred.get(&decl.name).cloned());
            match ty {
                // Type known: shadow the outer binding with it.
                Some(ty) => self.env.insert(decl.name.clone(), ty),
                // Type unknown: still shadow the outer binding, but mask it
                // to "undeclared" so a use of this binder is untypeable and
                // the formula fails-and-skips, rather than silently leaking
                // the outer declaration's type into the lemma.
                None => self.env.remove(&decl.name),
            }
        }
        let result = body(self);
        self.env.pop_scope();
        result
    }

    fn fail(&mut self, reason: &str) {
        if self.failed.is_none() {
            self.failed = Some(reason.to_string());
        }
    }
}

/// Flatten a same-operator ∧/∨ chain into its n-ary children, the way
/// Rodin's parser builds `AssociativePredicate` nodes. WD computation
/// must see the whole chain at once: pairwise processing yields
/// structurally different (wrong) lemmas.
fn flatten_chain<'a>(p: &'a Predicate, op: LogicalOp, out: &mut Vec<&'a Predicate>) {
    match p {
        Predicate::Logical {
            op: child_op,
            left,
            right,
        } if *child_op == op => {
            flatten_chain(left, op, out);
            flatten_chain(right, op, out);
        }
        _ => out.push(p),
    }
}

fn untyped(decls: &[TypedIdentifier]) -> Vec<TypedIdentifier> {
    decls
        .iter()
        .map(|d| TypedIdentifier::untyped(d.name.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Type;
    use rossi::parse_predicate_str;

    #[test]
    fn shadowing_binder_with_uninferable_type_skips_formula() {
        // Outer f : BOOL ↔ BOOL; the inner ∃f shadows it but its own type
        // can't be inferred from the body, so `f(1)` is untypeable and the
        // whole formula is flagged for skipping — it must not borrow the
        // outer declaration's type into the lemma.
        let mut env = TypeEnv::new();
        env.insert("f", Type::relation(Type::Boolean, Type::Boolean));
        let p = parse_predicate_str("∃f·f(1) = 2").unwrap();
        let mut c = WdComputer::new(env);
        let _ = c.wd_predicate(&p);
        assert!(c.failed.is_some());
    }
}
