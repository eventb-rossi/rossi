//! Cross-file reference tracking for Event-B workspaces
//!
//! This module manages workspace-wide dependencies between Event-B files,
//! tracking SEES, REFINES, and EXTENDS relationships to enable cross-file
//! navigation, renaming, and reference finding.
//!
//! The structural model is the shared [`rossi::deps::DependencyGraph`] — the
//! same single source of truth used by the static checker (`rossi-build`).
//! [`CrossReferenceManager`] owns one such graph plus the URI ↔ component-name
//! maps the language server needs for navigation.

use dashmap::DashMap;
use parking_lot::RwLock;
use rossi::deps::{DependencyGraph, kind_and_name};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

use crate::lsp_types::Url;

/// Canonical component / edge kinds, re-exported from the shared
/// [`rossi::deps`] dependency model so existing call sites keep referring to
/// `cross_references::{ComponentKind, ReferenceKind}`.
pub use rossi::deps::{ComponentKind, EdgeKind as ReferenceKind};

/// A detected dependency cycle (re-exported from [`rossi::deps`]).
pub use rossi::deps::Cycle as DependencyCycle;

/// Information about a component (context or machine) in the workspace.
///
/// A read-only view reconstructed on demand from the [`DependencyGraph`], which
/// is the source of truth.
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

/// The kind and name of the component defined in a given file.
#[derive(Debug, Clone)]
struct ComponentLoc {
    kind: ComponentKind,
    name: String,
}

/// Workspace-wide cross-reference manager.
///
/// The [`DependencyGraph`] is the single structural source of truth (shared
/// with `rossi-build`); the URI maps only translate between file URIs and
/// component names for navigation.
pub struct CrossReferenceManager {
    /// Structural dependency graph (SEES / REFINES / EXTENDS).
    graph: RwLock<DependencyGraph>,

    /// Map from file URI to the components defined there. Most files hold a
    /// single component, but `rossi import --merge` output concatenates
    /// several into one file.
    uri_to_component: DashMap<String, Vec<ComponentLoc>>,

    /// Map from component name to its file URI. Event-B component names are
    /// unique within a project, so every name maps to exactly one file (a
    /// file may own several names).
    name_to_uri: DashMap<String, String>,

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
            graph: RwLock::new(DependencyGraph::new()),
            uri_to_component: DashMap::new(),
            name_to_uri: DashMap::new(),
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

    /// Update or add the components defined in a document
    pub fn update_component(&self, uri: String, text: &str) {
        debug!("Updating components for URI: {}", uri);

        // Parse with error recovery (via the shared helper) so a local syntax
        // error does not tear the file out of the dependency graph:
        // SEES/REFINES/EXTENDS edges are recovered from the clause text even
        // when a predicate fails to parse.
        let components = crate::component_util::parse_all(text);
        if components.is_empty() {
            debug!("No components recovered for cross-references in {uri}");
            // Drop any previously-indexed components for this URI.
            self.remove_component(&uri);
            return;
        }

        // Component names are unique per project; within one file, keep the
        // first occurrence of a duplicated name (the maps can hold only one).
        let mut locs: Vec<ComponentLoc> = Vec::new();
        let mut kept: Vec<&rossi::Component> = Vec::new();
        for component in &components {
            let (kind, name) = kind_and_name(component);
            if locs.iter().any(|l| l.name == name) {
                warn!("Duplicate component name `{name}` in {uri}; keeping the first occurrence");
                continue;
            }
            locs.push(ComponentLoc { kind, name });
            kept.push(component);
        }

        // Snapshot the previous occupants of this URI (clone out, drop guard).
        let previous = self.uri_to_component.get(&uri).map(|r| r.value().clone());

        {
            let mut graph = self.graph.write();
            for prev in previous.iter().flatten() {
                if !locs
                    .iter()
                    .any(|l| l.kind == prev.kind && l.name == prev.name)
                {
                    graph.remove(prev.kind, &prev.name);
                }
            }
            for component in &kept {
                graph.upsert_component(component);
            }
        }

        // Drop stale name→URI entries for components renamed or removed from
        // this file (only if they still point at this file).
        for prev in previous.iter().flatten() {
            if !locs.iter().any(|l| l.name == prev.name)
                && self
                    .name_to_uri
                    .get(&prev.name)
                    .is_some_and(|u| u.value() == &uri)
            {
                self.name_to_uri.remove(&prev.name);
            }
        }

        for loc in &locs {
            self.name_to_uri.insert(loc.name.clone(), uri.clone());
        }
        self.uri_to_component.insert(uri, locs);
    }

    /// Remove a file's components when the file is deleted
    #[allow(dead_code)]
    pub fn remove_component(&self, uri: &str) {
        debug!("Removing components for URI: {}", uri);

        if let Some((_uri, locs)) = self.uri_to_component.remove(uri) {
            let mut graph = self.graph.write();
            for loc in &locs {
                graph.remove(loc.kind, &loc.name);
                if self
                    .name_to_uri
                    .get(&loc.name)
                    .is_some_and(|u| u.value().as_str() == uri)
                {
                    self.name_to_uri.remove(&loc.name);
                }
            }
        }
    }

    /// Find the URI of a component by its name
    ///
    /// This searches for contexts and machines by name and returns the file URI
    /// where that component is defined.
    pub fn find_component_uri(&self, component_name: &str) -> Option<String> {
        self.name_to_uri
            .get(component_name)
            .map(|u| u.value().clone())
    }

    /// Get component info by name
    pub fn get_component(&self, name: &str) -> Option<ComponentInfo> {
        let (kind, references) = self.graph.read().references_of(name)?;
        let uri = self
            .name_to_uri
            .get(name)
            .map(|u| u.value().clone())
            .unwrap_or_default();
        Some(ComponentInfo {
            uri,
            name: name.to_string(),
            kind,
            references,
        })
    }

    /// Get the name of the first component in a file
    #[allow(dead_code)]
    pub fn get_component_name(&self, uri: &str) -> Option<String> {
        self.uri_to_component
            .get(uri)
            .and_then(|locs| locs.value().first().map(|loc| loc.name.clone()))
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
        let graph = self.graph.read();
        graph
            .referencing(target_name, reference_kind)
            .into_iter()
            .filter_map(|(kind, name)| {
                let references = graph.references_of_kind(kind, &name)?;
                let uri = self
                    .name_to_uri
                    .get(&name)
                    .map(|u| u.value().clone())
                    .unwrap_or_default();
                Some(ComponentInfo {
                    uri,
                    name,
                    kind,
                    references,
                })
            })
            .collect()
    }

    /// Scan a directory for Event-B files and index them
    pub fn scan_workspace(&self, root_path: &Path) -> std::io::Result<usize> {
        debug!("Scanning workspace at: {:?}", root_path);

        let mut count = 0;

        // Recursively find all Event-B source files. Symlinks are followed
        // (Rodin workspaces commonly link shared model directories), so cap
        // the depth to keep linked runaway trees bounded; walkdir's loop
        // detection handles cycles.
        for entry in walkdir::WalkDir::new(root_path)
            .follow_links(true)
            .max_depth(64)
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
        self.graph.read().component_names()
    }

    /// Workspace component names of a single kind, in arbitrary order. Clones
    /// only that kind's names under a single read-lock (no reference-list
    /// cloning), unlike repeated [`Self::get_component`] calls.
    pub fn component_names_of_kind(&self, kind: ComponentKind) -> Vec<String> {
        self.graph.read().component_names_of_kind(kind)
    }

    /// Get all component URIs in the workspace
    pub fn all_component_uris(&self) -> Vec<String> {
        self.uri_to_component
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    // --- Transitive closure / visibility (delegated to the shared graph) ---

    /// Compute the transitive closure of a single reference kind starting from
    /// `start` (excluding `start`). Cycle-safe; referenced-but-absent targets
    /// are included but not traversed.
    pub fn transitive_closure(&self, start: &str, kind: ReferenceKind) -> Vec<String> {
        self.graph.read().transitive_closure(start, kind)
    }

    /// Return the refinement chain for a machine (transitive REFINES).
    pub fn refinement_chain(&self, machine_name: &str) -> Vec<String> {
        self.graph.read().refinement_chain(machine_name)
    }

    /// Return the extends chain for a context (transitive EXTENDS).
    pub fn extends_chain(&self, context_name: &str) -> Vec<String> {
        self.graph.read().extends_chain(context_name)
    }

    /// Return all contexts visible to a machine.
    ///
    /// A context is visible if the machine (or any machine in its refinement
    /// chain) directly SEES it, or it is transitively extended by any such seen
    /// context. Delegates to [`ordered_visible_contexts`](Self::ordered_visible_contexts).
    pub fn visible_contexts(&self, machine_name: &str) -> Vec<String> {
        self.ordered_visible_contexts(machine_name)
    }

    /// Contexts visible to a machine, in deterministic depth-first pre-order.
    pub fn ordered_visible_contexts(&self, machine_name: &str) -> Vec<String> {
        self.graph.read().ordered_visible_contexts(machine_name)
    }

    /// A context's transitive EXTENDS parents in depth-first pre-order, deduped.
    /// The starting context itself is not included.
    pub fn ordered_extends_chain(&self, context_name: &str) -> Vec<String> {
        self.graph.read().ordered_extends_chain(context_name)
    }

    /// Return all components reachable from `start` via any reference kind.
    /// Excludes `start` itself.
    #[allow(dead_code)]
    pub fn all_reachable(&self, start: &str) -> HashSet<String> {
        self.graph.read().all_reachable(start)
    }

    // --- Cycle detection ---

    /// Detect dependency cycles in the workspace.
    ///
    /// If `kind` is `Some(k)`, only edges of kind `k` are followed; if `None`,
    /// all edges are followed (the kind recorded is that of the edge that
    /// closed the cycle). Cycles are normalized (smallest name first) and
    /// deduplicated.
    pub fn detect_cycles(&self, kind: Option<ReferenceKind>) -> Vec<DependencyCycle> {
        let cycles = self.graph.read().detect_cycles(kind);
        if !cycles.is_empty() {
            warn!("Detected {} dependency cycles", cycles.len());
        }
        cycles
    }

    /// Detect circular dependencies in the workspace (deprecated wrapper).
    #[allow(dead_code)]
    #[deprecated(note = "Use detect_cycles(None) instead")]
    pub fn detect_circular_dependencies(&self) -> Vec<Vec<String>> {
        self.detect_cycles(None)
            .into_iter()
            .map(|c| c.components)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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
    fn test_update_multi_component_file() {
        let manager = CrossReferenceManager::new();
        let uri = "file:///merged.eventb".to_string();

        manager.update_component(
            uri.clone(),
            "CONTEXT ctx\nEND\n\nMACHINE mch\nSEES ctx\nEND\n",
        );

        assert_eq!(manager.all_component_names().len(), 2);
        assert_eq!(manager.find_component_uri("ctx"), Some(uri.clone()));
        assert_eq!(manager.find_component_uri("mch"), Some(uri.clone()));
        let mch = manager.get_component("mch").unwrap();
        assert_eq!(
            mch.references.get(&ReferenceKind::Sees).unwrap(),
            &vec!["ctx".to_string()]
        );

        // An edit that drops one component must unindex exactly that one.
        manager.update_component(uri.clone(), "CONTEXT ctx\nEND\n");
        assert_eq!(manager.all_component_names(), vec!["ctx".to_string()]);
        assert!(manager.find_component_uri("mch").is_none());

        manager.remove_component(&uri);
        assert!(manager.all_component_names().is_empty());
        assert!(manager.find_component_uri("ctx").is_none());
    }

    #[test]
    fn test_duplicate_names_in_one_file_first_wins() {
        let manager = CrossReferenceManager::new();
        let uri = "file:///dup.eventb".to_string();

        manager.update_component(
            uri.clone(),
            "MACHINE m\nVARIABLES\n    x\nEND\n\nMACHINE m\nEND\n",
        );

        assert_eq!(manager.all_component_names(), vec!["m".to_string()]);
        assert_eq!(manager.find_component_uri("m"), Some(uri));
    }

    #[test]
    fn sees_edge_survives_a_local_error() {
        // A machine with a broken invariant must still be indexed with its
        // SEES edge intact — recovery extracts the clause names even when a
        // predicate fails to parse, so cross-file navigation keeps working.
        let manager = CrossReferenceManager::new();
        let source = "CONTEXT C\nEND\n\nMACHINE M\nSEES C\nINVARIANTS\n    @i x ∈\nEND\n";
        manager.update_component("file:///model.eventb".to_string(), source);

        assert!(manager.find_component_uri("M").is_some());
        assert!(manager.find_component_uri("C").is_some());
        let m = manager.get_component("M").unwrap();
        assert_eq!(
            m.references.get(&ReferenceKind::Sees).unwrap(),
            &vec!["C".to_string()]
        );
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

    /// Regression test: a single pathological file used to overflow the
    /// stack inside `rossi::parse` and abort the whole server during the
    /// post-initialize workspace scan (originally hit via a fuzz artifact
    /// with thousands of nested parens left in /tmp).
    #[test]
    fn test_scan_workspace_survives_deeply_nested_file() {
        let root = std::env::temp_dir().join(format!(
            "rossi-lsp-deep-scan-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("good_ctx.eventb"), "CONTEXT good_ctx\nEND\n").unwrap();
        let pathological = format!(
            "context deep_ctx axioms @a {}x{} = 1 end",
            "(".repeat(5000),
            ")".repeat(5000)
        );
        std::fs::write(root.join("deep_ctx.eventb"), pathological).unwrap();

        let manager = CrossReferenceManager::new();
        let count = manager.scan_workspace(&root).unwrap();

        std::fs::remove_dir_all(root).unwrap();

        // Both files are visited; the good one is indexed, the over-deep one
        // is rejected by the parser's nesting guard instead of crashing.
        assert_eq!(count, 2);
        assert!(manager.find_component_uri("good_ctx").is_some());
        assert!(manager.find_component_uri("deep_ctx").is_none());
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

    // --- Test helpers for direct graph insertion (no parsing overhead) ---

    fn register_context(manager: &CrossReferenceManager, name: &str, extends: &[&str]) {
        manager
            .graph
            .write()
            .upsert_context(name, extends.iter().map(|s| s.to_string()).collect());
    }

    fn register_machine(
        manager: &CrossReferenceManager,
        name: &str,
        refines: &[&str],
        sees: &[&str],
    ) {
        manager.graph.write().upsert_machine(
            name,
            refines.first().map(|s| s.to_string()),
            sees.iter().map(|s| s.to_string()).collect(),
        );
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
}
