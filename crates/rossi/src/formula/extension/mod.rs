//! Operator extensions: user-defined operators as first-class nodes.
//!
//! An extension describes one operator — its syntax symbol, its kind
//! (notation, produced formula class, child arities), its typing rules
//! and its well-definedness contribution. Extensions are registered
//! process-globally: each distinct extension object is assigned one
//! dynamic tag (from [`FIRST_EXTENSION_TAG`]) for the lifetime of the
//! process, and factories carrying equal extension sets are interned
//! to the same instance, so factory identity keeps meaning equality of
//! the supported language.
//!
//! [`FIRST_EXTENSION_TAG`]: super::tag::FIRST_EXTENSION_TAG

pub mod datatype;

use std::sync::Arc;

use super::expression::Expression;
use super::predicate::Predicate;
use super::typecheck::{TcType, TypeCheckMediator};
use super::types::Type;
use super::wd::WdMediator;

/// How an operator is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notation {
    /// `op(a, b)`
    Prefix,
    /// `a op b`
    Infix,
}

/// Which formula class an operator produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormulaType {
    /// The operator is an expression.
    Expression,
    /// The operator is a predicate.
    Predicate,
}

/// How many children of one class an operator takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arity {
    /// Exactly `n` children.
    Fixed(usize),
    /// At least `n` children (associative operators).
    AtLeast(usize),
}

impl Arity {
    /// Whether `count` children satisfy this arity.
    pub fn accepts(&self, count: usize) -> bool {
        match self {
            Arity::Fixed(n) => count == *n,
            Arity::AtLeast(n) => count >= *n,
        }
    }
}

/// The wording of an expected arity in a diagnostic: `2`, `at least 1`.
impl std::fmt::Display for Arity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Arity::Fixed(n) => write!(f, "{n}"),
            Arity::AtLeast(n) => write!(f, "at least {n}"),
        }
    }
}

/// The child shape of an operator: how many expression children,
/// followed by how many predicate children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeDistribution {
    /// Arity of expression children.
    pub exprs: Arity,
    /// Arity of predicate children.
    pub preds: Arity,
}

/// The complete syntactic kind of an operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtensionKind {
    /// How the operator is written.
    pub notation: Notation,
    /// The formula class it produces.
    pub formula_type: FormulaType,
    /// Its child shape.
    pub children: TypeDistribution,
    /// Whether nested occurrences flatten.
    pub associative: bool,
}

impl ExtensionKind {
    /// A nullary expression operator.
    pub const fn atomic_expression() -> ExtensionKind {
        Self::prefix_expression(0)
    }

    /// Whether the operator takes no children at all, so its symbol stands
    /// bare like an atom.
    pub fn is_nullary(&self) -> bool {
        self.children.exprs == Arity::Fixed(0) && self.children.preds == Arity::Fixed(0)
    }

    /// A prefix expression operator over `n` expression children.
    pub const fn prefix_expression(n: usize) -> ExtensionKind {
        ExtensionKind {
            notation: Notation::Prefix,
            formula_type: FormulaType::Expression,
            children: TypeDistribution {
                exprs: Arity::Fixed(n),
                preds: Arity::Fixed(0),
            },
            associative: false,
        }
    }

    /// A binary infix expression operator, optionally associative.
    pub const fn infix_expression(associative: bool) -> ExtensionKind {
        ExtensionKind {
            notation: Notation::Infix,
            formula_type: FormulaType::Expression,
            children: TypeDistribution {
                exprs: if associative {
                    Arity::AtLeast(2)
                } else {
                    Arity::Fixed(2)
                },
                preds: Arity::Fixed(0),
            },
            associative,
        }
    }

    /// A prefix predicate operator over `n` expression children.
    pub const fn prefix_predicate(n: usize) -> ExtensionKind {
        ExtensionKind {
            notation: Notation::Prefix,
            formula_type: FormulaType::Predicate,
            children: TypeDistribution {
                exprs: Arity::Fixed(n),
                preds: Arity::Fixed(0),
            },
            associative: false,
        }
    }
}

/// A borrowed view of an extended node's children.
#[derive(Debug, Clone, Copy)]
pub struct ExtendedRef<'a> {
    /// Expression children.
    pub exprs: &'a [Expression],
    /// Predicate children.
    pub preds: &'a [Predicate],
}

/// Behavior common to all operator extensions.
///
/// Extensions are identified by object identity: registering the same
/// `Arc` twice yields the same tag, two structurally identical but
/// distinct objects get distinct tags.
pub trait FormulaExtension: Send + Sync {
    /// The operator's syntax symbol, e.g. `dist`.
    fn symbol(&self) -> &str;

    /// A stable unique identifier for the operator.
    fn id(&self) -> &str;

    /// The operator group, for parser precedence wiring.
    fn group_id(&self) -> &str;

    /// The operator's syntactic kind.
    fn kind(&self) -> ExtensionKind;

    /// Whether the children's well-definedness is conjoined with
    /// [`Self::wd_predicate`] (strict operators), or the extension's
    /// predicate stands alone.
    fn conjoin_children_wd(&self) -> bool;

    /// The operator's own well-definedness contribution.
    fn wd_predicate(&self, formula: ExtendedRef<'_>, wd: &WdMediator<'_>) -> Predicate;
}

/// An extension producing expressions.
pub trait ExpressionExtension: FormulaExtension {
    /// The node type when every child is typed, if derivable.
    fn synthesize_type(&self, exprs: &[Expression], preds: &[Predicate]) -> Option<Type>;

    /// Whether `proposed` is a legal type for the given children.
    fn verify_type(&self, proposed: &Type, exprs: &[Expression], preds: &[Predicate]) -> bool;

    /// Registers the operator's typing constraints; returns the node's
    /// type. Expression children arrive as solver handles, predicate
    /// children have already been checked.
    fn type_check(&self, mediator: &mut TypeCheckMediator<'_, '_>, exprs: &[TcType]) -> TcType;

    /// Whether this operator is a type constructor (its instances can
    /// appear in [`Type::Parametric`]).
    fn is_a_type_constructor(&self) -> bool {
        false
    }
}

/// An extension producing predicates.
pub trait PredicateExtension: FormulaExtension {
    /// Registers the operator's typing constraints.
    fn type_check(&self, mediator: &mut TypeCheckMediator<'_, '_>, exprs: &[TcType]);
}

/// One registered extension, of either class.
#[derive(Clone)]
pub enum Extension {
    /// An expression operator.
    Expr(Arc<dyn ExpressionExtension>),
    /// A predicate operator.
    Pred(Arc<dyn PredicateExtension>),
}

impl Extension {
    /// The extension's common behavior.
    pub fn common(&self) -> &dyn FormulaExtension {
        match self {
            Extension::Expr(e) => e.as_ref(),
            Extension::Pred(p) => p.as_ref(),
        }
    }

    /// The identity pointer used for tag registration.
    pub(super) fn identity(&self) -> usize {
        match self {
            Extension::Expr(e) => Arc::as_ptr(e) as *const () as usize,
            Extension::Pred(p) => Arc::as_ptr(p) as *const () as usize,
        }
    }
}

impl std::fmt::Debug for Extension {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Extension({})", self.common().symbol())
    }
}
