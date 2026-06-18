//! Shared Event-B symbol model and cursor-to-symbol resolution for LSP providers.
//!
//! `SymbolKind` classifies the five kinds of named symbols, and
//! `enumerate_symbols` walks a parsed `Component` to list them. On top of that
//! taxonomy, `SymbolIdentity` names a *resolved* symbol (its declaring
//! component, and the event for a parameter). `resolve_cursor` turns a cursor
//! position into a `Resolution`, scope-aware and most-local first: a formula
//! binder the cursor sits on or is bound by (a quantifier / lambda /
//! comprehension binder) resolves to its own local scope; an event `ANY`
//! parameter to its own event; and a global symbol to the component that
//! declares it by walking the refinement / sees / extends chains.
//! Go-to-definition, find-references, and rename build on this one resolver (and
//! the shared binder walk it delegates to) so the features cannot drift on what
//! a name means.

use rossi::Component;
use rossi::ast::Span;

use crate::component_loader::ComponentLoader;
use crate::component_util::{component_at_offset, parse_all};
use crate::cross_references::{ComponentKind, CrossReferenceManager};
use crate::document::ParsedDocument;
use crate::formula_walk;
use crate::identifier_utils::position_to_offset;
use crate::lsp_types::{Location, Position};
use crate::position::span_to_range;
use crate::text_utils;

/// The canonical name of the implicit initialisation event. `enumerate_symbols`
/// mints it and `event_declaration_span` matches it, so the spelling lives in
/// one place rather than as two coupled string literals.
pub(crate) const INITIALISATION_EVENT_NAME: &str = "INITIALISATION";

/// The kind of an Event-B named symbol.
///
/// Sets and constants are declared in contexts; variables, events, and
/// parameters in machines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Set,
    Constant,
    Variable,
    Event,
    Parameter,
}

/// A symbol discovered while walking a component: its name, kind, the owning
/// component (machine or context), and — for parameters — the enclosing event.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SymbolRef {
    pub name: String,
    pub kind: SymbolKind,
    pub owner: String,
    pub event: Option<String>,
}

/// Enumerate every named symbol declared directly in `component`.
///
/// Order is stable and follows source declaration order: contexts yield sets
/// then constants; machines yield variables, the INITIALISATION event (when
/// present), then each event followed by its parameters.
pub fn enumerate_symbols(component: &Component) -> Vec<SymbolRef> {
    let mut symbols = Vec::new();

    match component {
        Component::Context(context) => {
            for set in &context.sets {
                symbols.push(SymbolRef {
                    name: set.name().to_string(),
                    kind: SymbolKind::Set,
                    owner: context.name.clone(),
                    event: None,
                });
            }
            for constant in &context.constants {
                symbols.push(SymbolRef {
                    name: constant.name.clone(),
                    kind: SymbolKind::Constant,
                    owner: context.name.clone(),
                    event: None,
                });
            }
        }
        Component::Machine(machine) => {
            for variable in &machine.variables {
                symbols.push(SymbolRef {
                    name: variable.name.clone(),
                    kind: SymbolKind::Variable,
                    owner: machine.name.clone(),
                    event: None,
                });
            }
            if machine.initialisation.is_some() {
                symbols.push(SymbolRef {
                    name: INITIALISATION_EVENT_NAME.to_string(),
                    kind: SymbolKind::Event,
                    owner: machine.name.clone(),
                    event: None,
                });
            }
            for event in &machine.events {
                symbols.push(SymbolRef {
                    name: event.name.clone(),
                    kind: SymbolKind::Event,
                    owner: machine.name.clone(),
                    event: None,
                });
                for parameter in &event.parameters {
                    symbols.push(SymbolRef {
                        name: parameter.name.clone(),
                        kind: SymbolKind::Parameter,
                        owner: machine.name.clone(),
                        event: Some(event.name.clone()),
                    });
                }
            }
        }
    }

    symbols
}

/// A resolved symbol: which named entity a cursor position refers to.
///
/// `owner` is the component that *declares* the symbol (not necessarily the
/// component under the cursor — a name may resolve up a refinement / sees /
/// extends chain), and `event` is the enclosing event for a parameter. This is
/// the identity find-references groups its hits by and go-to-definition maps to
/// a declaration site.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SymbolIdentity {
    pub(crate) name: String,
    pub(crate) kind: SymbolKind,
    pub(crate) owner: String,
    pub(crate) event: Option<String>,
}

impl SymbolIdentity {
    pub(crate) fn parameter(name: &str, machine_name: &str, event_name: &str) -> Self {
        Self {
            name: name.to_string(),
            kind: SymbolKind::Parameter,
            owner: machine_name.to_string(),
            event: Some(event_name.to_string()),
        }
    }
}

impl From<SymbolRef> for SymbolIdentity {
    fn from(symbol: SymbolRef) -> Self {
        Self {
            name: symbol.name,
            kind: symbol.kind,
            owner: symbol.owner,
            event: symbol.event,
        }
    }
}

/// What the identifier at a cursor position resolves to.
///
/// Either a named component-level symbol (a variable / constant / set / event,
/// or an event `ANY` parameter) — the identity find-references groups its hits
/// by and go-to-definition maps to a declaration site — or a formula binder
/// local to one component, carrying its declaration and in-scope occurrences
/// inline (its `is_event_parameter` flag is always `false` here, since event
/// parameters resolve to a `Symbol`). Definition, references, and hover all
/// resolve through this one enum so they cannot disagree on what a name means.
pub(crate) enum Resolution {
    /// A component-level / inherited symbol, or an event parameter.
    Symbol(SymbolIdentity),
    /// A formula binder (a quantifier / lambda / comprehension binder) local to
    /// the cursor's own component — formula binders never cross a component
    /// boundary, so every span is in the cursor document.
    Bound(formula_walk::BoundResolution),
}

/// Resolve the identifier at `position` to what it names, scope-aware and
/// most-local first.
///
/// A formula binder the cursor sits on or is bound by is the most local scope and
/// resolves to a [`Resolution::Bound`]; an event `ANY` parameter and a
/// component-level / inherited symbol resolve to a [`Resolution::Symbol`].
///
/// `text` is the document the offsets index into and `masked` its comment-masked
/// form; `cursor` is the document's stored parse when it is open (its components
/// and `text` are one snapshot). When there is no stored parse (no document
/// manager, or a not-yet-opened file), the text is parsed with error recovery so
/// a symbol elsewhere in a broken document is still resolvable.
pub(crate) fn resolve_cursor(
    text: &str,
    masked: &str,
    position: Position,
    identifier: &str,
    loader: &ComponentLoader,
    cursor: Option<&ParsedDocument>,
) -> Option<Resolution> {
    let offset = position_to_offset(text, position).unwrap_or(text.len());

    let owned;
    let components: &[Component] = match cursor {
        Some(parsed) => parsed.components(),
        None => {
            owned = parse_all(text);
            &owned
        }
    };
    let component = component_at_offset(components, offset)?;

    // A formula binder the cursor sits on or is bound by is the most local scope.
    // Event `ANY` parameters are seeded as binders too, but they keep their own
    // component/event symbol identity below (richer for hover, cross-checked for
    // references), so they are excluded here and fall through to the parameter
    // path.
    if let Some(bound) = formula_walk::resolve_bound_at_offset(component, identifier, offset)
        && !bound.is_event_parameter
    {
        return Some(Resolution::Bound(bound));
    }

    resolve_symbol_identity_at_position(component, masked, position, identifier, loader)
        .map(Resolution::Symbol)
}

/// Resolve `identifier` within the component the cursor sits in. An event `ANY`
/// parameter (scoped to its event and shadowing a same-named global) is tried
/// first, positionally, before the component-wide symbols.
fn resolve_symbol_identity_at_position(
    component: &Component,
    masked: &str,
    position: Position,
    identifier: &str,
    loader: &ComponentLoader,
) -> Option<SymbolIdentity> {
    if let Component::Machine(machine) = component
        && let Some(parameter) =
            local_parameter_symbol_identity_at_position(machine, masked, position, identifier)
    {
        return Some(parameter);
    }

    resolve_symbol_identity_in_component(component, identifier, loader)
}

/// Resolve `identifier` to a symbol visible from `component`: declared directly,
/// or inherited through the refinement chain (machines), the visible contexts
/// (machines), or the extends chain (contexts). The local component is checked
/// first, so a local declaration shadows a same-named inherited one.
pub(crate) fn resolve_symbol_identity_in_component(
    component: &Component,
    identifier: &str,
    loader: &ComponentLoader,
) -> Option<SymbolIdentity> {
    if let Some(local) = local_symbol_identity(component, identifier) {
        return Some(local);
    }

    let manager = loader.manager();
    match component {
        Component::Machine(machine) => {
            for machine_name in manager.refinement_chain(&machine.name) {
                if let Some(loaded) = loader.load(&machine_name)
                    && let Some(symbol) = local_symbol_identity(loaded.component(), identifier)
                {
                    return Some(symbol);
                }
            }

            for context_name in manager.ordered_visible_contexts(&machine.name) {
                if let Some(loaded) = loader.load(&context_name)
                    && let Some(symbol) = local_symbol_identity(loaded.component(), identifier)
                {
                    return Some(symbol);
                }
            }
        }
        Component::Context(context) => {
            for context_name in manager.ordered_extends_chain(&context.name) {
                if let Some(loaded) = loader.load(&context_name)
                    && let Some(symbol) = local_symbol_identity(loaded.component(), identifier)
                {
                    return Some(symbol);
                }
            }
        }
    }

    None
}

/// Every component that could hold a reference to `symbol`: its owner plus the
/// components that inherit it (contexts/machines extending or seeing the owner,
/// machines refining it). A parameter is event-local, so only its owner.
pub(crate) fn candidate_components_for_symbol(
    symbol: &SymbolIdentity,
    manager: &CrossReferenceManager,
) -> Vec<String> {
    if symbol.kind == SymbolKind::Parameter {
        return vec![symbol.owner.clone()];
    }

    let mut candidates = Vec::new();
    let mut component_names = manager.all_component_names();
    component_names.sort();

    for component_name in component_names {
        if component_name == symbol.owner {
            candidates.push(component_name);
            continue;
        }

        let Some(info) = manager.get_component(&component_name) else {
            continue;
        };

        match (symbol.kind, info.kind) {
            (SymbolKind::Set | SymbolKind::Constant, ComponentKind::Context)
                if manager
                    .ordered_extends_chain(&component_name)
                    .contains(&symbol.owner) =>
            {
                candidates.push(component_name);
            }
            (SymbolKind::Set | SymbolKind::Constant, ComponentKind::Machine)
                if manager
                    .ordered_visible_contexts(&component_name)
                    .contains(&symbol.owner) =>
            {
                candidates.push(component_name);
            }
            (SymbolKind::Variable | SymbolKind::Event, ComponentKind::Machine)
                if manager
                    .refinement_chain(&component_name)
                    .contains(&symbol.owner) =>
            {
                candidates.push(component_name);
            }
            _ => {}
        }
    }

    candidates.sort();
    candidates.dedup();
    candidates
}

/// Resolve `identifier` to a symbol declared directly in `component`.
///
/// Parameters are excluded here — they are scoped to an event body and resolved
/// positionally by [`resolve_symbol_identity_at_position`].
fn local_symbol_identity(component: &Component, identifier: &str) -> Option<SymbolIdentity> {
    enumerate_symbols(component)
        .into_iter()
        .find(|symbol| symbol.name == identifier && symbol.kind != SymbolKind::Parameter)
        .map(SymbolIdentity::from)
}

/// Resolve `identifier` to the event `ANY` parameter it names, when the cursor's
/// line falls inside an event whose parameters include it. The shared resolver
/// [`text_utils::event_parameter_at_position`] owns the event-at-position
/// scoping (also used by hover), so the features cannot disagree on whether a
/// name is an event parameter here.
fn local_parameter_symbol_identity_at_position(
    machine: &rossi::Machine,
    masked: &str,
    position: Position,
    identifier: &str,
) -> Option<SymbolIdentity> {
    let event = text_utils::event_parameter_at_position(machine, masked, position, identifier)?;
    Some(SymbolIdentity::parameter(
        identifier,
        &machine.name,
        &event.name,
    ))
}

/// The declaration site of a resolved symbol — the source location
/// go-to-definition jumps to.
///
/// Loads `symbol.owner` (the declaring component, which may differ from the one
/// under the cursor) and reads the declared name's span from its AST: a
/// set / constant / variable via [`formula_walk::declaration_span`], an event via
/// its `EVENT name` token (or the `INITIALISATION` keyword), and a parameter via
/// its `ANY`-clause name span in the owning event. `None` when the component
/// cannot be loaded or the declaration has no recorded span.
pub(crate) fn declaration_location(
    symbol: &SymbolIdentity,
    loader: &ComponentLoader,
) -> Option<Location> {
    let loaded = loader.load(&symbol.owner)?;
    let span = match symbol.kind {
        SymbolKind::Parameter => {
            parameter_declaration_span(loaded.component(), symbol.event.as_deref()?, &symbol.name)?
        }
        SymbolKind::Event => event_declaration_span(loaded.component(), &symbol.name)?,
        SymbolKind::Variable | SymbolKind::Constant | SymbolKind::Set => {
            formula_walk::declaration_span(loaded.component(), &symbol.name)?
        }
    };
    Some(Location {
        uri: loaded.uri().clone(),
        range: span_to_range(&span, loaded.text()),
    })
}

/// The declaration name-span of `event`'s `ANY`-clause parameter `name`, if it
/// declares one. Shared with find-references so go-to-definition and the
/// declaration entry of find-references locate a parameter the same way.
pub(crate) fn event_parameter_span(event: &rossi::Event, name: &str) -> Option<Span> {
    event
        .parameters
        .iter()
        .find(|parameter| parameter.name == name)
        .and_then(|parameter| parameter.span)
}

/// The name span of parameter `name` declared in `event_name`'s `ANY` clause.
fn parameter_declaration_span(component: &Component, event_name: &str, name: &str) -> Option<Span> {
    let Component::Machine(machine) = component else {
        return None;
    };
    let event = machine
        .events
        .iter()
        .find(|event| event.name == event_name)?;
    event_parameter_span(event, name)
}

/// The name-token span of an event named `name` declared in `component`, or the
/// `INITIALISATION` keyword span for the implicit initialisation event.
fn event_declaration_span(component: &Component, name: &str) -> Option<Span> {
    let Component::Machine(machine) = component else {
        return None;
    };
    if name == INITIALISATION_EVENT_NAME {
        return machine
            .initialisation
            .as_ref()
            .and_then(|init| init.name_span);
    }
    machine
        .events
        .iter()
        .find(|event| event.name == name)
        .and_then(|event| event.name_span)
}
