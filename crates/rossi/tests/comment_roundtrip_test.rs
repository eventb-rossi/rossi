//! Comment round-trip tests (issue #31).
//!
//! The textual parser attaches `//` and `/* */` comments to the element they
//! follow; the pretty printer emits them back (trailing `//` for one-liners,
//! a Camille-style `/* */` block for multiline comments). These tests pin
//! the attachment rules, the emission layout, formatting idempotence, and
//! Rodin-XML → text → Rodin-XML comment fidelity.

mod common;

use common::{assert_roundtrip, parse_context, parse_machine};
use rossi::{
    Component, DeclListLayout, PrettyPrinter, Style, StyleOverrides, format_str, parse,
    parse_components, parse_xml, to_string, to_string_ascii, to_xml,
};

// =========================================================================
// Attachment rules
// =========================================================================

#[test]
fn trailing_comment_attaches_to_axiom() {
    let ctx = parse_context("CONTEXT c\nAXIOMS\n    @axm1 1 = 1 // why: base case\nEND\n");
    assert_eq!(ctx.axioms[0].comment.as_deref(), Some("why: base case"));
    assert_eq!(ctx.comment, None);
}

#[test]
fn standalone_comment_attaches_to_preceding_element() {
    // The issue's repro: a comment on its own line documents the element it
    // follows — here the context header.
    let src = "CONTEXT c\n// important: do not change\nAXIOMS\n    @axm1 1 = 1\nEND\n";
    let ctx = parse_context(src);
    assert_eq!(ctx.comment.as_deref(), Some("important: do not change"));
    assert_eq!(ctx.axioms[0].comment, None);
}

#[test]
fn comment_before_header_attaches_to_component() {
    let ctx = parse_context("// file header\nCONTEXT c\nAXIOMS\n    @axm1 1 = 1\nEND\n");
    assert_eq!(ctx.comment.as_deref(), Some("file header"));
}

#[test]
fn comments_attach_per_element_kind() {
    let src = "CONTEXT c // ctx
SETS
    S // deferred set
    T // another set
CONSTANTS
    k // a constant
AXIOMS
    @axm1 k ∈ S // an axiom
END
";
    let ctx = parse_context(src);
    assert_eq!(ctx.comment.as_deref(), Some("ctx"));
    assert_eq!(ctx.sets[0].comment.as_deref(), Some("deferred set"));
    assert_eq!(ctx.sets[1].comment.as_deref(), Some("another set"));
    assert_eq!(ctx.constants[0].comment.as_deref(), Some("a constant"));
    assert_eq!(ctx.axioms[0].comment.as_deref(), Some("an axiom"));
}

#[test]
fn comments_attach_in_machine_and_event() {
    let src = "MACHINE m // the machine
VARIABLES
    x // counts things
INVARIANTS
    @inv1 x ∈ ℕ // typing
EVENTS
    EVENT INITIALISATION // start of time
    THEN
        @act0 x ≔ 0 // zeroed
    END

    EVENT step // the event
    ANY
        y // a parameter
    WHERE
        @grd1 y > 0 // a guard
    THEN
        @act1 x ≔ x + y // an action
    END
END
";
    let m = parse_machine(src);
    assert_eq!(m.comment.as_deref(), Some("the machine"));
    assert_eq!(m.variables[0].comment.as_deref(), Some("counts things"));
    assert_eq!(m.invariants[0].comment.as_deref(), Some("typing"));
    let init = m.initialisation.as_ref().unwrap();
    assert_eq!(init.comment.as_deref(), Some("start of time"));
    assert_eq!(init.actions[0].comment.as_deref(), Some("zeroed"));
    let ev = &m.events[0];
    assert_eq!(ev.comment.as_deref(), Some("the event"));
    assert_eq!(ev.parameters[0].comment.as_deref(), Some("a parameter"));
    assert_eq!(ev.guards[0].comment.as_deref(), Some("a guard"));
    assert_eq!(ev.actions[0].comment.as_deref(), Some("an action"));
}

#[test]
fn block_comment_preserves_lines() {
    let src = "CONTEXT c\nAXIOMS\n    @axm1 1 = 1\n    /* first line\n       second line */\nEND\n";
    let ctx = parse_context(src);
    assert_eq!(
        ctx.axioms[0].comment.as_deref(),
        Some("first line\nsecond line")
    );
}

#[test]
fn consecutive_comments_join_with_newline() {
    let src = "CONTEXT c\nAXIOMS\n    @axm1 1 = 1 // first\n    // second\nEND\n";
    let ctx = parse_context(src);
    assert_eq!(ctx.axioms[0].comment.as_deref(), Some("first\nsecond"));
}

#[test]
fn whitespace_only_comments_are_dropped() {
    let ctx = parse_context("CONTEXT c //   \nAXIOMS\n    @axm1 1 = 1 /*  */\nEND\n");
    assert_eq!(ctx.comment, None);
    assert_eq!(ctx.axioms[0].comment, None);
}

#[test]
fn keywords_inside_comments_do_not_break_structure() {
    let src = "MACHINE m
VARIABLES
    x // END EVENT INVARIANTS @fake 1 = 1
INVARIANTS
    @inv1 x ∈ ℕ /* EVENT ghost
       WHERE END */
EVENTS
    EVENT step
    THEN
        @act1 x ≔ 1
    END
END
";
    let m = parse_machine(src);
    assert_eq!(m.variables.len(), 1);
    assert_eq!(m.invariants.len(), 1);
    assert_eq!(m.events.len(), 1);
    assert_eq!(m.events[0].name, "step");
    assert!(
        m.variables[0]
            .comment
            .as_deref()
            .unwrap()
            .contains("INVARIANTS")
    );
}

#[test]
fn multi_component_comments_attach_within_their_component() {
    let src = "CONTEXT c // ctx comment\nAXIOMS\n    @axm1 1 = 1\nEND\n\nMACHINE m // mch comment\nSEES\n    c\nEND\n";
    let components = rossi::parse_components(src).unwrap();
    let Component::Context(ctx) = &components[0] else {
        panic!("expected context first");
    };
    let Component::Machine(m) = &components[1] else {
        panic!("expected machine second");
    };
    assert_eq!(ctx.comment.as_deref(), Some("ctx comment"));
    assert_eq!(m.comment.as_deref(), Some("mch comment"));
}

// =========================================================================
// Printer emission
// =========================================================================

#[test]
fn single_line_comment_prints_trailing() {
    let src = "CONTEXT c\nAXIOMS\n    @axm1 1 = 1 // why: base\nEND\n";
    let printed = to_string(&parse(src).unwrap());
    assert_eq!(
        printed,
        "context c\n\naxioms\n  @axm1 1 = 1 // why: base\nend\n"
    );
}

#[test]
fn multiline_comment_prints_camille_block() {
    let mut ctx = rossi::Context::new("c".to_string());
    ctx.axioms.push(rossi::LabeledPredicate {
        label: Some("axm1".to_string()),
        is_theorem: false,
        predicate: rossi::parse_predicate_str("1 = 1").unwrap(),
        span: None,
        comment: Some("why: invariant base\nsecond line".to_string()),
    });
    let printed = to_string(&Component::Context(ctx));
    assert_eq!(
        printed,
        "context c\n\naxioms\n  @axm1 1 = 1\n    /* why: invariant base\n       second line */\nend\n"
    );
}

#[test]
fn block_close_inside_comment_is_sanitized() {
    let mut ctx = rossi::Context::new("c".to_string());
    ctx.comment = Some("has */ inside\nsecond".to_string());
    let printed = to_string(&Component::Context(ctx));
    assert!(printed.contains("* /"));
    // and it still parses back
    parse(&printed).unwrap();
}

#[test]
fn ascii_mode_does_not_touch_comment_text() {
    let src = "MACHINE m\nVARIABLES\n    x // keep <= and ∈ as written\nINVARIANTS\n    @inv1 x ∈ ℕ\nEND\n";
    let printed = to_string_ascii(&parse(src).unwrap());
    assert!(printed.contains("// keep <= and ∈ as written"));
    assert!(printed.contains("x : NAT"));
}

#[test]
fn commented_parameters_print_one_per_line() {
    // The rossi (one-per-line) layout: each parameter gets its own line, so a
    // trailing comment re-attaches to its parameter on reparse.
    let rossi_style = PrettyPrinter::styled(Style::Rossi);
    let src = "MACHINE m\nEVENTS\n    EVENT e\n    ANY\n        a // first\n        b\n    WHERE\n        @grd1 a > 0 ∧ b > 0\n    THEN\n        @act1 skip\n    END\nEND\n";
    let printed = rossi_style.print_component(&parse(src).unwrap());
    assert!(printed.contains("        a // first\n        b\n"));

    // Uncommented parameters split one per line too — newlines are ordinary
    // whitespace in the structural-list grammar, so the output reparses.
    let src2 = "MACHINE m\nEVENTS\n    EVENT e\n    ANY\n        a b\n    WHERE\n        @grd1 a > 0 ∧ b > 0\n    THEN\n        @act1 skip\n    END\nEND\n";
    let printed2 = rossi_style.print_component(&parse(src2).unwrap());
    assert!(
        printed2.contains("        a\n        b\n"),
        "got:\n{printed2}"
    );
    let reprinted = rossi_style.print_component(&parse(&printed2).unwrap());
    assert_eq!(printed2, reprinted, "one-per-line ANY is not a fixed point");
}

#[test]
fn empty_xml_param_comment_does_not_force_multiline() {
    // Regression (import_corpus): a Rodin parameter can carry an *empty*
    // `comment=""` attribute, which import stores as `Some("")`. A blank comment
    // renders nothing, so it must not push the ANY block to one-param-per-line —
    // otherwise the first print is multi-line while the reparse (blank → None)
    // prints the joined single-line form, breaking the import round-trip. The
    // textual parser never produces `Some("")`, so this is XML-only.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.event name="e">
        <org.eventb.core.parameter identifier="a" org.eventb.core.comment=""/>
        <org.eventb.core.parameter identifier="b"/>
        <org.eventb.core.guard label="grd1" predicate="a &gt; 0 ∧ b &gt; 0"/>
        <org.eventb.core.action label="act1" assignment="a ≔ a"/>
    </org.eventb.core.event>
</org.eventb.core.machineFile>"#;

    let component = parse_xml(xml).expect("xml parses");
    // Import really did attach the blank comment (`Some("")`), the trigger.
    let Component::Machine(m) = &component else {
        panic!("expected machine");
    };
    assert_eq!(m.events[0].parameters[0].comment.as_deref(), Some(""));

    // First print joins the parameters on one line (the blank renders nothing)…
    let text1 = to_string(&component);
    assert!(
        text1.contains("    any a b\n"),
        "ANY block should be single-line, got:\n{text1}"
    );
    // …and the text round-trips: reparse + reprint is a fixed point.
    let text2 = to_string(&parse(&text1).expect("reparse"));
    assert_eq!(text1, text2, "import round-trip not idempotent:\n{text1}");
}

#[test]
fn real_xml_param_comment_still_prints_per_line_and_roundtrips() {
    // A genuine (non-blank) parameter comment must still force the per-line
    // layout — so the trailing comment re-attaches on reparse — and round-trip.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.event name="e">
        <org.eventb.core.parameter identifier="a" org.eventb.core.comment="why a"/>
        <org.eventb.core.parameter identifier="b"/>
        <org.eventb.core.guard label="grd1" predicate="a &gt; 0 ∧ b &gt; 0"/>
        <org.eventb.core.action label="act1" assignment="a ≔ a"/>
    </org.eventb.core.event>
</org.eventb.core.machineFile>"#;

    let text1 = to_string(&parse_xml(xml).expect("xml parses"));
    assert!(
        text1.contains("    any a // why a\n        b\n"),
        "commented param ends its line; the rest hangs, got:\n{text1}"
    );
    let text2 = to_string(&parse(&text1).expect("reparse"));
    assert_eq!(
        text1, text2,
        "commented round-trip not idempotent:\n{text1}"
    );
}

// =========================================================================
// Round-trip and idempotence
// =========================================================================

#[test]
fn roundtrip_preserves_comments_in_ast() {
    for src in [
        "CONTEXT c // ctx\nSETS\n    S // set\nCONSTANTS\n    k // konst\nAXIOMS\n    @axm1 k ∈ S // ax\nEND\n",
        "MACHINE m // mch\nVARIABLES\n    x // var\nINVARIANTS\n    @inv1 x ∈ ℕ // inv\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act0 x ≔ 0 // zero\n    END\nEND\n",
        "CONTEXT c\nAXIOMS\n    @axm1 1 = 1\n    /* multi\n       line */\nEND\n",
    ] {
        assert_roundtrip(src);
    }
}

#[test]
fn format_is_idempotent_with_comments() {
    let printer = PrettyPrinter::new();
    for src in [
        "CONTEXT c\n// important: do not change\nAXIOMS\n    @axm1 1 = 1 // why: invariant base\nEND\n",
        "CONTEXT c\nAXIOMS\n    @axm1 1 = 1 // first\n    // second\nEND\n",
        "MACHINE m // mch\nVARIABLES\n    x /* one\n    two */\nINVARIANTS\n    @inv1 x ∈ ℕ\nEND\n",
    ] {
        let once = format_str(src, &printer).unwrap();
        let twice = format_str(&once, &printer).unwrap();
        assert_eq!(once, twice, "format not idempotent for:\n{src}");
    }
}

// =========================================================================
// Positional fidelity: `fmt` must not move a comment
// =========================================================================

/// Every comment in `src` as `(is_trailing, text, elements_before_it)`.
///
/// Positions are described relative to the surrounding elements rather than by
/// offset, so the signature is invariant under everything formatting is allowed
/// to change — indentation, wrapping, keyword case, blank lines.
fn comment_signature(src: &str) -> Vec<(bool, String, usize)> {
    let mut element_starts: Vec<usize> = Vec::new();
    for component in &parse_components(src).expect("parses") {
        collect_element_starts(component, &mut element_starts);
    }
    element_starts.sort_unstable();

    rossi::comments::comment_spans(src)
        .iter()
        .filter_map(|span| {
            // Only the lexical half is shared with the printer; the position
            // logic below is deliberately re-derived, so a bug in the
            // placements cannot cancel itself out in the assertion.
            let (_, text) = rossi::comments::comment_body(&src[span.start..span.end])?;
            let trailing = src[..span.start]
                .rsplit('\n')
                .next()
                .is_some_and(|line| !line.trim().is_empty());
            let before = element_starts.partition_point(|start| *start < span.start);
            Some((trailing, text, before))
        })
        .collect()
}

fn collect_element_starts(component: &Component, out: &mut Vec<usize>) {
    let mut push = |span: Option<rossi::ast::Span>| {
        if let Some(span) = span {
            out.push(span.start);
        }
    };
    match component {
        Component::Context(ctx) => {
            for set in &ctx.sets {
                push(set.span);
            }
            for constant in &ctx.constants {
                push(constant.span);
            }
            for axiom in &ctx.axioms {
                push(axiom.span);
            }
        }
        Component::Machine(m) => {
            for variable in &m.variables {
                push(variable.span);
            }
            for invariant in &m.invariants {
                push(invariant.span);
            }
            for variant in &m.variants {
                push(variant.span);
            }
            if let Some(init) = &m.initialisation {
                push(init.span);
                for action in &init.actions {
                    push(action.span);
                }
            }
            for event in &m.events {
                push(event.span);
                for parameter in &event.parameters {
                    push(parameter.span);
                }
                for lp in event
                    .guards
                    .iter()
                    .chain(&event.with)
                    .chain(&event.witnesses)
                {
                    push(lp.span);
                }
                for action in &event.actions {
                    push(action.span);
                }
            }
        }
    }
}

/// Formatting must leave every comment's text, marker and position alone, and
/// be a fixed point.
fn assert_comments_stay_put(src: &str, printer: &PrettyPrinter) {
    let once = format_str(src, printer).unwrap_or_else(|e| panic!("format {src:?}: {e}"));
    assert_eq!(
        comment_signature(src),
        comment_signature(&once),
        "fmt moved or rewrote a comment:\n--- before ---\n{src}--- after ---\n{once}"
    );
    let twice = format_str(&once, printer).expect("reformat");
    assert_eq!(once, twice, "fmt is not idempotent for:\n{src}");
}

#[test]
fn comments_keep_their_place_across_every_slot() {
    let sources = [
        // A banner above a clause keyword. It used to become a `/* */` block
        // trailing the last invariant.
        "machine m\nvariables x\ninvariants\n  @inv1 x ∈ ℕ\n\n////////////////\n// EVENTS\n////////////////\n\nevents\n  event INITIALISATION\n  then\n    @act0 x ≔ 0\n  end\nend\n",
        // A file header, and a footer after the final `end`. The footer used
        // to migrate backwards across three closing scopes onto the last
        // action — enough to turn a stray `-` into a coverage-exclusion
        // marker, which is how at least one downstream tool reads it.
        "// licence line one\n// licence line two\n\nmachine m\nvariables x\ninvariants\n  @inv1 x ∈ ℕ\nevents\n  event INITIALISATION\n  then\n    @act0 x ≔ 0\n  end\nend\n// footer\n",
        // Two separate comments in different clauses: they used to merge into
        // one block on the invariant.
        "machine m\nvariables x\ninvariants\n  @inv1 x ∈ ℕ\n\n// about the events\n\nevents\n  // about this one\n  event INITIALISATION\n  then\n    @act0 x ≔ 0\n  end\nend\n",
        // Trailing and own-line comments on consecutive lines stay distinct.
        "context c\naxioms\n  @axm1 1 = 1 // first\n  // second\nend\n",
        // Inside a declaration list, which Camille prints inline.
        "context c\nsets\n  S // about S\n  // about T\n  T\nconstants k\naxioms\n  @axm1 k ∈ S\nend\n",
        // A block comment keeps its `/* */` marker.
        "context c\naxioms\n  @axm1 1 = 1\n  /* first line\n     second line */\nend\n",
        // Before a closing `end`, inside an event.
        "machine m\nvariables x\ninvariants\n  @inv1 x ∈ ℕ\nevents\n  event INITIALISATION\n  then\n    @act0 x ≔ 0\n  // before end\n  end\nend\n",
        // Two components with a comment between them.
        "context c\naxioms\n  @axm1 1 = 1\nend\n\n// between the components\n\nmachine m\nsees c\nvariables x\ninvariants\n  @inv1 x ∈ ℕ\nend\n",
    ];
    for src in sources {
        for printer in comment_bearing_printers() {
            assert_comments_stay_put(src, &printer);
        }
    }
}

/// The printer configurations a comment's placement can actually depend on.
///
/// `comment_place` has to predict which clause keywords get a line of their
/// own, so the layout axes that move a keyword onto or off a shared line are
/// exactly where the two models can drift apart: the style preset, the
/// declaration-list layout crossed against it, and a width narrow enough that
/// an inline list wraps to a hanging line.
fn comment_bearing_printers() -> Vec<PrettyPrinter> {
    let mut printers = Vec::new();
    for style in [Style::Camille, Style::Rossi] {
        for decl_lists in [
            None,
            Some(DeclListLayout::Inline),
            Some(DeclListLayout::OnePerLine),
        ] {
            for max_line_width in [0, 28] {
                printers.push(PrettyPrinter::resolved(
                    style,
                    &StyleOverrides {
                        decl_lists,
                        max_line_width,
                        ..StyleOverrides::default()
                    },
                ));
            }
        }
    }
    printers
}

/// Format in Camille style, assert the result is a fixed point, and return it.
fn format_camille(src: &str) -> String {
    let printer = PrettyPrinter::styled(Style::Camille);
    let once = format_str(src, &printer).expect("formats");
    assert_eq!(
        once,
        format_str(&once, &printer).expect("reformat"),
        "fmt is not idempotent for:\n{src}"
    );
    once
}

#[test]
fn comment_on_an_inline_clause_keyword_line_is_kept() {
    // Camille prints `sets`/`constants`/`variables` with their names on the
    // keyword's line, so the keyword line has no room left for a trailing
    // comment: it used to be filed under the clause and then never rendered,
    // which dropped it. It is lifted above the keyword instead, where a
    // reparse files it under the same clause.
    let src = "context c\nconstants // the interesting ones\n  a b\naxioms\n  @axm1 a ∈ ℕ\nend\n";
    let printer = PrettyPrinter::styled(Style::Camille);
    let once = format_str(src, &printer).expect("formats");
    assert!(
        once.contains("// the interesting ones\nconstants a b\n"),
        "keyword comment lost:\n{once}"
    );
}

#[test]
fn comment_above_the_first_name_does_not_split_the_list() {
    // The comment belongs above the whole `constants a b` line, so it must not
    // force `a` to end its line — doing so made the second pass rejoin the
    // names and `fmt --check` fail on `fmt`'s own output.
    let src = "context c\nconstants\n  // pick these\n  a b\naxioms\n  @axm1 a ∈ ℕ\nend\n";
    let once = format_camille(src);
    assert!(
        once.contains("// pick these\nconstants a b\n"),
        "list split around the comment:\n{once}"
    );
}

#[test]
fn a_second_comment_on_one_line_is_kept_above_what_follows() {
    // Only one comment can trail a printed line; the extra one used to be
    // dropped on the floor. It is filed above the next line instead.
    let src = "context c\naxioms\n  @axm1 1 = 1 /* first */ /* second */\n  @axm2 2 = 2\nend\n";
    let once = format_camille(src);
    assert!(once.contains("/* first */"), "first comment lost:\n{once}");
    assert!(
        once.contains("/* second */"),
        "second comment lost:\n{once}"
    );
}

#[test]
fn comment_on_the_variant_keyword_line_follows_the_variant() {
    // Camille prints `variant` on the first variant's line, so a comment
    // written after the keyword belongs there too; it used to drift backwards
    // onto the preceding invariant.
    let src = "machine m\nvariables x\ninvariants\n  @inv1 x ∈ ℕ\nvariant // why this measure\n  x\nevents\n  event INITIALISATION\n  then\n    @act0 x ≔ 0\n  end\nend\n";
    let once = format_camille(src);
    assert!(
        once.contains("variant x // why this measure"),
        "comment left the variant:\n{once}"
    );
}

#[test]
fn licence_header_survives_formatting_verbatim() {
    // examples/base-model.eventb opens with a 14-line Apache header. It used
    // to be joined into one `/* */` block *below* `context C1`, which is both
    // a move and a marker some Event-B text front-ends cannot parse.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/base-model.eventb");
    let src = std::fs::read_to_string(&path).expect("read example");
    let header: String = src.lines().take(15).map(|l| format!("{l}\n")).collect();
    let formatted = format_str(&src, &PrettyPrinter::styled(Style::Camille)).expect("format");
    assert!(
        formatted.starts_with(&header),
        "licence header not preserved, got:\n{}",
        &formatted[..header.len().min(formatted.len())]
    );
    assert_comments_stay_put(&src, &PrettyPrinter::styled(Style::Camille));
}

#[test]
fn corpus_comments_never_move_under_formatting() {
    // Every archive EVENTB_CORPUS_DIR points at, imported to text and then
    // formatted: the whole point of the feature, over thousands of comments
    // written by hand. Skipped unless that variable is set.
    let Some(dir) = std::env::var_os("EVENTB_CORPUS_DIR") else {
        eprintln!("EVENTB_CORPUS_DIR is not set — skipping");
        return;
    };
    let printer = PrettyPrinter::styled(Style::Camille);
    let mut checked = 0;
    let entries = std::fs::read_dir(&dir).expect("read corpus dir");
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "zip") {
            continue;
        }
        let Ok(named) = rossi::parse_zip_file(&path) else {
            continue; // not a Rodin archive, or one rossi cannot read
        };
        for nc in &named {
            let text = to_string(&nc.component);
            if rossi::comments::comment_spans(&text).is_empty() {
                continue;
            }
            // Some corpus models use reserved words as identifiers and do not
            // reparse; that is a separate limitation, not a comment one.
            if format_str(&text, &printer).is_err() {
                continue;
            }
            assert_comments_stay_put(&text, &printer);
            checked += 1;
        }
    }
    assert!(checked > 0, "no corpus model was checked — bad corpus dir?");
}

// =========================================================================
// Rodin XML fidelity
// =========================================================================

#[test]
fn variant_comment_stays_on_the_variant() {
    // A variant was not a comment anchor, so its own trailing comment was
    // re-homed onto the preceding invariant — in the text *and* in the XML.
    // Rodin's IVariant is an ICommentedElement, so the variant can hold it.
    let src = "MACHINE m
VARIABLES
    x
INVARIANTS
    @inv1 x ∈ ℕ
VARIANT x // why this measure
EVENTS
    EVENT INITIALISATION
    THEN
        @act0 x ≔ 0
    END
END
";
    let m = parse_machine(src);
    assert_eq!(m.invariants[0].comment, None, "note must not land on @inv1");
    assert_eq!(m.variants[0].comment.as_deref(), Some("why this measure"));

    let xml = to_xml(&Component::Machine(m));
    assert!(
        xml.contains("org.eventb.core.variant")
            && xml.contains("org.eventb.core.comment=\"why this measure\""),
        "variant comment missing from XML:\n{xml}"
    );
    let Component::Machine(back) = parse_xml(&xml).expect("xml reparses") else {
        panic!("expected machine");
    };
    assert_eq!(
        back.variants[0].comment.as_deref(),
        Some("why this measure")
    );
}

#[test]
fn witness_comment_survives_export_to_xml() {
    // `write_witness_xml` was the one element writer that never emitted
    // `org.eventb.core.comment`, so a witness comment was visible in text and
    // absent from the `.bum` — `fmt` and `export` disagreed about the same
    // file. Rodin's `IWitness` is an `ICommentedElement`, so the attribute is
    // legal there.
    let src = "MACHINE m REFINES a
VARIABLES
    x
INVARIANTS
    @inv1 x ∈ ℕ
EVENTS
    EVENT INITIALISATION
    THEN
        @act0 x ≔ 0
    END

    EVENT dec REFINES d
    ANY
        p
    WHERE
        @grd1 p ∈ ℕ
    WITH
        @q p = q // why this witness
    THEN
        @act1 x ≔ p
    END
END
";
    let component = parse(src).expect("parses");
    let xml = to_xml(&component);
    assert!(
        xml.contains("org.eventb.core.witness")
            && xml.contains("org.eventb.core.comment=\"why this witness\""),
        "witness comment missing from XML:\n{xml}"
    );

    let back = parse_xml(&xml).expect("xml reparses");
    let Component::Machine(m) = &back else {
        panic!("expected machine");
    };
    let ev = m
        .events
        .iter()
        .find(|e| e.name == "dec")
        .expect("event dec");
    let witness = ev.with.first().or_else(|| ev.witnesses.first());
    assert_eq!(
        witness.and_then(|w| w.comment.as_deref()),
        Some("why this witness"),
    );
}

/// All comments of a component in traversal order, normalized, with a label
/// describing the carrying element (so mismatches are readable).
fn collect_comments(component: &Component) -> Vec<(String, String)> {
    let norm = |owner: &str, c: &Option<String>| -> Option<(String, String)> {
        c.as_deref()
            .and_then(rossi::comments::normalize_comment)
            .map(|t| (owner.to_string(), t))
    };
    let mut out = Vec::new();
    match component {
        Component::Context(ctx) => {
            out.extend(norm(&format!("context {}", ctx.name), &ctx.comment));
            for s in &ctx.sets {
                out.extend(norm(&format!("set {}", s.name), &s.comment));
            }
            for c in &ctx.constants {
                out.extend(norm(&format!("constant {}", c.name), &c.comment));
            }
            for a in &ctx.axioms {
                out.extend(norm(&format!("axiom {:?}", a.label), &a.comment));
            }
        }
        Component::Machine(m) => {
            out.extend(norm(&format!("machine {}", m.name), &m.comment));
            for v in &m.variables {
                out.extend(norm(&format!("variable {}", v.name), &v.comment));
            }
            for i in &m.invariants {
                out.extend(norm(&format!("invariant {:?}", i.label), &i.comment));
            }
            for v in &m.variants {
                out.extend(norm(&format!("variant {:?}", v.label), &v.comment));
            }
            if let Some(init) = &m.initialisation {
                out.extend(norm("initialisation", &init.comment));
                for a in &init.actions {
                    out.extend(norm(&format!("init action {:?}", a.label), &a.comment));
                }
            }
            for e in &m.events {
                out.extend(norm(&format!("event {}", e.name), &e.comment));
                for p in &e.parameters {
                    out.extend(norm(&format!("param {}", p.name), &p.comment));
                }
                for g in e.guards.iter().chain(&e.with).chain(&e.witnesses) {
                    out.extend(norm(&format!("predicate {:?}", g.label), &g.comment));
                }
                for a in &e.actions {
                    out.extend(norm(&format!("action {:?}", a.label), &a.comment));
                }
            }
        }
    }
    out
}

/// Import a Rodin zip, print to text, reparse, and require every normalized
/// comment to survive on the same element.
fn assert_zip_comments_roundtrip(path: &std::path::Path) {
    let named =
        rossi::parse_zip_file(path).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
    let mut total = 0;
    for nc in &named {
        let expected = collect_comments(&nc.component);
        let text = to_string(&nc.component);
        let reparsed = parse(&text).unwrap_or_else(|e| {
            panic!(
                "reparse {} from {}: {e}\n{text}",
                nc.filename,
                path.display()
            )
        });
        let actual = collect_comments(&reparsed);
        assert_eq!(
            expected,
            actual,
            "comment mismatch in {} from {}\nprinted:\n{}",
            nc.filename,
            path.display(),
            text
        );
        total += expected.len();
    }
    assert!(
        total > 0,
        "{} carries no comments — bad fixture?",
        path.display()
    );
}

#[test]
fn base_model_zip_comments_survive_text_roundtrip() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/base-model.zip");
    assert_zip_comments_roundtrip(&path);
}

#[test]
fn corpus_wellcommented_comments_survive_text_roundtrip() {
    // External corpus stressor (25 Rodin comments incl. multiline + unicode).
    // Skipped unless EVENTB_CORPUS_DIR points at the model collection.
    let Some(dir) = std::env::var_os("EVENTB_CORPUS_DIR") else {
        eprintln!("EVENTB_CORPUS_DIR is not set — skipping");
        return;
    };
    let path = std::path::Path::new(&dir).join("evbt_wellcommented.zip");
    if !path.is_file() {
        eprintln!("{} not found — skipping", path.display());
        return;
    }
    assert_zip_comments_roundtrip(&path);
}
