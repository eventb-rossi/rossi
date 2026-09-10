//! Bridge proof files between the checkout and the shared Rodin workspace.
//!
//! Proof state (`.bpr` proofs, `.bps` statuses, `.bpo` obligations) has two
//! homes: next to the `.eventb` sources (where `rossi import` places it and
//! version control keeps it) and the Rodin project the lens builds under
//! `<root>/.rossi/rodin`. This module syncs the two only at the "Open in
//! Rodin" session boundaries — the checkout is authoritative when a session
//! starts (seed), the workspace when it ends (mirror-back) — never
//! continuously.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Whether `name` is a proof-state file Rodin keeps next to a component.
fn is_proof_file_name(name: &str) -> bool {
    name.ends_with(".bpr") || name.ends_with(".bps") || name.ends_with(".bpo")
}

/// The proof files at `dir`'s top level: sorted, files only, missing
/// directory → empty. Proof files live flat next to their components, both
/// in the checkout and in a Rodin project directory.
fn proof_files_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_proof_file_name)
                && path.is_file()
        })
        .collect();
    paths.sort();
    paths
}

/// What [`seed_project`] wrote into the workspace project.
pub(crate) struct SeedReport {
    /// `(path, content hash)` of every file written, for the sync watcher's
    /// echo guard.
    pub written: Vec<(PathBuf, u64)>,
    /// Files copied because the workspace had no copy.
    pub copied: usize,
    /// Files whose differing workspace copy was overwritten — replaced data,
    /// surfaced to the user by the caller.
    pub replaced: usize,
}

/// Copy the proof files sitting next to the text sources into the workspace
/// project. The checkout is authoritative when a Rodin session starts, so a
/// differing workspace copy is overwritten (counted in
/// [`replaced`](SeedReport::replaced)). Nothing is ever deleted here: a
/// workspace file with no text-side counterpart may be the only copy of
/// proof work an interrupted session never mirrored back.
///
/// Proof files are collected from the directories holding the project's
/// text sources; on a basename collision across directories the first in
/// sorted order wins. Per-file IO failures are logged and skipped — seeding
/// must never break the lens flow.
pub(crate) fn seed_project(
    source_dir: &Path,
    workspace_dir: &Path,
    project_name: &str,
) -> std::io::Result<SeedReport> {
    let sources = super::build::collect_source_files(source_dir)?;
    let dirs: BTreeSet<PathBuf> = sources
        .iter()
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .collect();
    let mut text_files: BTreeMap<String, PathBuf> = BTreeMap::new();
    for dir in &dirs {
        for path in proof_files_in(dir) {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if let Some(winner) = text_files.get(name) {
                tracing::info!(
                    "duplicate proof file {name}: seeding {}, ignoring {}",
                    winner.display(),
                    path.display()
                );
                continue;
            }
            text_files.insert(name.to_string(), path);
        }
    }

    let mut report = SeedReport {
        written: Vec::new(),
        copied: 0,
        replaced: 0,
    };
    if text_files.is_empty() {
        return Ok(report);
    }
    let project_dir = workspace_dir.join(project_name);
    std::fs::create_dir_all(&project_dir)?;
    for (name, path) in text_files {
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::info!("cannot read {}: {e}; not seeded", path.display());
                continue;
            }
        };
        let dest = project_dir.join(&name);
        let existing = std::fs::read(&dest).ok();
        if existing.as_deref() == Some(bytes.as_slice()) {
            continue;
        }
        if let Err(e) = std::fs::write(&dest, &bytes) {
            tracing::info!("cannot write {}: {e}; not seeded", dest.display());
            continue;
        }
        match existing {
            Some(_) => report.replaced += 1,
            None => report.copied += 1,
        }
        report
            .written
            .push((dest, super::sync::content_hash(&bytes)));
    }
    Ok(report)
}

/// What [`mirror_back_project`] changed on the text side.
pub(crate) struct MirrorReport {
    /// Basenames copied next to their sources (created or overwritten).
    pub copied: Vec<String>,
    /// Basenames deleted next to their sources because Rodin deleted them.
    pub deleted: Vec<String>,
}

/// Copy the workspace project's proof files back next to the text sources —
/// the workspace is authoritative when a Rodin session ends. Only files
/// whose component stem the project's base manifest attributes to our
/// sources are touched (a component authored inside Rodin has no source to
/// sit next to, and its model is not synced either); each file lands next
/// to the source file its component came from. Returns `None` when the
/// project has no base manifest (never built by this server).
///
/// With `allow_deletions` — granted only when this session's seed ran, so
/// every text-side proof file provably had a workspace counterpart at
/// session start — a text-side `.bpr` whose workspace counterpart vanished
/// was deleted in Rodin and is deleted next to the sources too.
///
/// Only a `.bpr`. A proof file disappears from the workspace for one reason
/// (someone discarded that proof), but `.bpo` and `.bps` are derived: Rodin's
/// builder deletes them before regenerating them, and a session torn down
/// mid-build leaves them missing with nothing wrong. Reading that absence as
/// intent is how a transient workspace state became a deletion in the
/// checkout. They are still copied back whenever the workspace has them.
pub(crate) fn mirror_back_project(
    workspace_dir: &Path,
    project_name: &str,
    allow_deletions: bool,
) -> std::io::Result<Option<MirrorReport>> {
    let Some(manifest) = super::model_sync::load_manifest(workspace_dir, project_name) else {
        return Ok(None);
    };
    // Component stem → the directory its source file lives in.
    let mut dir_by_stem: BTreeMap<String, PathBuf> = BTreeMap::new();
    for (relative, xml_names) in &manifest.files {
        let absolute = manifest.source_root.join(relative);
        let absolute = std::fs::canonicalize(&absolute).unwrap_or(absolute);
        let Some(dir) = absolute.parent() else {
            continue;
        };
        for xml_name in xml_names {
            let Some(stem) = Path::new(xml_name).file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            dir_by_stem.insert(stem.to_string(), dir.to_path_buf());
        }
    }

    let project_dir = workspace_dir.join(project_name);
    let mut report = MirrorReport {
        copied: Vec::new(),
        deleted: Vec::new(),
    };
    for path in proof_files_in(&project_dir) {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(dir) = Path::new(name)
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|stem| dir_by_stem.get(stem))
        else {
            continue;
        };
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::info!("cannot read {}: {e}; not mirrored", path.display());
                continue;
            }
        };
        let dest = dir.join(name);
        if std::fs::read(&dest).ok().as_deref() == Some(bytes.as_slice()) {
            continue;
        }
        match std::fs::write(&dest, &bytes) {
            Ok(()) => report.copied.push(name.to_string()),
            Err(e) => tracing::info!("cannot write {}: {e}; not mirrored", dest.display()),
        }
    }

    if allow_deletions {
        let dirs: BTreeSet<&PathBuf> = dir_by_stem.values().collect();
        for dir in dirs {
            for path in proof_files_in(dir) {
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let known = Path::new(name)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|stem| dir_by_stem.contains_key(stem));
                if !known || !name.ends_with(".bpr") || project_dir.join(name).exists() {
                    continue;
                }
                match std::fs::remove_file(&path) {
                    Ok(()) => report.deleted.push(name.to_string()),
                    Err(e) => tracing::info!("cannot delete {}: {e}", path.display()),
                }
            }
        }
    }
    Ok(Some(report))
}

/// Slot holding the (at most one) live session monitor per server.
pub(crate) type SessionMonitorSlot = Arc<parking_lot::Mutex<Option<RodinSessionMonitor>>>;

/// A background task watching one Rodin session through the Eclipse
/// workspace lock: when the lock is released (Rodin quit), the armed
/// projects' proof files are mirrored back next to their sources. Dropping
/// the monitor aborts the task. It never touches the open-in-Rodin
/// single-flight guard — proving sessions last hours and re-clicks must
/// keep working.
pub(crate) struct RodinSessionMonitor {
    workspace_dir: PathBuf,
    /// Project name → whether this session's seed ran (gates deletions).
    projects: Arc<parking_lot::Mutex<BTreeMap<String, bool>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for RodinSessionMonitor {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// One fcntl probe every two seconds is free; sessions last as they last.
const SESSION_POLL: Duration = Duration::from_secs(2);

impl RodinSessionMonitor {
    /// Arm (or extend) the stop monitor for the running Rodin session.
    /// Re-arming the same workspace adds the project to the armed set; a
    /// different workspace or a finished monitor is replaced. Only projects
    /// armed here are mirrored — a project from an earlier session the user
    /// never opened must not receive surprise writes.
    pub(crate) fn arm(
        slot: &SessionMonitorSlot,
        client: &tower_lsp::Client,
        workspace_dir: &Path,
        project_name: &str,
        seeded: bool,
        written: &super::sync::WrittenFiles,
    ) {
        let mut slot = slot.lock();
        if let Some(monitor) = slot.as_ref()
            && monitor.workspace_dir == workspace_dir
            && !monitor.task.is_finished()
        {
            *monitor
                .projects
                .lock()
                .entry(project_name.to_string())
                .or_insert(false) |= seeded;
            return;
        }
        let projects = Arc::new(parking_lot::Mutex::new(BTreeMap::from([(
            project_name.to_string(),
            seeded,
        )])));
        let task = tokio::spawn(monitor_task(
            client.clone(),
            workspace_dir.to_path_buf(),
            Arc::clone(&projects),
            Arc::clone(written),
        ));
        let replaced = slot.replace(RodinSessionMonitor {
            workspace_dir: workspace_dir.to_path_buf(),
            projects,
            task,
        });
        if replaced.is_some() {
            tracing::info!("superseding the previous Rodin session monitor");
        }
    }
}

async fn monitor_task(
    client: tower_lsp::Client,
    workspace_dir: PathBuf,
    projects: Arc<parking_lot::Mutex<BTreeMap<String, bool>>>,
    written: super::sync::WrittenFiles,
) {
    let probe_dir = workspace_dir.clone();
    let ended = wait_for_session_end(
        move || super::lock::workspace_lock_state(&probe_dir),
        SESSION_POLL,
    )
    .await;
    if !ended {
        // Unknown probe (Windows, IO failure): the stop is unobservable.
        tracing::info!("cannot observe the Rodin session end; proofs not mirrored");
        return;
    }
    // A rebuild-on-save scheduled just before Rodin quit may still be
    // writing into the workspace; mirror only quiescent state.
    while written.building() {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let armed: Vec<(String, bool)> = projects
        .lock()
        .iter()
        .map(|(name, seeded)| (name.clone(), *seeded))
        .collect();
    let (mut copied, mut deleted) = (0usize, 0usize);
    for (project, seeded) in armed {
        let result = {
            let workspace_dir = workspace_dir.clone();
            let project = project.clone();
            tokio::task::spawn_blocking(move || {
                mirror_back_project(&workspace_dir, &project, seeded)
            })
            .await
        };
        match result {
            Ok(Ok(Some(report))) => {
                copied += report.copied.len();
                deleted += report.deleted.len();
            }
            Ok(Ok(None)) => tracing::info!("no build manifest for {project}; proofs not mirrored"),
            Ok(Err(e)) => tracing::info!("mirroring {project} failed: {e}"),
            Err(join_error) => tracing::info!("mirror task for {project} failed: {join_error}"),
        }
    }
    if copied + deleted > 0 {
        client
            .show_message(
                crate::lsp_types::MessageType::INFO,
                format!(
                    "Rodin closed: mirrored proof files next to the sources \
                     ({copied} copied, {deleted} deleted)."
                ),
            )
            .await;
    } else {
        tracing::info!("Rodin session ended; proof files already in sync");
    }
}

/// Poll the workspace lock until the running Rodin releases it. `true` when
/// the lock went `Free` (the session ended); `false` on `Unknown` (Windows,
/// or a probe failure) — the stop cannot be observed there and the caller
/// must not mirror. No timeout: sessions last as long as they last.
async fn wait_for_session_end(
    mut probe: impl FnMut() -> super::lock::LockState,
    poll: Duration,
) -> bool {
    loop {
        match probe() {
            super::lock::LockState::Free => return true,
            super::lock::LockState::Unknown => return false,
            super::lock::LockState::Held => tokio::time::sleep(poll).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn seed_copies_and_replaces_but_never_deletes() {
        let tmp = TempDir::new("rossi-proof-seed");
        let src = tmp.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("m.eventb"), "MACHINE M0\nEND\n").unwrap();
        std::fs::write(src.join("M0.bpr"), "text-proof").unwrap();
        std::fs::write(src.join("M0.bps"), "text-status").unwrap();
        std::fs::write(src.join("notes.txt.bak"), "not a proof").unwrap();
        let ws = tmp.join("ws");
        let project = ws.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        // A differing workspace copy is replaced; an unrelated workspace-only
        // proof file survives (it may be unmirrored work).
        std::fs::write(project.join("M0.bps"), "ws-status").unwrap();
        std::fs::write(project.join("M9.bpr"), "ws-only").unwrap();

        let report = seed_project(&src, &ws, "proj").unwrap();
        assert_eq!((report.copied, report.replaced), (1, 1));
        assert_eq!(
            std::fs::read(project.join("M0.bpr")).unwrap(),
            b"text-proof"
        );
        assert_eq!(
            std::fs::read(project.join("M0.bps")).unwrap(),
            b"text-status"
        );
        assert_eq!(std::fs::read(project.join("M9.bpr")).unwrap(), b"ws-only");
        // Echo-guard hashes match the bytes on disk.
        assert_eq!(report.written.len(), 2);
        for (path, hash) in &report.written {
            let bytes = std::fs::read(path).unwrap();
            assert_eq!(super::super::sync::content_hash(&bytes), *hash);
        }

        // Identical bytes are not rewritten on a second run.
        let again = seed_project(&src, &ws, "proj").unwrap();
        assert_eq!((again.copied, again.replaced), (0, 0));
        assert!(again.written.is_empty());
    }

    #[test]
    fn seed_first_wins_across_source_dirs() {
        let tmp = TempDir::new("rossi-proof-seed-dirs");
        let src = tmp.join("src");
        let a = src.join("a");
        let b = src.join("b");
        for (dir, machine) in [(&a, "MACHINE A0\nEND\n"), (&b, "MACHINE B0\nEND\n")] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("m.eventb"), machine).unwrap();
            std::fs::write(
                dir.join("M0.bpr"),
                dir.file_name().unwrap().as_encoded_bytes(),
            )
            .unwrap();
        }

        seed_project(&src, &tmp.join("ws"), "proj").unwrap();
        // Sorted directory order decides the collision: `a` before `b`.
        assert_eq!(
            std::fs::read(tmp.join("ws").join("proj").join("M0.bpr")).unwrap(),
            b"a"
        );
    }

    #[test]
    fn seed_without_proof_files_touches_nothing() {
        let tmp = TempDir::new("rossi-proof-seed-empty");
        let src = tmp.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("m.eventb"), "MACHINE M0\nEND\n").unwrap();

        let report = seed_project(&src, &tmp.join("ws"), "proj").unwrap();
        assert_eq!((report.copied, report.replaced), (0, 0));
        // Not even the project directory is created for an empty seed.
        assert!(!tmp.join("ws").exists());
    }

    use super::super::model_sync::{SourceFileRecord, write_base};

    /// Workspace + manifest fixture: `m.eventb` (M0) at the source root and
    /// `sub/n.eventb` (N0) nested below it.
    fn mirror_fixture(tmp: &TempDir) -> (PathBuf, PathBuf, PathBuf) {
        let src = tmp.join("src");
        let sub = src.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(src.join("m.eventb"), "MACHINE M0\nEND\n").unwrap();
        std::fs::write(sub.join("n.eventb"), "MACHINE N0\nEND\n").unwrap();
        let ws = tmp.join("ws");
        let project = ws.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        write_base(
            &ws,
            "proj",
            &src,
            &[
                SourceFileRecord {
                    relative: PathBuf::from("m.eventb"),
                    text: "MACHINE M0\nEND\n".to_string(),
                    component_files: vec!["M0.bum".to_string()],
                },
                SourceFileRecord {
                    relative: PathBuf::from("sub/n.eventb"),
                    text: "MACHINE N0\nEND\n".to_string(),
                    component_files: vec!["N0.bum".to_string()],
                },
            ],
        )
        .unwrap();
        (src, ws, project)
    }

    #[test]
    fn mirror_copies_next_to_the_mapped_sources() {
        let tmp = TempDir::new("rossi-proof-mirror");
        let (src, ws, project) = mirror_fixture(&tmp);
        std::fs::write(project.join("M0.bpr"), "ws-m0").unwrap();
        std::fs::write(project.join("N0.bpr"), "ws-n0").unwrap();
        // A component authored inside Rodin has no source to sit next to.
        std::fs::write(project.join("X9.bpr"), "rodin-own").unwrap();
        // A differing text-side copy is overwritten (the workspace wins).
        std::fs::write(src.join("M0.bpr"), "old").unwrap();

        let report = mirror_back_project(&ws, "proj", false).unwrap().unwrap();
        assert_eq!(report.copied, ["M0.bpr", "N0.bpr"]);
        assert!(report.deleted.is_empty());
        assert_eq!(std::fs::read(src.join("M0.bpr")).unwrap(), b"ws-m0");
        assert_eq!(
            std::fs::read(src.join("sub").join("N0.bpr")).unwrap(),
            b"ws-n0"
        );
        assert!(!src.join("X9.bpr").exists());

        // Identical bytes are not re-reported on a second run.
        let again = mirror_back_project(&ws, "proj", false).unwrap().unwrap();
        assert!(again.copied.is_empty());
    }

    #[test]
    fn mirror_deletions_are_gated_and_manifest_scoped() {
        let tmp = TempDir::new("rossi-proof-mirror-del");
        let (src, ws, _project) = mirror_fixture(&tmp);
        // Text-side files with no workspace counterpart: one manifest-known
        // (deleted in Rodin), one foreign (never ours to delete).
        std::fs::write(src.join("M0.bpr"), "deleted-in-rodin").unwrap();
        std::fs::write(src.join("Z9.bpr"), "foreign").unwrap();

        // Without a seeded session nothing may be deleted.
        let kept = mirror_back_project(&ws, "proj", false).unwrap().unwrap();
        assert!(kept.deleted.is_empty());
        assert!(src.join("M0.bpr").exists());

        let report = mirror_back_project(&ws, "proj", true).unwrap().unwrap();
        assert_eq!(report.deleted, ["M0.bpr"]);
        assert!(!src.join("M0.bpr").exists());
        assert!(src.join("Z9.bpr").exists());
    }

    #[test]
    fn mirror_never_deletes_derived_obligation_files() {
        let tmp = TempDir::new("rossi-proof-mirror-derived");
        let (src, ws, project) = mirror_fixture(&tmp);
        // What a Rodin session torn down mid-build leaves behind: the proof
        // survives in the workspace, the files its builder derives do not.
        std::fs::write(project.join("M0.bpr"), "ws-m0").unwrap();
        std::fs::write(src.join("M0.bpo"), "obligations").unwrap();
        std::fs::write(src.join("M0.bps"), "statuses").unwrap();

        let report = mirror_back_project(&ws, "proj", true).unwrap().unwrap();

        // Regenerable, and absent for a reason that is not the user's: both
        // stay in the checkout.
        assert!(report.deleted.is_empty(), "{:?}", report.deleted);
        assert_eq!(std::fs::read(src.join("M0.bpo")).unwrap(), b"obligations");
        assert_eq!(std::fs::read(src.join("M0.bps")).unwrap(), b"statuses");
    }

    #[test]
    fn mirror_without_manifest_is_none() {
        let tmp = TempDir::new("rossi-proof-mirror-nomanifest");
        assert!(
            mirror_back_project(&tmp.join("ws"), "proj", true)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn session_end_waits_for_free_and_gives_up_on_unknown() {
        use super::super::lock::LockState;
        // `Held` cannot be produced in-process (F_GETLK ignores the probing
        // process's own locks), so the transition logic takes a probe.
        let mut states = [LockState::Held, LockState::Held, LockState::Free].into_iter();
        assert!(wait_for_session_end(move || states.next().unwrap(), Duration::ZERO).await);

        let mut states = [LockState::Held, LockState::Unknown].into_iter();
        assert!(!wait_for_session_end(move || states.next().unwrap(), Duration::ZERO).await);
    }

    #[test]
    fn full_cycle_carries_proofs_both_ways() {
        let tmp = TempDir::new("rossi-proof-cycle");
        let src = tmp.join("src");
        std::fs::create_dir_all(&src).unwrap();
        // A machine with an invariant yields at least one obligation row.
        std::fs::write(
            src.join("m.eventb"),
            "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ∈ ℕ\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x ≔ 0\n    END\nEND\n",
        )
        .unwrap();
        std::fs::write(src.join("m.bpr"), "proof-v1").unwrap();
        let ws = tmp.join("ws");
        let project = ws.join("proj");

        // Seed, then build: the build writes the manifest and reconciles
        // around the seeded proof state.
        seed_project(&src, &ws, "proj").unwrap();
        super::super::build::build_rodin_project(
            &src,
            &super::super::build::Overlay::new(),
            &project,
            "proj",
        )
        .unwrap();
        assert_eq!(std::fs::read(project.join("m.bpr")).unwrap(), b"proof-v1");

        // Fake a Rodin session: a new proof version and a discharged status.
        std::fs::write(project.join("m.bpr"), "proof-v2").unwrap();
        let bps = project.join("m.bps");
        let generated = std::fs::read_to_string(&bps).unwrap();
        let doctored = generated.replace("confidence=\"-99\"", "confidence=\"1000\"");
        assert_ne!(
            generated, doctored,
            "fixture must contain an unattempted row"
        );
        std::fs::write(&bps, &doctored).unwrap();

        let report = mirror_back_project(&ws, "proj", true).unwrap().unwrap();
        assert!(
            report.copied.contains(&"m.bpr".to_string()),
            "{:?}",
            report.copied
        );
        assert_eq!(std::fs::read(src.join("m.bpr")).unwrap(), b"proof-v2");
        assert_eq!(
            std::fs::read_to_string(src.join("m.bps")).unwrap(),
            doctored
        );

        // The proof is deleted in Rodin: the next mirror deletes it next to
        // the sources too, and a re-seed is a pure no-op.
        std::fs::remove_file(project.join("m.bpr")).unwrap();
        let again = mirror_back_project(&ws, "proj", true).unwrap().unwrap();
        assert_eq!(again.deleted, ["m.bpr"]);
        assert!(!src.join("m.bpr").exists());
        let reseed = seed_project(&src, &ws, "proj").unwrap();
        assert_eq!((reseed.copied, reseed.replaced), (0, 0));
    }
}
