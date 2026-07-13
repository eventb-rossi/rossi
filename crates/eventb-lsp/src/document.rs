//! Document management
//!
//! This module handles in-memory storage of open documents, text synchronization,
//! and provides efficient text editing operations.

use crate::lsp_types::{Position, TextDocumentContentChangeEvent, Url};
use dashmap::DashMap;
use parking_lot::RwLock;
use ropey::Rope;
use rossi::{Component, ParseResult};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

/// A document's text together with its recovered parse, captured as one
/// snapshot. Bundling the two means every feature that reads the store gets
/// spans that index the exact text they were produced from — there is no window
/// where a reader pairs one version's `text` with another version's AST (which
/// would slice out of bounds, since requests and `didChange` run concurrently).
pub struct ParsedDocument {
    /// The text this parse was produced from.
    pub text: String,
    /// Recovered components + errors for [`Self::text`].
    pub parse: ParseResult<Vec<Component>>,
}

impl ParsedDocument {
    /// The recovered components, or an empty slice when nothing parsed. Saves
    /// every reader from spelling out `parse.component.as_deref().unwrap_or_default()`.
    pub fn components(&self) -> &[Component] {
        self.parse.component.as_deref().unwrap_or_default()
    }
}

/// Manages all open documents.
///
/// Besides the text, the manager owns the document's recovered parse — the
/// single source of truth every language feature reads. The parse is produced
/// lazily: an edit only updates the text, and the document is (re)parsed on the
/// next [`Self::parse_result`] after the edit invalidates the cached snapshot.
/// So features never re-parse the document themselves, always agree on
/// the same (recovery-tolerant) AST, and a burst of keystrokes parses at most
/// once — when analysis finally reads the parse — rather than once per edit.
pub struct DocumentManager {
    documents: DashMap<Url, Arc<RwLock<Document>>>,
    next_revision: AtomicU64,
}

/// Represents a single document
struct Document {
    /// Whether this state is still the map's live entry for its URI.
    open: bool,

    /// Document version (incremented on each change)
    version: i32,

    /// Text content (efficient rope data structure)
    text: Rope,

    /// Unique internal revision, including reopens whose LSP version collides.
    revision: u64,

    /// Lazily populated parse for this exact text/version/revision state.
    parse: Arc<OnceLock<Arc<ParsedDocument>>>,
}

impl DocumentManager {
    /// Create a new document manager
    pub fn new() -> Self {
        Self {
            documents: DashMap::new(),
            next_revision: AtomicU64::new(1),
        }
    }

    /// Open a new document
    pub fn open(&self, uri: Url, version: i32, text: String) {
        let rope = Rope::from_str(&text);
        let document = Document {
            open: true,
            version,
            text: rope,
            revision: self.next_revision(),
            parse: Arc::new(OnceLock::new()),
        };
        if let Some(previous) = self.documents.insert(uri, Arc::new(RwLock::new(document))) {
            previous.write().open = false;
        }
        // The parse is produced lazily on the first `parse_result` (which
        // `didOpen` requests right away to publish diagnostics), keeping a single
        // parse entry point.
    }

    /// Update document with incremental changes
    pub fn change(&self, uri: &Url, version: i32, changes: Vec<TextDocumentContentChangeEvent>) {
        if let Some(document) = self.document(uri) {
            let mut doc = document.write();
            if !doc.open {
                return;
            }
            doc.version = version;

            for change in changes {
                match change.range {
                    Some(range) => {
                        // Incremental change
                        let start = self.position_to_offset(&doc.text, range.start);
                        let end = self.position_to_offset(&doc.text, range.end);

                        // Remove the old text
                        if start < end && end <= doc.text.len_chars() {
                            doc.text.remove(start..end);
                        }

                        // Insert the new text
                        if start <= doc.text.len_chars() {
                            doc.text.insert(start, &change.text);
                        }
                    }
                    None => {
                        // Full document sync
                        doc.text = Rope::from_str(&change.text);
                    }
                }
            }

            doc.revision = self.next_revision();
            doc.parse = Arc::new(OnceLock::new());
        }

        // The stored parse is now stale; `parse_result` reparses lazily on the
        // next read. Deferring the parse here is what lets a burst of keystrokes
        // parse at most once instead of once per edit.
    }

    /// Close a document
    pub fn close(&self, uri: &Url) {
        let Some(document) = self.document(uri) else {
            return;
        };
        document.write().open = false;
        self.documents
            .remove_if(uri, |_, current| Arc::ptr_eq(current, &document));
    }

    /// The recovered parse of `uri` (text + components + errors), if the
    /// document is open. A cheap `Arc` snapshot of the single source of truth:
    /// the bundled text matches the AST exactly, so a reader slicing the text by
    /// a component/error span can never go out of bounds — even if a concurrent
    /// edit has since replaced the entry. Every feature reads this rather than
    /// parsing (or re-`get_text`-ing) the document itself.
    ///
    /// The parse is produced lazily and memoised per internal document revision:
    /// a cached snapshot is a cheap `Arc` clone, while concurrent misses share
    /// one initialization. An edit swaps in a fresh empty cache without waiting
    /// for an older parse, and that older caller returns `None` when superseded.
    pub fn parse_result(&self, uri: &Url) -> Option<Arc<ParsedDocument>> {
        self.parse_result_with_hook(uri, || {}, rossi::parse_components_with_recovery)
    }

    fn parse_result_with_hook(
        &self,
        uri: &Url,
        before_initialize: impl FnOnce(),
        parse: impl Fn(&str) -> ParseResult<Vec<Component>>,
    ) -> Option<Arc<ParsedDocument>> {
        let document = self.document(uri)?;
        let (parse_cell, text) = {
            let doc = document.read();
            if !doc.open {
                return None;
            }
            if let Some(parsed) = doc.parse.get() {
                return Some(Arc::clone(parsed));
            }
            (Arc::clone(&doc.parse), doc.text.clone())
        };

        // Callers racing on the same revision share this initialization. An
        // edit swaps in a fresh cell without waiting for the old parse.
        before_initialize();
        let parsed = Arc::clone(parse_cell.get_or_init(|| {
            let text = text.to_string();
            Arc::new(ParsedDocument {
                parse: parse(&text),
                text,
            })
        }));

        // A superseded caller bows out instead of chasing the new revision and
        // bypassing that edit's debounce window. Its detached cell is harmless.
        let current_document = self.document(uri)?;
        let current = current_document.read();
        if !current.open {
            return None;
        }
        let current_parse = current.parse.get()?;
        Arc::ptr_eq(current_parse, &parsed).then_some(parsed)
    }

    /// Run `commit` only while `snapshot` is still this open document's exact
    /// cached state. Holding the document guard prevents an edit, close, or
    /// reopen from interleaving with the synchronous commit.
    pub(crate) fn with_current_snapshot<T>(
        &self,
        uri: &Url,
        snapshot: &Arc<ParsedDocument>,
        commit: impl FnOnce(i32) -> T,
    ) -> Option<T> {
        let document = self.document(uri)?;
        let doc = document.read();
        if !doc.open {
            return None;
        }
        let current = doc.parse.get()?;
        Arc::ptr_eq(current, snapshot).then(|| commit(doc.version))
    }

    fn next_revision(&self) -> u64 {
        self.next_revision.fetch_add(1, Ordering::Relaxed)
    }

    fn document(&self, uri: &Url) -> Option<Arc<RwLock<Document>>> {
        self.documents
            .get(uri)
            .map(|entry| Arc::clone(entry.value()))
    }

    /// The current internal revision of `uri`, including lifecycle changes.
    pub(crate) fn revision(&self, uri: &Url) -> Option<u64> {
        let document = self.document(uri)?;
        let doc = document.read();
        if !doc.open {
            return None;
        }
        let revision = doc.revision;
        Some(revision)
    }

    /// The current LSP version of `uri`, if open.
    pub fn version(&self, uri: &Url) -> Option<i32> {
        let document = self.document(uri)?;
        let doc = document.read();
        if !doc.open {
            return None;
        }
        let version = doc.version;
        Some(version)
    }

    /// Get document text as string
    pub fn get_text(&self, uri: &Url) -> Option<String> {
        let document = self.document(uri)?;
        let doc = document.read();
        if !doc.open {
            return None;
        }
        let text = doc.text.to_string();
        Some(text)
    }

    /// URIs of every currently open document.
    pub(crate) fn all_uris(&self) -> Vec<Url> {
        self.documents
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Convert an LSP [`Position`] to a `ropey` char index.
    ///
    /// The rope-based twin of [`crate::position::position_to_offset`]: the
    /// column is interpreted as UTF-16 code units (the LSP convention) but the
    /// result is a char index, since the rope splices used for incremental edits
    /// are char-indexed. A column past the line clamps to the line's end.
    fn position_to_offset(&self, rope: &Rope, position: Position) -> usize {
        let line_idx = position.line as usize;

        // Ensure line index is valid
        if line_idx >= rope.len_lines() {
            return rope.len_chars();
        }

        let line_start_char = rope.line_to_char(line_idx);
        let line_end_char = if line_idx + 1 < rope.len_lines() {
            rope.line_to_char(line_idx + 1)
        } else {
            rope.len_chars()
        };

        // Map the UTF-16 column onto a char index, clamped to the line's content.
        let line_start_cu = rope.char_to_utf16_cu(line_start_char);
        let line_end_cu = rope.char_to_utf16_cu(line_end_char);
        let target_cu = (line_start_cu + position.character as usize).min(line_end_cu);
        rope.utf16_cu_to_char(target_cu)
    }
}

impl Default for DocumentManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp_types::Range;
    use std::sync::Barrier;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;
    use std::thread;

    #[test]
    fn test_document_manager_open_close() {
        let manager = DocumentManager::new();
        let uri = Url::parse("file:///test.eventb").unwrap();

        // Open document
        manager.open(uri.clone(), 1, "CONTEXT test\nEND\n".to_string());

        // Check document exists
        assert_eq!(manager.get_text(&uri).unwrap(), "CONTEXT test\nEND\n");

        // Close document
        manager.close(&uri);
        assert!(manager.get_text(&uri).is_none());
    }

    #[test]
    fn stored_parse_tracks_the_document_lifecycle() {
        let manager = DocumentManager::new();
        let uri = Url::parse("file:///test.eventb").unwrap();

        // Open populates the stored parse.
        manager.open(
            uri.clone(),
            1,
            "CONTEXT C0\nCONSTANTS\n    k\nEND\n".to_string(),
        );
        let parsed = manager.parse_result(&uri).expect("parse stored on open");
        assert_eq!(manager.version(&uri), Some(1));
        // The bundled text matches what was opened (so span-indexing is safe).
        assert_eq!(parsed.text, "CONTEXT C0\nCONSTANTS\n    k\nEND\n");
        let names: Vec<&str> = parsed
            .parse
            .component
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|c| c.name())
            .collect();
        assert_eq!(names, vec!["C0"]);

        // A full-document change refreshes both text and parse together.
        manager.change(
            &uri,
            2,
            vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "MACHINE M1\nEND\n".to_string(),
            }],
        );
        let parsed = manager
            .parse_result(&uri)
            .expect("parse refreshed on change");
        assert_eq!(manager.version(&uri), Some(2));
        assert_eq!(parsed.text, "MACHINE M1\nEND\n");
        assert_eq!(
            parsed.parse.component.as_deref().unwrap_or_default()[0].name(),
            "M1"
        );

        // Close drops it.
        manager.close(&uri);
        assert!(manager.parse_result(&uri).is_none());
    }

    #[test]
    fn parse_result_is_lazy_and_memoised_per_revision() {
        let manager = DocumentManager::new();
        let uri = Url::parse("file:///lazy.eventb").unwrap();
        manager.open(uri.clone(), 1, "CONTEXT C0\nEND\n".to_string());

        // Two reads at the same version share one parse — the second is a cheap
        // `Arc` clone, not a re-parse.
        let first = manager.parse_result(&uri).unwrap();
        let second = manager.parse_result(&uri).unwrap();
        assert!(
            Arc::ptr_eq(&first, &second),
            "same version reuses one parse"
        );

        // An edit only updates the text (the parse is deferred); the next read
        // produces a fresh snapshot for the new version and reflects the new
        // text.
        manager.change(
            &uri,
            2,
            vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "MACHINE M1\nEND\n".to_string(),
            }],
        );
        let third = manager.parse_result(&uri).unwrap();
        assert!(
            !Arc::ptr_eq(&second, &third),
            "an edit yields a fresh parse on the next read"
        );
        assert_eq!(third.components()[0].name(), "M1");

        // The refreshed snapshot is itself memoised at the new version.
        let fourth = manager.parse_result(&uri).unwrap();
        assert!(Arc::ptr_eq(&third, &fourth));
    }

    #[test]
    fn reopen_with_colliding_version_does_not_serve_stale_parse() {
        let manager = DocumentManager::new();
        let uri = Url::parse("file:///reopen.eventb").unwrap();

        manager.open(uri.clone(), 1, "CONTEXT A\nEND\n".to_string());
        assert_eq!(
            manager.parse_result(&uri).unwrap().components()[0].name(),
            "A"
        );

        // Re-open the same URI (no intervening close) with new text but the SAME
        // version number. Unless `open` drops the prior parse, the version-keyed
        // fast path would hand back the stale "A" parse for the new text.
        manager.open(uri.clone(), 1, "CONTEXT B\nEND\n".to_string());
        let parsed = manager.parse_result(&uri).unwrap();
        assert_eq!(parsed.text, "CONTEXT B\nEND\n");
        assert_eq!(parsed.components()[0].name(), "B");
    }

    #[test]
    fn slower_old_parse_cannot_overwrite_a_newer_snapshot() {
        let manager = Arc::new(DocumentManager::new());
        let uri = Url::parse("file:///concurrent.eventb").unwrap();
        manager.open(uri.clone(), 1, "CONTEXT old\nEND\n".to_string());

        let (parse_started_tx, parse_started_rx) = mpsc::channel();
        let (resume_parse_tx, resume_parse_rx) = mpsc::channel();
        let worker_manager = Arc::clone(&manager);
        let worker_uri = uri.clone();
        let worker = thread::spawn(move || {
            worker_manager.parse_result_with_hook(
                &worker_uri,
                || {},
                |text| {
                    parse_started_tx.send(()).unwrap();
                    resume_parse_rx.recv().unwrap();
                    rossi::parse_components_with_recovery(text)
                },
            )
        });

        parse_started_rx.recv().unwrap();
        manager.change(
            &uri,
            2,
            vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "CONTEXT new\nEND\n".to_string(),
            }],
        );
        let current = manager.parse_result(&uri).unwrap();
        resume_parse_tx.send(()).unwrap();
        let delayed = worker.join().unwrap();

        assert!(delayed.is_none(), "the superseded parse must bow out");
        assert_eq!(manager.version(&uri), Some(2));
        assert_eq!(current.components()[0].name(), "new");
    }

    #[test]
    fn concurrent_misses_parse_a_revision_once() {
        let manager = Arc::new(DocumentManager::new());
        let uri = Url::parse("file:///single-flight.eventb").unwrap();
        manager.open(uri.clone(), 1, "CONTEXT shared\nEND\n".to_string());

        let parses = Arc::new(AtomicUsize::new(0));
        let ready = Arc::new(Barrier::new(3));
        let parse_started = Arc::new(Barrier::new(2));
        let mut workers = Vec::new();

        for _ in 0..2 {
            let manager = Arc::clone(&manager);
            let uri = uri.clone();
            let parses = Arc::clone(&parses);
            let ready = Arc::clone(&ready);
            let parse_started = Arc::clone(&parse_started);
            workers.push(thread::spawn(move || {
                manager.parse_result_with_hook(
                    &uri,
                    || {
                        ready.wait();
                    },
                    |text| {
                        parses.fetch_add(1, Ordering::Relaxed);
                        parse_started.wait();
                        rossi::parse_components_with_recovery(text)
                    },
                )
            }));
        }

        // Release both callers at the cache initializer together. Exactly one
        // enters the parser; the other waits for its shared result.
        ready.wait();
        parse_started.wait();

        let first = workers.remove(0).join().unwrap().unwrap();
        let second = workers.remove(0).join().unwrap().unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(parses.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn only_the_exact_current_snapshot_can_commit() {
        let manager = DocumentManager::new();
        let uri = Url::parse("file:///commit.eventb").unwrap();
        manager.open(uri.clone(), 1, "CONTEXT old\nEND\n".to_string());
        let stale = manager.parse_result(&uri).unwrap();

        manager.change(
            &uri,
            2,
            vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "CONTEXT current\nEND\n".to_string(),
            }],
        );
        let mut committed = false;
        assert!(
            manager
                .with_current_snapshot(&uri, &stale, |_| committed = true)
                .is_none()
        );
        assert!(!committed, "an edited snapshot must not commit");

        let current = manager.parse_result(&uri).unwrap();
        assert_eq!(
            manager.with_current_snapshot(&uri, &current, |_| "committed"),
            Some("committed")
        );

        manager.open(uri.clone(), 2, "CONTEXT reopened\nEND\n".to_string());
        assert!(
            manager
                .with_current_snapshot(&uri, &current, |_| ())
                .is_none(),
            "a same-version reopen must reject the old snapshot"
        );

        let reopened = manager.parse_result(&uri).unwrap();
        manager.close(&uri);
        assert!(
            manager
                .with_current_snapshot(&uri, &reopened, |_| ())
                .is_none(),
            "a closed document must reject its former snapshot"
        );
    }

    #[test]
    fn stored_parse_recovers_a_local_error() {
        // A document with a broken predicate still yields a partial parse and
        // its errors from the store — the single source of truth diagnostics
        // and the symbol features both read.
        let manager = DocumentManager::new();
        let uri = Url::parse("file:///broken.eventb").unwrap();
        manager.open(
            uri.clone(),
            1,
            "MACHINE m\nVARIABLES\n    counter\nINVARIANTS\n    @i counter ∈\nEND\n".to_string(),
        );

        let parsed = manager.parse_result(&uri).unwrap();
        assert!(
            !parsed.parse.errors.is_empty(),
            "the broken invariant is reported"
        );
        let machine = parsed.parse.component.as_deref().unwrap_or_default();
        assert_eq!(machine[0].name(), "m");
    }

    #[test]
    fn test_position_to_offset() {
        let manager = DocumentManager::new();
        let text = "line 1\nline 2\nline 3";
        let rope = Rope::from_str(text);

        // Line start
        assert_eq!(manager.position_to_offset(&rope, Position::new(0, 0)), 0);
        assert_eq!(manager.position_to_offset(&rope, Position::new(1, 0)), 7);
        assert_eq!(manager.position_to_offset(&rope, Position::new(2, 0)), 14);

        // Mid-line
        assert_eq!(manager.position_to_offset(&rope, Position::new(0, 3)), 3);
        assert_eq!(manager.position_to_offset(&rope, Position::new(1, 3)), 10);
    }

    #[test]
    fn test_incremental_change() {
        let manager = DocumentManager::new();
        let uri = Url::parse("file:///test.eventb").unwrap();

        // Open document
        manager.open(uri.clone(), 1, "CONTEXT test\nEND\n".to_string());

        // Make an incremental change: insert " example" after "test"
        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position::new(0, 12),
                end: Position::new(0, 12),
            }),
            range_length: None,
            text: " example".to_string(),
        }];

        manager.change(&uri, 2, changes);

        assert_eq!(
            manager.get_text(&uri).unwrap(),
            "CONTEXT test example\nEND\n"
        );
    }

    #[test]
    fn incremental_change_after_astral_char_is_utf16() {
        // `𝔹` (U+1D539) is one char but *two* UTF-16 code units. A client sends
        // UTF-16 columns, so the position just after `𝔹` is column 2, not 1.
        // Char-indexing the column would splice in the wrong place.
        let manager = DocumentManager::new();
        let uri = Url::parse("file:///test.eventb").unwrap();
        manager.open(uri.clone(), 1, "𝔹x\n".to_string());

        // Insert "Y" at UTF-16 column 2 = between `𝔹` and `x`.
        let changes = vec![TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position::new(0, 2),
                end: Position::new(0, 2),
            }),
            range_length: None,
            text: "Y".to_string(),
        }];
        manager.change(&uri, 2, changes);

        assert_eq!(manager.get_text(&uri).unwrap(), "𝔹Yx\n");
    }

    #[test]
    fn test_full_document_sync() {
        let manager = DocumentManager::new();
        let uri = Url::parse("file:///test.eventb").unwrap();

        // Open document
        manager.open(uri.clone(), 1, "CONTEXT test\nEND\n".to_string());

        // Full document update
        let changes = vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "MACHINE new_machine\nEND\n".to_string(),
        }];

        manager.change(&uri, 2, changes);

        assert_eq!(
            manager.get_text(&uri).unwrap(),
            "MACHINE new_machine\nEND\n"
        );
    }
}
