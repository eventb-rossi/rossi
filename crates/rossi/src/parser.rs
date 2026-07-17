//! Rossi implementation
//!
//! This module provides the main parser interface using pest.

use pest::Parser;
use pest_derive::Parser;

use crate::ast::*;
use crate::deps::{ComponentKind, EdgeKind};
use crate::error::{ParseError, ParseResult};
use crate::nesting::{self, PARSER_STACK_SIZE, parser_stack_red_zone};
use crate::selection::SyntaxSnapshot;

/// Source, recovered AST, and owned syntax data from one revision.
#[derive(Debug)]
pub struct ParseSnapshot {
    source: String,
    result: ParseResult<Vec<Component>>,
    syntax: SyntaxSnapshot,
}

impl ParseSnapshot {
    /// Source text shared by every parsed and syntactic value in this snapshot.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The recovered components and parse errors from this snapshot.
    pub fn result(&self) -> &ParseResult<Vec<Component>> {
        &self.result
    }

    /// Return one enclosing-span chain per byte offset, in input order.
    pub fn enclosing_spans(&self, offsets: &[usize]) -> Vec<Vec<Span>> {
        self.syntax.enclosing_spans(&self.source, offsets)
    }

    /// Query signature syntax against this snapshot's own recovered errors.
    pub fn syntax_at_offset(&self, offset: usize) -> Option<crate::SyntaxAtOffset> {
        self.syntax.syntax_at_offset(
            &self.source,
            self.result.component.as_deref().unwrap_or_default(),
            &self.result.errors,
            offset,
        )
    }
}

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
/// with no terser spelling, so the caller keeps pest's own name.
///
/// Operator glyphs are sourced from the canonical [`OPERATOR_SPELLINGS`] table
/// via [`rule_to_operator_id`] + [`operators::spell`], so there is a single
/// source of truth. The explicit arms below are the deliberate exceptions:
///
/// * The Rodin private-use relations (`op_total_relation` &c.) and the
///   relational override store a private-use codepoint as their canonical
///   glyph, which renders as nothing in an error; we show the ASCII form
///   (`<+`) instead. `op_range_op` likewise reads better as `..` than the `‥` leader.
/// * Bracketing and separator tokens (`comma`, `lparen`, …) are not modelled
///   as operators, so they have no [`OperatorId`] to derive from.
///
/// [`OPERATOR_SPELLINGS`]: crate::operators::OPERATOR_SPELLINGS
/// [`operators::spell`]: crate::operators::spell
/// [`OperatorId`]: crate::operators::OperatorId
pub(crate) fn friendly_rule_name(rule: Rule) -> Option<&'static str> {
    Some(match rule {
        // Displayable spelling for operators whose canonical glyph (matching
        // the kernel-language spec code points) is an unrenderable Rodin
        // private-use code point — U+E100..=U+E103 — or the U+2025 `‥` leader.
        // Error messages show the ASCII form for the override's U+E103.
        Rule::op_overwrite => "<+",
        Rule::op_total_relation => "<<->",
        Rule::op_surjective_relation => "<->>",
        Rule::op_total_surjective_relation => "<<->>",
        Rule::op_range_op => "..",
        // Syntax tokens that are not modelled as operators.
        Rule::comma => ",",
        Rule::colon => ":",
        Rule::lparen => "(",
        Rule::rparen => ")",
        Rule::lbrace => "{",
        Rule::rbrace => "}",
        Rule::lbracket => "[",
        Rule::rbracket => "]",
        Rule::pipe => "|",
        _ => {
            // Keyword-headed rules render as their canonical spelling; otherwise
            // fall back to the operator table.
            if let Some(kw) = rule_to_keyword(rule) {
                return Some(crate::keywords::spell(kw));
            }
            return rule_to_operator_id(rule).map(|id| crate::operators::spell(id, true));
        }
    })
}

/// Owned display spelling for a [`Rule`], for diagnostic messages: the
/// [`friendly_rule_name`] glyph when there is one, else the debug name. Never
/// fails, so callers that name an operator in an error can rely on it.
pub(crate) fn display_rule(rule: Rule) -> String {
    friendly_rule_name(rule)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{rule:?}"))
}

/// Bridge a grammar [`Rule`] to its canonical [`OperatorId`], reusing the maps
/// already maintained for parsing (`rule_to_*`) and pretty-printing (`*_id`).
/// Returns `None` for rules that do not denote an operator. This is the link
/// that lets [`friendly_rule_name`] derive operator glyphs from the single
/// [`OPERATOR_SPELLINGS`] table.
///
/// [`OperatorId`]: crate::operators::OperatorId
/// [`OPERATOR_SPELLINGS`]: crate::operators::OPERATOR_SPELLINGS
fn rule_to_operator_id(rule: Rule) -> Option<crate::operators::OperatorId> {
    use crate::operators::{
        OperatorId, binary_op_id, comparison_op_id, logical_op_id, quantifier_id, unary_op_id,
    };
    if let Some(op) = rule_to_binary_op(rule) {
        return Some(binary_op_id(op));
    }
    if let Some(op) = rule_to_unary_op(rule) {
        return Some(unary_op_id(op));
    }
    if let Some(op) = rule_to_comparison_op(rule) {
        return Some(comparison_op_id(op));
    }
    if let Some(op) = rule_to_logical_op(rule) {
        return Some(logical_op_id(op));
    }
    if let Some(q) = rule_to_quantifier(rule) {
        return Some(quantifier_id(q));
    }
    // Operators that carry a spelling but no AST operator-enum representation.
    Some(match rule {
        Rule::op_not => OperatorId::Not,
        Rule::op_emptyset => OperatorId::EmptySet,
        Rule::op_becomes_equal => OperatorId::Assignment,
        Rule::op_becomes_in => OperatorId::BecomesIn,
        Rule::op_becomes_such => OperatorId::BecomesSuchThat,
        Rule::dot => OperatorId::Dot,
        _ => return None,
    })
}

/// Bridge a keyword-headed [`Rule`] to its [`KeywordId`], so a diagnostic names
/// the construct by its keyword (`EVENT`, `WHERE`, …) rather than the raw rule.
/// The token form (`kw_event`) and the clause form (`event`, `event_where`, …)
/// both resolve, so they collapse to one entry once de-duplicated. Returns `None`
/// for non-keyword rules, including the math-language atoms (`kw_true`,
/// `kw_int`, …) that are absent from the keyword table.
fn rule_to_keyword(rule: Rule) -> Option<KeywordId> {
    // Section rules reuse the clause→keyword maps maintained for parsing.
    if let Some(id) = machine_clause_keyword(rule).or_else(|| context_clause_keyword(rule)) {
        return Some(id);
    }
    // The whole-event rules carry no keyword spelling in their name.
    if matches!(rule, Rule::event | Rule::initialisation_event) {
        return Some(KeywordId::Event);
    }
    // A `kw_<spelling>` token or `event_<spelling>` clause rule (`event_where`,
    // `event_then`, …) resolves through the keyword table — the single source of
    // truth — keyed on its grammar name: pest renders a rule's `Debug` as that
    // name (the contract `display_rule` already relies on), and the table holds
    // the aliases (`when`→WHERE, `begin`→THEN). Math atoms (`kw_true`, `kw_int`,
    // …) and other non-keyword rules are absent from the table, so they yield
    // `None` and fall through to the operator path.
    let name = format!("{rule:?}");
    let spelling = name
        .strip_prefix("kw_")
        .or_else(|| name.strip_prefix("event_"))?;
    crate::keywords::lookup(spelling).map(|kw| kw.id)
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
    let span = Some(Span::from_pest(name_pair.as_span()));
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
    Ok(TypedIdentifier {
        name,
        type_expr,
        span,
    })
}

/// Collect typed identifiers from a quantifier, returning identifiers and the body predicate.
///
/// Shared by `negation_predicate` and `quantified_predicate` handlers.
fn collect_typed_identifiers_and_predicate(
    inner: &mut pest::iterators::Pairs<Rule>,
    bracketed: bool,
) -> Result<(Vec<TypedIdentifier>, Predicate), ParseError> {
    let mut identifiers = Vec::new();
    for p in inner.by_ref() {
        match p.as_rule() {
            Rule::typed_identifier => {
                identifiers.push(parse_typed_identifier(p)?);
            }
            Rule::predicate | Rule::predicate_no_semi => {
                // A quantifier body shares the enclosing closing bracket (if
                // any), so it inherits `bracketed`.
                let predicate = parse_predicate_inner(p, bracketed)?;
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
            // Skip the leading clause keyword (varies by call site)
            Rule::kw_extends
            | Rule::kw_constants
            | Rule::kw_variables
            | Rule::kw_refines
            | Rule::kw_sees
            | Rule::kw_any => {}
            _ => {
                return Err(ParseError::UnexpectedRule {
                    expected: "identifier".to_string(),
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
            // Skip the leading clause keyword (varies by call site)
            Rule::kw_constants | Rule::kw_variables | Rule::kw_any => {}
            _ => {
                return Err(ParseError::UnexpectedRule {
                    expected: "identifier".to_string(),
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
            _ => {
                return Err(ParseError::UnexpectedRule {
                    expected: "set_declaration".to_string(),
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

/// Reject a repeated section within a context or machine body.
///
/// Event-B has no structural syntax (Abrial), so sections may appear in any
/// order — but Rodin models each kind as a single container, so a section
/// keyword may appear at most once. This checks that multiplicity only; order is
/// unconstrained (the grammar still keeps the EVENTS block last). `seen`
/// accumulates the clause kinds already encountered in this body.
fn validate_unique_clause(
    rule: Rule,
    span: pest::Span,
    seen: &mut Vec<KeywordId>,
    keyword_fn: fn(Rule) -> Option<KeywordId>,
) -> Result<(), ParseError> {
    if let Some(keyword) = keyword_fn(rule) {
        if seen.contains(&keyword) {
            let name = crate::keywords::spell(keyword);
            let (line, col) = span.start_pos().line_col();
            return Err(ParseError::ClauseError {
                clause_type: name.to_string(),
                line,
                column: col,
                message: format!("Duplicate {} clause", name),
            });
        }
        seen.push(keyword);
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
    parse_components_guarded(input, |pair| components_from_pair(pair, input))
}

pub(crate) fn parse_components_guarded<T>(
    input: &str,
    build: impl FnOnce(pest::iterators::Pair<'_, Rule>) -> Result<T, ParseError>,
) -> Result<T, ParseError> {
    let depth = nesting::check_nesting(input)?;
    with_parser_stack(depth, || build(parse_components_pair(input)?))
}

fn parse_components_pair(input: &str) -> Result<pest::iterators::Pair<'_, Rule>, ParseError> {
    let pairs =
        RossiParser::parse(Rule::components, input).map_err(|e| ParseError::from(Box::new(e)))?;

    pairs
        .into_iter()
        .next()
        .ok_or_else(|| ParseError::UnexpectedRule {
            expected: "components".to_string(),
            found: "empty parse result".to_string(),
        })
}

fn components_from_pair(
    components_pair: pest::iterators::Pair<Rule>,
    input: &str,
) -> Result<Vec<Component>, ParseError> {
    let mut result = components_from_pair_unattached(components_pair)?;
    crate::comment_attach::attach_comments(input, &mut result);
    Ok(result)
}

fn components_from_pair_unattached(
    components_pair: pest::iterators::Pair<Rule>,
) -> Result<Vec<Component>, ParseError> {
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

/// Map a context clause rule to its header [`KeywordId`] (None for non-clause
/// rules such as `kw_end`).
fn context_clause_keyword(rule: Rule) -> Option<KeywordId> {
    match rule {
        Rule::context_clause_extends => Some(KeywordId::Extends),
        Rule::context_clause_sets => Some(KeywordId::Sets),
        Rule::context_clause_constants => Some(KeywordId::Constants),
        Rule::context_clause_axioms => Some(KeywordId::Axioms),
        Rule::context_clause_theorems => Some(KeywordId::Theorems),
        _ => None,
    }
}

/// Map a machine clause rule to its header [`KeywordId`] (None for non-clause
/// rules such as `kw_end`).
fn machine_clause_keyword(rule: Rule) -> Option<KeywordId> {
    match rule {
        Rule::machine_clause_refines => Some(KeywordId::Refines),
        Rule::machine_clause_sees => Some(KeywordId::Sees),
        Rule::machine_clause_variables => Some(KeywordId::Variables),
        Rule::machine_clause_invariants => Some(KeywordId::Invariants),
        Rule::machine_clause_theorems => Some(KeywordId::Theorems),
        Rule::machine_clause_variant => Some(KeywordId::Variant),
        Rule::machine_clause_events => Some(KeywordId::Events),
        _ => None,
    }
}

/// The span of `matched` (which starts at byte `start`) with trailing whitespace
/// dropped. A clause rule's span — strict (the pest rule absorbs whitespace up to
/// the next clause) or recovered (it runs to the next clause keyword) — would
/// otherwise carry the blank line(s) after its last member, so consumers record
/// it tight: ending on the last content character, not the following header.
fn trimmed_span(start: usize, matched: &str) -> Span {
    Span {
        start,
        end: start + matched.trim_end().len(),
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
    let mut seen_clauses: Vec<KeywordId> = Vec::new();
    for pair in inner {
        let pairs_to_process = if pair.as_rule() == Rule::context_body {
            pair.into_inner().collect::<Vec<_>>()
        } else {
            vec![pair]
        };

        for pair in pairs_to_process {
            validate_unique_clause(
                pair.as_rule(),
                pair.as_span(),
                &mut seen_clauses,
                context_clause_keyword,
            )?;

            // Record the clause's source region (header keyword through its last
            // member) so structural consumers can fold it without line scanning.
            if let Some(keyword) = context_clause_keyword(pair.as_rule()) {
                let span = pair.as_span();
                context.clauses.push(ClauseRegion::new(
                    keyword,
                    trimmed_span(span.start(), span.as_str()),
                ));
            }

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
    let mut seen_clauses: Vec<KeywordId> = Vec::new();
    for pair in inner {
        let pairs_to_process = if pair.as_rule() == Rule::machine_body {
            pair.into_inner().collect::<Vec<_>>()
        } else {
            vec![pair]
        };

        for pair in pairs_to_process {
            validate_unique_clause(
                pair.as_rule(),
                pair.as_span(),
                &mut seen_clauses,
                machine_clause_keyword,
            )?;

            // Record the clause's source region (header keyword through its last
            // member) so structural consumers can fold it without line scanning.
            if let Some(keyword) = machine_clause_keyword(pair.as_rule()) {
                let span = pair.as_span();
                machine.clauses.push(ClauseRegion::new(
                    keyword,
                    trimmed_span(span.start(), span.as_str()),
                ));
            }

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
                event.refines_span = Some(Span::from_pest(parent_pair.as_span()));
            }
        } else if peek.as_rule() == Rule::kw_refines {
            inner.next(); // consume kw_refines
            if let Some(parent_pair) = inner.next() {
                event.refines = Some(parent_pair.as_str().to_string());
                event.refines_span = Some(Span::from_pest(parent_pair.as_span()));
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
                    // `REFINES name` — capture the single target name and its span
                    // so a cursor on the target can navigate to the abstract event.
                    if let Some(p) = pair
                        .into_inner()
                        .find(|p| p.as_rule() == Rule::component_name)
                    {
                        event.refines = Some(p.as_str().to_string());
                        event.refines_span = Some(Span::from_pest(p.as_span()));
                    }
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
    let mut name_span = None;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::action_list => {
                actions = parse_action_list(p)?;
            }
            Rule::kw_extends => {
                extended = true;
            }
            // The INITIALISATION event has no identifier; its name is the
            // keyword itself, so record that token's span as the name span. An
            // extended init (`EVENT INITIALISATION extends INITIALISATION`) has
            // two such tokens — keep the first (the event's own name), not the
            // abstract event named after `extends`.
            Rule::kw_initialisation => {
                name_span.get_or_insert_with(|| Span::from_pest(p.as_span()));
            }
            Rule::kw_event | Rule::kw_then | Rule::kw_end => {}
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
        name_span,
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
    let action_span = Some(Span::from_pest(pair.as_span()));
    // Peek at the first inner token: if it is kw_skip this is a skip action.
    let mut inner = pair.into_inner().peekable();
    if inner.peek().map(|p| p.as_rule()) == Some(Rule::kw_skip) {
        return Ok(Action::new(ActionKind::Skip, action_span));
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
                let var_span = Some(Span::from_pest(p.as_span()));
                variables.push(Ident::new(declared_name(&p)?, var_span));
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
        // f(x) ≔ E  →  f ≔ f\u{E103}{x ↦ E}. Function override takes a single
        // argument (Rodin's FUNIMAGE is binary); a pair is the maplet `f(x ↦ y)`.
        use crate::ast::expression::BinaryOp;
        let function = variables
            .into_iter()
            .next()
            .ok_or(ParseError::MissingValue)?;
        let arg_pair = func_arg_pairs
            .into_iter()
            .next()
            .ok_or(ParseError::MissingValue)?;
        let domain = parse_expression(arg_pair)?;
        let rhs_pair = rhs_pairs
            .into_iter()
            .next()
            .ok_or(ParseError::MissingValue)?;
        let expression = parse_expression(rhs_pair)?;
        let maplet: Expression = ExpressionKind::Binary {
            op: BinaryOp::Maplet,
            left: Box::new(domain),
            right: Box::new(expression),
        }
        .into();
        let overwrite_rhs: Expression = ExpressionKind::Binary {
            op: BinaryOp::Overwrite,
            left: Box::new(ExpressionKind::Identifier(function.name.clone()).into()),
            right: Box::new(ExpressionKind::SetEnumeration(vec![maplet]).into()),
        }
        .into();
        return Ok(Action::new(
            ActionKind::Assignment {
                assignments: vec![(function, overwrite_rhs)],
            },
            action_span,
        ));
    }

    match op_pair.as_rule() {
        Rule::op_becomes_equal => {
            if rhs_pairs.is_empty() {
                return Err(ParseError::MissingValue);
            }
            if variables.len() != rhs_pairs.len() {
                let span = op_pair.as_span();
                let (line, column) = span.start_pos().line_col();
                return Err(ParseError::AssignmentArityMismatch {
                    targets: variables.len(),
                    expressions: rhs_pairs.len(),
                    line,
                    column,
                    span: Some(Span::from_pest(span)),
                });
            }
            let assignments = variables
                .into_iter()
                .zip(rhs_pairs)
                .map(|(variable, rhs)| Ok((variable, parse_expression(rhs)?)))
                .collect::<Result<Vec<_>, ParseError>>()?;
            Ok(Action::new(
                ActionKind::Assignment { assignments },
                action_span,
            ))
        }
        Rule::op_becomes_in => {
            let rhs = rhs_pairs
                .into_iter()
                .next()
                .ok_or(ParseError::MissingValue)?;
            let set = parse_expression(rhs)?;
            Ok(Action::new(
                ActionKind::BecomesIn { variables, set },
                action_span,
            ))
        }
        Rule::op_becomes_such => {
            let rhs = rhs_pairs
                .into_iter()
                .next()
                .ok_or(ParseError::MissingValue)?;
            let predicate = parse_predicate(rhs)?;
            Ok(Action::new(
                ActionKind::BecomesSuchThat {
                    variables,
                    predicate,
                },
                action_span,
            ))
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
    use crate::ast::expression::BinaryOp;
    use crate::op_info;

    let mut inner = pair.into_inner();

    // Get the first operand
    let first = inner.next().ok_or(ParseError::EmptyExpression)?;
    let mut left = parse_expression(first)?;

    // The operator binding the accumulated left operand (its root operator),
    // kept by `Rule` so the display spelling is built only on the error path.
    let set_level = op_info::binary_precedence(BinaryOp::Union);
    let mut prev: Option<(BinaryOp, Rule)> = None;

    // Process remaining (operator, operand) pairs
    while let Some(op_pair) = inner.next() {
        let op_rule = op_pair.as_rule();
        let op = rule_to_binary_op(op_rule).ok_or_else(|| ParseError::UnexpectedRule {
            expected: "binary operator".to_string(),
            found: format!("{op_rule:?}"),
        })?;

        // Reject set-level operators juxtaposed without the parentheses the
        // Event-B language requires (e.g. `A ∪ B ∩ C`). Only the set-operator
        // level has an incompatibility matrix: the other binary levels are
        // either freely mixing (arithmetic) or non-associative by grammar
        // (relation arrows, range, exponent), so they never reach this gate
        // carrying two operators.
        if op_info::binary_precedence(op) == set_level
            && let Some((prev_op, prev_rule)) = prev
            && !op_info::set_ops_acceptable(prev_op, op)
        {
            return Err(incompatible_operators(
                op_pair.as_span(),
                display_rule(prev_rule),
                display_rule(op_rule),
            ));
        }

        let right_pair = inner.next().ok_or(ParseError::EmptyExpression)?;
        let right = parse_expression(right_pair)?;
        // The folded node spans from the start of its left operand to the end
        // of its right operand — no single pest pair covers it.
        let span = fold_span(left.span, right.span);
        left = Expression::new(
            ExpressionKind::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            },
            span,
        );
        prev = Some((op, op_rule));
    }

    Ok(left)
}

/// Span covering a left operand's start through a right operand's end, when both
/// endpoints are known (left-associative operator folds).
fn fold_span(left: Option<Span>, right: Option<Span>) -> Option<Span> {
    match (left, right) {
        (Some(l), Some(r)) => Some(Span {
            start: l.start,
            end: r.end,
        }),
        _ => None,
    }
}

/// Build an expression for a bare `identifier` token. A reserved relational
/// atom (`id`, `prj1`, `prj2`, `pred`, `succ` — exact case) becomes the typed
/// [`ExpressionKind::AtomicBuiltin`]; every other word is an ordinary
/// identifier. Applying an atom (`prj1(x)`) is then handled by the surrounding
/// `function_application`, which wraps it in a `FunctionApplication` — matching
/// Rodin's atomic-expression + `FUNIMAGE` structure.
fn identifier_expression(name: &str, span: Option<Span>) -> Expression {
    match crate::ast::expression::AtomicBuiltinKind::from_name(name) {
        Some(kind) => Expression::new(ExpressionKind::AtomicBuiltin(kind), span),
        None => Expression::new(ExpressionKind::Identifier(name.to_string()), span),
    }
}

/// Build an [`ParseError::IncompatibleOperators`] anchored at the operator
/// `span` (the operator at which the incompatibility is detected). Called only
/// on the rejection path, so the `line_col` scan stays off the accepting path.
fn incompatible_operators(span: pest::Span<'_>, left: String, right: String) -> ParseError {
    let (line, column) = span.start_pos().line_col();
    ParseError::IncompatibleOperators {
        left,
        right,
        line,
        column,
        span: Some(Span::from_pest(span)),
    }
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
    // Span of the whole expression dispatched at this node. Leaf and structural
    // arms below attach it so every constructed node carries a source location.
    let node_span = Some(Span::from_pest(span));

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
                            Ok(Expression::new(
                                ExpressionKind::QuantifiedUnion {
                                    identifiers,
                                    predicate: Box::new(predicate),
                                    expression: Box::new(expression),
                                },
                                node_span,
                            ))
                        } else {
                            Ok(Expression::new(
                                ExpressionKind::QuantifiedInter {
                                    identifiers,
                                    predicate: Box::new(predicate),
                                    expression: Box::new(expression),
                                },
                                node_span,
                            ))
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
            Ok(Expression::new(
                ExpressionKind::Unary {
                    op,
                    operand: Box::new(operand),
                },
                node_span,
            ))
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
            Ok(Expression::new(
                ExpressionKind::Unary {
                    op,
                    operand: Box::new(parse_expression(operand_pair)?),
                },
                node_span,
            ))
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
            Ok(Expression::new(
                ExpressionKind::Lambda {
                    pattern,
                    predicate: Box::new(predicate),
                    expression: Box::new(expression),
                },
                node_span,
            ))
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
            if let ExpressionKind::Identifier(ref name) = base.kind
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
                    // Function application takes exactly one argument — Rodin's
                    // FUNIMAGE is binary (function, argument). The grammar admits
                    // a single expression between the parens; a pair is written
                    // with a maplet (`f(x ↦ y)`), never a comma list.
                    let argument =
                        if i < remaining.len() && remaining[i].as_rule() == Rule::expression {
                            let a = parse_expression(remaining[i].clone())?;
                            i += 1;
                            a
                        } else {
                            return Err(ParseError::EmptyExpression);
                        };

                    i += 1; // Skip rparen

                    // A closed builtin (card/min/max/union/inter) applied to its
                    // single argument; every other head is plain application.
                    if let ExpressionKind::Identifier(ref name) = result.kind
                        && let Some(builtin) =
                            crate::ast::expression::BuiltinFunction::from_name(name)
                    {
                        result = Expression::new(
                            ExpressionKind::BuiltinApplication {
                                function: builtin,
                                argument: Box::new(argument),
                            },
                            node_span,
                        );
                        continue;
                    }
                    result = Expression::new(
                        ExpressionKind::FunctionApplication {
                            function: Box::new(result),
                            argument: Box::new(argument),
                        },
                        node_span,
                    );
                } else if remaining[i].as_rule() == Rule::lbracket {
                    i += 1; // Skip lbracket
                    // Relational image: r[S]
                    if i < remaining.len() && remaining[i].as_rule() == Rule::expression {
                        let set = parse_expression(remaining[i].clone())?;
                        i += 1;
                        result = Expression::new(
                            ExpressionKind::RelationalImage {
                                relation: Box::new(result),
                                set: Box::new(set),
                            },
                            node_span,
                        );
                    }
                    // Skip rbracket
                    if i < remaining.len() && remaining[i].as_rule() == Rule::rbracket {
                        i += 1;
                    }
                } else if remaining[i].as_rule() == Rule::lbrace {
                    // Function update: f{x ↦ y, ...} == f <+ {x ↦ y, ...}.
                    // Rodin's static checker can emit this compact form for
                    // f(x) := y actions; we lower it to the same AST as the
                    // explicit <+ operator so semantic comparison converges.
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
                    result = Expression::new(
                        ExpressionKind::Binary {
                            op: crate::ast::expression::BinaryOp::Overwrite,
                            left: Box::new(result),
                            right: Box::new(Expression::new(
                                ExpressionKind::SetEnumeration(elements),
                                node_span,
                            )),
                        },
                        node_span,
                    );
                } else if remaining[i].as_rule() == Rule::op_inverse {
                    // Postfix inverse: r∼
                    result = Expression::new(
                        ExpressionKind::Unary {
                            op: crate::ast::expression::UnaryOp::Inverse,
                            operand: Box::new(result),
                        },
                        node_span,
                    );
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
                Rule::kw_bool_true => Ok(Expression::new(ExpressionKind::True, node_span)),
                Rule::kw_bool_false => Ok(Expression::new(ExpressionKind::False, node_span)),
                Rule::op_emptyset => Ok(Expression::new(ExpressionKind::EmptySet, node_span)),
                Rule::kw_nat => Ok(Expression::new(ExpressionKind::Naturals, node_span)),
                Rule::kw_nat1 => Ok(Expression::new(ExpressionKind::Naturals1, node_span)),
                Rule::kw_int => Ok(Expression::new(ExpressionKind::Integers, node_span)),
                Rule::bool_expr => {
                    // bool(P): extract the predicate child
                    let mut bool_inner = first.into_inner();
                    // skip kw_bool
                    bool_inner.next();
                    // skip lparen
                    bool_inner.next();
                    let pred_pair = bool_inner.next().ok_or(ParseError::MissingPredicate)?;
                    // bool(P) closes with `)`, so a trailing quantifier in P is bounded.
                    let predicate = parse_predicate_inner(pred_pair, true)?;
                    Ok(Expression::new(
                        ExpressionKind::Bool(Box::new(predicate)),
                        node_span,
                    ))
                }
                Rule::kw_bool_type => Ok(Expression::new(ExpressionKind::BoolType, node_span)),
                Rule::integer => {
                    let value = first
                        .as_str()
                        .parse::<i64>()
                        .map_err(|_| ParseError::InvalidInteger(first.as_str().to_string()))?;
                    Ok(Expression::new(ExpressionKind::Integer(value), node_span))
                }
                Rule::identifier => Ok(identifier_expression(first.as_str(), node_span)),
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
                    Ok(Expression::new(
                        ExpressionKind::SetEnumeration(elements),
                        node_span,
                    ))
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
                                return Ok(Expression::new(
                                    ExpressionKind::SetComprehension {
                                        identifiers,
                                        predicate: Box::new(
                                            predicate.ok_or(ParseError::MissingPredicate)?,
                                        ),
                                        expression: Some(Box::new(
                                            expression.ok_or(ParseError::EmptyExpression)?,
                                        )),
                                    },
                                    node_span,
                                ));
                            }
                            Rule::predicate => {
                                // Basic form: {x | P}. The predicate closes with
                                // `}`, so a trailing quantifier in P is bounded.
                                let predicate = parse_predicate_inner(p, true)?;
                                return Ok(Expression::new(
                                    ExpressionKind::SetComprehension {
                                        identifiers,
                                        predicate: Box::new(predicate),
                                        expression: None,
                                    },
                                    node_span,
                                ));
                            }
                            Rule::expression => {
                                // Expression form: {E | P}
                                // This is the third alternative in the grammar
                                let member_expression = parse_expression(p)?;
                                // Skip pipe, then parse predicate. The predicate
                                // closes with `}`, so a trailing quantifier is bounded.
                                let mut predicate = None;
                                for rest in iter.by_ref() {
                                    match rest.as_rule() {
                                        Rule::predicate => {
                                            predicate = Some(parse_predicate_inner(rest, true)?);
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
                                return Ok(Expression::new(
                                    ExpressionKind::SetBuilder {
                                        member_expression: Box::new(member_expression),
                                        predicate: Box::new(
                                            predicate.ok_or(ParseError::MissingPredicate)?,
                                        ),
                                    },
                                    node_span,
                                ));
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
            Ok(identifier_expression(pair.as_str(), node_span))
        }
        Rule::integer => {
            let value = pair
                .as_str()
                .parse::<i64>()
                .map_err(|_| ParseError::InvalidInteger(pair.as_str().to_string()))?;
            Ok(Expression::new(ExpressionKind::Integer(value), node_span))
        }
        _ => Err(ParseError::UnexpectedRule {
            expected: "expression".to_string(),
            found: format!("{:?} at {:?}", rule, span),
        }),
    }
}

/// Parse a predicate application (e.g., finite(S), partition(A, B, C))
fn parse_predicate_application(pair: pest::iterators::Pair<Rule>) -> Result<Predicate, ParseError> {
    let pred_span = Some(Span::from_pest(pair.as_span()));
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
        Ok(Predicate::new(
            PredicateKind::BuiltinApplication {
                predicate: builtin,
                arguments,
            },
            pred_span,
        ))
    } else if crate::builtins::is_reserved_word(&function) {
        // A reserved word applied where no builtin predicate resolves it:
        // the expression-only forms (`dom(x)`, `mod(x)`) and the generic
        // atoms (`pred(x)`, `id(x)` — expressions, never predicates).
        // Reject like Rodin instead of fabricating a user-defined predicate
        // application named by a reserved word.
        Err(reserved_word_error(&function, function_span))
    } else {
        Ok(Predicate::new(
            PredicateKind::Application {
                function: Ident::new(function, Some(Span::from_pest(function_span))),
                arguments,
            },
            pred_span,
        ))
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
    let pred_span = Some(Span::from_pest(pair.as_span()));
    let mut inner = pair.into_inner();
    let left_expr = parse_expression(inner.next().ok_or(ParseError::EmptyExpression)?)?;
    let op_pair = inner.next().ok_or(ParseError::MissingOperator)?;
    let right_expr = parse_expression(inner.next().ok_or(ParseError::EmptyExpression)?)?;

    let op =
        rule_to_comparison_op(op_pair.as_rule()).ok_or_else(|| ParseError::UnexpectedRule {
            expected: "comparison operator".to_string(),
            found: format!("{:?}", op_pair.as_rule()),
        })?;

    Ok(Predicate::new(
        PredicateKind::Comparison {
            op,
            left: left_expr,
            right: right_expr,
        },
        pred_span,
    ))
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

/// Whether `rule` is one of the binary-predicate precedence wrappers — the
/// `⇒`/`⇔` and `∧`/`∨` levels (and their `_no_semi` twins). These nest as a
/// chain of single-child wrappers down to `negation_predicate`; the dispatch in
/// [`parse_predicate_inner`] and the operand descent in [`leading_quantifier`]
/// both key off this set, so it lives in one place.
fn is_binary_predicate_wrapper(rule: Rule) -> bool {
    matches!(
        rule,
        Rule::implies_equiv_predicate
            | Rule::connective_predicate
            | Rule::implies_equiv_predicate_no_semi
            | Rule::connective_predicate_no_semi
    )
}

/// A bare, unparenthesised leading quantifier in `pair`, returned as its display
/// spelling. The operand sits one wrapper deep at each logical level (a
/// `negation_predicate` under `∧`/`∨`, a `connective_predicate` under `⇒`/`⇔`),
/// so descend through any binary-predicate wrappers to the leading
/// `negation_predicate` before inspecting it. A parenthesised `(∀x·P)` descends
/// through `atomic_predicate`, so its leading child is not a quantifier — only a
/// directly-quantified operand is reported.
fn leading_quantifier(pair: &pest::iterators::Pair<Rule>) -> Option<String> {
    let mut operand = pair.clone();
    while is_binary_predicate_wrapper(operand.as_rule()) {
        operand = operand.into_inner().next()?;
    }
    let first = operand.into_inner().next()?;
    match first.as_rule() {
        // `display_rule` never fails, so a matched quantifier always yields
        // `Some` — the gate fails closed rather than silently accepting.
        Rule::op_forall | Rule::op_exists => Some(display_rule(first.as_rule())),
        _ => None,
    }
}

/// Parse a binary logical predicate (conjunction/disjunction, implication,
/// equivalence). All operators in one call share a precedence level — `∧`/`∨`,
/// or `⇒`/`⇔` — and the same compatibility rules apply at both.
///
/// `bracketed` is true when this predicate is bounded on the right by a closing
/// bracket (`)` / `}`), which lets a trailing bare quantifier stand as an operand
/// — the bracket plays the role of the parentheses Event-B otherwise requires.
/// See [`parse_predicate_inner`].
fn parse_binary_predicate(
    pair: pest::iterators::Pair<Rule>,
    bracketed: bool,
) -> Result<Predicate, ParseError> {
    use crate::ast::predicate::LogicalOp;
    use crate::op_info;

    let mut inner = pair.into_inner();

    // Get the first operand
    let first = inner.next().ok_or(ParseError::EmptyPredicate)?;
    let mut left = parse_predicate_inner(first, bracketed)?;

    // The operator binding the accumulated left operand, kept by `Rule` so the
    // display spelling is built only on the error path.
    let mut prev: Option<(LogicalOp, Rule)> = None;

    // Process remaining (operator, operand) pairs
    while let Some(op_pair) = inner.next() {
        let op_rule = op_pair.as_rule();
        if let Some(op) = rule_to_logical_op(op_rule) {
            let right_pair = inner.next().ok_or(ParseError::EmptyPredicate)?;

            // Same-level compatibility gate, applied at both logical levels.
            // `∧`/`∨` may not be mixed; `⇒`/`⇔` may not be chained or mixed at
            // all (each is a non-associative singleton); and a bare quantifier
            // may not be an operand of any of them unless a closing bracket
            // bounds it — all otherwise need explicit parentheses.

            // Operators mixed without parentheses (checked when the operator is
            // reached, before its right operand — matching Rodin). No bracket
            // exception: a surrounding bracket never licenses a bare chain.
            if let Some((prev_op, prev_rule)) = prev
                && !op_info::logical_ops_compatible(prev_op, op)
            {
                return Err(incompatible_operators(
                    op_pair.as_span(),
                    display_rule(prev_rule),
                    display_rule(op_rule),
                ));
            }
            // A bare quantifier as the right operand, when no closing bracket
            // bounds it.
            if !bracketed && let Some(quantifier) = leading_quantifier(&right_pair) {
                return Err(incompatible_operators(
                    op_pair.as_span(),
                    display_rule(op_rule),
                    quantifier,
                ));
            }

            let right = parse_predicate_inner(right_pair, bracketed)?;
            let span = fold_span(left.span, right.span);
            left = Predicate::new(
                PredicateKind::Logical {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            );
            prev = Some((op, op_rule));
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
/// `implies_equiv_predicate` → `connective_predicate` → `negation_predicate`
/// → `atomic_predicate`, plus `_no_semi` twins)
/// produces a deeply nested Pair tree even for a simple comparison. We unwrap
/// single-child wrappers in a loop and only recurse for actual operators /
/// quantifiers / negation.
fn parse_predicate(pair: pest::iterators::Pair<Rule>) -> Result<Predicate, ParseError> {
    // Top-level entry: a formula attribute (axiom, guard, invariant, …) is not
    // bounded by a closing bracket, so a bare quantifier as a ∧/∨ operand is
    // rejected.
    parse_predicate_inner(pair, false)
}

/// Parse a predicate, tracking whether it is bounded on the right by a closing
/// bracket. A bare quantifier may be a ∧/∨ operand only when `bracketed` — the
/// `)`/`}` then stands in for the parentheses Event-B otherwise requires. The
/// flag propagates into quantifier bodies and connective operands; it is set on
/// entering `( … )`, `bool( … )`, and a `{ … ∣ P }` comprehension, and stays
/// false at top level and in `∣`-bounded such-that positions (λ, ⋃/⋂,
/// `{ x · P ∣ E }`).
fn parse_predicate_inner(
    pair: pest::iterators::Pair<Rule>,
    bracketed: bool,
) -> Result<Predicate, ParseError> {
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
            r if is_binary_predicate_wrapper(r) => {
                let mut probe = pair.clone().into_inner();
                let first = probe.next().ok_or(ParseError::EmptyPredicate)?;
                if probe.next().is_some() {
                    return parse_binary_predicate(pair, bracketed);
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
    // Span of the whole predicate dispatched at this node.
    let node_span = Some(Span::from_pest(pair.as_span()));

    match rule {
        Rule::negation_predicate | Rule::negation_predicate_no_semi => {
            let mut inner = pair.into_inner();
            let first = inner.next().ok_or(ParseError::EmptyPredicate)?;
            if first.as_rule() == Rule::op_not {
                let pred = parse_predicate_inner(
                    inner.next().ok_or(ParseError::EmptyPredicate)?,
                    bracketed,
                )?;
                Ok(Predicate::new(
                    PredicateKind::Not(Box::new(pred)),
                    node_span,
                ))
            } else if let Some(quantifier) = rule_to_quantifier(first.as_rule()) {
                // A quantified predicate appearing as a sub-formula operand,
                // e.g. a nested quantifier body. A bare quantifier directly
                // under a logical connective (`∧`/`∨`/`⇒`/`⇔`) is rejected
                // earlier in `parse_binary_predicate` unless a closing bracket
                // bounds it.
                let (identifiers, predicate) =
                    collect_typed_identifiers_and_predicate(&mut inner, bracketed)?;
                Ok(Predicate::new(
                    PredicateKind::Quantified {
                        quantifier,
                        identifiers,
                        predicate: Box::new(predicate),
                    },
                    node_span,
                ))
            } else {
                // Loop should have unwrapped the no-op alternative; defensive.
                parse_predicate_inner(first, bracketed)
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
            let (identifiers, predicate) =
                collect_typed_identifiers_and_predicate(&mut inner, bracketed)?;
            Ok(Predicate::new(
                PredicateKind::Quantified {
                    quantifier,
                    identifiers,
                    predicate: Box::new(predicate),
                },
                node_span,
            ))
        }
        Rule::atomic_predicate | Rule::atomic_predicate_no_semi => {
            let mut inner = pair.into_inner();
            let first = inner.next().ok_or(ParseError::EmptyPredicate)?;
            match first.as_rule() {
                Rule::kw_true => Ok(Predicate::new(PredicateKind::True, node_span)),
                Rule::kw_false => Ok(Predicate::new(PredicateKind::False, node_span)),
                // Only reachable for a parenthesised predicate, so it is bracketed.
                Rule::predicate => parse_predicate_inner(first, true),
                Rule::predicate_application => parse_predicate_application(first),
                Rule::comparison_predicate | Rule::comparison_predicate_no_semi => {
                    parse_comparison_predicate(first)
                }
                Rule::lparen => {
                    // Parenthesized predicate: lparen ~ predicate ~ rparen. The
                    // closing `)` bounds it, so a trailing quantifier is allowed.
                    let predicate_pair = inner.next().ok_or(ParseError::EmptyPredicate)?;
                    parse_predicate_inner(predicate_pair, true)
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
    let result = with_parser_stack(depth, || {
        let pairs = RossiParser::parse(Rule::predicate_complete, input)
            .map_err(|e| ParseError::from(Box::new(e)))?;

        let predicate_pair = pairs.into_iter().next().ok_or(ParseError::EmptyPredicate)?;
        parse_predicate(predicate_pair)
    });
    // A predicate that fails to parse but is really an assignment gets the
    // precise EB026 message instead of a generic formula error (the re-parse
    // runs its own stack guard, so it stays outside `with_parser_stack` above).
    result.map_err(|e| assignment_in_predicate_error(input).unwrap_or(e))
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

/// The six "becomes" (assignment) operator spellings — both the Unicode and
/// ASCII form of each of the three assignment operators — taken from the
/// operator table so this can't drift from the grammar (SSOT). No spelling is a
/// prefix of another, so scan order does not matter for correctness.
fn becomes_operators() -> [&'static str; 6] {
    use crate::operators::{OperatorId, spelling};
    let assign = spelling(OperatorId::Assignment); // ≔ / :=
    let in_ = spelling(OperatorId::BecomesIn); // :∈ / ::
    let such = spelling(OperatorId::BecomesSuchThat); // :∣ / :|
    [
        assign.unicode,
        assign.ascii,
        in_.unicode,
        in_.ascii,
        such.unicode,
        such.ascii,
    ]
}

/// Locate the first "becomes" operator (`≔`/`:=`, `:∈`/`::`, `:∣`/`:|`) in
/// `input`, returning it and its byte offset. Used to point EB026 at the
/// offending operator. Callers pass comment-free formula text (XML attribute
/// values, or comment-masked recovery segments), so no masking is needed here.
fn find_becomes_operator(input: &str) -> Option<(&'static str, usize)> {
    let operators = becomes_operators();
    input.char_indices().find_map(|(i, _)| {
        operators
            .into_iter()
            .find(|op| input[i..].starts_with(*op))
            .map(|op| (op, i))
    })
}

/// The EB026 discrimination: a formula that fails to parse as a predicate but
/// succeeds as an *action* is a misplaced assignment. Returns the offending
/// "becomes" operator and its byte offset in `input`, or `None` for a genuine
/// predicate error and for `skip` (an action carrying no operator).
///
/// The caller must already know the predicate parse failed — this only runs the
/// action re-parse and the operator scan.
fn becomes_operator_of_assignment(input: &str) -> Option<(&'static str, usize)> {
    if parse_action_str(input).is_err() {
        return None;
    }
    find_becomes_operator(input)
}

/// Strip an optional `@label` and/or leading `theorem` keyword from a recovered
/// predicate segment (any order: `@grd1 P`, `theorem @grd1 P`, `@grd1 theorem
/// P`, bare `P`), returning the body's byte offset within `content` and the body
/// text. Used to hand the bare formula to the EB026 assignment discrimination.
fn predicate_body_after_prefix(content: &str) -> (usize, &str) {
    let mut body = content.trim_start();
    loop {
        let before = body.len();
        if let Some(rest) = body.strip_prefix('@') {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            if end > 0 {
                body = rest[end..].trim_start();
            }
        }
        if starts_with_keyword(body, "theorem") {
            body = body["theorem".len()..].trim_start();
        }
        if body.len() == before {
            break;
        }
    }
    (subslice_offset(content, body), body)
}

/// Build a [`ParseError::AssignmentInPredicate`] (EB026) for a formula string
/// whose predicate parse failed, when it is really a misplaced assignment. The
/// `line`/`column`/`span` are relative to `input`. `None` when `input` is not an
/// assignment, so callers can fall back to the original error.
fn assignment_in_predicate_error(input: &str) -> Option<ParseError> {
    let (operator, offset) = becomes_operator_of_assignment(input)?;
    let (line, column) = offset_to_line_col(input, offset);
    Some(ParseError::AssignmentInPredicate {
        operator: operator.to_string(),
        line,
        column,
        span: Some(Span {
            start: offset,
            end: offset + operator.len(),
        }),
    })
}

// ============================================================================
// Error Recovery Functions
// ============================================================================

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

/// One recovered clause: the payload the strict parser would have produced for
/// it, plus its source region — both derived from a single [`clause_region`]
/// scan, so no caller re-scans a clause to record its region.
struct RecoveredClause<T> {
    /// The clause's source region, ready to push into a component's `clauses`;
    /// `None` when the clause keyword is absent.
    region: Option<ClauseRegion>,
    /// The recovered payload (declared names, labeled predicates, …).
    data: T,
}

type RecoveryPosition = (usize, KeywordId, usize);

impl<T> RecoveredClause<T> {
    /// A clause whose keyword was not found: no region, paired with whatever
    /// (empty) payload the caller has accumulated so far.
    fn absent(data: T) -> Self {
        Self { region: None, data }
    }
}

/// The raw content span and the line-tight [`ClauseRegion`] for one clause
/// keyword, or `None` when the keyword is absent.
///
/// Single source of truth for a clause's bounds during recovery: every path that
/// needs them — the data-recovery helpers ([`recover_identifiers`],
/// [`recover_labeled_predicates`]) and the variant scan — routes through here, so
/// [`extract_clause_content`] runs exactly once per clause. The raw span drives
/// data extraction (its untrimmed end is where line/segment scans stop); the
/// region carries the trimmed span folding/outline consume.
fn clause_region(
    text: &RecoveryText,
    keyword: KeywordId,
    bound: usize,
    positions: &[RecoveryPosition],
) -> Option<(Span, ClauseRegion)> {
    let raw = extract_clause_content(keyword, bound, positions)?;
    let region = ClauseRegion::new(
        keyword,
        trimmed_span(raw.start, &text.masked[raw.start..raw.end]),
    );
    Some((raw, region))
}

/// Whether `name` is acceptable as a declared identifier (parameter,
/// variable, constant, set carrier) in error-recovery output. Rejects
/// kernel_lang reserved words, matching [`declared_name`]. Structural keywords
/// remain valid when the grammar consumes them in an identifier position.
fn accepts_declared_name(name: &str) -> bool {
    crate::names::is_valid_math_identifier(name) && !crate::builtins::is_reserved_word(name)
}

/// Whether `name` is valid in the required first-name position after a clause
/// keyword. The grammar consumes this first name before applying its structural
/// keyword follow-set, so recovery must do the same before scanning for the next
/// clause boundary.
fn accepts_required_clause_name(keyword: KeywordId, name: &str) -> bool {
    match keyword {
        KeywordId::Sets | KeywordId::Constants | KeywordId::Variables | KeywordId::Any => {
            accepts_declared_name(name)
        }
        KeywordId::Extends | KeywordId::Refines | KeywordId::Sees => {
            crate::names::is_valid_component_name(name)
        }
        _ => false,
    }
}

/// The first non-whitespace token in `s`, with its document-relative span.
/// A single trailing comma is tolerated for recovery, but a bare leading comma
/// remains an invalid token rather than disappearing as an empty split item.
fn first_identifier_candidate(s: &str, base: usize) -> Option<(&str, Span)> {
    let candidate = s.trim_start().split(char::is_whitespace).next()?;
    let identifier = candidate.strip_suffix(',').unwrap_or(candidate);
    if identifier.is_empty() {
        return None;
    }
    let start = base + subslice_offset(s, identifier);
    Some((
        identifier,
        Span {
            start,
            end: start + identifier.len(),
        },
    ))
}

/// The required first name immediately following `content_start`, if it is
/// valid for `keyword`. Later tokens are deliberately not considered: an
/// invalid first token makes the strict grammar fail before reaching them.
fn required_clause_name_span(
    text: &RecoveryText,
    keyword: KeywordId,
    content_start: usize,
    bound: usize,
) -> Option<Span> {
    first_identifier_candidate(&text.masked[content_start..bound], content_start)
        .filter(|(name, _)| accepts_required_clause_name(keyword, name))
        .map(|(_, span)| span)
}

/// Extract the valid identifiers and source spans from one recovered clause.
fn recover_identifiers_from_span(
    text: &RecoveryText,
    keyword: KeywordId,
    span: Span,
) -> Vec<(String, Span)> {
    let spelling = crate::keywords::spell(keyword);
    let mut identifiers = Vec::new();

    for line in text.masked[span.start..span.end].lines() {
        let line = line.trim();
        let content = strip_keyword_prefix(line, spelling).unwrap_or(line);
        if content.is_empty() {
            continue;
        }
        // `content` is a subslice of `text.masked`; its byte offset there is
        // also its offset in the original (masking preserves byte layout).
        let base = subslice_offset(&text.masked, content);
        identifiers.extend(
            extract_identifiers(content, base)
                .into_iter()
                .filter(|(name, _)| accepts_required_clause_name(keyword, name)),
        );
    }

    identifiers
}

/// Extract identifiers (each with its source [`Span`]) from a clause during
/// error recovery. Also returns the clause's [`ClauseRegion`], recovered from the
/// same single [`clause_region`] scan.
///
/// Content written inline after the clause keyword (`VARIABLES x, y`) counts
/// like any other line. Declaring clauses (keyed off `keyword`, mirroring
/// `collect_identifiers_from_clause` on the strict path) drop reserved words:
/// the strict parse already reported them as ReservedWord errors, and keeping
/// them would hand downstream consumers (LSP completion, rename) a
/// declaration the parser itself forbids.
///
/// Spans are byte offsets into the recovery text (which shares its byte layout
/// with the original input the [`RecoveryText`] was built from), so they point
/// at the declared name exactly. Declaring clauses give navigation and symbol
/// providers a definition site even inside a component the strict parse
/// rejected; reference clauses (EXTENDS/SEES/REFINES) carry spans too, harmless
/// for callers that only want the names.
fn recover_identifiers(
    text: &RecoveryText,
    keyword: KeywordId,
    bound: usize,
    positions: &[RecoveryPosition],
) -> RecoveredClause<Vec<(String, Span)>> {
    // Keep only names the canonical validators accept in this position, so a
    // recovered AST stays round-trippable — whitespace-split recovery would
    // otherwise yield `a--b`/`x-y`, which the pretty-printer cannot re-emit.
    // Declaring clauses (SETS/CONSTANTS/VARIABLES) take mathematical
    // identifiers (and reject mathematical reserved words, mirroring the
    // strict path);
    // reference clauses (EXTENDS/SEES/REFINES) take component names.
    let Some((span, region)) = clause_region(text, keyword, bound, positions) else {
        return RecoveredClause::absent(Vec::new());
    };
    RecoveredClause {
        region: Some(region),
        data: recover_identifiers_from_span(text, keyword, span),
    }
}

/// Recover every occurrence of one component-level dependency clause.
///
/// The ordinary recovery AST keeps the first clause region because duplicate
/// clauses are invalid. Navigation and rename still need each operand's exact
/// location while the user is editing, so this source-location scan visits all
/// classified occurrences without changing the semantic AST.
fn recover_all_component_references(
    text: &RecoveryText,
    keyword: KeywordId,
    bound: usize,
    positions: &[RecoveryPosition],
) -> Vec<Ident> {
    let mut references = Vec::new();

    for (index, &(start, found, _)) in positions.iter().enumerate() {
        if found != keyword || start >= bound {
            continue;
        }
        let end = positions
            .get(index + 1)
            .map(|&(pos, _, _)| pos.min(bound))
            .unwrap_or(bound);
        references.extend(
            recover_identifiers_from_span(text, keyword, Span { start, end })
                .into_iter()
                .map(|(name, span)| Ident::new(name, Some(span))),
        );
    }

    references
}

/// Extract labeled predicates from a clause during error recovery. Also returns
/// the clause's [`ClauseRegion`], recovered from the same single [`clause_region`]
/// scan.
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
    keyword: KeywordId,
    label: &str,
    bound: usize,
    positions: &[RecoveryPosition],
    errors: &mut Vec<ParseError>,
) -> RecoveredClause<Vec<LabeledPredicate>> {
    let mut result = Vec::new();
    let Some((span, region)) = clause_region(text, keyword, bound, positions) else {
        return RecoveredClause::absent(result);
    };
    let spelling = crate::keywords::spell(keyword);

    // Segmentation: one `@`-label per segment (or per-line fallback). See
    // [`collect_segment_starts`] for the algorithm. The leading segment still
    // carries the clause keyword and is stripped here (the range-based variants
    // start at `content_start`, already past the keyword, so they don't need
    // this step).
    for pair in collect_segment_starts(text, span.start, span.end).windows(2) {
        let (seg_start, seg_end) = (pair[0], pair[1]);
        let raw = &text.masked[seg_start..seg_end];
        // Only the leading segment still carries the clause keyword.
        let content = if seg_start == span.start {
            strip_keyword_prefix(raw, spelling).unwrap_or_else(|| raw.trim())
        } else {
            raw.trim()
        };
        if content.is_empty() {
            continue;
        }
        // `content` is a subslice of `raw`; its absolute document offset (masked
        // and original share a byte layout).
        let abs_start = seg_start + subslice_offset(raw, content);
        match try_parse_labeled_predicate_from_text(content) {
            Ok(mut pred) => {
                // The segment was parsed in isolation with its span cleared:
                // anchor the span at the segment extent and lift the formula
                // spans to absolute document coordinates, as the AST visitor
                // does for the multi-component path.
                pred.span = Some(segment_span(abs_start, content));
                SpanShifter(abs_start).visit_predicate(&mut pred.predicate);
                result.push(pred);
            }
            Err(e) => push_recovery_error(errors, text, abs_start, content, label, e),
        }
    }
    RecoveredClause {
        region: Some(region),
        data: result,
    }
}

/// The label-inclusive source span of a recovered clause segment: the trimmed
/// `content` begins at `abs_start` and occupies `content.len()` bytes, so this
/// anchors the recovered predicate/action in the document outline and folding.
fn segment_span(abs_start: usize, content: &str) -> Span {
    Span {
        start: abs_start,
        end: abs_start + content.len(),
    }
}

/// Recover labeled items from the `@`-label (or per-line) segments of the byte
/// range `[from, to)`. `parse_segment` parses one trimmed segment, given its
/// absolute start, into a fully-spanned node; segments that fail to parse are
/// logged to `errors` (tagged `label`) and skipped. Shared by the predicate and
/// action range recoverers.
fn recover_clause_in_range<T>(
    text: &RecoveryText,
    from: usize,
    to: usize,
    label: &str,
    errors: &mut Vec<ParseError>,
    mut parse_segment: impl FnMut(&str, usize) -> Result<T, ParseError>,
) -> Vec<T> {
    // `clause_content_range` can hand back content_start > content_end when a
    // clause keyword straddles `to`; bail before collect_segment_starts slices.
    if from >= to {
        return Vec::new();
    }
    let mut result = Vec::new();
    for pair in collect_segment_starts(text, from, to).windows(2) {
        let (seg_start, seg_end) = (pair[0], pair[1]);
        let raw = &text.masked[seg_start..seg_end];
        let content = raw.trim();
        if content.is_empty() {
            continue;
        }
        // `content` is a subslice of `raw`; its absolute document offset (masked
        // and original share a byte layout).
        let abs_start = seg_start + subslice_offset(raw, content);
        match parse_segment(content, abs_start) {
            Ok(node) => result.push(node),
            Err(e) => push_recovery_error(errors, text, abs_start, content, label, e),
        }
    }
    result
}

/// Recover labeled predicates from an explicit `[from, to)` byte range.
///
/// Unlike [`recover_labeled_predicates`], which searches for a clause keyword,
/// this operates on a pre-computed content range. Used by [`recover_events`]
/// for per-event WHERE/WITH/WITNESS clause recovery, where the same keyword
/// can appear once per event.
///
/// Broken segments are logged to `errors` and skipped; the caller receives
/// only predicates whose formulas parsed successfully, so
/// `LabeledPredicate.predicate` is always populated.
fn recover_predicates_in_range(
    text: &RecoveryText,
    from: usize,
    to: usize,
    label: &str,
    errors: &mut Vec<ParseError>,
) -> Vec<LabeledPredicate> {
    recover_clause_in_range(text, from, to, label, errors, |content, abs_start| {
        let mut pred = try_parse_labeled_predicate_from_text(content)?;
        // The parsed span was cleared (segment-relative); anchor it at the
        // label-inclusive segment extent, then lift the formula spans.
        pred.span = Some(segment_span(abs_start, content));
        SpanShifter(abs_start).visit_predicate(&mut pred.predicate);
        Ok(pred)
    })
}

/// The byte offset where the labeled predicate at `label_start` begins, so its
/// recovery segment stays grammar-whole. Normally the label itself, but pulled
/// back over a `theorem` keyword that immediately precedes the label (the
/// grammar's `kw_theorem ~ label`, e.g. `theorem @grd1 …`). Never scans before
/// `floor`, the clause-keyword offset. `masked` is the comment-masked text the
/// label offsets index; the returned offset is valid in all [`RecoveryText`]
/// views (shared byte layout).
/// Build the segmentation start-points for the byte range `[from, to)`.
///
/// Returns a `Vec` whose first element is `from`, whose last element is `to`,
/// and whose interior elements are one entry per `@`-label inside the range
/// (each pulled back over a preceding `theorem` keyword via
/// [`predicate_start_for_label`]). When the range has no labels the fallback
/// is a per-line split, matching the behaviour of [`recover_labeled_predicates`].
///
/// Pass the result directly to `starts.windows(2)` to iterate segments.
fn collect_segment_starts(text: &RecoveryText, from: usize, to: usize) -> Vec<usize> {
    let span = Span {
        start: from,
        end: to,
    };
    let mut starts = vec![from];
    let mut label_iter = text
        .labels
        .iter()
        .filter(|lbl| span.contains(lbl.start))
        .map(|lbl| predicate_start_for_label(&text.masked, from, lbl.start))
        .peekable();
    if label_iter.peek().is_none() {
        let mut line_start = from;
        for line in text.masked[from..to].split_inclusive('\n') {
            if line_start != from {
                starts.push(line_start);
            }
            line_start += line.len();
        }
    } else {
        starts.extend(label_iter);
    }
    starts.push(to);
    starts
}

/// Append a [`ParseError::RecoverableError`] for a segment that failed to
/// parse. `label` is the clause name used in the message (e.g. `"guard"` or
/// `"action"`). `abs_start` and `content` are the absolute byte offset and
/// trimmed text of the failing segment; `source` is the underlying parse error.
fn push_recovery_error(
    errors: &mut Vec<ParseError>,
    text: &RecoveryText,
    abs_start: usize,
    content: &str,
    label: &str,
    source: ParseError,
) {
    // A predicate clause (invariant/guard/witness/axiom — never an action) that
    // is really a misplaced assignment gets the precise EB026 error instead of a
    // generic recovery error, matching the strict `parse_predicate_str` path.
    if label != "action" {
        let (body_offset, body) = predicate_body_after_prefix(content);
        if let Some((operator, rel)) = becomes_operator_of_assignment(body) {
            let op_start = abs_start + body_offset + rel;
            let (line, column) = offset_to_line_col(text.original, op_start);
            errors.push(ParseError::AssignmentInPredicate {
                operator: operator.to_string(),
                line,
                column,
                span: Some(Span {
                    start: op_start,
                    end: op_start + operator.len(),
                }),
            });
            return;
        }
    }
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
        source: Some(Box::new(source)),
    });
}

fn predicate_start_for_label(masked: &str, floor: usize, label_start: usize) -> usize {
    let prefix = masked[floor..label_start].trim_end();
    // Start of the last whitespace-separated token, measured as its byte length
    // back from the end of `prefix`. `rsplit` cuts on char boundaries, so a
    // multibyte whitespace — a no-break space, a line separator — never lands the
    // offset mid-char.
    let last_token = prefix.rsplit(char::is_whitespace).next().unwrap_or(prefix);
    let last_token_start = floor + (prefix.len() - last_token.len());
    if starts_with_keyword(&masked[last_token_start..label_start], "theorem") {
        last_token_start
    } else {
        label_start
    }
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
/// Carries through the [`ClauseRegion`] from [`recover_labeled_predicates`].
fn recover_theorem_predicates(
    text: &RecoveryText,
    bound: usize,
    positions: &[RecoveryPosition],
    errors: &mut Vec<ParseError>,
) -> RecoveredClause<Vec<LabeledPredicate>> {
    let mut recovered = recover_labeled_predicates(
        text,
        KeywordId::Theorems,
        "theorem",
        bound,
        positions,
        errors,
    );
    for p in &mut recovered.data {
        p.is_theorem = true;
    }
    recovered
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
            let context_pos =
                find_keyword_word(&text, crate::keywords::spell(KeywordId::Context), 0, end);
            let machine_pos =
                find_keyword_word(&text, crate::keywords::spell(KeywordId::Machine), 0, end);
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
        Err(first_error) => recover_components_after_error(input, first_error),
    }
}

/// Parse components and retain the owned syntax hierarchy for the same source.
///
/// On a strict parse, the AST and syntax hierarchy are both derived from one
/// Pest parse. Error recovery remains identical to
/// [`parse_components_with_recovery`]; a failed whole-document grammar parse
/// has no selection hierarchy, but lexical spans and recovered AST nodes remain
/// available for offset queries.
pub fn parse_components_snapshot(input: impl Into<String>) -> ParseSnapshot {
    let source = input.into();
    let strict = parse_components_guarded(&source, |components_pair| {
        let lexical = crate::comments::lexical_spans(&source);
        let mut components = components_from_pair_unattached(components_pair.clone())?;
        crate::comment_attach::attach_comments_from_spans(
            &source,
            &mut components,
            &lexical.comments,
        );
        let syntax = SyntaxSnapshot::from_pair_with_lexical(components_pair, lexical);
        Ok((components, syntax))
    });
    let (result, syntax) = match strict {
        Ok((components, syntax)) => (ParseResult::ok(components), syntax),
        Err(first_error @ ParseError::NestingTooDeep { .. }) => (
            ParseResult::err(first_error),
            SyntaxSnapshot::empty(&source),
        ),
        Err(first_error) => (
            recover_components_after_error(&source, first_error),
            SyntaxSnapshot::empty(&source),
        ),
    };
    ParseSnapshot {
        source,
        result,
        syntax,
    }
}

fn recover_components_after_error(
    input: &str,
    first_error: ParseError,
) -> ParseResult<Vec<Component>> {
    let text = RecoveryText::new(input);
    let headers = component_header_starts(&text);
    if headers.len() < 2 {
        // Zero or one component header: single-component recovery already
        // handles this exactly (including the no-header case).
        let result = parse_with_recovery(input);
        return ParseResult::with_errors(result.component.map(|c| vec![c]), result.errors);
    }

    // Region i runs from its header's line start to the next header's line
    // start; the first region extends back to offset 0 so junk before the first
    // header is still reported.
    let mut starts: Vec<usize> = headers
        .iter()
        .map(|&pos| line_start(&text.masked, pos))
        .collect();
    starts[0] = 0;

    let mut components = Vec::new();
    let mut errors = Vec::new();
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
                .map(|error| error.shift_location(start, line_delta)),
        );
        if let Some(mut component) = result.component {
            SpanShifter(start).visit_component(&mut component);
            components.push(component);
        }
    }

    if components.is_empty() {
        return ParseResult::err(first_error);
    }
    if errors.is_empty() {
        errors.push(first_error);
    }
    ParseResult::with_errors(Some(components), errors)
}

/// Byte offsets of every line-anchored, whole-word `CONTEXT`/`MACHINE`
/// header in the comment-masked text, in source order. Line-anchoring (only
/// whitespace before the keyword on its line) is what keeps a mid-line
/// mention — an identifier in a guard, say — from splitting a region.
fn component_header_starts(text: &RecoveryText) -> Vec<usize> {
    let mut starts = Vec::new();
    let end = text.masked.len();
    let protected_names: Vec<Span> = [
        crate::keywords::scope::CONTEXT,
        crate::keywords::scope::MACHINE,
    ]
    .into_iter()
    .flat_map(|scope| recovery_clause_positions(text, scope, 0, end))
    .filter_map(|(pos, keyword, len)| required_clause_name_span(text, keyword, pos + len, end))
    .collect();
    for keyword in [KeywordId::Context, KeywordId::Machine] {
        let spelling = crate::keywords::spell(keyword);
        let mut from = 0;
        while let Some(pos) = find_keyword_word(text, spelling, from, end) {
            if is_line_anchored(&text.masked, pos)
                && !protected_names.iter().any(|span| span.start == pos)
            {
                starts.push(pos);
            }
            from = pos + spelling.len();
        }
    }
    starts.sort_unstable();
    starts
}

/// The structural role of one component-name occurrence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentNameSite {
    /// A `CONTEXT` or `MACHINE` declaration name.
    Declaration(ComponentKind),
    /// An `EXTENDS`, `SEES`, or `REFINES` target.
    Dependency(EdgeKind),
}

/// One exact component declaration or dependency operand in source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentNameOccurrence {
    pub name: String,
    pub span: Option<Span>,
    pub site: ComponentNameSite,
}

impl ComponentNameOccurrence {
    fn new(name: String, span: Option<Span>, site: ComponentNameSite) -> Self {
        Self { name, span, site }
    }
}

/// Locate component declarations and component-level dependency operands.
///
/// This compatibility view retains the original [`Ident`] result. Consumers
/// that need each occurrence's structural role use
/// [`component_name_occurrences_with_sites`].
pub fn component_name_occurrences(input: &str) -> Vec<Ident> {
    component_name_occurrences_with_sites(input)
        .into_iter()
        .map(|occurrence| Ident::new(occurrence.name, occurrence.span))
        .collect()
}

/// Locate component declarations and component-level dependency operands,
/// retaining whether each name is a declaration or dependency target.
///
/// The result contains the names after `CONTEXT` / `MACHINE` headers and every
/// target in context `EXTENDS` or machine `REFINES` / `SEES` clauses. Event
/// refinement targets are excluded. The recovery scanner is used directly, so
/// locations remain available in syntactically broken components and repeated
/// structural clauses without adding source-only fields to the public AST.
pub fn component_name_occurrences_with_sites(input: &str) -> Vec<ComponentNameOccurrence> {
    let text = RecoveryText::new(input);
    let headers = component_header_starts(&text);
    let mut occurrences = Vec::new();

    for (index, &header) in headers.iter().enumerate() {
        let end = headers.get(index + 1).copied().unwrap_or(input.len());
        let (keyword, scope, kind, dependencies) = if text.masked_upper[header..]
            .starts_with(crate::keywords::spell(KeywordId::Context))
        {
            (
                KeywordId::Context,
                crate::keywords::scope::CONTEXT,
                ComponentKind::Context,
                &[(KeywordId::Extends, EdgeKind::Extends)][..],
            )
        } else {
            (
                KeywordId::Machine,
                crate::keywords::scope::MACHINE,
                ComponentKind::Machine,
                &[
                    (KeywordId::Refines, EdgeKind::Refines),
                    (KeywordId::Sees, EdgeKind::Sees),
                ][..],
            )
        };

        if let Some((name, span)) = component_name_after(&text, header, keyword)
            && span.end <= end
        {
            occurrences.push(ComponentNameOccurrence::new(
                name,
                Some(span),
                ComponentNameSite::Declaration(kind),
            ));
        }

        let positions = recovery_clause_positions(&text, scope, header, end);
        let bound = if keyword == KeywordId::Machine {
            first_event_region_start(&text, &positions).min(end)
        } else {
            end
        };
        for &(dependency, edge) in dependencies {
            occurrences.extend(
                recover_all_component_references(&text, dependency, bound, &positions)
                    .into_iter()
                    .map(|occurrence| {
                        ComponentNameOccurrence::new(
                            occurrence.name,
                            occurrence.span,
                            ComponentNameSite::Dependency(edge),
                        )
                    }),
            );
        }
    }

    occurrences.sort_by_key(|ident| ident.span.map(|span| (span.start, span.end)));
    occurrences.dedup_by(|left, right| left.name == right.name && left.span == right.span);
    occurrences
}

/// Byte offset of the start of the line containing `pos`.
pub(crate) fn line_start(s: &str, pos: usize) -> usize {
    s[..pos].rfind('\n').map_or(0, |i| i + 1)
}

/// Shifts every span reached through the default mutable AST traversal.
struct SpanShifter(usize);

impl VisitMut for SpanShifter {
    fn visit_span(&mut self, span: &mut Span) {
        span.shift(self.0);
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
    if let Some((name, span)) = component_name_after(text, header_pos, KeywordId::Context) {
        context.name = name;
        context.name_span = Some(span);
    }

    // Try to parse each clause independently. Contexts have no event
    // section, so the clause scan is unbounded.
    let bound = text.masked.len();
    let scope = crate::keywords::scope::CONTEXT;
    let positions = recovery_clause_positions(text, scope, 0, bound);
    let extends = recover_identifiers(text, KeywordId::Extends, bound, &positions);
    context.extends = extends.data.into_iter().map(|(name, _)| name).collect();
    let sets = recover_identifiers(text, KeywordId::Sets, bound, &positions);
    context.sets.extend(
        sets.data
            .into_iter()
            .map(|(name, span)| SetDeclaration::Deferred {
                name,
                comment: None,
                span: Some(span),
            }),
    );
    let constants = recover_identifiers(text, KeywordId::Constants, bound, &positions);
    context.constants = constants
        .data
        .into_iter()
        .map(|(name, span)| NamedElement::with_span(name, span))
        .collect();
    let axioms = recover_labeled_predicates(
        text,
        KeywordId::Axioms,
        "axiom",
        bound,
        &positions,
        &mut errors,
    );
    context.axioms = axioms.data;
    let theorems = recover_theorem_predicates(text, bound, &positions, &mut errors);
    context.axioms.extend(theorems.data);

    // Record each clause's source region (folding/outline consume these even in
    // a component the strict parse rejected), recovered from the same scans
    // above, in declaration order.
    context.clauses = [
        extends.region,
        sets.region,
        constants.region,
        axioms.region,
        theorems.region,
    ]
    .into_iter()
    .flatten()
    .collect();

    // Span the component from its header through its last non-blank content, so
    // block-level span consumers (folding, component-at-offset) anchor it even
    // when the strict parse failed. In a merged file this slice-relative span is
    // shifted to absolute coordinates by `shift_component_spans`.
    context.span = Some(Span {
        start: header_pos,
        end: text.masked.trim_end().len(),
    });

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
    if let Some((name, span)) = component_name_after(text, header_pos, KeywordId::Machine) {
        machine.name = name;
        machine.name_span = Some(span);
    }

    // Try to parse each clause independently. Machine-level clauses all
    // precede the event section, so bound the scan by its start: an
    // event-level REFINES (or a guard that parses like an invariant) must
    // not be recovered as machine-level data.
    let scope = crate::keywords::scope::MACHINE;
    let positions = recovery_clause_positions(text, scope, 0, text.masked.len());
    let bound = first_event_region_start(text, &positions);
    let refines = recover_identifiers(text, KeywordId::Refines, bound, &positions);
    machine.refines = refines.data.into_iter().next().map(|(name, _)| name);
    let sees = recover_identifiers(text, KeywordId::Sees, bound, &positions);
    machine.sees = sees.data.into_iter().map(|(name, _)| name).collect();
    let variables = recover_identifiers(text, KeywordId::Variables, bound, &positions);
    machine.variables = variables
        .data
        .into_iter()
        .map(|(name, span)| NamedElement::with_span(name, span))
        .collect();
    let invariants = recover_labeled_predicates(
        text,
        KeywordId::Invariants,
        "invariant",
        bound,
        &positions,
        &mut errors,
    );
    machine.invariants = invariants.data;
    let theorems = recover_theorem_predicates(text, bound, &positions, &mut errors);
    machine.invariants.extend(theorems.data);

    // The variant has no data-recovery helper; record its region from the same
    // single-scan source of truth as the clauses above (a `ClauseRegion` is
    // `Copy`, so this stays usable after it is collected into `clauses`).
    let variant =
        clause_region(text, KeywordId::Variant, bound, &positions).map(|(_, region)| region);

    // Best-effort recovery of the variant expression (for the outline): the
    // region's content past the `VARIANT` keyword, if it parses.
    if let Some(region) = variant {
        let kw_len = crate::keywords::spell(KeywordId::Variant).len();
        let body = text.masked[region.span.start + kw_len..region.span.end].trim();
        if !body.is_empty()
            && let Ok(expr) = parse_expression_str(body)
        {
            machine.variant = Some(expr);
        }
    }

    // Record each machine-level clause's source region (bounded by the event
    // region, like the declaration scans above), recovered from the same scans,
    // in declaration order. The EVENTS region is appended below.
    machine.clauses = [
        refines.region,
        sees.region,
        variables.region,
        invariants.region,
        theorems.region,
        variant,
    ]
    .into_iter()
    .flatten()
    .collect();

    // Recover the events (span-only) and the EVENTS clause region. The region
    // runs from the event-section start through the last recovered event's END
    // (matching the strict parse, where it ends at the last event, not the
    // machine END).
    let (initialisation, events, events_end) = recover_events(text, bound, &mut errors);
    machine.initialisation = initialisation;
    machine.events = events;
    if let Some(end) = events_end
        && end > bound
    {
        machine.clauses.push(ClauseRegion::new(
            KeywordId::Events,
            Span { start: bound, end },
        ));
    }

    // Span the component from its header through its last non-blank content, so
    // block-level span consumers (folding, component-at-offset) anchor it even
    // when the strict parse failed. In a merged file this slice-relative span is
    // shifted to absolute coordinates by `shift_component_spans`.
    machine.span = Some(Span {
        start: header_pos,
        end: text.masked.trim_end().len(),
    });

    dedup_recovered_errors(&mut errors);
    ParseResult::with_errors(Some(Component::Machine(machine)), errors)
}

/// Reconcile the strict failure (`errors[0]`) with the recovered-predicate
/// errors so a single broken predicate is flagged once, in the right place.
///
/// Two cases turn on where the strict error's byte position falls relative to
/// the recovered-predicate spans:
///
/// * **Inside** a recovered predicate's span — the strict parse stopped on the
///   exact offending token. Keep that precise position and drop the coarser
///   [`ParseError::RecoverableError`] that re-flags the same predicate.
/// * **Past** a recovered predicate, landing in a *later* one — a trailing
///   operator (`@a x ∈` with nothing after `∈`) makes the strict parser consume
///   across the newline into the next predicate's `@label`, so its position
///   points at an innocent predicate. Recovery already pinpointed the real
///   culprit with a byte-exact segment span, so drop the misleading strict
///   error and keep the recovered ones.
///
/// Recovery errors for predicates the strict parse never reached always survive.
fn dedup_recovered_errors(errors: &mut Vec<ParseError>) {
    // A strict parse and per-action recovery both see an arity mismatch. Keep
    // the recovery envelope: its source retains the precise count error while
    // its outer span bounds the whole malformed action for editor fallbacks.
    if let Some(strict_span) = errors.first().and_then(|error| match error {
        ParseError::AssignmentArityMismatch {
            span: Some(span), ..
        } => Some(*span),
        _ => None,
    }) {
        let matching_recovery = errors.iter().skip(1).any(|error| {
            let ParseError::RecoverableError {
                span: Some(recovery_span),
                source: Some(source),
                ..
            } = error
            else {
                return false;
            };
            matches!(
                source.as_ref(),
                ParseError::AssignmentArityMismatch {
                    span: Some(span),
                    ..
                } if (Span {
                        start: recovery_span.start + span.start,
                        end: recovery_span.start + span.end,
                    }) == strict_span
            )
        });
        if matching_recovery {
            errors.remove(0);
            return;
        }
    }

    let Some(strict) = errors.first().and_then(ParseError::span) else {
        return;
    };
    // Whether a recovered predicate's span encloses the strict error's position.
    let contains_strict = |s: &Span| s.start <= strict.start && strict.end <= s.end;

    // A misplaced-assignment recovery error (EB026) is the precise, actionable
    // report for a `:=`-in-predicate: the strict parser instead trips a little
    // further along (it reads `:` as ASCII membership, then rejects the `=`), so
    // its position is misleading. Whenever the EB026 operator span covers the
    // strict error, drop the strict error and keep the EB026.
    let eb026_covers_strict = errors.iter().skip(1).any(|e| {
        matches!(
            e,
            ParseError::AssignmentInPredicate { span: Some(s), .. } if contains_strict(s)
        )
    });
    if eb026_covers_strict {
        errors.remove(0);
        return;
    }

    let recovered_spans: Vec<Span> = errors
        .iter()
        .skip(1)
        .filter_map(|e| match e {
            ParseError::RecoverableError { span: Some(s), .. } => Some(*s),
            _ => None,
        })
        .collect();

    if recovered_spans.iter().any(&contains_strict) {
        // Strict error sits inside a recovered predicate: keep it, drop the
        // recovery error(s) that merely re-flag that same predicate.
        let mut is_strict = true;
        errors.retain(|e| {
            if is_strict {
                is_strict = false;
                return true; // never drop the strict error itself
            }
            !matches!(
                e,
                ParseError::RecoverableError { span: Some(s), .. } if contains_strict(s)
            )
        });
    } else if recovered_spans.iter().any(|s| s.end <= strict.start) {
        // Strict error fell through past a broken predicate into a later one;
        // recovery's segment error is the accurate report, so drop the strict
        // error that points at the wrong predicate.
        errors.remove(0);
    }
}

/// Byte offset where the event section begins. Prefer the structurally
/// classified machine `EVENTS` clause; the raw event spellings remain a
/// fallback for malformed input that omitted the section keyword entirely.
fn first_event_region_start(text: &RecoveryText, positions: &[RecoveryPosition]) -> usize {
    if let Some((pos, _, _)) = positions
        .iter()
        .copied()
        .find(|&(_, keyword, _)| keyword == KeywordId::Events)
    {
        return pos;
    }
    let protected_names: Vec<Span> = positions
        .iter()
        .filter_map(|&(pos, keyword, len)| {
            required_clause_name_span(text, keyword, pos + len, text.masked.len())
        })
        .collect();
    [KeywordId::Event, KeywordId::Initialisation]
        .iter()
        .flat_map(|&id| crate::keywords::keyword(id).spellings)
        .filter_map(|spelling| find_keyword_word(text, spelling, 0, text.masked.len()))
        .filter(|pos| !protected_names.iter().any(|span| span.start == *pos))
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

/// Whether `pos` starts a non-whitespace token on its line.
fn is_line_anchored(s: &str, pos: usize) -> bool {
    s[line_start(s, pos)..pos].chars().all(char::is_whitespace)
}

fn last_formula_segment_start(text: &RecoveryText, content_start: usize, boundary: usize) -> usize {
    let preceding = text.labels.partition_point(|label| label.start < boundary);
    text.labels[..preceding]
        .last()
        .filter(|label| label.start >= content_start)
        .map_or(content_start, |label| {
            predicate_start_for_label(&text.masked, content_start, label.start)
        })
}

#[derive(Clone, Copy)]
enum RecoveryFormulaKind {
    Component,
    Event,
}

impl RecoveryFormulaKind {
    fn contains(self, keyword: KeywordId) -> bool {
        match self {
            Self::Component => matches!(
                keyword,
                KeywordId::Axioms
                    | KeywordId::Invariants
                    | KeywordId::Theorems
                    | KeywordId::Variant
            ),
            Self::Event => matches!(
                keyword,
                KeywordId::Where | KeywordId::With | KeywordId::Witness | KeywordId::Then
            ),
        }
    }

    fn ends_before(
        self,
        text: &RecoveryText,
        keyword: KeywordId,
        content_start: usize,
        boundary: usize,
    ) -> bool {
        let segment_start = last_formula_segment_start(text, content_start, boundary);
        let content = text.masked[segment_start..boundary].trim();
        if content.is_empty() {
            return false;
        }
        match (self, keyword) {
            (Self::Component, KeywordId::Axioms | KeywordId::Invariants | KeywordId::Theorems)
            | (Self::Event, KeywordId::Where | KeywordId::With | KeywordId::Witness) => {
                try_parse_labeled_predicate_from_text(content).is_ok()
            }
            (Self::Component, KeywordId::Variant) => parse_expression_str(content).is_ok(),
            (Self::Event, KeywordId::Then) => try_parse_labeled_action_from_text(content).is_ok(),
            _ => true,
        }
    }
}

/// Classify sorted structural-keyword candidates while preserving required
/// names and structural words used inside formulas.
fn classify_recovery_positions(
    text: &RecoveryText,
    mut candidates: Vec<RecoveryPosition>,
    to: usize,
    formula_kind: RecoveryFormulaKind,
    mut protected_name: Option<Span>,
) -> Vec<RecoveryPosition> {
    candidates.sort_unstable_by_key(|&(pos, _, _)| pos);
    let mut positions = Vec::new();
    let mut formula_clause = None;
    for (pos, keyword, len) in candidates {
        if protected_name.is_some_and(|span| span.start == pos) {
            protected_name = None;
            continue;
        }
        if let Some((active_keyword, content_start)) = formula_clause
            && !is_line_anchored(&text.masked, pos)
            && !formula_kind.ends_before(text, active_keyword, content_start, pos)
        {
            continue;
        }

        positions.push((pos, keyword, len));
        protected_name = required_clause_name_span(text, keyword, pos + len, to);
        formula_clause = formula_kind
            .contains(keyword)
            .then_some((keyword, pos + len));
    }
    positions
}

/// Structurally classified context/machine clause boundaries in source order.
///
/// The pass is forward and scope-aware: after an actual clause header it marks
/// only that header's required first name as an identifier position, and it
/// parses formula prefixes before accepting inline boundaries. This mirrors the
/// grammar follow-set without repeatedly inferring roles from raw preceding
/// keyword spellings.
fn recovery_clause_positions(
    text: &RecoveryText,
    scope: u8,
    from: usize,
    to: usize,
) -> Vec<(usize, KeywordId, usize)> {
    let component_spelling = if scope == crate::keywords::scope::CONTEXT {
        crate::keywords::spell(KeywordId::Context)
    } else {
        crate::keywords::spell(KeywordId::Machine)
    };
    let header = find_keyword_word(text, component_spelling, from, to);
    let scan_from = header.map_or(from, |pos| pos + component_spelling.len());
    let protected_name = header.and_then(|pos| {
        first_identifier_candidate(
            text.masked[pos + component_spelling.len()..]
                .lines()
                .next()
                .unwrap_or(""),
            pos + component_spelling.len(),
        )
        .filter(|(name, _)| crate::names::is_valid_component_name(name))
        .map(|(_, span)| span)
    });

    let mut candidates = Vec::new();
    for keyword in crate::keywords::iter_completion_scope(scope) {
        for &spelling in keyword.spellings {
            for pos in keyword_positions(text, spelling, scan_from, to) {
                candidates.push((pos, keyword.id, spelling.len()));
            }
        }
    }
    classify_recovery_positions(
        text,
        candidates,
        to,
        RecoveryFormulaKind::Component,
        protected_name,
    )
}

/// Byte offsets of every whole-word occurrence of `needle_upper` (an
/// ASCII-uppercase keyword) in `[from, to)`, in source order.
fn keyword_positions(
    text: &RecoveryText,
    needle_upper: &str,
    from: usize,
    to: usize,
) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut search = from;
    while let Some(pos) = find_keyword_word(text, needle_upper, search, to) {
        positions.push(pos);
        search = pos + needle_upper.len();
    }
    positions
}

struct RecoveredEventRegion {
    header: usize,
    header_name: Option<(String, Span)>,
    is_initialisation: bool,
    header_target: Option<(String, Span, bool)>,
    body_end: usize,
    positions: Vec<RecoveryPosition>,
}

/// Structurally locate and classify each event once, retaining the decoded
/// header and clause positions for AST recovery.
fn recovered_event_regions(
    text: &RecoveryText,
    from: usize,
    to: usize,
) -> Vec<RecoveredEventRegion> {
    let event_kw = crate::keywords::spell(KeywordId::Event);
    let init_kw = crate::keywords::spell(KeywordId::Initialisation);
    let end_len = crate::keywords::spell(KeywordId::End).len();
    let mut regions = Vec::new();
    let mut search = from;
    while let Some(pos) = find_keyword_word(text, event_kw, search, to) {
        let kw_end = pos + event_kw.len();
        let header_line = text.masked[kw_end..].lines().next().unwrap_or("");
        let header_name = first_identifier_candidate(header_line, kw_end)
            .filter(|(name, _)| crate::names::is_valid_component_name(name))
            .map(|(name, span)| (name.to_string(), span));
        let name_end = header_name.as_ref().map_or(kw_end, |(_, span)| span.end);
        let is_initialisation = strip_keyword_prefix(header_line.trim_start(), init_kw).is_some();
        let header_target = (!is_initialisation)
            .then(|| event_header_target(text, name_end, to))
            .flatten();
        let body_start = header_target
            .as_ref()
            .map_or(name_end, |(_, span, _)| span.end);

        // A line-anchored EVENT is the missing-END recovery fallback. Inline
        // headers are found after the preceding END advances `search` here.
        let mut header_search = name_end;
        let next_header = loop {
            let Some(candidate) = find_keyword_word(text, event_kw, header_search, to) else {
                break to;
            };
            if is_line_anchored(&text.masked, candidate) {
                break candidate;
            }
            header_search = candidate + event_kw.len();
        };
        let mut positions =
            event_clause_positions(text, is_initialisation, body_start, next_header);
        let end_index = positions
            .iter()
            .position(|&(_, keyword, _)| keyword == KeywordId::End);
        let body_end = end_index.map_or(next_header, |index| positions[index].0 + end_len);
        if let Some(index) = end_index {
            positions.truncate(index + 1);
        }
        regions.push(RecoveredEventRegion {
            header: pos,
            header_name,
            is_initialisation,
            header_target,
            body_end,
            positions,
        });
        search = body_end;
        if search <= pos {
            search = kw_end;
        }
    }
    regions
}

/// Structurally classified boundaries inside one recovered event.
fn event_clause_positions(
    text: &RecoveryText,
    is_initialisation: bool,
    from: usize,
    to: usize,
) -> Vec<RecoveryPosition> {
    let mut candidates = Vec::new();
    let mut add_keyword = |keyword: KeywordId| {
        for &spelling in crate::keywords::keyword(keyword).spellings {
            for pos in keyword_positions(text, spelling, from, to) {
                candidates.push((pos, keyword, spelling.len()));
            }
        }
    };
    if is_initialisation {
        for keyword in [KeywordId::Then, KeywordId::End] {
            add_keyword(keyword);
        }
    } else {
        for keyword in crate::keywords::iter_completion_scope(crate::keywords::scope::EVENT)
            .filter(|keyword| keyword.id != KeywordId::Extends)
        {
            add_keyword(keyword.id);
        }
    }
    classify_recovery_positions(text, candidates, to, RecoveryFormulaKind::Event, None)
}

/// Derive an event clause's content range from its classified positions.
fn clause_content_range(
    positions: &[RecoveryPosition],
    clause_kw: KeywordId,
    boundary_kws: &[KeywordId],
    to: usize,
) -> Option<(usize, usize)> {
    let (index, &(kw_pos, _, kw_len)) = positions
        .iter()
        .enumerate()
        .find(|&(_, &(_, keyword, _))| keyword == clause_kw)?;

    let content_start = kw_pos + kw_len;
    let content_end = positions[index + 1..]
        .iter()
        .find(|&&(_, keyword, _)| boundary_kws.contains(&keyword))
        .map(|&(pos, _, _)| pos)
        .unwrap_or(to);
    Some((content_start, content_end))
}

/// Recover a named event's WITH, WITNESS, and THEN clause bodies. Called after
/// the event-specific clauses (ANY/WHERE) have already been handled.
/// INITIALISATION has only a THEN clause and is handled separately.
fn recover_common_event_clauses(
    text: &RecoveryText,
    positions: &[RecoveryPosition],
    with: &mut Vec<LabeledPredicate>,
    witnesses: &mut Vec<LabeledPredicate>,
    actions: &mut Vec<LabeledAction>,
    body_end: usize,
    errors: &mut Vec<ParseError>,
) {
    if let Some((content_start, content_end)) = clause_content_range(
        positions,
        KeywordId::With,
        &[KeywordId::Witness, KeywordId::Then, KeywordId::End],
        body_end,
    ) {
        *with =
            recover_predicates_in_range(text, content_start, content_end, "with predicate", errors);
    }
    if let Some((content_start, content_end)) = clause_content_range(
        positions,
        KeywordId::Witness,
        &[KeywordId::Then, KeywordId::End],
        body_end,
    ) {
        *witnesses =
            recover_predicates_in_range(text, content_start, content_end, "witness", errors);
    }
    if let Some((content_start, content_end)) =
        clause_content_range(positions, KeywordId::Then, &[KeywordId::End], body_end)
    {
        *actions = recover_actions_in_range(text, content_start, content_end, errors);
    }
}

/// Optional `REFINES`/`EXTENDS` target immediately after a named event.
fn event_header_target(
    text: &RecoveryText,
    name_end: usize,
    next_header: usize,
) -> Option<(String, Span, bool)> {
    let header_tail = text.masked.get(name_end..next_header)?;
    let (keyword, keyword_span) = first_identifier_candidate(header_tail, name_end)?;
    let keyword = crate::keywords::lookup(keyword)?.id;
    if !matches!(keyword, KeywordId::Extends | KeywordId::Refines) {
        return None;
    }
    first_identifier_candidate(
        &text.masked[keyword_span.end..next_header],
        keyword_span.end,
    )
    .filter(|(name, _)| crate::names::is_valid_component_name(name))
    .map(|(name, span)| (name.to_string(), span, keyword == KeywordId::Extends))
}

/// Recover a machine's events from the event region.
///
/// Each `EVENT name … END` / `EVENT INITIALISATION … END` is located by
/// whole-word keyword scanning, so a label such as `@end` never closes an
/// event. Events do not nest, so each one closes at the first `END` after its
/// header. After locating each event's extent, this function attempts
/// best-effort re-parsing of its clause bodies (WHERE/WITH/WITNESS guards,
/// THEN/BEGIN actions) so that formula-identifier tokens survive when only
/// one predicate in the machine has a syntax error. Parse failures for
/// individual predicates or actions are appended to `errors` and skipped.
fn recover_events(
    text: &RecoveryText,
    events_start: usize,
    errors: &mut Vec<ParseError>,
) -> (Option<InitialisationEvent>, Vec<Event>, Option<usize>) {
    let regions = recovered_event_regions(text, events_start, text.masked.len());

    let mut initialisation = None;
    let mut events = Vec::new();
    // The EVENTS clause runs from its header through the last event's END.
    let mut events_end = None;

    for region in regions {
        let RecoveredEventRegion {
            header,
            header_name,
            is_initialisation,
            header_target,
            body_end,
            positions,
        } = region;
        let span = Span {
            start: header,
            end: body_end,
        };
        events_end = Some(events_end.map_or(body_end, |prev: usize| prev.max(body_end)));

        if is_initialisation {
            let mut init = InitialisationEvent {
                actions: Vec::new(),
                comment: None,
                extended: false,
                with: Vec::new(),
                witnesses: Vec::new(),
                span: Some(span),
                name_span: header_name.map(|(_, span)| span),
            };
            // INITIALISATION has only a THEN/BEGIN action clause; the grammar
            // forbids WITH/WITNESS here and the strict parser always leaves
            // those empty, so recover just the actions rather than synthesizing
            // clauses a valid parse could never produce.
            if let Some((content_start, content_end)) =
                clause_content_range(&positions, KeywordId::Then, &[KeywordId::End], body_end)
            {
                init.actions = recover_actions_in_range(text, content_start, content_end, errors);
            }
            initialisation = Some(init);
        } else {
            let any_range = clause_content_range(
                &positions,
                KeywordId::Any,
                &[
                    KeywordId::Where,
                    KeywordId::With,
                    KeywordId::Witness,
                    KeywordId::Then,
                    KeywordId::End,
                ],
                body_end,
            );
            let (name, name_span) =
                header_name.map_or_else(|| (String::from("unknown"), None), |(n, s)| (n, Some(s)));
            let mut event = Event::new(name);
            event.span = Some(span);
            event.name_span = name_span;
            if let Some((target, target_span, extended)) = header_target {
                event.refines = Some(target);
                event.refines_span = Some(target_span);
                event.extended = extended;
            }

            // Recover ANY-clause parameters so goto-definition and semantic
            // tokens keep working for event parameters even when a guard or
            // action failed to parse.
            if let Some((content_start, content_end)) = any_range {
                event.parameters =
                    extract_identifiers(&text.masked[content_start..content_end], content_start)
                        .into_iter()
                        .filter(|(name, _)| accepts_declared_name(name))
                        .map(|(name, span)| NamedElement::with_span(name, span))
                        .collect();
            }

            // Recover WHERE/WHEN clause guards.
            if let Some((content_start, content_end)) = clause_content_range(
                &positions,
                KeywordId::Where,
                &[
                    KeywordId::With,
                    KeywordId::Witness,
                    KeywordId::Then,
                    KeywordId::End,
                ],
                body_end,
            ) {
                event.guards =
                    recover_predicates_in_range(text, content_start, content_end, "guard", errors);
            }

            recover_common_event_clauses(
                text,
                &positions,
                &mut event.with,
                &mut event.witnesses,
                &mut event.actions,
                body_end,
                errors,
            );

            events.push(event);
        }
    }

    (initialisation, events, events_end)
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
fn component_name_after(
    text: &RecoveryText,
    keyword_pos: usize,
    keyword: KeywordId,
) -> Option<(String, Span)> {
    let content_start = keyword_pos + crate::keywords::spell(keyword).len();
    let rest = text.masked[content_start..].lines().next()?;
    first_identifier_candidate(rest, content_start)
        .filter(|(name, _)| crate::names::is_valid_component_name(name))
        .map(|(name, span)| (name.to_string(), span))
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
fn extract_clause_content(
    clause_keyword: KeywordId,
    bound: usize,
    positions: &[RecoveryPosition],
) -> Option<Span> {
    let (index, &(start, _, _)) = positions
        .iter()
        .enumerate()
        .find(|&(_, &(start, keyword, _))| keyword == clause_keyword && start < bound)?;
    let end = positions
        .get(index + 1)
        .map(|&(pos, _, _)| pos.min(bound))
        .unwrap_or(bound);

    Some(Span { start, end })
}

/// Split a clause line into declared identifiers, each paired with its [`Span`].
///
/// Items are whitespace-separated, matching the structural grammar; a stray
/// comma is tolerated as a separator too, so recovery still salvages names from
/// a line a user wrote comma-separated even though the strict parser rejects it.
///
/// `base` is the byte offset of `s` within the text whose coordinates the
/// returned spans should use. Each identifier is a subslice of `s`, so its
/// offset within `s` (recovered by pointer arithmetic, always on a char
/// boundary) added to `base` gives a byte-exact span over the name.
fn extract_identifiers(s: &str, base: usize) -> Vec<(String, Span)> {
    s.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .map(|id| {
            let start = base + subslice_offset(s, id);
            let span = Span {
                start,
                end: start + id.len(),
            };
            (id.to_string(), span)
        })
        .collect()
}

/// Byte offset of `sub` within `parent`. `sub` MUST be a subslice of `parent`
/// (i.e. produced by slicing/`split`/`trim` of `parent`, sharing its
/// allocation) — recovery maps a scanned fragment back to an absolute offset in
/// the byte-layout-preserving masked text this way. The result is always on a
/// char boundary because `sub` is a real subslice.
fn subslice_offset(parent: &str, sub: &str) -> usize {
    sub.as_ptr() as usize - parent.as_ptr() as usize
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

/// Parse a labeled action from a string segment: `@label lhs := rhs` or
/// bare `lhs := rhs`. Used by [`recover_actions_in_range`].
///
/// Returns the parsed action together with the byte offset, within (trimmed)
/// `text`, at which the action body begins. The `@label` prefix is stripped
/// before parsing, so the action's spans are relative to that body offset, not
/// to `text`; the caller adds the offset when shifting spans into absolute
/// document coordinates.
fn try_parse_labeled_action_from_text(text: &str) -> Result<(LabeledAction, usize), ParseError> {
    let text = text.trim();

    // Extract an optional `@label` prefix, mirroring how the strict grammar
    // parses `label? ~ action`. The label ends at the first whitespace. A bare
    // `@` with nothing following (label_name is empty) is NOT treated as a
    // label-with-no-name: the whole text is passed through so parse_action_str
    // rejects it and the caller emits a diagnostic.
    let (label, action_text) = text
        .strip_prefix('@')
        .and_then(|rest| {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let name = rest[..end].trim_end_matches(':');
            (!name.is_empty()).then(|| (Some(name.to_string()), rest[end..].trim()))
        })
        .unwrap_or((None, text));

    // `action_text` is a subslice of `text`; this is the offset of the action
    // body past any `@label ` prefix that was stripped above.
    let body_offset = subslice_offset(text, action_text);
    let action = parse_action_str(action_text).map_err(|error| {
        // Keep recovery sources in the segment coordinate space documented by
        // `RecoverableError::source`. Arity diagnostics need their operator
        // span shifted past the optional label so consumers can combine it
        // with the outer recovery span; recompute line/column from that span
        // instead of applying a byte-only shift to every nested error.
        if let ParseError::AssignmentArityMismatch {
            targets,
            expressions,
            span: Some(span),
            ..
        } = error
        {
            let span = Span {
                start: body_offset + span.start,
                end: body_offset + span.end,
            };
            let (line, column) = offset_to_line_col(text, span.start);
            ParseError::AssignmentArityMismatch {
                targets,
                expressions,
                line,
                column,
                span: Some(span),
            }
        } else {
            error
        }
    })?;
    Ok((
        LabeledAction {
            label,
            action,
            span: None,
            comment: None,
        },
        body_offset,
    ))
}

/// Recover labeled actions from an explicit `[from, to)` byte range.
///
/// Mirrors [`recover_predicates_in_range`] but for event THEN/BEGIN clauses.
/// Broken segments are logged to `errors` and skipped; the caller receives
/// only actions whose bodies parsed successfully, so `LabeledAction.action`
/// is always populated.
fn recover_actions_in_range(
    text: &RecoveryText,
    from: usize,
    to: usize,
    errors: &mut Vec<ParseError>,
) -> Vec<LabeledAction> {
    recover_clause_in_range(text, from, to, "action", errors, |content, abs_start| {
        let (mut la, body_offset) = try_parse_labeled_action_from_text(content)?;
        // The label-inclusive span anchors the action in the outline; its body
        // spans are relative to the body, past the stripped label.
        la.span = Some(segment_span(abs_start, content));
        SpanShifter(abs_start + body_offset).visit_action(&mut la.action);
        Ok(la)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// EB026: a predicate string that is really an assignment is reported as
    /// [`ParseError::AssignmentInPredicate`], carrying the offending operator,
    /// not a generic pest error.
    #[test]
    fn parse_predicate_str_flags_misplaced_assignments() {
        for (src, op) in [
            ("x := 5", ":="),
            ("x ≔ 5", "≔"),
            ("x :∈ ℕ", ":∈"),
            ("x :| x' > 0", ":|"),
        ] {
            match parse_predicate_str(src) {
                Err(ParseError::AssignmentInPredicate { operator, .. }) => {
                    assert_eq!(operator, op, "wrong operator reported for {src:?}");
                }
                other => panic!("expected AssignmentInPredicate for {src:?}, got {other:?}"),
            }
        }
    }

    /// A valid predicate parses; a formula that is neither a valid predicate nor
    /// a valid assignment keeps its generic error; `skip` (an action with no
    /// becomes operator) is not misreported as EB026.
    #[test]
    fn parse_predicate_str_does_not_overreach() {
        assert!(parse_predicate_str("x = 5").is_ok());
        assert!(!matches!(
            parse_predicate_str("x ==== y"),
            Err(ParseError::AssignmentInPredicate { .. })
        ));
        assert!(!matches!(
            parse_predicate_str("skip"),
            Err(ParseError::AssignmentInPredicate { .. })
        ));
    }

    #[test]
    fn find_becomes_operator_locates_first_operator() {
        assert_eq!(find_becomes_operator("x := 5"), Some((":=", 2)));
        assert_eq!(find_becomes_operator("x = 5"), None);
    }

    /// EB026 flows through recovery too: an invariant clause whose predicate is a
    /// misplaced assignment yields a single top-level `AssignmentInPredicate`
    /// (with an absolute operator span), not a wrapped `RecoverableError` — and
    /// the misleading strict error (which trips at `=`, reading `:` as ASCII
    /// membership) is deduped away.
    #[test]
    fn recovery_reports_assignment_in_invariant() {
        let src = "machine M\ninvariants\n  @inv1 x := 5\nend\n";
        let result = parse_components_with_recovery(src);
        assert_eq!(
            result.errors.len(),
            1,
            "exactly one error, the EB026: {:?}",
            result.errors
        );
        let eb026 = &result.errors[0];
        assert!(matches!(eb026, ParseError::AssignmentInPredicate { .. }));
        let span = eb026.span().expect("EB026 carries an operator span");
        assert_eq!(&src[span.start..span.end], ":=");
    }

    /// EB026 in a component *after the first* in a multi-component file must be
    /// line-shifted to absolute coordinates (the multi-component recovery path
    /// parses each component in its own slice, then offsets the errors).
    #[test]
    fn recovery_shifts_assignment_in_later_component() {
        let src = concat!(
            "CONTEXT C\n",        // 1
            "CONSTANTS\n",        // 2
            "    k\n",            // 3
            "AXIOMS\n",           // 4
            "    @axm1 k = 5\n",  // 5
            "END\n",              // 6
            "MACHINE M\n",        // 7
            "VARIABLES\n",        // 8
            "    x\n",            // 9
            "INVARIANTS\n",       // 10
            "    @inv1 x := 5\n", // 11 — the `:=` is here
            "END\n",              // 12
        );
        let result = parse_components_with_recovery(src);
        let eb026 = result
            .errors
            .iter()
            .find(|e| matches!(e, ParseError::AssignmentInPredicate { .. }))
            .expect("EB026 is reported for the machine's invariant");
        // Absolute position: line 11, column 13 (the `:=`), not slice-relative.
        assert_eq!(eb026.position(), Some((11, 13)));
    }

    /// An action clause (THEN) legitimately uses becomes operators — recovery of
    /// a broken action must never be reclassified as EB026.
    #[test]
    fn recovery_does_not_flag_actions_as_eb026() {
        // A genuinely broken action (dangling `+`) recovers as a plain error.
        let src = "machine M\nevents\n  event evt\n  then\n    @act1 x := y +\n  end\nend\n";
        let result = parse_components_with_recovery(src);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, ParseError::AssignmentInPredicate { .. })),
            "an action must never be reported as EB026"
        );
    }

    /// The displayable glyph [`friendly_rule_name`] must return for every rule
    /// it handles. This pins the observable behaviour and is the authoritative
    /// enumeration the drift checks range over (pest's generated `Rule` has no
    /// variant iterator, and the crate does not depend on `strum`).
    #[rustfmt::skip]
    const GOLDEN: &[(Rule, &str)] = &[
        (Rule::op_and, "∧"),
        (Rule::op_or, "∨"),
        (Rule::op_implies, "⇒"),
        (Rule::op_equivalent, "⇔"),
        (Rule::op_not, "¬"),
        (Rule::op_forall, "∀"),
        (Rule::op_exists, "∃"),
        (Rule::op_in, "∈"),
        (Rule::op_notin, "∉"),
        (Rule::op_subset, "⊆"),
        (Rule::op_subset_strict, "⊂"),
        (Rule::op_not_subset, "⊈"),
        (Rule::op_not_subset_strict, "⊄"),
        (Rule::op_union, "∪"),
        (Rule::op_intersection, "∩"),
        (Rule::op_difference, "∖"),
        (Rule::op_cartesian, "×"),
        (Rule::op_powerset, "ℙ"),
        (Rule::op_powerset1, "ℙ1"),
        (Rule::op_emptyset, "∅"),
        (Rule::op_oftype, "⦂"),
        (Rule::op_maplet, "↦"),
        (Rule::op_relation, "↔"),
        (Rule::op_partial_fn, "⇸"),
        (Rule::op_total_fn, "→"),
        (Rule::op_partial_inj, "⤔"),
        (Rule::op_total_inj, "↣"),
        (Rule::op_partial_surj, "⤀"),
        (Rule::op_total_surj, "↠"),
        (Rule::op_bijection, "⤖"),
        (Rule::op_domain, "dom"),
        (Rule::op_range, "ran"),
        (Rule::op_inverse, "∼"),
        (Rule::op_semicolon, ";"),
        (Rule::op_composition, "∘"),
        (Rule::op_domain_restrict, "◁"),
        (Rule::op_domain_subtract, "⩤"),
        (Rule::op_range_restrict, "▷"),
        (Rule::op_range_subtract, "⩥"),
        (Rule::op_overwrite, "<+"),
        (Rule::op_direct_product, "⊗"),
        (Rule::op_parallel_product, "∥"),
        (Rule::op_total_surjective_relation, "<<->>"),
        (Rule::op_surjective_relation, "<->>"),
        (Rule::op_total_relation, "<<->"),
        (Rule::op_plus, "+"),
        (Rule::op_minus, "−"),
        (Rule::op_multiply, "∗"),
        (Rule::op_divide, "÷"),
        (Rule::op_modulo, "mod"),
        (Rule::op_exponent, "^"),
        (Rule::op_range_op, ".."),
        (Rule::op_eq, "="),
        (Rule::op_neq, "≠"),
        (Rule::op_lt, "<"),
        (Rule::op_le, "≤"),
        (Rule::op_gt, ">"),
        (Rule::op_ge, "≥"),
        (Rule::op_becomes_equal, "≔"),
        (Rule::op_becomes_in, ":∈"),
        (Rule::op_becomes_such, ":∣"),
        (Rule::dot, "·"),
        (Rule::comma, ","),
        (Rule::colon, ":"),
        (Rule::lparen, "("),
        (Rule::rparen, ")"),
        (Rule::lbrace, "{"),
        (Rule::rbrace, "}"),
        (Rule::lbracket, "["),
        (Rule::rbracket, "]"),
        (Rule::pipe, "|"),
    ];

    /// Operators whose displayable glyph deliberately differs from the
    /// canonical `OPERATOR_SPELLINGS` unicode (see [`friendly_rule_name`]).
    const DIVERGENCES: &[Rule] = &[
        Rule::op_overwrite,
        Rule::op_total_relation,
        Rule::op_surjective_relation,
        Rule::op_total_surjective_relation,
        Rule::op_range_op,
    ];

    #[test]
    fn friendly_rule_name_matches_golden() {
        for &(rule, expected) in GOLDEN {
            assert_eq!(
                friendly_rule_name(rule),
                Some(expected),
                "friendly_rule_name({rule:?})"
            );
        }
    }

    /// Single-source-of-truth guard: every operator rule listed in `GOLDEN`
    /// carries the glyph from `OPERATOR_SPELLINGS`, except the documented
    /// divergences — and each of those is still a genuine divergence, so the
    /// override stays justified.
    #[test]
    fn friendly_derives_from_spellings() {
        // Each divergence must be pinned in `GOLDEN`, otherwise the loop below
        // would silently skip its override-still-justified check.
        for &rule in DIVERGENCES {
            assert!(
                GOLDEN.iter().any(|&(r, _)| r == rule),
                "divergence {rule:?} is missing from GOLDEN"
            );
        }
        for &(rule, expected) in GOLDEN {
            let Some(id) = rule_to_operator_id(rule) else {
                continue; // syntax tokens have no OperatorId to derive from
            };
            let canonical = crate::operators::spell(id, true);
            if DIVERGENCES.contains(&rule) {
                assert_ne!(
                    expected, canonical,
                    "{rule:?} is listed as a divergence but now matches its \
                     canonical spelling; drop the override"
                );
            } else {
                assert_eq!(
                    expected, canonical,
                    "{rule:?} glyph drifted from OPERATOR_SPELLINGS"
                );
            }
        }
    }

    /// Drift guard for [`rule_to_keyword`] (the diagnostic friendly-naming of
    /// keyword rules): a keyword token, a section/event clause rule, the `event`
    /// rule that must collapse onto the `EVENT` keyword, and the aliases
    /// (`kw_when`→`Where`, `kw_begin`→`Then`) each resolve to the listed
    /// [`KeywordId`], rendered through [`crate::keywords::spell`] — so the surface
    /// spelling lives only in the keyword table. If a future keyword's rule stops
    /// resolving here, this fails. (Keywords have no glyph divergence from `spell`,
    /// unlike the operator `GOLDEN` table, so they delegate rather than pin a
    /// literal.)
    #[test]
    fn friendly_rule_name_spells_keyword_rules() {
        use crate::keywords::{KeywordId, spell};
        for (rule, id) in [
            (Rule::kw_variables, KeywordId::Variables),
            (Rule::kw_end, KeywordId::End),
            (Rule::kw_event, KeywordId::Event),
            (Rule::event, KeywordId::Event),
            (Rule::event_where, KeywordId::Where),
            (Rule::event_then, KeywordId::Then),
            (Rule::kw_when, KeywordId::Where),
            (Rule::kw_begin, KeywordId::Then),
            (Rule::machine_clause_variables, KeywordId::Variables),
            (Rule::context_clause_axioms, KeywordId::Axioms),
        ] {
            assert_eq!(friendly_rule_name(rule), Some(spell(id)), "{rule:?}");
        }
    }

    /// The INITIALISATION event has no identifier of its own — its name is the
    /// keyword — so its name span is the `INITIALISATION` token. LSP navigation
    /// (go-to-definition, the document outline) reads it, so pin that it is
    /// captured and covers exactly that keyword, in either casing.
    #[test]
    fn initialisation_name_span_covers_the_keyword() {
        for source in [
            "MACHINE m\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        x := 0\n    END\nEND",
            "machine m\nevents\n    event initialisation\n    then\n        x := 0\n    end\nend",
            // An extended init carries two INITIALISATION tokens; the name span
            // must be the event's own header keyword, not the abstract event
            // named after `extends`.
            "MACHINE m\nREFINES n\nEVENTS\n    EVENT INITIALISATION extends INITIALISATION\n    THEN\n        x := 0\n    END\nEND",
        ] {
            let Component::Machine(machine) = parse(source).expect("parses") else {
                panic!("expected a machine");
            };
            let init = machine.initialisation.expect("initialisation present");
            let span = init.name_span.expect("initialisation name span captured");
            assert!(
                source[span.start..span.end].eq_ignore_ascii_case("INITIALISATION"),
                "name span should cover the INITIALISATION keyword, got {:?}",
                &source[span.start..span.end]
            );
            // The event's own name is the first INITIALISATION token, not a
            // later one (e.g. the `extends` target).
            let first = source
                .to_ascii_uppercase()
                .find("INITIALISATION")
                .expect("source mentions INITIALISATION");
            assert_eq!(
                span.start, first,
                "name span should be the event's own INITIALISATION token"
            );
        }
    }

    #[test]
    fn component_name_occurrences_cover_exact_structural_targets() {
        let source = "CONTEXT Derived\nEXTENDS Base ENV-C\nEND\n\nMACHINE Concrete\nREFINES Abstract-M\nSEES Base ENV-C\nEND";
        let occurrences = component_name_occurrences_with_sites(source);
        let names: Vec<_> = occurrences
            .iter()
            .map(|occurrence| occurrence.name.as_str())
            .collect();

        assert_eq!(
            names,
            [
                "Derived",
                "Base",
                "ENV-C",
                "Concrete",
                "Abstract-M",
                "Base",
                "ENV-C"
            ]
        );
        assert_eq!(
            occurrences
                .iter()
                .map(|occurrence| occurrence.site)
                .collect::<Vec<_>>(),
            [
                ComponentNameSite::Declaration(ComponentKind::Context),
                ComponentNameSite::Dependency(EdgeKind::Extends),
                ComponentNameSite::Dependency(EdgeKind::Extends),
                ComponentNameSite::Declaration(ComponentKind::Machine),
                ComponentNameSite::Dependency(EdgeKind::Refines),
                ComponentNameSite::Dependency(EdgeKind::Sees),
                ComponentNameSite::Dependency(EdgeKind::Sees),
            ]
        );
        for occurrence in occurrences {
            let span = occurrence.span.expect("text occurrence has a span");
            assert_eq!(&source[span.start..span.end], occurrence.name);
        }
    }

    #[test]
    fn component_name_occurrences_survive_errors_and_repeated_clauses() {
        let source = "CONTEXT D\nAXIOMS\n    @a x ∈\nEND\n\nMACHINE M\nSEES C\nSEES D\nINVARIANTS\n    @i y ∈\nEND";
        let occurrences = component_name_occurrences(source);
        let sites: Vec<_> = occurrences
            .iter()
            .map(|occurrence| {
                let span = occurrence.span.expect("text occurrence has a span");
                (occurrence.name.as_str(), span.start)
            })
            .collect();

        assert_eq!(
            sites,
            [
                ("D", source.find("D").unwrap()),
                ("M", source.find("MACHINE M").unwrap() + "MACHINE ".len()),
                ("C", source.find("SEES C").unwrap() + "SEES ".len()),
                ("D", source.find("SEES D").unwrap() + "SEES ".len()),
            ]
        );
    }

    /// The `refines`/`extends` target's span is captured (inline and body-level),
    /// so a cursor on the target can be told apart from the event's own name —
    /// even when the two are identical (the issue #84 case).
    #[test]
    fn event_refines_target_span_covers_the_target_name() {
        // (source, expects_extended) — inline refines, inline extends, body REFINES.
        let cases = [
            (
                "MACHINE m\nREFINES n\nEVENTS\n    EVENT e refines f\n    THEN\n        x := 0\n    END\nEND",
                false,
            ),
            (
                "MACHINE m\nREFINES n\nEVENTS\n    EVENT e extends f\n    THEN\n        x := 0\n    END\nEND",
                true,
            ),
            (
                "MACHINE m\nREFINES n\nEVENTS\n    EVENT e\n    REFINES f\n    THEN\n        x := 0\n    END\nEND",
                false,
            ),
        ];
        for (source, extended) in cases {
            let Component::Machine(machine) = parse(source).expect("parses") else {
                panic!("expected a machine");
            };
            let event = machine.events.first().expect("one event");
            assert_eq!(event.refines.as_deref(), Some("f"));
            assert_eq!(event.extended, extended);
            let span = event.refines_span.expect("refines target span captured");
            assert_eq!(&source[span.start..span.end], "f");
            // The target span is the name after the keyword, never the event name.
            assert!(span.start > event.name_span.expect("name span").end);
        }

        // Same name in source: the target span is the second occurrence, distinct
        // from the event's own name span — the discriminator issue #84 needs.
        let source = "MACHINE m\nREFINES n\nEVENTS\n    EVENT ML_in extends ML_in\n    THEN\n        x := 0\n    END\nEND";
        let Component::Machine(machine) = parse(source).expect("parses") else {
            panic!("expected a machine");
        };
        let event = machine.events.first().expect("one event");
        let name = event.name_span.expect("name span");
        let target = event.refines_span.expect("refines target span");
        assert_eq!(&source[name.start..name.end], "ML_in");
        assert_eq!(&source[target.start..target.end], "ML_in");
        assert!(
            target.start > name.end,
            "the target span must follow the event's own name span"
        );
    }

    /// In a multi-component document the recovery path parses each component from
    /// its slice and lifts the slice-relative spans to absolute document offsets.
    /// The refines target span must be lifted with the rest, or a cursor check
    /// against it would land in the wrong component.
    #[test]
    fn refines_target_span_is_absolute_in_a_multi_component_document() {
        // The broken first component forces the whole-file parse to fail, so the
        // region-splitting recovery path runs; the machine region then starts
        // past offset 0 and its spans are shifted.
        let source = "CONTEXT c\nAXIOMS\n    @a x ∈\nEND\n\nMACHINE m\nREFINES n\nEVENTS\n    EVENT e extends e\n    THEN\n        skip\n    END\nEND";
        let components = parse_components_with_recovery(source)
            .component
            .unwrap_or_default();
        let machine = components
            .iter()
            .find_map(|c| match c {
                Component::Machine(m) => Some(m),
                Component::Context(_) => None,
            })
            .expect("machine recovered");
        let event = machine.events.first().expect("one event");
        let span = event.refines_span.expect("refines target span");
        // Slices correctly only when shifted to absolute coordinates.
        assert_eq!(&source[span.start..span.end], "e");
        assert!(
            span.start > event.name_span.expect("name span").end,
            "the shifted target span follows the event's own name span"
        );
    }

    /// Clause regions are recorded for every clause in source order, each
    /// spanning its header keyword through the clause's last member. Structural
    /// LSP features (folding, outline) read them, so pin the kinds and the two
    /// span boundaries that matter.
    #[test]
    fn clause_regions_are_recorded() {
        let ctx_src = "\
context C
extends Base
sets
    S
constants
    c
axioms
    @a1 c ∈ S
theorems
    @t1 c = c
end";
        let Component::Context(ctx) = parse(ctx_src).expect("context parses") else {
            panic!("expected a context");
        };
        let keywords: Vec<KeywordId> = ctx.clauses.iter().map(|c| c.keyword).collect();
        assert_eq!(
            keywords,
            vec![
                KeywordId::Extends,
                KeywordId::Sets,
                KeywordId::Constants,
                KeywordId::Axioms,
                KeywordId::Theorems,
            ]
        );

        let mch_src = "\
machine M
refines N
sees C
variables
    x
invariants
    @i1 x ∈ ℕ
theorems
    @t1 x = x
variant
    x
events
    event INITIALISATION
    then
        @act1 x ≔ 0
    end
end";
        let Component::Machine(m) = parse(mch_src).expect("machine parses") else {
            panic!("expected a machine");
        };
        let keywords: Vec<KeywordId> = m.clauses.iter().map(|c| c.keyword).collect();
        assert_eq!(
            keywords,
            vec![
                KeywordId::Refines,
                KeywordId::Sees,
                KeywordId::Variables,
                KeywordId::Invariants,
                KeywordId::Theorems,
                KeywordId::Variant,
                KeywordId::Events,
            ]
        );

        // The VARIABLES region spans its header through the last variable.
        let vars = m
            .clauses
            .iter()
            .find(|c| c.keyword == KeywordId::Variables)
            .expect("variables region");
        let vars_text = &mch_src[vars.span.start..vars.span.end];
        assert!(vars_text.starts_with("variables"));
        assert!(vars_text.trim_end().ends_with('x'));

        // The EVENTS region ends at the last event's END, not the machine END,
        // so it is strictly shorter than the whole machine span.
        let events = m
            .clauses
            .iter()
            .find(|c| c.keyword == KeywordId::Events)
            .expect("events region");
        let machine_span = m.span.expect("machine span");
        assert!(mch_src[events.span.start..events.span.end].starts_with("events"));
        assert!(events.span.end < machine_span.end);
    }
}
