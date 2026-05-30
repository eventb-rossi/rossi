//! Hover documentation provider for Event-B
//!
//! Provides helpful information when hovering over:
//! - Keywords (purpose and usage)
//! - Operators (Unicode and ASCII variants with descriptions)
//! - Identifiers (variables, constants, sets, parameters)
//! - Built-in types and constants

use crate::lsp_types::{Hover, HoverContents, HoverParams, MarkupContent, MarkupKind};
use dashmap::DashMap;
use rossi::{Component, Expression, LabeledPredicate, Predicate, PrettyPrinter, parse};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::cross_references::CrossReferenceManager;
use crate::document::DocumentManager;

const TOTAL_RELATION: &str = "\u{E100}";
const SURJECTIVE_RELATION: &str = "\u{E101}";
const TOTAL_SURJECTIVE_RELATION: &str = "\u{E102}";
const RELATIONAL_OVERRIDE: &str = "\u{E103}";

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
    /// Cache of parsed components for quick hover lookup
    component_cache: Arc<DashMap<String, Component>>,
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

    /// Update the cached component for a document
    pub fn update_component(&self, uri: String, text: &str) {
        if let Ok(component) = parse(text) {
            self.component_cache.insert(uri, component);
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

        // Get hover context from cached component
        let hover_ctx = self
            .component_cache
            .get(&uri)
            .map(|entry| {
                HoverContext::from_component_with_refs(
                    &entry,
                    self.cross_ref_manager.as_deref(),
                    self.document_manager.as_deref(),
                )
            })
            .unwrap_or_else(HoverContext::new);

        // Get the word at cursor
        let word = get_word_at_position(text, position)?;

        // Try different hover providers in order
        if let Some(hover) = self.hover_keyword(&word) {
            return Some(hover);
        }

        if let Some(hover) = self.hover_operator(&word) {
            return Some(hover);
        }

        if let Some(hover) = self.hover_identifier(&word, &hover_ctx) {
            return Some(hover);
        }

        if let Some(hover) = self.hover_builtin(&word) {
            return Some(hover);
        }

        None
    }

    /// Get hover information for keywords
    fn hover_keyword(&self, word: &str) -> Option<Hover> {
        lookup_doc(KEYWORD_DOCS, word).map(|(t, d)| create_hover(t, d))
    }

    /// Get hover information for operators
    fn hover_operator(&self, word: &str) -> Option<Hover> {
        lookup_doc(OPERATOR_DOCS, word).map(|(t, d)| create_hover(t, d))
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

    let component = match parse(&text) {
        Ok(c) => c,
        Err(_) => return,
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

    let component = match parse(&text) {
        Ok(c) => c,
        Err(_) => return,
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

use crate::identifier_utils::get_word_at_position;

/// Documentation entry: `(keys, title, markdown description)`.
///
/// Multiple keys (e.g., Unicode + ASCII spellings of the same operator) share
/// one entry — hover lookup is a linear scan over the table.
type DocEntry = (&'static [&'static str], &'static str, &'static str);

fn lookup_doc(table: &[DocEntry], word: &str) -> Option<(&'static str, &'static str)> {
    table
        .iter()
        .find(|(keys, _, _)| keys.contains(&word))
        .map(|(_, title, desc)| (*title, *desc))
}

const KEYWORD_DOCS: &[DocEntry] = &[
    // Top-level
    (
        &["CONTEXT"],
        "CONTEXT",
        "Defines a context containing static properties of a model.\n\nA context can declare sets, constants, axioms, and theorems.",
    ),
    (
        &["MACHINE"],
        "MACHINE",
        "Defines a machine containing dynamic behavior.\n\nA machine can declare variables, invariants, variants, and events.",
    ),
    (
        &["END"],
        "END",
        "Marks the end of a context, machine, or event definition.",
    ),
    // Context clauses
    (
        &["EXTENDS"],
        "EXTENDS",
        "Extends another context, inheriting its sets, constants, and axioms.\n\n```eventb\nEXTENDS\n    base_context\n```",
    ),
    (
        &["SETS"],
        "SETS",
        "Declares carrier sets (enumerated or deferred).\n\n```eventb\nSETS\n    STATUS\n    COLORS\n```",
    ),
    (
        &["CONSTANTS"],
        "CONSTANTS",
        "Declares constants whose values are constrained by axioms.\n\n```eventb\nCONSTANTS\n    max_value\n    min_value\n```",
    ),
    (
        &["AXIOMS"],
        "AXIOMS",
        "Declares axioms (properties) that must hold for constants and sets.\n\n```eventb\nAXIOMS\n    @axm1 max_value > 0\n    @axm2 max_value = 100\n```",
    ),
    // Machine clauses
    (
        &["REFINES"],
        "REFINES",
        "Refines an abstract machine, adding more detail.\n\n```eventb\nREFINES\n    abstract_machine\n```",
    ),
    (
        &["SEES"],
        "SEES",
        "References contexts to use their sets and constants.\n\n```eventb\nSEES\n    context_name\n```",
    ),
    (
        &["VARIABLES"],
        "VARIABLES",
        "Declares state variables.\n\n```eventb\nVARIABLES\n    count\n    total\n```",
    ),
    (
        &["INVARIANTS"],
        "INVARIANTS",
        "Declares invariants (properties) that must always hold.\n\n```eventb\nINVARIANTS\n    @inv1 count >= 0\n    @inv2 count <= max_value\n```",
    ),
    (
        &["VARIANT"],
        "VARIANT",
        "Declares a variant expression for proving termination.\n\n```eventb\nVARIANT\n    max_value - count\n```",
    ),
    (
        &["EVENTS"],
        "EVENTS",
        "Begins the events section of a machine.\n\n```eventb\nEVENTS\n    EVENT INITIALISATION\n    ...\n    EVENT event_name\n    ...\nEND\n```",
    ),
    // Event keywords
    (
        &["EVENT"],
        "EVENT",
        "Defines an event that can change the machine state.\n\n```eventb\nEVENT increment\nWHERE\n    @grd1 count < max_value\nTHEN\n    @act1 count := count + 1\nEND\n```",
    ),
    (
        &["INITIALISATION"],
        "INITIALISATION",
        "Special event that initializes machine variables.\n\n```eventb\nEVENT INITIALISATION\nTHEN\n    count := 0\nEND\n```",
    ),
    (
        &["STATUS"],
        "STATUS",
        "Specifies the convergence status of an event.\n\nValues: `ordinary`, `convergent`, `anticipated`",
    ),
    (
        &["ANY"],
        "ANY",
        "Introduces event parameters (local variables).\n\n```eventb\nANY x\nWHERE\n    @grd1 x ∈ ℕ\nTHEN\n    @act1 count := x\nEND\n```",
    ),
    (
        &["WHERE", "WHEN"],
        "WHERE/WHEN",
        "Declares event guards (preconditions).\n\n```eventb\nWHERE\n    @grd1 count < max_value\n    @grd2 count >= 0\n```",
    ),
    (
        &["WITH"],
        "WITH",
        "Specifies witness predicates for refinement.\n\n```eventb\nWITH\n    @x x = count + 1\n```",
    ),
    (
        &["WITNESS"],
        "WITNESS",
        "Declares witness predicates for abstract parameters.\n\n```eventb\nWITNESS\n    @x x = count + 1\n```",
    ),
    (
        &["THEN", "BEGIN"],
        "THEN/BEGIN",
        "Declares event actions (state changes).\n\n```eventb\nTHEN\n    @act1 count := count + 1\n    @act2 total := total + count\n```",
    ),
    // Event status values
    (
        &["ordinary"],
        "ordinary",
        "Ordinary event (default). Does not affect variant.",
    ),
    (
        &["convergent"],
        "convergent",
        "Convergent event. Must decrease the variant, proving termination.",
    ),
    (
        &["anticipated"],
        "anticipated",
        "Anticipated event. May increase variant but will be refined to convergent.",
    ),
    (
        &["extended"],
        "extended",
        "Extended event. Inherits guards and actions from refined event.",
    ),
];

const OPERATOR_DOCS: &[DocEntry] = &[
    // Logical operators
    (
        &["∧", "&"],
        "∧ (Logical AND)",
        "**Logical conjunction**\n\nReturns true if both operands are true.\n\n```eventb\nP ∧ Q\nP & Q  // ASCII alternative\n```",
    ),
    (
        &["∨", "or"],
        "∨ (Logical OR)",
        "**Logical disjunction**\n\nReturns true if at least one operand is true.\n\n```eventb\nP ∨ Q\nP or Q  // ASCII alternative\n```",
    ),
    (
        &["¬", "not"],
        "¬ (Logical NOT)",
        "**Logical negation**\n\nReturns the opposite truth value.\n\n```eventb\n¬P\nnot P  // ASCII alternative\n```",
    ),
    (
        &["⇒", "=>"],
        "⇒ (Implication)",
        "**Logical implication**\n\nP ⇒ Q means \"if P then Q\".\n\n```eventb\nx > 0 ⇒ x ≠ 0\nx > 0 => x /= 0  // ASCII alternative\n```",
    ),
    (
        &["⇔", "<=>"],
        "⇔ (Equivalence)",
        "**Logical equivalence**\n\nP ⇔ Q means \"P if and only if Q\".\n\n```eventb\nx = 0 ⇔ ¬(x > 0)\nx = 0 <=> not(x > 0)  // ASCII alternative\n```",
    ),
    (
        &["∀", "!"],
        "∀ (Universal Quantifier)",
        "**Universal quantification (for all)**\n\n```eventb\n∀ x · x ∈ S ⇒ P(x)\n! x . (x : S => P(x))  // ASCII alternative\n```\n\nReads as \"for all x such that x is in S, P(x) holds\".",
    ),
    (
        &["∃", "#"],
        "∃ (Existential Quantifier)",
        "**Existential quantification (exists)**\n\n```eventb\n∃ x · x ∈ S ∧ P(x)\n# x . (x : S & P(x))  // ASCII alternative\n```\n\nReads as \"there exists an x in S such that P(x) holds\".",
    ),
    // Set operators
    (
        &["∈", ":"],
        "∈ (Set Membership)",
        "**Set membership**\n\nChecks if an element belongs to a set.\n\n```eventb\nx ∈ ℕ\nx : NAT  // ASCII alternative\n```",
    ),
    (
        &["∉", "/:"],
        "∉ (Not Member)",
        "**Not a member of set**\n\nChecks if an element does not belong to a set.\n\n```eventb\nx ∉ S\nx /: S  // ASCII alternative\n```",
    ),
    (
        &["⊆", "<:"],
        "⊆ (Subset)",
        "**Subset or equal**\n\nA ⊆ B means all elements of A are in B.\n\n```eventb\nA ⊆ B\nA <: B  // ASCII alternative\n```",
    ),
    (
        &["⊂", "<<:"],
        "⊂ (Strict Subset)",
        "**Strict subset**\n\nA ⊂ B means A ⊆ B and A ≠ B.\n\n```eventb\nA ⊂ B\nA <<: B  // ASCII alternative\n```",
    ),
    (
        &["⊈", "/<:"],
        "⊈ (Not Subset)",
        "**Not subset or equal**\n\nA ⊈ B means at least one element of A is not in B.\n\n```eventb\nA ⊈ B\nA /<: B  // ASCII alternative\n```",
    ),
    (
        &["⊄", "/<<:"],
        "⊄ (Not Strict Subset)",
        "**Not strict subset**\n\nA ⊄ B means A is not a strict subset of B.\n\n```eventb\nA ⊄ B\nA /<<: B  // ASCII alternative\n```",
    ),
    (
        &["∪", "\\/"],
        "∪ (Set Union)",
        "**Set union**\n\nA ∪ B contains all elements in A or B.\n\n```eventb\nA ∪ B\nA \\/ B  // ASCII alternative\n```",
    ),
    (
        &["∩", "/\\"],
        "∩ (Set Intersection)",
        "**Set intersection**\n\nA ∩ B contains elements in both A and B.\n\n```eventb\nA ∩ B\nA /\\ B  // ASCII alternative\n```",
    ),
    (
        &["∖", "\\"],
        "∖ (Set Difference)",
        "**Set difference**\n\nA ∖ B contains elements in A but not in B.\n\n```eventb\nA ∖ B\nA \\ B  // ASCII alternative\n```",
    ),
    (
        &["ℙ", "POW"],
        "ℙ (Power Set)",
        "**Power set**\n\nℙ(S) is the set of all subsets of S.\n\n```eventb\nℙ(S)\nPOW(S)  // ASCII alternative\n```",
    ),
    (
        &["∅", "{}"],
        "∅ (Empty Set)",
        "**Empty set**\n\nThe set containing no elements.\n\n```eventb\n∅\n{}  // ASCII alternative\n```",
    ),
    (
        &["card"],
        "card (Cardinality)",
        "**Set cardinality**\n\nReturns the number of elements in a finite set.\n\n```eventb\ncard(S)\n```",
    ),
    // Relation operators
    (
        &["↔", "<->"],
        "↔ (Relation)",
        "**Relation**\n\nA ↔ B is the set of all relations between A and B, i.e. ℙ(A × B).\n\n```eventb\nA ↔ B\nA <-> B  // ASCII alternative\n```",
    ),
    (
        &[TOTAL_RELATION, "<<->"],
        "U+E100 (Total Relation)",
        "**Total relation**\n\nA total relation relates every element of the source set.\n\n```eventb\nA \u{E100} B\nA <<-> B  // ASCII alternative\n```",
    ),
    (
        &[SURJECTIVE_RELATION, "<->>"],
        "U+E101 (Surjective Relation)",
        "**Surjective relation**\n\nA surjective relation covers every element of the target set.\n\n```eventb\nA \u{E101} B\nA <->> B  // ASCII alternative\n```",
    ),
    (
        &[TOTAL_SURJECTIVE_RELATION, "<<->>"],
        "U+E102 (Total Surjective Relation)",
        "**Total surjective relation**\n\nA relation that is both total on the source set and surjective onto the target set.\n\n```eventb\nA \u{E102} B\nA <<->> B  // ASCII alternative\n```",
    ),
    (
        &["→", "-->"],
        "→ (Total Function)",
        "**Total function**\n\nA → B is a function defined for all elements of A.\n\n```eventb\nf ∈ A → B\nf : A --> B  // ASCII alternative\n```",
    ),
    (
        &["⇸", "+->"],
        "⇸ (Partial Function)",
        "**Partial function**\n\nA ⇸ B is a function that may not be defined for all elements of A.\n\n```eventb\nf ∈ A ⇸ B\nf : A +-> B  // ASCII alternative\n```",
    ),
    (
        &["↦", "|->"],
        "↦ (Maplet)",
        "**Maplet (ordered pair)**\n\nCreates an ordered pair for relations.\n\n```eventb\nx ↦ y\nx |-> y  // ASCII alternative\n```",
    ),
    (
        &["∘", "circ"],
        "∘ (Backward Composition)",
        "**Backward composition**\n\nf ∘ g applies g first, then f.\n\n```eventb\nf ∘ g\nf circ g  // ASCII alternative\n```",
    ),
    (
        &[";"],
        "; (Forward Composition)",
        "**Forward composition**\n\nf ; g applies f first, then g.\n\n```eventb\nf ; g\n```",
    ),
    (
        &["×", "**"],
        "× (Cartesian Product)",
        "**Cartesian product**\n\nA × B is the set of all pairs (a, b).\n\n```eventb\nA × B\nA ** B  // ASCII alternative\n```",
    ),
    (
        &["↣", ">->"],
        "↣ (Total Injection)",
        "**Total injection**\n\nA ↣ B is a total function that is injective.\n\n```eventb\nf ∈ A ↣ B\nf : A >-> B  // ASCII alternative\n```",
    ),
    (
        &["⤔", ">+>"],
        "⤔ (Partial Injection)",
        "**Partial injection**\n\nA ⤔ B is a partial function that is injective.\n\n```eventb\nf ∈ A ⤔ B\nf : A >+> B  // ASCII alternative\n```",
    ),
    (
        &["↠", "->>"],
        "↠ (Total Surjection)",
        "**Total surjection**\n\nA ↠ B is a total function that is surjective.\n\n```eventb\nf ∈ A ↠ B\nf : A ->> B  // ASCII alternative\n```",
    ),
    (
        &["⤀", "+>>"],
        "⤀ (Partial Surjection)",
        "**Partial surjection**\n\nA ⤀ B is a partial function that is surjective.\n\n```eventb\nf ∈ A ⤀ B\nf : A +>> B  // ASCII alternative\n```",
    ),
    (
        &["⤖", ">->>"],
        "⤖ (Bijection)",
        "**Bijection (total bijective function)**\n\nA ⤖ B is both injective and surjective.\n\n```eventb\nf ∈ A ⤖ B\nf : A >->> B  // ASCII alternative\n```",
    ),
    (
        &["dom"],
        "dom (Domain)",
        "**Domain of a relation**\n\nReturns the set of first elements.\n\n```eventb\ndom(r)\n```",
    ),
    (
        &["ran"],
        "ran (Range)",
        "**Range of a relation**\n\nReturns the set of second elements.\n\n```eventb\nran(r)\n```",
    ),
    (
        &["◁", "<|"],
        "◁ (Domain Restriction)",
        "**Domain restriction**\n\nS ◁ r restricts r to pairs whose first element is in S.\n\n```eventb\nS ◁ r\nS <| r  // ASCII alternative\n```",
    ),
    (
        &["⩤", "<<|"],
        "⩤ (Domain Subtraction)",
        "**Domain subtraction**\n\nS ⩤ r removes pairs whose first element is in S.\n\n```eventb\nS ⩤ r\nS <<| r  // ASCII alternative\n```",
    ),
    (
        &["▷", "|>"],
        "▷ (Range Restriction)",
        "**Range restriction**\n\nr ▷ S restricts r to pairs whose second element is in S.\n\n```eventb\nr ▷ S\nr |> S  // ASCII alternative\n```",
    ),
    (
        &["⩥", "|>>"],
        "⩥ (Range Subtraction)",
        "**Range subtraction**\n\nr ⩥ S removes pairs whose second element is in S.\n\n```eventb\nr ⩥ S\nr |>> S  // ASCII alternative\n```",
    ),
    (
        &[RELATIONAL_OVERRIDE, "<+"],
        "U+E103 (Relational Override)",
        "**Relational override**\n\nr \u{E103} s overrides r with s where they overlap.\n\n```eventb\nr \u{E103} s\nr <+ s  // ASCII alternative\n```",
    ),
    (
        &["⊗", "><"],
        "⊗ (Direct Product)",
        "**Direct product**\n\nCombines two relations into pairs of their images.\n\n```eventb\nr ⊗ s\nr >< s  // ASCII alternative\n```",
    ),
    (
        &["∥", "||"],
        "∥ (Parallel Product)",
        "**Parallel product**\n\nApplies two relations in parallel on pairs.\n\n```eventb\nr ∥ s\nr || s  // ASCII alternative\n```",
    ),
    (
        &["∼", "~"],
        "∼ (Inverse)",
        "**Relational inverse**\n\nr∼ reverses all pairs in the relation.\n\n```eventb\nr∼\nr~  // ASCII alternative\n```",
    ),
    (
        &["⦂", "oftype"],
        "⦂ (Type Constraint)",
        "**Type constraint (oftype)**\n\nAnnotates an expression with its type.\n\n```eventb\nE ⦂ T\nE oftype T  // ASCII alternative\n```",
    ),
    (
        &[".."],
        ".. (Integer Range)",
        "**Integer range**\n\na..b is the set of integers from a to b inclusive.\n\n```eventb\n1..10\n```",
    ),
    (
        &["λ", "%"],
        "λ (Lambda)",
        "**Lambda abstraction**\n\nDefines a function by an expression.\n\n```eventb\nλ x · x ∈ S ∣ E\n% x . (x : S | E)  // ASCII alternative\n```",
    ),
    (
        &["⋃", "UNION"],
        "⋃ (Generalized Union)",
        "**Generalized union**\n\n⋃ x · P ∣ E takes the union over all values satisfying P.\n\n```eventb\n⋃ x · x ∈ S ∣ f(x)\nUNION x . (x : S | f(x))  // ASCII alternative\n```",
    ),
    (
        &["⋂", "INTER"],
        "⋂ (Generalized Intersection)",
        "**Generalized intersection**\n\n⋂ x · P ∣ E takes the intersection over all values satisfying P.\n\n```eventb\n⋂ x · x ∈ S ∣ f(x)\nINTER x . (x : S | f(x))  // ASCII alternative\n```",
    ),
    (
        &["ℙ1", "POW1"],
        "ℙ1 (Non-empty Power Set)",
        "**Non-empty power set**\n\nℙ1(S) is the set of all non-empty subsets of S.\n\n```eventb\nℙ1(S)\nPOW1(S)  // ASCII alternative\n```",
    ),
    // Assignment operators
    (
        &[":="],
        "Deterministic Assignment",
        "**Deterministic assignment**\n\nAssigns a specific value to a variable.\n\n```eventb\ncount := count + 1\n```",
    ),
    (
        &[":|"],
        "Non-deterministic Assignment",
        "**Non-deterministic assignment (such that)**\n\nAssigns any value satisfying a predicate.\n\n```eventb\ncount :| count' ∈ ℕ ∧ count' > 0\n```",
    ),
    (
        &[":∈", "::"],
        "Non-deterministic Member Assignment",
        "**Non-deterministic assignment (member of)**\n\nAssigns any value from a set.\n\n```eventb\ncount :∈ ℕ\ncount :: NAT  // ASCII alternative\n```",
    ),
    // Comparison operators
    (
        &["="],
        "Equality",
        "**Equality**\n\nChecks if two values are equal.\n\n```eventb\nx = 5\n```",
    ),
    (
        &["≠", "/="],
        "Not Equal",
        "**Not equal**\n\nChecks if two values are different.\n\n```eventb\nx ≠ 0\nx /= 0  // ASCII alternative\n```",
    ),
    (
        &["<"],
        "Less Than",
        "**Less than**\n\n```eventb\nx < 10\n```",
    ),
    (
        &[">"],
        "Greater Than",
        "**Greater than**\n\n```eventb\nx > 0\n```",
    ),
    (
        &["≤", "<="],
        "Less Than or Equal",
        "**Less than or equal**\n\n```eventb\nx ≤ 100\nx <= 100  // ASCII alternative\n```",
    ),
    (
        &["≥", ">="],
        "Greater Than or Equal",
        "**Greater than or equal**\n\n```eventb\nx ≥ 0\nx >= 0  // ASCII alternative\n```",
    ),
    // Arithmetic operators
    (&["+"], "Addition", "**Addition**\n\n```eventb\nx + y\n```"),
    (
        &["−", "-"],
        "Subtraction",
        "**Subtraction**\n\n```eventb\nx − y\nx - y\n```",
    ),
    (
        &["÷", "/"],
        "Division",
        "**Division**\n\n```eventb\nx ÷ y\nx / y  // ASCII alternative\n```",
    ),
    (
        &["mod"],
        "Modulo",
        "**Modulo (remainder)**\n\nReturns the remainder after division.\n\n```eventb\nx mod y\n```",
    ),
    (
        &["^"],
        "Exponentiation",
        "**Exponentiation (power)**\n\n```eventb\nx ^ n\n```",
    ),
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
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp_types::{Position, Url};

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
            assert!(content.value.contains("Logical"));
            assert!(content.value.contains("AND"));
        }
    }

    #[test]
    fn test_hover_operator_ascii() {
        let provider = HoverProvider::new();

        let hover = provider.hover_operator("&");
        assert!(hover.is_some());
        let hover = hover.unwrap();
        if let HoverContents::Markup(content) = hover.contents {
            assert!(content.value.contains("Logical"));
            assert!(content.value.contains("AND"));
        }

        // /\ is now set intersection only (no longer logical AND)
        let hover = provider.hover_operator("/\\");
        assert!(hover.is_some());
        let hover = hover.unwrap();
        if let HoverContents::Markup(content) = hover.contents {
            assert!(content.value.contains("Set"));
            assert!(content.value.contains("Intersection"));
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
    fn test_get_word_at_position() {
        let text = "CONTEXT test_context";

        // Position at 'C' (start of CONTEXT)
        let word = get_word_at_position(text, Position::new(0, 0));
        assert_eq!(word, Some("CONTEXT".to_string()));

        // Position at 't' (in test_context)
        let word = get_word_at_position(text, Position::new(0, 8));
        assert_eq!(word, Some("test_context".to_string()));
    }

    #[test]
    fn test_get_word_at_position_unicode() {
        // Line with Unicode operators — previously panicked due to byte slicing
        let text = "    @inv1 count ∈ ℕ";

        // Hovering on 'count' (char index 10)
        let word = get_word_at_position(text, Position::new(0, 10));
        assert_eq!(word, Some("count".to_string()));

        // Hovering on 'inv1' (char index 5)
        let word = get_word_at_position(text, Position::new(0, 5));
        assert_eq!(word, Some("inv1".to_string()));
    }

    #[test]
    fn test_get_word_at_position_unicode_operator() {
        // Hovering on '∈' should return it as single char (operator fallback)
        let text = "    @inv1 count ∈ ℕ";
        // '∈' is at char index 16
        let word = get_word_at_position(text, Position::new(0, 16));
        assert_eq!(word, Some("∈".to_string()));

        // 'ℕ' is at char index 18
        let word = get_word_at_position(text, Position::new(0, 18));
        assert_eq!(word, Some("ℕ".to_string()));
    }

    #[test]
    fn test_hover_all_keywords() {
        let provider = HoverProvider::new();

        let keywords = vec![
            "CONTEXT",
            "MACHINE",
            "END",
            "EXTENDS",
            "SETS",
            "CONSTANTS",
            "AXIOMS",
            "REFINES",
            "SEES",
            "VARIABLES",
            "INVARIANTS",
            "VARIANT",
            "EVENTS",
            "EVENT",
            "INITIALISATION",
            "STATUS",
            "ANY",
            "WHERE",
            "THEN",
            "ordinary",
            "convergent",
            "anticipated",
        ];

        for keyword in keywords {
            let hover = provider.hover_keyword(keyword);
            assert!(hover.is_some(), "Missing hover for keyword: {}", keyword);
        }
    }

    #[test]
    fn test_hover_all_operators() {
        let provider = HoverProvider::new();

        let operators = vec![
            // Logical
            "∧",
            "&",
            "∨",
            "or",
            "¬",
            "not",
            "⇒",
            "=>", // Set
            "∈",
            ":",
            "⊆",
            "<:",
            "⊈",
            "/<:",
            "⊂",
            "<<:",
            "⊄",
            "/<<:",
            "∪",
            "\\/",
            "∩",
            "/\\", // Function types
            "↔",
            "<->",
            TOTAL_RELATION,
            "<<->",
            SURJECTIVE_RELATION,
            "<->>",
            TOTAL_SURJECTIVE_RELATION,
            "<<->>",
            "→",
            "-->",
            "⇸",
            "+->",
            "↣",
            ">->",
            "⤔",
            ">+>",
            "↠",
            "->>",
            "⤀",
            "+>>",
            "⤖",
            ">->>",
            // Relation operators
            "↦",
            "|->",
            "◁",
            "<|",
            "⩤",
            "<<|",
            "▷",
            "|>",
            "⩥",
            "|>>",
            "∘",
            "circ",
            ";",
            RELATIONAL_OVERRIDE,
            "<+",
            "⊗",
            "><",
            "∥",
            "||",
            "×",
            "**",
            "∼",
            "~",
            "⦂",
            "oftype", // Misc
            "..",
            "λ",
            "%",
            "⋃",
            "UNION",
            "⋂",
            "INTER",
            "ℙ",
            "POW",
            "ℙ1",
            "POW1", // Assignment
            ":=",
        ];

        for op in operators {
            let hover = provider.hover_operator(op);
            assert!(hover.is_some(), "Missing hover for operator: {}", op);
        }
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
