//! Integration tests for folding ranges

use rossi_lsp::folding::FoldingRangeProvider;
use rossi_lsp::lsp_types::{FoldingRangeParams, TextDocumentIdentifier, Url};

fn create_test_params(uri: &str) -> FoldingRangeParams {
    FoldingRangeParams {
        text_document: TextDocumentIdentifier {
            uri: Url::parse(uri).unwrap(),
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    }
}

/// Each row covers one clause / block type. Asserts the provider produces a
/// folding range with the given `(start_line, end_line)` against the sample
/// text — collapses what used to be nine near-identical per-clause tests.
#[test]
fn test_fold_clauses_and_blocks() {
    let provider = FoldingRangeProvider::new();
    let params = create_test_params("file:///test.eventb");

    let cases: &[(&str, &str, u32, u32)] = &[
        (
            "CONTEXT block",
            "CONTEXT test\nSETS\n    S\n    T\nCONSTANTS c\nAXIOMS\n    @axm1 c > 0\nEND",
            0,
            7,
        ),
        (
            "MACHINE block",
            "MACHINE test\nVARIABLES\n    x\n    y\nINVARIANTS\n    @inv1 x > 0\nEND",
            0,
            6,
        ),
        (
            "INITIALISATION block",
            "MACHINE test\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        x := 0\n    END\nEND",
            2,
            5,
        ),
        (
            "VARIABLES clause",
            "MACHINE test\nVARIABLES\n    x\n    y\n    z\nINVARIANTS\n    @inv1 TRUE\nEND",
            1,
            4,
        ),
        (
            "INVARIANTS clause",
            "MACHINE test\nVARIABLES x\nINVARIANTS\n    @inv1 x > 0\n    @inv2 x < 100\n    @inv3 x /= 50\nEVENTS\nEND",
            2,
            5,
        ),
        (
            "AXIOMS clause",
            "CONTEXT test\nCONSTANTS c d e\nAXIOMS\n    @axm1 c > 0\n    @axm2 d < 100\n    @axm3 e /= 0\nEND",
            2,
            5,
        ),
        (
            "SETS clause",
            "CONTEXT test\nSETS\n    S\n    T\n    U\nCONSTANTS c\nEND",
            1,
            4,
        ),
        (
            "CONSTANTS clause",
            "CONTEXT test\nSETS S\nCONSTANTS\n    c1\n    c2\n    c3\nAXIOMS\nEND",
            2,
            5,
        ),
    ];

    for (name, text, want_start, want_end) in cases {
        let ranges = provider
            .folding_ranges(&params, text)
            .unwrap_or_else(|| panic!("[{name}] folding_ranges returned None"));
        assert!(
            ranges
                .iter()
                .any(|r| r.start_line == *want_start && r.end_line == *want_end),
            "[{name}] no folding range at lines {want_start}..{want_end}; got: {ranges:?}"
        );
    }
}

#[test]
fn test_fold_event_blocks() {
    let provider = FoldingRangeProvider::new();
    let text = "MACHINE test\nEVENTS\n    EVENT evt1\n    THEN\n        x := 1\n    END\n    EVENT evt2\n    THEN\n        y := 2\n    END\nEND";
    let params = create_test_params("file:///test.eventb");

    let ranges = provider.folding_ranges(&params, text);

    assert!(ranges.is_some());
    let ranges = ranges.unwrap();

    // Should have ranges for both events
    let has_evt1 = ranges.iter().any(|r| r.start_line == 2 && r.end_line == 5);
    let has_evt2 = ranges.iter().any(|r| r.start_line == 6 && r.end_line == 9);

    assert!(has_evt1, "Should detect first EVENT block");
    assert!(has_evt2, "Should detect second EVENT block");
}

#[test]
fn test_no_fold_for_empty_clauses() {
    let provider = FoldingRangeProvider::new();
    let text = "MACHINE test\nVARIABLES\nINVARIANTS\nEVENTS\nEND";
    let params = create_test_params("file:///test.eventb");

    let ranges = provider.folding_ranges(&params, text);

    if let Some(ranges) = ranges {
        // Should not have ranges for empty clauses (only the MACHINE block)
        let vars_range = ranges.iter().find(|r| r.start_line == 1);
        let invs_range = ranges.iter().find(|r| r.start_line == 2);

        assert!(
            vars_range.is_none(),
            "Should not create folding range for empty VARIABLES clause"
        );
        assert!(
            invs_range.is_none(),
            "Should not create folding range for empty INVARIANTS clause"
        );
    }
}

#[test]
fn test_complex_machine_multiple_folds() {
    let provider = FoldingRangeProvider::new();
    let text = "\
MACHINE counter
VARIABLES
    count
INVARIANTS
    @inv1 count >= 0
    @inv2 count <= 100
EVENTS
    EVENT INITIALISATION
    THEN
        count := 0
    END

    EVENT increment
    WHEN
        count < 100
    THEN
        count := count + 1
    END
END";
    let params = create_test_params("file:///test.eventb");

    let ranges = provider.folding_ranges(&params, text);

    assert!(ranges.is_some());
    let ranges = ranges.unwrap();

    // Should have multiple ranges
    assert!(
        ranges.len() >= 5,
        "Should have at least 5 folding ranges (MACHINE, VARIABLES, INVARIANTS, INITIALISATION, EVENT)"
    );

    // Verify each type exists
    let has_machine = ranges.iter().any(|r| r.start_line == 0);
    let has_vars = ranges.iter().any(|r| r.start_line == 1);
    let has_invs = ranges.iter().any(|r| r.start_line == 3);

    assert!(has_machine, "Should have MACHINE block");
    assert!(has_vars, "Should have VARIABLES clause");
    assert!(has_invs, "Should have INVARIANTS clause");
}

#[test]
fn test_nested_event_in_events_clause() {
    let provider = FoldingRangeProvider::new();
    let text = "MACHINE test\nEVENTS\n    EVENT e1\n    END\nEND";
    let params = create_test_params("file:///test.eventb");

    let ranges = provider.folding_ranges(&params, text);

    assert!(ranges.is_some());
    let ranges = ranges.unwrap();

    // Should have range for EVENTS clause and EVENT block
    let has_events_clause = ranges.iter().any(|r| r.start_line == 1);
    let has_event_block = ranges.iter().any(|r| r.start_line == 2);

    assert!(has_events_clause, "Should detect EVENTS clause");
    assert!(has_event_block, "Should detect EVENT block");
}

#[test]
fn test_all_clause_types() {
    let provider = FoldingRangeProvider::new();
    let text = "CONTEXT test\nEXTENDS\n    parent\nSETS\n    S\nCONSTANTS\n    c\nAXIOMS\n    @axm1 TRUE\n    @thm1 theorem TRUE\nEND";
    let params = create_test_params("file:///test.eventb");

    let ranges = provider.folding_ranges(&params, text);

    assert!(ranges.is_some());
    let ranges = ranges.unwrap();

    // Should detect all clause types
    let has_extends = ranges.iter().any(|r| r.start_line == 1);
    let has_sets = ranges.iter().any(|r| r.start_line == 3);
    let has_constants = ranges.iter().any(|r| r.start_line == 5);
    let has_axioms = ranges.iter().any(|r| r.start_line == 7);

    assert!(has_extends, "Should detect EXTENDS clause");
    assert!(has_sets, "Should detect SETS clause");
    assert!(has_constants, "Should detect CONSTANTS clause");
    assert!(has_axioms, "Should detect AXIOMS clause");
}
