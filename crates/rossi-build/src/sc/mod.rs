//! The static-checker pipeline.
//!
//! Orchestrates per-component static checks and collects their outputs into
//! a [`BuildResult`]. Individual checkers live in submodules (`context`,
//! later `machine`, `events`, …).

use std::collections::HashMap;
use std::rc::Rc;

use crate::handles::HandleUri;
use crate::project::Project;
use crate::rodin_ids::{Kind, RodinIds, Scope};
use crate::type_env::TypeEnv;
use crate::xml_out::Element;
use crate::{BuildResult, Severity};

pub mod context;
pub mod context_record;
pub mod identifier_walker;
pub mod machine;
pub mod machine_record;

use machine_record::EventDecl;

/// Build the `source=` URI for a file-scoped child element of a context
/// or machine root: `<file_root>/<child_tag>` with a `Scope::File` id
/// lookup. Centralises the `get_or` + `.child()` pair used at every
/// axiom / invariant / constant / variable / sees / refines / variant
/// / extends call site.
pub(crate) fn file_child_source(
    ids: &RodinIds,
    file_root: &HandleUri,
    kind: Kind,
    child_tag: &str,
    label: &str,
) -> HandleUri {
    let id = ids.get_or(Scope::File, kind, label);
    file_root.child(child_tag, id)
}

/// Everything about a successfully-checked context that its dependents
/// need. Built once per context and then consumed by:
///   - child contexts (via EXTENDS)
///   - machines (via SEES)
///
/// The typed `record` is the canonical form; `body` / `extends_elems`
/// are XML renderings cached here so dependent contexts can cheaply
/// splice them into their `scInternalContext` blocks without re-rendering.
#[derive(Debug, Clone)]
pub struct CheckedContext {
    pub record: context_record::ContextRecord,
    /// Rendered `scAxiom` / `scCarrierSet` / `scConstant` rows for
    /// this context's body. `Rc`-shared so descendant contexts and
    /// machines that hoist us into their `scInternalContext` only pay
    /// a refcount bump per element.
    pub body: Vec<Rc<Element>>,
    /// Rendered `scExtendsContext` rows. Same sharing semantics as
    /// `body`.
    pub extends_elems: Vec<Rc<Element>>,
}

impl CheckedContext {
    pub fn name(&self) -> &str {
        &self.record.name
    }
    pub fn ancestors(&self) -> &[String] {
        &self.record.ancestors
    }
    pub fn env(&self) -> &TypeEnv {
        &self.record.env
    }
    pub fn output_filename(&self) -> &str {
        &self.record.output_filename
    }
}

/// Everything a dependent machine needs after a machine was checked:
/// - its environment (variables + constants from seen contexts),
/// - the set of variables it declares (so refinements can mark
///   `abstract=true` on inherited ones),
/// - the `scInvariant` elements it emitted (so refinements can inherit
///   them verbatim, preserving `source=` URIs that already point back
///   to the correct `.bum`),
/// - transitively-refined ancestor machine names.
#[derive(Debug, Clone)]
pub struct CheckedMachine {
    pub name: String,
    pub output_filename: String,
    pub env: TypeEnv,
    /// Names of every variable visible at the end of checking this
    /// machine — own + inherited from the REFINES chain.
    pub visible_variables: std::collections::BTreeSet<String>,
    /// Rendered `scInvariant` XML elements — full ancestor closure +
    /// own. Dependents splice these in once to get the complete
    /// invariant chain. `Rc`-shared so a refining machine inherits
    /// the closure with O(N) refcount bumps instead of N deep clones.
    pub invariant_elems: Vec<Rc<Element>>,
    /// Typed event records keyed by event label. Descendants extending
    /// an event read the `Rc<EventDecl>` and use it as the
    /// `inherited` parent for their own EventDecl chain — typed
    /// `Predicate` and `Action` ASTs survive all the way through, no
    /// XML round-trip required.
    pub events_by_label: HashMap<String, Rc<EventDecl>>,
    /// Transitively-refined ancestor machine names, oldest first.
    pub ancestors: Vec<String>,
}

/// Entry point called from [`crate::build`].
pub fn build_project(project: &Project) -> BuildResult {
    let mut result = BuildResult::default();
    let mut checked: HashMap<String, CheckedContext> = HashMap::new();

    // Topologically sort contexts by EXTENDS dependency.
    let order = match topo_sort_contexts(project) {
        Ok(o) => o,
        Err(cycle) => {
            result.diagnostics.push(crate::Diagnostic {
                severity: Severity::Error,
                origin: "project".into(),
                message: format!("circular EXTENDS: {}", cycle.join(" → ")),
                rule_id: Some(crate::RuleId::CircularExtends),
            });
            return result;
        }
    };

    for idx in order {
        let pc = &project.components[idx];
        let ctx = match &pc.component {
            rossi::Component::Context(c) => c,
            _ => continue,
        };
        match context::check_context(project, pc, ctx, &checked) {
            Ok((file, cc, mut diags)) => {
                checked.insert(cc.name().to_string(), cc);
                result.files.push(file);
                result.diagnostics.append(&mut diags);
            }
            Err(e) => {
                result.diagnostics.push(crate::Diagnostic {
                    severity: Severity::Error,
                    origin: ctx.name.clone(),
                    message: format!("failed to check context: {e}"),
                    rule_id: None,
                });
            }
        }
    }

    // Machines: emit after all contexts have been checked so SEES
    // targets are available. Topo-sort across REFINES ensures parents
    // are processed before children.
    let mach_order = match topo_sort_machines(project) {
        Ok(o) => o,
        Err(cycle) => {
            result.diagnostics.push(crate::Diagnostic {
                severity: Severity::Error,
                origin: "project".into(),
                message: format!("circular REFINES: {}", cycle.join(" → ")),
                rule_id: Some(crate::RuleId::CircularRefines),
            });
            return result;
        }
    };
    let mut checked_machines: HashMap<String, CheckedMachine> = HashMap::new();
    for idx in mach_order {
        let pc = &project.components[idx];
        let m = match &pc.component {
            rossi::Component::Machine(m) => m,
            _ => continue,
        };
        match machine::check_machine(project, pc, m, &checked, &checked_machines) {
            Ok((file, cm, mut diags)) => {
                checked_machines.insert(cm.name.clone(), cm);
                result.files.push(file);
                result.diagnostics.append(&mut diags);
            }
            Err(e) => {
                result.diagnostics.push(crate::Diagnostic {
                    severity: Severity::Error,
                    origin: m.name.clone(),
                    message: format!("failed to check machine: {e}"),
                    rule_id: None,
                });
            }
        }
    }

    result
}

fn topo_sort_machines(project: &Project) -> Result<Vec<usize>, Vec<String>> {
    use std::collections::HashSet;

    let mut name_to_idx: HashMap<&str, usize> = HashMap::new();
    for (i, pc) in project.components.iter().enumerate() {
        if let rossi::Component::Machine(m) = &pc.component {
            name_to_idx.insert(&m.name, i);
        }
    }

    let mut order = Vec::new();
    let mut visited: HashSet<usize> = HashSet::new();
    let mut in_stack: HashSet<usize> = HashSet::new();

    fn visit(
        idx: usize,
        project: &Project,
        name_to_idx: &HashMap<&str, usize>,
        visited: &mut HashSet<usize>,
        in_stack: &mut HashSet<usize>,
        order: &mut Vec<usize>,
        stack_names: &mut Vec<String>,
    ) -> std::result::Result<(), Vec<String>> {
        if visited.contains(&idx) {
            return Ok(());
        }
        if !in_stack.insert(idx) {
            return Err(stack_names.clone());
        }
        let m = match &project.components[idx].component {
            rossi::Component::Machine(m) => m,
            _ => {
                in_stack.remove(&idx);
                return Ok(());
            }
        };
        stack_names.push(m.name.clone());
        if let Some(parent) = m.refines.as_deref()
            && let Some(&pi) = name_to_idx.get(parent)
        {
            visit(
                pi,
                project,
                name_to_idx,
                visited,
                in_stack,
                order,
                stack_names,
            )?;
        }
        stack_names.pop();
        in_stack.remove(&idx);
        visited.insert(idx);
        order.push(idx);
        Ok(())
    }

    for i in 0..project.components.len() {
        let mut stack = Vec::new();
        visit(
            i,
            project,
            &name_to_idx,
            &mut visited,
            &mut in_stack,
            &mut order,
            &mut stack,
        )?;
    }
    Ok(order)
}

/// Topologically sort the project's contexts by EXTENDS. Returns component
/// indices (into `project.components`) in dependency order (parents first).
///
/// Returns `Err(cycle)` if a cycle exists; `cycle` is a list of context
/// names along the cycle, useful for diagnostics.
fn topo_sort_contexts(project: &Project) -> Result<Vec<usize>, Vec<String>> {
    use std::collections::HashSet;

    // Map context name → component index.
    let mut name_to_idx: HashMap<&str, usize> = HashMap::new();
    for (i, pc) in project.components.iter().enumerate() {
        if let rossi::Component::Context(c) = &pc.component {
            name_to_idx.insert(&c.name, i);
        }
    }

    let mut order = Vec::new();
    let mut visited: HashSet<usize> = HashSet::new();
    let mut in_stack: HashSet<usize> = HashSet::new();

    fn visit(
        idx: usize,
        project: &Project,
        name_to_idx: &HashMap<&str, usize>,
        visited: &mut std::collections::HashSet<usize>,
        in_stack: &mut std::collections::HashSet<usize>,
        order: &mut Vec<usize>,
        stack_names: &mut Vec<String>,
    ) -> Result<(), Vec<String>> {
        if visited.contains(&idx) {
            return Ok(());
        }
        if !in_stack.insert(idx) {
            return Err(stack_names.clone());
        }
        let ctx = match &project.components[idx].component {
            rossi::Component::Context(c) => c,
            _ => {
                in_stack.remove(&idx);
                return Ok(());
            }
        };
        stack_names.push(ctx.name.clone());
        for parent in &ctx.extends {
            if let Some(&pi) = name_to_idx.get(parent.as_str()) {
                visit(
                    pi,
                    project,
                    name_to_idx,
                    visited,
                    in_stack,
                    order,
                    stack_names,
                )?;
            }
            // Unknown parents (referenced by name but not in project) are
            // silently ignored here — surfaced as a diagnostic during
            // check_context.
        }
        stack_names.pop();
        in_stack.remove(&idx);
        visited.insert(idx);
        order.push(idx);
        Ok(())
    }

    for i in 0..project.components.len() {
        let mut stack = Vec::new();
        visit(
            i,
            project,
            &name_to_idx,
            &mut visited,
            &mut in_stack,
            &mut order,
            &mut stack,
        )?;
    }
    Ok(order)
}
