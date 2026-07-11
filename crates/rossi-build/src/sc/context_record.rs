//! Pure-data model of a statically-checked context.
//!
//! [`ContextRecord`] is the typed result of running [`super::context`] on
//! a `.buc`. It carries everything dependents (child contexts via EXTENDS,
//! machines via SEES) need to plug the context into their own environment
//! — type bindings, carrier-set / constant / axiom tables, and the
//! transitive EXTENDS closure.
//!
//! The `.bcc` XML is a *rendering* of this record (see
//! [`render_context_parts`]).
//! Keeping the two layers distinct means machines can consume the record
//! without parsing our own XML.

use std::rc::Rc;

use rossi::Predicate;

use crate::handles::HandleUri;
use crate::type_env::TypeEnv;
use crate::types::Type;
use crate::xml_out::{Element, RodinNameGenerator, attr, tag};

/// The typed record produced by checking one `.buc`.
#[derive(Debug, Clone)]
pub struct ContextRecord {
    pub name: String,
    #[allow(dead_code)] // surfaced via Debug; machine SC may use it
    pub filename: String,
    pub output_filename: String,

    /// Everything this context contributes to a dependent's environment.
    pub env: TypeEnv,

    /// Carrier sets declared in this context, alphabetically sorted.
    pub carrier_sets: Vec<CarrierSetDecl>,
    /// Constants declared in this context, alphabetically sorted.
    pub constants: Vec<ConstantDecl>,
    /// Axioms declared in this context, in source order.
    pub axioms: Vec<AxiomDecl>,

    /// Direct EXTENDS parents (in source order).
    pub extends: Vec<ExtendsDecl>,
    /// Transitively-extended ancestor names, grandparent-first.
    pub ancestors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CarrierSetDecl {
    pub name: String,
    pub ty: Type,
    pub source: HandleUri,
}

#[derive(Debug, Clone)]
pub struct ConstantDecl {
    pub name: String,
    pub ty: Type,
    pub source: HandleUri,
}

#[derive(Debug, Clone)]
pub struct AxiomDecl {
    pub label: String,
    /// Position of this axiom in the *raw* context's `axioms` list. Lets
    /// later passes (well-definedness) pair a kept decl back to its source
    /// clause by identity rather than by label, which is ambiguous when
    /// two clauses share an (effective) label.
    pub source_index: usize,
    /// Enriched predicate AST (binder types stamped, short-form
    /// comprehensions lowered) — the form `predicate_canonical` was
    /// rendered from. Kept so dependent machines and downstream
    /// analyses can re-check it without re-parsing strings.
    #[allow(dead_code)] // used by machine SC (M1+)
    pub predicate: Predicate,
    pub predicate_canonical: String,
    pub is_theorem: bool,
    pub source: HandleUri,
}

#[derive(Debug, Clone)]
pub struct ExtendsDecl {
    pub parent_name: String,
    pub sc_target: String,
    pub source: HandleUri,
}

/// Render the body of a context's `.bcc` — scAxiom, scCarrierSet,
/// scConstant — in Rodin's emission order.
///
/// Rows are wrapped in `Rc` at the collecting boundary so descendant
/// contexts and machines that splice us into their own output get
/// O(1) per-element clones.
pub(crate) fn render_context_parts(record: &ContextRecord) -> (Vec<Rc<Element>>, Vec<Rc<Element>>) {
    let mut names = RodinNameGenerator::default();
    let extends = render_extends(record, &mut names);
    for ancestor in &record.ancestors {
        names.observe(ancestor);
    }
    let body = render_body(record, &mut names);
    (extends, body)
}

fn render_body(record: &ContextRecord, names: &mut RodinNameGenerator) -> Vec<Rc<Element>> {
    let mut out = Vec::with_capacity(
        record.axioms.len() + record.carrier_sets.len() + record.constants.len(),
    );
    for ax in &record.axioms {
        out.push(names.generated(|name| render_axiom(ax, name)));
    }
    for cs in &record.carrier_sets {
        out.push(names.retained(Rc::new(render_carrier_set(cs))));
    }
    for c in &record.constants {
        out.push(names.retained(Rc::new(render_constant(c))));
    }
    out
}

/// Render the `scExtendsContext` elements.
fn render_extends(record: &ContextRecord, names: &mut RodinNameGenerator) -> Vec<Rc<Element>> {
    record
        .extends
        .iter()
        .map(|e| names.generated(|name| render_extend(e, name)))
        .collect()
}

fn render_axiom(a: &AxiomDecl, internal_name: String) -> Element {
    Element::new(tag::SC_AXIOM)
        .attr(attr::NAME, internal_name)
        .attr(attr::LABEL, a.label.clone())
        .attr(attr::PREDICATE, a.predicate_canonical.clone())
        .attr(attr::SOURCE, a.source.as_str())
        .attr_bool(attr::THEOREM, a.is_theorem)
}

fn render_carrier_set(cs: &CarrierSetDecl) -> Element {
    Element::new(tag::SC_CARRIER_SET)
        .attr(attr::NAME, cs.name.clone())
        .attr(attr::SOURCE, cs.source.as_str())
        .attr(attr::TYPE, cs.ty.to_rodin_canonical())
}

fn render_constant(c: &ConstantDecl) -> Element {
    Element::new(tag::SC_CONSTANT)
        .attr(attr::NAME, c.name.clone())
        .attr(attr::SOURCE, c.source.as_str())
        .attr(attr::TYPE, c.ty.to_rodin_canonical())
}

fn render_extend(e: &ExtendsDecl, internal_name: String) -> Element {
    Element::new(tag::SC_EXTENDS_CONTEXT)
        .attr(attr::NAME, internal_name)
        .attr(attr::SC_TARGET, e.sc_target.clone())
        .attr(attr::SOURCE, e.source.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_uri() -> HandleUri {
        HandleUri::root("proj", "Ctx.buc", "org.eventb.core.contextFile", "Ctx")
    }

    #[test]
    fn render_body_emits_axioms_then_carrier_sets_then_constants() {
        let rec = ContextRecord {
            name: "Ctx".into(),
            filename: "Ctx.buc".into(),
            output_filename: "Ctx.bcc".into(),
            env: TypeEnv::new(),
            carrier_sets: vec![CarrierSetDecl {
                name: "S".into(),
                ty: Type::carrier_set_type("S"),
                source: mk_uri().child("org.eventb.core.carrierSet", "S"),
            }],
            constants: vec![ConstantDecl {
                name: "c".into(),
                ty: Type::Integer,
                source: mk_uri().child("org.eventb.core.constant", "c"),
            }],
            axioms: vec![AxiomDecl {
                label: "axm1".into(),
                source_index: 0,
                predicate: rossi::parse_predicate_str("c ∈ ℕ").unwrap(),
                predicate_canonical: "c∈ℕ".into(),
                is_theorem: false,
                source: mk_uri().child("org.eventb.core.axiom", "axm1"),
            }],
            extends: vec![],
            ancestors: vec![],
        };
        let body = render_body(&rec, &mut RodinNameGenerator::default());
        assert_eq!(body.len(), 3);
        assert_eq!(body[0].tag, tag::SC_AXIOM);
        assert_eq!(body[1].tag, tag::SC_CARRIER_SET);
        assert_eq!(body[2].tag, tag::SC_CONSTANT);
        assert!(
            body[0]
                .attrs
                .iter()
                .any(|(k, v)| k == attr::PREDICATE && v == "c∈ℕ")
        );
        assert!(
            body[1]
                .attrs
                .iter()
                .any(|(k, v)| k == attr::TYPE && v == "ℙ(S)")
        );
        assert!(
            body[2]
                .attrs
                .iter()
                .any(|(k, v)| k == attr::TYPE && v == "ℤ")
        );
    }

    #[test]
    fn render_extends_preserves_source_order() {
        let rec = ContextRecord {
            name: "Ctx".into(),
            filename: "Ctx.buc".into(),
            output_filename: "Ctx.bcc".into(),
            env: TypeEnv::new(),
            carrier_sets: vec![],
            constants: vec![],
            axioms: vec![],
            extends: vec![
                ExtendsDecl {
                    parent_name: "P1".into(),
                    sc_target: "/proj/P1.bcc|org.eventb.core.scContextFile#P1".into(),
                    source: mk_uri().child("org.eventb.core.extendsContext", "P1"),
                },
                ExtendsDecl {
                    parent_name: "P2".into(),
                    sc_target: "/proj/P2.bcc|org.eventb.core.scContextFile#P2".into(),
                    source: mk_uri().child("org.eventb.core.extendsContext", "P2"),
                },
            ],
            ancestors: vec![],
        };
        let ex = render_extends(&rec, &mut RodinNameGenerator::default());
        assert_eq!(ex.len(), 2);
        assert_eq!(ex[0].attr_value(attr::NAME), Some("'"));
        assert_eq!(ex[1].attr_value(attr::NAME), Some("("));
    }
}
