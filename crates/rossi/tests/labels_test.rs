//! Camille labels are opaque names, terminated only by grammar whitespace.

mod common;

use rossi::{
    Component, PrettyPrinter, Style, format_str, parse, parse_with_recovery, parse_xml, to_xml,
};
use test_case::test_case;

fn labels(component: &Component) -> Vec<&str> {
    match component {
        Component::Context(c) => c.axioms.iter().filter_map(|p| p.label.as_deref()).collect(),
        Component::Machine(m) => m
            .invariants
            .iter()
            .filter_map(|p| p.label.as_deref())
            .chain(m.variants.iter().filter_map(|v| v.label.as_deref()))
            .chain(
                m.initialisation
                    .iter()
                    .flat_map(|e| &e.actions)
                    .filter_map(|a| a.label.as_deref()),
            )
            .chain(m.events.iter().flat_map(|e| {
                e.guards
                    .iter()
                    .chain(&e.with)
                    .chain(&e.witnesses)
                    .filter_map(|p| p.label.as_deref())
                    .chain(e.actions.iter().filter_map(|a| a.label.as_deref()))
            }))
            .collect(),
    }
}

#[test_case("a"; "plain")]
#[test_case("a:"; "colon")]
#[test_case("a::"; "repeated_colon")]
#[test_case(":"; "colon_only")]
#[test_case("group:a"; "internal_colon")]
#[test_case("метка:"; "unicode")]
#[test_case("😀:"; "supplementary_unicode")]
#[test_case("a@b"; "embedded_at")]
#[test_case("SAF5\""; "quote")]
#[test_case("inv//1"; "line_comment_marker")]
#[test_case("inv/*1"; "block_comment_marker")]
#[test_case("a\u{85}"; "non_separator")]
fn strict_and_recovered_labels_match(name: &str) {
    for separator in [
        ' ', '\t', '\n', '\u{0b}', '\u{1c}', '\u{a0}', '\u{2007}', '\u{2028}',
    ] {
        let context = format!(
            "context c\nconstants x\naxioms\n@{name}{separator}x : NAT\n\
             theorem @{name}{separator}x >= 0\nend\n"
        );
        let machine = format!(
            "machine m\nvariables x\ninvariants\n@{name}{separator}x : NAT\n\
             theorem @{name}{separator}x >= 0\nvariant @{name}{separator}x\n\
             events\nevent INITIALISATION then @{name}{separator}x := 0 end\n\
             event e where @{name}{separator}x > 0\nwith @{name}{separator}x = 1\n\
             witness @{name}{separator}x = 1\nthen @{name}{separator}x := x - 1 end\nend\n"
        );
        for (source, declaration, count) in
            [(context, "constants x", 2), (machine, "variables x", 8)]
        {
            let strict = parse(&source).unwrap();
            assert_eq!(labels(&strict), vec![name; count], "{source}");
            let broken = source.replacen(declaration, &format!("{declaration}\n+"), 1);
            let recovered = parse_with_recovery(&broken);
            assert!(recovered.has_recovered());
            assert_eq!(
                recovered.errors.len(),
                1,
                "{broken}\n{:?}",
                recovered.errors
            );
            assert_eq!(
                labels(recovered.component.as_ref().unwrap()),
                labels(&strict),
                "{broken}"
            );
        }
    }
}

#[test_case("axm1: 1 = 1")]
#[test_case("метка: 1 = 1 // note: comment")]
#[test_case("@ 1 = 1")]
fn invalid_labels_are_not_recovered_as_predicates(item: &str) {
    let source = format!("context c\naxioms\n{item}\n@valid 1 = 1\nend\n");
    assert!(parse(&source).is_err());
    let recovered = parse_with_recovery(&source);
    assert!(recovered.has_recovered());
    assert_eq!(labels(recovered.component.as_ref().unwrap()), ["valid"]);
}

#[test]
fn formatting_preserves_distinct_labels() {
    let source = "context c\naxioms\n@a 1 = 1\n@a: 1 = 1\n@a:: 1 = 1\n@: 1 = 1\n\
                  @group:a 1 = 1 & 2 = 2 & 3 = 3 & 4 = 4\nend\n";
    let expected = ["a", "a:", "a::", ":", "group:a"];
    for style in [Style::Camille, Style::Rossi] {
        for ascii in [false, true] {
            for width in [0, 30] {
                let mut printer = PrettyPrinter::styled(style).with_max_line_width(width);
                printer.use_unicode = !ascii;
                let output = common::format_checked(source, &printer);
                assert_eq!(labels(&parse(&output).unwrap()), expected);
                assert_eq!(format_str(&output, &printer).unwrap(), output);
            }
        }
    }
}

#[test]
fn rodin_xml_labels_survive_text_roundtrip() {
    let xml = r#"<org.eventb.core.contextFile version="3">
        <org.eventb.core.axiom name="_a" org.eventb.core.label="a"
          org.eventb.core.predicate="1 = 1"/>
        <org.eventb.core.axiom name="_b" org.eventb.core.label="a:"
          org.eventb.core.predicate="1 = 1"/>
        <org.eventb.core.axiom name="_c" org.eventb.core.label="a::"
          org.eventb.core.predicate="1 = 1"/>
        <org.eventb.core.axiom name="_d" org.eventb.core.label=":"
          org.eventb.core.predicate="1 = 1"/>
    </org.eventb.core.contextFile>"#;
    let original = parse_xml(xml).unwrap();
    let text = rossi::to_string(&original);
    let reparsed = parse(&text).unwrap();
    assert_eq!(labels(&reparsed), labels(&original));
    assert_eq!(
        labels(&parse_xml(&to_xml(&reparsed)).unwrap()),
        labels(&original)
    );
}
