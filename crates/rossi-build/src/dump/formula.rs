//! Typed formulas as dump nodes.
//!
//! One node shape carries every expression, predicate and declaration in the
//! model. A node always states its `tag` and `op`; the rest of its fields
//! appear only where they mean something, so a reader never has to tell a
//! meaningful null from an inapplicable one.
//!
//! Children are taken from the model's own positional accessor rather than
//! rebuilt per operator, so `children[i]` is the formula at child position
//! `i` by construction. That is what lets a consumer address a subformula in
//! the document and in Rodin with the same path.

use std::collections::BTreeMap;

use rossi::ast::Span;
use rossi::formula::position::FormulaRef;
use rossi::formula::{
    Assignment, AssignmentKind, BoundIdentDecl, Expression, ExpressionKind, Form, Predicate,
    PredicateKind, Type, fresh, tag,
};

use super::location::{LineIndex, SpanDump};
use super::opname;

/// A type as a tree, mirroring Rodin's type objects.
///
/// The document's type table maps each canonical type string to one of these.
/// Sub-types nest inline rather than referring back to the table by string,
/// so one lookup yields the whole structure and a consumer never has to walk
/// the table step by step to learn what a type is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(tag = "kind"))]
pub enum TypeNode {
    /// `ℤ`
    INT,
    /// `BOOL`
    BOOL,
    /// A carrier set.
    GIVEN { name: String },
    /// A power set.
    POW { base: Box<TypeNode> },
    /// A cartesian product.
    PROD {
        left: Box<TypeNode>,
        right: Box<TypeNode>,
    },
    /// A datatype instance. The extension's tag is deliberately absent: it is
    /// assigned per process and means nothing outside the one that wrote the
    /// document.
    PARAMETRIC {
        symbol: String,
        params: Vec<TypeNode>,
    },
}

impl TypeNode {
    fn of(ty: &Type) -> TypeNode {
        match ty {
            Type::Int => TypeNode::INT,
            Type::Bool => TypeNode::BOOL,
            Type::Given(name) => TypeNode::GIVEN { name: name.clone() },
            Type::Pow(base) => TypeNode::POW {
                base: Box::new(TypeNode::of(base)),
            },
            Type::Prod(left, right) => TypeNode::PROD {
                left: Box::new(TypeNode::of(left)),
                right: Box::new(TypeNode::of(right)),
            },
            Type::Parametric { symbol, params, .. } => TypeNode::PARAMETRIC {
                symbol: symbol.clone(),
                params: params.iter().map(TypeNode::of).collect(),
            },
        }
    }
}

/// An operator extension a document refers to.
///
/// `id` is the identity. An extension's tag is handed out per process, so two
/// documents from two runs can spell the same operator with different numbers
/// and the number must never be used to recognise one.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ExtensionInfo {
    pub id: String,
    pub symbol: String,
    pub group: String,
}

/// The reference an extended node carries to its operator.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ExtensionRef {
    pub id: String,
    pub symbol: String,
}

/// One expression, predicate or bound-identifier declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Node {
    /// Rodin's numeric tag for the operator.
    pub tag: u32,
    /// Rodin's constant name for the same operator.
    pub op: &'static str,
    /// The solved type, as a canonical Rodin string and a key into the
    /// document's type table. Expressions and declarations have one;
    /// predicates never do.
    #[cfg_attr(
        feature = "serde",
        serde(rename = "type", skip_serializing_if = "Option::is_none")
    )]
    pub ty: Option<String>,
    /// Where the node is written. Absent when the component was imported
    /// from Rodin XML, which carries no source text, and on the nodes a
    /// before-after predicate synthesizes.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub span: Option<SpanDump>,
    /// The identifier: a free identifier's name, a declaration's name, the
    /// name a bound occurrence resolves to, or a predicate variable's name.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub name: Option<String>,
    /// A bound occurrence's de Bruijn index, counting outward from 0 at the
    /// innermost binder. Within one declaration list, index 0 is the last
    /// declaration.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub index: Option<u32>,
    /// An integer literal, in decimal. Always a string: these are unbounded
    /// and would not survive a JSON number.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub value: Option<String>,
    /// How a comprehension or quantified union was written. Purely
    /// syntactic, and not recoverable from the tree.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub form: Option<&'static str>,
    /// The operator, when this node is an extension.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub extension: Option<ExtensionRef>,
    /// The children, in child-position order.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Vec::is_empty"))]
    pub children: Vec<Node>,
}

impl Node {
    fn leaf(tag: u32, op: &'static str) -> Node {
        Node {
            tag,
            op,
            ty: None,
            span: None,
            name: None,
            index: None,
            value: None,
            form: None,
            extension: None,
            children: Vec::new(),
        }
    }
}

/// One assignment.
///
/// Assignments have no child positions in the model, so instead of a numbered
/// list this names its parts the way Rodin's accessors do.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct AssignmentNode {
    pub tag: u32,
    pub op: &'static str,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub span: Option<SpanDump>,
    /// The assigned variables, always free identifiers.
    pub idents: Vec<Node>,
    /// `x ≔ E`: the assigned values, one per target.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub values: Option<Vec<Node>>,
    /// `x :∈ S`: the set chosen from.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub set: Option<Node>,
    /// `x :∣ P`: the after-state declarations the predicate binds. A primed
    /// occurrence inside `pred` is a bound identifier resolving to one of
    /// these, not a free identifier.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub primed: Option<Vec<Node>>,
    /// `x :∣ P`: the relation between before and after states.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub pred: Option<Node>,
}

/// What the converter accumulates across one document.
pub struct Ctx<'a> {
    /// The line table of the component being converted, when it has source
    /// text. `None` means the component came from Rodin XML.
    lines: Option<&'a LineIndex<'a>>,
    types: &'a mut BTreeMap<String, TypeNode>,
    extensions: &'a mut BTreeMap<String, ExtensionInfo>,
}

impl<'a> Ctx<'a> {
    pub fn new(
        lines: Option<&'a LineIndex<'a>>,
        types: &'a mut BTreeMap<String, TypeNode>,
        extensions: &'a mut BTreeMap<String, ExtensionInfo>,
    ) -> Self {
        Ctx {
            lines,
            types,
            extensions,
        }
    }

    /// The dump form of a span, if this component has text to resolve it
    /// against.
    ///
    /// The test is on the component, not on the span. Formulas parsed out of
    /// Rodin XML attributes do carry spans, but they index the attribute
    /// string, which the consumer never sees; emitting them would be worse
    /// than emitting nothing.
    fn span(&self, span: Option<Span>) -> Option<SpanDump> {
        match (self.lines, span) {
            (Some(lines), Some(span)) => Some(lines.span(span, None)),
            _ => None,
        }
    }

    /// Record a type and return its canonical string, which is its key in the
    /// document's type table.
    pub fn register_type(&mut self, ty: &Type) -> String {
        let key = ty.to_rodin_canonical();
        if !self.types.contains_key(&key) {
            self.types.insert(key.clone(), TypeNode::of(ty));
        }
        key
    }

    fn register_extension(&mut self, ext: &rossi::formula::Extension) -> ExtensionRef {
        let common = ext.common();
        let reference = ExtensionRef {
            id: common.id().to_string(),
            symbol: common.symbol().to_string(),
        };
        self.extensions
            .entry(reference.id.clone())
            .or_insert_with(|| ExtensionInfo {
                id: reference.id.clone(),
                symbol: reference.symbol.clone(),
                group: common.group_id().to_string(),
            });
        reference
    }
}

fn form_name(form: Form) -> &'static str {
    match form {
        Form::Explicit => "Explicit",
        Form::Implicit => "Implicit",
        Form::IdentList => "IdentList",
        Form::Lambda => "Lambda",
    }
}

/// The declarations a node binds over its body, or an empty slice.
///
/// Quantified expressions and predicates are the only binders in a formula.
/// The after-state declarations of a such-that assignment bind too, but an
/// assignment is not a `FormulaRef` and is handled separately.
fn binder_decls<'f>(fr: FormulaRef<'f>) -> &'f [BoundIdentDecl] {
    match fr {
        FormulaRef::Expr(e) => match e.kind() {
            ExpressionKind::Quantified { decls, .. } => decls,
            _ => &[],
        },
        FormulaRef::Pred(p) => match p.kind() {
            PredicateKind::Quantified { decls, .. } => decls,
            _ => &[],
        },
        FormulaRef::Decl(_) => &[],
    }
}

fn free_identifiers<'f>(fr: FormulaRef<'f>) -> &'f [String] {
    match fr {
        FormulaRef::Expr(e) => e.free_identifiers(),
        FormulaRef::Pred(p) => p.free_identifiers(),
        FormulaRef::Decl(d) => d.free_identifiers(),
    }
}

fn dangling_indices<'f>(fr: FormulaRef<'f>) -> &'f [u32] {
    match fr {
        FormulaRef::Expr(e) => e.dangling_bound_indices(),
        FormulaRef::Pred(p) => p.dangling_bound_indices(),
        FormulaRef::Decl(d) => d.dangling_bound_indices(),
    }
}

/// The name a bound occurrence resolves to, given the declarations in scope.
///
/// `scope` holds the resolved names of every enclosing declaration, innermost
/// last, so index 0 is the final entry. An index reaching past the scope is
/// dangling; such a formula is not well formed, and the node then reports its
/// index and no name rather than inventing one.
fn bound_name(scope: &[String], index: u32) -> Option<String> {
    scope
        .len()
        .checked_sub(1 + index as usize)
        .map(|i| scope[i].clone())
}

/// Convert the children of a node, pushing the binder's names onto the scope
/// for exactly the part of the child list that they bind.
///
/// A quantifier's declarations occupy the first child positions and are
/// themselves outside the binding; everything after them is inside it. That
/// split is the whole of scope handling in a formula.
fn children_of(ctx: &mut Ctx<'_>, fr: FormulaRef<'_>, scope: &mut Vec<String>) -> Vec<Node> {
    let decls = binder_decls(fr);
    let resolved = if decls.is_empty() {
        Vec::new()
    } else {
        fresh::resolve_binder_names(decls, free_identifiers(fr), dangling_indices(fr), scope)
    };

    let count = fr.child_count();
    let mut children = Vec::with_capacity(count);
    for index in 0..count {
        if !decls.is_empty() && index == decls.len() {
            scope.extend(resolved.iter().cloned());
        }
        let child = fr
            .child(index)
            .expect("child_count and child agree on the child range");
        children.push(match child {
            FormulaRef::Expr(e) => expression(ctx, e, scope),
            FormulaRef::Pred(p) => predicate(ctx, p, scope),
            FormulaRef::Decl(d) => declaration(
                ctx,
                d,
                resolved.get(index).map(String::as_str).unwrap_or(""),
            ),
        });
    }
    if !decls.is_empty() {
        scope.truncate(scope.len() - decls.len());
    }
    children
}

/// A bound-identifier declaration under the name it resolves to.
///
/// The declaration's own stored name is only a hint; `name` is what every
/// occurrence below it reports, so the two always agree.
fn declaration(ctx: &mut Ctx<'_>, decl: &BoundIdentDecl, name: &str) -> Node {
    Node {
        ty: decl.ty().map(|t| ctx.register_type(t)),
        span: ctx.span(decl.span()),
        name: Some(name.to_string()),
        ..Node::leaf(tag::BOUND_IDENT_DECL, "BOUND_IDENT_DECL")
    }
}

/// Convert one expression.
pub fn expression(ctx: &mut Ctx<'_>, expr: &Expression, scope: &mut Vec<String>) -> Node {
    let mut node = Node::leaf(expr.tag(), "");
    node.ty = expr.ty().map(|t| ctx.register_type(t));
    node.span = ctx.span(expr.span());

    node.op = match expr.kind() {
        ExpressionKind::FreeIdentifier(name) => {
            node.name = Some(name.clone());
            "FREE_IDENT"
        }
        ExpressionKind::BoundIdentifier(index) => {
            node.index = Some(*index);
            node.name = bound_name(scope, *index);
            "BOUND_IDENT"
        }
        ExpressionKind::IntegerLiteral(value) => {
            node.value = Some(value.to_string());
            "INTLIT"
        }
        ExpressionKind::SetExtension(_) => "SETEXT",
        ExpressionKind::Atomic(op) => opname::atomic(*op),
        ExpressionKind::Bool(_) => "KBOOL",
        ExpressionKind::Binary { op, .. } => opname::binary_expr(*op),
        ExpressionKind::Associative { op, .. } => opname::assoc_expr(*op),
        ExpressionKind::Unary { op, .. } => opname::unary_expr(*op),
        ExpressionKind::Quantified { op, form, .. } => {
            node.form = Some(form_name(*form));
            opname::quant_expr(*op)
        }
        // Ascriptions are unwrapped before conversion, so this arm cannot be
        // reached from a document. It exists because the model's kinds are
        // exhaustive, and it must not invent a node shape the format does not
        // define.
        ExpressionKind::Ascription { .. } => "OFTYPE",
        ExpressionKind::Extended { tag, .. } => {
            node.extension = expr
                .factory()
                .extension(*tag)
                .map(|ext| ctx.register_extension(ext));
            "EXTENDED"
        }
    };

    node.children = children_of(ctx, FormulaRef::Expr(expr), scope);
    node
}

/// Convert one predicate.
pub fn predicate(ctx: &mut Ctx<'_>, pred: &Predicate, scope: &mut Vec<String>) -> Node {
    let mut node = Node::leaf(pred.tag(), "");
    node.span = ctx.span(pred.span());

    node.op = match pred.kind() {
        PredicateKind::Literal(op) => opname::literal_pred(*op),
        PredicateKind::PredicateVariable(name) => {
            node.name = Some(name.clone());
            "PREDICATE_VARIABLE"
        }
        PredicateKind::Relational { op, .. } => opname::relational(*op),
        PredicateKind::Binary { op, .. } => opname::binary_pred(*op),
        PredicateKind::Associative { op, .. } => opname::assoc_pred(*op),
        PredicateKind::Not(_) => "NOT",
        PredicateKind::Quantified { op, .. } => opname::quant_pred(*op),
        PredicateKind::Simple(_) => "KFINITE",
        PredicateKind::Multiple(_) => "KPARTITION",
        // A predicate application never type-checks, so it cannot reach a
        // checked model. Should it ever arrive, the applied name is not a
        // child position and would otherwise be dropped silently.
        PredicateKind::Application { function, .. } => {
            node.name = Some(function.clone());
            "PRED_APPL"
        }
        PredicateKind::Extended { tag, .. } => {
            node.extension = pred
                .factory()
                .extension(*tag)
                .map(|ext| ctx.register_extension(ext));
            "EXTENDED"
        }
    };

    node.children = children_of(ctx, FormulaRef::Pred(pred), scope);
    node
}

fn expressions(ctx: &mut Ctx<'_>, exprs: &[Expression], scope: &mut Vec<String>) -> Vec<Node> {
    exprs.iter().map(|e| expression(ctx, e, scope)).collect()
}

/// Convert one assignment.
pub fn assignment(ctx: &mut Ctx<'_>, assign: &Assignment) -> AssignmentNode {
    let mut node = AssignmentNode {
        tag: assign.tag(),
        op: "",
        span: ctx.span(assign.span()),
        idents: Vec::new(),
        values: None,
        set: None,
        primed: None,
        pred: None,
    };
    // Assignment targets are free identifiers by construction, so they see no
    // binder and need no scope.
    let mut outer = Vec::new();

    match assign.kind() {
        AssignmentKind::BecomesEqualTo { idents, values } => {
            node.op = "BECOMES_EQUAL_TO";
            node.idents = expressions(ctx, idents, &mut outer);
            node.values = Some(expressions(ctx, values, &mut outer));
        }
        AssignmentKind::BecomesMemberOf { idents, set } => {
            node.op = "BECOMES_MEMBER_OF";
            node.idents = expressions(ctx, idents, &mut outer);
            node.set = Some(expression(ctx, set, &mut outer));
        }
        AssignmentKind::BecomesSuchThat {
            idents,
            primed,
            pred,
        } => {
            node.op = "BECOMES_SUCH_THAT";
            node.idents = expressions(ctx, idents, &mut outer);
            // The after-state declarations bind the predicate, and nothing
            // encloses an assignment, so they are resolved against the
            // assignment's own names in an empty outer scope.
            let resolved = fresh::resolve_binder_names(
                primed,
                assign.free_identifiers(),
                assign.dangling_bound_indices(),
                &[],
            );
            node.primed = Some(
                primed
                    .iter()
                    .zip(&resolved)
                    .map(|(decl, name)| declaration(ctx, decl, name))
                    .collect(),
            );
            let mut scope = resolved;
            node.pred = Some(predicate(ctx, pred, &mut scope));
        }
    }
    node
}

#[cfg(all(test, feature = "serde"))]
use tests as tests_support;

#[cfg(test)]
mod tests {
    use super::*;
    use rossi::formula::tag::{AssocPredOp, QuantPredOp, RelationalOp};
    use rossi::formula::{FormulaFactory, TypeEnvironmentBuilder};

    /// Convert the sole typed predicate of a one-invariant machine.
    pub(super) fn convert(source: &str) -> Node {
        let (node, _) = convert_with_types(source);
        node
    }

    fn convert_with_types(source: &str) -> (Node, BTreeMap<String, TypeNode>) {
        let typed = typecheck(source);
        let mut types = BTreeMap::new();
        let mut extensions = BTreeMap::new();
        let node = {
            let mut ctx = Ctx::new(None, &mut types, &mut extensions);
            predicate(&mut ctx, &typed, &mut Vec::new())
        };
        (node, types)
    }

    /// Parse and type-check a standalone predicate against an environment
    /// that gives every free name an integer-set type.
    pub(super) fn typecheck(source: &str) -> Predicate {
        let parsed = rossi::parse_predicate_str(source).expect("the predicate parses");
        let mut env = TypeEnvironmentBuilder::new();
        for name in parsed.free_identifiers() {
            let ty = if name.chars().next().is_some_and(char::is_uppercase) {
                Type::pow(Type::Int)
            } else {
                Type::Int
            };
            env.insert(name.clone(), ty);
        }
        let result = parsed.type_check(&env.into_snapshot());
        assert!(
            result.problems.is_empty(),
            "{source} did not type-check: {:?}",
            result.problems
        );
        result.typed.expect("a checked predicate")
    }

    fn child(node: &Node, path: &[usize]) -> Node {
        let mut current = node.clone();
        for index in path {
            current = current.children[*index].clone();
        }
        current
    }

    #[test]
    fn a_leaf_reports_its_rodin_tag_and_name() {
        let node = convert("x ∈ S");
        assert_eq!((node.tag, node.op), (107, "IN"));
        let left = child(&node, &[0]);
        assert_eq!((left.tag, left.op), (1, "FREE_IDENT"));
        assert_eq!(left.name.as_deref(), Some("x"));
        assert_eq!(left.ty.as_deref(), Some("ℤ"));
    }

    #[test]
    fn a_predicate_carries_no_type() {
        assert_eq!(convert("x ∈ S").ty, None);
    }

    #[test]
    fn children_follow_child_positions() {
        // `⇒` is child 0 then child 1, not a flattened list.
        let node = convert("x ∈ S ⇒ y ∈ S");
        assert_eq!(node.op, "LIMP");
        assert_eq!(node.children.len(), 2);
        assert_eq!(child(&node, &[0]).op, "IN");
        assert_eq!(child(&node, &[1]).op, "IN");
    }

    #[test]
    fn a_quantifier_puts_its_declarations_first() {
        let node = convert("∀ y · y ∈ S");
        assert_eq!(node.op, "FORALL");
        assert_eq!(node.children.len(), 2);
        let decl = child(&node, &[0]);
        assert_eq!((decl.tag, decl.op), (2, "BOUND_IDENT_DECL"));
        assert_eq!(decl.name.as_deref(), Some("y"));
        assert!(decl.children.is_empty(), "declarations are leaves");
    }

    #[test]
    fn a_bound_occurrence_names_its_declaration() {
        let occurrence = child(&convert("∀ y · y ∈ S"), &[1, 0]);
        assert_eq!((occurrence.tag, occurrence.op), (3, "BOUND_IDENT"));
        assert_eq!(occurrence.index, Some(0));
        assert_eq!(occurrence.name.as_deref(), Some("y"));
    }

    #[test]
    fn a_shadowing_binder_keeps_each_occurrence_on_its_own_declaration() {
        // Both binders spell their declaration `y`. Nothing inside the inner
        // one reaches the outer, so both keep the hint, and what separates
        // them is the index: the inner occurrence binds to the inner
        // declaration.
        let node = convert("∀ y · y ∈ S ∧ (∀ y · y ∈ S)");
        assert_eq!(child(&node, &[0]).name.as_deref(), Some("y"));
        let inner_decl = child(&node, &[1, 1, 0]);
        assert_eq!(inner_decl.name.as_deref(), Some("y"));

        let outer_use = child(&node, &[1, 0, 0]);
        let inner_use = child(&node, &[1, 1, 1, 0]);
        assert_eq!(
            (outer_use.index, outer_use.name.as_deref()),
            (Some(0), Some("y"))
        );
        assert_eq!(
            (inner_use.index, inner_use.name.as_deref()),
            (Some(0), Some("y"))
        );
    }

    #[test]
    fn a_declaration_that_would_capture_a_free_name_is_renamed() {
        // Surface syntax cannot write this: a binder named `y` would swallow
        // any `y` written below it. The checker's own rewrites can build it,
        // so the converter has to report a name that still reads as itself.
        // The declaration hint is `y` and a free `y` occurs in the body, so
        // the declaration must print as something else.
        let ff = FormulaFactory::default_factory();
        let decl = ff.bound_ident_decl("y", None, None, Some(Type::Int));
        let set = ff.free_identifier("S", None, Some(Type::pow(Type::Int)));
        let bound = ff.bound_identifier(0, None, Some(Type::Int));
        let free = ff.free_identifier("y", None, Some(Type::Int));
        let body = ff.associative_predicate(
            AssocPredOp::LAnd,
            vec![
                ff.relational_predicate(RelationalOp::In, bound, set.clone(), None),
                ff.relational_predicate(RelationalOp::In, free, set, None),
            ],
            None,
        );
        let quantified = ff.quantified_predicate(QuantPredOp::Forall, vec![decl], body, None);

        let mut types = BTreeMap::new();
        let mut extensions = BTreeMap::new();
        let node = {
            let mut ctx = Ctx::new(None, &mut types, &mut extensions);
            predicate(&mut ctx, &quantified, &mut Vec::new())
        };

        assert_eq!(child(&node, &[0]).name.as_deref(), Some("y0"));
        let bound_use = child(&node, &[1, 0, 0]);
        assert_eq!(
            (bound_use.op, bound_use.name.as_deref()),
            ("BOUND_IDENT", Some("y0"))
        );
        let free_use = child(&node, &[1, 1, 0]);
        assert_eq!(
            (free_use.op, free_use.name.as_deref()),
            ("FREE_IDENT", Some("y"))
        );
    }

    #[test]
    fn index_zero_is_the_last_declaration_of_a_list() {
        // Two declarations in one binder: the nearer one is the later one.
        let node = convert("∀ a, b · a ∈ S ∧ b ∈ S");
        assert_eq!(child(&node, &[0]).name.as_deref(), Some("a"));
        assert_eq!(child(&node, &[1]).name.as_deref(), Some("b"));
        let a = child(&node, &[2, 0, 0]);
        let b = child(&node, &[2, 1, 0]);
        assert_eq!((a.index, a.name.as_deref()), (Some(1), Some("a")));
        assert_eq!((b.index, b.name.as_deref()), (Some(0), Some("b")));
    }

    #[test]
    fn a_comprehension_records_how_it_was_written() {
        let node = convert("x ∈ {y · y ∈ S ∣ y}");
        let cset = child(&node, &[1]);
        assert_eq!((cset.tag, cset.op), (803, "CSET"));
        assert_eq!(cset.form, Some("Explicit"));
    }

    #[test]
    fn a_lambda_keeps_its_form() {
        let node = convert("x ∈ dom(λ y · y ∈ S ∣ y)");
        let cset = child(&node, &[1, 0]);
        assert_eq!(cset.op, "CSET");
        assert_eq!(cset.form, Some("Lambda"));
    }

    #[test]
    fn a_large_integer_literal_stays_a_string() {
        // Past the range a JSON number holds exactly, so writing it as a
        // number would round it silently.
        let big = i64::MAX.to_string();
        let node = convert(&format!("x = {big}"));
        let literal = child(&node, &[1]);
        assert_eq!((literal.tag, literal.op), (4, "INTLIT"));
        assert_eq!(literal.value.as_deref(), Some(big.as_str()));
    }

    #[test]
    fn an_integer_literal_beyond_the_parser_range_still_survives() {
        // The model stores an arbitrary-precision integer even though the
        // surface parser stops at 64 bits, so the field has to be a string
        // rather than merely wide.
        let ff = FormulaFactory::default_factory();
        let huge: num_bigint::BigInt = "123456789012345678901234567890"
            .parse()
            .expect("a big integer");
        let literal = ff.integer_literal(huge.clone(), None);

        let mut types = BTreeMap::new();
        let mut extensions = BTreeMap::new();
        let mut ctx = Ctx::new(None, &mut types, &mut extensions);
        let node = expression(&mut ctx, &literal, &mut Vec::new());

        assert_eq!(node.value.as_deref(), Some(huge.to_string().as_str()));
    }

    #[test]
    fn types_are_registered_as_nested_trees() {
        let (_, types) = convert_with_types("x ∈ S");
        assert_eq!(types.get("ℤ"), Some(&TypeNode::INT));
        assert_eq!(
            types.get("ℙ(ℤ)"),
            Some(&TypeNode::POW {
                base: Box::new(TypeNode::INT)
            })
        );
    }

    #[test]
    fn a_product_type_nests_both_sides() {
        let mut types = BTreeMap::new();
        let mut extensions = BTreeMap::new();
        let mut ctx = Ctx::new(None, &mut types, &mut extensions);
        let ty = Type::pow(Type::prod(Type::Int, Type::given("COLORS")));
        let key = ctx.register_type(&ty);

        assert_eq!(key, "ℙ(ℤ×COLORS)");
        assert_eq!(
            types.get(&key),
            Some(&TypeNode::POW {
                base: Box::new(TypeNode::PROD {
                    left: Box::new(TypeNode::INT),
                    right: Box::new(TypeNode::GIVEN {
                        name: "COLORS".to_string()
                    }),
                }),
            })
        );
    }

    #[test]
    fn a_registered_type_string_parses_back_to_the_same_type() {
        let mut types = BTreeMap::new();
        let mut extensions = BTreeMap::new();
        let mut ctx = Ctx::new(None, &mut types, &mut extensions);
        let ty = Type::pow(Type::prod(Type::Int, Type::Bool));
        let key = ctx.register_type(&ty);

        assert_eq!(Type::parse_rodin(&key), Some(ty));
    }

    #[test]
    fn a_component_without_source_text_emits_no_spans() {
        // `lines` is `None` here, standing in for a Rodin XML import: the
        // formula does carry spans, but they index a string the reader of
        // the document has never seen.
        let node = convert("x ∈ S");
        assert_eq!(node.span, None);
        assert_eq!(child(&node, &[0]).span, None);
    }

    /// Convert a factory-built assignment with no source text.
    fn convert_assignment(assign: &Assignment) -> AssignmentNode {
        let mut types = BTreeMap::new();
        let mut extensions = BTreeMap::new();
        let mut ctx = Ctx::new(None, &mut types, &mut extensions);
        assignment(&mut ctx, assign)
    }

    fn int_var(ff: &FormulaFactory, name: &str) -> Expression {
        ff.free_identifier(name, None, Some(Type::Int))
    }

    #[test]
    fn a_simple_assignment_names_its_targets_and_values() {
        let ff = FormulaFactory::default_factory();
        let assign = ff.becomes_equal_to(
            vec![int_var(&ff, "x"), int_var(&ff, "y")],
            vec![
                ff.integer_literal(1i64, None),
                ff.integer_literal(2i64, None),
            ],
            None,
        );
        let node = convert_assignment(&assign);

        assert_eq!((node.tag, node.op), (6, "BECOMES_EQUAL_TO"));
        let names: Vec<_> = node.idents.iter().map(|n| n.name.as_deref()).collect();
        assert_eq!(names, [Some("x"), Some("y")]);
        let values = node.values.expect("a simple assignment has values");
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].value.as_deref(), Some("1"));
        assert!(node.set.is_none() && node.primed.is_none() && node.pred.is_none());
    }

    #[test]
    fn a_choice_from_a_set_names_the_set() {
        let ff = FormulaFactory::default_factory();
        let assign = ff.becomes_member_of(
            vec![int_var(&ff, "x")],
            ff.atomic_expression(
                rossi::formula::tag::AtomicOp::Natural,
                None,
                Some(Type::pow(Type::Int)),
            ),
            None,
        );
        let node = convert_assignment(&assign);

        assert_eq!((node.tag, node.op), (7, "BECOMES_MEMBER_OF"));
        assert_eq!(node.set.expect("a set is named").op, "NATURAL");
        assert!(node.values.is_none());
    }

    #[test]
    fn an_after_state_occurrence_binds_to_the_primed_declaration() {
        // `x :∣ x' = 1`. The primed name is a declaration the predicate
        // binds, not a free identifier, so the occurrence inside reports an
        // index and resolves to that declaration.
        let ff = FormulaFactory::default_factory();
        let primed = ff.bound_ident_decl("x'", None, None, Some(Type::Int));
        let pred = ff.relational_predicate(
            RelationalOp::Equal,
            ff.bound_identifier(0, None, Some(Type::Int)),
            ff.integer_literal(1i64, None),
            None,
        );
        let assign = ff.becomes_such_that(vec![int_var(&ff, "x")], vec![primed], pred, None);
        let node = convert_assignment(&assign);

        assert_eq!((node.tag, node.op), (8, "BECOMES_SUCH_THAT"));
        let primed = node.primed.expect("an after-state declaration");
        assert_eq!(primed.len(), 1);
        assert_eq!(
            (primed[0].op, primed[0].name.as_deref()),
            ("BOUND_IDENT_DECL", Some("x'"))
        );

        let pred = node.pred.expect("a relating predicate");
        let occurrence = &pred.children[0];
        assert_eq!(occurrence.op, "BOUND_IDENT");
        assert_eq!(
            (occurrence.index, occurrence.name.as_deref()),
            (Some(0), Some("x'"))
        );
    }

    #[test]
    fn a_before_after_predicate_frees_the_after_state_names() {
        // The same assignment as a predicate over both states: what was a
        // bound declaration becomes an ordinary primed identifier, which is
        // what makes the layer usable on its own.
        let ff = FormulaFactory::default_factory();
        let primed = ff.bound_ident_decl("x'", None, None, Some(Type::Int));
        let pred = ff.relational_predicate(
            RelationalOp::Equal,
            ff.bound_identifier(0, None, Some(Type::Int)),
            ff.integer_literal(1i64, None),
            None,
        );
        let assign = ff.becomes_such_that(vec![int_var(&ff, "x")], vec![primed], pred, None);

        let ba = assign.ba_predicate();
        let mut types = BTreeMap::new();
        let mut extensions = BTreeMap::new();
        let mut ctx = Ctx::new(None, &mut types, &mut extensions);
        let node = predicate(&mut ctx, &ba, &mut Vec::new());

        assert_eq!(node.op, "EQUAL");
        let left = &node.children[0];
        assert_eq!((left.op, left.name.as_deref()), ("FREE_IDENT", Some("x'")));
        assert_eq!(left.index, None);
    }

    #[test]
    fn spans_are_emitted_against_the_component_text() {
        let source = "x ∈ S";
        let typed = typecheck(source);
        let lines = LineIndex::new(source);
        let mut types = BTreeMap::new();
        let mut extensions = BTreeMap::new();
        let mut ctx = Ctx::new(Some(&lines), &mut types, &mut extensions);
        let node = predicate(&mut ctx, &typed, &mut Vec::new());

        let left = child(&node, &[0]).span.expect("the left operand is placed");
        assert_eq!((left.start, left.end), (0, 1));
        assert_eq!((left.line, left.col), (1, 1));
        // `∈` is three bytes, so the right operand starts at byte 6 but at
        // character column 5.
        let right = child(&node, &[1])
            .span
            .expect("the right operand is placed");
        assert_eq!((right.start, right.line, right.col), (6, 1, 5));
    }
}

#[cfg(all(test, feature = "serde"))]
mod serde_tests {
    use super::tests_support::convert;
    use super::*;
    use rossi::formula::FormulaFactory;

    /// The JSON a node serializes to is the format's actual contract, so the
    /// key set and the omissions are pinned here rather than inferred from
    /// the struct.
    #[test]
    fn a_node_omits_every_field_that_does_not_apply() {
        let node = convert("x ∈ S");
        let json = serde_json::to_value(&node).expect("a node serializes");

        // A predicate: no type, and with no line index, no span either.
        assert_eq!(json["tag"], 107);
        assert_eq!(json["op"], "IN");
        assert!(json.get("type").is_none(), "a predicate has no type");
        assert!(json.get("span").is_none());
        assert!(json.get("name").is_none());
        assert!(json.get("index").is_none());
        assert!(json.get("value").is_none());
        assert!(json.get("form").is_none());
        assert!(json.get("extension").is_none());

        let left = &json["children"][0];
        assert_eq!(left["op"], "FREE_IDENT");
        assert_eq!(left["name"], "x");
        assert_eq!(left["type"], "ℤ");
        assert!(left.get("children").is_none(), "a leaf has no children key");
    }

    #[test]
    fn a_type_serializes_as_a_nested_tree_tagged_by_kind() {
        let ty = Type::pow(Type::prod(Type::Int, Type::given("COLORS")));
        let json = serde_json::to_value(TypeNode::of(&ty)).expect("a type serializes");

        assert_eq!(json["kind"], "POW");
        assert_eq!(json["base"]["kind"], "PROD");
        assert_eq!(json["base"]["left"]["kind"], "INT");
        assert_eq!(json["base"]["right"]["kind"], "GIVEN");
        assert_eq!(json["base"]["right"]["name"], "COLORS");
    }

    #[test]
    fn an_assignment_names_only_the_parts_its_operator_has() {
        let ff = FormulaFactory::default_factory();
        let assign = ff.becomes_equal_to(
            vec![ff.free_identifier("x", None, Some(Type::Int))],
            vec![ff.integer_literal(1i64, None)],
            None,
        );
        let mut types = BTreeMap::new();
        let mut extensions = BTreeMap::new();
        let mut ctx = Ctx::new(None, &mut types, &mut extensions);
        let node = assignment(&mut ctx, &assign);
        let json = serde_json::to_value(&node).expect("an assignment serializes");

        assert_eq!(json["op"], "BECOMES_EQUAL_TO");
        assert_eq!(json["idents"][0]["name"], "x");
        assert_eq!(json["values"][0]["value"], "1");
        assert!(json.get("set").is_none());
        assert!(json.get("primed").is_none());
        assert!(json.get("pred").is_none());
    }
}
