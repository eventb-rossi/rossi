//! Integration tests for the refactorings along the refinement chain.

use eventb_lsp::cross_references::CrossReferenceManager;
use eventb_lsp::document::DocumentManager;
use eventb_lsp::identifier_utils::position_to_offset;
use eventb_lsp::lsp_types::{
    CodeAction, CodeActionContext, CodeActionKind, CodeActionParams, DocumentChangeOperation,
    DocumentChanges, OneOf, Position, Range, ResourceOp, TextDocumentIdentifier, Uri,
    WorkDoneProgressParams,
};
use eventb_lsp::refinement::RefinementActionProvider;
use std::sync::Arc;

/// A provider over a workspace holding `files` (URI, text), each open.
fn provider(files: &[(&str, &str)]) -> RefinementActionProvider {
    let components = Arc::new(CrossReferenceManager::new());
    let documents = Arc::new(DocumentManager::new());
    for (uri, text) in files {
        components.update_component((*uri).to_string(), text);
        documents.open(uri.parse::<Uri>().unwrap(), 1, (*text).to_string());
    }
    RefinementActionProvider::new(components, documents)
}

/// The text `action` leaves in the document `uri` it edits in place.
fn applied(text: &str, uri: &str, action: &CodeAction) -> String {
    let changes = action
        .edit
        .as_ref()
        .and_then(|edit| edit.changes.as_ref())
        .unwrap_or_else(|| panic!("{:?} edits no document in place", action.title));
    // Last position first; inserts at one position land in array order, so
    // among those the last is applied first.
    let mut edits: Vec<_> = changes[&uri.parse::<Uri>().unwrap()]
        .iter()
        .enumerate()
        .collect();
    edits.sort_by_key(|(index, edit)| std::cmp::Reverse((edit.range.start, *index)));
    let mut result = text.to_string();
    for (_, edit) in edits {
        let start = position_to_offset(&result, edit.range.start).unwrap();
        let end = position_to_offset(&result, edit.range.end).unwrap();
        result.replace_range(start..end, &edit.new_text);
    }
    result
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
    let components = eventb_lsp::component_util::parse_all(text);
    provider.provide(
        &params,
        text,
        &components,
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

#[test]
fn an_abstract_event_on_a_dropped_variable_is_left_unrefined() {
    // M1 drops `v`. Extending `tick` would inherit its action on `v`, which
    // M1 no longer has; how to refine it is the user's to say. `tock` only
    // touches the kept `w`, so its stub is written.
    let abstraction = "\
machine M0
variables v w
invariants
  @t1 v ∈ ℕ
  @t2 w ∈ ℕ
events
  event INITIALISATION
    then
      @a1 v ≔ 0
      @a2 w ≔ 0
  end
  event tick
    then
      @a1 v ≔ v + 1
  end
  event tock
    where
      @g1 w < 9
    then
      @a2 w ≔ w + 1
  end
end
";
    let refinement = "\
machine M1 refines M0
variables w
events
  event INITIALISATION
    then
      @a2 w ≔ 0
  end
end
";
    let uri = "file:///ws/M1.eventb";
    let files = [("file:///ws/M0.eventb", abstraction), (uri, refinement)];
    let actions = refactors(&provider(&files), uri, refinement, 0, 3, false);
    let action = actions
        .iter()
        .find(|action| action.title.starts_with("Refine"))
        .expect("tock is still offered");
    assert_eq!(action.title, "Refine abstract event tock");
    assert_eq!(
        applied(refinement, uri, action),
        refinement.replace(
            "      @a2 w ≔ 0\n  end\n",
            "      @a2 w ≔ 0\n  end\n\n  event tock extends tock\n  end\n"
        )
    );
}

#[test]
fn a_variable_dropped_two_levels_down_is_found_through_the_extensions() {
    // M1 extends every event of M0 and keeps `v`; M2 drops it. Extending
    // M1's `tick` or INITIALISATION would inherit M0's actions on `v`.
    let root = "\
machine M0
variables v w
invariants
  @t1 v ∈ ℕ
  @t2 w ∈ ℕ
events
  event INITIALISATION
    then
      @a1 v ≔ 0
      @a2 w ≔ 0
  end
  event tick
    then
      @a1 v ≔ v + 1
  end
  event tock
    then
      @a2 w ≔ w + 1
  end
end
";
    let middle = "\
machine M1 refines M0
variables v w
events
  event INITIALISATION extends INITIALISATION
  end
  event tick extends tick
  end
  event tock extends tock
  end
end
";
    let refinement = "machine M2 refines M1\nvariables w\nend\n";
    let uri = "file:///ws/M2.eventb";
    let files = [
        ("file:///ws/M0.eventb", root),
        ("file:///ws/M1.eventb", middle),
        (uri, refinement),
    ];
    let actions = refactors(&provider(&files), uri, refinement, 0, 3, false);
    let action = actions
        .iter()
        .find(|action| action.title.starts_with("Refine"))
        .expect("tock is still offered");
    assert_eq!(action.title, "Refine abstract event tock");
}

#[test]
fn the_abstract_events_left_unrefined_are_refined() {
    let refinement = "\
machine M1 refines M0 sees C0 C1

variables x y

events
  event INITIALISATION extends INITIALISATION
  end

  event dec extends dec
  end
end
";
    let files = [
        ("file:///ws/M0.eventb", ABSTRACT),
        ("file:///ws/M1.eventb", refinement),
    ];
    let actions = refactors(
        &provider(&files),
        "file:///ws/M1.eventb",
        refinement,
        0,
        3,
        false,
    );
    let action = actions
        .iter()
        .find(|action| action.title.starts_with("Refine"))
        .expect("offered on the refinement's header");
    assert_eq!(action.title, "Refine abstract event wait");
    assert_eq!(
        applied(refinement, "file:///ws/M1.eventb", action),
        refinement.replace(
            "  event dec extends dec\n  end\n",
            "  event dec extends dec\n  end\n\n  anticipated event wait extends wait\n  end\n",
        )
    );

    // A refinement with no event yet gets its EVENTS clause too.
    let bare = "machine M1 refines M0\nvariables x y\nend\n";
    let provider = provider(&[
        ("file:///ws/M0.eventb", ABSTRACT),
        ("file:///ws/M1.eventb", bare),
    ]);
    let actions = refactors(&provider, "file:///ws/M1.eventb", bare, 0, 3, false);
    let action = actions
        .iter()
        .find(|action| action.title.starts_with("Refine"))
        .unwrap();
    assert_eq!(action.title, "Refine 3 abstract events left unrefined");
    let fixed = applied(bare, "file:///ws/M1.eventb", action);
    assert_eq!(
        fixed,
        "machine M1 refines M0\nvariables x y\nevents\n  event INITIALISATION extends INITIALISATION\n  end\n\n  event dec extends dec\n  end\n\n  anticipated event wait extends wait\n  end\nend\n"
    );
    rossi::parse(&fixed).expect("the refinement parses");
}

/// The error diagnostics of a static check over `files` (name, text).
fn check_errors(files: &[(&str, &str)]) -> Vec<String> {
    let components = files
        .iter()
        .flat_map(|(name, text)| {
            rossi_build::ProjectComponent::from_eventb(format!("{name}.eventb"), text).unwrap()
        })
        .collect();
    rossi_build::build(&rossi_build::Project::new("p", components))
        .diagnostics
        .into_iter()
        .filter(|d| d.severity == rossi_build::Severity::Error)
        .map(|d| d.to_string())
        .collect()
}

#[test]
fn an_extended_event_takes_in_what_it_inherits_past_wide_characters() {
    // Each `≔` in the comments is three bytes but one character: a refactor
    // that mixed the two would read the wrong lines, in either file.
    let base = BASE.replace("  event dec\n", "  // ≔≔≔≔\n  event dec\n");
    let refinement = "machine M1 refines M0\nvariables x\nevents\n  // ≔≔≔≔\n  event dec extends dec\n    then\n      @b1 x ≔ x\n  end\nend\n";
    let uri = "file:///ws/M1.eventb";
    let files = [("file:///ws/M0.eventb", base.as_str()), (uri, refinement)];
    let actions = refactors(&provider(&files), uri, refinement, 4, 9, false);
    let action = actions
        .iter()
        .find(|action| action.title.starts_with("Inline"))
        .expect("offered on the event's header");
    assert_eq!(
        applied(refinement, uri, action),
        "machine M1 refines M0\nvariables x\nevents\n  // ≔≔≔≔\n  event dec refines dec\n    any n\n    where\n      @g1 n ∈ ℕ\n      @g2 x > n\n    then\n      @a1 x ≔ x − n\n      @b1 x ≔ x\n  end\nend\n"
    );
}

#[test]
fn an_extended_event_reusing_an_inherited_label_is_not_inlined() {
    // Inlined, `@g1` would be a local duplicate, and the checker drops every
    // copy of one: the event would lose a guard it has now.
    let refinement = "machine M1 refines M0\nvariables x\nevents\n  event dec extends dec\n    where\n      @g1 x < 9\n  end\nend\n";
    let uri = "file:///ws/M1.eventb";
    let files = [("file:///ws/M0.eventb", BASE), (uri, refinement)];
    let actions = refactors(&provider(&files), uri, refinement, 3, 9, false);
    let action = actions
        .iter()
        .find(|action| action.title.starts_with("Inline"))
        .expect("offered, disabled");
    assert!(action.edit.is_none());
    assert_eq!(
        action
            .disabled
            .as_ref()
            .map(|disabled| disabled.reason.as_str()),
        Some("@g1 would be written twice")
    );

    // The same when two levels of the chain share the label.
    let deeper =
        "machine M2 refines M1\nvariables x\nevents\n  event dec extends dec\n  end\nend\n";
    let uri = "file:///ws/M2.eventb";
    let files = [
        ("file:///ws/M0.eventb", BASE),
        ("file:///ws/M1.eventb", refinement),
        (uri, deeper),
    ];
    let actions = refactors(&provider(&files), uri, deeper, 3, 9, false);
    let action = actions
        .iter()
        .find(|action| action.title.starts_with("Inline"))
        .expect("offered, disabled");
    assert!(action.edit.is_none());
}

const BASE: &str = "\
machine M0
variables x
invariants
  @t x ∈ ℕ
events
  event INITIALISATION
    then
      @a x ≔ 9
  end
  event dec
    any n
    where
      @g1 n ∈ ℕ
      @g2 x > n
    then
      @a1 x ≔ x − n
  end
end
";

#[test]
fn an_extended_event_takes_in_what_it_inherits() {
    let refinement = "\
machine M1 refines M0
variables x y
invariants
  @t y ∈ ℕ
events
  event INITIALISATION extends INITIALISATION
    then
      @b y ≔ 0
  end
  event dec extends dec
    where
      @g3 x > 1
    then
      @b1 y ≔ y + 1
  end
end
";
    let uri = "file:///ws/M1.eventb";
    let files = [("file:///ws/M0.eventb", BASE), (uri, refinement)];
    let actions = refactors(&provider(&files), uri, refinement, 9, 9, false);
    let action = actions
        .iter()
        .find(|action| action.title.starts_with("Inline"))
        .expect("offered on an extended event's header");
    assert_eq!(action.title, "Inline what dec inherits from dec");
    let inlined = applied(refinement, uri, action);
    assert_eq!(
        inlined,
        refinement.replace(
            "  event dec extends dec\n    where\n      @g3 x > 1\n    then\n      @b1 y ≔ y + 1\n",
            "  event dec refines dec\n    any n\n    where\n      @g1 n ∈ ℕ\n      @g2 x > n\n      @g3 x > 1\n    then\n      @a1 x ≔ x − n\n      @b1 y ≔ y + 1\n",
        )
    );
    // The inlined event means what the extended one did.
    assert_eq!(
        check_errors(&[("M0", BASE), ("M1", &inlined)]),
        Vec::<String>::new()
    );
}

#[test]
fn an_extended_event_with_no_clause_gets_them_written() {
    let refinement =
        "machine M1 refines M0\nvariables x\nevents\n  event dec extends dec\n  end\nend\n";
    let uri = "file:///ws/M1.eventb";
    let files = [("file:///ws/M0.eventb", BASE), (uri, refinement)];
    let actions = refactors(&provider(&files), uri, refinement, 3, 9, false);
    let action = actions
        .iter()
        .find(|action| action.title.starts_with("Inline"))
        .unwrap();
    assert_eq!(
        applied(refinement, uri, action),
        "machine M1 refines M0\nvariables x\nevents\n  event dec refines dec\n    any n\n    where\n      @g1 n ∈ ℕ\n      @g2 x > n\n    then\n      @a1 x ≔ x − n\n  end\nend\n"
    );
}

#[test]
fn a_repeating_event_is_extended_past_wide_characters() {
    // Each `≔` in the comment is three bytes but one character: a refactor
    // that mixed the two would drop the wrong lines.
    let commented = REPEATS_DEC.replace(
        "  event dec refines dec\n",
        "  // ≔≔≔≔\n  event dec refines dec\n",
    );
    let uri = "file:///ws/M1.eventb";
    let files = [("file:///ws/M0.eventb", BASE), (uri, commented.as_str())];
    let actions = refactors(&provider(&files), uri, &commented, 10, 9, false);
    let action = actions
        .iter()
        .find(|action| action.title.starts_with("Extend"))
        .expect("offered on the event's header");
    let extended = applied(&commented, uri, action);
    assert!(
        extended.contains(
            "  // ≔≔≔≔\n  event dec extends dec\n    any m\n    where\n      @g3 m = n\n"
        ),
        "{extended}"
    );
    assert_eq!(
        check_errors(&[("M0", BASE), ("M1", &extended)]),
        Vec::<String>::new()
    );
}

const REPEATS_DEC: &str = "\
machine M1 refines M0
variables x y
invariants
  @t y ∈ ℕ
events
  event INITIALISATION extends INITIALISATION
    then
      @b y ≔ 0
  end
  event dec refines dec
    any n m
    where
      @g1 n ∈ ℕ
      @g2 x > n
      @g3 m = n
    then
      @a1 x ≔ x − n
      @b1 y ≔ m
  end
end
";

#[test]
fn a_refining_event_repeating_its_abstract_one_becomes_extended() {
    let uri = "file:///ws/M1.eventb";
    let files = [("file:///ws/M0.eventb", BASE), (uri, REPEATS_DEC)];
    let actions = refactors(&provider(&files), uri, REPEATS_DEC, 9, 9, false);
    let action = actions
        .iter()
        .find(|action| action.title.starts_with("Extend"))
        .expect("offered on the event's header");
    assert_eq!(action.title, "Extend dec instead of repeating it");
    let extended = applied(REPEATS_DEC, uri, action);
    assert_eq!(
        extended,
        REPEATS_DEC.replace(
            "  event dec refines dec\n    any n m\n    where\n      @g1 n ∈ ℕ\n      @g2 x > n\n      @g3 m = n\n    then\n      @a1 x ≔ x − n\n      @b1 y ≔ m\n",
            "  event dec extends dec\n    any m\n    where\n      @g3 m = n\n    then\n      @b1 y ≔ m\n",
        )
    );
    assert_eq!(
        check_errors(&[("M0", BASE), ("M1", &extended)]),
        Vec::<String>::new()
    );
}

#[test]
fn a_status_written_before_the_refines_clause_stays() {
    let block = REPEATS_DEC.replace(
        "  event dec refines dec\n",
        "  event dec\n    status ordinary\n    refines dec\n",
    );
    let uri = "file:///ws/M1.eventb";
    let files = [("file:///ws/M0.eventb", BASE), (uri, block.as_str())];
    let actions = refactors(&provider(&files), uri, &block, 9, 9, false);
    let action = actions
        .iter()
        .find(|action| action.title.starts_with("Extend"))
        .expect("offered on the event's header");
    let extended = applied(&block, uri, action);
    assert!(
        extended.contains("  event dec extends dec\n    status ordinary\n    any m\n"),
        "{extended}"
    );
}

#[test]
fn an_event_that_changes_its_abstract_one_is_not_extended() {
    // `@g2` is strengthened, so the abstract guard is not repeated.
    let changed = REPEATS_DEC.replace("@g2 x > n", "@g2 x > n + 1");
    let uri = "file:///ws/M1.eventb";
    let files = [("file:///ws/M0.eventb", BASE), (uri, changed.as_str())];
    let actions = refactors(&provider(&files), uri, &changed, 9, 9, false);
    assert!(
        actions
            .iter()
            .all(|action| !action.title.starts_with("Extend")),
        "{actions:?}"
    );
}

#[test]
fn an_event_keeping_nothing_of_its_own_loses_the_emptied_clauses() {
    let only = "machine M1 refines M0\nvariables x\nevents\n  event dec refines dec\n    any n\n    where\n      @g1 n ∈ ℕ\n      @g2 x > n\n    then\n      @a1 x ≔ x − n\n  end\nend\n";
    let uri = "file:///ws/M1.eventb";
    let files = [("file:///ws/M0.eventb", BASE), (uri, only)];
    let actions = refactors(&provider(&files), uri, only, 3, 9, false);
    let action = actions
        .iter()
        .find(|action| action.title.starts_with("Extend"))
        .unwrap();
    assert_eq!(
        applied(only, uri, action),
        "machine M1 refines M0\nvariables x\nevents\n  event dec extends dec\n  end\nend\n"
    );

    // U+00A0 separates like a space but takes two bytes.
    let wide = only.replace("    where\n", "   \u{a0}where\n");
    let files = [("file:///ws/M0.eventb", BASE), (uri, wide.as_str())];
    let actions = refactors(&provider(&files), uri, &wide, 3, 9, false);
    let action = actions
        .iter()
        .find(|action| action.title.starts_with("Extend"))
        .unwrap();
    assert_eq!(
        applied(&wide, uri, action),
        "machine M1 refines M0\nvariables x\nevents\n  event dec extends dec\n  end\nend\n"
    );
}
