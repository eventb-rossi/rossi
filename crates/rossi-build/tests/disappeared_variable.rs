//! EB025: a refined machine that *drops* an abstract variable (data-refines it
//! away by not redeclaring it) must not reference that variable in any event.
//! The build/SC pipeline rejects an assignment to it, a read of it in an
//! action's RHS, and a read of it in a (non-theorem) guard — each an error that
//! drops the clause and marks the event `accurate=false` (Group R). A *theorem*
//! guard may read a variable disappearing at this level: the guard is kept,
//! the event stays accurate, and only a softer warning is reported. A variable
//! that vanished in an earlier refinement is unreadable even by a theorem.
//! Gluing invariants legitimately reference disappeared variables and are never
//! flagged.

use rossi_build::{Project, ProjectComponent, RuleId, Severity, build, sc_view::ScView};

/// The refinement `M2` of [`build_refinement`], with the caller's events.
fn refinement_source(m2_events: &str) -> String {
    // M2 refines M1 but keeps only `w`, so `v` has disappeared.
    format!(
        "MACHINE M2\n\
        REFINES M1\n\
        VARIABLES\n    w\n\
        INVARIANTS\n    @i1 w >= 0\n\
        EVENTS\n\
        EVENT INITIALISATION\n    THEN\n        @a2 w := 0\n    END\n\n\
        {m2_events}\
        END\n"
    )
}

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
    let m2 = refinement_source(m2_events);
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
fn referencing_disappeared_variable_is_an_error() {
    // EB025 fires wherever the dropped `v` is referenced: assigned by an
    // action (LHS), read in an action's RHS, or read in a (non-theorem)
    // guard — each a single error diagnostic on the offending clause.
    let cases: [(&str, &str, &str, &[&str]); 3] = [
        (
            "assigned in action LHS",
            "EVENT bump\n    THEN\n        @a1 v := w + 1\n    END\n\n",
            "M2.bump.a1",
            &["disappeared", "'v'"],
        ),
        (
            "read in action RHS",
            "EVENT peek\n    THEN\n        @a2 w := v + 1\n    END\n\n",
            "M2.peek.a2",
            &["references"],
        ),
        (
            "read in guard",
            "EVENT chk\n    WHERE\n        @g1 v > 0\n    THEN\n        @a2 w := w + 1\n    END\n\n",
            "M2.chk.g1",
            &[],
        ),
    ];
    for (case, events, origin, fragments) in cases {
        let r = build_refinement(events);
        let found = disappeared(&r);
        assert_eq!(found.len(), 1, "{case}: {:#?}", r.diagnostics);
        assert_eq!(found[0].severity, Severity::Error, "{case}");
        assert_eq!(found[0].origin, origin, "{case}");
        for fragment in fragments {
            assert!(
                found[0].message.contains(fragment),
                "{case}: expected {fragment:?} in message: {}",
                found[0].message
            );
        }
    }
}

#[test]
fn disappeared_variable_is_anchored_at_its_use() {
    // Each finding underlines the variable itself, so the editor marks the
    // name to keep or replace rather than the whole clause.
    for events in [
        "EVENT bump\n    THEN\n        @a1 v := w + 1\n    END\n\n",
        "EVENT peek\n    THEN\n        @a2 w := w + v\n    END\n\n",
        "EVENT chk\n    WHERE\n        @g1 w > v\n    THEN\n        @a2 w := w + 1\n    END\n\n",
        "EVENT thm\n    WHERE\n        theorem @g1 w > v\n    THEN\n        @a2 w := w + 1\n    END\n\n",
    ] {
        let r = build_refinement(events);
        let source = refinement_source(events);
        let finding = r
            .diagnostics
            .iter()
            .find(|d| d.message.contains("'v'"))
            .unwrap_or_else(|| panic!("`v` is reported: {:#?}", r.diagnostics));
        let span = finding.span.expect("a text clause carries a span");
        assert_eq!(&source[span.start..span.end], "v", "{events}");
    }
}

#[test]
fn theorem_guard_reading_disappeared_variable_is_kept() {
    // A *theorem* guard may read a variable disappearing at this level: the
    // guard is kept (emitted with theorem="true"), the event stays accurate,
    // and only the softer EB018 warning is reported, not the EB025 error.
    let r = build_refinement(
        "EVENT thm\n    WHERE\n        theorem @g1 v > 0\n    THEN\n        @a2 w := w + 1\n    END\n\n",
    );
    assert!(
        disappeared(&r).is_empty(),
        "a theorem guard read must not be EB025: {:#?}",
        r.diagnostics
    );
    let warning = r
        .diagnostics
        .iter()
        .find(|d| {
            d.rule_id == Some(RuleId::UndeclaredIdentifier) && d.severity == Severity::Warning
        })
        .expect("the theorem-guard abstract-only warning");
    assert!(
        !warning.message.contains("dropped"),
        "the kept guard's warning must not claim a drop: {}",
        warning.message
    );
    let v = ScView::from_xml(&r.file("M2.bcm").expect("M2.bcm").contents).unwrap();
    let thm = v.events.get("thm").expect("thm event present");
    assert!(thm.accurate, "the event keeps its accuracy");
    let guard = thm
        .guards
        .values()
        .find(|g| g.label == "g1")
        .expect("guard kept");
    assert!(guard.theorem, "kept guard stays a theorem");
}

#[test]
fn theorem_guard_reading_earlier_vanished_variable_is_an_error() {
    // `v` disappears at M2; a theorem guard in M3 may no longer read it —
    // the exemption covers only variables disappearing at this level.
    let m1 = "MACHINE M1\n\
        VARIABLES\n    v\n    w\n\
        INVARIANTS\n    @i1 v >= 0\n    @i2 w >= 0\n\
        EVENTS\n\
        EVENT INITIALISATION\n    THEN\n        @a1 v := 0\n        @a2 w := 0\n    END\n\n\
        EVENT tick\n    THEN\n        @a1 v := v + 1\n    END\n\
        END\n";
    let m2 = "MACHINE M2\n\
        REFINES M1\n\
        VARIABLES\n    w\n\
        INVARIANTS\n    @i1 w >= 0\n\
        EVENTS\n\
        EVENT INITIALISATION\n    THEN\n        @a2 w := 0\n    END\n\
        END\n";
    let m3 = "MACHINE M3\n\
        REFINES M2\n\
        VARIABLES\n    w\n\
        EVENTS\n\
        EVENT INITIALISATION\n    THEN\n        @a2 w := 0\n    END\n\n\
        EVENT thm\n    WHERE\n        theorem @g1 v > 0\n    THEN\n        @a2 w := w + 1\n    END\n\
        END\n";
    let mut components = ProjectComponent::from_eventb("M1.eventb", m1).unwrap();
    components.extend(ProjectComponent::from_eventb("M2.eventb", m2).unwrap());
    components.extend(ProjectComponent::from_eventb("M3.eventb", m3).unwrap());
    let r = build(&Project::new("chain", components));
    let found = disappeared(&r);
    assert_eq!(found.len(), 1, "{:#?}", r.diagnostics);
    assert_eq!(found[0].severity, Severity::Error);
    assert_eq!(found[0].origin, "M3.thm.g1");
    let v = ScView::from_xml(&r.file("M3.bcm").expect("M3.bcm").contents).unwrap();
    let thm = v.events.get("thm").expect("thm event present");
    assert!(!thm.accurate, "the event loses its accuracy");
    assert!(
        thm.guards.is_empty(),
        "the guard is dropped: {:#?}",
        thm.guards
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
