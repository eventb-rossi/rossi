//! Build a text source directory into the shared Rodin project directory.
//!
//! Mirrors the CLI's `rossi build <dir> -o <dir>` pipeline (parse text →
//! serialize → static-check → reconcile → write), with two differences owed
//! to living inside the language server: unsaved editor buffers overlay the
//! on-disk sources, and the destination is a *persistent* Rodin project the
//! user proves in — so generated `.bpo`/`.bps` reconcile against whatever is
//! on disk there (Rodin's own stamps and statuses carry over for unchanged
//! obligations) and `.bpr` proof files are never written, pruned, or touched.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rossi::{NamedComponent, parse_named_components, write_project_directory};
use rossi_build::pog::reconcile::reconcile_build_files;
use rossi_build::project::{duplicate_component_name, project_from_text_components};
use rossi_build::{Severity, build, is_normal_path_component};

/// What a project build left behind, for user-facing reporting.
#[derive(Debug)]
pub struct BuildOutcome {
    /// Rendered error diagnostics. Non-empty means the checked output was
    /// still written, with erroneous elements dropped (Rodin semantics).
    pub error_diagnostics: Vec<String>,
    /// Path and content hash of every file this build wrote, for the sync
    /// watcher's echo guard (see [`super::sync`]).
    pub written: Vec<(PathBuf, u64)>,
}

/// Text buffers overlaying the filesystem, keyed by canonicalized path.
pub type Overlay = HashMap<PathBuf, String>;

/// Collect the Event-B sources under `dir` (recursively, skipping
/// dot-directories such as the `.rossi` workspace itself), sorted.
///
/// Filters exactly as the LSP's workspace scan does. The two must agree: a
/// file this built into a Rodin project but the index never saw would be
/// invisible to cross-file diagnostics, so a sibling `SEES` of its components
/// reports EB009 while the build succeeds.
pub fn collect_source_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in rossi_build::walk::source_walk(dir) {
        let entry = entry.map_err(std::io::Error::other)?;
        if entry.file_type().is_file() && rossi_build::walk::is_source_file(entry.path()) {
            files.push(entry.path().to_path_buf());
        }
    }
    files.sort();
    Ok(files)
}

/// Read a source file, preferring the editor's in-memory text when the file
/// is open (matched via canonicalized paths).
fn read_with_overlay(path: &Path, overlay: &Overlay) -> std::io::Result<String> {
    if overlay.is_empty() {
        return std::fs::read_to_string(path);
    }
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Some(text) = overlay.get(&canonical) {
        return Ok(text.clone());
    }
    std::fs::read_to_string(path)
}

/// Build every Event-B text source under `source_dir` into the Rodin project
/// at `project_dir`: sources + `.project` via the exporter, checked files and
/// proof obligations via the static checker, `.bpo`/`.bps` reconciled against
/// the files being replaced so proof state Rodin wrote there survives.
pub fn build_rodin_project(
    source_dir: &Path,
    overlay: &Overlay,
    project_dir: &Path,
    project_name: &str,
) -> Result<BuildOutcome, String> {
    let sources = collect_source_files(source_dir)
        .map_err(|e| format!("cannot scan {}: {e}", source_dir.display()))?;
    if sources.is_empty() {
        return Err(format!(
            "no Event-B files found in {}",
            source_dir.display()
        ));
    }

    let mut components: Vec<NamedComponent> = Vec::new();
    let mut source_records: Vec<super::model_sync::SourceFileRecord> = Vec::new();
    for path in &sources {
        let text = read_with_overlay(path, overlay)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let named = parse_named_components(&text)
            .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
        source_records.push(super::model_sync::SourceFileRecord {
            relative: path.strip_prefix(source_dir).unwrap_or(path).to_path_buf(),
            text,
            component_files: named.iter().map(|nc| nc.filename.clone()).collect(),
        });
        components.extend(named);
    }

    // A duplicate component name cannot be serialized (its name is its
    // filename) and the project is invalid regardless; fail before touching
    // the destination.
    if let Some(name) = duplicate_component_name(&components) {
        return Err(format!(
            "duplicate component name '{name}' across the source files"
        ));
    }

    let (_bytes, project) = project_from_text_components(project_name, &components)
        .map_err(|e| format!("cannot assemble project: {e}"))?;
    let result = build(&project);
    if result.failed_outright() {
        let rendered: Vec<String> = result.diagnostics.iter().map(|d| d.to_string()).collect();
        return Err(format!(
            "the project produced no checked output:\n{}",
            rendered.join("\n")
        ));
    }

    // Write sources + `.project` first so the generated files land next to
    // them; the exporter creates the directory.
    write_project_directory(project_dir, &components, project_name)
        .map_err(|e| format!("cannot write project into {}: {e}", project_dir.display()))?;

    let mut files = result.files;
    for f in &files {
        if !is_normal_path_component(&f.filename) {
            return Err(format!("unsafe generated filename {:?}", f.filename));
        }
    }
    // Previous state comes from the destination — the files being replaced —
    // which is where Rodin recorded its stamps and proof statuses.
    //
    // Deliberately no status update pass here, unlike repack and the CLI
    // flows: this destination only ever feeds a running editor (the
    // save rebuild is gated on one holding the workspace, and the open
    // command
    // launches one), whose own builder recomputes every stale row the
    // moment these files land. Recomputing here would duplicate that work
    // inside the save loop — minutes on the largest models — and race
    // its own `.bps` writes. The stale stamps left by reconciliation
    // are exactly the signal its updater keys on.
    reconcile_build_files(&mut files, |name| {
        std::fs::read_to_string(project_dir.join(name)).ok()
    });
    // Hash generated files from the in-memory contents as they are written;
    // only the exporter-written files (whose bytes the exporter kept to
    // itself) are read back below.
    let mut written: Vec<(PathBuf, u64)> = Vec::with_capacity(files.len() + components.len() + 1);
    for f in &files {
        let path = project_dir.join(&f.filename);
        std::fs::write(&path, &f.contents)
            .map_err(|e| format!("cannot write {}: {e}", f.filename))?;
        written.push((path, super::sync::content_hash(f.contents.as_bytes())));
    }

    // The previous build's manifest (read before `write_base` replaces it)
    // is the record of which files *we* derived from sources — the only
    // files pruning may touch. Anything else in the project directory is
    // Rodin's (e.g. a machine created in the Rodin UI) and must survive.
    let previous_manifest = project_dir
        .parent()
        .and_then(|workspace_dir| super::model_sync::load_manifest(workspace_dir, project_name));
    prune_stale_generated_files(project_dir, &components, &files, previous_manifest.as_ref());

    // Record base snapshots for the Rodin→source model-edit sync. Best
    // effort: without a base, edits made in Rodin simply stay in Rodin.
    if let Some(workspace_dir) = project_dir.parent() {
        let source_root =
            std::fs::canonicalize(source_dir).unwrap_or_else(|_| source_dir.to_path_buf());
        if let Err(e) = super::model_sync::write_base(
            workspace_dir,
            project_name,
            &source_root,
            &source_records,
        ) {
            tracing::info!("could not record base snapshots: {e}");
        }
    }

    // Hash the exporter-written files (sources, descriptor) too, so the sync
    // watcher can drop the echoes of every one of our own writes.
    written.extend(
        std::iter::once(".project".to_string())
            .chain(components.iter().map(|nc| nc.filename.clone()))
            .filter_map(|name| {
                let path = project_dir.join(&name);
                let bytes = std::fs::read(&path).ok()?;
                Some((path, super::sync::content_hash(&bytes)))
            }),
    );

    Ok(BuildOutcome {
        error_diagnostics: result
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| d.to_string())
            .collect(),
        written,
    })
}

/// Remove generated/source files a previous build wrote for components that
/// no longer exist (renamed or deleted). Only files whose component the
/// *previous manifest* attributes to our own sources are candidates — a
/// component the user created inside Rodin (never in any manifest) is
/// Rodin's data and must never be deleted, and `.bpr` proofs are never
/// touched regardless. Without a previous manifest nothing is pruned.
/// Best effort.
fn prune_stale_generated_files(
    project_dir: &Path,
    components: &[NamedComponent],
    generated: &[rossi_build::ScFile],
    previous_manifest: Option<&super::model_sync::BaseManifest>,
) {
    let Some(previous) = previous_manifest else {
        return;
    };
    // Component names (XML stems) earlier builds derived from the sources.
    let own_stems: std::collections::BTreeSet<&str> = previous
        .files
        .values()
        .flatten()
        .filter_map(|xml_name| Path::new(xml_name).file_stem().and_then(|s| s.to_str()))
        .collect();
    let expected: std::collections::BTreeSet<&str> = components
        .iter()
        .map(|nc| nc.filename.as_str())
        .chain(generated.iter().map(|f| f.filename.as_str()))
        .collect();
    let Ok(entries) = std::fs::read_dir(project_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ours = path
            .file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|stem| own_stems.contains(stem));
        let stale =
            ours && matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("buc" | "bum" | "bcc" | "bcm" | "bpo" | "bps")
            ) && path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| !expected.contains(name));
        if stale && let Err(e) = std::fs::remove_file(&path) {
            tracing::info!("could not prune stale {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    const CTX: &str = "CONTEXT base_ctx\nCONSTANTS\n    lo\nAXIOMS\n    @axm1 lo ∈ ℤ\nEND\n";

    #[test]
    fn builds_a_full_rodin_project_from_text() {
        let root = TempDir::new("rossi-rodin-build-test");
        let src = root.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("base_ctx.eventb"), CTX).unwrap();
        let project = root.path().join("ws").join("src");

        let outcome =
            build_rodin_project(&src, &Overlay::new(), &project, "src").expect("build succeeds");

        assert!(outcome.error_diagnostics.is_empty());
        assert!(project.join(".project").is_file());
        assert!(
            project
                .join(".settings/org.eclipse.core.resources.prefs")
                .is_file()
        );
        assert!(project.join("base_ctx.buc").is_file());
        assert!(project.join("base_ctx.bcc").is_file());
        assert!(project.join("base_ctx.bpo").is_file());
        assert!(project.join("base_ctx.bps").is_file());
    }

    #[test]
    fn overlay_text_wins_over_disk() {
        let root = TempDir::new("rossi-rodin-overlay-test");
        let src = root.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let file = src.join("base_ctx.eventb");
        std::fs::write(&file, CTX).unwrap();
        let project = root.path().join("ws").join("src");

        let mut overlay = Overlay::new();
        overlay.insert(
            std::fs::canonicalize(&file).unwrap(),
            CTX.replace("base_ctx", "buffer_ctx"),
        );
        build_rodin_project(&src, &overlay, &project, "src").expect("build succeeds");

        assert!(project.join("buffer_ctx.buc").is_file());
        assert!(!project.join("base_ctx.buc").exists());
    }

    #[test]
    fn every_written_file_is_hashed_for_the_echo_guard() {
        let root = TempDir::new("rossi-rodin-hash-test");
        let src = root.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("base_ctx.eventb"), CTX).unwrap();
        let project = root.path().join("ws").join("src");

        let outcome = build_rodin_project(&src, &Overlay::new(), &project, "src").unwrap();

        for (path, hash) in &outcome.written {
            let bytes = std::fs::read(path)
                .unwrap_or_else(|e| panic!("{} was reported written: {e}", path.display()));
            assert_eq!(
                super::super::sync::content_hash(&bytes),
                *hash,
                "{} hash must match the bytes on disk",
                path.display()
            );
        }
        let names: Vec<&str> = outcome
            .written
            .iter()
            .filter_map(|(p, _)| p.file_name().and_then(|n| n.to_str()))
            .collect();
        for expected in [".project", "base_ctx.buc", "base_ctx.bpo", "base_ctx.bps"] {
            assert!(
                names.contains(&expected),
                "{expected} missing from {names:?}"
            );
        }
    }

    #[test]
    fn rebuild_prunes_stale_components_but_never_proofs() {
        let root = TempDir::new("rossi-rodin-prune-test");
        let src = root.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let file = src.join("model.eventb");
        std::fs::write(&file, CTX).unwrap();
        let project = root.path().join("ws").join("src");

        build_rodin_project(&src, &Overlay::new(), &project, "src").unwrap();
        std::fs::write(project.join("base_ctx.bpr"), "PROOFS").unwrap();

        // Rename the component; the old component's files must be pruned,
        // while the `.bpr` (even the now-orphaned one) stays untouched.
        std::fs::write(&file, CTX.replace("base_ctx", "renamed_ctx")).unwrap();
        build_rodin_project(&src, &Overlay::new(), &project, "src").unwrap();

        assert!(project.join("renamed_ctx.buc").is_file());
        assert!(!project.join("base_ctx.buc").exists());
        assert!(!project.join("base_ctx.bpo").exists());
        assert_eq!(
            std::fs::read_to_string(project.join("base_ctx.bpr")).unwrap(),
            "PROOFS"
        );
    }

    #[test]
    fn rebuild_never_prunes_components_created_inside_rodin() {
        let root = TempDir::new("rossi-rodin-foreign-test");
        let src = root.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("model.eventb"), CTX).unwrap();
        let project = root.path().join("ws").join("src");

        build_rodin_project(&src, &Overlay::new(), &project, "src").unwrap();
        // The user creates a machine inside Rodin: it exists only in the
        // project directory, never in any manifest or text source.
        std::fs::write(project.join("rodin_made.bum"), "<machineFile/>").unwrap();
        std::fs::write(project.join("rodin_made.bpo"), "<poFile/>").unwrap();

        build_rodin_project(&src, &Overlay::new(), &project, "src").unwrap();

        assert!(
            project.join("rodin_made.bum").is_file(),
            "a Rodin-authored component must survive rebuilds"
        );
        assert!(project.join("rodin_made.bpo").is_file());
    }

    #[test]
    fn rebuild_preserves_reconciled_statuses() {
        let root = TempDir::new("rossi-rodin-reconcile-test");
        let src = root.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        // A machine with an invariant yields at least one obligation row.
        std::fs::write(
            src.join("m.eventb"),
            "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ∈ ℕ\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x ≔ 0\n    END\nEND\n",
        )
        .unwrap();
        let project = root.path().join("ws").join("src");

        build_rodin_project(&src, &Overlay::new(), &project, "src").unwrap();
        let bps = project.join("m.bps");
        let first = std::fs::read_to_string(&bps).unwrap();
        // Doctor one status row the way a prover run would: mark it discharged.
        let doctored = first.replace("confidence=\"-99\"", "confidence=\"1000\"");
        assert_ne!(first, doctored, "fixture must contain an unattempted row");
        std::fs::write(&bps, &doctored).unwrap();

        build_rodin_project(&src, &Overlay::new(), &project, "src").unwrap();
        assert_eq!(
            std::fs::read_to_string(&bps).unwrap(),
            doctored,
            "an unchanged model must carry its proof statuses across rebuilds"
        );
    }

    #[test]
    fn duplicate_component_names_fail_before_writing() {
        let root = TempDir::new("rossi-rodin-dup-test");
        let src = root.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.eventb"), "MACHINE M\nEND\n").unwrap();
        std::fs::write(src.join("b.eventb"), "MACHINE M\nEND\n").unwrap();
        let project = root.path().join("ws").join("src");

        let err = build_rodin_project(&src, &Overlay::new(), &project, "src").unwrap_err();
        assert!(err.contains("duplicate component name"), "{err}");
        assert!(!project.exists(), "nothing may be written on failure");
    }
}
