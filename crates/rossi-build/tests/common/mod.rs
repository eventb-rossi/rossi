//! Shared plumbing for the corpus integration harnesses (`animate_corpus`,
//! `rodin_corpus`). These are `#[ignore]` tests driven by an external Event-B
//! model corpus and external tools, so the helpers here all follow the same
//! conventions: locate things via environment variables, skip cleanly when
//! unset, and resolve relative paths from the workspace root.
//!
//! Not every test uses every helper, so dead code is expected here.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rossi_build::BuildResult;
use rossi_build::build;
use rossi_build::project::discover_projects;
use rossi_build::repack::repackage_zip_bytes_multi;

/// The workspace root (two levels up from this crate's manifest).
pub fn workspace_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
}

/// The workspace `target/` directory (shared build/output dir for reports).
pub fn workspace_target() -> PathBuf {
    workspace_root().join("target")
}

/// Read a path from an environment variable, resolving a relative value from
/// the workspace root. Returns `None` if the variable is unset.
pub fn env_path(var: &str) -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var(var).ok()?);
    Some(if path.is_absolute() {
        path
    } else {
        workspace_root().join(path)
    })
}

/// Resolve an executable: an absolute or path-bearing value is taken as-is
/// (relative resolved from the workspace root); a bare name is looked up on
/// `PATH`. Returns `None` if no executable file is found.
pub fn resolve_program(program: &str) -> Option<PathBuf> {
    let path = PathBuf::from(program);
    if path.is_absolute() || program.contains('/') || program.contains('\\') {
        let resolved = if path.is_absolute() {
            path
        } else {
            workspace_root().join(path)
        };
        return is_executable_file(&resolved).then_some(resolved);
    }

    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(program))
            .find(|candidate| is_executable_file(candidate))
    })
}

/// True when `path` is a regular file with an executable bit set (Unix); on
/// other platforms, any regular file is treated as executable.
pub fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    #[cfg(not(unix))]
    {
        true
    }
}

/// Regenerate a corpus model: read the source archive, static-check it with
/// rossi, and write a repackaged archive (original sources + our freshly
/// generated `.bcc`/`.bcm`, proof artifacts dropped) to `out`. This is the
/// "drop the build files and regenerate them with rossi" step shared by every
/// corpus harness.
pub fn regen_one(zip: &Path, out: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = std::fs::read(zip)?;
    let fallback = zip
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("project");
    // A corpus archive may bundle several top-level Rodin projects (Eclipse's
    // multi-project Archive export); build each under its own name and drop its
    // checked files back under its own directory.
    let builds: Vec<(String, BuildResult)> = discover_projects(&bytes, fallback)?
        .into_iter()
        .map(|dp| (dp.prefix.clone(), build(&dp.into_project())))
        .collect();
    let new_bytes = repackage_zip_bytes_multi(
        &bytes,
        builds.iter().map(|(prefix, r)| (prefix.as_str(), r)),
    )?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(out, new_bytes)?;
    Ok(())
}

/// Spawn `cmd` as the leader of a fresh process group (Unix). The corpus
/// tools are wrapper scripts whose real work happens in a spawned JVM or
/// container; on a timeout, `Child::kill` alone would reap the wrapper and
/// orphan that subprocess mid-build, leaving it grinding CPU (and, for a
/// containerised Rodin, rewriting the regen archive) long after the harness
/// moved on. Group leadership lets [`wait_with_timeout`] SIGKILL the whole
/// tree instead.
pub fn spawn_in_group(cmd: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn()
}

/// SIGKILL the child's whole process group. A no-op when the child was not
/// spawned via [`spawn_in_group`]: no process group carries its pid then, so
/// the signal has nothing to land on.
fn kill_group(child: &std::process::Child) {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &format!("-{}", child.id())])
            .status();
    }
    #[cfg(not(unix))]
    let _ = child;
}

/// Why a [`wait_with_timeout`] call stopped short of a clean exit.
pub enum WaitError {
    Timeout,
    Io(std::io::Error),
}

/// Wait for `child`, draining stdout/stderr on background threads, and kill it
/// if `timeout` elapses first. Returns the exit status plus the captured
/// stdout/stderr. (Used because `eventb-animate` has no timeout flag and Rodin
/// builds can hang on pathological models.)
pub fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: Duration,
) -> Result<(std::process::ExitStatus, String, String), WaitError> {
    use std::io::Read;
    use std::sync::mpsc;
    use std::thread;

    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let (tx_out, rx_out) = mpsc::channel();
    let (tx_err, rx_err) = mpsc::channel();
    thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        let _ = tx_out.send(s);
    });
    thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        let _ = tx_err.send(s);
    });

    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = rx_out.recv().unwrap_or_default();
                let err = rx_err.recv().unwrap_or_default();
                return Ok((status, out, err));
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    kill_group(&child);
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(WaitError::Timeout);
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(WaitError::Io(e)),
        }
    }
}

/// Load a reference TSV into a `model -> result` map. Works for both
/// `animate_results.tsv` and `checker_results.tsv`: column 0 is the model name
/// and column 2 is the outcome (`success`/… or `valid`/`invalid`). The header
/// row and blank lines are skipped.
pub fn load_expected(tsv: &Path) -> Option<BTreeMap<String, String>> {
    let s = std::fs::read_to_string(tsv).ok()?;
    let mut out = BTreeMap::new();
    for (i, line) in s.lines().enumerate() {
        if i == 0 || line.trim().is_empty() {
            continue;
        }
        let mut cols = line.split('\t');
        let model = cols.next()?.to_string();
        let _exit = cols.next()?;
        let result = cols.next()?.to_string();
        out.insert(model, result);
    }
    Some(out)
}

/// Column 4 of `animate_results.tsv`: the machine the reference outcome was
/// recorded with. `(auto)` rows are omitted (eventb-animate picks).
pub fn load_machines(tsv: &Path) -> Option<BTreeMap<String, String>> {
    let s = std::fs::read_to_string(tsv).ok()?;
    let mut out = BTreeMap::new();
    for (i, line) in s.lines().enumerate() {
        if i == 0 || line.trim().is_empty() {
            continue;
        }
        let mut cols = line.split('\t');
        let model = cols.next()?.to_string();
        let machine = cols.nth(2)?; // skip exit_code, result
        if machine != "(auto)" {
            out.insert(model, machine.to_string());
        }
    }
    Some(out)
}

/// The corpus `model_flags.tsv` (model, flag, notes; one row per model+flag),
/// loaded into a `model -> set of flags` map. Known flags: `defective`,
/// `unsupported`, `rodin_rejected`, `checker_divergence`, `nondeterministic`,
/// `lsp_suite`, `keyword_identifier` (declares a name the textual grammar
/// cannot express, e.g. a constant named `end` — see the `import_corpus`
/// harness).
pub fn load_flags(tsv: &Path) -> Option<BTreeMap<String, BTreeSet<String>>> {
    let s = std::fs::read_to_string(tsv).ok()?;
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (i, line) in s.lines().enumerate() {
        if i == 0 || line.trim().is_empty() {
            continue;
        }
        let mut cols = line.split('\t');
        let model = cols.next()?.to_string();
        let flag = cols.next()?.to_string();
        out.entry(model).or_default().insert(flag);
    }
    Some(out)
}

/// Locate the external Event-B model corpus, or `None` if `EVENTB_CORPUS_DIR`
/// is unset or does not point at a directory (skip-when-unset).
pub fn locate_corpus() -> Option<PathBuf> {
    env_path("EVENTB_CORPUS_DIR").filter(|p| p.is_dir())
}

/// One row of a corpus report: the `model` and its `expected`/`actual`
/// outcomes, the resulting `verdict`, and any `notes`. Shared by every corpus
/// harness; see [`write_report`] for the columnar layout.
pub struct Row {
    pub model: String,
    pub expected: String,
    pub actual: String,
    pub verdict: String,
    pub notes: String,
}

impl Row {
    /// The fields in report-column order, for [`write_report`].
    pub fn to_fields(&self) -> Vec<String> {
        vec![
            self.model.clone(),
            self.expected.clone(),
            self.actual.clone(),
            self.verdict.clone(),
            self.notes.clone(),
        ]
    }
}

/// Write a TSV report: a tab-joined `header` followed by one tab-joined line
/// per row, each field [`sanitize`]d so embedded tabs/newlines never break the
/// columnar layout. Creates the parent directory if needed.
pub fn write_report(path: &Path, header: &[&str], rows: &[Vec<String>]) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut f = match std::fs::File::create(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("could not write {}: {e}", path.display());
            return;
        }
    };
    let _ = writeln!(f, "{}", header.join("\t"));
    for row in rows {
        let line = row
            .iter()
            .map(|c| sanitize(c))
            .collect::<Vec<_>>()
            .join("\t");
        let _ = writeln!(f, "{line}");
    }
}

/// Pull a short error-ish line from captured tool output for a report: the
/// last line mentioning an error, skipping stack-trace frames. Shared by the
/// harnesses' outcome classifiers.
pub fn log_hint(combined: &str) -> String {
    combined
        .lines()
        .rev()
        .find(|l| {
            if l.trim_start().starts_with("at ") {
                return false;
            }
            let lc = l.to_lowercase();
            lc.contains("error") || lc.contains("exception") || lc.contains("failed")
        })
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Collapse embedded tabs/newlines (and runs of whitespace) to single spaces so
/// a value stays on one TSV cell — pest's multi-line parse errors are a common
/// source of leakage. `split_whitespace` already treats `\t`/`\n`/`\r` as
/// boundaries, so splitting on them and re-joining is all the collapsing needed.
pub fn sanitize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
