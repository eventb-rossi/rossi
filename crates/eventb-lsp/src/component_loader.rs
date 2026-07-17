//! Request-scoped component loading shared by the cross-file providers.
//!
//! Find-references, go-to-definition, hover, and completion all need the parsed
//! AST of a component named somewhere else in the workspace. Doing that
//! naively — resolve the name to a file, read the text, parse it — re-parses the
//! same file every time the name is looked up, and a single find-references
//! request can look a file up many times (once per candidate, again for every
//! component in its refinement/context chain).
//!
//! [`ComponentLoader`] removes that waste with one rule: a file is the unit of
//! parsing, and it is parsed at most once per request.
//!
//! - **Open documents** reuse the [`DocumentManager`]'s stored parse
//!   ([`DocumentManager::parse_result`]) — the single source of truth every
//!   feature already reads — so they are never re-parsed here.
//! - **On-disk components** are read and parsed once, into the same
//!   [`ParsedDocument`] bundle (text + recovered AST) the store uses.
//!
//! Results are memoised per [`Url`], so a file holding several merged components
//! (the output of `rossi import --merge`) is parsed once no matter how many of
//! its component names are requested. The cache lives for one request only
//! (it holds a `!Sync` `RefCell`); it is never stored on a provider.

use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use rossi::{Component, ComponentNameSite};

use crate::component_util::component_reference_clause;
use crate::cross_references::CrossReferenceManager;
use crate::document::{DocumentManager, ParsedDocument};
use crate::identifier_utils::{self, WordBoundary};
use crate::lsp_types::{Range, Url};
use crate::position::span_to_range;

/// One component declaration or dependency occurrence in the workspace.
///
/// Providers map this neutral record to their own LSP response shapes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceComponentOccurrence {
    pub(crate) uri: Url,
    pub(crate) range: Range,
}

/// One component resolved for the current request: the parsed snapshot of the
/// file it lives in, plus the index of the component within that file. Holding
/// the whole [`ParsedDocument`] keeps every span the caller later slices indexed
/// against the exact `text` it was parsed from.
pub struct LoadedComponent {
    uri: Url,
    doc: Arc<ParsedDocument>,
    index: usize,
}

impl LoadedComponent {
    /// The file URI this component is defined in.
    pub fn uri(&self) -> &Url {
        &self.uri
    }

    /// The whole-file source text the component's spans index into.
    pub fn text(&self) -> &str {
        self.doc.text()
    }

    /// The parsed component.
    pub fn component(&self) -> &Component {
        &self.doc.components()[self.index]
    }
}

/// Loads and memoises workspace components for a single request.
///
/// Construct one per request, use it, and drop it; see the module docs.
pub struct ComponentLoader<'a> {
    manager: &'a CrossReferenceManager,
    documents: Option<&'a DocumentManager>,
    /// One parsed snapshot per file URI, built on first use.
    cache: RefCell<HashMap<Url, Arc<ParsedDocument>>>,
    /// Components from current open-document snapshots, computed on demand.
    open_components: OnceCell<BTreeMap<String, Url>>,
}

impl<'a> ComponentLoader<'a> {
    /// Create a loader over `manager`'s workspace index, preferring open
    /// documents from `documents` when available.
    pub fn new(manager: &'a CrossReferenceManager, documents: Option<&'a DocumentManager>) -> Self {
        Self {
            manager,
            documents,
            cache: RefCell::new(HashMap::new()),
            open_components: OnceCell::new(),
        }
    }

    /// Build a loader when a cross-reference manager is available, else `None`.
    /// Folds the providers' common `manager.map(|m| ComponentLoader::new(m, dm))`
    /// construction into one place.
    pub fn optional(
        manager: Option<&'a CrossReferenceManager>,
        documents: Option<&'a DocumentManager>,
    ) -> Option<Self> {
        manager.map(|manager| Self::new(manager, documents))
    }

    /// The workspace index this loader resolves names and dependency chains
    /// against. Callers use it for the structural queries (refinement chains,
    /// visible contexts) that complement [`Self::load`].
    pub fn manager(&self) -> &CrossReferenceManager {
        self.manager
    }

    /// The parsed snapshot of the file at `uri`, memoised per request.
    ///
    /// Reuses the open-document store when the file is open (no re-parse),
    /// otherwise reads it from disk and parses it once. Returns `None` only when
    /// the file is neither open nor a readable local path.
    pub fn parsed(&self, uri: &Url) -> Option<Arc<ParsedDocument>> {
        // Clone the cached handle out and drop the borrow before parsing, so a
        // miss never holds a borrow across `build` (which itself takes the
        // cache mutably).
        let cached = self.cache.borrow().get(uri).cloned();
        if let Some(doc) = cached {
            #[cfg(test)]
            crate::benchmark_metrics::loader_cache_hit();
            return Some(doc);
        }
        #[cfg(test)]
        crate::benchmark_metrics::loader_cache_miss();
        let doc = self.build(uri)?;
        self.cache
            .borrow_mut()
            .insert(uri.clone(), Arc::clone(&doc));
        Some(doc)
    }

    /// Current source text for `uri` without building an AST.
    ///
    /// Component occurrence queries need only the recovery scanner's exact
    /// source locations. Open documents come directly from the document store;
    /// closed documents incur one disk read and no otherwise-unused full parse.
    fn source_text(&self, uri: &Url) -> Option<String> {
        if let Some(documents) = self.documents {
            if let Some(text) = documents.get_text(uri) {
                return Some(text);
            }
            // Mirror `build`: do not fall through to stale disk text while a
            // concurrently replaced document is still open.
            if documents.version(uri).is_some() {
                return documents.get_text(uri);
            }
        }
        read_source_file(uri)
    }

    /// Parse the file at `uri` from the open-document store or disk. Never
    /// touches the cache itself.
    fn build(&self, uri: &Url) -> Option<Arc<ParsedDocument>> {
        if let Some(documents) = self.documents {
            if let Some(doc) = documents.parse_result(uri) {
                #[cfg(test)]
                crate::benchmark_metrics::document_parse_reuse();
                return Some(doc);
            }
            // A concurrent edit can supersede the parse that just completed.
            // Retry the current open revision once, but never fall through to
            // stale disk text for a URI that is still open.
            if documents.version(uri).is_some() {
                let doc = documents.parse_result(uri);
                #[cfg(test)]
                if doc.is_some() {
                    crate::benchmark_metrics::document_parse_reuse();
                }
                return doc;
            }
        }
        let text = read_source_file(uri)?;
        #[cfg(test)]
        crate::benchmark_metrics::disk_parse();
        Some(Arc::new(ParsedDocument::from_text(text)))
    }

    /// Load the component named `name`: resolve its file, get that file's parsed
    /// snapshot, and locate the component within it.
    ///
    /// The first component of that name is chosen, matching the lookup the
    /// parser-based helpers performed before this loader existed.
    pub fn load(&self, name: &str) -> Option<LoadedComponent> {
        if let Some(uri) = self
            .manager
            .find_component_uri(name)
            .and_then(|uri| Url::parse(&uri).ok())
            && let Some(loaded) = self.load_from_uri(uri, name)
        {
            return Some(loaded);
        }

        // The name index follows the diagnostics debounce. Fall back to current
        // open parses so a just-renamed component is immediately resolvable.
        let uri = self.open_components().get(name)?.clone();
        self.load_from_uri(uri, name)
    }

    /// Names declared by the latest snapshots of all open documents.
    pub(crate) fn open_component_names(&self) -> impl Iterator<Item = &str> {
        self.open_components().keys().map(String::as_str)
    }

    fn open_components(&self) -> &BTreeMap<String, Url> {
        self.open_components.get_or_init(|| {
            let mut components = BTreeMap::new();
            if let Some(documents) = self.documents {
                for uri in documents.all_uris() {
                    if let Some(doc) = self.parsed(&uri) {
                        for component in doc.components() {
                            components
                                .entry(component.name().to_string())
                                .or_insert_with(|| uri.clone());
                        }
                    }
                }
            }
            components
        })
    }

    /// Files that can contain a declaration or direct structural reference to
    /// `target`, plus every open document whose graph overlay may still be
    /// waiting for the diagnostics debounce.
    fn candidate_uris_for_component(&self, target: &str) -> Vec<Url> {
        let mut names = HashSet::from([target.to_string()]);
        names.extend(
            self.manager
                .find_referencing_components(target, None)
                .into_iter()
                .map(|component| component.name),
        );

        let mut uris: Vec<Url> = self
            .manager
            .component_uris_for_names(&names)
            .into_iter()
            .filter_map(|uri| Url::parse(&uri).ok())
            .collect();
        if let Some(documents) = self.documents {
            uris.extend(documents.all_uris());
        }
        uris.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        uris.dedup();
        uris
    }

    /// Find every declaration and direct structural reference to `target` in
    /// the current workspace snapshots.
    ///
    /// The graph selects closed candidate files, while all open documents are
    /// included to tolerate diagnostics debounce. Open source text wins over
    /// disk, and every selected URI is loaded and scanned at most once.
    pub(crate) fn component_occurrences(&self, target: &str) -> Vec<WorkspaceComponentOccurrence> {
        let mut occurrences = Vec::new();
        let candidates = self.candidate_uris_for_component(target);
        #[cfg(test)]
        crate::benchmark_metrics::component_candidate_uris(candidates.len());
        for uri in candidates {
            if let Some(text) = self.source_text(&uri) {
                #[cfg(test)]
                crate::benchmark_metrics::component_source_scanned(text.len());
                occurrences.extend(component_occurrences_in_source(&text, &uri, target));
            }
        }
        #[cfg(test)]
        crate::benchmark_metrics::component_occurrences(occurrences.len());
        occurrences
    }

    fn load_from_uri(&self, uri: Url, name: &str) -> Option<LoadedComponent> {
        let doc = self.parsed(&uri)?;
        let index = doc.components().iter().position(|c| c.name() == name)?;
        Some(LoadedComponent { uri, doc, index })
    }
}

/// Exact declaration/dependency occurrences for component `name` in one
/// source. Headerless prefixes use the shared clause classifier as a narrow
/// fallback; ordinary parsed components stay on the recovery scanner alone.
fn component_occurrences_in_source(
    text: &str,
    uri: &Url,
    name: &str,
) -> Vec<WorkspaceComponentOccurrence> {
    let occurrences = rossi::component_name_occurrences_with_sites(text);
    let fallback_end_line = occurrences
        .iter()
        .filter_map(|occurrence| match (occurrence.site, occurrence.span) {
            (ComponentNameSite::Declaration(_), Some(span)) => {
                Some(text[..span.start.min(text.len())].matches('\n').count())
            }
            _ => None,
        })
        .min();
    let mut workspace_occurrences: Vec<_> = occurrences
        .into_iter()
        .filter(|occurrence| occurrence.name == name)
        .filter_map(|occurrence| occurrence.span)
        .filter(|span| text.get(span.start..span.end) == Some(name))
        .map(|span| WorkspaceComponentOccurrence {
            uri: uri.clone(),
            range: span_to_range(&span, text),
        })
        .collect();

    // The exact scanner covers everything from the first anchored declaration
    // onward. Only a headerless prefix needs the syntactic fallback; ordinary
    // documents start on line zero and skip both extra full-document masks.
    if fallback_end_line != Some(0) {
        let fallback_line_range = fallback_end_line.map(|end| (0, end - 1));
        let masked = rossi::comments::mask_comments_chars(text);
        for location in identifier_utils::find_whole_word_locations(
            text,
            name,
            uri,
            fallback_line_range,
            WordBoundary::ComponentName,
        )
        .into_iter()
        .filter(|location| component_reference_clause(&masked, location.range.start).is_some())
        {
            let occurrence = WorkspaceComponentOccurrence {
                uri: location.uri,
                range: location.range,
            };
            if !workspace_occurrences.contains(&occurrence) {
                workspace_occurrences.push(occurrence);
            }
        }
    }
    workspace_occurrences
}

fn read_source_file(uri: &Url) -> Option<String> {
    std::fs::read_to_string(uri.to_file_path().ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_document_components_share_one_parse() {
        // A file holding two merged components: looking up both names resolves
        // to one cached snapshot (one cache entry, the same `Arc`), so the file
        // is parsed once however many of its names are requested.
        let uri = Url::parse("file:///merged.eventb").unwrap();
        let text = "CONTEXT A\nEND\n\nCONTEXT B\nEND\n";

        let manager = CrossReferenceManager::new();
        manager.update_component(uri.to_string(), text);
        let documents = DocumentManager::new();
        documents.open(uri.clone(), 1, text.to_string());

        let loader = ComponentLoader::new(&manager, Some(&documents));
        let a = loader.load("A").expect("A loads");
        let b = loader.load("B").expect("B loads");

        assert_eq!(a.component().name(), "A");
        assert_eq!(b.component().name(), "B");
        assert!(
            Arc::ptr_eq(&a.doc, &b.doc),
            "both names share one parsed snapshot"
        );
        assert_eq!(loader.cache.borrow().len(), 1, "one cache entry per file");
    }

    #[test]
    fn on_disk_merged_file_is_parsed_once() {
        // With no open document, the loader reads and parses from disk. The
        // URI-keyed cache means the second name does not re-parse the file:
        // both loaded components share the same `Arc<ParsedDocument>`.
        let root = std::env::temp_dir().join(format!(
            "eventb-lsp-loader-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("merged.eventb");
        std::fs::write(&path, "CONTEXT A\nEND\n\nCONTEXT B\nEND\n").unwrap();
        let uri = Url::from_file_path(&path).unwrap();

        let manager = CrossReferenceManager::new();
        manager.update_component(uri.to_string(), &std::fs::read_to_string(&path).unwrap());

        let loader = ComponentLoader::new(&manager, None);
        let a = loader.load("A").expect("A loads from disk");
        let b = loader.load("B").expect("B loads from disk");

        std::fs::remove_dir_all(&root).unwrap();

        assert_eq!(a.component().name(), "A");
        assert_eq!(b.component().name(), "B");
        assert!(
            Arc::ptr_eq(&a.doc, &b.doc),
            "the disk file is parsed once and shared"
        );
        assert_eq!(loader.cache.borrow().len(), 1);
    }

    #[test]
    fn duplicate_name_resolves_to_first_occurrence() {
        // Two blocks of the same name in one file: `load` picks the first,
        // matching the parser-based lookup it replaced.
        let uri = Url::parse("file:///dup.eventb").unwrap();
        let text = "CONTEXT C\nCONSTANTS\n    k1\nEND\n\nCONTEXT C\nCONSTANTS\n    k2\nEND\n";

        let manager = CrossReferenceManager::new();
        manager.update_component(uri.to_string(), text);
        let documents = DocumentManager::new();
        documents.open(uri.clone(), 1, text.to_string());

        let loader = ComponentLoader::new(&manager, Some(&documents));
        let c = loader.load("C").expect("C loads");
        assert_eq!(c.index, 0, "the first occurrence is chosen");
    }

    #[test]
    fn current_open_name_resolves_before_debounced_reindexing() {
        let manager = CrossReferenceManager::new();
        let documents = DocumentManager::new();
        let uri = Url::parse("file:///renamed.eventb").unwrap();
        manager.update_component(uri.to_string(), "CONTEXT old\nEND");
        documents.open(uri.clone(), 1, "CONTEXT old\nEND".to_string());
        documents.change(
            &uri,
            2,
            vec![crate::lsp_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "CONTEXT new\nEND".to_string(),
            }],
        );

        let loader = ComponentLoader::new(&manager, Some(&documents));
        assert_eq!(loader.load("new").unwrap().component().name(), "new");
    }

    #[test]
    fn component_candidates_skip_unrelated_closed_files() {
        let manager = CrossReferenceManager::new();
        manager.update_component("file:///c.eventb".into(), "CONTEXT C\nEND");
        manager.update_component("file:///m.eventb".into(), "MACHINE M\nSEES C\nEND");
        manager.update_component("file:///unrelated.eventb".into(), "CONTEXT U\nEND");

        let loader = ComponentLoader::new(&manager, None);
        let uris: Vec<_> = loader
            .candidate_uris_for_component("C")
            .into_iter()
            .map(|uri| uri.to_string())
            .collect();

        assert_eq!(uris, ["file:///c.eventb", "file:///m.eventb"]);
    }

    #[test]
    fn component_occurrences_prefer_headerless_open_text_and_use_utf16_ranges() {
        let context_uri = Url::parse("file:///c.eventb").unwrap();
        let machine_uri = Url::parse("file:///m.eventb").unwrap();
        let context = "CONTEXT C\nEND";
        let indexed_machine = "MACHINE M\nSEES C\nEND";
        let current_machine = "SEES /* 😀 */ C\nVARIABLES\n    C\n";

        let manager = CrossReferenceManager::new();
        manager.update_component(context_uri.to_string(), context);
        manager.update_component(machine_uri.to_string(), indexed_machine);
        let documents = DocumentManager::new();
        documents.open(context_uri.clone(), 1, context.to_string());
        documents.open(machine_uri.clone(), 2, current_machine.to_string());

        let loader = ComponentLoader::new(&manager, Some(&documents));
        let occurrences = loader.component_occurrences("C");
        assert_eq!(occurrences.len(), 2);
        assert!(occurrences.iter().any(|occurrence| {
            occurrence.uri == context_uri
                && occurrence.range.start == crate::lsp_types::Position::new(0, 8)
        }));
        assert!(occurrences.iter().any(|occurrence| {
            occurrence.uri == machine_uri
                && occurrence.range.start
                    == crate::position::offset_to_position(
                        current_machine,
                        current_machine.find("C\n").unwrap(),
                    )
        }));
    }
}
