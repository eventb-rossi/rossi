//! Watch the shared Rodin workspace and feed Rodin's results back.
//!
//! Rodin writes proof evidence (`.bpr` proofs, `.bps` statuses) into the
//! project directories rossi builds under `<root>/.rossi/rodin`. This
//! watcher notices those writes and refreshes the per-component proof-status
//! overlay the analyzer publishes as informational diagnostics — so a proof
//! discharged in Rodin shows up in the editor moments later, without any
//! manual sync step.
//!
//! Echo-loop prevention: every file the server itself writes into the
//! workspace is recorded (path → content hash) in the shared written-file
//! map; watcher events whose on-disk content still matches that hash are the
//! server's own writes bouncing back and are dropped.
//!
//! All failure modes degrade quietly: if the watcher cannot start or an
//! event cannot be processed, proof state simply refreshes on the next
//! successful build instead. Nothing here surfaces errors to the user.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::Watcher;
use parking_lot::Mutex;

use crate::server::Analyzer;

/// Files the server itself wrote into the workspace (path → content hash),
/// plus a count of builds currently writing. Shared between the build flow
/// (writer) and the watcher (reader): the watcher defers classifying a
/// batch while a build is in flight, because the build's hashes only land
/// once it returns — classifying earlier would mistake the build's own
/// writes for Rodin's and merge them back into the sources.
#[derive(Default)]
pub struct WriteRegistry {
    hashes: Mutex<HashMap<PathBuf, u64>>,
    builds_in_flight: std::sync::atomic::AtomicUsize,
}

impl WriteRegistry {
    /// Record files written by a completed build.
    pub fn record(&self, entries: impl IntoIterator<Item = (PathBuf, u64)>) {
        self.hashes.lock().extend(entries);
    }

    fn recorded_hash(&self, path: &Path) -> Option<u64> {
        self.hashes.lock().get(path).copied()
    }

    /// Whether a build is currently writing into the workspace.
    pub fn building(&self) -> bool {
        self.builds_in_flight
            .load(std::sync::atomic::Ordering::SeqCst)
            > 0
    }
}

/// Mark a build as writing into the workspace until the guard drops.
pub fn begin_build(registry: &WrittenFiles) -> BuildGuard {
    registry
        .builds_in_flight
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    BuildGuard(Arc::clone(registry))
}

/// See [`begin_build`].
pub struct BuildGuard(Arc<WriteRegistry>);

impl Drop for BuildGuard {
    fn drop(&mut self) {
        self.0
            .builds_in_flight
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// The shared write registry (see [`WriteRegistry`]).
pub type WrittenFiles = Arc<WriteRegistry>;

/// Quiet period after the last filesystem event before a batch is processed;
/// Rodin saves in bursts (proof file + status + builder outputs).
const DEBOUNCE: Duration = Duration::from_millis(500);

/// Hash used for the written-file echo guard. Stability across runs is not
/// needed — the map lives in memory only.
pub fn content_hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// A running watcher over one Rodin workspace directory. Dropping it stops
/// both the filesystem watcher and the processing task.
pub struct RodinSyncManager {
    workspace_dir: PathBuf,
    _watcher: notify::RecommendedWatcher,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for RodinSyncManager {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl RodinSyncManager {
    /// The workspace this manager watches.
    pub fn workspace_dir(&self) -> &Path {
        &self.workspace_dir
    }

    /// Start watching `workspace_dir`. Performs one initial proof-status
    /// scan so existing Rodin results surface without waiting for a change.
    ///
    /// Watcher creation can take *minutes* when the platform's file-event
    /// service is busy (macOS fseventsd on a churning volume), so callers
    /// must invoke this off the request path — see the server's
    /// `ensure_rodin_sync`, which runs it on a detached thread. `handle` is
    /// the runtime the processing task should run on.
    pub(crate) fn start(
        handle: &tokio::runtime::Handle,
        workspace_dir: PathBuf,
        written: WrittenFiles,
        analyzer: Analyzer,
    ) -> notify::Result<Self> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PathBuf>();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Ok(event) = event {
                    for path in event.paths {
                        let _ = tx.send(path);
                    }
                }
            })?;
        watcher.watch(&workspace_dir, notify::RecursiveMode::Recursive)?;

        let task_workspace = workspace_dir.clone();
        let task = handle.spawn(async move {
            // Initial scan: surface proof state Rodin left from earlier runs.
            refresh(&task_workspace, &analyzer, None).await;

            // Catch up on model edits Rodin saved while no server was
            // watching: run every manifest-known XML through the same merge
            // flow the watcher events use. For clean projects the semantic
            // -identity check reduces this to a no-op; a genuinely diverged
            // component merges its whole accumulated change (the flow is
            // state-based, not event-based). Without this pass, such an edit
            // would sit unmerged until the next rebuild overwrote it. Builds
            // already writing into the workspace are waited out first — the
            // same deferral the event loop applies.
            while written.building() {
                tokio::time::sleep(DEBOUNCE).await;
            }
            let batch = startup_model_batch(&task_workspace);
            if !batch.is_empty() {
                sync_model_edits(&task_workspace, batch, &analyzer).await;
            }

            let mut pending: BTreeSet<PathBuf> = BTreeSet::new();
            loop {
                tokio::select! {
                    received = rx.recv() => {
                        match received {
                            Some(path) => { pending.insert(path); }
                            None => break,
                        }
                    }
                    // Restarted on every event above, so this fires only
                    // after DEBOUNCE of quiet — one refresh per burst.
                    _ = tokio::time::sleep(DEBOUNCE), if !pending.is_empty() => {
                        // A build is writing into the workspace: its hashes
                        // land only when it returns, so classifying now
                        // would mistake its writes for Rodin's. Leave the
                        // batch pending and retry after the next quiet gap.
                        if written.building() {
                            continue;
                        }
                        let batch = std::mem::take(&mut pending);
                        let changes = classify_batch(&task_workspace, &batch, &written);
                        if !changes.model.is_empty() {
                            sync_model_edits(&task_workspace, changes.model, &analyzer).await;
                        }
                        if !changes.proof_projects.is_empty() {
                            refresh(&task_workspace, &analyzer, Some(changes.proof_projects)).await;
                        }
                    }
                }
            }
        });

        Ok(Self {
            workspace_dir,
            _watcher: watcher,
            task,
        })
    }
}

/// What a debounced event batch means for the sync flows, gathered in one
/// pass: component XML files changed by someone other than the server
/// (grouped as project directory + XML filename), and the project
/// directories with foreign proof-state changes. Only files directly inside
/// a project directory (a direct child of the workspace) count — that is
/// where Rodin keeps everything the downstream scans read.
struct BatchChanges {
    model: Vec<(PathBuf, String)>,
    proof_projects: BTreeSet<PathBuf>,
}

fn classify_batch(
    workspace_dir: &Path,
    batch: &BTreeSet<PathBuf>,
    written: &WrittenFiles,
) -> BatchChanges {
    let mut changes = BatchChanges {
        model: Vec::new(),
        proof_projects: BTreeSet::new(),
    };
    for path in batch {
        let Some(project_dir) = path.parent() else {
            continue;
        };
        if project_dir.parent() != Some(workspace_dir) {
            continue;
        }
        match path.extension().and_then(|e| e.to_str()) {
            Some("bum" | "buc") => {
                if path.is_file()
                    && !is_own_write(path, written)
                    && let Some(name) = path.file_name().and_then(|n| n.to_str())
                {
                    changes
                        .model
                        .push((project_dir.to_path_buf(), name.to_string()));
                }
            }
            // A deleted proof file (unreadable → not our write) counts too:
            // removing a proof in Rodin must clear its overlay line.
            Some("bpr" | "bps") if !is_own_write(path, written) => {
                changes.proof_projects.insert(project_dir.to_path_buf());
            }
            _ => {}
        }
    }
    changes
}

/// Every manifest-known component XML file currently present in the
/// workspace's projects, shaped like a watcher batch — the startup catch-up
/// input. Projects without a base manifest (never built by this server) are
/// skipped: there is no ancestor to merge against. XML files the manifest
/// does not know (components authored in Rodin) are skipped for the same
/// reason, and files the manifest knows but Rodin deleted are skipped
/// because deletions are deliberately not synced.
fn startup_model_batch(workspace_dir: &Path) -> Vec<(PathBuf, String)> {
    let mut batch = Vec::new();
    let Ok(entries) = std::fs::read_dir(workspace_dir) else {
        return batch;
    };
    for entry in entries.flatten() {
        let project_dir = entry.path();
        let Some(name) = project_dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') || !project_dir.is_dir() {
            continue;
        }
        let Some(manifest) = super::model_sync::load_manifest(workspace_dir, name) else {
            continue;
        };
        for xml_name in manifest.files.values().flatten() {
            if project_dir.join(xml_name).is_file() {
                batch.push((project_dir.clone(), xml_name.clone()));
            }
        }
    }
    batch
}

/// A path whose on-disk content still hashes to what the server last wrote
/// there is the server's own write echoing back through the watcher.
fn is_own_write(path: &Path, written: &WrittenFiles) -> bool {
    let Some(expected) = written.recorded_hash(path) else {
        return false;
    };
    match std::fs::read(path) {
        Ok(bytes) => content_hash(&bytes) == expected,
        Err(_) => false,
    }
}

/// Flow Rodin's model edits back into the Event-B sources: for each affected
/// source file, 3-way merge (base snapshot / current text / re-imported
/// components) and apply the result — see [`super::model_sync`].
async fn sync_model_edits(
    workspace_dir: &Path,
    changes: Vec<(PathBuf, String)>,
    analyzer: &Analyzer,
) {
    use super::model_sync::{self, MergeOutcome};
    use crate::lsp_types::MessageType;

    // Group the changed XML files per project — loading each project's
    // manifest exactly once — and per source file within the project. A
    // project whose manifest is missing is remembered as `None` so a burst
    // of its events logs (and re-reads) only once.
    type FileChanges = BTreeMap<PathBuf, BTreeSet<String>>;
    type ProjectEntry = Option<(Arc<model_sync::BaseManifest>, FileChanges)>;
    let mut by_project: BTreeMap<PathBuf, ProjectEntry> = BTreeMap::new();
    for (project_dir, xml_name) in changes {
        let slot = by_project.entry(project_dir.clone()).or_insert_with(|| {
            let manifest = project_dir
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|name| model_sync::load_manifest(workspace_dir, name));
            match manifest {
                Some(manifest) => Some((Arc::new(manifest), FileChanges::new())),
                None => {
                    tracing::info!(
                        "no build manifest for {}; Rodin edit not synced",
                        project_dir.display()
                    );
                    None
                }
            }
        });
        let Some((manifest, files)) = slot else {
            continue;
        };
        let Some(relative) = manifest.source_for(&xml_name) else {
            tracing::info!("{xml_name} is not in the build manifest; Rodin edit not synced");
            continue;
        };
        files
            .entry(relative.to_path_buf())
            .or_default()
            .insert(xml_name);
    }

    let printer = analyzer.printer();
    for (project_dir, entry) in by_project {
        let Some((manifest, files)) = entry else {
            continue;
        };
        for (relative, changed) in files {
            let absolute = manifest.source_root.join(&relative);
            let absolute = std::fs::canonicalize(&absolute).unwrap_or(absolute);
            let Some((target, ours)) = analyzer.source_text(&absolute) else {
                tracing::info!("cannot read {}; Rodin edit not synced", absolute.display());
                continue;
            };

            let outcome = {
                let workspace_dir = workspace_dir.to_path_buf();
                let printer = printer.clone();
                let manifest = Arc::clone(&manifest);
                let relative = relative.clone();
                let project_dir = project_dir.clone();
                tokio::task::spawn_blocking(move || {
                    model_sync::sync_source_file(
                        &manifest,
                        &workspace_dir,
                        &project_dir,
                        &relative,
                        &changed,
                        &ours,
                        &printer,
                    )
                })
                .await
            };
            let outcome = match outcome {
                Ok(Ok(outcome)) => outcome,
                Ok(Err(message)) => {
                    tracing::info!("Rodin edit not synced: {message}");
                    continue;
                }
                Err(join_error) => {
                    tracing::info!("Rodin edit sync failed: {join_error}");
                    continue;
                }
            };
            let Some(text) = outcome.text() else {
                continue;
            };

            if let Err(message) = analyzer.apply_source_text(&absolute, target, text).await {
                analyzer
                    .client()
                    .show_message(
                        MessageType::ERROR,
                        format!(
                            "Could not apply Rodin's edits to {}: {message}",
                            relative.display()
                        ),
                    )
                    .await;
                continue;
            }
            // Rodin's side is incorporated: advance the base so the next
            // merge uses the right ancestor. Not after a conflict, though —
            // the marker-laden text does not parse, and a base that does not
            // parse would permanently disable sync for this file. The old
            // base stays until the user resolves the markers and a clean
            // state lands.
            if !matches!(outcome, MergeOutcome::Conflict(_))
                && let Err(e) = model_sync::update_base_source(
                    workspace_dir,
                    &manifest.project_name,
                    &relative,
                    text,
                )
            {
                tracing::info!("could not advance base snapshot: {e}");
            }
            match outcome {
                MergeOutcome::Merged(_) => {
                    analyzer
                        .client()
                        .show_message(
                            MessageType::INFO,
                            format!("Merged Rodin's model edits into {}.", relative.display()),
                        )
                        .await;
                }
                MergeOutcome::Conflict(_) => {
                    analyzer
                        .client()
                        .show_message(
                            MessageType::WARNING,
                            format!(
                                "Merged Rodin's model edits into {} with conflicts — resolve the <<<<<<< markers.",
                                relative.display()
                            ),
                        )
                        .await;
                }
                MergeOutcome::Unchanged | MergeOutcome::FastForward(_) => {
                    tracing::info!("synced Rodin's model edits into {}", relative.display());
                }
            }
        }
    }
}

/// Recompute the proof-status overlay and republish diagnostics for the open
/// documents. `scope` limits the (potentially large) rescan to the given
/// project directories — the watcher passes the batch's projects, the build
/// flows their just-built project; `None` rescans the whole workspace (the
/// initial scan). The builds must call this explicitly: the watcher
/// classifies their writes as the server's own and rightly stays quiet.
pub(crate) async fn refresh(
    workspace_dir: &Path,
    analyzer: &Analyzer,
    scope: Option<BTreeSet<PathBuf>>,
) {
    let workspace_dir = workspace_dir.to_path_buf();
    let update = tokio::task::spawn_blocking(move || match scope {
        None => ProofStatusUpdate::Full(proof_status_for_workspace(&workspace_dir)),
        Some(dirs) => ProofStatusUpdate::Projects(
            dirs.into_iter()
                .map(|dir| {
                    let status = project_proof_status(&dir, &workspace_dir);
                    (dir, status)
                })
                .collect(),
        ),
    })
    .await;
    match update {
        Ok(update) => analyzer.refresh_proof_status(update).await,
        Err(join_error) => tracing::info!("rodin proof-status scan failed: {join_error}"),
    }
}

/// A freshly scanned proof status, to fold into the analyzer's overlay.
pub(crate) enum ProofStatusUpdate {
    /// The whole workspace was scanned; replace the overlay.
    Full(ProofStatusOverlay),
    /// Only these projects were scanned; merge them (an empty status removes
    /// the project's entry).
    Projects(Vec<(PathBuf, ProjectProofStatus)>),
}

/// The proof-status overlay: per-component messages, kept per project and
/// scoped to the source file each project's base manifest says the component
/// came from — so two projects that both contain an `M0` don't
/// cross-contaminate diagnostics.
#[derive(Debug, Default, PartialEq)]
pub struct ProofStatusOverlay {
    /// Project directory → its status. Only non-empty statuses are stored.
    pub(crate) projects: BTreeMap<PathBuf, ProjectProofStatus>,
}

/// One project's proof-status messages.
#[derive(Debug, Default, PartialEq)]
pub struct ProjectProofStatus {
    /// Canonicalized source file → component name → message.
    pub(crate) by_source: HashMap<PathBuf, HashMap<String, String>>,
    /// Component name → message, for projects without a base manifest
    /// (e.g. built by an earlier version). Name collisions are possible
    /// here; the scoped map above always wins.
    pub(crate) by_name: HashMap<String, String>,
}

impl ProjectProofStatus {
    pub fn is_empty(&self) -> bool {
        self.by_source.is_empty() && self.by_name.is_empty()
    }
}

impl ProofStatusOverlay {
    pub fn is_empty(&self) -> bool {
        self.projects.is_empty()
    }

    /// The message for `component` as declared in the document at `path`
    /// (canonicalized). A document any project's scoped map knows is
    /// answered only from scoped entries; the by-name fallback serves the
    /// rest.
    pub fn message_for(&self, path: Option<&Path>, component: &str) -> Option<&String> {
        if let Some(path) = path {
            let mut known = false;
            for status in self.projects.values() {
                if let Some(per_source) = status.by_source.get(path) {
                    known = true;
                    if let Some(message) = per_source.get(component) {
                        return Some(message);
                    }
                }
            }
            if known {
                return None;
            }
        }
        self.projects
            .values()
            .find_map(|status| status.by_name.get(component))
    }

    /// Fold a scan result in; returns whether anything actually changed (so
    /// the caller can skip republishing when nothing did).
    pub(crate) fn apply(&mut self, update: ProofStatusUpdate) -> bool {
        match update {
            ProofStatusUpdate::Full(new) => {
                if *self == new {
                    return false;
                }
                *self = new;
                true
            }
            ProofStatusUpdate::Projects(scanned) => {
                let mut changed = false;
                for (dir, status) in scanned {
                    if status.is_empty() {
                        changed |= self.projects.remove(&dir).is_some();
                    } else if self.projects.get(&dir) != Some(&status) {
                        self.projects.insert(dir, status);
                        changed = true;
                    }
                }
                changed
            }
        }
    }
}

/// Scan every Rodin project directory (a subdirectory holding a `.project`)
/// in the workspace. Fully discharged components are absent — no news is
/// good news.
pub fn proof_status_for_workspace(workspace_dir: &Path) -> ProofStatusOverlay {
    let mut overlay = ProofStatusOverlay::default();
    let Ok(entries) = std::fs::read_dir(workspace_dir) else {
        return overlay;
    };
    for entry in entries.flatten() {
        let project_dir = entry.path();
        let hidden = project_dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_none_or(|name| name.starts_with('.'));
        if hidden || !project_dir.is_dir() {
            continue;
        }
        let status = project_proof_status(&project_dir, workspace_dir);
        if !status.is_empty() {
            overlay.projects.insert(project_dir, status);
        }
    }
    overlay
}

/// One aggregated proof-status line per component of one project — one
/// informational line beats one diagnostic per obligation on the same
/// header. Empty (also for a directory that is not a project) means the
/// project has nothing to report.
fn project_proof_status(project_dir: &Path, workspace_dir: &Path) -> ProjectProofStatus {
    use rossi_build::rules::RuleId;

    let mut status = ProjectProofStatus::default();
    if !project_dir.join(".project").is_file() {
        return status;
    }
    let Ok(report) = rossi_build::proofs::check_directory(project_dir) else {
        return status;
    };
    // Component name (XML stem) → canonicalized source file, from the base
    // manifest — built once, canonicalizing each source file once.
    let sources_by_stem: HashMap<String, PathBuf> = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|name| super::model_sync::load_manifest(workspace_dir, name))
        .map(|manifest| {
            manifest
                .files
                .iter()
                .flat_map(|(relative, xml_names)| {
                    let absolute = manifest.source_root.join(relative);
                    let absolute = std::fs::canonicalize(&absolute).unwrap_or(absolute);
                    xml_names.iter().filter_map(move |xml_name| {
                        let stem = Path::new(xml_name).file_stem()?.to_str()?;
                        Some((stem.to_string(), absolute.clone()))
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let mut per_component: HashMap<String, (usize, usize)> = HashMap::new();
    for diagnostic in &report.diagnostics {
        let counts = per_component.entry(diagnostic.origin.clone()).or_default();
        match diagnostic.rule_id {
            Some(RuleId::UndischargedProof) => counts.0 += 1,
            Some(RuleId::BrokenProof) => counts.1 += 1,
            _ => {}
        }
    }
    for (component, (undischarged, broken)) in per_component {
        let mut parts = Vec::new();
        if undischarged > 0 {
            parts.push(format!("{undischarged} undischarged proof obligation(s)"));
        }
        if broken > 0 {
            parts.push(format!("{broken} broken proof(s)"));
        }
        if parts.is_empty() {
            continue;
        }
        let message = format!("Rodin: {}", parts.join(", "));
        match sources_by_stem.get(&component) {
            Some(source) => {
                status
                    .by_source
                    .entry(source.clone())
                    .or_default()
                    .insert(component, message);
            }
            None => {
                status.by_name.insert(component, message);
            }
        }
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn aggregates_proof_status_per_component() {
        let ws = TempDir::new("rossi-rodin-sync-status");
        let project = ws.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join(".project"), "<projectDescription/>").unwrap();
        std::fs::write(
            project.join("M0.bpo"),
            r#"<?xml version="1.0"?><org.eventb.core.poFile>
               <org.eventb.core.poSequent name="inv1/INV"/>
               <org.eventb.core.poSequent name="inv2/INV"/>
               <org.eventb.core.poSequent name="inv3/INV"/>
               </org.eventb.core.poFile>"#,
        )
        .unwrap();
        std::fs::write(
            project.join("M0.bps"),
            r#"<?xml version="1.0"?><org.eventb.core.psFile>
               <org.eventb.core.psStatus name="inv1/INV" org.eventb.core.confidence="1000" org.eventb.core.psBroken="false"/>
               <org.eventb.core.psStatus name="inv2/INV" org.eventb.core.confidence="-99" org.eventb.core.psBroken="false"/>
               <org.eventb.core.psStatus name="inv3/INV" org.eventb.core.confidence="1000" org.eventb.core.psBroken="true"/>
               </org.eventb.core.psFile>"#,
        )
        .unwrap();
        // A dot-directory (Eclipse .metadata) and a non-project dir are skipped.
        std::fs::create_dir_all(ws.path().join(".metadata")).unwrap();
        std::fs::create_dir_all(ws.path().join("not-a-project")).unwrap();

        let status = proof_status_for_workspace(ws.path());
        // No base manifest in this fixture → the by-name fallback serves it.
        assert_eq!(
            status.message_for(None, "M0").map(String::as_str),
            Some("Rodin: 1 undischarged proof obligation(s), 1 broken proof(s)")
        );
        assert_eq!(status.projects.len(), 1);
        let project_status = status.projects.values().next().unwrap();
        assert_eq!(project_status.by_name.len(), 1);
        assert!(project_status.by_source.is_empty());
    }

    #[test]
    fn overlay_scopes_same_named_components_by_source_file() {
        let ws = TempDir::new("rossi-rodin-sync-scope");
        let src_a = ws.path().join("src-a");
        std::fs::create_dir_all(&src_a).unwrap();
        let source_a = src_a.join("m.eventb");
        std::fs::write(&source_a, "MACHINE M0\nEND\n").unwrap();
        let project = ws.path().join("proj-a");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join(".project"), "<projectDescription/>").unwrap();
        std::fs::write(
            project.join("M0.bpo"),
            r#"<?xml version="1.0"?><org.eventb.core.poFile>
               <org.eventb.core.poSequent name="inv1/INV"/>
               </org.eventb.core.poFile>"#,
        )
        .unwrap();
        std::fs::write(
            project.join("M0.bps"),
            r#"<?xml version="1.0"?><org.eventb.core.psFile>
               <org.eventb.core.psStatus name="inv1/INV" org.eventb.core.confidence="-99" org.eventb.core.psBroken="false"/>
               </org.eventb.core.psFile>"#,
        )
        .unwrap();
        super::super::model_sync::write_base(
            ws.path(),
            "proj-a",
            &std::fs::canonicalize(&src_a).unwrap(),
            &[super::super::model_sync::SourceFileRecord {
                relative: PathBuf::from("m.eventb"),
                text: "MACHINE M0\nEND\n".to_string(),
                component_files: vec!["M0.bum".to_string()],
            }],
        )
        .unwrap();

        let status = proof_status_for_workspace(ws.path());
        let canonical_a = std::fs::canonicalize(&source_a).unwrap();
        assert!(
            status.message_for(Some(&canonical_a), "M0").is_some(),
            "the owning source file sees its component's status"
        );
        // A different file that merely declares a component named M0 must
        // not inherit this project's status.
        let unrelated = ws.path().join("unrelated.eventb");
        assert!(status.message_for(Some(&unrelated), "M0").is_none());
    }

    #[test]
    fn fully_discharged_components_are_absent() {
        let ws = TempDir::new("rossi-rodin-sync-clean");
        let project = ws.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join(".project"), "<projectDescription/>").unwrap();
        std::fs::write(
            project.join("M0.bpo"),
            r#"<?xml version="1.0"?><org.eventb.core.poFile>
               <org.eventb.core.poSequent name="inv1/INV"/>
               </org.eventb.core.poFile>"#,
        )
        .unwrap();
        std::fs::write(
            project.join("M0.bps"),
            r#"<?xml version="1.0"?><org.eventb.core.psFile>
               <org.eventb.core.psStatus name="inv1/INV" org.eventb.core.confidence="1000" org.eventb.core.psBroken="false"/>
               </org.eventb.core.psFile>"#,
        )
        .unwrap();

        assert!(proof_status_for_workspace(ws.path()).is_empty());
    }

    #[test]
    fn scoped_updates_merge_and_remove_project_entries() {
        let mut overlay = ProofStatusOverlay::default();
        let mut status = ProjectProofStatus::default();
        status
            .by_name
            .insert("M0".to_string(), "Rodin: 1 undischarged".to_string());

        assert!(overlay.apply(ProofStatusUpdate::Projects(vec![(
            PathBuf::from("/ws/proj"),
            status,
        )])));
        assert!(overlay.message_for(None, "M0").is_some());

        // The same scan again is a no-op.
        let mut same = ProjectProofStatus::default();
        same.by_name
            .insert("M0".to_string(), "Rodin: 1 undischarged".to_string());
        assert!(!overlay.apply(ProofStatusUpdate::Projects(vec![(
            PathBuf::from("/ws/proj"),
            same,
        )])));

        // An empty rescan removes the project (fully discharged).
        assert!(overlay.apply(ProofStatusUpdate::Projects(vec![(
            PathBuf::from("/ws/proj"),
            ProjectProofStatus::default(),
        )])));
        assert!(overlay.is_empty());
    }

    #[test]
    fn startup_batch_covers_only_manifest_known_files_on_disk() {
        let ws = TempDir::new("rossi-rodin-sync-startup");
        // A built project: manifest knows base_ctx.buc and gone.bum, but only
        // the former is on disk (the latter was deleted in Rodin).
        let project = ws.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("base_ctx.buc"), "<contextFile/>").unwrap();
        std::fs::write(project.join("rodin_authored.bum"), "<machineFile/>").unwrap();
        super::super::model_sync::write_base(
            ws.path(),
            "proj",
            Path::new("/src"),
            &[super::super::model_sync::SourceFileRecord {
                relative: PathBuf::from("model.eventb"),
                text: "CONTEXT base_ctx\nEND\n".to_string(),
                component_files: vec!["base_ctx.buc".to_string(), "gone.bum".to_string()],
            }],
        )
        .unwrap();
        // A directory without a manifest (never built by this server) and a
        // hidden directory contribute nothing.
        let foreign = ws.path().join("foreign");
        std::fs::create_dir_all(&foreign).unwrap();
        std::fs::write(foreign.join("M0.bum"), "<machineFile/>").unwrap();
        std::fs::create_dir_all(ws.path().join(".metadata")).unwrap();

        assert_eq!(
            startup_model_batch(ws.path()),
            vec![(project, "base_ctx.buc".to_string())]
        );
    }

    #[test]
    fn own_writes_are_recognized_until_content_changes() {
        let ws = TempDir::new("rossi-rodin-sync-echo");
        let project = ws.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let path = project.join("M0.bps");
        std::fs::write(&path, "state-a").unwrap();

        let written: WrittenFiles = Arc::new(WriteRegistry::default());
        written.record([(path.clone(), content_hash(b"state-a"))]);

        let batch: BTreeSet<PathBuf> = [path.clone()].into();
        assert!(
            classify_batch(ws.path(), &batch, &written)
                .proof_projects
                .is_empty(),
            "the server's own write must not trigger a refresh"
        );

        std::fs::write(&path, "state-b").unwrap();
        assert_eq!(
            classify_batch(ws.path(), &batch, &written)
                .proof_projects
                .into_iter()
                .collect::<Vec<_>>(),
            vec![project.clone()],
            "a foreign change to the same path must trigger a scoped refresh"
        );

        let other: BTreeSet<PathBuf> = [project.join("M0.bcc")].into();
        let changes = classify_batch(ws.path(), &other, &written);
        assert!(
            changes.proof_projects.is_empty() && changes.model.is_empty(),
            "checked files trigger neither flow"
        );
    }
}
