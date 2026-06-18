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

    /// True when the ASCII spelling contains no word characters, so it is safe
    /// to substitute eagerly while typing (symbolic combos like `=>`, `|->`),
    /// as opposed to alphabetic ops (`NAT`, `or`, `dom`) which would block
    /// typing ordinary words and are only offered through the `\name` leader.
    pub fn is_symbolic(&self) -> bool {
        !is_alphabetic_op(self.ascii)
    }

    /// True when an as-you-type input method should substitute this operator
    /// eagerly (maximal munch) rather than only through the `\name` leader.
    ///
    /// This is the single source of truth for eager-input eligibility, kept
    /// here with the operator data so editors never re-encode the policy. An
    /// op qualifies when it is symbolic, does not contain the `\` leader, and
    /// is not a bare `/` or `.` (which collide with `//` comments and decimal
    /// or qualifier dots). Multi-character forms like `/=` and `..` still
    /// qualify.
    pub fn is_eager_input(&self) -> bool {
        self.is_symbolic() && !self.ascii.contains('\\') && self.ascii != "/" && self.ascii != "."
    }

    /// Human-friendly leader-key aliases for this operator (e.g. `and`, `to`).
    pub fn aliases(&self) -> &'static [&'static str] {
        aliases_for(self.id)
    }
}

pub const TOTAL_RELATION: &str = "\u{E100}";
pub const SURJECTIVE_RELATION: &str = "\u{E101}";
pub const TOTAL_SURJECTIVE_RELATION: &str = "\u{E102}";
pub const RELATIONAL_OVERRIDE: &str = "\u{E103}";

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

/// Operators whose ASCII and Unicode spellings differ, in declaration order.
static DIFFERING_SPELLINGS: std::sync::LazyLock<Vec<&'static OperatorSpelling>> =
    std::sync::LazyLock::new(|| {
        OPERATOR_SPELLINGS
            .iter()
            .filter(|entry| entry.ascii != entry.unicode)
            .collect()
    });

/// Differing operators sorted by descending ASCII length so longer spellings
/// match before their prefixes during ASCII → Unicode conversion.
static ASCII_TO_UNICODE: std::sync::LazyLock<Vec<&'static OperatorSpelling>> =
    std::sync::LazyLock::new(|| {
        let mut entries = DIFFERING_SPELLINGS.clone();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.ascii.len()));
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

        for entry in ASCII_TO_UNICODE.iter() {
            if rest.starts_with(entry.ascii)
                && (!is_alphabetic_op(entry.ascii)
                    || (is_word_boundary(text, byte_pos)
                        && is_word_boundary_end(text, byte_pos + entry.ascii.len())))
            {
                matched = Some(*entry);
                break;
            }
        }

        if let Some(entry) = matched {
            result.push_str(entry.unicode);
            byte_pos += entry.ascii.len();
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
    })
}

pub fn has_unicode_operators(text: &str) -> bool {
    DIFFERING_SPELLINGS
        .iter()
        .any(|entry| text.contains(entry.unicode))
}

/// Every spelling (the Unicode and ASCII form of each entry) paired with its
/// character length, sorted by descending length so longer operators match
/// before their prefixes and substrings (`:=` before `:` and `=`).
static SPELLINGS_BY_LENGTH: std::sync::LazyLock<Vec<(&'static str, usize)>> =
    std::sync::LazyLock::new(|| {
        let mut entries: Vec<(&'static str, usize)> = OPERATOR_SPELLINGS
            .iter()
            .flat_map(|entry| [entry.unicode, entry.ascii])
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
    fn operator_at_single_char_operators() {
        assert_eq!(operator_at("a = b", 2), Some(("=", 2..3)));
        assert_eq!(operator_at("a ∈ S", 2), Some(("∈", 2..3)));
        assert_eq!(operator_at("x ≔ 1", 2), Some(("≔", 2..3)));
    }

    #[test]
    fn operator_at_alphabetic_needs_word_boundaries() {
        assert_eq!(operator_at("a mod b", 3), Some(("mod", 2..5)));
        assert_eq!(operator_at("model", 2), None);
        // `'` continues an identifier (primed variables), so no `mod` here.
        assert_eq!(operator_at("a mod' b", 3), None);
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

        // Reverse: every grammar `op_` literal is a canonical spelling, except
        // the `,,` empty-set alias. The `_` of the word-boundary guards lives in
        // the shared `word_char` rule now, so it no longer appears on `op_` lines.
        let allow = [",,"];
        let canonical: HashSet<&str> = OPERATOR_SPELLINGS
            .iter()
            .flat_map(|op| [op.unicode, op.ascii])
            .collect();
        for lit in &op_lits {
            if allow.contains(&lit.as_str()) {
                continue;
            }
            assert!(
                canonical.contains(lit.as_str()),
                "grammar op_ literal {lit:?} is missing from OPERATOR_SPELLINGS"
            );
        }
    }
}
