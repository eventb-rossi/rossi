//! Hover documentation provider for Event-B
//!
//! Provides helpful information when hovering over:
//! - Keywords (purpose and usage)
//! - Operators (Unicode and ASCII variants with descriptions)
//! - Identifiers (variables, constants, sets, parameters)
//! - Built-in types and constants

use crate::lsp_types::{Hover, HoverContents, HoverParams, MarkupContent, MarkupKind};
use dashmap::DashMap;
use rossi::{
    Component, Expression, LabeledPredicate, Predicate, PrettyPrinter,
    keywords::{self, KeywordId},
    operators::{self, OperatorId},
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::component_util::{component_at_offset, parse_all, parse_named};
use crate::cross_references::CrossReferenceManager;
use crate::document::DocumentManager;
use crate::identifier_utils::position_to_offset;

/// Context information extracted from a parsed component
#[derive(Debug, Clone)]
struct HoverContext {
    /// Variables with their source (machine name)
    variables: Vec<(String, String)>,
    /// Constants with their source (context name)
    constants: Vec<(String, String)>,
    /// Sets with their source (context name)
    sets: Vec<(String, String)>,
    /// Constraints (axioms/invariants) keyed by identifier name
    constraints: HashMap<String, Vec<String>>,
}

impl HoverContext {
    fn new() -> Self {
        Self {
            variables: Vec::new(),
            constants: Vec::new(),
            sets: Vec::new(),
            constraints: HashMap::new(),
        }
    }

    fn from_component_with_refs(
        component: &Component,
        cross_ref_manager: Option<&CrossReferenceManager>,
        document_manager: Option<&DocumentManager>,
    ) -> Self {
        let mut ctx = Self::new();

        match component {
            Component::Context(context) => {
                for constant in &context.constants {
                    ctx.constants
                        .push((constant.name.clone(), context.name.clone()));
                    let constraints = collect_constraints(&context.axioms, &constant.name);
                    if !constraints.is_empty() {
                        ctx.constraints.insert(constant.name.clone(), constraints);
                    }
                }
                for set in &context.sets {
                    ctx.sets
                        .push((set.name().to_string(), context.name.clone()));
                    let constraints = collect_constraints(&context.axioms, set.name());
                    if !constraints.is_empty() {
                        ctx.constraints.insert(set.name().to_string(), constraints);
                    }
                }

                // Resolve EXTENDS chain transitively
                if let Some(crm) = cross_ref_manager {
                    let mut visited = HashSet::new();
                    visited.insert(context.name.clone());
                    for parent_name in &context.extends {
                        resolve_context_symbols_with_source(
                            parent_name,
                            crm,
                            document_manager,
                            &mut ctx.constants,
                            &mut ctx.sets,
                            &mut ctx.constraints,
                            &mut visited,
                        );
                    }
                }
            }
            Component::Machine(machine) => {
                for var in &machine.variables {
                    ctx.variables.push((var.name.clone(), machine.name.clone()));
                    let constraints = collect_constraints(&machine.invariants, &var.name);
                    if !constraints.is_empty() {
                        ctx.constraints.insert(var.name.clone(), constraints);
                    }
                }

                // Resolve SEES contexts and REFINES machines
                if let Some(crm) = cross_ref_manager {
                    let mut visited = HashSet::new();
                    for ctx_name in &machine.sees {
                        resolve_context_symbols_with_source(
                            ctx_name,
                            crm,
                            document_manager,
                            &mut ctx.constants,
                            &mut ctx.sets,
                            &mut ctx.constraints,
                            &mut visited,
                        );
                    }
                    if let Some(ref refines_name) = machine.refines {
                        resolve_machine_symbols_with_source(
                            refines_name,
                            crm,
                            document_manager,
                            &mut ctx.variables,
                            &mut ctx.constants,
                            &mut ctx.sets,
                            &mut ctx.constraints,
                            &mut visited,
                        );
                    }
                }
            }
        }

        ctx
    }
}

/// Provides hover documentation for Event-B documents
pub struct HoverProvider {
    /// Cache of parsed components per document for quick hover lookup (a
    /// document may define several components)
    component_cache: Arc<DashMap<String, Vec<Component>>>,
    /// Cross-reference manager for workspace-wide navigation
    cross_ref_manager: Option<Arc<CrossReferenceManager>>,
    /// Document manager to access open documents
    document_manager: Option<Arc<DocumentManager>>,
}

impl HoverProvider {
    pub fn new() -> Self {
        Self {
            component_cache: Arc::new(DashMap::new()),
            cross_ref_manager: None,
            document_manager: None,
        }
    }

    /// Set the cross-reference manager for workspace-wide navigation
    pub fn set_cross_reference_manager(&mut self, manager: Arc<CrossReferenceManager>) {
        self.cross_ref_manager = Some(manager);
    }

    /// Set the document manager for accessing open documents
    pub fn set_document_manager(&mut self, manager: Arc<DocumentManager>) {
        self.document_manager = Some(manager);
    }

    /// Update the cached components for a document
    pub fn update_component(&self, uri: String, text: &str) {
        let components = parse_all(text);
        if !components.is_empty() {
            self.component_cache.insert(uri, components);
        } else {
            // Remove from cache if parsing fails
            self.component_cache.remove(&uri);
        }
    }

    /// Generate hover information for the given position
    pub fn hover(&self, params: &HoverParams, text: &str) -> Option<Hover> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .to_string();
        let position = params.text_document_position_params.position;

        // Get hover context from the cached component under the cursor
        let hover_ctx = self
            .component_cache
            .get(&uri)
            .and_then(|entry| {
                let offset = position_to_offset(text, position).unwrap_or(text.len());
                component_at_offset(&entry, offset).map(|component| {
                    HoverContext::from_component_with_refs(
                        component,
                        self.cross_ref_manager.as_deref(),
                        self.document_manager.as_deref(),
                    )
                })
            })
            .unwrap_or_else(HoverContext::new);

        // Get the token at cursor — an identifier or a whole (possibly
        // multi-character) operator like `:=`.
        let (word, range) = word_at_position(text, position)?;

        // Try different hover providers in order
        let mut hover = self
            .hover_keyword(&word)
            .or_else(|| self.hover_operator(&word))
            .or_else(|| self.hover_identifier(&word, &hover_ctx))
            .or_else(|| self.hover_builtin(&word))?;

        // Report the token's span so the client highlights all of `:=`, not
        // whatever its own word pattern guesses.
        hover.range = Some(range);
        Some(hover)
    }

    /// Get hover information for keywords
    fn hover_keyword(&self, word: &str) -> Option<Hover> {
        let id = keywords::lookup(word)?.id;
        KEYWORD_DOCS
            .iter()
            .find(|(doc_id, _, _)| *doc_id == id)
            .map(|(_, title, desc)| create_hover(title, desc))
    }

    /// Get hover information for operators
    fn hover_operator(&self, word: &str) -> Option<Hover> {
        if let Some((title, body)) = lookup_operator_doc(word) {
            return Some(create_hover(&title, body));
        }
        let (title, body) = lookup_doc(BUILTIN_OPERATOR_DOCS, word)?;
        Some(create_hover(title, body))
    }

    /// Get hover information for identifiers
    fn hover_identifier(&self, word: &str, ctx: &HoverContext) -> Option<Hover> {
        // Check if it's a variable
        if let Some((_, source)) = ctx.variables.iter().find(|(name, _)| name == word) {
            let mut description = format!(
                "**Variable** from machine `{}`\n\nState variable that can be modified by events.",
                source
            );
            if let Some(constraints) = ctx.constraints.get(word) {
                description.push_str("\n\n**Invariants:**\n");
                for c in constraints {
                    description.push_str(&format!("- `{}`\n", c));
                }
            }
            return Some(create_hover(&format!("Variable: {}", word), &description));
        }

        // Check if it's a constant
        if let Some((_, source)) = ctx.constants.iter().find(|(name, _)| name == word) {
            let mut description = format!(
                "**Constant** from context `{}`\n\nConstant value constrained by axioms.",
                source
            );
            if let Some(constraints) = ctx.constraints.get(word) {
                description.push_str("\n\n**Axioms:**\n");
                for c in constraints {
                    description.push_str(&format!("- `{}`\n", c));
                }
            }
            return Some(create_hover(&format!("Constant: {}", word), &description));
        }

        // Check if it's a set
        if let Some((_, source)) = ctx.sets.iter().find(|(name, _)| name == word) {
            let mut description = format!(
                "**Set** from context `{}`\n\nCarrier set used for typing.",
                source
            );
            if let Some(constraints) = ctx.constraints.get(word) {
                description.push_str("\n\n**Properties:**\n");
                for c in constraints {
                    description.push_str(&format!("- `{}`\n", c));
                }
            }
            return Some(create_hover(&format!("Set: {}", word), &description));
        }

        None
    }

    /// Get hover information for built-in types
    fn hover_builtin(&self, word: &str) -> Option<Hover> {
        let (title, description) = match word {
            "BOOL" => (
                "BOOL",
                "**Boolean type**\n\nThe set {TRUE, FALSE}.\n\n```eventb\nx ∈ BOOL\n```",
            ),
            "TRUE" => (
                "TRUE",
                "**Boolean true value**\n\nThe true boolean constant.\n\n```eventb\nflag = TRUE\n```",
            ),
            "FALSE" => (
                "FALSE",
                "**Boolean false value**\n\nThe false boolean constant.\n\n```eventb\nflag = FALSE\n```",
            ),
            "ℕ" | "NAT" => (
                "ℕ (Natural Numbers)",
                "**Natural numbers**\n\nThe set of non-negative integers: {0, 1, 2, 3, ...}\n\n```eventb\nn ∈ ℕ\nn : NAT  // ASCII alternative\n```",
            ),
            "ℕ1" | "NAT1" => (
                "ℕ1 (Positive Natural Numbers)",
                "**Positive natural numbers**\n\nThe set of positive integers: {1, 2, 3, ...}\n\n```eventb\nn ∈ ℕ1\nn : NAT1  // ASCII alternative\n```",
            ),
            "ℤ" | "INT" => (
                "ℤ (Integers)",
                "**Integers**\n\nThe set of all integers: {..., -2, -1, 0, 1, 2, ...}\n\n```eventb\nx ∈ ℤ\nx : INT  // ASCII alternative\n```",
            ),

            _ => return None,
        };

        Some(create_hover(title, description))
    }
}

impl Default for HoverProvider {
    fn default() -> Self {
        Self::new()
    }
}

// Cross-document resolution helpers

/// Resolve constants and sets (with source context name) from a context and its
/// EXTENDS parents transitively. Uses a visited set to prevent cycles; caps depth at 10.
fn resolve_context_symbols_with_source(
    context_name: &str,
    cross_ref_manager: &CrossReferenceManager,
    document_manager: Option<&DocumentManager>,
    constants: &mut Vec<(String, String)>,
    sets: &mut Vec<(String, String)>,
    constraints: &mut HashMap<String, Vec<String>>,
    visited: &mut HashSet<String>,
) {
    if visited.len() >= 10 || visited.contains(context_name) {
        return;
    }
    visited.insert(context_name.to_string());

    let text = match cross_ref_manager.load_component_text(context_name, document_manager) {
        Some(t) => t,
        None => return,
    };

    let component = match parse_named(&text, context_name) {
        Some(c) => c,
        None => return,
    };

    if let Component::Context(ctx) = &component {
        for constant in &ctx.constants {
            constants.push((constant.name.clone(), ctx.name.clone()));
            let c = collect_constraints(&ctx.axioms, &constant.name);
            if !c.is_empty() {
                constraints.insert(constant.name.clone(), c);
            }
        }
        for set in &ctx.sets {
            sets.push((set.name().to_string(), ctx.name.clone()));
            let c = collect_constraints(&ctx.axioms, set.name());
            if !c.is_empty() {
                constraints.insert(set.name().to_string(), c);
            }
        }

        // Recursively resolve EXTENDS parents
        for parent_name in &ctx.extends {
            resolve_context_symbols_with_source(
                parent_name,
                cross_ref_manager,
                document_manager,
                constants,
                sets,
                constraints,
                visited,
            );
        }
    }
}

/// Resolve variables (with source machine name) from a refined machine and its
/// transitive REFINES/SEES dependencies.
#[allow(clippy::too_many_arguments)]
fn resolve_machine_symbols_with_source(
    machine_name: &str,
    cross_ref_manager: &CrossReferenceManager,
    document_manager: Option<&DocumentManager>,
    variables: &mut Vec<(String, String)>,
    constants: &mut Vec<(String, String)>,
    sets: &mut Vec<(String, String)>,
    constraints: &mut HashMap<String, Vec<String>>,
    visited: &mut HashSet<String>,
) {
    if visited.len() >= 10 || visited.contains(machine_name) {
        return;
    }
    visited.insert(machine_name.to_string());

    let text = match cross_ref_manager.load_component_text(machine_name, document_manager) {
        Some(t) => t,
        None => return,
    };

    let component = match parse_named(&text, machine_name) {
        Some(c) => c,
        None => return,
    };

    if let Component::Machine(m) = &component {
        for var in &m.variables {
            variables.push((var.name.clone(), m.name.clone()));
            let c = collect_constraints(&m.invariants, &var.name);
            if !c.is_empty() {
                constraints.insert(var.name.clone(), c);
            }
        }

        // Resolve SEES contexts from the abstract machine
        for ctx_name in &m.sees {
            resolve_context_symbols_with_source(
                ctx_name,
                cross_ref_manager,
                document_manager,
                constants,
                sets,
                constraints,
                visited,
            );
        }

        // Recurse into further refinements
        if let Some(ref refines_name) = m.refines {
            resolve_machine_symbols_with_source(
                refines_name,
                cross_ref_manager,
                document_manager,
                variables,
                constants,
                sets,
                constraints,
                visited,
            );
        }
    }
}

// AST traversal helpers

/// Check whether an expression references identifier `id`.
fn expression_mentions_id(expr: &Expression, id: &str) -> bool {
    match expr {
        Expression::Identifier(name) => name == id,
        Expression::Binary { left, right, .. } => {
            expression_mentions_id(left, id) || expression_mentions_id(right, id)
        }
        Expression::Unary { operand, .. } => expression_mentions_id(operand, id),
        Expression::FunctionApplication {
            function,
            arguments,
        } => {
            expression_mentions_id(function, id)
                || arguments.iter().any(|a| expression_mentions_id(a, id))
        }
        Expression::BuiltinApplication { arguments, .. } => {
            arguments.iter().any(|a| expression_mentions_id(a, id))
        }
        Expression::SetEnumeration(elems) => elems.iter().any(|e| expression_mentions_id(e, id)),
        Expression::SetComprehension {
            predicate,
            expression,
            ..
        } => {
            predicate_mentions_id(predicate, id)
                || expression
                    .as_ref()
                    .is_some_and(|e| expression_mentions_id(e, id))
        }
        Expression::RelationalImage { relation, set } => {
            expression_mentions_id(relation, id) || expression_mentions_id(set, id)
        }
        Expression::QuantifiedUnion {
            predicate,
            expression,
            ..
        }
        | Expression::QuantifiedInter {
            predicate,
            expression,
            ..
        }
        | Expression::Lambda {
            predicate,
            expression,
            ..
        } => predicate_mentions_id(predicate, id) || expression_mentions_id(expression, id),
        Expression::Bool(pred) => predicate_mentions_id(pred, id),
        _ => false, // Integer, True, False, EmptySet, Naturals, etc.
    }
}

/// Check whether a predicate references identifier `id`.
fn predicate_mentions_id(pred: &Predicate, id: &str) -> bool {
    match pred {
        Predicate::Comparison { left, right, .. } => {
            expression_mentions_id(left, id) || expression_mentions_id(right, id)
        }
        Predicate::Not(p) => predicate_mentions_id(p, id),
        Predicate::Logical { left, right, .. } => {
            predicate_mentions_id(left, id) || predicate_mentions_id(right, id)
        }
        Predicate::Quantified { predicate, .. } => predicate_mentions_id(predicate, id),
        Predicate::Application { arguments, .. } => {
            arguments.iter().any(|a| expression_mentions_id(a, id))
        }
        Predicate::BuiltinApplication { arguments, .. } => {
            arguments.iter().any(|a| expression_mentions_id(a, id))
        }
        Predicate::True | Predicate::False => false,
    }
}

/// Collect formatted constraint strings from labeled predicates that mention `id`.
/// Capped at 5 results to avoid clutter.
fn collect_constraints(predicates: &[LabeledPredicate], id: &str) -> Vec<String> {
    let printer = PrettyPrinter::new();
    predicates
        .iter()
        .filter(|lp| predicate_mentions_id(&lp.predicate, id))
        .take(5)
        .map(|lp| {
            let text = printer.print_predicate(&lp.predicate);
            match &lp.label {
                Some(label) => format!("{}: {}", label, text),
                None => text,
            }
        })
        .collect()
}

// Helper functions

fn create_hover(title: &str, description: &str) -> Hover {
    let content = format!("# {}\n\n{}", title, description);

    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: content,
        }),
        range: None,
    }
}

use crate::identifier_utils::word_at_position;

/// Documentation entry: `(keys, title, markdown description)`.
type DocEntry = (&'static [&'static str], &'static str, &'static str);
/// Operator documentation entry: `(id, markdown body)`. The title is derived from
/// the canonical [`operators::OperatorSpelling`] (glyph + description) so the
/// operator's name and glyph live in exactly one place — the `rossi::operators`
/// table — and cannot drift from the spellings used by completion.
type OperatorDocEntry = (OperatorId, &'static str);
type KeywordDocEntry = (KeywordId, &'static str, &'static str);

fn lookup_doc(table: &[DocEntry], word: &str) -> Option<(&'static str, &'static str)> {
    table
        .iter()
        .find(|(keys, _, _)| keys.contains(&word))
        .map(|(_, title, desc)| (*title, *desc))
}

/// Build a hover title from the canonical spelling: `"<glyph> (<description>)"`.
fn operator_title(spelling: &operators::OperatorSpelling) -> String {
    format!("{} ({})", display_glyph(spelling), spelling.description)
}

/// Glyph to show in hover titles. Operators without a standard glyph use
/// private-use-area code points (U+E000..=U+F8FF) that won't render in fonts, so
/// fall back to the ASCII spelling for those.
fn display_glyph(spelling: &operators::OperatorSpelling) -> &'static str {
    if spelling
        .unicode
        .chars()
        .any(|c| ('\u{E000}'..='\u{F8FF}').contains(&c))
    {
        spelling.ascii
    } else {
        spelling.unicode
    }
}

fn lookup_operator_doc(word: &str) -> Option<(String, &'static str)> {
    let spelling = operators::lookup_token(word)?;
    let body = OPERATOR_DOCS
        .iter()
        .find(|(id, _)| *id == spelling.id)
        .map(|(_, body)| *body)?;
    Some((operator_title(spelling), body))
}

const KEYWORD_DOCS: &[KeywordDocEntry] = &[
    // Top-level
    (
        KeywordId::Context,
        "CONTEXT",
        "Defines a context containing static properties of a model.\n\nA context can declare sets, constants, axioms, and theorems.",
    ),
    (
        KeywordId::Machine,
        "MACHINE",
        "Defines a machine containing dynamic behavior.\n\nA machine can declare variables, invariants, variants, and events.",
    ),
    (
        KeywordId::End,
        "END",
        "Marks the end of a context, machine, or event definition.",
    ),
    // Context clauses
    (
        KeywordId::Extends,
        "EXTENDS",
        "Extends another context, inheriting its sets, constants, and axioms.\n\n```eventb\nEXTENDS\n    base_context\n```\n\nAt the event level, `EXTENDS` marks an extended event that inherits the abstract event's parameters, guards, and actions.",
    ),
    (
        KeywordId::Sets,
        "SETS",
        "Declares carrier sets (enumerated or deferred).\n\n```eventb\nSETS\n    STATUS\n    COLORS\n```",
    ),
    (
        KeywordId::Constants,
        "CONSTANTS",
        "Declares constants whose values are constrained by axioms.\n\n```eventb\nCONSTANTS\n    max_value\n    min_value\n```",
    ),
    (
        KeywordId::Axioms,
        "AXIOMS",
        "Declares axioms (properties) that must hold for constants and sets.\n\n```eventb\nAXIOMS\n    @axm1 max_value > 0\n    @axm2 max_value = 100\n```",
    ),
    // Machine clauses
    (
        KeywordId::Refines,
        "REFINES",
        "Refines an abstract machine, adding more detail.\n\n```eventb\nREFINES\n    abstract_machine\n```",
    ),
    (
        KeywordId::Sees,
        "SEES",
        "References contexts to use their sets and constants.\n\n```eventb\nSEES\n    context_name\n```",
    ),
    (
        KeywordId::Variables,
        "VARIABLES",
        "Declares state variables.\n\n```eventb\nVARIABLES\n    count\n    total\n```",
    ),
    (
        KeywordId::Invariants,
        "INVARIANTS",
        "Declares invariants (properties) that must always hold.\n\n```eventb\nINVARIANTS\n    @inv1 count >= 0\n    @inv2 count <= max_value\n```",
    ),
    (
        KeywordId::Theorems,
        "THEOREMS",
        "Declares theorems — properties proved once from the axioms/invariants, not preserved by every event. Equivalent to the inline `theorem @x` form, and stored/exported as theorem-flagged axioms/invariants (Rodin has no separate theorems container).\n\n```eventb\nTHEOREMS\n    @thm1 count ∈ ℕ\n```",
    ),
    (
        KeywordId::Variant,
        "VARIANT",
        "Declares a variant expression for proving termination.\n\n```eventb\nVARIANT\n    max_value - count\n```",
    ),
    (
        KeywordId::Events,
        "EVENTS",
        "Begins the events section of a machine.\n\n```eventb\nEVENTS\n    EVENT INITIALISATION\n    ...\n    EVENT event_name\n    ...\nEND\n```",
    ),
    // Event keywords
    (
        KeywordId::Event,
        "EVENT",
        "Defines an event that can change the machine state.\n\n```eventb\nEVENT increment\nWHERE\n    @grd1 count < max_value\nTHEN\n    @act1 count := count + 1\nEND\n```",
    ),
    (
        KeywordId::Initialisation,
        "INITIALISATION",
        "Special event that initializes machine variables.\n\n```eventb\nEVENT INITIALISATION\nTHEN\n    count := 0\nEND\n```",
    ),
    (
        KeywordId::Status,
        "STATUS",
        "Specifies the convergence status of an event.\n\nValues: `ordinary`, `convergent`, `anticipated`",
    ),
    (
        KeywordId::Any,
        "ANY",
        "Introduces event parameters (local variables).\n\n```eventb\nANY x\nWHERE\n    @grd1 x ∈ ℕ\nTHEN\n    @act1 count := x\nEND\n```",
    ),
    (
        KeywordId::Where,
        "WHERE/WHEN",
        "Declares event guards (preconditions).\n\n```eventb\nWHERE\n    @grd1 count < max_value\n    @grd2 count >= 0\n```",
    ),
    (
        KeywordId::With,
        "WITH",
        "Specifies witness predicates for refinement.\n\n```eventb\nWITH\n    @x x = count + 1\n```",
    ),
    (
        KeywordId::Witness,
        "WITNESS",
        "Declares witness predicates for abstract parameters.\n\n```eventb\nWITNESS\n    @x x = count + 1\n```",
    ),
    (
        KeywordId::Then,
        "THEN/BEGIN",
        "Declares event actions (state changes).\n\n```eventb\nTHEN\n    @act1 count := count + 1\n    @act2 total := total + count\n```",
    ),
    // Inline modifiers
    (
        KeywordId::Theorem,
        "theorem",
        "Marks a labeled predicate as a theorem — a property that follows from the others and is proved once, not preserved by every event.\n\n```eventb\nINVARIANTS\n    @thm1 theorem count ∈ ℕ\n```",
    ),
    (
        KeywordId::Skip,
        "skip",
        "A no-op action that makes no state change.\n\n```eventb\nTHEN\n    skip\n```",
    ),
    // Event status values
    (
        KeywordId::Ordinary,
        "ordinary",
        "Ordinary event (default). Does not affect variant.",
    ),
    (
        KeywordId::Convergent,
        "convergent",
        "Convergent event. Must decrease the variant, proving termination.",
    ),
    (
        KeywordId::Anticipated,
        "anticipated",
        "Anticipated event. May increase variant but will be refined to convergent.",
    ),
];

// Bodies hold only the explanation + examples. The title (glyph + name) is derived
// from the canonical `operators::OperatorSpelling`, so it is intentionally absent
// here and must not be restated in the body.
const OPERATOR_DOCS: &[OperatorDocEntry] = &[
    // Logical operators
    (
        OperatorId::And,
        "Returns true if both operands are true.\n\n```eventb\nP ∧ Q\nP & Q  // ASCII alternative\n```",
    ),
    (
        OperatorId::Or,
        "Returns true if at least one operand is true.\n\n```eventb\nP ∨ Q\nP or Q  // ASCII alternative\n```",
    ),
    (
        OperatorId::Not,
        "Returns the opposite truth value.\n\n```eventb\n¬P\nnot P  // ASCII alternative\n```",
    ),
    (
        OperatorId::Implies,
        "P ⇒ Q means \"if P then Q\".\n\n```eventb\nx > 0 ⇒ x ≠ 0\nx > 0 => x /= 0  // ASCII alternative\n```",
    ),
    (
        OperatorId::Equivalent,
        "P ⇔ Q means \"P if and only if Q\".\n\n```eventb\nx = 0 ⇔ ¬(x > 0)\nx = 0 <=> not(x > 0)  // ASCII alternative\n```",
    ),
    (
        OperatorId::ForAll,
        "Reads as \"for all x such that x is in S, P(x) holds\".\n\n```eventb\n∀ x · x ∈ S ⇒ P(x)\n! x . (x : S => P(x))  // ASCII alternative\n```",
    ),
    (
        OperatorId::Exists,
        "Reads as \"there exists an x in S such that P(x) holds\".\n\n```eventb\n∃ x · x ∈ S ∧ P(x)\n# x . (x : S & P(x))  // ASCII alternative\n```",
    ),
    // Set operators
    (
        OperatorId::In,
        "Checks if an element belongs to a set.\n\n```eventb\nx ∈ ℕ\nx : NAT  // ASCII alternative\n```",
    ),
    (
        OperatorId::NotIn,
        "Checks if an element does not belong to a set.\n\n```eventb\nx ∉ S\nx /: S  // ASCII alternative\n```",
    ),
    (
        OperatorId::Subset,
        "A ⊆ B means all elements of A are in B.\n\n```eventb\nA ⊆ B\nA <: B  // ASCII alternative\n```",
    ),
    (
        OperatorId::SubsetStrict,
        "A ⊂ B means A ⊆ B and A ≠ B.\n\n```eventb\nA ⊂ B\nA <<: B  // ASCII alternative\n```",
    ),
    (
        OperatorId::NotSubset,
        "A ⊈ B means at least one element of A is not in B.\n\n```eventb\nA ⊈ B\nA /<: B  // ASCII alternative\n```",
    ),
    (
        OperatorId::NotSubsetStrict,
        "A ⊄ B means A is not a strict subset of B.\n\n```eventb\nA ⊄ B\nA /<<: B  // ASCII alternative\n```",
    ),
    (
        OperatorId::Union,
        "A ∪ B contains all elements in A or B.\n\n```eventb\nA ∪ B\nA \\/ B  // ASCII alternative\n```",
    ),
    (
        OperatorId::Intersection,
        "A ∩ B contains elements in both A and B.\n\n```eventb\nA ∩ B\nA /\\ B  // ASCII alternative\n```",
    ),
    (
        OperatorId::Difference,
        "A ∖ B contains elements in A but not in B.\n\n```eventb\nA ∖ B\nA \\ B  // ASCII alternative\n```",
    ),
    (
        OperatorId::PowerSet,
        "ℙ(S) is the set of all subsets of S.\n\n```eventb\nℙ(S)\nPOW(S)  // ASCII alternative\n```",
    ),
    (
        OperatorId::EmptySet,
        "The set containing no elements.\n\n```eventb\n∅\n{}  // ASCII alternative\n```",
    ),
    // Relation operators
    (
        OperatorId::Relation,
        "A ↔ B is the set of all relations between A and B, i.e. ℙ(A × B).\n\n```eventb\nA ↔ B\nA <-> B  // ASCII alternative\n```",
    ),
    (
        OperatorId::TotalRelation,
        "Relates every element of the source set.\n\n```eventb\nA \u{E100} B\nA <<-> B  // ASCII alternative\n```",
    ),
    (
        OperatorId::SurjectiveRelation,
        "Covers every element of the target set.\n\n```eventb\nA \u{E101} B\nA <->> B  // ASCII alternative\n```",
    ),
    (
        OperatorId::TotalSurjectiveRelation,
        "Both total on the source set and surjective onto the target set.\n\n```eventb\nA \u{E102} B\nA <<->> B  // ASCII alternative\n```",
    ),
    (
        OperatorId::TotalFunction,
        "A → B is a function defined for all elements of A.\n\n```eventb\nf ∈ A → B\nf : A --> B  // ASCII alternative\n```",
    ),
    (
        OperatorId::PartialFunction,
        "A ⇸ B is a function that may not be defined for all elements of A.\n\n```eventb\nf ∈ A ⇸ B\nf : A +-> B  // ASCII alternative\n```",
    ),
    (
        OperatorId::Maplet,
        "Creates an ordered pair, used to build relations.\n\n```eventb\nx ↦ y\nx |-> y  // ASCII alternative\n```",
    ),
    (
        OperatorId::Composition,
        "f ∘ g applies g first, then f.\n\n```eventb\nf ∘ g\nf circ g  // ASCII alternative\n```",
    ),
    (
        OperatorId::Semicolon,
        "f ; g applies f first, then g.\n\n```eventb\nf ; g\n```",
    ),
    (
        OperatorId::CartesianProduct,
        "A × B is the set of all pairs (a, b).\n\n```eventb\nA × B\nA ** B  // ASCII alternative\n```",
    ),
    (
        OperatorId::TotalInjection,
        "A ↣ B is a total function that is injective.\n\n```eventb\nf ∈ A ↣ B\nf : A >-> B  // ASCII alternative\n```",
    ),
    (
        OperatorId::PartialInjection,
        "A ⤔ B is a partial function that is injective.\n\n```eventb\nf ∈ A ⤔ B\nf : A >+> B  // ASCII alternative\n```",
    ),
    (
        OperatorId::TotalSurjection,
        "A ↠ B is a total function that is surjective.\n\n```eventb\nf ∈ A ↠ B\nf : A ->> B  // ASCII alternative\n```",
    ),
    (
        OperatorId::PartialSurjection,
        "A ⤀ B is a partial function that is surjective.\n\n```eventb\nf ∈ A ⤀ B\nf : A +>> B  // ASCII alternative\n```",
    ),
    (
        OperatorId::Bijection,
        "A ⤖ B is a total function that is both injective and surjective.\n\n```eventb\nf ∈ A ⤖ B\nf : A >->> B  // ASCII alternative\n```",
    ),
    (
        OperatorId::Domain,
        "Returns the set of first elements of the pairs in a relation.\n\n```eventb\ndom(r)\n```",
    ),
    (
        OperatorId::RangeOfRelation,
        "Returns the set of second elements of the pairs in a relation.\n\n```eventb\nran(r)\n```",
    ),
    (
        OperatorId::DomainRestriction,
        "S ◁ r restricts r to pairs whose first element is in S.\n\n```eventb\nS ◁ r\nS <| r  // ASCII alternative\n```",
    ),
    (
        OperatorId::DomainSubtraction,
        "S ⩤ r removes pairs whose first element is in S.\n\n```eventb\nS ⩤ r\nS <<| r  // ASCII alternative\n```",
    ),
    (
        OperatorId::RangeRestriction,
        "r ▷ S restricts r to pairs whose second element is in S.\n\n```eventb\nr ▷ S\nr |> S  // ASCII alternative\n```",
    ),
    (
        OperatorId::RangeSubtraction,
        "r ⩥ S removes pairs whose second element is in S.\n\n```eventb\nr ⩥ S\nr |>> S  // ASCII alternative\n```",
    ),
    (
        OperatorId::Overwrite,
        "r \u{E103} s overrides r with s where they overlap.\n\n```eventb\nr \u{E103} s\nr <+ s  // ASCII alternative\n```",
    ),
    (
        OperatorId::DirectProduct,
        "Combines two relations into pairs of their images.\n\n```eventb\nr ⊗ s\nr >< s  // ASCII alternative\n```",
    ),
    (
        OperatorId::ParallelProduct,
        "Applies two relations in parallel on pairs.\n\n```eventb\nr ∥ s\nr || s  // ASCII alternative\n```",
    ),
    (
        OperatorId::Inverse,
        "r∼ reverses all pairs in the relation.\n\n```eventb\nr∼\nr~  // ASCII alternative\n```",
    ),
    (
        OperatorId::OfType,
        "Annotates an expression with its type (\"oftype\").\n\n```eventb\nE ⦂ T\nE oftype T  // ASCII alternative\n```",
    ),
    (
        OperatorId::Range,
        "a‥b is the set of integers from a to b inclusive.\n\n```eventb\n1‥10\n1..10  // ASCII alternative\n```",
    ),
    (
        OperatorId::Lambda,
        "Defines a function by an expression over a bound variable.\n\n```eventb\nλ x · x ∈ S ∣ E\n% x . (x : S | E)  // ASCII alternative\n```",
    ),
    // Quantifier and comprehension separators
    (
        OperatorId::Dot,
        "Separates the bound variable list from the body in quantifiers, lambdas, and comprehensions.\n\n```eventb\n∀ x · P\n! x . P  // ASCII alternative\n```",
    ),
    (
        OperatorId::Bar,
        "Separates the predicate from the expression in set comprehensions, lambdas, and quantified unions/intersections.\n\n```eventb\n{ x · x ∈ S ∣ f(x) }\n{ x . x : S | f(x) }  // ASCII alternative\n```",
    ),
    (
        OperatorId::QuantifiedUnion,
        "⋃ x · P ∣ E takes the union over all values satisfying P.\n\n```eventb\n⋃ x · x ∈ S ∣ f(x)\nUNION x . (x : S | f(x))  // ASCII alternative\n```",
    ),
    (
        OperatorId::QuantifiedIntersection,
        "⋂ x · P ∣ E takes the intersection over all values satisfying P.\n\n```eventb\n⋂ x · x ∈ S ∣ f(x)\nINTER x . (x : S | f(x))  // ASCII alternative\n```",
    ),
    (
        OperatorId::PowerSet1,
        "ℙ1(S) is the set of all non-empty subsets of S.\n\n```eventb\nℙ1(S)\nPOW1(S)  // ASCII alternative\n```",
    ),
    // Assignment operators
    (
        OperatorId::Assignment,
        "Assigns a specific value to a variable.\n\n```eventb\ncount ≔ count + 1\ncount := count + 1  // ASCII alternative\n```",
    ),
    (
        OperatorId::BecomesSuchThat,
        "Assigns any value satisfying a predicate (\"such that\").\n\n```eventb\ncount :∣ count' ∈ ℕ ∧ count' > 0\ncount :| count' : NAT & count' > 0  // ASCII alternative\n```",
    ),
    (
        OperatorId::BecomesIn,
        "Assigns any value drawn from a set (\"member of\").\n\n```eventb\ncount :∈ ℕ\ncount :: NAT  // ASCII alternative\n```",
    ),
    // Comparison operators
    (
        OperatorId::Equal,
        "Checks if two values are equal.\n\n```eventb\nx = 5\n```",
    ),
    (
        OperatorId::NotEqual,
        "Checks if two values are different.\n\n```eventb\nx ≠ 0\nx /= 0  // ASCII alternative\n```",
    ),
    (
        OperatorId::LessThan,
        "Checks if the left value is strictly less than the right.\n\n```eventb\nx < 10\n```",
    ),
    (
        OperatorId::GreaterThan,
        "Checks if the left value is strictly greater than the right.\n\n```eventb\nx > 0\n```",
    ),
    (
        OperatorId::LessEqual,
        "Checks if the left value is less than or equal to the right.\n\n```eventb\nx ≤ 100\nx <= 100  // ASCII alternative\n```",
    ),
    (
        OperatorId::GreaterEqual,
        "Checks if the left value is greater than or equal to the right.\n\n```eventb\nx ≥ 0\nx >= 0  // ASCII alternative\n```",
    ),
    // Arithmetic operators
    (
        OperatorId::Add,
        "Adds two integers.\n\n```eventb\nx + y\n```",
    ),
    (
        OperatorId::Subtract,
        "Subtracts the right integer from the left.\n\n```eventb\nx − y\nx - y  // ASCII alternative\n```",
    ),
    (
        OperatorId::Multiply,
        "Multiplies two integers.\n\n```eventb\nx ∗ y\nx * y  // ASCII alternative\n```",
    ),
    (
        OperatorId::Divide,
        "Integer division.\n\n```eventb\nx ÷ y\nx / y  // ASCII alternative\n```",
    ),
    (
        OperatorId::Modulo,
        "Returns the remainder after integer division.\n\n```eventb\nx mod y\n```",
    ),
    (
        OperatorId::Exponent,
        "Raises x to the power n.\n\n```eventb\nx ^ n\n```",
    ),
];

const BUILTIN_OPERATOR_DOCS: &[DocEntry] = &[
    (
        &["min"],
        "Minimum",
        "**Minimum of a set**\n\nReturns the smallest element.\n\n```eventb\nmin(S)\n```",
    ),
    (
        &["max"],
        "Maximum",
        "**Maximum of a set**\n\nReturns the largest element.\n\n```eventb\nmax(S)\n```",
    ),
    (
        &["card"],
        "card (Cardinality)",
        "**Set cardinality**\n\nReturns the number of elements in a finite set.\n\n```eventb\ncard(S)\n```",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp_types::{Position, Range, Url};

    fn word_at(text: &str, position: Position) -> Option<String> {
        word_at_position(text, position).map(|(word, _)| word)
    }

    fn hover_at(
        provider: &HoverProvider,
        source: &str,
        line: u32,
        character: u32,
    ) -> Option<Hover> {
        provider.hover(
            &HoverParams {
                text_document_position_params: crate::lsp_types::TextDocumentPositionParams {
                    text_document: crate::lsp_types::TextDocumentIdentifier {
                        uri: Url::parse("file:///test.eventb").unwrap(),
                    },
                    position: Position::new(line, character),
                },
                work_done_progress_params: Default::default(),
            },
            source,
        )
    }

    #[test]
    fn test_hover_provider_creation() {
        let provider = HoverProvider::new();
        assert!(provider.component_cache.is_empty());
    }

    #[test]
    fn test_hover_keyword() {
        let provider = HoverProvider::new();

        let hover = provider.hover_keyword("CONTEXT");
        assert!(hover.is_some());
        let hover = hover.unwrap();
        if let HoverContents::Markup(content) = hover.contents {
            assert!(content.value.contains("CONTEXT"));
            assert!(content.value.contains("static properties"));
        }
    }

    #[test]
    fn test_hover_operator_unicode() {
        let provider = HoverProvider::new();

        let hover = provider.hover_operator("∧");
        assert!(hover.is_some());
        let hover = hover.unwrap();
        if let HoverContents::Markup(content) = hover.contents {
            // Title is derived from the canonical description ("Logical and").
            assert!(content.value.contains("Logical and"));
            assert!(content.value.contains("both operands"));
        }
    }

    #[test]
    fn test_hover_operator_ascii() {
        let provider = HoverProvider::new();

        let hover = provider.hover_operator("&");
        assert!(hover.is_some());
        let hover = hover.unwrap();
        if let HoverContents::Markup(content) = hover.contents {
            assert!(content.value.contains("Logical and"));
            assert!(content.value.contains("both operands"));
        }

        // /\ is now set intersection only (no longer logical AND)
        let hover = provider.hover_operator("/\\");
        assert!(hover.is_some());
        let hover = hover.unwrap();
        if let HoverContents::Markup(content) = hover.contents {
            assert!(content.value.contains("Set intersection"));
            assert!(content.value.contains("∩"));
        }
    }

    #[test]
    fn test_hover_builtin() {
        let provider = HoverProvider::new();

        let hover = provider.hover_builtin("ℕ");
        assert!(hover.is_some());
        let hover = hover.unwrap();
        if let HoverContents::Markup(content) = hover.contents {
            assert!(content.value.contains("Natural"));
            assert!(content.value.contains("0, 1, 2"));
        }

        let hover = provider.hover_builtin("NAT");
        assert!(hover.is_some());
    }

    #[test]
    fn test_hover_identifier() {
        let provider = HoverProvider::new();

        let ctx = HoverContext {
            variables: vec![("count".to_string(), "counter".to_string())],
            constants: vec![("max_value".to_string(), "counter_ctx".to_string())],
            sets: vec![("STATUS".to_string(), "counter_ctx".to_string())],
            constraints: HashMap::new(),
        };

        let hover = provider.hover_identifier("count", &ctx);
        assert!(hover.is_some());
        let hover = hover.unwrap();
        if let HoverContents::Markup(content) = hover.contents {
            assert!(content.value.contains("Variable"));
            assert!(content.value.contains("counter"));
        }

        let hover = provider.hover_identifier("max_value", &ctx);
        assert!(hover.is_some());

        let hover = provider.hover_identifier("STATUS", &ctx);
        assert!(hover.is_some());
    }

    #[test]
    fn test_component_caching() {
        let provider = HoverProvider::new();
        let source = "CONTEXT test\nCONSTANTS\n    max_value\nEND";

        provider.update_component("file:///test.eventb".to_string(), source);

        assert!(provider.component_cache.contains_key("file:///test.eventb"));
    }

    #[test]
    fn test_word_at_position() {
        let text = "CONTEXT test_context";

        // Position at 'C' (start of CONTEXT)
        let word = word_at(text, Position::new(0, 0));
        assert_eq!(word, Some("CONTEXT".to_string()));

        // Position at 't' (in test_context)
        let word = word_at(text, Position::new(0, 8));
        assert_eq!(word, Some("test_context".to_string()));
    }

    #[test]
    fn test_word_at_position_unicode() {
        // Line with Unicode operators — previously panicked due to byte slicing
        let text = "    @inv1 count ∈ ℕ";

        // Hovering on 'count' (char index 10)
        let word = word_at(text, Position::new(0, 10));
        assert_eq!(word, Some("count".to_string()));

        // Hovering on 'inv1' (char index 5)
        let word = word_at(text, Position::new(0, 5));
        assert_eq!(word, Some("inv1".to_string()));
    }

    #[test]
    fn test_word_at_position_unicode_operator() {
        // Hovering on '∈' should return it as a single-char operator
        let text = "    @inv1 count ∈ ℕ";
        // '∈' is at char index 16
        let word = word_at(text, Position::new(0, 16));
        assert_eq!(word, Some("∈".to_string()));

        // 'ℕ' is at char index 18
        let word = word_at(text, Position::new(0, 18));
        assert_eq!(word, Some("ℕ".to_string()));
    }

    #[test]
    fn test_hover_multichar_operator_assignment() {
        let provider = HoverProvider::new();
        let source = "        count := count + 1";

        // `:=` spans chars 14..16; either character yields the assignment docs.
        for character in [14, 15] {
            let hover = hover_at(&provider, source, 0, character).expect("hover on `:=`");
            if let HoverContents::Markup(content) = &hover.contents {
                assert!(
                    content.value.contains("Assigns a specific value"),
                    "expected assignment docs, got: {}",
                    content.value
                );
            } else {
                panic!("Expected markup content");
            }
            assert_eq!(
                hover.range,
                Some(Range::new(Position::new(0, 14), Position::new(0, 16))),
            );
        }
    }

    #[test]
    fn test_hover_multichar_operator_unspaced() {
        let provider = HoverProvider::new();
        let source = "count:=count+1";

        // `:=` glued to the identifier (chars 5..7) — the operator must win
        // over the trailing edge of `count` (issue #34 for unspaced sources).
        for character in [5, 6] {
            let hover = hover_at(&provider, source, 0, character).expect("hover on `:=`");
            if let HoverContents::Markup(content) = &hover.contents {
                assert!(
                    content.value.contains("Assigns a specific value"),
                    "expected assignment docs, got: {}",
                    content.value
                );
            } else {
                panic!("Expected markup content");
            }
            assert_eq!(
                hover.range,
                Some(Range::new(Position::new(0, 5), Position::new(0, 7))),
            );
        }
    }

    #[test]
    fn test_hover_multichar_operator_equivalence() {
        let provider = HoverProvider::new();
        let source = "    @inv1 a <=> b";

        // `<=>` spans chars 12..15; the middle `=` must not hover as `<=`/`=>`.
        for character in [12, 13, 14] {
            let hover = hover_at(&provider, source, 0, character).expect("hover on `<=>`");
            if let HoverContents::Markup(content) = &hover.contents {
                assert!(
                    content.value.contains("if and only if"),
                    "expected equivalence docs, got: {}",
                    content.value
                );
            } else {
                panic!("Expected markup content");
            }
            assert_eq!(
                hover.range,
                Some(Range::new(Position::new(0, 12), Position::new(0, 15))),
            );
        }
    }

    #[test]
    fn test_hover_all_keywords() {
        let provider = HoverProvider::new();

        for (id, _, _) in KEYWORD_DOCS {
            for spelling in keywords::keyword(*id).spellings {
                // Hover lookup is case-insensitive.
                for variant in [spelling.to_string(), spelling.to_lowercase()] {
                    assert!(
                        provider.hover_keyword(&variant).is_some(),
                        "Missing hover for keyword: {variant}"
                    );
                }
            }
        }
    }

    #[test]
    fn test_hover_all_operators() {
        let provider = HoverProvider::new();

        // Iterate the canonical source of truth so a newly added operator without a
        // hover doc is caught here. Operators intentionally not served by
        // `hover_operator`:
        //   Naturals/Naturals1/Integers — richer set-builder content lives in
        //     `hover_builtin`; an operator hover would shadow it (dispatch order is
        //     keyword → operator → identifier → builtin).
        //   UnaryMinus — unreachable via `lookup_token`: it shares "−"/"-" with
        //     Subtract, which is declared first and always wins the lookup.
        let skip = [
            operators::OperatorId::Naturals,
            operators::OperatorId::Naturals1,
            operators::OperatorId::Integers,
            operators::OperatorId::UnaryMinus,
        ];

        for spelling in operators::OPERATOR_SPELLINGS {
            if skip.contains(&spelling.id) {
                continue;
            }
            for op in [spelling.unicode, spelling.ascii] {
                assert!(
                    provider.hover_operator(op).is_some(),
                    "Missing hover for operator {:?} ({op})",
                    spelling.id
                );
            }
        }
    }

    #[test]
    fn test_number_sets_resolve_to_builtin_not_operator() {
        let provider = HoverProvider::new();

        // ℕ/ℕ1/ℤ exist in the canonical operator table, but their hover must come
        // from `hover_builtin` (richer set-builder text). `hover_operator` must
        // return None so the builtin wins in dispatch order.
        for token in ["ℕ", "NAT", "ℕ1", "NAT1", "ℤ", "INT"] {
            assert!(
                provider.hover_operator(token).is_none(),
                "{token} must not be served as an operator hover (would shadow builtin)"
            );
        }

        let hover = provider.hover_builtin("ℕ").unwrap();
        if let HoverContents::Markup(content) = hover.contents {
            assert!(content.value.contains("Natural"));
            assert!(content.value.contains("0, 1, 2"));
        }
    }

    #[test]
    fn test_operator_title_uses_ascii_for_private_use_glyphs() {
        let provider = HoverProvider::new();

        // Overwrite's unicode spelling is a private-use code point that won't
        // render; the derived title must fall back to the ASCII spelling "<+".
        let hover = provider.hover_operator("<+").unwrap();
        if let HoverContents::Markup(content) = hover.contents {
            assert!(content.value.contains("<+ (Relational override)"));
            assert!(!content.value.contains("U+E"));
        }
    }

    #[test]
    fn test_hover_separator_operators() {
        let provider = HoverProvider::new();

        // Dot and Bar were previously undocumented; both spellings must hover.
        for token in ["·", ".", "∣", "|"] {
            assert!(
                provider.hover_operator(token).is_some(),
                "Missing hover for separator: {token}"
            );
        }
    }

    #[test]
    fn test_hover_for_theorems_section() {
        let provider = HoverProvider::new();
        assert!(provider.hover_keyword("THEOREMS").is_some());
        // Lookup is case-insensitive.
        assert!(provider.hover_keyword("theorems").is_some());
    }

    #[test]
    fn test_expression_mentions_id() {
        use rossi::ast::expression::BinaryOp;

        // Simple identifier
        assert!(expression_mentions_id(
            &Expression::Identifier("x".into()),
            "x"
        ));
        assert!(!expression_mentions_id(
            &Expression::Identifier("y".into()),
            "x"
        ));

        // Integer literal
        assert!(!expression_mentions_id(&Expression::Integer(42), "x"));

        // Binary expression
        let bin = Expression::binary(
            BinaryOp::Add,
            Expression::Identifier("x".into()),
            Expression::Integer(1),
        );
        assert!(expression_mentions_id(&bin, "x"));
        assert!(!expression_mentions_id(&bin, "y"));

        // Function application
        let app = Expression::FunctionApplication {
            function: Box::new(Expression::Identifier("f".into())),
            arguments: vec![Expression::Identifier("x".into())],
        };
        assert!(expression_mentions_id(&app, "f"));
        assert!(expression_mentions_id(&app, "x"));
        assert!(!expression_mentions_id(&app, "z"));
    }

    #[test]
    fn test_predicate_mentions_id() {
        use rossi::ast::predicate::ComparisonOp;

        // Comparison
        let pred = Predicate::comparison(
            ComparisonOp::In,
            Expression::Identifier("count".into()),
            Expression::Naturals,
        );
        assert!(predicate_mentions_id(&pred, "count"));
        assert!(!predicate_mentions_id(&pred, "other"));

        // Logical
        let pred2 = Predicate::comparison(
            ComparisonOp::GreaterEqual,
            Expression::Identifier("count".into()),
            Expression::Integer(0),
        );
        let conj = Predicate::logical(rossi::ast::predicate::LogicalOp::And, pred.clone(), pred2);
        assert!(predicate_mentions_id(&conj, "count"));
        assert!(!predicate_mentions_id(&conj, "x"));

        // Quantified
        let quant = Predicate::quantified(
            rossi::ast::predicate::Quantifier::ForAll,
            vec!["y".into()],
            pred,
        );
        assert!(predicate_mentions_id(&quant, "count"));

        // True/False literals
        assert!(!predicate_mentions_id(&Predicate::True, "anything"));
        assert!(!predicate_mentions_id(&Predicate::False, "anything"));
    }

    #[test]
    fn test_collect_constraints() {
        use rossi::ast::predicate::ComparisonOp;

        let predicates = vec![
            LabeledPredicate {
                label: Some("axm1".into()),
                is_theorem: false,
                predicate: Predicate::comparison(
                    ComparisonOp::In,
                    Expression::Identifier("max_value".into()),
                    Expression::Naturals,
                ),
                span: None,
                comment: None,
            },
            LabeledPredicate {
                label: Some("axm2".into()),
                is_theorem: false,
                predicate: Predicate::comparison(
                    ComparisonOp::Equal,
                    Expression::Identifier("max_value".into()),
                    Expression::Integer(100),
                ),
                span: None,
                comment: None,
            },
            LabeledPredicate {
                label: Some("axm3".into()),
                is_theorem: false,
                predicate: Predicate::comparison(
                    ComparisonOp::In,
                    Expression::Identifier("other".into()),
                    Expression::Integers,
                ),
                span: None,
                comment: None,
            },
        ];

        let constraints = collect_constraints(&predicates, "max_value");
        assert_eq!(constraints.len(), 2);
        assert!(constraints[0].starts_with("axm1:"));
        assert!(constraints[1].starts_with("axm2:"));

        // "other" only matches axm3
        let other_constraints = collect_constraints(&predicates, "other");
        assert_eq!(other_constraints.len(), 1);
        assert!(other_constraints[0].starts_with("axm3:"));

        // Unknown identifier matches none
        let empty = collect_constraints(&predicates, "unknown");
        assert!(empty.is_empty());
    }

    #[test]
    fn test_hover_identifier_with_constraints() {
        let provider = HoverProvider::new();

        let mut constraints = HashMap::new();
        constraints.insert(
            "count".to_string(),
            vec![
                "@inv1 count ∈ ℕ".to_string(),
                "@inv2 count ≤ max_value".to_string(),
            ],
        );
        constraints.insert(
            "max_value".to_string(),
            vec!["@axm1 max_value ∈ ℕ".to_string()],
        );

        let ctx = HoverContext {
            variables: vec![("count".to_string(), "counter".to_string())],
            constants: vec![("max_value".to_string(), "counter_ctx".to_string())],
            sets: vec![],
            constraints,
        };

        // Variable with invariants
        let hover = provider.hover_identifier("count", &ctx).unwrap();
        if let HoverContents::Markup(content) = hover.contents {
            assert!(content.value.contains("Variable"));
            assert!(content.value.contains("**Invariants:**"));
            assert!(content.value.contains("count ∈ ℕ"));
            assert!(content.value.contains("count ≤ max_value"));
        } else {
            panic!("Expected markup content");
        }

        // Constant with axioms
        let hover = provider.hover_identifier("max_value", &ctx).unwrap();
        if let HoverContents::Markup(content) = hover.contents {
            assert!(content.value.contains("Constant"));
            assert!(content.value.contains("**Axioms:**"));
            assert!(content.value.contains("max_value ∈ ℕ"));
        } else {
            panic!("Expected markup content");
        }
    }

    #[test]
    fn test_hover_full_model_with_constraints() {
        let provider = HoverProvider::new();

        let source = r#"
CONTEXT counter_ctx
CONSTANTS
    max_value
AXIOMS
    @axm1 max_value ∈ ℕ
    @axm2 max_value = 100
END
"#;

        let uri = "file:///counter_ctx.eventb".to_string();
        provider.update_component(uri.clone(), source);

        // Hover on max_value should show axiom constraints
        let hover = provider.hover(
            &HoverParams {
                text_document_position_params: crate::lsp_types::TextDocumentPositionParams {
                    text_document: crate::lsp_types::TextDocumentIdentifier {
                        uri: Url::parse(&uri).unwrap(),
                    },
                    position: Position::new(3, 4), // "max_value" line
                },
                work_done_progress_params: Default::default(),
            },
            source,
        );

        assert!(hover.is_some());
        let hover = hover.unwrap();
        if let HoverContents::Markup(content) = hover.contents {
            assert!(
                content.value.contains("**Axioms:**"),
                "Expected axioms section, got: {}",
                content.value
            );
            assert!(
                content.value.contains("max_value"),
                "Expected max_value in constraints, got: {}",
                content.value
            );
        } else {
            panic!("Expected markup content");
        }
    }

    #[test]
    fn test_hover_refined_variable() {
        let abstract_source = r#"
MACHINE abstract_mch
VARIABLES
    abstract_state
INVARIANTS
    @inv1 abstract_state ∈ ℕ
EVENTS
    EVENT INITIALISATION
    THEN
        abstract_state := 0
    END
END
"#;
        let concrete_source = r#"
MACHINE concrete_mch
REFINES
    abstract_mch
VARIABLES
    concrete_state
INVARIANTS
    @inv1 abstract_state = concrete_state
EVENTS
    EVENT INITIALISATION
    THEN
        concrete_state := 0
    END
END
"#;

        let crm = Arc::new(CrossReferenceManager::new());
        let dm = Arc::new(DocumentManager::new());

        crm.update_component("file:///abstract_mch.eventb".to_string(), abstract_source);
        let url = Url::parse("file:///abstract_mch.eventb").unwrap();
        dm.open(url, "rossi".to_string(), 1, abstract_source.to_string());

        crm.update_component("file:///concrete_mch.eventb".to_string(), concrete_source);
        let url = Url::parse("file:///concrete_mch.eventb").unwrap();
        dm.open(
            url.clone(),
            "rossi".to_string(),
            1,
            concrete_source.to_string(),
        );

        let mut provider = HoverProvider::new();
        provider.set_cross_reference_manager(Arc::clone(&crm));
        provider.set_document_manager(Arc::clone(&dm));
        provider.update_component("file:///concrete_mch.eventb".to_string(), concrete_source);

        // Hover on abstract_state in the invariant line (line 7 of raw string)
        // "    @inv1 abstract_state = concrete_state"
        // abstract_state starts at char 10
        let hover = provider.hover(
            &HoverParams {
                text_document_position_params: crate::lsp_types::TextDocumentPositionParams {
                    text_document: crate::lsp_types::TextDocumentIdentifier { uri: url },
                    position: Position::new(7, 10),
                },
                work_done_progress_params: Default::default(),
            },
            concrete_source,
        );

        assert!(hover.is_some(), "Should get hover for abstract_state");
        let hover = hover.unwrap();
        if let HoverContents::Markup(content) = hover.contents {
            assert!(
                content.value.contains("Variable"),
                "abstract_state should be a variable, got: {}",
                content.value
            );
            assert!(
                content.value.contains("abstract_mch"),
                "Should show source machine name, got: {}",
                content.value
            );
        } else {
            panic!("Expected markup content");
        }
    }
}
