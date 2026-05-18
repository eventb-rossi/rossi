//! Integration tests for code actions

use lsp_types::{
    CodeActionContext, CodeActionKind, CodeActionOrCommand, CodeActionParams, Position, Range,
    TextDocumentIdentifier, Url, WorkDoneProgressParams,
};
use rossi_lsp::code_actions::CodeActionProvider;

fn create_test_params(uri: &str, range: Range) -> CodeActionParams {
    CodeActionParams {
        text_document: TextDocumentIdentifier {
            uri: Url::parse(uri).unwrap(),
        },
        range,
        context: CodeActionContext {
            diagnostics: vec![],
            only: None,
            trigger_kind: None,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: Default::default(),
    }
}

#[test]
fn test_convert_selection_to_unicode() {
    let provider = CodeActionProvider::new();
    let text = "MACHINE test\nVARIABLES x\nINVARIANTS\n  @inv1 x : NAT /\\ x <= 10\nEND";

    // Select just "x : NAT /\ x <= 10"
    let params = create_test_params(
        "file:///test.eventb",
        Range {
            start: Position::new(3, 8),
            end: Position::new(3, 26),
        },
    );

    let actions = provider.provide_code_actions(&params, text);

    assert!(actions.is_some());
    let actions = actions.unwrap();

    // Should have selection conversion actions
    let selection_actions: Vec<_> = actions
        .iter()
        .filter(|action| {
            if let CodeActionOrCommand::CodeAction(action) = action {
                action.title.contains("selection")
            } else {
                false
            }
        })
        .collect();

    assert!(
        !selection_actions.is_empty(),
        "Should have selection conversion actions"
    );

    // Check that selection action is marked as preferred
    let has_preferred = actions.iter().any(|action| {
        if let CodeActionOrCommand::CodeAction(action) = action {
            action.is_preferred == Some(true)
        } else {
            false
        }
    });

    assert!(
        has_preferred,
        "Selection actions should be marked as preferred"
    );
}

#[test]
fn test_no_actions_for_plain_text() {
    let provider = CodeActionProvider::new();
    let text = "This is just plain text without any operators";
    let params = create_test_params(
        "file:///test.txt",
        Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        },
    );

    let actions = provider.provide_code_actions(&params, text);

    // Should have no actions for plain text
    assert!(
        actions.is_none() || actions.unwrap().is_empty(),
        "Should have no actions for plain text"
    );
}

#[test]
fn test_code_action_kinds() {
    let provider = CodeActionProvider::new();
    let text = "x /\\ y => z \\/ w";
    let params = create_test_params(
        "file:///test.eventb",
        Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        },
    );

    let actions = provider.provide_code_actions(&params, text);

    assert!(actions.is_some());
    let actions = actions.unwrap();

    // Check that all actions have the correct kind (REFACTOR)
    for action in actions {
        if let CodeActionOrCommand::CodeAction(action) = action {
            assert!(
                action.kind == Some(CodeActionKind::REFACTOR)
                    || action.kind == Some(CodeActionKind::REFACTOR_EXTRACT),
                "Action kind should be REFACTOR or REFACTOR_EXTRACT"
            );
        }
    }
}

#[test]
fn test_extract_constant_action_numeric_literal() {
    let provider = CodeActionProvider::new();
    let text = "MACHINE test\nVARIABLES x\nINVARIANTS\n  @inv1 x <= 42\nEND";

    // Select the numeric literal "42"
    let params = create_test_params(
        "file:///test.eventb",
        Range {
            start: Position::new(3, 13),
            end: Position::new(3, 15),
        },
    );

    let actions = provider.provide_code_actions(&params, text);

    assert!(actions.is_some());
    let actions = actions.unwrap();

    // Should have extract constant action
    let has_extract = actions.iter().any(|action| {
        if let CodeActionOrCommand::CodeAction(action) = action {
            action.title.contains("Extract constant")
        } else {
            false
        }
    });

    assert!(
        has_extract,
        "Should have extract constant action for numeric literal"
    );
}

#[test]
fn test_multiple_actions_returned() {
    let provider = CodeActionProvider::new();
    // Text with both ASCII and Unicode operators
    let text = "x : NAT /\\ y ∈ ℤ";
    let params = create_test_params(
        "file:///test.eventb",
        Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        },
    );

    let actions = provider.provide_code_actions(&params, text);

    assert!(actions.is_some());
    let actions = actions.unwrap();

    // Should have both conversion actions (to Unicode and to ASCII)
    assert!(actions.len() >= 2, "Should have multiple actions available");
}

#[test]
fn test_operator_conversion_correctness() {
    let provider = CodeActionProvider::new();

    // Test ASCII to Unicode
    let ascii_text = "x /\\ y \\/ z => w";
    let unicode_result = provider
        .provide_code_actions(
            &create_test_params(
                "file:///test.eventb",
                Range {
                    start: Position::new(0, 0),
                    end: Position::new(0, 0),
                },
            ),
            ascii_text,
        )
        .unwrap();

    // Verify that the conversion action exists
    let has_action = unicode_result.iter().any(|action| {
        if let CodeActionOrCommand::CodeAction(action) = action {
            action.title.contains("Unicode") && action.edit.is_some()
        } else {
            false
        }
    });

    assert!(has_action, "Should have Unicode conversion with edit");
}

#[test]
fn test_complex_expression_operators() {
    let provider = CodeActionProvider::new();
    let text = "!(x).(x : S => #(y).(y : T /\\ x |-> y : R))";
    let params = create_test_params(
        "file:///test.eventb",
        Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        },
    );

    let actions = provider.provide_code_actions(&params, text);

    assert!(actions.is_some());
    let actions = actions.unwrap();

    // Should detect quantifiers and other operators
    assert!(
        !actions.is_empty(),
        "Should detect operators in complex expression"
    );
}

#[test]
fn test_set_operators() {
    let provider = CodeActionProvider::new();
    let text = "S <: T /\\ x : S \\/ T";
    let params = create_test_params(
        "file:///test.eventb",
        Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        },
    );

    let actions = provider.provide_code_actions(&params, text);

    assert!(actions.is_some());
    assert!(!actions.unwrap().is_empty(), "Should detect set operators");
}

#[test]
fn test_relation_operators() {
    let provider = CodeActionProvider::new();
    let text = "r : S <-> T /\\ f : S >-> T";
    let params = create_test_params(
        "file:///test.eventb",
        Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        },
    );

    let actions = provider.provide_code_actions(&params, text);

    assert!(actions.is_some());
    assert!(
        !actions.unwrap().is_empty(),
        "Should detect relation operators"
    );
}

#[test]
fn test_add_missing_clause_machine() {
    let provider = CodeActionProvider::new();
    let text = "MACHINE test\nVARIABLES x\nEND";
    let params = create_test_params(
        "file:///test.eventb",
        Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        },
    );

    let actions = provider.provide_code_actions(&params, text);

    assert!(actions.is_some());
    let actions = actions.unwrap();

    // Should have action to add INVARIANTS
    let has_invariants = actions.iter().any(|action| {
        if let CodeActionOrCommand::CodeAction(action) = action {
            action.title.contains("INVARIANTS")
        } else {
            false
        }
    });

    assert!(has_invariants, "Should suggest adding INVARIANTS clause");
}

#[test]
fn test_add_missing_clause_context() {
    let provider = CodeActionProvider::new();
    let text = "CONTEXT test\nSETS S\nEND";
    let params = create_test_params(
        "file:///test.eventb",
        Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        },
    );

    let actions = provider.provide_code_actions(&params, text);

    assert!(actions.is_some());
    let actions = actions.unwrap();

    // Should have actions to add AXIOMS and CONSTANTS
    let has_axioms = actions.iter().any(|action| {
        if let CodeActionOrCommand::CodeAction(action) = action {
            action.title.contains("AXIOMS")
        } else {
            false
        }
    });

    let has_constants = actions.iter().any(|action| {
        if let CodeActionOrCommand::CodeAction(action) = action {
            action.title.contains("CONSTANTS")
        } else {
            false
        }
    });

    assert!(has_axioms, "Should suggest adding AXIOMS clause");
    assert!(has_constants, "Should suggest adding CONSTANTS clause");
}

#[test]
fn test_sort_variables() {
    let provider = CodeActionProvider::new();
    let text = "MACHINE test\nVARIABLES\n    z\n    a\n    m\nINVARIANTS\nEND";
    let params = create_test_params(
        "file:///test.eventb",
        Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        },
    );

    let actions = provider.provide_code_actions(&params, text);

    assert!(actions.is_some());
    let actions = actions.unwrap();

    // Should have action to sort variables
    let has_sort = actions.iter().any(|action| {
        if let CodeActionOrCommand::CodeAction(action) = action {
            action.title.contains("Sort") && action.title.contains("variables")
        } else {
            false
        }
    });

    assert!(has_sort, "Should suggest sorting variables");
}

#[test]
fn test_sort_constants() {
    let provider = CodeActionProvider::new();
    let text = "CONTEXT test\nCONSTANTS\n    c_z\n    c_a\n    c_m\nAXIOMS\nEND";
    let params = create_test_params(
        "file:///test.eventb",
        Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        },
    );

    let actions = provider.provide_code_actions(&params, text);

    assert!(actions.is_some());
    let actions = actions.unwrap();

    // Should have action to sort constants
    let has_sort = actions.iter().any(|action| {
        if let CodeActionOrCommand::CodeAction(action) = action {
            action.title.contains("Sort") && action.title.contains("constants")
        } else {
            false
        }
    });

    assert!(has_sort, "Should suggest sorting constants");
}

#[test]
fn test_no_sort_action_when_already_sorted() {
    let provider = CodeActionProvider::new();
    let text = "MACHINE test\nVARIABLES\n    a\n    m\n    z\nINVARIANTS\nEND";
    let params = create_test_params(
        "file:///test.eventb",
        Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        },
    );

    let actions = provider.provide_code_actions(&params, text);

    if let Some(actions) = actions {
        // Should NOT have action to sort variables (already sorted)
        let has_sort_vars = actions.iter().any(|action| {
            if let CodeActionOrCommand::CodeAction(action) = action {
                action.title.contains("Sort") && action.title.contains("variables")
            } else {
                false
            }
        });

        assert!(
            !has_sort_vars,
            "Should not suggest sorting when already sorted"
        );
    }
}

#[test]
fn test_rename_event_hint() {
    let provider = CodeActionProvider::new();
    let text = "MACHINE test\nEVENTS\n    EVENT evt1\n    END\nEND";
    let params = create_test_params(
        "file:///test.eventb",
        Range {
            start: Position::new(2, 0),
            end: Position::new(2, 0),
        },
    );

    let actions = provider.provide_code_actions(&params, text);

    assert!(actions.is_some());
    let actions = actions.unwrap();

    // Should have rename event hint
    let has_rename = actions.iter().any(|action| {
        if let CodeActionOrCommand::CodeAction(action) = action {
            action.title.contains("Rename event")
        } else {
            false
        }
    });

    assert!(has_rename, "Should suggest rename event hint");
}

#[test]
fn test_roundtrip_ascii_unicode_ascii() {
    let provider = CodeActionProvider::new();

    // Basic logical/set operators
    let ascii_text = "x : NAT & x <= 10 => x /= 0";
    let unicode = provider.convert_to_unicode(ascii_text);
    let back = provider.convert_to_ascii(&unicode);
    assert_eq!(back, ascii_text, "Roundtrip failed for basic operators");

    // Function types
    let ascii_text2 = "f : S --> T & g : S >-> T & h : S ->> T";
    let unicode2 = provider.convert_to_unicode(ascii_text2);
    let back2 = provider.convert_to_ascii(&unicode2);
    assert_eq!(back2, ascii_text2, "Roundtrip failed for function types");

    // Set operators with intersection/union
    let ascii_text3 = "S <: T /\\ x : S \\/ T";
    let unicode3 = provider.convert_to_unicode(ascii_text3);
    let back3 = provider.convert_to_ascii(&unicode3);
    assert_eq!(back3, ascii_text3, "Roundtrip failed for set operators");
}

#[test]
fn test_new_operator_mappings() {
    let provider = CodeActionProvider::new();

    // Verify corrected mappings: ¬ -> not (not !)
    assert_eq!(provider.convert_to_ascii("¬ P"), "not P");

    // × -> ** (not *)
    assert_eq!(provider.convert_to_ascii("S × T"), "S ** T");

    // → -> --> (not ->)
    assert_eq!(provider.convert_to_ascii("f → T"), "f --> T");

    // ∘ -> circ (not ;)
    assert_eq!(provider.convert_to_ascii("f ∘ g"), "f circ g");

    // ◁ and ▷ (correct Unicode symbols)
    assert_eq!(provider.convert_to_ascii("S ◁ r"), "S <| r");
    assert_eq!(provider.convert_to_ascii("r ▷ S"), "r |> S");

    // New operators: ∅, ⁻¹, ⋃, ⋂, ·, λ
    assert_eq!(provider.convert_to_ascii("∅"), "{}");
    assert_eq!(provider.convert_to_ascii("r⁻¹"), "r~");
    assert_eq!(provider.convert_to_ascii("⋃"), "UNION");
    assert_eq!(provider.convert_to_ascii("⋂"), "INTER");
    assert_eq!(provider.convert_to_ascii("·"), ".");
    assert_eq!(provider.convert_to_ascii("λ"), "%");

    // ⦂ -> oftype (not :|)
    assert_eq!(provider.convert_to_ascii("x⦂T"), "xoftypeT");
}

#[test]
fn test_diagnostic_based_action() {
    use lsp_types::{Diagnostic, DiagnosticSeverity};

    let provider = CodeActionProvider::new();
    let text = "MACHINE test\nVARIABLES x";

    // Create a diagnostic for missing END
    let diagnostic = Diagnostic {
        range: Range {
            start: Position::new(1, 0),
            end: Position::new(1, 10),
        },
        severity: Some(DiagnosticSeverity::ERROR),
        code: None,
        source: Some("rossi".to_string()),
        message: "Expected END keyword".to_string(),
        related_information: None,
        tags: None,
        code_description: None,
        data: None,
    };

    let mut params = create_test_params(
        "file:///test.eventb",
        Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        },
    );
    params.context.diagnostics = vec![diagnostic];

    let actions = provider.provide_code_actions(&params, text);

    assert!(actions.is_some());
    let actions = actions.unwrap();

    // Should have action to add missing END
    let has_end = actions.iter().any(|action| {
        if let CodeActionOrCommand::CodeAction(action) = action {
            action.title.contains("END") && action.kind == Some(CodeActionKind::QUICKFIX)
        } else {
            false
        }
    });

    assert!(has_end, "Should suggest adding missing END");
}
