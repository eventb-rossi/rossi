//! Parse an already-emitted `.bcc` / `.bcm` file into a normalized
//! semantic view for equivalence comparison.
//!
//! This is the oracle used by the corpus harness: two `.bcc`/`.bcm`
//! files are *semantically equivalent* iff their [`ScView`] values are
//! equal after normalisation. Normalisation:
//!
//! - strips the `name=` attribute (Rodin's auto-counter ≠ ours),
//! - re-parses each predicate / expression / assignment attribute into
//!   its AST so whitespace and bound-var ascriptions don't matter,
//! - sorts unordered collections (carrier sets, constants, variables,
//!   invariants, axioms) by their identifying key.
//!
//! Note: we only cover the `.bcc` fields currently emitted by the
//! checker (no machines yet). Missing fields should be added as those
//! features land.

use std::collections::BTreeMap;

use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event as XmlEvent};
use rossi::ast::expression::BinaryOp;
use rossi::{
    Action, ActionKind, Expression, ExpressionKind, Predicate, PredicateKind, parse_action_str,
    parse_predicate_str,
};

use crate::error::{ProjectError, Result};
use crate::xml_out::tag;

/// A normalized view of an `.bcc`/`.bcm` file suitable for semantic
/// comparison via `PartialEq`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScView {
    pub kind: RootKind,
    pub accurate: bool,
    pub carrier_sets: BTreeMap<String, CarrierSetRow>,
    pub constants: BTreeMap<String, ConstantRow>,
    /// Axioms keyed by `source` URI (label can collide across refinement).
    pub axioms: BTreeMap<String, AxiomRow>,
    /// Invariants keyed by `source` URI (label can collide across refinement).
    pub invariants: BTreeMap<String, InvariantRow>,
    pub variables: BTreeMap<String, VariableRow>,
    pub variant: Option<String>,
    pub events: BTreeMap<String, EventRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RootKind {
    #[default]
    Unknown,
    Context,
    Machine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarrierSetRow {
    pub type_str: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstantRow {
    pub type_str: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxiomRow {
    pub label: String,
    pub theorem: bool,
    pub predicate: Predicate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvariantRow {
    pub label: String,
    pub theorem: bool,
    pub predicate: Predicate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariableRow {
    pub type_str: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRow {
    pub accurate: bool,
    pub convergence: Option<String>,
    pub extended: bool,
    pub parameters: BTreeMap<String, String>, // name -> type_str
    /// Guards keyed by `source` URI, since refined events can inherit
    /// guards from ancestors and labels are only unique per-source.
    pub guards: BTreeMap<String, InvariantRow>,
    /// Actions keyed by `source` URI (same reason as guards).
    pub actions: BTreeMap<String, ActionRow>,
    /// Witnesses keyed by `source` URI. Value holds the (stripped)
    /// predicate so we diff its AST, not the raw string.
    pub witnesses: BTreeMap<String, WitnessRow>,
    /// `scRefinesEvent` entries keyed by `source` URI. Value is the
    /// project-stripped `scTarget` so we compare the abstract-event
    /// identity, not the project-name prefix.
    pub refines_events: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitnessRow {
    pub label: String,
    pub predicate: Predicate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionRow {
    pub label: String,
    /// The action, parsed into AST and stripped of Rodin's inserted
    /// type ascriptions. This makes `ActionRow: PartialEq` robust to
    /// whitespace differences in the assignment text (e.g.
    /// `register ∪ {u}` vs `register∪{u}`) and to Rodin's post-SC
    /// insertion of `⦂ T` on empty-set RHS.
    pub action: Action,
}

impl ScView {
    /// Parse a `.bcc` / `.bcm` XML string and normalize it.
    pub fn from_xml(xml: &str) -> Result<Self> {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);
        let mut buf = Vec::new();
        let mut view = ScView::default();
        let mut inside_event: Option<String> = None;

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(XmlEvent::Start(e)) => ingest_element(&mut view, &mut inside_event, &e)?,
                Ok(XmlEvent::Empty(e)) => ingest_element(&mut view, &mut inside_event, &e)?,
                Ok(XmlEvent::End(e)) => {
                    let name_bytes = e.name();
                    let end_tag = std::str::from_utf8(name_bytes.as_ref()).unwrap_or("");
                    if end_tag == tag::SC_EVENT {
                        inside_event = None;
                    }
                }
                Ok(XmlEvent::Eof) => break,
                Err(e) => return Err(ProjectError::Xml(e).into()),
                _ => {}
            }
            buf.clear();
        }
        Ok(view)
    }
}

fn ingest_element(
    view: &mut ScView,
    inside_event: &mut Option<String>,
    e: &BytesStart,
) -> Result<()> {
    let name_bytes = e.name();
    let elem_tag = std::str::from_utf8(name_bytes.as_ref())
        .map_err(|e| ProjectError::XmlTag(e.to_string()))?;
    match elem_tag {
        tag::SC_CONTEXT_FILE => handle_root_file(view, RootKind::Context, e)?,
        tag::SC_MACHINE_FILE => handle_root_file(view, RootKind::Machine, e)?,
        tag::SC_CARRIER_SET => handle_carrier_set(view, e)?,
        tag::SC_CONSTANT => handle_constant(view, e)?,
        tag::SC_AXIOM => handle_axiom(view, e)?,
        tag::SC_INVARIANT => handle_invariant(view, e)?,
        tag::SC_VARIABLE => handle_variable(view, e)?,
        tag::SC_VARIANT => view.variant = string_attr(e, b"expression")?,
        tag::SC_EVENT => handle_event(view, inside_event, e)?,
        tag::SC_PARAMETER => handle_parameter(view, inside_event, e)?,
        tag::SC_GUARD => handle_guard(view, inside_event, e)?,
        tag::SC_ACTION => handle_action(view, inside_event, e)?,
        tag::SC_WITNESS => handle_witness(view, inside_event, e)?,
        tag::SC_REFINES_EVENT => handle_refines_event(view, inside_event, e)?,
        _ => {}
    }
    Ok(())
}

fn handle_root_file(view: &mut ScView, kind: RootKind, e: &BytesStart) -> Result<()> {
    view.kind = kind;
    view.accurate = bool_attr(e, b"accurate")?.unwrap_or(false);
    Ok(())
}

fn handle_carrier_set(view: &mut ScView, e: &BytesStart) -> Result<()> {
    let Some(name) = plain_attr(e, b"name")? else {
        return Ok(());
    };
    let type_str = string_attr(e, b"type")?.unwrap_or_default();
    view.carrier_sets.insert(name, CarrierSetRow { type_str });
    Ok(())
}

fn handle_constant(view: &mut ScView, e: &BytesStart) -> Result<()> {
    let Some(name) = plain_attr(e, b"name")? else {
        return Ok(());
    };
    let type_str = string_attr(e, b"type")?.unwrap_or_default();
    view.constants.insert(name, ConstantRow { type_str });
    Ok(())
}

fn handle_axiom(view: &mut ScView, e: &BytesStart) -> Result<()> {
    let Some(label) = string_attr(e, b"label")? else {
        return Ok(());
    };
    let source = normalize_source(string_attr(e, b"source")?).unwrap_or_else(|| label.clone());
    let theorem = bool_attr(e, b"theorem")?.unwrap_or(false);
    let predicate = predicate_attr(e)?;
    view.axioms.insert(
        source,
        AxiomRow {
            label,
            theorem,
            predicate,
        },
    );
    Ok(())
}

fn handle_invariant(view: &mut ScView, e: &BytesStart) -> Result<()> {
    let Some(label) = string_attr(e, b"label")? else {
        return Ok(());
    };
    let source = normalize_source(string_attr(e, b"source")?).unwrap_or_else(|| label.clone());
    let theorem = bool_attr(e, b"theorem")?.unwrap_or(false);
    let predicate = predicate_attr(e)?;
    view.invariants.insert(
        source,
        InvariantRow {
            label,
            theorem,
            predicate,
        },
    );
    Ok(())
}

fn handle_variable(view: &mut ScView, e: &BytesStart) -> Result<()> {
    let Some(name) = plain_attr(e, b"name")? else {
        return Ok(());
    };
    let type_str = string_attr(e, b"type")?.unwrap_or_default();
    view.variables.insert(name, VariableRow { type_str });
    Ok(())
}

fn handle_event(
    view: &mut ScView,
    inside_event: &mut Option<String>,
    e: &BytesStart,
) -> Result<()> {
    let Some(label) = string_attr(e, b"label")? else {
        return Ok(());
    };
    let row = EventRow {
        accurate: bool_attr(e, b"accurate")?.unwrap_or(false),
        convergence: string_attr(e, b"convergence")?,
        extended: bool_attr(e, b"extended")?.unwrap_or(false),
        parameters: BTreeMap::new(),
        guards: BTreeMap::new(),
        actions: BTreeMap::new(),
        witnesses: BTreeMap::new(),
        refines_events: BTreeMap::new(),
    };
    view.events.insert(label.clone(), row);
    *inside_event = Some(label);
    Ok(())
}

fn handle_parameter(
    view: &mut ScView,
    inside_event: &Option<String>,
    e: &BytesStart,
) -> Result<()> {
    let Some(evt) = inside_event.as_ref() else {
        return Ok(());
    };
    let Some(name) = plain_attr(e, b"name")? else {
        return Ok(());
    };
    let type_str = string_attr(e, b"type")?.unwrap_or_default();
    if let Some(row) = view.events.get_mut(evt) {
        row.parameters.insert(name, type_str);
    }
    Ok(())
}

fn handle_guard(view: &mut ScView, inside_event: &Option<String>, e: &BytesStart) -> Result<()> {
    let Some(evt) = inside_event.as_ref() else {
        return Ok(());
    };
    let Some(label) = string_attr(e, b"label")? else {
        return Ok(());
    };
    let source = normalize_source(string_attr(e, b"source")?).unwrap_or_else(|| label.clone());
    let theorem = bool_attr(e, b"theorem")?.unwrap_or(false);
    let predicate = predicate_attr(e)?;
    if let Some(row) = view.events.get_mut(evt) {
        row.guards.insert(
            source,
            InvariantRow {
                label,
                theorem,
                predicate,
            },
        );
    }
    Ok(())
}

fn handle_action(view: &mut ScView, inside_event: &Option<String>, e: &BytesStart) -> Result<()> {
    let Some(evt) = inside_event.as_ref() else {
        return Ok(());
    };
    let Some(label) = string_attr(e, b"label")? else {
        return Ok(());
    };
    let source = normalize_source(string_attr(e, b"source")?).unwrap_or_else(|| label.clone());
    let action = action_attr(e)?;
    if let Some(row) = view.events.get_mut(evt) {
        row.actions.insert(source, ActionRow { label, action });
    }
    Ok(())
}

fn handle_witness(view: &mut ScView, inside_event: &Option<String>, e: &BytesStart) -> Result<()> {
    let Some(evt) = inside_event.as_ref() else {
        return Ok(());
    };
    let Some(label) = string_attr(e, b"label")? else {
        return Ok(());
    };
    let source = normalize_source(string_attr(e, b"source")?).unwrap_or_else(|| label.clone());
    let predicate = predicate_attr(e)?;
    if let Some(row) = view.events.get_mut(evt) {
        row.witnesses
            .insert(source, WitnessRow { label, predicate });
    }
    Ok(())
}

fn handle_refines_event(
    view: &mut ScView,
    inside_event: &Option<String>,
    e: &BytesStart,
) -> Result<()> {
    let Some(evt) = inside_event.as_ref() else {
        return Ok(());
    };
    let source = string_attr(e, b"source")?
        .and_then(|s| normalize_source(Some(s)))
        .unwrap_or_default();
    // scTarget shape: `/PROJECT/File.bcm|scMachineFile#M|scEvent#X`.
    // Rodin's `scEvent#X` uses its auto-counter name; ours uses
    // the event's label. The URIs are semantically equivalent
    // (both point at the abstract event) but textually differ,
    // so we truncate to the scMachineFile segment — "which
    // abstract machine is refined" is the semantic content.
    // Label-level comparison needs cross-file resolution; out
    // of scope for the semantic oracle.
    let sc_event_sep = format!("|{}#", tag::SC_EVENT);
    let sc_target = string_attr(e, b"scTarget")?
        .and_then(|s| normalize_source(Some(s)))
        .map(|s| s.split(&sc_event_sep).next().unwrap_or("").to_string())
        .unwrap_or_default();
    if let Some(row) = view.events.get_mut(evt) {
        row.refines_events.insert(source, sc_target);
    }
    Ok(())
}

fn plain_attr(e: &BytesStart, key: &[u8]) -> Result<Option<String>> {
    for a in e.attributes() {
        let a = a.map_err(|e| ProjectError::XmlAttribute(e.to_string()))?;
        if a.key.as_ref() == key {
            let v = a
                .normalized_value(XmlVersion::Implicit1_0)
                .map_err(|e| ProjectError::XmlAttribute(e.to_string()))?;
            return Ok(Some(v.into_owned()));
        }
    }
    Ok(None)
}

fn string_attr(e: &BytesStart, key: &[u8]) -> Result<Option<String>> {
    crate::xml_out::read_attr(e, key, |s| ProjectError::XmlAttribute(s).into())
}

fn bool_attr(e: &BytesStart, key: &[u8]) -> Result<Option<bool>> {
    Ok(string_attr(e, key)?.map(|s| s == "true"))
}

fn predicate_attr(e: &BytesStart) -> Result<Predicate> {
    let s = string_attr(e, b"predicate")?.unwrap_or_default();
    let s = s.trim();
    let ast = parse_predicate_str(s).map_err(|err| ProjectError::ReparseFormula {
        kind: "predicate",
        input: s.to_string(),
        err,
    })?;
    Ok(strip_type_ascriptions_pred(ast))
}

/// Parse the `org.eventb.core.assignment` attribute into an [`Action`],
/// stripping type ascriptions so Rodin-canonical and bare forms compare
/// equal.
fn action_attr(e: &BytesStart) -> Result<Action> {
    let s = string_attr(e, b"assignment")?.unwrap_or_default();
    let s = s.trim();
    let ast = parse_action_str(s).map_err(|err| ProjectError::ReparseFormula {
        kind: "action",
        input: s.to_string(),
        err,
    })?;
    Ok(strip_type_ascriptions_action(ast))
}

/// Strip the leading `/PROJECT/` segment from a source URI so that
/// views built from our output and views built from Rodin's can be
/// compared regardless of project-name differences. (Rodin stores the
/// project name in the workspace hierarchy; our `Project::name` may
/// use a different spelling derived from a zip filename.)
fn normalize_source(s: Option<String>) -> Option<String> {
    let s = s?;
    // Input shape: `/Project/File.ext|...` — drop everything up to and
    // including the second `/`.
    if let Some(rest) = s.strip_prefix('/')
        && let Some(i) = rest.find('/')
    {
        return Some(rest[i + 1..].to_string());
    }
    Some(s)
}

/// Strip type ascriptions from every expression inside an [`Action`].
/// Used so `register ≔ ∅ ⦂ ℙ(USERS)` compares equal to `register ≔ ∅`.
#[must_use]
pub fn strip_type_ascriptions_action(a: Action) -> Action {
    match a.kind {
        ActionKind::Skip => ActionKind::Skip.into(),
        ActionKind::Assignment {
            variables,
            expressions,
        } => ActionKind::Assignment {
            variables,
            expressions: expressions.into_iter().map(strip_expr).collect(),
        }
        .into(),
        ActionKind::BecomesIn { variables, set } => ActionKind::BecomesIn {
            variables,
            set: strip_expr(set),
        }
        .into(),
        ActionKind::BecomesSuchThat {
            variables,
            predicate,
        } => ActionKind::BecomesSuchThat {
            variables,
            predicate: strip_type_ascriptions_pred(predicate),
        }
        .into(),
    }
}

/// Rodin emits type-inference artifacts into predicate strings:
/// `∅ ⦂ ℙ(T)` and `∀x⦂T · P`. For semantic comparison we drop these,
/// since they carry no logical content.
#[must_use]
pub fn strip_type_ascriptions_pred(p: Predicate) -> Predicate {
    use PredicateKind as P;
    match p.kind {
        kind @ (P::True | P::False) => kind.into(),
        P::Comparison { op, left, right } => P::Comparison {
            op,
            left: strip_expr(left),
            right: strip_expr(right),
        }
        .into(),
        P::Not(inner) => P::Not(Box::new(strip_type_ascriptions_pred(*inner))).into(),
        P::Logical { op, left, right } => P::Logical {
            op,
            left: Box::new(strip_type_ascriptions_pred(*left)),
            right: Box::new(strip_type_ascriptions_pred(*right)),
        }
        .into(),
        P::Quantified {
            quantifier,
            identifiers,
            predicate,
        } => P::Quantified {
            quantifier,
            identifiers: identifiers
                .into_iter()
                .map(|mut ti| {
                    ti.type_expr = None;
                    ti
                })
                .collect(),
            predicate: Box::new(strip_type_ascriptions_pred(*predicate)),
        }
        .into(),
        P::Application {
            function,
            arguments,
        } => P::Application {
            function,
            arguments: arguments.into_iter().map(strip_expr).collect(),
        }
        .into(),
        P::BuiltinApplication {
            predicate,
            arguments,
        } => P::BuiltinApplication {
            predicate,
            arguments: arguments.into_iter().map(strip_expr).collect(),
        }
        .into(),
    }
}

/// True when `expr` is a left-associative maplet of the binder identifiers
/// in `ids`, in declared order. Arity-1 collapses to a single
/// `Identifier(ids[0])`; arity-n collapses to `((ids[0] ↦ ids[1]) ↦ … ↦
/// ids[n-1])`. Used to recognise Rodin's `{x⦂T·P|x}` round-trip of the
/// basic-form `{x|P}`.
fn projection_matches_binders(expr: &Expression, ids: &[rossi::ast::TypedIdentifier]) -> bool {
    fn walk(expr: &Expression, names: &[String]) -> bool {
        match (&expr.kind, names) {
            (ExpressionKind::Identifier(n), [single]) => n == single,
            (
                ExpressionKind::Binary {
                    op: rossi::ast::expression::BinaryOp::Maplet,
                    left,
                    right,
                },
                rest,
            ) if rest.len() >= 2 => {
                let (last, init) = rest.split_last().expect("len ≥ 2");
                matches!(&right.as_ref().kind, ExpressionKind::Identifier(n) if n == last)
                    && walk(left, init)
            }
            _ => false,
        }
    }
    if ids.is_empty() {
        return false;
    }
    let names: Vec<String> = ids.iter().map(|ti| ti.name.clone()).collect();
    walk(expr, &names)
}

fn strip_expr(e: Expression) -> Expression {
    use ExpressionKind as E;
    match e.kind {
        E::Binary {
            op: BinaryOp::OfType,
            left,
            right: _,
        } => strip_expr(*left),
        E::Binary { op, left, right } => E::Binary {
            op,
            left: Box::new(strip_expr(*left)),
            right: Box::new(strip_expr(*right)),
        }
        .into(),
        E::Unary { op, operand } => E::Unary {
            op,
            operand: Box::new(strip_expr(*operand)),
        }
        .into(),
        E::FunctionApplication { function, argument } => {
            // Rodin's static checker emits `prj1(s)` etc. as the generic-atomic
            // form `(prj1 ⦂ T)(s)`: a type-ascribed atom applied as a function.
            // After `OfType` stripping the atom is a bare `AtomicBuiltin(Prj1)`
            // and the shape is `FunctionApplication` — exactly what our parser
            // produces for the same surface text, so no special collapse is
            // needed.
            E::FunctionApplication {
                function: Box::new(strip_expr(*function)),
                argument: Box::new(strip_expr(*argument)),
            }
            .into()
        }
        E::BuiltinApplication { function, argument } => E::BuiltinApplication {
            function,
            argument: Box::new(strip_expr(*argument)),
        }
        .into(),
        E::SetEnumeration(items) => {
            E::SetEnumeration(items.into_iter().map(strip_expr).collect()).into()
        }
        E::SetComprehension {
            identifiers,
            predicate,
            expression,
        } => {
            let stripped_ids: Vec<_> = identifiers
                .into_iter()
                .map(|mut ti| {
                    ti.type_expr = None;
                    ti
                })
                .collect();
            // Rodin emits the basic form `{x | P}` as the extended form
            // `{x⦂T · P | x}`. When the projection equals the binders
            // (in left-associative maplet order), collapse it back so
            // the two forms compare equal.
            let collapsed = expression.and_then(|e| {
                let stripped = strip_expr(*e);
                if projection_matches_binders(&stripped, &stripped_ids) {
                    None
                } else {
                    Some(Box::new(stripped))
                }
            });
            E::SetComprehension {
                identifiers: stripped_ids,
                predicate: Box::new(strip_type_ascriptions_pred(*predicate)),
                expression: collapsed,
            }
            .into()
        }
        E::SetBuilder {
            member_expression,
            predicate,
        } => E::SetBuilder {
            member_expression: Box::new(strip_expr(*member_expression)),
            predicate: Box::new(strip_type_ascriptions_pred(*predicate)),
        }
        .into(),
        E::Lambda {
            pattern,
            predicate,
            expression,
        } => E::Lambda {
            pattern,
            predicate: Box::new(strip_type_ascriptions_pred(*predicate)),
            expression: Box::new(strip_expr(*expression)),
        }
        .into(),
        E::QuantifiedUnion {
            identifiers,
            predicate,
            expression,
        } => E::QuantifiedUnion {
            identifiers: identifiers
                .into_iter()
                .map(|mut ti| {
                    ti.type_expr = None;
                    ti
                })
                .collect(),
            predicate: Box::new(strip_type_ascriptions_pred(*predicate)),
            expression: Box::new(strip_expr(*expression)),
        }
        .into(),
        E::QuantifiedInter {
            identifiers,
            predicate,
            expression,
        } => E::QuantifiedInter {
            identifiers: identifiers
                .into_iter()
                .map(|mut ti| {
                    ti.type_expr = None;
                    ti
                })
                .collect(),
            predicate: Box::new(strip_type_ascriptions_pred(*predicate)),
            expression: Box::new(strip_expr(*expression)),
        }
        .into(),
        E::RelationalImage { relation, set } => E::RelationalImage {
            relation: Box::new(strip_expr(*relation)),
            set: Box::new(strip_expr(*set)),
        }
        .into(),
        E::Bool(p) => E::Bool(Box::new(strip_type_ascriptions_pred(*p))).into(),
        other => other.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_carrier_sets() {
        let xml = r#"<?xml version="1.0"?>
<org.eventb.core.scContextFile org.eventb.core.accurate="true">
<org.eventb.core.scCarrierSet name="USERS" org.eventb.core.type="ℙ(USERS)"/>
<org.eventb.core.scCarrierSet name="ITEMS" org.eventb.core.type="ℙ(ITEMS)"/>
</org.eventb.core.scContextFile>"#;
        let v = ScView::from_xml(xml).unwrap();
        assert_eq!(v.kind, RootKind::Context);
        assert!(v.accurate);
        assert_eq!(v.carrier_sets.len(), 2);
        assert_eq!(v.carrier_sets["USERS"].type_str, "ℙ(USERS)");
        assert_eq!(v.carrier_sets["ITEMS"].type_str, "ℙ(ITEMS)");
    }

    #[test]
    fn axiom_predicate_is_parsed_into_ast() {
        let xml = r#"<?xml version="1.0"?>
<org.eventb.core.scContextFile org.eventb.core.accurate="true">
<org.eventb.core.scAxiom name="'" org.eventb.core.label="axm1" org.eventb.core.predicate="n∈ℕ" org.eventb.core.theorem="false"/>
</org.eventb.core.scContextFile>"#;
        let v = ScView::from_xml(xml).unwrap();
        let ax = &v.axioms["axm1"];
        assert!(!ax.theorem);
        let expected = parse_predicate_str("n ∈ ℕ").unwrap();
        assert_eq!(ax.predicate, expected);
    }

    #[test]
    fn whitespace_differences_in_predicate_dont_matter() {
        // Two equivalent forms (tight vs spaced, and with / without bound
        // type ascriptions) should compare equal after normalisation.
        let a = r#"<?xml version="1.0"?>
<org.eventb.core.scContextFile>
<org.eventb.core.scAxiom name="'" org.eventb.core.label="a" org.eventb.core.predicate="∀x,y·x≤y⇒x≤y"/>
</org.eventb.core.scContextFile>"#;
        let b = r#"<?xml version="1.0"?>
<org.eventb.core.scContextFile>
<org.eventb.core.scAxiom name="Z" org.eventb.core.label="a" org.eventb.core.predicate="∀ x , y · x ≤ y ⇒ x ≤ y"/>
</org.eventb.core.scContextFile>"#;
        let va = ScView::from_xml(a).unwrap();
        let vb = ScView::from_xml(b).unwrap();
        assert_eq!(va, vb, "views should be equal despite whitespace");
    }

    /// Wrap one scAction's assignment in a minimal scMachineFile so we
    /// can exercise the scAction ingest path.
    fn make_bcm_xml(assignment: &str) -> String {
        format!(
            r#"<?xml version="1.0"?>
<org.eventb.core.scMachineFile org.eventb.core.accurate="true">
<org.eventb.core.scEvent name="E" org.eventb.core.accurate="true" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="E">
<org.eventb.core.scAction name="a1" org.eventb.core.assignment="{assignment}" org.eventb.core.label="act1"/>
</org.eventb.core.scEvent>
</org.eventb.core.scMachineFile>"#
        )
    }

    #[test]
    fn action_whitespace_variants_compare_equal() {
        // `register ≔ register∪{u}` vs `register ≔ register ∪ {u}` —
        // same AST, different string. ScView must PartialEq them equal.
        let tight = make_bcm_xml("register ≔ register∪{u}");
        let spaced = make_bcm_xml("register ≔ register ∪ {u}");
        let va = ScView::from_xml(&tight).unwrap();
        let vb = ScView::from_xml(&spaced).unwrap();
        assert_eq!(va, vb);
    }

    #[test]
    fn action_type_ascription_stripped() {
        // Rodin adds `⦂ ℙ(USERS)` on empty-set RHS after type-checking.
        // We strip it at view-time so the bare and annotated forms
        // compare equal.
        let with_asc = make_bcm_xml("register ≔ ∅ ⦂ ℙ(USERS)");
        let bare = make_bcm_xml("register ≔ ∅");
        let va = ScView::from_xml(&with_asc).unwrap();
        let vb = ScView::from_xml(&bare).unwrap();
        assert_eq!(va, vb);
    }

    #[test]
    fn becomes_in_action_whitespace_insensitive() {
        // `:∈` forms also go through ScView's action parse.
        let a = make_bcm_xml("k :∈ dom(f)");
        let b = make_bcm_xml("k  :∈  dom(f)");
        let va = ScView::from_xml(&a).unwrap();
        let vb = ScView::from_xml(&b).unwrap();
        assert_eq!(va, vb);
    }

    #[test]
    fn setcomp_extended_form_collapses_to_basic_arity1() {
        // Rodin emits `{x | P}` (basic) as `{x⦂T · P | x}` (extended
        // with a no-op projection equal to the binder). After
        // strip_expr these must compare equal.
        let basic = make_bcm_xml("active ≔ {x ∣ x ∈ active}");
        let rodin = make_bcm_xml("active ≔ {x⦂DIRECTIONS · x ∈ active ∣ x}");
        let va = ScView::from_xml(&basic).unwrap();
        let vb = ScView::from_xml(&rodin).unwrap();
        assert_eq!(va, vb);
    }

    #[test]
    fn setcomp_extended_form_collapses_to_basic_arity2() {
        // Multi-binder: projection is the left-associative maplet.
        let basic = make_bcm_xml("r ≔ {x, y ∣ x ∈ A ∧ y ∈ B}");
        let rodin = make_bcm_xml("r ≔ {x⦂S, y⦂T · x ∈ A ∧ y ∈ B ∣ x ↦ y}");
        let va = ScView::from_xml(&basic).unwrap();
        let vb = ScView::from_xml(&rodin).unwrap();
        assert_eq!(va, vb);
    }

    #[test]
    fn setcomp_non_collapsing_projection_preserved() {
        // Projection that ISN'T the binder must stay in extended form.
        let with_op = make_bcm_xml("r ≔ {x⦂ℤ · x ∈ ℕ ∣ x + 1}");
        let v_op = ScView::from_xml(&with_op).unwrap();
        let basic = make_bcm_xml("r ≔ {x ∣ x ∈ ℕ}");
        let v_basic = ScView::from_xml(&basic).unwrap();
        assert_ne!(
            v_op, v_basic,
            "extended form with non-trivial projection must NOT collapse to basic"
        );
    }
}

#[cfg(test)]
mod prj_function_application_tests {
    //! Regression tests for the `(f ◁ g)(x)` round-trip — see
    //! a real-world corpus model, where Rodin emits
    //! `left ≔ (mapping ◁ (prj1 ⦂ ℙ(ℤ×BOOL×ℤ)))(x)` and we must
    //! produce a stripped AST that matches our pretty-printed
    //! `left ≔ (mapping ◁ prj1)(x)`. The pretty-printer used to
    //! drop the parens around the Binary-typed function side,
    //! which collapsed `prj1(x)` into a different AST.
    use super::{strip_type_ascriptions_action, strip_type_ascriptions_pred};
    use rossi::{parse_action_str, parse_predicate_str};

    #[test]
    fn prj1_ascription_strips_to_bare() {
        let bare = parse_action_str("left ≔ (mapping ◁ prj1)(x)").unwrap();
        let asc = parse_action_str("left ≔ (mapping ◁ (prj1 ⦂ ℙ(ℤ×BOOL×ℤ)))(x)").unwrap();
        let a = strip_type_ascriptions_action(bare);
        let b = strip_type_ascriptions_action(asc);
        assert_eq!(a, b, "bare={a:#?}\nasc={b:#?}");
    }

    #[test]
    fn function_side_paren_drop_changes_ast() {
        // Asserts the precondition that motivates the pretty-printer
        // fix in `rossi::pretty`: `(mapping ◁ prj1)(x)` and
        // `mapping ◁ prj1(x)` parse to *different* ASTs, so the
        // printer must keep the outer parens whenever it emits a
        // FunctionApplication whose function side is a Binary.
        let with_parens = parse_action_str("left ≔ (mapping ◁ prj1)(x)").unwrap();
        let dropped = parse_action_str("left ≔ mapping ◁ prj1(x)").unwrap();
        assert_ne!(with_parens, dropped);
    }

    #[test]
    fn axiom_prj2_v1_v2_round_trip() {
        // From a corpus access-control context's axiom — the source predicate
        // uses V1-style `prj2(s)` (1-arg builtin call); Rodin's static
        // checker emits the V2 form `(prj2 ⦂ ℙ(α×β×β))(s)` in `.bcc`.
        // After stripping `⦂` ascriptions, both must collapse to the
        // same AST so the corpus harness reports semantic PASS.
        let ours = parse_predicate_str("∀s · s ∈ SUBJECTS ⇒ prj2(s) = domain_of_subj(s)").unwrap();
        let rodin = parse_predicate_str(
            "∀s⦂PROCESSES×DOMAINS · s∈SUBJECTS ⇒ \
             (prj2 ⦂ ℙ(PROCESSES×DOMAINS×DOMAINS))(s) = domain_of_subj(s)",
        )
        .unwrap();
        let a = strip_type_ascriptions_pred(ours);
        let b = strip_type_ascriptions_pred(rodin);
        assert_eq!(a, b, "ours={a:#?}\nrodin={b:#?}");
    }
}
