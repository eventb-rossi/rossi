//! Resolving an obligation's provenance handles to model elements.
//!
//! A `poSource` row names its element by handle: the file, the
//! component, then one `type#internalName` step per nesting level. The
//! internal name is the label for text input and a generated counter
//! for imported XML, so the handle alone does not say which invariant
//! or guard is meant. The checked model does: every declaration it
//! keeps carries the handle the checker wrote for it. This index walks
//! the model once and answers by handle.

use std::collections::HashMap;

use crate::handles::{HandleUri, handle_segments};
use crate::sc_model::ScModel;
use crate::sc_view::normalize_source;

/// What kind of element a handle names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementKind {
    Context,
    Machine,
    CarrierSet,
    Constant,
    Axiom,
    Extends,
    Sees,
    Refines,
    Variable,
    Invariant,
    Variant,
    Event,
    RefinesEvent,
    Parameter,
    Guard,
    Action,
    Witness,
}

/// The element a provenance handle names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceElement {
    /// The component that declares the element.
    pub component: String,
    pub kind: ElementKind,
    /// The label of a labelled element, the identifier of a
    /// declaration, or the target of a clause.
    pub name: String,
    /// The event a parameter, guard, action, witness or event
    /// refinement belongs to.
    pub event: Option<String>,
    /// Whether a labelled predicate is a theorem.
    pub theorem: bool,
}

/// Every element of a checked model, by the handle the checker wrote
/// for it.
#[derive(Debug, Default)]
pub struct SourceIndex {
    by_handle: HashMap<String, SourceElement>,
}

impl SourceIndex {
    /// Indexes every declaration `model` kept. An action an extended
    /// event inherits is indexed under the ancestor that declares it,
    /// which is where its handle points.
    pub fn from_model(model: &ScModel) -> SourceIndex {
        use ElementKind::*;
        let mut index = SourceIndex::default();
        for context in model.contexts.values() {
            let record = &context.record;
            let component = record.name.as_str();
            for set in &record.carrier_sets {
                index.insert(&set.source, component, CarrierSet, &set.name, None, false);
            }
            for constant in &record.constants {
                index.insert(
                    &constant.source,
                    component,
                    Constant,
                    &constant.name,
                    None,
                    false,
                );
            }
            for axiom in &record.axioms {
                index.insert(
                    &axiom.source,
                    component,
                    Axiom,
                    &axiom.label,
                    None,
                    axiom.is_theorem,
                );
            }
            for extends in &record.extends {
                index.insert(
                    &extends.source,
                    component,
                    Extends,
                    &extends.parent_name,
                    None,
                    false,
                );
            }
        }
        for machine in model.machines.values() {
            let record = &machine.record;
            let component = record.name.as_str();
            if let Some(refines) = &record.refines {
                index.insert(
                    &refines.source,
                    component,
                    Refines,
                    &refines.parent_name,
                    None,
                    false,
                );
            }
            for sees in &record.sees {
                index.insert(&sees.source, component, Sees, &sees.name, None, false);
            }
            for variable in &record.variables {
                index.insert(
                    &variable.source,
                    component,
                    Variable,
                    &variable.name,
                    None,
                    false,
                );
            }
            for invariant in &record.invariants {
                index.insert(
                    &invariant.source,
                    component,
                    Invariant,
                    &invariant.label,
                    None,
                    invariant.is_theorem,
                );
            }
            for variant in &record.variants {
                index.insert(
                    &variant.source,
                    component,
                    Variant,
                    &variant.label,
                    None,
                    false,
                );
            }
            for event in &record.events {
                let within = Some(event.label.as_str());
                index.insert(&event.source, component, Event, &event.label, None, false);
                for refines in &event.refines {
                    index.insert(
                        &refines.source,
                        component,
                        RefinesEvent,
                        &refines.abstract_label,
                        within,
                        false,
                    );
                }
                for parameter in &event.parameters {
                    index.insert(
                        &parameter.source,
                        component,
                        Parameter,
                        &parameter.name,
                        within,
                        false,
                    );
                }
                for guard in &event.guards {
                    index.insert(
                        &guard.source,
                        component,
                        Guard,
                        &guard.label,
                        within,
                        guard.is_theorem,
                    );
                }
                for action in event.own_actions() {
                    index.insert(
                        &action.source,
                        component,
                        Action,
                        &action.label,
                        within,
                        false,
                    );
                }
                for witness in &event.witnesses {
                    index.insert(
                        &witness.source,
                        component,
                        Witness,
                        &witness.label,
                        within,
                        false,
                    );
                }
            }
        }
        index
    }

    fn insert(
        &mut self,
        handle: &HandleUri,
        component: &str,
        kind: ElementKind,
        name: &str,
        event: Option<&str>,
        theorem: bool,
    ) {
        // An implicit event refinement (INITIALISATION, an extended
        // event) has no clause of its own and carries the event's
        // handle; the event, indexed first, is what that handle names.
        if let Some(key) = normalize_source(Some(handle.as_str().to_string())) {
            self.by_handle.entry(key).or_insert_with(|| SourceElement {
                component: component.to_string(),
                kind,
                name: name.to_string(),
                event: event.map(str::to_string),
                theorem,
            });
        }
    }

    /// The element `handle` names, whether the handle still carries its
    /// project segment or was normalized without it. A handle that
    /// names a component file itself resolves to the component.
    pub fn resolve(&self, handle: &str) -> Option<SourceElement> {
        let key = normalize_source(Some(handle.to_string()))?;
        if let Some(found) = self.by_handle.get(&key) {
            return Some(found.clone());
        }
        let mut segments = handle_segments(&key);
        let kind = match segments.as_slice() {
            [(ty, _)] if ty == "org.eventb.core.machineFile" => ElementKind::Machine,
            [(ty, _)] if ty == "org.eventb.core.contextFile" => ElementKind::Context,
            _ => return None,
        };
        let name = segments.swap_remove(0).1;
        Some(SourceElement {
            component: name.clone(),
            kind,
            name,
            event: None,
            theorem: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_component_file_handle_resolves_to_the_component() {
        let index = SourceIndex::default();
        let machine = index
            .resolve("/P/M.bum|org.eventb.core.machineFile#M")
            .expect("machine file");
        assert_eq!(
            (machine.kind, machine.component.as_str()),
            (ElementKind::Machine, "M")
        );
        let context = index
            .resolve("C.buc|org.eventb.core.contextFile#C")
            .expect("normalized context file");
        assert_eq!(
            (context.kind, context.name.as_str()),
            (ElementKind::Context, "C")
        );
        assert!(
            index
                .resolve("/P/M.bum|org.eventb.core.machineFile#M|org.eventb.core.invariant#inv1")
                .is_none()
        );
    }
}
