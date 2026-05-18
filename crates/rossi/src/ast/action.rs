//! Action AST nodes
//!
//! Actions represent state changes in Event-B events.

use super::{Expression, Predicate};

/// An Event-B action (state transition)
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Action {
    /// Skip action (no-op): skip
    Skip,

    /// Deterministic (parallel) assignment: x := E  or  x, y := E1, E2
    Assignment {
        variables: Vec<String>,
        expressions: Vec<Expression>,
    },

    /// Non-deterministic assignment (becomes member of): x :∈ S  or  x, y :∈ S
    BecomesIn {
        variables: Vec<String>,
        set: Expression,
    },

    /// Non-deterministic assignment (becomes such that): x :| P  or  x, y :| P
    BecomesSuchThat {
        variables: Vec<String>,
        predicate: Predicate,
    },

    /// Function override assignment: f(x) ≔ E  (equivalent to f ≔ f ◁ {x ↦ E})
    FunctionOverride {
        function: String,
        arguments: Vec<Expression>,
        expression: Expression,
    },
}

impl Action {
    /// Create a deterministic assignment action
    pub fn assignment(variable: impl Into<String>, expression: Expression) -> Self {
        Action::Assignment {
            variables: vec![variable.into()],
            expressions: vec![expression],
        }
    }

    /// Create a becomes-in action
    pub fn becomes_in(variable: impl Into<String>, set: Expression) -> Self {
        Action::BecomesIn {
            variables: vec![variable.into()],
            set,
        }
    }

    /// Create a becomes-such-that action
    pub fn becomes_such_that(variable: impl Into<String>, predicate: Predicate) -> Self {
        Action::BecomesSuchThat {
            variables: vec![variable.into()],
            predicate,
        }
    }
}
