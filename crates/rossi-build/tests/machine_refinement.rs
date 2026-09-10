//! Refinement-focused machine checks, one section module per fixture:
//!
//! - `refines_machine` (M4): scRefinesMachine emission and invariant carrying.
//! - `implicit_refines` (B2): synthesised scRefinesEvent for `extended="true"`
//!   events with no explicit `<refinesEvent>` child.
//! - `extended_events` (M5): extended events and witnesses.
//! - `inherited_param_scope` (C1+C2): inherited-parameter scope, on both the
//!   rendered `.bcm` ScView and the `build_with_model` ScModel API.

mod refines_machine {
    //! M4: a concrete machine REFINES an abstract machine.
    //!
    //! Pattern (from binary-search M0 → M1):
    //!
    //! ```text
    //! M0: variable r, invariant inv1: r ∈ ℤ, event found(e): e ∈ ℤ → r := e
    //! M1 refines M0:
    //!   variable k (new), keeps r,
    //!   new invariant inv1: k ∈ ℤ (same label, different predicate — Rodin allows this)
    //!   (no event-level refinement in this M4 test; events with REFINES land in M5)
    //! ```
    //!
    //! Expected in M1.bcm:
    //! - scRefinesMachine pointing at M0.bcm
    //! - M0's invariant copied in with source= back to M0.bum (label kept)
    //! - M1's invariant emitted with source= pointing to M1.bum
    //!
    //! (The scVariable abstract/concrete flags for this shape are pinned in
    //! `concrete_vs_abstract_variables.rs`.)

    use rossi_build::{Project, ProjectComponent, build, sc_view::ScView};

    fn project() -> Project {
        let ctx = ProjectComponent::from_xml(
            "Ctx.buc",
            r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
</org.eventb.core.contextFile>"#,
        )
        .unwrap();
        let m0 = ProjectComponent::from_xml(
        "M0.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_s0" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v_r" org.eventb.core.identifier="r"/>
<org.eventb.core.invariant name="_i_m0_1" org.eventb.core.label="inv1" org.eventb.core.predicate="r ∈ ℤ"/>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
        let m1 = ProjectComponent::from_xml(
        "M1.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.seesContext name="_s1" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v_k" org.eventb.core.identifier="k"/>
<org.eventb.core.invariant name="_i_m1_1" org.eventb.core.label="inv1" org.eventb.core.predicate="k ∈ ℤ"/>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
        Project::new("m4", vec![ctx, m0, m1])
    }

    fn m1_view() -> ScView {
        let r = build(&project());
        assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);
        ScView::from_xml(&r.file("M1.bcm").expect("M1.bcm").contents).unwrap()
    }

    #[test]
    fn sc_refines_machine_emitted() {
        let r = build(&project());
        let bcm = &r.file("M1.bcm").expect("M1.bcm").contents;
        assert!(
            bcm.contains("<org.eventb.core.scRefinesMachine"),
            "expected scRefinesMachine in:\n{bcm}"
        );
        assert!(
            bcm.contains("/m4/M0.bcm"),
            "expected scTarget pointing at M0.bcm:\n{bcm}"
        );
    }

    #[test]
    fn both_invariants_carried_in_order() {
        // M0's inv first (source order matches Rodin's emission).
        let v = m1_view();
        assert_eq!(v.invariants.len(), 2, "expected two invariants, got {v:#?}");
        // At least one should have source pointing at M0.bum, one at M1.bum.
        let sources: Vec<_> = v.invariants.keys().cloned().collect();
        // ScView strips the leading /PROJECT/ from source URIs so lookups
        // aren't project-name-sensitive — the file-name fragment stays.
        assert!(
            sources.iter().any(|s| s.starts_with("M0.bum")),
            "expected an invariant sourced from M0.bum, got {sources:?}"
        );
        assert!(
            sources.iter().any(|s| s.starts_with("M1.bum")),
            "expected an invariant sourced from M1.bum, got {sources:?}"
        );
    }

    #[test]
    fn machine_is_accurate() {
        let r = build(&project());
        let bcm = r.file("M1.bcm").expect("M1.bcm");
        assert!(bcm.accurate, "diagnostics: {:?}", r.diagnostics);
    }
}

mod implicit_refines {
    //! B2: `extended="true"` events in a refined machine imply an
    //! `scRefinesEvent` pointing at the abstract event with the same label,
    //! even when the `.bum` has no explicit `<refinesEvent>` child.
    //!
    //! Rodin's SC synthesises the refinesEvent during checking. Our parser
    //! only fills `event.refines` when an explicit child element is present,
    //! so we must detect the implicit case and emit the `scRefinesEvent`
    //! anyway.
    //!
    //! Covers two subpatterns:
    //!
    //! 1. INITIALISATION in M1 extends INITIALISATION from M0 (classic
    //!    refinement pattern; most machines look like this).
    //! 2. A regular named event `E` in M1 declared with `extended="true"`
    //!    and no `<refinesEvent>` child.

    use rossi_build::{Project, ProjectComponent, build, sc_view::ScView};

    fn project() -> Project {
        let ctx = ProjectComponent::from_xml(
            "Ctx.buc",
            r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.carrierSet name="_s" org.eventb.core.identifier="USERS"/>
</org.eventb.core.contextFile>"#,
        )
        .unwrap();
        // M0: INITIALISATION with action, and event `E(u)` with guard and action.
        let m0 = ProjectComponent::from_xml(
        "M0.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_s0" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v_reg" org.eventb.core.identifier="registered"/>
<org.eventb.core.invariant name="_i0" org.eventb.core.label="inv1" org.eventb.core.predicate="registered ⊆ USERS"/>
<org.eventb.core.event name="_init0" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a_init" org.eventb.core.assignment="registered ≔ ∅" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_ev_E" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="E">
<org.eventb.core.parameter name="_p_u" org.eventb.core.identifier="u"/>
<org.eventb.core.guard name="_g_E" org.eventb.core.label="grd1" org.eventb.core.predicate="u ∈ USERS"/>
<org.eventb.core.action name="_a_E" org.eventb.core.assignment="registered ≔ registered ∪ {u}" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
        // M1: REFINES M0. BOTH events use extended="true" with no
        // <refinesEvent> child (the text-level `extends INITIALISATION`
        // sugar leaves no explicit refinesEvent in the XML).
        let m1 = ProjectComponent::from_xml(
        "M1.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.seesContext name="_s1" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v_reg" org.eventb.core.identifier="registered"/>
<org.eventb.core.event name="_init1" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION"></org.eventb.core.event>
<org.eventb.core.event name="_ev_E1" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="E"></org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
        Project::new("mb2", vec![ctx, m0, m1])
    }

    fn m1_view() -> ScView {
        let r = build(&project());
        assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);
        ScView::from_xml(&r.file("M1.bcm").expect("M1.bcm").contents).unwrap()
    }

    #[test]
    fn initialisation_refines_event_synthesised() {
        // `extended="true"` INITIALISATION in M1 with no explicit
        // refinesEvent must still get an scRefinesEvent pointing at M0.
        let v = m1_view();
        let init = v.events.get("INITIALISATION").expect("INITIALISATION");
        assert_eq!(
            init.refines_events.len(),
            1,
            "INITIALISATION should have exactly one scRefinesEvent (inherited); got {:#?}",
            init.refines_events
        );
        let target = init.refines_events.values().next().unwrap();
        assert!(
            target.contains("M0.bcm") && target.contains("scMachineFile#M0"),
            "scRefinesEvent should point at M0's scMachineFile; got {target}"
        );
        // Action inheritance works on the implicit path too (pinned in depth
        // by M5's `extended_event_inherits_parent_actions`).
        assert_eq!(init.actions.len(), 1);
    }

    #[test]
    fn regular_extended_event_refines_event_synthesised() {
        let v = m1_view();
        let e = v.events.get("E").expect("E");
        assert_eq!(
            e.refines_events.len(),
            1,
            "E should have scRefinesEvent (inherited); got {:#?}",
            e.refines_events
        );
        let target = e.refines_events.values().next().unwrap();
        assert!(
            target.contains("M0.bcm") && target.contains("scMachineFile#M0"),
            "scRefinesEvent should point at M0's scMachineFile; got {target}"
        );
        // Action inheritance works on the implicit path too.
        assert_eq!(e.actions.len(), 1);
    }
}

mod extended_events {
    //! M5: extended events and witnesses.
    //!
    //! Two patterns covered:
    //!
    //! 1. **Non-extended refinement** (`extended=false`) — concrete event has
    //!    its own guards/actions but is tied to abstract event via
    //!    `scRefinesEvent`. If the concrete event drops an abstract parameter,
    //!    a `scWitness` carries the witnessing predicate.
    //!
    //! 2. **Extended refinement** (`extended=true`) — concrete event inherits
    //!    *all* parameters/guards/actions from the abstract chain, emitted
    //!    under the concrete `scEvent` with `source=` URIs pointing at the
    //!    originating `.bum`.

    use rossi_build::{Project, ProjectComponent, build};

    fn project() -> Project {
        let ctx = ProjectComponent::from_xml(
            "Ctx.buc",
            r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.carrierSet name="_s" org.eventb.core.identifier="USERS"/>
</org.eventb.core.contextFile>"#,
        )
        .unwrap();
        // M0 — abstract machine.
        //   INITIALISATION: register := ∅
        //   found(e): guard e ∈ register → act1 r := e
        let m0 = ProjectComponent::from_xml(
        "M0.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_s0" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v_r" org.eventb.core.identifier="r"/>
<org.eventb.core.variable name="_v_reg" org.eventb.core.identifier="register"/>
<org.eventb.core.invariant name="_i0" org.eventb.core.label="inv1" org.eventb.core.predicate="register ⊆ USERS"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv2" org.eventb.core.predicate="r ∈ USERS"/>
<org.eventb.core.event name="_init0" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0_1" org.eventb.core.assignment="register ≔ ∅" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_found" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="found">
<org.eventb.core.parameter name="_p_e" org.eventb.core.identifier="e"/>
<org.eventb.core.guard name="_g0" org.eventb.core.label="grd1" org.eventb.core.predicate="e ∈ register"/>
<org.eventb.core.action name="_a0" org.eventb.core.assignment="r ≔ e" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
        // M1 refines M0.
        //   Adds variable `k`.
        //   INITIALISATION is extended=true — inherits `register := ∅`,
        //     adds `k := whatever`.
        //   found: not extended; has grd1 and action r := k; witnesses e = k.
        let m1 = ProjectComponent::from_xml(
        "M1.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.seesContext name="_s1" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v_k" org.eventb.core.identifier="k"/>
<org.eventb.core.variable name="r" org.eventb.core.identifier="r"/>
<org.eventb.core.variable name="register" org.eventb.core.identifier="register"/>
<org.eventb.core.invariant name="_i2" org.eventb.core.label="inv1" org.eventb.core.predicate="k ∈ register"/>
<org.eventb.core.event name="_init1" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION">
<org.eventb.core.refinesEvent name="_re_init" org.eventb.core.target="INITIALISATION"/>
<org.eventb.core.action name="_a1_init" org.eventb.core.assignment="k ≔ r" org.eventb.core.label="act2"/>
</org.eventb.core.event>
<org.eventb.core.event name="_found1" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="found">
<org.eventb.core.refinesEvent name="_re_found" org.eventb.core.target="found"/>
<org.eventb.core.guard name="_g_found" org.eventb.core.label="grd1" org.eventb.core.predicate="k ∈ register"/>
<org.eventb.core.action name="_a_found" org.eventb.core.assignment="r ≔ k" org.eventb.core.label="act1"/>
<org.eventb.core.witness name="_w_e" org.eventb.core.label="e" org.eventb.core.predicate="e = k"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
        Project::new("m5", vec![ctx, m0, m1])
    }

    fn m1_bcm() -> String {
        let r = build(&project());
        assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);
        r.file("M1.bcm").expect("M1.bcm").contents.clone()
    }

    #[test]
    fn refines_event_emitted_on_non_extended() {
        let bcm = m1_bcm();
        assert!(
            bcm.contains(r#"<org.eventb.core.scRefinesEvent"#),
            "expected scRefinesEvent in:\n{bcm}"
        );
        assert!(
            bcm.contains("scTarget=") && bcm.contains("/m5/M0.bcm"),
            "scRefinesEvent should point at M0's scEvent:\n{bcm}"
        );
    }

    #[test]
    fn witness_emitted_for_dropped_parameter() {
        let bcm = m1_bcm();
        // `found` in M1 drops the abstract parameter `e` — witness `e = k`
        // must appear inside that event.
        assert!(
            bcm.contains("<org.eventb.core.scWitness"),
            "expected scWitness in:\n{bcm}"
        );
        assert!(
            bcm.contains(r#"org.eventb.core.predicate="e=k""#),
            "witness predicate `e=k` missing:\n{bcm}"
        );
    }

    #[test]
    fn extended_event_inherits_parent_actions() {
        let bcm = m1_bcm();
        // INITIALISATION is extended — it should inline M0's
        // `register ≔ ∅ ⦂ ℙ(USERS)` action AND its own `k ≔ r`.
        assert!(
            bcm.contains(r#"org.eventb.core.assignment="register ≔ ∅ ⦂ ℙ(USERS)""#),
            "expected inherited action from M0's INITIALISATION:\n{bcm}"
        );
        assert!(
            bcm.contains(r#"org.eventb.core.assignment="k ≔ r""#),
            "expected own action from M1's INITIALISATION:\n{bcm}"
        );
        // The inherited action's source should be M0.bum, not M1.bum.
        assert!(
            bcm.contains(r#"source="/m5/M0.bum"#) && bcm.contains(r#"action#_a0_1""#),
            "inherited action should carry M0.bum source:\n{bcm}"
        );
    }

    #[test]
    fn non_extended_event_does_not_inherit_parent_guards() {
        // M1's `found` has its own `grd1` referring to `k`; the abstract's
        // `grd1: e ∈ register` should NOT appear (extended=false).
        let bcm = m1_bcm();
        assert!(
            !bcm.contains(r#"predicate="e∈register""#),
            "non-extended event should not carry abstract guards; got:\n{bcm}"
        );
    }

    #[test]
    fn extended_event_redeclaring_inherited_parameter_is_diagnosed() {
        let m0 = ProjectComponent::from_xml(
        "M0.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION"/>
<org.eventb.core.event name="_event" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt">
<org.eventb.core.parameter name="_param" org.eventb.core.identifier="p"/>
<org.eventb.core.guard name="_guard" org.eventb.core.label="grd1" org.eventb.core.predicate="p ∈ ℤ"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
        let m1 = ProjectComponent::from_xml(
        "M1.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref_m" org.eventb.core.target="M0"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION">
<org.eventb.core.refinesEvent name="_ref_i" org.eventb.core.target="INITIALISATION"/>
</org.eventb.core.event>
<org.eventb.core.event name="_event" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="evt">
<org.eventb.core.refinesEvent name="_ref_e" org.eventb.core.target="evt"/>
<org.eventb.core.parameter name="_param" org.eventb.core.identifier="p"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
        let result = build(&Project::new("conflict", vec![m0, m1]));
        assert!(
            result.diagnostics.iter().any(|diagnostic| {
                diagnostic.origin == "M1.evt.p"
                    && diagnostic
                        .message
                        .contains("parameter `p` conflicts with an inherited parameter")
            }),
            "expected inherited parameter conflict: {:?}",
            result.diagnostics
        );
    }
}

mod inherited_param_scope {
    //! C1+C2: an `extended="true"` concrete event inherits parameters
    //! from the abstract chain. Both:
    //!
    //! 1. Type-inference for any new own parameters must see inherited
    //!    guards as typing axioms (C1).
    //! 2. When checking the event's own guards and actions, inherited
    //!    parameter names must be in the type env (C2).
    //!
    //! The common real-world pattern: a
    //! concrete event `E` is extended="true" without redeclaring the
    //! abstract parameter, and adds its own guard that references the
    //! inherited parameter. Our current code drops that guard because the
    //! identifier resolution walker can't see the inherited parameter.
    //!
    //! The same fixture also exercises the `ScModel` surface that downstream
    //! formula passes (well-definedness) build on:
    //!
    //! - `CheckedMachine::record` carries the typed invariant / variant /
    //!   event ASTs the `.bcm` was rendered from.
    //! - `EventDecl::chain_parameters` exposes inherited parameters of an
    //!   `extended="true"` event without redeclaration.
    //! - `CheckedMachine::event_env` rebuilds the event-local type scope
    //!   (machine env + chain parameters).

    use rossi_build::normalize::{canonical_typed_expression, canonical_typed_predicate};
    use rossi_build::{Project, ProjectComponent, Type, build, build_with_model, sc_view::ScView};

    fn project() -> Project {
        let ctx = ProjectComponent::from_xml(
            "Ctx.buc",
            r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.carrierSet name="_s" org.eventb.core.identifier="USERS"/>
</org.eventb.core.contextFile>"#,
        )
        .unwrap();
        // M0: `E(u)` with guard `u ∈ USERS`, action `registered := registered ∪ {u}`,
        // plus a variant so the record carries a typed variant expression.
        let m0 = ProjectComponent::from_xml(
        "M0.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_s0" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v_reg" org.eventb.core.identifier="registered"/>
<org.eventb.core.invariant name="_i0" org.eventb.core.label="inv1" org.eventb.core.predicate="registered ⊆ USERS ∧ (∀x · x ∈ ℤ ⇒ x = x)"/>
<org.eventb.core.variant name="_vr" org.eventb.core.expression="card({x ∣ x ∈ registered} ∖ registered)"/>
<org.eventb.core.event name="_init0" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="registered ≔ ∅" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_e" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="E">
<org.eventb.core.parameter name="_p_u" org.eventb.core.identifier="u"/>
<org.eventb.core.guard name="_g1" org.eventb.core.label="grd1" org.eventb.core.predicate="u ∈ USERS"/>
<org.eventb.core.action name="_a1" org.eventb.core.assignment="registered ≔ registered ∪ {u}" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_w" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="W">
<org.eventb.core.parameter name="_p" org.eventb.core.identifier="p"/>
<org.eventb.core.guard name="_g_w" org.eventb.core.label="grd1" org.eventb.core.predicate="p ∈ ℤ"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
        // M1 REFINES M0. `E` is extended="true" and adds its own guard
        // `u ∉ registered` referencing the INHERITED parameter u. No
        // explicit <parameter> redeclaration — that's the implicit pattern.
        // `W` refines `W` non-extended, witnessing the dropped parameter `p`.
        let m1 = ProjectComponent::from_xml(
        "M1.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="M0"/>
<org.eventb.core.seesContext name="_s1" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v_reg" org.eventb.core.identifier="registered"/>
<org.eventb.core.event name="_init1" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION"></org.eventb.core.event>
<org.eventb.core.event name="_e1" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="E">
<org.eventb.core.guard name="_g_own" org.eventb.core.label="grd_own" org.eventb.core.predicate="u ∉ registered"/>
</org.eventb.core.event>
<org.eventb.core.event name="_w1" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="W">
<org.eventb.core.refinesEvent name="_re" org.eventb.core.target="W"/>
<org.eventb.core.witness name="_wit" org.eventb.core.label="p" org.eventb.core.predicate="p = 0 ∧ (∀z · z = p)"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
        Project::new("scope", vec![ctx, m0, m1])
    }

    fn m1_view() -> ScView {
        let r = build(&project());
        assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);
        ScView::from_xml(&r.file("M1.bcm").expect("M1.bcm").contents).unwrap()
    }

    #[test]
    fn own_guard_referencing_inherited_param_survives() {
        // The concrete `E`'s own guard `u ∉ registered` names the
        // inherited parameter `u`. It must appear in M1.bcm — the
        // identifier walker must see `u` bound through the extended chain.
        let v = m1_view();
        let e = v.events.get("E").expect("E");
        let own_guard = e
            .guards
            .values()
            .find(|g| g.label == "grd_own")
            .expect("own guard grd_own");
        assert_eq!(
            own_guard.predicate,
            rossi::parse_predicate_str("u ∉ registered").unwrap(),
        );
    }

    #[test]
    fn extended_event_carries_all_guards() {
        // Inherited guard `u ∈ USERS` PLUS own guard `u ∉ registered`.
        let v = m1_view();
        let e = v.events.get("E").expect("E");
        assert_eq!(
            e.guards.len(),
            2,
            "expected both inherited and own guard; got {:#?}",
            e.guards
        );
    }

    #[test]
    fn event_stays_accurate() {
        let v = m1_view();
        let e = v.events.get("E").expect("E");
        assert!(
            e.accurate,
            "event should be accurate; something was dropped"
        );
    }

    #[test]
    fn parameter_inherited() {
        // `u` should still show up as an scParameter of E (inherited).
        let v = m1_view();
        let e = v.events.get("E").expect("E");
        assert_eq!(e.parameters.get("u").map(String::as_str), Some("USERS"));
    }

    #[test]
    fn machine_record_carries_typed_formulas() {
        let (r, model) = build_with_model(&project());
        assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);

        let m0 = model.machines.get("M0").expect("M0 in model");
        assert_eq!(m0.record.invariants.len(), 1);
        let invariant = &m0.record.invariants[0];
        assert_eq!(invariant.label, "inv1");
        assert!(
            canonical_typed_predicate(&invariant.typed).contains("∀x⦂ℤ·"),
            "invariant binder should carry its type: {:?}",
            invariant.predicate
        );

        let variant = m0.record.variants.first().expect("variant in record");
        let variant_typed = variant.typed.as_ref().expect("variant type-checks");
        assert_eq!(
            canonical_typed_expression(variant_typed),
            "card({x⦂USERS·x∈registered ∣ x} ∖ registered)"
        );

        let event = model.machines["M1"]
            .events_by_label
            .get("W")
            .expect("W in M1");
        let witness = event.witnesses.first().expect("p witness");
        assert_eq!(witness.label, "p");
        assert!(
            canonical_typed_predicate(&witness.typed).contains("∀z⦂ℤ·"),
            "witness binder should carry its type: {:?}",
            witness.predicate
        );
    }

    #[test]
    fn chain_parameters_sees_inherited_param() {
        let (r, model) = build_with_model(&project());
        assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);

        let m1 = model.machines.get("M1").expect("M1 in model");
        let e = m1.events_by_label.get("E").expect("event E");
        // M1's E declares no own parameters; `u` arrives via the chain.
        assert!(e.parameters.is_empty());
        let params: Vec<&str> = e
            .chain_parameters()
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(params, ["u"]);

        // A non-extended event has no chain.
        let m0 = model.machines.get("M0").expect("M0 in model");
        let e0 = m0.events_by_label.get("E").expect("event E in M0");
        let own: Vec<&str> = e0
            .chain_parameters()
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(own, ["u"]);
    }

    #[test]
    fn event_env_resolves_chain_params_and_variables() {
        let (r, model) = build_with_model(&project());
        assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);

        let m1 = model.machines.get("M1").expect("M1 in model");
        let e = m1.events_by_label.get("E").expect("event E");
        let env = m1.event_env(e);

        let users = Type::Given("USERS".into());
        assert_eq!(env.get("u"), Some(&users), "inherited parameter typed");
        assert_eq!(
            env.get("registered"),
            Some(&Type::pow(users.clone())),
            "machine variable visible"
        );
        assert!(env.get("nonexistent").is_none());
    }
}
