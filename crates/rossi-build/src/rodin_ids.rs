//! Sidecar extraction of Rodin's internal element IDs (the `name="_..."`
//! attribute) from raw `.buc` / `.bum` XML.
//!
//! Rodin stamps every element with an opaque internal id like
//! `_w4LsYO5MEeSpR9iqQeSCVw`. These ids don't affect parsing but DO appear
//! in `source=` handle URIs of the emitted `.bcc` / `.bcm` files. Preserving
//! them is the difference between our output being semantically-equivalent
//! and byte-equivalent to Rodin's.
//!
//! Rather than adding a `rodin_id` field to every `rossi::ast::*`
//! struct (a large breaking change), we extract the ids into a side table
//! keyed by `(element_kind, event_label, label_or_identifier)` and look
//! them up at emit time. If a lookup misses, the emitter falls back to the
//! identifier/label itself — semantically still correct, just not
//! byte-exact.

use std::collections::HashMap;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event as XmlEvent};
use rossi::ParseError;

use crate::xml_out::in_tag;

/// Where in the file an element lives. Mirrors the parent chain Rodin
/// uses in its handle URIs so lookups compose naturally.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum Scope {
    /// Top-level element in a context or machine file.
    File,
    /// Child of a named event (`label` = event's `org.eventb.core.label`).
    Event { label: String },
}

/// The kind of element whose Rodin id we store.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum Kind {
    CarrierSet,
    Constant,
    Axiom,
    Variable,
    Invariant,
    Variant,
    Event,
    Parameter,
    Guard,
    Action,
    Witness,
    ExtendsContext,
    SeesContext,
    RefinesMachine,
    RefinesEvent,
}

/// The lookup key.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct Key {
    pub scope: Scope,
    pub kind: Kind,
    /// For named elements (carrier set, constant, variable, parameter)
    /// this is the identifier; for labelled elements (axiom, invariant,
    /// guard, action, witness) it is the label.
    pub ident_or_label: String,
}

/// Sidecar table of Rodin internal element ids extracted from a single
/// `.buc` or `.bum` file.
#[derive(Debug, Default, Clone)]
pub struct RodinIds {
    by_key: HashMap<Key, String>,
}

impl RodinIds {
    /// Parse the sidecar from a `.buc` or `.bum` XML string.
    pub fn from_xml(xml: &str) -> Result<Self, ParseError> {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);
        let mut buf = Vec::new();
        let mut out = RodinIds::default();
        let mut current_event: Option<String> = None;

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(XmlEvent::Start(e)) => {
                    let tag = tag_name(&e)?;
                    if tag == in_tag::EVENT {
                        current_event = record_event_id(&mut out, &e)?;
                    } else {
                        record_child(&mut out, current_event.as_deref(), &tag, &e)?;
                    }
                }
                Ok(XmlEvent::End(e)) => {
                    let name_bytes = e.name();
                    let tag = std::str::from_utf8(name_bytes.as_ref())
                        .map_err(|e| ParseError::InvalidXml(e.to_string()))?;
                    if tag == in_tag::EVENT {
                        current_event = None;
                    }
                }
                Ok(XmlEvent::Empty(e)) => {
                    let tag = tag_name(&e)?;
                    if tag == in_tag::EVENT {
                        // Self-closing event (extended=true with no
                        // body). Record its id; its inner scope is
                        // empty, so we don't push `current_event`.
                        record_event_id(&mut out, &e)?;
                    } else {
                        record_child(&mut out, current_event.as_deref(), &tag, &e)?;
                    }
                }
                Ok(XmlEvent::Eof) => break,
                Err(e) => return Err(ParseError::InvalidXml(e.to_string())),
                _ => {}
            }
            buf.clear();
        }
        Ok(out)
    }

    pub fn insert(&mut self, key: Key, id: String) {
        self.by_key.insert(key, id);
    }

    pub fn get(&self, key: &Key) -> Option<&str> {
        self.by_key.get(key).map(String::as_str)
    }

    /// Convenience: look up by (scope, kind, ident/label) or fall back to
    /// the ident/label itself. This is what the emitter does when it needs
    /// an id for a `source=` URI segment.
    pub fn get_or<'a>(&'a self, scope: Scope, kind: Kind, ident_or_label: &'a str) -> &'a str {
        let key = Key {
            scope,
            kind,
            ident_or_label: ident_or_label.to_string(),
        };
        self.by_key
            .get(&key)
            .map(String::as_str)
            .unwrap_or(ident_or_label)
    }
}

/// Record an event's rodin_id in the side table and return its label
/// (so the caller can push it as the current_event scope).
fn record_event_id(out: &mut RodinIds, e: &BytesStart) -> Result<Option<String>, ParseError> {
    let label = attr(e, b"label")?.or_else(|| attr(e, b"name").ok().flatten());
    let rodin_name = attr(e, b"name")?;
    if let (Some(lbl), Some(id)) = (label.as_ref(), rodin_name) {
        out.insert(
            Key {
                scope: Scope::File,
                kind: Kind::Event,
                ident_or_label: lbl.clone(),
            },
            id,
        );
    }
    Ok(label)
}

fn record_child(
    out: &mut RodinIds,
    current_event: Option<&str>,
    tag: &str,
    e: &BytesStart,
) -> Result<(), ParseError> {
    let kind = match tag {
        in_tag::CARRIER_SET => Kind::CarrierSet,
        in_tag::CONSTANT => Kind::Constant,
        in_tag::AXIOM => Kind::Axiom,
        in_tag::VARIABLE => Kind::Variable,
        in_tag::INVARIANT => Kind::Invariant,
        in_tag::VARIANT => Kind::Variant,
        in_tag::PARAMETER => Kind::Parameter,
        in_tag::GUARD => Kind::Guard,
        in_tag::ACTION => Kind::Action,
        in_tag::WITNESS => Kind::Witness,
        in_tag::EXTENDS_CONTEXT => Kind::ExtendsContext,
        in_tag::SEES_CONTEXT => Kind::SeesContext,
        in_tag::REFINES_MACHINE => Kind::RefinesMachine,
        in_tag::REFINES_EVENT => Kind::RefinesEvent,
        _ => return Ok(()),
    };

    // Key: identifier for named elements, label for labelled ones,
    // `target` for extends/sees/refines.
    let ident_or_label = match kind {
        Kind::CarrierSet | Kind::Constant | Kind::Variable | Kind::Parameter => {
            attr(e, b"identifier")?
        }
        Kind::Axiom
        | Kind::Invariant
        | Kind::Guard
        | Kind::Action
        | Kind::Witness
        | Kind::Event
        | Kind::Variant => attr(e, b"label")?,
        Kind::ExtendsContext | Kind::SeesContext | Kind::RefinesMachine | Kind::RefinesEvent => {
            attr(e, b"target")?
        }
    };
    let rodin_name = attr(e, b"name")?;

    if let (Some(key_val), Some(id)) = (ident_or_label, rodin_name) {
        let scope = match (current_event, kind) {
            (
                Some(evt),
                Kind::Parameter | Kind::Guard | Kind::Action | Kind::Witness | Kind::RefinesEvent,
            ) => Scope::Event {
                label: evt.to_string(),
            },
            _ => Scope::File,
        };
        out.insert(
            Key {
                scope,
                kind,
                ident_or_label: key_val,
            },
            id,
        );
    }
    Ok(())
}

fn tag_name(e: &BytesStart) -> Result<String, ParseError> {
    std::str::from_utf8(e.name().as_ref())
        .map(str::to_string)
        .map_err(|e| ParseError::InvalidXml(e.to_string()))
}

fn attr(e: &BytesStart, key: &[u8]) -> Result<Option<String>, ParseError> {
    crate::xml_out::read_attr(e, key, ParseError::InvalidXml)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUCTION_BUC: &str = r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3">
<org.eventb.core.carrierSet name="_w4LsYO5MEeSpR9iqQeSCVw" org.eventb.core.identifier="USERS"/>
<org.eventb.core.carrierSet name="_qJ3S4O5PEeSpR9iqQeSCVw" org.eventb.core.identifier="AUCTIONS"/>
</org.eventb.core.contextFile>
"#;

    #[test]
    fn extracts_carrier_set_ids() {
        let ids = RodinIds::from_xml(AUCTION_BUC).unwrap();
        assert_eq!(
            ids.get_or(Scope::File, Kind::CarrierSet, "USERS"),
            "_w4LsYO5MEeSpR9iqQeSCVw"
        );
        assert_eq!(
            ids.get_or(Scope::File, Kind::CarrierSet, "AUCTIONS"),
            "_qJ3S4O5PEeSpR9iqQeSCVw"
        );
    }

    #[test]
    fn unknown_ident_falls_back_to_itself() {
        let ids = RodinIds::from_xml(AUCTION_BUC).unwrap();
        assert_eq!(
            ids.get_or(Scope::File, Kind::CarrierSet, "MISSING"),
            "MISSING"
        );
    }

    #[test]
    fn event_scoped_guards_are_keyed_by_event_label() {
        let xml = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5">
<org.eventb.core.event name="_evt1" org.eventb.core.label="Register">
  <org.eventb.core.guard name="_g1" org.eventb.core.label="grd1" org.eventb.core.predicate="u ∈ USERS"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;
        let ids = RodinIds::from_xml(xml).unwrap();
        assert_eq!(ids.get_or(Scope::File, Kind::Event, "Register"), "_evt1");
        assert_eq!(
            ids.get_or(
                Scope::Event {
                    label: "Register".into()
                },
                Kind::Guard,
                "grd1"
            ),
            "_g1"
        );
    }
}
