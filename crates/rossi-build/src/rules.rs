//! Stable rule identifiers, mirroring the eventb-checker EB001–EB019 catalogue.
//!
//! A diagnostic that carries a [`RuleId`] is one that downstream tools (CI
//! gates, SARIF consumers, IDEs) can reason about by code. Internal
//! catch-all sites (e.g. "failed to check context: {e}") deliberately stay
//! untagged — they expose no stable contract.

use crate::Severity;

/// Validation rule identifiers exposed in `Diagnostic.rule_id`.
///
/// Codes follow the eventb-checker scheme (`"EB001"`..`"EB019"`), with the
/// full catalogue implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuleId {
    /// EB001 — XML parse error (corrupt Rodin archive, malformed `.buc`/`.bum`).
    XmlParseError,
    /// EB002 — XML root element is neither `contextFile` nor `machineFile`.
    XmlRootError,
    /// EB003 — A required XML attribute is missing from a Rodin element.
    XmlAttributeError,
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
    /// EB010 — Non-trivial well-definedness condition (informational).
    WellDefinedness,
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
    /// EB021 — Declared name collides with rossi's textual operator
    /// vocabulary and can be silently re-lexed as a token.
    ShadowedName,
}

impl RuleId {
    /// Stable string code (`"EB001"`..`"EB019"`).
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            RuleId::XmlParseError => "EB001",
            RuleId::XmlRootError => "EB002",
            RuleId::XmlAttributeError => "EB003",
            RuleId::FormulaParseError => "EB005",
            RuleId::TypeError => "EB006",
            RuleId::CircularExtends => "EB007",
            RuleId::CircularRefines => "EB008",
            RuleId::CrossReferenceNotFound => "EB009",
            RuleId::WellDefinedness => "EB010",
            RuleId::DeadVariable => "EB011",
            RuleId::UnmodifiedVariable => "EB012",
            RuleId::DeadConstant => "EB013",
            RuleId::IncompleteInitialisation => "EB014",
            RuleId::UndeclaredIdentifier => "EB018",
            RuleId::DuplicateComponent => "EB019",
            RuleId::ShadowedName => "EB021",
        }
    }

    /// Short human-readable name, used as SARIF `shortDescription`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            RuleId::XmlParseError => "XML parse error",
            RuleId::XmlRootError => "Unexpected XML root",
            RuleId::XmlAttributeError => "Missing XML attribute",
            RuleId::FormulaParseError => "Formula parse error",
            RuleId::TypeError => "Type error",
            RuleId::CircularExtends => "Circular EXTENDS",
            RuleId::CircularRefines => "Circular REFINES",
            RuleId::CrossReferenceNotFound => "Cross-reference not found",
            RuleId::WellDefinedness => "Well-definedness condition",
            RuleId::DeadVariable => "Dead variable",
            RuleId::UnmodifiedVariable => "Unmodified variable",
            RuleId::DeadConstant => "Dead constant",
            RuleId::IncompleteInitialisation => "Incomplete INITIALISATION",
            RuleId::UndeclaredIdentifier => "Undeclared identifier",
            RuleId::DuplicateComponent => "Duplicate component",
            RuleId::ShadowedName => "Shadowed identifier",
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
            RuleId::WellDefinedness => {
                "A formula carries a non-trivial well-definedness condition that must be proven."
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
            RuleId::ShadowedName => {
                "A declared identifier collides with rossi's textual operator vocabulary (an ASCII operator spelling like `POW`/`or`, or a case variant of a literal token like `Nat`); uses of it can silently parse as the built-in token instead of the identifier."
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
            | RuleId::FormulaParseError
            | RuleId::TypeError
            | RuleId::CircularExtends
            | RuleId::CircularRefines
            | RuleId::CrossReferenceNotFound
            | RuleId::UndeclaredIdentifier => Severity::Error,
            RuleId::DeadVariable
            | RuleId::UnmodifiedVariable
            | RuleId::DeadConstant
            | RuleId::IncompleteInitialisation
            | RuleId::DuplicateComponent
            | RuleId::ShadowedName => Severity::Warning,
            RuleId::WellDefinedness => Severity::Info,
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
            RuleId::FormulaParseError,
            RuleId::TypeError,
            RuleId::CircularExtends,
            RuleId::CircularRefines,
            RuleId::CrossReferenceNotFound,
            RuleId::WellDefinedness,
            RuleId::DeadVariable,
            RuleId::UnmodifiedVariable,
            RuleId::DeadConstant,
            RuleId::IncompleteInitialisation,
            RuleId::UndeclaredIdentifier,
            RuleId::DuplicateComponent,
            RuleId::ShadowedName,
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
    fn codes_match_eventb_checker_catalogue() {
        assert_eq!(RuleId::XmlParseError.code(), "EB001");
        assert_eq!(RuleId::XmlRootError.code(), "EB002");
        assert_eq!(RuleId::XmlAttributeError.code(), "EB003");
        assert_eq!(RuleId::FormulaParseError.code(), "EB005");
        assert_eq!(RuleId::TypeError.code(), "EB006");
        assert_eq!(RuleId::CircularExtends.code(), "EB007");
        assert_eq!(RuleId::CircularRefines.code(), "EB008");
        assert_eq!(RuleId::CrossReferenceNotFound.code(), "EB009");
        assert_eq!(RuleId::WellDefinedness.code(), "EB010");
        assert_eq!(RuleId::DeadVariable.code(), "EB011");
        assert_eq!(RuleId::UnmodifiedVariable.code(), "EB012");
        assert_eq!(RuleId::DeadConstant.code(), "EB013");
        assert_eq!(RuleId::IncompleteInitialisation.code(), "EB014");
        assert_eq!(RuleId::UndeclaredIdentifier.code(), "EB018");
        assert_eq!(RuleId::DuplicateComponent.code(), "EB019");
    }

    #[test]
    fn display_uses_code() {
        assert_eq!(format!("{}", RuleId::CircularExtends), "EB007");
    }
}
