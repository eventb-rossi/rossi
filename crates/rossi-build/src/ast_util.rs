//! Small AST construction helpers shared across the static checker.

/// Names that an action writes to (its LHS targets). Shared by the SC
/// cascade-drop logic and the lint module's unmodified-variable / INIT
/// completeness checks.
pub(crate) fn lhs_variables(assignment: &rossi::Assignment) -> Vec<&str> {
    use rossi::{AssignmentKind, ExpressionKind};
    let idents = match assignment.kind() {
        AssignmentKind::BecomesEqualTo { idents, .. }
        | AssignmentKind::BecomesMemberOf { idents, .. }
        | AssignmentKind::BecomesSuchThat { idents, .. } => idents,
    };
    idents
        .iter()
        .filter_map(|ident| match ident.kind() {
            ExpressionKind::FreeIdentifier(name) => Some(name.as_str()),
            _ => None,
        })
        .collect()
}

/// Source span of the named element called `name`, when present and located.
/// The static checker's type-inference passes work over the declared names, so
/// they use this to recover the offending constant / variable / parameter's
/// span and anchor a diagnostic on its declaration rather than the component.
pub(crate) fn named_element_span(
    elements: &[rossi::NamedElement],
    name: &str,
) -> Option<rossi::ast::Span> {
    elements
        .iter()
        .find(|e| e.name == name)
        .and_then(|e| e.span)
}
