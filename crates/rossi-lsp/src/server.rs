//! LSP Server implementation

use crate::lsp_types::*;
use std::path::PathBuf;
use std::sync::Arc;
use tower_lsp::jsonrpc::Result;
use tower_lsp::{Client, LanguageServer};
use tracing::{debug, info};

use crate::analysis;
use crate::code_actions::CodeActionProvider;
use crate::completion::CompletionProvider;
use crate::config::{ConfigManager, RossiConfig};
use crate::cross_references::CrossReferenceManager;
use crate::definition::DefinitionProvider;
use crate::document::DocumentManager;
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

        Self {
            client,
            config_manager: Arc::new(ConfigManager::new()),
            document_manager,
            cross_reference_manager,
            formatting_provider: Arc::new(FormattingProvider::new()),
            completion_provider: Arc::new(completion_provider),
            hover_provider: Arc::new(hover_provider),
            definition_provider: Arc::new(definition_provider),
            reference_provider: Arc::new(reference_provider),
            rename_provider: Arc::new(rename_provider),
            workspace_symbol_provider: Arc::new(WorkspaceSymbolProvider::new()),
            semantic_tokens_provider: Arc::new(SemanticTokensProvider::new()),
            document_links_provider: Arc::new(document_links_provider),
            code_actions_provider: Arc::new(CodeActionProvider::new()),
            folding_range_provider: Arc::new(FoldingRangeProvider::new()),
            selection_range_provider: Arc::new(SelectionRangeProvider::new()),
            signature_help_provider: Arc::new(SignatureHelpProvider::new()),
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
                name: "rossi-language-server".to_string(),
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

        // Store document
        self.document_manager.open(
            uri.clone(),
            params.text_document.language_id,
            version,
            text.clone(),
        );

        // Update cross-reference manager
        self.cross_reference_manager
            .update_component(uri.to_string(), &text);

        // Update definition cache
        self.definition_provider
            .update_definitions(uri.to_string(), &text);

        // Update workspace symbols
        self.workspace_symbol_provider
            .update_symbols(uri.to_string(), &text);

        // Parse and publish diagnostics
        self.analyze_and_publish_diagnostics(uri, text, Some(version))
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        let changes = params.content_changes;

        debug!("Document changed: {} (version {})", uri, version);

        // Update document
        self.document_manager.change(&uri, version, changes);

        // Get updated text and publish diagnostics
        if let Some(text) = self.document_manager.get_text(&uri) {
            // Update cross-reference manager
            self.cross_reference_manager
                .update_component(uri.to_string(), &text);

            // Update definition cache
            self.definition_provider
                .update_definitions(uri.to_string(), &text);

            // Update workspace symbols
            self.workspace_symbol_provider
                .update_symbols(uri.to_string(), &text);

            self.analyze_and_publish_diagnostics(uri, text, Some(version))
                .await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        debug!("Document closed: {}", uri);

        // Remove document from open-document tracking
        self.document_manager.close(&uri);

        // Retain cross-reference data so that other open documents (machines/contexts)
        // can still resolve SEES/EXTENDS/REFINES references to this component.

        // Remove workspace symbols
        self.workspace_symbol_provider.remove_document(uri.as_ref());

        // Clear diagnostics
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        debug!("Document saved: {}", params.text_document.uri);
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri;
        debug!("Document symbol request for: {}", uri);

        // Get document text
        let text = match self.document_manager.get_text(&uri) {
            Some(text) => text,
            None => {
                debug!("Document not found: {}", uri);
                return Ok(None);
            }
        };

        // Parse the document; a multi-component file yields one root symbol
        // per component
        let components = match rossi::parse_components(&text) {
            Ok(comps) => comps,
            Err(e) => {
                debug!("Failed to parse document for symbols: {}", e);
                return Ok(None);
            }
        };

        // Extract symbols with source text for accurate span information
        let symbols = components
            .iter()
            .flat_map(|component| analysis::extract_symbols(component, &text))
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

        // Update component cache for this document
        self.completion_provider
            .update_component(uri.to_string(), &text);

        // Get completions
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

        // Update component cache for this document
        self.hover_provider.update_component(uri.to_string(), &text);

        // Get hover information
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

        // Get document text
        let text = match self.document_manager.get_text(uri) {
            Some(text) => text,
            None => {
                debug!("Document not found: {}", uri);
                return Ok(None);
            }
        };

        // Get semantic tokens
        let response = self
            .semantic_tokens_provider
            .semantic_tokens(&params, &text);

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

        // Get document text
        let text = match self.document_manager.get_text(uri) {
            Some(text) => text,
            None => {
                debug!("Document not found: {}", uri);
                return Ok(None);
            }
        };

        // Get folding ranges
        let response = self.folding_range_provider.folding_ranges(&params, &text);

        debug!(
            "Folding ranges returned: {}",
            response.as_ref().map_or(0, |ranges| ranges.len())
        );

        Ok(response)
    }
}

/// One operator spelling, as returned by the `rossi/operatorTable` custom
/// request. Only operators whose ASCII and Unicode spellings differ are
/// included. `symbolic` marks operators with no word characters (alphabetic
/// ops are leader-only); `eager` marks the subset an input method should
/// substitute as you type (see [`rossi::operators::OperatorSpelling::is_eager_input`]).
#[derive(Debug, Clone, serde::Serialize)]
pub struct OperatorRow {
    pub ascii: String,
    pub unicode: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub symbolic: bool,
    pub eager: bool,
}

/// Build the operator rows served by `rossi/operatorTable` from the
/// single-source table in `rossi::operators`. Only operators whose ASCII and
/// Unicode spellings differ are included (identical ones need no conversion).
pub fn operator_rows() -> Vec<OperatorRow> {
    rossi::operators::OPERATOR_SPELLINGS
        .iter()
        .filter(|entry| entry.ascii != entry.unicode)
        .map(|entry| OperatorRow {
            ascii: entry.ascii.to_string(),
            unicode: entry.unicode.to_string(),
            description: entry.description.to_string(),
            aliases: entry.aliases().iter().map(|a| a.to_string()).collect(),
            symbolic: entry.is_symbolic(),
            eager: entry.is_eager_input(),
        })
        .collect()
}

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
        Ok(operator_rows())
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

    /// Analyze a document and publish diagnostics
    async fn analyze_and_publish_diagnostics(&self, uri: Url, text: String, version: Option<i32>) {
        // Check if diagnostics are enabled
        let config = self.config_manager.get();
        if !config.diagnostics.enabled {
            // Clear diagnostics if disabled
            self.client.publish_diagnostics(uri, vec![], version).await;
            return;
        }

        let diagnostics = self.analyze_document(&text);
        self.client
            .publish_diagnostics(uri, diagnostics, version)
            .await;
    }

    /// Analyze a document and return diagnostics
    fn analyze_document(&self, text: &str) -> Vec<Diagnostic> {
        use rossi::parse_components_with_recovery;

        let parse_result = parse_components_with_recovery(text);

        // Convert parse errors to LSP diagnostics
        parse_result
            .errors
            .iter()
            .map(|e| parse_error_to_diagnostic(e, text))
            .collect()
    }
}

/// End byte offset of the token at byte offset `start`, for sizing a diagnostic
/// range when pest reports only a point: the end of the contiguous non-whitespace
/// run starting at `start`, bounded by the line. Zero-width at EOL/EOF, one char
/// when `start` lands on whitespace.
fn token_end_byte(text: &str, start: usize) -> usize {
    let rest = &text[start..];
    match rest.chars().next() {
        None | Some('\n') => start, // EOF / EOL: zero-width
        Some(first) if first.is_whitespace() => start + first.len_utf8(), // 1-char span
        // The leading non-whitespace run ends at the first whitespace (or EOL).
        _ => start + rest.find(char::is_whitespace).unwrap_or(rest.len()),
    }
}

/// Collapse pest's multi-line rendering (a location header, the source line, a
/// caret, then an `= expected …` line) to a single line: the editor already
/// shows the location via the diagnostic range, so only the `expected …`
/// content carries information.
fn concise_pest_message(message: &str) -> String {
    message
        .lines()
        .map(str::trim_start)
        .find_map(|l| l.strip_prefix("= "))
        .map(|expected| format!("Syntax error: {expected}"))
        .unwrap_or_else(|| message.trim().to_string()) // fallback: never drop info
}

/// Convert a parse error to an LSP diagnostic
fn parse_error_to_diagnostic(error: &rossi::ParseError, text: &str) -> Diagnostic {
    use rossi::ParseError;

    // pest's multi-line dump is collapsed to a single line; located variants
    // keep their own message; everything else uses the Display rendering.
    let message = match error {
        ParseError::PestError { message, .. } => concise_pest_message(message),
        ParseError::RecoverableError { message, .. } | ParseError::ClauseError { message, .. } => {
            message.clone()
        }
        _ => error.to_string(),
    };

    Diagnostic {
        range: parse_error_range(error, text),
        severity: Some(DiagnosticSeverity::ERROR),
        code: None,
        source: Some("rossi".to_string()),
        message,
        related_information: None,
        tags: None,
        code_description: None,
        data: None,
    }
}

/// LSP range for a parse-error diagnostic, rendered through the single UTF-16
/// converter (issue #48).
///
/// Everything resolves to a byte `[start, end)`: a non-empty span (issue #42)
/// underlines the offending token directly; a zero-width span (pest reports a
/// single point) or a span-less variant gives only a start, so the token is
/// sized in bytes from there. A span-less start comes from the 1-indexed
/// (line, column) — those variants (nesting, clause-order, recovery) point at
/// ASCII keywords/clause content, where char and UTF-16 columns coincide.
fn parse_error_range(error: &rossi::ParseError, text: &str) -> Range {
    let span = error.span();
    let start = match span {
        Some(s) => s.start,
        None => {
            let (line, column) = error.position().unwrap_or((1, 1));
            let pos = Position::new(
                line.saturating_sub(1) as u32,
                column.saturating_sub(1) as u32,
            );
            crate::position::position_to_offset(text, pos).unwrap_or(text.len())
        }
    };
    let end = match span {
        Some(s) if s.start < s.end => s.end,
        _ => token_end_byte(text, start),
    };
    crate::position::span_to_range(&rossi::ast::Span { start, end }, text)
}

#[cfg(test)]
mod tests {
    use super::{operator_rows, parse_error_to_diagnostic};
    use crate::lsp_types::Position;

    #[test]
    fn clause_order_diagnostic_stays_on_one_line() {
        // A misordered EXTENDS clause yields a ClauseError whose pest span
        // covers the whole multi-line clause; the diagnostic must NOT underline
        // all of it. With no span it falls back to a single-line, token-sized
        // range at the offending keyword.
        let text = "CONTEXT test\nSETS\n    S\nEXTENDS\n    other_ctx\nEND\n";
        let error = rossi::parse(text).expect_err("EXTENDS after SETS must fail");
        let diagnostic = parse_error_to_diagnostic(&error, text);
        assert_eq!(
            diagnostic.range.start.line, diagnostic.range.end.line,
            "clause-order diagnostic must stay on one line, got {:?}",
            diagnostic.range
        );
        // Sized to the `EXTENDS` keyword on line 4 (0-indexed 3), not the body.
        assert_eq!(diagnostic.range.start, Position::new(3, 0));
        assert_eq!(diagnostic.range.end, Position::new(3, 7));
    }

    #[test]
    fn reserved_word_diagnostic_spans_the_word_issue_42() {
        // The reserved word `dom` used as a constant name carries a byte span
        // (issue #42); the diagnostic range comes from that span and covers the
        // whole 3-char word, not the old byte-length special case.
        let text = "CONTEXT c0\nCONSTANTS\n    dom\nEND\n";
        let error = rossi::parse(text).expect_err("`dom` is a reserved word");
        let diagnostic = parse_error_to_diagnostic(&error, text);
        assert_eq!(diagnostic.range.start, Position::new(2, 4));
        assert_eq!(diagnostic.range.end, Position::new(2, 7));
    }

    #[test]
    fn pest_diagnostic_uses_real_position() {
        // End-to-end through the real parser: the strict-parse error must
        // carry pest's structured position, not 0:0, and the range must be
        // sized to the offending token (the stray `+`), not a fixed width.
        let text = "CONTEXT c\nCONSTANTS\n    c1\n    +\nEND\n";
        let error = rossi::parse(text).expect_err("the stray `+` must fail strict parsing");
        let diagnostic = parse_error_to_diagnostic(&error, text);
        assert_eq!(diagnostic.range.start, Position::new(3, 4));
        // Token span: just the single-character `+`, not start + 10.
        assert_eq!(diagnostic.range.end, Position::new(3, 5));
        // Message is collapsed to a single line (issue #32): no pest caret art.
        assert!(diagnostic.message.starts_with("Syntax error:"));
        assert!(!diagnostic.message.contains("-->"));
        assert!(!diagnostic.message.contains('\n'));
    }

    #[test]
    fn pest_diagnostic_sized_to_token_issue_32() {
        // Issue #32, example 1: a forgotten `@` on `axm2`. Through the real
        // LSP recovery path, the diagnostic must land on the offending line
        // (line 10) and underline just the token pest stopped at (`>`), rather
        // than a fixed 10-character block running past the end of the line.
        let text = concat!(
            "CONTEXT library_ctx\n",
            "EXTENDS\n",
            "    base_ctx\n",
            "SETS\n",
            "    BOOK, READER\n",
            "CONSTANTS\n",
            "    max_loans\n",
            "AXIOMS\n",
            "    @axm1: max_loans = 5\n",
            "    axm2: max_loans > 0\n",
            "END\n",
        );
        let result = rossi::parse_components_with_recovery(text);
        let error = result
            .errors
            .first()
            .expect("recovery must report the error");
        let diagnostic = parse_error_to_diagnostic(error, text);
        // Line 10 (0-indexed 9), the `>` at column 21 (0-indexed 20).
        assert_eq!(diagnostic.range.start, Position::new(9, 20));
        assert_eq!(diagnostic.range.end, Position::new(9, 21));
        assert!(!diagnostic.message.contains("-->"));
    }

    #[test]
    fn concise_pest_message_keeps_add_missing_end_trigger() {
        // The "Add missing END" quick fix (code_actions.rs) keys off
        // `message.contains("expected")`. Shortening pest's dump must not drop
        // that word, or the quick fix silently stops being offered.
        let text = "MACHINE m\nVARIABLES\n    x\n"; // no END
        let result = rossi::parse_components_with_recovery(text);
        let error = result.errors.first().expect("missing END must be reported");
        let diagnostic = parse_error_to_diagnostic(error, text);
        assert!(
            diagnostic.message.contains("expected"),
            "concise message must retain the quick-fix trigger, got: {}",
            diagnostic.message
        );
    }

    #[test]
    fn operator_rows_are_well_formed() {
        let rows = operator_rows();
        assert!(!rows.is_empty(), "operator table must not be empty");

        // Every row differs (ascii != unicode) and has non-empty spellings.
        for row in &rows {
            assert_ne!(row.ascii, row.unicode);
            assert!(!row.ascii.is_empty() && !row.unicode.is_empty());
        }

        // Representative symbolic op carries aliases and is eager-eligible.
        let implies = rows
            .iter()
            .find(|r| r.ascii == "=>")
            .expect("`=>` should be present");
        assert_eq!(implies.unicode, "⇒");
        assert!(implies.symbolic);
        assert!(implies.eager);
        assert!(implies.aliases.iter().any(|a| a == "implies"));

        // Alphabetic op is leader-only (symbolic and eager both false).
        let nat = rows
            .iter()
            .find(|r| r.ascii == "NAT")
            .expect("`NAT` should be present");
        assert!(!nat.symbolic);
        assert!(!nat.eager);

        // A bare `/` is symbolic but blocklisted from eager (`//` comments).
        let divide = rows
            .iter()
            .find(|r| r.ascii == "/")
            .expect("`/` should be present");
        assert!(divide.symbolic);
        assert!(!divide.eager);

        // Serializes to a flat JSON array the extension can consume.
        let json = serde_json::to_value(&rows).expect("serializes");
        assert!(json.is_array());
    }
}
