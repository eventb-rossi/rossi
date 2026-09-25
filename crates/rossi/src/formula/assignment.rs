//! Assignment nodes (event actions).

use std::sync::Arc;

use crate::ast::Span;

use super::decl::BoundIdentDecl;
use super::expression::Expression;
use super::factory::FormulaFactory;
use super::hashing::{combine, fold};
use super::predicate::Predicate;
use super::tag::{self, Tag};

/// An immutable assignment: the formula of one event action.
///
/// Event-B has no `skip` assignment: an event with no effect simply has no
/// actions.
#[derive(Debug, Clone)]
pub struct Assignment(pub(super) Arc<AssignData>);

#[derive(Debug)]
pub(super) struct AssignData {
    pub(super) kind: AssignmentKind,
    pub(super) span: Option<Span>,
    pub(super) hash: u64,
    /// Whether every embedded expression and declaration carries a
    /// solved type.
    pub(super) typed: bool,
    pub(super) free_idents: Box<[String]>,
    pub(super) dangling: Box<[u32]>,
    pub(super) factory: FormulaFactory,
}

/// The structural kind of an [`Assignment`].
///
/// Kinds are public for pattern matching, but assignments can only be
/// constructed through a [`super::FormulaFactory`], which guarantees
/// the assigned identifiers are free-identifier nodes and the parallel
/// lists line up.
#[derive(Debug)]
pub enum AssignmentKind {
    /// `x, y ≔ E, F` — deterministic, possibly multi-target.
    BecomesEqualTo {
        /// The assigned identifiers (free-identifier expressions).
        idents: Vec<Expression>,
        /// The assigned values, parallel to `idents`.
        values: Vec<Expression>,
    },
    /// `x, y :∈ S` — nondeterministic choice from a set. With several
    /// targets, the set ranges over the left-nested product of the
    /// target types.
    BecomesMemberOf {
        /// The assigned identifiers (free-identifier expressions).
        idents: Vec<Expression>,
        /// The set chosen from.
        set: Expression,
    },
    /// `x, y :∣ P` — nondeterministic, constrained by a before-after
    /// predicate over the primed identifiers.
    BecomesSuchThat {
        /// The assigned identifiers (free-identifier expressions).
        idents: Vec<Expression>,
        /// One primed declaration per assigned identifier (`x'`); the
        /// condition is scoped under them, innermost-last.
        primed: Vec<BoundIdentDecl>,
        /// The before-after predicate.
        pred: Predicate,
    },
}

impl Assignment {
    /// The structural kind, for pattern matching.
    pub fn kind(&self) -> &AssignmentKind {
        &self.0.kind
    }

    /// The node's numeric tag.
    pub fn tag(&self) -> Tag {
        kind_tag(&self.0.kind)
    }

    /// The source span, if the assignment came from source text.
    pub fn span(&self) -> Option<Span> {
        self.0.span
    }

    /// Whether every embedded expression and declaration carries a
    /// solved type.
    pub fn is_type_checked(&self) -> bool {
        self.0.typed
    }

    /// The factory this assignment was built with.
    pub fn factory(&self) -> &FormulaFactory {
        &self.0.factory
    }

    /// Free-identifier names occurring in the assignment, sorted and
    /// deduplicated. Cached at construction.
    pub fn free_identifiers(&self) -> &[String] {
        &self.0.free_idents
    }

    /// De Bruijn indices occurring in the assignment that are not bound
    /// within it, sorted ascending. Cached at construction.
    pub fn dangling_bound_indices(&self) -> &[u32] {
        &self.0.dangling
    }

    /// The assigned identifiers, for any assignment kind.
    pub fn assigned_identifiers(&self) -> &[Expression] {
        match &self.0.kind {
            AssignmentKind::BecomesEqualTo { idents, .. }
            | AssignmentKind::BecomesMemberOf { idents, .. }
            | AssignmentKind::BecomesSuchThat { idents, .. } => idents,
        }
    }
}

impl PartialEq for Assignment {
    fn eq(&self, other: &Self) -> bool {
        if Arc::ptr_eq(&self.0, &other.0) {
            return true;
        }
        if self.0.hash != other.0.hash {
            return false;
        }
        kind_eq(&self.0.kind, &other.0.kind)
    }
}

impl Eq for Assignment {}

impl std::hash::Hash for Assignment {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_u64(self.0.hash);
    }
}

/// The numeric tag of a kind.
pub(super) fn kind_tag(kind: &AssignmentKind) -> Tag {
    match kind {
        AssignmentKind::BecomesEqualTo { .. } => tag::BECOMES_EQUAL_TO,
        AssignmentKind::BecomesMemberOf { .. } => tag::BECOMES_MEMBER_OF,
        AssignmentKind::BecomesSuchThat { .. } => tag::BECOMES_SUCH_THAT,
    }
}

/// The cached structural hash of a kind. The primed declarations of a
/// such-that assignment are excluded: their names are not part of
/// equality.
pub(super) fn kind_hash(kind: &AssignmentKind) -> u64 {
    let children = match kind {
        AssignmentKind::BecomesEqualTo { idents, values } => combine(
            fold(idents.iter().map(|i| i.0.hash)),
            fold(values.iter().map(|v| v.0.hash)),
        ),
        AssignmentKind::BecomesMemberOf { idents, set } => {
            combine(fold(idents.iter().map(|i| i.0.hash)), set.0.hash)
        }
        AssignmentKind::BecomesSuchThat { idents, pred, .. } => {
            combine(fold(idents.iter().map(|i| i.0.hash)), pred.0.hash)
        }
    };
    combine(children, u64::from(kind_tag(kind)))
}

/// Structural kind equality. Primed declarations compare by solved type
/// only, like quantifier declarations.
pub(super) fn kind_eq(a: &AssignmentKind, b: &AssignmentKind) -> bool {
    use AssignmentKind as K;
    match (a, b) {
        (
            K::BecomesEqualTo { idents, values },
            K::BecomesEqualTo {
                idents: idents2,
                values: values2,
            },
        ) => idents == idents2 && values == values2,
        (
            K::BecomesMemberOf { idents, set },
            K::BecomesMemberOf {
                idents: idents2,
                set: set2,
            },
        ) => idents == idents2 && set == set2,
        (
            K::BecomesSuchThat {
                idents,
                primed,
                pred,
            },
            K::BecomesSuchThat {
                idents: idents2,
                primed: primed2,
                pred: pred2,
            },
        ) => {
            idents == idents2
                && primed.len() == primed2.len()
                && primed.iter().zip(primed2).all(|(d, d2)| d.alpha_eq(d2))
                && pred == pred2
        }
        _ => false,
    }
}

/// Whether every embedded expression and declaration carries a solved
/// type.
pub(super) fn kind_typed(kind: &AssignmentKind) -> bool {
    match kind {
        AssignmentKind::BecomesEqualTo { idents, values } => {
            idents.iter().all(Expression::is_type_checked)
                && values.iter().all(Expression::is_type_checked)
        }
        AssignmentKind::BecomesMemberOf { idents, set } => {
            idents.iter().all(Expression::is_type_checked) && set.is_type_checked()
        }
        AssignmentKind::BecomesSuchThat {
            idents,
            primed,
            pred,
        } => {
            idents.iter().all(Expression::is_type_checked)
                && primed.iter().all(BoundIdentDecl::is_type_checked)
                && pred.is_type_checked()
        }
    }
}
