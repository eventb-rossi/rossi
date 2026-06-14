//! The static-checker pipeline.
//!
//! Orchestrates per-component static checks and collects their outputs into
//! a [`BuildResult`]. Individual checkers live in submodules (`context`,
//! later `machine`, `events`, …).

use std::collections::HashMap;
use std::rc::Rc;

use rossi::deps::{DependencyGraph, EdgeKind};

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
    /// The typed record this machine's `.bcm` was rendered from —
    /// name, output filename, environment, invariants, variant, events,
    /// and ancestors in enriched-AST form. The machine analogue of
    /// [`CheckedContext::record`] and the single home for those fields;
    /// downstream passes (well-definedness, IDE tooling) read them via
    /// the accessors below.
    pub record: machine_record::MachineRecord,
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
}

impl CheckedMachine {
    pub fn name(&self) -> &str {
        &self.record.name
    }
    pub fn output_filename(&self) -> &str {
        &self.record.output_filename
    }
    pub fn env(&self) -> &TypeEnv {
        &self.record.env
    }
    /// Transitively-refined ancestor machine names, oldest first.
    pub fn ancestors(&self) -> &[String] {
        &self.record.ancestors
    }

    /// The type environment in scope inside `event`: the machine env
    /// (variables + seen constants) extended with every parameter of
    /// the event's extended-event chain. `event` should come from this
    /// machine's [`Self::events_by_label`] / record.
    pub fn event_env(&self, event: &machine_record::EventDecl) -> TypeEnv {
        let mut env = self.env().clone();
        env.push_scope();
        for p in event.chain_parameters() {
            env.insert(p.name.clone(), p.ty.clone());
        }
        env
    }
}

/// The typed model retained after a build: every successfully-checked
/// component, keyed by name. [`crate::build`] discards it;
/// [`crate::build_with_model`] returns it for downstream passes that need
/// the resolved type environments and event records (well-definedness,
/// IDE tooling) without re-deriving them from the emitted XML.
#[derive(Debug, Default)]
pub struct ScModel {
    pub contexts: HashMap<String, CheckedContext>,
    pub machines: HashMap<String, CheckedMachine>,
}

/// Entry point called from [`crate::build`] / [`crate::build_with_model`].
pub fn build_project(project: &Project) -> (BuildResult, ScModel) {
    let mut result = BuildResult::default();
    let mut checked: HashMap<String, CheckedContext> = HashMap::new();

    // The shared dependency graph is the single source of truth for the
    // EXTENDS / REFINES / SEES visibility semantics (see `rossi::deps`).
    let graph = DependencyGraph::from_components(project.components.iter().map(|pc| &pc.component));

    // Topologically sort contexts by EXTENDS dependency.
    let order = match topo_indices(project, &graph, EdgeKind::Extends) {
        Ok(o) => o,
        Err(cycle) => {
            result.diagnostics.push(crate::Diagnostic {
                severity: Severity::Error,
                origin: "project".into(),
                message: format!("circular EXTENDS: {}", cycle.join(" → ")),
                rule_id: Some(crate::RuleId::CircularExtends),
            });
            return (result, ScModel::default());
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
    let mach_order = match topo_indices(project, &graph, EdgeKind::Refines) {
        Ok(o) => o,
        Err(cycle) => {
            result.diagnostics.push(crate::Diagnostic {
                severity: Severity::Error,
                origin: "project".into(),
                message: format!("circular REFINES: {}", cycle.join(" → ")),
                rule_id: Some(crate::RuleId::CircularRefines),
            });
            return (
                result,
                ScModel {
                    contexts: checked,
                    machines: HashMap::new(),
                },
            );
        }
    };
    // NOTE: `checked` / `checked_machines` keep only components whose check
    // succeeded — failed components have diagnostics but no model entry.
    let mut checked_machines: HashMap<String, CheckedMachine> = HashMap::new();
    for idx in mach_order {
        let pc = &project.components[idx];
        let m = match &pc.component {
            rossi::Component::Machine(m) => m,
            _ => continue,
        };
        match machine::check_machine(project, pc, m, &checked, &checked_machines) {
            Ok((file, cm, mut diags)) => {
                checked_machines.insert(cm.name().to_string(), cm);
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

    (
        result,
        ScModel {
            contexts: checked,
            machines: checked_machines,
        },
    )
}

/// Map the shared dependency graph's topological order for `edge` back to
/// indices into `project.components`, parents first.
///
/// Only nodes of `edge.source_kind()` participate (contexts for
/// [`EdgeKind::Extends`], machines for [`EdgeKind::Refines`]). Returns
/// `Err(cycle)` if a cycle exists; `cycle` is a list of component names along
/// the cycle, useful for diagnostics.
fn topo_indices(
    project: &Project,
    graph: &DependencyGraph,
    edge: EdgeKind,
) -> std::result::Result<Vec<usize>, Vec<String>> {
    let mut name_to_idx: HashMap<&str, usize> = HashMap::new();
    for (i, pc) in project.components.iter().enumerate() {
        let name = match (&pc.component, edge) {
            (rossi::Component::Context(c), EdgeKind::Extends) => Some(c.name.as_str()),
            (rossi::Component::Machine(m), EdgeKind::Refines) => Some(m.name.as_str()),
            _ => None,
        };
        if let Some(name) = name {
            name_to_idx.insert(name, i);
        }
    }

    match graph.topological_order(edge) {
        Ok(order) => Ok(order
            .iter()
            .filter_map(|name| name_to_idx.get(name.as_str()).copied())
            .collect()),
        Err(cycle) => Err(cycle.components),
    }
}
