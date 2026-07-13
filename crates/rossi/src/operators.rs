//! Canonical Event-B operator spellings.
//!
//! This module is the shared reference for Unicode and ASCII spellings used by
//! the parser-facing tools, pretty-printer, and LSP features.

use crate::ast::expression::{BinaryOp, UnaryOp};
use crate::ast::predicate::{ComparisonOp, LogicalOp, Quantifier};
use crate::keywords::is_word_char;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperatorCategory {
    PredicateComparison,
    PredicateLogical,
    Quantifier,
    ExpressionBinary,
    ExpressionUnary,
    ExpressionAtom,
    SyntaxToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperatorId {
    Equal,
    NotEqual,
    LessThan,
    LessEqual,
    GreaterThan,
    GreaterEqual,
    In,
    NotIn,
    Subset,
    SubsetStrict,
    NotSubset,
    NotSubsetStrict,
    And,
    Or,
    Not,
    Implies,
    Equivalent,
    ForAll,
    Exists,
    EmptySet,
    Naturals,
    Naturals1,
    Integers,
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Exponent,
    Range,
    Union,
    Intersection,
    Difference,
    CartesianProduct,
    Relation,
    TotalRelation,
    SurjectiveRelation,
    TotalSurjectiveRelation,
    TotalFunction,
    PartialFunction,
    TotalInjection,
    PartialInjection,
    TotalSurjection,
    PartialSurjection,
    Bijection,
    Composition,
    Semicolon,
    DomainRestriction,
    DomainSubtraction,
    RangeRestriction,
    RangeSubtraction,
    Overwrite,
    DirectProduct,
    ParallelProduct,
    OfType,
    Maplet,
    UnaryMinus,
    PowerSet,
    PowerSet1,
    Domain,
    RangeOfRelation,
    Inverse,
    Lambda,
    Dot,
    Bar,
    QuantifiedUnion,
    QuantifiedIntersection,
    Assignment,
    BecomesIn,
    BecomesSuchThat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperatorSpelling {
    pub id: OperatorId,
    pub category: OperatorCategory,
    pub unicode: &'static str,
    pub ascii: &'static str,
    pub description: &'static str,
    pub completion: bool,
}

impl OperatorSpelling {
    pub const fn text(self, use_unicode: bool) -> &'static str {
        if use_unicode {
            self.unicode
        } else {
            self.ascii
        }
    }

    /// The spelling to write into emitted text — completion inserts, document
    /// formatting, hover titles, the input-method table. Identical to
    /// [`Self::text`], except an operator whose only Unicode spelling is a Rodin
    /// private-use-area glyph always yields its ASCII form: that glyph has no
    /// portable rendering and shows as tofu without Rodin's math font, so rossi
    /// never emits it into a buffer. Private-use spellings are still accepted on
    /// input (the grammar lexes them) — they are never produced.
    pub fn emit_text(self, use_unicode: bool) -> &'static str {
        if is_private_use_glyph(self.unicode) {
            self.ascii
        } else {
            self.text(use_unicode)
        }
    }

    /// True when the ASCII spelling contains no word characters, so it is safe
    /// to substitute eagerly while typing (symbolic combos like `=>`, `|->`),
    /// as opposed to alphabetic ops (`NAT`, `or`, `dom`) which would block
    /// typing ordinary words and are only offered through the `\name` leader.
    pub fn is_symbolic(&self) -> bool {
        is_symbolic_spelling(self.ascii)
    }

    /// True when an as-you-type input method should substitute this operator
    /// eagerly (maximal munch) rather than only through the `\name` leader. See
    /// [`is_eager_input_spelling`] for the eligibility rationale.
    pub fn is_eager_input(&self) -> bool {
        is_eager_input_spelling(self.ascii)
    }

    /// Human-friendly leader-key aliases for this operator (e.g. `and`, `to`).
    pub fn aliases(&self) -> &'static [&'static str] {
        aliases_for(self.id)
    }
}

/// True when an ASCII spelling contains no word characters, so it is safe to
/// substitute eagerly while typing (symbolic combos like `=>`, `|->`, `,,`), as
/// opposed to alphabetic ops (`NAT`, `or`, `dom`) which would block typing
/// ordinary words and are only offered through the `\name` leader.
///
/// A free function (rather than only a method) so it applies to the extra input
/// spellings in [`ascii_input_aliases`], which have no `OperatorSpelling` of
/// their own.
pub fn is_symbolic_spelling(ascii: &str) -> bool {
    !is_alphabetic_op(ascii)
}

/// True when an as-you-type input method should substitute the ASCII spelling
/// eagerly (maximal munch) rather than only through the `\name` leader.
///
/// This is the single source of truth for eager-input eligibility, kept here
/// with the operator data so editors never re-encode the policy. A spelling
/// qualifies when it is symbolic, does not contain the `\` leader, and is not a
/// bare `/` or `.` (which collide with `//` comments and decimal or qualifier
/// dots). Multi-character forms like `/=` and `..` still qualify.
pub fn is_eager_input_spelling(ascii: &str) -> bool {
    is_symbolic_spelling(ascii) && !ascii.contains('\\') && ascii != "/" && ascii != "."
}

// Event-B operators that Rodin renders through Unicode **Private-Use Area** code
// points, because they have no glyph in the standard Unicode blocks. These four
// are the *complete* set Rodin uses (`U+E100..=U+E103`); the
// `private_use_glyphs_match_rodin` guard test pins the operator table to them so
// the implementation can't silently drift from Rodin.
//
// Source of truth: Rodin's `BinaryExpression`/`AssociativeExpression` operator
// tables (corroborated by ProB's `UnicodeTranslator`). Each is paired with a
// plain-ASCII spelling that renders everywhere; see [`OPERATOR_SPELLINGS`].
pub const TOTAL_RELATION: &str = "\u{E100}"; // Rodin TREL,  ASCII `<<->`
pub const SURJECTIVE_RELATION: &str = "\u{E101}"; // Rodin SREL,  ASCII `<->>`
pub const TOTAL_SURJECTIVE_RELATION: &str = "\u{E102}"; // Rodin STREL, ASCII `<<->>`
pub const RELATIONAL_OVERRIDE: &str = "\u{E103}"; // Rodin OVR,   ASCII `<+`

/// True when `s` contains a Unicode Private-Use Area code point
/// (`U+E000..=U+F8FF`) — a glyph that has no portable rendering and only shows
/// under Rodin's math font. The four constants above are the only such spellings
/// rossi uses, so callers (e.g. hover) fall back to ASCII when this holds.
pub fn is_private_use_glyph(s: &str) -> bool {
    s.chars().any(|c| ('\u{E000}'..='\u{F8FF}').contains(&c))
}

pub const OPERATOR_SPELLINGS: &[OperatorSpelling] = &[
    // Predicate comparisons
    op(
        OperatorId::Equal,
        OperatorCategory::PredicateComparison,
        "=",
        "=",
        "Equality",
        true,
    ),
    op(
        OperatorId::NotEqual,
        OperatorCategory::PredicateComparison,
        "≠",
        "/=",
        "Not equal",
        true,
    ),
    op(
        OperatorId::LessThan,
        OperatorCategory::PredicateComparison,
        "<",
        "<",
        "Less than",
        true,
    ),
    op(
        OperatorId::LessEqual,
        OperatorCategory::PredicateComparison,
        "≤",
        "<=",
        "Less than or equal",
        true,
    ),
    op(
        OperatorId::GreaterThan,
        OperatorCategory::PredicateComparison,
        ">",
        ">",
        "Greater than",
        true,
    ),
    op(
        OperatorId::GreaterEqual,
        OperatorCategory::PredicateComparison,
        "≥",
        ">=",
        "Greater than or equal",
        true,
    ),
    op(
        OperatorId::In,
        OperatorCategory::PredicateComparison,
        "∈",
        ":",
        "Set membership",
        true,
    ),
    op(
        OperatorId::NotIn,
        OperatorCategory::PredicateComparison,
        "∉",
        "/:",
        "Not in set",
        true,
    ),
    op(
        OperatorId::Subset,
        OperatorCategory::PredicateComparison,
        "⊆",
        "<:",
        "Subset or equal",
        true,
    ),
    op(
        OperatorId::SubsetStrict,
        OperatorCategory::PredicateComparison,
        "⊂",
        "<<:",
        "Strict subset",
        true,
    ),
    op(
        OperatorId::NotSubset,
        OperatorCategory::PredicateComparison,
        "⊈",
        "/<:",
        "Not subset or equal",
        true,
    ),
    op(
        OperatorId::NotSubsetStrict,
        OperatorCategory::PredicateComparison,
        "⊄",
        "/<<:",
        "Not strict subset",
        true,
    ),
    // Predicate logic and quantifiers
    op(
        OperatorId::And,
        OperatorCategory::PredicateLogical,
        "∧",
        "&",
        "Logical and",
        true,
    ),
    op(
        OperatorId::Or,
        OperatorCategory::PredicateLogical,
        "∨",
        "or",
        "Logical or",
        true,
    ),
    op(
        OperatorId::Not,
        OperatorCategory::PredicateLogical,
        "¬",
        "not",
        "Logical negation",
        true,
    ),
    op(
        OperatorId::Implies,
        OperatorCategory::PredicateLogical,
        "⇒",
        "=>",
        "Logical implication",
        true,
    ),
    op(
        OperatorId::Equivalent,
        OperatorCategory::PredicateLogical,
        "⇔",
        "<=>",
        "Logical equivalence",
        true,
    ),
    op(
        OperatorId::ForAll,
        OperatorCategory::Quantifier,
        "∀",
        "!",
        "Universal quantifier",
        true,
    ),
    op(
        OperatorId::Exists,
        OperatorCategory::Quantifier,
        "∃",
        "#",
        "Existential quantifier",
        true,
    ),
    // Expression atoms and binary operators
    op(
        OperatorId::EmptySet,
        OperatorCategory::ExpressionAtom,
        "∅",
        "{}",
        "Empty set",
        true,
    ),
    op(
        OperatorId::Naturals,
        OperatorCategory::ExpressionAtom,
        "ℕ",
        "NAT",
        "Natural numbers",
        false,
    ),
    op(
        OperatorId::Naturals1,
        OperatorCategory::ExpressionAtom,
        "ℕ1",
        "NAT1",
        "Positive natural numbers",
        false,
    ),
    op(
        OperatorId::Integers,
        OperatorCategory::ExpressionAtom,
        "ℤ",
        "INT",
        "Integers",
        false,
    ),
    op(
        OperatorId::Add,
        OperatorCategory::ExpressionBinary,
        "+",
        "+",
        "Addition",
        true,
    ),
    op(
        OperatorId::Subtract,
        OperatorCategory::ExpressionBinary,
        "−",
        "-",
        "Subtraction",
        true,
    ),
    op(
        OperatorId::Multiply,
        OperatorCategory::ExpressionBinary,
        "∗",
        "*",
        "Multiplication",
        true,
    ),
    op(
        OperatorId::Divide,
        OperatorCategory::ExpressionBinary,
        "÷",
        "/",
        "Division",
        true,
    ),
    op(
        OperatorId::Modulo,
        OperatorCategory::ExpressionBinary,
        "mod",
        "mod",
        "Modulo",
        true,
    ),
    op(
        OperatorId::Exponent,
        OperatorCategory::ExpressionBinary,
        "^",
        "^",
        "Exponentiation",
        true,
    ),
    op(
        OperatorId::Range,
        OperatorCategory::ExpressionBinary,
        "‥",
        "..",
        "Integer range",
        true,
    ),
    op(
        OperatorId::Union,
        OperatorCategory::ExpressionBinary,
        "∪",
        "\\/",
        "Set union",
        true,
    ),
    op(
        OperatorId::Intersection,
        OperatorCategory::ExpressionBinary,
        "∩",
        "/\\",
        "Set intersection",
        true,
    ),
    op(
        OperatorId::Difference,
        OperatorCategory::ExpressionBinary,
        "∖",
        "\\",
        "Set difference",
        true,
    ),
    op(
        OperatorId::CartesianProduct,
        OperatorCategory::ExpressionBinary,
        "×",
        "**",
        "Cartesian product",
        true,
    ),
    op(
        OperatorId::Relation,
        OperatorCategory::ExpressionBinary,
        "↔",
        "<->",
        "Relation",
        true,
    ),
    op(
        OperatorId::TotalRelation,
        OperatorCategory::ExpressionBinary,
        TOTAL_RELATION,
        "<<->",
        "Total relation",
        true,
    ),
    op(
        OperatorId::SurjectiveRelation,
        OperatorCategory::ExpressionBinary,
        SURJECTIVE_RELATION,
        "<->>",
        "Surjective relation",
        true,
    ),
    op(
        OperatorId::TotalSurjectiveRelation,
        OperatorCategory::ExpressionBinary,
        TOTAL_SURJECTIVE_RELATION,
        "<<->>",
        "Total surjective relation",
        true,
    ),
    op(
        OperatorId::TotalFunction,
        OperatorCategory::ExpressionBinary,
        "→",
        "-->",
        "Total function",
        true,
    ),
    op(
        OperatorId::PartialFunction,
        OperatorCategory::ExpressionBinary,
        "⇸",
        "+->",
        "Partial function",
        true,
    ),
    op(
        OperatorId::TotalInjection,
        OperatorCategory::ExpressionBinary,
        "↣",
        ">->",
        "Total injection",
        true,
    ),
    op(
        OperatorId::PartialInjection,
        OperatorCategory::ExpressionBinary,
        "⤔",
        ">+>",
        "Partial injection",
        true,
    ),
    op(
        OperatorId::TotalSurjection,
        OperatorCategory::ExpressionBinary,
        "↠",
        "->>",
        "Total surjection",
        true,
    ),
    op(
        OperatorId::PartialSurjection,
        OperatorCategory::ExpressionBinary,
        "⤀",
        "+>>",
        "Partial surjection",
        true,
    ),
    op(
        OperatorId::Bijection,
        OperatorCategory::ExpressionBinary,
        "⤖",
        ">->>",
        "Bijection",
        true,
    ),
    op(
        OperatorId::Composition,
        OperatorCategory::ExpressionBinary,
        "∘",
        "circ",
        "Backward composition",
        true,
    ),
    op(
        OperatorId::Semicolon,
        OperatorCategory::ExpressionBinary,
        ";",
        ";",
        "Forward composition",
        true,
    ),
    op(
        OperatorId::DomainRestriction,
        OperatorCategory::ExpressionBinary,
        "◁",
        "<|",
        "Domain restriction",
        true,
    ),
    op(
        OperatorId::DomainSubtraction,
        OperatorCategory::ExpressionBinary,
        "⩤",
        "<<|",
        "Domain subtraction",
        true,
    ),
    op(
        OperatorId::RangeRestriction,
        OperatorCategory::ExpressionBinary,
        "▷",
        "|>",
        "Range restriction",
        true,
    ),
    op(
        OperatorId::RangeSubtraction,
        OperatorCategory::ExpressionBinary,
        "⩥",
        "|>>",
        "Range subtraction",
        true,
    ),
    op(
        OperatorId::Overwrite,
        OperatorCategory::ExpressionBinary,
        RELATIONAL_OVERRIDE,
        "<+",
        "Relational override",
        true,
    ),
    op(
        OperatorId::DirectProduct,
        OperatorCategory::ExpressionBinary,
        "⊗",
        "><",
        "Direct product",
        true,
    ),
    op(
        OperatorId::ParallelProduct,
        OperatorCategory::ExpressionBinary,
        "∥",
        "||",
        "Parallel product",
        true,
    ),
    op(
        OperatorId::OfType,
        OperatorCategory::ExpressionBinary,
        "⦂",
        "oftype",
        "Type constraint",
        true,
    ),
    op(
        OperatorId::Maplet,
        OperatorCategory::ExpressionBinary,
        "↦",
        "|->",
        "Maplet",
        true,
    ),
    // Unary expression operators
    op(
        OperatorId::UnaryMinus,
        OperatorCategory::ExpressionUnary,
        "−",
        "-",
        "Unary minus",
        false,
    ),
    op(
        OperatorId::PowerSet,
        OperatorCategory::ExpressionUnary,
        "ℙ",
        "POW",
        "Power set",
        true,
    ),
    op(
        OperatorId::PowerSet1,
        OperatorCategory::ExpressionUnary,
        "ℙ1",
        "POW1",
        "Non-empty power set",
        true,
    ),
    op(
        OperatorId::Domain,
        OperatorCategory::ExpressionUnary,
        "dom",
        "dom",
        "Domain",
        false,
    ),
    op(
        OperatorId::RangeOfRelation,
        OperatorCategory::ExpressionUnary,
        "ran",
        "ran",
        "Range",
        false,
    ),
    op(
        OperatorId::Inverse,
        OperatorCategory::ExpressionUnary,
        "∼",
        "~",
        "Relational inverse",
        true,
    ),
    // Syntax tokens with Rodin keyboard equivalents
    op(
        OperatorId::Lambda,
        OperatorCategory::SyntaxToken,
        "λ",
        "%",
        "Lambda abstraction",
        true,
    ),
    op(
        OperatorId::Dot,
        OperatorCategory::SyntaxToken,
        "·",
        ".",
        "Separator dot",
        false,
    ),
    op(
        OperatorId::Bar,
        OperatorCategory::SyntaxToken,
        "∣",
        "|",
        "Such-that bar",
        false,
    ),
    op(
        OperatorId::QuantifiedUnion,
        OperatorCategory::SyntaxToken,
        "⋃",
        "UNION",
        "Generalized union",
        true,
    ),
    op(
        OperatorId::QuantifiedIntersection,
        OperatorCategory::SyntaxToken,
        "⋂",
        "INTER",
        "Generalized intersection",
        true,
    ),
    op(
        OperatorId::Assignment,
        OperatorCategory::SyntaxToken,
        "≔",
        ":=",
        "Deterministic assignment",
        true,
    ),
    op(
        OperatorId::BecomesIn,
        OperatorCategory::SyntaxToken,
        ":∈",
        "::",
        "Non-deterministic member assignment",
        true,
    ),
    op(
        OperatorId::BecomesSuchThat,
        OperatorCategory::SyntaxToken,
        ":∣",
        ":|",
        "Non-deterministic predicate assignment",
        true,
    ),
];

const fn op(
    id: OperatorId,
    category: OperatorCategory,
    unicode: &'static str,
    ascii: &'static str,
    description: &'static str,
    completion: bool,
) -> OperatorSpelling {
    OperatorSpelling {
        id,
        category,
        unicode,
        ascii,
        description,
        completion,
    }
}

pub fn spelling(id: OperatorId) -> &'static OperatorSpelling {
    OPERATOR_SPELLINGS
        .iter()
        .find(|entry| entry.id == id)
        .expect("operator spelling is missing")
}

pub fn spell(id: OperatorId, use_unicode: bool) -> &'static str {
    spelling(id).text(use_unicode)
}

pub fn lookup_token(token: &str) -> Option<&'static OperatorSpelling> {
    OPERATOR_SPELLINGS
        .iter()
        .find(|entry| entry.unicode == token || entry.ascii == token)
        .or_else(|| {
            // An accepted ASCII input alias (e.g. `,,`) resolves to its operator.
            ASCII_INPUT_ALIASES
                .iter()
                .find_map(|&(alias, entry)| (alias == token).then_some(entry))
        })
}

/// Human-friendly leader-key aliases used by editor `\name` input methods,
/// keyed by operator id. Returns `&[]` for operators without curated aliases.
///
/// This is the single source of truth for the leader vocabulary; aliases must
/// be unique across operators. Grow as needed.
pub fn aliases_for(id: OperatorId) -> &'static [&'static str] {
    use OperatorId::*;
    match id {
        And => &["and", "land", "wedge"],
        Or => &["or", "lor", "vee"],
        Not => &["not", "lnot", "neg"],
        Implies => &["implies", "imp"],
        Equivalent => &["iff", "equiv"],
        ForAll => &["forall", "all"],
        Exists => &["exists", "ex"],
        In => &["in", "mem"],
        NotIn => &["notin", "nin"],
        Subset => &["subseteq", "sub"],
        SubsetStrict => &["subset", "subsetneq"],
        NotSubset => &["nsubseteq"],
        NotSubsetStrict => &["nsubset"],
        LessEqual => &["le", "leq"],
        GreaterEqual => &["ge", "geq"],
        NotEqual => &["ne", "neq"],
        EmptySet => &["emptyset", "empty"],
        Naturals => &["nat"],
        Naturals1 => &["nat1"],
        Integers => &["int"],
        PowerSet => &["pow", "powerset"],
        PowerSet1 => &["pow1"],
        Union => &["union", "cup"],
        Intersection => &["inter", "cap"],
        Difference => &["setminus", "diff"],
        CartesianProduct => &["times", "prod"],
        Relation => &["rel"],
        TotalFunction => &["to", "tfun", "fun"],
        PartialFunction => &["pfun"],
        TotalInjection => &["tinj"],
        PartialInjection => &["pinj"],
        TotalSurjection => &["tsur"],
        PartialSurjection => &["psur"],
        Bijection => &["bij"],
        Maplet => &["maplet", "mapsto"],
        Lambda => &["lambda"],
        Composition => &["circ", "comp"],
        Inverse => &["inv", "inverse"],
        Assignment => &["assign", "becomes"],
        DirectProduct => &["dprod"],
        ParallelProduct => &["pprod"],
        OfType => &["oftype"],
        QuantifiedUnion => &["qunion"],
        QuantifiedIntersection => &["qinter"],
        _ => &[],
    }
}

/// Extra *symbolic* ASCII spellings accepted on input for an operator, beyond
/// its canonical `ascii`. Unlike the word aliases in [`aliases_for`], these are
/// not leader names: they are eager-input combos (Rodin's `,,` for the maplet
/// `↦`) that convert as-you-type and during batch normalization but are NEVER
/// produced on output — the canonical `↦`/`|->` is still what round-trips.
///
/// Source of truth for the ASCII → Unicode direction only; the Unicode → ASCII
/// direction stays canonical (`↦` must never become `,,`). Returns `&[]` for
/// operators without input aliases.
pub fn ascii_input_aliases(id: OperatorId) -> &'static [&'static str] {
    match id {
        // Rodin's keyboard maps both `|->` and `,,` to the maplet `↦`.
        OperatorId::Maplet => &[",,"],
        // Rodin's keyboard registers a second ASCII form for each surjection
        // arrow (`+->>` for `⤀`, `-->>` for `↠`) alongside the canonical
        // `+>>`/`->>`; these longer forms are what the Event-B summary documents.
        // Input-only: emission stays `+>>`/`->>`.
        OperatorId::PartialSurjection => &["+->>"],
        OperatorId::TotalSurjection => &["-->>"],
        _ => &[],
    }
}

/// Every ASCII input alias paired with the operator it spells, in declaration
/// order. Feeds the ASCII → Unicode derived tables (`convert_to_unicode`,
/// `has_ascii_operators`, hover) without touching the Unicode → ASCII direction.
static ASCII_INPUT_ALIASES: std::sync::LazyLock<Vec<(&'static str, &'static OperatorSpelling)>> =
    std::sync::LazyLock::new(|| {
        OPERATOR_SPELLINGS
            .iter()
            .flat_map(|entry| {
                ascii_input_aliases(entry.id)
                    .iter()
                    .map(move |&alias| (alias, entry))
            })
            .collect()
    });

/// Every registered ASCII input alias paired with the operator it spells (e.g.
/// `(",,", <maplet>)`), in declaration order. The single accessor for the full
/// alias set, so callers (the editor operator table, conversion tables) iterate
/// one prebuilt list instead of re-deriving it from [`ascii_input_aliases`].
pub fn ascii_input_alias_entries() -> &'static [(&'static str, &'static OperatorSpelling)] {
    ASCII_INPUT_ALIASES.as_slice()
}

/// One operator spelling, projected for editor input methods. Consumed both by
/// the LSP `rossi/operatorTable` request and by the generated editor grammars,
/// so the mapping lives next to its source table and is never re-encoded.
///
/// Only operators whose ASCII and *emitted* spellings differ are included.
/// `symbolic` marks operators with no word characters (alphabetic ops are
/// leader-only); `eager` marks the subset an input method should substitute as
/// you type (see [`OperatorSpelling::is_eager_input`]). The `serde` feature
/// adds `Serialize` so the LSP can hand the rows to editor clients as JSON.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct OperatorRow {
    pub ascii: String,
    pub unicode: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub symbolic: bool,
    pub eager: bool,
}

/// Build the operator input-method rows from the single-source table in this
/// module. Only operators whose ASCII and *emitted* spellings differ are
/// included: identical ones need no conversion, and the private-use operators
/// emit ASCII (`emit_text`) so they collapse to `ascii == unicode` here and
/// drop out.
///
/// Each operator's extra ASCII input aliases (e.g. `,,` for the maplet ↦) are
/// emitted as their own rows so the editor input method converts them like any
/// other spelling. They share the operator's emitted Unicode but carry no
/// leader aliases of their own (those already ride on the canonical row).
pub fn operator_rows() -> Vec<OperatorRow> {
    let mut seen = std::collections::HashSet::new();
    OPERATOR_SPELLINGS
        .iter()
        .filter_map(|entry| {
            // `emit_text` is the spelling editors should substitute to; the
            // private-use operators emit ASCII, so they collapse to `ascii ==
            // unicode` here and drop out (no point converting `<+` to itself,
            // and nothing should ever substitute to a private-use glyph).
            let unicode = entry.emit_text(true);
            (entry.ascii != unicode && seen.insert((entry.ascii, unicode))).then(|| OperatorRow {
                ascii: entry.ascii.to_string(),
                unicode: unicode.to_string(),
                description: entry.description.to_string(),
                aliases: entry.aliases().iter().map(|a| a.to_string()).collect(),
                symbolic: entry.is_symbolic(),
                eager: entry.is_eager_input(),
            })
        })
        .chain(
            ascii_input_alias_entries()
                .iter()
                .map(|&(alias, entry)| OperatorRow {
                    ascii: alias.to_string(),
                    unicode: entry.emit_text(true).to_string(),
                    description: entry.description.to_string(),
                    aliases: Vec::new(),
                    symbolic: is_symbolic_spelling(alias),
                    eager: is_eager_input_spelling(alias),
                }),
        )
        .collect()
}

/// Operators whose ASCII and Unicode spellings differ, in declaration order.
static DIFFERING_SPELLINGS: std::sync::LazyLock<Vec<&'static OperatorSpelling>> =
    std::sync::LazyLock::new(|| {
        OPERATOR_SPELLINGS
            .iter()
            .filter(|entry| entry.ascii != entry.unicode)
            .collect()
    });

/// ASCII spellings to recognize during ASCII → Unicode conversion, each paired
/// with the operator it spells. Covers every differing operator's canonical
/// `ascii` plus the [`ASCII_INPUT_ALIASES`] (e.g. `,,` → maplet). Sorted by
/// descending ASCII length so longer spellings match before their prefixes.
static ASCII_TO_UNICODE: std::sync::LazyLock<Vec<(&'static str, &'static OperatorSpelling)>> =
    std::sync::LazyLock::new(|| {
        let mut entries: Vec<(&'static str, &'static OperatorSpelling)> = DIFFERING_SPELLINGS
            .iter()
            .map(|entry| (entry.ascii, *entry))
            .chain(ASCII_INPUT_ALIASES.iter().copied())
            .collect();
        entries.sort_by_key(|(ascii, _)| std::cmp::Reverse(ascii.len()));
        entries
    });

/// Differing operators sorted by descending Unicode length so longer spellings
/// replace before their prefixes during Unicode → ASCII conversion.
static UNICODE_TO_ASCII: std::sync::LazyLock<Vec<&'static OperatorSpelling>> =
    std::sync::LazyLock::new(|| {
        let mut entries = DIFFERING_SPELLINGS.clone();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.unicode.len()));
        entries
    });

pub fn convert_to_unicode(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut byte_pos = 0;
    while byte_pos < text.len() {
        let rest = &text[byte_pos..];
        let mut matched = None;

        for &(ascii, entry) in ASCII_TO_UNICODE.iter() {
            if rest.starts_with(ascii)
                && (!is_alphabetic_op(ascii)
                    || (is_word_boundary(text, byte_pos)
                        && is_word_boundary_end(text, byte_pos + ascii.len())))
            {
                matched = Some((ascii, entry));
                break;
            }
        }

        if let Some((ascii, entry)) = matched {
            // `emit_text` leaves the four private-use operators in ASCII (their
            // glyph would not render) while consuming the full ASCII match, so a
            // spelling like `<<->` is kept whole rather than partly rewritten
            // into the shorter `<->` (`↔`).
            result.push_str(entry.emit_text(true));
            byte_pos += ascii.len();
        } else if let Some(ch) = rest.chars().next() {
            result.push(ch);
            byte_pos += ch.len_utf8();
        } else {
            break;
        }
    }

    result
}

pub fn convert_to_ascii(text: &str) -> String {
    let mut result = text.to_string();
    for entry in UNICODE_TO_ASCII.iter() {
        result = result.replace(entry.unicode, entry.ascii);
    }

    result
}

pub fn has_ascii_operators(text: &str) -> bool {
    DIFFERING_SPELLINGS.iter().any(|entry| {
        if is_alphabetic_op(entry.ascii) {
            contains_whole_word(text, entry.ascii)
        } else {
            text.contains(entry.ascii)
        }
    }) || ASCII_INPUT_ALIASES
        .iter()
        .any(|&(alias, _)| text.contains(alias))
}

pub fn has_unicode_operators(text: &str) -> bool {
    DIFFERING_SPELLINGS
        .iter()
        .any(|entry| text.contains(entry.unicode))
}

/// Every spelling (the Unicode and ASCII form of each entry, plus the ASCII
/// input aliases like `,,`) paired with its character length, sorted by
/// descending length so longer operators match before their prefixes and
/// substrings (`:=` before `:` and `=`). Aliases are included so hovering a
/// literal `,,` still resolves to the maplet.
static SPELLINGS_BY_LENGTH: std::sync::LazyLock<Vec<(&'static str, usize)>> =
    std::sync::LazyLock::new(|| {
        let mut entries: Vec<(&'static str, usize)> = OPERATOR_SPELLINGS
            .iter()
            .flat_map(|entry| [entry.unicode, entry.ascii])
            .chain(ASCII_INPUT_ALIASES.iter().map(|&(alias, _)| alias))
            .map(|text| (text, text.chars().count()))
            .collect();
        entries.sort_by_key(|(_, len)| std::cmp::Reverse(*len));
        entries
    });

/// Maximal munch at a cursor: the longest operator spelling whose occurrence in
/// `line` contains the character at `char_pos`, together with its character
/// range. The cursor may sit on any character of the operator, so hovering the
/// `=` of `:=` still yields `:=`. Alphabetic spellings (`mod`, `NAT`, …) only
/// match between identifier word boundaries, never inside words like `model`.
pub fn operator_at(line: &str, char_pos: usize) -> Option<(&'static str, std::ops::Range<usize>)> {
    let chars: Vec<char> = line.chars().collect();
    if char_pos >= chars.len() {
        return None;
    }

    for &(text, len) in SPELLINGS_BY_LENGTH.iter() {
        for start in (char_pos + 1).saturating_sub(len)..=char_pos {
            if start + len > chars.len()
                || !chars[start..start + len].iter().copied().eq(text.chars())
            {
                continue;
            }
            let blocked = is_alphabetic_op(text)
                && ((start > 0 && is_word_char(chars[start - 1]))
                    || chars.get(start + len).copied().is_some_and(is_word_char));
            if !blocked {
                return Some((text, start..start + len));
            }
        }
    }
    None
}

/// Maximal munch at a byte offset: the longest operator spelling beginning at
/// `byte_pos`, together with its byte range.
pub(crate) fn operator_starting_at(
    input: &str,
    byte_pos: usize,
) -> Option<(&'static str, std::ops::Range<usize>)> {
    if !input.is_char_boundary(byte_pos) {
        return None;
    }
    let rest = &input[byte_pos..];
    for &(text, _) in SPELLINGS_BY_LENGTH.iter() {
        if !rest.starts_with(text) {
            continue;
        }
        let end = byte_pos + text.len();
        let blocked = is_alphabetic_op(text)
            && (input[..byte_pos]
                .chars()
                .next_back()
                .is_some_and(is_word_char)
                || input[end..].chars().next().is_some_and(is_word_char));
        if !blocked {
            return Some((text, byte_pos..end));
        }
    }
    None
}

pub fn binary_op_id(op: BinaryOp) -> OperatorId {
    match op {
        BinaryOp::Add => OperatorId::Add,
        BinaryOp::Subtract => OperatorId::Subtract,
        BinaryOp::Multiply => OperatorId::Multiply,
        BinaryOp::Divide => OperatorId::Divide,
        BinaryOp::Modulo => OperatorId::Modulo,
        BinaryOp::Exponent => OperatorId::Exponent,
        BinaryOp::Range => OperatorId::Range,
        BinaryOp::Union => OperatorId::Union,
        BinaryOp::Intersection => OperatorId::Intersection,
        BinaryOp::Difference => OperatorId::Difference,
        BinaryOp::CartesianProduct => OperatorId::CartesianProduct,
        BinaryOp::Relation => OperatorId::Relation,
        BinaryOp::TotalRelation => OperatorId::TotalRelation,
        BinaryOp::SurjectiveRelation => OperatorId::SurjectiveRelation,
        BinaryOp::TotalSurjectiveRelation => OperatorId::TotalSurjectiveRelation,
        BinaryOp::TotalFunction => OperatorId::TotalFunction,
        BinaryOp::PartialFunction => OperatorId::PartialFunction,
        BinaryOp::TotalInjection => OperatorId::TotalInjection,
        BinaryOp::PartialInjection => OperatorId::PartialInjection,
        BinaryOp::TotalSurjection => OperatorId::TotalSurjection,
        BinaryOp::PartialSurjection => OperatorId::PartialSurjection,
        BinaryOp::Bijection => OperatorId::Bijection,
        BinaryOp::Composition => OperatorId::Composition,
        BinaryOp::Semicolon => OperatorId::Semicolon,
        BinaryOp::DomainRestriction => OperatorId::DomainRestriction,
        BinaryOp::DomainSubtraction => OperatorId::DomainSubtraction,
        BinaryOp::RangeRestriction => OperatorId::RangeRestriction,
        BinaryOp::RangeSubtraction => OperatorId::RangeSubtraction,
        BinaryOp::Overwrite => OperatorId::Overwrite,
        BinaryOp::DirectProduct => OperatorId::DirectProduct,
        BinaryOp::ParallelProduct => OperatorId::ParallelProduct,
        BinaryOp::OfType => OperatorId::OfType,
        BinaryOp::Maplet => OperatorId::Maplet,
    }
}

pub fn unary_op_id(op: UnaryOp) -> OperatorId {
    match op {
        UnaryOp::Minus => OperatorId::UnaryMinus,
        UnaryOp::PowerSet => OperatorId::PowerSet,
        UnaryOp::PowerSet1 => OperatorId::PowerSet1,
        UnaryOp::Domain => OperatorId::Domain,
        UnaryOp::Range => OperatorId::RangeOfRelation,
        UnaryOp::Inverse => OperatorId::Inverse,
    }
}

pub fn comparison_op_id(op: ComparisonOp) -> OperatorId {
    match op {
        ComparisonOp::Equal => OperatorId::Equal,
        ComparisonOp::NotEqual => OperatorId::NotEqual,
        ComparisonOp::LessThan => OperatorId::LessThan,
        ComparisonOp::LessEqual => OperatorId::LessEqual,
        ComparisonOp::GreaterThan => OperatorId::GreaterThan,
        ComparisonOp::GreaterEqual => OperatorId::GreaterEqual,
        ComparisonOp::In => OperatorId::In,
        ComparisonOp::NotIn => OperatorId::NotIn,
        ComparisonOp::Subset => OperatorId::Subset,
        ComparisonOp::SubsetStrict => OperatorId::SubsetStrict,
        ComparisonOp::NotSubset => OperatorId::NotSubset,
        ComparisonOp::NotSubsetStrict => OperatorId::NotSubsetStrict,
    }
}

pub fn logical_op_id(op: LogicalOp) -> OperatorId {
    match op {
        LogicalOp::And => OperatorId::And,
        LogicalOp::Or => OperatorId::Or,
        LogicalOp::Implies => OperatorId::Implies,
        LogicalOp::Equivalent => OperatorId::Equivalent,
    }
}

pub fn quantifier_id(quantifier: Quantifier) -> OperatorId {
    match quantifier {
        Quantifier::ForAll => OperatorId::ForAll,
        Quantifier::Exists => OperatorId::Exists,
    }
}

fn is_alphabetic_op(op: &str) -> bool {
    op.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_word_boundary(text: &str, byte_pos: usize) -> bool {
    if byte_pos > 0
        && let Some(ch) = text.as_bytes().get(byte_pos - 1)
        && (ch.is_ascii_alphanumeric() || *ch == b'_')
    {
        return false;
    }
    true
}

fn is_word_boundary_end(text: &str, byte_pos: usize) -> bool {
    if let Some(ch) = text.as_bytes().get(byte_pos)
        && (ch.is_ascii_alphanumeric() || *ch == b'_')
    {
        return false;
    }
    true
}

fn contains_whole_word(text: &str, word: &str) -> bool {
    let mut start = 0;
    while let Some(pos) = text[start..].find(word) {
        let abs_pos = start + pos;
        if is_word_boundary(text, abs_pos) && is_word_boundary_end(text, abs_pos + word.len()) {
            return true;
        }
        start = abs_pos + word.len();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn aliases_are_unique_and_well_formed() {
        let mut owner: HashMap<&str, OperatorId> = HashMap::new();
        for entry in OPERATOR_SPELLINGS {
            for alias in entry.aliases() {
                assert!(
                    !alias.is_empty()
                        && alias.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                    "alias {alias:?} for {:?} must be a non-empty word token",
                    entry.id
                );
                if let Some(prev) = owner.insert(alias, entry.id) {
                    panic!(
                        "alias {alias:?} claimed by both {prev:?} and {:?}",
                        entry.id
                    );
                }
            }
        }
    }

    #[test]
    fn is_symbolic_matches_ascii_shape() {
        assert!(spelling(OperatorId::Implies).is_symbolic()); // "=>"
        assert!(spelling(OperatorId::Maplet).is_symbolic()); // "|->"
        assert!(spelling(OperatorId::Assignment).is_symbolic()); // ":="
        assert!(!spelling(OperatorId::Naturals).is_symbolic()); // "NAT"
        assert!(!spelling(OperatorId::Or).is_symbolic()); // "or"
    }

    #[test]
    fn is_eager_input_excludes_collisions() {
        assert!(spelling(OperatorId::Implies).is_eager_input()); // "=>"
        assert!(spelling(OperatorId::Range).is_eager_input()); // ".." (multi-char)
        assert!(!spelling(OperatorId::Divide).is_eager_input()); // bare "/" (// comments)
        assert!(!spelling(OperatorId::Dot).is_eager_input()); // bare "." (decimals)
        assert!(!spelling(OperatorId::Union).is_eager_input()); // "\/" contains the leader
        assert!(!spelling(OperatorId::Naturals).is_eager_input()); // alphabetic
    }

    #[test]
    fn comma_comma_is_an_eager_maplet_input_alias() {
        // Rodin's keyboard maps `,,` to the maplet ↦, exactly like `|->`.
        assert_eq!(ascii_input_aliases(OperatorId::Maplet), &[",,"]);
        assert!(is_symbolic_spelling(",,"));
        assert!(is_eager_input_spelling(",,"));
        // It resolves back to the maplet operator (hover, operator_at).
        assert_eq!(lookup_token(",,").map(|op| op.id), Some(OperatorId::Maplet));
        assert_eq!(operator_at("x ,, y", 2), Some((",,", 2..4)));
    }

    #[test]
    fn comma_comma_converts_to_maplet_but_never_back() {
        // ASCII → Unicode rewrites `,,` to the canonical ↦ …
        assert_eq!(convert_to_unicode("x ,, y"), "x ↦ y");
        assert!(has_ascii_operators("x ,, y"));
        // … while a lone comma (list separator) is left untouched.
        assert_eq!(convert_to_unicode("f(a, b)"), "f(a, b)");
        // Unicode → ASCII stays canonical: ↦ becomes `|->`, never `,,`.
        assert_eq!(convert_to_ascii("x ↦ y"), "x |-> y");
    }

    #[test]
    fn surjection_arrows_have_eager_alternative_input_spellings() {
        // Rodin's keyboard registers `+->>`/`-->>` beside `+>>`/`->>`; both are
        // eager-input aliases for the surjection arrows.
        assert_eq!(
            ascii_input_aliases(OperatorId::PartialSurjection),
            &["+->>"]
        );
        assert_eq!(ascii_input_aliases(OperatorId::TotalSurjection), &["-->>"]);
        assert!(is_eager_input_spelling("+->>"));
        assert!(is_eager_input_spelling("-->>"));
        assert_eq!(
            lookup_token("+->>").map(|op| op.id),
            Some(OperatorId::PartialSurjection)
        );
        assert_eq!(
            lookup_token("-->>").map(|op| op.id),
            Some(OperatorId::TotalSurjection)
        );
    }

    #[test]
    fn surjection_aliases_convert_to_unicode_but_never_back() {
        // ASCII → Unicode rewrites the longer forms to the canonical arrows …
        assert_eq!(convert_to_unicode("S +->> T"), "S ⤀ T");
        assert_eq!(convert_to_unicode("S -->> T"), "S ↠ T");
        // … while the partial/total function arrows they extend stay distinct.
        assert_eq!(convert_to_unicode("S +-> T"), "S ⇸ T");
        assert_eq!(convert_to_unicode("S --> T"), "S → T");
        // Unicode → ASCII stays canonical: `+>>`/`->>`, never the alias forms.
        assert_eq!(convert_to_ascii("S ⤀ T"), "S +>> T");
        assert_eq!(convert_to_ascii("S ↠ T"), "S ->> T");
    }

    #[test]
    fn operator_at_returns_whole_multichar_operator() {
        // Cursor on either character of `:=` yields the whole operator.
        assert_eq!(operator_at("count := 0", 6), Some((":=", 6..8)));
        assert_eq!(operator_at("count := 0", 7), Some((":=", 6..8)));
        assert_eq!(operator_at("x :: S", 2), Some(("::", 2..4)));
        assert_eq!(operator_at("x :| P", 3), Some((":|", 2..4)));
        assert_eq!(operator_at("x :∈ S", 3), Some((":∈", 2..4)));
    }

    #[test]
    fn operator_at_prefers_longest_match() {
        // The middle of `<=>` also matches `<=` and `=>`; maximal munch wins.
        for pos in 2..5 {
            assert_eq!(operator_at("a <=> b", pos), Some(("<=>", 2..5)));
        }
        assert_eq!(operator_at("x /= y", 2), Some(("/=", 2..4)));
    }

    #[test]
    fn operator_starting_at_prefers_longest_match_and_byte_ranges() {
        assert_eq!(operator_starting_at("r |>> S", 2), Some(("|>>", 2..5)));
        assert_eq!(operator_starting_at("λx·P", 0), Some(("λ", 0..2)));
        assert_eq!(operator_starting_at("λx·P", 3), Some(("·", 3..5)));
        assert_eq!(operator_starting_at("model", 0), None);
    }

    #[test]
    fn operator_at_single_char_operators() {
        assert_eq!(operator_at("a = b", 2), Some(("=", 2..3)));
        assert_eq!(operator_at("a ∈ S", 2), Some(("∈", 2..3)));
        assert_eq!(operator_at("x ≔ 1", 2), Some(("≔", 2..3)));
    }

    #[test]
    fn operator_at_alphabetic_needs_word_boundaries() {
        assert_eq!(operator_at("a mod b", 3), Some(("mod", 2..5)));
        assert_eq!(operator_at("model", 2), None);
        // A prime never extends a keyword (it attaches only to a plain
        // identifier), so `mod` is still recognized in `mod'` — matching the
        // grammar and Event-B, where `mod'` lexes as `mod` then `'`.
        assert_eq!(operator_at("a mod' b", 3), Some(("mod", 2..5)));
    }

    #[test]
    fn operator_at_misses() {
        // Identifier characters and plain punctuation are not operators.
        assert_eq!(operator_at("abc", 1), None);
        assert_eq!(operator_at("f(x)", 1), None);
        // Out of bounds.
        assert_eq!(operator_at("a = b", 99), None);
        assert_eq!(operator_at("", 0), None);
    }

    /// Extract and un-escape every `"…"` literal on a line, handling the pest
    /// escapes our operator rules use (`\\`, `\"`, `\u{XXXX}`).
    fn pest_string_literals(line: &str) -> Vec<String> {
        let mut lits = Vec::new();
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '"' {
                continue;
            }
            let mut s = String::new();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => match chars.next() {
                        Some('\\') => s.push('\\'),
                        Some('"') => s.push('"'),
                        Some('n') => s.push('\n'),
                        Some('t') => s.push('\t'),
                        Some('r') => s.push('\r'),
                        Some('u') => {
                            // \u{XXXX}
                            if chars.next() == Some('{') {
                                let mut hex = String::new();
                                while let Some(&h) = chars.peek() {
                                    chars.next();
                                    if h == '}' {
                                        break;
                                    }
                                    hex.push(h);
                                }
                                if let Some(ch) =
                                    u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)
                                {
                                    s.push(ch);
                                }
                            }
                        }
                        Some(other) => s.push(other),
                        None => break,
                    },
                    _ => s.push(c),
                }
            }
            lits.push(s);
        }
        lits
    }

    /// The editor-grammar generator renders [`OPERATOR_SPELLINGS`], so that table
    /// must stay the faithful mirror of the `op_*` rules in `grammar.pest`. This
    /// closes the chain `tables → grammar.pest → generated editor grammars` for
    /// operators, the way [`crate::keywords::tests::keywords_match_grammar`] does
    /// for keywords.
    #[test]
    fn operators_match_grammar() {
        use std::collections::HashSet;
        let grammar = include_str!("grammar.pest");

        // `op_*` literals (the symbolic/relational operators) and `kw_*` literals
        // (where the number-set/boolean atoms `ℕ NAT ℤ INT …` live) are collected
        // separately: the forward check accepts either, the reverse check ranges
        // over `op_*` only.
        let mut op_lits: HashSet<String> = HashSet::new();
        let mut atom_lits: HashSet<String> = HashSet::new();
        for line in grammar.lines() {
            let line = line.trim_start();
            let bucket = if line.starts_with("op_") {
                &mut op_lits
            } else if line.starts_with("kw_") {
                &mut atom_lits
            } else {
                continue;
            };
            let rhs = line.split_once('=').map(|x| x.1).unwrap_or("");
            bucket.extend(pest_string_literals(rhs));
        }

        // Forward: every canonical spelling has a matching grammar literal. Atoms
        // are case-insensitive keyword rules (`^"nat"`), so compare lowercased.
        // Skip the syntax tokens the grammar defines under bespoke rule names
        // (`dot`, lambda/quantifier/comprehension rules).
        let defined_elsewhere = [
            OperatorId::Lambda,
            OperatorId::Dot,
            OperatorId::Bar,
            OperatorId::QuantifiedUnion,
            OperatorId::QuantifiedIntersection,
        ];
        let folded: HashSet<String> = op_lits
            .iter()
            .chain(atom_lits.iter())
            .map(|s| s.to_lowercase())
            .collect();
        for op in OPERATOR_SPELLINGS {
            if defined_elsewhere.contains(&op.id) {
                continue;
            }
            for s in [op.unicode, op.ascii] {
                assert!(
                    folded.contains(&s.to_lowercase()),
                    "operator spelling {s:?} ({:?}) has no op_/kw_ literal in grammar.pest",
                    op.id
                );
            }
        }

        // Reverse: every grammar `op_` literal is either a canonical spelling or
        // a registered ASCII input alias (e.g. the `,,` maplet alias; the
        // canonical maplet is `↦`/`|->`). The allowlist is derived from
        // `ascii_input_aliases` so a new alias stays in sync automatically. The
        // `_` of the word-boundary guards lives in the shared `word_char` rule
        // now, so it no longer appears on `op_` lines.
        let aliases: HashSet<&str> = OPERATOR_SPELLINGS
            .iter()
            .flat_map(|op| ascii_input_aliases(op.id).iter().copied())
            .collect();
        let canonical: HashSet<&str> = OPERATOR_SPELLINGS
            .iter()
            .flat_map(|op| [op.unicode, op.ascii])
            .collect();
        for lit in &op_lits {
            if aliases.contains(lit.as_str()) {
                continue;
            }
            assert!(
                canonical.contains(lit.as_str()),
                "grammar op_ literal {lit:?} is missing from OPERATOR_SPELLINGS"
            );
        }

        // Guard the other direction: every registered ASCII input alias must
        // actually appear as a grammar `op_` literal, so the alias SSOT can't
        // drift from `grammar.pest`.
        let op_lit_set: HashSet<&str> = op_lits.iter().map(String::as_str).collect();
        for alias in &aliases {
            assert!(
                op_lit_set.contains(alias),
                "ASCII input alias {alias:?} has no op_ literal in grammar.pest"
            );
        }
    }

    /// The Private-Use Area glyphs in [`OPERATOR_SPELLINGS`] are *exactly* the
    /// four Rodin uses, mapped to the operators Rodin maps them to. This ties the
    /// table to Rodin's source of truth: a stray PUA glyph on a new operator, or a
    /// re-pointed code point, fails here. Raw `\u{E10x}` literals are the
    /// independent anchor, so a wrong named constant is caught too.
    #[test]
    fn private_use_glyphs_match_rodin() {
        use std::collections::HashSet;

        // The named constants hold Rodin's code points verbatim.
        assert_eq!(TOTAL_RELATION, "\u{E100}");
        assert_eq!(SURJECTIVE_RELATION, "\u{E101}");
        assert_eq!(TOTAL_SURJECTIVE_RELATION, "\u{E102}");
        assert_eq!(RELATIONAL_OVERRIDE, "\u{E103}");

        let expected: HashSet<(OperatorId, &str)> = [
            (OperatorId::TotalRelation, "\u{E100}"),
            (OperatorId::SurjectiveRelation, "\u{E101}"),
            (OperatorId::TotalSurjectiveRelation, "\u{E102}"),
            (OperatorId::Overwrite, "\u{E103}"),
        ]
        .into_iter()
        .collect();

        let actual: HashSet<(OperatorId, &str)> = OPERATOR_SPELLINGS
            .iter()
            .filter(|op| is_private_use_glyph(op.unicode))
            .map(|op| (op.id, op.unicode))
            .collect();

        assert_eq!(
            actual, expected,
            "the private-use glyphs in OPERATOR_SPELLINGS diverged from Rodin's set"
        );
    }

    #[test]
    fn is_private_use_glyph_detects_only_the_pua_range() {
        assert!(is_private_use_glyph(RELATIONAL_OVERRIDE));
        assert!(is_private_use_glyph(TOTAL_RELATION));
        // Standard-Unicode operator glyphs and ASCII spellings are not private-use.
        assert!(!is_private_use_glyph("↦"));
        assert!(!is_private_use_glyph("⦂"));
        assert!(!is_private_use_glyph("<<->"));
    }

    /// The invariant this module enforces: `emit_text` never produces a
    /// private-use-area code point, in either spelling mode. Operators whose only
    /// Unicode form is a Rodin private-use glyph fall back to their ASCII
    /// spelling; everything else emits exactly what `text` would.
    #[test]
    fn emit_text_never_yields_a_private_use_glyph() {
        // The invariant: nothing emitted is ever a private-use code point.
        for entry in OPERATOR_SPELLINGS {
            for use_unicode in [true, false] {
                assert!(
                    !is_private_use_glyph(entry.emit_text(use_unicode)),
                    "{:?} emits a private-use glyph (use_unicode={use_unicode})",
                    entry.id
                );
            }
        }

        // The four private-use operators emit their ASCII spelling even in
        // Unicode mode; operators with a portable glyph are left untouched.
        assert_eq!(spelling(OperatorId::Overwrite).emit_text(true), "<+");
        assert_eq!(spelling(OperatorId::TotalRelation).emit_text(true), "<<->");
        assert_eq!(
            spelling(OperatorId::SurjectiveRelation).emit_text(true),
            "<->>"
        );
        assert_eq!(
            spelling(OperatorId::TotalSurjectiveRelation).emit_text(true),
            "<<->>"
        );
        let and = spelling(OperatorId::And);
        assert_eq!(and.emit_text(true), and.unicode);
        assert_eq!(and.emit_text(false), and.ascii);
    }

    /// ASCII → Unicode conversion never introduces a private-use glyph: the four
    /// private-use operators are dropped from the conversion table, so their
    /// ASCII spellings round-trip unchanged.
    #[test]
    fn convert_to_unicode_keeps_private_use_operators_ascii() {
        assert_eq!(convert_to_unicode("f <+ g"), "f <+ g");
        assert_eq!(convert_to_unicode("A <<-> B"), "A <<-> B");
        assert_eq!(convert_to_unicode("A <->> B"), "A <->> B");
        assert_eq!(convert_to_unicode("A <<->> B"), "A <<->> B");
        // Convert-to-ASCII still cleans pasted private-use glyphs back to ASCII.
        assert_eq!(convert_to_ascii("f \u{E103} g"), "f <+ g");
    }

    #[test]
    fn operator_rows_are_well_formed() {
        let rows = operator_rows();
        assert!(!rows.is_empty(), "operator table must not be empty");

        // Every row differs (ascii != unicode) and has non-empty spellings, and
        // no row substitutes to a private-use glyph that would render as tofu.
        // ascii keys must be unique (operator_rows() deduplicates by (ascii, unicode)).
        for row in &rows {
            assert_ne!(row.ascii, row.unicode);
            assert!(!row.ascii.is_empty() && !row.unicode.is_empty());
            assert!(
                !is_private_use_glyph(&row.unicode),
                "operator row {:?} substitutes to a private-use glyph",
                row.ascii
            );
        }

        // The private-use operators emit ASCII, so they have no conversion row.
        for ascii in ["<+", "<<->", "<->>", "<<->>"] {
            assert!(
                !rows.iter().any(|r| r.ascii == ascii),
                "{ascii:?} should not appear in the input-method table"
            );
        }
        let ascii_set: std::collections::HashSet<&str> =
            rows.iter().map(|r| r.ascii.as_str()).collect();
        assert_eq!(
            ascii_set.len(),
            rows.len(),
            "operator_rows() must have unique ascii keys"
        );

        // Representative symbolic op carries aliases and is eager-eligible.
        let implies = rows
            .iter()
            .find(|r| r.ascii == "=>")
            .expect("`=>` should be present");
        assert_eq!(implies.unicode, "⇒");
        assert!(implies.symbolic);
        assert!(implies.eager);
        assert!(implies.aliases.iter().any(|a| a == "implies"));

        // Alphabetic op is leader-only (symbolic and eager both false).
        let nat = rows
            .iter()
            .find(|r| r.ascii == "NAT")
            .expect("`NAT` should be present");
        assert!(!nat.symbolic);
        assert!(!nat.eager);

        // A bare `/` is symbolic but blocklisted from eager (`//` comments).
        let divide = rows
            .iter()
            .find(|r| r.ascii == "/")
            .expect("`/` should be present");
        assert!(divide.symbolic);
        assert!(!divide.eager);

        // The serialized JSON shape the editors consume is covered end-to-end by
        // the LSP wire test `eventb-lsp/tests/operator_table_test.rs`.
    }
}
