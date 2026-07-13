//! Attach source comments to the AST elements they follow.
//!
//! The pest grammar consumes `//` and `/* */` comments silently, so the
//! parser never sees them. This post-parse pass re-scans the source with
//! [`crate::comments`] (the single comment lexer) and stores each comment's
//! text in the `comment` field of the nearest preceding commentable element
//! — the same "comment documents the element it follows" convention Rodin's
//! Camille editor uses. The pretty printer emits these fields back out, so
//! parse → print round-trips comments (issue #31).
//!
//! Attachment is by byte position: an element "anchors" at the start of its
//! span, and a comment belongs to the anchor with the greatest start not
//! after the comment. Comments before the first anchor (e.g. above the
//! `MACHINE` header) attach to the component itself. Several comments
//! landing on one element are joined with `\n` — exactly how a multiline
//! Rodin comment is stored, so XML → text → XML preserves comment content.

use crate::ast::{Component, Span};
use crate::comments;

/// Attach every comment in `source` to the element it follows.
///
/// `components` must have been parsed from `source` (spans are byte offsets
/// into it). Elements without location info (none, after a strict parse)
/// simply never receive comments.
pub(crate) fn attach_comments(source: &str, components: &mut [Component]) {
    let comment_spans = comments::comment_spans(source);
    attach_comments_from_spans(source, components, &comment_spans);
}

/// Attach comments using spans already produced by the shared lexical scan.
pub(crate) fn attach_comments_from_spans(
    source: &str,
    components: &mut [Component],
    comment_spans: &[Span],
) {
    if comment_spans.is_empty() {
        return;
    }

    // (anchor byte, comment slot) for every commentable element.
    let mut anchors: Vec<(usize, &mut Option<String>)> = Vec::new();

    // Record `slot` as an anchor at the start of `span`, if the element has
    // one. (A closure can't be used here — it would need `&mut anchors`
    // captured alongside the `&mut …comment` slot borrows — so a macro that
    // expands inline is the clean fit.)
    macro_rules! anchor {
        ($span:expr, $slot:expr) => {
            if let Some(span) = $span {
                anchors.push((span.start, $slot));
            }
        };
    }

    for component in components.iter_mut() {
        match component {
            Component::Context(ctx) => {
                anchor!(ctx.span.or(ctx.name_span), &mut ctx.comment);
                for set in &mut ctx.sets {
                    anchor!(set.span(), set.comment_mut());
                }
                for constant in &mut ctx.constants {
                    anchor!(constant.span, &mut constant.comment);
                }
                for axiom in &mut ctx.axioms {
                    anchor!(axiom.span, &mut axiom.comment);
                }
            }
            Component::Machine(machine) => {
                anchor!(machine.span.or(machine.name_span), &mut machine.comment);
                for variable in &mut machine.variables {
                    anchor!(variable.span, &mut variable.comment);
                }
                for invariant in &mut machine.invariants {
                    anchor!(invariant.span, &mut invariant.comment);
                }
                if let Some(init) = &mut machine.initialisation {
                    anchor!(init.span, &mut init.comment);
                    for action in &mut init.actions {
                        anchor!(action.span, &mut action.comment);
                    }
                    for predicate in init.with.iter_mut().chain(&mut init.witnesses) {
                        anchor!(predicate.span, &mut predicate.comment);
                    }
                }
                for event in &mut machine.events {
                    anchor!(event.span.or(event.name_span), &mut event.comment);
                    for parameter in &mut event.parameters {
                        anchor!(parameter.span, &mut parameter.comment);
                    }
                    for predicate in event
                        .guards
                        .iter_mut()
                        .chain(&mut event.with)
                        .chain(&mut event.witnesses)
                    {
                        anchor!(predicate.span, &mut predicate.comment);
                    }
                    for action in &mut event.actions {
                        anchor!(action.span, &mut action.comment);
                    }
                }
            }
        }
    }
    if anchors.is_empty() {
        return;
    }

    // INITIALISATION may sit between events, and multi-component files
    // interleave: struct order is not source order.
    anchors.sort_by_key(|(start, _)| *start);

    for span in comment_spans {
        let Some(text) = comments::comment_text(&source[span.start..span.end]) else {
            continue; // blank comment — nothing worth keeping
        };
        // Nearest preceding anchor; a comment before everything (above the
        // first component header) documents the component.
        let i = anchors
            .partition_point(|(start, _)| *start <= span.start)
            .saturating_sub(1);
        let slot = &mut *anchors[i].1;
        match slot {
            Some(existing) => {
                existing.push('\n');
                existing.push_str(&text);
            }
            None => *slot = Some(text),
        }
    }
}
