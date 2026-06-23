//! A machine variable whose only typing invariant *buries* it inside an
//! operand expression — here `w` in `f ∈ ℤ ⇸ ℤ ∖ {w}` — must still be
//! typed. Rodin types every free identifier by giving it a fresh type
//! variable (`getIdentType`) and solving the surrounding equations; the
//! SETMINUS forces `{w} : ℙ(ℤ)`, hence `w : ℤ`. Regression guard for the
//! "could not infer variable type" / "unknown identifier" cascade that
//! otherwise drops the variable and every clause referencing it.

use rossi_build::{Project, ProjectComponent, Severity, build, sc_view::ScView};

const MACHINE_BUM: &str = r#"<?xml version="1.0"?>
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

fn project() -> Project {
    Project::new(
        "p",
        vec![ProjectComponent::from_xml("M.bum", MACHINE_BUM).unwrap()],
    )
}

#[test]
fn variable_buried_in_invariant_is_typed() {
    let r = build(&project());

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
