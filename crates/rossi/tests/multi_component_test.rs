use indoc::indoc;
use rossi::{
    Component, ParseError, components_to_string, parse_components, parse_components_with_recovery,
    parse_with_recovery,
};

#[test]
fn test_single_context_via_parse_components() {
    let source = indoc! {"
        CONTEXT C0
        END
    "};
    let result = parse_components(source).unwrap();
    assert_eq!(result.len(), 1);
    assert!(matches!(&result[0], Component::Context(c) if c.name == "C0"));
}

#[test]
fn test_single_machine_via_parse_components() {
    let source = indoc! {"
        MACHINE M0
        END
    "};
    let result = parse_components(source).unwrap();
    assert_eq!(result.len(), 1);
    assert!(matches!(&result[0], Component::Machine(m) if m.name == "M0"));
}

#[test]
fn test_two_contexts() {
    let source = indoc! {"
        CONTEXT C0
        END

        CONTEXT C1
        END
    "};
    let result = parse_components(source).unwrap();
    assert_eq!(result.len(), 2);
    assert!(matches!(&result[0], Component::Context(c) if c.name == "C0"));
    assert!(matches!(&result[1], Component::Context(c) if c.name == "C1"));
}

#[test]
fn test_mixed_context_and_machine() {
    let source = indoc! {"
        CONTEXT C0
        SETS
            STATUS
        END

        MACHINE M0
        SEES
            C0
        VARIABLES
            x
        INVARIANTS
            @inv1 x : STATUS
        END
    "};
    let result = parse_components(source).unwrap();
    assert_eq!(result.len(), 2);
    assert!(matches!(&result[0], Component::Context(c) if c.name == "C0"));
    assert!(matches!(&result[1], Component::Machine(m) if m.name == "M0"));
}

#[test]
fn test_traffic_light_file() {
    let source = include_str!("../examples/traffic-light.txt");
    let result = parse_components(source).unwrap();
    assert_eq!(result.len(), 4);

    let names: Vec<&str> = result.iter().map(|c| c.name()).collect();
    assert_eq!(names, vec!["M0", "C1", "M1", "M2"]);

    // Verify types
    assert!(matches!(&result[0], Component::Machine(_)));
    assert!(matches!(&result[1], Component::Context(_)));
    assert!(matches!(&result[2], Component::Machine(_)));
    assert!(matches!(&result[3], Component::Machine(_)));
}

#[test]
fn test_roundtrip_multi_component() {
    let source = include_str!("../examples/traffic-light.txt");
    let components = parse_components(source).unwrap();

    let printed = components_to_string(&components);
    let reparsed = parse_components(&printed).unwrap();

    assert_eq!(components.len(), reparsed.len());

    for (orig, re) in components.iter().zip(reparsed.iter()) {
        assert_eq!(orig.name(), re.name());
    }
}

#[test]
fn test_empty_input_is_error() {
    let result = parse_components("");
    assert!(result.is_err());
}

#[test]
fn test_duplicate_component_names_accepted() {
    let source = indoc! {"
        MACHINE M0
        END

        MACHINE M0
        END
    "};
    let result = parse_components(source).unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].name(), "M0");
    assert_eq!(result[1].name(), "M0");
}

// --- parse_components_with_recovery ---

/// The structured 1-indexed line of an error, where it carries one.
fn error_line(error: &ParseError) -> Option<usize> {
    error.position().map(|(line, _)| line)
}

#[test]
fn recovery_valid_multi_component_has_no_errors() {
    let source = include_str!("../examples/traffic-light.txt");
    let result = parse_components_with_recovery(source);
    assert!(result.is_ok(), "unexpected errors: {:?}", result.errors);

    let components = result.component.unwrap();
    let names: Vec<&str> = components.iter().map(|c| c.name()).collect();
    assert_eq!(names, vec!["M0", "C1", "M1", "M2"]);

    // Spans come from a whole-file strict parse, so they are file-absolute.
    let second = components[1].span().expect("strict parse records spans");
    assert!(second.start > 0);
    assert!(source[second.start..].starts_with("CONTEXT"));
}

#[test]
fn recovery_middle_component_broken() {
    let source = indoc! {"
        CONTEXT C0
        END

        MACHINE M1
        VARIABLES
            x
        INVARIANTS
            @inv1 x +++
        END

        MACHINE M2
        END
    "};
    let result = parse_components_with_recovery(source);
    assert!(result.has_recovered());

    let components = result.component.unwrap();
    let names: Vec<&str> = components.iter().map(|c| c.name()).collect();
    assert_eq!(names, vec!["C0", "M1", "M2"]);

    // The broken invariant is on (1-indexed) line 8; every reported line must
    // be absolute within the full input, i.e. inside M1's region (lines 4-9).
    let lines: Vec<usize> = result.errors.iter().filter_map(error_line).collect();
    assert!(!lines.is_empty());
    assert!(
        lines.iter().all(|&l| (4..=9).contains(&l)),
        "error lines must fall in M1's region: {lines:?}"
    );
    assert!(
        lines.contains(&8),
        "the broken invariant on line 8 must be reported: {lines:?}"
    );

    // The recovered middle component carries its region as an approximate
    // span (position-based consumers dispatch on it); its strict-parsed
    // neighbours keep exact spans.
    let m1_span = components[1]
        .span()
        .expect("recovered component must get its region span");
    assert!(source[m1_span.start..].starts_with("MACHINE M1"));
    let m2_span = components[2].span().expect("strict span");
    assert!(source[m2_span.start..].starts_with("MACHINE M2"));
    assert!(m1_span.end <= m2_span.start, "regions must not overlap");
}

#[test]
fn recovery_shifts_reserved_word_errors_to_absolute_lines() {
    // The ReservedWord error from a later component's region must be
    // reported at its file-absolute line, like every other located error.
    let source = indoc! {"
        CONTEXT C0
        END

        MACHINE M1
        VARIABLES
            dom
        INVARIANTS
            @inv1 1 = 1
        END
    "};
    let result = parse_components_with_recovery(source);
    assert!(result.has_recovered());

    let reserved: Vec<_> = result
        .errors
        .iter()
        .filter(|e| matches!(e, ParseError::ReservedWord { word, .. } if word == "dom"))
        .collect();
    assert_eq!(reserved.len(), 1, "errors: {:?}", result.errors);
    // `dom` is declared on (1-indexed) line 6 of the full input.
    assert_eq!(error_line(reserved[0]), Some(6));
    let span = reserved[0].span().expect("reserved word keeps its span");
    assert_eq!(&source[span.start..span.end], "dom");
}

#[test]
fn recovery_shifts_incompatible_operator_error_to_absolute_location() {
    let source = indoc! {"
        CONTEXT C0
        END
        MACHINE M1
        VARIABLES
            a
            b
            c
        VARIANT
            a ∪ b ∩ c
        END
    "};
    let result = parse_components_with_recovery(source);
    let error = result
        .errors
        .iter()
        .find(|error| matches!(error, ParseError::IncompatibleOperators { .. }))
        .unwrap_or_else(|| panic!("expected incompatible operators, got {:?}", result.errors));

    assert_eq!(error_line(error), Some(9));
    let span = error.span().expect("incompatible operator keeps its span");
    assert_eq!(&source[span.start..span.end], "∩");
    assert!(span.start > source.find("MACHINE M1").unwrap());
}

#[test]
fn recovery_shifts_assignment_error_span_to_absolute_location() {
    let source = indoc! {"
        CONTEXT C0
        END
        MACHINE M1
        VARIABLES
            x
        INVARIANTS
            @inv1 x := 5
        END
    "};
    let result = parse_components_with_recovery(source);
    let error = result
        .errors
        .iter()
        .find(|error| matches!(error, ParseError::AssignmentInPredicate { .. }))
        .expect("the later component reports assignment in a predicate");

    assert_eq!(error_line(error), Some(7));
    let span = error.span().expect("assignment operator keeps its span");
    assert_eq!(&source[span.start..span.end], ":=");
}

#[test]
fn recovery_shifts_clause_error_to_absolute_line() {
    let source = indoc! {"
        CONTEXT C0
        END
        MACHINE M1
        VARIABLES
            x
        VARIABLES
            y
        END
    "};
    let result = parse_components_with_recovery(source);
    let error = result
        .errors
        .iter()
        .find(|error| matches!(error, ParseError::ClauseError { .. }))
        .expect("the later component reports its duplicate clause");

    assert_eq!(error.position(), Some((6, 1)));
}

#[test]
fn recovery_preserves_absolute_pest_error_span() {
    let source = indoc! {"
        CONTEXT C0
        END
        MACHINE M1
        VARIABLES
            x-y
        END
    "};
    let result = parse_components_with_recovery(source);
    let error = result
        .errors
        .iter()
        .find(|error| matches!(error, ParseError::PestError { .. }))
        .expect("the later component reports its syntax error");
    let span = error.span().expect("pest error keeps its span");

    assert!(span.start >= source.find("x-y").unwrap());
    assert!(span.start < source.rfind("END\n").unwrap_or(source.len()));
    assert_eq!(
        error_line(error),
        Some(source[..span.start].matches('\n').count() + 1)
    );
}

#[test]
fn recovery_preserves_absolute_recoverable_error_span() {
    let source = indoc! {"
        CONTEXT C0
        END
        CONTEXT C1
        AXIOMS
            @axm1 $$$
            @axm2 ###
        END
    "};
    let result = parse_components_with_recovery(source);
    let error = result
        .errors
        .iter()
        .find(|error| {
            matches!(
                error,
                ParseError::RecoverableError { message, .. } if message.contains("@axm2")
            )
        })
        .expect("the later component reports its second broken axiom");

    assert_eq!(error.position(), Some((6, 5)));
    let span = error.span().expect("recoverable error keeps its span");
    assert_eq!(&source[span.start..span.end], "@axm2 ###");

    let ParseError::RecoverableError {
        source: Some(nested),
        ..
    } = error
    else {
        panic!("recoverable error keeps its parser source");
    };
    let nested_span = nested.span().expect("nested parser error keeps its span");
    assert_eq!(nested.position(), Some((1, 8)));
    assert_eq!((nested_span.start, nested_span.end), (7, 7));
}

#[test]
fn recovery_junk_before_first_component() {
    let source = indoc! {"
        this is not event-b

        CONTEXT C0
        END

        MACHINE M1
        END
    "};
    let result = parse_components_with_recovery(source);
    assert!(result.has_recovered());

    let components = result.component.unwrap();
    let names: Vec<&str> = components.iter().map(|c| c.name()).collect();
    assert_eq!(names, vec!["C0", "M1"]);
}

#[test]
fn recovery_last_component_broken() {
    let source = indoc! {"
        CONTEXT C0
        END

        MACHINE M1
        VARIABLES
            x
        INVARIANTS
            @inv1 x +++
        END
    "};
    let result = parse_components_with_recovery(source);
    assert!(result.has_recovered());

    let components = result.component.unwrap();
    let names: Vec<&str> = components.iter().map(|c| c.name()).collect();
    assert_eq!(names, vec!["C0", "M1"]);

    let lines: Vec<usize> = result.errors.iter().filter_map(error_line).collect();
    assert!(
        lines.contains(&8),
        "the broken invariant on line 8 must be reported: {lines:?}"
    );
}

#[test]
fn recovery_midline_keyword_does_not_split() {
    // `MACHINE` mid-line (here in a broken axiom) must not start a new
    // region: headers are only recognized at the start of a line.
    let source = indoc! {"
        CONTEXT C0
        SETS
            S
        AXIOMS
            @axm1 partition(S, MACHINE) +++
        END

        MACHINE M1
        END
    "};
    let result = parse_components_with_recovery(source);
    assert!(result.has_recovered());

    let components = result.component.unwrap();
    let names: Vec<&str> = components.iter().map(|c| c.name()).collect();
    assert_eq!(names, vec!["C0", "M1"], "no phantom component");
}

#[test]
fn recovery_nesting_too_deep_fails_fast() {
    let source = format!(
        "MACHINE M0\nEND\n\nMACHINE M1\nINVARIANTS\n    @inv1 {}1 = 1{}\nEND\n",
        "(".repeat(400),
        ")".repeat(400)
    );
    let result = parse_components_with_recovery(&source);
    assert!(result.is_err());
    let [ParseError::NestingTooDeep { line, .. }] = result.errors.as_slice() else {
        panic!("expected one nesting error, got {:?}", result.errors);
    };
    assert_eq!(
        *line, 6,
        "the whole-input guard already reports absolute lines"
    );
}

#[test]
fn recovery_single_component_matches_single_recovery() {
    let source = indoc! {"
        MACHINE M0
        VARIABLES
            x
        INVARIANTS
            @inv1 x +++
        END
    "};
    let multi = parse_components_with_recovery(source);
    let single = parse_with_recovery(source);

    let components = multi.component.unwrap();
    assert_eq!(components.len(), 1);
    assert_eq!(components[0].name(), single.component.unwrap().name());
    assert_eq!(multi.errors.len(), single.errors.len());
    assert_eq!(
        multi
            .errors
            .iter()
            .filter_map(error_line)
            .collect::<Vec<_>>(),
        single
            .errors
            .iter()
            .filter_map(error_line)
            .collect::<Vec<_>>()
    );
}
