//! `textDocument/documentHighlight`: every occurrence of the symbol under the
//! cursor, within the cursor's own document.
//!
//! This is find-references narrowed to one file, so it resolves through
//! [`ReferenceProvider::find_references`] rather than keeping a second notion
//! of what an occurrence is — a highlight that disagreed with the reference
//! list would be a bug users could see side by side. The one thing the
//! reference list does not carry is the read/write distinction, so the roles
//! are recovered here from the same occurrence walker
//! ([`formula_walk::collect_in_component`]) that produced the spans.

use std::collections::HashSet;

use rossi::ast::Span;

use crate::document::ParsedDocument;
use crate::formula_walk::{self, IdentRole};
use crate::identifier_utils;
use crate::lsp_types::*;
use crate::position::PositionIndex;
use crate::references::ReferenceProvider;

/// Highlights for the symbol at `params`' position, or `None` when the cursor
/// is not on a resolvable identifier.
pub fn document_highlights(
    references: &ReferenceProvider,
    params: &DocumentHighlightParams,
    doc: &ParsedDocument,
) -> Option<Vec<DocumentHighlight>> {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;

    // Masked so a cursor inside a comment resolves to no identifier, matching
    // how `find_references` reads the same position.
    let masked = rossi::comments::mask_comments_chars(doc.text());
    let (name, _) = identifier_utils::identifier_at_position(&masked, position)?;

    // A cursor on a bare keyword must not light up every `END` in the file.
    // `find_references` falls back to a whole-word text scan when the cursor
    // does not resolve, which is a reasonable last resort for an explicit
    // request but not for a feature that fires on every cursor move. A name a
    // component actually declares stays eligible, because Event-B does let a
    // declaration spell a keyword.
    if rossi::keywords::is_keyword(&name) && !is_declared(doc, &name) {
        return None;
    }

    // `include_declaration` is true: an editor highlights the declaration
    // along with the uses, unlike a "find references" list where the user may
    // want only the uses.
    let reference_params = ReferenceParams {
        text_document_position: params.text_document_position_params.clone(),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        context: ReferenceContext {
            include_declaration: true,
        },
    };
    let locations = references.find_references(&reference_params, doc.text())?;

    let writes = write_starts(doc, &name);
    let highlights: Vec<DocumentHighlight> = locations
        .into_iter()
        // Cross-file references are real but belong to `references`; a
        // highlight range is only meaningful in the document it was requested
        // for, and a client would mis-paint one that named another file.
        .filter(|location| &location.uri == uri)
        .map(|location| DocumentHighlight {
            kind: Some(
                if writes.contains(&(location.range.start.line, location.range.start.character)) {
                    DocumentHighlightKind::WRITE
                } else {
                    DocumentHighlightKind::READ
                },
            ),
            range: location.range,
        })
        .collect();

    (!highlights.is_empty()).then_some(highlights)
}

/// Whether any component in `doc` declares `name`: as the component itself, as
/// a clause entry (variable / constant / set), as an event, or as an event
/// parameter. Only the open document is consulted, which is all the keyword
/// guard above needs.
fn is_declared(doc: &ParsedDocument, name: &str) -> bool {
    doc.components().iter().any(|component| {
        component.name() == name
            || crate::symbols::enumerate_symbols(component)
                .iter()
                .any(|symbol| symbol.name == name)
    })
}

/// Start positions, in `doc`, of the occurrences of `name` that write it:
/// clause declarations, assignment targets and binder declarations. Keyed by
/// start rather than by full range because a primed write target (`x'`) spans
/// one character more than the unprimed declaration it refers to, while both
/// start at the same place.
///
/// An empty set is the safe answer: every occurrence is then reported as a
/// read, which is what a client that ignores `kind` renders anyway.
fn write_starts(doc: &ParsedDocument, name: &str) -> HashSet<(u32, u32)> {
    let index = PositionIndex::new(doc.text());
    doc.components()
        .iter()
        .flat_map(|component| {
            // The clause declaration (a VARIABLES / CONSTANTS / SETS entry) is
            // not a formula occurrence, so the walker never reports it; it is
            // a write all the same.
            formula_walk::declaration_span(component, name)
                .into_iter()
                .chain(
                    formula_walk::collect_in_component(component, name)
                        .into_iter()
                        .filter(|hit| {
                            matches!(hit.role, IdentRole::WriteTarget | IdentRole::Binder)
                        })
                        .map(|hit| hit.span),
                )
        })
        .map(|span: Span| {
            let start = index.position(span.start);
            (start.line, start.character)
        })
        .collect()
}
