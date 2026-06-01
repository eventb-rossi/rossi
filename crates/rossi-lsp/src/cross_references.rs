//! Cross-file reference tracking for Event-B workspaces
//!
//! This module manages workspace-wide dependencies between Event-B files,
//! tracking SEES, REFINES, and EXTENDS relationships to enable cross-file
//! navigation, renaming, and reference finding.

use crate::lsp_types::Url;
use dashmap::DashMap;
use parking_lot::RwLock;
use rossi::{Component, parse};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, warn};

use crate::document::DocumentManager;

/// Maximum depth for transitive traversal
const MAX_TRAVERSAL_DEPTH: usize = 20;

/// A detected dependency cycle
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyCycle {
    /// The reference kind forming this cycle
    pub kind: ReferenceKind,
    /// Component names in traversal order (implicitly closed: last → first).
    /// Normalized: lexicographically smallest element first.
    pub components: Vec<String>,
}

/// DFS coloring for cycle detection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Color {
    White,
    Gray,
    Black,
}

/// Type of cross-file reference
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReferenceKind {
    /// Machine SEES context
    Sees,
    /// Machine REFINES machine
    Refines,
    /// Context EXTENDS context
    Extends,
}

/// Information about a component (context or machine) in the workspace
#[derive(Debug, Clone)]
pub struct ComponentInfo {
    /// URI of the file containing this component
    pub uri: String,
    /// Name of the component
    pub name: String,
    /// Type of component
    pub kind: ComponentKind,
    /// Components this one references (SEES, REFINES, or EXTENDS)
    pub references: HashMap<ReferenceKind, Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentKind {
    Context,
    Machine,
}

/// Workspace-wide cross-reference manager
pub struct CrossReferenceManager {
    /// Map from component name to component info
    /// Key: component name (e.g., "counter_ctx", "counter_machine")
    /// Value: information about that component
    components: Arc<DashMap<String, ComponentInfo>>,

    /// Map from URI to component name
    /// Key: file URI
    /// Value: name of the component in that file
    uri_to_component: Arc<DashMap<String, String>>,

    /// Workspace root path (if available)
    workspace_root: RwLock<Option<PathBuf>>,
}

impl Default for CrossReferenceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl CrossReferenceManager {
    /// Create a new cross-reference manager
    pub fn new() -> Self {
        Self {
            components: Arc::new(DashMap::new()),
            uri_to_component: Arc::new(DashMap::new()),
            workspace_root: RwLock::new(None),
        }
    }

    /// Set the workspace root directory
    pub fn set_workspace_root(&self, root: PathBuf) {
        debug!("Setting workspace root: {:?}", root);
        *self.workspace_root.write() = Some(root);
    }

    /// Get the workspace root directory
    pub fn workspace_root(&self) -> Option<PathBuf> {
        self.workspace_root.read().clone()
    }

    /// Update or add a component from a document
    pub fn update_component(&self, uri: String, text: &str) {
        debug!("Updating component for URI: {}", uri);

        // Parse the component
        let component = match parse(text) {
            Ok(comp) => comp,
            Err(e) => {
                debug!("Failed to parse component for cross-references: {}", e);
                // Remove old component if parsing fails
                if let Some(old_name) = self.uri_to_component.remove(&uri) {
                    self.components.remove(&old_name.1);
                }
                return;
            }
        };

        // Extract component info
        let info = self.extract_component_info(&component, &uri);

        // Remove old mapping if component name changed
        if let Some(old_entry) = self.uri_to_component.get(&uri)
            && old_entry.value() != &info.name
        {
            self.components.remove(old_entry.value());
        }

        // Update mappings
        self.uri_to_component.insert(uri, info.name.clone());
        self.components.insert(info.name.clone(), info);
    }

    /// Remove a component when its file is deleted
    #[allow(dead_code)]
    pub fn remove_component(&self, uri: &str) {
        debug!("Removing component for URI: {}", uri);

        if let Some((_uri, name)) = self.uri_to_component.remove(uri) {
            self.components.remove(&name);
        }
    }

    /// Find the URI of a component by its name
    ///
    /// This searches for contexts and machines by name and returns the file URI
    /// where that component is defined.
    pub fn find_component_uri(&self, component_name: &str) -> Option<String> {
        self.components
            .get(component_name)
            .map(|info| info.uri.clone())
    }

    /// Get component info by name
    pub fn get_component(&self, name: &str) -> Option<ComponentInfo> {
        self.components.get(name).map(|info| info.clone())
    }

    /// Get component name from URI
    #[allow(dead_code)]
    pub fn get_component_name(&self, uri: &str) -> Option<String> {
        self.uri_to_component.get(uri).map(|name| name.clone())
    }

    /// Find all components that reference a given component
    ///
    /// For example, find all machines that SEE a context, or all machines that
    /// REFINE a given abstract machine.
    #[allow(dead_code)]
    pub fn find_referencing_components(
        &self,
        target_name: &str,
        reference_kind: Option<ReferenceKind>,
    ) -> Vec<ComponentInfo> {
        let mut result = Vec::new();

        for entry in self.components.iter() {
            let info = entry.value();

            // Check if this component references the target
            for (kind, refs) in &info.references {
                if (reference_kind.is_none() || reference_kind == Some(*kind))
                    && refs.contains(&target_name.to_string())
                {
                    result.push(info.clone());
                    break;
                }
            }
        }

        result
    }

    /// Scan a directory for Event-B files and index them
    pub fn scan_workspace(&self, root_path: &Path) -> std::io::Result<usize> {
        debug!("Scanning workspace at: {:?}", root_path);

        let mut count = 0;

        // Recursively find all Event-B source files
        for entry in walkdir::WalkDir::new(root_path)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if matches!(path.extension().and_then(|s| s.to_str()), Some("eventb")) {
                // Convert path to URI
                if let Ok(uri) = Url::from_file_path(path) {
                    // Read and index the file
                    if let Ok(content) = std::fs::read_to_string(path) {
                        self.update_component(uri.to_string(), &content);
                        count += 1;
                    }
                }
            }
        }

        debug!("Scanned {} Event-B files in workspace", count);
        Ok(count)
    }

    /// Get all component names in the workspace
    pub fn all_component_names(&self) -> Vec<String> {
        self.components
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Load the source text of a component by name.
    /// Tries open documents first (via DocumentManager), falls back to disk.
    pub fn load_component_text(
        &self,
        component_name: &str,
        document_manager: Option<&DocumentManager>,
    ) -> Option<String> {
        let uri_str = self.find_component_uri(component_name)?;
        if let Some(dm) = document_manager
            && let Ok(uri) = Url::parse(&uri_str)
            && let Some(text) = dm.get_text(&uri)
        {
            return Some(text);
        }
        if let Ok(uri) = Url::parse(&uri_str)
            && let Ok(path) = uri.to_file_path()
        {
            return std::fs::read_to_string(path).ok();
        }
        None
    }

    /// Get all component URIs in the workspace
    pub fn all_component_uris(&self) -> Vec<String> {
        self.uri_to_component
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    // --- Transitive closure methods ---

    /// Compute the transitive closure of a single reference kind starting from `start`.
    ///
    /// Returns component names reachable from `start` (excluding `start` itself)
    /// via edges of the given `kind`. Uses iterative DFS, capped at
    /// `MAX_TRAVERSAL_DEPTH` results.
    pub fn transitive_closure(&self, start: &str, kind: ReferenceKind) -> Vec<String> {
        let mut visited = HashSet::new();
        visited.insert(start.to_string());
        let mut stack = vec![start.to_string()];
        let mut result = Vec::new();

        'outer: while let Some(current) = stack.pop() {
            if let Some(info) = self.components.get(&current)
                && let Some(refs) = info.references.get(&kind)
            {
                for ref_name in refs {
                    if visited.insert(ref_name.clone()) {
                        result.push(ref_name.clone());
                        stack.push(ref_name.clone());
                        if result.len() >= MAX_TRAVERSAL_DEPTH {
                            break 'outer;
                        }
                    }
                }
            }
        }

        result
    }

    /// Return the refinement chain for a machine (transitive REFINES).
    pub fn refinement_chain(&self, machine_name: &str) -> Vec<String> {
        self.transitive_closure(machine_name, ReferenceKind::Refines)
    }

    /// Return the extends chain for a context (transitive EXTENDS).
    pub fn extends_chain(&self, context_name: &str) -> Vec<String> {
        self.transitive_closure(context_name, ReferenceKind::Extends)
    }

    /// Return all contexts visible to a machine.
    ///
    /// A context is visible if:
    /// - The machine (or any machine in its refinement chain) directly SEES it, or
    /// - It is transitively extended by any such seen context.
    ///
    /// The result is deduplicated but unordered; callers that need a stable
    /// order should use [`ordered_visible_contexts`](Self::ordered_visible_contexts),
    /// which this delegates to.
    pub fn visible_contexts(&self, machine_name: &str) -> Vec<String> {
        self.ordered_visible_contexts(machine_name)
    }

    /// Contexts visible to a machine, in deterministic depth-first pre-order.
    ///
    /// Like [`visible_contexts`](Self::visible_contexts) but order-preserving:
    /// the machine and its refinement chain are visited in order; within each,
    /// SEES targets are visited in declaration order; each seen context is
    /// emitted immediately before its transitive EXTENDS parents. Duplicates are
    /// dropped (first occurrence wins).
    pub fn ordered_visible_contexts(&self, machine_name: &str) -> Vec<String> {
        let mut machines = vec![machine_name.to_string()];
        machines.extend(self.refinement_chain(machine_name));

        let mut contexts = Vec::new();
        let mut seen = HashSet::new();
        for mch in &machines {
            let sees = self
                .components
                .get(mch)
                .and_then(|info| info.references.get(&ReferenceKind::Sees).cloned());
            if let Some(sees) = sees {
                for ctx in &sees {
                    self.push_context_and_parents(ctx, &mut contexts, &mut seen);
                }
            }
        }

        contexts
    }

    /// A context's transitive EXTENDS parents in depth-first pre-order, deduped.
    ///
    /// The starting context itself is not included (only its ancestors).
    pub fn ordered_extends_chain(&self, context_name: &str) -> Vec<String> {
        let mut contexts = Vec::new();
        let mut seen = HashSet::new();
        let parents = self
            .components
            .get(context_name)
            .and_then(|info| info.references.get(&ReferenceKind::Extends).cloned());
        if let Some(parents) = parents {
            for parent in &parents {
                self.push_context_and_parents(parent, &mut contexts, &mut seen);
            }
        }

        contexts
    }

    /// Append `context_name` then its transitive EXTENDS parents (pre-order),
    /// skipping any already in `seen`.
    fn push_context_and_parents(
        &self,
        context_name: &str,
        contexts: &mut Vec<String>,
        seen: &mut HashSet<String>,
    ) {
        if !seen.insert(context_name.to_string()) {
            return;
        }
        contexts.push(context_name.to_string());

        let parents = self
            .components
            .get(context_name)
            .and_then(|info| info.references.get(&ReferenceKind::Extends).cloned());
        if let Some(parents) = parents {
            for parent in &parents {
                self.push_context_and_parents(parent, contexts, seen);
            }
        }
    }

    /// Return all components reachable from `start` via any reference kind (BFS).
    ///
    /// Excludes `start` itself.
    #[allow(dead_code)]
    pub fn all_reachable(&self, start: &str) -> HashSet<String> {
        let mut visited = HashSet::new();
        visited.insert(start.to_string());
        let mut queue = VecDeque::new();
        queue.push_back(start.to_string());
        let mut result = HashSet::new();

        while let Some(current) = queue.pop_front() {
            if let Some(info) = self.components.get(&current) {
                for refs in info.references.values() {
                    for ref_name in refs {
                        if visited.insert(ref_name.clone()) {
                            result.insert(ref_name.clone());
                            queue.push_back(ref_name.clone());
                        }
                    }
                }
            }
        }

        result
    }

    // --- Cycle detection ---

    /// Detect dependency cycles in the workspace.
    ///
    /// If `kind` is `Some(k)`, only edges of kind `k` are followed.
    /// If `kind` is `None`, all edges are followed (the kind recorded in
    /// the result is the kind of the edge that closed the cycle).
    ///
    /// Cycles are normalized so the lexicographically smallest component
    /// name appears first, and deduplicated.
    pub fn detect_cycles(&self, kind: Option<ReferenceKind>) -> Vec<DependencyCycle> {
        let mut colors: HashMap<String, Color> = HashMap::new();
        for entry in self.components.iter() {
            colors.insert(entry.key().clone(), Color::White);
        }

        let mut raw_cycles: Vec<(ReferenceKind, Vec<String>)> = Vec::new();

        let component_names: Vec<String> =
            self.components.iter().map(|e| e.key().clone()).collect();
        for name in &component_names {
            if colors.get(name) == Some(&Color::White) {
                let mut path = Vec::new();
                self.dfs_cycle_detect(name, kind, &mut colors, &mut path, &mut raw_cycles);
            }
        }

        // Deduplicate using (kind, normalized components) as the key,
        // so cycles with the same nodes but different edge kinds are kept.
        let mut seen: HashSet<(ReferenceKind, Vec<String>)> = HashSet::new();
        let mut result = Vec::new();
        for (k, cycle) in raw_cycles {
            let normalized = Self::normalize_cycle(cycle);
            if seen.insert((k, normalized.clone())) {
                result.push(DependencyCycle {
                    kind: k,
                    components: normalized,
                });
            }
        }

        if !result.is_empty() {
            warn!("Detected {} dependency cycles", result.len());
        }

        result
    }

    /// Detect circular dependencies in the workspace (deprecated wrapper).
    ///
    /// Returns a list of dependency cycles as plain `Vec<String>`.
    /// Prefer [`Self::detect_cycles`] for new code.
    #[allow(dead_code)]
    #[deprecated(note = "Use detect_cycles(None) instead")]
    pub fn detect_circular_dependencies(&self) -> Vec<Vec<String>> {
        self.detect_cycles(None)
            .into_iter()
            .map(|c| c.components)
            .collect()
    }

    // Private helper methods

    /// Recursive DFS for cycle detection with White/Gray/Black coloring.
    fn dfs_cycle_detect(
        &self,
        current: &str,
        kind_filter: Option<ReferenceKind>,
        colors: &mut HashMap<String, Color>,
        path: &mut Vec<String>,
        cycles: &mut Vec<(ReferenceKind, Vec<String>)>,
    ) {
        colors.insert(current.to_string(), Color::Gray);
        path.push(current.to_string());

        // Snapshot the edges we need, then drop the DashMap Ref guard
        // before recursing to avoid holding read locks across the call stack.
        let edges: Vec<(ReferenceKind, Vec<String>)> = self
            .components
            .get(current)
            .map(|info| {
                let kinds = match kind_filter {
                    Some(k) => vec![k],
                    None => info.references.keys().copied().collect(),
                };
                kinds
                    .into_iter()
                    .filter_map(|k| info.references.get(&k).map(|r| (k, r.clone())))
                    .collect()
            })
            .unwrap_or_default();

        for (k, refs) in edges {
            for ref_name in &refs {
                match colors.get(ref_name.as_str()).copied() {
                    Some(Color::Gray) => {
                        // Found a cycle — extract from path
                        if let Some(pos) = path.iter().position(|name| name == ref_name) {
                            let cycle: Vec<String> = path[pos..].to_vec();
                            cycles.push((k, cycle));
                        }
                    }
                    Some(Color::White) | None => {
                        self.dfs_cycle_detect(ref_name, kind_filter, colors, path, cycles);
                    }
                    Some(Color::Black) => {}
                }
            }
        }

        path.pop();
        colors.insert(current.to_string(), Color::Black);
    }

    /// Normalize a cycle so the lexicographically smallest element is first.
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

    /// Extract component information from a parsed component
    fn extract_component_info(&self, component: &Component, uri: &str) -> ComponentInfo {
        match component {
            Component::Context(ctx) => {
                let mut references = HashMap::new();
                if !ctx.extends.is_empty() {
                    references.insert(ReferenceKind::Extends, ctx.extends.clone());
                }

                ComponentInfo {
                    uri: uri.to_string(),
                    name: ctx.name.clone(),
                    kind: ComponentKind::Context,
                    references,
                }
            }
            Component::Machine(mch) => {
                let mut references = HashMap::new();
                if !mch.sees.is_empty() {
                    references.insert(ReferenceKind::Sees, mch.sees.clone());
                }
                if let Some(ref refines) = mch.refines {
                    references.insert(ReferenceKind::Refines, vec![refines.clone()]);
                }

                ComponentInfo {
                    uri: uri.to_string(),
                    name: mch.name.clone(),
                    kind: ComponentKind::Machine,
                    references,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cross_reference_manager_creation() {
        let manager = CrossReferenceManager::new();
        assert!(manager.all_component_names().is_empty());
    }

    #[test]
    fn test_update_context() {
        let manager = CrossReferenceManager::new();

        let source = r#"
CONTEXT base_ctx
SETS
    STATUS
END
"#;

        manager.update_component("file:///base_ctx.eventb".to_string(), source);

        assert_eq!(manager.all_component_names().len(), 1);
        assert!(manager.find_component_uri("base_ctx").is_some());
    }

    #[test]
    fn test_scan_workspace_indexes_eventb_files_only() {
        let root = std::env::temp_dir().join(format!(
            "rossi-lsp-scan-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("eventb_ctx.eventb"), "CONTEXT eventb_ctx\nEND\n").unwrap();
        std::fs::write(root.join("rossi_ctx.rossi"), "CONTEXT rossi_ctx\nEND\n").unwrap();
        std::fs::write(root.join("ignored.txt"), "CONTEXT ignored\nEND\n").unwrap();

        let manager = CrossReferenceManager::new();
        let count = manager.scan_workspace(&root).unwrap();

        std::fs::remove_dir_all(root).unwrap();

        assert_eq!(count, 1);
        assert!(manager.find_component_uri("eventb_ctx").is_some());
        assert!(manager.find_component_uri("rossi_ctx").is_none());
        assert!(manager.find_component_uri("ignored").is_none());
    }

    #[test]
    fn test_update_context_with_extends() {
        let manager = CrossReferenceManager::new();

        let base = r#"
CONTEXT base_ctx
SETS
    STATUS
END
"#;

        let derived = r#"
CONTEXT derived_ctx
EXTENDS base_ctx
CONSTANTS
    max_val
END
"#;

        manager.update_component("file:///base_ctx.eventb".to_string(), base);
        manager.update_component("file:///derived_ctx.eventb".to_string(), derived);

        let derived_info = manager.get_component("derived_ctx").unwrap();
        assert_eq!(derived_info.kind, ComponentKind::Context);
        assert!(
            derived_info
                .references
                .contains_key(&ReferenceKind::Extends)
        );
        assert_eq!(
            derived_info
                .references
                .get(&ReferenceKind::Extends)
                .unwrap(),
            &vec!["base_ctx".to_string()]
        );
    }

    #[test]
    fn test_update_machine_with_sees() {
        let manager = CrossReferenceManager::new();

        let context = r#"
CONTEXT ctx
CONSTANTS
    max_val
END
"#;

        let machine = r#"
MACHINE mch
SEES ctx
VARIABLES
    count
END
"#;

        manager.update_component("file:///ctx.eventb".to_string(), context);
        manager.update_component("file:///mch.eventb".to_string(), machine);

        let mch_info = manager.get_component("mch").unwrap();
        assert_eq!(mch_info.kind, ComponentKind::Machine);
        assert!(mch_info.references.contains_key(&ReferenceKind::Sees));
        assert_eq!(
            mch_info.references.get(&ReferenceKind::Sees).unwrap(),
            &vec!["ctx".to_string()]
        );
    }

    #[test]
    fn test_update_machine_with_refines() {
        let manager = CrossReferenceManager::new();

        let abstract_mch = r#"
MACHINE abstract_mch
VARIABLES
    state
END
"#;

        let concrete_mch = r#"
MACHINE concrete_mch
REFINES abstract_mch
VARIABLES
    state
    detail
END
"#;

        manager.update_component("file:///abstract_mch.eventb".to_string(), abstract_mch);
        manager.update_component("file:///concrete_mch.eventb".to_string(), concrete_mch);

        let concrete_info = manager.get_component("concrete_mch").unwrap();
        assert_eq!(concrete_info.kind, ComponentKind::Machine);
        assert!(
            concrete_info
                .references
                .contains_key(&ReferenceKind::Refines)
        );
        assert_eq!(
            concrete_info
                .references
                .get(&ReferenceKind::Refines)
                .unwrap(),
            &vec!["abstract_mch".to_string()]
        );
    }

    #[test]
    fn test_find_referencing_components() {
        let manager = CrossReferenceManager::new();

        let context = r#"
CONTEXT ctx
CONSTANTS
    max_val
END
"#;

        let machine1 = r#"
MACHINE mch1
SEES ctx
VARIABLES
    count
END
"#;

        let machine2 = r#"
MACHINE mch2
SEES ctx
VARIABLES
    value
END
"#;

        manager.update_component("file:///ctx.eventb".to_string(), context);
        manager.update_component("file:///mch1.eventb".to_string(), machine1);
        manager.update_component("file:///mch2.eventb".to_string(), machine2);

        // Find all machines that SEE ctx
        let referencing = manager.find_referencing_components("ctx", Some(ReferenceKind::Sees));
        assert_eq!(referencing.len(), 2);

        let names: Vec<_> = referencing.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"mch1"));
        assert!(names.contains(&"mch2"));
    }

    #[test]
    fn test_remove_component() {
        let manager = CrossReferenceManager::new();

        let source = r#"
CONTEXT test_ctx
END
"#;

        let uri = "file:///test_ctx.eventb".to_string();
        manager.update_component(uri.clone(), source);

        assert_eq!(manager.all_component_names().len(), 1);

        manager.remove_component(&uri);

        assert_eq!(manager.all_component_names().len(), 0);
        assert!(manager.find_component_uri("test_ctx").is_none());
    }

    #[test]
    fn test_circular_dependency_detection() {
        let manager = CrossReferenceManager::new();

        // Create a circular dependency: ctx1 extends ctx2, ctx2 extends ctx1
        let ctx1 = r#"
CONTEXT ctx1
EXTENDS ctx2
END
"#;

        let ctx2 = r#"
CONTEXT ctx2
EXTENDS ctx1
END
"#;

        manager.update_component("file:///ctx1.eventb".to_string(), ctx1);
        manager.update_component("file:///ctx2.eventb".to_string(), ctx2);

        #[allow(deprecated)]
        let cycles = manager.detect_circular_dependencies();
        assert!(!cycles.is_empty());
        // Should detect the cycle between ctx1 and ctx2
        assert!(cycles.iter().any(
            |cycle| cycle.contains(&"ctx1".to_string()) && cycle.contains(&"ctx2".to_string())
        ));
    }

    #[test]
    fn test_set_workspace_root_through_arc() {
        let manager = Arc::new(CrossReferenceManager::new());
        assert!(manager.workspace_root().is_none());
        manager.set_workspace_root(PathBuf::from("/tmp/test"));
        assert_eq!(manager.workspace_root(), Some(PathBuf::from("/tmp/test")));
    }

    #[test]
    fn test_get_component_name_from_uri() {
        let manager = CrossReferenceManager::new();

        let source = r#"
CONTEXT test_ctx
END
"#;

        let uri = "file:///test_ctx.eventb".to_string();
        manager.update_component(uri.clone(), source);

        let name = manager.get_component_name(&uri);
        assert_eq!(name, Some("test_ctx".to_string()));
    }

    // --- Test helpers for direct DashMap insertion (no parsing overhead) ---

    fn register_context(manager: &CrossReferenceManager, name: &str, extends: &[&str]) {
        let mut references = HashMap::new();
        if !extends.is_empty() {
            references.insert(
                ReferenceKind::Extends,
                extends.iter().map(|s| s.to_string()).collect(),
            );
        }
        let info = ComponentInfo {
            uri: format!("file:///{name}.eventb"),
            name: name.to_string(),
            kind: ComponentKind::Context,
            references,
        };
        manager.components.insert(name.to_string(), info);
    }

    fn register_machine(
        manager: &CrossReferenceManager,
        name: &str,
        refines: &[&str],
        sees: &[&str],
    ) {
        let mut references = HashMap::new();
        if !refines.is_empty() {
            references.insert(
                ReferenceKind::Refines,
                refines.iter().map(|s| s.to_string()).collect(),
            );
        }
        if !sees.is_empty() {
            references.insert(
                ReferenceKind::Sees,
                sees.iter().map(|s| s.to_string()).collect(),
            );
        }
        let info = ComponentInfo {
            uri: format!("file:///{name}.eventb"),
            name: name.to_string(),
            kind: ComponentKind::Machine,
            references,
        };
        manager.components.insert(name.to_string(), info);
    }

    // --- Transitive closure tests ---

    #[test]
    fn test_transitive_closure_simple_chain() {
        let manager = CrossReferenceManager::new();
        register_context(&manager, "ctx_a", &["ctx_b"]);
        register_context(&manager, "ctx_b", &["ctx_c"]);
        register_context(&manager, "ctx_c", &[]);

        let result = manager.transitive_closure("ctx_a", ReferenceKind::Extends);
        assert!(result.contains(&"ctx_b".to_string()));
        assert!(result.contains(&"ctx_c".to_string()));
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_transitive_closure_diamond() {
        let manager = CrossReferenceManager::new();
        // ctx_a extends both ctx_b and ctx_c; both extend ctx_d
        register_context(&manager, "ctx_a", &["ctx_b", "ctx_c"]);
        register_context(&manager, "ctx_b", &["ctx_d"]);
        register_context(&manager, "ctx_c", &["ctx_d"]);
        register_context(&manager, "ctx_d", &[]);

        let result = manager.transitive_closure("ctx_a", ReferenceKind::Extends);
        assert!(result.contains(&"ctx_b".to_string()));
        assert!(result.contains(&"ctx_c".to_string()));
        assert!(result.contains(&"ctx_d".to_string()));
        // ctx_d appears only once
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_transitive_closure_wrong_kind() {
        let manager = CrossReferenceManager::new();
        register_machine(&manager, "mch_a", &["mch_b"], &[]);
        register_machine(&manager, "mch_b", &[], &[]);

        let result = manager.transitive_closure("mch_a", ReferenceKind::Extends);
        assert!(result.is_empty());
    }

    #[test]
    fn test_transitive_closure_missing_component() {
        let manager = CrossReferenceManager::new();
        register_context(&manager, "ctx_a", &["ctx_b"]); // ctx_b not registered

        let result = manager.transitive_closure("ctx_a", ReferenceKind::Extends);
        // ctx_b is in the result (referenced) but traversal stops there
        assert_eq!(result, vec!["ctx_b".to_string()]);
    }

    #[test]
    fn test_transitive_closure_cycle_terminates() {
        let manager = CrossReferenceManager::new();
        register_context(&manager, "ctx_a", &["ctx_b"]);
        register_context(&manager, "ctx_b", &["ctx_a"]);

        let result = manager.transitive_closure("ctx_a", ReferenceKind::Extends);
        assert_eq!(result, vec!["ctx_b".to_string()]);
    }

    // --- Convenience wrapper tests ---

    #[test]
    fn test_refinement_chain() {
        let manager = CrossReferenceManager::new();
        register_machine(&manager, "mch_c", &["mch_b"], &[]);
        register_machine(&manager, "mch_b", &["mch_a"], &[]);
        register_machine(&manager, "mch_a", &[], &[]);

        let chain = manager.refinement_chain("mch_c");
        assert!(chain.contains(&"mch_b".to_string()));
        assert!(chain.contains(&"mch_a".to_string()));
        assert_eq!(chain.len(), 2);
    }

    #[test]
    fn test_extends_chain() {
        let manager = CrossReferenceManager::new();
        register_context(&manager, "ctx_c", &["ctx_b"]);
        register_context(&manager, "ctx_b", &["ctx_a"]);
        register_context(&manager, "ctx_a", &[]);

        let chain = manager.extends_chain("ctx_c");
        assert!(chain.contains(&"ctx_b".to_string()));
        assert!(chain.contains(&"ctx_a".to_string()));
        assert_eq!(chain.len(), 2);
    }

    // --- visible_contexts tests ---

    #[test]
    fn test_visible_contexts_direct_sees() {
        let manager = CrossReferenceManager::new();
        register_context(&manager, "ctx", &[]);
        register_machine(&manager, "mch", &[], &["ctx"]);

        let visible = manager.visible_contexts("mch");
        assert_eq!(visible.len(), 1);
        assert!(visible.contains(&"ctx".to_string()));
    }

    #[test]
    fn test_visible_contexts_sees_plus_extends() {
        let manager = CrossReferenceManager::new();
        register_context(&manager, "ctx_parent", &[]);
        register_context(&manager, "ctx_child", &["ctx_parent"]);
        register_machine(&manager, "mch", &[], &["ctx_child"]);

        let mut visible = manager.visible_contexts("mch");
        visible.sort();
        assert_eq!(
            visible,
            vec!["ctx_child".to_string(), "ctx_parent".to_string()]
        );
    }

    #[test]
    fn test_visible_contexts_inherited_via_refines() {
        let manager = CrossReferenceManager::new();
        register_context(&manager, "ctx", &[]);
        register_machine(&manager, "mch_abstract", &[], &["ctx"]);
        register_machine(&manager, "mch_concrete", &["mch_abstract"], &[]);

        let visible = manager.visible_contexts("mch_concrete");
        assert_eq!(visible.len(), 1);
        assert!(visible.contains(&"ctx".to_string()));
    }

    #[test]
    fn test_visible_contexts_full_chain() {
        let manager = CrossReferenceManager::new();
        // Two-level refinement + SEES + EXTENDS
        register_context(&manager, "base_ctx", &[]);
        register_context(&manager, "derived_ctx", &["base_ctx"]);
        register_context(&manager, "extra_ctx", &[]);
        register_machine(&manager, "mch0", &[], &["derived_ctx"]);
        register_machine(&manager, "mch1", &["mch0"], &["extra_ctx"]);
        register_machine(&manager, "mch2", &["mch1"], &[]);

        let mut visible = manager.visible_contexts("mch2");
        visible.sort();
        assert_eq!(
            visible,
            vec![
                "base_ctx".to_string(),
                "derived_ctx".to_string(),
                "extra_ctx".to_string(),
            ]
        );
    }

    #[test]
    fn test_visible_contexts_deduplication() {
        let manager = CrossReferenceManager::new();
        // Both mch_abstract and mch_concrete SEE the same context
        register_context(&manager, "ctx", &[]);
        register_machine(&manager, "mch_abstract", &[], &["ctx"]);
        register_machine(&manager, "mch_concrete", &["mch_abstract"], &["ctx"]);

        let visible = manager.visible_contexts("mch_concrete");
        assert_eq!(visible.len(), 1);
        assert!(visible.contains(&"ctx".to_string()));
    }

    // --- all_reachable tests ---

    #[test]
    fn test_all_reachable_mixed() {
        let manager = CrossReferenceManager::new();
        register_context(&manager, "ctx", &[]);
        register_machine(&manager, "mch_a", &[], &["ctx"]);
        register_machine(&manager, "mch_b", &["mch_a"], &[]);

        let reachable = manager.all_reachable("mch_b");
        assert!(reachable.contains("mch_a"));
        assert!(reachable.contains("ctx"));
        assert!(!reachable.contains("mch_b"));
    }

    #[test]
    fn test_all_reachable_isolated() {
        let manager = CrossReferenceManager::new();
        register_context(&manager, "lonely", &[]);

        let reachable = manager.all_reachable("lonely");
        assert!(reachable.is_empty());
    }

    // --- Cycle detection tests ---

    #[test]
    fn test_detect_cycles_simple_two_node() {
        let manager = CrossReferenceManager::new();
        register_context(&manager, "ctx1", &["ctx2"]);
        register_context(&manager, "ctx2", &["ctx1"]);

        let cycles = manager.detect_cycles(None);
        assert_eq!(cycles.len(), 1);
        assert_eq!(
            cycles[0].components,
            vec!["ctx1".to_string(), "ctx2".to_string()]
        );
        assert_eq!(cycles[0].kind, ReferenceKind::Extends);
    }

    #[test]
    fn test_detect_cycles_three_node() {
        let manager = CrossReferenceManager::new();
        register_context(&manager, "a", &["b"]);
        register_context(&manager, "b", &["c"]);
        register_context(&manager, "c", &["a"]);

        let cycles = manager.detect_cycles(Some(ReferenceKind::Extends));
        assert_eq!(cycles.len(), 1);
        assert_eq!(
            cycles[0].components,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn test_detect_cycles_filter_by_kind() {
        let manager = CrossReferenceManager::new();
        // EXTENDS cycle exists
        register_context(&manager, "ctx1", &["ctx2"]);
        register_context(&manager, "ctx2", &["ctx1"]);

        // Filtering by REFINES should find nothing
        let cycles = manager.detect_cycles(Some(ReferenceKind::Refines));
        assert!(cycles.is_empty());
    }

    #[test]
    fn test_detect_cycles_no_cycle() {
        let manager = CrossReferenceManager::new();
        register_context(&manager, "ctx_a", &["ctx_b"]);
        register_context(&manager, "ctx_b", &["ctx_c"]);
        register_context(&manager, "ctx_c", &[]);

        let cycles = manager.detect_cycles(None);
        assert!(cycles.is_empty());
    }

    #[test]
    fn test_detect_cycles_multiple_independent() {
        let manager = CrossReferenceManager::new();
        // Cycle 1: ctx1 ↔ ctx2
        register_context(&manager, "ctx1", &["ctx2"]);
        register_context(&manager, "ctx2", &["ctx1"]);
        // Cycle 2: mch1 ↔ mch2
        register_machine(&manager, "mch1", &["mch2"], &[]);
        register_machine(&manager, "mch2", &["mch1"], &[]);

        let cycles = manager.detect_cycles(None);
        assert_eq!(cycles.len(), 2);
    }

    #[test]
    fn test_detect_cycles_self_loop() {
        let manager = CrossReferenceManager::new();
        register_context(&manager, "ctx_x", &["ctx_x"]);

        let cycles = manager.detect_cycles(None);
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].components, vec!["ctx_x".to_string()]);
    }

    // --- normalize_cycle tests ---

    #[test]
    fn test_normalize_cycle() {
        let cycle = vec!["c".to_string(), "a".to_string(), "b".to_string()];
        let normalized = CrossReferenceManager::normalize_cycle(cycle);
        assert_eq!(
            normalized,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn test_normalize_cycle_empty() {
        let normalized = CrossReferenceManager::normalize_cycle(vec![]);
        assert!(normalized.is_empty());
    }

    #[test]
    fn test_normalize_cycle_single() {
        let normalized = CrossReferenceManager::normalize_cycle(vec!["x".to_string()]);
        assert_eq!(normalized, vec!["x".to_string()]);
    }
}
