//! Camille-style pretty printing.
//!
//! Pins the layout of [`Style::Camille`] — the shape Rodin's Camille text
//! editor prints (lowercase keywords, inline header clauses and declaration
//! lists, a blank line between clauses and events, the 0/2/4/6 event
//! ladder) — plus the style toggles that override a preset, reparse
//! round-trips for every toggle combination, and formatting idempotence.

mod common;

use common::format_checked;
use rossi::{DeclListLayout, KeywordCase, PrettyPrinter, Style, StyleOverrides, format_str};

fn camille() -> PrettyPrinter {
    PrettyPrinter::styled(Style::Camille)
}

// =========================================================================
// Camille default layout
// =========================================================================

#[test]
fn camille_prints_full_machine() {
    let source = "MACHINE bridge_m1 REFINES bridge_m0 SEES bridge_ctx c2\n\
                  VARIABLES a b c\n\
                  INVARIANTS\n@inv1 a : NAT\ntheorem @thm1 a >= 0\n\
                  VARIANT 2*a+b\n\
                  EVENTS\n\
                  EVENT INITIALISATION THEN @act1 a := 0 END\n\
                  convergent EVENT ML_out REFINES ML_out ANY p q WHERE @grd1 p < a THEN @act1 a := p END\n\
                  EVENT stop WHERE @grd1 a = 0 THEN @act1 skip END\n\
                  END\n";
    let expected = "machine bridge_m1 refines bridge_m0 sees bridge_ctx c2\n\
                    \n\
                    variables a b c\n\
                    \n\
                    invariants\n\
                    \x20\x20@inv1 a ∈ ℕ\n\
                    \x20\x20theorem @thm1 a ≥ 0\n\
                    \n\
                    variant 2 ∗ a + b\n\
                    \n\
                    events\n\
                    \x20\x20event INITIALISATION\n\
                    \x20\x20\x20\x20then\n\
                    \x20\x20\x20\x20\x20\x20@act1 a ≔ 0\n\
                    \x20\x20end\n\
                    \n\
                    \x20\x20convergent event ML_out refines ML_out\n\
                    \x20\x20\x20\x20any p q\n\
                    \x20\x20\x20\x20where\n\
                    \x20\x20\x20\x20\x20\x20@grd1 p < a\n\
                    \x20\x20\x20\x20then\n\
                    \x20\x20\x20\x20\x20\x20@act1 a ≔ p\n\
                    \x20\x20end\n\
                    \n\
                    \x20\x20event stop\n\
                    \x20\x20\x20\x20where\n\
                    \x20\x20\x20\x20\x20\x20@grd1 a = 0\n\
                    \x20\x20\x20\x20then\n\
                    \x20\x20\x20\x20\x20\x20@act1 skip\n\
                    \x20\x20end\n\
                    end\n";
    assert_eq!(format_checked(source, &camille()), expected);
}

#[test]
fn camille_prints_full_context() {
    let source = "CONTEXT ctx2 EXTENDS ctx0 ctx1\n\
                  SETS S T\nCONSTANTS k f\n\
                  AXIOMS\n@axm1 partition(S, {k}, T)\ntheorem @thm1 k : S\nEND\n";
    let expected = "context ctx2 extends ctx0 ctx1\n\
                    \n\
                    sets S T\n\
                    \n\
                    constants k f\n\
                    \n\
                    axioms\n\
                    \x20\x20@axm1 partition(S, {k}, T)\n\
                    \x20\x20theorem @thm1 k ∈ S\n\
                    end\n";
    assert_eq!(format_checked(source, &camille()), expected);
}

#[test]
fn camille_prints_empty_components() {
    assert_eq!(
        format_checked("CONTEXT c END\n", &camille()),
        "context c\nend\n"
    );
    assert_eq!(
        format_checked("MACHINE m END\n", &camille()),
        "machine m\nend\n"
    );
}

#[test]
fn camille_machine_without_events_has_no_trailing_gap() {
    let source = "MACHINE m\nVARIABLES x\nINVARIANTS\n@inv1 x : NAT\nEND\n";
    let expected = "machine m\n\nvariables x\n\ninvariants\n\x20\x20@inv1 x ∈ ℕ\nend\n";
    assert_eq!(format_checked(source, &camille()), expected);
}

#[test]
fn camille_first_event_follows_events_keyword_directly() {
    // No INITIALISATION: no blank line between `events` and the first
    // event; one blank line between successive events (Camille's rule).
    let source =
        "MACHINE m\nEVENTS\nEVENT e1 THEN @act1 skip END\nEVENT e2 THEN @act1 skip END\nEND\n";
    let expected = "machine m\n\
                    \n\
                    events\n\
                    \x20\x20event e1\n\
                    \x20\x20\x20\x20then\n\
                    \x20\x20\x20\x20\x20\x20@act1 skip\n\
                    \x20\x20end\n\
                    \n\
                    \x20\x20event e2\n\
                    \x20\x20\x20\x20then\n\
                    \x20\x20\x20\x20\x20\x20@act1 skip\n\
                    \x20\x20end\n\
                    end\n";
    assert_eq!(format_checked(source, &camille()), expected);
}

#[test]
fn camille_prints_variants_first_inline() {
    let source = "MACHINE m\nVARIABLES x\nINVARIANTS\n@inv1 x : NAT\n\
                  VARIANT x @second x + 1\n\
                  EVENTS\nconvergent EVENT e THEN @act1 skip END\nEND\n";
    let output = format_checked(source, &camille());
    assert!(
        output.contains("\nvariant x\n\x20\x20@second x + 1\n"),
        "first variant inline, later ones labeled on own lines, got:\n{output}"
    );

    let labeled = "MACHINE m\nVARIANT @first 5\nEND\n";
    let output = format_checked(labeled, &camille());
    assert!(
        output.contains("\nvariant @first 5\n"),
        "labeled first variant stays inline, got:\n{output}"
    );
}

#[test]
fn camille_normalizes_when_begin_and_status() {
    let source = "MACHINE m\nEVENTS\n\
                  EVENT e1 STATUS convergent WHEN @g1 1 = 1 BEGIN @act1 skip END\n\
                  END\n";
    let output = format_checked(source, &camille());
    assert!(
        output.contains("\x20\x20convergent event e1\n"),
        "got:\n{output}"
    );
    assert!(output.contains("\x20\x20\x20\x20where\n"), "got:\n{output}");
    assert!(output.contains("\x20\x20\x20\x20then\n"), "got:\n{output}");
}

#[test]
fn camille_prints_extended_event_header() {
    let source = "MACHINE m REFINES m0\nEVENTS\nEVENT e extends e THEN @act1 skip END\nEND\n";
    let output = format_checked(source, &camille());
    assert!(
        output.contains("\x20\x20event e extends e\n"),
        "got:\n{output}"
    );
}

// =========================================================================
// Comments
// =========================================================================

#[test]
fn camille_header_comment_trails_inline_clauses() {
    let source =
        "MACHINE m REFINES m0 // the machine\nVARIABLES x\nINVARIANTS\n@inv1 x : NAT\nEND\n";
    let output = format_checked(source, &camille());
    assert!(
        output.starts_with("machine m refines m0 // the machine\n"),
        "comment must trail the complete header line, got:\n{output}"
    );
}

#[test]
fn camille_header_multiline_comment_becomes_block() {
    let source = "// first\n// second\nMACHINE m\nEND\n";
    let expected = "machine m\n\x20\x20/* first\n\x20\x20\x20\x20\x20second */\nend\n";
    assert_eq!(format_checked(source, &camille()), expected);
}

#[test]
fn camille_commented_list_item_ends_its_line() {
    let source = "MACHINE m\nVARIABLES\nx // about x\ny\nz\nINVARIANTS\n@inv1 x : NAT\nEND\n";
    let output = format_checked(source, &camille());
    assert!(
        output.contains("\nvariables x // about x\n\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20y z\n"),
        "commented item ends the line; the rest hangs under the first name, got:\n{output}"
    );
}

#[test]
fn camille_middle_commented_list_item_splits_the_line() {
    let source = "MACHINE m\nVARIABLES\nx\ny // about y\nz\nINVARIANTS\n@inv1 x : NAT\nEND\n";
    let output = format_checked(source, &camille());
    assert!(
        output.contains("\nvariables x y // about y\n\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20z\n"),
        "got:\n{output}"
    );
}

#[test]
fn camille_commented_parameter_hangs_in_any() {
    let source = "MACHINE m\nEVENTS\nEVENT e\nANY\np // param p\nq\nWHERE\n@grd1 p < q\nTHEN\n@act1 skip\nEND\nEND\n";
    let output = format_checked(source, &camille());
    assert!(
        output.contains("\x20\x20\x20\x20any p // param p\n\x20\x20\x20\x20\x20\x20\x20\x20q\n"),
        "got:\n{output}"
    );
}

#[test]
fn camille_formatting_is_idempotent_with_comments() {
    let source = "// header\nMACHINE m REFINES m0 SEES c // trailing\n\
                  VARIABLES\nx // one\ny\nINVARIANTS\n@inv1 x : NAT // inv\n\
                  EVENTS\nEVENT INITIALISATION THEN @act1 x := 0 @act2 y := 0 END\n\
                  EVENT e // event note\nANY\np // param\nWHERE\n@g p > 0\nTHEN\n@a x := p\nEND\nEND\n";
    let printer = camille();
    let once = format_str(source, &printer).unwrap();
    let twice = format_str(&once, &printer).unwrap();
    assert_eq!(once, twice, "camille formatting must be idempotent");
}

// =========================================================================
// Style toggles override the preset
// =========================================================================

#[test]
fn toggles_override_each_preset() {
    let source = "MACHINE m REFINES m0\nVARIABLES x y\nINVARIANTS\n@inv1 x : NAT\n\
                  EVENTS\nEVENT e ANY p WHERE @g p > 0 THEN @act1 skip END\nEND\n";

    let mut upper_camille = camille();
    upper_camille.keyword_case = KeywordCase::Upper;
    let output = format_checked(source, &upper_camille);
    assert!(
        output.starts_with("MACHINE m REFINES m0\n"),
        "got:\n{output}"
    );
    assert!(output.contains("\nVARIABLES x y\n"), "got:\n{output}");
    assert!(output.contains("\x20\x20\x20\x20WHERE\n"), "got:\n{output}");

    let mut per_line_camille = camille();
    per_line_camille.decl_lists = DeclListLayout::OnePerLine;
    let output = format_checked(source, &per_line_camille);
    assert!(
        output.contains("\nvariables\n\x20\x20x\n\x20\x20y\n"),
        "got:\n{output}"
    );
    assert!(
        output.contains("\x20\x20\x20\x20any\n\x20\x20\x20\x20\x20\x20p\n"),
        "got:\n{output}"
    );

    let mut dense_camille = camille();
    dense_camille.blank_between_clauses = false;
    let output = format_checked(source, &dense_camille);
    assert!(
        output.starts_with("machine m refines m0\nvariables x y\ninvariants\n"),
        "got:\n{output}"
    );

    let mut inline_rossi = PrettyPrinter::styled(Style::Rossi);
    inline_rossi.decl_lists = DeclListLayout::Inline;
    let output = format_checked(source, &inline_rossi);
    assert!(output.contains("\nVARIABLES x y\n"), "got:\n{output}");
    // The event ladder stays flat: ANY on the keyword line at one indent.
    assert!(output.contains("\n    ANY p\n"), "got:\n{output}");

    let mut airy_rossi = PrettyPrinter::styled(Style::Rossi);
    airy_rossi.blank_between_clauses = true;
    let output = format_checked(source, &airy_rossi);
    assert!(
        output.starts_with("MACHINE m\n\nREFINES\n    m0\n\nVARIABLES\n"),
        "got:\n{output}"
    );

    let mut lower_rossi = PrettyPrinter::styled(Style::Rossi);
    lower_rossi.keyword_case = KeywordCase::Lower;
    let output = format_checked(source, &lower_rossi);
    assert!(
        output.starts_with("machine m\nrefines\n    m0\nvariables\n"),
        "got:\n{output}"
    );
}

// =========================================================================
// Preset + override resolution
// =========================================================================

#[test]
fn resolved_applies_overrides_on_top_of_preset() {
    let defaults = PrettyPrinter::resolved(Style::Camille, &StyleOverrides::default());
    assert_eq!(defaults.indent, "  ");
    assert_eq!(defaults.keyword_case, KeywordCase::Lower);
    assert_eq!(defaults.decl_lists, DeclListLayout::Inline);
    assert!(defaults.blank_between_clauses);
    assert!(defaults.use_unicode);
    // Portable by default: no preset asks for Rodin's private-use glyphs.
    assert!(!defaults.private_use_glyphs);
    // Wrapping is off in every preset; only explicit overrides enable it.
    assert_eq!(defaults.max_line_width, 0);

    let overridden = PrettyPrinter::resolved(
        Style::Camille,
        &StyleOverrides {
            keyword_case: Some(KeywordCase::Upper),
            decl_lists: Some(DeclListLayout::OnePerLine),
            blank_between_clauses: Some(false),
            indent: Some("    ".to_string()),
            use_unicode: false,
            private_use_glyphs: false,
            max_line_width: 100,
        },
    );
    assert_eq!(overridden.indent, "    ");
    assert_eq!(overridden.keyword_case, KeywordCase::Upper);
    assert_eq!(overridden.decl_lists, DeclListLayout::OnePerLine);
    assert!(!overridden.blank_between_clauses);
    assert!(!overridden.use_unicode);
    assert_eq!(overridden.max_line_width, 100);

    // An explicit empty indent override is honored (the CLI's
    // `--indent=""`); only `None` follows the preset.
    let empty_indent = PrettyPrinter::resolved(
        Style::Rossi,
        &StyleOverrides {
            indent: Some(String::new()),
            ..StyleOverrides::default()
        },
    );
    assert_eq!(empty_indent.indent, "");
}
