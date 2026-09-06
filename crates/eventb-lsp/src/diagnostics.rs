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
/// single-component checks (EB021-023) — but the checks only when the parse
/// is clean. `rossi validate` checks solely a fully-successful parse; the
/// LSP matches that, both for consistency and because running the checks
/// over a recovered (error-bearing) AST would double-report a duplicated
/// clause as both a parse error and a duplicate-name error.
pub(crate) fn document_diagnostics(doc: &ParsedDocument) -> Vec<Diagnostic> {
    let mut diagnostics: Vec<Diagnostic> = doc
        .parse()
        .errors
        .iter()
        .map(|e| parse_error_to_diagnostic(e, doc.text()))
        .collect();
    if doc.parse().errors.is_empty() {
        diagnostics.extend(lint_diagnostics(doc.components(), doc.text()));
    }
    diagnostics
}

/// The diagnostic code of an ASCII operator spelling flagged under
/// `rossi.format.enforceUnicode`; the quick-fix provider keys on it. Not an
/// `EBnnn` rule: it is an editor-side style advisory whose CI counterpart is
/// `rossi fmt --check`, not a `rossi validate` finding.
pub const ASCII_OPERATOR_CODE: &str = "ascii-operator";

/// Every ASCII operator spelling in the code of `text` (comments, labels and
/// component names masked first) as `(range, ASCII spelling, Unicode spelling)`: what
/// [`ascii_operator_diagnostics`] flags, and what its quick fix checks a
/// diagnostic against, so the two can never disagree. It is exactly what the
/// fix-all conversion would rewrite — the private-use operators, which stay
/// ASCII even in Unicode mode, are not included.
pub(crate) fn ascii_operators(text: &str) -> Vec<(Range, &str, &'static str)> {
    let masked = rossi::comments::mask_opaque(text);
    let index = crate::position::PositionIndex::new(text);
    rossi::operators::ascii_operator_spans(&masked, false)
        .into_iter()
        .map(|(span, unicode)| {
            let range = Range::new(index.position(span.start), index.position(span.end));
            (range, &text[span], unicode)
        })
        .collect()
}

/// One advisory diagnostic per ASCII operator spelling in `text` (see
/// [`ascii_operators`]), naming the Unicode spelling it should be. Textual,
/// so it runs mid-edit like the proof overlay.
pub(crate) fn ascii_operator_diagnostics(text: &str) -> Vec<Diagnostic> {
    ascii_operators(text)
        .into_iter()
        .map(|(range, ascii, unicode)| {
            lsp_diagnostic(
                range,
                DiagnosticSeverity::INFORMATION,
                Some(NumberOrString::String(ASCII_OPERATOR_CODE.to_string())),
                format!("use `{unicode}` instead of ASCII `{ascii}`"),
            )
        })
        .collect()
}

/// One informational line per component header with undischarged or broken
/// proofs, from the shared Rodin workspace's proof-status overlay (kept
/// fresh by the rodin sync watcher). Derived from disk state, not the AST,
/// so callers emit it regardless of parse health — the recovered components
/// still carry their name spans, just like the Open in Rodin lens.
pub(crate) fn proof_status_diagnostics(
    doc: &ParsedDocument,
    path: Option<&std::path::Path>,
    status: &crate::rodin::sync::ProofStatusOverlay,
) -> Vec<Diagnostic> {
    doc.components()
        .iter()
        .filter_map(|component| {
            let message = status.message_for(path, component.name())?;
            let span = component.name_span()?;
            Some(lsp_diagnostic(
                crate::position::span_to_range(&span, doc.text()),
                DiagnosticSeverity::INFORMATION,
                None,
                message.clone(),
            ))
        })
        .collect()
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
    // A mistake the grammar names precisely carries its rule, which is what the
    // quick-fix provider keys on; a plain syntax error has none.
    let code =
        RuleId::for_parse_error(error).map(|rule| NumberOrString::String(rule.code().to_string()));

    lsp_diagnostic(
        parse_error_range(error, text),
        DiagnosticSeverity::ERROR,
        code,
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

/// Run the cheap, single-component checks over each parsed `component` and
/// convert the findings to LSP diagnostics.
///
/// These are exactly the checks that need no project, no cross-component
/// resolution, and no type inference — the component-local errors from
/// `rossi_build::component_semantic_diagnostics` (duplicate identifiers and
/// labels, EB021/EB022; a primed declaration or a primed use, EB033/EB018),
/// the shadowed-name (EB023) and keyword-name (EB028) lints from
/// `rossi_build::lint::run_component`, and the non-portable-whitespace
/// advisory (EB031) from `rossi_build::lint::run_source`, which reads `text`
/// directly because whitespace sits between AST nodes rather than inside one
/// — so they are safe to recompute on every keystroke alongside the parse
/// errors. `rossi validate` runs the same passes on loose `.eventb` text;
/// this only maps their output into the protocol's shape. `text` is the
/// source the components were parsed from, so the spans index into it.
/// The result is lazy so the sole caller can extend its diagnostics vector
/// directly, without a throwaway intermediate `Vec`.
pub(crate) fn lint_diagnostics<'a>(
    components: &'a [rossi::Component],
    text: &'a str,
) -> impl Iterator<Item = Diagnostic> + 'a {
    components
        .iter()
        .flat_map(|c| {
            rossi_build::component_semantic_diagnostics(c)
                .into_iter()
                .chain(rossi_build::lint::run_component(c))
                .chain(rossi_build::lint::run_source(c, text))
        })
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

/// LSP range over a component's name — the anchor for a component-level
/// diagnostic, and the span-less fallback for clause-level ones. Falls back to
/// the file start when the name span is unavailable (a recovered parse).
fn name_range(component: &rossi::Component, text: &str) -> Range {
    component
        .name_span()
        .map(|s| crate::position::span_to_range(&s, text))
        .unwrap_or_else(crate::analysis::default_range)
}

/// LSP range anchoring a cross-component diagnostic on a component's `keyword`
/// clause (SEES / EXTENDS / REFINES). Per-name spans aren't recorded in the AST,
/// so the diagnostic underlines the clause keyword and names the offending
/// target in its message. Falls back to the component name.
fn clause_keyword_range(component: &rossi::Component, keyword: KeywordId, text: &str) -> Range {
    component
        .clauses()
        .iter()
        .find(|c| c.keyword == keyword)
        .map(|c| crate::position::span_to_range(&c.span, text))
        .unwrap_or_else(|| name_range(component, text))
}

/// The clause keyword that introduces a cross-component edge.
fn edge_keyword(edge: EdgeKind) -> KeywordId {
    match edge {
        EdgeKind::Extends => KeywordId::Extends,
        EdgeKind::Sees => KeywordId::Sees,
        EdgeKind::Refines => KeywordId::Refines,
    }
}

/// The LSP severity a rule carries, sourced from rossi-build's per-rule default
/// ([`RuleId::default_severity`]) so the editor and `rossi validate` can't
/// disagree on whether a finding is an error or a warning.
fn rule_severity(rule: RuleId) -> DiagnosticSeverity {
    build_severity_to_lsp(rule.default_severity())
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
                rule_severity(rule),
                Some(NumberOrString::String(rule.code().to_string())),
                format!(
                    "circular {}: {}",
                    keywords::spell(keyword),
                    cycle_chain(cycle)
                ),
            ));
        }
    }
    out
}

/// The cross-reference edges a component declares, as `(edge kind, target
/// name)`: a machine's REFINES parent and SEES contexts, or a context's EXTENDS
/// parents. The target [`ComponentKind`] is derived from the edge via
/// [`EdgeKind::target_kind`], the shared SSOT, rather than re-stated here.
/// Also the edge SSOT for the animate closure walk.
pub(crate) fn component_references(component: &rossi::Component) -> Vec<(EdgeKind, &str)> {
    match component {
        rossi::Component::Machine(m) => m
            .refines
            .iter()
            .map(|r| (EdgeKind::Refines, r.as_str()))
            .chain(m.sees.iter().map(|s| (EdgeKind::Sees, s.as_str())))
            .collect(),
        rossi::Component::Context(c) => c
            .extends
            .iter()
            .map(|e| (EdgeKind::Extends, e.as_str()))
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
        for (edge, target) in component_references(component) {
            let kind = edge.target_kind();
            if exists(kind, target) {
                continue;
            }
            out.push(lsp_diagnostic(
                clause_keyword_range(component, edge_keyword(edge), text),
                rule_severity(RuleId::CrossReferenceNotFound),
                Some(NumberOrString::String(
                    RuleId::CrossReferenceNotFound.code().to_string(),
                )),
                format!(
                    "{} references unknown {} `{}`",
                    keywords::spell(edge_keyword(edge)),
                    component_kind_word(kind),
                    target
                ),
            ));
        }
    }
    out
}

/// Duplicate-component diagnostics (EB019): a component name declared more
/// than once. `declarations(name)` answers as
/// [`crate::cross_references::CrossReferenceManager::component_declarations`]
/// does — each declaring file with its declaration count — which is what
/// separates several declarations in one (e.g. `import --merge`) file from one
/// apiece across several files. The caller gates on a scanned workspace, since
/// cross-file duplicates aren't observable from a single file. Anchored on the
/// component name; an Error, and worded to match `rossi validate`.
pub(crate) fn duplicate_component_diagnostics(
    components: &[rossi::Component],
    declarations: impl Fn(&str) -> Vec<(String, usize)>,
    text: &str,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for component in components {
        let name = component.name();
        let files = declarations(name);
        if files.iter().map(|(_, count)| count).sum::<usize>() < 2 {
            continue;
        }
        let display = |uri: &String| crate::uri_identity::DocumentUris::display_name(uri);
        let message = match files.as_slice() {
            [(file, count)] => format!(
                "component `{name}` is defined {count} times in {}",
                display(file)
            ),
            _ => format!(
                "component `{name}` is defined in multiple files: {}",
                files
                    .iter()
                    .map(|(uri, _)| display(uri))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        out.push(lsp_diagnostic(
            name_range(component, text),
            rule_severity(RuleId::DuplicateComponent),
            Some(NumberOrString::String(
                RuleId::DuplicateComponent.code().to_string(),
            )),
            message,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        ascii_operator_diagnostics, cross_reference_diagnostics, cycle_diagnostics,
        document_diagnostics, duplicate_component_diagnostics, lint_diagnostics,
        parse_error_to_diagnostic, proof_status_diagnostics,
    };
    use crate::document::ParsedDocument;
    use crate::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position};
    use rossi::deps::{ComponentKind, Cycle, EdgeKind};

    /// A name's own segments read as ASCII spellings (see
    /// `rossi::comments::LexicalSpans::names`); none of them is code.
    #[test]
    fn ascii_operator_diagnostics_skip_component_names() {
        let text = concat!(
            "MACHINE A-C0\n",
            "REFINES end-to-end\n",
            "SEES CTX-INT-1 A-or-B\n",
            "VARIABLES x\n",
            "INVARIANTS\n",
            "  @i x - 1 : NAT\n",
            "END\n",
        );
        let diagnostics = ascii_operator_diagnostics(text);
        let flagged: Vec<&str> = diagnostics.iter().map(|d| d.message.as_str()).collect();
        // Only the invariant's own operators, never a name's hyphen or a
        // keyword-shaped segment inside one.
        assert_eq!(
            flagged,
            [
                "use `−` instead of ASCII `-`",
                "use `∈` instead of ASCII `:`",
                "use `ℕ` instead of ASCII `NAT`",
            ]
        );
    }

    #[test]
    fn ascii_operator_diagnostics_flag_code_spellings_only() {
        // `@inv-1` is a label, `// x <= 1` a comment, and `<+` the
        // private-use override that stays ASCII in Unicode mode: none is
        // flagged. The three ASCII spellings in code are, each on its own
        // token with its Unicode form.
        let text = "@inv-1 x : NAT & x <+ y // x <= 1\n";
        let diagnostics = ascii_operator_diagnostics(text);
        let flagged: Vec<(u32, u32, &str)> = diagnostics
            .iter()
            .map(|d| {
                (
                    d.range.start.character,
                    d.range.end.character,
                    d.message.as_str(),
                )
            })
            .collect();
        assert_eq!(
            flagged,
            [
                (9, 10, "use `∈` instead of ASCII `:`"),
                (11, 14, "use `ℕ` instead of ASCII `NAT`"),
                (15, 16, "use `∧` instead of ASCII `&`"),
            ]
        );
        assert!(diagnostics.iter().all(|d| {
            d.code == Some(NumberOrString::String("ascii-operator".to_string()))
                && d.severity == Some(DiagnosticSeverity::INFORMATION)
        }));
    }

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
    fn structural_parse_errors_carry_their_rule_and_underline_the_construct() {
        // The range is what a quick fix acts on: the clause keyword for an
        // empty clause, the label for a bare label, the whole clause for one
        // written out of order, the item itself for a missing label. The
        // wording is pinned where the errors are raised
        // (`crates/rossi/tests/empty_clause_test.rs`).
        for (text, code, range) in [
            (
                "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT e\n    WHERE\n    THEN\n        @act1 x ≔ 1\n    END\nEND\n",
                "EB029",
                (Position::new(5, 4), Position::new(5, 9)),
            ),
            (
                "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1\nEND\n",
                "EB029",
                (Position::new(4, 4), Position::new(4, 9)),
            ),
            (
                "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT e\n    THEN\n        @act1 x ≔ 1\n    WITH\n        @w y = 1\n    END\nEND\n",
                "EB030",
                (Position::new(7, 4), Position::new(8, 16)),
            ),
            (
                "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT e\n    THEN\n        x ≔ 1\n    END\nEND\n",
                "EB032",
                (Position::new(6, 8), Position::new(6, 13)),
            ),
        ] {
            let error = rossi::parse(text).expect_err("must fail strict parsing");
            let diagnostic = parse_error_to_diagnostic(&error, text);
            assert_eq!(code_of(&diagnostic), Some(code), "for:\n{text}");
            assert_eq!(
                (diagnostic.range.start, diagnostic.range.end),
                range,
                "for:\n{text}"
            );
        }
    }

    #[test]
    fn a_misplaced_clause_is_reported_once() {
        // Recovery re-reads the event from text, and a clause written after
        // THEN sits inside what would otherwise be read as the action list —
        // reporting the same mistake again as two unparsable actions. The
        // actions stop at the misplaced keyword, so only the real error stands.
        let text = "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT e\n    THEN\n        @act1 x ≔ 1\n    WITH\n        @w y = 1\n    END\nEND\n";
        let diagnostics = document_diagnostics(&doc_of(text));
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(code_of(&diagnostics[0]), Some("EB030"));
    }

    #[test]
    fn pest_diagnostic_lists_symbols_not_rule_names() {
        // The expected list is rendered for a reader: pest's internal rule
        // names (`op_in, op_notin, …` used to leak straight into the
        // diagnostic) become the Event-B symbols a user types, and a whole
        // class of them becomes the category it stands for.
        for (text, expected) in [
            (
                "CONTEXT c\nAXIOMS\n    @a S sdfsdf T\nEND\n",
                "Syntax error: expected an operator",
            ),
            (
                "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT e\n    THEN\n        @act1 x\n    END\nEND\n",
                "Syntax error: expected ≔, :∈, :∣, ,, or (",
            ),
        ] {
            let error = rossi::parse(text).expect_err("must fail strict parsing");
            let diagnostic = parse_error_to_diagnostic(&error, text);
            assert_eq!(diagnostic.message, expected);
            assert!(
                !diagnostic.message.contains("op_"),
                "internal rule names must not leak, got: {}",
                diagnostic.message
            );
        }
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

    #[test]
    fn assignment_in_invariant_is_eb026_error() {
        // `@inv1 x := 5` is an assignment where a predicate is required. Through
        // the recovery path the LSP reports it as an EB026 error underlining just
        // the `:=` operator, so the quick-fix provider can match on the code.
        let text = "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x := 5\nEND\n";
        let result = rossi::parse_components_with_recovery(text);
        let diagnostics: Vec<_> = result
            .errors
            .iter()
            .map(|e| parse_error_to_diagnostic(e, text))
            .collect();
        assert_eq!(
            diagnostics.len(),
            1,
            "one EB026 diagnostic: {diagnostics:?}"
        );
        let d = &diagnostics[0];
        assert_eq!(code_of(d), Some("EB026"));
        assert_eq!(d.severity, Some(DiagnosticSeverity::ERROR));
        assert!(
            d.message.contains("assignment operator"),
            "message names the operator: {}",
            d.message
        );
        // The range underlines just `:=` on the invariant line (0-indexed 4).
        assert_eq!(d.range.start.line, 4);
        assert_eq!(d.range.start.character, 12);
        assert_eq!(d.range.end.character, 14);
    }

    // --- single-component lints (EB021-023, EB028, EB033) --------------------
    //
    // These exercise the run_component pass surfaced through the LSP. The
    // snippets parse cleanly (strict `rossi::parse`), so every diagnostic comes
    // from the lint, not from a parse error.

    fn lint_for(text: &str) -> Vec<Diagnostic> {
        let component = rossi::parse(text).expect("snippet parses cleanly");
        lint_diagnostics(std::slice::from_ref(&component), text).collect()
    }

    fn doc_of(text: &str) -> ParsedDocument {
        ParsedDocument::from_text(text.to_string())
    }

    fn code_of(d: &Diagnostic) -> Option<&str> {
        match &d.code {
            Some(NumberOrString::String(s)) => Some(s),
            _ => None,
        }
    }

    #[test]
    fn proof_status_diagnostics_anchor_on_component_headers() {
        use crate::rodin::sync::{ProjectProofStatus, ProofStatusOverlay, ProofStatusUpdate};

        let mut status = ProjectProofStatus::default();
        status.by_name.insert(
            "M0".to_string(),
            "Rodin: 1 undischarged proof obligation(s)".to_string(),
        );
        let mut overlay = ProofStatusOverlay::default();
        overlay.apply(ProofStatusUpdate::Projects(vec![(
            std::path::PathBuf::from("/ws/proj"),
            status,
        )]));

        let doc = doc_of("MACHINE M0\nEND\n");
        let diags = proof_status_diagnostics(&doc, None, &overlay);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::INFORMATION));
        assert!(diags[0].message.contains("undischarged"), "{diags:?}");
        assert_eq!(diags[0].range.start.line, 0);

        // A mid-edit document (parse errors, recovered AST) keeps the line:
        // the overlay is disk-derived and must not vanish while typing.
        let broken = doc_of("MACHINE M0\nVARIABLES\n    ∧∧\nEND\n");
        assert!(
            !broken.parse().errors.is_empty(),
            "fixture must carry a parse error"
        );
        assert_eq!(proof_status_diagnostics(&broken, None, &overlay).len(), 1);
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
    fn keyword_name_is_eb028_warning() {
        // `end` parses as a carrier set (the first item of a list is not
        // keyword-guarded) but spells a structural keyword no textual
        // notation can represent — a keyword name (EB028), reported as a
        // Warning on the declaration.
        let text = "CONTEXT c\nSETS\n    end\nAXIOMS\n    @axm1 1 = 1\nEND\n";
        let diags = lint_for(text);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(code_of(&diags[0]), Some("EB028"));
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(diags[0].range.start.line, 2);
        assert_eq!(diags[0].range.start.character, 4);
        assert_eq!(diags[0].range.end.character, 7);
    }

    #[test]
    fn primed_declared_name_is_eb033_error() {
        // `c'` parses as a constant name (the grammar allows the after-state
        // prime on any identifier) but Rodin's `IdentifierModule` refuses a
        // primed declaration outright, so this is an Error on the declaration.
        let text = "CONTEXT c\nCONSTANTS\n    c'\nAXIOMS\n    @axm1 1 = 1\nEND\n";
        let diags = lint_for(text);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(code_of(&diags[0]), Some("EB033"));
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(diags[0].range.start.line, 2);
        assert_eq!(diags[0].range.start.character, 4);
        assert_eq!(diags[0].range.end.character, 6);
    }

    #[test]
    fn primed_use_in_an_axiom_is_eb018_error() {
        // The one scope finding a lone document can decide: no declaration
        // may be primed, so nothing a SEES / EXTENDS parent could hold would
        // ever put `c'` in scope.
        let text = "CONTEXT c\nCONSTANTS\n    k\nAXIOMS\n    @axm1 k' = 1\nEND\n";
        let diags = lint_for(text);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(code_of(&diags[0]), Some("EB018"));
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
        assert!(
            diags[0].message.contains("unknown identifier 'k''"),
            "{:?}",
            diags[0]
        );
    }

    #[test]
    fn bound_after_state_read_is_not_reported() {
        // `x :∣ x' = x + 1` binds `x'` as a primed declaration in the formula
        // model, so it is not a free primed read.
        let text = "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT e\n    THEN\n        @act1 x :∣ x' = x + 1\n    END\nEND\n";
        let diags = lint_for(text);
        assert!(diags.is_empty(), "{diags:?}");
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
        assert!(doc.parse().errors.is_empty(), "snippet must parse cleanly");
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
            !doc.parse().errors.is_empty(),
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

    // --- cross-component: duplicate component name (EB019) ------------------

    #[test]
    fn duplicate_component_within_one_file_names_the_count() {
        // Two declarations, one file: listing the file twice would read as a
        // cross-file duplicate, so say how many times instead.
        let text = "CONTEXT c\nEND\n";
        let declarations = |n: &str| {
            if n == "c" {
                vec![("merged.eventb".to_string(), 2)]
            } else {
                vec![]
            }
        };
        let diags = duplicate_component_diagnostics(&parse_one(text), declarations, text);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(code_of(&diags[0]), Some("EB019"));
        assert_eq!(
            diags[0].message,
            "component `c` is defined 2 times in merged.eventb"
        );
    }

    #[test]
    fn two_files_sharing_a_basename_are_not_reported_as_one_file() {
        // Two directories may each hold an `m.eventb`. Counting by basename
        // would report the pair as one file holding two declarations.
        let text = "CONTEXT c\nEND\n";
        let declarations = |n: &str| {
            if n == "c" {
                vec![
                    ("file:///one/m.eventb".to_string(), 1),
                    ("file:///two/m.eventb".to_string(), 1),
                ]
            } else {
                vec![]
            }
        };
        let diags = duplicate_component_diagnostics(&parse_one(text), declarations, text);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(
            diags[0].message,
            "component `c` is defined in multiple files: m.eventb, m.eventb"
        );
    }

    #[test]
    fn duplicate_component_is_eb019_error() {
        let text = "CONTEXT c\nEND\n";
        let files = |n: &str| {
            if n == "c" {
                vec![("a.eventb".to_string(), 1), ("b.eventb".to_string(), 1)]
            } else {
                vec![]
            }
        };
        let diags = duplicate_component_diagnostics(&parse_one(text), files, text);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(code_of(&diags[0]), Some("EB019"));
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
        assert!(
            diags[0].message.contains("a.eventb") && diags[0].message.contains("b.eventb"),
            "{}",
            diags[0].message
        );
    }

    #[test]
    fn unique_component_has_no_eb019() {
        let text = "CONTEXT c\nEND\n";
        // Defined in exactly one file — not a cross-file duplicate.
        let files = |_n: &str| vec![("c.eventb".to_string(), 1)];
        assert!(duplicate_component_diagnostics(&parse_one(text), files, text).is_empty());
    }
}
