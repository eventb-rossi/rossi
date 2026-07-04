//! EB019: `build` fails a project whose component names collide.
//!
//! A duplicate name is a project-integrity failure — every reference to the
//! name is ambiguous and the emitted `.bcc`/`.bcm` identities collide — and
//! Rodin cannot even represent the state (a component's name is its file
//! identity; the per-name proof files are shared across kinds). Like a
//! dependency cycle, the build reports it and emits nothing.

use rossi_build::project::{Project, ProjectComponent};
use rossi_build::{RuleId, build};

fn project(files: &[(&str, &str)]) -> Project {
    let components = files
        .iter()
        .flat_map(|(name, text)| ProjectComponent::from_eventb(*name, text).unwrap())
        .collect();
    Project::new("test", components)
}

fn eb019_messages(result: &rossi_build::BuildResult) -> Vec<&str> {
    result
        .diagnostics
        .iter()
        .filter(|d| d.rule_id == Some(RuleId::DuplicateComponent))
        .map(|d| d.message.as_str())
        .collect()
}

#[test]
fn duplicate_machines_fail_the_build() {
    let result = build(&project(&[
        ("a.eventb", "MACHINE M\nEND\n"),
        ("b.eventb", "MACHINE M\nEND\n"),
    ]));
    let messages = eb019_messages(&result);
    assert_eq!(messages.len(), 1, "{result:#?}");
    assert!(
        messages[0].contains("a.eventb") && messages[0].contains("b.eventb"),
        "{}",
        messages[0]
    );
    assert!(!result.is_ok());
    assert!(
        result.files.is_empty(),
        "nothing may be emitted: {result:#?}"
    );
}

#[test]
fn cross_kind_name_collision_fails_the_build() {
    // A machine and a context may not share a name either: in Rodin the
    // per-name proof files (`N.bpo`/`N.bps`/`N.bpr`) are shared across
    // kinds, so these outputs could never be imported side by side.
    let result = build(&project(&[
        ("machine_n.eventb", "MACHINE N\nEND\n"),
        ("context_n.eventb", "CONTEXT N\nEND\n"),
    ]));
    assert_eq!(eb019_messages(&result).len(), 1, "{result:#?}");
    assert!(!result.is_ok());
    assert!(result.files.is_empty(), "{result:#?}");
}

#[test]
fn unique_names_do_not_trip_eb019() {
    let result = build(&project(&[
        ("m.eventb", "MACHINE M\nEND\n"),
        ("c.eventb", "CONTEXT C\nEND\n"),
    ]));
    assert!(eb019_messages(&result).is_empty(), "{result:#?}");
    assert_eq!(result.files.len(), 2, "{result:#?}");
}
