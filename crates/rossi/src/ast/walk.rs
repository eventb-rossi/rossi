//! Scope-aware identifier traversal over Event-B formulas.
//!
//! A single walker threads a binder stack through expressions, predicates, and
//! actions and reports every identifier occurrence — reads, binder
//! declarations, action write targets, and predicate-application names — with
//! its source span and the binders in scope at that point. The static checker,
//! lint, and the language server all drive this one walker, so identifier
//! resolution has a single source of truth.
//!
//! The walker is policy-free: it reports occurrences verbatim (including the
//! apostrophe-suffixed after-state form `x'` produced by before-after
//! predicates) and leaves filtering, canonicalisation, and scope resolution to
//! the [`IdentVisitor`] implementation.
//!
//! Identifiers in a binder's type annotation (`∀x⦂T·…`) are reported in the
//! *enclosing* scope, before the binder is pushed, so a carrier set or constant
//! used only as a bound-variable type is attributed correctly.

use std::ops::ControlFlow;

use super::{
    Action, ActionKind, Expression, ExpressionKind, IdentPattern, Predicate, PredicateKind, Span,
    TypedIdentifier,
};

/// A binder in scope at an occurrence, innermost last.
#[derive(Debug, Clone)]
pub struct Binder {
    /// The bound variable's name.
    pub name: String,
    /// Source span of the binder's name token, if known.
    pub span: Option<Span>,
}

/// The syntactic role of a reported identifier occurrence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentRole {
    /// A read of an identifier (an `Expression::Identifier`).
    Usage,
    /// A binder declaration (quantifier / lambda / set-comprehension parameter).
    Binder,
    /// An action write target (assignment / `becomes` LHS, or the
    /// function-override name).
    WriteTarget,
    /// The name of a user-defined predicate application.
    PredicateCall,
}

/// One identifier occurrence reported by the walker.
pub struct IdentOccurrence<'a> {
    /// The identifier text, verbatim (a before-after read keeps its `'`).
    pub name: &'a str,
    /// Source span of this occurrence, if known.
    pub span: Option<Span>,
    /// What this occurrence is.
    pub role: IdentRole,
    /// The binders in scope here, innermost last. A binder declaration is *not*
    /// in its own snapshot.
    pub binders: &'a [Binder],
}

/// Invoked for every identifier occurrence the walker encounters. Returning
/// [`ControlFlow::Break`] aborts the rest of the traversal.
pub trait IdentVisitor {
    /// Visit one identifier occurrence.
    fn visit(&mut self, occ: IdentOccurrence<'_>) -> ControlFlow<()>;
}

fn emit<V: IdentVisitor>(
    v: &mut V,
    name: &str,
    span: Option<Span>,
    role: IdentRole,
    binders: &[Binder],
) -> ControlFlow<()> {
    v.visit(IdentOccurrence {
        name,
        span,
        role,
        binders,
    })
}

/// Walk a predicate, reporting every identifier occurrence. `binders` is the
/// stack of enclosing binders (empty at a top-level formula; seed it with event
/// parameters to scope guards / actions correctly).
pub fn walk_predicate<V: IdentVisitor>(
    p: &Predicate,
    binders: &mut Vec<Binder>,
    v: &mut V,
) -> ControlFlow<()> {
    match &p.kind {
        PredicateKind::True | PredicateKind::False => ControlFlow::Continue(()),
        PredicateKind::Comparison { left, right, .. } => {
            walk_expression(left, binders, v)?;
            walk_expression(right, binders, v)
        }
        PredicateKind::Not(inner) => walk_predicate(inner, binders, v),
        PredicateKind::Logical { left, right, .. } => {
            walk_predicate(left, binders, v)?;
            walk_predicate(right, binders, v)
        }
        PredicateKind::Quantified {
            identifiers,
            predicate,
            ..
        } => {
            binder_decls(identifiers, binders, v)?;
            with_binders(binders, binder_frame(identifiers), |binders| {
                walk_predicate(predicate, binders, v)
            })
        }
        PredicateKind::Application {
            function,
            arguments,
        } => {
            emit(
                v,
                &function.name,
                function.span,
                IdentRole::PredicateCall,
                binders,
            )?;
            for a in arguments {
                walk_expression(a, binders, v)?;
            }
            ControlFlow::Continue(())
        }
        PredicateKind::BuiltinApplication { arguments, .. } => {
            for a in arguments {
                walk_expression(a, binders, v)?;
            }
            ControlFlow::Continue(())
        }
    }
}

/// Walk an expression, reporting every identifier occurrence.
pub fn walk_expression<V: IdentVisitor>(
    e: &Expression,
    binders: &mut Vec<Binder>,
    v: &mut V,
) -> ControlFlow<()> {
    match &e.kind {
        ExpressionKind::Identifier(n) => emit(v, n, e.span, IdentRole::Usage, binders),
        ExpressionKind::Integer(_)
        | ExpressionKind::True
        | ExpressionKind::False
        | ExpressionKind::EmptySet
        | ExpressionKind::Naturals
        | ExpressionKind::Naturals1
        | ExpressionKind::Integers
        | ExpressionKind::BoolType
        | ExpressionKind::StringLiteral(_) => ControlFlow::Continue(()),
        ExpressionKind::Binary { left, right, .. } => {
            walk_expression(left, binders, v)?;
            walk_expression(right, binders, v)
        }
        ExpressionKind::Unary { operand, .. } => walk_expression(operand, binders, v),
        ExpressionKind::FunctionApplication {
            function,
            arguments,
        } => {
            walk_expression(function, binders, v)?;
            for a in arguments {
                walk_expression(a, binders, v)?;
            }
            ControlFlow::Continue(())
        }
        ExpressionKind::BuiltinApplication { arguments, .. } => {
            for a in arguments {
                walk_expression(a, binders, v)?;
            }
            ControlFlow::Continue(())
        }
        ExpressionKind::SetEnumeration(items) => {
            for a in items {
                walk_expression(a, binders, v)?;
            }
            ControlFlow::Continue(())
        }
        ExpressionKind::Bool(p) => walk_predicate(p, binders, v),
        ExpressionKind::RelationalImage { relation, set } => {
            walk_expression(relation, binders, v)?;
            walk_expression(set, binders, v)
        }
        ExpressionKind::IfThenElse {
            condition,
            then_expr,
            else_expr,
        } => {
            walk_predicate(condition, binders, v)?;
            walk_expression(then_expr, binders, v)?;
            walk_expression(else_expr, binders, v)
        }
        ExpressionKind::SetComprehension {
            identifiers,
            predicate,
            expression,
        } => {
            binder_decls(identifiers, binders, v)?;
            with_binders(binders, binder_frame(identifiers), |binders| {
                walk_predicate(predicate, binders, v)?;
                if let Some(e) = expression {
                    walk_expression(e, binders, v)?;
                }
                ControlFlow::Continue(())
            })
        }
        ExpressionKind::SetBuilder {
            member_expression,
            predicate,
        } => {
            walk_expression(member_expression, binders, v)?;
            walk_predicate(predicate, binders, v)
        }
        ExpressionKind::Lambda {
            pattern,
            predicate,
            expression,
        } => {
            pattern_decls(pattern, binders, v)?;
            let mut frame = Vec::new();
            pattern_frame(pattern, &mut frame);
            with_binders(binders, frame, |binders| {
                walk_predicate(predicate, binders, v)?;
                walk_expression(expression, binders, v)
            })
        }
        ExpressionKind::QuantifiedUnion {
            identifiers,
            predicate,
            expression,
        }
        | ExpressionKind::QuantifiedInter {
            identifiers,
            predicate,
            expression,
        } => {
            binder_decls(identifiers, binders, v)?;
            with_binders(binders, binder_frame(identifiers), |binders| {
                walk_predicate(predicate, binders, v)?;
                walk_expression(expression, binders, v)
            })
        }
    }
}

/// Walk an action, reporting its write targets (as [`IdentRole::WriteTarget`])
/// and every identifier read on its right-hand side.
pub fn walk_action<V: IdentVisitor>(
    a: &Action,
    binders: &mut Vec<Binder>,
    v: &mut V,
) -> ControlFlow<()> {
    match &a.kind {
        ActionKind::Skip => ControlFlow::Continue(()),
        ActionKind::Assignment {
            variables,
            expressions,
        } => {
            write_targets(variables, binders, v)?;
            for e in expressions {
                walk_expression(e, binders, v)?;
            }
            ControlFlow::Continue(())
        }
        ActionKind::BecomesIn { variables, set } => {
            write_targets(variables, binders, v)?;
            walk_expression(set, binders, v)
        }
        ActionKind::BecomesSuchThat {
            variables,
            predicate,
        } => {
            write_targets(variables, binders, v)?;
            walk_predicate(predicate, binders, v)
        }
        ActionKind::FunctionOverride {
            function,
            arguments,
            expression,
        } => {
            emit(
                v,
                &function.name,
                function.span,
                IdentRole::WriteTarget,
                binders,
            )?;
            for a in arguments {
                walk_expression(a, binders, v)?;
            }
            walk_expression(expression, binders, v)
        }
    }
}

fn write_targets<V: IdentVisitor>(
    variables: &[super::Ident],
    binders: &[Binder],
    v: &mut V,
) -> ControlFlow<()> {
    for var in variables {
        emit(v, &var.name, var.span, IdentRole::WriteTarget, binders)?;
    }
    ControlFlow::Continue(())
}

/// Report each binder declaration (in the enclosing scope) and walk its type
/// annotation, also in the enclosing scope.
fn binder_decls<V: IdentVisitor>(
    idents: &[TypedIdentifier],
    binders: &mut Vec<Binder>,
    v: &mut V,
) -> ControlFlow<()> {
    for ti in idents {
        emit(v, &ti.name, ti.span, IdentRole::Binder, binders)?;
        if let Some(t) = &ti.type_expr {
            walk_expression(t, binders, v)?;
        }
    }
    ControlFlow::Continue(())
}

fn binder_frame(idents: &[TypedIdentifier]) -> Vec<Binder> {
    idents
        .iter()
        .map(|ti| Binder {
            name: ti.name.clone(),
            span: ti.span,
        })
        .collect()
}

/// Report each leaf binder of a lambda pattern (enclosing scope) and walk its
/// type annotation.
fn pattern_decls<V: IdentVisitor>(
    pattern: &IdentPattern,
    binders: &mut Vec<Binder>,
    v: &mut V,
) -> ControlFlow<()> {
    match pattern {
        IdentPattern::Identifier(ti) => {
            emit(v, &ti.name, ti.span, IdentRole::Binder, binders)?;
            if let Some(t) = &ti.type_expr {
                walk_expression(t, binders, v)?;
            }
            ControlFlow::Continue(())
        }
        IdentPattern::Maplet(l, r) => {
            pattern_decls(l, binders, v)?;
            pattern_decls(r, binders, v)
        }
    }
}

fn pattern_frame(pattern: &IdentPattern, out: &mut Vec<Binder>) {
    match pattern {
        IdentPattern::Identifier(ti) => out.push(Binder {
            name: ti.name.clone(),
            span: ti.span,
        }),
        IdentPattern::Maplet(l, r) => {
            pattern_frame(l, out);
            pattern_frame(r, out);
        }
    }
}

/// Scope guard: push `frame` onto `binders`, run `body`, then truncate back.
fn with_binders<F>(binders: &mut Vec<Binder>, frame: Vec<Binder>, body: F) -> ControlFlow<()>
where
    F: FnOnce(&mut Vec<Binder>) -> ControlFlow<()>,
{
    let depth = binders.len();
    binders.extend(frame);
    let r = body(binders);
    binders.truncate(depth);
    r
}
