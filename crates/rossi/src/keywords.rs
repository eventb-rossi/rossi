//! Canonical Event-B structural keywords.
//!
//! This module is the single source of truth for section keywords, event
//! keywords, status values, and the inline `theorem`/`skip` modifiers. It
//! mirrors [`crate::operators`]: a const table ([`KEYWORDS`]) plus lookup
//! helpers. LSP features (completion, hover, semantic tokens, folding) and the
//! parser's error recovery all derive their keyword sets from this table rather
//! than restating them.
//!
//! The vocabulary matches the structural keyword list documented in
//! `docs/EVENTB_LANGUAGE_REFERENCE.md` and is kept in sync with `grammar.pest`
//! by the `keywords_match_grammar` test.

/// Stable identifier for a structural keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeywordId {
    // Component
    Context,
    Machine,
    End,
    // Context clauses
    Extends,
    Sets,
    Constants,
    Axioms,
    // Machine clauses
    Refines,
    Sees,
    Variables,
    Invariants,
    Variant,
    Events,
    // Event declarations
    Event,
    Initialisation,
    // Event clauses
    Status,
    Any,
    Where,
    With,
    Witness,
    Then,
    // Status values
    Ordinary,
    Convergent,
    Anticipated,
    // Inline modifiers (appear inside predicates/actions, not as clause headers)
    Theorem,
    Skip,
    // The THEOREMS section header (context and machine). The parser lowers its
    // members into the axioms/invariants vec with `is_theorem = true`, since Rodin
    // models a theorem as a flagged axiom/invariant, not a separate container.
    Theorems,
}

/// Structural grouping, used to derive context-specific keyword sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeywordGroup {
    Component,
    ContextClause,
    MachineClause,
    EventDecl,
    EventClause,
    Status,
    Inline,
}

/// Completion-context bitflags: the structural scopes where a keyword may be
/// offered as a completion.
pub mod scope {
    pub const CONTEXT: u8 = 1 << 0;
    pub const MACHINE: u8 = 1 << 1;
    /// The `EVENTS` section body (offers `EVENT`, `INITIALISATION`).
    pub const EVENTS: u8 = 1 << 2;
    /// Inside a single event.
    pub const EVENT: u8 = 1 << 3;
}

/// A structural keyword and its associated metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keyword {
    pub id: KeywordId,
    /// Accepted spellings, canonical (display) form first
    /// (e.g. `["WHERE", "WHEN"]`). Section/event keywords are uppercase, status
    /// and inline keywords lowercase; all lookups are case-insensitive.
    pub spellings: &'static [&'static str],
    pub group: KeywordGroup,
    /// Bitmask of [`scope`] flags where this keyword is offered in completion;
    /// `0` means it is never offered as a context keyword.
    pub completion_scopes: u8,
    /// Short description shown in completion items.
    pub summary: &'static str,
}

impl Keyword {
    /// Canonical (display) spelling.
    pub const fn text(&self) -> &'static str {
        self.spellings[0]
    }
}

const fn kw(
    id: KeywordId,
    spellings: &'static [&'static str],
    group: KeywordGroup,
    completion_scopes: u8,
    summary: &'static str,
) -> Keyword {
    Keyword {
        id,
        spellings,
        group,
        completion_scopes,
        summary,
    }
}

use KeywordGroup as Grp;
use KeywordId::*;

pub const KEYWORDS: &[Keyword] = &[
    // Component
    kw(
        Context,
        &["CONTEXT"],
        Grp::Component,
        0,
        "Define a context (static properties)",
    ),
    kw(
        Machine,
        &["MACHINE"],
        Grp::Component,
        0,
        "Define a machine (dynamic behavior)",
    ),
    kw(
        End,
        &["END"],
        Grp::Component,
        scope::CONTEXT | scope::MACHINE | scope::EVENT,
        "End the current block",
    ),
    // Context clauses
    kw(
        Extends,
        &["EXTENDS"],
        Grp::ContextClause,
        scope::CONTEXT | scope::EVENT,
        "Extend another context or abstract event",
    ),
    kw(
        Sets,
        &["SETS"],
        Grp::ContextClause,
        scope::CONTEXT,
        "Define carrier sets",
    ),
    kw(
        Constants,
        &["CONSTANTS"],
        Grp::ContextClause,
        scope::CONTEXT,
        "Define constants",
    ),
    kw(
        Axioms,
        &["AXIOMS"],
        Grp::ContextClause,
        scope::CONTEXT,
        "Define axioms (properties)",
    ),
    // Machine clauses
    kw(
        Refines,
        &["REFINES"],
        Grp::MachineClause,
        scope::MACHINE | scope::EVENT,
        "Refine an abstract machine or event",
    ),
    kw(
        Sees,
        &["SEES"],
        Grp::MachineClause,
        scope::MACHINE,
        "See a context",
    ),
    kw(
        Variables,
        &["VARIABLES"],
        Grp::MachineClause,
        scope::MACHINE,
        "Define state variables",
    ),
    kw(
        Invariants,
        &["INVARIANTS"],
        Grp::MachineClause,
        scope::MACHINE,
        "Define invariants (properties)",
    ),
    kw(
        Variant,
        &["VARIANT"],
        Grp::MachineClause,
        scope::MACHINE,
        "Define variant for termination",
    ),
    kw(
        Events,
        &["EVENTS"],
        Grp::MachineClause,
        scope::MACHINE,
        "Begin events section",
    ),
    // Event declarations
    kw(
        Event,
        &["EVENT"],
        Grp::EventDecl,
        scope::EVENTS,
        "Define a new event",
    ),
    kw(
        Initialisation,
        &["INITIALISATION"],
        Grp::EventDecl,
        scope::EVENTS,
        "Define initialization event",
    ),
    // Event clauses
    kw(
        Status,
        &["STATUS"],
        Grp::EventClause,
        scope::EVENT,
        "Define event status",
    ),
    kw(
        Any,
        &["ANY"],
        Grp::EventClause,
        scope::EVENT,
        "Introduce event parameters",
    ),
    kw(
        Where,
        &["WHERE", "WHEN"],
        Grp::EventClause,
        scope::EVENT,
        "Define event guards",
    ),
    kw(
        With,
        &["WITH"],
        Grp::EventClause,
        scope::EVENT,
        "Specify witnesses",
    ),
    kw(
        Witness,
        &["WITNESS"],
        Grp::EventClause,
        scope::EVENT,
        "Define witness values",
    ),
    kw(
        Then,
        &["THEN", "BEGIN"],
        Grp::EventClause,
        scope::EVENT,
        "Define event actions",
    ),
    // Status values (offered via a `STATUS`-line trigger, not a block scope)
    kw(
        Ordinary,
        &["ordinary"],
        Grp::Status,
        0,
        "Ordinary event (default)",
    ),
    kw(
        Convergent,
        &["convergent"],
        Grp::Status,
        0,
        "Convergent event (decreases variant)",
    ),
    kw(
        Anticipated,
        &["anticipated"],
        Grp::Status,
        0,
        "Anticipated event (may increase variant)",
    ),
    // Inline modifiers
    kw(
        Theorem,
        &["theorem"],
        Grp::Inline,
        0,
        "Mark a labeled predicate as a theorem",
    ),
    kw(
        Skip,
        &["skip"],
        Grp::Inline,
        0,
        "No-op action (does nothing)",
    ),
    // A context AND machine clause; the dual scope is carried by `completion_scopes`
    // (mirroring EXTENDS/REFINES). Members lower into the axioms/invariants vec with
    // `is_theorem = true` — a theorem is a flagged axiom/invariant in Rodin's model.
    kw(
        Theorems,
        &["THEOREMS"],
        Grp::ContextClause,
        scope::CONTEXT | scope::MACHINE,
        "Declares theorems (properties proved once, not preserved by events)",
    ),
];

/// Look up a keyword by any of its spellings (case-insensitive).
pub fn lookup(word: &str) -> Option<&'static Keyword> {
    KEYWORDS
        .iter()
        .find(|k| k.spellings.iter().any(|s| s.eq_ignore_ascii_case(word)))
}

/// The keyword for an id. Panics if the table is missing it (mirrors
/// [`crate::operators::spelling`]).
pub fn keyword(id: KeywordId) -> &'static Keyword {
    KEYWORDS
        .iter()
        .find(|k| k.id == id)
        .expect("keyword is missing from KEYWORDS")
}

/// Canonical spelling for an id.
pub fn spell(id: KeywordId) -> &'static str {
    keyword(id).text()
}

/// Whether `word` is any structural keyword (case-insensitive).
pub fn is_keyword(word: &str) -> bool {
    lookup(word).is_some()
}

/// Keywords offered in the given completion scope (a bitmask of [`scope`] flags).
pub fn iter_completion_scope(scope_mask: u8) -> impl Iterator<Item = &'static Keyword> {
    KEYWORDS
        .iter()
        .filter(move |k| k.completion_scopes & scope_mask != 0)
}

/// Keywords in the given group.
pub fn iter_group(group: KeywordGroup) -> impl Iterator<Item = &'static Keyword> {
    KEYWORDS.iter().filter(move |k| k.group == group)
}

/// A context or machine clause header (the folding/clause boundary group).
fn is_clause_group(k: &Keyword) -> bool {
    matches!(k.group, Grp::ContextClause | Grp::MachineClause)
}

/// A clause header or `END` (the parser's error-recovery boundary set).
fn is_recovery_group(k: &Keyword) -> bool {
    is_clause_group(k) || k.id == End
}

/// Whether `word` begins a context/machine clause (used by folding boundaries).
pub fn is_clause_keyword(word: &str) -> bool {
    lookup(word).is_some_and(is_clause_group)
}

/// Whether `word` begins a clause or ends a component (used by the parser's
/// error-recovery clause splitting). Equivalent to [`is_clause_keyword`] plus `END`.
pub fn is_recovery_boundary(word: &str) -> bool {
    lookup(word).is_some_and(is_recovery_group)
}

/// Whether `word` starts any structural region (used by clause-boundary
/// detection). Every keyword except the status values and inline modifiers.
pub fn is_clause_boundary(word: &str) -> bool {
    lookup(word).is_some_and(|k| !matches!(k.group, Grp::Status | Grp::Inline))
}

/// Uppercase spellings of all recovery-boundary keywords, for the parser's
/// offset scan over an uppercased component body.
pub fn recovery_boundary_spellings() -> impl Iterator<Item = &'static str> {
    KEYWORDS
        .iter()
        .filter(|&k| is_recovery_group(k))
        .flat_map(|k| k.spellings.iter().copied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn all_ids() -> [KeywordId; 27] {
        [
            Context,
            Machine,
            End,
            Extends,
            Sets,
            Constants,
            Axioms,
            Refines,
            Sees,
            Variables,
            Invariants,
            Variant,
            Events,
            Event,
            Initialisation,
            Status,
            Any,
            Where,
            With,
            Witness,
            Then,
            Ordinary,
            Convergent,
            Anticipated,
            Theorem,
            Skip,
            Theorems,
        ]
    }

    #[test]
    fn every_id_has_exactly_one_row() {
        for id in all_ids() {
            let rows = KEYWORDS.iter().filter(|k| k.id == id).count();
            assert_eq!(rows, 1, "{id:?} should have exactly one row, found {rows}");
        }
        assert_eq!(
            KEYWORDS.len(),
            all_ids().len(),
            "KEYWORDS has rows not covered by all_ids()"
        );
    }

    #[test]
    fn lookup_round_trips_every_id() {
        for id in all_ids() {
            assert_eq!(lookup(keyword(id).text()).map(|k| k.id), Some(id));
        }
    }

    #[test]
    fn lookup_is_case_insensitive() {
        let a = lookup("SETS").map(|k| k.id);
        assert_eq!(a, Some(Sets));
        assert_eq!(lookup("sets").map(|k| k.id), a);
        assert_eq!(lookup("Sets").map(|k| k.id), a);
    }

    #[test]
    fn aliases_resolve_to_canonical_id() {
        assert_eq!(lookup("WHEN").map(|k| k.id), Some(Where));
        assert_eq!(lookup("BEGIN").map(|k| k.id), Some(Then));
        assert_eq!(spell(Where), "WHERE");
        assert_eq!(spell(Then), "THEN");
    }

    #[test]
    fn dual_scope_and_status_scopes() {
        assert_eq!(
            keyword(Refines).completion_scopes,
            scope::MACHINE | scope::EVENT
        );
        assert_eq!(
            keyword(Extends).completion_scopes,
            scope::CONTEXT | scope::EVENT
        );
        for k in iter_group(KeywordGroup::Status) {
            assert_eq!(
                k.completion_scopes, 0,
                "{:?} should not be a context keyword",
                k.id
            );
        }
    }

    #[test]
    fn boundary_predicates_are_nested_subsets() {
        let clause: HashSet<&str> = KEYWORDS
            .iter()
            .flat_map(|k| k.spellings.iter().copied())
            .filter(|w| is_clause_keyword(w))
            .collect();
        let recovery: HashSet<&str> = KEYWORDS
            .iter()
            .flat_map(|k| k.spellings.iter().copied())
            .filter(|w| is_recovery_boundary(w))
            .collect();
        let boundary: HashSet<&str> = KEYWORDS
            .iter()
            .flat_map(|k| k.spellings.iter().copied())
            .filter(|w| is_clause_boundary(w))
            .collect();

        assert!(clause.is_subset(&recovery));
        assert!(recovery.is_subset(&boundary));
        let extra: HashSet<&str> = recovery.difference(&clause).copied().collect();
        assert_eq!(extra, HashSet::from(["END"]));
    }

    #[test]
    fn theorems_is_a_clause_keyword() {
        // THEOREMS is a real context+machine clause: it folds, bounds recovery, and
        // is offered for completion in both component scopes.
        assert!(is_clause_keyword("THEOREMS"));
        assert!(is_recovery_boundary("THEOREMS"));
        assert!(is_clause_boundary("THEOREMS"));
        assert_eq!(
            keyword(Theorems).completion_scopes,
            scope::CONTEXT | scope::MACHINE
        );
        assert!(iter_completion_scope(scope::CONTEXT).any(|k| k.id == Theorems));
        assert!(iter_completion_scope(scope::MACHINE).any(|k| k.id == Theorems));
    }

    #[test]
    fn keywords_match_grammar() {
        let grammar = include_str!("grammar.pest");
        // Collect every `kw_xxx = @{ ^"xxx" ... }` literal, lowercased.
        let mut grammar_kw: HashSet<String> = HashSet::new();
        for line in grammar.lines() {
            let line = line.trim_start();
            if !line.starts_with("kw_") {
                continue;
            }
            if let Some(start) = line.find("^\"") {
                let rest = &line[start + 2..];
                if let Some(end) = rest.find('"') {
                    grammar_kw.insert(rest[..end].to_ascii_lowercase());
                }
            }
        }

        // Forward: every table spelling has a `kw_` rule in the grammar.
        for k in KEYWORDS {
            for s in k.spellings {
                assert!(
                    grammar_kw.contains(&s.to_ascii_lowercase()),
                    "table spelling {s:?} has no kw_ rule in grammar.pest"
                );
            }
        }

        // Reverse: every grammar keyword is in the table, except the
        // math-language keywords handled by `builtins`/`operators`.
        let allow = [
            "true", "false", "nat", "nat1", "int", "bool", "if", "else", "union", "inter",
        ];
        let table: HashSet<String> = KEYWORDS
            .iter()
            .flat_map(|k| k.spellings.iter().map(|s| s.to_ascii_lowercase()))
            .collect();
        for g in &grammar_kw {
            if allow.contains(&g.as_str()) {
                continue;
            }
            assert!(
                table.contains(g),
                "grammar keyword {g:?} is missing from KEYWORDS"
            );
        }
    }

    #[test]
    fn keywords_match_language_reference() {
        // docs/EVENTB_LANGUAGE_REFERENCE.md:352 — documented structural keyword list.
        // Two keywords are added here as known doc-omissions that the grammar and
        // EBNF nonetheless define: `STATUS` (event EBNF at :413) and `THEOREMS`
        // (context/machine EBNF at :391/:403 — the :352 list mentions only the
        // inline `theorem` flag).
        let expected = HashSet::from([
            "CONTEXT",
            "MACHINE",
            "EXTENDS",
            "REFINES",
            "SEES",
            "SETS",
            "CONSTANTS",
            "AXIOMS",
            "VARIABLES",
            "INVARIANTS",
            "VARIANT",
            "EVENTS",
            "EVENT",
            "ANY",
            "WHERE",
            "WHEN",
            "WITH",
            "WITNESS",
            "THEN",
            "BEGIN",
            "END",
            "INITIALISATION",
            "theorem",
            "ordinary",
            "convergent",
            "anticipated",
            "skip",
            "STATUS",
            "THEOREMS",
        ]);
        let table: HashSet<&str> = KEYWORDS
            .iter()
            .flat_map(|k| k.spellings.iter().copied())
            .collect();
        assert_eq!(table, expected);
    }
}
