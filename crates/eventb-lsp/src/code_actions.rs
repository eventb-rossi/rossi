//! Code Actions for Event-B
//!
//! Provides quick fixes and refactorings including:
//! - Operator conversion (ASCII ↔ Unicode)
//! - Quick fixes for the rule diagnostics

use crate::cross_references::{ComponentKind, CrossReferenceManager, ReferenceKind};
use crate::document::DocumentManager;
use crate::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, CodeActionResponse,
    Position, Range, SymbolKind, TextEdit, Uri, WorkspaceEdit,
};
use crate::text_utils::{line_keyword, line_keyword_is, line_tight_end};
use crate::workspace::WorkspaceSymbolProvider;
use rossi::Component;
use rossi::keywords::KeywordId;
use rossi::operators;
use rossi_build::rules::RuleId;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// The source action that normalizes every operator to the configured
/// convention (`rossi.format.useUnicode`) and changes nothing else. A
/// `source.fixAll.*` kind so editors can run it on save — VS Code's
/// `editor.codeActionsOnSave` only triggers `source.*` kinds — without
/// reformatting the document the way `textDocument/formatting` would.
pub const FIX_ALL_KIND: CodeActionKind = CodeActionKind::new("source.fixAll.rossi");

/// Which nesting a misplaced clause sits in, and so which order names its
/// destination and which keywords the upward scan must not cross.
#[derive(Clone, Copy)]
enum MoveScope {
    /// An event's clauses (EB030): `ANY`, `WHERE`, `WITH`, `WITNESS`, `THEN`.
    EventClause,
    /// A context's or machine's sections (EB034).
    ComponentSection,
}

impl MoveScope {
    /// The keywords `clause` must precede, given the lines above it.
    ///
    /// A section is judged against its own component's list, found by scanning
    /// up for the header: `THEOREMS` is in both lists at different positions,
    /// so the keyword alone does not say which order applies. A section can
    /// never sit below the `EVENTS` block — the grammar pins it last — so the
    /// scan never crosses an event `END` on its way to the header.
    fn boundary(
        self,
        lines: &[&str],
        first: usize,
        clause: KeywordId,
    ) -> Option<&'static [KeywordId]> {
        match self {
            MoveScope::EventClause => Some(rossi::keywords::event_clause_boundary(clause)),
            MoveScope::ComponentSection => lines[..first]
                .iter()
                .rev()
                .filter_map(|line| line_keyword(line))
                .find_map(|keyword| match keyword {
                    KeywordId::Context => Some(rossi::keywords::context_clause_boundary(clause)),
                    KeywordId::Machine => Some(rossi::keywords::machine_clause_boundary(clause)),
                    _ => None,
                }),
        }
    }

    /// The keywords that close the region the clause may move within.
    fn stop_keywords(self) -> &'static [KeywordId] {
        match self {
            MoveScope::EventClause => &[KeywordId::Event, KeywordId::Events, KeywordId::End],
            MoveScope::ComponentSection => {
                &[KeywordId::Context, KeywordId::Machine, KeywordId::End]
            }
        }
    }

    /// Whether a comment above the clause header belongs to the clause and so
    /// has to move with it. Only a top-level clause carries a `ClauseRegion`
    /// for `rossi::comment_place` to anchor one to.
    fn carries_leading_comments(self) -> bool {
        matches!(self, MoveScope::ComponentSection)
    }
}

/// Whether the line at `index` holds a comment and nothing else.
///
/// `mask_comments_chars` is position-preserving and blanks every comment byte,
/// so such a line is blank in `masked` and not in `raw`. Comparing the two is
/// what tells it from an ordinary blank line, which must not be dragged along.
fn is_comment_only(raw: &[&str], masked: &[&str], index: usize) -> bool {
    masked.get(index).is_some_and(|line| line.trim().is_empty())
        && raw.get(index).is_some_and(|line| !line.trim().is_empty())
}

/// Whether the line at `index` opens inside a comment that started on an
/// earlier line, so the two cannot be separated without cutting the comment in
/// half. The byte before the line is its predecessor's newline, which a block
/// comment spanning both covers.
fn continues_a_comment(text: &str, index: usize) -> bool {
    index > 0
        && crate::position::position_to_offset(text, Position::new(index as u32, 0)).is_some_and(
            |offset| rossi::comments::offset_in_comment(text, offset.saturating_sub(1)),
        )
}

/// The line the section headed at `index` really begins on: the comment lines
/// written above it, which `rossi::comment_place` anchors to the section, move
/// with it.
///
/// A block comment opened on a code line is that line's trailing comment and
/// not the section's, so the lines it spans are given back: moving them would
/// split the comment.
fn section_start(text: &str, raw: &[&str], masked: &[&str], index: usize) -> usize {
    let mut start = index;
    while start > 0 && is_comment_only(raw, masked, start - 1) {
        start -= 1;
    }
    while start < index && continues_a_comment(text, start) {
        start += 1;
    }
    start
}

/// The operator style a conversion direction targets, as spelled in titles.
fn style_name(to_unicode: bool) -> &'static str {
    if to_unicode { "Unicode" } else { "ASCII" }
}

/// Whether the client's `only` filter admits `kind`: no filter, or an entry
/// equal to `kind` or a dot-delimited prefix of it (`source` and
/// `source.fixAll` both admit `source.fixAll.rossi`).
pub(crate) fn kind_requested(params: &CodeActionParams, kind: &CodeActionKind) -> bool {
    params.context.only.as_ref().is_none_or(|only| {
        only.iter().any(|requested| {
            kind.as_str()
                .strip_prefix(requested.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
        })
    })
}

/// A workspace edit applying `edits` to the document at `uri`.
pub(crate) fn document_edit(uri: &Uri, edits: Vec<TextEdit>) -> WorkspaceEdit {
    WorkspaceEdit::new(HashMap::from([(uri.clone(), edits)]))
}

/// A workspace edit replacing `range` of the document at `uri` with `new_text`.
pub(crate) fn single_edit(uri: &Uri, range: Range, new_text: String) -> WorkspaceEdit {
    document_edit(uri, vec![TextEdit { range, new_text }])
}

/// The edits moving the whole lines `first..=last` of `text` above line
/// `destination`.
fn move_lines(text: &str, first: usize, last: usize, destination: usize) -> Vec<TextEdit> {
    let moved = text
        .lines()
        .skip(first)
        .take(last - first + 1)
        .map(|line| format!("{line}\n"))
        .collect();
    let line = |index: usize| Position::new(index as u32, 0);
    vec![
        TextEdit {
            range: Range::new(line(destination), line(destination)),
            new_text: moved,
        },
        TextEdit {
            range: Range::new(line(first), line(last + 1)),
            new_text: String::new(),
        },
    ]
}

/// A quick fix answering `diagnostic` with `edit`.
fn quick_fix(
    title: String,
    diagnostic: &crate::lsp_types::Diagnostic,
    edit: WorkspaceEdit,
    preferred: bool,
) -> CodeAction {
    CodeAction {
        title,
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diagnostic.clone()]),
        edit: Some(edit),
        is_preferred: Some(preferred),
        ..Default::default()
    }
}

/// An empty range at byte `offset` of `text`, where an insert goes.
pub(crate) fn point(text: &str, offset: usize) -> Range {
    let position = crate::position::offset_to_position(text, offset);
    Range::new(position, position)
}

/// A workspace edit replacing the whole of `text` (at `uri`) with `new_text`.
fn full_document_edit(uri: &Uri, text: &str, new_text: String) -> WorkspaceEdit {
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

/// The start of the line holding byte `offset` of `text`.
pub(crate) fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |at| at + 1)
}

/// The end of the line holding byte `offset` of `text`, before its newline.
pub(crate) fn line_end(text: &str, offset: usize) -> usize {
    text[offset..]
        .find('\n')
        .map_or(text.len(), |at| offset + at)
}

/// The whole lines from the one holding byte `start` through the one holding
/// `end`, newline included, when nothing but whitespace and comments shares
/// them with `start..end`.
pub(crate) fn own_lines(
    text: &str,
    masked: &str,
    start: usize,
    end: usize,
) -> Option<std::ops::Range<usize>> {
    let (first, last) = (line_start(text, start), line_end(text, end));
    (masked[first..start].trim().is_empty() && masked[end..last].trim().is_empty())
        .then(|| first..(last + 1).min(text.len()))
}

/// The leading whitespace of `line`.
pub(crate) fn indentation(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

/// `names` added to a name list opened by `keyword` whose last name ends at
/// byte `end`: on the same line when the list is written inline (`sees a b`),
/// each on a line of its own at the same indentation when every name has one.
fn append_to_list(text: &str, end: usize, keyword: KeywordId, names: &[&str]) -> TextEdit {
    // A clause's span may run on over a comment after its last name, where a
    // name written would be commented out: write it after the last code.
    let end = rossi::comments::mask_comments(text)[..end].trim_end().len();
    let line = &text[line_start(text, end)..end];
    let separator = if line_keyword(line) == Some(keyword) {
        " ".to_string()
    } else {
        format!("\n{}", indentation(line))
    };
    TextEdit {
        range: point(text, end),
        new_text: names
            .iter()
            .map(|name| format!("{separator}{name}"))
            .collect(),
    }
}

/// The spelling of `keyword`, in lower case when `lowercase`.
pub(crate) fn keyword_text(keyword: KeywordId, lowercase: bool) -> String {
    let spelled = rossi::keywords::spell(keyword);
    if lowercase {
        spelled.to_lowercase()
    } else {
        spelled.to_string()
    }
}

/// A new `keyword names` clause on the line after the one holding byte
/// `after`, indented by `indent` and in lower case when `lowercase`. `None`
/// when a block comment opened on that line is still open at its end, where
/// it would swallow the clause.
fn new_clause_line(
    text: &str,
    after: usize,
    indent: &str,
    keyword: KeywordId,
    lowercase: bool,
    names: &[&str],
) -> Option<TextEdit> {
    after_line(
        text,
        after,
        format!(
            "\n{indent}{} {}",
            keyword_text(keyword, lowercase),
            names.join(" ")
        ),
    )
}

/// `new_text` inserted at the end of the line holding byte `offset`, or
/// `None` when a block comment opened there is still open at its end, where
/// it would swallow the insert.
pub(crate) fn after_line(text: &str, offset: usize, new_text: String) -> Option<TextEdit> {
    let end = line_end(text, offset);
    (!rossi::comments::offset_in_comment(text, end)).then(|| TextEdit {
        range: point(text, end),
        new_text,
    })
}

/// `name` added to `component`'s name-list `clause` (EXTENDS, SEES,
/// VARIABLES, SETS or CONSTANTS), or `None` when there is nowhere safe to
/// write it.
///
/// An existing clause gets the name after its last one. A missing clause is
/// written where `rossi fmt` would put it: on the line after the last clause
/// that must precede it, or after the header, in the header keyword's case.
fn component_list_insert(
    text: &str,
    component: &Component,
    clause: KeywordId,
    name: &str,
) -> Option<TextEdit> {
    if let Some(region) = component.clauses().iter().find(|r| r.keyword == clause) {
        return Some(append_to_list(text, region.span.end, clause, &[name]));
    }
    let follows = match component {
        Component::Context(_) => rossi::keywords::context_clause_boundary(clause),
        Component::Machine(_) => rossi::keywords::machine_clause_boundary(clause),
    };
    let header = component.span()?.start;
    let after = component
        .clauses()
        .iter()
        .filter(|r| !follows.contains(&r.keyword))
        .map(|r| r.span.end)
        .chain(component.name_span().map(|span| span.end))
        .max()?;
    new_clause_line(
        text,
        after,
        indentation(&text[line_start(text, header)..header]),
        clause,
        text[header..].starts_with(char::is_lowercase),
        &[name],
    )
}

/// The end of `event`'s header: past its name and any REFINES targets.
pub(crate) fn event_header_end(event: &rossi::Event) -> Option<usize> {
    event
        .refines
        .iter()
        .filter_map(|target| target.span)
        .map(|s| s.end)
        .chain(event.name_span.map(|s| s.end))
        .max()
}

/// The indentation of `event`'s clause keywords: that of the first one
/// written under the header, or for an event with no clause yet its
/// header's indentation and two more. The event's END is no clause: in the
/// camille layout it sits at the header's indentation.
pub(crate) fn event_clause_indent(text: &str, event: &rossi::Event) -> Option<String> {
    let span = event.span?;
    let header_line = &text[line_start(text, span.start)..span.start];
    Some(
        text[event_header_end(event)?..span.end]
            .lines()
            .skip(1)
            .find(|line| line_keyword(line).is_some_and(|keyword| keyword != KeywordId::End))
            .map(|line| indentation(line).to_string())
            .unwrap_or_else(|| format!("{}  ", indentation(header_line))),
    )
}

/// Whether `event`'s header spells EVENT in lower case.
pub(crate) fn event_is_lowercase(text: &str, event: &rossi::Event) -> bool {
    event.span.is_some_and(|span| {
        text[span.start..span.end]
            .split_whitespace()
            .find(|word| line_keyword(word) == Some(KeywordId::Event))
            .is_some_and(|word| word.starts_with(char::is_lowercase))
    })
}

/// `names` added to `event`'s parameters, or `None` when there is nowhere
/// safe to write them. A new ANY clause goes after the header and any REFINES
/// targets, indented like the event's other clauses.
pub(crate) fn parameter_insert(
    text: &str,
    event: &rossi::Event,
    names: &[&str],
) -> Option<TextEdit> {
    if let Some(last) = event
        .parameters
        .iter()
        .filter_map(|p| p.span)
        .map(|s| s.end)
        .max()
    {
        return Some(append_to_list(text, last, KeywordId::Any, names));
    }
    new_clause_line(
        text,
        event_header_end(event)?,
        &event_clause_indent(text, event)?,
        KeywordId::Any,
        event_is_lowercase(text, event),
        names,
    )
}

/// A label for the item at byte `item` that no other label in its scope
/// takes, nor any of `also_taken`. `masked` is `text` with its comment bytes
/// blanked, so byte offsets agree between the two.
///
/// The stem follows Rodin's own naming for the enclosing clause
/// ([`label_stem`]), whose keyword is the last one written before the item:
/// on the item's own line for an inline `EVENT e THEN x ≔ 1 END`, on a line
/// above it for the indented form. A guard, witness or action label is unique
/// within its event, an axiom, invariant or theorem within the component, and
/// Rodin numbers each event's items from 1, so the first free number is looked
/// for in the scope a clash would be found in.
fn free_label(
    text: &str,
    lexical: &rossi::comments::LexicalSpans,
    masked: &str,
    item: usize,
    also_taken: &HashSet<String>,
) -> Option<String> {
    let stem = masked[..item]
        .split_whitespace()
        .rev()
        .find_map(label_stem)?;
    let scope = label_scope(stem, masked, item);
    let taken: HashSet<&str> = lexical
        .labels
        .iter()
        .filter(|span| scope.contains(&span.start))
        // The span covers `@name`; only the leading sigil is syntax.
        .map(|span| &text[span.start + 1..span.end])
        .collect();
    (1..)
        .map(|n| format!("{stem}{n}"))
        .find(|label| !taken.contains(label.as_str()) && !also_taken.contains(label))
}

/// The byte range a label with `stem` must be unique in: its event for a
/// guard, witness or action, the whole document otherwise.
fn label_scope(stem: &str, masked: &str, item: usize) -> std::ops::Range<usize> {
    match stem {
        "grd" | "wit" | "act" => enclosing_event_range(masked, item),
        _ => 0..masked.len(),
    }
}

/// The groupings an incompatible-operators `error` offers, located in the
/// document `text`. The first such error of a document is reported bare; one
/// the recovery met after another error is wrapped, its positions relative to
/// the predicate it failed on, and is placed by that predicate's start.
fn incompatible_groupings(error: &rossi::ParseError, text: &str) -> Option<Vec<rossi::ast::Span>> {
    use rossi::ParseError;
    let (groupings, base, within) = match error {
        ParseError::IncompatibleOperators { groupings, .. } => (groupings, 0, 0..text.len()),
        ParseError::RecoverableError {
            span: Some(element),
            source: Some(source),
            ..
        } => match source.as_ref() {
            ParseError::IncompatibleOperators { groupings, .. } => {
                (groupings, element.start, element.start..element.end)
            }
            _ => return None,
        },
        _ => return None,
    };
    let located: Vec<rossi::ast::Span> = groupings
        .iter()
        .map(|group| rossi::ast::Span {
            start: base + group.start,
            end: base + group.end,
        })
        .collect();
    let sound = located.iter().all(|group| {
        within.start <= group.start
            && group.end <= within.end
            && text.get(group.start..group.end).is_some()
    });
    (sound && !located.is_empty()).then_some(located)
}

/// `text` shortened for a title: at most 40 characters, the cut marked.
fn abbreviated(text: &str) -> String {
    if text.chars().count() <= 40 {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(39).collect::<String>())
    }
}

/// Whether `pred` has a top-level conjunct of the shape that types `name`
/// on its own: `name ∈ E`, `name ⊆ E` or `name = E`, the name bare on the
/// left.
fn types_by_shape(pred: &rossi::Predicate, name: &str) -> bool {
    use rossi::formula::PredicateKind;
    use rossi::formula::tag::{AssocPredOp, RelationalOp};
    match pred.kind() {
        PredicateKind::Associative {
            op: AssocPredOp::LAnd,
            children,
        } => children.iter().any(|child| types_by_shape(child, name)),
        PredicateKind::Relational {
            op: RelationalOp::In | RelationalOp::SubsetEq | RelationalOp::Equal,
            left,
            ..
        } => matches!(left.kind(), rossi::formula::ExpressionKind::FreeIdentifier(n) if n == name),
        _ => false,
    }
}

/// The optimal string alignment distance between `a` and `b`: edits of one
/// character, a swap of two adjacent ones counting as one edit.
fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut rows = vec![vec![0; b.len() + 1]; a.len() + 1];
    for (i, row) in rows.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in rows[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (rows[i - 1][j] + 1)
                .min(rows[i][j - 1] + 1)
                .min(rows[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(rows[i - 2][j - 2] + 1);
            }
            rows[i][j] = best;
        }
    }
    rows[a.len()][b.len()]
}

/// The `candidates` close enough to `name` to be what was meant, closest
/// first, at most three. A short name admits fewer edits, and one of one or
/// two characters none: any other short name is as close as the right one.
fn spelled_alike(name: &str, candidates: &[String]) -> Vec<String> {
    let budget = match name.chars().count() {
        0..=2 => return Vec::new(),
        3..=4 => 1,
        _ => 2,
    };
    let mut close: Vec<(usize, &String)> = candidates
        .iter()
        .filter(|candidate| candidate.as_str() != name)
        .map(|candidate| (edit_distance(name, candidate), candidate))
        .filter(|(distance, _)| *distance <= budget)
        .collect();
    close.sort();
    close.dedup();
    close.into_iter().take(3).map(|(_, c)| c.clone()).collect()
}

/// Provides code actions and refactorings
pub struct CodeActionProvider {
    /// The workspace's declared names, for the fixes that look beyond the
    /// document: which context declares a name the document does not.
    workspace_symbols: Option<Arc<WorkspaceSymbolProvider>>,
    /// The workspace's component graph, so a fix never adds an EXTENDS that
    /// closes a cycle, and knows the components a misspelled target may mean.
    cross_ref_manager: Option<Arc<CrossReferenceManager>>,
    /// The open documents, whose dependency closure the fixes reading the
    /// checked model check.
    document_manager: Option<Arc<DocumentManager>>,
}

impl CodeActionProvider {
    pub fn new() -> Self {
        Self {
            workspace_symbols: None,
            cross_ref_manager: None,
            document_manager: None,
        }
    }

    /// Set the document manager whose parses the model-reading fixes check.
    pub fn set_document_manager(&mut self, manager: Arc<DocumentManager>) {
        self.document_manager = Some(manager);
    }

    /// The checked model of the open document at `uri` and its dependency
    /// closure, as the semantic diagnostics see it.
    fn checked_model(&self, uri: &Uri) -> Option<rossi_build::sc_model::ScModel> {
        let documents = self.document_manager.as_ref()?;
        let manager = self.cross_ref_manager.as_ref()?;
        let doc = documents.parse_result(uri)?;
        let loader = crate::component_loader::ComponentLoader::new(manager, Some(documents));
        let project = crate::closure::project_for(&doc, &loader, "lsp-code-actions");
        Some(rossi_build::check_with_model(&project).1)
    }

    /// Set the workspace symbol index the cross-file fixes search.
    pub fn set_workspace_symbols(&mut self, symbols: Arc<WorkspaceSymbolProvider>) {
        self.workspace_symbols = Some(symbols);
    }

    /// Set the cross-reference manager the cross-file fixes consult.
    pub fn set_cross_reference_manager(&mut self, manager: Arc<CrossReferenceManager>) {
        self.cross_ref_manager = Some(manager);
    }

    /// [`Self::provide_code_actions_parsed`] over a parse of `text` made
    /// here, for a caller holding none of its own.
    pub fn provide_code_actions(
        &self,
        params: &CodeActionParams,
        text: &str,
        use_unicode: bool,
        private_use_glyphs: bool,
    ) -> Option<CodeActionResponse> {
        let parsed = rossi::parse_components_with_recovery(text);
        self.provide_code_actions_parsed(params, text, &parsed, use_unicode, private_use_glyphs)
    }

    /// Provide code actions for a given document position/range.
    /// `parsed` is the recovered parse of `text`, which every fix reading the
    /// AST or the parse errors shares rather than parsing again. `use_unicode`
    /// is the operator convention (`rossi.format.useUnicode`) the fix-all
    /// source action normalizes to; `private_use_glyphs`
    /// (`rossi.format.privateUseGlyphs`) is how a conversion toward Unicode
    /// spells the four relation/override operators.
    pub fn provide_code_actions_parsed(
        &self,
        params: &CodeActionParams,
        text: &str,
        parsed: &rossi::ParseResult<Vec<Component>>,
        use_unicode: bool,
        private_use_glyphs: bool,
    ) -> Option<CodeActionResponse> {
        let components = parsed.component.as_deref().unwrap_or_default();
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
            actions.extend(self.provide_diagnostic_based_actions(
                params,
                text,
                components,
                &parsed.errors,
                private_use_glyphs,
            ));
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
        uri: &Uri,
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
        uri: &Uri,
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

    /// Provide diagnostic-based quick fixes. `components` and `errors` are
    /// the recovered parse of `text`.
    fn provide_diagnostic_based_actions(
        &self,
        params: &CodeActionParams,
        text: &str,
        components: &[Component],
        errors: &[rossi::ParseError],
        private_use_glyphs: bool,
    ) -> Vec<CodeActionOrCommand> {
        let mut actions = Vec::new();

        // Offer "Add missing END" only for a syntax error at end-of-input. A
        // missing terminator is reported there (pest's EOF position); a syntax
        // error inside the body sits earlier. Keying off the position, not the
        // old `message.contains("expected")`, which matched every syntax
        // error, avoids suggesting an END for a typo deep inside a predicate,
        // and (unlike a "no END anywhere" text scan) is not fooled by a nested
        // END (`if … then … else … end`, an event END, or an `END` inside a
        // label). A diagnostic carrying a rule code is about the model, not
        // its syntax, so it never qualifies. The component check is done
        // last, only once a candidate diagnostic exists.
        let end_of_input = crate::position::offset_to_position(text, text.len());
        if let Some(diagnostic) = params
            .context
            .diagnostics
            .iter()
            .find(|d| d.code.is_none() && d.range.start >= end_of_input)
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
            if let Some(action) = self.create_move_clause_action(
                &params.text_document.uri,
                diagnostic,
                text,
                MoveScope::EventClause,
            ) {
                actions.push(CodeActionOrCommand::CodeAction(action));
            }
        }

        // Move a section written below one it must precede (EB034).
        for diagnostic in params
            .context
            .diagnostics
            .iter()
            .filter(|d| diagnostic_code_is(d, RuleId::SectionOutOfOrder.code()))
        {
            if let Some(action) = self.create_move_clause_action(
                &params.text_document.uri,
                diagnostic,
                text,
                MoveScope::ComponentSection,
            ) {
                actions.push(CodeActionOrCommand::CodeAction(action));
            }
        }

        let uri = &params.text_document.uri;
        let with_code = |rule: RuleId| {
            params
                .context
                .diagnostics
                .iter()
                .filter(move |d| diagnostic_code_is(d, rule.code()))
        };

        // Parenthesize operators mixed without the parentheses Event-B
        // requires. The first such error is EB005; one recovered after another
        // error carries no code, so both are matched against the parse errors.
        for diagnostic in
            params.context.diagnostics.iter().filter(|d| {
                d.code.is_none() || diagnostic_code_is(d, RuleId::FormulaParseError.code())
            })
        {
            actions.extend(
                self.create_parenthesize_actions(uri, diagnostic, text, errors)
                    .into_iter()
                    .map(CodeActionOrCommand::CodeAction),
            );
        }

        // Some fixes below read the checked model, which is computed at most
        // once, and only when such a fix has a diagnostic to fix.
        let model = std::cell::OnceCell::new();

        // Relabel an item whose label another one takes (EB022).
        for diagnostic in with_code(RuleId::DuplicateLabel) {
            actions.extend(
                self.create_relabel_action(uri, diagnostic, text, components, &model)
                    .map(CodeActionOrCommand::CodeAction),
            );
        }

        // Move the predicate typing a name above the one reading it (EB020).
        for diagnostic in with_code(RuleId::UnknownType) {
            actions.extend(
                self.create_move_typing_action(uri, diagnostic, text, components)
                    .map(CodeActionOrCommand::CodeAction),
            );
        }

        // Keep a variable the refinement dropped but still uses (EB025).
        for diagnostic in with_code(RuleId::DisappearedVariable) {
            actions.extend(
                self.create_keep_variable_action(uri, diagnostic, text, components)
                    .map(CodeActionOrCommand::CodeAction),
            );
        }

        // See or extend the context that declares an undeclared name, or
        // declare it here (EB018).
        for diagnostic in with_code(RuleId::UndeclaredIdentifier) {
            actions.extend(
                self.create_import_context_actions(uri, diagnostic, text, components)
                    .into_iter()
                    .chain(self.create_declare_actions(uri, diagnostic, text, components))
                    .map(CodeActionOrCommand::CodeAction),
            );
        }

        // A name nothing resolves (EB018) or a component or abstract event
        // nothing declares (EB009) may be a misspelling of one in scope.
        for diagnostic in
            with_code(RuleId::UndeclaredIdentifier).chain(with_code(RuleId::CrossReferenceNotFound))
        {
            actions.extend(
                self.create_respell_actions(uri, diagnostic, text, components, &model)
                    .into_iter()
                    .map(CodeActionOrCommand::CodeAction),
            );
        }

        // Write an ordinary space for a separator Camille cannot read (EB031).
        for diagnostic in with_code(RuleId::NonPortableWhitespace) {
            let mut separator = text_in_range(text, diagnostic.range)
                .into_iter()
                .flat_map(str::chars);
            if let (Some(c), None) = (separator.next(), separator.next())
                && rossi::keywords::camille_unreadable_separator(c)
            {
                actions.push(CodeActionOrCommand::CodeAction(quick_fix(
                    format!("Replace U+{:04X} with a space", c as u32),
                    diagnostic,
                    single_edit(uri, diagnostic.range, " ".to_string()),
                    true,
                )));
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
        uri: &Uri,
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

    /// Quick fixes for EB018 (an undeclared identifier) naming a constant or
    /// carrier set that a workspace context declares: a machine sees that
    /// context, a context extends it. One fix per declaring context, the only
    /// one preferred.
    ///
    /// The diagnostic underlines the name itself, so the range is the name.
    /// A context that already extends the document's context (directly or
    /// not) is left out, since extending it back would close a cycle.
    fn create_import_context_actions(
        &self,
        uri: &Uri,
        diagnostic: &crate::lsp_types::Diagnostic,
        text: &str,
        components: &[Component],
    ) -> Vec<CodeAction> {
        let Some(symbols) = &self.workspace_symbols else {
            return Vec::new();
        };
        let Some(name) = text_in_range(text, diagnostic.range)
            .filter(|name| rossi::names::is_valid_math_identifier(name))
        else {
            return Vec::new();
        };
        let Some(component) = crate::position::position_to_offset(text, diagnostic.range.start)
            .and_then(|offset| crate::component_util::component_at_offset(components, offset))
        else {
            return Vec::new();
        };
        let (clause, present) = match component {
            Component::Machine(machine) => (KeywordId::Sees, &machine.sees),
            Component::Context(context) => (KeywordId::Extends, &context.extends),
        };
        let closes_cycle = |target: &str| {
            clause == KeywordId::Extends
                && self.cross_ref_manager.as_ref().is_some_and(|manager| {
                    manager
                        .transitive_closure(target, ReferenceKind::Extends)
                        .iter()
                        .any(|ancestor| ancestor == component.name())
                })
        };
        let mut contexts: Vec<String> = symbols
            .declarations_of(name)
            .into_iter()
            .filter(|(_, kind)| matches!(*kind, SymbolKind::CONSTANT | SymbolKind::ENUM))
            .map(|(container, _)| container)
            .filter(|target| {
                target != component.name() && !present.contains(target) && !closes_cycle(target)
            })
            .collect();
        contexts.sort();
        contexts.dedup();
        let preferred = contexts.len() == 1;
        contexts
            .iter()
            .filter_map(|target| {
                let insert = component_list_insert(text, component, clause, target)?;
                Some(quick_fix(
                    format!("Add {target} to {}", rossi::keywords::spell(clause)),
                    diagnostic,
                    document_edit(uri, vec![insert]),
                    preferred,
                ))
            })
            .collect()
    }

    /// Quick fix for EB022 (a label another item already takes): give the
    /// item a free label. Of two items sharing a label, the finding marks the
    /// first; the one written last is relabeled, so the label an existing
    /// proof is filed under stays where it was. A label clashing with one an
    /// extended event inherits is relabeled clear of the inherited labels,
    /// which the checked model knows.
    fn create_relabel_action(
        &self,
        uri: &Uri,
        diagnostic: &crate::lsp_types::Diagnostic,
        text: &str,
        components: &[Component],
        model: &std::cell::OnceCell<Option<rossi_build::sc_model::ScModel>>,
    ) -> Option<CodeAction> {
        let lexical = rossi::comments::lexical_spans(text);
        let masked = lexical.mask_comments(text);
        let start = crate::position::position_to_offset(text, diagnostic.range.start)?;
        let end = crate::position::position_to_offset(text, diagnostic.range.end)?;
        let marked = lexical
            .labels
            .iter()
            .find(|span| (start..end.max(start + 1)).contains(&span.start))?;
        let old = &text[marked.start + 1..marked.end];
        let stem = masked[..marked.start]
            .split_whitespace()
            .rev()
            .find_map(label_stem)?;
        let scope = label_scope(stem, &masked, marked.start);
        let same: Vec<_> = lexical
            .labels
            .iter()
            .filter(|span| scope.contains(&span.start) && &text[span.start + 1..span.end] == old)
            .collect();
        let target = **same.last()?;

        // The labels an extended event inherits share its guards' and
        // actions' namespace without being written in this document.
        let mut inherited = HashSet::new();
        if matches!(stem, "grd" | "act")
            && let Some(Component::Machine(machine)) =
                crate::component_util::component_at_offset(components, target.start)
            && let Some(event) = crate::symbols::event_at_offset(machine, target.start)
            && event.extended
            && let Some(decl) = model
                .get_or_init(|| self.checked_model(uri))
                .as_ref()
                .and_then(|model| model.machines.get(&machine.name))
                .and_then(|checked| checked.events_by_label.get(&event.name))
        {
            for ancestor in decl.chain_root_first() {
                inherited.extend(ancestor.guards.iter().map(|g| g.label.clone()));
                inherited.extend(ancestor.own_actions().iter().map(|a| a.label.clone()));
            }
        }
        let label = free_label(text, &lexical, &masked, target.start, &inherited)?;
        let title = if same.len() > 1 {
            format!("Relabel the last @{old} as @{label}")
        } else {
            format!("Relabel @{old} as @{label}")
        };
        let range = crate::position::span_to_range(
            &rossi::ast::Span {
                start: target.start + 1,
                end: target.end,
            },
            text,
        );
        Some(quick_fix(
            title,
            diagnostic,
            single_edit(uri, range, label),
            true,
        ))
    }

    /// Quick fixes for operators mixed without the parentheses Event-B
    /// requires: parenthesize one of the groupings the parser says resolve
    /// it. `errors` is the document's parse; the fixes answer the error the
    /// diagnostic was made from. A grouping is offered only when the document
    /// parses with fewer errors once it is parenthesized, so one that moves
    /// the mistake along the chain is not.
    fn create_parenthesize_actions(
        &self,
        uri: &Uri,
        diagnostic: &crate::lsp_types::Diagnostic,
        text: &str,
        errors: &[rossi::ParseError],
    ) -> Vec<CodeAction> {
        let Some(groupings) = errors.iter().find_map(|error| {
            let groupings = incompatible_groupings(error, text)?;
            (crate::diagnostics::parse_error_to_diagnostic(error, text).range == diagnostic.range)
                .then_some(groupings)
        }) else {
            return Vec::new();
        };
        groupings
            .into_iter()
            .filter_map(|group| {
                let grouped = &text[group.start..group.end];
                let fixed = format!("{}({grouped}){}", &text[..group.start], &text[group.end..]);
                if rossi::parse_components_with_recovery(&fixed).errors.len() >= errors.len() {
                    return None;
                }
                let at = |offset| {
                    let position = crate::position::offset_to_position(text, offset);
                    Range::new(position, position)
                };
                let edits = vec![
                    TextEdit {
                        range: at(group.start),
                        new_text: "(".to_string(),
                    },
                    TextEdit {
                        range: at(group.end),
                        new_text: ")".to_string(),
                    },
                ];
                Some(quick_fix(
                    format!("Parenthesize {}", abbreviated(grouped)),
                    diagnostic,
                    document_edit(uri, edits),
                    false,
                ))
            })
            .collect()
    }

    /// Quick fix for EB020 (a predicate reads names before any predicate of
    /// its list types them): move the predicate that types one of them above
    /// the one reading it, with the comment lines written above each kept
    /// with its predicate.
    ///
    /// The typing predicate is the first later one in the same list (axioms,
    /// invariants, or an event's guards) with a conjunct of the typing shape
    /// `n ∈ E`, `n ⊆ E` or `n = E` for a name the reading predicate uses; failing
    /// that, the first later one using the name the finding underlines.
    fn create_move_typing_action(
        &self,
        uri: &Uri,
        diagnostic: &crate::lsp_types::Diagnostic,
        text: &str,
        components: &[Component],
    ) -> Option<CodeAction> {
        let name = text_in_range(text, diagnostic.range)?;
        let offset = crate::position::position_to_offset(text, diagnostic.range.start)?;
        let lists: Vec<&[rossi::LabeledPredicate]> =
            match crate::component_util::component_at_offset(components, offset)? {
                Component::Context(context) => vec![&context.axioms],
                Component::Machine(machine) => std::iter::once(machine.invariants.as_slice())
                    .chain(machine.events.iter().map(|event| event.guards.as_slice()))
                    .collect(),
            };
        let (list, read) = lists.into_iter().find_map(|list| {
            let index = list
                .iter()
                .position(|item| item.span.is_some_and(|span| span.contains(offset)))?;
            Some((list, index))
        })?;
        let reading = &list[read];
        let later = &list[read + 1..];
        let typing = later
            .iter()
            .find(|item| {
                reading
                    .predicate
                    .free_identifiers()
                    .iter()
                    .any(|read| types_by_shape(&item.predicate, read))
            })
            .or_else(|| {
                later
                    .iter()
                    .find(|item| item.predicate.free_identifiers().iter().any(|n| n == name))
            })?;

        let masked = rossi::comments::mask_comments(text);
        let (raw_lines, masked_lines): (Vec<&str>, Vec<&str>) =
            (text.lines().collect(), masked.lines().collect());
        let line_index = |offset: usize| text[..offset].matches('\n').count();
        // Whole lines move, so each predicate must have its lines to itself.
        let owns_lines =
            |span: rossi::ast::Span| own_lines(text, &masked, span.start, span.end).is_some();
        // A span may run on over a comment written after the predicate; the
        // predicate ends at its last character of code.
        let code = |span: rossi::ast::Span| rossi::ast::Span {
            start: span.start,
            end: line_tight_end(&masked, span),
        };
        let (read_span, typing_span) = (code(reading.span?), code(typing.span?));
        if !owns_lines(read_span) || !owns_lines(typing_span) {
            return None;
        }
        let destination =
            section_start(text, &raw_lines, &masked_lines, line_index(read_span.start));
        let first = section_start(
            text,
            &raw_lines,
            &masked_lines,
            line_index(typing_span.start),
        );
        let last = line_index(typing_span.end);
        if first <= line_index(read_span.end) {
            return None;
        }
        let label = |item: &rossi::LabeledPredicate| {
            item.label
                .as_deref()
                .map_or_else(|| "the predicate".to_string(), |label| format!("@{label}"))
        };
        Some(quick_fix(
            format!("Move {} above {}", label(typing), label(reading)),
            diagnostic,
            document_edit(uri, move_lines(text, first, last, destination)),
            true,
        ))
    }

    /// Quick fix for EB025 (a variable the refinement dropped is still used):
    /// declare it again in the machine's VARIABLES, which is how Event-B
    /// keeps an abstract variable. The diagnostic underlines the variable.
    fn create_keep_variable_action(
        &self,
        uri: &Uri,
        diagnostic: &crate::lsp_types::Diagnostic,
        text: &str,
        components: &[Component],
    ) -> Option<CodeAction> {
        let written = text_in_range(text, diagnostic.range)?;
        let name = crate::formula_walk::canonical(written);
        if !rossi::names::is_valid_math_identifier(name) {
            return None;
        }
        let offset = crate::position::position_to_offset(text, diagnostic.range.start)?;
        let component = crate::component_util::component_at_offset(components, offset)?;
        if !matches!(component, Component::Machine(_)) {
            return None;
        }
        let insert = component_list_insert(text, component, KeywordId::Variables, name)?;
        Some(quick_fix(
            format!("Keep {name} in VARIABLES"),
            diagnostic,
            document_edit(uri, vec![insert]),
            true,
        ))
    }

    /// Quick fixes replacing a name nothing declares with one spelled alike:
    /// for EB018, a name in scope where it is used (the event's parameters
    /// included); for EB009, a component of the kind the clause names, or an
    /// event of the abstract machine for an event's REFINES target. Closest
    /// first, at most three, preferred when there is only one. `model` is the
    /// document's checked model, computed on first use.
    fn create_respell_actions(
        &self,
        uri: &Uri,
        diagnostic: &crate::lsp_types::Diagnostic,
        text: &str,
        components: &[Component],
        model: &std::cell::OnceCell<Option<rossi_build::sc_model::ScModel>>,
    ) -> Vec<CodeAction> {
        let Some(name) = text_in_range(text, diagnostic.range) else {
            return Vec::new();
        };
        let Some(offset) = crate::position::position_to_offset(text, diagnostic.range.start) else {
            return Vec::new();
        };
        let Some(component) = crate::component_util::component_at_offset(components, offset) else {
            return Vec::new();
        };
        let event = match component {
            Component::Machine(machine) => crate::symbols::event_at_offset(machine, offset),
            Component::Context(_) => None,
        };
        let model = || model.get_or_init(|| self.checked_model(uri)).as_ref();
        let names = |env: &rossi_build::type_env::TypeEnv| {
            env.iter()
                .map(|(name, _)| name.to_string())
                .collect::<Vec<_>>()
        };
        let in_event_target = event.is_some_and(|event| {
            event
                .refines
                .iter()
                .any(|target| target.span.is_some_and(|span| span.contains(offset)))
        });
        let candidates: Vec<String> = match component {
            _ if diagnostic_code_is(diagnostic, RuleId::UndeclaredIdentifier.code()) => {
                match component {
                    Component::Context(context) => model()
                        .and_then(|model| model.contexts.get(&context.name))
                        .map(|checked| names(checked.env())),
                    Component::Machine(machine) => model()
                        .and_then(|model| model.machines.get(&machine.name))
                        .map(|checked| {
                            match event.and_then(|e| checked.events_by_label.get(&e.name)) {
                                Some(decl) => names(&checked.event_env(decl)),
                                None => names(checked.env()),
                            }
                        }),
                }
                .unwrap_or_default()
            }
            Component::Machine(machine) if in_event_target => machine
                .refines
                .as_ref()
                .and_then(|parent| model()?.machines.get(parent))
                .map(|parent| parent.events_by_label.keys().cloned().collect())
                .unwrap_or_default(),
            _ => {
                let Some(manager) = &self.cross_ref_manager else {
                    return Vec::new();
                };
                let kind = match component {
                    Component::Machine(machine) if machine.refines.as_deref() == Some(name) => {
                        ComponentKind::Machine
                    }
                    _ => ComponentKind::Context,
                };
                manager
                    .component_names_of_kind(kind)
                    .into_iter()
                    .filter(|candidate| candidate != component.name())
                    .collect()
            }
        };
        let similar = spelled_alike(name, &candidates);
        let preferred = similar.len() == 1;
        similar
            .into_iter()
            .map(|replacement| {
                quick_fix(
                    format!("Change to {replacement}"),
                    diagnostic,
                    single_edit(uri, diagnostic.range, replacement),
                    preferred,
                )
            })
            .collect()
    }

    /// Quick fixes for EB018 (an undeclared identifier) that declare the name
    /// where it is used: a constant of a context; a variable of a machine, and
    /// also a parameter of the event it is used in. Never preferred, since
    /// each introduces a symbol the user has to type. A primed name is left
    /// alone: an after-state name is never declared, the prime is the error.
    fn create_declare_actions(
        &self,
        uri: &Uri,
        diagnostic: &crate::lsp_types::Diagnostic,
        text: &str,
        components: &[Component],
    ) -> Vec<CodeAction> {
        let Some(name) = text_in_range(text, diagnostic.range).filter(|name| {
            rossi::names::is_valid_math_identifier(name)
                && !rossi::names::is_primed_identifier(name)
        }) else {
            return Vec::new();
        };
        let Some(offset) = crate::position::position_to_offset(text, diagnostic.range.start) else {
            return Vec::new();
        };
        let Some(component) = crate::component_util::component_at_offset(components, offset) else {
            return Vec::new();
        };
        let mut declarations = Vec::new();
        match component {
            Component::Context(_) => declarations.push((
                format!("Declare {name} as a constant"),
                component_list_insert(text, component, KeywordId::Constants, name),
            )),
            Component::Machine(machine) => {
                declarations.push((
                    format!("Declare {name} as a variable"),
                    component_list_insert(text, component, KeywordId::Variables, name),
                ));
                if let Some(event) = crate::symbols::event_at_offset(machine, offset) {
                    declarations.push((
                        format!("Declare {name} as a parameter of {}", event.name),
                        parameter_insert(text, event, &[name]),
                    ));
                }
            }
        }
        declarations
            .into_iter()
            .filter_map(|(title, insert)| {
                Some(quick_fix(
                    title,
                    diagnostic,
                    document_edit(uri, vec![insert?]),
                    false,
                ))
            })
            .collect()
    }

    /// Quick fix for EB026 (assignment operator in a predicate). The diagnostic
    /// range underlines just the becomes operator; replace it with the predicate
    /// operator it was most likely meant to be — `:=`/`≔` → `=` (equality),
    /// `:∈`/`::` → `∈` (membership). `:|`/`:∣` (becomes-such-that) has a
    /// predicate right-hand side that cannot be rewritten by a single-token swap,
    /// so no fix is offered for it (the diagnostic still stands).
    fn create_fix_assignment_in_predicate_action(
        &self,
        uri: &Uri,
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
        uri: &Uri,
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
        uri: &Uri,
        diagnostic: &crate::lsp_types::Diagnostic,
        text: &str,
    ) -> Option<CodeAction> {
        // One lexical scan serves both the mask and the labels already written.
        let lexical = rossi::comments::lexical_spans(text);
        let masked = lexical.mask_comments(text);
        let item = crate::position::position_to_offset(text, diagnostic.range.start)?;
        let label = free_label(text, &lexical, &masked, item, &HashSet::new())?;
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

    /// Quick fix for EB030 (an event clause written out of order) and EB034
    /// (a component section): move the clause above the earliest one it must
    /// precede.
    ///
    /// The diagnostic spans the whole clause, so the lines it covers are what
    /// moves. The destination comes from the clause order the parser and the
    /// lint already read ([`rossi::keywords::event_clause_boundary`] and its
    /// component siblings), scanning up only to the line that opens or closes
    /// the enclosing region, so a clause is never lifted out of it.
    fn create_move_clause_action(
        &self,
        uri: &Uri,
        diagnostic: &crate::lsp_types::Diagnostic,
        text: &str,
        scope: MoveScope,
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
        let follows = scope.boundary(&lines, first, clause)?;
        let stop = scope.stop_keywords();
        let mut target = None;
        for (index, line) in lines[..first].iter().enumerate().rev() {
            let Some(keyword) = line_keyword(line) else {
                continue;
            };
            // The region this clause belongs to starts here, so the scan
            // stops. For an event: `END` closes the event above (an inline
            // status — `convergent EVENT e` — hides the header keyword, so
            // `EVENT` alone is not enough to stay inside the event), and
            // `EVENTS` opens the block. For a section: the component header,
            // or the `END` of the component above it in a multi-component
            // file.
            if stop.contains(&keyword) {
                break;
            }
            if follows.contains(&keyword) {
                target = Some((index, keyword));
            }
        }
        let (mut target_line, target_keyword) = target?;
        // A comment written above a section header belongs to that section —
        // only a top-level clause carries a `ClauseRegion` for
        // `rossi::comment_place` to anchor one to — so it has to travel with
        // it, or the move re-files it under whatever ends up there instead.
        // The destination is read the same way, or the insert lands between
        // the target section and its own comment and re-files that one. An
        // event's inner clause keywords carry no region, so a comment above
        // `WHERE` is already anchored to the first guard and stays put.
        let mut first = first;
        if scope.carries_leading_comments() {
            let raw: Vec<&str> = text.lines().collect();
            first = section_start(text, &raw, &lines, first);
            target_line = section_start(text, &raw, &lines, target_line);
        }
        if target_line >= first {
            return None;
        }
        let edits = move_lines(text, first, last, target_line);
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
        uri: &Uri,
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
    fn test_text_in_range_single_line() {
        let text = "hello world";
        let range = Range {
            start: Position::new(0, 0),
            end: Position::new(0, 5),
        };
        assert_eq!(text_in_range(text, range), Some("hello"));
    }

    #[test]
    fn test_text_in_range_multi_line() {
        let text = "line1\nline2\nline3";
        let range = Range {
            start: Position::new(0, 2),
            end: Position::new(2, 3),
        };
        assert_eq!(text_in_range(text, range), Some("ne1\nline2\nlin"));
    }

    #[test]
    fn test_text_in_range_unicode() {
        // "x ∈ ℕ" — ∈ is 3 bytes, ℕ is 3 bytes, but each is 1 character
        let text = "x ∈ ℕ ∧ y ≤ 10";
        // Character positions: x(0) (1)∈(2) (3)ℕ(4) (5)∧(6) (7)y(8) (9)≤(10) (11)1(12)0(13)
        let range = Range {
            start: Position::new(0, 2),
            end: Position::new(0, 4),
        };
        assert_eq!(text_in_range(text, range), Some("∈ "));
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
}
