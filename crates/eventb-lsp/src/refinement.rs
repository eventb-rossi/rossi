//! Refactorings along the refinement chain.
//!
//! The counterparts of Rodin's Refine wizard (`RefineMachine`, and
//! `ExtendContext` for a context): a refinement of the machine, or an
//! extension of the context, at the cursor, written to a new file next to it;
//! and the refinement of the abstract events a machine leaves unrefined.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rossi::{Component, Context, Event, EventStatus, InitialisationEvent, Machine, NamedElement};

use crate::code_actions::kind_requested;
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
        if !kind_requested(params, &CodeActionKind::REFACTOR) {
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
        let on_header = component.name_span().is_some_and(|name| {
            let line = |offset: usize| text[..offset].matches('\n').count();
            line(name.start) == line(cursor)
        });
        let mut actions = Vec::new();
        if on_header && creates_files {
            actions.extend(self.create_next_level_action(
                &params.text_document.uri,
                component,
                printer,
            ));
        }
        if on_header && let Component::Machine(machine) = component {
            actions.extend(self.refine_unrefined_action(
                &params.text_document.uri,
                text,
                machine,
                printer,
            ));
        }
        actions
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

/// The start of the line holding byte `offset` of `text`.
fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |at| at + 1)
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
