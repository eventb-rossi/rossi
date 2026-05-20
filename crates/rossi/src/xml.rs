//! XML parser for native Event-B format (.buc and .bum files)
//!
//! This module provides support for parsing Event-B models in their native XML format
//! as used by the Rodin platform (version 3.0 and above).
//!
//! ## File Types
//! - `.buc` files: Event-B contexts
//! - `.bum` files: Event-B machines
//!
//! ## Example
//! ```no_run
//! use rossi::parse_xml;
//!
//! let xml = r#"
//! <?xml version="1.0" encoding="UTF-8"?>
//! <org.eventb.core.contextFile version="3">
//!     <org.eventb.core.context name="counter_ctx"/>
//! </org.eventb.core.contextFile>
//! "#;
//!
//! let component = parse_xml(xml).unwrap();
//! ```

use crate::ast::context::SetDeclaration;
use crate::ast::{
    Context, Event, EventStatus, FileMetadata, InitialisationEvent, LabeledPredicate, Machine,
    NamedElement,
};
use crate::error::{ParseError, ParseResult, Result};
use crate::pretty::PrettyPrinter;
use crate::{Component, parser};
use quick_xml::events::Event as XmlEvent;
use quick_xml::{Reader, XmlVersion};
use std::io::Read as IoRead;

/// Decode XML entities in a string
/// Format an optional comment as an XML attribute string (with leading space)
fn format_comment_attr(comment: Option<&str>) -> String {
    match comment {
        Some(c) => format!(" org.eventb.core.comment=\"{}\"", escape_xml(c)),
        None => String::new(),
    }
}

/// Encode XML entities in a string
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Extract an optional string attribute from an XML element.
///
/// Tries both the exact key and the `org.eventb.core.`-prefixed version,
/// so both hand-crafted (unprefixed) and real Rodin (prefixed) XML work.
fn get_xml_attr(e: &quick_xml::events::BytesStart, key: &[u8]) -> Result<Option<String>> {
    let prefixed_key = [b"org.eventb.core." as &[u8], key].concat();
    for attr in e.attributes() {
        let attr = attr.map_err(|e| ParseError::InvalidXml(e.to_string()))?;
        if attr.key.as_ref() == key || attr.key.as_ref() == prefixed_key.as_slice() {
            let value = attr
                .normalized_value(XmlVersion::Implicit1_0)
                .map_err(|e| ParseError::InvalidXml(e.to_string()))?;
            return Ok(Some(value.into_owned()));
        }
    }
    Ok(None)
}

/// Wrap a per-file [`ParseError`] in [`ParseError::FileContext`] so the
/// failing inner filename rides alongside the original variant.
fn wrap_file_error(filename: &str, source: ParseError) -> ParseError {
    ParseError::FileContext {
        filename: filename.to_string(),
        source: Box::new(source),
    }
}

/// Extract a *required* string attribute from an XML element, surfacing a
/// structured [`ParseError::MissingXmlAttribute`] (EB003) if it's absent.
/// `key` is the unprefixed attribute name (e.g. `b"target"`); the element
/// name in the error is read from `e.name()` so callers can't drift.
fn required_attr(e: &quick_xml::events::BytesStart, key: &[u8]) -> Result<String> {
    match get_xml_attr(e, key)? {
        Some(v) => Ok(v),
        None => {
            let element = std::str::from_utf8(e.name().as_ref())
                .map_err(|err| ParseError::InvalidXml(err.to_string()))?
                .to_string();
            let attribute = std::str::from_utf8(key)
                .expect("XML attribute keys are ASCII byte literals")
                .to_string();
            Err(ParseError::MissingXmlAttribute { element, attribute })
        }
    }
}

/// Reject identifiers that our Event-B XML loader can't honestly handle.
///
/// We deliberately mirror Rodin's permissive XML naming rather than the
/// stricter text-grammar identifier rule:
///
/// - First character must be an ASCII letter or `_`. (Leading digits or
///   leading hyphens would confuse the text-grammar lexer if the name ever
///   flowed into a parsed predicate.)
/// - Subsequent characters: ASCII alphanumeric, `_`, `'`, or `-`. Rodin
///   permits hyphens in machine and context names (e.g. `A-C0`,
///   `CTX-1`); those names only appear in opaque attribute positions
///   (refines target, sees target) so the text grammar never has to lex
///   them.
/// - Keyword-named identifiers (`end`, `events`, `extends`, ...) are
///   accepted: Rodin permits them and our expression-position grammar
///   parses them as identifiers (the `kw_*` rules fire only in their
///   specific structural positions, not as a general reservation).
fn validate_identifier(name: &str, origin: &str) -> Result<()> {
    if name.is_empty() {
        return Err(ParseError::UnsupportedIdentifier {
            name: name.to_string(),
            origin: origin.to_string(),
            reason: "empty".to_string(),
        });
    }
    let first = name.chars().next().unwrap();
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(ParseError::UnsupportedIdentifier {
            name: name.to_string(),
            origin: origin.to_string(),
            reason: format!("must start with ASCII letter or '_', got {:?}", first),
        });
    }
    for c in name.chars() {
        if !(c.is_ascii_alphanumeric() || c == '_' || c == '\'' || c == '-') {
            return Err(ParseError::UnsupportedIdentifier {
                name: name.to_string(),
                origin: origin.to_string(),
                reason: format!("contains unsupported character {:?}", c),
            });
        }
    }
    Ok(())
}

/// Format a source label like "S1.bum" or "XML input" for error messages.
fn source_label(source_file: Option<&str>) -> String {
    source_file.unwrap_or("XML input").to_string()
}

/// Wrap a parser failure on an XML attribute value with its origin (file,
/// element kind, label, attribute name, raw text). The underlying pest error
/// is kept in `reason` so the user still sees it.
fn wrap_attr_error(
    origin: &str,
    element_kind: &str,
    label: Option<&str>,
    attr_name: &str,
    value: &str,
    err: ParseError,
) -> ParseError {
    let label_part = match label {
        Some(l) => format!(" (label={:?})", l),
        None => String::new(),
    };
    ParseError::MalformedAttribute {
        origin: format!("{} <{}{}>", origin, element_kind, label_part),
        label: String::new(),
        attr_name: attr_name.to_string(),
        value: value.to_string(),
        reason: err.to_string(),
    }
}

/// Parse a predicate attribute string, wrapping any error with origin context.
fn parse_predicate_attr(
    value: &str,
    origin: &str,
    element_kind: &str,
    label: Option<&str>,
    attr_name: &str,
) -> Result<crate::ast::Predicate> {
    parser::parse_predicate_str(value)
        .map_err(|e| wrap_attr_error(origin, element_kind, label, attr_name, value, e))
}

/// Parse an expression attribute string, wrapping any error with origin context.
fn parse_expression_attr(
    value: &str,
    origin: &str,
    element_kind: &str,
    label: Option<&str>,
    attr_name: &str,
) -> Result<crate::ast::Expression> {
    parser::parse_expression_str(value)
        .map_err(|e| wrap_attr_error(origin, element_kind, label, attr_name, value, e))
}

/// Parse an action (assignment) attribute string, wrapping any error with origin context.
fn parse_action_attr(
    value: &str,
    origin: &str,
    element_kind: &str,
    label: Option<&str>,
    attr_name: &str,
) -> Result<crate::ast::Action> {
    parser::parse_action_str(value)
        .map_err(|e| wrap_attr_error(origin, element_kind, label, attr_name, value, e))
}

/// Parse a labeled predicate from XML attributes (axiom or invariant element).
/// Returns `(LabeledPredicate, is_theorem)` if a predicate was found, None otherwise.
///
/// Handles both unprefixed (`label`, `predicate`) and Rodin-prefixed
/// (`org.eventb.core.label`, `org.eventb.core.predicate`) attribute names.
fn parse_xml_labeled_predicate(
    e: &quick_xml::events::BytesStart,
    origin: &str,
    element_kind: &str,
) -> Result<Option<(LabeledPredicate, bool)>> {
    let mut label = None;
    let mut predicate_str = String::new();
    let mut is_theorem = false;
    let mut comment = None;

    for attr in e.attributes() {
        let attr = attr.map_err(|e| ParseError::InvalidXml(e.to_string()))?;
        let key = std::str::from_utf8(attr.key.as_ref())
            .map_err(|e| ParseError::InvalidXml(e.to_string()))?;
        let value = attr
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|e| ParseError::InvalidXml(e.to_string()))?;

        let key = key.strip_prefix("org.eventb.core.").unwrap_or(key);
        match key {
            "label" => label = Some(value.to_string()),
            "predicate" => predicate_str = value.to_string(),
            "theorem" => is_theorem = &*value == "true",
            "comment" => comment = Some(value.to_string()),
            _ => {}
        }
    }

    if predicate_str.is_empty() {
        return Ok(None);
    }

    let predicate = parse_predicate_attr(
        &predicate_str,
        origin,
        element_kind,
        label.as_deref(),
        "predicate",
    )?;
    Ok(Some((
        LabeledPredicate {
            label,
            is_theorem,
            predicate,
            span: None,
            comment,
        },
        is_theorem,
    )))
}

/// Write labeled predicates as XML elements
fn write_labeled_predicates_xml(
    xml: &mut String,
    items: &[LabeledPredicate],
    element_name: &str,
    printer: &PrettyPrinter,
    indent: &str,
) {
    for item in items {
        let predicate_str = printer.print_predicate(&item.predicate);
        let label_attr = if let Some(label) = &item.label {
            format!(" org.eventb.core.label=\"{}\"", escape_xml(label))
        } else {
            String::new()
        };
        let comment_attr = if let Some(comment) = &item.comment {
            format!(" org.eventb.core.comment=\"{}\"", escape_xml(comment))
        } else {
            String::new()
        };
        xml.push_str(&format!(
            "{}<{} name=\"internal\"{} org.eventb.core.predicate=\"{}\" org.eventb.core.theorem=\"{}\"{}/>\n",
            indent,
            element_name,
            label_attr,
            escape_xml(&predicate_str),
            item.is_theorem,
            comment_attr
        ));
    }
}

/// Parses a native Event-B XML string into a Component
///
/// The XML can be either a context (.buc) or machine (.bum) file.
///
/// # Arguments
/// * `xml` - The XML string to parse
///
/// # Returns
/// A `Result` containing either a `Component::Context` or `Component::Machine`
///
/// # Errors
/// Returns a `ParseError` if the XML is malformed or doesn't match the Event-B schema
pub fn parse_xml(xml: &str) -> Result<Component> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut component_type: Option<ComponentType> = None;

    // First pass: determine if the document root is a context or machine.
    loop {
        let event = reader.read_event_into(&mut buf);
        let name_str = match &event {
            Ok(XmlEvent::Start(e)) | Ok(XmlEvent::Empty(e)) => Some(
                std::str::from_utf8(e.name().as_ref())
                    .map_err(|e| ParseError::InvalidXml(e.to_string()))?
                    .to_string(),
            ),
            Ok(XmlEvent::Eof) => break,
            Err(e) => return Err(ParseError::InvalidXml(e.to_string())),
            _ => None,
        };
        if let Some(name) = name_str {
            if name == "org.eventb.core.contextFile" {
                component_type = Some(ComponentType::Context);
                break;
            } else if name == "org.eventb.core.machineFile" {
                component_type = Some(ComponentType::Machine);
                break;
            } else {
                return Err(ParseError::UnexpectedXmlRoot { found: name });
            }
        }
        buf.clear();
    }

    match component_type {
        Some(ComponentType::Context) => {
            let context = parse_context_xml_with_name(xml, None, None)?;
            Ok(Component::Context(context))
        }
        Some(ComponentType::Machine) => {
            let machine = parse_machine_xml_with_name(xml, None, None)?;
            Ok(Component::Machine(machine))
        }
        None => Err(ParseError::UnexpectedXmlRoot {
            found: String::new(),
        }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComponentType {
    Context,
    Machine,
}

/// Parses a context XML (.buc file) into a Context AST node
///
/// # Arguments
/// * `xml` - The XML content
/// * `default_name` - Optional default name to use if not specified in XML
/// * `source_file` - Optional filename for error messages (e.g. "C0.buc")
fn parse_context_xml_with_name(
    xml: &str,
    default_name: Option<&str>,
    source_file: Option<&str>,
) -> Result<Context> {
    let origin = source_label(source_file);
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut context_name = default_name.unwrap_or("").to_string();
    let mut context_comment = None;
    let mut extends = Vec::new();
    let mut sets = Vec::new();
    let mut constants = Vec::new();
    let mut axioms = Vec::new();
    let mut metadata = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(e)) => {
                let name_bytes = e.name();
                let tag_name = std::str::from_utf8(name_bytes.as_ref())
                    .map_err(|e| ParseError::InvalidXml(e.to_string()))?
                    .to_string();

                if tag_name == "org.eventb.core.contextFile" {
                    let version = get_xml_attr(&e, b"version")?;
                    let configuration = get_xml_attr(&e, b"org.eventb.core.configuration")?;
                    metadata = Some(FileMetadata {
                        version,
                        configuration,
                    });
                }
            }
            Ok(XmlEvent::Empty(e)) => {
                let name_bytes = e.name();
                let tag_name = std::str::from_utf8(name_bytes.as_ref())
                    .map_err(|e| ParseError::InvalidXml(e.to_string()))?
                    .to_string();

                match tag_name.as_str() {
                    "org.eventb.core.context" => {
                        // Try `identifier` first (Rodin prefixed format), then
                        // fall back to `name` (hand-crafted files).
                        if let Some(name) = get_xml_attr(&e, b"identifier")? {
                            context_name = name;
                        } else if let Some(name) = get_xml_attr(&e, b"name")? {
                            context_name = name;
                        }
                        context_comment = get_xml_attr(&e, b"comment")?;
                    }
                    "org.eventb.core.extendsContext" => {
                        let target = required_attr(&e, b"target")?;
                        validate_identifier(&target, &format!("extends target in {}", origin))?;
                        extends.push(target);
                    }
                    "org.eventb.core.carrierSet" => {
                        if let Some(set_name) = get_xml_attr(&e, b"identifier")? {
                            validate_identifier(&set_name, &format!("carrier set in {}", origin))?;
                            let comment = get_xml_attr(&e, b"comment")?;
                            sets.push(SetDeclaration::Deferred {
                                name: set_name,
                                comment,
                            });
                        }
                    }
                    "org.eventb.core.constant" => {
                        if let Some(const_name) = get_xml_attr(&e, b"identifier")? {
                            validate_identifier(&const_name, &format!("constant in {}", origin))?;
                            let comment = get_xml_attr(&e, b"comment")?;
                            constants.push(NamedElement::with_comment(const_name, comment));
                        }
                    }
                    "org.eventb.core.axiom" => {
                        if let Some((mut labeled_pred, is_theorem)) =
                            parse_xml_labeled_predicate(&e, &origin, "axiom")?
                        {
                            labeled_pred.is_theorem = is_theorem;
                            axioms.push(labeled_pred);
                        }
                    }
                    _ => {}
                }
            }
            Ok(XmlEvent::Eof) => break,
            Err(e) => return Err(ParseError::InvalidXml(e.to_string())),
            _ => {}
        }
        buf.clear();
    }

    // If no name was found in XML or provided as default, use "unnamed_context"
    if context_name.is_empty() {
        context_name = "unnamed_context".to_string();
    } else {
        validate_identifier(&context_name, &format!("context name in {}", origin))?;
    }

    Ok(Context {
        name: context_name,
        extends,
        sets,
        constants,
        axioms,
        span: None,
        name_span: None,
        comment: context_comment,
        metadata,
    })
}

/// Parses a machine XML (.bum file) into a Machine AST node
///
/// # Arguments
/// * `xml` - The XML content
/// * `default_name` - Optional default name to use if not specified in XML
/// * `source_file` - Optional filename for error messages (e.g. "M0.bum")
fn parse_machine_xml_with_name(
    xml: &str,
    default_name: Option<&str>,
    source_file: Option<&str>,
) -> Result<Machine> {
    let origin = source_label(source_file);
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut machine_name = default_name.unwrap_or("").to_string();
    let mut machine_comment = None;
    let mut refines: Option<String> = None;
    let mut sees = Vec::new();
    let mut variables = Vec::new();
    let mut invariants = Vec::new();
    let mut variant = None;
    let mut initialisation = None;
    let mut events = Vec::new();
    let mut current_event: Option<EventBuilder> = None;
    let mut metadata = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(e)) => {
                let name_bytes = e.name();
                let tag_name = std::str::from_utf8(name_bytes.as_ref())
                    .map_err(|e| ParseError::InvalidXml(e.to_string()))?
                    .to_string();

                if tag_name == "org.eventb.core.machineFile" {
                    let version = get_xml_attr(&e, b"version")?;
                    let configuration = get_xml_attr(&e, b"org.eventb.core.configuration")?;
                    metadata = Some(FileMetadata {
                        version,
                        configuration,
                    });
                } else if tag_name == "org.eventb.core.event" {
                    // In real Rodin XML, `name` is an internal id (e.g. `'`)
                    // and `org.eventb.core.label` holds the human-readable name.
                    // Fall back to `name` for hand-crafted files that lack `label`.
                    let event_name = get_xml_attr(&e, b"label")?
                        .or(get_xml_attr(&e, b"name")?)
                        .unwrap_or_default();
                    let convergence = get_xml_attr(&e, b"convergence")?;
                    let event_comment = get_xml_attr(&e, b"comment")?;
                    let extended = get_xml_attr(&e, b"extended")?
                        .map(|v| v == "true")
                        .unwrap_or(false);

                    current_event = Some(EventBuilder {
                        name: event_name,
                        convergence,
                        comment: event_comment,
                        refines: None,
                        parameters: Vec::new(),
                        guards: Vec::new(),
                        with: Vec::new(),
                        witnesses: Vec::new(),
                        actions: Vec::new(),
                        extended,
                    });
                }
            }
            Ok(XmlEvent::Empty(e)) => {
                let name_bytes = e.name();
                let tag_name = std::str::from_utf8(name_bytes.as_ref())
                    .map_err(|e| ParseError::InvalidXml(e.to_string()))?
                    .to_string();

                match tag_name.as_str() {
                    "org.eventb.core.machine" => {
                        // Try `identifier` first (Rodin prefixed format), then
                        // fall back to `name` (hand-crafted files).
                        if let Some(name) = get_xml_attr(&e, b"identifier")? {
                            machine_name = name;
                        } else if let Some(name) = get_xml_attr(&e, b"name")? {
                            machine_name = name;
                        }
                        machine_comment = get_xml_attr(&e, b"comment")?;
                    }
                    "org.eventb.core.refinesMachine" => {
                        let target = required_attr(&e, b"target")?;
                        validate_identifier(&target, &format!("refines target in {}", origin))?;
                        refines = Some(target);
                    }
                    "org.eventb.core.seesContext" => {
                        let target = required_attr(&e, b"target")?;
                        validate_identifier(&target, &format!("sees target in {}", origin))?;
                        sees.push(target);
                    }
                    "org.eventb.core.variable" => {
                        if let Some(var_name) = get_xml_attr(&e, b"identifier")? {
                            validate_identifier(&var_name, &format!("variable in {}", origin))?;
                            let comment = get_xml_attr(&e, b"comment")?;
                            variables.push(NamedElement::with_comment(var_name, comment));
                        }
                    }
                    "org.eventb.core.invariant" => {
                        if let Some((mut labeled_pred, is_theorem)) =
                            parse_xml_labeled_predicate(&e, &origin, "invariant")?
                        {
                            labeled_pred.is_theorem = is_theorem;
                            invariants.push(labeled_pred);
                        }
                    }
                    "org.eventb.core.variant" => {
                        if let Some(expr_str) = get_xml_attr(&e, b"expression")? {
                            variant = Some(parse_expression_attr(
                                &expr_str,
                                &origin,
                                "variant",
                                None,
                                "expression",
                            )?);
                        }
                    }
                    // Self-closing `<event .../>` — common for extended
                    // events that inherit everything and add nothing of
                    // their own (e.g. `event X extends X end`). Rodin
                    // writes these as XmlEvent::Empty; the Start/End
                    // handler for `org.eventb.core.event` doesn't fire.
                    "org.eventb.core.event" => {
                        let event_name = get_xml_attr(&e, b"label")?
                            .or(get_xml_attr(&e, b"name")?)
                            .unwrap_or_default();
                        let convergence = get_xml_attr(&e, b"convergence")?;
                        let event_comment = get_xml_attr(&e, b"comment")?;
                        let extended = get_xml_attr(&e, b"extended")?
                            .map(|v| v == "true")
                            .unwrap_or(false);
                        // Build and immediately flush — no body, so we
                        // go straight from Empty to "finalise".
                        let status = match convergence.as_deref() {
                            Some("0") | None => None,
                            Some("1") => Some(EventStatus::Convergent),
                            Some("2") => Some(EventStatus::Anticipated),
                            Some(other) => {
                                return Err(ParseError::InvalidXml(format!(
                                    "Unknown event convergence value '{}' for event '{}'",
                                    other, event_name
                                )));
                            }
                        };
                        if event_name.to_uppercase() == "INITIALISATION" {
                            initialisation = Some(InitialisationEvent {
                                actions: Vec::new(),
                                comment: event_comment,
                                extended,
                                with: Vec::new(),
                                witnesses: Vec::new(),
                            });
                        } else {
                            validate_identifier(&event_name, &format!("event name in {}", origin))?;
                            events.push(Event {
                                name: event_name,
                                status,
                                refines: None,
                                parameters: Vec::new(),
                                guards: Vec::new(),
                                with: Vec::new(),
                                witnesses: Vec::new(),
                                actions: Vec::new(),
                                span: None,
                                name_span: None,
                                comment: event_comment,
                                extended,
                            });
                        }
                    }
                    // Event sub-elements
                    "org.eventb.core.refinesEvent" => {
                        if let Some(ref mut event) = current_event
                            && let Some(target) = get_xml_attr(&e, b"target")?
                        {
                            event.refines = Some(target);
                        }
                    }
                    "org.eventb.core.parameter" => {
                        if let Some(ref mut event) = current_event
                            && let Some(param) = get_xml_attr(&e, b"identifier")?
                        {
                            validate_identifier(
                                &param,
                                &format!("parameter in event {:?} of {}", event.name, origin),
                            )?;
                            let comment = get_xml_attr(&e, b"comment")?;
                            event
                                .parameters
                                .push(NamedElement::with_comment(param, comment));
                        }
                    }
                    "org.eventb.core.guard" => {
                        if let Some(ref mut event) = current_event {
                            let event_origin = format!("{} (event {:?})", origin, event.name);
                            if let Some((labeled_pred, _)) =
                                parse_xml_labeled_predicate(&e, &event_origin, "guard")?
                            {
                                event.guards.push(labeled_pred);
                            }
                        }
                    }
                    "org.eventb.core.witness" => {
                        if let Some(ref mut event) = current_event {
                            let label = get_xml_attr(&e, b"label")?;
                            let predicate_str = get_xml_attr(&e, b"predicate")?.unwrap_or_default();
                            let kind = get_xml_attr(&e, b"rossi.kind")?;

                            if !predicate_str.is_empty() {
                                let event_origin = format!("{} (event {:?})", origin, event.name);
                                let predicate = parse_predicate_attr(
                                    &predicate_str,
                                    &event_origin,
                                    "witness",
                                    label.as_deref(),
                                    "predicate",
                                )?;
                                let lp = LabeledPredicate {
                                    label,
                                    is_theorem: false,
                                    predicate,
                                    span: None,
                                    comment: None,
                                };
                                if kind.as_deref() == Some("witness") {
                                    event.witnesses.push(lp);
                                } else {
                                    event.with.push(lp); // Real Rodin XML defaults to WITH
                                }
                            }
                        }
                    }
                    "org.eventb.core.withBinding" => {
                        if let Some(ref mut event) = current_event {
                            let identifier = get_xml_attr(&e, b"identifier")?.unwrap_or_default();
                            let expression_str =
                                get_xml_attr(&e, b"expression")?.unwrap_or_default();

                            if !expression_str.is_empty() && !identifier.is_empty() {
                                validate_identifier(
                                    &identifier,
                                    &format!(
                                        "withBinding identifier in event {:?} of {}",
                                        event.name, origin
                                    ),
                                )?;
                                // Note: Rodin withBinding maps an abstract variable to a concrete
                                // witness expression. We convert it to an equality predicate
                                // "identifier = expression" for simplicity. This loses the semantic
                                // distinction between a witness binding and a logical assertion,
                                // but matches the textual Event-B representation used in WITH clauses.
                                let predicate_str = format!("{} = {}", identifier, expression_str);
                                let event_origin = format!("{} (event {:?})", origin, event.name);
                                let predicate = parse_predicate_attr(
                                    &predicate_str,
                                    &event_origin,
                                    "withBinding",
                                    Some(&identifier),
                                    "predicate",
                                )?;
                                event.with.push(LabeledPredicate {
                                    label: Some(identifier),
                                    is_theorem: false,
                                    predicate,
                                    span: None,
                                    comment: None,
                                });
                            }
                        }
                    }
                    "org.eventb.core.action" => {
                        if let Some(ref mut event) = current_event {
                            let label = get_xml_attr(&e, b"label")?;
                            let assignment_str =
                                get_xml_attr(&e, b"assignment")?.unwrap_or_default();
                            let comment = get_xml_attr(&e, b"comment")?;

                            if !assignment_str.is_empty() {
                                let event_origin = format!("{} (event {:?})", origin, event.name);
                                let action = parse_action_attr(
                                    &assignment_str,
                                    &event_origin,
                                    "action",
                                    label.as_deref(),
                                    "assignment",
                                )?;
                                event.actions.push(crate::ast::LabeledAction {
                                    label,
                                    action,
                                    span: None,
                                    comment,
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(XmlEvent::End(e)) => {
                let name_bytes = e.name();
                let tag_name = std::str::from_utf8(name_bytes.as_ref())
                    .map_err(|e| ParseError::InvalidXml(e.to_string()))?
                    .to_string();

                if tag_name == "org.eventb.core.event" {
                    // Finish the current event
                    if let Some(event_builder) = current_event.take() {
                        let status = match event_builder.convergence.as_deref() {
                            Some("0") | None => None,
                            Some("1") => Some(EventStatus::Convergent),
                            Some("2") => Some(EventStatus::Anticipated),
                            Some(other) => {
                                return Err(ParseError::InvalidXml(format!(
                                    "Unknown event convergence value '{}' for event '{}'",
                                    other, event_builder.name
                                )));
                            }
                        };

                        if event_builder.name.to_uppercase() == "INITIALISATION" {
                            initialisation = Some(InitialisationEvent {
                                actions: event_builder.actions,
                                comment: event_builder.comment,
                                extended: event_builder.extended,
                                with: event_builder.with,
                                witnesses: event_builder.witnesses,
                            });
                        } else {
                            validate_identifier(
                                &event_builder.name,
                                &format!("event name in {}", origin),
                            )?;
                            events.push(Event {
                                name: event_builder.name,
                                status,
                                refines: event_builder.refines,
                                parameters: event_builder.parameters,
                                guards: event_builder.guards,
                                with: event_builder.with,
                                witnesses: event_builder.witnesses,
                                actions: event_builder.actions,
                                span: None,
                                name_span: None,
                                comment: event_builder.comment,
                                extended: event_builder.extended,
                            });
                        }
                    }
                }
            }
            Ok(XmlEvent::Eof) => break,
            Err(e) => return Err(ParseError::InvalidXml(e.to_string())),
            _ => {}
        }
        buf.clear();
    }

    // If no name was found in XML or provided as default, use "unnamed_machine"
    if machine_name.is_empty() {
        machine_name = "unnamed_machine".to_string();
    } else {
        validate_identifier(&machine_name, &format!("machine name in {}", origin))?;
    }

    Ok(Machine {
        name: machine_name,
        refines,
        sees,
        variables,
        invariants,
        variant,
        initialisation,
        events,
        span: None,
        name_span: None,
        comment: machine_comment,
        metadata,
    })
}

/// Helper struct for building events during XML parsing
#[derive(Debug)]
struct EventBuilder {
    name: String,
    convergence: Option<String>,
    comment: Option<String>,
    refines: Option<String>,
    parameters: Vec<NamedElement>,
    guards: Vec<LabeledPredicate>,
    with: Vec<LabeledPredicate>,
    witnesses: Vec<LabeledPredicate>,
    actions: Vec<crate::ast::LabeledAction>,
    extended: bool,
}

/// A named Event-B component with its source filename
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NamedComponent {
    /// The filename (without path) where this component was found
    pub filename: String,
    /// The parsed component (Context or Machine)
    pub component: Component,
}

/// Parses all Event-B components from a zip archive
///
/// Rodin Event-B models are stored as zip archives containing .buc (context)
/// and .bum (machine) files. This function extracts and parses all Event-B
/// components found in the archive.
///
/// # Arguments
/// * `zip_data` - The raw bytes of the zip file
///
/// # Returns
/// A vector of `NamedComponent` structs, each containing a filename and
/// the parsed component
///
/// # Example
/// ```no_run
/// use rossi::parse_zip;
/// use std::fs;
///
/// let zip_data = fs::read("model.zip").unwrap();
/// let components = parse_zip(&zip_data).unwrap();
///
/// for named_comp in components {
///     println!("Found {} in file {}",
///         match named_comp.component {
///             rossi::Component::Context(ref c) => &c.name,
///             rossi::Component::Machine(ref m) => &m.name,
///         },
///         named_comp.filename
///     );
/// }
/// ```
pub fn parse_zip(zip_data: &[u8]) -> Result<Vec<NamedComponent>> {
    let cursor = std::io::Cursor::new(zip_data);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| ParseError::InvalidXml(format!("Failed to open zip archive: {}", e)))?;

    let mut components = Vec::new();

    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| ParseError::InvalidXml(format!("Failed to read zip entry: {}", e)))?;

        let filename = file.name().split('/').next_back().unwrap_or("").to_string();

        // Only process .buc and .bum files
        if filename.ends_with(".buc") || filename.ends_with(".bum") {
            let mut xml_content = String::new();
            file.read_to_string(&mut xml_content).map_err(|e| {
                ParseError::InvalidXml(format!("Failed to read {}: {}", filename, e))
            })?;

            // Extract name from filename (e.g., "C0.buc" -> "C0")
            let default_name = filename
                .strip_suffix(".buc")
                .or_else(|| filename.strip_suffix(".bum"))
                .unwrap_or(&filename);

            let component = if filename.ends_with(".buc") {
                Component::Context(parse_context_xml_with_name(
                    &xml_content,
                    Some(default_name),
                    Some(&filename),
                )?)
            } else {
                Component::Machine(parse_machine_xml_with_name(
                    &xml_content,
                    Some(default_name),
                    Some(&filename),
                )?)
            };

            components.push(NamedComponent {
                filename,
                component,
            });
        }
    }

    Ok(components)
}

/// Parses all Event-B components from a zip file on disk
///
/// This is a convenience wrapper around `parse_zip` that reads the file for you.
///
/// # Arguments
/// * `path` - Path to the zip file
///
/// # Returns
/// A vector of `NamedComponent` structs
///
/// # Example
/// ```no_run
/// use rossi::parse_zip_file;
///
/// let components = parse_zip_file("model.zip").unwrap();
/// println!("Found {} components", components.len());
/// ```
pub fn parse_zip_file<P: AsRef<std::path::Path>>(path: P) -> Result<Vec<NamedComponent>> {
    let data = std::fs::read(path)?;
    parse_zip(&data)
}

/// Parses all Event-B components from a zip archive with error recovery
///
/// Unlike `parse_zip`, this function continues parsing remaining files when
/// individual entries fail, collecting all errors while returning successfully
/// parsed components.
///
/// # Arguments
/// * `zip_data` - Raw bytes of the zip archive
///
/// # Returns
/// A `ParseResult<Vec<NamedComponent>>` containing:
/// - Successfully parsed components in `component`
/// - Any per-file errors in `errors`
///
/// If the archive itself cannot be opened, `component` is `None` and a single
/// error is returned.
///
/// # Example
/// ```no_run
/// use rossi::parse_zip_with_recovery;
/// use std::fs;
///
/// let zip_data = fs::read("model.zip").unwrap();
/// let result = parse_zip_with_recovery(&zip_data);
///
/// if let Some(components) = &result.component {
///     println!("Parsed {} components", components.len());
/// }
/// for err in result.get_errors() {
///     eprintln!("Error: {}", err);
/// }
/// ```
pub fn parse_zip_with_recovery(zip_data: &[u8]) -> ParseResult<Vec<NamedComponent>> {
    let cursor = std::io::Cursor::new(zip_data);
    let mut archive = match zip::ZipArchive::new(cursor) {
        Ok(a) => a,
        Err(e) => {
            return ParseResult::err(ParseError::InvalidXml(format!(
                "Failed to open zip archive: {}",
                e
            )));
        }
    };

    let mut components = Vec::new();
    let mut errors = Vec::new();

    for i in 0..archive.len() {
        let mut file = match archive.by_index(i) {
            Ok(f) => f,
            Err(e) => {
                errors.push(ParseError::InvalidXml(format!(
                    "Failed to read zip entry {}: {}",
                    i, e
                )));
                continue;
            }
        };

        let filename = file.name().split('/').next_back().unwrap_or("").to_string();

        // Only process .buc and .bum files
        if filename.ends_with(".buc") || filename.ends_with(".bum") {
            let mut xml_content = String::new();
            if let Err(e) = file.read_to_string(&mut xml_content) {
                errors.push(wrap_file_error(
                    &filename,
                    ParseError::IoError(e.to_string()),
                ));
                continue;
            }

            // Extract name from filename (e.g., "C0.buc" -> "C0")
            let default_name = filename
                .strip_suffix(".buc")
                .or_else(|| filename.strip_suffix(".bum"))
                .unwrap_or(&filename);

            let component = if filename.ends_with(".buc") {
                match parse_context_xml_with_name(&xml_content, Some(default_name), Some(&filename))
                {
                    Ok(ctx) => Component::Context(ctx),
                    Err(e) => {
                        errors.push(wrap_file_error(&filename, e));
                        continue;
                    }
                }
            } else {
                match parse_machine_xml_with_name(&xml_content, Some(default_name), Some(&filename))
                {
                    Ok(m) => Component::Machine(m),
                    Err(e) => {
                        errors.push(wrap_file_error(&filename, e));
                        continue;
                    }
                }
            };

            components.push(NamedComponent {
                filename,
                component,
            });
        }
    }

    ParseResult::with_errors(Some(components), errors)
}

/// Parses all Event-B components from a zip file on disk with error recovery
///
/// This is a convenience wrapper around `parse_zip_with_recovery` that reads
/// the file for you.
///
/// # Arguments
/// * `path` - Path to the zip file
///
/// # Returns
/// A `ParseResult<Vec<NamedComponent>>` with successfully parsed components and errors
///
/// # Example
/// ```no_run
/// use rossi::parse_zip_file_with_recovery;
///
/// let result = parse_zip_file_with_recovery("model.zip");
/// if result.is_ok() {
///     println!("All components parsed successfully");
/// }
/// ```
pub fn parse_zip_file_with_recovery<P: AsRef<std::path::Path>>(
    path: P,
) -> ParseResult<Vec<NamedComponent>> {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => return ParseResult::err(ParseError::IoError(e.to_string())),
    };
    parse_zip_with_recovery(&data)
}

// ============================================================================
// XML Serialization (AST to XML)
// ============================================================================

/// Converts a Component (Context or Machine) to native Event-B XML format
///
/// # Arguments
/// * `component` - The component to serialize
///
/// # Returns
/// An XML string in the native Event-B format (.buc or .bum)
///
/// # Example
/// ```
/// use rossi::{Context, Component, SetDeclaration, to_xml};
///
/// let mut ctx = Context::new("test_ctx".to_string());
/// ctx.sets.push(SetDeclaration::Deferred { name: "STATUS".to_string(), comment: None });
///
/// let xml = to_xml(&Component::Context(ctx));
/// assert!(xml.contains("<org.eventb.core.contextFile"));
/// ```
pub fn to_xml(component: &Component) -> String {
    match component {
        Component::Context(ctx) => context_to_xml(ctx),
        Component::Machine(machine) => machine_to_xml(machine),
    }
}

/// Returns the canonical filename for a component based on its type and name.
///
/// Context "foo" becomes "foo.buc", Machine "foo" becomes "foo.bum".
///
/// # Example
/// ```
/// use rossi::{Component, Context, component_filename};
///
/// let ctx = Context::new("counter_ctx".to_string());
/// let filename = component_filename(&Component::Context(ctx));
/// assert_eq!(filename, "counter_ctx.buc");
/// ```
pub fn component_filename(component: &Component) -> String {
    match component {
        Component::Context(ctx) => format!("{}.buc", ctx.name),
        Component::Machine(m) => format!("{}.bum", m.name),
    }
}

/// Creates a zip archive in memory from a slice of named components.
///
/// Each component is serialized to its Rodin XML format via [`to_xml`] and
/// stored in the archive under its [`NamedComponent::filename`].
///
/// # Example
/// ```
/// use rossi::{Component, Context, NamedComponent, to_zip, parse_zip};
///
/// let ctx = Context::new("test".to_string());
/// let named = NamedComponent {
///     filename: "test.buc".to_string(),
///     component: Component::Context(ctx),
/// };
/// let zip_data = to_zip(&[named]).unwrap();
/// let parsed = parse_zip(&zip_data).unwrap();
/// assert_eq!(parsed.len(), 1);
/// ```
pub fn to_zip(components: &[NamedComponent]) -> Result<Vec<u8>> {
    use std::io::Write;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let mut buf = Vec::new();
    {
        let mut writer = ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

        for named in components {
            let xml = to_xml(&named.component);
            writer
                .start_file(&named.filename, options)
                .map_err(|e| ParseError::IoError(format!("Failed to write zip entry: {}", e)))?;
            writer.write_all(xml.as_bytes())?;
        }
        writer
            .finish()
            .map_err(|e| ParseError::IoError(format!("Failed to finalize zip: {}", e)))?;
    }
    Ok(buf)
}

/// Creates a zip archive from named components and writes it to a file.
///
/// This is a convenience wrapper around [`to_zip`] followed by [`std::fs::write`].
///
/// # Example
/// ```no_run
/// use rossi::{Component, Context, NamedComponent, write_zip_file};
///
/// let ctx = Context::new("test".to_string());
/// let named = NamedComponent {
///     filename: "test.buc".to_string(),
///     component: Component::Context(ctx),
/// };
/// write_zip_file("output.zip", &[named]).unwrap();
/// ```
pub fn write_zip_file<P: AsRef<std::path::Path>>(
    path: P,
    components: &[NamedComponent],
) -> Result<()> {
    let data = to_zip(components)?;
    std::fs::write(path, data)?;
    Ok(())
}

/// Converts a Context to .buc XML format
fn context_to_xml(ctx: &Context) -> String {
    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");

    // Use metadata if available, otherwise defaults
    let version = ctx
        .metadata
        .as_ref()
        .and_then(|m| m.version.as_deref())
        .unwrap_or("3");
    let configuration = ctx
        .metadata
        .as_ref()
        .and_then(|m| m.configuration.as_deref())
        .unwrap_or("org.eventb.core.fwd");
    xml.push_str(&format!(
        "<org.eventb.core.contextFile version=\"{}\" org.eventb.core.configuration=\"{}\">\n",
        escape_xml(version),
        escape_xml(configuration)
    ));

    // Context identity element (preserves name through standalone XML roundtrip)
    let comment_attr = format_comment_attr(ctx.comment.as_deref());
    xml.push_str(&format!(
        "    <org.eventb.core.context name=\"internal\" org.eventb.core.identifier=\"{}\"{}/>\n",
        escape_xml(&ctx.name),
        comment_attr
    ));

    // Extends
    for extended in &ctx.extends {
        xml.push_str(&format!(
            "    <org.eventb.core.extendsContext name=\"internal\" org.eventb.core.target=\"{}\"/>\n",
            escape_xml(extended)
        ));
    }

    // Sets
    for set in &ctx.sets {
        let set_comment = format_comment_attr(set.comment());
        xml.push_str(&format!(
            "    <org.eventb.core.carrierSet name=\"internal\" org.eventb.core.identifier=\"{}\"{}/>\n",
            escape_xml(set.name()),
            set_comment
        ));
    }

    // Constants
    for constant in &ctx.constants {
        let const_comment = format_comment_attr(constant.comment.as_deref());
        xml.push_str(&format!(
            "    <org.eventb.core.constant name=\"internal\" org.eventb.core.identifier=\"{}\"{}/>\n",
            escape_xml(&constant.name),
            const_comment
        ));
    }

    // Axioms and theorems (theorems have is_theorem = true)
    let printer = PrettyPrinter::new();
    write_labeled_predicates_xml(
        &mut xml,
        &ctx.axioms,
        "org.eventb.core.axiom",
        &printer,
        "    ",
    );

    xml.push_str("</org.eventb.core.contextFile>\n");
    xml
}

/// Converts a Machine to .bum XML format
fn machine_to_xml(machine: &Machine) -> String {
    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");

    // Use metadata if available, otherwise defaults
    let version = machine
        .metadata
        .as_ref()
        .and_then(|m| m.version.as_deref())
        .unwrap_or("5");
    let configuration = machine
        .metadata
        .as_ref()
        .and_then(|m| m.configuration.as_deref())
        .unwrap_or("org.eventb.core.fwd");
    xml.push_str(&format!(
        "<org.eventb.core.machineFile version=\"{}\" org.eventb.core.configuration=\"{}\">\n",
        escape_xml(version),
        escape_xml(configuration)
    ));

    // Machine identity element (preserves name through standalone XML roundtrip)
    let comment_attr = format_comment_attr(machine.comment.as_deref());
    xml.push_str(&format!(
        "    <org.eventb.core.machine name=\"internal\" org.eventb.core.identifier=\"{}\"{}/>\n",
        escape_xml(&machine.name),
        comment_attr
    ));

    // Refines
    if let Some(ref refined) = machine.refines {
        xml.push_str(&format!(
            "    <org.eventb.core.refinesMachine name=\"internal\" org.eventb.core.target=\"{}\"/>\n",
            escape_xml(refined)
        ));
    }

    // Sees
    for seen in &machine.sees {
        xml.push_str(&format!(
            "    <org.eventb.core.seesContext name=\"internal\" org.eventb.core.target=\"{}\"/>\n",
            escape_xml(seen)
        ));
    }

    // Variables
    for variable in &machine.variables {
        let var_comment = format_comment_attr(variable.comment.as_deref());
        xml.push_str(&format!(
            "    <org.eventb.core.variable name=\"internal\" org.eventb.core.identifier=\"{}\"{}/>\n",
            escape_xml(&variable.name),
            var_comment
        ));
    }

    // Invariants and theorems (theorems have is_theorem = true)
    let printer = PrettyPrinter::new();
    write_labeled_predicates_xml(
        &mut xml,
        &machine.invariants,
        "org.eventb.core.invariant",
        &printer,
        "    ",
    );

    // Variant
    if let Some(variant) = &machine.variant {
        let expr_str = printer.print_expression(variant);
        xml.push_str(&format!(
            "    <org.eventb.core.variant name=\"internal\" org.eventb.core.expression=\"{}\"/>\n",
            escape_xml(&expr_str)
        ));
    }

    // INITIALISATION event
    if let Some(init) = &machine.initialisation {
        let init_comment = format_comment_attr(init.comment.as_deref());
        let extended_str = if init.extended { "true" } else { "false" };
        xml.push_str(&format!(
            "    <org.eventb.core.event name=\"internal\" org.eventb.core.convergence=\"0\" org.eventb.core.extended=\"{}\" org.eventb.core.label=\"INITIALISATION\"{}>\n",
            extended_str, init_comment
        ));
        for lp in &init.with {
            write_witness_xml(&mut xml, lp, &printer, "        ", false);
        }
        for lp in &init.witnesses {
            write_witness_xml(&mut xml, lp, &printer, "        ", true);
        }
        for action in &init.actions {
            write_action_xml(&mut xml, action, &printer, "        ");
        }
        xml.push_str("    </org.eventb.core.event>\n");
    }

    // Events
    for event in &machine.events {
        write_event_xml(&mut xml, event, &printer);
    }

    xml.push_str("</org.eventb.core.machineFile>\n");
    xml
}

/// Helper function to write an event to XML
fn write_event_xml(xml: &mut String, event: &Event, printer: &PrettyPrinter) {
    // Event opening tag with convergence attribute
    let convergence = match event.status {
        Some(EventStatus::Ordinary) | None => "0",
        Some(EventStatus::Convergent) => "1",
        Some(EventStatus::Anticipated) => "2",
    };

    let extended_str = if event.extended { "true" } else { "false" };
    let event_comment = format_comment_attr(event.comment.as_deref());
    xml.push_str(&format!(
        "    <org.eventb.core.event name=\"internal\" org.eventb.core.convergence=\"{}\" org.eventb.core.extended=\"{}\" org.eventb.core.label=\"{}\"{}>\n",
        convergence,
        extended_str,
        escape_xml(&event.name),
        event_comment
    ));

    // Refines
    if let Some(ref refined) = event.refines {
        xml.push_str(&format!(
            "        <org.eventb.core.refinesEvent name=\"internal\" org.eventb.core.target=\"{}\"/>\n",
            escape_xml(refined)
        ));
    }

    // Parameters
    for param in &event.parameters {
        let param_comment = format_comment_attr(param.comment.as_deref());
        xml.push_str(&format!(
            "        <org.eventb.core.parameter name=\"internal\" org.eventb.core.identifier=\"{}\"{}/>\n",
            escape_xml(&param.name),
            param_comment
        ));
    }

    // Guards
    for guard in &event.guards {
        let predicate_str = printer.print_predicate(&guard.predicate);
        let label_attr = if let Some(label) = &guard.label {
            format!(" org.eventb.core.label=\"{}\"", escape_xml(label))
        } else {
            String::new()
        };
        let guard_comment = format_comment_attr(guard.comment.as_deref());
        xml.push_str(&format!(
            "        <org.eventb.core.guard name=\"internal\"{} org.eventb.core.predicate=\"{}\"{}/>\n",
            label_attr,
            escape_xml(&predicate_str),
            guard_comment
        ));
    }

    for lp in &event.with {
        write_witness_xml(xml, lp, printer, "        ", false);
    }
    for lp in &event.witnesses {
        write_witness_xml(xml, lp, printer, "        ", true);
    }

    // Actions
    for action in &event.actions {
        write_action_xml(xml, action, printer, "        ");
    }

    xml.push_str("    </org.eventb.core.event>\n");
}

/// Helper function to write a witness predicate to XML.
/// `kind_witness` distinguishes the WITNESS clause (tagged with
/// `rossi.kind="witness"` for round-trip) from the default WITH
/// channel that real Rodin XML uses.
fn write_witness_xml(
    xml: &mut String,
    lp: &crate::ast::LabeledPredicate,
    printer: &PrettyPrinter,
    indent: &str,
    kind_witness: bool,
) {
    let predicate_str = printer.print_predicate(&lp.predicate);
    let label_attr = lp
        .label
        .as_deref()
        .map(|l| format!(" org.eventb.core.label=\"{}\"", escape_xml(l)))
        .unwrap_or_default();
    let kind_attr = if kind_witness {
        " rossi.kind=\"witness\""
    } else {
        ""
    };
    xml.push_str(&format!(
        "{}<org.eventb.core.witness name=\"internal\"{} org.eventb.core.predicate=\"{}\"{}/>\n",
        indent,
        label_attr,
        escape_xml(&predicate_str),
        kind_attr,
    ));
}

/// Helper function to write an action to XML
fn write_action_xml(
    xml: &mut String,
    action: &crate::ast::LabeledAction,
    printer: &PrettyPrinter,
    indent: &str,
) {
    let action_str = printer.print_action(&action.action);
    let label_attr = if let Some(label) = &action.label {
        format!(" org.eventb.core.label=\"{}\"", escape_xml(label))
    } else {
        String::new()
    };
    let comment_attr = format_comment_attr(action.comment.as_deref());
    xml.push_str(&format!(
        "{}<org.eventb.core.action name=\"internal\"{} org.eventb.core.assignment=\"{}\"{}/>\n",
        indent,
        label_attr,
        escape_xml(&action_str),
        comment_attr
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_context_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
    <org.eventb.core.context name="test_ctx"/>
</org.eventb.core.contextFile>"#;

        let result = parse_xml(xml);
        assert!(result.is_ok(), "Parse error: {:?}", result.err());

        if let Component::Context(ctx) = result.unwrap() {
            assert_eq!(ctx.name, "test_ctx");
        } else {
            panic!("Expected Context component");
        }
    }

    #[test]
    fn test_parse_context_with_sets_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
    <org.eventb.core.context name="counter_ctx"/>
    <org.eventb.core.carrierSet identifier="STATUS"/>
    <org.eventb.core.carrierSet identifier="PERSON"/>
</org.eventb.core.contextFile>"#;

        let result = parse_xml(xml);
        assert!(result.is_ok(), "Parse error: {:?}", result.err());

        if let Component::Context(ctx) = result.unwrap() {
            assert_eq!(ctx.name, "counter_ctx");
            assert_eq!(ctx.sets.len(), 2);
            assert_eq!(ctx.sets[0].name(), "STATUS");
            assert_eq!(ctx.sets[1].name(), "PERSON");
        } else {
            panic!("Expected Context component");
        }
    }

    #[test]
    fn test_parse_simple_machine_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.machine name="counter"/>
</org.eventb.core.machineFile>"#;

        let result = parse_xml(xml);
        assert!(result.is_ok(), "Parse error: {:?}", result.err());

        if let Component::Machine(m) = result.unwrap() {
            assert_eq!(m.name, "counter");
        } else {
            panic!("Expected Machine component");
        }
    }

    // ========================================================================
    // XML Serialization Tests
    // ========================================================================

    #[test]
    fn test_escape_xml() {
        assert_eq!(escape_xml("a & b"), "a &amp; b");
        assert_eq!(escape_xml("a < b"), "a &lt; b");
        assert_eq!(escape_xml("a > b"), "a &gt; b");
        assert_eq!(escape_xml("a \"b\" c"), "a &quot;b&quot; c");
        assert_eq!(escape_xml("a 'b' c"), "a &apos;b&apos; c");
        assert_eq!(escape_xml("a &lt; b"), "a &amp;lt; b");
    }

    #[test]
    fn test_simple_context_to_xml() {
        let ctx = Context {
            name: "test_ctx".to_string(),
            extends: vec![],
            sets: vec![],
            constants: vec![],
            axioms: vec![],

            span: None,
            name_span: None,
            comment: None,
            metadata: None,
        };

        let xml = to_xml(&Component::Context(ctx));
        assert!(xml.contains("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(xml.contains("<org.eventb.core.contextFile"));
        assert!(xml.contains("version=\"3\""));
        assert!(xml.contains("org.eventb.core.identifier=\"test_ctx\""));
        assert!(xml.contains("</org.eventb.core.contextFile>"));
    }

    #[test]
    fn test_context_with_sets_to_xml() {
        let ctx = Context {
            name: "counter_ctx".to_string(),
            extends: vec![],
            sets: vec![
                SetDeclaration::Deferred {
                    name: "STATUS".to_string(),
                    comment: None,
                },
                SetDeclaration::Deferred {
                    name: "PERSON".to_string(),
                    comment: None,
                },
            ],
            constants: vec![],
            axioms: vec![],

            span: None,
            name_span: None,
            comment: None,
            metadata: None,
        };

        let xml = to_xml(&Component::Context(ctx));
        assert!(xml.contains("org.eventb.core.identifier=\"STATUS\""));
        assert!(xml.contains("org.eventb.core.identifier=\"PERSON\""));
    }

    #[test]
    fn test_context_with_axioms_to_xml() {
        use crate::ast::Predicate;

        let ctx = Context {
            name: "test_ctx".to_string(),
            extends: vec![],
            sets: vec![],
            constants: vec![NamedElement::new("max_value".to_string())],
            axioms: vec![LabeledPredicate {
                label: Some("axm1".to_string()),
                is_theorem: false,
                predicate: Predicate::Comparison {
                    op: crate::ast::predicate::ComparisonOp::Equal,
                    left: crate::ast::Expression::Identifier("max_value".to_string()),
                    right: crate::ast::Expression::Integer(100),
                },
                span: None,
                comment: None,
            }],

            span: None,
            name_span: None,
            comment: None,
            metadata: None,
        };

        let xml = to_xml(&Component::Context(ctx));
        assert!(xml.contains("org.eventb.core.identifier=\"max_value\""));
        assert!(xml.contains("org.eventb.core.label=\"axm1\""));
        assert!(xml.contains("org.eventb.core.predicate=\"max_value = 100\""));
        assert!(xml.contains("org.eventb.core.theorem=\"false\""));
    }

    #[test]
    fn test_simple_machine_to_xml() {
        let machine = Machine {
            name: "counter".to_string(),
            refines: None,
            sees: vec![],
            variables: vec![],
            invariants: vec![],

            variant: None,
            initialisation: None,
            events: vec![],
            span: None,
            name_span: None,
            comment: None,
            metadata: None,
        };

        let xml = to_xml(&Component::Machine(machine));
        assert!(xml.contains("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(xml.contains("<org.eventb.core.machineFile"));
        assert!(xml.contains("version=\"5\""));
        assert!(xml.contains("org.eventb.core.identifier=\"counter\""));
        assert!(xml.contains("</org.eventb.core.machineFile>"));
    }

    #[test]
    fn test_machine_with_variables_to_xml() {
        let machine = Machine {
            name: "counter".to_string(),
            refines: None,
            sees: vec!["counter_ctx".to_string()],
            variables: vec![NamedElement::new("count".to_string())],
            invariants: vec![],

            variant: None,
            initialisation: None,
            events: vec![],
            span: None,
            name_span: None,
            comment: None,
            metadata: None,
        };

        let xml = to_xml(&Component::Machine(machine));
        assert!(xml.contains("org.eventb.core.target=\"counter_ctx\""));
        assert!(xml.contains("org.eventb.core.identifier=\"count\""));
    }

    #[test]
    fn test_machine_with_initialisation_to_xml() {
        use crate::ast::{Action, LabeledAction};

        let machine = Machine {
            name: "counter".to_string(),
            refines: None,
            sees: vec![],
            variables: vec![NamedElement::new("count".to_string())],
            invariants: vec![],

            variant: None,
            initialisation: Some(InitialisationEvent {
                actions: vec![LabeledAction {
                    label: Some("act1".to_string()),
                    action: Action::Assignment {
                        variables: vec!["count".to_string()],
                        expressions: vec![crate::ast::Expression::Integer(0)],
                    },
                    span: None,
                    comment: None,
                }],
                comment: None,
                extended: false,
                with: Vec::new(),
                witnesses: Vec::new(),
            }),
            events: vec![],
            span: None,
            name_span: None,
            comment: None,
            metadata: None,
        };

        let xml = to_xml(&Component::Machine(machine));
        assert!(xml.contains("org.eventb.core.label=\"INITIALISATION\""));
        assert!(xml.contains("org.eventb.core.label=\"act1\""));
        assert!(xml.contains("org.eventb.core.assignment=\"count \u{2254} 0\""));
    }

    #[test]
    fn test_machine_with_event_to_xml() {
        use crate::ast::{Action, LabeledAction, Predicate};

        let event = Event {
            name: "increment".to_string(),
            status: Some(EventStatus::Ordinary),
            refines: None,
            parameters: vec![],
            guards: vec![LabeledPredicate {
                label: Some("grd1".to_string()),
                is_theorem: false,
                predicate: Predicate::Comparison {
                    op: crate::ast::predicate::ComparisonOp::LessThan,
                    left: crate::ast::Expression::Identifier("count".to_string()),
                    right: crate::ast::Expression::Identifier("max_value".to_string()),
                },
                span: None,
                comment: None,
            }],
            with: vec![],
            witnesses: vec![],
            actions: vec![LabeledAction {
                label: Some("act1".to_string()),
                action: Action::Assignment {
                    variables: vec!["count".to_string()],
                    expressions: vec![crate::ast::Expression::Binary {
                        op: crate::ast::expression::BinaryOp::Add,
                        left: Box::new(crate::ast::Expression::Identifier("count".to_string())),
                        right: Box::new(crate::ast::Expression::Integer(1)),
                    }],
                },
                span: None,
                comment: None,
            }],
            span: None,
            name_span: None,
            comment: None,
            extended: false,
        };

        let machine = Machine {
            name: "counter".to_string(),
            refines: None,
            sees: vec![],
            variables: vec![NamedElement::new("count".to_string())],
            invariants: vec![],

            variant: None,
            initialisation: None,
            events: vec![event],
            span: None,
            name_span: None,
            comment: None,
            metadata: None,
        };

        let xml = to_xml(&Component::Machine(machine));
        assert!(xml.contains("<org.eventb.core.event"));
        assert!(xml.contains("org.eventb.core.convergence=\"0\""));
        assert!(xml.contains("<org.eventb.core.guard"));
        assert!(xml.contains("org.eventb.core.predicate=\"count &lt; max_value\""));
        assert!(xml.contains("org.eventb.core.assignment=\"count \u{2254} count + 1\""));
    }

    #[test]
    fn test_roundtrip_simple_context() {
        let original_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
    <org.eventb.core.context name="test_ctx"/>
    <org.eventb.core.carrierSet identifier="STATUS"/>
</org.eventb.core.contextFile>"#;

        // Parse XML to AST
        let component = parse_xml(original_xml).expect("Failed to parse original XML");

        // Convert AST back to XML
        let serialized_xml = to_xml(&component);

        // Parse the serialized XML again
        let reparsed = parse_xml(&serialized_xml).expect("Failed to parse serialized XML");

        // Compare the AST structures
        match (&component, &reparsed) {
            (Component::Context(ctx1), Component::Context(ctx2)) => {
                assert_eq!(ctx1.name, ctx2.name);
                assert_eq!(ctx1.sets, ctx2.sets);
            }
            _ => panic!("Expected Context components"),
        }
    }

    #[test]
    fn test_roundtrip_counter_example() {
        // Read the actual example file
        let original_xml = include_str!("../examples/counter_ctx.buc");

        // Parse XML to AST
        let component = parse_xml(original_xml).expect("Failed to parse counter_ctx.buc");

        // Convert AST back to XML
        let serialized_xml = to_xml(&component);

        // Parse the serialized XML again
        let reparsed = parse_xml(&serialized_xml).expect("Failed to parse serialized XML");

        // Compare the AST structures
        match (&component, &reparsed) {
            (Component::Context(ctx1), Component::Context(ctx2)) => {
                assert_eq!(ctx1.name, ctx2.name);
                assert_eq!(ctx1.sets, ctx2.sets);
                assert_eq!(ctx1.constants, ctx2.constants);
                assert_eq!(ctx1.axioms.len(), ctx2.axioms.len());
            }
            _ => panic!("Expected Context components"),
        }
    }

    #[test]
    fn test_roundtrip_machine_example() {
        // Read the actual example file
        let original_xml = include_str!("../examples/counter.bum");

        // Parse XML to AST
        let component = parse_xml(original_xml).expect("Failed to parse counter.bum");

        // Convert AST back to XML
        let serialized_xml = to_xml(&component);

        // Parse the serialized XML again
        let reparsed = parse_xml(&serialized_xml).expect("Failed to parse serialized XML");

        // Compare the AST structures
        match (&component, &reparsed) {
            (Component::Machine(m1), Component::Machine(m2)) => {
                assert_eq!(m1.name, m2.name);
                assert_eq!(m1.sees, m2.sees);
                assert_eq!(m1.variables, m2.variables);
                assert_eq!(m1.invariants.len(), m2.invariants.len());
                assert_eq!(m1.events.len(), m2.events.len());
            }
            _ => panic!("Expected Machine components"),
        }
    }

    #[test]
    fn test_xml_escaping_in_predicates() {
        use crate::ast::Predicate;

        let ctx = Context {
            name: "test".to_string(),
            extends: vec![],
            sets: vec![],
            constants: vec![],
            axioms: vec![LabeledPredicate {
                label: Some("axm1".to_string()),
                is_theorem: false,
                predicate: Predicate::Comparison {
                    op: crate::ast::predicate::ComparisonOp::GreaterThan,
                    left: crate::ast::Expression::Identifier("x".to_string()),
                    right: crate::ast::Expression::Integer(0),
                },
                span: None,
                comment: None,
            }],

            span: None,
            name_span: None,
            comment: None,
            metadata: None,
        };

        let xml = to_xml(&Component::Context(ctx));
        // Should escape > as &gt;
        assert!(xml.contains("&gt;"));
        assert!(!xml.contains("x > 0")); // Should not contain unescaped >
    }

    #[test]
    fn test_text_to_xml_conversion() {
        // Parse textual Event-B format
        let text_context = r#"
        CONTEXT test_ctx
        SETS
            STATUS
        CONSTANTS
            max_value
        AXIOMS
            @axm1 max_value = 100
        END
        "#;

        let component = crate::parser::parse(text_context).expect("Failed to parse text");
        let xml = to_xml(&component);

        // Verify it produces valid XML
        assert!(xml.contains("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(xml.contains("<org.eventb.core.contextFile"));
        assert!(xml.contains("org.eventb.core.identifier=\"test_ctx\""));
        assert!(xml.contains("org.eventb.core.identifier=\"STATUS\""));
        assert!(xml.contains("org.eventb.core.identifier=\"max_value\""));
        assert!(xml.contains("org.eventb.core.label=\"axm1\""));

        // Verify round-trip: text → XML → AST should match original AST
        let reparsed = parse_xml(&xml).expect("Failed to reparse XML");
        if let Component::Context(ctx) = reparsed {
            assert_eq!(ctx.name, "test_ctx");
        } else {
            panic!("Expected Context component");
        }
    }

    #[test]
    fn test_text_machine_to_xml_conversion() {
        // Parse textual Event-B machine format
        let text_machine = r#"
        MACHINE counter
        VARIABLES
            count
        INVARIANTS
            @inv1 count >= 0
        EVENTS
            EVENT INITIALISATION
            THEN
                count := 0
            END

            EVENT increment
            THEN
                count := count + 1
            END
        END
        "#;

        let component = crate::parser::parse(text_machine).expect("Failed to parse text");
        let xml = to_xml(&component);

        // Verify it produces valid XML
        assert!(xml.contains("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(xml.contains("<org.eventb.core.machineFile"));
        assert!(xml.contains("org.eventb.core.identifier=\"counter\""));
        assert!(xml.contains("org.eventb.core.identifier=\"count\""));
        assert!(xml.contains("<org.eventb.core.invariant"));
        assert!(xml.contains("org.eventb.core.label=\"INITIALISATION\""));
        assert!(xml.contains("org.eventb.core.label=\"increment\""));

        // Verify round-trip
        let reparsed = parse_xml(&xml).expect("Failed to reparse XML");
        if let Component::Machine(m2) = reparsed {
            assert_eq!(m2.name, "counter");
            assert_eq!(m2.variables, vec![NamedElement::new("count".to_string())]);
            assert_eq!(m2.events.len(), 1);
        } else {
            panic!("Expected Machine component");
        }
    }

    #[test]
    fn test_event_convergence_status() {
        use crate::ast::{Action, LabeledAction};

        let ordinary_event = Event {
            name: "evt1".to_string(),
            status: Some(EventStatus::Ordinary),
            refines: None,
            parameters: vec![],
            guards: vec![],
            with: vec![],
            witnesses: vec![],
            actions: vec![LabeledAction {
                label: None,
                action: Action::Assignment {
                    variables: vec!["x".to_string()],
                    expressions: vec![crate::ast::Expression::Integer(1)],
                },
                span: None,
                comment: None,
            }],
            span: None,
            name_span: None,
            comment: None,
            extended: false,
        };

        let convergent_event = Event {
            name: "evt2".to_string(),
            status: Some(EventStatus::Convergent),
            refines: None,
            parameters: vec![],
            guards: vec![],
            with: vec![],
            witnesses: vec![],
            actions: vec![],
            span: None,
            name_span: None,
            comment: None,
            extended: false,
        };

        let anticipated_event = Event {
            name: "evt3".to_string(),
            status: Some(EventStatus::Anticipated),
            refines: None,
            parameters: vec![],
            guards: vec![],
            with: vec![],
            witnesses: vec![],
            actions: vec![],
            span: None,
            name_span: None,
            comment: None,
            extended: false,
        };

        let machine = Machine {
            name: "test".to_string(),
            refines: None,
            sees: vec![],
            variables: vec![],
            invariants: vec![],

            variant: None,
            initialisation: None,
            events: vec![ordinary_event, convergent_event, anticipated_event],
            span: None,
            name_span: None,
            comment: None,
            metadata: None,
        };

        let xml = to_xml(&Component::Machine(machine));
        assert!(xml.contains("org.eventb.core.label=\"evt1\""));
        assert!(xml.contains("org.eventb.core.convergence=\"0\""));
        assert!(xml.contains("org.eventb.core.label=\"evt2\""));
        assert!(xml.contains("org.eventb.core.convergence=\"1\""));
        assert!(xml.contains("org.eventb.core.label=\"evt3\""));
        assert!(xml.contains("org.eventb.core.convergence=\"2\""));
    }

    #[test]
    fn test_xml_with_binding_write() {
        use crate::ast::Predicate;
        use crate::ast::predicate::ComparisonOp;

        let event = Event {
            name: "refine_evt".to_string(),
            status: Some(EventStatus::Ordinary),
            refines: Some("abstract_evt".to_string()),
            parameters: vec![],
            guards: vec![],
            with: vec![LabeledPredicate {
                label: Some("x".to_string()),
                is_theorem: false,
                predicate: Predicate::Comparison {
                    op: ComparisonOp::Equal,
                    left: crate::ast::Expression::Identifier("x".to_string()),
                    right: crate::ast::Expression::Identifier("y".to_string()),
                },
                span: None,
                comment: None,
            }],
            witnesses: vec![],
            actions: vec![],
            span: None,
            name_span: None,
            comment: None,
            extended: false,
        };

        let machine = Machine {
            name: "test".to_string(),
            refines: None,
            sees: vec![],
            variables: vec![],
            invariants: vec![],

            variant: None,
            initialisation: None,
            events: vec![event],
            span: None,
            name_span: None,
            comment: None,
            metadata: None,
        };

        let xml = to_xml(&Component::Machine(machine));
        // WITH bindings are serialized as witness elements (default, no kind marker)
        assert!(xml.contains("org.eventb.core.witness"));
        assert!(xml.contains("org.eventb.core.label=\"x\""));
        assert!(xml.contains("org.eventb.core.predicate=\"x = y\""));
        assert!(
            !xml.contains("rossi.kind"),
            "WITH should not have a kind attribute (it's the default)"
        );
    }

    #[test]
    fn test_xml_with_binding_roundtrip() {
        use crate::ast::Predicate;
        use crate::ast::predicate::ComparisonOp;

        let event = Event {
            name: "refine_evt".to_string(),
            status: Some(EventStatus::Ordinary),
            refines: Some("abstract_evt".to_string()),
            parameters: vec![],
            guards: vec![],
            with: vec![LabeledPredicate {
                label: Some("x".to_string()),
                is_theorem: false,
                predicate: Predicate::Comparison {
                    op: ComparisonOp::Equal,
                    left: crate::ast::Expression::Identifier("x".to_string()),
                    right: crate::ast::Expression::Identifier("y".to_string()),
                },
                span: None,
                comment: None,
            }],
            witnesses: vec![],
            actions: vec![],
            span: None,
            name_span: None,
            comment: None,
            extended: false,
        };

        let machine = Machine {
            name: "test".to_string(),
            refines: None,
            sees: vec![],
            variables: vec![],
            invariants: vec![],

            variant: None,
            initialisation: None,
            events: vec![event],
            span: None,
            name_span: None,
            comment: None,
            metadata: None,
        };

        // Write to XML and parse back
        let xml = to_xml(&Component::Machine(machine));
        let reparsed = parse_xml(&xml).expect("Failed to reparse XML");

        // WITH bindings survive the roundtrip via rossi.kind attribute
        if let Component::Machine(m) = reparsed {
            assert_eq!(m.events.len(), 1);
            let evt = &m.events[0];
            assert_eq!(evt.name, "refine_evt");
            assert_eq!(evt.with.len(), 1);
            assert_eq!(evt.with[0].label, Some("x".to_string()));
            assert!(evt.witnesses.is_empty());
        } else {
            panic!("Expected Machine component");
        }
    }

    #[test]
    fn test_xml_unknown_convergence_value_errors() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.machine name="test"/>
    <org.eventb.core.event name="bad_evt" convergence="99">
    </org.eventb.core.event>
</org.eventb.core.machineFile>"#;

        let result = parse_xml(xml);
        assert!(
            result.is_err(),
            "Expected error for unknown convergence value"
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("99"),
            "Error should mention the bad value '99': {}",
            err_msg
        );
        assert!(
            err_msg.contains("bad_evt"),
            "Error should mention the event name 'bad_evt': {}",
            err_msg
        );
    }

    #[test]
    fn test_xml_missing_convergence_defaults_to_ordinary() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.machine name="test"/>
    <org.eventb.core.event name="evt_no_conv">
    </org.eventb.core.event>
</org.eventb.core.machineFile>"#;

        let result = parse_xml(xml);
        assert!(result.is_ok(), "Parse error: {:?}", result.err());

        if let Component::Machine(m) = result.unwrap() {
            assert_eq!(m.events.len(), 1);
            assert_eq!(m.events[0].name, "evt_no_conv");
            assert_eq!(m.events[0].status, None);
        } else {
            panic!("Expected Machine component");
        }
    }

    #[test]
    fn test_xml_read_with_binding_element() {
        // Test reading a non-standard org.eventb.core.withBinding element
        // The expression "y + 1" is converted to predicate "x = y + 1"
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="5">
    <org.eventb.core.machine name="test"/>
    <org.eventb.core.event name="evt1" convergence="0">
        <org.eventb.core.refinesEvent target="abstract_evt"/>
        <org.eventb.core.withBinding identifier="x" expression="y + 1"/>
    </org.eventb.core.event>
</org.eventb.core.machineFile>"#;

        let result = parse_xml(xml).expect("Failed to parse XML with withBinding");

        if let Component::Machine(m) = result {
            assert_eq!(m.events.len(), 1);
            let evt = &m.events[0];
            assert_eq!(evt.name, "evt1");
            assert_eq!(evt.with.len(), 1);
            assert_eq!(evt.with[0].label, Some("x".to_string()));
            // Verify the predicate is "x = y + 1"
            match &evt.with[0].predicate {
                crate::ast::Predicate::Comparison { op, left, right } => {
                    assert_eq!(*op, crate::ast::predicate::ComparisonOp::Equal);
                    assert_eq!(*left, crate::ast::Expression::Identifier("x".to_string()));
                    match right {
                        crate::ast::Expression::Binary { op, left, right } => {
                            assert_eq!(*op, crate::ast::expression::BinaryOp::Add);
                            assert_eq!(**left, crate::ast::Expression::Identifier("y".to_string()));
                            assert_eq!(**right, crate::ast::Expression::Integer(1));
                        }
                        other => panic!("Expected Binary expression, got {:?}", other),
                    }
                }
                other => panic!("Expected Comparison predicate, got {:?}", other),
            }
        } else {
            panic!("Expected Machine component");
        }
    }

    // ========================================================================
    // Zip Writing Tests
    // ========================================================================

    #[test]
    fn test_component_filename() {
        let ctx = Context::new("counter_ctx".to_string());
        assert_eq!(
            component_filename(&Component::Context(ctx)),
            "counter_ctx.buc"
        );

        let m = Machine::new("counter".to_string());
        assert_eq!(component_filename(&Component::Machine(m)), "counter.bum");
    }

    #[test]
    fn test_to_zip_single_context() {
        let ctx = Context::new("test_ctx".to_string());
        let named = NamedComponent {
            filename: "test_ctx.buc".to_string(),
            component: Component::Context(ctx),
        };

        let zip_data = to_zip(&[named]).unwrap();
        let parsed = parse_zip(&zip_data).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].filename, "test_ctx.buc");
        if let Component::Context(ref c) = parsed[0].component {
            assert_eq!(c.name, "test_ctx");
        } else {
            panic!("Expected Context");
        }
    }

    #[test]
    fn test_to_zip_multiple_components() {
        let ctx = Context::new("my_ctx".to_string());
        let machine = Machine::new("my_machine".to_string());

        let components = vec![
            NamedComponent {
                filename: "my_ctx.buc".to_string(),
                component: Component::Context(ctx),
            },
            NamedComponent {
                filename: "my_machine.bum".to_string(),
                component: Component::Machine(machine),
            },
        ];

        let zip_data = to_zip(&components).unwrap();
        let parsed = parse_zip(&zip_data).unwrap();
        assert_eq!(parsed.len(), 2);

        assert_eq!(parsed[0].filename, "my_ctx.buc");
        assert!(matches!(parsed[0].component, Component::Context(_)));

        assert_eq!(parsed[1].filename, "my_machine.bum");
        assert!(matches!(parsed[1].component, Component::Machine(_)));
    }

    #[test]
    fn test_to_zip_roundtrip_from_text() {
        let text = r#"
        CONTEXT roundtrip_ctx
        SETS
            STATUS
        CONSTANTS
            max_val
        AXIOMS
            @axm1 max_val = 42
        END
        "#;

        let component = crate::parser::parse(text).expect("Failed to parse text");
        let filename = component_filename(&component);
        assert_eq!(filename, "roundtrip_ctx.buc");

        let named = NamedComponent {
            filename,
            component,
        };

        let zip_data = to_zip(&[named]).unwrap();
        let parsed = parse_zip(&zip_data).unwrap();
        assert_eq!(parsed.len(), 1);
        if let Component::Context(ref c) = parsed[0].component {
            assert_eq!(c.name, "roundtrip_ctx");
            assert_eq!(c.sets.len(), 1);
            assert_eq!(c.constants.len(), 1);
            assert_eq!(c.axioms.len(), 1);
        } else {
            panic!("Expected Context");
        }
    }

    #[test]
    fn test_to_zip_empty() {
        let zip_data = to_zip(&[]).unwrap();
        let parsed = parse_zip(&zip_data).unwrap();
        assert!(parsed.is_empty());
    }
}
