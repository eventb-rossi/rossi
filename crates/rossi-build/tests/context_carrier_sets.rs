//! End-to-end test: a context with only carrier sets should produce a .bcc
//! whose scCarrierSet rows semantically match Rodin's output.
//!
//! Fixture is inlined so this test runs without external files.

use rossi_build::{Project, ProjectComponent, build};

/// The raw AuctionContext.buc fixture.
/// Trimmed of the `text_representation` and `text_lastmodified` attributes
/// that Rodin puts on the root and which the parser already drops.
const AUCTION_CONTEXT_BUC: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.contextFile org.eventb.core.configuration="org.eventb.core.fwd;org.eventb.codegen.ui.cgConfig;de.prob.symbolic.ctxBase;de.prob.units.mchBase" version="3">
<org.eventb.core.carrierSet name="_w4LsYO5MEeSpR9iqQeSCVw" org.eventb.core.identifier="USERS"/>
<org.eventb.core.carrierSet name="_qJ3S4O5PEeSpR9iqQeSCVw" org.eventb.core.identifier="AUCTIONS"/>
<org.eventb.core.carrierSet name="_4PKc0O5TEeSpR9iqQeSCVw" org.eventb.core.identifier="ITEMS"/>
</org.eventb.core.contextFile>
"#;

fn make_project() -> Project {
    let pc = ProjectComponent::from_xml("AuctionContext.buc", AUCTION_CONTEXT_BUC).expect("parse");
    Project::new("COMP1216", vec![pc])
}

#[test]
fn emits_one_bcc_file() {
    let project = make_project();
    let result = build(&project);
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].filename, "AuctionContext.bcc");
    assert!(result.files[0].accurate);
}

#[test]
fn carrier_sets_are_sorted_alphabetically() {
    let project = make_project();
    let result = build(&project);
    let xml = &result.files[0].contents;
    let idx_auctions = xml.find("name=\"AUCTIONS\"").expect("AUCTIONS present");
    let idx_items = xml.find("name=\"ITEMS\"").expect("ITEMS present");
    let idx_users = xml.find("name=\"USERS\"").expect("USERS present");
    assert!(
        idx_auctions < idx_items && idx_items < idx_users,
        "expected AUCTIONS < ITEMS < USERS, got order {idx_auctions}, {idx_items}, {idx_users}"
    );
}

#[test]
fn carrier_sets_have_powerset_type() {
    let project = make_project();
    let result = build(&project);
    let xml = &result.files[0].contents;
    for set in ["AUCTIONS", "ITEMS", "USERS"] {
        let expected = format!("org.eventb.core.type=\"ℙ({set})\"");
        assert!(
            xml.contains(&expected),
            "expected {expected} in output:\n{xml}"
        );
    }
}

#[test]
fn root_element_and_accuracy() {
    let project = make_project();
    let result = build(&project);
    let xml = &result.files[0].contents;
    assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n"));
    assert!(xml.contains("<org.eventb.core.scContextFile"));
    assert!(xml.contains("org.eventb.core.accurate=\"true\""));
    assert!(xml.trim_end().ends_with("</org.eventb.core.scContextFile>"));
}

#[test]
fn carrier_set_source_handle_uses_rodin_internal_id() {
    let project = make_project();
    let result = build(&project);
    let xml = &result.files[0].contents;
    // Exact URI Rodin emits in auction's AuctionContext.bcc.
    for id in [
        "_w4LsYO5MEeSpR9iqQeSCVw", // USERS
        "_qJ3S4O5PEeSpR9iqQeSCVw", // AUCTIONS
        "_4PKc0O5TEeSpR9iqQeSCVw", // ITEMS
    ] {
        let expected = format!(
            "/COMP1216/AuctionContext.buc|org.eventb.core.contextFile#AuctionContext|org.eventb.core.carrierSet#{id}"
        );
        assert!(
            xml.contains(&expected),
            "expected {expected} in output:\n{xml}"
        );
    }
}

/// Matches Rodin's AuctionContext.bcc byte-for-byte, modulo trailing newline.
#[test]
fn auction_context_bcc_is_byte_exact() {
    let project = make_project();
    let result = build(&project);
    let actual = result.files[0].contents.trim_end();
    let expected = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.scContextFile org.eventb.core.accurate="true" org.eventb.core.configuration="org.eventb.core.fwd;org.eventb.codegen.ui.cgConfig;de.prob.symbolic.ctxBase;de.prob.units.mchBase">
<org.eventb.core.scCarrierSet name="AUCTIONS" org.eventb.core.source="/COMP1216/AuctionContext.buc|org.eventb.core.contextFile#AuctionContext|org.eventb.core.carrierSet#_qJ3S4O5PEeSpR9iqQeSCVw" org.eventb.core.type="ℙ(AUCTIONS)"/>
<org.eventb.core.scCarrierSet name="ITEMS" org.eventb.core.source="/COMP1216/AuctionContext.buc|org.eventb.core.contextFile#AuctionContext|org.eventb.core.carrierSet#_4PKc0O5TEeSpR9iqQeSCVw" org.eventb.core.type="ℙ(ITEMS)"/>
<org.eventb.core.scCarrierSet name="USERS" org.eventb.core.source="/COMP1216/AuctionContext.buc|org.eventb.core.contextFile#AuctionContext|org.eventb.core.carrierSet#_w4LsYO5MEeSpR9iqQeSCVw" org.eventb.core.type="ℙ(USERS)"/>
</org.eventb.core.scContextFile>"#;
    assert_eq!(actual, expected);
}
