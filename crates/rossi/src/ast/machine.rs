//! Machine AST nodes
//!
//! Machines define the dynamic properties of Event-B models including
//! variables, invariants, and events.

use super::{
    ClauseRegion, Event, Expression, FileMetadata, InitialisationEvent, LabeledPredicate,
    NamedElement, Span,
};

/// An Event-B Machine component
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Machine {
    /// Name of the machine
    pub name: String,

    /// Machine that this machine refines (a machine can only refine at most one abstract machine)
    pub refines: Option<String>,

    /// Contexts that this machine sees (uses)
    pub sees: Vec<String>,

    /// Variables declared in this machine
    pub variables: Vec<NamedElement>,

    /// Invariants (properties that must be maintained by events).
    /// Theorems are stored here with `is_theorem = true`.
    pub invariants: Vec<LabeledPredicate>,

    /// Variant (expression that must decrease for convergent events)
    pub variant: Option<Expression>,

    /// Initialisation event
    pub initialisation: Option<InitialisationEvent>,

    /// Events that define the behavior of the machine
    pub events: Vec<Event>,

    /// Source location of the entire machine (MACHINE name ... END)
    pub span: Option<Span>,

    /// Source location of the machine name
    pub name_span: Option<Span>,

    /// Source regions of the machine's clause sections (textual parse only),
    /// used by structural LSP features such as folding.
    #[cfg_attr(feature = "serde", serde(default))]
    pub clauses: Vec<ClauseRegion>,

    /// Comment from Rodin XML
    pub comment: Option<String>,

    /// File-level metadata from Rodin XML
    pub metadata: Option<FileMetadata>,
}

impl Machine {
    /// Create a new machine with the given name
    pub fn new(name: String) -> Self {
        Self {
            name,
            refines: None,
            sees: Vec::new(),
            variables: Vec::new(),
            invariants: Vec::new(),
            variant: None,
            initialisation: None,
            events: Vec::new(),
            span: None,
            name_span: None,
            clauses: Vec::new(),
            comment: None,
            metadata: None,
        }
    }
}
