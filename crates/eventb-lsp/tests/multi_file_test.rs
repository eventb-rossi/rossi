//! Integration tests for multi-file cross-reference functionality
//!
//! These tests verify that the cross-reference manager correctly tracks
//! dependencies between Event-B files and that workspace-wide operations work.

use eventb_lsp::cross_references::{ComponentKind, CrossReferenceManager, ReferenceKind};
use eventb_lsp::document::DocumentManager;
use eventb_lsp::lsp_types::*;
use eventb_lsp::references::ReferenceProvider;
use eventb_lsp::rename::RenameProvider;
use std::sync::Arc;

/// Helper to create a URI from a simple filename
fn make_uri(filename: &str) -> Url {
    Url::parse(&format!("file:///{}", filename)).unwrap()
}

/// Helper to create ReferenceParams
#[allow(dead_code)]
fn make_reference_params(uri: Url, line: u32, character: u32) -> ReferenceParams {
    ReferenceParams {
        text_document_position: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri },
            position: Position::new(line, character),
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        context: ReferenceContext {
            include_declaration: true,
        },
    }
}

/// Helper to create RenameParams
#[allow(dead_code)]
fn make_rename_params(uri: Url, line: u32, character: u32, new_name: &str) -> RenameParams {
    RenameParams {
        text_document_position: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri },
            position: Position::new(line, character),
        },
        new_name: new_name.to_string(),
        work_done_progress_params: WorkDoneProgressParams::default(),
    }
}

fn make_reference_provider(documents: &[(Url, &str)]) -> ReferenceProvider {
    let cross_ref_manager = Arc::new(CrossReferenceManager::new());
    let document_manager = Arc::new(DocumentManager::new());

    for (uri, source) in documents {
        cross_ref_manager.update_component(uri.to_string(), source);
        document_manager.open(uri.clone(), "eventb".to_string(), 1, (*source).to_string());
    }

    let mut reference_provider = ReferenceProvider::new();
    reference_provider.set_cross_reference_manager(cross_ref_manager);
    reference_provider.set_document_manager(document_manager);
    reference_provider
}

#[test]
fn test_cross_reference_manager_tracks_sees() {
    let ctx_uri = make_uri("base_ctx.eventb");
    let mch_uri = make_uri("machine.eventb");

    let ctx_source = r#"
CONTEXT base_ctx
CONSTANTS
    max_value
END
"#;

    let mch_source = r#"
MACHINE machine
SEES base_ctx
VARIABLES
    count
END
"#;

    let manager = CrossReferenceManager::new();
    manager.update_component(ctx_uri.to_string(), ctx_source);
    manager.update_component(mch_uri.to_string(), mch_source);

    // Verify context is tracked
    let ctx_info = manager.get_component("base_ctx");
    assert!(ctx_info.is_some());
    let ctx_info = ctx_info.unwrap();
    assert_eq!(ctx_info.name, "base_ctx");
    assert_eq!(ctx_info.kind, ComponentKind::Context);

    // Verify machine is tracked and has SEES reference
    let mch_info = manager.get_component("machine");
    assert!(mch_info.is_some());
    let mch_info = mch_info.unwrap();
    assert_eq!(mch_info.name, "machine");
    assert_eq!(mch_info.kind, ComponentKind::Machine);
    assert!(mch_info.references.contains_key(&ReferenceKind::Sees));
    assert_eq!(
        mch_info.references.get(&ReferenceKind::Sees).unwrap(),
        &vec!["base_ctx".to_string()]
    );

    // Verify we can find the context URI by name
    let found_uri = manager.find_component_uri("base_ctx");
    assert_eq!(found_uri, Some(ctx_uri.to_string()));
}

#[test]
fn test_cross_reference_manager_tracks_refines() {
    let abstract_uri = make_uri("abstract_mch.eventb");
    let concrete_uri = make_uri("concrete_mch.eventb");

    let abstract_source = r#"
MACHINE abstract_mch
VARIABLES
    state
END
"#;

    let concrete_source = r#"
MACHINE concrete_mch
REFINES abstract_mch
VARIABLES
    state
    detail
END
"#;

    let manager = CrossReferenceManager::new();
    manager.update_component(abstract_uri.to_string(), abstract_source);
    manager.update_component(concrete_uri.to_string(), concrete_source);

    // Verify concrete machine has REFINES reference
    let concrete_info = manager.get_component("concrete_mch").unwrap();
    assert!(
        concrete_info
            .references
            .contains_key(&ReferenceKind::Refines)
    );
    assert_eq!(
        concrete_info
            .references
            .get(&ReferenceKind::Refines)
            .unwrap(),
        &vec!["abstract_mch".to_string()]
    );

    // Verify we can find referencing components
    let referencing =
        manager.find_referencing_components("abstract_mch", Some(ReferenceKind::Refines));
    assert_eq!(referencing.len(), 1);
    assert_eq!(referencing[0].name, "concrete_mch");
}

#[test]
fn test_cross_reference_manager_tracks_extends() {
    let base_uri = make_uri("base_ctx.eventb");
    let derived_uri = make_uri("derived_ctx.eventb");

    let base_source = r#"
CONTEXT base_ctx
SETS
    STATUS
END
"#;

    let derived_source = r#"
CONTEXT derived_ctx
EXTENDS base_ctx
CONSTANTS
    default_status
END
"#;

    let manager = CrossReferenceManager::new();
    manager.update_component(base_uri.to_string(), base_source);
    manager.update_component(derived_uri.to_string(), derived_source);

    // Verify derived context has EXTENDS reference
    let derived_info = manager.get_component("derived_ctx").unwrap();
    assert!(
        derived_info
            .references
            .contains_key(&ReferenceKind::Extends)
    );
    assert_eq!(
        derived_info
            .references
            .get(&ReferenceKind::Extends)
            .unwrap(),
        &vec!["base_ctx".to_string()]
    );
}

#[test]
fn test_cross_file_find_references_for_component() {
    // Setup: Create a context and multiple machines that SEE it
    let ctx_uri = make_uri("shared_ctx.eventb");
    let mch1_uri = make_uri("machine1.eventb");
    let mch2_uri = make_uri("machine2.eventb");

    let ctx_source = r#"
CONTEXT shared_ctx
CONSTANTS
    max_val
END
"#;

    let mch1_source = r#"
MACHINE machine1
SEES shared_ctx
VARIABLES
    count
END
"#;

    let mch2_source = r#"
MACHINE machine2
SEES shared_ctx
VARIABLES
    value
END
"#;

    // Create cross-reference manager and index all files
    let cross_ref_manager = Arc::new(CrossReferenceManager::new());
    cross_ref_manager.update_component(ctx_uri.to_string(), ctx_source);
    cross_ref_manager.update_component(mch1_uri.to_string(), mch1_source);
    cross_ref_manager.update_component(mch2_uri.to_string(), mch2_source);

    // Create document manager (though files aren't open)
    let document_manager = Arc::new(DocumentManager::new());

    // Create reference provider
    let mut reference_provider = ReferenceProvider::new();
    reference_provider.set_cross_reference_manager(Arc::clone(&cross_ref_manager));
    reference_provider.set_document_manager(Arc::clone(&document_manager));

    // Component names should be indexed for workspace reference searches.
    // We can't easily test the full find_references without real files,
    // but we can verify the manager is set up correctly
    assert!(cross_ref_manager.find_component_uri("shared_ctx").is_some());
    assert!(cross_ref_manager.find_component_uri("machine1").is_some());
    assert!(cross_ref_manager.find_component_uri("machine2").is_some());
}

#[test]
fn test_cross_file_references_for_seen_context_constant() {
    let ctx_uri = make_uri("C1.eventb");
    let mch_uri = make_uri("M1.eventb");

    let ctx_source = "CONTEXT C1\nCONSTANTS\n    Root\nAXIOMS\n    @RootType Root ∈ ℕ\nEND\n";
    let mch_source =
        "MACHINE M1\nSEES\n    C1\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x = Root\nEND\n";

    let reference_provider =
        make_reference_provider(&[(ctx_uri.clone(), ctx_source), (mch_uri.clone(), mch_source)]);

    let params = make_reference_params(mch_uri.clone(), 6, 14);
    let refs = reference_provider
        .find_references(&params, mch_source)
        .unwrap();

    assert!(refs.iter().any(|location| location.uri == ctx_uri));
    assert!(refs.iter().any(|location| location.uri == mch_uri));
    assert_eq!(refs.len(), 3);
}

#[test]
fn test_cross_file_references_from_context_constant_declaration_include_seen_machines() {
    let ctx_uri = make_uri("C1.eventb");
    let mch_uri = make_uri("M1.eventb");

    let ctx_source = "CONTEXT C1\nCONSTANTS\n    Root\nAXIOMS\n    @RootType Root ∈ ℕ\nEND\n";
    let mch_source = "MACHINE M1\nSEES C1\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x = Root\nEND\n";

    let reference_provider =
        make_reference_provider(&[(ctx_uri.clone(), ctx_source), (mch_uri.clone(), mch_source)]);

    let params = make_reference_params(ctx_uri.clone(), 2, 4);
    let refs = reference_provider
        .find_references(&params, ctx_source)
        .unwrap();

    assert!(refs.iter().any(|location| location.uri == ctx_uri));
    assert!(refs.iter().any(|location| location.uri == mch_uri));
    assert_eq!(refs.len(), 3);
}

#[test]
fn test_seen_context_constant_references_exclude_shadowing_machine_variable() {
    let ctx_uri = make_uri("C1.eventb");
    let mch_uri = make_uri("M1.eventb");

    let ctx_source = "CONTEXT C1\nCONSTANTS\n    x\nAXIOMS\n    @axm1 x ∈ ℕ\nEND\n";
    let mch_source = "MACHINE M1\nSEES C1\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ∈ ℕ\nEND\n";

    let reference_provider =
        make_reference_provider(&[(ctx_uri.clone(), ctx_source), (mch_uri.clone(), mch_source)]);

    let machine_params = make_reference_params(mch_uri.clone(), 3, 4);
    let machine_refs = reference_provider
        .find_references(&machine_params, mch_source)
        .unwrap();
    assert_eq!(machine_refs.len(), 2);
    assert!(machine_refs.iter().all(|location| location.uri == mch_uri));

    let context_params = make_reference_params(ctx_uri.clone(), 2, 4);
    let context_refs = reference_provider
        .find_references(&context_params, ctx_source)
        .unwrap();
    assert_eq!(context_refs.len(), 2);
    assert!(context_refs.iter().all(|location| location.uri == ctx_uri));
}

#[test]
fn test_local_symbol_named_like_component_uses_symbol_references() {
    let ctx_uri = make_uri("C1.eventb");
    let mch_uri = make_uri("M1.eventb");

    let ctx_source = "CONTEXT C1\nEND\n";
    let mch_source = "MACHINE M1\nSEES C1\nVARIABLES\n    C1\nINVARIANTS\n    @inv1 C1 ∈ ℕ\nEND\n";

    let reference_provider =
        make_reference_provider(&[(ctx_uri.clone(), ctx_source), (mch_uri.clone(), mch_source)]);

    let params = make_reference_params(mch_uri.clone(), 3, 4);
    let refs = reference_provider
        .find_references(&params, mch_source)
        .unwrap();

    assert_eq!(refs.len(), 2);
    assert!(refs.iter().all(|location| location.uri == mch_uri));
    assert!(
        refs.iter().all(|location| location.range.start.line != 1),
        "component dependency clause must not be counted as a variable reference"
    );
}

#[test]
fn test_event_parameter_references_are_event_scoped() {
    let mch_uri = make_uri("M1.eventb");

    let mch_source = "\
MACHINE M1
EVENTS
    EVENT first
    ANY
        x
    WHERE
        @grd1 x ∈ ℕ
    THEN
        skip
    END

    EVENT second
    ANY
        x
    WHERE
        @grd1 x ∈ ℕ
    THEN
        skip
    END
END
";

    let reference_provider = make_reference_provider(&[(mch_uri.clone(), mch_source)]);

    let params = make_reference_params(mch_uri.clone(), 4, 8);
    let refs = reference_provider
        .find_references(&params, mch_source)
        .unwrap();

    assert_eq!(refs.len(), 2);
    assert!(refs.iter().all(|location| location.uri == mch_uri));
    assert!(
        refs.iter().all(|location| location.range.start.line < 10),
        "references for first.x must not include second.x"
    );
}

#[test]
fn test_cross_file_references_for_extended_seen_context_constant() {
    let base_uri = make_uri("C0.eventb");
    let derived_uri = make_uri("C1.eventb");
    let mch_uri = make_uri("M1.eventb");

    let base_source =
        "CONTEXT C0\nCONSTANTS\n    max_value\nAXIOMS\n    @axm1 max_value ∈ ℕ\nEND\n";
    let derived_source = "CONTEXT C1\nEXTENDS C0\nEND\n";
    let mch_source =
        "MACHINE M1\nSEES C1\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x = max_value\nEND\n";

    let reference_provider = make_reference_provider(&[
        (base_uri.clone(), base_source),
        (derived_uri, derived_source),
        (mch_uri.clone(), mch_source),
    ]);

    let params = make_reference_params(mch_uri.clone(), 5, 14);
    let refs = reference_provider
        .find_references(&params, mch_source)
        .unwrap();

    assert!(refs.iter().any(|location| location.uri == base_uri));
    assert!(refs.iter().any(|location| location.uri == mch_uri));
    assert_eq!(refs.len(), 3);
}

#[test]
fn test_extended_context_constant_declaration_references_include_seen_child_context() {
    let base_uri = make_uri("C0.eventb");
    let derived_uri = make_uri("C1.eventb");
    let mch_uri = make_uri("M1.eventb");

    let base_source =
        "CONTEXT C0\nCONSTANTS\n    max_value\nAXIOMS\n    @axm1 max_value ∈ ℕ\nEND\n";
    let derived_source = "CONTEXT C1\nEXTENDS C0\nEND\n";
    let mch_source =
        "MACHINE M1\nSEES C1\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x = max_value\nEND\n";

    let reference_provider = make_reference_provider(&[
        (base_uri.clone(), base_source),
        (derived_uri, derived_source),
        (mch_uri.clone(), mch_source),
    ]);

    let params = make_reference_params(base_uri.clone(), 2, 4);
    let refs = reference_provider
        .find_references(&params, base_source)
        .unwrap();

    assert!(refs.iter().any(|location| location.uri == base_uri));
    assert!(refs.iter().any(|location| location.uri == mch_uri));
    assert_eq!(refs.len(), 3);
}

#[test]
fn test_abstract_machine_variable_references_exclude_shadowing_concrete_variable() {
    let abstract_uri = make_uri("M0.eventb");
    let concrete_uri = make_uri("M1.eventb");

    let abstract_source =
        "MACHINE M0\nVARIABLES\n    state\nINVARIANTS\n    @inv1 state ∈ ℕ\nEND\n";
    let concrete_source =
        "MACHINE M1\nREFINES M0\nVARIABLES\n    state\nINVARIANTS\n    @inv1 state ∈ ℕ\nEND\n";

    let reference_provider = make_reference_provider(&[
        (abstract_uri.clone(), abstract_source),
        (concrete_uri.clone(), concrete_source),
    ]);

    let abstract_params = make_reference_params(abstract_uri.clone(), 2, 4);
    let abstract_refs = reference_provider
        .find_references(&abstract_params, abstract_source)
        .unwrap();
    assert_eq!(abstract_refs.len(), 2);
    assert!(
        abstract_refs
            .iter()
            .all(|location| location.uri == abstract_uri)
    );

    let concrete_params = make_reference_params(concrete_uri.clone(), 3, 4);
    let concrete_refs = reference_provider
        .find_references(&concrete_params, concrete_source)
        .unwrap();
    assert_eq!(concrete_refs.len(), 2);
    assert!(
        concrete_refs
            .iter()
            .all(|location| location.uri == concrete_uri)
    );
}

#[test]
fn test_abstract_machine_variable_references_include_concrete_usages_when_not_shadowed() {
    let abstract_uri = make_uri("M0.eventb");
    let concrete_uri = make_uri("M1.eventb");

    let abstract_source =
        "MACHINE M0\nVARIABLES\n    state\nINVARIANTS\n    @inv1 state ∈ ℕ\nEND\n";
    let concrete_source = "MACHINE M1\nREFINES M0\nINVARIANTS\n    @inv1 state ∈ ℕ\nEND\n";

    let reference_provider = make_reference_provider(&[
        (abstract_uri.clone(), abstract_source),
        (concrete_uri.clone(), concrete_source),
    ]);

    let params = make_reference_params(abstract_uri.clone(), 2, 4);
    let refs = reference_provider
        .find_references(&params, abstract_source)
        .unwrap();

    assert!(refs.iter().any(|location| location.uri == abstract_uri));
    assert!(refs.iter().any(|location| location.uri == concrete_uri));
    assert_eq!(refs.len(), 3);
}

#[test]
fn test_cross_file_rename_component_detection() {
    // Setup: Create a context and a machine that SEES it
    let ctx_uri = make_uri("old_ctx.eventb");
    let mch_uri = make_uri("app_machine.eventb");

    let ctx_source = r#"
CONTEXT old_ctx
CONSTANTS
    value
END
"#;

    let mch_source = r#"
MACHINE app_machine
SEES old_ctx
VARIABLES
    state
END
"#;

    // Create cross-reference manager and index both files
    let cross_ref_manager = Arc::new(CrossReferenceManager::new());
    cross_ref_manager.update_component(ctx_uri.to_string(), ctx_source);
    cross_ref_manager.update_component(mch_uri.to_string(), mch_source);

    // Create rename provider
    let document_manager = Arc::new(DocumentManager::new());
    let mut rename_provider = RenameProvider::new();
    rename_provider.set_cross_reference_manager(Arc::clone(&cross_ref_manager));
    rename_provider.set_document_manager(Arc::clone(&document_manager));

    // Verify component is detected by the cross-ref manager
    assert!(cross_ref_manager.find_component_uri("old_ctx").is_some());
    assert!(
        cross_ref_manager
            .find_component_uri("app_machine")
            .is_some()
    );
}

#[test]
fn test_local_symbol_not_tracked_as_component() {
    let mch_uri = make_uri("machine.eventb");

    let mch_source = r#"
MACHINE machine
VARIABLES
    count
INVARIANTS
    @inv1 count ∈ ℕ
END
"#;

    let manager = CrossReferenceManager::new();
    manager.update_component(mch_uri.to_string(), mch_source);

    // Verify machine is tracked as a component
    assert!(manager.find_component_uri("machine").is_some());

    // Verify local variable is NOT tracked as a component
    assert!(manager.find_component_uri("count").is_none());
}

#[test]
fn test_circular_dependency_detection() {
    let ctx1_uri = make_uri("ctx1.eventb");
    let ctx2_uri = make_uri("ctx2.eventb");

    let ctx1_source = r#"
CONTEXT ctx1
EXTENDS ctx2
END
"#;

    let ctx2_source = r#"
CONTEXT ctx2
EXTENDS ctx1
END
"#;

    let manager = CrossReferenceManager::new();
    manager.update_component(ctx1_uri.to_string(), ctx1_source);
    manager.update_component(ctx2_uri.to_string(), ctx2_source);

    #[allow(deprecated)]
    let cycles = manager.detect_circular_dependencies();

    assert!(!cycles.is_empty(), "Should detect circular dependency");
    let has_both = cycles
        .iter()
        .any(|cycle| cycle.contains(&"ctx1".to_string()) && cycle.contains(&"ctx2".to_string()));
    assert!(has_both, "Cycle should contain both ctx1 and ctx2");
}

#[test]
fn test_multi_level_refinement_tracking() {
    let abstract_uri = make_uri("abstract.eventb");
    let middle_uri = make_uri("middle.eventb");
    let concrete_uri = make_uri("concrete.eventb");

    let abstract_source = r#"
MACHINE abstract
VARIABLES
    x
END
"#;

    let middle_source = r#"
MACHINE middle
REFINES abstract
VARIABLES
    x
    y
END
"#;

    let concrete_source = r#"
MACHINE concrete
REFINES middle
VARIABLES
    x
    y
    z
END
"#;

    let manager = CrossReferenceManager::new();
    manager.update_component(abstract_uri.to_string(), abstract_source);
    manager.update_component(middle_uri.to_string(), middle_source);
    manager.update_component(concrete_uri.to_string(), concrete_source);

    // Verify refinement chain
    let middle_info = manager.get_component("middle").unwrap();
    assert_eq!(
        middle_info.references.get(&ReferenceKind::Refines).unwrap(),
        &vec!["abstract".to_string()]
    );

    let concrete_info = manager.get_component("concrete").unwrap();
    assert_eq!(
        concrete_info
            .references
            .get(&ReferenceKind::Refines)
            .unwrap(),
        &vec!["middle".to_string()]
    );

    // Verify reverse lookup
    let middle_refiners =
        manager.find_referencing_components("middle", Some(ReferenceKind::Refines));
    assert_eq!(middle_refiners.len(), 1);
    assert_eq!(middle_refiners[0].name, "concrete");

    let abstract_refiners =
        manager.find_referencing_components("abstract", Some(ReferenceKind::Refines));
    assert_eq!(abstract_refiners.len(), 1);
    assert_eq!(abstract_refiners[0].name, "middle");
}

#[test]
fn test_component_with_multiple_sees() {
    let ctx1_uri = make_uri("ctx1.eventb");
    let ctx2_uri = make_uri("ctx2.eventb");
    let mch_uri = make_uri("machine.eventb");

    let ctx1_source = r#"
CONTEXT ctx1
CONSTANTS
    c1
END
"#;

    let ctx2_source = r#"
CONTEXT ctx2
CONSTANTS
    c2
END
"#;

    let mch_source = r#"
MACHINE machine
SEES ctx1 ctx2
VARIABLES
    v
END
"#;

    let manager = CrossReferenceManager::new();
    manager.update_component(ctx1_uri.to_string(), ctx1_source);
    manager.update_component(ctx2_uri.to_string(), ctx2_source);
    manager.update_component(mch_uri.to_string(), mch_source);

    // Verify machine SEES both contexts
    let mch_info = manager.get_component("machine").unwrap();
    let sees_refs = mch_info.references.get(&ReferenceKind::Sees).unwrap();
    assert_eq!(sees_refs.len(), 2);
    assert!(sees_refs.contains(&"ctx1".to_string()));
    assert!(sees_refs.contains(&"ctx2".to_string()));

    // Verify both contexts can be found
    assert!(manager.find_component_uri("ctx1").is_some());
    assert!(manager.find_component_uri("ctx2").is_some());
}

#[test]
fn test_component_removal() {
    let ctx_uri = make_uri("temp_ctx.eventb");

    let ctx_source = r#"
CONTEXT temp_ctx
END
"#;

    let manager = CrossReferenceManager::new();
    manager.update_component(ctx_uri.to_string(), ctx_source);

    // Verify component is tracked
    assert!(manager.find_component_uri("temp_ctx").is_some());

    // Remove component
    manager.remove_component(ctx_uri.as_ref());

    // Verify component is no longer tracked
    assert!(manager.find_component_uri("temp_ctx").is_none());
}

#[test]
fn test_component_name_update() {
    let uri = make_uri("component.eventb");

    let old_source = r#"
CONTEXT old_name
END
"#;

    let new_source = r#"
CONTEXT new_name
END
"#;

    let manager = CrossReferenceManager::new();

    // Add component with old name
    manager.update_component(uri.to_string(), old_source);
    assert!(manager.find_component_uri("old_name").is_some());

    // Update to new name
    manager.update_component(uri.to_string(), new_source);
    assert!(manager.find_component_uri("new_name").is_some());
    assert!(manager.find_component_uri("old_name").is_none());
}

#[test]
fn test_workspace_index_consistency() {
    let ctx_uri = make_uri("ctx.eventb");
    let mch_uri = make_uri("mch.eventb");

    let ctx_source = r#"
CONTEXT ctx
END
"#;

    let mch_source = r#"
MACHINE mch
SEES ctx
END
"#;

    let manager = CrossReferenceManager::new();
    manager.update_component(ctx_uri.to_string(), ctx_source);
    manager.update_component(mch_uri.to_string(), mch_source);

    // Verify both components are in the index
    let all_names = manager.all_component_names();
    assert_eq!(all_names.len(), 2);
    assert!(all_names.contains(&"ctx".to_string()));
    assert!(all_names.contains(&"mch".to_string()));

    // Verify URI mapping is consistent
    assert_eq!(
        manager.get_component_name(ctx_uri.as_ref()),
        Some("ctx".to_string())
    );
    assert_eq!(
        manager.get_component_name(mch_uri.as_ref()),
        Some("mch".to_string())
    );
}

/// Issue #84 — find-references stays consistent with go-to-definition on an
/// event's `extends`/`refines` target. Clicking the *target* (which names the
/// abstract event, even when the refined event keeps the name) resolves
/// cross-file to the abstract event; clicking the event's *own* name stays on
/// the local event. Before the target span was honoured, both clicks resolved
/// to the local event.
#[test]
fn refines_target_references_resolve_to_the_abstract_event() {
    let abs_uri = make_uri("abstract.eventb");
    let con_uri = make_uri("concrete.eventb");
    let abs = "MACHINE abstract\nVARIABLES\n    state\nEVENTS\n    EVENT step\n    THEN\n        state ≔ state\n    END\nEND";
    let con = "MACHINE concrete\nREFINES abstract\nVARIABLES\n    state\nEVENTS\n    EVENT step extends step\n    THEN\n        state ≔ state\n    END\nEND";
    let provider = make_reference_provider(&[(abs_uri.clone(), abs), (con_uri.clone(), con)]);

    // The `extends` target (second `step`, char 24) resolves to the abstract
    // event's declaration, not the local event.
    let target = provider
        .find_references(&make_reference_params(con_uri.clone(), 5, 24), con)
        .expect("references resolve");
    assert_eq!(target.len(), 1, "{target:?}");
    assert_eq!(target[0].uri, abs_uri);
    assert_eq!(target[0].range.start, Position::new(4, 10));

    // The event's own name (first `step`, char 11) stays on the local event.
    let own = provider
        .find_references(&make_reference_params(con_uri.clone(), 5, 11), con)
        .expect("references resolve");
    assert!(
        !own.is_empty() && own.iter().all(|r| r.uri == con_uri),
        "own-name references stay local: {own:?}"
    );
}
