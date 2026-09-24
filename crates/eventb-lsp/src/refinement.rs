//! Refactorings along the refinement chain.
//!
//! The counterparts of Rodin's Refine wizard (`RefineMachine`): a refinement
//! of the machine at the cursor, written to a new file next to it.

use std::sync::Arc;

use rossi::{Component, Event, EventStatus, InitialisationEvent, Machine, NamedElement};

use crate::code_actions::kind_requested;
use crate::cross_references::CrossReferenceManager;
use crate::lsp_types::{
    CodeAction, CodeActionKind, CodeActionParams, CreateFile, CreateFileOptions,
    DocumentChangeOperation, DocumentChanges, OneOf, OptionalVersionedTextDocumentIdentifier,
    Position, Range, ResourceOp, TextDocumentEdit, TextEdit, Uri, WorkspaceEdit,
};

/// Provides the refactors that create or rewrite components along the
/// refinement chain.
pub struct RefinementActionProvider {
    /// The workspace's components, so a new component takes a free name.
    cross_ref_manager: Arc<CrossReferenceManager>,
}

impl RefinementActionProvider {
    pub fn new(cross_ref_manager: Arc<CrossReferenceManager>) -> Self {
        Self { cross_ref_manager }
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
        if on_header
            && creates_files
            && let Component::Machine(machine) = component
        {
            actions.extend(self.create_refinement_action(
                &params.text_document.uri,
                machine,
                printer,
            ));
        }
        actions
    }

    /// Create a refinement of `machine` in a new file beside it.
    fn create_refinement_action(
        &self,
        uri: &Uri,
        machine: &Machine,
        printer: &rossi::PrettyPrinter,
    ) -> Option<CodeAction> {
        let directory = uri.to_file_path()?.parent()?.to_path_buf();
        let name = self.free_name(&machine.name, &directory);
        let target = Uri::from_file_path(directory.join(format!("{name}.eventb")))?;
        Some(CodeAction {
            title: format!("Create refinement {name} of {}", machine.name),
            kind: Some(CodeActionKind::REFACTOR),
            diagnostics: None,
            edit: Some(create_file_edit(
                target,
                printer.print_machine(&refinement_of(machine, name)),
            )),
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
