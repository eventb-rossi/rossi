//! Action AST nodes
//!
//! Actions represent state changes in Event-B events.

use super::{Expression, Ident, Predicate, Span};

/// An Event-B action (state transition) together with its source location.
///
/// The action variant lives in [`ActionKind`]; `span` records where the action
/// came from, or `None` for synthesized / Rodin-XML nodes. Equality ignores
/// `span` (see [`Expression`] for the rationale).
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Action {
    /// The action variant.
    pub kind: ActionKind,
    /// Source span of this action, if known.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub span: Option<Span>,
}

impl Action {
    /// Wrap a kind with an explicit (optional) span.
    pub fn new(kind: ActionKind, span: Option<Span>) -> Self {
        Self { kind, span }
    }
}

/// Equality compares the kind only; the span is positional metadata.
impl PartialEq for Action {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

impl Eq for Action {}

impl From<ActionKind> for Action {
    /// Build a span-less action from its kind.
    fn from(kind: ActionKind) -> Self {
        Self { kind, span: None }
    }
}

/// The variants of an Event-B [`Action`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ActionKind {
    /// Skip action (no-op): skip
    Skip,

    /// Deterministic (parallel) assignment: x := E  or  x, y := E1, E2.
    /// Each target is stored with its corresponding right-hand-side value.
    Assignment {
        assignments: Vec<(Ident, Expression)>,
    },

    /// Non-deterministic assignment (becomes member of): x :∈ S  or  x, y :∈ S
    BecomesIn {
        variables: Vec<Ident>,
        set: Expression,
    },

    /// Non-deterministic assignment (becomes such that): x :| P  or  x, y :| P
    BecomesSuchThat {
        variables: Vec<Ident>,
        predicate: Predicate,
    },
}

impl Action {
    /// Create a deterministic assignment action
    pub fn assignment(variable: impl Into<Ident>, expression: Expression) -> Self {
        ActionKind::Assignment {
            assignments: vec![(variable.into(), expression)],
        }
        .into()
    }

    /// Create a becomes-in action
    pub fn becomes_in(variable: impl Into<Ident>, set: Expression) -> Self {
        ActionKind::BecomesIn {
            variables: vec![variable.into()],
            set,
        }
        .into()
    }

    /// Create a becomes-such-that action
    pub fn becomes_such_that(variable: impl Into<Ident>, predicate: Predicate) -> Self {
        ActionKind::BecomesSuchThat {
            variables: vec![variable.into()],
            predicate,
        }
        .into()
    }
}
