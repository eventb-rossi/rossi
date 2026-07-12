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
//! relation operators (`◁`, `▷`, `⩤`, `⩥`, `<+`, `⊗`, `∥`, `;`, `∘`),
//! relational image `r[A]`, `bool(P)`, type ascription `e ⦂ T`,
//! lambda, set comprehension, set builder, and
//! quantified union / intersection.

use std::collections::BTreeMap;

use rossi::ast::TypedIdentifier;
use rossi::ast::expression::{AtomicBuiltinKind, BinaryOp, BuiltinFunction, IdentPattern, UnaryOp};
use rossi::ast::predicate::{BuiltinPredicate, ComparisonOp, LogicalOp};
use rossi::{Expression, ExpressionKind, Predicate, PredicateKind};

use crate::type_env::TypeEnv;
use crate::types::Type;

// ---------------------------------------------------------------------------
// Unification machinery (inference-only; never reaches the public `Type`).
// ---------------------------------------------------------------------------
//
// The synthesizer reads types off structurally, but the polymorphic atoms
// `id` / `prj1` / `prj2` / `∅` (Rodin's KID_GEN / KPRJ1_GEN / KPRJ2_GEN /
// EMPTYSET) carry a *type variable* that the surrounding context must solve.
// `ITy` is `Type` plus a `Var` leaf; `Unifier` solves the variables (mirroring
// Rodin's `TypeUnifier`); `ground` lowers a fully-solved `ITy` back to `Type`
// and drops anything still holding a free variable — exactly Rodin's treatment
// of an unsolved `TypeVariable`. None of this escapes `infer.rs`.

/// Inference-only partial type: [`Type`] extended with a unification
/// variable leaf. Built only inside [`synth`]; lowered to [`Type`] by
/// [`ground`] at the [`type_of_expression`] boundary.
#[derive(Clone, Debug, PartialEq)]
enum ITy {
    Boolean,
    Integer,
    GivenSet(String),
    PowerSet(Box<ITy>),
    Product(Box<ITy>, Box<ITy>),
    Var(u32),
}

impl ITy {
    fn pow(t: ITy) -> ITy {
        ITy::PowerSet(Box::new(t))
    }
    fn prod(l: ITy, r: ITy) -> ITy {
        ITy::Product(Box::new(l), Box::new(r))
    }
    /// `ℙ(l × r)` — Event-B's `l ↔ r`. Mirrors [`Type::relation`].
    fn relation(l: ITy, r: ITy) -> ITy {
        ITy::pow(ITy::prod(l, r))
    }
}

/// Lift a concrete [`Type`] into [`ITy`]. Introduces no variables.
impl From<&Type> for ITy {
    fn from(t: &Type) -> ITy {
        match t {
            Type::Boolean => ITy::Boolean,
            Type::Integer => ITy::Integer,
            Type::GivenSet(n) => ITy::GivenSet(n.clone()),
            Type::PowerSet(inner) => ITy::pow(ITy::from(inner.as_ref())),
            Type::Product(l, r) => ITy::prod(ITy::from(l.as_ref()), ITy::from(r.as_ref())),
        }
    }
}

/// A first-order unifier over [`ITy`], mirroring Rodin's `TypeUnifier`:
/// structural descent through `ℙ` / `×`, given-set equality by name,
/// ground `ℤ` / `BOOL`, variable binding with an occurs check.
struct Unifier {
    /// `slots[i]` is the binding for `Var(i)` (`None` while unsolved).
    slots: Vec<Option<ITy>>,
    /// One untyped identifier to type as a fresh variable rather than via
    /// `env` — Rodin's `getIdentType`, which mints a fresh `TypeVariable`
    /// for any free identifier absent from the type environment so the
    /// surrounding equations can solve it. `None` (the default) leaves
    /// `synth` behaving exactly as before, so `type_of_expression` and
    /// every existing caller are unaffected; only
    /// [`infer_ident_via_unification`] sets it.
    target: Option<(String, u32)>,
}

impl Unifier {
    fn new() -> Unifier {
        Unifier {
            slots: Vec::new(),
            target: None,
        }
    }

    /// The variable type standing in for `name`, if `name` is this
    /// unifier's [`Unifier::target`]. Holds no borrow of `target` across
    /// the `synth` identifier arm.
    fn target_var(&self, name: &str) -> Option<ITy> {
        match &self.target {
            Some((n, var)) if n == name => Some(ITy::Var(*var)),
            _ => None,
        }
    }

    /// Mint a fresh, unbound variable.
    fn fresh(&mut self) -> ITy {
        let id = self.slots.len() as u32;
        self.slots.push(None);
        ITy::Var(id)
    }

    /// Mint a fresh variable and return its slot id (for callers that
    /// [`ground`] the variable directly rather than threading the `ITy`).
    fn fresh_var(&mut self) -> u32 {
        let id = self.slots.len() as u32;
        self.slots.push(None);
        id
    }

    /// Apply the current substitution everywhere (Rodin's `solve`).
    fn resolve(&self, t: &ITy) -> ITy {
        match t {
            ITy::Var(i) => match &self.slots[*i as usize] {
                Some(bound) => self.resolve(bound),
                None => t.clone(),
            },
            ITy::PowerSet(inner) => ITy::pow(self.resolve(inner)),
            ITy::Product(l, r) => ITy::prod(self.resolve(l), self.resolve(r)),
            _ => t.clone(),
        }
    }

    /// Unify two partial types. `Err(())` on a clash or a circular
    /// binding (occurs check); callers turn that into a dropped type.
    fn unify(&mut self, a: &ITy, b: &ITy) -> Result<(), ()> {
        let a = self.resolve(a);
        let b = self.resolve(b);
        match (a, b) {
            (ITy::Var(i), ITy::Var(j)) if i == j => Ok(()),
            (ITy::Var(i), other) | (other, ITy::Var(i)) => {
                if self.occurs(i, &other) {
                    return Err(());
                }
                self.slots[i as usize] = Some(other);
                Ok(())
            }
            (ITy::PowerSet(c1), ITy::PowerSet(c2)) => self.unify(&c1, &c2),
            (ITy::Product(l1, r1), ITy::Product(l2, r2)) => {
                self.unify(&l1, &l2)?;
                self.unify(&r1, &r2)
            }
            (ITy::Integer, ITy::Integer) => Ok(()),
            (ITy::Boolean, ITy::Boolean) => Ok(()),
            (ITy::GivenSet(n1), ITy::GivenSet(n2)) if n1 == n2 => Ok(()),
            _ => Err(()),
        }
    }

    /// Does `Var(var)` occur in `t`? `t` is assumed already resolved.
    fn occurs(&self, var: u32, t: &ITy) -> bool {
        match t {
            ITy::Var(i) => *i == var,
            ITy::PowerSet(inner) => self.occurs(var, inner),
            ITy::Product(l, r) => self.occurs(var, l) || self.occurs(var, r),
            _ => false,
        }
    }

    /// Destructure (or constrain) `t` as a relation `ℙ(l × r)`, returning
    /// `(l, r)`. Replacement for [`Type::into_relation`]: when `t` is (or
    /// resolves to) a variable, fresh `l` / `r` are minted and bound into
    /// it, so a polymorphic atom flowing through a relation operator gets
    /// its variable pinned by the other operand.
    fn as_relation(&mut self, t: &ITy) -> Option<(ITy, ITy)> {
        match self.resolve(t) {
            ITy::PowerSet(inner) => match *inner {
                ITy::Product(l, r) => Some((*l, *r)),
                ITy::Var(_) => {
                    let l = self.fresh();
                    let r = self.fresh();
                    self.unify(&inner, &ITy::prod(l.clone(), r.clone())).ok()?;
                    Some((l, r))
                }
                _ => None,
            },
            v @ ITy::Var(_) => {
                let l = self.fresh();
                let r = self.fresh();
                self.unify(&v, &ITy::relation(l.clone(), r.clone())).ok()?;
                Some((l, r))
            }
            _ => None,
        }
    }
}

/// Lower a partial type to a concrete [`Type`]. Returns `None` if any
/// unification variable survives resolution — the constant then drops,
/// matching Rodin's treatment of an unsolved `TypeVariable`.
fn ground(u: &Unifier, t: &ITy) -> Option<Type> {
    match u.resolve(t) {
        ITy::Boolean => Some(Type::Boolean),
        ITy::Integer => Some(Type::Integer),
        ITy::GivenSet(n) => Some(Type::GivenSet(n)),
        ITy::PowerSet(inner) => Some(Type::pow(ground(u, &inner)?)),
        ITy::Product(l, r) => Some(Type::prod(ground(u, &l)?, ground(u, &r)?)),
        ITy::Var(_) => None,
    }
}

/// Derive the type of an expression given a type environment.
///
/// Returns `None` when the expression cannot be typed with the information
/// in `env` — either it references an untyped identifier, or a polymorphic
/// atom's type variable is left unconstrained by the surrounding context.
pub fn type_of_expression(env: &TypeEnv, expr: &Expression) -> Option<Type> {
    let mut u = Unifier::new();
    let t = synth(env, expr, &mut u)?;
    ground(&u, &t)
}

/// The type of a generic relational atom (`id`, `prj1`, `prj2`, `pred`,
/// `succ`). `succ`/`pred` are the monomorphic integer relations (Rodin's
/// KSUCC/KPRED, `ℙ(ℤ×ℤ)`); `id`/`prj1`/`prj2` are the generic atoms
/// (KID_GEN/KPRJ1_GEN/KPRJ2_GEN) and carry fresh type variables the surrounding
/// context must solve — if nothing solves them the expression keeps a free
/// variable and `ground` drops it, exactly as Rodin drops an unsolved
/// `TypeVariable`. Applying an atom (`prj1(x)`) is the `FunctionApplication`
/// arm: it synths this relation type and returns its codomain.
fn atomic_builtin_type(kind: AtomicBuiltinKind, u: &mut Unifier) -> ITy {
    use AtomicBuiltinKind as A;
    match kind {
        A::Succ | A::Pred => ITy::relation(ITy::Integer, ITy::Integer),
        A::Id => {
            let a = u.fresh();
            ITy::relation(a.clone(), a)
        }
        A::Prj1 => {
            let a = u.fresh();
            let b = u.fresh();
            ITy::relation(ITy::prod(a.clone(), b), a)
        }
        A::Prj2 => {
            let a = u.fresh();
            let b = u.fresh();
            ITy::relation(ITy::prod(a, b.clone()), b)
        }
    }
}

/// Synthesize both operands and destructure each as a relation `ℙ(·×·)`,
/// returning `((dom_l, ran_l), (dom_r, ran_r))`. The shared prologue of the
/// composition / product arms; each caller then unifies the components it
/// shares and builds its own result.
fn synth_two_relations(
    env: &TypeEnv,
    left: &Expression,
    right: &Expression,
    u: &mut Unifier,
) -> Option<((ITy, ITy), (ITy, ITy))> {
    let lt = synth(env, left, u)?;
    let lr = u.as_relation(&lt)?;
    let rt = synth(env, right, u)?;
    let rr = u.as_relation(&rt)?;
    Some((lr, rr))
}

/// Constrain one side of a relation (its domain or range) to the element type
/// of a restricting `set`. Used by `◁`/`▷`/`⩤`/`⩥` and relational image; a
/// `None` set (a sibling still resolving in the fixpoint) is simply skipped.
fn constrain_with_set(u: &mut Unifier, side: &ITy, set: Option<ITy>) {
    if let Some(set_t) = set {
        let elem = u.fresh();
        u.unify(&set_t, &ITy::pow(elem.clone())).ok();
        u.unify(side, &elem).ok();
    }
}

/// Combine two operands that must share a type: unify them when both type,
/// tolerate one still-unresolved side (returning the other), and drop only
/// when neither types. Used by `∪`/`∩`/`∖`/`<+`.
fn unify_or_either(u: &mut Unifier, a: Option<ITy>, b: Option<ITy>) -> Option<ITy> {
    match (a, b) {
        (Some(a), Some(b)) => {
            u.unify(&a, &b).ok()?;
            Some(a)
        }
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    }
}

/// Synthesize a partial type for `expr`, threading the unifier `u`. The
/// public [`type_of_expression`] wraps this and grounds the result. One
/// `Unifier` serves a whole top-level expression — type variables never
/// need to cross a top-level boundary (binder bodies and equality
/// propagation see concrete types by the time they read a sub-result).
fn synth(env: &TypeEnv, expr: &Expression, u: &mut Unifier) -> Option<ITy> {
    match &expr.kind {
        ExpressionKind::Integer(_) => Some(ITy::Integer),
        ExpressionKind::True | ExpressionKind::False => Some(ITy::Boolean),
        ExpressionKind::Integers | ExpressionKind::Naturals | ExpressionKind::Naturals1 => {
            Some(ITy::pow(ITy::Integer))
        }
        ExpressionKind::BoolType => Some(ITy::pow(ITy::Boolean)),
        // Relational atoms (`id`/`prj1`/`prj2`/`pred`/`succ`) are their own AST
        // node now; an `Identifier` is an env-typed user name — or the
        // unifier's `target` (if any), which synthesizes as its fresh
        // variable so the surrounding equations can solve it (Rodin's
        // `getIdentType`). `env` is consulted first so a same-named binder
        // (inserted into the cloned local scope by the binder arms below)
        // shadows the target — the target being inferred is never itself in
        // `env`, so a free occurrence still falls through to its variable.
        ExpressionKind::Identifier(name) => match env.get(name) {
            Some(t) => Some(ITy::from(t)),
            None => u.target_var(name),
        },
        ExpressionKind::AtomicBuiltin(kind) => Some(atomic_builtin_type(*kind, u)),
        // `∅` is the generic empty set (Rodin's EMPTYSET): ℙ(α). Bare it
        // keeps a free variable and drops; in context (`∅ ∪ r`, `∅ ⦂ T`)
        // the variable is solved.
        ExpressionKind::EmptySet => Some(ITy::pow(u.fresh())),
        ExpressionKind::Unary { op, operand } => {
            let inner = synth(env, operand, u)?;
            match op {
                UnaryOp::Minus => Some(ITy::Integer),
                UnaryOp::PowerSet | UnaryOp::PowerSet1 => {
                    // POW(X) : ℙ(ℙ(elem))
                    match u.resolve(&inner) {
                        ITy::PowerSet(t) => Some(ITy::pow(ITy::pow(*t))),
                        _ => None,
                    }
                }
                UnaryOp::Domain => {
                    let (l, _) = u.as_relation(&inner)?;
                    Some(ITy::pow(l))
                }
                UnaryOp::Range => {
                    let (_, r) = u.as_relation(&inner)?;
                    Some(ITy::pow(r))
                }
                UnaryOp::Inverse => {
                    let (l, r) = u.as_relation(&inner)?;
                    Some(ITy::relation(r, l))
                }
            }
        }
        ExpressionKind::SetEnumeration(items) => {
            // `{e₁, e₂, …}` has type ℙ(T) where T is the common element
            // type. A polymorphic item (`∅`) contributes a fresh variable
            // that unifies with its typed siblings (so `{x, ∅}` types as
            // `ℙ(typeof(x))`); an all-polymorphic set keeps a free variable
            // and drops, matching Rodin.
            let mut ty: Option<ITy> = None;
            for it in items {
                let t = synth(env, it, u)?;
                if let Some(prev) = &ty {
                    u.unify(prev, &t).ok()?;
                } else {
                    ty = Some(t);
                }
            }
            ty.map(ITy::pow)
        }
        ExpressionKind::Binary { op, left, right } => match op {
            BinaryOp::Add
            | BinaryOp::Subtract
            | BinaryOp::Multiply
            | BinaryOp::Divide
            | BinaryOp::Modulo
            | BinaryOp::Exponent => Some(ITy::Integer),
            BinaryOp::Range => Some(ITy::pow(ITy::Integer)),
            // Set/relation-preserving binary ops: both operands share a
            // type, so unify them — that lets a polymorphic operand (`∅`,
            // `id`) be pinned by its sibling. One legitimately-untyped side
            // (a constant not yet resolved in the fixpoint) is tolerated.
            BinaryOp::Union
            | BinaryOp::Intersection
            | BinaryOp::Difference
            | BinaryOp::Overwrite => {
                let lt = synth(env, left, u);
                let rt = synth(env, right, u);
                unify_or_either(u, lt, rt)
            }
            // `S ◁ r` / `S ⩤ r`: the set restricts the relation's domain;
            // the result keeps the relation's type. Unifying the set's
            // element type with the domain pins a polymorphic relation
            // (e.g. `S ◁ id : ℙ(S×S)`).
            BinaryOp::DomainRestriction | BinaryOp::DomainSubtraction => {
                let rel = synth(env, right, u)?;
                let (dom, _) = u.as_relation(&rel)?;
                let set = synth(env, left, u);
                constrain_with_set(u, &dom, set);
                Some(rel)
            }
            // `r ▷ S` / `r ⩥ S`: the set restricts the relation's range.
            BinaryOp::RangeRestriction | BinaryOp::RangeSubtraction => {
                let rel = synth(env, left, u)?;
                let (_, ran) = u.as_relation(&rel)?;
                let set = synth(env, right, u);
                constrain_with_set(u, &ran, set);
                Some(rel)
            }
            // Forward / backward composition, unifying the shared middle
            // type so a polymorphic operand is pinned by the other.
            // `r ; s` (forward, Semicolon): r:ℙ(α×β), s:ℙ(β×γ) ⇒ ℙ(α×γ).
            // `s ∘ r` (backward, Composition): s:ℙ(β×γ), r:ℙ(α×β) ⇒ ℙ(α×γ).
            BinaryOp::Semicolon => {
                let ((la, lb), (rb, rc)) = synth_two_relations(env, left, right, u)?;
                u.unify(&lb, &rb).ok()?;
                Some(ITy::relation(la, rc))
            }
            BinaryOp::Composition => {
                let ((lb, lc), (ra, rb)) = synth_two_relations(env, left, right, u)?;
                u.unify(&lb, &rb).ok()?;
                Some(ITy::relation(ra, lc))
            }
            // `r ⊗ s` (DirectProduct): r:ℙ(α×β), s:ℙ(α×γ) ⇒ ℙ(α×(β×γ)).
            // Unify the shared domain so e.g. `r ⊗ id` pins `id` from `r`.
            BinaryOp::DirectProduct => {
                let ((la, lb), (ra, rb)) = synth_two_relations(env, left, right, u)?;
                u.unify(&la, &ra).ok()?;
                Some(ITy::relation(la, ITy::prod(lb, rb)))
            }
            // `r ∥ s` (ParallelProduct): r:ℙ(α×β), s:ℙ(γ×δ) ⇒ ℙ((α×γ)×(β×δ)).
            BinaryOp::ParallelProduct => {
                let ((la, lb), (ra, rb)) = synth_two_relations(env, left, right, u)?;
                Some(ITy::relation(ITy::prod(la, ra), ITy::prod(lb, rb)))
            }
            BinaryOp::CartesianProduct => {
                let lt = synth(env, left, u)?;
                let rt = synth(env, right, u)?;
                match (u.resolve(&lt), u.resolve(&rt)) {
                    (ITy::PowerSet(l), ITy::PowerSet(r)) => Some(ITy::relation(*l, *r)),
                    _ => None,
                }
            }
            BinaryOp::Maplet => {
                let lt = synth(env, left, u)?;
                let rt = synth(env, right, u)?;
                Some(ITy::prod(lt, rt))
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
                let lt = synth(env, left, u)?;
                let rt = synth(env, right, u)?;
                match (u.resolve(&lt), u.resolve(&rt)) {
                    (ITy::PowerSet(l), ITy::PowerSet(r)) => Some(ITy::pow(ITy::relation(*l, *r))),
                    _ => None,
                }
            }
            // `e ⦂ T` — type ascription. The RHS is itself a type
            // expression (`ℤ`, `ℙ(USERS)`, `T × U`); interpret it as a
            // [`Type`] rather than as a set value.
            BinaryOp::OfType => parse_type_from_expression(right).map(|t| ITy::from(&t)),
        },
        ExpressionKind::FunctionApplication {
            function,
            arguments,
        } => {
            // `f(x)`: when `f : ℙ(α × β)`, the application has type `β`. A pair
            // argument is the single maplet `f(a ↦ b)`. Best-effort unify the
            // argument against the domain (an argument still resolving in the
            // fixpoint is skipped) so a polymorphic function's domain variable
            // gets pinned; return the codomain.
            let f = synth(env, function, u)?;
            let (dom, codomain) = u.as_relation(&f)?;
            if let Some(arg) = arguments.first()
                && let Some(arg_t) = synth(env, arg, u)
            {
                u.unify(&dom, &arg_t).ok();
            }
            Some(codomain)
        }
        ExpressionKind::BuiltinApplication {
            function,
            arguments,
        } => match function {
            // Cardinality / min / max of any set return integers.
            BuiltinFunction::Card | BuiltinFunction::Min | BuiltinFunction::Max => {
                Some(ITy::Integer)
            }
            // Generalized union/intersection collapses one power-set level:
            // union(S)/inter(S) : ℙ(α) when S : ℙ(ℙ(α)).
            BuiltinFunction::Union | BuiltinFunction::Inter => {
                let arg = synth(env, arguments.first()?, u)?;
                match u.resolve(&arg) {
                    ITy::PowerSet(inner) if matches!(*inner, ITy::PowerSet(_)) => Some(*inner),
                    _ => None,
                }
            }
        },
        // `r[A]` — relational image: `r : ℙ(α × β)`, `A : ℙ(α)` ⇒ `ℙ(β)`.
        // Unifying A's element type with the domain pins a polymorphic
        // relation (e.g. `id[S] : ℙ(S)`).
        ExpressionKind::RelationalImage { relation, set } => {
            let rel = synth(env, relation, u)?;
            let (dom, ran) = u.as_relation(&rel)?;
            let set_t = synth(env, set, u);
            constrain_with_set(u, &dom, set_t);
            Some(ITy::pow(ran))
        }
        // `bool(P)` — promotes a predicate to a Boolean value.
        ExpressionKind::Bool(_) => Some(ITy::Boolean),
        // λ pattern · P ∣ E. Bind the pattern names from explicit type
        // ascriptions or from `P`, then return ℙ(dom × typeof(E)).
        ExpressionKind::Lambda {
            pattern,
            predicate,
            expression,
        } => {
            let names = pattern.identifiers();
            let mut identifiers = Vec::new();
            collect_pattern_identifiers(pattern, &mut identifiers);
            let mut local = env.clone();
            let bound = bind_names(&mut local, &names, &identifiers, predicate);
            if !names.iter().all(|n| bound.contains_key(*n)) {
                return None;
            }
            let dom = pattern_to_type(pattern, &bound)?;
            let body_ty = synth(&local, expression, u)?;
            Some(ITy::relation(ITy::from(&dom), body_ty))
        }
        // `{ x ⦂ T · P ∣ E }` (extended) and `{ x · P }` (basic). Bind
        // each binder from explicit `T` if present, else from `P`. Body
        // type is typeof(E) for the extended form, else the
        // left-associative maplet of the binders for the basic form.
        ExpressionKind::SetComprehension {
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
                Some(e) => synth(&local, e, u)?,
                None => ITy::from(&binder_left_assoc_product(&names, &bound)?),
            };
            Some(ITy::pow(body_ty))
        }
        // `{ E ∣ P }` — set builder. Bound identifiers are the free
        // identifiers of `E` not already in scope; bind them from `P`.
        ExpressionKind::SetBuilder {
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
            let body_ty = synth(&local, member_expression, u)?;
            Some(ITy::pow(body_ty))
        }
        // `⋃ x ⦂ T · P ∣ E` and `⋂ x ⦂ T · P ∣ E`. Bind binders, then
        // return typeof(E) — the body must already be a set.
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
            let names: Vec<&str> = identifiers.iter().map(|i| i.name.as_str()).collect();
            let mut local = env.clone();
            let bound = bind_names(&mut local, &names, identifiers, predicate);
            if !names.iter().all(|n| bound.contains_key(*n)) {
                return None;
            }
            synth(&local, expression, u)
        }
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
    // `local` is a clone dedicated to this binder. Mask every same-named
    // outer declaration before installing explicit or inferred binder types.
    for name in names {
        local.remove(name);
    }
    // Pass 1: explicit `⦂ T`. The slice is empty for binder forms that
    // cannot carry explicit annotations.
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
    match &expr.kind {
        ExpressionKind::Integers => Some(Type::Integer),
        ExpressionKind::BoolType => Some(Type::Boolean),
        ExpressionKind::Identifier(n) => Some(Type::GivenSet(n.clone())),
        ExpressionKind::Unary {
            op: UnaryOp::PowerSet | UnaryOp::PowerSet1,
            operand,
        } => Some(Type::pow(parse_type_from_expression(operand)?)),
        ExpressionKind::Binary {
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

fn collect_pattern_identifiers(pat: &IdentPattern, out: &mut Vec<TypedIdentifier>) {
    match pat {
        IdentPattern::Identifier(identifier) => out.push(identifier.clone()),
        IdentPattern::Maplet(left, right) => {
            collect_pattern_identifiers(left, out);
            collect_pattern_identifiers(right, out);
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
/// The caller must ensure each candidate name is absent from `env` or bound
/// there to the fresh binder, never inherited from an outer declaration.
pub(crate) fn collect_binder_types(
    env: &TypeEnv,
    pred: &Predicate,
    names: &[&str],
    out: &mut BTreeMap<String, Type>,
) {
    match &pred.kind {
        PredicateKind::Comparison {
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
                &left.kind,
                ExpressionKind::Binary {
                    op: BinaryOp::Maplet,
                    ..
                }
            ) && let Some(Type::PowerSet(pair)) = type_of_expression(env, right)
            {
                collect_from_maplet(left, &pair, names, out);
            }
        }
        PredicateKind::Comparison {
            op: ComparisonOp::Equal | ComparisonOp::NotEqual,
            left,
            right,
        } => {
            for (a, b) in [(left, right), (right, left)] {
                if let ExpressionKind::Identifier(n) = &a.kind
                    && names.contains(&n.as_str())
                    && !out.contains_key(n)
                    && let Some(t) = type_of_expression(env, b)
                {
                    out.insert(n.clone(), t);
                }
            }
        }
        // Integer ordering: `x < e`, `x ≤ e`, `x > e`, `x ≥ e` (and the
        // symmetric forms) demand both sides be ℤ, so a bare binder on
        // either side is ℤ. Mirrors `infer_constant_from_predicate`'s
        // ordering rule. (Covers corpus lambda guards `λp· p ≥ 0 ∣ …`
        // / `λp· p < 0 ∣ …`.)
        PredicateKind::Comparison {
            op:
                ComparisonOp::LessThan
                | ComparisonOp::LessEqual
                | ComparisonOp::GreaterThan
                | ComparisonOp::GreaterEqual,
            left,
            right,
        } => {
            for side in [left, right] {
                if let ExpressionKind::Identifier(n) = &side.kind
                    && names.contains(&n.as_str())
                {
                    out.entry(n.clone()).or_insert(Type::Integer);
                }
            }
        }
        PredicateKind::Logical {
            op: LogicalOp::And,
            left,
            right,
        } => {
            collect_binder_types(env, left, names, out);
            collect_binder_types(env, right, names, out);
        }
        // Implication: typing constraints in the antecedent carry over
        // to the binder. (Common shape: `∀x · x∈ℤ ⇒ P`.)
        PredicateKind::Logical {
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
        PredicateKind::Logical {
            op: LogicalOp::Equivalent,
            left,
            right,
        } => {
            collect_binder_types(env, left, names, out);
            collect_binder_types(env, right, names, out);
        }
        PredicateKind::Quantified {
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
                let inner_names: Vec<&str> = identifiers
                    .iter()
                    .map(|identifier| identifier.name.as_str())
                    .collect();
                let mut local = env.clone();
                bind_names(&mut local, &inner_names, identifiers, body);
                collect_binder_types(&local, body, &unshadowed, out);
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
    match (&maplet.kind, pair) {
        (
            ExpressionKind::Binary {
                op: BinaryOp::Maplet,
                left,
                right,
            },
            Type::Product(lty, rty),
        ) => {
            collect_from_maplet(left, lty, names, out);
            collect_from_maplet(right, rty, names, out);
        }
        (ExpressionKind::Identifier(n), _) if names.contains(&n.as_str()) => {
            out.entry(n.clone()).or_insert_with(|| pair.clone());
        }
        _ => {}
    }
}

/// Collect bare identifiers referenced in `expr`, in left-to-right
/// order, deduped. Used by the SetBuilder arm to figure out which names
/// are binders. Recurses through binary/unary/application/`bool(P)`/etc.
/// but stops at lambda /
/// set-comprehension / set-builder / quantified-union / quantified-inter —
/// those nodes' internal binders shouldn't leak. Names bound by a quantifier
/// inside `bool(P)` are also filtered out.
///
/// This stops-at-binders, expression-only contract is the opposite of
/// `wd::normal`'s `expression_free_names`, which descends *into* binders
/// (and predicates) to find every free occurrence for unused-declaration
/// dropping. They are intentionally distinct analyses — don't unify them.
pub(crate) fn collect_free_identifiers<'a>(expr: &'a Expression, out: &mut Vec<&'a str>) {
    let mut shadow: Vec<&'a str> = Vec::new();
    collect_free_idents_expr(expr, &mut shadow, out);
}

fn collect_free_idents_expr<'a>(
    expr: &'a Expression,
    shadow: &mut Vec<&'a str>,
    out: &mut Vec<&'a str>,
) {
    match &expr.kind {
        ExpressionKind::Identifier(n)
            if !shadow.contains(&n.as_str()) && !out.contains(&n.as_str()) =>
        {
            out.push(n.as_str());
        }
        ExpressionKind::Binary { left, right, .. } => {
            collect_free_idents_expr(left, shadow, out);
            collect_free_idents_expr(right, shadow, out);
        }
        ExpressionKind::Unary { operand, .. } => collect_free_idents_expr(operand, shadow, out),
        ExpressionKind::SetEnumeration(items) => {
            for i in items {
                collect_free_idents_expr(i, shadow, out);
            }
        }
        ExpressionKind::FunctionApplication {
            function,
            arguments,
        } => {
            collect_free_idents_expr(function, shadow, out);
            for a in arguments {
                collect_free_idents_expr(a, shadow, out);
            }
        }
        ExpressionKind::BuiltinApplication { arguments, .. } => {
            for a in arguments {
                collect_free_idents_expr(a, shadow, out);
            }
        }
        ExpressionKind::RelationalImage { relation, set } => {
            collect_free_idents_expr(relation, shadow, out);
            collect_free_idents_expr(set, shadow, out);
        }
        ExpressionKind::Bool(p) => collect_free_idents_pred(p, shadow, out),
        _ => {}
    }
}

fn collect_free_idents_pred<'a>(
    pred: &'a Predicate,
    shadow: &mut Vec<&'a str>,
    out: &mut Vec<&'a str>,
) {
    match &pred.kind {
        PredicateKind::Comparison { left, right, .. } => {
            collect_free_idents_expr(left, shadow, out);
            collect_free_idents_expr(right, shadow, out);
        }
        PredicateKind::Logical { left, right, .. } => {
            collect_free_idents_pred(left, shadow, out);
            collect_free_idents_pred(right, shadow, out);
        }
        PredicateKind::Not(inner) => collect_free_idents_pred(inner, shadow, out),
        PredicateKind::Quantified {
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
        PredicateKind::Application { arguments, .. }
        | PredicateKind::BuiltinApplication { arguments, .. } => {
            for a in arguments {
                collect_free_idents_expr(a, shadow, out);
            }
        }
        PredicateKind::True | PredicateKind::False => {}
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
    if let PredicateKind::Quantified {
        identifiers,
        predicate: body,
        ..
    } = &predicate.kind
    {
        // A same-named bound variable is a different identifier, so the
        // quantified subtree contains no occurrence of this outer constant.
        if identifiers.iter().any(|t| t.name == constant_name) {
            return None;
        }

        // Enter the quantifier once through a binder-local environment. This
        // keeps both the main inference rules and their fallback passes from
        // revisiting the body against unmasked outer declarations.
        let names: Vec<&str> = identifiers.iter().map(|t| t.name.as_str()).collect();
        let mut local = env.clone();
        bind_names(&mut local, &names, identifiers, body);
        return infer_constant_from_predicate(&local, body, constant_name);
    }

    match &predicate.kind {
        PredicateKind::Comparison { op, left, right } => match op {
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
                if let ExpressionKind::Binary {
                    op: BinaryOp::Maplet,
                    ..
                } = &left.kind
                    && let Some(Type::PowerSet(pair)) = type_of_expression(env, right)
                    && let Some(ty) = infer_from_maplet_pattern(left, &pair, constant_name)
                {
                    return Some(ty);
                }
                // `c(args) ∈ S` — the constant applied as a function: the
                // application rule `c : ℙ(α × β)` with `args : α` and
                // `S : ℙ(β)` types the constant as a relation from the
                // argument type to S's element type. (Covers the corpus
                // shape `∀i · i ∈ 0‥7 ⇒ P(i) ∈ 0‥6`, which types
                // `P : ℙ(ℤ × ℤ)` in Rodin.)
                if let ExpressionKind::FunctionApplication {
                    function,
                    arguments,
                } = &left.kind
                    && matches!(&function.kind, ExpressionKind::Identifier(n) if n == constant_name)
                    && let Some(arg) = arguments.first()
                    && let Some(arg_t) = type_of_expression(env, arg)
                    && let Some(Type::PowerSet(elem)) = type_of_expression(env, right)
                {
                    return Some(Type::pow(Type::prod(arg_t, *elem)));
                }
                None
            }
            ComparisonOp::Subset
            | ComparisonOp::SubsetStrict
            | ComparisonOp::NotSubset
            | ComparisonOp::NotSubsetStrict => {
                // `c ⊆ S` types `c : typeof(S)`. Return `None` as a value
                // (not via `?`) so a buried target — e.g. `{c} ⊆ S` — falls
                // through to the `.or_else` chain below.
                match as_ident(left) {
                    Some(name) if name == constant_name => type_of_expression(env, right),
                    _ => None,
                }
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
                if matches!(&left.kind, ExpressionKind::Identifier(n) if n == constant_name)
                    || matches!(&right.kind, ExpressionKind::Identifier(n) if n == constant_name)
                {
                    Some(Type::Integer)
                } else {
                    None
                }
            }
        },
        PredicateKind::BuiltinApplication {
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
        PredicateKind::Logical {
            op: LogicalOp::And | LogicalOp::Or | LogicalOp::Implies | LogicalOp::Equivalent,
            left,
            right,
        } => infer_constant_from_predicate(env, left, constant_name)
            .or_else(|| infer_constant_from_predicate(env, right, constant_name)),
        _ => None,
    }
    .or_else(|| infer_from_function_argument(env, predicate, constant_name))
    .or_else(|| infer_ident_via_unification(env, predicate, constant_name))
}

/// Last-resort typing: scan `pred` for an [`ExpressionKind::FunctionApplication`] whose
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
    match &p.kind {
        PredicateKind::Comparison { left, right, .. } => {
            walk_expr_for_arg(env, left, target, found);
            walk_expr_for_arg(env, right, target, found);
        }
        PredicateKind::Logical { left, right, .. } => {
            walk_pred_for_arg(env, left, target, found);
            walk_pred_for_arg(env, right, target, found);
        }
        PredicateKind::Not(inner) => walk_pred_for_arg(env, inner, target, found),
        PredicateKind::Quantified {
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
        PredicateKind::Application { arguments, .. }
        | PredicateKind::BuiltinApplication { arguments, .. } => {
            for a in arguments {
                walk_expr_for_arg(env, a, target, found);
            }
        }
        PredicateKind::True | PredicateKind::False => {}
    }
}

fn walk_expr_for_arg(env: &TypeEnv, e: &Expression, target: &str, found: &mut Option<Type>) {
    if found.is_some() {
        return;
    }
    match &e.kind {
        ExpressionKind::FunctionApplication {
            function,
            arguments,
        } => {
            walk_expr_for_arg(env, function, target, found);
            if found.is_some() {
                return;
            }
            if let Some((dom, _)) = type_of_expression(env, function).and_then(Type::into_relation)
                && let Some(arg) = arguments.first()
                && let Some(t) = infer_from_maplet_pattern(arg, &dom, target)
            {
                *found = Some(t);
                return;
            }
            for a in arguments {
                walk_expr_for_arg(env, a, target, found);
                if found.is_some() {
                    return;
                }
            }
        }
        ExpressionKind::Binary { left, right, .. } => {
            walk_expr_for_arg(env, left, target, found);
            if found.is_some() {
                return;
            }
            walk_expr_for_arg(env, right, target, found);
        }
        ExpressionKind::Unary { operand, .. } => walk_expr_for_arg(env, operand, target, found),
        ExpressionKind::SetEnumeration(items) => {
            for i in items {
                walk_expr_for_arg(env, i, target, found);
                if found.is_some() {
                    return;
                }
            }
        }
        ExpressionKind::BuiltinApplication { arguments, .. } => {
            for a in arguments {
                walk_expr_for_arg(env, a, target, found);
                if found.is_some() {
                    return;
                }
            }
        }
        ExpressionKind::RelationalImage { relation, set } => {
            walk_expr_for_arg(env, relation, target, found);
            if found.is_some() {
                return;
            }
            walk_expr_for_arg(env, set, target, found);
        }
        ExpressionKind::Bool(p) => walk_pred_for_arg(env, p, target, found),
        _ => {}
    }
}

/// Last-resort, Rodin-faithful typing of a *buried* identifier — one that
/// the syntactic patterns above never reach because it appears only inside
/// an operand expression (e.g. `w` in `v ∈ a ⇸ S ∖ {w}`). Give `target`
/// the fresh-variable treatment Rodin's `getIdentType` gives every free
/// identifier, synthesize the predicate's expression operands with that
/// variable in place, register the relational equation Rodin's
/// `RelationalPredicate.typeCheck` registers, and read the solved type back
/// (`ground` — Rodin's `solveTypeVariables`, which drops an unsolved
/// variable). Descends `∧`/`∨`/`⇒`/`⇔`/`¬` and quantifier bodies
/// (a same-named binder shadows `target`).
fn infer_ident_via_unification(env: &TypeEnv, pred: &Predicate, target: &str) -> Option<Type> {
    match &pred.kind {
        PredicateKind::Comparison { op, left, right } => {
            infer_ident_from_comparison(env, op, left, right, target)
        }
        PredicateKind::Logical { left, right, .. } => {
            infer_ident_via_unification(env, left, target)
                .or_else(|| infer_ident_via_unification(env, right, target))
        }
        PredicateKind::Not(inner) => infer_ident_via_unification(env, inner, target),
        PredicateKind::Quantified {
            identifiers,
            predicate: body,
            ..
        } if !identifiers.iter().any(|t| t.name == target) => {
            infer_ident_via_unification(env, body, target)
        }
        _ => None,
    }
}

/// Type `target` from a single comparison. First try the target's own
/// operand alone (its internal operators may already pin it, as `S ∖ {w}`
/// pins `w`); then, if both operands synthesize, add the comparison's
/// relational equation and solve. Only ever grounds the target variable
/// after the synth that could bind it returned `Some` — a type is committed
/// only when the expression type-checks, exactly as Rodin commits a type
/// only when `solveTypeVariables` succeeds.
fn infer_ident_from_comparison(
    env: &TypeEnv,
    op: &ComparisonOp,
    left: &Expression,
    right: &Expression,
    target: &str,
) -> Option<Type> {
    // `expr_mentions` reports a *free* occurrence; a target that appears
    // only as a same-named binder is not mentioned, and neither path should
    // run for it (the binder is a different identifier).
    let in_left = expr_mentions(left, target);
    let in_right = expr_mentions(right, target);
    if !in_left && !in_right {
        return None;
    }

    // 1. Operand-self: `prs ⇸ FACTORY ∖ {rf}` synthesizes and the inner
    //    SETMINUS pins `rf` without needing the other operand.
    if in_left && let Some(t) = synth_ident_in_expr(env, left, target) {
        return Some(t);
    }
    if in_right && let Some(t) = synth_ident_in_expr(env, right, target) {
        return Some(t);
    }

    // 2. Relational equation across both operands (RelationalPredicate.typeCheck).
    //    Requires both operands to synthesize — `?` enforces ground-after-Some,
    //    so `a = b` with both sides untyped yields no spurious type.
    let (mut u, tv) = target_unifier(target);
    let lt = synth(env, left, &mut u)?;
    let rt = synth(env, right, &mut u)?;
    apply_relational_equation(&mut u, op, &lt, &rt);
    ground(&u, &ITy::Var(tv))
}

/// A fresh unifier whose [`Unifier::target`] is `name`, plus that target's
/// variable id — the shared setup for the buried-identifier attempts.
fn target_unifier(name: &str) -> (Unifier, u32) {
    let mut u = Unifier::new();
    let tv = u.fresh_var();
    u.target = Some((name.to_string(), tv));
    (u, tv)
}

/// Synthesize `expr` with `target` standing in as a fresh variable, then
/// read that variable's solved type. `None` unless `synth` succeeds (no
/// type from an aborted synth) and the variable grounds to a concrete type.
fn synth_ident_in_expr(env: &TypeEnv, expr: &Expression, target: &str) -> Option<Type> {
    let (mut u, tv) = target_unifier(target);
    synth(env, expr, &mut u)?;
    ground(&u, &ITy::Var(tv))
}

/// The unification equation `RelationalPredicate.typeCheck` registers for
/// `op`, applied to already-synthesized operand types. Clashes are ignored
/// (`.ok()`): the caller decides nothing was learned when the target
/// variable fails to ground.
fn apply_relational_equation(u: &mut Unifier, op: &ComparisonOp, lt: &ITy, rt: &ITy) {
    match op {
        // `x = y` / `x ≠ y`: both sides share a type.
        ComparisonOp::Equal | ComparisonOp::NotEqual => {
            u.unify(lt, rt).ok();
        }
        // `x ∈ S` / `x ∉ S`: `S : ℙ(typeof x)`.
        ComparisonOp::In | ComparisonOp::NotIn => {
            u.unify(rt, &ITy::pow(lt.clone())).ok();
        }
        // `x ⊆ S` (and strict / negated): both sides are `ℙ(α)`.
        ComparisonOp::Subset
        | ComparisonOp::SubsetStrict
        | ComparisonOp::NotSubset
        | ComparisonOp::NotSubsetStrict => {
            let alpha = u.fresh();
            u.unify(lt, &ITy::pow(alpha.clone())).ok();
            u.unify(rt, &ITy::pow(alpha)).ok();
        }
        // Integer ordering: both sides are ℤ.
        ComparisonOp::LessThan
        | ComparisonOp::LessEqual
        | ComparisonOp::GreaterThan
        | ComparisonOp::GreaterEqual => {
            u.unify(lt, &ITy::Integer).ok();
            u.unify(rt, &ITy::Integer).ok();
        }
    }
}

/// Does `target` occur free (not shadowed by an inner binder) anywhere in
/// `expr`? Reuses [`collect_free_identifiers`], which already handles
/// lambda / comprehension / quantifier shadowing.
fn expr_mentions(expr: &Expression, target: &str) -> bool {
    let mut names: Vec<&str> = Vec::new();
    collect_free_identifiers(expr, &mut names);
    names.contains(&target)
}

fn as_ident(e: &Expression) -> Option<&str> {
    match &e.kind {
        ExpressionKind::Identifier(n) => Some(n.as_str()),
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
    match (&maplet.kind, pair) {
        (
            ExpressionKind::Binary {
                op: BinaryOp::Maplet,
                left,
                right,
            },
            Type::Product(lty, rty),
        ) => infer_from_maplet_pattern(left, lty, constant_name)
            .or_else(|| infer_from_maplet_pattern(right, rty, constant_name)),
        (ExpressionKind::Identifier(n), _) if n == constant_name => Some(pair.clone()),
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
    if let ExpressionKind::Identifier(n) = &lhs.kind
        && n == constant_name
        && let Some(t) = type_of_expression(env, rhs)
    {
        return Some(t);
    }
    if let ExpressionKind::Identifier(n) = &rhs.kind
        && n == constant_name
        && let Some(t) = type_of_expression(env, lhs)
    {
        return Some(t);
    }
    // Maplet-equality: `(p ↦ q) = expr` where typeof(expr) = T × U.
    let try_maplet_eq = |maplet: &Expression, other: &Expression| -> Option<Type> {
        if !matches!(
            &maplet.kind,
            ExpressionKind::Binary {
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
        let ExpressionKind::SetEnumeration(items) = &enum_.kind else {
            return None;
        };
        if !items
            .iter()
            .any(|e| matches!(&e.kind, ExpressionKind::Identifier(n) if n == constant_name))
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
    match &expr.kind {
        ExpressionKind::Identifier(n) => n == constant_name,
        ExpressionKind::Binary {
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
            match &arg.kind {
                ExpressionKind::SetEnumeration(items) => {
                    for item in items {
                        if matches!(&item.kind, ExpressionKind::Identifier(n) if n == constant_name)
                        {
                            return Some(*elem.clone());
                        }
                    }
                }
                ExpressionKind::Identifier(n) if n == constant_name => {
                    return Some(Type::pow(*elem.clone()));
                }
                _ => {}
            }
        }
    }
    // Path 2: head IS the unknown — type it from any tail entry whose
    // type is known. Singletons `{x}` give `head : ℙ(typeof(x))`; bare
    // subsets `X : ℙ(T)` give `head : ℙ(T)`.
    if matches!(&head.kind, ExpressionKind::Identifier(n) if n == constant_name) {
        for arg in tail {
            match &arg.kind {
                ExpressionKind::SetEnumeration(items) => {
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
    use rossi::{parse_expression_str, parse_predicate_str};

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
            type_of_expression(&env, &ExpressionKind::Integer(42).into()),
            Some(Type::Integer)
        );
        assert_eq!(
            type_of_expression(&env, &ExpressionKind::True.into()),
            Some(Type::Boolean)
        );
        assert_eq!(
            type_of_expression(&env, &ExpressionKind::Integers.into()),
            Some(Type::pow(Type::Integer))
        );
    }

    #[test]
    fn identifier_lookup() {
        let env = env_with(&[("n", Type::Integer)]);
        assert_eq!(
            type_of_expression(&env, &ExpressionKind::Identifier("n".into()).into()),
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
    fn infer_const_from_own_function_application() {
        // `f(1) ∈ 0‥6` types the applied constant itself: f : ℙ(ℤ × ℤ).
        let env = TypeEnv::new();
        let p = parse_predicate_str("f(1) ∈ 0‥6").unwrap();
        let ty = infer_constant_from_predicate(&env, &p, "f");
        assert_eq!(ty, Some(Type::relation(Type::Integer, Type::Integer)));
    }

    #[test]
    fn infer_const_from_function_application_under_quantifier() {
        // The binder `i` is typed by the antecedent, then
        // `P(i) ∈ 0‥6` types P : ℙ(ℤ × ℤ).
        let env = TypeEnv::new();
        let p = parse_predicate_str("∀i · (i ∈ 0‥7 ⇒ P(i) ∈ 0‥6)").unwrap();
        let ty = infer_constant_from_predicate(&env, &p, "P");
        assert_eq!(ty, Some(Type::relation(Type::Integer, Type::Integer)));
    }

    // --- Identifiers buried inside an operand expression ---------------
    // These mirror Rodin: every free identifier gets a fresh type variable
    // (`getIdentType`) and the surrounding equations solve it. The patterns
    // above only reach an identifier that is the *bare operand* of the
    // predicate; these need the `infer_ident_via_unification` fall-back.

    #[test]
    fn infer_buried_in_set_difference() {
        // `v ∈ A ⇸ S ∖ {w}`: `w` only appears inside the SETMINUS operand,
        // which forces `{w} : ℙ(S)`, hence `w : S`. (The shape that the
        // proof-language model's `rf` needs.)
        let mut env = TypeEnv::new();
        env.add_carrier_set("A");
        env.add_carrier_set("S");
        let p = parse_predicate_str("v ∈ A ⇸ S ∖ {w}").unwrap();
        let ty = infer_constant_from_predicate(&env, &p, "w");
        assert_eq!(ty, Some(Type::GivenSet("S".into())));
    }

    #[test]
    fn infer_buried_in_singleton_subset() {
        // `{w} ⊆ S`: the subset equation makes both sides `ℙ(α)`, so `w : S`.
        let mut env = TypeEnv::new();
        env.add_carrier_set("S");
        let p = parse_predicate_str("{w} ⊆ S").unwrap();
        let ty = infer_constant_from_predicate(&env, &p, "w");
        assert_eq!(ty, Some(Type::GivenSet("S".into())));
    }

    #[test]
    fn infer_buried_in_cartesian_product_equality() {
        // `p = q × {w}` with `p : ℙ(A × S)`, `q : ℙ(A)`: the equality
        // equation pins `{w} : ℙ(S)`, hence `w : S`.
        let env = env_with(&[
            (
                "p",
                Type::relation(Type::GivenSet("A".into()), Type::GivenSet("S".into())),
            ),
            ("q", Type::pow(Type::GivenSet("A".into()))),
        ]);
        let pr = parse_predicate_str("p = q × {w}").unwrap();
        let ty = infer_constant_from_predicate(&env, &pr, "w");
        assert_eq!(ty, Some(Type::GivenSet("S".into())));
    }

    #[test]
    fn no_type_from_equality_of_two_untyped_idents() {
        // `a = b` with neither side typed pins nothing — the fall-back must
        // not invent a type (locks in ground-after-Some).
        let env = TypeEnv::new();
        let p = parse_predicate_str("a = b").unwrap();
        assert_eq!(infer_constant_from_predicate(&env, &p, "a"), None);
    }

    #[test]
    fn no_type_from_shadowing_binder_of_same_name() {
        // `x` here is a *comprehension binder*, not a free identifier — it
        // does not type the machine variable `x`. The fall-back must respect
        // the binder scope and infer nothing (a free identifier in `synth`
        // must defer to a same-named binder in the local scope).
        let mut env = TypeEnv::new();
        env.add_carrier_set("S");
        env.insert("c", Type::pow(Type::GivenSet("S".into())));
        let p = parse_predicate_str("c = {x · x ∈ S ∣ x}").unwrap();
        assert_eq!(infer_constant_from_predicate(&env, &p, "x"), None);
    }

    #[test]
    fn same_named_outer_does_not_type_lambda_binder() {
        let mut env = TypeEnv::new();
        env.insert("x", Type::Integer);
        let expr = parse_expression_str("λx·x=x ∣ x").unwrap();
        assert_eq!(type_of_expression(&env, &expr), None);
    }

    #[test]
    fn explicitly_typed_lambda_binder_survives_outer_masking() {
        let mut env = TypeEnv::new();
        env.insert("x", Type::Boolean);
        let expr = parse_expression_str("λx⦂ℤ·⊤ ∣ x").unwrap();
        assert_eq!(
            type_of_expression(&env, &expr),
            Some(Type::relation(Type::Integer, Type::Integer))
        );
    }

    #[test]
    fn same_named_outer_does_not_type_quantified_constant_dependency() {
        let mut env = TypeEnv::new();
        env.insert("x", Type::Integer);
        let pred = parse_predicate_str("∀x·c=x").unwrap();
        assert_eq!(infer_constant_from_predicate(&env, &pred, "c"), None);
    }

    #[test]
    fn infer_const_from_union_of_lambdas_with_ordering_guards() {
        // Each lambda binder is typed ℤ by its ordering guard
        // (`p ≥ 0` / `p < 0`), so the union of lambdas types the
        // constant ℙ(ℤ × ℤ).
        let env = TypeEnv::new();
        let p = parse_predicate_str("F = (λp· p ≥ 0 ∣ p + 1) ∪ (λp· p < 0 ∣ p − 1)").unwrap();
        let ty = infer_constant_from_predicate(&env, &p, "F");
        assert_eq!(ty, Some(Type::relation(Type::Integer, Type::Integer)));
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
        let p = parse_predicate_str("maximum = 100").unwrap();
        let ty = infer_constant_from_predicate(&env, &p, "maximum");
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
    fn succ_pred_are_integer_relations() {
        // `succ`/`pred` are core atomic relations of type ℙ(ℤ × ℤ); applying
        // one to an integer yields ℤ, and `c = succ` types `c` as the relation.
        let env = TypeEnv::new();
        let int_rel = Type::relation(Type::Integer, Type::Integer);
        assert_eq!(
            type_of_expression(&env, &parse_expr("succ")),
            Some(int_rel.clone())
        );
        assert_eq!(type_of_expression(&env, &parse_expr("pred")), Some(int_rel));
        assert_eq!(
            type_of_expression(&env, &parse_expr("succ(3)")),
            Some(Type::Integer)
        );
        let p = parse_predicate_str("c = succ").unwrap();
        assert_eq!(
            infer_constant_from_predicate(&env, &p, "c"),
            Some(Type::relation(Type::Integer, Type::Integer))
        );
    }

    #[test]
    fn generalized_union_inter_collapse_a_power_set_level() {
        // nested : ℙ(ℙ(USERS)) ⇒ union(nested)/inter(nested) : ℙ(USERS).
        let mut env = TypeEnv::new();
        env.add_carrier_set("USERS");
        env.insert(
            "nested",
            Type::pow(Type::pow(Type::GivenSet("USERS".into()))),
        );
        let expected = Some(Type::pow(Type::GivenSet("USERS".into())));
        assert_eq!(
            type_of_expression(&env, &parse_expr("union(nested)")),
            expected
        );
        assert_eq!(
            type_of_expression(&env, &parse_expr("inter(nested)")),
            expected
        );
        // Non-nested argument (ℙ(USERS)) has no power-set level to collapse.
        env.insert("flat", Type::pow(Type::GivenSet("USERS".into())));
        assert_eq!(type_of_expression(&env, &parse_expr("union(flat)")), None);
    }

    #[test]
    fn id_is_application_of_generic_identity() {
        // id(x) : typeof(x). id(S) : ℙ(S); id(n) : ℤ. (Rodin KID_GEN, not the
        // legacy identity-on-set reading that would give ℙ(S × S).)
        let mut env = TypeEnv::new();
        env.add_carrier_set("S");
        env.insert("n", Type::Integer);
        assert_eq!(
            type_of_expression(&env, &parse_expr("id(S)")),
            Some(Type::pow(Type::GivenSet("S".into())))
        );
        assert_eq!(
            type_of_expression(&env, &parse_expr("id(n)")),
            Some(Type::Integer)
        );
    }

    #[test]
    fn prj1_prj2_are_application_of_generic_projections() {
        // For a maplet m : S × T, prj1(m) : S and prj2(m) : T (Rodin
        // KPRJ1_GEN/KPRJ2_GEN application). The legacy "projection of a
        // relation" reading would instead take r : ℙ(S × T); a relation
        // argument has no maplet to project, so it types as None.
        let mut env = TypeEnv::new();
        env.add_carrier_set("S");
        env.add_carrier_set("T");
        let s = Type::GivenSet("S".into());
        let t = Type::GivenSet("T".into());
        env.insert("m", Type::prod(s.clone(), t.clone()));
        assert_eq!(type_of_expression(&env, &parse_expr("prj1(m)")), Some(s));
        assert_eq!(type_of_expression(&env, &parse_expr("prj2(m)")), Some(t));
        // A relation argument (ℙ(S × T)) is not a maplet ⇒ no projection.
        env.insert(
            "r",
            Type::relation(Type::GivenSet("S".into()), Type::GivenSet("T".into())),
        );
        assert_eq!(type_of_expression(&env, &parse_expr("prj1(r)")), None);
    }

    #[test]
    fn function_application_returns_codomain() {
        // floor : ℙ(CABINS × FLOORS), cabin : CABINS ⇒ floor(cabin) : FLOORS.
        let mut env = TypeEnv::new();
        env.add_carrier_set("CABINS");
        env.add_carrier_set("FLOORS");
        env.insert(
            "floor",
            Type::relation(
                Type::GivenSet("CABINS".into()),
                Type::GivenSet("FLOORS".into()),
            ),
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
            Type::relation(
                Type::GivenSet("ROOM".into()),
                Type::GivenSet("GUEST".into()),
            ),
        );
        env.insert("gst", Type::GivenSet("GUEST".into()));
        let ty = type_of_expression(&env, &parse_expr("checked ▷ {gst}"));
        assert_eq!(
            ty,
            Some(Type::relation(
                Type::GivenSet("ROOM".into()),
                Type::GivenSet("GUEST".into()),
            ))
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
            Type::relation(
                Type::GivenSet("ROOM".into()),
                Type::GivenSet("GUEST".into()),
            ),
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
            Type::relation(Type::GivenSet("A".into()), Type::GivenSet("B".into())),
        );
        env.insert(
            "s",
            Type::relation(Type::GivenSet("B".into()), Type::GivenSet("C".into())),
        );
        assert_eq!(
            type_of_expression(&env, &parse_expr("r ; s")),
            Some(Type::relation(
                Type::GivenSet("A".into()),
                Type::GivenSet("C".into()),
            ))
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
            Type::relation(
                Type::GivenSet("LIFTS".into()),
                Type::GivenSet("STATES".into()),
            ),
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
            Type::relation(
                Type::GivenSet("LIFTS".into()),
                Type::GivenSet("STATES".into()),
            ),
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
        let inner_fn = Type::relation(
            Type::GivenSet("FLOORS".into()),
            Type::GivenSet("BUTTON_STATES".into()),
        );
        env.insert(
            "lift_buttons_states",
            Type::relation(Type::GivenSet("LIFTS".into()), inner_fn),
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
            Some(Type::relation(
                Type::GivenSet("USERS".into()),
                Type::GivenSet("USERS".into()),
            ))
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
            Some(Type::relation(
                Type::GivenSet("ROLES".into()),
                Type::Boolean,
            ))
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
            Type::relation(
                Type::GivenSet("PORTS".into()),
                Type::prod(Type::GivenSet("MESSAGES".into()), Type::Integer),
            ),
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
            Type::relation(
                Type::GivenSet("PORTS".into()),
                Type::prod(Type::GivenSet("MESSAGES".into()), Type::Integer),
            ),
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
            Type::relation(Type::GivenSet("TRAIN".into()), Type::GivenSet("VSS".into())),
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
    fn set_builder_binder_typed_through_bool_member() {
        // `{ bool(x ∈ S) ∣ x ∈ S }` — `x` is the SetBuilder binder
        // (typed from the predicate as the element type of `S`).
        // The whole expression types as `ℙ(BOOL)`.
        let mut env = TypeEnv::new();
        env.add_carrier_set("S");
        let e = parse_expr("{ bool(x ∈ S) ∣ x ∈ S }");
        assert_eq!(type_of_expression(&env, &e), Some(Type::pow(Type::Boolean)),);
    }

    // ===== polymorphic atoms resolved by unification against context =====

    /// Env with carrier set S, `selfrel : S ↔ S`, and `sub_s : ℙ(S)`.
    fn env_s() -> TypeEnv {
        let mut env = TypeEnv::new();
        env.add_carrier_set("S");
        env.insert("selfrel", Type::relation(s_ty(), s_ty()));
        env.insert("sub_s", Type::pow(s_ty()));
        env
    }

    fn s_ty() -> Type {
        Type::GivenSet("S".into())
    }

    #[test]
    fn bare_polymorphic_atoms_drop() {
        // With no surrounding context to solve the type variable, the
        // generic atoms keep a free variable and drop — exactly as Rodin
        // leaves an unsolved TypeVariable untyped.
        let env = env_s();
        for src in [
            "id",
            "prj1",
            "prj2",
            "∅",
            "dom(id)",
            "ran(prj1)",
            "∅ ∪ ∅",
            "id ; id",
        ] {
            assert_eq!(
                type_of_expression(&env, &parse_expr(src)),
                None,
                "expected `{src}` to drop (free type variable)"
            );
        }
    }

    #[test]
    fn identity_restricted_to_set_resolves() {
        // `sub_s ◁ id` and `id ▷ sub_s`: the set pins id's variable to S.
        let env = env_s();
        let s_rel = Type::relation(s_ty(), s_ty());
        assert_eq!(
            type_of_expression(&env, &parse_expr("sub_s ◁ id")),
            Some(s_rel.clone())
        );
        assert_eq!(
            type_of_expression(&env, &parse_expr("id ▷ sub_s")),
            Some(s_rel)
        );
    }

    #[test]
    fn image_of_identity_resolves() {
        // `id[sub_s]`: id's domain unifies with sub_s's element type S,
        // so the image has type ℙ(S).
        let env = env_s();
        assert_eq!(
            type_of_expression(&env, &parse_expr("id[sub_s]")),
            Some(Type::pow(s_ty()))
        );
    }

    #[test]
    fn composition_with_identity_resolves() {
        // `id ; selfrel` and `selfrel ; id`: the shared middle type pins id.
        let env = env_s();
        let s_rel = Type::relation(s_ty(), s_ty());
        assert_eq!(
            type_of_expression(&env, &parse_expr("id ; selfrel")),
            Some(s_rel.clone())
        );
        assert_eq!(
            type_of_expression(&env, &parse_expr("selfrel ; id")),
            Some(s_rel)
        );
    }

    #[test]
    fn direct_product_with_identity_resolves() {
        // `selfrel ⊗ id`: the shared domain S pins id ⇒ ℙ(S × (S × S)).
        let env = env_s();
        assert_eq!(
            type_of_expression(&env, &parse_expr("selfrel ⊗ id")),
            Some(Type::relation(s_ty(), Type::prod(s_ty(), s_ty())))
        );
    }

    #[test]
    fn empty_set_union_resolves() {
        // `∅ ∪ selfrel`: ∅'s ℙ(α) unifies with selfrel ⇒ ℙ(S × S).
        let env = env_s();
        assert_eq!(
            type_of_expression(&env, &parse_expr("∅ ∪ selfrel")),
            Some(Type::relation(s_ty(), s_ty()))
        );
    }

    #[test]
    fn projection_in_composition_resolves() {
        // `(selfrel ⊗ selfrel) ; prj1`: the left relation S ↔ (S × S)
        // composed with prj1 : (S × S) ↔ S pins prj1's variables ⇒ ℙ(S × S).
        let env = env_s();
        assert_eq!(
            type_of_expression(&env, &parse_expr("(selfrel ⊗ selfrel) ; prj1")),
            Some(Type::relation(s_ty(), s_ty()))
        );
    }
}
