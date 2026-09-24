//! Integration tests for the refactorings along the refinement chain.

use eventb_lsp::cross_references::CrossReferenceManager;
use eventb_lsp::lsp_types::{
    CodeAction, CodeActionContext, CodeActionKind, CodeActionParams, DocumentChangeOperation,
    DocumentChanges, OneOf, Position, Range, ResourceOp, TextDocumentIdentifier, Uri,
    WorkDoneProgressParams,
};
use eventb_lsp::refinement::RefinementActionProvider;
use std::sync::Arc;

/// A provider over a workspace holding `files` (URI, text).
fn provider(files: &[(&str, &str)]) -> RefinementActionProvider {
    let components = Arc::new(CrossReferenceManager::new());
    for (uri, text) in files {
        components.update_component((*uri).to_string(), text);
    }
    RefinementActionProvider::new(components)
}

/// The refactors offered with the cursor at `line`, `column` of `uri`.
fn refactors(
    provider: &RefinementActionProvider,
    uri: &str,
    text: &str,
    line: u32,
    column: u32,
    creates_files: bool,
) -> Vec<CodeAction> {
    let cursor = Position::new(line, column);
    let params = CodeActionParams {
        text_document: TextDocumentIdentifier {
            uri: uri.parse::<Uri>().unwrap(),
        },
        range: Range::new(cursor, cursor),
        context: CodeActionContext {
            diagnostics: vec![],
            only: Some(vec![CodeActionKind::REFACTOR]),
            trigger_kind: None,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: Default::default(),
    };
    provider.provide(
        &params,
        text,
        &rossi::PrettyPrinter::default(),
        creates_files,
    )
}

/// The file an action creates and the text it writes into it.
fn created(action: &CodeAction) -> (String, String) {
    let Some(DocumentChanges::Operations(operations)) = action
        .edit
        .as_ref()
        .and_then(|edit| edit.document_changes.clone())
    else {
        panic!("{:?} creates no file", action.title);
    };
    let [
        DocumentChangeOperation::Op(ResourceOp::Create(create)),
        DocumentChangeOperation::Edit(edit),
    ] = operations.as_slice()
    else {
        panic!("expected a create and its text: {operations:?}");
    };
    assert_eq!(edit.text_document.uri, create.uri);
    let [OneOf::Left(text)] = edit.edits.as_slice() else {
        panic!("expected one insert: {:?}", edit.edits);
    };
    (create.uri.as_str().to_string(), text.new_text.clone())
}

const ABSTRACT: &str = "\
machine M0 sees C0 C1
variables x y
invariants
  @t1 x ∈ ℕ
  @t2 y ∈ ℕ
variant x
events
  event INITIALISATION
    then
      @a1 x ≔ 0
      @a2 y ≔ 0
  end
  convergent event dec
    where
      @g x > 0
    then
      @a x ≔ x − 1
  end
  anticipated event wait
    then
      @a y ≔ y + 1
  end
end
";

#[test]
fn a_refinement_is_created_the_way_rodin_refines() {
    let uri = "file:///ws/M0.eventb";
    let provider = provider(&[(uri, ABSTRACT)]);
    let actions = refactors(&provider, uri, ABSTRACT, 0, 3, true);
    let action = actions
        .iter()
        .find(|action| action.title.starts_with("Create refinement"))
        .expect("offered on the machine header");
    assert_eq!(action.title, "Create refinement M1 of M0");
    let (created_uri, text) = created(action);
    assert_eq!(created_uri, "file:///ws/M1.eventb");
    // The SEES and every variable are kept; invariants and the variant are
    // not repeated; each event is extended, convergent ones become ordinary
    // and anticipated ones stay anticipated.
    assert_eq!(
        text,
        "\
machine M1 refines M0 sees C0 C1

variables x y

events
  event INITIALISATION extends INITIALISATION
  end

  event dec extends dec
  end

  anticipated event wait extends wait
  end
end
"
    );
    let rossi::Component::Machine(machine) = rossi::parse(&text).expect("the refinement parses")
    else {
        panic!("a machine");
    };
    assert_eq!(machine.refines.as_deref(), Some("M0"));
}

#[test]
fn the_refinement_takes_the_next_free_name() {
    // `M1` is taken, so the proposal counts on, keeping the digits' width.
    let taken = "machine M1\nend\n";
    for (name, expected) in [
        ("M0", "M2"),
        ("M09", "M10"),
        ("M007", "M008"),
        ("Base", "Base1"),
    ] {
        let text = ABSTRACT.replacen("M0", name, 1);
        let uri = format!("file:///ws/{name}.eventb");
        let provider = provider(&[(&uri, &text), ("file:///ws/M1.eventb", taken)]);
        let actions = refactors(&provider, &uri, &text, 0, 3, true);
        let action = actions
            .iter()
            .find(|action| action.title.starts_with("Create refinement"))
            .unwrap();
        assert_eq!(
            action.title,
            format!("Create refinement {expected} of {name}")
        );
    }
}

#[test]
fn no_file_is_created_for_a_client_that_cannot() {
    let uri = "file:///ws/M0.eventb";
    let provider = provider(&[(uri, ABSTRACT)]);
    assert!(refactors(&provider, uri, ABSTRACT, 0, 3, false).is_empty());
    // Nor away from the header.
    assert!(refactors(&provider, uri, ABSTRACT, 3, 3, true).is_empty());
}

#[test]
fn an_extension_is_created_the_way_rodin_extends() {
    let uri = "file:///ws/C0.eventb";
    let context = "context C0\nsets S\nconstants k\naxioms\n  @k k ∈ S\nend\n";
    let provider = provider(&[(uri, context)]);
    let actions = refactors(&provider, uri, context, 0, 3, true);
    let action = actions
        .iter()
        .find(|action| action.title.starts_with("Create extension"))
        .expect("offered on the context header");
    assert_eq!(action.title, "Create extension C1 of C0");
    let (created_uri, text) = created(action);
    assert_eq!(created_uri, "file:///ws/C1.eventb");
    assert_eq!(text, "context C1 extends C0\nend\n");
}
