//! M3: a machine that SEES a context whose ancestors form a diamond must
//! emit scInternalContext for every transitively-seen context, each
//! appearing exactly once.
//!
//! Layout:
//!   Base  (sets USERS)
//!   Left  extends Base  (sets LEFT_ONLY)
//!   Right extends Base  (sets RIGHT_ONLY)
//!   Top   extends Left, Right
//!   Mch   sees Top
//!
//! Expected: Mch.bcm has scInternalContext for {Base, Left, Right, Top},
//! and Base appears exactly once (not once per path).

use rossi_build::{Project, ProjectComponent, build, sc_view::ScView};

fn ctx(filename: &str, body: &str) -> ProjectComponent {
    let xml = format!(
        r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
{body}
</org.eventb.core.contextFile>"#
    );
    ProjectComponent::from_xml(filename, &xml).unwrap()
}

fn make_project() -> Project {
    let base = ctx(
        "Base.buc",
        r#"<org.eventb.core.carrierSet name="_u" org.eventb.core.identifier="USERS"/>"#,
    );
    let left = ctx(
        "Left.buc",
        r#"<org.eventb.core.extendsContext name="_e1" org.eventb.core.target="Base"/>
<org.eventb.core.carrierSet name="_l" org.eventb.core.identifier="LEFT_ONLY"/>"#,
    );
    let right = ctx(
        "Right.buc",
        r#"<org.eventb.core.extendsContext name="_e2" org.eventb.core.target="Base"/>
<org.eventb.core.carrierSet name="_r" org.eventb.core.identifier="RIGHT_ONLY"/>"#,
    );
    let top = ctx(
        "Top.buc",
        r#"<org.eventb.core.extendsContext name="_e3" org.eventb.core.target="Left"/>
<org.eventb.core.extendsContext name="_e4" org.eventb.core.target="Right"/>"#,
    );
    let mch = ProjectComponent::from_xml(
        "Mch.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_s" org.eventb.core.target="Top"/>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
    Project::new("m3", vec![base, left, right, top, mch])
}

#[test]
fn machine_emits_internal_context_for_all_ancestors() {
    let r = build(&make_project());
    let bcm = &r.file("Mch.bcm").expect("Mch.bcm").contents;
    for name in ["Base", "Left", "Right", "Top"] {
        let marker = format!("<org.eventb.core.scInternalContext name=\"{name}\"");
        assert!(
            bcm.contains(&marker),
            "expected {marker} in machine .bcm:\n{bcm}"
        );
    }
}

#[test]
fn sees_context_target_is_bare_uri() {
    // Rodin emits `scSeesContext.scTarget="/PROJECT/CTX.bcc"` without a
    // `|org.eventb.core.scContextFile#NAME` fragment; ProB rejects the
    // fragmented form. See docs/ANIMATE_FAILURES.md (RC1).
    let r = build(&make_project());
    let bcm = &r.file("Mch.bcm").expect("Mch.bcm").contents;
    assert!(
        bcm.contains("org.eventb.core.scTarget=\"/m3/Top.bcc\""),
        "expected bare scTarget for sees:\n{bcm}"
    );
    assert!(
        !bcm.contains("/m3/Top.bcc|org.eventb.core.scContextFile#"),
        "scSeesContext.scTarget must not carry a scContextFile fragment:\n{bcm}"
    );
}

#[test]
fn diamond_base_appears_exactly_once() {
    let r = build(&make_project());
    let bcm = &r.file("Mch.bcm").expect("Mch.bcm").contents;
    let marker = "<org.eventb.core.scInternalContext name=\"Base\"";
    let count = bcm.matches(marker).count();
    assert_eq!(
        count, 1,
        "Base should appear exactly once, found {count} times in:\n{bcm}"
    );
}

#[test]
fn variables_can_reference_carrier_sets_from_any_ancestor() {
    // With a diamond SEES, ancestor-defined sets must be visible for
    // inference on invariants / guards / variables in the machine.
    // We assert this indirectly: compile should succeed without
    // "unknown identifier" diagnostics.
    let ctx_extra = ctx(
        "Base.buc",
        r#"<org.eventb.core.carrierSet name="_u" org.eventb.core.identifier="USERS"/>"#,
    );
    let left = ctx(
        "Left.buc",
        r#"<org.eventb.core.extendsContext name="_e1" org.eventb.core.target="Base"/>
<org.eventb.core.carrierSet name="_l" org.eventb.core.identifier="ITEMS"/>"#,
    );
    let right = ctx(
        "Right.buc",
        r#"<org.eventb.core.extendsContext name="_e2" org.eventb.core.target="Base"/>
<org.eventb.core.carrierSet name="_r" org.eventb.core.identifier="AUCTIONS"/>"#,
    );
    let top = ctx(
        "Top.buc",
        r#"<org.eventb.core.extendsContext name="_e3" org.eventb.core.target="Left"/>
<org.eventb.core.extendsContext name="_e4" org.eventb.core.target="Right"/>"#,
    );
    let mch = ProjectComponent::from_xml(
        "Mch.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_s" org.eventb.core.target="Top"/>
<org.eventb.core.variable name="_v1" org.eventb.core.identifier="registered"/>
<org.eventb.core.variable name="_v2" org.eventb.core.identifier="inventory"/>
<org.eventb.core.variable name="_v3" org.eventb.core.identifier="sales"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="registered ⊆ USERS"/>
<org.eventb.core.invariant name="_i2" org.eventb.core.label="inv2" org.eventb.core.predicate="inventory ⊆ ITEMS"/>
<org.eventb.core.invariant name="_i3" org.eventb.core.label="inv3" org.eventb.core.predicate="sales ⊆ AUCTIONS"/>
</org.eventb.core.machineFile>"#,
    )
    .unwrap();
    let project = Project::new("m3_diamond", vec![ctx_extra, left, right, top, mch]);
    let r = build(&project);
    assert!(r.is_ok(), "diagnostics: {:?}", r.diagnostics);
    let view = ScView::from_xml(&r.file("Mch.bcm").unwrap().contents).unwrap();
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
    assert_eq!(
        view.variables.get("sales").map(|v| v.type_str.as_str()),
        Some("ℙ(AUCTIONS)")
    );
}
