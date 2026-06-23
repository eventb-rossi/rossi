//! Regression: a `VARIANT` whose expression starts with `(`, sitting right
//! after an `INVARIANTS`/`THEOREMS` section, must parse and round-trip.
//!
//! Before the fix, the section's `(labeled_predicate)*` repetition was
//! unguarded: it speculatively parsed the following `VARIANT` keyword as a
//! permissive `identifier`, absorbed the `(…)` as a function-application
//! argument, and only failed at `EVENTS`. The follow-set guard
//! (`!machine_section_kw` / `!context_section_kw`) stops the list at the
//! section boundary — the textual counterpart of how Camille/ProB/CamilleX
//! bound a formula against structural keywords.
//!
//! Originally surfaced by the import→re-parse round-trip on corpus models
//! whose machines declare such a variant (see rossi-build `import_corpus`).

use rossi::{parse, to_string};

/// `parse` succeeds and `print → re-parse → re-print` is byte-stable — the
/// same property `import_corpus` checks against the corpus.
fn assert_stable_roundtrip(source: &str) {
    let component = parse(source).expect("parse should succeed");
    let printed = to_string(&component);
    let reparsed = parse(&printed).expect("printed form must re-parse");
    assert_eq!(
        printed,
        to_string(&reparsed),
        "round-trip must be byte-stable"
    );
}

#[test]
fn variant_paren_expr_after_invariants_roundtrips() {
    assert_stable_roundtrip(
        r#"MACHINE m
VARIABLES
    a
    b
    c
INVARIANTS
    @inv1 a ∈ ℙ(b)
VARIANT
    (a × b) ∖ c
EVENTS
    EVENT INITIALISATION
    THEN
        @act1 a ≔ a
    END
END
"#,
    );
}

#[test]
fn variant_paren_expr_after_theorems_roundtrips() {
    assert_stable_roundtrip(
        r#"MACHINE m
VARIABLES
    a
    b
    c
THEOREMS
    @thm1 a ∈ ℙ(b)
VARIANT
    (a × b) ∖ c
EVENTS
    EVENT INITIALISATION
    THEN
        @act1 a ≔ a
    END
END
"#,
    );
}

#[test]
fn context_theorems_paren_predicate_after_axioms_parses() {
    // Symmetric context case: a `THEOREMS` section whose first predicate is a
    // bare parenthesized predicate, following an `AXIOMS` section. Before the
    // `!context_section_kw` guard, the AXIOMS `(labeled_predicate)*` swallowed
    // the `THEOREMS` keyword + `(…)` and failed at `END`.
    //
    // Asserts parse only (not byte-stable round-trip): printing an *unlabeled*
    // theorem is a separate, orthogonal limitation and is not what this guard
    // addresses.
    let source = r#"CONTEXT c
SETS
    S
CONSTANTS
    x
AXIOMS
    @axm1 x ∈ S
THEOREMS
    (x ∈ S)
END
"#;
    parse(source).expect("AXIOMS list must not swallow the following THEOREMS section");
}
