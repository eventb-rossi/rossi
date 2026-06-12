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

/// Configuration for the pretty printer
#[derive(Debug, Clone)]
pub struct PrettyPrinter {
    /// Use Unicode operators (true) or ASCII (false)
    pub use_unicode: bool,
    /// Indentation string (default: 4 spaces)
    pub indent: String,
}

impl Default for PrettyPrinter {
    fn default() -> Self {
        Self {
            use_unicode: true,
            indent: "    ".to_string(),
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
            indent: "    ".to_string(),
        }
    }

    /// Set the indentation string
    pub fn with_indent(mut self, indent: String) -> Self {
        self.indent = indent;
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
            if event.parameters.iter().any(|p| p.comment.is_some()) {
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
                writeln!(output, "{}{}", double_indent, param_names.join(", ")).unwrap();
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
        match expr {
            Expression::Integer(n) => n.to_string(),
            Expression::Identifier(name) => name.clone(),
            // TRUE and FALSE are members of the BOOL carrier set (expression-level
            // constants), not predicate constants ⊤/⊥.  They are always rendered
            // as identifiers regardless of Unicode mode.
            Expression::True => "TRUE".to_string(),
            Expression::False => "FALSE".to_string(),
            Expression::EmptySet => self.op(OperatorId::EmptySet).to_string(),
            Expression::Naturals => self.op(OperatorId::Naturals).to_string(),
            Expression::Naturals1 => self.op(OperatorId::Naturals1).to_string(),
            Expression::Integers => self.op(OperatorId::Integers).to_string(),
            Expression::BoolType => "BOOL".to_string(),

            Expression::SetEnumeration(elements) => {
                let elems: Vec<String> =
                    elements.iter().map(|e| self.print_expression(e)).collect();
                format!("{{{}}}", elems.join(", "))
            }

            Expression::SetComprehension {
                identifiers,
                predicate,
                expression,
            } => {
                let ids_str = self.format_typed_identifiers(identifiers);
                if let Some(expr) = expression {
                    // Extended form: {x · P | E}
                    let mid = self.op(OperatorId::Dot);
                    let bar = self.op(OperatorId::Bar);
                    format!(
                        "{{{}{}{}{}{}}}",
                        ids_str,
                        mid,
                        self.print_predicate(predicate),
                        bar,
                        self.print_expression(expr)
                    )
                } else {
                    // Basic form: {x | P}
                    let bar = self.op(OperatorId::Bar);
                    format!("{{{}{}{}}}", ids_str, bar, self.print_predicate(predicate))
                }
            }

            Expression::SetBuilder {
                member_expression,
                predicate,
            } => {
                let bar = self.op(OperatorId::Bar);
                format!(
                    "{{{}{}{}}}",
                    self.print_expression(member_expression),
                    bar,
                    self.print_predicate(predicate)
                )
            }

            Expression::RelationalImage { relation, set } => {
                // Relational image [S] binds at the primary level, so binary
                // and unary expressions as the relation need parentheses to
                // avoid `a + b[S]` being parsed as `a + (b[S])`.
                let relation_str = if Self::needs_parens_for_relational_image(relation) {
                    format!("({})", self.print_expression(relation))
                } else {
                    self.print_expression(relation)
                };
                format!("{}[{}]", relation_str, self.print_expression(set))
            }

            Expression::QuantifiedUnion {
                identifiers,
                predicate,
                expression,
            } => {
                let op = self.op(OperatorId::QuantifiedUnion);
                self.format_quantified_expr(op, identifiers, predicate, expression)
            }

            Expression::QuantifiedInter {
                identifiers,
                predicate,
                expression,
            } => {
                let op = self.op(OperatorId::QuantifiedIntersection);
                self.format_quantified_expr(op, identifiers, predicate, expression)
            }

            Expression::Lambda {
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
                    self.print_ident_pattern(pattern),
                    mid,
                    self.print_predicate(predicate),
                    bar,
                    self.print_expression(expression)
                )
            }

            Expression::Binary { op, left, right } => {
                let op_str = self.print_binary_op(*op);
                let left_str = self.print_child_expr(left, *op, false);
                let right_str = self.print_child_expr(right, *op, true);
                format!("{} {} {}", left_str, op_str, right_str)
            }

            Expression::Unary { op, operand } => {
                let op_str = self.print_unary_op(*op);
                if *op == UnaryOp::Inverse {
                    // Postfix: operand∼ — parenthesize complex operands
                    let needs_parens = !matches!(
                        operand.as_ref(),
                        Expression::Identifier(_)
                            | Expression::Integer(_)
                            | Expression::FunctionApplication { .. }
                            | Expression::RelationalImage { .. }
                            | Expression::BuiltinApplication { .. }
                            | Expression::Unary {
                                op: UnaryOp::Inverse,
                                ..
                            }
                    );
                    let operand_str = self.print_expression(operand);
                    if needs_parens {
                        format!("({}){}", operand_str, op_str)
                    } else {
                        format!("{}{}", operand_str, op_str)
                    }
                } else {
                    let operand_str = self.print_expression(operand);
                    format!("{}({})", op_str, operand_str)
                }
            }

            Expression::FunctionApplication {
                function,
                arguments,
            } => {
                let mut func_str = self.print_expression(function);
                if Self::needs_parens_for_relational_image(function) {
                    func_str = format!("({})", func_str);
                }
                let args: Vec<String> =
                    arguments.iter().map(|a| self.print_expression(a)).collect();
                format!("{}({})", func_str, args.join(", "))
            }

            Expression::BuiltinApplication {
                function,
                arguments,
            } => {
                let args: Vec<String> =
                    arguments.iter().map(|a| self.print_expression(a)).collect();
                format!("{}({})", function.name(), args.join(", "))
            }

            Expression::Bool(predicate) => {
                format!("bool({})", self.print_predicate(predicate))
            }

            Expression::StringLiteral(s) => {
                let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
                format!("\"{}\"", escaped)
            }

            Expression::IfThenElse {
                condition,
                then_expr,
                else_expr,
            } => {
                format!(
                    "IF {} THEN {} ELSE {} END",
                    self.print_predicate(condition),
                    self.print_expression(then_expr),
                    self.print_expression(else_expr)
                )
            }
        }
    }

    /// Format a list of typed identifiers, rendering type annotations with ⦂/oftype
    fn format_typed_identifiers(&self, identifiers: &[TypedIdentifier]) -> String {
        identifiers
            .iter()
            .map(|id| {
                if let Some(ref type_expr) = id.type_expr {
                    let oftype = self.oftype_annotation();
                    format!("{}{}{}", id.name, oftype, self.print_expression(type_expr))
                } else {
                    id.name.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Format a quantified expression (QuantifiedUnion, QuantifiedInter)
    fn format_quantified_expr(
        &self,
        keyword: &str,
        identifiers: &[TypedIdentifier],
        predicate: &Predicate,
        expression: &Expression,
    ) -> String {
        let mid = self.op(OperatorId::Dot);
        let bar = self.op(OperatorId::Bar);
        let ids_str = self.format_typed_identifiers(identifiers);
        format!(
            "{} {}{}{}{}{}",
            keyword,
            ids_str,
            mid,
            self.print_predicate(predicate),
            bar,
            self.print_expression(expression)
        )
    }

    /// Print a lambda ident-pattern
    fn print_ident_pattern(&self, pattern: &IdentPattern) -> String {
        match pattern {
            IdentPattern::Identifier(t) => match &t.type_expr {
                Some(ty) => {
                    let oftype = self.oftype_annotation();
                    format!("{}{}{}", t.name, oftype, self.print_expression(ty))
                }
                None => t.name.clone(),
            },
            IdentPattern::Maplet(left, right) => {
                let maplet = self.op(OperatorId::Maplet);
                let left_str = self.print_ident_pattern(left);
                // Right child needs parens only if it's a Maplet (since maplet is left-assoc)
                let right_str = match right.as_ref() {
                    IdentPattern::Maplet(_, _) => {
                        format!("({})", self.print_ident_pattern(right))
                    }
                    _ => self.print_ident_pattern(right),
                };
                format!("{} {} {}", left_str, maplet, right_str)
            }
        }
    }

    /// Print a child of a binary expression, adding parentheses only when needed
    fn print_child_expr(&self, child: &Expression, parent_op: BinaryOp, is_right: bool) -> String {
        // Quantified/lambda expressions sit above binary operators in the grammar
        // hierarchy, so they must be parenthesized when appearing as operands.
        if matches!(
            child,
            Expression::Lambda { .. }
                | Expression::QuantifiedUnion { .. }
                | Expression::QuantifiedInter { .. }
        ) {
            return format!("({})", self.print_expression(child));
        }
        if let Expression::Binary { op: child_op, .. } = child {
            let child_prec = Self::op_precedence(*child_op);
            let parent_prec = Self::op_precedence(parent_op);

            let needs_parens = if child_prec < parent_prec {
                true
            } else if child_prec > parent_prec {
                false
            } else {
                // Same precedence: check compatibility matrix
                if !Self::ops_are_compatible(*child_op, parent_op) {
                    true // Incompatible operators: always need parens
                } else if Self::is_right_associative(parent_op) {
                    !is_right // left child needs parens for right-associative
                } else if Self::is_non_associative(parent_op) {
                    true // non-associative ops always need parens at same level
                } else {
                    is_right // right child needs parens for left-associative
                }
            };

            if needs_parens {
                return format!("({})", self.print_expression(child));
            }
        }
        self.print_expression(child)
    }

    /// Precedence level of a binary operator (higher = binds tighter)
    fn op_precedence(op: BinaryOp) -> u8 {
        match op {
            // Maplet / pair constructor (lowest binary precedence per
            // kernel_lang Table 3.1: `a ↦ b ↔ c = a ↦ (b ↔ c)`)
            BinaryOp::Maplet => 1,

            // Relation types (bind tighter than maplet, looser than set ops)
            BinaryOp::Relation
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
            | BinaryOp::OfType => 2,

            // Binary set operators
            BinaryOp::Union
            | BinaryOp::Intersection
            | BinaryOp::Difference
            | BinaryOp::CartesianProduct
            | BinaryOp::Overwrite
            | BinaryOp::Semicolon
            | BinaryOp::Composition
            | BinaryOp::DomainRestriction
            | BinaryOp::DomainSubtraction
            | BinaryOp::RangeRestriction
            | BinaryOp::RangeSubtraction
            | BinaryOp::DirectProduct
            | BinaryOp::ParallelProduct => 3,

            // Interval
            BinaryOp::Range => 4,

            // Additive (arithmetic only)
            BinaryOp::Add | BinaryOp::Subtract => 5,

            // Multiplicative (arithmetic only)
            BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Modulo => 6,

            // Exponent — highest arithmetic precedence per spec §3.3.6
            BinaryOp::Exponent => 7,
        }
    }

    fn is_right_associative(_op: BinaryOp) -> bool {
        // Event-B has no right-associative binary operators at expression
        // level. Maplet is left-associative per spec p.18 (`a ↦ b ↦ c =
        // (a ↦ b) ↦ c`). Kept as a function for symmetry with
        // `is_non_associative`.
        false
    }

    fn is_non_associative(op: BinaryOp) -> bool {
        matches!(
            op,
            BinaryOp::Range
                | BinaryOp::Exponent
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
                | BinaryOp::OfType
        )
    }

    /// Check if an expression needs parentheses when used as the relation of
    /// a relational image (`expr[S]`). Binary, unary, and quantified forms
    /// need wrapping because `[S]` binds at the primary expression level.
    fn needs_parens_for_relational_image(expr: &Expression) -> bool {
        match expr {
            Expression::Unary { op, .. } if *op == UnaryOp::Inverse => false,
            Expression::Binary { .. }
            | Expression::Unary { .. }
            | Expression::Lambda { .. }
            | Expression::QuantifiedUnion { .. }
            | Expression::QuantifiedInter { .. } => true,
            _ => false,
        }
    }

    /// Print an expression in a context where only a `pair-expression` (or lower)
    /// is valid. Lambda, quantified union, and quantified intersection expressions
    /// sit above `pair-expression` in the grammar and must be parenthesized.
    fn print_expr_as_pair(&self, expr: &Expression) -> String {
        if matches!(
            expr,
            Expression::Lambda { .. }
                | Expression::QuantifiedUnion { .. }
                | Expression::QuantifiedInter { .. }
        ) {
            format!("({})", self.print_expression(expr))
        } else {
            self.print_expression(expr)
        }
    }

    /// Pick between a Unicode and ASCII symbol based on the printer mode.
    #[inline]
    fn sym(&self, unicode: &'static str, ascii: &'static str) -> &'static str {
        if self.use_unicode { unicode } else { ascii }
    }

    /// Pick an operator spelling from the shared Event-B table.
    #[inline]
    fn op(&self, id: OperatorId) -> &'static str {
        operators::spell(id, self.use_unicode)
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
        match pred {
            Predicate::True => self.sym("⊤", "TRUE").to_string(),
            Predicate::False => self.sym("⊥", "FALSE").to_string(),

            Predicate::Comparison { op, left, right } => {
                let op_str = self.print_comparison_op(*op);
                let left_str = self.print_expr_as_pair(left);
                let right_str = self.print_expr_as_pair(right);
                format!("{} {} {}", left_str, op_str, right_str)
            }

            Predicate::Not(p) => {
                let not = self.op(OperatorId::Not);
                format!("{}({})", not, self.print_predicate(p))
            }

            Predicate::Logical { op, left, right } => {
                let op_str = self.print_logical_op(*op);
                let left_str = self.print_predicate_child(left, *op, false);
                let right_str = self.print_predicate_child(right, *op, true);
                format!("{} {} {}", left_str, op_str, right_str)
            }

            Predicate::Quantified {
                quantifier,
                identifiers,
                predicate,
            } => {
                let quant_str = self.op(operators::quantifier_id(*quantifier));
                let mid = self.op(OperatorId::Dot);
                let ids_str = self.format_typed_identifiers(identifiers);
                format!(
                    "{}{}{}{}",
                    quant_str,
                    ids_str,
                    mid,
                    self.print_predicate(predicate)
                )
            }

            Predicate::Application {
                function,
                arguments,
            } => {
                let args: Vec<String> =
                    arguments.iter().map(|a| self.print_expression(a)).collect();
                format!("{}({})", function, args.join(", "))
            }

            Predicate::BuiltinApplication {
                predicate,
                arguments,
            } => {
                let args: Vec<String> =
                    arguments.iter().map(|a| self.print_expression(a)).collect();
                format!("{}({})", predicate.name(), args.join(", "))
            }
        }
    }

    /// Precedence of a logical operator (higher = binds tighter).
    ///
    /// And/Or share the same precedence level; Camille compatibility classes
    /// (see `pred_compat_class`) decide whether parentheses are needed.
    fn logical_op_precedence(op: LogicalOp) -> u8 {
        match op {
            LogicalOp::Equivalent => 1,
            LogicalOp::Implies => 2,
            LogicalOp::Or | LogicalOp::And => 3,
        }
    }

    /// Camille compatibility class for predicate logical operators.
    ///
    /// Operators at the same precedence level but in different classes always
    /// require explicit parentheses. Class 0 means "singleton" — incompatible
    /// with everything, including itself.
    fn pred_compat_class(op: LogicalOp) -> u8 {
        match op {
            LogicalOp::And => 1,
            LogicalOp::Or => 2,
            LogicalOp::Implies | LogicalOp::Equivalent => 0, // non-associative singletons
        }
    }

    /// Check whether two set-level operators are compatible for mixing
    /// without parentheses. The `child` operator appears as the left operand
    /// of the `parent` operator in a flat sequence: `... child ... parent ...`.
    ///
    /// This is an asymmetric relation derived empirically from the Rodin
    /// formula parser's actual behaviour.
    fn are_set_ops_compatible(child: BinaryOp, parent: BinaryOp) -> bool {
        use BinaryOp::*;
        matches!(
            (child, parent),
            (Union, Union)
                | (Intersection, Intersection)
                | (Intersection, Difference)
                | (Composition, Composition)
                | (Semicolon, Semicolon)
                | (Overwrite, Overwrite)
                | (DomainRestriction, Intersection)
                | (DomainRestriction, Difference)
                | (DomainRestriction, Semicolon)
                | (DomainSubtraction, Intersection)
                | (DomainSubtraction, Difference)
                | (DomainSubtraction, Semicolon)
        )
    }

    /// Check whether two same-precedence operators are compatible (can mix
    /// without parentheses). For arithmetic and other non-set levels, uses
    /// simple same-operator grouping.
    fn ops_are_compatible(child: BinaryOp, parent: BinaryOp) -> bool {
        let prec = Self::op_precedence(child);
        debug_assert_eq!(prec, Self::op_precedence(parent));

        match prec {
            // Set operator level — use the asymmetric compatibility matrix
            p if p == Self::op_precedence(BinaryOp::Union) => {
                Self::are_set_ops_compatible(child, parent)
            }
            // Additive: + and - can freely mix (left-associative)
            p if p == Self::op_precedence(BinaryOp::Add) => true,
            // Multiplicative: *, ÷, mod can freely mix (left-associative)
            p if p == Self::op_precedence(BinaryOp::Multiply) => true,
            // Maplet: left-associative, self-compatible
            p if p == Self::op_precedence(BinaryOp::Maplet) => child == parent,
            // Everything else (arrows, range, exponent): incompatible
            _ => false,
        }
    }

    /// Print a child predicate of a logical connective, adding parentheses
    /// when necessary for correct precedence and associativity.
    fn print_predicate_child(
        &self,
        child: &Predicate,
        parent_op: LogicalOp,
        is_right: bool,
    ) -> String {
        let needs_parens = match child {
            // Quantifiers are below all logical connectives in the grammar
            // hierarchy, so they always need parentheses inside a logical op.
            Predicate::Quantified { .. } => true,
            Predicate::Logical { op: child_op, .. } => {
                let child_prec = Self::logical_op_precedence(*child_op);
                let parent_prec = Self::logical_op_precedence(parent_op);
                if child_prec < parent_prec {
                    true // lower precedence → needs parens
                } else if child_prec > parent_prec {
                    false // higher precedence → no parens
                } else {
                    // Same precedence: check Camille compatibility class
                    let child_class = Self::pred_compat_class(*child_op);
                    let parent_class = Self::pred_compat_class(parent_op);
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
            format!("({})", self.print_predicate(child))
        } else {
            self.print_predicate(child)
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

    /// True if `s` contains a `;` outside all (), [], {} delimiters and
    /// string literals — i.e. a top-level forward composition (the printer
    /// emits `;` for nothing else).
    fn has_bare_semicolon(s: &str) -> bool {
        let mut depth = 0usize;
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            match c {
                // Skip string-literal content: a stray delimiter or `;`
                // inside it must not affect the scan. The printer escapes
                // only `\` and `"` (see Expression::StringLiteral).
                '"' => {
                    while let Some(c) = chars.next() {
                        match c {
                            '\\' => {
                                chars.next();
                            }
                            '"' => break,
                            _ => {}
                        }
                    }
                }
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
    fn print_action_expr(&self, expr: &Expression) -> String {
        Self::guard_action_part(self.print_expression(expr))
    }

    /// Convert an Action to text
    pub fn print_action(&self, action: &Action) -> String {
        let assign = self.op(OperatorId::Assignment);
        match action {
            Action::Skip => "skip".to_string(),
            Action::Assignment {
                variables,
                expressions,
            } => {
                let vars = variables.join(", ");
                let exprs: Vec<String> = expressions
                    .iter()
                    .map(|e| self.print_action_expr(e))
                    .collect();
                format!("{} {} {}", vars, assign, exprs.join(", "))
            }
            Action::BecomesIn { variables, set } => {
                let vars = variables.join(", ");
                let op = self.op(OperatorId::BecomesIn);
                format!("{} {} {}", vars, op, self.print_action_expr(set))
            }
            Action::BecomesSuchThat {
                variables,
                predicate,
            } => {
                let vars = variables.join(", ");
                let op = self.op(OperatorId::BecomesSuchThat);
                format!(
                    "{} {} {}",
                    vars,
                    op,
                    Self::guard_action_part(self.print_predicate(predicate))
                )
            }
            Action::FunctionOverride {
                function,
                arguments,
                expression,
            } => {
                let args: Vec<String> = arguments
                    .iter()
                    .map(|e| self.print_action_expr(e))
                    .collect();
                format!(
                    "{}({}) {} {}",
                    function,
                    args.join(", "),
                    assign,
                    self.print_action_expr(expression)
                )
            }
        }
    }
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
