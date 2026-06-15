//! End-to-end regression for the reported bug: a single local syntax error
//! used to disable hover and go-to-definition for the *whole* document. Here a
//! machine invariant is left dangling (`partition(...) ∈` with no right-hand
//! side), exactly the edit from the report, and every navigation feature must
//! keep working everywhere outside the broken clause.

use rossi_lsp::cross_references::CrossReferenceManager;
use rossi_lsp::definition::DefinitionProvider;
use rossi_lsp::document::DocumentManager;
use rossi_lsp::hover::HoverProvider;
use rossi_lsp::lsp_types::*;
use std::sync::Arc;

/// A two-component model (context + machine that sees it), mirroring
/// `base-model.eventb`, with one broken invariant in the machine.
///
/// Line map (0-indexed):
/// ```text
///  0
///  1 context C1
///  2 sets
///  3     Names
///  4 constants
///  5     Root
///  6 axioms
///  7     @RootType Root ∈ Names
///  8 end
///  9
/// 10 machine M1
/// 11 sees C1
/// 12 variables
/// 13     Roles
/// 14     AdmRoles
/// 15 invariants
/// 16     @EntitiesPartition
/// 17         partition(Roles, AdmRoles) ∈
/// 18     @RolesType
/// 19         Roles ⊆ Names
/// 20 end
/// ```
const SOURCE: &str = r#"
context C1
sets
    Names
constants
    Root
axioms
    @RootType Root ∈ Names
end

machine M1
sees C1
variables
    Roles
    AdmRoles
invariants
    @EntitiesPartition
        partition(Roles, AdmRoles) ∈
    @RolesType
        Roles ⊆ Names
end
"#;

fn uri() -> Url {
    Url::parse("file:///model.eventb").unwrap()
}

fn goto_params(line: u32, character: u32) -> GotoDefinitionParams {
    GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri() },
            position: Position::new(line, character),
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    }
}

fn hover_params(line: u32, character: u32) -> HoverParams {
    HoverParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri() },
            position: Position::new(line, character),
        },
        work_done_progress_params: Default::default(),
    }
}

fn scalar(response: Option<GotoDefinitionResponse>) -> Location {
    match response.expect("definition should resolve") {
        GotoDefinitionResponse::Scalar(location) => location,
        other => panic!("expected a scalar location, got {other:?}"),
    }
}

fn setup() -> (DefinitionProvider, HoverProvider) {
    let crm = Arc::new(CrossReferenceManager::new());
    let dm = Arc::new(DocumentManager::new());
    crm.update_component(uri().to_string(), SOURCE);
    dm.open(uri(), "eventb".to_string(), 1, SOURCE.to_string());

    let mut def = DefinitionProvider::new();
    def.set_cross_reference_manager(Arc::clone(&crm));
    def.set_document_manager(Arc::clone(&dm));
    def.update_definitions(uri().to_string(), SOURCE);

    let mut hov = HoverProvider::new();
    hov.set_cross_reference_manager(Arc::clone(&crm));
    hov.set_document_manager(Arc::clone(&dm));

    (def, hov)
}

#[test]
fn goto_resolves_a_variable_in_the_broken_machine() {
    // `Roles` used on line 19 resolves to its declaration on line 13 — even
    // though the machine containing it failed a strict parse (recovery records
    // the declaration's span).
    let (def, _hov) = setup();
    let location = scalar(def.goto_definition(&goto_params(19, 10), SOURCE));
    assert_eq!(location.uri, uri());
    assert_eq!(location.range.start, Position::new(13, 4));
}

#[test]
fn goto_resolves_a_cross_file_set_from_the_broken_machine() {
    // `Names` (a set in the seen context C1) resolves from inside the broken
    // machine — the healthy context keeps its real spans.
    let (def, _hov) = setup();
    let location = scalar(def.goto_definition(&goto_params(19, 18), SOURCE));
    assert_eq!(location.uri, uri());
    assert_eq!(location.range.start, Position::new(3, 4));
}

#[test]
fn goto_resolves_the_sees_context_name() {
    // `C1` in `sees C1` (line 11) jumps to `context C1` (line 1).
    let (def, _hov) = setup();
    let location = scalar(def.goto_definition(&goto_params(11, 5), SOURCE));
    assert_eq!(location.uri, uri());
    assert_eq!(location.range.start, Position::new(1, 8));
}

#[test]
fn hover_still_works_after_a_local_error() {
    // Hovering a variable used near the broken invariant still produces the
    // variable's documentation rather than nothing.
    let (_def, hov) = setup();
    let hover = hov
        .hover(&hover_params(19, 10), SOURCE)
        .expect("hover on `Roles` should resolve");
    let HoverContents::Markup(content) = hover.contents else {
        panic!("expected markup hover content");
    };
    assert!(
        content.value.contains("Variable"),
        "expected variable docs, got: {}",
        content.value
    );
}
