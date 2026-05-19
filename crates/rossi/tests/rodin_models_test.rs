//! Integration tests for parsing real Rodin Event-B models
//!
//! These tests use models from https://github.com/17451k/eventb-models
//! to verify that the parser can handle real-world Event-B specifications.

use rossi::{Component, NamedComponent, parse_components, parse_zip_file, parse_zip_with_recovery};

fn component_kind_and_name(component: &Component) -> (&'static str, &str) {
    match component {
        Component::Context(context) => ("Context", context.name.as_str()),
        Component::Machine(machine) => ("Machine", machine.name.as_str()),
    }
}

fn assert_text_components(filename: &str, expected: &[(&str, &str)]) {
    let path = format!("examples/{filename}");
    let source =
        std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("Failed to read {path}: {err}"));

    let components =
        parse_components(&source).unwrap_or_else(|err| panic!("Failed to parse {filename}: {err}"));
    let actual: Vec<_> = components.iter().map(component_kind_and_name).collect();

    assert_eq!(actual.as_slice(), expected, "{filename} components differ");
}

fn named_component_parts(component: &NamedComponent) -> (&str, &'static str, &str) {
    let (kind, name) = component_kind_and_name(&component.component);
    (component.filename.as_str(), kind, name)
}

fn assert_zip_components(filename: &str, expected: &[(&str, &str, &str)]) {
    let path = format!("examples/{filename}");
    let components =
        parse_zip_file(&path).unwrap_or_else(|err| panic!("Failed to parse {path}: {err}"));
    let actual: Vec<_> = components.iter().map(named_component_parts).collect();

    assert_eq!(actual.as_slice(), expected, "{filename} components differ");
}

/// Test parsing binary-search model from text format
#[test]
fn test_binary_search_txt() {
    assert_text_components(
        "binary-search.txt",
        &[
            ("Context", "C0"),
            ("Machine", "M0"),
            ("Machine", "M1"),
            ("Machine", "M2"),
            ("Machine", "M3"),
        ],
    );
}

/// Test parsing binary-search model from zip format
#[test]
fn test_binary_search_zip() {
    assert_zip_components(
        "binary-search.zip",
        &[
            ("C0.buc", "Context", "C0"),
            ("M0.bum", "Machine", "M0"),
            ("M1.bum", "Machine", "M1"),
            ("M2.bum", "Machine", "M2"),
            ("M3.bum", "Machine", "M3"),
        ],
    );
}

/// Test parsing cars-on-bridge model from text format
#[test]
fn test_cars_on_bridge_txt() {
    assert_text_components(
        "cars-on-bridge.txt",
        &[
            ("Context", "C0"),
            ("Machine", "M0"),
            ("Machine", "M1"),
            ("Context", "C2"),
            ("Machine", "M2"),
            ("Context", "C3"),
            ("Machine", "M3"),
        ],
    );
}

/// Test parsing cars-on-bridge model from zip format
#[test]
fn test_cars_on_bridge_zip() {
    assert_zip_components(
        "cars-on-bridge.zip",
        &[
            ("C0.buc", "Context", "C0"),
            ("C2.buc", "Context", "C2"),
            ("C3.buc", "Context", "C3"),
            ("M0.bum", "Machine", "M0"),
            ("M1.bum", "Machine", "M1"),
            ("M2.bum", "Machine", "M2"),
            ("M3.bum", "Machine", "M3"),
        ],
    );
}

/// Test parsing file-system model from text format
#[test]
fn test_file_system_txt() {
    assert_text_components("file-system.txt", &[("Context", "C0"), ("Machine", "M0")]);
}

/// Test parsing file-system model from zip format
#[test]
fn test_file_system_zip() {
    assert_zip_components(
        "file-system.zip",
        &[("C0.buc", "Context", "C0"), ("M0.bum", "Machine", "M0")],
    );
}

/// Test parsing traffic-light model from text format
#[test]
fn test_traffic_light_txt() {
    assert_text_components(
        "traffic-light.txt",
        &[
            ("Machine", "M0"),
            ("Context", "C1"),
            ("Machine", "M1"),
            ("Machine", "M2"),
        ],
    );
}

/// Test parsing traffic-light model from zip format
#[test]
fn test_traffic_light_zip() {
    assert_zip_components(
        "traffic-light.zip",
        &[
            ("C1.buc", "Context", "C1"),
            ("M0.bum", "Machine", "M0"),
            ("M1.bum", "Machine", "M1"),
            ("M2.bum", "Machine", "M2"),
        ],
    );
}

/// Test parsing base-model from zip format
#[test]
fn test_base_model_zip() {
    assert_zip_components(
        "base-model.zip",
        &[("C1.buc", "Context", "C1"), ("M1.bum", "Machine", "M1")],
    );
}

// ============================================================================
// parse_zip_with_recovery tests
// ============================================================================

/// Valid archive: recovery version produces same results as strict, with no errors
#[test]
fn test_parse_zip_with_recovery_valid_archive() {
    let zip_data = std::fs::read("examples/base-model.zip").expect("Failed to read base-model.zip");

    let strict = parse_zip_file("examples/base-model.zip").expect("strict parse failed");
    let recovery = parse_zip_with_recovery(&zip_data);

    assert!(recovery.is_ok(), "recovery should have no errors");
    let components = recovery.component.expect("should have components");

    assert_eq!(
        components.len(),
        strict.len(),
        "recovery should produce same number of components as strict"
    );

    for (s, r) in strict.iter().zip(components.iter()) {
        assert_eq!(s.filename, r.filename);
    }
}

/// Partial failure: one valid .buc and one invalid .bum → 1 component + 1 error
#[test]
fn test_parse_zip_with_recovery_partial_failure() {
    use std::io::Write;

    let valid_buc = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.contextFile org.eventb.core.configuration="org.eventb.core.fwd" version="3">
    <org.eventb.core.carrierSet name="'" org.eventb.core.identifier="STATUS"/>
</org.eventb.core.contextFile>"#;

    // Machine with an unparseable predicate — triggers parser error in parse_xml_labeled_predicate
    let invalid_bum = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.machineFile org.eventb.core.configuration="org.eventb.core.fwd" version="3">
    <org.eventb.core.invariant name="'" label="inv1" predicate="@@@ bad predicate"/>
</org.eventb.core.machineFile>"#;

    let mut buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("Good.buc", options).unwrap();
        writer.write_all(valid_buc.as_bytes()).unwrap();
        writer.start_file("Bad.bum", options).unwrap();
        writer.write_all(invalid_bum.as_bytes()).unwrap();
        writer.finish().unwrap();
    }

    let result = parse_zip_with_recovery(&buf);

    let components = result.component.expect("should have Some components");
    assert_eq!(components.len(), 1, "should have 1 successful component");
    assert_eq!(components[0].filename, "Good.buc");

    assert_eq!(result.errors.len(), 1, "should have 1 error");
    let err_msg = format!("{}", result.errors[0]);
    assert!(
        err_msg.contains("Bad.bum"),
        "error should mention the failing file: {}",
        err_msg
    );
}

/// All files fail: empty components vec + errors for each
#[test]
fn test_parse_zip_with_recovery_all_fail() {
    use std::io::Write;

    // Context with an unparseable axiom predicate
    let invalid_buc = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.contextFile org.eventb.core.configuration="org.eventb.core.fwd" version="3">
    <org.eventb.core.axiom name="'" label="axm1" predicate="@@@ bad"/>
</org.eventb.core.contextFile>"#;

    // Machine with an unparseable invariant predicate
    let invalid_bum = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.machineFile org.eventb.core.configuration="org.eventb.core.fwd" version="3">
    <org.eventb.core.invariant name="'" label="inv1" predicate="@@@ bad"/>
</org.eventb.core.machineFile>"#;

    let mut buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("A.buc", options).unwrap();
        writer.write_all(invalid_buc.as_bytes()).unwrap();
        writer.start_file("B.bum", options).unwrap();
        writer.write_all(invalid_bum.as_bytes()).unwrap();
        writer.finish().unwrap();
    }

    let result = parse_zip_with_recovery(&buf);

    let components = result.component.expect("should have Some (empty vec)");
    assert!(
        components.is_empty(),
        "should have no successful components"
    );
    assert_eq!(result.errors.len(), 2, "should have 2 errors");
}

/// Invalid archive bytes: component is None, 1 error
#[test]
fn test_parse_zip_with_recovery_invalid_archive() {
    let garbage = b"this is definitely not a zip file";
    let result = parse_zip_with_recovery(garbage);

    assert!(result.component.is_none(), "component should be None");
    assert_eq!(result.errors.len(), 1);
    let err_msg = format!("{}", result.errors[0]);
    assert!(
        err_msg.contains("zip archive"),
        "error should mention zip archive: {}",
        err_msg
    );
}

/// Regression: strict parse_zip still fails on first bad file
#[test]
fn test_parse_zip_strict_still_fails() {
    use std::io::Write;

    // Context with an unparseable axiom — triggers error in strict mode
    let invalid_buc = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.contextFile org.eventb.core.configuration="org.eventb.core.fwd" version="3">
    <org.eventb.core.axiom name="'" label="axm1" predicate="@@@ bad"/>
</org.eventb.core.contextFile>"#;

    let valid_bum = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.machineFile org.eventb.core.configuration="org.eventb.core.fwd" version="3">
</org.eventb.core.machineFile>"#;

    let mut buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("Bad.buc", options).unwrap();
        writer.write_all(invalid_buc.as_bytes()).unwrap();
        writer.start_file("Good.bum", options).unwrap();
        writer.write_all(valid_bum.as_bytes()).unwrap();
        writer.finish().unwrap();
    }

    let result = rossi::parse_zip(&buf);
    assert!(result.is_err(), "strict parse_zip should fail on bad file");
}
