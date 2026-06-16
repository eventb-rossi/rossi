//! LSP integration tests over the bundled example models.
//!
//! The Rodin project archives in `crates/rossi/examples` are converted to
//! textual Event-B through `parse_zip_file` + the default pretty printer (the
//! core of `rossi import`, which additionally appends one trailing newline per
//! written file), and the LSP providers are driven directly against the
//! result: exact cross-file assertions on the compact cars-on-bridge model,
//! invariant assertions on every file of all bundled models.
//!
//! A second, merged pass re-runs the providers with each model concatenated
//! into ONE multi-component document (the shape `rossi import --merge`
//! produces), with exact assertions on the checked-in merged traffic-light
//! fixture.
//!
//! The archives ship with the repo, so the suite runs on every plain
//! `cargo test` — no environment variables, no `--ignored`.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use rossi_lsp::analysis;
use rossi_lsp::cross_references::{CrossReferenceManager, ReferenceKind};
use rossi_lsp::definition::DefinitionProvider;
use rossi_lsp::document::DocumentManager;
use rossi_lsp::document_links::DocumentLinkProvider;
use rossi_lsp::folding::FoldingRangeProvider;
use rossi_lsp::formatting::FormattingProvider;
use rossi_lsp::hover::HoverProvider;
use rossi_lsp::identifier_utils::{WordBoundary, find_whole_word_locations, position_to_offset};
use rossi_lsp::lsp_types::*;
use rossi_lsp::references::ReferenceProvider;
use rossi_lsp::rename::RenameProvider;
use rossi_lsp::selection_range::SelectionRangeProvider;
use rossi_lsp::workspace::WorkspaceSymbolProvider;

mod common;
use common::{decode_tokens, slice_range};

/// The checked-in merged form of the traffic-light model (M0, C1, M1, M2 in
/// one file) — the document shape `rossi import --merge` produces.
const MERGED_TRAFFIC_LIGHT: &str = include_str!("../../rossi/examples/traffic-light.txt");

const CARS: &str = "cars-on-bridge.zip";
const BINARY_SEARCH: &str = "binary-search.zip";
const BASE_MODEL: &str = "base-model.zip";
const TRAFFIC_LIGHT: &str = "traffic-light.zip";
const FILE_SYSTEM: &str = "file-system.zip";

/// Every bundled model with its expected component count (.buc + .bum).
const ALL_MODELS: &[(&str, usize)] = &[
    (CARS, 7),
    (BINARY_SEARCH, 5),
    (BASE_MODEL, 2),
    (TRAFFIC_LIGHT, 4),
    (FILE_SYSTEM, 2),
];

// ============================================================================
// Model location
// ============================================================================

fn examples_dir() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // crates/
    path.push("rossi");
    path.push("examples");
    assert!(
        path.is_dir(),
        "bundled examples directory missing: {}",
        path.display()
    );
    path
}

// ============================================================================
// Model loading and workspace setup
// ============================================================================

/// One converted component: parsed once at load, printed once, with the
/// synthetic workspace URI derived from its model and name.
///
/// In a merged workspace ([`Workspace::open_merged`]) every component shares
/// one URI and `text` is the whole merged document; `start_line` then marks
/// where this component begins, so position helpers can search within it.
struct ModelFile {
    name: String,
    uri: Url,
    component: rossi::Component,
    text: String,
    /// 0-indexed first line of this component within `text` (0 when the
    /// component has its own file).
    start_line: usize,
}

/// Convert every `.buc`/`.bum` in the archive to textual Event-B, in zip order.
fn load_model(zip_name: &str) -> Vec<ModelFile> {
    let model = zip_name.trim_end_matches(".zip");
    let path = examples_dir().join(zip_name);
    rossi::parse_zip_file(&path)
        .unwrap_or_else(|e| panic!("{}: parse_zip_file failed: {e}", path.display()))
        .into_iter()
        .map(|named| {
            let name = named.component.name().to_string();
            ModelFile {
                uri: Url::parse(&format!("file:///{model}/{name}.eventb")).unwrap(),
                text: rossi::to_string(&named.component),
                component: named.component,
                name,
                start_line: 0,
            }
        })
        .collect()
}

/// Merge a model's converted components into one text the way
/// `rossi import --merge` does: zip order, a blank line between components.
fn merged_model_text(files: &[ModelFile]) -> String {
    let mut output = String::new();
    for (i, file) in files.iter().enumerate() {
        if i > 0 {
            output.push('\n');
        }
        output.push_str(&file.text);
        output.push('\n');
    }
    output
}

/// Every file of every bundled model, tagged with its archive name.
fn all_model_files() -> Vec<(&'static str, ModelFile)> {
    ALL_MODELS
        .iter()
        .flat_map(|&(zip_name, _)| {
            load_model(zip_name)
                .into_iter()
                .map(move |file| (zip_name, file))
        })
        .collect()
}

/// A converted model registered in both managers, the way the live server
/// holds open documents.
struct Workspace {
    crm: Arc<CrossReferenceManager>,
    dm: Arc<DocumentManager>,
    files: Vec<ModelFile>,
}

impl Workspace {
    fn open(zip_name: &str) -> Self {
        let model = zip_name.trim_end_matches(".zip");
        let files = load_model(zip_name);

        // The cross-reference name→URI map is flat; a model with duplicate
        // component names would make every lookup ambiguous.
        let names: HashSet<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names.len(),
            files.len(),
            "{model}: duplicate component names in model"
        );

        let crm = Arc::new(CrossReferenceManager::new());
        let dm = Arc::new(DocumentManager::new());
        // Both registrations are required: cross-file features load other
        // files through the DocumentManager first and fall back to disk,
        // and these synthetic URIs have no disk file behind them.
        for file in &files {
            crm.update_component(file.uri.to_string(), &file.text);
            dm.open(file.uri.clone(), "eventb".to_string(), 1, file.text.clone());
        }
        assert_eq!(
            crm.all_component_names().len(),
            files.len(),
            "{model}: some converted components failed to parse/index"
        );

        Self { crm, dm, files }
    }

    /// Open a model with all of its components merged into ONE document, the
    /// shape `rossi import --merge` produces.
    fn open_merged(zip_name: &str) -> Self {
        let model = zip_name.trim_end_matches(".zip");
        Self::from_merged_text(model, &merged_model_text(&load_model(zip_name)))
    }

    /// Open a workspace whose single document `text` holds one or more
    /// components. Every [`ModelFile`] shares the document's URI and text;
    /// `start_line` locates the component within it.
    fn from_merged_text(model: &str, text: &str) -> Self {
        let components = rossi::parse_components(text)
            .unwrap_or_else(|e| panic!("{model}: merged text does not parse: {e}"));
        let uri = Url::parse(&format!("file:///{model}/merged.eventb")).unwrap();

        let files: Vec<ModelFile> = components
            .into_iter()
            .map(|component| {
                let span = component
                    .span()
                    .unwrap_or_else(|| panic!("{model}: strict parse must record spans"));
                ModelFile {
                    name: component.name().to_string(),
                    uri: uri.clone(),
                    start_line: text[..span.start].matches('\n').count(),
                    component,
                    text: text.to_string(),
                }
            })
            .collect();

        let names: HashSet<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names.len(),
            files.len(),
            "{model}: duplicate component names"
        );

        let crm = Arc::new(CrossReferenceManager::new());
        let dm = Arc::new(DocumentManager::new());
        crm.update_component(uri.to_string(), text);
        dm.open(uri, "eventb".to_string(), 1, text.to_string());
        assert_eq!(
            crm.all_component_names().len(),
            files.len(),
            "{model}: every merged component must be indexed"
        );

        Self { crm, dm, files }
    }

    /// Register an extra standalone document (its own URI) alongside the
    /// workspace's existing files.
    fn add_document(&mut self, model: &str, text: &str) {
        let component = rossi::parse(text).expect("extra document must parse");
        let name = component.name().to_string();
        let uri = Url::parse(&format!("file:///{model}/{name}.eventb")).unwrap();
        self.crm.update_component(uri.to_string(), text);
        self.dm
            .open(uri.clone(), "eventb".to_string(), 1, text.to_string());
        self.files.push(ModelFile {
            name,
            uri,
            component,
            text: text.to_string(),
            start_line: 0,
        });
    }

    fn entry(&self, name: &str) -> &ModelFile {
        self.files
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("no component named {name}"))
    }

    fn text(&self, name: &str) -> &str {
        &self.entry(name).text
    }

    fn uri(&self, name: &str) -> Url {
        self.entry(name).uri.clone()
    }

    fn text_for_uri(&self, uri: &Url) -> &str {
        &self
            .files
            .iter()
            .find(|f| f.uri == *uri)
            .unwrap_or_else(|| panic!("no component at {uri}"))
            .text
    }
}

// ============================================================================
// Position and text helpers (char-based columns, like the providers)
// ============================================================================

fn probe_uri() -> Url {
    Url::parse("file:///probe.eventb").unwrap()
}

/// Char-based start position of the `n`-th whole-word occurrence of `word`.
fn nth_occurrence(text: &str, word: &str, n: usize) -> Position {
    let locations =
        find_whole_word_locations(text, word, &probe_uri(), None, WordBoundary::MathIdentifier);
    locations
        .get(n)
        .unwrap_or_else(|| {
            panic!(
                "occurrence {n} of `{word}` not found ({} total)",
                locations.len()
            )
        })
        .range
        .start
}

/// Char-based start position of the first whole-word occurrence of `word` at
/// or after 0-indexed `line`. In merged documents this anchors searches to a
/// component's region (its [`ModelFile::start_line`]).
fn occurrence_after_line(text: &str, word: &str, line: usize) -> Position {
    find_whole_word_locations(text, word, &probe_uri(), None, WordBoundary::MathIdentifier)
        .into_iter()
        .map(|location| location.range.start)
        .find(|position| position.line as usize >= line)
        .unwrap_or_else(|| panic!("`{word}` not found at or after line {line}"))
}

/// 0-indexed line of the first occurrence of `needle`.
fn line_of(text: &str, needle: &str) -> usize {
    let offset = text
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` not found"));
    text[..offset].matches('\n').count()
}

/// Position of the first whole-word occurrence of `name` after the first
/// whole-word occurrence of the `clause` keyword (SEES/REFINES/EXTENDS) at or
/// after `start_line` (the component's region in a merged document; 0 for a
/// single-component file). Layout-independent (works for one-target-per-line
/// and inline forms), but callers must pass machines/contexts that actually
/// have the machine-level clause: the first keyword occurrence is otherwise
/// an event-level one.
fn pos_in_clause(text: &str, clause: &str, name: &str, start_line: usize) -> Position {
    let clause_pos = occurrence_after_line(text, clause, start_line);
    find_whole_word_locations(text, name, &probe_uri(), None, WordBoundary::MathIdentifier)
        .into_iter()
        .map(|location| location.range.start)
        .find(|position| *position > clause_pos)
        .unwrap_or_else(|| panic!("`{name}` not found after the {clause} keyword"))
}

/// Apply char-position `TextEdit`s bottom-up so earlier offsets stay valid.
/// Asserts the edits are non-overlapping — overlapping or duplicate edits
/// from a provider are a bug, not something to apply silently.
fn apply_edits(text: &str, edits: &[TextEdit]) -> String {
    let mut edits: Vec<&TextEdit> = edits.iter().collect();
    edits.sort_by_key(|e| e.range.start);
    for pair in edits.windows(2) {
        assert!(
            pair[0].range.end <= pair[1].range.start,
            "overlapping edits: {:?} and {:?}",
            pair[0],
            pair[1]
        );
    }
    let mut result = text.to_string();
    for edit in edits.iter().rev() {
        let start = position_to_offset(&result, edit.range.start)
            .expect("edit start out of bounds (whole-document sentinel edits are unsupported)");
        let end = position_to_offset(&result, edit.range.end)
            .expect("edit end out of bounds (whole-document sentinel edits are unsupported)");
        result.replace_range(start..end, &edit.new_text);
    }
    result
}

/// Raw AST-level component references (SEES/REFINES/EXTENDS targets).
/// Deliberately read straight off the AST fields rather than through
/// rossi::deps — in the scan test the dependency graph is the thing under
/// test, so it cannot also supply the expectations.
fn component_edges(component: &rossi::Component) -> Vec<(ReferenceKind, &str)> {
    match component {
        rossi::Component::Machine(m) => m
            .sees
            .iter()
            .map(|s| (ReferenceKind::Sees, s.as_str()))
            .chain(m.refines.as_deref().map(|r| (ReferenceKind::Refines, r)))
            .collect(),
        rossi::Component::Context(c) => c
            .extends
            .iter()
            .map(|e| (ReferenceKind::Extends, e.as_str()))
            .collect(),
    }
}

/// Containment with an exclusive end (LSP convention), except for the
/// degenerate empty-range-at-position case the provider returns when no
/// AST span encloses the offset.
fn range_contains_pos(range: &Range, pos: Position) -> bool {
    range.start <= pos && (pos < range.end || (range.start == range.end && pos == range.end))
}

fn range_contains(outer: &Range, inner: &Range) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

// ============================================================================
// LSP params builders and provider factories
// ============================================================================

fn goto_params(uri: Url, position: Position) -> GotoDefinitionParams {
    GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri },
            position,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    }
}

fn reference_params(uri: Url, position: Position) -> ReferenceParams {
    ReferenceParams {
        text_document_position: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri },
            position,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        context: ReferenceContext {
            include_declaration: true,
        },
    }
}

fn rename_params(uri: Url, position: Position, new_name: &str) -> RenameParams {
    RenameParams {
        text_document_position: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri },
            position,
        },
        new_name: new_name.to_string(),
        work_done_progress_params: WorkDoneProgressParams::default(),
    }
}

fn hover_params(uri: Url, position: Position) -> HoverParams {
    HoverParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri },
            position,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
    }
}

fn doclink_params(uri: Url) -> DocumentLinkParams {
    DocumentLinkParams {
        text_document: TextDocumentIdentifier { uri },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    }
}

/// Build a definition provider over the whole workspace.
///
/// `update_definitions` resolves cross-file definitions EAGERLY at update
/// time, so it must run only after `Workspace::open` has registered every
/// file in both managers; updating inside the registration loop would break
/// on models whose zips list machines before the contexts they see.
fn definition_provider(ws: &Workspace) -> DefinitionProvider {
    let mut provider = DefinitionProvider::new();
    provider.set_cross_reference_manager(Arc::clone(&ws.crm));
    provider.set_document_manager(Arc::clone(&ws.dm));
    let mut seen = HashSet::new();
    for file in &ws.files {
        // Merged workspaces share one URI across files — update it once.
        if seen.insert(&file.uri) {
            provider.update_definitions(file.uri.to_string(), &file.text);
        }
    }
    provider
}

fn reference_provider(ws: &Workspace) -> ReferenceProvider {
    let mut provider = ReferenceProvider::new();
    provider.set_cross_reference_manager(Arc::clone(&ws.crm));
    provider.set_document_manager(Arc::clone(&ws.dm));
    provider
}

fn rename_provider(ws: &Workspace) -> RenameProvider {
    let mut provider = RenameProvider::new();
    provider.set_cross_reference_manager(Arc::clone(&ws.crm));
    provider.set_document_manager(Arc::clone(&ws.dm));
    provider
}

fn hover_provider(ws: &Workspace) -> HoverProvider {
    // Every workspace file is already open in `ws.dm`, so hover reads its
    // components from the shared parse — no per-document priming needed.
    let mut provider = HoverProvider::new();
    provider.set_cross_reference_manager(Arc::clone(&ws.crm));
    provider.set_document_manager(Arc::clone(&ws.dm));
    provider
}

fn scalar_location(response: GotoDefinitionResponse) -> Location {
    match response {
        GotoDefinitionResponse::Scalar(location) => location,
        other => panic!("expected a scalar location, got {other:?}"),
    }
}

/// A cursor on `target` inside `from`'s `clause` block must resolve to the
/// component name in `target`'s file (which, in a merged workspace, may be
/// the same document).
fn assert_goto_clause(
    ws: &Workspace,
    provider: &DefinitionProvider,
    from: &str,
    clause: &str,
    target: &str,
) {
    let entry = ws.entry(from);
    let text = &entry.text;
    let position = pos_in_clause(text, clause, target, entry.start_line);
    let location = scalar_location(
        provider
            .goto_definition(&goto_params(ws.uri(from), position), text)
            .unwrap_or_else(|| panic!("{from} {clause} {target}: no definition")),
    );
    assert_eq!(
        location.uri,
        ws.uri(target),
        "{from} {clause} {target}: wrong target file"
    );
    assert_eq!(
        slice_range(ws.text(target), location.range),
        target,
        "{from} {clause} {target}: range does not cover the component name"
    );
}

// ============================================================================
// Gate: conversion round-trips for every bundled model
// ============================================================================

#[test]
fn examples_conversion_gate() {
    for &(zip_name, expected_count) in ALL_MODELS {
        let files = load_model(zip_name);
        assert_eq!(
            files.len(),
            expected_count,
            "{zip_name}: unexpected component count"
        );
        for file in &files {
            let reparsed = rossi::parse(&file.text).unwrap_or_else(|e| {
                panic!(
                    "{zip_name}/{}: converted text does not re-parse: {e}",
                    file.name
                )
            });
            assert_eq!(
                reparsed.name(),
                file.name,
                "{zip_name}/{}: component name drifted through conversion",
                file.name
            );
        }
    }
}

// ============================================================================
// cars-on-bridge: exact cross-file assertions
//
// The C0←{M0,M1} SEES, C0←C2 EXTENDS, and M0←M1 REFINES edges asserted here
// are also pinned (independently, from the on-disk scan path) in
// workspace_scan_builds_cross_ref_graph — keep the two in sync when the
// example model changes.
// ============================================================================

#[test]
fn cars_goto_definition_cross_file() {
    let ws = Workspace::open(CARS);
    let provider = definition_provider(&ws);

    for (from, clause, target) in [
        ("M1", "REFINES", "M0"),
        ("M0", "SEES", "C0"),
        ("C2", "EXTENDS", "C0"),
    ] {
        assert_goto_clause(&ws, &provider, from, clause, target);
    }
}

/// `base-model.eventb` is a single document holding `context C1` and the
/// `machine M1` that sees it, written with LOWERCASE keywords (Camille style).
/// Goto-definition on the `C1` in `sees C1` must jump to the `context C1`
/// header — the case the bug report flagged.
#[test]
fn base_model_lowercase_sees_goto_definition() {
    let text = include_str!("../../rossi/examples/base-model.eventb");
    let ws = Workspace::from_merged_text("base-model", text);
    let provider = definition_provider(&ws);
    assert_goto_clause(&ws, &provider, "M1", "sees", "C1");
}

#[test]
fn cars_goto_definition_identifiers() {
    let ws = Workspace::open(CARS);
    let provider = definition_provider(&ws);
    let m0 = ws.text("M0");

    // Local: `cars_number` is declared under M0's own VARIABLES; cross-file:
    // `cars_limit` is declared in the seen context C0. Either way the
    // definition must land on the declaration (the identifier's first
    // occurrence in the declaring file).
    for (word, declared_in, usage_occurrence) in [("cars_number", "M0", 1), ("cars_limit", "C0", 0)]
    {
        let usage = nth_occurrence(m0, word, usage_occurrence);
        let location = scalar_location(
            provider
                .goto_definition(&goto_params(ws.uri("M0"), usage), m0)
                .unwrap_or_else(|| panic!("no definition for {word}")),
        );
        assert_eq!(location.uri, ws.uri(declared_in), "{word}: wrong file");
        let declaration = nth_occurrence(ws.text(declared_in), word, 0);
        assert_eq!(
            location.range.start.line, declaration.line,
            "{word}: definition must point at its declaration"
        );
        assert_eq!(slice_range(ws.text(declared_in), location.range), word);
    }
}

#[test]
fn cars_references_context_constant() {
    let ws = Workspace::open(CARS);
    let provider = reference_provider(&ws);
    let c0 = ws.text("C0");

    let declaration = nth_occurrence(c0, "cars_limit", 0);
    let locations = provider
        .find_references(&reference_params(ws.uri("C0"), declaration), c0)
        .expect("references to cars_limit");

    // No location may be reported twice.
    let mut seen = HashSet::new();
    for location in &locations {
        let key = (
            location.uri.to_string(),
            location.range.start.line,
            location.range.start.character,
            location.range.end.line,
            location.range.end.character,
        );
        assert!(
            seen.insert(key),
            "duplicate reference location {location:?}"
        );
        assert_eq!(
            slice_range(ws.text_for_uri(&location.uri), location.range),
            "cars_limit",
            "bad reference range in {}",
            location.uri
        );
    }

    // Ground truth: every whole-word occurrence in every file of the model
    // (cars_limit is visible everywhere — every machine sees a context in
    // C0's extends chain). M3 uses it once, so containment-only checks
    // would miss a provider that skips transitive visibility.
    let expected: BTreeMap<String, usize> = ws
        .files
        .iter()
        .filter_map(|f| {
            let count = find_whole_word_locations(
                &f.text,
                "cars_limit",
                &f.uri,
                None,
                WordBoundary::MathIdentifier,
            )
            .len();
            (count > 0).then(|| (f.uri.to_string(), count))
        })
        .collect();
    let mut actual: BTreeMap<String, usize> = BTreeMap::new();
    for location in &locations {
        *actual.entry(location.uri.to_string()).or_default() += 1;
    }
    assert_eq!(
        actual, expected,
        "per-file reference counts must match the whole-word occurrences"
    );
}

#[test]
fn cars_rename_component_cross_file() {
    let ws = Workspace::open(CARS);
    let provider = rename_provider(&ws);
    let c0 = ws.text("C0");

    let position = nth_occurrence(c0, "C0", 0); // the CONTEXT C0 header
    let edit = provider
        .rename(&rename_params(ws.uri("C0"), position, "C0_v2"), c0)
        .expect("rename C0");
    let changes = edit.changes.expect("rename returns changes");

    let mut touched: Vec<Url> = changes.keys().cloned().collect();
    touched.sort();
    let mut expected: Vec<Url> = ["C0", "C2", "M0", "M1"].iter().map(|n| ws.uri(n)).collect();
    expected.sort();
    assert_eq!(
        touched, expected,
        "rename must touch the context and everything referencing it"
    );

    for (uri, edits) in &changes {
        let text = ws.text_for_uri(uri);
        let renamed = apply_edits(text, edits);
        rossi::parse(&renamed)
            .unwrap_or_else(|e| panic!("{uri}: text no longer parses after rename: {e}"));
        assert!(
            find_whole_word_locations(&renamed, "C0", uri, None, WordBoundary::MathIdentifier)
                .is_empty(),
            "{uri}: whole-word C0 left behind after rename"
        );
        assert!(
            !find_whole_word_locations(&renamed, "C0_v2", uri, None, WordBoundary::MathIdentifier)
                .is_empty(),
            "{uri}: C0_v2 missing after rename"
        );
    }
}

#[test]
fn cars_rename_constant_is_single_file() {
    // Pins current behavior: only component names rename across files;
    // constants/variables rename within the requesting document only.
    let ws = Workspace::open(CARS);
    let provider = rename_provider(&ws);
    let m0 = ws.text("M0");

    let position = nth_occurrence(m0, "cars_limit", 0);
    let edit = provider
        .rename(&rename_params(ws.uri("M0"), position, "limit"), m0)
        .expect("rename cars_limit");
    let changes = edit.changes.expect("rename returns changes");

    let touched: Vec<Url> = changes.keys().cloned().collect();
    assert_eq!(
        touched,
        vec![ws.uri("M0")],
        "constant rename is single-file today; if this fails, cross-file \
         symbol rename has been implemented — update this test"
    );
}

#[test]
fn cars_document_symbols_contain_events() {
    let ws = Workspace::open(CARS);

    let root_children = |name: &str, detail: &str| -> Vec<String> {
        let entry = ws.entry(name);
        let symbols = analysis::extract_symbols(&entry.component, &entry.text);
        assert_eq!(symbols.len(), 1, "{name}: one root symbol");
        assert_eq!(symbols[0].name, name);
        assert_eq!(symbols[0].detail.as_deref(), Some(detail));
        symbols[0]
            .children
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(|s| s.name.clone())
            .collect()
    };

    let m0 = root_children("M0", "Machine");
    for expected in [
        "cars_number",
        "inv1",
        "inv2",
        "inv3",
        "variant",
        "INITIALISATION",
        "ML_out",
        "ML_in",
    ] {
        assert!(
            m0.contains(&expected.to_string()),
            "M0 symbols missing {expected}, got {m0:?}"
        );
    }

    let m1 = root_children("M1", "Machine");
    for expected in ["IL_out", "IL_in"] {
        assert!(
            m1.contains(&expected.to_string()),
            "M1 symbols missing {expected}, got {m1:?}"
        );
    }

    let c2 = root_children("C2", "Context");
    for expected in ["colour", "red", "green"] {
        assert!(
            c2.contains(&expected.to_string()),
            "C2 symbols missing {expected}, got {c2:?}"
        );
    }
}

#[test]
fn cars_hover_cross_file_constant() {
    let ws = Workspace::open(CARS);
    let provider = hover_provider(&ws);
    let m0 = ws.text("M0");

    let usage = nth_occurrence(m0, "cars_limit", 0);
    let hover = provider
        .hover(&hover_params(ws.uri("M0"), usage), m0)
        .expect("hover on cars_limit");
    // The provider only ever constructs Markup contents; a different variant
    // is itself a regression worth failing on.
    let HoverContents::Markup(markup) = &hover.contents else {
        panic!("expected markup hover contents, got {:?}", hover.contents);
    };
    assert!(
        markup.value.contains("cars_limit"),
        "hover must mention the constant, got: {}",
        markup.value
    );
    assert!(
        markup.value.contains("C0"),
        "hover must name the declaring context (cross-file info), got: {}",
        markup.value
    );
}

// ============================================================================
// Invariants over every file of all bundled models
// ============================================================================

#[test]
fn all_models_semantic_tokens_invariants() {
    for (zip_name, file) in all_model_files() {
        let name = &file.name;
        let tokens = decode_tokens(&file.text);
        assert!(!tokens.is_empty(), "{zip_name}/{name}: no semantic tokens");

        let lines: Vec<&str> = file.text.lines().collect();
        // (line, end column) of the previous token: tokens must be sorted
        // and non-overlapping, not merely start-ascending.
        let mut previous: Option<(u32, u32)> = None;
        for &(line, col, len, _) in &tokens {
            assert!(
                len > 0,
                "{zip_name}/{name}: zero-length token at {line}:{col}"
            );
            assert!(
                (line as usize) < lines.len(),
                "{zip_name}/{name}: token line {line} out of bounds"
            );
            let width = lines[line as usize].chars().count() as u32;
            assert!(
                col + len <= width,
                "{zip_name}/{name}: token {line}:{col}+{len} exceeds line width {width} \
                 (columns must be chars, not bytes)"
            );
            if let Some((prev_line, prev_end)) = previous {
                assert!(
                    line > prev_line || col >= prev_end,
                    "{zip_name}/{name}: token at {line}:{col} overlaps the previous one \
                     ending at {prev_line}:{prev_end}"
                );
            }
            previous = Some((line, col + len));
        }
    }
}

#[test]
fn all_models_folding_invariants() {
    let provider = FoldingRangeProvider::new();
    for (zip_name, file) in all_model_files() {
        let name = &file.name;
        let ranges = provider
            .folding_ranges(&file.text)
            .unwrap_or_else(|| panic!("{zip_name}/{name}: no folding ranges"));
        let line_count = file.text.lines().count() as u32;

        for range in &ranges {
            assert!(
                range.start_line <= range.end_line && range.end_line < line_count,
                "{zip_name}/{name}: bad folding range {}..{} ({line_count} lines)",
                range.start_line,
                range.end_line
            );
        }
        assert!(
            ranges.iter().any(|r| r.start_line == 0),
            "{zip_name}/{name}: no component-spanning folding range"
        );
        let event_count = file
            .text
            .lines()
            .filter(|l| l.trim_start().starts_with("EVENT "))
            .count();
        assert!(
            ranges.len() >= event_count,
            "{zip_name}/{name}: {} folding ranges for {event_count} EVENT blocks",
            ranges.len()
        );
    }
}

#[test]
fn all_models_selection_ranges_nest() {
    let provider = SelectionRangeProvider::new();
    for (zip_name, file) in all_model_files() {
        let name = &file.name;
        let identifiers: Vec<&str> = match &file.component {
            rossi::Component::Machine(m) => m.variables.iter().map(|v| v.name.as_str()).collect(),
            rossi::Component::Context(c) => c
                .constants
                .iter()
                .map(|c| c.name.as_str())
                .chain(c.sets.iter().map(|s| s.name()))
                .collect(),
        };
        // Declared identifiers must always be locatable in the printed text;
        // a silent miss here would skip the file without testing anything.
        let positions: Vec<Position> = identifiers
            .iter()
            .take(8)
            .map(|id| {
                find_whole_word_locations(
                    &file.text,
                    id,
                    &probe_uri(),
                    None,
                    WordBoundary::MathIdentifier,
                )
                .first()
                .unwrap_or_else(|| {
                    panic!("{zip_name}/{name}: declared identifier `{id}` not found in text")
                })
                .range
                .start
            })
            .collect();
        if positions.is_empty() {
            continue; // component declares no identifiers (axioms-only context)
        }

        let results = provider.selection_ranges(&file.text, &positions);
        assert_eq!(
            results.len(),
            positions.len(),
            "{zip_name}/{name}: one selection range per position"
        );
        for (position, selection) in positions.iter().zip(&results) {
            assert!(
                range_contains_pos(&selection.range, *position),
                "{zip_name}/{name}: innermost range {:?} misses position {position:?}",
                selection.range
            );
            let mut current = selection;
            while let Some(parent) = &current.parent {
                assert!(
                    range_contains(&parent.range, &current.range),
                    "{zip_name}/{name}: parent {:?} does not contain child {:?}",
                    parent.range,
                    current.range
                );
                current = parent;
            }
        }
    }
}

#[test]
fn all_models_formatting_identity() {
    let provider = FormattingProvider::new();
    for (zip_name, file) in all_model_files() {
        let name = &file.name;
        let edits = provider
            .format(&file.text)
            .unwrap_or_else(|e| panic!("{zip_name}/{name}: format failed: {e}"));
        assert_eq!(
            edits.len(),
            1,
            "{zip_name}/{name}: formatting returns one full-document edit"
        );
        let formatted = &edits[0].new_text;
        if formatted != &file.text {
            let original: Vec<&str> = file.text.lines().collect();
            let new: Vec<&str> = formatted.lines().collect();
            let index = original
                .iter()
                .zip(&new)
                .position(|(a, b)| a != b)
                .unwrap_or(original.len().min(new.len()));
            panic!(
                "{zip_name}/{name}: formatting printer output is not the identity; \
                 first divergence at line {index}: {:?} vs {:?} \
                 ({} vs {} lines total)",
                original.get(index),
                new.get(index),
                original.len(),
                new.len()
            );
        }
    }
}

#[test]
fn all_models_document_links() {
    for &(zip_name, _) in ALL_MODELS {
        let ws = Workspace::open(zip_name);
        let mut provider = DocumentLinkProvider::new();
        provider.set_cross_reference_manager(Arc::clone(&ws.crm));

        for file in &ws.files {
            let name = &file.name;
            let mut expected: Vec<String> = component_edges(&file.component)
                .into_iter()
                .map(|(_, target)| target.to_string())
                .collect();
            expected.sort();

            let links = provider
                .document_links(&doclink_params(file.uri.clone()), &file.text)
                .unwrap_or_default();

            let mut linked = Vec::new();
            for link in &links {
                let target_name = slice_range(&file.text, link.range);
                let target = link
                    .target
                    .as_ref()
                    .unwrap_or_else(|| panic!("{zip_name}/{name}: link without target"))
                    .to_string();
                assert_eq!(
                    Some(target.as_str()),
                    ws.crm.find_component_uri(&target_name).as_deref(),
                    "{zip_name}/{name}: link target mismatch for `{target_name}`"
                );
                linked.push(target_name);
            }
            linked.sort();
            // Exact match: every clause reference is linked exactly once, and
            // nothing else is (no duplicate or stray links).
            assert_eq!(
                linked, expected,
                "{zip_name}/{name}: document links must cover the clause targets exactly"
            );
        }
    }
}

// ============================================================================
// Merged pass: every model as ONE multi-component document (`import --merge`)
// ============================================================================

#[test]
fn all_models_merged_invariants() {
    for &(zip_name, expected_count) in ALL_MODELS {
        let ws = Workspace::open_merged(zip_name);
        assert_eq!(
            ws.files.len(),
            expected_count,
            "{zip_name}: component count after merge"
        );
        let text = &ws.files[0].text;
        let uri = &ws.files[0].uri;

        // The diagnostics path agrees with the strict parse: no errors.
        let recovered = rossi::parse_components_with_recovery(text);
        assert!(
            recovered.is_ok(),
            "{zip_name}: merged text must produce no diagnostics: {:?}",
            recovered.errors
        );

        // Every component name resolves to the one merged URI.
        for file in &ws.files {
            assert_eq!(
                ws.crm.find_component_uri(&file.name).as_deref(),
                Some(uri.as_str()),
                "{zip_name}: {} not indexed under the merged URI",
                file.name
            );
        }

        // Document symbols: one root per component, covering disjoint,
        // strictly increasing regions of the document.
        let roots: Vec<DocumentSymbol> = ws
            .files
            .iter()
            .flat_map(|f| analysis::extract_symbols(&f.component, text))
            .collect();
        assert_eq!(
            roots.len(),
            expected_count,
            "{zip_name}: one symbol root per component"
        );
        for pair in roots.windows(2) {
            assert!(
                pair[0].range.end <= pair[1].range.start,
                "{zip_name}: overlapping component symbols {} / {}",
                pair[0].name,
                pair[1].name
            );
        }

        // Workspace symbols are indexed from the merged document.
        let provider = WorkspaceSymbolProvider::new();
        provider.update_symbols(uri.to_string(), text);
        assert!(
            !provider.search("").is_empty(),
            "{zip_name}: workspace symbol index is empty"
        );

        // Semantic tokens reach past the last component's header —
        // highlighting must not stop after the first component.
        let last_start = ws.files.last().unwrap().start_line as u32;
        assert!(
            decode_tokens(text)
                .iter()
                .any(|&(line, ..)| line > last_start),
            "{zip_name}: no semantic tokens past the last component header"
        );

        // Folding offers a range starting at every component header.
        let folding = FoldingRangeProvider::new()
            .folding_ranges(text)
            .unwrap_or_else(|| panic!("{zip_name}: no folding ranges for merged text"));
        for file in &ws.files {
            assert!(
                folding
                    .iter()
                    .any(|r| r.start_line as usize == file.start_line),
                "{zip_name}: no folding range starting at component {} (line {})",
                file.name,
                file.start_line
            );
        }
    }
}

// ----------------------------------------------------------------------------
// traffic-light merged: exact assertions on the checked-in fixture
// ----------------------------------------------------------------------------

fn merged_traffic_light() -> Workspace {
    Workspace::from_merged_text("traffic-light", MERGED_TRAFFIC_LIGHT)
}

#[test]
fn merged_components_and_edges_are_indexed() {
    let ws = merged_traffic_light();
    let names: Vec<&str> = ws.files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["M0", "C1", "M1", "M2"]);

    let referencing = |target: &str, kind: ReferenceKind| -> Vec<String> {
        let mut names: Vec<String> = ws
            .crm
            .find_referencing_components(target, Some(kind))
            .into_iter()
            .map(|info| info.name)
            .collect();
        names.sort();
        names
    };
    assert_eq!(referencing("C1", ReferenceKind::Sees), ["M1", "M2"]);
    assert_eq!(referencing("M0", ReferenceKind::Refines), ["M1"]);
    assert_eq!(referencing("M1", ReferenceKind::Refines), ["M2"]);
}

#[test]
fn merged_goto_clause_targets_within_one_file() {
    let ws = merged_traffic_light();
    let provider = definition_provider(&ws);

    // SEES/REFINES targets live in the same document; navigation must land
    // on each target component's header inside it.
    for (from, clause, target) in [
        ("M1", "REFINES", "M0"),
        ("M1", "SEES", "C1"),
        ("M2", "REFINES", "M1"),
        ("M2", "SEES", "C1"),
    ] {
        assert_goto_clause(&ws, &provider, from, clause, target);
    }
}

#[test]
fn merged_goto_definition_scopes_to_component_under_cursor() {
    // `peds_colour` is declared in both M1 and M2 (same name, same file).
    let ws = merged_traffic_light();
    let provider = definition_provider(&ws);
    let text = &ws.files[0].text;
    let uri = ws.uri("M1");

    let m1_decl = occurrence_after_line(text, "peds_colour", ws.entry("M1").start_line);
    let m2_decl = occurrence_after_line(text, "peds_colour", ws.entry("M2").start_line);

    // A use inside M1 (the @inv4 invariant) resolves to M1's declaration...
    let m1_use = occurrence_after_line(text, "peds_colour", m1_decl.line as usize + 1);
    assert!(
        (m1_use.line as usize) < ws.entry("M2").start_line,
        "use site escaped M1"
    );
    let location = scalar_location(
        provider
            .goto_definition(&goto_params(uri.clone(), m1_use), text)
            .expect("no definition for peds_colour in M1"),
    );
    assert_eq!(
        location.range.start, m1_decl,
        "M1 use must hit M1's declaration"
    );

    // ...and a use inside M2 (push_button's guard) resolves to M2's.
    let m2_use = occurrence_after_line(text, "peds_colour", m2_decl.line as usize + 1);
    let location = scalar_location(
        provider
            .goto_definition(&goto_params(uri, m2_use), text)
            .expect("no definition for peds_colour in M2"),
    );
    assert_eq!(
        location.range.start, m2_decl,
        "M2 use must hit M2's declaration"
    );
}

#[test]
fn merged_goto_definition_constant_from_seen_context_in_same_file() {
    let ws = merged_traffic_light();
    let provider = definition_provider(&ws);
    let text = &ws.files[0].text;

    // `green` used in M1's invariants resolves to the CONSTANTS declaration
    // in C1 — a cross-component lookup that never leaves the file.
    let use_site = occurrence_after_line(text, "green", ws.entry("M1").start_line);
    let location = scalar_location(
        provider
            .goto_definition(&goto_params(ws.uri("M1"), use_site), text)
            .expect("green must resolve to C1's declaration"),
    );

    assert_eq!(location.uri, ws.uri("C1"));
    let line = location.range.start.line as usize;
    assert!(
        line > ws.entry("C1").start_line && line < ws.entry("M1").start_line,
        "declaration must sit inside C1's region, got line {line}"
    );
}

#[test]
fn separate_file_refines_machine_inside_merged_file() {
    // A refinement of M2 living in its own single-component file must
    // navigate INTO the merged document.
    let mut ws = merged_traffic_light();
    ws.add_document(
        "traffic-light",
        "MACHINE M3\nREFINES\n    M2\nSEES\n    C1\nVARIABLES\n    peds_colour\n    cars_colours\n    button\nEND\n",
    );
    let provider = definition_provider(&ws);

    assert_goto_clause(&ws, &provider, "M3", "REFINES", "M2");
    assert_goto_clause(&ws, &provider, "M3", "SEES", "C1");
}

#[test]
fn merged_hover_resolves_identifiers_in_later_components() {
    let ws = merged_traffic_light();
    let provider = hover_provider(&ws);
    let text = &ws.files[0].text;

    let hover_text = |position: Position| -> String {
        let hover = provider
            .hover(&hover_params(ws.uri("M0"), position), text)
            .unwrap_or_else(|| panic!("no hover at {position:?}"));
        match hover.contents {
            HoverContents::Markup(markup) => markup.value,
            other => panic!("unexpected hover contents: {other:?}"),
        }
    };

    // `button` is declared in M2, the fourth component.
    let button_use = occurrence_after_line(text, "button", ws.entry("M2").start_line + 1);
    let markup = hover_text(button_use);
    assert!(
        markup.contains("Variable") && markup.contains("M2"),
        "hover must describe `button` as a variable of M2, got: {markup}"
    );

    // `cars_go` keeps resolving against M0 (the first component).
    let cars_go_use = occurrence_after_line(text, "cars_go", line_of(text, "INVARIANTS"));
    let markup = hover_text(cars_go_use);
    assert!(
        markup.contains("Variable") && markup.contains("M0"),
        "hover must describe `cars_go` as a variable of M0, got: {markup}"
    );
}

#[test]
fn merged_workspace_symbols_cover_every_component() {
    let ws = merged_traffic_light();
    let text = &ws.files[0].text;
    let provider = WorkspaceSymbolProvider::new();
    provider.update_symbols(ws.files[0].uri.to_string(), text);

    // One symbol per (name, container) — declared in the right component,
    // located in the right region of the document.
    for (name, container) in [
        ("cars_go", "M0"),
        ("COLOURS", "C1"),
        ("cars_colours", "M1"),
        ("button", "M2"),
        ("push_button", "M2"),
    ] {
        let results = provider.search(name);
        let hit = results
            .iter()
            .find(|s| s.name == name && s.container_name.as_deref() == Some(container))
            .unwrap_or_else(|| panic!("`{name}` not found with container {container}"));
        assert!(
            hit.location.range.start.line as usize >= ws.entry(container).start_line,
            "`{name}` must be located inside {container}'s region"
        );
    }

    // `peds_colour` is declared in both M1 and M2.
    let peds = provider.search("peds_colour");
    let containers: Vec<_> = peds
        .iter()
        .filter_map(|s| s.container_name.as_deref())
        .collect();
    assert!(
        containers.contains(&"M1") && containers.contains(&"M2"),
        "peds_colour must be indexed for both machines, got {containers:?}"
    );
}

#[test]
fn merged_semantic_tokens_broken_component_does_not_rescan_from_top() {
    // M1 broken → it comes back from recovery with its region as span; its
    // visit must start there, not at offset 0 (which would re-tokenize M0's
    // header under M1's context).
    let broken =
        MERGED_TRAFFIC_LIGHT.replace("@inv5 peds_go = ⊤ ⇔ peds_colour = green", "@inv5 +++");
    assert_ne!(
        broken, MERGED_TRAFFIC_LIGHT,
        "fixture drifted: M1 invariant not found"
    );

    let tokens = decode_tokens(&broken);
    let at_m0_header = tokens
        .iter()
        .filter(|&&(line, col, ..)| line == 0 && col == 0)
        .count();
    assert_eq!(
        at_m0_header, 1,
        "M0's MACHINE keyword must be tokenized exactly once"
    );
}

// ============================================================================
// binary-search: refinement-chain navigation, shared context with many seers
// ============================================================================

#[test]
fn binary_search_chain_navigation() {
    let ws = Workspace::open(BINARY_SEARCH);
    let provider = definition_provider(&ws);

    for (machine, refines) in [
        ("M0", None),
        ("M1", Some("M0")),
        ("M2", Some("M1")),
        ("M3", Some("M2")),
    ] {
        let rossi::Component::Machine(m) = &ws.entry(machine).component else {
            panic!("{machine} is not a machine");
        };
        assert_eq!(m.sees, ["C0"], "{machine} must see the shared context");
        assert_eq!(
            m.refines.as_deref(),
            refines,
            "{machine}: wrong refinement parent"
        );
        assert_goto_clause(&ws, &provider, machine, "SEES", "C0");
        if let Some(parent) = refines {
            assert_goto_clause(&ws, &provider, machine, "REFINES", parent);
        }
    }
}

// ============================================================================
// Workspace scan: the real `initialized()` flow over on-disk .eventb files
// ============================================================================

#[test]
fn workspace_scan_builds_cross_ref_graph() {
    let out_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("examples-lsp");

    for &(zip_name, _) in ALL_MODELS {
        let model = zip_name.trim_end_matches(".zip");
        let model_dir = out_root.join(model);
        // Start clean so stale files from earlier runs can't skew the scan.
        let _ = std::fs::remove_dir_all(&model_dir);
        std::fs::create_dir_all(&model_dir).unwrap();

        let files = load_model(zip_name);
        for file in &files {
            std::fs::write(model_dir.join(format!("{}.eventb", file.name)), &file.text).unwrap();
        }

        // Fresh manager per model: the name→URI map is flat, and models may
        // reuse component names.
        let crm = CrossReferenceManager::new();
        let scanned = crm
            .scan_workspace(&model_dir)
            .unwrap_or_else(|e| panic!("{model}: scan_workspace failed: {e}"));
        // scan_workspace counts files read, not components indexed — assert
        // both, or a file that fails to parse goes silently missing.
        assert_eq!(scanned, files.len(), "{model}: scan_workspace file count");
        assert_eq!(
            crm.all_component_names().len(),
            files.len(),
            "{model}: every scanned file must be indexed in the graph"
        );

        let referencing = |target: &str, kind: ReferenceKind| -> Vec<String> {
            let mut names: Vec<String> = crm
                .find_referencing_components(target, Some(kind))
                .into_iter()
                .map(|info| info.name)
                .collect();
            names.sort();
            names
        };

        // Full graph equality, expectations derived from the converted ASTs:
        // for every component and every edge kind, the scan-built graph must
        // report exactly the components whose AST carries that edge.
        let ast_referencing = |target: &str, kind: ReferenceKind| -> Vec<String> {
            let mut names: Vec<String> = files
                .iter()
                .filter(|f| {
                    component_edges(&f.component)
                        .iter()
                        .any(|&(k, t)| k == kind && t == target)
                })
                .map(|f| f.name.clone())
                .collect();
            names.sort();
            names
        };
        for target in &files {
            for kind in [
                ReferenceKind::Sees,
                ReferenceKind::Refines,
                ReferenceKind::Extends,
            ] {
                assert_eq!(
                    referencing(&target.name, kind),
                    ast_referencing(&target.name, kind),
                    "{model}: {kind:?} edges into {} diverge from the AST",
                    target.name
                );
            }
        }

        // Hand-pinned ground truth, independent of the AST-derived loop above
        // (a shared parser bug would fool derived expectations, not these).
        // Kept in sync with the cars-on-bridge exact tests near the top.
        match zip_name {
            CARS => {
                let c0_uri = Url::from_file_path(model_dir.join("C0.eventb"))
                    .unwrap()
                    .to_string();
                assert_eq!(crm.find_component_uri("C0"), Some(c0_uri));
                assert_eq!(referencing("C0", ReferenceKind::Sees), ["M0", "M1"]);
                assert_eq!(referencing("C0", ReferenceKind::Extends), ["C2"]);
                assert_eq!(referencing("M1", ReferenceKind::Refines), ["M2"]);
                assert_eq!(crm.ordered_extends_chain("C3"), ["C2", "C0"]);
            }
            BINARY_SEARCH => {
                assert_eq!(
                    referencing("C0", ReferenceKind::Sees),
                    ["M0", "M1", "M2", "M3"]
                );
                assert_eq!(referencing("M2", ReferenceKind::Refines), ["M3"]);
            }
            BASE_MODEL => {
                assert_eq!(referencing("C1", ReferenceKind::Sees), ["M1"]);
            }
            TRAFFIC_LIGHT => {
                assert_eq!(referencing("C1", ReferenceKind::Sees), ["M1", "M2"]);
                assert_eq!(referencing("M0", ReferenceKind::Refines), ["M1"]);
            }
            FILE_SYSTEM => {
                assert_eq!(referencing("C0", ReferenceKind::Sees), ["M0"]);
            }
            _ => unreachable!(),
        }
    }
}
