//! Event AST nodes
//!
//! Events define state transitions in Event-B machines.

use super::{LabeledAction, LabeledPredicate, NamedElement, Span};

/// Event convergence status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum EventStatus {
    /// Regular event (does not affect variant)
    Ordinary,
    /// Event that decreases the variant
    Convergent,
    /// Event anticipated to be convergent in refinement
    Anticipated,
}

/// An Event-B event
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Event {
    /// Name of the event
    pub name: String,

    /// Convergence status
    pub status: Option<EventStatus>,

    /// Event that this event refines (an event can only refine at most one abstract event)
    pub refines: Option<String>,

    /// Parameters (ANY clause)
    pub parameters: Vec<NamedElement>,

    /// Guards (WHERE/WHEN clause)
    pub guards: Vec<LabeledPredicate>,

    /// WITH clause — labeled predicates witnessing abstract variables
    pub with: Vec<LabeledPredicate>,

    /// WITNESS clause — labeled predicates witnessing abstract parameters
    pub witnesses: Vec<LabeledPredicate>,

    /// Actions (THEN/BEGIN clause)
    pub actions: Vec<LabeledAction>,

    /// Source location of the entire event (EVENT name ... END)
    pub span: Option<Span>,

    /// Source location of the event name
    pub name_span: Option<Span>,

    /// Source location of the `refines`/`extends` target name (the abstract
    /// event this one refines or extends), when one is present
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub refines_span: Option<Span>,

    /// Comment from Rodin XML
    pub comment: Option<String>,

    /// Whether this event extends (rather than refines) its parent
    pub extended: bool,
}

impl Event {
    /// Create a new event with the given name
    pub fn new(name: String) -> Self {
        Self {
            name,
            status: None,
            refines: None,
            parameters: Vec::new(),
            guards: Vec::new(),
            with: Vec::new(),
            witnesses: Vec::new(),
            actions: Vec::new(),
            span: None,
            name_span: None,
            refines_span: None,
            comment: None,
            extended: false,
        }
    }
}

/// The INITIALISATION event
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InitialisationEvent {
    /// Actions that initialize the variables
    pub actions: Vec<LabeledAction>,
    /// Comment from Rodin XML
    pub comment: Option<String>,
    /// Whether this initialisation extends (inherits from) the refined machine's initialisation
    pub extended: bool,
    /// WITH clause — labeled predicates witnessing abstract variables
    pub with: Vec<LabeledPredicate>,
    /// WITNESS clause — labeled predicates witnessing abstract parameters
    pub witnesses: Vec<LabeledPredicate>,
    /// Source location of the whole INITIALISATION event (textual parse only)
    pub span: Option<Span>,
    /// Source location of the INITIALISATION name token (textual parse only)
    pub name_span: Option<Span>,
}
