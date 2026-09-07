//! Tests for syntax error recovery
//!
//! These tests verify that the parser can recover from syntax errors
//! and produce partial ASTs with error information.

use rossi::{Component, ParseError, parse, parse_components_with_recovery, parse_with_recovery};
use test_case::test_case;

/// Unwrap the recovered component as a context, failing the test otherwise.
fn expect_context(result: &rossi::ParseResult<Component>) -> &rossi::ast::Context {
    match &result.component {
        Some(Component::Context(ctx)) => ctx,
        other => panic!("Expected a Context component, got {other:?}"),
    }
}

/// Unwrap the recovered component as a machine, failing the test otherwise.
fn expect_machine(result: &rossi::ParseResult<Component>) -> &rossi::ast::Machine {
    match &result.component {
        Some(Component::Machine(m)) => m,
        other => panic!("Expected a Machine component, got {other:?}"),
    }
}

#[test]
fn test_recovery_context_with_invalid_axiom() {
    let source = r#"
    CONTEXT test
    SETS
        MySet
    CONSTANTS
        c1
    AXIOMS
        @axm1 c1 = 10
        @axm2 invalid syntax $$$ here
        @axm3 c1 > 0
    END
    "#;

    let result = parse_with_recovery(source);

    // Should have recovered with errors
    assert!(result.has_recovered(), "Expected recovery with errors");
    assert!(!result.errors.is_empty(), "Expected at least one error");

    // Should have a partial component
    let ctx = expect_context(&result);
    assert_eq!(ctx.name, "test");
    assert_eq!(ctx.sets.len(), 1);
    assert_eq!(ctx.sets[0].name, "MySet");
    assert_eq!(ctx.constants.len(), 1);
    assert_eq!(ctx.constants[0].name, "c1");
    // Should have recovered the valid axioms
    assert!(!ctx.axioms.is_empty(), "Should have at least some axioms");
}

#[test]
fn test_recovery_machine_with_invalid_invariant() {
    let source = r#"
    MACHINE test
    VARIABLES
        x y
    INVARIANTS
        @inv1 x >= 0
        @inv2 invalid @#$ syntax
        @inv3 y >= 0
    END
    "#;

    let result = parse_with_recovery(source);

    // Should have recovered
    assert!(result.has_recovered(), "Expected recovery with errors");

    let m = expect_machine(&result);
    assert_eq!(m.name, "test");
    assert_eq!(m.variables.len(), 2);
    assert_eq!(m.variables[0].name, "x");
    assert_eq!(m.variables[1].name, "y");
    // Should have recovered some invariants
    assert!(
        !m.invariants.is_empty(),
        "Should have at least some invariants"
    );
}

#[test]
fn recovery_records_declaration_spans() {
    // A recovered declaration carries the byte span of its name, so navigation
    // and symbol providers resolve it even inside a component the strict parse
    // rejected. The trailing `∈` forces the machine into recovery.
    let source = "MACHINE m\nVARIABLES\n    counter\nINVARIANTS\n    @i counter ∈\nEND\n";
    let result = parse_with_recovery(source);

    let m = expect_machine(&result);
    assert_eq!(m.variables.len(), 1);
    let span = m.variables[0]
        .span
        .expect("recovered variable carries a span");
    // The span covers exactly the declared `counter` (line 2), not the use in
    // the broken invariant.
    assert_eq!(span.start, source.find("    counter\n").unwrap() + 4);
    assert_eq!(&source[span.start..span.end], "counter");
}

#[test]
fn recovery_accepts_underscore_names_like_strict_parser() {
    let strict_source = "\
MACHINE _m
SEES
    _ctx
VARIABLES
    _x
INVARIANTS
    @i _x ∈ ℤ
EVENTS
    EVENT _evt
    ANY
        _p
    WHERE
        @g _p = _x
    END
END
";
    let recovered_source = strict_source.replace("@i _x ∈ ℤ", "@i _x ∈");

    let strict = parse(strict_source).expect("strict parsing accepts leading underscores");
    let recovered = parse_with_recovery(&recovered_source);
    let strict = match strict {
        Component::Machine(machine) => machine,
        other => panic!("expected a strict machine, got {other:?}"),
    };
    let recovered = expect_machine(&recovered);

    assert_eq!(recovered.name, strict.name);
    assert_eq!(recovered.sees, strict.sees);
    assert_eq!(recovered.variables[0].name, strict.variables[0].name);
    assert_eq!(recovered.events[0].name, strict.events[0].name);
    assert_eq!(
        recovered.events[0].parameters[0].name,
        strict.events[0].parameters[0].name
    );

    for declaration in [&recovered.variables[0], &recovered.events[0].parameters[0]] {
        let span = declaration
            .span
            .expect("recovered declaration carries a span");
        assert_eq!(&recovered_source[span.start..span.end], declaration.name);
    }
}

#[test]
fn recovery_accepts_structural_keyword_declarations_like_strict_parser() {
    let strict_machine = "MACHINE m\nVARIABLES\n    end\nINVARIANTS\n    @i end ∈ ℤ\nEND\n";
    parse(strict_machine).expect("strict parsing accepts `end` as the first variable");
    let broken_machine = strict_machine.replace("@i end ∈ ℤ", "@i end ∈");
    let recovered = parse_with_recovery(&broken_machine);
    let machine = expect_machine(&recovered);
    assert_eq!(machine.variables[0].name, "end");

    let strict_context = "CONTEXT c\nCONSTANTS\n    end\nAXIOMS\n    @a end = end\nEND\n";
    parse(strict_context).expect("strict parsing accepts `end` as the first constant");
    let broken_context = strict_context.replace("@a end = end", "@a end =");
    let recovered = parse_with_recovery(&broken_context);
    let context = expect_context(&recovered);
    assert_eq!(context.constants[0].name, "end");

    let strict_event = "\
MACHINE m
EVENTS
    EVENT e
    ANY
        end
    WHERE
        @g end = end
    END
END
";
    parse(strict_event).expect("strict parsing accepts `end` as the first parameter");
    let broken_event = strict_event.replace("@g end = end", "@g end =");
    let recovered = parse_with_recovery(&broken_event);
    let machine = expect_machine(&recovered);
    assert_eq!(machine.events[0].parameters[0].name, "end");
}

#[test]
fn recovery_still_rejects_noncanonical_and_reserved_names() {
    let source = "\
MACHINE 1m
SEES
    1ctx a--b ä
VARIABLES
    1x ä x-y dom _x
INVARIANTS
    @i _x ∈
EVENTS
    EVENT a--b
    ANY
        1p ä p-q dom _p
    WHERE
        @g _p = _x
    END
END
";

    let recovered = parse_with_recovery(source);
    let machine = expect_machine(&recovered);
    assert_eq!(machine.name, "unknown");
    assert!(machine.sees.is_empty());
    let variables: Vec<&str> = machine
        .variables
        .iter()
        .map(|variable| variable.name.as_str())
        .collect();
    assert_eq!(variables, ["_x"]);
    assert_eq!(machine.events[0].name, "unknown");
    let parameters: Vec<&str> = machine.events[0]
        .parameters
        .iter()
        .map(|parameter| parameter.name.as_str())
        .collect();
    assert_eq!(parameters, ["_p"]);
}

#[test]
fn recovery_does_not_treat_component_or_reference_names_as_clauses() {
    let component_name = "MACHINE VARIABLES\nINVARIANTS\n    @i 1 =\nEND\n";
    let recovered = parse_with_recovery(component_name);
    let machine = expect_machine(&recovered);
    assert_eq!(machine.name, "VARIABLES");
    assert!(machine.variables.is_empty());

    let reference_name = "\
MACHINE m
SEES
    VARIABLES
INVARIANTS
    @i 1 =
END
";
    let recovered = parse_with_recovery(reference_name);
    let machine = expect_machine(&recovered);
    assert_eq!(machine.sees, ["VARIABLES"]);
    assert!(machine.variables.is_empty());
}

#[test]
fn recovery_does_not_readmit_later_structural_keywords_as_names() {
    let source = "\
MACHINE m
VARIABLES x VARIABLES
INVARIANTS
    @i x ∈
END
";
    let recovered = parse_with_recovery(source);
    let machine = expect_machine(&recovered);
    let variables: Vec<&str> = machine
        .variables
        .iter()
        .map(|variable| variable.name.as_str())
        .collect();
    assert_eq!(variables, ["x"]);
}

#[test]
fn recovery_does_not_protect_a_name_after_an_invalid_leading_comma() {
    let source = "\
MACHINE m
VARIABLES ,
INVARIANTS
    @i x ∈
END
";
    let recovered = parse_with_recovery(source);
    let machine = expect_machine(&recovered);
    assert!(machine.variables.is_empty());
}

#[test]
fn recovery_scopes_event_names_targets_and_any_clauses() {
    let source = "\
MACHINE m
VARIABLES
    x
INVARIANTS
    @broken x ∈
EVENTS
    EVENT ANY
    WHERE
        @g x = any
    END
    EVENT refined REFINES any
    WHERE
        @g x = any
    END
    EVENT target_end REFINES END
    END
END
";
    let recovered = parse_with_recovery(source);
    let machine = expect_machine(&recovered);
    assert_eq!(machine.events.len(), 3);
    assert!(
        machine
            .events
            .iter()
            .all(|event| event.parameters.is_empty())
    );

    for event in &machine.events {
        let span = event.span.expect("recovered event carries a span");
        let event_text = &source[span.start..span.end];
        assert!(event_text.trim_end().ends_with("END"));
        assert_eq!(event_text.matches("\n    END").count(), 1, "{event_text:?}");
    }
}

#[test]
fn recovery_handles_multibyte_whitespace_before_keyword_named_clauses() {
    let source = "MACHINE\u{a0}m\nVARIABLES\n    x\nINVARIANTS\n    @i x ∈\nEND\n";
    let recovered = parse_with_recovery(source);

    let machine = expect_machine(&recovered);
    assert_eq!(machine.variables[0].name, "x");
}

#[test]
fn recovery_accepts_event_as_an_event_name() {
    let source = "\
MACHINE m
INVARIANTS
    @broken 1 =
EVENTS
    EVENT EVENT
    END
END
";
    let recovered = parse_with_recovery(source);

    let machine = expect_machine(&recovered);
    assert_eq!(machine.events.len(), 1);
    assert_eq!(machine.events[0].name, "EVENT");
}

#[test]
fn recovery_uses_component_specific_clause_boundaries() {
    let strict_source = "\
MACHINE m
VARIABLES x CONSTANTS
INVARIANTS
    @i x = x
END
";
    let recovered_source = strict_source.replace("@i x = x", "@i x =");
    let Component::Machine(strict) = parse(strict_source).expect("strict source parses") else {
        panic!("expected a machine");
    };
    let recovered = parse_with_recovery(&recovered_source);
    let recovered = expect_machine(&recovered);

    let strict_names: Vec<&str> = strict.variables.iter().map(|v| v.name.as_str()).collect();
    let recovered_names: Vec<&str> = recovered
        .variables
        .iter()
        .map(|v| v.name.as_str())
        .collect();
    assert_eq!(recovered_names, strict_names);
    assert_eq!(recovered_names, ["x", "CONSTANTS"]);
}

#[test]
fn recovery_distinguishes_formula_keywords_from_machine_clauses() {
    let strict_source = "\
MACHINE m
INVARIANTS
    @valid SEES = SEES
VARIABLES x
END
";
    let recovered_source = strict_source.replace("@valid", "@broken x ∈\n    @valid");
    let Component::Machine(strict) = parse(strict_source).expect("strict source parses") else {
        panic!("expected a machine");
    };
    let recovered = parse_with_recovery(&recovered_source);
    let recovered = expect_machine(&recovered);

    assert_eq!(strict.variables[0].name, "x");
    assert_eq!(recovered.variables[0].name, "x");
}

#[test]
fn recovery_keeps_keyword_named_variables_before_the_event_section() {
    for name in ["EVENTS", "EVENT", "INITIALISATION"] {
        let strict_source =
            format!("MACHINE m\nVARIABLES {name} x\nINVARIANTS\n    @i {name} = {name}\nEND\n");
        parse(&strict_source).expect("strict source parses");
        let recovered_source =
            strict_source.replace(&format!("@i {name} = {name}"), &format!("@i {name} ="));
        let recovered = parse_with_recovery(&recovered_source);
        let machine = expect_machine(&recovered);
        let names: Vec<&str> = machine.variables.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, [name, "x"], "failed for {name}");
        assert!(machine.events.is_empty(), "failed for {name}");
    }
}

#[test]
fn recovery_preserves_event_header_metadata() {
    for (keyword, extended) in [("REFINES", false), ("EXTENDS", true)] {
        let source = format!(
            "MACHINE m\nINVARIANTS\n    @broken 1 =\nEVENTS\n    EVENT e {keyword} abstract\n    WHERE\n        @g 1 = 1\n    END\nEND\n"
        );
        let recovered = parse_with_recovery(&source);
        let event = &expect_machine(&recovered).events[0];

        assert_eq!(
            event.refines.first().map(|t| t.name.as_str()),
            Some("abstract")
        );
        assert_eq!(event.extended, extended);
        let span = event.refines[0].span.expect("target span recovered");
        assert_eq!(&source[span.start..span.end], "abstract");
    }
}

#[test]
fn recovery_keeps_formula_keywords_inside_event_clauses() {
    for strict_source in [
        "MACHINE m\nVARIABLES then x\nINVARIANTS @i x ∈ ℤ\nEVENTS EVENT e WHERE @g then = then THEN @a x := then END END\n",
        "MACHINE m\nVARIABLES end x\nINVARIANTS @i x ∈ ℤ\nEVENTS EVENT e WHERE @g end = end THEN @a x := x END END\n",
        "MACHINE m\nVARIABLES event x\nINVARIANTS @i x ∈ ℤ\nEVENTS EVENT e WHERE @g event = event THEN @a x := x END END\n",
    ] {
        let recovered_source = strict_source.replace("@i x ∈ ℤ", "@i x ∈");
        parse(strict_source).expect("strict source parses");
        let recovered = parse_with_recovery(&recovered_source);
        let event = &expect_machine(&recovered).events[0];

        assert_eq!(
            event.guards.len(),
            1,
            "{strict_source}\nevent={event:?}\nerrors={:?}",
            recovered.errors
        );
        assert_eq!(event.actions.len(), 1, "{strict_source}\nevent={event:?}");
        let span = event.span.expect("event span recovered");
        assert_eq!(&recovered_source[span.end - "END".len()..span.end], "END");
    }
}

#[test]
fn recovery_does_not_apply_named_targets_to_initialisation() {
    let source = "\
MACHINE m
INVARIANTS
    @broken 1 =
EVENTS
    EVENT INITIALISATION REFINES END
END
";
    let recovered = parse_with_recovery(source);
    let init = expect_machine(&recovered)
        .initialisation
        .as_ref()
        .expect("initialisation recovered");
    let span = init.span.expect("initialisation span recovered");

    assert_eq!(
        source[span.start..span.end].trim(),
        "EVENT INITIALISATION REFINES END"
    );
}

#[test]
fn multi_component_recovery_ignores_keyword_named_declarations_as_headers() {
    let source = "\
MACHINE m1
VARIABLES
    MACHINE
INVARIANTS
    @i MACHINE = MACHINE
END

MACHINE m2
INVARIANTS
    @j 1 =
END
";
    let recovered = parse_components_with_recovery(source);
    let components = recovered.component.expect("components recovered");

    assert_eq!(components.len(), 2);
    let first = components
        .iter()
        .find_map(|component| match component {
            Component::Machine(machine) if machine.name == "m1" => Some(machine),
            _ => None,
        })
        .expect("first machine recovered");
    assert_eq!(first.variables[0].name, "MACHINE");
}

#[test]
fn trailing_operator_does_not_flag_the_following_predicate() {
    // `@a … ∈` with nothing after `∈` makes the strict parser consume across the
    // newline into `@b`'s label, so its error points at the innocent `@b`.
    // Recovery flags `@a` precisely; the misleading strict error must be dropped,
    // leaving exactly one error, located on `@a`'s predicate (not `@b`).
    let source = "MACHINE m\nVARIABLES\n    x\n    y\nINVARIANTS\n    @a x ∈\n    @b y ∈ ℕ\nEND\n";
    let result = parse_with_recovery(source);

    assert!(result.has_recovered());
    assert_eq!(
        result.errors.len(),
        1,
        "exactly one error expected, got {:?}",
        result.errors
    );

    let span = result.errors[0]
        .span()
        .expect("the surviving error carries a span");
    let b_label = source.find("@b").unwrap();
    assert!(
        span.end <= b_label,
        "error span {span:?} must stay on @a's predicate, not reach @b at {b_label}"
    );
    assert!(source[span.start..span.end].contains("@a"));
}

#[test]
fn recovery_spans_and_clause_regions_are_absolute_in_multi_component_files() {
    use rossi::keywords::KeywordId;

    // In a merged file a recovered declaration's span and clause regions must be
    // shifted out of their per-component region into absolute document
    // coordinates. Here C0 is healthy and M0 is broken, so M0 recovers from a
    // non-zero region offset.
    let source = "CONTEXT C0\nCONSTANTS\n    k\nEND\n\nMACHINE M0\nVARIABLES\n    counter\nINVARIANTS\n    @i counter ∈\nEND\n";
    let result = parse_components_with_recovery(source);

    let components = result.component.expect("recovered components");
    let machine = components
        .iter()
        .find_map(|c| match c {
            Component::Machine(m) => Some(m),
            Component::Context(_) => None,
        })
        .expect("machine M0 recovered");
    assert_eq!(machine.variables.len(), 1);
    let span = machine.variables[0]
        .span
        .expect("recovered variable carries a span");
    assert_eq!(span.start, source.find("    counter\n").unwrap() + 4);
    assert_eq!(&source[span.start..span.end], "counter");

    let vars = machine
        .clauses
        .iter()
        .find(|c| c.keyword == KeywordId::Variables)
        .expect("variables region recovered");
    // The region indexes into the full source at the second component, not the
    // per-region slice.
    assert!(
        source[vars.span.start..vars.span.end].starts_with("VARIABLES"),
        "got {:?}",
        &source[vars.span.start..vars.span.end]
    );
    assert!(vars.span.start > source.find("MACHINE M0").unwrap());
}

#[test]
fn test_recovery_multiline_invariant_isolates_the_broken_one() {
    // Idiomatic Event-B writes each invariant as a `@label` on one line with
    // the predicate indented below. A syntax error in one predicate must not
    // be blamed on the surrounding (correct) invariants: recovery segments by
    // label, not by physical line, so it no longer lights up the whole block.
    let source = r#"
MACHINE m
VARIABLES
    x
INVARIANTS
    @CommonRole1
        x ∈ ℕ
    @CommonRole2
        x ≥ 0
    @CommonRole3
        x ≠ 1
    @CommonRole4
        ∀u · u ∈ ℕ sdfsdf x ≠ u
    @CommonRole5
        x ≤ 100
END
"#;

    let result = parse_with_recovery(source);
    assert!(result.has_recovered(), "expected recovery with errors");

    // The four correct invariants survive; only the broken one is dropped.
    let m = expect_machine(&result);
    assert_eq!(
        m.invariants.len(),
        4,
        "the four correct invariants should be recovered, got {:?}",
        m.invariants
    );

    // Exactly one diagnostic survives: the precise strict-parse error on the
    // offending token, not a coarse per-label recovery error. The duplicate
    // recovery error covering the same predicate is collapsed away.
    assert_eq!(
        result.errors.len(),
        1,
        "exactly one diagnostic should remain, got {:?}",
        result.errors
    );
    assert!(
        !matches!(result.errors[0], rossi::ParseError::RecoverableError { .. }),
        "the surviving diagnostic should be the precise strict error, got {:?}",
        result.errors[0]
    );

    // It points at the broken predicate's line, never spilling over the block.
    let bad = source.find("sdfsdf").expect("fixture has the bad token");
    let bad_line = source[..bad].matches('\n').count() + 1;
    let (line, _) = result.errors[0]
        .position()
        .expect("the strict error has a position");
    assert_eq!(
        line, bad_line,
        "the diagnostic should sit on the broken line"
    );
}

#[test]
fn test_recovery_context_missing_end() {
    let source = r#"
    CONTEXT test
    SETS
        MySet
    CONSTANTS
        c1
    "#;

    let result = parse_with_recovery(source);

    // Should have some errors due to missing END
    assert!(!result.errors.is_empty());

    // Should still recover what it can
    let ctx = expect_context(&result);
    assert_eq!(ctx.name, "test");
    assert_eq!(ctx.sets.len(), 1);
    assert_eq!(ctx.constants.len(), 1);
}

#[test]
fn test_successful_parse_no_errors() {
    let source = r#"
    CONTEXT perfect
    SETS
        MySet
    CONSTANTS
        c1
    AXIOMS
        @axm1 c1 = 10
    END
    "#;

    let result = parse_with_recovery(source);

    // Should succeed with no errors
    assert!(result.is_ok(), "Expected no errors");
    assert!(result.errors.is_empty(), "Expected empty error list");

    let ctx = expect_context(&result);
    assert_eq!(ctx.name, "perfect");
    assert_eq!(ctx.sets.len(), 1);
    assert_eq!(ctx.constants.len(), 1);
    assert_eq!(ctx.axioms.len(), 1);
}

#[test]
fn test_recovery_result_api() {
    // The ParseResult accessor surface on a clean parse. into_component and
    // into_result each consume the result, so each gets a fresh parse.
    let source = r#"
    CONTEXT test
    END
    "#;

    let result = parse_with_recovery(source);
    assert!(result.is_ok());
    assert!(!result.is_err());
    assert!(!result.has_recovered());
    assert!(result.get_errors().is_empty());

    let component = parse_with_recovery(source).into_component();
    assert!(component.is_some());

    let result = parse_with_recovery(source).into_result();
    assert!(result.is_ok(), "Valid input should convert to Ok result");
}

#[test]
fn test_recovery_with_commas_in_lists() {
    let source = r#"
    CONTEXT comma_test
    SETS
        Set1,
        Set2,
        Set3
    CONSTANTS
        c1, c2, c3
    END
    "#;

    let result = parse_with_recovery(source);

    // A comma between declared names is invalid, but recovery still salvages the
    // individual identifiers from the list rather than dropping the whole clause.
    let ctx = expect_context(&result);
    assert_eq!(ctx.name, "comma_test");
    assert!(ctx.sets.len() >= 2, "Should recover multiple sets");
    assert!(
        ctx.constants.len() >= 2,
        "Should recover multiple constants"
    );
}

#[test]
fn test_recovery_unknown_component_type() {
    // An undispatchable header — an unknown component keyword or free prose —
    // must fail completely since we can't determine the component type.
    for source in [
        r#"
    UNKNOWN something
    SETS
        MySet
    END
    "#,
        "this is not Event-B at all",
    ] {
        let result = parse_with_recovery(source);
        assert!(result.is_err(), "Expected complete failure for {source:?}");
        assert!(result.component.is_none(), "no component for {source:?}");
    }
}

// ============================================================================
// Issue #24: a colon inside a comment must not derail recovery.
//
// The LSP runs `parse_with_recovery` on every edit; each test below plants one
// unrelated syntax error (a stray `+` in CONSTANTS) to force the recovery path,
// then checks that comments never produce spurious "Failed to parse" errors.
// ============================================================================

/// Errors other than the initial strict-parse error (always `errors[0]`).
fn recovery_errors(result: &rossi::ParseResult<Component>) -> Vec<String> {
    result.errors[1..].iter().map(|e| e.to_string()).collect()
}

// One scaffold for the issue-#24 label-handling family: an optional SETS
// clause and the AXIOMS block under test are spliced into a context whose
// stray `+` in CONSTANTS forces the recovery path; recovery must then parse
// the axioms without spurious errors and with the expected labels/flags.
//
// A colon inside a `//` comment must not flag valid axioms.
#[test_case(
    None,
    "@axm1 c1 = 1 // note: positive\n        @axm2 c1 = 1 // plain comment without it",
    &[(Some("axm1"), false), (Some("axm2"), false)]
    ; "colon_in_comment_axiom_not_flagged"
)]
// The ASCII spelling of ∈ is `:`; it must not act as a label separator.
#[test_case(Some("S"), "@axm1 c1 : S", &[(Some("axm1"), false)] ; "ascii_membership_with_at_label")]
// Colons inside a block comment must not produce errors either.
#[test_case(
    None,
    "/* background: this constant\n           is the answer: to everything */\n        @axm1 c1 = 42",
    &[(Some("axm1"), false)]
    ; "block_comment_with_colon"
)]
// The trailing colon belongs to the label in both strict parsing and recovery.
#[test_case(None, "@axm1: c1 = 1", &[(Some("axm1:"), false)] ; "trailing_colon_label_matches_strict_parser")]
// `@axm1//note` is a complete label per the grammar; masking must not
// truncate it into an unparseable `@axm1` stub.
#[test_case(None, "@axm1//note c1 = 1", &[(Some("axm1//note"), false)] ; "comment_markers_in_label_no_spurious_error")]
// The grammar allows `theorem @label P` and `@label theorem P` inline;
// recovery must parse both and set the theorem flag.
#[test_case(
    None,
    "@axm1 c1 = 1\n        theorem @thm1 c1 > 0\n        @thm2 theorem c1 < 2",
    &[(Some("axm1"), false), (Some("thm1"), true), (Some("thm2"), true)]
    ; "inline_theorem_forms"
)]
fn test_recovery_issue24_label_family(
    sets: Option<&str>,
    axioms: &str,
    expected: &[(Option<&str>, bool)],
) {
    let sets_clause = sets.map_or_else(String::new, |names| format!("    SETS\n        {names}\n"));
    let source = format!(
        "\n    CONTEXT issue24\n{sets_clause}    CONSTANTS\n        c1\n        +\n    AXIOMS\n        {axioms}\n    END\n    "
    );

    let result = parse_with_recovery(&source);

    assert!(result.has_recovered(), "Expected recovery with errors");
    let extra = recovery_errors(&result);
    assert!(extra.is_empty(), "Expected no extra errors, got: {extra:?}");

    let ctx = expect_context(&result);
    let flags: Vec<(Option<&str>, bool)> = ctx
        .axioms
        .iter()
        .map(|a| (a.label.as_deref(), a.is_theorem))
        .collect();
    assert_eq!(flags, expected);
}

#[test]
fn test_recovery_comment_only_lines_no_errors() {
    let source = r#"
    MACHINE issue24_comment_lines
    VARIABLES
        x
        +
    INVARIANTS
        // TODO: tighten this bound
        @inv1 x >= 0
    END
    "#;

    let result = parse_with_recovery(source);

    let extra = recovery_errors(&result);
    assert!(
        extra.is_empty(),
        "Comment-only lines must not produce errors, got: {extra:?}"
    );

    let m = expect_machine(&result);
    assert_eq!(m.invariants.len(), 1);
    assert_eq!(m.invariants[0].label.as_deref(), Some("inv1"));
}

#[test]
fn test_recovery_identifiers_ignore_comment_text() {
    let source = r#"
    CONTEXT issue24_identifier_leak
    CONSTANTS
        c1, c2 // alias: c3, c4
        +
    END
    "#;

    let result = parse_with_recovery(source);

    let ctx = expect_context(&result);
    let names: Vec<&str> = ctx.constants.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["c1", "c2"], "comment text must not leak constants");
}

#[test]
fn test_recovery_clause_keyword_in_comment_does_not_truncate() {
    let source = r#"
    MACHINE issue24_keyword_in_comment
    VARIABLES
        x, // see INVARIANTS: below
        y
        +
    INVARIANTS
        @inv1 x >= 0
    END
    "#;

    let result = parse_with_recovery(source);

    let m = expect_machine(&result);
    let names: Vec<&str> = m.variables.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(
        names,
        ["x", "y"],
        "clause keyword in a comment must not end the VARIABLES clause"
    );
    assert_eq!(m.invariants.len(), 1);
}

#[test]
fn test_recovery_dispatch_ignores_component_keyword_in_comment() {
    // "CONTEXT" appearing only inside a comment must not flip a broken
    // machine into context recovery.
    let source = r#"
    MACHINE issue24_dispatch // refines the CONTEXT: below
    VARIABLES
        x
        +
    END
    "#;

    let result = parse_with_recovery(source);

    assert!(
        matches!(result.component, Some(Component::Machine(_))),
        "Expected machine recovery, got {:?}",
        result.component
    );
}

#[test]
fn test_recovery_keyword_inside_identifier_does_not_truncate_clause() {
    // `trend` contains END and `offsets` contains SETS; neither may end
    // or start a clause.
    let source = r#"
    MACHINE issue24_trend
    VARIABLES
        x,
        trend,
        offsets
        +
    INVARIANTS
        @inv1 x >= 0
    END
    "#;

    let result = parse_with_recovery(source);

    let m = expect_machine(&result);
    let names: Vec<&str> = m.variables.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["x", "trend", "offsets"]);
    assert_eq!(m.invariants.len(), 1);
}

#[test]
fn test_recovery_sees_context_identifier_does_not_flip_dispatch() {
    // `context_defs` contains CONTEXT as a substring, and `context` is the
    // keyword as a whole word; neither may flip the broken machine into
    // context recovery — the machine header comes first in the text.
    for name in ["context_defs", "context"] {
        let source = format!(
            "\n    MACHINE issue24_sees\n    SEES\n        {name}\n    VARIABLES\n        x\n        +\n    END\n    "
        );

        let result = parse_with_recovery(&source);

        let m = expect_machine(&result);
        assert_eq!(m.name, "issue24_sees", "failed for {name}");
        assert_eq!(m.sees, [name], "failed for {name}");
        assert_eq!(m.variables.len(), 1, "failed for {name}");
    }
}

#[test]
fn test_recovery_error_names_label_with_position() {
    // Two broken axioms. The strict parse pinpoints the first, so its recovery
    // duplicate is collapsed away; recovery still reports the second one, with
    // a byte-exact position and a concise, label-named message (no masked
    // comment artifacts).
    let source =
        "CONTEXT issue24_position\nAXIOMS\n    @axm1 $$$\n    @axm2 ### // why: broken\nEND\n";

    let result = parse_with_recovery(source);

    let error = result
        .errors
        .iter()
        .find_map(|e| match e {
            rossi::ParseError::RecoverableError {
                line,
                column,
                message,
                ..
            } => Some((*line, *column, message.clone())),
            _ => None,
        })
        .expect("the second broken axiom must be reported by recovery");

    let (line, column, message) = error;
    assert_eq!(line, 4, "1-indexed line of the second broken axiom");
    assert_eq!(column, 5, "1-indexed column of the axiom text");
    assert_eq!(message, "Failed to parse axiom: @axm2");
}

#[test]
fn test_recovery_span_is_byte_exact_under_multibyte() {
    // A RecoverableError's byte span must slice exactly the offending
    // `@label … predicate`, no matter how many multibyte (`∀ ∈ ℕ`) or astral
    // (`𝔹`) characters precede it or sit inside it. The span is assembled from
    // byte offsets (pointer arithmetic over the masked text), so a byte/char
    // mismatch would mis-slice it — this pins the byte coordinate.
    //
    // Two broken axioms: the strict parse pinpoints the first, collapsing its
    // recovery duplicate, so recovery's own span survives on the second.
    let source = "CONTEXT c\nAXIOMS\n    @axm1 ∀x·x∈ℕ $$$\n    @axm2 ∀y·y∈ℕ ### 𝔹\nEND\n";

    let result = parse_with_recovery(source);

    let span = result
        .errors
        .iter()
        .find_map(|e| match e {
            rossi::ParseError::RecoverableError {
                message,
                span: Some(span),
                ..
            } if message.contains("@axm2") => Some(*span),
            _ => None,
        })
        .expect("recovery reports the second broken axiom with a byte span");

    assert_eq!(
        &source[span.start..span.end],
        "@axm2 ∀y·y∈ℕ ### 𝔹",
        "the span must slice exactly the @axm2 predicate"
    );
    // The span begins past the multibyte first axiom: the preceding `∀ ∈ ℕ`
    // bytes do not shift the offset off the `@axm2` boundary.
    assert!(
        span.start > source.find("@axm1").expect("fixture has @axm1"),
        "span starts after the first axiom, not before it"
    );
}

#[test]
fn test_recovery_survives_bom_before_header() {
    // A UTF-8 BOM (not whitespace, common in Windows-saved files) before
    // MACHINE must not defeat recovery dispatch.
    let source =
        "\u{feff}MACHINE bom_machine\nVARIABLES\n    x\n    +\nINVARIANTS\n    @inv1 x >= 0\nEND\n";

    let result = parse_with_recovery(source);

    let m = expect_machine(&result);
    assert_eq!(m.name, "bom_machine");
    assert_eq!(m.variables.len(), 1);
    assert_eq!(m.invariants.len(), 1);
}

#[test]
fn test_recovery_inline_clause_content() {
    // Identifiers and predicates written on the clause keyword's own line
    // must be recovered like any other.
    let source = r#"
    CONTEXT inline_clauses
    CONSTANTS c1 c2
        +
    AXIOMS @axm1 c1 = 1
        @axm2 c2 = 2
    END
    "#;

    let result = parse_with_recovery(source);

    let ctx = expect_context(&result);
    let names: Vec<&str> = ctx.constants.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["c1", "c2"]);
    let labels: Vec<&str> = ctx
        .axioms
        .iter()
        .filter_map(|a| a.label.as_deref())
        .collect();
    assert_eq!(labels, ["axm1", "axm2"]);
}

#[test]
fn test_recovery_event_refines_is_not_machine_refines() {
    // An event-level REFINES must not be recovered as the machine's
    // refinement target (the machine here refines nothing).
    let source = r#"
    MACHINE event_refines
    VARIABLES
        x
        +
    EVENTS
        EVENT inc REFINES inc_abs
        WHERE
            @grd1 x > 0
        THEN
            @act1 x := x + 1
        END
    END
    "#;

    let result = parse_with_recovery(source);

    let m = expect_machine(&result);
    assert_eq!(
        m.refines, None,
        "event-level REFINES leaked into the machine header"
    );
    assert_eq!(m.variables.len(), 1);
}

#[test]
fn test_recovery_keyword_inside_label_does_not_truncate_clause() {
    // `@safety-END` is one label (any non-whitespace after `@`); the END
    // inside it must not terminate the INVARIANTS clause scan.
    let source = r#"
    MACHINE label_keyword
    VARIABLES
        x
        +
    INVARIANTS
        @safety-END x > 0
        @inv2 x < 10
    END
    "#;

    let result = parse_with_recovery(source);

    let m = expect_machine(&result);
    let labels: Vec<&str> = m
        .invariants
        .iter()
        .filter_map(|i| i.label.as_deref())
        .collect();
    assert_eq!(labels, ["safety-END", "inv2"]);
    assert!(
        recovery_errors(&result).is_empty(),
        "no spurious error for the keyword-bearing label: {:?}",
        recovery_errors(&result)
    );
}

#[test]
fn test_recovery_set_error_does_not_leak_into_axioms_issue_32() {
    // Issue #32, example 2: a malformed SETS line (`BOOK: READER`) must be
    // reported once, at the SETS line, and must NOT produce spurious errors on
    // the well-formed axioms that follow — nothing in AXIOMS references SETS.
    let source = concat!(
        "CONTEXT library_ctx\n",
        "EXTENDS\n",
        "    base_ctx\n",
        "SETS\n",
        "    BOOK: READER\n",
        "CONSTANTS\n",
        "    max_loans\n",
        "AXIOMS\n",
        "    @axm1: max_loans = 5\n",
        "    @axm2: max_loans > 0\n",
        "END\n",
    );

    let result = parse_components_with_recovery(source);

    // Exactly one error, anchored to the SETS line (line 5).
    assert_eq!(
        result.errors.len(),
        1,
        "the SETS error must not leak into the axioms, got {:?}",
        result.errors
    );
    let line = match &result.errors[0] {
        rossi::ParseError::PestError { line, .. } => *line,
        other => panic!("expected a PestError at the SETS line, got {other:?}"),
    };
    assert_eq!(line, 5, "error must point at the malformed SETS line");

    // Both axioms recover cleanly.
    let components = result
        .component
        .expect("a partial context must be recovered");
    let ctx = match &components[..] {
        [Component::Context(ctx)] => ctx,
        other => panic!("expected a single recovered context, got {other:?}"),
    };
    let labels: Vec<&str> = ctx
        .axioms
        .iter()
        .filter_map(|a| a.label.as_deref())
        .collect();
    assert_eq!(
        labels,
        vec!["axm1:", "axm2:"],
        "both axioms must be recovered"
    );
}

#[test]
fn test_recovery_rejects_enumerated_sets_without_promoting_elements() {
    let source = concat!(
        "CONTEXT phases\n",
        "SETS\n",
        "    CYCLE_PHASE = {idle, washing, complete}\n",
        "END\n",
    );

    let result = parse_with_recovery(source);
    assert!(result.has_recovered(), "expected a recovered syntax error");
    assert_eq!(result.errors.len(), 1, "expected one syntax error");

    let ctx = expect_context(&result);
    let set_names: Vec<&str> = ctx.sets.iter().map(|set| set.name.as_str()).collect();
    assert_eq!(set_names, ["CYCLE_PHASE"]);
}

#[test]
fn test_recovery_does_not_read_ascii_membership_as_a_label() {
    // `:` is the ASCII spelling of ∈, so `c1 : S` is a membership predicate
    // with no label — never label `c1` with predicate `S`.
    let source = "\n    CONTEXT issue24\n    SETS\n        S\n    CONSTANTS\n        c1\n        +\n    AXIOMS\n        c1 : S\n    END\n    ";

    let result = parse_with_recovery(source);
    let extra = recovery_errors(&result);
    assert_eq!(extra.len(), 1, "the bare predicate fails, got {extra:?}");
    assert!(
        expect_context(&result).axioms.is_empty(),
        "`c1` is not a label, so nothing recovers"
    );
}

#[test]
fn test_recovery_reports_each_label_less_predicate_on_its_own_line() {
    // Label-less predicates are a parse error, one per item. Label-anchored
    // segmentation finds no `@` here, so it must fall back to a per-line split:
    // lumping the lines into one segment would report a single failure for the
    // pair, and blame the wrong text for it.
    let source = r#"
CONTEXT c
SETS
    S $ broken
AXIOMS
    1 ∈ ℕ
    2 ∈ ℕ
END
"#;

    let result = parse_with_recovery(source);
    assert!(result.has_recovered(), "expected recovery with errors");

    let extra = recovery_errors(&result);
    assert_eq!(extra.len(), 2, "one failure per bare line, got {extra:?}");
    // Both keep the strict parser's own error, so the editor can hang the
    // insert-label fix off the rule code rather than a generic message.
    assert!(
        result
            .errors
            .iter()
            .filter(|e| matches!(e, ParseError::MissingLabel { .. }))
            .count()
            == 2,
        "both bare lines report a missing label, got {:?}",
        result.errors
    );

    // The bare membership spelling is the reason the split matters: `1 ∈ ℕ` is
    // a predicate with no label, never label `1` with predicate `∈ ℕ`.
    let ctx = expect_context(&result);
    assert!(
        ctx.axioms.is_empty(),
        "an unlabeled axiom is not recovered, got {:?}",
        ctx.axioms
    );
}

#[test]
fn test_recovery_unicode_whitespace_before_label_does_not_panic() {
    // A multibyte Unicode whitespace (U+00A0, no-break space) between the clause
    // keyword and a labelled predicate must not crash recovery: the segment-start
    // scan walks chars, not raw bytes, so it never slices inside a multibyte
    // whitespace. The grammar now treats U+00A0 as whitespace (Rodin parity), so
    // here the trailing `$$$` is what fails the strict parse and drives recovery;
    // the recovery byte-scanners stay ASCII-only and still walk over the U+00A0.
    let source = "CONTEXT c\nAXIOMS\n\u{a0}theorem @thm1 broken $$$\nEND\n";
    // Reaching this assertion at all is the regression check — the byte-index
    // scan this once used panicked here. Recovery should also report the broken
    // predicate against a partial parse.
    let result = parse_with_recovery(source);
    assert!(
        result.has_recovered(),
        "recovery should report the broken predicate, not panic"
    );
}

#[test]
fn test_recovery_multiline_theorem_orderings_stay_whole() {
    // Label-anchored segmentation keeps a theorem-flagged predicate whole across
    // a line break in both grammar orderings: a leading `theorem @label` (the
    // segment start is pulled back over the keyword) and a trailing
    // `@label theorem`, each with the predicate indented on the next line. A
    // broken CONSTANTS clause forces recovery into the AXIOMS.
    let source = r#"
    CONTEXT c
    CONSTANTS
        c1
        +
    AXIOMS
        theorem @thm1
            c1 = 1
        @thm2 theorem
            c1 < 2
    END
    "#;

    let result = parse_with_recovery(source);

    let extra = recovery_errors(&result);
    assert!(extra.is_empty(), "Expected no extra errors, got: {extra:?}");

    let ctx = expect_context(&result);
    let flags: Vec<(Option<&str>, bool)> = ctx
        .axioms
        .iter()
        .map(|a| (a.label.as_deref(), a.is_theorem))
        .collect();
    assert_eq!(flags, [(Some("thm1"), true), (Some("thm2"), true)]);
}

#[test]
fn test_recovery_mixed_labelled_and_bare_predicates() {
    // A clause that mixes a leading bare predicate with labelled ones splits
    // into one segment per item: the leading segment (clause keyword to first
    // label) carries the bare predicate, and each label opens its own —
    // possibly multi-line — segment. Only `@axm1` recovers; the bare predicate
    // and the broken `@axm2` are reported one apiece.
    let source = r#"
    CONTEXT c
    CONSTANTS
        c1
        +
    AXIOMS
        c0 = 0
        @axm1
            c1 = 1
        @axm2 $$$
    END
    "#;

    let result = parse_with_recovery(source);

    let ctx = expect_context(&result);
    let labels: Vec<Option<&str>> = ctx.axioms.iter().map(|a| a.label.as_deref()).collect();
    assert_eq!(labels, [Some("axm1")], "only @axm1 recovers");

    let extra = recovery_errors(&result);
    assert_eq!(
        extra.len(),
        2,
        "the bare predicate and @axm2 each fail, got {extra:?}"
    );
    assert!(
        extra[1].contains("@axm2"),
        "the second failure names @axm2, got: {}",
        extra[1]
    );
}

#[test]
fn recovery_records_events_variant_and_clause_regions() {
    use rossi::keywords::KeywordId;

    // The broken invariant forces the whole machine into recovery; the events,
    // variant, and clause regions past it must still be recovered so structural
    // LSP features (folding, outline) survive a syntax error mid-edit.
    let source = "\
MACHINE m
VARIABLES
    x
INVARIANTS
    @i invalid @#$ syntax
VARIANT
    x
EVENTS
    EVENT INITIALISATION
    THEN
        @a x := 0
    END
    EVENT step
    WHEN
        @g x > 0
    THEN
        @b x := 0
    END
END
";
    let result = parse_with_recovery(source);
    assert!(result.has_recovered(), "expected recovery with errors");
    let m = expect_machine(&result);

    // The named event recovered, with a span covering its whole block.
    assert_eq!(m.events.len(), 1, "one named event, got {:?}", m.events);
    assert_eq!(m.events[0].name, "step");
    let evt_span = m.events[0].span.expect("event carries a span");
    let evt_text = &source[evt_span.start..evt_span.end];
    assert!(evt_text.starts_with("EVENT step"), "got {evt_text:?}");
    assert!(evt_text.trim_end().ends_with("END"), "got {evt_text:?}");

    // The INITIALISATION recovered, its name span on the keyword.
    let init = m.initialisation.as_ref().expect("initialisation recovered");
    let name = init.name_span.expect("init name span");
    assert_eq!(&source[name.start..name.end], "INITIALISATION");

    // The variant expression recovered (best effort).
    assert!(!m.variants.is_empty(), "variant recovered");

    // Clause regions cover variant and events; the EVENTS region ends at the
    // last event's END, not the machine END.
    let keywords: Vec<KeywordId> = m.clauses.iter().map(|c| c.keyword).collect();
    assert!(keywords.contains(&KeywordId::Variant), "got {keywords:?}");
    assert!(keywords.contains(&KeywordId::Events), "got {keywords:?}");
    let events = m
        .clauses
        .iter()
        .find(|c| c.keyword == KeywordId::Events)
        .expect("events region");
    assert!(source[events.span.start..events.span.end].starts_with("EVENTS"));
    // The region ends at the last event's END; the machine's own END follows it.
    assert!(
        source[events.span.end..].contains("END"),
        "EVENTS region must end before the machine END"
    );
}

#[test]
fn recovery_spans_the_component_block() {
    // A broken single component still carries a block span from its header
    // keyword through its last content, so block-level consumers (e.g. folding)
    // anchor it even when the strict parse failed.
    let source = "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @i broken @#$ syntax\nEND\n";
    let result = parse_with_recovery(source);
    let m = expect_machine(&result);
    let span = m.span.expect("recovered machine carries a block span");
    let text = &source[span.start..span.end];
    assert!(text.starts_with("MACHINE"), "got {text:?}");
    assert!(text.ends_with("END"), "got {text:?}");
    // The span stops at the final END, not the trailing newline.
    assert_eq!(span.end, source.trim_end().len());
}

// The two tests below pin the EXACT clause regions (keyword + byte-precise source
// slice) and recovered payloads for a fully-populated broken context and machine.
// They characterise current recovery behaviour so a clause-scan refactor can be
// proven byte-for-byte behaviour-preserving: the same regions and payloads must
// survive a change to how recovery scans clauses.

#[test]
fn recovery_context_clause_regions_and_payloads_characterized() {
    use rossi::keywords::KeywordId;

    // The broken @axm2 forces the whole context into recovery; every context
    // clause kind (EXTENDS/SETS/CONSTANTS/AXIOMS/THEOREMS) is present so the
    // recorded clause regions cover the full set. EXTENDS and SETS carry two
    // whitespace-separated names each to pin exact multi-name recovery.
    let source = "\
CONTEXT characterized
EXTENDS base1 base2
SETS
    S T
CONSTANTS
    k
AXIOMS
    @axm1 k ∈ S
    @axm2 invalid @#$ syntax
THEOREMS
    @thm1 k = k
END
";
    let result = parse_with_recovery(source);
    assert!(result.has_recovered(), "expected recovery with errors");
    let ctx = expect_context(&result);

    // Clause regions: exact keyword order and byte-exact source slices.
    let regions: Vec<(KeywordId, &str)> = ctx
        .clauses
        .iter()
        .map(|c| (c.keyword, &source[c.span.start..c.span.end]))
        .collect();
    assert_eq!(
        regions,
        vec![
            (KeywordId::Extends, "EXTENDS base1 base2"),
            (KeywordId::Sets, "SETS\n    S T"),
            (KeywordId::Constants, "CONSTANTS\n    k"),
            (
                KeywordId::Axioms,
                "AXIOMS\n    @axm1 k ∈ S\n    @axm2 invalid @#$ syntax",
            ),
            (KeywordId::Theorems, "THEOREMS\n    @thm1 k = k"),
        ],
    );

    // Recovered payloads: names from declaration clauses, predicates from AXIOMS
    // and THEOREMS (the latter flagged, lowered into the same axioms vec).
    assert_eq!(ctx.extends, vec!["base1".to_string(), "base2".to_string()]);
    let set_names: Vec<&str> = ctx.sets.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(set_names, ["S", "T"]);
    assert_eq!(ctx.constants.len(), 1);
    assert_eq!(ctx.constants[0].name, "k");
    let axioms: Vec<(Option<&str>, bool)> = ctx
        .axioms
        .iter()
        .map(|a| (a.label.as_deref(), a.is_theorem))
        .collect();
    // @axm1 parses, @axm2 fails (dropped), @thm1 recovered as a flagged theorem.
    assert_eq!(axioms, vec![(Some("axm1"), false), (Some("thm1"), true)]);
}

#[test]
fn recovery_machine_clause_regions_and_payloads_characterized() {
    use rossi::keywords::KeywordId;

    // The broken @inv2 forces the whole machine into recovery; every
    // machine-level clause kind is present, plus an events section, so the
    // recorded clause regions cover the full set (REFINES through EVENTS).
    let source = "\
MACHINE characterized
REFINES m0
SEES c0
VARIABLES
    v
INVARIANTS
    @inv1 v ∈ ℕ
    @inv2 invalid @#$ syntax
THEOREMS
    @thm1 v = v
VARIANT
    v
EVENTS
    EVENT INITIALISATION
    THEN
        @act1 v := 0
    END
    EVENT step
    WHEN
        @grd1 v > 0
    THEN
        @act2 v := v
    END
END
";
    let result = parse_with_recovery(source);
    assert!(result.has_recovered(), "expected recovery with errors");
    let m = expect_machine(&result);

    // Clause regions through VARIANT are byte-exact. EVENTS is recorded by a
    // separate path (recover_events, untouched by the clause-scan refactor), so
    // it is only checked for its keyword and that it opens on the keyword.
    let regions: Vec<(KeywordId, &str)> = m
        .clauses
        .iter()
        .map(|c| (c.keyword, &source[c.span.start..c.span.end]))
        .collect();
    assert_eq!(regions.len(), 7, "got {regions:?}");
    assert_eq!(
        &regions[..6],
        &[
            (KeywordId::Refines, "REFINES m0"),
            (KeywordId::Sees, "SEES c0"),
            (KeywordId::Variables, "VARIABLES\n    v"),
            (
                KeywordId::Invariants,
                "INVARIANTS\n    @inv1 v ∈ ℕ\n    @inv2 invalid @#$ syntax",
            ),
            (KeywordId::Theorems, "THEOREMS\n    @thm1 v = v"),
            (KeywordId::Variant, "VARIANT\n    v"),
        ],
    );
    assert_eq!(regions[6].0, KeywordId::Events);
    assert!(regions[6].1.starts_with("EVENTS"), "got {:?}", regions[6].1);

    // Recovered payloads.
    assert_eq!(m.refines.as_deref(), Some("m0"));
    assert_eq!(m.sees, vec!["c0".to_string()]);
    assert_eq!(m.variables.len(), 1);
    assert_eq!(m.variables[0].name, "v");
    let invariants: Vec<(Option<&str>, bool)> = m
        .invariants
        .iter()
        .map(|i| (i.label.as_deref(), i.is_theorem))
        .collect();
    // @inv1 parses, @inv2 fails (dropped), @thm1 recovered as a flagged theorem.
    assert_eq!(
        invariants,
        vec![(Some("inv1"), false), (Some("thm1"), true)],
    );
    assert!(!m.variants.is_empty(), "variant expression recovered");
}

#[test]
fn recovery_extracts_any_clause_parameters() {
    // A broken guard forces the whole machine into error recovery.  The
    // recovered event must still expose its ANY-clause parameters with byte-
    // exact spans so goto-definition and semantic tokens keep working.
    let source = "\
MACHINE m
VARIABLES
    v
EVENTS
    EVENT step
    ANY
        container
    WHERE
        @grd1 container ∈ ℕ  ℕ
    THEN
        v := 0
    END
END
";
    let result = parse_with_recovery(source);
    assert!(result.has_recovered(), "broken guard must trigger recovery");
    let m = expect_machine(&result);

    assert_eq!(m.events.len(), 1);
    let params = &m.events[0].parameters;
    assert_eq!(params.len(), 1, "one ANY-clause parameter, got {params:?}");
    assert_eq!(params[0].name, "container");

    // The span must point at the identifier text in the source.
    let span = params[0].span.expect("parameter carries a span");
    assert_eq!(&source[span.start..span.end], "container");
}

#[test]
fn recovery_populates_event_guards_and_actions() {
    // When a machine invariant fails (forcing recovery), event WHERE/WHEN guards
    // and THEN actions must still be populated so the formula-walk that backs
    // semantic token coloring visits them. The broken guard is dropped (its
    // formula cannot be parsed); the healthy ones survive.
    let source = "\
MACHINE m
VARIABLES
    x y
INVARIANTS
    @inv1 x ∈ ℕ
    @inv2 invalid @#$ syntax
EVENTS
    EVENT step
    ANY
        p
    WHERE
        @grd1 p ∈ ℕ
        @grd2 p   p
        @grd3 x > 0
    THEN
        @act1 x := p
        @act2 y := x
    END
END
";
    let result = parse_with_recovery(source);
    assert!(result.has_recovered(), "expected recovery with errors");
    let m = expect_machine(&result);

    let event = &m.events[0];
    assert_eq!(event.name, "step");

    // ANY-clause parameter (name recovery).
    assert_eq!(
        event
            .parameters
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["p"],
        "parameter recovered"
    );

    // Guards: @grd1 and @grd3 parse; @grd2 (`p   p` is not a predicate) is dropped.
    let guard_labels: Vec<Option<&str>> = event.guards.iter().map(|g| g.label.as_deref()).collect();
    assert!(
        guard_labels.contains(&Some("grd1")),
        "@grd1 must be recovered, got {guard_labels:?}"
    );
    assert!(
        guard_labels.contains(&Some("grd3")),
        "@grd3 must be recovered, got {guard_labels:?}"
    );
    assert!(
        !guard_labels.contains(&Some("grd2")),
        "@grd2 has a parse error and must be absent, got {guard_labels:?}"
    );

    // Actions: @act1 and @act2 both parse.
    let action_labels: Vec<Option<&str>> =
        event.actions.iter().map(|a| a.label.as_deref()).collect();
    assert_eq!(
        action_labels,
        [Some("act1"), Some("act2")],
        "both actions must be recovered, got {action_labels:?}"
    );
}

#[test]
fn recovery_populates_initialisation_actions() {
    // INITIALISATION THEN actions must be recovered so their formula trees are
    // available to the semantic-token formula walk.
    let source = "\
MACHINE m
VARIABLES
    x y
INVARIANTS
    @inv1 invalid @#$ syntax
EVENTS
    EVENT INITIALISATION
    THEN
        @act1 x := 0
        @act2 y := 1
    END
END
";
    let result = parse_with_recovery(source);
    assert!(result.has_recovered(), "expected recovery with errors");
    let m = expect_machine(&result);

    let init = m.initialisation.as_ref().expect("initialisation recovered");
    let action_labels: Vec<Option<&str>> =
        init.actions.iter().map(|a| a.label.as_deref()).collect();
    assert_eq!(
        action_labels,
        [Some("act1"), Some("act2")],
        "both INITIALISATION actions must be recovered, got {action_labels:?}"
    );
}

#[test]
fn recovery_formula_identifier_spans_are_absolute() {
    // Every recovered guard / action formula identifier must carry an absolute
    // span that maps back to its own text in the source. This is what semantic
    // tokens rely on to colour identifiers; an off-by-prefix shift would colour
    // the wrong bytes. A label prefix (`@grd1 `, `@act1 `) must not skew the
    // action body's spans.
    use rossi::formula::occurrences::{
        Occurrence, OccurrenceVisitor, walk_assignment, walk_predicate,
    };
    use std::ops::ControlFlow;

    let source = "\
MACHINE m
VARIABLES
    counter
INVARIANTS
    @inv1 broken @#$ syntax
EVENTS
    EVENT step
    ANY
        amount
    WHERE
        @grd1 amount ∈ ℕ
    THEN
        @act1 counter := amount
    END
END
";
    let result = parse_with_recovery(source);
    let m = expect_machine(&result);
    let event = &m.events[0];

    // Collect every identifier occurrence (with a span) and assert the span
    // slices its own name out of the source.
    struct SpanCheck<'s> {
        source: &'s str,
        seen: Vec<String>,
    }
    impl OccurrenceVisitor for SpanCheck<'_> {
        fn visit(&mut self, occ: Occurrence<'_>) -> ControlFlow<()> {
            if let Some(span) = occ.span {
                assert_eq!(
                    &self.source[span.start..span.end],
                    occ.name,
                    "{:?} span {:?} must slice the identifier text",
                    occ.name,
                    span
                );
                self.seen.push(occ.name.to_string());
            }
            ControlFlow::Continue(())
        }
    }

    let mut check = SpanCheck {
        source,
        seen: Vec::new(),
    };
    let mut binders = Vec::new();
    for guard in &event.guards {
        let _ = walk_predicate(&guard.predicate, &mut binders, &mut check);
    }
    for action in &event.actions {
        if let Some(assignment) = action.action.assignment() {
            let _ = walk_assignment(assignment, &mut binders, &mut check);
        }
    }

    // The guard reads `amount`; the action writes `counter` from `amount`.
    assert!(
        check.seen.contains(&"amount".to_string()),
        "guard/action identifier `amount` must be visited with a span, saw {:?}",
        check.seen
    );
    assert!(
        check.seen.contains(&"counter".to_string()),
        "action write target `counter` must be visited with a span, saw {:?}",
        check.seen
    );
}

#[test]
fn recovery_extracts_newline_separated_any_parameters() {
    // Rodin-style files list ANY parameters one per line with no commas (the
    // grammar separates declared names by whitespace). When a guard fails and
    // forces recovery, every parameter must still be recovered with a byte-exact
    // span — not just the first — so the LSP colours all parameter declarations,
    // not only the one named in the broken guard.
    let source = "\
MACHINE m
VARIABLES
    v
EVENTS
    EVENT step
    ANY
        user
        subject
        roleName
    WHERE
        @grd1 user ∈ S :
        @grd2 subject ∈ T
    THEN
        v ≔ 0
    END
END
";
    let result = parse_with_recovery(source);
    assert!(result.has_recovered(), "broken guard must trigger recovery");
    let m = expect_machine(&result);
    assert_eq!(m.events.len(), 1);

    let params = &m.events[0].parameters;
    assert_eq!(
        params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["user", "subject", "roleName"],
        "every whitespace-separated ANY parameter must be recovered, got {params:?}"
    );

    // Each parameter's span slices its own name out of the source.
    for param in params {
        let span = param.span.expect("recovered parameter carries a span");
        assert_eq!(
            &source[span.start..span.end],
            param.name,
            "parameter {} span {span:?} must slice its own name",
            param.name
        );
    }
}

#[test]
fn recovery_does_not_invent_with_witness_for_initialisation() {
    // The grammar gives INITIALISATION only a THEN clause, and the strict parser
    // always leaves with/witnesses empty. The broken invariant forces recovery;
    // recover_events then treats `EVENT INITIALISATION` as the init event. Even
    // with a stray WITH clause present, recovery must NOT synthesize init.with /
    // init.witnesses (a valid parse could never produce them) while the THEN
    // actions still recover.
    let source = "\
MACHINE m
VARIABLES
    x
INVARIANTS
    @inv1 broken @#$ syntax
EVENTS
    EVENT INITIALISATION
    WITH
        @w1 x = 0
    THEN
        @act1 x ≔ 0
    END
END
";
    let result = parse_with_recovery(source);
    assert!(
        result.has_recovered(),
        "the broken invariant must trigger recovery"
    );
    let m = expect_machine(&result);
    let init = m.initialisation.as_ref().expect("initialisation recovered");
    assert!(
        init.with.is_empty(),
        "init.with must stay empty (grammar forbids WITH on INITIALISATION), got {:?}",
        init.with
    );
    assert!(
        init.witnesses.is_empty(),
        "init.witnesses must stay empty, got {:?}",
        init.witnesses
    );
    assert_eq!(
        init.actions
            .iter()
            .map(|a| a.label.as_deref())
            .collect::<Vec<_>>(),
        [Some("act1")],
        "THEN actions are still recovered"
    );
}

#[test]
fn recovery_anchors_guard_and_action_spans_for_the_outline() {
    // Recovered guards/actions must carry their label-inclusive source span, so
    // the document outline anchors each at its location instead of collapsing to
    // (0,0). The broken invariant forces recovery; the event's clauses are healthy.
    let source = "\
MACHINE m
VARIABLES
    x
INVARIANTS
    @inv1 broken @#$ syntax
EVENTS
    EVENT step
    WHERE
        @grd1 x > 0
    THEN
        @act1 x ≔ 1
    END
END
";
    let result = parse_with_recovery(source);
    assert!(result.has_recovered(), "expected recovery with errors");
    let m = expect_machine(&result);
    let event = &m.events[0];

    let guard_span = event.guards[0]
        .span
        .expect("recovered guard carries a span");
    assert_eq!(
        &source[guard_span.start..guard_span.end],
        "@grd1 x > 0",
        "guard span must cover its label-inclusive source"
    );

    let action_span = event.actions[0]
        .span
        .expect("recovered action carries a span");
    assert_eq!(
        &source[action_span.start..action_span.end],
        "@act1 x ≔ 1",
        "action span must cover its label-inclusive source"
    );
}

#[test]
fn recovery_anchors_variant_spans_in_the_document() {
    // The recovery path parsed each variant item out of a detached slice, so
    // the expression's own spans were relative to that slice and resolved
    // against the wrong region of the file. The item span is new besides.
    let source = "\
MACHINE m
VARIABLES
    x
INVARIANTS
    @inv1 broken @#$ syntax
VARIANT
    @vrn1 x
EVENTS
    EVENT step
    THEN
        @act1 x ≔ 1
    END
END
";
    let result = parse_with_recovery(source);
    assert!(result.has_recovered(), "expected recovery with errors");
    let m = expect_machine(&result);
    let variant = &m.variants[0];

    let span = variant.span.expect("recovered variant carries a span");
    assert_eq!(
        &source[span.start..span.end],
        "@vrn1 x",
        "variant span must cover its label-inclusive source"
    );

    let expression_span = variant.expression.span().expect("expression span");
    assert_eq!(
        &source[expression_span.start..expression_span.end],
        "x",
        "the expression must be spanned in document coordinates"
    );
}
