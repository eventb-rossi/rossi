//! Rossi implementation
//!
//! This module provides the main parser interface using pest.

use pest::Parser;
use pest_derive::Parser;

use crate::ast::*;
use crate::error::ParseError;
use crate::nesting::{self, PARSER_STACK_SIZE, parser_stack_red_zone};

/// The pest-generated parser.
///
/// INVARIANT: never call `RossiParser::parse` directly. Every parse must go
/// through an entry point that first runs `nesting::check_nesting` (rejects
/// input deep enough to overflow the stack) and then wraps the pest parse +
/// AST build in `with_parser_stack`. Bypassing the guard reintroduces a
/// process-aborting stack overflow on adversarial input.
#[derive(Parser)]
#[grammar = "grammar.pest"]
pub struct RossiParser;

/// A human-readable spelling for a grammar [`Rule`], used to rewrite pest's
/// `expected …` lists from internal rule names (`op_in`, `lbrace`) into the
/// Event-B symbols a user actually types (`∈`, `{`). Returns `None` for rules
/// with no terser spelling, so the caller keeps pest's own name. Operators map
/// to a canonical Event-B glyph (usually the Unicode form); the few Rodin
/// private-use relations have no standard glyph, so they keep an ASCII
/// spelling.
#[rustfmt::skip]
pub(crate) fn friendly_rule_name(rule: Rule) -> Option<&'static str> {
    Some(match rule {
        Rule::op_and => "∧",
        Rule::op_or => "∨",
        Rule::op_implies => "⇒",
        Rule::op_equivalent => "⇔",
        Rule::op_not => "¬",
        Rule::op_forall => "∀",
        Rule::op_exists => "∃",
        Rule::op_in => "∈",
        Rule::op_notin => "∉",
        Rule::op_subset => "⊆",
        Rule::op_subset_strict => "⊂",
        Rule::op_not_subset => "⊈",
        Rule::op_not_subset_strict => "⊄",
        Rule::op_union => "∪",
        Rule::op_intersection => "∩",
        Rule::op_difference => "∖",
        Rule::op_cartesian => "×",
        Rule::op_powerset => "ℙ",
        Rule::op_powerset1 => "ℙ1",
        Rule::op_emptyset => "∅",
        Rule::op_oftype => "⦂",
        Rule::op_maplet => "↦",
        Rule::op_relation => "↔",
        Rule::op_partial_fn => "⇸",
        Rule::op_total_fn => "→",
        Rule::op_partial_inj => "⤔",
        Rule::op_total_inj => "↣",
        Rule::op_partial_surj => "⤀",
        Rule::op_total_surj => "↠",
        Rule::op_bijection => "⤖",
        Rule::op_domain => "dom",
        Rule::op_range => "ran",
        Rule::op_inverse => "∼",
        Rule::op_semicolon => ";",
        Rule::op_composition => "∘",
        Rule::op_domain_restrict => "◁",
        Rule::op_domain_subtract => "⩤",
        Rule::op_range_restrict => "▷",
        Rule::op_range_subtract => "⩥",
        Rule::op_overwrite => "⊕",
        Rule::op_direct_product => "⊗",
        Rule::op_parallel_product => "∥",
        Rule::op_total_surjective_relation => "<<->>",
        Rule::op_surjective_relation => "<->>",
        Rule::op_total_relation => "<<->",
        Rule::op_plus => "+",
        Rule::op_minus => "−",
        Rule::op_multiply => "∗",
        Rule::op_divide => "÷",
        Rule::op_modulo => "mod",
        Rule::op_exponent => "^",
        Rule::op_range_op => "..",
        Rule::op_eq => "=",
        Rule::op_neq => "≠",
        Rule::op_lt => "<",
        Rule::op_le => "≤",
        Rule::op_gt => ">",
        Rule::op_ge => "≥",
        Rule::op_becomes_equal => "≔",
        Rule::op_becomes_in => ":∈",
        Rule::op_becomes_such => ":∣",
        Rule::dot => "·",
        Rule::comma => ",",
        Rule::colon => ":",
        Rule::lparen => "(",
        Rule::rparen => ")",
        Rule::lbrace => "{",
        Rule::rbrace => "}",
        Rule::lbracket => "[",
        Rule::rbracket => "]",
        Rule::pipe => "|",
        _ => return None,
    })
}

/// Run a parse (pest + AST build) with guaranteed stack headroom.
///
/// Both pest's generated parser and the AST builder recurse on nested formula
/// constructs; inputs near [`nesting::MAX_NESTING_DEPTH`] need more stack
/// than debug builds or 2 MB worker threads provide. `stacker::maybe_grow`
/// moves execution to a heap-allocated segment when the remaining stack drops
/// below a red zone sized from `depth` — the metric [`nesting::check_nesting`]
/// measured for this input — so shallow formulas (the per-XML-attribute
/// import path, per-keystroke LSP parses) skip the segment allocation on
/// ordinary stacks. Inputs deeper than the limit never get here — the
/// pre-scan rejects them first.
pub(crate) fn with_parser_stack<T>(depth: usize, f: impl FnOnce() -> T) -> T {
    stacker::maybe_grow(parser_stack_red_zone(depth), PARSER_STACK_SIZE, f)
}

/// Build the located error for a kernel_lang §2.2 reserved word misused as
/// an ordinary identifier.
fn reserved_word_error(word: &str, span: pest::Span<'_>) -> ParseError {
    let (line, column) = span.start_pos().line_col();
    ParseError::ReservedWord {
        word: word.to_string(),
        line,
        column,
        span: Some(Span::from_pest(span)),
    }
}

/// Extract an identifier that *names* a user identifier — a declaration
/// (constant, variable, carrier set or element, event parameter, binder) or
/// an assignment target — rejecting the kernel_lang §2.2 reserved words
/// ([`crate::builtins::is_reserved_word`]). All declared names must come
/// through here.
fn declared_name(pair: &pest::iterators::Pair<Rule>) -> Result<String, ParseError> {
    if crate::builtins::is_reserved_word(pair.as_str()) {
        return Err(reserved_word_error(pair.as_str(), pair.as_span()));
    }
    Ok(pair.as_str().to_string())
}

/// Reject reserved operator words (`card`, `dom`, `mod`, …) standing as a
/// plain identifier in formula position. The generic atoms (`id`, `pred`, …)
/// pass — they are legal bare expressions.
fn reject_reserved_operator_word(pair: &pest::iterators::Pair<Rule>) -> Result<(), ParseError> {
    if crate::builtins::is_reserved_operator_word(pair.as_str()) {
        return Err(reserved_word_error(pair.as_str(), pair.as_span()));
    }
    Ok(())
}

/// Parse a typed_identifier rule into a TypedIdentifier
fn parse_typed_identifier(
    pair: pest::iterators::Pair<Rule>,
) -> Result<TypedIdentifier, ParseError> {
    let mut inner = pair.into_inner();
    let name_pair = inner.next().ok_or(ParseError::MissingVariable)?;
    let name = declared_name(&name_pair)?;
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

/// Extract all identifiers from a clause pair, skipping the keyword.
///
/// Clauses that *declare* mathematical identifiers (CONSTANTS, VARIABLES,
/// event ANY) reject kernel_lang §2.2 reserved words; the declaring role is
/// derived from the clause's own leading keyword. Component-reference
/// clauses (EXTENDS, REFINES, SEES) stay permissive: component names are
/// structural labels, not formula identifiers (Camille's eventbstruct lexer
/// doesn't reserve these words there either).
fn collect_identifiers_from_clause(
    pair: pest::iterators::Pair<Rule>,
) -> Result<Vec<String>, ParseError> {
    let declares = matches!(
        pair.as_rule(),
        Rule::context_clause_constants | Rule::machine_clause_variables | Rule::event_any
    );
    let mut identifiers = Vec::new();
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::identifier => {
                identifiers.push(if declares {
                    declared_name(&p)?
                } else {
                    p.as_str().to_string()
                });
            }
            // Component references (EXTENDS, REFINES, SEES) — hyphen-capable
            // structural names, never declarations.
            Rule::component_name => {
                identifiers.push(p.as_str().to_string());
            }
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

/// Extract declared elements from a clause pair, keeping per-identifier
/// spans so trailing comments can attach to them (constants, variables,
/// event parameters).
///
/// These clauses all *declare* identifiers, so each name is routed through
/// [`declared_name`] to reject kernel_lang §2.2 reserved words, exactly as
/// the `String`-only [`collect_identifiers_from_clause`] does.
fn collect_named_elements_from_clause(
    pair: pest::iterators::Pair<Rule>,
) -> Result<Vec<NamedElement>, ParseError> {
    let mut elements = Vec::new();
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::identifier => elements.push(NamedElement {
                name: declared_name(&p)?,
                comment: None,
                span: Some(Span::from_pest(p.as_span())),
            }),
            Rule::comma => {} // optional comma separator
            // Skip the leading clause keyword (varies by call site)
            Rule::kw_constants | Rule::kw_variables | Rule::kw_any => {}
            _ => {
                return Err(ParseError::UnexpectedRule {
                    expected: "identifier or comma".to_string(),
                    found: format!("{:?}", p.as_rule()),
                });
            }
        }
    }
    Ok(elements)
}

/// Parse a set declaration (deferred or enumerated)
fn parse_set_declaration(pair: pest::iterators::Pair<Rule>) -> Result<SetDeclaration, ParseError> {
    let span = Some(Span::from_pest(pair.as_span()));
    let mut inner = pair.into_inner();
    let name_pair = inner.next().ok_or(ParseError::MissingVariable)?;
    let name = declared_name(&name_pair)?;

    // Check if there's an '=' followed by enumerated elements
    let mut elements = Vec::new();
    let mut has_eq = false;
    for p in inner {
        match p.as_rule() {
            Rule::op_eq => {
                has_eq = true;
            }
            Rule::identifier => {
                elements.push(declared_name(&p)?);
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
            span,
        })
    } else {
        Ok(SetDeclaration::Deferred {
            name,
            comment: None,
            span,
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

/// Collect the predicates of a `THEOREMS` clause, forcing `is_theorem = true` on
/// each. A `THEOREMS` section is sugar for theorem-flagged axioms/invariants:
/// Rodin models a theorem as a boolean attribute on an axiom/invariant element
/// (there is no theorem container), so a member of a THEOREMS section is a theorem
/// even when written without the inline `theorem` keyword. The result is appended
/// to the same `axioms`/`invariants` vec, keeping rossi's model identical to
/// Rodin's and lossless across the Rodin XML round-trip.
fn collect_theorem_predicates(
    pair: pest::iterators::Pair<Rule>,
    keyword_rule: Rule,
) -> Result<Vec<LabeledPredicate>, ParseError> {
    let mut predicates = collect_labeled_predicates(pair, keyword_rule)?;
    for p in &mut predicates {
        p.is_theorem = true;
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
    let depth = nesting::check_nesting(input)?;
    with_parser_stack(depth, || parse_unguarded(input))
}

/// Body of [`parse`]. Only callable through the guarded entry point — see
/// the invariant on [`RossiParser`].
fn parse_unguarded(input: &str) -> Result<Component, ParseError> {
    let pairs =
        RossiParser::parse(Rule::component, input).map_err(|e| ParseError::from(Box::new(e)))?;

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

    let mut component = match inner.as_rule() {
        Rule::context => parse_context(inner)?,
        Rule::machine => parse_machine(inner)?,
        _ => {
            return Err(ParseError::UnexpectedRule {
                expected: "context or machine".to_string(),
                found: format!("{:?}", inner.as_rule()),
            });
        }
    };
    crate::comment_attach::attach_comments(input, std::slice::from_mut(&mut component));
    Ok(component)
}

/// Parse one or more Event-B components (Contexts and/or Machines) from source text.
///
/// This is the multi-component counterpart of [`parse`]. Files produced by
/// `rossi import --merge` or the reference `eventb-to-txt` tool may contain
/// several `CONTEXT` and `MACHINE` blocks concatenated in a single file.
///
/// Returns `Ok(Vec<Component>)` with one entry per parsed component.
pub fn parse_components(input: &str) -> Result<Vec<Component>, ParseError> {
    let depth = nesting::check_nesting(input)?;
    with_parser_stack(depth, || parse_components_unguarded(input))
}

/// Body of [`parse_components`]. Only callable through the guarded entry
/// point — see the invariant on [`RossiParser`].
fn parse_components_unguarded(input: &str) -> Result<Vec<Component>, ParseError> {
    let pairs =
        RossiParser::parse(Rule::components, input).map_err(|e| ParseError::from(Box::new(e)))?;

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

    crate::comment_attach::attach_comments(input, &mut result);
    Ok(result)
}

/// Return the canonical ordering index for a context clause rule.
/// EXTENDS=0, SETS=1, CONSTANTS=2, AXIOMS=3, THEOREMS=4
fn context_clause_order(rule: Rule) -> Option<usize> {
    match rule {
        Rule::context_clause_extends => Some(0),
        Rule::context_clause_sets => Some(1),
        Rule::context_clause_constants => Some(2),
        Rule::context_clause_axioms => Some(3),
        Rule::context_clause_theorems => Some(4),
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
        Rule::context_clause_theorems => "THEOREMS",
        _ => "unknown",
    }
}

/// Return the canonical ordering index for a machine clause rule.
/// REFINES=0, SEES=1, VARIABLES=2, INVARIANTS=3, THEOREMS=4, VARIANT=5, EVENTS=6
fn machine_clause_order(rule: Rule) -> Option<usize> {
    match rule {
        Rule::machine_clause_refines => Some(0),
        Rule::machine_clause_sees => Some(1),
        Rule::machine_clause_variables => Some(2),
        Rule::machine_clause_invariants => Some(3),
        Rule::machine_clause_theorems => Some(4),
        Rule::machine_clause_variant => Some(5),
        Rule::machine_clause_events => Some(6),
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
        Rule::machine_clause_theorems => "THEOREMS",
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
                    context
                        .constants
                        .extend(collect_named_elements_from_clause(pair)?);
                }
                Rule::context_clause_axioms => {
                    context
                        .axioms
                        .extend(collect_labeled_predicates(pair, Rule::kw_axioms)?);
                }
                Rule::context_clause_theorems => {
                    // THEOREMS lowers into `axioms` with `is_theorem = true`.
                    context
                        .axioms
                        .extend(collect_theorem_predicates(pair, Rule::kw_theorems)?);
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
                    machine
                        .variables
                        .extend(collect_named_elements_from_clause(pair)?);
                }
                Rule::machine_clause_invariants => {
                    machine
                        .invariants
                        .extend(collect_labeled_predicates(pair, Rule::kw_invariants)?);
                }
                Rule::machine_clause_theorems => {
                    // THEOREMS lowers into `invariants` with `is_theorem = true`.
                    machine
                        .invariants
                        .extend(collect_theorem_predicates(pair, Rule::kw_theorems)?);
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
                    event
                        .parameters
                        .extend(collect_named_elements_from_clause(pair)?);
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
    let span = Some(Span::from_pest(pair.as_span()));
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
        span,
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
                // Assignment targets are uses of *declared* variables, so no
                // reserved word can name one: `pred ≔ 0` is as invalid as
                // `dom ≔ 0`.
                variables.push(declared_name(&p)?);
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
/// `maplet_expr` → `relation_type_expr` → `set_operator_expr` →
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
        Rule::closed_unary_expr => {
            // Fixed layout: op ~ lparen ~ expression ~ rparen.
            let mut inner = pair.into_inner();
            let op_pair = inner.next().ok_or(ParseError::MissingOperator)?;
            let op =
                rule_to_unary_op(op_pair.as_rule()).ok_or_else(|| ParseError::UnexpectedRule {
                    expected: "dom or ran".to_string(),
                    found: format!("{:?}", op_pair.as_rule()),
                })?;
            let operand_pair = inner.nth(1).ok_or(ParseError::EmptyExpression)?;
            Ok(Expression::Unary {
                op,
                operand: Box::new(parse_expression(operand_pair)?),
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
            let base_pair = inner.next().ok_or(ParseError::EmptyExpression)?;
            let base_span = base_pair.as_span();
            let base = parse_expression(base_pair)?;

            // Check if there are any function applications or relational images
            let remaining: Vec<_> = inner.collect();

            // A reserved operator word is only legal where the
            // `BuiltinFunction::from_name` resolution below consumes it.
            // Anywhere else — bare, under postfix `∼`, image `[…]`, or an
            // unresolvable application like `mod(x)` — it is an invalid
            // identifier (see `builtins::RESERVED_OPERATOR_WORDS`). This is
            // the expression-position check; predicate applications,
            // assignment targets, and declarations have sibling checks.
            if let Expression::Identifier(ref name) = base
                && crate::builtins::is_reserved_operator_word(name)
            {
                let resolves = remaining
                    .first()
                    .is_some_and(|p| p.as_rule() == Rule::lparen)
                    && crate::ast::expression::BuiltinFunction::from_name(name).is_some();
                if !resolves {
                    return Err(reserved_word_error(name, base_span));
                }
            }

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
        Rule::identifier => {
            // Defensive: only reachable if a caller hands parse_expression a
            // raw identifier pair (formula identifiers arrive wrapped in
            // function_application, which carries the position-aware check).
            reject_reserved_operator_word(&pair)?;
            Ok(Expression::Identifier(pair.as_str().to_string()))
        }
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
    let function_pair = inner.next().ok_or(ParseError::MissingVariable)?;
    let function_span = function_pair.as_span();
    let function = function_pair.as_str().to_string();
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
    } else if crate::builtins::is_reserved_word(&function) {
        // A reserved word applied where no builtin predicate resolves it:
        // the expression-only forms (`dom(x)`, `mod(x)`) and the generic
        // atoms (`pred(x)`, `id(x)` — expressions, never predicates).
        // Reject like Rodin instead of fabricating a user-defined predicate
        // application named by a reserved word.
        Err(reserved_word_error(&function, function_span))
    } else {
        Ok(Predicate::Application {
            function,
            arguments,
        })
    }
}

/// Map a grammar comparison operator rule to a [`ComparisonOp`].
///
/// [`ComparisonOp`]: crate::ast::predicate::ComparisonOp
fn rule_to_comparison_op(rule: Rule) -> Option<crate::ast::predicate::ComparisonOp> {
    use crate::ast::predicate::ComparisonOp;
    match rule {
        Rule::op_eq => Some(ComparisonOp::Equal),
        Rule::op_neq => Some(ComparisonOp::NotEqual),
        Rule::op_lt => Some(ComparisonOp::LessThan),
        Rule::op_le => Some(ComparisonOp::LessEqual),
        Rule::op_gt => Some(ComparisonOp::GreaterThan),
        Rule::op_ge => Some(ComparisonOp::GreaterEqual),
        Rule::op_in => Some(ComparisonOp::In),
        Rule::op_notin => Some(ComparisonOp::NotIn),
        Rule::op_subset => Some(ComparisonOp::Subset),
        Rule::op_subset_strict => Some(ComparisonOp::SubsetStrict),
        Rule::op_not_subset => Some(ComparisonOp::NotSubset),
        Rule::op_not_subset_strict => Some(ComparisonOp::NotSubsetStrict),
        _ => None,
    }
}

/// Map a grammar quantifier rule to a [`Quantifier`].
///
/// [`Quantifier`]: crate::ast::predicate::Quantifier
fn rule_to_quantifier(rule: Rule) -> Option<crate::ast::predicate::Quantifier> {
    use crate::ast::predicate::Quantifier;
    match rule {
        Rule::op_forall => Some(Quantifier::ForAll),
        Rule::op_exists => Some(Quantifier::Exists),
        _ => None,
    }
}

fn parse_comparison_predicate(pair: pest::iterators::Pair<Rule>) -> Result<Predicate, ParseError> {
    let mut inner = pair.into_inner();
    let left_expr = parse_expression(inner.next().ok_or(ParseError::EmptyExpression)?)?;
    let op_pair = inner.next().ok_or(ParseError::MissingOperator)?;
    let right_expr = parse_expression(inner.next().ok_or(ParseError::EmptyExpression)?)?;

    let op =
        rule_to_comparison_op(op_pair.as_rule()).ok_or_else(|| ParseError::UnexpectedRule {
            expected: "comparison operator".to_string(),
            found: format!("{:?}", op_pair.as_rule()),
        })?;

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
            let mut inner = pair.into_inner();
            let first = inner.next().ok_or(ParseError::EmptyPredicate)?;
            if first.as_rule() == Rule::op_not {
                let pred = parse_predicate(inner.next().ok_or(ParseError::EmptyPredicate)?)?;
                Ok(Predicate::Not(Box::new(pred)))
            } else if let Some(quantifier) = rule_to_quantifier(first.as_rule()) {
                // Quantified predicate nested inside conjunction/disjunction
                // (Rodin extension: spec requires parens, but Rodin accepts bare quantifiers)
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
            let mut inner = pair.into_inner();
            let first = inner.next().ok_or(ParseError::EmptyPredicate)?;
            let quantifier =
                rule_to_quantifier(first.as_rule()).ok_or_else(|| ParseError::UnexpectedRule {
                    expected: "∀ or ∃".to_string(),
                    found: format!("{:?}", first.as_rule()),
                })?;
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
    let depth = nesting::check_nesting(input)?;
    with_parser_stack(depth, || {
        let pairs = RossiParser::parse(Rule::predicate_complete, input)
            .map_err(|e| ParseError::from(Box::new(e)))?;

        let predicate_pair = pairs.into_iter().next().ok_or(ParseError::EmptyPredicate)?;
        parse_predicate(predicate_pair)
    })
}

/// Parse an expression from a string (used by XML parser)
///
/// Uses `expression_complete` (with SOI/EOI) to ensure the entire input is consumed.
pub fn parse_expression_str(input: &str) -> Result<Expression, ParseError> {
    let depth = nesting::check_nesting(input)?;
    with_parser_stack(depth, || {
        let pairs = RossiParser::parse(Rule::expression_complete, input)
            .map_err(|e| ParseError::from(Box::new(e)))?;

        let expression_pair = pairs
            .into_iter()
            .next()
            .ok_or(ParseError::EmptyExpression)?;
        parse_expression(expression_pair)
    })
}

/// Parse an action from a string (used by XML parser)
///
/// Uses `action_complete` (with SOI/EOI) to ensure the entire input is
/// consumed. The input holds exactly one action, so unlike actions in a
/// THEN block it is parsed with the full expression grammar
/// (`standalone_action`): a bare `;` is forward composition, not an
/// action boundary.
pub fn parse_action_str(input: &str) -> Result<Action, ParseError> {
    let depth = nesting::check_nesting(input)?;
    with_parser_stack(depth, || {
        let pairs = RossiParser::parse(Rule::action_complete, input)
            .map_err(|e| ParseError::from(Box::new(e)))?;

        let action_pair = pairs.into_iter().next().ok_or(ParseError::MissingAction)?;
        parse_action(action_pair)
    })
}

// ============================================================================
// Error Recovery Functions
// ============================================================================

use crate::error::ParseResult;
use crate::keywords::{KeywordId, is_structural_word_bounded};

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

/// The text views the recovery scanner works on. ASCII masking and ASCII
/// uppercasing both preserve byte layout, so offsets are interchangeable
/// across all three.
struct RecoveryText<'a> {
    /// The source as written — only ever quoted in error messages.
    original: &'a str,
    /// Comment-masked copy: all structural scanning runs on this, so comment
    /// text cannot influence any structural decision (issue #24).
    masked: String,
    /// ASCII-uppercased `masked`, for case-insensitive keyword search.
    masked_upper: String,
    /// `@`-label spans (valid in all views): keyword scans skip them, so a
    /// keyword spelled inside a label (`@safety-END`) is never structural.
    labels: Vec<Span>,
}

impl<'a> RecoveryText<'a> {
    fn new(original: &'a str) -> Self {
        let spans = crate::comments::lexical_spans(original);
        let masked = spans.mask_comments(original);
        let masked_upper = masked.to_ascii_uppercase();
        Self {
            original,
            masked,
            masked_upper,
            labels: spans.labels,
        }
    }
}

/// Extract identifiers from a clause during error recovery.
///
/// Content written inline after the clause keyword (`VARIABLES x, y`) counts
/// like any other line. Declaring clauses (keyed off `keyword`, mirroring
/// `collect_identifiers_from_clause` on the strict path) drop reserved words:
/// the strict parse already reported them as ReservedWord errors, and keeping
/// them would hand downstream consumers (LSP completion, rename) a
/// declaration the parser itself forbids.
fn recover_identifiers(text: &RecoveryText, keyword: &str, bound: usize) -> Vec<String> {
    let declares = matches!(keyword, "SETS" | "CONSTANTS" | "VARIABLES");
    // Keep only names the grammar would re-accept in this position, so a
    // recovered AST stays round-trippable — whitespace-split recovery would
    // otherwise yield `a--b`/`x-y`, which the pretty-printer cannot re-emit.
    // Declaring clauses (SETS/CONSTANTS/VARIABLES) take mathematical
    // identifiers (and reject reserved words, mirroring the strict path);
    // reference clauses (EXTENDS/SEES/REFINES) take component names.
    let accepts = |name: &String| {
        if declares {
            crate::names::is_valid_math_identifier(name) && !crate::builtins::is_reserved_word(name)
        } else {
            crate::names::is_valid_component_name(name)
        }
    };
    let mut result = Vec::new();
    if let Some(span) = extract_clause_content(text, keyword, bound) {
        for line in text.masked[span.start..span.end].lines() {
            let line = line.trim();
            let content = strip_keyword_prefix(line, keyword).unwrap_or(line);
            if !content.is_empty() {
                result.extend(extract_identifiers(content).into_iter().filter(&accepts));
            }
        }
    }
    result
}

/// Extract labeled predicates from a clause during error recovery.
///
/// The clause is segmented into labeled predicates by label-bearing lines, so
/// a predicate that spans several physical lines (`@inv1` on one line, the
/// predicate indented below) stays whole — `WHITESPACE` includes `\n` in the
/// grammar. A comment-only line masks to whitespace and falls inside whichever
/// predicate it sits in; a predicate written inline after the clause keyword
/// (`AXIOMS @axm1 P`) is recovered like any other. Error positions are
/// byte-exact.
fn recover_labeled_predicates(
    text: &RecoveryText,
    keyword: &str,
    label: &str,
    bound: usize,
    errors: &mut Vec<ParseError>,
) -> Vec<LabeledPredicate> {
    let mut result = Vec::new();
    let Some(span) = extract_clause_content(text, keyword, bound) else {
        return result;
    };

    // Collect the byte offset where each labeled predicate begins: the clause
    // keyword (the leading segment, which also covers a predicate written
    // inline after the keyword) plus every later predicate start. Splitting on
    // `\n` instead — as this once did — cut every multi-line label off from its
    // body and reported each correct predicate as a failure, lighting up the
    // whole clause in the editor.
    //
    // A predicate starts at a line whose first token is `@label` or `theorem`.
    // When the clause has no such line it is a run of bare, label-less
    // predicates (the grammar's `label?` is optional) — there, every line
    // starts a predicate, so each is recovered rather than lumped into one
    // segment the single-predicate parser would reject.
    let body = &text.masked[span.start..span.end];
    let any_label = body
        .split_inclusive('\n')
        .skip(1)
        .any(line_starts_labeled_predicate);
    let mut starts = vec![span.start];
    let mut line_start = span.start;
    for line in body.split_inclusive('\n') {
        if line_start != span.start && (!any_label || line_starts_labeled_predicate(line)) {
            starts.push(line_start);
        }
        line_start += line.len();
    }
    starts.push(span.end);

    for pair in starts.windows(2) {
        let (seg_start, seg_end) = (pair[0], pair[1]);
        let raw = &text.masked[seg_start..seg_end];
        // Only the leading segment still carries the clause keyword.
        let content = if seg_start == span.start {
            strip_keyword_prefix(raw, keyword).unwrap_or_else(|| raw.trim())
        } else {
            raw.trim()
        };
        if content.is_empty() {
            continue;
        }
        match try_parse_labeled_predicate_from_text(content) {
            Ok(pred) => result.push(pred),
            Err(e) => {
                // `content` is a subslice of `raw`; recover its absolute offset
                // (masked and original share a byte layout).
                let content_offset = content.as_ptr() as usize - raw.as_ptr() as usize;
                let abs_start = seg_start + content_offset;
                let abs_end = abs_start + content.len();
                let (err_line, err_col) = offset_to_line_col(text.original, abs_start);
                let subject = leading_label(content)
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        text.original[abs_start..abs_end]
                            .lines()
                            .next()
                            .unwrap_or_default()
                            .trim()
                            .to_string()
                    });
                errors.push(ParseError::RecoverableError {
                    line: err_line,
                    column: err_col,
                    message: format!("Failed to parse {label}: {subject}"),
                    span: Some(Span {
                        start: abs_start,
                        end: abs_end,
                    }),
                    source: Some(Box::new(e)),
                });
            }
        }
    }
    result
}

/// Whether `line` (a physical line, possibly indented) begins a new labeled
/// predicate during recovery: its first token is a `@label` or the `theorem`
/// keyword. A continuation line of a multi-line predicate begins with neither.
fn line_starts_labeled_predicate(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with('@') || starts_with_keyword(t, "theorem")
}

/// Whether `s` begins with `kw` as a whole word (case-insensitive): the match
/// is not immediately followed by another word char, so `theorem` matches in
/// `theorem @t1` but `theoremish` is left alone. Boundary detection goes
/// through the shared [`crate::keywords::is_word_bounded`] so it can't drift
/// from the rest of the keyword scanning.
fn starts_with_keyword(s: &str, kw: &str) -> bool {
    s.get(..kw.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(kw))
        && crate::keywords::is_word_bounded(s, 0, kw.len())
}

/// The leading `@label` of a recovered segment, if any. The segment may open
/// with the `theorem` keyword before the label (`theorem @grd1 …`); the label,
/// including its `@`, keeps the recovery error message short. A stray `@`
/// inside a malformed predicate is not mistaken for a leading label.
fn leading_label(content: &str) -> Option<&str> {
    let at = content.find('@')?;
    let prefix = content[..at].trim();
    if !prefix.is_empty() && !prefix.eq_ignore_ascii_case("theorem") {
        return None;
    }
    let rest = &content[at..];
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    Some(&rest[..end])
}

/// Recover a `THEOREMS` clause, forcing `is_theorem = true` on each predicate.
/// Mirrors the strict parser, which lowers THEOREMS into the axioms/invariants vec
/// with the flag set (a theorem is a flagged axiom/invariant in Rodin's model).
fn recover_theorem_predicates(
    text: &RecoveryText,
    bound: usize,
    errors: &mut Vec<ParseError>,
) -> Vec<LabeledPredicate> {
    let mut result = recover_labeled_predicates(text, "THEOREMS", "theorem", bound, errors);
    for p in &mut result {
        p.is_theorem = true;
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
        // A depth rejection applies to the whole input — clause recovery
        // would just re-trigger it line by line, so fail fast.
        Err(first_error @ ParseError::NestingTooDeep { .. }) => ParseResult::err(first_error),
        Err(first_error) => {
            // Parsing failed, try to recover. Dispatch on the EARLIEST
            // whole-word CONTEXT/MACHINE in the comment-masked text: the
            // header keyword always precedes any identifier that happens to
            // spell the other keyword (so a variable named `context` cannot
            // flip a machine into context recovery), and junk the grammar
            // rejects before the header (a UTF-8 BOM, stray tokens) does
            // not defeat recovery.
            let text = RecoveryText::new(input);
            let end = text.masked.len();
            let context_pos = find_keyword_word(&text, "CONTEXT", 0, end);
            let machine_pos = find_keyword_word(&text, "MACHINE", 0, end);
            match (context_pos, machine_pos) {
                (Some(ctx), Some(mch)) if mch < ctx => {
                    parse_machine_with_recovery(&text, mch, first_error)
                }
                (Some(ctx), _) => parse_context_with_recovery(&text, ctx, first_error),
                (None, Some(mch)) => parse_machine_with_recovery(&text, mch, first_error),
                // Can't determine type, return original error
                (None, None) => ParseResult::err(first_error),
            }
        }
    }
}

/// Parse one or more Event-B components with error recovery.
///
/// The multi-component counterpart of [`parse_with_recovery`], for files
/// produced by `rossi import --merge` (several `CONTEXT`/`MACHINE` blocks
/// concatenated in one file). On a strict-parse failure the input is split
/// into per-component regions at line-anchored `CONTEXT`/`MACHINE` headers
/// and each region recovers independently, so one broken component does not
/// take down its siblings. All spans and error positions are absolute within
/// the full input.
pub fn parse_components_with_recovery(input: &str) -> ParseResult<Vec<Component>> {
    // First, try normal parsing — spans come out file-absolute for free.
    match parse_components(input) {
        Ok(components) => ParseResult::ok(components),
        // A depth rejection applies to the whole input — see parse_with_recovery.
        Err(first_error @ ParseError::NestingTooDeep { .. }) => ParseResult::err(first_error),
        Err(first_error) => {
            let text = RecoveryText::new(input);
            let headers = component_header_starts(&text);
            if headers.len() < 2 {
                // Zero or one component header: single-component recovery
                // already handles this exactly (including the no-header case).
                let result = parse_with_recovery(input);
                return ParseResult::with_errors(result.component.map(|c| vec![c]), result.errors);
            }

            // Region i runs from its header's line start to the next header's
            // line start; the first region is extended back to offset 0 so
            // junk before the first header is still reported. Headers sit on
            // distinct lines (line-anchored), so the starts stay strictly
            // ascending even after the first is pulled back to 0.
            let mut starts: Vec<usize> = headers
                .iter()
                .map(|&pos| line_start(&text.masked, pos))
                .collect();
            starts[0] = 0;

            let mut components = Vec::new();
            let mut errors = Vec::new();
            // Regions begin at line boundaries, so the slice's line N is the
            // input's line N + line_delta (columns are unchanged). Counted
            // incrementally — regions are consecutive, so each gap between
            // starts is scanned once.
            let mut line_delta = 0;
            let mut prev_start = 0;
            for (i, &start) in starts.iter().enumerate() {
                let end = starts.get(i + 1).copied().unwrap_or(input.len());
                line_delta += input[prev_start..start].matches('\n').count();
                prev_start = start;
                let result = parse_with_recovery(&input[start..end]);
                errors.extend(
                    result
                        .errors
                        .into_iter()
                        .map(|e| offset_error_lines(e, line_delta)),
                );
                if let Some(mut component) = result.component {
                    shift_component_spans(&mut component, start);
                    // Recovery builds span-less components; give them their
                    // region as an approximate span so position-based
                    // consumers (component-at-offset dispatch, semantic
                    // tokens) still anchor to the right part of the file.
                    if component.span().is_none() {
                        set_component_span(&mut component, Span { start, end });
                    }
                    components.push(component);
                }
            }

            if components.is_empty() {
                return ParseResult::err(first_error);
            }
            if errors.is_empty() {
                // The whole-file parse failed, so something is wrong even if
                // every region recovered silently — keep the original error.
                errors.push(first_error);
            }
            ParseResult::with_errors(Some(components), errors)
        }
    }
}

/// Byte offsets of every line-anchored, whole-word `CONTEXT`/`MACHINE`
/// header in the comment-masked text, in source order. Line-anchoring (only
/// whitespace before the keyword on its line) is what keeps a mid-line
/// mention — an identifier in a guard, say — from splitting a region.
fn component_header_starts(text: &RecoveryText) -> Vec<usize> {
    let mut starts = Vec::new();
    let end = text.masked.len();
    for keyword in ["CONTEXT", "MACHINE"] {
        let mut from = 0;
        while let Some(pos) = find_keyword_word(text, keyword, from, end) {
            if text.masked[line_start(&text.masked, pos)..pos]
                .chars()
                .all(char::is_whitespace)
            {
                starts.push(pos);
            }
            from = pos + keyword.len();
        }
    }
    starts.sort_unstable();
    starts
}

/// Byte offset of the start of the line containing `pos`.
fn line_start(s: &str, pos: usize) -> usize {
    s[..pos].rfind('\n').map_or(0, |i| i + 1)
}

/// Set a component's top-level span (clause/event spans untouched).
fn set_component_span(component: &mut Component, span: Span) {
    match component {
        Component::Context(ctx) => ctx.span = Some(span),
        Component::Machine(machine) => machine.span = Some(span),
    }
}

/// Shift every span in a component by `delta` bytes. Used to translate
/// slice-relative spans from a per-region strict parse back into absolute
/// input positions. Recovered components carry no spans, so a `None` span
/// stays `None`.
fn shift_component_spans(component: &mut Component, delta: usize) {
    fn shift(span: &mut Option<Span>, delta: usize) {
        if let Some(s) = span {
            s.start += delta;
            s.end += delta;
        }
    }
    if delta == 0 {
        return;
    }
    match component {
        Component::Context(ctx) => {
            shift(&mut ctx.span, delta);
            shift(&mut ctx.name_span, delta);
            for set in &mut ctx.sets {
                shift(set.span_mut(), delta);
            }
            for constant in &mut ctx.constants {
                shift(&mut constant.span, delta);
            }
            for axiom in &mut ctx.axioms {
                shift(&mut axiom.span, delta);
            }
        }
        Component::Machine(machine) => {
            shift(&mut machine.span, delta);
            shift(&mut machine.name_span, delta);
            for variable in &mut machine.variables {
                shift(&mut variable.span, delta);
            }
            for invariant in &mut machine.invariants {
                shift(&mut invariant.span, delta);
            }
            if let Some(init) = &mut machine.initialisation {
                shift(&mut init.span, delta);
                for action in &mut init.actions {
                    shift(&mut action.span, delta);
                }
                for predicate in init.with.iter_mut().chain(&mut init.witnesses) {
                    shift(&mut predicate.span, delta);
                }
            }
            for event in &mut machine.events {
                shift(&mut event.span, delta);
                shift(&mut event.name_span, delta);
                for parameter in &mut event.parameters {
                    shift(&mut parameter.span, delta);
                }
                for predicate in event
                    .guards
                    .iter_mut()
                    .chain(&mut event.with)
                    .chain(&mut event.witnesses)
                {
                    shift(&mut predicate.span, delta);
                }
                for action in &mut event.actions {
                    shift(&mut action.span, delta);
                }
            }
        }
    }
}

/// Shift the 1-indexed line numbers in an error by `line_delta` lines.
/// Region slices start at line boundaries, so columns are unchanged.
fn offset_error_lines(error: ParseError, line_delta: usize) -> ParseError {
    if line_delta == 0 {
        return error;
    }
    match error {
        // The byte span is relative to the slice that produced the error; a
        // line delta can't translate byte offsets, so drop it and let consumers
        // fall back to the (shifted) line/column.
        ParseError::PestError {
            message,
            line,
            column,
            span: _,
        } => ParseError::PestError {
            message,
            line: line + line_delta,
            column,
            span: None,
        },
        ParseError::NestingTooDeep {
            limit,
            line,
            column,
        } => ParseError::NestingTooDeep {
            limit,
            line: line + line_delta,
            column,
        },
        ParseError::ReservedWord {
            word,
            line,
            column,
            span: _,
        } => ParseError::ReservedWord {
            word,
            line: line + line_delta,
            column,
            span: None,
        },
        ParseError::ClauseError {
            clause_type,
            line,
            column,
            message,
        } => ParseError::ClauseError {
            clause_type,
            line: line + line_delta,
            column,
            message,
        },
        ParseError::RecoverableError {
            line,
            column,
            message,
            span: _,
            source,
        } => ParseError::RecoverableError {
            line: line + line_delta,
            column,
            message,
            // Byte span is relative to the region slice; drop it like the
            // other span-bearing variants above.
            span: None,
            source: source.map(|e| Box::new(offset_error_lines(*e, line_delta))),
        },
        ParseError::MultipleErrors(errors) => ParseError::MultipleErrors(
            errors
                .into_iter()
                .map(|e| offset_error_lines(e, line_delta))
                .collect(),
        ),
        other => other,
    }
}

/// Attempt to parse a context with error recovery
fn parse_context_with_recovery(
    text: &RecoveryText,
    header_pos: usize,
    initial_error: ParseError,
) -> ParseResult<Component> {
    let mut errors = vec![initial_error];
    let mut context = Context::new(String::from("unknown"));

    // Try to extract the context name
    if let Some(name) = component_name_after(text, header_pos, "CONTEXT") {
        context.name = name;
    }

    // Try to parse each clause independently. Contexts have no event
    // section, so the clause scan is unbounded.
    let bound = text.masked.len();
    context.extends = recover_identifiers(text, "EXTENDS", bound);
    context.sets.extend(
        recover_identifiers(text, "SETS", bound)
            .into_iter()
            .map(|name| SetDeclaration::Deferred {
                name,
                comment: None,
                span: None,
            }),
    );
    context.constants = recover_identifiers(text, "CONSTANTS", bound)
        .into_iter()
        .map(NamedElement::new)
        .collect();
    context.axioms = recover_labeled_predicates(text, "AXIOMS", "axiom", bound, &mut errors);
    context
        .axioms
        .extend(recover_theorem_predicates(text, bound, &mut errors));

    dedup_recovered_errors(&mut errors);
    ParseResult::with_errors(Some(Component::Context(context)), errors)
}

/// Attempt to parse a machine with error recovery
fn parse_machine_with_recovery(
    text: &RecoveryText,
    header_pos: usize,
    initial_error: ParseError,
) -> ParseResult<Component> {
    let mut errors = vec![initial_error];
    let mut machine = Machine::new(String::from("unknown"));

    // Try to extract the machine name
    if let Some(name) = component_name_after(text, header_pos, "MACHINE") {
        machine.name = name;
    }

    // Try to parse each clause independently. Machine-level clauses all
    // precede the event section, so bound the scan by its start: an
    // event-level REFINES (or a guard that parses like an invariant) must
    // not be recovered as machine-level data.
    let bound = first_event_region_start(text);
    machine.refines = recover_identifiers(text, "REFINES", bound)
        .into_iter()
        .next();
    machine.sees = recover_identifiers(text, "SEES", bound);
    machine.variables = recover_identifiers(text, "VARIABLES", bound)
        .into_iter()
        .map(NamedElement::new)
        .collect();
    machine.invariants =
        recover_labeled_predicates(text, "INVARIANTS", "invariant", bound, &mut errors);
    machine
        .invariants
        .extend(recover_theorem_predicates(text, bound, &mut errors));

    // Note: VARIANT and EVENTS are more complex and would need specialized recovery
    // For now, we'll skip them if they fail to parse

    dedup_recovered_errors(&mut errors);
    ParseResult::with_errors(Some(Component::Machine(machine)), errors)
}

/// Drop a recovered-predicate error that merely re-flags a predicate the
/// strict parse already pinpointed. The strict failure (`errors[0]`) carries a
/// byte position at the exact offending token; a [`ParseError::RecoverableError`]
/// spanning the predicate that contains that token would underline it a second
/// time, so keep the precise strict error and discard the coarser duplicate.
/// Recovery errors for *other* broken predicates (which the strict parse never
/// reached) keep their span and survive.
fn dedup_recovered_errors(errors: &mut Vec<ParseError>) {
    let Some(strict) = errors.first().and_then(ParseError::span) else {
        return;
    };
    let mut is_strict = true;
    errors.retain(|e| {
        if is_strict {
            is_strict = false;
            return true; // never drop the strict error itself
        }
        !matches!(
            e,
            ParseError::RecoverableError { span: Some(s), .. }
                if s.start <= strict.start && strict.end <= s.end
        )
    });
}

/// Byte offset where the event section begins: the first whole-word
/// `EVENTS`/`EVENT`/`INITIALISATION`, or the end of the text if there is none.
fn first_event_region_start(text: &RecoveryText) -> usize {
    [
        KeywordId::Events,
        KeywordId::Event,
        KeywordId::Initialisation,
    ]
    .iter()
    .flat_map(|&id| crate::keywords::keyword(id).spellings)
    .filter_map(|spelling| find_keyword_word(text, spelling, 0, text.masked.len()))
    .min()
    .unwrap_or(text.masked.len())
}

/// Find `needle_upper` (an ASCII-uppercase keyword) in the uppercased
/// masked text, starting at `from` and strictly before `to`, only as a
/// whole word — `END` must not match inside `TREND` or `ENDPOINTS` — and
/// never inside a label (the spans precomputed in [`RecoveryText`]).
fn find_keyword_word(
    text: &RecoveryText,
    needle_upper: &str,
    from: usize,
    to: usize,
) -> Option<usize> {
    debug_assert!(
        !needle_upper.chars().any(|c| c.is_ascii_lowercase()),
        "needle must be ASCII-uppercase: {needle_upper}"
    );
    let upper = &text.masked_upper;
    let mut search = from;
    while let Some(pos) = upper[search..].find(needle_upper) {
        let offset = search + pos;
        if offset >= to {
            return None;
        }
        if let Some(label) = crate::comments::span_containing(&text.labels, offset) {
            search = label.end;
        } else if is_structural_word_bounded(upper, offset, needle_upper.len()) {
            return Some(offset);
        } else {
            // The needle is ASCII, so offset + 1 stays on a char boundary.
            search = offset + 1;
        }
    }
    None
}

/// If `line` starts with `keyword` as a whole word (case-insensitive),
/// return the rest of the line, trimmed.
fn strip_keyword_prefix<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = line.get(..keyword.len())?;
    if !rest.eq_ignore_ascii_case(keyword) || !is_structural_word_bounded(line, 0, keyword.len()) {
        return None;
    }
    Some(line[keyword.len()..].trim())
}

/// Extract the component name following its header keyword at `keyword_pos`
/// (a whole-word match in the masked text): the first identifier on the
/// rest of that line. Only a grammar-valid component name is accepted, so a
/// recovered AST never carries a name the pretty-printer cannot re-emit
/// (`MACHINE a--b`, `CONTEXT ä`); a malformed header simply leaves the
/// component's default `"unknown"` name.
fn component_name_after(text: &RecoveryText, keyword_pos: usize, keyword: &str) -> Option<String> {
    let rest = text.masked[keyword_pos + keyword.len()..].lines().next()?;
    extract_identifier(rest)
        .ok()
        .filter(|name| crate::names::is_valid_component_name(name))
}

/// Find the span between a clause keyword and the next clause or END,
/// looking only before `bound` (the event-region start for machine-level
/// clauses, end of text otherwise).
///
/// The offsets are valid in all three [`RecoveryText`] views (shared byte
/// layout). Clause keywords (both parameters and the boundary spellings from
/// the keyword table) are uppercase by convention, so no case mapping happens
/// here — which is also what keeps the offsets valid: Unicode `to_uppercase`
/// could change byte length (ß → SS).
fn extract_clause_content(text: &RecoveryText, clause_keyword: &str, bound: usize) -> Option<Span> {
    // Find the start of this clause
    let start = find_keyword_word(text, clause_keyword, 0, bound)?;

    // Find the end of this clause: the next clause keyword, THEOREMS, or END.
    // Each scan is capped at the best end found so far.
    let mut end = bound;
    for keyword in crate::keywords::recovery_boundary_spellings() {
        if keyword.eq_ignore_ascii_case(clause_keyword) {
            continue;
        }
        if let Some(pos) = find_keyword_word(text, keyword, start + clause_keyword.len(), end) {
            end = pos;
        }
    }

    Some(Span { start, end })
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

/// Parse a labeled predicate from a single line of text (used by error recovery)
///
/// Uses `labeled_predicate_complete` (with SOI/EOI) to ensure the entire input
/// is consumed. The span is cleared: it would be relative to the line, not
/// the document.
fn parse_labeled_predicate_str(input: &str) -> Result<LabeledPredicate, ParseError> {
    let depth = nesting::check_nesting(input)?;
    with_parser_stack(depth, || {
        let pairs = RossiParser::parse(Rule::labeled_predicate_complete, input)
            .map_err(|e| ParseError::from(Box::new(e)))?;

        let pair = pairs
            .into_iter()
            .next()
            .ok_or(ParseError::MissingPredicate)?;
        let mut result = parse_labeled_predicate(pair)?;
        result.span = None;
        Ok(result)
    })
}

/// Try to parse a labeled predicate from a single line of text
///
/// All grammar-defined forms (`@label P`, `theorem @label P`,
/// `@label theorem P`, bare `P` — including ASCII membership `c : S`) go
/// through the strict `labeled_predicate` rule, so recovery cannot drift
/// from the grammar (issue #24; this includes the trailing-colon label
/// spelling `@axm1: P`, where the colon belongs to the label). Only the
/// grammar-external `label: P` colon form is handled heuristically on top.
fn try_parse_labeled_predicate_from_text(text: &str) -> Result<LabeledPredicate, ParseError> {
    let text = text.trim();
    let strict_error = match parse_labeled_predicate_str(text) {
        Ok(result) => return Ok(result),
        Err(e) => e,
    };

    // "label:" form. Only reached for lines the grammar rejects outright:
    // a colon that is ASCII membership already parsed above.
    if !text.starts_with('@')
        && let Some(colon_pos) = text.find(':')
    {
        let potential_label = text[..colon_pos].trim();
        if !potential_label.is_empty()
            // Unicode labels are fine here (Rodin permits them); the ASCII
            // `is_word_char` predicate is only for keyword boundaries.
            && potential_label
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_')
            && let Ok(predicate) = parse_predicate_str(text[colon_pos + 1..].trim())
        {
            return Ok(LabeledPredicate {
                label: Some(potential_label.to_string()),
                is_theorem: false,
                predicate,
                span: None,
                comment: None,
            });
        }
    }

    Err(strict_error)
}
