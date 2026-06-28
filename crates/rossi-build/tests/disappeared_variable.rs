//! EB025: a refined machine that *drops* an abstract variable (data-refines it
//! away by not redeclaring it) must not assign that variable in any event. The
//! build/SC pipeline rejects the assignment as an error and drops the action,
//! marking the event `accurate=false` (Group R). A mere *read* of the vanished
//! variable in an action's RHS stays a warning — it is a dangling reference,
//! not an illegal write.

use rossi_build::{Project, ProjectComponent, RuleId, Severity, build};

/// Build a project from two `.eventb` machines: an abstract `M1` and a
/// refinement `M2` whose body is supplied by the caller.
fn build_refinement(m2_events: &str) -> rossi_build::BuildResult {
    let m1 = "MACHINE M1\n\
        VARIABLES\n    v\n    w\n\
        INVARIANTS\n    @i1 v >= 0\n    @i2 w >= 0\n\
        EVENTS\n\
        EVENT INITIALISATION\n    THEN\n        @a1 v := 0\n        @a2 w := 0\n    END\n\n\
        EVENT tick\n    THEN\n        @a1 v := v + 1\n    END\n\
        END\n";
    // M2 refines M1 but keeps only `w`, so `v` has disappeared.
    let m2 = format!(
        "MACHINE M2\n\
        REFINES M1\n\
        VARIABLES\n    w\n\
        INVARIANTS\n    @i1 w >= 0\n\
        EVENTS\n\
        EVENT INITIALISATION\n    THEN\n        @a2 w := 0\n    END\n\n\
        {m2_events}\
        END\n"
    );
    let mut components = ProjectComponent::from_eventb("M1.eventb", m1).unwrap();
    components.extend(ProjectComponent::from_eventb("M2.eventb", &m2).unwrap());
    build(&Project::new("disv", components))
}

fn disappeared(r: &rossi_build::BuildResult) -> Vec<&rossi_build::Diagnostic> {
    r.diagnostics
        .iter()
        .filter(|d| d.rule_id == Some(RuleId::DisappearedVariable))
        .collect()
}

#[test]
fn assigning_disappeared_variable_is_an_error() {
    // `bump` assigns `v`, which M2 dropped — an EB025 error on the action.
    let r = build_refinement("EVENT bump\n    THEN\n        @a1 v := w + 1\n    END\n\n");
    let found = disappeared(&r);
    assert_eq!(found.len(), 1, "{:#?}", r.diagnostics);
    assert_eq!(found[0].severity, Severity::Error);
    assert_eq!(found[0].origin, "M2.bump.a1");
    assert!(
        found[0].message.contains("disappeared") && found[0].message.contains("'v'"),
        "{}",
        found[0].message
    );
}

#[test]
fn reading_disappeared_variable_in_rhs_stays_a_warning() {
    // `peek` only reads `v` in its RHS — a dangling reference, dropped as the
    // existing EB018 warning, NOT the EB025 error.
    let r = build_refinement("EVENT peek\n    THEN\n        @a2 w := v + 1\n    END\n\n");
    assert!(
        disappeared(&r).is_empty(),
        "a read is not EB025: {:#?}",
        r.diagnostics
    );
    assert!(
        r.diagnostics.iter().any(|d| {
            d.rule_id == Some(RuleId::UndeclaredIdentifier) && d.severity == Severity::Warning
        }),
        "expected the abstract-only read warning: {:#?}",
        r.diagnostics
    );
}

#[test]
fn dropped_variable_left_unassigned_is_clean() {
    // M2 drops `v` and never assigns it — a legitimate data refinement as far
    // as this check is concerned. No EB025.
    let r = build_refinement("EVENT step\n    THEN\n        @a2 w := w + 1\n    END\n\n");
    assert!(disappeared(&r).is_empty(), "{:#?}", r.diagnostics);
}

#[test]
fn redeclared_variable_assignment_is_not_disappeared() {
    // When M2 keeps `v` in its own VARIABLES, the variable has NOT disappeared,
    // so assigning it is build-clean (the skip-refinement concern is EB024's
    // job in `validate`, not a build error). No EB025.
    let m1 = "MACHINE M1\n\
        VARIABLES\n    v\n\
        INVARIANTS\n    @i1 v >= 0\n\
        EVENTS\n\
        EVENT INITIALISATION\n    THEN\n        @a1 v := 0\n    END\n\n\
        EVENT tick\n    THEN\n        @a1 v := v + 1\n    END\n\
        END\n";
    let m2 = "MACHINE M2\n\
        REFINES M1\n\
        VARIABLES\n    v\n\
        INVARIANTS\n    @i1 v >= 0\n\
        EVENTS\n\
        EVENT INITIALISATION\n    THEN\n        @a1 v := 0\n    END\n\n\
        EVENT bump\n    THEN\n        @a1 v := v + 1\n    END\n\
        END\n";
    let mut components = ProjectComponent::from_eventb("M1.eventb", m1).unwrap();
    components.extend(ProjectComponent::from_eventb("M2.eventb", m2).unwrap());
    let r = build(&Project::new("kept", components));
    assert!(disappeared(&r).is_empty(), "{:#?}", r.diagnostics);
}
