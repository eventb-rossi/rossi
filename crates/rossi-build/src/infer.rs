//! Type inference used by the context and machine static checker.
//!
//! The dispatch is intentionally pragmatic: we cover the predicate /
//! expression shapes Rodin's `.bcc`/`.bcm` files actually use to type
//! constants, variables, and event parameters. Anything still unhandled
//! falls through and surfaces as a diagnostic, matching Rodin's "drop
//! and continue" behaviour.
//!
//! Typing axioms understood for [`infer_constant_from_predicate`]:
//!
//! - `c ∈ S`                            S : ℙ(T)        ⇒ c : T
//! - `c ⊆ S`                            S : ℙ(T)        ⇒ c : ℙ(T)
//! - `c = expr`                         expr typeable    ⇒ c : typeof(expr)
//! - `c ∈ S₁ × S₂`                     both typeable    ⇒ c : T₁ × T₂
//! - `partition(S, {a₁}, {a₂}, …)`     S : ℙ(T)        ⇒ each aᵢ : T
//! - `partition(c, {a}, {b}, …)`       any aᵢ : T       ⇒ c : ℙ(T)
//! - `f(c) = e` / `f(c) ∈ S` / similar  f : ℙ(α × β)    ⇒ c : α
//! - any of the above nested under `∧`, `∨`, `⇒`, `⇔`, or `∀ x · …`
//!   (quantifier-bound variables don't shadow the constant)
//!
//! Expression typing in [`type_of_expression`] additionally covers
//! relation operators (`◁`, `▷`, `⩤`, `⩥`, `⊕`, `⊗`, `∥`, `;`, `∘`),
//! relational image `r[A]`, `bool(P)`, type ascription `e ⦂ T`,
//! `if … then … else …`, lambda, set comprehension, set builder, and
//! quantified union / intersection.

use std::collections::BTreeMap;

use rossi::ast::TypedIdentifier;
use rossi::ast::expression::{BinaryOp, BuiltinFunction, IdentPattern, UnaryOp};
use rossi::ast::predicate::{BuiltinPredicate, ComparisonOp, LogicalOp};
use rossi::{Expression, Predicate};

use crate::ast_util::left_assoc_maplet;
use crate::type_env::TypeEnv;
use crate::types::Type;

/// Derive the type of an expression given a type environment.
///
/// Returns `None` when the expression cannot be typed with the information
/// in `env` (typically because it references an untyped identifier).
pub fn type_of_expression(env: &TypeEnv, expr: &Expression) -> Option<Type> {
    match expr {
        Expression::Integer(_) => Some(Type::Integer),
        Expression::True | Expression::False => Some(Type::Boolean),
        Expression::Integers | Expression::Naturals | Expression::Naturals1 => {
            Some(Type::pow(Type::Integer))
        }
        Expression::BoolType => Some(Type::pow(Type::Boolean)),
        Expression::Identifier(name) => env.get(name).cloned(),
        Expression::EmptySet => None, // needs a type annotation / context
        Expression::Unary { op, operand } => {
            let inner = type_of_expression(env, operand)?;
            match op {
                UnaryOp::Minus => Some(Type::Integer),
                UnaryOp::PowerSet | UnaryOp::PowerSet1 => {
                    // POW(X) : ℙ(ℙ(elem))
                    match inner {
                        Type::PowerSet(t) => Some(Type::pow(Type::pow(*t))),
                        _ => None,
                    }
                }
                UnaryOp::Domain => match inner {
                    Type::PowerSet(inner) => match *inner {
                        Type::Product(l, _) => Some(Type::pow(*l)),
                        _ => None,
                    },
                    _ => None,
                },
                UnaryOp::Range => match inner {
                    Type::PowerSet(inner) => match *inner {
                        Type::Product(_, r) => Some(Type::pow(*r)),
                        _ => None,
                    },
                    _ => None,
                },
                UnaryOp::Inverse => match inner {
                    Type::PowerSet(inner) => match *inner {
                        Type::Product(l, r) => Some(Type::pow(Type::prod(*r, *l))),
                        _ => None,
                    },
                    _ => None,
                },
            }
        }
        Expression::SetEnumeration(items) => {
            // `{e₁, e₂, …}` has type ℙ(T) where T is the common element
            // type. Untyped EmptySet items contribute no constraint —
            // they pick up whatever the typed siblings establish (so
            // `{abstract_selectedAirplane, ∅}` types as
            // `ℙ(typeof(abstract_selectedAirplane))`).
            let mut ty: Option<Type> = None;
            for it in items {
                let t = match type_of_expression(env, it) {
                    Some(t) => t,
                    None if matches!(it, Expression::EmptySet) => continue,
                    None => return None,
                };
                if let Some(prev) = &ty {
                    if *prev != t {
                        return None;
                    }
                } else {
                    ty = Some(t);
                }
            }
            ty.map(Type::pow)
        }
        Expression::Binary { op, left, right } => match op {
            BinaryOp::Add
            | BinaryOp::Subtract
            | BinaryOp::Multiply
            | BinaryOp::Divide
            | BinaryOp::Modulo
            | BinaryOp::Exponent => Some(Type::Integer),
            BinaryOp::Range => Some(Type::pow(Type::Integer)),
            // Relation-preserving binary ops: result has the same type as
            // whichever side is a relation (`ℙ(α×β)`).
            // - `Union`/`Intersection`/`Difference`/`Overwrite`: either side
            //   is a relation; pick the typed one.
            // - `DomainRestriction (◁)`/`DomainSubtraction (⩤)`: relation is
            //   on the right.
            // - `RangeRestriction (▷)`/`RangeSubtraction (⩥)`: relation is
            //   on the left.
            BinaryOp::Union | BinaryOp::Intersection | BinaryOp::Difference => {
                type_of_expression(env, left).or_else(|| type_of_expression(env, right))
            }
            BinaryOp::Overwrite => {
                type_of_expression(env, left).or_else(|| type_of_expression(env, right))
            }
            BinaryOp::DomainRestriction | BinaryOp::DomainSubtraction => {
                type_of_expression(env, right).or_else(|| type_of_expression(env, left))
            }
            BinaryOp::RangeRestriction | BinaryOp::RangeSubtraction => {
                type_of_expression(env, left).or_else(|| type_of_expression(env, right))
            }
            // Forward / backward composition.
            // `r ; s` (forward, Semicolon): r:ℙ(α×β), s:ℙ(β×γ) ⇒ ℙ(α×γ).
            // `s ∘ r` (backward, Composition): s:ℙ(β×γ), r:ℙ(α×β) ⇒ ℙ(α×γ).
            BinaryOp::Semicolon => {
                let (lt, rt) = (
                    type_of_expression(env, left)?,
                    type_of_expression(env, right)?,
                );
                let (Type::PowerSet(lp), Type::PowerSet(rp)) = (lt, rt) else {
                    return None;
                };
                let (Type::Product(la, _), Type::Product(_, rb)) = (*lp, *rp) else {
                    return None;
                };
                Some(Type::pow(Type::prod(*la, *rb)))
            }
            BinaryOp::Composition => {
                let (lt, rt) = (
                    type_of_expression(env, left)?,
                    type_of_expression(env, right)?,
                );
                let (Type::PowerSet(lp), Type::PowerSet(rp)) = (lt, rt) else {
                    return None;
                };
                let (Type::Product(_, lb), Type::Product(ra, _)) = (*lp, *rp) else {
                    return None;
                };
                Some(Type::pow(Type::prod(*ra, *lb)))
            }
            // `r ⊗ s` (DirectProduct): r:ℙ(α×β), s:ℙ(α×γ) ⇒ ℙ(α×(β×γ)).
            BinaryOp::DirectProduct => {
                let (lt, rt) = (
                    type_of_expression(env, left)?,
                    type_of_expression(env, right)?,
                );
                let (Type::PowerSet(lp), Type::PowerSet(rp)) = (lt, rt) else {
                    return None;
                };
                let (Type::Product(la, lb), Type::Product(_, rb)) = (*lp, *rp) else {
                    return None;
                };
                Some(Type::pow(Type::prod(*la, Type::prod(*lb, *rb))))
            }
            // `r ∥ s` (ParallelProduct): r:ℙ(α×β), s:ℙ(γ×δ) ⇒ ℙ((α×γ)×(β×δ)).
            BinaryOp::ParallelProduct => {
                let (lt, rt) = (
                    type_of_expression(env, left)?,
                    type_of_expression(env, right)?,
                );
                let (Type::PowerSet(lp), Type::PowerSet(rp)) = (lt, rt) else {
                    return None;
                };
                let (Type::Product(la, lb), Type::Product(ra, rb)) = (*lp, *rp) else {
                    return None;
                };
                Some(Type::pow(Type::prod(
                    Type::prod(*la, *ra),
                    Type::prod(*lb, *rb),
                )))
            }
            BinaryOp::CartesianProduct => {
                let lt = type_of_expression(env, left)?;
                let rt = type_of_expression(env, right)?;
                match (lt, rt) {
                    (Type::PowerSet(l), Type::PowerSet(r)) => Some(Type::pow(Type::prod(*l, *r))),
                    _ => None,
                }
            }
            BinaryOp::Maplet => {
                let lt = type_of_expression(env, left)?;
                let rt = type_of_expression(env, right)?;
                Some(Type::prod(lt, rt))
            }
            // Relation / function type constructors: `S ↔ T`, `S → T`, etc.
            // all produce `ℙ(ℙ(S×T))` (a set of relations of S to T).
            BinaryOp::Relation
            | BinaryOp::TotalRelation
            | BinaryOp::SurjectiveRelation
            | BinaryOp::TotalSurjectiveRelation
            | BinaryOp::TotalFunction
            | BinaryOp::PartialFunction
            | BinaryOp::TotalInjection
            | BinaryOp::PartialInjection
            | BinaryOp::TotalSurjection
            | BinaryOp::PartialSurjection
            | BinaryOp::Bijection => {
                let lt = type_of_expression(env, left)?;
                let rt = type_of_expression(env, right)?;
                match (lt, rt) {
                    (Type::PowerSet(l), Type::PowerSet(r)) => {
                        Some(Type::pow(Type::pow(Type::prod(*l, *r))))
                    }
                    _ => None,
                }
            }
            // `e ⦂ T` — type ascription. The RHS is itself a type
            // expression (`ℤ`, `ℙ(USERS)`, `T × U`); interpret it as a
            // [`Type`] rather than as a set value.
            BinaryOp::OfType => parse_type_from_expression(right),
        },
        Expression::FunctionApplication {
            function,
            arguments: _,
        } => {
            // `f(x)` (or curried `f(a, b)` ≡ `f(a ↦ b)`): when
            // `f : ℙ(α × β)`, the application has type `β`. We don't
            // typecheck the argument here — Rodin's well-definedness
            // pass owns that — but we do return the codomain so
            // dependent constants/parameters can pick it up.
            let fn_ty = type_of_expression(env, function)?;
            let Type::PowerSet(prod) = fn_ty else {
                return None;
            };
            let Type::Product(_, codomain) = *prod else {
                return None;
            };
            Some(*codomain)
        }
        Expression::BuiltinApplication {
            function,
            arguments,
        } => match function {
            // Cardinality / min / max of any set return integers.
            BuiltinFunction::Card | BuiltinFunction::Min | BuiltinFunction::Max => {
                Some(Type::Integer)
            }
            // id(S) : ℙ(T × T) when S : ℙ(T).
            BuiltinFunction::Id => {
                let arg_ty = type_of_expression(env, arguments.first()?)?;
                let Type::PowerSet(elem) = arg_ty else {
                    return None;
                };
                let elem_clone = (*elem).clone();
                Some(Type::pow(Type::prod(*elem, elem_clone)))
            }
            // prj1(r) : ℙ((α × β) × α) when r : ℙ(α × β).
            BuiltinFunction::Prj1 => {
                let arg_ty = type_of_expression(env, arguments.first()?)?;
                let Type::PowerSet(prod) = arg_ty else {
                    return None;
                };
                let Type::Product(l, r) = *prod else {
                    return None;
                };
                let l_clone = (*l).clone();
                Some(Type::pow(Type::prod(Type::prod(*l, *r), l_clone)))
            }
            // prj2(r) : ℙ((α × β) × β) when r : ℙ(α × β).
            BuiltinFunction::Prj2 => {
                let arg_ty = type_of_expression(env, arguments.first()?)?;
                let Type::PowerSet(prod) = arg_ty else {
                    return None;
                };
                let Type::Product(l, r) = *prod else {
                    return None;
                };
                let r_clone = (*r).clone();
                Some(Type::pow(Type::prod(Type::prod(*l, *r), r_clone)))
            }
        },
        // `r[A]` — relational image: `r : ℙ(α × β)` ⇒ `r[A] : ℙ(β)`.
        Expression::RelationalImage { relation, set: _ } => {
            let r = type_of_expression(env, relation)?;
            let Type::PowerSet(prod) = r else {
                return None;
            };
            let Type::Product(_, b) = *prod else {
                return None;
            };
            Some(Type::pow(*b))
        }
        // `bool(P)` — promotes a predicate to a Boolean value.
        Expression::Bool(_) => Some(Type::Boolean),
        // `if P then E1 else E2` — both branches share the same type.
        Expression::IfThenElse {
            condition: _,
            then_expr,
            else_expr,
        } => type_of_expression(env, then_expr).or_else(|| type_of_expression(env, else_expr)),
        // λ pattern · P ∣ E. Bind the pattern names from explicit type
        // ascriptions or from `P`, then return ℙ(dom × typeof(E)).
        Expression::Lambda {
            pattern,
            predicate,
            expression,
        } => {
            let names = pattern.identifiers();
            let mut local = env.clone();
            let bound = bind_names(&mut local, &names, &[], predicate);
            if !names.iter().all(|n| bound.contains_key(*n)) {
                return None;
            }
            let dom = pattern_to_type(pattern, &bound)?;
            let body_ty = type_of_expression(&local, expression)?;
            Some(Type::pow(Type::prod(dom, body_ty)))
        }
        // `{ x ⦂ T · P ∣ E }` (extended) and `{ x · P }` (basic). Bind
        // each binder from explicit `T` if present, else from `P`. Body
        // type is typeof(E) for the extended form, else the
        // left-associative maplet of the binders for the basic form.
        Expression::SetComprehension {
            identifiers,
            predicate,
            expression,
        } => {
            let names: Vec<&str> = identifiers.iter().map(|i| i.name.as_str()).collect();
            let mut local = env.clone();
            let bound = bind_names(&mut local, &names, identifiers, predicate);
            if !names.iter().all(|n| bound.contains_key(*n)) {
                return None;
            }
            let body_ty = match expression {
                Some(e) => type_of_expression(&local, e)?,
                None => binder_left_assoc_product(&names, &bound)?,
            };
            Some(Type::pow(body_ty))
        }
        // `{ E ∣ P }` — set builder. Bound identifiers are the free
        // identifiers of `E` not already in scope; bind them from `P`.
        Expression::SetBuilder {
            member_expression,
            predicate,
        } => {
            let mut free = Vec::new();
            collect_free_identifiers(member_expression, &mut free);
            let bound_names: Vec<&str> =
                free.iter().filter(|n| !env.contains(n)).copied().collect();
            let mut local = env.clone();
            let bound = bind_names(&mut local, &bound_names, &[], predicate);
            if !bound_names.iter().all(|n| bound.contains_key(*n)) {
                return None;
            }
            let body_ty = type_of_expression(&local, member_expression)?;
            Some(Type::pow(body_ty))
        }
        // `⋃ x ⦂ T · P ∣ E` and `⋂ x ⦂ T · P ∣ E`. Bind binders, then
        // return typeof(E) — the body must already be a set.
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
            let names: Vec<&str> = identifiers.iter().map(|i| i.name.as_str()).collect();
            let mut local = env.clone();
            let bound = bind_names(&mut local, &names, identifiers, predicate);
            if !names.iter().all(|n| bound.contains_key(*n)) {
                return None;
            }
            type_of_expression(&local, expression)
        }
        _ => None,
    }
}

/// Build a left-associative product type from a list of binder names,
/// looking each one up in the binder map.
fn binder_left_assoc_product(names: &[&str], bound: &BTreeMap<String, Type>) -> Option<Type> {
    let mut iter = names.iter();
    let mut acc = bound.get(*iter.next()?)?.clone();
    for n in iter {
        let t = bound.get(*n)?.clone();
        acc = Type::prod(acc, t);
    }
    Some(acc)
}

/// Resolve types for the named binders by consulting (in order):
/// 1. Explicit `⦂ T` ascriptions on each [`TypedIdentifier`] (when
///    `idents` is given).
/// 2. The predicate body, via [`collect_binder_types`].
///
/// On return, every successfully-typed binder is inserted into `local`
/// and reflected in the returned map. Names that cannot be typed are
/// simply absent — callers decide whether that's fatal.
fn bind_names(
    local: &mut TypeEnv,
    names: &[&str],
    idents: &[TypedIdentifier],
    predicate: &Predicate,
) -> BTreeMap<String, Type> {
    let mut found: BTreeMap<String, Type> = BTreeMap::new();
    // Pass 1: explicit `⦂ T`. The map of TypedIdentifiers may be empty
    // (Lambda / SetBuilder don't carry one).
    for ti in idents {
        if let Some(te) = &ti.type_expr
            && let Some(t) = parse_type_from_expression(te)
        {
            found.insert(ti.name.clone(), t.clone());
            local.insert(ti.name.clone(), t);
        }
    }
    // Pass 2: consult the predicate body for what's still missing.
    let missing: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| !found.contains_key(*n))
        .collect();
    if !missing.is_empty() {
        let mut from_pred: BTreeMap<String, Type> = BTreeMap::new();
        collect_binder_types(local, predicate, &missing, &mut from_pred);
        for (n, t) in from_pred {
            local.insert(n.clone(), t.clone());
            found.insert(n, t);
        }
    }
    found
}

/// Build a [`Type`] from an [`Expression`] interpreted *as a type*.
/// This is used by `e ⦂ T` ascriptions and explicit binder types in
/// quantifiers / set comprehensions, where the RHS expression encodes a
/// type literal: `ℤ`, `BOOL`, a carrier-set name, `ℙ(T)`, or `T × U`.
pub(crate) fn parse_type_from_expression(expr: &Expression) -> Option<Type> {
    match expr {
        Expression::Integers => Some(Type::Integer),
        Expression::BoolType => Some(Type::Boolean),
        Expression::Identifier(n) => Some(Type::GivenSet(n.clone())),
        Expression::Unary {
            op: UnaryOp::PowerSet | UnaryOp::PowerSet1,
            operand,
        } => Some(Type::pow(parse_type_from_expression(operand)?)),
        Expression::Binary {
            op: BinaryOp::CartesianProduct,
            left,
            right,
        } => Some(Type::prod(
            parse_type_from_expression(left)?,
            parse_type_from_expression(right)?,
        )),
        _ => None,
    }
}

/// Convert an [`IdentPattern`] into the matching domain [`Type`] using
/// a binder-name → type map. Returns `None` if any leaf isn't bound.
fn pattern_to_type(pat: &IdentPattern, bound: &BTreeMap<String, Type>) -> Option<Type> {
    match pat {
        IdentPattern::Identifier(t) => bound.get(t.name.as_str()).cloned(),
        IdentPattern::Maplet(left, right) => {
            let lt = pattern_to_type(left, bound)?;
            let rt = pattern_to_type(right, bound)?;
            Some(Type::prod(lt, rt))
        }
    }
}

/// Distribute an expected type across an [`IdentPattern`], returning
/// each binder's type. Inverse of [`pattern_to_type`]: maps `(Maplet l r,
/// Product L R)` into `{l: L, r: R}` recursively. Returns `None` if
/// the type's nesting doesn't match the pattern's (e.g., a `Maplet`
/// pattern against a non-`Product` type). Used by the enrich pass
/// (Group S) to lift a lambda's expected function-domain type into
/// its untyped binder leaves.
pub(crate) fn pattern_to_binder_types(
    pat: &IdentPattern,
    expected: &Type,
) -> Option<BTreeMap<String, Type>> {
    let mut out = BTreeMap::new();
    fill_pattern_binder_types(pat, expected, &mut out)?;
    Some(out)
}

fn fill_pattern_binder_types(
    pat: &IdentPattern,
    expected: &Type,
    out: &mut BTreeMap<String, Type>,
) -> Option<()> {
    match (pat, expected) {
        (IdentPattern::Identifier(t), _) => {
            out.insert(t.name.clone(), expected.clone());
            Some(())
        }
        (IdentPattern::Maplet(l, r), Type::Product(lt, rt)) => {
            fill_pattern_binder_types(l, lt, out)?;
            fill_pattern_binder_types(r, rt, out)?;
            Some(())
        }
        _ => None,
    }
}

/// Walk `pred` to find typing constraints on each name in `names` and
/// fill `out` (only adding entries that aren't already present).
///
/// Recognises the same shapes as [`infer_constant_from_predicate`]
/// (membership, maplet membership, equality), descends through `∧`,
/// and into universal/existential bodies. Bound variables from inner
/// quantifiers shadow same-named entries in `names` for that subtree.
pub(crate) fn collect_binder_types(
    env: &TypeEnv,
    pred: &Predicate,
    names: &[&str],
    out: &mut BTreeMap<String, Type>,
) {
    match pred {
        Predicate::Comparison {
            op: ComparisonOp::In | ComparisonOp::NotIn,
            left,
            right,
        } => {
            if let Some(name) = as_ident(left)
                && names.contains(&name)
                && !out.contains_key(name)
                && let Some(Type::PowerSet(elem)) = type_of_expression(env, right)
            {
                out.insert(name.to_string(), *elem);
            }
            if matches!(
                left,
                Expression::Binary {
                    op: BinaryOp::Maplet,
                    ..
                }
            ) && let Some(Type::PowerSet(pair)) = type_of_expression(env, right)
            {
                collect_from_maplet(left, &pair, names, out);
            }
        }
        Predicate::Comparison {
            op: ComparisonOp::Equal | ComparisonOp::NotEqual,
            left,
            right,
        } => {
            for (a, b) in [(left, right), (right, left)] {
                if let Expression::Identifier(n) = a
                    && names.contains(&n.as_str())
                    && !out.contains_key(n)
                    && let Some(t) = type_of_expression(env, b)
                {
                    out.insert(n.clone(), t);
                }
            }
        }
        Predicate::Logical {
            op: LogicalOp::And,
            left,
            right,
        } => {
            collect_binder_types(env, left, names, out);
            collect_binder_types(env, right, names, out);
        }
        // Implication: typing constraints in the antecedent carry over
        // to the binder. (Common shape: `∀x · x∈ℤ ⇒ P`.)
        Predicate::Logical {
            op: LogicalOp::Implies,
            left,
            ..
        } => {
            collect_binder_types(env, left, names, out);
        }
        // Equivalence: typing carries from either side. (Common shape:
        // `∀a,t · a ∈ policies(t) ⇔ a ∈ POLICYSETS` — the left
        // typing predicate types `a`, which the right side then uses to
        // type `POLICYSETS`.)
        Predicate::Logical {
            op: LogicalOp::Equivalent,
            left,
            right,
        } => {
            collect_binder_types(env, left, names, out);
            collect_binder_types(env, right, names, out);
        }
        Predicate::Quantified {
            identifiers,
            predicate: body,
            ..
        } => {
            // Bound variables shadow same-named outer binders in this
            // subtree; everything else is still fair game.
            let shadowed: std::collections::BTreeSet<&str> =
                identifiers.iter().map(|t| t.name.as_str()).collect();
            let unshadowed: Vec<&str> = names
                .iter()
                .copied()
                .filter(|n| !shadowed.contains(*n))
                .collect();
            if !unshadowed.is_empty() {
                collect_binder_types(env, body, &unshadowed, out);
            }
        }
        _ => {}
    }
}

fn collect_from_maplet(
    maplet: &Expression,
    pair: &Type,
    names: &[&str],
    out: &mut BTreeMap<String, Type>,
) {
    match (maplet, pair) {
        (
            Expression::Binary {
                op: BinaryOp::Maplet,
                left,
                right,
            },
            Type::Product(lty, rty),
        ) => {
            collect_from_maplet(left, lty, names, out);
            collect_from_maplet(right, rty, names, out);
        }
        (Expression::Identifier(n), _) if names.contains(&n.as_str()) => {
            out.entry(n.clone()).or_insert_with(|| pair.clone());
        }
        _ => {}
    }
}

/// Collect bare identifiers referenced in `expr`, in left-to-right
/// order, deduped. Used by the SetBuilder arm to figure out which names
/// are binders. Recurses through binary/unary/application/`bool(P)`/
/// `if … then … else …`/etc. but stops at lambda / set-comprehension /
/// set-builder / quantified-union / quantified-inter — those nodes'
/// internal binders shouldn't leak. Names bound by a quantifier inside
/// `bool(P)` or an `if-then-else` condition are also filtered out.
pub(crate) fn collect_free_identifiers<'a>(expr: &'a Expression, out: &mut Vec<&'a str>) {
    let mut shadow: Vec<&'a str> = Vec::new();
    collect_free_idents_expr(expr, &mut shadow, out);
}

fn collect_free_idents_expr<'a>(
    expr: &'a Expression,
    shadow: &mut Vec<&'a str>,
    out: &mut Vec<&'a str>,
) {
    match expr {
        Expression::Identifier(n)
            if !shadow.contains(&n.as_str()) && !out.contains(&n.as_str()) =>
        {
            out.push(n.as_str());
        }
        Expression::Binary { left, right, .. } => {
            collect_free_idents_expr(left, shadow, out);
            collect_free_idents_expr(right, shadow, out);
        }
        Expression::Unary { operand, .. } => collect_free_idents_expr(operand, shadow, out),
        Expression::SetEnumeration(items) => {
            for i in items {
                collect_free_idents_expr(i, shadow, out);
            }
        }
        Expression::FunctionApplication {
            function,
            arguments,
        } => {
            collect_free_idents_expr(function, shadow, out);
            for a in arguments {
                collect_free_idents_expr(a, shadow, out);
            }
        }
        Expression::BuiltinApplication { arguments, .. } => {
            for a in arguments {
                collect_free_idents_expr(a, shadow, out);
            }
        }
        Expression::RelationalImage { relation, set } => {
            collect_free_idents_expr(relation, shadow, out);
            collect_free_idents_expr(set, shadow, out);
        }
        Expression::Bool(p) => collect_free_idents_pred(p, shadow, out),
        Expression::IfThenElse {
            condition,
            then_expr,
            else_expr,
        } => {
            collect_free_idents_pred(condition, shadow, out);
            collect_free_idents_expr(then_expr, shadow, out);
            collect_free_idents_expr(else_expr, shadow, out);
        }
        _ => {}
    }
}

fn collect_free_idents_pred<'a>(
    pred: &'a Predicate,
    shadow: &mut Vec<&'a str>,
    out: &mut Vec<&'a str>,
) {
    match pred {
        Predicate::Comparison { left, right, .. } => {
            collect_free_idents_expr(left, shadow, out);
            collect_free_idents_expr(right, shadow, out);
        }
        Predicate::Logical { left, right, .. } => {
            collect_free_idents_pred(left, shadow, out);
            collect_free_idents_pred(right, shadow, out);
        }
        Predicate::Not(inner) => collect_free_idents_pred(inner, shadow, out),
        Predicate::Quantified {
            identifiers,
            predicate,
            ..
        } => {
            let prev = shadow.len();
            for ti in identifiers {
                shadow.push(ti.name.as_str());
            }
            collect_free_idents_pred(predicate, shadow, out);
            shadow.truncate(prev);
        }
        Predicate::Application { arguments, .. }
        | Predicate::BuiltinApplication { arguments, .. } => {
            for a in arguments {
                collect_free_idents_expr(a, shadow, out);
            }
        }
        Predicate::True | Predicate::False => {}
    }
}

/// Try to extract the type of an identifier from a single "typing axiom".
///
/// Returns the constant's inferred type if the predicate has the shape
/// `name ∈ Set`, `name ⊆ Set`, `name = expr`, or
/// `partition(S, …, {name}, …)`, where the relevant side is typeable
/// from `env`.
pub fn infer_constant_from_predicate(
    env: &TypeEnv,
    predicate: &Predicate,
    constant_name: &str,
) -> Option<Type> {
    match predicate {
        Predicate::Comparison { op, left, right } => match op {
            ComparisonOp::In | ComparisonOp::NotIn => {
                // `c ∈ S` and `c ∉ S` constrain `c` to S's element
                // type identically — negation doesn't change typing.
                if let Some(name) = as_ident(left)
                    && name == constant_name
                    && let Some(Type::PowerSet(elem)) = type_of_expression(env, right)
                {
                    return Some(*elem);
                }
                // Symmetric form: `expr ∈ c` types `c : ℙ(typeof(expr))`.
                // (Covers a corpus model's `a ∈ POLICYSETS` where `a : ACCESSES`
                // is a binder pre-typed by the quantifier descent below.)
                if let Some(name) = as_ident(right)
                    && name == constant_name
                    && let Some(t) = type_of_expression(env, left)
                {
                    return Some(Type::pow(t));
                }
                // `p ↦ v ∈ relation` where `relation : ℙ(T × U)`:
                // infer `p : T` if constant_name is p, `v : U` if v.
                // This generalises to nested maplets `p ↦ q ↦ v`.
                if let Expression::Binary {
                    op: BinaryOp::Maplet,
                    ..
                } = left
                    && let Some(Type::PowerSet(pair)) = type_of_expression(env, right)
                    && let Some(ty) = infer_from_maplet_pattern(left, &pair, constant_name)
                {
                    return Some(ty);
                }
                None
            }
            ComparisonOp::Subset
            | ComparisonOp::SubsetStrict
            | ComparisonOp::NotSubset
            | ComparisonOp::NotSubsetStrict => {
                let name = as_ident(left)?;
                if name != constant_name {
                    return None;
                }
                type_of_expression(env, right)
            }
            ComparisonOp::Equal | ComparisonOp::NotEqual => {
                // Both `c = expr` and `c ≠ expr` constrain `c` to
                // expr's type — negation doesn't change typing.
                infer_from_equality(env, left, right, constant_name)
            }
            // Integer ordering: `c < expr`, `c > expr`, `c ≤ expr`, `c ≥
            // expr` (and the symmetric forms with c on the right) all
            // demand both sides be ℤ. So if either side mentions
            // `constant_name` as a bare identifier, it's ℤ.
            // (Covers corpus shapes like `maa>1`, `mii>−10`.)
            ComparisonOp::LessThan
            | ComparisonOp::LessEqual
            | ComparisonOp::GreaterThan
            | ComparisonOp::GreaterEqual => {
                if matches!(left, Expression::Identifier(n) if n == constant_name)
                    || matches!(right, Expression::Identifier(n) if n == constant_name)
                {
                    Some(Type::Integer)
                } else {
                    None
                }
            }
        },
        Predicate::BuiltinApplication {
            predicate: BuiltinPredicate::Partition,
            arguments,
        } => infer_from_partition(env, arguments, constant_name),
        // Descend into any binary logical connective: either side may
        // carry a typing predicate for the constant, which is already
        // in scope and constrained whenever the predicate as a whole
        // holds. Motivating corpus shapes:
        //   - `∧` — `axm: P ∧ Q` declares whatever either conjunct declares
        //   - `⇔` — e.g. `∀a,t · a∈policies(t) ⇔ a∈POLICYSETS`
        //     (POLICYSETS typed via the LHS)
        //   - `∨` — a corpus model's
        //     `new_blockedTime = X ∨ new_blockedTime = Y`
        //   - `⇒` — e.g. `(A ⇒ p = {q}) ∧ (¬A ⇒ p = ∅)`
        //     (constant-typing dual of `collect_binder_types`'s
        //     antecedent-only rule — binders are introduced by `⇒`,
        //     constants aren't).
        Predicate::Logical {
            op: LogicalOp::And | LogicalOp::Or | LogicalOp::Implies | LogicalOp::Equivalent,
            left,
            right,
        } => infer_constant_from_predicate(env, left, constant_name)
            .or_else(|| infer_constant_from_predicate(env, right, constant_name)),
        // Descend through quantifier bodies. Bound variables that
        // happen to share the constant's name shadow it inside the
        // body, so we skip the descent in that case.
        //
        // Augment the env with each binder's inferred type before
        // recursing — typing predicates inside the body that mention
        // the constant on one side and a binder on the other (e.g.
        // `a ∈ POLICYSETS` where `a` is a quantifier-bound variable
        // with type `ACCESSES`) need the binder typed in scope.
        Predicate::Quantified {
            identifiers,
            predicate: body,
            ..
        } if !identifiers.iter().any(|t| t.name == constant_name) => {
            let names: Vec<&str> = identifiers.iter().map(|t| t.name.as_str()).collect();
            let mut binder_types: BTreeMap<String, Type> = BTreeMap::new();
            collect_binder_types(env, body, &names, &mut binder_types);
            if binder_types.is_empty() {
                infer_constant_from_predicate(env, body, constant_name)
            } else {
                let mut local = env.clone();
                for (n, t) in binder_types {
                    local.insert(n, t);
                }
                infer_constant_from_predicate(&local, body, constant_name)
            }
        }
        _ => None,
    }
    .or_else(|| infer_from_function_argument(env, predicate, constant_name))
}

/// Last-resort typing: scan `pred` for an [`Expression::FunctionApplication`] whose
/// function has a known relation type and one of whose arguments is
/// `constant_name`. Lift the constant's type from the function's
/// domain. Covers parameters that only appear as keys to known
/// functions (e.g. `lift_states(lift) = IDLE` types `lift : LIFTS`
/// when `lift_states : ℙ(LIFTS × STATES)`).
fn infer_from_function_argument(
    env: &TypeEnv,
    pred: &Predicate,
    constant_name: &str,
) -> Option<Type> {
    let mut found: Option<Type> = None;
    walk_pred_for_arg(env, pred, constant_name, &mut found);
    found
}

fn walk_pred_for_arg(env: &TypeEnv, p: &Predicate, target: &str, found: &mut Option<Type>) {
    if found.is_some() {
        return;
    }
    match p {
        Predicate::Comparison { left, right, .. } => {
            walk_expr_for_arg(env, left, target, found);
            walk_expr_for_arg(env, right, target, found);
        }
        Predicate::Logical { left, right, .. } => {
            walk_pred_for_arg(env, left, target, found);
            walk_pred_for_arg(env, right, target, found);
        }
        Predicate::Not(inner) => walk_pred_for_arg(env, inner, target, found),
        Predicate::Quantified {
            identifiers,
            predicate: body,
            ..
        } => {
            // Quantifier-bound names shadow `target` in the body.
            if identifiers.iter().any(|t| t.name == target) {
                return;
            }
            walk_pred_for_arg(env, body, target, found);
        }
        Predicate::Application { arguments, .. }
        | Predicate::BuiltinApplication { arguments, .. } => {
            for a in arguments {
                walk_expr_for_arg(env, a, target, found);
            }
        }
        Predicate::True | Predicate::False => {}
    }
}

fn walk_expr_for_arg(env: &TypeEnv, e: &Expression, target: &str, found: &mut Option<Type>) {
    if found.is_some() {
        return;
    }
    match e {
        Expression::FunctionApplication {
            function,
            arguments,
        } => {
            walk_expr_for_arg(env, function, target, found);
            if found.is_some() {
                return;
            }
            if let Some(Type::PowerSet(prod)) = type_of_expression(env, function)
                && let Type::Product(dom, _cod) = *prod
            {
                let arg_expr = left_assoc_maplet(arguments);
                if let Some(t) = infer_from_maplet_pattern(&arg_expr, &dom, target) {
                    *found = Some(t);
                    return;
                }
            }
            for a in arguments {
                walk_expr_for_arg(env, a, target, found);
                if found.is_some() {
                    return;
                }
            }
        }
        Expression::Binary { left, right, .. } => {
            walk_expr_for_arg(env, left, target, found);
            if found.is_some() {
                return;
            }
            walk_expr_for_arg(env, right, target, found);
        }
        Expression::Unary { operand, .. } => walk_expr_for_arg(env, operand, target, found),
        Expression::SetEnumeration(items) => {
            for i in items {
                walk_expr_for_arg(env, i, target, found);
                if found.is_some() {
                    return;
                }
            }
        }
        Expression::BuiltinApplication { arguments, .. } => {
            for a in arguments {
                walk_expr_for_arg(env, a, target, found);
                if found.is_some() {
                    return;
                }
            }
        }
        Expression::RelationalImage { relation, set } => {
            walk_expr_for_arg(env, relation, target, found);
            if found.is_some() {
                return;
            }
            walk_expr_for_arg(env, set, target, found);
        }
        Expression::Bool(p) => walk_pred_for_arg(env, p, target, found),
        Expression::IfThenElse {
            condition,
            then_expr,
            else_expr,
        } => {
            walk_pred_for_arg(env, condition, target, found);
            if found.is_some() {
                return;
            }
            walk_expr_for_arg(env, then_expr, target, found);
            if found.is_some() {
                return;
            }
            walk_expr_for_arg(env, else_expr, target, found);
        }
        _ => {}
    }
}

fn as_ident(e: &Expression) -> Option<&str> {
    match e {
        Expression::Identifier(n) => Some(n.as_str()),
        _ => None,
    }
}

/// `maplet = p ↦ q` against paired `ty = T × U` yields `p : T`,
/// `q : U`. Generalises to nested: `p ↦ q ↦ v` against `(T × U) × V`.
/// Returns the type of `constant_name` if it appears anywhere in the
/// pattern, else `None`.
fn infer_from_maplet_pattern(
    maplet: &Expression,
    pair: &Type,
    constant_name: &str,
) -> Option<Type> {
    match (maplet, pair) {
        (
            Expression::Binary {
                op: BinaryOp::Maplet,
                left,
                right,
            },
            Type::Product(lty, rty),
        ) => infer_from_maplet_pattern(left, lty, constant_name)
            .or_else(|| infer_from_maplet_pattern(right, rty, constant_name)),
        (Expression::Identifier(n), _) if n == constant_name => Some(pair.clone()),
        _ => None,
    }
}

/// Handle `lhs = rhs` typing:
/// - direct: `c = expr` with typeable expr ⇒ c : typeof(expr)
/// - reverse: `expr = c` with typeable expr ⇒ c : typeof(expr)
/// - maplet-eq: `(p ↦ q) = expr` where expr : T × U ⇒ p : T, q : U;
///   nested maplets decomposed against nested products. Mirrored.
/// - set-eq: `S = {c₁, c₂, …}` where S : ℙ(T) ⇒ each cᵢ : T
/// - set-eq reversed: `{c₁, c₂, …} = S` same as above
/// - set-op-eq: `e₁ op e₂ = expr` (or symmetric) with op ∈ {∪, ∩, ∖},
///   where the unknown appears as a bare atom anywhere in the
///   equality and the typed-up side resolves to a power-set type ⇒
///   unknown shares that type. `type_of_expression` already descends
///   `∪`/`∩`/`∖` chains for the first typeable operand, so the
///   unknown may sit beside the typed operand inside the chain.
fn infer_from_equality(
    env: &TypeEnv,
    lhs: &Expression,
    rhs: &Expression,
    constant_name: &str,
) -> Option<Type> {
    // Direct: c = expr
    if let Expression::Identifier(n) = lhs
        && n == constant_name
        && let Some(t) = type_of_expression(env, rhs)
    {
        return Some(t);
    }
    if let Expression::Identifier(n) = rhs
        && n == constant_name
        && let Some(t) = type_of_expression(env, lhs)
    {
        return Some(t);
    }
    // Maplet-equality: `(p ↦ q) = expr` where typeof(expr) = T × U.
    let try_maplet_eq = |maplet: &Expression, other: &Expression| -> Option<Type> {
        if !matches!(
            maplet,
            Expression::Binary {
                op: BinaryOp::Maplet,
                ..
            }
        ) {
            return None;
        }
        let pair = type_of_expression(env, other)?;
        infer_from_maplet_pattern(maplet, &pair, constant_name)
    };
    if let Some(t) = try_maplet_eq(lhs, rhs).or_else(|| try_maplet_eq(rhs, lhs)) {
        return Some(t);
    }
    // Set-equality: `S = {e₁, …}`. Either side may be the typed Set.
    let try_set_eq = |set: &Expression, enum_: &Expression| -> Option<Type> {
        let Expression::SetEnumeration(items) = enum_ else {
            return None;
        };
        if !items
            .iter()
            .any(|e| matches!(e, Expression::Identifier(n) if n == constant_name))
        {
            return None;
        }
        match type_of_expression(env, set)? {
            Type::PowerSet(elem) => Some(*elem),
            _ => None,
        }
    };
    if let Some(t) = try_set_eq(lhs, rhs).or_else(|| try_set_eq(rhs, lhs)) {
        return Some(t);
    }
    // Set-op equality: `e₁ op e₂ = expr` with op ∈ {∪, ∩, ∖}.
    // `type_of_expression`'s built-in descent already returns any
    // typeable operand's type for these ops; the unknown only needs
    // to be reachable as a bare leaf on either side.
    let try_set_op_eq = |side: &Expression, other: &Expression| -> Option<Type> {
        if !contains_unknown_atom(side, constant_name)
            && !contains_unknown_atom(other, constant_name)
        {
            return None;
        }
        let ty = type_of_expression(env, side)?;
        matches!(ty, Type::PowerSet(_)).then_some(ty)
    };
    try_set_op_eq(lhs, rhs).or_else(|| try_set_op_eq(rhs, lhs))
}

/// True when `expr` contains `constant_name` as a bare leaf identifier
/// reachable through any combination of `Union` / `Intersection` /
/// `Difference` binary nodes. Other operators (e.g., maplets, function
/// applications) terminate the descent — we don't try to invert them.
fn contains_unknown_atom(expr: &Expression, constant_name: &str) -> bool {
    match expr {
        Expression::Identifier(n) => n == constant_name,
        Expression::Binary {
            op: BinaryOp::Union | BinaryOp::Intersection | BinaryOp::Difference,
            left,
            right,
        } => {
            contains_unknown_atom(left, constant_name)
                || contains_unknown_atom(right, constant_name)
        }
        _ => false,
    }
}

/// Extract the type of `constant_name` from a `partition(S, …)` call.
///
/// Let S : ℙ(T). Then each subsequent argument is a subset of S:
/// - `{e₁, e₂, …}` — a set enumeration. Each element that is an
///   identifier `x` named `constant_name` has type T (singleton contents).
/// - `X` — a bare identifier. Treated as a subset of S: X : ℙ(T).
///
/// These two forms coexist; Rodin's `partition(S, {a}, {b})` declares
/// `a, b : T`, while `partition(S, A, B)` declares `A, B : ℙ(T)`.
fn infer_from_partition(
    env: &TypeEnv,
    arguments: &[Expression],
    constant_name: &str,
) -> Option<Type> {
    let (head, tail) = arguments.split_first()?;
    // Path 1: head's type is known; type unknowns appearing in the
    // tail (as singletons or as bare subset names).
    if let Some(Type::PowerSet(elem)) = type_of_expression(env, head) {
        for arg in tail {
            match arg {
                Expression::SetEnumeration(items) => {
                    for item in items {
                        if matches!(item, Expression::Identifier(n) if n == constant_name) {
                            return Some(*elem.clone());
                        }
                    }
                }
                Expression::Identifier(n) if n == constant_name => {
                    return Some(Type::pow(*elem.clone()));
                }
                _ => {}
            }
        }
    }
    // Path 2: head IS the unknown — type it from any tail entry whose
    // type is known. Singletons `{x}` give `head : ℙ(typeof(x))`; bare
    // subsets `X : ℙ(T)` give `head : ℙ(T)`.
    if matches!(head, Expression::Identifier(n) if n == constant_name) {
        for arg in tail {
            match arg {
                Expression::SetEnumeration(items) => {
                    for item in items {
                        if let Some(t) = type_of_expression(env, item) {
                            return Some(Type::pow(t));
                        }
                    }
                }
                _ => {
                    if let Some(t) = type_of_expression(env, arg) {
                        return Some(t);
                    }
                }
            }
        }
    }
    None
}

/// Pump a TypeEnv to a fixed point over a list of (constant_name, predicates_to_consider)
/// using [`infer_constant_from_predicate`].
///
/// `seeds` is the initial env (already populated with carrier sets).
/// `constants` is the list of constant names that still need typing.
/// `axioms` is the flat list of all axiom predicates visible in the current
/// context scope.
///
/// Returns the final env and the list of constants that remained untyped.
pub fn infer_constants<'a>(
    env: &mut TypeEnv,
    constants: &'a [String],
    axioms: &[Predicate],
) -> Vec<&'a str> {
    let mut remaining: Vec<&str> = constants
        .iter()
        .map(String::as_str)
        .filter(|c| !env.contains(c))
        .collect();

    loop {
        let mut progress = false;
        remaining.retain(|c| {
            for ax in axioms {
                if let Some(ty) = infer_constant_from_predicate(env, ax, c) {
                    env.insert(*c, ty);
                    progress = true;
                    return false; // drop from remaining
                }
            }
            true
        });
        if !progress {
            break;
        }
    }
    remaining
}

#[cfg(test)]
mod tests {
    use super::*;
    use rossi::parse_predicate_str;

    fn env_with(pairs: &[(&str, Type)]) -> TypeEnv {
        let mut env = TypeEnv::new();
        for (n, t) in pairs {
            env.insert(*n, t.clone());
        }
        env
    }

    #[test]
    fn literal_types() {
        let env = TypeEnv::new();
        assert_eq!(
            type_of_expression(&env, &Expression::Integer(42)),
            Some(Type::Integer)
        );
        assert_eq!(
            type_of_expression(&env, &Expression::True),
            Some(Type::Boolean)
        );
        assert_eq!(
            type_of_expression(&env, &Expression::Integers),
            Some(Type::pow(Type::Integer))
        );
    }

    #[test]
    fn identifier_lookup() {
        let env = env_with(&[("n", Type::Integer)]);
        assert_eq!(
            type_of_expression(&env, &Expression::Identifier("n".into())),
            Some(Type::Integer)
        );
    }

    #[test]
    fn infer_const_in_carrier_set() {
        let mut env = TypeEnv::new();
        env.add_carrier_set("USERS");
        let p = parse_predicate_str("alice ∈ USERS").unwrap();
        let ty = infer_constant_from_predicate(&env, &p, "alice");
        assert_eq!(ty, Some(Type::GivenSet("USERS".into())));
    }

    #[test]
    fn infer_const_subset_of_carrier_set() {
        let mut env = TypeEnv::new();
        env.add_carrier_set("USERS");
        let p = parse_predicate_str("admins ⊆ USERS").unwrap();
        let ty = infer_constant_from_predicate(&env, &p, "admins");
        assert_eq!(ty, Some(Type::pow(Type::GivenSet("USERS".into()))));
    }

    #[test]
    fn infer_const_equal_to_integer() {
        let env = TypeEnv::new();
        let p = parse_predicate_str("max = 100").unwrap();
        let ty = infer_constant_from_predicate(&env, &p, "max");
        assert_eq!(ty, Some(Type::Integer));
    }

    #[test]
    fn infer_const_equal_to_integers_set() {
        let env = TypeEnv::new();
        let p = parse_predicate_str("Z = ℤ").unwrap();
        let ty = infer_constant_from_predicate(&env, &p, "Z");
        assert_eq!(ty, Some(Type::pow(Type::Integer)));
    }

    // ===== type_of_expression: builtin / function-application arms =====

    fn parse_expr(s: &str) -> Expression {
        rossi::parse_expression_str(s).expect("valid expression")
    }

    #[test]
    fn builtin_card_min_max_are_integer() {
        let mut env = TypeEnv::new();
        env.add_carrier_set("USERS");
        env.insert("admins", Type::pow(Type::GivenSet("USERS".into())));
        assert_eq!(
            type_of_expression(&env, &parse_expr("card(admins)")),
            Some(Type::Integer)
        );
        assert_eq!(
            type_of_expression(&env, &parse_expr("min(0 ‥ 10)")),
            Some(Type::Integer)
        );
        assert_eq!(
            type_of_expression(&env, &parse_expr("max(0 ‥ 10)")),
            Some(Type::Integer)
        );
    }

    #[test]
    fn function_application_returns_codomain() {
        // floor : ℙ(CABINS × FLOORS), cabin : CABINS ⇒ floor(cabin) : FLOORS.
        let mut env = TypeEnv::new();
        env.add_carrier_set("CABINS");
        env.add_carrier_set("FLOORS");
        env.insert(
            "floor",
            Type::pow(Type::prod(
                Type::GivenSet("CABINS".into()),
                Type::GivenSet("FLOORS".into()),
            )),
        );
        env.insert("cabin", Type::GivenSet("CABINS".into()));
        assert_eq!(
            type_of_expression(&env, &parse_expr("floor(cabin)")),
            Some(Type::GivenSet("FLOORS".into()))
        );
    }

    #[test]
    fn equality_with_min_typed_constant() {
        // `f = min(floors)` should give `f : ℤ` because min returns Integer.
        let mut env = TypeEnv::new();
        env.insert("floors", Type::pow(Type::Integer));
        let p = parse_predicate_str("f = min(floors)").unwrap();
        assert_eq!(
            infer_constant_from_predicate(&env, &p, "f"),
            Some(Type::Integer)
        );
    }

    #[test]
    fn equality_propagation_through_union() {
        // `register : ℙ(USER), register = in ∪ out` ⇒ `in : ℙ(USER)`,
        // `out : ℙ(USER)`.
        let mut env = TypeEnv::new();
        env.add_carrier_set("USER");
        env.insert("register", Type::pow(Type::GivenSet("USER".into())));
        let p = parse_predicate_str("register = in ∪ out").unwrap();
        assert_eq!(
            infer_constant_from_predicate(&env, &p, "in"),
            Some(Type::pow(Type::GivenSet("USER".into())))
        );
        assert_eq!(
            infer_constant_from_predicate(&env, &p, "out"),
            Some(Type::pow(Type::GivenSet("USER".into())))
        );
    }

    #[test]
    fn fixed_point_infers_multiple_constants() {
        let mut env = TypeEnv::new();
        env.add_carrier_set("USERS");
        let axioms = vec![
            parse_predicate_str("n = 100").unwrap(),
            parse_predicate_str("alice ∈ USERS").unwrap(),
            parse_predicate_str("admins ⊆ USERS").unwrap(),
        ];
        let consts = vec!["n".to_string(), "alice".to_string(), "admins".to_string()];
        let unresolved = infer_constants(&mut env, &consts, &axioms);
        assert!(unresolved.is_empty());
        assert_eq!(env.get("n"), Some(&Type::Integer));
        assert_eq!(env.get("alice"), Some(&Type::GivenSet("USERS".into())));
        assert_eq!(
            env.get("admins"),
            Some(&Type::pow(Type::GivenSet("USERS".into())))
        );
    }

    // ===== relation-preserving binary ops =====

    #[test]
    fn range_restriction_preserves_relation_type() {
        // `checked : ℙ(ROOM × GUEST)` ⇒ `checked ▷ {gst}` has the same
        // type as `checked`. From a real-world corpus model.
        let mut env = TypeEnv::new();
        env.add_carrier_set("ROOM");
        env.add_carrier_set("GUEST");
        env.insert(
            "checked",
            Type::pow(Type::prod(
                Type::GivenSet("ROOM".into()),
                Type::GivenSet("GUEST".into()),
            )),
        );
        env.insert("gst", Type::GivenSet("GUEST".into()));
        let ty = type_of_expression(&env, &parse_expr("checked ▷ {gst}"));
        assert_eq!(
            ty,
            Some(Type::pow(Type::prod(
                Type::GivenSet("ROOM".into()),
                Type::GivenSet("GUEST".into()),
            )))
        );
    }

    #[test]
    fn dom_of_range_restriction() {
        // `result = dom(checked ▷ {gst})` types `result : ℙ(ROOM)`.
        let mut env = TypeEnv::new();
        env.add_carrier_set("ROOM");
        env.add_carrier_set("GUEST");
        env.insert(
            "checked",
            Type::pow(Type::prod(
                Type::GivenSet("ROOM".into()),
                Type::GivenSet("GUEST".into()),
            )),
        );
        env.insert("gst", Type::GivenSet("GUEST".into()));
        let p = parse_predicate_str("result = dom(checked ▷ {gst})").unwrap();
        assert_eq!(
            infer_constant_from_predicate(&env, &p, "result"),
            Some(Type::pow(Type::GivenSet("ROOM".into())))
        );
    }

    #[test]
    fn forward_composition_resolves_inner_arrow() {
        // `r : ℙ(α × β)`, `s : ℙ(β × γ)` ⇒ `r ; s : ℙ(α × γ)`.
        let mut env = TypeEnv::new();
        env.add_carrier_set("A");
        env.add_carrier_set("B");
        env.add_carrier_set("C");
        env.insert(
            "r",
            Type::pow(Type::prod(
                Type::GivenSet("A".into()),
                Type::GivenSet("B".into()),
            )),
        );
        env.insert(
            "s",
            Type::pow(Type::prod(
                Type::GivenSet("B".into()),
                Type::GivenSet("C".into()),
            )),
        );
        assert_eq!(
            type_of_expression(&env, &parse_expr("r ; s")),
            Some(Type::pow(Type::prod(
                Type::GivenSet("A".into()),
                Type::GivenSet("C".into()),
            )))
        );
    }

    #[test]
    fn relational_image_yields_powerset_codomain() {
        // `lift_states[lifts] : ℙ(STATES)` when
        // `lift_states : ℙ(LIFTS × STATES)`.
        let mut env = TypeEnv::new();
        env.add_carrier_set("LIFTS");
        env.add_carrier_set("STATES");
        env.insert(
            "lift_states",
            Type::pow(Type::prod(
                Type::GivenSet("LIFTS".into()),
                Type::GivenSet("STATES".into()),
            )),
        );
        env.insert("lifts", Type::pow(Type::GivenSet("LIFTS".into())));
        assert_eq!(
            type_of_expression(&env, &parse_expr("lift_states[lifts]")),
            Some(Type::pow(Type::GivenSet("STATES".into())))
        );
    }

    #[test]
    fn bool_predicate_yields_boolean() {
        let env = TypeEnv::new();
        assert_eq!(
            type_of_expression(&env, &parse_expr("bool(0 = 0)")),
            Some(Type::Boolean)
        );
    }

    #[test]
    fn of_type_ascription_takes_rhs() {
        // `∅ ⦂ ℙ(USERS)` has type ℙ(USERS) — RHS is the literal type.
        let env = TypeEnv::new();
        assert_eq!(
            type_of_expression(&env, &parse_expr("∅ ⦂ ℙ(USERS)")),
            Some(Type::pow(Type::GivenSet("USERS".into())))
        );
    }

    // ===== argument typing from function application =====

    #[test]
    fn parameter_typed_via_function_application_argument() {
        // Lift's activate_lift only constrains `lift` via
        // `lift_states(lift) = IDLE`. With `lift_states : ℙ(LIFTS × STATES)`
        // and `IDLE : STATES` in scope, we type `lift : LIFTS`.
        let mut env = TypeEnv::new();
        env.add_carrier_set("LIFTS");
        env.add_carrier_set("STATES");
        env.insert(
            "lift_states",
            Type::pow(Type::prod(
                Type::GivenSet("LIFTS".into()),
                Type::GivenSet("STATES".into()),
            )),
        );
        env.insert("IDLE", Type::GivenSet("STATES".into()));
        let p = parse_predicate_str("lift_states(lift) = IDLE").unwrap();
        assert_eq!(
            infer_constant_from_predicate(&env, &p, "lift"),
            Some(Type::GivenSet("LIFTS".into()))
        );
    }

    #[test]
    fn parameter_typed_via_curried_function_application() {
        // `lift_buttons_states(lift)(floor) = ACTIVE` types
        // `floor : FLOORS` when the outer function returns
        // `ℙ(FLOORS × BUTTON_STATES)`.
        let mut env = TypeEnv::new();
        env.add_carrier_set("LIFTS");
        env.add_carrier_set("FLOORS");
        env.add_carrier_set("BUTTON_STATES");
        let inner_fn = Type::pow(Type::prod(
            Type::GivenSet("FLOORS".into()),
            Type::GivenSet("BUTTON_STATES".into()),
        ));
        env.insert(
            "lift_buttons_states",
            Type::pow(Type::prod(Type::GivenSet("LIFTS".into()), inner_fn)),
        );
        env.insert("lift", Type::GivenSet("LIFTS".into()));
        env.insert("ACTIVE", Type::GivenSet("BUTTON_STATES".into()));
        let p = parse_predicate_str("lift_buttons_states(lift)(floor) = ACTIVE").unwrap();
        assert_eq!(
            infer_constant_from_predicate(&env, &p, "floor"),
            Some(Type::GivenSet("FLOORS".into()))
        );
    }

    // ===== partition with unknown head =====

    #[test]
    fn partition_types_unknown_head_set() {
        // `partition(roles, {userOrdRole}, {userAdmRole})` types `roles`
        // as `ℙ(USER)` when the singletons' element types are known.
        let mut env = TypeEnv::new();
        env.add_carrier_set("USER");
        env.insert("userOrdRole", Type::GivenSet("USER".into()));
        env.insert("userAdmRole", Type::GivenSet("USER".into()));
        let p = parse_predicate_str("partition(roles, {userOrdRole}, {userAdmRole})").unwrap();
        assert_eq!(
            infer_constant_from_predicate(&env, &p, "roles"),
            Some(Type::pow(Type::GivenSet("USER".into())))
        );
    }

    // ===== quantifier-body descent =====

    #[test]
    fn typing_axiom_inside_quantifier_body() {
        // `∀x · c ∈ USERS` (silly but well-formed) types `c : USERS`
        // because the typing constraint is inside the universal body.
        let mut env = TypeEnv::new();
        env.add_carrier_set("USERS");
        // Implication-consequent descent: `∀x · x ∈ ℕ ⇒ c ∈ USERS`
        // types `c : USERS` via the consequent.
        let p = parse_predicate_str("∀x · x ∈ ℕ ⇒ c ∈ USERS").unwrap();
        assert_eq!(
            infer_constant_from_predicate(&env, &p, "c"),
            Some(Type::GivenSet("USERS".into())),
        );
        // Direct typing inside a quantifier body — no implication.
        let p2 = parse_predicate_str("∀x · c ∈ USERS").unwrap();
        assert_eq!(
            infer_constant_from_predicate(&env, &p2, "c"),
            Some(Type::GivenSet("USERS".into()))
        );
    }

    #[test]
    fn quantifier_binder_shadows_outer_constant() {
        // `∀c · c ∈ USERS` does NOT type the outer `c`.
        let mut env = TypeEnv::new();
        env.add_carrier_set("USERS");
        let p = parse_predicate_str("∀c · c ∈ USERS").unwrap();
        assert_eq!(infer_constant_from_predicate(&env, &p, "c"), None);
    }

    #[test]
    fn parameter_typed_via_implication_pair_consequent() {
        // real-world guard shape: `(A ⇒ p = {q}) ∧ (¬A ⇒ p = ∅)` where the
        // sibling `q : PROCESSES` is already typed. The constant-typer
        // must descend each `⇒` consequent to discover `p : ℙ(PROCESSES)`.
        // Antecedents are degenerate by design — we exercise consequent
        // descent, not implication semantics.
        let mut env = TypeEnv::new();
        env.add_carrier_set("PROCESSES");
        env.insert("q", Type::GivenSet("PROCESSES".into()));
        let p = parse_predicate_str("(q = q ⇒ p = {q}) ∧ (q ≠ q ⇒ p = ∅)").unwrap();
        assert_eq!(
            infer_constant_from_predicate(&env, &p, "p"),
            Some(Type::pow(Type::GivenSet("PROCESSES".into()))),
        );
    }

    #[test]
    fn constant_typed_via_implication_antecedent() {
        // Symmetric direction: `(c ∈ USERS ⇒ P)` types `c` from the
        // antecedent. The descent visits both sides.
        let mut env = TypeEnv::new();
        env.add_carrier_set("USERS");
        let p = parse_predicate_str("c ∈ USERS ⇒ ⊤").unwrap();
        assert_eq!(
            infer_constant_from_predicate(&env, &p, "c"),
            Some(Type::GivenSet("USERS".into())),
        );
    }

    // ===== lambda / set comprehension / quantified union =====

    #[test]
    fn lambda_constant_yields_function_type() {
        // `f = λx · x ∈ USERS ∣ x` types `f : ℙ(USERS × USERS)`.
        let mut env = TypeEnv::new();
        env.add_carrier_set("USERS");
        let p = parse_predicate_str("f = (λx · x ∈ USERS ∣ x)").unwrap();
        assert_eq!(
            infer_constant_from_predicate(&env, &p, "f"),
            Some(Type::pow(Type::prod(
                Type::GivenSet("USERS".into()),
                Type::GivenSet("USERS".into()),
            )))
        );
    }

    #[test]
    fn setcomp_basic_form_yields_powerset_of_binders() {
        // `S = {x ∣ x ∈ USERS}` types `S : ℙ(USERS)`.
        let mut env = TypeEnv::new();
        env.add_carrier_set("USERS");
        let p = parse_predicate_str("S = {x ∣ x ∈ USERS}").unwrap();
        assert_eq!(
            infer_constant_from_predicate(&env, &p, "S"),
            Some(Type::pow(Type::GivenSet("USERS".into())))
        );
    }

    #[test]
    fn setbuilder_with_maplet_yields_relation_type() {
        // `direct = {x ↦ y ∣ x ∈ ROLES ∧ y = TRUE}` types `direct` as
        // `ℙ(ROLES × BOOL)` — the body is `x ↦ y` where `x : ROLES` and
        // `y : BOOL`.
        let mut env = TypeEnv::new();
        env.add_carrier_set("ROLES");
        let p = parse_predicate_str("direct = {x ↦ y ∣ x ∈ ROLES ∧ y = TRUE}").unwrap();
        assert_eq!(
            infer_constant_from_predicate(&env, &p, "direct"),
            Some(Type::pow(Type::prod(
                Type::GivenSet("ROLES".into()),
                Type::Boolean,
            )))
        );
    }

    #[test]
    fn parameter_typed_via_maplet_equality_against_function_application() {
        // real-world shape: `m ↦ t = f(port)` where
        // `f : ℙ(PORTS × (MESSAGES × ℤ))`. The maplet leaves take
        // their types from the function's codomain product.
        let mut env = TypeEnv::new();
        env.add_carrier_set("PORTS");
        env.add_carrier_set("MESSAGES");
        env.insert("port", Type::GivenSet("PORTS".into()));
        env.insert(
            "f",
            Type::pow(Type::prod(
                Type::GivenSet("PORTS".into()),
                Type::prod(Type::GivenSet("MESSAGES".into()), Type::Integer),
            )),
        );
        let p = parse_predicate_str("m ↦ t = f(port)").unwrap();
        assert_eq!(
            infer_constant_from_predicate(&env, &p, "m"),
            Some(Type::GivenSet("MESSAGES".into())),
        );
        assert_eq!(
            infer_constant_from_predicate(&env, &p, "t"),
            Some(Type::Integer),
        );
    }

    #[test]
    fn parameter_typed_via_maplet_equality_reversed() {
        // Same shape, equality reversed: `f(port) = m ↦ t`.
        let mut env = TypeEnv::new();
        env.add_carrier_set("PORTS");
        env.add_carrier_set("MESSAGES");
        env.insert("port", Type::GivenSet("PORTS".into()));
        env.insert(
            "f",
            Type::pow(Type::prod(
                Type::GivenSet("PORTS".into()),
                Type::prod(Type::GivenSet("MESSAGES".into()), Type::Integer),
            )),
        );
        let p = parse_predicate_str("f(port) = m ↦ t").unwrap();
        assert_eq!(
            infer_constant_from_predicate(&env, &p, "m"),
            Some(Type::GivenSet("MESSAGES".into())),
        );
    }

    #[test]
    fn parameter_typed_via_intersection_with_typed_sibling_operand() {
        // corpus shape: `vss_set ∩ ma[{tr}] = ∅`. The
        // intersection's right operand `ma[{tr}]` has known type
        // ℙ(VSS) — that pins `vss_set : ℙ(VSS)` even though neither
        // full side of the equality resolves on its own (LHS contains
        // the unknown, RHS is the polymorphic empty set).
        let mut env = TypeEnv::new();
        env.add_carrier_set("TRAIN");
        env.add_carrier_set("VSS");
        env.insert("tr", Type::GivenSet("TRAIN".into()));
        env.insert(
            "ma",
            Type::pow(Type::prod(
                Type::GivenSet("TRAIN".into()),
                Type::GivenSet("VSS".into()),
            )),
        );
        let p = parse_predicate_str("vss_set ∩ ma[{tr}] = ∅").unwrap();
        assert_eq!(
            infer_constant_from_predicate(&env, &p, "vss_set"),
            Some(Type::pow(Type::GivenSet("VSS".into()))),
        );
    }

    #[test]
    fn parameter_typed_via_union_with_typed_sibling_operand() {
        // Mirror direction: typed sibling on the left of the chain
        // and the equality reversed (`∅ = known ∪ unk`). Exercises
        // both the chain-resolver walking past `∪` and the symmetric
        // arm of `try_set_op_eq`.
        let mut env = TypeEnv::new();
        env.add_carrier_set("VSS");
        env.insert("known", Type::pow(Type::GivenSet("VSS".into())));
        let p = parse_predicate_str("∅ = known ∪ unk").unwrap();
        assert_eq!(
            infer_constant_from_predicate(&env, &p, "unk"),
            Some(Type::pow(Type::GivenSet("VSS".into()))),
        );
    }

    // ===== collect_free_identifiers =====

    #[test]
    fn collect_free_idents_through_bool() {
        // `bool(x ∈ S)` — both `x` and `S` are free.
        let e = parse_expr("bool(x ∈ S)");
        let mut out = Vec::new();
        collect_free_identifiers(&e, &mut out);
        assert!(out.contains(&"x"), "missing x in {out:?}");
        assert!(out.contains(&"S"), "missing S in {out:?}");
    }

    #[test]
    fn collect_free_idents_bool_quantifier_shadows() {
        // `bool(∀x · x ∈ y)` — `x` is locally bound, only `y` leaks.
        let e = parse_expr("bool(∀x · x ∈ y)");
        let mut out = Vec::new();
        collect_free_identifiers(&e, &mut out);
        assert!(out.contains(&"y"), "missing y in {out:?}");
        assert!(!out.contains(&"x"), "x must not leak: {out:?}");
    }

    #[test]
    fn collect_free_idents_through_if_then_else() {
        // `if c = c then a else b end` — all three identifiers free.
        let e = parse_expr("if c = c then a else b end");
        let mut out = Vec::new();
        collect_free_identifiers(&e, &mut out);
        for n in ["c", "a", "b"] {
            assert!(out.contains(&n), "missing {n} in {out:?}");
        }
    }

    #[test]
    fn set_builder_binder_typed_through_bool_member() {
        // `{ bool(x ∈ S) ∣ x ∈ S }` — `x` is the SetBuilder binder
        // (typed from the predicate as the element type of `S`).
        // The whole expression types as `ℙ(BOOL)`.
        let mut env = TypeEnv::new();
        env.add_carrier_set("S");
        let e = parse_expr("{ bool(x ∈ S) ∣ x ∈ S }");
        assert_eq!(type_of_expression(&env, &e), Some(Type::pow(Type::Boolean)),);
    }
}
