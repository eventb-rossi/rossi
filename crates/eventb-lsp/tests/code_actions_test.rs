//! Integration tests for code actions

use eventb_lsp::code_actions::{CodeActionProvider, FIX_ALL_KIND};
use eventb_lsp::cross_references::CrossReferenceManager;
use eventb_lsp::diagnostics::ASCII_OPERATOR_CODE;
use eventb_lsp::document::DocumentManager;
use eventb_lsp::identifier_utils::position_to_offset;
use eventb_lsp::lsp_types::{
    CodeAction, CodeActionContext, CodeActionKind, CodeActionOrCommand, CodeActionParams, Position,
    Range, TextDocumentIdentifier, TextEdit, Uri, WorkDoneProgressParams,
};
use eventb_lsp::workspace::WorkspaceSymbolProvider;
use std::sync::Arc;

fn create_test_params(uri: &str, range: Range) -> CodeActionParams {
    CodeActionParams {
        text_document: TextDocumentIdentifier {
            uri: uri.parse::<Uri>().unwrap(),
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
                action.kind == Some(CodeActionKind::REFACTOR) || action.kind == Some(FIX_ALL_KIND),
                "Action kind should be REFACTOR or the fix-all source kind"
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
    let uri = ("file:///m.eventb").parse::<Uri>().unwrap();
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
fn test_diagnostic_based_action() {
    use eventb_lsp::lsp_types::{Diagnostic, DiagnosticSeverity};

    let provider = CodeActionProvider::new();
    let text = "MACHINE test\nVARIABLES x";

    // Create a diagnostic for missing END, where pest reports it: just past
    // the last character of the file.
    let diagnostic = Diagnostic {
        range: Range {
            start: Position::new(1, 11),
            end: Position::new(1, 11),
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

/// Apply one document's `edits` to `text`, last position first so the earlier
/// ones stay where they were computed.
fn apply_edits(text: &str, edits: &[TextEdit]) -> String {
    // Inserts at one position land in array order, so among those the last
    // is applied first.
    let mut edits: Vec<(usize, &TextEdit)> = edits.iter().enumerate().collect();
    edits.sort_by_key(|(index, edit)| std::cmp::Reverse((edit.range.start, *index)));
    let mut result = text.to_string();
    for (_, edit) in edits {
        let start = position_to_offset(&result, edit.range.start).expect("edit start in bounds");
        let end = position_to_offset(&result, edit.range.end).expect("edit end in bounds");
        result.replace_range(start..end, &edit.new_text);
    }
    result
}

/// The document an action's edit leaves behind, for an action editing only
/// the document at `uri`.
fn applied(text: &str, uri: &str, action: &CodeAction) -> String {
    let changes = action
        .edit
        .as_ref()
        .and_then(|edit| edit.changes.as_ref())
        .unwrap_or_else(|| panic!("{:?} carries no edit", action.title));
    apply_edits(text, &changes[&uri.parse::<Uri>().unwrap()])
}

/// A quick fix offered on a document with no diagnostic is one the user takes
/// on trust, so it must leave a valid model valid and meaning what it did.
#[test]
fn quick_fixes_offered_without_a_diagnostic_keep_the_model() {
    let provider = CodeActionProvider::new();
    let uri = "file:///m.eventb";
    for text in [
        "machine m\nsees c\nend\n",
        "context c\nend\n",
        "context c\nextends b\nend\n",
    ] {
        let original = rossi::parse(text).expect("the fixture parses");
        let mut params = create_test_params(
            uri,
            Range {
                start: Position::new(0, 0),
                end: Position::new(0, 0),
            },
        );
        params.context.only = Some(vec![CodeActionKind::QUICKFIX]);
        for action in provider
            .provide_code_actions(&params, text, true, false)
            .unwrap_or_default()
        {
            let CodeActionOrCommand::CodeAction(action) = action else {
                continue;
            };
            let result = applied(text, uri, &action);
            let parsed = rossi::parse(&result).unwrap_or_else(|error| {
                panic!(
                    "{:?} left text that does not parse:\n{result}\n{error}",
                    action.title
                )
            });
            let clauses = |component: &rossi::Component| match component {
                rossi::Component::Machine(machine) => (machine.sees.clone(), vec![]),
                rossi::Component::Context(context) => (vec![], context.extends.clone()),
            };
            assert_eq!(
                clauses(&parsed),
                clauses(&original),
                "{:?} changed what the model sees or extends:\n{result}",
                action.title
            );
        }
    }
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
        [&("file:///m.eventb").parse::<Uri>().unwrap()][0];
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
        [&("file:///m.eventb").parse::<Uri>().unwrap()][0];
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
fn test_add_missing_end_not_offered_for_a_rule_diagnostic_on_the_last_line() {
    // A rule diagnostic is about what the model says, never about a missing
    // terminator, even when it underlines the last line of the file: here an
    // undeclared identifier in a model that is already closed by its END.
    let provider = CodeActionProvider::new();
    let text = "MACHINE m\nVARIABLES x\nINVARIANTS @inv1 y ∈ ℕ END";
    let range = Range {
        start: Position::new(2, 17),
        end: Position::new(2, 18),
    };
    let params = diagnostic_params("file:///test.eventb", range, "EB018");

    let actions = provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();

    assert!(
        action_titled(&actions, "Add missing END").is_none(),
        "Add-missing-END must not be offered for a rule diagnostic, got {actions:?}"
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
        [&("file:///m.eventb").parse::<Uri>().unwrap()][0];
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
        [&("file:///m.eventb").parse::<Uri>().unwrap()][0];
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
        (
            "context c\naxioms\n@axm1:\u{a0}1 = 1\n1 = 1\nend\n",
            Range {
                start: Position::new(3, 0),
                end: Position::new(3, 5),
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
        [&("file:///m.eventb").parse::<Uri>().unwrap()];
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
        [&("file:///m.eventb").parse::<Uri>().unwrap()];
    assert_eq!(
        edits[0].range.start,
        Position::new(9, 0),
        "the clause moves above its own THEN, not the previous event's"
    );
}

#[test]
fn eb034_offers_to_move_the_section_above_the_one_it_must_precede() {
    let provider = CodeActionProvider::new();
    let text = "CONTEXT c\nAXIOMS\n    @a 1 = 1\nSETS\n    S\nEND\n";
    // The diagnostic spans the whole misplaced section, as the lint emits it.
    let section = Range {
        start: Position::new(3, 0),
        end: Position::new(4, 5),
    };
    let params = diagnostic_params("file:///c.eventb", section, "EB034");
    let actions = provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();

    let fix = action_titled(&actions, "Move").expect("a Move quick fix must be offered for EB034");
    assert_eq!(fix.title, "Move SETS above AXIOMS");
    assert_eq!(fix.kind, Some(CodeActionKind::QUICKFIX));
    let edits = &fix.edit.as_ref().unwrap().changes.as_ref().unwrap()
        [&("file:///c.eventb").parse::<Uri>().unwrap()];
    assert_eq!(edits.len(), 2, "{edits:?}");
    assert_eq!(
        edits[0].range,
        Range {
            start: Position::new(1, 0),
            end: Position::new(1, 0),
        }
    );
    assert_eq!(edits[0].new_text, "SETS\n    S\n");
    assert_eq!(
        edits[1].range,
        Range {
            start: Position::new(3, 0),
            end: Position::new(5, 0),
        }
    );
}

#[test]
fn eb034_reads_a_machine_theorems_section_against_the_machine_order() {
    // THEOREMS is in both section lists at different positions, so the order
    // comes from the enclosing component header, not from the keyword.
    let provider = CodeActionProvider::new();
    let text = "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @i x ∈ ℤ\nVARIANT\n    x\nTHEOREMS\n    @t x = x\nEND\n";
    let section = Range {
        start: Position::new(7, 0),
        end: Position::new(8, 12),
    };
    let params = diagnostic_params("file:///m.eventb", section, "EB034");
    let actions = provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();
    let fix = action_titled(&actions, "Move").expect("a Move quick fix must be offered");
    assert_eq!(fix.title, "Move THEOREMS above VARIANT");
}

#[test]
fn eb034_move_stays_inside_its_own_component() {
    // The scan stops at the component header, and at the END of the component
    // above it, so a section is never lifted into the previous component.
    let provider = CodeActionProvider::new();
    let text = "CONTEXT c1\nSETS\n    T\nEND\nCONTEXT c2\nAXIOMS\n    @a 1 = 1\nSETS\n    S\nEND\n";
    let section = Range {
        start: Position::new(7, 0),
        end: Position::new(8, 5),
    };
    let params = diagnostic_params("file:///c.eventb", section, "EB034");
    let actions = provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();
    let fix = action_titled(&actions, "Move").expect("a Move quick fix must be offered");
    let edits = &fix.edit.as_ref().unwrap().changes.as_ref().unwrap()
        [&("file:///c.eventb").parse::<Uri>().unwrap()];
    assert_eq!(
        edits[0].range.start,
        Position::new(5, 0),
        "the section moves above its own AXIOMS, not the previous component's SETS"
    );
}

#[test]
fn eb034_carries_an_attached_comment_with_the_section() {
    // Only a top-level clause carries a `ClauseRegion`, so a comment written
    // above a section header is anchored to that section and has to travel
    // with it. A blank line is not a comment and stays where it is.
    let provider = CodeActionProvider::new();
    let text = "CONTEXT c\nAXIOMS\n    @a 1 = 1\n\n// the carrier sets\nSETS\n    S\nEND\n";
    let section = Range {
        start: Position::new(5, 0),
        end: Position::new(6, 5),
    };
    let params = diagnostic_params("file:///c.eventb", section, "EB034");
    let actions = provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();
    let fix = action_titled(&actions, "Move").expect("a Move quick fix must be offered");
    let edits = &fix.edit.as_ref().unwrap().changes.as_ref().unwrap()
        [&("file:///c.eventb").parse::<Uri>().unwrap()];
    assert_eq!(edits[0].new_text, "// the carrier sets\nSETS\n    S\n");
    assert_eq!(
        edits[1].range,
        Range {
            start: Position::new(4, 0),
            end: Position::new(7, 0),
        },
        "the comment line is deleted with the section, the blank line is not"
    );
}

#[test]
fn eb034_leaves_the_target_section_with_its_own_comment() {
    // The insert lands above the destination's comment, not between it and
    // its header: a comment above a section is that section's, so dropping
    // SETS in between would re-file the AXIOMS comment onto SETS.
    let provider = CodeActionProvider::new();
    let text = "CONTEXT c\n// the axioms\nAXIOMS\n    @a 1 = 1\nSETS\n    S\nEND\n";
    let section = Range {
        start: Position::new(4, 0),
        end: Position::new(5, 5),
    };
    let params = diagnostic_params("file:///c.eventb", section, "EB034");
    let actions = provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();
    let fix = action_titled(&actions, "Move").expect("a Move quick fix must be offered");
    let edits = &fix.edit.as_ref().unwrap().changes.as_ref().unwrap()
        [&("file:///c.eventb").parse::<Uri>().unwrap()];
    assert_eq!(
        edits[0].range.start,
        Position::new(1, 0),
        "the section goes above the AXIOMS comment, not below it"
    );
}

#[test]
fn eb034_never_splits_a_comment_opened_on_a_code_line() {
    // The lines above the section are the tail of a block comment the axiom
    // line opened, so they are that line's trailing comment, not the
    // section's. Moving them would leave an unterminated `/*` behind.
    let provider = CodeActionProvider::new();
    let text = "CONTEXT c\nAXIOMS\n    @a 1 = 1 /* trailing\n    comment */\nSETS\n    S\nEND\n";
    let section = Range {
        start: Position::new(4, 0),
        end: Position::new(5, 5),
    };
    let params = diagnostic_params("file:///c.eventb", section, "EB034");
    let actions = provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();
    let fix = action_titled(&actions, "Move").expect("a Move quick fix must be offered");
    let edits = &fix.edit.as_ref().unwrap().changes.as_ref().unwrap()
        [&("file:///c.eventb").parse::<Uri>().unwrap()];
    assert_eq!(edits[0].new_text, "SETS\n    S\n");
    assert_eq!(
        edits[1].range.start,
        Position::new(4, 0),
        "the comment stays with the axiom line that opened it"
    );
}

#[test]
fn eb034_offers_nothing_when_the_section_shares_its_last_line() {
    // Whole lines move, so a section that does not own its last line would
    // drag the component's END along with it.
    let provider = CodeActionProvider::new();
    let text = "CONTEXT c\nAXIOMS\n    @a 1 = 1\nSETS S END\n";
    let section = Range {
        start: Position::new(3, 0),
        end: Position::new(3, 6),
    };
    let params = diagnostic_params("file:///c.eventb", section, "EB034");
    let actions = provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();
    assert!(
        action_titled(&actions, "Move").is_none(),
        "the END shares the line: {actions:?}"
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

/// A provider over a workspace holding `files` (URI, text), with the indexes
/// the cross-file fixes read.
fn workspace_provider(files: &[(&str, &str)]) -> CodeActionProvider {
    let symbols = Arc::new(WorkspaceSymbolProvider::new());
    let components = Arc::new(CrossReferenceManager::new());
    let documents = Arc::new(DocumentManager::new());
    for (uri, text) in files {
        symbols.update_symbols((*uri).to_string(), text);
        components.update_component((*uri).to_string(), text);
        documents.open(uri.parse::<Uri>().unwrap(), 1, (*text).to_string());
    }
    let mut provider = CodeActionProvider::new();
    provider.set_workspace_symbols(symbols);
    provider.set_cross_reference_manager(components);
    provider.set_document_manager(documents);
    provider
}

/// The range of the first `word` on 0-indexed `line` of `text`, in the
/// UTF-16 columns LSP counts (every character here is in the BMP).
fn word_on_line(text: &str, line: u32, word: &str) -> Range {
    let content = text.lines().nth(line as usize).expect("line exists");
    let byte = content.find(word).expect("word on the line");
    let start = content[..byte].chars().count() as u32;
    Range {
        start: Position::new(line, start),
        end: Position::new(line, start + word.chars().count() as u32),
    }
}

/// The quick fixes offered for an EB018 on `word` at `line`.
fn eb018_fixes(
    provider: &CodeActionProvider,
    uri: &str,
    text: &str,
    line: u32,
    word: &str,
) -> Vec<CodeAction> {
    let mut params = diagnostic_params(uri, word_on_line(text, line, word), "EB018");
    params.context.only = Some(vec![CodeActionKind::QUICKFIX]);
    provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|action| match action {
            CodeActionOrCommand::CodeAction(action) => Some(action),
            _ => None,
        })
        .collect()
}

const DECLARES_K: &str = "context c\nconstants k\naxioms\n  @k k ∈ ℕ\nend\n";

#[test]
fn eb018_sees_the_context_declaring_the_name() {
    // (machine as written, the same machine seeing `c`): no SEES yet, one
    // written inline, one written as a block, and one to go after REFINES.
    let cases = [
        (
            "machine m\nvariables x\ninvariants\n  @t x ∈ ℕ\n  @i x < k\nend\n",
            "machine m\nsees c\nvariables x\ninvariants\n  @t x ∈ ℕ\n  @i x < k\nend\n",
        ),
        (
            "machine m\nsees d\nvariables x\ninvariants\n  @t x ∈ ℕ\n  @i x < k\nend\n",
            "machine m\nsees d c\nvariables x\ninvariants\n  @t x ∈ ℕ\n  @i x < k\nend\n",
        ),
        (
            "MACHINE m\nSEES\n    d\nVARIABLES\n    x\nINVARIANTS\n    @t x ∈ ℕ\n    @i x < k\nEND\n",
            "MACHINE m\nSEES\n    d\n    c\nVARIABLES\n    x\nINVARIANTS\n    @t x ∈ ℕ\n    @i x < k\nEND\n",
        ),
        (
            "machine m\nrefines a // the abstraction\nvariables x\ninvariants\n  @t x ∈ ℕ\n  @i x < k\nend\n",
            "machine m\nrefines a // the abstraction\nsees c\nvariables x\ninvariants\n  @t x ∈ ℕ\n  @i x < k\nend\n",
        ),
    ];
    for (text, expected) in cases {
        let uri = "file:///m.eventb";
        let provider = workspace_provider(&[("file:///c.eventb", DECLARES_K), (uri, text)]);
        let line = text.lines().position(|l| l.contains("@i")).unwrap() as u32;
        let fixes = eb018_fixes(&provider, uri, text, line, "k");
        let fix = fixes
            .iter()
            .find(|action| action.title == "Add c to SEES")
            .unwrap_or_else(|| panic!("no SEES fix for:\n{text}\ngot {fixes:?}"));
        assert_eq!(fix.kind, Some(CodeActionKind::QUICKFIX));
        assert_eq!(fix.is_preferred, Some(true));
        assert_eq!(applied(text, uri, fix), expected);
        rossi::parse(expected).expect("the fixed machine parses");
    }
}

#[test]
fn eb018_extends_the_context_declaring_the_name() {
    let uri = "file:///d.eventb";
    let text = "context d\nconstants j\naxioms\n  @j j = k\nend\n";
    let provider = workspace_provider(&[("file:///c.eventb", DECLARES_K), (uri, text)]);
    let fixes = eb018_fixes(&provider, uri, text, 3, "k");
    let fix = fixes
        .iter()
        .find(|action| action.title == "Add c to EXTENDS")
        .unwrap_or_else(|| panic!("no EXTENDS fix, got {fixes:?}"));
    assert_eq!(
        applied(text, uri, fix),
        "context d\nextends c\nconstants j\naxioms\n  @j j = k\nend\n"
    );
}

#[test]
fn eb018_never_extends_into_a_cycle() {
    // `c` already extends `d`, so `d` extending `c` would close a cycle.
    let uri = "file:///d.eventb";
    let text = "context d\nconstants j\naxioms\n  @j j = k\nend\n";
    let extends_d = "context c\nextends d\nconstants k\naxioms\n  @k k ∈ ℕ\nend\n";
    let provider = workspace_provider(&[("file:///c.eventb", extends_d), (uri, text)]);
    let fixes = eb018_fixes(&provider, uri, text, 3, "k");
    assert!(
        fixes.iter().all(|action| !action.title.contains("EXTENDS")),
        "{fixes:?}"
    );
}

#[test]
fn eb018_offers_every_declaring_context_and_prefers_none() {
    let uri = "file:///m.eventb";
    let text = "machine m\nvariables x\ninvariants\n  @t x ∈ ℕ\n  @i x < k\nend\n";
    let provider = workspace_provider(&[
        ("file:///c.eventb", DECLARES_K),
        ("file:///e.eventb", "context e\nsets k\nend\n"),
        (uri, text),
    ]);
    let fixes = eb018_fixes(&provider, uri, text, 4, "k");
    let titles: Vec<&str> = fixes
        .iter()
        .map(|action| action.title.as_str())
        .filter(|title| title.starts_with("Add"))
        .collect();
    assert_eq!(titles, ["Add c to SEES", "Add e to SEES"]);
    assert!(
        fixes
            .iter()
            .all(|action| action.is_preferred == Some(false))
    );
}

/// The document the quick fix titled `title` leaves, among those offered for
/// an EB018 on `word` at `line`.
fn eb018_fixed(text: &str, line: u32, word: &str, title: &str) -> String {
    let uri = "file:///m.eventb";
    let provider = CodeActionProvider::new();
    let fixes = eb018_fixes(&provider, uri, text, line, word);
    let fix = fixes
        .iter()
        .find(|action| action.title == title)
        .unwrap_or_else(|| panic!("no {title:?} for:\n{text}\ngot {fixes:?}"));
    assert_eq!(
        fix.is_preferred,
        Some(false),
        "a new declaration is a choice"
    );
    let fixed = applied(text, uri, fix);
    rossi::parse(&fixed).unwrap_or_else(|error| panic!("{error}:\n{fixed}"));
    fixed
}

#[test]
fn eb018_declares_the_name_as_a_variable() {
    let cases = [
        (
            "machine m\nvariables x\ninvariants\n  @t x ∈ ℕ\n  @i k ∈ ℕ\nend\n",
            "machine m\nvariables x k\ninvariants\n  @t x ∈ ℕ\n  @i k ∈ ℕ\nend\n",
        ),
        (
            "machine m\nsees c\ninvariants\n  @i k ∈ ℕ\nend\n",
            "machine m\nsees c\nvariables k\ninvariants\n  @i k ∈ ℕ\nend\n",
        ),
        (
            "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @t x ∈ ℕ\n    @i k ∈ ℕ\nEND\n",
            "MACHINE m\nVARIABLES\n    x\n    k\nINVARIANTS\n    @t x ∈ ℕ\n    @i k ∈ ℕ\nEND\n",
        ),
    ];
    for (text, expected) in cases {
        let line = text.lines().position(|l| l.contains("@i")).unwrap() as u32;
        assert_eq!(
            eb018_fixed(text, line, "k", "Declare k as a variable"),
            expected
        );
    }
}

#[test]
fn eb018_declares_the_name_as_a_parameter_of_its_event() {
    let cases = [
        (
            "machine m\nevents\n  event e\n    where\n      @g p > 0\n  end\nend\n",
            "machine m\nevents\n  event e\n    any p\n    where\n      @g p > 0\n  end\nend\n",
        ),
        (
            "machine m\nevents\n  event e\n    any q\n    where\n      @g p > q\n  end\nend\n",
            "machine m\nevents\n  event e\n    any q p\n    where\n      @g p > q\n  end\nend\n",
        ),
        (
            "MACHINE m\nEVENTS\n    EVENT e\n    ANY\n        q\n    WHERE\n        @g p > q\n    END\nEND\n",
            "MACHINE m\nEVENTS\n    EVENT e\n    ANY\n        q\n        p\n    WHERE\n        @g p > q\n    END\nEND\n",
        ),
        (
            "machine m\nrefines a\nevents\n  convergent event e refines f\n    where\n      @g p > 0\n  end\nend\n",
            "machine m\nrefines a\nevents\n  convergent event e refines f\n    any p\n    where\n      @g p > 0\n  end\nend\n",
        ),
    ];
    for (text, expected) in cases {
        let line = text.lines().position(|l| l.contains("@g")).unwrap() as u32;
        assert_eq!(
            eb018_fixed(text, line, "p", "Declare p as a parameter of e"),
            expected
        );
    }
}

#[test]
fn eb018_declares_the_name_as_a_constant() {
    let cases = [
        (
            "context d\naxioms\n  @a j > 0\nend\n",
            "context d\nconstants j\naxioms\n  @a j > 0\nend\n",
        ),
        (
            "context d\nextends c\nsets S\naxioms\n  @a j > 0\nend\n",
            "context d\nextends c\nsets S\nconstants j\naxioms\n  @a j > 0\nend\n",
        ),
    ];
    for (text, expected) in cases {
        let line = text.lines().position(|l| l.contains("@a")).unwrap() as u32;
        assert_eq!(
            eb018_fixed(text, line, "j", "Declare j as a constant"),
            expected
        );
    }
}

#[test]
fn eb018_declares_nothing_for_a_primed_name() {
    // An after-state name is never declared; the prime is what is wrong.
    let text = "machine m\nvariables x\ninvariants\n  @t x ∈ ℕ\n  @i x' ∈ ℕ\nend\n";
    let fixes = eb018_fixes(
        &CodeActionProvider::new(),
        "file:///m.eventb",
        text,
        4,
        "x'",
    );
    assert!(
        fixes
            .iter()
            .all(|action| !action.title.starts_with("Declare")),
        "{fixes:?}"
    );
}

/// The titles of the quick fixes offered for a diagnostic `code` on `word` at
/// `line` of the document `uri` in a workspace of `files`.
fn fix_titles(files: &[(&str, &str)], uri: &str, code: &str, line: u32, word: &str) -> Vec<String> {
    let text = files.iter().find(|(u, _)| *u == uri).unwrap().1;
    let provider = workspace_provider(files);
    let mut params = diagnostic_params(uri, word_on_line(text, line, word), code);
    params.context.only = Some(vec![CodeActionKind::QUICKFIX]);
    provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|action| match action {
            CodeActionOrCommand::CodeAction(action) => Some(action.title),
            _ => None,
        })
        .collect()
}

#[test]
fn eb018_offers_the_names_in_scope_spelled_alike() {
    let machine = "\
machine m
variables count total
invariants
  @t1 count ∈ ℕ
  @t2 total ∈ ℕ
  @i cuont ≤ total
events
  event add
    any amount
    where
      @g1 amount ∈ ℕ
      @g2 amuont > 0
    then
      @a total ≔ total + amount
  end
end
";
    let uri = "file:///m.eventb";
    let titles = fix_titles(&[(uri, machine)], uri, "EB018", 5, "cuont");
    assert!(
        titles.contains(&"Change to count".to_string()),
        "{titles:?}"
    );
    // A parameter is in scope inside its own event only.
    let titles = fix_titles(&[(uri, machine)], uri, "EB018", 11, "amuont");
    assert!(
        titles.contains(&"Change to amount".to_string()),
        "{titles:?}"
    );
    let titles = fix_titles(&[(uri, machine)], uri, "EB018", 5, "cuont");
    assert!(
        !titles.contains(&"Change to amount".to_string()),
        "{titles:?}"
    );

    let context = "context c\nconstants limit\naxioms\n  @l limit ∈ ℕ\n  @m limt > 0\nend\n";
    let uri = "file:///c.eventb";
    let titles = fix_titles(&[(uri, context)], uri, "EB018", 4, "limt");
    assert!(
        titles.contains(&"Change to limit".to_string()),
        "{titles:?}"
    );
}

#[test]
fn eb018_offers_nothing_spelled_too_differently() {
    let machine = "machine m\nvariables x\ninvariants\n  @t x ∈ ℕ\n  @i y > x\nend\n";
    let uri = "file:///m.eventb";
    let titles = fix_titles(&[(uri, machine)], uri, "EB018", 4, "y");
    assert!(
        titles.iter().all(|t| !t.starts_with("Change to")),
        "{titles:?}"
    );
}

#[test]
fn eb009_offers_the_components_and_abstract_events_spelled_alike() {
    let context = "context ctx\nend\n";
    let abstraction = "machine abs\nevents\n  event inc\n  end\n  event dec\n  end\nend\n";
    let machine = "machine m\nrefines abs\nsees ctz\nevents\n  event inc refines inx\n  end\nend\n";
    let files = [
        ("file:///ctx.eventb", context),
        ("file:///abs.eventb", abstraction),
        ("file:///m.eventb", machine),
    ];
    let titles = fix_titles(&files, "file:///m.eventb", "EB009", 2, "ctz");
    assert_eq!(titles, ["Change to ctx"]);
    let titles = fix_titles(&files, "file:///m.eventb", "EB009", 4, "inx");
    assert_eq!(titles, ["Change to inc"]);
}

#[test]
fn eb025_keeps_the_disappeared_variable() {
    let machine = "\
machine m2
refines m1
variables w
invariants
  @i w ∈ ℕ
events
  event tick
    where
      @g v > 0
  end
end
";
    let uri = "file:///m2.eventb";
    let provider = CodeActionProvider::new();
    let mut params = diagnostic_params(uri, word_on_line(machine, 8, "v"), "EB025");
    params.context.only = Some(vec![CodeActionKind::QUICKFIX]);
    let actions = provider
        .provide_code_actions(&params, machine, true, false)
        .unwrap_or_default();
    let fix = action_titled(&actions, "Keep v").expect("a keep fix for EB025");
    assert_eq!(fix.title, "Keep v in VARIABLES");
    assert_eq!(fix.is_preferred, Some(true));
    assert_eq!(
        applied(machine, uri, fix),
        machine.replace("variables w\n", "variables w v\n")
    );
}

#[test]
fn eb022_relabels_past_a_comment_holding_wide_characters() {
    // Each `≔` in the comment is three bytes but one character: a fix that
    // mixed the two would read eight bytes on, inside the `∈`.
    let uri = "file:///c0.eventb";
    let text = "context c0\n// ≔≔≔≔\nconstants c d\naxioms\n  @a1 c ∈ ℕ\n  @a1 d ∈ ℕ\nend\n";
    let first = Range::new(Position::new(4, 2), Position::new(4, 11));
    let fix = eb022_fix(&CodeActionProvider::new(), uri, text, first);
    assert_eq!(
        applied(text, uri, &fix),
        text.replace("  @a1 d ∈ ℕ", "  @axm1 d ∈ ℕ")
    );
}

/// The single quick fix offered for an EB022 whose range is `range`.
fn eb022_fix(provider: &CodeActionProvider, uri: &str, text: &str, range: Range) -> CodeAction {
    let mut params = diagnostic_params(uri, range, "EB022");
    params.context.only = Some(vec![CodeActionKind::QUICKFIX]);
    let actions: Vec<CodeAction> = provider
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|action| match action {
            CodeActionOrCommand::CodeAction(action) => Some(action),
            _ => None,
        })
        .collect();
    assert_eq!(actions.len(), 1, "{actions:?}");
    actions.into_iter().next().unwrap()
}

#[test]
fn eb022_relabels_the_last_duplicate() {
    // The finding underlines the first `@inv1`; the one written later is the
    // one relabeled, so an existing proof keeps its obligation name.
    let uri = "file:///m.eventb";
    let text =
        "machine m\nvariables x\ninvariants\n  @inv1 x ∈ ℕ\n  @inv2 x ≥ 0\n  @inv1 x < 9\nend\n";
    let first = Range::new(Position::new(3, 2), Position::new(3, 13));
    let fix = eb022_fix(&CodeActionProvider::new(), uri, text, first);
    assert_eq!(fix.title, "Relabel the last @inv1 as @inv3");
    assert_eq!(
        applied(text, uri, &fix),
        text.replace("  @inv1 x < 9", "  @inv3 x < 9")
    );

    // Guards and actions share one namespace, but each keeps its own stem.
    let text = "machine m\nevents\n  event e\n    where\n      @a1 1 = 1\n    then\n      @a1 x ≔ 1\n  end\nend\n";
    let first = Range::new(Position::new(4, 6), Position::new(4, 15));
    let fix = eb022_fix(&CodeActionProvider::new(), uri, text, first);
    assert_eq!(
        applied(text, uri, &fix),
        text.replace("@a1 x ≔ 1", "@act1 x ≔ 1")
    );
}

#[test]
fn eb022_relabels_clear_of_the_inherited_labels() {
    let abstraction = "machine a\nevents\n  event e\n    where\n      @grd1 1 = 1\n      @grd2 2 = 2\n  end\nend\n";
    let machine = "machine m\nrefines a\nevents\n  event e extends e\n    where\n      @grd1 3 = 3\n  end\nend\n";
    let uri = "file:///m.eventb";
    let provider = workspace_provider(&[("file:///a.eventb", abstraction), (uri, machine)]);
    let clash = Range::new(Position::new(5, 6), Position::new(5, 17));
    let fix = eb022_fix(&provider, uri, machine, clash);
    assert_eq!(fix.title, "Relabel @grd1 as @grd3");
    assert_eq!(
        applied(machine, uri, &fix),
        machine.replace("@grd1 3 = 3", "@grd3 3 = 3")
    );
}

#[test]
fn eb020_moves_past_a_comment_holding_wide_characters() {
    // Each `≔` in the comment is three bytes but one character: a fix that
    // mixed the two would misplace every line it reads.
    let uri = "file:///c0.eventb";
    let text = "context c0\n// c ≔ d ≔ e\nconstants c d\naxioms\n  @a1 c = d\n  @a2 d > 0\n  @a3 c ∈ ℕ\nend\n";
    let mut params = diagnostic_params(uri, word_on_line(text, 4, "c"), "EB020");
    params.context.only = Some(vec![CodeActionKind::QUICKFIX]);
    let actions = CodeActionProvider::new()
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();
    let fix = action_titled(&actions, "Move").expect("a move fix for EB020");
    assert_eq!(
        applied(text, uri, fix),
        text.replace(
            "  @a1 c = d\n  @a2 d > 0\n  @a3 c ∈ ℕ\n",
            "  @a3 c ∈ ℕ\n  @a1 c = d\n  @a2 d > 0\n"
        )
    );
}

#[test]
fn eb020_moves_the_typing_predicate_above_the_read() {
    let uri = "file:///c0.eventb";
    // (document, line of the untyped read, name, expected title, expected text)
    let cases = [
        (
            "context c0\nconstants c d\naxioms\n  @a1 c = d\n  @a2 d > 0\n  @a3 c ∈ ℕ\nend\n",
            3,
            "c",
            "Move @a3 above @a1",
            "context c0\nconstants c d\naxioms\n  @a3 c ∈ ℕ\n  @a1 c = d\n  @a2 d > 0\nend\n",
        ),
        (
            "machine m\nevents\n  event e\n    any p q\n    where\n      @g1 p = q\n      // types it\n      @g2 q ⊆ ℕ\n  end\nend\n",
            5,
            "p",
            "Move @g2 above @g1",
            "machine m\nevents\n  event e\n    any p q\n    where\n      // types it\n      @g2 q ⊆ ℕ\n      @g1 p = q\n  end\nend\n",
        ),
    ];
    for (text, line, name, title, expected) in cases {
        let mut params = diagnostic_params(uri, word_on_line(text, line, name), "EB020");
        params.context.only = Some(vec![CodeActionKind::QUICKFIX]);
        let actions = CodeActionProvider::new()
            .provide_code_actions(&params, text, true, false)
            .unwrap_or_default();
        let fix = action_titled(&actions, "Move").expect("a move fix for EB020");
        assert_eq!(fix.title, title);
        assert_eq!(applied(text, uri, fix), expected);
    }
}

/// The parenthesizing fixes offered for the diagnostic the server publishes
/// first for `text`, as (title, fixed text).
fn parenthesize_fixes(text: &str, error_index: usize) -> Vec<(String, String)> {
    let uri = "file:///m.eventb";
    let errors = rossi::parse_components_with_recovery(text).errors;
    let diagnostic = eventb_lsp::diagnostics::parse_error_to_diagnostic(&errors[error_index], text);
    let mut params = create_test_params(uri, diagnostic.range);
    params.context.diagnostics = vec![diagnostic];
    params.context.only = Some(vec![CodeActionKind::QUICKFIX]);
    CodeActionProvider::new()
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|action| match action {
            CodeActionOrCommand::CodeAction(action) if action.title.starts_with("Parenthesize") => {
                let fixed = applied(text, uri, &action);
                Some((action.title, fixed))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn incompatible_operators_are_parenthesized_either_way() {
    let text = "machine m\nvariables x\ninvariants\n  @t x ∈ ℕ\n  @i x = 1 ∧ x = 2 ∨ x = 3\nend\n";
    let fixes = parenthesize_fixes(text, 0);
    assert_eq!(
        fixes,
        [
            (
                "Parenthesize x = 1 ∧ x = 2".to_string(),
                text.replace("x = 1 ∧ x = 2 ∨", "(x = 1 ∧ x = 2) ∨"),
            ),
            (
                "Parenthesize x = 2 ∨ x = 3".to_string(),
                text.replace("∧ x = 2 ∨ x = 3", "∧ (x = 2 ∨ x = 3)"),
            ),
        ]
    );
    for (_, fixed) in fixes {
        rossi::parse(&fixed).expect("the parenthesized model parses");
    }
}

#[test]
fn a_recovered_incompatible_operators_error_is_parenthesized_too() {
    // Only the first error is reported bare; a later one arrives wrapped by
    // the recovery, located relative to its own predicate.
    let text = "machine m\nvariables x\ninvariants\n  @i x = 1 ∧ x = 2 ∨ x = 3\n  @j x ∈ {1} ∪ {2} ∩ {3}\nend\n";
    let fixes = parenthesize_fixes(text, 1);
    let titles: Vec<&str> = fixes.iter().map(|(title, _)| title.as_str()).collect();
    assert_eq!(titles, ["Parenthesize {1} ∪ {2}", "Parenthesize {2} ∩ {3}"]);
    assert_eq!(fixes[0].1, text.replace("{1} ∪ {2} ∩", "({1} ∪ {2}) ∩"));
}

#[test]
fn a_grouping_that_leaves_the_formula_broken_is_not_offered() {
    // Grouping the chain so far leaves `… ∧ c = 3 ∨ d = 4` just as broken.
    let text = "machine m\nvariables a\ninvariants\n  @i a = 1 ∨ a = 2 ∧ a = 3 ∨ a = 4\nend\n";
    let titles: Vec<String> = parenthesize_fixes(text, 0)
        .into_iter()
        .map(|(title, _)| title)
        .collect();
    assert_eq!(titles, ["Parenthesize a = 2 ∧ a = 3"]);
}

#[test]
fn eb031_replaces_the_separator_with_a_space() {
    let uri = "file:///c.eventb";
    let text = "context c\nconstants a\u{3000}b\nend\n";
    let separator = Range::new(Position::new(1, 11), Position::new(1, 12));
    let mut params = diagnostic_params(uri, separator, "EB031");
    params.context.only = Some(vec![CodeActionKind::QUICKFIX]);
    let actions = CodeActionProvider::new()
        .provide_code_actions(&params, text, true, false)
        .unwrap_or_default();
    let fix = action_titled(&actions, "Replace").expect("a fix for EB031");
    assert_eq!(fix.title, "Replace U+3000 with a space");
    assert_eq!(applied(text, uri, fix), "context c\nconstants a b\nend\n");
}
