//! Group P: untyped variables alone are not a file-level inaccuracy
//! signal. Rodin parity — verified against
//! `tutorial_contract-annotations.zip/AM1.bcm`, where variables
//! `x,y,z,pc` are untyped (no invariants) but the file stays
//! `accurate="true"` with only the writing event marked
//! `accurate="false"`. A bystander event that doesn't touch the
//! untyped variable stays `accurate="true"`.

use rossi_build::{Project, ProjectComponent, Severity, build, sc_view::ScView};

const MACHINE_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_v1" org.eventb.core.identifier="x"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_ev1" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt1"/>
</org.eventb.core.machineFile>
"#;

fn project() -> Project {
    Project::new(
        "p",
        vec![ProjectComponent::from_xml("M.bum", MACHINE_BUM).unwrap()],
    )
}

#[test]
fn file_stays_accurate_when_only_variable_typing_fails() {
    // The cascade-drop emits an Error diagnostic on the dropped action,
    // so `r.is_ok()` is intentionally false here. The file itself must
    // still emit, and the file-level `accurate` flag must stay `true`.
    let r = build(&project());
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
}

#[test]
fn untyped_variable_emits_warning_not_error() {
    let r = build(&project());
    let warnings: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .filter(|d| d.message.contains("could not infer variable type"))
        .collect();
    assert_eq!(
        warnings.len(),
        1,
        "expected exactly one untyped-variable warning; diagnostics: {:?}",
        r.diagnostics
    );
    assert_eq!(warnings[0].origin, "M.x");
}
