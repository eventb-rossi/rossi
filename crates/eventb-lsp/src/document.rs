//! Document management
//!
//! This module handles in-memory storage of open documents, text synchronization,
//! and provides efficient text editing operations.

use crate::lsp_types::{Position, TextDocumentContentChangeEvent, Url};
use dashmap::DashMap;
use ropey::Rope;
use rossi::{Component, ParseResult};
use std::sync::Arc;

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
/// next [`Self::parse_result`] when the stored snapshot lags the current
/// version. So features never re-parse the document themselves, always agree on
/// the same (recovery-tolerant) AST, and a burst of keystrokes parses at most
/// once — when analysis finally reads the parse — rather than once per edit.
pub struct DocumentManager {
    documents: DashMap<Url, Document>,
    /// Each open document's recovered parse, tagged with the document version it
    /// was produced from. `parse_result` reparses when this version lags the
    /// document's. Wrapped in `Arc` so readers take a cheap, self-consistent
    /// snapshot.
    parses: DashMap<Url, (i32, Arc<ParsedDocument>)>,
}

/// Represents a single document
struct Document {
    /// Document version (incremented on each change)
    version: i32,

    /// Text content (efficient rope data structure)
    text: Rope,
}

impl DocumentManager {
    /// Create a new document manager
    pub fn new() -> Self {
        Self {
            documents: DashMap::new(),
            parses: DashMap::new(),
        }
    }

    /// Open a new document
    pub fn open(&self, uri: Url, version: i32, text: String) {
        let rope = Rope::from_str(&text);
        let document = Document {
            version,
            text: rope,
        };
        // Drop any parse left over from a previous open of this URI (a re-open
        // without an intervening close, possibly with a colliding version):
        // otherwise the version-keyed fast path in `parse_result` could return
        // the stale parse for the freshly-opened text.
        self.parses.remove(&uri);
        self.documents.insert(uri, document);
        // The parse is produced lazily on the first `parse_result` (which
        // `didOpen` requests right away to publish diagnostics), keeping a single
        // parse entry point.
    }

    /// Update document with incremental changes
    pub fn change(&self, uri: &Url, version: i32, changes: Vec<TextDocumentContentChangeEvent>) {
        if let Some(mut doc) = self.documents.get_mut(uri) {
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
        }

        // The stored parse is now stale; `parse_result` reparses lazily on the
        // next read. Deferring the parse here is what lets a burst of keystrokes
        // parse at most once instead of once per edit.
    }

    /// Close a document
    pub fn close(&self, uri: &Url) {
        self.documents.remove(uri);
        self.parses.remove(uri);
    }

    /// The recovered parse of `uri` (text + components + errors), if the
    /// document is open. A cheap `Arc` snapshot of the single source of truth:
    /// the bundled text matches the AST exactly, so a reader slicing the text by
    /// a component/error span can never go out of bounds — even if a concurrent
    /// edit has since replaced the entry. Every feature reads this rather than
    /// parsing (or re-`get_text`-ing) the document itself.
    ///
    /// The parse is produced lazily and memoised per document version: when the
    /// stored snapshot already matches the document's current version this is a
    /// cheap `Arc` clone; when an edit has left it stale (or it is absent), the
    /// current text is parsed once here and the snapshot refreshed. So reads
    /// always see a parse consistent with the latest text without the edit path
    /// having to parse on every keystroke.
    pub fn parse_result(&self, uri: &Url) -> Option<Arc<ParsedDocument>> {
        let current_version = self.documents.get(uri).map(|doc| doc.version)?;

        // Fast path: the stored snapshot already matches the document version.
        // Drop the read guard before any insert below to avoid a shard deadlock.
        let fresh = self.parses.get(uri).and_then(|entry| {
            let (stored_version, parsed) = entry.value();
            (*stored_version == current_version).then(|| Arc::clone(parsed))
        });
        if let Some(parsed) = fresh {
            return Some(parsed);
        }

        // Stale (an edit deferred its parse) or missing: reparse the current
        // text under a single read and refresh the snapshot. The version is read
        // alongside the text so the snapshot is tagged with exactly what it was
        // produced from.
        let (version, text) = {
            let doc = self.documents.get(uri)?;
            (doc.version, doc.text.to_string())
        };
        let parse = rossi::parse_components_with_recovery(&text);
        let parsed = Arc::new(ParsedDocument { text, parse });
        self.parses
            .insert(uri.clone(), (version, Arc::clone(&parsed)));
        Some(parsed)
    }

    /// The current LSP version of `uri`, if open. Lets a deferred consumer (the
    /// debounced analysis) check whether the document it was scheduled for is
    /// still the latest, and tag its publish with the state it actually ran on.
    pub fn version(&self, uri: &Url) -> Option<i32> {
        self.documents.get(uri).map(|doc| doc.version)
    }

    /// Get document text as string
    pub fn get_text(&self, uri: &Url) -> Option<String> {
        self.documents.get(uri).map(|doc| doc.text.to_string())
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
    fn parse_result_is_lazy_and_memoised_per_version() {
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
