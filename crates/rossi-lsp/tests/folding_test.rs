//! Integration tests for folding ranges (derived from the parsed AST).

use rossi_lsp::folding::FoldingRangeProvider;
use rossi_lsp::lsp_types::FoldingRange;

fn folds(text: &str) -> Vec<FoldingRange> {
    FoldingRangeProvider::new()
        .folding_ranges(text)
        .unwrap_or_default()
}

fn has(ranges: &[FoldingRange], start: u32, end: u32) -> bool {
    ranges
        .iter()
        .any(|r| r.start_line == start && r.end_line == end)
}

/// The reported bug: a machine that contains events folded only to its first
/// nested event END instead of its own END. Drive the real example model and
/// assert the whole machine folds (and the context still does too).
#[test]
fn base_model_machine_folds_to_its_own_end() {
    let text = include_str!("../../rossi/examples/base-model.eventb");
    let ranges = folds(text);

    // `context C1` (line 16) … `end` (line 63): 0-indexed 15..62.
    assert!(
        has(&ranges, 15, 62),
        "context C1 must fold 15..62; got {ranges:?}"
    );
    // `machine M1` (line 66) … final `end` (line 1249): 0-indexed 65..1248.
    assert!(
        has(&ranges, 65, 1248),
        "machine M1 must fold to its own END (65..1248), not the first event END"
    );
}

/// Each clause/block kind produces a fold over its full extent.
#[test]
fn clause_and_block_folds() {
    // 0 CONTEXT | 1 SETS | 2 S | 3 T | 4 CONSTANTS | 5 k | 6 AXIOMS | 7 @a1 | 8 END
    let ctx = "CONTEXT c\nSETS\n    S\n    T\nCONSTANTS\n    k\nAXIOMS\n    @a1 k > 0\nEND";
    let ranges = folds(ctx);
    assert!(has(&ranges, 0, 8), "context block; got {ranges:?}");
    assert!(has(&ranges, 1, 3), "sets clause; got {ranges:?}");
    assert!(has(&ranges, 4, 5), "constants clause; got {ranges:?}");
    assert!(has(&ranges, 6, 7), "axioms clause; got {ranges:?}");

    // 0 MACHINE | 1 VARIABLES | 2 x | 3 y | 4 INVARIANTS | 5 @i | 6 END
    let mch = "MACHINE m\nVARIABLES\n    x\n    y\nINVARIANTS\n    @i x > 0\nEND";
    let ranges = folds(mch);
    assert!(has(&ranges, 0, 6), "machine block; got {ranges:?}");
    assert!(has(&ranges, 1, 3), "variables clause; got {ranges:?}");
    assert!(has(&ranges, 4, 5), "invariants clause; got {ranges:?}");
}

/// Keywords spelled inside comments are masked by the lexer, so they can never
/// open or close a fold — the AST never sees them.
#[test]
fn comment_keywords_do_not_create_folds() {
    // 0 MACHINE | 1 EVENTS | 2 EVENT evt // not the END | 3 THEN |
    // 4 @a x := 1 /* EVENT ghost */ | 5 END | 6 END
    let text = "\
MACHINE test
EVENTS
    EVENT evt // not the END
    THEN
        @a x := 1 /* EVENT ghost */
    END
END";
    let ranges = folds(text);
    assert!(
        has(&ranges, 2, 5),
        "event fold must span 2..5 despite comment keywords; got {ranges:?}"
    );
    assert!(
        !ranges.iter().any(|r| r.start_line == 4),
        "no fold may open on the commented line; got {ranges:?}"
    );
}

/// Folding is driven by the recovery-tolerant parse, so a local syntax error
/// does not erase the document's folds.
#[test]
fn broken_document_still_folds() {
    // The broken invariant forces recovery.
    let text = "\
MACHINE m
VARIABLES
    x
INVARIANTS
    @i broken @#$ syntax
EVENTS
    EVENT step
    THEN
        @a x := 0
    END
END";
    let ranges = folds(text);
    let last = text.lines().count() as u32 - 1;
    assert!(
        has(&ranges, 0, last),
        "machine block must fold despite the error; got {ranges:?}"
    );
    // 6 EVENT step | 7 THEN | 8 @a x := 0 | 9 END
    assert!(
        has(&ranges, 6, 9),
        "the recoverable event must still fold; got {ranges:?}"
    );
}
