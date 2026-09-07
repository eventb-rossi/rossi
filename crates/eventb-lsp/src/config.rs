//! Configuration management for Rossi LSP Server
//!
//! This module handles:
//! - Reading configuration from the LSP client
//! - Listening for configuration changes
//! - Distributing configuration to all providers

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// Complete Rossi LSP server configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RossiConfig {
    /// Formatting configuration
    #[serde(default)]
    pub format: FormatConfig,

    /// Diagnostics configuration
    #[serde(default)]
    pub diagnostics: DiagnosticsConfig,

    /// Completion configuration
    #[serde(default)]
    pub completion: CompletionConfig,

    /// Rodin integration configuration
    #[serde(default)]
    pub rodin: RodinConfig,

    /// eventb-animate integration configuration
    #[serde(default)]
    pub animate: AnimateConfig,

    /// Inlay hints configuration
    #[serde(default)]
    pub inlay_hints: InlayHintsConfig,
}

impl RossiConfig {
    /// Parse configuration supplied by an LSP client.
    ///
    /// Some clients send the configured section directly (`{"format": ...}`),
    /// while others send the full settings object (`{"rossi": {"format": ...}}`).
    pub fn from_client_settings(settings: &Value) -> Result<Self, serde_json::Error> {
        match settings.get("rossi") {
            Some(rossi_settings) => serde_json::from_value(rossi_settings.clone()),
            None => serde_json::from_value(settings.clone()),
        }
    }
}

/// Formatting configuration.
///
/// The style fields are deliberately tolerant strings/options rather than
/// strict enums: `RossiConfig::from_client_settings` is all-or-nothing, so
/// a typo in one setting must fall back to the preset default instead of
/// discarding the user's whole configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatConfig {
    /// Formatting style preset: "camille" or "rossi". Empty or unknown
    /// selects the default preset.
    #[serde(default)]
    pub style: String,

    /// Use Unicode operators (∧, ∨, ⇒) instead of ASCII (/\, \/, =>)
    #[serde(default = "default_use_unicode")]
    pub use_unicode: bool,

    /// Flag ASCII operator spellings with an advisory diagnostic, for a
    /// project that keeps its sources in Unicode (`rossi fmt --check` being
    /// its CI gate). Off by default: ASCII is accepted input everywhere.
    /// Has no effect unless `use_unicode` is on.
    #[serde(default)]
    pub enforce_unicode: bool,

    /// Emit Rodin's private-use glyphs (U+E100..E103) for `<<->`, `<->>`,
    /// `<<->>` and `<+` rather than their ASCII spelling, for a project that
    /// exchanges sources with a tool reading only Rodin's spelling. Off by
    /// default: those glyphs need Rodin's Brave Sans Mono font to render. Has
    /// no effect unless `use_unicode` is on.
    #[serde(default)]
    pub private_use_glyphs: bool,

    /// Indentation string (e.g., "  " or "    "); empty follows the style
    /// preset (2 spaces camille, 4 spaces rossi)
    #[serde(default)]
    pub indentation: String,

    /// Keyword-case override: "lower" or "upper"; empty follows the preset
    #[serde(default)]
    pub keyword_case: String,

    /// Declaration-list layout override: "inline" or "one-per-line";
    /// empty follows the preset
    #[serde(default)]
    pub decl_lists: String,

    /// Blank line between top-level clauses; unset follows the preset
    #[serde(default, deserialize_with = "tolerant_blank_between_clauses")]
    pub blank_between_clauses: Option<bool>,

    /// Maximum line width when formatting, in characters (a tab counts
    /// as one); long formulas wrap onto operator-leading continuation
    /// lines. 0 disables wrapping
    #[serde(
        default = "default_max_line_width",
        deserialize_with = "tolerant_max_line_width"
    )]
    pub max_line_width: u32,
}

impl FormatConfig {
    /// Whether ASCII operator spellings get the advisory diagnostic: opted in
    /// through `enforce_unicode`, and only under the Unicode convention —
    /// with `use_unicode` off, the fix-all action and the formatter would
    /// rewrite back to ASCII whatever a quick fix converted.
    pub fn flags_ascii_operators(&self) -> bool {
        self.enforce_unicode && self.use_unicode
    }

    /// Whether emitted text spells the four relation/override operators with
    /// Rodin's private-use glyphs: opted in through `private_use_glyphs`, and
    /// only under the Unicode convention — with `use_unicode` off the whole
    /// document is spelled in ASCII, so there is no Unicode spelling to pick.
    /// The one source of truth for the mode, so the formatter, the conversion
    /// actions, completion, hover and the input method cannot disagree.
    pub fn emits_private_use_glyphs(&self) -> bool {
        self.private_use_glyphs && self.use_unicode
    }

    /// The pretty-printer this configuration denotes — the single mapping
    /// used everywhere the server renders Event-B text into a user's file
    /// (`textDocument/formatting` and the Rodin model-edit sync), so the two
    /// can never format the same file differently. Built through
    /// `PrettyPrinter::resolved`, the same preset + override resolution the
    /// CLI uses; editor output stays portable unless `private_use_glyphs`
    /// asks for Rodin's spelling.
    pub fn printer(&self) -> rossi::PrettyPrinter {
        let style = match self.style.to_ascii_lowercase().as_str() {
            "camille" => rossi::Style::Camille,
            "rossi" => rossi::Style::Rossi,
            _ => rossi::Style::default(),
        };
        let keyword_case = match self.keyword_case.to_ascii_lowercase().as_str() {
            "lower" => Some(rossi::KeywordCase::Lower),
            "upper" => Some(rossi::KeywordCase::Upper),
            _ => None,
        };
        let decl_lists = match self.decl_lists.to_ascii_lowercase().as_str() {
            "inline" => Some(rossi::DeclListLayout::Inline),
            "one-per-line" => Some(rossi::DeclListLayout::OnePerLine),
            _ => None,
        };
        rossi::PrettyPrinter::resolved(
            style,
            &rossi::StyleOverrides {
                keyword_case,
                decl_lists,
                blank_between_clauses: self.blank_between_clauses,
                // An empty indentation follows the preset's indent (the
                // clients' "unset" spelling); `Some` is always an override.
                indent: (!self.indentation.is_empty()).then(|| self.indentation.clone()),
                use_unicode: self.use_unicode,
                private_use_glyphs: self.emits_private_use_glyphs(),
                max_line_width: self.max_line_width as usize,
            },
        )
    }
}

impl Default for FormatConfig {
    fn default() -> Self {
        Self {
            style: String::new(),
            use_unicode: default_use_unicode(),
            enforce_unicode: false,
            private_use_glyphs: false,
            indentation: String::new(),
            keyword_case: String::new(),
            decl_lists: String::new(),
            blank_between_clauses: None,
            max_line_width: default_max_line_width(),
        }
    }
}

fn default_use_unicode() -> bool {
    true
}

fn default_max_line_width() -> u32 {
    rossi::DEFAULT_MAX_LINE_WIDTH as u32
}

/// Tolerant `u32` deserialization: like the string style fields, an
/// out-of-range or mistyped value (negative, fractional, string — some
/// clients enforce no schema) falls back to `default` instead of failing
/// the all-or-nothing `from_client_settings` parse.
fn tolerant_u32<'de, D>(deserializer: D, default: u32) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(value
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .unwrap_or(default))
}

fn tolerant_max_line_width<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    tolerant_u32(deserializer, default_max_line_width())
}

/// Tolerant deserializer for `blankBetweenClauses`: any non-boolean value
/// follows the preset instead of discarding the whole configuration.
fn tolerant_blank_between_clauses<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(value.as_bool())
}

/// Diagnostics configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsConfig {
    /// Enable diagnostics
    #[serde(default = "default_diagnostics_enabled")]
    pub enabled: bool,

    /// Delay in milliseconds the server waits after the last edit before it
    /// reparses, refreshes the indexes, and republishes diagnostics. Coalesces
    /// rapid keystrokes into a single analysis. `0` analyzes on every edit.
    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u32,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            enabled: default_diagnostics_enabled(),
            debounce_ms: default_debounce_ms(),
        }
    }
}

fn default_diagnostics_enabled() -> bool {
    true
}

fn default_debounce_ms() -> u32 {
    500
}

/// Completion configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionConfig {
    /// Enable completion
    #[serde(default = "default_completion_enabled")]
    pub enabled: bool,
}

impl Default for CompletionConfig {
    fn default() -> Self {
        Self {
            enabled: default_completion_enabled(),
        }
    }
}

fn default_completion_enabled() -> bool {
    true
}

/// Rodin integration configuration ("Open in Rodin" code lens)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RodinConfig {
    /// Rodin executable path, macOS `.app` bundle path, or macOS application
    /// name. Empty selects the platform default (`/Applications/Rodin.app`,
    /// `rodin.exe`, or `rodin`).
    #[serde(default)]
    pub path: String,

    /// Directory used as the shared Rodin workspace. Empty selects
    /// `<workspace-root>/.rossi/rodin`.
    #[serde(default)]
    pub workspace: String,

    /// Mutual live synchronization with a running Rodin. While Rodin is
    /// open on the shared workspace, saving a source file rebuilds its
    /// project (Rodin's seeded auto-refresh then picks the edit up within a
    /// few seconds), and edits saved in Rodin flow back into the `.eventb`
    /// sources via the workspace watcher (three-way model merge, proof
    /// status refresh). Model edits Rodin saved while no server was running
    /// are caught up when the watcher starts. On by default; turning it off
    /// also stops the watcher.
    #[serde(default = "default_sync")]
    pub sync: bool,

    /// Bridge proof files (`.bpr`/`.bps`/`.bpo`) between the checkout and
    /// the shared workspace at "Open in Rodin" session boundaries: the
    /// files sitting next to the sources are copied into the Rodin project
    /// when the lens runs (the checkout wins; nothing in the workspace is
    /// deleted), and the project's files are copied back next to the
    /// sources when the launched Rodin exits (the workspace wins; a proof
    /// deleted in Rodin is deleted next to the sources too). Captured when
    /// the lens runs — flipping it mid-session does not stop an armed
    /// mirror. The exit mirror needs the Eclipse workspace lock probe and
    /// is unavailable on Windows. On by default.
    #[serde(default = "default_mirror_proofs")]
    pub mirror_proofs: bool,

    /// Use the Rodin bridge plug-in when the running Rodin publishes one.
    /// It can do inside the live instance what writing files at it cannot:
    /// registering a project while Rodin holds the workspace, and bringing
    /// the project forward. With no plug-in (stock Rodin) this setting
    /// changes nothing: the file-mediated path runs either way. On by
    /// default; turn it off to keep to that path even where a bridge exists.
    #[serde(default = "default_bridge")]
    pub bridge: bool,
}

impl Default for RodinConfig {
    fn default() -> Self {
        Self {
            path: String::new(),
            workspace: String::new(),
            sync: default_sync(),
            mirror_proofs: default_mirror_proofs(),
            bridge: default_bridge(),
        }
    }
}

fn default_sync() -> bool {
    true
}

fn default_bridge() -> bool {
    true
}

fn default_mirror_proofs() -> bool {
    true
}

/// eventb-animate integration configuration ("Model-check" / "Disprove POs"
/// code lenses)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimateConfig {
    /// eventb-animate executable path or bare command name. Empty resolves
    /// `eventb-animate` via PATH when a lens runs.
    #[serde(default)]
    pub path: String,

    /// `--time-limit` (seconds) for the model-check lens; the watchdog that
    /// kills a hung tool derives from it. `0` selects the default.
    #[serde(default = "default_time_limit_secs")]
    pub time_limit_secs: u32,

    /// `--disprove-timeout` (milliseconds) per proof obligation for the
    /// Disprove POs lens; also feeds that lens's watchdog. `0` selects the
    /// default.
    #[serde(default = "default_disprove_timeout_ms")]
    pub disprove_timeout_ms: u32,
}

impl AnimateConfig {
    /// The effective `--time-limit`, with `0` mapped back to the default.
    pub fn effective_time_limit_secs(&self) -> u32 {
        if self.time_limit_secs == 0 {
            default_time_limit_secs()
        } else {
            self.time_limit_secs
        }
    }

    /// The effective `--disprove-timeout`, with `0` mapped back to the
    /// default.
    pub fn effective_disprove_timeout_ms(&self) -> u32 {
        if self.disprove_timeout_ms == 0 {
            default_disprove_timeout_ms()
        } else {
            self.disprove_timeout_ms
        }
    }
}

impl Default for AnimateConfig {
    fn default() -> Self {
        Self {
            path: String::new(),
            time_limit_secs: default_time_limit_secs(),
            disprove_timeout_ms: default_disprove_timeout_ms(),
        }
    }
}

fn default_time_limit_secs() -> u32 {
    120
}

fn default_disprove_timeout_ms() -> u32 {
    1000
}

/// Inlay hints configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InlayHintsConfig {
    /// Enable inlay hints (inferred declaration types)
    #[serde(default = "default_inlay_hints_enabled")]
    pub enabled: bool,

    /// Mark formulas carrying a non-trivial well-definedness condition with
    /// a "WD" hint whose tooltip shows the condition
    #[serde(default = "default_inlay_hints_well_definedness")]
    pub well_definedness: bool,

    /// Maximum rendered length of a type label in characters; longer labels
    /// are truncated with '…' and carry the full type as their tooltip.
    /// `0` disables truncation
    #[serde(
        default = "default_inlay_hints_max_length",
        deserialize_with = "tolerant_inlay_hints_max_length"
    )]
    pub max_length: u32,
}

impl Default for InlayHintsConfig {
    fn default() -> Self {
        Self {
            enabled: default_inlay_hints_enabled(),
            well_definedness: default_inlay_hints_well_definedness(),
            max_length: default_inlay_hints_max_length(),
        }
    }
}

fn default_inlay_hints_enabled() -> bool {
    true
}

fn default_inlay_hints_well_definedness() -> bool {
    true
}

fn default_inlay_hints_max_length() -> u32 {
    32
}

fn tolerant_inlay_hints_max_length<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    tolerant_u32(deserializer, default_inlay_hints_max_length())
}

/// Configuration manager that holds the current configuration
pub struct ConfigManager {
    config: RwLock<Arc<RossiConfig>>,
}

impl ConfigManager {
    /// Create a new configuration manager with default settings
    pub fn new() -> Self {
        Self {
            config: RwLock::new(Arc::new(RossiConfig::default())),
        }
    }

    /// Get a cheap snapshot of the current configuration.
    pub fn get(&self) -> Arc<RossiConfig> {
        Arc::clone(&self.config.read())
    }

    /// Update the entire configuration
    pub fn update(&self, config: RossiConfig) {
        *self.config.write() = Arc::new(config);
    }
}

impl Default for ConfigManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RossiConfig::default();

        assert!(config.format.use_unicode);
        // Empty follows the style preset.
        assert_eq!(config.format.style, "");
        assert_eq!(config.format.indentation, "");

        assert!(config.diagnostics.enabled);
        assert_eq!(config.diagnostics.debounce_ms, 500);

        assert!(config.completion.enabled);

        assert!(config.rodin.sync);
        assert!(config.rodin.mirror_proofs);

        assert!(config.inlay_hints.enabled);
        assert!(config.inlay_hints.well_definedness);
        assert_eq!(config.inlay_hints.max_length, 32);
    }

    #[test]
    fn test_inlay_hints_settings_parse_nested() {
        let settings = serde_json::json!({
            "rossi": {
                "inlayHints": {
                    "enabled": false,
                    "wellDefinedness": false,
                    "maxLength": 0
                }
            }
        });
        let config = RossiConfig::from_client_settings(&settings).unwrap();
        assert!(!config.inlay_hints.enabled);
        assert!(!config.inlay_hints.well_definedness);
        assert_eq!(config.inlay_hints.max_length, 0);
    }

    #[test]
    fn test_inlay_hints_invalid_max_length_keeps_whole_config() {
        // Same all-or-nothing rationale as `maxLineWidth`: a mistyped value
        // falls back to the default and the sibling settings survive.
        for bad_length in [
            serde_json::json!(-1),
            serde_json::json!(32.5),
            serde_json::json!("32"),
        ] {
            let settings = serde_json::json!({
                "rossi": {
                    "inlayHints": { "maxLength": bad_length, "enabled": false }
                }
            });
            let config = RossiConfig::from_client_settings(&settings)
                .unwrap_or_else(|e| panic!("config discarded for {bad_length}: {e}"));
            assert_eq!(config.inlay_hints.max_length, 32, "for {bad_length}");
            assert!(!config.inlay_hints.enabled);
        }
    }

    #[test]
    fn test_config_manager_get_set() {
        let manager = ConfigManager::new();

        // Check defaults
        let config = manager.get();
        assert!(config.format.use_unicode);

        // Update configuration
        let mut new_config = (*config).clone();
        new_config.format.use_unicode = false;
        new_config.format.indentation = "  ".to_string();
        manager.update(new_config);

        // Check updated values
        let updated = manager.get();
        assert!(!updated.format.use_unicode);
        assert_eq!(updated.format.indentation, "  ");
    }

    #[test]
    fn test_json_serialization() {
        let config = RossiConfig::default();

        // Serialize to JSON
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("format"));
        assert!(json.contains("useUnicode"));
        assert!(json.contains("privateUseGlyphs"));

        // Deserialize from JSON
        let deserialized: RossiConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config.format.use_unicode, deserialized.format.use_unicode);
        assert_eq!(
            config.format.private_use_glyphs,
            deserialized.format.private_use_glyphs
        );
    }

    /// `privateUseGlyphs` picks a Unicode spelling, so it means nothing under
    /// the ASCII convention — the same shape as `enforceUnicode`.
    #[test]
    fn private_use_glyphs_needs_the_unicode_convention() {
        let mut format = FormatConfig::default();
        assert!(!format.emits_private_use_glyphs());

        format.private_use_glyphs = true;
        assert!(format.emits_private_use_glyphs());
        assert!(format.printer().private_use_glyphs);

        format.use_unicode = false;
        assert!(!format.emits_private_use_glyphs());
        assert!(!format.printer().private_use_glyphs);
    }

    #[test]
    fn test_json_with_custom_values() {
        let json = r#"{
            "format": {
                "useUnicode": false,
                "enforceUnicode": true,
                "privateUseGlyphs": true,
                "indentation": "  "
            },
            "diagnostics": {
                "enabled": false,
                "debounceMs": 1000
            },
            "completion": {
                "enabled": true
            }
        }"#;

        let config: RossiConfig = serde_json::from_str(json).unwrap();

        assert!(!config.format.use_unicode);
        assert!(config.format.enforce_unicode);
        assert!(config.format.private_use_glyphs);
        assert_eq!(config.format.indentation, "  ");

        assert!(!config.diagnostics.enabled);
        assert_eq!(config.diagnostics.debounce_ms, 1000);

        assert!(config.completion.enabled);
    }

    #[test]
    fn test_partial_json_uses_defaults() {
        let json = r#"{
            "format": {
                "useUnicode": false
            }
        }"#;

        let config: RossiConfig = serde_json::from_str(json).unwrap();

        // Specified value
        assert!(!config.format.use_unicode);

        // Default values ("" = follow the style preset)
        assert_eq!(config.format.indentation, "");
        assert!(!config.format.enforce_unicode);
        assert!(config.diagnostics.enabled);
        assert!(config.rodin.mirror_proofs);
    }

    #[test]
    fn test_format_style_settings_map_to_printer() {
        let settings = serde_json::json!({
            "rossi": { "format": { "style": "camille" } }
        });
        let config = RossiConfig::from_client_settings(&settings).unwrap();
        let printer = config.format.printer();
        assert_eq!(printer.style, rossi::Style::Camille);
        assert_eq!(printer.indent, "  ");
        assert_eq!(printer.keyword_case, rossi::KeywordCase::Lower);
        assert_eq!(printer.decl_lists, rossi::DeclListLayout::Inline);
        assert!(printer.blank_between_clauses);

        let settings = serde_json::json!({
            "rossi": { "format": {
                "style": "camille",
                "keywordCase": "upper",
                "declLists": "one-per-line",
                "blankBetweenClauses": false,
                "indentation": "    "
            } }
        });
        let config = RossiConfig::from_client_settings(&settings).unwrap();
        let printer = config.format.printer();
        assert_eq!(printer.style, rossi::Style::Camille);
        assert_eq!(printer.keyword_case, rossi::KeywordCase::Upper);
        assert_eq!(printer.decl_lists, rossi::DeclListLayout::OnePerLine);
        assert!(!printer.blank_between_clauses);
        assert_eq!(printer.indent, "    ");
    }

    #[test]
    fn test_format_max_line_width_maps_to_printer() {
        // Absent: the 120-column default.
        let settings = serde_json::json!({ "rossi": { "format": {} } });
        let config = RossiConfig::from_client_settings(&settings).unwrap();
        assert_eq!(config.format.printer().max_line_width, 120);

        // Explicit value.
        let settings = serde_json::json!({
            "rossi": { "format": { "maxLineWidth": 100 } }
        });
        let config = RossiConfig::from_client_settings(&settings).unwrap();
        assert_eq!(config.format.printer().max_line_width, 100);

        // 0 disables wrapping.
        let settings = serde_json::json!({
            "rossi": { "format": { "maxLineWidth": 0 } }
        });
        let config = RossiConfig::from_client_settings(&settings).unwrap();
        assert_eq!(config.format.printer().max_line_width, 0);
    }

    #[test]
    fn test_format_invalid_non_string_values_keep_whole_config() {
        // `from_client_settings` is all-or-nothing: a mistyped value in the
        // non-string fields (some clients enforce no schema) must fall back
        // to the default instead of discarding the whole configuration —
        // the sibling settings must survive.
        for bad_width in [
            serde_json::json!(-1),
            serde_json::json!(120.5),
            serde_json::json!("120"),
            serde_json::json!(u64::from(u32::MAX) + 1),
        ] {
            let settings = serde_json::json!({
                "rossi": {
                    "format": { "maxLineWidth": bad_width, "useUnicode": false },
                    "diagnostics": { "enabled": false }
                }
            });
            let config = RossiConfig::from_client_settings(&settings)
                .unwrap_or_else(|e| panic!("config discarded for {bad_width}: {e}"));
            assert_eq!(config.format.max_line_width, 120, "for {bad_width}");
            assert!(!config.format.use_unicode);
            assert!(!config.diagnostics.enabled);
        }

        let settings = serde_json::json!({
            "rossi": { "format": { "blankBetweenClauses": "false" } }
        });
        let config = RossiConfig::from_client_settings(&settings).unwrap();
        assert_eq!(config.format.blank_between_clauses, None);
    }

    #[test]
    fn test_format_unknown_style_values_fall_back_to_preset() {
        // Unknown values must not fail whole-config parsing (it is
        // all-or-nothing) and fall back to the preset defaults.
        let settings = serde_json::json!({
            "rossi": { "format": {
                "style": "fancy",
                "keywordCase": "mixed",
                "declLists": "wat"
            } }
        });
        let config = RossiConfig::from_client_settings(&settings).unwrap();
        let printer = config.format.printer();
        let preset = rossi::PrettyPrinter::styled(rossi::Style::default());
        assert_eq!(printer.style, preset.style);
        assert_eq!(printer.keyword_case, preset.keyword_case);
        assert_eq!(printer.decl_lists, preset.decl_lists);
        assert_eq!(printer.indent, preset.indent);
    }

    #[test]
    fn test_default_config_printer_matches_library_default() {
        // An untouched client config denotes the library's default printer
        // plus the user-facing 120-column wrap — the same resolved printer
        // the CLI defaults build, so LSP formatting and `rossi fmt` agree
        // byte-for-byte.
        let lsp = FormatConfig::default().printer();
        let expected = rossi::PrettyPrinter::resolved(
            rossi::Style::default(),
            &rossi::StyleOverrides {
                max_line_width: 120,
                ..rossi::StyleOverrides::default()
            },
        );
        assert_eq!(lsp, expected);

        // The wrap width is the ONLY deliberate delta from the library
        // default (which stays flat for XML/canonical output).
        let library = rossi::PrettyPrinter::default();
        let flattened = FormatConfig {
            max_line_width: 0,
            ..FormatConfig::default()
        }
        .printer();
        assert_eq!(flattened, library);
    }

    #[test]
    fn test_format_empty_indentation_follows_preset() {
        let rossi_style = FormatConfig {
            style: "rossi".to_string(),
            ..FormatConfig::default()
        };
        assert_eq!(rossi_style.printer().indent, "    ");

        let camille_style = FormatConfig {
            style: "camille".to_string(),
            ..FormatConfig::default()
        };
        assert_eq!(camille_style.printer().indent, "  ");
    }

    #[test]
    fn test_client_settings_direct_config() {
        let settings = serde_json::json!({
            "format": {
                "useUnicode": false,
                "indentation": "  "
            },
            "diagnostics": {
                "enabled": false
            }
        });

        let config = RossiConfig::from_client_settings(&settings).unwrap();
        assert!(!config.format.use_unicode);
        assert_eq!(config.format.indentation, "  ");
        assert!(!config.diagnostics.enabled);
    }

    #[test]
    fn test_client_settings_nested_rossi_config() {
        let settings = serde_json::json!({
            "rossi": {
                "format": {
                    "useUnicode": false
                },
                "diagnostics": {
                    "debounceMs": 250
                }
            }
        });

        let config = RossiConfig::from_client_settings(&settings).unwrap();
        assert!(!config.format.use_unicode);
        assert_eq!(config.diagnostics.debounce_ms, 250);
    }

    #[test]
    fn test_rodin_config_defaults_and_nested_parse() {
        let config = RossiConfig::default();
        assert_eq!(config.rodin.path, "");
        assert_eq!(config.rodin.workspace, "");
        assert!(config.rodin.sync);

        let settings = serde_json::json!({
            "rossi": {
                "rodin": {
                    "path": "/Applications/Rodin.app",
                    "workspace": "/tmp/rodin-ws",
                    "sync": false
                }
            }
        });
        let config = RossiConfig::from_client_settings(&settings).unwrap();
        assert_eq!(config.rodin.path, "/Applications/Rodin.app");
        assert_eq!(config.rodin.workspace, "/tmp/rodin-ws");
        assert!(!config.rodin.sync);

        // An absent key keeps sync on.
        let settings = serde_json::json!({ "rossi": { "rodin": {} } });
        let config = RossiConfig::from_client_settings(&settings).unwrap();
        assert!(config.rodin.sync);
    }

    #[test]
    fn test_animate_config_defaults_and_nested_parse() {
        let config = RossiConfig::default();
        assert_eq!(config.animate.path, "");
        assert_eq!(config.animate.time_limit_secs, 120);
        assert_eq!(config.animate.disprove_timeout_ms, 1000);

        let settings = serde_json::json!({
            "rossi": {
                "animate": {
                    "path": "/opt/eventb-animate/bin/eventb-animate",
                    "timeLimitSecs": 30,
                    "disproveTimeoutMs": 500
                }
            }
        });
        let config = RossiConfig::from_client_settings(&settings).unwrap();
        assert_eq!(
            config.animate.path,
            "/opt/eventb-animate/bin/eventb-animate"
        );
        assert_eq!(config.animate.time_limit_secs, 30);
        assert_eq!(config.animate.disprove_timeout_ms, 500);

        // An absent key keeps the defaults; 0 falls back to them.
        let settings = serde_json::json!({ "rossi": { "animate": {} } });
        let config = RossiConfig::from_client_settings(&settings).unwrap();
        assert_eq!(config.animate.time_limit_secs, 120);
        let zeroed = AnimateConfig {
            time_limit_secs: 0,
            disprove_timeout_ms: 0,
            ..AnimateConfig::default()
        };
        assert_eq!(zeroed.effective_time_limit_secs(), 120);
        assert_eq!(zeroed.effective_disprove_timeout_ms(), 1000);
    }
}
