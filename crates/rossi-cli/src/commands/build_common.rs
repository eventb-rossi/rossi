//! Diagnostics reporting and exit gating shared by the commands that run the
//! static-check + POG pipeline (`rossi build`, `rossi export --build`).
//!
//! Both commands produce the same shape — one `(archive prefix, BuildResult)`
//! per project — and must report and gate on it identically, so a broken
//! project can never slide through CI under either entry point.

use rossi::NamedComponent;
use rossi_build::project::{DiscoveredProject, discover_projects};
use rossi_build::repack::repackage_zip_bytes_multi;
use rossi_build::rodin_ids::RodinIds;
use rossi_build::{BuildResult, Project, ProjectComponent, Severity, build};

use super::eventb_io::CmdResult;

/// Human-readable name for a project's archive prefix (the flat/root
/// project has an empty prefix).
pub(crate) fn project_label(prefix: &str) -> &str {
    if prefix.is_empty() {
        "(root)"
    } else {
        prefix.trim_end_matches('/')
    }
}

pub(crate) fn report_diagnostics(results: &[(String, BuildResult)]) {
    // A diagnostic's Display carries only the bare component name, so in a
    // multi-project archive (where sibling projects can share component names)
    // print a per-project header to disambiguate which project each came from.
    let multi = results.len() > 1;
    for (prefix, result) in results {
        if multi && !result.diagnostics.is_empty() {
            let label = project_label(prefix);
            eprintln!("--- {label} ---");
        }
        for d in &result.diagnostics {
            eprintln!("{d}");
        }
    }
}

/// Labels of the projects that failed outright — duplicate component names
/// (EB019) or a dependency cycle (EB007/EB008) — and so produced nothing.
pub(crate) fn failed_labels(results: &[(String, BuildResult)]) -> Vec<&str> {
    results
        .iter()
        .filter(|(_, r)| r.failed_outright())
        .map(|(prefix, _)| project_label(prefix))
        .collect()
}

/// Static-check each discovered project, returning one
/// `(archive prefix, BuildResult)` per project — the shared check step of
/// `rossi build` and `rossi export --build`, so prefixes and handle URIs can
/// never diverge between the two.
pub(crate) fn build_discovered(projects: Vec<DiscoveredProject>) -> Vec<(String, BuildResult)> {
    projects
        .into_iter()
        .map(|dp| (dp.prefix.clone(), build(&dp.into_project())))
        .collect()
}

/// [`build_discovered`] for a source archive whose projects are not yet
/// discovered — `rossi build`'s zip-input path.
pub(crate) fn build_archive_projects(
    zip_bytes: &[u8],
    fallback_name: &str,
) -> CmdResult<Vec<(String, BuildResult)>> {
    Ok(build_discovered(discover_projects(
        zip_bytes,
        fallback_name,
    )?))
}

/// Repackage the source archive with each project's checked files dropped
/// under its own prefix.
pub(crate) fn repack_results(
    src_bytes: &[u8],
    results: &[(String, BuildResult)],
) -> std::io::Result<Vec<u8>> {
    repackage_zip_bytes_multi(
        src_bytes,
        results
            .iter()
            .map(|(prefix, result)| (prefix.as_str(), result)),
    )
}

/// The pre-write decision: the outright-failed labels, and whether they
/// are every project, in which case there is nothing worth writing.
/// Healthy sibling projects in a multi-project archive still get their
/// output written.
pub(crate) fn write_decision(results: &[(String, BuildResult)]) -> (Vec<&str>, bool) {
    let failed = failed_labels(results);
    let nothing_to_write = !failed.is_empty() && failed.len() == results.len();
    (failed, nothing_to_write)
}

/// The pre-write gate of the human flow: [`write_decision`], with the
/// diagnostics reported on the way out when nothing will be written.
pub(crate) fn gate_before_write(results: &[(String, BuildResult)]) -> CmdResult<Vec<&str>> {
    let (failed, nothing_to_write) = write_decision(results);
    if nothing_to_write {
        report_diagnostics(results);
        return Err(NOTHING_WRITTEN.into());
    }
    Ok(failed)
}

/// Why a build wrote nothing at all.
pub(crate) const NOTHING_WRITTEN: &str = "no project produced checked output; see the diagnostics";

/// The post-write exit gate: fail on outright-failed sibling projects, then
/// on any error diagnostic, so a broken project cannot slide through CI.
/// `output_noun` names the written artifact in the message ("checked output"
/// for `rossi build`, "the output" for `rossi export --build`).
pub(crate) fn gate_after_write(
    results: &[(String, BuildResult)],
    failed: &[&str],
    output_noun: &str,
) -> CmdResult<()> {
    if !failed.is_empty() {
        return Err(format!(
            "project(s) {} produced no checked output; see the diagnostics above",
            failed.join(", ")
        )
        .into());
    }
    let errors = error_diagnostic_count(results);
    if errors > 0 {
        return Err(format!(
            "{errors} error diagnostic(s); {output_noun} was still written \
             (erroneous elements are dropped and their files marked inaccurate)"
        )
        .into());
    }
    Ok(())
}

/// The EB019 duplicate-component-name failure for one project, assembled
/// directly (serialising into a Rodin source archive is impossible — a
/// component's name is its entry filename), so the failure surfaces as the
/// SC's diagnostic instead of a zip-writer error. Shared by `rossi build`'s
/// text-input path and `rossi export --build`.
pub(crate) fn eb019_result(name: &str, components: Vec<NamedComponent>) -> BuildResult {
    let project = Project::new(
        name,
        components
            .into_iter()
            .map(|nc| ProjectComponent {
                filename: nc.filename,
                component: nc.component,
                rodin_ids: RodinIds::default(),
                source: None,
            })
            .collect(),
    );
    build(&project)
}

/// Total error diagnostics across all projects.
pub(crate) fn error_diagnostic_count(results: &[(String, BuildResult)]) -> usize {
    results
        .iter()
        .flat_map(|(_, r)| &r.diagnostics)
        .filter(|d| d.severity == Severity::Error)
        .count()
}
