//! Attach source comments to the AST elements they document.
//!
//! The pest grammar consumes `//` and `/* */` comments silently, so the
//! parser never sees them. This post-parse pass re-scans the source with
//! [`crate::comments`] (the single comment lexer) and stores each comment's
//! text in the `comment` field of a commentable element. A comment after code
//! on the same line documents that element; a comment on its own line
//! introduces the next one. The pretty printer emits these fields back out,
//! so parse → print round-trips comments (issue #31).
//!
//! This is the carrier Rodin can store, and the one `rossi export` needs. It
//! is deliberately *not* what `rossi fmt` uses: text -> text formatting keeps a
//! comment's own position instead, via [`crate::comment_place`]. That module
//! enumerates the same elements as this one, so the two lists have to stay in
//! step — every element given a comment slot here needs an `Item` anchor there,
//! or `fmt` will drop its comments.
//!
//! Attachment is by byte position: an element "anchors" at the start of its
//! span. Comments before the first anchor (e.g. above the `MACHINE` header)
//! attach to the component itself. A standalone comment with no next element
//! before `END` attaches to the preceding one. Several comments landing on
//! one element are joined with `\n`, as in a multiline Rodin comment.

use crate::ast::{Component, Span};
use crate::comments;

/// Attach every comment in `source` to its documented element.
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

    // (source span, leaf item, comment slot) for every commentable element.
    let mut anchors: Vec<(Span, bool, &mut Option<String>)> = Vec::new();
    let mut scope_ends = Vec::new();

    // Record `slot` as an anchor at the start of `span`, if the element has
    // one. (A closure can't be used here — it would need `&mut anchors`
    // captured alongside the `&mut …comment` slot borrows — so a macro that
    // expands inline is the clean fit.)
    macro_rules! anchor {
        ($span:expr, $slot:expr, $leaf:expr) => {
            if let Some(span) = $span {
                anchors.push((span, $leaf, $slot));
            }
        };
    }

    for component in components.iter_mut() {
        match component {
            Component::Context(ctx) => {
                anchor!(ctx.span.or(ctx.name_span), &mut ctx.comment, false);
                scope_ends.extend(ctx.span.map(|span| span.end));
                for set in &mut ctx.sets {
                    anchor!(set.span, &mut set.comment, true);
                }
                for constant in &mut ctx.constants {
                    anchor!(constant.span, &mut constant.comment, true);
                }
                for axiom in &mut ctx.axioms {
                    anchor!(axiom.span, &mut axiom.comment, true);
                }
            }
            Component::Machine(machine) => {
                anchor!(
                    machine.span.or(machine.name_span),
                    &mut machine.comment,
                    false
                );
                scope_ends.extend(machine.span.map(|span| span.end));
                for variable in &mut machine.variables {
                    anchor!(variable.span, &mut variable.comment, true);
                }
                for invariant in &mut machine.invariants {
                    anchor!(invariant.span, &mut invariant.comment, true);
                }
                for variant in &mut machine.variants {
                    anchor!(variant.span, &mut variant.comment, true);
                }
                if let Some(init) = &mut machine.initialisation {
                    anchor!(init.span, &mut init.comment, false);
                    scope_ends.extend(init.span.map(|span| span.end));
                    for action in &mut init.actions {
                        anchor!(action.span, &mut action.comment, true);
                    }
                    for predicate in init.with.iter_mut().chain(&mut init.witnesses) {
                        anchor!(predicate.span, &mut predicate.comment, true);
                    }
                }
                for event in &mut machine.events {
                    anchor!(event.span.or(event.name_span), &mut event.comment, false);
                    scope_ends.extend(event.span.map(|span| span.end));
                    for parameter in &mut event.parameters {
                        anchor!(parameter.span, &mut parameter.comment, true);
                    }
                    for predicate in event
                        .guards
                        .iter_mut()
                        .chain(&mut event.with)
                        .chain(&mut event.witnesses)
                    {
                        anchor!(predicate.span, &mut predicate.comment, true);
                    }
                    for action in &mut event.actions {
                        anchor!(action.span, &mut action.comment, true);
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
    anchors.sort_by_key(|(span, _, _)| span.start);
    scope_ends.sort_unstable();
    let code = comments::mask_comments(source);

    for span in comment_spans {
        let Some(text) = comments::comment_text(&source[span.start..span.end]) else {
            continue; // blank comment — nothing worth keeping
        };
        let previous = anchors
            .partition_point(|(anchor, _, _)| anchor.start <= span.start)
            .saturating_sub(1);
        let following = anchors.partition_point(|(anchor, _, _)| anchor.start < span.end);
        let after_code = code[..span.start]
            .rsplit('\n')
            .next()
            .is_some_and(|line| !line.trim().is_empty());
        // Pest spans may swallow the comment itself as trailing trivia. It is
        // inside the item only when code from that item follows the comment.
        let inside_item = anchors[previous].1
            && anchors[previous].0.end > span.end
            && !code[span.end..anchors[previous].0.end].trim().is_empty();
        // Do not move a comment past the END of its component or event.
        let next_before_end = anchors.get(following).is_some_and(|(next, _, _)| {
            let end = scope_ends.partition_point(|end| *end <= span.start);
            scope_ends.get(end).is_none_or(|end| next.start < *end)
        });
        let i = if after_code || inside_item || !next_before_end {
            previous
        } else {
            following
        };
        let slot = &mut *anchors[i].2;
        match slot {
            Some(existing) => {
                existing.push('\n');
                existing.push_str(&text);
            }
            None => *slot = Some(text),
        }
    }
}
