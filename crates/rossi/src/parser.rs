//! Rossi implementation
//!
//! This module provides the main parser interface using pest.

use pest::Parser;
use pest_derive::Parser;

use crate::ast::*;
use crate::error::ParseError;

#[derive(Parser)]
#[grammar = "grammar.pest"]
pub struct RossiParser;

/// Parse a typed_identifier rule into a TypedIdentifier
fn parse_typed_identifier(
    pair: pest::iterators::Pair<Rule>,
) -> Result<TypedIdentifier, ParseError> {
    let mut inner = pair.into_inner();
    let name = inner
        .next()
        .ok_or(ParseError::MissingVariable)?
        .as_str()
        .to_string();
    // Skip op_oftype if present, then parse the type expression
    let mut type_expr = None;
    for p in inner {
        match p.as_rule() {
            Rule::op_oftype => {}
            Rule::ident_binder_type => {
                type_expr = Some(Box::new(parse_expression(p)?));
            }
            _ => {}
        }
    }
    Ok(TypedIdentifier { name, type_expr })
}

/// Collect typed identifiers from a quantifier, returning identifiers and the body predicate.
///
/// Shared by `negation_predicate` and `quantified_predicate` handlers.
fn collect_typed_identifiers_and_predicate(
    inner: &mut pest::iterators::Pairs<Rule>,
) -> Result<(Vec<TypedIdentifier>, Predicate), ParseError> {
    let mut identifiers = Vec::new();
    for p in inner.by_ref() {
        match p.as_rule() {
            Rule::typed_identifier => {
                identifiers.push(parse_typed_identifier(p)?);
            }
            Rule::predicate | Rule::predicate_no_semi => {
                let predicate = parse_predicate(p)?;
                return Ok((identifiers, predicate));
            }
            Rule::comma | Rule::dot => {}
            _ => {
                return Err(ParseError::UnexpectedRule {
                    expected: "identifier, predicate, or delimiter".to_string(),
                    found: format!("{:?}", p.as_rule()),
                });
            }
        }
    }
    Err(ParseError::MissingPredicate)
}

/// Extract all identifiers from a clause pair, skipping the keyword
fn collect_identifiers_from_clause(
    pair: pest::iterators::Pair<Rule>,
) -> Result<Vec<String>, ParseError> {
    let mut identifiers = Vec::new();
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::identifier => identifiers.push(p.as_str().to_string()),
            Rule::comma => {} // optional comma separator
            // Skip the leading clause keyword (varies by call site)
            Rule::kw_extends
            | Rule::kw_constants
            | Rule::kw_variables
            | Rule::kw_refines
            | Rule::kw_sees
            | Rule::kw_any => {}
            _ => {
                return Err(ParseError::UnexpectedRule {
                    expected: "identifier or comma".to_string(),
                    found: format!("{:?}", p.as_rule()),
                });
            }
        }
    }
    Ok(identifiers)
}

/// Parse a set declaration (deferred or enumerated)
fn parse_set_declaration(pair: pest::iterators::Pair<Rule>) -> Result<SetDeclaration, ParseError> {
    let mut inner = pair.into_inner();
    let name_pair = inner.next().ok_or(ParseError::MissingVariable)?;
    let name = name_pair.as_str().to_string();

    // Check if there's an '=' followed by enumerated elements
    let mut elements = Vec::new();
    let mut has_eq = false;
    for p in inner {
        match p.as_rule() {
            Rule::op_eq => {
                has_eq = true;
            }
            Rule::identifier => {
                elements.push(p.as_str().to_string());
            }
            Rule::lbrace | Rule::rbrace | Rule::comma => {}
            _ => {
                return Err(ParseError::UnexpectedRule {
                    expected: "set element identifier or delimiter".to_string(),
                    found: format!("{:?}", p.as_rule()),
                });
            }
        }
    }

    if has_eq {
        Ok(SetDeclaration::Enumerated {
            name,
            elements,
            comment: None,
        })
    } else {
        Ok(SetDeclaration::Deferred {
            name,
            comment: None,
        })
    }
}

/// Collect set declarations from a sets clause
fn collect_set_declarations(
    pair: pest::iterators::Pair<Rule>,
) -> Result<Vec<SetDeclaration>, ParseError> {
    let mut declarations = Vec::new();
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::kw_sets => {}
            Rule::set_declaration => {
                declarations.push(parse_set_declaration(p)?);
            }
            Rule::comma => {}
            _ => {
                return Err(ParseError::UnexpectedRule {
                    expected: "set_declaration or comma".to_string(),
                    found: format!("{:?}", p.as_rule()),
                });
            }
        }
    }
    Ok(declarations)
}

/// Collect labeled predicates from a clause pair, skipping the keyword
fn collect_labeled_predicates(
    pair: pest::iterators::Pair<Rule>,
    keyword_rule: Rule,
) -> Result<Vec<LabeledPredicate>, ParseError> {
    let mut predicates = Vec::new();
    for p in pair.into_inner() {
        if p.as_rule() == keyword_rule {
            continue;
        }
        if p.as_rule() == Rule::labeled_predicate {
            predicates.push(parse_labeled_predicate(p)?);
        } else {
            return Err(ParseError::UnexpectedRule {
                expected: "labeled_predicate".to_string(),
                found: format!("{:?}", p.as_rule()),
            });
        }
    }
    Ok(predicates)
}

/// Validate clause ordering within a context or machine body.
/// Returns an error if a clause appears out of order or is duplicated.
fn validate_clause_order(
    rule: Rule,
    span: pest::Span,
    last_order: &mut Option<(usize, Rule)>,
    order_fn: fn(Rule) -> Option<usize>,
    name_fn: fn(Rule) -> &'static str,
) -> Result<(), ParseError> {
    if let Some(order) = order_fn(rule) {
        let name = name_fn(rule);
        let (line, col) = span.start_pos().line_col();
        if let Some((prev_order, prev_rule)) = *last_order {
            if order == prev_order {
                return Err(ParseError::ClauseError {
                    clause_type: name.to_string(),
                    line,
                    column: col,
                    message: format!("Duplicate {} clause", name),
                });
            } else if order < prev_order {
                return Err(ParseError::ClauseError {
                    clause_type: name.to_string(),
                    line,
                    column: col,
                    message: format!(
                        "{} clause must appear before {} clause",
                        name,
                        name_fn(prev_rule)
                    ),
                });
            }
        }
        *last_order = Some((order, rule));
    }
    Ok(())
}

/// Extract the label string from a `label` grammar rule pair
fn extract_label(pair: pest::iterators::Pair<Rule>) -> Option<String> {
    pair.into_inner().next().and_then(|label_inner| {
        if label_inner.as_rule() == Rule::label_text {
            let text = label_inner.as_str();
            // Strip optional trailing colon (for eventb-to-txt compat)
            Some(text.trim_end_matches(':').to_string())
        } else {
            None
        }
    })
}

/// Parse an Event-B component (Context or Machine) from source text
pub fn parse(input: &str) -> Result<Component, ParseError> {
    let pairs = RossiParser::parse(Rule::component, input)
        .map_err(|e| ParseError::PestError(e.to_string()))?;

    let component_pair = pairs
        .into_iter()
        .next()
        .ok_or_else(|| ParseError::UnexpectedRule {
            expected: "component".to_string(),
            found: "empty parse result".to_string(),
        })?;
    let inner = component_pair
        .into_inner()
        .next()
        .ok_or_else(|| ParseError::UnexpectedRule {
            expected: "context or machine".to_string(),
            found: "empty component".to_string(),
        })?;

    match inner.as_rule() {
        Rule::context => parse_context(inner),
        Rule::machine => parse_machine(inner),
        _ => Err(ParseError::UnexpectedRule {
            expected: "context or machine".to_string(),
            found: format!("{:?}", inner.as_rule()),
        }),
    }
}

/// Parse one or more Event-B components (Contexts and/or Machines) from source text.
///
/// This is the multi-component counterpart of [`parse`]. Files produced by
/// `rossi import --merge` or the reference `eventb-to-txt` tool may contain
/// several `CONTEXT` and `MACHINE` blocks concatenated in a single file.
///
/// Returns `Ok(Vec<Component>)` with one entry per parsed component.
pub fn parse_components(input: &str) -> Result<Vec<Component>, ParseError> {
    let pairs = RossiParser::parse(Rule::components, input)
        .map_err(|e| ParseError::PestError(e.to_string()))?;

    let components_pair = pairs
        .into_iter()
        .next()
        .ok_or_else(|| ParseError::UnexpectedRule {
            expected: "components".to_string(),
            found: "empty parse result".to_string(),
        })?;

    let mut result = Vec::new();
    for inner in components_pair.into_inner() {
        match inner.as_rule() {
            Rule::context => result.push(parse_context(inner)?),
            Rule::machine => result.push(parse_machine(inner)?),
            Rule::EOI => {}
            _ => {
                return Err(ParseError::UnexpectedRule {
                    expected: "context or machine".to_string(),
                    found: format!("{:?}", inner.as_rule()),
                });
            }
        }
    }

    Ok(result)
}

/// Return the canonical ordering index for a context clause rule.
/// EXTENDS=0, SETS=1, CONSTANTS=2, AXIOMS=3
fn context_clause_order(rule: Rule) -> Option<usize> {
    match rule {
        Rule::context_clause_extends => Some(0),
        Rule::context_clause_sets => Some(1),
        Rule::context_clause_constants => Some(2),
        Rule::context_clause_axioms => Some(3),
        _ => None,
    }
}

/// Return the human-readable name for a context clause rule.
fn context_clause_name(rule: Rule) -> &'static str {
    match rule {
        Rule::context_clause_extends => "EXTENDS",
        Rule::context_clause_sets => "SETS",
        Rule::context_clause_constants => "CONSTANTS",
        Rule::context_clause_axioms => "AXIOMS",
        _ => "unknown",
    }
}

/// Return the canonical ordering index for a machine clause rule.
/// REFINES=0, SEES=1, VARIABLES=2, INVARIANTS=3, VARIANT=4, EVENTS=5
fn machine_clause_order(rule: Rule) -> Option<usize> {
    match rule {
        Rule::machine_clause_refines => Some(0),
        Rule::machine_clause_sees => Some(1),
        Rule::machine_clause_variables => Some(2),
        Rule::machine_clause_invariants => Some(3),
        Rule::machine_clause_variant => Some(4),
        Rule::machine_clause_events => Some(5),
        _ => None,
    }
}

/// Return the human-readable name for a machine clause rule.
fn machine_clause_name(rule: Rule) -> &'static str {
    match rule {
        Rule::machine_clause_refines => "REFINES",
        Rule::machine_clause_sees => "SEES",
        Rule::machine_clause_variables => "VARIABLES",
        Rule::machine_clause_invariants => "INVARIANTS",
        Rule::machine_clause_variant => "VARIANT",
        Rule::machine_clause_events => "EVENTS",
        _ => "unknown",
    }
}

/// Parse a Context component
fn parse_context(pair: pest::iterators::Pair<Rule>) -> Result<Component, ParseError> {
    let context_span = Span::from_pest(pair.as_span());
    let mut context = Context::new(String::new());
    let mut inner = pair.into_inner();

    // Skip kw_context (validated)
    match inner.next().map(|p| p.as_rule()) {
        Some(Rule::kw_context) => {}
        other => {
            return Err(ParseError::UnexpectedRule {
                expected: "CONTEXT keyword".to_string(),
                found: format!("{:?}", other),
            });
        }
    }

    // Parse name
    if let Some(name_pair) = inner.next() {
        context.name = name_pair.as_str().to_string();
        context.name_span = Some(Span::from_pest(name_pair.as_span()));
    }

    // Parse context body - flatten if wrapped
    let mut last_order: Option<(usize, Rule)> = None;
    for pair in inner {
        let pairs_to_process = if pair.as_rule() == Rule::context_body {
            pair.into_inner().collect::<Vec<_>>()
        } else {
            vec![pair]
        };

        for pair in pairs_to_process {
            validate_clause_order(
                pair.as_rule(),
                pair.as_span(),
                &mut last_order,
                context_clause_order,
                context_clause_name,
            )?;

            match pair.as_rule() {
                Rule::context_clause_extends => {
                    context
                        .extends
                        .extend(collect_identifiers_from_clause(pair)?);
                }
                Rule::context_clause_sets => {
                    context.sets.extend(collect_set_declarations(pair)?);
                }
                Rule::context_clause_constants => {
                    context.constants.extend(
                        collect_identifiers_from_clause(pair)?
                            .into_iter()
                            .map(NamedElement::new),
                    );
                }
                Rule::context_clause_axioms => {
                    context
                        .axioms
                        .extend(collect_labeled_predicates(pair, Rule::kw_axioms)?);
                }
                Rule::kw_end => break,
                _ => {
                    return Err(ParseError::UnexpectedRule {
                        expected: "context clause or END".to_string(),
                        found: format!("{:?}", pair.as_rule()),
                    });
                }
            }
        }
    }

    context.span = Some(context_span);
    Ok(Component::Context(context))
}

/// Parse a Machine component
fn parse_machine(pair: pest::iterators::Pair<Rule>) -> Result<Component, ParseError> {
    let machine_span = Span::from_pest(pair.as_span());
    let mut machine = Machine::new(String::new());
    let mut inner = pair.into_inner();

    // Skip kw_machine (validated)
    match inner.next().map(|p| p.as_rule()) {
        Some(Rule::kw_machine) => {}
        other => {
            return Err(ParseError::UnexpectedRule {
                expected: "MACHINE keyword".to_string(),
                found: format!("{:?}", other),
            });
        }
    }

    // Parse name
    if let Some(name_pair) = inner.next() {
        machine.name = name_pair.as_str().to_string();
        machine.name_span = Some(Span::from_pest(name_pair.as_span()));
    }

    // Parse machine body - flatten if wrapped
    let mut last_order: Option<(usize, Rule)> = None;
    for pair in inner {
        let pairs_to_process = if pair.as_rule() == Rule::machine_body {
            pair.into_inner().collect::<Vec<_>>()
        } else {
            vec![pair]
        };

        for pair in pairs_to_process {
            validate_clause_order(
                pair.as_rule(),
                pair.as_span(),
                &mut last_order,
                machine_clause_order,
                machine_clause_name,
            )?;

            match pair.as_rule() {
                Rule::machine_clause_refines => {
                    machine.refines = collect_identifiers_from_clause(pair)?.into_iter().next();
                }
                Rule::machine_clause_sees => {
                    machine.sees.extend(collect_identifiers_from_clause(pair)?);
                }
                Rule::machine_clause_variables => {
                    machine.variables.extend(
                        collect_identifiers_from_clause(pair)?
                            .into_iter()
                            .map(NamedElement::new),
                    );
                }
                Rule::machine_clause_invariants => {
                    machine
                        .invariants
                        .extend(collect_labeled_predicates(pair, Rule::kw_invariants)?);
                }
                Rule::machine_clause_variant => {
                    for vp in pair.into_inner() {
                        match vp.as_rule() {
                            Rule::kw_variant => {}
                            _ => {
                                machine.variant = Some(parse_expression(vp)?);
                            }
                        }
                    }
                }
                Rule::machine_clause_events => {
                    for event_pair in pair.into_inner() {
                        match event_pair.as_rule() {
                            Rule::kw_events => {}
                            Rule::initialisation_event => {
                                machine.initialisation =
                                    Some(parse_initialisation_event(event_pair)?);
                            }
                            Rule::event => {
                                machine.events.push(parse_event(event_pair)?);
                            }
                            _ => {
                                return Err(ParseError::UnexpectedRule {
                                    expected: "event or initialisation_event".to_string(),
                                    found: format!("{:?}", event_pair.as_rule()),
                                });
                            }
                        }
                    }
                }
                Rule::kw_end => break,
                _ => {
                    return Err(ParseError::UnexpectedRule {
                        expected: "machine clause or END".to_string(),
                        found: format!("{:?}", pair.as_rule()),
                    });
                }
            }
        }
    }

    machine.span = Some(machine_span);
    Ok(Component::Machine(machine))
}

/// Parse a labeled predicate
fn parse_labeled_predicate(
    pair: pest::iterators::Pair<Rule>,
) -> Result<LabeledPredicate, ParseError> {
    let span = Span::from_pest(pair.as_span());
    let inner = pair.into_inner();
    let mut label = None;
    let mut is_theorem = false;
    let mut predicate = None;

    for p in inner {
        match p.as_rule() {
            Rule::label => {
                label = extract_label(p);
            }
            Rule::kw_theorem => {
                is_theorem = true;
            }
            Rule::predicate => {
                predicate = Some(parse_predicate(p)?);
            }
            _ => {
                return Err(ParseError::UnexpectedRule {
                    expected: "label, theorem keyword, or predicate".to_string(),
                    found: format!("{:?}", p.as_rule()),
                });
            }
        }
    }

    Ok(LabeledPredicate {
        label,
        is_theorem,
        predicate: predicate.ok_or(ParseError::MissingPredicate)?,
        span: Some(span),
        comment: None,
    })
}

/// Parse an event
fn parse_event(pair: pest::iterators::Pair<Rule>) -> Result<Event, ParseError> {
    let event_span = Span::from_pest(pair.as_span());
    let mut event = Event::new(String::new());
    let mut inner = pair.into_inner();

    // Check for optional inline status (before EVENT keyword)
    // e.g. "convergent event foo ..."
    if let Some(peek) = inner.peek()
        && peek.as_rule() == Rule::event_inline_status
    {
        let status_pair = inner.next().expect("peek confirmed element exists");
        for sp in status_pair.into_inner() {
            match sp.as_rule() {
                Rule::kw_ordinary => event.status = Some(EventStatus::Ordinary),
                Rule::kw_convergent => event.status = Some(EventStatus::Convergent),
                Rule::kw_anticipated => event.status = Some(EventStatus::Anticipated),
                _ => {}
            }
        }
    }

    // Skip kw_event (validated)
    match inner.next().map(|p| p.as_rule()) {
        Some(Rule::kw_event) => {}
        other => {
            return Err(ParseError::UnexpectedRule {
                expected: "EVENT keyword".to_string(),
                found: format!("{:?}", other),
            });
        }
    }

    // Parse name
    if let Some(name_pair) = inner.next() {
        event.name = name_pair.as_str().to_string();
        event.name_span = Some(Span::from_pest(name_pair.as_span()));
    }

    // Check for optional `extends identifier` or `refines identifier` before event_body
    if let Some(peek) = inner.peek() {
        if peek.as_rule() == Rule::kw_extends {
            inner.next(); // consume kw_extends
            if let Some(parent_pair) = inner.next() {
                event.extended = true;
                event.refines = Some(parent_pair.as_str().to_string());
            }
        } else if peek.as_rule() == Rule::kw_refines {
            inner.next(); // consume kw_refines
            if let Some(parent_pair) = inner.next() {
                event.refines = Some(parent_pair.as_str().to_string());
            }
        }
    }

    // Parse event body - flatten if wrapped
    for pair in inner {
        let pairs_to_process = if pair.as_rule() == Rule::event_body {
            pair.into_inner().collect::<Vec<_>>()
        } else {
            vec![pair]
        };

        for pair in pairs_to_process {
            match pair.as_rule() {
                Rule::event_status => {
                    for status_pair in pair.into_inner() {
                        match status_pair.as_rule() {
                            Rule::kw_status => {}
                            Rule::kw_ordinary => event.status = Some(EventStatus::Ordinary),
                            Rule::kw_convergent => event.status = Some(EventStatus::Convergent),
                            Rule::kw_anticipated => event.status = Some(EventStatus::Anticipated),
                            _ => {
                                return Err(ParseError::UnexpectedRule {
                                    expected: "ordinary, convergent, or anticipated".to_string(),
                                    found: format!("{:?}", status_pair.as_rule()),
                                });
                            }
                        }
                    }
                }
                Rule::event_refines => {
                    event.refines = collect_identifiers_from_clause(pair)?.into_iter().next();
                }
                Rule::event_any => {
                    event.parameters.extend(
                        collect_identifiers_from_clause(pair)?
                            .into_iter()
                            .map(NamedElement::new),
                    );
                }
                Rule::event_where => {
                    for labeled_pred_pair in pair.into_inner() {
                        match labeled_pred_pair.as_rule() {
                            Rule::kw_where | Rule::kw_when => {}
                            Rule::labeled_predicate => {
                                event
                                    .guards
                                    .push(parse_labeled_predicate(labeled_pred_pair)?);
                            }
                            _ => {
                                return Err(ParseError::UnexpectedRule {
                                    expected: "labeled_predicate".to_string(),
                                    found: format!("{:?}", labeled_pred_pair.as_rule()),
                                });
                            }
                        }
                    }
                }
                Rule::event_with => {
                    event
                        .with
                        .extend(collect_labeled_predicates(pair, Rule::kw_with)?);
                }
                Rule::event_witness => {
                    event
                        .witnesses
                        .extend(collect_labeled_predicates(pair, Rule::kw_witness)?);
                }
                Rule::event_then => {
                    for tp in pair.into_inner() {
                        match tp.as_rule() {
                            Rule::kw_then | Rule::kw_begin => {}
                            Rule::action_list => {
                                event.actions = parse_action_list(tp)?;
                            }
                            _ => {
                                return Err(ParseError::UnexpectedRule {
                                    expected: "THEN/BEGIN keyword or action_list".to_string(),
                                    found: format!("{:?}", tp.as_rule()),
                                });
                            }
                        }
                    }
                }
                Rule::kw_end => break,
                _ => {
                    return Err(ParseError::UnexpectedRule {
                        expected: "event clause or END".to_string(),
                        found: format!("{:?}", pair.as_rule()),
                    });
                }
            }
        }
    }

    event.span = Some(event_span);
    Ok(event)
}

/// Parse an initialisation event
fn parse_initialisation_event(
    pair: pest::iterators::Pair<Rule>,
) -> Result<InitialisationEvent, ParseError> {
    let mut actions = Vec::new();
    let mut extended = false;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::action_list => {
                actions = parse_action_list(p)?;
            }
            Rule::kw_extends => {
                extended = true;
            }
            Rule::kw_event | Rule::kw_initialisation | Rule::kw_then | Rule::kw_end => {}
            _ => {
                return Err(ParseError::UnexpectedRule {
                    expected: "action_list or keyword".to_string(),
                    found: format!("{:?}", p.as_rule()),
                });
            }
        }
    }

    Ok(InitialisationEvent {
        actions,
        comment: None,
        extended,
        with: Vec::new(),
        witnesses: Vec::new(),
    })
}

/// Parse an action list
fn parse_action_list(pair: pest::iterators::Pair<Rule>) -> Result<Vec<LabeledAction>, ParseError> {
    let mut actions = Vec::new();

    for action_pair in pair.into_inner() {
        match action_pair.as_rule() {
            Rule::labeled_action => {
                actions.push(parse_labeled_action(action_pair)?);
            }
            _ => {
                return Err(ParseError::UnexpectedRule {
                    expected: "labeled_action".to_string(),
                    found: format!("{:?}", action_pair.as_rule()),
                });
            }
        }
    }

    Ok(actions)
}

/// Parse a single action (supports multiple variables: x, y := e1, e2)
fn parse_action(pair: pest::iterators::Pair<Rule>) -> Result<Action, ParseError> {
    // Peek at the first inner token: if it is kw_skip this is a skip action.
    let mut inner = pair.into_inner().peekable();
    if inner.peek().map(|p| p.as_rule()) == Some(Rule::kw_skip) {
        return Ok(Action::Skip);
    }
    let inner = inner;

    let mut variables = Vec::new();
    let mut op: Option<pest::iterators::Pair<Rule>> = None;
    let mut rhs_pairs = Vec::new();
    let mut is_func_override = false;
    let mut func_arg_pairs = Vec::new();

    for p in inner {
        match p.as_rule() {
            Rule::identifier if op.is_none() && !is_func_override => {
                variables.push(p.as_str().to_string());
            }
            Rule::comma if op.is_none() => {} // separator between LHS identifiers/arguments
            Rule::lparen if op.is_none() => {
                is_func_override = true;
            }
            Rule::rparen if op.is_none() => {} // closing paren of function override LHS
            _ if op.is_none()
                && is_func_override
                && p.as_rule() != Rule::op_becomes_equal
                && p.as_rule() != Rule::op_becomes_in
                && p.as_rule() != Rule::op_becomes_such =>
            {
                func_arg_pairs.push(p);
            }
            Rule::op_becomes_equal | Rule::op_becomes_in | Rule::op_becomes_such => {
                op = Some(p);
            }
            Rule::comma if op.is_some() => {} // separator between RHS expressions
            _ => {
                rhs_pairs.push(p);
            }
        }
    }

    let op_pair = op.ok_or(ParseError::MissingOperator)?;

    if is_func_override {
        // Function override: f(x, ...) ≔ E
        let function = variables
            .into_iter()
            .next()
            .ok_or(ParseError::MissingValue)?;
        let mut arguments = Vec::new();
        for arg in func_arg_pairs {
            arguments.push(parse_expression(arg)?);
        }
        let rhs = rhs_pairs
            .into_iter()
            .next()
            .ok_or(ParseError::MissingValue)?;
        let expression = parse_expression(rhs)?;
        return Ok(Action::FunctionOverride {
            function,
            arguments,
            expression,
        });
    }

    match op_pair.as_rule() {
        Rule::op_becomes_equal => {
            let mut expressions = Vec::new();
            for rhs in rhs_pairs {
                expressions.push(parse_expression(rhs)?);
            }
            if expressions.is_empty() {
                return Err(ParseError::MissingValue);
            }
            Ok(Action::Assignment {
                variables,
                expressions,
            })
        }
        Rule::op_becomes_in => {
            let rhs = rhs_pairs
                .into_iter()
                .next()
                .ok_or(ParseError::MissingValue)?;
            let set = parse_expression(rhs)?;
            Ok(Action::BecomesIn { variables, set })
        }
        Rule::op_becomes_such => {
            let rhs = rhs_pairs
                .into_iter()
                .next()
                .ok_or(ParseError::MissingValue)?;
            let predicate = parse_predicate(rhs)?;
            Ok(Action::BecomesSuchThat {
                variables,
                predicate,
            })
        }
        _ => Err(ParseError::UnexpectedRule {
            expected: "assignment operator".to_string(),
            found: format!("{:?}", op_pair.as_rule()),
        }),
    }
}

/// Parse a labeled action
fn parse_labeled_action(pair: pest::iterators::Pair<Rule>) -> Result<LabeledAction, ParseError> {
    use crate::ast::LabeledAction;

    let span = Span::from_pest(pair.as_span());
    let inner = pair.into_inner();
    let mut label = None;
    let mut action = None;

    for p in inner {
        match p.as_rule() {
            Rule::label => {
                label = extract_label(p);
            }
            Rule::action => {
                action = Some(parse_action(p)?);
            }
            _ => {
                return Err(ParseError::UnexpectedRule {
                    expected: "label or action".to_string(),
                    found: format!("{:?}", p.as_rule()),
                });
            }
        }
    }

    Ok(LabeledAction {
        label,
        action: action.ok_or(ParseError::MissingAction)?,
        span: Some(span),
        comment: None,
    })
}

/// Map a grammar operator rule to a BinaryOp
fn rule_to_binary_op(rule: Rule) -> Option<crate::ast::expression::BinaryOp> {
    use crate::ast::expression::BinaryOp;
    match rule {
        // Additive operators
        Rule::op_plus => Some(BinaryOp::Add),
        Rule::op_minus => Some(BinaryOp::Subtract),
        Rule::op_union => Some(BinaryOp::Union),
        Rule::op_difference => Some(BinaryOp::Difference),
        // Multiplicative operators
        Rule::op_multiply => Some(BinaryOp::Multiply),
        Rule::op_divide => Some(BinaryOp::Divide),
        Rule::op_modulo => Some(BinaryOp::Modulo),
        Rule::op_cartesian => Some(BinaryOp::CartesianProduct),
        Rule::op_intersection => Some(BinaryOp::Intersection),
        Rule::op_composition => Some(BinaryOp::Composition),
        Rule::op_semicolon => Some(BinaryOp::Semicolon),
        // Relational/range operator
        Rule::op_range_op => Some(BinaryOp::Range),
        // Domain/range restriction and subtraction
        Rule::op_domain_restrict => Some(BinaryOp::DomainRestriction),
        Rule::op_domain_subtract => Some(BinaryOp::DomainSubtraction),
        Rule::op_range_restrict => Some(BinaryOp::RangeRestriction),
        Rule::op_range_subtract => Some(BinaryOp::RangeSubtraction),
        // Overwrite
        Rule::op_overwrite => Some(BinaryOp::Overwrite),
        // Direct and parallel product
        Rule::op_direct_product => Some(BinaryOp::DirectProduct),
        Rule::op_parallel_product => Some(BinaryOp::ParallelProduct),
        // Exponent
        Rule::op_exponent => Some(BinaryOp::Exponent),
        // Maplet
        Rule::op_maplet => Some(BinaryOp::Maplet),
        // Relation/function type operators
        Rule::op_relation => Some(BinaryOp::Relation),
        Rule::op_total_relation => Some(BinaryOp::TotalRelation),
        Rule::op_surjective_relation => Some(BinaryOp::SurjectiveRelation),
        Rule::op_total_surjective_relation => Some(BinaryOp::TotalSurjectiveRelation),
        Rule::op_total_fn => Some(BinaryOp::TotalFunction),
        Rule::op_partial_fn => Some(BinaryOp::PartialFunction),
        Rule::op_total_inj => Some(BinaryOp::TotalInjection),
        Rule::op_partial_inj => Some(BinaryOp::PartialInjection),
        Rule::op_total_surj => Some(BinaryOp::TotalSurjection),
        Rule::op_partial_surj => Some(BinaryOp::PartialSurjection),
        Rule::op_bijection => Some(BinaryOp::Bijection),
        Rule::op_oftype => Some(BinaryOp::OfType),
        _ => None,
    }
}

/// Map a grammar operator rule to a UnaryOp
fn rule_to_unary_op(rule: Rule) -> Option<crate::ast::expression::UnaryOp> {
    use crate::ast::expression::UnaryOp;
    match rule {
        Rule::op_minus => Some(UnaryOp::Minus),
        Rule::op_powerset => Some(UnaryOp::PowerSet),
        Rule::op_powerset1 => Some(UnaryOp::PowerSet1),
        Rule::op_domain => Some(UnaryOp::Domain),
        Rule::op_range => Some(UnaryOp::Range),
        Rule::op_inverse => Some(UnaryOp::Inverse),
        _ => None,
    }
}

/// Parse a binary expression (additive, multiplicative, or relational)
fn parse_binary_expr(pair: pest::iterators::Pair<Rule>) -> Result<Expression, ParseError> {
    let mut inner = pair.into_inner();

    // Get the first operand
    let first = inner.next().ok_or(ParseError::EmptyExpression)?;
    let mut left = parse_expression(first)?;

    // Process remaining (operator, operand) pairs
    while let Some(op_pair) = inner.next() {
        let op =
            rule_to_binary_op(op_pair.as_rule()).ok_or_else(|| ParseError::UnexpectedRule {
                expected: "binary operator".to_string(),
                found: format!("{:?}", op_pair.as_rule()),
            })?;
        let right_pair = inner.next().ok_or(ParseError::EmptyExpression)?;
        let right = parse_expression(right_pair)?;
        left = Expression::Binary {
            op,
            left: Box::new(left),
            right: Box::new(right),
        };
    }

    Ok(left)
}

/// Parse an expression
///
/// The grammar's expression precedence chain (`expression` →
/// `relation_type_expr` → `maplet_expr` → `set_operator_expr` →
/// `relational_expr` → `additive_expr` → `multiplicative_expr` →
/// `exponent_expr` → `unary_expr` → `function_application` → `primary_expr`,
/// plus `_no_semi` twins for actions) produces a deeply-nested Pair tree even
/// for leaf identifiers. Recursing through every wrapper level burns one
/// stack frame per level per leaf, which overflows on deeply-nested formulas
/// (e.g. file-system's `C ∖ {x ↦ y ∣ y ∈ dom(f(x))}[C] ≠ ∅`). Instead we
/// unwrap single-child wrappers iteratively below, only recursing for actual
/// structural work.
fn parse_expression(pair: pest::iterators::Pair<Rule>) -> Result<Expression, ParseError> {
    let mut pair = pair;
    let rule = loop {
        let r = pair.as_rule();
        match r {
            // Always single-child by grammar — pure passthrough.
            Rule::expression | Rule::action_expression => {
                pair = pair
                    .into_inner()
                    .next()
                    .ok_or(ParseError::EmptyExpression)?;
            }
            // Binary precedence wrappers. Single child = no operator at this
            // level (descend); multi-child = real chain (dispatch).
            Rule::relation_type_expr
            | Rule::relation_type_expr_no_semi
            | Rule::ident_binder_type
            | Rule::maplet_expr
            | Rule::maplet_expr_no_semi
            | Rule::set_operator_expr
            | Rule::set_operator_expr_no_semi
            | Rule::relational_expr
            | Rule::relational_expr_no_semi
            | Rule::exponent_expr
            | Rule::exponent_expr_no_semi
            | Rule::additive_expr
            | Rule::additive_expr_no_semi
            | Rule::multiplicative_expr
            | Rule::multiplicative_expr_no_semi => {
                let mut probe = pair.clone().into_inner();
                let first = probe.next().ok_or(ParseError::EmptyExpression)?;
                if probe.next().is_some() {
                    return parse_binary_expr(pair);
                }
                pair = first;
            }
            // Unary wrapper. Prefix op present = real unary (handled below);
            // otherwise = passthrough to function_application.
            Rule::unary_expr => {
                let mut probe = pair.clone().into_inner();
                let first = probe.next().ok_or(ParseError::EmptyExpression)?;
                if rule_to_unary_op(first.as_rule()).is_some() {
                    break r;
                }
                pair = first;
            }
            _ => break r,
        }
    };
    let span = pair.as_span();

    match rule {
        Rule::quantified_union_expr
        | Rule::quantified_inter_expr
        | Rule::quantified_union_expr_no_semi
        | Rule::quantified_inter_expr_no_semi => {
            // Quantified union/inter: kw ~ typed_identifier ~ (comma ~ typed_identifier)* ~ dot ~ predicate ~ pipe ~ expression
            let is_union =
                rule == Rule::quantified_union_expr || rule == Rule::quantified_union_expr_no_semi;
            let mut inner = pair.into_inner();
            let mut identifiers = Vec::new();

            // Collect typed identifiers until we hit the predicate
            for p in inner.by_ref() {
                match p.as_rule() {
                    Rule::typed_identifier => {
                        identifiers.push(parse_typed_identifier(p)?);
                    }
                    Rule::predicate => {
                        let predicate = parse_predicate(p)?;
                        // Skip pipe token, then get expression
                        let expr_pair = loop {
                            let next = inner.next().ok_or(ParseError::EmptyExpression)?;
                            match next.as_rule() {
                                Rule::expression | Rule::action_expression => break next,
                                Rule::pipe => continue,
                                _ => {
                                    return Err(ParseError::UnexpectedRule {
                                        expected: "expression or pipe".to_string(),
                                        found: format!("{:?}", next.as_rule()),
                                    });
                                }
                            }
                        };
                        let expression = parse_expression(expr_pair)?;
                        return if is_union {
                            Ok(Expression::QuantifiedUnion {
                                identifiers,
                                predicate: Box::new(predicate),
                                expression: Box::new(expression),
                            })
                        } else {
                            Ok(Expression::QuantifiedInter {
                                identifiers,
                                predicate: Box::new(predicate),
                                expression: Box::new(expression),
                            })
                        };
                    }
                    Rule::kw_UNION | Rule::kw_INTER | Rule::comma | Rule::dot => {}
                    _ => {
                        return Err(ParseError::UnexpectedRule {
                            expected: "identifier, predicate, or delimiter".to_string(),
                            found: format!("{:?}", p.as_rule()),
                        });
                    }
                }
            }
            Err(ParseError::MissingPredicate)
        }
        Rule::unary_expr => {
            // Reached only when the iterative loop above detected a real
            // prefix unary operator at this level.
            let mut inner = pair.into_inner();
            let first = inner.next().ok_or(ParseError::EmptyExpression)?;
            let op =
                rule_to_unary_op(first.as_rule()).ok_or_else(|| ParseError::UnexpectedRule {
                    expected: "unary operator".to_string(),
                    found: format!("{:?}", first.as_rule()),
                })?;
            let operand = parse_expression(inner.next().ok_or(ParseError::EmptyExpression)?)?;
            Ok(Expression::Unary {
                op,
                operand: Box::new(operand),
            })
        }
        Rule::lambda_expr | Rule::lambda_expr_no_semi => {
            // Lambda expression: λ pattern · P ∣ E
            // Grammar: ("λ" | "%") ~ ident_pattern ~ dot ~ predicate ~ pipe ~ expression
            let mut inner = pair.into_inner();
            let mut pattern = None;
            let mut predicate = None;

            for p in inner.by_ref() {
                match p.as_rule() {
                    Rule::ident_pattern => {
                        pattern = Some(parse_ident_pattern(p)?);
                    }
                    Rule::predicate => {
                        predicate = Some(parse_predicate(p)?);
                        break;
                    }
                    Rule::dot => {}
                    _ => {}
                }
            }

            let pattern = pattern.ok_or(ParseError::EmptyExpression)?;
            let predicate = predicate.ok_or(ParseError::MissingPredicate)?;

            // Skip the pipe token, then get the expression
            let expr_pair = loop {
                let next = inner.next().ok_or(ParseError::EmptyExpression)?;
                match next.as_rule() {
                    Rule::expression | Rule::action_expression => break next,
                    Rule::pipe => continue,
                    _ => {
                        return Err(ParseError::UnexpectedRule {
                            expected: "expression or pipe".to_string(),
                            found: format!("{:?}", next.as_rule()),
                        });
                    }
                }
            };
            let expression = parse_expression(expr_pair)?;
            Ok(Expression::Lambda {
                pattern,
                predicate: Box::new(predicate),
                expression: Box::new(expression),
            })
        }
        Rule::function_application => {
            let mut inner = pair.into_inner();

            // Parse the base expression (function)
            let base = parse_expression(inner.next().ok_or(ParseError::EmptyExpression)?)?;

            // Check if there are any function applications or relational images
            let remaining: Vec<_> = inner.collect();
            if remaining.is_empty() {
                return Ok(base);
            }

            // Parse function applications and relational images
            let mut result = base;
            let mut i = 0;
            while i < remaining.len() {
                if remaining[i].as_rule() == Rule::lparen {
                    i += 1; // Skip lparen
                    let mut arguments = Vec::new();

                    // Collect arguments until we hit rparen
                    while i < remaining.len() && remaining[i].as_rule() != Rule::rparen {
                        match remaining[i].as_rule() {
                            Rule::expression => {
                                arguments.push(parse_expression(remaining[i].clone())?);
                            }
                            Rule::comma => {}
                            _ => {
                                return Err(ParseError::UnexpectedRule {
                                    expected: "expression or comma".to_string(),
                                    found: format!("{:?}", remaining[i].as_rule()),
                                });
                            }
                        }
                        i += 1;
                    }

                    i += 1; // Skip rparen

                    // Check if base is a known built-in function
                    if let Expression::Identifier(ref name) = result
                        && let Some(builtin) =
                            crate::ast::expression::BuiltinFunction::from_name(name)
                    {
                        if !builtin.check_arity(arguments.len()) {
                            return Err(ParseError::ArityMismatch {
                                name: builtin.name().to_string(),
                                expected: if builtin.min_arity() == builtin.max_arity() {
                                    builtin.min_arity().to_string()
                                } else {
                                    format!("{} to {}", builtin.min_arity(), builtin.max_arity())
                                },
                                actual: arguments.len(),
                            });
                        }
                        result = Expression::BuiltinApplication {
                            function: builtin,
                            arguments,
                        };
                        continue;
                    }
                    result = Expression::FunctionApplication {
                        function: Box::new(result),
                        arguments,
                    };
                } else if remaining[i].as_rule() == Rule::lbracket {
                    i += 1; // Skip lbracket
                    // Relational image: r[S]
                    if i < remaining.len() && remaining[i].as_rule() == Rule::expression {
                        let set = parse_expression(remaining[i].clone())?;
                        i += 1;
                        result = Expression::RelationalImage {
                            relation: Box::new(result),
                            set: Box::new(set),
                        };
                    }
                    // Skip rbracket
                    if i < remaining.len() && remaining[i].as_rule() == Rule::rbracket {
                        i += 1;
                    }
                } else if remaining[i].as_rule() == Rule::lbrace {
                    // Function update: f{x ↦ y, ...} == f ⊕ {x ↦ y, ...}.
                    // Rodin's static checker can emit this compact form for
                    // f(x) := y actions; we lower it to the same AST as the
                    // explicit ⊕ operator so semantic comparison converges.
                    i += 1; // skip lbrace
                    let mut elements = Vec::new();
                    while i < remaining.len() && remaining[i].as_rule() != Rule::rbrace {
                        match remaining[i].as_rule() {
                            Rule::expression => {
                                elements.push(parse_expression(remaining[i].clone())?);
                            }
                            Rule::comma => {}
                            _ => {
                                return Err(ParseError::UnexpectedRule {
                                    expected: "expression or comma".to_string(),
                                    found: format!("{:?}", remaining[i].as_rule()),
                                });
                            }
                        }
                        i += 1;
                    }
                    i += 1; // skip rbrace
                    result = Expression::Binary {
                        op: crate::ast::expression::BinaryOp::Overwrite,
                        left: Box::new(result),
                        right: Box::new(Expression::SetEnumeration(elements)),
                    };
                } else if remaining[i].as_rule() == Rule::op_inverse {
                    // Postfix inverse: r∼
                    result = Expression::Unary {
                        op: crate::ast::expression::UnaryOp::Inverse,
                        operand: Box::new(result),
                    };
                    i += 1;
                } else {
                    return Err(ParseError::UnexpectedRule {
                        expected: "lparen, lbracket, lbrace, or op_inverse".to_string(),
                        found: format!("{:?}", remaining[i].as_rule()),
                    });
                }
            }

            Ok(result)
        }
        Rule::primary_expr => {
            let mut inner = pair.into_inner();
            let first = inner.next().ok_or(ParseError::EmptyExpression)?;
            match first.as_rule() {
                Rule::kw_true => Ok(Expression::True),
                Rule::kw_false => Ok(Expression::False),
                Rule::op_emptyset => Ok(Expression::EmptySet),
                Rule::kw_nat => Ok(Expression::Naturals),
                Rule::kw_nat1 => Ok(Expression::Naturals1),
                Rule::kw_int => Ok(Expression::Integers),
                Rule::bool_expr => {
                    // bool(P): extract the predicate child
                    let mut bool_inner = first.into_inner();
                    // skip kw_bool
                    bool_inner.next();
                    // skip lparen
                    bool_inner.next();
                    let pred_pair = bool_inner.next().ok_or(ParseError::MissingPredicate)?;
                    let predicate = parse_predicate(pred_pair)?;
                    Ok(Expression::Bool(Box::new(predicate)))
                }
                Rule::kw_bool => Ok(Expression::BoolType),
                Rule::string_literal => {
                    // Extract string_inner content and process escapes.
                    // The grammar only allows \" and \\ as escape sequences;
                    // other backslash sequences are rejected at the grammar
                    // level. The fallback arms below are defensive, preserving
                    // the backslash verbatim if the grammar is ever extended.
                    let mut s = String::new();
                    for p in first.into_inner() {
                        match p.as_rule() {
                            Rule::string_inner => {
                                let raw = p.as_str();
                                let mut chars = raw.chars();
                                while let Some(c) = chars.next() {
                                    if c == '\\' {
                                        match chars.next() {
                                            Some('"') => s.push('"'),
                                            Some('\\') => s.push('\\'),
                                            // Defensive: grammar currently rejects
                                            // unknown escapes, but preserve them
                                            // verbatim if the grammar is extended.
                                            Some(other) => {
                                                s.push('\\');
                                                s.push(other);
                                            }
                                            None => s.push('\\'),
                                        }
                                    } else {
                                        s.push(c);
                                    }
                                }
                            }
                            _ => {
                                return Err(ParseError::UnexpectedRule {
                                    expected: "string_inner".to_string(),
                                    found: format!("{:?}", p.as_rule()),
                                });
                            }
                        }
                    }
                    Ok(Expression::StringLiteral(s))
                }
                Rule::integer => {
                    let value = first
                        .as_str()
                        .parse::<i64>()
                        .map_err(|_| ParseError::InvalidInteger(first.as_str().to_string()))?;
                    Ok(Expression::Integer(value))
                }
                Rule::if_then_else_expr => {
                    let mut ite_inner = first.into_inner();
                    // Skip kw_if
                    ite_inner.next();
                    let cond_pair = ite_inner.next().ok_or(ParseError::MissingPredicate)?;
                    let condition = parse_predicate(cond_pair)?;
                    // Skip kw_then
                    ite_inner.next();
                    let then_pair = ite_inner.next().ok_or(ParseError::EmptyExpression)?;
                    let then_expr = parse_expression(then_pair)?;
                    // Skip kw_else
                    ite_inner.next();
                    let else_pair = ite_inner.next().ok_or(ParseError::EmptyExpression)?;
                    let else_expr = parse_expression(else_pair)?;
                    Ok(Expression::IfThenElse {
                        condition: Box::new(condition),
                        then_expr: Box::new(then_expr),
                        else_expr: Box::new(else_expr),
                    })
                }
                Rule::identifier => Ok(Expression::Identifier(first.as_str().to_string())),
                Rule::expression => parse_expression(first),
                Rule::lparen => {
                    // Parenthesized expression: lparen ~ expression ~ rparen
                    let expr_pair = inner.next().ok_or(ParseError::EmptyExpression)?;
                    parse_expression(expr_pair)
                }
                Rule::set_enumeration => {
                    let mut elements = Vec::new();
                    for p in first.into_inner() {
                        match p.as_rule() {
                            Rule::expression => elements.push(parse_expression(p)?),
                            Rule::lbrace | Rule::rbrace | Rule::comma => {}
                            _ => {
                                return Err(ParseError::UnexpectedRule {
                                    expected: "expression or delimiter".to_string(),
                                    found: format!("{:?}", p.as_rule()),
                                });
                            }
                        }
                    }
                    Ok(Expression::SetEnumeration(elements))
                }
                Rule::set_comprehension => {
                    let mut identifiers = Vec::new();

                    let inner_pairs: Vec<_> = first.into_inner().collect();
                    let mut iter = inner_pairs.into_iter();

                    while let Some(p) = iter.next() {
                        match p.as_rule() {
                            Rule::typed_identifier => {
                                identifiers.push(parse_typed_identifier(p)?);
                            }
                            Rule::dot => {
                                // Extended form: {x · P | E}
                                // Next should be predicate, then pipe, then expression
                                let mut predicate = None;
                                let mut expression = None;
                                for rest in iter.by_ref() {
                                    match rest.as_rule() {
                                        Rule::predicate => {
                                            predicate = Some(parse_predicate(rest)?);
                                        }
                                        Rule::expression => {
                                            expression = Some(parse_expression(rest)?);
                                        }
                                        Rule::pipe | Rule::rbrace => {}
                                        _ => {
                                            return Err(ParseError::UnexpectedRule {
                                                expected: "predicate, expression, pipe, or rbrace"
                                                    .to_string(),
                                                found: format!("{:?}", rest.as_rule()),
                                            });
                                        }
                                    }
                                }
                                return Ok(Expression::SetComprehension {
                                    identifiers,
                                    predicate: Box::new(
                                        predicate.ok_or(ParseError::MissingPredicate)?,
                                    ),
                                    expression: Some(Box::new(
                                        expression.ok_or(ParseError::EmptyExpression)?,
                                    )),
                                });
                            }
                            Rule::predicate => {
                                // Basic form: {x | P}
                                let predicate = parse_predicate(p)?;
                                return Ok(Expression::SetComprehension {
                                    identifiers,
                                    predicate: Box::new(predicate),
                                    expression: None,
                                });
                            }
                            Rule::expression => {
                                // Expression form: {E | P}
                                // This is the third alternative in the grammar
                                let member_expression = parse_expression(p)?;
                                // Skip pipe, then parse predicate
                                let mut predicate = None;
                                for rest in iter.by_ref() {
                                    match rest.as_rule() {
                                        Rule::predicate => {
                                            predicate = Some(parse_predicate(rest)?);
                                        }
                                        Rule::pipe | Rule::rbrace => {}
                                        _ => {
                                            return Err(ParseError::UnexpectedRule {
                                                expected: "predicate, pipe, or rbrace".to_string(),
                                                found: format!("{:?}", rest.as_rule()),
                                            });
                                        }
                                    }
                                }
                                return Ok(Expression::SetBuilder {
                                    member_expression: Box::new(member_expression),
                                    predicate: Box::new(
                                        predicate.ok_or(ParseError::MissingPredicate)?,
                                    ),
                                });
                            }
                            Rule::lbrace | Rule::rbrace | Rule::comma | Rule::pipe => {}
                            _ => {
                                return Err(ParseError::UnexpectedRule {
                                    expected: "identifier, expression, predicate, or delimiter"
                                        .to_string(),
                                    found: format!("{:?}", p.as_rule()),
                                });
                            }
                        }
                    }

                    Err(ParseError::MissingPredicate)
                }
                _ => Err(ParseError::UnexpectedRule {
                    expected: "primary expression".to_string(),
                    found: format!("{:?}", first.as_rule()),
                }),
            }
        }
        Rule::identifier => Ok(Expression::Identifier(pair.as_str().to_string())),
        Rule::integer => {
            let value = pair
                .as_str()
                .parse::<i64>()
                .map_err(|_| ParseError::InvalidInteger(pair.as_str().to_string()))?;
            Ok(Expression::Integer(value))
        }
        _ => Err(ParseError::UnexpectedRule {
            expected: "expression".to_string(),
            found: format!("{:?} at {:?}", rule, span),
        }),
    }
}

/// Parse a predicate application (e.g., finite(S), partition(A, B, C))
fn parse_predicate_application(pair: pest::iterators::Pair<Rule>) -> Result<Predicate, ParseError> {
    let mut inner = pair.into_inner();
    let function = inner
        .next()
        .ok_or(ParseError::MissingVariable)?
        .as_str()
        .to_string();
    let mut arguments = Vec::new();
    for p in inner {
        match p.as_rule() {
            Rule::expression => arguments.push(parse_expression(p)?),
            Rule::lparen | Rule::rparen | Rule::comma => {}
            _ => {
                return Err(ParseError::UnexpectedRule {
                    expected: "expression or delimiter".to_string(),
                    found: format!("{:?}", p.as_rule()),
                });
            }
        }
    }

    if let Some(builtin) = crate::ast::predicate::BuiltinPredicate::from_name(&function) {
        if !builtin.check_arity(arguments.len()) {
            return Err(ParseError::ArityMismatch {
                name: builtin.name().to_string(),
                expected: if builtin.min_arity() == 1 {
                    builtin.min_arity().to_string()
                } else {
                    format!("at least {}", builtin.min_arity())
                },
                actual: arguments.len(),
            });
        }
        Ok(Predicate::BuiltinApplication {
            predicate: builtin,
            arguments,
        })
    } else {
        Ok(Predicate::Application {
            function,
            arguments,
        })
    }
}

fn parse_comparison_predicate(pair: pest::iterators::Pair<Rule>) -> Result<Predicate, ParseError> {
    use crate::ast::predicate::ComparisonOp;

    let mut inner = pair.into_inner();
    let left_expr = parse_expression(inner.next().ok_or(ParseError::EmptyExpression)?)?;
    let op_pair = inner.next().ok_or(ParseError::MissingOperator)?;
    let right_expr = parse_expression(inner.next().ok_or(ParseError::EmptyExpression)?)?;

    let op = match op_pair.as_rule() {
        Rule::op_eq => ComparisonOp::Equal,
        Rule::op_neq => ComparisonOp::NotEqual,
        Rule::op_lt => ComparisonOp::LessThan,
        Rule::op_le => ComparisonOp::LessEqual,
        Rule::op_gt => ComparisonOp::GreaterThan,
        Rule::op_ge => ComparisonOp::GreaterEqual,
        Rule::op_in => ComparisonOp::In,
        Rule::op_notin => ComparisonOp::NotIn,
        Rule::op_subset => ComparisonOp::Subset,
        Rule::op_subset_strict => ComparisonOp::SubsetStrict,
        Rule::op_not_subset => ComparisonOp::NotSubset,
        Rule::op_not_subset_strict => ComparisonOp::NotSubsetStrict,
        _ => {
            return Err(ParseError::UnexpectedRule {
                expected: "comparison operator".to_string(),
                found: format!("{:?}", op_pair.as_rule()),
            });
        }
    };

    Ok(Predicate::Comparison {
        op,
        left: left_expr,
        right: right_expr,
    })
}

/// Map a grammar logical operator rule to a LogicalOp
fn rule_to_logical_op(rule: Rule) -> Option<crate::ast::predicate::LogicalOp> {
    use crate::ast::predicate::LogicalOp;
    match rule {
        Rule::op_and => Some(LogicalOp::And),
        Rule::op_or => Some(LogicalOp::Or),
        Rule::op_implies => Some(LogicalOp::Implies),
        Rule::op_equivalent => Some(LogicalOp::Equivalent),
        _ => None,
    }
}

/// Parse a binary logical predicate (conjunction, disjunction, implication, equivalence)
fn parse_binary_predicate(pair: pest::iterators::Pair<Rule>) -> Result<Predicate, ParseError> {
    let mut inner = pair.into_inner();

    // Get the first operand
    let first = inner.next().ok_or(ParseError::EmptyPredicate)?;
    let mut left = parse_predicate(first)?;

    // Process remaining (operator, operand) pairs
    while let Some(op_pair) = inner.next() {
        if let Some(op) = rule_to_logical_op(op_pair.as_rule()) {
            let right_pair = inner.next().ok_or(ParseError::EmptyPredicate)?;
            let right = parse_predicate(right_pair)?;
            left = Predicate::Logical {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
    }

    Ok(left)
}

/// Parse a lambda ident-pattern: `ident_pattern_atom ~ (op_maplet ~ ident_pattern_atom)*`
fn parse_ident_pattern(pair: pest::iterators::Pair<Rule>) -> Result<IdentPattern, ParseError> {
    let mut inner = pair.into_inner();
    let first = inner.next().ok_or(ParseError::EmptyExpression)?;
    let mut result = parse_ident_pattern_atom(first)?;

    while let Some(next) = inner.next() {
        if next.as_rule() == Rule::op_maplet {
            let right_pair = inner.next().ok_or(ParseError::EmptyExpression)?;
            let right = parse_ident_pattern_atom(right_pair)?;
            result = IdentPattern::Maplet(Box::new(result), Box::new(right));
        }
    }
    Ok(result)
}

/// Parse a lambda ident-pattern atom: `lparen ~ ident_pattern ~ rparen | typed_identifier`
fn parse_ident_pattern_atom(pair: pest::iterators::Pair<Rule>) -> Result<IdentPattern, ParseError> {
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::typed_identifier => {
                return Ok(IdentPattern::Identifier(parse_typed_identifier(inner)?));
            }
            Rule::ident_pattern => return parse_ident_pattern(inner),
            _ => {} // skip lparen, rparen
        }
    }
    Err(ParseError::EmptyExpression)
}

/// Parse a predicate
///
/// Same iterative-descent treatment as [`parse_expression`]: the predicate
/// precedence chain (`predicate` → `quantified_predicate` →
/// `equivalence_predicate` → `implication_predicate` → `disjunction_predicate`
/// → `conjunction_predicate` → `negation_predicate` → `atomic_predicate`,
/// plus `_no_semi` twins) produces a deeply nested Pair tree even for a
/// simple comparison. We unwrap single-child wrappers in a loop and only
/// recurse for actual operators / quantifiers / negation.
fn parse_predicate(pair: pest::iterators::Pair<Rule>) -> Result<Predicate, ParseError> {
    let mut pair = pair;
    let rule = loop {
        let r = pair.as_rule();
        match r {
            // Always single-child by grammar.
            Rule::predicate | Rule::predicate_no_semi => {
                pair = pair.into_inner().next().ok_or(ParseError::EmptyPredicate)?;
            }
            // Binary precedence wrappers. Single child = no operator at this
            // level (descend); multi-child = real chain (dispatch).
            Rule::equivalence_predicate
            | Rule::implication_predicate
            | Rule::disjunction_predicate
            | Rule::conjunction_predicate
            | Rule::equivalence_predicate_no_semi
            | Rule::implication_predicate_no_semi
            | Rule::disjunction_predicate_no_semi
            | Rule::conjunction_predicate_no_semi => {
                let mut probe = pair.clone().into_inner();
                let first = probe.next().ok_or(ParseError::EmptyPredicate)?;
                if probe.next().is_some() {
                    return parse_binary_predicate(pair);
                }
                pair = first;
            }
            // negation_predicate: real negation/quantifier (`¬`, `∀`, `∃`)
            // at this level, otherwise passthrough to atomic_predicate.
            Rule::negation_predicate | Rule::negation_predicate_no_semi => {
                let mut probe = pair.clone().into_inner();
                let first = probe.next().ok_or(ParseError::EmptyPredicate)?;
                match first.as_rule() {
                    Rule::op_not | Rule::op_forall | Rule::op_exists => break r,
                    _ => pair = first,
                }
            }
            // quantified_predicate: real quantifier here, otherwise passthrough.
            Rule::quantified_predicate | Rule::quantified_predicate_no_semi => {
                let mut probe = pair.clone().into_inner();
                let first = probe.next().ok_or(ParseError::EmptyPredicate)?;
                match first.as_rule() {
                    Rule::op_forall | Rule::op_exists => break r,
                    _ => pair = first,
                }
            }
            _ => break r,
        }
    };

    match rule {
        Rule::negation_predicate | Rule::negation_predicate_no_semi => {
            use crate::ast::predicate::Quantifier;

            let mut inner = pair.into_inner();
            let first = inner.next().ok_or(ParseError::EmptyPredicate)?;
            if first.as_rule() == Rule::op_not {
                let pred = parse_predicate(inner.next().ok_or(ParseError::EmptyPredicate)?)?;
                Ok(Predicate::Not(Box::new(pred)))
            } else if first.as_rule() == Rule::op_forall || first.as_rule() == Rule::op_exists {
                // Quantified predicate nested inside conjunction/disjunction
                // (Rodin extension: spec requires parens, but Rodin accepts bare quantifiers)
                let quantifier = if first.as_rule() == Rule::op_forall {
                    Quantifier::ForAll
                } else {
                    Quantifier::Exists
                };
                let (identifiers, predicate) = collect_typed_identifiers_and_predicate(&mut inner)?;
                Ok(Predicate::Quantified {
                    quantifier,
                    identifiers,
                    predicate: Box::new(predicate),
                })
            } else {
                // Loop should have unwrapped the no-op alternative; defensive.
                parse_predicate(first)
            }
        }
        Rule::quantified_predicate | Rule::quantified_predicate_no_semi => {
            // Reached only when the iterative loop above detected a real
            // quantifier (`∀`/`∃`) at this level.
            use crate::ast::predicate::Quantifier;

            let mut inner = pair.into_inner();
            let first = inner.next().ok_or(ParseError::EmptyPredicate)?;
            let quantifier = match first.as_rule() {
                Rule::op_forall => Quantifier::ForAll,
                Rule::op_exists => Quantifier::Exists,
                _ => {
                    return Err(ParseError::UnexpectedRule {
                        expected: "∀ or ∃".to_string(),
                        found: format!("{:?}", first.as_rule()),
                    });
                }
            };
            let (identifiers, predicate) = collect_typed_identifiers_and_predicate(&mut inner)?;
            Ok(Predicate::Quantified {
                quantifier,
                identifiers,
                predicate: Box::new(predicate),
            })
        }
        Rule::atomic_predicate | Rule::atomic_predicate_no_semi => {
            let mut inner = pair.into_inner();
            let first = inner.next().ok_or(ParseError::EmptyPredicate)?;
            match first.as_rule() {
                Rule::kw_true => Ok(Predicate::True),
                Rule::kw_false => Ok(Predicate::False),
                Rule::predicate => parse_predicate(first),
                Rule::predicate_application => parse_predicate_application(first),
                Rule::comparison_predicate | Rule::comparison_predicate_no_semi => {
                    parse_comparison_predicate(first)
                }
                Rule::lparen => {
                    // Parenthesized predicate: lparen ~ predicate ~ rparen
                    let predicate_pair = inner.next().ok_or(ParseError::EmptyPredicate)?;
                    parse_predicate(predicate_pair)
                }
                _ => Err(ParseError::UnexpectedRule {
                    expected: "atomic predicate".to_string(),
                    found: format!("{:?}", first.as_rule()),
                }),
            }
        }
        Rule::comparison_predicate | Rule::comparison_predicate_no_semi => {
            parse_comparison_predicate(pair)
        }
        _ => Err(ParseError::UnexpectedRule {
            expected: "predicate".to_string(),
            found: format!("{:?}", rule),
        }),
    }
}

/// Parse a predicate from a string (used by XML parser)
///
/// Uses `predicate_complete` (with SOI/EOI) to ensure the entire input is consumed.
pub fn parse_predicate_str(input: &str) -> Result<Predicate, ParseError> {
    let pairs = RossiParser::parse(Rule::predicate_complete, input)
        .map_err(|e| ParseError::PestError(e.to_string()))?;

    let predicate_pair = pairs.into_iter().next().ok_or(ParseError::EmptyPredicate)?;
    parse_predicate(predicate_pair)
}

/// Parse an expression from a string (used by XML parser)
///
/// Uses `expression_complete` (with SOI/EOI) to ensure the entire input is consumed.
pub fn parse_expression_str(input: &str) -> Result<Expression, ParseError> {
    let pairs = RossiParser::parse(Rule::expression_complete, input)
        .map_err(|e| ParseError::PestError(e.to_string()))?;

    let expression_pair = pairs
        .into_iter()
        .next()
        .ok_or(ParseError::EmptyExpression)?;
    parse_expression(expression_pair)
}

/// Parse an action from a string (used by XML parser)
///
/// Uses `action_complete` (with SOI/EOI) to ensure the entire input is consumed.
pub fn parse_action_str(input: &str) -> Result<Action, ParseError> {
    let pairs = RossiParser::parse(Rule::action_complete, input)
        .map_err(|e| ParseError::PestError(e.to_string()))?;

    let action_pair = pairs.into_iter().next().ok_or(ParseError::MissingAction)?;
    parse_action(action_pair)
}

// ============================================================================
// Error Recovery Functions
// ============================================================================

use crate::error::ParseResult;

/// Compute (line, column) from a byte offset in the source text, both 1-indexed.
///
/// Line 1 is the first line, column 1 is the first character on that line.
/// This convention is suitable for human-readable error messages.
///
/// Note: [`Span::to_line_col`] uses 0-indexed values for LSP compatibility.
fn offset_to_line_col(source: &str, offset: usize) -> (usize, usize) {
    let span = Span {
        start: offset,
        end: offset,
    };
    let (line, col) = span.to_line_col(source);
    (line + 1, col + 1)
}

/// Find the byte offset of a substring in the source, searching from a given start position
fn find_line_offset(source: &str, line_content: &str, search_from: usize) -> Option<usize> {
    source[search_from..]
        .find(line_content)
        .map(|pos| search_from + pos)
}

/// Extract identifiers from a clause during error recovery
fn recover_identifiers(input: &str, keyword: &str) -> Vec<String> {
    let mut result = Vec::new();
    if let Some(content) = extract_clause_content(input, keyword) {
        for line in content.lines() {
            let line = line.trim();
            if !line.is_empty() && !line.to_uppercase().starts_with(keyword) {
                result.extend(extract_identifiers(line));
            }
        }
    }
    result
}

/// Extract labeled predicates from a clause during error recovery
fn recover_labeled_predicates(
    input: &str,
    keyword: &str,
    label: &str,
    errors: &mut Vec<ParseError>,
) -> Vec<LabeledPredicate> {
    let mut result = Vec::new();
    if let Some(content) = extract_clause_content(input, keyword) {
        let mut search_pos = 0;
        for line in content.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.to_uppercase().starts_with(keyword) {
                match try_parse_labeled_predicate_from_text(trimmed) {
                    Ok(pred) => result.push(pred),
                    Err(e) => {
                        let (err_line, err_col) =
                            if let Some(offset) = find_line_offset(input, trimmed, search_pos) {
                                search_pos = offset + trimmed.len();
                                offset_to_line_col(input, offset)
                            } else {
                                (0, 0)
                            };
                        errors.push(ParseError::RecoverableError {
                            line: err_line,
                            column: err_col,
                            message: format!("Failed to parse {}: {}", label, trimmed),
                            source: Some(Box::new(e)),
                        });
                    }
                }
            }
        }
    }
    result
}

/// Parse an Event-B component with error recovery
///
/// This function attempts to parse the input and recover from syntax errors
/// by skipping problematic sections and continuing with the rest of the document.
/// It returns a ParseResult that may contain a partial AST along with all errors encountered.
///
/// # Examples
///
/// ```no_run
/// use rossi::parse_with_recovery;
///
/// let source = r#"
/// CONTEXT test
/// SETS
///     MySet
/// CONSTANTS
///     invalid syntax here
/// AXIOMS
///     @axm1 1 = 1
/// END
/// "#;
///
/// let result = parse_with_recovery(source);
/// if result.has_recovered() {
///     println!("Parsed with {} errors", result.errors.len());
///     if let Some(component) = result.component {
///         println!("Partial AST: {:?}", component);
///     }
/// }
/// ```
pub fn parse_with_recovery(input: &str) -> ParseResult<Component> {
    // First, try normal parsing
    match parse(input) {
        Ok(component) => ParseResult::ok(component),
        Err(first_error) => {
            // Parsing failed, try to recover
            // Determine if it's a context or machine by looking for keywords
            if input.to_uppercase().contains("CONTEXT") {
                parse_context_with_recovery(input, first_error)
            } else if input.to_uppercase().contains("MACHINE") {
                parse_machine_with_recovery(input, first_error)
            } else {
                // Can't determine type, return original error
                ParseResult::err(first_error)
            }
        }
    }
}

/// Attempt to parse a context with error recovery
fn parse_context_with_recovery(input: &str, initial_error: ParseError) -> ParseResult<Component> {
    let mut errors = vec![initial_error];
    let mut context = Context::new(String::from("unknown"));

    // Try to extract the context name
    if let Some(name) = extract_component_name(input, "CONTEXT") {
        context.name = name;
    }

    // Try to parse each clause independently
    context.extends = recover_identifiers(input, "EXTENDS");
    context
        .sets
        .extend(recover_identifiers(input, "SETS").into_iter().map(|name| {
            SetDeclaration::Deferred {
                name,
                comment: None,
            }
        }));
    context.constants = recover_identifiers(input, "CONSTANTS")
        .into_iter()
        .map(NamedElement::new)
        .collect();
    context.axioms = recover_labeled_predicates(input, "AXIOMS", "axiom", &mut errors);

    ParseResult::with_errors(Some(Component::Context(context)), errors)
}

/// Attempt to parse a machine with error recovery
fn parse_machine_with_recovery(input: &str, initial_error: ParseError) -> ParseResult<Component> {
    let mut errors = vec![initial_error];
    let mut machine = Machine::new(String::from("unknown"));

    // Try to extract the machine name
    if let Some(name) = extract_component_name(input, "MACHINE") {
        machine.name = name;
    }

    // Try to parse each clause independently
    machine.refines = recover_identifiers(input, "REFINES").into_iter().next();
    machine.sees = recover_identifiers(input, "SEES");
    machine.variables = recover_identifiers(input, "VARIABLES")
        .into_iter()
        .map(NamedElement::new)
        .collect();
    machine.invariants = recover_labeled_predicates(input, "INVARIANTS", "invariant", &mut errors);

    // Note: VARIANT and EVENTS are more complex and would need specialized recovery
    // For now, we'll skip them if they fail to parse

    ParseResult::with_errors(Some(Component::Machine(machine)), errors)
}

/// Helper function to extract a component name from source
fn extract_component_name(input: &str, keyword: &str) -> Option<String> {
    for line in input.lines() {
        let line = line.trim();
        if line.to_uppercase().starts_with(keyword) {
            // Extract the identifier after the keyword
            let rest = &line[keyword.len()..].trim();
            return extract_identifier(rest).ok();
        }
    }
    None
}

/// Extract the content between a clause keyword and the next clause or END
fn extract_clause_content(input: &str, clause_keyword: &str) -> Option<String> {
    let upper_input = input.to_uppercase();
    let keyword_upper = clause_keyword.to_uppercase();

    // Find the start of this clause
    let start = upper_input.find(&keyword_upper)?;

    // Find the end of this clause: the next clause keyword, THEOREMS, or END.
    let mut end = input.len();
    for keyword in crate::keywords::recovery_boundary_spellings() {
        if keyword.eq_ignore_ascii_case(clause_keyword) {
            continue;
        }
        if let Some(pos) = upper_input[start + keyword_upper.len()..].find(keyword) {
            let absolute_pos = start + keyword_upper.len() + pos;
            if absolute_pos > start && absolute_pos < end {
                end = absolute_pos;
            }
        }
    }

    Some(input[start..end].to_string())
}

/// Extract an identifier from a string (handles commas and whitespace)
fn extract_identifier(s: &str) -> Result<String, ParseError> {
    let s = s.trim().trim_end_matches(',').trim();
    if s.is_empty() {
        return Err(ParseError::MissingVariable);
    }

    // Take the first word as identifier
    let id = s
        .split(|c: char| c.is_whitespace() || c == ',')
        .next()
        .unwrap_or("")
        .trim();

    if id.is_empty() || !id.starts_with(|c: char| c.is_alphabetic()) {
        return Err(ParseError::MissingVariable);
    }

    Ok(id.to_string())
}

/// Extract multiple identifiers from a comma-separated string
fn extract_identifiers(s: &str) -> Vec<String> {
    s.split(',')
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .filter_map(|part| {
            let id = part.split(char::is_whitespace).next().unwrap_or("").trim();
            if !id.is_empty() && id.chars().next().is_some_and(|c| c.is_alphabetic()) {
                Some(id.to_string())
            } else {
                None
            }
        })
        .collect()
}

/// Try to parse a labeled predicate from a single line of text
fn try_parse_labeled_predicate_from_text(text: &str) -> Result<LabeledPredicate, ParseError> {
    // Look for label (either "@label" or "label:")
    let text = text.trim();
    let (label, predicate_text) = if let Some(colon_pos) = text.find(':') {
        let potential_label = text[..colon_pos].trim();
        // Check if it looks like a label (not an operator like ":")
        if potential_label
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_')
            && !potential_label.is_empty()
        {
            (
                Some(potential_label.to_string()),
                text[colon_pos + 1..].trim(),
            )
        } else {
            (None, text)
        }
    } else if let Some(stripped) = text.strip_prefix('@') {
        let parts: Vec<&str> = stripped.splitn(2, char::is_whitespace).collect();
        if parts.len() == 2 {
            (Some(parts[0].to_string()), parts[1].trim())
        } else {
            (None, text)
        }
    } else {
        (None, text)
    };

    // Try to parse the predicate part
    match parse_predicate_str(predicate_text) {
        Ok(predicate) => Ok(LabeledPredicate {
            label,
            is_theorem: false,
            predicate,
            span: None,
            comment: None,
        }),
        Err(e) => Err(e),
    }
}
