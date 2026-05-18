//! AST analysis and symbol extraction
//!
//! This module analyzes Event-B components and extracts symbols for navigation
//! and other LSP features.

use lsp_types::{DocumentSymbol, Position, Range, SymbolKind};
use rossi::ast::Span;
use rossi::{Component, Context, Event, EventStatus, Machine};

/// Convert a Span to an LSP Range using the source text
fn span_to_range(span: &Span, source: &str) -> Range {
    let (start_line, start_col) = span.to_line_col(source);

    // Calculate end position
    let mut line = start_line;
    let mut col = start_col;
    for (i, c) in source.char_indices() {
        if i >= span.end {
            break;
        }
        if i >= span.start {
            if c == '\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
    }

    Range {
        start: Position::new(start_line as u32, start_col as u32),
        end: Position::new(line as u32, col as u32),
    }
}

/// Extract document symbols from a component
pub fn extract_symbols(component: &Component, source: &str) -> Vec<DocumentSymbol> {
    match component {
        Component::Context(ctx) => extract_context_symbols(ctx, source),
        Component::Machine(machine) => extract_machine_symbols(machine, source),
    }
}

/// Extract symbols from a Context
fn extract_context_symbols(ctx: &Context, source: &str) -> Vec<DocumentSymbol> {
    let mut symbols = Vec::new();

    // Add SETS as enum symbols
    for set in &ctx.sets {
        symbols.push(create_symbol(
            set.name().to_string(),
            SymbolKind::ENUM,
            "Set",
            default_range(),
        ));
    }

    // Add CONSTANTS as constant symbols
    for constant in &ctx.constants {
        symbols.push(create_symbol(
            constant.name.clone(),
            SymbolKind::CONSTANT,
            "Constant",
            default_range(),
        ));
    }

    // Add AXIOMS (including theorems) as property symbols
    for axiom in &ctx.axioms {
        let label = axiom
            .label
            .clone()
            .unwrap_or_else(|| "unlabeled".to_string());
        let range = axiom
            .span
            .as_ref()
            .map_or_else(default_range, |span| span_to_range(span, source));
        let detail = if axiom.is_theorem { "Theorem" } else { "Axiom" };
        symbols.push(create_symbol(label, SymbolKind::PROPERTY, detail, range));
    }

    // Wrap in a parent symbol for the context
    let range = ctx
        .span
        .as_ref()
        .map_or_else(default_range, |span| span_to_range(span, source));
    let name_range = ctx
        .name_span
        .as_ref()
        .map_or_else(default_range, |span| span_to_range(span, source));

    let context_symbol = DocumentSymbol {
        name: ctx.name.clone(),
        detail: Some("Context".to_string()),
        kind: SymbolKind::MODULE,
        tags: None,
        range,
        selection_range: name_range,
        children: Some(symbols),
        #[allow(deprecated)]
        deprecated: None,
    };

    vec![context_symbol]
}

/// Extract symbols from a Machine
fn extract_machine_symbols(machine: &Machine, source: &str) -> Vec<DocumentSymbol> {
    let mut symbols = Vec::new();

    // Add VARIABLES as variable symbols
    for var in &machine.variables {
        symbols.push(create_symbol(
            var.name.clone(),
            SymbolKind::VARIABLE,
            "Variable",
            default_range(),
        ));
    }

    // Add INVARIANTS (including theorems) as property symbols
    for invariant in &machine.invariants {
        let label = invariant
            .label
            .clone()
            .unwrap_or_else(|| "unlabeled".to_string());
        let range = invariant
            .span
            .as_ref()
            .map_or_else(default_range, |span| span_to_range(span, source));
        let detail = if invariant.is_theorem {
            "Theorem"
        } else {
            "Invariant"
        };
        symbols.push(create_symbol(label, SymbolKind::PROPERTY, detail, range));
    }

    // Add VARIANT if present
    if machine.variant.is_some() {
        symbols.push(create_symbol(
            "variant".to_string(),
            SymbolKind::NUMBER,
            "Variant",
            default_range(),
        ));
    }

    // Add INITIALISATION event if present
    if let Some(init) = &machine.initialisation {
        let mut init_children = Vec::new();

        // Add actions as nested symbols
        for action in &init.actions {
            let label = action
                .label
                .clone()
                .unwrap_or_else(|| "unlabeled".to_string());
            let range = action
                .span
                .as_ref()
                .map_or_else(default_range, |span| span_to_range(span, source));
            init_children.push(create_symbol(label, SymbolKind::PROPERTY, "Action", range));
        }

        // InitialisationEvent doesn't have its own span, use default
        let range = default_range();

        let init_symbol = DocumentSymbol {
            name: "INITIALISATION".to_string(),
            detail: Some("Event".to_string()),
            kind: SymbolKind::CONSTRUCTOR,
            tags: None,
            range,
            selection_range: range,
            children: if init_children.is_empty() {
                None
            } else {
                Some(init_children)
            },
            #[allow(deprecated)]
            deprecated: None,
        };
        symbols.push(init_symbol);
    }

    // Add EVENTS as function symbols
    for event in &machine.events {
        symbols.push(extract_event_symbol(event, source));
    }

    // Wrap in a parent symbol for the machine
    let range = machine
        .span
        .as_ref()
        .map_or_else(default_range, |span| span_to_range(span, source));
    let name_range = machine
        .name_span
        .as_ref()
        .map_or_else(default_range, |span| span_to_range(span, source));

    let machine_symbol = DocumentSymbol {
        name: machine.name.clone(),
        detail: Some("Machine".to_string()),
        kind: SymbolKind::MODULE,
        tags: None,
        range,
        selection_range: name_range,
        children: Some(symbols),
        #[allow(deprecated)]
        deprecated: None,
    };

    vec![machine_symbol]
}

/// Extract symbol from an Event
fn extract_event_symbol(event: &Event, source: &str) -> DocumentSymbol {
    let mut children = Vec::new();

    // Add parameters
    for param in &event.parameters {
        children.push(create_symbol(
            param.name.clone(),
            SymbolKind::TYPE_PARAMETER,
            "Parameter",
            default_range(),
        ));
    }

    // Add guards
    for guard in &event.guards {
        let label = guard
            .label
            .clone()
            .unwrap_or_else(|| "unlabeled".to_string());
        let range = guard
            .span
            .as_ref()
            .map_or_else(default_range, |span| span_to_range(span, source));
        children.push(create_symbol(label, SymbolKind::PROPERTY, "Guard", range));
    }

    // Add WITH bindings
    for lp in &event.with {
        let label = lp.label.clone().unwrap_or_else(|| "unlabeled".to_string());
        children.push(create_symbol(
            label,
            SymbolKind::PROPERTY,
            "With",
            default_range(),
        ));
    }

    // Add witnesses
    for lp in &event.witnesses {
        let label = lp.label.clone().unwrap_or_else(|| "unlabeled".to_string());
        children.push(create_symbol(
            label,
            SymbolKind::PROPERTY,
            "Witness",
            default_range(),
        ));
    }

    // Add actions
    for action in &event.actions {
        let label = action
            .label
            .clone()
            .unwrap_or_else(|| "unlabeled".to_string());
        let range = action
            .span
            .as_ref()
            .map_or_else(default_range, |span| span_to_range(span, source));
        children.push(create_symbol(label, SymbolKind::PROPERTY, "Action", range));
    }

    // Determine event detail based on status
    let detail = match event.status {
        Some(EventStatus::Ordinary) => "Event (ordinary)".to_string(),
        Some(EventStatus::Convergent) => "Event (convergent)".to_string(),
        Some(EventStatus::Anticipated) => "Event (anticipated)".to_string(),
        None => "Event".to_string(),
    };

    let range = event
        .span
        .as_ref()
        .map_or_else(default_range, |span| span_to_range(span, source));
    let name_range = event
        .name_span
        .as_ref()
        .map_or_else(default_range, |span| span_to_range(span, source));

    DocumentSymbol {
        name: event.name.clone(),
        detail: Some(detail),
        kind: SymbolKind::FUNCTION,
        tags: None,
        range,
        selection_range: name_range,
        children: if children.is_empty() {
            None
        } else {
            Some(children)
        },
        #[allow(deprecated)]
        deprecated: None,
    }
}

/// Create a simple symbol
fn create_symbol(name: String, kind: SymbolKind, detail: &str, range: Range) -> DocumentSymbol {
    DocumentSymbol {
        name,
        detail: Some(detail.to_string()),
        kind,
        tags: None,
        range,
        selection_range: range,
        children: None,
        #[allow(deprecated)]
        deprecated: None,
    }
}

/// Create a default range (0,0)-(0,0)
/// Used as fallback when span information is not available
fn default_range() -> Range {
    Range {
        start: lsp_types::Position::new(0, 0),
        end: lsp_types::Position::new(0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rossi::{LabeledPredicate, Predicate, parse};

    #[test]
    fn test_extract_context_symbols() {
        let mut ctx = Context::new("test_ctx".to_string());
        ctx.sets = vec![
            rossi::SetDeclaration::Deferred {
                name: "SET1".to_string(),
                comment: None,
            },
            rossi::SetDeclaration::Deferred {
                name: "SET2".to_string(),
                comment: None,
            },
        ];
        ctx.constants = vec![
            rossi::NamedElement::new("const1".to_string()),
            rossi::NamedElement::new("const2".to_string()),
        ];
        ctx.axioms = vec![LabeledPredicate {
            label: Some("axm1".to_string()),
            is_theorem: false,
            predicate: Predicate::True,
            span: None,
            comment: None,
        }];

        let source = "";
        let symbols = extract_context_symbols(&ctx, source);

        assert_eq!(symbols.len(), 1); // One top-level context symbol
        let ctx_symbol = &symbols[0];
        assert_eq!(ctx_symbol.name, "test_ctx");
        assert_eq!(ctx_symbol.kind, SymbolKind::MODULE);

        let children = ctx_symbol.children.as_ref().unwrap();
        assert_eq!(children.len(), 5); // 2 sets + 2 constants + 1 axiom

        // Check sets
        assert_eq!(children[0].name, "SET1");
        assert_eq!(children[0].kind, SymbolKind::ENUM);
        assert_eq!(children[1].name, "SET2");
        assert_eq!(children[1].kind, SymbolKind::ENUM);

        // Check constants
        assert_eq!(children[2].name, "const1");
        assert_eq!(children[2].kind, SymbolKind::CONSTANT);
        assert_eq!(children[3].name, "const2");
        assert_eq!(children[3].kind, SymbolKind::CONSTANT);

        // Check axiom
        assert_eq!(children[4].name, "axm1");
        assert_eq!(children[4].kind, SymbolKind::PROPERTY);
    }

    #[test]
    fn test_extract_machine_symbols() {
        let mut machine = Machine::new("test_machine".to_string());
        machine.variables = vec![
            rossi::NamedElement::new("var1".to_string()),
            rossi::NamedElement::new("var2".to_string()),
        ];
        machine.invariants = vec![LabeledPredicate {
            label: Some("inv1".to_string()),
            is_theorem: false,
            predicate: Predicate::True,
            span: None,
            comment: None,
        }];

        let source = "";
        let symbols = extract_machine_symbols(&machine, source);

        assert_eq!(symbols.len(), 1); // One top-level machine symbol
        let machine_symbol = &symbols[0];
        assert_eq!(machine_symbol.name, "test_machine");
        assert_eq!(machine_symbol.kind, SymbolKind::MODULE);

        let children = machine_symbol.children.as_ref().unwrap();
        assert_eq!(children.len(), 3); // 2 variables + 1 invariant

        // Check variables
        assert_eq!(children[0].name, "var1");
        assert_eq!(children[0].kind, SymbolKind::VARIABLE);
        assert_eq!(children[1].name, "var2");
        assert_eq!(children[1].kind, SymbolKind::VARIABLE);

        // Check invariant
        assert_eq!(children[2].name, "inv1");
        assert_eq!(children[2].kind, SymbolKind::PROPERTY);
    }

    #[test]
    fn test_extract_symbols_from_parsed_context() {
        let source = r#"
        CONTEXT counter_ctx
        SETS
            STATUS
        CONSTANTS
            max_value
        AXIOMS
            @axm1 max_value = 100
        END
        "#;

        let component = parse(source).unwrap();
        let symbols = extract_symbols(&component, source);

        assert_eq!(symbols.len(), 1);
        let ctx_symbol = &symbols[0];
        assert_eq!(ctx_symbol.name, "counter_ctx");

        let children = ctx_symbol.children.as_ref().unwrap();
        assert!(children.iter().any(|s| s.name == "STATUS"));
        assert!(children.iter().any(|s| s.name == "max_value"));
        assert!(children.iter().any(|s| s.name == "axm1"));
    }

    #[test]
    fn test_extract_symbols_from_parsed_machine() {
        let source = r#"
        MACHINE counter
        VARIABLES
            count
        INVARIANTS
            @inv1 count >= 0
        EVENTS
            EVENT INITIALISATION
            THEN
                @act1 count := 0
            END

            EVENT increment
            WHERE
                @grd1 count < 100
            THEN
                @act1 count := count + 1
            END
        END
        "#;

        let component = parse(source).unwrap();
        let symbols = extract_symbols(&component, source);

        assert_eq!(symbols.len(), 1);
        let machine_symbol = &symbols[0];
        assert_eq!(machine_symbol.name, "counter");

        let children = machine_symbol.children.as_ref().unwrap();

        // Should have: count variable, inv1, INITIALISATION, increment event
        assert!(children.iter().any(|s| s.name == "count"));
        assert!(children.iter().any(|s| s.name == "inv1"));
        assert!(children.iter().any(|s| s.name == "INITIALISATION"));
        assert!(children.iter().any(|s| s.name == "increment"));

        // Check increment event has children
        let increment = children.iter().find(|s| s.name == "increment").unwrap();
        assert!(increment.children.is_some());
        let event_children = increment.children.as_ref().unwrap();
        assert!(event_children.iter().any(|s| s.name == "grd1"));
        assert!(event_children.iter().any(|s| s.name == "act1"));
    }

    #[test]
    fn test_event_status_in_detail() {
        let mut ordinary_event = Event::new("evt1".to_string());
        ordinary_event.status = Some(EventStatus::Ordinary);

        let mut convergent_event = Event::new("evt2".to_string());
        convergent_event.status = Some(EventStatus::Convergent);

        let mut anticipated_event = Event::new("evt3".to_string());
        anticipated_event.status = Some(EventStatus::Anticipated);

        let source = "";
        let sym1 = extract_event_symbol(&ordinary_event, source);
        assert_eq!(sym1.detail, Some("Event (ordinary)".to_string()));

        let sym2 = extract_event_symbol(&convergent_event, source);
        assert_eq!(sym2.detail, Some("Event (convergent)".to_string()));

        let sym3 = extract_event_symbol(&anticipated_event, source);
        assert_eq!(sym3.detail, Some("Event (anticipated)".to_string()));
    }
}
