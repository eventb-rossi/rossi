//! Shared text utilities for comment-aware code analysis
//!
//! Provides `CommentTracker` for identifying code vs comment regions in Event-B source text.

/// A character-index span within a line that is code (not inside a comment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeSpan {
    /// Start character index (inclusive)
    pub start: usize,
    /// End character index (exclusive)
    pub end: usize,
}

/// Tracks block comment state across lines and returns code-only spans.
///
/// Call `code_spans` for each line in order; the tracker carries `in_block_comment`
/// state so multi-line `/* ... */` comments are handled correctly.
pub struct CommentTracker {
    in_block_comment: bool,
}

impl Default for CommentTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl CommentTracker {
    pub fn new() -> Self {
        Self {
            in_block_comment: false,
        }
    }

    /// Returns character ranges within `chars` that are code (not comments).
    ///
    /// Handles `//` (rest of line), `/* ... */` (inline and multi-line).
    /// Must be called for every line in order to maintain block comment state.
    pub fn code_spans(&mut self, chars: &[char]) -> Vec<CodeSpan> {
        let mut spans = Vec::new();
        let len = chars.len();
        let mut i = 0;

        if self.in_block_comment {
            // Scan for closing */
            while i + 1 < len {
                if chars[i] == '*' && chars[i + 1] == '/' {
                    self.in_block_comment = false;
                    i += 2;
                    break;
                }
                i += 1;
            }
            // If we reached end without finding */, check last char
            if self.in_block_comment {
                return spans;
            }
        }

        let mut code_start = i;

        while i < len {
            if i + 1 < len && chars[i] == '/' && chars[i + 1] == '/' {
                // Line comment — rest of line is comment
                if code_start < i {
                    spans.push(CodeSpan {
                        start: code_start,
                        end: i,
                    });
                }
                return spans;
            }

            if i + 1 < len && chars[i] == '/' && chars[i + 1] == '*' {
                // Block comment start
                if code_start < i {
                    spans.push(CodeSpan {
                        start: code_start,
                        end: i,
                    });
                }
                self.in_block_comment = true;
                i += 2;

                // Scan for closing */ within this line
                while i + 1 < len {
                    if chars[i] == '*' && chars[i + 1] == '/' {
                        self.in_block_comment = false;
                        i += 2;
                        break;
                    }
                    i += 1;
                }
                // If still in block comment and reached last char without match
                if self.in_block_comment {
                    return spans;
                }
                code_start = i;
                continue;
            }

            i += 1;
        }

        if code_start < len {
            spans.push(CodeSpan {
                start: code_start,
                end: len,
            });
        }

        spans
    }
}

pub fn is_identifier_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

pub fn identifier_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();

    for ch in line.chars() {
        if is_identifier_char(ch) {
            current.push(ch);
        } else if !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
    }

    if !current.is_empty() {
        words.push(current);
    }

    words
}

pub fn first_identifier_word(line: &str) -> Option<String> {
    identifier_words(line).into_iter().next()
}

pub fn event_name_from_line(line: &str) -> Option<String> {
    let words = identifier_words(line);
    let event_idx = words
        .iter()
        .position(|word| word.eq_ignore_ascii_case("EVENT"))?;

    if event_idx + 1 >= words.len() {
        return None;
    }

    Some(words[event_idx + 1].clone())
}

pub fn is_clause_boundary_keyword(word: &str) -> bool {
    [
        "MACHINE",
        "CONTEXT",
        "REFINES",
        "SEES",
        "EXTENDS",
        "SETS",
        "CONSTANTS",
        "VARIABLES",
        "AXIOMS",
        "INVARIANTS",
        "THEOREMS",
        "VARIANT",
        "EVENTS",
        "EVENT",
        "ANY",
        "WHERE",
        "WITH",
        "THEN",
        "STATUS",
        "END",
    ]
    .iter()
    .any(|keyword| word.eq_ignore_ascii_case(keyword))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(line: &str, tracker: &mut CommentTracker) -> Vec<CodeSpan> {
        let chars: Vec<char> = line.chars().collect();
        tracker.code_spans(&chars)
    }

    #[test]
    fn test_no_comments() {
        let mut t = CommentTracker::new();
        assert_eq!(
            spans("count := count + 1", &mut t),
            vec![CodeSpan { start: 0, end: 18 }]
        );
    }

    #[test]
    fn test_line_comment() {
        let mut t = CommentTracker::new();
        assert_eq!(
            spans("count := 0 // init", &mut t),
            vec![CodeSpan { start: 0, end: 11 }]
        );
    }

    #[test]
    fn test_line_comment_at_start() {
        let mut t = CommentTracker::new();
        assert_eq!(spans("// entire line is comment", &mut t), vec![]);
    }

    #[test]
    fn test_single_line_block_comment() {
        let mut t = CommentTracker::new();
        assert_eq!(
            spans("a /* comment */ b", &mut t),
            vec![
                CodeSpan { start: 0, end: 2 },
                CodeSpan { start: 15, end: 17 },
            ]
        );
    }

    #[test]
    fn test_multi_line_block_comment() {
        let mut t = CommentTracker::new();
        // Line 1: block comment starts
        assert_eq!(
            spans("code /* start", &mut t),
            vec![CodeSpan { start: 0, end: 5 }]
        );
        // Line 2: entirely inside block comment
        assert_eq!(spans("still in comment", &mut t), vec![]);
        // Line 3: block comment ends
        assert_eq!(
            spans("end */ more_code", &mut t),
            vec![CodeSpan { start: 6, end: 16 }]
        );
    }

    #[test]
    fn test_multiple_block_comments_one_line() {
        let mut t = CommentTracker::new();
        // "a /* x */ b /* y */ c"
        //  0123456789012345678901
        // code: [0,2) [9,12) [19,21)
        assert_eq!(
            spans("a /* x */ b /* y */ c", &mut t),
            vec![
                CodeSpan { start: 0, end: 2 },
                CodeSpan { start: 9, end: 12 },
                CodeSpan { start: 19, end: 21 },
            ]
        );
    }

    #[test]
    fn test_block_comment_covers_entire_line() {
        let mut t = CommentTracker::new();
        assert_eq!(spans("/* all comment */", &mut t), vec![]);
    }

    #[test]
    fn test_empty_line() {
        let mut t = CommentTracker::new();
        assert_eq!(spans("", &mut t), vec![]);
    }

    #[test]
    fn test_block_comment_state_preserved() {
        let mut t = CommentTracker::new();
        spans("/*", &mut t);
        assert_eq!(spans("inside", &mut t), vec![]);
        spans("*/", &mut t);
        assert_eq!(
            spans("outside", &mut t),
            vec![CodeSpan { start: 0, end: 7 }]
        );
    }
}
