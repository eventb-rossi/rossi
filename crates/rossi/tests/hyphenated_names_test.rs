//! Issue #28 — hyphenated structural names.
//!
//! Rodin stores machine/context names as file names and event names as
//! labels (bare strings), so real models carry hyphens in them. The text
//! grammar accepts those names in structural positions (`component_name`
//! rule) while keeping mathematical identifiers hyphen-free per
//! kernel_lang §2.2, and `rossi import` output must re-parse.

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
    assert_eq!(m.events[0].refines.as_deref(), Some("prepost-step"));
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
    assert_eq!(m.events[0].refines.as_deref(), Some("do-step"));
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
        m.variant.is_none(),
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
    @act1 skip
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
        "CONTEXT \u{e4}\nEND\n",
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
    assert_eq!(b.events[0].refines.as_deref(), Some("prepost-computing"));

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
