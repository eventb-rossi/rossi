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

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use rossi::Component;

use crate::cross_references::CrossReferenceManager;
use crate::document::{DocumentManager, ParsedDocument};
use crate::lsp_types::Url;

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
        &self.doc.text
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
}

impl<'a> ComponentLoader<'a> {
    /// Create a loader over `manager`'s workspace index, preferring open
    /// documents from `documents` when available.
    pub fn new(manager: &'a CrossReferenceManager, documents: Option<&'a DocumentManager>) -> Self {
        Self {
            manager,
            documents,
            cache: RefCell::new(HashMap::new()),
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
            return Some(doc);
        }
        let doc = self.build(uri)?;
        self.cache
            .borrow_mut()
            .insert(uri.clone(), Arc::clone(&doc));
        Some(doc)
    }

    /// Parse the file at `uri` from the open-document store or disk. Never
    /// touches the cache itself.
    fn build(&self, uri: &Url) -> Option<Arc<ParsedDocument>> {
        if let Some(doc) = self.documents.and_then(|dm| dm.parse_result(uri)) {
            return Some(doc);
        }
        let path = uri.to_file_path().ok()?;
        let text = std::fs::read_to_string(path).ok()?;
        let parse = rossi::parse_components_with_recovery(&text);
        Some(Arc::new(ParsedDocument { text, parse }))
    }

    /// Load the component named `name`: resolve its file, get that file's parsed
    /// snapshot, and locate the component within it.
    ///
    /// The first component of that name is chosen, matching the lookup the
    /// parser-based helpers performed before this loader existed.
    pub fn load(&self, name: &str) -> Option<LoadedComponent> {
        let uri = Url::parse(&self.manager.find_component_uri(name)?).ok()?;
        let doc = self.parsed(&uri)?;
        let index = doc.components().iter().position(|c| c.name() == name)?;
        Some(LoadedComponent { uri, doc, index })
    }
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
}
