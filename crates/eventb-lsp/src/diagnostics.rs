//! Conversion of rossi's internal findings into LSP [`Diagnostic`]s.
//!
//! This is the single place that turns a parse error (and, in future, other
//! rossi findings) into the `lsp_types::Diagnostic` the editor renders. All
//! byte-span → UTF-16 range mapping goes through [`crate::position`], so the
//! column convention can't drift from the rest of the server.

use crate::document::ParsedDocument;
use crate::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range};
use rossi::deps::{ComponentKind, Cycle, EdgeKind, kind_and_name};
use rossi::keywords::{self, KeywordId};
use rossi_build::RuleId;

/// Assemble an LSP [`Diagnostic`] from the parts that vary, filling the fields
/// every diagnostic this server emits shares: the `"rossi"` source and the
/// unused optional fields. The single place those defaults live, so the parse
/// and lint converters can't drift apart.
fn lsp_diagnostic(
    range: Range,
    severity: DiagnosticSeverity,
    code: Option<NumberOrString>,
    message: String,
) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(severity),
        code,
        source: Some("rossi".to_string()),
        message,
        related_information: None,
        tags: None,
        code_description: None,
        data: None,
    }
}

/// Diagnostics for a parsed document: the parse errors, plus the cheap
/// single-component lints (EB021-023) — but the lints only when the parse is
/// clean. `rossi validate` lints solely a fully-successful parse; the LSP
/// matches that, both for consistency and because running the lints over a
/// recovered (error-bearing) AST would double-report a duplicated clause as
/// both a parse error and a duplicate-name lint.
pub(crate) fn document_diagnostics(doc: &ParsedDocument) -> Vec<Diagnostic> {
    let mut diagnostics: Vec<Diagnostic> = doc
        .parse
        .errors
        .iter()
        .map(|e| parse_error_to_diagnostic(e, &doc.text))
        .collect();
    if doc.parse.errors.is_empty() {
        diagnostics.extend(lint_diagnostics(doc.components(), &doc.text));
    }
    diagnostics
}

/// End byte offset of the token at byte offset `start`, for sizing a diagnostic
/// range when pest reports only a point: the end of the contiguous non-whitespace
/// run starting at `start`, bounded by the line. Zero-width at EOL/EOF, one char
/// when `start` lands on whitespace.
fn token_end_byte(text: &str, start: usize) -> usize {
    let rest = &text[start..];
    match rest.chars().next() {
        None | Some('\n') => start, // EOF / EOL: zero-width
        Some(first) if first.is_whitespace() => start + first.len_utf8(), // 1-char span
        // The leading non-whitespace run ends at the first whitespace (or EOL).
        _ => start + rest.find(char::is_whitespace).unwrap_or(rest.len()),
    }
}

/// Collapse pest's multi-line rendering (a location header, the source line, a
/// caret, then an `= expected …` line) to a single line: the editor already
/// shows the location via the diagnostic range, so only the `expected …`
/// content carries information.
fn concise_pest_message(message: &str) -> String {
    message
        .lines()
        .map(str::trim_start)
        .find_map(|l| l.strip_prefix("= "))
        .map(|expected| format!("Syntax error: {expected}"))
        .unwrap_or_else(|| message.trim().to_string()) // fallback: never drop info
}

/// Convert a parse error to an LSP diagnostic
pub(crate) fn parse_error_to_diagnostic(error: &rossi::ParseError, text: &str) -> Diagnostic {
    use rossi::ParseError;

    // pest's multi-line dump is collapsed to a single line; located variants
    // keep their own message; everything else uses the Display rendering.
    let message = match error {
        ParseError::PestError { message, .. } => concise_pest_message(message),
        ParseError::RecoverableError { message, .. } | ParseError::ClauseError { message, .. } => {
            message.clone()
        }
        _ => error.to_string(),
    };

    lsp_diagnostic(
        parse_error_range(error, text),
        DiagnosticSeverity::ERROR,
        None,
        message,
    )
}

/// LSP range for a parse-error diagnostic, rendered through the single UTF-16
/// converter (issue #48).
///
/// Everything resolves to a byte `[start, end)`: a non-empty span (issue #42)
/// underlines the offending token directly; a zero-width span (pest reports a
/// single point) or a span-less variant gives only a start, so the token is
/// sized in bytes from there. A span-less start comes from the 1-indexed
/// (line, column) — those variants (nesting, clause-order, recovery) point at
/// ASCII keywords/clause content, where char and UTF-16 columns coincide.
fn parse_error_range(error: &rossi::ParseError, text: &str) -> Range {
    let span = error.span();
    let start = match span {
        Some(s) => s.start,
        None => {
            let (line, column) = error.position().unwrap_or((1, 1));
            let pos = Position::new(
                line.saturating_sub(1) as u32,
                column.saturating_sub(1) as u32,
            );
            crate::position::position_to_offset(text, pos).unwrap_or(text.len())
        }
    };
    let end = match span {
        Some(s) if s.start < s.end => s.end,
        _ => token_end_byte(text, start),
    };
    crate::position::span_to_range(&rossi::ast::Span { start, end }, text)
}

/// Run the cheap, single-component lint passes over each parsed `component` and
/// convert the findings to LSP diagnostics.
///
/// These are exactly the lints that need no project, no cross-component
/// resolution, and no type inference — duplicate identifiers (EB021), duplicate
/// labels (EB022) and shadowed names (EB023) — so they are safe to recompute on
/// every keystroke alongside the parse errors. The logic lives in
/// `rossi_build::lint::run_component` (the same pass `rossi validate` runs on
/// loose `.eventb` text); this only maps its output into the protocol's shape.
/// `text` is the source the components were parsed from, so the diagnostic spans
/// index into it. The result is lazy so the sole caller can extend its
/// diagnostics vector directly, without a throwaway intermediate `Vec`.
pub(crate) fn lint_diagnostics<'a>(
    components: &'a [rossi::Component],
    text: &'a str,
) -> impl Iterator<Item = Diagnostic> + 'a {
    components
        .iter()
        .flat_map(rossi_build::lint::run_component)
        .map(move |d| build_diagnostic_to_lsp(&d, text))
}

/// Convert a `rossi-build` lint/build diagnostic to an LSP diagnostic.
///
/// The byte span maps to a UTF-16 range through [`crate::position`], the shared
/// converter the parse-error path uses. A span-less finding falls back to
/// [`crate::analysis::default_range`], the server-wide span-less default (the
/// single-component lints always carry a span when their component was parsed
/// from text, so this is only a defensive default). The stable `EBnnn` rule id
/// becomes the diagnostic `code`, matching what `rossi validate` reports.
fn build_diagnostic_to_lsp(d: &rossi_build::Diagnostic, text: &str) -> Diagnostic {
    let range = match d.span {
        Some(span) => crate::position::span_to_range(&span, text),
        None => crate::analysis::default_range(),
    };
    lsp_diagnostic(
        range,
        build_severity_to_lsp(d.severity),
        d.rule_id
            .map(|r| NumberOrString::String(r.code().to_string())),
        d.message.clone(),
    )
}

/// Map a `rossi-build` severity onto the LSP severity scale.
fn build_severity_to_lsp(severity: rossi_build::Severity) -> DiagnosticSeverity {
    match severity {
        rossi_build::Severity::Error => DiagnosticSeverity::ERROR,
        rossi_build::Severity::Warning => DiagnosticSeverity::WARNING,
        rossi_build::Severity::Info => DiagnosticSeverity::INFORMATION,
    }
}

/// LSP range anchoring a cross-component diagnostic on a component's `keyword`
/// clause (SEES / EXTENDS / REFINES). Per-name spans aren't recorded in the AST,
/// so the diagnostic underlines the clause keyword and names the offending
/// target in its message. Falls back to the component name, then the file start.
fn clause_keyword_range(component: &rossi::Component, keyword: KeywordId, text: &str) -> Range {
    component
        .clauses()
        .iter()
        .find(|c| c.keyword == keyword)
        .map(|c| crate::position::span_to_range(&c.span, text))
        .or_else(|| {
            component
                .name_span()
                .map(|s| crate::position::span_to_range(&s, text))
        })
        .unwrap_or_else(crate::analysis::default_range)
}

/// Render a cycle as `a → b → a` (normalized order, implicitly closed).
fn cycle_chain(cycle: &Cycle) -> String {
    let mut names: Vec<&str> = cycle.components.iter().map(String::as_str).collect();
    if let Some(first) = cycle.components.first() {
        names.push(first);
    }
    names.join(" → ")
}

/// Circular EXTENDS / REFINES diagnostics (EB007 / EB008) for the open
/// document's components.
///
/// A detected cycle is always real (the dependency graph holds the edges that
/// close it), so this is never gated on the workspace — and a self-loop (a
/// component that EXTENDS / REFINES itself) is just a length-1 cycle, caught here
/// too, even in single-file mode. SEES cannot cycle (machine → context is
/// one-way), so those edges are skipped. The diagnostic lands on the offending
/// component's own clause keyword.
pub(crate) fn cycle_diagnostics(
    components: &[rossi::Component],
    cycles: &[Cycle],
    text: &str,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for component in components {
        let (kind, name) = kind_and_name(component);
        for cycle in cycles {
            let (keyword, rule) = match cycle.kind {
                EdgeKind::Extends => (KeywordId::Extends, RuleId::CircularExtends),
                EdgeKind::Refines => (KeywordId::Refines, RuleId::CircularRefines),
                EdgeKind::Sees => continue,
            };
            // Flag this component only if it is a member of the cycle, of the
            // edge's source kind (names are project-unique, but stay kind-exact).
            if cycle.kind.source_kind() != kind || !cycle.components.iter().any(|n| n == &name) {
                continue;
            }
            out.push(lsp_diagnostic(
                clause_keyword_range(component, keyword, text),
                DiagnosticSeverity::ERROR,
                Some(NumberOrString::String(rule.code().to_string())),
                format!(
                    "circular {} dependency: {}",
                    keywords::spell(keyword),
                    cycle_chain(cycle)
                ),
            ));
        }
    }
    out
}

/// The cross-references a component declares, as `(clause keyword, target kind,
/// target name)`: a machine's REFINES parent and SEES contexts, or a context's
/// EXTENDS parents.
fn component_references(component: &rossi::Component) -> Vec<(KeywordId, ComponentKind, &str)> {
    match component {
        rossi::Component::Machine(m) => m
            .refines
            .iter()
            .map(|r| (KeywordId::Refines, ComponentKind::Machine, r.as_str()))
            .chain(
                m.sees
                    .iter()
                    .map(|s| (KeywordId::Sees, ComponentKind::Context, s.as_str())),
            )
            .collect(),
        rossi::Component::Context(c) => c
            .extends
            .iter()
            .map(|e| (KeywordId::Extends, ComponentKind::Context, e.as_str()))
            .collect(),
    }
}

/// The lowercase word for a component kind, for diagnostic messages.
fn component_kind_word(kind: ComponentKind) -> &'static str {
    match kind {
        ComponentKind::Context => "context",
        ComponentKind::Machine => "machine",
    }
}

/// Unknown-cross-reference diagnostics (EB009): a SEES / EXTENDS / REFINES clause
/// naming a component absent from the workspace.
///
/// `exists(kind, name)` resolves a target against the workspace, kind-aware
/// (SEES/EXTENDS point at contexts, REFINES at a machine). The caller gates this
/// on a scanned workspace — in single-file mode no siblings are indexed, so every
/// target would look missing. Even with a workspace, Rodin-XML components
/// (`.buc`/`.bcc`, …) aren't `.eventb` and so aren't indexed, so a reference to
/// one can be a false positive; the diagnostic is an Error to match `rossi
/// validate`, accepting that residual risk. Anchored on the clause keyword
/// (per-name spans aren't recorded), with the missing target named in the message.
pub(crate) fn cross_reference_diagnostics(
    components: &[rossi::Component],
    exists: impl Fn(ComponentKind, &str) -> bool,
    text: &str,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for component in components {
        for (keyword, kind, target) in component_references(component) {
            if exists(kind, target) {
                continue;
            }
            out.push(lsp_diagnostic(
                clause_keyword_range(component, keyword, text),
                DiagnosticSeverity::ERROR,
                Some(NumberOrString::String(
                    RuleId::CrossReferenceNotFound.code().to_string(),
                )),
                format!(
                    "{} references unknown {} `{}`",
                    keywords::spell(keyword),
                    component_kind_word(kind),
                    target
                ),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        cross_reference_diagnostics, cycle_diagnostics, document_diagnostics, lint_diagnostics,
        parse_error_to_diagnostic,
    };
    use crate::document::ParsedDocument;
    use crate::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position};
    use rossi::deps::{ComponentKind, Cycle, EdgeKind};

    #[test]
    fn duplicate_clause_diagnostic_stays_on_one_line() {
        // A duplicate SETS clause yields a span-less ClauseError; the diagnostic
        // must be a single-line, token-sized range at the offending keyword, never
        // the whole multi-line clause.
        let text = "CONTEXT test\nSETS\n    S\nSETS\n    T\nEND\n";
        let error = rossi::parse(text).expect_err("duplicate SETS must fail");
        let diagnostic = parse_error_to_diagnostic(&error, text);
        assert_eq!(
            diagnostic.range.start.line, diagnostic.range.end.line,
            "clause diagnostic must stay on one line, got {:?}",
            diagnostic.range
        );
        // Sized to the duplicate `SETS` keyword on line 4 (0-indexed 3), not the body.
        assert_eq!(diagnostic.range.start, Position::new(3, 0));
        assert_eq!(diagnostic.range.end, Position::new(3, 4));
    }

    #[test]
    fn reserved_word_diagnostic_spans_the_word_issue_42() {
        // The reserved word `dom` used as a constant name carries a byte span
        // (issue #42); the diagnostic range comes from that span and covers the
        // whole 3-char word, not the old byte-length special case.
        let text = "CONTEXT c0\nCONSTANTS\n    dom\nEND\n";
        let error = rossi::parse(text).expect_err("`dom` is a reserved word");
        let diagnostic = parse_error_to_diagnostic(&error, text);
        assert_eq!(diagnostic.range.start, Position::new(2, 4));
        assert_eq!(diagnostic.range.end, Position::new(2, 7));
    }

    #[test]
    fn pest_diagnostic_uses_real_position() {
        // End-to-end through the real parser: the strict-parse error must
        // carry pest's structured position, not 0:0, and the range must be
        // sized to the offending token (the stray `+`), not a fixed width.
        let text = "CONTEXT c\nCONSTANTS\n    c1\n    +\nEND\n";
        let error = rossi::parse(text).expect_err("the stray `+` must fail strict parsing");
        let diagnostic = parse_error_to_diagnostic(&error, text);
        assert_eq!(diagnostic.range.start, Position::new(3, 4));
        // Token span: just the single-character `+`, not start + 10.
        assert_eq!(diagnostic.range.end, Position::new(3, 5));
        // Message is collapsed to a single line (issue #32): no pest caret art.
        assert!(diagnostic.message.starts_with("Syntax error:"));
        assert!(!diagnostic.message.contains("-->"));
        assert!(!diagnostic.message.contains('\n'));
    }

    #[test]
    fn pest_diagnostic_lists_symbols_not_rule_names() {
        // The expected-token list is rendered with the Event-B symbols a user
        // types, not pest's internal rule names (`op_in, op_notin, …` used to
        // leak straight into the diagnostic).
        let text = "CONTEXT c\nAXIOMS\n    @a S sdfsdf T\nEND\n";
        let error = rossi::parse(text).expect_err("`sdfsdf` where an operator is expected fails");
        let diagnostic = parse_error_to_diagnostic(&error, text);
        assert!(
            diagnostic.message.contains('∈'),
            "expected-list should use symbols, got: {}",
            diagnostic.message
        );
        assert!(
            !diagnostic.message.contains("op_in"),
            "internal rule names must not leak, got: {}",
            diagnostic.message
        );
    }

    #[test]
    fn pest_diagnostic_sized_to_token_issue_32() {
        // Issue #32, example 1: a forgotten `@` on `axm2`. Through the real
        // LSP recovery path, the diagnostic must land on the offending line
        // (line 10) and underline just the token pest stopped at (`>`), rather
        // than a fixed 10-character block running past the end of the line.
        let text = concat!(
            "CONTEXT library_ctx\n",
            "EXTENDS\n",
            "    base_ctx\n",
            "SETS\n",
            "    BOOK READER\n",
            "CONSTANTS\n",
            "    max_loans\n",
            "AXIOMS\n",
            "    @axm1: max_loans = 5\n",
            "    axm2: max_loans > 0\n",
            "END\n",
        );
        let result = rossi::parse_components_with_recovery(text);
        let error = result
            .errors
            .first()
            .expect("recovery must report the error");
        let diagnostic = parse_error_to_diagnostic(error, text);
        // Line 10 (0-indexed 9), the `>` at column 21 (0-indexed 20).
        assert_eq!(diagnostic.range.start, Position::new(9, 20));
        assert_eq!(diagnostic.range.end, Position::new(9, 21));
        assert!(!diagnostic.message.contains("-->"));
    }

    #[test]
    fn trailing_operator_flags_only_the_broken_predicate() {
        // The reported edit: a `… ∈` invariant left dangling. The strict parser
        // runs past it into the next label, but only the broken predicate may be
        // flagged — the following @RolesPartition must stay clean.
        let text = concat!(
            "MACHINE m\n",
            "VARIABLES\n",
            "    Roles\n",
            "    AdmRoles\n",
            "INVARIANTS\n",
            "    @EntitiesPartition Roles ∈\n",
            "    @RolesPartition Roles ⊆ AdmRoles\n",
            "END\n",
        );
        let result = rossi::parse_components_with_recovery(text);
        let diagnostics: Vec<_> = result
            .errors
            .iter()
            .map(|e| parse_error_to_diagnostic(e, text))
            .collect();

        assert_eq!(
            diagnostics.len(),
            1,
            "only the broken predicate is flagged, got {diagnostics:?}"
        );
        // The diagnostic stays on the @EntitiesPartition line (0-indexed 5),
        // never reaching @RolesPartition on line 6.
        assert!(diagnostics[0].range.end.line < 6);
    }

    // --- single-component lints (EB021-023) ---------------------------------
    //
    // These exercise the run_component pass surfaced through the LSP. The
    // snippets parse cleanly (strict `rossi::parse`), so every diagnostic comes
    // from the lint, not from a parse error.

    fn lint_for(text: &str) -> Vec<Diagnostic> {
        let component = rossi::parse(text).expect("snippet parses cleanly");
        lint_diagnostics(std::slice::from_ref(&component), text).collect()
    }

    fn doc_of(text: &str) -> ParsedDocument {
        ParsedDocument {
            text: text.to_string(),
            parse: rossi::parse_components_with_recovery(text),
        }
    }

    fn code_of(d: &Diagnostic) -> Option<&str> {
        match &d.code {
            Some(NumberOrString::String(s)) => Some(s),
            _ => None,
        }
    }

    #[test]
    fn duplicate_variable_is_eb021_error() {
        // `x` declared twice — a duplicate identifier (EB021), not a parse error.
        let text = "MACHINE m\nVARIABLES\n    x\n    x\nINVARIANTS\n    @inv1 x = x\nEND\n";
        let diags = lint_for(text);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let d = &diags[0];
        assert_eq!(code_of(d), Some("EB021"));
        assert_eq!(d.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(d.source.as_deref(), Some("rossi"));
        // The span underlines a single `x` (one char at the 4-space indent),
        // not the whole VARIABLES block.
        assert_eq!(d.range.start.character, 4);
        assert_eq!(d.range.end.character, 5);
        assert_eq!(d.range.start.line, d.range.end.line);
    }

    #[test]
    fn duplicate_label_is_eb022_error() {
        // Two invariants share the label `@inv1` — a duplicate label (EB022).
        let text =
            "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x = x\n    @inv1 x = x\nEND\n";
        let diags = lint_for(text);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(code_of(&diags[0]), Some("EB022"));
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
    }

    #[test]
    fn shadowed_name_is_eb023_warning() {
        // `NAT` is a valid identifier (not a reserved word, so it parses) but
        // re-lexes as ℕ — a shadowed name (EB023), reported as a Warning.
        let text = "CONTEXT c\nCONSTANTS\n    NAT\nAXIOMS\n    @axm1 1 = 1\nEND\n";
        let diags = lint_for(text);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(code_of(&diags[0]), Some("EB023"));
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
    }

    #[test]
    fn clean_component_has_no_lints() {
        let text = "CONTEXT c\nCONSTANTS\n    k\nAXIOMS\n    @axm1 k = 1\nEND\n";
        assert!(lint_for(text).is_empty());
    }

    #[test]
    fn document_diagnostics_emits_lint_on_clean_parse() {
        // A clean parse with a duplicate variable: the lint rides alongside the
        // (empty) parse errors.
        let text = "MACHINE m\nVARIABLES\n    x\n    x\nINVARIANTS\n    @inv1 x = x\nEND\n";
        let doc = doc_of(text);
        assert!(doc.parse.errors.is_empty(), "snippet must parse cleanly");
        let diags = document_diagnostics(&doc);
        assert!(
            diags.iter().any(|d| code_of(d) == Some("EB021")),
            "{diags:?}"
        );
    }

    #[test]
    fn document_diagnostics_gates_lints_on_a_broken_parse() {
        // A duplicated SETS clause is a parse error; recovery still leaves the
        // repeated name in the component, but the lints are gated on a clean
        // parse, so only the parse error surfaces — no duplicate-name lint
        // piggybacks on it (no double squiggle; matches `rossi validate`).
        let text = "CONTEXT c\nSETS\n    S\nSETS\n    S\nEND\n";
        let doc = doc_of(text);
        assert!(
            !doc.parse.errors.is_empty(),
            "duplicate SETS must be a parse error"
        );
        let diags = document_diagnostics(&doc);
        assert!(!diags.is_empty(), "the parse error must still be reported");
        // Parse errors carry no rule code; a leaked lint would carry EB0xx.
        assert!(
            diags.iter().all(|d| d.code.is_none()),
            "no lint may piggyback on a broken parse, got {diags:?}"
        );
    }

    // --- cross-component: circular EXTENDS / REFINES (EB007/008) ------------

    fn parse_one(text: &str) -> Vec<rossi::Component> {
        vec![rossi::parse(text).expect("snippet parses cleanly")]
    }

    #[test]
    fn self_extends_is_eb007() {
        // A context that EXTENDS itself — a length-1 cycle (self-reference).
        let text = "CONTEXT c\nEXTENDS c\nEND\n";
        let cycles = [Cycle {
            kind: EdgeKind::Extends,
            components: vec!["c".to_string()],
        }];
        let diags = cycle_diagnostics(&parse_one(text), &cycles, text);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(code_of(&diags[0]), Some("EB007"));
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
        assert!(diags[0].message.contains("c → c"), "{}", diags[0].message);
    }

    #[test]
    fn two_node_extends_cycle_is_eb007() {
        let text = "CONTEXT a\nEXTENDS b\nEND\n";
        let cycles = [Cycle {
            kind: EdgeKind::Extends,
            components: vec!["a".to_string(), "b".to_string()],
        }];
        let diags = cycle_diagnostics(&parse_one(text), &cycles, text);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(code_of(&diags[0]), Some("EB007"));
    }

    #[test]
    fn self_refines_is_eb008() {
        let text = "MACHINE m\nREFINES m\nEND\n";
        let cycles = [Cycle {
            kind: EdgeKind::Refines,
            components: vec!["m".to_string()],
        }];
        let diags = cycle_diagnostics(&parse_one(text), &cycles, text);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(code_of(&diags[0]), Some("EB008"));
    }

    #[test]
    fn cycle_not_touching_open_doc_is_ignored() {
        let text = "CONTEXT c\nEND\n";
        let cycles = [Cycle {
            kind: EdgeKind::Extends,
            components: vec!["x".to_string(), "y".to_string()],
        }];
        assert!(cycle_diagnostics(&parse_one(text), &cycles, text).is_empty());
    }

    // --- cross-component: unknown SEES/EXTENDS/REFINES target (EB009) -------

    #[test]
    fn unknown_sees_target_is_eb009() {
        let text = "MACHINE m\nSEES C\nEND\n";
        let diags = cross_reference_diagnostics(&parse_one(text), |_k, _n| false, text);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(code_of(&diags[0]), Some("EB009"));
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
        assert!(
            diags[0].message.contains("context `C`"),
            "{}",
            diags[0].message
        );
    }

    #[test]
    fn known_targets_produce_no_eb009() {
        let text = "MACHINE m\nSEES C\nEND\n";
        let diags = cross_reference_diagnostics(&parse_one(text), |_k, _n| true, text);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn unknown_refines_target_is_eb009_and_kind_aware() {
        // A machine named `abs` would satisfy REFINES; the closure reports only
        // contexts as existing, so the machine target stays unresolved.
        let text = "MACHINE m\nREFINES abs\nEND\n";
        let diags = cross_reference_diagnostics(
            &parse_one(text),
            |kind, _n| kind == ComponentKind::Context,
            text,
        );
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(code_of(&diags[0]), Some("EB009"));
        assert!(
            diags[0].message.contains("machine `abs`"),
            "{}",
            diags[0].message
        );
    }
}
