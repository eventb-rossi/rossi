//! Canonical Event-B operator spellings.
//!
//! This module is the shared reference for Unicode and ASCII spellings used by
//! the parser-facing tools, pretty-printer, and LSP features.

use crate::ast::expression::{BinaryOp, UnaryOp};
use crate::ast::predicate::{ComparisonOp, LogicalOp, Quantifier};

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

pub fn convert_to_unicode(text: &str) -> String {
    let mut entries: Vec<_> = OPERATOR_SPELLINGS
        .iter()
        .filter(|entry| entry.ascii != entry.unicode)
        .collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.ascii.len()));

    let mut result = String::with_capacity(text.len());
    let mut byte_pos = 0;
    while byte_pos < text.len() {
        let rest = &text[byte_pos..];
        let mut matched = None;

        for entry in &entries {
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
    let mut entries: Vec<_> = OPERATOR_SPELLINGS
        .iter()
        .filter(|entry| entry.ascii != entry.unicode)
        .collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.unicode.len()));

    for entry in entries {
        result = result.replace(entry.unicode, entry.ascii);
    }

    result
}

pub fn has_ascii_operators(text: &str) -> bool {
    OPERATOR_SPELLINGS
        .iter()
        .filter(|entry| entry.ascii != entry.unicode)
        .any(|entry| {
            if is_alphabetic_op(entry.ascii) {
                contains_whole_word(text, entry.ascii)
            } else {
                text.contains(entry.ascii)
            }
        })
}

pub fn has_unicode_operators(text: &str) -> bool {
    OPERATOR_SPELLINGS
        .iter()
        .filter(|entry| entry.ascii != entry.unicode)
        .any(|entry| text.contains(entry.unicode))
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
