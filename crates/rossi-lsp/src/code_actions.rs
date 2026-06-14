//! Code Actions for Event-B
//!
//! Provides quick fixes and refactorings including:
//! - Operator conversion (ASCII ↔ Unicode)
//! - Extract constant from literal
//! - Sort clauses alphabetically
//! - And more refactorings

use crate::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, CodeActionResponse,
    Position, Range, TextEdit, Url, WorkspaceEdit,
};
use crate::text_utils::line_keyword_is;
use rossi::keywords::KeywordId;
use rossi::operators;
use std::collections::HashMap;

/// LSP end position of `text` (last line index, UTF-16 length of the last line),
/// computed in a single pass over the lines.
fn document_end_position(text: &str) -> Position {
    let mut line_count: u32 = 0;
    let mut last_line_length: u32 = 0;
    for line in text.lines() {
        line_count += 1;
        last_line_length = crate::position::utf16_len(line);
    }
    Position::new(line_count.saturating_sub(1), last_line_length)
}

/// Whether any line in `text` begins with the keyword `id` (case-insensitive).
/// A parse-free probe for a component's kind or which clauses are present, so
/// the action still fires on a mid-edit document that does not yet parse.
fn has_keyword_line(text: &str, id: KeywordId) -> bool {
    text.lines().any(|line| line_keyword_is(line, id))
}

/// Provides code actions and refactorings
pub struct CodeActionProvider;

impl CodeActionProvider {
    pub fn new() -> Self {
        Self
    }

    /// Provide code actions for a given document position/range
    pub fn provide_code_actions(
        &self,
        params: &CodeActionParams,
        text: &str,
    ) -> Option<CodeActionResponse> {
        let mut actions = Vec::new();

        // Add operator conversion actions
        actions.extend(self.provide_operator_conversion_actions(params, text));

        // Add diagnostic-based quick fixes (from diagnostics in context)
        actions.extend(self.provide_diagnostic_based_actions(params, text));

        // Add missing clause actions
        actions.extend(self.provide_add_missing_clause_actions(params, text));

        // Add sort clauses action
        actions.extend(self.provide_sort_clauses_actions(params, text));

        // Add extract constant action if a literal is selected
        if let Some(action) = self.provide_extract_constant_action(params, text) {
            actions.push(action);
        }

        // Add rename event action if cursor is on an event name
        if let Some(action) = self.provide_rename_event_action(params, text) {
            actions.push(action);
        }

        if actions.is_empty() {
            None
        } else {
            Some(actions)
        }
    }

    /// Provide actions to convert operators between ASCII and Unicode
    fn provide_operator_conversion_actions(
        &self,
        params: &CodeActionParams,
        text: &str,
    ) -> Vec<CodeActionOrCommand> {
        let mut actions = Vec::new();

        // Check if we can convert the entire document to Unicode
        if self.has_ascii_operators(text)
            && let Some(action) =
                self.create_convert_to_unicode_action(&params.text_document.uri, text)
        {
            actions.push(CodeActionOrCommand::CodeAction(action));
        }

        // Check if we can convert the entire document to ASCII
        if self.has_unicode_operators(text)
            && let Some(action) =
                self.create_convert_to_ascii_action(&params.text_document.uri, text)
        {
            actions.push(CodeActionOrCommand::CodeAction(action));
        }

        // Check if we can convert just the selection. Operator detection and
        // conversion use the FULL document's comment spans (via byte offsets),
        // so a selection that opens inside a `/* */` or `//` comment keeps its
        // prose intact instead of having operator spellings rewritten.
        if params.range.start != params.range.end
            && let (Some(start), Some(end)) = (
                crate::identifier_utils::position_to_offset(text, params.range.start),
                crate::identifier_utils::position_to_offset(text, params.range.end),
            )
            && start < end
        {
            let masked = rossi::comments::mask_comments(text);
            let selected = &text[start..end];

            let conversions = [
                (
                    operators::has_ascii_operators as fn(&str) -> bool,
                    operators::convert_to_unicode as fn(&str) -> String,
                    "Convert selection to Unicode",
                ),
                (
                    operators::has_unicode_operators,
                    operators::convert_to_ascii,
                    "Convert selection to ASCII",
                ),
            ];
            for (has_operators, convert, title) in conversions {
                if has_operators(&masked[start..end]) {
                    let converted =
                        rossi::comments::map_code_segments_in_range(text, start, end, convert);
                    if let Some(action) = self.create_convert_selection_action(
                        &params.text_document.uri,
                        title,
                        converted,
                        selected,
                        &params.range,
                    ) {
                        actions.push(CodeActionOrCommand::CodeAction(action));
                    }
                }
            }
        }

        actions
    }

    /// Check if the code (comments excluded) contains ASCII operators
    fn has_ascii_operators(&self, text: &str) -> bool {
        operators::has_ascii_operators(&rossi::comments::mask_comments(text))
    }

    /// Check if the code (comments excluded) contains Unicode operators
    fn has_unicode_operators(&self, text: &str) -> bool {
        operators::has_unicode_operators(&rossi::comments::mask_comments(text))
    }

    /// Convert ASCII operators to Unicode in the given text.
    /// Comment text is never rewritten — `<=` in prose stays `<=`.
    pub fn convert_to_unicode(&self, text: &str) -> String {
        rossi::comments::map_code_segments(text, operators::convert_to_unicode)
    }

    /// Convert Unicode operators to ASCII in the given text.
    /// Comment text is never rewritten.
    pub fn convert_to_ascii(&self, text: &str) -> String {
        rossi::comments::map_code_segments(text, operators::convert_to_ascii)
    }

    /// Create action to convert entire document to Unicode
    fn create_convert_to_unicode_action(&self, uri: &Url, text: &str) -> Option<CodeAction> {
        let converted = self.convert_to_unicode(text);
        if converted == text {
            return None;
        }

        let mut changes = HashMap::new();
        changes.insert(
            uri.clone(),
            vec![TextEdit {
                range: Range {
                    start: Position::new(0, 0),
                    end: document_end_position(text),
                },
                new_text: converted,
            }],
        );

        Some(CodeAction {
            title: "Convert all operators to Unicode".to_string(),
            kind: Some(CodeActionKind::REFACTOR),
            diagnostics: None,
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                document_changes: None,
                change_annotations: None,
            }),
            command: None,
            is_preferred: Some(false),
            disabled: None,
            data: None,
        })
    }

    /// Create action to convert entire document to ASCII
    fn create_convert_to_ascii_action(&self, uri: &Url, text: &str) -> Option<CodeAction> {
        let converted = self.convert_to_ascii(text);
        if converted == text {
            return None;
        }

        let mut changes = HashMap::new();
        changes.insert(
            uri.clone(),
            vec![TextEdit {
                range: Range {
                    start: Position::new(0, 0),
                    end: document_end_position(text),
                },
                new_text: converted,
            }],
        );

        Some(CodeAction {
            title: "Convert all operators to ASCII".to_string(),
            kind: Some(CodeActionKind::REFACTOR),
            diagnostics: None,
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                document_changes: None,
                change_annotations: None,
            }),
            command: None,
            is_preferred: Some(false),
            disabled: None,
            data: None,
        })
    }

    /// Build a "Convert selection" refactor action that replaces `range` with
    /// `new_text`, or `None` when conversion changed nothing (`new_text`
    /// equals the `original` selected slice).
    fn create_convert_selection_action(
        &self,
        uri: &Url,
        title: &str,
        new_text: String,
        original: &str,
        range: &Range,
    ) -> Option<CodeAction> {
        if new_text == original {
            return None;
        }

        let mut changes = HashMap::new();
        changes.insert(
            uri.clone(),
            vec![TextEdit {
                range: *range,
                new_text,
            }],
        );

        Some(CodeAction {
            title: title.to_string(),
            kind: Some(CodeActionKind::REFACTOR),
            diagnostics: None,
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                document_changes: None,
                change_annotations: None,
            }),
            command: None,
            is_preferred: Some(true),
            disabled: None,
            data: None,
        })
    }

    /// Provide action to extract a constant from a literal
    fn provide_extract_constant_action(
        &self,
        params: &CodeActionParams,
        text: &str,
    ) -> Option<CodeActionOrCommand> {
        // Only provide this action if there's a selection
        if params.range.start == params.range.end {
            return None;
        }

        let selected_text = self.get_text_in_range(text, &params.range)?;

        // Check if selection looks like a numeric literal or simple expression
        if !self.is_extractable_literal(&selected_text) {
            return None;
        }

        let constant_name = format!("CONSTANT_{}", selected_text.replace([' ', '-'], "_"));

        // Find where to insert the constant declaration
        // For now, we'll just provide the action without automatic insertion
        // This would need more sophisticated analysis to find the right location

        Some(CodeActionOrCommand::CodeAction(CodeAction {
            title: format!("Extract constant '{}'", constant_name),
            kind: Some(CodeActionKind::REFACTOR_EXTRACT),
            diagnostics: None,
            edit: None, // Would need to implement full text editing logic
            command: None,
            is_preferred: Some(false),
            disabled: Some(crate::lsp_types::CodeActionDisabled {
                reason: "Not yet implemented - requires multi-location editing".to_string(),
            }),
            data: None,
        }))
    }

    /// Check if the selected text is an extractable literal
    fn is_extractable_literal(&self, text: &str) -> bool {
        let trimmed = text.trim();

        // Check for numeric literals
        if trimmed.parse::<i64>().is_ok() {
            return true;
        }

        // Check for simple set literals like {1, 2, 3}
        if trimmed.starts_with('{') && trimmed.ends_with('}') {
            return true;
        }

        false
    }

    /// Provide diagnostic-based quick fixes
    fn provide_diagnostic_based_actions(
        &self,
        params: &CodeActionParams,
        text: &str,
    ) -> Vec<CodeActionOrCommand> {
        let mut actions = Vec::new();

        // Offer "Add missing END" only for a diagnostic at end-of-input. A
        // missing terminator is reported there (pest's EOF position); a syntax
        // error inside the body sits on an earlier line. Keying off the
        // position — not the old `message.contains("expected")`, which matched
        // every syntax error — avoids suggesting an END for a typo deep inside
        // a predicate, and (unlike a "no END anywhere" text scan) is not fooled
        // by a nested END (`if … then … else … end`, an event END, or an `END`
        // inside a label). The component check is done last, only once a
        // candidate diagnostic exists.
        let end_line = document_end_position(text).line;
        if let Some(diagnostic) = params
            .context
            .diagnostics
            .iter()
            .find(|d| d.range.start.line >= end_line)
            && (has_keyword_line(text, KeywordId::Machine)
                || has_keyword_line(text, KeywordId::Context))
            && let Some(action) =
                self.create_add_missing_end_action(&params.text_document.uri, diagnostic, text)
        {
            actions.push(CodeActionOrCommand::CodeAction(action));
        }

        actions
    }

    /// Create action to add missing END keyword
    fn create_add_missing_end_action(
        &self,
        uri: &Url,
        diagnostic: &crate::lsp_types::Diagnostic,
        text: &str,
    ) -> Option<CodeAction> {
        // Keyword sniffing below must not match words inside comments.
        let masked = rossi::comments::mask_comments_chars(text);
        let lines: Vec<&str> = masked.lines().collect();
        if lines.is_empty() {
            return None;
        }
        // A missing END is reported at end-of-file — one line past the last
        // line — so clamp instead of bailing on positions beyond the text.
        let line_idx = (diagnostic.range.start.line as usize).min(lines.len() - 1);

        let line = lines[line_idx];

        // Determine what kind of END we need based on context (keywords are
        // case-insensitive; an event's END is indented under the EVENTS section)
        let end_keyword = if line_keyword_is(line, KeywordId::Machine)
            || line_keyword_is(line, KeywordId::Context)
        {
            "END"
        } else if line_keyword_is(line, KeywordId::Event) {
            "    END"
        } else {
            "END"
        };

        // Insert END at the end of the file or after the problematic line
        let insert_line = lines.len() as u32;
        let mut changes = HashMap::new();
        changes.insert(
            uri.clone(),
            vec![TextEdit {
                range: Range {
                    start: Position::new(insert_line, 0),
                    end: Position::new(insert_line, 0),
                },
                new_text: format!("{}\n", end_keyword),
            }],
        );

        Some(CodeAction {
            title: format!("Add missing {}", end_keyword.trim()),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diagnostic.clone()]),
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                document_changes: None,
                change_annotations: None,
            }),
            command: None,
            is_preferred: Some(true),
            disabled: None,
            data: None,
        })
    }

    /// Provide actions to add missing clauses
    fn provide_add_missing_clause_actions(
        &self,
        params: &CodeActionParams,
        text: &str,
    ) -> Vec<CodeActionOrCommand> {
        let mut actions = Vec::new();

        // Detect if we're in a MACHINE or CONTEXT — on comment-masked text,
        // so clause keywords mentioned in comments neither suppress nor
        // trigger these actions.
        let masked = rossi::comments::mask_comments(text);
        let text = masked.as_str();

        // Detect if we're in a MACHINE or CONTEXT (keywords are case-insensitive)
        let is_machine = has_keyword_line(text, KeywordId::Machine);
        let is_context = has_keyword_line(text, KeywordId::Context);

        if is_machine {
            // Check for missing clauses in machines
            if !has_keyword_line(text, KeywordId::Invariants)
                && let Some(action) = self.create_add_clause_action(
                    &params.text_document.uri,
                    text,
                    "INVARIANTS",
                    "    @inv1 TRUE",
                )
            {
                actions.push(CodeActionOrCommand::CodeAction(action));
            }
            if !has_keyword_line(text, KeywordId::Variables)
                && let Some(action) =
                    self.create_add_clause_action(&params.text_document.uri, text, "VARIABLES", "")
            {
                actions.push(CodeActionOrCommand::CodeAction(action));
            }
        }

        if is_context {
            // Check for missing clauses in contexts
            if !has_keyword_line(text, KeywordId::Axioms)
                && let Some(action) = self.create_add_clause_action(
                    &params.text_document.uri,
                    text,
                    "AXIOMS",
                    "    @axm1 TRUE",
                )
            {
                actions.push(CodeActionOrCommand::CodeAction(action));
            }
            if !has_keyword_line(text, KeywordId::Constants)
                && let Some(action) =
                    self.create_add_clause_action(&params.text_document.uri, text, "CONSTANTS", "")
            {
                actions.push(CodeActionOrCommand::CodeAction(action));
            }
            if !has_keyword_line(text, KeywordId::Sets)
                && let Some(action) =
                    self.create_add_clause_action(&params.text_document.uri, text, "SETS", "")
            {
                actions.push(CodeActionOrCommand::CodeAction(action));
            }
        }

        actions
    }

    /// Create action to add a missing clause
    fn create_add_clause_action(
        &self,
        uri: &Url,
        text: &str,
        clause_name: &str,
        example_content: &str,
    ) -> Option<CodeAction> {
        let lines: Vec<&str> = text.lines().collect();

        // Find a good insertion point (after the component declaration;
        // keywords are case-insensitive)
        let mut insert_line = 1; // Default to line 1
        for (idx, line) in lines.iter().enumerate() {
            if line_keyword_is(line, KeywordId::Machine)
                || line_keyword_is(line, KeywordId::Context)
            {
                insert_line = idx + 1;
                break;
            }
        }

        let new_text = if example_content.is_empty() {
            format!("{}\n", clause_name)
        } else {
            format!("{}\n{}\n", clause_name, example_content)
        };

        let mut changes = HashMap::new();
        changes.insert(
            uri.clone(),
            vec![TextEdit {
                range: Range {
                    start: Position::new(insert_line as u32, 0),
                    end: Position::new(insert_line as u32, 0),
                },
                new_text,
            }],
        );

        Some(CodeAction {
            title: format!("Add {} clause", clause_name),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: None,
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                document_changes: None,
                change_annotations: None,
            }),
            command: None,
            is_preferred: Some(false),
            disabled: None,
            data: None,
        })
    }

    /// Provide actions to sort clauses alphabetically
    fn provide_sort_clauses_actions(
        &self,
        params: &CodeActionParams,
        text: &str,
    ) -> Vec<CodeActionOrCommand> {
        let mut actions = Vec::new();

        // Try to find sortable clauses
        if let Some(action) = self.create_sort_variables_action(&params.text_document.uri, text) {
            actions.push(CodeActionOrCommand::CodeAction(action));
        }

        if let Some(action) = self.create_sort_constants_action(&params.text_document.uri, text) {
            actions.push(CodeActionOrCommand::CodeAction(action));
        }

        actions
    }

    /// Create action to sort VARIABLES clause
    fn create_sort_variables_action(&self, uri: &Url, text: &str) -> Option<CodeAction> {
        self.create_sort_clause_action(uri, text, "VARIABLES")
    }

    /// Create action to sort CONSTANTS clause
    fn create_sort_constants_action(&self, uri: &Url, text: &str) -> Option<CodeAction> {
        self.create_sort_clause_action(uri, text, "CONSTANTS")
    }

    /// Generic method to create a sort clause action
    fn create_sort_clause_action(
        &self,
        uri: &Url,
        text: &str,
        clause_name: &str,
    ) -> Option<CodeAction> {
        let lines: Vec<&str> = text.lines().collect();

        // Find the clause
        let mut clause_start = None;
        let mut clause_end = None;

        for (idx, line) in lines.iter().enumerate() {
            if line.trim().eq_ignore_ascii_case(clause_name) {
                clause_start = Some(idx);
            } else if clause_start.is_some() && clause_end.is_none() {
                // Check if we've reached the end of the clause (keywords are
                // case-insensitive)
                let trimmed = line.trim();
                if trimmed.is_empty()
                    || line_keyword_is(trimmed, KeywordId::Invariants)
                    || line_keyword_is(trimmed, KeywordId::Axioms)
                    || line_keyword_is(trimmed, KeywordId::Events)
                    || line_keyword_is(trimmed, KeywordId::End)
                    || line_keyword_is(trimmed, KeywordId::Initialisation)
                {
                    clause_end = Some(idx);
                    break;
                }
            }
        }

        if let (Some(start), Some(end)) = (clause_start, clause_end) {
            if end <= start + 1 {
                return None; // No items to sort
            }

            // Extract and sort the items
            let items: Vec<&str> = lines[start + 1..end].to_vec();
            if items.is_empty() {
                return None;
            }

            let mut sorted_items: Vec<String> = items.iter().map(|s| s.to_string()).collect();
            sorted_items.sort();

            // Check if already sorted
            let already_sorted = items.iter().zip(sorted_items.iter()).all(|(a, b)| a == b);
            if already_sorted {
                return None;
            }

            let sorted_text = sorted_items.join("\n") + "\n";

            let mut changes = HashMap::new();
            changes.insert(
                uri.clone(),
                vec![TextEdit {
                    range: Range {
                        start: Position::new((start + 1) as u32, 0),
                        end: Position::new(end as u32, 0),
                    },
                    new_text: sorted_text,
                }],
            );

            Some(CodeAction {
                title: format!("Sort {} alphabetically", clause_name.to_lowercase()),
                kind: Some(CodeActionKind::REFACTOR),
                diagnostics: None,
                edit: Some(WorkspaceEdit {
                    changes: Some(changes),
                    document_changes: None,
                    change_annotations: None,
                }),
                command: None,
                is_preferred: Some(false),
                disabled: None,
                data: None,
            })
        } else {
            None
        }
    }

    /// Provide action to trigger rename on an event
    fn provide_rename_event_action(
        &self,
        params: &CodeActionParams,
        text: &str,
    ) -> Option<CodeActionOrCommand> {
        // Check if cursor is on an EVENT declaration
        let lines: Vec<&str> = text.lines().collect();
        let cursor_line = params.range.start.line as usize;

        if cursor_line >= lines.len() {
            return None;
        }

        let line = lines[cursor_line].trim();

        // Check if this line is an event declaration (keyword is case-insensitive)
        if line_keyword_is(line, KeywordId::Event) {
            // Note: Rename is better handled by the LSP rename feature
            // This code action would just provide a hint
            Some(CodeActionOrCommand::CodeAction(CodeAction {
                title: "Rename event (use F2 or rename command)".to_string(),
                kind: Some(CodeActionKind::REFACTOR),
                diagnostics: None,
                edit: None,
                command: None,
                is_preferred: Some(false),
                disabled: Some(crate::lsp_types::CodeActionDisabled {
                    reason: "Use the LSP rename feature instead (F2)".to_string(),
                }),
                data: None,
            }))
        } else {
            None
        }
    }

    /// Get text within a range
    ///
    /// LSP positions use character (code-point) offsets, not byte offsets.
    /// This method properly converts character offsets to byte offsets.
    fn get_text_in_range(&self, text: &str, range: &Range) -> Option<String> {
        let lines: Vec<&str> = text.lines().collect();

        let start_line = range.start.line as usize;
        let end_line = range.end.line as usize;

        if start_line >= lines.len() || end_line >= lines.len() {
            return None;
        }

        if start_line == end_line {
            let line = lines[start_line];
            let start_byte = crate::position::utf16_to_byte(line, range.start.character as usize)?;
            let end_byte = crate::position::utf16_to_byte(line, range.end.character as usize)?;
            Some(line[start_byte..end_byte].to_string())
        } else {
            let mut result = String::new();

            // First line
            let start_byte =
                crate::position::utf16_to_byte(lines[start_line], range.start.character as usize)?;
            result.push_str(&lines[start_line][start_byte..]);
            result.push('\n');

            // Middle lines
            for line in lines.iter().take(end_line).skip(start_line + 1) {
                result.push_str(line);
                result.push('\n');
            }

            // Last line
            let end_byte =
                crate::position::utf16_to_byte(lines[end_line], range.end.character as usize)?;
            result.push_str(&lines[end_line][..end_byte]);

            Some(result)
        }
    }
}

impl Default for CodeActionProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_ascii_operators() {
        let provider = CodeActionProvider::new();
        assert!(provider.has_ascii_operators("x & y"));
        assert!(provider.has_ascii_operators("x => y"));
        assert!(!provider.has_ascii_operators("x + y"));
        // Alphabetic operators with word-boundary matching
        assert!(provider.has_ascii_operators("not x"));
        assert!(provider.has_ascii_operators("f circ g"));
        assert!(provider.has_ascii_operators("UNION(x, S, E)"));
        assert!(provider.has_ascii_operators("INTER(x, S, E)"));
        // "not" inside identifier should NOT match
        assert!(!provider.has_ascii_operators("notation"));
    }

    #[test]
    fn test_has_unicode_operators() {
        let provider = CodeActionProvider::new();
        assert!(provider.has_unicode_operators("x ∧ y"));
        assert!(provider.has_unicode_operators("x ⇒ y"));
        assert!(!provider.has_unicode_operators("x + y"));
    }

    #[test]
    fn test_convert_to_unicode() {
        let provider = CodeActionProvider::new();
        assert_eq!(provider.convert_to_unicode("x & y"), "x ∧ y");
        assert_eq!(provider.convert_to_unicode("x => y"), "x ⇒ y");
        assert_eq!(provider.convert_to_unicode("x : NAT"), "x ∈ ℕ");
        assert_eq!(provider.convert_to_unicode("x :: S"), "x :∈ S");
        assert_eq!(provider.convert_to_unicode("x :| x' : NAT"), "x :∣ x' ∈ ℕ");
        assert_eq!(provider.convert_to_unicode("r~"), "r∼");
        assert_eq!(
            provider.convert_to_unicode("x & y => z or w"),
            "x ∧ y ⇒ z ∨ w"
        );
    }

    #[test]
    fn test_convert_to_ascii() {
        let provider = CodeActionProvider::new();
        assert_eq!(provider.convert_to_ascii("x ∧ y"), "x & y");
        assert_eq!(provider.convert_to_ascii("x ⇒ y"), "x => y");
        assert_eq!(provider.convert_to_ascii("x ∈ ℕ"), "x : NAT");
        assert_eq!(
            provider.convert_to_ascii("x ∧ y ⇒ z ∨ w"),
            "x & y => z or w"
        );
        // New mappings
        assert_eq!(provider.convert_to_ascii("¬ P"), "not P");
        assert_eq!(provider.convert_to_ascii("S × T"), "S ** T");
        assert_eq!(provider.convert_to_ascii("1 ‥ 10"), "1 .. 10");
        assert_eq!(provider.convert_to_ascii("x − y"), "x - y");
        assert_eq!(provider.convert_to_ascii("x ∗ y"), "x * y");
        assert_eq!(provider.convert_to_ascii("f → g"), "f --> g");
        assert_eq!(provider.convert_to_ascii("\u{E100}"), "<<->");
        assert_eq!(provider.convert_to_ascii("\u{E101}"), "<->>");
        assert_eq!(provider.convert_to_ascii("\u{E102}"), "<<->>");
        assert_eq!(provider.convert_to_ascii("f ↠ g"), "f ->> g");
        assert_eq!(provider.convert_to_ascii("f ∘ g"), "f circ g");
        assert_eq!(provider.convert_to_ascii("⊆"), "<:");
        assert_eq!(provider.convert_to_ascii("⊂"), "<<:");
        assert_eq!(provider.convert_to_ascii("⊈"), "/<:");
        assert_eq!(provider.convert_to_ascii("⊄"), "/<<:");
        assert_eq!(provider.convert_to_ascii("◁"), "<|");
        assert_eq!(provider.convert_to_ascii("▷"), "|>");
        assert_eq!(provider.convert_to_ascii("\u{E103}"), "<+");
        assert_eq!(provider.convert_to_ascii("⤔"), ">+>");
        assert_eq!(provider.convert_to_ascii("⤀"), "+>>");
        assert_eq!(provider.convert_to_ascii("⤖"), ">->>");
        assert_eq!(provider.convert_to_ascii("⦂"), "oftype");
        assert_eq!(provider.convert_to_ascii("∅"), "{}");
        assert_eq!(provider.convert_to_ascii("r∼"), "r~");
        assert_eq!(provider.convert_to_ascii("⋃"), "UNION");
        assert_eq!(provider.convert_to_ascii("⋂"), "INTER");
        assert_eq!(provider.convert_to_ascii("·"), ".");
        assert_eq!(provider.convert_to_ascii("λ"), "%");
        assert_eq!(provider.convert_to_ascii("x :∈ S"), "x :: S");
        assert_eq!(provider.convert_to_ascii("x :∣ x' ∈ ℕ"), "x :| x' : NAT");
    }

    #[test]
    fn test_roundtrip_ascii_unicode_ascii() {
        let provider = CodeActionProvider::new();
        let ascii_text = "x : NAT & x <= 10 => x /= 0";
        let unicode = provider.convert_to_unicode(ascii_text);
        let back = provider.convert_to_ascii(&unicode);
        assert_eq!(back, ascii_text);
    }

    #[test]
    fn test_roundtrip_set_operators() {
        let provider = CodeActionProvider::new();
        let ascii_text = "S <: T /\\ x : S \\/ T";
        let unicode = provider.convert_to_unicode(ascii_text);
        let back = provider.convert_to_ascii(&unicode);
        assert_eq!(back, ascii_text);
    }

    #[test]
    fn test_roundtrip_function_types() {
        let provider = CodeActionProvider::new();
        let ascii_text = "f : S --> T & g : S >-> T & h : S ->> T & k : S >->> T";
        let unicode = provider.convert_to_unicode(ascii_text);
        let back = provider.convert_to_ascii(&unicode);
        assert_eq!(back, ascii_text);
    }

    #[test]
    fn test_is_extractable_literal() {
        let provider = CodeActionProvider::new();
        assert!(provider.is_extractable_literal("42"));
        assert!(provider.is_extractable_literal("  123  "));
        assert!(provider.is_extractable_literal("{1, 2, 3}"));
        assert!(!provider.is_extractable_literal("x + y"));
    }

    #[test]
    fn test_get_text_in_range_single_line() {
        let provider = CodeActionProvider::new();
        let text = "hello world";
        let range = Range {
            start: Position::new(0, 0),
            end: Position::new(0, 5),
        };
        assert_eq!(
            provider.get_text_in_range(text, &range),
            Some("hello".to_string())
        );
    }

    #[test]
    fn test_get_text_in_range_multi_line() {
        let provider = CodeActionProvider::new();
        let text = "line1\nline2\nline3";
        let range = Range {
            start: Position::new(0, 2),
            end: Position::new(2, 3),
        };
        let result = provider.get_text_in_range(text, &range);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "ne1\nline2\nlin");
    }

    #[test]
    fn test_get_text_in_range_unicode() {
        let provider = CodeActionProvider::new();
        // "x ∈ ℕ" — ∈ is 3 bytes, ℕ is 3 bytes, but each is 1 character
        let text = "x ∈ ℕ ∧ y ≤ 10";
        // Character positions: x(0) (1)∈(2) (3)ℕ(4) (5)∧(6) (7)y(8) (9)≤(10) (11)1(12)0(13)
        let range = Range {
            start: Position::new(0, 2),
            end: Position::new(0, 4),
        };
        let result = provider.get_text_in_range(text, &range);
        assert_eq!(result, Some("∈ ".to_string()));
    }

    #[test]
    fn test_has_keyword_line_is_case_insensitive() {
        assert!(has_keyword_line(
            "machine m\nvariables\n    x\nend",
            KeywordId::Machine
        ));
        assert!(has_keyword_line("MACHINE m", KeywordId::Machine));
        assert!(!has_keyword_line("context c\nend", KeywordId::Machine));
        // First-token precision: a keyword embedded in an identifier never matches.
        assert!(!has_keyword_line("    machinery\n", KeywordId::Machine));
    }

    #[test]
    fn test_sort_clause_action_lowercase_keywords() {
        let provider = CodeActionProvider::new();
        let uri = Url::parse("file:///m.eventb").unwrap();
        // Lowercase keywords; an out-of-order `variables` clause ended by `events`.
        let text = "machine m\nvariables\n    b\n    a\n    c\nevents\nend";
        let action = provider
            .create_sort_clause_action(&uri, text, "VARIABLES")
            .expect("should offer to sort the lowercase variables clause");
        assert_eq!(action.title, "Sort variables alphabetically");
    }

    #[test]
    fn test_add_clause_inserts_after_lowercase_header() {
        let provider = CodeActionProvider::new();
        let uri = Url::parse("file:///m.eventb").unwrap();
        let text = "machine m\nvariables\n    x\nend";
        let action = provider
            .create_add_clause_action(&uri, text, "INVARIANTS", "    @inv1 TRUE")
            .expect("should offer to add a clause");
        let edit = &action.edit.unwrap().changes.unwrap()[&uri][0];
        // Inserted right after the lowercase `machine` header (line 0).
        assert_eq!(edit.range.start.line, 1);
    }
}
