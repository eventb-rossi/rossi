//! `textDocument/typeDefinition`: from a variable, constant or parameter to
//! the carrier sets its type is built from.
//!
//! Event-B has no type declarations of its own; a type is an expression over
//! `ℤ`, `BOOL` and the carrier sets declared in `SETS` clauses. So "go to the
//! type" means: infer the symbol's type with the static checker, take every
//! given set it mentions, and jump to where each is declared. `x ∈ ℙ(USERS ×
//! ROLES)` offers `USERS` and `ROLES`; `n ∈ ℕ` offers nothing, because `ℤ` is
//! declared nowhere.

use rossi::Component;
use rossi::formula::Type;

use crate::component_loader::ComponentLoader;
use crate::document::ParsedDocument;
use crate::identifier_utils;
use crate::lsp_types::*;
use crate::position::span_to_range;
use crate::symbols::{Resolution, SymbolIdentity, SymbolKind, resolve_cursor};

/// The declarations of the carrier sets in the type of the symbol at
/// `position`, or `None` when the cursor is not on a typed symbol.
pub(crate) fn type_definitions(
    doc: &ParsedDocument,
    loader: &ComponentLoader,
    position: Position,
) -> Option<Vec<Location>> {
    let masked = rossi::comments::mask_comments_chars(doc.text());
    let (name, _) = identifier_utils::identifier_at_position(&masked, position)?;

    // The shared, scope-aware resolver: a binder or an event parameter that
    // shadows a variable resolves to itself, and `x'` to its base.
    let symbol = match resolve_cursor(doc.text(), &masked, position, &name, loader, Some(doc))? {
        Resolution::Symbol(symbol) => symbol,
        Resolution::Component(_) | Resolution::Bound(_) => return None,
    };

    // The same closure and check the inlay hints run.
    let project = crate::closure::project_for(doc, loader, "lsp-type-definition");
    let (_result, model) = rossi_build::check_with_model(&project);

    let sets = match symbol.kind {
        // A carrier set is its own type; its declaration is the answer.
        SymbolKind::Set => vec![symbol.name.clone()],
        SymbolKind::Event => return None,
        _ => {
            let ty = declared_type(&model, &symbol)?;
            let mut sets = Vec::new();
            ty.collect_given_sets(&mut sets);
            sets.sort();
            sets.dedup();
            sets
        }
    };

    let locations: Vec<Location> = sets
        .iter()
        .filter_map(|set| declaration_of(set, &project, loader))
        .collect();
    (!locations.is_empty()).then_some(locations)
}

/// The checked type of `symbol` in its owning component's record.
fn declared_type(model: &rossi_build::sc_model::ScModel, symbol: &SymbolIdentity) -> Option<Type> {
    match symbol.kind {
        SymbolKind::Variable => model
            .machines
            .get(&symbol.owner)?
            .record
            .variables
            .iter()
            .find(|v| v.name == symbol.name)
            .map(|v| v.ty.clone()),
        SymbolKind::Parameter => {
            let event = symbol.event.as_deref()?;
            model
                .machines
                .get(&symbol.owner)?
                .record
                .events
                .iter()
                .find(|e| e.label == event)?
                .parameters
                .iter()
                .find(|p| p.name == symbol.name)
                .map(|p| p.ty.clone())
        }
        SymbolKind::Constant => model
            .contexts
            .get(&symbol.owner)?
            .record
            .constants
            .iter()
            .find(|c| c.name == symbol.name)
            .map(|c| c.ty.clone()),
        SymbolKind::Set | SymbolKind::Event => None,
    }
}

/// Where the carrier set `set` is declared: in a context of the closure
/// first (the only ones whose sets can appear in the type, and already in
/// hand), else in whichever indexed context declares it.
fn declaration_of(
    set: &str,
    project: &rossi_build::Project,
    loader: &ComponentLoader,
) -> Option<Location> {
    let in_context = |component: &Component, text: &str, uri: &Url| {
        let Component::Context(context) = component else {
            return None;
        };
        let element = context.sets.iter().find(|s| s.name == set)?;
        Some(Location::new(
            uri.clone(),
            span_to_range(&element.span?, text),
        ))
    };

    project
        .components
        .iter()
        .filter(|component| matches!(component.component, Component::Context(_)))
        .map(|component| component.component.name().to_string())
        .chain(
            loader
                .manager()
                .component_names_of_kind(rossi::deps::ComponentKind::Context),
        )
        .filter_map(|candidate| loader.load(&candidate))
        .find_map(|loaded| in_context(loaded.component(), loaded.text(), loaded.uri()))
}
