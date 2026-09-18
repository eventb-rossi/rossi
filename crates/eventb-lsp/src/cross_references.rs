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
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// A component defined in a given file, with the edges its graph node is
/// built from. Carrying the edges — rather than just the kind and name — is
/// what lets a name declared by two files survive the removal of one of them:
/// the surviving declarer's node is rebuilt from memory, with no re-read and
/// no re-parse.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ComponentLoc {
    name: String,
    edges: ComponentEdges,
}

/// The components a file declares on disk, and any overlay for the buffer the
/// editor currently holds open for it. The same shape the workspace symbol
/// index uses, and for the same reason: an open buffer answers for its file
/// wholesale, and closing it must reveal what is saved without re-reading.
///
/// `open: Some(vec![])` is meaningful — a buffer that parses to nothing
/// deliberately shadows the saved components rather than letting them leak
/// back while the document is invalid.
#[derive(Debug, Default, Clone)]
struct DocumentComponents {
    disk: Vec<ComponentLoc>,
    open: Option<Vec<ComponentLoc>>,
}

impl DocumentComponents {
    fn effective(&self) -> &[ComponentLoc] {
        self.open.as_deref().unwrap_or(&self.disk)
    }

    fn is_empty(&self) -> bool {
        self.disk.is_empty() && self.open.is_none()
    }
}

/// The outgoing edges of a component, mirroring the two shapes
/// [`DependencyGraph::upsert_context`] and [`DependencyGraph::upsert_machine`]
/// accept.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ComponentEdges {
    Context {
        extends: Vec<String>,
    },
    Machine {
        refines: Option<String>,
        sees: Vec<String>,
    },
}

impl ComponentLoc {
    fn of_all(components: &[rossi::Component]) -> Vec<Self> {
        components.iter().map(Self::of).collect()
    }

    fn of(component: &rossi::Component) -> Self {
        let (_, name) = kind_and_name(component);
        let edges = match component {
            rossi::Component::Context(context) => ComponentEdges::Context {
                extends: context.extends.clone(),
            },
            rossi::Component::Machine(machine) => ComponentEdges::Machine {
                refines: machine.refines.clone(),
                sees: machine.sees.clone(),
            },
        };
        Self { name, edges }
    }

    /// Make `graph`'s node for this name be this declaration, of this kind.
    ///
    /// The node of the other kind goes too: `upsert_context` / `upsert_machine`
    /// each replace a node of their own kind only, so a component that changed
    /// kind under a stable name (`MACHINE m` edited into `CONTEXT m`) would
    /// otherwise leave the old node — and its edges — standing.
    fn rewrite_in(self, graph: &mut DependencyGraph) {
        match self.edges {
            ComponentEdges::Context { extends } => {
                graph.remove(ComponentKind::Machine, &self.name);
                graph.upsert_context(&self.name, extends);
            }
            ComponentEdges::Machine { refines, sees } => {
                graph.remove(ComponentKind::Context, &self.name);
                graph.upsert_machine(&self.name, refines, sees);
            }
        }
    }
}

/// Workspace-wide cross-reference manager.
///
/// The [`DependencyGraph`] is the single structural source of truth (shared
/// with `rossi-build`); the URI maps only translate between file URIs and
/// component names for navigation.
pub struct CrossReferenceManager {
    /// Structural dependency graph (SEES / REFINES / EXTENDS).
    graph: RwLock<DependencyGraph>,

    /// Map from file URI to the components saved there and any open overlay.
    /// Most files hold a single component, but `rossi import --merge` output
    /// concatenates several into one file. Keyed by canonical identity,
    /// so one file reaching the server under two spellings occupies one entry.
    uri_to_component: DashMap<String, DocumentComponents>,

    /// Map from component name to every file that declares it. Names are
    /// meant to be unique within a project, but duplicates are *diagnosed*
    /// (EB019), not prevented, so this is genuinely many-to-one: while a
    /// duplicate stands, both declarers must stay indexed, or removing one
    /// takes the graph node with it and strands every reference to the name.
    /// Ordered, so the declarer that represents the name is a stable choice
    /// rather than whichever write happened last.
    name_to_uris: DashMap<String, BTreeSet<String>>,

    /// Workspace root path (if available)
    workspace_root: RwLock<Option<PathBuf>>,

    /// Raw client/scan URI spellings mapped to one canonical file identity,
    /// shared with the workspace symbol index so the two agree on which
    /// spellings name the same file — and so one file costs one
    /// `canonicalize`, not one per index.
    document_uris: Arc<crate::uri_identity::DocumentUris>,

    /// Set once [`Self::scan_workspace`] has walked the workspace and indexed
    /// its on-disk `.eventb` files. Distinct from `workspace_root.is_some()`,
    /// which becomes true at `initialize` — before the scan runs — so gating on
    /// it would let cross-reference checks fire against an empty graph.
    scanned: AtomicBool,
}

/// The distinct component names a document declares.
fn declared_names(locs: &[ComponentLoc]) -> BTreeSet<String> {
    locs.iter().map(|loc| loc.name.clone()).collect()
}

impl Default for CrossReferenceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl CrossReferenceManager {
    /// Create a new cross-reference manager
    pub fn new() -> Self {
        Self::with_uris(Arc::default())
    }

    /// Build an index sharing `document_uris` with the workspace symbol index,
    /// so a file resolves to one identity across both and is canonicalized
    /// once rather than once per index.
    pub(crate) fn with_uris(document_uris: Arc<crate::uri_identity::DocumentUris>) -> Self {
        Self {
            graph: RwLock::new(DependencyGraph::new()),
            uri_to_component: DashMap::new(),
            name_to_uris: DashMap::new(),
            document_uris,
            workspace_root: RwLock::new(None),
            scanned: AtomicBool::new(false),
        }
    }

    /// The identity table this index keys by, for sharing with another index.
    pub(crate) fn document_uris(&self) -> &Arc<crate::uri_identity::DocumentUris> {
        &self.document_uris
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
    /// Update or add the components defined in a document, parsing `text` first.
    ///
    /// For callers that hold only the source text: the workspace disk scan and
    /// the unit tests. The edit path (`didOpen`/`didChange`) instead calls
    /// [`Self::index_components`] with the document's already stored parse, so
    /// the file is not parsed a second time just to refresh this index.
    ///
    /// Parses with error recovery (via the shared helper) so a local syntax
    /// error does not tear the file out of the dependency graph:
    /// SEES/REFINES/EXTENDS edges are recovered from the clause text even when a
    /// predicate fails to parse.
    pub fn update_component(&self, uri: String, text: &str) {
        let components = crate::component_util::parse_all(text);
        self.index_components(uri, &components);
    }

    /// Index a document's already-parsed components as the open overlay for
    /// its file. The single-source-of-truth entry point for the edit path,
    /// which passes the document manager's stored parse straight through.
    ///
    /// A buffer that parses to nothing installs an *empty* overlay rather than
    /// dropping the file: while the document is open it answers for its file,
    /// and letting the saved components reappear mid-edit would resolve
    /// references against text the user can no longer see.
    pub fn index_components(&self, uri: String, components: &[rossi::Component]) {
        debug!("Updating open components for URI: {}", uri);
        self.write_layer(uri, |document| {
            let previous = document.open.replace(ComponentLoc::of_all(components));
            previous.as_deref() != document.open.as_deref()
        });
    }

    /// Refresh the saved components for a file from the workspace scan or a
    /// watched-file event. Resolves the spelling first, so a file the watcher
    /// reports before any editor opens it is already keyed by file identity
    /// and the later open overlay lands on the same entry.
    pub(crate) fn index_disk_components(&self, uri: String, components: &[rossi::Component]) {
        debug!("Updating disk components for URI: {}", uri);
        self.register_document_uri(&uri);
        self.write_layer(uri, |document| {
            let previous = std::mem::replace(&mut document.disk, ComponentLoc::of_all(components));
            // A saved layer under an open buffer changes nothing the index
            // answers with, so a save costs no name-index or graph work.
            document.open.is_none() && previous != document.disk
        });
    }

    /// Drop a document's open overlay, revealing whatever is saved for it.
    /// Closing an editor needs no file read: the saved layer is already here.
    pub(crate) fn remove_document(&self, uri: &str) {
        debug!("Removing open components for URI: {}", uri);
        self.write_layer(uri.to_string(), |document| {
            let previous = document.open.take();
            previous.is_some_and(|open| open != document.disk)
        });
    }

    /// Drop a deleted file's saved components, keeping any open overlay: the
    /// buffer outlives the file until the editor closes it.
    pub(crate) fn remove_disk_document(&self, uri: &str) {
        debug!("Removing disk components for URI: {}", uri);
        self.write_layer(uri.to_string(), |document| {
            let moved = document.open.is_none() && !document.disk.is_empty();
            document.disk.clear();
            moved
        });
    }

    /// Remove a file from the index entirely, both layers. One pass, so the
    /// index is never observed holding half of a removed file.
    pub fn remove_component(&self, uri: &str) {
        debug!("Removing components for URI: {}", uri);
        self.write_layer(uri.to_string(), |document| {
            let moved = !document.effective().is_empty();
            document.open = None;
            document.disk.clear();
            moved
        });
    }

    /// Write one layer of a document and bring the name index and the graph
    /// back in step with the result.
    ///
    /// The two locks are only ever taken in one order: this reads and writes
    /// the declaration maps, and only then — in [`Self::rebuild_names`] —
    /// takes the graph.
    fn write_layer(&self, uri: String, edit: impl FnOnce(&mut DocumentComponents) -> bool) {
        let uri = self.document_uris.key(&uri);

        // `edit` reports whether it changed what this document answers with.
        // A keystroke inside a formula moves no declaration, and a save under
        // an open buffer changes only the layer beneath it, so the common
        // edit-path write ends here — before any name-index or graph work.
        let (before, after) = {
            let mut entry = self.uri_to_component.entry(uri.clone()).or_default();
            let document = entry.value_mut();
            let before = declared_names(document.effective());
            if !edit(document) {
                drop(entry);
                self.uri_to_component
                    .remove_if(&uri, |_, document| document.is_empty());
                return;
            }
            let after = declared_names(document.effective());
            (before, after)
        };
        self.uri_to_component
            .remove_if(&uri, |_, document| document.is_empty());

        for name in &after {
            self.name_to_uris
                .entry(name.clone())
                .or_default()
                .insert(uri.clone());
        }
        for name in before.difference(&after) {
            self.forget_declaration(name, &uri);
        }
        self.rebuild_names(before.union(&after).map(String::as_str));
    }

    /// Drop `uri` from the set of files declaring `name`, discarding the name
    /// entirely once nothing declares it.
    fn forget_declaration(&self, name: &str, uri: &str) {
        self.name_to_uris.remove_if_mut(name, |_, uris| {
            uris.remove(uri);
            uris.is_empty()
        });
    }

    /// Rewrite the graph node for each of `names` from whichever file now
    /// represents it, dropping the node when no file declares it any more.
    ///
    /// Every lookup happens before the graph lock is taken: the declaration
    /// maps are read first and the graph written once, so the two are only
    /// ever acquired in that order.
    fn rebuild_names<'a>(&self, names: impl Iterator<Item = &'a str>) {
        let mut pending: BTreeMap<&'a str, Option<ComponentLoc>> = BTreeMap::new();
        for name in names {
            pending
                .entry(name)
                .or_insert_with(|| self.declaration(name));
        }
        if pending.is_empty() {
            return;
        }
        let mut graph = self.graph.write();
        for (name, declaration) in pending {
            match declaration {
                Some(loc) => loc.rewrite_in(&mut graph),
                // The kind went with the last declaration, so clear both: a
                // name is at most one of them, and removing the other is a
                // no-op.
                None => {
                    graph.remove(ComponentKind::Context, name);
                    graph.remove(ComponentKind::Machine, name);
                }
            }
        }
    }

    /// The declaration of `name` that represents it: the first component of
    /// that name in the lowest-ordered file declaring it, reading each file's
    /// open buffer where it has one. Deterministic by construction — the
    /// alternative is whichever write landed last, which varies run to run and
    /// would make go-to-definition unstable.
    fn declaration(&self, name: &str) -> Option<ComponentLoc> {
        let uris = self.name_to_uris.get(name)?;
        for uri in uris.value() {
            if let Some(document) = self.uri_to_component.get(uri)
                && let Some(loc) = document
                    .value()
                    .effective()
                    .iter()
                    .find(|loc| loc.name == name)
            {
                return Some(loc.clone());
            }
        }
        None
    }

    /// Resolve a file URI once on the blocking pool before open-document
    /// analysis starts, so scan and client spellings share one entry.
    pub(crate) fn register_document_uri(&self, uri: &str) {
        self.document_uris.register(uri);
    }

    /// Find the URI of a component by its name
    ///
    /// This searches for contexts and machines by name and returns the file URI
    /// where that component is defined.
    pub fn find_component_uri(&self, component_name: &str) -> Option<String> {
        self.name_to_uris
            .get(component_name)
            .and_then(|uris| uris.value().first().cloned())
    }

    /// URIs of every indexed file declaring any of `component_names`.
    ///
    /// Unlike [`Self::find_component_uri`], this preserves duplicate
    /// declarations so syntax-aware workspace operations can still visit every
    /// file while diagnostics report the invalid duplicate-name state. The URI
    /// index is scanned once however many names the caller supplies.
    pub(crate) fn component_uris_for_names(
        &self,
        component_names: &HashSet<String>,
    ) -> Vec<String> {
        self.uri_to_component
            .iter()
            .filter(|entry| {
                entry
                    .value()
                    .effective()
                    .iter()
                    .any(|location| component_names.contains(location.name.as_str()))
            })
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Get component info by name
    pub fn get_component(&self, name: &str) -> Option<ComponentInfo> {
        let (kind, references) = self.graph.read().references_of(name)?;
        let uri = self.find_component_uri(name).unwrap_or_default();
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
            .get(&self.document_uris.key(uri))
            .and_then(|document| {
                document
                    .value()
                    .effective()
                    .first()
                    .map(|loc| loc.name.clone())
            })
    }

    /// Find all components that reference a given component
    ///
    /// For example, find all machines that SEE a context, or all machines that
    /// REFINE a given abstract machine.
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
                let uri = self.find_component_uri(&name).unwrap_or_default();
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
        self.scan_workspace_with(root_path, |_, _, _| {})
    }

    /// Scan a directory once, exposing each parsed file to another workspace
    /// index so callers do not repeat the filesystem walk or parse.
    pub(crate) fn scan_workspace_with<F>(
        &self,
        root_path: &Path,
        mut index_file: F,
    ) -> std::io::Result<usize>
    where
        F: FnMut(String, &[rossi::Component], &str),
    {
        debug!("Scanning workspace at: {:?}", root_path);

        self.scanned.store(false, Ordering::Release);
        let mut sources = Vec::new();

        // Recursively find all Event-B source files, via the crate's one
        // source-tree walk (symlinks followed, depth-capped, dot-directories
        // skipped) so this index and the Rodin build can never disagree
        // about what a source tree contains.
        for entry in rossi_build::walk::source_walk(root_path) {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type().is_file() && rossi_build::walk::is_source_file(path) {
                let uri = Url::from_file_path(path).map_err(|()| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("cannot convert {} to a file URI", path.display()),
                    )
                })?;
                let content = std::fs::read_to_string(path).map_err(|error| {
                    std::io::Error::new(error.kind(), format!("{}: {error}", path.display()))
                })?;
                sources.push((uri.to_string(), content));
            }
        }

        let count = sources.len();
        for (uri, content) in sources {
            let components = crate::component_util::parse_all(&content);
            // `index_disk_components` resolves the scan's spelling, off the
            // request path, so the client's own spelling of the same file
            // lands on this entry.
            self.index_disk_components(uri.clone(), &components);
            index_file(uri, &components, &content);
        }

        debug!("Scanned {} Event-B files in workspace", count);
        self.scanned.store(true, Ordering::Release);
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

    // --- Queries backing cross-component diagnostics ---

    /// Whether [`Self::scan_workspace`] has run — i.e. the on-disk `.eventb`
    /// files have been indexed. Diagnostics that would false-positive without a
    /// full workspace view (unresolved references, duplicate component names)
    /// gate on this: in single-file mode no siblings are indexed, so every
    /// cross-component reference would look missing. Gating on the actual scan
    /// (not merely a set workspace root) avoids that flood in the window before
    /// the scan completes, or when it fails.
    pub fn is_scanned(&self) -> bool {
        self.scanned.load(Ordering::Acquire)
    }

    /// Whether a component of `kind` named `name` is indexed in the workspace.
    /// Kind-aware (a context and a machine may share a name), so a SEES / EXTENDS
    /// / REFINES target can be resolved against its expected [`ComponentKind`].
    pub fn contains(&self, kind: ComponentKind, name: &str) -> bool {
        self.graph.read().contains(kind, name)
    }

    /// Copy one indexed node into a request-scoped dependency graph.
    pub(crate) fn copy_dependency_node(
        &self,
        target: &mut DependencyGraph,
        kind: ComponentKind,
        name: &str,
    ) -> bool {
        target.copy_node_from(&self.graph.read(), kind, name)
    }

    /// Each file declaring `name`, with how many times it declares it. A
    /// merged file declaring it twice is one entry with a count of two, so the
    /// caller can tell that from one declaration apiece in two files.
    ///
    /// Files are identities, never basenames: two directories may each hold an
    /// `m.eventb`, and collapsing those would report a cross-file duplicate as
    /// a within-file one. Queried per open-document name, so no
    /// whole-workspace map is built per publish.
    pub fn component_declarations(&self, name: &str) -> Vec<(String, usize)> {
        let Some(uris) = self.name_to_uris.get(name) else {
            return Vec::new();
        };
        uris.value()
            .iter()
            .filter_map(|uri| {
                let document = self.uri_to_component.get(uri)?;
                let count = document
                    .value()
                    .effective()
                    .iter()
                    .filter(|loc| loc.name == name)
                    .count();
                (count > 0).then(|| (uri.clone(), count))
            })
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

    use crate::test_util::TempDir as TempWorkspace;

    /// The components of a source, for feeding a layer directly.
    fn parse(text: &str) -> Vec<rossi::Component> {
        crate::component_util::parse_all(text)
    }

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
    fn test_component_name_update() {
        let uri = "file:///component.eventb".to_string();

        let old_source = r#"
CONTEXT old_name
END
"#;

        let new_source = r#"
CONTEXT new_name
END
"#;

        let manager = CrossReferenceManager::new();

        // Add component with old name
        manager.update_component(uri.clone(), old_source);
        assert!(manager.find_component_uri("old_name").is_some());

        // Update to new name
        manager.update_component(uri, new_source);
        assert!(manager.find_component_uri("new_name").is_some());
        assert!(manager.find_component_uri("old_name").is_none());
    }

    #[test]
    fn test_component_with_multiple_sees() {
        let ctx1_source = r#"
CONTEXT ctx1
CONSTANTS
    c1
END
"#;

        let ctx2_source = r#"
CONTEXT ctx2
CONSTANTS
    c2
END
"#;

        let mch_source = r#"
MACHINE machine
SEES ctx1 ctx2
VARIABLES
    v
END
"#;

        let manager = CrossReferenceManager::new();
        manager.update_component("file:///ctx1.eventb".to_string(), ctx1_source);
        manager.update_component("file:///ctx2.eventb".to_string(), ctx2_source);
        manager.update_component("file:///machine.eventb".to_string(), mch_source);

        // Verify machine SEES both contexts
        let mch_info = manager.get_component("machine").unwrap();
        let sees_refs = mch_info.references.get(&ReferenceKind::Sees).unwrap();
        assert_eq!(sees_refs.len(), 2);
        assert!(sees_refs.contains(&"ctx1".to_string()));
        assert!(sees_refs.contains(&"ctx2".to_string()));

        // Verify both contexts can be found
        assert!(manager.find_component_uri("ctx1").is_some());
        assert!(manager.find_component_uri("ctx2").is_some());
    }

    #[test]
    fn test_local_symbol_not_tracked_as_component() {
        let mch_source = r#"
MACHINE machine
VARIABLES
    count
INVARIANTS
    @inv1 count ∈ ℕ
END
"#;

        let manager = CrossReferenceManager::new();
        manager.update_component("file:///machine.eventb".to_string(), mch_source);

        // Verify machine is tracked as a component
        assert!(manager.find_component_uri("machine").is_some());

        // Verify local variable is NOT tracked as a component
        assert!(manager.find_component_uri("count").is_none());
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
        let root = TempWorkspace::new("eventb-lsp-scan-test");
        std::fs::write(root.join("eventb_ctx.eventb"), "CONTEXT eventb_ctx\nEND\n").unwrap();
        std::fs::write(root.join("rossi_ctx.rossi"), "CONTEXT rossi_ctx\nEND\n").unwrap();
        std::fs::write(root.join("ignored.txt"), "CONTEXT ignored\nEND\n").unwrap();

        let manager = CrossReferenceManager::new();
        let count = manager.scan_workspace(&root).unwrap();

        assert_eq!(count, 1);
        assert!(manager.find_component_uri("eventb_ctx").is_some());
        assert!(manager.find_component_uri("rossi_ctx").is_none());
        assert!(manager.find_component_uri("ignored").is_none());
    }

    #[test]
    fn removing_a_file_keeps_a_name_another_file_has_claimed() {
        let manager = CrossReferenceManager::new();
        manager.update_component("file:///old.eventb".to_string(), "CONTEXT ctx\nEND\n");
        // The move's create half lands first and also declares `ctx`.
        manager.update_component("file:///new.eventb".to_string(), "CONTEXT ctx\nEND\n");

        manager.remove_component("file:///old.eventb");

        assert_eq!(
            manager.find_component_uri("ctx").as_deref(),
            Some("file:///new.eventb")
        );
        assert!(manager.contains(ComponentKind::Context, "ctx"));
    }

    #[test]
    fn removing_a_declarer_rebuilds_the_node_from_the_survivor() {
        // The survivor's own edges must come back, not the removed file's:
        // the node is rebuilt from the declaration that remains, so a stale
        // EXTENDS parent cannot outlive the file that declared it.
        let manager = CrossReferenceManager::new();
        manager.update_component("file:///base.eventb".to_string(), "CONTEXT base\nEND\n");
        manager.update_component(
            "file:///a.eventb".to_string(),
            "CONTEXT dup\nEXTENDS base\nEND\n",
        );
        manager.update_component("file:///b.eventb".to_string(), "CONTEXT dup\nEND\n");

        // `a.eventb` sorts first, so it represents `dup` until it is removed.
        assert_eq!(manager.extends_chain("dup"), vec!["base".to_string()]);

        manager.remove_component("file:///a.eventb");

        assert_eq!(
            manager.find_component_uri("dup").as_deref(),
            Some("file:///b.eventb")
        );
        assert!(
            manager.extends_chain("dup").is_empty(),
            "the surviving declaration extends nothing"
        );
    }

    #[test]
    fn the_representative_declarer_does_not_depend_on_write_order() {
        let src = "CONTEXT ctx\nEND\n";
        let forwards = CrossReferenceManager::new();
        forwards.update_component("file:///a.eventb".to_string(), src);
        forwards.update_component("file:///b.eventb".to_string(), src);

        let backwards = CrossReferenceManager::new();
        backwards.update_component("file:///b.eventb".to_string(), src);
        backwards.update_component("file:///a.eventb".to_string(), src);

        assert_eq!(
            forwards.find_component_uri("ctx"),
            backwards.find_component_uri("ctx")
        );
        assert_eq!(
            forwards.find_component_uri("ctx").as_deref(),
            Some("file:///a.eventb")
        );
    }

    #[test]
    fn an_open_overlay_shadows_the_saved_components_until_it_is_dropped() {
        let uri = "file:///model.eventb".to_string();
        let manager = CrossReferenceManager::new();
        manager.index_disk_components(uri.clone(), &parse("CONTEXT saved\nEND\n"));

        manager.update_component(uri.clone(), "CONTEXT unsaved\nEND\n");
        assert_eq!(manager.find_component_uri("unsaved"), Some(uri.clone()));
        assert!(manager.find_component_uri("saved").is_none());

        // Closing reveals what is saved, with no file read at all.
        manager.remove_document(&uri);

        assert!(manager.find_component_uri("unsaved").is_none());
        assert_eq!(manager.find_component_uri("saved"), Some(uri));
    }

    #[test]
    fn a_file_deleted_while_open_keeps_its_buffer_and_loses_its_saved_layer() {
        let uri = "file:///model.eventb".to_string();
        let manager = CrossReferenceManager::new();
        manager.index_disk_components(uri.clone(), &parse("CONTEXT saved\nEND\n"));
        manager.update_component(uri.clone(), "CONTEXT open\nEND\n");

        manager.remove_disk_document(&uri);

        assert_eq!(manager.find_component_uri("open"), Some(uri.clone()));
        manager.remove_document(&uri);
        assert!(manager.find_component_uri("open").is_none());
        assert!(manager.find_component_uri("saved").is_none());
    }

    #[test]
    fn an_unparsable_buffer_hides_the_saved_components() {
        // While a document is open it answers for its file; letting the saved
        // components reappear mid-edit would resolve references against text
        // the user can no longer see.
        let manager = CrossReferenceManager::new();
        manager.index_disk_components(
            "file:///m.eventb".to_string(),
            &crate::component_util::parse_all("CONTEXT saved\nEND\n"),
        );

        manager.update_component("file:///m.eventb".to_string(), "not Event-B at all");
        assert!(manager.find_component_uri("saved").is_none());

        manager.remove_document("file:///m.eventb");
        assert_eq!(
            manager.find_component_uri("saved").as_deref(),
            Some("file:///m.eventb")
        );
    }

    #[test]
    fn changing_a_component_kind_drops_the_old_node() {
        let manager = CrossReferenceManager::new();
        manager.update_component("file:///m.eventb".to_string(), "MACHINE m\nSEES ctx\nEND\n");
        assert!(manager.contains(ComponentKind::Machine, "m"));

        manager.update_component("file:///m.eventb".to_string(), "CONTEXT m\nEND\n");

        assert!(manager.contains(ComponentKind::Context, "m"));
        assert!(!manager.contains(ComponentKind::Machine, "m"));
    }

    #[cfg(unix)]
    #[test]
    fn a_watched_file_created_after_the_scan_keys_as_the_editor_will_spell_it() {
        // The watcher reports a brand-new file before any editor opens it, so
        // its saved layer must already be keyed by file identity — otherwise
        // the later `didOpen` overlay lands on a second entry and the one
        // component looks like a cross-file duplicate.
        let root = TempWorkspace::new("eventb-lsp-symlink-watch");
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("link")).unwrap();
        std::fs::write(root.join("real").join("m.eventb"), "CONTEXT c\nEND\n").unwrap();

        let manager = CrossReferenceManager::new();
        let uri = Url::from_file_path(root.join("link").join("m.eventb")).unwrap();
        manager.index_disk_components(uri.to_string(), &parse("CONTEXT c\nEND\n"));
        manager.update_component(uri.to_string(), "CONTEXT c\nEND\n");

        let declarations = manager.component_declarations("c");
        assert_eq!(declarations.len(), 1, "{declarations:?}");
    }

    #[test]
    fn scan_workspace_read_error_leaves_index_incomplete() {
        let root = TempWorkspace::new("eventb-lsp-scan-error-test");
        std::fs::write(root.join("good.eventb"), "CONTEXT good\nEND\n").unwrap();
        std::fs::write(root.join("unreadable.eventb"), [0xff]).unwrap();

        let manager = CrossReferenceManager::new();
        let result = manager.scan_workspace(&root);

        assert!(result.is_err());
        assert!(!manager.is_scanned());
        assert!(manager.find_component_uri("good").is_none());
    }

    #[test]
    fn scan_workspace_skips_dot_directories() {
        let root = TempWorkspace::new("eventb-lsp-dot-dir-scan-test");
        std::fs::write(
            root.join("visible_ctx.eventb"),
            "CONTEXT visible_ctx\nEND\n",
        )
        .unwrap();
        let hidden = root.join(".rossi").join("rodin").join("proj");
        std::fs::create_dir_all(&hidden).unwrap();
        std::fs::write(
            hidden.join("hidden_ctx.eventb"),
            "CONTEXT hidden_ctx\nEND\n",
        )
        .unwrap();

        let manager = CrossReferenceManager::new();
        let count = manager.scan_workspace(&root).unwrap();

        assert_eq!(count, 1);
        assert!(manager.find_component_uri("visible_ctx").is_some());
        assert!(manager.find_component_uri("hidden_ctx").is_none());
    }

    /// Regression test: a single pathological file used to overflow the
    /// stack inside `rossi::parse` and abort the whole server during the
    /// post-initialize workspace scan (originally hit via a fuzz artifact
    /// with thousands of nested parens left in /tmp).
    #[test]
    fn test_scan_workspace_survives_deeply_nested_file() {
        let root = TempWorkspace::new("eventb-lsp-deep-scan-test");
        std::fs::write(root.join("good_ctx.eventb"), "CONTEXT good_ctx\nEND\n").unwrap();
        let pathological = format!(
            "context deep_ctx axioms @a {}x{} = 1 end",
            "(".repeat(5000),
            ")".repeat(5000)
        );
        std::fs::write(root.join("deep_ctx.eventb"), pathological).unwrap();

        let manager = CrossReferenceManager::new();
        let count = manager.scan_workspace(&root).unwrap();

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
    fn is_scanned_set_only_after_scan() {
        let root = TempWorkspace::new("eventb-lsp-scanned");
        std::fs::write(root.join("c.eventb"), "CONTEXT c\nEND\n").unwrap();

        let manager = CrossReferenceManager::new();
        // A set workspace root alone is not yet a completed scan.
        manager.set_workspace_root(root.to_path_buf());
        assert!(!manager.is_scanned());

        manager.scan_workspace(&root).unwrap();
        assert!(manager.is_scanned());
    }

    #[test]
    fn contains_is_kind_aware() {
        let manager = CrossReferenceManager::new();
        register_context(&manager, "C", &[]);
        register_machine(&manager, "M", &[], &[]);
        assert!(manager.contains(ComponentKind::Context, "C"));
        assert!(manager.contains(ComponentKind::Machine, "M"));
        // Right name, wrong kind — a context is not a machine.
        assert!(!manager.contains(ComponentKind::Machine, "C"));
        assert!(!manager.contains(ComponentKind::Context, "absent"));
    }

    #[test]
    fn component_declarations_counts_same_named_files_in_two_directories() {
        // Two directories may each hold an `m.eventb`. That is a real
        // cross-file duplicate, so the two must stay distinguishable — which
        // sharing a basename would not be.
        let manager = CrossReferenceManager::new();
        let src = "CONTEXT c\nEND\n";
        manager.update_component("file:///one/m.eventb".to_string(), src);
        manager.update_component("file:///two/m.eventb".to_string(), src);

        let files = manager.component_declarations("c");

        assert_eq!(
            files,
            [
                ("file:///one/m.eventb".to_string(), 1),
                ("file:///two/m.eventb".to_string(), 1)
            ],
            "{files:?}"
        );
    }

    #[test]
    fn component_declarations_finds_cross_file_dups() {
        let manager = CrossReferenceManager::new();
        let src = "CONTEXT c\nEND\n";
        manager.update_component("file:///a.eventb".to_string(), src);
        manager.update_component("file:///b.eventb".to_string(), src);
        let files = manager.component_declarations("c");
        assert_eq!(files.len(), 2, "{files:?}");
    }

    #[test]
    fn component_declarations_dedupes_same_file_by_path() {
        // The scan and the edit path can key the same physical file under URI
        // spellings that differ but resolve to one path (here, a `.` segment).
        // It must count once, not look like a cross-file duplicate.
        let manager = CrossReferenceManager::new();
        let src = "CONTEXT c\nEND\n";
        manager.update_component("file:///dir/x.eventb".to_string(), src);
        manager.update_component("file:///dir/./x.eventb".to_string(), src);
        let files = manager.component_declarations("c");
        assert_eq!(
            files.len(),
            1,
            "same physical file must count once: {files:?}"
        );
    }

    #[test]
    fn component_declarations_counts_a_name_declared_twice_in_one_file() {
        // `rossi import --merge` concatenates a project into one file, so a
        // name repeated inside it is a real duplicate the index must surface
        // — not something to collapse on the way in.
        let manager = CrossReferenceManager::new();
        manager.update_component(
            "file:///merged.eventb".to_string(),
            "CONTEXT dup\nEND\n\nCONTEXT dup\nEND\n",
        );

        assert_eq!(
            manager.component_declarations("dup"),
            [("file:///merged.eventb".to_string(), 2)]
        );
    }

    #[test]
    fn component_declarations_single_when_unique() {
        let manager = CrossReferenceManager::new();
        manager.update_component("file:///a.eventb".to_string(), "CONTEXT a\nEND\n");
        assert_eq!(manager.component_declarations("a").len(), 1);
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
