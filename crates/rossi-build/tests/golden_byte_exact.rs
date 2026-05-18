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

/// A corpus-derived context with a single deferred carrier set USER.
const USERMODEL_C0_USERS_BUC: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.contextFile org.eventb.core.configuration="org.eventb.core.fwd" org.eventb.core.generated="false" version="3">
<org.eventb.core.carrierSet name="_internal0000000000001" org.eventb.core.generated="false" org.eventb.core.identifier="USER"/>
</org.eventb.core.contextFile>
"#;

/// The corresponding `.bcc` exactly as Rodin emits it.
const USERMODEL_C0_USERS_BCC: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.scContextFile org.eventb.core.accurate="true" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.scCarrierSet name="USER" org.eventb.core.source="/usermodel/c0_users.buc|org.eventb.core.contextFile#c0_users|org.eventb.core.carrierSet#_internal0000000000001" org.eventb.core.type="ℙ(USER)"/>
</org.eventb.core.scContextFile>"#;

/// Question1_C0.buc from a corpus assignment archive (two deferred
/// carrier sets, alpha-sorted).
const Q1_C0_BUC: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.contextFile org.eventb.core.configuration="org.eventb.core.fwd;de.prob.symbolic.ctxBase;de.prob.units.mchBase" version="3">
<org.eventb.core.carrierSet name="'" org.eventb.core.identifier="BOOK"/>
<org.eventb.core.carrierSet name="(" org.eventb.core.identifier="CHILD"/>
</org.eventb.core.contextFile>
"#;

const Q1_C0_BCC: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.scContextFile org.eventb.core.accurate="true" org.eventb.core.configuration="org.eventb.core.fwd;de.prob.symbolic.ctxBase;de.prob.units.mchBase">
<org.eventb.core.scCarrierSet name="BOOK" org.eventb.core.source="/Question1/Question1_C0.buc|org.eventb.core.contextFile#Question1_C0|org.eventb.core.carrierSet#'" org.eventb.core.type="ℙ(BOOK)"/>
<org.eventb.core.scCarrierSet name="CHILD" org.eventb.core.source="/Question1/Question1_C0.buc|org.eventb.core.contextFile#Question1_C0|org.eventb.core.carrierSet#(" org.eventb.core.type="ℙ(CHILD)"/>
</org.eventb.core.scContextFile>"#;

#[test]
fn usermodel_c0_users_bcc_byte_exact() {
    let pc = ProjectComponent::from_xml("c0_users.buc", USERMODEL_C0_USERS_BUC).expect("parse");
    // Rodin's URIs use the project name; ours must match it.
    let project = Project::new("usermodel", vec![pc]);
    let result = build(&project);
    assert_eq!(result.files.len(), 1);
    assert_eq!(
        result.files[0].contents.trim_end(),
        USERMODEL_C0_USERS_BCC.trim_end()
    );
}

#[test]
fn question1_c0_bcc_byte_exact() {
    let pc = ProjectComponent::from_xml("Question1_C0.buc", Q1_C0_BUC).expect("parse");
    // Rodin's URIs use "Question1" here (one of four sibling projects in
    // the zip; the filename stem is the project name).
    let project = Project::new("Question1", vec![pc]);
    let result = build(&project);
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].contents.trim_end(), Q1_C0_BCC.trim_end());
}
