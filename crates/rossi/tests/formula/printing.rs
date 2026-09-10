//! Rendering formula-model trees: spacing, parenthesization, binder
//! name resolution and freshening.

use rossi::PrettyPrinter;
use rossi::formula::tag::{
    AssocExprOp, AssocPredOp, AtomicOp, BinaryExprOp, BinaryPredOp, QuantExprOp, RelationalOp,
    UnaryExprOp,
};
use rossi::formula::{Expression, Form, Predicate, Type};

use crate::common::{bid, decl, decl_ty, eq_pred, ff, fid, forall, int};

fn readable() -> PrettyPrinter {
    PrettyPrinter::new()
}

fn canonical() -> PrettyPrinter {
    PrettyPrinter::rodin_canonical()
}

fn formula_string() -> PrettyPrinter {
    PrettyPrinter::rodin_formula_string()
}

fn plus(children: Vec<Expression>) -> Expression {
    ff().associative_expression(AssocExprOp::Plus, children, None)
}

// --- basic spacing ---

#[test]
fn readable_and_canonical_spacing() {
    let sum = plus(vec![fid("a"), fid("b"), int(3)]);
    assert_eq!(readable().print_formula_expression(&sum), "a + b + 3");
    assert_eq!(canonical().print_formula_expression(&sum), "a+b+3");

    let pred = eq_pred(fid("x"), int(1));
    assert_eq!(readable().print_formula_predicate(&pred), "x = 1");
    assert_eq!(canonical().print_formula_predicate(&pred), "x=1");

    let ascii = PrettyPrinter::ascii();
    let subset = ff().relational_predicate(RelationalOp::SubsetEq, fid("s"), fid("t"), None);
    // The model's SubsetEq is the legacy inclusive subset.
    assert_eq!(readable().print_formula_predicate(&subset), "s ⊆ t");
    assert_eq!(ascii.print_formula_predicate(&subset), "s <: t");
}

#[test]
fn nested_same_associative_operator_keeps_its_parentheses() {
    // A parse flattens `(a ∪ b) ∪ c`, so a nested child of the same
    // operator only ever comes from construction (a substitution); Rodin
    // parenthesizes it on either side, and so does every mode here.
    let union = |children| ff().associative_expression(AssocExprOp::BUnion, children, None);
    let left = union(vec![union(vec![fid("a"), fid("b")]), fid("c")]);
    assert_eq!(readable().print_formula_expression(&left), "(a ∪ b) ∪ c");
    assert_eq!(canonical().print_formula_expression(&left), "(a∪b)∪c");
    let right = union(vec![fid("a"), union(vec![fid("b"), fid("c")])]);
    assert_eq!(canonical().print_formula_expression(&right), "a∪(b∪c)");
}

#[test]
fn canonical_spaces_binary_and_tightens_associative_operators() {
    // Rodin prints its associative operators tight and every binary
    // expression operator spaced, the Cartesian product included: `×` is
    // tight only inside a type. Both Rodin modes share the rule.
    let product = ff().binary_expression(BinaryExprOp::CProd, fid("A"), fid("B"), None);
    assert_eq!(canonical().print_formula_expression(&product), "A × B");
    assert_eq!(formula_string().print_formula_expression(&product), "A × B");
    let composed = ff().associative_expression(AssocExprOp::FComp, vec![fid("p"), fid("q")], None);
    assert_eq!(canonical().print_formula_expression(&composed), "p;q");
    let union = ff().associative_expression(AssocExprOp::BUnion, vec![fid("s"), fid("t")], None);
    assert_eq!(canonical().print_formula_expression(&union), "s∪t");

    // An ascribed operand is parenthesized under an operator, as Rodin's
    // `S∪(∅ ⦂ ℙ(T))`, and left bare as an argument.
    let empty = ff().ascription(
        ff().atomic_expression(AtomicOp::EmptySet, None, None),
        ff().unary_expression(UnaryExprOp::Pow, fid("T"), None),
        None,
    );
    let joined =
        ff().associative_expression(AssocExprOp::BUnion, vec![fid("S"), empty.clone()], None);
    assert_eq!(
        canonical().print_formula_expression(&joined),
        "S∪(∅ ⦂ ℙ(T))"
    );
    let applied = ff().binary_expression(BinaryExprOp::FunImage, fid("f"), empty.clone(), None);
    assert_eq!(
        canonical().print_formula_expression(&applied),
        "f(∅ ⦂ ℙ(T))"
    );
    // Under a binary operator too: Rodin's checked files read
    // `{x ↦ (∅ ⦂ ℙ(T))}`.
    let pair = ff().binary_expression(BinaryExprOp::Mapsto, fid("x"), empty, None);
    assert_eq!(
        canonical().print_formula_expression(&pair),
        "x ↦ (∅ ⦂ ℙ(T))"
    );

    // After the `⦂` comes a type, and Rodin spells a type with a tight
    // product: `ℙ(A×B)` there, `A × B` as an expression.
    let typed_empty = ff().ascription(
        ff().atomic_expression(AtomicOp::EmptySet, None, None),
        ff().unary_expression(UnaryExprOp::Pow, product.clone(), None),
        None,
    );
    assert_eq!(
        canonical().print_formula_expression(&typed_empty),
        "∅ ⦂ ℙ(A×B)"
    );
}

#[test]
fn rodin_formula_string_matches_diagnostic_parenthesization_and_spacing() {
    let relation = ff().binary_expression(
        BinaryExprOp::RanRes,
        ff().binary_expression(BinaryExprOp::DomRes, fid("s"), fid("r"), None),
        fid("t"),
        None,
    );
    assert_eq!(
        formula_string().print_formula_expression(&relation),
        "s ◁ r ▷ t"
    );

    let application = ff().binary_expression(BinaryExprOp::FunImage, fid("f"), fid("x"), None);
    let converse = ff().unary_expression(UnaryExprOp::Converse, application, None);
    assert_eq!(
        formula_string().print_formula_expression(&converse),
        "(f(x))∼"
    );

    let negated = ff().not_predicate(eq_pred(fid("x"), int(-1)), None);
    assert_eq!(formula_string().print_formula_predicate(&negated), "¬x=−1");

    let ascribed_sum = ff().ascription(
        plus(vec![fid("a"), fid("b")]),
        ff().atomic_expression(AtomicOp::Integer, None, None),
        None,
    );
    let product = ff().associative_expression(AssocExprOp::Mul, vec![ascribed_sum, fid("c")], None);
    assert_eq!(
        formula_string().print_formula_expression(&product),
        "(a+b)∗c"
    );

    let annotated = ff().bound_ident_decl(
        "x",
        None,
        Some(ff().atomic_expression(AtomicOp::Integer, None, None)),
        Some(Type::Int),
    );
    let quantified = forall(vec![annotated], eq_pred(bid(0), int(1)));
    assert_eq!(
        formula_string().print_formula_predicate(&quantified),
        "∀x·x=1"
    );
}

#[test]
fn precedence_parenthesization_matches_the_tables() {
    // a − (b − c): same precedence, right child parenthesized.
    let nested = ff().binary_expression(
        BinaryExprOp::Minus,
        fid("a"),
        ff().binary_expression(BinaryExprOp::Minus, fid("b"), fid("c"), None),
        None,
    );
    assert_eq!(readable().print_formula_expression(&nested), "a − (b − c)");

    // (a + b) ∗ c: lower-precedence child parenthesized; the
    // associative sum behaves as its binary equivalent.
    let product = ff().associative_expression(
        AssocExprOp::Mul,
        vec![plus(vec![fid("a"), fid("b")]), fid("c")],
        None,
    );
    assert_eq!(readable().print_formula_expression(&product), "(a + b) ∗ c");
}

#[test]
fn applications_images_and_unaries_print_structurally() {
    let app = ff().binary_expression(BinaryExprOp::FunImage, fid("f"), fid("x"), None);
    assert_eq!(readable().print_formula_expression(&app), "f(x)");

    let image = ff().binary_expression(
        BinaryExprOp::RelImage,
        ff().unary_expression(UnaryExprOp::Converse, fid("f"), None),
        fid("s"),
        None,
    );
    assert_eq!(readable().print_formula_expression(&image), "f∼[s]");

    let union_head = ff().binary_expression(
        BinaryExprOp::RelImage,
        ff().associative_expression(AssocExprOp::BUnion, vec![fid("f"), fid("g")], None),
        fid("s"),
        None,
    );
    assert_eq!(
        readable().print_formula_expression(&union_head),
        "(f ∪ g)[s]"
    );

    let negated = ff().unary_expression(UnaryExprOp::UnMinus, fid("x"), None);
    assert_eq!(readable().print_formula_expression(&negated), "−(x)");

    // A negative integer literal prints bare, so under
    // `∗ ÷ mod ^` it needs the same parentheses as a unary-minus node:
    // unparenthesized, `−2^2` would re-parse as `−(2^2)`. Stored
    // output writes
    // `(−2)^2`. An additive context keeps the bare literal.
    let pow = ff().binary_expression(BinaryExprOp::Expn, int(-2), int(2), None);
    assert_eq!(formula_string().print_formula_expression(&pow), "(−2) ^ 2");
    let prod = ff().associative_expression(AssocExprOp::Mul, vec![int(-2), fid("x")], None);
    assert_eq!(formula_string().print_formula_expression(&prod), "(−2)∗x");
    let shifted = plus(vec![int(-2), fid("x")]);
    assert_eq!(formula_string().print_formula_expression(&shifted), "−2+x");

    let card = ff().unary_expression(UnaryExprOp::KCard, fid("s"), None);
    assert_eq!(readable().print_formula_expression(&card), "card(s)");

    let inverse_of_union = ff().unary_expression(
        UnaryExprOp::Converse,
        ff().associative_expression(AssocExprOp::BUnion, vec![fid("f"), fid("g")], None),
        None,
    );
    assert_eq!(
        readable().print_formula_expression(&inverse_of_union),
        "(f ∪ g)∼"
    );
}

// --- binders ---

#[test]
fn quantifiers_resolve_declaration_hints() {
    let pred = forall(vec![decl("x"), decl("y")], eq_pred(bid(1), bid(0)));
    assert_eq!(readable().print_formula_predicate(&pred), "∀x, y·x = y");
    assert_eq!(canonical().print_formula_predicate(&pred), "∀x,y·x=y");
}

#[test]
fn colliding_hints_are_freshened() {
    // ∀x · x = x_free — the declaration hint collides with the free
    // identifier and moves aside.
    let pred = forall(vec![decl("x")], eq_pred(bid(0), fid("x")));
    assert_eq!(readable().print_formula_predicate(&pred), "∀x0·x0 = x");
}

#[test]
fn shadowing_reuses_the_hint() {
    // ∀x · x = 1 ∧ (∀x · x = 2): the inner body never references the
    // outer declaration, so both print as x.
    let inner = forall(vec![decl("x")], eq_pred(bid(0), int(2)));
    let outer = forall(
        vec![decl("x")],
        ff().associative_predicate(
            AssocPredOp::LAnd,
            vec![eq_pred(bid(0), int(1)), inner],
            None,
        ),
    );
    assert_eq!(
        readable().print_formula_predicate(&outer),
        "∀x·x = 1 ∧ (∀x·x = 2)"
    );

    // But an inner reference to the outer declaration forces a fresh
    // inner name.
    let capturing = forall(
        vec![decl("x")],
        forall(vec![decl("x")], eq_pred(bid(0), bid(1))),
    );
    assert_eq!(
        readable().print_formula_predicate(&capturing),
        "∀x·∀x0·x0 = x"
    );
}

#[test]
fn declaration_annotations_and_typed_mode() {
    let annotated = ff().bound_ident_decl(
        "x",
        None,
        Some(ff().atomic_expression(AtomicOp::Integer, None, None)),
        None,
    );
    let pred = forall(vec![annotated], eq_pred(bid(0), int(1)));
    assert_eq!(readable().print_formula_predicate(&pred), "∀x⦂ℤ·x = 1");

    // Typed mode spells the solved type even without an annotation.
    let typed = forall(
        vec![decl_ty("x", Type::pow(Type::given("S")))],
        eq_pred(bid(0), fid("t")),
    );
    let printer = canonical().with_typed_decls(true);
    assert_eq!(printer.print_formula_predicate(&typed), "∀x⦂ℙ(S)·x=t");
    // Without the flag the declaration prints bare.
    assert_eq!(canonical().print_formula_predicate(&typed), "∀x·x=t");
}

// --- comprehension forms ---

#[test]
fn comprehension_forms_print_as_spelled() {
    let one_decl = || vec![decl("x")];
    let body = || eq_pred(bid(0), int(1));

    let explicit = ff().quantified_expression(
        QuantExprOp::CSet,
        one_decl(),
        body(),
        bid(0),
        None,
        Form::Explicit,
    );
    assert_eq!(
        readable().print_formula_expression(&explicit),
        "{x·x = 1∣x}"
    );

    let ident_list = ff().quantified_expression(
        QuantExprOp::CSet,
        one_decl(),
        body(),
        bid(0),
        None,
        Form::IdentList,
    );
    assert_eq!(
        readable().print_formula_expression(&ident_list),
        "{x∣x = 1}"
    );

    let implicit = ff().quantified_expression(
        QuantExprOp::CSet,
        one_decl(),
        body(),
        plus(vec![bid(0), int(1)]),
        None,
        Form::Implicit,
    );
    assert_eq!(
        readable().print_formula_expression(&implicit),
        "{x + 1∣x = 1}"
    );

    let union = ff().quantified_expression(
        QuantExprOp::QUnion,
        one_decl(),
        body(),
        ff().set_extension(vec![bid(0)], None),
        None,
        Form::Explicit,
    );
    assert_eq!(readable().print_formula_expression(&union), "⋃ x·x = 1∣{x}");
}

#[test]
fn typed_mode_escalates_short_comprehensions_to_explicit() {
    let one_decl = || vec![decl_ty("x", Type::Int)];
    let body = || eq_pred(bid(0), int(1));
    let printer = canonical().with_typed_decls(true);

    // {x∣x=1} has no spot for the declaration's type: typed printing
    // switches to the explicit spelling.
    let ident_list = ff().quantified_expression(
        QuantExprOp::CSet,
        one_decl(),
        body(),
        bid(0),
        None,
        Form::IdentList,
    );
    assert_eq!(
        printer.print_formula_expression(&ident_list),
        "{x⦂ℤ·x=1 ∣ x}"
    );

    // Same for {E∣P}.
    let implicit = ff().quantified_expression(
        QuantExprOp::CSet,
        one_decl(),
        body(),
        plus(vec![bid(0), int(1)]),
        None,
        Form::Implicit,
    );
    assert_eq!(
        printer.print_formula_expression(&implicit),
        "{x⦂ℤ·x=1 ∣ x+1}"
    );

    // The lambda spelling annotates its pattern leaves instead.
    let lambda = ff().quantified_expression(
        QuantExprOp::CSet,
        one_decl(),
        body(),
        ff().binary_expression(BinaryExprOp::Mapsto, bid(0), int(2), None),
        None,
        Form::Lambda,
    );
    assert_eq!(printer.print_formula_expression(&lambda), "λ x⦂ℤ·x=1 ∣ 2");
}

#[test]
fn lambda_patterns_render_from_declarations() {
    // λ x ↦ y · x = y ∣ x + y
    let pattern = ff().binary_expression(BinaryExprOp::Mapsto, bid(1), bid(0), None);
    let value = ff().binary_expression(
        BinaryExprOp::Mapsto,
        pattern,
        plus(vec![bid(1), bid(0)]),
        None,
    );
    let lambda = ff().quantified_expression(
        QuantExprOp::CSet,
        vec![decl("x"), decl("y")],
        eq_pred(bid(1), bid(0)),
        value,
        None,
        Form::Lambda,
    );
    assert_eq!(
        readable().print_formula_expression(&lambda),
        "λ x ↦ y·x = y∣x + y"
    );
}

// --- predicates and assignments ---

#[test]
fn logical_connectives_parenthesize_like_the_legacy_rules() {
    let a = || eq_pred(fid("a"), int(1));
    let b = || eq_pred(fid("b"), int(2));
    let c = || eq_pred(fid("c"), int(3));

    // a ∧ (b ∨ c): incompatible connectives parenthesize.
    let mixed = ff().associative_predicate(
        AssocPredOp::LAnd,
        vec![
            a(),
            ff().associative_predicate(AssocPredOp::LOr, vec![b(), c()], None),
        ],
        None,
    );
    assert_eq!(
        readable().print_formula_predicate(&mixed),
        "a = 1 ∧ (b = 2 ∨ c = 3)"
    );

    // Implication chains: right-nested needs parens.
    let imp = ff().binary_predicate(
        BinaryPredOp::LImp,
        a(),
        ff().binary_predicate(BinaryPredOp::LImp, b(), c(), None),
        None,
    );
    assert_eq!(
        readable().print_formula_predicate(&imp),
        "a = 1 ⇒ (b = 2 ⇒ c = 3)"
    );

    // Quantifiers inside connectives always parenthesize.
    let quantified = ff().associative_predicate(
        AssocPredOp::LAnd,
        vec![a(), forall(vec![decl("x")], eq_pred(bid(0), int(1)))],
        None,
    );
    assert_eq!(
        readable().print_formula_predicate(&quantified),
        "a = 1 ∧ (∀x·x = 1)"
    );
}

#[test]
fn assignments_print_their_forms() {
    let deterministic = ff().becomes_equal_to(
        vec![fid("x"), fid("y")],
        vec![int(1), plus(vec![fid("x"), int(2)])],
        None,
    );
    assert_eq!(
        readable().print_formula_assignment(&deterministic),
        "x, y ≔ 1, x + 2"
    );

    let membership = ff().becomes_member_of(vec![fid("x")], fid("s"), None);
    assert_eq!(readable().print_formula_assignment(&membership), "x :∈ s");

    let such_that = ff().becomes_such_that(
        vec![fid("x")],
        vec![decl("x'")],
        eq_pred(bid(0), plus(vec![fid("x"), int(1)])),
        None,
    );
    assert_eq!(
        readable().print_formula_assignment(&such_that),
        "x :∣ x' = x + 1"
    );
}

#[test]
fn literals_and_special_predicates() {
    let finite = ff().simple_predicate(fid("s"), None);
    assert_eq!(readable().print_formula_predicate(&finite), "finite(s)");

    let partition = ff().multiple_predicate(vec![fid("s"), fid("a"), fid("b")], None);
    assert_eq!(
        readable().print_formula_predicate(&partition),
        "partition(s, a, b)"
    );
    assert_eq!(
        canonical().print_formula_predicate(&partition),
        "partition(s,a,b)"
    );

    let application = ff().predicate_application("p", None, vec![fid("x")], None);
    assert_eq!(readable().print_formula_predicate(&application), "p(x)");

    let ascribed = ff().ascription(
        ff().atomic_expression(AtomicOp::EmptySet, None, None),
        ff().unary_expression(
            UnaryExprOp::Pow,
            ff().atomic_expression(AtomicOp::Integer, None, None),
            None,
        ),
        None,
    );
    // Rodin spaces an expression ascription (a bound declaration's is the
    // tight one) and parenthesizes it as an operand.
    let inside = eq_pred(fid("x"), ascribed.clone());
    assert_eq!(canonical().print_formula_predicate(&inside), "x=(∅ ⦂ ℙ(ℤ))");
    assert_eq!(readable().print_formula_expression(&ascribed), "∅ ⦂ ℙ(ℤ)");
}

#[test]
fn dangling_indices_render_visibly() {
    let stray: Predicate = eq_pred(bid(3), int(1));
    assert_eq!(readable().print_formula_predicate(&stray), "[[3]] = 1");
}
