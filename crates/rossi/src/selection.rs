//! Owned syntax queries for editor features.
//!
//! A parsed document retains an owned, span-only copy of the Pest hierarchy.
//! Signature help and smart selection query that shared hierarchy by byte
//! offset without exposing Pest outside this crate.

use crate::ast::{
    ActionKind, Component, Expression, ExpressionKind, IdentPattern, Predicate, PredicateKind, Span,
};
use crate::comments::{self, LexicalSpans};
use crate::error::ParseError;
use crate::names::is_valid_math_identifier;
use crate::operators::{self, OperatorId};
use crate::parser::{Rule, line_start, parse_components_guarded};

/// A signature-help construct enclosing a source offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxConstruct {
    UniversalQuantifier,
    ExistentialQuantifier,
    Lambda,
    BasicSetComprehension,
    ExtendedSetComprehension,
    SetBuilder,
}

/// The syntactic part of a construct containing the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxParameter {
    Identifiers,
    Pattern,
    Predicate,
    Expression,
}

/// The innermost supported construct and parameter at a source offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntaxAtOffset {
    pub construct: SyntaxConstruct,
    pub parameter: SyntaxParameter,
}

fn parameter_at(construct: SyntaxConstruct, after_dot: bool, after_pipe: bool) -> SyntaxParameter {
    match construct {
        SyntaxConstruct::UniversalQuantifier | SyntaxConstruct::ExistentialQuantifier => {
            if after_dot {
                SyntaxParameter::Predicate
            } else {
                SyntaxParameter::Identifiers
            }
        }
        SyntaxConstruct::Lambda => {
            if after_pipe {
                SyntaxParameter::Expression
            } else if after_dot {
                SyntaxParameter::Predicate
            } else {
                SyntaxParameter::Pattern
            }
        }
        SyntaxConstruct::ExtendedSetComprehension => {
            if after_pipe {
                SyntaxParameter::Expression
            } else if after_dot {
                SyntaxParameter::Predicate
            } else {
                SyntaxParameter::Identifiers
            }
        }
        SyntaxConstruct::BasicSetComprehension => {
            if after_pipe {
                SyntaxParameter::Predicate
            } else {
                SyntaxParameter::Identifiers
            }
        }
        SyntaxConstruct::SetBuilder => {
            if after_pipe {
                SyntaxParameter::Predicate
            } else {
                SyntaxParameter::Expression
            }
        }
    }
}

#[derive(Debug)]
struct SyntaxNode {
    rule: Rule,
    raw: Span,
    children: Vec<SyntaxNode>,
}

/// Owned syntax data for one exact source snapshot.
///
/// `root` is present when the whole document passed the Pest grammar. Lexical
/// spans are retained even for invalid input so comment-aware, bounded
/// signature fallback remains available during editing.
#[derive(Debug)]
pub(crate) struct SyntaxSnapshot {
    root: Option<SyntaxNode>,
    comments: Vec<Span>,
    labels: Vec<Span>,
}

impl SyntaxSnapshot {
    pub(crate) fn empty(text: &str) -> Self {
        let LexicalSpans { comments, labels } = comments::lexical_spans(text);
        Self {
            root: None,
            comments,
            labels,
        }
    }

    pub(crate) fn from_pair(text: &str, pair: pest::iterators::Pair<Rule>) -> Self {
        Self::from_pair_with_lexical(pair, comments::lexical_spans(text))
    }

    pub(crate) fn from_pair_with_lexical(
        pair: pest::iterators::Pair<Rule>,
        lexical: LexicalSpans,
    ) -> Self {
        Self {
            root: Some(build_node(pair)),
            comments: lexical.comments,
            labels: lexical.labels,
        }
    }

    /// Return one enclosing-span chain per byte offset, in input order.
    ///
    /// Chains are ordered outermost to innermost. Querying never parses; an
    /// invalid offset or a snapshot without a whole-document syntax tree yields
    /// an empty chain for that offset.
    pub(crate) fn enclosing_spans(&self, source: &str, offsets: &[usize]) -> Vec<Vec<Span>> {
        offsets
            .iter()
            .map(|&offset| {
                let mut spans = Vec::new();
                if let Some(root) = &self.root
                    && encloses(root.raw, offset)
                {
                    collect_owned_path(root, source, offset, &mut spans);
                    spans.dedup();
                }
                spans
            })
            .collect()
    }

    /// Find the innermost supported signature construct at `offset`.
    ///
    /// Complete constructs come from the stored Pest hierarchy. If a construct is
    /// incomplete and therefore absent from that hierarchy, a small delimiter-aware
    /// fallback is allowed only inside a recovery-error span (or on the exact
    /// line of a point error). Comments and labels are always opaque.
    pub(crate) fn syntax_at_offset(
        &self,
        source: &str,
        components: &[Component],
        errors: &[ParseError],
        offset: usize,
    ) -> Option<SyntaxAtOffset> {
        if offset > source.len() || !source.is_char_boundary(offset) || self.is_opaque(offset) {
            return None;
        }

        if let Some(root) = &self.root
            && let Some(found) = syntax_in_tree(root, offset)
        {
            return Some(found);
        }
        if self.root.is_none()
            && let Some(found) = self.syntax_in_recovered_components(source, components, offset)
        {
            return Some(found);
        }

        let region = fallback_region(source, errors, offset)?;
        self.scan_incomplete(source, region, offset)
    }

    fn syntax_in_recovered_components(
        &self,
        source: &str,
        components: &[Component],
        offset: usize,
    ) -> Option<SyntaxAtOffset> {
        let mut stack = Vec::new();
        for component in components {
            push_component_formulas(component, &mut stack);
        }

        let mut found = None;
        while let Some(formula) = stack.pop() {
            match formula {
                Formula::Predicate(predicate) => {
                    if predicate.span.is_some_and(|span| !span.contains(offset)) {
                        continue;
                    }
                    found = self
                        .syntax_for_recovered_predicate(source, predicate, offset)
                        .or(found);
                    push_predicate_children(predicate, &mut stack);
                }
                Formula::Expression(expression) => {
                    if expression.span.is_some_and(|span| !span.contains(offset)) {
                        continue;
                    }
                    found = self
                        .syntax_for_recovered_expression(source, expression, offset)
                        .or(found);
                    push_expression_children(expression, &mut stack);
                }
                Formula::Pattern(pattern) => push_pattern_children(pattern, &mut stack),
            }
        }
        found
    }

    fn syntax_for_recovered_predicate(
        &self,
        source: &str,
        predicate: &Predicate,
        offset: usize,
    ) -> Option<SyntaxAtOffset> {
        let PredicateKind::Quantified {
            quantifier,
            predicate: body,
            ..
        } = &predicate.kind
        else {
            return None;
        };
        let span = predicate.span?;
        let body_span = body.span?;
        if offset >= body_span.end {
            return None;
        }
        let dot_end =
            self.last_operator_end(source, span.start, body_span.start, OperatorId::Dot)?;
        let construct = match quantifier {
            crate::ast::predicate::Quantifier::ForAll => SyntaxConstruct::UniversalQuantifier,
            crate::ast::predicate::Quantifier::Exists => SyntaxConstruct::ExistentialQuantifier,
        };
        Some(SyntaxAtOffset {
            construct,
            parameter: parameter_at(construct, offset >= dot_end, false),
        })
    }

    fn syntax_for_recovered_expression(
        &self,
        source: &str,
        expression: &Expression,
        offset: usize,
    ) -> Option<SyntaxAtOffset> {
        let span = expression.span?;
        match &expression.kind {
            ExpressionKind::SetComprehension {
                predicate,
                expression,
                ..
            } => {
                let predicate_span = predicate.span?;
                let final_end = expression
                    .as_deref()
                    .and_then(|expression| expression.span)
                    .map_or(predicate_span.end, |expression| expression.end);
                if offset >= final_end {
                    return None;
                }
                let pipe_start = if expression.is_some() {
                    predicate_span.end
                } else {
                    span.start
                };
                let pipe_limit = expression
                    .as_deref()
                    .and_then(|expression| expression.span)
                    .map_or(predicate_span.start, |expression| expression.start);
                let pipe_end =
                    self.last_operator_end(source, pipe_start, pipe_limit, OperatorId::Bar)?;
                let (construct, dot_end) = if expression.is_some() {
                    let dot_end = self.last_operator_end(
                        source,
                        span.start,
                        predicate_span.start,
                        OperatorId::Dot,
                    )?;
                    (SyntaxConstruct::ExtendedSetComprehension, Some(dot_end))
                } else {
                    (SyntaxConstruct::BasicSetComprehension, None)
                };
                Some(SyntaxAtOffset {
                    construct,
                    parameter: parameter_at(
                        construct,
                        dot_end.is_some_and(|dot| offset >= dot),
                        offset >= pipe_end,
                    ),
                })
            }
            ExpressionKind::SetBuilder {
                member_expression,
                predicate,
            } => {
                let member_span = member_expression.span?;
                let predicate_span = predicate.span?;
                if offset >= predicate_span.end {
                    return None;
                }
                let pipe_end = self.last_operator_end(
                    source,
                    member_span.end,
                    predicate_span.start,
                    OperatorId::Bar,
                )?;
                Some(SyntaxAtOffset {
                    construct: SyntaxConstruct::SetBuilder,
                    parameter: parameter_at(SyntaxConstruct::SetBuilder, false, offset >= pipe_end),
                })
            }
            ExpressionKind::Lambda {
                predicate,
                expression,
                ..
            } => {
                let predicate_span = predicate.span?;
                let expression_span = expression.span?;
                if offset >= expression_span.end {
                    return None;
                }
                let dot_end = self.last_operator_end(
                    source,
                    span.start,
                    predicate_span.start,
                    OperatorId::Dot,
                )?;
                let pipe_end = self.last_operator_end(
                    source,
                    predicate_span.end,
                    expression_span.start,
                    OperatorId::Bar,
                )?;
                Some(SyntaxAtOffset {
                    construct: SyntaxConstruct::Lambda,
                    parameter: parameter_at(
                        SyntaxConstruct::Lambda,
                        offset >= dot_end,
                        offset >= pipe_end,
                    ),
                })
            }
            _ => None,
        }
    }

    fn last_operator_end(
        &self,
        source: &str,
        start: usize,
        end: usize,
        expected: OperatorId,
    ) -> Option<usize> {
        let text = source.get(start..end)?;
        let mut found = None;
        let mut operator_end = 0;
        for (relative, _) in text.char_indices() {
            if relative < operator_end || self.is_opaque(start + relative) {
                continue;
            }
            let Some((token, range)) = operators::operator_starting_at(text, relative) else {
                continue;
            };
            operator_end = range.end;
            if operators::lookup_token(token).is_some_and(|operator| operator.id == expected) {
                found = Some(start + range.end);
            }
        }
        found
    }

    fn is_opaque(&self, offset: usize) -> bool {
        comments::span_containing(&self.comments, offset).is_some()
            || comments::span_containing(&self.labels, offset).is_some()
    }

    fn scan_incomplete(&self, source: &str, region: Span, offset: usize) -> Option<SyntaxAtOffset> {
        let end = offset.min(region.end).min(source.len());
        let mut bytes = source.as_bytes()[region.start..end].to_vec();
        for span in self.comments.iter().chain(&self.labels) {
            let start = span.start.max(region.start);
            let span_end = span.end.min(end);
            if start < span_end {
                bytes[start - region.start..span_end - region.start].fill(b' ');
            }
        }
        let masked = std::str::from_utf8(&bytes).ok()?;
        let text = masked;

        let mut delimiters: Vec<char> = Vec::new();
        let mut candidates = Vec::<IncompleteCandidate>::new();
        let mut operator_end = 0;
        for (relative, ch) in text.char_indices() {
            if relative < operator_end {
                continue;
            }
            let position = region.start + relative;
            let operator =
                operators::operator_starting_at(text, relative).and_then(|(token, range)| {
                    operator_end = range.end;
                    operators::lookup_token(token).map(|operator| operator.id)
                });
            match ch {
                '(' | '[' => delimiters.push(ch),
                '{' => {
                    delimiters.push(ch);
                    candidates.push(IncompleteCandidate::new(
                        IncompleteKind::Set,
                        position,
                        delimiters.len(),
                    ));
                }
                ')' | ']' | '}' => {
                    let opening = match ch {
                        ')' => '(',
                        ']' => '[',
                        '}' => '{',
                        _ => unreachable!(),
                    };
                    if delimiters.last() == Some(&opening) {
                        delimiters.pop();
                        let depth = delimiters.len();
                        candidates.retain(|candidate| candidate.depth <= depth);
                    } else {
                        delimiters.clear();
                        candidates.clear();
                    }
                }
                ',' if operator.is_none() => {
                    record_item_boundary(&mut candidates, delimiters.len())
                }
                _ => {}
            }
            let Some(operator) = operator else {
                continue;
            };
            match operator {
                OperatorId::ForAll => candidates.push(IncompleteCandidate::new(
                    IncompleteKind::ForAll,
                    position,
                    delimiters.len(),
                )),
                OperatorId::Exists => candidates.push(IncompleteCandidate::new(
                    IncompleteKind::Exists,
                    position,
                    delimiters.len(),
                )),
                OperatorId::Lambda => candidates.push(IncompleteCandidate::new(
                    IncompleteKind::Lambda,
                    position,
                    delimiters.len(),
                )),
                OperatorId::Dot => {
                    record_separator(&mut candidates, delimiters.len(), position, true)
                }
                OperatorId::Bar => {
                    record_separator(&mut candidates, delimiters.len(), position, false)
                }
                _ => {}
            }
        }

        candidates
            .into_iter()
            .filter(|candidate| candidate.is_signature_construct())
            .max_by_key(|candidate| (candidate.depth, candidate.start))
            .map(|candidate| candidate.at_offset(masked, region.start, end))
    }
}

/// Parse `text` once and return an enclosing-span chain for every offset.
///
/// Callers that already own a [`crate::ParseSnapshot`] should use its method.
pub fn enclosing_spans_batch(text: &str, offsets: &[usize]) -> Vec<Vec<Span>> {
    parse_components_guarded(text, |pair| {
        Ok(SyntaxSnapshot::from_pair(text, pair).enclosing_spans(text, offsets))
    })
    .unwrap_or_else(|_| vec![Vec::new(); offsets.len()])
}

/// Return the source spans enclosing `offset`, ordered outermost to innermost.
pub fn enclosing_spans(text: &str, offset: usize) -> Vec<Span> {
    enclosing_spans_batch(text, &[offset])
        .pop()
        .unwrap_or_default()
}

fn build_node(pair: pest::iterators::Pair<Rule>) -> SyntaxNode {
    let rule = pair.as_rule();
    let raw = Span::from_pest(pair.as_span());
    let children = pair.into_inner().map(build_node).collect();
    SyntaxNode {
        rule,
        raw,
        children,
    }
}

fn collect_owned_path(node: &SyntaxNode, source: &str, offset: usize, out: &mut Vec<Span>) {
    if let Some(span) = trim_span(source, node.raw.start, node.raw.end) {
        out.push(span);
    }
    if let Some(child) = node
        .children
        .iter()
        .find(|child| encloses(child.raw, offset))
    {
        collect_owned_path(child, source, offset, out);
    }
}

fn syntax_in_tree(root: &SyntaxNode, offset: usize) -> Option<SyntaxAtOffset> {
    let mut current = root;
    let mut found = None;
    loop {
        found = syntax_for_node(current, offset).or(found);
        let Some(child) = current
            .children
            .iter()
            .find(|child| encloses(child.raw, offset))
        else {
            return found;
        };
        current = child;
    }
}

fn syntax_for_node(node: &SyntaxNode, offset: usize) -> Option<SyntaxAtOffset> {
    match node.rule {
        Rule::quantified_predicate | Rule::quantified_predicate_no_semi => {
            if offset >= last_leaf_end(node) {
                return None;
            }
            let construct = match child(node, Rule::op_forall) {
                Some(_) => SyntaxConstruct::UniversalQuantifier,
                None if child(node, Rule::op_exists).is_some() => {
                    SyntaxConstruct::ExistentialQuantifier
                }
                None => return None,
            };
            let dot = child(node, Rule::dot)?;
            Some(SyntaxAtOffset {
                construct,
                parameter: parameter_at(construct, offset >= dot.raw.end, false),
            })
        }
        Rule::lambda_expr | Rule::lambda_expr_no_semi => {
            if offset >= last_leaf_end(node) {
                return None;
            }
            let dot = child(node, Rule::dot)?;
            let pipe = child(node, Rule::pipe)?;
            Some(SyntaxAtOffset {
                construct: SyntaxConstruct::Lambda,
                parameter: parameter_at(
                    SyntaxConstruct::Lambda,
                    offset >= dot.raw.end,
                    offset >= pipe.raw.end,
                ),
            })
        }
        Rule::set_comprehension => {
            let dot = child(node, Rule::dot);
            let pipe = child(node, Rule::pipe)?;
            let expression_form = dot.is_none()
                && node
                    .children
                    .iter()
                    .take_while(|part| part.rule != Rule::pipe)
                    .any(|part| part.rule == Rule::expression);
            let construct = if dot.is_some() {
                SyntaxConstruct::ExtendedSetComprehension
            } else if expression_form {
                SyntaxConstruct::SetBuilder
            } else {
                SyntaxConstruct::BasicSetComprehension
            };
            Some(SyntaxAtOffset {
                construct,
                parameter: parameter_at(
                    construct,
                    dot.is_some_and(|dot| offset >= dot.raw.end),
                    offset >= pipe.raw.end,
                ),
            })
        }
        _ => None,
    }
}

fn child(node: &SyntaxNode, rule: Rule) -> Option<&SyntaxNode> {
    node.children.iter().find(|child| child.rule == rule)
}

fn last_leaf_end(mut node: &SyntaxNode) -> usize {
    while let Some(last) = node.children.last() {
        node = last;
    }
    node.raw.end
}

#[derive(Clone, Copy)]
enum Formula<'a> {
    Predicate(&'a Predicate),
    Expression(&'a Expression),
    Pattern(&'a IdentPattern),
}

fn push_component_formulas<'a>(component: &'a Component, stack: &mut Vec<Formula<'a>>) {
    match component {
        Component::Context(context) => stack.extend(
            context
                .axioms
                .iter()
                .map(|axiom| Formula::Predicate(&axiom.predicate)),
        ),
        Component::Machine(machine) => {
            stack.extend(
                machine
                    .invariants
                    .iter()
                    .map(|invariant| Formula::Predicate(&invariant.predicate)),
            );
            if let Some(variant) = &machine.variant {
                stack.push(Formula::Expression(variant));
            }
            if let Some(initialisation) = &machine.initialisation {
                stack.extend(
                    initialisation
                        .with
                        .iter()
                        .chain(&initialisation.witnesses)
                        .map(|predicate| Formula::Predicate(&predicate.predicate)),
                );
                for action in &initialisation.actions {
                    push_action_formulas(&action.action.kind, stack);
                }
            }
            for event in &machine.events {
                stack.extend(
                    event
                        .guards
                        .iter()
                        .chain(&event.with)
                        .chain(&event.witnesses)
                        .map(|predicate| Formula::Predicate(&predicate.predicate)),
                );
                for action in &event.actions {
                    push_action_formulas(&action.action.kind, stack);
                }
            }
        }
    }
}

fn push_action_formulas<'a>(kind: &'a ActionKind, stack: &mut Vec<Formula<'a>>) {
    match kind {
        ActionKind::Skip => {}
        ActionKind::Assignment { expressions, .. } => {
            stack.extend(expressions.iter().map(Formula::Expression));
        }
        ActionKind::BecomesIn { set, .. } => stack.push(Formula::Expression(set)),
        ActionKind::BecomesSuchThat { predicate, .. } => {
            stack.push(Formula::Predicate(predicate));
        }
    }
}

fn push_predicate_children<'a>(predicate: &'a Predicate, stack: &mut Vec<Formula<'a>>) {
    match &predicate.kind {
        PredicateKind::True | PredicateKind::False => {}
        PredicateKind::Comparison { left, right, .. } => {
            stack.extend([Formula::Expression(left), Formula::Expression(right)]);
        }
        PredicateKind::Not(inner) => stack.push(Formula::Predicate(inner)),
        PredicateKind::Logical { left, right, .. } => {
            stack.extend([Formula::Predicate(left), Formula::Predicate(right)]);
        }
        PredicateKind::Quantified {
            identifiers,
            predicate,
            ..
        } => {
            stack.extend(
                identifiers
                    .iter()
                    .filter_map(|identifier| identifier.type_expr.as_deref())
                    .map(Formula::Expression),
            );
            stack.push(Formula::Predicate(predicate));
        }
        PredicateKind::Application { arguments, .. }
        | PredicateKind::BuiltinApplication { arguments, .. } => {
            stack.extend(arguments.iter().map(Formula::Expression));
        }
    }
}

fn push_expression_children<'a>(expression: &'a Expression, stack: &mut Vec<Formula<'a>>) {
    match &expression.kind {
        ExpressionKind::Integer(_)
        | ExpressionKind::Identifier(_)
        | ExpressionKind::True
        | ExpressionKind::False
        | ExpressionKind::EmptySet
        | ExpressionKind::Naturals
        | ExpressionKind::Naturals1
        | ExpressionKind::Integers
        | ExpressionKind::BoolType
        | ExpressionKind::AtomicBuiltin(_) => {}
        ExpressionKind::SetEnumeration(items) => {
            stack.extend(items.iter().map(Formula::Expression));
        }
        ExpressionKind::SetComprehension {
            identifiers,
            predicate,
            expression,
        } => {
            stack.extend(
                identifiers
                    .iter()
                    .filter_map(|identifier| identifier.type_expr.as_deref())
                    .map(Formula::Expression),
            );
            stack.push(Formula::Predicate(predicate));
            if let Some(expression) = expression {
                stack.push(Formula::Expression(expression));
            }
        }
        ExpressionKind::SetBuilder {
            member_expression,
            predicate,
        } => {
            stack.extend([
                Formula::Expression(member_expression),
                Formula::Predicate(predicate),
            ]);
        }
        ExpressionKind::RelationalImage { relation, set } => {
            stack.extend([Formula::Expression(relation), Formula::Expression(set)]);
        }
        ExpressionKind::QuantifiedUnion {
            identifiers,
            predicate,
            expression,
        }
        | ExpressionKind::QuantifiedInter {
            identifiers,
            predicate,
            expression,
        } => {
            stack.extend(
                identifiers
                    .iter()
                    .filter_map(|identifier| identifier.type_expr.as_deref())
                    .map(Formula::Expression),
            );
            stack.extend([
                Formula::Predicate(predicate),
                Formula::Expression(expression),
            ]);
        }
        ExpressionKind::Lambda {
            pattern,
            predicate,
            expression,
        } => {
            stack.extend([
                Formula::Pattern(pattern),
                Formula::Predicate(predicate),
                Formula::Expression(expression),
            ]);
        }
        ExpressionKind::Binary { left, right, .. } => {
            stack.extend([Formula::Expression(left), Formula::Expression(right)]);
        }
        ExpressionKind::Unary { operand, .. }
        | ExpressionKind::BuiltinApplication {
            argument: operand, ..
        } => stack.push(Formula::Expression(operand)),
        ExpressionKind::FunctionApplication { function, argument } => {
            stack.extend([Formula::Expression(function), Formula::Expression(argument)]);
        }
        ExpressionKind::Bool(predicate) => stack.push(Formula::Predicate(predicate)),
    }
}

fn push_pattern_children<'a>(pattern: &'a IdentPattern, stack: &mut Vec<Formula<'a>>) {
    match pattern {
        IdentPattern::Identifier(identifier) => {
            if let Some(expression) = identifier.type_expr.as_deref() {
                stack.push(Formula::Expression(expression));
            }
        }
        IdentPattern::Maplet(left, right) => {
            stack.extend([Formula::Pattern(left), Formula::Pattern(right)]);
        }
    }
}

fn encloses(span: Span, offset: usize) -> bool {
    span.start < span.end && span.contains(offset)
}

fn trim_span(text: &str, mut start: usize, mut end: usize) -> Option<Span> {
    let bytes = text.as_bytes();
    while start < end && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    (start < end).then_some(Span { start, end })
}

fn fallback_region(source: &str, errors: &[ParseError], offset: usize) -> Option<Span> {
    if let Some(span) = errors
        .iter()
        .find_map(|error| recovery_span_containing(error, offset))
    {
        return Some(span);
    }

    let cursor_line = Span {
        start: offset,
        end: offset,
    }
    .to_line_col(source)
    .0 + 1;
    if !errors
        .iter()
        .filter_map(ParseError::position)
        .any(|(line, _)| line == cursor_line)
    {
        return None;
    }
    let start = line_start(source, offset);
    let end = source[offset..]
        .find('\n')
        .map_or(source.len(), |position| offset + position);
    Some(Span { start, end })
}

fn recovery_span_containing(error: &ParseError, offset: usize) -> Option<Span> {
    match error {
        ParseError::RecoverableError {
            span: Some(span), ..
        } if span.start <= offset && offset <= span.end => Some(*span),
        ParseError::FileContext { source, .. } => recovery_span_containing(source, offset),
        ParseError::MultipleErrors(errors) => errors
            .iter()
            .find_map(|error| recovery_span_containing(error, offset)),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy)]
enum IncompleteKind {
    ForAll,
    Exists,
    Lambda,
    Set,
}

#[derive(Debug, Clone, Copy)]
struct IncompleteCandidate {
    kind: IncompleteKind,
    start: usize,
    depth: usize,
    dot: Option<usize>,
    pipe: Option<usize>,
}

impl IncompleteCandidate {
    fn new(kind: IncompleteKind, start: usize, depth: usize) -> Self {
        Self {
            kind,
            start,
            depth,
            dot: None,
            pipe: None,
        }
    }

    fn is_signature_construct(&self) -> bool {
        !matches!(self.kind, IncompleteKind::Set) || self.dot.is_some() || self.pipe.is_some()
    }

    fn at_offset(self, source: &str, origin: usize, offset: usize) -> SyntaxAtOffset {
        let construct = match self.kind {
            IncompleteKind::ForAll => SyntaxConstruct::UniversalQuantifier,
            IncompleteKind::Exists => SyntaxConstruct::ExistentialQuantifier,
            IncompleteKind::Lambda => SyntaxConstruct::Lambda,
            IncompleteKind::Set => {
                if self.dot.is_some() {
                    SyntaxConstruct::ExtendedSetComprehension
                } else if looks_like_identifier_list(
                    &source[self.start + 1 - origin..self.pipe.unwrap_or(offset) - origin],
                ) {
                    SyntaxConstruct::BasicSetComprehension
                } else {
                    SyntaxConstruct::SetBuilder
                }
            }
        };
        SyntaxAtOffset {
            construct,
            parameter: parameter_at(
                construct,
                self.dot.is_some_and(|dot| offset > dot),
                self.pipe.is_some_and(|pipe| offset > pipe),
            ),
        }
    }
}

fn record_separator(
    candidates: &mut Vec<IncompleteCandidate>,
    depth: usize,
    position: usize,
    dot: bool,
) {
    let Some(index) = candidates.iter().rposition(|candidate| {
        candidate.depth == depth
            && if dot {
                candidate.dot.is_none()
            } else {
                candidate.pipe.is_none()
                    && matches!(candidate.kind, IncompleteKind::Lambda | IncompleteKind::Set)
                    && (candidate.dot.is_some() || matches!(candidate.kind, IncompleteKind::Set))
            }
    }) else {
        return;
    };
    if dot {
        candidates[index].dot = Some(position);
    } else {
        candidates[index].pipe = Some(position);
        let start = candidates[index].start;
        candidates.retain(|candidate| candidate.depth < depth || candidate.start <= start);
    }
}

fn record_item_boundary(candidates: &mut Vec<IncompleteCandidate>, depth: usize) {
    candidates.retain(|candidate| {
        candidate.depth != depth
            || match candidate.kind {
                IncompleteKind::ForAll | IncompleteKind::Exists => candidate.dot.is_none(),
                IncompleteKind::Set => candidate.dot.is_none() && candidate.pipe.is_none(),
                IncompleteKind::Lambda => false,
            }
    });
}

fn looks_like_identifier_list(text: &str) -> bool {
    let mut start = 0;
    let mut depth = 0usize;
    for (index, ch) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ',')))
    {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                if !looks_like_typed_identifier(&text[start..index]) {
                    return false;
                }
                start = index + 1;
            }
            _ => {}
        }
    }
    true
}

fn looks_like_typed_identifier(text: &str) -> bool {
    let text = text.trim();
    if is_valid_math_identifier(text) {
        return true;
    }
    let spelling = operators::spelling(OperatorId::OfType);
    [spelling.unicode, spelling.ascii]
        .into_iter()
        .any(|separator| {
            text.split_once(separator)
                .is_some_and(|(name, type_expression)| {
                    is_valid_math_identifier(name.trim()) && !type_expression.trim().is_empty()
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_components_snapshot;

    fn at(text: &str, needle: &str) -> usize {
        text.find(needle).expect("needle present in text")
    }

    fn syntax_at(text: &str, offset: usize) -> Option<SyntaxAtOffset> {
        let snapshot = parse_components_snapshot(text);
        snapshot.syntax_at_offset(offset)
    }

    fn slice<'a>(text: &'a str, span: &Span) -> &'a str {
        &text[span.start..span.end]
    }

    #[test]
    fn nests_from_token_to_component() {
        let text = "MACHINE m\nINVARIANTS\n  @inv1 x + 1 > 0\nEND\n";
        let spans = enclosing_spans(text, at(text, "x + 1"));

        assert!(!spans.is_empty(), "expected a span stack");
        assert!(slice(text, &spans[0]).starts_with("MACHINE"));
        assert_eq!(slice(text, spans.last().unwrap()), "x");
        for pair in spans.windows(2) {
            assert!(
                pair[0].end - pair[0].start > pair[1].end - pair[1].start,
                "spans must strictly shrink: {pair:?}"
            );
        }
        let slices: Vec<&str> = spans.iter().map(|span| slice(text, span)).collect();
        assert!(slices.contains(&"x + 1"), "got {slices:?}");
        assert!(slices.contains(&"x + 1 > 0"), "got {slices:?}");
    }

    #[test]
    fn batches_offsets_against_one_snapshot() {
        let text = "MACHINE m\nINVARIANTS\n  @inv1 x + 1 > 0\nEND\n";
        let snapshot = parse_components_snapshot(text);
        let offsets = [at(text, "x + 1"), at(text, "+ 1"), at(text, "> 0")];
        let batched = snapshot.enclosing_spans(&offsets);

        assert_eq!(batched.len(), offsets.len());
        for (chain, offset) in batched.iter().zip(offsets) {
            assert_eq!(*chain, enclosing_spans(text, offset));
        }
    }

    #[test]
    fn handles_multibyte_unicode() {
        let text = "CONTEXT c\nAXIOMS\n  @axm1 a ∈ ℕ\nEND\n";
        let snapshot = parse_components_snapshot(text);
        let chains = snapshot.enclosing_spans(&[at(text, "a ∈"), at(text, "ℕ")]);
        assert_eq!(slice(text, chains[0].last().unwrap()), "a");
        assert_eq!(slice(text, chains[1].last().unwrap()), "ℕ");
        assert_eq!(
            slice(text, enclosing_spans(text, at(text, "ℕ")).last().unwrap()),
            "ℕ"
        );
    }

    #[test]
    fn unparsable_input_yields_empty_selection_chains() {
        let snapshot = parse_components_snapshot("@@@ not an event-b component");
        assert_eq!(snapshot.enclosing_spans(&[0, 4]), vec![vec![], vec![]]);
        assert!(enclosing_spans("@@@ not an event-b component", 0).is_empty());
    }

    #[test]
    fn complete_constructs_advance_at_separators_across_layout() {
        let text = concat!(
            "MACHINE m\nINVARIANTS\n",
            "@q ∀x·\n  x > 0\n",
            "@l (λx·  x > 0 ∣  x + 1)(1) = 2\n",
            "@s {x·  x > 0 ∣  x + 1} ⊆ ℕ\n",
            "END\n",
        );
        for (needle, width, construct, parameter) in [
            (
                "·\n",
                "·".len(),
                SyntaxConstruct::UniversalQuantifier,
                SyntaxParameter::Predicate,
            ),
            (
                "λx·  ",
                "λx·".len(),
                SyntaxConstruct::Lambda,
                SyntaxParameter::Predicate,
            ),
            (
                "0 ∣  x",
                "0 ∣".len(),
                SyntaxConstruct::Lambda,
                SyntaxParameter::Expression,
            ),
            (
                "{x·  ",
                "{x·".len(),
                SyntaxConstruct::ExtendedSetComprehension,
                SyntaxParameter::Predicate,
            ),
            (
                "0 ∣  x + 1}",
                "0 ∣".len(),
                SyntaxConstruct::ExtendedSetComprehension,
                SyntaxParameter::Expression,
            ),
        ] {
            assert_eq!(
                syntax_at(text, at(text, needle) + width),
                Some(SyntaxAtOffset {
                    construct,
                    parameter,
                }),
                "cursor after {needle:?}",
            );
        }
    }

    #[test]
    fn incomplete_fallback_tracks_real_construct_boundaries() {
        let cases = [
            (
                "MACHINE m\nINVARIANTS\n@bad λx·x ∈ r || \nEND\n",
                SyntaxConstruct::Lambda,
                SyntaxParameter::Predicate,
            ),
            (
                "MACHINE m\nINVARIANTS\n@bad {1 ∣ \nEND\n",
                SyntaxConstruct::SetBuilder,
                SyntaxParameter::Predicate,
            ),
            (
                "MACHINE m\nINVARIANTS\n@bad {x | \nEND\n",
                SyntaxConstruct::BasicSetComprehension,
                SyntaxParameter::Predicate,
            ),
            (
                "MACHINE m\nINVARIANTS\n@bad λx,,y· \nEND\n",
                SyntaxConstruct::Lambda,
                SyntaxParameter::Predicate,
            ),
            (
                "MACHINE m\nINVARIANTS\n@bad λx·∀y·y ∈ ℕ ∣ \nEND\n",
                SyntaxConstruct::Lambda,
                SyntaxParameter::Expression,
            ),
            (
                "MACHINE m\nINVARIANTS\n@bad {x·∀y·y ∈ ℕ ∣ \nEND\n",
                SyntaxConstruct::ExtendedSetComprehension,
                SyntaxParameter::Expression,
            ),
        ];
        for (text, construct, parameter) in cases {
            let cursor = text.find(" \nEND").unwrap();
            assert_eq!(
                syntax_at(text, cursor),
                Some(SyntaxAtOffset {
                    construct,
                    parameter,
                }),
                "{text:?}",
            );
        }

        for text in [
            "MACHINE m\nINVARIANTS\n@bad x,y := λz·z ∈ ℕ ∣ z, \nEND\n",
            "MACHINE m\nINVARIANTS\n@bad (λx·x ∈ ℕ ∣ x] y > 0\nEND\n",
        ] {
            let cursor = text.find("\nEND").unwrap();
            assert_eq!(syntax_at(text, cursor), None, "{text:?}");
        }
    }

    #[test]
    fn reports_grammar_aligned_construct_parts() {
        let text = concat!(
            "MACHINE m\nINVARIANTS\n",
            "@q ∀x·x > 0 ⇒ x ∈ ℕ\n",
            "@l (λx·x ∈ ℕ ∣ x + 1)(1) = 2\n",
            "@s {x·x ∈ ℕ ∣ x + 1} ⊆ ℕ\n",
            "@b {x ↦ x + 1 ∣ x ∈ ℕ} ⊆ ℕ × ℕ\n",
            "END\n",
        );
        let snapshot = parse_components_snapshot(text);
        assert_eq!(
            snapshot.syntax_at_offset(at(text, "∀x") + "∀".len()),
            Some(SyntaxAtOffset {
                construct: SyntaxConstruct::UniversalQuantifier,
                parameter: SyntaxParameter::Identifiers,
            })
        );
        assert_eq!(
            snapshot.syntax_at_offset(at(text, "x > 0")),
            Some(SyntaxAtOffset {
                construct: SyntaxConstruct::UniversalQuantifier,
                parameter: SyntaxParameter::Predicate,
            })
        );
        assert_eq!(
            snapshot.syntax_at_offset(at(text, "x ∈ ℕ ∣")),
            Some(SyntaxAtOffset {
                construct: SyntaxConstruct::Lambda,
                parameter: SyntaxParameter::Predicate,
            })
        );
        assert_eq!(
            snapshot.syntax_at_offset(at(text, "x + 1)(1)")),
            Some(SyntaxAtOffset {
                construct: SyntaxConstruct::Lambda,
                parameter: SyntaxParameter::Expression,
            })
        );
        assert_eq!(
            snapshot.syntax_at_offset(at(text, "x + 1} ⊆")),
            Some(SyntaxAtOffset {
                construct: SyntaxConstruct::ExtendedSetComprehension,
                parameter: SyntaxParameter::Expression,
            })
        );
        assert_eq!(
            snapshot.syntax_at_offset(at(text, "x ↦ x + 1")),
            Some(SyntaxAtOffset {
                construct: SyntaxConstruct::SetBuilder,
                parameter: SyntaxParameter::Expression,
            })
        );
    }

    #[test]
    fn ignores_comments_and_completed_constructs() {
        let text = "MACHINE m\nINVARIANTS\n@i ∀x·x > 0 /* ∃y·y > 0 */\nEND\n";
        let snapshot = parse_components_snapshot(text);
        assert!(snapshot.syntax_at_offset(at(text, "∃y")).is_none());
        let quantifier_end = text.find(" /*").unwrap();
        assert!(snapshot.syntax_at_offset(quantifier_end).is_none());
    }

    #[test]
    fn incomplete_fallback_is_error_bounded_and_delimiter_aware() {
        let text = concat!(
            "MACHINE m\nINVARIANTS\n",
            "@bad (λx·x ∈ ℕ ∣ )\n",
            "@later y > 0 // ∀z·\n",
            "END\n",
        );
        let snapshot = parse_components_snapshot(text);
        let in_body = at(text, "∣ )") + "∣".len();
        assert_eq!(
            snapshot.syntax_at_offset(in_body),
            Some(SyntaxAtOffset {
                construct: SyntaxConstruct::Lambda,
                parameter: SyntaxParameter::Expression,
            })
        );
        assert!(snapshot.syntax_at_offset(at(text, "@later") + 2).is_none());
        assert!(snapshot.syntax_at_offset(at(text, "∀z")).is_none());
    }

    #[test]
    fn recovered_sibling_construct_keeps_signature_syntax() {
        let text = concat!(
            "MACHINE m\nINVARIANTS\n",
            "@bad x ∈\n",
            "@good ∀y·y ∈ ℕ\n",
            "END\n",
        );
        let snapshot = parse_components_snapshot(text);
        assert_eq!(
            snapshot.syntax_at_offset(at(text, "y ∈ ℕ")),
            Some(SyntaxAtOffset {
                construct: SyntaxConstruct::UniversalQuantifier,
                parameter: SyntaxParameter::Predicate,
            })
        );
    }
}
