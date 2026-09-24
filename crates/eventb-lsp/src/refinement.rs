//! Refactorings along the refinement chain.
//!
//! The counterparts of Rodin's Refine wizard (`RefineMachine`, and
//! `ExtendContext` for a context): a refinement of the machine, or an
//! extension of the context, at the cursor, written to a new file next to it;
//! and the refinement of the abstract events a machine leaves unrefined.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rossi::{Component, Context, Event, EventStatus, InitialisationEvent, Machine, NamedElement};

use crate::code_actions::{
    event_clause_indent, event_header_end, event_is_lowercase, keyword_text, kind_requested,
    line_start, own_lines, parameter_insert,
};
use crate::component_loader::ComponentLoader;
use crate::cross_references::CrossReferenceManager;
use crate::document::DocumentManager;
use crate::lsp_types::{
    CodeAction, CodeActionKind, CodeActionParams, CreateFile, CreateFileOptions,
    DocumentChangeOperation, DocumentChanges, OneOf, OptionalVersionedTextDocumentIdentifier,
    Position, Range, ResourceOp, TextDocumentEdit, TextEdit, Uri, WorkspaceEdit,
};
use crate::text_utils::line_keyword;
use rossi::keywords::KeywordId;

/// Provides the refactors that create or rewrite components along the
/// refinement chain.
pub struct RefinementActionProvider {
    /// The workspace's components, so a new component takes a free name and
    /// an abstraction can be found.
    cross_ref_manager: Arc<CrossReferenceManager>,
    /// The open documents, read before the saved files for an abstraction.
    document_manager: Arc<DocumentManager>,
}

impl RefinementActionProvider {
    pub fn new(
        cross_ref_manager: Arc<CrossReferenceManager>,
        document_manager: Arc<DocumentManager>,
    ) -> Self {
        Self {
            cross_ref_manager,
            document_manager,
        }
    }

    /// The machine `name`, as the open documents or the workspace have it.
    fn load_machine(&self, name: &str) -> Option<Machine> {
        let loader = ComponentLoader::new(&self.cross_ref_manager, Some(&self.document_manager));
        match loader.load(name)?.component() {
            Component::Machine(machine) => Some(machine.clone()),
            Component::Context(_) => None,
        }
    }

    /// The refactors for the component at the cursor. `printer` writes new
    /// components in the configured style; a refactor creating a file is
    /// offered only when the client `creates_files`.
    pub fn provide(
        &self,
        params: &CodeActionParams,
        text: &str,
        printer: &rossi::PrettyPrinter,
        creates_files: bool,
    ) -> Vec<CodeAction> {
        let refactor = kind_requested(params, &CodeActionKind::REFACTOR);
        let inline = kind_requested(params, &CodeActionKind::REFACTOR_INLINE);
        if !refactor && !inline {
            return Vec::new();
        }
        let Some(cursor) = crate::position::position_to_offset(text, params.range.start) else {
            return Vec::new();
        };
        let components = crate::component_util::parse_all(text);
        let Some(component) = crate::component_util::component_at_offset(&components, cursor)
        else {
            return Vec::new();
        };
        let cursor_line = line_start(text, cursor)
            ..text[cursor..]
                .find('\n')
                .map_or(text.len(), |at| cursor + at);
        let on_line = |offset: usize| cursor_line.contains(&offset);
        let on_header = component
            .name_span()
            .is_some_and(|name| on_line(name.start));
        let mut actions = Vec::new();
        if refactor && on_header && creates_files {
            actions.extend(self.create_next_level_action(
                &params.text_document.uri,
                component,
                printer,
            ));
        }
        if let Component::Machine(machine) = component
            && let Some(event) = machine
                .events
                .iter()
                .find(|event| event.name_span.is_some_and(|name| on_line(name.start)))
        {
            if inline && event.extended {
                actions.extend(self.inline_inherited_action(
                    &params.text_document.uri,
                    text,
                    machine,
                    event,
                ));
            }
            if refactor && !event.extended {
                actions.extend(self.extend_instead_action(
                    &params.text_document.uri,
                    text,
                    machine,
                    event,
                ));
            }
        }
        if refactor
            && on_header
            && let Component::Machine(machine) = component
        {
            actions.extend(self.refine_unrefined_action(
                &params.text_document.uri,
                text,
                machine,
                printer,
            ));
        }
        actions
    }

    /// The events `event` of `machine` extends, root first, each with the
    /// text of the file it is written in: its abstract event, and that one's
    /// while it is extended in turn.
    fn extended_chain(&self, machine: &Machine, event: &Event) -> Option<Vec<(String, Event)>> {
        let loader = ComponentLoader::new(&self.cross_ref_manager, Some(&self.document_manager));
        let mut chain = Vec::new();
        let mut visited = HashSet::new();
        let mut parent = machine.refines.clone()?;
        let mut target = event.refines.first()?.name.clone();
        loop {
            if !visited.insert(parent.clone()) {
                return None;
            }
            let loaded = loader.load(&parent)?;
            let Component::Machine(abstraction) = loaded.component() else {
                return None;
            };
            let abstract_event = abstraction.events.iter().find(|e| e.name == target)?;
            chain.push((loaded.text().to_string(), abstract_event.clone()));
            if !abstract_event.extended {
                break;
            }
            parent = abstraction.refines.clone()?;
            target = abstract_event.refines.first()?.name.clone();
        }
        chain.reverse();
        Some(chain)
    }

    /// Write what an extended `event` inherits into it: the parameters,
    /// guards and actions of the events it extends, root first and under
    /// their own labels, ahead of its own; and make it refine its abstract
    /// event instead of extending it. The event means what it did: it keeps
    /// every abstract parameter, so it needs no witness. Each inherited
    /// element is copied as written in its own file.
    fn inline_inherited_action(
        &self,
        uri: &Uri,
        text: &str,
        machine: &Machine,
        event: &Event,
    ) -> Option<CodeAction> {
        let chain = self.extended_chain(machine, event)?;
        let target = event.refines.first()?;
        let title = format!("Inline what {} inherits from {}", event.name, target.name);
        // Written into one event, two guards or actions under one label are
        // a local duplicate, and the checker drops every copy of one: an own
        // label reusing an inherited one, or two levels of the chain sharing
        // a label, would lose an element the event has now.
        let mut labels = HashSet::new();
        let shared = chain
            .iter()
            .map(|(_, e)| e)
            .chain(std::iter::once(event))
            .flat_map(|e| {
                e.guards
                    .iter()
                    .filter_map(|g| g.label.as_deref())
                    .chain(e.actions.iter().filter_map(|a| a.label.as_deref()))
            })
            .find(|label| !labels.insert(*label));
        if let Some(label) = shared {
            return Some(CodeAction {
                title,
                kind: Some(CodeActionKind::REFACTOR_INLINE),
                diagnostics: None,
                edit: None,
                command: None,
                is_preferred: Some(false),
                disabled: Some(crate::lsp_types::CodeActionDisabled {
                    reason: format!("@{label} would be written twice"),
                }),
                data: None,
            });
        }
        let mut parameters: Vec<&str> = Vec::new();
        let mut guards = Vec::new();
        let mut actions = Vec::new();
        for (file, ancestor) in &chain {
            let masked = rossi::comments::mask_comments(file);
            // An element's span may run on over a comment written after it.
            let written = |span: rossi::ast::Span| {
                file[span.start..span.start + masked[span.start..span.end].trim_end().len()]
                    .to_string()
            };
            for parameter in &ancestor.parameters {
                let name = parameter.name.as_str();
                if !parameters.contains(&name)
                    && !event.parameters.iter().any(|own| own.name == name)
                {
                    parameters.push(name);
                }
            }
            // An element with no span cannot be copied, and leaving it out
            // would change what the event means.
            for guard in &ancestor.guards {
                guards.push(written(guard.span?));
            }
            for action in &ancestor.actions {
                actions.push(written(action.span?));
            }
        }

        let masked = rossi::comments::mask_comments(text);
        let span = event.span?;
        let lowercase = event_is_lowercase(text, event);
        let clause_indent = event_clause_indent(text, event)?;
        // The start of a line an element has to itself, or `None`.
        let own_line = |offset: usize| {
            let start = line_start(text, offset);
            masked[start..offset].trim().is_empty().then_some(start)
        };
        let item_indent = event
            .guards
            .iter()
            .filter_map(|g| g.span)
            .chain(event.actions.iter().filter_map(|a| a.span))
            .find_map(|item| own_line(item.start))
            .map(|start| crate::code_actions::indentation(&text[start..]).to_string())
            .unwrap_or_else(|| format!("{clause_indent}  "));
        let at = |offset: usize| {
            let position = crate::position::offset_to_position(text, offset);
            Range::new(position, position)
        };

        // The header: `extends t` becomes `refines t`.
        let name_end = event.name_span?.end;
        let target_start = target.span?.start;
        let keyword =
            name_end + masked[name_end..target_start].find(|c: char| !c.is_whitespace())?;
        let keyword_end = keyword + masked[keyword..target_start].trim_end().len();
        let mut edits = vec![TextEdit {
            range: crate::position::span_to_range(
                &rossi::ast::Span {
                    start: keyword,
                    end: keyword_end,
                },
                text,
            ),
            new_text: keyword_text(
                KeywordId::Refines,
                masked[keyword..].starts_with(char::is_lowercase),
            ),
        }];
        if !parameters.is_empty() {
            let insert = parameter_insert(text, event, &parameters)?;
            edits.push(TextEdit {
                range: Range::new(insert.position, insert.position),
                new_text: insert.text,
            });
        }
        let items = |items: &[String]| -> String {
            items
                .iter()
                .map(|item| format!("{item_indent}{item}\n"))
                .collect()
        };
        if !guards.is_empty() {
            match event.guards.first().and_then(|g| g.span) {
                Some(first) => edits.push(TextEdit {
                    range: at(own_line(first.start)?),
                    new_text: items(&guards),
                }),
                None => {
                    let after = event
                        .parameters
                        .iter()
                        .filter_map(|p| p.span)
                        .map(|s| s.end)
                        .chain(event_header_end(event))
                        .max()?;
                    let line_end = text[after..].find('\n').map_or(text.len(), |at| after + at);
                    let block = items(&guards);
                    edits.push(TextEdit {
                        range: at(line_end),
                        new_text: format!(
                            "\n{clause_indent}{}\n{}",
                            keyword_text(KeywordId::Where, lowercase),
                            block.trim_end_matches('\n')
                        ),
                    });
                }
            }
        }
        if !actions.is_empty() {
            match event.actions.first().and_then(|a| a.span) {
                Some(first) => edits.push(TextEdit {
                    range: at(own_line(first.start)?),
                    new_text: items(&actions),
                }),
                None => {
                    // A new THEN goes on the line of the event's END, which
                    // must hold nothing else.
                    let end_line = line_start(text, span.end.checked_sub(1)?);
                    if line_keyword(&masked[end_line..span.end]) != Some(KeywordId::End) {
                        return None;
                    }
                    edits.push(TextEdit {
                        range: at(end_line),
                        new_text: format!(
                            "{clause_indent}{}\n{}",
                            keyword_text(KeywordId::Then, lowercase),
                            items(&actions)
                        ),
                    });
                }
            }
        }
        Some(CodeAction {
            title,
            kind: Some(CodeActionKind::REFACTOR_INLINE),
            diagnostics: None,
            edit: Some(WorkspaceEdit {
                changes: Some(HashMap::from([(uri.clone(), edits)])),
                document_changes: None,
                change_annotations: None,
            }),
            command: None,
            is_preferred: Some(false),
            disabled: None,
            data: None,
        })
    }

    /// Make a refining `event` that repeats everything its abstract event
    /// has extend it instead: drop the abstract parameters, guards and
    /// actions it lists again, and turn `refines` into `extends`. The event
    /// means what it did, since an extended event inherits exactly what is
    /// dropped. Offered only when each abstract guard and action is repeated
    /// as it is (compared as formulas, not as text) and each abstract
    /// parameter is declared again; disabled when a label the event keeps
    /// would clash with an inherited one.
    fn extend_instead_action(
        &self,
        uri: &Uri,
        text: &str,
        machine: &Machine,
        event: &Event,
    ) -> Option<CodeAction> {
        let [target] = event.refines.as_slice() else {
            return None;
        };
        let chain = self.extended_chain(machine, event)?;
        let inherited_parameters: HashSet<&str> = chain
            .iter()
            .flat_map(|(_, e)| e.parameters.iter().map(|p| p.name.as_str()))
            .collect();
        if !inherited_parameters
            .iter()
            .all(|name| event.parameters.iter().any(|own| own.name == *name))
        {
            return None;
        }
        // Each inherited element is matched to a distinct own one written the
        // same; what stays unmatched is the event's own.
        fn matched<T, U>(
            own: &[T],
            inherited: &[&U],
            same: impl Fn(&T, &U) -> bool,
        ) -> Option<Vec<bool>> {
            let mut used = vec![false; own.len()];
            for element in inherited {
                let index = (0..own.len()).find(|&i| !used[i] && same(&own[i], element))?;
                used[index] = true;
            }
            Some(used)
        }
        let inherited_guards: Vec<&rossi::LabeledPredicate> =
            chain.iter().flat_map(|(_, e)| e.guards.iter()).collect();
        let inherited_actions: Vec<&rossi::LabeledAction> =
            chain.iter().flat_map(|(_, e)| e.actions.iter()).collect();
        let repeated_guards = matched(&event.guards, &inherited_guards, |own, inherited| {
            own.is_theorem == inherited.is_theorem && own.predicate == inherited.predicate
        })?;
        let repeated_actions = matched(&event.actions, &inherited_actions, |own, inherited| {
            own.action == inherited.action
        })?;

        let title = format!("Extend {} instead of repeating it", target.name);
        let inherited_labels: HashSet<&str> = inherited_guards
            .iter()
            .filter_map(|g| g.label.as_deref())
            .chain(inherited_actions.iter().filter_map(|a| a.label.as_deref()))
            .collect();
        let clash = event
            .guards
            .iter()
            .zip(&repeated_guards)
            .filter(|(_, repeated)| !**repeated)
            .filter_map(|(g, _)| g.label.as_deref())
            .chain(
                event
                    .actions
                    .iter()
                    .zip(&repeated_actions)
                    .filter(|(_, repeated)| !**repeated)
                    .filter_map(|(a, _)| a.label.as_deref()),
            )
            .find(|label| inherited_labels.contains(label));
        if let Some(label) = clash {
            return Some(CodeAction {
                title,
                kind: Some(CodeActionKind::REFACTOR),
                diagnostics: None,
                edit: None,
                command: None,
                is_preferred: Some(false),
                disabled: Some(crate::lsp_types::CodeActionDisabled {
                    reason: format!("the event's own @{label} would clash with an inherited label"),
                }),
                data: None,
            });
        }

        let masked = rossi::comments::mask_comments(text);
        let lowercase = event_is_lowercase(text, event);
        let mut removed: Vec<std::ops::Range<usize>> = Vec::new();

        // The header: `refines t` becomes `extends t`, pulled up onto the
        // header line when the target was written in a clause below it.
        let name_end = event.name_span?.end;
        let target_span = target.span?;
        let keyword =
            name_end + masked[name_end..target_span.start].find(|c: char| !c.is_whitespace())?;
        let mut edits = Vec::new();
        if text[name_end..keyword].contains('\n') {
            removed.push(own_lines(text, &masked, keyword, target_span.end)?);
            edits.push(TextEdit {
                range: point(text, name_end),
                new_text: format!(
                    " {} {}",
                    keyword_text(KeywordId::Extends, lowercase),
                    target.name
                ),
            });
        } else {
            edits.push(TextEdit {
                range: crate::position::span_to_range(
                    &rossi::ast::Span {
                        start: keyword,
                        end: keyword + masked[keyword..target_span.start].trim_end().len(),
                    },
                    text,
                ),
                new_text: keyword_text(
                    KeywordId::Extends,
                    masked[keyword..].starts_with(char::is_lowercase),
                ),
            });
        }

        // The parameters declared again.
        let parameters: Vec<rossi::ast::Span> = event
            .parameters
            .iter()
            .map(|p| p.span)
            .collect::<Option<_>>()?;
        let dropped: Vec<bool> = event
            .parameters
            .iter()
            .map(|p| inherited_parameters.contains(p.name.as_str()))
            .collect();
        removed.extend(clause_removals(text, &masked, &parameters, &dropped, true)?);
        // The guards and actions repeated.
        let spans =
            |spans: Vec<Option<rossi::ast::Span>>| spans.into_iter().collect::<Option<Vec<_>>>();
        let guards = spans(event.guards.iter().map(|g| g.span).collect())?;
        removed.extend(clause_removals(
            text,
            &masked,
            &guards,
            &repeated_guards,
            false,
        )?);
        let actions = spans(event.actions.iter().map(|a| a.span).collect())?;
        removed.extend(clause_removals(
            text,
            &masked,
            &actions,
            &repeated_actions,
            false,
        )?);

        edits.extend(removed.into_iter().map(|range| TextEdit {
            range: crate::position::span_to_range(
                &rossi::ast::Span {
                    start: range.start,
                    end: range.end,
                },
                text,
            ),
            new_text: String::new(),
        }));
        Some(CodeAction {
            title,
            kind: Some(CodeActionKind::REFACTOR),
            diagnostics: None,
            edit: Some(WorkspaceEdit {
                changes: Some(HashMap::from([(uri.clone(), edits)])),
                document_changes: None,
                change_annotations: None,
            }),
            command: None,
            is_preferred: Some(false),
            disabled: None,
            data: None,
        })
    }

    /// Refine the events of `machine`'s abstraction that no event of it
    /// refines, with the extended events a refinement starts from: Rodin
    /// warns about an abstract event left unrefined. They go after the last
    /// event, or into a new EVENTS clause before the machine's END. An event
    /// whose extension would inherit a variable `machine` drops is left out.
    fn refine_unrefined_action(
        &self,
        uri: &Uri,
        text: &str,
        machine: &Machine,
        printer: &rossi::PrettyPrinter,
    ) -> Option<CodeAction> {
        let abstraction = self.load_machine(machine.refines.as_deref()?)?;
        let refined: HashSet<&str> = machine
            .events
            .iter()
            .flat_map(|event| event.refines.iter().map(|target| target.name.as_str()))
            .collect();
        let mut stubs = refinement_of(&abstraction, machine.name.clone());
        stubs
            .events
            .retain(|event| !refined.contains(event.name.as_str()));
        if machine.initialisation.is_some() {
            stubs.initialisation = None;
        }
        // An extended event inherits every guard and action of the events it
        // extends. One reading or assigning a variable this machine drops
        // would bring back what the machine no longer has, and how to refine
        // such an event is the user's to say, so it gets no stub.
        let kept: HashSet<&str> = machine.variables.iter().map(|v| v.name.as_str()).collect();
        let dropped: HashSet<&str> = abstraction
            .variables
            .iter()
            .map(|v| v.name.as_str())
            .filter(|name| !kept.contains(name))
            .collect();
        let initialisation = rossi::keywords::spell(KeywordId::Initialisation);
        let inherits_dropped = |event: &str| {
            let mut visited = HashSet::new();
            let (mut parent, mut target) = (abstraction.clone(), event.to_string());
            while visited.insert(parent.name.clone()) {
                let (guards, actions, extended, next) = if target == initialisation {
                    let Some(init) = &parent.initialisation else {
                        return false;
                    };
                    (&[][..], &init.actions[..], init.extended, target.clone())
                } else {
                    let Some(abstract_event) = parent.events.iter().find(|e| e.name == target)
                    else {
                        return false;
                    };
                    let next = abstract_event.refines.first().map(|t| t.name.clone());
                    (
                        &abstract_event.guards[..],
                        &abstract_event.actions[..],
                        abstract_event.extended,
                        next.unwrap_or_default(),
                    )
                };
                let touches = guards
                    .iter()
                    .flat_map(|guard| guard.predicate.free_identifiers())
                    .chain(
                        actions
                            .iter()
                            .flat_map(|action| action.action.free_identifiers()),
                    )
                    .any(|name| dropped.contains(name.trim_end_matches('\'')));
                if touches {
                    return true;
                }
                let Some(grand) = extended
                    .then(|| parent.refines.as_deref().and_then(|p| self.load_machine(p)))
                    .flatten()
                else {
                    return false;
                };
                (parent, target) = (grand, next);
            }
            false
        };
        if !dropped.is_empty() {
            stubs.events.retain(|event| !inherits_dropped(&event.name));
            if stubs.initialisation.is_some() && inherits_dropped(initialisation) {
                stubs.initialisation = None;
            }
        }
        let count = stubs.events.len() + usize::from(stubs.initialisation.is_some());
        let title = match (count, stubs.initialisation.as_ref(), stubs.events.first()) {
            (0, _, _) => return None,
            (1, Some(_), _) => "Refine abstract event INITIALISATION".to_string(),
            (1, None, Some(event)) => format!("Refine abstract event {}", event.name),
            _ => format!("Refine {count} abstract events left unrefined"),
        };
        let (events_line, block) = printed_events(printer, &stubs)?;
        let last_event = machine
            .events
            .iter()
            .filter_map(|event| event.span)
            .chain(machine.initialisation.as_ref().and_then(|init| init.span))
            .map(|span| span.end)
            .max();
        let (at, insert) = match last_event {
            Some(end) => (
                text[end..].find('\n').map_or(text.len(), |at| end + at + 1),
                format!("\n{block}"),
            ),
            None => {
                let end_line = line_start(text, machine.span?.end.checked_sub(1)?);
                let has_clause = machine
                    .clauses
                    .iter()
                    .any(|clause| clause.keyword == KeywordId::Events);
                let header = if has_clause {
                    String::new()
                } else {
                    format!("{events_line}\n")
                };
                (end_line, format!("{header}{block}"))
            }
        };
        let position = crate::position::offset_to_position(text, at);
        Some(CodeAction {
            title,
            kind: Some(CodeActionKind::REFACTOR),
            diagnostics: None,
            edit: Some(WorkspaceEdit {
                changes: Some(HashMap::from([(
                    uri.clone(),
                    vec![TextEdit {
                        range: Range::new(position, position),
                        new_text: insert,
                    }],
                )])),
                document_changes: None,
                change_annotations: None,
            }),
            command: None,
            is_preferred: Some(false),
            disabled: None,
            data: None,
        })
    }

    /// Create the next level of `component` in a new file beside it: a
    /// refinement of a machine, an extension of a context.
    fn create_next_level_action(
        &self,
        uri: &Uri,
        component: &Component,
        printer: &rossi::PrettyPrinter,
    ) -> Option<CodeAction> {
        let directory = uri.to_file_path()?.parent()?.to_path_buf();
        let name = self.free_name(component.name(), &directory);
        let target = Uri::from_file_path(directory.join(format!("{name}.eventb")))?;
        let (what, text) = match component {
            Component::Machine(machine) => (
                "refinement",
                printer.print_machine(&refinement_of(machine, name.clone())),
            ),
            Component::Context(context) => {
                let mut extension = Context::new(name.clone());
                extension.extends = vec![context.name.clone()];
                ("extension", printer.print_context(&extension))
            }
        };
        Some(CodeAction {
            title: format!("Create {what} {name} of {}", component.name()),
            kind: Some(CodeActionKind::REFACTOR),
            diagnostics: None,
            edit: Some(create_file_edit(target, text)),
            command: None,
            is_preferred: Some(false),
            disabled: None,
            data: None,
        })
    }

    /// The name Rodin proposes for a refinement of `name`: its trailing
    /// number counted on, keeping the digits' width, or `1` appended when it
    /// has none, until no workspace component and no file in `directory`
    /// takes it (`RefineProposer`). Rodin also counts a leading number on,
    /// but a name written in `.eventb` text cannot start with a digit.
    fn free_name(&self, name: &str, directory: &std::path::Path) -> String {
        let taken = self.cross_ref_manager.all_component_names();
        let base = name.trim_end_matches(|c: char| c.is_ascii_digit());
        let digits = &name[base.len()..];
        let width = digits.len().max(1);
        let mut number = digits.parse::<u128>().unwrap_or(0);
        loop {
            number += 1;
            let candidate = format!("{base}{number:0width$}");
            // The workspace indexes only `.eventb` text: a Rodin machine or
            // context file beside it takes the name too.
            if !taken.contains(&candidate)
                && ["eventb", "bum", "buc"]
                    .iter()
                    .all(|extension| !directory.join(format!("{candidate}.{extension}")).exists())
            {
                return candidate;
            }
        }
    }
}

/// The refinement `name` of `machine` Rodin's Refine wizard writes: REFINES
/// the machine, the same SEES, every variable kept, no invariant or variant,
/// and each event extended under its own name. Convergence is not inherited
/// as such: an anticipated event stays anticipated, a convergent one becomes
/// ordinary, since the refinement has no variant of its own yet.
fn refinement_of(machine: &Machine, name: String) -> Machine {
    let mut refinement = Machine::new(name);
    refinement.refines = Some(machine.name.clone());
    refinement.sees = machine.sees.clone();
    refinement.variables = machine
        .variables
        .iter()
        .map(|variable| NamedElement::new(variable.name.clone()))
        .collect();
    refinement.initialisation = machine
        .initialisation
        .as_ref()
        .map(|_| InitialisationEvent {
            actions: Vec::new(),
            comment: None,
            extended: true,
            with: Vec::new(),
            witnesses: Vec::new(),
            span: None,
            name_span: None,
        });
    refinement.events = machine
        .events
        .iter()
        .map(|event| {
            let mut extended = Event::new(event.name.clone());
            extended.status = (event.status == Some(EventStatus::Anticipated))
                .then_some(EventStatus::Anticipated);
            extended.refines = vec![NamedElement::new(event.name.clone())];
            extended.extended = true;
            extended
        })
        .collect();
    refinement
}

/// An empty range at byte `offset` of `text`.
fn point(text: &str, offset: usize) -> Range {
    let position = crate::position::offset_to_position(text, offset);
    Range::new(position, position)
}

/// The ranges deleting the `items` of one clause marked in `dropped`, or the
/// whole clause, keyword included, when every item goes. An item is deleted
/// with its lines; a name in an inline list (`names`) with the space before
/// it. `None` when an item or the clause does not have its lines to itself.
fn clause_removals(
    text: &str,
    masked: &str,
    items: &[rossi::ast::Span],
    dropped: &[bool],
    names: bool,
) -> Option<Vec<std::ops::Range<usize>>> {
    // An element's span may run on over a comment written after it.
    let code_end =
        |span: &rossi::ast::Span| span.start + masked[span.start..span.end].trim_end().len();
    if items.is_empty() || !dropped.iter().any(|d| *d) {
        return Some(Vec::new());
    }
    if dropped.iter().all(|d| *d) {
        let before = masked[..items[0].start].trim_end();
        let keyword = before.rfind(char::is_whitespace).map_or(0, |at| at + 1);
        line_keyword(&masked[keyword..items[0].start])?;
        return Some(vec![own_lines(
            text,
            masked,
            keyword,
            code_end(items.last()?),
        )?]);
    }
    items
        .iter()
        .zip(dropped)
        .filter(|(_, dropped)| **dropped)
        .map(
            |(item, _)| match own_lines(text, masked, item.start, code_end(item)) {
                Some(lines) => Some(lines),
                None if names => {
                    let previous = masked[..item.start].trim_end().len();
                    Some(previous..item.end)
                }
                None => None,
            },
        )
        .collect()
}

/// `machine`'s events as the printer writes them: the EVENTS keyword line and
/// the block of events under it, in the configured style.
fn printed_events(printer: &rossi::PrettyPrinter, machine: &Machine) -> Option<(String, String)> {
    let printed = printer.print_machine(machine);
    let lines: Vec<&str> = printed.lines().collect();
    let keyword = lines
        .iter()
        .position(|line| line_keyword(line) == Some(KeywordId::Events))?;
    // Everything up to the machine's own END, the last line.
    let block: String = lines[keyword + 1..lines.len() - 1]
        .iter()
        .map(|line| format!("{line}\n"))
        .collect();
    Some((lines[keyword].to_string(), block))
}

/// A workspace edit creating the file `uri`, which must not exist yet, with
/// `text` in it.
fn create_file_edit(uri: Uri, text: String) -> WorkspaceEdit {
    WorkspaceEdit {
        changes: None,
        document_changes: Some(DocumentChanges::Operations(vec![
            DocumentChangeOperation::Op(ResourceOp::Create(CreateFile {
                uri: uri.clone(),
                options: Some(CreateFileOptions {
                    overwrite: Some(false),
                    ignore_if_exists: Some(false),
                }),
                annotation_id: None,
            })),
            DocumentChangeOperation::Edit(TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier { uri, version: None },
                edits: vec![OneOf::Left(TextEdit {
                    range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                    new_text: text,
                })],
            }),
        ])),
        change_annotations: None,
    }
}
