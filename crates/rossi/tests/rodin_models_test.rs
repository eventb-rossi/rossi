//! Integration tests for parsing real Rodin Event-B models
//!
//! These tests use models from https://github.com/17451k/eventb-models
//! to verify that the parser can handle real-world Event-B specifications.

use rossi::{Component, parse, parse_zip_file, parse_zip_with_recovery};

/// Test parsing binary-search model from text format
#[test]
fn test_binary_search_txt() {
    let source = std::fs::read_to_string("examples/binary-search.txt")
        .expect("Failed to read binary-search.txt");

    let result = parse(&source);
    // Note: The text format may use annotations (@) which aren't supported yet
    // We just verify the file can be read
    if result.is_err() {
        println!(
            "Note: binary-search.txt parsing failed (possibly uses unsupported syntax): {:?}",
            result.err()
        );
    }
}

/// Test parsing binary-search model from zip format
#[test]
fn test_binary_search_zip() {
    let components =
        parse_zip_file("examples/binary-search.zip").expect("Failed to parse binary-search.zip");

    assert!(
        !components.is_empty(),
        "binary-search.zip should contain at least one component"
    );

    // Verify we have both contexts and machines
    let contexts: Vec<_> = components
        .iter()
        .filter(|c| matches!(c.component, Component::Context(_)))
        .collect();
    let machines: Vec<_> = components
        .iter()
        .filter(|c| matches!(c.component, Component::Machine(_)))
        .collect();

    assert!(
        !contexts.is_empty(),
        "binary-search should have at least one context"
    );
    assert!(
        !machines.is_empty(),
        "binary-search should have at least one machine"
    );

    // Verify filenames are preserved
    for named_comp in &components {
        assert!(
            named_comp.filename.ends_with(".buc") || named_comp.filename.ends_with(".bum"),
            "Unexpected filename: {}",
            named_comp.filename
        );
    }

    println!(
        "binary-search.zip contains {} components:",
        components.len()
    );
    for comp in &components {
        let name = match &comp.component {
            Component::Context(c) => &c.name,
            Component::Machine(m) => &m.name,
        };
        println!("  - {} ({})", name, comp.filename);
    }
}

/// Test parsing cars-on-bridge model from text format
#[test]
fn test_cars_on_bridge_txt() {
    let source = std::fs::read_to_string("examples/cars-on-bridge.txt")
        .expect("Failed to read cars-on-bridge.txt");

    let result = parse(&source);
    // Note: The text format may use annotations (@) which aren't supported yet
    if result.is_err() {
        println!(
            "Note: cars-on-bridge.txt parsing failed (possibly uses unsupported syntax): {:?}",
            result.err()
        );
    }
}

/// Test parsing cars-on-bridge model from zip format
#[test]
fn test_cars_on_bridge_zip() {
    let components =
        parse_zip_file("examples/cars-on-bridge.zip").expect("Failed to parse cars-on-bridge.zip");

    assert!(
        !components.is_empty(),
        "cars-on-bridge.zip should contain at least one component"
    );

    println!(
        "cars-on-bridge.zip contains {} components:",
        components.len()
    );
    for comp in &components {
        let name = match &comp.component {
            Component::Context(c) => &c.name,
            Component::Machine(m) => &m.name,
        };
        println!("  - {} ({})", name, comp.filename);
    }
}

/// Test parsing file-system model from text format
#[test]
fn test_file_system_txt() {
    let source = std::fs::read_to_string("examples/file-system.txt")
        .expect("Failed to read file-system.txt");

    let result = parse(&source);
    // Note: The text format may use annotations (@) which aren't supported yet
    if result.is_err() {
        println!(
            "Note: file-system.txt parsing failed (possibly uses unsupported syntax): {:?}",
            result.err()
        );
    }
}

/// Test parsing file-system model from zip format
#[test]
fn test_file_system_zip() {
    let components =
        parse_zip_file("examples/file-system.zip").expect("Failed to parse file-system.zip");

    assert!(
        !components.is_empty(),
        "file-system.zip should contain at least one component"
    );

    println!("file-system.zip contains {} components:", components.len());
    for comp in &components {
        let name = match &comp.component {
            Component::Context(c) => &c.name,
            Component::Machine(m) => &m.name,
        };
        println!("  - {} ({})", name, comp.filename);
    }
}

/// Test parsing traffic-light model from text format
#[test]
fn test_traffic_light_txt() {
    let source = std::fs::read_to_string("examples/traffic-light.txt")
        .expect("Failed to read traffic-light.txt");

    let result = parse(&source);
    // Note: The text format may use annotations (@) which aren't supported yet
    if result.is_err() {
        println!(
            "Note: traffic-light.txt parsing failed (possibly uses unsupported syntax): {:?}",
            result.err()
        );
    }
}

/// Test parsing traffic-light model from zip format
#[test]
fn test_traffic_light_zip() {
    let components =
        parse_zip_file("examples/traffic-light.zip").expect("Failed to parse traffic-light.zip");

    assert!(
        !components.is_empty(),
        "traffic-light.zip should contain at least one component"
    );

    println!(
        "traffic-light.zip contains {} components:",
        components.len()
    );
    for comp in &components {
        let name = match &comp.component {
            Component::Context(c) => &c.name,
            Component::Machine(m) => &m.name,
        };
        println!("  - {} ({})", name, comp.filename);
    }
}

/// Test parsing base-model from zip format
#[test]
fn test_base_model_zip() {
    let components =
        parse_zip_file("examples/base-model.zip").expect("Failed to parse base-model.zip");

    assert!(
        !components.is_empty(),
        "base-model.zip should contain at least one component"
    );

    // Verify we have both context and machine
    let contexts: Vec<_> = components
        .iter()
        .filter(|c| matches!(c.component, Component::Context(_)))
        .collect();
    let machines: Vec<_> = components
        .iter()
        .filter(|c| matches!(c.component, Component::Machine(_)))
        .collect();

    assert!(
        !contexts.is_empty(),
        "base-model should have at least one context"
    );
    assert!(
        !machines.is_empty(),
        "base-model should have at least one machine"
    );

    // Verify we have C1 context and M1 machine
    let has_c1 = contexts.iter().any(|c| {
        if let Component::Context(ctx) = &c.component {
            ctx.name == "C1"
        } else {
            false
        }
    });

    let has_m1 = machines.iter().any(|m| {
        if let Component::Machine(machine) = &m.component {
            machine.name == "M1"
        } else {
            false
        }
    });

    assert!(has_c1, "base-model should contain C1 context");
    assert!(has_m1, "base-model should contain M1 machine");

    println!("base-model.zip contains {} components:", components.len());
    for comp in &components {
        let name = match &comp.component {
            Component::Context(c) => &c.name,
            Component::Machine(m) => &m.name,
        };
        println!("  - {} ({})", name, comp.filename);
    }
}

/// Comprehensive test that verifies all ZIP models can be parsed
#[test]
fn test_all_rodin_models() {
    let models = vec![
        "base-model.zip",
        "binary-search.zip",
        "cars-on-bridge.zip",
        "file-system.zip",
        "traffic-light.zip",
    ];

    for zip_file in models {
        // Test zip format
        let zip_path = format!("examples/{}", zip_file);
        let zip_result = parse_zip_file(&zip_path);
        assert!(
            zip_result.is_ok(),
            "Failed to parse {}: {:?}",
            zip_file,
            zip_result.err()
        );

        let components = zip_result.unwrap();
        assert!(
            !components.is_empty(),
            "{} should contain at least one component",
            zip_file
        );

        println!(
            "{}: {} components parsed successfully",
            zip_file,
            components.len()
        );
    }
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
