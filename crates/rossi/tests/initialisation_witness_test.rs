//! Witnesses on the INITIALISATION event.
//!
//! A refinement that drops a variable the abstract INITIALISATION assigns
//! nondeterministically witnesses its after-value there (`@x' …`); Rodin's
//! static checker requires it, and Camille, which reads INITIALISATION as an
//! ordinary event, writes it in a WITH clause before THEN. The parser and the
//! printer must both carry it; the recovery parser's side is pinned with the
//! other recovery tests.

use rossi::{Component, PrettyPrinter, format_str, parse, parse_xml, to_string};

/// A refinement whose INITIALISATION witnesses the dropped `e`.
const WITNESSED: &str = "\
machine M1 refines M0
variables x
invariants
  @inv1 x = e + 1
events
  event INITIALISATION
    with
      // e was one less
      @e' e' = x' − 1
    then
      @act1 x ≔ 1
  end
end
";

fn machine(component: Component) -> rossi::Machine {
    match component {
        Component::Machine(machine) => machine,
        Component::Context(_) => panic!("expected a machine"),
    }
}

#[test]
fn the_witness_is_read_into_the_initialisation() {
    let machine = machine(parse(WITNESSED).expect("the refinement parses"));
    assert!(machine.events.is_empty(), "{:?}", machine.events);
    let init = machine.initialisation.expect("an INITIALISATION");
    let labels: Vec<_> = init.with.iter().map(|w| w.label.as_deref()).collect();
    assert_eq!(labels, [Some("e'")]);
    assert_eq!(init.actions.len(), 1);

    // Under WITNESS too, and on an extended INITIALISATION.
    let other = WITNESSED.replace("    with\n", "    witness\n").replace(
        "event INITIALISATION\n",
        "event INITIALISATION extends INITIALISATION\n",
    );
    let init = machine_init(&other);
    assert!(init.extended);
    assert_eq!(init.witnesses.len(), 1);
}

fn machine_init(text: &str) -> rossi::InitialisationEvent {
    machine(parse(text).expect("the refinement parses"))
        .initialisation
        .expect("an INITIALISATION")
}

#[test]
fn an_imported_witness_is_printed_and_read_back() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.machineFile org.eventb.core.configuration="org.eventb.core.fwd" version="5">
<org.eventb.core.refinesMachine name="_r" org.eventb.core.target="M0"/>
<org.eventb.core.variable name="_v" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_i" org.eventb.core.label="inv1" org.eventb.core.predicate="x = e + 1"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.witness name="_w" org.eventb.core.label="e'" org.eventb.core.predicate="e' = x' − 1"/>
<org.eventb.core.action name="_a" org.eventb.core.assignment="x ≔ 1" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;
    let text = to_string(&parse_xml(xml).expect("the XML reads"));
    assert!(text.contains("@e' e' = x' − 1"), "{text}");
    let init = machine_init(&text);
    assert_eq!(init.with.len(), 1, "{text}");
}

#[test]
fn formatting_keeps_the_witness_and_its_comment() {
    let formatted =
        format_str(WITNESSED, &PrettyPrinter::default()).expect("the refinement formats");
    assert!(
        formatted.contains("    with\n      // e was one less\n      @e' e' = x' − 1\n    then\n"),
        "{formatted}"
    );
}
