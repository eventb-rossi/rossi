//! Event-scoped proof obligations: the per-event hypothesis chain and
//! guard well-definedness, with inherited-guard suppression.

mod common;
use common::{find, generate, xml};

const M0: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_ia" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_evt" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt">
<org.eventb.core.parameter name="_p" org.eventb.core.identifier="p"/>
<org.eventb.core.guard name="_g1" org.eventb.core.label="grd1" org.eventb.core.predicate="p &gt; 0"/>
<org.eventb.core.guard name="_g2" org.eventb.core.label="grd2" org.eventb.core.predicate="10 ÷ p &gt; x"/>
<org.eventb.core.action name="_ea" org.eventb.core.assignment="x ≔ x + p" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;

#[test]
fn guard_wd_with_event_hypothesis_chain() {
    let files = generate("prj", vec![xml("M0.bum", M0)]);
    let contents = &find(&files, "M0.bpo").contents;

    for needle in [
        // grd2 guards its division; grd1 is total and typing-free, so
        // only grd2 produces an obligation, hypothesizing grd1.
        r#"<org.eventb.core.poSequent name="evt/grd2/WD" org.eventb.core.accurate="true" org.eventb.core.poDesc="Well-definedness of Guard" org.eventb.core.poStamp="0">"#,
        r#"org.eventb.core.predicate="p≠0" org.eventb.core.source="/prj/M0.bum|org.eventb.core.machineFile#M0|org.eventb.core.event#_evt|org.eventb.core.guard#_g2""#,
        // The event's identifier set carries the parameter and the
        // primed after-value of the assigned variable, and chains on
        // the machine's full hypothesis.
        r#"<org.eventb.core.poIdentifier name="p" org.eventb.core.type="ℤ"/>"#,
        r#"<org.eventb.core.poIdentifier name="x'" org.eventb.core.type="ℤ"/>"#,
    ] {
        assert!(
            contents.contains(needle),
            "missing {needle} in:\n{contents}"
        );
    }
    assert!(!contents.contains(r#"name="evt/grd1/WD""#));
}

#[test]
fn event_chain_set_names_follow_the_checked_event_identity() {
    let files = generate("prj", vec![xml("M0.bum", M0)]);
    let contents = &find(&files, "M0.bpo").contents;

    // The scEvent internal names continue the file counter past the
    // variable "x": INITIALISATION → "y", evt → "z". Guard rows name
    // from the event's own counter, so grd1 → "'".
    for needle in [
        r#"<org.eventb.core.poPredicateSet name="EVTIDENTy" org.eventb.core.parentSet="/prj/M0.bpo|org.eventb.core.poFile#M0|org.eventb.core.poPredicateSet#CTXHYP" org.eventb.core.poStamp="0">"#,
        r#"<org.eventb.core.poPredicateSet name="EVTALLHYPy" org.eventb.core.parentSet="/prj/M0.bpo|org.eventb.core.poFile#M0|org.eventb.core.poPredicateSet#EVTIDENTy" org.eventb.core.poStamp="0"/>"#,
        r#"<org.eventb.core.poPredicateSet name="EVTIDENTz" org.eventb.core.parentSet="/prj/M0.bpo|org.eventb.core.poFile#M0|org.eventb.core.poPredicateSet#ALLHYP" org.eventb.core.poStamp="0">"#,
        // grd2's obligation cuts after grd1.
        r#"<org.eventb.core.poPredicateSet name="EVTHYPz'" org.eventb.core.parentSet="/prj/M0.bpo|org.eventb.core.poFile#M0|org.eventb.core.poPredicateSet#EVTIDENTz" org.eventb.core.poStamp="0">"#,
        r#"<org.eventb.core.poPredicateSet name="EVTALLHYPz" org.eventb.core.parentSet="/prj/M0.bpo|org.eventb.core.poFile#M0|org.eventb.core.poPredicateSet#EVTHYPz'" org.eventb.core.poStamp="0">"#,
        r#"<org.eventb.core.poPredicate name="PRD0" org.eventb.core.predicate="p&gt;0""#,
        r#"<org.eventb.core.poPredicate name="PRD1" org.eventb.core.predicate="10 ÷ p&gt;x""#,
    ] {
        assert!(
            contents.contains(needle),
            "missing {needle} in:\n{contents}"
        );
    }
}

#[test]
fn inherited_guards_are_not_reproved() {
    let m1 = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.refinesMachine name="_r" org.eventb.core.target="M0"/>
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="INITIALISATION"/>
<org.eventb.core.event name="_evt" org.eventb.core.convergence="0" org.eventb.core.extended="true" org.eventb.core.label="evt">
<org.eventb.core.guard name="_h1" org.eventb.core.label="grd3" org.eventb.core.predicate="20 ÷ p &gt; x"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;
    let files = generate("prj", vec![xml("M0.bum", M0), xml("M1.bum", m1)]);
    let contents = &find(&files, "M1.bpo").contents;

    // grd2 is inherited unchanged at a compatible position: already
    // proved in the abstraction. The new grd3 still needs its guard.
    assert!(
        !contents.contains(r#"name="evt/grd2/WD""#),
        "inherited guard must not be re-proved:\n{contents}"
    );
    assert!(
        contents.contains(r#"<org.eventb.core.poSequent name="evt/grd3/WD""#),
        "own guard needs an obligation:\n{contents}"
    );
    // Its hypothesis cuts after the inherited guards.
    assert!(contents.contains(r#"org.eventb.core.predicate="p≠0""#));
}

/// A machine whose event grows a set that an invariant unions with
/// another.
const NESTED: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_s" org.eventb.core.identifier="s"/>
<org.eventb.core.variable name="_t" org.eventb.core.identifier="t"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="s ⊆ ℤ"/>
<org.eventb.core.invariant name="_i2" org.eventb.core.label="inv2" org.eventb.core.predicate="t ⊆ ℤ"/>
<org.eventb.core.invariant name="_i3" org.eventb.core.label="inv3" org.eventb.core.predicate="s ∪ t ⊆ ℕ"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_ia" org.eventb.core.assignment="s, t ≔ ∅, ∅" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_evt" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt">
<org.eventb.core.parameter name="_p" org.eventb.core.identifier="p"/>
<org.eventb.core.guard name="_g1" org.eventb.core.label="grd1" org.eventb.core.predicate="p ∈ ℕ"/>
<org.eventb.core.action name="_ea" org.eventb.core.assignment="s ≔ s ∪ {p}" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;

#[test]
fn substituted_goal_keeps_a_nested_union_parenthesized() {
    let files = generate("prj", vec![xml("M0.bum", NESTED)]);
    let contents = &find(&files, "M0.bpo").contents;

    // Substituting `s ∪ {p}` for `s` in `s ∪ t ⊆ ℕ` nests a union under a
    // union. Rodin keeps that node and parenthesizes it, so the goal must
    // read the same way: a flat `s∪{p}∪t` re-stamps the sequent.
    assert!(
        contents.contains(r#"org.eventb.core.predicate="(s∪{p})∪t⊆ℕ""#),
        "goal of evt/inv3/INV in:\n{contents}"
    );
}
