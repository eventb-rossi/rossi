//! Go-to-definition provider for Event-B.
//!
//! Resolves the identifier under the cursor to what it names — scope-aware,
//! through the shared resolver in [`crate::symbols`] that find-references also
//! uses — and jumps to its declaration site:
//! - a formula binder the cursor sits on or is bound by (a quantifier `∀`/`∃`,
//!   `λ`, set comprehension, quantified `⋃`/`⋂`) resolves to *its own* binder,
//!   never a same-named global it shadows;
//! - an event `ANY` parameter resolves to *its own* event's declaration, never a
//!   same-named parameter of a sibling event;
//! - a cursor on an event's `refines`/`extends` target resolves to the abstract
//!   event it names — found up the refinement chain, never the local same-named
//!   event, even when the refined event keeps its name (`event ML_in extends ML_in`);
//! - a variable / constant / set / event resolves to the component that declares
//!   it, found by walking the refinement / sees / extends chains, with a local
//!   declaration shadowing a same-named inherited one;
//! - a cursor inside a machine-level SEES / REFINES / EXTENDS clause instead
//!   navigates to the referenced component's own name.
//!
//! There is no eager per-document index: resolution runs on demand against the
//! document's stored parse — the same parse find-references and hover read — and
//! shares one resolver with find-references, so the two cannot drift on what a
//! name resolves to.

use crate::lsp_types::{GotoDefinitionParams, GotoDefinitionResponse, Location, Position};
use std::sync::Arc;

use crate::component_loader::ComponentLoader;
use crate::cross_references::CrossReferenceManager;
use crate::document::DocumentManager;
use crate::identifier_utils::identifier_at_position;
use crate::position::span_to_range;
use crate::references::component_reference_clause;
use crate::symbols::{Resolution, declaration_location, resolve_cursor};

/// Provides go-to-definition functionality.
pub struct DefinitionProvider {
    /// Cross-reference manager for workspace-wide navigation.
    cross_ref_manager: Option<Arc<CrossReferenceManager>>,
    /// Document manager for reading open documents' stored parses.
    document_manager: Option<Arc<DocumentManager>>,
}

impl DefinitionProvider {
    pub fn new() -> Self {
        Self {
            cross_ref_manager: None,
            document_manager: None,
        }
    }

    /// Set the cross-reference manager for workspace-wide navigation.
    pub fn set_cross_reference_manager(&mut self, manager: Arc<CrossReferenceManager>) {
        self.cross_ref_manager = Some(manager);
    }

    /// Set the document manager for reading open documents.
    pub fn set_document_manager(&mut self, manager: Arc<DocumentManager>) {
        self.document_manager = Some(manager);
    }

    /// Handle a go-to-definition request.
    pub fn goto_definition(
        &self,
        params: &GotoDefinitionParams,
        text: &str,
    ) -> Option<GotoDefinitionResponse> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        // Resolve against the document's stored parse when it is open: its
        // components and text index one snapshot, so offsets and spans agree.
        // The handler `text` is the fallback for a document that is not open —
        // the resolver then recovers components from it.
        let cursor = self
            .document_manager
            .as_ref()
            .and_then(|dm| dm.parse_result(uri));
        let text = cursor.as_deref().map_or(text, |parsed| parsed.text());

        // Structural scans run on comment-masked text (char columns preserved):
        // a cursor inside a comment finds no identifier and resolves to nothing.
        let masked = rossi::comments::mask_comments_chars(text);
        let (word, _) = identifier_at_position(&masked, position)?;

        // The workspace graph is needed to load the declaring component; without
        // it there is no navigation target. The server always wires it up.
        let manager = self.cross_ref_manager.as_ref()?;
        let loader = ComponentLoader::new(manager, self.document_manager.as_deref());

        // Stage 1: a cursor in a SEES / REFINES / EXTENDS clause navigates to the
        // referenced component's own name.
        if let Some(location) = find_cross_file_reference(&masked, position, &word, &loader) {
            return Some(GotoDefinitionResponse::Scalar(location));
        }

        // Stage 2: resolve the identifier to what it names (scope-aware) and jump
        // to its declaration site — a formula binder to its own binder (always in
        // this document), a symbol to its declaring component.
        let location =
            match resolve_cursor(text, &masked, position, &word, &loader, cursor.as_deref())? {
                Resolution::Bound(bound) => Location {
                    uri: uri.clone(),
                    range: span_to_range(&bound.declaration?, text),
                },
                Resolution::Symbol(symbol) => declaration_location(&symbol, &loader)?,
            };
        Some(GotoDefinitionResponse::Scalar(location))
    }
}

impl Default for DefinitionProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve a cursor sitting in a SEES / REFINES / EXTENDS clause to the
/// referenced component's own name. `masked` is the comment-masked document, so
/// a keyword spelled inside a comment cannot be mistaken for a clause boundary.
/// The component's name span is read from the parser (the source of truth), so
/// the target is exact for any casing or spacing.
fn find_cross_file_reference(
    masked: &str,
    position: Position,
    word: &str,
    loader: &ComponentLoader,
) -> Option<Location> {
    // Reuses the references provider's detector: case-insensitive and
    // in-event-aware, so a `sees` spelled any way resolves and one mentioned
    // inside an event never misfires.
    component_reference_clause(masked, position)?;

    let loaded = loader.load(word)?;
    let name_span = loaded.component().name_span()?;
    let range = span_to_range(&name_span, loaded.text());
    Some(Location::new(loaded.uri().clone(), range))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp_types::{Range, TextDocumentIdentifier, TextDocumentPositionParams, Url};

    /// Register every component in both managers and open each in the document
    /// manager, returning a provider wired to them. On-demand resolution reads
    /// the stored parses, so every component the lookup may touch — the cursor's
    /// and any declaring one up a chain — must be registered.
    fn setup(components: &[(&str, &str)]) -> DefinitionProvider {
        let crm = Arc::new(CrossReferenceManager::new());
        let dm = Arc::new(DocumentManager::new());

        for (uri, source) in components {
            crm.update_component(uri.to_string(), source);
            let url = Url::parse(uri).unwrap();
            dm.open(url, 1, source.to_string());
        }

        let mut provider = DefinitionProvider::new();
        provider.set_cross_reference_manager(crm);
        provider.set_document_manager(dm);
        provider
    }

    fn goto_params(uri: &str, line: u32, character: u32) -> GotoDefinitionParams {
        GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: Url::parse(uri).unwrap(),
                },
                position: Position::new(line, character),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        }
    }

    /// Resolve at `(line, ch)` in `uri` and assert it lands on `want_uri`'s
    /// `want` range — the single target location, panicking if nothing resolves.
    fn assert_goto(
        provider: &DefinitionProvider,
        uri: &str,
        source: &str,
        line: u32,
        ch: u32,
        want_uri: &str,
        want: Range,
    ) {
        let response = provider
            .goto_definition(&goto_params(uri, line, ch), source)
            .expect("definition should resolve");
        let location = match response {
            GotoDefinitionResponse::Scalar(location) => location,
            other => panic!("expected a scalar location, got {other:?}"),
        };
        assert_eq!(location.uri.as_str(), want_uri);
        assert_eq!(location.range, want);
    }

    fn goto_none(provider: &DefinitionProvider, uri: &str, source: &str, line: u32, ch: u32) {
        assert!(
            provider
                .goto_definition(&goto_params(uri, line, ch), source)
                .is_none(),
            "expected no definition at {line}:{ch}"
        );
    }

    /// `(line, start_char)..(line, end_char)` on one line.
    fn range(line: u32, start: u32, end: u32) -> Range {
        Range {
            start: Position::new(line, start),
            end: Position::new(line, end),
        }
    }

    #[test]
    fn local_variable_use_resolves_to_its_declaration() {
        let uri = "file:///m.eventb";
        let source = "MACHINE m\nVARIABLES\n    count\nINVARIANTS\n    @inv1 count ∈ ℕ\nEND";
        let provider = setup(&[(uri, source)]);

        // `count` used in @inv1 (line 4) → its declaration (line 2).
        assert_goto(&provider, uri, source, 4, 12, uri, range(2, 4, 9));
    }

    // Issue #100 — a formula binder shadowing a same-named machine variable.
    //   2      x              <- the variable declaration (cols 4..5)
    //   4  @inv1 x ∈ ℕ        <- a free use of the variable (col 10)
    //   5  @inv2 ∀ x · x > 0  <- `∀ x` binder (col 12), bound use (col 16)
    const SHADOWING_BINDER: &str =
        "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ∈ ℕ\n    @inv2 ∀ x · x > 0\nEND";

    #[test]
    fn bound_variable_use_resolves_to_its_binder_not_the_global() {
        // The reported bug: the `x` in `@inv2 ∀ x · x > 0` is bound by the
        // quantifier, so go-to-definition lands on the `∀ x` binder (line 5),
        // never the variable declaration on line 2.
        let uri = "file:///m.eventb";
        let provider = setup(&[(uri, SHADOWING_BINDER)]);

        assert_goto(
            &provider,
            uri,
            SHADOWING_BINDER,
            5,
            16,
            uri,
            range(5, 12, 13),
        );
    }

    #[test]
    fn binder_declaration_resolves_to_itself_not_the_global() {
        // A cursor on the binder `x` itself resolves to the binder, not the
        // same-named global variable it shadows.
        let uri = "file:///m.eventb";
        let provider = setup(&[(uri, SHADOWING_BINDER)]);

        assert_goto(
            &provider,
            uri,
            SHADOWING_BINDER,
            5,
            12,
            uri,
            range(5, 12, 13),
        );
    }

    #[test]
    fn free_use_beside_a_shadowing_binder_still_resolves_to_the_variable() {
        // The free `x` in @inv1 is *not* bound by the @inv2 quantifier, so it
        // still resolves to the variable declaration — the binder-awareness is
        // one-directional and does not capture free uses.
        let uri = "file:///m.eventb";
        let provider = setup(&[(uri, SHADOWING_BINDER)]);

        assert_goto(&provider, uri, SHADOWING_BINDER, 4, 10, uri, range(2, 4, 5));
    }

    // A machine with two events that both declare a parameter named `subject`.
    // Line map (0-indexed):
    //   4  EVENT create_user
    //   7          subject         <- create_user's declaration (cols 8..15)
    //   9          @grd1 subject…   <- use in create_user's guard (col 14)
    //  13  EVENT create_object
    //  15          subject         <- create_object's declaration (cols 8..15)
    //  17          @grd1 subject…   <- use in create_object's guard (col 14)
    const TWO_EVENTS: &str = "\
MACHINE m
VARIABLES
    v
EVENTS
    EVENT create_user
    ANY
        user
        subject
    WHERE
        @grd1 subject ∈ S
    THEN
        v ≔ 0
    END
    EVENT create_object
    ANY
        subject
    WHERE
        @grd1 subject ∈ S
    THEN
        v ≔ 0
    END
END";

    #[test]
    fn event_parameter_resolves_within_its_own_event() {
        // The reported bug (#1): clicking `subject` declared in `create_object`
        // must stay in `create_object` (line 15), not jump to `create_user`'s
        // same-named parameter on line 7.
        let uri = "file:///m.eventb";
        let provider = setup(&[(uri, TWO_EVENTS)]);

        assert_goto(&provider, uri, TWO_EVENTS, 15, 10, uri, range(15, 8, 15));
    }

    #[test]
    fn parameter_used_in_a_guard_resolves_to_its_own_event() {
        // The reported bug (#2): `subject` used in `create_object`'s guard
        // (line 17) resolves to `create_object`'s declaration (line 15), not
        // `create_user`'s (line 7).
        let uri = "file:///m.eventb";
        let provider = setup(&[(uri, TWO_EVENTS)]);

        assert_goto(&provider, uri, TWO_EVENTS, 17, 16, uri, range(15, 8, 15));
    }

    #[test]
    fn sibling_events_same_named_parameters_stay_independent() {
        // The mirror of the bug: `subject` in `create_user`'s guard (line 9)
        // resolves to `create_user`'s own declaration (line 7), confirming the
        // two parameters are independent and scoping is symmetric.
        let uri = "file:///m.eventb";
        let provider = setup(&[(uri, TWO_EVENTS)]);

        assert_goto(&provider, uri, TWO_EVENTS, 9, 16, uri, range(7, 8, 15));
    }

    #[test]
    fn guard_only_name_is_not_a_definition() {
        // `q` is an ANY parameter; `k` only appears in the guard text and is not
        // declared anywhere, so it resolves to nothing.
        let uri = "file:///m.eventb";
        let source = "MACHINE m\nVARIABLES\n    v\nEVENTS\n  EVENT e\n  ANY\n    q\n  WHERE\n    @grd1 q > k\n  THEN\n    v ≔ 0\n  END\nEND";
        let provider = setup(&[(uri, source)]);

        // `q` (parameter) used in the guard resolves to its declaration (line 6).
        assert_goto(&provider, uri, source, 8, 10, uri, range(6, 4, 5));

        // `k` is guard-only, not a definition.
        goto_none(&provider, uri, source, 8, 14);
    }

    #[test]
    fn cursor_in_a_comment_resolves_to_nothing() {
        // `count` mentioned in the header comment is prose; the cursor on it
        // resolves to nothing, while the real declaration still resolves.
        let uri = "file:///m.eventb";
        let source = "MACHINE test // count of VARIABLES\nVARIABLES\n    count\nEND";
        let provider = setup(&[(uri, source)]);

        goto_none(&provider, uri, source, 0, 17);

        assert_goto(&provider, uri, source, 2, 6, uri, range(2, 4, 9));
    }

    #[test]
    fn event_name_resolves_to_its_declaration() {
        // An event named `ent` is a substring of `EVENT`; resolution reads the
        // name span from the AST and lands on the name, never inside the keyword.
        let uri = "file:///m.eventb";
        let source = "MACHINE m\nEVENTS\n    EVENT ent\n    THEN\n        skip\n    END\nEND";
        let provider = setup(&[(uri, source)]);

        // after "    EVENT "
        assert_goto(&provider, uri, source, 2, 11, uri, range(2, 10, 13));
    }

    #[test]
    fn cross_file_constant_resolves_via_sees() {
        // `max_value` used in the machine's invariant resolves to its CONSTANTS
        // declaration in the context the machine SEES.
        let ctx_uri = "file:///ctx.eventb";
        let ctx = "CONTEXT ctx\nCONSTANTS\n    max_value\nAXIOMS\n    @axm1 max_value ∈ ℕ\nEND";
        let m_uri = "file:///counter.eventb";
        let m = "MACHINE counter\nSEES\n    ctx\nVARIABLES\n    count\nINVARIANTS\n    @inv1 count ≤ max_value\nEND";
        let provider = setup(&[(ctx_uri, ctx), (m_uri, m)]);

        assert_goto(&provider, m_uri, m, 6, 20, ctx_uri, range(2, 4, 13));
    }

    #[test]
    fn local_variable_shadows_a_seen_constant() {
        // A machine variable `x` and a seen constant `x`: the cursor on `x` in
        // the machine's invariant resolves to the local variable, not the
        // context constant.
        let ctx_uri = "file:///ctx.eventb";
        let ctx = "CONTEXT ctx\nCONSTANTS\n    x\nEND";
        let m_uri = "file:///m.eventb";
        let m = "MACHINE m\nSEES\n    ctx\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ∈ ℕ\nEND";
        let provider = setup(&[(ctx_uri, ctx), (m_uri, m)]);

        assert_goto(&provider, m_uri, m, 6, 10, m_uri, range(4, 4, 5));
    }

    #[test]
    fn refined_event_name_resolves_to_the_abstract_machine() {
        // `update` named in the concrete machine's `REFINES update` clause
        // resolves to the abstract machine's event declaration (the differing-
        // name refinement case, which must keep working).
        let abs_uri = "file:///abstract.eventb";
        let abs = "MACHINE abstract_mch\nVARIABLES\n    state\nEVENTS\n    EVENT update\n    THEN\n        state ≔ state + 1\n    END\nEND";
        let con_uri = "file:///concrete.eventb";
        let con = "MACHINE concrete_mch\nREFINES\n    abstract_mch\nVARIABLES\n    state\nEVENTS\n    EVENT update_v2\n    REFINES update\n    THEN\n        state ≔ state + 1\n    END\nEND";
        let provider = setup(&[(abs_uri, abs), (con_uri, con)]);

        // `update` after "    EVENT "
        assert_goto(&provider, con_uri, con, 7, 14, abs_uri, range(4, 10, 16));
    }

    // The abstract machine declaring `EVENT ML_in`, shared by the two same-name
    // refinement tests below (its event name span is `range(4, 10, 15)`).
    const ML_IN_ABS_URI: &str = "file:///abstract.eventb";
    const ML_IN_ABS: &str = "MACHINE abstract_mch\nVARIABLES\n    state\nEVENTS\n    EVENT ML_in\n    THEN\n        state ≔ state\n    END\nEND";

    #[test]
    fn refined_event_keeping_its_name_resolves_to_the_abstract_event() {
        // The issue #84 case: an inline `extends` whose target keeps the event's
        // own name. Clicking the *target* jumps to the abstract event; clicking
        // the event's *own* name stays on the local declaration.
        let con_uri = "file:///concrete.eventb";
        let con = "MACHINE concrete_mch\nREFINES\n    abstract_mch\nVARIABLES\n    state\nEVENTS\n    EVENT ML_in extends ML_in\n    THEN\n        state ≔ state\n    END\nEND";
        let provider = setup(&[(ML_IN_ABS_URI, ML_IN_ABS), (con_uri, con)]);

        // The `extends` target (second `ML_in`, char 24) → abstract event.
        assert_goto(
            &provider,
            con_uri,
            con,
            6,
            26,
            ML_IN_ABS_URI,
            range(4, 10, 15),
        );
        // The event's own name (first `ML_in`, char 10) stays local.
        assert_goto(&provider, con_uri, con, 6, 12, con_uri, range(6, 10, 15));
    }

    #[test]
    fn refined_event_keeping_its_name_via_body_refines_resolves_to_the_abstract_event() {
        // The same, through a body-level `REFINES` clause with the kept name.
        let con_uri = "file:///concrete.eventb";
        let con = "MACHINE concrete_mch\nREFINES\n    abstract_mch\nVARIABLES\n    state\nEVENTS\n    EVENT ML_in\n    REFINES ML_in\n    THEN\n        state ≔ state\n    END\nEND";
        let provider = setup(&[(ML_IN_ABS_URI, ML_IN_ABS), (con_uri, con)]);

        // The `REFINES` target (`ML_in` at char 12) → abstract event.
        assert_goto(
            &provider,
            con_uri,
            con,
            7,
            14,
            ML_IN_ABS_URI,
            range(4, 10, 15),
        );
    }

    // `C1` on `context C1` spans cols 8..10 in every casing variant below.
    const C1_NAME: Range = Range {
        start: Position {
            line: 0,
            character: 8,
        },
        end: Position {
            line: 0,
            character: 10,
        },
    };

    /// A cursor on the `C1` in `sees C1` must jump to `context C1` (line 0), in a
    /// single document that holds both (as in base-model.eventb).
    fn assert_sees_target(source: &str, line: u32, character: u32) {
        let uri = "file:///model.eventb";
        let provider = setup(&[(uri, source)]);
        assert_goto(&provider, uri, source, line, character, uri, C1_NAME);
    }

    #[test]
    fn lowercase_sees_resolves_same_file_context() {
        // The originally reported lowercase keywords (as in base-model.eventb).
        let source = "context C1\nsets\n    S1\nend\n\nmachine M1\nsees C1\nvariables\n    v\nend";
        assert_sees_target(source, 6, 5);
    }

    #[test]
    fn mixed_case_sees_resolves_same_file_context() {
        let source = "Context C1\nSets\n    S1\nEnd\n\nMachine M1\nSees C1\nVariables\n    v\nEnd";
        assert_sees_target(source, 6, 5);
    }

    #[test]
    fn uppercase_sees_still_resolves() {
        let source = "CONTEXT C1\nSETS\n    S1\nEND\n\nMACHINE M1\nSEES C1\nVARIABLES\n    v\nEND";
        assert_sees_target(source, 6, 5);
    }
}
