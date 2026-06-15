//! Document management
//!
//! This module handles in-memory storage of open documents, text synchronization,
//! and provides efficient text editing operations.

use crate::lsp_types::{Position, TextDocumentContentChangeEvent, Url};
use dashmap::DashMap;
use ropey::Rope;
use rossi::{Component, ParseResult};
use std::sync::Arc;
use std::time::Instant;

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
/// single source of truth every language feature reads. The document is parsed
/// once whenever its text changes (open/change), so features never re-parse it
/// themselves and always agree on the same (recovery-tolerant) AST.
pub struct DocumentManager {
    documents: DashMap<Url, Document>,
    /// Text + recovered parse of each open document, refreshed on every edit.
    /// Wrapped in `Arc` so readers take a cheap, self-consistent snapshot.
    parses: DashMap<Url, Arc<ParsedDocument>>,
}

/// Represents a single document
pub struct Document {
    /// Document URI
    #[allow(dead_code)]
    pub uri: Url,

    /// Language ID (should be "eventb")
    #[allow(dead_code)]
    pub language_id: String,

    /// Document version (incremented on each change)
    pub version: i32,

    /// Text content (efficient rope data structure)
    pub text: Rope,

    /// Last modification timestamp
    pub last_modified: Instant,
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
    pub fn open(&self, uri: Url, language_id: String, version: i32, text: String) {
        let rope = Rope::from_str(&text);
        let document = Document {
            uri: uri.clone(),
            language_id,
            version,
            text: rope,
            last_modified: Instant::now(),
        };
        self.documents.insert(uri.clone(), document);
        self.store_parse(uri, text);
    }

    /// Parse `text` with recovery and store it as `uri`'s snapshot. The text is
    /// kept alongside the parse so readers never pair it with a different
    /// version's AST. Shared by `open` and `change` so both parse identically.
    fn store_parse(&self, uri: Url, text: String) {
        let parse = rossi::parse_components_with_recovery(&text);
        self.parses
            .insert(uri, Arc::new(ParsedDocument { text, parse }));
    }

    /// Update document with incremental changes
    pub fn change(&self, uri: &Url, version: i32, changes: Vec<TextDocumentContentChangeEvent>) {
        if let Some(mut doc) = self.documents.get_mut(uri) {
            doc.version = version;
            doc.last_modified = Instant::now();

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

        // Refresh the stored parse from the new text. The `get_mut` guard above
        // is dropped, so `get_text` can take its own read lock without deadlock.
        if let Some(text) = self.get_text(uri) {
            self.store_parse(uri.clone(), text);
        }
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
    pub fn parse_result(&self, uri: &Url) -> Option<Arc<ParsedDocument>> {
        self.parses.get(uri).map(|entry| Arc::clone(entry.value()))
    }

    /// Get document text as string
    pub fn get_text(&self, uri: &Url) -> Option<String> {
        self.documents.get(uri).map(|doc| doc.text.to_string())
    }

    /// Get document
    #[allow(dead_code)]
    pub fn get(&self, uri: &Url) -> Option<dashmap::mapref::one::Ref<'_, Url, Document>> {
        self.documents.get(uri)
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
        manager.open(
            uri.clone(),
            "rossi".to_string(),
            1,
            "CONTEXT test\nEND\n".to_string(),
        );

        // Check document exists
        assert!(manager.get(&uri).is_some());
        assert_eq!(manager.get_text(&uri).unwrap(), "CONTEXT test\nEND\n");

        // Close document
        manager.close(&uri);
        assert!(manager.get(&uri).is_none());
    }

    #[test]
    fn stored_parse_tracks_the_document_lifecycle() {
        let manager = DocumentManager::new();
        let uri = Url::parse("file:///test.eventb").unwrap();

        // Open populates the stored parse.
        manager.open(
            uri.clone(),
            "rossi".to_string(),
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
    fn stored_parse_recovers_a_local_error() {
        // A document with a broken predicate still yields a partial parse and
        // its errors from the store — the single source of truth diagnostics
        // and the symbol features both read.
        let manager = DocumentManager::new();
        let uri = Url::parse("file:///broken.eventb").unwrap();
        manager.open(
            uri.clone(),
            "rossi".to_string(),
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
        manager.open(
            uri.clone(),
            "rossi".to_string(),
            1,
            "CONTEXT test\nEND\n".to_string(),
        );

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
        manager.open(uri.clone(), "rossi".to_string(), 1, "𝔹x\n".to_string());

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
        manager.open(
            uri.clone(),
            "rossi".to_string(),
            1,
            "CONTEXT test\nEND\n".to_string(),
        );

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
