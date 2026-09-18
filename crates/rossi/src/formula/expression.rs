//! Expression nodes.

use std::sync::Arc;

use num_bigint::BigInt;

use crate::ast::Span;

use super::decl::BoundIdentDecl;
use super::factory::FormulaFactory;
use super::hashing::{combine, fold, hash_one};
use super::predicate::Predicate;
use super::tag::{self, AssocExprOp, AtomicOp, BinaryExprOp, QuantExprOp, Tag, UnaryExprOp};
use super::types::Type;

/// An immutable, possibly typed expression.
///
/// Cloning is O(1): the node is a shared handle. Equality is structural
/// (spans never participate; solved types do) with alpha-equivalence
/// for quantified subterms, and starts with a pointer, then a
/// cached-hash fast path.
#[derive(Debug, Clone)]
pub struct Expression(pub(super) Arc<ExprData>);

#[derive(Debug)]
pub(super) struct ExprData {
    pub(super) kind: ExpressionKind,
    pub(super) ty: Option<Type>,
    pub(super) span: Option<Span>,
    pub(super) hash: u64,
    pub(super) free_idents: Box<[String]>,
    pub(super) dangling: Box<[u32]>,
    pub(super) factory: FormulaFactory,
}

/// The structural kind of an [`Expression`].
///
/// Kinds are public for pattern matching, but expressions can only be
/// constructed through a [`FormulaFactory`], which owns the structural
/// invariants (child counts, binder arity, form validation).
#[derive(Debug)]
pub enum ExpressionKind {
    /// A free identifier occurrence, e.g. `x` or the primed `x'`.
    FreeIdentifier(String),
    /// A bound identifier occurrence, as a de Bruijn index: 0 refers to
    /// the innermost enclosing declaration. Within one quantifier's
    /// declaration list, index 0 maps to the *last* declaration.
    BoundIdentifier(u32),
    /// An integer literal of arbitrary precision.
    IntegerLiteral(BigInt),
    /// A set defined in extension: `{a, b, c}`. Never empty — a typed
    /// empty set is the `∅` atom.
    SetExtension(Vec<Expression>),
    /// A nullary operator, e.g. `ℤ`, `∅`, `TRUE`, `succ`.
    Atomic(AtomicOp),
    /// `bool(P)` — the predicate reified as a boolean expression.
    Bool(Predicate),
    /// A binary operator, e.g. `x ↦ y`, `a − b`, `f(x)`.
    Binary {
        /// The operator.
        op: BinaryExprOp,
        /// The left operand.
        left: Expression,
        /// The right operand.
        right: Expression,
    },
    /// An associative operator with two or more children, e.g.
    /// `a + b + c`.
    Associative {
        /// The operator.
        op: AssocExprOp,
        /// The children, in source order; always at least two.
        children: Vec<Expression>,
    },
    /// A unary operator, e.g. `card(S)`, `ℙ(S)`, `−x`, `r∼`.
    Unary {
        /// The operator.
        op: UnaryExprOp,
        /// The operand.
        child: Expression,
    },
    /// A quantified expression: comprehension set, quantified union or
    /// quantified intersection.
    Quantified {
        /// The operator.
        op: QuantExprOp,
        /// The bound declarations; always at least one. Index 0 in the
        /// body refers to the last declaration.
        decls: Vec<BoundIdentDecl>,
        /// The constraining predicate, scoped under the declarations.
        pred: Predicate,
        /// The value expression, scoped under the declarations.
        expr: Expression,
        /// How the expression prints; no mathematical meaning.
        form: Form,
    },
    /// A type ascription `E ⦂ T`.
    ///
    /// The right operand is the source spelling of the ascribed type; it
    /// is interpreted as a type by the type-checker. Ascriptions on
    /// arbitrary expressions are accepted (a deliberate leniency of this
    /// dialect) and preserved for printing.
    Ascription {
        /// The ascribed expression.
        expr: Expression,
        /// The type, spelled as an expression.
        type_expr: Expression,
    },
    /// An occurrence of a registered operator extension.
    Extended {
        /// The extension's dynamic tag (`>= FIRST_EXTENSION_TAG`).
        tag: Tag,
        /// Expression children, before all predicate children.
        exprs: Vec<Expression>,
        /// Predicate children.
        preds: Vec<Predicate>,
    },
}

/// The print form of a quantified expression. Presentation only: it
/// never affects the meaning, equality, or hash of the expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// `{x, y · P ∣ E}` — declarations, predicate and expression all
    /// spelled out. Every quantified expression can print this way.
    Explicit,
    /// `{E ∣ P}` — the declarations are implied by the expression,
    /// which must reference exactly the locally bound identifiers and
    /// nothing else.
    Implicit,
    /// `{x ∣ P}` — only the declaration and the predicate are spelled;
    /// the expression is the declared identifier itself.
    IdentList,
    /// `λ pattern · P ∣ E` — the expression is `pattern ↦ E` where the
    /// pattern is a maplet tree over exactly the bound identifiers.
    Lambda,
}

impl Expression {
    /// The structural kind, for pattern matching.
    pub fn kind(&self) -> &ExpressionKind {
        &self.0.kind
    }

    /// The node's numeric tag.
    pub fn tag(&self) -> Tag {
        kind_tag(&self.0.kind)
    }

    /// The solved type, if the expression has been type-checked (or was
    /// built from type-checked parts).
    pub fn ty(&self) -> Option<&Type> {
        self.0.ty.as_ref()
    }

    /// The source span, if the expression came from source text.
    pub fn span(&self) -> Option<Span> {
        self.0.span
    }

    /// Whether the expression carries a solved type.
    pub fn is_type_checked(&self) -> bool {
        self.0.ty.is_some()
    }

    /// Whether this expression denotes a type: `ℤ`, `BOOL`, a carrier
    /// set, a power set or relation or cartesian product of type
    /// expressions, or a parametric type built from a type constructor.
    ///
    /// This reads a *type-checked* tree, so a carrier set must already
    /// carry `ℙ(Given(name))`. Before type-checking — in the parser, say —
    /// `typecheck::type_from_expression` answers the same question about
    /// the same shapes without that requirement.
    pub fn is_type_expression(&self) -> bool {
        match self.kind() {
            ExpressionKind::Atomic(AtomicOp::Integer | AtomicOp::Bool) => true,
            ExpressionKind::FreeIdentifier(name) => match self.ty() {
                Some(Type::Pow(base)) => {
                    matches!(base.as_ref(), Type::Given(given) if given == name)
                }
                _ => false,
            },
            ExpressionKind::Unary {
                op: UnaryExprOp::Pow,
                child,
            } => child.is_type_expression(),
            ExpressionKind::Binary {
                op: BinaryExprOp::CProd | BinaryExprOp::Rel,
                left,
                right,
            } => left.is_type_expression() && right.is_type_expression(),
            ExpressionKind::Extended { tag, exprs, preds } => {
                let Some(super::extension::Extension::Expr(extension)) =
                    self.factory().extension(*tag)
                else {
                    return false;
                };
                extension.is_a_type_constructor()
                    && preds.is_empty()
                    && exprs.iter().all(Expression::is_type_expression)
            }
            _ => false,
        }
    }

    /// The factory this expression was built with.
    pub fn factory(&self) -> &FormulaFactory {
        &self.0.factory
    }

    /// Free-identifier names occurring in the expression, sorted and
    /// deduplicated. Cached at construction.
    pub fn free_identifiers(&self) -> &[String] {
        &self.0.free_idents
    }

    /// De Bruijn indices occurring in the expression that are not bound
    /// within it, sorted ascending. Cached at construction.
    pub fn dangling_bound_indices(&self) -> &[u32] {
        &self.0.dangling
    }
}

impl PartialEq for Expression {
    fn eq(&self, other: &Self) -> bool {
        if Arc::ptr_eq(&self.0, &other.0) {
            return true;
        }
        if self.0.hash != other.0.hash {
            return false;
        }
        if self.0.ty != other.0.ty {
            return false;
        }
        kind_eq(&self.0.kind, &other.0.kind)
    }
}

impl Eq for Expression {}

impl std::hash::Hash for Expression {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_u64(self.0.hash);
    }
}

/// The numeric tag of a kind.
pub(super) fn kind_tag(kind: &ExpressionKind) -> Tag {
    match kind {
        ExpressionKind::FreeIdentifier(_) => tag::FREE_IDENT,
        ExpressionKind::BoundIdentifier(_) => tag::BOUND_IDENT,
        ExpressionKind::IntegerLiteral(_) => tag::INTLIT,
        ExpressionKind::SetExtension(_) => tag::SETEXT,
        ExpressionKind::Atomic(op) => op.tag(),
        ExpressionKind::Bool(_) => tag::KBOOL,
        ExpressionKind::Binary { op, .. } => op.tag(),
        ExpressionKind::Associative { op, .. } => op.tag(),
        ExpressionKind::Unary { op, .. } => op.tag(),
        ExpressionKind::Quantified { op, .. } => op.tag(),
        ExpressionKind::Ascription { .. } => tag::OFTYPE,
        ExpressionKind::Extended { tag, .. } => *tag,
    }
}

/// The cached structural hash of a kind: children first, then the tag.
///
/// Quantified expressions hash their declaration *count*, not the
/// declarations, so alpha-equivalent formulas hash alike.
pub(super) fn kind_hash(kind: &ExpressionKind) -> u64 {
    let children = match kind {
        ExpressionKind::FreeIdentifier(name) => hash_one(name),
        ExpressionKind::BoundIdentifier(index) => u64::from(*index),
        ExpressionKind::IntegerLiteral(value) => hash_one(value),
        ExpressionKind::SetExtension(members) => fold(members.iter().map(|m| m.0.hash)),
        ExpressionKind::Atomic(_) => 0,
        ExpressionKind::Bool(pred) => pred.0.hash,
        ExpressionKind::Binary { left, right, .. } => combine(left.0.hash, right.0.hash),
        ExpressionKind::Associative { children, .. } => fold(children.iter().map(|c| c.0.hash)),
        ExpressionKind::Unary { child, .. } => child.0.hash,
        ExpressionKind::Quantified {
            decls, pred, expr, ..
        } => combine(combine(decls.len() as u64, pred.0.hash), expr.0.hash),
        ExpressionKind::Ascription { expr, type_expr } => combine(expr.0.hash, type_expr.0.hash),
        ExpressionKind::Extended { exprs, preds, .. } => combine(
            fold(exprs.iter().map(|e| e.0.hash)),
            fold(preds.iter().map(|p| p.0.hash)),
        ),
    };
    combine(children, u64::from(kind_tag(kind)))
}

/// Structural kind equality. Assumes tags/hashes were already compared
/// by the caller; still matches on the operator for correctness.
pub(super) fn kind_eq(a: &ExpressionKind, b: &ExpressionKind) -> bool {
    use ExpressionKind as K;
    match (a, b) {
        (K::FreeIdentifier(x), K::FreeIdentifier(y)) => x == y,
        (K::BoundIdentifier(x), K::BoundIdentifier(y)) => x == y,
        (K::IntegerLiteral(x), K::IntegerLiteral(y)) => x == y,
        (K::SetExtension(x), K::SetExtension(y)) => x == y,
        (K::Atomic(x), K::Atomic(y)) => x == y,
        (K::Bool(x), K::Bool(y)) => x == y,
        (
            K::Binary { op, left, right },
            K::Binary {
                op: op2,
                left: left2,
                right: right2,
            },
        ) => op == op2 && left == left2 && right == right2,
        (
            K::Associative { op, children },
            K::Associative {
                op: op2,
                children: children2,
            },
        ) => op == op2 && children == children2,
        (
            K::Unary { op, child },
            K::Unary {
                op: op2,
                child: child2,
            },
        ) => op == op2 && child == child2,
        (
            K::Quantified {
                op,
                decls,
                pred,
                expr,
                ..
            },
            K::Quantified {
                op: op2,
                decls: decls2,
                pred: pred2,
                expr: expr2,
                ..
            },
        ) => {
            // The print form does not participate; declarations compare
            // by solved type only (alpha-equivalence).
            op == op2
                && decls.len() == decls2.len()
                && decls.iter().zip(decls2).all(|(d, d2)| d.alpha_eq(d2))
                && pred == pred2
                && expr == expr2
        }
        (
            K::Ascription { expr, type_expr },
            K::Ascription {
                expr: expr2,
                type_expr: type_expr2,
            },
        ) => expr == expr2 && type_expr == type_expr2,
        (
            K::Extended { tag, exprs, preds },
            K::Extended {
                tag: tag2,
                exprs: exprs2,
                preds: preds2,
            },
        ) => tag == tag2 && exprs == exprs2 && preds == preds2,
        _ => false,
    }
}

/// Validates a requested print form against the actual expression,
/// downgrading as needed. Called once at construction; the stored form
/// is therefore always printable.
pub(super) fn filter_form(form: Form, op: QuantExprOp, n_decls: usize, expr: &Expression) -> Form {
    match form {
        Form::Lambda if op == QuantExprOp::CSet && verify_lambda(n_decls, expr) => Form::Lambda,
        Form::IdentList if op == QuantExprOp::CSet && verify_ident_list(n_decls, expr) => {
            Form::IdentList
        }
        Form::Explicit => Form::Explicit,
        _ if verify_implicit(n_decls, expr) => Form::Implicit,
        _ => Form::Explicit,
    }
}

/// A lambda's expression is `pattern ↦ body` where the pattern is a
/// maplet tree over bound identifiers whose indices strictly decrease
/// left-to-right, ending at 0 — i.e. pairwise-distinct identifiers as
/// the user sees them.
fn verify_lambda(n_decls: usize, expr: &Expression) -> bool {
    let ExpressionKind::Binary {
        op: BinaryExprOp::Mapsto,
        left: pattern,
        ..
    } = &expr.0.kind
    else {
        return false;
    };
    let mut expected = n_decls as i64 - 1;
    fn traverse(pattern: &Expression, expected: &mut i64) -> bool {
        match &pattern.0.kind {
            ExpressionKind::Binary {
                op: BinaryExprOp::Mapsto,
                left,
                right,
            } => traverse(left, expected) && traverse(right, expected),
            ExpressionKind::BoundIdentifier(index) => {
                let matches = i64::from(*index) == *expected;
                *expected -= 1;
                matches
            }
            _ => false,
        }
    }
    traverse(pattern, &mut expected) && expected == -1
}

/// An ident-list comprehension's expression is exactly the left-nested
/// maplet chain of the declarations: `bₙ₋₁ ↦ … ↦ b₁ ↦ b₀`. Anything
/// else would print as an ident list but re-parse differently.
fn verify_ident_list(n_decls: usize, expr: &Expression) -> bool {
    let mut node = expr;
    let mut expected_right: u32 = 0;
    loop {
        match &node.0.kind {
            ExpressionKind::Binary {
                op: BinaryExprOp::Mapsto,
                left,
                right,
            } => {
                if !matches!(
                    right.0.kind,
                    ExpressionKind::BoundIdentifier(i) if i == expected_right
                ) {
                    return false;
                }
                expected_right += 1;
                node = left;
            }
            ExpressionKind::BoundIdentifier(index) => {
                return n_decls == expected_right as usize + 1
                    && u64::from(*index) == expected_right as u64;
            }
            _ => return false,
        }
    }
}

/// An implicit-form expression references exactly the locally bound
/// identifiers: no free identifiers, no identifiers bound outside, and
/// all of the local ones.
fn verify_implicit(n_decls: usize, expr: &Expression) -> bool {
    if !expr.0.free_idents.is_empty() {
        return false;
    }
    let dangling = &expr.0.dangling;
    dangling.len() == n_decls
        && dangling
            .last()
            .is_some_and(|last| *last as usize == n_decls - 1)
}

/// The fixed type of a nullary operator, if it has one. The generic
/// operators (`∅`, `id`, `prj1`, `prj2`) have no fixed type: they are
/// typed by ascription or by the type-checker.
pub(super) fn atomic_fixed_type(op: AtomicOp) -> Option<Type> {
    match op {
        AtomicOp::Integer | AtomicOp::Natural | AtomicOp::Natural1 => Some(Type::pow(Type::Int)),
        AtomicOp::Bool => Some(Type::pow(Type::Bool)),
        AtomicOp::True | AtomicOp::False => Some(Type::Bool),
        AtomicOp::KPred | AtomicOp::KSucc => Some(Type::relation(Type::Int, Type::Int)),
        AtomicOp::EmptySet | AtomicOp::KPrj1Gen | AtomicOp::KPrj2Gen | AtomicOp::KIdGen => None,
    }
}

/// Whether `ty` is a legal type for the nullary operator `op`: the
/// fixed type for closed operators, or the operator's shape for the
/// generic ones.
pub(super) fn verify_atomic_type(op: AtomicOp, ty: &Type) -> bool {
    if let Some(fixed) = atomic_fixed_type(op) {
        return *ty == fixed;
    }
    match op {
        // ∅ ⦂ ℙ(α)
        AtomicOp::EmptySet => matches!(ty, Type::Pow(_)),
        // id ⦂ ℙ(α × α)
        AtomicOp::KIdGen => match (ty.source(), ty.target()) {
            (Some(source), Some(target)) => source == target,
            _ => false,
        },
        // prj1 ⦂ ℙ((α × β) × α),  prj2 ⦂ ℙ((α × β) × β)
        AtomicOp::KPrj1Gen | AtomicOp::KPrj2Gen => {
            let (Some(Type::Prod(alpha, beta)), Some(target)) = (ty.source(), ty.target()) else {
                return false;
            };
            if op == AtomicOp::KPrj1Gen {
                target == alpha.as_ref()
            } else {
                target == beta.as_ref()
            }
        }
        _ => unreachable!("closed operators are handled by their fixed type"),
    }
}

/// Bottom-up type synthesis: the type of a node whose children are all
/// type-checked and shape-compatible, `None` otherwise. Never fails
/// loudly — an untypeable construction is simply left unchecked, and
/// the type-checker reports the problem with a location.
pub(super) fn synthesize_type(kind: &ExpressionKind) -> Option<Type> {
    match kind {
        // Leaves are typed by the caller (or have a fixed type).
        ExpressionKind::FreeIdentifier(_) | ExpressionKind::BoundIdentifier(_) => None,
        ExpressionKind::IntegerLiteral(_) => Some(Type::Int),
        ExpressionKind::Atomic(op) => atomic_fixed_type(*op),
        ExpressionKind::SetExtension(members) => {
            let first = members.first()?.ty()?;
            members
                .iter()
                .skip(1)
                .all(|m| m.ty() == Some(first))
                .then(|| Type::pow(first.clone()))
        }
        ExpressionKind::Bool(pred) => pred.is_type_checked().then_some(Type::Bool),
        ExpressionKind::Binary { op, left, right } => {
            synthesize_binary(*op, left.ty()?, right.ty()?)
        }
        ExpressionKind::Associative { op, children } => synthesize_associative(*op, children),
        ExpressionKind::Unary { op, child } => synthesize_unary(*op, child.ty()?),
        ExpressionKind::Quantified { op, expr, .. } => {
            let expr_ty = expr.ty()?;
            match op {
                QuantExprOp::QUnion | QuantExprOp::QInter => {
                    expr_ty.base_type().is_some().then(|| expr_ty.clone())
                }
                QuantExprOp::CSet => Some(Type::pow(expr_ty.clone())),
            }
        }
        // The ascription is a constraint on its expression; once the
        // expression is typed, the node shares its type. Whether the
        // spelled type agrees is the type-checker's question.
        ExpressionKind::Ascription { expr, .. } => expr.ty().cloned(),
        // Extended expressions are typed by their extension's rules,
        // once the extension mechanism lands.
        ExpressionKind::Extended { .. } => None,
    }
}

fn synthesize_binary(op: BinaryExprOp, left: &Type, right: &Type) -> Option<Type> {
    use BinaryExprOp as Op;
    match op {
        Op::Mapsto => Some(Type::prod(left.clone(), right.clone())),
        // A ↔ B and the function/relation arrows: ℙ(α), ℙ(β) → ℙ(ℙ(α×β))
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
            let alpha = left.base_type()?;
            let beta = right.base_type()?;
            Some(Type::pow(Type::relation(alpha.clone(), beta.clone())))
        }
        Op::SetMinus => (left.base_type().is_some() && left == right).then(|| left.clone()),
        Op::CProd => {
            let alpha = left.base_type()?;
            let beta = right.base_type()?;
            Some(Type::relation(alpha.clone(), beta.clone()))
        }
        // ℙ(α×β) ⊗ ℙ(α×γ) → ℙ(α×(β×γ))
        Op::DProd => {
            let (a1, beta) = (left.source()?, left.target()?);
            let (a2, gamma) = (right.source()?, right.target()?);
            (a1 == a2).then(|| Type::relation(a1.clone(), Type::prod(beta.clone(), gamma.clone())))
        }
        // ℙ(α×β) ∥ ℙ(γ×δ) → ℙ((α×γ)×(β×δ))
        Op::PProd => {
            let (alpha, beta) = (left.source()?, left.target()?);
            let (gamma, delta) = (right.source()?, right.target()?);
            Some(Type::relation(
                Type::prod(alpha.clone(), gamma.clone()),
                Type::prod(beta.clone(), delta.clone()),
            ))
        }
        Op::DomRes | Op::DomSub => {
            let alpha = left.base_type()?;
            (right.source()? == alpha).then(|| right.clone())
        }
        Op::RanRes | Op::RanSub => {
            let beta = right.base_type()?;
            (left.target()? == beta).then(|| left.clone())
        }
        Op::UpTo => (*left == Type::Int && *right == Type::Int).then(|| Type::pow(Type::Int)),
        Op::Minus | Op::Div | Op::Mod | Op::Expn => {
            (*left == Type::Int && *right == Type::Int).then_some(Type::Int)
        }
        Op::FunImage => {
            let beta = left.target()?;
            (left.source()? == right).then(|| beta.clone())
        }
        Op::RelImage => {
            let alpha = right.base_type()?;
            let beta = left.target()?;
            (left.source()? == alpha).then(|| Type::pow(beta.clone()))
        }
    }
}

fn synthesize_associative(op: AssocExprOp, children: &[Expression]) -> Option<Type> {
    use AssocExprOp as Op;
    let first = children.first()?.ty()?;
    let all_same = || children.iter().skip(1).all(|c| c.ty() == Some(first));
    match op {
        Op::BUnion | Op::BInter => {
            (first.base_type().is_some() && all_same()).then(|| first.clone())
        }
        Op::Ovr => (first.source().is_some() && all_same()).then(|| first.clone()),
        // p ; q ; r — targets chain into sources left to right.
        Op::FComp => {
            for pair in children.windows(2) {
                if pair[0].ty()?.target()? != pair[1].ty()?.source()? {
                    return None;
                }
            }
            Some(Type::relation(
                first.source()?.clone(),
                children.last()?.ty()?.target()?.clone(),
            ))
        }
        // p ∘ q ∘ r — the rightmost applies first.
        Op::BComp => {
            for pair in children.windows(2) {
                if pair[1].ty()?.target()? != pair[0].ty()?.source()? {
                    return None;
                }
            }
            Some(Type::relation(
                children.last()?.ty()?.source()?.clone(),
                first.target()?.clone(),
            ))
        }
        Op::Plus | Op::Mul => (*first == Type::Int && all_same()).then_some(Type::Int),
    }
}

fn synthesize_unary(op: UnaryExprOp, child: &Type) -> Option<Type> {
    use UnaryExprOp as Op;
    match op {
        Op::KCard => child.base_type().is_some().then_some(Type::Int),
        Op::Pow | Op::Pow1 => child
            .base_type()
            .is_some()
            .then(|| Type::pow(child.clone())),
        Op::KUnion | Op::KInter => {
            let inner = child.base_type()?;
            inner.base_type().is_some().then(|| inner.clone())
        }
        Op::KDom => Some(Type::pow(child.source()?.clone())),
        Op::KRan => Some(Type::pow(child.target()?.clone())),
        Op::KMin | Op::KMax => (*child == Type::pow(Type::Int)).then_some(Type::Int),
        Op::Converse => Some(Type::relation(
            child.target()?.clone(),
            child.source()?.clone(),
        )),
        Op::UnMinus => (*child == Type::Int).then_some(Type::Int),
    }
}
