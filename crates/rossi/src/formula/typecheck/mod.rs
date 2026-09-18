//! Two-pass type checking.
//!
//! Pass 1 walks the formula, assigning every expression node and
//! declaration a slot in a shadow list of inference variables and
//! registering the operator constraints; free identifiers resolve
//! through the environment, unknown ones get fresh variables and an
//! inferred-environment entry. Solving then either yields a type for
//! every slot or reports problems (with the offending node's span).
//! Pass 2, run only on success, rebuilds the formula in the same
//! traversal order, stamping solved types onto leaves and
//! declarations; compound nodes type themselves by construction.
//!
//! Type ascriptions and declaration annotations are *constraints*: the
//! spelled type is interpreted (`ℤ`, `BOOL`, given sets, `ℙ(·)`,
//! products) and unified with the ascribed node. A spelling that is
//! not a type makes an ascription ill-typed but leaves a declaration
//! annotation simply unconstraining, mirroring the checker this layer
//! replaces.

mod unifier;

use std::collections::HashMap;

use crate::ast::Span;

use super::assignment::{Assignment, AssignmentKind};
use super::decl::BoundIdentDecl;
use super::expression::{Expression, ExpressionKind};
use super::factory::FormulaFactory;
use super::predicate::{Predicate, PredicateKind};
use super::tag::{AssocExprOp, AtomicOp, BinaryExprOp, QuantExprOp, RelationalOp, UnaryExprOp};
use super::typenv::{InferredTypeEnvironment, SealedTypeEnvironment};
use super::types::Type;

use unifier::{TRef, TypeUnifier, UnifyError};

/// Why a formula failed to type-check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProblemKind {
    /// Two structurally incompatible types were required to be equal.
    TypesDoNotMatch,
    /// A type would have to contain itself.
    Circularity,
    /// The type of this expression could not be determined.
    UntypedExpression,
    /// The type of this free identifier could not be determined.
    UntypedIdentifier(String),
    /// A bound identifier without a matching declaration.
    DanglingBoundIdentifier,
    /// A user predicate application; there is no way to declare its
    /// operator, so it can never be checked.
    UncheckableApplication,
}

/// One type-check problem, anchored to the offending node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeCheckProblem {
    /// What went wrong.
    pub kind: ProblemKind,
    /// Where, if the node has a source span.
    pub span: Option<Span>,
}

/// The outcome of a type check.
#[derive(Debug, Clone)]
pub struct TypeCheckResult<T> {
    /// The fully typed rebuild of the formula; present iff there are
    /// no problems.
    pub typed: Option<T>,
    /// Types derived for initially-unknown free identifiers, in
    /// first-occurrence order; empty unless the check succeeded.
    pub inferred: InferredTypeEnvironment,
    /// Everything that went wrong.
    pub problems: Vec<TypeCheckProblem>,
}

impl<T> TypeCheckResult<T> {
    /// Whether the check succeeded.
    pub fn is_success(&self) -> bool {
        self.problems.is_empty()
    }
}

/// An opaque handle to a type being solved; what extension typing
/// rules receive for their expression children. Inference variables
/// never escape through it.
#[derive(Debug, Clone, Copy)]
pub struct TcType(TRef);

/// The interface extension typing rules use to state constraints.
pub struct TypeCheckMediator<'c, 'a> {
    checker: &'c mut Checker<'a>,
    span: Option<Span>,
}

impl TypeCheckMediator<'_, '_> {
    /// A fresh type unknown.
    pub fn fresh(&mut self) -> TcType {
        TcType(self.checker.uni.fresh())
    }

    /// A known type.
    pub fn from_type(&mut self, ty: &Type) -> TcType {
        TcType(self.checker.uni.lift(ty))
    }

    /// `ℙ(base)`
    pub fn pow(&mut self, base: TcType) -> TcType {
        TcType(self.checker.uni.pow(base.0))
    }

    /// `left × right`
    pub fn prod(&mut self, left: TcType, right: TcType) -> TcType {
        TcType(self.checker.uni.prod(left.0, right.0))
    }

    /// `ℙ(left × right)`
    pub fn relation(&mut self, left: TcType, right: TcType) -> TcType {
        TcType(self.checker.uni.relation(left.0, right.0))
    }

    /// An instance of a registered type constructor.
    pub fn parametric(
        &mut self,
        tag: super::tag::Tag,
        symbol: &str,
        params: Vec<TcType>,
    ) -> TcType {
        let params = params.into_iter().map(|p| p.0).collect();
        TcType(self.checker.uni.parametric(tag, symbol, params))
    }

    /// Requires two types to be equal; a conflict is reported against
    /// the extended node.
    pub fn same_type(&mut self, a: TcType, b: TcType) {
        let span = self.span;
        self.checker.expect(a.0, b.0, span);
    }
}

/// Interprets an expression as a type spelling: `ℤ`, `BOOL`, a free
/// identifier as a given set, `ℙ(·)`, products and relations, and a type
/// constructor of the expression's factory applied to type spellings
/// (`List(T)`) as the parametric type.
///
/// The accepted set is Rodin's, which pairs `Expression.isATypeExpression`
/// with `Expression.toType`: `ℙ1(·)` is a set constructor and not a type,
/// while `S ↔ T` is one and reads as `ℙ(S × T)`.
///
/// [`Expression::is_type_expression`] answers the same question about a
/// *type-checked* tree and must accept the same shapes; the two differ only
/// in the identifier rule, where it demands a solved `ℙ(Given(name))` and
/// this reads any name as a given set. Keep them in step.
pub fn type_from_expression(expr: &Expression) -> Option<Type> {
    match expr.kind() {
        ExpressionKind::Atomic(AtomicOp::Integer) => Some(Type::Int),
        ExpressionKind::Atomic(AtomicOp::Bool) => Some(Type::Bool),
        ExpressionKind::FreeIdentifier(name) => Some(Type::given(name.clone())),
        ExpressionKind::Unary {
            op: UnaryExprOp::Pow,
            child,
        } => Some(Type::pow(type_from_expression(child)?)),
        ExpressionKind::Binary {
            op: BinaryExprOp::CProd,
            left,
            right,
        } => Some(Type::prod(
            type_from_expression(left)?,
            type_from_expression(right)?,
        )),
        ExpressionKind::Binary {
            op: BinaryExprOp::Rel,
            left,
            right,
        } => Some(Type::relation(
            type_from_expression(left)?,
            type_from_expression(right)?,
        )),
        ExpressionKind::Extended { tag, exprs, preds } => {
            let super::extension::Extension::Expr(constructor) = expr.factory().extension(*tag)?
            else {
                return None;
            };
            if !constructor.is_a_type_constructor() || !preds.is_empty() {
                return None;
            }
            Some(Type::Parametric {
                tag: *tag,
                symbol: constructor.symbol().to_string(),
                params: exprs
                    .iter()
                    .map(type_from_expression)
                    .collect::<Option<Vec<_>>>()?,
            })
        }
        _ => None,
    }
}

impl Predicate {
    /// Type-checks this predicate against `env`.
    ///
    /// The outcome depends on `env` only through the bindings of the
    /// predicate's [free identifiers](Predicate::free_identifiers),
    /// which include the given sets its type annotations spell; a
    /// memo of checks may key on those bindings alone.
    pub fn type_check(&self, env: &SealedTypeEnvironment) -> TypeCheckResult<Predicate> {
        let mut checker = Checker::new(env);
        checker.check_pred(self);
        checker.finish(self.factory().clone(), |rebuilder| {
            rebuilder.rebuild_pred(self)
        })
    }
}

impl Expression {
    /// Type-checks this expression against `env`.
    ///
    /// As for predicates, the outcome depends on `env` only through the
    /// bindings of the expression's
    /// [free identifiers](Expression::free_identifiers).
    pub fn type_check(&self, env: &SealedTypeEnvironment) -> TypeCheckResult<Expression> {
        let mut checker = Checker::new(env);
        checker.check_expr(self);
        checker.finish(self.factory().clone(), |rebuilder| {
            rebuilder.rebuild_expr(self)
        })
    }

    /// Type-checks this expression against `env`, requiring it to have
    /// the given type.
    pub fn type_check_with_expected(
        &self,
        env: &SealedTypeEnvironment,
        expected: &Type,
    ) -> TypeCheckResult<Expression> {
        let mut checker = Checker::new(env);
        let actual = checker.check_expr(self);
        let expected = checker.uni.lift(expected);
        checker.expect(actual, expected, self.span());
        checker.finish(self.factory().clone(), |rebuilder| {
            rebuilder.rebuild_expr(self)
        })
    }
}

impl Assignment {
    /// Type-checks this assignment against `env`. Each target's type
    /// comes from the environment (or is inferred); values, sets and
    /// before-after predicates are constrained accordingly, with the
    /// primed declarations of a such-that assignment sharing their
    /// target's type.
    pub fn type_check(&self, env: &SealedTypeEnvironment) -> TypeCheckResult<Assignment> {
        let mut checker = Checker::new(env);
        checker.check_assign(self);
        checker.finish(self.factory().clone(), |rebuilder| {
            rebuilder.rebuild_assign(self)
        })
    }
}

/// Pass 1: constraint generation.
struct Checker<'a> {
    uni: TypeUnifier,
    env: &'a SealedTypeEnvironment,
    /// Fresh variables for initially-unknown free identifiers, in
    /// first-occurrence order.
    inferred: Vec<(String, TRef, Option<Span>)>,
    inferred_index: HashMap<String, usize>,
    /// Environment types already lifted into the arena, by name.
    /// Environment types are ground, so every occurrence of a name can
    /// share one lifted tree instead of re-copying it per occurrence.
    env_lifted: HashMap<String, TRef>,
    /// Types of the enclosing declarations, innermost last.
    bound: Vec<TRef>,
    /// One slot per expression node and declaration, in traversal
    /// order; pass 2 consumes them in lockstep.
    shadow: Vec<(TRef, Option<Span>)>,
    problems: Vec<TypeCheckProblem>,
}

impl<'a> Checker<'a> {
    fn new(env: &'a SealedTypeEnvironment) -> Self {
        Checker {
            uni: TypeUnifier::new(),
            env,
            inferred: Vec::new(),
            inferred_index: HashMap::new(),
            env_lifted: HashMap::new(),
            bound: Vec::new(),
            shadow: Vec::new(),
            problems: Vec::new(),
        }
    }

    fn problem(&mut self, kind: ProblemKind, span: Option<Span>) {
        self.problems.push(TypeCheckProblem { kind, span });
    }

    /// Requires `a = b`, recording a problem at `span` on failure.
    fn expect(&mut self, a: TRef, b: TRef, span: Option<Span>) {
        match self.uni.unify(a, b) {
            Ok(()) => {}
            Err(UnifyError::Mismatch) => self.problem(ProblemKind::TypesDoNotMatch, span),
            Err(UnifyError::Circular) => self.problem(ProblemKind::Circularity, span),
        }
    }

    fn free_identifier(&mut self, name: &str, span: Option<Span>) -> TRef {
        if let Some(ty) = self.env.get(name) {
            if let Some(&lifted) = self.env_lifted.get(name) {
                return lifted;
            }
            let lifted = self.uni.lift(ty);
            self.env_lifted.insert(name.to_string(), lifted);
            return lifted;
        }
        if let Some(index) = self.inferred_index.get(name) {
            return self.inferred[*index].1;
        }
        let fresh = self.uni.fresh();
        self.inferred_index
            .insert(name.to_string(), self.inferred.len());
        self.inferred.push((name.to_string(), fresh, span));
        fresh
    }

    /// A type spelling's given-set names are identifier references:
    /// any name the environment does not know goes through the same
    /// inference channel as an unknown free identifier, so a
    /// misspelled carrier set in a `⦂` annotation or ascription
    /// surfaces as an inferred entry (which strict callers reject)
    /// instead of silently minting a phantom type.
    fn note_spelled_givens(&mut self, spelled: &Type, span: Option<Span>) {
        let mut givens = Vec::new();
        spelled.collect_given_sets(&mut givens);
        for name in givens {
            if self.env.get(&name).is_none() {
                let t = self.free_identifier(&name, span);
                let set_ty = Type::pow(Type::given(name));
                let lifted = self.uni.lift(&set_ty);
                self.expect(t, lifted, span);
            }
        }
    }

    /// Declares the binders of a quantified construct: each gets a
    /// shadow slot and its optional declared type or annotation as a
    /// constraint. Returns the refs, in declaration order.
    fn declare(&mut self, decls: &[BoundIdentDecl]) -> Vec<TRef> {
        decls
            .iter()
            .map(|decl| {
                let t = self.uni.fresh();
                self.shadow.push((t, decl.span()));
                if let Some(ty) = decl.ty() {
                    let declared = self.uni.lift(ty);
                    self.expect(t, declared, decl.span());
                } else if let Some(spelled) = decl.annotation().and_then(type_from_expression) {
                    self.note_spelled_givens(&spelled, decl.span());
                    let spelled = self.uni.lift(&spelled);
                    self.expect(t, spelled, decl.span());
                }
                t
            })
            .collect()
    }

    fn check_expr(&mut self, e: &Expression) -> TRef {
        let slot = self.shadow.len();
        self.shadow.push((0, e.span()));
        let t = self.check_expr_inner(e);
        self.shadow[slot].0 = t;
        // An explicit type on the node is a constraint too (ascribed
        // generic atoms, hand-built typed leaves).
        if let Some(ty) = e.ty() {
            let declared = self.uni.lift(ty);
            self.expect(t, declared, e.span());
        }
        t
    }

    fn check_expr_inner(&mut self, e: &Expression) -> TRef {
        let span = e.span();
        match e.kind() {
            ExpressionKind::FreeIdentifier(name) => self.free_identifier(name, span),
            ExpressionKind::BoundIdentifier(index) => {
                let index = *index as usize;
                if index < self.bound.len() {
                    self.bound[self.bound.len() - 1 - index]
                } else {
                    self.problem(ProblemKind::DanglingBoundIdentifier, span);
                    self.uni.fresh()
                }
            }
            ExpressionKind::IntegerLiteral(_) => self.uni.int(),
            ExpressionKind::Atomic(op) => self.check_atomic(*op),
            ExpressionKind::SetExtension(members) => {
                let alpha = self.uni.fresh();
                for member in members {
                    let t = self.check_expr(member);
                    self.expect(t, alpha, member.span());
                }
                self.uni.pow(alpha)
            }
            ExpressionKind::Bool(pred) => {
                self.check_pred(pred);
                self.uni.bool()
            }
            ExpressionKind::Binary { op, left, right } => {
                let l = self.check_expr(left);
                let r = self.check_expr(right);
                self.check_binary(*op, l, r, span)
            }
            ExpressionKind::Associative { op, children } => self.check_associative(*op, children),
            ExpressionKind::Unary { op, child } => {
                let c = self.check_expr(child);
                self.check_unary(*op, c, child.span())
            }
            ExpressionKind::Quantified {
                op,
                decls,
                pred,
                expr,
                ..
            } => {
                let binders = self.declare(decls);
                self.bound.extend(&binders);
                self.check_pred(pred);
                let value = self.check_expr(expr);
                self.bound.truncate(self.bound.len() - binders.len());
                match op {
                    QuantExprOp::QUnion | QuantExprOp::QInter => {
                        let alpha = self.uni.fresh();
                        let set = self.uni.pow(alpha);
                        self.expect(value, set, expr.span());
                        set
                    }
                    QuantExprOp::CSet => self.uni.pow(value),
                }
            }
            ExpressionKind::Ascription { expr, type_expr } => {
                let t = self.check_expr(expr);
                match type_from_expression(type_expr) {
                    Some(spelled) => {
                        self.note_spelled_givens(&spelled, type_expr.span());
                        let spelled = self.uni.lift(&spelled);
                        self.expect(t, spelled, span);
                    }
                    None => self.problem(ProblemKind::TypesDoNotMatch, type_expr.span()),
                }
                t
            }
            ExpressionKind::Extended { tag, exprs, preds } => {
                let extension = match e.factory().extension(*tag) {
                    Some(super::extension::Extension::Expr(ext)) => ext.clone(),
                    _ => unreachable!("extended nodes carry a registered extension"),
                };
                let child_refs: Vec<TcType> =
                    exprs.iter().map(|c| TcType(self.check_expr(c))).collect();
                for pred in preds {
                    self.check_pred(pred);
                }
                let mut mediator = TypeCheckMediator {
                    checker: self,
                    span,
                };
                extension.type_check(&mut mediator, &child_refs).0
            }
        }
    }

    fn check_atomic(&mut self, op: AtomicOp) -> TRef {
        use AtomicOp as Op;
        match op {
            Op::Integer | Op::Natural | Op::Natural1 => {
                let int = self.uni.int();
                self.uni.pow(int)
            }
            Op::Bool => {
                let boolean = self.uni.bool();
                self.uni.pow(boolean)
            }
            Op::True | Op::False => self.uni.bool(),
            Op::KPred | Op::KSucc => {
                let a = self.uni.int();
                let b = self.uni.int();
                self.uni.relation(a, b)
            }
            Op::EmptySet => {
                let alpha = self.uni.fresh();
                self.uni.pow(alpha)
            }
            Op::KIdGen => {
                let alpha = self.uni.fresh();
                self.uni.relation(alpha, alpha)
            }
            Op::KPrj1Gen => {
                let alpha = self.uni.fresh();
                let beta = self.uni.fresh();
                let pair = self.uni.prod(alpha, beta);
                self.uni.relation(pair, alpha)
            }
            Op::KPrj2Gen => {
                let alpha = self.uni.fresh();
                let beta = self.uni.fresh();
                let pair = self.uni.prod(alpha, beta);
                self.uni.relation(pair, beta)
            }
        }
    }

    fn check_binary(&mut self, op: BinaryExprOp, l: TRef, r: TRef, span: Option<Span>) -> TRef {
        use BinaryExprOp as Op;
        match op {
            Op::Mapsto => self.uni.prod(l, r),
            Op::Rel
            | Op::TRel
            | Op::SRel
            | Op::STRel
            | Op::PFun
            | Op::TFun
            | Op::PInj
            | Op::TInj
            | Op::PSur
            | Op::TSur
            | Op::TBij => {
                let alpha = self.uni.fresh();
                let beta = self.uni.fresh();
                let left_set = self.uni.pow(alpha);
                let right_set = self.uni.pow(beta);
                self.expect(l, left_set, span);
                self.expect(r, right_set, span);
                let rel = self.uni.relation(alpha, beta);
                self.uni.pow(rel)
            }
            Op::SetMinus => {
                let alpha = self.uni.fresh();
                let set = self.uni.pow(alpha);
                self.expect(l, set, span);
                self.expect(r, set, span);
                set
            }
            Op::CProd => {
                let alpha = self.uni.fresh();
                let beta = self.uni.fresh();
                let left_set = self.uni.pow(alpha);
                let right_set = self.uni.pow(beta);
                self.expect(l, left_set, span);
                self.expect(r, right_set, span);
                self.uni.relation(alpha, beta)
            }
            Op::DProd => {
                let alpha = self.uni.fresh();
                let beta = self.uni.fresh();
                let gamma = self.uni.fresh();
                let left_rel = self.uni.relation(alpha, beta);
                let right_rel = self.uni.relation(alpha, gamma);
                self.expect(l, left_rel, span);
                self.expect(r, right_rel, span);
                let pair = self.uni.prod(beta, gamma);
                self.uni.relation(alpha, pair)
            }
            Op::PProd => {
                let alpha = self.uni.fresh();
                let beta = self.uni.fresh();
                let gamma = self.uni.fresh();
                let delta = self.uni.fresh();
                let left_rel = self.uni.relation(alpha, beta);
                let right_rel = self.uni.relation(gamma, delta);
                self.expect(l, left_rel, span);
                self.expect(r, right_rel, span);
                let sources = self.uni.prod(alpha, gamma);
                let targets = self.uni.prod(beta, delta);
                self.uni.relation(sources, targets)
            }
            Op::DomRes | Op::DomSub => {
                let alpha = self.uni.fresh();
                let beta = self.uni.fresh();
                let set = self.uni.pow(alpha);
                let rel = self.uni.relation(alpha, beta);
                self.expect(l, set, span);
                self.expect(r, rel, span);
                rel
            }
            Op::RanRes | Op::RanSub => {
                let alpha = self.uni.fresh();
                let beta = self.uni.fresh();
                let rel = self.uni.relation(alpha, beta);
                let set = self.uni.pow(beta);
                self.expect(l, rel, span);
                self.expect(r, set, span);
                rel
            }
            Op::UpTo => {
                let int = self.uni.int();
                self.expect(l, int, span);
                self.expect(r, int, span);
                self.uni.pow(int)
            }
            Op::Minus | Op::Div | Op::Mod | Op::Expn => {
                let int = self.uni.int();
                self.expect(l, int, span);
                self.expect(r, int, span);
                int
            }
            Op::FunImage => {
                let alpha = self.uni.fresh();
                let beta = self.uni.fresh();
                let rel = self.uni.relation(alpha, beta);
                self.expect(l, rel, span);
                self.expect(r, alpha, span);
                beta
            }
            Op::RelImage => {
                let alpha = self.uni.fresh();
                let beta = self.uni.fresh();
                let rel = self.uni.relation(alpha, beta);
                let set = self.uni.pow(alpha);
                self.expect(l, rel, span);
                self.expect(r, set, span);
                self.uni.pow(beta)
            }
        }
    }

    fn check_associative(&mut self, op: AssocExprOp, children: &[Expression]) -> TRef {
        use AssocExprOp as Op;
        match op {
            Op::BUnion | Op::BInter => {
                let alpha = self.uni.fresh();
                let set = self.uni.pow(alpha);
                for child in children {
                    let t = self.check_expr(child);
                    self.expect(t, set, child.span());
                }
                set
            }
            Op::Ovr => {
                let alpha = self.uni.fresh();
                let beta = self.uni.fresh();
                let rel = self.uni.relation(alpha, beta);
                for child in children {
                    let t = self.check_expr(child);
                    self.expect(t, rel, child.span());
                }
                rel
            }
            Op::FComp => {
                // p ; q ; r — targets chain into sources left to right.
                let mut source = self.uni.fresh();
                let first_source = source;
                for child in children {
                    let target = self.uni.fresh();
                    let rel = self.uni.relation(source, target);
                    let t = self.check_expr(child);
                    self.expect(t, rel, child.span());
                    source = target;
                }
                self.uni.relation(first_source, source)
            }
            Op::BComp => {
                // p ∘ q ∘ r — the rightmost applies first; targets
                // chain into sources right to left.
                let mut target = self.uni.fresh();
                let first_target = target;
                for child in children {
                    let source = self.uni.fresh();
                    let rel = self.uni.relation(source, target);
                    let t = self.check_expr(child);
                    self.expect(t, rel, child.span());
                    target = source;
                }
                self.uni.relation(target, first_target)
            }
            Op::Plus | Op::Mul => {
                let int = self.uni.int();
                for child in children {
                    let t = self.check_expr(child);
                    self.expect(t, int, child.span());
                }
                int
            }
        }
    }

    fn check_unary(&mut self, op: UnaryExprOp, c: TRef, child_span: Option<Span>) -> TRef {
        use UnaryExprOp as Op;
        match op {
            Op::KCard => {
                let alpha = self.uni.fresh();
                let set = self.uni.pow(alpha);
                self.expect(c, set, child_span);
                self.uni.int()
            }
            Op::Pow | Op::Pow1 => {
                let alpha = self.uni.fresh();
                let set = self.uni.pow(alpha);
                self.expect(c, set, child_span);
                self.uni.pow(set)
            }
            Op::KUnion | Op::KInter => {
                let alpha = self.uni.fresh();
                let set = self.uni.pow(alpha);
                let family = self.uni.pow(set);
                self.expect(c, family, child_span);
                set
            }
            Op::KDom => {
                let alpha = self.uni.fresh();
                let beta = self.uni.fresh();
                let rel = self.uni.relation(alpha, beta);
                self.expect(c, rel, child_span);
                self.uni.pow(alpha)
            }
            Op::KRan => {
                let alpha = self.uni.fresh();
                let beta = self.uni.fresh();
                let rel = self.uni.relation(alpha, beta);
                self.expect(c, rel, child_span);
                self.uni.pow(beta)
            }
            Op::KMin | Op::KMax => {
                let int = self.uni.int();
                let set = self.uni.pow(int);
                self.expect(c, set, child_span);
                int
            }
            Op::Converse => {
                let alpha = self.uni.fresh();
                let beta = self.uni.fresh();
                let rel = self.uni.relation(alpha, beta);
                self.expect(c, rel, child_span);
                self.uni.relation(beta, alpha)
            }
            Op::UnMinus => {
                let int = self.uni.int();
                self.expect(c, int, child_span);
                int
            }
        }
    }

    fn check_pred(&mut self, p: &Predicate) {
        let span = p.span();
        match p.kind() {
            PredicateKind::Literal(_) | PredicateKind::PredicateVariable(_) => {}
            PredicateKind::Relational { op, left, right } => {
                let l = self.check_expr(left);
                let r = self.check_expr(right);
                use RelationalOp as Op;
                match op {
                    Op::Equal | Op::NotEqual => self.expect(l, r, span),
                    Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                        let int = self.uni.int();
                        self.expect(l, int, left.span());
                        self.expect(r, int, right.span());
                    }
                    Op::In | Op::NotIn => {
                        let set = self.uni.pow(l);
                        self.expect(r, set, span);
                    }
                    Op::Subset | Op::NotSubset | Op::SubsetEq | Op::NotSubsetEq => {
                        let alpha = self.uni.fresh();
                        let set = self.uni.pow(alpha);
                        self.expect(l, set, left.span());
                        self.expect(r, set, right.span());
                        self.expect(l, r, span);
                    }
                }
            }
            PredicateKind::Binary { left, right, .. } => {
                self.check_pred(left);
                self.check_pred(right);
            }
            PredicateKind::Associative { children, .. } => {
                for child in children {
                    self.check_pred(child);
                }
            }
            PredicateKind::Not(child) => self.check_pred(child),
            PredicateKind::Quantified { decls, pred, .. } => {
                let binders = self.declare(decls);
                self.bound.extend(&binders);
                self.check_pred(pred);
                self.bound.truncate(self.bound.len() - binders.len());
            }
            PredicateKind::Simple(child) => {
                let t = self.check_expr(child);
                let alpha = self.uni.fresh();
                let set = self.uni.pow(alpha);
                self.expect(t, set, child.span());
            }
            PredicateKind::Multiple(children) => {
                let alpha = self.uni.fresh();
                let set = self.uni.pow(alpha);
                for child in children {
                    let t = self.check_expr(child);
                    self.expect(t, set, child.span());
                }
            }
            PredicateKind::Application { args, .. } => {
                for arg in args {
                    self.check_expr(arg);
                }
                self.problem(ProblemKind::UncheckableApplication, span);
            }
            PredicateKind::Extended { tag, exprs, preds } => {
                let extension = match p.factory().extension(*tag) {
                    Some(super::extension::Extension::Pred(ext)) => ext.clone(),
                    _ => unreachable!("extended nodes carry a registered extension"),
                };
                let child_refs: Vec<TcType> =
                    exprs.iter().map(|c| TcType(self.check_expr(c))).collect();
                for pred in preds {
                    self.check_pred(pred);
                }
                let mut mediator = TypeCheckMediator {
                    checker: self,
                    span,
                };
                extension.type_check(&mut mediator, &child_refs);
            }
        }
    }

    fn check_assign(&mut self, a: &Assignment) {
        match a.kind() {
            AssignmentKind::BecomesEqualTo { idents, values } => {
                for (ident, value) in idents.iter().zip(values) {
                    let target = self.check_expr(ident);
                    let v = self.check_expr(value);
                    self.expect(v, target, value.span());
                }
            }
            AssignmentKind::BecomesMemberOf { idents, set } => {
                let mut product = None;
                for ident in idents {
                    let t = self.check_expr(ident);
                    product = Some(match product {
                        None => t,
                        Some(acc) => self.uni.prod(acc, t),
                    });
                }
                let element = product.expect("at least one target");
                let expected = self.uni.pow(element);
                let s = self.check_expr(set);
                self.expect(s, expected, set.span());
            }
            AssignmentKind::BecomesSuchThat {
                idents,
                primed,
                pred,
            } => {
                let targets: Vec<TRef> = idents.iter().map(|i| self.check_expr(i)).collect();
                let binders = self.declare(primed);
                // Each primed declaration shares its target's type.
                for (target, primed_ref) in targets.iter().zip(&binders) {
                    self.expect(*primed_ref, *target, a.span());
                }
                self.bound.extend(&binders);
                self.check_pred(pred);
                self.bound.truncate(self.bound.len() - binders.len());
            }
        }
    }

    /// Solves, reports unsolved slots, and runs pass 2 on success.
    fn finish<T>(
        mut self,
        ff: FormulaFactory,
        rebuild: impl FnOnce(&mut Rebuilder<'_>) -> T,
    ) -> TypeCheckResult<T> {
        let mut inferred_env = InferredTypeEnvironment::default();
        for (name, t, span) in &self.inferred {
            match self.uni.solve(*t) {
                Some(ty) => {
                    inferred_env.push(name.clone(), ty);
                }
                None => {
                    self.problems.push(TypeCheckProblem {
                        kind: ProblemKind::UntypedIdentifier(name.clone()),
                        span: *span,
                    });
                }
            }
        }
        if self.problems.is_empty() {
            for (t, span) in &self.shadow {
                if !self.uni.is_solved(*t) {
                    self.problems.push(TypeCheckProblem {
                        kind: ProblemKind::UntypedExpression,
                        span: *span,
                    });
                }
            }
        }
        if !self.problems.is_empty() {
            return TypeCheckResult {
                typed: None,
                inferred: InferredTypeEnvironment::default(),
                problems: self.problems,
            };
        }
        let mut rebuilder = Rebuilder {
            uni: &self.uni,
            shadow: &self.shadow,
            cursor: 0,
            ff,
        };
        let typed = rebuild(&mut rebuilder);
        debug_assert_eq!(rebuilder.cursor, self.shadow.len());
        TypeCheckResult {
            typed: Some(typed),
            inferred: inferred_env,
            problems: Vec::new(),
        }
    }
}

/// Pass 2: rebuilds the formula with solved types, consuming the
/// shadow slots in the same traversal order as pass 1.
struct Rebuilder<'a> {
    uni: &'a TypeUnifier,
    shadow: &'a [(TRef, Option<Span>)],
    cursor: usize,
    ff: FormulaFactory,
}

impl Rebuilder<'_> {
    /// Consumes the next shadow slot, returning its ref for the arms
    /// that spell the type out. The compound arms drop it: their
    /// factory constructors re-synthesize the type from the (typed)
    /// children, so materializing the solved tree for them is waste.
    fn next_slot(&mut self) -> TRef {
        let (t, _) = self.shadow[self.cursor];
        self.cursor += 1;
        t
    }

    fn solved(&self, slot: TRef) -> Type {
        self.uni.solve(slot).expect("pass 2 runs only when solved")
    }

    fn rebuild_decls(&mut self, decls: &[BoundIdentDecl]) -> Vec<BoundIdentDecl> {
        decls
            .iter()
            .map(|decl| {
                let slot = self.next_slot();
                let ty = self.solved(slot);
                self.ff.bound_ident_decl(
                    decl.name(),
                    decl.span(),
                    decl.annotation().cloned(),
                    Some(ty),
                )
            })
            .collect()
    }

    fn rebuild_expr(&mut self, e: &Expression) -> Expression {
        let slot = self.next_slot();
        let span = e.span();
        match e.kind() {
            ExpressionKind::FreeIdentifier(name) => {
                let solved = self.solved(slot);
                self.ff.free_identifier(name, span, Some(solved))
            }
            ExpressionKind::BoundIdentifier(index) => {
                let solved = self.solved(slot);
                self.ff.bound_identifier(*index, span, Some(solved))
            }
            ExpressionKind::IntegerLiteral(value) => self.ff.integer_literal(value.clone(), span),
            ExpressionKind::Atomic(op) => {
                let solved = self.solved(slot);
                self.ff.atomic_expression(*op, span, Some(solved))
            }
            ExpressionKind::SetExtension(members) => {
                let members = members.iter().map(|m| self.rebuild_expr(m)).collect();
                self.ff.set_extension(members, span)
            }
            ExpressionKind::Bool(pred) => {
                let pred = self.rebuild_pred(pred);
                self.ff.bool_expression(pred, span)
            }
            ExpressionKind::Binary { op, left, right } => {
                let left = self.rebuild_expr(left);
                let right = self.rebuild_expr(right);
                self.ff.binary_expression(*op, left, right, span)
            }
            ExpressionKind::Associative { op, children } => {
                let children = children.iter().map(|c| self.rebuild_expr(c)).collect();
                self.ff.associative_expression(*op, children, span)
            }
            ExpressionKind::Unary { op, child } => {
                let child = self.rebuild_expr(child);
                self.ff.unary_expression(*op, child, span)
            }
            ExpressionKind::Quantified {
                op,
                decls,
                pred,
                expr,
                form,
            } => {
                let new_decls = self.rebuild_decls(decls);
                let pred = self.rebuild_pred(pred);
                let expr = self.rebuild_expr(expr);
                self.ff
                    .quantified_expression(*op, new_decls, pred, expr, span, *form)
            }
            ExpressionKind::Ascription { expr, type_expr } => {
                let expr = self.rebuild_expr(expr);
                // The spelled type is presentation; it stays verbatim.
                self.ff.ascription(expr, type_expr.clone(), span)
            }
            ExpressionKind::Extended { tag, exprs, preds } => {
                let extension = match self.ff.extension(*tag) {
                    Some(super::extension::Extension::Expr(ext)) => ext.clone(),
                    _ => unreachable!("extended nodes carry a registered extension"),
                };
                let exprs = exprs.iter().map(|c| self.rebuild_expr(c)).collect();
                let preds = preds.iter().map(|c| self.rebuild_pred(c)).collect();
                let solved = self.solved(slot);
                self.ff
                    .extended_expression(&extension, exprs, preds, span, Some(solved))
                    .expect("a checked construction fits its extension")
            }
        }
    }

    fn rebuild_pred(&mut self, p: &Predicate) -> Predicate {
        let span = p.span();
        match p.kind() {
            PredicateKind::Literal(op) => self.ff.literal_predicate(*op, span),
            PredicateKind::PredicateVariable(name) => self.ff.predicate_variable(name, span),
            PredicateKind::Relational { op, left, right } => {
                let left = self.rebuild_expr(left);
                let right = self.rebuild_expr(right);
                self.ff.relational_predicate(*op, left, right, span)
            }
            PredicateKind::Binary { op, left, right } => {
                let left = self.rebuild_pred(left);
                let right = self.rebuild_pred(right);
                self.ff.binary_predicate(*op, left, right, span)
            }
            PredicateKind::Associative { op, children } => {
                let children = children.iter().map(|c| self.rebuild_pred(c)).collect();
                self.ff.associative_predicate(*op, children, span)
            }
            PredicateKind::Not(child) => {
                let child = self.rebuild_pred(child);
                self.ff.not_predicate(child, span)
            }
            PredicateKind::Quantified { op, decls, pred } => {
                let new_decls = self.rebuild_decls(decls);
                let pred = self.rebuild_pred(pred);
                self.ff.quantified_predicate(*op, new_decls, pred, span)
            }
            PredicateKind::Simple(child) => {
                let child = self.rebuild_expr(child);
                self.ff.simple_predicate(child, span)
            }
            PredicateKind::Multiple(children) => {
                let children = children.iter().map(|c| self.rebuild_expr(c)).collect();
                self.ff.multiple_predicate(children, span)
            }
            PredicateKind::Application { .. } => {
                unreachable!("applications never type-check")
            }
            PredicateKind::Extended { tag, exprs, preds } => {
                let extension = match self.ff.extension(*tag) {
                    Some(super::extension::Extension::Pred(ext)) => ext.clone(),
                    _ => unreachable!("extended nodes carry a registered extension"),
                };
                let exprs = exprs.iter().map(|c| self.rebuild_expr(c)).collect();
                let preds = preds.iter().map(|c| self.rebuild_pred(c)).collect();
                self.ff
                    .extended_predicate(&extension, exprs, preds, span)
                    .expect("a checked construction fits its extension")
            }
        }
    }

    fn rebuild_assign(&mut self, a: &Assignment) -> Assignment {
        let span = a.span();
        match a.kind() {
            AssignmentKind::BecomesEqualTo { idents, values } => {
                // Pass 1 interleaved target/value; consume in the same
                // order.
                let mut new_idents = Vec::with_capacity(idents.len());
                let mut new_values = Vec::with_capacity(values.len());
                for (ident, value) in idents.iter().zip(values) {
                    new_idents.push(self.rebuild_expr(ident));
                    new_values.push(self.rebuild_expr(value));
                }
                self.ff.becomes_equal_to(new_idents, new_values, span)
            }
            AssignmentKind::BecomesMemberOf { idents, set } => {
                let idents = idents.iter().map(|i| self.rebuild_expr(i)).collect();
                let set = self.rebuild_expr(set);
                self.ff.becomes_member_of(idents, set, span)
            }
            AssignmentKind::BecomesSuchThat {
                idents,
                primed,
                pred,
            } => {
                let new_idents: Vec<Expression> =
                    idents.iter().map(|i| self.rebuild_expr(i)).collect();
                let new_primed = self.rebuild_decls(primed);
                let pred = self.rebuild_pred(pred);
                self.ff
                    .becomes_such_that(new_idents, new_primed, pred, span)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formula::TypeEnvironmentBuilder;

    /// The contract memoizing callers rely on: a check observes only
    /// the bindings of the formula's free identifiers — including a
    /// given set spelled in an annotation — so unrelated bindings do
    /// not change its outcome.
    #[test]
    fn outcome_depends_only_on_free_identifier_bindings() {
        let pred = crate::parse_predicate_str("∀y⦂S·x∈T ∧ y∈T").expect("parses");
        assert_eq!(pred.free_identifiers(), ["S", "T", "x"]);
        let mut env = TypeEnvironmentBuilder::new();
        env.add_given_set("S");
        env.insert("T", Type::pow(Type::given("S")));
        env.insert("x", Type::given("S"));
        let narrow = env.make_snapshot();
        env.add_given_set("U");
        env.insert("z", Type::Int);
        let wide = env.make_snapshot();

        let checked = pred.type_check(&narrow).typed.expect("type-checks");
        assert_eq!(pred.type_check(&wide).typed, Some(checked));
    }
}
