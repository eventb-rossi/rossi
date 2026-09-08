//! `rossi dump` — write a checked project as a `rossi-model` JSON document.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;
use rossi_build::dump;
use rossi_build::project::{self, DiscoveredProject, Project, ProjectComponent, discover_projects};

use crate::commands::eventb_io::{self, InputKind};
use crate::commands::report::{Report, finish_structured_output};

#[derive(Args)]
pub struct DumpArgs {
    /// Input to dump: a Rodin `.zip`, a directory (a Rodin project or a
    /// folder of Event-B text), or a single `.eventb`/`.txt`/`.buc`/`.bum`
    /// file.
    #[arg(value_name = "INPUT", required_unless_present = "schema")]
    input: Option<PathBuf>,

    /// Write the document here instead of standard output
    #[arg(short, long, value_name = "FILE")]
    output: Option<PathBuf>,

    /// Project to dump from an archive holding several (its name or its
    /// archive prefix)
    #[arg(long, value_name = "NAME", conflicts_with = "schema")]
    project: Option<String>,

    /// Indent the document so it can be read
    ///
    /// The default is one line. A document is written for a tool rather than
    /// for a reader, and indenting one costs four times the bytes.
    #[arg(long, conflicts_with = "schema")]
    pretty: bool,

    /// Print the JSON Schema of the document format and exit
    #[arg(long)]
    schema: bool,
}

/// A loaded project, with what the document needs to say about where it came
/// from.
struct Loaded {
    project: Project,
    prefix: Option<String>,
    /// The real path of each component, keyed by the Rodin filename the
    /// loader gave it.
    source_paths: BTreeMap<String, String>,
}

pub fn run(args: DumpArgs) -> ExitCode {
    if args.schema {
        return write_schema(args.output.as_deref());
    }

    let input = args.input.as_deref().expect("clap requires an input");
    let loaded = match load(input, args.project.as_deref()) {
        Ok(loaded) => loaded,
        Err(failure) => {
            eprintln!("rossi dump: {}", failure.message);
            return ExitCode::from(failure.code);
        }
    };

    let (mut build, sc) = rossi_build::check_with_model(&loaded.project);
    // The checker renders each component's checked XML into a string on the
    // way past. A document is built from the model and the findings, so those
    // strings are never read, and on a large project they are tens of
    // megabytes held live for the whole conversion.
    build.files = Vec::new();
    let options = dump::Options {
        input: Some(input.display().to_string()),
        prefix: loaded.prefix,
        source_paths: loaded.source_paths,
    };
    let model = dump::model(&loaded.project, &sc, &build, &options);
    // The document is written whatever it contains; a rejected project still
    // has a checked part worth reading, and `diagnostics` says what is
    // missing from it.
    let exit = if model.has_errors() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    };

    let mut report = match Report::open(args.output.as_deref()) {
        Ok(report) => report,
        Err(e) => {
            eprintln!("rossi dump: failed to open output: {e}");
            return ExitCode::from(1);
        }
    };
    let written = report
        .structured(|out| {
            if args.pretty {
                serde_json::to_writer_pretty(&mut *out, &model)
            } else {
                serde_json::to_writer(&mut *out, &model)
            }
            .map_err(io::Error::from)?;
            writeln!(out)
        })
        .and_then(|()| report.flush());

    finish_structured_output("dump", written, exit, &mut io::stderr().lock())
}

fn write_schema(output: Option<&Path>) -> ExitCode {
    let mut report = match Report::open(output) {
        Ok(report) => report,
        Err(e) => {
            eprintln!("rossi dump: failed to open output: {e}");
            return ExitCode::from(1);
        }
    };
    let written = report
        .structured(|out| {
            out.write_all(dump::JSON_SCHEMA.as_bytes())?;
            if !dump::JSON_SCHEMA.ends_with('\n') {
                writeln!(out)?;
            }
            Ok(())
        })
        .and_then(|()| report.flush());

    finish_structured_output("dump", written, ExitCode::SUCCESS, &mut io::stderr().lock())
}

/// Why an input could not be dumped, and what that should exit with.
struct Failure {
    message: String,
    /// 2 for a usage mistake, 1 for everything else.
    code: u8,
}

impl Failure {
    fn usage(message: impl Into<String>) -> Failure {
        Failure {
            message: message.into(),
            code: 2,
        }
    }
}

impl From<Box<dyn std::error::Error>> for Failure {
    fn from(error: Box<dyn std::error::Error>) -> Failure {
        Failure {
            message: error.to_string(),
            code: 1,
        }
    }
}

/// Load a project without losing where anything was written.
///
/// The build command routes Event-B text through a Rodin archive and reads it
/// back, which keeps its handles byte-exact but discards the source text and
/// every span with it. A document is largely about where things are, so this
/// loads each input directly instead, and keeps the handles identical by
/// other means: each component is named as the archive writer would name it,
/// and the project name goes through the same normalisation the archive
/// descriptor applies. The path the text really came from is recorded
/// separately and reported in the document's source list.
fn load(input: &Path, wanted_project: Option<&str>) -> Result<Loaded, Failure> {
    if !input.exists() {
        return Err(Failure::usage(format!(
            "Input not found: {}",
            input.display()
        )));
    }

    if input.is_dir() {
        // A directory is one project, so there is nothing for --project to
        // choose between.
        if wanted_project.is_some() {
            return Err(Failure::usage(
                "--project selects from an archive holding several projects; a directory is one project",
            ));
        }
        return load_directory(input);
    }

    match eventb_io::classify_file(input).map_err(Failure::from)? {
        InputKind::Text => load_text(&[input.to_path_buf()], eventb_io::file_project_name(input)),
        InputKind::RodinXml => {
            // Read straight into a project component rather than through the
            // shared helper, which drops the Rodin identifiers the file
            // carries; those are what a handle is built from.
            let xml = std::fs::read_to_string(input).map_err(|e| Failure {
                message: format!("{}: {e}", input.display()),
                code: 1,
            })?;
            let filename = input
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| Failure::usage(format!("Invalid filename: {}", input.display())))?;
            let component = ProjectComponent::from_xml(filename, &xml).map_err(|e| Failure {
                message: e.to_string(),
                code: 1,
            })?;
            Ok(Loaded {
                project: Project::new(
                    rossi::descriptor_project_name(&eventb_io::file_project_name(input))
                        .to_string(),
                    vec![component],
                ),
                prefix: None,
                source_paths: BTreeMap::new(),
            })
        }
        InputKind::RodinZip => load_archive(input, wanted_project),
    }
}

fn load_directory(input: &Path) -> Result<Loaded, Failure> {
    let files = project::component_files(input).map_err(|e| Failure {
        message: e.to_string(),
        code: 1,
    })?;
    if files.iter().any(|path| project::is_xml_component(path)) {
        let mut project = Project::from_directory(input).map_err(|e| Failure {
            message: e.to_string(),
            code: 1,
        })?;
        // A directory is read in whatever order the filesystem hands it back,
        // which would make the document depend on the machine it was produced
        // on.
        project
            .components
            .sort_by(|a, b| a.filename.cmp(&b.filename));
        project.name = rossi::descriptor_project_name(&project.name).to_string();
        return Ok(Loaded {
            project,
            prefix: None,
            source_paths: BTreeMap::new(),
        });
    }

    let text_files =
        eventb_io::collect_eventb_files(&[input.to_path_buf()]).map_err(Failure::from)?;
    if text_files.is_empty() {
        return Err(Failure::usage(format!(
            "No Event-B components found in directory: {}",
            input.display()
        )));
    }
    load_text(&text_files, eventb_io::dir_project_name(input))
}

/// Load Event-B text under the Rodin names its components would be written
/// with, so a document's handles are the ones a build produces.
fn load_text(files: &[PathBuf], project_name: String) -> Result<Loaded, Failure> {
    let mut components = Vec::new();
    let mut source_paths = BTreeMap::new();
    for path in files {
        let text = std::fs::read_to_string(path).map_err(|e| Failure {
            message: format!("{}: {e}", path.display()),
            code: 1,
        })?;
        let parsed = rossi::parse_components(&text).map_err(|e| Failure {
            message: format!("Failed to parse {}: {e}", path.display()),
            code: 1,
        })?;
        for component in parsed {
            let filename = rossi::component_filename(&component);
            source_paths.insert(filename.clone(), path.display().to_string());
            components.push(ProjectComponent::from_parsed(
                filename,
                component,
                Some(text.clone()),
            ));
        }
    }

    Ok(Loaded {
        project: Project::new(
            rossi::descriptor_project_name(&project_name).to_string(),
            components,
        ),
        prefix: None,
        source_paths,
    })
}

fn load_archive(input: &Path, wanted_project: Option<&str>) -> Result<Loaded, Failure> {
    let bytes = std::fs::read(input).map_err(|e| Failure {
        message: format!("{}: {e}", input.display()),
        code: 1,
    })?;
    let projects =
        discover_projects(&bytes, &eventb_io::file_project_name(input)).map_err(|e| Failure {
            message: e.to_string(),
            code: 1,
        })?;
    if projects.is_empty() {
        return Err(Failure::usage(format!(
            "No Rodin project found in {}",
            input.display()
        )));
    }

    let selected = match wanted_project {
        Some(wanted) => projects
            .iter()
            .position(|p| matches_selection(p, wanted))
            .ok_or_else(|| {
                Failure::usage(format!(
                    "No project named '{wanted}' in {}. Available: {}",
                    input.display(),
                    describe(&projects)
                ))
            })?,
        None if projects.len() == 1 => 0,
        None => {
            return Err(Failure::usage(format!(
                "{} holds several projects; choose one with --project. Available: {}",
                input.display(),
                describe(&projects)
            )));
        }
    };

    let discovered = projects
        .into_iter()
        .nth(selected)
        .expect("the chosen project");
    let prefix = (!discovered.prefix.is_empty()).then(|| discovered.prefix.clone());
    Ok(Loaded {
        project: discovered.into_project(),
        prefix,
        source_paths: BTreeMap::new(),
    })
}

/// A project is chosen by its name, or by the archive prefix it sits under.
///
/// The name is the identity a reader knows; the prefix is what an archive
/// listing shows. Accepting both means neither has to be explained.
fn matches_selection(project: &DiscoveredProject, wanted: &str) -> bool {
    project.name == wanted || project.prefix.trim_end_matches('/') == wanted
}

fn describe(projects: &[DiscoveredProject]) -> String {
    projects
        .iter()
        .map(|p| {
            if p.prefix.is_empty() {
                p.name.clone()
            } else {
                format!("{} ({})", p.name, p.prefix.trim_end_matches('/'))
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}
