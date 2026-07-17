//! Shared helpers for multi-component documents.
//!
//! A `.eventb` file may contain several `CONTEXT`/`MACHINE` blocks (the
//! output of `rossi import --merge`). Providers that used to parse a document
//! into a single [`Component`] use these helpers instead: parse every
//! component and pick the one under the cursor for position-based features.
//! Cross-file lookups by name go through
//! [`ComponentLoader`](crate::component_loader::ComponentLoader), which parses
//! each file at most once per request.

use rossi::deps::{ComponentKind, EdgeKind};
use rossi::keywords::KeywordId;
use rossi::{Component, ComponentNameSite};

use crate::lsp_types::Position;
use crate::position::position_to_offset;
use crate::text_utils;

/// Provider-neutral identity of a component name under the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComponentIdentity {
    pub(crate) name: String,
    pub(crate) site: ComponentNameSite,
}

impl ComponentIdentity {
    pub(crate) fn kind(&self) -> ComponentKind {
        match self.site {
            ComponentNameSite::Declaration(kind) => kind,
            ComponentNameSite::Dependency(edge) => edge.target_kind(),
        }
    }
}

/// Parse every component in `text`, recovering a partial AST from local syntax
/// errors. A single broken predicate no longer blanks out the whole document:
/// the component it sits in is recovered (sans the broken clause) and its
/// siblings parse normally. Returns an empty vector only when nothing at all
/// could be recovered — there is no `CONTEXT`/`MACHINE` header to anchor on.
pub fn parse_all(text: &str) -> Vec<Component> {
    rossi::parse_components_with_recovery(text)
        .component
        .unwrap_or_default()
}

/// Inclusive line window of a component within `text`, for bounding line-based
/// text searches to that component in a multi-component document. The whole
/// file when the span is missing (XML import, error recovery) — the
/// single-component behavior.
pub fn component_line_window(component: &Component, text: &str) -> (usize, usize) {
    match component.span() {
        Some(span) => (
            text[..span.start.min(text.len())].matches('\n').count(),
            text[..span.end.min(text.len())].matches('\n').count(),
        ),
        None => (0, usize::MAX),
    }
}

/// Iterate `(line_number, line)` pairs of `text` restricted to an inclusive
/// line window (as produced by [`component_line_window`]).
pub fn lines_in_window(text: &str, window: (usize, usize)) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .take(window.1.saturating_add(1))
        .skip(window.0)
}

/// The component containing byte `offset`.
///
/// Falls back gracefully when no span contains the offset (the cursor sits in
/// inter-component whitespace, or a recovered component carries no spans):
/// the last component that starts at or before the offset, else the first
/// component. Returns `None` only for an empty slice.
pub fn component_at_offset(components: &[Component], offset: usize) -> Option<&Component> {
    components
        .iter()
        .find(|c| c.span().is_some_and(|s| s.contains(offset)))
        .or_else(|| {
            components
                .iter()
                .rev()
                .find(|c| c.span().is_some_and(|s| s.start <= offset))
        })
        .or_else(|| components.first())
}

/// Resolve a component declaration or component-level dependency operand at
/// `position`. Exact recovery-scanner occurrences win; the clause scanner is
/// used by itself only when no component header could be recovered, preserving
/// support for a temporarily headerless dependency clause.
///
/// The trailing edge is included because LSP identifier lookup treats the
/// caret immediately after a name as targeting that name.
pub(crate) fn resolve_component_at_position(
    text: &str,
    masked: &str,
    position: Position,
    name: &str,
) -> Option<ComponentIdentity> {
    if !rossi::names::is_valid_component_name(name) {
        return None;
    }

    let offset = position_to_offset(text, position)?;
    let fallback_dependency = component_reference_clause(masked, position);
    if fallback_dependency.is_none() && !component_keyword_before_position(masked, position) {
        return None;
    }

    let site = rossi::component_name_occurrences_with_sites(text)
        .iter()
        .find(|occurrence| {
            occurrence.name == name
                && occurrence
                    .span
                    .is_some_and(|span| span.contains(offset) || span.end == offset)
        })
        .map(|occurrence| occurrence.site)
        .or_else(|| fallback_dependency.map(ComponentNameSite::Dependency))?;

    Some(ComponentIdentity {
        name: name.to_string(),
        site,
    })
}

/// Whether a structural component keyword precedes the cursor on its line.
/// This cheap gate keeps ordinary formula requests away from the full recovery
/// occurrence scan while still admitting compact one-line components.
fn component_keyword_before_position(masked: &str, position: Position) -> bool {
    let Some(offset) = position_to_offset(masked, position) else {
        return false;
    };
    let line_start = masked[..offset].rfind('\n').map_or(0, |index| index + 1);
    text_utils::identifier_words(&masked[line_start..offset])
        .into_iter()
        .filter_map(|word| rossi::keywords::lookup(&word))
        .any(|keyword| {
            matches!(
                keyword.id,
                KeywordId::Context
                    | KeywordId::Machine
                    | KeywordId::Extends
                    | KeywordId::Sees
                    | KeywordId::Refines
            )
        })
}

/// The component-level SEES/REFINES/EXTENDS clause `position` sits in, if any.
/// `masked` must be comment-masked text so keywords in prose cannot open or
/// close structural regions.
pub(crate) fn component_reference_clause(masked: &str, position: Position) -> Option<EdgeKind> {
    let mut current_clause = None;
    let mut in_event = false;
    let mut reached = false;

    for (idx, line) in masked.lines().enumerate() {
        if idx > position.line as usize {
            break;
        }
        reached |= idx == position.line as usize;

        if text_utils::event_name_from_line(line).is_some() {
            in_event = true;
            current_clause = None;
            continue;
        }

        if text_utils::line_keyword_is(line, KeywordId::End) && in_event {
            in_event = false;
            current_clause = None;
        } else if text_utils::line_keyword_is(line, KeywordId::Sees) && !in_event {
            current_clause = Some(EdgeKind::Sees);
        } else if text_utils::line_keyword_is(line, KeywordId::Extends) && !in_event {
            current_clause = Some(EdgeKind::Extends);
        } else if text_utils::line_keyword_is(line, KeywordId::Refines) && !in_event {
            current_clause = Some(EdgeKind::Refines);
        } else if text_utils::is_declaration_scan_boundary(line) {
            current_clause = None;
        }
    }

    reached.then_some(current_clause).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::identifier_utils::identifier_at_position;
    use crate::position::offset_to_position;

    const TWO_COMPONENTS: &str = "CONTEXT C0\nEND\n\nMACHINE M0\nVARIABLES\n    x\nEND\n";

    #[test]
    fn parse_all_returns_every_component() {
        let components = parse_all(TWO_COMPONENTS);
        let names: Vec<&str> = components.iter().map(|c| c.name()).collect();
        assert_eq!(names, vec!["C0", "M0"]);
    }

    #[test]
    fn parse_all_returns_empty_on_error() {
        // No CONTEXT/MACHINE header: recovery has nothing to anchor on, so the
        // result is still empty.
        assert!(parse_all("not event-b").is_empty());
    }

    #[test]
    fn parse_all_recovers_partial_components_on_local_error() {
        // A broken predicate (`@a k ∈` with no right-hand side) used to fail
        // the whole strict parse and blank the document. Recovery keeps both
        // components: the broken one (sans the bad axiom) and its sibling.
        let text = "CONTEXT C0\nCONSTANTS\n    k\nAXIOMS\n    @a k ∈\nEND\n\nMACHINE M0\nVARIABLES\n    x\nEND\n";
        let components = parse_all(text);
        let names: Vec<&str> = components.iter().map(|c| c.name()).collect();
        assert_eq!(names, vec!["C0", "M0"]);
    }

    #[test]
    fn component_at_offset_dispatches_by_position() {
        let components = parse_all(TWO_COMPONENTS);
        let in_c0 = TWO_COMPONENTS.find("C0").unwrap();
        let in_m0 = TWO_COMPONENTS.find("x").unwrap();

        assert_eq!(
            component_at_offset(&components, in_c0).unwrap().name(),
            "C0"
        );
        assert_eq!(
            component_at_offset(&components, in_m0).unwrap().name(),
            "M0"
        );
    }

    #[test]
    fn component_at_offset_gap_binds_to_preceding_component() {
        let components = parse_all(TWO_COMPONENTS);
        // The blank line between the two components.
        let gap = TWO_COMPONENTS.find("\n\n").unwrap() + 1;
        assert_eq!(component_at_offset(&components, gap).unwrap().name(), "C0");
    }

    #[test]
    fn component_at_offset_without_spans_falls_back_to_first() {
        let components = vec![Component::Context(rossi::Context::new("c".into()))];
        assert_eq!(component_at_offset(&components, 42).unwrap().name(), "c");
        assert!(component_at_offset(&[], 0).is_none());
    }

    fn resolve_at(source: &str, offset: usize) -> Option<ComponentIdentity> {
        let masked = rossi::comments::mask_comments_chars(source);
        let position = offset_to_position(source, offset);
        let (name, _) = identifier_at_position(source, position)?;
        resolve_component_at_position(source, &masked, position, &name)
    }

    #[test]
    fn component_resolution_classifies_declarations_dependencies_and_trailing_edges() {
        let source = "CONTEXT Base \nEND\n\nCONTEXT Derived\nEXTENDS Base \nEND\n\nMACHINE Abstract \nEND\n\nMACHINE Concrete\nREFINES Abstract \nSEES Base \nEND";
        let cases = [
            (
                "CONTEXT Base",
                ComponentIdentity {
                    name: "Base".into(),
                    site: ComponentNameSite::Declaration(ComponentKind::Context),
                },
            ),
            (
                "EXTENDS Base",
                ComponentIdentity {
                    name: "Base".into(),
                    site: ComponentNameSite::Dependency(EdgeKind::Extends),
                },
            ),
            (
                "MACHINE Abstract",
                ComponentIdentity {
                    name: "Abstract".into(),
                    site: ComponentNameSite::Declaration(ComponentKind::Machine),
                },
            ),
            (
                "REFINES Abstract",
                ComponentIdentity {
                    name: "Abstract".into(),
                    site: ComponentNameSite::Dependency(EdgeKind::Refines),
                },
            ),
            (
                "SEES Base",
                ComponentIdentity {
                    name: "Base".into(),
                    site: ComponentNameSite::Dependency(EdgeKind::Sees),
                },
            ),
        ];

        for (marker, expected) in cases {
            let offset = source.find(marker).unwrap() + marker.len();
            assert_eq!(resolve_at(source, offset), Some(expected), "{marker}");
        }
    }

    #[test]
    fn component_resolution_classifies_compact_inline_dependencies() {
        let source = "MACHINE Concrete REFINES Abstract SEES Base END";

        let refines = source.find("Abstract").unwrap();
        assert_eq!(
            resolve_at(source, refines),
            Some(ComponentIdentity {
                name: "Abstract".into(),
                site: ComponentNameSite::Dependency(EdgeKind::Refines),
            })
        );

        let sees = source.find("Base").unwrap();
        assert_eq!(
            resolve_at(source, sees),
            Some(ComponentIdentity {
                name: "Base".into(),
                site: ComponentNameSite::Dependency(EdgeKind::Sees),
            })
        );
    }

    #[test]
    fn headerless_dependency_recovery_accepts_keywords_and_partial_documents() {
        let keyword = "SEES MACHINE";
        assert_eq!(
            resolve_at(keyword, keyword.find("MACHINE").unwrap()),
            Some(ComponentIdentity {
                name: "MACHINE".into(),
                site: ComponentNameSite::Dependency(EdgeKind::Sees),
            })
        );

        let partial = "SEES Base\nEND\n\nMACHINE M\nEND";
        assert_eq!(
            resolve_at(partial, partial.find("Base").unwrap()),
            Some(ComponentIdentity {
                name: "Base".into(),
                site: ComponentNameSite::Dependency(EdgeKind::Sees),
            })
        );
    }

    #[test]
    fn declaration_resolution_survives_an_ast_wide_depth_error() {
        let source = format!(
            "CONTEXT Deep\nAXIOMS\n    @a {}(1 = 1)\nEND",
            "¬".repeat(rossi::MAX_NESTING_DEPTH + 1)
        );
        assert!(parse_all(&source).is_empty());

        assert_eq!(
            resolve_at(&source, source.find("Deep").unwrap()),
            Some(ComponentIdentity {
                name: "Deep".into(),
                site: ComponentNameSite::Declaration(ComponentKind::Context),
            })
        );
    }

    #[test]
    fn headerless_dependency_recovery_is_utf16_aware_and_stays_structural() {
        let source = "SEES /* 😀 */ Base \nVARIABLES\n    Base\n";
        let dependency_offset = source.find("Base ").unwrap() + "Base".len();
        assert_eq!(
            resolve_at(source, dependency_offset),
            Some(ComponentIdentity {
                name: "Base".into(),
                site: ComponentNameSite::Dependency(EdgeKind::Sees),
            })
        );

        let formula_offset = source.rfind("Base").unwrap();
        assert_eq!(resolve_at(source, formula_offset), None);
    }

    #[test]
    fn event_refinement_and_formula_collisions_are_not_components() {
        let source = "MACHINE Base\nVARIABLES\n    Base\nEVENTS\n    EVENT Base extends Base\n    THEN\n        Base := Base\n    END\nEND";

        for marker in [
            "VARIABLES\n    Base",
            "EVENT Base",
            "extends Base",
            ":= Base",
        ] {
            let offset = source.find(marker).unwrap() + marker.rfind("Base").unwrap();
            assert_eq!(resolve_at(source, offset), None, "{marker}");
        }
    }
}
