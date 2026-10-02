//! Shared proof-state handling for the commands that touch `.bpr` /
//! `.bps` / `.bpo` files: where `rossi export --proofs` and `rossi
//! import` read local proof state from, and the verdict `rossi prove`
//! and `rossi clean` both judge a stored proof by.
//!
//! Proof state is the trio Rodin keeps next to a component: `.bpr` (the
//! proofs), `.bps` (their statuses), and `.bpo` (the obligations they were
//! recorded against). All three are collected byte-exact as
//! `(basename, bytes)` pairs, sorted by basename; what happens to them
//! afterwards (byte-exact passthrough for `.bpr`, reconcile baselines for
//! `.bpo`/`.bps`) is the repack pipeline's business, not this module's.
//!
//! Extensions are matched lowercase-exact, mirroring `rossi-build`'s repack
//! (`keep_input_entry`) — a file the repack pipeline would not treat as proof
//! state must not be collected as such here.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use rossi_build::is_normal_path_component;
use rossi_prove::bpr::{ProofBody, ProofEntry};
use rossi_prove::confidence::Bucket;
use rossi_prove::po_loader::{PoFile, PoProject};
use rossi_prove::status::compute_status;
use rossi_prove::{Confidence, ProverSequent};

use super::eventb_io::{CmdResult, is_zip_ext};

/// The `(component stem, contents)` of one kind of proof-state file,
/// stems keeping their archive directory prefix.
pub(crate) type FileMap = BTreeMap<String, Vec<u8>>;

/// A project's proof state, split by kind.
#[derive(Default)]
pub(crate) struct ProofFiles {
    /// Generated obligations.
    pub(crate) bpo: FileMap,
    /// The proofs themselves.
    pub(crate) bpr: FileMap,
    /// Per-obligation statuses.
    pub(crate) bps: FileMap,
}

/// Collects the proof state of a `.zip` archive or a project
/// directory, keyed by component stem.
///
/// Callers decide which kinds they need and which absence is fatal —
/// `rossi prove` cannot work without obligations, `rossi clean`
/// without proofs — so this reports no emptiness of its own.
pub(crate) fn collect_proof_files(input: &Path) -> CmdResult<ProofFiles> {
    let mut files = ProofFiles::default();
    let mut insert = |name: &str, bytes: Vec<u8>| {
        if let Some((stem, ext)) = name.rsplit_once('.') {
            let map = match ext {
                "bpo" => &mut files.bpo,
                "bpr" => &mut files.bpr,
                "bps" => &mut files.bps,
                _ => return,
            };
            map.insert(stem.to_string(), bytes);
        }
    };
    if input.is_dir() {
        for (basename, bytes) in proofs_in_dir(input)? {
            insert(&basename, bytes);
        }
    } else {
        let bytes = fs::read(input)?;
        visit_zip_proofs(
            &bytes,
            |_| true,
            |name, bytes| {
                insert(name, bytes);
                Ok(())
            },
        )?;
    }
    Ok(files)
}

/// Parses the obligation files into one project per archive
/// directory, because hypothesis-set chains cross component files and
/// resolve by file basename.
///
/// Parsing is the expensive half of reading a project, so it runs on
/// the shared pool.
pub(crate) fn load_projects(bpos: &FileMap) -> CmdResult<BTreeMap<&str, PoProject>> {
    let parsed: Vec<Result<PoFile, String>> = rossi_prove::thread_pool().install(|| {
        bpos.par_iter()
            .map(|(stem, contents)| {
                PoFile::read(contents.as_slice()).map_err(|err| format!("{stem}.bpo: {err}"))
            })
            .collect()
    });
    let mut projects: BTreeMap<&str, PoProject> = BTreeMap::new();
    for (stem, parsed) in bpos.keys().zip(parsed) {
        let (dir, file) = split_stem(stem);
        projects
            .entry(dir)
            .or_default()
            .insert(format!("{file}.bpo"), parsed?);
    }
    Ok(projects)
}

/// A component stem split into its archive directory and basename.
pub(crate) fn split_stem(stem: &str) -> (&str, &str) {
    stem.rsplit_once('/').unwrap_or(("", stem))
}

/// One obligation's verdict, recomputed from its stored proof rather
/// than read out of the `.bps` cache.
///
/// The cached status is exactly what cannot be trusted here: a proof
/// Rodin can no longer open still carries whatever confidence it had
/// when it last worked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProofStatus {
    /// The proof cannot be represented at all — an old storage
    /// vintage, an extended mathematical language, or content that
    /// does not parse.
    Unsupported,
    /// The obligation itself could not be loaded from its `.bpo`.
    Error,
    /// The stored proof no longer applies to its obligation.
    Broken,
    /// The proof stands, at the recorded confidence.
    Checked(Bucket),
}

impl ProofStatus {
    /// The word `rossi prove` and `rossi clean` print.
    pub(crate) fn label(self) -> &'static str {
        match self {
            ProofStatus::Unsupported => "unsupported",
            ProofStatus::Error => "error",
            ProofStatus::Broken => "broken",
            ProofStatus::Checked(bucket) => bucket.as_str(),
        }
    }

    /// Whether the stored proof is past use, so emptying it loses
    /// nothing.
    ///
    /// Only a broken proof qualifies. [`ProofStatus::Unsupported`]
    /// deliberately does not: it means *rossi* cannot represent the
    /// proof, not that the proof is bad — most of them are an older
    /// storage vintage Rodin reads perfectly well (23% of the models
    /// collection's 68k stored proofs, almost all `old_vintage`).
    /// Emptying those would destroy working proofs over a gap in this
    /// reader. [`ProofStatus::Error`] is out for the same kind of
    /// reason: the fault is then in the `.bpo`, and discarding the
    /// proof would not mend it.
    pub(crate) fn is_stale(self) -> bool {
        matches!(self, ProofStatus::Broken)
    }
}

/// A status serializes as the word [`ProofStatus::label`] prints, so
/// the JSON and the human report never disagree.
impl serde::Serialize for ProofStatus {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.label())
    }
}

/// A stored proof judged against its obligation.
pub(crate) struct Classified {
    pub(crate) status: ProofStatus,
    /// The sequent the verdict was computed against, when the
    /// obligation loaded — so a caller that goes on to replay the
    /// proof does not load it a second time.
    pub(crate) sequent: Option<ProverSequent>,
}

/// Judges one stored proof against the obligation of the same name in
/// `path`'s `.bpo`.
///
/// The single source of truth for what "broken" means: `rossi prove`
/// reports it and `rossi clean --broken` acts on it, and the two must
/// never disagree about which proofs are past use.
pub(crate) fn classify(project: &PoProject, path: &str, proof: &ProofEntry) -> Classified {
    match &proof.body {
        ProofBody::Skipped => unreachable!("classifying a proof that was not read"),
        ProofBody::Unsupported(_) => {
            return Classified {
                status: ProofStatus::Unsupported,
                sequent: None,
            };
        }
        ProofBody::Loaded(_) => {}
    }
    let Ok(sequent) = project.load(path, &proof.name) else {
        return Classified {
            status: ProofStatus::Error,
            sequent: None,
        };
    };
    let verdict = compute_status(&sequent, proof);
    let status = if verdict.broken {
        ProofStatus::Broken
    } else {
        ProofStatus::Checked(Confidence::classify(verdict.confidence.map(i64::from)))
    };
    Classified {
        status,
        sequent: Some(sequent),
    }
}

/// Whether `name` (a basename or archive entry name) is a proof-state file.
fn is_proof_file_name(name: &str) -> bool {
    name.ends_with(".bpr") || name.ends_with(".bps") || name.ends_with(".bpo")
}

/// Where `--proofs` reads local proof state from.
pub(crate) enum ProofSource {
    /// Bare `--proofs`: the directories next to the text inputs, then the
    /// LSP's shared Rodin workspace (`<root>/.rossi/rodin/<project>`).
    Local,
    /// `--proofs=DIR`: a Rodin project directory (or, for a multi-project
    /// export, a directory of `<project>/` subdirectories).
    Dir(PathBuf),
    /// `--proofs=FILE.zip`: a Rodin archive, read once up front.
    Zip(Vec<u8>),
}

impl ProofSource {
    /// Open the source behind `--proofs[=PATH]`. `None` is the bare form.
    pub(crate) fn open(path: Option<&Path>) -> CmdResult<Self> {
        let Some(path) = path else {
            return Ok(ProofSource::Local);
        };
        if path.is_dir() {
            return Ok(ProofSource::Dir(path.to_path_buf()));
        }
        if path.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(is_zip_ext)
        {
            return Ok(ProofSource::Zip(fs::read(path)?));
        }
        Err(format!(
            "--proofs path must be a directory or a .zip archive: {}",
            path.display()
        )
        .into())
    }

    /// Collect the proof files for one exported project.
    ///
    /// `project_name` is `Some` for a sub-project of a multi-project export
    /// (scoping an explicit source to `PATH/<name>/` or `<name>/…` entries)
    /// and `None` for a flat export. `local_dirs` are the directories the
    /// project's text inputs live in — only the bare [`ProofSource::Local`]
    /// form reads them.
    pub(crate) fn for_project(
        &self,
        project_name: Option<&str>,
        local_dirs: &[PathBuf],
    ) -> CmdResult<Vec<(String, Vec<u8>)>> {
        // First-wins insertion, so precedence is expressed purely by
        // collection order.
        let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut absorb = |entries: Vec<(String, Vec<u8>)>| {
            for (basename, bytes) in entries {
                files.entry(basename).or_insert(bytes);
            }
        };
        match self {
            ProofSource::Local => {
                // Next-to-inputs wins over the workspace. A custom
                // `rossi.rodin.workspace` setting lives in editor
                // configuration and is not discoverable here —
                // `--proofs=PATH` covers those setups.
                for dir in local_dirs {
                    absorb(proofs_in_dir(dir)?);
                }
                for dir in local_dirs {
                    if let Some(project_dir) = rossi_build::workspace::workspace_project_dir(dir) {
                        absorb(proofs_in_dir(&project_dir)?);
                    }
                }
            }
            ProofSource::Dir(root) => {
                // A sub-project with no matching directory simply has no
                // proofs to carry.
                let dir = match project_name {
                    Some(name) => root.join(name),
                    None => root.clone(),
                };
                absorb(proofs_in_dir(&dir)?);
            }
            ProofSource::Zip(bytes) => absorb(match project_name {
                Some(name) => zip_proofs_at_prefix(bytes, &format!("{name}/"))?,
                None => zip_proofs_any_prefix(bytes)?,
            }),
        }
        Ok(files.into_iter().collect())
    }
}

/// The proof files at `dir`'s top level (non-recursive, sorted, files only) —
/// the same shallow posture `rossi build` uses for a project directory's
/// `.bpr` files. A missing directory contributes nothing.
pub(crate) fn proofs_in_dir(dir: &Path) -> CmdResult<Vec<(String, Vec<u8>)>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut paths: Vec<PathBuf> = entries
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_proof_file_name)
                && path.is_file()
        })
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let basename = path
                .file_name()
                .and_then(|n| n.to_str())
                .expect("filtered on a UTF-8 file name")
                .to_string();
            Ok((basename, fs::read(&path)?))
        })
        .collect()
}

/// The proof entries sitting directly under `prefix` (`"Name/"`, or `""` for
/// a flat archive) as `(basename, bytes)` pairs. Nested entries and unsafe
/// basenames from the (untrusted) archive are skipped.
pub(crate) fn zip_proofs_at_prefix(
    zip_bytes: &[u8],
    prefix: &str,
) -> CmdResult<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    visit_zip_proofs(
        zip_bytes,
        |name| {
            name.strip_prefix(prefix)
                .is_some_and(|basename| !basename.contains('/'))
        },
        |name, bytes| {
            let basename = name.strip_prefix(prefix).expect("gated on the prefix");
            out.push((basename.to_string(), bytes));
            Ok(())
        },
    )?;
    Ok(out)
}

/// Collect proof entries from any prefix of the archive, keyed by basename —
/// the flat-export reading of a `--proofs=FILE.zip` source. The same basename
/// under two prefixes is fine when the bytes agree and an error when they
/// differ (there is no way to pick for a single flat project).
fn zip_proofs_any_prefix(zip_bytes: &[u8]) -> CmdResult<Vec<(String, Vec<u8>)>> {
    let mut seen: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    visit_zip_proofs(
        zip_bytes,
        |_| true,
        |name, bytes| {
            let basename = name.rsplit_once('/').map_or(name, |(_, b)| b).to_string();
            match seen.get(&basename) {
                Some(previous) if *previous != bytes => Err(format!(
                    "--proofs archive holds conflicting copies of {basename}; \
                     export one project at a time or point --proofs at a project directory"
                )
                .into()),
                Some(_) => Ok(()),
                None => {
                    seen.insert(basename, bytes);
                    Ok(())
                }
            }
        },
    )?;
    Ok(seen.into_iter().collect())
}

/// Walk a zip's proof-file entries, handing each safe `(entry name, bytes)`
/// to `visit`. Directory entries, non-proof extensions, and unsafe basenames
/// are skipped, and an entry `want` rejects is skipped **before** its bytes
/// are inflated — so a caller filtering by prefix never pays to decompress
/// the other projects' proofs.
pub(crate) fn visit_zip_proofs(
    zip_bytes: &[u8],
    mut want: impl FnMut(&str) -> bool,
    mut visit: impl FnMut(&str, Vec<u8>) -> CmdResult<()>,
) -> CmdResult<()> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        if !is_proof_file_name(&name) {
            continue;
        }
        let basename = name.rsplit_once('/').map_or(name.as_str(), |(_, b)| b);
        if !is_normal_path_component(basename) {
            continue;
        }
        if !want(&name) {
            continue;
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes)?;
        visit(&name, bytes)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RAII temp dir; the unit-test analogue of the CLI tests' helper.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(prefix: &str) -> Self {
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("{prefix}-{nanos}-{seq}"));
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn local_source_prefers_input_dir_over_workspace() {
        let tmp = TempDir::new("rossi-proofs-precedence");
        let root = &tmp.0;
        let model = root.join("model");
        fs::create_dir_all(&model).unwrap();
        fs::write(model.join("M.bpr"), b"local").unwrap();
        fs::write(model.join("N.bpr"), b"local-only").unwrap();

        let project = root.join(".rossi").join("rodin").join("model");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("M.bpr"), b"workspace").unwrap();
        fs::write(project.join("M.bps"), b"workspace-only").unwrap();

        let files = ProofSource::Local
            .for_project(None, &[model])
            .expect("local resolution");
        assert_eq!(
            files,
            vec![
                ("M.bpr".to_string(), b"local".to_vec()),
                ("M.bps".to_string(), b"workspace-only".to_vec()),
                ("N.bpr".to_string(), b"local-only".to_vec()),
            ]
        );
    }

    #[test]
    fn dir_source_scopes_sub_projects_by_name() {
        let tmp = TempDir::new("rossi-proofs-dir-scope");
        let root = &tmp.0;
        fs::create_dir_all(root.join("A")).unwrap();
        fs::write(root.join("A").join("M.bpr"), b"a-proof").unwrap();
        fs::write(root.join("M.bpr"), b"flat-proof").unwrap();

        let source = ProofSource::Dir(root.clone());
        let sub = source.for_project(Some("A"), &[]).unwrap();
        assert_eq!(sub[0].1, b"a-proof");
        let flat = source.for_project(None, &[]).unwrap();
        assert_eq!(flat[0].1, b"flat-proof");
        let missing = source.for_project(Some("B"), &[]).unwrap();
        assert!(missing.is_empty());
    }
}
