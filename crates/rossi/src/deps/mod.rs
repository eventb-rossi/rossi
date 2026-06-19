//! Cross-component dependency graph for Event-B (SEES / REFINES / EXTENDS).
//!
//! This is the single source of truth for the inter-component visibility
//! semantics shared by the static checker ([`rossi-build`]) and the language
//! server ([`eventb-lsp`]):
//!
//! - **topological ordering** ([`DependencyGraph::topological_order`]) — drives
//!   the build, ensuring parents are checked before children, and aborts on a
//!   cycle;
//! - **cycle detection** ([`DependencyGraph::detect_cycles`]) — surfaces
//!   circular EXTENDS / REFINES (/ SEES) chains as diagnostics;
//! - **reachability & visibility** ([`DependencyGraph::transitive_closure`],
//!   [`DependencyGraph::ordered_visible_contexts`], …) — powers cross-file
//!   navigation, reference finding, and renaming.
//!
//! Nodes are keyed by `(ComponentKind, name)`. Keying by kind (rather than name
//! alone) means a context and a machine that happen to share a name never
//! collide, and cross-kind `SEES` edges resolve unambiguously because every
//! [`EdgeKind`] knows its [`EdgeKind::source_kind`] / [`EdgeKind::target_kind`].
//! Valid Rodin projects use project-unique component names, so the name-based
//! query helpers used by the language server resolve to a single node.
//!
//! [`rossi-build`]: https://docs.rs/rossi-build
//! [`eventb-lsp`]: https://docs.rs/eventb-lsp

use crate::Component;
use std::collections::{HashMap, HashSet, VecDeque};

/// Whether a component is a context or a machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ComponentKind {
    Context,
    Machine,
}

/// A directed cross-component dependency edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EdgeKind {
    /// Context EXTENDS context.
    Extends,
    /// Machine SEES context.
    Sees,
    /// Machine REFINES machine.
    Refines,
}

impl EdgeKind {
    /// The kind of component an edge of this kind originates from.
    pub fn source_kind(self) -> ComponentKind {
        match self {
            EdgeKind::Extends => ComponentKind::Context,
            EdgeKind::Sees | EdgeKind::Refines => ComponentKind::Machine,
        }
    }

    /// The kind of component an edge of this kind points to.
    pub fn target_kind(self) -> ComponentKind {
        match self {
            EdgeKind::Extends | EdgeKind::Sees => ComponentKind::Context,
            EdgeKind::Refines => ComponentKind::Machine,
        }
    }
}

/// The kind and name of a parsed [`Component`] — the canonical classification
/// used by every consumer of this graph.
pub fn kind_and_name(component: &Component) -> (ComponentKind, String) {
    match component {
        Component::Context(ctx) => (ComponentKind::Context, ctx.name.clone()),
        Component::Machine(mch) => (ComponentKind::Machine, mch.name.clone()),
    }
}

/// A detected dependency cycle, normalized for stable reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cycle {
    /// The edge kind whose traversal closed the cycle.
    pub kind: EdgeKind,
    /// Component names in cycle order, implicitly closed (last → first),
    /// rotated so the lexicographically smallest name appears first.
    pub components: Vec<String>,
}

/// DFS progress for cycle detection. A node is `Active` while on the current
/// DFS stack and `Done` once fully explored; an unvisited node is simply absent
/// from the map (the classic "white" state).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Visit {
    Active,
    Done,
}

#[derive(Debug, Clone)]
struct ContextNode {
    /// EXTENDS parents (other contexts).
    extends: Vec<String>,
}

#[derive(Debug, Clone)]
struct MachineNode {
    /// REFINES parent (a machine; at most one).
    refines: Option<String>,
    /// SEES targets (contexts).
    sees: Vec<String>,
}

/// A directed graph of Event-B components and their SEES / REFINES / EXTENDS
/// dependencies.
#[derive(Debug, Clone, Default)]
pub struct DependencyGraph {
    contexts: HashMap<String, ContextNode>,
    machines: HashMap<String, MachineNode>,
}

impl DependencyGraph {
    /// Create an empty graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a graph from an iterator of components.
    pub fn from_components<'a, I>(components: I) -> Self
    where
        I: IntoIterator<Item = &'a Component>,
    {
        let mut graph = Self::new();
        for component in components {
            graph.upsert_component(component);
        }
        graph
    }

    // --- Construction / incremental update ---

    /// Insert or replace the node for a parsed component.
    pub fn upsert_component(&mut self, component: &Component) {
        match component {
            Component::Context(ctx) => self.upsert_context(&ctx.name, ctx.extends.clone()),
            Component::Machine(mch) => {
                self.upsert_machine(&mch.name, mch.refines.clone(), mch.sees.clone())
            }
        }
    }

    /// Insert or replace a context node and its EXTENDS parents.
    pub fn upsert_context(&mut self, name: &str, extends: Vec<String>) {
        self.contexts
            .insert(name.to_string(), ContextNode { extends });
    }

    /// Insert or replace a machine node, its REFINES parent and SEES targets.
    pub fn upsert_machine(&mut self, name: &str, refines: Option<String>, sees: Vec<String>) {
        self.machines
            .insert(name.to_string(), MachineNode { refines, sees });
    }

    /// Remove a node of the given kind.
    pub fn remove(&mut self, kind: ComponentKind, name: &str) {
        match kind {
            ComponentKind::Context => {
                self.contexts.remove(name);
            }
            ComponentKind::Machine => {
                self.machines.remove(name);
            }
        }
    }

    // --- Inspection ---

    /// Whether a node of the given kind exists.
    pub fn contains(&self, kind: ComponentKind, name: &str) -> bool {
        match kind {
            ComponentKind::Context => self.contexts.contains_key(name),
            ComponentKind::Machine => self.machines.contains_key(name),
        }
    }

    /// The kind of the node with the given name, if any. Prefers a context
    /// when a context and machine improbably share a name.
    pub fn kind_of(&self, name: &str) -> Option<ComponentKind> {
        if self.contexts.contains_key(name) {
            Some(ComponentKind::Context)
        } else if self.machines.contains_key(name) {
            Some(ComponentKind::Machine)
        } else {
            None
        }
    }

    /// All component names in the graph (contexts then machines).
    pub fn component_names(&self) -> Vec<String> {
        self.contexts
            .keys()
            .chain(self.machines.keys())
            .cloned()
            .collect()
    }

    /// The outgoing references of a node, grouped by edge kind, together with
    /// the node's kind. Returns `None` if no such node exists.
    pub fn references_of(
        &self,
        name: &str,
    ) -> Option<(ComponentKind, HashMap<EdgeKind, Vec<String>>)> {
        let kind = self.kind_of(name)?;
        let references = self.references_of_kind(kind, name)?;
        Some((kind, references))
    }

    /// The outgoing references of a specific node, grouped by edge kind.
    pub fn references_of_kind(
        &self,
        kind: ComponentKind,
        name: &str,
    ) -> Option<HashMap<EdgeKind, Vec<String>>> {
        let mut refs = HashMap::new();
        match kind {
            ComponentKind::Context => {
                let node = self.contexts.get(name)?;
                if !node.extends.is_empty() {
                    refs.insert(EdgeKind::Extends, node.extends.clone());
                }
            }
            ComponentKind::Machine => {
                let node = self.machines.get(name)?;
                if !node.sees.is_empty() {
                    refs.insert(EdgeKind::Sees, node.sees.clone());
                }
                if let Some(parent) = &node.refines {
                    refs.insert(EdgeKind::Refines, vec![parent.clone()]);
                }
            }
        }
        Some(refs)
    }

    /// The direct targets of `name`'s edges of a single kind. Empty if the node
    /// does not exist or has no such edges.
    fn out_edges(&self, name: &str, edge: EdgeKind) -> &[String] {
        match edge {
            EdgeKind::Extends => self
                .contexts
                .get(name)
                .map_or(&[][..], |n| n.extends.as_slice()),
            EdgeKind::Sees => self
                .machines
                .get(name)
                .map_or(&[][..], |n| n.sees.as_slice()),
            EdgeKind::Refines => self
                .machines
                .get(name)
                .map_or(&[][..], |n| n.refines.as_slice()),
        }
    }

    /// Names of all nodes of a given kind.
    fn names_of(&self, kind: ComponentKind) -> Vec<&str> {
        match kind {
            ComponentKind::Context => self.contexts.keys().map(String::as_str).collect(),
            ComponentKind::Machine => self.machines.keys().map(String::as_str).collect(),
        }
    }

    /// Owned names of all nodes of a given kind (clones only that kind's keys).
    pub fn component_names_of_kind(&self, kind: ComponentKind) -> Vec<String> {
        self.names_of(kind).into_iter().map(String::from).collect()
    }

    // --- Topological order (build) ---

    /// Topologically order the nodes reachable through a single edge kind,
    /// parents first. Only nodes of `edge.source_kind()` participate (EXTENDS
    /// over contexts, REFINES over machines).
    ///
    /// Ordering is deterministic: independent components are visited in
    /// lexicographic order. Returns `Err(cycle)` if a cycle exists.
    pub fn topological_order(&self, edge: EdgeKind) -> Result<Vec<String>, Cycle> {
        let mut seeds = self.names_of(edge.source_kind());
        seeds.sort_unstable();

        let mut order = Vec::new();
        let mut visited: HashSet<&str> = HashSet::new();
        let mut in_stack: HashSet<&str> = HashSet::new();
        let mut stack: Vec<&str> = Vec::new();
        for seed in seeds {
            self.topo_visit(
                edge,
                seed,
                &mut visited,
                &mut in_stack,
                &mut stack,
                &mut order,
            )?;
        }
        Ok(order)
    }

    fn topo_visit<'g>(
        &'g self,
        edge: EdgeKind,
        name: &'g str,
        visited: &mut HashSet<&'g str>,
        in_stack: &mut HashSet<&'g str>,
        stack: &mut Vec<&'g str>,
        order: &mut Vec<String>,
    ) -> Result<(), Cycle> {
        if visited.contains(name) {
            return Ok(());
        }
        if in_stack.contains(name) {
            let pos = stack.iter().position(|n| *n == name).unwrap_or(0);
            let path: Vec<String> = stack[pos..].iter().map(|s| s.to_string()).collect();
            return Err(Cycle {
                kind: edge,
                components: normalize_cycle(path),
            });
        }
        in_stack.insert(name);
        stack.push(name);
        for parent in self.out_edges(name, edge) {
            // Unknown parents (referenced but absent) are silently skipped;
            // they are surfaced elsewhere as "unknown reference" diagnostics.
            if self.contains(edge.source_kind(), parent) {
                self.topo_visit(edge, parent, visited, in_stack, stack, order)?;
            }
        }
        stack.pop();
        in_stack.remove(name);
        visited.insert(name);
        order.push(name.to_string());
        Ok(())
    }

    // --- Cycle detection (diagnostics) ---

    /// Detect dependency cycles.
    ///
    /// If `edge` is `Some(k)`, only edges of kind `k` are followed. If `edge`
    /// is `None`, all edges are followed and the kind recorded in each result
    /// is that of the edge that closed the cycle. Cycles are normalized (the
    /// lexicographically smallest name first) and deduplicated by
    /// `(kind, members)`.
    pub fn detect_cycles(&self, edge: Option<EdgeKind>) -> Vec<Cycle> {
        let mut nodes: Vec<(ComponentKind, &str)> = Vec::new();
        nodes.extend(
            self.contexts
                .keys()
                .map(|n| (ComponentKind::Context, n.as_str())),
        );
        nodes.extend(
            self.machines
                .keys()
                .map(|n| (ComponentKind::Machine, n.as_str())),
        );
        nodes.sort_unstable();

        let mut state: HashMap<(ComponentKind, &str), Visit> = HashMap::new();
        let mut path: Vec<(ComponentKind, &str)> = Vec::new();
        let mut raw: Vec<(EdgeKind, Vec<String>)> = Vec::new();
        for node in nodes {
            if !state.contains_key(&node) {
                self.cycle_visit(node, edge, &mut state, &mut path, &mut raw);
            }
        }

        let mut seen: HashSet<(EdgeKind, Vec<String>)> = HashSet::new();
        let mut result = Vec::new();
        for (kind, members) in raw {
            let normalized = normalize_cycle(members);
            if seen.insert((kind, normalized.clone())) {
                result.push(Cycle {
                    kind,
                    components: normalized,
                });
            }
        }
        result
    }

    fn cycle_visit<'g>(
        &'g self,
        node: (ComponentKind, &'g str),
        filter: Option<EdgeKind>,
        state: &mut HashMap<(ComponentKind, &'g str), Visit>,
        path: &mut Vec<(ComponentKind, &'g str)>,
        cycles: &mut Vec<(EdgeKind, Vec<String>)>,
    ) {
        state.insert(node, Visit::Active);
        path.push(node);
        for (edge, target) in self.cycle_out_edges(node, filter) {
            match state.get(&target).copied() {
                Some(Visit::Active) => {
                    if let Some(pos) = path.iter().position(|n| *n == target) {
                        let members = path[pos..].iter().map(|(_, n)| n.to_string()).collect();
                        cycles.push((edge, members));
                    }
                }
                Some(Visit::Done) => {}
                None => self.cycle_visit(target, filter, state, path, cycles),
            }
        }
        path.pop();
        state.insert(node, Visit::Done);
    }

    /// All outgoing edges of a node as `(edge kind, (target kind, target name))`,
    /// optionally filtered to a single edge kind.
    fn cycle_out_edges<'g>(
        &'g self,
        node: (ComponentKind, &'g str),
        filter: Option<EdgeKind>,
    ) -> Vec<(EdgeKind, (ComponentKind, &'g str))> {
        let (kind, name) = node;
        let mut edges = Vec::new();
        let want = |e: EdgeKind| filter.is_none() || filter == Some(e);
        match kind {
            ComponentKind::Context => {
                if want(EdgeKind::Extends)
                    && let Some(n) = self.contexts.get(name)
                {
                    for target in &n.extends {
                        edges.push((EdgeKind::Extends, (ComponentKind::Context, target.as_str())));
                    }
                }
            }
            ComponentKind::Machine => {
                if let Some(n) = self.machines.get(name) {
                    if want(EdgeKind::Sees) {
                        for target in &n.sees {
                            edges.push((EdgeKind::Sees, (ComponentKind::Context, target.as_str())));
                        }
                    }
                    if want(EdgeKind::Refines)
                        && let Some(parent) = &n.refines
                    {
                        edges.push((EdgeKind::Refines, (ComponentKind::Machine, parent.as_str())));
                    }
                }
            }
        }
        edges
    }

    // --- Reachability & visibility (navigation) ---

    /// Component names reachable from `start` via edges of a single kind
    /// (excluding `start`). Cycle-safe via a visited set; referenced-but-absent
    /// targets are included but not traversed.
    pub fn transitive_closure(&self, start: &str, edge: EdgeKind) -> Vec<String> {
        let mut visited = HashSet::new();
        visited.insert(start.to_string());
        let mut stack = vec![start.to_string()];
        let mut result = Vec::new();
        while let Some(current) = stack.pop() {
            for target in self.out_edges(&current, edge) {
                if visited.insert(target.clone()) {
                    result.push(target.clone());
                    stack.push(target.clone());
                }
            }
        }
        result
    }

    /// Transitive REFINES parents of a machine.
    pub fn refinement_chain(&self, machine: &str) -> Vec<String> {
        self.transitive_closure(machine, EdgeKind::Refines)
    }

    /// Transitive EXTENDS parents of a context.
    pub fn extends_chain(&self, context: &str) -> Vec<String> {
        self.transitive_closure(context, EdgeKind::Extends)
    }

    /// Contexts visible to a machine, in deterministic depth-first pre-order.
    ///
    /// The machine and its REFINES chain are visited in order; within each,
    /// SEES targets are visited in declaration order; each seen context is
    /// emitted immediately before its transitive EXTENDS parents. Duplicates
    /// are dropped (first occurrence wins).
    pub fn ordered_visible_contexts(&self, machine: &str) -> Vec<String> {
        let mut machines = vec![machine.to_string()];
        machines.extend(self.refinement_chain(machine));

        let mut contexts = Vec::new();
        let mut seen = HashSet::new();
        for mch in &machines {
            if let Some(node) = self.machines.get(mch) {
                for ctx in &node.sees {
                    self.push_context_and_parents(ctx, &mut contexts, &mut seen);
                }
            }
        }
        contexts
    }

    /// A context's transitive EXTENDS parents in depth-first pre-order, deduped.
    /// The starting context itself is not included.
    pub fn ordered_extends_chain(&self, context: &str) -> Vec<String> {
        let mut contexts = Vec::new();
        let mut seen = HashSet::new();
        if let Some(node) = self.contexts.get(context) {
            for parent in &node.extends {
                self.push_context_and_parents(parent, &mut contexts, &mut seen);
            }
        }
        contexts
    }

    fn push_context_and_parents(
        &self,
        context: &str,
        contexts: &mut Vec<String>,
        seen: &mut HashSet<String>,
    ) {
        if !seen.insert(context.to_string()) {
            return;
        }
        contexts.push(context.to_string());
        if let Some(node) = self.contexts.get(context) {
            for parent in &node.extends {
                self.push_context_and_parents(parent, contexts, seen);
            }
        }
    }

    /// All components reachable from `start` via any edge kind (BFS), excluding
    /// `start`.
    pub fn all_reachable(&self, start: &str) -> HashSet<String> {
        let mut visited = HashSet::new();
        visited.insert(start.to_string());
        let mut queue = VecDeque::new();
        queue.push_back(start.to_string());
        let mut result = HashSet::new();
        while let Some(current) = queue.pop_front() {
            for target in self.all_out_targets(&current) {
                if visited.insert(target.clone()) {
                    result.insert(target.clone());
                    queue.push_back(target);
                }
            }
        }
        result
    }

    fn all_out_targets(&self, name: &str) -> Vec<String> {
        let mut targets = Vec::new();
        if let Some(node) = self.contexts.get(name) {
            targets.extend(node.extends.iter().cloned());
        }
        if let Some(node) = self.machines.get(name) {
            targets.extend(node.sees.iter().cloned());
            if let Some(parent) = &node.refines {
                targets.push(parent.clone());
            }
        }
        targets
    }

    /// All components that directly reference `target`, optionally restricted to
    /// a single edge kind. Each referrer appears once; results are sorted by
    /// `(kind, name)` for determinism.
    pub fn referencing(
        &self,
        target: &str,
        edge: Option<EdgeKind>,
    ) -> Vec<(ComponentKind, String)> {
        let want = |e: EdgeKind| edge.is_none() || edge == Some(e);
        let mut result = Vec::new();
        for (name, node) in &self.contexts {
            if want(EdgeKind::Extends) && node.extends.iter().any(|t| t == target) {
                result.push((ComponentKind::Context, name.clone()));
            }
        }
        for (name, node) in &self.machines {
            let sees_hit = want(EdgeKind::Sees) && node.sees.iter().any(|t| t == target);
            let refines_hit = want(EdgeKind::Refines) && node.refines.as_deref() == Some(target);
            if sees_hit || refines_hit {
                result.push((ComponentKind::Machine, name.clone()));
            }
        }
        result.sort();
        result
    }
}

/// Rotate a cycle so the lexicographically smallest element is first.
fn normalize_cycle(cycle: Vec<String>) -> Vec<String> {
    if cycle.is_empty() {
        return cycle;
    }
    let min_pos = cycle
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| a.cmp(b))
        .map(|(i, _)| i)
        .unwrap_or(0);
    let mut normalized = Vec::with_capacity(cycle.len());
    normalized.extend_from_slice(&cycle[min_pos..]);
    normalized.extend_from_slice(&cycle[..min_pos]);
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(graph: &mut DependencyGraph, name: &str, extends: &[&str]) {
        graph.upsert_context(name, extends.iter().map(|s| s.to_string()).collect());
    }

    fn mch(graph: &mut DependencyGraph, name: &str, refines: Option<&str>, sees: &[&str]) {
        graph.upsert_machine(
            name,
            refines.map(str::to_string),
            sees.iter().map(|s| s.to_string()).collect(),
        );
    }

    #[test]
    fn edge_kind_endpoints() {
        assert_eq!(EdgeKind::Extends.source_kind(), ComponentKind::Context);
        assert_eq!(EdgeKind::Extends.target_kind(), ComponentKind::Context);
        assert_eq!(EdgeKind::Sees.source_kind(), ComponentKind::Machine);
        assert_eq!(EdgeKind::Sees.target_kind(), ComponentKind::Context);
        assert_eq!(EdgeKind::Refines.source_kind(), ComponentKind::Machine);
        assert_eq!(EdgeKind::Refines.target_kind(), ComponentKind::Machine);
    }

    #[test]
    fn from_components_indexes_edges() {
        let derived = crate::parse("CONTEXT derived\nEXTENDS base\nEND\n").unwrap();
        let machine = crate::parse("MACHINE m\nREFINES m0\nSEES derived\nEND\n").unwrap();
        let graph = DependencyGraph::from_components([&derived, &machine]);

        assert_eq!(graph.kind_of("derived"), Some(ComponentKind::Context));
        assert_eq!(graph.kind_of("m"), Some(ComponentKind::Machine));
        let (_, refs) = graph.references_of("m").unwrap();
        assert_eq!(
            refs.get(&EdgeKind::Refines).unwrap(),
            &vec!["m0".to_string()]
        );
        assert_eq!(
            refs.get(&EdgeKind::Sees).unwrap(),
            &vec!["derived".to_string()]
        );
    }

    #[test]
    fn topological_order_parents_first() {
        let mut graph = DependencyGraph::new();
        ctx(&mut graph, "a", &["b"]);
        ctx(&mut graph, "b", &["c"]);
        ctx(&mut graph, "c", &[]);

        let order = graph.topological_order(EdgeKind::Extends).unwrap();
        let pos = |n: &str| order.iter().position(|x| x == n).unwrap();
        assert!(pos("c") < pos("b"));
        assert!(pos("b") < pos("a"));
    }

    #[test]
    fn topological_order_is_deterministic() {
        let mut graph = DependencyGraph::new();
        ctx(&mut graph, "z", &[]);
        ctx(&mut graph, "a", &[]);
        ctx(&mut graph, "m", &[]);
        // Independent nodes are ordered lexicographically.
        assert_eq!(
            graph.topological_order(EdgeKind::Extends).unwrap(),
            vec!["a".to_string(), "m".to_string(), "z".to_string()]
        );
    }

    #[test]
    fn topological_order_ignores_unknown_parents() {
        let mut graph = DependencyGraph::new();
        ctx(&mut graph, "a", &["missing"]);
        assert_eq!(
            graph.topological_order(EdgeKind::Extends).unwrap(),
            vec!["a".to_string()]
        );
    }

    #[test]
    fn topological_order_reports_cycle() {
        let mut graph = DependencyGraph::new();
        ctx(&mut graph, "a", &["b"]);
        ctx(&mut graph, "b", &["a"]);
        let cycle = graph.topological_order(EdgeKind::Extends).unwrap_err();
        assert_eq!(cycle.kind, EdgeKind::Extends);
        assert_eq!(cycle.components, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn topological_order_machines_by_refines() {
        let mut graph = DependencyGraph::new();
        mch(&mut graph, "m2", Some("m1"), &[]);
        mch(&mut graph, "m1", Some("m0"), &[]);
        mch(&mut graph, "m0", None, &[]);
        let order = graph.topological_order(EdgeKind::Refines).unwrap();
        assert_eq!(
            order,
            vec!["m0".to_string(), "m1".to_string(), "m2".to_string()]
        );
    }

    #[test]
    fn transitive_closure_diamond_dedup() {
        let mut graph = DependencyGraph::new();
        ctx(&mut graph, "a", &["b", "c"]);
        ctx(&mut graph, "b", &["d"]);
        ctx(&mut graph, "c", &["d"]);
        ctx(&mut graph, "d", &[]);
        let mut closure = graph.transitive_closure("a", EdgeKind::Extends);
        closure.sort();
        assert_eq!(closure, vec!["b", "c", "d"]);
    }

    #[test]
    fn transitive_closure_includes_missing_target_but_stops() {
        let mut graph = DependencyGraph::new();
        ctx(&mut graph, "a", &["b"]); // b absent
        assert_eq!(
            graph.transitive_closure("a", EdgeKind::Extends),
            vec!["b".to_string()]
        );
    }

    #[test]
    fn transitive_closure_terminates_on_cycle() {
        let mut graph = DependencyGraph::new();
        ctx(&mut graph, "a", &["b"]);
        ctx(&mut graph, "b", &["a"]);
        assert_eq!(
            graph.transitive_closure("a", EdgeKind::Extends),
            vec!["b".to_string()]
        );
    }

    #[test]
    fn transitive_closure_no_arbitrary_cap() {
        // A chain longer than the old MAX_TRAVERSAL_DEPTH (20) must not truncate.
        let mut graph = DependencyGraph::new();
        for i in 0..50 {
            let parent = format!("c{}", i + 1);
            graph.upsert_context(&format!("c{i}"), vec![parent]);
        }
        graph.upsert_context("c50", vec![]);
        assert_eq!(graph.transitive_closure("c0", EdgeKind::Extends).len(), 50);
    }

    #[test]
    fn ordered_visible_contexts_refines_sees_extends() {
        let mut graph = DependencyGraph::new();
        ctx(&mut graph, "base", &[]);
        ctx(&mut graph, "derived", &["base"]);
        ctx(&mut graph, "extra", &[]);
        mch(&mut graph, "m0", None, &["derived"]);
        mch(&mut graph, "m1", Some("m0"), &["extra"]);
        mch(&mut graph, "m2", Some("m1"), &[]);

        // m2 -> (m2 sees none) -> m1 sees extra -> m0 sees derived -> base.
        assert_eq!(
            graph.ordered_visible_contexts("m2"),
            vec![
                "extra".to_string(),
                "derived".to_string(),
                "base".to_string()
            ]
        );
    }

    #[test]
    fn ordered_extends_chain_preorder() {
        let mut graph = DependencyGraph::new();
        ctx(&mut graph, "a", &["b"]);
        ctx(&mut graph, "b", &["c"]);
        ctx(&mut graph, "c", &[]);
        assert_eq!(
            graph.ordered_extends_chain("a"),
            vec!["b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn all_reachable_mixed_edges() {
        let mut graph = DependencyGraph::new();
        ctx(&mut graph, "ctx", &[]);
        mch(&mut graph, "m_a", None, &["ctx"]);
        mch(&mut graph, "m_b", Some("m_a"), &[]);
        let reachable = graph.all_reachable("m_b");
        assert!(reachable.contains("m_a"));
        assert!(reachable.contains("ctx"));
        assert!(!reachable.contains("m_b"));
    }

    #[test]
    fn detect_cycles_two_node_extends() {
        let mut graph = DependencyGraph::new();
        ctx(&mut graph, "ctx1", &["ctx2"]);
        ctx(&mut graph, "ctx2", &["ctx1"]);
        let cycles = graph.detect_cycles(None);
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].kind, EdgeKind::Extends);
        assert_eq!(
            cycles[0].components,
            vec!["ctx1".to_string(), "ctx2".to_string()]
        );
    }

    #[test]
    fn detect_cycles_filter_by_kind() {
        let mut graph = DependencyGraph::new();
        ctx(&mut graph, "ctx1", &["ctx2"]);
        ctx(&mut graph, "ctx2", &["ctx1"]);
        assert!(graph.detect_cycles(Some(EdgeKind::Refines)).is_empty());
        assert_eq!(graph.detect_cycles(Some(EdgeKind::Extends)).len(), 1);
    }

    #[test]
    fn detect_cycles_self_loop() {
        let mut graph = DependencyGraph::new();
        ctx(&mut graph, "x", &["x"]);
        let cycles = graph.detect_cycles(None);
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].components, vec!["x".to_string()]);
    }

    #[test]
    fn detect_cycles_multiple_independent() {
        let mut graph = DependencyGraph::new();
        ctx(&mut graph, "ctx1", &["ctx2"]);
        ctx(&mut graph, "ctx2", &["ctx1"]);
        mch(&mut graph, "m1", Some("m2"), &[]);
        mch(&mut graph, "m2", Some("m1"), &[]);
        assert_eq!(graph.detect_cycles(None).len(), 2);
    }

    #[test]
    fn detect_cycles_none_when_acyclic() {
        let mut graph = DependencyGraph::new();
        ctx(&mut graph, "a", &["b"]);
        ctx(&mut graph, "b", &["c"]);
        ctx(&mut graph, "c", &[]);
        assert!(graph.detect_cycles(None).is_empty());
    }

    #[test]
    fn referencing_reverse_lookup() {
        let mut graph = DependencyGraph::new();
        ctx(&mut graph, "ctx", &[]);
        mch(&mut graph, "m1", None, &["ctx"]);
        mch(&mut graph, "m2", None, &["ctx"]);
        let referrers = graph.referencing("ctx", Some(EdgeKind::Sees));
        assert_eq!(
            referrers,
            vec![
                (ComponentKind::Machine, "m1".to_string()),
                (ComponentKind::Machine, "m2".to_string()),
            ]
        );
    }

    #[test]
    fn referencing_counts_each_node_once() {
        // A machine that both sees and refines the same name (degenerate) is
        // reported once under the "any kind" query.
        let mut graph = DependencyGraph::new();
        mch(&mut graph, "m", Some("shared"), &["shared"]);
        assert_eq!(
            graph.referencing("shared", None),
            vec![(ComponentKind::Machine, "m".to_string())]
        );
    }

    #[test]
    fn remove_drops_node() {
        let mut graph = DependencyGraph::new();
        ctx(&mut graph, "a", &[]);
        assert!(graph.contains(ComponentKind::Context, "a"));
        graph.remove(ComponentKind::Context, "a");
        assert!(!graph.contains(ComponentKind::Context, "a"));
    }

    #[test]
    fn context_and_machine_may_share_a_name() {
        let mut graph = DependencyGraph::new();
        ctx(&mut graph, "shared", &[]);
        mch(&mut graph, "shared", None, &[]);
        assert!(graph.contains(ComponentKind::Context, "shared"));
        assert!(graph.contains(ComponentKind::Machine, "shared"));
        graph.remove(ComponentKind::Context, "shared");
        assert!(!graph.contains(ComponentKind::Context, "shared"));
        assert!(graph.contains(ComponentKind::Machine, "shared"));
    }

    #[test]
    fn normalize_cycle_rotates_to_smallest() {
        assert_eq!(
            normalize_cycle(vec!["c".into(), "a".into(), "b".into()]),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert!(normalize_cycle(vec![]).is_empty());
        assert_eq!(normalize_cycle(vec!["x".into()]), vec!["x".to_string()]);
    }
}
