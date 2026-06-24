//! A loaded Event-B project — the input to [`crate::build`].
//!
//! A [`Project`] is a named collection of parsed `Component` values (contexts
//! and machines) ready to be statically checked. It can be loaded from:
//!
//! - a Rodin `.zip` archive ([`Project::from_zip_file`] / [`Project::from_zip_bytes`]),
//! - a directory of `.buc` / `.bum` (Rodin XML) or `.eventb` / `.txt` (textual)
//!   files ([`Project::from_directory`]),
//! - or assembled in memory from parsed components ([`Project::new`]).
//!
//! Components parsed from `.eventb` text retain their [`source`](ProjectComponent::source)
//! and AST spans, so a caller can resolve a diagnostic's byte span to a
//! line/column. XML imports carry neither.

use rossi::{Component, parse_components, parse_xml};

use crate::error::{ProjectError, Result};
use crate::rodin_ids::RodinIds;

/// An Event-B project: a set of contexts and machines with a project name.
///
/// The project name is what ends up in the leading `/PROJECT/...` segment of
/// every `source=` / `scTarget=` URI in emitted `.bcc`/`.bcm` files.
#[derive(Debug, Clone)]
pub struct Project {
    /// Name used as the leading path segment in handle URIs.
    /// For a zip archive this defaults to the archive's basename.
    pub name: String,
    /// The parsed components in this project.
    pub components: Vec<ProjectComponent>,
}

/// A single parsed component plus the filename it came from.
#[derive(Debug, Clone)]
pub struct ProjectComponent {
    /// Source filename including extension, e.g. `"AuctionContext.buc"`.
    pub filename: String,
    /// The parsed AST component.
    pub component: Component,
    /// Sidecar of Rodin internal element ids scraped from the original
    /// XML. Used to emit byte-exact `source=` URIs. Empty when the
    /// component was not loaded from XML.
    pub rodin_ids: RodinIds,
    /// The original `.eventb` text this component was parsed from, when it came
    /// from a textual source. `None` for Rodin-XML imports. Together with the
    /// AST spans (also textual-only) this lets a caller turn a diagnostic's
    /// byte span into a line/column.
    pub source: Option<String>,
}

impl ProjectComponent {
    /// Parse raw `.buc`/`.bum` XML and bind it to a filename.
    ///
    /// `filename` may include a directory prefix (as from a zip entry like
    /// `binary-search/C0.buc`); only the basename's stem is used for the
    /// component's Event-B name and output filename, mirroring Rodin's
    /// "filename is identity" convention.
    pub fn from_xml(filename: impl Into<String>, xml: &str) -> Result<Self> {
        let filename = basename(&filename.into()).to_string();
        let mut component = parse_xml(xml)?;
        let rodin_ids = RodinIds::from_xml(xml)?;
        let stem = filename
            .rsplit_once('.')
            .map(|(s, _)| s)
            .unwrap_or(&filename)
            .to_string();
        match &mut component {
            Component::Context(c) => c.name = stem,
            Component::Machine(m) => m.name = stem,
        }
        Ok(Self {
            filename,
            component,
            rodin_ids,
            // XML carries no `.eventb` text and the parse sets no spans.
            source: None,
        })
    }

    /// Parse `.eventb` / `.txt` text and bind it to a filename, retaining the
    /// source so a caller can resolve the AST spans to line/columns.
    ///
    /// A textual file may hold more than one component, so this returns a
    /// `Vec`; each component shares the whole file as its `source` (spans are
    /// byte offsets into that text). The component keeps the name it declares
    /// (`context C1` → `C1`); unlike [`from_xml`](Self::from_xml) the filename
    /// stem is not imposed.
    pub fn from_eventb(filename: impl Into<String>, text: &str) -> Result<Vec<Self>> {
        let filename = basename(&filename.into()).to_string();
        let components = parse_components(text)?;
        Ok(components
            .into_iter()
            .map(|component| Self {
                filename: filename.clone(),
                component,
                rodin_ids: RodinIds::default(),
                source: Some(text.to_string()),
            })
            .collect())
    }

    /// Short name (extension stripped). For `"AuctionContext.buc"` this is
    /// `"AuctionContext"`.
    pub fn stem(&self) -> &str {
        self.filename
            .rsplit_once('.')
            .map(|(stem, _)| stem)
            .unwrap_or(&self.filename)
    }

    /// The output filename for this component. A `.buc` becomes `.bcc`; a
    /// `.bum` becomes `.bcm`.
    pub fn output_filename(&self) -> String {
        let stem = self.stem();
        match &self.component {
            Component::Context(_) => format!("{stem}.bcc"),
            Component::Machine(_) => format!("{stem}.bcm"),
        }
    }
}

impl Project {
    /// Build a project in memory.
    pub fn new(name: impl Into<String>, components: Vec<ProjectComponent>) -> Self {
        Self {
            name: name.into(),
            components,
        }
    }

    /// Load a project from a Rodin `.zip` archive on disk.
    pub fn from_zip_file<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let data = std::fs::read(path)?;
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("project")
            .to_string();
        Self::from_zip_bytes(&name, &data)
    }

    /// Load a project from a zip archive already in memory.
    pub fn from_zip_bytes(name: impl Into<String>, data: &[u8]) -> Result<Self> {
        // Use the raw-XML path per file so we can also extract RodinIds.
        let reader = std::io::Cursor::new(data);
        let mut archive = zip::ZipArchive::new(reader)?;
        let mut components = Vec::new();
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let filename = entry.name().to_string();
            if !is_xml_input(&filename) {
                continue;
            }
            let mut xml = String::new();
            std::io::Read::read_to_string(&mut entry, &mut xml)?;
            components.push(ProjectComponent::from_xml(filename, &xml)?);
        }
        Ok(Self {
            name: name.into(),
            components,
        })
    }

    /// Load a project from a directory of component files — `.buc` / `.bum`
    /// (Rodin XML) or `.eventb` (textual).
    ///
    /// A directory is loaded as one kind or the other: if any Rodin XML file is
    /// present the directory is treated as a Rodin project and loose `.eventb`
    /// text is ignored (a real export may sit beside scratch files); otherwise
    /// the `.eventb` files are loaded. Preferring XML keeps `rossi build` on a
    /// Rodin directory byte-identical. Textual components retain their source
    /// and spans (see [`ProjectComponent::source`]); XML ones do not. Only
    /// component files are read — unrelated files (dotfiles, binaries, notes)
    /// are never opened.
    pub fn from_directory<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        if !path.is_dir() {
            return Err(ProjectError::NotADirectory(path.to_path_buf()).into());
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("project")
            .to_string();

        let files: Vec<_> = walkdir::WalkDir::new(path)
            .max_depth(1)
            .into_iter()
            .flatten()
            .filter(|entry| entry.file_type().is_file())
            .collect();
        let has_xml = files
            .iter()
            .any(|entry| entry.file_name().to_str().is_some_and(is_xml_input));

        let mut components = Vec::new();
        for entry in &files {
            let Some(filename) = entry.file_name().to_str() else {
                continue;
            };
            // Read only the files that are actually components of this project's
            // kind; opening unrelated files would both waste work and let a
            // binary / non-UTF-8 entry abort the whole load.
            if has_xml {
                if is_xml_input(filename) {
                    let xml = std::fs::read_to_string(entry.path())?;
                    components.push(ProjectComponent::from_xml(filename, &xml)?);
                }
            } else if is_eventb_input(filename) {
                let text = std::fs::read_to_string(entry.path())?;
                components.extend(ProjectComponent::from_eventb(filename, &text)?);
            }
        }

        Ok(Self { name, components })
    }

    /// Iterate contexts.
    pub fn contexts(&self) -> impl Iterator<Item = (&ProjectComponent, &rossi::Context)> {
        self.components.iter().filter_map(|pc| match &pc.component {
            Component::Context(c) => Some((pc, c)),
            _ => None,
        })
    }

    /// Iterate machines.
    pub fn machines(&self) -> impl Iterator<Item = (&ProjectComponent, &rossi::Machine)> {
        self.components.iter().filter_map(|pc| match &pc.component {
            Component::Machine(m) => Some((pc, m)),
            _ => None,
        })
    }

    /// Find a context by its Event-B name (not filename).
    pub fn context(&self, name: &str) -> Option<&rossi::Context> {
        self.contexts()
            .find(|(_, c)| c.name == name)
            .map(|(_, c)| c)
    }

    /// Find a machine by its Event-B name (not filename).
    pub fn machine(&self, name: &str) -> Option<&rossi::Machine> {
        self.machines()
            .find(|(_, m)| m.name == name)
            .map(|(_, m)| m)
    }
}

/// A single Rodin project discovered inside a (possibly multi-project) archive.
///
/// A Rodin `.zip` exported by Eclipse's Archive-File wizard holds one top-level
/// directory per project (each with its own `.project` descriptor); Rodin
/// imports it back as one project per directory. [`discover_projects`] yields
/// this unit so each project can be built and repackaged under its own
/// directory rather than flattened into one.
#[derive(Debug, Clone)]
pub struct DiscoveredProject {
    /// Archive path prefix for this project's entries, e.g. `"MyProject/"`,
    /// or `""` for a flat (root-level) archive. Includes the trailing `/` when
    /// non-empty, matching the repack layer's prefix convention.
    pub prefix: String,
    /// Resolved project name — the leading `/name/` segment stamped into every
    /// emitted handle URI (see name resolution in [`discover_projects`]).
    pub name: String,
    /// The components (contexts and machines) belonging to this project only.
    pub components: Vec<ProjectComponent>,
}

impl DiscoveredProject {
    /// Materialize as a [`Project`] ready for [`crate::build`].
    #[must_use]
    pub fn into_project(self) -> Project {
        Project::new(self.name, self.components)
    }
}

/// Split a Rodin archive into its constituent projects.
///
/// A Rodin `.zip` may bundle several top-level project directories, each with
/// its own `.project` and a self-contained set of `.buc`/`.bum` (Eclipse's
/// Archive-File export, which Rodin imports back as one project per directory).
/// Entries are grouped by their top-level segment (the part before the first
/// `/`; `""` when an entry sits at the archive root). A group becomes a project
/// when it holds a `.project` and/or any `.buc`/`.bum`; stray root files that
/// are neither are ignored here (the repack layer still copies them verbatim).
///
/// Each project's name is resolved for byte-exact handle parity with Rodin:
/// 1. the name embedded in that project's own first `.bcc`/`.bcm` handle URIs,
/// 2. else the `<name>` in that project's `.project` descriptor,
/// 3. else the top-level directory segment,
/// 4. else (a flat archive with neither) `fallback_name`.
///
/// The returned projects are sorted by `prefix` for deterministic output.
pub fn discover_projects(zip_bytes: &[u8], fallback_name: &str) -> Result<Vec<DiscoveredProject>> {
    #[derive(Default)]
    struct Group {
        components: Vec<ProjectComponent>,
        checked_name: Option<String>,
        project_name: Option<String>,
        has_project: bool,
    }

    let reader = std::io::Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(reader)?;
    // BTreeMap keys (= prefixes) keep the output deterministically prefix-sorted.
    let mut groups: std::collections::BTreeMap<String, Group> = std::collections::BTreeMap::new();

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().to_string();
        let prefix = match name.find('/') {
            Some(slash) => name[..=slash].to_string(),
            None => String::new(),
        };

        if is_xml_input(&name) {
            let mut xml = String::new();
            std::io::Read::read_to_string(&mut entry, &mut xml)?;
            let component = ProjectComponent::from_xml(name, &xml)?;
            groups.entry(prefix).or_default().components.push(component);
        } else if name == format!("{prefix}.project") {
            // Only the descriptor at this group's top level marks the project;
            // a deeper `sub/.project` belongs to a nested resource, not here.
            // A non-UTF-8 descriptor only costs us the name (a fallback covers
            // it), so a read error is skipped rather than failing the build.
            let group = groups.entry(prefix).or_default();
            group.has_project = true;
            let mut xml = String::new();
            if std::io::Read::read_to_string(&mut entry, &mut xml).is_ok() {
                group.project_name = rossi::read_project_name(&xml);
            }
        } else if name.ends_with(".bcc") || name.ends_with(".bcm") {
            // Only the first readable checked file per group is needed for the
            // name; a stale/non-UTF-8 one is skipped (we never built from these
            // before, so a read error must not abort an otherwise-valid build).
            let group = groups.entry(prefix).or_default();
            if group.checked_name.is_none() {
                let mut xml = String::new();
                if std::io::Read::read_to_string(&mut entry, &mut xml).is_ok() {
                    group.checked_name = infer_project_name_from_checked_xml(&xml);
                }
            }
        }
    }

    let mut projects = Vec::new();
    for (prefix, group) in groups {
        if group.components.is_empty() && !group.has_project {
            continue;
        }
        let name = group
            .checked_name
            .or(group.project_name)
            .or_else(|| {
                let seg = prefix.trim_end_matches('/');
                (!seg.is_empty()).then(|| seg.to_string())
            })
            .unwrap_or_else(|| fallback_name.to_string());
        projects.push(DiscoveredProject {
            prefix,
            name,
            components: group.components,
        });
    }

    Ok(projects)
}

/// A Rodin XML component file (`.buc` context / `.bum` machine).
fn is_xml_input(name: &str) -> bool {
    name.ends_with(".buc") || name.ends_with(".bum")
}

/// A textual Event-B component file. Only `.eventb` is auto-loaded from a
/// directory — `.txt` is too generic to scan for (a `README.txt` is not a
/// component), though a `.txt` passed explicitly is still validated as text.
fn is_eventb_input(name: &str) -> bool {
    name.ends_with(".eventb")
}

/// Extract Rodin's project name from a `.bcc` / `.bcm` XML payload.
///
/// Rodin embeds the project name as the leading path segment of its handle
/// URIs (e.g. `org.eventb.core.source="/auction/AuctionContext.buc..."`).
/// When repackaging a third-party archive whose top-level directory does not
/// match the project name, callers want the *Rodin* name so emitted handles
/// remain byte-identical. Returns `None` if no recognizable marker is found.
#[must_use]
pub fn infer_project_name_from_checked_xml(xml: &str) -> Option<String> {
    let marker = "org.eventb.core.source=\"/";
    let i = xml.find(marker)?;
    let rest = &xml[i + marker.len()..];
    let slash = rest.find('/')?;
    Some(rest[..slash].to_string())
}

fn basename(path: &str) -> &str {
    // Handles '/' — zip archives normalize to forward slashes regardless of
    // the host OS.
    path.rsplit_once('/').map(|(_, b)| b).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_eventb_retains_source_name_and_spans() {
        let text = "CONTEXT C1\nSETS\n    S\nCONSTANTS\n    c\nAXIOMS\n    @axm1 c ∈ ℕ\nEND\n";
        let comps = ProjectComponent::from_eventb("C1.eventb", text).unwrap();
        assert_eq!(comps.len(), 1);
        let pc = &comps[0];
        // The whole file text is retained for span resolution.
        assert_eq!(pc.source.as_deref(), Some(text));
        // The declared name wins; the filename stem is not imposed.
        assert_eq!(pc.component.name(), "C1");
        // The carrier set carries a source span (textual parse).
        match &pc.component {
            Component::Context(c) => assert!(c.sets[0].span().is_some()),
            other => panic!("expected a context, got {other:?}"),
        }
    }

    #[test]
    fn eventb_project_semantic_diagnostic_is_spanned() {
        // Semantic checks run over an `.eventb`-sourced project, and the
        // resulting diagnostic anchors on the offending element. `k` has no
        // typing axiom, so the static check cannot infer its type.
        let text = "CONTEXT C1\nCONSTANTS\n    k\nAXIOMS\n    @axm1 ⊤\nEND\n";
        let comps = ProjectComponent::from_eventb("C1.eventb", text).unwrap();
        let project = Project::new("p", comps);

        let result = crate::build(&project);
        let diag = result
            .diagnostics
            .iter()
            .find(|d| d.message.contains("could not infer type"))
            .expect("untyped constant should be flagged");
        let span = diag.span.expect("semantic diagnostic should carry a span");
        assert_eq!(&text[span.start..span.end], "k");
    }

    // --- discover_projects -------------------------------------------------

    use std::io::Write;
    use zip::write::{SimpleFileOptions, ZipWriter};

    fn ctx_xml() -> &'static str {
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <org.eventb.core.contextFile version=\"3\" \
         org.eventb.core.configuration=\"org.eventb.core.fwd\"></org.eventb.core.contextFile>\n"
    }
    fn mch_xml() -> &'static str {
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <org.eventb.core.machineFile version=\"5\" \
         org.eventb.core.configuration=\"org.eventb.core.fwd\"></org.eventb.core.machineFile>\n"
    }
    fn project_xml(name: &str) -> String {
        format!(
            "<?xml version=\"1.0\"?>\n<projectDescription>\n  <name>{name}</name>\n</projectDescription>\n"
        )
    }
    fn checked_xml(project: &str) -> String {
        format!("<x org.eventb.core.source=\"/{project}/foo.buc|t#c\"/>")
    }

    fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        let mut w = ZipWriter::new(&mut cursor);
        let opts = SimpleFileOptions::default();
        for (name, body) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(body).unwrap();
        }
        w.finish().unwrap();
        cursor.into_inner()
    }

    #[test]
    fn discover_flat_archive_is_one_project() {
        let zip = make_zip(&[
            ("C.buc", ctx_xml().as_bytes()),
            ("M.bum", mch_xml().as_bytes()),
        ]);
        let projects = discover_projects(&zip, "fallback").unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].prefix, "");
        // No checked file and no `.project`: flat archive falls back to caller name.
        assert_eq!(projects[0].name, "fallback");
        assert_eq!(projects[0].components.len(), 2);
    }

    #[test]
    fn discover_splits_sibling_dirs_with_colliding_basenames() {
        // Two projects each with a `context.buc` and same-named machine — the
        // exact shape that the old flat loader collapsed.
        let zip = make_zip(&[
            ("A/.project", project_xml("A").as_bytes()),
            ("A/context.buc", ctx_xml().as_bytes()),
            ("A/M1.bum", mch_xml().as_bytes()),
            ("B/.project", project_xml("B").as_bytes()),
            ("B/context.buc", ctx_xml().as_bytes()),
            ("B/M1.bum", mch_xml().as_bytes()),
        ]);
        let projects = discover_projects(&zip, "fallback").unwrap();
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[0].prefix, "A/");
        assert_eq!(projects[1].prefix, "B/");
        // Each project carries only its own components — no cross-contamination.
        assert_eq!(projects[0].components.len(), 2);
        assert_eq!(projects[1].components.len(), 2);
    }

    #[test]
    fn discover_keeps_project_only_dir_with_no_components() {
        // A source-only `.project` dir (no `.buc`/`.bum`) beside a real
        // Event-B project dir — both are discovered, the former empty.
        let zip = make_zip(&[
            ("src/.project", project_xml("src").as_bytes()),
            ("src/diagram.txt", b"diagram"),
            ("model/.project", project_xml("model").as_bytes()),
            ("model/M.bum", mch_xml().as_bytes()),
        ]);
        let projects = discover_projects(&zip, "fallback").unwrap();
        assert_eq!(projects.len(), 2);
        let src = projects.iter().find(|p| p.prefix == "src/").unwrap();
        assert!(src.components.is_empty());
        let model = projects.iter().find(|p| p.prefix == "model/").unwrap();
        assert_eq!(model.components.len(), 1);
    }

    #[test]
    fn discover_name_resolution_priority() {
        // (a) checked .bcm handle prefix wins even over the dir segment.
        let a = make_zip(&[
            ("dir/.project", project_xml("ProjName").as_bytes()),
            ("dir/M.bum", mch_xml().as_bytes()),
            ("dir/M.bcm", checked_xml("HandleName").as_bytes()),
        ]);
        assert_eq!(discover_projects(&a, "fb").unwrap()[0].name, "HandleName");

        // (b) no checked file -> the `.project` <name> wins over the dir segment.
        let b = make_zip(&[
            ("dir/.project", project_xml("ProjName").as_bytes()),
            ("dir/M.bum", mch_xml().as_bytes()),
        ]);
        assert_eq!(discover_projects(&b, "fb").unwrap()[0].name, "ProjName");

        // (c) neither checked nor `.project` -> the top-level dir segment.
        let c = make_zip(&[("dir/M.bum", mch_xml().as_bytes())]);
        assert_eq!(discover_projects(&c, "fb").unwrap()[0].name, "dir");
    }

    #[test]
    fn discover_ignores_nested_project_descriptor() {
        // A descriptor deeper than the top level (A/sub/.project) must not be
        // mistaken for project A's own .project (which would steal A's name).
        let zip = make_zip(&[
            ("A/.project", project_xml("RealName").as_bytes()),
            ("A/M.bum", mch_xml().as_bytes()),
            ("A/sub/.project", project_xml("WrongName").as_bytes()),
        ]);
        let projects = discover_projects(&zip, "fb").unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "RealName");
    }

    #[test]
    fn discover_tolerates_non_utf8_checked_file() {
        // A stale/corrupt non-UTF-8 .bcm must be skipped for name inference,
        // not abort discovery — the valid .bum source still builds.
        let zip = make_zip(&[
            ("dir/M.bum", mch_xml().as_bytes()),
            ("dir/M.bcm", b"\xff\xfe not valid utf-8"),
        ]);
        let projects = discover_projects(&zip, "fb").unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].components.len(), 1);
        // Name falls through past the unreadable checked file to the dir segment.
        assert_eq!(projects[0].name, "dir");
    }
}
