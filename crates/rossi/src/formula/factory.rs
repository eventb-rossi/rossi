//! The formula factory: the only way to construct formula nodes.
//!
//! Constructors validate the structural invariants the rest of the
//! layer relies on (child counts, binder arity, assigned-identifier
//! shape, print-form validity), compute the cached structural hash and
//! identifier caches bottom-up, and stamp every node with the factory
//! that built it. Violating a structural invariant is a programming
//! error and panics.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock, Mutex};

use num_bigint::BigInt;

use crate::ast::Span;

use super::assignment::{self, AssignData, Assignment, AssignmentKind};
use super::caches::CacheBuilder;
use super::decl::{self, BoundIdentDecl, DeclData};
use super::expression::{self, ExprData, Expression, ExpressionKind, Form};
use super::extension::{ExpressionExtension, Extension, PredicateExtension};
use super::predicate::{self, PredData, Predicate, PredicateKind};
use super::tag::{
    self, AssocExprOp, AssocPredOp, AtomicOp, BinaryExprOp, BinaryPredOp, LiteralPredOp,
    QuantExprOp, QuantPredOp, RelationalOp, Tag, UnaryExprOp,
};
use super::types::Type;

/// Builds formula nodes.
///
/// A factory is a cheap-to-clone handle carrying the set of operator
/// extensions it supports; factories with equal extension sets are
/// interned to the same instance, so factories compare by identity.
#[derive(Clone)]
pub struct FormulaFactory(pub(super) Arc<FactoryData>);

pub(super) struct FactoryData {
    /// This factory's extensions, by dynamic tag.
    extensions: BTreeMap<Tag, Extension>,
    /// Extension object identity → tag, for construction lookups.
    by_identity: HashMap<usize, Tag>,
    /// Extension symbol → tag and extension, for the parser's operator
    /// resolution (one lookup per spelling).
    by_symbol: HashMap<String, (Tag, Extension)>,
}

impl std::fmt::Debug for FactoryData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FactoryData")
            .field("extensions", &self.extensions.len())
            .finish()
    }
}

pub(super) static DEFAULT: LazyLock<FormulaFactory> = LazyLock::new(|| {
    FormulaFactory(Arc::new(FactoryData {
        extensions: BTreeMap::new(),
        by_identity: HashMap::new(),
        by_symbol: HashMap::new(),
    }))
});

/// The process-global extension registry: identity → permanent tag,
/// plus the interned factories per extension set.
struct Registry {
    tags: HashMap<usize, (Tag, Extension)>,
    next: Tag,
    factories: HashMap<Vec<Tag>, FormulaFactory>,
}

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(|| {
    Mutex::new(Registry {
        tags: HashMap::new(),
        next: tag::FIRST_EXTENSION_TAG,
        factories: HashMap::new(),
    })
});

impl Registry {
    /// The permanent tag of an extension object, allocated on first
    /// sight.
    fn tag_for(&mut self, ext: &Extension) -> Tag {
        let identity = ext.identity();
        match self.tags.get(&identity) {
            Some((tag, _)) => *tag,
            None => {
                let tag = self.next;
                self.next += 1;
                self.tags.insert(identity, (tag, ext.clone()));
                tag
            }
        }
    }
}

/// Allocates (or looks up) the permanent tag of an extension without
/// building a factory. Datatype construction uses this so a type
/// constructor knows its own tag before any factory exists.
pub(super) fn register_extension_tag(ext: &Extension) -> Tag {
    REGISTRY.lock().expect("registry lock").tag_for(ext)
}

/// A rejected extension set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionError {
    /// Two extensions in the set share a syntax symbol, or the symbol
    /// collides with core vocabulary.
    DuplicateSymbol(String),
    /// Two extensions in the set share an identifier.
    DuplicateId(String),
}

/// A rejected extended-node construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FactoryError {
    /// The extension is not part of this factory's set.
    UnknownExtension,
    /// The child counts do not fit the extension's kind.
    ArityMismatch,
    /// The explicit type was rejected by the extension.
    TypeMisfit,
}

impl FormulaFactory {
    /// The factory for the core mathematical language.
    pub fn default_factory() -> FormulaFactory {
        DEFAULT.clone()
    }

    /// The interned factory supporting the core language plus the
    /// given extensions. Each distinct extension object keeps one
    /// process-wide tag; an empty set yields the default factory.
    pub fn with_extensions(
        extensions: impl IntoIterator<Item = Extension>,
    ) -> Result<FormulaFactory, ExtensionError> {
        let extensions: Vec<Extension> = extensions.into_iter().collect();
        for (i, ext) in extensions.iter().enumerate() {
            let symbol = ext.common().symbol();
            let id = ext.common().id();
            if crate::builtins::is_reserved_name(symbol) {
                return Err(ExtensionError::DuplicateSymbol(symbol.to_string()));
            }
            for other in &extensions[i + 1..] {
                if other.identity() == ext.identity() {
                    continue;
                }
                if other.common().symbol() == symbol {
                    return Err(ExtensionError::DuplicateSymbol(symbol.to_string()));
                }
                if other.common().id() == id {
                    return Err(ExtensionError::DuplicateId(id.to_string()));
                }
            }
        }
        if extensions.is_empty() {
            return Ok(Self::default_factory());
        }
        let mut registry = REGISTRY.lock().expect("registry lock");
        let mut by_tag: BTreeMap<Tag, Extension> = BTreeMap::new();
        for ext in extensions {
            let tag = registry.tag_for(&ext);
            by_tag.insert(tag, ext);
        }
        let key: Vec<Tag> = by_tag.keys().copied().collect();
        if let Some(factory) = registry.factories.get(&key) {
            return Ok(factory.clone());
        }
        let by_identity = by_tag
            .iter()
            .map(|(tag, ext)| (ext.identity(), *tag))
            .collect();
        let by_symbol = by_tag
            .iter()
            .map(|(tag, ext)| (ext.common().symbol().to_string(), (*tag, ext.clone())))
            .collect();
        let factory = FormulaFactory(Arc::new(FactoryData {
            extensions: by_tag,
            by_identity,
            by_symbol,
        }));
        registry.factories.insert(key, factory.clone());
        Ok(factory)
    }

    /// The extension registered under `tag` in this factory, if any.
    pub fn extension(&self, tag: Tag) -> Option<&Extension> {
        self.0.extensions.get(&tag)
    }

    /// The extension whose operator symbol is `symbol`, with its tag. Symbols
    /// are unique within a factory ([`Self::with_extensions`] rejects a
    /// duplicate), so this is how the parser resolves an operator spelling.
    pub fn extension_by_symbol(&self, symbol: &str) -> Option<(Tag, &Extension)> {
        self.0.by_symbol.get(symbol).map(|(tag, ext)| (*tag, ext))
    }

    /// The extensions this factory supports, in tag order.
    pub fn extensions(&self) -> impl Iterator<Item = (Tag, &Extension)> {
        self.0.extensions.iter().map(|(tag, ext)| (*tag, ext))
    }

    fn tag_of_identity(&self, identity: usize) -> Result<Tag, FactoryError> {
        self.0
            .by_identity
            .get(&identity)
            .copied()
            .ok_or(FactoryError::UnknownExtension)
    }

    fn same_factory(&self, other: &FormulaFactory) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    #[track_caller]
    fn check_expr_children<'a>(&self, children: impl IntoIterator<Item = &'a Expression>) {
        for child in children {
            assert!(
                self.same_factory(child.factory()),
                "child expression was built with a different factory"
            );
        }
    }

    #[track_caller]
    fn check_pred_children<'a>(&self, children: impl IntoIterator<Item = &'a Predicate>) {
        for child in children {
            assert!(
                self.same_factory(child.factory()),
                "child predicate was built with a different factory"
            );
        }
    }

    #[track_caller]
    fn check_decls<'a>(&self, decls: impl IntoIterator<Item = &'a BoundIdentDecl>) {
        for decl in decls {
            assert!(
                self.same_factory(decl.factory()),
                "declaration was built with a different factory"
            );
        }
    }

    fn make_expr(
        &self,
        kind: ExpressionKind,
        ty: Option<Type>,
        span: Option<Span>,
        caches: CacheBuilder,
    ) -> Expression {
        // A node built from type-checked parts is type-checked by
        // construction: synthesize the type unless the caller provided
        // one (leaves and ascribed generic operators).
        let ty = ty.or_else(|| expression::synthesize_type(&kind));
        let (free_idents, dangling) = caches.finish();
        Expression(Arc::new(ExprData {
            hash: expression::kind_hash(&kind),
            kind,
            ty,
            span,
            free_idents,
            dangling,
            factory: self.clone(),
        }))
    }

    fn make_pred(
        &self,
        kind: PredicateKind,
        span: Option<Span>,
        caches: CacheBuilder,
    ) -> Predicate {
        let (free_idents, dangling) = caches.finish();
        Predicate(Arc::new(PredData {
            hash: predicate::kind_hash(&kind),
            typed: predicate::kind_typed(&kind),
            kind,
            span,
            free_idents,
            dangling,
            factory: self.clone(),
        }))
    }

    fn make_assign(
        &self,
        kind: AssignmentKind,
        span: Option<Span>,
        caches: CacheBuilder,
    ) -> Assignment {
        let (free_idents, dangling) = caches.finish();
        Assignment(Arc::new(AssignData {
            hash: assignment::kind_hash(&kind),
            typed: assignment::kind_typed(&kind),
            kind,
            span,
            free_idents,
            dangling,
            factory: self.clone(),
        }))
    }

    // ----- declarations -------------------------------------------------

    /// A bound-identifier declaration. The annotation is the source
    /// `⦂ T` spelling, if any; the solved type is normally stamped by
    /// the type-checker.
    pub fn bound_ident_decl(
        &self,
        name: impl Into<String>,
        span: Option<Span>,
        annotation: Option<Expression>,
        ty: Option<Type>,
    ) -> BoundIdentDecl {
        let name = name.into();
        assert!(!name.is_empty(), "declaration name must not be empty");
        self.check_expr_children(&annotation);
        let mut caches = CacheBuilder::new();
        if let Some(annotation) = &annotation {
            caches.add_expr(annotation);
        }
        let (mut free_idents, dangling) = caches.finish();
        if let Some(ty) = &ty {
            let mut givens = Vec::new();
            ty.collect_given_sets(&mut givens);
            if !givens.is_empty() {
                let mut merged = free_idents.into_vec();
                merged.append(&mut givens);
                merged.sort_unstable();
                merged.dedup();
                free_idents = merged.into();
            }
        }
        BoundIdentDecl(Arc::new(DeclData {
            hash: decl::decl_hash(&name),
            name,
            annotation,
            ty,
            span,
            free_idents,
            dangling,
            factory: self.clone(),
        }))
    }

    // ----- expressions --------------------------------------------------

    /// A free identifier occurrence, e.g. `x` or the primed `x'`.
    pub fn free_identifier(
        &self,
        name: impl Into<String>,
        span: Option<Span>,
        ty: Option<Type>,
    ) -> Expression {
        let name = name.into();
        assert!(!name.is_empty(), "identifier name must not be empty");
        let mut caches = CacheBuilder::new();
        caches.add_free_name(name.clone());
        self.make_expr(ExpressionKind::FreeIdentifier(name), ty, span, caches)
    }

    /// A bound identifier occurrence; index 0 refers to the innermost
    /// enclosing declaration.
    pub fn bound_identifier(&self, index: u32, span: Option<Span>, ty: Option<Type>) -> Expression {
        let mut caches = CacheBuilder::new();
        caches.add_dangling_index(index);
        self.make_expr(ExpressionKind::BoundIdentifier(index), ty, span, caches)
    }

    /// An integer literal.
    pub fn integer_literal(&self, value: impl Into<BigInt>, span: Option<Span>) -> Expression {
        self.make_expr(
            ExpressionKind::IntegerLiteral(value.into()),
            None,
            span,
            CacheBuilder::new(),
        )
    }

    /// A nullary operator, e.g. `ℤ`, `∅`, `TRUE`, `succ`.
    ///
    /// Closed operators are typed by construction; the generic ones
    /// (`∅`, `id`, `prj1`, `prj2`) stay untyped unless a type is given.
    /// An explicit type must fit the operator.
    #[track_caller]
    pub fn atomic_expression(
        &self,
        op: AtomicOp,
        span: Option<Span>,
        ty: Option<Type>,
    ) -> Expression {
        if let Some(ty) = &ty {
            assert!(
                expression::verify_atomic_type(op, ty),
                "explicit type does not fit the operator"
            );
        }
        self.make_expr(ExpressionKind::Atomic(op), ty, span, CacheBuilder::new())
    }

    /// A set defined in extension, e.g. `{a, b, c}`. Never empty: a
    /// typed empty set is the `∅` atom.
    #[track_caller]
    pub fn set_extension(&self, members: Vec<Expression>, span: Option<Span>) -> Expression {
        assert!(
            !members.is_empty(),
            "a set extension needs at least one member"
        );
        self.check_expr_children(&members);
        let mut caches = CacheBuilder::new();
        for member in &members {
            caches.add_expr(member);
        }
        self.make_expr(ExpressionKind::SetExtension(members), None, span, caches)
    }

    /// `bool(P)`.
    pub fn bool_expression(&self, pred: Predicate, span: Option<Span>) -> Expression {
        self.check_pred_children([&pred]);
        let mut caches = CacheBuilder::new();
        caches.add_pred(&pred);
        self.make_expr(ExpressionKind::Bool(pred), None, span, caches)
    }

    /// A binary expression, e.g. `x ↦ y`, `a − b`, `f(x)`.
    pub fn binary_expression(
        &self,
        op: BinaryExprOp,
        left: Expression,
        right: Expression,
        span: Option<Span>,
    ) -> Expression {
        self.check_expr_children([&left, &right]);
        let mut caches = CacheBuilder::new();
        caches.add_expr(&left);
        caches.add_expr(&right);
        self.make_expr(
            ExpressionKind::Binary { op, left, right },
            None,
            span,
            caches,
        )
    }

    /// An associative expression with at least two children, e.g.
    /// `a + b + c`.
    #[track_caller]
    pub fn associative_expression(
        &self,
        op: AssocExprOp,
        children: Vec<Expression>,
        span: Option<Span>,
    ) -> Expression {
        assert!(
            children.len() >= 2,
            "an associative expression needs at least two children"
        );
        self.check_expr_children(&children);
        let mut caches = CacheBuilder::new();
        for child in &children {
            caches.add_expr(child);
        }
        self.make_expr(
            ExpressionKind::Associative { op, children },
            None,
            span,
            caches,
        )
    }

    /// A unary expression, e.g. `card(S)`, `−x`.
    pub fn unary_expression(
        &self,
        op: UnaryExprOp,
        child: Expression,
        span: Option<Span>,
    ) -> Expression {
        self.check_expr_children([&child]);
        let mut caches = CacheBuilder::new();
        caches.add_expr(&child);
        self.make_expr(ExpressionKind::Unary { op, child }, None, span, caches)
    }

    /// A quantified expression (comprehension set, quantified union or
    /// intersection). The requested print form is validated against the
    /// actual expression and downgraded if it does not hold.
    #[track_caller]
    pub fn quantified_expression(
        &self,
        op: QuantExprOp,
        decls: Vec<BoundIdentDecl>,
        pred: Predicate,
        expr: Expression,
        span: Option<Span>,
        form: Form,
    ) -> Expression {
        assert!(
            !decls.is_empty(),
            "a quantified expression needs at least one declaration"
        );
        self.check_decls(&decls);
        self.check_pred_children([&pred]);
        self.check_expr_children([&expr]);
        let form = expression::filter_form(form, op, decls.len(), &expr);
        let mut caches = CacheBuilder::new();
        for decl in &decls {
            caches.add_decl(decl);
        }
        caches.add_scoped_pred(&pred, decls.len());
        caches.add_scoped_expr(&expr, decls.len());
        self.make_expr(
            ExpressionKind::Quantified {
                op,
                decls,
                pred,
                expr,
                form,
            },
            None,
            span,
            caches,
        )
    }

    /// The canonical left-nested maplet chain over the `count` innermost
    /// bound identifiers, in declaration order:
    /// `bid(count−1) ↦ … ↦ bid(0)`. This is the value shape the
    /// ident-list comprehension form and the lambda pattern require
    /// (see `Form` filtering).
    #[track_caller]
    pub fn bound_ident_chain(&self, count: usize) -> Expression {
        assert!(count > 0, "a chain needs at least one identifier");
        let mut chain = self.bound_identifier(count as u32 - 1, None, None);
        for index in (0..count - 1).rev() {
            let right = self.bound_identifier(index as u32, None, None);
            chain = self.binary_expression(BinaryExprOp::Mapsto, chain, right, None);
        }
        chain
    }

    /// A type ascription `E ⦂ T`, with the type kept in its source
    /// spelling.
    pub fn ascription(
        &self,
        expr: Expression,
        type_expr: Expression,
        span: Option<Span>,
    ) -> Expression {
        self.check_expr_children([&expr, &type_expr]);
        let mut caches = CacheBuilder::new();
        caches.add_expr(&expr);
        caches.add_expr(&type_expr);
        self.make_expr(
            ExpressionKind::Ascription { expr, type_expr },
            None,
            span,
            caches,
        )
    }

    // ----- predicates ---------------------------------------------------

    /// `⊤` or `⊥`.
    pub fn literal_predicate(&self, op: LiteralPredOp, span: Option<Span>) -> Predicate {
        self.make_pred(PredicateKind::Literal(op), span, CacheBuilder::new())
    }

    /// A predicate meta-variable; the name must start with `$`.
    #[track_caller]
    pub fn predicate_variable(&self, name: impl Into<String>, span: Option<Span>) -> Predicate {
        let name = name.into();
        assert!(
            name.starts_with('$') && name.len() > 1,
            "a predicate variable name starts with '$'"
        );
        self.make_pred(
            PredicateKind::PredicateVariable(name),
            span,
            CacheBuilder::new(),
        )
    }

    /// A relational predicate, e.g. `x = y`, `a ∈ S`.
    pub fn relational_predicate(
        &self,
        op: RelationalOp,
        left: Expression,
        right: Expression,
        span: Option<Span>,
    ) -> Predicate {
        self.check_expr_children([&left, &right]);
        let mut caches = CacheBuilder::new();
        caches.add_expr(&left);
        caches.add_expr(&right);
        self.make_pred(PredicateKind::Relational { op, left, right }, span, caches)
    }

    /// `P ⇒ Q` or `P ⇔ Q`.
    pub fn binary_predicate(
        &self,
        op: BinaryPredOp,
        left: Predicate,
        right: Predicate,
        span: Option<Span>,
    ) -> Predicate {
        self.check_pred_children([&left, &right]);
        let mut caches = CacheBuilder::new();
        caches.add_pred(&left);
        caches.add_pred(&right);
        self.make_pred(PredicateKind::Binary { op, left, right }, span, caches)
    }

    /// A conjunction or disjunction with at least two children.
    #[track_caller]
    pub fn associative_predicate(
        &self,
        op: AssocPredOp,
        children: Vec<Predicate>,
        span: Option<Span>,
    ) -> Predicate {
        assert!(
            children.len() >= 2,
            "an associative predicate needs at least two children"
        );
        self.check_pred_children(&children);
        let mut caches = CacheBuilder::new();
        for child in &children {
            caches.add_pred(child);
        }
        self.make_pred(PredicateKind::Associative { op, children }, span, caches)
    }

    /// `¬ P`.
    pub fn not_predicate(&self, child: Predicate, span: Option<Span>) -> Predicate {
        self.check_pred_children([&child]);
        let mut caches = CacheBuilder::new();
        caches.add_pred(&child);
        self.make_pred(PredicateKind::Not(child), span, caches)
    }

    /// A quantified predicate `∀ x · P` or `∃ x · P`.
    #[track_caller]
    pub fn quantified_predicate(
        &self,
        op: QuantPredOp,
        decls: Vec<BoundIdentDecl>,
        pred: Predicate,
        span: Option<Span>,
    ) -> Predicate {
        assert!(
            !decls.is_empty(),
            "a quantified predicate needs at least one declaration"
        );
        self.check_decls(&decls);
        self.check_pred_children([&pred]);
        let mut caches = CacheBuilder::new();
        for decl in &decls {
            caches.add_decl(decl);
        }
        caches.add_scoped_pred(&pred, decls.len());
        self.make_pred(PredicateKind::Quantified { op, decls, pred }, span, caches)
    }

    /// `finite(S)`.
    pub fn simple_predicate(&self, child: Expression, span: Option<Span>) -> Predicate {
        self.check_expr_children([&child]);
        let mut caches = CacheBuilder::new();
        caches.add_expr(&child);
        self.make_pred(PredicateKind::Simple(child), span, caches)
    }

    /// `partition(S, S₁, …, Sₙ)` with at least one child.
    #[track_caller]
    pub fn multiple_predicate(&self, children: Vec<Expression>, span: Option<Span>) -> Predicate {
        assert!(
            !children.is_empty(),
            "a multiple predicate needs at least one child"
        );
        self.check_expr_children(&children);
        let mut caches = CacheBuilder::new();
        for child in &children {
            caches.add_expr(child);
        }
        self.make_pred(PredicateKind::Multiple(children), span, caches)
    }

    /// User predicate application `p(x, y)` — the surface-language
    /// tolerance node.
    #[track_caller]
    pub fn predicate_application(
        &self,
        function: impl Into<String>,
        function_span: Option<Span>,
        args: Vec<Expression>,
        span: Option<Span>,
    ) -> Predicate {
        let function = function.into();
        assert!(
            !function.is_empty(),
            "applied predicate name must not be empty"
        );
        self.check_expr_children(&args);
        let mut caches = CacheBuilder::new();
        caches.add_free_name(function.clone());
        for arg in &args {
            caches.add_expr(arg);
        }
        self.make_pred(
            PredicateKind::Application {
                function,
                function_span,
                args,
            },
            span,
            caches,
        )
    }

    // ----- extended nodes -----------------------------------------------

    /// An occurrence of an expression extension. The child counts must
    /// fit the extension's kind; an explicit type must be accepted by
    /// the extension, and without one the type is synthesized when all
    /// children are typed.
    /// Shared head of the extended constructors: resolves the
    /// extension's tag, checks arity and child ownership, and builds
    /// the child caches.
    fn check_extended(
        &self,
        identity: usize,
        kind: &super::extension::ExtensionKind,
        exprs: &[Expression],
        preds: &[Predicate],
    ) -> Result<(Tag, CacheBuilder), FactoryError> {
        let tag = self.tag_of_identity(identity)?;
        if !kind.children.exprs.accepts(exprs.len()) || !kind.children.preds.accepts(preds.len()) {
            return Err(FactoryError::ArityMismatch);
        }
        self.check_expr_children(exprs);
        self.check_pred_children(preds);
        let mut caches = CacheBuilder::new();
        for child in exprs {
            caches.add_expr(child);
        }
        for child in preds {
            caches.add_pred(child);
        }
        Ok((tag, caches))
    }

    pub fn extended_expression(
        &self,
        extension: &Arc<dyn ExpressionExtension>,
        exprs: Vec<Expression>,
        preds: Vec<Predicate>,
        span: Option<Span>,
        ty: Option<Type>,
    ) -> Result<Expression, FactoryError> {
        let identity = Arc::as_ptr(extension) as *const () as usize;
        let (tag, caches) = self.check_extended(identity, &extension.kind(), &exprs, &preds)?;
        let ty = match ty {
            Some(ty) => {
                if !extension.verify_type(&ty, &exprs, &preds) {
                    return Err(FactoryError::TypeMisfit);
                }
                Some(ty)
            }
            None => {
                let all_typed = exprs.iter().all(Expression::is_type_checked)
                    && preds.iter().all(Predicate::is_type_checked);
                all_typed
                    .then(|| extension.synthesize_type(&exprs, &preds))
                    .flatten()
            }
        };
        Ok(self.make_expr(
            ExpressionKind::Extended { tag, exprs, preds },
            ty,
            span,
            caches,
        ))
    }

    /// An occurrence of a predicate extension; see
    /// [`Self::extended_expression`].
    pub fn extended_predicate(
        &self,
        extension: &Arc<dyn PredicateExtension>,
        exprs: Vec<Expression>,
        preds: Vec<Predicate>,
        span: Option<Span>,
    ) -> Result<Predicate, FactoryError> {
        let identity = Arc::as_ptr(extension) as *const () as usize;
        let (tag, caches) = self.check_extended(identity, &extension.kind(), &exprs, &preds)?;
        Ok(self.make_pred(PredicateKind::Extended { tag, exprs, preds }, span, caches))
    }

    // ----- assignments --------------------------------------------------

    /// `x, y ≔ E, F`.
    #[track_caller]
    pub fn becomes_equal_to(
        &self,
        idents: Vec<Expression>,
        values: Vec<Expression>,
        span: Option<Span>,
    ) -> Assignment {
        assert!(
            !idents.is_empty(),
            "an assignment needs at least one target"
        );
        assert_eq!(
            idents.len(),
            values.len(),
            "assignment targets and values must line up"
        );
        assert_free_identifiers(&idents);
        self.check_expr_children(idents.iter().chain(&values));
        let mut caches = CacheBuilder::new();
        for expr in idents.iter().chain(&values) {
            caches.add_expr(expr);
        }
        self.make_assign(
            AssignmentKind::BecomesEqualTo { idents, values },
            span,
            caches,
        )
    }

    /// `x, y :∈ S`.
    #[track_caller]
    pub fn becomes_member_of(
        &self,
        idents: Vec<Expression>,
        set: Expression,
        span: Option<Span>,
    ) -> Assignment {
        assert!(
            !idents.is_empty(),
            "an assignment needs at least one target"
        );
        assert_free_identifiers(&idents);
        self.check_expr_children(idents.iter().chain([&set]));
        let mut caches = CacheBuilder::new();
        for ident in &idents {
            caches.add_expr(ident);
        }
        caches.add_expr(&set);
        self.make_assign(
            AssignmentKind::BecomesMemberOf { idents, set },
            span,
            caches,
        )
    }

    /// `x, y :∣ P`, with one primed declaration per target bound over
    /// the condition.
    #[track_caller]
    pub fn becomes_such_that(
        &self,
        idents: Vec<Expression>,
        primed: Vec<BoundIdentDecl>,
        pred: Predicate,
        span: Option<Span>,
    ) -> Assignment {
        assert!(
            !idents.is_empty(),
            "an assignment needs at least one target"
        );
        assert_eq!(
            idents.len(),
            primed.len(),
            "one primed declaration per assignment target"
        );
        assert_free_identifiers(&idents);
        self.check_expr_children(&idents);
        self.check_decls(&primed);
        self.check_pred_children([&pred]);
        let mut caches = CacheBuilder::new();
        for ident in &idents {
            caches.add_expr(ident);
        }
        for decl in &primed {
            caches.add_decl(decl);
        }
        caches.add_scoped_pred(&pred, primed.len());
        self.make_assign(
            AssignmentKind::BecomesSuchThat {
                idents,
                primed,
                pred,
            },
            span,
            caches,
        )
    }
}

impl PartialEq for FormulaFactory {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for FormulaFactory {}

/// Hashes by identity, consistent with [`PartialEq`], so a factory can key a
/// map of per-factory caches.
impl std::hash::Hash for FormulaFactory {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state);
    }
}

impl std::fmt::Debug for FormulaFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FormulaFactory")
    }
}

#[track_caller]
fn assert_free_identifiers(idents: &[Expression]) {
    for ident in idents {
        assert!(
            matches!(ident.kind(), ExpressionKind::FreeIdentifier(_)),
            "assignment targets must be free identifiers"
        );
    }
}
