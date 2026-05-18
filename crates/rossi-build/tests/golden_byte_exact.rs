//! Byte-exact regression locks.
//!
//! These tests pin the three `.bcc` fixtures that currently produce
//! byte-for-byte identical output to Rodin. They run before any
//! whitespace-sensitive change (e.g. step #27's ScView rework) so that
//! silent byte-exact regressions get caught immediately rather than
//! surfacing as a drop in the corpus metric.
//!
//! The auction fixture is already covered by
//! `tests/context_carrier_sets.rs::auction_context_bcc_is_byte_exact`.
//! This file adds the other two.

use rossi_build::{Project, ProjectComponent, build};

/// progman/c0_users.buc (single deferred carrier set USER).
const PROGMAN_C0_USERS_BUC: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.contextFile org.eventb.core.configuration="org.eventb.core.fwd" org.eventb.core.generated="false" version="3">
<org.eventb.core.carrierSet name="_swFc0EHSEeqHCPZ-G665YQ" org.eventb.core.generated="false" org.eventb.core.identifier="USER"/>
</org.eventb.core.contextFile>
"#;

/// progman/c0_users.bcc as Rodin emits it.
const PROGMAN_C0_USERS_BCC: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.scContextFile org.eventb.core.accurate="true" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.scCarrierSet name="USER" org.eventb.core.source="/progmanmodel/c0_users.buc|org.eventb.core.contextFile#c0_users|org.eventb.core.carrierSet#_swFc0EHSEeqHCPZ-G665YQ" org.eventb.core.type="ℙ(USER)"/>
</org.eventb.core.scContextFile>"#;

/// ca648_assignment1/Question1_C0.buc (two deferred carrier sets, alpha-sorted).
const CA648_Q1_C0_BUC: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.contextFile org.eventb.core.configuration="org.eventb.core.fwd;de.prob.symbolic.ctxBase;de.prob.units.mchBase" version="3">
<org.eventb.core.carrierSet name="'" org.eventb.core.identifier="BOOK"/>
<org.eventb.core.carrierSet name="(" org.eventb.core.identifier="CHILD"/>
</org.eventb.core.contextFile>
"#;

const CA648_Q1_C0_BCC: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.scContextFile org.eventb.core.accurate="true" org.eventb.core.configuration="org.eventb.core.fwd;de.prob.symbolic.ctxBase;de.prob.units.mchBase">
<org.eventb.core.scCarrierSet name="BOOK" org.eventb.core.source="/Question1/Question1_C0.buc|org.eventb.core.contextFile#Question1_C0|org.eventb.core.carrierSet#'" org.eventb.core.type="ℙ(BOOK)"/>
<org.eventb.core.scCarrierSet name="CHILD" org.eventb.core.source="/Question1/Question1_C0.buc|org.eventb.core.contextFile#Question1_C0|org.eventb.core.carrierSet#(" org.eventb.core.type="ℙ(CHILD)"/>
</org.eventb.core.scContextFile>"#;

#[test]
fn progman_c0_users_bcc_byte_exact() {
    let pc = ProjectComponent::from_xml("c0_users.buc", PROGMAN_C0_USERS_BUC).expect("parse");
    // Rodin's own URIs in this model use the project name "progmanmodel".
    let project = Project::new("progmanmodel", vec![pc]);
    let result = build(&project);
    assert_eq!(result.files.len(), 1);
    assert_eq!(
        result.files[0].contents.trim_end(),
        PROGMAN_C0_USERS_BCC.trim_end()
    );
}

#[test]
fn ca648_question1_c0_bcc_byte_exact() {
    let pc = ProjectComponent::from_xml("Question1_C0.buc", CA648_Q1_C0_BUC).expect("parse");
    // Rodin's URIs use "Question1" here (one of four sibling projects in
    // the zip; the filename stem is the project name).
    let project = Project::new("Question1", vec![pc]);
    let result = build(&project);
    assert_eq!(result.files.len(), 1);
    assert_eq!(
        result.files[0].contents.trim_end(),
        CA648_Q1_C0_BCC.trim_end()
    );
}
