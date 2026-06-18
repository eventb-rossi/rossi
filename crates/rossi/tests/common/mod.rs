//! Shared test utilities for Event-B parser integration tests.

#![allow(dead_code)]

use rossi::{
    Component, Context, Expression, Machine, PredicateKind, parse, to_string, to_string_ascii,
};

/// Clear all spans from a Component for AST comparison (spans differ after roundtrip).
pub fn clear_spans(component: &mut Component) {
    match component {
        Component::Context(ctx) => {
            ctx.span = None;
            ctx.name_span = None;
            // Clause regions are span-derived metadata; offsets shift when the
            // source is reformatted, so drop them for AST comparison.
            ctx.clauses.clear();
            for set in &mut ctx.sets {
                *set.span_mut() = None;
            }
            for constant in &mut ctx.constants {
                constant.span = None;
            }
            for axiom in &mut ctx.axioms {
                axiom.span = None;
            }
        }
        Component::Machine(machine) => {
            machine.span = None;
            machine.name_span = None;
            // Clause regions are span-derived metadata; offsets shift when the
            // source is reformatted, so drop them for AST comparison.
            machine.clauses.clear();
            for var in &mut machine.variables {
                var.span = None;
            }
            for inv in &mut machine.invariants {
                inv.span = None;
            }
            if let Some(init) = &mut machine.initialisation {
                init.span = None;
                init.name_span = None;
                for action in &mut init.actions {
                    action.span = None;
                }
                for lp in init.with.iter_mut().chain(&mut init.witnesses) {
                    lp.span = None;
                }
            }
            for event in &mut machine.events {
                event.span = None;
                event.name_span = None;
                for param in &mut event.parameters {
                    param.span = None;
                }
                for guard in &mut event.guards {
                    guard.span = None;
                }
                for lp in &mut event.with {
                    lp.span = None;
                }
                for lp in &mut event.witnesses {
                    lp.span = None;
                }
                for action in &mut event.actions {
                    action.span = None;
                }
            }
        }
    }
}

/// Roundtrip helper: parse -> pretty-print -> re-parse -> compare ASTs.
pub fn assert_roundtrip(source: &str) {
    let mut component1 = parse(source).unwrap_or_else(|e| panic!("Failed to parse source: {e}"));
    let output = to_string(&component1);
    let mut component2 = parse(&output)
        .unwrap_or_else(|e| panic!("Failed to parse roundtrip output: {e}\nOutput was:\n{output}"));

    clear_spans(&mut component1);
    clear_spans(&mut component2);

    assert_eq!(
        component1, component2,
        "Roundtrip mismatch.\nOriginal source:\n{source}\nPretty-printed:\n{output}"
    );
}

/// Roundtrip helper for ASCII mode: parse -> ASCII print -> re-parse -> compare ASTs.
pub fn assert_roundtrip_ascii(source: &str) {
    let mut component1 = parse(source).unwrap_or_else(|e| panic!("Failed to parse source: {e}"));
    let output = to_string_ascii(&component1);
    let mut component2 = parse(&output).unwrap_or_else(|e| {
        panic!("Failed to parse ASCII roundtrip output: {e}\nOutput was:\n{output}")
    });

    clear_spans(&mut component1);
    clear_spans(&mut component2);

    assert_eq!(
        component1, component2,
        "ASCII roundtrip mismatch.\nOriginal source:\n{source}\nASCII output:\n{output}"
    );
}

/// Parse source and extract the Context, panicking if it's not a Context.
pub fn parse_context(source: &str) -> Context {
    match parse(source).unwrap_or_else(|e| panic!("Failed to parse: {e}")) {
        Component::Context(ctx) => ctx,
        Component::Machine(_) => panic!("Expected Context, got Machine"),
    }
}

/// Parse source and extract the Machine, panicking if it's not a Machine.
pub fn parse_machine(source: &str) -> Machine {
    match parse(source).unwrap_or_else(|e| panic!("Failed to parse: {e}")) {
        Component::Machine(m) => m,
        Component::Context(_) => panic!("Expected Machine, got Context"),
    }
}

/// Parse a Context source and return the RHS expression of the first axiom's comparison.
pub fn parse_axiom_rhs(source: &str) -> Expression {
    let ctx = parse_context(source);
    if let PredicateKind::Comparison { right, .. } = &ctx.axioms[0].predicate.kind {
        return right.clone();
    }
    panic!("Expected Context with comparison axiom");
}

/// Parse a Context source and return the LHS expression of the first axiom's comparison.
pub fn parse_expr_axiom(source: &str) -> Expression {
    let ctx = parse_context(source);
    if let PredicateKind::Comparison { left, .. } = &ctx.axioms[0].predicate.kind {
        return left.clone();
    }
    panic!("Expected Context with comparison axiom");
}

/// Generate a CONTEXT source with given constants and axiom body.
///
/// Example: `axiom_context("x, y, r", "r = x |-> y")` produces:
/// ```text
/// CONTEXT test
/// CONSTANTS
///     x, y, r
/// AXIOMS
///     @axm1 r = x |-> y
/// END
/// ```
pub fn axiom_context(constants: &str, axiom_body: &str) -> String {
    let constants = ws_idents(constants);
    format!("CONTEXT test\nCONSTANTS\n    {constants}\nAXIOMS\n    @axm1 {axiom_body}\nEND\n")
}

/// Generate a MACHINE source with given variables and invariant body.
pub fn invariant_machine(variables: &str, invariant_body: &str) -> String {
    let variables = ws_idents(variables);
    format!(
        "MACHINE test\nVARIABLES\n    {variables}\nINVARIANTS\n    @inv1 {invariant_body}\nEND\n"
    )
}

/// Declared identifiers are whitespace-separated. Accept either spelling from a
/// caller and normalise to whitespace so the generated fixture parses (a comma
/// between declared names is a parse error in the real grammar).
fn ws_idents(idents: &str) -> String {
    idents
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
