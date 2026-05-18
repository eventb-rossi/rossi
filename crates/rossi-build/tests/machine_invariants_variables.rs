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
fn emits_bcc_and_bcm() {
    let r = build(&make_project());
    assert_eq!(r.files.len(), 2);
    let names: Vec<_> = r.files.iter().map(|f| f.filename.as_str()).collect();
    assert!(names.contains(&"Ctx.bcc"), "expected Ctx.bcc in {names:?}");
    assert!(names.contains(&"Mch.bcm"), "expected Mch.bcm in {names:?}");
}

#[test]
fn machine_root_is_accurate() {
    let r = build(&make_project());
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
