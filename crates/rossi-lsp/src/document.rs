//! Document management
//!
//! This module handles in-memory storage of open documents, text synchronization,
//! and provides efficient text editing operations.

use crate::lsp_types::{Position, TextDocumentContentChangeEvent, Url};
use dashmap::DashMap;
use ropey::Rope;
use std::time::Instant;

/// Manages all open documents
pub struct DocumentManager {
    documents: DashMap<Url, Document>,
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
        self.documents.insert(uri, document);
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
    }

    /// Close a document
    pub fn close(&self, uri: &Url) {
        self.documents.remove(uri);
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
