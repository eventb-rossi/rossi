//! Small AST construction helpers shared across the static checker.

use rossi::Expression;
use rossi::ast::expression::BinaryOp;

/// Names that an action writes to (its LHS targets). Shared by the SC
/// cascade-drop logic and the lint module's unmodified-variable / INIT
/// completeness checks.
pub(crate) fn lhs_variables(action: &rossi::Action) -> Vec<&str> {
    use rossi::Action;
    match action {
        Action::Skip => Vec::new(),
        Action::Assignment { variables, .. }
        | Action::BecomesIn { variables, .. }
        | Action::BecomesSuchThat { variables, .. } => {
            variables.iter().map(String::as_str).collect()
        }
        Action::FunctionOverride { function, .. } => vec![function.as_str()],
    }
}

/// Build a left-associative maplet chain from a non-empty argument list:
/// `[a]` → `a`; `[a, b]` → `a ↦ b`; `[a, b, c]` → `(a ↦ b) ↦ c`.
///
/// Used to align curried/multi-arg call sites against a function's
/// product-shaped domain type, and to normalise `FunctionOverride`
/// argument tuples.
pub(crate) fn left_assoc_maplet(args: &[Expression]) -> Expression {
    let mut iter = args.iter().cloned();
    let mut acc = iter.next().expect("left_assoc_maplet requires ≥1 argument");
    for next in iter {
        acc = Expression::Binary {
            op: BinaryOp::Maplet,
            left: Box::new(acc),
            right: Box::new(next),
        };
    }
    acc
}
