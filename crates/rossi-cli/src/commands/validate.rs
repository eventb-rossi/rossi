use clap::{Args, ValueEnum};
use rossi::{Component, ParseError, parse_components, parse_zip_file_with_recovery};
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

    /// Skip rossi-build semantic checks (cycles, cross-refs, type errors).
    #[arg(long)]
    no_semantic: bool,

    /// Skip rossi-build advisory lint passes (dead variable, unmodified
    /// variable, dead constant, incomplete INIT, duplicate component,
    /// shadowed name).
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
            if !result.success {
                all_success = false;
                if !cli.continue_on_error && !aggregating_format {
                    print_text_result(&result, cli.quiet);
                    return ExitCode::from(1);
                }
            }

            if cli.format == OutputFormat::Text && !cli.quiet {
                print_text_result(&result, cli.quiet);
            }

            results.push(result);
        }
    }

    match cli.format {
        OutputFormat::Text => {
            if !cli.quiet && results.len() > 1 {
                print_summary(&results);
            }
        }
        OutputFormat::Json => write_json(&results, &mut io::stdout().lock()),
        OutputFormat::Sarif => {
            sarif::emit(&results, &mut io::stdout().lock()).expect("writing SARIF to stdout failed")
        }
    }

    if !all_success {
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
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
/// so only the component-local lints run here — the reference-based ones
/// need the project paths (directories, zip archives).
fn validate_text_source(display: &Path, source: &str, cli: &ValidateArgs) -> Vec<ValidationResult> {
    match parse_components(source) {
        Ok(components) => {
            let mut results: Vec<ValidationResult> = components
                .iter()
                .map(|c| success_result(display, None, c))
                .collect();
            if !cli.no_lints {
                for component in &components {
                    for diag in rossi_build::lint::run_component(component) {
                        results.push(fold_diagnostic(display, diag));
                    }
                }
            }
            results
        }
        // Loose `.eventb` text → Camille parse failure (EB004).
        Err(e) => {
            let mut result = error_result(
                display,
                None,
                format!("{e}"),
                Some(RuleId::CamilleParseError),
            );
            result.region = parse_error_region(&e, source);
            vec![result]
        }
    }
}

fn validate_zip_file(file: &Path, cli: &ValidateArgs) -> Vec<ValidationResult> {
    let parse_result = parse_zip_file_with_recovery(file);
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
        match Project::from_zip_file(file) {
            Ok(project) => fold_semantic(&project, file, cli, &mut results),
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
            fold_semantic(&project, dir, cli, &mut results);
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

fn fold_semantic(
    project: &Project,
    file: &Path,
    cli: &ValidateArgs,
    out: &mut Vec<ValidationResult>,
) {
    let build = rossi_build::build(project);
    for diag in build.diagnostics {
        out.push(fold_diagnostic(file, diag));
    }
    if !cli.no_lints {
        for diag in rossi_build::lint::run(project) {
            out.push(fold_diagnostic(file, diag));
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

fn fold_diagnostic(file: &Path, diag: Diagnostic) -> ValidationResult {
    ValidationResult {
        file: file.to_path_buf(),
        success: diag.severity != Severity::Error,
        inner_filename: None,
        error: Some(diag.message),
        component_type: None,
        component_name: None,
        severity: Some(diag.severity),
        rule_id: diag.rule_id,
        origin: Some(diag.origin),
        region: None,
    }
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

fn print_text_result(result: &ValidationResult, quiet: bool) {
    let mut file_info = match &result.inner_filename {
        Some(inner) => format!("{}:{}", result.file.display(), inner),
        None => format!("{}", result.file.display()),
    };
    // Append the 1-indexed start position when known (issue #42).
    if let Some(region) = &result.region {
        file_info = format!("{file_info}:{}:{}", region.start_line, region.start_column);
    }

    if result.component_name.is_some() {
        if !quiet {
            println!(
                "✓ {} - Valid {} '{}'",
                file_info,
                result.component_type.unwrap_or("?"),
                result.component_name.as_deref().unwrap_or("?")
            );
        }
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

fn write_json(results: &[ValidationResult], out: &mut impl Write) {
    if let Err(e) = serde_json::to_writer_pretty(&mut *out, results) {
        eprintln!("failed to serialize JSON: {e}");
        return;
    }
    let _ = writeln!(out);
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
