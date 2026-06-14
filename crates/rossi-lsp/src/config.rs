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

    /// Trace configuration
    #[serde(default)]
    pub trace: TraceConfig,
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

/// Formatting configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatConfig {
    /// Use Unicode operators (∧, ∨, ⇒) instead of ASCII (/\, \/, =>)
    #[serde(default = "default_use_unicode")]
    pub use_unicode: bool,

    /// Indentation string (e.g., "  " or "    ")
    #[serde(default = "default_indentation")]
    pub indentation: String,

    /// Maximum line length for formatting (currently not used)
    #[serde(default = "default_max_line_length")]
    pub max_line_length: u32,
}

impl Default for FormatConfig {
    fn default() -> Self {
        Self {
            use_unicode: default_use_unicode(),
            indentation: default_indentation(),
            max_line_length: default_max_line_length(),
        }
    }
}

fn default_use_unicode() -> bool {
    true
}

fn default_indentation() -> String {
    "    ".to_string()
}

fn default_max_line_length() -> u32 {
    100
}

/// Diagnostics configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsConfig {
    /// Enable diagnostics
    #[serde(default = "default_diagnostics_enabled")]
    pub enabled: bool,

    /// Debounce delay in milliseconds (currently not used)
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

    /// Trigger characters for completion
    #[serde(default = "default_trigger_characters")]
    pub trigger_characters: Vec<String>,
}

impl Default for CompletionConfig {
    fn default() -> Self {
        Self {
            enabled: default_completion_enabled(),
            trigger_characters: default_trigger_characters(),
        }
    }
}

fn default_completion_enabled() -> bool {
    true
}

fn default_trigger_characters() -> Vec<String> {
    vec![
        ":".to_string(),
        ".".to_string(),
        "(".to_string(),
        "{".to_string(),
    ]
}

/// Trace configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceConfig {
    /// Server trace level: "off", "messages", or "verbose"
    #[serde(default = "default_trace_level")]
    pub server: String,
}

impl Default for TraceConfig {
    fn default() -> Self {
        Self {
            server: default_trace_level(),
        }
    }
}

fn default_trace_level() -> String {
    "off".to_string()
}

/// Configuration manager that holds the current configuration
pub struct ConfigManager {
    config: Arc<RwLock<RossiConfig>>,
}

impl ConfigManager {
    /// Create a new configuration manager with default settings
    pub fn new() -> Self {
        Self {
            config: Arc::new(RwLock::new(RossiConfig::default())),
        }
    }

    /// Get the current configuration
    pub fn get(&self) -> RossiConfig {
        self.config.read().clone()
    }

    /// Update the entire configuration
    pub fn update(&self, config: RossiConfig) {
        *self.config.write() = config;
    }

    /// Update just the format configuration
    #[allow(dead_code)]
    pub fn update_format(&self, format: FormatConfig) {
        self.config.write().format = format;
    }

    /// Update just the diagnostics configuration
    #[allow(dead_code)]
    pub fn update_diagnostics(&self, diagnostics: DiagnosticsConfig) {
        self.config.write().diagnostics = diagnostics;
    }

    /// Update just the completion configuration
    #[allow(dead_code)]
    pub fn update_completion(&self, completion: CompletionConfig) {
        self.config.write().completion = completion;
    }

    /// Update just the trace configuration
    #[allow(dead_code)]
    pub fn update_trace(&self, trace: TraceConfig) {
        self.config.write().trace = trace;
    }

    /// Get the format configuration
    #[allow(dead_code)]
    pub fn get_format(&self) -> FormatConfig {
        self.config.read().format.clone()
    }

    /// Get the diagnostics configuration
    #[allow(dead_code)]
    pub fn get_diagnostics(&self) -> DiagnosticsConfig {
        self.config.read().diagnostics.clone()
    }

    /// Get the completion configuration
    #[allow(dead_code)]
    pub fn get_completion(&self) -> CompletionConfig {
        self.config.read().completion.clone()
    }

    /// Get the trace configuration
    #[allow(dead_code)]
    pub fn get_trace(&self) -> TraceConfig {
        self.config.read().trace.clone()
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
        assert_eq!(config.format.indentation, "    ");
        assert_eq!(config.format.max_line_length, 100);

        assert!(config.diagnostics.enabled);
        assert_eq!(config.diagnostics.debounce_ms, 500);

        assert!(config.completion.enabled);
        assert_eq!(config.completion.trigger_characters.len(), 4);

        assert_eq!(config.trace.server, "off");
    }

    #[test]
    fn test_config_manager_get_set() {
        let manager = ConfigManager::new();

        // Check defaults
        let config = manager.get();
        assert!(config.format.use_unicode);

        // Update configuration
        let mut new_config = config.clone();
        new_config.format.use_unicode = false;
        new_config.format.indentation = "  ".to_string();
        manager.update(new_config);

        // Check updated values
        let updated = manager.get();
        assert!(!updated.format.use_unicode);
        assert_eq!(updated.format.indentation, "  ");
    }

    #[test]
    fn test_config_manager_partial_updates() {
        let manager = ConfigManager::new();

        // Update only format config
        manager.update_format(FormatConfig {
            use_unicode: false,
            indentation: "  ".to_string(),
            max_line_length: 120,
        });

        let config = manager.get();
        assert!(!config.format.use_unicode);
        assert_eq!(config.format.indentation, "  ");
        assert_eq!(config.format.max_line_length, 120);

        // Other configs should remain default
        assert!(config.diagnostics.enabled);
        assert!(config.completion.enabled);
    }

    #[test]
    fn test_format_config_getters() {
        let manager = ConfigManager::new();

        let format = manager.get_format();
        assert!(format.use_unicode);
        assert_eq!(format.indentation, "    ");
    }

    #[test]
    fn test_json_serialization() {
        let config = RossiConfig::default();

        // Serialize to JSON
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("format"));
        assert!(json.contains("useUnicode"));

        // Deserialize from JSON
        let deserialized: RossiConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config.format.use_unicode, deserialized.format.use_unicode);
    }

    #[test]
    fn test_json_with_custom_values() {
        let json = r#"{
            "format": {
                "useUnicode": false,
                "indentation": "  ",
                "maxLineLength": 80
            },
            "diagnostics": {
                "enabled": false,
                "debounceMs": 1000
            },
            "completion": {
                "enabled": true,
                "triggerCharacters": [":", "."]
            },
            "trace": {
                "server": "verbose"
            }
        }"#;

        let config: RossiConfig = serde_json::from_str(json).unwrap();

        assert!(!config.format.use_unicode);
        assert_eq!(config.format.indentation, "  ");
        assert_eq!(config.format.max_line_length, 80);

        assert!(!config.diagnostics.enabled);
        assert_eq!(config.diagnostics.debounce_ms, 1000);

        assert!(config.completion.enabled);
        assert_eq!(config.completion.trigger_characters.len(), 2);

        assert_eq!(config.trace.server, "verbose");
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

        // Default values
        assert_eq!(config.format.indentation, "    ");
        assert_eq!(config.format.max_line_length, 100);
        assert!(config.diagnostics.enabled);
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
}
