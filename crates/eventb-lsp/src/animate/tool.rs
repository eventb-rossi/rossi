//! eventb-animate binary resolution, command construction, and the watchdog
//! that keeps a hung JVM from wedging the lens.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use crate::config::AnimateConfig;

use super::{AnimateError, AnimateMode};

/// The bare command an empty `rossi.animate.path` resolves via PATH.
pub(crate) const TOOL_NAME: &str = "eventb-animate";

/// Watchdog headroom past the tool's own internal limits. Deliberately
/// generous: the very first run on a machine extracts ProB into `~/.prob`
/// (tens of seconds, outside `--time-limit`), and every run pays a cold JVM
/// start. A genuinely hung process still dies.
pub(crate) const GRACE: Duration = Duration::from_secs(90);

/// The program string to spawn: the configured value, or [`TOOL_NAME`] when
/// the setting is empty.
pub(crate) fn effective_tool(configured: &str) -> String {
    let trimmed = configured.trim();
    if trimmed.is_empty() {
        TOOL_NAME.to_string()
    } else {
        trimmed.to_string()
    }
}

/// The concrete filesystem location a tool setting denotes, when it denotes
/// one. `None` for bare names, which only the spawn's PATH lookup can
/// resolve — the shared classification rule (`launch::has_path_separator`),
/// so the existence pre-check can never disagree with the spawn.
pub(crate) fn concrete_path(program: &str) -> Option<PathBuf> {
    crate::rodin::launch::has_path_separator(program).then(|| PathBuf::from(program))
}

/// The tool invocation for one run. `--json -` puts the report alone on
/// stdout; `-m` pins the clicked machine so the tool's own most-refined
/// auto-selection never picks a different one.
pub(crate) fn command_args(
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
/// deadline scales with `po_count` — the still-open count once recorded
/// proof state is merged (every generated sequent when there is none),
/// doubled for slack around solver setup per obligation. An all-discharged
/// run leaves [`GRACE`] alone, ample for the gate-only pass.
pub(crate) fn watchdog(mode: AnimateMode, config: &AnimateConfig, po_count: usize) -> Duration {
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

#[derive(Debug)]
pub(crate) struct ToolOutput {
    pub stdout: String,
    pub stderr: String,
    /// The process exit code, or `None` when a signal killed the tool.
    /// eventb-animate names the failure kind here: 66 for an unusable input,
    /// 70 for its own failure.
    pub code: Option<i32>,
}

/// Spawn the tool and wait for it under `watchdog`, or until `cancel` fires.
/// On either the whole process group is killed (unix) — the packaged tool
/// is a launcher script that may not `exec` its JVM, and `kill_on_drop`
/// alone would only reap the launcher; elsewhere `kill_on_drop` is the
/// fallback.
pub(crate) async fn run_tool(
    program: &str,
    args: &[OsString],
    watchdog: Duration,
    cancel: Option<&Arc<crate::progress::Cancel>>,
) -> Result<ToolOutput, AnimateError> {
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
            return Err(AnimateError::ToolMissing(program.to_string()));
        }
        Err(error) => {
            return Err(AnimateError::ToolFailed(format!(
                "failed to start '{program}': {error}"
            )));
        }
    };
    let pid = child.id();
    // Dropping the wait future kill_on_drop's the direct child; the group
    // kill takes the JVM the launcher started down with it.
    let kill_group = || {
        #[cfg(unix)]
        if let Some(pid) = pid {
            // The child was made its own group leader via
            // process_group(0), so its pid is the pgid.
            unsafe { libc::killpg(pid as i32, libc::SIGKILL) };
        }
        #[cfg(not(unix))]
        let _ = pid;
    };
    let cancelled = async {
        match cancel {
            Some(cancel) => cancel.cancelled().await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        waited = tokio::time::timeout(watchdog, child.wait_with_output()) => match waited {
            Ok(Ok(output)) => Ok(ToolOutput {
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                code: output.status.code(),
            }),
            Ok(Err(error)) => Err(AnimateError::ToolFailed(format!(
                "waiting for '{program}' failed: {error}"
            ))),
            Err(_elapsed) => {
                kill_group();
                Err(AnimateError::Timeout(watchdog.as_secs()))
            }
        },
        () = cancelled => {
            kill_group();
            Err(AnimateError::Cancelled)
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
    fn bare_names_and_concrete_paths_are_classified_like_rodin() {
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
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, AnimateError::ToolMissing(_)), "{error:?}");
    }
}
