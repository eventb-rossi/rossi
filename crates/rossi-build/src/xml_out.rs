//! Deterministic XML emitter for `.bcc` / `.bcm` files.
//!
//! Rather than `quick-xml`'s attribute API (which has no stable ordering
//! guarantee), we build output by hand: push ordered `(key, value)` pairs
//! onto each element and serialise in insertion order. This is the minimum
//! machinery needed for byte-stable output; we can swap to `quick-xml`'s
//! `Writer` later if desired without changing the public shape of this
//! module.
//!
//! Children are held as `Rc<Element>` so cloning an element only copies
//! the spine (tag, attrs, children Vec); the subtrees are refcount-
//! shared. Every ancestor-hoist / inheritance loop in the SC pipeline
//! pushes parent-rendered children into descendant elements via
//! `push(el.clone())`, and the `Rc` keeps that O(1) per element.

use std::rc::Rc;

use quick_xml::XmlVersion;
use quick_xml::events::BytesStart;

/// Rodin database internal-name allocator.
///
/// This mirrors `org.rodinp.internal.core.NameGenerator`. Every XML parent
/// owns one allocator. Explicitly named and copied children are observed;
/// freshly-created children receive the next name after the greatest one.
#[derive(Debug, Default)]
pub(crate) struct RodinNameGenerator {
    greatest: Vec<u8>,
}

impl RodinNameGenerator {
    const FIRST: u8 = b'\'';
    const EXCLUDED: u8 = b'<';
    const LAST: u8 = b'~';

    pub(crate) fn observe(&mut self, name: &str) {
        let name = name.as_bytes();
        if Self::is_valid(name)
            && (name.len() > self.greatest.len()
                || (name.len() == self.greatest.len() && name > self.greatest.as_slice()))
        {
            self.greatest.clear();
            self.greatest.extend_from_slice(name);
        }
    }

    pub(crate) fn fresh(&mut self) -> String {
        for byte in self.greatest.iter_mut().rev() {
            if *byte == Self::LAST {
                *byte = Self::FIRST;
                continue;
            }
            *byte += 1;
            if *byte == Self::EXCLUDED {
                *byte += 1;
            }
            return String::from_utf8(self.greatest.clone()).expect("Rodin names are ASCII");
        }

        self.greatest.push(Self::FIRST);
        String::from_utf8(self.greatest.clone()).expect("Rodin names are ASCII")
    }

    /// Create a generated child and bind its fresh identity in one operation.
    pub(crate) fn generated(&mut self, render: impl FnOnce(String) -> Element) -> Rc<Element> {
        Rc::new(render(self.fresh()))
    }

    /// Register and return an explicitly named or copied child.
    pub(crate) fn retained(&mut self, element: Rc<Element>) -> Rc<Element> {
        self.observe(
            element
                .attr_value(attr::NAME)
                .expect("Rodin child has an internal name"),
        );
        element
    }

    fn is_valid(name: &[u8]) -> bool {
        name.iter()
            .all(|c| (Self::FIRST..=Self::LAST).contains(c) && *c != Self::EXCLUDED)
    }
}

/// Rodin's XML attribute namespace prefix. Stripped or prefixed when
/// matching attribute keys in input `.buc` / `.bum` files.
pub(crate) const NS_PREFIX: &[u8] = b"org.eventb.core.";

/// Read an attribute by `key` from `e`, accepting both the bare
/// `key` and the `org.eventb.core.`-prefixed form. Both quick-xml
/// iterator errors and unescape errors are funnelled through the
/// `err` closure, which lifts the displayable form into the caller's
/// error type.
pub(crate) fn read_attr<E>(
    e: &BytesStart<'_>,
    key: &[u8],
    mut err: impl FnMut(String) -> E,
) -> Result<Option<String>, E> {
    let prefixed = [NS_PREFIX, key].concat();
    for a in e.attributes() {
        let a = a.map_err(|e| err(e.to_string()))?;
        if a.key.as_ref() == key || a.key.as_ref() == prefixed.as_slice() {
            let v = a
                .normalized_value(XmlVersion::Implicit1_0)
                .map_err(|e| err(e.to_string()))?;
            return Ok(Some(v.into_owned()));
        }
    }
    Ok(None)
}

/// One emitted XML element.
#[derive(Debug, Clone)]
pub struct Element {
    pub tag: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Rc<Element>>,
}

impl Element {
    pub fn new(tag: impl Into<String>) -> Self {
        Self {
            tag: tag.into(),
            attrs: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Append an attribute. Order is preserved.
    pub fn attr(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attrs.push((key.into(), value.into()));
        self
    }

    /// Append a boolean attribute written as `"true"` / `"false"`.
    pub fn attr_bool(self, key: impl Into<String>, value: bool) -> Self {
        self.attr(key, if value { "true" } else { "false" })
    }

    /// Append a child. Accepts an owned [`Element`] (wrapped in a
    /// fresh `Rc`) or an existing `Rc<Element>` (forwarded as-is —
    /// the fast path used by ancestor-hoist loops).
    pub fn push(&mut self, child: impl Into<Rc<Element>>) {
        self.children.push(child.into());
    }

    /// Read an already-emitted attribute.
    pub fn attr_value(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, value)| value.as_str())
    }

    /// Render to a full XML document.
    pub fn to_document(&self) -> String {
        let mut out = String::new();
        out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n");
        self.write_to(&mut out, 0);
        out
    }

    fn write_to(&self, out: &mut String, depth: usize) {
        // Rodin's output is compact (no indentation on root or children). For
        // now we match Rodin's newline-separated, un-indented style so that
        // byte-diffing is meaningful.
        let _ = depth;
        out.push('<');
        out.push_str(&self.tag);
        for (k, v) in &self.attrs {
            out.push(' ');
            out.push_str(k);
            out.push_str("=\"");
            escape_attr(v, out);
            out.push('"');
        }
        if self.children.is_empty() {
            out.push_str("/>\n");
        } else {
            out.push_str(">\n");
            for child in &self.children {
                child.write_to(out, depth + 1);
            }
            out.push_str("</");
            out.push_str(&self.tag);
            out.push_str(">\n");
        }
    }
}

fn escape_attr(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#10;"),
            '\t' => out.push_str("&#9;"),
            _ => out.push(c),
        }
    }
}

/// Keyed helpers for common attribute names used in `.bcc`/`.bcm`. Using
/// constants rather than raw strings protects against typos.
pub mod attr {
    pub const NAME: &str = "name";
    pub const ACCURATE: &str = "org.eventb.core.accurate";
    pub const CONFIGURATION: &str = "org.eventb.core.configuration";
    pub const SOURCE: &str = "org.eventb.core.source";
    pub const SC_TARGET: &str = "org.eventb.core.scTarget";
    pub const TYPE: &str = "org.eventb.core.type";
    pub const IDENTIFIER: &str = "org.eventb.core.identifier";
    pub const LABEL: &str = "org.eventb.core.label";
    pub const PREDICATE: &str = "org.eventb.core.predicate";
    pub const ASSIGNMENT: &str = "org.eventb.core.assignment";
    pub const EXPRESSION: &str = "org.eventb.core.expression";
    pub const THEOREM: &str = "org.eventb.core.theorem";
    pub const CONVERGENCE: &str = "org.eventb.core.convergence";
    pub const EXTENDED: &str = "org.eventb.core.extended";
    pub const ABSTRACT: &str = "org.eventb.core.abstract";
    pub const CONCRETE: &str = "org.eventb.core.concrete";
    pub const COMMENT: &str = "org.eventb.core.comment";
}

/// Element tags used in `.bcc`/`.bcm`.
pub mod tag {
    pub const SC_CONTEXT_FILE: &str = "org.eventb.core.scContextFile";
    pub const SC_MACHINE_FILE: &str = "org.eventb.core.scMachineFile";
    pub const SC_INTERNAL_CONTEXT: &str = "org.eventb.core.scInternalContext";
    pub const SC_CARRIER_SET: &str = "org.eventb.core.scCarrierSet";
    pub const SC_CONSTANT: &str = "org.eventb.core.scConstant";
    pub const SC_AXIOM: &str = "org.eventb.core.scAxiom";
    pub const SC_INVARIANT: &str = "org.eventb.core.scInvariant";
    pub const SC_VARIABLE: &str = "org.eventb.core.scVariable";
    pub const SC_VARIANT: &str = "org.eventb.core.scVariant";
    pub const SC_EVENT: &str = "org.eventb.core.scEvent";
    pub const SC_GUARD: &str = "org.eventb.core.scGuard";
    pub const SC_ACTION: &str = "org.eventb.core.scAction";
    pub const SC_PARAMETER: &str = "org.eventb.core.scParameter";
    pub const SC_WITNESS: &str = "org.eventb.core.scWitness";
    pub const SC_SEES_CONTEXT: &str = "org.eventb.core.scSeesContext";
    pub const SC_REFINES_MACHINE: &str = "org.eventb.core.scRefinesMachine";
    pub const SC_REFINES_EVENT: &str = "org.eventb.core.scRefinesEvent";
    pub const SC_EXTENDS_CONTEXT: &str = "org.eventb.core.scExtendsContext";
}

/// Element tags and segment strings used when consuming `.buc`/`.bum`
/// input or constructing `source=` URIs. Mirrors [`tag`] for the
/// unchecked ("input") side, avoiding string-literal drift between
/// the reader, the rodin-id sidecar, and the handle-URI builder.
pub mod in_tag {
    // root elements
    pub const CONTEXT_FILE: &str = "org.eventb.core.contextFile";
    pub const MACHINE_FILE: &str = "org.eventb.core.machineFile";
    // children
    pub const CARRIER_SET: &str = "org.eventb.core.carrierSet";
    pub const CONSTANT: &str = "org.eventb.core.constant";
    pub const AXIOM: &str = "org.eventb.core.axiom";
    pub const INVARIANT: &str = "org.eventb.core.invariant";
    pub const VARIABLE: &str = "org.eventb.core.variable";
    pub const VARIANT: &str = "org.eventb.core.variant";
    pub const EVENT: &str = "org.eventb.core.event";
    pub const PARAMETER: &str = "org.eventb.core.parameter";
    pub const GUARD: &str = "org.eventb.core.guard";
    pub const ACTION: &str = "org.eventb.core.action";
    pub const WITNESS: &str = "org.eventb.core.witness";
    pub const EXTENDS_CONTEXT: &str = "org.eventb.core.extendsContext";
    pub const SEES_CONTEXT: &str = "org.eventb.core.seesContext";
    pub const REFINES_MACHINE: &str = "org.eventb.core.refinesMachine";
    pub const REFINES_EVENT: &str = "org.eventb.core.refinesEvent";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_roundtrip() {
        let root = Element::new(tag::SC_CONTEXT_FILE)
            .attr_bool(attr::ACCURATE, true)
            .attr(attr::CONFIGURATION, "org.eventb.core.fwd");
        let xml = root.to_document();
        assert!(xml.contains("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>"));
        assert!(xml.contains("<org.eventb.core.scContextFile"));
        assert!(xml.contains("org.eventb.core.accurate=\"true\""));
    }

    #[test]
    fn attribute_order_is_insertion_order() {
        let root = Element::new("t")
            .attr("a", "1")
            .attr("c", "3")
            .attr("b", "2");
        let xml = root.to_document();
        // Assert order: a, c, b — NOT alphabetical
        let idx_a = xml.find(" a=").unwrap();
        let idx_c = xml.find(" c=").unwrap();
        let idx_b = xml.find(" b=").unwrap();
        assert!(idx_a < idx_c && idx_c < idx_b);
    }

    #[test]
    fn nested_children_emit() {
        let mut root =
            Element::new(tag::SC_CONTEXT_FILE).attr(attr::CONFIGURATION, "org.eventb.core.fwd");
        root.push(
            Element::new(tag::SC_CARRIER_SET)
                .attr(attr::NAME, "USERS")
                .attr(attr::TYPE, "ℙ(USERS)"),
        );
        let xml = root.to_document();
        assert!(xml.contains("<org.eventb.core.scCarrierSet"));
        assert!(xml.contains("name=\"USERS\""));
        assert!(xml.contains("org.eventb.core.type=\"ℙ(USERS)\""));
        assert!(xml.ends_with("</org.eventb.core.scContextFile>\n"));
    }

    #[test]
    fn escapes_special_chars() {
        let e = Element::new("t").attr("k", "a<b&c\"d\ne");
        let xml = e.to_document();
        assert!(xml.contains("a&lt;b&amp;c&quot;d&#10;e"));
    }

    #[test]
    fn rodin_names_follow_registered_greatest_name() {
        let mut names = RodinNameGenerator::default();
        assert_eq!(names.fresh(), "'");
        assert_eq!(names.fresh(), "(");

        names.observe(";");
        assert_eq!(names.fresh(), "=");
        names.observe("~");
        assert_eq!(names.fresh(), "''");

        names.observe("invalid ");
        assert_eq!(names.fresh(), "'(");
        names.observe("floppy_ctx");
        assert_eq!(names.fresh(), "floppy_cty");
    }
}
