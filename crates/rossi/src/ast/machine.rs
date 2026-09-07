//! Machine AST nodes
//!
//! Machines define the dynamic properties of Event-B models including
//! variables, invariants, and events.

use super::{
    ClauseRegion, Event, FileMetadata, InitialisationEvent, LabeledPredicate, NamedElement, Span,
};
use crate::formula::Expression;

/// An Event-B Machine component
#[derive(Debug, Clone, PartialEq, Eq)]
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

    /// Variants (expressions that must decrease for convergent events),
    /// in declaration order. Several labeled variants form a
    /// lexicographic order.
    pub variants: Vec<Variant>,

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
    pub clauses: Vec<ClauseRegion>,

    /// Comment from Rodin XML
    pub comment: Option<String>,

    /// File-level metadata from Rodin XML
    pub metadata: Option<FileMetadata>,
}

/// The label Rodin gives a variant when none is written.
pub const DEFAULT_VARIANT_LABEL: &str = "vrn";

/// A machine variant: an expression bounding convergent events,
/// optionally labeled (`None` stands for [`DEFAULT_VARIANT_LABEL`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    /// Optional variant label
    pub label: Option<String>,

    /// The variant expression
    pub expression: Expression,

    /// Source location of the whole item, label included
    pub span: Option<Span>,

    /// Comment from Rodin XML
    pub comment: Option<String>,
}

impl Variant {
    /// The variant's label, defaulting to [`DEFAULT_VARIANT_LABEL`].
    #[must_use]
    pub fn effective_label(&self) -> &str {
        self.label.as_deref().unwrap_or(DEFAULT_VARIANT_LABEL)
    }
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
            variants: Vec::new(),
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
