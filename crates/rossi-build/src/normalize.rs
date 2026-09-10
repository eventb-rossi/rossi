//! Rodin-canonical formula formatting.
//!
//! Rodin's `.bcc`/`.bcm` attribute values use tighter spacing than readable
//! Event-B text. [`PrettyPrinter`] emits that representation directly from
//! the AST; the typed renderings add the type annotations Rodin's
//! `toStringWithTypes` introduces: every bound declaration carries its type,
//! and every generic atom (`∅`, `id`, `prj1`, `prj2`) is ascribed with the
//! type it was checked at, so the printed formula re-parses to the same
//! typed one. Rodin's static checker and proof-obligation generator both
//! write through that method, and its builder compares strings, so the
//! spelling has to match to the character.

use rossi::formula::{self, FormulaRewriter};
use rossi::pretty::PrettyPrinter;
use rossi::{Expression, Predicate};

/// Canonicalise a predicate to Rodin's tight form.
pub fn canonical_predicate(p: &Predicate) -> String {
    PrettyPrinter::rodin_canonical().print_formula_predicate(p)
}

/// Canonicalise a typed predicate: the tight form with every bound
/// declaration carrying its solved type and every generic atom ascribed
/// (`x ≠ ∅` serialises as `x≠(∅ ⦂ ℙ(T))`).
pub fn canonical_typed_predicate(p: &formula::Predicate) -> String {
    typed_canonical_printer().print_formula_predicate(&p.rewrite(&mut AscribeGenericAtoms))
}

/// See [`canonical_typed_predicate`].
pub fn canonical_typed_expression(e: &formula::Expression) -> String {
    typed_canonical_printer().print_formula_expression(&e.rewrite(&mut AscribeGenericAtoms))
}

/// See [`canonical_typed_predicate`] (`x ≔ ∅` serialises as
/// `x ≔ ∅ ⦂ ℙ(T)`).
pub fn canonical_typed_assignment(a: &formula::Assignment) -> String {
    typed_canonical_printer().print_formula_assignment(&a.rewrite(&mut AscribeGenericAtoms))
}

fn typed_canonical_printer() -> PrettyPrinter {
    PrettyPrinter::rodin_canonical().with_typed_decls(true)
}

/// Wrap every type-checked generic atom in an ascription to its solved
/// type, as Rodin's `toStringWithTypes` spells it. An ascription the
/// source wrote is unwrapped by the same pass (post-order, so its atom has
/// already been re-ascribed), so a written `∅ ⦂ ℙ(T)` comes out once and
/// exactly as an inferred one would: the type on the node is the truth
/// either way.
struct AscribeGenericAtoms;

impl FormulaRewriter for AscribeGenericAtoms {
    fn rewrite_expression(&mut self, expr: &Expression) -> Expression {
        use formula::tag::AtomicOp;
        match (expr.kind(), expr.ty()) {
            (formula::ExpressionKind::Ascription { expr: inner, .. }, _) => inner.clone(),
            (
                formula::ExpressionKind::Atomic(
                    AtomicOp::EmptySet | AtomicOp::KIdGen | AtomicOp::KPrj1Gen | AtomicOp::KPrj2Gen,
                ),
                Some(ty),
            ) => {
                let ff = expr.factory();
                ff.ascription(expr.clone(), ty.to_expression(ff), None)
            }
            _ => expr.clone(),
        }
    }
}

/// Canonicalise an expression to Rodin's tight form.
pub fn canonical_expression(e: &Expression) -> String {
    PrettyPrinter::rodin_canonical().print_formula_expression(e)
}

/// Canonicalise an action body (`skip` or an assignment).
pub fn canonical_action(a: &rossi::ActionBody) -> String {
    PrettyPrinter::rodin_canonical().print_action_body(a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::type_env::TypeEnv;
    use rossi::formula::Type;
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

    /// The typed rendering of an assignment, through the same seam the
    /// checker uses.
    fn canonical_typed_from_str(src: &str, env: &TypeEnv) -> String {
        use rossi::parse_action_str;
        let a = parse_action_str(src).unwrap();
        let typed = crate::sc::typing::typed_assignment(env, &a).expect("assignment type-checks");
        canonical_typed_assignment(&typed)
    }

    /// The typed rendering of a predicate, through the same seam the
    /// checker uses.
    fn canonical_typed_pred_from_str(src: &str, env: &TypeEnv) -> String {
        let p = parse_predicate_str(src).unwrap();
        let typed = crate::sc::typing::typed_predicate(env, &p)
            .unwrap_or_else(|| panic!("`{src}` type-checks"));
        canonical_typed_predicate(&typed)
    }

    #[test]
    fn generic_atoms_in_predicates_are_ascribed_as_rodin_spells_them() {
        let mut env = TypeEnv::new();
        env.insert("USERS", Type::pow(Type::Given("USERS".into())));
        env.insert("x", Type::pow(Type::Given("USERS".into())));
        env.insert(
            "r",
            Type::relation(Type::Given("USERS".into()), Type::Given("USERS".into())),
        );
        // An operand is parenthesised, an argument is not, and the type
        // after the `⦂` keeps its product tight.
        assert_eq!(
            canonical_typed_pred_from_str("x ≠ ∅", &env),
            "x≠(∅ ⦂ ℙ(USERS))"
        );
        assert_eq!(
            canonical_typed_pred_from_str("finite(∅ ∪ x)", &env),
            "finite((∅ ⦂ ℙ(USERS))∪x)"
        );
        assert_eq!(
            canonical_typed_pred_from_str("r = id", &env),
            "r=(id ⦂ ℙ(USERS×USERS))"
        );
        // A written ascription is not doubled: the source spelling and the
        // inferred one come out the same.
        assert_eq!(
            canonical_typed_pred_from_str("x ≠ (∅ ⦂ ℙ(USERS))", &env),
            "x≠(∅ ⦂ ℙ(USERS))"
        );
    }

    #[test]
    fn empty_set_assignment_gets_powerset_ascription() {
        let mut env = TypeEnv::new();
        env.insert("x", Type::pow(Type::Given("USERS".into())));
        assert_eq!(canonical_typed_from_str("x ≔ ∅", &env), "x ≔ ∅ ⦂ ℙ(USERS)");
    }

    #[test]
    fn parallel_assignment_annotates_every_pair() {
        let mut env = TypeEnv::new();
        env.insert("x", Type::pow(Type::Given("USERS".into())));
        env.insert("y", Type::pow(Type::Given("ITEMS".into())));
        assert_eq!(
            canonical_typed_from_str("x, y ≔ ∅, ∅", &env),
            "x,y ≔ ∅ ⦂ ℙ(USERS),∅ ⦂ ℙ(ITEMS)"
        );
    }

    #[test]
    fn integer_assignment_unchanged() {
        let mut env = TypeEnv::new();
        env.insert("n", Type::Int);
        // `0` isn't an empty set — no ascription.
        assert_eq!(canonical_typed_from_str("n ≔ 0", &env), "n ≔ 0");
    }

    #[test]
    fn untyped_assignment_renders_bare() {
        use rossi::parse_action_str;
        // The render-time fallback for a decl with no typed form.
        let a = parse_action_str("x ≔ ∅").unwrap();
        assert_eq!(canonical_action(&a), "x ≔ ∅");
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
        // The parser lowers `f(x) ≔ E` to `f ≔ f\u{E103}{x ↦ E}` directly;
        // the canonical form emits the lowered Assignment.
        let a = parse_action_str("currentFloor(c) ≔ f").unwrap();
        assert_eq!(
            canonical_action(&a),
            "currentFloor ≔ currentFloor\u{E103}{c ↦ f}"
        );
    }

    #[test]
    fn function_override_maplet_arg() {
        use rossi::parse_action_str;
        // Override on a pair domain uses a maplet argument `g(a ↦ b) ≔ y`
        // (function application is single-argument); it lowers to the override
        // `g ≔ g <+ {(a ↦ b) ↦ y}`, the maplet printed flat (left-associative).
        let a = parse_action_str("g(a ↦ b) ≔ y").unwrap();
        assert_eq!(canonical_action(&a), "g ≔ g\u{E103}{a ↦ b ↦ y}");
    }

    /// Rodin keeps relation operators spaced and spells them with private-use
    /// glyphs, while relational override uses a tight private-use glyph.
    #[test]
    fn relation_operators_stay_spaced_with_their_glyph() {
        use rossi::parse_predicate_str;
        let p = parse_predicate_str("r ∈ A <<-> B").unwrap();
        assert_eq!(canonical_predicate(&p), "r∈A \u{E100} B");
    }
}
