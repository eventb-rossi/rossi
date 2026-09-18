//! `textDocument/prepareTypeHierarchy` and its super/subtype follow-ups,
//! over Event-B's refinement and extension relations.
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
use crate::cross_references::{ComponentKind, CrossReferenceManager, ReferenceKind};
use crate::document::DocumentManager;
use crate::lsp_types::*;
use crate::position::span_to_range;

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

        let component = doc.components().iter().find(|component| {
            component
                .span()
                .is_some_and(|span| span.start <= offset && offset <= span.end)
        })?;

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
