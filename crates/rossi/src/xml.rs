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
//!     <org.eventb.core.carrierSet name="S" org.eventb.core.identifier="S"/>
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
use quick_xml::escape::{escape, unescape};
use quick_xml::events::Event as XmlEvent;
use quick_xml::{Reader, XmlVersion};
use std::borrow::Cow;
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
fn escape_xml(s: &str) -> Cow<'_, str> {
    escape(s)
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

/// Validate a *structural* name read from Event-B XML (machine/context
/// names, refines/sees/extends targets, event names) and return the form we
/// store: surrounding whitespace is trimmed away. Rodin tolerates stray
/// whitespace around names (a real-world corpus model carries an event
/// label with a trailing space), so we clean rather than reject — every
/// identifier-position read goes through here, keeping cross-references
/// (refines targets, etc.) consistent with the trimmed declarations.
///
/// Beyond trimming, the charset check delegates to
/// [`crate::names::check_component_name`] — the single source of truth shared
/// with the text grammar's `component_name` rule. Rodin treats these names as
/// file names and labels (bare strings), so real models carry hyphens
/// (`A-C0`, `CTX-1`); the text grammar lexes them right after
/// `MACHINE`/`EVENT`/`REFINES`/…, so import accepts exactly what re-parsing
/// can honestly handle (issue #28) — no more (a trailing or doubled `-`
/// would pretty-print into unparseable text and is rejected here).
///
/// *Structural* keyword-named identifiers (`end`, `events`, `extends`, ...)
/// are accepted: Rodin permits them and our expression-position grammar
/// parses them as identifiers (the `kw_*` rules fire only in their
/// specific structural positions, not as a general reservation). The
/// mathematical reserved words (`dom`, `card`, ...) are a different story —
/// see [`validate_declared_identifier`].
fn validate_component_name(name: &str, origin: &str) -> Result<String> {
    validate_name_with(name, origin, crate::names::check_component_name)
}

/// Validate a *mathematical* identifier position (witness labels and
/// withBinding identifiers, which name abstract parameters or primed
/// variables like `x'`): trimmed, then checked against
/// [`crate::names::check_math_identifier`] — kernel_lang §2.2 names, never
/// hyphenated.
fn validate_math_identifier(name: &str, origin: &str) -> Result<String> {
    validate_name_with(name, origin, crate::names::check_math_identifier)
}

/// Trim `name`, run `check` (the [`crate::names`] predicate for the position),
/// and wrap any failure in a [`ParseError::UnsupportedIdentifier`] carrying
/// the original (untrimmed) name and the predicate's reason string.
fn validate_name_with(
    name: &str,
    origin: &str,
    check: fn(&str) -> std::result::Result<(), crate::names::NameError>,
) -> Result<String> {
    let trimmed = name.trim();
    match check(trimmed) {
        Ok(()) => Ok(trimmed.to_string()),
        Err(e) => Err(ParseError::UnsupportedIdentifier {
            name: name.to_string(),
            origin: origin.to_string(),
            reason: e.to_string(),
        }),
    }
}

/// [`validate_math_identifier`] plus the kernel_lang §2.2 reserved-word
/// check, for *mathematical declarations* (carrier sets, constants,
/// variables, event parameters). Rodin's own `isValidIdentifierName` rejects
/// these names, so no Rodin-exported XML contains them; a hand-crafted file
/// that does would otherwise import into an AST the text grammar can no
/// longer express (pretty-print → re-parse fails on the declaration).
/// Structural names (component/event names, refines/sees targets) go through
/// the hyphen-capable [`validate_component_name`] instead.
fn validate_declared_identifier(name: &str, origin: &str) -> Result<String> {
    let validated = validate_math_identifier(name, origin)?;
    if crate::builtins::is_reserved_word(&validated) {
        return Err(ParseError::UnsupportedIdentifier {
            name: name.to_string(),
            origin: origin.to_string(),
            reason: "reserved word of the Event-B mathematical language".to_string(),
        });
    }
    Ok(validated)
}

/// Read an event's name from its XML element. In real Rodin XML, `name` is
/// an internal id (e.g. `'`) and `org.eventb.core.label` holds the
/// human-readable name; fall back to `name` for hand-crafted files that
/// lack `label`. Trimmed here (not just in `validate_component_name`) so the
/// INITIALISATION check at the call sites sees the cleaned name.
fn event_name_attr(e: &quick_xml::events::BytesStart) -> Result<String> {
    let raw = get_xml_attr(e, b"label")?
        .or(get_xml_attr(e, b"name")?)
        .unwrap_or_default();
    Ok(raw.trim().to_string())
}

/// Trim a label read from Event-B XML, treating a whitespace-only value as
/// absent. Labels are not identifiers (Rodin allows arbitrary text), so they
/// are not validated — but surrounding whitespace would not survive a text
/// round-trip, so it is cleaned the same way identifiers are.
fn non_empty_trimmed(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
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
    // A nesting rejection, an operator-incompatibility rejection, or a
    // misplaced-assignment rejection (EB026) is a property of the formula, not
    // of the XML envelope — keep the variant intact so consumers classify it by
    // its own rule and surface its precise message instead of a
    // malformed-attribute wrapper. Other failures need the attribute context;
    // their parser coordinates are local to `value`, not the XML document.
    if matches!(
        err,
        ParseError::NestingTooDeep { .. }
            | ParseError::IncompatibleOperators { .. }
            | ParseError::AssignmentInPredicate { .. }
    ) {
        return err;
    }
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
            // Trimmed like identifiers: Rodin tolerates stray whitespace
            // around labels, but the text format's label rule cannot carry
            // it, so a padded label would not survive a text round-trip.
            "label" => label = non_empty_trimmed(&value),
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

/// Returns the element's label as an XML-escaped `name` attribute value, or `_{idx}` when
/// no label is present. The `_{idx}` fallback uses an underscore prefix, which is not a
/// valid start character for Event-B labels or identifiers, so it cannot collide with any
/// user-defined name.
fn label_or_index(label: Option<&str>, idx: usize) -> String {
    label
        .map(|label| escape_xml(label).into_owned())
        .unwrap_or_else(|| format!("_{idx}"))
}

/// Write labeled predicates as XML elements.
///
/// Theorems (including any parsed from a `THEOREMS` section, which lower into the
/// axioms/invariants vec with `is_theorem = true`) are written as ordinary
/// `org.eventb.core.axiom`/`org.eventb.core.invariant` elements carrying
/// `org.eventb.core.theorem="true"` — Rodin has no separate theorems container.
fn write_labeled_predicates_xml(
    xml: &mut String,
    items: &[LabeledPredicate],
    element_name: &str,
    printer: &PrettyPrinter,
    indent: &str,
) {
    for (i, item) in items.iter().enumerate() {
        let predicate_str = printer.print_predicate(&item.predicate);
        let name = label_or_index(item.label.as_deref(), i);
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
            "{}<{} name=\"{}\"{} org.eventb.core.predicate=\"{}\" org.eventb.core.theorem=\"{}\"{}/>\n",
            indent,
            element_name,
            name,
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
                    context_comment = get_xml_attr(&e, b"org.eventb.core.comment")?;
                }
            }
            Ok(XmlEvent::Empty(e)) => {
                let name_bytes = e.name();
                let tag_name = std::str::from_utf8(name_bytes.as_ref())
                    .map_err(|e| ParseError::InvalidXml(e.to_string()))?
                    .to_string();

                match tag_name.as_str() {
                    "org.eventb.core.extendsContext" => {
                        let target = required_attr(&e, b"target")?;
                        let target = validate_component_name(
                            &target,
                            &format!("extends target in {}", origin),
                        )?;
                        extends.push(target);
                    }
                    "org.eventb.core.carrierSet" => {
                        if let Some(set_name) = get_xml_attr(&e, b"identifier")? {
                            let set_name = validate_declared_identifier(
                                &set_name,
                                &format!("carrier set in {}", origin),
                            )?;
                            let comment = get_xml_attr(&e, b"comment")?;
                            sets.push(SetDeclaration::Deferred {
                                name: set_name,
                                comment,
                                span: None,
                            });
                        }
                    }
                    "org.eventb.core.constant" => {
                        if let Some(const_name) = get_xml_attr(&e, b"identifier")? {
                            let const_name = validate_declared_identifier(
                                &const_name,
                                &format!("constant in {}", origin),
                            )?;
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
        context_name =
            validate_component_name(&context_name, &format!("context name in {}", origin))?;
    }

    Ok(Context {
        name: context_name,
        extends,
        sets,
        constants,
        axioms,
        span: None,
        name_span: None,
        clauses: Vec::new(),
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
                    machine_comment = get_xml_attr(&e, b"org.eventb.core.comment")?;
                } else if tag_name == "org.eventb.core.event" {
                    let event_name = event_name_attr(&e)?;
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
                    "org.eventb.core.refinesMachine" => {
                        let target = required_attr(&e, b"target")?;
                        let target = validate_component_name(
                            &target,
                            &format!("refines target in {}", origin),
                        )?;
                        refines = Some(target);
                    }
                    "org.eventb.core.seesContext" => {
                        let target = required_attr(&e, b"target")?;
                        let target = validate_component_name(
                            &target,
                            &format!("sees target in {}", origin),
                        )?;
                        sees.push(target);
                    }
                    "org.eventb.core.variable" => {
                        if let Some(var_name) = get_xml_attr(&e, b"identifier")? {
                            let var_name = validate_declared_identifier(
                                &var_name,
                                &format!("variable in {}", origin),
                            )?;
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
                        let event_name = event_name_attr(&e)?;
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
                                span: None,
                                name_span: None,
                            });
                        } else {
                            let event_name = validate_component_name(
                                &event_name,
                                &format!("event name in {}", origin),
                            )?;
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
                                refines_span: None,
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
                            let target = validate_component_name(
                                &target,
                                &format!("refines target in event {:?} of {}", event.name, origin),
                            )?;
                            event.refines = Some(target);
                        }
                    }
                    "org.eventb.core.parameter" => {
                        if let Some(ref mut event) = current_event
                            && let Some(param) = get_xml_attr(&e, b"identifier")?
                        {
                            let param = validate_declared_identifier(
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
                            // A witness label is an identifier position: it names
                            // the witnessed abstract parameter or variable (e.g.
                            // `x'`), so it must stay consistent with the trimmed
                            // declarations.
                            let label = match get_xml_attr(&e, b"label")? {
                                Some(l) => Some(validate_math_identifier(
                                    &l,
                                    &format!(
                                        "witness label in event {:?} of {}",
                                        event.name, origin
                                    ),
                                )?),
                                None => None,
                            };
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
                                let identifier = validate_math_identifier(
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
                            let label =
                                get_xml_attr(&e, b"label")?.and_then(|l| non_empty_trimmed(&l));
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
                                span: None,
                                name_span: None,
                            });
                        } else {
                            let event_name = validate_component_name(
                                &event_builder.name,
                                &format!("event name in {}", origin),
                            )?;
                            events.push(Event {
                                name: event_name,
                                status,
                                refines: event_builder.refines,
                                parameters: event_builder.parameters,
                                guards: event_builder.guards,
                                with: event_builder.with,
                                witnesses: event_builder.witnesses,
                                actions: event_builder.actions,
                                span: None,
                                name_span: None,
                                refines_span: None,
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
        machine_name =
            validate_component_name(&machine_name, &format!("machine name in {}", origin))?;
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
        clauses: Vec::new(),
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

/// A named Rodin project — a project name plus the components that belong to it.
///
/// This is the unit written under its own `<name>/` directory by
/// [`to_multi_project_zip`] / [`write_multi_project_directory`], so a model
/// holding several top-level Rodin projects (e.g. a decomposition) round-trips
/// without sibling components colliding. A single-project export uses the flat
/// [`to_project_zip`] / [`write_project_directory`] writers instead.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NamedProject {
    /// The Rodin project name. Used both as the `<name>/` archive/directory
    /// prefix and as the `.project` descriptor's `<name>`.
    pub name: String,
    /// The components (contexts and machines) belonging to this project.
    pub components: Vec<NamedComponent>,
}

#[derive(Debug, Clone, Copy)]
enum ZipComponentKind {
    Context,
    Machine,
}

impl ZipComponentKind {
    fn from_filename(filename: &str) -> Option<Self> {
        if filename.ends_with(".buc") {
            Some(Self::Context)
        } else if filename.ends_with(".bum") {
            Some(Self::Machine)
        } else {
            None
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Self::Context => ".buc",
            Self::Machine => ".bum",
        }
    }
}

#[derive(Debug)]
struct ZipComponentEntry {
    index: usize,
    kind: ZipComponentKind,
}

/// Identify component entries from the central directory before opening any
/// of them. Keeping the classification here gives strict and recovery parsing
/// the same basename, extension, and default-name rules.
fn zip_component_entries<R>(archive: &zip::ZipArchive<R>) -> Vec<ZipComponentEntry>
where
    R: IoRead + std::io::Seek,
{
    archive
        .file_names()
        .enumerate()
        .filter_map(|(index, path)| {
            let filename = path.split('/').next_back().unwrap_or("");
            let kind = ZipComponentKind::from_filename(filename)?;
            Some(ZipComponentEntry { index, kind })
        })
        .collect()
}

/// Open, decode, and parse one classified Event-B component entry.
fn parse_zip_entry<R>(
    archive: &mut zip::ZipArchive<R>,
    entry: ZipComponentEntry,
) -> Result<NamedComponent>
where
    R: IoRead + std::io::Seek,
{
    let ZipComponentEntry { index, kind } = entry;
    let filename = archive
        .name_for_index(index)
        .unwrap_or("")
        .split('/')
        .next_back()
        .unwrap_or("")
        .to_string();
    let default_name = filename.strip_suffix(kind.suffix()).unwrap_or(&filename);
    let result = (|| {
        let mut file = archive.by_index(index).map_err(|e| {
            ParseError::InvalidXml(format!("Failed to read zip entry {index}: {e}"))
        })?;
        let mut xml_content = String::new();
        file.read_to_string(&mut xml_content)
            .map_err(|e| ParseError::IoError(e.to_string()))?;

        let component = match kind {
            ZipComponentKind::Context => Component::Context(parse_context_xml_with_name(
                &xml_content,
                Some(default_name),
                Some(&filename),
            )?),
            ZipComponentKind::Machine => Component::Machine(parse_machine_xml_with_name(
                &xml_content,
                Some(default_name),
                Some(&filename),
            )?),
        };
        Ok(component)
    })();

    let component = result.map_err(|source| wrap_file_error(&filename, source))?;
    Ok(NamedComponent {
        filename,
        component,
    })
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

    zip_component_entries(&archive)
        .into_iter()
        .map(|entry| parse_zip_entry(&mut archive, entry))
        .collect()
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

    let (components, errors) = zip_component_entries(&archive)
        .into_iter()
        .map(|entry| parse_zip_entry(&mut archive, entry))
        .fold(
            (Vec::new(), Vec::new()),
            |(mut components, mut errors), result| {
                match result {
                    Ok(component) => components.push(component),
                    Err(error) => errors.push(error),
                }
                (components, errors)
            },
        );

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
/// ctx.sets.push(SetDeclaration::Deferred { name: "STATUS".to_string(), comment: None, span: None });
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
    write_projects_zip([("", None, components)])
}

/// Creates a Rodin project zip archive in memory from named components.
///
/// The archive contains a root `.project` descriptor plus each component
/// serialized to its native Rodin XML format.
pub fn to_project_zip(components: &[NamedComponent], project_name: &str) -> Result<Vec<u8>> {
    write_projects_zip([("", Some(project_name), components)])
}

/// Creates a multi-project Rodin zip archive in memory.
///
/// Each [`NamedProject`] is written under its own `<name>/` directory prefix,
/// carrying its own `<name>/.project` descriptor plus its components' Rodin XML.
/// This is what an Eclipse "Archive File" export of several top-level projects
/// looks like, so a decomposition exported here re-imports as one project per
/// directory and sibling components sharing a basename never overwrite. A
/// single-project export should use the flat [`to_project_zip`].
///
/// # Example
/// ```
/// use rossi::{Component, Context, Machine, NamedComponent, NamedProject, to_multi_project_zip, parse_zip};
///
/// let a = NamedProject {
///     name: "A".to_string(),
///     components: vec![NamedComponent {
///         filename: "C.buc".to_string(),
///         component: Component::Context(Context::new("C".to_string())),
///     }],
/// };
/// let b = NamedProject {
///     name: "B".to_string(),
///     components: vec![NamedComponent {
///         filename: "M.bum".to_string(),
///         component: Component::Machine(Machine::new("M".to_string())),
///     }],
/// };
/// let zip_data = to_multi_project_zip(&[a, b]).unwrap();
/// // Both projects' components survive, namespaced by their directory prefix.
/// assert_eq!(parse_zip(&zip_data).unwrap().len(), 2);
/// ```
pub fn to_multi_project_zip(projects: &[NamedProject]) -> Result<Vec<u8>> {
    // Each project's entries live under `<name>/`; pre-materialize the prefixes
    // so the shared writer can borrow them.
    let prefixes: Vec<String> = projects.iter().map(|p| format!("{}/", p.name)).collect();
    write_projects_zip(projects.iter().zip(&prefixes).map(|(p, prefix)| {
        (
            prefix.as_str(),
            Some(p.name.as_str()),
            p.components.as_slice(),
        )
    }))
}

/// Serializes one or more projects into an in-memory zip archive.
///
/// Each project is `(prefix, descriptor_name, components)`: its entries are
/// written at `{prefix}{filename}` (so `prefix` is `""` for a flat archive or
/// `"Name/"` for a sub-project), preceded by a `{prefix}.project` descriptor
/// when `descriptor_name` is `Some`. Taking the projects as an iterator of
/// borrows lets the flat wrappers pass a single entry without allocating.
fn write_projects_zip<'a>(
    projects: impl IntoIterator<Item = (&'a str, Option<&'a str>, &'a [NamedComponent])>,
) -> Result<Vec<u8>> {
    use std::io::Write;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let mut buf = Vec::new();
    {
        let mut writer = ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

        for (prefix, descriptor_name, components) in projects {
            if let Some(name) = descriptor_name {
                writer
                    .start_file(format!("{prefix}.project"), options)
                    .map_err(|e| {
                        ParseError::IoError(format!("Failed to write zip entry: {}", e))
                    })?;
                writer.write_all(rodin_project_file_xml(name).as_bytes())?;
            }

            for named in components {
                let xml = to_xml(&named.component);
                writer
                    .start_file(format!("{prefix}{}", named.filename), options)
                    .map_err(|e| {
                        ParseError::IoError(format!("Failed to write zip entry: {}", e))
                    })?;
                writer.write_all(xml.as_bytes())?;
            }
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

/// Creates a Rodin project zip archive from named components and writes it to a file.
pub fn write_project_zip_file<P: AsRef<std::path::Path>>(
    path: P,
    components: &[NamedComponent],
    project_name: &str,
) -> Result<()> {
    let data = to_project_zip(components, project_name)?;
    std::fs::write(path, data)?;
    Ok(())
}

/// Creates a Rodin project directory from named components.
pub fn write_project_directory<P: AsRef<std::path::Path>>(
    path: P,
    components: &[NamedComponent],
    project_name: &str,
) -> Result<()> {
    write_projects_directory(path.as_ref(), [("", Some(project_name), components)])
}

/// Creates a multi-project Rodin directory tree from named projects.
///
/// The on-disk analogue of [`to_multi_project_zip`]: each [`NamedProject`] is
/// written under its own `<root>/<name>/` subdirectory with its own `.project`
/// descriptor and component XML, so sibling projects sharing a component
/// basename never collide. A single-project export uses [`write_project_directory`].
pub fn write_multi_project_directory<P: AsRef<std::path::Path>>(
    path: P,
    projects: &[NamedProject],
) -> Result<()> {
    write_projects_directory(
        path.as_ref(),
        projects.iter().map(|p| {
            (
                p.name.as_str(),
                Some(p.name.as_str()),
                p.components.as_slice(),
            )
        }),
    )
}

/// Writes one or more projects into a directory tree rooted at `root`.
///
/// Each project is `(subdir, descriptor_name, components)`: an empty `subdir`
/// writes the project flat into `root` (the single-project case), otherwise it
/// goes under `root/subdir`. A `.project` descriptor is written when
/// `descriptor_name` is `Some`. Shared by [`write_project_directory`] and
/// [`write_multi_project_directory`].
fn write_projects_directory<'a>(
    root: &std::path::Path,
    projects: impl IntoIterator<Item = (&'a str, Option<&'a str>, &'a [NamedComponent])>,
) -> Result<()> {
    for (subdir, descriptor_name, components) in projects {
        let base = if subdir.is_empty() {
            root.to_path_buf()
        } else {
            root.join(subdir)
        };
        std::fs::create_dir_all(&base)?;
        if let Some(name) = descriptor_name {
            std::fs::write(base.join(".project"), rodin_project_file_xml(name))?;
        }

        for named in components {
            let file_path = base.join(&named.filename);
            if let Some(parent) = file_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(file_path, to_xml(&named.component))?;
        }
    }

    Ok(())
}

/// Extract the project name from a Rodin/Eclipse `.project` descriptor.
///
/// The first element whose local name is `name` holds the Eclipse `IProject`
/// name — the same string Rodin uses as the leading `/ProjectName/` segment of
/// every handle URI. The later `<name>` inside `<buildCommand>`
/// (`org.rodinp.core.rodinbuilder`) is never the first match, so taking the
/// first occurrence is correct. Returns `None` if the XML is invalid or no
/// non-empty `name` element is present. The inverse of
/// `rodin_project_file_xml`.
#[must_use]
pub fn read_project_name(xml: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);

    loop {
        match reader.read_event().ok()? {
            XmlEvent::Start(e) if e.name().local_name().as_ref() == b"name" => {
                let mut depth = 1usize;
                let mut name = String::new();
                loop {
                    match reader.read_event().ok()? {
                        XmlEvent::Start(_) => depth += 1,
                        XmlEvent::End(_) => {
                            depth -= 1;
                            if depth == 0 {
                                let name = name.trim();
                                return (!name.is_empty()).then(|| name.to_string());
                            }
                        }
                        XmlEvent::Text(text) => {
                            let decoded = text.decode().ok()?;
                            name.push_str(&unescape(&decoded).ok()?);
                        }
                        XmlEvent::GeneralRef(reference) => {
                            let reference = reference.decode().ok()?;
                            let encoded = format!("&{reference};");
                            name.push_str(&unescape(&encoded).ok()?);
                        }
                        XmlEvent::CData(cdata) => name.push_str(&cdata.decode().ok()?),
                        XmlEvent::Eof => return None,
                        _ => {}
                    }
                }
            }
            XmlEvent::Empty(e) if e.name().local_name().as_ref() == b"name" => return None,
            XmlEvent::Eof => return None,
            _ => {}
        }
    }
}

fn rodin_project_file_xml(project_name: &str) -> String {
    let project_name = if project_name.trim().is_empty() {
        "rossi_project"
    } else {
        project_name.trim()
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<projectDescription>
  <name>{}</name>
  <comment></comment>
  <projects></projects>
  <buildSpec>
    <buildCommand>
      <name>org.rodinp.core.rodinbuilder</name>
      <arguments></arguments>
    </buildCommand>
  </buildSpec>
  <natures>
    <nature>org.rodinp.core.rodinnature</nature>
  </natures>
</projectDescription>
"#,
        escape_xml(project_name)
    )
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
    let comment_attr = format_comment_attr(ctx.comment.as_deref());
    xml.push_str(&format!(
        "<org.eventb.core.contextFile version=\"{}\" org.eventb.core.configuration=\"{}\"{}>\n",
        escape_xml(version),
        escape_xml(configuration),
        comment_attr,
    ));

    // Extends (deduplicated: duplicate entries would produce sibling name collisions)
    {
        let mut emitted = std::collections::HashSet::new();
        for extended in &ctx.extends {
            if emitted.insert(extended.as_str()) {
                let esc = escape_xml(extended);
                xml.push_str(&format!(
                    "    <org.eventb.core.extendsContext name=\"{esc}\" org.eventb.core.target=\"{esc}\"/>\n"
                ));
            }
        }
    }

    // Sets
    for set in &ctx.sets {
        let set_comment = format_comment_attr(set.comment());
        let esc = escape_xml(set.name());
        xml.push_str(&format!(
            "    <org.eventb.core.carrierSet name=\"{esc}\" org.eventb.core.identifier=\"{esc}\"{set_comment}/>\n"
        ));
    }

    // Constants
    for constant in &ctx.constants {
        let const_comment = format_comment_attr(constant.comment.as_deref());
        let esc = escape_xml(&constant.name);
        xml.push_str(&format!(
            "    <org.eventb.core.constant name=\"{esc}\" org.eventb.core.identifier=\"{esc}\"{const_comment}/>\n"
        ));
    }

    // Axioms and theorems (theorems have is_theorem = true)
    let printer = rodin_xml_printer();
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
    let comment_attr = format_comment_attr(machine.comment.as_deref());
    xml.push_str(&format!(
        "<org.eventb.core.machineFile version=\"{}\" org.eventb.core.configuration=\"{}\"{}>\n",
        escape_xml(version),
        escape_xml(configuration),
        comment_attr,
    ));

    // Refines
    if let Some(ref refined) = machine.refines {
        let esc = escape_xml(refined);
        xml.push_str(&format!(
            "    <org.eventb.core.refinesMachine name=\"{esc}\" org.eventb.core.target=\"{esc}\"/>\n"
        ));
    }

    // Sees (deduplicated: duplicate entries would produce sibling name collisions)
    {
        let mut emitted = std::collections::HashSet::new();
        for seen in &machine.sees {
            if emitted.insert(seen.as_str()) {
                let esc = escape_xml(seen);
                xml.push_str(&format!(
                    "    <org.eventb.core.seesContext name=\"{esc}\" org.eventb.core.target=\"{esc}\"/>\n"
                ));
            }
        }
    }

    // Variables
    for variable in &machine.variables {
        let var_comment = format_comment_attr(variable.comment.as_deref());
        let esc = escape_xml(&variable.name);
        xml.push_str(&format!(
            "    <org.eventb.core.variable name=\"{esc}\" org.eventb.core.identifier=\"{esc}\"{var_comment}/>\n"
        ));
    }

    // Invariants and theorems (theorems have is_theorem = true)
    let printer = rodin_xml_printer();
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
            "    <org.eventb.core.variant name=\"_vr\" org.eventb.core.expression=\"{}\"/>\n",
            escape_xml(&expr_str)
        ));
    }

    // INITIALISATION event
    if let Some(init) = &machine.initialisation {
        let init_comment = format_comment_attr(init.comment.as_deref());
        let extended_str = if init.extended { "true" } else { "false" };
        xml.push_str(&format!(
            "    <org.eventb.core.event name=\"INITIALISATION\" org.eventb.core.convergence=\"0\" org.eventb.core.extended=\"{}\" org.eventb.core.label=\"INITIALISATION\"{}>\n",
            extended_str, init_comment
        ));
        let mut idx = 0usize;
        for lp in &init.with {
            write_witness_xml(&mut xml, lp, &printer, "        ", false, idx);
            idx += 1;
        }
        for lp in &init.witnesses {
            write_witness_xml(&mut xml, lp, &printer, "        ", true, idx);
            idx += 1;
        }
        for action in &init.actions {
            write_action_xml(&mut xml, action, &printer, "        ", idx);
            idx += 1;
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

/// Formula printer for native Rodin XML attributes.
///
/// Rodin's parser expects its private-use spellings for relation operators
/// and relational override.  Portable ASCII spellings remain appropriate for
/// textual Event-B, but make a generated `.buc` / `.bum` fail to rebuild.
fn rodin_xml_printer() -> PrettyPrinter {
    PrettyPrinter::new().with_private_use_glyphs(true)
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
        "    <org.eventb.core.event name=\"{}\" org.eventb.core.convergence=\"{}\" org.eventb.core.extended=\"{}\" org.eventb.core.label=\"{}\"{}>\n",
        escape_xml(&event.name),
        convergence,
        extended_str,
        escape_xml(&event.name),
        event_comment
    ));

    // Refines
    if let Some(ref refined) = event.refines {
        let esc = escape_xml(refined);
        xml.push_str(&format!(
            "        <org.eventb.core.refinesEvent name=\"{esc}\" org.eventb.core.target=\"{esc}\"/>\n"
        ));
    }

    // Parameters
    for param in &event.parameters {
        let param_comment = format_comment_attr(param.comment.as_deref());
        let esc = escape_xml(&param.name);
        xml.push_str(&format!(
            "        <org.eventb.core.parameter name=\"{esc}\" org.eventb.core.identifier=\"{esc}\"{param_comment}/>\n"
        ));
    }

    // Guards, witnesses, and actions share a single index so that unlabeled fallback names
    // (_0, _1, …) are unique across all siblings within the event.
    let mut idx = 0usize;
    for guard in &event.guards {
        let predicate_str = printer.print_predicate(&guard.predicate);
        let name = label_or_index(guard.label.as_deref(), idx);
        let label_attr = if let Some(label) = &guard.label {
            format!(" org.eventb.core.label=\"{}\"", escape_xml(label))
        } else {
            String::new()
        };
        let guard_comment = format_comment_attr(guard.comment.as_deref());
        xml.push_str(&format!(
            "        <org.eventb.core.guard name=\"{}\"{} org.eventb.core.predicate=\"{}\"{}/>\n",
            name,
            label_attr,
            escape_xml(&predicate_str),
            guard_comment
        ));
        idx += 1;
    }
    for lp in &event.with {
        write_witness_xml(xml, lp, printer, "        ", false, idx);
        idx += 1;
    }
    for lp in &event.witnesses {
        write_witness_xml(xml, lp, printer, "        ", true, idx);
        idx += 1;
    }
    for action in &event.actions {
        write_action_xml(xml, action, printer, "        ", idx);
        idx += 1;
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
    idx: usize,
) {
    let predicate_str = printer.print_predicate(&lp.predicate);
    let name = label_or_index(lp.label.as_deref(), idx);
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
        "{}<org.eventb.core.witness name=\"{}\"{} org.eventb.core.predicate=\"{}\"{}/>\n",
        indent,
        name,
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
    idx: usize,
) {
    let action_str = printer.print_action(&action.action);
    let name = label_or_index(action.label.as_deref(), idx);
    let label_attr = if let Some(label) = &action.label {
        format!(" org.eventb.core.label=\"{}\"", escape_xml(label))
    } else {
        String::new()
    };
    let comment_attr = format_comment_attr(action.comment.as_deref());
    xml.push_str(&format!(
        "{}<org.eventb.core.action name=\"{}\"{} org.eventb.core.assignment=\"{}\"{}/>\n",
        indent,
        name,
        label_attr,
        escape_xml(&action_str),
        comment_attr
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zip_with_entries(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, content) in entries {
                writer.start_file(*name, options).unwrap();
                std::io::Write::write_all(&mut writer, content).unwrap();
            }
            writer.finish().unwrap();
        }
        bytes
    }

    fn tempdir_unique(prefix: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn wrap_attr_error_preserves_nesting_rejections() {
        // NestingTooDeep must survive attribute wrapping so the validate CLI
        // classifies it as a formula error (EB005), not malformed XML (EB001).
        let nesting = ParseError::NestingTooDeep {
            limit: crate::nesting::MAX_NESTING_DEPTH,
            line: 1,
            column: 7,
        };
        assert!(matches!(
            wrap_attr_error(
                "ctx.buc",
                "axiom",
                Some("a1"),
                "predicate",
                "((…))",
                nesting
            ),
            ParseError::NestingTooDeep { .. }
        ));
        // Other parse errors still get the attribute envelope.
        assert!(matches!(
            wrap_attr_error(
                "ctx.buc",
                "axiom",
                Some("a1"),
                "predicate",
                "x =",
                ParseError::EmptyPredicate
            ),
            ParseError::MalformedAttribute { .. }
        ));
    }

    #[test]
    fn wrap_attr_error_preserves_assignment_in_predicate() {
        // A misplaced-assignment rejection (EB026) must survive attribute
        // wrapping so the validate CLI classifies it as EB026, not EB005.
        let eb026 = ParseError::AssignmentInPredicate {
            operator: ":=".to_string(),
            line: 1,
            column: 3,
            span: Some(crate::ast::Span { start: 2, end: 4 }),
        };
        assert!(matches!(
            wrap_attr_error(
                "m.bum",
                "invariant",
                Some("inv1"),
                "predicate",
                "x := 5",
                eb026
            ),
            ParseError::AssignmentInPredicate { .. }
        ));
    }

    #[test]
    fn over_deep_attribute_surfaces_nesting_error_from_xml() {
        let deep = format!(
            "{}x{} = 1",
            "(".repeat(crate::nesting::MAX_NESTING_DEPTH + 1),
            ")".repeat(crate::nesting::MAX_NESTING_DEPTH + 1)
        );
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
<org.eventb.core.axiom name="a" org.eventb.core.label="axm1" org.eventb.core.predicate="{}"/>
</org.eventb.core.contextFile>"#,
            escape_xml(&deep)
        );
        match parse_xml(&xml) {
            Err(ParseError::NestingTooDeep { .. }) => {}
            other => panic!("expected NestingTooDeep, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_simple_context_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
</org.eventb.core.contextFile>"#;

        let result = parse_xml(xml);
        assert!(result.is_ok(), "Parse error: {:?}", result.err());

        if let Component::Context(ctx) = result.unwrap() {
            assert_eq!(ctx.name, "unnamed_context");
        } else {
            panic!("Expected Context component");
        }
    }

    #[test]
    fn test_parse_context_with_sets_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
    <org.eventb.core.carrierSet identifier="STATUS"/>
    <org.eventb.core.carrierSet identifier="PERSON"/>
</org.eventb.core.contextFile>"#;

        let result = parse_xml(xml);
        assert!(result.is_ok(), "Parse error: {:?}", result.err());

        if let Component::Context(ctx) = result.unwrap() {
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
</org.eventb.core.machineFile>"#;

        let result = parse_xml(xml);
        assert!(result.is_ok(), "Parse error: {:?}", result.err());

        if let Component::Machine(m) = result.unwrap() {
            assert_eq!(m.name, "unnamed_machine");
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
            clauses: Vec::new(),
            comment: None,
            metadata: None,
        };

        let xml = to_xml(&Component::Context(ctx));
        assert!(xml.contains("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(xml.contains("<org.eventb.core.contextFile"));
        assert!(xml.contains("version=\"3\""));
        assert!(!xml.contains("<org.eventb.core.context "));
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
                    span: None,
                },
                SetDeclaration::Deferred {
                    name: "PERSON".to_string(),
                    comment: None,
                    span: None,
                },
            ],
            constants: vec![],
            axioms: vec![],

            span: None,
            name_span: None,
            clauses: Vec::new(),
            comment: None,
            metadata: None,
        };

        let xml = to_xml(&Component::Context(ctx));
        assert!(xml.contains("org.eventb.core.identifier=\"STATUS\""));
        assert!(xml.contains("org.eventb.core.identifier=\"PERSON\""));
    }

    #[test]
    fn test_context_with_axioms_to_xml() {
        let ctx = Context {
            name: "test_ctx".to_string(),
            extends: vec![],
            sets: vec![],
            constants: vec![NamedElement::new("max_value".to_string())],
            axioms: vec![LabeledPredicate {
                label: Some("axm1".to_string()),
                is_theorem: false,
                predicate: crate::ast::PredicateKind::Comparison {
                    op: crate::ast::predicate::ComparisonOp::Equal,
                    left: crate::ast::ExpressionKind::Identifier("max_value".to_string()).into(),
                    right: crate::ast::ExpressionKind::Integer(100).into(),
                }
                .into(),
                span: None,
                comment: None,
            }],

            span: None,
            name_span: None,
            clauses: Vec::new(),
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
            clauses: Vec::new(),
            comment: None,
            metadata: None,
        };

        let xml = to_xml(&Component::Machine(machine));
        assert!(xml.contains("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(xml.contains("<org.eventb.core.machineFile"));
        assert!(xml.contains("version=\"5\""));
        assert!(!xml.contains("<org.eventb.core.machine "));
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
            clauses: Vec::new(),
            comment: None,
            metadata: None,
        };

        let xml = to_xml(&Component::Machine(machine));
        assert!(xml.contains("org.eventb.core.target=\"counter_ctx\""));
        assert!(xml.contains("org.eventb.core.identifier=\"count\""));
    }

    #[test]
    fn test_machine_with_initialisation_to_xml() {
        use crate::ast::LabeledAction;

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
                    action: crate::ast::ActionKind::Assignment {
                        assignments: vec![(
                            "count".into(),
                            crate::ast::ExpressionKind::Integer(0).into(),
                        )],
                    }
                    .into(),
                    span: None,
                    comment: None,
                }],
                comment: None,
                extended: false,
                with: Vec::new(),
                witnesses: Vec::new(),
                span: None,
                name_span: None,
            }),
            events: vec![],
            span: None,
            name_span: None,
            clauses: Vec::new(),
            comment: None,
            metadata: None,
        };

        let xml = to_xml(&Component::Machine(machine));
        assert!(xml.contains("org.eventb.core.label=\"INITIALISATION\""));
        assert!(xml.contains("org.eventb.core.label=\"act1\""));
        assert!(xml.contains("org.eventb.core.assignment=\"count \u{2254} 0\""));
    }

    #[test]
    fn source_xml_uses_rodin_override_glyph() {
        use crate::ast::LabeledAction;

        let action = crate::parse_action_str("image ≔ image <+ {sector ↦ value}")
            .expect("override action parses");
        let mut machine = Machine::new("M".to_string());
        machine.initialisation = Some(InitialisationEvent {
            actions: vec![LabeledAction {
                label: Some("act1".to_string()),
                action: action.clone(),
                span: None,
                comment: None,
            }],
            comment: None,
            extended: false,
            with: Vec::new(),
            witnesses: Vec::new(),
            span: None,
            name_span: None,
        });

        let xml = to_xml(&Component::Machine(machine));
        assert!(xml.contains(crate::operators::RELATIONAL_OVERRIDE));
        assert!(!xml.contains("&lt;+"));

        let reparsed = parse_xml(&xml).expect("Rodin-native XML reparses");
        let Component::Machine(reparsed) = reparsed else {
            panic!("expected machine");
        };
        assert_eq!(reparsed.initialisation.unwrap().actions[0].action, action);
    }

    #[test]
    fn test_machine_with_event_to_xml() {
        use crate::ast::LabeledAction;

        let event = Event {
            name: "increment".to_string(),
            status: Some(EventStatus::Ordinary),
            refines: None,
            parameters: vec![],
            guards: vec![LabeledPredicate {
                label: Some("grd1".to_string()),
                is_theorem: false,
                predicate: crate::ast::PredicateKind::Comparison {
                    op: crate::ast::predicate::ComparisonOp::LessThan,
                    left: crate::ast::ExpressionKind::Identifier("count".to_string()).into(),
                    right: crate::ast::ExpressionKind::Identifier("max_value".to_string()).into(),
                }
                .into(),
                span: None,
                comment: None,
            }],
            with: vec![],
            witnesses: vec![],
            actions: vec![LabeledAction {
                label: Some("act1".to_string()),
                action: crate::ast::ActionKind::Assignment {
                    assignments: vec![(
                        "count".into(),
                        crate::ast::ExpressionKind::Binary {
                            op: crate::ast::expression::BinaryOp::Add,
                            left: Box::new(
                                crate::ast::ExpressionKind::Identifier("count".to_string()).into(),
                            ),
                            right: Box::new(crate::ast::ExpressionKind::Integer(1).into()),
                        }
                        .into(),
                    )],
                }
                .into(),
                span: None,
                comment: None,
            }],
            span: None,
            name_span: None,
            refines_span: None,
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
            clauses: Vec::new(),
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
    <org.eventb.core.carrierSet identifier="STATUS"/>
</org.eventb.core.contextFile>"#;

        // Parse XML to AST
        let component = parse_xml(original_xml).expect("Failed to parse original XML");

        // Convert AST back to XML
        let serialized_xml = to_xml(&component);

        // Parse the serialized XML again
        let reparsed = parse_xml(&serialized_xml).expect("Failed to parse serialized XML");

        // Name is not preserved through standalone parse_xml (no filename); check structure only.
        match (&component, &reparsed) {
            (Component::Context(ctx1), Component::Context(ctx2)) => {
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

        // Name is not preserved through standalone parse_xml (no filename); check structure only.
        match (&component, &reparsed) {
            (Component::Context(ctx1), Component::Context(ctx2)) => {
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

        // Name is not preserved through standalone parse_xml (no filename); check structure only.
        match (&component, &reparsed) {
            (Component::Machine(m1), Component::Machine(m2)) => {
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
        let ctx = Context {
            name: "test".to_string(),
            extends: vec![],
            sets: vec![],
            constants: vec![],
            axioms: vec![LabeledPredicate {
                label: Some("axm1".to_string()),
                is_theorem: false,
                predicate: crate::ast::PredicateKind::Comparison {
                    op: crate::ast::predicate::ComparisonOp::GreaterThan,
                    left: crate::ast::ExpressionKind::Identifier("x".to_string()).into(),
                    right: crate::ast::ExpressionKind::Integer(0).into(),
                }
                .into(),
                span: None,
                comment: None,
            }],

            span: None,
            name_span: None,
            clauses: Vec::new(),
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
        assert!(!xml.contains("<org.eventb.core.context "));
        assert!(xml.contains("org.eventb.core.identifier=\"STATUS\""));
        assert!(xml.contains("org.eventb.core.identifier=\"max_value\""));
        assert!(xml.contains("org.eventb.core.label=\"axm1\""));

        // Verify round-trip: text → XML → parse again should succeed
        let reparsed = parse_xml(&xml).expect("Failed to reparse XML");
        assert!(matches!(reparsed, Component::Context(_)));
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
        assert!(!xml.contains("<org.eventb.core.machine "));
        assert!(xml.contains("org.eventb.core.identifier=\"count\""));
        assert!(xml.contains("<org.eventb.core.invariant"));
        assert!(xml.contains("org.eventb.core.label=\"INITIALISATION\""));
        assert!(xml.contains("org.eventb.core.label=\"increment\""));

        // Verify round-trip: text → XML → parse again should succeed
        let reparsed = parse_xml(&xml).expect("Failed to reparse XML");
        if let Component::Machine(m2) = reparsed {
            assert_eq!(m2.variables, vec![NamedElement::new("count".to_string())]);
            assert_eq!(m2.events.len(), 1);
        } else {
            panic!("Expected Machine component");
        }
    }

    #[test]
    fn test_event_convergence_status() {
        use crate::ast::LabeledAction;

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
                action: crate::ast::ActionKind::Assignment {
                    assignments: vec![("x".into(), crate::ast::ExpressionKind::Integer(1).into())],
                }
                .into(),
                span: None,
                comment: None,
            }],
            span: None,
            name_span: None,
            refines_span: None,
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
            refines_span: None,
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
            refines_span: None,
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
            clauses: Vec::new(),
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
                predicate: crate::ast::PredicateKind::Comparison {
                    op: ComparisonOp::Equal,
                    left: crate::ast::ExpressionKind::Identifier("x".to_string()).into(),
                    right: crate::ast::ExpressionKind::Identifier("y".to_string()).into(),
                }
                .into(),
                span: None,
                comment: None,
            }],
            witnesses: vec![],
            actions: vec![],
            span: None,
            name_span: None,
            refines_span: None,
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
            clauses: Vec::new(),
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
                predicate: crate::ast::PredicateKind::Comparison {
                    op: ComparisonOp::Equal,
                    left: crate::ast::ExpressionKind::Identifier("x".to_string()).into(),
                    right: crate::ast::ExpressionKind::Identifier("y".to_string()).into(),
                }
                .into(),
                span: None,
                comment: None,
            }],
            witnesses: vec![],
            actions: vec![],
            span: None,
            name_span: None,
            refines_span: None,
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
            clauses: Vec::new(),
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
            match &evt.with[0].predicate.kind {
                crate::ast::PredicateKind::Comparison { op, left, right } => {
                    assert_eq!(*op, crate::ast::predicate::ComparisonOp::Equal);
                    assert_eq!(
                        *left,
                        crate::ast::ExpressionKind::Identifier("x".to_string()).into()
                    );
                    match &right.kind {
                        crate::ast::ExpressionKind::Binary { op, left, right } => {
                            assert_eq!(*op, crate::ast::expression::BinaryOp::Add);
                            assert_eq!(
                                **left,
                                crate::ast::ExpressionKind::Identifier("y".to_string()).into()
                            );
                            assert_eq!(**right, crate::ast::ExpressionKind::Integer(1).into());
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
    fn zip_modes_share_entry_filtering_and_default_names() {
        let context = br#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3"/>"#;
        let machine = br#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="3"/>"#;
        let bytes = zip_with_entries(&[
            ("project/notes.txt", &[0xff]),
            ("project/DefaultContext.buc", context),
            ("project/DefaultMachine.bum", machine),
        ]);

        let strict = parse_zip(&bytes).unwrap();
        let recovery = parse_zip_with_recovery(&bytes);
        assert!(recovery.errors.is_empty(), "{:?}", recovery.errors);
        let recovered = recovery.component.unwrap();
        assert_eq!(strict, recovered);
        assert_eq!(strict[0].filename, "DefaultContext.buc");
        assert_eq!(strict[0].component.name(), "DefaultContext");
        assert_eq!(strict[1].filename, "DefaultMachine.bum");
        assert_eq!(strict[1].component.name(), "DefaultMachine");
    }

    #[test]
    fn zip_modes_share_filename_wrapped_utf8_errors() {
        let machine = br#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile version="3"/>"#;
        let bytes =
            zip_with_entries(&[("project/Bad.buc", &[0xff]), ("project/Good.bum", machine)]);

        let strict_error = parse_zip(&bytes).unwrap_err();
        let ParseError::FileContext { filename, source } = &strict_error else {
            panic!("expected filename-wrapped strict error, got {strict_error:?}");
        };
        assert_eq!(filename, "Bad.buc");
        assert!(matches!(source.as_ref(), ParseError::IoError(_)));

        let recovery = parse_zip_with_recovery(&bytes);
        assert_eq!(recovery.errors.len(), 1);
        assert_eq!(
            recovery.errors[0].to_string(),
            strict_error.to_string(),
            "strict and recovery should expose the same entry error"
        );
        let components = recovery.component.unwrap();
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].filename, "Good.bum");
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

    #[test]
    fn test_read_project_name_round_trips_writer() {
        // The first <name> is the project name; the <name> inside buildCommand
        // (org.rodinp.core.rodinbuilder) must not be picked up.
        let xml = rodin_project_file_xml("MyProject");
        assert_eq!(read_project_name(&xml).as_deref(), Some("MyProject"));

        // Entities are unescaped (inverse of escape_xml).
        let xml = rodin_project_file_xml("Rossi & <Project>");
        assert_eq!(
            read_project_name(&xml).as_deref(),
            Some("Rossi & <Project>")
        );

        // No descriptor / empty name -> None.
        assert_eq!(read_project_name("<projectDescription/>"), None);
        assert_eq!(read_project_name("<name></name>"), None);
    }

    #[test]
    fn test_read_project_name_parses_xml_variations() {
        let cases = [
            (
                r#"<projectDescription><name kind="project">A&#32;B&#x20;C</name></projectDescription>"#,
                "A B C",
            ),
            (
                r#"<p:projectDescription xmlns:p="urn:eclipse"><p:name>Namespaced</p:name></p:projectDescription>"#,
                "Namespaced",
            ),
            (
                "<projectDescription><name><![CDATA[A & B]]></name></projectDescription>",
                "A & B",
            ),
        ];
        for (xml, expected) in cases {
            assert_eq!(read_project_name(xml).as_deref(), Some(expected));
        }
        assert_eq!(read_project_name("<projectDescription><name>broken"), None);
    }

    #[test]
    fn test_to_project_zip_includes_project_descriptor() {
        let ctx = Context::new("test_ctx".to_string());
        let named = NamedComponent {
            filename: "test_ctx.buc".to_string(),
            component: Component::Context(ctx),
        };

        let zip_data = to_project_zip(&[named], "Rossi & <Project>").unwrap();
        let parsed = parse_zip(&zip_data).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].filename, "test_ctx.buc");

        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_data)).unwrap();
        let mut project = archive.by_name(".project").unwrap();
        let mut project_xml = String::new();
        std::io::Read::read_to_string(&mut project, &mut project_xml).unwrap();
        assert!(project_xml.contains("<name>Rossi &amp; &lt;Project&gt;</name>"));
        assert!(project_xml.contains("<nature>org.rodinp.core.rodinnature</nature>"));
        assert!(project_xml.contains("<name>org.rodinp.core.rodinbuilder</name>"));
    }

    #[test]
    fn test_write_project_directory_includes_project_descriptor() {
        let dir = tempdir_unique("rossi-project-dir");
        let ctx = Context::new("test_ctx".to_string());
        let named = NamedComponent {
            filename: "test_ctx.buc".to_string(),
            component: Component::Context(ctx),
        };

        write_project_directory(&dir, &[named], "Dir Project").unwrap();
        let project_xml = std::fs::read_to_string(dir.join(".project")).unwrap();
        assert!(project_xml.contains("<name>Dir Project</name>"));
        assert!(dir.join("test_ctx.buc").exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_to_project_zip_falls_back_for_blank_project_name() {
        let zip_data = to_project_zip(&[], "   ").unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_data)).unwrap();
        let mut project = archive.by_name(".project").unwrap();
        let mut project_xml = String::new();
        std::io::Read::read_to_string(&mut project, &mut project_xml).unwrap();
        assert!(project_xml.contains("<name>rossi_project</name>"));
    }

    fn ctx_named(name: &str) -> NamedComponent {
        NamedComponent {
            filename: format!("{name}.buc"),
            component: Component::Context(Context::new(name.to_string())),
        }
    }

    fn entry_names(zip_data: &[u8]) -> Vec<String> {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_data)).unwrap();
        (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect()
    }

    #[test]
    fn test_to_multi_project_zip_namespaces_each_project() {
        // Two projects whose component basenames collide ("C.buc" in both) —
        // the case a flat archive would overwrite.
        let projects = [
            NamedProject {
                name: "A".to_string(),
                components: vec![ctx_named("C")],
            },
            NamedProject {
                name: "B".to_string(),
                components: vec![ctx_named("C")],
            },
        ];
        let zip_data = to_multi_project_zip(&projects).unwrap();
        let names = entry_names(&zip_data);
        assert_eq!(
            names,
            vec!["A/.project", "A/C.buc", "B/.project", "B/C.buc"],
            "each project is namespaced under its own directory prefix",
        );

        // Each `.project` descriptor carries its own project name.
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&zip_data)).unwrap();
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("A/.project").unwrap(), &mut xml)
            .unwrap();
        assert!(xml.contains("<name>A</name>"));
    }

    #[test]
    fn test_single_project_zip_keeps_flat_rodin_layout() {
        // Pin the Rodin-compatible layout the generalized writer must preserve:
        // a root `.project` descriptor first, then each component flat at its
        // basename (no directory prefix), in input order. A refactor of
        // `write_projects_zip` that prefixed or reordered entries would break
        // Rodin import — this guards against that, unlike comparing the writer
        // to itself.
        let components = [
            ctx_named("C0"),
            NamedComponent {
                filename: "M0.bum".to_string(),
                component: Component::Machine(Machine::new("M0".to_string())),
            },
        ];
        let zip = to_project_zip(&components, "rossi_project").unwrap();
        assert_eq!(entry_names(&zip), vec![".project", "C0.buc", "M0.bum"]);
    }

    #[test]
    fn test_write_multi_project_directory_namespaces_each_project() {
        let dir = tempdir_unique("rossi-multi-project-dir");
        let projects = [
            NamedProject {
                name: "A".to_string(),
                components: vec![ctx_named("C")],
            },
            NamedProject {
                name: "B".to_string(),
                components: vec![ctx_named("C")],
            },
        ];
        write_multi_project_directory(&dir, &projects).unwrap();

        for proj in ["A", "B"] {
            let proj_dir = dir.join(proj);
            assert!(proj_dir.join("C.buc").exists());
            let descriptor = std::fs::read_to_string(proj_dir.join(".project")).unwrap();
            assert!(descriptor.contains(&format!("<name>{proj}</name>")));
        }

        std::fs::remove_dir_all(&dir).ok();
    }
}
