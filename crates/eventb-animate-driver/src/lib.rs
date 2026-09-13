//! Driving `eventb-animate`, the ProB-backed model checker of the Event-B
//! toolchain: which binary to spawn, the command line of each run mode, the
//! watchdog that keeps a hung JVM from wedging its caller, and the verdicts
//! read out of the tool's JSON report ([`report`]).
//!
//! Every run uses `--json -`: stdout carries exactly one JSON document and
//! all human output goes to stderr, so a caller classifies the run from the
//! document and the exit code together. The language server and the MCP
//! server both drive the tool this way; this crate is the one place the
//! contract with the tool is written down.

pub mod report;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The bare command an empty path setting resolves via PATH.
pub const TOOL_NAME: &str = "eventb-animate";

/// Watchdog headroom past the tool's own internal limits. Deliberately
/// generous: the very first run on a machine extracts ProB into `~/.prob`
/// (tens of seconds, outside `--time-limit`), and every run pays a cold JVM
/// start. A genuinely hung process still dies.
pub const GRACE: Duration = Duration::from_secs(90);

/// How to run the tool: where it is and how long it may work.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimateConfig {
    /// eventb-animate executable path or bare command name. Empty resolves
    /// `eventb-animate` via PATH when a run starts.
    #[serde(default)]
    pub path: String,

    /// `--time-limit` (seconds) for a model check; the watchdog that kills
    /// a hung tool derives from it. `0` selects the default.
    #[serde(default = "default_time_limit_secs")]
    pub time_limit_secs: u32,

    /// `--disprove-timeout` (milliseconds) per proof obligation for a
    /// disprover run; also feeds that run's watchdog. `0` selects the
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

/// Which of the two run modes a request runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnimateMode {
    /// Explicit-state model check (invariants + deadlock).
    Check,
    /// `po --disprove`: attempt a ProB disproof of every open obligation.
    Po,
}

/// Everything that can end a tool run without a report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// The tool is not installed where the configuration points.
    Missing(String),
    /// The tool ran but failed or produced no parseable report.
    Failed(String),
    /// The watchdog killed a run that outlived its deadline (seconds).
    Timeout(u64),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::Missing(program) => write!(f, "eventb-animate was not found ({program})"),
            ToolError::Failed(message) => write!(f, "eventb-animate failed: {message}"),
            ToolError::Timeout(secs) => write!(f, "eventb-animate timed out after {secs} s"),
        }
    }
}

impl std::error::Error for ToolError {}

/// The program string to spawn: the configured value, or [`TOOL_NAME`] when
/// the setting is empty.
pub fn effective_tool(configured: &str) -> String {
    let trimmed = configured.trim();
    if trimmed.is_empty() {
        TOOL_NAME.to_string()
    } else {
        trimmed.to_string()
    }
}

/// The concrete filesystem location a tool setting denotes, when it denotes
/// one. `None` for bare names, which only the spawn's PATH lookup can
/// resolve: a setting that contains a path separator is a path, anything
/// else is a name, and an existence pre-check must classify exactly as the
/// spawn does.
pub fn concrete_path(program: &str) -> Option<PathBuf> {
    (program.contains('/') || program.contains('\\')).then(|| PathBuf::from(program))
}

/// The tool invocation for one run. `--json -` puts the report alone on
/// stdout; `-m` pins the machine so the tool's own most-refined
/// auto-selection never picks a different one.
pub fn command_args(
    mode: AnimateMode,
    config: &AnimateConfig,
    machine: &str,
    project_dir: &Path,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    match mode {
        AnimateMode::Check => {
            args.push("--time-limit".into());
            args.push(config.effective_time_limit_secs().to_string().into());
        }
        AnimateMode::Po => {
            args.push("po".into());
            args.push("--disprove".into());
            args.push("--disprove-timeout".into());
            args.push(config.effective_disprove_timeout_ms().to_string().into());
        }
    }
    args.push("--json".into());
    args.push("-".into());
    args.push("-m".into());
    args.push(machine.into());
    args.push(project_dir.as_os_str().to_owned());
    args
}

/// The outer deadline for one run. Check is bounded by its own
/// `--time-limit`; po runs one solver attempt per open obligation, so the
/// deadline scales with `po_count` (the still-open count once recorded
/// proof state is merged; every generated sequent when there is none),
/// doubled for slack around solver setup per obligation. An all-discharged
/// run leaves [`GRACE`] alone, ample for the gate-only pass.
pub fn watchdog(mode: AnimateMode, config: &AnimateConfig, po_count: usize) -> Duration {
    match mode {
        AnimateMode::Check => {
            GRACE + Duration::from_secs(u64::from(config.effective_time_limit_secs()))
        }
        AnimateMode::Po => {
            GRACE
                + Duration::from_millis(
                    2 * po_count as u64 * u64::from(config.effective_disprove_timeout_ms()),
                )
        }
    }
}

/// What a finished run left behind.
#[derive(Debug)]
pub struct ToolOutput {
    pub stdout: String,
    pub stderr: String,
    /// The process exit code, or `None` when a signal killed the tool.
    /// eventb-animate names the failure kind here: 66 for an unusable input,
    /// 70 for its own failure.
    pub code: Option<i32>,
}

/// Spawn the tool and wait for it under `watchdog`. On timeout the whole
/// process group is killed (unix): the packaged tool is a launcher script
/// that may not `exec` its JVM, and `kill_on_drop` alone would only reap the
/// launcher; elsewhere `kill_on_drop` is the fallback.
pub async fn run_tool(
    program: &str,
    args: &[OsString],
    watchdog: Duration,
) -> Result<ToolOutput, ToolError> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ToolError::Missing(program.to_string()));
        }
        Err(error) => {
            return Err(ToolError::Failed(format!(
                "failed to start '{program}': {error}"
            )));
        }
    };
    #[cfg(unix)]
    let pid = child.id();
    match tokio::time::timeout(watchdog, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok(ToolOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            code: output.status.code(),
        }),
        Ok(Err(error)) => Err(ToolError::Failed(format!(
            "waiting for '{program}' failed: {error}"
        ))),
        Err(_elapsed) => {
            // Dropping the wait future already kill_on_drop'd the direct
            // child; take its whole group down with it.
            #[cfg(unix)]
            if let Some(pid) = pid {
                // The child was made its own group leader via
                // process_group(0), so its pid is the pgid.
                unsafe { libc::killpg(pid as i32, libc::SIGKILL) };
            }
            Err(ToolError::Timeout(watchdog.as_secs()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_lines_match_the_tool_contract() {
        let config = AnimateConfig {
            time_limit_secs: 30,
            disprove_timeout_ms: 500,
            ..AnimateConfig::default()
        };
        let dir = Path::new("/tmp/proj");
        let check: Vec<_> = command_args(AnimateMode::Check, &config, "M1", dir)
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            check,
            ["--time-limit", "30", "--json", "-", "-m", "M1", "/tmp/proj"]
        );
        let po: Vec<_> = command_args(AnimateMode::Po, &config, "M1", dir)
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            po,
            [
                "po",
                "--disprove",
                "--disprove-timeout",
                "500",
                "--json",
                "-",
                "-m",
                "M1",
                "/tmp/proj"
            ]
        );
    }

    #[test]
    fn watchdogs_scale_with_their_bounds() {
        let config = AnimateConfig {
            time_limit_secs: 10,
            disprove_timeout_ms: 1000,
            ..AnimateConfig::default()
        };
        assert_eq!(
            watchdog(AnimateMode::Check, &config, 0),
            GRACE + Duration::from_secs(10)
        );
        assert_eq!(
            watchdog(AnimateMode::Po, &config, 5),
            GRACE + Duration::from_secs(10)
        );
    }

    #[test]
    fn zero_settings_select_the_defaults() {
        let config = AnimateConfig {
            time_limit_secs: 0,
            disprove_timeout_ms: 0,
            ..AnimateConfig::default()
        };
        assert_eq!(config.effective_time_limit_secs(), 120);
        assert_eq!(config.effective_disprove_timeout_ms(), 1000);
    }

    #[test]
    fn bare_names_and_concrete_paths_are_told_apart() {
        assert_eq!(effective_tool(""), TOOL_NAME);
        assert_eq!(effective_tool("  "), TOOL_NAME);
        assert_eq!(effective_tool("my-animate"), "my-animate");
        assert_eq!(concrete_path("eventb-animate"), None);
        assert_eq!(
            concrete_path("/opt/bin/eventb-animate"),
            Some(PathBuf::from("/opt/bin/eventb-animate"))
        );
        assert_eq!(
            concrete_path("bin\\eventb-animate.bat"),
            Some(PathBuf::from("bin\\eventb-animate.bat"))
        );
    }

    #[tokio::test]
    async fn missing_bare_tool_reports_tool_missing() {
        let error = run_tool(
            "rossi-test-definitely-not-installed",
            &[],
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, ToolError::Missing(_)), "{error:?}");
    }
}
