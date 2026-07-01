//! Integration tests for code actions

use eventb_lsp::code_actions::CodeActionProvider;
use eventb_lsp::lsp_types::{
    CodeActionContext, CodeActionKind, CodeActionOrCommand, CodeActionParams, Position, Range,
    TextDocumentIdentifier, Url, WorkDoneProgressParams,
};

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

    // New operators: ∅, ∼, ⋃, ⋂, ·, λ
    assert_eq!(provider.convert_to_ascii("∅"), "{}");
    assert_eq!(provider.convert_to_ascii("r∼"), "r~");
    assert_eq!(provider.convert_to_ascii("⋃"), "UNION");
    assert_eq!(provider.convert_to_ascii("⋂"), "INTER");
    assert_eq!(provider.convert_to_ascii("·"), ".");
    assert_eq!(provider.convert_to_ascii("λ"), "%");

    // ⦂ -> oftype (not :|)
    assert_eq!(provider.convert_to_ascii("x⦂T"), "xoftypeT");
}

#[test]
fn test_diagnostic_based_action() {
    use eventb_lsp::lsp_types::{Diagnostic, DiagnosticSeverity};

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

#[test]
fn test_add_missing_end_offered_for_eof_diagnostic() {
    // A missing END is reported by the parser one line PAST the last line
    // (pest's end-of-input position); the quick fix must still be offered.
    use eventb_lsp::lsp_types::Diagnostic;

    let provider = CodeActionProvider::new();
    let text = "MACHINE m\nVARIABLES\n    x\n"; // 3 lines, no END
    let eof = Range {
        start: Position::new(3, 0),
        end: Position::new(3, 0),
    };
    let mut params = create_test_params("file:///test.eventb", eof);
    params.context.diagnostics = vec![Diagnostic {
        range: eof,
        message: "Pest parsing error: expected machine_clause or END".to_string(),
        ..Default::default()
    }];

    let actions = provider
        .provide_code_actions(&params, text)
        .unwrap_or_default();

    assert!(
        actions.iter().any(|a| matches!(
            a,
            CodeActionOrCommand::CodeAction(action) if action.title.contains("Add missing END")
        )),
        "the Add-missing-END quick fix must be offered for an EOF diagnostic, got {actions:?}"
    );
}

/// Build a CodeActionParams carrying a single EB026 diagnostic whose range is
/// `op_range` (the becomes operator), the shape the diagnostics provider emits.
fn eb026_params(uri: &str, op_range: Range) -> CodeActionParams {
    use eventb_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};
    let mut params = create_test_params(uri, op_range);
    params.context.diagnostics = vec![Diagnostic {
        range: op_range,
        severity: Some(DiagnosticSeverity::ERROR),
        code: Some(NumberOrString::String("EB026".to_string())),
        message: "assignment operator `:=` used where a predicate is required".to_string(),
        ..Default::default()
    }];
    params
}

#[test]
fn eb026_offers_equality_swap_for_becomes_equal() {
    // `@inv1 x := 5` → offer replacing `:=` with `=`, attached to the diagnostic.
    let provider = CodeActionProvider::new();
    let text = "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x := 5\nEND\n";
    let op = Range {
        start: Position::new(4, 12),
        end: Position::new(4, 14),
    };
    let params = eb026_params("file:///m.eventb", op);
    let actions = provider
        .provide_code_actions(&params, text)
        .unwrap_or_default();

    let fix = actions.iter().find_map(|a| match a {
        CodeActionOrCommand::CodeAction(action) if action.title.contains("Replace") => Some(action),
        _ => None,
    });
    let fix = fix.expect("a Replace quick fix must be offered for EB026");
    assert_eq!(fix.title, "Replace `:=` with `=`");
    assert_eq!(fix.kind, Some(CodeActionKind::QUICKFIX));
    assert!(fix.diagnostics.is_some(), "fix attaches to the diagnostic");
    let edit = &fix.edit.as_ref().unwrap().changes.as_ref().unwrap()
        [&Url::parse("file:///m.eventb").unwrap()][0];
    assert_eq!(edit.new_text, "=");
    assert_eq!(edit.range, op, "edit replaces exactly the operator");
}

#[test]
fn eb026_offers_membership_swap_for_becomes_in() {
    // `@inv1 x :∈ ℕ` → offer replacing `:∈` with `∈`.
    let provider = CodeActionProvider::new();
    let text = "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x :∈ ℕ\nEND\n";
    let op = Range {
        start: Position::new(4, 12),
        end: Position::new(4, 14),
    };
    let params = eb026_params("file:///m.eventb", op);
    let actions = provider
        .provide_code_actions(&params, text)
        .unwrap_or_default();

    assert!(
        actions.iter().any(|a| matches!(
            a,
            CodeActionOrCommand::CodeAction(action) if action.title == "Replace `:∈` with `∈`"
        )),
        "the `:∈` → `∈` quick fix must be offered, got {actions:?}"
    );
}

#[test]
fn eb026_offers_no_swap_for_becomes_such_that() {
    // `@inv1 x :| x' > 0` (becomes-such-that) has a predicate RHS with no
    // single-token fix, so no quick fix is offered — the diagnostic still stands.
    let provider = CodeActionProvider::new();
    let text = "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x :| x' > 0\nEND\n";
    let op = Range {
        start: Position::new(4, 12),
        end: Position::new(4, 14),
    };
    let params = eb026_params("file:///m.eventb", op);
    let actions = provider
        .provide_code_actions(&params, text)
        .unwrap_or_default();

    assert!(
        !actions.iter().any(|a| matches!(
            a,
            CodeActionOrCommand::CodeAction(action) if action.title.starts_with("Replace")
        )),
        "no Replace quick fix for `:|`, got {actions:?}"
    );
}

#[test]
fn test_add_missing_end_not_offered_when_terminated() {
    // A complete MACHINE … END whose only problem is a typo deep inside a
    // predicate must NOT offer "Add missing END": the component is already
    // terminated. The trigger is structural, not the diagnostic's prose.
    use eventb_lsp::lsp_types::Diagnostic;

    let provider = CodeActionProvider::new();
    let text = "MACHINE m\nINVARIANTS\n    @inv1 x ∈ ℕ sdfsdf y\nEND\n";
    let range = Range {
        start: Position::new(2, 18),
        end: Position::new(2, 24),
    };
    let mut params = create_test_params("file:///test.eventb", range);
    params.context.diagnostics = vec![Diagnostic {
        range,
        message: "Syntax error: expected ∈, ∉, …".to_string(),
        ..Default::default()
    }];

    let actions = provider
        .provide_code_actions(&params, text)
        .unwrap_or_default();

    assert!(
        !actions.iter().any(|a| matches!(
            a,
            CodeActionOrCommand::CodeAction(action) if action.title.contains("Add missing END")
        )),
        "Add-missing-END must not be offered when END is present, got {actions:?}"
    );
}

#[test]
fn test_operator_conversion_leaves_comments_alone() {
    let provider = CodeActionProvider::new();
    let text = "MACHINE test\nVARIABLES x\nINVARIANTS\n  @inv1 x : NAT & x <= 10 // prose: x <= 10 and & stay ASCII\nEND";

    let converted = provider.convert_to_unicode(text);

    // Code is converted...
    assert!(converted.contains("x ∈ ℕ ∧ x ≤ 10 //"));
    // ...comment prose is untouched.
    assert!(converted.contains("// prose: x <= 10 and & stay ASCII"));
}

#[test]
fn test_selection_conversion_preserves_comment_opened_before_selection() {
    // A selection that begins INSIDE a `/* */` block comment (the `/*` is
    // outside the selection) must not have the comment prose's operator
    // spellings rewritten — only the code after the comment closes.
    let provider = CodeActionProvider::new();
    // Line 2: `  @inv1 a /* note <= keep */ b <= c`
    //          col 13 is `note` (inside the comment); col 35 is end of line.
    let text = "MACHINE m\nINVARIANTS\n  @inv1 a /* note <= keep */ b <= c\nEND";

    let params = create_test_params(
        "file:///test.eventb",
        Range {
            start: Position::new(2, 13),
            end: Position::new(2, 35),
        },
    );
    let actions = provider.provide_code_actions(&params, text).unwrap();

    let edit_text = actions
        .iter()
        .find_map(|a| match a {
            CodeActionOrCommand::CodeAction(action)
                if action.title == "Convert selection to Unicode" =>
            {
                let changes = action.edit.as_ref()?.changes.as_ref()?;
                Some(changes.values().next()?[0].new_text.clone())
            }
            _ => None,
        })
        .expect("expected a 'Convert selection to Unicode' action");

    // `<=` inside the comment stays ASCII; `<=` in the trailing code converts.
    assert!(
        edit_text.contains("note <= keep"),
        "comment prose must be untouched, got: {edit_text:?}"
    );
    assert!(
        edit_text.contains("b ≤ c"),
        "trailing code must be converted, got: {edit_text:?}"
    );
}

#[test]
fn test_ascii_operators_in_comments_do_not_offer_conversion() {
    let provider = CodeActionProvider::new();
    // The only ASCII operator spellings are inside the comment.
    let text = "MACHINE test\nVARIABLES x\nINVARIANTS\n  @inv1 x ∈ ℕ // note: x <= 10\nEND";

    let params = create_test_params(
        "file:///test.eventb",
        Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        },
    );
    let actions = provider.provide_code_actions(&params, text);

    let offers_unicode_conversion = actions.iter().flatten().any(|action| {
        if let CodeActionOrCommand::CodeAction(action) = action {
            action.title.contains("Unicode")
        } else {
            false
        }
    });
    assert!(
        !offers_unicode_conversion,
        "ASCII operators inside comments must not trigger the conversion action"
    );
}
