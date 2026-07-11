//! INITIALISATION repair: a concrete, typed variable that no action
//! assigns gets a synthetic `becomesSuchThat ⊤` default, all such
//! variables gathered into one `GEN` action, and the event (not the
//! machine) is marked inaccurate.

use rossi_build::{Project, ProjectComponent, build, sc_view::ScView};

// --- standalone machine, INITIALISATION has no actions ---------------
const ONE_VAR_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_v" org.eventb.core.identifier="v"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="I" org.eventb.core.predicate="v ∈ ℕ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION"/>
</org.eventb.core.machineFile>
"#;

// Byte-exact expectation for the case above: the
// machine stays accurate, INITIALISATION is inaccurate, and carries one
// generated `v :∣ ⊤` action sourced at the event element.
const ONE_VAR_BCM: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.scMachineFile org.eventb.core.accurate="true" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.scInvariant name="'" org.eventb.core.label="I" org.eventb.core.predicate="v∈ℕ" org.eventb.core.source="/r/M.bum|org.eventb.core.machineFile#M|org.eventb.core.invariant#_i" org.eventb.core.theorem="false"/>
<org.eventb.core.scVariable name="v" org.eventb.core.abstract="false" org.eventb.core.concrete="true" org.eventb.core.source="/r/M.bum|org.eventb.core.machineFile#M|org.eventb.core.variable#_v" org.eventb.core.type="ℤ"/>
<org.eventb.core.scEvent name="w" org.eventb.core.accurate="false" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION" org.eventb.core.source="/r/M.bum|org.eventb.core.machineFile#M|org.eventb.core.event#_init">
<org.eventb.core.scAction name="'" org.eventb.core.assignment="v :∣ ⊤" org.eventb.core.label="GEN" org.eventb.core.source="/r/M.bum|org.eventb.core.machineFile#M|org.eventb.core.event#_init"/>
</org.eventb.core.scEvent>
</org.eventb.core.scMachineFile>"#;

// --- standalone machine, INITIALISATION assigns only one of two ------
const PARTIAL_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_a" org.eventb.core.identifier="a"/>
<org.eventb.core.variable name="_b" org.eventb.core.identifier="b"/>
<org.eventb.core.invariant name="_ia" org.eventb.core.label="ia" org.eventb.core.predicate="a ∈ ℕ"/>
<org.eventb.core.invariant name="_ib" org.eventb.core.label="ib" org.eventb.core.predicate="b ∈ ℕ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a0" org.eventb.core.assignment="a ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;

// --- standalone machine, INITIALISATION empty, two variables --------
// Variables are declared b-then-a to confirm the combined GEN LHS is
// emitted in rossi's deterministic alphabetical order, not source order.
const EMPTY_TWO_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_b" org.eventb.core.identifier="b"/>
<org.eventb.core.variable name="_a" org.eventb.core.identifier="a"/>
<org.eventb.core.invariant name="_ia" org.eventb.core.label="ia" org.eventb.core.predicate="a ∈ ℕ"/>
<org.eventb.core.invariant name="_ib" org.eventb.core.label="ib" org.eventb.core.predicate="b ∈ ℕ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION"/>
</org.eventb.core.machineFile>
"#;

// --- refinement, extended INITIALISATION, both INITs incomplete ------
const ABS_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_ix" org.eventb.core.label="ix" org.eventb.core.predicate="x ∈ ℕ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION"/>
</org.eventb.core.machineFile>
"#;

const REF_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_ref" org.eventb.core.target="Abs"/>
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.variable name="_y" org.eventb.core.identifier="y"/>
<org.eventb.core.invariant name="_iy" org.eventb.core.label="iy" org.eventb.core.predicate="y ∈ ℕ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION"/>
</org.eventb.core.machineFile>
"#;

fn one_var() -> Project {
    Project::new(
        "r",
        vec![ProjectComponent::from_xml("M.bum", ONE_VAR_BUM).unwrap()],
    )
}

fn partial() -> Project {
    Project::new(
        "r",
        vec![ProjectComponent::from_xml("M.bum", PARTIAL_BUM).unwrap()],
    )
}

fn empty_two() -> Project {
    Project::new(
        "r",
        vec![ProjectComponent::from_xml("M.bum", EMPTY_TWO_BUM).unwrap()],
    )
}

fn refinement() -> Project {
    Project::new(
        "r",
        vec![
            ProjectComponent::from_xml("Abs.bum", ABS_BUM).unwrap(),
            ProjectComponent::from_xml("Ref.bum", REF_BUM).unwrap(),
        ],
    )
}

#[test]
fn standalone_empty_init_repairs_single_variable_byte_exact() {
    let r = build(&one_var());
    let bcm = r.file("M.bcm").expect("M.bcm");
    assert_eq!(bcm.contents.trim_end(), ONE_VAR_BCM.trim_end());
}

#[test]
fn repaired_init_is_inaccurate_but_machine_stays_accurate() {
    let r = build(&one_var());
    let v = ScView::from_xml(&r.file("M.bcm").unwrap().contents).unwrap();
    assert!(v.accurate, "machine root must stay accurate");
    let init = v
        .events
        .get("INITIALISATION")
        .expect("INITIALISATION emitted");
    assert!(
        !init.accurate,
        "INITIALISATION must be inaccurate after repair"
    );
}

#[test]
fn partial_init_repairs_only_the_unassigned_variable() {
    let r = build(&partial());
    let bcm = &r.file("M.bcm").unwrap().contents;
    // `a` is assigned, so only `b` is repaired; the real action survives.
    assert!(bcm.contains(r#"org.eventb.core.assignment="a ≔ 0" org.eventb.core.label="act1""#));
    assert!(bcm.contains(r#"org.eventb.core.assignment="b :∣ ⊤" org.eventb.core.label="GEN""#));
    assert!(
        !bcm.contains(r#"assignment="a :∣"#),
        "assigned variable `a` must not be repaired:\n{bcm}"
    );
    let v = ScView::from_xml(bcm).unwrap();
    assert!(v.accurate);
    assert!(!v.events["INITIALISATION"].accurate);
}

#[test]
fn empty_init_combines_unassigned_variables_into_one_action() {
    let r = build(&empty_two());
    let bcm = &r.file("M.bcm").unwrap().contents;
    // One combined `GEN` action, variables in alphabetical order.
    assert!(
        bcm.contains(r#"org.eventb.core.assignment="a,b :∣ ⊤" org.eventb.core.label="GEN""#),
        "expected a single combined GEN action `a,b :∣ ⊤`:\n{bcm}"
    );
    // Exactly one scAction (the combined GEN), not one per variable. ScView
    // keys actions by source URI, so per-variable GEN actions would collapse
    // to a single entry there and hide the regression; count raw elements.
    assert_eq!(
        bcm.matches("<org.eventb.core.scAction ").count(),
        1,
        "all unassigned variables must share one GEN action:\n{bcm}"
    );
}

#[test]
fn extended_init_inherits_repair_and_adds_fresh_label() {
    let r = build(&refinement());

    // Abstract INIT is repaired for its own variable `x`.
    let abs = &r.file("Abs.bcm").unwrap().contents;
    assert!(abs.contains(r#"org.eventb.core.assignment="x :∣ ⊤" org.eventb.core.label="GEN""#));

    // The extended child inherits that `GEN: x :∣ ⊤` (so `x` is already
    // assigned) and repairs only the new variable `y` under a fresh
    // `GEN1` label.
    let refn = &r.file("Ref.bcm").unwrap().contents;
    assert!(refn.contains(r#"org.eventb.core.assignment="x :∣ ⊤" org.eventb.core.label="GEN""#));
    assert!(refn.contains(r#"org.eventb.core.assignment="y :∣ ⊤" org.eventb.core.label="GEN1""#));

    let v = ScView::from_xml(refn).unwrap();
    let init = &v.events["INITIALISATION"];
    assert!(!init.accurate);
    assert_eq!(init.actions.len(), 2, "inherited x plus generated y");
}
