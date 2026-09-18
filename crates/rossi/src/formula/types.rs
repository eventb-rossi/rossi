//! Mathematical types carried by checked formula nodes.
//!
//! The canonical string produced by [`Type::to_rodin_canonical`] is the
//! form written into the `org.eventb.core.type` attribute of checked
//! elements, e.g. `ℙ(USERS×(AUCTIONS×ITEMS))`.
//!
//! Types are always fully solved: there is no type-variable form here.
//! Inference variables exist only inside the type-checker and never
//! escape into node types.

use std::sync::Arc;

use super::tag::Tag;

/// The type of an Event-B expression.
///
/// Inner types are shared with [`Arc`], so cloning a type — which
/// happens for every node a type-check rebuilds — never copies the
/// spine.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type {
    /// `BOOL`
    Bool,
    /// `ℤ`
    Int,
    /// A given set from a carrier-set declaration, e.g. `USERS`.
    Given(String),
    /// `ℙ(T)`
    Pow(Arc<Type>),
    /// `T × U` (left, right)
    Prod(Arc<Type>, Arc<Type>),
    /// An instance of a registered type constructor, e.g. `List(ℤ)`.
    ///
    /// Identified by the constructor's extension tag; the symbol is
    /// carried for display. Tags are stable only within one process, so
    /// parametric types must never be persisted by tag.
    Parametric {
        /// The type constructor's extension tag.
        tag: Tag,
        /// The type constructor's syntax symbol, e.g. `List`.
        symbol: String,
        /// The type parameters, in declaration order.
        params: Vec<Type>,
    },
}

impl Type {
    /// A given set: `Type::given("USERS")` → `USERS`.
    pub fn given(name: impl Into<String>) -> Type {
        Type::Given(name.into())
    }

    /// Powerset convenience constructor: `Type::pow(Type::Int)` → `ℙ(ℤ)`.
    pub fn pow(t: Type) -> Type {
        Type::Pow(Arc::new(t))
    }

    /// Cartesian product convenience constructor.
    pub fn prod(left: Type, right: Type) -> Type {
        Type::Prod(Arc::new(left), Arc::new(right))
    }

    /// Relation / function type `ℙ(left × right)` — Event-B's `left ↔ right`.
    pub fn relation(left: Type, right: Type) -> Type {
        Type::pow(Type::prod(left, right))
    }

    /// A carrier-set `S` has type `ℙ(S)` — the type of the set itself,
    /// not of its elements.
    pub fn carrier_set_type(name: &str) -> Type {
        Type::pow(Type::given(name))
    }

    /// Whether every value of the type can be enumerated, given which
    /// carrier sets `bounded` vouches for.
    ///
    /// `BOOL` is finite and `ℤ` is not. A given set is whatever the caller
    /// knows: the type alone cannot say, since a context may bound it with
    /// `finite(S)` or a partition and the type does not see axioms. A
    /// parametric type may be infinite (`List(BOOL)` is), so it counts as
    /// not.
    pub fn is_finite_with(&self, bounded: &dyn Fn(&str) -> bool) -> bool {
        match self {
            Type::Bool => true,
            Type::Int | Type::Parametric { .. } => false,
            Type::Given(name) => bounded(name),
            Type::Pow(inner) => inner.is_finite_with(bounded),
            Type::Prod(left, right) => {
                left.is_finite_with(bounded) && right.is_finite_with(bounded)
            }
        }
    }

    /// The element type if this is a powerset: `ℙ(T)` → `T`.
    pub fn base_type(&self) -> Option<&Type> {
        match self {
            Type::Pow(inner) => Some(inner),
            _ => None,
        }
    }

    /// The domain type if this is a relational type: `ℙ(α × β)` → `α`.
    pub fn source(&self) -> Option<&Type> {
        match self.base_type()? {
            Type::Prod(left, _) => Some(left),
            _ => None,
        }
    }

    /// The range type if this is a relational type: `ℙ(α × β)` → `β`.
    pub fn target(&self) -> Option<&Type> {
        match self.base_type()? {
            Type::Prod(_, right) => Some(right),
            _ => None,
        }
    }

    /// The expression denoting this type as a set: `ℤ`, `BOOL`, the
    /// given set's identifier (typed `ℙ(S)`), `ℙ(·)` and `×` of the
    /// inner sets, and a parametric type as its type constructor applied
    /// to its parameters' expressions (`List(ℤ)`). The result is
    /// type-checked.
    ///
    /// # Panics
    ///
    /// If a parametric type's constructor is not a type-constructor
    /// extension of `ff`: a type belongs to the factory that declared its
    /// constructor, like every formula node.
    pub fn to_expression(&self, ff: &super::factory::FormulaFactory) -> super::Expression {
        use super::tag::{AtomicOp, BinaryExprOp, UnaryExprOp};
        match self {
            Type::Bool => ff.atomic_expression(AtomicOp::Bool, None, None),
            Type::Int => ff.atomic_expression(AtomicOp::Integer, None, None),
            Type::Given(name) => ff.free_identifier(name, None, Some(Type::pow(self.clone()))),
            Type::Pow(inner) => {
                ff.unary_expression(UnaryExprOp::Pow, inner.to_expression(ff), None)
            }
            Type::Prod(left, right) => ff.binary_expression(
                BinaryExprOp::CProd,
                left.to_expression(ff),
                right.to_expression(ff),
                None,
            ),
            Type::Parametric {
                tag,
                symbol,
                params,
            } => {
                let Some(super::extension::Extension::Expr(constructor)) = ff.extension(*tag)
                else {
                    panic!("parametric type {symbol} is not an extension of this factory")
                };
                assert!(
                    constructor.is_a_type_constructor(),
                    "extension {symbol} is not a type constructor"
                );
                let params = params.iter().map(|p| p.to_expression(ff)).collect();
                ff.extended_expression(
                    constructor,
                    params,
                    Vec::new(),
                    None,
                    Some(Type::pow(self.clone())),
                )
                .expect("a type constructor applied to its parameters")
            }
        }
    }

    /// Appends the names of the given sets occurring in this type, in
    /// traversal order and possibly with duplicates.
    pub fn collect_given_sets(&self, out: &mut Vec<String>) {
        match self {
            Type::Bool | Type::Int => {}
            Type::Given(name) => out.push(name.clone()),
            Type::Pow(inner) => inner.collect_given_sets(out),
            Type::Prod(left, right) => {
                left.collect_given_sets(out);
                right.collect_given_sets(out);
            }
            Type::Parametric { params, .. } => {
                for param in params {
                    param.collect_given_sets(out);
                }
            }
        }
    }

    /// The canonical string, as it appears in the
    /// `org.eventb.core.type="..."` attribute of `.bcc`/`.bcm` elements.
    ///
    /// The form collapses whitespace and uses Unicode symbols only:
    /// - `BOOL`
    /// - `ℤ`
    /// - `USERS`
    /// - `ℙ(ℤ)`
    /// - `USERS×AUCTIONS`
    /// - `ℙ(USERS×(AUCTIONS×ITEMS))`
    /// - `List(ℤ)`
    ///
    /// Products are right-associative and parenthesised only on the
    /// right-hand side of another product (confirmed against
    /// `AuctionMachine.bcm` and `binary-search/M2.bcm`).
    pub fn to_rodin_canonical(&self) -> String {
        let mut out = String::new();
        self.write_canonical(&mut out);
        out
    }

    /// Parses a canonical string back into a type: the inverse of
    /// [`Type::to_rodin_canonical`] for the extension-free forms found
    /// in `org.eventb.core.type` attributes.
    ///
    /// `None` when the string is not an extension-free type spelling —
    /// malformed input, a non-type expression such as `1+2`, or a
    /// parametric type like `List(ℤ)`, which parses as a function
    /// application and is rejected by the interpretation step. A
    /// parametric spelling resolves through [`Type::parse_rodin_with`].
    pub fn parse_rodin(s: &str) -> Option<Type> {
        Self::parse_rodin_with(s, &super::factory::DEFAULT)
    }

    /// [`Type::parse_rodin`] against `ff`: a type constructor of `ff`
    /// applied to type spellings (`List(ℤ)`, `ℙ(List(T)×T)`) is the
    /// parametric type, read through the formula parser. The canonical fast
    /// path knows the factory only to decline a word that names one of its
    /// operators, which is never a given set.
    pub fn parse_rodin_with(s: &str, ff: &super::factory::FormulaFactory) -> Option<Type> {
        parse_canonical(s, ff).or_else(|| parse_spelled_with(s, ff))
    }

    fn write_canonical(&self, out: &mut String) {
        match self {
            Type::Bool => out.push_str("BOOL"),
            Type::Int => out.push('ℤ'),
            Type::Given(name) => out.push_str(name),
            Type::Pow(inner) => {
                out.push('ℙ');
                out.push('(');
                inner.write_canonical(out);
                out.push(')');
            }
            Type::Prod(left, right) => {
                left.write_canonical(out);
                out.push('×');
                // Right operand of a product gets parenthesised if it is
                // itself a product; the left one never is.
                match right.as_ref() {
                    Type::Prod(..) => {
                        out.push('(');
                        right.write_canonical(out);
                        out.push(')');
                    }
                    _ => right.write_canonical(out),
                }
            }
            Type::Parametric { symbol, params, .. } => {
                out.push_str(symbol);
                if let Some((first, rest)) = params.split_first() {
                    out.push('(');
                    first.write_canonical(out);
                    for param in rest {
                        out.push(',');
                        param.write_canonical(out);
                    }
                    out.push(')');
                }
            }
        }
    }
}

/// A type spelling read through the formula parser against `ff`: the
/// general path, and the authority on everything [`parse_canonical`]
/// declines.
fn parse_spelled_with(s: &str, ff: &super::factory::FormulaFactory) -> Option<Type> {
    let expr = crate::parser::parse_expression_str_with(s, ff).ok()?;
    super::typecheck::type_from_expression(&expr)
}

/// The canonical spellings of [`Type::to_rodin_canonical`] read
/// directly: `ℤ`, `BOOL`, an identifier, `ℙ(T)`, `(T)` and products,
/// folded left like the formula parser does, with no whitespace. Proof
/// files repeat a few dozen such spellings millions of times, and the
/// formula parser costs microseconds per call.
///
/// The only claim is inclusion: a `Some` here is what [`parse_spelled_with`]
/// returns too. Every other input — whitespace, the ASCII operator
/// spellings, `ℙ1`, primes, non-ASCII identifier characters, reserved
/// or keyword words, an application `S(x)`, deep nesting — is `None`,
/// leaving the formula parser to decide, so this never has to know how
/// the parser treats an unusual spelling.
fn parse_canonical(s: &str, ff: &super::factory::FormulaFactory) -> Option<Type> {
    let mut cursor = Canonical { rest: s, ff };
    let ty = cursor.product(0)?;
    cursor.rest.is_empty().then_some(ty)
}

/// The cursor of [`parse_canonical`].
struct Canonical<'a> {
    rest: &'a str,
    /// The factory whose operator symbols are never given sets.
    ff: &'a super::factory::FormulaFactory,
}

impl Canonical<'_> {
    /// Parenthesis depth beyond which the formula parser takes over
    /// (with its own nesting limit and stack growth).
    const MAX_DEPTH: usize = 32;

    fn eat(&mut self, token: &str) -> bool {
        match self.rest.strip_prefix(token) {
            Some(rest) => {
                self.rest = rest;
                true
            }
            None => false,
        }
    }

    /// `atom ('×' atom)*`, left-nested.
    fn product(&mut self, depth: usize) -> Option<Type> {
        let mut ty = self.atom(depth)?;
        while self.eat("×") {
            ty = Type::prod(ty, self.atom(depth)?);
        }
        Some(ty)
    }

    fn atom(&mut self, depth: usize) -> Option<Type> {
        if depth > Self::MAX_DEPTH {
            return None;
        }
        if self.eat("ℤ") {
            return Some(Type::Int);
        }
        if self.eat("ℙ(") {
            let inner = self.product(depth + 1)?;
            return self.eat(")").then(|| Type::pow(inner));
        }
        if self.eat("(") {
            let inner = self.product(depth + 1)?;
            return self.eat(")").then_some(inner);
        }
        // An identifier: the grammar's `ident_core`, as `names` spells
        // it. `BOOL` is a keyword token before it is a word, as in the
        // grammar; an operator symbol of the factory is a token too.
        let mut chars = self.rest.chars();
        let first = chars.next()?;
        if !crate::names::is_math_identifier_start(first) {
            return None;
        }
        let tail = chars.as_str();
        let len = first.len_utf8()
            + tail
                .find(|c| !crate::names::is_math_identifier_part(c))
                .unwrap_or(tail.len());
        let (word, rest) = self.rest.split_at(len);
        self.rest = rest;
        if word == "BOOL" {
            Some(Type::Bool)
        } else if crate::builtins::is_reserved_name(word)
            || crate::keywords::is_keyword(word)
            || self.ff.extension_by_symbol(word).is_some()
        {
            None
        } else {
            Some(Type::given(word))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_primitives() {
        assert_eq!(Type::Int.to_rodin_canonical(), "ℤ");
        assert_eq!(Type::Bool.to_rodin_canonical(), "BOOL");
        assert_eq!(Type::given("USERS").to_rodin_canonical(), "USERS");
    }

    #[test]
    fn canonical_carrier_set() {
        // A carrier set USERS has type ℙ(USERS) — what appears on
        // scCarrierSet.
        assert_eq!(
            Type::carrier_set_type("USERS").to_rodin_canonical(),
            "ℙ(USERS)"
        );
    }

    #[test]
    fn canonical_flat_product() {
        // AUCTIONS × ITEMS
        let t = Type::prod(Type::given("AUCTIONS"), Type::given("ITEMS"));
        assert_eq!(t.to_rodin_canonical(), "AUCTIONS×ITEMS");
    }

    #[test]
    fn canonical_right_nested_product() {
        // USERS × (AUCTIONS × ITEMS) — from AuctionMachine.bcm's `buyer`.
        let t = Type::prod(
            Type::given("USERS"),
            Type::prod(Type::given("AUCTIONS"), Type::given("ITEMS")),
        );
        assert_eq!(t.to_rodin_canonical(), "USERS×(AUCTIONS×ITEMS)");
    }

    #[test]
    fn canonical_left_nested_product_stays_flat() {
        // (A × B) × C prints without parentheses: `×` is
        // left-associative (kernel_lang p.18), so the flat spelling
        // A×B×C re-reads as (A×B)×C — only a product in the right
        // operand needs parentheses to survive a round-trip.
        let t = Type::prod(
            Type::prod(Type::given("A"), Type::given("B")),
            Type::given("C"),
        );
        assert_eq!(t.to_rodin_canonical(), "A×B×C");
    }

    #[test]
    fn canonical_powerset_of_product() {
        let t = Type::pow(Type::prod(
            Type::given("USERS"),
            Type::prod(Type::given("AUCTIONS"), Type::given("ITEMS")),
        ));
        assert_eq!(t.to_rodin_canonical(), "ℙ(USERS×(AUCTIONS×ITEMS))");
    }

    #[test]
    fn canonical_parametric() {
        let list_int = Type::Parametric {
            tag: 1000,
            symbol: "List".into(),
            params: vec![Type::Int],
        };
        assert_eq!(list_int.to_rodin_canonical(), "List(ℤ)");

        let pair = Type::Parametric {
            tag: 1001,
            symbol: "Pair".into(),
            params: vec![Type::Int, Type::Bool],
        };
        assert_eq!(pair.to_rodin_canonical(), "Pair(ℤ,BOOL)");

        let enumeration = Type::Parametric {
            tag: 1002,
            symbol: "Direction".into(),
            params: vec![],
        };
        assert_eq!(enumeration.to_rodin_canonical(), "Direction");
    }

    #[test]
    fn parse_rodin_roundtrip() {
        // Every extension-free canonical form parses back to the type
        // that produced it.
        let types = [
            Type::Int,
            Type::Bool,
            Type::given("USERS"),
            Type::pow(Type::Int),
            Type::carrier_set_type("USERS"),
            Type::prod(Type::given("AUCTIONS"), Type::given("ITEMS")),
            Type::prod(
                Type::given("USERS"),
                Type::prod(Type::given("AUCTIONS"), Type::given("ITEMS")),
            ),
            Type::prod(
                Type::prod(Type::given("A"), Type::given("B")),
                Type::given("C"),
            ),
            Type::pow(Type::prod(
                Type::given("USERS"),
                Type::prod(Type::given("AUCTIONS"), Type::given("ITEMS")),
            )),
            Type::relation(Type::Int, Type::given("S")),
        ];
        for t in types {
            let canonical = t.to_rodin_canonical();
            // The fast path handles every canonical form by itself.
            assert_eq!(
                parse_canonical(&canonical, &crate::formula::factory::DEFAULT),
                Some(t.clone()),
                "{canonical}"
            );
            assert_eq!(Type::parse_rodin(&canonical), Some(t));
        }
    }

    /// The token alphabet of the differential tests: the canonical
    /// tokens, each class of spelling the formula parser treats
    /// specially (keyword tokens, reserved words, ASCII operator
    /// spellings, structural keywords), and near misses.
    const TOKENS: [&str; 24] = [
        "ℤ", "BOOL", "S", "x", "ℙ(", "(", ")", "×", "INT", "NAT", "POW", "ℙ1(", "card", "id",
        "true", "end", "skip", "x'", "BOOLEAN", "_a", "1", "**", " ", "é",
    ];

    /// Whenever the fast path accepts a string, the formula parser
    /// agrees — the inclusion `parse_canonical` promises.
    fn agrees_with_parser(s: &str) {
        if let Some(fast) = parse_canonical(s, &crate::formula::factory::DEFAULT) {
            let default = super::super::factory::FormulaFactory::default_factory();
            assert_eq!(parse_spelled_with(s, &default), Some(fast), "{s:?}");
        }
    }

    #[test]
    fn canonical_fast_path_agrees_with_parser() {
        // Every sequence of up to three tokens.
        let mut sequences = vec![String::new()];
        for _ in 0..3 {
            let next: Vec<String> = sequences
                .iter()
                .flat_map(|prefix| TOKENS.iter().map(move |token| format!("{prefix}{token}")))
                .collect();
            for s in &next {
                agrees_with_parser(s);
            }
            sequences = next;
        }
    }

    proptest::proptest! {
        /// Longer random token sequences.
        #[test]
        fn canonical_fast_path_agrees_with_parser_on_long_inputs(
            tokens in proptest::collection::vec(0..TOKENS.len(), 0..10)
        ) {
            let s: String = tokens.iter().map(|&i| TOKENS[i]).collect();
            agrees_with_parser(&s);
        }
    }

    /// Every ASCII word the grammar reads as a token, and every ASCII
    /// operator spelling, is declined by the fast path — except `BOOL`,
    /// the one keyword it reads itself: the word lists it consults must
    /// keep up with `grammar.pest`.
    #[test]
    fn grammar_words_are_declined_by_fast_path() {
        let grammar = include_str!("../grammar.pest");
        let mut words: Vec<String> = grammar
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .flat_map(crate::operators::pest_string_literals)
            .collect();
        words.extend(
            crate::operators::OPERATOR_SPELLINGS
                .iter()
                .map(|op| op.ascii.to_string()),
        );
        // Token words only: `"_"` is a character-class literal, and an
        // identifier the fast path reads like the formula parser does.
        words.retain(|word| {
            word.starts_with(|c: char| c.is_ascii_alphabetic())
                && crate::names::is_valid_math_identifier(word)
        });
        assert!(words.iter().any(|word| word == "POW"), "{words:?}");
        let accepted: Vec<String> = words
            .into_iter()
            .filter(|word| {
                word != "BOOL" && parse_canonical(word, &crate::formula::factory::DEFAULT).is_some()
            })
            .collect();
        assert!(accepted.is_empty(), "{accepted:?}");
    }

    /// Spellings the formula parser owns are declined by the fast path
    /// and still resolved by `parse_rodin`.
    #[test]
    fn parser_owned_spellings_fall_through() {
        for (s, expected) in [
            ("INT", Some(Type::Int)),
            ("POW(ℤ)", Some(Type::pow(Type::Int))),
            ("ℤ × S", Some(Type::prod(Type::Int, Type::given("S")))),
            (
                "S↔T",
                Some(Type::relation(Type::given("S"), Type::given("T"))),
            ),
            ("card", None),
            ("S(x)", None),
        ] {
            assert_eq!(
                parse_canonical(s, &crate::formula::factory::DEFAULT),
                None,
                "{s}"
            );
            assert_eq!(Type::parse_rodin(s), expected, "{s}");
        }
    }

    #[test]
    fn parse_rodin_flat_product_is_left_nested() {
        // The flat spelling is the canonical form of the left-nested
        // product, so it must parse back left-nested.
        assert_eq!(
            Type::parse_rodin("A×B×C"),
            Some(Type::prod(
                Type::prod(Type::given("A"), Type::given("B")),
                Type::given("C"),
            ))
        );
    }

    #[test]
    fn parse_rodin_rejects_non_types() {
        // Well-formed expressions that are not type spellings, the
        // parametric form (a function application to the parser), and
        // malformed input.
        for s in ["1+2", "S∪T", "x↦y", "{1}", "ℙ1(S)", "List(ℤ)", "ℙ(", ""] {
            assert_eq!(Type::parse_rodin(s), None, "accepted {s:?}");
        }
    }

    #[test]
    fn relation_constructor() {
        // relation(α, β) is ℙ(α×β) — equal to the primitive spelling and
        // rendering the same canonical string.
        let r = Type::relation(Type::Int, Type::given("S"));
        assert_eq!(r, Type::pow(Type::prod(Type::Int, Type::given("S"))));
        assert_eq!(r.to_rodin_canonical(), "ℙ(ℤ×S)");
    }

    #[test]
    fn relational_accessors() {
        let r = Type::relation(Type::Int, Type::given("S"));
        assert_eq!(
            r.base_type(),
            Some(&Type::prod(Type::Int, Type::given("S")))
        );
        assert_eq!(r.source(), Some(&Type::Int));
        assert_eq!(r.target(), Some(&Type::given("S")));

        // A powerset of a non-product has a base type but no source or
        // target; a bare type has neither.
        let p = Type::pow(Type::Int);
        assert_eq!(p.base_type(), Some(&Type::Int));
        assert_eq!(p.source(), None);
        assert_eq!(p.target(), None);
        assert_eq!(Type::Int.base_type(), None);
        assert_eq!(Type::Int.source(), None);
    }
}
