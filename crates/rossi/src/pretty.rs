//! Pretty printer for converting AST back to Event-B text
//!
//! This module provides functionality to convert parsed AST structures
//! back into formatted Event-B text. It supports both Unicode and ASCII
//! operators, customizable indentation, a choice of structural layout
//! ([`Style`]), and produces output that can be parsed back into the same
//! AST (roundtrip support).
//!
//! # Examples
//!
//! Basic usage with default settings (the Camille style: Unicode
//! operators, lowercase keywords, 2-space indentation):
//!
//! ```
//! use rossi::{parse, to_string};
//!
//! let source = "CONTEXT test\nSETS\n    STATUS\nEND\n";
//! let component = parse(source).unwrap();
//! let output = to_string(&component);
//! assert_eq!(output, "context test\n\nsets STATUS\nend\n");
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
//! use rossi::{parse, PrettyPrinter, Style};
//!
//! let source = "CONTEXT test\nEND\n";
//! let component = parse(source).unwrap();
//!
//! // The original uppercase layout with 2-space indentation.
//! let printer = PrettyPrinter::styled(Style::Rossi).with_indent("  ".to_string());
//! let output = printer.print_component(&component);
//! ```

use crate::ast::*;
use crate::comments;
use crate::op_info;
use crate::operators::{self, OperatorId};
use crate::operators::{BinaryOp, UnaryOp};
use crate::operators::{ComparisonOp, LogicalOp, Quantifier};
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

/// Structural layout preset for Event-B text output.
///
/// The preset drives the axes that are not individually togglable — inline
/// vs block header clauses (`refines`/`sees`/`extends`), variant layout,
/// the event indentation ladder, and the blank-line shape inside `events`
/// — and supplies the defaults for the togglable axes via
/// [`PrettyPrinter::styled`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// The layout Rodin's Camille text editor prints: lowercase keywords,
    /// header clauses and declaration lists inline, a blank line between
    /// clauses and events, 2-space indent.
    #[default]
    Camille,
    /// rossi's original layout: uppercase keywords, every clause payload
    /// broken onto indented lines, no blank lines between clauses,
    /// 4-space indent.
    Rossi,
}

/// Casing of structural keywords (`MACHINE` vs `machine`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeywordCase {
    Lower,
    Upper,
}

/// Layout of header clauses (`extends`/`refines`/`sees`), resolved from
/// the [`Style`] preset so a new preset is forced to choose one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderClauseLayout {
    /// Inline on the component/event header line (Camille), moved onto
    /// continuation lines at the configured width.
    Inline,
    /// A block clause: keyword on its own line, one target per line.
    Block,
}

/// Layout of identifier declaration lists
/// (`variables`/`sets`/`constants`/`any`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclListLayout {
    /// Names space-separated on the keyword line, continuation lines
    /// hanging-aligned under the first name (Camille).
    Inline,
    /// Keyword alone on its line, one name per line at one indent.
    OnePerLine,
}

/// Default maximum line width, in characters, for the user-facing
/// formatter paths (CLI `fmt`/`import`, LSP formatting). The presets
/// themselves never wrap; the user-facing paths opt in with this width
/// unless overridden.
pub const DEFAULT_MAX_LINE_WIDTH: usize = 120;

/// Explicit overrides applied on top of a [`Style`] preset by
/// [`PrettyPrinter::resolved`]. `None` fields follow the preset;
/// `use_unicode` and `max_line_width` are plain values because every
/// preset agrees on them (Unicode on, wrapping off), so "follow the
/// preset" and the default coincide.
#[derive(Debug, Clone)]
pub struct StyleOverrides {
    pub keyword_case: Option<KeywordCase>,
    pub decl_lists: Option<DeclListLayout>,
    pub blank_between_clauses: Option<bool>,
    /// `None` keeps the preset's indent; `Some("")` is an explicit empty
    /// indent.
    pub indent: Option<String>,
    pub use_unicode: bool,
    /// Emit the raw Rodin private-use glyphs for the relation/override
    /// operators (see [`PrettyPrinter::private_use_glyphs`]).
    pub private_use_glyphs: bool,
    /// `0` disables wrapping.
    pub max_line_width: usize,
}

impl Default for StyleOverrides {
    fn default() -> Self {
        Self {
            keyword_case: None,
            decl_lists: None,
            blank_between_clauses: None,
            indent: None,
            use_unicode: true,
            private_use_glyphs: false,
            max_line_width: 0,
        }
    }
}

/// Whitespace convention for formulas emitted by [`PrettyPrinter`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum FormulaSpacing {
    /// Readable Event-B text with spaces around operators and after commas.
    #[default]
    Readable,
    /// Rodin's compact canonical form used in static-checker XML attributes.
    RodinCanonical,
    /// Rodin's `Formula#toString()` form used in marker diagnostics.
    RodinFormulaString,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrettyPrinter {
    /// Use Unicode operators (true) or ASCII (false)
    pub use_unicode: bool,
    /// Indentation string (default: the style preset's — 2 spaces camille,
    /// 4 spaces rossi)
    pub indent: String,
    /// Emit the raw Rodin private-use-area glyphs (U+E100..E103) for the
    /// relation/override operators instead of their ASCII spelling. Off by
    /// default: those glyphs render as tofu without Rodin's font, so output
    /// meant for an editor stays portable. Rodin-canonical formatting (the
    /// static checker's `canonical` form) turns this on to match Rodin's
    /// internal bcc/bcm spelling exactly, and users interoperating with a tool
    /// that reads only Rodin's spelling opt in through `--private-use-glyphs`
    /// or `rossi.format.privateUseGlyphs`; see `OperatorSpelling::emit_text`.
    pub private_use_glyphs: bool,
    /// Whitespace convention for formulas.
    pub formula_spacing: FormulaSpacing,
    /// When printing formula-model declarations, spell their solved
    /// types (`x⦂ℤ`) instead of only their source annotations — what
    /// canonical static-checker text uses.
    pub typed_decls: bool,
    /// Structural layout preset (see [`Style`]).
    pub style: Style,
    /// Casing of structural keywords.
    pub keyword_case: KeywordCase,
    /// Layout of header clauses (`extends`/`refines`/`sees`).
    pub header_clauses: HeaderClauseLayout,
    /// Layout of identifier declaration lists.
    pub decl_lists: DeclListLayout,
    /// Emit one blank line before each top-level clause keyword.
    pub blank_between_clauses: bool,
    /// Maximum output line width in characters (a tab counts as one);
    /// `0` disables wrapping. Off in every preset — only the user-facing
    /// formatter paths (CLI `fmt`/`import`, LSP formatting) opt in via
    /// [`StyleOverrides`] — so Rodin-canonical strings and XML attribute
    /// values can never gain newlines. Long formulas wrap onto
    /// operator-leading continuation lines hanging-aligned under the
    /// formula's start column; comment text is never wrapped, so a
    /// trailing `//` comment may exceed the width. A wrapped element in
    /// an entirely unlabelled clause slightly degrades the recovery
    /// parser's per-line blame when the file already has errors.
    pub max_line_width: usize,
}

impl Default for PrettyPrinter {
    fn default() -> Self {
        Self::styled(Style::default())
    }
}

impl PrettyPrinter {
    /// Create a new pretty printer with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// A printer for the given style preset, every axis (including the
    /// indent) at the preset's default.
    pub fn styled(style: Style) -> Self {
        let (indent, keyword_case, header_clauses, decl_lists, blank_between_clauses) = match style
        {
            Style::Camille => (
                "  ",
                KeywordCase::Lower,
                HeaderClauseLayout::Inline,
                DeclListLayout::Inline,
                true,
            ),
            Style::Rossi => (
                "    ",
                KeywordCase::Upper,
                HeaderClauseLayout::Block,
                DeclListLayout::OnePerLine,
                false,
            ),
        };
        Self {
            use_unicode: true,
            indent: indent.to_string(),
            private_use_glyphs: false,
            formula_spacing: FormulaSpacing::Readable,
            typed_decls: false,
            style,
            keyword_case,
            header_clauses,
            decl_lists,
            blank_between_clauses,
            max_line_width: 0,
        }
    }

    /// The preset + override resolution every user-facing formatter path
    /// (CLI `fmt`/`import`, LSP formatting, Rodin model sync) builds its
    /// printer through, so they can never disagree on what a style means.
    pub fn resolved(style: Style, overrides: &StyleOverrides) -> Self {
        let mut printer = Self::styled(style);
        if let Some(keyword_case) = overrides.keyword_case {
            printer.keyword_case = keyword_case;
        }
        if let Some(decl_lists) = overrides.decl_lists {
            printer.decl_lists = decl_lists;
        }
        if let Some(blank) = overrides.blank_between_clauses {
            printer.blank_between_clauses = blank;
        }
        if let Some(indent) = &overrides.indent {
            printer.indent = indent.clone();
        }
        printer.use_unicode = overrides.use_unicode;
        printer.private_use_glyphs = overrides.private_use_glyphs;
        printer.max_line_width = overrides.max_line_width;
        printer
    }

    /// Set the maximum line width (`0` disables wrapping).
    pub fn with_max_line_width(mut self, width: usize) -> Self {
        self.max_line_width = width;
        self
    }

    /// Create a pretty printer that uses ASCII operators
    pub fn ascii() -> Self {
        Self {
            use_unicode: false,
            ..Self::default()
        }
    }

    /// Spell solved declaration types when printing formula-model
    /// declarations.
    pub fn with_typed_decls(mut self, typed_decls: bool) -> Self {
        self.typed_decls = typed_decls;
        self
    }

    /// Create a printer for Rodin's compact static-checker formula form.
    pub fn rodin_canonical() -> Self {
        Self {
            private_use_glyphs: true,
            formula_spacing: FormulaSpacing::RodinCanonical,
            ..Self::default()
        }
    }

    /// Create a printer matching Rodin's formula spelling in diagnostics.
    pub fn rodin_formula_string() -> Self {
        Self::rodin_canonical().with_formula_spacing(FormulaSpacing::RodinFormulaString)
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

    /// The structural keyword in the configured case.
    #[inline]
    fn kw(&self, upper: &'static str, lower: &'static str) -> &'static str {
        match self.keyword_case {
            KeywordCase::Upper => upper,
            KeywordCase::Lower => lower,
        }
    }

    /// Blank line before a top-level clause keyword, when the style asks
    /// for one. Never emitted before the closing END.
    #[inline]
    fn clause_gap(&self, output: &mut String) {
        if self.blank_between_clauses {
            output.push('\n');
        }
    }

    /// Append an inline header clause segment (`refines m0`, `sees c1 c2`),
    /// moving the whole segment onto a continuation line at `cont_indent`
    /// when the current physical line would exceed the configured width,
    /// and filling the segment's own words across continuation lines when
    /// even a line of its own is not wide enough (a long `sees` list).
    /// Breaking before a clause keyword or between names is safe —
    /// newlines are ordinary whitespace in the grammar.
    fn push_header_segment(&self, header: &mut String, segment: &str, cont_indent: &str) {
        let line_width = end_col(0, header);
        let segment_width = segment.chars().count();
        if self.max_line_width == 0 || line_width + 1 + segment_width <= self.max_line_width {
            header.push(' ');
            header.push_str(segment);
            return;
        }
        header.push('\n');
        header.push_str(cont_indent);
        let cont_width = cont_indent.chars().count();
        if cont_width + segment_width <= self.max_line_width {
            header.push_str(segment);
            return;
        }
        let mut cur = cont_width;
        for (i, word) in segment.split(' ').enumerate() {
            let w = word.chars().count();
            if i > 0 {
                if cur + 1 + w <= self.max_line_width {
                    header.push(' ');
                    cur += 1;
                } else {
                    header.push('\n');
                    header.push_str(cont_indent);
                    cur = cont_width;
                }
            }
            header.push_str(word);
            cur += w;
        }
    }

    /// Indentation ladder inside EVENTS per style: (event header, event
    /// body keyword, body item). Camille indents the body keywords one
    /// level below the event header; the original layout keeps them level.
    fn event_ladder(&self) -> (String, String, String) {
        match self.style {
            Style::Camille => (
                self.indent.clone(),
                self.indent.repeat(2),
                self.indent.repeat(3),
            ),
            Style::Rossi => (
                self.indent.clone(),
                self.indent.clone(),
                self.indent.repeat(2),
            ),
        }
    }

    /// Print an identifier list inline on the keyword line, Camille style:
    /// names space-separated, continuation lines hanging-aligned under the
    /// first name (in characters, like Camille). A commented name always
    /// ends its physical line, so its trailing `//` (or following block)
    /// re-attaches to that name on reparse; the remaining names continue
    /// on the next hanging line.
    fn print_inline_name_list(
        &self,
        output: &mut String,
        keyword: &str,
        clause_indent: &str,
        items: &[NamedElement],
    ) {
        let head = format!("{clause_indent}{keyword}");
        let hang = " ".repeat(head.chars().count());
        let mut line = head;
        let mut pending = false;
        for item in items {
            // Wrap to the hanging column when the next name would exceed
            // the width — but never break between the keyword (or a fresh
            // hanging line) and its first name.
            if pending
                && self.max_line_width > 0
                && line.chars().count() + 1 + item.name.chars().count() > self.max_line_width
            {
                writeln!(output, "{line}").unwrap();
                line.clear();
                line.push_str(&hang);
            }
            write!(line, " {}", item.name).unwrap();
            pending = true;
            if renders_comment(item.comment.as_deref()) {
                self.writeln_commented(output, &line, item.comment.as_deref(), &hang);
                line.clear();
                line.push_str(&hang);
                pending = false;
            }
        }
        if pending {
            writeln!(output, "{line}").unwrap();
        }
    }

    /// Convert a Context to formatted text
    pub fn print_context(&self, context: &Context) -> String {
        let mut output = String::new();

        debug_assert_component_name(&context.name, "context name");
        let mut header = format!("{} {}", self.kw("CONTEXT", "context"), context.name);
        if self.header_clauses == HeaderClauseLayout::Inline && !context.extends.is_empty() {
            let mut segment = self.kw("EXTENDS", "extends").to_string();
            for ext in &context.extends {
                debug_assert_component_name(ext, "extends target");
                write!(segment, " {ext}").unwrap();
            }
            self.push_header_segment(&mut header, &segment, &self.indent);
        }
        self.writeln_commented(&mut output, &header, context.comment.as_deref(), "");

        if self.header_clauses == HeaderClauseLayout::Block && !context.extends.is_empty() {
            self.clause_gap(&mut output);
            writeln!(output, "{}", self.kw("EXTENDS", "extends")).unwrap();
            for ext in &context.extends {
                debug_assert_component_name(ext, "extends target");
                writeln!(output, "{}{}", self.indent, ext).unwrap();
            }
        }

        if !context.sets.is_empty() {
            self.clause_gap(&mut output);
            self.print_decl_list(&mut output, self.kw("SETS", "sets"), &context.sets);
        }

        if !context.constants.is_empty() {
            self.clause_gap(&mut output);
            self.print_decl_list(
                &mut output,
                self.kw("CONSTANTS", "constants"),
                &context.constants,
            );
        }

        if !context.axioms.is_empty() {
            self.clause_gap(&mut output);
            writeln!(output, "{}", self.kw("AXIOMS", "axioms")).unwrap();
            for axiom in &context.axioms {
                self.print_labeled_predicate(&mut output, axiom, &self.indent);
            }
        }

        writeln!(output, "{}", self.kw("END", "end")).unwrap();
        output
    }

    /// Print a top-level declaration list (`sets`/`constants`/`variables`)
    /// in the configured [`DeclListLayout`].
    fn print_decl_list(&self, output: &mut String, keyword: &str, items: &[NamedElement]) {
        match self.decl_lists {
            DeclListLayout::Inline => self.print_inline_name_list(output, keyword, "", items),
            DeclListLayout::OnePerLine => {
                writeln!(output, "{keyword}").unwrap();
                for item in items {
                    self.writeln_commented(
                        output,
                        &format!("{}{}", self.indent, item.name),
                        item.comment.as_deref(),
                        &self.indent,
                    );
                }
            }
        }
    }

    /// Convert a Machine to formatted text
    pub fn print_machine(&self, machine: &Machine) -> String {
        let mut output = String::new();

        debug_assert_component_name(&machine.name, "machine name");
        let mut header = format!("{} {}", self.kw("MACHINE", "machine"), machine.name);
        if self.header_clauses == HeaderClauseLayout::Inline {
            if let Some(ref refines) = machine.refines {
                debug_assert_component_name(refines, "refines target");
                let segment = format!("{} {refines}", self.kw("REFINES", "refines"));
                self.push_header_segment(&mut header, &segment, &self.indent);
            }
            if !machine.sees.is_empty() {
                let mut segment = self.kw("SEES", "sees").to_string();
                for sees in &machine.sees {
                    debug_assert_component_name(sees, "sees target");
                    write!(segment, " {sees}").unwrap();
                }
                self.push_header_segment(&mut header, &segment, &self.indent);
            }
        }
        self.writeln_commented(&mut output, &header, machine.comment.as_deref(), "");

        if self.header_clauses == HeaderClauseLayout::Block {
            if let Some(ref refines) = machine.refines {
                debug_assert_component_name(refines, "refines target");
                self.clause_gap(&mut output);
                writeln!(output, "{}", self.kw("REFINES", "refines")).unwrap();
                writeln!(output, "{}{}", self.indent, refines).unwrap();
            }

            if !machine.sees.is_empty() {
                self.clause_gap(&mut output);
                writeln!(output, "{}", self.kw("SEES", "sees")).unwrap();
                for sees in &machine.sees {
                    debug_assert_component_name(sees, "sees target");
                    writeln!(output, "{}{}", self.indent, sees).unwrap();
                }
            }
        }

        if !machine.variables.is_empty() {
            self.clause_gap(&mut output);
            self.print_decl_list(
                &mut output,
                self.kw("VARIABLES", "variables"),
                &machine.variables,
            );
        }

        if !machine.invariants.is_empty() {
            self.clause_gap(&mut output);
            writeln!(output, "{}", self.kw("INVARIANTS", "invariants")).unwrap();
            for inv in &machine.invariants {
                self.print_labeled_predicate(&mut output, inv, &self.indent);
            }
        }

        if !machine.variants.is_empty() {
            self.clause_gap(&mut output);
            let keyword = self.kw("VARIANT", "variant");
            match self.style {
                Style::Camille => {
                    // The first variant sits inline on the keyword line;
                    // later variants need their `@label` sigil to delimit
                    // the expressions.
                    for (i, variant) in machine.variants.iter().enumerate() {
                        let head = match &variant.label {
                            Some(label) if i == 0 => format!("{keyword} @{label} "),
                            None if i == 0 => format!("{keyword} "),
                            _ => format!("{}@{} ", self.indent, variant.effective_label()),
                        };
                        let base = if i == 0 { "" } else { self.indent.as_str() };
                        let expr = self.print_expression_at(
                            &variant.expression,
                            head.chars().count(),
                            base.chars().count(),
                        );
                        let line = format!("{head}{expr}");
                        self.writeln_commented(
                            &mut output,
                            &line,
                            variant.comment.as_deref(),
                            base,
                        );
                    }
                }
                Style::Rossi => {
                    writeln!(output, "{keyword}").unwrap();
                    for (i, variant) in machine.variants.iter().enumerate() {
                        // The grammar only allows a bare expression in first
                        // position; spell out the default label elsewhere so
                        // the output stays parseable.
                        let head = match &variant.label {
                            Some(label) => format!("{}@{label} ", self.indent),
                            None if i == 0 => self.indent.clone(),
                            None => {
                                format!("{}@{} ", self.indent, crate::ast::DEFAULT_VARIANT_LABEL)
                            }
                        };
                        let expr = self.print_expression_at(
                            &variant.expression,
                            head.chars().count(),
                            self.indent.chars().count(),
                        );
                        let line = format!("{head}{expr}");
                        let indent = self.indent.clone();
                        self.writeln_commented(
                            &mut output,
                            &line,
                            variant.comment.as_deref(),
                            &indent,
                        );
                    }
                }
            }
        }

        if machine.initialisation.is_some() || !machine.events.is_empty() {
            self.clause_gap(&mut output);
            writeln!(output, "{}", self.kw("EVENTS", "events")).unwrap();

            // Camille: no blank line before the first item, one blank line
            // between successive items. Rossi: a blank line before every
            // non-INITIALISATION event.
            let blank_before_first_event = match self.style {
                Style::Camille => machine.initialisation.is_some(),
                Style::Rossi => true,
            };
            if let Some(init) = &machine.initialisation {
                self.print_initialisation(&mut output, init);
            }
            for (i, event) in machine.events.iter().enumerate() {
                if i > 0 || blank_before_first_event {
                    writeln!(output).unwrap();
                }
                self.print_event(&mut output, event);
            }
        }

        writeln!(output, "{}", self.kw("END", "end")).unwrap();
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
        let head = match &lp.label {
            Some(label) => format!("{indent}{theorem_str}@{label} "),
            None => format!("{indent}{theorem_str}"),
        };
        let rendered =
            self.print_predicate_at(&lp.predicate, head.chars().count(), indent.chars().count());
        let line = format!("{head}{rendered}");
        self.writeln_commented(output, &line, lp.comment.as_deref(), indent);
    }

    /// Print a labeled action
    fn print_labeled_action(&self, output: &mut String, la: &LabeledAction, indent: &str) {
        let head = match &la.label {
            Some(label) => format!("{indent}@{label} "),
            None => indent.to_string(),
        };
        let rendered =
            self.print_action_at(&la.action, head.chars().count(), indent.chars().count());
        let line = format!("{head}{rendered}");
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
        let (event_indent, kw_indent, item_indent) = self.event_ladder();
        let event_kw = self.kw("EVENT", "event");
        let header = if init.extended {
            format!("{event_indent}{event_kw} INITIALISATION extends INITIALISATION")
        } else {
            format!("{event_indent}{event_kw} INITIALISATION")
        };
        self.writeln_commented(output, &header, init.comment.as_deref(), &event_indent);
        if !init.actions.is_empty() {
            writeln!(output, "{kw_indent}{}", self.kw("THEN", "then")).unwrap();
            self.print_action_list(output, &init.actions, &item_indent);
        }
        writeln!(output, "{event_indent}{}", self.kw("END", "end")).unwrap();
    }

    /// Print an event
    fn print_event(&self, output: &mut String, event: &Event) {
        let (event_indent, kw_indent, item_indent) = self.event_ladder();

        debug_assert_component_name(&event.name, "event name");
        for target in &event.refines {
            debug_assert_component_name(&target.name, "event refines target");
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
        let event_kw = self.kw("EVENT", "event");
        let mut header = match event.refines.first() {
            Some(parent) if event.extended => format!(
                "{event_indent}{status_prefix}{event_kw} {} extends {}",
                event.name, parent.name
            ),
            _ => format!("{event_indent}{status_prefix}{event_kw} {}", event.name),
        };
        if self.header_clauses == HeaderClauseLayout::Inline
            && !event.extended
            && !event.refines.is_empty()
        {
            let mut segment = self.kw("REFINES", "refines").to_string();
            for target in &event.refines {
                write!(segment, " {}", target.name).unwrap();
            }
            self.push_header_segment(&mut header, &segment, &kw_indent);
        }
        self.writeln_commented(output, &header, event.comment.as_deref(), &event_indent);

        // Print REFINES as a block clause when not extended, one target per
        // line (Camille inlines the targets on the header line instead).
        if self.header_clauses == HeaderClauseLayout::Block
            && !event.extended
            && !event.refines.is_empty()
        {
            writeln!(output, "{kw_indent}{}", self.kw("REFINES", "refines")).unwrap();
            for target in &event.refines {
                writeln!(output, "{item_indent}{}", target.name).unwrap();
            }
        }

        if !event.parameters.is_empty() {
            let any_kw = self.kw("ANY", "any");
            match self.decl_lists {
                DeclListLayout::Inline => {
                    self.print_inline_name_list(output, any_kw, &kw_indent, &event.parameters);
                }
                DeclListLayout::OnePerLine => {
                    writeln!(output, "{kw_indent}{any_kw}").unwrap();
                    for param in &event.parameters {
                        self.writeln_commented(
                            output,
                            &format!("{item_indent}{}", param.name),
                            param.comment.as_deref(),
                            &item_indent,
                        );
                    }
                }
            }
        }

        if !event.guards.is_empty() {
            writeln!(output, "{kw_indent}{}", self.kw("WHERE", "where")).unwrap();
            for guard in &event.guards {
                self.print_labeled_predicate(output, guard, &item_indent);
            }
        }

        if !event.with.is_empty() {
            writeln!(output, "{kw_indent}{}", self.kw("WITH", "with")).unwrap();
            for lp in &event.with {
                self.print_labeled_predicate(output, lp, &item_indent);
            }
        }

        if !event.witnesses.is_empty() {
            writeln!(output, "{kw_indent}{}", self.kw("WITNESS", "witness")).unwrap();
            for lp in &event.witnesses {
                self.print_labeled_predicate(output, lp, &item_indent);
            }
        }

        if !event.actions.is_empty() {
            writeln!(output, "{kw_indent}{}", self.kw("THEN", "then")).unwrap();
            self.print_action_list(output, &event.actions, &item_indent);
        }

        writeln!(output, "{event_indent}{}", self.kw("END", "end")).unwrap();
    }

    #[inline]
    fn comma_separator(&self) -> &'static str {
        match self.formula_spacing {
            FormulaSpacing::Readable => ", ",
            FormulaSpacing::RodinCanonical | FormulaSpacing::RodinFormulaString => ",",
        }
    }

    #[inline]
    fn tight_operator_separator(&self, id: OperatorId) -> &'static str {
        if self.formula_spacing != FormulaSpacing::Readable
            && (self.use_unicode || operators::spelling(id).is_symbolic())
        {
            ""
        } else {
            " "
        }
    }

    #[inline]
    fn binary_separator(&self, op: BinaryOp, context: FormulaContext) -> &'static str {
        match self.formula_spacing {
            FormulaSpacing::Readable => " ",
            FormulaSpacing::RodinCanonical if self.rodin_binary_is_tight(op, context) => "",
            FormulaSpacing::RodinFormulaString if self.formula_string_binary_is_tight(op) => "",
            FormulaSpacing::RodinCanonical | FormulaSpacing::RodinFormulaString => " ",
        }
    }

    fn formula_string_binary_is_tight(&self, op: BinaryOp) -> bool {
        matches!(
            op,
            BinaryOp::Add
                | BinaryOp::Multiply
                | BinaryOp::Union
                | BinaryOp::Intersection
                | BinaryOp::Overwrite
                | BinaryOp::Composition
                | BinaryOp::Semicolon
        )
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
    /// `private_use_glyphs` is set, `emit_text` prints the private-use
    /// relation/override operators as ASCII (their glyph won't render without
    /// Rodin's font).
    #[inline]
    fn op(&self, id: OperatorId) -> &'static str {
        operators::spelling(id).emit_text(self.use_unicode, self.private_use_glyphs)
    }

    #[inline]
    fn oftype_annotation(&self) -> &'static str {
        if self.use_unicode {
            self.op(OperatorId::OfType)
        } else {
            " oftype "
        }
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

    /// Format an action body: `skip`, or the modelled assignment.
    pub fn print_action_body(&self, body: &crate::ast::ActionBody) -> String {
        match body {
            crate::ast::ActionBody::Skip { .. } => "skip".to_string(),
            crate::ast::ActionBody::Assignment(assignment) => {
                self.print_formula_assignment(assignment)
            }
        }
    }
}

// ===== formula-model printing =====
//
// Formulas print through the shared operator tables (spelling,
// precedence, spacing), so text and structural printing stay in one
// configuration. Bound identifiers are
// resolved to names through a stack of enclosing declaration names;
// a declaration keeps its hint unless a name visible in its body would
// be captured, in which case it is freshened.

use crate::formula::fresh::{FreshNameSolver, resolve_idents};
use crate::formula::tag::{
    AssocExprOp, AssocPredOp, AtomicOp, BinaryExprOp, BinaryPredOp, LiteralPredOp, QuantExprOp,
    QuantPredOp, RelationalOp, UnaryExprOp,
};
use crate::formula::{self, ExpressionKind as FExprKind, Form, PredicateKind as FPredKind};

/// The legacy operator equivalent of a binary formula operator, for the
/// shared precedence/spacing tables. Function application and
/// relational image print structurally and never go through this.
fn legacy_binary(op: BinaryExprOp) -> BinaryOp {
    match op {
        BinaryExprOp::Mapsto => BinaryOp::Maplet,
        BinaryExprOp::Rel => BinaryOp::Relation,
        BinaryExprOp::TRel => BinaryOp::TotalRelation,
        BinaryExprOp::SRel => BinaryOp::SurjectiveRelation,
        BinaryExprOp::STRel => BinaryOp::TotalSurjectiveRelation,
        BinaryExprOp::PFun => BinaryOp::PartialFunction,
        BinaryExprOp::TFun => BinaryOp::TotalFunction,
        BinaryExprOp::PInj => BinaryOp::PartialInjection,
        BinaryExprOp::TInj => BinaryOp::TotalInjection,
        BinaryExprOp::PSur => BinaryOp::PartialSurjection,
        BinaryExprOp::TSur => BinaryOp::TotalSurjection,
        BinaryExprOp::TBij => BinaryOp::Bijection,
        BinaryExprOp::SetMinus => BinaryOp::Difference,
        BinaryExprOp::CProd => BinaryOp::CartesianProduct,
        BinaryExprOp::DProd => BinaryOp::DirectProduct,
        BinaryExprOp::PProd => BinaryOp::ParallelProduct,
        BinaryExprOp::DomRes => BinaryOp::DomainRestriction,
        BinaryExprOp::DomSub => BinaryOp::DomainSubtraction,
        BinaryExprOp::RanRes => BinaryOp::RangeRestriction,
        BinaryExprOp::RanSub => BinaryOp::RangeSubtraction,
        BinaryExprOp::UpTo => BinaryOp::Range,
        BinaryExprOp::Minus => BinaryOp::Subtract,
        BinaryExprOp::Div => BinaryOp::Divide,
        BinaryExprOp::Mod => BinaryOp::Modulo,
        BinaryExprOp::Expn => BinaryOp::Exponent,
        BinaryExprOp::FunImage | BinaryExprOp::RelImage => {
            unreachable!("applications and images print structurally")
        }
    }
}

fn legacy_assoc(op: AssocExprOp) -> BinaryOp {
    match op {
        AssocExprOp::BUnion => BinaryOp::Union,
        AssocExprOp::BInter => BinaryOp::Intersection,
        AssocExprOp::BComp => BinaryOp::Composition,
        AssocExprOp::FComp => BinaryOp::Semicolon,
        AssocExprOp::Ovr => BinaryOp::Overwrite,
        AssocExprOp::Plus => BinaryOp::Add,
        AssocExprOp::Mul => BinaryOp::Multiply,
    }
}

fn legacy_comparison(op: RelationalOp) -> ComparisonOp {
    match op {
        RelationalOp::Equal => ComparisonOp::Equal,
        RelationalOp::NotEqual => ComparisonOp::NotEqual,
        RelationalOp::Lt => ComparisonOp::LessThan,
        RelationalOp::Le => ComparisonOp::LessEqual,
        RelationalOp::Gt => ComparisonOp::GreaterThan,
        RelationalOp::Ge => ComparisonOp::GreaterEqual,
        RelationalOp::In => ComparisonOp::In,
        RelationalOp::NotIn => ComparisonOp::NotIn,
        // The model names the strict operator `Subset` (⊂); the legacy
        // enum uses `Subset` for ⊆.
        RelationalOp::Subset => ComparisonOp::SubsetStrict,
        RelationalOp::NotSubset => ComparisonOp::NotSubsetStrict,
        RelationalOp::SubsetEq => ComparisonOp::Subset,
        RelationalOp::NotSubsetEq => ComparisonOp::NotSubset,
    }
}

fn legacy_logical(op: AssocPredOp) -> LogicalOp {
    match op {
        AssocPredOp::LAnd => LogicalOp::And,
        AssocPredOp::LOr => LogicalOp::Or,
    }
}

fn legacy_binary_pred(op: BinaryPredOp) -> LogicalOp {
    match op {
        BinaryPredOp::LImp => LogicalOp::Implies,
        BinaryPredOp::LEqv => LogicalOp::Equivalent,
    }
}

/// The legacy binary operator a node behaves as in precedence and
/// parenthesization decisions, if any.
fn effective_binary(kind: &FExprKind) -> Option<BinaryOp> {
    match kind {
        FExprKind::Binary {
            op: BinaryExprOp::FunImage | BinaryExprOp::RelImage,
            ..
        } => None,
        FExprKind::Binary { op, .. } => Some(legacy_binary(*op)),
        FExprKind::Associative { op, .. } => Some(legacy_assoc(*op)),
        FExprKind::Ascription { .. } => Some(BinaryOp::OfType),
        _ => None,
    }
}

/// Whether a formula-model expression must be parenthesized where only
/// a pair-expression (or lower) is grammatical: lambda and the
/// quantified unions/intersections sit above that level.
fn fm_above_pair(kind: &FExprKind) -> bool {
    match kind {
        FExprKind::Quantified { op, form, .. } => match op {
            QuantExprOp::QUnion | QuantExprOp::QInter => true,
            QuantExprOp::CSet => *form == Form::Lambda,
        },
        _ => false,
    }
}

/// Threaded, immutable state for width-aware rendering. Columns are char
/// counts (`.chars().count()`) — the unit the hanging-list layout already
/// uses; a tab counts as one.
struct WrapCtx {
    /// Configured maximum line width (always > 0 here).
    width: usize,
    /// Char width of the element's own leading indentation — the anchor
    /// for the pathological-hang cap.
    base_col: usize,
    /// Char width of one indent unit.
    indent_w: usize,
}

impl WrapCtx {
    /// A flat rendering fits when it ends at or before the width.
    fn fits(&self, col: usize, flat: &str) -> bool {
        col + flat.chars().count() <= self.width
    }

    /// Continuation column for a construct that started at `col`: hanging
    /// alignment, capped so a deep start cannot push every continuation
    /// past half the width.
    fn hang(&self, col: usize) -> usize {
        if col <= self.width / 2 {
            col
        } else {
            self.base_col + 2 * self.indent_w
        }
    }

    /// Column for a construct moved onto its own fresh line (quantifier
    /// bodies, an assignment's own-line right-hand side).
    fn nest(&self, col: usize) -> usize {
        self.hang(col) + self.indent_w
    }

    /// A context whose width is reduced by `n` columns, reserving room for
    /// the closing delimiter(s) the caller appends after the wrapped
    /// content's last line — without this, content packed exactly to the
    /// width would push its `)`/`]`/`}` past it.
    fn narrowed(&self, n: usize) -> WrapCtx {
        WrapCtx {
            width: self.width.saturating_sub(n),
            base_col: self.base_col,
            indent_w: self.indent_w,
        }
    }
}

/// A line break followed by `col` spaces of continuation indentation.
fn cont_line(col: usize) -> String {
    format!("\n{}", " ".repeat(col))
}

/// The column after `rendered` when it started at `col`. Continuation
/// lines carry absolute indentation, so a multi-line rendering ends at
/// its last line's own width.
fn end_col(col: usize, rendered: &str) -> usize {
    match rendered.rfind('\n') {
        Some(i) => rendered[i + 1..].chars().count(),
        None => col + rendered.chars().count(),
    }
}

impl PrettyPrinter {
    /// Convert a formula-model expression to text.
    pub fn print_formula_expression(&self, expr: &formula::Expression) -> String {
        let mut names = Vec::new();
        self.fm_expr(expr, FormulaContext::Expression, &mut names)
    }

    /// Convert a formula-model predicate to text.
    pub fn print_formula_predicate(&self, pred: &formula::Predicate) -> String {
        let mut names = Vec::new();
        self.fm_pred(pred, FormulaContext::Predicate, &mut names)
    }

    /// The display name of a bound occurrence; an index without an
    /// enclosing declaration renders as a visible placeholder.
    fn fm_bound_name(names: &[String], index: u32) -> String {
        match names.len().checked_sub(1 + index as usize) {
            Some(i) => names[i].clone(),
            None => format!("[[{index}]]"),
        }
    }

    /// Resolves the printing names of a binding construct's
    /// declarations: the names visible in its subtree (its free
    /// identifiers plus the enclosing declarations it references) must
    /// not be captured.
    fn fm_resolve_decls(
        &self,
        decls: &[formula::BoundIdentDecl],
        free: &[String],
        dangling: &[u32],
        names: &[String],
    ) -> Vec<String> {
        let mut solver = FreshNameSolver::new(free.iter().cloned());
        for index in dangling {
            if let Some(i) = names.len().checked_sub(1 + *index as usize) {
                solver.add(names[i].clone());
            }
        }
        resolve_idents(decls, &mut solver)
    }

    /// Prints a declaration list under its resolved names, with `⦂`
    /// annotations from the solved types (in typed mode) or the source
    /// spelling. Annotations are scoped to the enclosing context.
    fn fm_decls(
        &self,
        decls: &[formula::BoundIdentDecl],
        resolved: &[String],
        context: FormulaContext,
        names: &mut Vec<String>,
    ) -> String {
        decls
            .iter()
            .zip(resolved)
            .map(|(decl, name)| self.fm_decl(decl, name, context, names))
            .collect::<Vec<_>>()
            .join(self.comma_separator())
    }

    /// Renders one declaration as `name` or `name ⦂ annotation` — the
    /// one home of the annotated-declaration spelling (declaration
    /// lists and lambda pattern leaves).
    fn fm_decl(
        &self,
        decl: &formula::BoundIdentDecl,
        name: &str,
        context: FormulaContext,
        names: &mut Vec<String>,
    ) -> String {
        match self.fm_decl_annotation(decl, context, names) {
            Some(annotation) => format!("{}{}{}", name, self.oftype_annotation(), annotation),
            None => name.to_string(),
        }
    }

    fn fm_decl_annotation(
        &self,
        decl: &formula::BoundIdentDecl,
        context: FormulaContext,
        names: &mut Vec<String>,
    ) -> Option<String> {
        if self.formula_spacing == FormulaSpacing::RodinFormulaString {
            return None;
        }
        if self.typed_decls {
            if let Some(ty) = decl.ty() {
                let spelled = ty.to_expression(decl.factory());
                return Some(self.fm_expr(&spelled, context, &mut Vec::new()));
            }
        }
        decl.annotation()
            .map(|annotation| self.fm_expr(annotation, context, names))
    }

    fn fm_expr(
        &self,
        expr: &formula::Expression,
        context: FormulaContext,
        names: &mut Vec<String>,
    ) -> String {
        let expr = self.visible_expr(expr);
        match expr.kind() {
            FExprKind::FreeIdentifier(name) => name.clone(),
            FExprKind::BoundIdentifier(index) => Self::fm_bound_name(names, *index),
            FExprKind::IntegerLiteral(value) => {
                let rendered = value.to_string();
                if self.formula_spacing == FormulaSpacing::RodinFormulaString
                    && let Some(unsigned) = rendered.strip_prefix('-')
                {
                    return format!(
                        "{}{unsigned}",
                        self.op(operators::unary_op_id(UnaryOp::Minus))
                    );
                }
                rendered
            }
            FExprKind::Atomic(op) => match op {
                AtomicOp::Integer => self.op(OperatorId::Integers).to_string(),
                AtomicOp::Natural => self.op(OperatorId::Naturals).to_string(),
                AtomicOp::Natural1 => self.op(OperatorId::Naturals1).to_string(),
                AtomicOp::Bool => "BOOL".to_string(),
                AtomicOp::True => "TRUE".to_string(),
                AtomicOp::False => "FALSE".to_string(),
                AtomicOp::EmptySet => self.op(OperatorId::EmptySet).to_string(),
                AtomicOp::KPred => "pred".to_string(),
                AtomicOp::KSucc => "succ".to_string(),
                AtomicOp::KPrj1Gen => "prj1".to_string(),
                AtomicOp::KPrj2Gen => "prj2".to_string(),
                AtomicOp::KIdGen => "id".to_string(),
            },
            FExprKind::SetExtension(members) => {
                let elems: Vec<String> = members
                    .iter()
                    .map(|m| self.fm_expr(m, context, names))
                    .collect();
                format!("{{{}}}", elems.join(self.comma_separator()))
            }
            FExprKind::Bool(pred) => {
                format!("bool({})", self.fm_pred(pred, context, names))
            }
            FExprKind::Binary {
                op: op @ (BinaryExprOp::FunImage | BinaryExprOp::RelImage),
                left,
                right,
            } => {
                let left = self.visible_expr(left);
                let mut applied = self.fm_expr(left, context, names);
                if Self::fm_parens_for_image(left.kind()) {
                    applied = format!("({applied})");
                }
                let argument = self.fm_expr(right, context, names);
                if *op == BinaryExprOp::FunImage {
                    format!("{applied}({argument})")
                } else {
                    format!("{applied}[{argument}]")
                }
            }
            FExprKind::Binary { op, left, right } => {
                let old = legacy_binary(*op);
                self.fm_binary(old, left, right, context, names)
            }
            FExprKind::Ascription { expr, type_expr } => {
                self.fm_binary(BinaryOp::OfType, expr, type_expr, context, names)
            }
            FExprKind::Associative { op, children } => {
                let old = legacy_assoc(*op);
                let op_str = self.op(operators::binary_op_id(old));
                let separator = self.binary_separator(old, context);
                let joint = format!("{separator}{op_str}{separator}");
                children
                    .iter()
                    .enumerate()
                    .map(|(i, child)| self.fm_child_expr(child, old, i > 0, context, names))
                    .collect::<Vec<_>>()
                    .join(&joint)
            }
            FExprKind::Unary { op, child } => match self.unary_head(*op) {
                Some(head) => format!("{head}({})", self.fm_expr(child, context, names)),
                None => {
                    let child = self.visible_expr(child);
                    let needs_parens = match child.kind() {
                        FExprKind::FreeIdentifier(_)
                        | FExprKind::BoundIdentifier(_)
                        | FExprKind::Atomic(_)
                        | FExprKind::IntegerLiteral(_)
                        | FExprKind::Unary {
                            op:
                                UnaryExprOp::KCard
                                | UnaryExprOp::KMin
                                | UnaryExprOp::KMax
                                | UnaryExprOp::KUnion
                                | UnaryExprOp::KInter
                                | UnaryExprOp::Converse,
                            ..
                        } => false,
                        FExprKind::Binary {
                            op: BinaryExprOp::FunImage | BinaryExprOp::RelImage,
                            ..
                        } => self.formula_spacing == FormulaSpacing::RodinFormulaString,
                        _ => true,
                    };
                    let operand = self.fm_expr(child, context, names);
                    let op_str = self.op(operators::unary_op_id(UnaryOp::Inverse));
                    if needs_parens {
                        format!("({operand}){op_str}")
                    } else {
                        format!("{operand}{op_str}")
                    }
                }
            },
            FExprKind::Quantified {
                op,
                decls,
                pred,
                expr: value,
                form,
            } => {
                let resolved = self.fm_resolve_decls(
                    decls,
                    expr.free_identifiers(),
                    expr.dangling_bound_indices(),
                    names,
                );
                let mid = self.op(OperatorId::Dot);
                let bar = if self.formula_spacing == FormulaSpacing::RodinFormulaString {
                    format!(" {} ", self.op(OperatorId::Bar))
                } else {
                    self.op(OperatorId::Bar).to_string()
                };
                // The short comprehension spellings have no place to
                // carry the declarations' types, so typed printing
                // escalates them to the explicit form (the lambda
                // spelling annotates its pattern leaves instead).
                let form = if self.typed_decls && matches!(form, Form::Implicit | Form::IdentList) {
                    &Form::Explicit
                } else {
                    form
                };
                match op {
                    QuantExprOp::CSet => match form {
                        Form::Lambda => {
                            let value = self.visible_expr(value);
                            let FExprKind::Binary {
                                op: BinaryExprOp::Mapsto,
                                left: pattern,
                                right: body,
                            } = value.kind()
                            else {
                                unreachable!("lambda form implies a maplet expression")
                            };
                            let lambda = self.op(OperatorId::Lambda);
                            let pattern_str =
                                self.fm_lambda_pattern(pattern, decls, &resolved, context, names);
                            names.extend(resolved.iter().cloned());
                            let pred_str = self.fm_pred(pred, context, names);
                            let body_str = self.fm_expr(body, context, names);
                            names.truncate(names.len() - decls.len());
                            format!("{lambda} {pattern_str}{mid}{pred_str}{bar}{body_str}")
                        }
                        Form::Implicit => {
                            names.extend(resolved.iter().cloned());
                            let value_str = self.fm_expr(value, context, names);
                            let pred_str = self.fm_pred(pred, context, names);
                            names.truncate(names.len() - decls.len());
                            format!("{{{value_str}{bar}{pred_str}}}")
                        }
                        Form::IdentList => {
                            let ids = self.fm_decls(decls, &resolved, context, names);
                            names.extend(resolved.iter().cloned());
                            let pred_str = self.fm_pred(pred, context, names);
                            names.truncate(names.len() - decls.len());
                            format!("{{{ids}{bar}{pred_str}}}")
                        }
                        Form::Explicit => {
                            let ids = self.fm_decls(decls, &resolved, context, names);
                            names.extend(resolved.iter().cloned());
                            let pred_str = self.fm_pred(pred, context, names);
                            let value_str = self.fm_expr(value, context, names);
                            names.truncate(names.len() - decls.len());
                            format!("{{{ids}{mid}{pred_str}{bar}{value_str}}}")
                        }
                    },
                    QuantExprOp::QUnion | QuantExprOp::QInter => {
                        // Only the explicit spelling exists in the
                        // grammar for these.
                        let keyword = self.op(match op {
                            QuantExprOp::QUnion => OperatorId::QuantifiedUnion,
                            QuantExprOp::QInter => OperatorId::QuantifiedIntersection,
                            QuantExprOp::CSet => unreachable!(),
                        });
                        let ids = self.fm_decls(decls, &resolved, context, names);
                        names.extend(resolved.iter().cloned());
                        let pred_str = self.fm_pred(pred, context, names);
                        let value_str = self.fm_expr(value, context, names);
                        names.truncate(names.len() - decls.len());
                        format!("{keyword} {ids}{mid}{pred_str}{bar}{value_str}")
                    }
                }
            }
            FExprKind::Extended { tag, exprs, preds } => {
                self.fm_extended(expr.factory(), *tag, exprs, preds, context, names)
            }
        }
    }

    /// The prefix head spelling of a unary operator (`card`, `dom`, `ℙ`,
    /// …); `None` for the postfix converse. Shared by the flat renderer
    /// and the wrap layer so the spellings cannot drift.
    fn unary_head(&self, op: UnaryExprOp) -> Option<&'static str> {
        Some(match op {
            UnaryExprOp::KCard => "card",
            UnaryExprOp::KMin => "min",
            UnaryExprOp::KMax => "max",
            UnaryExprOp::KUnion => "union",
            UnaryExprOp::KInter => "inter",
            UnaryExprOp::Pow => self.op(operators::unary_op_id(UnaryOp::PowerSet)),
            UnaryExprOp::Pow1 => self.op(operators::unary_op_id(UnaryOp::PowerSet1)),
            UnaryExprOp::KDom => self.op(operators::unary_op_id(UnaryOp::Domain)),
            UnaryExprOp::KRan => self.op(operators::unary_op_id(UnaryOp::Range)),
            UnaryExprOp::UnMinus => self.op(operators::unary_op_id(UnaryOp::Minus)),
            UnaryExprOp::Converse => return None,
        })
    }

    /// The expression visible in formula-string mode, where source type
    /// ascriptions are presentation-only.
    fn visible_expr<'a>(&self, mut expr: &'a formula::Expression) -> &'a formula::Expression {
        if self.formula_spacing == FormulaSpacing::RodinFormulaString {
            while let FExprKind::Ascription { expr: inner, .. } = expr.kind() {
                expr = inner;
            }
        }
        expr
    }

    fn fm_binary(
        &self,
        old: BinaryOp,
        left: &formula::Expression,
        right: &formula::Expression,
        context: FormulaContext,
        names: &mut Vec<String>,
    ) -> String {
        let op_str = self.op(operators::binary_op_id(old));
        let separator = self.binary_separator(old, context);
        let left_str = self.fm_child_expr(left, old, false, context, names);
        let right_str = self.fm_child_expr(right, old, true, context, names);
        format!("{left_str}{separator}{op_str}{separator}{right_str}")
    }

    /// Mirror of the legacy child-parenthesization rules.
    fn fm_child_expr(
        &self,
        child: &formula::Expression,
        parent_op: BinaryOp,
        is_right: bool,
        context: FormulaContext,
        names: &mut Vec<String>,
    ) -> String {
        if self.fm_expr_child_needs_parens(child, parent_op, is_right) {
            format!("({})", self.fm_expr(child, context, names))
        } else {
            self.fm_expr(child, context, names)
        }
    }

    /// The parenthesization decision of [`Self::fm_child_expr`], standalone
    /// so the wrap layer can learn whether an operand carries a `(` prefix
    /// before rendering it.
    fn fm_expr_child_needs_parens(
        &self,
        child: &formula::Expression,
        parent_op: BinaryOp,
        is_right: bool,
    ) -> bool {
        let child = self.visible_expr(child);
        if fm_above_pair(child.kind()) {
            return true;
        }
        if matches!(
            child.kind(),
            FExprKind::Unary {
                op: UnaryExprOp::UnMinus,
                ..
            }
        ) || matches!(
            child.kind(),
            FExprKind::IntegerLiteral(value) if value.sign() == num_bigint::Sign::Minus
        ) {
            // Unary minus parses at additive precedence: printed bare
            // under a tighter arithmetic operator it would swallow the
            // rest of the term on re-parsing. A negative integer literal
            // prints as the same bare `−N`, so it needs the same
            // parentheses: `(−2)^n` is written, never `−2^n`.
            return matches!(
                parent_op,
                BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Modulo | BinaryOp::Exponent
            );
        }
        let Some(child_op) = effective_binary(child.kind()) else {
            return false;
        };
        let child_prec = op_info::binary_precedence(child_op);
        let parent_prec = op_info::binary_precedence(parent_op);
        if child_prec != parent_prec {
            return child_prec < parent_prec;
        }
        if self.formula_spacing == FormulaSpacing::RodinFormulaString
            && !is_right
            && child_op == BinaryOp::DomainRestriction
            && parent_op == BinaryOp::RangeRestriction
        {
            return false;
        }
        !op_info::binary_ops_compatible(child_op, parent_op)
            || op_info::is_non_associative(parent_op)
            || is_right
    }

    /// Mirror of the relational-image / application head rule: binary
    /// and prefix-unary operands bind looser than `f(x)` / `r[S]`.
    fn fm_parens_for_image(kind: &FExprKind) -> bool {
        match kind {
            FExprKind::Unary {
                op: UnaryExprOp::Converse,
                ..
            } => false,
            FExprKind::Unary {
                op:
                    UnaryExprOp::KCard
                    | UnaryExprOp::KMin
                    | UnaryExprOp::KMax
                    | UnaryExprOp::KUnion
                    | UnaryExprOp::KInter,
                ..
            } => false,
            FExprKind::Binary {
                op: BinaryExprOp::FunImage | BinaryExprOp::RelImage,
                ..
            } => false,
            FExprKind::Binary { .. }
            | FExprKind::Associative { .. }
            | FExprKind::Ascription { .. }
            | FExprKind::Unary { .. } => true,
            // The binder forms follow the pair-level rule.
            kind @ FExprKind::Quantified { .. } => fm_above_pair(kind),
            _ => false,
        }
    }

    /// Print an expression where only a pair-expression is grammatical.
    fn fm_pair(
        &self,
        expr: &formula::Expression,
        context: FormulaContext,
        names: &mut Vec<String>,
    ) -> String {
        let expr = self.visible_expr(expr);
        if fm_above_pair(expr.kind()) {
            format!("({})", self.fm_expr(expr, context, names))
        } else {
            self.fm_expr(expr, context, names)
        }
    }

    /// Renders a lambda pattern from its maplet tree: leaves are the
    /// construct's own declarations.
    fn fm_lambda_pattern(
        &self,
        pattern: &formula::Expression,
        decls: &[formula::BoundIdentDecl],
        resolved: &[String],
        context: FormulaContext,
        names: &mut Vec<String>,
    ) -> String {
        let pattern = self.visible_expr(pattern);
        match pattern.kind() {
            FExprKind::BoundIdentifier(index) => {
                let position = decls.len() - 1 - *index as usize;
                self.fm_decl(&decls[position], &resolved[position], context, names)
            }
            FExprKind::Binary {
                op: BinaryExprOp::Mapsto,
                left,
                right,
            } => {
                let maplet = self.op(OperatorId::Maplet);
                let left_str = self.fm_lambda_pattern(left, decls, resolved, context, names);
                let right = self.visible_expr(right);
                let right_str = match right.kind() {
                    FExprKind::Binary {
                        op: BinaryExprOp::Mapsto,
                        ..
                    } => format!(
                        "({})",
                        self.fm_lambda_pattern(right, decls, resolved, context, names)
                    ),
                    _ => self.fm_lambda_pattern(right, decls, resolved, context, names),
                };
                format!("{left_str} {maplet} {right_str}")
            }
            _ => unreachable!("lambda patterns are maplet trees over declarations"),
        }
    }

    fn fm_pred(
        &self,
        pred: &formula::Predicate,
        context: FormulaContext,
        names: &mut Vec<String>,
    ) -> String {
        match pred.kind() {
            FPredKind::Literal(op) => match op {
                LiteralPredOp::BTrue => self.sym("⊤", "true").to_string(),
                LiteralPredOp::BFalse => self.sym("⊥", "false").to_string(),
            },
            FPredKind::PredicateVariable(name) => name.clone(),
            FPredKind::Relational { op, left, right } => {
                let old = legacy_comparison(*op);
                let op_str = self.op(operators::comparison_op_id(old));
                let separator = self.tight_operator_separator(operators::comparison_op_id(old));
                let left_str = self.fm_pair(left, context, names);
                let right_str = self.fm_pair(right, context, names);
                format!("{left_str}{separator}{op_str}{separator}{right_str}")
            }
            FPredKind::Not(child) => {
                let not = self.op(OperatorId::Not);
                let rendered = self.fm_pred(child, context, names);
                if self.formula_spacing == FormulaSpacing::RodinFormulaString
                    && !matches!(
                        child.kind(),
                        FPredKind::Binary { .. }
                            | FPredKind::Associative { .. }
                            | FPredKind::Quantified { .. }
                    )
                {
                    format!("{not}{rendered}")
                } else {
                    format!("{not}({rendered})")
                }
            }
            FPredKind::Binary { op, left, right } => {
                let old = legacy_binary_pred(*op);
                let op_str = self.op(operators::logical_op_id(old));
                let separator = self.tight_operator_separator(operators::logical_op_id(old));
                let left_str = self.fm_pred_child(left, old, false, context, names);
                let right_str = self.fm_pred_child(right, old, true, context, names);
                format!("{left_str}{separator}{op_str}{separator}{right_str}")
            }
            FPredKind::Associative { op, children } => {
                let old = legacy_logical(*op);
                let op_str = self.op(operators::logical_op_id(old));
                let separator = self.tight_operator_separator(operators::logical_op_id(old));
                let joint = format!("{separator}{op_str}{separator}");
                children
                    .iter()
                    .enumerate()
                    .map(|(i, child)| self.fm_pred_child(child, old, i > 0, context, names))
                    .collect::<Vec<_>>()
                    .join(&joint)
            }
            FPredKind::Quantified {
                op,
                decls,
                pred: body,
            } => {
                let resolved = self.fm_resolve_decls(
                    decls,
                    pred.free_identifiers(),
                    pred.dangling_bound_indices(),
                    names,
                );
                let quantifier = self.op(match op {
                    QuantPredOp::Forall => operators::quantifier_id(Quantifier::ForAll),
                    QuantPredOp::Exists => operators::quantifier_id(Quantifier::Exists),
                });
                let mid = self.op(OperatorId::Dot);
                let ids = self.fm_decls(decls, &resolved, context, names);
                names.extend(resolved.iter().cloned());
                let body_str = self.fm_pred(body, context, names);
                names.truncate(names.len() - decls.len());
                format!("{quantifier}{ids}{mid}{body_str}")
            }
            FPredKind::Simple(child) => {
                format!("finite({})", self.fm_expr(child, context, names))
            }
            FPredKind::Multiple(children) => {
                let args: Vec<String> = children
                    .iter()
                    .map(|c| self.fm_expr(c, context, names))
                    .collect();
                format!("partition({})", args.join(self.comma_separator()))
            }
            FPredKind::Application { function, args, .. } => {
                let rendered: Vec<String> = args
                    .iter()
                    .map(|a| self.fm_expr(a, context, names))
                    .collect();
                format!("{}({})", function, rendered.join(self.comma_separator()))
            }
            FPredKind::Extended { tag, exprs, preds } => {
                self.fm_extended(pred.factory(), *tag, exprs, preds, context, names)
            }
        }
    }

    /// Mirror of the legacy logical-connective parenthesization rules.
    fn fm_pred_child(
        &self,
        child: &formula::Predicate,
        parent_op: LogicalOp,
        is_right: bool,
        context: FormulaContext,
        names: &mut Vec<String>,
    ) -> String {
        if self.fm_pred_child_needs_parens(child, parent_op, is_right) {
            format!("({})", self.fm_pred(child, context, names))
        } else {
            self.fm_pred(child, context, names)
        }
    }

    /// The parenthesization decision of [`Self::fm_pred_child`], standalone
    /// so the wrap layer can learn whether an operand carries a `(` prefix
    /// before rendering it.
    fn fm_pred_child_needs_parens(
        &self,
        child: &formula::Predicate,
        parent_op: LogicalOp,
        is_right: bool,
    ) -> bool {
        let child_op = match child.kind() {
            FPredKind::Quantified { .. } => return true,
            FPredKind::Associative { op, .. } => Some(legacy_logical(*op)),
            FPredKind::Binary { op, .. } => Some(legacy_binary_pred(*op)),
            _ => None,
        };
        match child_op {
            Some(child_op) => {
                let child_prec = op_info::logical_precedence(child_op);
                let parent_prec = op_info::logical_precedence(parent_op);
                if child_prec < parent_prec {
                    true
                } else if child_prec > parent_prec {
                    false
                } else {
                    let child_class = op_info::logical_compat_class(child_op);
                    let parent_class = op_info::logical_compat_class(parent_op);
                    child_class == 0 || parent_class == 0 || child_class != parent_class || is_right
                }
            }
            None => false,
        }
    }

    fn fm_extended(
        &self,
        factory: &formula::FormulaFactory,
        tag: crate::formula::tag::Tag,
        exprs: &[formula::Expression],
        preds: &[formula::Predicate],
        context: FormulaContext,
        names: &mut Vec<String>,
    ) -> String {
        let symbol = factory
            .extension(tag)
            .map(|ext| ext.common().symbol().to_string())
            .unwrap_or_else(|| format!("[[ext:{tag}]]"));
        let mut args: Vec<String> = exprs
            .iter()
            .map(|e| self.fm_expr(e, context, names))
            .collect();
        args.extend(preds.iter().map(|p| self.fm_pred(p, context, names)));
        if args.is_empty() {
            symbol
        } else {
            format!("{}({})", symbol, args.join(self.comma_separator()))
        }
    }

    /// Convert a formula-model assignment to text.
    /// The comma-joined left-hand side of an assignment.
    fn assignment_targets(&self, idents: &[formula::Expression]) -> String {
        idents
            .iter()
            .map(|ident| match ident.kind() {
                FExprKind::FreeIdentifier(name) => name.clone(),
                _ => unreachable!("assignment targets are free identifiers"),
            })
            .collect::<Vec<_>>()
            .join(self.comma_separator())
    }

    pub fn print_formula_assignment(&self, assign: &formula::Assignment) -> String {
        use formula::AssignmentKind as K;
        let context = FormulaContext::Action;
        let mut names = Vec::new();
        let targets = |idents: &[formula::Expression]| self.assignment_targets(idents);
        match assign.kind() {
            K::BecomesEqualTo { idents, values } => {
                let rendered: Vec<String> = values
                    .iter()
                    .map(|value| Self::guard_action_part(self.fm_expr(value, context, &mut names)))
                    .collect();
                format!(
                    "{} {} {}",
                    targets(idents),
                    self.op(OperatorId::Assignment),
                    rendered.join(self.comma_separator())
                )
            }
            K::BecomesMemberOf { idents, set } => {
                format!(
                    "{} {} {}",
                    targets(idents),
                    self.op(OperatorId::BecomesIn),
                    Self::guard_action_part(self.fm_expr(set, context, &mut names))
                )
            }
            K::BecomesSuchThat {
                idents,
                primed,
                pred,
            } => {
                let resolved = self.fm_resolve_decls(
                    primed,
                    assign.free_identifiers(),
                    assign.dangling_bound_indices(),
                    &names,
                );
                names.extend(resolved);
                let condition = Self::guard_action_part(self.fm_pred(pred, context, &mut names));
                format!(
                    "{} {} {}",
                    targets(idents),
                    self.op(OperatorId::BecomesSuchThat),
                    condition
                )
            }
        }
    }

    // ===== width-aware wrapping ==========================================
    //
    // Flat-first, AST-driven: every wrap function first renders its node
    // with the flat `fm_*` renderer and keeps that when it fits; on
    // overflow it splits the node's own structure — associative/binary
    // chains break before each operator (operator-leading continuations,
    // the only universally reparse-safe break), argument lists break after
    // commas inside their brackets, quantifiers break after `·` — and
    // recurses into operands that still overflow. Operand parenthesization
    // reuses the same `fm_*_needs_parens` decisions the flat renderers
    // apply, so wrapping can never change a formula's structure, and
    // recursion always descends into strictly smaller children, so it
    // terminates at any width. Labeled-element printing and the one
    // deliberate wrapped entry point, `print_formula_predicate_wrapped`,
    // call in here; the rest of the public `print_formula_*` API stays
    // unconditionally flat, keeping Rodin-canonical strings and XML
    // attributes newline-free.

    fn wrap_ctx(&self, base_col: usize) -> WrapCtx {
        debug_assert!(
            self.formula_spacing == FormulaSpacing::Readable,
            "wrapping is a readable-mode feature; canonical output must stay flat"
        );
        WrapCtx {
            width: self.max_line_width,
            base_col,
            indent_w: self.indent.chars().count().max(1),
        }
    }

    /// Render a predicate starting at column `start_col` (the width of
    /// everything already on its first line), wrapping at the configured
    /// width; `base_col` is the element's own indentation width. Flat when
    /// wrapping is off.
    fn print_predicate_at(
        &self,
        pred: &formula::Predicate,
        start_col: usize,
        base_col: usize,
    ) -> String {
        if self.max_line_width == 0 {
            return self.print_formula_predicate(pred);
        }
        let wc = self.wrap_ctx(base_col);
        let mut names = Vec::new();
        self.wrap_pred(pred, start_col, &wc, FormulaContext::Predicate, &mut names)
    }

    /// Convert a formula-model predicate to text starting at column 0,
    /// wrapped at `max_line_width`. Flat — identical to
    /// [`Self::print_formula_predicate`] — when wrapping is off or the
    /// printer is a canonical one, whose output (Rodin-canonical strings,
    /// XML attributes) must stay newline-free.
    pub fn print_formula_predicate_wrapped(&self, pred: &formula::Predicate) -> String {
        if self.formula_spacing != FormulaSpacing::Readable {
            return self.print_formula_predicate(pred);
        }
        self.print_predicate_at(pred, 0, 0)
    }

    /// [`Self::print_predicate_at`] for expressions.
    fn print_expression_at(
        &self,
        expr: &formula::Expression,
        start_col: usize,
        base_col: usize,
    ) -> String {
        if self.max_line_width == 0 {
            return self.print_formula_expression(expr);
        }
        let wc = self.wrap_ctx(base_col);
        let mut names = Vec::new();
        self.wrap_expr(expr, start_col, &wc, FormulaContext::Expression, &mut names)
    }

    /// [`Self::print_predicate_at`] for action bodies.
    fn print_action_at(
        &self,
        body: &crate::ast::ActionBody,
        start_col: usize,
        base_col: usize,
    ) -> String {
        match body {
            crate::ast::ActionBody::Skip { .. } => "skip".to_string(),
            crate::ast::ActionBody::Assignment(assign) => {
                if self.max_line_width == 0 {
                    return self.print_formula_assignment(assign);
                }
                let wc = self.wrap_ctx(base_col);
                self.wrap_assignment(assign, start_col, &wc)
            }
        }
    }

    fn wrap_pred(
        &self,
        pred: &formula::Predicate,
        col: usize,
        wc: &WrapCtx,
        context: FormulaContext,
        names: &mut Vec<String>,
    ) -> String {
        let flat = self.fm_pred(pred, context, names);
        if wc.fits(col, &flat) {
            return flat;
        }
        self.wrap_pred_overflow(pred, flat, col, wc, context, names)
    }

    /// Operator-leading chain of logical operands: the first at `col`,
    /// each following operand on a continuation line after the operator.
    #[allow(clippy::too_many_arguments)]
    fn wrap_pred_chain(
        &self,
        operands: &[&formula::Predicate],
        old: LogicalOp,
        col: usize,
        wc: &WrapCtx,
        context: FormulaContext,
        names: &mut Vec<String>,
    ) -> String {
        let op_str = self.op(operators::logical_op_id(old));
        let cont = wc.hang(col);
        let item_col = cont + op_str.chars().count() + 1;
        let mut out = self.wrap_pred_operand(operands[0], old, false, col, wc, context, names);
        for child in &operands[1..] {
            out.push_str(&cont_line(cont));
            out.push_str(op_str);
            out.push(' ');
            out.push_str(&self.wrap_pred_operand(child, old, true, item_col, wc, context, names));
        }
        out
    }

    /// Split `pred`, already flat-rendered as `flat` and known not to fit
    /// at `col` — a caller that has probed the flat form skips
    /// re-rendering it.
    #[allow(clippy::too_many_arguments)]
    fn wrap_pred_overflow(
        &self,
        pred: &formula::Predicate,
        flat: String,
        col: usize,
        wc: &WrapCtx,
        context: FormulaContext,
        names: &mut Vec<String>,
    ) -> String {
        match pred.kind() {
            FPredKind::Associative { op, children } => {
                let operands: Vec<&formula::Predicate> = children.iter().collect();
                self.wrap_pred_chain(&operands, legacy_logical(*op), col, wc, context, names)
            }
            FPredKind::Binary { op, left, right } => self.wrap_pred_chain(
                &[left, right],
                legacy_binary_pred(*op),
                col,
                wc,
                context,
                names,
            ),
            FPredKind::Relational { op, left, right } => {
                let old = legacy_comparison(*op);
                let op_str = self.op(operators::comparison_op_id(old));
                let cont = wc.hang(col);
                let item_col = cont + op_str.chars().count() + 1;
                let mut out = self.wrap_pair(left, col, wc, context, names);
                out.push_str(&cont_line(cont));
                out.push_str(op_str);
                out.push(' ');
                out.push_str(&self.wrap_pair(right, item_col, wc, context, names));
                out
            }
            FPredKind::Not(child) => {
                // Readable mode always parenthesizes the operand.
                let not = self.op(OperatorId::Not);
                let inner_col = col + not.chars().count() + 1;
                format!(
                    "{not}({})",
                    self.wrap_pred(child, inner_col, &wc.narrowed(1), context, names)
                )
            }
            FPredKind::Quantified {
                op,
                decls,
                pred: body,
            } => {
                let resolved = self.fm_resolve_decls(
                    decls,
                    pred.free_identifiers(),
                    pred.dangling_bound_indices(),
                    names,
                );
                let quantifier = self.op(match op {
                    QuantPredOp::Forall => operators::quantifier_id(Quantifier::ForAll),
                    QuantPredOp::Exists => operators::quantifier_id(Quantifier::Exists),
                });
                let mid = self.op(OperatorId::Dot);
                let ids = {
                    let parts: Vec<String> = decls
                        .iter()
                        .zip(&resolved)
                        .map(|(decl, name)| self.fm_decl(decl, name, context, names))
                        .collect();
                    // The `·` after the declarations shares their last line.
                    self.fill_parts(&parts, col + quantifier.chars().count(), &wc.narrowed(1))
                };
                names.extend(resolved.iter().cloned());
                let body_col = wc.nest(col);
                let body_str = self.wrap_pred(body, body_col, wc, context, names);
                names.truncate(names.len() - decls.len());
                format!("{quantifier}{ids}{mid}{}{body_str}", cont_line(body_col))
            }
            FPredKind::Simple(child) => {
                let inner_col = col + "finite(".chars().count();
                format!(
                    "finite({})",
                    self.wrap_expr(child, inner_col, &wc.narrowed(1), context, names)
                )
            }
            FPredKind::Multiple(children) => {
                let inner_col = col + "partition(".chars().count();
                format!(
                    "partition({})",
                    self.wrap_expr_list(
                        children,
                        inner_col,
                        &wc.narrowed(1),
                        context,
                        names,
                        false
                    )
                )
            }
            FPredKind::Application { function, args, .. } => {
                let inner_col = col + function.chars().count() + 1;
                format!(
                    "{function}({})",
                    self.wrap_expr_list(args, inner_col, &wc.narrowed(1), context, names, false)
                )
            }
            // Atoms and extension applications stay flat (best-effort).
            FPredKind::Literal(_)
            | FPredKind::PredicateVariable(_)
            | FPredKind::Extended { .. } => flat,
        }
    }

    /// One operand of a logical chain: flat when it fits; a still-long
    /// operand that needs parentheses re-enters one column past its `(`.
    #[allow(clippy::too_many_arguments)]
    fn wrap_pred_operand(
        &self,
        child: &formula::Predicate,
        parent_op: LogicalOp,
        is_right: bool,
        col: usize,
        wc: &WrapCtx,
        context: FormulaContext,
        names: &mut Vec<String>,
    ) -> String {
        let flat = self.fm_pred_child(child, parent_op, is_right, context, names);
        if wc.fits(col, &flat) {
            return flat;
        }
        if self.fm_pred_child_needs_parens(child, parent_op, is_right) {
            format!(
                "({})",
                self.wrap_pred(child, col + 1, &wc.narrowed(1), context, names)
            )
        } else {
            // Without parens the probe just rendered is exactly `fm_pred`.
            self.wrap_pred_overflow(child, flat, col, wc, context, names)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn wrap_expr_operand(
        &self,
        child: &formula::Expression,
        parent_op: BinaryOp,
        is_right: bool,
        col: usize,
        wc: &WrapCtx,
        context: FormulaContext,
        names: &mut Vec<String>,
    ) -> String {
        let flat = self.fm_child_expr(child, parent_op, is_right, context, names);
        if wc.fits(col, &flat) {
            return flat;
        }
        if self.fm_expr_child_needs_parens(child, parent_op, is_right) {
            format!(
                "({})",
                self.wrap_expr(child, col + 1, &wc.narrowed(1), context, names)
            )
        } else {
            // Without parens the probe just rendered is exactly `fm_expr`.
            self.wrap_expr_overflow(child, flat, col, wc, context, names)
        }
    }

    /// A relational operand (pair level), mirroring [`Self::fm_pair`].
    fn wrap_pair(
        &self,
        expr: &formula::Expression,
        col: usize,
        wc: &WrapCtx,
        context: FormulaContext,
        names: &mut Vec<String>,
    ) -> String {
        let flat = self.fm_pair(expr, context, names);
        if wc.fits(col, &flat) {
            return flat;
        }
        if fm_above_pair(self.visible_expr(expr).kind()) {
            format!(
                "({})",
                self.wrap_expr(expr, col + 1, &wc.narrowed(1), context, names)
            )
        } else {
            // Without parens the probe just rendered is exactly `fm_expr`.
            self.wrap_expr_overflow(expr, flat, col, wc, context, names)
        }
    }

    fn wrap_expr(
        &self,
        expr: &formula::Expression,
        col: usize,
        wc: &WrapCtx,
        context: FormulaContext,
        names: &mut Vec<String>,
    ) -> String {
        let flat = self.fm_expr(expr, context, names);
        if wc.fits(col, &flat) {
            return flat;
        }
        self.wrap_expr_overflow(expr, flat, col, wc, context, names)
    }

    /// [`Self::wrap_pred_overflow`] for expressions.
    #[allow(clippy::too_many_arguments)]
    fn wrap_expr_overflow(
        &self,
        expr: &formula::Expression,
        flat: String,
        col: usize,
        wc: &WrapCtx,
        context: FormulaContext,
        names: &mut Vec<String>,
    ) -> String {
        let expr = self.visible_expr(expr);
        match expr.kind() {
            FExprKind::Associative { op, children } => {
                let operands: Vec<&formula::Expression> = children.iter().collect();
                self.wrap_expr_chain(&operands, legacy_assoc(*op), col, wc, context, names)
            }
            FExprKind::Binary {
                op: op @ (BinaryExprOp::FunImage | BinaryExprOp::RelImage),
                left,
                right,
            } => {
                let left = self.visible_expr(left);
                let mut applied = self.fm_expr(left, context, names);
                if Self::fm_parens_for_image(left.kind()) {
                    applied = format!("({applied})");
                }
                let inner_col = col + applied.chars().count() + 1;
                let inner = self.wrap_expr(right, inner_col, &wc.narrowed(1), context, names);
                if *op == BinaryExprOp::FunImage {
                    format!("{applied}({inner})")
                } else {
                    format!("{applied}[{inner}]")
                }
            }
            FExprKind::Binary { op, left, right } => {
                self.wrap_expr_chain(&[left, right], legacy_binary(*op), col, wc, context, names)
            }
            FExprKind::Ascription {
                expr: inner,
                type_expr,
            } => self.wrap_expr_chain(
                &[inner, type_expr],
                BinaryOp::OfType,
                col,
                wc,
                context,
                names,
            ),
            FExprKind::SetExtension(members) => {
                format!(
                    "{{{}}}",
                    self.wrap_expr_list(members, col + 1, &wc.narrowed(1), context, names, false)
                )
            }
            FExprKind::Bool(pred) => {
                let inner_col = col + "bool(".chars().count();
                format!(
                    "bool({})",
                    self.wrap_pred(pred, inner_col, &wc.narrowed(1), context, names)
                )
            }
            FExprKind::Unary { op, child } => match self.unary_head(*op) {
                // Postfix converse stays flat (best-effort; rare).
                None => flat,
                Some(head) => {
                    let inner_col = col + head.chars().count() + 1;
                    format!(
                        "{head}({})",
                        self.wrap_expr(child, inner_col, &wc.narrowed(1), context, names)
                    )
                }
            },
            FExprKind::Quantified { .. } => {
                self.wrap_quantified_expr(expr, col, wc, context, names)
            }
            // Leaves and extension applications stay flat (best-effort).
            _ => flat,
        }
    }

    /// [`Self::wrap_pred_chain`] for expression operands.
    #[allow(clippy::too_many_arguments)]
    fn wrap_expr_chain(
        &self,
        operands: &[&formula::Expression],
        old: BinaryOp,
        col: usize,
        wc: &WrapCtx,
        context: FormulaContext,
        names: &mut Vec<String>,
    ) -> String {
        let op_str = self.op(operators::binary_op_id(old));
        let cont = wc.hang(col);
        let item_col = cont + op_str.chars().count() + 1;
        let mut out = self.wrap_expr_operand(operands[0], old, false, col, wc, context, names);
        for child in &operands[1..] {
            out.push_str(&cont_line(cont));
            out.push_str(op_str);
            out.push(' ');
            out.push_str(&self.wrap_expr_operand(child, old, true, item_col, wc, context, names));
        }
        out
    }

    /// A quantified expression, breaking after `·` and before `∣`
    /// (bar-leading, like the operator-leading chain style). Mirrors the
    /// flat `fm_expr` arm's name threading exactly.
    fn wrap_quantified_expr(
        &self,
        expr: &formula::Expression,
        col: usize,
        wc: &WrapCtx,
        context: FormulaContext,
        names: &mut Vec<String>,
    ) -> String {
        let FExprKind::Quantified {
            op,
            decls,
            pred,
            expr: value,
            form,
        } = expr.kind()
        else {
            unreachable!("wrap_quantified_expr takes quantified expressions")
        };
        let resolved = self.fm_resolve_decls(
            decls,
            expr.free_identifiers(),
            expr.dangling_bound_indices(),
            names,
        );
        let mid = self.op(OperatorId::Dot);
        let bar = self.op(OperatorId::Bar);
        let form = if self.typed_decls && matches!(form, Form::Implicit | Form::IdentList) {
            &Form::Explicit
        } else {
            form
        };
        match op {
            QuantExprOp::CSet => {
                let inner = wc.hang(col + 1);
                match form {
                    Form::Lambda => {
                        let value = self.visible_expr(value);
                        let FExprKind::Binary {
                            op: BinaryExprOp::Mapsto,
                            left: pattern,
                            right: body,
                        } = value.kind()
                        else {
                            unreachable!("lambda form implies a maplet expression")
                        };
                        let lambda = self.op(OperatorId::Lambda);
                        let pattern_str =
                            self.fm_lambda_pattern(pattern, decls, &resolved, context, names);
                        names.extend(resolved.iter().cloned());
                        let body_col = wc.nest(col);
                        let pred_str = self.wrap_pred(pred, body_col, wc, context, names);
                        let value_col = body_col + bar.chars().count() + 1;
                        let body_str = self.wrap_expr(body, value_col, wc, context, names);
                        names.truncate(names.len() - decls.len());
                        format!(
                            "{lambda} {pattern_str}{mid}{cont}{pred_str}{cont}{bar} {body_str}",
                            cont = cont_line(body_col)
                        )
                    }
                    Form::Implicit => {
                        names.extend(resolved.iter().cloned());
                        let value_str = self.wrap_expr(value, col + 1, wc, context, names);
                        let pred_col = inner + bar.chars().count() + 1;
                        let pred_str =
                            self.wrap_pred(pred, pred_col, &wc.narrowed(1), context, names);
                        names.truncate(names.len() - decls.len());
                        format!(
                            "{{{value_str}{cont}{bar} {pred_str}}}",
                            cont = cont_line(inner)
                        )
                    }
                    Form::IdentList => {
                        let ids = self.fm_decls(decls, &resolved, context, names);
                        names.extend(resolved.iter().cloned());
                        let pred_col = inner + bar.chars().count() + 1;
                        let pred_str =
                            self.wrap_pred(pred, pred_col, &wc.narrowed(1), context, names);
                        names.truncate(names.len() - decls.len());
                        format!("{{{ids}{cont}{bar} {pred_str}}}", cont = cont_line(inner))
                    }
                    Form::Explicit => {
                        let ids = self.fm_decls(decls, &resolved, context, names);
                        names.extend(resolved.iter().cloned());
                        let pred_str = self.wrap_pred(pred, inner, wc, context, names);
                        let value_col = inner + bar.chars().count() + 1;
                        let value_str =
                            self.wrap_expr(value, value_col, &wc.narrowed(1), context, names);
                        names.truncate(names.len() - decls.len());
                        format!(
                            "{{{ids}{mid}{cont}{pred_str}{cont}{bar} {value_str}}}",
                            cont = cont_line(inner)
                        )
                    }
                }
            }
            QuantExprOp::QUnion | QuantExprOp::QInter => {
                let keyword = self.op(match op {
                    QuantExprOp::QUnion => OperatorId::QuantifiedUnion,
                    QuantExprOp::QInter => OperatorId::QuantifiedIntersection,
                    QuantExprOp::CSet => unreachable!(),
                });
                let ids = self.fm_decls(decls, &resolved, context, names);
                names.extend(resolved.iter().cloned());
                let body_col = wc.nest(col);
                let pred_str = self.wrap_pred(pred, body_col, wc, context, names);
                let value_col = body_col + bar.chars().count() + 1;
                let value_str = self.wrap_expr(value, value_col, wc, context, names);
                names.truncate(names.len() - decls.len());
                format!(
                    "{keyword} {ids}{mid}{cont}{pred_str}{cont}{bar} {value_str}",
                    cont = cont_line(body_col)
                )
            }
        }
    }

    /// Greedy comma fill over expressions inside brackets: flat items pack
    /// onto the line, a break lands after the comma, and an item that
    /// still overflows a fresh continuation line wraps recursively.
    /// `guarded` renders items as assignment parts (a bare `;` is
    /// parenthesized so the list reparses as one action).
    #[allow(clippy::too_many_arguments)]
    fn wrap_expr_list(
        &self,
        items: &[formula::Expression],
        col: usize,
        wc: &WrapCtx,
        context: FormulaContext,
        names: &mut Vec<String>,
        guarded: bool,
    ) -> String {
        let cont = wc.hang(col);
        let mut out = String::new();
        let mut cur = col;
        for (i, item) in items.iter().enumerate() {
            let flat = {
                let raw = self.fm_expr(item, context, names);
                if guarded {
                    Self::guard_action_part(raw)
                } else {
                    raw
                }
            };
            let w = flat.chars().count();
            // A non-final item is followed on its line by at least the
            // separating comma; hold it one column short so the comma
            // itself cannot pass the width.
            let reserve = usize::from(i + 1 < items.len());
            if i > 0 {
                if cur + 2 + w + reserve <= wc.width {
                    out.push_str(", ");
                    cur += 2;
                } else {
                    out.push(',');
                    out.push_str(&cont_line(cont));
                    cur = cont;
                }
            }
            if cur + w + reserve <= wc.width {
                out.push_str(&flat);
                cur += w;
            } else {
                let narrow = wc.narrowed(reserve);
                let rendered = if guarded {
                    self.wrap_guarded_expr(item, cur, &narrow, context, names)
                } else {
                    // The unguarded probe is exactly `fm_expr` and just
                    // failed the (equivalent) narrowed fit check.
                    self.wrap_expr_overflow(item, flat, cur, &narrow, context, names)
                };
                cur = end_col(cur, &rendered);
                out.push_str(&rendered);
            }
        }
        out
    }

    /// Greedy comma fill over pre-rendered single-line pieces
    /// (declaration lists).
    fn fill_parts(&self, parts: &[String], col: usize, wc: &WrapCtx) -> String {
        let cont = wc.hang(col);
        let mut out = String::new();
        let mut cur = col;
        for (i, part) in parts.iter().enumerate() {
            let w = part.chars().count();
            let reserve = usize::from(i + 1 < parts.len());
            if i > 0 {
                if cur + 2 + w + reserve <= wc.width {
                    out.push_str(", ");
                    cur += 2;
                } else {
                    out.push(',');
                    out.push_str(&cont_line(cont));
                    cur = cont;
                }
            }
            out.push_str(part);
            cur += w;
        }
        out
    }

    fn wrap_assignment(&self, assign: &formula::Assignment, col: usize, wc: &WrapCtx) -> String {
        use formula::AssignmentKind as K;
        let flat = self.print_formula_assignment(assign);
        if wc.fits(col, &flat) {
            return flat;
        }
        let context = FormulaContext::Action;
        let mut names = Vec::new();
        let targets = |idents: &[formula::Expression]| self.assignment_targets(idents);
        match assign.kind() {
            K::BecomesEqualTo { idents, values } => {
                let head = format!("{} {}", targets(idents), self.op(OperatorId::Assignment));
                let (out, rhs_col) = self.assign_rhs_position(&head, col, wc);
                format!(
                    "{out}{}",
                    self.wrap_expr_list(values, rhs_col, wc, context, &mut names, true)
                )
            }
            K::BecomesMemberOf { idents, set } => {
                let head = format!("{} {}", targets(idents), self.op(OperatorId::BecomesIn));
                let (out, rhs_col) = self.assign_rhs_position(&head, col, wc);
                format!(
                    "{out}{}",
                    self.wrap_guarded_expr(set, rhs_col, wc, context, &mut names)
                )
            }
            K::BecomesSuchThat {
                idents,
                primed,
                pred,
            } => {
                let head = format!(
                    "{} {}",
                    targets(idents),
                    self.op(OperatorId::BecomesSuchThat)
                );
                let resolved = self.fm_resolve_decls(
                    primed,
                    assign.free_identifiers(),
                    assign.dangling_bound_indices(),
                    &names,
                );
                names.extend(resolved);
                let (out, rhs_col) = self.assign_rhs_position(&head, col, wc);
                let flat_pred = self.fm_pred(pred, context, &mut names);
                let condition = if Self::has_bare_semicolon(&flat_pred) {
                    format!(
                        "({})",
                        self.wrap_pred(pred, rhs_col + 1, &wc.narrowed(1), context, &mut names)
                    )
                } else if wc.fits(rhs_col, &flat_pred) {
                    flat_pred
                } else {
                    self.wrap_pred_overflow(pred, flat_pred, rhs_col, wc, context, &mut names)
                };
                format!("{out}{condition}")
            }
        }
    }

    /// Where an assignment's right-hand side starts: hanging after the
    /// operator, or on its own nested line when the head is too deep.
    /// Returns the emitted prefix (head plus separator) and the column
    /// the right-hand side starts at.
    fn assign_rhs_position(&self, head: &str, col: usize, wc: &WrapCtx) -> (String, usize) {
        let rhs_col = col + head.chars().count() + 1;
        if rhs_col <= wc.width / 2 {
            (format!("{head} "), rhs_col)
        } else {
            let nest_col = wc.nest(col);
            (format!("{head}{}", cont_line(nest_col)), nest_col)
        }
    }

    /// An assignment part that must stay reparseable as one action: a
    /// value carrying a bare `;` is parenthesized structurally, with the
    /// wrapped content one column past the `(`.
    fn wrap_guarded_expr(
        &self,
        value: &formula::Expression,
        col: usize,
        wc: &WrapCtx,
        context: FormulaContext,
        names: &mut Vec<String>,
    ) -> String {
        let flat = self.fm_expr(value, context, names);
        if Self::has_bare_semicolon(&flat) {
            format!(
                "({})",
                self.wrap_expr(value, col + 1, &wc.narrowed(1), context, names)
            )
        } else if wc.fits(col, &flat) {
            flat
        } else {
            self.wrap_expr_overflow(value, flat, col, wc, context, names)
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
