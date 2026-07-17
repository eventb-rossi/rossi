//! Code completion provider for Event-B
//!
//! Provides intelligent auto-completion for:
//! - Keywords (context-aware based on position)
//! - Operators (Unicode and ASCII variants)
//! - Identifiers (variables, constants, sets, parameters)
//! - Snippets (common patterns like events, axioms)

use crate::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionParams, CompletionResponse, CompletionTextEdit,
    Documentation, InsertTextFormat, Position, Range, TextEdit,
};
use rossi::{Component, keywords, operators};

use crate::component_loader::ComponentLoader;
use crate::component_util::{component_at_offset, component_reference_clause};
use crate::config::{CompletionConfig, FormatConfig};
use crate::identifier_utils::position_to_offset;
use crate::position::{line_run_to_range, utf16_to_byte, utf16_to_char_col};
use crate::resolved_environment::ResolvedEnvironment;
use std::collections::HashSet;
use std::sync::Arc;

use crate::cross_references::CrossReferenceManager;
use crate::document::DocumentManager;
use crate::text_utils;

/// Completion context - tracks what's available at the cursor position
#[derive(Debug, Clone)]
struct CompletionContext {
    /// Variables available in current scope
    variables: Vec<String>,
    /// Constants available in current scope (from seen contexts)
    constants: Vec<String>,
    /// Sets available in current scope (from seen contexts)
    sets: Vec<String>,
    /// Parameters from current event's ANY clause
    parameters: Vec<String>,
    /// Formula binders (∀/∃/λ/comprehension/⋃/⋂) in scope at the cursor
    locals: Vec<String>,
}

impl CompletionContext {
    fn new() -> Self {
        Self {
            variables: Vec::new(),
            constants: Vec::new(),
            sets: Vec::new(),
            parameters: Vec::new(),
            locals: Vec::new(),
        }
    }

    fn from_component_with_refs(component: &Component, loader: Option<&ComponentLoader>) -> Self {
        let mut ctx = Self::new();
        ctx.add_component(component);

        match component {
            Component::Context(_) => {
                if let Some(loader) = loader {
                    let environment = ResolvedEnvironment::new(component, loader);
                    for inherited in environment.extended_contexts() {
                        ctx.add_component(inherited);
                    }
                }
            }
            Component::Machine(_) => {
                if let Some(loader) = loader {
                    let environment = ResolvedEnvironment::new(component, loader);
                    for inherited in environment.refined_machines() {
                        ctx.add_component(inherited);
                    }
                    for visible in environment.visible_contexts() {
                        ctx.add_component(visible);
                    }
                }
            }
        }

        ctx
    }

    fn add_component(&mut self, component: &Component) {
        match component {
            Component::Context(context) => {
                self.constants
                    .extend(context.constants.iter().map(|c| c.name.clone()));
                self.sets
                    .extend(context.sets.iter().map(|s| s.name().to_string()));
            }
            Component::Machine(machine) => {
                self.variables
                    .extend(machine.variables.iter().map(|v| v.name.clone()));
            }
        }
    }

    /// Augment the context with the symbols scoped to the cursor: the enclosing
    /// event's `ANY` parameters and the formula binders in scope at `offset`.
    /// `masked` must be the comment-masked form, and `offset` the byte offset, of
    /// the same source the `component` was parsed from, so its event line ranges
    /// and binder spans index one snapshot.
    fn add_local_scope(
        &mut self,
        component: &Component,
        masked: &str,
        position: Position,
        offset: usize,
    ) {
        if let Component::Machine(machine) = component
            && let Some(event) = text_utils::enclosing_event(machine, masked, position)
        {
            self.parameters
                .extend(event.parameters.iter().map(|p| p.name.clone()));
        }
        // Formula binders occur in both contexts and machines.
        self.locals
            .extend(crate::formula_walk::binders_in_scope_at_offset(
                component, offset,
            ));
    }
}

#[cfg(test)]
pub(crate) fn benchmark_environment_construction(
    component: &Component,
    loader: &ComponentLoader<'_>,
) -> usize {
    let context = CompletionContext::from_component_with_refs(component, Some(loader));
    std::hint::black_box(&context);
    context.variables.len() + context.constants.len() + context.sets.len()
}

/// Provides code completion for Event-B documents
pub struct CompletionProvider {
    /// Cross-reference manager for workspace-wide navigation
    cross_ref_manager: Option<Arc<CrossReferenceManager>>,
    /// Document manager — the source of the document's shared recovered parse
    document_manager: Option<Arc<DocumentManager>>,
}

impl CompletionProvider {
    pub fn new() -> Self {
        Self {
            cross_ref_manager: None,
            document_manager: None,
        }
    }

    /// Set the cross-reference manager for workspace-wide navigation
    pub fn set_cross_reference_manager(&mut self, manager: Arc<CrossReferenceManager>) {
        self.cross_ref_manager = Some(manager);
    }

    /// Set the document manager for accessing open documents
    pub fn set_document_manager(&mut self, manager: Arc<DocumentManager>) {
        self.document_manager = Some(manager);
    }

    /// Generate completion items for the given position
    pub fn complete(
        &self,
        params: &CompletionParams,
        text: &str,
        completion_config: &CompletionConfig,
        format_config: &FormatConfig,
    ) -> Option<CompletionResponse> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        if !completion_config.enabled {
            return None;
        }

        // No completions inside a comment — it's prose, not Event-B.
        let lexical = rossi::comments::lexical_spans(text);
        if let Some(offset) = position_to_offset(text, position)
            && rossi::comments::span_containing(&lexical.comments, offset).is_some()
        {
            return None;
        }
        // Structural context detection scans the comment-masked line, so an
        // `EVENT` mentioned in a trailing comment cannot change the scope.
        let masked = lexical.mask_comments_chars(text);
        let line_text = get_line_text(&masked, position);

        // Get completion context from the component under the cursor in the
        // document's shared parse (the single source of truth maintained by the
        // document manager), along with that component's own name (to exclude it
        // from component-name completion — a component never references itself).
        let parsed = self
            .document_manager
            .as_ref()
            .and_then(|dm| dm.parse_result(uri));
        // One loader per request: each visible context/machine in the SEES /
        // EXTENDS / REFINES walk is parsed at most once, reusing open documents'
        // stored parses.
        let loader = ComponentLoader::optional(
            self.cross_ref_manager.as_deref(),
            self.document_manager.as_deref(),
        );
        // Select the cursor's component against the stored parse's own text, so
        // the offset and the component spans index one snapshot — the handler
        // `text` is a separate copy a concurrent edit can desync from the parse.
        let (completion_ctx, self_name, keyword_scope, offer_status_values) = parsed
            .as_deref()
            .and_then(|parsed| {
                let offset =
                    position_to_offset(parsed.text(), position).unwrap_or(parsed.text().len());
                // Scope keywords, event `ANY` parameters, and formula binders
                // against the same snapshot the component was parsed from. When
                // it matches the handler text (the common case), reuse its mask.
                let reparsed_mask;
                let scope_masked = if parsed.text() == text {
                    masked.as_str()
                } else {
                    reparsed_mask = rossi::comments::mask_comments_chars(parsed.text());
                    &reparsed_mask
                };
                let scope_line = if parsed.text() == text {
                    line_text
                } else {
                    get_line_text(scope_masked, position)
                };
                let component = component_at_offset(parsed.components(), offset)?;
                let keyword_scope =
                    keyword_scope_at_offset(component, scope_masked, scope_line, offset);
                let offer_status_values = keyword_scope & keywords::scope::EVENT != 0
                    && status_value_trigger(scope_line, position.character as usize);
                let mut ctx =
                    CompletionContext::from_component_with_refs(component, loader.as_ref());
                ctx.add_local_scope(component, scope_masked, position, offset);
                Some((
                    ctx,
                    Some(rossi::deps::kind_and_name(component).1),
                    keyword_scope,
                    offer_status_values,
                ))
            })
            .unwrap_or((
                CompletionContext::new(),
                None,
                keywords::scope::TOP_LEVEL,
                false,
            ));

        // Determine what to complete based on context
        let mut items = Vec::new();

        // `position.character` is a UTF-16 column; `get_word_at_position` slices
        // by char, so convert first or an astral char before the cursor would
        // truncate the word.
        let char_col = utf16_to_char_col(line_text, position.character as usize);
        let word_at_cursor = get_word_at_position(line_text, char_col);

        // Add keyword completions
        items.extend(self.get_keyword_completions(keyword_scope, offer_status_values));

        // Add operator completions
        items.extend(self.get_operator_completions(format_config.use_unicode));

        // Add identifier completions
        items.extend(self.get_identifier_completions(&completion_ctx, &word_at_cursor));

        // Add component-name completions on REFINES/SEES/EXTENDS clauses, with a
        // hyphen-aware replace range so editors match/replace across `-` (#36).
        items.extend(self.get_component_name_completions(&masked, position, self_name.as_deref()));

        // Add snippet completions
        items.extend(self.get_snippet_completions(line_text, position));

        // Add built-in type completions
        items.extend(self.get_builtin_completions(&word_at_cursor));

        if items.is_empty() {
            None
        } else {
            Some(CompletionResponse::Array(items))
        }
    }

    /// Get keyword completions based on context
    fn get_keyword_completions(
        &self,
        keyword_scope: u8,
        offer_status_values: bool,
    ) -> Vec<CompletionItem> {
        use keywords::KeywordGroup;
        let mut items = Vec::new();

        push_keyword_items(&mut items, keywords::iter_completion_scope(keyword_scope));

        if offer_status_values {
            push_keyword_items(&mut items, keywords::iter_group(KeywordGroup::Status));
        }

        items
    }

    /// Get operator completions (Unicode or ASCII based on config)
    fn get_operator_completions(&self, use_unicode: bool) -> Vec<CompletionItem> {
        operators::OPERATOR_SPELLINGS
            .iter()
            .filter(|entry| entry.completion)
            .map(|entry| {
                let label = entry.emit_text(use_unicode);
                let alternative = entry.emit_text(!use_unicode);
                let alternative = if alternative == label {
                    ""
                } else {
                    alternative
                };
                create_operator_item(label, alternative, entry.description)
            })
            .collect()
    }

    /// Get identifier completions from the current context.
    ///
    /// The symbol classes are offered most-local first, and a name is offered
    /// only once: an in-scope binder or event parameter shadows a same-named
    /// global symbol, so it wins the single completion item for that name rather
    /// than the editor showing the name twice with conflicting kinds.
    fn get_identifier_completions(
        &self,
        ctx: &CompletionContext,
        _word: &str,
    ) -> Vec<CompletionItem> {
        // (names, kind, detail, documentation noun) — most local first.
        let groups: [(&[String], CompletionItemKind, &str, &str); 5] = [
            (
                &ctx.locals,
                CompletionItemKind::VARIABLE,
                "Bound variable",
                "Bound variable",
            ),
            (
                &ctx.parameters,
                CompletionItemKind::VARIABLE,
                "Parameter",
                "Event parameter",
            ),
            (
                &ctx.variables,
                CompletionItemKind::VARIABLE,
                "Variable",
                "State variable",
            ),
            (
                &ctx.constants,
                CompletionItemKind::CONSTANT,
                "Constant",
                "Constant",
            ),
            (&ctx.sets, CompletionItemKind::ENUM, "Set", "Carrier set"),
        ];

        let mut items = Vec::new();
        let mut seen = HashSet::new();
        for (names, kind, detail, noun) in groups {
            for name in names {
                if !seen.insert(name.clone()) {
                    continue;
                }
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(kind),
                    detail: Some(detail.to_string()),
                    documentation: Some(Documentation::String(format!("{noun} `{name}`"))),
                    ..Default::default()
                });
            }
        }

        items
    }

    /// Component-name completions for a REFINES/SEES/EXTENDS clause. The cheap
    /// clause check runs first, so the workspace component list is only queried
    /// when the cursor is actually in such a clause. Each item carries an
    /// explicit edit spanning the whole (possibly hyphenated) word under the
    /// cursor, so the editor filters and replaces across `-` rather than only
    /// the segment after the last hyphen (issue #36). `self_name` is the
    /// enclosing component, excluded so it can't reference itself.
    fn get_component_name_completions(
        &self,
        masked: &str,
        position: Position,
        self_name: Option<&str>,
    ) -> Vec<CompletionItem> {
        let Some(clause) = component_reference_clause(masked, position) else {
            return Vec::new();
        };
        let Some(crm) = self.cross_ref_manager.as_deref() else {
            return Vec::new();
        };
        // REFINES targets a machine; SEES/EXTENDS a context (the edge's target).
        let kind = clause.target_kind();

        let range = hyphenated_word_range(masked, position);
        crm.component_names_of_kind(kind)
            .into_iter()
            .filter(|name| Some(name.as_str()) != self_name)
            .map(|name| CompletionItem {
                label: name.clone(),
                kind: Some(CompletionItemKind::MODULE),
                detail: Some("Component".to_string()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range,
                    new_text: name,
                })),
                ..Default::default()
            })
            .collect()
    }

    /// Snippet completions, sourced from the canonical [`rossi::snippets`]
    /// table — the same table the editor snippet libraries are generated from.
    /// Serving them here means the LSP (the path Sublime Text uses, since it has
    /// no native snippet files) offers exactly the snippets every other editor
    /// ships, so the two can never drift.
    ///
    /// When the cursor follows a `\name` input leader (e.g. the user typed
    /// `\exists` and pressed Tab), the editor's own word boundary excludes the
    /// leading backslash, so a plain insert leaves it stranded in front of the
    /// expanded body (`\∃ …`, which then fails to parse — issue #78). In that
    /// case each item gets an explicit edit that replaces the whole `\name`
    /// span, plus a backslashed `filter_text` so the client still matches it.
    fn get_snippet_completions(&self, line: &str, position: Position) -> Vec<CompletionItem> {
        let leader = leader_token_range(line, position);
        rossi::snippets::SNIPPETS
            .iter()
            .map(|snippet| {
                let body = snippet.body.join("\n");
                // With a `\name` leader, replace the whole `\name` span via an
                // explicit edit (so the backslash is consumed, not stranded) and
                // filter on the backslashed prefix; otherwise insert at the
                // cursor and let the editor match on the label as usual.
                let (insert_text, text_edit, filter_text) = match leader {
                    Some(range) => (
                        None,
                        Some(CompletionTextEdit::Edit(TextEdit {
                            range,
                            new_text: body,
                        })),
                        Some(format!("\\{}", snippet.prefix)),
                    ),
                    None => (Some(body), None, None),
                };
                CompletionItem {
                    label: snippet.prefix.to_string(),
                    kind: Some(CompletionItemKind::SNIPPET),
                    detail: Some(snippet.name.to_string()),
                    documentation: Some(Documentation::String(snippet.description.to_string())),
                    insert_text,
                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                    text_edit,
                    filter_text,
                    ..Default::default()
                }
            })
            .collect()
    }

    /// Get built-in type and constant completions
    fn get_builtin_completions(&self, _word: &str) -> Vec<CompletionItem> {
        vec![
            create_builtin_item("BOOL", "Boolean type {TRUE, FALSE}"),
            create_builtin_item("TRUE", "Boolean true value"),
            create_builtin_item("FALSE", "Boolean false value"),
            create_builtin_item("ℕ", "Natural numbers (0, 1, 2, ...)"),
            create_builtin_item("NAT", "Natural numbers (ASCII)"),
            create_builtin_item("ℕ1", "Positive natural numbers (1, 2, 3, ...)"),
            create_builtin_item("NAT1", "Positive natural numbers (ASCII)"),
            create_builtin_item("ℤ", "Integers (..., -1, 0, 1, ...)"),
            create_builtin_item("INT", "Integers (ASCII)"),
        ]
    }
}

impl Default for CompletionProvider {
    fn default() -> Self {
        Self::new()
    }
}

// Helper functions

fn create_keyword_item(keyword: &str, description: &str) -> CompletionItem {
    CompletionItem {
        label: keyword.to_string(),
        kind: Some(CompletionItemKind::KEYWORD),
        detail: Some("Keyword".to_string()),
        documentation: Some(Documentation::String(description.to_string())),
        ..Default::default()
    }
}

/// Push a completion item for every spelling of each keyword (so alternates like
/// `WHEN`/`BEGIN` are offered alongside `WHERE`/`THEN`).
fn push_keyword_items<'a>(
    items: &mut Vec<CompletionItem>,
    iter: impl Iterator<Item = &'a keywords::Keyword>,
) {
    for kw in iter {
        for spelling in kw.spellings {
            items.push(create_keyword_item(spelling, kw.summary));
        }
    }
}

fn create_operator_item(operator: &str, alternative: &str, description: &str) -> CompletionItem {
    let detail = if alternative.is_empty() {
        "Operator".to_string()
    } else {
        format!("Operator (alternative: {})", alternative)
    };

    CompletionItem {
        label: operator.to_string(),
        kind: Some(CompletionItemKind::OPERATOR),
        detail: Some(detail),
        documentation: Some(Documentation::String(description.to_string())),
        ..Default::default()
    }
}

fn create_builtin_item(name: &str, description: &str) -> CompletionItem {
    CompletionItem {
        label: name.to_string(),
        kind: Some(CompletionItemKind::CONSTANT),
        detail: Some("Built-in".to_string()),
        documentation: Some(Documentation::String(description.to_string())),
        ..Default::default()
    }
}

fn get_line_text(text: &str, position: Position) -> &str {
    text.lines().nth(position.line as usize).unwrap_or("")
}

fn get_word_at_position(line: &str, char_pos: usize) -> String {
    // `char_pos` is a character column, not a byte offset — slice by chars so a
    // multi-byte operator (e.g. `∈`, `≤`) before the cursor can't land mid-char
    // and panic.
    let before_cursor: String = line.chars().take(char_pos).collect();
    before_cursor
        .split_whitespace()
        .last()
        .unwrap_or("")
        .to_string()
}

/// The range of the (possibly hyphenated) word ending at the cursor, used as a
/// completion edit range so a hyphenated component name is replaced whole, not
/// just its last `-` segment. Scans left over component-name characters
/// (`keywords::is_structural_word_char`: ASCII alphanumerics, `_`, `-`) — the
/// same charset the grammar's `component_name` accepts — so it never extends
/// across a character that can't be part of a name.
fn hyphenated_word_range(masked: &str, position: Position) -> Range {
    let line = masked.lines().nth(position.line as usize).unwrap_or("");
    let chars: Vec<char> = line.chars().collect();
    // The incoming `position.character` is a UTF-16 column; the scan below
    // indexes `chars` by char, and the returned range is emitted as UTF-16
    // columns — so convert in on the way down and back out on the way up.
    // `utf16_to_char_col` already clamps to the line's char count.
    let cursor = utf16_to_char_col(line, position.character as usize);
    let mut start = cursor;
    while start > 0 && keywords::is_structural_word_char(chars[start - 1]) {
        start -= 1;
    }
    // A component name can't start with `-`, so don't pull a leading hyphen
    // into the replace range.
    while start < cursor && chars[start] == '-' {
        start += 1;
    }
    line_run_to_range(line, position.line, start, cursor)
}

/// The range of a `\name` input-leader token ending at the cursor, if one is
/// present. Scans left over `[A-Za-z0-9_]` name characters; if a single `\`
/// sits immediately before that run (or right before the cursor, for a bare
/// `\`), returns the range from the backslash through the cursor — used as a
/// snippet completion's edit range so expanding `\exists` replaces the whole
/// `\exists`, not just the word after the backslash (issue #78). Returns `None`
/// when there is no leading backslash, leaving plain word typing to the
/// editor's default replace behaviour. Columns are UTF-16 in and out (LSP),
/// converted via `utf16_to_char_col` / `line_run_to_range`, as in
/// [`hyphenated_word_range`].
fn leader_token_range(line: &str, position: Position) -> Option<Range> {
    let chars: Vec<char> = line.chars().collect();
    let cursor = utf16_to_char_col(line, position.character as usize);
    let mut start = cursor;
    while start > 0 && (chars[start - 1].is_ascii_alphanumeric() || chars[start - 1] == '_') {
        start -= 1;
    }
    (start > 0 && chars[start - 1] == '\\')
        .then(|| line_run_to_range(line, position.line, start - 1, cursor))
}

/// Structural keyword scope at `offset` in `component`'s parse snapshot.
fn keyword_scope_at_offset(component: &Component, masked: &str, line: &str, offset: usize) -> u8 {
    // A structural END line is already closing its block; do not suggest a new
    // clause on top of it. The mask keeps END in comments from matching.
    if text_utils::line_keyword_is(line, keywords::KeywordId::End) {
        return 0;
    }

    let Some(component_span) = component.span() else {
        return keyword_scope_in_component(component, masked, offset);
    };
    if offset < component_span.start {
        return keywords::scope::TOP_LEVEL;
    }
    if cursor_in_span(component_span, offset) {
        return keyword_scope_in_component(component, masked, offset);
    }

    // Recovery line-tightens unfinished components, so trailing edit whitespace
    // sits just beyond their span. With no structural component END before the
    // cursor, retain the selected component's scope.
    keyword_scope_after_component_span(component, masked, offset)
}

fn keyword_scope_after_component_span(component: &Component, masked: &str, offset: usize) -> u8 {
    let (closed, incomplete_events) = fallback_boundaries_before_offset(component, masked, offset);
    if closed {
        return keywords::scope::TOP_LEVEL;
    }
    match component {
        Component::Context(_) => keywords::scope::CONTEXT,
        Component::Machine(machine) => {
            if incomplete_events
                || machine
                    .clauses
                    .iter()
                    .any(|clause| clause.keyword == keywords::KeywordId::Events)
            {
                keywords::scope::EVENTS
            } else {
                keywords::scope::MACHINE
            }
        }
    }
}

fn keyword_scope_in_component(component: &Component, masked: &str, offset: usize) -> u8 {
    let in_clause = component
        .clauses()
        .iter()
        .any(|clause| cursor_in_span(clause.span, offset));

    match component {
        Component::Context(_) => {
            if in_clause {
                0
            } else {
                keywords::scope::CONTEXT
            }
        }
        Component::Machine(machine) => {
            if let Some(initialisation) = machine
                .initialisation
                .as_ref()
                .filter(|event| event.span.is_some_and(|span| cursor_in_span(span, offset)))
            {
                return keyword_scope_in_initialisation(initialisation, masked, offset);
            }
            if let Some(event) = machine
                .events
                .iter()
                .find(|event| event.span.is_some_and(|span| cursor_in_span(span, offset)))
            {
                return keyword_scope_in_event(event, masked, offset);
            }

            // EVENTS is terminal in the machine grammar. Its line-tight span
            // ends at the last event, but whitespace before the machine END is
            // still the EVENTS body, so the clause start is the lasting bound.
            if machine
                .clauses
                .iter()
                .find(|clause| clause.keyword == keywords::KeywordId::Events)
                .is_some_and(|events| offset >= events.span.start)
            {
                return keywords::scope::EVENTS;
            }

            if in_clause {
                return 0;
            }

            if incomplete_events_scope(component, masked, offset) {
                keywords::scope::EVENTS
            } else {
                keywords::scope::MACHINE
            }
        }
    }
}

fn keyword_scope_in_initialisation(
    event: &rossi::InitialisationEvent,
    masked: &str,
    offset: usize,
) -> u8 {
    keyword_scope_around_actions(
        &event.actions,
        masked,
        offset,
        keywords::scope::INITIALISATION,
    )
}

fn keyword_scope_in_event(event: &rossi::Event, masked: &str, offset: usize) -> u8 {
    let in_member = event
        .parameters
        .iter()
        .filter_map(|parameter| parameter.span)
        .chain(event.guards.iter().filter_map(|guard| guard.span))
        .chain(event.with.iter().filter_map(|predicate| predicate.span))
        .chain(event.witnesses.iter().filter_map(|witness| witness.span))
        .any(|span| member_position(span, masked, offset) == MemberPosition::Inside);
    if in_member {
        return 0;
    }
    keyword_scope_around_actions(&event.actions, masked, offset, keywords::scope::EVENT)
}

fn keyword_scope_around_actions(
    actions: &[rossi::LabeledAction],
    masked: &str,
    offset: usize,
    before_actions: u8,
) -> u8 {
    let mut follows_action = false;
    for span in actions.iter().filter_map(|action| action.span) {
        match member_position(span, masked, offset) {
            MemberPosition::Before => {}
            MemberPosition::Inside => return 0,
            MemberPosition::After => follows_action = true,
        }
    }
    if follows_action {
        keywords::scope::EVENT_END
    } else {
        before_actions
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MemberPosition {
    Before,
    Inside,
    After,
}

/// Classify the cursor against a member's line-tight span. Trailing whitespace
/// on the final line remains inside the member, so completion immediately after
/// a declaration/formula/action cannot offer a structural clause mid-line.
fn member_position(span: rossi::ast::Span, masked: &str, offset: usize) -> MemberPosition {
    if offset < span.start {
        return MemberPosition::Before;
    }
    let end = masked
        .get(span.start..span.end)
        .map_or(span.end, |text| span.start + text.trim_end().len());
    if offset <= end
        || masked
            .get(end..offset)
            .is_some_and(|between| !between.contains('\n'))
    {
        MemberPosition::Inside
    } else {
        MemberPosition::After
    }
}

fn status_value_trigger(line: &str, utf16_col: usize) -> bool {
    let prefix_end = utf16_to_byte(line, utf16_col).unwrap_or(line.len());
    let prefix = &line[..prefix_end];
    if !text_utils::line_keyword_is(prefix, keywords::KeywordId::Status) {
        return false;
    }
    let mut words = prefix.split_whitespace();
    words.next();
    let Some(partial) = words.next() else {
        return true;
    };
    if words.next().is_some() {
        return false;
    }
    keywords::iter_group(keywords::KeywordGroup::Status).any(|keyword| {
        keyword.spellings.iter().any(|spelling| {
            spelling.len() >= partial.len()
                && spelling[..partial.len()].eq_ignore_ascii_case(partial)
        })
    })
}

/// Cursor containment treats the end of a line-tight structural span as part
/// of that construct: completion is commonly requested immediately after its
/// final token. The following newline remains outside it.
fn cursor_in_span(span: rossi::ast::Span, offset: usize) -> bool {
    span.start <= offset && offset <= span.end
}

/// Whether a selected machine has a bare, still-open EVENTS header before the
/// cursor but no parsed EVENTS region. This is the only textual scope fallback.
/// `masked` is the parse snapshot's comment-masked text, so comments and
/// keyword-shaped identifiers inside parsed clauses cannot open the section.
fn incomplete_events_scope(component: &Component, masked: &str, offset: usize) -> bool {
    let Component::Machine(machine) = component else {
        return false;
    };
    if machine
        .clauses
        .iter()
        .any(|clause| clause.keyword == keywords::KeywordId::Events)
    {
        return false;
    }

    fallback_boundaries_before_offset(component, masked, offset).1
}

/// Structural component boundaries before `offset`: whether the component has
/// closed, and whether an unparsed EVENTS header remains open. Keyword-shaped
/// identifiers inside parsed clauses do not count as boundaries.
fn fallback_boundaries_before_offset(
    component: &Component,
    masked: &str,
    offset: usize,
) -> (bool, bool) {
    let Some(start) = component.span().map(|span| span.start) else {
        return (false, false);
    };
    let Some(prefix) = masked.get(start..offset.min(masked.len())) else {
        return (false, false);
    };
    let mut incomplete_events = false;
    for (line_start, line) in lines_rev_with_offsets(prefix) {
        let line_start = start + line_start;
        if structural_line_keyword(component, line, line_start, keywords::KeywordId::End) {
            return (true, false);
        }
        if structural_line_keyword(component, line, line_start, keywords::KeywordId::Events) {
            incomplete_events = true;
        }
    }
    (false, incomplete_events)
}

fn structural_line_keyword(
    component: &Component,
    line: &str,
    line_start: usize,
    keyword: keywords::KeywordId,
) -> bool {
    if !text_utils::line_keyword_is(line, keyword) {
        return false;
    }
    let keyword_offset = line_start + line.len() - line.trim_start().len();
    !component
        .clauses()
        .iter()
        .any(|clause| cursor_in_span(clause.span, keyword_offset))
}

fn lines_rev_with_offsets(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut end = text.len();
    let mut done = false;
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        let start = text[..end].rfind('\n').map_or(0, |index| index + 1);
        let line = &text[start..end];
        if start == 0 {
            done = true;
        } else {
            end = start - 1;
        }
        Some((start, line))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keyword_completions() {
        let provider = CompletionProvider::new();
        let items = provider.get_keyword_completions(keywords::scope::TOP_LEVEL, false);

        // Should include top-level keywords
        assert!(items.iter().any(|item| item.label == "CONTEXT"));
        assert!(items.iter().any(|item| item.label == "MACHINE"));
    }

    #[test]
    fn test_no_completions_inside_comment() {
        let provider = CompletionProvider::new();
        let text = "MACHINE m // type EVENT here\nEND\n";
        let params = CompletionParams {
            text_document_position: crate::lsp_types::TextDocumentPositionParams {
                text_document: crate::lsp_types::TextDocumentIdentifier {
                    uri: crate::lsp_types::Url::parse("file:///test.eventb").unwrap(),
                },
                position: Position::new(0, 20), // inside the comment
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: None,
        };

        assert!(
            provider
                .complete(
                    &params,
                    text,
                    &CompletionConfig::default(),
                    &FormatConfig::default(),
                )
                .is_none()
        );
    }

    #[test]
    fn test_operator_completions_unicode() {
        let provider = CompletionProvider::new();
        let items = provider.get_operator_completions(true);

        // Should include Unicode operators
        assert!(items.iter().any(|item| item.label == "∧"));
        assert!(items.iter().any(|item| item.label == "∨"));
        assert!(items.iter().any(|item| item.label == "⇒"));
        assert!(items.iter().any(|item| item.label == "∈"));
        assert!(items.iter().any(|item| item.label == "⊈"));
        // The private-use operators have no portable glyph, so even in Unicode
        // mode their completion inserts the ASCII spelling, never a tofu glyph.
        assert!(items.iter().any(|item| item.label == "<<->"));
        assert!(items.iter().any(|item| item.label == "<+"));
        assert!(
            !items
                .iter()
                .any(|item| operators::is_private_use_glyph(&item.label)),
            "no operator completion should insert a private-use glyph"
        );
        assert!(items.iter().any(|item| item.label == "‥"));
        assert!(items.iter().any(|item| item.label == "−"));
        assert!(items.iter().any(|item| item.label == ":∣"));
        assert!(items.iter().any(|item| item.label == "ℙ"));
        assert!(!items.iter().any(|item| item.label == "℘"));
    }

    #[test]
    fn test_operator_completions_ascii() {
        let provider = CompletionProvider::new();
        let items = provider.get_operator_completions(false);

        // Should include ASCII operators
        assert!(items.iter().any(|item| item.label == "&"));
        assert!(items.iter().any(|item| item.label == "or"));
        assert!(items.iter().any(|item| item.label == "=>"));
        assert!(items.iter().any(|item| item.label == ":"));
        assert!(items.iter().any(|item| item.label == "::"));
    }

    #[test]
    fn test_identifier_completions() {
        let provider = CompletionProvider::new();
        let ctx = CompletionContext {
            variables: vec!["count".to_string(), "total".to_string()],
            constants: vec!["max_value".to_string()],
            sets: vec!["STATUS".to_string()],
            parameters: vec!["x".to_string()],
            locals: vec!["bound".to_string()],
        };

        let items = provider.get_identifier_completions(&ctx, "");

        assert!(items.iter().any(|item| item.label == "count"));
        assert!(items.iter().any(|item| item.label == "total"));
        assert!(items.iter().any(|item| item.label == "max_value"));
        assert!(items.iter().any(|item| item.label == "STATUS"));
        assert!(items.iter().any(|item| item.label == "x"));
        assert!(
            items
                .iter()
                .any(|item| item.label == "bound"
                    && item.detail.as_deref() == Some("Bound variable"))
        );
    }

    #[test]
    fn identifier_completions_offer_a_shadowed_name_once_most_local_wins() {
        let provider = CompletionProvider::new();
        // `x` is both a state variable and an in-scope binder; the binder shadows
        // it, so `x` is offered once, as the bound variable.
        let ctx = CompletionContext {
            variables: vec!["x".to_string()],
            constants: Vec::new(),
            sets: Vec::new(),
            parameters: Vec::new(),
            locals: vec!["x".to_string()],
        };

        let items = provider.get_identifier_completions(&ctx, "");
        let xs: Vec<_> = items.iter().filter(|item| item.label == "x").collect();
        assert_eq!(xs.len(), 1, "a shadowed name is offered once, got {xs:?}");
        assert_eq!(xs[0].detail.as_deref(), Some("Bound variable"));
    }

    #[test]
    fn test_builtin_completions() {
        let provider = CompletionProvider::new();
        let items = provider.get_builtin_completions("");

        assert!(items.iter().any(|item| item.label == "BOOL"));
        assert!(items.iter().any(|item| item.label == "TRUE"));
        assert!(items.iter().any(|item| item.label == "FALSE"));
        assert!(items.iter().any(|item| item.label == "ℕ"));
        assert!(items.iter().any(|item| item.label == "NAT"));
        assert!(items.iter().any(|item| item.label == "ℤ"));
        assert!(items.iter().any(|item| item.label == "INT"));
    }

    #[test]
    fn test_snippet_completions() {
        let provider = CompletionProvider::new();
        // No `\` leader, so items carry a plain insert_text and no text_edit.
        let items = provider.get_snippet_completions("", Position::new(0, 0));

        // Every snippet comes from the canonical table — one item per entry.
        assert_eq!(items.len(), rossi::snippets::SNIPPETS.len());
        assert!(items.iter().any(|item| item.label == "evt"));
        assert!(items.iter().any(|item| item.label == "forall"));
        assert!(items.iter().any(|item| item.label == "exists"));
        // Every item is a snippet carrying its body, with no edit range when
        // there is no leader to consume.
        assert!(items.iter().all(|item| {
            item.kind == Some(CompletionItemKind::SNIPPET)
                && item.insert_text_format == Some(InsertTextFormat::SNIPPET)
                && item.insert_text.is_some()
                && item.text_edit.is_none()
        }));
        // The old ad-hoc labels are gone now that the table is the source.
        assert!(!items.iter().any(|item| item.label == "event"));
        assert!(!items.iter().any(|item| item.label == "labeled_predicate"));
    }

    #[test]
    fn leader_token_range_spans_backslash_and_word() {
        // `\exists`, cursor at the end (UTF-16 col 7) → the whole `\exists`.
        let range = leader_token_range("\\exists", Position::new(0, 7))
            .expect("a `\\name` leader must be detected");
        assert_eq!(range.start, Position::new(0, 0));
        assert_eq!(range.end, Position::new(0, 7));

        // A bare `\` (col 1) still counts — replacing it consumes the leader.
        let bare = leader_token_range("\\", Position::new(0, 1))
            .expect("a bare backslash is a leader too");
        assert_eq!(bare.start, Position::new(0, 0));
        assert_eq!(bare.end, Position::new(0, 1));

        // A plain word (no backslash) is not a leader — let the editor decide.
        assert!(leader_token_range("exists", Position::new(0, 6)).is_none());
    }

    #[test]
    fn snippet_completion_consumes_leader_backslash() {
        let provider = CompletionProvider::new();
        // The user typed `\exists` and triggered completion.
        let items = provider.get_snippet_completions("\\exists", Position::new(0, 7));
        // With a leader every item (single- and multi-line bodies alike) carries
        // the edit, with no leftover insert_text a client might prefer over it.
        assert!(
            items
                .iter()
                .all(|i| i.text_edit.is_some() && i.insert_text.is_none())
        );
        let item = items
            .iter()
            .find(|i| i.label == "exists")
            .expect("the exists snippet must be offered");

        // The edit must replace the whole `\exists`, backslash included, so the
        // expanded body stands alone rather than `\∃ …` (issue #78).
        let body = rossi::snippets::SNIPPETS
            .iter()
            .find(|s| s.prefix == "exists")
            .unwrap()
            .body
            .join("\n");
        match item.text_edit.as_ref().expect("leader needs a text_edit") {
            CompletionTextEdit::Edit(edit) => {
                assert_eq!(edit.range.start, Position::new(0, 0));
                assert_eq!(edit.range.end, Position::new(0, 7));
                assert_eq!(edit.new_text, body);
            }
            other => panic!("expected a plain TextEdit, got {other:?}"),
        }
        // Filter on the backslashed form so the client still surfaces the item,
        // and keep snippet semantics for the inserted tabstops.
        assert_eq!(item.filter_text.as_deref(), Some("\\exists"));
        assert_eq!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));
    }

    #[test]
    fn test_completion_refined_variables() {
        use crate::lsp_types::Url;

        let abstract_source = "MACHINE abstract_mch\nVARIABLES\n    abstract_state\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        abstract_state := 0\n    END\nEND";
        let concrete_source = "MACHINE concrete_mch\nREFINES\n    abstract_mch\nVARIABLES\n    concrete_state\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        concrete_state := 0\n    END\nEND";

        let crm = Arc::new(CrossReferenceManager::new());
        let dm = Arc::new(DocumentManager::new());

        crm.update_component("file:///abstract_mch.eventb".to_string(), abstract_source);
        let url = Url::parse("file:///abstract_mch.eventb").unwrap();
        dm.open(url, 1, abstract_source.to_string());

        crm.update_component("file:///concrete_mch.eventb".to_string(), concrete_source);
        let concrete_url = Url::parse("file:///concrete_mch.eventb").unwrap();
        dm.open(concrete_url.clone(), 1, concrete_source.to_string());

        let mut provider = CompletionProvider::new();
        provider.set_cross_reference_manager(Arc::clone(&crm));
        provider.set_document_manager(Arc::clone(&dm));

        // Build completion context from the component in the shared parse — the
        // same source `complete` reads from.
        let parsed = dm.parse_result(&concrete_url).unwrap();
        let components = parsed.parse().component.as_deref().unwrap();
        let loader = ComponentLoader::optional(
            provider.cross_ref_manager.as_deref(),
            provider.document_manager.as_deref(),
        );
        let ctx = CompletionContext::from_component_with_refs(&components[0], loader.as_ref());

        // Should include abstract_state from refined machine
        assert!(
            ctx.variables.contains(&"abstract_state".to_string()),
            "abstract_state should appear in completions, got: {:?}",
            ctx.variables
        );
        // Should also include local concrete_state
        assert!(
            ctx.variables.contains(&"concrete_state".to_string()),
            "concrete_state should appear in completions"
        );
    }

    #[test]
    fn completion_includes_symbols_beyond_ten_seen_contexts() {
        use crate::lsp_types::Url;

        let crm = Arc::new(CrossReferenceManager::new());
        let dm = Arc::new(DocumentManager::new());
        for i in 0..=10 {
            let uri = Url::parse(&format!("file:///c{i}.eventb")).unwrap();
            let source = format!("CONTEXT c{i}\nCONSTANTS\n    k{i}\nEND");
            crm.update_component(uri.to_string(), &source);
            dm.open(uri, 1, source);
        }

        let machine = format!(
            "MACHINE m\nSEES\n{}\nINVARIANTS\n    @inv1 k10 = k10\nEND",
            (0..=10)
                .map(|i| format!("    c{i}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let uri = Url::parse("file:///m.eventb").unwrap();
        crm.update_component(uri.to_string(), &machine);
        dm.open(uri.clone(), 1, machine.clone());

        let mut provider = CompletionProvider::new();
        provider.set_cross_reference_manager(crm);
        provider.set_document_manager(dm);
        let position = crate::position::offset_to_position(
            &machine,
            machine.find("k10 =").expect("target use") + 2,
        );
        let params = CompletionParams {
            text_document_position: crate::lsp_types::TextDocumentPositionParams {
                text_document: crate::lsp_types::TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: None,
        };

        let Some(CompletionResponse::Array(items)) = provider.complete(
            &params,
            &machine,
            &CompletionConfig::default(),
            &FormatConfig::default(),
        ) else {
            panic!("expected completion items");
        };
        assert!(
            items.iter().any(|item| item.label == "k10"),
            "the eleventh seen context must remain visible"
        );
    }

    /// Run the full completion pipeline against a single open document and return
    /// the `(label, detail)` of every produced item — the same path an editor
    /// drives, so the scope wiring (not just the helpers) is exercised.
    fn complete_labels(source: &str, position: Position) -> Vec<(String, Option<String>)> {
        use crate::lsp_types::Url;

        let crm = Arc::new(CrossReferenceManager::new());
        let dm = Arc::new(DocumentManager::new());
        let url = Url::parse("file:///m.eventb").unwrap();
        crm.update_component(url.to_string(), source);
        dm.open(url.clone(), 1, source.to_string());

        let mut provider = CompletionProvider::new();
        provider.set_cross_reference_manager(crm);
        provider.set_document_manager(dm);

        let params = CompletionParams {
            text_document_position: crate::lsp_types::TextDocumentPositionParams {
                text_document: crate::lsp_types::TextDocumentIdentifier { uri: url },
                position,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: None,
        };

        match provider.complete(
            &params,
            source,
            &CompletionConfig::default(),
            &FormatConfig::default(),
        ) {
            Some(CompletionResponse::Array(items)) => {
                items.into_iter().map(|i| (i.label, i.detail)).collect()
            }
            _ => Vec::new(),
        }
    }

    /// Run completion at the `|` marker after removing it from the source, so
    /// scope tests stay readable while the parser sees only Event-B text.
    fn complete_labels_at_marker(source: &str) -> Vec<(String, Option<String>)> {
        let marker = source
            .find('|')
            .expect("source must contain a cursor marker");
        assert_eq!(
            source[marker + 1..].find('|'),
            None,
            "source must contain exactly one cursor marker"
        );
        let mut source = source.to_string();
        source.remove(marker);
        let position = crate::position::offset_to_position(&source, marker);
        complete_labels(&source, position)
    }

    fn has_label(labels: &[(String, Option<String>)], expected: &str) -> bool {
        labels.iter().any(|(label, _)| label == expected)
    }

    fn assert_keyword_scope(source: &str, present: &[&str], absent: &[&str]) {
        let labels = complete_labels_at_marker(source);
        for label in present {
            assert!(has_label(&labels, label), "missing {label}; got {labels:?}");
        }
        for label in absent {
            assert!(
                !has_label(&labels, label),
                "unexpected {label}; got {labels:?}"
            );
        }
    }

    #[test]
    fn keyword_completion_follows_structural_scope() {
        assert_keyword_scope(
            "CONTEXT c\n|\nEND",
            &["SETS", "AXIOMS"],
            &["MACHINE", "VARIABLES", "EVENTS", "EVENT", "ANY"],
        );
        assert_keyword_scope(
            "MACHINE m\n|\nEND",
            &["VARIABLES", "EVENTS"],
            &["CONTEXT", "SETS", "AXIOMS", "EVENT", "ANY"],
        );
        assert_keyword_scope(
            "MACHINE m\nVARIABLES\n    x|\nINVARIANTS\n    @i x = x\nEND",
            &[],
            &["VARIABLES", "INVARIANTS", "EVENTS", "SETS", "EVENT"],
        );
        assert_keyword_scope(
            "MACHINE m\nVARIABLES\n    x\n|\nEND",
            &["INVARIANTS", "EVENTS"],
            &["SETS", "EVENT", "ANY"],
        );
        assert_keyword_scope(
            "MACHINE m\nEVENTS\n    EVENT e\n    END\n    |\nEND",
            &["EVENT", "INITIALISATION"],
            &["MACHINE", "VARIABLES", "EVENTS", "ANY", "THEN"],
        );
        assert_keyword_scope(
            "MACHINE m\nEVENTS\n    EVENT e\n        |\n    END\nEND",
            &["ANY", "WHERE", "THEN"],
            &["CONTEXT", "VARIABLES", "EVENTS", "EVENT", "INITIALISATION"],
        );
        assert_keyword_scope(
            "CONTEXT c\nEND\n|\nMACHINE m\nEND",
            &["CONTEXT", "MACHINE"],
            &["SETS", "VARIABLES", "EVENTS", "EVENT", "ANY"],
        );
    }

    #[test]
    fn keyword_completion_suppresses_event_clauses_inside_actions() {
        assert_keyword_scope(
            "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT e\n    THEN\n        @a x := x|\n    END\nEND",
            &[],
            &["STATUS", "ANY", "WHERE", "WITH", "WITNESS", "THEN", "END"],
        );
    }

    #[test]
    fn initialisation_has_its_own_keyword_scope() {
        assert_keyword_scope(
            "MACHINE m\nEVENTS\n    EVENT INITIALISATION\n        |\n    END\nEND",
            &["EXTENDS", "THEN", "END"],
            &["STATUS", "REFINES", "ANY", "WHERE", "WITH", "WITNESS"],
        );
    }

    #[test]
    fn only_end_is_offered_after_event_actions() {
        assert_keyword_scope(
            "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT e\n    THEN\n        @a x := x\n        |\n    END\nEND",
            &["END"],
            &[
                "STATUS", "REFINES", "ANY", "WHERE", "WITH", "WITNESS", "THEN",
            ],
        );
    }

    #[test]
    fn unfinished_component_keeps_its_structural_scope() {
        assert_keyword_scope(
            "MACHINE m\n|",
            &["VARIABLES", "EVENTS"],
            &["CONTEXT", "MACHINE", "SETS", "AXIOMS", "EVENT"],
        );
        assert_keyword_scope(
            "CONTEXT c\n|",
            &["SETS", "AXIOMS"],
            &["CONTEXT", "MACHINE", "VARIABLES", "EVENTS", "EVENT"],
        );
    }

    #[test]
    fn incomplete_events_header_uses_the_narrow_scope_fallback() {
        assert_keyword_scope(
            "MACHINE m\nEVENTS\n    |",
            &["EVENT", "INITIALISATION"],
            &["CONTEXT", "VARIABLES", "EVENTS", "ANY", "THEN"],
        );
        assert_keyword_scope(
            "MACHINE m\nINVARIANTS\n    @i 1 = 1\n// EVENTS\n|\nEND",
            &["EVENTS"],
            &["EVENT", "INITIALISATION"],
        );
        assert_keyword_scope(
            "MACHINE m\nEVENTS\nEND\n|",
            &["CONTEXT", "MACHINE"],
            &["EVENT", "INITIALISATION"],
        );
    }

    #[test]
    fn events_identifier_does_not_open_the_events_section() {
        assert_keyword_scope(
            "MACHINE m\nVARIABLES\n    EVENTS\n    |\nEND",
            &["INVARIANTS", "EVENTS"],
            &["EVENT", "INITIALISATION"],
        );
    }

    #[test]
    fn status_values_require_a_status_clause_line() {
        let status =
            complete_labels_at_marker("MACHINE m\nEVENTS\n    EVENT e\n    STATUS |\n    END\nEND");
        for value in ["ordinary", "convergent", "anticipated"] {
            assert!(has_label(&status, value), "missing {value}; got {status:?}");
        }

        let action = complete_labels_at_marker(
            "MACHINE m\nEVENTS\n    EVENT e\n    THEN\n        @a STATUS| := STATUS\n    END\nEND",
        );
        for value in ["ordinary", "convergent", "anticipated"] {
            assert!(
                !has_label(&action, value),
                "STATUS as an identifier must not offer {value}; got {action:?}"
            );
        }

        let parameter = complete_labels_at_marker(
            "MACHINE m\nEVENTS\n    EVENT e\n    ANY\n        STATUS |\n    END\nEND",
        );
        let before_status = complete_labels_at_marker(
            "MACHINE m\nEVENTS\n    EVENT e\n        |STATUS ordinary\n    END\nEND",
        );
        let unlabelled_action = complete_labels_at_marker(
            "MACHINE m\nEVENTS\n    EVENT e\n    THEN\n        STATUS := |\n    END\nEND",
        );
        for labels in [&parameter, &before_status, &unlabelled_action] {
            for value in ["ordinary", "convergent", "anticipated"] {
                assert!(
                    !has_label(labels, value),
                    "non-clause STATUS must not offer {value}; got {labels:?}"
                );
            }
        }
    }

    #[test]
    fn completion_offers_the_enclosing_event_parameters() {
        // Cursor on the action line (index 10), inside event `e` whose ANY
        // clause declares `amount` (issue #102).
        let source = "MACHINE m\nVARIABLES\n    v\nEVENTS\n  EVENT e\n  ANY\n    amount\n  WHERE\n    @grd1 amount > 0\n  THEN\n    @act1 v := 0\n  END\nEND";
        let labels = complete_labels(source, Position::new(10, 8));

        assert!(
            labels
                .iter()
                .any(|(label, detail)| label == "amount" && detail.as_deref() == Some("Parameter")),
            "the event's ANY parameter `amount` must be offered, got {labels:?}"
        );
    }

    #[test]
    fn completion_does_not_offer_a_sibling_events_parameters() {
        // Two events; the cursor sits in `e1` (action line, index 8). Only `e1`'s
        // parameter is in scope — `e2`'s `p2` must not be offered.
        let source = "MACHINE m\nVARIABLES\n    v\nEVENTS\n  EVENT e1\n  ANY\n    p1\n  THEN\n    @act1 v := 0\n  END\n  EVENT e2\n  ANY\n    p2\n  THEN\n    @act2 v := 1\n  END\nEND";
        let labels = complete_labels(source, Position::new(8, 8));

        assert!(
            labels.iter().any(|(label, _)| label == "p1"),
            "the enclosing event's parameter `p1` must be offered, got {labels:?}"
        );
        assert!(
            !labels.iter().any(|(label, _)| label == "p2"),
            "a sibling event's parameter `p2` must not be offered, got {labels:?}"
        );
    }

    // The invariant `@i1 ∀ k · k > 0` is on line index 4; `k` is bound over the
    // body `k > 0`. The event action `@act1 …` on line index 8 is outside it.
    const WITH_QUANTIFIER: &str = "MACHINE m\nVARIABLES\n    v\nINVARIANTS\n    @i1 ∀ k · k > 0\nEVENTS\n    EVENT e\n    THEN\n        @act1 v := 0\n    END\nEND";

    #[test]
    fn completion_offers_in_scope_formula_binders() {
        // Cursor inside the quantifier body `k > 0`, just past the bound use `k`.
        let labels = complete_labels(WITH_QUANTIFIER, Position::new(4, 15));
        assert!(
            labels
                .iter()
                .any(|(label, detail)| label == "k" && detail.as_deref() == Some("Bound variable")),
            "the in-scope binder `k` must be offered, got {labels:?}"
        );
    }

    #[test]
    fn completion_omits_binders_outside_their_body() {
        // Cursor in the event action, outside the quantifier body — `k` is gone.
        let labels = complete_labels(WITH_QUANTIFIER, Position::new(8, 16));
        assert!(
            !labels.iter().any(|(label, _)| label == "k"),
            "a binder must not be offered outside its body, got {labels:?}"
        );
    }

    /// Build a provider whose workspace holds an abstract machine, a context,
    /// and the current machine `concrete_mch`.
    fn provider_with_workspace() -> CompletionProvider {
        let abstract_source = "MACHINE abstract_mch\nVARIABLES\n    s\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        s := 0\n    END\nEND";
        let ctx_source = "CONTEXT ctx0\nCONSTANTS\n    c\nAXIOMS\n    @a1 c = 0\nEND";
        let concrete_source = "MACHINE concrete_mch\nREFINES\n    abstract_mch\nVARIABLES\n    t\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        t := 0\n    END\nEND";

        let crm = Arc::new(CrossReferenceManager::new());
        crm.update_component("file:///abstract_mch.eventb".to_string(), abstract_source);
        crm.update_component("file:///ctx0.eventb".to_string(), ctx_source);
        crm.update_component("file:///concrete_mch.eventb".to_string(), concrete_source);

        let mut provider = CompletionProvider::new();
        provider.set_cross_reference_manager(crm);
        provider
    }

    #[test]
    fn test_component_names_filtered_by_kind_excluding_self() {
        let provider = provider_with_workspace();
        // A REFINES clause in concrete_mch: offer abstract machines only,
        // exclude concrete_mch itself, never offer a context.
        let masked = "MACHINE concrete_mch\nREFINES\n    \nEND\n";
        let labels: Vec<String> = provider
            .get_component_name_completions(masked, Position::new(2, 4), Some("concrete_mch"))
            .into_iter()
            .map(|i| i.label)
            .collect();

        assert!(
            labels.contains(&"abstract_mch".to_string()),
            "REFINES should offer the abstract machine, got {labels:?}"
        );
        assert!(
            !labels.contains(&"concrete_mch".to_string()),
            "the current component must be excluded, got {labels:?}"
        );
        assert!(
            !labels.contains(&"ctx0".to_string()),
            "REFINES must not offer a context, got {labels:?}"
        );
    }

    #[test]
    fn test_component_name_completion_spans_hyphenated_word() {
        let crm = Arc::new(CrossReferenceManager::new());
        crm.update_component(
            "file:///abstract-mch.eventb".to_string(),
            "MACHINE abstract-mch\nVARIABLES\n    s\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        s := 0\n    END\nEND",
        );
        let mut provider = CompletionProvider::new();
        provider.set_cross_reference_manager(crm);

        // REFINES target on its own indented line; cursor after `abstract-`.
        let masked = "MACHINE concrete\nREFINES\n    abstract-\nEND\n";
        let items =
            provider.get_component_name_completions(masked, Position::new(2, 13), Some("concrete"));
        let item = items
            .iter()
            .find(|i| i.label == "abstract-mch")
            .expect("hyphenated machine name must be offered in a REFINES clause");
        assert_eq!(item.kind, Some(CompletionItemKind::MODULE));

        // The edit must replace the whole hyphenated prefix `abstract-`, so the
        // editor matches across `-` rather than only the empty last segment.
        match item
            .text_edit
            .as_ref()
            .expect("component item needs a text_edit")
        {
            CompletionTextEdit::Edit(edit) => {
                assert_eq!(edit.range.start, Position::new(2, 4));
                assert_eq!(edit.range.end, Position::new(2, 13));
                assert_eq!(edit.new_text, "abstract-mch");
            }
            other => panic!("expected a plain TextEdit, got {other:?}"),
        }
    }

    #[test]
    fn hyphenated_word_range_is_utf16_after_astral() {
        // An astral `𝔹` (U+1D539 — two UTF-16 code units, one `char`) before the
        // word means the incoming UTF-16 cursor column and the emitted edit
        // range must both account for the surrogate pair, not the single char it
        // spans. LSP columns are UTF-16.
        let masked = "    𝔹 abstract-";
        // Cursor just past the trailing `-`: UTF-16 column 16
        // (4 spaces + 𝔹(2) + 1 space + "abstract-"(9)).
        let range = hyphenated_word_range(masked, Position::new(0, 16));
        // The replaced `abstract-` starts at the `a` (UTF-16 col 7), ends at 16.
        assert_eq!(range.start, Position::new(0, 7));
        assert_eq!(range.end, Position::new(0, 16));
    }

    #[test]
    fn test_component_names_not_offered_outside_reference_clause() {
        let provider = provider_with_workspace();
        // VARIABLES clause is not a component-reference position.
        let masked = "MACHINE m\nVARIABLES\n    x\nEND\n";
        let items = provider.get_component_name_completions(masked, Position::new(2, 5), Some("m"));
        assert!(
            items.is_empty(),
            "component names must not be offered outside REFINES/SEES/EXTENDS, got {items:?}"
        );
    }

    #[test]
    fn dependency_completion_keeps_component_and_general_suggestions() {
        let labels = complete_labels_at_marker(
            "CONTEXT C\nEND\n\nMACHINE M\nSEES C|\nVARIABLES\n    x\nEND",
        );

        assert!(
            labels
                .iter()
                .any(|(label, detail)| label == "C" && detail.as_deref() == Some("Component")),
            "the compatible component must be offered, got {labels:?}"
        );
        assert!(
            labels
                .iter()
                .any(|(label, detail)| label == "x" && detail.as_deref() == Some("Variable")),
            "existing general suggestions must remain, got {labels:?}"
        );
    }
}
