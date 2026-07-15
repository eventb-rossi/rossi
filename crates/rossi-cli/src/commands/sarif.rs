//! SARIF 2.1.0 emitter for `rossi validate`.
//!
//! Spec: <https://docs.oasis-open.org/sarif/sarif/v2.1.0/sarif-v2.1.0.html>.
//! Only the subset relevant to a single-driver validator is emitted (no
//! conversions, no graphs, no taxonomies).

use rossi_build::{RuleId, Severity};
use serde_json::{Value, json};
use std::io::{self, Write};

use crate::commands::validate::{Region, ValidationResult};

const SCHEMA_URI: &str = "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/master/Schemata/sarif-schema-2.1.0.json";

const INFORMATION_URI: &str = "https://github.com/eventb-rossi/rossi";

/// Serialise `results` as a SARIF 2.1.0 document and write it to `out`.
pub fn emit(results: &[ValidationResult], mut out: impl Write) -> io::Result<()> {
    let doc = build_document(results);
    serde_json::to_writer_pretty(&mut out, &doc)
        .map_err(|e| io::Error::new(e.io_error_kind().unwrap_or(io::ErrorKind::Other), e))?;
    writeln!(out)?;
    Ok(())
}

fn build_document(results: &[ValidationResult]) -> Value {
    let rules: Vec<Value> = RuleId::all().iter().map(|r| rule_descriptor(*r)).collect();
    let sarif_results: Vec<Value> = results.iter().filter_map(result_to_sarif).collect();

    json!({
        "$schema": SCHEMA_URI,
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "rossi",
                    "version": env!("CARGO_PKG_VERSION"),
                    "informationUri": INFORMATION_URI,
                    "rules": rules,
                }
            },
            "results": sarif_results,
        }]
    })
}

fn rule_descriptor(rule: RuleId) -> Value {
    json!({
        "id": rule.code(),
        "name": rule.name(),
        "shortDescription": { "text": rule.name() },
        "fullDescription": { "text": rule.help() },
        "defaultConfiguration": { "level": sarif_level(rule.default_severity()) },
    })
}

fn result_to_sarif(result: &ValidationResult) -> Option<Value> {
    let rule = result.rule_id?;
    let level = sarif_level(result.severity.unwrap_or(Severity::Warning));
    let message = result.error.clone().unwrap_or_default();
    let uri = uri_for(result);

    let mut location = json!({
        "physicalLocation": {
            "artifactLocation": { "uri": uri }
        }
    });
    if let Some(region) = &result.region {
        location["physicalLocation"]["region"] = region_to_sarif(region);
    }
    if let Some(origin) = &result.origin {
        location["logicalLocations"] = json!([{ "name": origin }]);
    }

    Some(json!({
        "ruleId": rule.code(),
        "level": level,
        "message": { "text": message },
        "locations": [location],
    }))
}

/// A SARIF `region` object (1-indexed lines/columns, character units).
fn region_to_sarif(region: &Region) -> Value {
    json!({
        "startLine": region.start_line,
        "startColumn": region.start_column,
        "endLine": region.end_line,
        "endColumn": region.end_column,
    })
}

fn uri_for(result: &ValidationResult) -> String {
    let base = result.file.display().to_string();
    match &result.inner_filename {
        Some(inner) => format!("{base}!/{inner}"),
        None => base,
    }
}

fn sarif_level(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "note",
    }
}
