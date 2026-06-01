//! Shared Event-B symbol model for LSP providers.
//!
//! `SymbolKind` classifies the five kinds of named symbols, and
//! `enumerate_symbols` walks a parsed `Component` to list them. Go-to-definition
//! and find-references both build on this single taxonomy so it can't drift
//! between features.

use rossi::Component;

/// The kind of an Event-B named symbol.
///
/// Sets and constants are declared in contexts; variables, events, and
/// parameters in machines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Set,
    Constant,
    Variable,
    Event,
    Parameter,
}

/// A symbol discovered while walking a component: its name, kind, the owning
/// component (machine or context), and — for parameters — the enclosing event.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SymbolRef {
    pub name: String,
    pub kind: SymbolKind,
    pub owner: String,
    pub event: Option<String>,
}

/// Enumerate every named symbol declared directly in `component`.
///
/// Order is stable and follows source declaration order: contexts yield sets
/// then constants; machines yield variables, the INITIALISATION event (when
/// present), then each event followed by its parameters.
pub fn enumerate_symbols(component: &Component) -> Vec<SymbolRef> {
    let mut symbols = Vec::new();

    match component {
        Component::Context(context) => {
            for set in &context.sets {
                symbols.push(SymbolRef {
                    name: set.name().to_string(),
                    kind: SymbolKind::Set,
                    owner: context.name.clone(),
                    event: None,
                });
            }
            for constant in &context.constants {
                symbols.push(SymbolRef {
                    name: constant.name.clone(),
                    kind: SymbolKind::Constant,
                    owner: context.name.clone(),
                    event: None,
                });
            }
        }
        Component::Machine(machine) => {
            for variable in &machine.variables {
                symbols.push(SymbolRef {
                    name: variable.name.clone(),
                    kind: SymbolKind::Variable,
                    owner: machine.name.clone(),
                    event: None,
                });
            }
            if machine.initialisation.is_some() {
                symbols.push(SymbolRef {
                    name: "INITIALISATION".to_string(),
                    kind: SymbolKind::Event,
                    owner: machine.name.clone(),
                    event: None,
                });
            }
            for event in &machine.events {
                symbols.push(SymbolRef {
                    name: event.name.clone(),
                    kind: SymbolKind::Event,
                    owner: machine.name.clone(),
                    event: None,
                });
                for parameter in &event.parameters {
                    symbols.push(SymbolRef {
                        name: parameter.name.clone(),
                        kind: SymbolKind::Parameter,
                        owner: machine.name.clone(),
                        event: Some(event.name.clone()),
                    });
                }
            }
        }
    }

    symbols
}
