//! Proof obligations of a machine: well-definedness and provability of
//! its own invariants, and the well-definedness and finiteness of its
//! variant.
//!
//! The hypothesis stack starts at `CTXHYP` — the seen contexts'
//! identifiers and axioms — followed by `ABSHYP` with every machine
//! variable as a typed identifier and the refinement ancestors'
//! invariants as plain hypotheses (they were proved in the
//! abstraction). The machine's own invariants form the incremental
//! table: invariant *n*'s obligations hypothesize exactly the
//! invariants declared before it.

use rossi::formula::Type;

use crate::ScFile;
use crate::project::Project;
use crate::sc::{CheckedMachine, ScModel};
use crate::xml_out::{Element, RodinNameGenerator, attr, tag as xtag};

use super::context::push_context_hypotheses;
use super::hyp::{
    ABS_HYP_NAME, ALL_HYP_NAME, CTX_HYP_NAME, HYP_PREFIX, HypothesisManager, HypothesisRow,
};
use super::model::{Hint, PoFile, PogPredicate, PogSource, ProofObligation, Role, is_trivial};
use super::natures::Nature;

/// Generate `M.bpo` and `M.bps` for a checked machine.
pub(super) fn generate(
    project: &Project,
    model: &ScModel,
    machine: &CheckedMachine,
) -> (ScFile, ScFile) {
    let mut po = PoFile::new(&project.name, machine.name());

    // CTXHYP: the seen contexts' identifiers and axioms, in hoist order.
    let mut ctx_hyp = Element::new(xtag::PO_PREDICATE_SET)
        .attr(attr::NAME, CTX_HYP_NAME)
        .attr(attr::PO_STAMP, "0");
    let mut ctx_names = RodinNameGenerator::default();
    for context in model.seen_contexts(machine) {
        push_context_hypotheses(&mut ctx_hyp, &mut ctx_names, &context.record);
    }
    po.push(ctx_hyp);

    // ABSHYP: every machine variable as a typed identifier, then the
    // refinement ancestors' invariants as plain hypotheses.
    let mut abs_hyp = Element::new(xtag::PO_PREDICATE_SET)
        .attr(attr::NAME, ABS_HYP_NAME)
        .attr(attr::PARENT_SET, po.set_handle(CTX_HYP_NAME).as_str())
        .attr(attr::PO_STAMP, "0");
    let mut abs_names = RodinNameGenerator::default();
    for variable in &machine.record.variables {
        abs_names.observe(&variable.name);
        abs_hyp.push(
            Element::new(xtag::PO_IDENTIFIER)
                .attr(attr::NAME, &variable.name)
                .attr(attr::TYPE, variable.ty.to_rodin_canonical()),
        );
    }
    for invariant in model.inherited_invariants(machine) {
        abs_hyp.push(super::model::predicate_element(
            abs_names.fresh(),
            &invariant.typed,
            &invariant.source,
        ));
    }
    po.push(abs_hyp);

    // The machine's own invariants form the incremental table.
    let internal_names = own_invariant_internal_names(machine);
    let rows: Vec<HypothesisRow> = machine
        .record
        .invariants
        .iter()
        .zip(&internal_names)
        .map(|(invariant, internal_name)| HypothesisRow {
            internal_name: internal_name.clone(),
            predicate: invariant.typed.clone(),
            source: invariant.source.clone(),
        })
        .collect();
    let mut manager = HypothesisManager::new(
        ABS_HYP_NAME,
        super::hyp::IDENT_HYP_NAME,
        HYP_PREFIX,
        ALL_HYP_NAME,
        rows,
    );

    for (index, invariant) in machine.record.invariants.iter().enumerate() {
        let wd_nature = if invariant.is_theorem {
            Nature::TheoremWellDefinedness
        } else {
            Nature::InvariantWellDefinedness
        };
        let wd = invariant.typed.wd_lemma();
        if !is_trivial(&wd) {
            let hypothesis = manager.make_hypothesis(index);
            po.create_po(ProofObligation {
                name: format!("{}/WD", invariant.label),
                nature: wd_nature,
                global_hypotheses: po.set_handle(&hypothesis),
                local_hypotheses: Vec::new(),
                goal: PogPredicate::new(wd, invariant.source.clone()),
                sources: vec![PogSource::new(Role::Default, invariant.source.clone())],
                hints: vec![Hint::Interval {
                    start: po.set_handle(manager.root_hypothesis()),
                    end: po.set_handle(&hypothesis),
                }],
                accurate: machine.accurate,
            });
        }

        if invariant.is_theorem && !is_trivial(&invariant.typed) {
            let hypothesis = manager.make_hypothesis(index);
            po.create_po(ProofObligation {
                name: format!("{}/THM", invariant.label),
                nature: Nature::Theorem,
                global_hypotheses: po.set_handle(&hypothesis),
                local_hypotheses: Vec::new(),
                goal: PogPredicate::new(invariant.typed.clone(), invariant.source.clone()),
                sources: vec![PogSource::new(Role::Default, invariant.source.clone())],
                hints: vec![Hint::Interval {
                    start: po.set_handle(manager.root_hypothesis()),
                    end: po.set_handle(&hypothesis),
                }],
                accurate: machine.accurate,
            });
        }
    }

    generate_variant_pos(machine, &mut po);

    let ff = machine_factory(machine);
    let variables = super::tables::MachineVariables::new(&machine.record);
    for event in &machine.record.events {
        let mut scope = super::event::EventScope::new(model, machine, &variables, event, &ff);
        super::event::generate_event(
            &mut po,
            &mut scope,
            &manager,
            &machine.record.invariants,
            &machine.record.variants,
        );
    }

    manager.create_hypotheses(&mut po);
    po.into_sc_files(machine.accurate)
}

/// The formula factory the machine's typed formulas were built with —
/// every formula of one project shares it. Falls back to the core
/// factory for a machine with no formulas at all.
fn machine_factory(machine: &CheckedMachine) -> rossi::formula::FormulaFactory {
    machine
        .record
        .invariants
        .first()
        .map(|invariant| invariant.typed.factory().clone())
        .or_else(|| {
            machine.record.events.iter().find_map(|event| {
                event
                    .guards
                    .first()
                    .map(|guard| guard.typed.factory().clone())
                    .or_else(|| {
                        event
                            .actions
                            .iter()
                            .find_map(|action| action.typed.as_ref())
                            .map(|assignment| assignment.factory().clone())
                    })
            })
        })
        .unwrap_or_else(rossi::formula::FormulaFactory::default_factory)
}

/// `VWD` (well-definedness) and `FIN` (finiteness), per variant in
/// declaration order.
fn generate_variant_pos(machine: &CheckedMachine, po: &mut PoFile) {
    let variants = &machine.record.variants;
    let single_default = single_default_variant(variants);
    for variant in variants {
        let Some(typed) = &variant.typed else {
            continue;
        };
        let sources = vec![PogSource::new(Role::Default, variant.source.clone())];

        po.create_po(ProofObligation {
            name: variant_po_name(single_default, &variant.label, "VWD"),
            nature: Nature::VariantWellDefinedness,
            global_hypotheses: po.set_handle(ALL_HYP_NAME),
            local_hypotheses: Vec::new(),
            goal: PogPredicate::new(typed.wd_lemma(), variant.source.clone()),
            sources: sources.clone(),
            hints: Vec::new(),
            accurate: machine.accurate,
        });

        if let Some(ty) = typed.ty()
            && must_prove_finite(ty)
        {
            let finite = typed.factory().simple_predicate(typed.clone(), None);
            po.create_po(ProofObligation {
                name: variant_po_name(single_default, &variant.label, "FIN"),
                nature: Nature::VariantFiniteness,
                global_hypotheses: po.set_handle(ALL_HYP_NAME),
                local_hypotheses: Vec::new(),
                goal: PogPredicate::new(finite, variant.source.clone()),
                sources,
                hints: Vec::new(),
                accurate: machine.accurate,
            });
        }
    }
}

/// Whether the machine's only variant carries the default label — the
/// one shape whose obligation names omit the label segment.
pub(super) fn single_default_variant(variants: &[crate::sc::machine_record::VariantDecl]) -> bool {
    variants.len() == 1 && variants[0].label == rossi::DEFAULT_VARIANT_LABEL
}

/// A machine whose only variant carries the default label omits the
/// label segment; every other shape includes it.
pub(super) fn variant_po_name(single_default: bool, label: &str, suffix: &str) -> String {
    if single_default {
        suffix.to_string()
    } else {
        format!("{label}/{suffix}")
    }
}

/// An integer variant decreases, so it needs no finiteness proof; a
/// set variant does, unless its type is finite by construction. A given
/// set can be infinite, so only the type's own structure is trusted here.
fn must_prove_finite(ty: &Type) -> bool {
    !matches!(ty, Type::Int) && !ty.is_finite_with(&|_| false)
}

/// The checked-file internal names of the machine's own invariants —
/// the tail of the rendered invariant closure, in declaration order.
fn own_invariant_internal_names(machine: &CheckedMachine) -> Vec<String> {
    let own = machine.record.invariants.len();
    let elems = &machine.invariant_elems;
    elems[elems.len().saturating_sub(own)..]
        .iter()
        .map(|element| {
            element
                .attr_value(attr::NAME)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}
