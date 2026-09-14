//! Running the model checker over the loaded project: choosing the
//! machine, writing the throwaway project (bounded when asked), running
//! the tool, and mapping what it printed back to the model.

use std::collections::HashSet;
use std::path::Path;

use eventb_animate_driver::report::{self, Report};
use eventb_animate_driver::{
    AnimateConfig, ProbSettings, Run, ToolError, concrete_path, effective_tool, run_args, run_tool,
    run_watchdog,
};
use rossi::ast::LabeledPredicate;
use rossi::{Component, Context, NamedComponent};
use rossi_build::{Diagnostic, Project, ProjectComponent, ScFile};

use crate::report::ViolatedInvariant;
use crate::workspace::Loaded;

/// The machine to run: the named one, or the most refined one when the
/// project has exactly one machine nothing refines.
pub fn select_machine(loaded: &Loaded, wanted: Option<&str>) -> Result<String, String> {
    let components = loaded
        .checks()
        .map(|checked| checked.project.components.as_slice())
        .unwrap_or_default();
    let machines: Vec<&str> = components
        .iter()
        .filter_map(|pc| match &pc.component {
            Component::Machine(machine) => Some(machine.name.as_str()),
            Component::Context(_) => None,
        })
        .collect();
    if let Some(name) = wanted {
        return if machines.contains(&name) {
            Ok(name.to_string())
        } else {
            Err(format!(
                "no machine named `{name}`; the machines are {machines:?}"
            ))
        };
    }
    let refined: HashSet<&str> = components
        .iter()
        .filter_map(|pc| match &pc.component {
            Component::Machine(machine) => machine.refines.as_deref(),
            Component::Context(_) => None,
        })
        .collect();
    let leaves: Vec<&str> = machines
        .iter()
        .copied()
        .filter(|machine| !refined.contains(machine))
        .collect();
    match leaves.as_slice() {
        [one] => Ok((*one).to_string()),
        [] => Err("the project has no machine to check".to_string()),
        many => Err(format!(
            "several machines are most refined ({many:?}); name one with `machine`"
        )),
    }
}

/// The components to check. With bounds, the machine's contexts are
/// replaced by one that extends them all and adds one axiom per bound,
/// so the bounds constrain the constants without touching the model.
pub fn bounded_components(
    project: &Project,
    machine: &str,
    bounds: &[String],
) -> Result<Vec<NamedComponent>, String> {
    let mut components: Vec<NamedComponent> = project
        .components
        .iter()
        .map(|pc| NamedComponent {
            filename: rossi::component_filename(&pc.component),
            component: pc.component.clone(),
        })
        .collect();
    if bounds.is_empty() {
        return Ok(components);
    }
    let context_name = format!("{machine}_bounds");
    if components
        .iter()
        .any(|named| named.component.name() == context_name)
    {
        return Err(format!(
            "the project already has a component named `{context_name}`; \
             rename it to run this machine with bounds"
        ));
    }
    let Some(Component::Machine(target)) = components
        .iter_mut()
        .map(|named| &mut named.component)
        .find(|component| matches!(component, Component::Machine(m) if m.name == machine))
    else {
        return Err(format!("no machine named `{machine}`"));
    };
    let mut context = Context::new(context_name.clone());
    context.extends = std::mem::replace(&mut target.sees, vec![context_name]);
    for (index, bound) in bounds.iter().enumerate() {
        let predicate = rossi::parse_predicate_str(bound)
            .map_err(|error| format!("bound {} (`{bound}`) does not parse: {error}", index + 1))?;
        context.axioms.push(LabeledPredicate {
            label: Some(format!("bnd{}", index + 1)),
            is_theorem: false,
            predicate,
            span: None,
            comment: None,
        });
    }
    let context = Component::Context(context);
    components.push(NamedComponent {
        filename: rossi::component_filename(&context),
        component: context,
    });
    Ok(components)
}

/// The files of a build of `components`, or the diagnostics that stop
/// it: a bound that names an unknown constant or is ill-typed is an
/// error of the run, not of the model.
pub fn build_files(
    name: &str,
    components: &[NamedComponent],
) -> Result<Vec<ScFile>, Vec<Diagnostic>> {
    let project = Project::new(
        name,
        components
            .iter()
            .map(|named| {
                ProjectComponent::from_parsed(named.filename.clone(), named.component.clone(), None)
            })
            .collect(),
    );
    let build = rossi_build::build(&project);
    if build.is_ok() {
        Ok(build.files)
    } else {
        Err(build.diagnostics)
    }
}

/// A complete Rodin project (sources, checked and proof files) in a fresh
/// temporary directory, removed when the guard drops.
pub fn write_project(
    name: &str,
    components: &[NamedComponent],
    files: &[ScFile],
) -> Result<tempfile::TempDir, String> {
    let dir = tempfile::Builder::new()
        .prefix("rossi-mcp-")
        .tempdir()
        .map_err(|e| format!("cannot create a temporary project: {e}"))?;
    rossi::write_project_directory(dir.path(), components, name)
        .map_err(|e| format!("cannot write the temporary project: {e}"))?;
    rossi_build::write_sc_files(dir.path(), files)
        .map_err(|e| format!("cannot write into the temporary project: {e}"))?;
    Ok(dir)
}

/// Run the tool on the project under `dir` and read its report.
pub async fn run(
    config: &AnimateConfig,
    run: &Run,
    settings: &ProbSettings,
    machine: &str,
    dir: &Path,
    po_count: usize,
) -> Result<(Report, Option<i32>), ToolError> {
    let program = effective_tool(&config.path);
    if let Some(concrete) = concrete_path(&program)
        && !concrete.exists()
    {
        return Err(ToolError::Missing(program));
    }
    let args = run_args(run, settings, machine, dir);
    let output = run_tool(&program, &args, run_watchdog(run, po_count)).await?;
    let report = report::parse(&output.stdout, &output.stderr)?;
    Ok((report, output.code))
}

/// The declarations the tool's printed violated predicates name, one
/// row per printed string; the matching rule is the driver's, and a
/// string nothing matched keeps the printed form so a violation is
/// never dropped.
pub fn violated_invariants(
    violated: &[String],
    loaded: &Loaded,
    machine: &str,
) -> Vec<ViolatedInvariant> {
    let invariants = loaded
        .checked
        .as_ref()
        .map(|checked| checked.invariants.as_slice())
        .unwrap_or_default();
    report::match_violated(violated, invariants, machine)
        .into_iter()
        .zip(violated)
        .map(|(hits, printed)| {
            let hit = hits.first();
            ViolatedInvariant {
                text: printed.clone(),
                component: hit.map(|info| info.component.clone()),
                label: hit.map(|info| info.label.clone()),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> Project {
        let ctx = "CONTEXT c\nCONSTANTS n\nAXIOMS\n    @axm1 n ∈ ℕ\nEND\n";
        let mach = "MACHINE m\nSEES c\nVARIABLES x\nINVARIANTS\n    @inv1 x ∈ ℕ\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x ≔ 0\n    END\nEND\n";
        let mut components = Vec::new();
        for text in [ctx, mach] {
            for component in rossi::parse_components(text).unwrap() {
                components.push(ProjectComponent::from_parsed(
                    rossi::component_filename(&component),
                    component,
                    Some(text.to_string()),
                ));
            }
        }
        Project::new("p", components)
    }

    #[test]
    fn bounds_become_a_context_the_machine_sees_instead() {
        let bounded = bounded_components(&project(), "m", &["n < 5".to_string()]).unwrap();
        assert_eq!(bounded.len(), 3);
        let Component::Context(context) = &bounded[2].component else {
            panic!("the bounds context comes last");
        };
        assert_eq!(context.name, "m_bounds");
        assert_eq!(context.extends, ["c"]);
        assert_eq!(context.axioms.len(), 1);
        assert_eq!(context.axioms[0].label.as_deref(), Some("bnd1"));
        let Component::Machine(machine) = &bounded[1].component else {
            panic!("the machine keeps its place");
        };
        assert_eq!(machine.sees, ["m_bounds"]);
        assert!(build_files("p", &bounded).is_ok());
    }

    #[test]
    fn a_bounds_context_that_would_collide_is_refused() {
        let ctx = "CONTEXT m_bounds\nEND\n";
        let mut project = project();
        for component in rossi::parse_components(ctx).unwrap() {
            project.components.push(ProjectComponent::from_parsed(
                rossi::component_filename(&component),
                component,
                Some(ctx.to_string()),
            ));
        }
        let error = bounded_components(&project, "m", &["n < 5".to_string()]).unwrap_err();
        assert!(
            error.contains("already has a component named `m_bounds`"),
            "{error}"
        );
    }

    #[test]
    fn no_bounds_leave_the_components_alone() {
        let same = bounded_components(&project(), "m", &[]).unwrap();
        assert_eq!(same.len(), 2);
    }

    #[test]
    fn a_bound_that_does_not_parse_or_check_is_reported() {
        let error = bounded_components(&project(), "m", &["n <".to_string()]).unwrap_err();
        assert!(error.contains("bound 1"), "{error}");
        let bounded = bounded_components(&project(), "m", &["zz < 5".to_string()]).unwrap();
        let diagnostics = build_files("p", &bounded).unwrap_err();
        assert!(!diagnostics.is_empty());
    }
}
