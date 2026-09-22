//! Resolving a proof obligation's name back to the source element it is
//! about.
//!
//! The proof-obligation generator names each sequent after the elements it
//! was derived from, in a fixed shape (see `rossi_build::pog`):
//!
//! - `<label>/<suffix>` for a component-level element: an invariant's
//!   `inv1/WD`, a theorem's `thm1/THM`, an axiom's `axm1/WD`;
//! - `<event>/<label>/<suffix>` for an element inside an event: a guard's
//!   `evt/grd1/GRD`, an action's `evt/act1/FIS`, a witness's `evt/w/WFIS`,
//!   and, for invariant preservation, the *machine's* invariant under the
//!   event that must preserve it, `evt/inv1/INV`;
//! - `<event>/<suffix>` for an obligation about the event as a whole:
//!   `evt/MRG`, `evt/VAR`, `evt/NAT`.
//!
//! The checked model that produced the obligation carries no spans, so the
//! name is parsed and its parts looked up in the parsed component. Every
//! miss falls one level out, down to the component's name, so an obligation
//! always anchors somewhere the user can see rather than on the file start.

use rossi::Component;
use rossi::ast::{LabeledAction, LabeledPredicate, Span};
use rossi::keywords::{KeywordId, spell};

use super::Block;
use crate::lsp_types::Range;
use crate::position::PositionIndex;
use crate::symbols::{INITIALISATION_EVENT_NAME, event_declaration_span};

/// The range the obligation `name` is about, for `component`, through the
/// caller's position index (one per document, shared across obligations).
pub(crate) fn range_for(component: &Component, name: &str, index: &PositionIndex) -> Range {
    let span = span_for(component, name).or_else(|| component.name_span());
    span.map(|span| Range::new(index.position(span.start), index.position(span.end)))
        .unwrap_or_else(crate::analysis::default_range)
}

/// The blocks of `component`, in source order: the clauses that hold
/// labeled elements (invariants, theorems, variant, axioms), then the
/// initialisation and the events. EVENTS itself is not a block, so no two
/// blocks overlap. A component without spans (an XML import, a recovered
/// parse) contributes none.
pub(crate) fn blocks_for(component: &Component, index: &PositionIndex) -> Vec<Block> {
    let mut blocks: Vec<Block> = component
        .clauses()
        .iter()
        .filter(|clause| {
            matches!(
                clause.keyword,
                KeywordId::Invariants
                    | KeywordId::Theorems
                    | KeywordId::Variant
                    | KeywordId::Axioms
            )
        })
        .map(|clause| {
            let keyword = spell(clause.keyword);
            let header = Span {
                start: clause.span.start,
                end: clause.span.start + keyword.len(),
            };
            Block {
                name: keyword.to_string(),
                header: index.range(&header),
                range: index.range(&clause.span),
            }
        })
        .collect();
    if let Component::Machine(machine) = component {
        let events = machine
            .initialisation
            .iter()
            .map(|init| (INITIALISATION_EVENT_NAME, init.span, init.name_span))
            .chain(
                machine
                    .events
                    .iter()
                    .map(|event| (event.name.as_str(), event.span, event.name_span)),
            );
        for (name, span, name_span) in events {
            let (Some(span), Some(name_span)) = (span, name_span) else {
                continue;
            };
            blocks.push(Block {
                name: name.to_string(),
                header: index.range(&name_span),
                range: index.range(&span),
            });
        }
    }
    blocks
}

fn span_for(component: &Component, name: &str) -> Option<Span> {
    let parts: Vec<&str> = name.split('/').collect();
    match (component, parts.as_slice()) {
        (Component::Machine(machine), [event, label, _suffix]) => {
            // Inside the event first, then the machine's invariants (for
            // `evt/inv/INV`), then the event itself: an `evt/x/EQL` names a
            // variable, which has no labeled element to land on.
            event_element_span(machine, event, label)
                .or_else(|| predicate_span(&machine.invariants, label))
                .or_else(|| event_declaration_span(component, event))
        }
        (Component::Machine(machine), [label, _suffix]) => {
            predicate_span(&machine.invariants, label)
                .or_else(|| {
                    machine
                        .variants
                        .iter()
                        .find(|variant| variant.label.as_deref() == Some(label))
                        .and_then(|variant| variant.span)
                })
                .or_else(|| event_declaration_span(component, label))
        }
        (Component::Context(context), [label, _suffix]) => predicate_span(&context.axioms, label),
        _ => None,
    }
}

/// The span of the labeled guard, witness, `with` predicate or action named
/// `label` inside the event named `event`.
fn event_element_span(machine: &rossi::Machine, event: &str, label: &str) -> Option<Span> {
    if event == INITIALISATION_EVENT_NAME {
        let init = machine.initialisation.as_ref()?;
        return predicate_span(&init.with, label)
            .or_else(|| predicate_span(&init.witnesses, label))
            .or_else(|| action_span(&init.actions, label));
    }
    let event = machine.events.iter().find(|e| e.name == event)?;
    predicate_span(&event.guards, label)
        .or_else(|| predicate_span(&event.with, label))
        .or_else(|| predicate_span(&event.witnesses, label))
        .or_else(|| action_span(&event.actions, label))
}

fn predicate_span(predicates: &[LabeledPredicate], label: &str) -> Option<Span> {
    predicates
        .iter()
        .find(|p| p.label.as_deref() == Some(label))
        .and_then(|p| p.span)
}

fn action_span(actions: &[LabeledAction], label: &str) -> Option<Span> {
    actions
        .iter()
        .find(|a| a.label.as_deref() == Some(label))
        .and_then(|a| a.span)
}
