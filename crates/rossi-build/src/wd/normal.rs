//! AST normalizations Rodin applies around WD improvement.
//!
//! [`flatten`] ports the quantifier part of Rodin's `Formula.flatten()`:
//! directly-nested same-quantifier prefixes merge (`∀x·∀y·P` → `∀x,y·P`)
//! and declarations with no free occurrence in the body are dropped —
//! after improvement subsumes every use of a bound identifier, Rodin's
//! output loses the binder (`∀x·P` → `P` when `x` is unused).
//!
//! [`resolve_binders`] ports `QuantifiedUtil.resolveIdents`, the renaming
//! Rodin's `toString` applies to bound-identifier declarations: a
//! declaration whose name is already taken — by an enclosing binder or by
//! a free identifier of the quantified node — gets the first fresh
//! `name0`, `name1`, … (`∃b·∀x0·x0∈{y − x,…}⇒b≥x0` when `x` is bound
//! outside). Synthesized binders (the `$`-prefixed placeholders from
//! [`super::builder::bounded`]) resolve to their preferred name through
//! the same rule; `$` cannot occur in an Event-B identifier, so
//! placeholders never capture user names.

use std::collections::BTreeSet;

use rossi::ast::expression::IdentPattern;
use rossi::{Expression, Predicate, TypedIdentifier};

// ---------------------------------------------------------------------
// Free-name collection (name-based, shadow-aware)
// ---------------------------------------------------------------------
//
// This walk *descends into* every binder (quantifiers, lambdas,
// comprehensions, QUnion/QInter) with a shadowing stack, and covers
// predicates as well as expressions, so `free_names` answers "is this
// bound name actually referenced anywhere in the body?" — exactly what
// `flatten` and `resolve_binders` need to drop unused declarations and
// rename on capture.
//
// It is deliberately NOT the same analysis as
// `infer::collect_free_identifiers`, which *stops at* binders and is
// expression-only (it discovers the implicit binders of a SetBuilder).
// The two compute different things on purpose and must stay separate;
// the only point they meet is the `SetBuilder` arm below, which calls
// `collect_free_identifiers` to learn which names that node binds.

pub(crate) fn predicate_free_names(
    p: &Predicate,
    bound: &mut Vec<String>,
    out: &mut BTreeSet<String>,
) {
    match p {
        Predicate::True | Predicate::False => {}
        Predicate::Comparison { left, right, .. } => {
            expression_free_names(left, bound, out);
            expression_free_names(right, bound, out);
        }
        Predicate::Not(inner) => predicate_free_names(inner, bound, out),
        Predicate::Logical { left, right, .. } => {
            predicate_free_names(left, bound, out);
            predicate_free_names(right, bound, out);
        }
        Predicate::Quantified {
            identifiers,
            predicate,
            ..
        } => {
            let depth = bound.len();
            bound.extend(identifiers.iter().map(|i| i.name.clone()));
            predicate_free_names(predicate, bound, out);
            bound.truncate(depth);
        }
        Predicate::Application { arguments, .. }
        | Predicate::BuiltinApplication { arguments, .. } => {
            for arg in arguments {
                expression_free_names(arg, bound, out);
            }
        }
    }
}

pub(crate) fn expression_free_names(
    e: &Expression,
    bound: &mut Vec<String>,
    out: &mut BTreeSet<String>,
) {
    match e {
        Expression::Identifier(name) => {
            if !bound.iter().any(|b| b == name) {
                out.insert(name.clone());
            }
        }
        Expression::Integer(_)
        | Expression::True
        | Expression::False
        | Expression::EmptySet
        | Expression::Naturals
        | Expression::Naturals1
        | Expression::Integers
        | Expression::BoolType
        | Expression::StringLiteral(_) => {}
        Expression::SetEnumeration(items) => {
            for item in items {
                expression_free_names(item, bound, out);
            }
        }
        Expression::SetComprehension {
            identifiers,
            predicate,
            expression,
        } => {
            let depth = bound.len();
            bound.extend(identifiers.iter().map(|i| i.name.clone()));
            predicate_free_names(predicate, bound, out);
            if let Some(body) = expression {
                expression_free_names(body, bound, out);
            }
            bound.truncate(depth);
        }
        Expression::SetBuilder {
            member_expression,
            predicate,
        } => {
            // The implicit form binds every identifier of the member
            // expression (mirrors the WD computer).
            let mut names: Vec<&str> = Vec::new();
            crate::infer::collect_free_identifiers(member_expression, &mut names);
            let depth = bound.len();
            bound.extend(names.iter().map(|n| (*n).to_string()));
            predicate_free_names(predicate, bound, out);
            bound.truncate(depth);
        }
        Expression::RelationalImage { relation, set } => {
            expression_free_names(relation, bound, out);
            expression_free_names(set, bound, out);
        }
        Expression::QuantifiedUnion {
            identifiers,
            predicate,
            expression,
        }
        | Expression::QuantifiedInter {
            identifiers,
            predicate,
            expression,
        } => {
            let depth = bound.len();
            bound.extend(identifiers.iter().map(|i| i.name.clone()));
            predicate_free_names(predicate, bound, out);
            expression_free_names(expression, bound, out);
            bound.truncate(depth);
        }
        Expression::Lambda {
            pattern,
            predicate,
            expression,
        } => {
            let depth = bound.len();
            bound.extend(pattern.identifiers().into_iter().map(str::to_string));
            predicate_free_names(predicate, bound, out);
            expression_free_names(expression, bound, out);
            bound.truncate(depth);
        }
        Expression::Binary { left, right, .. } => {
            expression_free_names(left, bound, out);
            expression_free_names(right, bound, out);
        }
        Expression::Unary { operand, .. } => expression_free_names(operand, bound, out),
        Expression::FunctionApplication {
            function,
            arguments,
        } => {
            expression_free_names(function, bound, out);
            for arg in arguments {
                expression_free_names(arg, bound, out);
            }
        }
        Expression::BuiltinApplication { arguments, .. } => {
            for arg in arguments {
                expression_free_names(arg, bound, out);
            }
        }
        Expression::Bool(p) => predicate_free_names(p, bound, out),
        Expression::IfThenElse {
            condition,
            then_expr,
            else_expr,
        } => {
            predicate_free_names(condition, bound, out);
            expression_free_names(then_expr, bound, out);
            expression_free_names(else_expr, bound, out);
        }
    }
}

fn free_names(p: &Predicate) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    predicate_free_names(p, &mut Vec::new(), &mut out);
    out
}

// ---------------------------------------------------------------------
// flatten — quantifier merging and unused-declaration removal
// ---------------------------------------------------------------------

/// Port of the quantifier simplifications of Rodin's `Formula.flatten()`.
/// (The associative-chain merging part is moot on rossi's binary AST —
/// rendering and improvement both flatten chains structurally.)
pub fn flatten(p: Predicate) -> Predicate {
    match p {
        Predicate::True | Predicate::False => p,
        Predicate::Comparison { op, left, right } => Predicate::Comparison {
            op,
            left: flatten_expr(left),
            right: flatten_expr(right),
        },
        Predicate::Not(inner) => Predicate::Not(Box::new(flatten(*inner))),
        Predicate::Logical { op, left, right } => Predicate::Logical {
            op,
            left: Box::new(flatten(*left)),
            right: Box::new(flatten(*right)),
        },
        Predicate::Quantified {
            quantifier,
            mut identifiers,
            predicate,
        } => {
            let mut body = flatten(*predicate);
            // Merge directly-nested same-quantifier prefixes.
            while let Predicate::Quantified {
                quantifier: inner_q,
                identifiers: inner_ids,
                predicate: inner_body,
            } = body
            {
                if inner_q == quantifier {
                    identifiers.extend(inner_ids);
                    body = *inner_body;
                } else {
                    body = Predicate::Quantified {
                        quantifier: inner_q,
                        identifiers: inner_ids,
                        predicate: inner_body,
                    };
                    break;
                }
            }
            // Drop declarations with no bound occurrence in the body.
            // Merging same-quantifier prefixes can leave two binders with
            // the same name (`∀x·∀x·P` → decls `[x, x]`); a free `x` in the
            // body binds to the *innermost* (last) one only, so an earlier
            // shadowed duplicate is dead even though the name is free —
            // De Bruijn flatten drops it. Name-set membership alone keeps
            // both, diverging from Rodin (`∀x,x0·…`).
            let free = free_names(&body);
            let names: Vec<&str> = identifiers.iter().map(|d| d.name.as_str()).collect();
            let keep: Vec<bool> = (0..identifiers.len())
                .map(|i| free.contains(names[i]) && !names[i + 1..].contains(&names[i]))
                .collect();
            let mut keep = keep.into_iter();
            identifiers.retain(|_| keep.next().unwrap_or(false));
            if identifiers.is_empty() {
                body
            } else {
                Predicate::Quantified {
                    quantifier,
                    identifiers,
                    predicate: Box::new(body),
                }
            }
        }
        Predicate::Application {
            function,
            arguments,
        } => Predicate::Application {
            function,
            arguments: arguments.into_iter().map(flatten_expr).collect(),
        },
        Predicate::BuiltinApplication {
            predicate,
            arguments,
        } => Predicate::BuiltinApplication {
            predicate,
            arguments: arguments.into_iter().map(flatten_expr).collect(),
        },
    }
}

fn flatten_expr(e: Expression) -> Expression {
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
        | Expression::StringLiteral(_) => e,
        Expression::SetEnumeration(items) => {
            Expression::SetEnumeration(items.into_iter().map(flatten_expr).collect())
        }
        Expression::SetComprehension {
            identifiers,
            predicate,
            expression,
        } => Expression::SetComprehension {
            identifiers,
            predicate: Box::new(flatten(*predicate)),
            expression: expression.map(|b| Box::new(flatten_expr(*b))),
        },
        Expression::SetBuilder {
            member_expression,
            predicate,
        } => Expression::SetBuilder {
            member_expression: Box::new(flatten_expr(*member_expression)),
            predicate: Box::new(flatten(*predicate)),
        },
        Expression::RelationalImage { relation, set } => Expression::RelationalImage {
            relation: Box::new(flatten_expr(*relation)),
            set: Box::new(flatten_expr(*set)),
        },
        Expression::QuantifiedUnion {
            identifiers,
            predicate,
            expression,
        } => Expression::QuantifiedUnion {
            identifiers,
            predicate: Box::new(flatten(*predicate)),
            expression: Box::new(flatten_expr(*expression)),
        },
        Expression::QuantifiedInter {
            identifiers,
            predicate,
            expression,
        } => Expression::QuantifiedInter {
            identifiers,
            predicate: Box::new(flatten(*predicate)),
            expression: Box::new(flatten_expr(*expression)),
        },
        Expression::Lambda {
            pattern,
            predicate,
            expression,
        } => Expression::Lambda {
            pattern,
            predicate: Box::new(flatten(*predicate)),
            expression: Box::new(flatten_expr(*expression)),
        },
        Expression::Binary { op, left, right } => Expression::Binary {
            op,
            left: Box::new(flatten_expr(*left)),
            right: Box::new(flatten_expr(*right)),
        },
        Expression::Unary { op, operand } => Expression::Unary {
            op,
            operand: Box::new(flatten_expr(*operand)),
        },
        Expression::FunctionApplication {
            function,
            arguments,
        } => Expression::FunctionApplication {
            function: Box::new(flatten_expr(*function)),
            arguments: arguments.into_iter().map(flatten_expr).collect(),
        },
        Expression::BuiltinApplication {
            function,
            arguments,
        } => Expression::BuiltinApplication {
            function,
            arguments: arguments.into_iter().map(flatten_expr).collect(),
        },
        Expression::Bool(p) => Expression::Bool(Box::new(flatten(*p))),
        Expression::IfThenElse {
            condition,
            then_expr,
            else_expr,
        } => Expression::IfThenElse {
            condition: Box::new(flatten(*condition)),
            then_expr: Box::new(flatten_expr(*then_expr)),
            else_expr: Box::new(flatten_expr(*else_expr)),
        },
    }
}

// ---------------------------------------------------------------------
// BinderRewriter — capture-avoiding bound-identifier renaming
// ---------------------------------------------------------------------

/// Policy for choosing a bound identifier's replacement during a
/// [`BinderRewriter`] walk — the only thing that differs between Rodin's
/// `toString` collision resolution and the improver's positional-marker
/// renaming, which are otherwise the same traversal.
pub(crate) trait NamePolicy {
    /// Whether the walk must compute a binder node's free names before
    /// choosing replacements (collision resolution needs them; globally
    /// unique markers never clash, so they don't).
    const NEEDS_NODE_FREE: bool;
    /// Choose the replacement for `original`. `taken` reports whether a
    /// candidate already clashes with a free name of the node or an
    /// earlier sibling declaration.
    fn choose(&mut self, original: &str, taken: &dyn Fn(&str) -> bool) -> String;
}

/// Rodin `toString` collision resolution: keep each binder's preferred
/// name (a `$name` placeholder resolves to `name`), suffixing `name0`,
/// `name1`, … only on a real clash with the node's free names.
pub(crate) struct DisplayPolicy;

impl NamePolicy for DisplayPolicy {
    const NEEDS_NODE_FREE: bool = true;
    fn choose(&mut self, original: &str, taken: &dyn Fn(&str) -> bool) -> String {
        let preferred = original.strip_prefix('$').unwrap_or(original);
        let mut chosen = preferred.to_string();
        let mut i = 0usize;
        while taken(&chosen) {
            chosen = format!("{preferred}{i}");
            i += 1;
        }
        chosen
    }
}

/// Rename bound-identifier declarations the way Rodin's `toString`
/// resolves them: against the names *free in the quantified node* —
/// mere shadowing without capture keeps the name (`∃b·∀x·x∈{x·P ∣ x}…`
/// prints two `x` binders). Placeholders (`$name`) resolve to `name`
/// through the same collision rule.
pub fn resolve_binders(p: &Predicate) -> Predicate {
    BinderRewriter::new(DisplayPolicy).pred(p)
}

/// A capture-avoiding rewrite of bound identifiers, parameterized by a
/// [`NamePolicy`]. The single binder-scoped traversal shared by
/// [`resolve_binders`] and the improver's leaf normalization, so a scoping
/// fix can't land in one copy and silently miss the other.
pub(crate) struct BinderRewriter<P> {
    /// original name → replacement name, innermost binder last.
    scope: Vec<(String, String)>,
    policy: P,
}

impl<P: NamePolicy> BinderRewriter<P> {
    pub(crate) fn new(policy: P) -> Self {
        Self {
            scope: Vec::new(),
            policy,
        }
    }

    /// Seed the rewriter with an enclosing scope: the improver maps the
    /// tree-level binders to their positional slots before walking a leaf.
    pub(crate) fn with_scope(scope: Vec<(String, String)>, policy: P) -> Self {
        Self { scope, policy }
    }

    fn display<'a>(&'a self, name: &'a str) -> &'a str {
        self.scope
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map_or(name, |(_, d)| d.as_str())
    }

    /// Push replacements for one binder node's declarations, deferring the
    /// per-name choice to the policy. A referenced enclosing binder shows
    /// up in `node_free` through the scope mapping; an unreferenced one
    /// causes no collision — matching Rodin's `resolveIdents`, which only
    /// consults the names used in the subtree. Returns the scope checkpoint
    /// to truncate to afterwards.
    fn bind(
        &mut self,
        decls: &[String],
        node_free: &BTreeSet<String>,
    ) -> (usize, Vec<TypedIdentifier>) {
        let checkpoint = self.scope.len();
        let mut displays: Vec<TypedIdentifier> = Vec::with_capacity(decls.len());
        for name in decls {
            let chosen = {
                let taken = |cand: &str| {
                    node_free.contains(cand) || displays.iter().any(|d| d.name == cand)
                };
                self.policy.choose(name, &taken)
            };
            self.scope.push((name.clone(), chosen.clone()));
            displays.push(TypedIdentifier::untyped(chosen));
        }
        (checkpoint, displays)
    }

    fn unbind(&mut self, checkpoint: usize) {
        self.scope.truncate(checkpoint);
    }

    /// Free names of a binder node, mapped through the current scope to
    /// their replacement form. Empty when the policy renames to globally
    /// unique markers that can never collide.
    fn node_free(
        &self,
        bound: &[String],
        preds: &[&Predicate],
        exprs: &[&Expression],
    ) -> BTreeSet<String> {
        if !P::NEEDS_NODE_FREE {
            return BTreeSet::new();
        }
        let mut raw = BTreeSet::new();
        let mut bound_vec: Vec<String> = bound.to_vec();
        for p in preds {
            predicate_free_names(p, &mut bound_vec, &mut raw);
        }
        for e in exprs {
            expression_free_names(e, &mut bound_vec, &mut raw);
        }
        raw.iter().map(|n| self.display(n).to_string()).collect()
    }

    pub(crate) fn pred(&mut self, p: &Predicate) -> Predicate {
        match p {
            Predicate::True | Predicate::False => p.clone(),
            Predicate::Comparison { op, left, right } => Predicate::Comparison {
                op: *op,
                left: self.expr(left),
                right: self.expr(right),
            },
            Predicate::Not(inner) => Predicate::Not(Box::new(self.pred(inner))),
            Predicate::Logical { op, left, right } => Predicate::Logical {
                op: *op,
                left: Box::new(self.pred(left)),
                right: Box::new(self.pred(right)),
            },
            Predicate::Quantified {
                quantifier,
                identifiers,
                predicate,
            } => {
                let names: Vec<String> = identifiers.iter().map(|i| i.name.clone()).collect();
                let free = self.node_free(&names, &[predicate], &[]);
                let (checkpoint, displays) = self.bind(&names, &free);
                let body = self.pred(predicate);
                self.unbind(checkpoint);
                Predicate::Quantified {
                    quantifier: *quantifier,
                    identifiers: displays,
                    predicate: Box::new(body),
                }
            }
            Predicate::Application {
                function,
                arguments,
            } => Predicate::Application {
                function: function.clone(),
                arguments: arguments.iter().map(|a| self.expr(a)).collect(),
            },
            Predicate::BuiltinApplication {
                predicate,
                arguments,
            } => Predicate::BuiltinApplication {
                predicate: *predicate,
                arguments: arguments.iter().map(|a| self.expr(a)).collect(),
            },
        }
    }

    fn expr(&mut self, e: &Expression) -> Expression {
        match e {
            Expression::Identifier(name) => Expression::Identifier(self.display(name).to_string()),
            Expression::Integer(_)
            | Expression::True
            | Expression::False
            | Expression::EmptySet
            | Expression::Naturals
            | Expression::Naturals1
            | Expression::Integers
            | Expression::BoolType
            | Expression::StringLiteral(_) => e.clone(),
            Expression::SetEnumeration(items) => {
                Expression::SetEnumeration(items.iter().map(|i| self.expr(i)).collect())
            }
            Expression::SetComprehension {
                identifiers,
                predicate,
                expression,
            } => {
                let names: Vec<String> = identifiers.iter().map(|i| i.name.clone()).collect();
                let mut parts: Vec<&Expression> = Vec::new();
                if let Some(body) = expression {
                    parts.push(body);
                }
                let free = self.node_free(&names, &[predicate], &parts);
                let (checkpoint, displays) = self.bind(&names, &free);
                let p = self.pred(predicate);
                let body = expression.as_ref().map(|b| Box::new(self.expr(b)));
                self.unbind(checkpoint);
                Expression::SetComprehension {
                    identifiers: displays,
                    predicate: Box::new(p),
                    expression: body,
                }
            }
            Expression::SetBuilder {
                member_expression,
                predicate,
            } => {
                let mut raw: Vec<&str> = Vec::new();
                crate::infer::collect_free_identifiers(member_expression, &mut raw);
                let mut names: Vec<String> = Vec::new();
                for n in raw {
                    if names.iter().all(|m| m != n) {
                        names.push(n.to_string());
                    }
                }
                let free = self.node_free(&names, &[predicate], &[member_expression]);
                let (checkpoint, _) = self.bind(&names, &free);
                let member = self.expr(member_expression);
                let p = self.pred(predicate);
                self.unbind(checkpoint);
                Expression::SetBuilder {
                    member_expression: Box::new(member),
                    predicate: Box::new(p),
                }
            }
            Expression::RelationalImage { relation, set } => Expression::RelationalImage {
                relation: Box::new(self.expr(relation)),
                set: Box::new(self.expr(set)),
            },
            Expression::QuantifiedUnion {
                identifiers,
                predicate,
                expression,
            } => {
                let (ids, p, b) = self.quantified_expr(identifiers, predicate, expression);
                Expression::QuantifiedUnion {
                    identifiers: ids,
                    predicate: p,
                    expression: b,
                }
            }
            Expression::QuantifiedInter {
                identifiers,
                predicate,
                expression,
            } => {
                let (ids, p, b) = self.quantified_expr(identifiers, predicate, expression);
                Expression::QuantifiedInter {
                    identifiers: ids,
                    predicate: p,
                    expression: b,
                }
            }
            Expression::Lambda {
                pattern,
                predicate,
                expression,
            } => {
                let names: Vec<String> = pattern
                    .identifiers()
                    .into_iter()
                    .map(str::to_string)
                    .collect();
                let free = self.node_free(&names, &[predicate], &[expression]);
                let (checkpoint, _) = self.bind(&names, &free);
                let renamed = self.pattern(pattern);
                let p = self.pred(predicate);
                let body = self.expr(expression);
                self.unbind(checkpoint);
                Expression::Lambda {
                    pattern: renamed,
                    predicate: Box::new(p),
                    expression: Box::new(body),
                }
            }
            Expression::Binary { op, left, right } => Expression::Binary {
                op: *op,
                left: Box::new(self.expr(left)),
                right: Box::new(self.expr(right)),
            },
            Expression::Unary { op, operand } => Expression::Unary {
                op: *op,
                operand: Box::new(self.expr(operand)),
            },
            Expression::FunctionApplication {
                function,
                arguments,
            } => Expression::FunctionApplication {
                function: Box::new(self.expr(function)),
                arguments: arguments.iter().map(|a| self.expr(a)).collect(),
            },
            Expression::BuiltinApplication {
                function,
                arguments,
            } => Expression::BuiltinApplication {
                function: *function,
                arguments: arguments.iter().map(|a| self.expr(a)).collect(),
            },
            Expression::Bool(p) => Expression::Bool(Box::new(self.pred(p))),
            Expression::IfThenElse {
                condition,
                then_expr,
                else_expr,
            } => Expression::IfThenElse {
                condition: Box::new(self.pred(condition)),
                then_expr: Box::new(self.expr(then_expr)),
                else_expr: Box::new(self.expr(else_expr)),
            },
        }
    }

    fn quantified_expr(
        &mut self,
        identifiers: &[TypedIdentifier],
        predicate: &Predicate,
        expression: &Expression,
    ) -> (Vec<TypedIdentifier>, Box<Predicate>, Box<Expression>) {
        let names: Vec<String> = identifiers.iter().map(|i| i.name.clone()).collect();
        let free = self.node_free(&names, &[predicate], &[expression]);
        let (checkpoint, displays) = self.bind(&names, &free);
        let p = self.pred(predicate);
        let body = self.expr(expression);
        self.unbind(checkpoint);
        (displays, Box::new(p), Box::new(body))
    }

    fn pattern(&mut self, pattern: &IdentPattern) -> IdentPattern {
        match pattern {
            IdentPattern::Identifier(t) => IdentPattern::Identifier(TypedIdentifier::untyped(
                self.display(&t.name).to_string(),
            )),
            IdentPattern::Maplet(l, r) => {
                IdentPattern::Maplet(Box::new(self.pattern(l)), Box::new(self.pattern(r)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wd::render::render_predicate;
    use rossi::parse_predicate_str;

    #[test]
    fn flatten_drops_unused_decls() {
        let p = parse_predicate_str("∀x,y·x∈S").unwrap();
        assert_eq!(render_predicate(&flatten(p)), "∀x·x∈S");
        let q = parse_predicate_str("∀x·a∈S").unwrap();
        assert_eq!(render_predicate(&flatten(q)), "a∈S");
    }

    #[test]
    fn flatten_merges_nested_same_quantifier() {
        let p = parse_predicate_str("∀x·∀y·x ↦ y∈S").unwrap();
        assert_eq!(render_predicate(&flatten(p)), "∀x,y·x ↦ y∈S");
        // Different quantifiers stay nested.
        let q = parse_predicate_str("∀x·∃y·x ↦ y∈S").unwrap();
        assert_eq!(render_predicate(&flatten(q)), "∀x·∃y·x ↦ y∈S");
    }

    #[test]
    fn flatten_drops_shadowed_duplicate_binder() {
        // ∀x·∀x·x∈S merges to decls [x, x]; the body's x binds to the
        // inner one, so the outer shadowed x is dead and dropped — Rodin's
        // De Bruijn flatten yields `∀x·x∈S`, not a renamed `∀x,x0·…`.
        let p = parse_predicate_str("∀x·∀x·x∈S").unwrap();
        assert_eq!(render_predicate(&flatten(p)), "∀x·x∈S");
        let q = parse_predicate_str("∀x·∀x·∀x·x∈S").unwrap();
        assert_eq!(render_predicate(&flatten(q)), "∀x·x∈S");
    }

    #[test]
    fn placeholders_resolve_to_preferred_names() {
        let set = rossi::parse_expression_str("s").unwrap();
        let p = crate::wd::builder::bounded(set, false);
        assert_eq!(render_predicate(&resolve_binders(&p)), "∃b·∀x·x∈s⇒b≥x");
    }

    #[test]
    fn colliding_placeholder_gets_suffixed() {
        // bounded() inside ∀x — the synthesized x collides and becomes x0,
        // while occurrences of the outer x are untouched.
        let set = rossi::parse_expression_str("{y − x,x − y}").unwrap();
        let inner = crate::wd::builder::bounded(set, false);
        let outer = Predicate::quantified(
            rossi::ast::predicate::Quantifier::ForAll,
            vec![
                TypedIdentifier::untyped("x".into()),
                TypedIdentifier::untyped("y".into()),
            ],
            inner,
        );
        assert_eq!(
            render_predicate(&resolve_binders(&outer)),
            "∀x,y·∃b·∀x0·x0∈{y − x,x − y}⇒b≥x0"
        );
    }

    #[test]
    fn shadowing_without_capture_keeps_the_name() {
        // The inner binder never references the outer x, so Rodin keeps
        // both named x.
        let p = parse_predicate_str("∀x·x∈S∧(∀x·x∈T)").unwrap();
        assert_eq!(render_predicate(&resolve_binders(&p)), "∀x·x∈S∧(∀x·x∈T)");
    }
}
