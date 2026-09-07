//! Assert label text and byte spans, rather than only grammar acceptance.

mod common;

#[test]
fn label_nodes_match_rossi_names_and_source_spans() {
    let mut parser = common::eventb_parser();
    for name in [
        "a",
        "a:",
        "a::",
        ":",
        "метка:",
        "😀:",
        "a@b",
        "SAF5\"",
        "a//1",
    ] {
        for separator in [" ", "\t", "\u{1c}", "\u{a0}", "\u{2007}"] {
            let source = format!("context c\naxioms\n@{name}{separator}1 = 1\nend\n");
            let rossi::Component::Context(context) = rossi::parse(&source).unwrap() else {
                panic!("expected context");
            };
            assert_eq!(context.axioms[0].label.as_deref(), Some(name));
            let tree = parser.parse(&source, None).unwrap();
            assert!(!tree.root_node().has_error(), "{source}");
            let start = source.find('@').unwrap();
            let node = tree
                .root_node()
                .descendant_for_byte_range(start, start + 1)
                .unwrap();
            assert_eq!(node.kind(), "label", "{source}");
            assert_eq!(node.byte_range(), start..start + 1 + name.len(), "{source}");
            assert_eq!(
                node.utf8_text(source.as_bytes()).unwrap(),
                format!("@{name}")
            );
        }
    }
}
