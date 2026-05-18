//! Rodin `IRodinElement` URIs used in the `org.eventb.core.source` and
//! `org.eventb.core.scTarget` attributes of checked files.
//!
//! Format (from real `.bcc`/`.bcm` samples):
//!
//! ```text
//! /PROJECT/FILE.ext|org.eventb.core.fileType#FileName|org.eventb.core.kind#rodinId(|...)*
//! ```
//!
//! For example:
//!
//! ```text
//! /COMP1216/AuctionContext.buc|org.eventb.core.contextFile#AuctionContext|org.eventb.core.carrierSet#_qJ3S4O5PEeSpR9iqQeSCVw
//! ```

use std::fmt::Write;

/// A Rodin element handle URI.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HandleUri(String);

impl HandleUri {
    /// A file-level handle, e.g. `/COMP1216/AuctionContext.bcc`.
    pub fn file(project: &str, filename: &str) -> Self {
        HandleUri(format!("/{project}/{filename}"))
    }

    /// Top-level element handle inside a file.
    ///
    /// `root_type` is something like `"org.eventb.core.contextFile"` or
    /// `"org.eventb.core.machineFile"`; `name` is the component's name.
    pub fn root(project: &str, filename: &str, root_type: &str, name: &str) -> Self {
        HandleUri(format!("/{project}/{filename}|{root_type}#{name}"))
    }

    /// Extend an existing handle with a child step.
    ///
    /// `child_type` is e.g. `"org.eventb.core.carrierSet"`, `"org.eventb.core.event"`,
    /// `"org.eventb.core.guard"`. `id` is the child's Rodin internal name.
    ///
    /// The id is escaped per Rodin's URI rules — `/` becomes `\/` so it
    /// doesn't get confused with path separators in the URI. This is the
    /// exact shape Rodin emits when a Rodin internal-name counter
    /// happens to land on the `/` character.
    pub fn child(&self, child_type: &str, id: &str) -> Self {
        let mut s = self.0.clone();
        write!(&mut s, "|{child_type}#").unwrap();
        escape_handle_id(id, &mut s);
        HandleUri(s)
    }

    /// Raw string view, for writing into XML attributes.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for HandleUri {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<HandleUri> for String {
    fn from(h: HandleUri) -> String {
        h.0
    }
}

/// Escape a Rodin element-name segment for inclusion in a handle URI.
///
/// Rodin's auto-counter for internal element names cycles through ASCII
/// punctuation, sometimes landing on characters that conflict with the
/// URI grammar:
///
/// - `/` would be confused with the path separator at the start of the
///   URI.
/// - `|` is the URI segment separator.
/// - `\` is the escape character itself, so a literal `\` must be doubled.
///
/// We prepend `\` to each. Rodin emits the same shape; matching it lets
/// our `source=` and `scTarget=` URIs round-trip byte-equal in the
/// corpus diff.
fn escape_handle_id(id: &str, out: &mut String) {
    for c in id.chars() {
        if matches!(c, '/' | '|' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
}

/// Public escape helper for callers that build URIs by `format!()`
/// rather than via [`HandleUri::child`]. Returns a fresh `String`.
#[must_use]
pub fn escape_handle_id_owned(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    escape_handle_id(id, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_handle() {
        assert_eq!(
            HandleUri::file("COMP1216", "AuctionContext.bcc").as_str(),
            "/COMP1216/AuctionContext.bcc"
        );
    }

    #[test]
    fn carrier_set_handle_matches_rodin_sample() {
        // From AuctionContext.bcc line 3:
        //   /COMP1216/AuctionContext.buc|org.eventb.core.contextFile#AuctionContext|org.eventb.core.carrierSet#_qJ3S4O5PEeSpR9iqQeSCVw
        let h = HandleUri::root(
            "COMP1216",
            "AuctionContext.buc",
            "org.eventb.core.contextFile",
            "AuctionContext",
        )
        .child("org.eventb.core.carrierSet", "_qJ3S4O5PEeSpR9iqQeSCVw");
        assert_eq!(
            h.as_str(),
            "/COMP1216/AuctionContext.buc|org.eventb.core.contextFile#AuctionContext|org.eventb.core.carrierSet#_qJ3S4O5PEeSpR9iqQeSCVw"
        );
    }

    #[test]
    fn slash_in_id_is_escaped() {
        // Rodin's ASCII counter can hit `/`; the handle URI must escape it.
        let h = HandleUri::root("proj", "M.bum", "org.eventb.core.machineFile", "M")
            .child("org.eventb.core.event", "/");
        assert!(
            h.as_str().contains("#\\/"),
            "expected escaped `\\/`, got {}",
            h.as_str()
        );
    }

    #[test]
    fn pipe_in_id_is_escaped() {
        // Rodin's ASCII counter can land on `|` (the URI separator).
        // It must be escaped as `\|` so the URI structure stays intact.
        let h = HandleUri::root("proj", "M.bum", "org.eventb.core.machineFile", "M")
            .child("org.eventb.core.event", "|");
        assert!(
            h.as_str().contains("#\\|"),
            "expected escaped `\\|`, got {}",
            h.as_str()
        );
    }

    #[test]
    fn backslash_in_id_is_escaped() {
        // A literal `\` in the id must double itself, otherwise it would
        // be interpreted as the start of an escape sequence.
        let h = HandleUri::root("proj", "M.bum", "org.eventb.core.machineFile", "M")
            .child("org.eventb.core.event", "\\");
        assert!(
            h.as_str().contains("#\\\\"),
            "expected doubled `\\\\`, got {}",
            h.as_str()
        );
    }

    #[test]
    fn escape_handle_id_owned_helper() {
        assert_eq!(escape_handle_id_owned("a/b|c\\d"), "a\\/b\\|c\\\\d");
        assert_eq!(escape_handle_id_owned("plain"), "plain");
    }

    #[test]
    fn nested_event_guard_handle() {
        // From AuctionMachine.bcm line 27:
        //   /COMP1216/AuctionMachine.bum|org.eventb.core.machineFile#AuctionMachine|org.eventb.core.event#_ekPJAO5OEeSpR9iqQeSCVw|org.eventb.core.guard#_ekVPoe5OEeSpR9iqQeSCVw
        let h = HandleUri::root(
            "COMP1216",
            "AuctionMachine.bum",
            "org.eventb.core.machineFile",
            "AuctionMachine",
        )
        .child("org.eventb.core.event", "_ekPJAO5OEeSpR9iqQeSCVw")
        .child("org.eventb.core.guard", "_ekVPoe5OEeSpR9iqQeSCVw");
        assert_eq!(
            h.as_str(),
            "/COMP1216/AuctionMachine.bum|org.eventb.core.machineFile#AuctionMachine|org.eventb.core.event#_ekPJAO5OEeSpR9iqQeSCVw|org.eventb.core.guard#_ekVPoe5OEeSpR9iqQeSCVw"
        );
    }
}
