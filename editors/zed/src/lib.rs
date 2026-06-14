//! Zed extension for Event-B (Rossi).
//!
//! Zed needs Rust only to launch the language server; syntax highlighting comes
//! from the standalone `tree-sitter-eventb` grammar (pinned in `extension.toml`,
//! developed in this monorepo under `editors/tree-sitter-eventb/`) and its
//! `languages/eventb/highlights.scm`, and everything else (diagnostics,
//! completion, hover, goto, rename, formatting, code actions, outline,
//! folding) comes from `rossi-language-server` over LSP.
//!
//! The server binary is resolved from the user's Zed LSP settings
//! (`lsp.rossi-language-server.binary.path`) if set, otherwise from `PATH`. The
//! project publishes no prebuilt binaries, so there is intentionally no
//! GitHub-release download path — users install it with
//! `cargo install --path crates/rossi-lsp`.

use zed_extension_api::{self as zed, LanguageServerId, Result};

const SERVER_BINARY: &str = "rossi-language-server";

struct RossiExtension;

impl zed::Extension for RossiExtension {
    fn new() -> Self {
        RossiExtension
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        // Pull the user's explicit binary overrides (path/arguments/env) from the
        // LSP settings once; each falls back independently below.
        let (path, arguments, custom_env) =
            zed::settings::LspSettings::for_worktree(language_server_id.as_ref(), worktree)
                .ok()
                .and_then(|settings| settings.binary)
                .map(|binary| (binary.path, binary.arguments, binary.env))
                .unwrap_or_default();

        // Honor the configured path, else fall back to `rossi-language-server` on PATH.
        let command = path
            .or_else(|| worktree.which(SERVER_BINARY))
            .ok_or_else(|| {
                format!(
                    "`{SERVER_BINARY}` not found on PATH. Install it with \
                     `cargo install --path crates/rossi-lsp`, or set \
                     `lsp.{SERVER_BINARY}.binary.path` in your Zed settings."
                )
            })?;

        // Inherit the worktree's shell environment, then layer any vars the user
        // set under `lsp.rossi-language-server.binary.env` on top (e.g. RUST_LOG)
        // so they reach the server process.
        let mut env = worktree.shell_env();
        if let Some(custom_env) = custom_env {
            env.extend(custom_env);
        }

        Ok(zed::Command {
            command,
            args: arguments.unwrap_or_default(),
            env,
        })
    }

    fn language_server_initialization_options(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        // Forward `lsp.rossi-language-server.initialization_options` to the
        // server's `initialize` request. rossi-language-server applies config from
        // initializationOptions at startup (as well as from didChangeConfiguration,
        // which Zed feeds from `language_server_workspace_configuration` below), so
        // this is the channel for options that must be set before the first reply.
        let options =
            zed::settings::LspSettings::for_worktree(language_server_id.as_ref(), worktree)
                .ok()
                .and_then(|settings| settings.initialization_options);
        Ok(options)
    }

    fn language_server_workspace_configuration(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        // Pass the user's `lsp.rossi-language-server.settings` through unchanged;
        // the server reads its options from the `rossi` section (see the README
        // for the shape, mirroring the Neovim lspconfig defaults).
        let settings =
            zed::settings::LspSettings::for_worktree(language_server_id.as_ref(), worktree)
                .ok()
                .and_then(|settings| settings.settings);
        Ok(settings)
    }
}

zed::register_extension!(RossiExtension);
