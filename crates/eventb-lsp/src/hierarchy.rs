//! `textDocument/prepareTypeHierarchy` and its super/subtype follow-ups, plus
//! `textDocument/implementation`, over Event-B's refinement and extension
//! relations.
//!
//! A machine REFINES an abstract machine and a context EXTENDS an abstract
//! context; both are "this component is a more concrete form of that one",
//! which is exactly what a type hierarchy displays. SEES is deliberately not
//! part of it: a machine that sees a context is *using* it, not refining it,
//! and folding the two together would make the tree claim a refinement that
//! does not exist.
//!
//! The edges come from [`CrossReferenceManager`]'s dependency graph, which
//! already holds both directions, so subtypes need no extra index.

use std::sync::Arc;

use crate::component_loader::ComponentLoader;
use crate::component_util::{component_at_offset, covers};
use crate::cross_references::{ComponentKind, CrossReferenceManager, ReferenceKind};
use crate::document::DocumentManager;
use crate::lsp_types::*;
use crate::position::span_to_range;
use crate::symbols::{INITIALISATION_EVENT_NAME, event_declaration_span};
use rossi::deps::kind_and_name;

/// Resolves type hierarchy items against the workspace dependency graph.
pub struct TypeHierarchyProvider {
    cross_ref_manager: Arc<CrossReferenceManager>,
    document_manager: Arc<DocumentManager>,
}

impl TypeHierarchyProvider {
    pub fn new(
        cross_ref_manager: Arc<CrossReferenceManager>,
        document_manager: Arc<DocumentManager>,
    ) -> Self {
        Self {
            cross_ref_manager,
            document_manager,
        }
    }

    /// The component the cursor sits in, as the root of a hierarchy.
    ///
    /// Anywhere inside a component counts, not just its name: a user asking
    /// for the refinement tree while reading an invariant means the machine
    /// that invariant belongs to. A file holding several components resolves
    /// to the one whose span covers the cursor.
    pub fn prepare(&self, uri: &Url, position: Position) -> Option<Vec<TypeHierarchyItem>> {
        let doc = self.document_manager.parse_result(uri)?;
        let offset = crate::position::position_to_offset(doc.text(), position)?;

        let component = component_at_offset(doc.components(), offset)?;

        let item = item_for(component, doc.text(), uri)?;
        Some(vec![item])
    }

    /// What `item` refines or extends: one step up, not the whole chain, so
    /// the client can expand lazily the way the protocol intends.
    pub fn supertypes(&self, item: &TypeHierarchyItem) -> Option<Vec<TypeHierarchyItem>> {
        let info = self.cross_ref_manager.get_component(&item.name)?;
        let kind = refinement_edge(info.kind);
        let names = info.references.get(&kind).cloned().unwrap_or_default();
        Some(self.items_for_names(&names))
    }

    /// What refines or extends `item`.
    pub fn subtypes(&self, item: &TypeHierarchyItem) -> Option<Vec<TypeHierarchyItem>> {
        let info = self.cross_ref_manager.get_component(&item.name)?;
        let names: Vec<String> = self
            .cross_ref_manager
            .find_referencing_components(&item.name, Some(refinement_edge(info.kind)))
            .into_iter()
            .map(|referencing| referencing.name)
            .collect();
        Some(self.items_for_names(&names))
    }

    /// Build an item per name, dropping any the workspace can no longer load
    /// — a graph edge can outlive the file it points at, and a hierarchy row
    /// with no location is worse than a missing row.
    fn items_for_names(&self, names: &[String]) -> Vec<TypeHierarchyItem> {
        let loader = ComponentLoader::new(&self.cross_ref_manager, Some(&self.document_manager));
        names
            .iter()
            .filter_map(|name| {
                let loaded = loader.load(name)?;
                item_for(loaded.component(), loaded.text(), loaded.uri())
            })
            .collect()
    }

    /// `textDocument/implementation`: what concretises the thing under the cursor.
    ///
    /// On an event name, the events that refine it in the machines that refine
    /// this one. Anywhere else in a component, the components that refine or
    /// extend it. Both are one level down, matching the hierarchy's one-step
    /// answers rather than flattening a whole chain into the jump list.
    pub fn implementations(&self, uri: &Url, position: Position) -> Option<Vec<Location>> {
        let documents = &*self.document_manager;
        let manager = &*self.cross_ref_manager;
        let doc = documents.parse_result(uri)?;
        let offset = crate::position::position_to_offset(doc.text(), position)?;

        let component = component_at_offset(doc.components(), offset)?;

        let loader = ComponentLoader::new(manager, Some(documents));
        let refiners = manager.find_referencing_components(
            component.name(),
            Some(refinement_edge(kind_and_name(component).0)),
        );

        // On an event name the answer is per event; anywhere else it is the
        // refining components themselves. INITIALISATION is not in `events` — the
        // AST keeps it in its own field — so it is matched separately, by the name
        // it always has.
        let event: Option<&str> = match component {
            rossi::Component::Machine(machine) => machine
                .events
                .iter()
                .find(|event| event.name_span.is_some_and(|span| covers(span, offset)))
                .map(|event| event.name.as_str())
                .or_else(|| {
                    machine
                        .initialisation
                        .as_ref()
                        .filter(|init| init.name_span.is_some_and(|span| covers(span, offset)))
                        .map(|_| INITIALISATION_EVENT_NAME)
                }),
            rossi::Component::Context(_) => None,
        };

        let locations: Vec<Location> = refiners
            .iter()
            .filter_map(|refiner| {
                let loaded = loader.load(&refiner.name)?;
                Some(match event {
                    Some(name) => {
                        refining_events(loaded.component(), loaded.text(), loaded.uri(), name)
                    }
                    None => loaded
                        .component()
                        .name_span()
                        .map(|span| {
                            vec![Location::new(
                                loaded.uri().clone(),
                                span_to_range(&span, loaded.text()),
                            )]
                        })
                        .unwrap_or_default(),
                })
            })
            .flatten()
            .collect();

        (!locations.is_empty()).then_some(locations)
    }
}

/// Every event in `component` that refines the abstract event `name`.
///
/// An event names its abstract events in a REFINES clause. An event with no
/// such clause implicitly refines the abstract event of the same name, which
/// is the rule Rodin applies, so a concrete event that simply reuses the name
/// is reported too. INITIALISATION lives in its own AST field rather than in
/// `events`, and always refines its abstract counterpart.
fn refining_events(
    component: &rossi::Component,
    text: &str,
    uri: &Url,
    name: &str,
) -> Vec<Location> {
    let rossi::Component::Machine(machine) = component else {
        return Vec::new();
    };

    if name == INITIALISATION_EVENT_NAME {
        return event_declaration_span(component, name)
            .map(|span| vec![Location::new(uri.clone(), span_to_range(&span, text))])
            .unwrap_or_default();
    }

    machine
        .events
        .iter()
        .filter(|event| {
            if event.refines.is_empty() {
                event.name == name
            } else {
                event
                    .refines
                    .iter()
                    .any(|abstract_event| abstract_event.name == name)
            }
        })
        .filter_map(|event| {
            Some(Location::new(
                uri.clone(),
                span_to_range(&event.name_span?, text),
            ))
        })
        .collect()
}

/// The edge that means "is a refinement of" for a component of this kind.
fn refinement_edge(kind: ComponentKind) -> ReferenceKind {
    match kind {
        ComponentKind::Machine => ReferenceKind::Refines,
        ComponentKind::Context => ReferenceKind::Extends,
    }
}

/// One hierarchy row for a parsed component. `None` when the component has no
/// name span, which only a recovered parse produces.
fn item_for(component: &rossi::Component, text: &str, uri: &Url) -> Option<TypeHierarchyItem> {
    let selection_range = span_to_range(&component.name_span()?, text);
    // `range` must enclose `selection_range`; a component whose outer span was
    // lost to recovery falls back to the name itself rather than to a range
    // that might not contain it.
    let range = component
        .span()
        .map(|span| span_to_range(&span, text))
        .unwrap_or(selection_range);

    Some(TypeHierarchyItem {
        name: component.name().to_string(),
        // The outline already draws both machines and contexts as modules;
        // matching it keeps one icon per concept across the two views.
        kind: SymbolKind::MODULE,
        tags: None,
        detail: Some(
            match component {
                rossi::Component::Machine(_) => "Machine",
                rossi::Component::Context(_) => "Context",
            }
            .to_string(),
        ),
        uri: uri.clone(),
        range,
        selection_range,
        // Component names are unique across a workspace (a duplicate is
        // reported as EB019), so the name in the item is enough to resolve
        // the follow-up requests; no opaque payload is needed.
        data: None,
    })
}
