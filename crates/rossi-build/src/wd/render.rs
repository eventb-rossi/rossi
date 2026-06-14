//! Rodin-`toString` renderer for WD lemmas.
//!
//! Renders a predicate the way Rodin's `Formula.toString()` does, because
//! the WD message contract is byte-identical output to eventb-checker
//! (which prints Rodin `Predicate#toString()` verbatim). This differs from
//! the Camille canonical form in `crate::normalize` on several axes, so it
//! gets its own renderer rather than retrofitting `rossi::pretty`:
//!
//! - Rodin-*associative* expression operators (`∪ ∩ + ∗ ∘ ; `) print
//!   tight, all other infix expression operators (including `↦`, `−`,
//!   `∖`, `×`, the arrows) get one space each side: `Union ⇸ ℙ(Union × Names)`.
//! - Relational and logical predicate operators are always tight:
//!   `∀e·e∈dom(f)⇒f∈S ⇸ T`.
//! - Bound-identifier declarations never show types (`∀x·…`, not `∀x⦂ℤ·…`).
//! - The comprehension bar is spaced: `{y·y∈s ∣ y}`.
//!
//! Parenthesization starts from the shared `rossi::op_info` table and is
//! patched against eventb-checker oracle findings.

use rossi::ast::expression::{BinaryOp, IdentPattern, UnaryOp};
use rossi::ast::predicate::LogicalOp;
use rossi::operators::{self, binary_op_id, comparison_op_id, logical_op_id, quantifier_id};
use rossi::{Expression, Predicate, TypedIdentifier, op_info};

/// Render a predicate as Rodin's `Predicate#toString()` would.
#[must_use]
pub fn render_predicate(p: &Predicate) -> String {
    let mut s = String::new();
    pred(&mut s, p);
    s
}

/// Render an expression as Rodin's `Expression#toString()` would.
#[must_use]
pub fn render_expression(e: &Expression) -> String {
    let mut s = String::new();
    expr(&mut s, e);
    s
}

fn spell(id: operators::OperatorId) -> &'static str {
    operators::spell(id, true)
}

// ---------------------------------------------------------------------
// Predicates
// ---------------------------------------------------------------------

fn pred(out: &mut String, p: &Predicate) {
    match p {
        Predicate::True => out.push('⊤'),
        Predicate::False => out.push('⊥'),
        Predicate::Comparison { op, left, right } => {
            expr_comparison_operand(out, left);
            out.push_str(spell(comparison_op_id(*op)));
            expr_comparison_operand(out, right);
        }
        Predicate::Not(inner) => {
            out.push('¬');
            pred_not_operand(out, inner);
        }
        Predicate::Logical { op, .. } => {
            let mut children: Vec<&Predicate> = Vec::new();
            flatten_logical(p, *op, &mut children);
            let sym = spell(logical_op_id(*op));
            for (i, child) in children.iter().enumerate() {
                if i > 0 {
                    out.push_str(sym);
                }
                pred_logical_operand(out, child, *op);
            }
        }
        Predicate::Quantified {
            quantifier,
            identifiers,
            predicate,
        } => {
            out.push_str(spell(quantifier_id(*quantifier)));
            decls(out, identifiers);
            out.push('·');
            pred(out, predicate); // quantifier body extends to the right
        }
        Predicate::Application {
            function,
            arguments,
        } => {
            out.push_str(function);
            out.push('(');
            args(out, arguments);
            out.push(')');
        }
        Predicate::BuiltinApplication {
            predicate,
            arguments,
        } => {
            out.push_str(predicate.name());
            out.push('(');
            args(out, arguments);
            out.push(')');
        }
    }
}

/// Collect the children of a same-operator ∧/∨ chain (any nesting shape),
/// mirroring Rodin's n-ary associative predicates after `flatten()`.
/// `⇒` and `⇔` are binary in Rodin, so they only ever yield two children.
fn flatten_logical<'a>(p: &'a Predicate, op: LogicalOp, out: &mut Vec<&'a Predicate>) {
    match p {
        Predicate::Logical {
            op: child_op,
            left,
            right,
        } if *child_op == op && matches!(op, LogicalOp::And | LogicalOp::Or) => {
            flatten_logical(left, op, out);
            flatten_logical(right, op, out);
        }
        Predicate::Logical {
            op: child_op,
            left,
            right,
        } if *child_op == op => {
            // ⇒ / ⇔ — binary, left then right.
            out.push(left);
            out.push(right);
        }
        _ => out.push(p),
    }
}

fn pred_logical_operand(out: &mut String, child: &Predicate, parent_op: LogicalOp) {
    let needs_parens = match child {
        // Quantifiers sit below the connectives in Rodin's grammar: they
        // are parenthesized as operands, even as the right child of `⇒`
        // (`…⇒(∀p,n·…)` in the oracle output).
        Predicate::Quantified { .. } => true,
        Predicate::Logical { op: child_op, .. } => {
            let child_prec = op_info::logical_precedence(*child_op);
            let parent_prec = op_info::logical_precedence(parent_op);
            // Same-op ∧/∨ chains were flattened away; what remains needs
            // parens when not strictly tighter than the parent.
            child_prec <= parent_prec
        }
        _ => false,
    };
    if needs_parens {
        out.push('(');
        pred(out, child);
        out.push(')');
    } else {
        pred(out, child);
    }
}

fn pred_not_operand(out: &mut String, child: &Predicate) {
    match child {
        Predicate::Logical { .. } | Predicate::Quantified { .. } => {
            out.push('(');
            pred(out, child);
            out.push(')');
        }
        _ => pred(out, child),
    }
}

// ---------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------

/// True for operators Rodin models as `AssociativeExpression` — these
/// print tight (`a∪b`, `a+b`). Every other infix expression operator is
/// a Rodin `BinaryExpression` and gets one space each side (`a − b`,
/// `S × T`, `f  g`… exception: `` is associative, see list).
fn is_rodin_associative(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Add
            | BinaryOp::Multiply
            | BinaryOp::Union
            | BinaryOp::Intersection
            | BinaryOp::Composition
            | BinaryOp::Semicolon
            | BinaryOp::Overwrite
    )
}

fn expr(out: &mut String, e: &Expression) {
    match e {
        Expression::Integer(n) => out.push_str(&n.to_string()),
        Expression::Identifier(name) => out.push_str(name),
        Expression::True => out.push_str("TRUE"),
        Expression::False => out.push_str("FALSE"),
        Expression::EmptySet => out.push('∅'),
        Expression::Naturals => out.push('ℕ'),
        Expression::Naturals1 => out.push_str("ℕ1"),
        Expression::Integers => out.push('ℤ'),
        Expression::BoolType => out.push_str("BOOL"),
        Expression::StringLiteral(s) => {
            out.push('"');
            out.push_str(s);
            out.push('"');
        }
        Expression::SetEnumeration(items) => {
            out.push('{');
            args(out, items);
            out.push('}');
        }
        Expression::SetComprehension {
            identifiers,
            predicate,
            expression,
        } => {
            out.push('{');
            match expression {
                Some(body) => {
                    decls(out, identifiers);
                    out.push('·');
                    pred(out, predicate);
                    out.push_str(" ∣ ");
                    expr(out, body);
                }
                None => {
                    decls(out, identifiers);
                    out.push_str(" ∣ ");
                    pred(out, predicate);
                }
            }
            out.push('}');
        }
        Expression::SetBuilder {
            member_expression,
            predicate,
        } => {
            out.push('{');
            expr(out, member_expression);
            out.push_str(" ∣ ");
            pred(out, predicate);
            out.push('}');
        }
        Expression::RelationalImage { relation, set } => {
            expr_tight_operand(out, relation);
            out.push('[');
            expr(out, set);
            out.push(']');
        }
        Expression::QuantifiedUnion {
            identifiers,
            predicate,
            expression,
        } => quantified_expr(out, "⋃", identifiers, predicate, expression),
        Expression::QuantifiedInter {
            identifiers,
            predicate,
            expression,
        } => quantified_expr(out, "⋂", identifiers, predicate, expression),
        Expression::Lambda {
            pattern,
            predicate,
            expression,
        } => {
            out.push('λ');
            ident_pattern(out, pattern);
            out.push('·');
            pred(out, predicate);
            out.push_str(" ∣ ");
            expr(out, expression);
        }
        Expression::Binary {
            op: BinaryOp::OfType,
            left,
            ..
        } => {
            // Rodin folds the `⦂T` type ascription into the atomic's type
            // at type-check, so `Predicate#toString()` prints only the
            // ascribed expression (`finite(∅⦂ℙ(S))` → `finite(∅)`).
            // Verified against eventb-checker.
            expr(out, left);
        }
        Expression::Binary { op, left, right } => {
            expr_binary_operand(out, left, *op, false);
            if is_rodin_associative(*op) {
                out.push_str(spell(binary_op_id(*op)));
            } else {
                out.push(' ');
                out.push_str(spell(binary_op_id(*op)));
                out.push(' ');
            }
            expr_binary_operand(out, right, *op, true);
        }
        Expression::Unary { op, operand } => match op {
            UnaryOp::Minus => {
                out.push('−');
                expr_minus_operand(out, operand);
            }
            UnaryOp::PowerSet => call(out, "ℙ", operand),
            UnaryOp::PowerSet1 => call(out, "ℙ1", operand),
            UnaryOp::Domain => call(out, "dom", operand),
            UnaryOp::Range => call(out, "ran", operand),
            UnaryOp::Inverse => {
                expr_tight_operand(out, operand);
                out.push('∼');
            }
        },
        Expression::FunctionApplication {
            function,
            arguments,
        } => {
            expr_tight_operand(out, function);
            out.push('(');
            args(out, arguments);
            out.push(')');
        }
        Expression::BuiltinApplication {
            function,
            arguments,
        } => {
            out.push_str(function.name());
            out.push('(');
            args(out, arguments);
            out.push(')');
        }
        Expression::Bool(p) => {
            out.push_str("bool(");
            pred(out, p);
            out.push(')');
        }
        Expression::IfThenElse {
            condition,
            then_expr,
            else_expr,
        } => {
            // ProB extension — never produced by Rodin, rendered for
            // completeness only.
            out.push_str("IF ");
            pred(out, condition);
            out.push_str(" THEN ");
            expr(out, then_expr);
            out.push_str(" ELSE ");
            expr(out, else_expr);
            out.push_str(" END");
        }
    }
}

fn quantified_expr(
    out: &mut String,
    sym: &str,
    identifiers: &[TypedIdentifier],
    predicate: &Predicate,
    expression: &Expression,
) {
    out.push_str(sym);
    decls(out, identifiers);
    out.push('·');
    pred(out, predicate);
    out.push_str(" ∣ ");
    expr(out, expression);
}

fn call(out: &mut String, name: &str, operand: &Expression) {
    out.push_str(name);
    out.push('(');
    expr(out, operand);
    out.push(')');
}

/// Operand of a binary expression operator: parenthesized per the shared
/// precedence/compatibility table.
fn expr_binary_operand(out: &mut String, child: &Expression, parent_op: BinaryOp, is_right: bool) {
    let needs_parens = match strip_oftype(child) {
        Expression::Lambda { .. }
        | Expression::QuantifiedUnion { .. }
        | Expression::QuantifiedInter { .. } => true,
        Expression::Unary {
            op: UnaryOp::Minus, ..
        } => {
            // Unary minus binds at additive precedence, left-associatively
            // (`(−c)∗d`, but `−a+b` / `−a − b` bare). Verified against
            // eventb-checker.
            let parent_prec = op_info::binary_precedence(parent_op);
            let child_prec = op_info::unary_minus_precedence();
            child_prec < parent_prec || (child_prec == parent_prec && is_right)
        }
        Expression::Binary { op: child_op, .. } => {
            let child_prec = op_info::binary_precedence(*child_op);
            let parent_prec = op_info::binary_precedence(parent_op);
            if child_prec < parent_prec {
                true
            } else if child_prec > parent_prec {
                false
            } else if *child_op == BinaryOp::CartesianProduct
                && parent_op == BinaryOp::CartesianProduct
            {
                // Rodin prints left-nested × chains bare:
                // `interaction_modes × ℤ × ℙ(AIRPLANES × ℤ)`.
                is_right
            } else if !op_info::binary_ops_compatible(*child_op, parent_op)
                || op_info::is_non_associative(parent_op)
            {
                true
            } else {
                // Left-associative: right child needs parens.
                is_right
            }
        }
        _ => false,
    };
    if needs_parens {
        out.push('(');
        expr(out, child);
        out.push(')');
    } else {
        expr(out, child);
    }
}

/// See through a source-level `⦂` type ascription: Rodin folds the type
/// into the atomic's type at type-check, so `e⦂T` renders and
/// parenthesizes exactly as `e`. Idempotent on every other expression.
fn strip_oftype(e: &Expression) -> &Expression {
    let mut cur = e;
    while let Expression::Binary {
        op: BinaryOp::OfType,
        left,
        ..
    } = cur
    {
        cur = left;
    }
    cur
}

/// Operand of unary `−`. Rodin binds unary minus at additive precedence,
/// so a tighter operand (`∗`/`÷`/`mod`/`^`) prints bare (`−a∗b`) while a
/// same-or-looser binary, or a nested `−`, needs parens (`−(a+b)`,
/// `−(−a)`). Verified against eventb-checker.
fn expr_minus_operand(out: &mut String, child: &Expression) {
    let needs_parens = match strip_oftype(child) {
        Expression::Binary { op, .. } => {
            op_info::binary_precedence(*op) <= op_info::unary_minus_precedence()
        }
        Expression::Unary {
            op: UnaryOp::Minus, ..
        }
        | Expression::Lambda { .. }
        | Expression::QuantifiedUnion { .. }
        | Expression::QuantifiedInter { .. } => true,
        _ => false,
    };
    if needs_parens {
        out.push('(');
        expr(out, child);
        out.push(')');
    } else {
        expr(out, child);
    }
}

/// Operand in a tight position: function position of an application,
/// relation of `r[S]`, postfix `∼`. Only atom-like expressions go bare.
fn expr_tight_operand(out: &mut String, child: &Expression) {
    let bare = matches!(
        strip_oftype(child),
        Expression::Integer(_)
            | Expression::Identifier(_)
            | Expression::True
            | Expression::False
            | Expression::EmptySet
            | Expression::Naturals
            | Expression::Naturals1
            | Expression::Integers
            | Expression::BoolType
            | Expression::SetEnumeration(_)
            | Expression::SetComprehension { .. }
            | Expression::SetBuilder { .. }
            | Expression::RelationalImage { .. }
            | Expression::FunctionApplication { .. }
            | Expression::BuiltinApplication { .. }
            | Expression::Bool(_)
            | Expression::Unary {
                op: UnaryOp::PowerSet
                    | UnaryOp::PowerSet1
                    | UnaryOp::Domain
                    | UnaryOp::Range
                    | UnaryOp::Inverse,
                ..
            }
    );
    if bare {
        expr(out, child);
    } else {
        out.push('(');
        expr(out, child);
        out.push(')');
    }
}

/// Operand of a relational predicate (`∈`, `=`, …): expressions of any
/// binary precedence go bare (`x ↦ y+1∈dom(f)`); quantified expression
/// forms are parenthesized. Sees through a `⦂` ascription when deciding
/// (like the other operand handlers), so `(λ…⦂T)∈R` keeps its parens.
fn expr_comparison_operand(out: &mut String, child: &Expression) {
    match strip_oftype(child) {
        Expression::Lambda { .. }
        | Expression::QuantifiedUnion { .. }
        | Expression::QuantifiedInter { .. } => {
            out.push('(');
            expr(out, child);
            out.push(')');
        }
        _ => expr(out, child),
    }
}

// ---------------------------------------------------------------------
// Shared bits
// ---------------------------------------------------------------------

fn args(out: &mut String, items: &[Expression]) {
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        expr(out, item);
    }
}

/// Bound-identifier declarations: names only, comma-tight — Rodin's
/// `toString` never prints declaration types.
fn decls(out: &mut String, identifiers: &[TypedIdentifier]) {
    for (i, ident) in identifiers.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&ident.name);
    }
}

fn ident_pattern(out: &mut String, pattern: &IdentPattern) {
    match pattern {
        IdentPattern::Identifier(t) => out.push_str(&t.name),
        IdentPattern::Maplet(l, r) => {
            ident_pattern(out, l);
            out.push_str(" ↦ ");
            ident_pattern(out, r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rossi::{parse_expression_str, parse_predicate_str};

    fn rp(src: &str) -> String {
        render_predicate(&parse_predicate_str(src).unwrap())
    }

    #[test]
    fn oftype_ascribed_lambda_operand_keeps_parens() {
        // `expr_comparison_operand` must see through `⦂` like the other
        // operand handlers: an OfType-ascribed lambda compared with `∈`
        // keeps the parentheses a bare lambda gets, else the relation is
        // swallowed into the lambda body (`λx·x>0 ∣ x∈R`). The `⦂` folds
        // away in rendering, so both forms are identical.
        assert_eq!(rp("(λx·x>0∣x)⦂ℙ(ℤ×ℤ)∈R"), "(λx·x>0 ∣ x)∈R");
        assert_eq!(rp("(λx·x>0∣x)∈R"), "(λx·x>0 ∣ x)∈R");
    }

    #[test]
    fn relational_and_logical_are_tight() {
        assert_eq!(rp("x ∈ S ∧ y = 3 ⇒ z ≠ 0"), "x∈S∧y=3⇒z≠0");
    }

    #[test]
    fn binary_expression_ops_are_spaced() {
        assert_eq!(rp("f ∈ S ∖ T ⇸ U × V"), "f∈S ∖ T ⇸ U × V");
        assert_eq!(rp("a − 1 ∈ 1 ‥ b"), "a − 1∈1 ‥ b");
    }

    #[test]
    fn associative_expression_ops_are_tight() {
        assert_eq!(rp("a + b ∗ c ∈ S ∪ T"), "a+b∗c∈S∪T");
    }

    #[test]
    fn maplet_is_spaced_inside_tight_relation() {
        assert_eq!(rp("x ↦ y ∈ dom(f)"), "x ↦ y∈dom(f)");
    }

    #[test]
    fn implication_in_conjunction_is_parenthesized() {
        let p = Predicate::logical(
            LogicalOp::And,
            parse_predicate_str("a ∈ S ⇒ b ∈ S").unwrap(),
            parse_predicate_str("c ∈ S").unwrap(),
        );
        assert_eq!(render_predicate(&p), "(a∈S⇒b∈S)∧c∈S");
    }

    #[test]
    fn conjunction_under_implication_is_bare() {
        assert_eq!(rp("a ∈ S ∧ b ∈ S ⇒ c ∈ S"), "a∈S∧b∈S⇒c∈S");
    }

    #[test]
    fn quantifier_is_parenthesized_as_operand() {
        let p = Predicate::logical(
            LogicalOp::Implies,
            parse_predicate_str("s ≠ ∅").unwrap(),
            parse_predicate_str("∀x · x ∈ s").unwrap(),
        );
        assert_eq!(render_predicate(&p), "s≠∅⇒(∀x·x∈s)");
        let q = Predicate::logical(
            LogicalOp::And,
            parse_predicate_str("s ≠ ∅").unwrap(),
            parse_predicate_str("∃b · ∀x · x ∈ s ⇒ b ≥ x").unwrap(),
        );
        assert_eq!(render_predicate(&q), "s≠∅∧(∃b·∀x·x∈s⇒b≥x)");
    }

    #[test]
    fn quantifier_body_extends_right_without_parens() {
        assert_eq!(rp("∀e · e ∈ dom(f) ⇒ f ∈ S ⇸ T"), "∀e·e∈dom(f)⇒f∈S ⇸ T");
    }

    #[test]
    fn set_extension_commas_are_tight() {
        assert_eq!(rp("{TRUE ↦ a, FALSE ↦ b} ≠ ∅"), "{TRUE ↦ a,FALSE ↦ b}≠∅");
    }

    #[test]
    fn comprehension_bar_is_spaced() {
        let e = parse_expression_str("{y · y ∈ s ∣ y}").unwrap();
        assert_eq!(render_expression(&e), "{y·y∈s ∣ y}");
    }

    #[test]
    fn inverse_is_tight_postfix() {
        assert_eq!(rp("r∼[{c}] ⊆ S"), "r∼[{c}]⊆S");
    }

    #[test]
    fn type_expression_needs_no_parens() {
        // partial(f) for f : ℤ×ℤ ⇸ ℤ — × binds tighter than ⇸.
        assert_eq!(rp("g ∈ ℤ × ℤ ⇸ ℤ"), "g∈ℤ × ℤ ⇸ ℤ");
    }

    fn re(src: &str) -> String {
        render_expression(&parse_expression_str(src).unwrap())
    }

    #[test]
    fn unary_minus_parenthesized_under_tighter_operator() {
        // Unary minus binds at additive precedence; multiplicative,
        // division and exponent are tighter, so the minus needs parens
        // on either side. Golden strings from eventb-checker.
        assert_eq!(re("(−a) ∗ b"), "(−a)∗b");
        assert_eq!(re("b ∗ (−a)"), "b∗(−a)");
        assert_eq!(re("(−a) ÷ b"), "(−a) ÷ b");
        assert_eq!(re("(−a) ^ b"), "(−a) ^ b");
        assert_eq!(re("(−a) mod b"), "(−a) mod b");
        assert_eq!(re("(−a) ∗ (−b)"), "(−a)∗(−b)");
    }

    #[test]
    fn unary_minus_bare_at_additive_level() {
        // Same precedence as +/−, left-associative: the left operand is
        // bare, the right operand is parenthesized. Golden strings from
        // eventb-checker.
        assert_eq!(re("(−a) + b"), "−a+b");
        assert_eq!(re("(−a) − b"), "−a − b");
        assert_eq!(re("b + (−a)"), "b+(−a)");
        assert_eq!(re("b − (−a)"), "b − (−a)");
    }

    #[test]
    fn unary_minus_operand_follows_its_own_precedence() {
        // The operand of `−` is bare when it binds tighter (`−a∗b`) and
        // parenthesized otherwise (`−(a+b)`, `−(−a)`). Golden strings
        // from eventb-checker.
        assert_eq!(re("−(a ∗ b)"), "−a∗b");
        assert_eq!(re("−(a + b)"), "−(a+b)");
        assert_eq!(re("−(−a)"), "−(−a)");
        assert_eq!(re("−(a)"), "−a");
    }

    #[test]
    fn oftype_ascription_is_folded_away() {
        // Rodin folds `⦂T` into the atomic's type; toString omits it.
        assert_eq!(re("∅ ⦂ ℙ(S)"), "∅");
        assert_eq!(rp("∅ ⦂ ℙ(S) = ∅ ⦂ ℙ(S)"), "∅=∅");
        assert_eq!(rp("(∅ ⦂ ℙ(S)) ∪ s = s"), "∅∪s=s");
    }
}
