//! A loaded Event-B project — the input to [`crate::build`].
//!
//! A [`Project`] is a named collection of parsed `Component` values (contexts
//! and machines) ready to be statically checked. It can be loaded from:
//!
//! - a Rodin `.zip` archive ([`Project::from_zip_file`] / [`Project::from_zip_bytes`]),
//! - a directory containing `.buc` / `.bum` files ([`Project::from_directory`]),
//! - or assembled in memory from parsed components ([`Project::new`]).

use rossi::{Component, parse_xml};

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
        })
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
            if !is_input_file(&filename) {
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

    /// Load a project from a directory of `.buc` / `.bum` files.
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

        let mut components = Vec::new();
        for entry in walkdir::WalkDir::new(path)
            .max_depth(1)
            .into_iter()
            .flatten()
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let filename = match entry.file_name().to_str() {
                Some(n) if is_input_file(n) => n.to_string(),
                _ => continue,
            };
            let xml = std::fs::read_to_string(entry.path())?;
            components.push(ProjectComponent::from_xml(filename, &xml)?);
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

fn is_input_file(name: &str) -> bool {
    name.ends_with(".buc") || name.ends_with(".bum")
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

/// Last path segment (`a/b/M1.bpr` → `M1.bpr`). Handles `/` only — zip
/// archives normalize to forward slashes regardless of the host OS.
pub(crate) fn basename(path: &str) -> &str {
    path.rsplit_once('/').map(|(_, b)| b).unwrap_or(path)
}
