//! "Open in Rodin": build the current model into a persistent Rodin
//! workspace and open the Rodin IDE on it.
//!
//! Exposed to editors as a CodeLens on every `MACHINE`/`CONTEXT` header,
//! whose command ([`COMMAND_OPEN`]) the server executes. The lens'd file's
//! whole containing directory becomes one Rodin project (sibling `SEES`
//! targets must resolve, and that is also the granularity proof state syncs
//! at) inside a *stable* workspace — by default `<root>/.rossi/rodin` — so
//! everything Rodin writes there persists: `.bpr` proofs are never touched
//! and rebuilt `.bpo`/`.bps` reconcile against the previous state, which is
//! exactly what makes proofs survive model edits.

pub mod bridge;
pub mod build;
pub mod launch;
pub mod live;
pub mod lock;
pub mod model_sync;
pub(crate) mod proof_mirror;
pub mod sync;

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::lsp_types::*;
use crate::progress::Progress;
use tower_lsp::Client;

/// The `workspace/executeCommand` command behind the code lens.
pub const COMMAND_OPEN: &str = "rossi.rodin.open";

/// How every "Rodin is already running" message opens.
const ALREADY_RUNNING: &str = "Rodin is already running on this workspace:";

/// Refreshing a project's files does not reload an editor Rodin already has
/// open on a component, whichever way the files got there, so every arm that
/// reports a rebuilt project has to say this.
const REOPEN_NOTE: &str = "Editors already open on a component show the new \
     content after reopening it (or F5 inside the editor; the Explorer's F5 \
     does not reload files).";

/// The "Open in Rodin" lenses for a document: one per component, anchored on
/// the component's name. When the parse yields no components (mid-edit
/// breakage), fall back to scanning for `MACHINE`/`CONTEXT` header lines so
/// the lens stays visible while the user types.
pub fn code_lenses(components: &[rossi::Component], text: &str, uri: &Url) -> Vec<CodeLens> {
    let lenses: Vec<CodeLens> = components
        .iter()
        .filter_map(|component| {
            let span = component.name_span().or_else(|| component.span())?;
            Some(lens_at(crate::position::span_to_range(&span, text), uri))
        })
        .collect();
    if !lenses.is_empty() {
        return lenses;
    }
    header_line_scan(text, uri)
}

fn lens_at(range: Range, uri: &Url) -> CodeLens {
    CodeLens {
        range,
        command: Some(Command {
            title: "Open in Rodin".to_string(),
            command: COMMAND_OPEN.to_string(),
            arguments: Some(vec![serde_json::json!(uri.to_string())]),
        }),
        data: None,
    }
}

fn header_line_scan(text: &str, uri: &Url) -> Vec<CodeLens> {
    crate::text_utils::header_lines(text)
        .map(|header| {
            lens_at(
                crate::position::full_line_range(header.text, header.line as u32),
                uri,
            )
        })
        .collect()
}

/// Everything the spawned open-in-Rodin task needs, resolved up front by the
/// request handler so the task itself never touches server state.
pub struct OpenRequest {
    /// Directory whose Event-B text sources form the project.
    pub source_dir: PathBuf,
    /// The project's name in the shared workspace — computed by the caller
    /// via [`rossi_build::workspace::project_name_for`] so every path (lens,
    /// rebuild-on-save) agrees.
    pub project_name: String,
    /// Open documents, snapshotted lazily inside the build's blocking task
    /// so their text overlays the on-disk sources.
    pub documents: std::sync::Arc<crate::document::DocumentManager>,
    /// The shared Rodin workspace directory (holds `.metadata` + projects).
    pub workspace_dir: PathBuf,
    /// The raw `rossi.rodin.path` setting ("" → platform default).
    pub configured_rodin_path: String,
    /// Whether the client advertised `window.workDoneProgress`.
    pub progress_supported: bool,
    /// Shared record of files the server wrote into the workspace, so the
    /// sync watcher can tell the server's own writes from Rodin's.
    pub written: sync::WrittenFiles,
    /// The `rossi.rodin.mirrorProofs` setting: copy text-adjacent proof
    /// files into the project before the build, and mirror the project's
    /// proof files back next to the sources when the session ends.
    pub mirror_proofs: bool,
    /// The `rossi.rodin.bridge` setting: talk to a running Rodin's bridge
    /// plug-in when one is published, instead of only writing files at it.
    pub bridge: bool,
    /// Slot for the per-workspace Rodin session stop monitor.
    pub(crate) session_monitor: proof_mirror::SessionMonitorSlot,
    /// Analysis handles, for refreshing the proof-status overlay after the
    /// build (the watcher rightly ignores our own writes).
    pub(crate) analyzer: crate::server::Analyzer,
}

/// Build a source directory into the shared workspace under the write-guard
/// protocol every build must follow: the [`sync::begin_build`] guard is held
/// across the build so the watcher defers classifying events, the written
/// hashes are recorded before the guard drops, and the proof-status overlay
/// is refreshed afterwards (scoped to this project, spawned off the caller's
/// latency path — the watcher rightly classifies these writes as our own and
/// stays quiet). The one implementation of that ordering; both the lens flow
/// and rebuild-on-save go through here.
pub(crate) async fn build_into_workspace(
    source_dir: PathBuf,
    documents: std::sync::Arc<crate::document::DocumentManager>,
    workspace_dir: PathBuf,
    project_name: String,
    written: &sync::WrittenFiles,
    analyzer: &crate::server::Analyzer,
) -> Result<build::BuildOutcome, String> {
    let project_dir = workspace_dir.join(&project_name);
    let build_guard = sync::begin_build(written);
    let outcome = {
        let project_dir = project_dir.clone();
        tokio::task::spawn_blocking(move || {
            // The overlay is snapshotted here, off the async workers: it
            // canonicalizes and materializes every open buffer.
            let overlay = documents.open_file_texts();
            build::build_rodin_project(&source_dir, &overlay, &project_dir, &project_name)
        })
        .await
    };
    let outcome = match outcome {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(message)) => return Err(message),
        Err(join_error) => return Err(format!("the build task failed: {join_error}")),
    };
    written.record(outcome.written.iter().cloned());
    drop(build_guard);

    let analyzer = analyzer.clone();
    tokio::spawn(async move {
        sync::refresh(
            &workspace_dir,
            &analyzer,
            Some(std::collections::BTreeSet::from([project_dir])),
        )
        .await;
    });
    Ok(outcome)
}

/// Run the full flow: build into the shared project, register it in the
/// workspace on first contact, and launch (or defer to) the Rodin GUI.
/// All outcomes are reported through the client; the caller only spawns.
pub async fn open_in_rodin(client: Client, request: OpenRequest) {
    let platform = launch::Platform::current();
    let project_name = request.project_name.clone();
    let project_dir = request.workspace_dir.join(&project_name);

    let progress = Progress::begin(&client, request.progress_supported, "Open in Rodin").await;

    // The checkout's proof files are authoritative at session start: copy
    // them into the project before the build so the seeded `.bpo`/`.bps`
    // become its reconcile baselines and the `.bpr` proofs are in place when
    // Rodin opens. Skipped while a running Rodin holds the workspace — its
    // proof editors may hold newer state in memory than the files. Failures
    // never break the lens flow.
    let mut seeded = false;
    if request.mirror_proofs
        && lock::workspace_lock_state(&request.workspace_dir) != lock::LockState::Held
    {
        progress.report("seeding proof files").await;
        // Same write-guard protocol as the build: hold the guard across the
        // writes and record their hashes before it drops, so the watcher
        // never mistakes seeded files for Rodin's own proof edits.
        let seed_guard = sync::begin_build(&request.written);
        let (source_dir, workspace_dir, name) = (
            request.source_dir.clone(),
            request.workspace_dir.clone(),
            project_name.clone(),
        );
        let seed_result = tokio::task::spawn_blocking(move || {
            proof_mirror::seed_project(&source_dir, &workspace_dir, &name)
        })
        .await;
        match seed_result {
            Ok(Ok(report)) => {
                seeded = true;
                request.written.record(report.written);
                if report.replaced > 0 {
                    client
                        .show_message(
                            MessageType::INFO,
                            format!(
                                "Open in Rodin: replaced {} workspace proof file(s) \
                                 with the checkout copies.",
                                report.replaced
                            ),
                        )
                        .await;
                }
            }
            Ok(Err(e)) => tracing::info!("proof seeding skipped: {e}"),
            Err(join_error) => tracing::info!("proof seeding task failed: {join_error}"),
        }
        drop(seed_guard);
    }

    progress.report("building the Rodin project").await;

    let outcome = build_into_workspace(
        request.source_dir.clone(),
        request.documents,
        request.workspace_dir.clone(),
        project_name.clone(),
        &request.written,
        &request.analyzer,
    )
    .await;
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(message) => {
            return progress
                .finish(MessageType::ERROR, format!("Open in Rodin: {message}"))
                .await;
        }
    };
    if !outcome.error_diagnostics.is_empty() {
        // Matching `rossi build`: erroneous elements were dropped, the
        // checked output was still written — open Rodin on it anyway.
        client
            .show_message(
                MessageType::WARNING,
                format!(
                    "Open in Rodin: the build reported {} error diagnostic(s); \
                     erroneous elements were dropped from the checked files ({})",
                    outcome.error_diagnostics.len(),
                    outcome.error_diagnostics[0]
                ),
            )
            .await;
    }

    let lock_state = lock::workspace_lock_state(&request.workspace_dir);

    // The bridge plug-in, if this Rodin has one, can do inside the running
    // instance what nothing outside it can: register the project and bring it
    // forward. A bridge that answers is itself proof a Rodin is running, and
    // that is the only proof to be had where the lock cannot be probed at all
    // (Windows), so the attempt follows the same "not demonstrably free" rule
    // as `lock::rebuild_on_save_wanted` rather than waiting for `Held`.
    let bridged = if request.bridge && lock_state != lock::LockState::Free {
        register_through_bridge(&request.workspace_dir, &project_dir, &project_name).await
    } else {
        None
    };

    if bridged.is_some() || lock_state == lock::LockState::Held {
        // The running session still ends someday — arm the stop monitor so
        // its proofs are mirrored back even though this click launched
        // nothing (covers an LSP restarted mid-session). Only where the lock
        // says `Held`, though: the monitor watches for that lock to be
        // released, so where the probe cannot read it at all there is nothing
        // for it to see, and arming it would only promise a mirror that
        // cannot happen.
        if request.mirror_proofs && lock_state == lock::LockState::Held {
            proof_mirror::RodinSessionMonitor::arm(
                &request.session_monitor,
                &client,
                &request.workspace_dir,
                &project_name,
                seeded,
                &request.written,
            );
        }
        // A project that was never registered cannot be, while Rodin holds
        // the workspace: the headless registration needs the same Eclipse
        // workspace. Say so instead of promising an automatic pickup that
        // will never happen.
        let (level, text) = match bridged {
            Some(registered) => (
                MessageType::INFO,
                format!(
                    "{ALREADY_RUNNING} project '{registered}' was rebuilt and \
                     registered in it through the Rodin bridge. {REOPEN_NOTE}"
                ),
            ),
            None if launch::project_registered(&request.workspace_dir, &project_name) => (
                MessageType::INFO,
                format!(
                    "{ALREADY_RUNNING} project '{project_name}' was rebuilt; Rodin \
                     picks the files up within a few seconds. {REOPEN_NOTE}"
                ),
            ),
            None => (
                MessageType::WARNING,
                format!(
                    "{ALREADY_RUNNING} project '{project_name}' was built at {} but \
                     cannot be registered while Rodin holds the workspace. In Rodin, use \
                     File > Import > Existing Projects to add it, or close Rodin and run \
                     Open in Rodin again.",
                    project_dir.display()
                ),
            ),
        };
        return progress.finish(level, text).await;
    }

    let rodin_path = launch::effective_rodin_path(&request.configured_rodin_path, platform);
    // A setting that denotes a concrete path (rather than a name resolved
    // via PATH or macOS app activation) gets an actionable error before any
    // spawn attempt — the classification lives in `launch`, next to the
    // launch commands it must agree with.
    if let Some(concrete) = launch::concrete_path(&rodin_path, platform)
        && !concrete.exists()
    {
        return progress
            .finish(
                MessageType::ERROR,
                format!(
                    "Rodin was not found at {}. Install Rodin or point the \
                     rossi.rodin.path setting at it. (The project was still built at {}.)",
                    concrete.display(),
                    project_dir.display()
                ),
            )
            .await;
    }

    if !launch::project_registered(&request.workspace_dir, &project_name) {
        progress
            .report("registering the project in the Rodin workspace")
            .await;
        let registration = match launch::ant_runner_executable(&rodin_path, platform) {
            Ok(ant_runner) => {
                launch::register_project(&ant_runner, &request.workspace_dir, &project_dir).await
            }
            Err(message) => Err(message),
        };
        if let Err(message) = registration {
            return progress
                .finish(
                    MessageType::ERROR,
                    format!("Open in Rodin: {message} (check the rossi.rodin.path setting)"),
                )
                .await;
        }
    }
    launch::seed_workspace_prefs(&request.workspace_dir);

    progress.report("launching Rodin").await;
    let (command, args) = launch::launch_command(&rodin_path, &request.workspace_dir, platform);
    if let Err(message) = launch::launch_gui(&command, &args) {
        return progress
            .finish(
                MessageType::ERROR,
                format!("Open in Rodin: {message} (check the rossi.rodin.path setting)"),
            )
            .await;
    }
    progress
        .finish(
            MessageType::INFO,
            format!("Opened {project_name} in Rodin."),
        )
        .await;

    // The caller's single-flight guard lives for this future: keep it held
    // until the launched Rodin actually takes the workspace lock. Between
    // the spawn above and lock acquisition the lock probe reads `Free`, so
    // a second lens click during Rodin's boot would otherwise launch a
    // duplicate instance (doomed to Eclipse's "workspace in use" dialog).
    let state = wait_for_workspace_lock(&request.workspace_dir, BOOT_LOCK_TIMEOUT).await;
    // Only an observed `Held` arms the stop monitor: `Free` means Rodin
    // never came up, and after `Unknown` the release is unobservable anyway.
    if state == lock::LockState::Held && request.mirror_proofs {
        proof_mirror::RodinSessionMonitor::arm(
            &request.session_monitor,
            &client,
            &request.workspace_dir,
            &project_name,
            seeded,
            &request.written,
        );
    }
}

/// Ask a running Rodin to re-read the components a rebuild just wrote and
/// reload any editor open on them.
///
/// This is what spares the user an F5: Eclipse's seeded auto-refresh notices
/// the files on its own within a few seconds, but an editor already open on a
/// component keeps showing what it loaded. Best effort throughout, since the
/// auto-refresh remains the fallback and a missing bridge is the normal case.
pub(crate) async fn reload_in_rodin(
    workspace_dir: &Path,
    project_name: &str,
    outcome: &build::BuildOutcome,
) {
    // Only the sources Rodin reads; the checked and proof files it regenerates
    // itself, and reloading an editor on them means nothing.
    let files: Vec<String> = outcome
        .written
        .iter()
        .filter(|(path, _)| rossi_build::project::is_xml_component(path))
        .filter_map(|(path, _)| Some(path.file_name()?.to_str()?.to_string()))
        .collect();
    if files.is_empty() {
        return;
    }
    let bridge = match bridge::Bridge::connect(workspace_dir).await {
        Ok(bridge) => bridge,
        Err(message) => {
            tracing::debug!("no Rodin bridge to reload through: {message}");
            return;
        }
    };
    if !bridge.supports(bridge::RELOAD) {
        tracing::info!(
            "the Rodin {} bridge does not offer {}",
            bridge.rodin_version(),
            bridge::RELOAD
        );
        return;
    }
    match bridge.reload(project_name, &files).await {
        Ok(()) => tracing::debug!("reloaded {} component(s) in Rodin", files.len()),
        Err(message) => tracing::info!("the Rodin bridge could not reload: {message}"),
    }
}

/// Register and reveal the project through a running Rodin's bridge plug-in,
/// answering the name the workspace registered it under.
///
/// `None` means no bridge answered, whether because there is no plug-in, an
/// older one, or a descriptor left behind by a Rodin that was killed; the
/// caller then falls back to the file-mediated path. Nothing here is reported
/// to the user directly, since a missing bridge is the normal case for stock
/// Rodin, not a fault.
async fn register_through_bridge(
    workspace_dir: &Path,
    project_dir: &Path,
    project_name: &str,
) -> Option<String> {
    let bridge = match bridge::Bridge::connect(workspace_dir).await {
        Ok(bridge) => bridge,
        Err(message) => {
            tracing::info!("no Rodin bridge for this workspace: {message}");
            return None;
        }
    };
    if !bridge.supports(bridge::REGISTER) {
        tracing::info!(
            "the Rodin {} bridge does not offer {}",
            bridge.rodin_version(),
            bridge::REGISTER
        );
        return None;
    }
    let registered = match bridge.register_project(project_dir).await {
        Ok(registered) => registered,
        Err(message) => {
            tracing::info!("the Rodin bridge could not register the project: {message}");
            return None;
        }
    };
    // The workspace registers a project under the name in its `.project`
    // descriptor, not the directory's: reveal and report the name it answered
    // with, so a mismatch cannot point the user at a project nobody has.
    let registered = registered.unwrap_or_else(|| project_name.to_string());
    // Registering is the part that matters; failing to bring the window
    // forward is cosmetic and must not discard the success above. A plug-in
    // too old to offer the method answers `METHOD_NOT_FOUND`, which lands
    // here as any other failed reveal does.
    if let Err(message) = bridge.reveal_project(&registered).await {
        tracing::info!("the Rodin bridge could not reveal the project: {message}");
    }
    Some(registered)
}

/// How long a just-launched Rodin gets to take the workspace lock before
/// [`open_in_rodin`] stops waiting for it (and with that stops holding the
/// single-flight guard). Generous — cold Eclipse starts are slow — but
/// bounded, so a Rodin that failed to come up re-enables the lens.
pub(crate) const BOOT_LOCK_TIMEOUT: Duration = Duration::from_secs(60);

/// Poll the workspace lock until it leaves [`lock::LockState::Free`] or the
/// timeout passes, returning the state that ended the wait (`Free` on
/// timeout). `Held` means the launched instance owns the workspace;
/// `Unknown` (Windows, probe failures) ends the wait too — the probe cannot
/// observe more there, and on Windows the `.lock` file appearing is itself
/// the boot signal that moves the state off `Free`.
async fn wait_for_workspace_lock(workspace_dir: &Path, timeout: Duration) -> lock::LockState {
    const POLL: Duration = Duration::from_secs(1);
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let state = lock::workspace_lock_state(workspace_dir);
        if state != lock::LockState::Free || tokio::time::Instant::now() >= deadline {
            return state;
        }
        tokio::time::sleep(POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn boot_lock_wait_is_bounded_on_a_free_workspace() {
        // A workspace nothing ever locks stays `Free`; the wait must end at
        // the deadline instead of holding the single-flight guard forever,
        // and report `Free` so the caller does not arm a stop monitor.
        let state =
            wait_for_workspace_lock(Path::new("/nonexistent/rossi-boot-ws"), Duration::ZERO).await;
        assert_eq!(state, lock::LockState::Free);
    }

    #[test]
    fn header_scan_finds_machine_and_context_only() {
        let uri = Url::parse("file:///m.eventb").unwrap();
        let text = "CONTEXT c\nEND\nMACHINE m\nEND\nMACHINERY x\nCONTEXTUAL y\n";
        let lenses = header_line_scan(text, &uri);
        assert_eq!(lenses.len(), 2);
        assert_eq!(lenses[0].range.start.line, 0);
        assert_eq!(lenses[1].range.start.line, 2);
        let cmd = lenses[0].command.as_ref().unwrap();
        assert_eq!(cmd.command, COMMAND_OPEN);
        assert_eq!(
            cmd.arguments.as_ref().unwrap()[0],
            serde_json::json!("file:///m.eventb")
        );
    }

    #[test]
    fn parsed_components_anchor_lenses_on_name_spans() {
        let uri = Url::parse("file:///m.eventb").unwrap();
        let text = "MACHINE counters\nEND\n";
        let components = rossi::parse_components(text).unwrap();
        let lenses = code_lenses(&components, text, &uri);
        assert_eq!(lenses.len(), 1);
        assert_eq!(lenses[0].range.start.line, 0);
        // The lens anchors on the name token, not column zero.
        assert_eq!(lenses[0].range.start.character, 8);
    }
}
