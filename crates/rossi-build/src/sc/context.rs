//! Context static checker: `.buc` → `.bcc`.
//!
//! Produces a [`ContextRecord`] (typed data) and renders it to XML.
//! Handles carrier sets, constants (with type inference from simple
//! axioms), axioms, theorems, and EXTENDS inheritance with
//! `scInternalContext` inlining.

use std::collections::HashMap;

use rossi::{Context, LabeledPredicate, NamedElement, SetDeclaration};

use crate::checked_predicate::check_labeled_predicate;
use crate::error::Result;
use crate::handles::HandleUri;
use crate::infer::infer_constants;
use crate::project::{Project, ProjectComponent};
use crate::rodin_ids::{Kind, RodinIds};
use crate::type_env::TypeEnv;
use crate::types::Type;
use crate::xml_out::{Element, attr, in_tag, tag};
use crate::{Diagnostic, ScFile, Severity};

use super::CheckedContext;
use super::context_record::{
    AxiomDecl, CarrierSetDecl, ConstantDecl, ContextRecord, ExtendsDecl, render_body,
    render_extends,
};

/// Emit a `.bcc` for a single context.
///
/// `checked` contains contexts already processed by the pipeline — used
/// to resolve EXTENDS dependencies.
pub fn check_context(
    project: &Project,
    pc: &ProjectComponent,
    ctx: &Context,
    checked: &HashMap<String, CheckedContext>,
) -> Result<(ScFile, CheckedContext, Vec<Diagnostic>)> {
    let mut diags = Vec::new();
    let mut accurate = true;

    // -----------------------------------------------------------------
    // Environment — inherit from extends, add own carrier sets, then
    // infer constants.
    // -----------------------------------------------------------------
    let mut env = TypeEnv::new();
    for parent_name in &ctx.extends {
        match checked.get(parent_name) {
            Some(parent) => {
                merge_env(&mut env, parent.env());
                // Extending an inaccurate context makes this context
                // inaccurate too (silent — the inaccuracy is the parent's
                // own reported problem).
                if !parent.accurate {
                    accurate = false;
                }
            }
            None => {
                diags.push(Diagnostic {
                    severity: Severity::Error,
                    origin: ctx.name.clone(),
                    message: format!("EXTENDS unknown context '{parent_name}'"),
                    rule_id: Some(crate::RuleId::CrossReferenceNotFound),
                    // EXTENDS targets carry no per-entry span; anchor on the
                    // context name.
                    span: ctx.name_span,
                });
                accurate = false;
            }
        }
    }
    // Filter duplicate identifiers / axiom labels per the SC drop semantics
    // documented in `crate::duplicates` (identifiers drop every occurrence;
    // axiom labels keep the first). Enumerated set *elements* have no SC
    // declaration of their own, so for them only the up-front diagnostic
    // applies.
    let dups = crate::duplicates::context_duplicates(ctx);
    let dup_ids = dups.identifiers.names;
    let dup_axiom_labels = dups.axiom_labels.names;

    for set in &ctx.sets {
        if dup_ids.contains(set.name()) {
            continue;
        }
        env.add_carrier_set(set.name());
    }

    // Dropped 2nd+ occurrences of a duplicated axiom label must not
    // contribute typing either; the kept first occurrence still types.
    let mut typing_kept = crate::duplicates::FirstKept::new(&dup_axiom_labels);
    let axiom_preds: Vec<_> = ctx
        .axioms
        .iter()
        .filter(|ax| !typing_kept.drops(ax.label.as_deref()))
        .map(|a| a.predicate.clone())
        .collect();
    // Duplicated constants are dropped, not typed — they are EB021 errors,
    // not EB006 inference failures.
    let constant_names: Vec<String> = ctx
        .constants
        .iter()
        .filter(|c| !dup_ids.contains(&c.name))
        .map(|c| c.name.clone())
        .collect();
    let unresolved = infer_constants(&mut env, &constant_names, &axiom_preds);
    for name in &unresolved {
        diags.push(Diagnostic {
            severity: Severity::Error,
            origin: format!("{}.{}", ctx.name, name),
            message: "could not infer type from axioms (no typing axiom found)".to_string(),
            rule_id: Some(crate::RuleId::TypeError),
            span: crate::ast_util::named_element_span(&ctx.constants, name),
        });
        accurate = false;
    }

    // -----------------------------------------------------------------
    // Build typed decls — this is the durable record; XML is derived.
    // -----------------------------------------------------------------
    let file_root = HandleUri::root(
        &project.name,
        &pc.filename,
        "org.eventb.core.contextFile",
        &ctx.name,
    );

    let extends = build_extends_decls(&pc.rodin_ids, &file_root, project, ctx, checked);

    let mut axioms: Vec<AxiomDecl> = Vec::with_capacity(ctx.axioms.len());
    let mut emit_kept = crate::duplicates::FirstKept::new(&dup_axiom_labels);
    for (i, ax) in ctx.axioms.iter().enumerate() {
        // 2nd+ use of a duplicated axiom label: Rodin keeps the first
        // occurrence, drops the rest, and marks the context inaccurate (the
        // EB022 error is already reported).
        if emit_kept.drops(ax.label.as_deref()) {
            accurate = false;
            continue;
        }
        match build_axiom_decl(&pc.rodin_ids, &file_root, i, ax, &env, &ctx.name) {
            Ok(decl) => axioms.push(decl),
            Err(diag) => {
                accurate = false;
                diags.push(diag);
            }
        }
    }

    let mut carrier_sets: Vec<CarrierSetDecl> = ctx
        .sets
        .iter()
        .filter(|s| !dup_ids.contains(s.name()))
        .map(|s| build_carrier_set_decl(&pc.rodin_ids, &file_root, s))
        .collect();
    carrier_sets.sort_by(|a, b| a.name.cmp(&b.name));

    // The explicit duplicate filter matters even though `env.contains` is
    // also checked: an EXTENDS parent exporting the same name would leave
    // it in `env`, and both local duplicates would emit.
    let mut constants: Vec<ConstantDecl> = ctx
        .constants
        .iter()
        .filter(|c| !dup_ids.contains(&c.name) && env.contains(&c.name))
        .map(|c| build_constant_decl(&pc.rodin_ids, &file_root, c, &env))
        .collect();
    constants.sort_by(|a, b| a.name.cmp(&b.name));

    let ancestors = collect_ancestors(&ctx.extends, checked);

    let record = ContextRecord {
        name: ctx.name.clone(),
        filename: pc.filename.clone(),
        output_filename: pc.output_filename(),
        env,
        carrier_sets,
        constants,
        axioms,
        extends,
        ancestors,
    };

    // -----------------------------------------------------------------
    // Render to XML.
    // -----------------------------------------------------------------
    let extends_elems = render_extends(&record);
    let own_body = render_body(&record);

    let configuration = ctx
        .metadata
        .as_ref()
        .and_then(|m| m.configuration.clone())
        .unwrap_or_else(|| "org.eventb.core.fwd".to_string());
    let mut root = Element::new(tag::SC_CONTEXT_FILE)
        .attr_bool(attr::ACCURATE, accurate)
        .attr(attr::CONFIGURATION, configuration);

    // Direct-parent scExtendsContext elements first.
    for el in &extends_elems {
        root.push(el.clone());
    }

    // Hoisted scInternalContext for every transitively-extended ancestor.
    for ancestor in &record.ancestors {
        let Some(parent) = checked.get(ancestor) else {
            continue;
        };
        let mut ic = Element::new(tag::SC_INTERNAL_CONTEXT).attr(attr::NAME, ancestor.as_str());
        for el in &parent.extends_elems {
            ic.push(el.clone());
        }
        for el in &parent.body {
            ic.push(el.clone());
        }
        root.push(ic);
    }

    // Own body (axioms → carrier sets → constants).
    for el in &own_body {
        root.push(el.clone());
    }

    let file = ScFile {
        filename: pc.output_filename(),
        contents: root.to_document(),
        accurate,
    };

    let cc = CheckedContext {
        record,
        body: own_body,
        extends_elems,
        accurate,
    };

    Ok((file, cc, diags))
}

// ---------------------------------------------------------------------
// Decl builders — pure record construction, no XML.
// ---------------------------------------------------------------------

fn build_extends_decls(
    ids: &RodinIds,
    file_root: &HandleUri,
    project: &Project,
    ctx: &Context,
    checked: &HashMap<String, CheckedContext>,
) -> Vec<ExtendsDecl> {
    let mut out = Vec::with_capacity(ctx.extends.len());
    for parent_name in &ctx.extends {
        let Some(parent) = checked.get(parent_name) else {
            continue;
        };
        let source = crate::sc::file_child_source(
            ids,
            file_root,
            Kind::ExtendsContext,
            in_tag::EXTENDS_CONTEXT,
            parent_name,
        );
        let sc_target = format!(
            "/{}/{}|org.eventb.core.scContextFile#{}",
            project.name,
            parent.output_filename(),
            parent.name()
        );
        out.push(ExtendsDecl {
            parent_name: parent_name.clone(),
            sc_target,
            source,
        });
    }
    out
}

fn build_carrier_set_decl(
    ids: &RodinIds,
    file_root: &HandleUri,
    set: &SetDeclaration,
) -> CarrierSetDecl {
    let name = set.name();
    CarrierSetDecl {
        name: name.to_string(),
        ty: Type::carrier_set_type(name),
        source: crate::sc::file_child_source(
            ids,
            file_root,
            Kind::CarrierSet,
            in_tag::CARRIER_SET,
            name,
        ),
    }
}

fn build_constant_decl(
    ids: &RodinIds,
    file_root: &HandleUri,
    c: &NamedElement,
    env: &TypeEnv,
) -> ConstantDecl {
    let ty = env.get(&c.name).cloned().unwrap_or(Type::Integer);
    ConstantDecl {
        name: c.name.clone(),
        ty,
        source: crate::sc::file_child_source(
            ids,
            file_root,
            Kind::Constant,
            in_tag::CONSTANT,
            &c.name,
        ),
    }
}

fn build_axiom_decl(
    ids: &RodinIds,
    file_root: &HandleUri,
    source_index: usize,
    ax: &LabeledPredicate,
    env: &TypeEnv,
    ctx_name: &str,
) -> std::result::Result<AxiomDecl, Diagnostic> {
    let (label, pc) =
        check_labeled_predicate(ax, env, "axm", "axiom", |lbl| format!("{ctx_name}.{lbl}"))?;
    let source = crate::sc::file_child_source(ids, file_root, Kind::Axiom, in_tag::AXIOM, &label);
    Ok(AxiomDecl {
        label,
        source_index,
        predicate_canonical: pc.canonical,
        predicate: pc.predicate,
        is_theorem: ax.is_theorem,
        source,
    })
}

// ---------------------------------------------------------------------
// Helpers shared with upcoming machine SC.
// ---------------------------------------------------------------------

/// Flatten the transitive-closure of EXTENDS, in topological order
/// (root ancestor first, direct parent last). Duplicates — a diamond —
/// are kept only once (first occurrence wins).
fn collect_ancestors(direct: &[String], checked: &HashMap<String, CheckedContext>) -> Vec<String> {
    use std::collections::HashSet;
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for parent_name in direct {
        let Some(parent) = checked.get(parent_name) else {
            continue;
        };
        for gp in parent.ancestors() {
            if seen.insert(gp.clone()) {
                out.push(gp.clone());
            }
        }
        if seen.insert(parent_name.clone()) {
            out.push(parent_name.clone());
        }
    }
    out
}

fn merge_env(into: &mut TypeEnv, other: &TypeEnv) {
    for (k, v) in other.iter() {
        into.insert_if_absent(k, v.clone());
    }
}
