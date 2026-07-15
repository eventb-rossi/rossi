use clap::{Args, ValueEnum};
use rossi::{
    Component, ParseError, parse_components, parse_components_with_recovery,
    parse_zip_with_recovery,
};
use rossi_build::project::discover_projects;
use rossi_build::{Diagnostic, Project, RuleId, Severity, error::ProjectError};
use serde::{Serialize, Serializer};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::commands::eventb_io;
use crate::commands::sarif;

#[derive(Args)]
pub struct ValidateArgs {
    /// Event-B files (`.eventb`), Rodin ZIP archives (`.zip`), or
    /// unzipped Rodin project directories. `-` reads Event-B text from stdin
    /// (see `--stdin-filename`).
    #[arg(required = true, value_name = "FILE")]
    files: Vec<PathBuf>,

    /// Output format
    #[arg(short, long, value_enum, default_value = "text")]
    format: OutputFormat,

    /// Quiet mode (only show errors)
    #[arg(short, long)]
    quiet: bool,

    /// Continue validating files even if one fails
    #[arg(short, long)]
    continue_on_error: bool,

    /// Skip rossi-build semantic checks (duplicate component names,
    /// duplicate identifiers/labels, cycles, cross-refs, type errors).
    #[arg(long)]
    no_semantic: bool,

    /// Skip rossi-build advisory lint passes (dead variable, unmodified
    /// variable, incomplete INIT, shadowed name).
    #[arg(long)]
    no_lints: bool,

    /// Reported file name for `-` (stdin) input (default: `<stdin>`).
    #[arg(long, value_name = "PATH")]
    stdin_filename: Option<String>,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    /// Human-readable text output
    Text,
    /// JSON output
    Json,
    /// SARIF 2.1.0 output (for IDE / GitHub code-scanning consumers)
    Sarif,
}

/// 1-indexed source region of a parse failure (character columns — the SARIF
/// and Camille convention). `end` equals `start` for a single point.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Region {
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

/// One row of the validation report — a parsed component, a parse failure,
/// or a semantic/lint diagnostic. All constructors live below so the
/// derived `success` / `severity` pair stays consistent.
#[derive(Debug, Serialize)]
pub struct ValidationResult {
    pub file: PathBuf,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inner_filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component_name: Option<String>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "ser_severity"
    )]
    pub severity: Option<Severity>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "ser_rule_id"
    )]
    pub rule_id: Option<RuleId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// Source region of a parse failure, when known (issue #42). Populated for
    /// loose-text parse errors, where the source is available to resolve it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<Region>,
}

fn ser_severity<S: Serializer>(value: &Option<Severity>, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_some(&value.expect("skipped by serde when None").to_string())
}

fn ser_rule_id<S: Serializer>(value: &Option<RuleId>, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_some(value.expect("skipped by serde when None").code())
}

pub fn run(cli: ValidateArgs) -> ExitCode {
    if let Err(e) = eventb_io::stdin_is_sole_input(&cli.files) {
        eprintln!("rossi validate: {e}");
        return ExitCode::from(2);
    }

    let mut results = Vec::new();
    let mut all_success = true;
    let aggregating_format = matches!(cli.format, OutputFormat::Json | OutputFormat::Sarif);

    for file in &cli.files {
        let file_results = validate_file(file, &cli);

        for result in file_results {
            if cli.format == OutputFormat::Text
                && (!cli.quiet || result.severity == Some(Severity::Error))
            {
                print_text_result(&result);
            }

            if !result.success {
                all_success = false;
                if !cli.continue_on_error && !aggregating_format {
                    return ExitCode::from(1);
                }
            }

            results.push(result);
        }
    }

    let validation_exit = if all_success {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    };
    let output_result = match cli.format {
        OutputFormat::Text => {
            if !cli.quiet && results.len() > 1 {
                print_summary(&results);
            }
            return validation_exit;
        }
        OutputFormat::Json => write_json(&results, &mut io::stdout().lock()),
        OutputFormat::Sarif => sarif::emit(&results, &mut io::stdout().lock()),
    };

    let mut stderr = io::stderr().lock();
    finish_structured_output(output_result, validation_exit, &mut stderr)
}

fn finish_structured_output(
    output_result: io::Result<()>,
    validation_exit: ExitCode,
    stderr: &mut impl Write,
) -> ExitCode {
    match output_result {
        Ok(()) => validation_exit,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => validation_exit,
        Err(e) => {
            let _ = writeln!(stderr, "rossi validate: failed to write output: {e}");
            ExitCode::from(1)
        }
    }
}

fn validate_file(file: &Path, cli: &ValidateArgs) -> Vec<ValidationResult> {
    if eventb_io::is_stdin(file) {
        return validate_stdin(cli);
    }

    if !file.exists() {
        return vec![error_result(
            file,
            None,
            format!("File not found: {}", file.display()),
            None,
        )];
    }

    if file.is_dir() {
        return validate_directory(file, cli);
    }

    if let Some(ext) = file.extension()
        && ext.eq_ignore_ascii_case("zip")
    {
        return validate_zip_file(file, cli);
    }

    validate_text_file(file, cli)
}

fn validate_text_file(file: &Path, cli: &ValidateArgs) -> Vec<ValidationResult> {
    let source = match fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            return vec![error_result(
                file,
                None,
                format!("Failed to read file: {e}"),
                None,
            )];
        }
    };
    validate_text_source(file, &source, cli)
}

/// Validate Event-B `-` (stdin) text, reported under `--stdin-filename`.
fn validate_stdin(cli: &ValidateArgs) -> Vec<ValidationResult> {
    let display = Path::new(cli.stdin_filename.as_deref().unwrap_or("<stdin>"));
    match eventb_io::read_stdin_to_string() {
        Ok(source) => validate_text_source(display, &source, cli),
        Err(e) => vec![error_result(
            display,
            None,
            format!("Failed to read standard input: {e}"),
            None,
        )],
    }
}

/// Validate Event-B text from any source, reporting rows under `display`.
/// Loose text has no project (its SEES/EXTENDS parents are usually absent),
/// so the SC build doesn't run here; the component-local checks do — the
/// duplicate-name errors (EB021/EB022, semantic, from the same shared core
/// the SC uses) and the component-local lints. The reference-based lints
/// need the project paths (directories, zip archives).
fn validate_text_source(display: &Path, source: &str, cli: &ValidateArgs) -> Vec<ValidationResult> {
    match parse_components(source) {
        Ok(components) => {
            let mut results: Vec<ValidationResult> = components
                .iter()
                .map(|c| success_result(display, None, c))
                .collect();
            for component in &components {
                if !cli.no_semantic {
                    for diag in rossi_build::duplicates::component_duplicate_diagnostics(component)
                    {
                        // Loose text is a single source; every span indexes into it.
                        results.push(fold_diagnostic(display, diag, None, Some(source)));
                    }
                }
                if !cli.no_lints {
                    for diag in rossi_build::lint::run_component(component) {
                        results.push(fold_diagnostic(display, diag, None, Some(source)));
                    }
                }
            }
            results
        }
        // Loose `.eventb` text → Camille parse failure (EB004). Some formula
        // failures carry precise, actionable variants; recover
        // per-clause to report them rather than collapsing them into a whole-file
        // EB004.
        Err(e) => {
            let recovered = parse_components_with_recovery(source);
            let mut results: Vec<ValidationResult> = recovered
                .errors
                .iter()
                .filter_map(precise_formula_error)
                .map(|(err, absolute_span)| {
                    let mut result = error_result(
                        display,
                        None,
                        format!("{err}"),
                        Some(rule_for_parse_error(err)),
                    );
                    result.region = absolute_span
                        .map(|span| span_to_region(source, span))
                        .or_else(|| parse_error_region(err, source));
                    result
                })
                .collect();
            // Keep the whole-file EB004 unless precise formula failures are the
            // only errors recovery found. A co-occurring generic failure must
            // never be silently dropped.
            let only_precise_errors =
                !results.is_empty() && results.len() == recovered.errors.len();
            if !only_precise_errors {
                let mut fallback = error_result(
                    display,
                    None,
                    format!("{e}"),
                    Some(RuleId::CamilleParseError),
                );
                fallback.region = parse_error_region(&e, source);
                results.push(fallback);
            }
            results
        }
    }
}

/// Return a precise loose-text formula error and, when it came from recovery,
/// its source-absolute span. Recovery sources use segment-relative coordinates;
/// the outer envelope anchors that segment in the document.
fn precise_formula_error(err: &ParseError) -> Option<(&ParseError, Option<rossi::ast::Span>)> {
    match err {
        ParseError::AssignmentInPredicate { .. } | ParseError::AssignmentArityMismatch { .. } => {
            Some((err, None))
        }
        ParseError::RecoverableError {
            span: Some(recovery_span),
            source: Some(source),
            ..
        } if matches!(source.as_ref(), ParseError::AssignmentArityMismatch { .. }) => {
            let absolute_span = source.span().map_or(*recovery_span, |mut span| {
                span.shift(recovery_span.start);
                span
            });
            Some((source, Some(absolute_span)))
        }
        _ => None,
    }
}

/// Validate a Rodin `.zip`, project by project.
///
/// A Rodin archive may bundle several top-level projects (an Eclipse "Archive
/// File" export of a decomposition). Discovering and checking each project on
/// its own keeps semantic checks correct — sibling projects that share a
/// component name (each its own `M.bum`) no longer flag false duplicate /
/// cross-reference diagnostics — and lets every row be attributed to the
/// project it came from. Component rows carry a project-qualified
/// `inner_filename` (e.g. `A/M.bum`) only when more than one project is present,
/// so a single project (flat or nested under one directory) keeps its bare
/// basename, exactly as before.
///
/// Discovery is strict: one malformed component aborts it. In that case we fall
/// back to the tolerant flat parse ([`validate_zip_flat_fallback`]) so every bad
/// file still gets a granular diagnostic — an already-broken archive forgoes
/// per-project attribution, which it never had.
fn validate_zip_file(file: &Path, cli: &ValidateArgs) -> Vec<ValidationResult> {
    let bytes = match fs::read(file) {
        Ok(b) => b,
        Err(e) => {
            return vec![error_result(
                file,
                None,
                format!("Failed to read zip file: {e}"),
                None,
            )];
        }
    };
    let fallback = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("project");
    let projects = match discover_projects(&bytes, fallback) {
        Ok(projects) => projects,
        // Reuse the already-read bytes — the fallback must not re-read the file.
        Err(_) => return validate_zip_flat_fallback(file, &bytes, fallback, cli),
    };

    // Drop source-only projects (a stray `.project` with no components): they
    // carry no rows and must not flip the multi gate — otherwise a single real
    // project beside a root-level descriptor would be spuriously prefix-qualified
    // (and diverge from `rossi import`, which also drops them).
    let projects: Vec<_> = projects
        .into_iter()
        .filter(|p| !p.components.is_empty())
        .collect();
    if projects.is_empty() {
        return vec![error_result(
            file,
            None,
            "No Event-B components found in zip file".to_string(),
            None,
        )];
    }

    let multi = projects.len() > 1;
    let mut results = Vec::new();
    for dp in projects {
        // Qualify rows by project only when sibling projects could otherwise be
        // confused; a lone project keeps its bare basename.
        let prefix = if multi {
            dp.prefix.clone()
        } else {
            String::new()
        };
        for pc in &dp.components {
            results.push(success_result(
                file,
                Some(format!("{prefix}{}", pc.filename)),
                &pc.component,
            ));
        }
        if !cli.no_semantic {
            let project = dp.into_project();
            fold_semantic(&project, file, cli, &prefix, &mut results);
        }
    }
    results
}

/// Tolerant flat validation of a `.zip`, used when strict project discovery
/// fails on a malformed component. Reports each parse failure granularly and,
/// when the archive otherwise parses, folds the whole archive's semantic
/// diagnostics as a single project — the historical behavior.
fn validate_zip_flat_fallback(
    file: &Path,
    bytes: &[u8],
    name: &str,
    cli: &ValidateArgs,
) -> Vec<ValidationResult> {
    let parse_result = parse_zip_with_recovery(bytes);
    let mut results = Vec::new();
    let mut had_parse_error = false;

    for err in parse_result.get_errors() {
        had_parse_error = true;
        results.push(error_result(
            file,
            None,
            format!("{err}"),
            Some(rule_for_parse_error(err)),
        ));
    }

    if let Some(components) = parse_result.component {
        if components.is_empty() && results.is_empty() {
            results.push(error_result(
                file,
                None,
                "No Event-B components found in zip file".to_string(),
                None,
            ));
            had_parse_error = true;
        } else {
            for named in components {
                results.push(success_result(file, Some(named.filename), &named.component));
            }
        }
    } else if results.is_empty() {
        results.push(error_result(
            file,
            None,
            "Failed to parse zip file".to_string(),
            None,
        ));
        had_parse_error = true;
    }

    if !cli.no_semantic && !had_parse_error {
        match Project::from_zip_bytes(name, bytes) {
            // The fallback treats the whole archive as one project (no prefix).
            Ok(project) => fold_semantic(&project, file, cli, "", &mut results),
            Err(e) => results.push(error_result(
                file,
                None,
                format!("{e}"),
                rule_for_build_error(&e),
            )),
        }
    }

    results
}

fn validate_directory(dir: &Path, cli: &ValidateArgs) -> Vec<ValidationResult> {
    let mut results = Vec::new();

    if cli.no_semantic {
        results.push(error_result(
            dir,
            None,
            "directory inputs require semantic checks; drop --no-semantic or pass a .zip / .eventb file".to_string(),
            None,
        ));
        return results;
    }

    match Project::from_directory(dir) {
        Ok(project) => {
            for pc in &project.components {
                results.push(success_result(
                    dir,
                    Some(pc.filename.clone()),
                    &pc.component,
                ));
            }
            // A directory is loaded as a single project (no prefix).
            fold_semantic(&project, dir, cli, "", &mut results);
        }
        Err(e) => results.push(error_result(
            dir,
            None,
            format!("{e}"),
            rule_for_build_error(&e),
        )),
    }

    results
}

/// Fold a project's build + lint diagnostics into rows, qualifying each row's
/// `inner_filename` with `prefix` (the project's archive prefix, or `""` for a
/// single project / directory) so a multi-project archive's rows say which
/// project they came from.
fn fold_semantic(
    project: &Project,
    file: &Path,
    cli: &ValidateArgs,
    prefix: &str,
    out: &mut Vec<ValidationResult>,
) {
    let build = rossi_build::build(project);
    for diag in build.diagnostics {
        out.push(fold_project_diagnostic(file, diag, project, prefix));
    }
    if !cli.no_lints {
        for diag in rossi_build::lint::run(project) {
            out.push(fold_project_diagnostic(file, diag, project, prefix));
        }
    }
}

fn success_result(file: &Path, inner: Option<String>, component: &Component) -> ValidationResult {
    let (component_type, component_name) = match component {
        Component::Context(c) => ("Context", c.name.clone()),
        Component::Machine(m) => ("Machine", m.name.clone()),
    };
    ValidationResult {
        file: file.to_path_buf(),
        success: true,
        inner_filename: inner,
        error: None,
        component_type: Some(component_type),
        component_name: Some(component_name),
        severity: None,
        rule_id: None,
        origin: None,
        region: None,
    }
}

/// Fold a build/lint [`Diagnostic`] into a [`ValidationResult`], resolving its
/// byte span to a 1-indexed [`Region`] against `source` — the text of the
/// component the span indexes into — when both are present. `inner_filename`
/// names the component file inside a directory/archive so editors open it.
fn fold_diagnostic(
    file: &Path,
    diag: Diagnostic,
    inner_filename: Option<String>,
    source: Option<&str>,
) -> ValidationResult {
    let region = diag
        .span
        .zip(source)
        .map(|(span, src)| span_to_region(src, span));
    ValidationResult {
        file: file.to_path_buf(),
        success: diag.severity != Severity::Error,
        inner_filename,
        error: Some(diag.message),
        component_type: None,
        component_name: None,
        severity: Some(diag.severity),
        rule_id: diag.rule_id,
        origin: Some(diag.origin),
        region,
    }
}

/// Fold a project (build/lint) diagnostic, resolving it against the component
/// it belongs to: the leading dot-separated segment of `origin` is the
/// component name, which yields that component's source (for the region) and
/// filename (for `inner_filename`). `prefix` is prepended to the filename so a
/// multi-project archive's rows are project-qualified (empty for a single
/// project). Components imported from Rodin XML carry no source, so their
/// diagnostics stay region-less.
fn fold_project_diagnostic(
    file: &Path,
    diag: Diagnostic,
    project: &Project,
    prefix: &str,
) -> ValidationResult {
    let component = diag.origin.split('.').next().unwrap_or(&diag.origin);
    let mut carriers = project
        .components
        .iter()
        .filter(|pc| pc.component.name() == component);
    let pc = carriers.next();
    // Under duplicate component names (EB019, an Error) the origin is
    // ambiguous: anchoring to the first carrier would point the editor at
    // the wrong file, with a byte span resolved against the wrong source.
    // Report such diagnostics unanchored instead.
    let pc = if carriers.next().is_some() { None } else { pc };
    let inner = pc.map(|pc| format!("{prefix}{}", pc.filename));
    let source = pc.and_then(|pc| pc.source.as_deref());
    fold_diagnostic(file, diag, inner, source)
}

/// Pick the right [`RuleId`] for an XML-path [`ParseError`] surfaced by
/// [`parse_zip_file_with_recovery`]. Unwraps the `FileContext` envelope so
/// EB002/EB003 emitted from deep inside the per-file parse still reach the
/// CLI as structured rule codes; everything else falls back to EB001.
fn rule_for_parse_error(err: &ParseError) -> RuleId {
    let mut inner = err;
    while let ParseError::FileContext { source, .. } = inner {
        inner = source;
    }
    match inner {
        ParseError::UnexpectedXmlRoot { .. } => RuleId::XmlRootError,
        ParseError::MissingXmlAttribute { .. } => RuleId::XmlAttributeError,
        // A formula inside an XML attribute exceeded the nesting limit —
        // that's a formula problem (EB005), not a malformed-XML one.
        ParseError::NestingTooDeep { .. } => RuleId::FormulaParseError,
        // `MalformedAttribute` is only raised by `wrap_attr_error` when a
        // formula attribute (predicate/expression/assignment) is rejected by
        // the grammar — that's a formula syntax error, not XML corruption.
        ParseError::MalformedAttribute { .. } => RuleId::FormulaParseError,
        // Incompatible-operator rejection is a formula syntax error: the Rodin
        // formula parser folds it into the same diagnostic class.
        ParseError::IncompatibleOperators { .. } => RuleId::FormulaParseError,
        ParseError::AssignmentArityMismatch { .. } => RuleId::FormulaParseError,
        // A predicate written as an assignment gets its own rule (EB026) rather
        // than the generic formula error.
        ParseError::AssignmentInPredicate { .. } => RuleId::AssignmentInPredicate,
        _ => RuleId::XmlParseError,
    }
}

/// Same idea for [`rossi_build::Error`] surfaced by
/// `Project::from_zip_file` / `from_directory`.
fn rule_for_build_error(err: &rossi_build::Error) -> Option<RuleId> {
    match err {
        rossi_build::Error::Parse(p) => Some(rule_for_parse_error(p)),
        rossi_build::Error::Project(project) => match project.as_ref() {
            // `ProjectError::XmlAttribute` is raised on malformed-XML attribute
            // iteration (a quick-xml failure), not on a missing-but-required
            // attribute — that's EB003 only when surfaced via the rossi
            // parser's `MissingXmlAttribute`. Both XML-corruption variants
            // map to EB001 here.
            ProjectError::Xml(_) | ProjectError::XmlTag(_) | ProjectError::XmlAttribute(_) => {
                Some(RuleId::XmlParseError)
            }
            ProjectError::ReparseFormula { .. } => Some(RuleId::FormulaParseError),
            ProjectError::NotADirectory(_) => None,
        },
        rossi_build::Error::Io(_) | rossi_build::Error::Zip(_) => None,
    }
}

fn error_result(
    file: &Path,
    inner: Option<String>,
    message: String,
    rule_id: Option<RuleId>,
) -> ValidationResult {
    ValidationResult {
        file: file.to_path_buf(),
        success: false,
        inner_filename: inner,
        error: Some(message),
        component_type: None,
        component_name: None,
        severity: Some(Severity::Error),
        rule_id,
        origin: None,
        region: None,
    }
}

/// Resolve a loose-text parse error to a 1-indexed source [`Region`] (issue
/// #42). The start is the error's reported position; the end comes from its
/// byte span when present (a zero-width pest position yields a point region).
fn parse_error_region(err: &ParseError, source: &str) -> Option<Region> {
    let (start_line, start_column) = err.position()?;
    let (end_line, end_column) = match err.span() {
        Some(span) if span.end > span.start => line_col_1_indexed(source, span.end),
        _ => (start_line, start_column),
    };
    Some(Region {
        start_line,
        start_column,
        end_line,
        end_column,
    })
}

/// 1-indexed (line, column) of `byte_offset` in `source` — the SARIF/Camille
/// convention. [`Span::to_line_col`] reads only the start, so a point span at
/// the offset yields its position.
fn line_col_1_indexed(source: &str, byte_offset: usize) -> (usize, usize) {
    let (line, col) = rossi::ast::Span {
        start: byte_offset,
        end: byte_offset,
    }
    .to_line_col(source);
    (line + 1, col + 1)
}

/// Resolve an AST byte [`span`](rossi::ast::Span) to a 1-indexed source
/// [`Region`]. Both ends are mapped through `source`; a zero-width span yields
/// a point region.
fn span_to_region(source: &str, span: rossi::ast::Span) -> Region {
    let (start_line, start_column) = line_col_1_indexed(source, span.start);
    let (end_line, end_column) = line_col_1_indexed(source, span.end);
    Region {
        start_line,
        start_column,
        end_line,
        end_column,
    }
}

fn print_text_result(result: &ValidationResult) {
    let mut file_info = match &result.inner_filename {
        Some(inner) => format!("{}:{}", result.file.display(), inner),
        None => format!("{}", result.file.display()),
    };
    // Append the 1-indexed start position when known (issue #42).
    if let Some(region) = &result.region {
        file_info = format!("{file_info}:{}:{}", region.start_line, region.start_column);
    }

    if result.component_name.is_some() {
        println!(
            "✓ {} - Valid {} '{}'",
            file_info,
            result.component_type.unwrap_or("?"),
            result.component_name.as_deref().unwrap_or("?")
        );
        return;
    }

    let is_error = result.severity == Some(Severity::Error);
    let glyph = if is_error { "✗" } else { "!" };
    let prefix = result
        .rule_id
        .map(|r| format!("[{}] ", r.code()))
        .unwrap_or_default();
    let where_ = result
        .origin
        .as_deref()
        .map(|o| format!(" ({o})"))
        .unwrap_or_default();
    let message = result.error.as_deref().unwrap_or("");

    let line = format!("{glyph} {file_info}{where_} - {prefix}{message}");
    if is_error {
        eprintln!("{line}");
    } else {
        println!("{line}");
    }
}

fn print_summary(results: &[ValidationResult]) {
    let total = results.len();
    let passed = results.iter().filter(|r| r.success).count();
    let failed = total - passed;

    println!("\n{}", "=".repeat(50));
    println!("Summary:");
    println!("  Total:  {total}");
    println!("  Passed: {passed} ✓");
    println!("  Failed: {failed} ✗");
    println!("{}", "=".repeat(50));
}

fn write_json(results: &[ValidationResult], out: &mut impl Write) -> io::Result<()> {
    serde_json::to_writer_pretty(&mut *out, results)
        .map_err(|e| io::Error::new(e.io_error_kind().unwrap_or(io::ErrorKind::Other), e))?;
    writeln!(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ByteLimitWriter {
        remaining: usize,
    }

    impl Write for ByteLimitWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::other("write limit reached"));
            }
            let written = self.remaining.min(buf.len());
            self.remaining -= written;
            Ok(written)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn json_output_propagates_trailing_newline_failure() {
        let serialized = serde_json::to_vec_pretty(&Vec::<ValidationResult>::new()).unwrap();
        let mut out = ByteLimitWriter {
            remaining: serialized.len(),
        };

        let error = write_json(&[], &mut out).expect_err("the newline write must fail");

        assert_eq!(error.kind(), io::ErrorKind::Other);
    }

    #[test]
    fn output_failure_is_reported_and_exits_nonzero() {
        let mut stderr = Vec::new();

        let exit = finish_structured_output(
            Err(io::Error::other("write failed")),
            ExitCode::SUCCESS,
            &mut stderr,
        );

        assert_eq!(exit, ExitCode::from(1));
        assert_eq!(
            String::from_utf8(stderr).unwrap(),
            "rossi validate: failed to write output: write failed\n"
        );
    }

    #[test]
    fn broken_pipe_preserves_failed_validation_exit() {
        let mut stderr = Vec::new();
        let failed_validation = ExitCode::from(1);

        let exit = finish_structured_output(
            Err(io::Error::from(io::ErrorKind::BrokenPipe)),
            failed_validation,
            &mut stderr,
        );

        assert_eq!(exit, failed_validation);
        assert!(stderr.is_empty());
    }

    #[test]
    fn parse_error_region_covers_reserved_word() {
        // The reserved word `dom` carries a byte span (issue #42); the region's
        // end comes from that span, covering the whole word.
        let source = "CONTEXT c0\nCONSTANTS\n    dom\nEND\n";
        let err = rossi::parse(source).expect_err("`dom` is reserved");
        let region = parse_error_region(&err, source).expect("located error has a region");
        assert_eq!((region.start_line, region.start_column), (3, 5));
        assert_eq!((region.end_line, region.end_column), (3, 8));
    }

    #[test]
    fn parse_error_region_is_a_point_without_a_span() {
        // A bare pest position (zero-width span) yields a point region.
        let source = "CONTEXT c\nCONSTANTS\n    c1\n    +\nEND\n";
        let err = rossi::parse(source).expect_err("the stray `+` must fail");
        let region = parse_error_region(&err, source).expect("located error has a region");
        assert_eq!(
            (region.start_line, region.start_column),
            (region.end_line, region.end_column)
        );
    }

    #[test]
    fn span_to_region_maps_both_ends_one_indexed() {
        let source = "line one\nUnion x\n";
        let start = source.find("Union").unwrap();
        let region = span_to_region(
            source,
            rossi::ast::Span {
                start,
                end: start + "Union".len(),
            },
        );
        assert_eq!((region.start_line, region.start_column), (2, 1));
        assert_eq!((region.end_line, region.end_column), (2, 6));
    }

    #[test]
    fn loose_lint_diagnostic_is_positioned() {
        // The reported bug: a lint diagnostic must land on the declaration
        // line, not line 1. Validating the source directly resolves the span
        // the lint attached.
        let source = "CONTEXT C\nSETS\n    UNION\nEND\n";
        let components = rossi::parse_components(source).unwrap();
        let diag = rossi_build::lint::run_component(&components[0])
            .into_iter()
            .find(|d| d.rule_id == Some(RuleId::ShadowedName))
            .expect("UNION shadows the quantified-union token");
        let result = fold_diagnostic(Path::new("c.eventb"), diag, None, Some(source));
        let region = result.region.expect("region resolved from the lint span");
        assert_eq!((region.start_line, region.start_column), (3, 5));
        assert_eq!((region.end_line, region.end_column), (3, 10));
    }

    #[test]
    fn loose_text_flags_assignment_in_invariant() {
        // A `:=` in an invariant is a misplaced assignment: loose-text validate
        // reports it as EB026 (positioned on the operator), not a whole-file
        // EB004 Camille error.
        let source = "MACHINE M\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x := 5\nEND\n";
        let cli = ValidateArgs {
            files: vec![],
            format: OutputFormat::Text,
            quiet: false,
            continue_on_error: false,
            no_semantic: false,
            no_lints: false,
            stdin_filename: None,
        };
        let results = validate_text_source(Path::new("m.eventb"), source, &cli);
        let failures: Vec<_> = results.iter().filter(|r| !r.success).collect();
        assert_eq!(failures.len(), 1, "exactly one failure row: {results:?}");
        assert_eq!(failures[0].rule_id, Some(RuleId::AssignmentInPredicate));
        let region = failures[0].region.expect("EB026 carries a region");
        assert_eq!(region.start_line, 5, "operator is on the invariant line");
        assert!(
            failures[0]
                .error
                .as_deref()
                .is_some_and(|m| m.contains("assignment operator")),
            "message names the assignment operator: {:?}",
            failures[0].error
        );
    }

    #[test]
    fn loose_text_keeps_fallback_alongside_assignment_when_other_errors_exist() {
        // A file with BOTH a genuinely broken clause (`@inv1 y ∈` dangling) and a
        // misplaced assignment (`@inv2 x := 5`): the precise EB026 must surface,
        // but the co-occurring failure must not be silently dropped — the
        // whole-file EB004 fallback is retained so the user still sees it.
        let source = "MACHINE M\nVARIABLES\n    x\n    y\nINVARIANTS\n    @inv1 y ∈\n    @inv2 x := 5\nEND\n";
        let cli = ValidateArgs {
            files: vec![],
            format: OutputFormat::Text,
            quiet: false,
            continue_on_error: false,
            no_semantic: false,
            no_lints: false,
            stdin_filename: None,
        };
        let results = validate_text_source(Path::new("m.eventb"), source, &cli);
        assert!(
            results
                .iter()
                .any(|r| r.rule_id == Some(RuleId::AssignmentInPredicate)),
            "the misplaced assignment is reported as EB026: {results:?}"
        );
        assert!(
            results
                .iter()
                .any(|r| r.rule_id == Some(RuleId::CamilleParseError)),
            "the co-occurring parse error is not swallowed (EB004 fallback kept): {results:?}"
        );
    }

    #[test]
    fn rule_for_parse_error_maps_assignment_in_predicate() {
        let err = ParseError::AssignmentInPredicate {
            operator: ":=".to_string(),
            line: 1,
            column: 3,
            span: None,
        };
        assert_eq!(rule_for_parse_error(&err), RuleId::AssignmentInPredicate);
    }

    #[test]
    fn rule_for_parse_error_maps_assignment_arity_to_eb005() {
        let err = ParseError::AssignmentArityMismatch {
            targets: 2,
            expressions: 1,
            line: 1,
            column: 6,
            span: None,
        };
        assert_eq!(rule_for_parse_error(&err), RuleId::FormulaParseError);
    }

    #[test]
    fn recovered_assignment_arity_resolves_operator_in_later_component() {
        let source = concat!(
            "CONTEXT C\nEND\n",
            "MACHINE M\nVARIABLES x y\nEVENTS\n",
            "EVENT e\nTHEN\n",
            "@act1 x, y := 1\n",
            "END\nEND\n",
        );
        let recovered = parse_components_with_recovery(source);
        assert_eq!(
            recovered.errors.len(),
            1,
            "unexpected errors: {recovered:?}"
        );

        let (error, span) = precise_formula_error(&recovered.errors[0])
            .expect("recovery retains the precise arity cause");
        assert!(matches!(
            error,
            ParseError::AssignmentArityMismatch {
                targets: 2,
                expressions: 1,
                ..
            }
        ));
        let span = span.expect("recovered operator span is source-absolute");
        assert_eq!(&source[span.start..span.end], ":=");
    }

    #[test]
    fn project_diagnostic_resolves_component_source_and_file() {
        // A semantic diagnostic over an .eventb project is attributed to its
        // component: the region comes from that component's source and the
        // inner filename points the editor at the file.
        let source = "CONTEXT C\nCONSTANTS\n    k\nAXIOMS\n    @axm1 ⊤\nEND\n";
        let components = rossi_build::ProjectComponent::from_eventb("C.eventb", source).unwrap();
        let project = rossi_build::Project::new("p", components);
        let diag = rossi_build::build(&project)
            .diagnostics
            .into_iter()
            .find(|d| d.message.contains("could not infer type"))
            .expect("untyped constant is flagged");
        let result = fold_project_diagnostic(Path::new("proj"), diag, &project, "");
        assert_eq!(result.inner_filename.as_deref(), Some("C.eventb"));
        let region = result.region.expect("region from the component source");
        assert_eq!((region.start_line, region.start_column), (3, 5));
    }

    #[test]
    fn project_diagnostic_inner_filename_is_prefix_qualified() {
        // In a multi-project archive each row is namespaced by its project's
        // prefix so editors (and users) can tell sibling projects apart.
        let source = "CONTEXT C\nCONSTANTS\n    k\nAXIOMS\n    @axm1 ⊤\nEND\n";
        let components = rossi_build::ProjectComponent::from_eventb("C.buc", source).unwrap();
        let project = rossi_build::Project::new("Sub", components);
        let diag = rossi_build::build(&project)
            .diagnostics
            .into_iter()
            .find(|d| d.message.contains("could not infer type"))
            .expect("untyped constant is flagged");
        let result = fold_project_diagnostic(Path::new("proj"), diag, &project, "Sub/");
        assert_eq!(result.inner_filename.as_deref(), Some("Sub/C.buc"));
    }
}
