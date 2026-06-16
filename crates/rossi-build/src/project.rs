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

/// Scan a `.zip` archive for the first `.bcc` / `.bcm` and infer the Rodin
/// project name from its handle URIs. Returns `None` for archives without
/// pre-built checked files.
pub fn infer_project_name_from_archive_bytes(zip_bytes: &[u8]) -> Option<String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).ok()?;
    for i in 0..archive.len() {
        let Ok(mut entry) = archive.by_index(i) else {
            continue;
        };
        let n = entry.name().to_string();
        if !(n.ends_with(".bcc") || n.ends_with(".bcm")) {
            continue;
        }
        let mut xml = String::new();
        if std::io::Read::read_to_string(&mut entry, &mut xml).is_err() {
            continue;
        }
        if let Some(name) = infer_project_name_from_checked_xml(&xml) {
            return Some(name);
        }
    }
    None
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
}
