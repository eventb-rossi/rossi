//! Decomposition base machines (`ch.ethz.eventb.decomposition.mchBase`)
//! must round-trip to an attribute-only `<scMachineFile/>`. Rodin's SC
//! does the same — the decomposition plugin owns the contents and the
//! standard SC produces a stub.
//!
//! Sources: a real-world corpus model has 15 such machines; their
//! reference `.bcm` is literally
//! `<org.eventb.core.scMachineFile org.eventb.core.configuration="ch.ethz.eventb.decomposition.mchBase"/>`.

use rossi_build::{Project, ProjectComponent, build, sc_view::ScView};

const STUB_CFG: &str = "ch.ethz.eventb.decomposition.mchBase";

fn stub_machine(name: &str, body: &str) -> ProjectComponent {
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="{STUB_CFG}">
{body}</org.eventb.core.machineFile>"#
    );
    ProjectComponent::from_xml(format!("{name}.bum"), &xml).unwrap()
}

#[test]
fn emits_attribute_only_stub() {
    let body = r#"<org.eventb.core.variable name="_v" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="typ" org.eventb.core.predicate="x ∈ ℤ" org.eventb.core.theorem="true"/>
<org.eventb.core.event name="_e0" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
"#;
    let project = Project::new("decomp_stub", vec![stub_machine("stub_m0", body)]);
    let r = build(&project);

    let bcm = r.file("stub_m0.bcm").expect("stub_m0.bcm");
    let expected = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n\
         <org.eventb.core.scMachineFile org.eventb.core.configuration=\"{STUB_CFG}\"/>\n"
    );
    assert_eq!(bcm.contents, expected);
    assert!(!bcm.accurate, "stub must report accurate=false");

    // Body declarations must NOT survive into the stub.
    let view = ScView::from_xml(&bcm.contents).unwrap();
    assert!(view.variables.is_empty());
    assert!(view.invariants.is_empty());
    assert!(view.events.is_empty());
    assert!(view.carrier_sets.is_empty());
    assert!(view.constants.is_empty());
    assert!(view.axioms.is_empty());
    assert!(!view.accurate);
}

#[test]
fn refinement_chain_of_stubs_does_not_crash() {
    // Mirrors the corpus model: one decomposition stub refining
    // another.
    let m0 = stub_machine("stub_m0", "");
    let m1_body = r#"<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="stub_m0"/>
"#;
    let m1 = stub_machine("stub_m1", m1_body);
    let project = Project::new("decomp_stub", vec![m0, m1]);
    let r = build(&project);

    assert_eq!(r.files.len(), 2);
    for f in &r.files {
        assert!(!f.accurate, "{} should be a stub", f.filename);
        assert!(
            f.contents
                .contains("<org.eventb.core.scMachineFile org.eventb.core.configuration"),
            "{} should be attribute-only: {}",
            f.filename,
            f.contents
        );
        assert!(
            !f.contents.contains("scInternalContext") && !f.contents.contains("scRefinesMachine"),
            "{} should have no body children: {}",
            f.filename,
            f.contents
        );
    }
    // No "REFINES unknown machine" diagnostics — both stubs register a
    // CheckedMachine entry so the refining stub finds its parent.
    assert!(
        !r.diagnostics
            .iter()
            .any(|d| d.message.contains("REFINES unknown")),
        "diagnostics: {:?}",
        r.diagnostics
    );
}

#[test]
fn non_decomposition_machine_unaffected() {
    // Sanity: a regular fwd-config machine still gets its body.
    let m = ProjectComponent::from_xml(
        "Reg.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_v" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
    let r = build(&Project::new("reg", vec![m]));
    let bcm = r.file("Reg.bcm").expect("Reg.bcm");
    let view = ScView::from_xml(&bcm.contents).unwrap();
    assert!(view.accurate);
    assert!(view.variables.contains_key("x"));
    assert!(!view.invariants.is_empty());
}
