//! Name policy (issue #28) and XML-import identifier validation.
//!
//! Rodin stores machine/context names as file names and event names as
//! labels (bare strings), so real models carry hyphens in them. The text
//! grammar accepts those names in structural positions (`component_name`
//! rule) while keeping mathematical identifiers hyphen-free per
//! kernel_lang §2.2, and `rossi import` output must re-parse. XML import
//! enforces the same name policy and wraps malformed formula attributes
//! for cleaner diagnostics.

use rossi::error::ParseError;
use rossi::{Component, parse, parse_components, parse_components_with_recovery, parse_xml};

fn parse_one(source: &str) -> Component {
    parse(source).expect("should parse")
}

#[test]
fn machine_with_hyphenated_names_in_every_structural_position() {
    let source = "\
MACHINE M-ALPHA
REFINES
    M-ALPHA-0
SEES
    CTX-1
VARIABLES
    x
INVARIANTS
    @inv1 x ∈ ℕ
EVENTS
EVENT INITIALISATION
THEN
    @act1 x ≔ 0
END
EVENT do-step
REFINES
    prepost-step
WHERE
    @grd1 x > 0
THEN
    @act1 x ≔ x − 1
END
END
";
    let Component::Machine(m) = parse_one(source) else {
        panic!("expected Machine");
    };
    assert_eq!(m.name, "M-ALPHA");
    assert_eq!(m.refines.as_deref(), Some("M-ALPHA-0"));
    assert_eq!(m.sees, vec!["CTX-1"]);
    assert_eq!(m.events.len(), 1);
    assert_eq!(m.events[0].name, "do-step");
    assert_eq!(m.events[0].refines[0].name, "prepost-step");
}

#[test]
fn context_with_hyphenated_name_and_extends() {
    let source = "\
CONTEXT ENV_C-1
EXTENDS
    ENV_C-0 base-ctx
CONSTANTS
    c
AXIOMS
    @axm1 c ∈ ℕ
END
";
    let Component::Context(ctx) = parse_one(source) else {
        panic!("expected Context");
    };
    assert_eq!(ctx.name, "ENV_C-1");
    assert_eq!(ctx.extends, vec!["ENV_C-0", "base-ctx"]);
}

#[test]
fn event_extends_hyphenated_parent() {
    let source = "\
MACHINE m1
EVENTS
EVENT do-step extends do-step
END
END
";
    let Component::Machine(m) = parse_one(source) else {
        panic!("expected Machine");
    };
    assert_eq!(m.events[0].name, "do-step");
    assert_eq!(m.events[0].refines[0].name, "do-step");
    assert!(m.events[0].extended);
}

// ----- keyword-boundary interactions -------------------------------------

#[test]
fn sees_list_name_with_embedded_keyword_is_one_name() {
    // `end-to-end` must not stop at the embedded `end`; `variant-x` must
    // not silently start a VARIANT clause (which would misparse `-x` as a
    // unary-minus variant expression).
    let source = "\
MACHINE m1
SEES
    c1 end-to-end variant-x
VARIABLES
    v
INVARIANTS
    @inv1 v ∈ ℕ
END
";
    let Component::Machine(m) = parse_one(source) else {
        panic!("expected Machine");
    };
    assert_eq!(m.sees, vec!["c1", "end-to-end", "variant-x"]);
    assert!(
        m.variants.is_empty(),
        "variant-x must not open a VARIANT clause"
    );
}

#[test]
fn component_named_with_embedded_keyword() {
    let source = "MACHINE end-to-end\nEND\n";
    let Component::Machine(m) = parse_one(source) else {
        panic!("expected Machine");
    };
    assert_eq!(m.name, "end-to-end");

    let source = "CONTEXT events-x\nEND\n";
    let Component::Context(ctx) = parse_one(source) else {
        panic!("expected Context");
    };
    assert_eq!(ctx.name, "events-x");
}

#[test]
fn event_named_with_keyword_prefix() {
    let source = "\
MACHINE m1
EVENTS
EVENT end-update
THEN
    @act1 x ≔ 0
END
EVENT INITIALISATION-x
END
END
";
    let Component::Machine(m) = parse_one(source) else {
        panic!("expected Machine");
    };
    assert_eq!(m.events.len(), 2);
    assert_eq!(m.events[0].name, "end-update");
    // INITIALISATION-x is an ordinary event, not the INITIALISATION slot.
    assert_eq!(m.events[1].name, "INITIALISATION-x");
    assert!(m.initialisation.is_none());
}

#[test]
fn multi_component_recovery_keeps_hyphenated_names_whole() {
    // `the-MACHINE-x` inside a SEES list must not be treated as a MACHINE
    // header by the multi-component splitter.
    let source = "\
MACHINE m-1
SEES
    the-MACHINE-x
END
CONTEXT the-MACHINE-x
END
";
    let components = parse_components(source).expect("should parse two components");
    assert_eq!(components.len(), 2);
    let Component::Machine(m) = &components[0] else {
        panic!("expected Machine first");
    };
    assert_eq!(m.name, "m-1");
    assert_eq!(m.sees, vec!["the-MACHINE-x"]);
    let Component::Context(ctx) = &components[1] else {
        panic!("expected Context second");
    };
    assert_eq!(ctx.name, "the-MACHINE-x");
}

#[test]
fn math_keyword_boundaries_unchanged_across_hyphen() {
    // In formulas `-` is subtraction and must still bind keywords on both
    // sides: `NAT-1` is `ℕ − 1`-shaped lexically (kw_nat still matches),
    // `a-dom(r)` keeps `dom` as an operator.
    let source = "\
MACHINE m1
VARIABLES
    a
INVARIANTS
    @inv1 a ∈ NAT
EVENTS
EVENT INITIALISATION
THEN
    @act1 a ≔ card(NAT1-a‥5-dom({1↦2}) ∪ {0})
END
END
";
    parse(source).expect("math positions must keep treating '-' as minus");
}

// ----- negatives ----------------------------------------------------------

#[test]
fn malformed_hyphen_component_names_rejected() {
    for source in [
        "MACHINE a- \nEND\n",
        "MACHINE a--b\nEND\n",
        "MACHINE -a\nEND\n",
    ] {
        assert!(
            parse(source).is_err(),
            "should reject malformed name in {source:?}"
        );
    }
}

#[test]
fn hyphen_rejected_in_math_declarations() {
    // VARIABLES/CONSTANTS/ANY declare mathematical identifiers — `x-y`
    // must not parse as one declared name (kernel_lang §2.2).
    for source in [
        "MACHINE m1\nVARIABLES\n    x-y\nEND\n",
        "CONTEXT c1\nCONSTANTS\n    c-1\nEND\n",
        "MACHINE m1\nEVENTS\nEVENT e1\nANY\n    p-1\nWHERE\n    @grd1 p-1 ∈ ℕ\nEND\nEND\n",
    ] {
        assert!(
            parse(source).is_err(),
            "should reject hyphenated math declaration in {source:?}"
        );
    }
}

// ----- recovery never yields an unprintable name --------------------------

#[test]
fn recovery_rejects_invalid_component_names() {
    // Malformed headers/targets must not flow into a recovered AST the pretty
    // printer cannot re-emit (its debug_assert would otherwise panic): the
    // bad name is dropped and the component keeps its default name.
    for src in [
        "MACHINE a--b\nEND\n",
        "MACHINE m1\nSEES\n    a--b\nEND\n",
        "CONTEXT \u{3bb}\nEND\n",
    ] {
        let result = parse_components_with_recovery(src);
        let components = result.component.expect("recovery yields a partial AST");
        for component in &components {
            // Must not panic, and must re-parse.
            let text = rossi::to_string(component);
            rossi::parse(&text)
                .unwrap_or_else(|e| panic!("recovered AST must re-parse, got {e}\n{text}"));
        }
    }
}

// ----- import → pretty-print → re-parse round-trip -------------------------

#[test]
fn xml_import_round_trips_through_text() {
    // The issue #28 reproduction: Rodin XML with hyphenated structural
    // names everywhere import permits them; the pretty-printed text must
    // re-parse to the same structure.
    let machine_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.refinesMachine name="r" org.eventb.core.target="M-ALPHA-0"/>
    <org.eventb.core.seesContext name="s" org.eventb.core.target="ENV_C-1"/>
    <org.eventb.core.variable name="v" org.eventb.core.identifier="x"/>
    <org.eventb.core.invariant name="i" org.eventb.core.label="inv1" org.eventb.core.predicate="x &#x2208; &#x2115;" org.eventb.core.theorem="false"/>
    <org.eventb.core.event name="e0" org.eventb.core.label="INITIALISATION" org.eventb.core.convergence="0" org.eventb.core.extended="false">
        <org.eventb.core.action name="a" org.eventb.core.label="act1" org.eventb.core.assignment="x &#x2254; 0"/>
    </org.eventb.core.event>
    <org.eventb.core.event name="e1" org.eventb.core.label="computing-computing" org.eventb.core.convergence="0" org.eventb.core.extended="false">
        <org.eventb.core.refinesEvent name="re" org.eventb.core.target="prepost-computing"/>
        <org.eventb.core.action name="a" org.eventb.core.label="act1" org.eventb.core.assignment="x &#x2254; x + 1"/>
    </org.eventb.core.event>
</org.eventb.core.machineFile>"#;

    let imported = parse_xml(machine_xml).expect("import should accept hyphenated names");
    let text = rossi::to_string(&imported);
    let reparsed = parse(&text)
        .unwrap_or_else(|e| panic!("import output must re-parse, got {e}\n--- text ---\n{text}"));

    let (Component::Machine(a), Component::Machine(b)) = (&imported, &reparsed) else {
        panic!("expected machines");
    };
    assert_eq!(b.refines.as_deref(), Some("M-ALPHA-0"));
    assert_eq!(b.sees, vec!["ENV_C-1"]);
    assert_eq!(a.events.len(), b.events.len());
    assert_eq!(b.events[0].name, "computing-computing");
    assert_eq!(b.events[0].refines[0].name, "prepost-computing");

    let context_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
    <org.eventb.core.extendsContext name="x" org.eventb.core.target="ENV_C-0"/>
    <org.eventb.core.constant name="c" org.eventb.core.identifier="k"/>
    <org.eventb.core.axiom name="a" org.eventb.core.label="axm1" org.eventb.core.predicate="k &#x2208; &#x2115;" org.eventb.core.theorem="false"/>
</org.eventb.core.contextFile>"#;

    let imported = parse_xml(context_xml).expect("import should accept hyphenated extends");
    let text = rossi::to_string(&imported);
    let reparsed = parse(&text)
        .unwrap_or_else(|e| panic!("import output must re-parse, got {e}\n--- text ---\n{text}"));
    let Component::Context(ctx) = &reparsed else {
        panic!("expected Context");
    };
    assert_eq!(ctx.extends, vec!["ENV_C-0"]);
}

// ----- XML import: identifier validation -----------------------------------

#[test]
fn reserved_keyword_constant_accepted() {
    // Rodin permits keyword-named identifiers (`end`, `events`, …) in
    // XML. Our expression-position grammar parses them as identifiers
    // — the `kw_*` rules only fire in their specific structural
    // positions (e.g. `kw_end` in context-decl), not as a general
    // reservation. So `partition(L, {end})` parses correctly.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
    <org.eventb.core.constant name="int1" org.eventb.core.identifier="end"/>
</org.eventb.core.contextFile>"#;

    let comp = parse_xml(xml).expect("should accept `end` as constant name");
    if let rossi::Component::Context(ctx) = comp {
        assert_eq!(ctx.constants.len(), 1);
        assert_eq!(ctx.constants[0].name, "end");
    } else {
        panic!("expected Context");
    }
}

#[test]
fn malformed_component_name_target_rejected() {
    // The text grammar's `component_name` rule requires a letter or `_`
    // start and every `-` to open a non-empty segment, so import rejects
    // what pretty-printing could not re-parse (issue #28). The per-character
    // classification is unit-tested in `names`; this pins the wiring for the
    // opaque-target path.
    for (bad, reason_substring) in [
        ("1bad", "must start with a letter or '_'"),
        ("-bad", "must start with a letter or '_'"),
        ("bad-", "'-' must be followed by"),
        ("ba--d", "'-' must be followed by"),
    ] {
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
    <org.eventb.core.extendsContext name="bad" org.eventb.core.target="{bad}"/>
</org.eventb.core.contextFile>"#
        );
        let err = parse_xml(&xml).expect_err(&format!("should reject `{bad}` as extends target"));
        match err {
            ParseError::UnsupportedIdentifier { name, reason, .. } => {
                assert_eq!(name, bad);
                assert!(
                    reason.contains(reason_substring),
                    "`{bad}` reason: {reason}"
                );
            }
            other => panic!("expected UnsupportedIdentifier for `{bad}`, got {other:?}"),
        }
    }
}

#[test]
fn hyphen_in_declared_identifier_rejected() {
    // Constants/variables/sets/parameters are mathematical identifiers
    // (kernel_lang §2.2): Rodin's own isValidIdentifierName rejects
    // hyphens there, and so do we — only structural names get them.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
    <org.eventb.core.constant name="c" org.eventb.core.identifier="c-1"/>
</org.eventb.core.contextFile>"#;

    let err = parse_xml(xml).expect_err("should reject hyphenated constant");
    match err {
        ParseError::UnsupportedIdentifier { name, reason, .. } => {
            assert_eq!(name, "c-1");
            assert!(
                reason.contains("unsupported character '-'"),
                "reason: {reason}"
            );
        }
        other => panic!("expected UnsupportedIdentifier, got {other:?}"),
    }
}

#[test]
fn hyphen_in_witness_label_rejected() {
    // A witness label names an abstract parameter or primed variable — a
    // mathematical identifier position, never a component name.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.event name="'" org.eventb.core.label="evt" org.eventb.core.convergence="0" org.eventb.core.extended="false">
        <org.eventb.core.witness name="w" org.eventb.core.label="p-1" org.eventb.core.predicate="p-1 = 0" rossi.kind="witness"/>
    </org.eventb.core.event>
</org.eventb.core.machineFile>"#;

    let err = parse_xml(xml).expect_err("should reject hyphenated witness label");
    match err {
        ParseError::UnsupportedIdentifier { name, reason, .. } => {
            assert_eq!(name, "p-1");
            assert!(
                reason.contains("unsupported character '-'"),
                "reason: {reason}"
            );
        }
        other => panic!("expected UnsupportedIdentifier, got {other:?}"),
    }
}

// ----- XML import: whitespace around names ---------------------------------

#[test]
fn surrounding_whitespace_in_event_label_trimmed() {
    // Rodin tolerates stray whitespace around names — a real-world corpus
    // model carries an event label with a trailing space. We trim instead
    // of rejecting, and the refinesEvent target is trimmed the same way so
    // the refinement link stays consistent.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.event name="'" org.eventb.core.label="stop " org.eventb.core.convergence="0" org.eventb.core.extended="false">
        <org.eventb.core.refinesEvent name="'" org.eventb.core.target="stop "/>
    </org.eventb.core.event>
</org.eventb.core.machineFile>"#;

    let comp = parse_xml(xml).expect("should accept event label with trailing space");
    if let rossi::Component::Machine(m) = comp {
        assert_eq!(m.events[0].name, "stop");
        assert_eq!(m.events[0].refines[0].name, "stop");
    } else {
        panic!("expected Machine");
    }
}

#[test]
fn whitespace_padded_initialisation_label_recognised() {
    // The trim happens before the INITIALISATION check, so a padded label
    // still lands in the initialisation slot rather than becoming a
    // misnamed ordinary event.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.event name="'" org.eventb.core.label="INITIALISATION " org.eventb.core.convergence="0" org.eventb.core.extended="false"/>
</org.eventb.core.machineFile>"#;

    let comp = parse_xml(xml).expect("should accept padded INITIALISATION label");
    if let rossi::Component::Machine(m) = comp {
        assert!(m.initialisation.is_some());
        assert!(m.events.is_empty());
    } else {
        panic!("expected Machine");
    }
}

#[test]
fn whitespace_only_identifier_rejected() {
    // Trimming must not let an all-whitespace name slip through as empty.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
    <org.eventb.core.constant name="int1" org.eventb.core.identifier="   "/>
</org.eventb.core.contextFile>"#;

    let err = parse_xml(xml).expect_err("should reject whitespace-only identifier");
    match err {
        ParseError::UnsupportedIdentifier { reason, .. } => {
            assert_eq!(reason, "empty");
        }
        other => panic!("expected UnsupportedIdentifier, got {other:?}"),
    }
}

// ----- XML import: malformed-attribute wrapping ----------------------------

#[test]
fn malformed_predicate_attribute_wraps_pest_error() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
    <org.eventb.core.axiom name="int1" org.eventb.core.label="axm1" org.eventb.core.predicate="a &#x2227; )" org.eventb.core.theorem="false"/>
</org.eventb.core.contextFile>"#;

    let err = parse_xml(xml).expect_err("should reject malformed predicate");
    match err {
        ParseError::MalformedAttribute {
            attr_name,
            origin,
            value,
            reason,
            ..
        } => {
            assert_eq!(attr_name, "predicate");
            assert!(origin.contains("axiom"), "origin: {origin}");
            assert!(
                origin.contains("\"axm1\""),
                "origin should mention label, got {origin}"
            );
            assert!(value.contains('\u{2227}'), "raw value: {value}");
            assert!(reason.contains("Pest parsing error"), "reason: {reason}");
        }
        other => panic!("expected MalformedAttribute, got {other:?}"),
    }
}

// ----- Rodin's identifier character classes --------------------------------

#[test]
fn unicode_letters_are_identifiers() {
    // Rodin classifies identifier characters with Java's identifier classes,
    // so any letter starts a name: a carrier set `ℝ`, constants `α` and `β`.
    let source = "\
CONTEXT C
SETS
    ℝ
CONSTANTS
    α
    β
AXIOMS
    @axm1 α ∈ ℝ
    @axm2 β = α
END
";
    let Component::Context(ctx) = parse_one(source) else {
        panic!("expected Context");
    };
    assert_eq!(ctx.sets[0].name, "ℝ");
    assert_eq!(ctx.constants[0].name, "α");
    assert_eq!(ctx.constants[1].name, "β");

    let mut expected = Component::Context(ctx);
    expected.clear_spans();
    let text = rossi::to_string(&expected);
    let mut reparsed = parse(&text).unwrap_or_else(|e| panic!("must re-parse, got {e}\n{text}"));
    reparsed.clear_spans();
    assert_eq!(reparsed, expected);

    // XML import applies the same classes.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
    <org.eventb.core.carrierSet name="s" org.eventb.core.identifier="ℝ"/>
    <org.eventb.core.constant name="c" org.eventb.core.identifier="α"/>
</org.eventb.core.contextFile>"#;
    let Component::Context(imported) = parse_xml(xml).expect("import accepts Unicode names") else {
        panic!("expected Context");
    };
    assert_eq!(imported.sets[0].name, "ℝ");
    assert_eq!(imported.constants[0].name, "α");
}

#[test]
fn reserved_glyphs_stay_tokens() {
    // Rodin's lexer reads the longest identifier image and then looks the
    // whole image up: `ℤ` alone is the integer set, while `ℤx` is one
    // identifier. `λ` is excluded from the classes, so a lambda still lexes.
    let source = "\
MACHINE M
VARIABLES
    ℤx
INVARIANTS
    @inv1 ℤx ∈ ℤ
    @inv2 (λy·y ∈ ℕ ∣ y + ℤx)(1) ∈ ℤ
EVENTS
EVENT INITIALISATION
THEN
    @act1 ℤx ≔ 0
END
END
";
    let Component::Machine(m) = parse_one(source) else {
        panic!("expected Machine");
    };
    assert_eq!(m.variables[0].name, "ℤx");
    let mut expected = Component::Machine(m);
    expected.clear_spans();
    let text = rossi::to_string(&expected);
    assert!(text.contains("ℤx ∈ ℤ"), "{text}");
    let mut reparsed = parse(&text).expect("re-parse");
    reparsed.clear_spans();
    assert_eq!(reparsed, expected);

    // A bare glyph is the token, so it can never name a user identifier:
    // the text grammar does not lex it as one, and XML import refuses it
    // like every reserved word.
    for glyph in rossi::builtins::RESERVED_GLYPH_WORDS {
        let source = format!("MACHINE M\nVARIABLES\n    {glyph}\nEND\n");
        assert!(
            parse(&source).is_err(),
            "{glyph} must not declare a variable"
        );
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
    <org.eventb.core.constant name="c" org.eventb.core.identifier="{glyph}"/>
</org.eventb.core.contextFile>"#
        );
        match parse_xml(&xml) {
            Err(ParseError::UnsupportedIdentifier { .. }) => {}
            other => panic!("{glyph}: expected an unsupported-identifier error, got {other:?}"),
        }
    }
}
