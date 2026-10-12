//! The project under the root, loaded on demand and cached.
//!
//! A load reads the source files, parses and checks them, generates the
//! proof obligations, and reconciles the generated proof files against
//! the previous build under the output directory, so stamps and statuses
//! carry across edits. The result is cached under a fingerprint of the
//! files the load read; a tool call that finds the fingerprint unchanged
//! reuses it, one that finds it changed loads again. Loads run on a
//! blocking thread and are serialized, so two concurrent calls never
//! build twice.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;

use rossi::ast::LineIndex;
use rossi::{Component, ParseError};
use rossi_build::pog::obligations::Obligations;
use rossi_build::pog::reconcile::{reconcile_build_files, reset_all_statuses};
use rossi_build::pog::sources::SourceIndex;
use rossi_build::pog::status::update_statuses;
use rossi_build::{BuildResult, Diagnostic, Project, ProjectComponent, RuleId, ScFile, Severity};

use crate::report::{
    ComponentRecord, DiagnosticCounts, DiagnosticRecord, EdgeRecord, Region, TextIndex,
};

/// What kind of sources the root holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// A folder of `.eventb` (or `.txt`) files.
    Text,
    /// A folder of `.bum`/`.buc` files: a Rodin project directory.
    Rodin,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Text => "text",
            SourceKind::Rodin => "rodin",
        }
    }
}

/// Which proof statuses a run of the model checker is written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Statuses {
    /// The statuses as generated: every obligation unattempted.
    Generated,
    /// The statuses reconciled with the previous build.
    Recorded,
}

/// What the root holds, as of one scan.
struct Scan {
    /// Identifies the files the load would read. Two scans with the same
    /// fingerprint are taken to be the same input.
    fingerprint: u64,
    kind: SourceKind,
    /// The source files, in a stable order.
    paths: Vec<PathBuf>,
}

/// One labelled invariant, with the renderings the model checker's
/// printed predicates are matched against.
#[derive(Debug, Clone)]
pub struct InvariantInfo {
    pub component: String,
    pub label: String,
    /// The spellings of the predicate the tool may print
    /// ([`rossi_build::normalize::predicate_renderings`]).
    pub renderings: Vec<String>,
}

impl eventb_animate_driver::report::DeclaredInvariant for InvariantInfo {
    fn component(&self) -> &str {
        &self.component
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn renderings(&self) -> &[String] {
        &self.renderings
    }
}

/// A project that parsed, with everything checking it produced.
///
/// These stand or fall together — there is no build without a project,
/// and no obligations or provenance without a build — so they are one
/// value rather than a row of options each caller has to re-correlate.
#[derive(Debug)]
pub struct Checked {
    pub project: Project,
    /// The build, its proof files reconciled against the output
    /// directory. The checked model itself is not kept: it is not
    /// shareable across threads, and everything taken from it (the
    /// well-definedness conditions, the provenance index) is taken at
    /// load.
    pub build: BuildResult,
    /// Where each obligation's provenance handles point.
    pub sources: SourceIndex,
    /// The generated obligations; absent only if the generated files
    /// cannot be read back.
    pub obligations: Option<Obligations>,
    pub lints: Vec<Diagnostic>,
    /// The well-definedness findings, reported on request.
    pub wd: Vec<Diagnostic>,
    /// Every labelled invariant of every machine.
    pub invariants: Vec<InvariantInfo>,
}

/// The project as last loaded.
#[derive(Debug)]
pub struct Loaded {
    pub fingerprint: u64,
    pub name: String,
    pub kind: SourceKind,
    pub output_dir: PathBuf,
    pub components: Vec<ComponentRecord>,
    pub edges: Vec<EdgeRecord>,
    /// Whole-file failures; when any, [`Loaded::checked`] is absent.
    pub parse_errors: Vec<DiagnosticRecord>,
    /// Absent when the model does not parse.
    pub checked: Option<Checked>,
    /// The source file and text of each component, by component name.
    texts: HashMap<String, (String, Option<String>)>,
}

impl Loaded {
    /// Where each component's text is, with a line index into it.
    ///
    /// Resolving a position rescans the source from its start, so a
    /// report with thousands of findings indexes each file once here
    /// rather than once per finding.
    pub fn text_index(&self) -> TextIndex<'_> {
        self.texts
            .iter()
            .map(|(component, (file, text))| {
                (
                    component.as_str(),
                    (file.as_str(), text.as_deref().map(LineIndex::new)),
                )
            })
            .collect()
    }

    /// Every finding of the load: parse failures, then the checker's
    /// diagnostics, then the lints, then (when asked) the
    /// well-definedness conditions.
    pub fn diagnostics(&self, include_wd: bool) -> Vec<DiagnosticRecord> {
        let index = self.text_index();
        let mut records = self.parse_errors.clone();
        records.extend(
            self.checker_diagnostics(include_wd)
                .map(|diagnostic| DiagnosticRecord::of_diagnostic(diagnostic, &index)),
        );
        records
    }

    /// How many findings of each severity the load has.
    ///
    /// Counted from the diagnostics themselves: a caller that wants only
    /// the totals should not pay for a record, a filename and a source
    /// position per finding.
    pub fn counts(&self, include_wd: bool) -> DiagnosticCounts {
        DiagnosticCounts::of(
            self.parse_errors
                .iter()
                .map(|record| record.severity)
                .chain(self.checker_diagnostics(include_wd).map(|d| d.severity)),
        )
    }

    /// The checker's findings, in report order.
    fn checker_diagnostics(&self, include_wd: bool) -> impl Iterator<Item = &Diagnostic> {
        let checked = self.checked.iter();
        checked.flat_map(move |checked| {
            checked
                .build
                .diagnostics
                .iter()
                .chain(checked.lints.iter())
                .chain(
                    checked
                        .wd
                        .iter()
                        .take(if include_wd { usize::MAX } else { 0 }),
                )
        })
    }

    /// The generated obligations, when the load has them.
    pub fn obligations(&self) -> Option<&Obligations> {
        self.checked.as_ref()?.obligations.as_ref()
    }

    /// Whether the model checks: it parsed, and the checker reported no
    /// error.
    pub fn checks(&self) -> Option<&Checked> {
        self.checked.as_ref().filter(|c| c.build.is_ok())
    }

    /// The build's files as a run of the model checker should see them.
    ///
    /// The disprover gate reads the recorded verdicts and skips what is
    /// already discharged, so it wants them. Every other run must not see
    /// them: the model checker takes a discharged invariant obligation as
    /// licence to skip re-checking that invariant after an event, and a
    /// check that trusts a proof is not the independent check the caller
    /// asked for. Resetting also keeps an unbounded run answering exactly
    /// as the bounded one, which is built from scratch.
    pub fn files_for(&self, statuses: Statuses) -> Vec<ScFile> {
        let Some(checked) = &self.checked else {
            return Vec::new();
        };
        let mut files = checked.build.files.clone();
        if statuses == Statuses::Generated {
            reset_all_statuses(&mut files);
        }
        files
    }
}

/// The root directory and its cached load.
pub struct Workspace {
    root: PathBuf,
    cache: Mutex<Option<Arc<Loaded>>>,
}

impl Workspace {
    pub fn new(root: PathBuf) -> Self {
        Workspace {
            root,
            cache: Mutex::new(None),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The project as of the files now under the root.
    ///
    /// The scan runs before the lock is taken, so two read-only calls do
    /// not queue behind each other; the fingerprint is then re-checked
    /// under the lock, which is what keeps two callers from building the
    /// same input twice.
    pub async fn load(&self) -> Result<Arc<Loaded>, String> {
        let root = self.root.clone();
        let scan = tokio::task::spawn_blocking(move || scan(&root))
            .await
            .map_err(|e| format!("the scan task failed: {e}"))??;
        let mut cache = self.cache.lock().await;
        if let Some(loaded) = cache.as_ref()
            && loaded.fingerprint == scan.fingerprint
        {
            return Ok(Arc::clone(loaded));
        }
        let root = self.root.clone();
        let loaded = tokio::task::spawn_blocking(move || Arc::new(Loaded::read(&root, scan)))
            .await
            .map_err(|e| format!("the build task failed: {e}"))?;
        *cache = Some(Arc::clone(&loaded));
        Ok(loaded)
    }
}

/// The project name every handle starts with, and the output directory's
/// name: the root's own name, made safe.
fn project_name(root: &Path) -> String {
    let base = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    rossi::descriptor_project_name(&rossi_build::workspace::sanitize_project_name(base)).to_string()
}

/// Where a build's files go.
pub fn output_dir(root: &Path) -> PathBuf {
    root.join(".rossi").join("build").join(project_name(root))
}

fn is_text_source(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("eventb") || ext.eq_ignore_ascii_case("txt"))
}

/// What the root and the previous build look like now.
///
/// The fingerprint is each file's path, length and modification time,
/// not its contents: a tool call runs this to decide whether the cached
/// load still stands, and the previous build's proof files alone reach
/// tens of megabytes, which is not worth re-reading and re-hashing
/// between two calls a second apart. An edit that preserves both the
/// length and the timestamp is therefore missed, which no editor or
/// agent writing a file produces.
fn scan(root: &Path) -> Result<Scan, String> {
    if !root.is_dir() {
        return Err(format!("not a directory: {}", root.display()));
    }
    let component_files = rossi_build::project::component_files(root)
        .map_err(|e| format!("cannot list {}: {e}", root.display()))?;
    let kind = if component_files
        .iter()
        .any(|path| rossi_build::project::is_xml_component(path))
    {
        SourceKind::Rodin
    } else {
        SourceKind::Text
    };

    let mut paths: Vec<PathBuf> = match kind {
        SourceKind::Rodin => component_files,
        SourceKind::Text => std::fs::read_dir(root)
            .map_err(|e| format!("cannot read {}: {e}", root.display()))?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file() && is_text_source(path))
            .collect(),
    };
    paths.sort();

    // The previous proof state shapes the reconciled files, so it is part
    // of what identifies a load.
    let mut previous: Vec<PathBuf> = std::fs::read_dir(output_dir(root))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| matches!(ext, "bpo" | "bps" | "bpr"))
        })
        .collect();
    previous.sort();

    let mut hasher = DefaultHasher::new();
    for path in paths.iter().chain(previous.iter()) {
        path.hash(&mut hasher);
        if let Ok(meta) = std::fs::metadata(path) {
            meta.len().hash(&mut hasher);
            if let Ok(modified) = meta.modified() {
                modified.hash(&mut hasher);
            }
        }
    }

    Ok(Scan {
        fingerprint: hasher.finish(),
        kind,
        paths,
    })
}

impl Loaded {
    fn read(root: &Path, scan: Scan) -> Loaded {
        let name = project_name(root);
        let mut loaded = Loaded {
            fingerprint: scan.fingerprint,
            name: name.clone(),
            kind: scan.kind,
            output_dir: output_dir(root),
            components: Vec::new(),
            edges: Vec::new(),
            parse_errors: Vec::new(),
            checked: None,
            texts: HashMap::new(),
        };

        // A Rodin project directory is read by the project loader itself;
        // only text sources are read here.
        let project = match scan.kind {
            SourceKind::Text => loaded.text_project(&name, &scan.paths, root),
            SourceKind::Rodin => loaded.rodin_project(root),
        };
        let Some(project) = project else {
            return loaded;
        };
        loaded.describe(&project);
        loaded.checked = Some(loaded.check(project));
        loaded
    }

    /// The components, their dependencies and their invariants, as the
    /// documents report them.
    fn describe(&mut self, project: &Project) {
        let mut components = Vec::new();
        let mut edges = Vec::new();
        for pc in &project.components {
            let component = &pc.component;
            components.push(ComponentRecord {
                name: component.name().to_string(),
                kind: match component {
                    Component::Context(_) => "context",
                    Component::Machine(_) => "machine",
                }
                .to_string(),
                file: self
                    .texts
                    .get(component.name())
                    .map(|(file, _)| file.clone())
                    .unwrap_or_else(|| pc.filename.clone()),
            });
            let mut edge = |from: &str, to: &str, kind: &str| {
                edges.push(EdgeRecord {
                    from: from.to_string(),
                    to: to.to_string(),
                    kind: kind.to_string(),
                });
            };
            match component {
                Component::Context(context) => {
                    for parent in &context.extends {
                        edge(&context.name, parent, "extends");
                    }
                }
                Component::Machine(machine) => {
                    if let Some(parent) = &machine.refines {
                        edge(&machine.name, parent, "refines");
                    }
                    for seen in &machine.sees {
                        edge(&machine.name, seen, "sees");
                    }
                }
            }
        }
        self.components = components;
        self.edges = edges;
    }

    /// Check the project, generate its obligations, and reconcile them
    /// with the previous build.
    fn check(&self, project: Project) -> Checked {
        let out = &self.output_dir;
        let (mut build, model) = rossi_build::build_with_model(&project);
        // The same reconciliation and status pass a `rossi build` into the
        // directory runs: previous state is what the output directory
        // holds, `.bpr` proofs there are never touched.
        let synthesized: HashMap<String, HashSet<String>> =
            reconcile_build_files(&mut build.files, |name| {
                std::fs::read_to_string(out.join(name)).ok()
            });
        update_statuses(&mut build.files, &synthesized, |name| {
            std::fs::read(out.join(name)).ok()
        });

        let invariants = project
            .components
            .iter()
            .filter_map(|pc| match &pc.component {
                Component::Machine(machine) => Some(machine),
                Component::Context(_) => None,
            })
            .flat_map(|machine| {
                machine.invariants.iter().filter_map(|invariant| {
                    Some(InvariantInfo {
                        component: machine.name.clone(),
                        label: invariant.label.clone()?,
                        renderings: rossi_build::normalize::predicate_renderings(
                            &invariant.predicate,
                        ),
                    })
                })
            })
            .collect();

        Checked {
            lints: rossi_build::lint::run(&project),
            wd: rossi_build::wd::run(&project, &model),
            obligations: Obligations::from_files(&build.files).ok(),
            sources: SourceIndex::from_model(&model),
            invariants,
            project,
            build,
        }
    }

    /// Parse the text sources into a project. Each component takes the
    /// filename a Rodin archive would give it, so the handles a build
    /// writes match those of an exported archive. A file that fails to
    /// parse becomes a finding and no project: a recovered tree drops
    /// elements, and checking it would report on a different model.
    fn text_project(&mut self, name: &str, paths: &[PathBuf], root: &Path) -> Option<Project> {
        let mut components = Vec::new();
        for path in paths {
            let file = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned();
            let text = match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(error) => {
                    self.parse_errors.push(DiagnosticRecord::for_file(
                        RuleId::CamilleParseError,
                        &file,
                        format!("cannot read: {error}"),
                        None,
                    ));
                    continue;
                }
            };
            match rossi::parse_components(&text) {
                Ok(parsed) => {
                    for component in parsed {
                        self.texts.insert(
                            component.name().to_string(),
                            (file.clone(), Some(text.clone())),
                        );
                        components.push(ProjectComponent::from_parsed(
                            rossi::component_filename(&component),
                            component,
                            Some(text.clone()),
                        ));
                    }
                }
                Err(error) => self
                    .parse_errors
                    .extend(parse_failures(&file, &text, &error)),
            }
        }
        if !self.parse_errors.is_empty() {
            return None;
        }
        if components.is_empty() {
            self.parse_errors.push(DiagnosticRecord::for_file(
                RuleId::CamilleParseError,
                "",
                "no Event-B components under the root".to_string(),
                None,
            ));
            return None;
        }
        Some(Project::new(name, components))
    }

    fn rodin_project(&mut self, root: &Path) -> Option<Project> {
        match Project::from_directory(root) {
            Ok(project) => {
                for pc in &project.components {
                    self.texts
                        .insert(pc.component.name().to_string(), (pc.filename.clone(), None));
                }
                Some(project)
            }
            Err(error) => {
                self.parse_errors.push(DiagnosticRecord::for_file(
                    RuleId::XmlParseError,
                    "",
                    error.to_string(),
                    None,
                ));
                None
            }
        }
    }
}

/// The findings for a text file that does not parse: the precise formula
/// failures the recovering parser can still name, else the file's first
/// syntax error as the reader reports it.
fn parse_failures(file: &str, text: &str, error: &ParseError) -> Vec<DiagnosticRecord> {
    let index = LineIndex::new(text);
    let recovered = rossi::parse_components_with_recovery(text);
    let precise: Vec<DiagnosticRecord> = recovered
        .errors
        .iter()
        .filter_map(ParseError::precise_formula_cause)
        .map(|(err, span)| {
            let region = span.map(|span| Region::of_span(&index, span)).or_else(|| {
                err.position()
                    .map(|(line, column)| Region::point(line, column))
            });
            DiagnosticRecord::for_file(rule_of(err), file, err.to_string(), region)
        })
        .collect();
    if !precise.is_empty() {
        return precise;
    }
    let region = error
        .position()
        .map(|(line, column)| Region::point(line, column));
    vec![DiagnosticRecord::for_file(
        rule_of(error),
        file,
        error.to_string(),
        region,
    )]
}

/// The rule a parse failure is reported under, the same one `rossi
/// validate` gives it, falling back to the whole-file Camille error.
fn rule_of(error: &ParseError) -> RuleId {
    RuleId::for_parse_error(error).unwrap_or(RuleId::CamilleParseError)
}

/// Write a build's files under the output directory.
pub fn write_build(loaded: &Loaded) -> Result<(), String> {
    let Some(checked) = &loaded.checked else {
        return Err("nothing to write: the project did not load".to_string());
    };
    std::fs::create_dir_all(&loaded.output_dir)
        .map_err(|e| format!("cannot create {}: {e}", loaded.output_dir.display()))?;
    rossi_build::write_sc_files(&loaded.output_dir, &checked.build.files)
        .map_err(|e| format!("cannot write into {}: {e}", loaded.output_dir.display()))
}

/// The obligations of a load by nature name, for the build report.
pub fn obligation_counts(loaded: &Loaded) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for po in loaded.obligations().into_iter().flat_map(Obligations::iter) {
        let key = po.nature.map_or_else(
            || po.description.to_string(),
            |nature| nature.name().to_string(),
        );
        *counts.entry(key).or_insert(0) += 1;
    }
    counts
}

/// The severity floor a report is filtered at.
pub fn severity_floor(name: Option<&str>, include_wd: bool) -> Result<Severity, String> {
    match name {
        // The well-definedness conditions are `info`, so asking for them
        // without naming a severity would otherwise filter every one of
        // them back out.
        None if include_wd => Ok(Severity::Info),
        None | Some("warning") => Ok(Severity::Warning),
        Some("error") => Ok(Severity::Error),
        Some("info") => Ok(Severity::Info),
        Some(other) => Err(format!(
            "unknown severity `{other}`: use error, warning or info"
        )),
    }
}
