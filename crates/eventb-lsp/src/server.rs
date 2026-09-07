//! LSP Server implementation

use crate::lsp_types::*;
use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, watch};
use tower_lsp::jsonrpc::{Error, Result};
use tower_lsp::{Client, LanguageServer};
use tracing::{debug, info};

use rossi::operators::{OperatorId, spelling};
use rossi_build::walk::SOURCE_EXTENSION;

use crate::analysis;
use crate::code_actions::CodeActionProvider;
use crate::completion::CompletionProvider;
use crate::config::{ConfigManager, RossiConfig};
use crate::cross_references::CrossReferenceManager;
use crate::definition::DefinitionProvider;
use crate::document::{DocumentManager, ParsedDocument};
use crate::document_links::DocumentLinkProvider;
use crate::folding::FoldingRangeProvider;
use crate::hover::HoverProvider;
use crate::inlay_hints::InlayHintsProvider;
use crate::references::ReferenceProvider;
use crate::rename::RenameProvider;
use crate::selection_range::SelectionRangeProvider;
use crate::semantic_tokens::SemanticTokensProvider;
use crate::signature_help::SignatureHelpProvider;
use crate::workspace::WorkspaceSymbolProvider;

#[derive(Debug)]
struct WorkspaceScanState {
    complete: watch::Sender<bool>,
}

impl WorkspaceScanState {
    fn new() -> Self {
        let (complete, _) = watch::channel(false);
        Self { complete }
    }

    fn complete(&self) {
        self.complete.send_replace(true);
    }

    async fn wait(&self) {
        let mut complete = self.complete.subscribe();
        let _ = complete.wait_for(|complete| *complete).await;
    }
}

/// Refresh both indexes' saved layers for one file, reading and parsing it
/// once. A file that is gone drops its saved layer in each; any other read
/// error leaves both alone, so the two cannot disagree about what is on disk.
fn refresh_saved_layers(
    xrefs: &CrossReferenceManager,
    symbols: &WorkspaceSymbolProvider,
    uri: &Url,
) {
    let text = match uri.to_file_path().map(std::fs::read_to_string) {
        Ok(Ok(text)) => text,
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            xrefs.remove_disk_document(uri.as_str());
            symbols.remove_disk_document(uri.as_str());
            return;
        }
        Ok(Err(error)) => {
            info!("Failed to read {uri}: {error}");
            return;
        }
        Err(()) => {
            info!("Not a file URI, so nothing to read: {uri}");
            return;
        }
    };
    let components = crate::component_util::parse_all(&text);
    xrefs.index_disk_components(uri.to_string(), &components);
    symbols.index_disk_components(uri.to_string(), &components, &text);
}

/// Run blocking filesystem or parsing work away from Tokio's async workers.
async fn run_blocking<F, T>(task: F) -> Result<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(task).await.map_err(|error| {
        info!("Blocking LSP task failed: {error}");
        Error::internal_error()
    })
}

fn operator_characters(ids: &[OperatorId]) -> Vec<String> {
    ids.iter()
        .flat_map(|id| {
            let operator = spelling(*id);
            [operator.unicode.to_string(), operator.ascii.to_string()]
        })
        .collect()
}

fn signature_trigger_characters() -> Vec<String> {
    let mut characters = operator_characters(&[
        OperatorId::ForAll,
        OperatorId::Exists,
        OperatorId::Lambda,
        OperatorId::Dot,
        OperatorId::Bar,
    ]);
    characters.extend(["{".to_string(), ",".to_string()]);
    characters
}

fn signature_retrigger_characters() -> Vec<String> {
    let mut characters = operator_characters(&[OperatorId::Dot, OperatorId::Bar]);
    characters.push(",".to_string());
    characters
}

/// The shared handles the post-edit analysis needs, bundled so the inline
/// (`didOpen`/`didSave`/zero-debounce) and the spawned (debounced) paths run the
/// same code. `Clone` is a handful of `Arc`/`Client` clones, so the debounced
/// task moves one of these into its future instead of a fistful of fields.
#[derive(Clone)]
pub(crate) struct Analyzer {
    document_manager: Arc<DocumentManager>,
    cross_reference_manager: Arc<CrossReferenceManager>,
    workspace_symbol_provider: Arc<WorkspaceSymbolProvider>,
    config_manager: Arc<ConfigManager>,
    diagnostic_locks: Arc<DashMap<Url, Arc<Mutex<()>>>>,
    /// Per-component proof-status lines from the shared Rodin workspace,
    /// scoped to the source file each project was built from, maintained by
    /// the rodin sync watcher. Empty until a Rodin workspace exists.
    proof_status: Arc<parking_lot::RwLock<crate::rodin::sync::ProofStatusOverlay>>,
    /// Findings from eventb-animate lens runs, keyed by `(machine, mode)`
    /// and re-anchored onto the live buffers at publish time. Empty until a
    /// lens runs.
    animate_findings: Arc<parking_lot::RwLock<crate::animate::diagnostics::FindingsOverlay>>,
    client: Client,
}

impl Analyzer {
    /// Refresh the cross-reference and workspace-symbol indexes from `uri`'s
    /// stored parse, then publish its diagnostics. Reads the single source of
    /// truth once and fans it out to every eager index (none of which
    /// re-parses). Go-to-definition keeps no index — it resolves on demand
    /// against this same stored parse.
    pub(crate) async fn analyze(&self, uri: Url) {
        let Some(doc) = self.document_manager.parse_result(&uri) else {
            return;
        };
        let diagnostic_lock = self.diagnostic_lock(&uri);
        {
            let _publish_guard = diagnostic_lock.lock().await;
            let version = self
                .document_manager
                .with_current_snapshot(&uri, &doc, |version| {
                    // Commit both indexes while the exact snapshot remains current. A
                    // concurrent edit either happens before this guard (and skips the
                    // whole commit) or after it.
                    let key = uri.to_string();
                    let components = doc.components();
                    self.cross_reference_manager
                        .index_components(key.clone(), components);
                    self.workspace_symbol_provider
                        .index_components(key, components, doc.text());
                    version
                });

            if let Some(version) = version {
                // Workspace-wide diagnostics can be expensive, so derive them outside
                // the per-document state lock and recheck the snapshot before sending.
                let diagnostics = if self.config_manager.get().diagnostics.enabled {
                    self.diagnostics_for(&uri, &doc)
                } else {
                    vec![]
                };
                if self
                    .document_manager
                    .with_current_snapshot(&uri, &doc, |_| ())
                    .is_some()
                {
                    self.client
                        .publish_diagnostics(uri.clone(), diagnostics, Some(version))
                        .await;
                }
            }
        }
        self.evict_diagnostic_lock(&uri, &diagnostic_lock);
    }

    /// Clear diagnostics after every earlier analysis for this URI has either
    /// published or bowed out. The same lock also orders a subsequent reopen.
    async fn clear_diagnostics(&self, uri: Url) {
        let diagnostic_lock = self.diagnostic_lock(&uri);
        {
            let _publish_guard = diagnostic_lock.lock().await;
            self.client
                .publish_diagnostics(uri.clone(), vec![], None)
                .await;
        }
        self.evict_diagnostic_lock(&uri, &diagnostic_lock);
    }

    fn diagnostic_lock(&self, uri: &Url) -> Arc<Mutex<()>> {
        let entry = self
            .diagnostic_locks
            .entry(uri.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())));
        Arc::clone(entry.value())
    }

    fn evict_diagnostic_lock(&self, uri: &Url, lock: &Arc<Mutex<()>>) {
        self.diagnostic_locks.remove_if(uri, |_, current| {
            Arc::ptr_eq(current, lock) && Arc::strong_count(current) == 2
        });
    }

    /// All diagnostics for a parsed document: the parse errors and
    /// single-component lints from [`crate::diagnostics::document_diagnostics`],
    /// plus the cross-component checks (cycles, unresolved references, duplicate
    /// component names) read from the shared workspace graph.
    ///
    /// The cross-component checks see the recovered AST, so — like the
    /// single-component lints — they run only on a clean parse, lest a transient
    /// mid-edit syntax error flash a spurious cycle / duplicate / unknown
    /// reference. An edit refreshes the graph for the edited document only, so
    /// a dependent open file isn't re-published until it is itself touched:
    /// under editing, cross-file diagnostics are eventually consistent, not
    /// instantly so. A change arriving from disk is not mid-anything, so
    /// [`RossiLanguageServer::did_change_watched_files`] does republish every
    /// open document.
    fn diagnostics_for(&self, uri: &Url, doc: &ParsedDocument) -> Vec<Diagnostic> {
        let xrefs = &self.cross_reference_manager;
        let mut diags = crate::diagnostics::document_diagnostics(doc);
        // The proof-status overlay comes from disk, not the AST, so it is
        // emitted even mid-edit, before the clean-parse gate below.
        diags.extend(self.proof_status_diagnostics(uri, doc));
        // The operator-convention advisory is textual, so it too runs mid-edit.
        let format = &self.config_manager.get().format;
        if format.flags_ascii_operators() {
            diags.extend(crate::diagnostics::ascii_operator_diagnostics(
                doc.text(),
                format.emits_private_use_glyphs(),
            ));
        }
        // Animate findings are anchored by name and resolved via text-scan
        // fallbacks, so they too survive mid-edit breakage.
        diags.extend(crate::animate::diagnostics::animate_diagnostics(
            uri,
            doc,
            &self.animate_findings.read(),
        ));
        if !doc.parse().errors.is_empty() {
            return diags;
        }
        // Circular EXTENDS/REFINES need no workspace gating: a detected cycle is
        // always real (a self-loop is length-1).
        diags.extend(crate::diagnostics::cycle_diagnostics(
            doc.components(),
            &xrefs.detect_cycles(None),
            doc.text(),
        ));
        // Unresolved references / duplicate names would false-positive without a
        // workspace view (single-file mode indexes no siblings), so emit them
        // only once it is scanned.
        if xrefs.is_scanned() {
            diags.extend(crate::diagnostics::cross_reference_diagnostics(
                doc.components(),
                |kind, name| xrefs.contains(kind, name),
                doc.text(),
            ));
            diags.extend(crate::diagnostics::duplicate_component_diagnostics(
                doc.components(),
                |name| xrefs.component_declarations(name),
                doc.text(),
            ));
        }
        diags
    }

    /// The proof-status overlay's diagnostics for a document. Derived from
    /// disk state (Rodin's `.bps` files), not the AST, so — unlike the
    /// workspace-graph checks — it is *not* gated on a clean parse:
    /// [`Self::diagnostics_for`] emits it before its early return, matching
    /// the lens, which also survives mid-edit breakage. Cheap when no Rodin
    /// workspace exists, and the canonicalize syscall stays off the overlay
    /// lock.
    fn proof_status_diagnostics(&self, uri: &Url, doc: &ParsedDocument) -> Vec<Diagnostic> {
        if self.proof_status.read().is_empty() {
            return Vec::new();
        }
        let path = uri
            .to_file_path()
            .ok()
            .map(|p| std::fs::canonicalize(&p).unwrap_or(p));
        let proof_status = self.proof_status.read();
        crate::diagnostics::proof_status_diagnostics(doc, path.as_deref(), &proof_status)
    }

    /// The pretty-printer matching the user's formatting configuration, for
    /// rendering components imported back from Rodin.
    pub(crate) fn printer(&self) -> rossi::PrettyPrinter {
        self.config_manager.get().format.printer()
    }

    /// The LSP client, for flows (the rodin sync watcher) that message the
    /// user directly.
    pub(crate) fn client(&self) -> &Client {
        &self.client
    }

    /// A source file's current text: the open editor buffer when there is
    /// one (with its URI and version), else the file on disk. `path` must be
    /// canonicalized. The `(uri, version)` pair identifies the exact buffer
    /// snapshot for [`Self::apply_source_text`]'s staleness check.
    pub(crate) fn source_text(
        &self,
        path: &std::path::Path,
    ) -> Option<(Option<(Url, i32)>, String)> {
        if let Some((uri, version, text)) = self.document_manager.open_document_by_path(path) {
            return Some((Some((uri, version)), text));
        }
        std::fs::read_to_string(path).ok().map(|text| (None, text))
    }

    /// The open buffer's version, if the document is open. A direct lookup
    /// that copies nothing, unlike [`Self::source_text`], so it is cheap
    /// enough to poll.
    pub(crate) fn document_version(&self, uri: &Url) -> Option<i32> {
        self.document_manager.version(uri)
    }

    /// Replace a source file's content: via `workspace/applyEdit` for open
    /// documents (the editor shows the change, undo works), by writing the
    /// file for closed ones. `target` is the `(uri, version)` snapshot the
    /// replacement text was computed against ([`Self::source_text`]) and is
    /// the document the edit goes to; if the buffer has moved on since, the
    /// edit is refused rather than silently overwriting keystrokes typed in
    /// the meantime, and the edit itself is sent versioned so a conforming
    /// client rejects it too.
    pub(crate) async fn apply_source_text(
        &self,
        path: &std::path::Path,
        target: Option<(Url, i32)>,
        new_text: &str,
    ) -> std::result::Result<(), String> {
        let Some((uri, version)) = target else {
            return std::fs::write(path, new_text)
                .map_err(|e| format!("cannot write {}: {e}", path.display()));
        };
        // Full-document replacement against the *current* buffer text.
        let Some((current_version, current)) = self.document_manager.open_text_and_version(&uri)
        else {
            // Closed since we looked: fall back to disk.
            return std::fs::write(path, new_text)
                .map_err(|e| format!("cannot write {}: {e}", path.display()));
        };
        if current_version != version {
            return Err(
                "the document changed while the merge was computed; not applied".to_string(),
            );
        }
        let full_range = Range {
            start: Position::new(0, 0),
            end: crate::position::offset_to_position(&current, current.len()),
        };
        let edit = WorkspaceEdit {
            document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier {
                    uri,
                    version: Some(version),
                },
                edits: vec![OneOf::Left(TextEdit::new(full_range, new_text.to_string()))],
            }])),
            ..WorkspaceEdit::default()
        };
        match self.client.apply_edit(edit).await {
            Ok(response) if response.applied => Ok(()),
            Ok(response) => Err(format!(
                "the editor rejected the edit{}",
                response
                    .failure_reason
                    .map(|r| format!(": {r}"))
                    .unwrap_or_default()
            )),
            Err(e) => Err(format!("applyEdit failed: {e}")),
        }
    }

    /// Fold a proof-status scan into the overlay and, if anything changed,
    /// republish diagnostics for every open document. Called by the rodin
    /// sync watcher and the build flows after proof state changed on disk.
    /// Only diagnostics are republished — the parses did not change, so the
    /// cross-reference and symbol indexes are left alone.
    pub(crate) async fn refresh_proof_status(&self, update: crate::rodin::sync::ProofStatusUpdate) {
        if !self.proof_status.write().apply(update) {
            return;
        }
        self.republish_all_diagnostics().await;
    }

    /// Replace one `(machine, mode)` slot of the animate findings overlay
    /// and, if anything visible changed, republish diagnostics for every
    /// open document — an empty `findings` set retracts the previous run's.
    /// The animate counterpart of [`Self::refresh_proof_status`].
    pub(crate) async fn refresh_animate_findings(
        &self,
        machine: String,
        mode: crate::animate::AnimateMode,
        findings: Vec<crate::animate::diagnostics::Finding>,
    ) {
        if !self.animate_findings.write().apply(machine, mode, findings) {
            return;
        }
        self.republish_all_diagnostics().await;
    }

    /// Republish every open document's diagnostics, for a change to an input
    /// they all share — a proof-status or animate overlay, or the workspace
    /// graph. The buffers themselves are untouched, so this is the cheap
    /// half of [`Self::analyze`] per document rather than a re-analysis.
    pub(crate) async fn republish_all_diagnostics(&self) {
        for uri in self.document_manager.all_uris() {
            self.republish_diagnostics(uri).await;
        }
    }

    /// Recompute and publish a document's diagnostics from its stored parse,
    /// without re-committing the cross-reference/symbol indexes — the cheap
    /// half of [`Self::analyze`], for refreshes where only diagnostic inputs
    /// (like the proof-status overlay) changed.
    async fn republish_diagnostics(&self, uri: Url) {
        let Some(doc) = self.document_manager.parse_result(&uri) else {
            return;
        };
        let diagnostic_lock = self.diagnostic_lock(&uri);
        {
            let _publish_guard = diagnostic_lock.lock().await;
            let version = self
                .document_manager
                .with_current_snapshot(&uri, &doc, |version| version);
            if let Some(version) = version {
                let diagnostics = if self.config_manager.get().diagnostics.enabled {
                    self.diagnostics_for(&uri, &doc)
                } else {
                    vec![]
                };
                if self
                    .document_manager
                    .with_current_snapshot(&uri, &doc, |_| ())
                    .is_some()
                {
                    self.client
                        .publish_diagnostics(uri.clone(), diagnostics, Some(version))
                        .await;
                }
            }
        }
        self.evict_diagnostic_lock(&uri, &diagnostic_lock);
    }
}

/// Lifecycle of the Rodin workspace watcher. Creation happens on a detached
/// thread (it can take minutes when the platform's file-event service is
/// busy), so a `Starting` marker keeps concurrent starts from stacking up
/// and lets shutdown or a workspace switch discard a late arrival.
enum RodinSyncState {
    Off,
    Starting(PathBuf),
    Ready(crate::rodin::sync::RodinSyncManager),
}

/// Gap between attempts while waiting for a launching Rodin to publish its
/// bridge. The window itself is the lens's own wait for the workspace lock,
/// since that is the launch this is keeping pace with.
const LIVE_SYNC_RETRY: Duration = Duration::from_secs(2);

/// Lifecycle of the held bridge connection carrying Rodin's unsaved edits.
/// Connecting happens off the request path (a stale descriptor costs a connect
/// timeout), so — exactly as for [`RodinSyncState`] — a `Starting` marker keeps
/// concurrent starts from stacking up and lets shutdown or a workspace switch
/// discard a late arrival instead of adopting it.
enum LiveSyncState {
    Off,
    /// A start in flight, with the lease that identifies it: the path alone
    /// cannot tell two starts for the same workspace apart, and a superseded
    /// one must not report its own failure over the lease that replaced it.
    Starting(PathBuf, u64),
    Ready(crate::rodin::live::LiveSyncManager),
}

/// The Rossi Language Server
pub struct RossiLanguageServer {
    /// LSP client for sending notifications and requests
    client: Client,
    /// Configuration manager
    config_manager: Arc<ConfigManager>,
    /// Document manager for tracking open documents
    document_manager: Arc<DocumentManager>,
    /// Cross-reference manager for workspace-wide dependencies
    cross_reference_manager: Arc<CrossReferenceManager>,
    /// Completion provider
    completion_provider: Arc<CompletionProvider>,
    /// Hover provider
    hover_provider: Arc<HoverProvider>,
    /// Definition provider
    definition_provider: Arc<DefinitionProvider>,
    /// Reference provider
    reference_provider: Arc<ReferenceProvider>,
    /// Rename provider
    rename_provider: Arc<RenameProvider>,
    /// Workspace symbol provider
    workspace_symbol_provider: Arc<WorkspaceSymbolProvider>,
    /// Completion signal for the initial disk-backed workspace scan.
    workspace_scan_state: WorkspaceScanState,
    /// Semantic tokens provider
    semantic_tokens_provider: Arc<SemanticTokensProvider>,
    /// Document links provider
    document_links_provider: Arc<DocumentLinkProvider>,
    /// Code actions provider
    code_actions_provider: Arc<CodeActionProvider>,
    /// Folding range provider
    folding_range_provider: Arc<FoldingRangeProvider>,
    /// Inlay hints provider
    inlay_hints_provider: Arc<InlayHintsProvider>,
    /// Selection range provider (smart expand/shrink selection)
    selection_range_provider: Arc<SelectionRangeProvider>,
    /// Signature help provider
    signature_help_provider: Arc<SignatureHelpProvider>,
    /// Shared handles for the post-edit analysis, reused by the inline and
    /// debounced paths.
    analyzer: Analyzer,
    /// Single-flight guard for the Open in Rodin flow: a second invocation
    /// while one runs (builds, registers, launches, and then waits for the
    /// launched Rodin to take the workspace lock) is refused, not queued.
    /// The boot wait is what keeps a second lens click during Rodin's
    /// startup from launching a duplicate instance.
    rodin_open_in_flight: SingleFlight,
    /// Single-flight guard shared by both eventb-animate lens commands: one
    /// JVM + ProB instance at a time is the resource being protected.
    /// Deliberately separate from the Rodin guard — that one can be held
    /// for up to a minute waiting on Eclipse's workspace lock, and neither
    /// flow should block the other.
    animate_in_flight: SingleFlight,
    /// Whether the client advertised `window.workDoneProgress` support.
    supports_work_done_progress: std::sync::atomic::AtomicBool,
    /// Whether the client advertised `workspace.inlayHint.refreshSupport`.
    supports_inlay_hint_refresh: std::sync::atomic::AtomicBool,
    /// Whether the client advertised
    /// `workspace.didChangeWatchedFiles.dynamicRegistration`, i.e. whether it
    /// will watch the source tree for us if asked.
    supports_watched_files_registration: std::sync::atomic::AtomicBool,
    /// Watcher over the shared Rodin workspace, started lazily once such a
    /// workspace exists; dropped (stopped) on shutdown. `Arc`'d because the
    /// slow watcher creation completes on a detached thread.
    rodin_sync: Arc<parking_lot::Mutex<RodinSyncState>>,
    /// Content hashes of files the server wrote into the Rodin workspace —
    /// the sync watcher's echo guard, shared with the build flow.
    rodin_written: crate::rodin::sync::WrittenFiles,
    /// Per-project debounce generations for rebuild-on-save: a burst of
    /// saves collapses to one rebuild of each affected project.
    rodin_rebuild_generations: Arc<parking_lot::Mutex<std::collections::HashMap<PathBuf, u64>>>,
    /// The Rodin session stop monitor (at most one per workspace): armed by
    /// the Open in Rodin flow, mirrors proof files back next to the sources
    /// when the launched Rodin releases the workspace lock.
    rodin_session_monitor: crate::rodin::proof_mirror::SessionMonitorSlot,
    /// The held bridge connection carrying Rodin's unsaved edits, when
    /// `rossi.rodin.liveSync` is on and a plug-in is there to push them.
    /// Dropped on shutdown and whenever the workspace or the setting changes.
    rodin_live: Arc<parking_lot::Mutex<LiveSyncState>>,
    /// Hands each live-sync start the lease it checks before installing
    /// anything. Only ever read and bumped under `rodin_live`.
    rodin_live_lease: std::sync::atomic::AtomicU64,
}

/// A single-flight command guard: [`SingleFlight::try_begin`] either yields
/// the release guard or reports the command busy, so acquisition can never
/// be separated from release — a handler cannot latch the flag by returning
/// early without a guard to drop.
#[derive(Default)]
struct SingleFlight(Arc<std::sync::atomic::AtomicBool>);

impl SingleFlight {
    /// Take the flight slot. `None` while a previous holder's guard lives.
    fn try_begin(&self) -> Option<InFlightReset> {
        use std::sync::atomic::Ordering;
        (!self.0.swap(true, Ordering::SeqCst)).then(|| InFlightReset(Arc::clone(&self.0)))
    }
}

/// Resets the single-flight flag when the task future is dropped —
/// completion and panic alike — so one failure can't wedge the command
/// forever.
struct InFlightReset(Arc<std::sync::atomic::AtomicBool>);

impl Drop for InFlightReset {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// The Rodin workspace project a source file maps to — see
/// [`RossiLanguageServer::rodin_project_target`].
struct RodinProjectTarget {
    source_dir: PathBuf,
    workspace_dir: PathBuf,
    project_name: String,
    /// `workspace_dir`/`project_name`. Not existence-checked; callers gate
    /// as they need.
    project_dir: PathBuf,
}

/// The `file://` document URI every lens command carries as its first
/// argument — the shared decode for the `workspace/executeCommand` handlers,
/// erroring with the command's own name.
fn file_uri_argument(params: &ExecuteCommandParams) -> Result<Url> {
    let uri = params
        .arguments
        .first()
        .and_then(|value| value.as_str())
        .and_then(|value| Url::parse(value).ok())
        .ok_or_else(|| {
            Error::invalid_params(format!(
                "{} expects a document URI argument",
                params.command
            ))
        })?;
    if uri.to_file_path().is_err() {
        return Err(Error::invalid_params(format!(
            "{} needs a file:// URI",
            params.command
        )));
    }
    Ok(uri)
}

impl RossiLanguageServer {
    /// Create a new Rossi Language Server
    pub fn new(client: Client) -> Self {
        info!("Creating Rossi Language Server");

        // Create shared managers
        let cross_reference_manager = Arc::new(CrossReferenceManager::new());
        let document_manager = Arc::new(DocumentManager::new());

        // Create definition provider and set cross-reference and document managers
        let mut definition_provider = DefinitionProvider::new();
        definition_provider.set_cross_reference_manager(Arc::clone(&cross_reference_manager));
        definition_provider.set_document_manager(Arc::clone(&document_manager));

        // Create reference provider and set cross-reference manager
        let mut reference_provider = ReferenceProvider::new();
        reference_provider.set_cross_reference_manager(Arc::clone(&cross_reference_manager));
        reference_provider.set_document_manager(Arc::clone(&document_manager));

        // Create rename provider and set cross-reference manager
        let mut rename_provider = RenameProvider::new();
        rename_provider.set_cross_reference_manager(Arc::clone(&cross_reference_manager));
        rename_provider.set_document_manager(Arc::clone(&document_manager));

        // Create completion provider and set cross-reference and document managers
        let mut completion_provider = CompletionProvider::new();
        completion_provider.set_cross_reference_manager(Arc::clone(&cross_reference_manager));
        completion_provider.set_document_manager(Arc::clone(&document_manager));

        // Create hover provider and set cross-reference and document managers
        let mut hover_provider = HoverProvider::new();
        hover_provider.set_cross_reference_manager(Arc::clone(&cross_reference_manager));
        hover_provider.set_document_manager(Arc::clone(&document_manager));

        // Create document links provider and set cross-reference manager
        let mut document_links_provider = DocumentLinkProvider::new();
        document_links_provider.set_cross_reference_manager(Arc::clone(&cross_reference_manager));

        // Shared handles. The config manager and workspace-symbol index are Arc'd
        // up front so the analyzer's eager indexing and the request handlers share
        // one instance. The definition provider keeps no index — it resolves on
        // demand — so it is a request-handler field only, not fanned out to.
        let config_manager = Arc::new(ConfigManager::new());
        let definition_provider = Arc::new(definition_provider);
        // One identity table across both indexes: a file resolves to the same
        // key in each, and is canonicalized once rather than once per index.
        let workspace_symbol_provider = Arc::new(WorkspaceSymbolProvider::with_uris(Arc::clone(
            cross_reference_manager.document_uris(),
        )));
        let workspace_scan_state = WorkspaceScanState::new();
        let analyzer = Analyzer {
            document_manager: Arc::clone(&document_manager),
            cross_reference_manager: Arc::clone(&cross_reference_manager),
            workspace_symbol_provider: Arc::clone(&workspace_symbol_provider),
            config_manager: Arc::clone(&config_manager),
            diagnostic_locks: Arc::new(DashMap::new()),
            proof_status: Arc::new(parking_lot::RwLock::new(
                crate::rodin::sync::ProofStatusOverlay::default(),
            )),
            animate_findings: Arc::new(parking_lot::RwLock::new(
                crate::animate::diagnostics::FindingsOverlay::default(),
            )),
            client: client.clone(),
        };

        let inlay_hints_provider = Arc::new(InlayHintsProvider::new(
            Arc::clone(&document_manager),
            Arc::clone(&cross_reference_manager),
        ));

        Self {
            client,
            config_manager,
            document_manager,
            cross_reference_manager,
            completion_provider: Arc::new(completion_provider),
            hover_provider: Arc::new(hover_provider),
            definition_provider,
            reference_provider: Arc::new(reference_provider),
            rename_provider: Arc::new(rename_provider),
            workspace_symbol_provider,
            workspace_scan_state,
            semantic_tokens_provider: Arc::new(SemanticTokensProvider::new()),
            document_links_provider: Arc::new(document_links_provider),
            code_actions_provider: Arc::new(CodeActionProvider::new()),
            folding_range_provider: Arc::new(FoldingRangeProvider::new()),
            inlay_hints_provider,
            selection_range_provider: Arc::new(SelectionRangeProvider::new()),
            signature_help_provider: Arc::new(SignatureHelpProvider::new()),
            analyzer,
            rodin_open_in_flight: SingleFlight::default(),
            animate_in_flight: SingleFlight::default(),
            supports_work_done_progress: std::sync::atomic::AtomicBool::new(false),
            supports_inlay_hint_refresh: std::sync::atomic::AtomicBool::new(false),
            supports_watched_files_registration: std::sync::atomic::AtomicBool::new(false),
            rodin_sync: Arc::new(parking_lot::Mutex::new(RodinSyncState::Off)),
            rodin_live: Arc::new(parking_lot::Mutex::new(LiveSyncState::Off)),
            rodin_live_lease: std::sync::atomic::AtomicU64::new(0),
            rodin_written: Arc::new(crate::rodin::sync::WriteRegistry::default()),
            rodin_rebuild_generations: Arc::new(parking_lot::Mutex::new(
                std::collections::HashMap::new(),
            )),
            rodin_session_monitor: Arc::new(parking_lot::Mutex::new(None)),
        }
    }

    /// The shared Rodin workspace directory for the current configuration,
    /// or `None` when no root is known and no override is set. A relative
    /// `rossi.rodin.workspace` setting is anchored at the workspace root —
    /// never at the server process's working directory, which is an
    /// arbitrary location under most editors.
    fn resolved_rodin_workspace(&self, fallback_root: Option<&std::path::Path>) -> Option<PathBuf> {
        let root = || {
            self.cross_reference_manager
                .workspace_root()
                .or_else(|| fallback_root.map(std::path::Path::to_path_buf))
        };
        let configured = self.config_manager.get().rodin.workspace.trim().to_string();
        if !configured.is_empty() {
            let configured = PathBuf::from(configured);
            if configured.is_absolute() {
                return Some(configured);
            }
            return match root() {
                Some(root) => Some(root.join(configured)),
                None => Some(configured),
            };
        }
        root().map(|root| rossi_build::workspace::default_workspace_dir(&root))
    }

    /// The Rodin workspace project the file at `uri` maps to — the one
    /// resolution (source dir → workspace → project name) every consumer of
    /// an existing project must agree on with the Open in Rodin flow, so
    /// rebuild-on-save and the animate po lens read exactly the directory
    /// Rodin records into. `None` for non-file URIs, files without a usable
    /// parent, and when no workspace resolves.
    fn rodin_project_target(&self, uri: &Url) -> Option<RodinProjectTarget> {
        let source_dir = uri
            .to_file_path()
            .ok()?
            .parent()
            .map(std::path::Path::to_path_buf)
            .filter(|dir| !dir.as_os_str().is_empty())?;
        let workspace_dir = self.resolved_rodin_workspace(Some(&source_dir))?;
        let project_name = rossi_build::workspace::project_name_for(
            &source_dir,
            self.cross_reference_manager.workspace_root().as_deref(),
        );
        let project_dir = workspace_dir.join(&project_name);
        Some(RodinProjectTarget {
            source_dir,
            workspace_dir,
            project_name,
            project_dir,
        })
    }

    /// The "Open in Rodin" command: resolve the request up front, refuse a
    /// concurrent run, and spawn the flow.
    async fn execute_rodin_open(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        use std::sync::atomic::Ordering;

        let uri = file_uri_argument(&params)?;
        let path = uri
            .to_file_path()
            .expect("file_uri_argument checked the scheme");
        let source_dir = path
            .parent()
            .map(std::path::Path::to_path_buf)
            .filter(|dir| !dir.as_os_str().is_empty())
            .ok_or_else(|| Error::invalid_params("the document has no parent directory"))?;

        let Some(reset) = self.rodin_open_in_flight.try_begin() else {
            self.client
                .show_message(MessageType::INFO, "Open in Rodin is already running.")
                .await;
            return Ok(None);
        };

        let config = self.config_manager.get();
        let workspace_dir = self
            .resolved_rodin_workspace(Some(&source_dir))
            .expect("a source dir fallback always yields a workspace dir");
        // Start the sync watcher before the build so even the first build's
        // results are watched; the directory must exist to be watchable
        // (creating it also serves the build below), and `rossi.rodin.sync`
        // is the watcher's master switch.
        if std::fs::create_dir_all(&workspace_dir).is_ok() && config.rodin.sync {
            self.ensure_rodin_sync(&workspace_dir);
            self.ensure_rodin_live(&workspace_dir, true);
        }
        let project_name = rossi_build::workspace::project_name_for(
            &source_dir,
            self.cross_reference_manager.workspace_root().as_deref(),
        );
        let request = crate::rodin::OpenRequest {
            source_dir,
            project_name,
            documents: Arc::clone(&self.document_manager),
            workspace_dir,
            configured_rodin_path: config.rodin.path.clone(),
            progress_supported: self.supports_work_done_progress.load(Ordering::Relaxed),
            written: Arc::clone(&self.rodin_written),
            mirror_proofs: config.rodin.mirror_proofs,
            bridge: config.rodin.bridge,
            session_monitor: Arc::clone(&self.rodin_session_monitor),
            analyzer: self.analyzer.clone(),
        };

        let client = self.client.clone();
        tokio::spawn(async move {
            let _reset = reset;
            crate::rodin::open_in_rodin(client, request).await;
        });
        Ok(None)
    }

    /// An eventb-animate lens command ([`crate::animate::COMMAND_CHECK`] /
    /// [`crate::animate::COMMAND_PO`]): validate the `[uri, machine]`
    /// arguments, refuse a concurrent run, and spawn the flow.
    async fn execute_animate(
        &self,
        mode: crate::animate::AnimateMode,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        use std::sync::atomic::Ordering;

        let uri = file_uri_argument(&params)?;
        let machine = params
            .arguments
            .get(1)
            .and_then(|value| value.as_str())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                Error::invalid_params(format!(
                    "{} expects a machine name argument",
                    params.command
                ))
            })?
            .to_string();

        let Some(reset) = self.animate_in_flight.try_begin() else {
            self.client
                .show_message(MessageType::INFO, "eventb-animate is already running.")
                .await;
            return Ok(None);
        };

        // The shared Rodin workspace project for the clicked file's
        // directory, when one exists — only Po mode reads recorded proof
        // state, so Check clicks skip the resolution entirely.
        let rodin_project_dir = matches!(mode, crate::animate::AnimateMode::Po)
            .then(|| self.rodin_project_target(&uri))
            .flatten()
            .map(|target| target.project_dir)
            .filter(|dir| dir.is_dir());

        let request = crate::animate::AnimateRequest {
            input: crate::animate::ExecuteInput {
                mode,
                uri,
                machine,
                documents: Arc::clone(&self.document_manager),
                cross_references: Arc::clone(&self.cross_reference_manager),
                config: self.config_manager.get().animate.clone(),
                rodin_project_dir,
            },
            progress_supported: self.supports_work_done_progress.load(Ordering::Relaxed),
            analyzer: self.analyzer.clone(),
        };

        let client = self.client.clone();
        tokio::spawn(async move {
            let _reset = reset;
            crate::animate::run(client, request).await;
        });
        Ok(None)
    }

    /// Replace the watcher state unless `keep` says the current state
    /// already serves; returns whether a replacement happened. A superseded
    /// `Ready` watcher is dropped off the lock and off the runtime, on a
    /// detached thread — FSEvents teardown can block just like creation
    /// does, and this is the one place that rule lives.
    fn replace_rodin_sync(
        &self,
        next: RodinSyncState,
        keep: impl FnOnce(&RodinSyncState) -> bool,
    ) -> bool {
        let previous = {
            let mut state = self.rodin_sync.lock();
            if keep(&state) {
                return false;
            }
            std::mem::replace(&mut *state, next)
        };
        if matches!(previous, RodinSyncState::Ready(_)) {
            std::thread::spawn(move || drop(previous));
        }
        true
    }

    /// Start the watcher for the currently configured Rodin workspace, but
    /// only when that directory already exists — watching only pre-existing
    /// directories keeps `.rossi/` from appearing in projects that never
    /// used Rodin. The policy shared by `initialized` (a workspace left by
    /// an earlier session) and configuration changes (re-targeting, or
    /// flipping `rossi.rodin.sync`, the mutual-synchronization master
    /// switch — off tears a running watcher down).
    fn ensure_rodin_sync_for_existing_workspace(&self) {
        if !self.config_manager.get().rodin.sync {
            *self.rodin_live.lock() = LiveSyncState::Off;
            self.replace_rodin_sync(RodinSyncState::Off, |state| {
                matches!(state, RodinSyncState::Off)
            });
            return;
        }
        if let Some(rodin_workspace) = self.resolved_rodin_workspace(None)
            && rodin_workspace.is_dir()
        {
            self.ensure_rodin_sync(&rodin_workspace);
            self.ensure_rodin_live(&rodin_workspace, false);
        }
    }

    /// Start the Rodin workspace watcher if it isn't running (or starting)
    /// for this directory yet. Creation runs on a detached thread — it can
    /// take minutes when the platform's file-event service is backed up, and
    /// must block neither the request handler nor (via the blocking pool)
    /// runtime shutdown. Failures degrade to a log line — proof state then
    /// refreshes on the next build instead.
    fn ensure_rodin_sync(&self, workspace_dir: &std::path::Path) {
        let started = self.replace_rodin_sync(
            RodinSyncState::Starting(workspace_dir.to_path_buf()),
            |state| match state {
                RodinSyncState::Ready(manager) => manager.workspace_dir() == workspace_dir,
                RodinSyncState::Starting(dir) => dir == workspace_dir,
                RodinSyncState::Off => false,
            },
        );
        if !started {
            return;
        }

        let slot = Arc::clone(&self.rodin_sync);
        let written = Arc::clone(&self.rodin_written);
        let analyzer = self.analyzer.clone();
        let handle = tokio::runtime::Handle::current();
        let dir = workspace_dir.to_path_buf();
        std::thread::spawn(move || {
            let started = crate::rodin::sync::RodinSyncManager::start(
                &handle,
                dir.clone(),
                written,
                analyzer,
            );
            let mut state = slot.lock();
            // Only install if nothing superseded this start (shutdown reset
            // the state, or a config change targeted another directory).
            let current = matches!(&*state, RodinSyncState::Starting(d) if *d == dir);
            match started {
                Ok(manager) if current => *state = RodinSyncState::Ready(manager),
                Ok(_) => {}
                Err(e) => {
                    if current {
                        *state = RodinSyncState::Off;
                    }
                    info!("could not watch Rodin workspace {dir:?}: {e}");
                }
            }
        });
    }

    /// Debounced rebuild of the saved document's Rodin project, when
    /// `rossi.rodin.sync` is on (the default), the project already exists in
    /// the shared workspace, and a running Rodin holds the workspace lock —
    /// its seeded polling auto-refresh then picks the edit up within a few
    /// seconds, without another lens click. Errors only log — the editor
    /// already shows this document's diagnostics.
    fn schedule_rodin_rebuild(&self, uri: &Url) {
        if !self.config_manager.get().rodin.sync {
            return;
        }
        let Some(target) = self.rodin_project_target(uri) else {
            return;
        };
        if !target.project_dir.is_dir() {
            return;
        }
        let RodinProjectTarget {
            source_dir,
            workspace_dir,
            project_name,
            project_dir,
        } = target;

        let generation = {
            let mut generations = self.rodin_rebuild_generations.lock();
            let entry = generations.entry(project_dir.clone()).or_insert(0);
            *entry += 1;
            *entry
        };
        let generations = Arc::clone(&self.rodin_rebuild_generations);
        let document_manager = Arc::clone(&self.document_manager);
        let written = Arc::clone(&self.rodin_written);
        let bridge = self.config_manager.get().rodin.bridge;
        let analyzer = self.analyzer.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            {
                let mut generations = generations.lock();
                if generations.get(&project_dir) != Some(&generation) {
                    return; // superseded by a newer save
                }
                generations.remove(&project_dir);
            }
            // Only a running Rodin consumes these rebuilds — probe the lock
            // after the debounce, not at scheduling time, since Rodin may
            // have started or quit meanwhile.
            if !crate::rodin::lock::rebuild_on_save_wanted(
                crate::rodin::lock::workspace_lock_state(&workspace_dir),
            ) {
                debug!(
                    "rebuild-on-save skipped: no running Rodin holds {}",
                    workspace_dir.display()
                );
                return;
            }
            let result = crate::rodin::build_into_workspace(
                source_dir,
                document_manager,
                workspace_dir.clone(),
                project_name.clone(),
                &written,
                &analyzer,
            )
            .await;
            let outcome = match result {
                Ok(outcome) => {
                    debug!("rebuilt Rodin project {} on save", project_dir.display());
                    outcome
                }
                Err(message) => {
                    info!("rebuild-on-save skipped: {message}");
                    return;
                }
            };
            if bridge {
                crate::rodin::reload_in_rodin(&workspace_dir, &project_name, &outcome).await;
            }
        });
    }

    /// Start, or stop, the held bridge connection that carries Rodin's unsaved
    /// edits, so it follows `rossi.rodin.liveSync` and the workspace the
    /// watcher is on.
    ///
    /// Connecting is off the request path because a stale descriptor costs a
    /// connect timeout. Nothing here is reported to the user: no plug-in, an
    /// older one, or a Rodin that is not running all mean the same thing, that
    /// live sync is off and the save-driven merge still runs.
    fn ensure_rodin_live(&self, workspace_dir: &std::path::Path, launching: bool) {
        let config = self.config_manager.get();
        let wanted = config.rodin.sync && config.rodin.bridge && config.rodin.live_sync;
        let lease;
        {
            let mut slot = self.rodin_live.lock();
            if !wanted {
                // Unwanted now: what is there has to go.
                *slot = LiveSyncState::Off;
                return;
            }
            let current = match &*slot {
                // A connection that ended — Rodin quit — is not current: left
                // installed it would make every later call a no-op and live
                // sync would never come back for the Rodin that replaced it.
                LiveSyncState::Ready(manager) => {
                    manager.workspace_dir() == workspace_dir && manager.is_running()
                }
                LiveSyncState::Starting(dir, _) => dir == workspace_dir,
                LiveSyncState::Off => false,
            };
            if current {
                return;
            }
            lease = self
                .rodin_live_lease
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            *slot = LiveSyncState::Starting(workspace_dir.to_path_buf(), lease);
        }

        let slot = Arc::clone(&self.rodin_live);
        let analyzer = self.analyzer.clone();
        let dir = workspace_dir.to_path_buf();
        tokio::spawn(async move {
            // The lens asks for this *before* it launches Rodin, so on a
            // first click there is no bridge to find yet and one attempt would
            // always fail. Keep trying while that Rodin boots; `Starting` is
            // the lease, so a shutdown or a workspace switch ends the retries
            // by replacing it. Nothing is launching for the other callers
            // (startup, a settings change), so they get the single attempt
            // rather than a minute of polling a port that will not answer.
            let deadline = tokio::time::Instant::now()
                + if launching {
                    crate::rodin::BOOT_LOCK_TIMEOUT
                } else {
                    Duration::ZERO
                };
            loop {
                let started =
                    crate::rodin::live::LiveSyncManager::start(dir.clone(), analyzer.clone()).await;
                {
                    let mut slot = slot.lock();
                    // Only install if nothing superseded this start.
                    if !matches!(&*slot, LiveSyncState::Starting(_, held) if *held == lease) {
                        return;
                    }
                    match started {
                        Ok(manager) => {
                            info!("live sync with Rodin is on for {}", dir.display());
                            *slot = LiveSyncState::Ready(manager);
                            return;
                        }
                        Err(message) if tokio::time::Instant::now() >= deadline => {
                            *slot = LiveSyncState::Off;
                            info!("live sync with Rodin is off: {message}");
                            return;
                        }
                        Err(_) => {}
                    }
                }
                tokio::time::sleep(LIVE_SYNC_RETRY).await;
            }
        });
    }

    /// Ask the client to watch the workspace's Event-B sources, so writes made
    /// outside the editor reach [`Self::did_change_watched_files`]. Registering
    /// from the server rather than from each editor's client configuration
    /// gives every editor the same behaviour from one place, and ties the
    /// watcher's lifetime to a language server that actually started.
    ///
    /// A failure is logged and the session continues on the startup snapshot;
    /// the capability guard lives at the call site, since a server must not
    /// send requests the client never announced support for.
    async fn register_source_watcher(&self) {
        let registration = Registration {
            id: "rossi-eventb-source-watcher".to_string(),
            method: "workspace/didChangeWatchedFiles".to_string(),
            // A `DidChangeWatchedFilesRegistrationOptions` holding one
            // `FileSystemWatcher`, whose omitted `kind` means the default —
            // create, change and delete — which is what the graph must follow.
            // An LSP glob cannot express a negation, so the dot-directories
            // the scan skips are filtered server-side instead; see
            // `did_change_watched_files`.
            register_options: Some(serde_json::json!({
                "watchers": [{ "globPattern": format!("**/*.{SOURCE_EXTENSION}") }]
            })),
        };
        if let Err(error) = self.client.register_capability(vec![registration]).await {
            info!("Failed to register the Event-B source watcher: {error}");
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for RossiLanguageServer {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        info!(
            "Received initialize request from client: {:?}",
            params.client_info
        );

        // Extract workspace root from initialize params
        let workspace_root: Option<PathBuf> = params
            .workspace_folders
            .as_ref()
            .and_then(|folders| folders.first())
            .and_then(|folder| folder.uri.to_file_path().ok())
            .or_else(|| {
                params
                    .root_uri
                    .as_ref()
                    .and_then(|uri| uri.to_file_path().ok())
            })
            .or_else(|| {
                #[allow(deprecated)]
                params.root_path.as_ref().map(PathBuf::from)
            });

        if let Some(root) = workspace_root {
            info!("Workspace root: {:?}", root);
            self.cross_reference_manager.set_workspace_root(root);
        }

        self.supports_work_done_progress.store(
            params
                .capabilities
                .window
                .as_ref()
                .and_then(|window| window.work_done_progress)
                .unwrap_or(false),
            std::sync::atomic::Ordering::Relaxed,
        );

        self.supports_inlay_hint_refresh.store(
            params
                .capabilities
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.inlay_hint.as_ref())
                .and_then(|inlay_hint| inlay_hint.refresh_support)
                .unwrap_or(false),
            std::sync::atomic::Ordering::Relaxed,
        );

        self.supports_watched_files_registration.store(
            params
                .capabilities
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.did_change_watched_files.as_ref())
                .and_then(|watched_files| watched_files.dynamic_registration)
                .unwrap_or(false),
            std::sync::atomic::Ordering::Relaxed,
        );

        if let Some(settings) = params.initialization_options.as_ref() {
            match RossiConfig::from_client_settings(settings) {
                Ok(config) => {
                    info!("Applying initialization configuration: {:?}", config);
                    self.config_manager.update(config);
                }
                Err(e) => {
                    info!("Failed to parse initialization configuration: {}", e);
                }
            }
        }

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "eventb-language-server".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            capabilities: ServerCapabilities {
                // All positions this server emits/consumes are UTF-16 code units
                // (see `crate::position`). UTF-16 is the LSP default, so this is
                // an explicit statement of the contract rather than a change.
                position_encoding: Some(PositionEncodingKind::UTF16),
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::INCREMENTAL),
                        will_save: Some(false),
                        will_save_wait_until: Some(false),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(false),
                        })),
                    },
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(false),
                    trigger_characters: Some(vec![
                        ".".to_string(),
                        ":".to_string(),
                        "\\".to_string(),
                        "/".to_string(),
                        "!".to_string(),
                        "#".to_string(),
                    ]),
                    all_commit_characters: None,
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                    completion_item: None,
                }),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                document_formatting_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                })),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            work_done_progress_options: WorkDoneProgressOptions::default(),
                            legend: SemanticTokensProvider::legend(),
                            range: Some(false),
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                        },
                    ),
                ),
                document_link_provider: Some(DocumentLinkOptions {
                    resolve_provider: Some(false),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                }),
                code_action_provider: Some(CodeActionProviderCapability::Options(
                    CodeActionOptions {
                        code_action_kinds: Some(vec![
                            CodeActionKind::REFACTOR,
                            CodeActionKind::REFACTOR_EXTRACT,
                            CodeActionKind::QUICKFIX,
                            crate::code_actions::FIX_ALL_KIND,
                        ]),
                        work_done_progress_options: WorkDoneProgressOptions::default(),
                        resolve_provider: Some(false),
                    },
                )),
                code_lens_provider: Some(CodeLensOptions {
                    resolve_provider: Some(false),
                }),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec![
                        crate::rodin::COMMAND_OPEN.to_string(),
                        crate::animate::COMMAND_CHECK.to_string(),
                        crate::animate::COMMAND_PO.to_string(),
                    ],
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                }),
                folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
                inlay_hint_provider: Some(OneOf::Left(true)),
                selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(signature_trigger_characters()),
                    retrigger_characters: Some(signature_retrigger_characters()),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                }),
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        info!("Server initialized successfully");

        // Scan workspace for Event-B files to populate cross-reference index
        if let Some(root) = self.cross_reference_manager.workspace_root() {
            let manager = Arc::clone(&self.cross_reference_manager);
            let symbols = Arc::clone(&self.workspace_symbol_provider);
            match run_blocking(move || {
                manager.scan_workspace_with(&root, |uri, components, text| {
                    symbols.index_disk_components(uri, components, text);
                })
            })
            .await
            {
                Ok(Ok(count)) => {
                    info!("Indexed {} Event-B files from workspace", count);
                }
                Ok(Err(e)) => {
                    info!("Failed to scan workspace: {}", e);
                }
                Err(e) => info!("Failed to scan workspace: {}", e),
            }
        }
        self.workspace_scan_state.complete();

        // Ask the client to watch the source tree, now that the scan this
        // would otherwise race has finished. A client without dynamic
        // registration simply keeps the startup snapshot.
        if self
            .supports_watched_files_registration
            .load(std::sync::atomic::Ordering::Relaxed)
            && self.cross_reference_manager.workspace_root().is_some()
        {
            self.register_source_watcher().await;
        }

        // A Rodin workspace left by an earlier session carries proof state
        // worth surfacing right away.
        self.ensure_rodin_sync_for_existing_workspace();

        self.client
            .log_message(MessageType::INFO, "Rossi Language Server initialized")
            .await;
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        info!("Configuration change received");

        match RossiConfig::from_client_settings(&params.settings) {
            Ok(config) => {
                info!("Updating configuration: {:?}", config);
                self.config_manager.update(config);

                // A changed `rossi.rodin.workspace` must re-target the sync
                // watcher, or proof status and model-edit merges keep coming
                // from the abandoned directory. No-ops when unchanged.
                self.ensure_rodin_sync_for_existing_workspace();

                // Diagnostics depend on the configuration too (the enabled
                // switch, the operator-convention advisory), so every open
                // document is republished under the new settings.
                self.analyzer.republish_all_diagnostics().await;

                // Hints depend on the configuration (enabled state, label
                // rendering); ask clients that support it to re-request them
                // under the new settings. The capability guard is protocol
                // correctness, not error avoidance: a server must not send
                // requests the client never announced support for.
                if self
                    .supports_inlay_hint_refresh
                    .load(std::sync::atomic::Ordering::Relaxed)
                    && let Err(error) = self.client.inlay_hint_refresh().await
                {
                    info!("Inlay hint refresh failed: {error}");
                }

                self.client
                    .log_message(MessageType::INFO, "Configuration updated successfully")
                    .await;
            }
            Err(e) => {
                info!("Failed to parse configuration: {}", e);
                self.client
                    .log_message(
                        MessageType::WARNING,
                        format!("Failed to parse configuration: {}", e),
                    )
                    .await;
            }
        }
    }

    async fn shutdown(&self) -> Result<()> {
        info!("Received shutdown request");
        // Stop the Rodin workspace watcher and its processing task (a
        // creation still in flight sees the reset state and discards itself).
        *self.rodin_live.lock() = LiveSyncState::Off;
        self.replace_rodin_sync(RodinSyncState::Off, |_| false);
        // Abort the session stop monitor: with the server gone the mirror
        // cannot fire anyway, and the next lens click re-seeds.
        drop(self.rodin_session_monitor.lock().take());
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        let version = params.text_document.version;

        debug!("Document opened: {}", uri);

        let symbols = Arc::clone(&self.workspace_symbol_provider);
        let xrefs = Arc::clone(&self.cross_reference_manager);
        let uri_key = uri.to_string();
        if let Err(error) = run_blocking(move || {
            symbols.register_document_uri(&uri_key);
            xrefs.register_document_uri(&uri_key);
        })
        .await
        {
            info!("Failed to normalize document URI: {error}");
        }

        // Store the document; its parse is produced lazily on first read below.
        self.document_manager.open(uri.clone(), version, text);

        // Opening analyzes promptly (not debounced): refresh the eager indexes
        // and publish diagnostics from the document's stored parse.
        self.analyzer.analyze(uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        let changes = params.content_changes;

        debug!("Document changed: {} (version {})", uri, version);

        // Apply the text edit synchronously (cheap); the (re)parse is deferred
        // to the analysis below so a burst of keystrokes parses at most once.
        self.document_manager.change(&uri, version, changes);
        let revision = self.document_manager.revision(&uri);

        // Coalesce rapid edits behind the configured debounce window. A zero
        // window analyzes inline (the previous behaviour).
        let debounce_ms = self.config_manager.get().diagnostics.debounce_ms;
        if debounce_ms == 0 {
            self.analyzer.analyze(uri).await;
            return;
        }

        // Schedule the analysis after the window. Rather than tracking and
        // aborting prior tasks, each task checks at wake-up whether its unique
        // internal revision is still current. This also distinguishes a reopened
        // document whose LSP version counter happens to collide.
        let Some(revision) = revision else {
            return;
        };
        let analyzer = self.analyzer.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(debounce_ms as u64)).await;
            if analyzer.document_manager.revision(&uri) == Some(revision) {
                analyzer.analyze(uri).await;
            }
        });
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        debug!("Document closed: {}", uri);

        // Remove document from open-document tracking. Any debounced analysis
        // still pending for this URI finds no matching revision at wake-up and
        // bows out.
        self.document_manager.close(&uri);

        // Drop the closed document's cached inlay hints.
        self.inlay_hints_provider.evict(&uri);

        // Drop both open overlays, revealing what is saved for the file. No
        // read: each index already holds its disk layer.
        self.cross_reference_manager.remove_document(uri.as_ref());
        self.workspace_symbol_provider.remove_document(uri.as_ref());

        // Clear diagnostics
        self.analyzer.clear_diagnostics(uri).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        debug!("Document saved: {}", uri);

        // A save is a natural "done editing" signal: flush the analysis now
        // rather than leaving the user to wait out the remaining debounce window
        // for fresh diagnostics and indexes. A pending debounced task for the
        // same version then finds nothing newer and re-runs an identical (cheap,
        // memoised-parse) analysis.
        if self.document_manager.version(&uri).is_some() {
            self.analyzer.analyze(uri.clone()).await;
            // Both saved layers, not just the symbol one: a client without
            // dynamic registration sends no watched-file event, and this is
            // then the only moment either index learns the file changed.
            // Under the still-open buffer neither changes what the index
            // answers with, so this costs a read and a parse and no more.
            let xrefs = Arc::clone(&self.cross_reference_manager);
            let symbols = Arc::clone(&self.workspace_symbol_provider);
            let save_uri = uri.clone();
            if let Err(error) =
                run_blocking(move || refresh_saved_layers(&xrefs, &symbols, &save_uri)).await
            {
                info!("Failed to refresh saved layers for {uri}: {error}");
            }
        }

        self.schedule_rodin_rebuild(&uri);
    }

    /// Refresh the disk-backed indexes when Event-B files change outside the
    /// editor: a `git checkout`, a `rossi import`, a Rodin write, a file
    /// created in the explorer. The startup scan is otherwise the only moment
    /// the workspace graph learns what is on disk, so every cross-file
    /// diagnostic would be checked against a snapshot ageing from the first
    /// external write onwards.
    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        // Single-file mode indexes no siblings and `is_scanned()` keeps the
        // cross-file diagnostics off, so there is nothing to keep current.
        let Some(root) = self.cross_reference_manager.workspace_root() else {
            return;
        };

        // The reported `FileChangeType` is deliberately ignored: clients
        // coalesce and misreport it (a rename arrives as a delete plus a
        // create), and both refresh helpers below re-read the file and treat a
        // missing one as a removal. One uniform path is shorter than three and
        // cannot be desynchronised by a client's bookkeeping.
        // Events are filtered through the workspace scan's own two rules, and
        // collected into a set: a client may report one path twice in a batch
        // (a create plus a change), and re-reading it would be pure waste
        // since the refresh below reads from disk anyway. Order does not
        // matter for the same reason.
        let changed: std::collections::HashSet<Url> = params
            .changes
            .into_iter()
            .map(|change| change.uri)
            .filter(|uri| {
                uri.to_file_path().is_ok_and(|path| {
                    rossi_build::walk::is_source_file(&path)
                        && rossi_build::walk::is_within_source_walk(&root, &path)
                })
            })
            .collect();
        if changed.is_empty() {
            return;
        }
        debug!("Refreshing {} watched file(s)", changed.len());

        // One blocking hop for the whole batch, and one read and parse per
        // file within it — a branch switch delivers hundreds of events, and
        // both indexes take the same parse. Their disk and open layers are
        // kept apart, so a file is refreshed the same way whether or not the
        // editor holds it open: an open buffer keeps answering for its file,
        // and a file deleted while open still loses what was saved for it.
        let xrefs = Arc::clone(&self.cross_reference_manager);
        let symbols = Arc::clone(&self.workspace_symbol_provider);
        let refreshed = run_blocking(move || {
            for uri in changed {
                refresh_saved_layers(&xrefs, &symbols, &uri);
            }
        })
        .await;
        if let Err(error) = refreshed {
            info!("Failed to refresh watched files: {error}");
        }

        // The open buffers themselves did not change, only the workspace graph
        // they are checked against.
        self.analyzer.republish_all_diagnostics().await;
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri;
        debug!("Document symbol request for: {}", uri);

        // Read the document's shared parse (no per-request re-parse). A
        // multi-component file yields one root symbol per component, and
        // recovery keeps the outline alive through a local syntax error instead
        // of collapsing it to nothing. Symbols are sliced from the parse's own
        // text, so spans always index in bounds.
        let Some(doc) = self.document_manager.parse_result(&uri) else {
            debug!("Document not found: {}", uri);
            return Ok(None);
        };
        let components = doc.components();
        if components.is_empty() {
            debug!("No components recovered for document symbols: {}", uri);
            return Ok(None);
        }

        // Extract symbols with source text for accurate span information
        let symbols = components
            .iter()
            .flat_map(|component| analysis::extract_symbols(component, doc.text()))
            .collect();

        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        let uri = params.text_document.uri;
        debug!("Selection range request for: {}", uri);

        let manager = Arc::clone(&self.document_manager);
        let parse_uri = uri.clone();
        let Some(document) =
            run_blocking(move || manager.parse_result_for_request(&parse_uri)).await?
        else {
            debug!("Document not found: {}", uri);
            return Ok(None);
        };

        let ranges = self
            .selection_range_provider
            .selection_ranges(&document, &params.positions);
        Ok(Some(ranges))
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;
        debug!("Formatting request for: {}", uri);

        // Get document text
        let text = match self.document_manager.get_text(&uri) {
            Some(text) => text,
            None => {
                debug!("Document not found: {}", uri);
                return Ok(None);
            }
        };

        // Format the document
        let config = self.config_manager.get();
        match crate::formatting::format(&text, &config.format) {
            Ok(edits) => {
                debug!("Document formatted successfully: {}", uri);
                Ok(Some(edits))
            }
            Err(e) => {
                debug!("Failed to format document: {}", e);
                // Return None on error - don't crash the server
                Ok(None)
            }
        }
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        debug!("Completion request for: {} at {:?}", uri, position);

        // Get document text
        let text = match self.document_manager.get_text(uri) {
            Some(text) => text,
            None => {
                debug!("Document not found: {}", uri);
                return Ok(None);
            }
        };

        // Completion reads the document's shared parse from the document
        // manager — no per-request re-parse. Cross-file cold loads run on the
        // blocking pool so they cannot occupy an async handler thread.
        let config = self.config_manager.get();
        let provider = Arc::clone(&self.completion_provider);
        let response = run_blocking(move || {
            provider.complete(&params, &text, &config.completion, &config.format)
        })
        .await?;

        debug!(
            "Completion returned {} items",
            response.as_ref().map_or(0, |r| match r {
                CompletionResponse::Array(items) => items.len(),
                CompletionResponse::List(list) => list.items.len(),
            })
        );

        Ok(response)
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        debug!("Hover request for: {} at {:?}", uri, position);

        // Get document text
        let text = match self.document_manager.get_text(uri) {
            Some(text) => text,
            None => {
                debug!("Document not found: {}", uri);
                return Ok(None);
            }
        };

        // Hover reads the document's shared parse from the document manager —
        // no per-request re-parse. Cross-file cold loads run on the blocking
        // pool so they cannot occupy an async handler thread.
        let provider = Arc::clone(&self.hover_provider);
        let private_use_glyphs = self.config_manager.get().format.emits_private_use_glyphs();
        let response =
            run_blocking(move || provider.hover(&params, &text, private_use_glyphs)).await?;

        debug!(
            "Hover returned: {}",
            if response.is_some() {
                "Some(hover)"
            } else {
                "None"
            }
        );

        Ok(response)
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        debug!("Signature help request for: {} at {:?}", uri, position);

        let manager = Arc::clone(&self.document_manager);
        let parse_uri = uri.clone();
        let Some(document) =
            run_blocking(move || manager.parse_result_for_request(&parse_uri)).await?
        else {
            debug!("Document not found: {}", uri);
            return Ok(None);
        };

        let response = self
            .signature_help_provider
            .signature_help(&params, &document);

        debug!(
            "Signature help returned: {}",
            if response.is_some() {
                "Some(signature)"
            } else {
                "None"
            }
        );

        Ok(response)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        debug!("Go-to-definition request for: {} at {:?}", uri, position);

        // Get document text
        let text = match self.document_manager.get_text(uri) {
            Some(text) => text,
            None => {
                debug!("Document not found: {}", uri);
                return Ok(None);
            }
        };

        // Resolve cross-file definitions on the blocking pool because a cold
        // component load reads and parses its file.
        let provider = Arc::clone(&self.definition_provider);
        let response = run_blocking(move || provider.goto_definition(&params, &text)).await?;

        debug!(
            "Go-to-definition returned: {}",
            if response.is_some() {
                "Some(location)"
            } else {
                "None"
            }
        );

        Ok(response)
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        debug!("References request for: {} at {:?}", uri, position);

        // Get document text
        let text = match self.document_manager.get_text(uri) {
            Some(text) => text,
            None => {
                debug!("Document not found: {}", uri);
                return Ok(None);
            }
        };

        // Search cross-file references on the blocking pool because cold
        // component loads read and parse their files.
        let provider = Arc::clone(&self.reference_provider);
        let response = run_blocking(move || provider.find_references(&params, &text)).await?;

        debug!(
            "References returned: {} locations",
            response.as_ref().map_or(0, |v| v.len())
        );

        Ok(response)
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        let query = &params.query;
        debug!("Workspace symbol search for: '{}'", query);

        self.workspace_scan_state.wait().await;

        // Search across all indexed symbols
        let symbols = self.workspace_symbol_provider.search(query);

        debug!("Workspace symbol search returned {} symbols", symbols.len());

        Ok(Some(symbols))
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let uri = &params.text_document.uri;
        let position = params.position;
        debug!("Prepare rename request for: {} at {:?}", uri, position);

        // Get document text
        let text = match self.document_manager.get_text(uri) {
            Some(text) => text,
            None => {
                debug!("Document not found: {}", uri);
                return Ok(None);
            }
        };

        // Check if the symbol can be renamed
        let range = self.rename_provider.prepare_rename(&params, &text);

        if let Some(range) = range {
            debug!("Symbol at {:?} can be renamed", position);
            Ok(Some(PrepareRenameResponse::Range(range)))
        } else {
            debug!("Symbol at {:?} cannot be renamed", position);
            Ok(None)
        }
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let new_name = &params.new_name;
        debug!(
            "Rename request for: {} at {:?} to '{}'",
            uri, position, new_name
        );

        // Get document text
        let text = match self.document_manager.get_text(uri) {
            Some(text) => text,
            None => {
                debug!("Document not found: {}", uri);
                return Ok(None);
            }
        };

        // A component rename reads every closed workspace file, so keep the
        // complete operation off the async handler threads.
        let provider = Arc::clone(&self.rename_provider);
        let response = run_blocking(move || provider.rename(&params, &text)).await?;

        debug!(
            "Rename returned: {}",
            if response.is_some() {
                "Some(WorkspaceEdit)"
            } else {
                "None"
            }
        );

        Ok(response)
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = &params.text_document.uri;
        debug!("Semantic tokens request for: {}", uri);

        // Highlight from the document's shared parse (no per-request re-parse).
        // The builder slices text by component spans, so it must use the parse's
        // own text — never a separately fetched snapshot that a concurrent edit
        // could have advanced past those spans.
        let Some(doc) = self.document_manager.parse_result(uri) else {
            debug!("Document not found: {}", uri);
            return Ok(None);
        };
        let response =
            self.semantic_tokens_provider
                .semantic_tokens(&params, doc.text(), doc.components());

        debug!(
            "Semantic tokens returned: {}",
            if response.is_some() {
                "Some(tokens)"
            } else {
                "None"
            }
        );

        Ok(response)
    }

    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        let uri = &params.text_document.uri;
        debug!("Document link request for: {}", uri);

        // Get document text
        let text = match self.document_manager.get_text(uri) {
            Some(text) => text,
            None => {
                debug!("Document not found: {}", uri);
                return Ok(None);
            }
        };

        // Get document links
        let response = self.document_links_provider.document_links(&params, &text);

        debug!(
            "Document links returned: {}",
            response.as_ref().map_or(0, |links| links.len())
        );

        Ok(response)
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = &params.text_document.uri;
        debug!("Code action request for: {}", uri);

        // Get document text
        let text = match self.document_manager.get_text(uri) {
            Some(text) => text,
            None => {
                debug!("Document not found: {}", uri);
                return Ok(None);
            }
        };

        // Get code actions
        let format = &self.config_manager.get().format;
        let response = self.code_actions_provider.provide_code_actions(
            &params,
            &text,
            format.use_unicode,
            format.emits_private_use_glyphs(),
        );

        debug!(
            "Code actions returned: {}",
            response.as_ref().map_or(0, |actions| actions.len())
        );

        Ok(response)
    }

    async fn code_lens(&self, params: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        let uri = params.text_document.uri;
        debug!("Code lens request for: {}", uri);

        let Some(doc) = self.document_manager.parse_result(&uri) else {
            return Ok(None);
        };
        let mut lenses = crate::rodin::code_lenses(doc.components(), doc.text(), &uri);
        lenses.extend(crate::animate::code_lenses(
            doc.components(),
            doc.text(),
            &uri,
        ));
        Ok(Some(lenses))
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        match params.command.as_str() {
            crate::rodin::COMMAND_OPEN => self.execute_rodin_open(params).await,
            crate::animate::COMMAND_CHECK => {
                self.execute_animate(crate::animate::AnimateMode::Check, params)
                    .await
            }
            crate::animate::COMMAND_PO => {
                self.execute_animate(crate::animate::AnimateMode::Po, params)
                    .await
            }
            other => Err(Error::invalid_params(format!("unknown command: {other}"))),
        }
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        let uri = &params.text_document.uri;
        debug!("Folding range request for: {}", uri);

        // Fold from the document's shared, recovery-tolerant parse (no
        // per-request re-parse), so folds are derived from the same AST every
        // other feature reads and survive a local syntax error.
        let Some(doc) = self.document_manager.parse_result(uri) else {
            debug!("Document not found: {}", uri);
            return Ok(None);
        };
        let response = self
            .folding_range_provider
            .folding_ranges_from_components(doc.components(), doc.text());

        debug!(
            "Folding ranges returned: {}",
            response.as_ref().map_or(0, |ranges| ranges.len())
        );

        Ok(response)
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = params.text_document.uri;
        debug!("Inlay hint request for: {}", uri);

        let config = self.config_manager.get();
        if !config.inlay_hints.enabled {
            return Ok(None);
        }

        // The common case — scrolling and re-requests at an unchanged buffer
        // state — is a cache hit: a lock and two binary searches, served
        // inline without a blocking-pool round-trip.
        let range = params.range;
        if let Some(hints) = self.inlay_hints_provider.cached_hints(&uri, range, &config) {
            return Ok(Some(hints));
        }

        // A miss runs type inference over the document's dependency closure
        // on the blocking pool.
        let provider = Arc::clone(&self.inlay_hints_provider);
        let response = run_blocking(move || provider.compute_hints(&uri, range, &config)).await?;

        debug!(
            "Inlay hints returned: {}",
            response.as_ref().map_or(0, |hints| hints.len())
        );

        Ok(response)
    }
}

/// `OperatorRow` and its builder [`rossi::operators::operator_rows`] now live
/// next to their source table in [`rossi::operators`]. Re-exported here so the
/// `eventb_lsp::server::OperatorRow` path stays stable for clients of the
/// `rossi/operatorTable` request.
pub use rossi::operators::OperatorRow;

impl RossiLanguageServer {
    /// Custom request `rossi/operatorTable`: the single-source operator table
    /// exposed to editor-side input methods so the VSCode extension never
    /// duplicates the mapping in TypeScript.
    ///
    /// The handler must stay parameter-less. tower-lsp routes a handler that
    /// declares a params argument (even `_params: ()`) through its *required*
    /// params path, which rejects a request whose `params` field is absent with
    /// `-32602 "Missing params field"`. `vscode-languageclient` sends this
    /// request with no `params`, so a params-taking signature makes the server
    /// 404 the input method: the client's matcher never loads and neither eager
    /// combos (`/=`) nor the `\name` leader convert. Covered by the wire-level
    /// `tests/operator_table_test.rs` so it can't regress.
    ///
    /// The param-less form also rejects an *explicit* `params: null`, but that
    /// is moot: `vscode-languageclient` omits `params`, and no other client
    /// calls this method.
    pub async fn operator_table(&self) -> Result<Vec<OperatorRow>> {
        Ok(rossi::operators::operator_rows(
            self.config_manager.get().format.emits_private_use_glyphs(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{WorkspaceScanState, run_blocking};
    use std::time::Duration;

    #[tokio::test(flavor = "current_thread")]
    async fn blocking_work_runs_off_the_async_handler_thread() {
        let handler_thread = std::thread::current().id();
        let blocking_thread = run_blocking(|| std::thread::current().id()).await.unwrap();

        assert_ne!(blocking_thread, handler_thread);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn workspace_symbol_requests_wait_for_the_disk_scan() {
        let state = WorkspaceScanState::new();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), state.wait())
                .await
                .is_err()
        );

        state.complete();
        state.wait().await;
    }
}
