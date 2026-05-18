//! XML structure error tests — covers the EB002 (`UnexpectedXmlRoot`)
//! and EB003 (`MissingXmlAttribute`) variants surfaced by the XML parser,
//! plus the `FileContext` wrapper that preserves the inner variant
//! through `parse_zip_with_recovery`.

use rossi::{ParseError, parse_xml, parse_zip_with_recovery};

#[test]
fn unexpected_xml_root_returns_eb002() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<some.unknown.root version="3"/>"#;

    match parse_xml(xml) {
        Err(ParseError::UnexpectedXmlRoot { found }) => {
            assert_eq!(found, "some.unknown.root");
        }
        other => panic!("expected UnexpectedXmlRoot, got {other:?}"),
    }
}

#[test]
fn nested_supported_root_still_reports_first_root_as_eb002() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<wrapper>
    <org.eventb.core.contextFile version="3">
        <org.eventb.core.context name="C0"/>
    </org.eventb.core.contextFile>
</wrapper>"#;

    match parse_xml(xml) {
        Err(ParseError::UnexpectedXmlRoot { found }) => {
            assert_eq!(found, "wrapper");
        }
        other => panic!("expected UnexpectedXmlRoot for wrapper, got {other:?}"),
    }
}

#[test]
fn empty_xml_root_field_when_no_start_event() {
    // No Start event at all: the parser falls through with no first root,
    // so `found` is the empty string.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>"#;
    match parse_xml(xml) {
        Err(ParseError::UnexpectedXmlRoot { found }) => {
            assert_eq!(found, "");
        }
        other => panic!("expected UnexpectedXmlRoot, got {other:?}"),
    }
}

#[test]
fn missing_extends_target_returns_eb003() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
    <org.eventb.core.context name="C0"/>
    <org.eventb.core.extendsContext name="internal"/>
</org.eventb.core.contextFile>"#;

    match parse_xml(xml) {
        Err(ParseError::MissingXmlAttribute { element, attribute }) => {
            assert_eq!(element, "org.eventb.core.extendsContext");
            assert_eq!(attribute, "target");
        }
        other => panic!("expected MissingXmlAttribute, got {other:?}"),
    }
}

#[test]
fn missing_sees_target_returns_eb003() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.machine name="M0"/>
    <org.eventb.core.seesContext name="internal"/>
</org.eventb.core.machineFile>"#;

    match parse_xml(xml) {
        Err(ParseError::MissingXmlAttribute { element, attribute }) => {
            assert_eq!(element, "org.eventb.core.seesContext");
            assert_eq!(attribute, "target");
        }
        other => panic!("expected MissingXmlAttribute, got {other:?}"),
    }
}

#[test]
fn missing_refines_target_returns_eb003() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.machine name="M1"/>
    <org.eventb.core.refinesMachine name="internal"/>
</org.eventb.core.machineFile>"#;

    match parse_xml(xml) {
        Err(ParseError::MissingXmlAttribute { element, attribute }) => {
            assert_eq!(element, "org.eventb.core.refinesMachine");
            assert_eq!(attribute, "target");
        }
        other => panic!("expected MissingXmlAttribute, got {other:?}"),
    }
}

#[test]
fn file_context_preserves_inner_variant_through_recovery() {
    // Build a two-entry in-memory zip: one good context, one with an
    // extendsContext missing its target attribute. The recovery loop must
    // record a `FileContext` whose inner variant is `MissingXmlAttribute`.
    let good = br#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
    <org.eventb.core.context name="Good"/>
</org.eventb.core.contextFile>"#;

    let bad = br#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
    <org.eventb.core.context name="Bad"/>
    <org.eventb.core.extendsContext name="internal"/>
</org.eventb.core.contextFile>"#;

    let mut buf = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut buf);
        let mut zw = zip::ZipWriter::new(cursor);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
        zw.start_file("Good.buc", opts).unwrap();
        std::io::Write::write_all(&mut zw, good).unwrap();
        zw.start_file("Bad.buc", opts).unwrap();
        std::io::Write::write_all(&mut zw, bad).unwrap();
        zw.finish().unwrap();
    }

    let result = parse_zip_with_recovery(&buf);
    assert_eq!(
        result.errors.len(),
        1,
        "expected 1 error, got {:?}",
        result.errors
    );
    match &result.errors[0] {
        ParseError::FileContext { filename, source } => {
            assert_eq!(filename, "Bad.buc");
            match source.as_ref() {
                ParseError::MissingXmlAttribute { element, attribute } => {
                    assert_eq!(element, "org.eventb.core.extendsContext");
                    assert_eq!(attribute, "target");
                }
                other => panic!("expected inner MissingXmlAttribute, got {other:?}"),
            }
            // Display impl renders the legacy "Failed to parse {filename}: …"
            // string so console output stays human-friendly.
            let rendered = format!("{}", result.errors[0]);
            assert!(
                rendered.starts_with("Failed to parse Bad.buc: "),
                "unexpected Display rendering: {rendered:?}"
            );
        }
        other => panic!("expected FileContext, got {other:?}"),
    }
    // The good component still parses despite the bad one.
    let comps = result.component.expect("good component should survive");
    assert_eq!(comps.len(), 1);
    assert_eq!(comps[0].filename, "Good.buc");
}
