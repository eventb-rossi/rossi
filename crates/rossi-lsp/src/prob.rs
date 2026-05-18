//! ProB Integration Module
//!
//! This module provides integration with the ProB animator and model checker for Event-B.
//!
//! ProB is a powerful tool for Event-B that provides:
//! - Animation (executing the model)
//! - Model checking
//! - Constraint solving
//! - Counterexample generation
//!
//! This module supports:
//! - Detecting ProB installation
//! - Launching ProB animator from IDE
//! - Running model checking from IDE
//! - Displaying counterexamples
//! - Code lenses for "Run ProB" and "Animate" actions
//! - Integrating ProB feedback into diagnostics

use lsp_types::{CodeLens, Command, Position, Range, Url};
use parking_lot::RwLock;
use std::path::Path;
use std::process::{Command as ProcessCommand, Stdio};
use tracing::{debug, info, warn};

use crate::config::ProBConfig;

/// ProB integration provider
pub struct ProBProvider {
    /// Path to probcli executable (None if not detected)
    probcli_path: RwLock<Option<String>>,
    /// Configuration
    config: RwLock<ProBConfig>,
}

impl ProBProvider {
    /// Create a new ProB provider
    pub fn new() -> Self {
        let probcli_path = Self::detect_probcli();
        if let Some(ref path) = probcli_path {
            info!("ProB detected at: {}", path);
        } else {
            warn!("ProB (probcli) not found in PATH. ProB features will be disabled.");
        }

        Self {
            probcli_path: RwLock::new(probcli_path),
            config: RwLock::new(ProBConfig::default()),
        }
    }

    #[cfg(test)]
    fn new_for_test() -> Self {
        Self {
            probcli_path: RwLock::new(None),
            config: RwLock::new(ProBConfig::default()),
        }
    }

    #[cfg(test)]
    fn new_for_test_available() -> Self {
        Self {
            probcli_path: RwLock::new(Some("probcli".to_string())),
            config: RwLock::new(ProBConfig::default()),
        }
    }

    /// Detect ProB installation by looking for probcli in PATH
    fn detect_probcli() -> Option<String> {
        debug!("Attempting to detect ProB installation");

        // Try to run 'probcli -version' to check if probcli is available
        let result = ProcessCommand::new("probcli")
            .arg("-version")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();

        match result {
            Ok(output) if output.status.success() => {
                let version = String::from_utf8_lossy(&output.stdout);
                info!("ProB version: {}", version.trim());
                Some("probcli".to_string())
            }
            Ok(output) => {
                debug!(
                    "probcli found but returned error: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                None
            }
            Err(e) => {
                debug!("probcli not found: {}", e);
                None
            }
        }
    }

    /// Update configuration
    ///
    /// If `config.path` is non-empty, uses it as the probcli path.
    /// If empty, keeps the auto-detected path.
    pub fn update_config(&self, config: ProBConfig) {
        if !config.path.is_empty() {
            *self.probcli_path.write() = Some(config.path.clone());
        }
        *self.config.write() = config;
    }

    /// Check if ProB is available
    pub fn is_available(&self) -> bool {
        self.config.read().enabled && self.probcli_path.read().is_some()
    }

    /// Provide code lenses for ProB actions on a document
    ///
    /// This adds code lenses at MACHINE and CONTEXT declarations with:
    /// - "Animate with ProB" - launches ProB animator
    /// - "Check with ProB" - runs model checking
    pub fn provide_code_lenses(&self, source: &str, uri: &Url) -> Vec<CodeLens> {
        let mut lenses = Vec::new();

        // Only provide code lenses if ProB is available
        if !self.is_available() {
            return lenses;
        }

        // Parse to find MACHINE and CONTEXT declarations
        let lines: Vec<&str> = source.lines().collect();

        for (line_num, line) in lines.iter().enumerate() {
            let trimmed = line.trim();

            // Check for MACHINE declaration
            if trimmed.starts_with("MACHINE") {
                let range = Range {
                    start: Position {
                        line: line_num as u32,
                        character: 0,
                    },
                    end: Position {
                        line: line_num as u32,
                        character: line.len() as u32,
                    },
                };

                // Add "Animate with ProB" code lens
                lenses.push(CodeLens {
                    range,
                    command: Some(Command {
                        title: "▶ Animate with ProB".to_string(),
                        command: "rossi.prob.animate".to_string(),
                        arguments: Some(vec![serde_json::json!(uri.to_string())]),
                    }),
                    data: None,
                });

                // Add "Check with ProB" code lens
                lenses.push(CodeLens {
                    range,
                    command: Some(Command {
                        title: "✓ Check with ProB".to_string(),
                        command: "rossi.prob.modelcheck".to_string(),
                        arguments: Some(vec![serde_json::json!(uri.to_string())]),
                    }),
                    data: None,
                });
            }

            // Check for CONTEXT declaration
            if trimmed.starts_with("CONTEXT") {
                let range = Range {
                    start: Position {
                        line: line_num as u32,
                        character: 0,
                    },
                    end: Position {
                        line: line_num as u32,
                        character: line.len() as u32,
                    },
                };

                // Add "Check with ProB" code lens for contexts
                lenses.push(CodeLens {
                    range,
                    command: Some(Command {
                        title: "✓ Check with ProB".to_string(),
                        command: "rossi.prob.modelcheck".to_string(),
                        arguments: Some(vec![serde_json::json!(uri.to_string())]),
                    }),
                    data: None,
                });
            }
        }

        debug!("Providing {} ProB code lenses", lenses.len());
        lenses
    }

    /// Execute ProB animation on a file
    ///
    /// This launches the ProB animator for the given Event-B file.
    /// The animator allows interactive exploration of the model.
    pub fn animate(&self, file_path: &Path) -> Result<ProBResult, ProBError> {
        let probcli_path = self.probcli_path.read();
        let probcli = probcli_path.as_ref().ok_or(ProBError::NotInstalled)?;

        info!("Launching ProB animator for: {:?}", file_path);

        // Check if file exists
        if !file_path.exists() {
            return Err(ProBError::FileNotFound(
                file_path.to_string_lossy().to_string(),
            ));
        }

        let steps = self.config.read().animate_steps;

        let mut cmd = ProcessCommand::new(probcli);
        cmd.arg(file_path);

        if steps > 0 {
            cmd.arg("-animate").arg(steps.to_string());
        } else {
            cmd.arg("-init");
        }

        let output = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| ProBError::ExecutionFailed(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output.status.success() {
            Ok(ProBResult {
                success: true,
                stdout,
                stderr,
                counterexamples: Vec::new(),
            })
        } else {
            Err(ProBError::AnimationFailed {
                stderr,
                exit_code: output.status.code(),
            })
        }
    }

    /// Execute ProB model checking on a file
    ///
    /// This runs ProB model checker on the given Event-B file.
    /// It checks for invariant violations, deadlocks, and other properties.
    pub fn modelcheck(&self, file_path: &Path) -> Result<ProBResult, ProBError> {
        let probcli_path = self.probcli_path.read();
        let probcli = probcli_path.as_ref().ok_or(ProBError::NotInstalled)?;

        info!("Running ProB model checker for: {:?}", file_path);

        // Check if file exists
        if !file_path.exists() {
            return Err(ProBError::FileNotFound(
                file_path.to_string_lossy().to_string(),
            ));
        }

        let timeout = self.config.read().timeout;

        // Run model checking with probcli
        // Note: we do NOT pass -nodead or -noinv, as those flags *disable*
        // deadlock and invariant checking respectively. Without them, probcli
        // checks all three categories (invariants, deadlocks, assertions) by default.
        let output = ProcessCommand::new(probcli)
            .arg(file_path)
            .arg("-model_check")
            .arg("-timeout")
            .arg(timeout.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| ProBError::ExecutionFailed(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        // Parse output for counterexamples
        let counterexamples = Self::parse_counterexamples(&stdout);

        if output.status.success() && counterexamples.is_empty() {
            Ok(ProBResult {
                success: true,
                stdout,
                stderr,
                counterexamples,
            })
        } else if !counterexamples.is_empty() {
            Ok(ProBResult {
                success: false,
                stdout,
                stderr,
                counterexamples,
            })
        } else {
            Err(ProBError::ModelCheckFailed {
                stderr,
                exit_code: output.status.code(),
            })
        }
    }

    /// Parse ProB output for counterexamples
    ///
    /// Uses negative guards to avoid false positives from lines like
    /// "No INVARIANT VIOLATION found" or "Model checking completed without DEADLOCK".
    fn parse_counterexamples(output: &str) -> Vec<Counterexample> {
        let mut counterexamples = Vec::new();

        for line in output.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let upper = trimmed.to_uppercase();

            // Check for invariant violation
            if let Some(pos) = upper.find("INVARIANT VIOLATION")
                && !Self::is_negated(&upper, pos)
            {
                counterexamples.push(Counterexample {
                    kind: CounterexampleKind::InvariantViolation,
                    description: trimmed.to_string(),
                });
                continue;
            }

            // Check for deadlock
            if let Some(pos) = upper.find("DEADLOCK")
                && !Self::is_negated(&upper, pos)
            {
                counterexamples.push(Counterexample {
                    kind: CounterexampleKind::Deadlock,
                    description: trimmed.to_string(),
                });
                continue;
            }

            // Check for assertion failure (requires both ASSERTION and VIOLATION)
            if upper.contains("VIOLATION")
                && let Some(pos) = upper.find("ASSERTION")
                && !Self::is_negated(&upper, pos)
            {
                counterexamples.push(Counterexample {
                    kind: CounterexampleKind::AssertionFailure,
                    description: trimmed.to_string(),
                });
            }
        }

        if !counterexamples.is_empty() {
            info!("Found {} counterexample(s)", counterexamples.len());
        }

        counterexamples
    }

    /// Check whether the text before `keyword_pos` contains a negation word
    /// (e.g. "No ", "without"), indicating the line is a negative report.
    fn is_negated(upper_line: &str, keyword_pos: usize) -> bool {
        let before = &upper_line[..keyword_pos];
        before.contains("NO ") || before.contains("WITHOUT")
    }

    /// Find the 0-based line number of a keyword in the source text
    fn find_keyword_line(source: &str, keyword: &str) -> Option<u32> {
        for (line_num, line) in source.lines().enumerate() {
            if line.trim().starts_with(keyword) {
                return Some(line_num as u32);
            }
        }
        None
    }

    /// Convert ProB results into LSP diagnostics
    ///
    /// This converts counterexamples and errors from ProB into LSP diagnostics
    /// that can be displayed in the editor. Diagnostics are positioned at the
    /// relevant section (INVARIANTS, MACHINE, AXIOMS) when possible.
    pub fn results_to_diagnostics(
        &self,
        result: &ProBResult,
        source: &str,
    ) -> Vec<lsp_types::Diagnostic> {
        let mut diagnostics = Vec::new();

        for (idx, counterexample) in result.counterexamples.iter().enumerate() {
            let line = match counterexample.kind {
                CounterexampleKind::InvariantViolation => {
                    Self::find_keyword_line(source, "INVARIANTS").unwrap_or(0)
                }
                CounterexampleKind::Deadlock => {
                    Self::find_keyword_line(source, "MACHINE").unwrap_or(0)
                }
                CounterexampleKind::AssertionFailure => {
                    Self::find_keyword_line(source, "AXIOMS").unwrap_or(0)
                }
            };

            diagnostics.push(lsp_types::Diagnostic {
                range: Range {
                    start: Position { line, character: 0 },
                    end: Position { line, character: 0 },
                },
                severity: Some(lsp_types::DiagnosticSeverity::ERROR),
                code: Some(lsp_types::NumberOrString::String(format!("prob-{}", idx))),
                code_description: None,
                source: Some("ProB".to_string()),
                message: counterexample.description.clone(),
                related_information: None,
                tags: None,
                data: None,
            });
        }

        diagnostics
    }
}

impl Default for ProBProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// Result from a ProB operation
#[derive(Debug, Clone)]
pub struct ProBResult {
    /// Whether the operation succeeded
    pub success: bool,
    /// Standard output from ProB
    pub stdout: String,
    /// Standard error from ProB
    #[allow(dead_code)]
    pub stderr: String,
    /// Counterexamples found (if any)
    pub counterexamples: Vec<Counterexample>,
}

/// A counterexample found by ProB
#[derive(Debug, Clone)]
pub struct Counterexample {
    /// Kind of counterexample
    pub kind: CounterexampleKind,
    /// Description of the counterexample
    pub description: String,
}

/// Kind of counterexample
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CounterexampleKind {
    /// Invariant violation
    InvariantViolation,
    /// Deadlock detected
    Deadlock,
    /// Assertion failure
    AssertionFailure,
}

/// Errors that can occur during ProB operations
#[derive(Debug, Clone)]
pub enum ProBError {
    /// ProB is not installed
    NotInstalled,
    /// File not found
    FileNotFound(String),
    /// Execution failed
    ExecutionFailed(String),
    /// Animation failed
    AnimationFailed {
        stderr: String,
        exit_code: Option<i32>,
    },
    /// Model checking failed
    ModelCheckFailed {
        stderr: String,
        exit_code: Option<i32>,
    },
}

impl std::fmt::Display for ProBError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProBError::NotInstalled => {
                write!(f, "ProB (probcli) is not installed or not found in PATH")
            }
            ProBError::FileNotFound(path) => write!(f, "File not found: {}", path),
            ProBError::ExecutionFailed(msg) => write!(f, "Execution failed: {}", msg),
            ProBError::AnimationFailed {
                stderr, exit_code, ..
            } => {
                write!(
                    f,
                    "Animation failed (exit code: {:?}): {}",
                    exit_code, stderr
                )
            }
            ProBError::ModelCheckFailed {
                stderr, exit_code, ..
            } => {
                write!(
                    f,
                    "Model checking failed (exit code: {:?}): {}",
                    exit_code, stderr
                )
            }
        }
    }
}

impl std::error::Error for ProBError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prob_provider_creation() {
        let provider = ProBProvider::new_for_test();
        // Provider should be created with no probcli path
        assert!(provider.probcli_path.read().is_none());
    }

    #[test]
    fn test_parse_counterexamples_invariant_violation() {
        let output = "Error: INVARIANT VIOLATION in state 5";

        let counterexamples = ProBProvider::parse_counterexamples(output);

        assert_eq!(counterexamples.len(), 1);
        assert_eq!(
            counterexamples[0].kind,
            CounterexampleKind::InvariantViolation
        );
    }

    #[test]
    fn test_parse_counterexamples_deadlock() {
        let output = "Error: DEADLOCK detected in state 3";

        let counterexamples = ProBProvider::parse_counterexamples(output);

        assert_eq!(counterexamples.len(), 1);
        assert_eq!(counterexamples[0].kind, CounterexampleKind::Deadlock);
    }

    #[test]
    fn test_parse_counterexamples_none() {
        let output = "Model checking completed successfully";

        let counterexamples = ProBProvider::parse_counterexamples(output);

        assert_eq!(counterexamples.len(), 0);
    }

    #[test]
    fn test_parse_counterexamples_no_false_positive() {
        // These lines should NOT produce counterexamples
        let output = "No INVARIANT VIOLATION found\n\
                       Model checking completed without DEADLOCK\n\
                       No ASSERTION VIOLATION detected";

        let counterexamples = ProBProvider::parse_counterexamples(output);
        assert_eq!(counterexamples.len(), 0);
    }

    #[test]
    fn test_parse_counterexamples_assertion_requires_violation() {
        // "ASSERTION" alone (without "VIOLATION") should not match
        let output = "Checking ASSERTION properties";

        let counterexamples = ProBProvider::parse_counterexamples(output);
        assert_eq!(counterexamples.len(), 0);

        // But with both keywords it should match
        let output = "ASSERTION VIOLATION found in state 2";
        let counterexamples = ProBProvider::parse_counterexamples(output);
        assert_eq!(counterexamples.len(), 1);
        assert_eq!(
            counterexamples[0].kind,
            CounterexampleKind::AssertionFailure
        );
    }

    #[test]
    fn test_code_lenses_without_prob() {
        let provider = ProBProvider::new_for_test();
        let source = "MACHINE Example\nEND";
        let uri = Url::parse("file:///test.eventb").unwrap();

        let lenses = provider.provide_code_lenses(source, &uri);

        // Should not provide code lenses if ProB is not available
        assert_eq!(lenses.len(), 0);
    }

    #[test]
    fn test_code_lenses_with_machine() {
        let provider = ProBProvider::new_for_test_available();
        let source = "MACHINE Example\nEND";
        let uri = Url::parse("file:///test.eventb").unwrap();

        let lenses = provider.provide_code_lenses(source, &uri);

        // Should provide 2 code lenses for MACHINE (animate and check)
        assert_eq!(lenses.len(), 2);
        assert!(
            lenses[0]
                .command
                .as_ref()
                .unwrap()
                .title
                .contains("Animate")
        );
        assert!(lenses[1].command.as_ref().unwrap().title.contains("Check"));
    }

    #[test]
    fn test_code_lenses_with_context() {
        let provider = ProBProvider::new_for_test_available();
        let source = "CONTEXT Example\nEND";
        let uri = Url::parse("file:///test.eventb").unwrap();

        let lenses = provider.provide_code_lenses(source, &uri);

        // Should provide 1 code lens for CONTEXT (check only)
        assert_eq!(lenses.len(), 1);
        assert!(lenses[0].command.as_ref().unwrap().title.contains("Check"));
    }

    #[test]
    fn test_code_lenses_disabled_by_config() {
        let provider = ProBProvider::new_for_test_available();
        provider.update_config(ProBConfig {
            enabled: false,
            path: String::new(),
            ..ProBConfig::default()
        });
        let source = "MACHINE Example\nEND";
        let uri = Url::parse("file:///test.eventb").unwrap();

        let lenses = provider.provide_code_lenses(source, &uri);
        assert_eq!(lenses.len(), 0);
    }

    #[test]
    fn test_is_available_respects_config() {
        let provider = ProBProvider::new_for_test_available();
        assert!(provider.is_available());

        // Disable via config
        provider.update_config(ProBConfig {
            enabled: false,
            path: String::new(),
            ..ProBConfig::default()
        });
        assert!(!provider.is_available());
    }

    #[test]
    fn test_results_to_diagnostics() {
        let provider = ProBProvider::new_for_test();
        let result = ProBResult {
            success: false,
            stdout: String::new(),
            stderr: String::new(),
            counterexamples: vec![Counterexample {
                kind: CounterexampleKind::InvariantViolation,
                description: "Invariant x > 0 violated".to_string(),
            }],
        };

        let diagnostics = provider.results_to_diagnostics(&result, "");

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].source, Some("ProB".to_string()));
        assert!(diagnostics[0].message.contains("Invariant"));
    }

    #[test]
    fn test_results_to_diagnostics_positions() {
        let provider = ProBProvider::new_for_test();
        let source = "MACHINE Counter\nVARIABLES\n  count\nINVARIANTS\n  @inv1 count >= 0\nEND";

        // Invariant violation should point to INVARIANTS line (line 3)
        let result = ProBResult {
            success: false,
            stdout: String::new(),
            stderr: String::new(),
            counterexamples: vec![Counterexample {
                kind: CounterexampleKind::InvariantViolation,
                description: "INVARIANT VIOLATION".to_string(),
            }],
        };
        let diagnostics = provider.results_to_diagnostics(&result, source);
        assert_eq!(diagnostics[0].range.start.line, 3);

        // Deadlock should point to MACHINE line (line 0)
        let result = ProBResult {
            success: false,
            stdout: String::new(),
            stderr: String::new(),
            counterexamples: vec![Counterexample {
                kind: CounterexampleKind::Deadlock,
                description: "DEADLOCK".to_string(),
            }],
        };
        let diagnostics = provider.results_to_diagnostics(&result, source);
        assert_eq!(diagnostics[0].range.start.line, 0);
    }

    #[test]
    fn test_update_config_sets_path() {
        let provider = ProBProvider::new_for_test();
        assert!(provider.probcli_path.read().is_none());

        provider.update_config(ProBConfig {
            enabled: true,
            path: "/usr/bin/probcli".to_string(),
            ..ProBConfig::default()
        });
        assert_eq!(
            *provider.probcli_path.read(),
            Some("/usr/bin/probcli".to_string())
        );
    }

    #[test]
    fn test_update_config_empty_path_keeps_autodetected() {
        let provider = ProBProvider::new_for_test_available();
        assert_eq!(*provider.probcli_path.read(), Some("probcli".to_string()));

        // Empty path should keep the auto-detected value
        provider.update_config(ProBConfig {
            enabled: true,
            path: String::new(),
            ..ProBConfig::default()
        });
        assert_eq!(*provider.probcli_path.read(), Some("probcli".to_string()));
    }
}
