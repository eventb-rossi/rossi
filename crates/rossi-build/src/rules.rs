//! Stable `EBnnn` rule identifiers for validation diagnostics.
//!
//! A diagnostic that carries a [`RuleId`] is one that downstream tools (CI
//! gates, SARIF consumers, IDEs) can reason about by code. Internal
//! catch-all sites (e.g. "failed to check context: {e}") deliberately stay
//! untagged — they expose no stable contract.

use crate::Severity;

/// Validation rule identifiers exposed in `Diagnostic.rule_id`.
///
/// Codes use the stable `EBnnn` scheme (`"EB001"`..`"EB025"`); gaps
/// correspond to rules not yet implemented in rossi (e.g. EB010 well-
/// definedness, EB015–17 proof status, EB020 unknown type). EB023 and EB024
/// are rossi-only extensions; EB025 is a refinement static-check emitted by
/// `crate::build`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuleId {
    /// EB001 — XML parse error (corrupt Rodin archive, malformed `.buc`/`.bum`).
    XmlParseError,
    /// EB002 — XML root element is neither `contextFile` nor `machineFile`.
    XmlRootError,
    /// EB003 — A required XML attribute is missing from a Rodin element.
    XmlAttributeError,
    /// EB004 — Camille parse error (an `.eventb` file rejected as a whole).
    CamilleParseError,
    /// EB005 — Formula parse error (Camille / pest grammar rejected source text).
    FormulaParseError,
    /// EB006 — Type error (ill-typed predicate or expression; element dropped).
    TypeError,
    /// EB007 — Circular EXTENDS chain among contexts.
    CircularExtends,
    /// EB008 — Circular REFINES chain among machines.
    CircularRefines,
    /// EB009 — Cross-reference target not found (unknown SEES / EXTENDS / REFINES name).
    CrossReferenceNotFound,
    /// EB011 — Declared variable never referenced.
    DeadVariable,
    /// EB012 — Variable referenced but never assigned by any event.
    UnmodifiedVariable,
    /// EB013 — Declared constant never referenced in any axiom.
    DeadConstant,
    /// EB014 — INITIALISATION leaves one or more variables unassigned.
    IncompleteInitialisation,
    /// EB018 — Undeclared identifier in a guard, witness, or action.
    UndeclaredIdentifier,
    /// EB019 — Same component name defined in more than one file.
    DuplicateComponent,
    /// EB021 — An identifier (variable, constant, carrier set, or event
    /// parameter) is declared more than once within the same scope.
    DuplicateIdentifier,
    /// EB022 — A label (invariant, event, guard, action, axiom, or witness)
    /// is used more than once within the same scope.
    DuplicateLabel,
    /// EB023 — Declared name collides with rossi's textual operator
    /// vocabulary and can be silently re-lexed as a token. (rossi-only.)
    ShadowedName,
    /// EB024 — A new event (one that does not REFINE an abstract event)
    /// assigns a variable inherited from an abstract machine. (rossi-only.)
    NewEventAssignsInheritedVariable,
    /// EB025 — An event assigns a variable that an abstract machine declares
    /// but this refinement dropped (data-refined away), so it no longer exists
    /// in the concrete state and cannot be assigned.
    DisappearedVariable,
    /// EB026 — A predicate context (invariant, guard, witness, or axiom) uses an
    /// assignment operator (`:=`/`≔`, `:∈`/`::`, `:|`/`:∣`) where a predicate is
    /// required; the intended operator is almost always `=`.
    AssignmentInPredicate,
}

impl RuleId {
    /// Stable string code (`"EB001"`..`"EB023"`).
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            RuleId::XmlParseError => "EB001",
            RuleId::XmlRootError => "EB002",
            RuleId::XmlAttributeError => "EB003",
            RuleId::CamilleParseError => "EB004",
            RuleId::FormulaParseError => "EB005",
            RuleId::TypeError => "EB006",
            RuleId::CircularExtends => "EB007",
            RuleId::CircularRefines => "EB008",
            RuleId::CrossReferenceNotFound => "EB009",
            RuleId::DeadVariable => "EB011",
            RuleId::UnmodifiedVariable => "EB012",
            RuleId::DeadConstant => "EB013",
            RuleId::IncompleteInitialisation => "EB014",
            RuleId::UndeclaredIdentifier => "EB018",
            RuleId::DuplicateComponent => "EB019",
            RuleId::DuplicateIdentifier => "EB021",
            RuleId::DuplicateLabel => "EB022",
            RuleId::ShadowedName => "EB023",
            RuleId::NewEventAssignsInheritedVariable => "EB024",
            RuleId::DisappearedVariable => "EB025",
            RuleId::AssignmentInPredicate => "EB026",
        }
    }

    /// Short human-readable name, used as SARIF `shortDescription`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            RuleId::XmlParseError => "XML parse error",
            RuleId::XmlRootError => "Unexpected XML root",
            RuleId::XmlAttributeError => "Missing XML attribute",
            RuleId::CamilleParseError => "Camille parse error",
            RuleId::FormulaParseError => "Formula parse error",
            RuleId::TypeError => "Type error",
            RuleId::CircularExtends => "Circular EXTENDS",
            RuleId::CircularRefines => "Circular REFINES",
            RuleId::CrossReferenceNotFound => "Cross-reference not found",
            RuleId::DeadVariable => "Dead variable",
            RuleId::UnmodifiedVariable => "Unmodified variable",
            RuleId::DeadConstant => "Dead constant",
            RuleId::IncompleteInitialisation => "Incomplete INITIALISATION",
            RuleId::UndeclaredIdentifier => "Undeclared identifier",
            RuleId::DuplicateComponent => "Duplicate component",
            RuleId::DuplicateIdentifier => "Duplicate identifier",
            RuleId::DuplicateLabel => "Duplicate label",
            RuleId::ShadowedName => "Shadowed identifier",
            RuleId::NewEventAssignsInheritedVariable => "New event assigns inherited variable",
            RuleId::DisappearedVariable => "Disappeared variable assigned",
            RuleId::AssignmentInPredicate => "Assignment operator in predicate",
        }
    }

    /// One-line explanation, used as SARIF `fullDescription`.
    #[must_use]
    pub fn help(self) -> &'static str {
        match self {
            RuleId::XmlParseError => {
                "A Rodin XML file (.buc, .bum, .bcc, .bcm) could not be parsed."
            }
            RuleId::XmlRootError => {
                "A Rodin XML file's root element is not `org.eventb.core.contextFile` or `org.eventb.core.machineFile`."
            }
            RuleId::XmlAttributeError => {
                "A Rodin XML element is missing a required attribute (e.g. the `target` of an extends/refines/sees clause)."
            }
            RuleId::CamilleParseError => {
                "An .eventb file could not be parsed using the Camille textual notation grammar."
            }
            RuleId::FormulaParseError => {
                "A predicate or expression rejected by the Event-B formula grammar."
            }
            RuleId::TypeError => {
                "A predicate or expression failed type checking and was dropped from the output."
            }
            RuleId::CircularExtends => "A cycle was detected among contexts connected by EXTENDS.",
            RuleId::CircularRefines => "A cycle was detected among machines connected by REFINES.",
            RuleId::CrossReferenceNotFound => {
                "A SEES, EXTENDS, or REFINES clause names a component that does not exist."
            }
            RuleId::DeadVariable => {
                "A machine variable is declared but never referenced in any invariant, guard, or action."
            }
            RuleId::UnmodifiedVariable => {
                "A machine variable is referenced but never assigned by any event (not even INITIALISATION)."
            }
            RuleId::DeadConstant => {
                "A constant is declared but never referenced in any axiom of the owning context chain or any machine that SEES it."
            }
            RuleId::IncompleteInitialisation => {
                "INITIALISATION leaves one or more machine variables unassigned."
            }
            RuleId::UndeclaredIdentifier => {
                "A guard, witness, or action references an identifier that is not in scope."
            }
            RuleId::DuplicateComponent => {
                "The same component name is defined in more than one file in the project."
            }
            RuleId::DuplicateIdentifier => {
                "An identifier (variable, constant, carrier set, or event parameter) is declared more than once within the same scope."
            }
            RuleId::DuplicateLabel => {
                "A label (invariant, event, guard, action, axiom, or witness) is used more than once within the same scope."
            }
            RuleId::ShadowedName => {
                "A declared identifier collides with rossi's textual operator vocabulary (an ASCII operator spelling like `POW`/`or`, or a case variant of a literal token like `Nat`); uses of it can silently parse as the built-in token instead of the identifier."
            }
            RuleId::NewEventAssignsInheritedVariable => {
                "A new event (one that does not REFINE an abstract event) assigns a variable inherited from an abstract machine and kept in this refinement. A new event implicitly refines `skip`, so it must not modify inherited state; doing so leaves the event's refinement proof obligation unprovable. Either REFINES the abstract event that changes the variable, or data-refine the variable."
            }
            RuleId::DisappearedVariable => {
                "An event assigns a variable that an abstract machine declares but this refinement does not keep (it was data-refined away). A disappeared variable no longer exists in the concrete state, so it cannot be assigned; either redeclare it in this machine's VARIABLES, or remove the assignment."
            }
            RuleId::AssignmentInPredicate => {
                "An invariant, guard, witness, or axiom uses an assignment operator (`:=`/`≔`, `:∈`/`::`, or `:|`/`:∣`) where a predicate is required. An assignment cannot stand in a predicate position; the intended operator is most likely `=` for equality."
            }
        }
    }

    /// The severity a diagnostic carrying this rule typically reports at.
    /// Used by SARIF as `defaultConfiguration.level`.
    #[must_use]
    pub fn default_severity(self) -> Severity {
        match self {
            RuleId::XmlParseError
            | RuleId::XmlRootError
            | RuleId::XmlAttributeError
            | RuleId::CamilleParseError
            | RuleId::FormulaParseError
            | RuleId::TypeError
            | RuleId::CircularExtends
            | RuleId::CircularRefines
            | RuleId::CrossReferenceNotFound
            | RuleId::UndeclaredIdentifier
            | RuleId::DuplicateIdentifier
            | RuleId::DuplicateLabel
            | RuleId::NewEventAssignsInheritedVariable
            | RuleId::DisappearedVariable
            | RuleId::AssignmentInPredicate => Severity::Error,
            RuleId::DeadVariable
            | RuleId::UnmodifiedVariable
            | RuleId::DeadConstant
            | RuleId::IncompleteInitialisation
            | RuleId::DuplicateComponent
            | RuleId::ShadowedName => Severity::Warning,
        }
    }

    /// Every defined rule, in catalogue order. Used to build the SARIF
    /// `tool.driver.rules[]` descriptor list.
    #[must_use]
    pub fn all() -> &'static [RuleId] {
        &[
            RuleId::XmlParseError,
            RuleId::XmlRootError,
            RuleId::XmlAttributeError,
            RuleId::CamilleParseError,
            RuleId::FormulaParseError,
            RuleId::TypeError,
            RuleId::CircularExtends,
            RuleId::CircularRefines,
            RuleId::CrossReferenceNotFound,
            RuleId::DeadVariable,
            RuleId::UnmodifiedVariable,
            RuleId::DeadConstant,
            RuleId::IncompleteInitialisation,
            RuleId::UndeclaredIdentifier,
            RuleId::DuplicateComponent,
            RuleId::DuplicateIdentifier,
            RuleId::DuplicateLabel,
            RuleId::ShadowedName,
            RuleId::NewEventAssignsInheritedVariable,
            RuleId::DisappearedVariable,
            RuleId::AssignmentInPredicate,
        ]
    }
}

impl std::fmt::Display for RuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_rule_has_a_unique_code() {
        let codes: HashSet<_> = RuleId::all().iter().map(|r| r.code()).collect();
        assert_eq!(codes.len(), RuleId::all().len());
    }

    #[test]
    fn codes_are_stable() {
        assert_eq!(RuleId::XmlParseError.code(), "EB001");
        assert_eq!(RuleId::XmlRootError.code(), "EB002");
        assert_eq!(RuleId::XmlAttributeError.code(), "EB003");
        assert_eq!(RuleId::CamilleParseError.code(), "EB004");
        assert_eq!(RuleId::FormulaParseError.code(), "EB005");
        assert_eq!(RuleId::TypeError.code(), "EB006");
        assert_eq!(RuleId::CircularExtends.code(), "EB007");
        assert_eq!(RuleId::CircularRefines.code(), "EB008");
        assert_eq!(RuleId::CrossReferenceNotFound.code(), "EB009");
        assert_eq!(RuleId::DeadVariable.code(), "EB011");
        assert_eq!(RuleId::UnmodifiedVariable.code(), "EB012");
        assert_eq!(RuleId::DeadConstant.code(), "EB013");
        assert_eq!(RuleId::IncompleteInitialisation.code(), "EB014");
        assert_eq!(RuleId::UndeclaredIdentifier.code(), "EB018");
        assert_eq!(RuleId::DuplicateComponent.code(), "EB019");
        assert_eq!(RuleId::DuplicateIdentifier.code(), "EB021");
        assert_eq!(RuleId::DuplicateLabel.code(), "EB022");
        assert_eq!(RuleId::ShadowedName.code(), "EB023");
        assert_eq!(RuleId::NewEventAssignsInheritedVariable.code(), "EB024");
        assert_eq!(RuleId::DisappearedVariable.code(), "EB025");
        assert_eq!(RuleId::AssignmentInPredicate.code(), "EB026");
    }

    #[test]
    fn display_uses_code() {
        assert_eq!(format!("{}", RuleId::CircularExtends), "EB007");
    }
}
