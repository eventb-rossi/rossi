//! Pretty printer for converting AST back to Event-B text
//!
//! This module provides functionality to convert parsed AST structures
//! back into formatted Event-B text. It supports both Unicode and ASCII
//! operators, customizable indentation, and produces output that can be
//! parsed back into the same AST (roundtrip support).
//!
//! # Examples
//!
//! Basic usage with default settings (Unicode operators, 4-space indentation):
//!
//! ```
//! use rossi::{parse, to_string};
//!
//! let source = "CONTEXT test\nSETS\n    STATUS\nEND\n";
//! let component = parse(source).unwrap();
//! let output = to_string(&component);
//! println!("{}", output);
//! ```
//!
//! Using ASCII operators:
//!
//! ```
//! use rossi::{parse, to_string_ascii};
//!
//! let source = "CONTEXT test\nEND\n";
//! let component = parse(source).unwrap();
//! let output = to_string_ascii(&component);
//! ```
//!
//! Custom configuration:
//!
//! ```
//! use rossi::{parse, PrettyPrinter};
//!
//! let source = "CONTEXT test\nEND\n";
//! let component = parse(source).unwrap();
//!
//! let printer = PrettyPrinter::new()
//!     .with_indent("  ".to_string()); // 2-space indentation
//! let output = printer.print_component(&component);
//! ```

use crate::ast::context::SetDeclaration;
use crate::ast::expression::{BinaryOp, IdentPattern, UnaryOp};
use crate::ast::predicate::{ComparisonOp, LogicalOp};
use crate::ast::*;
use crate::comments;
use crate::op_info;
use crate::operators::{self, OperatorId};
use std::fmt::Write;

/// Debug guard: a structural name about to be emitted must be re-lexable by
/// the grammar's `component_name` rule, or the printed text could not be
/// parsed back (issue #28). Parser- and XML-built ASTs are validated
/// upstream; this catches programmatically constructed ones.
fn debug_assert_component_name(name: &str, role: &str) {
    debug_assert!(
        crate::names::is_valid_component_name(name),
        "{role} {name:?} is not a valid component name; printed output would not re-parse"
    );
}

/// True when a comment would actually render, i.e. it survives
/// [`comments::normalize_comment`] — the same test [`PrettyPrinter::writeln_commented`]
/// applies. Layout decisions that key on "has a comment" must use this, not a
/// bare `Option::is_some`: an imported blank comment (`Some("")` from a Rodin
/// `comment=""` attribute) prints as an empty line and reparses to `None`, so
/// treating it as a real comment makes the round-trip non-idempotent.
fn renders_comment(comment: Option<&str>) -> bool {
    comment.and_then(comments::normalize_comment).is_some()
}

/// Whitespace convention for formulas emitted by [`PrettyPrinter`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum FormulaSpacing {
    /// Readable Event-B text with spaces around operators and after commas.
    #[default]
    Readable,
    /// Rodin's compact canonical form used in static-checker XML attributes.
    RodinCanonical,
}

/// The top-level formula being rendered. Rodin formats binary type
/// ascriptions differently in predicates than in expressions and actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormulaContext {
    Predicate,
    Expression,
    Action,
}

/// Configuration for the pretty printer
#[derive(Debug, Clone)]
pub struct PrettyPrinter {
    /// Use Unicode operators (true) or ASCII (false)
    pub use_unicode: bool,
    /// Indentation string (default: 4 spaces)
    pub indent: String,
    /// Emit the raw Rodin private-use-area glyphs (U+E100..E103) for the
    /// relation/override operators instead of their ASCII spelling. Off by
    /// default: those glyphs render as tofu without Rodin's font, so output
    /// meant for an editor stays portable. Rodin-canonical formatting (the
    /// static checker's `canonical` form) turns this on to match Rodin's
    /// internal bcc/bcm spelling exactly; see `OperatorSpelling::emit_text`.
    pub private_use_glyphs: bool,
    /// Whitespace convention for formulas.
    pub formula_spacing: FormulaSpacing,
}

impl Default for PrettyPrinter {
    fn default() -> Self {
        Self {
            use_unicode: true,
            indent: "    ".to_string(),
            private_use_glyphs: false,
            formula_spacing: FormulaSpacing::Readable,
        }
    }
}

impl PrettyPrinter {
    /// Create a new pretty printer with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a pretty printer that uses ASCII operators
    pub fn ascii() -> Self {
        Self {
            use_unicode: false,
            private_use_glyphs: false,
            indent: "    ".to_string(),
            formula_spacing: FormulaSpacing::Readable,
        }
    }

    /// Create a printer for Rodin's compact static-checker formula form.
    pub fn rodin_canonical() -> Self {
        Self {
            private_use_glyphs: true,
            formula_spacing: FormulaSpacing::RodinCanonical,
            ..Self::default()
        }
    }

    /// Set the indentation string
    pub fn with_indent(mut self, indent: String) -> Self {
        self.indent = indent;
        self
    }

    /// Emit the raw Rodin private-use glyphs for the relation/override
    /// operators (see [`PrettyPrinter::private_use_glyphs`]).
    pub fn with_private_use_glyphs(mut self, yes: bool) -> Self {
        self.private_use_glyphs = yes;
        self
    }

    /// Set the whitespace convention used for formulas.
    pub fn with_formula_spacing(mut self, formula_spacing: FormulaSpacing) -> Self {
        self.formula_spacing = formula_spacing;
        self
    }

    /// Convert a Component to formatted Event-B text
    pub fn print_component(&self, component: &Component) -> String {
        match component {
            Component::Context(ctx) => self.print_context(ctx),
            Component::Machine(mch) => self.print_machine(mch),
        }
    }

    /// Convert multiple Components to formatted Event-B text, separated by blank lines
    pub fn print_components(&self, components: &[Component]) -> String {
        let mut output = String::new();
        for (i, component) in components.iter().enumerate() {
            if i > 0 {
                output.push('\n');
            }
            output.push_str(&self.print_component(component));
        }
        output
    }

    /// Print one element line followed by its comment, Camille style.
    ///
    /// `line` is the complete element line without the trailing newline and
    /// `indent` is the element's own indentation. A single-line comment
    /// trails the element (`line // text`); a multiline comment becomes a
    /// `/* ... */` block on the following lines, one level deeper, with
    /// continuation lines aligned under the first — the same layout Rodin's
    /// Camille editor prints. Comments are normalized first, so a blank
    /// comment emits nothing and parse → print is idempotent.
    fn writeln_commented(
        &self,
        output: &mut String,
        line: &str,
        comment: Option<&str>,
        indent: &str,
    ) {
        let Some(text) = comment.and_then(comments::normalize_comment) else {
            writeln!(output, "{line}").unwrap();
            return;
        };
        if !text.contains('\n') {
            writeln!(output, "{line} // {text}").unwrap();
            return;
        }
        // `*/` inside the text (possible only via Rodin XML) would close the
        // block early; break it up, losing one byte of fidelity.
        let text = text.replace("*/", "* /");
        writeln!(output, "{line}").unwrap();
        let block_indent = format!("{indent}{}", self.indent);
        let mut lines = text.split('\n');
        let first = lines.next().unwrap();
        write!(output, "{block_indent}/* {first}").unwrap();
        for cont in lines {
            writeln!(output).unwrap();
            if !cont.is_empty() {
                write!(output, "{block_indent}   {cont}").unwrap();
            }
        }
        writeln!(output, " */").unwrap();
    }

    /// Convert a Context to formatted text
    pub fn print_context(&self, context: &Context) -> String {
        let mut output = String::new();

        debug_assert_component_name(&context.name, "context name");
        self.writeln_commented(
            &mut output,
            &format!("CONTEXT {}", context.name),
            context.comment.as_deref(),
            "",
        );

        if !context.extends.is_empty() {
            writeln!(output, "EXTENDS").unwrap();
            for ext in &context.extends {
                debug_assert_component_name(ext, "extends target");
                writeln!(output, "{}{}", self.indent, ext).unwrap();
            }
        }

        if !context.sets.is_empty() {
            writeln!(output, "SETS").unwrap();
            for set in &context.sets {
                let line = match set {
                    SetDeclaration::Deferred { name, .. } => {
                        format!("{}{}", self.indent, name)
                    }
                    SetDeclaration::Enumerated { name, elements, .. } => {
                        format!("{}{} = {{{}}}", self.indent, name, elements.join(", "))
                    }
                };
                self.writeln_commented(&mut output, &line, set.comment(), &self.indent);
            }
        }

        if !context.constants.is_empty() {
            writeln!(output, "CONSTANTS").unwrap();
            for constant in &context.constants {
                self.writeln_commented(
                    &mut output,
                    &format!("{}{}", self.indent, constant.name),
                    constant.comment.as_deref(),
                    &self.indent,
                );
            }
        }

        if !context.axioms.is_empty() {
            writeln!(output, "AXIOMS").unwrap();
            for axiom in &context.axioms {
                self.print_labeled_predicate(&mut output, axiom, &self.indent);
            }
        }

        writeln!(output, "END").unwrap();
        output
    }

    /// Convert a Machine to formatted text
    pub fn print_machine(&self, machine: &Machine) -> String {
        let mut output = String::new();

        debug_assert_component_name(&machine.name, "machine name");
        self.writeln_commented(
            &mut output,
            &format!("MACHINE {}", machine.name),
            machine.comment.as_deref(),
            "",
        );

        if let Some(ref refines) = machine.refines {
            debug_assert_component_name(refines, "refines target");
            writeln!(output, "REFINES").unwrap();
            writeln!(output, "{}{}", self.indent, refines).unwrap();
        }

        if !machine.sees.is_empty() {
            writeln!(output, "SEES").unwrap();
            for sees in &machine.sees {
                debug_assert_component_name(sees, "sees target");
                writeln!(output, "{}{}", self.indent, sees).unwrap();
            }
        }

        if !machine.variables.is_empty() {
            writeln!(output, "VARIABLES").unwrap();
            for var in &machine.variables {
                self.writeln_commented(
                    &mut output,
                    &format!("{}{}", self.indent, var.name),
                    var.comment.as_deref(),
                    &self.indent,
                );
            }
        }

        if !machine.invariants.is_empty() {
            writeln!(output, "INVARIANTS").unwrap();
            for inv in &machine.invariants {
                self.print_labeled_predicate(&mut output, inv, &self.indent);
            }
        }

        if let Some(variant) = &machine.variant {
            writeln!(output, "VARIANT").unwrap();
            writeln!(output, "{}{}", self.indent, self.print_expression(variant)).unwrap();
        }

        if machine.initialisation.is_some() || !machine.events.is_empty() {
            writeln!(output, "EVENTS").unwrap();

            if let Some(init) = &machine.initialisation {
                self.print_initialisation(&mut output, init);
            }

            for event in &machine.events {
                writeln!(output).unwrap();
                self.print_event(&mut output, event);
            }
        }

        writeln!(output, "END").unwrap();
        output
    }

    /// Print a labeled predicate.
    ///
    /// Theorems are always emitted in the inline `theorem @x` form within
    /// AXIOMS/INVARIANTS, never as a separate `THEOREMS` section. This is the
    /// canonical, order-preserving form and mirrors Rodin's model, where a theorem
    /// is a boolean attribute on an axiom/invariant rather than a distinct section.
    /// Parsing a `THEOREMS` section is therefore normalized to inline on output.
    fn print_labeled_predicate(&self, output: &mut String, lp: &LabeledPredicate, indent: &str) {
        let theorem_str = if lp.is_theorem { "theorem " } else { "" };
        let line = if let Some(label) = &lp.label {
            format!(
                "{}{}@{} {}",
                indent,
                theorem_str,
                label,
                self.print_predicate(&lp.predicate)
            )
        } else {
            format!(
                "{}{}{}",
                indent,
                theorem_str,
                self.print_predicate(&lp.predicate)
            )
        };
        self.writeln_commented(output, &line, lp.comment.as_deref(), indent);
    }

    /// Print a labeled action
    fn print_labeled_action(&self, output: &mut String, la: &LabeledAction, indent: &str) {
        let line = if let Some(label) = &la.label {
            format!("{}@{} {}", indent, label, self.print_action(&la.action))
        } else {
            format!("{}{}", indent, self.print_action(&la.action))
        };
        self.writeln_commented(output, &line, la.comment.as_deref(), indent);
    }

    /// Print an action list (one action per line, no separators).
    fn print_action_list(&self, output: &mut String, actions: &[LabeledAction], indent: &str) {
        for action in actions {
            self.print_labeled_action(output, action, indent);
        }
    }

    /// Print an initialisation event
    fn print_initialisation(&self, output: &mut String, init: &InitialisationEvent) {
        let double_indent = format!("{}{}", self.indent, self.indent);
        let header = if init.extended {
            format!("{}EVENT INITIALISATION extends INITIALISATION", self.indent)
        } else {
            format!("{}EVENT INITIALISATION", self.indent)
        };
        self.writeln_commented(output, &header, init.comment.as_deref(), &self.indent);
        if !init.actions.is_empty() {
            writeln!(output, "{}THEN", self.indent).unwrap();
            self.print_action_list(output, &init.actions, &double_indent);
        }
        writeln!(output, "{}END", self.indent).unwrap();
    }

    /// Print an event
    fn print_event(&self, output: &mut String, event: &Event) {
        let double_indent = format!("{}{}", self.indent, self.indent);

        debug_assert_component_name(&event.name, "event name");
        if let Some(ref parent) = event.refines {
            debug_assert_component_name(parent, "event refines target");
        }

        // Emit status inline before EVENT keyword (Camille-compatible form):
        // `convergent EVENT name` instead of `EVENT name\nSTATUS convergent`
        let status_prefix = match &event.status {
            Some(EventStatus::Convergent) => "convergent ",
            Some(EventStatus::Anticipated) => "anticipated ",
            _ => "",
        };

        // When `extended` is true and there is a refines target, use
        // `EVENT name extends parent` syntax (Rodin extension mechanism).
        let header = match &event.refines {
            Some(parent) if event.extended => format!(
                "{}{}EVENT {} extends {}",
                self.indent, status_prefix, event.name, parent
            ),
            _ => format!("{}{}EVENT {}", self.indent, status_prefix, event.name),
        };
        self.writeln_commented(output, &header, event.comment.as_deref(), &self.indent);

        // Print REFINES clause when not extended
        if !event.extended
            && let Some(ref refines) = event.refines
        {
            writeln!(output, "{}REFINES", self.indent).unwrap();
            writeln!(output, "{}{}", double_indent, refines).unwrap();
        }

        if !event.parameters.is_empty() {
            writeln!(output, "{}ANY", self.indent).unwrap();
            if event
                .parameters
                .iter()
                .any(|p| renders_comment(p.comment.as_deref()))
            {
                // A commented parameter needs its own line for the trailing
                // comment to re-attach to it on reparse.
                for param in &event.parameters {
                    self.writeln_commented(
                        output,
                        &format!("{}{}", double_indent, param.name),
                        param.comment.as_deref(),
                        &double_indent,
                    );
                }
            } else {
                let param_names: Vec<&str> =
                    event.parameters.iter().map(|p| p.name.as_str()).collect();
                // Parameters are whitespace-separated, not comma-separated, so
                // the line reparses under the structural-list grammar.
                writeln!(output, "{}{}", double_indent, param_names.join(" ")).unwrap();
            }
        }

        if !event.guards.is_empty() {
            writeln!(output, "{}WHERE", self.indent).unwrap();
            for guard in &event.guards {
                self.print_labeled_predicate(output, guard, &double_indent);
            }
        }

        if !event.with.is_empty() {
            writeln!(output, "{}WITH", self.indent).unwrap();
            for lp in &event.with {
                self.print_labeled_predicate(output, lp, &double_indent);
            }
        }

        if !event.witnesses.is_empty() {
            writeln!(output, "{}WITNESS", self.indent).unwrap();
            for lp in &event.witnesses {
                self.print_labeled_predicate(output, lp, &double_indent);
            }
        }

        if !event.actions.is_empty() {
            writeln!(output, "{}THEN", self.indent).unwrap();
            self.print_action_list(output, &event.actions, &double_indent);
        }

        writeln!(output, "{}END", self.indent).unwrap();
    }

    /// Convert an Expression to text
    pub fn print_expression(&self, expr: &Expression) -> String {
        self.print_expression_with_context(expr, FormulaContext::Expression)
    }

    fn print_expression_with_context(&self, expr: &Expression, context: FormulaContext) -> String {
        match &expr.kind {
            ExpressionKind::Integer(n) => n.to_string(),
            ExpressionKind::Identifier(name) => name.clone(),
            // TRUE and FALSE are members of the BOOL carrier set (expression-level
            // constants), not predicate constants ⊤/⊥.  They are always rendered
            // as identifiers regardless of Unicode mode.
            ExpressionKind::True => "TRUE".to_string(),
            ExpressionKind::False => "FALSE".to_string(),
            ExpressionKind::EmptySet => self.op(OperatorId::EmptySet).to_string(),
            ExpressionKind::Naturals => self.op(OperatorId::Naturals).to_string(),
            ExpressionKind::Naturals1 => self.op(OperatorId::Naturals1).to_string(),
            ExpressionKind::Integers => self.op(OperatorId::Integers).to_string(),
            ExpressionKind::BoolType => "BOOL".to_string(),

            ExpressionKind::SetEnumeration(elements) => {
                let elems: Vec<String> = elements
                    .iter()
                    .map(|e| self.print_expression_with_context(e, context))
                    .collect();
                format!("{{{}}}", elems.join(self.comma_separator()))
            }

            ExpressionKind::SetComprehension {
                identifiers,
                predicate,
                expression,
            } => {
                let ids_str = self.format_typed_identifiers(identifiers, context);
                if let Some(expr) = expression {
                    // Extended form: {x · P | E}
                    let mid = self.op(OperatorId::Dot);
                    let bar = self.op(OperatorId::Bar);
                    format!(
                        "{{{}{}{}{}{}}}",
                        ids_str,
                        mid,
                        self.print_predicate_with_context(predicate, context),
                        bar,
                        self.print_expression_with_context(expr, context)
                    )
                } else {
                    // Basic form: {x | P}
                    let bar = self.op(OperatorId::Bar);
                    format!(
                        "{{{}{}{}}}",
                        ids_str,
                        bar,
                        self.print_predicate_with_context(predicate, context)
                    )
                }
            }

            ExpressionKind::SetBuilder {
                member_expression,
                predicate,
            } => {
                let bar = self.op(OperatorId::Bar);
                format!(
                    "{{{}{}{}}}",
                    self.print_expression_with_context(member_expression, context),
                    bar,
                    self.print_predicate_with_context(predicate, context)
                )
            }

            ExpressionKind::RelationalImage { relation, set } => {
                // Relational image [S] binds at the primary level, so binary
                // and unary expressions as the relation need parentheses to
                // avoid `a + b[S]` being parsed as `a + (b[S])`.
                let relation_str = if Self::needs_parens_for_relational_image(relation) {
                    format!(
                        "({})",
                        self.print_expression_with_context(relation, context)
                    )
                } else {
                    self.print_expression_with_context(relation, context)
                };
                format!(
                    "{}[{}]",
                    relation_str,
                    self.print_expression_with_context(set, context)
                )
            }

            ExpressionKind::QuantifiedUnion {
                identifiers,
                predicate,
                expression,
            } => {
                let op = self.op(OperatorId::QuantifiedUnion);
                self.format_quantified_expr(op, identifiers, predicate, expression, context)
            }

            ExpressionKind::QuantifiedInter {
                identifiers,
                predicate,
                expression,
            } => {
                let op = self.op(OperatorId::QuantifiedIntersection);
                self.format_quantified_expr(op, identifiers, predicate, expression, context)
            }

            ExpressionKind::Lambda {
                pattern,
                predicate,
                expression,
            } => {
                let lambda = self.op(OperatorId::Lambda);
                let mid = self.op(OperatorId::Dot);
                let bar = self.op(OperatorId::Bar);
                format!(
                    "{} {}{}{}{}{}",
                    lambda,
                    self.print_ident_pattern(pattern, context),
                    mid,
                    self.print_predicate_with_context(predicate, context),
                    bar,
                    self.print_expression_with_context(expression, context)
                )
            }

            ExpressionKind::Binary { op, left, right } => {
                let op_str = self.print_binary_op(*op);
                let left_str = self.print_child_expr(left, *op, false, context);
                let right_str = self.print_child_expr(right, *op, true, context);
                let separator = self.binary_separator(*op, context);
                format!("{left_str}{separator}{op_str}{separator}{right_str}")
            }

            ExpressionKind::Unary { op, operand } => {
                let op_str = self.print_unary_op(*op);
                if *op == UnaryOp::Inverse {
                    // Postfix: operand∼ — parenthesize complex operands
                    let needs_parens = !matches!(
                        operand.as_ref().kind,
                        ExpressionKind::Identifier(_)
                            | ExpressionKind::AtomicBuiltin(_)
                            | ExpressionKind::Integer(_)
                            | ExpressionKind::FunctionApplication { .. }
                            | ExpressionKind::RelationalImage { .. }
                            | ExpressionKind::BuiltinApplication { .. }
                            | ExpressionKind::Unary {
                                op: UnaryOp::Inverse,
                                ..
                            }
                    );
                    let operand_str = self.print_expression_with_context(operand, context);
                    if needs_parens {
                        format!("({}){}", operand_str, op_str)
                    } else {
                        format!("{}{}", operand_str, op_str)
                    }
                } else {
                    let operand_str = self.print_expression_with_context(operand, context);
                    format!("{}({})", op_str, operand_str)
                }
            }

            ExpressionKind::FunctionApplication { function, argument } => {
                let mut func_str = self.print_expression_with_context(function, context);
                if Self::needs_parens_for_relational_image(function) {
                    func_str = format!("({})", func_str);
                }
                format!(
                    "{}({})",
                    func_str,
                    self.print_expression_with_context(argument, context)
                )
            }

            ExpressionKind::BuiltinApplication { function, argument } => {
                format!(
                    "{}({})",
                    function.name(),
                    self.print_expression_with_context(argument, context)
                )
            }

            // Generic relational atom (`id`, `prj1`, …): a bare word.
            ExpressionKind::AtomicBuiltin(kind) => kind.name().to_string(),

            ExpressionKind::Bool(predicate) => {
                format!(
                    "bool({})",
                    self.print_predicate_with_context(predicate, context)
                )
            }
        }
    }

    /// Format a list of typed identifiers, rendering type annotations with ⦂/oftype
    fn format_typed_identifiers(
        &self,
        identifiers: &[TypedIdentifier],
        context: FormulaContext,
    ) -> String {
        identifiers
            .iter()
            .map(|id| {
                if let Some(ref type_expr) = id.type_expr {
                    let oftype = self.oftype_annotation();
                    format!(
                        "{}{}{}",
                        id.name,
                        oftype,
                        self.print_expression_with_context(type_expr, context)
                    )
                } else {
                    id.name.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(self.comma_separator())
    }

    /// Format a quantified expression (QuantifiedUnion, QuantifiedInter)
    fn format_quantified_expr(
        &self,
        keyword: &str,
        identifiers: &[TypedIdentifier],
        predicate: &Predicate,
        expression: &Expression,
        context: FormulaContext,
    ) -> String {
        let mid = self.op(OperatorId::Dot);
        let bar = self.op(OperatorId::Bar);
        let ids_str = self.format_typed_identifiers(identifiers, context);
        format!(
            "{} {}{}{}{}{}",
            keyword,
            ids_str,
            mid,
            self.print_predicate_with_context(predicate, context),
            bar,
            self.print_expression_with_context(expression, context)
        )
    }

    /// Print a lambda ident-pattern
    fn print_ident_pattern(&self, pattern: &IdentPattern, context: FormulaContext) -> String {
        match pattern {
            IdentPattern::Identifier(t) => match &t.type_expr {
                Some(ty) => {
                    let oftype = self.oftype_annotation();
                    format!(
                        "{}{}{}",
                        t.name,
                        oftype,
                        self.print_expression_with_context(ty, context)
                    )
                }
                None => t.name.clone(),
            },
            IdentPattern::Maplet(left, right) => {
                let maplet = self.op(OperatorId::Maplet);
                let left_str = self.print_ident_pattern(left, context);
                // Right child needs parens only if it's a Maplet (since maplet is left-assoc)
                let right_str = match right.as_ref() {
                    IdentPattern::Maplet(_, _) => {
                        format!("({})", self.print_ident_pattern(right, context))
                    }
                    _ => self.print_ident_pattern(right, context),
                };
                format!("{} {} {}", left_str, maplet, right_str)
            }
        }
    }

    /// Print a child of a binary expression, adding parentheses only when needed
    fn print_child_expr(
        &self,
        child: &Expression,
        parent_op: BinaryOp,
        is_right: bool,
        context: FormulaContext,
    ) -> String {
        // Quantified/lambda expressions sit above binary operators in the grammar
        // hierarchy, so they must be parenthesized when appearing as operands.
        if matches!(
            child.kind,
            ExpressionKind::Lambda { .. }
                | ExpressionKind::QuantifiedUnion { .. }
                | ExpressionKind::QuantifiedInter { .. }
        ) {
            return format!("({})", self.print_expression_with_context(child, context));
        }
        if let ExpressionKind::Binary { op: child_op, .. } = &child.kind {
            let child_prec = op_info::binary_precedence(*child_op);
            let parent_prec = op_info::binary_precedence(parent_op);

            let needs_parens = if child_prec < parent_prec {
                true
            } else if child_prec > parent_prec {
                false
            } else {
                // Same precedence: check compatibility matrix
                !op_info::binary_ops_compatible(*child_op, parent_op)
                    || op_info::is_non_associative(parent_op)
                    || is_right
            };

            if needs_parens {
                return format!("({})", self.print_expression_with_context(child, context));
            }
        }
        self.print_expression_with_context(child, context)
    }

    /// Check if an expression needs parentheses when used as the relation of
    /// a relational image (`expr[S]`). Binary, unary, and quantified forms
    /// need wrapping because `[S]` binds at the primary expression level.
    fn needs_parens_for_relational_image(expr: &Expression) -> bool {
        match &expr.kind {
            ExpressionKind::Unary { op, .. } if *op == UnaryOp::Inverse => false,
            ExpressionKind::Binary { .. }
            | ExpressionKind::Unary { .. }
            | ExpressionKind::Lambda { .. }
            | ExpressionKind::QuantifiedUnion { .. }
            | ExpressionKind::QuantifiedInter { .. } => true,
            _ => false,
        }
    }

    /// Print an expression in a context where only a `pair-expression` (or lower)
    /// is valid. Lambda, quantified union, and quantified intersection expressions
    /// sit above `pair-expression` in the grammar and must be parenthesized.
    fn print_expr_as_pair(&self, expr: &Expression, context: FormulaContext) -> String {
        if matches!(
            expr.kind,
            ExpressionKind::Lambda { .. }
                | ExpressionKind::QuantifiedUnion { .. }
                | ExpressionKind::QuantifiedInter { .. }
        ) {
            format!("({})", self.print_expression_with_context(expr, context))
        } else {
            self.print_expression_with_context(expr, context)
        }
    }

    #[inline]
    fn comma_separator(&self) -> &'static str {
        match self.formula_spacing {
            FormulaSpacing::Readable => ", ",
            FormulaSpacing::RodinCanonical => ",",
        }
    }

    #[inline]
    fn tight_operator_separator(&self, id: OperatorId) -> &'static str {
        if self.formula_spacing == FormulaSpacing::RodinCanonical
            && (self.use_unicode || operators::spelling(id).is_symbolic())
        {
            ""
        } else {
            " "
        }
    }

    #[inline]
    fn binary_separator(&self, op: BinaryOp, context: FormulaContext) -> &'static str {
        if self.formula_spacing == FormulaSpacing::RodinCanonical
            && self.rodin_binary_is_tight(op, context)
        {
            ""
        } else {
            " "
        }
    }

    /// Whether Rodin removes whitespace around this binary expression
    /// operator. Keep this match exhaustive so a new AST operator cannot gain
    /// an accidental default spacing policy.
    fn rodin_binary_is_tight(&self, op: BinaryOp, context: FormulaContext) -> bool {
        match op {
            BinaryOp::Add
            | BinaryOp::Multiply
            | BinaryOp::Union
            | BinaryOp::Intersection
            | BinaryOp::CartesianProduct
            | BinaryOp::Overwrite => true,
            // Rodin tightens a binary type ascription in a predicate, while
            // standalone expressions and assignments retain spaces. ASCII
            // `oftype` needs word-separating whitespace in every context.
            BinaryOp::OfType => self.use_unicode && context == FormulaContext::Predicate,
            BinaryOp::Subtract
            | BinaryOp::Divide
            | BinaryOp::Modulo
            | BinaryOp::Exponent
            | BinaryOp::Range
            | BinaryOp::Difference
            | BinaryOp::Relation
            | BinaryOp::TotalRelation
            | BinaryOp::SurjectiveRelation
            | BinaryOp::TotalSurjectiveRelation
            | BinaryOp::TotalFunction
            | BinaryOp::PartialFunction
            | BinaryOp::TotalInjection
            | BinaryOp::PartialInjection
            | BinaryOp::TotalSurjection
            | BinaryOp::PartialSurjection
            | BinaryOp::Bijection
            | BinaryOp::Composition
            | BinaryOp::Semicolon
            | BinaryOp::DomainRestriction
            | BinaryOp::DomainSubtraction
            | BinaryOp::RangeRestriction
            | BinaryOp::RangeSubtraction
            | BinaryOp::DirectProduct
            | BinaryOp::ParallelProduct
            | BinaryOp::Maplet => false,
        }
    }

    /// Pick between a Unicode and ASCII symbol based on the printer mode.
    #[inline]
    fn sym(&self, unicode: &'static str, ascii: &'static str) -> &'static str {
        if self.use_unicode { unicode } else { ascii }
    }

    /// Pick an operator spelling from the shared Event-B table. Unless
    /// `private_use_glyphs` is set, this routes through `emit_text` so the
    /// private-use relation/override operators print as ASCII (their glyph
    /// won't render without Rodin's font).
    #[inline]
    fn op(&self, id: OperatorId) -> &'static str {
        if self.private_use_glyphs {
            operators::spell(id, self.use_unicode)
        } else {
            operators::spelling(id).emit_text(self.use_unicode)
        }
    }

    #[inline]
    fn oftype_annotation(&self) -> &'static str {
        if self.use_unicode {
            self.op(OperatorId::OfType)
        } else {
            " oftype "
        }
    }

    /// Convert a binary operator to text
    fn print_binary_op(&self, op: BinaryOp) -> &'static str {
        self.op(operators::binary_op_id(op))
    }

    /// Convert a unary operator to text
    fn print_unary_op(&self, op: UnaryOp) -> &'static str {
        self.op(operators::unary_op_id(op))
    }

    /// Convert a Predicate to text
    pub fn print_predicate(&self, pred: &Predicate) -> String {
        self.print_predicate_with_context(pred, FormulaContext::Predicate)
    }

    fn print_predicate_with_context(&self, pred: &Predicate, context: FormulaContext) -> String {
        match &pred.kind {
            PredicateKind::True => self.sym("⊤", "true").to_string(),
            PredicateKind::False => self.sym("⊥", "false").to_string(),

            PredicateKind::Comparison { op, left, right } => {
                let op_str = self.print_comparison_op(*op);
                let left_str = self.print_expr_as_pair(left, context);
                let right_str = self.print_expr_as_pair(right, context);
                let separator = self.tight_operator_separator(operators::comparison_op_id(*op));
                format!("{left_str}{separator}{op_str}{separator}{right_str}")
            }

            PredicateKind::Not(p) => {
                let not = self.op(OperatorId::Not);
                format!("{}({})", not, self.print_predicate_with_context(p, context))
            }

            PredicateKind::Logical { op, left, right } => {
                let op_str = self.print_logical_op(*op);
                let left_str = self.print_predicate_child(left, *op, false, context);
                let right_str = self.print_predicate_child(right, *op, true, context);
                let separator = self.tight_operator_separator(operators::logical_op_id(*op));
                format!("{left_str}{separator}{op_str}{separator}{right_str}")
            }

            PredicateKind::Quantified {
                quantifier,
                identifiers,
                predicate,
            } => {
                let quant_str = self.op(operators::quantifier_id(*quantifier));
                let mid = self.op(OperatorId::Dot);
                let ids_str = self.format_typed_identifiers(identifiers, context);
                format!(
                    "{}{}{}{}",
                    quant_str,
                    ids_str,
                    mid,
                    self.print_predicate_with_context(predicate, context)
                )
            }

            PredicateKind::Application {
                function,
                arguments,
            } => {
                let args: Vec<String> = arguments
                    .iter()
                    .map(|a| self.print_expression_with_context(a, context))
                    .collect();
                format!(
                    "{}({})",
                    function.as_str(),
                    args.join(self.comma_separator())
                )
            }

            PredicateKind::BuiltinApplication {
                predicate,
                arguments,
            } => {
                let args: Vec<String> = arguments
                    .iter()
                    .map(|a| self.print_expression_with_context(a, context))
                    .collect();
                format!(
                    "{}({})",
                    predicate.name(),
                    args.join(self.comma_separator())
                )
            }
        }
    }

    /// Print a child predicate of a logical connective, adding parentheses
    /// when necessary for correct precedence and associativity.
    fn print_predicate_child(
        &self,
        child: &Predicate,
        parent_op: LogicalOp,
        is_right: bool,
        context: FormulaContext,
    ) -> String {
        let needs_parens = match &child.kind {
            // Quantifiers are below all logical connectives in the grammar
            // hierarchy, so they always need parentheses inside a logical op.
            PredicateKind::Quantified { .. } => true,
            PredicateKind::Logical { op: child_op, .. } => {
                let child_prec = op_info::logical_precedence(*child_op);
                let parent_prec = op_info::logical_precedence(parent_op);
                if child_prec < parent_prec {
                    true // lower precedence → needs parens
                } else if child_prec > parent_prec {
                    false // higher precedence → no parens
                } else {
                    // Same precedence: check Camille compatibility class
                    let child_class = op_info::logical_compat_class(*child_op);
                    let parent_class = op_info::logical_compat_class(parent_op);
                    if child_class == 0 || parent_class == 0 || child_class != parent_class {
                        true // Incompatible (And↔Or, Implies↔Implies, etc.)
                    } else {
                        is_right // Same class, left-associative: right child gets parens
                    }
                }
            }
            _ => false,
        };
        if needs_parens {
            format!("({})", self.print_predicate_with_context(child, context))
        } else {
            self.print_predicate_with_context(child, context)
        }
    }

    /// Convert a comparison operator to text
    fn print_comparison_op(&self, op: ComparisonOp) -> &'static str {
        self.op(operators::comparison_op_id(op))
    }

    /// Convert a logical operator to text
    fn print_logical_op(&self, op: LogicalOp) -> &'static str {
        self.op(operators::logical_op_id(op))
    }

    /// True if `s` contains a `;` outside all (), [], {} delimiters —
    /// i.e. a top-level forward composition (the printer emits `;` for nothing else).
    fn has_bare_semicolon(s: &str) -> bool {
        let mut depth = 0usize;
        for c in s.chars() {
            match c {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth = depth.saturating_sub(1),
                ';' if depth == 0 => return true,
                _ => {}
            }
        }
        false
    }

    /// Wrap `s` in parentheses iff it has a bare `;`, so the text-format
    /// `action` rule (whose `_no_semi` expression variants reserve `;` for
    /// action boundaries) can re-parse printed actions. Parentheses are
    /// precedence-derived, not AST nodes, so the round-tripped AST is
    /// identical.
    fn guard_action_part(s: String) -> String {
        if Self::has_bare_semicolon(&s) {
            format!("({})", s)
        } else {
            s
        }
    }

    /// Print an expression in an action position, guarding bare `;`.
    fn print_action_expr(&self, expr: &Expression, context: FormulaContext) -> String {
        Self::guard_action_part(self.print_expression_with_context(expr, context))
    }

    /// Convert an Action to text
    pub fn print_action(&self, action: &Action) -> String {
        let context = FormulaContext::Action;
        let assign = self.op(OperatorId::Assignment);
        match &action.kind {
            ActionKind::Skip => "skip".to_string(),
            ActionKind::Assignment { assignments } => {
                let vars = join_idents(
                    assignments.iter().map(|(variable, _)| variable),
                    self.comma_separator(),
                );
                let exprs: Vec<String> = assignments
                    .iter()
                    .map(|(_, expression)| self.print_action_expr(expression, context))
                    .collect();
                format!("{} {} {}", vars, assign, exprs.join(self.comma_separator()))
            }
            ActionKind::BecomesIn { variables, set } => {
                let vars = join_idents(variables, self.comma_separator());
                let op = self.op(OperatorId::BecomesIn);
                format!("{} {} {}", vars, op, self.print_action_expr(set, context))
            }
            ActionKind::BecomesSuchThat {
                variables,
                predicate,
            } => {
                let vars = join_idents(variables, self.comma_separator());
                let op = self.op(OperatorId::BecomesSuchThat);
                format!(
                    "{} {} {}",
                    vars,
                    op,
                    Self::guard_action_part(self.print_predicate_with_context(predicate, context))
                )
            }
        }
    }
}

/// Render a comma-separated list of identifier names (assignment / becomes LHS).
fn join_idents<'a>(vars: impl IntoIterator<Item = &'a Ident>, separator: &str) -> String {
    vars.into_iter()
        .map(Ident::as_str)
        .collect::<Vec<_>>()
        .join(separator)
}

/// Convenience function to convert a Component to text with default settings
pub fn to_string(component: &Component) -> String {
    PrettyPrinter::new().print_component(component)
}

/// Convenience function to convert a Component to ASCII text
pub fn to_string_ascii(component: &Component) -> String {
    PrettyPrinter::ascii().print_component(component)
}

/// Convenience function to convert multiple Components to text with default settings
pub fn components_to_string(components: &[Component]) -> String {
    PrettyPrinter::new().print_components(components)
}

/// Convenience function to convert multiple Components to ASCII text
pub fn components_to_string_ascii(components: &[Component]) -> String {
    PrettyPrinter::ascii().print_components(components)
}

/// Parse Event-B text (one or more components) and re-emit it formatted with
/// `printer`.
///
/// This is the shared parse-then-print entry point: `rossi fmt` and the language
/// server both format through it, and `rossi import` prints through the same
/// [`PrettyPrinter`], so command-line and editor formatting always agree.
///
/// # Errors
///
/// Returns a [`ParseError`](crate::ParseError) if `src` is not valid Event-B.
pub fn format_str(src: &str, printer: &PrettyPrinter) -> Result<String, crate::error::ParseError> {
    let components = crate::parser::parse_components(src)?;
    Ok(printer.print_components(&components))
}
