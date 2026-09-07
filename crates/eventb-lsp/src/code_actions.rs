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
use crate::text_utils::{line_keyword, line_keyword_is};
use rossi::keywords::KeywordId;
use rossi::operators;
use rossi_build::rules::RuleId;
use std::collections::{HashMap, HashSet};

/// The source action that normalizes every operator to the configured
/// convention (`rossi.format.useUnicode`) and changes nothing else. A
/// `source.fixAll.*` kind so editors can run it on save — VS Code's
/// `editor.codeActionsOnSave` only triggers `source.*` kinds — without
/// reformatting the document the way `textDocument/formatting` would.
pub const FIX_ALL_KIND: CodeActionKind = CodeActionKind::new("source.fixAll.rossi");

/// The operator style a conversion direction targets, as spelled in titles.
fn style_name(to_unicode: bool) -> &'static str {
    if to_unicode { "Unicode" } else { "ASCII" }
}

/// Whether the client's `only` filter admits `kind`: no filter, or an entry
/// equal to `kind` or a dot-delimited prefix of it (`source` and
/// `source.fixAll` both admit `source.fixAll.rossi`).
fn kind_requested(params: &CodeActionParams, kind: &CodeActionKind) -> bool {
    params.context.only.as_ref().is_none_or(|only| {
        only.iter().any(|requested| {
            kind.as_str()
                .strip_prefix(requested.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
        })
    })
}

/// A workspace edit replacing `range` of the document at `uri` with `new_text`.
fn single_edit(uri: &Url, range: Range, new_text: String) -> WorkspaceEdit {
    WorkspaceEdit {
        changes: Some(HashMap::from([(
            uri.clone(),
            vec![TextEdit { range, new_text }],
        )])),
        document_changes: None,
        change_annotations: None,
    }
}

/// A workspace edit replacing the whole of `text` (at `uri`) with `new_text`.
fn full_document_edit(uri: &Url, text: &str, new_text: String) -> WorkspaceEdit {
    let range = Range {
        start: Position::new(0, 0),
        end: document_end_position(text),
    };
    single_edit(uri, range, new_text)
}

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

/// Whether an LSP diagnostic carries the string rule `code` (e.g. `"EB026"`),
/// so a quick fix can attach itself to exactly that diagnostic.
fn diagnostic_code_is(diagnostic: &crate::lsp_types::Diagnostic, code: &str) -> bool {
    matches!(
        &diagnostic.code,
        Some(crate::lsp_types::NumberOrString::String(s)) if s == code
    )
}

/// The 0-indexed `line` of `text`, if it has one.
fn line_of(text: &str, line: u32) -> Option<&str> {
    text.lines().nth(line as usize)
}

/// The slice of `text` a diagnostic `range` covers.
fn text_in_range(text: &str, range: Range) -> Option<&str> {
    let start = crate::position::position_to_offset(text, range.start)?;
    let end = crate::position::position_to_offset(text, range.end)?;
    text.get(start..end)
}

/// The keyword the diagnostic `range` underlines, if it underlines exactly
/// one. A diagnostic on a formula yields `None`, and so does one on a label:
/// EB029 covers both an empty clause and a label with no formula, and a label
/// carries its `@` sigil, which no keyword spelling does.
fn keyword_at(text: &str, range: Range) -> Option<KeywordId> {
    rossi::keywords::lookup(text_in_range(text, range)?).map(|keyword| keyword.id)
}

/// Rodin's label stem for the clause `word` opens — `@axm1` under AXIOMS, and
/// so on. `None` for anything that is not a clause keyword carrying labeled
/// items; `WHEN` and `BEGIN` resolve to `WHERE` and `THEN` in `lookup`, so they
/// need no arm of their own.
fn label_stem(word: &str) -> Option<&'static str> {
    Some(match rossi::keywords::lookup(word)?.id {
        KeywordId::Axioms => "axm",
        KeywordId::Theorems => "thm",
        KeywordId::Invariants => "inv",
        KeywordId::Where => "grd",
        KeywordId::With | KeywordId::Witness => "wit",
        KeywordId::Then => "act",
        _ => return None,
    })
}

/// The byte range of the event holding `offset` in the comment-masked `masked`:
/// from the line that opens it to the one opening the next, or the ends of the
/// document when there is none. The whole line is scanned rather than its first
/// token, because an inline status (`convergent EVENT e`) hides the keyword.
fn enclosing_event_range(masked: &str, offset: usize) -> std::ops::Range<usize> {
    let mut start = 0;
    let mut end = masked.len();
    let mut at = 0;
    for line in masked.split_inclusive('\n') {
        let opens_event = line
            .split_whitespace()
            .any(|word| line_keyword(word) == Some(KeywordId::Event));
        if opens_event {
            if at <= offset {
                start = at;
            } else {
                end = end.min(at);
            }
        }
        at += line.len();
    }
    start..end
}

/// Provides code actions and refactorings
pub struct CodeActionProvider;

impl CodeActionProvider {
    pub fn new() -> Self {
        Self
    }

    /// Provide code actions for a given document position/range.
    /// `use_unicode` is the operator convention (`rossi.format.useUnicode`)
    /// the fix-all source action normalizes to; `private_use_glyphs`
    /// (`rossi.format.privateUseGlyphs`) is how a conversion toward Unicode
    /// spells the four relation/override operators.
    pub fn provide_code_actions(
        &self,
        params: &CodeActionParams,
        text: &str,
        use_unicode: bool,
        private_use_glyphs: bool,
    ) -> Option<CodeActionResponse> {
        // Each group is computed only when the client's `only` filter admits
        // its kind, so an on-save request for the fix-all neither pays for
        // nor receives the refactors and quick fixes it would discard.
        let requested = |kind: &CodeActionKind| kind_requested(params, kind);
        let mut actions = Vec::new();

        // Add operator conversion actions, including the on-save normalization
        if requested(&CodeActionKind::REFACTOR) || requested(&FIX_ALL_KIND) {
            actions.extend(self.provide_operator_conversion_actions(
                params,
                text,
                use_unicode,
                private_use_glyphs,
            ));
        }

        if requested(&CodeActionKind::QUICKFIX) {
            // Add diagnostic-based quick fixes (from diagnostics in context)
            actions.extend(self.provide_diagnostic_based_actions(params, text, private_use_glyphs));

            // Add missing clause actions
            actions.extend(self.provide_add_missing_clause_actions(params, text));
        }

        if requested(&CodeActionKind::REFACTOR) {
            // Add sort clauses action
            actions.extend(self.provide_sort_clauses_actions(params, text));

            // Add rename event action if cursor is on an event name
            if let Some(action) = self.provide_rename_event_action(params, text) {
                actions.push(action);
            }
        }

        // Add extract constant action if a literal is selected
        if requested(&CodeActionKind::REFACTOR_EXTRACT)
            && let Some(action) = self.provide_extract_constant_action(params, text)
        {
            actions.push(action);
        }

        if actions.is_empty() {
            None
        } else {
            Some(actions)
        }
    }

    /// Provide actions to convert operators between ASCII and Unicode: the
    /// whole document each way — the direction of the configured convention
    /// doubling as the [`FIX_ALL_KIND`] on-save action, sharing its edit —
    /// and the selection.
    fn provide_operator_conversion_actions(
        &self,
        params: &CodeActionParams,
        text: &str,
        use_unicode: bool,
        private_use_glyphs: bool,
    ) -> Vec<CodeActionOrCommand> {
        let uri = &params.text_document.uri;
        let refactor = kind_requested(params, &CodeActionKind::REFACTOR);
        let fix_all = kind_requested(params, &FIX_ALL_KIND);
        // Operator detection sees the code only — comments, labels and
        // component names masked, positions preserved — so prose and names
        // neither trigger a conversion nor get rewritten by one.
        let masked = rossi::comments::mask_opaque(text);
        let mut actions = Vec::new();

        for to_unicode in [true, false] {
            let present = if to_unicode {
                operators::has_ascii_operators(&masked)
            } else {
                operators::has_unicode_operators(&masked)
            };
            // The fix-all normalizes toward the convention; it is offered only
            // when it would change something, so running it on save is a
            // no-op for a document already there.
            let normalizes = fix_all && to_unicode == use_unicode;
            if !present || !(refactor || normalizes) {
                continue;
            }
            let Some(action) =
                self.create_convert_all_action(uri, text, to_unicode, private_use_glyphs)
            else {
                continue;
            };
            if normalizes {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: format!("Normalize operators to {}", style_name(to_unicode)),
                    kind: Some(FIX_ALL_KIND),
                    ..action.clone()
                }));
            }
            if refactor {
                actions.push(CodeActionOrCommand::CodeAction(action));
            }
        }

        // Check if we can convert just the selection. Operator detection and
        // conversion use the FULL document's comment spans (via byte offsets),
        // so a selection that opens inside a `/* */` or `//` comment keeps its
        // prose intact instead of having operator spellings rewritten.
        if refactor
            && params.range.start != params.range.end
            && let (Some(start), Some(end)) = (
                crate::identifier_utils::position_to_offset(text, params.range.start),
                crate::identifier_utils::position_to_offset(text, params.range.end),
            )
            && start < end
        {
            let selected = &text[start..end];

            let conversions = [
                (
                    operators::has_ascii_operators as fn(&str) -> bool,
                    true,
                    "Convert selection to Unicode",
                ),
                (
                    operators::has_unicode_operators,
                    false,
                    "Convert selection to ASCII",
                ),
            ];
            for (has_operators, to_unicode, title) in conversions {
                if has_operators(&masked[start..end]) {
                    let converted =
                        rossi::comments::map_code_segments_in_range(text, start, end, |code| {
                            if to_unicode {
                                operators::convert_to_unicode(code, private_use_glyphs)
                            } else {
                                operators::convert_to_ascii(code)
                            }
                        });
                    if let Some(action) = self.create_convert_selection_action(
                        uri,
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

    /// Convert ASCII operators to Unicode in the given text.
    /// Comment text is never rewritten — `<=` in prose stays `<=`.
    pub fn convert_to_unicode(&self, text: &str, private_use_glyphs: bool) -> String {
        rossi::comments::map_code_segments(text, |code| {
            operators::convert_to_unicode(code, private_use_glyphs)
        })
    }

    /// Convert Unicode operators to ASCII in the given text.
    /// Comment text is never rewritten.
    pub fn convert_to_ascii(&self, text: &str) -> String {
        rossi::comments::map_code_segments(text, operators::convert_to_ascii)
    }

    /// The whole-document refactor toward `to_unicode`'s spelling, or `None`
    /// when the document is already there.
    fn create_convert_all_action(
        &self,
        uri: &Url,
        text: &str,
        to_unicode: bool,
        private_use_glyphs: bool,
    ) -> Option<CodeAction> {
        let converted = if to_unicode {
            self.convert_to_unicode(text, private_use_glyphs)
        } else {
            self.convert_to_ascii(text)
        };
        if converted == text {
            return None;
        }

        Some(CodeAction {
            title: format!("Convert all operators to {}", style_name(to_unicode)),
            kind: Some(CodeActionKind::REFACTOR),
            diagnostics: None,
            edit: Some(full_document_edit(uri, text, converted)),
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
        private_use_glyphs: bool,
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

        // Fix a misplaced assignment operator (EB026): swap `:=`/`≔` → `=` or
        // `:∈`/`::` → `∈`. Keyed on the rule code the diagnostics provider
        // attaches, so it never fires on an unrelated syntax error.
        for diagnostic in params
            .context
            .diagnostics
            .iter()
            .filter(|d| diagnostic_code_is(d, RuleId::AssignmentInPredicate.code()))
        {
            if let Some(action) = self.create_fix_assignment_in_predicate_action(
                &params.text_document.uri,
                diagnostic,
                text,
            ) {
                actions.push(CodeActionOrCommand::CodeAction(action));
            }
        }

        // Delete a clause header with nothing under it (EB029). The same rule
        // also covers a label with no formula, where there is nothing to fix
        // on the user's behalf — only they can write the missing predicate —
        // so the action is offered only when the diagnostic underlines a
        // clause keyword.
        for diagnostic in params
            .context
            .diagnostics
            .iter()
            .filter(|d| diagnostic_code_is(d, RuleId::EmptyClause.code()))
        {
            if let Some(action) =
                self.create_remove_empty_clause_action(&params.text_document.uri, diagnostic, text)
            {
                actions.push(CodeActionOrCommand::CodeAction(action));
            }
        }

        // Write the label an item is missing (EB032).
        for diagnostic in params
            .context
            .diagnostics
            .iter()
            .filter(|d| diagnostic_code_is(d, RuleId::MissingLabel.code()))
        {
            if let Some(action) =
                self.create_insert_label_action(&params.text_document.uri, diagnostic, text)
            {
                actions.push(CodeActionOrCommand::CodeAction(action));
            }
        }

        // Move a clause written below one it must precede (EB030).
        for diagnostic in params
            .context
            .diagnostics
            .iter()
            .filter(|d| diagnostic_code_is(d, RuleId::ClauseOutOfOrder.code()))
        {
            if let Some(action) =
                self.create_move_clause_action(&params.text_document.uri, diagnostic, text)
            {
                actions.push(CodeActionOrCommand::CodeAction(action));
            }
        }

        // Rewrite an ASCII operator spelling flagged under
        // rossi.format.enforceUnicode to its Unicode form — for a diagnostic
        // that is still current, i.e. whose range is one of the operators the
        // advisory flags right now. A range the client carried across an edit
        // may cover anything by then.
        let advisories: Vec<_> = params
            .context
            .diagnostics
            .iter()
            .filter(|d| diagnostic_code_is(d, crate::diagnostics::ASCII_OPERATOR_CODE))
            .collect();
        if !advisories.is_empty() {
            let current = crate::diagnostics::ascii_operators(text, private_use_glyphs);
            for diagnostic in advisories {
                if let Some((_, ascii, unicode)) = current
                    .iter()
                    .find(|(range, _, _)| *range == diagnostic.range)
                {
                    actions.push(CodeActionOrCommand::CodeAction(self.replace_operator_fix(
                        &params.text_document.uri,
                        diagnostic,
                        ascii,
                        unicode,
                    )));
                }
            }
        }

        actions
    }

    /// A quick fix replacing the operator `diagnostic` underlines with
    /// `replacement`, attached to that diagnostic.
    fn replace_operator_fix(
        &self,
        uri: &Url,
        diagnostic: &crate::lsp_types::Diagnostic,
        operator: &str,
        replacement: &str,
    ) -> CodeAction {
        CodeAction {
            title: format!("Replace `{operator}` with `{replacement}`"),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diagnostic.clone()]),
            edit: Some(single_edit(uri, diagnostic.range, replacement.to_string())),
            command: None,
            is_preferred: Some(true),
            disabled: None,
            data: None,
        }
    }

    /// Quick fix for EB026 (assignment operator in a predicate). The diagnostic
    /// range underlines just the becomes operator; replace it with the predicate
    /// operator it was most likely meant to be — `:=`/`≔` → `=` (equality),
    /// `:∈`/`::` → `∈` (membership). `:|`/`:∣` (becomes-such-that) has a
    /// predicate right-hand side that cannot be rewritten by a single-token swap,
    /// so no fix is offered for it (the diagnostic still stands).
    fn create_fix_assignment_in_predicate_action(
        &self,
        uri: &Url,
        diagnostic: &crate::lsp_types::Diagnostic,
        text: &str,
    ) -> Option<CodeAction> {
        let operator = text_in_range(text, diagnostic.range)?;
        let replacement = match operator {
            ":=" | "≔" => "=",
            ":∈" | "::" => "∈",
            _ => return None,
        };
        Some(self.replace_operator_fix(uri, diagnostic, operator, replacement))
    }

    /// Quick fix for EB029 (an empty clause): delete the header that has
    /// nothing under it. The keyword usually sits alone on its line, so the
    /// line goes with it; a header sharing its line with something else loses
    /// only the keyword. Returns `None` when the diagnostic underlines a label
    /// rather than a clause keyword — the other half of EB029, where the
    /// missing formula is the user's to write.
    fn create_remove_empty_clause_action(
        &self,
        uri: &Url,
        diagnostic: &crate::lsp_types::Diagnostic,
        text: &str,
    ) -> Option<CodeAction> {
        let keyword = keyword_at(text, diagnostic.range)?;
        let line = diagnostic.range.start.line;
        let alone = line_of(text, line)?.trim() == text_in_range(text, diagnostic.range)?;
        let range = if alone {
            Range {
                start: Position::new(line, 0),
                end: Position::new(line + 1, 0),
            }
        } else {
            diagnostic.range
        };
        Some(CodeAction {
            title: format!("Remove empty {}", rossi::keywords::spell(keyword)),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diagnostic.clone()]),
            edit: Some(single_edit(uri, range, String::new())),
            command: None,
            is_preferred: Some(true),
            disabled: None,
            data: None,
        })
    }

    /// Quick fix for EB032 (an item with no label): write one in front of it.
    ///
    /// The stem follows Rodin's own naming for the enclosing clause
    /// ([`label_stem`]) and the number is the first free one in the scope the
    /// label has to be unique in (EB022), so the fix reads like the labels
    /// around it and cannot collide with one.
    fn create_insert_label_action(
        &self,
        uri: &Url,
        diagnostic: &crate::lsp_types::Diagnostic,
        text: &str,
    ) -> Option<CodeAction> {
        // One lexical scan serves both the mask and the labels already written.
        let lexical = rossi::comments::lexical_spans(text);
        let masked = lexical.mask_comments_chars(text);
        let item = crate::position::position_to_offset(&masked, diagnostic.range.start)?;
        // The clause keyword is the last one written before the item: on the
        // item's own line for an inline `EVENT e THEN x ≔ 1 END`, on a line
        // above it for the indented form.
        let stem = masked[..item]
            .split_whitespace()
            .rev()
            .find_map(label_stem)?;
        // A guard, witness or action label is unique within its event, an
        // axiom, invariant or theorem within the component, and Rodin numbers
        // each event's items from 1 — so the free number is looked for in the
        // scope the clash would be found in.
        let scope = match stem {
            "grd" | "wit" | "act" => enclosing_event_range(&masked, item),
            _ => 0..masked.len(),
        };
        let taken: HashSet<&str> = lexical
            .labels
            .iter()
            .filter(|span| scope.contains(&span.start))
            // The span covers `@name`; only the leading sigil is syntax.
            .map(|span| &text[span.start + 1..span.end])
            .collect();
        let label = (1..)
            .map(|n| format!("{stem}{n}"))
            .find(|label| !taken.contains(label.as_str()))?;
        Some(CodeAction {
            title: format!("Insert label @{label}"),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diagnostic.clone()]),
            edit: Some(single_edit(
                uri,
                Range {
                    start: diagnostic.range.start,
                    end: diagnostic.range.start,
                },
                format!("@{label} "),
            )),
            command: None,
            is_preferred: Some(true),
            disabled: None,
            data: None,
        })
    }

    /// Quick fix for EB030 (an event clause written out of order): move the
    /// clause above the earliest clause it must precede.
    ///
    /// The diagnostic spans the whole clause, so the lines it covers are what
    /// moves. The destination comes from the grammar's own clause order
    /// ([`rossi::keywords::event_clause_boundary`]), scanning up only to the
    /// line that opens or closes the enclosing event, so a clause is never
    /// lifted out of it.
    fn create_move_clause_action(
        &self,
        uri: &Url,
        diagnostic: &crate::lsp_types::Diagnostic,
        text: &str,
    ) -> Option<CodeAction> {
        let masked = rossi::comments::mask_comments_chars(text);
        let lines: Vec<&str> = masked.lines().collect();
        let first = diagnostic.range.start.line as usize;
        let last = diagnostic.range.end.line as usize;
        if last >= lines.len() {
            return None;
        }
        // The range spans the whole clause, so the keyword is the first token
        // of its first line.
        let clause = line_keyword(lines[first])?;
        // Whole lines move, so the clause must own its last one: in
        // `WITH @w y = 1 END` the event's END would travel with the clause.
        let clause_end = crate::position::position_to_offset(&masked, diagnostic.range.end)?;
        if !masked[clause_end..]
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            return None;
        }
        let follows = rossi::keywords::event_clause_boundary(clause);
        let mut target = None;
        for (index, line) in lines[..first].iter().enumerate().rev() {
            let Some(keyword) = line_keyword(line) else {
                continue;
            };
            // The event this clause belongs to starts here, so the scan stops:
            // `END` closes the event above (an inline status — `convergent
            // EVENT e` — hides the header keyword, so `EVENT` alone is not
            // enough to stay inside the event), and `EVENTS` opens the block.
            if matches!(
                keyword,
                KeywordId::Event | KeywordId::Events | KeywordId::End
            ) {
                break;
            }
            if follows.contains(&keyword) {
                target = Some((index, keyword));
            }
        }
        let (target_line, target_keyword) = target?;
        let moved: String = text
            .lines()
            .skip(first)
            .take(last - first + 1)
            .map(|line| format!("{line}\n"))
            .collect();
        let edits = vec![
            TextEdit {
                range: Range {
                    start: Position::new(target_line as u32, 0),
                    end: Position::new(target_line as u32, 0),
                },
                new_text: moved,
            },
            TextEdit {
                range: Range {
                    start: Position::new(first as u32, 0),
                    end: Position::new(last as u32 + 1, 0),
                },
                new_text: String::new(),
            },
        ];
        Some(CodeAction {
            title: format!(
                "Move {} above {}",
                rossi::keywords::spell(clause),
                rossi::keywords::spell(target_keyword)
            ),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diagnostic.clone()]),
            edit: Some(WorkspaceEdit {
                changes: Some(HashMap::from([(uri.clone(), edits)])),
                document_changes: None,
                change_annotations: None,
            }),
            command: None,
            is_preferred: Some(true),
            disabled: None,
            data: None,
        })
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

    /// Get text within a range.
    ///
    /// LSP positions are UTF-16 offsets, so this goes through the one
    /// converter rather than indexing bytes.
    fn get_text_in_range(&self, text: &str, range: &Range) -> Option<String> {
        text_in_range(text, *range).map(str::to_owned)
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
        assert!(operators::has_ascii_operators("x & y"));
        assert!(operators::has_ascii_operators("x => y"));
        assert!(!operators::has_ascii_operators("x + y"));
        // Alphabetic operators with word-boundary matching
        assert!(operators::has_ascii_operators("not x"));
        assert!(operators::has_ascii_operators("f circ g"));
        assert!(operators::has_ascii_operators("UNION(x, S, E)"));
        assert!(operators::has_ascii_operators("INTER(x, S, E)"));
        // "not" inside identifier should NOT match
        assert!(!operators::has_ascii_operators("notation"));
    }

    #[test]
    fn test_has_unicode_operators() {
        assert!(operators::has_unicode_operators("x ∧ y"));
        assert!(operators::has_unicode_operators("x ⇒ y"));
        assert!(!operators::has_unicode_operators("x + y"));
    }

    #[test]
    fn test_convert_to_unicode() {
        let provider = CodeActionProvider::new();
        assert_eq!(provider.convert_to_unicode("x & y", false), "x ∧ y");
        assert_eq!(provider.convert_to_unicode("x => y", false), "x ⇒ y");
        assert_eq!(provider.convert_to_unicode("x : NAT", false), "x ∈ ℕ");
        assert_eq!(provider.convert_to_unicode("x :: S", false), "x :∈ S");
        assert_eq!(
            provider.convert_to_unicode("x :| x' : NAT", false),
            "x :∣ x' ∈ ℕ"
        );
        assert_eq!(provider.convert_to_unicode("r~", false), "r∼");
        assert_eq!(
            provider.convert_to_unicode("x & y => z or w", false),
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
    fn test_convert_keeps_label_text() {
        // `@inv1.1` and `@safety-END` are names: the whole-document
        // conversion must not read their `.` and `-` as operators.
        let provider = CodeActionProvider::new();
        assert_eq!(
            provider.convert_to_unicode("@inv1.1 x : NAT\n@safety-END x - 1 > 0", false),
            "@inv1.1 x ∈ ℕ\n@safety-END x − 1 > 0"
        );
    }

    /// The whole-document conversion is offered as a `source.fixAll` on-save
    /// action, so it must never produce text that stops parsing. Component
    /// and event names are the trap — see
    /// `rossi::comments::LexicalSpans::names`.
    #[test]
    fn test_convert_keeps_component_and_event_names() {
        let provider = CodeActionProvider::new();
        let source = concat!(
            "MACHINE A-C0\n",
            "REFINES end-to-end\n",
            "SEES CTX-INT-1 A-or-B\n",
            "VARIABLES x\n",
            "INVARIANTS\n",
            "  @i x - 1 : NAT\n",
            "EVENTS\n",
            "EVENT do-step\n",
            "THEN\n",
            "  @a x := x - 1\n",
            "END\n",
            "END\n",
        );
        let converted = provider.convert_to_unicode(source, false);
        for name in ["A-C0", "end-to-end", "CTX-INT-1", "A-or-B", "do-step"] {
            assert!(
                converted.contains(name),
                "{name} was rewritten:\n{converted}"
            );
        }
        // The formulas around them still convert.
        assert!(converted.contains("x − 1 ∈ ℕ"), "{converted}");
        assert!(converted.contains("x ≔ x − 1"), "{converted}");
        // The invariant that matters for an on-save action: what it writes
        // back still parses. `A−C0` would not — `component_name` takes an
        // ASCII hyphen and nothing else.
        rossi::parse(source).expect("the fixture parses to begin with");
        rossi::parse(&converted).expect("the converted document must still parse");
    }

    #[test]
    fn test_roundtrip_ascii_unicode_ascii() {
        let provider = CodeActionProvider::new();
        let ascii_text = "x : NAT & x <= 10 => x /= 0";
        let unicode = provider.convert_to_unicode(ascii_text, false);
        let back = provider.convert_to_ascii(&unicode);
        assert_eq!(back, ascii_text);
    }

    #[test]
    fn test_roundtrip_set_operators() {
        let provider = CodeActionProvider::new();
        let ascii_text = "S <: T /\\ x : S \\/ T";
        let unicode = provider.convert_to_unicode(ascii_text, false);
        let back = provider.convert_to_ascii(&unicode);
        assert_eq!(back, ascii_text);
    }

    #[test]
    fn test_roundtrip_function_types() {
        let provider = CodeActionProvider::new();
        let ascii_text = "f : S --> T & g : S >-> T & h : S ->> T & k : S >->> T";
        let unicode = provider.convert_to_unicode(ascii_text, false);
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
