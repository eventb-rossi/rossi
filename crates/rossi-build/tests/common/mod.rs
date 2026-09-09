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
/// rossi, and write a repackaged archive (original sources and `.bpr` proofs
/// plus our freshly generated `.bcc`/`.bcm` and reconciled `.bpo`/`.bps`) to
/// `out`. This is the "regenerate the build files with rossi" step shared by
/// every corpus harness.
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

/// The `.bpo` entries of a zip archive, keyed by entry path.
pub fn bpo_entries(zip_bytes: &[u8]) -> Result<BTreeMap<String, String>, String> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(|e| format!("zip: {e}"))?;
    let mut out = BTreeMap::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("zip: {e}"))?;
        if entry.name().ends_with(".bpo") {
            let mut contents = String::new();
            std::io::Read::read_to_string(&mut entry, &mut contents)
                .map_err(|e| format!("zip read: {e}"))?;
            out.insert(entry.name().to_string(), contents);
        }
    }
    Ok(out)
}

/// The obligations a regenerated archive carries, as
/// `(project prefix, component, sequent)` — the set an external consumer of
/// the generated `.bpo` should read back. The prefix is `"Project/"`, or
/// empty for a flat archive, so a caller that names obligations by component
/// alone can drop it and one that qualifies them can concatenate.
///
/// The component is kept separate from the prefix rather than folded into a
/// single name: a multi-project archive routinely repeats component names
/// across projects, and flattening them loses every obligation that collides.
pub fn generated_obligations(zip: &Path) -> Result<BTreeSet<(String, String, String)>, String> {
    let bytes = std::fs::read(zip).map_err(|e| format!("read: {e}"))?;
    let mut out = BTreeSet::new();
    for (path, contents) in bpo_entries(&bytes)? {
        let stem = path.trim_end_matches(".bpo");
        let (prefix, component) = match stem.rsplit_once('/') {
            Some((dir, name)) => (format!("{dir}/"), name),
            None => (String::new(), stem),
        };
        let view = rossi_build::po_view::PoView::from_xml(&contents)
            .map_err(|e| format!("{path}: {e}"))?;
        for sequent in view.sequents.keys() {
            out.insert((prefix.clone(), component.to_string(), sequent.clone()));
        }
    }
    Ok(out)
}

/// Whether `model` carries a flag marking it as input the corpus gates cannot
/// hold to the same standard: broken sources, models needing an Event-B
/// extension rossi does not support, and models no toolchain accepts.
pub fn flagged_unsupported(flags: &BTreeMap<String, BTreeSet<String>>, model: &str) -> bool {
    flags.get(model).is_some_and(|f| {
        f.iter().any(|flag| {
            matches!(
                flag.as_str(),
                "defective" | "keyword_identifier" | "unsupported" | "rodin_rejected"
            )
        })
    })
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

/// The corpus directory: `EVENTB_CORPUS_DIR`, else the conventional
/// `eventb-models-collection` sibling checkout.
pub fn corpus_dir() -> Option<PathBuf> {
    locate_corpus().or_else(|| {
        let sibling = workspace_root().join("../eventb-models-collection");
        sibling.is_dir().then_some(sibling)
    })
}

/// Parse raw `.buc`/`.bum` XML into a project component.
pub fn xml(filename: &str, body: &str) -> rossi_build::ProjectComponent {
    rossi_build::ProjectComponent::from_xml(filename, body).unwrap()
}

/// Build the components and generate their proof-obligation files,
/// asserting the build itself is clean.
pub fn generate(
    name: &str,
    components: Vec<rossi_build::ProjectComponent>,
) -> Vec<rossi_build::ScFile> {
    let project = rossi_build::Project::new(name, components);
    let (build, model) = rossi_build::build_with_model(&project);
    assert!(build.is_ok(), "build diagnostics: {:?}", build.diagnostics);
    rossi_build::pog::generate(&project, &model)
}

/// The generated file named `name`.
pub fn find<'a>(files: &'a [rossi_build::ScFile], name: &str) -> &'a rossi_build::ScFile {
    files.iter().find(|f| f.filename == name).unwrap()
}

/// The `eventb-checker` command to run: `EVENTB_CHECKER` if set, else the CLI
/// resolved from `PATH`. May be a wrapper (e.g. `java -jar …`) exposed as a
/// single executable.
pub fn eventb_checker_bin() -> String {
    std::env::var("EVENTB_CHECKER").unwrap_or_else(|_| "eventb-checker".to_string())
}

/// Whether the oracle CLI is runnable (`<oracle> --version` succeeds).
pub fn oracle_available(oracle: &str) -> bool {
    std::process::Command::new(oracle)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The `.zip` corpus models in `dir`, sorted for deterministic iteration.
pub fn collect_zips(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut zips: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "zip"))
        .collect();
    zips.sort();
    Ok(zips)
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

/// Models whose reference or freshly-regenerated proof obligations are
/// known not to be reproducible: rows carrying the `pog_divergence`
/// flag in the corpus `model_flags.tsv`, with the audited reason in the
/// notes column. Shared by the proof-obligation gates.
pub fn pog_known_divergence(corpus: &Path, model: &str) -> Option<String> {
    known_divergence(corpus, model, "pog_divergence")
}

/// The audited reason a model carries `flag` in the corpus
/// `model_flags.tsv`, if it does.
fn known_divergence(corpus: &Path, model: &str, flag: &str) -> Option<String> {
    let tsv = std::fs::read_to_string(corpus.join("model_flags.tsv")).ok()?;
    for line in tsv.lines().skip(1) {
        let mut cols = line.split('\t');
        if cols.next() == Some(model) && cols.next() == Some(flag) {
            return Some(cols.next().unwrap_or("").to_string());
        }
    }
    None
}

/// Models whose recorded proof statuses are known not to be
/// reproducible by the proof-reuse harness: rows carrying the
/// `prove_divergence` flag in the corpus `model_flags.tsv`, with the
/// audited reason.
pub fn prove_known_divergence(corpus: &Path, model: &str) -> Option<String> {
    known_divergence(corpus, model, "prove_divergence")
}

/// Compare a reference proof-obligation view against a generated one,
/// appending findings. The comparison is semantic: sequent name sets,
/// natures, accuracy, goals, flattened hypotheses and identifiers,
/// sources as sets (their order varied across generator versions), and
/// hints resolved to the content they select.
pub fn diff_po_views(
    file: &str,
    reference: &rossi_build::po_view::PoView,
    ours: &rossi_build::po_view::PoView,
    max_problems: usize,
    problems: &mut Vec<String>,
) {
    for name in reference.sequents.keys() {
        if !ours.sequents.contains_key(name) {
            problems.push(format!("{file}: missing sequent {name}"));
        }
    }
    for name in ours.sequents.keys() {
        if !reference.sequents.contains_key(name) {
            problems.push(format!("{file}: extra sequent {name}"));
        }
    }

    for (name, theirs) in &reference.sequents {
        let Some(mine) = ours.sequents.get(name) else {
            continue;
        };
        if theirs.description != mine.description {
            problems.push(format!(
                "{file}: {name}: nature {:?} vs {:?}",
                theirs.description, mine.description
            ));
        }
        if theirs.accurate != mine.accurate {
            problems.push(format!(
                "{file}: {name}: accurate {} vs {}",
                theirs.accurate, mine.accurate
            ));
        }
        if theirs.goal != mine.goal {
            problems.push(format!("{file}: {name}: goal differs"));
        }
        let their_hyps = reference.flattened_hypotheses(name);
        let my_hyps = ours.flattened_hypotheses(name);
        if their_hyps != my_hyps {
            problems.push(format!(
                "{file}: {name}: hypotheses differ ({} vs {})",
                their_hyps.len(),
                my_hyps.len()
            ));
        }
        if reference.flattened_identifiers(name) != ours.flattened_identifiers(name) {
            problems.push(format!("{file}: {name}: identifiers differ"));
        }
        let their_sources: std::collections::BTreeSet<_> = theirs.sources.iter().collect();
        let my_sources: std::collections::BTreeSet<_> = mine.sources.iter().collect();
        if their_sources != my_sources {
            problems.push(format!("{file}: {name}: sources differ"));
        }
        if reference.resolved_hints(name) != ours.resolved_hints(name) {
            problems.push(format!("{file}: {name}: hints differ"));
        }
        if problems.len() >= max_problems {
            return;
        }
    }
}

/// One corpus archive's proof-obligation files, grouped one
/// [`rossi_prove::PoProject`] per archive directory — hypothesis-set
/// chains cross component files within a project and resolve by file
/// basename (archives are routinely renamed, so handle paths keep the
/// original project name). Legacy-vintage files (`GOAL` child naming)
/// and unreadable ones are reported rather than loaded, so each gate
/// keeps its own accounting for them.
pub struct PoArchive {
    archive: zip::ZipArchive<std::fs::File>,
    /// The per-directory projects the checked stems load from.
    pub projects: BTreeMap<String, rossi_prove::PoProject>,
    /// Component stems (entry names without extension) whose
    /// obligations parsed, in sorted order.
    pub checked: Vec<String>,
    /// Legacy-vintage stems with their sequent counts.
    pub legacy: Vec<(String, usize)>,
    /// Stems whose `.bpo` did not read or parse, with the reason.
    pub unreadable: Vec<(String, String)>,
}

/// Splits a component stem into its archive directory (with trailing
/// slash, empty for the root) and basename.
pub fn stem_parts(stem: &str) -> (&str, &str) {
    stem.rsplit_once('/').unwrap_or(("", stem))
}

impl PoArchive {
    /// Loads every `.bpo` of the archive at `path`.
    pub fn load(path: &Path) -> Result<PoArchive, String> {
        let file = std::fs::File::open(path).map_err(|err| err.to_string())?;
        let mut archive = zip::ZipArchive::new(file).map_err(|err| err.to_string())?;
        let mut stems: Vec<String> = archive
            .file_names()
            .filter(|name| name.ends_with(".bpo"))
            .map(|name| name.trim_end_matches(".bpo").to_string())
            .collect();
        stems.sort();

        let mut projects: BTreeMap<String, rossi_prove::PoProject> = BTreeMap::new();
        let mut checked = Vec::new();
        let mut legacy = Vec::new();
        let mut unreadable = Vec::new();
        for stem in stems {
            let (dir, file) = stem_parts(&stem);
            let mut bytes = Vec::new();
            let readable = archive
                .by_name(&format!("{stem}.bpo"))
                .ok()
                .and_then(|mut entry| std::io::Read::read_to_end(&mut entry, &mut bytes).ok())
                .is_some();
            if !readable {
                unreadable.push((stem, "unreadable zip entry".to_string()));
                continue;
            }
            let bpo = String::from_utf8_lossy(&bytes).into_owned();
            // Legacy obligation vintage: files predating the current
            // child-naming scheme are out of scope wholesale.
            if bpo.contains("name=\"GOAL\"") {
                let sequents = bpo.matches("<org.eventb.core.poSequent ").count();
                legacy.push((stem, sequents));
                continue;
            }
            match rossi_prove::PoFile::read(bpo.as_bytes()) {
                Ok(parsed) => {
                    projects
                        .entry(dir.to_string())
                        .or_default()
                        .insert(format!("{file}.bpo"), parsed);
                    checked.push(stem);
                }
                Err(err) => unreadable.push((stem, err.to_string())),
            }
        }
        Ok(PoArchive {
            archive,
            projects,
            checked,
            legacy,
            unreadable,
        })
    }

    /// The raw bytes of one archive entry, `None` when absent or
    /// unreadable.
    pub fn entry(&mut self, name: &str) -> Option<Vec<u8>> {
        use std::io::Read;
        let mut bytes = Vec::new();
        self.archive
            .by_name(name)
            .ok()?
            .read_to_end(&mut bytes)
            .ok()?;
        Some(bytes)
    }
}

/// A model an oracle gate cannot judge vs. a real divergence.
pub enum FailOrSkip {
    Skip(String),
    Fail(String),
}

// --- dump document fixtures ---

/// The text of an example model, resolved from the workspace root so it does
/// not depend on the working directory.
pub fn example(name: &str) -> String {
    let path = workspace_root().join("crates/rossi/examples").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// A project built from Event-B text under the Rodin names the dump command
/// gives its components, so handles match what a build writes.
pub fn text_project(name: &str, files: &[(&str, &str)]) -> rossi_build::project::Project {
    use rossi_build::project::{Project, ProjectComponent};
    let mut components = Vec::new();
    for (rodin_name, text) in files {
        components.extend(
            ProjectComponent::from_eventb(*rodin_name, text)
                .unwrap_or_else(|e| panic!("{rodin_name} parses: {e}")),
        );
    }
    Project::new(name, components)
}

/// A project assembled from example files on disk.
pub fn example_project(name: &str, files: &[(&str, &str)]) -> rossi_build::project::Project {
    let loaded: Vec<(&str, String)> = files
        .iter()
        .map(|(rodin_name, example_name)| (*rodin_name, example(example_name)))
        .collect();
    let borrowed: Vec<(&str, &str)> = loaded
        .iter()
        .map(|(rodin_name, text)| (*rodin_name, text.as_str()))
        .collect();
    text_project(name, &borrowed)
}

/// The example models the dump tests share, as the Rodin name each component
/// takes and the example file it comes from.
pub const DUMP_MODELS: &[(&str, &[(&str, &str)])] = &[
    (
        "bank_account",
        &[
            ("bank_account_ctx.buc", "bank_account_ctx.eventb"),
            ("bank_account.bum", "bank_account_machine.eventb"),
        ],
    ),
    (
        "refinement",
        &[
            ("refinement_abstract.bum", "refinement_abstract.eventb"),
            ("refinement_concrete.bum", "refinement_concrete.eventb"),
        ],
    ),
];
