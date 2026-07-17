//! Hover documentation provider for Event-B
//!
//! Provides helpful information when hovering over:
//! - Keywords (purpose and usage)
//! - Operators (Unicode and ASCII variants with descriptions)
//! - Identifiers (variables, constants, sets, parameters)
//! - Built-in types and constants

use crate::lsp_types::{Hover, HoverContents, HoverParams, MarkupContent, MarkupKind, Position};
use rossi::{
    Component, ComponentNameSite, Event, Expression, LabeledPredicate, PrettyPrinter,
    deps::{ComponentKind, EdgeKind},
    keywords::{self, KeywordId},
    operators::{self, OperatorId},
};
use std::collections::HashMap;
use std::sync::Arc;

use crate::component_loader::ComponentLoader;
use crate::component_util::{
    ComponentIdentity, component_at_offset, parse_all, resolve_component_at_position,
};
use crate::cross_references::CrossReferenceManager;
use crate::document::DocumentManager;
use crate::formula_walk;
use crate::identifier_utils::position_to_offset;
use crate::resolved_environment::ResolvedEnvironment;
use crate::text_utils;

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
    constraints: HashMap<String, HashMap<String, Vec<String>>>,
    /// Variant expressions in scope — the cursor machine's and every one down its
    /// REFINES chain. Checked against the hovered identifier to surface the
    /// variant that constrains it.
    variants: Vec<Expression>,
}

impl HoverContext {
    fn new() -> Self {
        Self {
            variables: Vec::new(),
            constants: Vec::new(),
            sets: Vec::new(),
            constraints: HashMap::new(),
            variants: Vec::new(),
        }
    }

    fn from_component_with_refs(component: &Component, loader: Option<&ComponentLoader>) -> Self {
        let mut ctx = Self::new();
        ctx.add_component(component);

        if let Some(loader) = loader {
            let environment = ResolvedEnvironment::new(component, loader);
            match component {
                Component::Context(_) => {
                    for inherited in environment.extended_contexts() {
                        ctx.add_component(inherited);
                    }
                }
                Component::Machine(_) => {
                    for inherited in environment.refined_machines() {
                        ctx.add_component(inherited);
                    }
                    for visible in environment.visible_contexts() {
                        ctx.add_component(visible);
                    }
                }
            }
        }

        ctx
    }

    fn add_component(&mut self, component: &Component) {
        match component {
            Component::Context(context) => {
                for constant in &context.constants {
                    self.constants
                        .push((constant.name.clone(), context.name.clone()));
                    let constraints = collect_constraints(&context.axioms, &constant.name);
                    if !constraints.is_empty() {
                        self.constraints
                            .entry(context.name.clone())
                            .or_default()
                            .insert(constant.name.clone(), constraints);
                    }
                }
                for set in &context.sets {
                    self.sets
                        .push((set.name().to_string(), context.name.clone()));
                    let constraints = collect_constraints(&context.axioms, set.name());
                    if !constraints.is_empty() {
                        self.constraints
                            .entry(context.name.clone())
                            .or_default()
                            .insert(set.name().to_string(), constraints);
                    }
                }
            }
            Component::Machine(machine) => {
                for variable in &machine.variables {
                    self.variables
                        .push((variable.name.clone(), machine.name.clone()));
                    let constraints = collect_constraints(&machine.invariants, &variable.name);
                    if !constraints.is_empty() {
                        self.constraints
                            .entry(machine.name.clone())
                            .or_default()
                            .insert(variable.name.clone(), constraints);
                    }
                }
                if let Some(variant) = &machine.variant {
                    self.variants.push(variant.clone());
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn benchmark_environment_construction(
    component: &Component,
    loader: &ComponentLoader<'_>,
) -> usize {
    let context = HoverContext::from_component_with_refs(component, Some(loader));
    std::hint::black_box(&context);
    context.variables.len()
        + context.constants.len()
        + context.sets.len()
        + context.constraints.len()
        + context.variants.len()
}

/// Provides hover documentation for Event-B documents
pub struct HoverProvider {
    /// Cross-reference manager for workspace-wide navigation
    cross_ref_manager: Option<Arc<CrossReferenceManager>>,
    /// Document manager — the source of the document's shared recovered parse
    document_manager: Option<Arc<DocumentManager>>,
}

impl HoverProvider {
    pub fn new() -> Self {
        Self {
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

    /// Generate hover information for the given position
    pub fn hover(&self, params: &HoverParams, text: &str) -> Option<Hover> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        // Prefer the shared parsed snapshot so token lookup, structural
        // resolution, and AST spans all index the same source revision.
        let parsed = self
            .document_manager
            .as_ref()
            .and_then(|dm| dm.parse_result(uri));
        let text = parsed.as_deref().map_or(text, |parsed| parsed.text());

        // No hover inside comments: `:=` or a keyword in prose is not the
        // operator/keyword it spells.
        if let Some(offset) = position_to_offset(text, position)
            && rossi::comments::offset_in_comment(text, offset)
        {
            return None;
        }

        // Get the token at cursor — an identifier or a whole (possibly
        // multi-character) operator like `:=`.
        let (word, range) = word_at_position(text, position)?;
        let masked = rossi::comments::mask_comments_chars(text);
        if let Some(component) = resolve_component_at_position(text, &masked, position, &word) {
            let mut hover = hover_component(&component);
            hover.range = Some(range);
            return Some(hover);
        }

        // Static documentation needs no AST or dependency environment. Keep it
        // ahead of fallback parsing for bare-provider and closed-document hovers.
        if let Some(mut hover) = self
            .hover_keyword(&word)
            .or_else(|| self.hover_operator(&word))
            .or_else(|| self.hover_builtin(&word))
        {
            hover.range = Some(range);
            return Some(hover);
        }

        // Select the cursor's component against the stored parse's own text, or
        // recover from the same served text when no document snapshot exists.
        let owned;
        let components: &[Component] = match parsed.as_deref() {
            Some(parsed) => parsed.components(),
            None => {
                owned = parse_all(text);
                &owned
            }
        };
        let offset = position_to_offset(text, position).unwrap_or(text.len());
        let cursor_component = component_at_offset(components, offset);

        // One loader per request: each visible context/machine in the SEES /
        // EXTENDS / REFINES walk is parsed at most once, reusing open documents'
        // stored parses. Component and static hovers returned before this work.
        let loader = ComponentLoader::optional(
            self.cross_ref_manager.as_deref(),
            self.document_manager.as_deref(),
        );
        let hover_ctx = cursor_component
            .map(|component| HoverContext::from_component_with_refs(component, loader.as_ref()))
            .unwrap_or_else(HoverContext::new);

        // Try different hover providers in order, most-local scope first. A
        // formula binder the cursor sits on or is bound by (a quantifier / lambda
        // / comprehension binder) shadows everything wider, so it is tried before
        // the event parameter and the component-wide identifier. An event
        // ANY-clause parameter is scoped to its event and shadows a same-named
        // global, resolved positionally next. Both share the resolver
        // find-references uses, so hover cannot drift from navigation.
        let mut hover = hover_bound(&word, cursor_component, text, position)
            .or_else(|| hover_parameter(&word, cursor_component, text, position))
            .or_else(|| self.hover_identifier(&word, &hover_ctx))?;

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
            if let Some(constraints) = ctx
                .constraints
                .get(source)
                .and_then(|by_name| by_name.get(word))
            {
                append_constraint_section(&mut description, "Invariants", constraints);
            }
            append_variant_section(&mut description, &ctx.variants, word);
            return Some(create_hover(&format!("Variable: {}", word), &description));
        }

        // Check if it's a constant
        if let Some((_, source)) = ctx.constants.iter().find(|(name, _)| name == word) {
            let mut description = format!(
                "**Constant** from context `{}`\n\nConstant value constrained by axioms.",
                source
            );
            if let Some(constraints) = ctx
                .constraints
                .get(source)
                .and_then(|by_name| by_name.get(word))
            {
                append_constraint_section(&mut description, "Axioms", constraints);
            }
            append_variant_section(&mut description, &ctx.variants, word);
            return Some(create_hover(&format!("Constant: {}", word), &description));
        }

        // Check if it's a set
        if let Some((_, source)) = ctx.sets.iter().find(|(name, _)| name == word) {
            let mut description = format!(
                "**Set** from context `{}`\n\nCarrier set used for typing.",
                source
            );
            if let Some(constraints) = ctx
                .constraints
                .get(source)
                .and_then(|by_name| by_name.get(word))
            {
                append_constraint_section(&mut description, "Properties", constraints);
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

// AST traversal helpers

/// Collect formatted constraint strings from labeled predicates that mention `id`.
/// Capped at 5 results to avoid clutter. Identifier matching is delegated to the
/// shared walker (`formula_walk`) that find-references uses, so the clauses shown
/// here cannot drift from where the symbol is actually used.
fn collect_constraints(predicates: &[LabeledPredicate], id: &str) -> Vec<String> {
    let printer = PrettyPrinter::new();
    predicates
        .iter()
        .filter(|lp| formula_walk::predicate_mentions(&lp.predicate, id))
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

/// The distinct rendered variant expressions that mention `id`, in chain order.
/// Identifier matching reuses the shared walker, mirroring `collect_constraints`.
/// Distinct renderings only — an abstract and a concrete machine repeating the
/// same variant collapse to one line.
fn variants_mentioning(variants: &[Expression], id: &str) -> Vec<String> {
    if variants.is_empty() {
        return Vec::new();
    }
    let printer = PrettyPrinter::new();
    let mut out: Vec<String> = Vec::new();
    for variant in variants {
        if formula_walk::expression_mentions(variant, id) {
            let text = printer.print_expression(variant);
            if !out.contains(&text) {
                out.push(text);
            }
        }
    }
    out
}

/// Append the `Variant` section listing the in-scope variants that mention `id`.
/// Shared by the variable and constant hovers so the header and lookup live once.
fn append_variant_section(description: &mut String, variants: &[Expression], id: &str) {
    append_constraint_section(description, "Variant", &variants_mentioning(variants, id));
}

/// Append a markdown bullet list of `constraints` under a bold `**{header}:**`
/// heading to `description`, or nothing when `constraints` is empty. Shared by
/// the variable/constant/set hovers (their invariants/axioms/properties) and
/// the parameter hover (its guards) so the list formatting lives in one place.
fn append_constraint_section(description: &mut String, header: &str, constraints: &[String]) {
    if constraints.is_empty() {
        return;
    }
    description.push_str(&format!("\n\n**{header}:**\n"));
    for constraint in constraints {
        description.push_str(&format!("- `{constraint}`\n"));
    }
}

/// Hover for a formula binder under the cursor — a quantifier (`∀`/`∃`), `λ`,
/// set comprehension, or quantified `⋃`/`⋂` bound variable, whether the cursor
/// is on the binder declaration or a use it binds. `component` and `text` are
/// selected from one source snapshot by `hover`. Returns `None` when the cursor
/// is not on (or bound by) a formula binder; an event `ANY` parameter is left to
/// the richer [`hover_parameter`] card.
fn hover_bound(
    word: &str,
    component: Option<&Component>,
    text: &str,
    position: Position,
) -> Option<Hover> {
    let offset = position_to_offset(text, position)?;
    let bound = formula_walk::resolve_bound_at_offset(component?, word, offset)?;
    if bound.is_event_parameter {
        return None;
    }
    Some(create_hover(
        &format!("Bound variable: {word}"),
        "**Bound variable**\n\nLocal variable bound by a quantifier (`∀`/`∃`), `λ`, \
         set comprehension, or quantified `⋃`/`⋂` — in scope only within its binder.",
    ))
}

/// Hover for an event `ANY`-clause parameter under the cursor. `component` and
/// `text` are selected from one source snapshot by `hover`. Returns `None` when
/// the cursor is not inside a machine event, or `word` is not one of that
/// event's parameters.
fn hover_parameter(
    word: &str,
    component: Option<&Component>,
    text: &str,
    position: Position,
) -> Option<Hover> {
    let Component::Machine(machine) = component? else {
        return None;
    };
    let masked = rossi::comments::mask_comments_chars(text);
    text_utils::event_parameter_at_position(machine, &masked, position, word)
        .map(|event| build_param_hover(word, &machine.name, event))
}

/// Hover for an event `ANY`-clause parameter: the event and machine it belongs
/// to, plus the guards that mention it (mirroring the invariants/axioms shown
/// for variables and constants). `with` / witness predicates are intentionally
/// excluded — those witness *abstract* symbols during refinement and would
/// mislabel the parameter.
fn build_param_hover(word: &str, machine_name: &str, event: &Event) -> Hover {
    let mut description = format!(
        "**Parameter** of event `{}` in machine `{}`\n\nLocal variable bound by the event's `ANY` clause.",
        event.name, machine_name
    );
    append_constraint_section(
        &mut description,
        "Guards",
        &collect_constraints(&event.guards, word),
    );
    create_hover(&format!("Parameter: {}", word), &description)
}

// Helper functions

fn hover_component(component: &ComponentIdentity) -> Hover {
    let kind = match component.kind() {
        ComponentKind::Context => "Context",
        ComponentKind::Machine => "Machine",
    };
    let description = match component.site {
        ComponentNameSite::Declaration(_) => {
            format!("**{kind} component**\n\nComponent declaration.")
        }
        ComponentNameSite::Dependency(edge) => {
            let clause = match edge {
                EdgeKind::Extends => "EXTENDS",
                EdgeKind::Sees => "SEES",
                EdgeKind::Refines => "REFINES",
            };
            format!("**{kind} component**\n\n`{clause}` target.")
        }
    };
    create_hover(&format!("{kind}: {}", component.name), &description)
}

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

/// Glyph to show in hover titles — the same spelling rossi emits everywhere
/// else (see [`operators::OperatorSpelling::emit_text`]), so operators whose only
/// Unicode form is a private-use-area glyph fall back to ASCII rather than tofu.
fn display_glyph(spelling: &operators::OperatorSpelling) -> &'static str {
    spelling.emit_text(true)
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
        "Relates every element of the source set.\n\n```eventb\nA <<-> B\n```",
    ),
    (
        OperatorId::SurjectiveRelation,
        "Covers every element of the target set.\n\n```eventb\nA <->> B\n```",
    ),
    (
        OperatorId::TotalSurjectiveRelation,
        "Both total on the source set and surjective onto the target set.\n\n```eventb\nA <<->> B\n```",
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
        "r <+ s overrides r with s where they overlap.\n\n```eventb\nr <+ s\n```",
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
    use rossi::{ExpressionKind, Predicate};

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
        assert!(provider.document_manager.is_none());
    }

    #[test]
    fn test_no_hover_inside_comment() {
        let provider = HoverProvider::new();
        let source = "MACHINE m // the := and MACHINE here are prose\nEND\n";

        // Cursor on `:=` inside the comment (col 18) and on `MACHINE` in the
        // comment (col 24): no hover. The real MACHINE keyword still hovers.
        assert!(hover_at(&provider, source, 0, 18).is_none());
        assert!(hover_at(&provider, source, 0, 24).is_none());
        assert!(hover_at(&provider, source, 0, 2).is_some());
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
            variants: Vec::new(),
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
    fn test_hover_reads_components_from_document_manager() {
        // With the document open in the manager, hover resolves an identifier
        // from the shared parse — no provider-local cache.
        let uri = "file:///test.eventb";
        let source = "CONTEXT test\nCONSTANTS\n    max_value\nAXIOMS\n    @axm1 max_value ∈ ℕ\nEND";
        let dm = Arc::new(DocumentManager::new());
        dm.open(Url::parse(uri).unwrap(), 1, source.to_string());
        let mut provider = HoverProvider::new();
        provider.set_document_manager(Arc::clone(&dm));

        let hover = provider
            .hover(
                &HoverParams {
                    text_document_position_params: crate::lsp_types::TextDocumentPositionParams {
                        text_document: crate::lsp_types::TextDocumentIdentifier {
                            uri: Url::parse(uri).unwrap(),
                        },
                        position: Position::new(2, 4), // `max_value`
                    },
                    work_done_progress_params: Default::default(),
                },
                source,
            )
            .expect("hover on max_value resolves via the shared parse");
        let HoverContents::Markup(content) = hover.contents else {
            panic!("expected markup content");
        };
        assert!(content.value.contains("Constant"));
    }

    #[test]
    fn component_sites_hover_as_components_despite_formula_collisions() {
        let source = "CONTEXT C\nCONSTANTS\n    C\nEND\n\nMACHINE M\nSEES C\nVARIABLES\n    C\nEND";

        let declaration =
            markup(hover_with_doc(source, 0, 8).expect("hover on component declaration"));
        assert!(declaration.contains("# Context: C"), "got: {declaration}");
        assert!(
            declaration.contains("Component declaration"),
            "got: {declaration}"
        );

        let dependency = markup(hover_with_doc(source, 6, 5).expect("hover on SEES target"));
        assert!(dependency.contains("# Context: C"), "got: {dependency}");
        assert!(dependency.contains("`SEES` target"), "got: {dependency}");

        let formula = markup(hover_with_doc(source, 8, 4).expect("hover on variable"));
        assert!(formula.contains("# Variable: C"), "got: {formula}");
    }

    #[test]
    fn keyword_spelled_dependency_hovers_as_a_component() {
        let source = "CONTEXT MACHINE\nEND\n\nMACHINE M\nSEES MACHINE\nEND";
        let dependency = markup(
            hover_with_doc(source, 4, 5).expect("hover on keyword-spelled component target"),
        );

        assert!(
            dependency.contains("# Context: MACHINE"),
            "got: {dependency}"
        );
        assert!(dependency.contains("`SEES` target"), "got: {dependency}");
    }

    // A machine where the event parameter `p` shadows a same-named state
    // variable, `q` is a plain parameter, and `v` is a global used inside the
    // event body. Event `e` spans lines 7..=16.
    const PARAM_MACHINE: &str = "MACHINE m\nVARIABLES\n    p\n    v\nINVARIANTS\n    @inv1 p ∈ ℕ\nEVENTS\n  EVENT e\n  ANY\n    q\n    p\n  WHERE\n    @grd1 q ∈ ℕ\n    @grd2 p > q\n  THEN\n    @act1 v ≔ q\n  END\nEND";

    fn hover_with_doc(source: &str, line: u32, character: u32) -> Option<Hover> {
        let uri = "file:///param.eventb";
        let dm = Arc::new(DocumentManager::new());
        dm.open(Url::parse(uri).unwrap(), 1, source.to_string());
        let mut provider = HoverProvider::new();
        provider.set_document_manager(Arc::clone(&dm));
        provider.hover(
            &HoverParams {
                text_document_position_params: crate::lsp_types::TextDocumentPositionParams {
                    text_document: crate::lsp_types::TextDocumentIdentifier {
                        uri: Url::parse(uri).unwrap(),
                    },
                    position: Position::new(line, character),
                },
                work_done_progress_params: Default::default(),
            },
            source,
        )
    }

    fn markup(hover: Hover) -> String {
        match hover.contents {
            HoverContents::Markup(content) => content.value,
            other => panic!("expected markup content, got {other:?}"),
        }
    }

    // Issue #100 — a quantifier binder shadowing a same-named machine variable.
    //   4  @inv1 x ∈ ℕ        <- a free use of the variable (col 10)
    //   5  @inv2 ∀ x · x > 0  <- `∀ x` binder (col 12), bound use (col 16)
    const SHADOWING_BINDER: &str =
        "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ∈ ℕ\n    @inv2 ∀ x · x > 0\nEND";

    #[test]
    fn hover_on_a_bound_variable_names_it_local_not_the_global() {
        // The reported bug: hovering the bound `x` in `∀ x · x > 0` describes a
        // bound variable, not the machine variable `x` it shadows.
        let value = markup(hover_with_doc(SHADOWING_BINDER, 5, 16).expect("hover on bound x"));
        assert!(value.contains("**Bound variable**"), "got: {value}");
        assert!(
            !value.contains("**Variable**"),
            "must not show the global variable card: {value}"
        );
    }

    #[test]
    fn fallback_parse_keeps_bound_variable_scope() {
        let provider = HoverProvider::new();
        let value = markup(
            hover_at(&provider, SHADOWING_BINDER, 5, 16)
                .expect("hover on bound variable without a document manager"),
        );

        assert!(value.contains("**Bound variable**"), "got: {value}");
        assert!(!value.contains("**Variable**"), "got: {value}");
    }

    #[test]
    fn hover_on_the_binder_declaration_names_it_local() {
        // A cursor on the binder `x` itself is also a bound variable.
        let value = markup(hover_with_doc(SHADOWING_BINDER, 5, 12).expect("hover on binder x"));
        assert!(value.contains("**Bound variable**"), "got: {value}");
    }

    #[test]
    fn hover_on_a_free_use_beside_a_binder_still_shows_the_variable() {
        // The free `x` in @inv1 is not bound by the @inv2 quantifier, so hover
        // still shows the machine variable card.
        let value = markup(hover_with_doc(SHADOWING_BINDER, 4, 10).expect("hover on free x"));
        assert!(value.contains("**Variable**"), "got: {value}");
        assert!(!value.contains("Bound variable"), "got: {value}");
    }

    #[test]
    fn param_hover_at_any_declaration_names_event_and_machine() {
        // `q` at its ANY declaration (line 9).
        let value = markup(hover_with_doc(PARAM_MACHINE, 9, 4).expect("hover on parameter q"));
        assert!(value.contains("Parameter"), "got: {value}");
        assert!(value.contains("`e`"), "names the event, got: {value}");
        assert!(value.contains("`m`"), "names the machine, got: {value}");
    }

    #[test]
    fn param_hover_at_guard_use_lists_guards() {
        // `q` used in @grd1 (line 12) hovers the same parameter, with the guards
        // that mention it.
        let value = markup(hover_with_doc(PARAM_MACHINE, 12, 10).expect("hover on parameter use"));
        assert!(value.contains("Parameter"), "got: {value}");
        assert!(value.contains("Guards:"), "lists guards, got: {value}");
        assert!(
            value.contains("grd1"),
            "shows the guard label, got: {value}"
        );
    }

    #[test]
    fn param_hover_shadows_same_named_global_variable() {
        // `p` is both a state variable and event `e`'s parameter. Inside the
        // event (line 13) the parameter shadows the variable.
        let value =
            markup(hover_with_doc(PARAM_MACHINE, 13, 10).expect("hover on shadowing param"));
        assert!(value.contains("Parameter"), "got: {value}");
        assert!(
            !value.contains("Variable"),
            "the parameter shadows the global variable inside its event, got: {value}"
        );
    }

    #[test]
    fn fallback_parse_keeps_parameter_scope() {
        let provider = HoverProvider::new();
        let value = markup(
            hover_at(&provider, PARAM_MACHINE, 13, 10)
                .expect("hover on parameter without a document manager"),
        );

        assert!(value.contains("Parameter"), "got: {value}");
        assert!(!value.contains("Variable"), "got: {value}");
    }

    #[test]
    fn global_variable_hovers_outside_the_event() {
        // The same name `p` in the invariant (line 5) is outside every event, so
        // it resolves to the state variable.
        let value = markup(hover_with_doc(PARAM_MACHINE, 5, 10).expect("hover on variable"));
        assert!(value.contains("Variable"), "got: {value}");
        assert!(!value.contains("Parameter"), "got: {value}");
    }

    #[test]
    fn global_variable_used_in_event_body_still_hovers_as_variable() {
        // `v` is a global variable used in the event's action (line 15); it is
        // not a parameter, so the positional parameter check falls through to
        // the global identifier hover.
        let value =
            markup(hover_with_doc(PARAM_MACHINE, 15, 10).expect("hover on global in event"));
        assert!(value.contains("Variable"), "got: {value}");
        assert!(!value.contains("Parameter"), "got: {value}");
    }

    // Issue #85 — a machine VARIANT surfaced on the hover of the identifiers it
    // constrains. The variant `flag + count` mentions variables `count` and
    // `flag`, but not `extra`.
    //   2      count       6  @inv1 count ∈ ℕ
    //   3      flag        7  VARIANT
    //   4      extra       8      flag + count
    const VARIANT_MACHINE: &str = "MACHINE m\nVARIABLES\n    count\n    flag\n    extra\nINVARIANTS\n    @inv1 count ∈ ℕ\nVARIANT\n    flag + count\nEND";

    #[test]
    fn variable_in_the_variant_shows_it() {
        // `count` (in @inv1, line 6) is part of the variant.
        let value = markup(hover_with_doc(VARIANT_MACHINE, 6, 10).expect("hover on count"));
        assert!(value.contains("Variable"), "got: {value}");
        assert!(value.contains("**Variant:**"), "got: {value}");
        // `flag` appears only in the variant, so it proves the variant text is shown.
        assert!(
            value.contains("flag"),
            "shows the variant expression: {value}"
        );
    }

    #[test]
    fn variable_not_in_the_variant_has_no_variant_section() {
        // `extra` (line 4) appears in neither an invariant nor the variant.
        let value = markup(hover_with_doc(VARIANT_MACHINE, 4, 4).expect("hover on extra"));
        assert!(value.contains("Variable"), "got: {value}");
        assert!(!value.contains("**Variant:**"), "got: {value}");
    }

    #[test]
    fn no_variant_clause_means_no_variant_section() {
        // SHADOWING_BINDER has no VARIANT; the free `x` hover stays variant-free.
        let value = markup(hover_with_doc(SHADOWING_BINDER, 4, 10).expect("hover on x"));
        assert!(value.contains("**Variable**"), "got: {value}");
        assert!(!value.contains("**Variant:**"), "got: {value}");
    }

    /// Open every `(uri, source)` in a workspace and hover `target_uri` at the
    /// position, so cross-file SEES / REFINES resolution is available.
    fn hover_in_workspace(
        docs: &[(&str, &str)],
        target_uri: &str,
        line: u32,
        character: u32,
    ) -> Option<Hover> {
        let crm = Arc::new(CrossReferenceManager::new());
        let dm = Arc::new(DocumentManager::new());
        for (uri, src) in docs {
            crm.update_component((*uri).to_string(), src);
            dm.open(Url::parse(uri).unwrap(), 1, (*src).to_string());
        }
        let mut provider = HoverProvider::new();
        provider.set_cross_reference_manager(Arc::clone(&crm));
        provider.set_document_manager(Arc::clone(&dm));
        let target = docs.iter().find(|(u, _)| *u == target_uri).unwrap().1;
        provider.hover(
            &HoverParams {
                text_document_position_params: crate::lsp_types::TextDocumentPositionParams {
                    text_document: crate::lsp_types::TextDocumentIdentifier {
                        uri: Url::parse(target_uri).unwrap(),
                    },
                    position: Position::new(line, character),
                },
                work_done_progress_params: Default::default(),
            },
            target,
        )
    }

    #[test]
    fn constant_used_in_the_variant_shows_it() {
        // `bound` is a constant from context `c`; the machine's variant
        // `bound + count` uses it, so hovering the constant shows the variant.
        let ctx = "CONTEXT c\nCONSTANTS\n    bound\nAXIOMS\n    @axm1 bound ∈ ℕ\nEND";
        let mch = "MACHINE m\nSEES\n    c\nVARIABLES\n    count\nINVARIANTS\n    @inv1 count ∈ ℕ\nVARIANT\n    bound + count\nEND";
        // `bound` in the VARIANT body (line 8, char 4).
        let value = markup(
            hover_in_workspace(
                &[("file:///c.eventb", ctx), ("file:///m.eventb", mch)],
                "file:///m.eventb",
                8,
                4,
            )
            .expect("hover on bound"),
        );
        assert!(value.contains("Constant"), "got: {value}");
        assert!(value.contains("**Variant:**"), "got: {value}");
        assert!(
            value.contains("count"),
            "shows the variant expression: {value}"
        );
    }

    #[test]
    fn hover_resolves_symbols_beyond_ten_seen_contexts() {
        let mut owned = Vec::new();
        for i in 0..=10 {
            owned.push((
                format!("file:///c{i}.eventb"),
                format!("CONTEXT c{i}\nCONSTANTS\n    k{i}\nEND"),
            ));
        }
        let machine = format!(
            "MACHINE m\nSEES\n{}\nINVARIANTS\n    @inv1 k10 = k10\nEND",
            (0..=10)
                .map(|i| format!("    c{i}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        owned.push(("file:///m.eventb".to_string(), machine));
        let docs: Vec<(&str, &str)> = owned
            .iter()
            .map(|(uri, source)| (uri.as_str(), source.as_str()))
            .collect();
        let target = docs.last().unwrap().1;
        let position =
            crate::position::offset_to_position(target, target.find("k10 =").expect("target use"));

        let value = markup(
            hover_in_workspace(&docs, "file:///m.eventb", position.line, position.character)
                .expect("hover on k10 from the eleventh seen context"),
        );
        assert!(value.contains("Constant"), "got: {value}");
        assert!(value.contains("`c10`"), "got: {value}");
    }

    #[test]
    fn inherited_variable_hover_keeps_its_own_constraints() {
        let context = "CONTEXT c\nCONSTANTS\n    x\nAXIOMS\n    @axm1 x = 1\nEND";
        let abstract_machine = "MACHINE a\nVARIABLES\n    x\nINVARIANTS\n    @inv_a x ∈ ℕ\nEND";
        let concrete_machine =
            "MACHINE m\nREFINES\n    a\nSEES\n    c\nINVARIANTS\n    @inv_m x > 0\nEND";

        let value = markup(
            hover_in_workspace(
                &[
                    ("file:///c.eventb", context),
                    ("file:///a.eventb", abstract_machine),
                    ("file:///m.eventb", concrete_machine),
                ],
                "file:///m.eventb",
                6,
                11,
            )
            .expect("hover on inherited variable x"),
        );
        assert!(value.contains("Variable"), "got: {value}");
        assert!(value.contains("`a`"), "got: {value}");
        assert!(value.contains("inv_a"), "got: {value}");
        assert!(!value.contains("axm1"), "got: {value}");
    }

    #[test]
    fn variant_from_an_abstract_machine_shows_in_the_refinement() {
        // The abstract machine declares `VARIANT shared + 1`; the concrete
        // machine refines it and has no variant of its own. Hovering `shared` in
        // the concrete machine still surfaces the inherited variant (whole-chain)
        // — asserting the rendered text, not just the section header.
        let abstract_mch = "MACHINE m0\nVARIABLES\n    shared\nINVARIANTS\n    @inv1 shared ∈ ℕ\nVARIANT\n    shared + 1\nEND";
        let concrete = "MACHINE m1\nREFINES\n    m0\nVARIABLES\n    shared\nINVARIANTS\n    @inv2 shared > 0\nEND";
        // `shared` in the concrete invariant @inv2 (line 6, char 10).
        let value = markup(
            hover_in_workspace(
                &[
                    ("file:///m0.eventb", abstract_mch),
                    ("file:///m1.eventb", concrete),
                ],
                "file:///m1.eventb",
                6,
                10,
            )
            .expect("hover on shared"),
        );
        assert!(value.contains("Variable"), "got: {value}");
        assert!(value.contains("**Variant:**"), "got: {value}");
        assert!(
            value.contains("shared + 1"),
            "shows the inherited variant text, not just the header: {value}"
        );
    }

    #[test]
    fn distinct_variants_down_the_chain_are_all_listed_in_order() {
        // Both machines declare a variant; the concrete one (`shared + 1`) and
        // the inherited one (`shared`) are both shown, cursor machine first.
        let abstract_mch = "MACHINE m0\nVARIABLES\n    shared\nINVARIANTS\n    @inv1 shared ∈ ℕ\nVARIANT\n    shared\nEND";
        let concrete = "MACHINE m1\nREFINES\n    m0\nVARIABLES\n    shared\nINVARIANTS\n    @inv2 shared > 0\nVARIANT\n    shared + 1\nEND";
        let value = markup(
            hover_in_workspace(
                &[
                    ("file:///m0.eventb", abstract_mch),
                    ("file:///m1.eventb", concrete),
                ],
                "file:///m1.eventb",
                6,
                10,
            )
            .expect("hover on shared"),
        );
        let concrete_variant = value.find("shared + 1").expect("concrete variant");
        // The closing backtick after `shared` matches only the bare inherited one.
        let inherited = value.find("- `shared`").expect("inherited variant");
        assert!(
            concrete_variant < inherited,
            "the cursor machine's variant is listed before the inherited one: {value}"
        );
    }

    #[test]
    fn an_identical_variant_repeated_down_the_chain_is_deduplicated() {
        // Abstract and concrete machines declare the SAME variant `shared`; the
        // section lists it once, not twice.
        let abstract_mch = "MACHINE m0\nVARIABLES\n    shared\nINVARIANTS\n    @inv1 shared ∈ ℕ\nVARIANT\n    shared\nEND";
        let concrete = "MACHINE m1\nREFINES\n    m0\nVARIABLES\n    shared\nINVARIANTS\n    @inv2 shared > 0\nVARIANT\n    shared\nEND";
        let value = markup(
            hover_in_workspace(
                &[
                    ("file:///m0.eventb", abstract_mch),
                    ("file:///m1.eventb", concrete),
                ],
                "file:///m1.eventb",
                6,
                10,
            )
            .expect("hover on shared"),
        );
        assert_eq!(
            value.matches("- `shared`").count(),
            1,
            "the repeated variant collapses to a single line: {value}"
        );
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

    /// The hand-written hover bodies for the private-use operators must show the
    /// ASCII spelling and never the private-use glyph — it renders as tofu without
    /// Rodin's font, the same reason `emit_text` keeps it out of the title.
    #[test]
    fn test_private_use_hover_bodies_avoid_the_glyph() {
        let provider = HoverProvider::new();
        for id in [
            OperatorId::TotalRelation,
            OperatorId::SurjectiveRelation,
            OperatorId::TotalSurjectiveRelation,
            OperatorId::Overwrite,
        ] {
            let spelling = operators::spelling(id);
            assert!(
                operators::is_private_use_glyph(spelling.unicode),
                "{id:?} is expected to be a private-use operator"
            );
            let hover = provider
                .hover_operator(spelling.ascii)
                .unwrap_or_else(|| panic!("no hover for {id:?}"));
            let HoverContents::Markup(content) = hover.contents else {
                panic!("expected markup hover for {id:?}");
            };
            assert!(
                !operators::is_private_use_glyph(&content.value),
                "{id:?} hover (title or body) must not show the private-use glyph"
            );
            assert!(
                content.value.contains(spelling.ascii),
                "{id:?} hover body must show the ASCII spelling {:?}",
                spelling.ascii
            );
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
    fn test_collect_constraints() {
        use rossi::ast::predicate::ComparisonOp;

        let predicates = vec![
            LabeledPredicate {
                label: Some("axm1".into()),
                is_theorem: false,
                predicate: Predicate::comparison(
                    ComparisonOp::In,
                    ExpressionKind::Identifier("max_value".into()).into(),
                    ExpressionKind::Naturals.into(),
                ),
                span: None,
                comment: None,
            },
            LabeledPredicate {
                label: Some("axm2".into()),
                is_theorem: false,
                predicate: Predicate::comparison(
                    ComparisonOp::Equal,
                    ExpressionKind::Identifier("max_value".into()).into(),
                    ExpressionKind::Integer(100).into(),
                ),
                span: None,
                comment: None,
            },
            LabeledPredicate {
                label: Some("axm3".into()),
                is_theorem: false,
                predicate: Predicate::comparison(
                    ComparisonOp::In,
                    ExpressionKind::Identifier("other".into()).into(),
                    ExpressionKind::Integers.into(),
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
        constraints
            .entry("counter".to_string())
            .or_insert_with(HashMap::new)
            .insert(
                "count".to_string(),
                vec![
                    "@inv1 count ∈ ℕ".to_string(),
                    "@inv2 count ≤ max_value".to_string(),
                ],
            );
        constraints
            .entry("counter_ctx".to_string())
            .or_insert_with(HashMap::new)
            .insert(
                "max_value".to_string(),
                vec!["@axm1 max_value ∈ ℕ".to_string()],
            );

        let ctx = HoverContext {
            variables: vec![("count".to_string(), "counter".to_string())],
            constants: vec![("max_value".to_string(), "counter_ctx".to_string())],
            sets: vec![],
            constraints,
            variants: Vec::new(),
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
        let dm = Arc::new(DocumentManager::new());
        dm.open(Url::parse(&uri).unwrap(), 1, source.to_string());
        let mut provider = HoverProvider::new();
        provider.set_document_manager(Arc::clone(&dm));

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
        dm.open(url, 1, abstract_source.to_string());

        crm.update_component("file:///concrete_mch.eventb".to_string(), concrete_source);
        let url = Url::parse("file:///concrete_mch.eventb").unwrap();
        dm.open(url.clone(), 1, concrete_source.to_string());

        let mut provider = HoverProvider::new();
        provider.set_cross_reference_manager(Arc::clone(&crm));
        provider.set_document_manager(Arc::clone(&dm));

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
