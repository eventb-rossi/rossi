//! LSP Server implementation

use lsp_types::*;
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
use crate::prob::ProBProvider;
use crate::references::ReferenceProvider;
use crate::rename::RenameProvider;
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
    /// Signature help provider
    signature_help_provider: Arc<SignatureHelpProvider>,
    /// ProB integration provider
    prob_provider: Arc<ProBProvider>,
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
            signature_help_provider: Arc::new(SignatureHelpProvider::new()),
            prob_provider: Arc::new(ProBProvider::new()),
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
                code_lens_provider: Some(CodeLensOptions {
                    resolve_provider: Some(false),
                }),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec![
                        "rossi.prob.animate".to_string(),
                        "rossi.prob.modelcheck".to_string(),
                    ],
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

        // Parse the document
        let component = match rossi::parse(&text) {
            Ok(comp) => comp,
            Err(e) => {
                debug!("Failed to parse document for symbols: {}", e);
                return Ok(None);
            }
        };

        // Extract symbols with source text for accurate span information
        let symbols = analysis::extract_symbols(&component, &text);

        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
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

    async fn code_lens(&self, params: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        let uri = &params.text_document.uri;
        debug!("Code lens request for: {}", uri);

        // Get document text
        let text = match self.document_manager.get_text(uri) {
            Some(text) => text,
            None => {
                debug!("Document not found: {}", uri);
                return Ok(None);
            }
        };

        // Get code lenses from ProB provider
        let lenses = self.prob_provider.provide_code_lenses(&text, uri);

        debug!("Code lenses returned: {}", lenses.len());

        Ok(Some(lenses))
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        debug!("Execute command request: {}", params.command);

        match params.command.as_str() {
            "rossi.prob.animate" => self.execute_prob_animate(&params).await,
            "rossi.prob.modelcheck" => self.execute_prob_modelcheck(&params).await,
            _ => {
                debug!("Unknown command: {}", params.command);
                Ok(None)
            }
        }
    }
}

impl RossiLanguageServer {
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

        // Apply ProB configuration
        self.prob_provider.update_config(config.prob.clone());

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
        use rossi::parse_with_recovery;

        let parse_result = parse_with_recovery(text);

        // Convert parse errors to LSP diagnostics
        parse_result
            .errors
            .iter()
            .map(|error| self.parse_error_to_diagnostic(error))
            .collect()
    }

    /// Convert a parse error to an LSP diagnostic
    fn parse_error_to_diagnostic(&self, error: &rossi::ParseError) -> Diagnostic {
        use rossi::ParseError;

        match error {
            ParseError::RecoverableError {
                line,
                column,
                message,
                ..
            } => Diagnostic {
                range: Range {
                    start: Position::new(
                        line.saturating_sub(1) as u32,
                        column.saturating_sub(1) as u32,
                    ),
                    end: Position::new(
                        line.saturating_sub(1) as u32,
                        column.saturating_sub(1) as u32 + 10,
                    ),
                },
                severity: Some(DiagnosticSeverity::ERROR),
                code: None,
                source: Some("rossi".to_string()),
                message: message.clone(),
                related_information: None,
                tags: None,
                code_description: None,
                data: None,
            },
            ParseError::ClauseError {
                line,
                column,
                message,
                ..
            } => Diagnostic {
                range: Range {
                    start: Position::new(
                        line.saturating_sub(1) as u32,
                        column.saturating_sub(1) as u32,
                    ),
                    end: Position::new(
                        line.saturating_sub(1) as u32,
                        column.saturating_sub(1) as u32 + 10,
                    ),
                },
                severity: Some(DiagnosticSeverity::ERROR),
                code: None,
                source: Some("rossi".to_string()),
                message: message.clone(),
                related_information: None,
                tags: None,
                code_description: None,
                data: None,
            },
            _ => Diagnostic {
                range: Range {
                    start: Position::new(0, 0),
                    end: Position::new(0, 10),
                },
                severity: Some(DiagnosticSeverity::ERROR),
                code: None,
                source: Some("rossi".to_string()),
                message: error.to_string(),
                related_information: None,
                tags: None,
                code_description: None,
                data: None,
            },
        }
    }

    /// Execute ProB animation command
    async fn execute_prob_animate(
        &self,
        params: &ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        use tracing::error;

        // Check if ProB is available
        if !self.prob_provider.is_available() {
            self.client
                .show_message(
                    MessageType::ERROR,
                    "ProB (probcli) is not installed or not found in PATH. Please install ProB to use this feature.",
                )
                .await;
            return Ok(None);
        }

        // Extract file URI from arguments
        let uri = match params.arguments.first() {
            Some(serde_json::Value::String(uri_str)) => match Url::parse(uri_str) {
                Ok(uri) => uri,
                Err(e) => {
                    error!("Failed to parse URI: {}", e);
                    self.client
                        .show_message(MessageType::ERROR, format!("Invalid URI: {}", e))
                        .await;
                    return Ok(None);
                }
            },
            _ => {
                error!("No URI provided in command arguments");
                self.client
                    .show_message(MessageType::ERROR, "No file URI provided")
                    .await;
                return Ok(None);
            }
        };

        // Convert URI to file path
        let file_path = match uri.to_file_path() {
            Ok(path) => path,
            Err(_) => {
                error!("Failed to convert URI to file path");
                self.client
                    .show_message(MessageType::ERROR, "Invalid file path")
                    .await;
                return Ok(None);
            }
        };

        info!("Executing ProB animation for: {:?}", file_path);

        // Show progress message
        self.client
            .show_message(
                MessageType::INFO,
                format!("Launching ProB animator for {}", file_path.display()),
            )
            .await;

        // Execute ProB animation
        match self.prob_provider.animate(&file_path) {
            Ok(result) => {
                self.client
                    .show_message(
                        MessageType::INFO,
                        format!(
                            "ProB animation completed. Output: {}",
                            result.stdout.lines().take(3).collect::<Vec<_>>().join(" ")
                        ),
                    )
                    .await;
                Ok(Some(serde_json::json!({ "success": true })))
            }
            Err(e) => {
                error!("ProB animation failed: {}", e);
                self.client
                    .show_message(MessageType::ERROR, format!("ProB animation failed: {}", e))
                    .await;
                Ok(Some(
                    serde_json::json!({ "success": false, "error": e.to_string() }),
                ))
            }
        }
    }

    /// Execute ProB model checking command
    async fn execute_prob_modelcheck(
        &self,
        params: &ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        use tracing::error;

        // Check if ProB is available
        if !self.prob_provider.is_available() {
            self.client
                .show_message(
                    MessageType::ERROR,
                    "ProB (probcli) is not installed or not found in PATH. Please install ProB to use this feature.",
                )
                .await;
            return Ok(None);
        }

        // Extract file URI from arguments
        let uri = match params.arguments.first() {
            Some(serde_json::Value::String(uri_str)) => match Url::parse(uri_str) {
                Ok(uri) => uri,
                Err(e) => {
                    error!("Failed to parse URI: {}", e);
                    self.client
                        .show_message(MessageType::ERROR, format!("Invalid URI: {}", e))
                        .await;
                    return Ok(None);
                }
            },
            _ => {
                error!("No URI provided in command arguments");
                self.client
                    .show_message(MessageType::ERROR, "No file URI provided")
                    .await;
                return Ok(None);
            }
        };

        // Convert URI to file path
        let file_path = match uri.to_file_path() {
            Ok(path) => path,
            Err(_) => {
                error!("Failed to convert URI to file path");
                self.client
                    .show_message(MessageType::ERROR, "Invalid file path")
                    .await;
                return Ok(None);
            }
        };

        info!("Executing ProB model checking for: {:?}", file_path);

        // Show progress message
        self.client
            .show_message(
                MessageType::INFO,
                format!("Running ProB model checker on {}", file_path.display()),
            )
            .await;

        // Execute ProB model checking
        match self.prob_provider.modelcheck(&file_path) {
            Ok(result) => {
                if result.success {
                    self.client
                        .show_message(
                            MessageType::INFO,
                            "ProB model checking completed successfully. No errors found.",
                        )
                        .await;
                } else {
                    let msg = if result.counterexamples.is_empty() {
                        "ProB model checking completed with warnings.".to_string()
                    } else {
                        format!(
                            "ProB found {} issue(s). Check diagnostics for details.",
                            result.counterexamples.len()
                        )
                    };
                    self.client.show_message(MessageType::WARNING, msg).await;

                    // Publish diagnostics for counterexamples
                    let text = self.document_manager.get_text(&uri).unwrap_or_default();
                    let diagnostics = self.prob_provider.results_to_diagnostics(&result, &text);
                    self.client
                        .publish_diagnostics(uri.clone(), diagnostics, None)
                        .await;
                }
                Ok(Some(serde_json::json!({
                    "success": result.success,
                    "counterexamples": result.counterexamples.len()
                })))
            }
            Err(e) => {
                error!("ProB model checking failed: {}", e);
                self.client
                    .show_message(
                        MessageType::ERROR,
                        format!("ProB model checking failed: {}", e),
                    )
                    .await;
                Ok(Some(
                    serde_json::json!({ "success": false, "error": e.to_string() }),
                ))
            }
        }
    }
}
