//! Reconciling regenerated `.bpo` / `.bps` pairs with previous output:
//! stamp carry-forward, status-row preservation, and byte-stable
//! rebuilds of unchanged models.

mod common;

use rossi_build::po_view::PoView;
use rossi_build::pog::reconcile::{
    HASH_ORDERED, reconcile_build_files, reconcile_pair, reset_stale_statuses,
};
use rossi_build::{Project, ScFile};

/// Build a one-machine project and return its `(bpo, bps)` contents.
fn generate(machine: &str) -> (String, String) {
    let files = common::generate("prj", vec![common::xml("M0.bum", machine)]);
    (
        common::find(&files, "M0.bpo").contents.clone(),
        common::find(&files, "M0.bps").contents.clone(),
    )
}

/// One variable, one invariant, INITIALISATION, and `extra` further
/// events. The guard of `evt` is a parameter so a single-event change
/// can be isolated.
fn machine(guard: &str, extra_events: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_a" org.eventb.core.identifier="a"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="a ≥ 0"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_ia" org.eventb.core.assignment="a ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_evt" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt">
<org.eventb.core.guard name="_g1" org.eventb.core.label="grd1" org.eventb.core.predicate="{guard}"/>
<org.eventb.core.action name="_ea" org.eventb.core.assignment="a ≔ a + 1" org.eventb.core.label="act1"/>
</org.eventb.core.event>
{extra_events}
</org.eventb.core.machineFile>"#
    )
}

const EVT2: &str = r#"<org.eventb.core.event name="_evt2" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt2">
<org.eventb.core.guard name="_g1" org.eventb.core.label="grd1" org.eventb.core.predicate="a &gt; 5"/>
<org.eventb.core.action name="_ea" org.eventb.core.assignment="a ≔ a − 1" org.eventb.core.label="act1"/>
</org.eventb.core.event>"#;

fn base() -> (String, String) {
    generate(&machine("a &lt; 10", ""))
}

/// Two variables and an event with two parameters assigning both, so
/// the machine's identifier set and the event's (parameters and primed
/// variables) each hold more than one name.
fn two_variable_machine(guard: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_a" org.eventb.core.identifier="a"/>
<org.eventb.core.variable name="_b" org.eventb.core.identifier="b"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="a ≥ 0"/>
<org.eventb.core.invariant name="_i2" org.eventb.core.label="inv2" org.eventb.core.predicate="b ≥ 0"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_ia" org.eventb.core.assignment="a, b ≔ 0, 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_evt" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt">
<org.eventb.core.parameter name="_p" org.eventb.core.identifier="p"/>
<org.eventb.core.parameter name="_q" org.eventb.core.identifier="q"/>
<org.eventb.core.guard name="_g1" org.eventb.core.label="grd1" org.eventb.core.predicate="{guard}"/>
<org.eventb.core.guard name="_g2" org.eventb.core.label="grd2" org.eventb.core.predicate="q &gt; 0"/>
<org.eventb.core.action name="_ea" org.eventb.core.assignment="a, b ≔ a + p, b + q" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#
    )
}

/// Whether an emitted line declares one of the kinds Rodin orders by
/// hash table.
fn is_declaration(line: &str) -> bool {
    HASH_ORDERED
        .iter()
        .any(|tag| line.trim_start().starts_with(&format!("<{tag} ")))
}

/// The declared names of every run, in document order.
fn declaration_runs(xml: &str) -> Vec<Vec<String>> {
    let mut runs = Vec::new();
    let mut run = Vec::new();
    for line in xml.lines() {
        if is_declaration(line) {
            let rest = &line[line.find(" name=\"").unwrap() + 7..];
            run.push(rest[..rest.find('"').unwrap()].to_string());
        } else if !run.is_empty() {
            runs.push(std::mem::take(&mut run));
        }
    }
    runs
}

/// What Rodin would have written: every run reversed, and every line
/// indented, since that is how its writer lays a file out.
fn as_rodin_wrote_it(xml: &str) -> String {
    let mut lines: Vec<String> = xml.lines().map(|line| format!("    {line}")).collect();
    for run in lines.chunk_by_mut(|a, b| is_declaration(a) && is_declaration(b)) {
        if is_declaration(&run[0]) {
            run.reverse();
        }
    }
    lines.join("\n") + "\n"
}

#[test]
fn no_previous_state_leaves_output_untouched() {
    let (bpo, bps) = base();
    let (bpo_out, bps_out) = reconcile_pair(None, None, &bpo, &bps);
    assert_eq!(bpo_out, bpo);
    assert_eq!(bps_out, bps);
}

#[test]
fn unchanged_model_passes_previous_bytes_through() {
    let (bpo, bps) = base();
    // The previous files differ cosmetically: non-zero stamps, spaced
    // predicate spelling, and a discharged status row.
    let old_bpo = bpo
        .replacen(r#"poStamp="0""#, r#"poStamp="5""#, 1)
        .replace("a≥0", "a ≥ 0");
    assert_ne!(old_bpo, bpo, "the cosmetic edit must apply");
    let old_bps = bps.replacen(
        r#"org.eventb.core.confidence="-99""#,
        r#"org.eventb.core.confidence="1000""#,
        1,
    );

    let (bpo_out, bps_out) = reconcile_pair(Some(&old_bpo), Some(&old_bps), &bpo, &bps);
    assert_eq!(bpo_out, old_bpo, "unchanged obligations keep the old bytes");
    assert_eq!(bps_out, old_bps, "unchanged statuses keep the old bytes");
}

#[test]
fn changed_guard_keeps_previous_identifier_order() {
    // The previous file lists every identifier set the other way round,
    // as Rodin's hash tables might. The changed guard makes the file
    // regenerate rather than pass through, and the regenerated sets
    // keep the previous order while the stamps move as before.
    let (old_bpo, old_bps) = generate(&two_variable_machine("p &gt; 0"));
    let old_bpo = as_rodin_wrote_it(&old_bpo);
    let (new_bpo, new_bps) = generate(&two_variable_machine("p &gt; 1"));
    assert_ne!(declaration_runs(&old_bpo), declaration_runs(&new_bpo));

    let (bpo_out, _) = reconcile_pair(Some(&old_bpo), Some(&old_bps), &new_bpo, &new_bps);

    assert_eq!(declaration_runs(&bpo_out), declaration_runs(&old_bpo));
    assert!(
        !bpo_out.contains("    <"),
        "Rodin's indentation must not leak in"
    );
    let view = PoView::from_xml(&bpo_out).unwrap();
    assert_eq!(view.stamp.as_deref(), Some("1"));
    assert_eq!(view.sequents["evt/inv1/INV"].stamp.as_deref(), Some("1"));
    assert_eq!(
        view.sequents["INITIALISATION/inv1/INV"].stamp.as_deref(),
        Some("0")
    );
}

#[test]
fn checked_files_follow_previous_order() {
    // Rodin's static checker orders carrier sets and constants (one
    // mixed sequence), variables and event parameters by hash table;
    // rossi sorts them. Regenerating over Rodin's files keeps its order
    // in the context, the machine's copy of it, the variables and each
    // event's parameters.
    let context = common::xml(
        "C.buc",
        r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.carrierSet name="_S" org.eventb.core.identifier="S"/>
<org.eventb.core.carrierSet name="_T" org.eventb.core.identifier="T"/>
<org.eventb.core.constant name="_c" org.eventb.core.identifier="c"/>
<org.eventb.core.axiom name="_a1" org.eventb.core.label="axm1" org.eventb.core.predicate="c ∈ S"/>
</org.eventb.core.contextFile>"#,
    );
    let machine = common::xml(
        "M.bum",
        r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_sees" org.eventb.core.target="C"/>
<org.eventb.core.variable name="_a" org.eventb.core.identifier="a"/>
<org.eventb.core.variable name="_b" org.eventb.core.identifier="b"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="a ∈ S"/>
<org.eventb.core.invariant name="_i2" org.eventb.core.label="inv2" org.eventb.core.predicate="b ∈ T"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_ia" org.eventb.core.assignment="a :∈ S" org.eventb.core.label="act1"/>
<org.eventb.core.action name="_ib" org.eventb.core.assignment="b :∈ T" org.eventb.core.label="act2"/>
</org.eventb.core.event>
<org.eventb.core.event name="_evt" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="evt">
<org.eventb.core.parameter name="_p" org.eventb.core.identifier="p"/>
<org.eventb.core.parameter name="_q" org.eventb.core.identifier="q"/>
<org.eventb.core.guard name="_g1" org.eventb.core.label="grd1" org.eventb.core.predicate="p ∈ S ∧ q ∈ T"/>
<org.eventb.core.action name="_ea" org.eventb.core.assignment="a, b ≔ p, q" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#,
    );
    let result = rossi_build::build(&Project::new("prj", vec![context, machine]));
    assert!(result.is_ok(), "diagnostics: {:?}", result.diagnostics);
    let mut files = result.files;
    let previous: std::collections::HashMap<String, String> = files
        .iter()
        .filter(|file| file.filename.ends_with(".bcc") || file.filename.ends_with(".bcm"))
        .map(|file| (file.filename.clone(), as_rodin_wrote_it(&file.contents)))
        .collect();
    // Three runs in the machine (the copied context, the variables, the
    // event's parameters) and one in the context, all of more than one
    // name, so reversing them is a real reorder.
    assert_eq!(declaration_runs(&previous["M.bcm"]).len(), 3);
    assert!(
        declaration_runs(&previous["M.bcm"])
            .iter()
            .all(|run| run.len() > 1)
    );

    reconcile_build_files(&mut files, |name| previous.get(name).cloned());

    for name in ["C.bcc", "M.bcm"] {
        let out = &common::find(&files, name).contents;
        assert_eq!(
            declaration_runs(out),
            declaration_runs(&previous[name]),
            "{name}"
        );
        assert!(
            !out.contains("    <"),
            "{name}: Rodin's indentation must not leak in"
        );
    }
}

#[test]
fn changed_guard_bumps_only_the_affected_sequent() {
    let (old_bpo, old_bps) = base();
    let (new_bpo, new_bps) = generate(&machine("a &lt; 9", ""));
    // Doctor the previous statuses: both rows carry proof results, one
    // of them manual, plus an attribute the generator never writes.
    let old_bps = old_bps
        .replace(
            r#"org.eventb.core.confidence="-99" org.eventb.core.poStamp="0" org.eventb.core.psManual="false""#,
            r#"org.eventb.core.confidence="1000" org.eventb.core.poStamp="0" org.eventb.core.psManual="true" org.eventb.core.psBroken="false""#,
        );

    let (bpo_out, bps_out) = reconcile_pair(Some(&old_bpo), Some(&old_bps), &new_bpo, &new_bps);
    let view = PoView::from_xml(&bpo_out).unwrap();

    // The file stamp and the changed sequent move to 1; the untouched
    // sequent keeps its old stamp.
    assert_eq!(view.stamp.as_deref(), Some("1"));
    assert_eq!(view.sequents["evt/inv1/INV"].stamp.as_deref(), Some("1"));
    assert_eq!(
        view.sequents["INITIALISATION/inv1/INV"].stamp.as_deref(),
        Some("0")
    );

    // The guard lives in the event's hypothesis sets: those bump, the
    // machine-level sets don't.
    for name in ["CTXHYP", "ABSHYP", "ALLHYP"] {
        assert_eq!(view.sets[name].stamp.as_deref(), Some("0"), "{name}");
    }
    let bumped: Vec<&str> = view
        .sets
        .iter()
        .filter(|(_, set)| set.stamp.as_deref() == Some("1"))
        .map(|(name, _)| name.as_str())
        .collect();
    assert!(
        !bumped.is_empty() && bumped.iter().all(|name| name.starts_with("EVT")),
        "changed sets: {bumped:?}"
    );

    // Both status rows carry over byte-verbatim — the changed
    // sequent's row keeps its old stamp, now differing from the
    // sequent's, which is the stale marker downstream provers use.
    assert!(bps_out.contains(
        r#"<org.eventb.core.psStatus name="evt/inv1/INV" org.eventb.core.confidence="1000" org.eventb.core.poStamp="0" org.eventb.core.psManual="true" org.eventb.core.psBroken="false"/>"#
    ));
    assert!(bps_out.contains(
        r#"<org.eventb.core.psStatus name="INITIALISATION/inv1/INV" org.eventb.core.confidence="1000" org.eventb.core.poStamp="0" org.eventb.core.psManual="true" org.eventb.core.psBroken="false"/>"#
    ));
}

#[test]
fn added_obligation_gets_a_fresh_unattempted_row() {
    let (old_bpo, old_bps) = base();
    let old_bps = old_bps.replace(
        r#"org.eventb.core.confidence="-99""#,
        r#"org.eventb.core.confidence="1000""#,
    );
    let (new_bpo, new_bps) = generate(&machine("a &lt; 10", EVT2));

    let (bpo_out, bps_out) = reconcile_pair(Some(&old_bpo), Some(&old_bps), &new_bpo, &new_bps);
    let view = PoView::from_xml(&bpo_out).unwrap();

    // Existing sequents are untouched by the added event.
    assert_eq!(view.sequents["evt/inv1/INV"].stamp.as_deref(), Some("0"));
    assert_eq!(
        view.sequents["INITIALISATION/inv1/INV"].stamp.as_deref(),
        Some("0")
    );
    // The new sequent and the file get the fresh stamp; its status row
    // is fresh and unattempted with the same stamp.
    assert_eq!(view.sequents["evt2/inv1/INV"].stamp.as_deref(), Some("1"));
    assert_eq!(view.stamp.as_deref(), Some("1"));
    assert!(bps_out.contains(
        r#"<org.eventb.core.psStatus name="evt2/inv1/INV" org.eventb.core.confidence="-99" org.eventb.core.poStamp="1" org.eventb.core.psManual="false"/>"#
    ));
    // The carried rows keep their doctored confidence.
    assert!(bps_out.contains(r#"name="evt/inv1/INV" org.eventb.core.confidence="1000""#));
}

#[test]
fn removed_obligation_drops_its_row() {
    let (old_bpo, old_bps) = generate(&machine("a &lt; 10", EVT2));
    let old_bps = old_bps.replace(
        r#"org.eventb.core.confidence="-99""#,
        r#"org.eventb.core.confidence="1000""#,
    );
    let (new_bpo, new_bps) = base();

    let (bpo_out, bps_out) = reconcile_pair(Some(&old_bpo), Some(&old_bps), &new_bpo, &new_bps);
    let view = PoView::from_xml(&bpo_out).unwrap();

    assert!(!view.sequents.contains_key("evt2/inv1/INV"));
    assert!(!bps_out.contains("evt2/inv1/INV"), "vanished row must drop");
    // The surviving rows carry over with their results.
    assert!(bps_out.contains(r#"name="evt/inv1/INV" org.eventb.core.confidence="1000""#));
    assert!(
        bps_out.contains(r#"name="INITIALISATION/inv1/INV" org.eventb.core.confidence="1000""#)
    );
}

#[test]
fn unparseable_previous_file_is_ignored() {
    let (bpo, bps) = base();
    // A predicate that no longer parses: the old file contributes no
    // stamps, but statuses still carry by name.
    let old_bpo = bpo.replace("a≥0", "a≥0∧(");
    let old_bps = bps.replace(
        r#"org.eventb.core.confidence="-99""#,
        r#"org.eventb.core.confidence="1000""#,
    );

    let (bpo_out, bps_out) = reconcile_pair(Some(&old_bpo), Some(&old_bps), &bpo, &bps);
    assert_eq!(bpo_out, bpo, "no stamp carry from an unreadable file");
    assert!(bps_out.contains(r#"org.eventb.core.confidence="1000""#));
}

#[test]
fn stub_output_passes_previous_files_through() {
    let (old_bpo, old_bps) = base();
    let stub_bpo = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n<org.eventb.core.poFile org.eventb.core.poStamp=\"0\"/>\n";
    let stub_bps =
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n<org.eventb.core.psFile/>\n";

    let (bpo_out, bps_out) = reconcile_pair(Some(&old_bpo), Some(&old_bps), stub_bpo, stub_bps);
    assert_eq!(bpo_out, old_bpo);
    assert_eq!(bps_out, old_bps);
}

#[test]
fn stub_detection_invariant_holds() {
    // The stub passthrough infers "decomposition stub" from an empty
    // view; pin the invariant it rests on from both sides, so a
    // generator change that breaks it fails here instead of silently
    // resurrecting or wiping proof state.
    let (bpo, _) = base();
    let view = PoView::from_xml(&bpo).unwrap();
    assert!(
        !view.sets.is_empty(),
        "a real component must emit its hypothesis roots"
    );

    let stub = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="ch.ethz.eventb.decomposition.mchBase">
<org.eventb.core.variable name="_a" org.eventb.core.identifier="a"/>
</org.eventb.core.machineFile>"#;
    let (bpo, _) = generate(stub);
    let view = PoView::from_xml(&bpo).unwrap();
    assert!(
        view.sets.is_empty() && view.sequents.is_empty(),
        "a decomposition stub must emit an empty file: {bpo}"
    );
}

#[test]
fn fresh_stamp_exceeds_every_previous_stamp() {
    let (old_bpo, _) = base();
    // A previous file whose element stamp runs ahead of its file
    // stamp: the fresh stamp must clear both, or a carried stamp could
    // collide with a fresh one and revalidate a stale proof.
    let old_bpo = old_bpo
        .replacen(r#"poStamp="0""#, r#"poStamp="2""#, 1)
        .replace(
            r#"name="evt/inv1/INV" org.eventb.core.accurate="true" org.eventb.core.poDesc="Invariant  preservation" org.eventb.core.poStamp="0""#,
            r#"name="evt/inv1/INV" org.eventb.core.accurate="true" org.eventb.core.poDesc="Invariant  preservation" org.eventb.core.poStamp="7""#,
        );
    let (new_bpo, new_bps) = generate(&machine("a &lt; 9", ""));

    let (bpo_out, _) = reconcile_pair(Some(&old_bpo), None, &new_bpo, &new_bps);
    let view = PoView::from_xml(&bpo_out).unwrap();
    assert_eq!(view.stamp.as_deref(), Some("8"));
    assert_eq!(view.sequents["evt/inv1/INV"].stamp.as_deref(), Some("8"));
}

/// Wrap one component's `(bpo, bps)` contents as the build-file slice
/// [`reset_stale_statuses`] operates on.
fn pair_files(bpo: String, bps: String) -> Vec<ScFile> {
    vec![
        ScFile {
            filename: "M0.bpo".into(),
            contents: bpo,
            accurate: true,
        },
        ScFile {
            filename: "M0.bps".into(),
            contents: bps,
            accurate: true,
        },
    ]
}

#[test]
fn guard_keeps_stamp_matched_rows_including_broken() {
    let (bpo, bps) = base();
    // Both rows stamp-match their sequents (all stamps "0"): one
    // discharged, one discharged-but-broken.
    let bps = bps
        .replace(
            r#"name="INITIALISATION/inv1/INV" org.eventb.core.confidence="-99""#,
            r#"name="INITIALISATION/inv1/INV" org.eventb.core.confidence="1000""#,
        )
        .replace(
            r#"name="evt/inv1/INV" org.eventb.core.confidence="-99" org.eventb.core.poStamp="0" org.eventb.core.psManual="false""#,
            r#"name="evt/inv1/INV" org.eventb.core.confidence="1000" org.eventb.core.poStamp="0" org.eventb.core.psManual="false" org.eventb.core.psBroken="true""#,
        );
    let mut files = pair_files(bpo, bps.clone());

    let counts = reset_stale_statuses(&mut files);

    assert_eq!(files[1].contents, bps, "stamp-matched rows carry verbatim");
    // Two sequents; only the non-broken discharged row is skipped — the
    // broken one stays open (the gate fails it and the disprover runs).
    assert_eq!(counts, vec![("M0.bpo".to_string(), 1)]);
}

#[test]
fn guard_resets_stamp_mismatched_rows() {
    let (old_bpo, old_bps) = base();
    let old_bps = old_bps.replace(
        r#"org.eventb.core.confidence="-99""#,
        r#"org.eventb.core.confidence="1000""#,
    );
    // The changed guard bumps evt/inv1/INV to stamp "1" while its
    // carried row keeps stamp "0" — the divergence the guard resets.
    let (new_bpo, new_bps) = generate(&machine("a &lt; 9", ""));
    let (bpo_out, bps_out) = reconcile_pair(Some(&old_bpo), Some(&old_bps), &new_bpo, &new_bps);
    let mut files = pair_files(bpo_out, bps_out);

    let counts = reset_stale_statuses(&mut files);

    assert!(
        files[1].contents.contains(
            r#"<org.eventb.core.psStatus name="evt/inv1/INV" org.eventb.core.confidence="-99" org.eventb.core.poStamp="1" org.eventb.core.psManual="false"/>"#
        ),
        "the stale row resets to a fresh unattempted one carrying the sequent's stamp: {}",
        files[1].contents
    );
    assert!(
        files[1]
            .contents
            .contains(r#"name="INITIALISATION/inv1/INV" org.eventb.core.confidence="1000""#),
        "the untouched sequent's row survives"
    );
    assert_eq!(counts, vec![("M0.bpo".to_string(), 1)]);
}

#[test]
fn guard_is_noop_without_previous_state() {
    let (bpo, bps) = base();
    let mut files = pair_files(bpo, bps.clone());

    let counts = reset_stale_statuses(&mut files);

    assert_eq!(files[1].contents, bps, "fresh output is untouched");
    assert_eq!(counts, vec![("M0.bpo".to_string(), 2)]);
}

#[test]
fn guard_counts_reviewed_and_pending_as_open() {
    let (bpo, bps) = base();
    let bps = bps
        .replace(
            r#"name="INITIALISATION/inv1/INV" org.eventb.core.confidence="-99""#,
            r#"name="INITIALISATION/inv1/INV" org.eventb.core.confidence="300""#,
        )
        .replace(
            r#"name="evt/inv1/INV" org.eventb.core.confidence="-99""#,
            r#"name="evt/inv1/INV" org.eventb.core.confidence="50""#,
        );
    let mut files = pair_files(bpo, bps.clone());

    let counts = reset_stale_statuses(&mut files);

    assert_eq!(files[1].contents, bps, "stamp-matched rows carry verbatim");
    // Reviewed (300) and pending (50) both stay open — only discharged
    // (> 500) rows are skipped, matching the po gate.
    assert_eq!(counts, vec![("M0.bpo".to_string(), 2)]);
}
