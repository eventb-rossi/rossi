//! Declaration type inlay hints: inferred types rendered at declaration
//! sites, computed over the document's dependency closure from current
//! buffer snapshots.

use std::sync::Arc;

use eventb_lsp::config::RossiConfig;
use eventb_lsp::cross_references::CrossReferenceManager;
use eventb_lsp::document::DocumentManager;
use eventb_lsp::inlay_hints::InlayHintsProvider;
use eventb_lsp::lsp_types::{
    InlayHint, InlayHintKind, InlayHintLabel, InlayHintTooltip, Position, Range,
    TextDocumentContentChangeEvent, Uri,
};
use eventb_lsp::position::offset_to_position;

struct Fixture {
    documents: Arc<DocumentManager>,
    manager: Arc<CrossReferenceManager>,
    provider: InlayHintsProvider,
}

impl Fixture {
    fn new() -> Self {
        let documents = Arc::new(DocumentManager::new());
        let manager = Arc::new(CrossReferenceManager::new());
        let provider = InlayHintsProvider::new(Arc::clone(&documents), Arc::clone(&manager));
        Self {
            documents,
            manager,
            provider,
        }
    }

    fn open(&self, uri: &str, text: &str) -> Uri {
        let url = uri.parse::<Uri>().unwrap();
        self.manager.update_component(uri.to_owned(), text);
        self.documents.open(url.clone(), 1, text.to_string());
        url
    }

    fn hints(&self, uri: &Uri, config: &Arc<RossiConfig>) -> Vec<InlayHint> {
        self.provider
            .inlay_hints(uri, full_range(), config)
            .expect("document is open")
    }
}

fn full_range() -> Range {
    Range::new(Position::new(0, 0), Position::new(u32::MAX, u32::MAX))
}

fn default_config() -> Arc<RossiConfig> {
    Arc::new(RossiConfig::default())
}

/// The default configuration with one tweak applied.
fn config_with(tweak: impl FnOnce(&mut RossiConfig)) -> Arc<RossiConfig> {
    let mut config = RossiConfig::default();
    tweak(&mut config);
    Arc::new(config)
}

/// A machine with one ℤ-typed variable — the smallest hintable document.
const INT_MACHINE: &str = "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ∈ ℤ\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x := 0\n    END\nEND\n";

/// [`INT_MACHINE`] plus an invariant dividing by `x` — one type hint plus
/// one WD marker.
const WD_MACHINE: &str = "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ∈ ℤ\n    @inv2 10 ÷ x > 0\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x := 1\n    END\nEND\n";

/// The labels of every hint anchored right after an occurrence of `name`
/// in `text`, in occurrence order.
fn labels_at(hints: &[InlayHint], text: &str, name: &str) -> Vec<String> {
    let mut labels = Vec::new();
    let mut offset = 0;
    while let Some(found) = text[offset..].find(name) {
        let end = offset + found + name.len();
        let position = offset_to_position(text, end);
        for hint in hints {
            if hint.position == position {
                let InlayHintLabel::String(label) = &hint.label else {
                    panic!("string label expected");
                };
                labels.push(label.clone());
            }
        }
        offset = end;
    }
    labels
}

fn label_text(hint: &InlayHint) -> &str {
    match &hint.label {
        InlayHintLabel::String(label) => label,
        InlayHintLabel::LabelParts(_) => panic!("string label expected"),
    }
}

fn tooltip_text(hint: &InlayHint) -> Option<&str> {
    match &hint.tooltip {
        Some(InlayHintTooltip::String(text)) => Some(text),
        Some(InlayHintTooltip::MarkupContent(_)) => panic!("string tooltip expected"),
        None => None,
    }
}

#[test]
fn machine_variables_are_hinted_at_their_declaration() {
    let fixture = Fixture::new();
    let text = INT_MACHINE;
    let uri = fixture.open("file:///m.eventb", text);

    let hints = fixture.hints(&uri, &default_config());

    assert_eq!(hints.len(), 1, "one variable, one hint: {hints:?}");
    let declaration_end = offset_to_position(text, text.find("    x\n").unwrap() + 5);
    assert_eq!(hints[0].position, declaration_end);
    assert_eq!(label_text(&hints[0]), ": ℤ");
    assert_eq!(hints[0].kind, Some(InlayHintKind::TYPE));
    assert_eq!(tooltip_text(&hints[0]), None);
}

#[test]
fn event_parameters_are_hinted_from_their_guards() {
    let fixture = Fixture::new();
    let text = "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ∈ ℤ\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x := 0\n    END\n    EVENT step\n    ANY\n        p\n    WHERE\n        @grd1 p ∈ ℤ ↔ ℤ\n    THEN\n        @act1 x := x + 1\n    END\nEND\n";
    let uri = fixture.open("file:///m.eventb", text);

    let hints = fixture.hints(&uri, &default_config());

    assert_eq!(
        labels_at(&hints, text, "p"),
        vec![": ℙ(ℤ×ℤ)"],
        "the relation-typed parameter is hinted at its ANY declaration"
    );
}

#[test]
fn constants_are_hinted_and_carrier_sets_are_not() {
    let fixture = Fixture::new();
    let text = "CONTEXT c\nSETS\n    S\nCONSTANTS\n    k\nAXIOMS\n    @axm1 k ∈ S\nEND\n";
    let uri = fixture.open("file:///c.eventb", text);

    let hints = fixture.hints(&uri, &default_config());

    assert_eq!(hints.len(), 1, "the carrier set gets no hint: {hints:?}");
    assert_eq!(labels_at(&hints, text, "k"), vec![": S"]);
}

#[test]
fn seen_context_types_resolve_across_files() {
    let fixture = Fixture::new();
    fixture.open(
        "file:///c.eventb",
        "CONTEXT c\nSETS\n    S\nCONSTANTS\n    k\nAXIOMS\n    @axm1 k ∈ S\nEND\n",
    );
    let text = "MACHINE m\nSEES\n    c\nVARIABLES\n    v\nINVARIANTS\n    @inv1 v ⊆ S\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 v := ∅\n    END\nEND\n";
    let uri = fixture.open("file:///m.eventb", text);

    let hints = fixture.hints(&uri, &default_config());

    assert_eq!(labels_at(&hints, text, "v"), vec![": ℙ(S)"]);
}

#[test]
fn refinement_hints_only_declaration_sites_in_this_file() {
    let fixture = Fixture::new();
    fixture.open(
        "file:///abstract.eventb",
        "MACHINE abstract\nVARIABLES\n    kept\n    dropped\nINVARIANTS\n    @inv1 kept ∈ ℤ\n    @inv2 dropped ∈ ℤ\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 kept := 0\n        @act2 dropped := 0\n    END\nEND\n",
    );
    // `kept` is redeclared (its typing invariant lives one machine up);
    // `dropped` is not redeclared and must not be hinted here.
    let text = "MACHINE concrete\nREFINES\n    abstract\nVARIABLES\n    kept\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 kept := 0\n    END\nEND\n";
    let uri = fixture.open("file:///concrete.eventb", text);

    let hints = fixture.hints(&uri, &default_config());

    assert_eq!(hints.len(), 1, "{hints:?}");
    let declaration_end = offset_to_position(text, text.find("    kept\n").unwrap() + 8);
    assert_eq!(hints[0].position, declaration_end);
    assert_eq!(label_text(&hints[0]), ": ℤ");
}

#[test]
fn untypeable_declarations_are_skipped_but_siblings_are_hinted() {
    let fixture = Fixture::new();
    let text = "MACHINE m\nVARIABLES\n    typed\n    mystery\nINVARIANTS\n    @inv1 typed ∈ ℤ\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 typed := 0\n    END\nEND\n";
    let uri = fixture.open("file:///m.eventb", text);

    let hints = fixture.hints(&uri, &default_config());

    assert_eq!(labels_at(&hints, text, "typed"), vec![": ℤ"]);
    assert_eq!(labels_at(&hints, text, "mystery"), Vec::<String>::new());
}

#[test]
fn a_broken_sibling_component_does_not_suppress_healthy_hints() {
    let fixture = Fixture::new();
    let text = format!("{INT_MACHINE}\nMACHINE broken\nVARIABLES\n    +\nEND\n");
    let uri = fixture.open("file:///m.eventb", &text);

    let hints = fixture.hints(&uri, &default_config());

    assert_eq!(labels_at(&hints, &text, "x"), vec![": ℤ"]);
}

#[test]
fn hints_are_clipped_to_the_requested_range() {
    let fixture = Fixture::new();
    let text = "MACHINE m\nVARIABLES\n    x\n    y\nINVARIANTS\n    @inv1 x ∈ ℤ\n    @inv2 y ∈ ℤ\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x := 0\n        @act2 y := 0\n    END\nEND\n";
    let uri = fixture.open("file:///m.eventb", text);
    let config = default_config();

    let all = fixture.hints(&uri, &config);
    assert_eq!(all.len(), 2);

    // Only the line declaring `x`.
    let clipped = fixture
        .provider
        .inlay_hints(
            &uri,
            Range::new(Position::new(2, 0), Position::new(3, 0)),
            &config,
        )
        .unwrap();
    assert_eq!(clipped.len(), 1);
    assert_eq!(clipped[0].position.line, 2);
}

#[test]
fn long_types_truncate_with_the_full_type_as_tooltip() {
    let fixture = Fixture::new();
    let text = "MACHINE m\nVARIABLES\n    r\nINVARIANTS\n    @inv1 r ∈ ℤ ↔ (ℤ ↔ (ℤ ↔ ℤ))\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 r := ∅\n    END\nEND\n";
    let uri = fixture.open("file:///m.eventb", text);
    let full_type = "ℙ(ℤ×ℙ(ℤ×ℙ(ℤ×ℤ)))";

    let hints = fixture.hints(&uri, &config_with(|c| c.inlay_hints.max_length = 8));
    assert_eq!(hints.len(), 1);
    let expected: String = full_type.chars().take(7).collect();
    assert_eq!(label_text(&hints[0]), format!(": {expected}…"));
    assert_eq!(tooltip_text(&hints[0]), Some(full_type));

    let hints = fixture.hints(&uri, &config_with(|c| c.inlay_hints.max_length = 0));
    assert_eq!(label_text(&hints[0]), format!(": {full_type}"));
    assert_eq!(tooltip_text(&hints[0]), None);
}

#[test]
fn ascii_configuration_renders_ascii_type_spellings() {
    let fixture = Fixture::new();
    let text = "MACHINE m\nVARIABLES\n    s\nINVARIANTS\n    @inv1 s ⊆ ℤ\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 s := ∅\n    END\nEND\n";
    let uri = fixture.open("file:///m.eventb", text);

    let hints = fixture.hints(&uri, &config_with(|c| c.format.use_unicode = false));

    assert_eq!(hints.len(), 1);
    assert_eq!(label_text(&hints[0]), ": POW(INT)");
}

#[test]
fn an_edit_invalidates_the_cached_hints() {
    let fixture = Fixture::new();
    let uri = fixture.open("file:///m.eventb", INT_MACHINE);
    let config = default_config();

    let before = fixture.hints(&uri, &config);
    assert_eq!(label_text(&before[0]), ": ℤ");

    let retyped = "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ⊆ ℤ\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x := ∅\n    END\nEND\n";
    fixture.documents.change(
        &uri,
        2,
        vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: retyped.to_string(),
        }],
    );

    let after = fixture.hints(&uri, &config);
    assert_eq!(label_text(&after[0]), ": ℙ(ℤ)");
}

#[test]
fn well_definedness_markers_carry_the_lemma_tooltip() {
    let fixture = Fixture::new();
    let text = WD_MACHINE;
    let uri = fixture.open("file:///m.eventb", text);

    let hints = fixture.hints(&uri, &default_config());

    // One type hint on `x`, one WD marker on the dividing invariant.
    assert_eq!(hints.len(), 2, "{hints:?}");
    let wd = &hints[1];
    let formula = "10 ÷ x > 0";
    let formula_end = offset_to_position(text, text.find(formula).unwrap() + formula.len());
    assert_eq!(wd.position, formula_end);
    assert_eq!(label_text(wd), "WD");
    assert_eq!(wd.kind, None);
    assert_eq!(wd.padding_left, Some(true));
    let Some(InlayHintTooltip::MarkupContent(tooltip)) = &wd.tooltip else {
        panic!("markup tooltip expected: {:?}", wd.tooltip);
    };
    assert!(
        tooltip.value.contains("Well-definedness condition:") && tooltip.value.contains('≠'),
        "tooltip must show the rendered lemma: {}",
        tooltip.value
    );
}

#[test]
fn well_definedness_tooltips_respect_the_ascii_configuration() {
    let fixture = Fixture::new();
    let text = WD_MACHINE;
    let uri = fixture.open("file:///m.eventb", text);

    let hints = fixture.hints(&uri, &config_with(|c| c.format.use_unicode = false));

    let Some(InlayHintTooltip::MarkupContent(tooltip)) = &hints[1].tooltip else {
        panic!("markup tooltip expected");
    };
    assert!(
        tooltip.value.contains("/="),
        "the lemma must use the ASCII operator spelling: {}",
        tooltip.value
    );
}

#[test]
fn long_well_definedness_tooltips_wrap_at_the_configured_width() {
    let fixture = Fixture::new();
    // Three applications of the partial `f` give a multi-conjunct lemma
    // well past 40 characters.
    let text = "MACHINE m\nVARIABLES\n    f\n    x\nINVARIANTS\n    @inv1 f ∈ ℤ ⇸ ℤ\n    @inv2 x ∈ ℤ\n    @inv3 f(x) + f(x + 1) + f(x + 2) > 0\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 f := ∅\n        @act2 x := 0\n    END\nEND\n";
    let uri = fixture.open("file:///m.eventb", text);

    let lemma_lines = |config| {
        let hints = fixture.hints(&uri, &config);
        let wd = hints
            .iter()
            .find(|hint| label_text(hint) == "WD")
            .expect("a WD hint");
        let Some(InlayHintTooltip::MarkupContent(tooltip)) = &wd.tooltip else {
            panic!("markup tooltip expected: {:?}", wd.tooltip);
        };
        let lines: Vec<String> = tooltip
            .value
            .lines()
            .skip(2)
            .take_while(|line| *line != "```")
            .map(str::to_string)
            .collect();
        assert!(!lines.is_empty(), "no lemma in tooltip: {}", tooltip.value);
        lines
    };

    let wrapped = lemma_lines(config_with(|c| c.format.max_line_width = 40));
    assert!(wrapped.len() > 1, "expected a wrapped lemma: {wrapped:?}");
    for line in &wrapped {
        assert!(
            line.chars().count() <= 40,
            "line exceeds 40 chars: {line:?}"
        );
    }

    let flat = lemma_lines(config_with(|c| c.format.max_line_width = 0));
    assert_eq!(flat.len(), 1, "width 0 must keep the lemma flat: {flat:?}");
}

#[test]
fn well_definedness_markers_can_be_disabled() {
    let fixture = Fixture::new();
    let text = WD_MACHINE;
    let uri = fixture.open("file:///m.eventb", text);

    let hints = fixture.hints(
        &uri,
        &config_with(|c| c.inlay_hints.well_definedness = false),
    );

    assert_eq!(hints.len(), 1, "only the type hint remains: {hints:?}");
    assert_eq!(label_text(&hints[0]), ": ℤ");
}

#[test]
fn dependency_wd_conditions_do_not_leak_into_this_file() {
    let fixture = Fixture::new();
    let context_text =
        "CONTEXT c\nCONSTANTS\n    k\nAXIOMS\n    @axm1 k ∈ ℤ\n    @axm2 10 ÷ k > 0\nEND\n";
    let context_uri = fixture.open("file:///c.eventb", context_text);
    let machine_text = "MACHINE m\nSEES\n    c\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ∈ ℤ\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x := k\n    END\nEND\n";
    let machine_uri = fixture.open("file:///m.eventb", machine_text);

    let config = default_config();
    let machine_hints = fixture.hints(&machine_uri, &config);
    assert_eq!(
        machine_hints.len(),
        1,
        "the seen context's WD condition must not appear here: {machine_hints:?}"
    );
    assert_eq!(label_text(&machine_hints[0]), ": ℤ");

    // The context's own file does carry the marker.
    let context_hints = fixture.hints(&context_uri, &config);
    assert_eq!(context_hints.len(), 2, "{context_hints:?}");
    assert_eq!(label_text(&context_hints[1]), "WD");
}

#[test]
fn well_definedness_markers_skip_a_trailing_comment() {
    let fixture = Fixture::new();
    let text = WD_MACHINE.replace("> 0\n", "> 0 // may divide\n");
    let uri = fixture.open("file:///m.eventb", &text);

    let hints = fixture.hints(&uri, &default_config());

    // The formula span swallows the trailing comment; the marker must still
    // anchor at the formula's last visible character.
    assert_eq!(hints.len(), 2, "{hints:?}");
    let formula = "10 ÷ x > 0";
    let formula_end = offset_to_position(&text, text.find(formula).unwrap() + formula.len());
    assert_eq!(hints[1].position, formula_end, "{hints:?}");
    assert_eq!(label_text(&hints[1]), "WD");
}

#[test]
fn closing_a_dependency_invalidates_the_cached_hints() {
    let fixture = Fixture::new();
    let context_text = "CONTEXT c\nCONSTANTS\n    k\nAXIOMS\n    @axm1 k ∈ ℤ\nEND\n";
    let context_uri = fixture.open("file:///c.eventb", context_text);
    let machine_text = "MACHINE m\nSEES\n    c\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x = k\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x := k\n    END\nEND\n";
    let machine_uri = fixture.open("file:///m.eventb", machine_text);
    let config = default_config();

    // Hints computed while c's open buffer types k as BOOL.
    fixture.documents.change(
        &context_uri,
        2,
        vec![TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: context_text.replace("k ∈ ℤ", "k ∈ BOOL"),
        }],
    );
    let edited = fixture.hints(&machine_uri, &config);
    assert_eq!(label_text(&edited[0]), ": BOOL");

    // Closing c discards the buffer (this fixture has no disk fallback), so
    // the machine can no longer type `x`: hints derived from the discarded
    // buffer must not keep being served from the cache.
    fixture.documents.close(&context_uri);
    let restored = fixture.hints(&machine_uri, &config);
    assert!(
        restored.is_empty(),
        "hints from the closed buffer must not survive: {restored:?}"
    );
}

#[test]
fn duplicate_component_names_get_no_hints() {
    // Two same-name machines are an EB019 error on the build path; the
    // second copy must not be silently annotated with the first copy's types.
    let fixture = Fixture::new();
    let second_copy = INT_MACHINE
        .replace("x ∈ ℤ", "x ∈ BOOL")
        .replace("x := 0", "x := TRUE");
    let text = format!("{INT_MACHINE}\n{second_copy}");
    let uri = fixture.open("file:///m.eventb", &text);

    let hints = fixture.hints(&uri, &default_config());

    assert!(hints.is_empty(), "{hints:?}");
}
