//! Basic (non-refinement) machine SC emission, one section module per
//! fixture:
//!
//! - `invariants_variables` (M1): SEES + variables + typing invariants.
//! - `events` (M2): events without refinement.
//! - `sees_diamond` (M3): transitively-seen context diamond.
//! - `variant` (B1): scVariant emission.
//! - `variable_typing`: buried-identifier inference + untyped-variable
//!   accuracy regressions.
//! - `setcomp_lowering` (RC2): short-form set comprehension promoted to
//!   the long form in emitted actions.
//! - `primed_declarations`: a variable or event parameter carrying the
//!   after-state prime is refused and dropped.

mod invariants_variables {
    //! M1: smallest useful machine — SEES a context, declares variables, and
    //! invariants that type them. No REFINES. No events.
    //!
    //! Asserts via `ScView` (semantic diff oracle) that:
    //! - scMachineFile wraps the right elements
    //! - scSeesContext points at the seen context's .bcc
    //! - scInternalContext inlines the seen context's body
    //! - scVariable rows have their inferred types
    //! - scInvariant predicates round-trip to the original ASTs

    use rossi_build::{Project, ProjectComponent, build, sc_view::ScView};

    const CTX_BUC: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.carrierSet name="_set1" org.eventb.core.identifier="USERS"/>
<org.eventb.core.carrierSet name="_set2" org.eventb.core.identifier="ITEMS"/>
</org.eventb.core.contextFile>
"#;

    const MACHINE_BUM: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_sees1" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v1" org.eventb.core.identifier="registered"/>
<org.eventb.core.variable name="_v2" org.eventb.core.identifier="inventory"/>
<org.eventb.core.invariant name="_inv1" org.eventb.core.label="inv1" org.eventb.core.predicate="registered ⊆ USERS"/>
<org.eventb.core.invariant name="_inv2" org.eventb.core.label="inv2" org.eventb.core.predicate="inventory ⊆ ITEMS"/>
</org.eventb.core.machineFile>
"#;

    fn make_project() -> Project {
        Project::new(
            "m1",
            vec![
                ProjectComponent::from_xml("Ctx.buc", CTX_BUC).unwrap(),
                ProjectComponent::from_xml("Mch.bum", MACHINE_BUM).unwrap(),
            ],
        )
    }

    #[test]
    fn machine_root_is_accurate() {
        let r = build(&make_project());
        // The seen context's .bcc and the .bcm, plus the generated
        // proof files.
        assert_eq!(r.files.len(), 6);
        let names: Vec<_> = r.files.iter().map(|f| f.filename.as_str()).collect();
        assert!(names.contains(&"Ctx.bcc"), "expected Ctx.bcc in {names:?}");
        let bcm = r.file("Mch.bcm").expect("Mch.bcm");
        assert!(bcm.accurate, "diagnostics: {:?}", r.diagnostics);
        let view = ScView::from_xml(&bcm.contents).unwrap();
        assert_eq!(view.kind, rossi_build::sc_view::RootKind::Machine);
        assert!(view.accurate);
    }

    #[test]
    fn variables_get_powerset_types() {
        let r = build(&make_project());
        let bcm = r.file("Mch.bcm").expect("Mch.bcm");
        let view = ScView::from_xml(&bcm.contents).unwrap();
        assert_eq!(
            view.variables
                .get("registered")
                .map(|v| v.type_str.as_str()),
            Some("ℙ(USERS)")
        );
        assert_eq!(
            view.variables.get("inventory").map(|v| v.type_str.as_str()),
            Some("ℙ(ITEMS)")
        );
    }

    #[test]
    fn invariants_preserve_predicate_semantics() {
        use rossi::parse_predicate_str;
        let r = build(&make_project());
        let bcm = r.file("Mch.bcm").expect("Mch.bcm");
        let view = ScView::from_xml(&bcm.contents).unwrap();
        // Invariants keyed by source URI now; look them up by label.
        let inv1 = view
            .invariants
            .values()
            .find(|i| i.label == "inv1")
            .expect("inv1");
        let inv2 = view
            .invariants
            .values()
            .find(|i| i.label == "inv2")
            .expect("inv2");
        assert!(!inv1.theorem);
        assert_eq!(
            inv1.predicate,
            parse_predicate_str("registered ⊆ USERS").unwrap()
        );
        assert_eq!(
            inv2.predicate,
            parse_predicate_str("inventory ⊆ ITEMS").unwrap()
        );
    }

    #[test]
    fn sees_produces_sc_internal_context() {
        let r = build(&make_project());
        let bcm = r.file("Mch.bcm").expect("Mch.bcm");
        assert!(
            bcm.contents.contains("<org.eventb.core.scSeesContext"),
            "expected scSeesContext in machine .bcm:\n{}",
            bcm.contents
        );
        assert!(
            bcm.contents
                .contains("<org.eventb.core.scInternalContext name=\"Ctx\""),
            "expected scInternalContext for seen context:\n{}",
            bcm.contents
        );
        // The seen context's carrier sets should be inlined inside it.
        assert!(bcm.contents.contains("name=\"USERS\""));
        assert!(bcm.contents.contains("name=\"ITEMS\""));
    }
}

mod events {
    //! M2: events without refinement.
    //!
    //! Exercises: INITIALISATION, events with parameters (ANY) / guards (WHERE) /
    //! actions (THEN), parameter type inference from guards, convergence
    //! encoding, and empty-set type ascription on assignment RHS.

    use rossi_build::{Project, ProjectComponent, build, sc_view::ScView};

    const CTX_BUC: &str = r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.carrierSet name="_set1" org.eventb.core.identifier="USERS"/>
</org.eventb.core.contextFile>
"#;

    /// Machine with:
    ///   INITIALISATION: `registered := ∅`
    ///   Register(u): guard `u ∈ USERS`, action `registered := registered ∪ {u}`
    ///   Leave(u): guard `u ∈ registered`, action `registered := registered ∖ {u}`
    const MACHINE_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_s1" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v1" org.eventb.core.identifier="registered"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="registered ⊆ USERS"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="registered ≔ ∅" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_ev_reg" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="Register">
<org.eventb.core.parameter name="_p1" org.eventb.core.identifier="u"/>
<org.eventb.core.guard name="_g1" org.eventb.core.label="grd1" org.eventb.core.predicate="u ∈ USERS"/>
<org.eventb.core.action name="_a1" org.eventb.core.assignment="registered ≔ registered ∪ {u}" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_ev_leave" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="Leave">
<org.eventb.core.parameter name="_p2" org.eventb.core.identifier="u"/>
<org.eventb.core.guard name="_g2" org.eventb.core.label="grd1" org.eventb.core.predicate="u ∈ registered"/>
<org.eventb.core.action name="_a2" org.eventb.core.assignment="registered ≔ registered ∖ {u}" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;

    const PARAMETER_CONFLICT_MACHINE_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_v1" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="x ⊆ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="x ≔ ∅" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_ev" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="Clash">
<org.eventb.core.parameter name="_p1" org.eventb.core.identifier="x"/>
<org.eventb.core.guard name="_g1" org.eventb.core.label="grd1" org.eventb.core.predicate="x ∈ ℤ"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;

    const UNTYPED_PARAMETER_CONFLICT_MACHINE_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_v1" org.eventb.core.identifier="x"/>
<org.eventb.core.event name="_ev" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="Clash">
<org.eventb.core.parameter name="_p1" org.eventb.core.identifier="x"/>
<org.eventb.core.guard name="_g1" org.eventb.core.label="grd1" org.eventb.core.predicate="x ∈ ℤ"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;

    fn make_project() -> Project {
        Project::new(
            "m2",
            vec![
                ProjectComponent::from_xml("Ctx.buc", CTX_BUC).unwrap(),
                ProjectComponent::from_xml("Mch.bum", MACHINE_BUM).unwrap(),
            ],
        )
    }

    fn machine_view() -> ScView {
        let r = build(&make_project());
        assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);
        ScView::from_xml(&r.file("Mch.bcm").expect("Mch.bcm").contents).unwrap()
    }

    #[test]
    fn initialisation_action_gets_empty_set_type_ascription() {
        // `registered ≔ ∅` should canonicalize to `registered ≔ ∅ ⦂ ℙ(USERS)`
        // because `registered : ℙ(USERS)` is known.
        let r = build(&make_project());
        let bcm = &r.file("Mch.bcm").expect("Mch.bcm").contents;
        assert!(
            bcm.contains(r#"org.eventb.core.assignment="registered ≔ ∅ ⦂ ℙ(USERS)""#),
            "expected empty-set type ascription in INITIALISATION:\n{bcm}"
        );
    }

    #[test]
    fn parameter_type_is_inferred_from_guard() {
        // Register.u : USERS (from `u ∈ USERS`, where USERS : ℙ(USERS))
        let v = machine_view();
        let reg = v.events.get("Register").expect("Register");
        assert_eq!(reg.parameters.get("u").map(String::as_str), Some("USERS"));
    }

    #[test]
    fn parameter_typed_via_variable_reference() {
        // Leave.u : USERS (from `u ∈ registered`, where registered : ℙ(USERS))
        let v = machine_view();
        let leave = v.events.get("Leave").expect("Leave");
        assert_eq!(leave.parameters.get("u").map(String::as_str), Some("USERS"));
    }

    #[test]
    fn convergence_encoding() {
        let v = machine_view();
        assert_eq!(
            v.events
                .get("Register")
                .and_then(|e| e.convergence.as_deref()),
            Some("0")
        );
    }

    #[test]
    fn event_guards_and_actions_captured_by_sc_view() {
        let v = machine_view();
        let reg = v.events.get("Register").expect("Register");
        assert_eq!(reg.guards.len(), 1, "Register has one guard");
        assert_eq!(reg.actions.len(), 1, "Register has one action");
        // Guard predicate is `u∈USERS` (canonical) — ScView parses it back.
        // Guards are keyed by source URI now (labels can collide across REFINES).
        let grd1 = reg
            .guards
            .values()
            .find(|g| g.label == "grd1")
            .expect("grd1 by label");
        assert!(!grd1.theorem);
        assert_eq!(
            grd1.predicate,
            rossi::parse_predicate_str("u ∈ USERS").unwrap()
        );
    }

    #[test]
    fn machine_file_is_accurate_when_all_events_type_check() {
        let r = build(&make_project());
        let bcm = r.file("Mch.bcm").expect("Mch.bcm");
        assert!(bcm.accurate, "diagnostics: {:?}", r.diagnostics);
    }

    /// Shared core of the parameter-conflict regressions: build a one-machine
    /// project from `machine_xml`, assert the conflict diagnostic at
    /// `Mch.Clash.x`, and assert the parameter was dropped from the checked
    /// event. Returns the build result and parsed view for case-specific
    /// follow-up assertions.
    fn assert_param_conflict(machine_xml: &str, case: &str) -> (rossi_build::BuildResult, ScView) {
        let project = Project::new(
            "conflict",
            vec![ProjectComponent::from_xml("Mch.bum", machine_xml).unwrap()],
        );
        let result = build(&project);
        assert!(
            result.diagnostics.iter().any(|diagnostic| {
                diagnostic.origin == "Mch.Clash.x"
                    && diagnostic
                        .message
                        .contains("parameter `x` conflicts with a visible identifier")
            }),
            "{case}: expected parameter conflict diagnostic: {:?}",
            result.diagnostics
        );
        let bcm = result.file("Mch.bcm").expect("Mch.bcm");
        let view = ScView::from_xml(&bcm.contents).unwrap();
        let event = view.events.get("Clash").expect("Clash event");
        assert!(
            !event.parameters.contains_key("x"),
            "{case}: parameter `x` must be dropped"
        );
        (result, view)
    }

    #[test]
    fn parameter_conflicting_with_machine_variable_is_diagnosed_and_dropped() {
        for (case, machine_xml, typed) in [
            ("typed variable", PARAMETER_CONFLICT_MACHINE_BUM, true),
            (
                "untyped variable",
                UNTYPED_PARAMETER_CONFLICT_MACHINE_BUM,
                false,
            ),
        ] {
            let (result, view) = assert_param_conflict(machine_xml, case);
            if typed {
                // With `x ⊆ ℤ` typing the machine variable, the clash is
                // additionally a TypeError, the conflicting guard is dropped,
                // and the event is marked inaccurate.
                assert!(
                    result.diagnostics.iter().any(|diagnostic| {
                        diagnostic.origin == "Mch.Clash.x"
                            && diagnostic.rule_id == Some(rossi_build::RuleId::TypeError)
                    }),
                    "{case}: expected TypeError diagnostic: {:?}",
                    result.diagnostics
                );
                let event = view.events.get("Clash").expect("Clash event");
                assert!(event.guards.is_empty(), "conflicting guard must be dropped");
                assert!(
                    !event.accurate,
                    "the guard must be checked against the machine variable and dropped"
                );
            }
        }
    }
}

mod sees_diamond {
    //! M3: a machine that SEES a context whose ancestors form a diamond must
    //! emit scInternalContext for every transitively-seen context, each
    //! appearing exactly once.
    //!
    //! Layout:
    //!   Base  (sets USERS)
    //!   Left  extends Base  (sets LEFT_ONLY)
    //!   Right extends Base  (sets RIGHT_ONLY)
    //!   Top   extends Left, Right
    //!   Mch   sees Top
    //!
    //! Expected: Mch.bcm has scInternalContext for {Base, Left, Right, Top},
    //! and Base appears exactly once (not once per path).

    use rossi_build::{Project, ProjectComponent, build, sc_view::ScView};

    fn ctx(filename: &str, body: &str) -> ProjectComponent {
        let xml = format!(
            r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
{body}
</org.eventb.core.contextFile>"#
        );
        ProjectComponent::from_xml(filename, &xml).unwrap()
    }

    /// Build the diamond `Base ← {Left, Right} ← Top` with the given
    /// carrier-set names declared in Left/Right, plus the given machine.
    fn diamond_project(
        name: &str,
        left_set: &str,
        right_set: &str,
        mch: ProjectComponent,
    ) -> Project {
        let base = ctx(
            "Base.buc",
            r#"<org.eventb.core.carrierSet name="_u" org.eventb.core.identifier="USERS"/>"#,
        );
        let left = ctx(
            "Left.buc",
            &format!(
                r#"<org.eventb.core.extendsContext name="_e1" org.eventb.core.target="Base"/>
<org.eventb.core.carrierSet name="_l" org.eventb.core.identifier="{left_set}"/>"#
            ),
        );
        let right = ctx(
            "Right.buc",
            &format!(
                r#"<org.eventb.core.extendsContext name="_e2" org.eventb.core.target="Base"/>
<org.eventb.core.carrierSet name="_r" org.eventb.core.identifier="{right_set}"/>"#
            ),
        );
        let top = ctx(
            "Top.buc",
            r#"<org.eventb.core.extendsContext name="_e3" org.eventb.core.target="Left"/>
<org.eventb.core.extendsContext name="_e4" org.eventb.core.target="Right"/>"#,
        );
        Project::new(name, vec![base, left, right, top, mch])
    }

    fn make_project() -> Project {
        let mch = ProjectComponent::from_xml(
            "Mch.bum",
            r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_s" org.eventb.core.target="Top"/>
</org.eventb.core.machineFile>"#,
        )
        .unwrap();
        diamond_project("m3", "LEFT_ONLY", "RIGHT_ONLY", mch)
    }

    #[test]
    fn machine_emits_internal_context_for_all_ancestors() {
        let r = build(&make_project());
        let bcm = &r.file("Mch.bcm").expect("Mch.bcm").contents;
        for name in ["Base", "Left", "Right", "Top"] {
            let marker = format!("<org.eventb.core.scInternalContext name=\"{name}\"");
            assert!(
                bcm.contains(&marker),
                "expected {marker} in machine .bcm:\n{bcm}"
            );
        }
    }

    #[test]
    fn sees_context_target_is_bare_uri() {
        // Rodin emits `scSeesContext.scTarget="/PROJECT/CTX.bcc"` without a
        // `|org.eventb.core.scContextFile#NAME` fragment; ProB rejects the
        // fragmented form. See docs/ANIMATE_FAILURES.md (RC1).
        let r = build(&make_project());
        let bcm = &r.file("Mch.bcm").expect("Mch.bcm").contents;
        assert!(
            bcm.contains("org.eventb.core.scTarget=\"/m3/Top.bcc\""),
            "expected bare scTarget for sees:\n{bcm}"
        );
        assert!(
            !bcm.contains("/m3/Top.bcc|org.eventb.core.scContextFile#"),
            "scSeesContext.scTarget must not carry a scContextFile fragment:\n{bcm}"
        );
    }

    #[test]
    fn diamond_base_appears_exactly_once() {
        let r = build(&make_project());
        let bcm = &r.file("Mch.bcm").expect("Mch.bcm").contents;
        let marker = "<org.eventb.core.scInternalContext name=\"Base\"";
        let count = bcm.matches(marker).count();
        assert_eq!(
            count, 1,
            "Base should appear exactly once, found {count} times in:\n{bcm}"
        );
    }

    #[test]
    fn variables_can_reference_carrier_sets_from_any_ancestor() {
        // With a diamond SEES, ancestor-defined sets must be visible for
        // inference on invariants / guards / variables in the machine.
        // We assert this indirectly: compile should succeed without
        // "unknown identifier" diagnostics.
        let mch = ProjectComponent::from_xml(
        "Mch.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_s" org.eventb.core.target="Top"/>
<org.eventb.core.variable name="_v1" org.eventb.core.identifier="registered"/>
<org.eventb.core.variable name="_v2" org.eventb.core.identifier="inventory"/>
<org.eventb.core.variable name="_v3" org.eventb.core.identifier="sales"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="registered ⊆ USERS"/>
<org.eventb.core.invariant name="_i2" org.eventb.core.label="inv2" org.eventb.core.predicate="inventory ⊆ ITEMS"/>
<org.eventb.core.invariant name="_i3" org.eventb.core.label="inv3" org.eventb.core.predicate="sales ⊆ AUCTIONS"/>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
        let project = diamond_project("m3_diamond", "ITEMS", "AUCTIONS", mch);
        let r = build(&project);
        assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);
        let view = ScView::from_xml(&r.file("Mch.bcm").unwrap().contents).unwrap();
        assert_eq!(
            view.variables
                .get("registered")
                .map(|v| v.type_str.as_str()),
            Some("ℙ(USERS)")
        );
        assert_eq!(
            view.variables.get("inventory").map(|v| v.type_str.as_str()),
            Some("ℙ(ITEMS)")
        );
        assert_eq!(
            view.variables.get("sales").map(|v| v.type_str.as_str()),
            Some("ℙ(AUCTIONS)")
        );
    }
}

mod variant {
    //! B1: emit `scVariant` element for a machine that declares a VARIANT.
    //!
    //! Rodin's shape (from binary-search/M2.bcm):
    //!
    //!   <org.eventb.core.scVariant name="C7" org.eventb.core.expression="j − i"
    //!       org.eventb.core.label="vrn"
    //!       org.eventb.core.source="/binary-search/M2.bum|...|variant#7"/>
    //!
    //! Emission order inside `scMachineFile`: scInvariants → scVariables →
    //! scVariant → scEvents (confirmed against M2.bcm).

    use rossi_build::{Project, ProjectComponent, build, sc_view::ScView};

    const CTX_BUC: &str = r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
</org.eventb.core.contextFile>
"#;

    /// Minimal machine with a VARIANT for a convergent event.
    /// - variable `n`
    /// - invariant `n ∈ ℕ`
    /// - variant `n` (must decrease on each convergent event)
    /// - event `decrement` convergent, guard `n > 0`, action `n := n − 1`
    const MACHINE_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_s" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v" org.eventb.core.identifier="n"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="n ∈ ℕ"/>
<org.eventb.core.variant name="_vr" org.eventb.core.expression="n"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="n ≔ 10" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_dec" org.eventb.core.convergence="1" org.eventb.core.extended="false" org.eventb.core.label="decrement">
<org.eventb.core.guard name="_g" org.eventb.core.label="grd1" org.eventb.core.predicate="n &gt; 0"/>
<org.eventb.core.action name="_a" org.eventb.core.assignment="n ≔ n − 1" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;

    fn project() -> Project {
        Project::new(
            "mb1",
            vec![
                ProjectComponent::from_xml("Ctx.buc", CTX_BUC).unwrap(),
                ProjectComponent::from_xml("Mch.bum", MACHINE_BUM).unwrap(),
            ],
        )
    }

    #[test]
    fn sc_variant_element_emitted() {
        let r = build(&project());
        assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);
        let bcm = &r.file("Mch.bcm").expect("Mch.bcm").contents;
        assert!(
            bcm.contains("<org.eventb.core.scVariant"),
            "expected scVariant in:\n{bcm}"
        );
        assert!(
            bcm.contains(r#"org.eventb.core.expression="n""#),
            "expected variant expression `n`:\n{bcm}"
        );
        assert!(
            bcm.contains(r#"org.eventb.core.label="vrn""#),
            "expected default label `vrn`:\n{bcm}"
        );
    }

    #[test]
    fn variant_appears_between_variables_and_events() {
        let r = build(&project());
        let bcm = &r.file("Mch.bcm").expect("Mch.bcm").contents;
        let idx_var = bcm.find("<org.eventb.core.scVariable").unwrap();
        let idx_variant = bcm.find("<org.eventb.core.scVariant").unwrap();
        let idx_event = bcm.find("<org.eventb.core.scEvent").unwrap();
        assert!(
            idx_var < idx_variant && idx_variant < idx_event,
            "expected scVariable → scVariant → scEvent order; got {idx_var}, {idx_variant}, {idx_event}"
        );
    }

    #[test]
    fn sc_view_captures_variant() {
        let r = build(&project());
        let bcm = &r.file("Mch.bcm").expect("Mch.bcm").contents;
        let view = ScView::from_xml(bcm).unwrap();
        assert_eq!(view.variants.get("vrn").map(String::as_str), Some("n"));
    }

    /// Two labeled variants, emitted in declaration order with their
    /// labels; a third one reusing a label is dropped (EB022 keeps the
    /// first occurrence).
    const MULTI_VARIANT_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_v" org.eventb.core.identifier="n"/>
<org.eventb.core.variable name="_w" org.eventb.core.identifier="k"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="n ∈ ℕ"/>
<org.eventb.core.invariant name="_j" org.eventb.core.label="inv2" org.eventb.core.predicate="k ∈ ℕ"/>
<org.eventb.core.variant name="_v1" org.eventb.core.expression="n" org.eventb.core.label="vrn1"/>
<org.eventb.core.variant name="_v2" org.eventb.core.expression="n − k" org.eventb.core.label="vrn2"/>
<org.eventb.core.variant name="_v3" org.eventb.core.expression="k" org.eventb.core.label="vrn1"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="n ≔ 10" org.eventb.core.label="act1"/>
<org.eventb.core.action name="_a1" org.eventb.core.assignment="k ≔ 0" org.eventb.core.label="act2"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;

    #[test]
    fn labeled_variants_emitted_in_order_and_duplicates_dropped() {
        let r = build(&Project::new(
            "mbn",
            vec![ProjectComponent::from_xml("Mch.bum", MULTI_VARIANT_BUM).unwrap()],
        ));
        let bcm = &r.file("Mch.bcm").expect("Mch.bcm").contents;
        let view = ScView::from_xml(bcm).unwrap();
        let variants: Vec<(&str, &str)> = view
            .variants
            .iter()
            .map(|(l, e)| (l.as_str(), e.as_str()))
            .collect();
        assert_eq!(variants, vec![("vrn1", "n"), ("vrn2", "n − k")]);
        let first = bcm.find(r#"org.eventb.core.label="vrn1""#).unwrap();
        let second = bcm.find(r#"org.eventb.core.label="vrn2""#).unwrap();
        assert!(first < second, "declaration order preserved");
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("variant label `vrn1`")),
            "EB022 for the duplicated label: {:?}",
            r.diagnostics
        );
    }
}

mod variable_typing {
    //! Variable-typing regressions, one fixture per scenario.

    use rossi_build::{Project, ProjectComponent, Severity, build, sc_view::ScView};

    /// A machine variable whose only typing invariant *buries* it inside an
    /// operand expression — here `w` in `f ∈ ℤ ⇸ ℤ ∖ {w}` — must still be
    /// typed. Rodin types every free identifier by giving it a fresh type
    /// variable (`getIdentType`) and solving the surrounding equations; the
    /// SETMINUS forces `{w} : ℙ(ℤ)`, hence `w : ℤ`. Regression guard for the
    /// "could not infer variable type" / "unknown identifier" cascade that
    /// otherwise drops the variable and every clause referencing it.
    const BURIED_IDENT_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_v1" org.eventb.core.identifier="w"/>
<org.eventb.core.variable name="_v2" org.eventb.core.identifier="f"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="f ∈ ℤ ⇸ ℤ ∖ {w}"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="w ≔ 0" org.eventb.core.label="act1"/>
<org.eventb.core.action name="_a2" org.eventb.core.assignment="f ≔ ∅" org.eventb.core.label="act2"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;

    fn buried_project() -> Project {
        Project::new(
            "p",
            vec![ProjectComponent::from_xml("M.bum", BURIED_IDENT_BUM).unwrap()],
        )
    }

    #[test]
    fn variable_buried_in_invariant_is_typed() {
        let r = build(&buried_project());

        // The buried identifier must not raise the "could not infer variable
        // type" warning (the bug signature).
        let untyped: Vec<_> = r
            .diagnostics
            .iter()
            .filter(|d| d.message.contains("could not infer variable type"))
            .collect();
        assert!(
            untyped.is_empty(),
            "no variable should be left untyped; diagnostics: {:?}",
            r.diagnostics
        );

        // ... and no "unknown identifier" cascade either.
        let errors: Vec<_> = r
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "no error diagnostics expected; diagnostics: {:?}",
            r.diagnostics
        );

        let bcm = r.file("M.bcm").expect("M.bcm");
        assert!(bcm.accurate, "file should be accurate: {:?}", r.diagnostics);

        let v = ScView::from_xml(&bcm.contents).unwrap();
        assert_eq!(
            v.variables.get("w").map(|row| row.type_str.as_str()),
            Some("ℤ"),
            "w should be typed ℤ via the buried `ℤ ∖ {{w}}`"
        );
        assert_eq!(
            v.variables.get("f").map(|row| row.type_str.as_str()),
            Some("ℙ(ℤ×ℤ)"),
        );
    }

    /// Group P: untyped variables alone are not a file-level inaccuracy
    /// signal. Rodin parity — verified against a real-world corpus
    /// machine whose variables are untyped (no invariants) but the file stays
    /// `accurate="true"` with only the writing event marked
    /// `accurate="false"`. A bystander event that doesn't touch the
    /// untyped variable stays `accurate="true"`.
    const UNTYPED_VAR_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_v1" org.eventb.core.identifier="x"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_ev1" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt1"/>
</org.eventb.core.machineFile>
"#;

    fn untyped_project() -> Project {
        Project::new(
            "p",
            vec![ProjectComponent::from_xml("M.bum", UNTYPED_VAR_BUM).unwrap()],
        )
    }

    #[test]
    fn file_stays_accurate_and_untyped_variable_emits_error() {
        // The cascade-drop emits an Error diagnostic on the dropped action,
        // so `r.is_ok()` is intentionally false here. The file itself must
        // still emit, and the file-level `accurate` flag must stay `true`.
        let r = build(&untyped_project());
        let bcm = r.file("M.bcm").expect("M.bcm");
        assert!(
            bcm.accurate,
            "file should stay accurate; diagnostics: {:?}",
            r.diagnostics
        );
        let v = ScView::from_xml(&bcm.contents).unwrap();
        let init = v
            .events
            .get("INITIALISATION")
            .expect("INITIALISATION present");
        assert!(
            !init.accurate,
            "INITIALISATION should be inaccurate (untyped LHS); diagnostics: {:?}",
            r.diagnostics
        );
        let evt1 = v.events.get("evt1").expect("evt1 present");
        assert!(
            evt1.accurate,
            "evt1 should stay accurate (doesn't touch x); diagnostics: {:?}",
            r.diagnostics
        );

        // Rodin's UntypedVariableError is an error marker (the variable is
        // dropped from the output); the file-accuracy behaviour above is
        // unaffected by the severity.
        let errors: Vec<_> = r
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .filter(|d| d.message.contains("could not infer variable type"))
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "expected exactly one untyped-variable error; diagnostics: {:?}",
            r.diagnostics
        );
        assert_eq!(errors[0].origin, "M.x");
    }
}

mod setcomp_lowering {
    //! RC2: a basic short-form set comprehension `{ x ∣ P }` on an action's
    //! RHS must be promoted to the long form `{ x⦂T · P ∣ x }` in the
    //! emitted .bcm. ProB's predicate parser only accepts the long form.

    use rossi_build::{Project, ProjectComponent, build};

    const CTX_BUC: &str = r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.carrierSet name="_set1" org.eventb.core.identifier="USERS"/>
</org.eventb.core.contextFile>
"#;

    const SETCOMP_MACHINE_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_s1" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v1" org.eventb.core.identifier="registered"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="registered ⊆ USERS"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="registered ≔ {y ∣ y ∈ USERS}" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;

    fn make_project() -> Project {
        Project::new(
            "rc2",
            vec![
                ProjectComponent::from_xml("Ctx.buc", CTX_BUC).unwrap(),
                ProjectComponent::from_xml("Mch.bum", SETCOMP_MACHINE_BUM).unwrap(),
            ],
        )
    }

    #[test]
    fn action_rhs_short_form_setcomp_is_emitted_as_long_form() {
        let r = build(&make_project());
        assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);
        let bcm = &r.file("Mch.bcm").expect("Mch.bcm").contents;
        // Without the `·`, ProB rejects the predicate with
        // `Parse failed because either: Expected: · but was: ∣`.
        assert!(
            bcm.contains("y⦂USERS·"),
            "expected long-form binder `y⦂USERS·` in emitted action:\n{bcm}"
        );
        assert!(
            bcm.contains(" ∣ y}"),
            "expected ` ∣ y}}` (binder name as member) in emitted action:\n{bcm}"
        );
        assert!(
            !bcm.contains("{y ∣"),
            "basic short form `{{y ∣` must not survive into the .bcm:\n{bcm}"
        );
    }
}

mod primed_declarations {
    //! Rodin parses every declaration with `primeAllowed = false`
    //! (`IdentifierModule`), so a primed variable or event parameter draws
    //! `InvalidIdentifierError` and registers no symbol. rossi reports EB033
    //! and drops the name, leaving the formulas that used it to cascade.

    use rossi_build::{BuildResult, Project, ProjectComponent, RuleId, build};

    fn build_source(filename: &str, source: &str) -> BuildResult {
        let components = ProjectComponent::from_eventb(filename, source).unwrap();
        build(&Project::new("p", components))
    }

    #[test]
    fn primed_variable_is_rejected_and_dropped() {
        let r = build_source(
            "M.eventb",
            r#"
MACHINE M
VARIABLES
    v'
INVARIANTS
    @inv1 v' ∈ ℕ
EVENTS
    EVENT INITIALISATION
    THEN
        @act1 v' ≔ 0
    END
END
"#,
        );
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.rule_id == Some(RuleId::PrimedDeclaredName)),
            "diagnostics: {:?}",
            r.diagnostics
        );
        let bcm = &r.file("M.bcm").expect("M.bcm").contents;
        assert!(
            !bcm.contains("scVariable"),
            "the primed variable must be dropped: {bcm}"
        );
    }

    #[test]
    fn primed_event_parameter_is_rejected() {
        let r = build_source(
            "N.eventb",
            r#"
MACHINE N
VARIABLES
    v
INVARIANTS
    @inv1 v ∈ ℕ
EVENTS
    EVENT INITIALISATION
    THEN
        @act1 v ≔ 0
    END
    EVENT e
    ANY
        p'
    WHERE
        @grd1 p' ∈ ℕ
    THEN
        @act2 v ≔ p'
    END
END
"#,
        );
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.rule_id == Some(RuleId::PrimedDeclaredName)),
            "diagnostics: {:?}",
            r.diagnostics
        );
    }

    /// The primed declaration a becomes-such-that introduces lives in the
    /// formula model, not in the machine's VARIABLES, so a machine using one
    /// must build clean.
    #[test]
    fn bound_after_state_read_builds_clean() {
        let r = build_source(
            "K.eventb",
            r#"
MACHINE K
VARIABLES
    x
INVARIANTS
    @inv1 x ∈ ℕ
EVENTS
    EVENT INITIALISATION
    THEN
        @act1 x ≔ 0
    END
    EVENT e
    THEN
        @act2 x :∣ x' = x + 1
    END
END
"#,
        );
        assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);
    }
}
