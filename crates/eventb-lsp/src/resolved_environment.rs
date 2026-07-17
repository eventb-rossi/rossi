//! Request-scoped, ordered views of a component's dependency environment.
//!
//! The workspace dependency graph is updated on the diagnostics debounce, while
//! language features must reflect the latest open-document snapshots immediately.
//! [`ResolvedEnvironment`] therefore discovers dependencies through the shared
//! [`ComponentLoader`], builds a graph from those exact parsed components, and
//! delegates ordering and cycle handling to [`DependencyGraph`].

use std::collections::{HashMap, HashSet, VecDeque};

use rossi::Component;
use rossi::deps::{ComponentKind, DependencyGraph, EdgeKind, kind_and_name};

use crate::component_loader::{ComponentLoader, LoadedComponent};

/// The loadable dependency environment of one parsed component.
///
/// The root component is not returned by the query methods. Callers already
/// hold it and combine its declarations with the inherited components below.
pub(crate) struct ResolvedEnvironment {
    root: (ComponentKind, String),
    graph: DependencyGraph,
    components: HashMap<(ComponentKind, String), LoadedComponent>,
}

impl ResolvedEnvironment {
    /// Discover every loadable dependency reachable from `root`.
    pub(crate) fn new(root: &Component, loader: &ComponentLoader) -> Self {
        Self::with_scope(root, loader, DependencyScope::All)
    }

    /// Discover only the root machine's REFINES chain.
    pub(crate) fn refinements(root: &Component, loader: &ComponentLoader) -> Self {
        Self::with_scope(root, loader, DependencyScope::Refinements)
    }

    fn with_scope(root: &Component, loader: &ComponentLoader, scope: DependencyScope) -> Self {
        #[cfg(test)]
        crate::benchmark_metrics::environment_started();
        let root_key = kind_and_name(root);
        let mut graph = DependencyGraph::new();
        graph.upsert_component(root);

        let mut pending = VecDeque::from(direct_dependencies(&graph, &root_key, scope));
        let mut seen = HashSet::from([root_key.clone()]);
        let mut components = HashMap::new();

        while let Some(expected) = pending.pop_front() {
            #[cfg(test)]
            crate::benchmark_metrics::queue_popped();
            if !seen.insert(expected.clone()) {
                continue;
            }
            #[cfg(test)]
            crate::benchmark_metrics::unique_node();

            match loader.load(&expected.1) {
                Some(loaded) if kind_and_name(loaded.component()) == expected => {
                    #[cfg(test)]
                    crate::benchmark_metrics::loaded_node();
                    graph.upsert_component(loaded.component());
                    components.insert(expected.clone(), loaded);
                }
                // Preserve indexed descendants even when this component's
                // current source is temporarily unavailable.
                _ if loader
                    .manager()
                    .copy_dependency_node(&mut graph, expected.0, &expected.1) =>
                {
                    #[cfg(test)]
                    crate::benchmark_metrics::indexed_fallback_node();
                }
                _ => {
                    #[cfg(test)]
                    crate::benchmark_metrics::unavailable_node();
                    continue;
                }
            }
            pending.extend(direct_dependencies(&graph, &expected, scope));
        }

        Self {
            root: root_key,
            graph,
            components,
        }
    }

    /// Machines inherited through the root machine's REFINES chain.
    pub(crate) fn refined_machines(&self) -> Vec<&Component> {
        if self.root.0 != ComponentKind::Machine {
            return Vec::new();
        }
        self.loaded_components(
            ComponentKind::Machine,
            self.graph.refinement_chain(&self.root.1),
        )
    }

    /// Contexts visible through the root machine's SEES/REFINES environment.
    pub(crate) fn visible_contexts(&self) -> Vec<&Component> {
        if self.root.0 != ComponentKind::Machine {
            return Vec::new();
        }
        self.loaded_components(
            ComponentKind::Context,
            self.graph.ordered_visible_contexts(&self.root.1),
        )
    }

    /// Contexts inherited through the root context's EXTENDS chain.
    pub(crate) fn extended_contexts(&self) -> Vec<&Component> {
        if self.root.0 != ComponentKind::Context {
            return Vec::new();
        }
        self.loaded_components(
            ComponentKind::Context,
            self.graph.ordered_extends_chain(&self.root.1),
        )
    }

    fn loaded_components(&self, kind: ComponentKind, names: Vec<String>) -> Vec<&Component> {
        names
            .into_iter()
            .filter_map(|name| {
                self.components
                    .get(&(kind, name))
                    .map(LoadedComponent::component)
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn benchmark_cardinality(&self) -> usize {
        std::hint::black_box(self);
        self.components.len() + 1
    }

    #[cfg(test)]
    pub(crate) fn benchmark_direct_edges(&self) -> usize {
        let mut components = self.graph.component_names();
        components.sort();
        components
            .into_iter()
            .filter_map(|name| {
                let kind = self.graph.kind_of(&name)?;
                Some(direct_dependencies(
                    &self.graph,
                    &(kind, name),
                    DependencyScope::All,
                ))
            })
            .map(|dependencies| dependencies.len())
            .sum()
    }
}

#[derive(Clone, Copy)]
enum DependencyScope {
    All,
    Refinements,
}

fn direct_dependencies(
    graph: &DependencyGraph,
    component: &(ComponentKind, String),
    scope: DependencyScope,
) -> Vec<(ComponentKind, String)> {
    let dependencies = graph
        .references_of_kind(component.0, &component.1)
        .into_iter()
        .flatten()
        .filter(|(edge, _)| matches!(scope, DependencyScope::All) || *edge == EdgeKind::Refines)
        .flat_map(|(edge, names)| {
            names
                .into_iter()
                .map(move |name| (edge.target_kind(), name))
        })
        .collect::<Vec<_>>();
    #[cfg(test)]
    crate::benchmark_metrics::direct_edges(dependencies.len());
    dependencies
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::cross_references::CrossReferenceManager;
    use crate::document::DocumentManager;
    use crate::lsp_types::{TextDocumentContentChangeEvent, Url};

    fn register(
        manager: &CrossReferenceManager,
        documents: &DocumentManager,
        uri: &str,
        source: &str,
    ) {
        manager.update_component(uri.to_string(), source);
        documents.open(Url::parse(uri).unwrap(), 1, source.to_string());
    }

    fn parse_one(source: &str) -> Component {
        crate::component_util::parse_all(source)
            .into_iter()
            .next()
            .expect("component parses")
    }

    #[test]
    fn long_extends_and_refines_chains_are_complete() {
        let manager = CrossReferenceManager::new();
        let documents = DocumentManager::new();

        for i in 1..=12 {
            let parent = (i < 12).then(|| format!("c{}", i + 1));
            let extends = parent.map_or(String::new(), |parent| format!("\nEXTENDS\n    {parent}"));
            register(
                &manager,
                &documents,
                &format!("file:///c{i}.eventb"),
                &format!("CONTEXT c{i}{extends}\nEND"),
            );
        }
        let root_context = parse_one("CONTEXT c0\nEXTENDS\n    c1\nEND");
        let loader = ComponentLoader::new(&manager, Some(&documents));
        let environment = ResolvedEnvironment::new(&root_context, &loader);
        assert_eq!(
            environment
                .extended_contexts()
                .into_iter()
                .map(Component::name)
                .collect::<Vec<_>>(),
            (1..=12).map(|i| format!("c{i}")).collect::<Vec<_>>()
        );

        for i in 1..=12 {
            let refines = if i < 12 {
                format!("\nREFINES\n    m{}", i + 1)
            } else {
                String::new()
            };
            register(
                &manager,
                &documents,
                &format!("file:///m{i}.eventb"),
                &format!("MACHINE m{i}{refines}\nEND"),
            );
        }
        let root_machine = parse_one("MACHINE m0\nREFINES\n    m1\nEND");
        let loader = ComponentLoader::new(&manager, Some(&documents));
        let environment = ResolvedEnvironment::new(&root_machine, &loader);
        assert_eq!(
            environment
                .refined_machines()
                .into_iter()
                .map(Component::name)
                .collect::<Vec<_>>(),
            (1..=12).map(|i| format!("m{i}")).collect::<Vec<_>>()
        );
    }

    #[test]
    fn mixed_seen_contexts_do_not_hide_the_refined_machine() {
        let manager = CrossReferenceManager::new();
        let documents = DocumentManager::new();
        for i in 0..10 {
            register(
                &manager,
                &documents,
                &format!("file:///c{i}.eventb"),
                &format!("CONTEXT c{i}\nEND"),
            );
        }
        register(
            &manager,
            &documents,
            "file:///abstract.eventb",
            "MACHINE abstract\nVARIABLES\n    inherited\nEND",
        );
        let root = parse_one(&format!(
            "MACHINE concrete\nREFINES\n    abstract\nSEES\n{}\nEND",
            (0..10)
                .map(|i| format!("    c{i}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
        let loader = ComponentLoader::new(&manager, Some(&documents));
        let environment = ResolvedEnvironment::new(&root, &loader);

        assert_eq!(environment.visible_contexts().len(), 10);
        assert_eq!(
            environment
                .refined_machines()
                .into_iter()
                .map(Component::name)
                .collect::<Vec<_>>(),
            ["abstract"]
        );
    }

    #[test]
    fn contexts_are_ordered_depth_first_and_cycles_are_deduplicated() {
        let manager = CrossReferenceManager::new();
        let documents = DocumentManager::new();
        register(
            &manager,
            &documents,
            "file:///b.eventb",
            "CONTEXT b\nEXTENDS\n    d\nEND",
        );
        register(
            &manager,
            &documents,
            "file:///c.eventb",
            "CONTEXT c\nEXTENDS\n    e\nEND",
        );
        register(
            &manager,
            &documents,
            "file:///d.eventb",
            "CONTEXT d\nEXTENDS\n    root\nEND",
        );
        register(&manager, &documents, "file:///e.eventb", "CONTEXT e\nEND");
        let root = parse_one("CONTEXT root\nEXTENDS\n    b\n    c\nEND");
        let loader = ComponentLoader::new(&manager, Some(&documents));
        let environment = ResolvedEnvironment::new(&root, &loader);

        assert_eq!(
            environment
                .extended_contexts()
                .into_iter()
                .map(Component::name)
                .collect::<Vec<_>>(),
            ["b", "d", "c", "e"]
        );
    }

    #[test]
    fn missing_targets_are_skipped_without_truncating_other_branches() {
        let manager = CrossReferenceManager::new();
        let documents = DocumentManager::new();
        register(
            &manager,
            &documents,
            "file:///present.eventb",
            "CONTEXT present\nEND",
        );
        let root = parse_one("CONTEXT root\nEXTENDS\n    missing\n    present\nEND");
        let loader = ComponentLoader::new(&manager, Some(&documents));
        let environment = ResolvedEnvironment::new(&root, &loader);

        assert_eq!(
            environment
                .extended_contexts()
                .into_iter()
                .map(Component::name)
                .collect::<Vec<_>>(),
            ["present"]
        );
    }

    #[test]
    fn indexed_descendants_survive_an_unloadable_intermediate_component() {
        let manager = CrossReferenceManager::new();
        let documents = DocumentManager::new();
        manager.update_component(
            "file:///missing-m1.eventb".to_string(),
            "MACHINE m1\nREFINES\n    m2\nEND",
        );
        register(&manager, &documents, "file:///m2.eventb", "MACHINE m2\nEND");
        let root = parse_one("MACHINE m0\nREFINES\n    m1\nEND");
        let loader = ComponentLoader::new(&manager, Some(&documents));
        let environment = ResolvedEnvironment::new(&root, &loader);

        assert_eq!(
            environment
                .refined_machines()
                .into_iter()
                .map(Component::name)
                .collect::<Vec<_>>(),
            ["m2"]
        );
    }

    #[test]
    fn current_document_edges_override_the_debounced_workspace_graph() {
        let manager = CrossReferenceManager::new();
        let documents = DocumentManager::new();
        register(&manager, &documents, "file:///c.eventb", "CONTEXT c\nEND");
        register(&manager, &documents, "file:///m.eventb", "MACHINE m\nEND");

        let uri = Url::parse("file:///m.eventb").unwrap();
        documents.change(
            &uri,
            2,
            vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "MACHINE m\nSEES\n    c\nEND".to_string(),
            }],
        );
        assert!(manager.ordered_visible_contexts("m").is_empty());

        let current = documents.parse_result(&uri).unwrap();
        let loader = ComponentLoader::new(&manager, Some(&documents));
        let environment = ResolvedEnvironment::new(&current.components()[0], &loader);
        assert_eq!(
            environment
                .visible_contexts()
                .into_iter()
                .map(Component::name)
                .collect::<Vec<_>>(),
            ["c"]
        );
    }

    #[test]
    fn current_transitive_edges_override_the_debounced_workspace_graph() {
        let manager = CrossReferenceManager::new();
        let documents = DocumentManager::new();
        register(
            &manager,
            &documents,
            "file:///old.eventb",
            "CONTEXT old\nEND",
        );
        register(
            &manager,
            &documents,
            "file:///new.eventb",
            "CONTEXT new\nEND",
        );
        register(
            &manager,
            &documents,
            "file:///mid.eventb",
            "CONTEXT mid\nEXTENDS\n    old\nEND",
        );
        let mid_uri = Url::parse("file:///mid.eventb").unwrap();
        documents.change(
            &mid_uri,
            2,
            vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "CONTEXT mid\nEXTENDS\n    new\nEND".to_string(),
            }],
        );
        assert_eq!(manager.ordered_extends_chain("mid"), ["old"]);

        let root = parse_one("CONTEXT root\nEXTENDS\n    mid\nEND");
        let loader = ComponentLoader::new(&manager, Some(&documents));
        let environment = ResolvedEnvironment::new(&root, &loader);
        assert_eq!(
            environment
                .extended_contexts()
                .into_iter()
                .map(Component::name)
                .collect::<Vec<_>>(),
            ["mid", "new"]
        );
    }
}
