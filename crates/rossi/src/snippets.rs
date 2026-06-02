//! Canonical Event-B code snippets.
//!
//! This module is the shared reference for the editor snippet libraries
//! (VS Code, LuaSnip, yasnippet) the way [`crate::operators`] is the shared
//! reference for operator spellings. The `rossi gen-grammars` command renders
//! this table into each editor's native snippet format, so every editor offers
//! the same prefixes, descriptions and bodies and they can never drift.
//!
//! Bodies are kept verbatim, including VS Code tabstop placeholders such as
//! `${1:label}`, `$1` and the final-cursor `$0`; the per-editor emitters either
//! pass them through (VS Code, LuaSnip) or translate them to the target format.

/// One snippet: a name, the trigger prefix, a human-readable description, and
/// the body lines (verbatim, with VS Code tabstop placeholders).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snippet {
    /// Display name (the key in the VS Code snippet object).
    pub name: &'static str,
    /// Trigger prefix typed by the user.
    pub prefix: &'static str,
    /// Human-readable description shown in completion UI.
    pub description: &'static str,
    /// Body lines, with VS Code tabstop placeholders (`${1:x}`, `$0`).
    pub body: &'static [&'static str],
}

/// Every Event-B snippet, transcribed verbatim from
/// `editors/vscode/snippets/eventb.json`. This is the single source of truth;
/// the generated `eventb.json` files are produced from it by `gen-grammars`.
pub const SNIPPETS: &[Snippet] = &[
    Snippet {
        name: "Event-B Context",
        prefix: "ctx",
        description: "Create an Event-B context",
        body: &[
            "CONTEXT ${1:context_name}",
            "SETS",
            "    ${2:SET_NAME}",
            "CONSTANTS",
            "    ${3:const_name}",
            "AXIOMS",
            "    ${4:axm1}: ${5:predicate}",
            "END",
        ],
    },
    Snippet {
        name: "Event-B Machine",
        prefix: "mch",
        description: "Create an Event-B machine",
        body: &[
            "MACHINE ${1:machine_name}",
            "VARIABLES",
            "    ${2:var_name}",
            "INVARIANTS",
            "    ${3:inv1}: ${4:predicate}",
            "EVENTS",
            "    INITIALISATION",
            "    BEGIN",
            "        ${5:var_name} := ${6:value}",
            "    END",
            "END",
        ],
    },
    Snippet {
        name: "Event-B Event",
        prefix: "evt",
        description: "Create an Event-B event",
        body: &[
            "EVENT ${1:event_name}",
            "WHERE",
            "    ${2:grd1}: ${3:guard}",
            "THEN",
            "    ${4:act1}: ${5:action}",
            "END",
        ],
    },
    Snippet {
        name: "Event-B Initialisation",
        prefix: "init",
        description: "Create an initialisation event",
        body: &[
            "INITIALISATION",
            "BEGIN",
            "    ${1:var} := ${2:value}",
            "END",
        ],
    },
    Snippet {
        name: "Axiom",
        prefix: "axm",
        description: "Create a labeled axiom",
        body: &["${1:axm1}: ${2:predicate}"],
    },
    Snippet {
        name: "Invariant",
        prefix: "inv",
        description: "Create a labeled invariant",
        body: &["${1:inv1}: ${2:predicate}"],
    },
    Snippet {
        name: "Guard",
        prefix: "grd",
        description: "Create a labeled guard",
        body: &["${1:grd1}: ${2:condition}"],
    },
    Snippet {
        name: "Action (Assignment)",
        prefix: "act",
        description: "Create a labeled deterministic assignment",
        body: &["${1:act1}: ${2:var} := ${3:expression}"],
    },
    Snippet {
        name: "Action (Non-deterministic)",
        prefix: "actnd",
        description: "Create a labeled non-deterministic assignment",
        body: &["${1:act1}: ${2:var} :∈ ${3:set}"],
    },
    Snippet {
        name: "Action (Such That)",
        prefix: "actst",
        description: "Create a labeled assignment with predicate",
        body: &["${1:act1}: ${2:var} :| ${3:predicate}"],
    },
    Snippet {
        name: "For All (Universal Quantification)",
        prefix: "forall",
        description: "Universal quantification (forall)",
        body: &["∀${1:x}·(${2:x} ∈ ${3:set} ⇒ ${4:predicate})"],
    },
    Snippet {
        name: "Exists (Existential Quantification)",
        prefix: "exists",
        description: "Existential quantification (exists)",
        body: &["∃${1:x}·(${2:x} ∈ ${3:set} ∧ ${4:predicate})"],
    },
    Snippet {
        name: "Lambda Abstraction",
        prefix: "lambda",
        description: "Lambda abstraction",
        body: &["λ${1:x}·(${2:x} ∈ ${3:domain} | ${4:expression})"],
    },
    Snippet {
        name: "Set Comprehension",
        prefix: "setcomp",
        description: "Set comprehension",
        body: &["{${1:x} · ${2:x} ∈ ${3:set} ∧ ${4:predicate}}"],
    },
    Snippet {
        name: "Refinement Event",
        prefix: "refines",
        description: "Create an event that refines an abstract event",
        body: &[
            "EVENT ${1:concrete_event}",
            "REFINES ${2:abstract_event}",
            "ANY",
            "    ${3:new_param}",
            "WHERE",
            "    ${4:grd1}: ${5:guard}",
            "WITH",
            "    ${6:abs_param}: ${7:witness}",
            "THEN",
            "    ${8:act1}: ${9:action}",
            "END",
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Guards against accidental loss when editing the table; bump this when
    /// intentionally adding or removing a snippet. This table is the source of
    /// truth — `gen-grammars` renders every editor's library (including VS
    /// Code) from it.
    #[test]
    fn snippet_count_is_stable() {
        assert_eq!(SNIPPETS.len(), 15);
    }

    /// Prefixes are the trigger keys; they must be unique so the editor
    /// snippet engines never have an ambiguous expansion.
    #[test]
    fn prefixes_are_unique_and_well_formed() {
        let mut seen: HashSet<&str> = HashSet::new();
        for snippet in SNIPPETS {
            assert!(
                !snippet.prefix.is_empty()
                    && snippet
                        .prefix
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "prefix {:?} must be a non-empty word token",
                snippet.prefix
            );
            assert!(
                seen.insert(snippet.prefix),
                "duplicate snippet prefix {:?}",
                snippet.prefix
            );
        }
    }

    /// Names are the VS Code snippet object keys; they must be unique too.
    #[test]
    fn names_are_unique_and_bodies_non_empty() {
        let mut seen: HashSet<&str> = HashSet::new();
        for snippet in SNIPPETS {
            assert!(!snippet.name.is_empty(), "snippet name must be non-empty");
            assert!(
                !snippet.description.is_empty(),
                "snippet {:?} must have a description",
                snippet.name
            );
            assert!(
                !snippet.body.is_empty(),
                "snippet {:?} must have a body",
                snippet.name
            );
            assert!(
                seen.insert(snippet.name),
                "duplicate snippet name {:?}",
                snippet.name
            );
        }
    }
}
