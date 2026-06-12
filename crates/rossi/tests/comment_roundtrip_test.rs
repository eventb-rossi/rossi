//! Comment round-trip tests (issue #31).
//!
//! The textual parser attaches `//` and `/* */` comments to the element they
//! follow; the pretty printer emits them back (trailing `//` for one-liners,
//! a Camille-style `/* */` block for multiline comments). These tests pin
//! the attachment rules, the emission layout, formatting idempotence, and
//! Rodin-XML → text → Rodin-XML comment fidelity.

mod common;

use common::{assert_roundtrip, parse_context, parse_machine};
use rossi::{Component, PrettyPrinter, format_str, parse, to_string, to_string_ascii};

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
    T = {a, b} // enumerated set
CONSTANTS
    k // a constant
AXIOMS
    @axm1 k ∈ S // an axiom
END
";
    let ctx = parse_context(src);
    assert_eq!(ctx.comment.as_deref(), Some("ctx"));
    assert_eq!(ctx.sets[0].comment(), Some("deferred set"));
    assert_eq!(ctx.sets[1].comment(), Some("enumerated set"));
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
        "CONTEXT c\nAXIOMS\n    @axm1 1 = 1 // why: base\nEND\n"
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
        "CONTEXT c\nAXIOMS\n    @axm1 1 = 1\n        /* why: invariant base\n           second line */\nEND\n"
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
    let src = "MACHINE m\nEVENTS\n    EVENT e\n    ANY\n        a // first\n        b\n    WHERE\n        @grd1 a > 0 ∧ b > 0\n    THEN\n        @act1 skip\n    END\nEND\n";
    let printed = to_string(&parse(src).unwrap());
    assert!(printed.contains("        a // first\n        b\n"));

    // Without comments the joined one-line form is preserved.
    let src2 = "MACHINE m\nEVENTS\n    EVENT e\n    ANY\n        a, b\n    WHERE\n        @grd1 a > 0 ∧ b > 0\n    THEN\n        @act1 skip\n    END\nEND\n";
    let printed2 = to_string(&parse(src2).unwrap());
    assert!(printed2.contains("        a, b\n"));
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
// Rodin XML fidelity
// =========================================================================

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
                out.extend(norm(
                    &format!("set {}", s.name()),
                    &s.comment().map(str::to_string),
                ));
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
