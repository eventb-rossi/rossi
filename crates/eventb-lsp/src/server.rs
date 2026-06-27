//! LSP Server implementation

use crate::lsp_types::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tower_lsp::jsonrpc::Result;
use tower_lsp::{Client, LanguageServer};
use tracing::{debug, info};

use crate::analysis;
use crate::code_actions::CodeActionProvider;
use crate::completion::CompletionProvider;
use crate::config::{ConfigManager, RossiConfig};
use crate::cross_references::CrossReferenceManager;
use crate::definition::DefinitionProvider;
use crate::document::{DocumentManager, ParsedDocument};
use crate::document_links::DocumentLinkProvider;
use crate::folding::FoldingRangeProvider;
use crate::formatting::FormattingProvider;
use crate::hover::HoverProvider;
use crate::references::ReferenceProvider;
use crate::rename::RenameProvider;
use crate::selection_range::SelectionRangeProvider;
use crate::semantic_tokens::SemanticTokensProvider;
use crate::signature_help::SignatureHelpProvider;
use crate::workspace::WorkspaceSymbolProvider;

/// The shared handles the post-edit analysis needs, bundled so the inline
/// (`didOpen`/`didSave`/zero-debounce) and the spawned (debounced) paths run the
/// same code. `Clone` is a handful of `Arc`/`Client` clones, so the debounced
/// task moves one of these into its future instead of a fistful of fields.
#[derive(Clone)]
struct Analyzer {
    document_manager: Arc<DocumentManager>,
    cross_reference_manager: Arc<CrossReferenceManager>,
    workspace_symbol_provider: Arc<WorkspaceSymbolProvider>,
    config_manager: Arc<ConfigManager>,
    client: Client,
}

impl Analyzer {
    /// Refresh the cross-reference and workspace-symbol indexes from `uri`'s
    /// stored parse, then publish its diagnostics. Reads the single source of
    /// truth once and fans it out to every eager index (none of which
    /// re-parses). Go-to-definition keeps no index — it resolves on demand
    /// against this same stored parse.
    async fn analyze(&self, uri: Url) {
        // Read the single source of truth (and the version it is for) once, then
        // fan the same snapshot out to every eager index and the diagnostics.
        let doc = self.document_manager.parse_result(&uri);
        let version = self.document_manager.version(&uri);
        if let Some(doc) = &doc {
            let key = uri.to_string();
            let components = doc.components();
            self.cross_reference_manager
                .index_components(key.clone(), components);
            self.workspace_symbol_provider
                .index_components(key, components, &doc.text);
        }
        self.publish_diagnostics(uri, doc.as_deref(), version).await;
    }

    /// Publish `uri`'s diagnostics from the already-read parse `doc`, or clear
    /// them when diagnostics are disabled. The diagnostics
    /// ([`crate::diagnostics::document_diagnostics`]) are the parse errors plus,
    /// on a clean parse, the cheap single-component lints (duplicate / shadowed
    /// names, EB021-023). The publish is tagged with the document's current
    /// `version` so the version always identifies the text the diagnostics were
    /// computed from. Spans are mapped against that parse's own text, so a
    /// concurrent edit cannot make a span index past the text it is rendered
    /// into.
    async fn publish_diagnostics(
        &self,
        uri: Url,
        doc: Option<&ParsedDocument>,
        version: Option<i32>,
    ) {
        if !self.config_manager.get().diagnostics.enabled {
            self.client.publish_diagnostics(uri, vec![], version).await;
            return;
        }

        let diagnostics = doc.map(|doc| self.diagnostics_for(doc)).unwrap_or_default();
        self.client
            .publish_diagnostics(uri, diagnostics, version)
            .await;
    }

    /// All diagnostics for a parsed document: the parse errors and
    /// single-component lints from [`crate::diagnostics::document_diagnostics`],
    /// plus the cross-component checks (cycles, unresolved references, duplicate
    /// component names) read from the shared workspace graph.
    ///
    /// The cross-component checks see the recovered AST, so — like the
    /// single-component lints — they run only on a clean parse, lest a transient
    /// mid-edit syntax error flash a spurious cycle / duplicate / unknown
    /// reference. The graph is refreshed for the edited document only, so a
    /// dependent open file isn't re-published until it is itself touched:
    /// cross-file diagnostics are eventually consistent, not instantly so.
    fn diagnostics_for(&self, doc: &ParsedDocument) -> Vec<Diagnostic> {
        let xrefs = &self.cross_reference_manager;
        let mut diags = crate::diagnostics::document_diagnostics(doc);
        if !doc.parse.errors.is_empty() {
            return diags;
        }
        // Circular EXTENDS/REFINES need no workspace gating: a detected cycle is
        // always real (a self-loop is length-1).
        diags.extend(crate::diagnostics::cycle_diagnostics(
            doc.components(),
            &xrefs.detect_cycles(None),
            &doc.text,
        ));
        // Unresolved references / duplicate names would false-positive without a
        // workspace view (single-file mode indexes no siblings), so emit them
        // only once it is scanned.
        if xrefs.is_scanned() {
            diags.extend(crate::diagnostics::cross_reference_diagnostics(
                doc.components(),
                |kind, name| xrefs.contains(kind, name),
                &doc.text,
            ));
            diags.extend(crate::diagnostics::duplicate_component_diagnostics(
                doc.components(),
                |name| xrefs.component_definition_files(name),
                &doc.text,
            ));
        }
        diags
    }
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
    /// Formatting provider
    formatting_provider: Arc<FormattingProvider>,
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
    /// Semantic tokens provider
    semantic_tokens_provider: Arc<SemanticTokensProvider>,
    /// Document links provider
    document_links_provider: Arc<DocumentLinkProvider>,
    /// Code actions provider
    code_actions_provider: Arc<CodeActionProvider>,
    /// Folding range provider
    folding_range_provider: Arc<FoldingRangeProvider>,
    /// Selection range provider (smart expand/shrink selection)
    selection_range_provider: Arc<SelectionRangeProvider>,
    /// Signature help provider
    signature_help_provider: Arc<SignatureHelpProvider>,
    /// Shared handles for the post-edit analysis, reused by the inline and
    /// debounced paths.
    analyzer: Analyzer,
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
        let workspace_symbol_provider = Arc::new(WorkspaceSymbolProvider::new());
        let analyzer = Analyzer {
            document_manager: Arc::clone(&document_manager),
            cross_reference_manager: Arc::clone(&cross_reference_manager),
            workspace_symbol_provider: Arc::clone(&workspace_symbol_provider),
            config_manager: Arc::clone(&config_manager),
            client: client.clone(),
        };

        Self {
            client,
            config_manager,
            document_manager,
            cross_reference_manager,
            formatting_provider: Arc::new(FormattingProvider::new()),
            completion_provider: Arc::new(completion_provider),
            hover_provider: Arc::new(hover_provider),
            definition_provider,
            reference_provider: Arc::new(reference_provider),
            rename_provider: Arc::new(rename_provider),
            workspace_symbol_provider,
            semantic_tokens_provider: Arc::new(SemanticTokensProvider::new()),
            document_links_provider: Arc::new(document_links_provider),
            code_actions_provider: Arc::new(CodeActionProvider::new()),
            folding_range_provider: Arc::new(FoldingRangeProvider::new()),
            selection_range_provider: Arc::new(SelectionRangeProvider::new()),
            signature_help_provider: Arc::new(SignatureHelpProvider::new()),
            analyzer,
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
                        ]),
                        work_done_progress_options: WorkDoneProgressOptions::default(),
                        resolve_provider: Some(false),
                    },
                )),
                folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
                selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec![
                        "∀".to_string(),
                        "∃".to_string(),
                        "!".to_string(),
                        "#".to_string(),
                        "λ".to_string(),
                        "{".to_string(),
                        "·".to_string(),
                        ".".to_string(),
                        ",".to_string(),
                        "⇒".to_string(),
                        "|".to_string(),
                    ]),
                    retrigger_characters: Some(vec![
                        "·".to_string(),
                        ".".to_string(),
                        ",".to_string(),
                        "⇒".to_string(),
                        "|".to_string(),
                    ]),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                }),
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        info!("Server initialized successfully");

        // Apply default configuration to all providers
        self.apply_configuration();

        // Scan workspace for Event-B files to populate cross-reference index
        if let Some(root) = self.cross_reference_manager.workspace_root() {
            match self.cross_reference_manager.scan_workspace(&root) {
                Ok(count) => {
                    info!("Indexed {} Event-B files from workspace", count);
                }
                Err(e) => {
                    info!("Failed to scan workspace: {}", e);
                }
            }
        }

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
                self.apply_configuration();

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
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        let version = params.text_document.version;

        debug!("Document opened: {}", uri);

        // Store the document; its parse is produced lazily on first read below.
        self.document_manager
            .open(uri.clone(), params.text_document.language_id, version, text);

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

        // Coalesce rapid edits behind the configured debounce window. A zero
        // window analyzes inline (the previous behaviour).
        let debounce_ms = self.config_manager.get().diagnostics.debounce_ms;
        if debounce_ms == 0 {
            self.analyzer.analyze(uri).await;
            return;
        }

        // Schedule the analysis after the window. Rather than tracking and
        // aborting prior tasks, each task checks at wake-up whether its edit is
        // still the document's latest version; a superseded (or closed) document
        // makes the task bow out, so only the final edit of a burst analyzes —
        // and it publishes for exactly the version it parsed.
        let analyzer = self.analyzer.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(debounce_ms as u64)).await;
            if analyzer.document_manager.version(&uri) == Some(version) {
                analyzer.analyze(uri).await;
            }
        });
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        debug!("Document closed: {}", uri);

        // Remove document from open-document tracking. Any debounced analysis
        // still pending for this URI finds `version(&uri) == None` at wake-up and
        // bows out, so it cannot publish diagnostics after this close clears them.
        self.document_manager.close(&uri);

        // Retain cross-reference data so that other open documents (machines/contexts)
        // can still resolve SEES/EXTENDS/REFINES references to this component.

        // Remove workspace symbols
        self.workspace_symbol_provider.remove_document(uri.as_ref());

        // Clear diagnostics
        self.client.publish_diagnostics(uri, vec![], None).await;
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
            self.analyzer.analyze(uri).await;
        }
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
            .flat_map(|component| analysis::extract_symbols(component, &doc.text))
            .collect();

        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        let uri = params.text_document.uri;
        debug!("Selection range request for: {}", uri);

        let text = match self.document_manager.get_text(&uri) {
            Some(text) => text,
            None => {
                debug!("Document not found: {}", uri);
                return Ok(None);
            }
        };

        let ranges = self
            .selection_range_provider
            .selection_ranges(&text, &params.positions);
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
        match self.formatting_provider.format(&text) {
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
        // manager — no per-request re-parse.
        let response = self.completion_provider.complete(&params, &text);

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
        // no per-request re-parse.
        let response = self.hover_provider.hover(&params, &text);

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

        // Get document text
        let text = match self.document_manager.get_text(uri) {
            Some(text) => text,
            None => {
                debug!("Document not found: {}", uri);
                return Ok(None);
            }
        };

        // Get signature help information
        let response = self.signature_help_provider.signature_help(&params, &text);

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

        // Get definition location
        let response = self.definition_provider.goto_definition(&params, &text);

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

        // Find all references
        let response = self.reference_provider.find_references(&params, &text);

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

        // Perform the rename
        let response = self.rename_provider.rename(&params, &text);

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
                .semantic_tokens(&params, &doc.text, doc.components());

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
        let response = self
            .code_actions_provider
            .provide_code_actions(&params, &text);

        debug!(
            "Code actions returned: {}",
            response.as_ref().map_or(0, |actions| actions.len())
        );

        Ok(response)
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
            .folding_ranges_from_components(doc.components(), &doc.text);

        debug!(
            "Folding ranges returned: {}",
            response.as_ref().map_or(0, |ranges| ranges.len())
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
        Ok(rossi::operators::operator_rows())
    }

    /// Apply current configuration to all providers
    fn apply_configuration(&self) {
        use crate::completion::CompletionConfig as LspCompletionConfig;
        use crate::formatting::FormattingConfig;

        let config = self.config_manager.get();

        // Apply formatting configuration
        let format_config = FormattingConfig {
            use_unicode: config.format.use_unicode,
            indentation: config.format.indentation.clone(),
        };
        self.formatting_provider.update_config(format_config);

        // Apply completion configuration
        let completion_config = LspCompletionConfig {
            enabled: config.completion.enabled,
            use_unicode: config.format.use_unicode,
            enable_snippets: true, // Always enable snippets for now
        };
        self.completion_provider.update_config(completion_config);

        info!("Configuration applied to all providers");
    }
}
