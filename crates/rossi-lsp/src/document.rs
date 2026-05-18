//! Document management
//!
//! This module handles in-memory storage of open documents, text synchronization,
//! and provides efficient text editing operations.

use dashmap::DashMap;
use lsp_types::{Position, Range, TextDocumentContentChangeEvent, Url};
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

    /// Convert LSP Position to byte offset
    fn position_to_offset(&self, rope: &Rope, position: Position) -> usize {
        let line_idx = position.line as usize;
        let col_idx = position.character as usize;

        // Ensure line index is valid
        if line_idx >= rope.len_lines() {
            return rope.len_chars();
        }

        let line_start = rope.line_to_char(line_idx);
        let line_end = if line_idx + 1 < rope.len_lines() {
            rope.line_to_char(line_idx + 1)
        } else {
            rope.len_chars()
        };

        // Ensure column index is valid
        let line_length = line_end - line_start;
        let offset = line_start + col_idx.min(line_length);

        offset.min(rope.len_chars())
    }

    /// Convert byte offset to LSP Position
    #[allow(dead_code)]
    pub fn offset_to_position(&self, rope: &Rope, offset: usize) -> Position {
        let offset = offset.min(rope.len_chars());
        let line_idx = rope.char_to_line(offset);
        let line_start = rope.line_to_char(line_idx);
        let col_idx = offset - line_start;
        Position::new(line_idx as u32, col_idx as u32)
    }

    /// Convert LSP Range to byte range
    #[allow(dead_code)]
    pub fn lsp_range_to_offsets(&self, rope: &Rope, range: Range) -> (usize, usize) {
        let start = self.position_to_offset(rope, range.start);
        let end = self.position_to_offset(rope, range.end);
        (start, end)
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
    fn test_offset_to_position() {
        let manager = DocumentManager::new();
        let text = "line 1\nline 2\nline 3";
        let rope = Rope::from_str(text);

        // Line starts
        assert_eq!(manager.offset_to_position(&rope, 0), Position::new(0, 0));
        assert_eq!(manager.offset_to_position(&rope, 7), Position::new(1, 0));
        assert_eq!(manager.offset_to_position(&rope, 14), Position::new(2, 0));

        // Mid-line
        assert_eq!(manager.offset_to_position(&rope, 3), Position::new(0, 3));
        assert_eq!(manager.offset_to_position(&rope, 10), Position::new(1, 3));
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
