//! Tests for identifier validation + malformed-attribute error wrapping
//! introduced to give cleaner diagnostics for Rodin inputs our text format
//! cannot round-trip.

use rossi::error::ParseError;
use rossi::parse_xml;

#[test]
fn hyphen_in_context_identifier_accepted() {
    // Rodin permits hyphens in machine/context names (e.g. `A-C0`,
    // `CTX-1`). They appear in opaque attribute positions such as
    // seesContext targets, so we test via that path.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.seesContext name="A-C0" org.eventb.core.target="A-C0"/>
</org.eventb.core.machineFile>"#;

    let comp = parse_xml(xml).expect("should accept hyphenated context name");
    if let rossi::Component::Machine(m) = comp {
        assert_eq!(m.sees[0], "A-C0");
    } else {
        panic!("expected Machine");
    }
}

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
fn digit_leading_identifier_rejected() {
    // Leading digit still rejected — would confuse the text-grammar
    // lexer if the name ever flowed into a parsed predicate.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
    <org.eventb.core.extendsContext name="bad" org.eventb.core.target="1bad"/>
</org.eventb.core.contextFile>"#;

    let err = parse_xml(xml).expect_err("should reject digit-leading identifier");
    match err {
        ParseError::UnsupportedIdentifier { name, reason, .. } => {
            assert_eq!(name, "1bad");
            assert!(
                reason.contains("must start with ASCII letter or '_'"),
                "reason: {reason}"
            );
        }
        other => panic!("expected UnsupportedIdentifier, got {other:?}"),
    }
}

#[test]
fn leading_hyphen_identifier_rejected() {
    // A name like `-foo` is rejected: hyphen is only allowed after the
    // first character.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
    <org.eventb.core.extendsContext name="bad" org.eventb.core.target="-bad"/>
</org.eventb.core.contextFile>"#;

    let err = parse_xml(xml).expect_err("should reject leading hyphen");
    match err {
        ParseError::UnsupportedIdentifier { name, reason, .. } => {
            assert_eq!(name, "-bad");
            assert!(
                reason.contains("must start with ASCII letter or '_'"),
                "reason: {reason}"
            );
        }
        other => panic!("expected UnsupportedIdentifier, got {other:?}"),
    }
}

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
        assert_eq!(m.events[0].refines.as_deref(), Some("stop"));
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
