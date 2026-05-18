use indoc::indoc;
use rossi::{Component, components_to_string, parse_components};

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

    let names: Vec<&str> = result
        .iter()
        .map(|c| match c {
            Component::Context(ctx) => ctx.name.as_str(),
            Component::Machine(m) => m.name.as_str(),
        })
        .collect();
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
        let orig_name = match orig {
            Component::Context(c) => &c.name,
            Component::Machine(m) => &m.name,
        };
        let re_name = match re {
            Component::Context(c) => &c.name,
            Component::Machine(m) => &m.name,
        };
        assert_eq!(orig_name, re_name);
    }
}

#[test]
fn test_empty_input_is_error() {
    let result = parse_components("");
    assert!(result.is_err());
}
