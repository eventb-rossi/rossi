//! The project under the root, loaded on demand and cached by content.
//!
//! A load reads the source files, parses and checks them, generates the
//! proof obligations, and reconciles the generated proof files against
//! the previous build under the output directory, so stamps and statuses
//! carry across edits. The result is cached under a digest of every file
//! the load read; a tool call that finds the digest unchanged reuses it,
//! one that finds it changed loads again. Loads run on a blocking thread
//! and are serialized, so two concurrent calls never build twice.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;

use rossi::{Component, ParseError};
use rossi_build::pog::obligations::Obligations;
use rossi_build::pog::reconcile::reconcile_build_files;
use rossi_build::pog::sources::SourceIndex;
use rossi_build::pog::status::update_statuses;
use rossi_build::{BuildResult, Diagnostic, Project, ProjectComponent, RuleId};

use crate::report::{ComponentRecord, DiagnosticRecord, EdgeRecord, Region};

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

/// One source file read from the root.
#[derive(Debug, Clone)]
struct SourceFile {
    /// Relative to the root.
    path: String,
    text: String,
}

/// The files a load reads, and the digest that identifies them.
struct Scan {
    digest: u64,
    kind: SourceKind,
    sources: Vec<SourceFile>,
}

/// One labelled invariant, with the renderings the model checker's
/// printed predicates are matched against.
#[derive(Debug, Clone)]
pub struct InvariantInfo {
    pub component: String,
    pub label: String,
    /// Whitespace-stripped renderings (canonical and ASCII) of the
    /// predicate; the tool prints the checked file's predicate code,
    /// which the build derives from the same tree.
    pub renderings: Vec<String>,
}

/// The project as last loaded.
#[derive(Debug)]
pub struct Loaded {
    pub digest: u64,
    pub name: String,
    pub kind: SourceKind,
    pub output_dir: PathBuf,
    pub components: Vec<ComponentRecord>,
    pub edges: Vec<EdgeRecord>,
    /// Whole-file failures; when any, `project` is absent.
    pub parse_errors: Vec<DiagnosticRecord>,
    pub project: Option<Project>,
    /// The build with its proof files reconciled against the output
    /// directory. The checked model itself is not kept: it is not
    /// shareable across threads, and every answer that needs it (the
    /// well-definedness conditions, the source index) is taken at load.
    pub build: Option<BuildResult>,
    pub lints: Vec<Diagnostic>,
    /// The well-definedness findings, reported on request.
    pub wd: Vec<Diagnostic>,
    pub obligations: Option<Obligations>,
    pub sources: Option<SourceIndex>,
    /// Every labelled invariant of every machine.
    pub invariants: Vec<InvariantInfo>,
    /// The status files as generated, before reconciliation carried the
    /// previous build's verdicts into them: one all-unattempted `.bps`
    /// per component, by filename. See [`Loaded::files_for`].
    fresh_statuses: HashMap<String, String>,
    /// The source file and text of each component, by component name.
    files_of: HashMap<String, (String, Option<String>)>,
}

impl Loaded {
    /// The source file and text a component came from.
    pub fn source_of(&self, component: &str) -> Option<(String, Option<String>)> {
        self.files_of.get(component).cloned()
    }

    /// The build's files as a run of the model checker should see them.
    ///
    /// The disprover gate reads the recorded verdicts and skips what is
    /// already discharged, so it wants the reconciled statuses. Every
    /// other run must not see them: the model checker takes a discharged
    /// invariant obligation as licence to skip re-checking that invariant
    /// after an event, and a check that trusts a proof is not the
    /// independent check the caller asked for. Restoring the generated
    /// statuses also keeps an unbounded run answering exactly as the
    /// bounded one, which is built from scratch.
    ///
    /// A restored status file carries the stamp it was generated with
    /// while its obligations keep the reconciled one, so the pair reads
    /// as stale. Nothing here minds: an unattempted row grants the model
    /// checker nothing to skip whatever its stamp, the gate that does
    /// read stamps is the one getting the recorded statuses, and the
    /// files live only as long as the run.
    pub fn files_for(&self, recorded_status: bool) -> Vec<rossi_build::ScFile> {
        let Some(build) = &self.build else {
            return Vec::new();
        };
        build
            .files
            .iter()
            .map(|file| match self.fresh_statuses.get(&file.filename) {
                Some(fresh) if !recorded_status => rossi_build::ScFile {
                    contents: fresh.clone(),
                    ..file.clone()
                },
                _ => file.clone(),
            })
            .collect()
    }

    /// A finding located in the project's sources.
    pub fn record(&self, diagnostic: &Diagnostic) -> DiagnosticRecord {
        DiagnosticRecord::of_diagnostic(diagnostic, |component| self.source_of(component))
    }

    /// Every finding of the load: parse failures, then the checker's
    /// diagnostics, then the lints, then (when asked) the
    /// well-definedness conditions.
    pub fn diagnostics(&self, include_wd: bool) -> Vec<DiagnosticRecord> {
        let mut records = self.parse_errors.clone();
        if let Some(build) = &self.build {
            records.extend(build.diagnostics.iter().map(|d| self.record(d)));
        }
        records.extend(self.lints.iter().map(|d| self.record(d)));
        if include_wd {
            records.extend(self.wd.iter().map(|d| self.record(d)));
        }
        records
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
    pub async fn load(&self) -> Result<Arc<Loaded>, String> {
        let mut cache = self.cache.lock().await;
        let root = self.root.clone();
        let scan = tokio::task::spawn_blocking(move || scan(&root))
            .await
            .map_err(|e| format!("the scan task failed: {e}"))??;
        if let Some(loaded) = cache.as_ref()
            && loaded.digest == scan.digest
        {
            return Ok(Arc::clone(loaded));
        }
        let root = self.root.clone();
        let loaded = tokio::task::spawn_blocking(move || Arc::new(Loaded::from_scan(&root, scan)))
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

/// Read the sources under the root (its direct children only) and the
/// previous proof files under the output directory, and digest them all.
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

    let mut hasher = DefaultHasher::new();
    let mut sources = Vec::new();
    for path in paths {
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        relative.hash(&mut hasher);
        text.hash(&mut hasher);
        sources.push(SourceFile {
            path: relative,
            text,
        });
    }

    // The previous proof state shapes the reconciled files, so it is part
    // of what identifies a load.
    let out = output_dir(root);
    if let Ok(entries) = std::fs::read_dir(&out) {
        let mut previous: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| matches!(ext, "bpo" | "bps" | "bpr"))
            })
            .collect();
        previous.sort();
        for path in previous {
            if let Ok(bytes) = std::fs::read(&path) {
                path.file_name().hash(&mut hasher);
                bytes.hash(&mut hasher);
            }
        }
    }

    Ok(Scan {
        digest: hasher.finish(),
        kind,
        sources,
    })
}

impl Loaded {
    fn from_scan(root: &Path, scan: Scan) -> Loaded {
        let name = project_name(root);
        let out = output_dir(root);
        let mut loaded = Loaded {
            digest: scan.digest,
            name: name.clone(),
            kind: scan.kind,
            output_dir: out.clone(),
            components: Vec::new(),
            edges: Vec::new(),
            parse_errors: Vec::new(),
            project: None,
            build: None,
            lints: Vec::new(),
            wd: Vec::new(),
            obligations: None,
            sources: None,
            invariants: Vec::new(),
            fresh_statuses: HashMap::new(),
            files_of: HashMap::new(),
        };

        let project = match scan.kind {
            SourceKind::Text => loaded.text_project(&name, &scan.sources),
            SourceKind::Rodin => loaded.rodin_project(root),
        };
        let Some(project) = project else {
            return loaded;
        };

        for pc in &project.components {
            let component = &pc.component;
            loaded.components.push(ComponentRecord {
                name: component.name().to_string(),
                kind: match component {
                    Component::Context(_) => "context",
                    Component::Machine(_) => "machine",
                }
                .to_string(),
                file: loaded
                    .files_of
                    .get(component.name())
                    .map(|(file, _)| file.clone())
                    .unwrap_or_else(|| pc.filename.clone()),
            });
            match component {
                Component::Context(context) => {
                    for parent in &context.extends {
                        loaded.edge(&context.name, parent, "extends");
                    }
                }
                Component::Machine(machine) => {
                    if let Some(parent) = &machine.refines {
                        loaded.edge(&machine.name, parent, "refines");
                    }
                    for seen in &machine.sees {
                        loaded.edge(&machine.name, seen, "sees");
                    }
                    let ascii = rossi::pretty::PrettyPrinter::ascii();
                    for invariant in &machine.invariants {
                        let Some(label) = &invariant.label else {
                            continue;
                        };
                        let mut renderings = vec![
                            crate::animate::normalize_predicate(
                                &rossi_build::normalize::canonical_predicate(&invariant.predicate),
                            ),
                            crate::animate::normalize_predicate(
                                &ascii.print_formula_predicate(&invariant.predicate),
                            ),
                        ];
                        renderings.dedup();
                        loaded.invariants.push(InvariantInfo {
                            component: machine.name.clone(),
                            label: label.clone(),
                            renderings,
                        });
                    }
                }
            }
        }

        let (mut build, model) = rossi_build::build_with_model(&project);
        // The generated statuses, kept before reconciliation overwrites
        // them, are what a model-checking run is written with.
        loaded.fresh_statuses = build
            .files
            .iter()
            .filter(|file| file.filename.ends_with(".bps"))
            .map(|file| (file.filename.clone(), file.contents.clone()))
            .collect();
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

        loaded.lints = rossi_build::lint::run(&project);
        loaded.wd = rossi_build::wd::run(&project, &model);
        loaded.obligations = Obligations::from_files(&build.files).ok();
        loaded.sources = Some(SourceIndex::from_model(&model));
        loaded.project = Some(project);
        loaded.build = Some(build);
        loaded
    }

    fn edge(&mut self, from: &str, to: &str, kind: &str) {
        self.edges.push(EdgeRecord {
            from: from.to_string(),
            to: to.to_string(),
            kind: kind.to_string(),
        });
    }

    /// Parse the text sources into a project. Each component takes the
    /// filename a Rodin archive would give it, so the handles a build
    /// writes match those of an exported archive. A file that fails to
    /// parse becomes a finding and no project: a recovered tree drops
    /// elements, and checking it would report on a different model.
    fn text_project(&mut self, name: &str, sources: &[SourceFile]) -> Option<Project> {
        let mut components = Vec::new();
        for source in sources {
            match rossi::parse_components(&source.text) {
                Ok(parsed) => {
                    for component in parsed {
                        self.files_of.insert(
                            component.name().to_string(),
                            (source.path.clone(), Some(source.text.clone())),
                        );
                        components.push(ProjectComponent::from_parsed(
                            rossi::component_filename(&component),
                            component,
                            Some(source.text.clone()),
                        ));
                    }
                }
                Err(error) => {
                    self.parse_errors
                        .extend(parse_failures(&source.path, &source.text, &error))
                }
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
                    self.files_of
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
    let recovered = rossi::parse_components_with_recovery(text);
    let precise: Vec<DiagnosticRecord> = recovered
        .errors
        .iter()
        .filter_map(|err| precise_formula_error(err))
        .map(|(err, span)| {
            let rule = match err {
                ParseError::AssignmentInPredicate { .. } => RuleId::AssignmentInPredicate,
                _ => RuleId::FormulaParseError,
            };
            let region = span.map(|span| Region::of_span(text, span)).or_else(|| {
                err.position()
                    .map(|(line, column)| Region::point(line, column))
            });
            DiagnosticRecord::for_file(rule, file, err.to_string(), region)
        })
        .collect();
    if !precise.is_empty() {
        return precise;
    }
    let region = error
        .position()
        .map(|(line, column)| Region::point(line, column));
    vec![DiagnosticRecord::for_file(
        RuleId::CamilleParseError,
        file,
        error.to_string(),
        region,
    )]
}

/// A formula failure worth reporting on its own, with its absolute span
/// when the recovering parser located it.
fn precise_formula_error(err: &ParseError) -> Option<(&ParseError, Option<rossi::ast::Span>)> {
    match err {
        ParseError::AssignmentInPredicate { .. } | ParseError::AssignmentArityMismatch { .. } => {
            Some((err, None))
        }
        ParseError::RecoverableError {
            span: Some(recovery_span),
            source: Some(source),
            ..
        } if matches!(source.as_ref(), ParseError::AssignmentArityMismatch { .. }) => {
            let absolute = source.span().map_or(*recovery_span, |mut span| {
                span.shift(recovery_span.start);
                span
            });
            Some((source, Some(absolute)))
        }
        _ => None,
    }
}

/// The written form of a build: every generated file under the output
/// directory.
pub fn write_build(loaded: &Loaded) -> Result<(), String> {
    let Some(build) = &loaded.build else {
        return Err("nothing to write: the project did not load".to_string());
    };
    std::fs::create_dir_all(&loaded.output_dir)
        .map_err(|e| format!("cannot create {}: {e}", loaded.output_dir.display()))?;
    for file in &build.files {
        if !rossi_build::is_normal_path_component(&file.filename) {
            return Err(format!("unsafe generated filename {:?}", file.filename));
        }
        let path = loaded.output_dir.join(&file.filename);
        std::fs::write(&path, &file.contents)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    Ok(())
}

/// The obligations of a load by nature name, for the build report.
pub fn obligation_counts(loaded: &Loaded) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    if let Some(obligations) = &loaded.obligations {
        for po in obligations.iter() {
            let key = po.nature.map_or_else(
                || po.description.to_string(),
                |nature| format!("{nature:?}"),
            );
            *counts.entry(key).or_insert(0) += 1;
        }
    }
    counts
}
