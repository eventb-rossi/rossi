//! Base snapshots and the 3-way merge that flows Rodin model edits back
//! into the Event-B text sources.
//!
//! Every successful build into the shared Rodin workspace records, under
//! `<workspace>/.base/<project>/`, a manifest (which component XML came from
//! which source file) and a verbatim snapshot of each source file *as
//! built*. When Rodin later edits a `.bum`/`.buc`, that snapshot is the
//! common ancestor of a classic 3-way merge:
//!
//! - **base**  — the source text at the last build (the snapshot);
//! - **ours**  — the source text now (editor buffer or disk);
//! - **theirs** — the base text with each semantically-changed component's
//!   span replaced by the re-imported, pretty-printed component (so hand
//!   formatting outside changed components survives).
//!
//! `ours == base` fast-forwards to theirs; otherwise `diffy` merges, falling
//! back to git-style conflict markers. Components whose re-imported XML is
//! semantically identical to the base (Rodin loves rewriting internal ids)
//! are recognized via normalized `to_xml` comparison and treated as
//! unchanged. Component *deletions* in Rodin are deliberately not synced —
//! deleting user-written text automatically is not worth the risk.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rossi::PrettyPrinter;
use serde::{Deserialize, Serialize};

/// Where a project's base snapshots live: `<workspace>/.base/<project>`.
pub fn base_dir(workspace_dir: &Path, project_name: &str) -> PathBuf {
    workspace_dir.join(".base").join(project_name)
}

/// The record of one build: which component XML files each source file
/// produced, and where the sources live.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaseManifest {
    pub project_name: String,
    /// Absolute path of the source directory the project was built from.
    pub source_root: PathBuf,
    /// Source file (relative) → its component XML filenames, in file order.
    /// The single source of truth for the file↔component relation; the
    /// inverse direction is answered by [`Self::source_for`].
    pub files: BTreeMap<PathBuf, Vec<String>>,
}

impl BaseManifest {
    /// The source file (relative to `source_root`) that produced a component
    /// XML file (e.g. `m.bum`), if this build knows it. Linear over a
    /// handful of entries.
    pub fn source_for(&self, xml_name: &str) -> Option<&Path> {
        self.files.iter().find_map(|(relative, xml_names)| {
            xml_names
                .iter()
                .any(|name| name == xml_name)
                .then_some(relative.as_path())
        })
    }
}

/// One source file as it went into a build.
pub struct SourceFileRecord {
    /// Path relative to the source root.
    pub relative: PathBuf,
    /// The exact text the build read (overlay or disk).
    pub text: String,
    /// Component XML filenames this file produced, in order.
    pub component_files: Vec<String>,
}

/// Write the manifest and per-source snapshots for a just-completed build.
pub fn write_base(
    workspace_dir: &Path,
    project_name: &str,
    source_root: &Path,
    sources: &[SourceFileRecord],
) -> std::io::Result<()> {
    let dir = base_dir(workspace_dir, project_name);
    let manifest = BaseManifest {
        project_name: project_name.to_string(),
        source_root: source_root.to_path_buf(),
        files: sources
            .iter()
            .map(|s| (s.relative.clone(), s.component_files.clone()))
            .collect(),
    };
    // Replace the whole base dir so snapshots of deleted sources don't linger.
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).map_err(std::io::Error::other)?,
    )?;
    for source in sources {
        let path = dir.join("sources").join(&source.relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, &source.text)?;
    }
    Ok(())
}

/// Load a project's manifest, if a build has recorded one.
pub fn load_manifest(workspace_dir: &Path, project_name: &str) -> Option<BaseManifest> {
    let bytes = std::fs::read(base_dir(workspace_dir, project_name).join("manifest.json")).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn base_source_path(workspace_dir: &Path, project_name: &str, relative: &Path) -> PathBuf {
    base_dir(workspace_dir, project_name)
        .join("sources")
        .join(relative)
}

/// Advance one source file's snapshot after its Rodin-side state has been
/// incorporated, so the next merge uses the right ancestor.
pub fn update_base_source(
    workspace_dir: &Path,
    project_name: &str,
    relative: &Path,
    text: &str,
) -> std::io::Result<()> {
    let path = base_source_path(workspace_dir, project_name, relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)
}

/// How one source file should change in response to Rodin's model edits.
#[derive(Debug, PartialEq, Eq)]
pub enum MergeOutcome {
    /// Every changed XML was semantically identical to the base — no edit.
    Unchanged,
    /// The source was untouched since the build; adopt Rodin's version.
    FastForward(String),
    /// Both sides changed; the 3-way merge was clean.
    Merged(String),
    /// Both sides changed the same region; git-style conflict markers.
    Conflict(String),
}

impl MergeOutcome {
    /// The text to apply, if any. For conflicts the marker-laden text is
    /// applied so the user can resolve it, but the base must *not* advance
    /// to it — marker text does not parse, and an unparseable base would
    /// permanently disable sync for the file.
    pub fn text(&self) -> Option<&str> {
        match self {
            MergeOutcome::Unchanged => None,
            MergeOutcome::FastForward(t) | MergeOutcome::Merged(t) | MergeOutcome::Conflict(t) => {
                Some(t)
            }
        }
    }
}

/// Recompute one source file from the Rodin project directory.
///
/// `ours` is the file's current text (editor buffer or disk); the base
/// snapshot and the project dir's component XML provide the other two sides.
/// `changed` is the set of component XML filenames the watcher saw change —
/// only those are re-imported and compared; the file's other components are
/// unchanged by construction (their own edits arrive in their own batches).
pub fn sync_source_file(
    manifest: &BaseManifest,
    workspace_dir: &Path,
    project_dir: &Path,
    relative: &Path,
    changed: &std::collections::BTreeSet<String>,
    ours: &str,
    printer: &PrettyPrinter,
) -> Result<MergeOutcome, String> {
    let base = std::fs::read_to_string(base_source_path(
        workspace_dir,
        &manifest.project_name,
        relative,
    ))
    .map_err(|e| format!("no base snapshot for {}: {e}", relative.display()))?;

    merge_source_file(manifest, relative, changed, &base, ours, printer, |name| {
        // Deleted in Rodin (or transiently unreadable): not synced.
        std::fs::read_to_string(project_dir.join(name)).ok()
    })
}

/// The merge itself, with every input supplied rather than read.
///
/// `sync_source_file` is the disk-backed caller: the base comes from the
/// snapshot and `xml_for` reads the project directory. A live edit has neither
/// on disk yet, so it passes its own ancestor and hands over the XML the
/// plug-in pushed. Everything here is pure, which is what lets the two share
/// one merge instead of growing a second one.
pub fn merge_source_file(
    manifest: &BaseManifest,
    relative: &Path,
    changed: &std::collections::BTreeSet<String>,
    base: &str,
    ours: &str,
    printer: &PrettyPrinter,
    xml_for: impl Fn(&str) -> Option<String>,
) -> Result<MergeOutcome, String> {
    let component_files = manifest
        .files
        .get(relative)
        .ok_or_else(|| format!("{} is not in the build manifest", relative.display()))?;

    let base_components = rossi::parse_components(base).map_err(|e| {
        format!(
            "base snapshot of {} does not parse: {e}",
            relative.display()
        )
    })?;

    // Gather the semantically-changed components: name → replacement text.
    let mut replaced: Vec<(&rossi::Component, String)> = Vec::new();
    let mut added: Vec<String> = Vec::new();
    for xml_name in component_files
        .iter()
        .filter(|name| changed.contains(*name))
    {
        let Some(xml) = xml_for(xml_name) else {
            continue;
        };
        let imported = rossi_build::ProjectComponent::from_xml(xml_name.clone(), &xml)
            .map_err(|e| format!("cannot import {xml_name}: {e}"))?;
        let base_match = base_components
            .iter()
            .find(|c| c.name() == imported.component.name());
        match base_match {
            Some(base_component)
                if normalized_xml(xml_name, base_component)
                    == rossi::to_xml(&imported.component) =>
            {
                // Semantically identical — Rodin only shuffled internals.
            }
            Some(base_component) => {
                replaced.push((base_component, printer.print_component(&imported.component)));
            }
            None => added.push(printer.print_component(&imported.component)),
        }
    }
    if replaced.is_empty() && added.is_empty() {
        return Ok(MergeOutcome::Unchanged);
    }

    // "Theirs": the base text with each changed component's span replaced by
    // its re-imported rendering (hand formatting elsewhere survives).
    let mut theirs = base.to_string();
    let mut splices: Vec<(rossi::ast::Span, String)> = Vec::new();
    for (base_component, new_text) in &replaced {
        match base_component.span() {
            Some(span) => splices.push((span, new_text.trim_end().to_string())),
            // No span (shouldn't happen for parsed text): re-render whole file.
            None => {
                return Err(format!(
                    "component {} in {} has no source span",
                    base_component.name(),
                    relative.display()
                ));
            }
        }
    }
    splices.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
    for (span, new_text) in splices {
        theirs.replace_range(span.start..span.end, &new_text);
    }
    for new_text in &added {
        if !theirs.ends_with('\n') {
            theirs.push('\n');
        }
        theirs.push('\n');
        theirs.push_str(new_text);
    }

    if ours == base {
        return Ok(MergeOutcome::FastForward(theirs));
    }
    match diffy::merge(base, ours, &theirs) {
        Ok(clean) => Ok(MergeOutcome::Merged(clean)),
        Err(conflicted) => Ok(MergeOutcome::Conflict(conflicted)),
    }
}

/// A text-parsed component's XML in the same normalized form an *imported*
/// component serializes to. One XML round-trip applies attribute-value
/// normalization (newlines inside comments become spaces), which the export
/// side emits literally — without this hop, every component with a
/// multi-line comment would falsely compare as changed.
fn normalized_xml(xml_name: &str, component: &rossi::Component) -> String {
    let exported = rossi::to_xml(component);
    match rossi_build::ProjectComponent::from_xml(xml_name.to_string(), &exported) {
        Ok(reimported) => rossi::to_xml(&reimported.component),
        Err(_) => exported,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    // The canonical (camille) shape the sync's printer emits, so a Rodin
    // edit re-rendered over an untouched source differs only where the
    // model changed.
    const BASE: &str = "context base_ctx\n\nconstants lo\n\naxioms\n  @axm1 lo ∈ ℤ\nend\n";

    /// A workspace with a recorded build of one source file (`model.eventb`
    /// holding `base_ctx`) and the matching project-dir XML.
    fn fixture(prefix: &str) -> (TempDir, PathBuf, BaseManifest) {
        let ws = TempDir::new(prefix);
        let project_dir = ws.path().join("proj");
        std::fs::create_dir_all(&project_dir).unwrap();
        let components = rossi::parse_components(BASE).unwrap();
        std::fs::write(
            project_dir.join("base_ctx.buc"),
            rossi::to_xml(&components[0]),
        )
        .unwrap();
        write_base(
            ws.path(),
            "proj",
            Path::new("/src"),
            &[SourceFileRecord {
                relative: PathBuf::from("model.eventb"),
                text: BASE.to_string(),
                component_files: vec!["base_ctx.buc".to_string()],
            }],
        )
        .unwrap();
        let manifest = load_manifest(ws.path(), "proj").unwrap();
        (ws, project_dir, manifest)
    }

    fn run(ws: &Path, project_dir: &Path, manifest: &BaseManifest, ours: &str) -> MergeOutcome {
        // The watcher batch in these scenarios is the one edited component.
        let changed = std::collections::BTreeSet::from(["base_ctx.buc".to_string()]);
        sync_source_file(
            manifest,
            ws,
            project_dir,
            Path::new("model.eventb"),
            &changed,
            ours,
            &PrettyPrinter::default(),
        )
        .unwrap()
    }

    #[test]
    fn manifest_round_trips() {
        let (_ws, _project_dir, manifest) = fixture("rossi-model-sync-manifest");
        assert_eq!(manifest.project_name, "proj");
        assert_eq!(manifest.source_root, Path::new("/src"));
        assert_eq!(
            manifest.source_for("base_ctx.buc"),
            Some(Path::new("model.eventb"))
        );
        assert_eq!(manifest.source_for("other.bum"), None);
        assert_eq!(
            manifest.files.get(Path::new("model.eventb")).unwrap(),
            &["base_ctx.buc"]
        );
    }

    #[test]
    fn semantically_identical_xml_is_unchanged() {
        let (ws, project_dir, manifest) = fixture("rossi-model-sync-noop");
        // Shuffle Rodin's internal element ids: still the same model.
        let xml = std::fs::read_to_string(project_dir.join("base_ctx.buc")).unwrap();
        std::fs::write(
            project_dir.join("base_ctx.buc"),
            xml.replace("name=\"", "name=\"rodin_rewrote_"),
        )
        .unwrap();

        assert_eq!(
            run(ws.path(), &project_dir, &manifest, BASE),
            MergeOutcome::Unchanged
        );
    }

    /// The project XML after Rodin renames `lo` to `hi`.
    fn rodin_edit(project_dir: &Path) {
        let edited = BASE.replace("lo", "hi");
        let components = rossi::parse_components(&edited).unwrap();
        std::fs::write(
            project_dir.join("base_ctx.buc"),
            rossi::to_xml(&components[0]),
        )
        .unwrap();
    }

    #[test]
    fn untouched_source_fast_forwards() {
        let (ws, project_dir, manifest) = fixture("rossi-model-sync-ff");
        rodin_edit(&project_dir);

        match run(ws.path(), &project_dir, &manifest, BASE) {
            MergeOutcome::FastForward(text) => {
                assert!(text.contains("hi ∈ ℤ"), "{text}");
                assert!(!text.contains("lo"), "{text}");
            }
            other => panic!("expected fast-forward, got {other:?}"),
        }
    }

    #[test]
    fn disjoint_edits_merge_cleanly() {
        let (ws, project_dir, manifest) = fixture("rossi-model-sync-merge");
        rodin_edit(&project_dir);
        // Local edit on a different line: a comment above the context.
        let ours = format!("// local note\n{BASE}");

        match run(ws.path(), &project_dir, &manifest, &ours) {
            MergeOutcome::Merged(text) => {
                assert!(text.contains("// local note"), "{text}");
                assert!(text.contains("hi ∈ ℤ"), "{text}");
            }
            other => panic!("expected clean merge, got {other:?}"),
        }
    }

    #[test]
    fn same_line_edits_conflict_with_markers() {
        let (ws, project_dir, manifest) = fixture("rossi-model-sync-conflict");
        rodin_edit(&project_dir);
        // Local edit renames the same constant differently.
        let ours = BASE.replace("lo", "local_name");

        match run(ws.path(), &project_dir, &manifest, &ours) {
            MergeOutcome::Conflict(text) => {
                assert!(text.contains("<<<<<<<"), "{text}");
                assert!(text.contains("local_name"), "{text}");
                assert!(text.contains("hi"), "{text}");
            }
            other => panic!("expected conflict, got {other:?}"),
        }
    }

    #[test]
    fn splice_preserves_text_outside_the_component() {
        let ws = TempDir::new("rossi-model-sync-splice");
        let project_dir = ws.path().join("proj");
        std::fs::create_dir_all(&project_dir).unwrap();
        // Two components in one file, hand-separated by a comment.
        let two = format!("{BASE}\n// separator comment\n\nMACHINE m\nSEES base_ctx\nEND\n");
        let components = rossi::parse_components(&two).unwrap();
        std::fs::write(
            project_dir.join("base_ctx.buc"),
            rossi::to_xml(&components[0]),
        )
        .unwrap();
        std::fs::write(project_dir.join("m.bum"), rossi::to_xml(&components[1])).unwrap();
        write_base(
            ws.path(),
            "proj",
            Path::new("/src"),
            &[SourceFileRecord {
                relative: PathBuf::from("model.eventb"),
                text: two.clone(),
                component_files: vec!["base_ctx.buc".to_string(), "m.bum".to_string()],
            }],
        )
        .unwrap();
        let manifest = load_manifest(ws.path(), "proj").unwrap();
        rodin_edit(&project_dir);

        match run(ws.path(), &project_dir, &manifest, &two) {
            MergeOutcome::FastForward(text) => {
                assert!(text.contains("// separator comment"), "{text}");
                assert!(text.contains("hi ∈ ℤ"), "{text}");
                assert!(text.contains("MACHINE m"), "{text}");
            }
            other => panic!("expected fast-forward, got {other:?}"),
        }
    }
}
