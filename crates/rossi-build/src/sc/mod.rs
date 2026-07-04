//! The static-checker pipeline.
//!
//! Orchestrates per-component static checks and collects their outputs into
//! a [`BuildResult`]. Individual checkers live in submodules (`context`,
//! later `machine`, `events`, …).

use std::collections::{BTreeMap, HashMap};
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

/// Canonical INITIALISATION event label. In Event-B the static-checker
/// event label is the grammar keyword itself, so it is sourced from rossi's
/// keyword table rather than spelled out at each use site.
pub(crate) fn initialisation_label() -> &'static str {
    rossi::keywords::spell(rossi::keywords::KeywordId::Initialisation)
}

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
    /// The `accurate` flag of this context's emitted `ScFile`. Dependents
    /// read it to propagate inaccuracy: a context that EXTENDS an
    /// inaccurate context, or a machine that SEES one, is itself
    /// inaccurate.
    pub accurate: bool,
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
    /// The `accurate` flag of this machine's emitted `ScFile`. A refining
    /// machine reads it to propagate inaccuracy: refining an inaccurate
    /// machine makes the refinement inaccurate too.
    pub accurate: bool,
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

/// EB019: every component name must be unique within a project — a duplicate
/// makes every SEES/EXTENDS/REFINES reference to the name ambiguous, and the
/// emitted `.bcc`/`.bcm` file identities collide. Rodin cannot even represent
/// the state: a component's name is its file identity, and the per-name proof
/// files are shared across kinds, so a machine and a context may not share a
/// name either.
fn duplicate_component_diagnostics(project: &Project) -> Vec<crate::Diagnostic> {
    let mut by_name: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for pc in &project.components {
        by_name
            .entry(pc.component.name())
            .or_default()
            .push(pc.filename.as_str());
    }
    by_name
        .into_iter()
        .filter(|(_, files)| files.len() > 1)
        .map(|(name, files)| {
            // A text file may hold several components, so the duplicates can
            // all live in ONE file — say so instead of listing it twice.
            let mut distinct = files.clone();
            distinct.dedup();
            let message = if distinct.len() == 1 {
                format!(
                    "component `{name}` is defined {} times in {}",
                    files.len(),
                    distinct[0]
                )
            } else {
                format!(
                    "component `{name}` is defined in multiple files: {}",
                    distinct.join(", ")
                )
            };
            crate::Diagnostic {
                severity: crate::RuleId::DuplicateComponent.default_severity(),
                origin: name.to_string(),
                message,
                rule_id: Some(crate::RuleId::DuplicateComponent),
                // A duplicate-component finding is about file paths, not a
                // single source location — no span to attach.
                span: None,
            }
        })
        .collect()
}

/// Entry point called from [`crate::build`] / [`crate::build_with_model`].
pub fn build_project(project: &Project) -> (BuildResult, ScModel) {
    let mut result = BuildResult::default();
    let mut checked: HashMap<String, CheckedContext> = HashMap::new();

    // Duplicate component names are a project-integrity failure (EB019):
    // no output the build could emit would be well-defined. Like a
    // dependency cycle: report and stop.
    result.diagnostics = duplicate_component_diagnostics(project);
    if !result.diagnostics.is_empty() {
        return (result, ScModel::default());
    }

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
                // Project-level cycle: no single source element to anchor on.
                span: None,
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
                    span: ctx.name_span,
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
                // Project-level cycle: no single source element to anchor on.
                span: None,
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
                    span: m.name_span,
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
