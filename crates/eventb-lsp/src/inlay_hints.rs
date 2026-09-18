//! Inlay hints: inferred declaration types rendered after each declared name.
//!
//! Event-B declarations are bare identifiers — `VARIABLES x`, `CONSTANTS c`,
//! `ANY p` — whose types are recovered by the static checker from invariants,
//! axioms, and guards, often clauses away or up the REFINES/EXTENDS chain.
//! This provider surfaces the inferred type at the declaration site itself.
//!
//! Unlike the diagnostics path, which deliberately avoids type inference on
//! every keystroke, hints are pull-based: the editor asks for the visible
//! range when it needs them, the whole-document hint list is computed once
//! over the document's dependency closure, and repeated requests at an
//! unchanged buffer state are served from a per-URI cache.

use std::collections::HashMap;
use std::sync::Arc;

use rossi::ast::NamedElement;
use rossi::formula::{FormulaFactory, Type};
use rossi::{Component, PrettyPrinter};

use crate::component_loader::ComponentLoader;
use crate::config::RossiConfig;
use crate::cross_references::CrossReferenceManager;
use crate::document::DocumentManager;
use crate::lsp_types::{
    InlayHint, InlayHintKind, InlayHintLabel, InlayHintTooltip, MarkupContent, MarkupKind, Range,
    Url,
};
use crate::position::PositionIndex;
use crate::text_utils::line_tight_end;

/// Serves `textDocument/inlayHint` from per-URI cached whole-document lists.
pub struct InlayHintsProvider {
    document_manager: Arc<DocumentManager>,
    cross_reference_manager: Arc<CrossReferenceManager>,
    cache: parking_lot::Mutex<HashMap<Url, CachedHints>>,
}

/// One cached whole-document hint computation. Valid while no buffer anywhere
/// has changed (the closure's inputs are open buffers plus disk fallbacks, so
/// any open/change/close is the moment inferred types can change) and the
/// request still runs under the same configuration snapshot.
struct CachedHints {
    change_counter: u64,
    /// Compared by `Arc::ptr_eq`: the config manager swaps the whole `Arc` on
    /// every update, so pointer identity is exactly "unchanged since".
    config: Arc<RossiConfig>,
    /// Sorted by position, un-clipped; requests slice out their range.
    hints: Vec<InlayHint>,
}

impl InlayHintsProvider {
    pub fn new(
        document_manager: Arc<DocumentManager>,
        cross_reference_manager: Arc<CrossReferenceManager>,
    ) -> Self {
        Self {
            document_manager,
            cross_reference_manager,
            cache: parking_lot::Mutex::new(HashMap::new()),
        }
    }

    /// The hints for `uri` clipped to `range`, served from the cache alone;
    /// `None` when there is no valid entry. Cheap enough for the async
    /// handler thread: a lock, two binary searches, and the response clones.
    pub fn cached_hints(
        &self,
        uri: &Url,
        range: Range,
        config: &Arc<RossiConfig>,
    ) -> Option<Vec<InlayHint>> {
        let change_counter = self.document_manager.change_counter();
        let cache = self.cache.lock();
        let entry = cache.get(uri)?;
        (entry.change_counter == change_counter && Arc::ptr_eq(&entry.config, config))
            .then(|| clip(&entry.hints, range))
    }

    /// Compute, cache, and clip the hints for `uri`; `None` when the document
    /// is not open. Runs the static check over the document's dependency
    /// closure — call from the blocking pool. Concurrent computes for one URI
    /// may run twice — clients debounce their requests, so a rare duplicate
    /// is preferred over another lock order to reason about.
    pub fn compute_hints(
        &self,
        uri: &Url,
        range: Range,
        config: &Arc<RossiConfig>,
    ) -> Option<Vec<InlayHint>> {
        // Read the stamp before computing: an edit landing mid-compute then
        // invalidates the entry we store, never the other way around.
        let change_counter = self.document_manager.change_counter();
        let hints = self.compute(uri, config)?;
        let clipped = clip(&hints, range);
        self.cache.lock().insert(
            uri.clone(),
            CachedHints {
                change_counter,
                config: Arc::clone(config),
                hints,
            },
        );
        Some(clipped)
    }

    /// The hints for `uri` clipped to `range`; `None` when the document is
    /// not open. Cache-hit-or-compute in one call, for callers indifferent to
    /// the blocking boundary (the server tries [`Self::cached_hints`] inline
    /// and moves only a miss to the blocking pool).
    pub fn inlay_hints(
        &self,
        uri: &Url,
        range: Range,
        config: &Arc<RossiConfig>,
    ) -> Option<Vec<InlayHint>> {
        self.cached_hints(uri, range, config)
            .or_else(|| self.compute_hints(uri, range, config))
    }

    /// Drop the cached hints of a closed document.
    pub fn evict(&self, uri: &Url) {
        self.cache.lock().remove(uri);
    }

    /// Compute the whole-document hint list: assemble the dependency closure
    /// from current buffer snapshots, run the static check (without proof
    /// obligations), and join the typed declaration records back to this
    /// file's AST declaration sites, which carry the spans.
    fn compute(&self, uri: &Url, config: &RossiConfig) -> Option<Vec<InlayHint>> {
        let doc = self.document_manager.parse_result(uri)?;

        // The loadable closure of every component in this file, assembled by
        // the shared builder so hints and semantic diagnostics agree on what
        // a file depends on. Only this file's declaration sites are read
        // below: a dependency's spans index its own text.
        let loader =
            ComponentLoader::new(&self.cross_reference_manager, Some(&self.document_manager));
        let project = crate::closure::project_for(&doc, &loader, "lsp-inlay-hints");
        // This file's components, for the WD pass, which must not be fed the
        // dependencies.
        let local = crate::closure::local_names(&doc);
        // Drop-but-continue: whatever failed to check is simply absent from
        // the model, and its declarations get no hints.
        let (_result, model) = rossi_build::check_with_model(&project);

        let index = PositionIndex::new(doc.text());
        // One style-resolved printer for every label and tooltip. Labels stay
        // single-line regardless of the width — they render through the
        // always-flat print_formula_* API — while the WD tooltip alone opts
        // into wrapping at the configured width.
        let printer = config.format.printer();
        let max_length = config.inlay_hints.max_length as usize;
        let mut hints = Vec::new();
        for component in doc.components() {
            match component {
                Component::Machine(machine) => {
                    let Some(checked) = model.machines.get(&machine.name) else {
                        continue;
                    };
                    push_declaration_hints(
                        &mut hints,
                        &machine.variables,
                        checked
                            .record
                            .variables
                            .iter()
                            .map(|variable| (variable.name.as_str(), &variable.ty)),
                        &index,
                        &printer,
                        max_length,
                    );

                    let events: HashMap<&str, _> = checked
                        .record
                        .events
                        .iter()
                        .map(|event| (event.label.as_str(), event))
                        .collect();
                    for event in &machine.events {
                        let Some(decl) = events.get(event.name.as_str()) else {
                            continue;
                        };
                        push_declaration_hints(
                            &mut hints,
                            &event.parameters,
                            decl.parameters
                                .iter()
                                .map(|parameter| (parameter.name.as_str(), &parameter.ty)),
                            &index,
                            &printer,
                            max_length,
                        );
                    }
                }
                Component::Context(context) => {
                    let Some(checked) = model.contexts.get(&context.name) else {
                        continue;
                    };
                    push_declaration_hints(
                        &mut hints,
                        &context.constants,
                        checked
                            .record
                            .constants
                            .iter()
                            .map(|constant| (constant.name.as_str(), &constant.ty)),
                        &index,
                        &printer,
                        max_length,
                    );
                    // Carrier sets are deliberately not hinted: a set S always
                    // types as ℙ(S), so the hint would carry no information.
                }
            }
        }

        if config.inlay_hints.well_definedness {
            push_well_definedness_hints(
                &mut hints,
                project
                    .components
                    .iter()
                    .filter(|component| local.contains(component.component.name())),
                doc.text(),
                &model,
                &index,
                &printer,
            );
        }

        hints.sort_by_key(|hint| hint.position);
        Some(hints)
    }
}

/// Append one "WD" marker per formula of `local_components` whose
/// well-definedness lemma is non-trivial, with the lemma — rendered in the
/// user's configured style — as the hint tooltip. The lemma is a conjunction
/// over the whole formula, so there is one marker per formula, at the
/// formula's end.
///
/// `local_components` must be the components whose spans index `text`: the
/// loop below resolves each condition's span against `text` without re-reading
/// the source the condition names, so a dependency would both have its lemma
/// computed for nothing and anchor its marker at an offset into the wrong file.
fn push_well_definedness_hints<'a>(
    hints: &mut Vec<InlayHint>,
    local_components: impl IntoIterator<Item = &'a rossi_build::ProjectComponent>,
    text: &str,
    model: &rossi_build::sc_model::ScModel,
    index: &PositionIndex,
    printer: &PrettyPrinter,
) {
    let conditions = rossi_build::wd::conditions(local_components, model);
    if conditions.is_empty() {
        return;
    }
    // Formula spans swallow the trailing trivia — whitespace and comments —
    // up to the next token (the grammar's implicit rules); anchor each marker
    // at its formula's last visible character instead.
    let masked = rossi::comments::mask_comments(text);
    for condition in conditions {
        let Some(span) = condition.span.map(|located| located.value) else {
            continue;
        };
        hints.push(InlayHint {
            position: index.position(line_tight_end(&masked, span)),
            label: InlayHintLabel::String("WD".to_string()),
            // Neither of the two protocol kinds (Type, Parameter) applies;
            // omitted, the client falls back to a reasonable default.
            kind: None,
            text_edits: None,
            tooltip: Some(InlayHintTooltip::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!(
                    "Well-definedness condition:\n```\n{}\n```",
                    printer.print_formula_predicate_wrapped(&condition.lemma)
                ),
            })),
            padding_left: Some(true),
            padding_right: None,
            data: None,
        });
    }
}

/// Append one `: τ` hint per declared name that the checker typed and the
/// textual parser located. Names without a span (Rodin-XML imports) or
/// without an inferred type (no typing predicate found) are skipped.
fn push_declaration_hints<'a>(
    hints: &mut Vec<InlayHint>,
    declarations: &[NamedElement],
    typed: impl Iterator<Item = (&'a str, &'a Type)>,
    index: &PositionIndex,
    printer: &PrettyPrinter,
    max_length: usize,
) {
    if declarations.is_empty() {
        return;
    }
    let types: HashMap<&str, &Type> = typed.collect();
    for declaration in declarations {
        let Some(span) = &declaration.span else {
            continue;
        };
        let Some(ty) = types.get(declaration.name.as_str()) else {
            continue;
        };
        let rendered = render_type(ty, printer);
        let (label, tooltip) = if max_length > 0 && rendered.chars().count() > max_length {
            let truncated: String = rendered.chars().take(max_length - 1).collect();
            (
                format!(": {truncated}…"),
                Some(InlayHintTooltip::String(rendered)),
            )
        } else {
            (format!(": {rendered}"), None)
        };
        hints.push(InlayHint {
            position: index.position(span.end),
            label: InlayHintLabel::String(label),
            kind: Some(InlayHintKind::TYPE),
            text_edits: None,
            tooltip,
            padding_left: None,
            padding_right: None,
            data: None,
        });
    }
}

/// Render a type in the user's configured formatting style. The canonical
/// Rodin spelling is Unicode; the ASCII spelling goes through the shared
/// style-resolved printer so operator spellings match formatted output.
fn render_type(ty: &Type, printer: &PrettyPrinter) -> String {
    if printer.use_unicode {
        return ty.to_rodin_canonical();
    }
    printer.print_formula_expression(&ty.to_expression(&FormulaFactory::default_factory()))
}

/// The hints intersecting `range` — a contiguous run, since the cached list
/// is sorted by position.
fn clip(hints: &[InlayHint], range: Range) -> Vec<InlayHint> {
    let start = hints.partition_point(|hint| hint.position < range.start);
    let end = hints.partition_point(|hint| hint.position <= range.end);
    hints[start..end].to_vec()
}
