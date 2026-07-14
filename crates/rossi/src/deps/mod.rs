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
//! Nodes have stable [`ComponentId`]s and a kind-aware multimap from names to
//! IDs. A context and a machine that happen to share a name never collide, and
//! duplicate declarations retain separate identities until project diagnostics
//! reject them. Name-based helpers remain convenient for valid Rodin projects,
//! where component names are unique.
//!
//! [`rossi-build`]: https://docs.rs/rossi-build
//! [`eventb-lsp`]: https://docs.rs/eventb-lsp

use crate::Component;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

/// Whether a component is a context or a machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ComponentKind {
    Context,
    Machine,
}

/// Stable identity of one component node in a [`DependencyGraph`].
///
/// IDs are never reused within a graph, including after a node is removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ComponentId(usize);

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

/// A machine's uniquely-resolved REFINES ancestors, nearest parent first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefinementAncestry {
    /// Resolved ancestor node IDs, nearest parent first.
    pub components: Vec<ComponentId>,
    /// Whether the walk reached a root machine.
    ///
    /// This is `false` when a parent is missing, duplicated, or circular.
    pub complete: bool,
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

#[derive(Debug, Clone)]
enum GraphNode {
    Context { name: String, data: ContextNode },
    Machine { name: String, data: MachineNode },
}

impl GraphNode {
    fn kind(&self) -> ComponentKind {
        match self {
            Self::Context { .. } => ComponentKind::Context,
            Self::Machine { .. } => ComponentKind::Machine,
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Context { name, .. } | Self::Machine { name, .. } => name,
        }
    }
}

/// A directed graph of Event-B components and their SEES / REFINES / EXTENDS
/// dependencies.
#[derive(Debug, Clone, Default)]
pub struct DependencyGraph {
    nodes: Vec<Option<GraphNode>>,
    contexts: HashMap<String, Vec<ComponentId>>,
    machines: HashMap<String, Vec<ComponentId>>,
}

impl DependencyGraph {
    /// Create an empty graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a graph from an iterator of components, preserving every item as
    /// a distinct node when names are duplicated.
    pub fn from_components<'a, I>(components: I) -> Self
    where
        I: IntoIterator<Item = &'a Component>,
    {
        let mut graph = Self::new();
        for component in components {
            graph.insert_component(component);
        }
        graph
    }

    // --- Construction / incremental update ---

    /// Insert a parsed component as a distinct node and return its stable ID.
    ///
    /// Unlike [`Self::upsert_component`], this preserves duplicate names.
    pub fn insert_component(&mut self, component: &Component) -> ComponentId {
        match component {
            Component::Context(ctx) => self.insert_context(&ctx.name, ctx.extends.clone()),
            Component::Machine(mch) => {
                self.insert_machine(&mch.name, mch.refines.clone(), mch.sees.clone())
            }
        }
    }

    /// Insert a distinct context node and return its stable ID.
    pub fn insert_context(&mut self, name: &str, extends: Vec<String>) -> ComponentId {
        self.insert_node(GraphNode::Context {
            name: name.to_string(),
            data: ContextNode { extends },
        })
    }

    /// Insert a distinct machine node and return its stable ID.
    pub fn insert_machine(
        &mut self,
        name: &str,
        refines: Option<String>,
        sees: Vec<String>,
    ) -> ComponentId {
        self.insert_node(GraphNode::Machine {
            name: name.to_string(),
            data: MachineNode { refines, sees },
        })
    }

    fn insert_node(&mut self, node: GraphNode) -> ComponentId {
        let id = ComponentId(self.nodes.len());
        let kind = node.kind();
        let name = node.name().to_string();
        self.nodes.push(Some(node));
        self.name_index_mut(kind).entry(name).or_default().push(id);
        id
    }

    /// Insert or replace the most recently inserted node for a parsed
    /// component, retaining its stable ID.
    pub fn upsert_component(&mut self, component: &Component) {
        match component {
            Component::Context(ctx) => self.upsert_context(&ctx.name, ctx.extends.clone()),
            Component::Machine(mch) => {
                self.upsert_machine(&mch.name, mch.refines.clone(), mch.sees.clone())
            }
        }
    }

    /// Insert or replace the most recently inserted context node and its
    /// EXTENDS parents, retaining its stable ID.
    pub fn upsert_context(&mut self, name: &str, extends: Vec<String>) {
        if let Some(id) = self.selected_id(ComponentKind::Context, name) {
            self.nodes[id.0] = Some(GraphNode::Context {
                name: name.to_string(),
                data: ContextNode { extends },
            });
        } else {
            self.insert_context(name, extends);
        }
    }

    /// Insert or replace the most recently inserted machine node, its REFINES
    /// parent and SEES targets, retaining its stable ID.
    pub fn upsert_machine(&mut self, name: &str, refines: Option<String>, sees: Vec<String>) {
        if let Some(id) = self.selected_id(ComponentKind::Machine, name) {
            self.nodes[id.0] = Some(GraphNode::Machine {
                name: name.to_string(),
                data: MachineNode { refines, sees },
            });
        } else {
            self.insert_machine(name, refines, sees);
        }
    }

    /// Remove every node of the given kind and name.
    pub fn remove(&mut self, kind: ComponentKind, name: &str) {
        if let Some(ids) = self.name_index_mut(kind).remove(name) {
            for id in ids {
                self.nodes[id.0] = None;
            }
        }
    }

    /// Copy one node and its direct edges from `source`, replacing any local
    /// node of the same kind and name. Returns whether the source node exists.
    pub fn copy_node_from(
        &mut self,
        source: &DependencyGraph,
        kind: ComponentKind,
        name: &str,
    ) -> bool {
        match source.selected_node(kind, name) {
            Some(GraphNode::Context { data, .. }) => {
                self.upsert_context(name, data.extends.clone());
                true
            }
            Some(GraphNode::Machine { data, .. }) => {
                self.upsert_machine(name, data.refines.clone(), data.sees.clone());
                true
            }
            None => false,
        }
    }

    // --- Inspection ---

    fn name_index(&self, kind: ComponentKind) -> &HashMap<String, Vec<ComponentId>> {
        match kind {
            ComponentKind::Context => &self.contexts,
            ComponentKind::Machine => &self.machines,
        }
    }

    fn name_index_mut(&mut self, kind: ComponentKind) -> &mut HashMap<String, Vec<ComponentId>> {
        match kind {
            ComponentKind::Context => &mut self.contexts,
            ComponentKind::Machine => &mut self.machines,
        }
    }

    fn selected_id(&self, kind: ComponentKind, name: &str) -> Option<ComponentId> {
        self.components_named(kind, name).last().copied()
    }

    fn selected_node(&self, kind: ComponentKind, name: &str) -> Option<&GraphNode> {
        self.selected_id(kind, name)
            .and_then(|id| self.nodes.get(id.0)?.as_ref())
    }

    fn context_node(&self, name: &str) -> Option<&ContextNode> {
        match self.selected_node(ComponentKind::Context, name)? {
            GraphNode::Context { data, .. } => Some(data),
            GraphNode::Machine { .. } => None,
        }
    }

    fn machine_node(&self, name: &str) -> Option<&MachineNode> {
        match self.selected_node(ComponentKind::Machine, name)? {
            GraphNode::Machine { data, .. } => Some(data),
            GraphNode::Context { .. } => None,
        }
    }

    fn machine_node_by_id(&self, id: ComponentId) -> Option<&MachineNode> {
        match self.nodes.get(id.0)?.as_ref()? {
            GraphNode::Machine { data, .. } => Some(data),
            GraphNode::Context { .. } => None,
        }
    }

    /// Inspect a node by stable ID, returning its kind and name.
    pub fn component(&self, id: ComponentId) -> Option<(ComponentKind, &str)> {
        let node = self.nodes.get(id.0)?.as_ref()?;
        Some((node.kind(), node.name()))
    }

    /// All stable IDs declared with the given kind and name, in insertion
    /// order.
    pub fn components_named(&self, kind: ComponentKind, name: &str) -> &[ComponentId] {
        self.name_index(kind)
            .get(name)
            .map_or(&[][..], Vec::as_slice)
    }

    /// Whether a node of the given kind exists.
    pub fn contains(&self, kind: ComponentKind, name: &str) -> bool {
        !self.components_named(kind, name).is_empty()
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
                let node = self.context_node(name)?;
                if !node.extends.is_empty() {
                    refs.insert(EdgeKind::Extends, node.extends.clone());
                }
            }
            ComponentKind::Machine => {
                let node = self.machine_node(name)?;
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
                .context_node(name)
                .map_or(&[][..], |n| n.extends.as_slice()),
            EdgeKind::Sees => self
                .machine_node(name)
                .map_or(&[][..], |n| n.sees.as_slice()),
            EdgeKind::Refines => self
                .machine_node(name)
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
                    && let Some(n) = self.context_node(name)
                {
                    for target in &n.extends {
                        edges.push((EdgeKind::Extends, (ComponentKind::Context, target.as_str())));
                    }
                }
            }
            ComponentKind::Machine => {
                if let Some(n) = self.machine_node(name) {
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

    /// Uniquely-resolved REFINES ancestors of a machine node.
    ///
    /// A missing or duplicated parent name, or a cycle, returns the resolved
    /// prefix with [`RefinementAncestry::complete`] set to `false`.
    pub fn refinement_ancestry(&self, machine: ComponentId) -> RefinementAncestry {
        let Some(mut current) = self.machine_node_by_id(machine) else {
            return RefinementAncestry {
                components: Vec::new(),
                complete: false,
            };
        };
        let mut components = Vec::new();
        let mut visited = HashSet::from([machine]);
        loop {
            let Some(parent_name) = current.refines.as_deref() else {
                return RefinementAncestry {
                    components,
                    complete: true,
                };
            };
            let [parent] = self.components_named(ComponentKind::Machine, parent_name) else {
                return RefinementAncestry {
                    components,
                    complete: false,
                };
            };
            if !visited.insert(*parent) {
                return RefinementAncestry {
                    components,
                    complete: false,
                };
            }
            components.push(*parent);
            current = self
                .machine_node_by_id(*parent)
                .expect("machine name index contains only machine nodes");
        }
    }

    /// All machines that transitively REFINE each requested machine, by stable
    /// ID and in request order.
    ///
    /// A child whose parent name has duplicate declarations attaches to every
    /// candidate parent. This conservative reverse lookup lets callers retain
    /// duplicate-name diagnostics without dropping descendant relationships.
    ///
    /// The duplicate-aware reverse index is built once for the whole batch.
    pub fn refinement_descendants(&self, machines: &[ComponentId]) -> Vec<BTreeSet<ComponentId>> {
        if machines.is_empty() {
            return Vec::new();
        }

        let mut children: HashMap<ComponentId, Vec<ComponentId>> = HashMap::new();
        for (index, node) in self.nodes.iter().enumerate() {
            let Some(GraphNode::Machine { data, .. }) = node else {
                continue;
            };
            let Some(parent_name) = data.refines.as_deref() else {
                continue;
            };
            let child = ComponentId(index);
            for parent in self.components_named(ComponentKind::Machine, parent_name) {
                children.entry(*parent).or_default().push(child);
            }
        }

        machines
            .iter()
            .map(|&machine| self.refinement_descendants_from(machine, &children))
            .collect()
    }

    fn refinement_descendants_from(
        &self,
        machine: ComponentId,
        children: &HashMap<ComponentId, Vec<ComponentId>>,
    ) -> BTreeSet<ComponentId> {
        if self.machine_node_by_id(machine).is_none() {
            return BTreeSet::new();
        }
        let mut descendants = BTreeSet::new();
        let mut visited = HashSet::from([machine]);
        let mut stack = children.get(&machine).cloned().unwrap_or_default();
        while let Some(node) = stack.pop() {
            if !visited.insert(node) {
                continue;
            }
            descendants.insert(node);
            if let Some(next) = children.get(&node) {
                stack.extend(next.iter().copied());
            }
        }
        descendants
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
            if let Some(node) = self.machine_node(mch) {
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
        if let Some(node) = self.context_node(context) {
            for parent in &node.extends {
                self.push_context_and_parents(parent, &mut contexts, &mut seen);
            }
        }
        contexts
    }

    fn push_context_and_parents<'a>(
        &'a self,
        context: &'a str,
        contexts: &mut Vec<String>,
        seen: &mut HashSet<String>,
    ) {
        let mut stack = vec![context];
        while let Some(current) = stack.pop() {
            if !seen.insert(current.to_string()) {
                continue;
            }
            contexts.push(current.to_string());
            if let Some(node) = self.context_node(current) {
                for parent in node.extends.iter().rev() {
                    stack.push(parent);
                }
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
        if let Some(node) = self.context_node(name) {
            targets.extend(node.extends.iter().cloned());
        }
        if let Some(node) = self.machine_node(name) {
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
        for name in self.contexts.keys() {
            if want(EdgeKind::Extends)
                && self
                    .context_node(name)
                    .is_some_and(|node| node.extends.iter().any(|t| t == target))
            {
                result.push((ComponentKind::Context, name.clone()));
            }
        }
        for name in self.machines.keys() {
            let Some(node) = self.machine_node(name) else {
                continue;
            };
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
    fn duplicate_names_keep_distinct_stable_ids() {
        let first = crate::parse("MACHINE parent\nEND\n").unwrap();
        let second = crate::parse("MACHINE parent\nEND\n").unwrap();
        let child = crate::parse("MACHINE child\nREFINES parent\nEND\n").unwrap();
        let graph = DependencyGraph::from_components([&first, &second, &child]);

        let parents = graph.components_named(ComponentKind::Machine, "parent");
        assert_eq!(parents.len(), 2);
        assert_ne!(parents[0], parents[1]);
        assert_eq!(
            graph.component(parents[0]),
            Some((ComponentKind::Machine, "parent"))
        );

        let child = graph.components_named(ComponentKind::Machine, "child")[0];
        assert_eq!(
            graph.refinement_ancestry(child),
            RefinementAncestry {
                components: Vec::new(),
                complete: false,
            }
        );
    }

    #[test]
    fn upsert_preserves_ids_and_removed_ids_are_not_reused() {
        let mut graph = DependencyGraph::new();
        let original = graph.insert_context("ctx", vec!["old".to_string()]);

        graph.upsert_context("ctx", vec!["new".to_string()]);
        assert_eq!(
            graph.components_named(ComponentKind::Context, "ctx"),
            &[original]
        );
        assert_eq!(graph.ordered_extends_chain("ctx"), ["new"]);

        graph.remove(ComponentKind::Context, "ctx");
        assert_eq!(graph.component(original), None);
        let replacement = graph.insert_context("ctx", Vec::new());
        assert_ne!(replacement, original);
    }

    #[test]
    fn refinement_identity_queries_preserve_duplicate_parent_semantics() {
        let first = crate::parse("MACHINE parent\nEND\n").unwrap();
        let second = crate::parse("MACHINE parent\nEND\n").unwrap();
        let child = crate::parse("MACHINE child\nREFINES parent\nEND\n").unwrap();
        let grandchild = crate::parse("MACHINE grandchild\nREFINES child\nEND\n").unwrap();
        let graph = DependencyGraph::from_components([&first, &second, &child, &grandchild]);

        let parents = graph.components_named(ComponentKind::Machine, "parent");
        let child = graph.components_named(ComponentKind::Machine, "child")[0];
        let grandchild = graph.components_named(ComponentKind::Machine, "grandchild")[0];
        assert_eq!(
            graph.refinement_ancestry(grandchild),
            RefinementAncestry {
                components: vec![child],
                complete: false,
            }
        );

        assert_eq!(
            graph.refinement_descendants(parents),
            vec![
                [child, grandchild].into_iter().collect(),
                [child, grandchild].into_iter().collect(),
            ]
        );
        assert!(graph.refinement_descendants(&[]).is_empty());
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
    fn ordered_extends_chain_handles_deep_graphs_iteratively() {
        let mut graph = DependencyGraph::new();
        for i in 0..10_000 {
            graph.upsert_context(&format!("c{i}"), vec![format!("c{}", i + 1)]);
        }
        graph.upsert_context("c10000", vec![]);

        let chain = graph.ordered_extends_chain("c0");
        assert_eq!(chain.len(), 10_000);
        assert_eq!(chain.first().map(String::as_str), Some("c1"));
        assert_eq!(chain.last().map(String::as_str), Some("c10000"));
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
