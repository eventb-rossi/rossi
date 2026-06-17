//! Rodin-canonical formula formatting.
//!
//! The rossi pretty-printer produces readable Event-B text with
//! generous spacing. Rodin's `.bcc`/`.bcm` attribute values use a
//! tighter canonical form — no spaces around logical operators,
//! comparisons, function application, type ascription, quantifier
//! separators, or commas.
//!
//! Rather than fork the pretty-printer, we post-process its Unicode
//! output. The result is not guaranteed byte-exact with Rodin in every
//! edge case (Rodin's exact rules differ subtly per operator family),
//! but the output is always semantically equivalent — it parses back
//! into the same AST via `parse_predicate_str` / `parse_expression_str`.

use rossi::ast::expression::BinaryOp;
use rossi::pretty::PrettyPrinter;
use rossi::{Action, ActionKind, Expression, ExpressionKind, Predicate};

use crate::type_env::TypeEnv;
use crate::types::Type;

/// Canonicalise a predicate to Rodin's tight form.
pub fn canonical_predicate(p: &Predicate) -> String {
    let raw = PrettyPrinter::new().print_predicate(p);
    tighten(&raw, TightenMode::Predicate)
}

/// Canonicalise an expression to Rodin's tight form.
pub fn canonical_expression(e: &Expression) -> String {
    let raw = PrettyPrinter::new().print_expression(e);
    tighten(&raw, TightenMode::Expression)
}

/// Canonicalise an action (assignment).
pub fn canonical_action(a: &Action) -> String {
    let raw = PrettyPrinter::new().print_action(a);
    tighten(&raw, TightenMode::Action)
}

/// Canonicalise an action, injecting `⦂ T` annotations on any bare
/// empty-set RHS expressions using the known type of the LHS variable.
///
/// Rodin's SC does this during type-checking: `x ≔ ∅` becomes
/// `x ≔ ∅ ⦂ ℙ(USERS)` when `x : ℙ(USERS)`. Only deterministic assignments
/// are affected; `:∈` (becomes-in) and `:|` (becomes-such-that) keep
/// their raw form because the RHS is a set / predicate, not a value.
pub fn canonical_action_with_env(a: &Action, env: &TypeEnv) -> String {
    let annotated = annotate_empty_sets(a, env);
    canonical_action(&annotated)
}

fn annotate_empty_sets(a: &Action, env: &TypeEnv) -> Action {
    match &a.kind {
        ActionKind::Assignment {
            variables,
            expressions,
        } => {
            let mut new_exprs = Vec::with_capacity(expressions.len());
            for (var, expr) in variables.iter().zip(expressions.iter()) {
                new_exprs.push(match (&expr.kind, env.get(var.as_str())) {
                    (ExpressionKind::EmptySet, Some(ty)) => typed_empty_set(ty),
                    _ => expr.clone(),
                });
            }
            ActionKind::Assignment {
                variables: variables.clone(),
                expressions: new_exprs,
            }
            .into()
        }
        _ => a.clone(),
    }
}

fn typed_empty_set(ty: &Type) -> Expression {
    // Rodin only annotates empty sets on set-typed LHS (ℙ(T)).
    // An assignment `n ≔ 0` to an ℤ-typed variable is never an empty set,
    // so we guard here.
    if !matches!(ty, Type::PowerSet(_)) {
        return ExpressionKind::EmptySet.into();
    }
    ExpressionKind::Binary {
        op: BinaryOp::OfType,
        left: Box::new(ExpressionKind::EmptySet.into()),
        right: Box::new(type_to_expression(ty)),
    }
    .into()
}

/// Convert a [`Type`] into the `Expression` shape used for type ascriptions
/// in predicate / action text, so the pretty-printer can emit it.
pub(crate) fn type_to_expression(ty: &Type) -> Expression {
    use ExpressionKind as E;
    match ty {
        Type::Integer => E::Integers.into(),
        Type::Boolean => E::BoolType.into(),
        Type::GivenSet(name) => E::Identifier(name.clone()).into(),
        Type::PowerSet(inner) => E::Unary {
            op: rossi::ast::expression::UnaryOp::PowerSet,
            operand: Box::new(type_to_expression(inner)),
        }
        .into(),
        Type::Product(l, r) => E::Binary {
            op: BinaryOp::CartesianProduct,
            left: Box::new(type_to_expression(l)),
            right: Box::new(type_to_expression(r)),
        }
        .into(),
    }
}

/// Context for whitespace rules. Rodin's canonical form differs slightly
/// between predicates, expressions, and actions — most visibly:
/// - In predicates, `⦂` is tight (`∀x⦂ℤ·P`).
/// - In actions, `⦂` is spaced (`x ≔ ∅ ⦂ ℙ(USERS)`).
#[derive(Debug, Clone, Copy)]
enum TightenMode {
    Predicate,
    Expression,
    Action,
}

fn tighten(input: &str, mode: TightenMode) -> String {
    // Collapse runs of internal whitespace to single spaces first.
    let mut s = collapse_spaces(input);

    // Always-tight operators. Rodin's bcc/bcm canonical form drops spaces
    // around: comparison, logical, multiply (∗), centre-dot (·), additive
    // `+`, the symmetric set ops `∪`, `∩`, `×`, and U+E103 (Rodin's
    // private-use overwrite glyph; 1311 corpus occurrences all tight).
    // It keeps spaces around `−`, `↦`, `‥`, and the asymmetric set ops
    // `∖`, `⩤`, `⩥`, `◁`, `▷`.
    const ALWAYS_TIGHT: &[&str] = &[
        "⊆", "⊂", "⊄", "⊈", "∉", "∈", "≠", "≤", "≥", "=", "<", ">", "∧", "∨", "⇒", "⇔", "¬", "·",
        "∗", "+", "∪", "∩", "×", "\u{E103}",
    ];
    for op in ALWAYS_TIGHT {
        s = strip_space_around(&s, op);
    }
    // `⦂` — tight in predicates (inside quantifier binders), spaced in
    // actions and standalone expressions.
    if matches!(mode, TightenMode::Predicate) {
        s = strip_space_around(&s, "⦂");
    }
    // Comma is tight in predicates (quantifier binders, finite).
    // Expressions use spaced commas inside set enumerations (e.g.
    // Rodin's `LAMP_STATUS = {0,100}` — actually no, samples show no
    // space; treat as tight.)
    s = strip_space_around(&s, ",");
    s.trim().to_string()
}

fn collapse_spaces(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_space = false;
    for c in input.chars() {
        if c == ' ' || c == '\t' || c == '\n' {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out
}

/// Remove a single space immediately before or after each occurrence of
/// `op` in `s`.
fn strip_space_around(s: &str, op: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let bytes = s.as_bytes();
    let op_bytes = op.as_bytes();
    while i < bytes.len() {
        if bytes[i..].starts_with(op_bytes) {
            // Strip trailing space in `out`.
            if out.ends_with(' ') {
                out.pop();
            }
            out.push_str(op);
            i += op_bytes.len();
            // Skip one leading space after.
            if i < bytes.len() && bytes[i] == b' ' {
                i += 1;
            }
        } else {
            // Advance by one char (handle multi-byte).
            let ch_len = utf8_char_len(bytes[i]);
            out.push_str(&s[i..i + ch_len]);
            i += ch_len;
        }
    }
    out
}

fn utf8_char_len(first_byte: u8) -> usize {
    // Continuation bytes (0x80..0xC0) shouldn't appear as a leading byte
    // in well-formed UTF-8; the `< 0xC0` branch is paranoia for robustness.
    match first_byte {
        0..=0xBF => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rossi::parse_predicate_str;

    fn canonical_from_str(src: &str) -> String {
        let p = parse_predicate_str(src).unwrap();
        canonical_predicate(&p)
    }

    #[test]
    fn tight_membership() {
        assert_eq!(canonical_from_str("n ∈ ℕ"), "n∈ℕ");
        assert_eq!(canonical_from_str("register ⊆ USERS"), "register⊆USERS");
    }

    #[test]
    fn arithmetic_inside_function_app() {
        // `f(x) ≤ f(y)` → `f(x)≤f(y)`
        let input = parse_predicate_str("f(x) ≤ f(y)").unwrap();
        assert_eq!(canonical_predicate(&input), "f(x)≤f(y)");
    }

    #[test]
    fn logical_chain_is_tight() {
        let p = parse_predicate_str("x ∈ dom(f) ∧ y ∈ dom(f) ∧ x ≤ y").unwrap();
        assert_eq!(canonical_predicate(&p), "x∈dom(f)∧y∈dom(f)∧x≤y");
    }

    #[test]
    fn empty_set_assignment_gets_powerset_ascription() {
        use rossi::parse_action_str;
        let mut env = TypeEnv::new();
        env.insert("x", Type::pow(Type::GivenSet("USERS".into())));
        let a = parse_action_str("x ≔ ∅").unwrap();
        assert_eq!(canonical_action_with_env(&a, &env), "x ≔ ∅ ⦂ ℙ(USERS)");
    }

    #[test]
    fn integer_assignment_unchanged() {
        use rossi::parse_action_str;
        let mut env = TypeEnv::new();
        env.insert("n", Type::Integer);
        let a = parse_action_str("n ≔ 0").unwrap();
        // `0` isn't an empty set — no ascription.
        assert_eq!(canonical_action_with_env(&a, &env), "n ≔ 0");
    }

    #[test]
    fn empty_set_assignment_without_env_stays_bare() {
        use rossi::parse_action_str;
        let env = TypeEnv::new();
        let a = parse_action_str("x ≔ ∅").unwrap();
        assert_eq!(canonical_action_with_env(&a, &env), "x ≔ ∅");
    }

    #[test]
    fn quantified_predicate_matches_rodin() {
        // From binary-search/C0.bcc axm4.
        let p = parse_predicate_str("∀x⦂ℤ, y⦂ℤ · x ∈ dom(f) ∧ y ∈ dom(f) ∧ x ≤ y ⇒ f(x) ≤ f(y)")
            .unwrap();
        assert_eq!(
            canonical_predicate(&p),
            "∀x⦂ℤ,y⦂ℤ·x∈dom(f)∧y∈dom(f)∧x≤y⇒f(x)≤f(y)"
        );
    }

    #[test]
    fn function_override_canonical_form() {
        use rossi::parse_action_str;
        let env = TypeEnv::new();
        // The parser lowers `f(x) ≔ E` to `f ≔ f\u{E103}{x ↦ E}` directly.
        // canonical_action_with_env tightens the pretty-printed Assignment.
        let a = parse_action_str("currentFloor(c) ≔ f").unwrap();
        assert_eq!(
            canonical_action_with_env(&a, &env),
            "currentFloor ≔ currentFloor\u{E103}{c ↦ f}"
        );
    }

    #[test]
    fn function_override_maplet_arg() {
        use rossi::parse_action_str;
        let env = TypeEnv::new();
        // Override on a pair domain uses a maplet argument `g(a ↦ b) ≔ y`
        // (function application is single-argument); it lowers to the override
        // `g ≔ g <+ {(a ↦ b) ↦ y}`, the maplet printed flat (left-associative).
        let a = parse_action_str("g(a ↦ b) ≔ y").unwrap();
        assert_eq!(
            canonical_action_with_env(&a, &env),
            "g ≔ g\u{E103}{a ↦ b ↦ y}"
        );
    }
}
