//! RC2: a basic short-form set comprehension `{ x ∣ P }` on an action's
//! RHS must be promoted to the long form `{ x⦂T · P ∣ x }` in the
//! emitted .bcm. ProB's predicate parser only accepts the long form.

use rossi_build::{Project, ProjectComponent, build};

const CTX_BUC: &str = r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.carrierSet name="_set1" org.eventb.core.identifier="USERS"/>
</org.eventb.core.contextFile>
"#;

const MACHINE_BUM: &str = r#"<?xml version="1.0"?>
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
            ProjectComponent::from_xml("Mch.bum", MACHINE_BUM).unwrap(),
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
        bcm.contains("∣y}"),
        "expected `∣y}}` (binder name as member) in emitted action:\n{bcm}"
    );
    assert!(
        !bcm.contains("{y∣"),
        "basic short form `{{y∣` must not survive into the .bcm:\n{bcm}"
    );
}
