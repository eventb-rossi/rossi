//! Integration tests for code actions

use eventb_lsp::code_actions::{CodeActionProvider, FIX_ALL_KIND};
use eventb_lsp::diagnostics::ASCII_OPERATOR_CODE;
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

    let actions = provider.provide_code_actions(&params, text, true, false);

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

    let actions = provider.provide_code_actions(&params, text, true, false);

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

    let actions = provider.provide_code_actions(&params, text, true, false);

    assert!(actions.is_some());
    let actions = actions.unwrap();

    // Check that all actions have the correct kind (REFACTOR), apart from
    // the on-save operator normalization, which is a source action
    for action in actions {
        if let CodeActionOrCommand::CodeAction(action) = action {
            assert!(
                action.kind == Some(CodeActionKind::REFACTOR)
                    || action.kind == Some(CodeActionKind::REFACTOR_EXTRACT)
                    || action.kind == Some(FIX_ALL_KIND),
                "Action kind should be REFACTOR, REFACTOR_EXTRACT, or the fix-all source kind"
            );
        }
    }
}

/// The `source.fixAll.rossi` action among `actions`, if offered.
fn fix_all_action(actions: &[CodeActionOrCommand]) -> Option<&eventb_lsp::lsp_types::CodeAction> {
    actions.iter().find_map(|a| match a {
        CodeActionOrCommand::CodeAction(action) if action.kind.as_ref() == Some(&FIX_ALL_KIND) => {
            Some(action)
        }
        _ => None,
    })
}

#[test]
fn fix_all_normalizes_operators_to_the_convention() {
    // One whole-document edit that rewrites the operator spellings toward
    // `useUnicode` and nothing else — layout, comment prose, and label text
    // are untouched.
    let provider = CodeActionProvider::new();
    let uri = Url::parse("file:///m.eventb").unwrap();
    let cases = [
        (
            true,
            "MACHINE m\nINVARIANTS\n  @inv-1 x : NAT & x <= 10 // x <= 10\nEND\n",
            "Normalize operators to Unicode",
            "MACHINE m\nINVARIANTS\n  @inv-1 x ∈ ℕ ∧ x ≤ 10 // x <= 10\nEND\n",
        ),
        (
            false,
            "@inv1 x ∈ ℕ ∧ x ≤ 10",
            "Normalize operators to ASCII",
            "@inv1 x : NAT & x <= 10",
        ),
    ];
    for (use_unicode, text, title, normalized) in cases {
        let params = create_test_params(
            uri.as_str(),
            Range {
                start: Position::new(0, 0),
                end: Position::new(0, 0),
            },
        );
        let actions = provider
            .provide_code_actions(&params, text, use_unicode, false)
            .unwrap_or_default();

        let action = fix_all_action(&actions).expect("the fix-all source action must be offered");
        assert_eq!(action.title, title);
        let edits = &action.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri];
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].range.start, Position::new(0, 0));
        assert_eq!(edits[0].new_text, normalized);
    }
}

#[test]
fn fix_all_not_offered_when_already_in_the_convention() {
    // Running the action on save must be a no-op for a conforming document:
    // no action, hence no edit and no dirty buffer. ASCII spellings inside a
    // comment do not count.
    let provider = CodeActionProvider::new();
    let text = "@inv1 x ∈ ℕ ∧ x ≤ 10 // x <= 10";
    let params = create_test_params(
        "file:///m.eventb",
        Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        },
    );
    let actions = provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();

    assert!(
        fix_all_action(&actions).is_none(),
        "no fix-all action for a document already in the convention, got {actions:?}"
    );
}

#[test]
fn fix_all_honours_the_only_filter() {
    // `editor.codeActionsOnSave` requests exactly the configured kind; a
    // parent kind (`source`, `source.fixAll`) admits it too, while a request
    // for other kinds — or a kind that merely shares a prefix — does not.
    // Whatever the filter, nothing outside it comes back: a client that
    // applies every returned edit on save must get the fix-all alone.
    let provider = CodeActionProvider::new();
    let text = "MACHINE m\nVARIABLES x\nINVARIANTS\n  @inv1 x : NAT & x <= 10\nEND\n";
    let cases = [
        ("source.fixAll.rossi", true),
        ("source.fixAll", true),
        ("source", true),
        ("quickfix", false),
        ("source.fixAllElse", false),
    ];
    for (only, expected) in cases {
        let mut params = create_test_params(
            "file:///m.eventb",
            Range {
                start: Position::new(0, 0),
                end: Position::new(0, 0),
            },
        );
        params.context.only = Some(vec![CodeActionKind::new(only)]);
        let actions = provider
            .provide_code_actions(&params, text, true, false)
            .unwrap_or_default();
        assert_eq!(
            fix_all_action(&actions).is_some(),
            expected,
            "only = [{only:?}]"
        );
        assert!(
            actions.iter().all(|a| matches!(
                a,
                CodeActionOrCommand::CodeAction(action)
                    if action.kind.as_ref().is_some_and(|kind| kind.as_str().starts_with(only))
            )),
            "only = [{only:?}] must filter the whole response, got {actions:?}"
        );
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

    let actions = provider.provide_code_actions(&params, text, true, false);

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
fn test_operator_detection_offers_conversion_actions() {
    let provider = CodeActionProvider::new();
    // (case, text, min_actions, required_title): a Some(required_title) row
    // additionally demands one action whose title contains it and that
    // carries a workspace edit.
    let cases: [(&str, &str, usize, Option<&str>); 5] = [
        (
            "mixed ascii and unicode operators",
            "x : NAT /\\ y ∈ ℤ",
            2,
            None,
        ),
        (
            "quantifiers in complex expression",
            "!(x).(x : S => #(y).(y : T /\\ x |-> y : R))",
            1,
            None,
        ),
        ("set operators", "S <: T /\\ x : S \\/ T", 1, None),
        ("relation operators", "r : S <-> T /\\ f : S >-> T", 1, None),
        (
            "ascii to unicode conversion",
            "x /\\ y \\/ z => w",
            1,
            Some("Unicode"),
        ),
    ];

    for (case, text, min_actions, required_title) in cases {
        let params = create_test_params(
            "file:///test.eventb",
            Range {
                start: Position::new(0, 0),
                end: Position::new(0, 0),
            },
        );
        let actions = provider
            .provide_code_actions(&params, text, true, false)
            .unwrap_or_default();

        assert!(
            actions.len() >= min_actions,
            "{case}: expected at least {min_actions} actions, got {}",
            actions.len()
        );

        if let Some(title) = required_title {
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    CodeActionOrCommand::CodeAction(action)
                        if action.title.contains(title) && action.edit.is_some()
                )),
                "{case}: expected a {title:?} conversion action with an edit"
            );
        }
    }
}

#[test]
fn test_clause_and_sort_actions_offered() {
    let provider = CodeActionProvider::new();
    // (case, text, title_groups): each group is a set of substrings that must
    // all appear in the title of a SINGLE offered action; different groups may
    // be satisfied by different actions.
    let cases: [(&str, &str, &[&[&str]]); 4] = [
        (
            "machine missing INVARIANTS",
            "MACHINE test\nVARIABLES x\nEND",
            &[&["INVARIANTS"]],
        ),
        (
            "context missing AXIOMS and CONSTANTS",
            "CONTEXT test\nSETS S\nEND",
            &[&["AXIOMS"], &["CONSTANTS"]],
        ),
        (
            "unsorted variables",
            "MACHINE test\nVARIABLES\n    z\n    a\n    m\nINVARIANTS\nEND",
            &[&["Sort", "variables"]],
        ),
        (
            "unsorted constants",
            "CONTEXT test\nCONSTANTS\n    c_z\n    c_a\n    c_m\nAXIOMS\nEND",
            &[&["Sort", "constants"]],
        ),
    ];

    for (case, text, title_groups) in cases {
        let params = create_test_params(
            "file:///test.eventb",
            Range {
                start: Position::new(0, 0),
                end: Position::new(0, 0),
            },
        );
        let actions = provider
            .provide_code_actions(&params, text, true, false)
            .unwrap_or_default();

        for group in title_groups {
            assert!(
                actions.iter().any(|action| matches!(
                    action,
                    CodeActionOrCommand::CodeAction(action)
                        if group.iter().all(|fragment| action.title.contains(fragment))
                )),
                "{case}: expected one action whose title contains all of {group:?}"
            );
        }
    }
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

    let actions = provider.provide_code_actions(&params, text, true, false);

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

    let actions = provider.provide_code_actions(&params, text, true, false);

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

    let actions = provider.provide_code_actions(&params, text, true, false);

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
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();

    assert!(
        actions.iter().any(|a| matches!(
            a,
            CodeActionOrCommand::CodeAction(action) if action.title.contains("Add missing END")
        )),
        "the Add-missing-END quick fix must be offered for an EOF diagnostic, got {actions:?}"
    );
}

/// The quick fix among `actions` whose title starts with `prefix`, if offered.
fn action_titled<'a>(
    actions: &'a [CodeActionOrCommand],
    prefix: &str,
) -> Option<&'a eventb_lsp::lsp_types::CodeAction> {
    actions.iter().find_map(|a| match a {
        CodeActionOrCommand::CodeAction(action) if action.title.starts_with(prefix) => Some(action),
        _ => None,
    })
}

/// Build a CodeActionParams carrying a single diagnostic with rule `code`
/// whose range is `op_range` (the operator it underlines), the shape the
/// diagnostics provider emits; the quick fixes read only the range and code.
fn diagnostic_params(uri: &str, op_range: Range, code: &str) -> CodeActionParams {
    use eventb_lsp::lsp_types::{Diagnostic, NumberOrString};
    let mut params = create_test_params(uri, op_range);
    params.context.diagnostics = vec![Diagnostic {
        range: op_range,
        code: Some(NumberOrString::String(code.to_string())),
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
    let params = diagnostic_params("file:///m.eventb", op, "EB026");
    let actions = provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();

    let fix =
        action_titled(&actions, "Replace").expect("a Replace quick fix must be offered for EB026");
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
    let params = diagnostic_params("file:///m.eventb", op, "EB026");
    let actions = provider
        .provide_code_actions(&params, text, true, false)
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
    let params = diagnostic_params("file:///m.eventb", op, "EB026");
    let actions = provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();

    assert!(
        action_titled(&actions, "Replace").is_none(),
        "no Replace quick fix for `:|`, got {actions:?}"
    );
}

#[test]
fn ascii_operator_advisory_offers_the_unicode_spelling() {
    // A `rossi.format.enforceUnicode` advisory on `&` → a quick fix replacing
    // exactly that token with `∧`, attached to the diagnostic.
    let provider = CodeActionProvider::new();
    let text = "MACHINE m\nINVARIANTS\n    @inv1 x : NAT & x <= 10\nEND\n";
    let op = Range {
        start: Position::new(2, 18),
        end: Position::new(2, 19),
    };
    let params = diagnostic_params("file:///m.eventb", op, ASCII_OPERATOR_CODE);
    let actions = provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();

    let fix = action_titled(&actions, "Replace")
        .expect("a Replace quick fix must be offered for the advisory");
    assert_eq!(fix.title, "Replace `&` with `∧`");
    assert_eq!(fix.kind, Some(CodeActionKind::QUICKFIX));
    assert!(fix.diagnostics.is_some(), "fix attaches to the diagnostic");
    let edit = &fix.edit.as_ref().unwrap().changes.as_ref().unwrap()
        [&Url::parse("file:///m.eventb").unwrap()][0];
    assert_eq!(edit.new_text, "∧");
    assert_eq!(edit.range, op, "edit replaces exactly the operator");
}

#[test]
fn ascii_operator_advisory_offers_nothing_for_a_stale_range() {
    // A diagnostic range the client carried across an edit may no longer
    // cover one operator token; the quick fix must not rewrite whatever it
    // covers now (here code plus comment prose).
    let provider = CodeActionProvider::new();
    let text = "@inv1 x : NAT // a <= b\n";
    let stale = Range {
        start: Position::new(0, 6),
        end: Position::new(0, 23),
    };
    let params = diagnostic_params("file:///m.eventb", stale, ASCII_OPERATOR_CODE);
    let actions = provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();

    assert!(
        action_titled(&actions, "Replace").is_none(),
        "no Replace quick fix for a range that is not one operator token, got {actions:?}"
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
        .provide_code_actions(&params, text, true, false)
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

    let converted = provider.convert_to_unicode(text, false);

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
    let actions = provider
        .provide_code_actions(&params, text, true, false)
        .unwrap();

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
    let actions = provider.provide_code_actions(&params, text, true, false);

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

#[test]
fn eb029_offers_to_remove_an_empty_clause() {
    // `WHERE` with no guards: deleting the header is the whole fix, and the
    // keyword is alone on its line, so the line goes with it.
    let provider = CodeActionProvider::new();
    let text = "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT e\n    WHERE\n    THEN\n        @act1 x ≔ 1\n    END\nEND\n";
    // The diagnostic underlines the clause keyword, as the provider emits it.
    let keyword = Range {
        start: Position::new(5, 4),
        end: Position::new(5, 9),
    };
    let params = diagnostic_params("file:///m.eventb", keyword, "EB029");
    let actions = provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();

    let fix = action_titled(&actions, "Remove empty")
        .expect("a Remove quick fix must be offered for EB029");
    assert_eq!(fix.title, "Remove empty WHERE");
    assert_eq!(fix.kind, Some(CodeActionKind::QUICKFIX));
    assert!(fix.diagnostics.is_some(), "fix attaches to the diagnostic");
    let edit = &fix.edit.as_ref().unwrap().changes.as_ref().unwrap()
        [&Url::parse("file:///m.eventb").unwrap()][0];
    assert_eq!(edit.new_text, "");
    assert_eq!(
        edit.range,
        Range {
            start: Position::new(5, 0),
            end: Position::new(6, 0),
        },
        "the whole line goes, not just the keyword"
    );
}

#[test]
fn eb029_offers_nothing_for_a_label_with_no_formula() {
    // Same rule, but nothing to delete on the user's behalf: what belongs
    // after `@inv1` is a predicate only they can write.
    let provider = CodeActionProvider::new();
    let text = "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1\nEND\n";
    // The diagnostic underlines the label, not a clause keyword.
    let label = Range {
        start: Position::new(4, 4),
        end: Position::new(4, 9),
    };
    let params = diagnostic_params("file:///m.eventb", label, "EB029");
    let actions = provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();

    assert!(
        action_titled(&actions, "Remove empty").is_none(),
        "a bare label is not an empty clause: {actions:?}"
    );
}

#[test]
fn eb032_offers_to_insert_a_label() {
    // The action has no label; the stem comes from the enclosing clause and
    // the number from the labels the same event already uses. `@act1` in the
    // event above is a different namespace (EB022 scopes action labels per
    // event), so it does not push this one to `@act2`.
    let provider = CodeActionProvider::new();
    let text = "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x ≔ 0\n    END\n    EVENT e\n    THEN\n        x ≔ 1\n    END\nEND\n";
    // The diagnostic underlines the item, as the provider emits it.
    let item = Range {
        start: Position::new(10, 8),
        end: Position::new(10, 13),
    };
    let params = diagnostic_params("file:///m.eventb", item, "EB032");
    let actions = provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();

    let fix = action_titled(&actions, "Insert label")
        .expect("an Insert quick fix must be offered for EB032");
    assert_eq!(fix.title, "Insert label @act1");
    assert_eq!(fix.kind, Some(CodeActionKind::QUICKFIX));
    let edit = &fix.edit.as_ref().unwrap().changes.as_ref().unwrap()
        [&Url::parse("file:///m.eventb").unwrap()][0];
    assert_eq!(edit.new_text, "@act1 ");
    assert_eq!(
        edit.range,
        Range {
            start: Position::new(10, 8),
            end: Position::new(10, 8),
        },
        "the label goes in front of the item, replacing nothing"
    );
}

#[test]
fn eb032_numbers_past_the_labels_in_scope() {
    // Guards and actions share one namespace per event, invariants one per
    // component, so each is looked for where its clash would be.
    let provider = CodeActionProvider::new();
    for (text, item, title) in [
        (
            "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT e\n    THEN\n        @act1 x ≔ 0\n        x ≔ 1\n    END\nEND\n",
            Range {
                start: Position::new(7, 8),
                end: Position::new(7, 13),
            },
            "Insert label @act2",
        ),
        (
            "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ∈ ℕ\n    x > 0\nEND\n",
            Range {
                start: Position::new(5, 4),
                end: Position::new(5, 9),
            },
            "Insert label @inv2",
        ),
    ] {
        let params = diagnostic_params("file:///m.eventb", item, "EB032");
        let actions = provider
            .provide_code_actions(&params, text, true, false)
            .unwrap_or_default();
        let fix = action_titled(&actions, "Insert label")
            .unwrap_or_else(|| panic!("no Insert quick fix for:\n{text}"));
        assert_eq!(fix.title, title, "for:\n{text}");
    }
}

#[test]
fn eb032_finds_the_clause_written_inline_before_the_item() {
    // The clause keyword need not open a line of its own: reading the last one
    // written before the item covers `EVENTS EVENT e THEN x ≔ 1 END`, where
    // the item's line opens with `EVENTS`.
    let provider = CodeActionProvider::new();
    let text = "MACHINE m\nVARIABLES\n    x\nEVENTS EVENT e THEN x ≔ 1 END\nEND\n";
    let item = Range {
        start: Position::new(3, 20),
        end: Position::new(3, 25),
    };
    let params = diagnostic_params("file:///m.eventb", item, "EB032");
    let actions = provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();

    let fix = action_titled(&actions, "Insert label")
        .expect("an inline clause must still name the label");
    assert_eq!(fix.title, "Insert label @act1");
}

#[test]
fn eb032_names_the_label_after_its_clause() {
    // Each clause has its own stem: Rodin writes `grd1` in a guard and `inv1`
    // in an invariant, so the fix does too.
    let provider = CodeActionProvider::new();
    for (text, item, title) in [
        (
            "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    x ∈ ℕ\nEND\n",
            Range {
                start: Position::new(4, 4),
                end: Position::new(4, 9),
            },
            "Insert label @inv1",
        ),
        (
            "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT e\n    WHERE\n        x > 0\n    THEN\n        @act1 x ≔ 1\n    END\nEND\n",
            Range {
                start: Position::new(6, 8),
                end: Position::new(6, 13),
            },
            "Insert label @grd1",
        ),
        (
            "CONTEXT c\nCONSTANTS\n    k\nAXIOMS\n    k = 1\nEND\n",
            Range {
                start: Position::new(4, 4),
                end: Position::new(4, 9),
            },
            "Insert label @axm1",
        ),
    ] {
        let params = diagnostic_params("file:///m.eventb", item, "EB032");
        let actions = provider
            .provide_code_actions(&params, text, true, false)
            .unwrap_or_default();
        let fix = action_titled(&actions, "Insert label")
            .unwrap_or_else(|| panic!("no Insert quick fix for:\n{text}"));
        assert_eq!(fix.title, title, "for:\n{text}");
    }
}

#[test]
fn eb030_offers_to_move_the_clause_above_the_one_it_must_precede() {
    let provider = CodeActionProvider::new();
    let text = "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT e\n    THEN\n        @act1 x ≔ 1\n    WITH\n        @w y = 1\n    END\nEND\n";
    // The diagnostic spans the whole misplaced clause, as the provider emits it.
    let clause = Range {
        start: Position::new(7, 4),
        end: Position::new(8, 16),
    };
    let params = diagnostic_params("file:///m.eventb", clause, "EB030");
    let actions = provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();

    let fix = action_titled(&actions, "Move").expect("a Move quick fix must be offered for EB030");
    assert_eq!(fix.title, "Move WITH above THEN");
    assert_eq!(fix.kind, Some(CodeActionKind::QUICKFIX));
    let edits = &fix.edit.as_ref().unwrap().changes.as_ref().unwrap()
        [&Url::parse("file:///m.eventb").unwrap()];
    // Insert the clause above THEN, then delete it where it was written.
    assert_eq!(edits.len(), 2, "{edits:?}");
    assert_eq!(
        edits[0].range,
        Range {
            start: Position::new(5, 0),
            end: Position::new(5, 0),
        }
    );
    assert_eq!(edits[0].new_text, "    WITH\n        @w y = 1\n");
    assert_eq!(
        edits[1].range,
        Range {
            start: Position::new(7, 0),
            end: Position::new(9, 0),
        }
    );
    assert_eq!(edits[1].new_text, "");
}

#[test]
fn eb030_move_stays_inside_its_own_event() {
    // `convergent EVENT b` hides the header keyword behind the inline status:
    // the scan must still stop there (the previous event's `END` closes it)
    // rather than targeting a clause of the event above.
    let provider = CodeActionProvider::new();
    let text = "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT a\n    THEN\n        @a1 x ≔ 1\n    END\n    convergent EVENT b\n    THEN\n        @b1 x ≔ 2\n    WITH\n        @w y = 1\n    END\nEND\n";
    let clause = Range {
        start: Position::new(11, 4),
        end: Position::new(12, 16),
    };
    let params = diagnostic_params("file:///m.eventb", clause, "EB030");
    let actions = provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();
    let fix = action_titled(&actions, "Move").expect("a Move quick fix must be offered");
    let edits = &fix.edit.as_ref().unwrap().changes.as_ref().unwrap()
        [&Url::parse("file:///m.eventb").unwrap()];
    assert_eq!(
        edits[0].range.start,
        Position::new(9, 0),
        "the clause moves above its own THEN, not the previous event's"
    );
}

#[test]
fn eb030_offers_nothing_when_the_clause_shares_its_last_line() {
    // Whole lines move, so a clause that does not own its last line would
    // drag the event's END along with it.
    let provider = CodeActionProvider::new();
    let text = "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT e\n    THEN\n        @act1 x ≔ 1\n    WITH @w y = 1 END\nEND\n";
    let clause = Range {
        start: Position::new(7, 4),
        end: Position::new(7, 17),
    };
    let params = diagnostic_params("file:///m.eventb", clause, "EB030");
    let actions = provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();
    assert!(
        action_titled(&actions, "Move").is_none(),
        "the END shares the line: {actions:?}"
    );
}
