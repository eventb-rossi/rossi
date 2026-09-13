//! `rossi build`: diagnostics, output writing, and output-path containment.

use crate::helpers::{
    ASCII_CONTEXT, BuildFixture, DUP_VARIABLE_MACHINE, dir_has_ext, extract_zip_to,
    rewrite_zip_entry, rossi_command, tempdir_unique, zip_entry_bytes, zip_entry_names,
};

#[test]
fn build_duplicate_component_names_fail_with_eb019() {
    // `rossi build` must fail the same project `validate` fails: the EB019
    // diagnostic, exit 1, and no output written — not a zip-writer IO error
    // about colliding entry names.
    let tmp = tempdir_unique("rossi-cli-build-dup-names");
    std::fs::write(tmp.join("a.eventb"), "MACHINE M\nEND\n").unwrap();
    std::fs::write(tmp.join("b.eventb"), "MACHINE M\nEND\n").unwrap();
    let out_zip = tmp.join("out.zip");

    let output = rossi_command()
        .args([
            "build",
            tmp.to_str().unwrap(),
            "--output",
            out_zip.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");

    assert!(
        !output.status.success(),
        "duplicate component names must fail the build; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[EB019]"), "stderr: {stderr}");
    assert!(
        !stderr.contains("Duplicate filename"),
        "the zip-writer error must be unreachable: {stderr}"
    );
    assert!(!out_zip.exists(), "no output may be written");

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn build_fails_on_error_diagnostics_but_still_writes_output() {
    // An error diagnostic that still leaves checked output (here EB006: a
    // constant with no typing axiom) must fail the build, while the filtered
    // output is written all the same — matching Rodin, which drops the
    // erroneous element and still produces the checked file.
    let tmp = tempdir_unique("rossi-cli-build-error-diag");
    let src = tmp.join("c.eventb");
    std::fs::write(
        &src,
        "CONTEXT c\nCONSTANTS\n    x\nAXIOMS\n    @axm1 1 = 1\nEND\n",
    )
    .unwrap();
    let out_zip = tmp.join("out.zip");

    let output = rossi_command()
        .args([
            "build",
            src.to_str().unwrap(),
            "-o",
            out_zip.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");

    assert!(
        !output.status.success(),
        "error diagnostics must fail the build; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[EB006]"), "stderr: {stderr}");
    assert!(stderr.contains("error diagnostic"), "stderr: {stderr}");
    assert!(out_zip.exists(), "filtered output must still be written");
    let extracted = tmp.join("extracted");
    std::fs::create_dir_all(&extracted).unwrap();
    extract_zip_to(&out_zip, &extracted);
    assert!(
        dir_has_ext(&extracted, &["bcc"]),
        "expected the checked output (.bcc) despite the error"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn build_fails_on_circular_refines_with_context_sibling() {
    // Regression: a REFINES cycle beside a healthy context used to exit 0
    // because the context's checked file made the project look successful.
    // The cycle's EB008 error must fail the build while the context's output
    // is still written.
    let tmp = tempdir_unique("rossi-cli-build-refines-cycle");
    let src = tmp.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("m.eventb"), "MACHINE m\nREFINES m\nEND\n").unwrap();
    std::fs::write(src.join("c.eventb"), ASCII_CONTEXT).unwrap();
    let out_zip = tmp.join("out.zip");

    let output = rossi_command()
        .args([
            "build",
            src.to_str().unwrap(),
            "-o",
            out_zip.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");

    assert!(
        !output.status.success(),
        "a dependency cycle must fail the build; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[EB008]"), "stderr: {stderr}");
    assert!(
        out_zip.exists(),
        "the context's output must still be written"
    );
    let extracted = tmp.join("extracted");
    std::fs::create_dir_all(&extracted).unwrap();
    extract_zip_to(&out_zip, &extracted);
    assert!(
        dir_has_ext(&extracted, &["bcc"]),
        "expected the sibling context's .bcc in the built zip"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn build_fails_on_duplicate_identifier() {
    let tmp = tempdir_unique("rossi-cli-build-dup-var");
    std::fs::write(tmp.join("M.eventb"), DUP_VARIABLE_MACHINE).unwrap();
    let out_zip = tmp.join("out.zip");

    let output = rossi_command()
        .args([
            "build",
            tmp.to_str().unwrap(),
            "-o",
            out_zip.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");

    assert!(
        !output.status.success(),
        "a duplicate identifier must fail the build; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[EB021]"), "stderr: {stderr}");
    assert!(
        out_zip.exists(),
        "the filtered output is still written despite the error"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[cfg(unix)]
fn symlink_dir(target: &std::path::Path, link: &std::path::Path) {
    std::fs::create_dir_all(target).unwrap();
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[test]
fn build_directory_output_rejects_unsafe_prefixes_before_writing() {
    let cases = [
        ("parent-prefix", ["../C.buc", "safe/D.buc"]),
        ("rooted-prefix", ["/C.buc", "safe/D.buc"]),
        ("sanitized-collision", ["../C.buc", "./D.buc"]),
    ];

    for (case, entries) in cases {
        let fixture = BuildFixture::new(&entries, "out");
        let output = fixture.run();

        assert!(!output.status.success(), "{case} should be rejected");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("unsafe archive prefix"),
            "{case}: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !fixture.root.join("C.bcc").exists(),
            "{case} escaped output root"
        );
        assert!(
            !fixture.output.exists(),
            "{case} preflight failure must not create the output root"
        );
    }
}

#[test]
fn build_directory_output_preserves_safe_multi_project_layout() {
    let fixture = BuildFixture::new(&["A/C.buc", "B/D.buc"], "out");
    fixture.assert_success("safe multi-project build");
    assert!(fixture.output.join("A/C.bcc").exists());
    assert!(fixture.output.join("B/D.bcc").exists());
}

#[test]
fn build_zip_output_preserves_raw_archive_prefixes() {
    let fixture = BuildFixture::new(&["../C.buc", "safe/D.buc"], "out.zip");
    fixture.assert_success("archive repacking");
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&fixture.output).unwrap()).unwrap();
    assert!(archive.by_name("../C.bcc").is_ok());
    assert!(archive.by_name("safe/D.bcc").is_ok());
}

#[cfg(unix)]
#[test]
fn build_directory_output_rejects_escaping_project_symlink() {
    let fixture = BuildFixture::new(&["evil/C.buc", "safe/D.buc"], "out");
    let outside = fixture.root.join("outside");
    symlink_dir(&outside, &fixture.output.join("evil"));
    let output = fixture.run();

    assert!(
        !output.status.success(),
        "escaping symlink should be rejected"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("escapes output directory"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!outside.join("C.bcc").exists());
    assert!(
        !fixture.output.join("safe/D.bcc").exists(),
        "preflight must reject before writing a safe sibling"
    );
}

#[cfg(unix)]
#[test]
fn build_directory_output_allows_contained_project_symlink() {
    let fixture = BuildFixture::new(&["linked/C.buc", "safe/D.buc"], "out");
    let actual = fixture.output.join("actual");
    symlink_dir(&actual, &fixture.output.join("linked"));
    fixture.assert_success("contained symlink");
    assert!(actual.join("C.bcc").exists());
    assert!(fixture.output.join("safe/D.bcc").exists());
}

#[cfg(unix)]
#[test]
fn build_directory_output_allows_symlinked_root() {
    let fixture = BuildFixture::new(&["A/C.buc", "B/D.buc"], "out");
    let actual = fixture.root.join("actual");
    symlink_dir(&actual, &fixture.output);
    fixture.assert_success("symlinked root");
    assert!(actual.join("A/C.bcc").exists());
    assert!(actual.join("B/D.bcc").exists());
}

#[test]
fn build_eventb_file_packs_sources_and_checked() {
    let tmp = tempdir_unique("rossi-cli-build-eventb");
    let out_zip = tmp.join("out.zip");

    let output = rossi_command()
        .args([
            "build",
            "../rossi/examples/counter.eventb",
            "-o",
            out_zip.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");

    assert!(
        output.status.success(),
        "build from text should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out_zip.exists());

    let extracted = tmp.join("extracted");
    std::fs::create_dir_all(&extracted).unwrap();
    extract_zip_to(&out_zip, &extracted);
    // The output must carry both the component source and our checked file,
    // just like the old export-then-build round-trip did.
    assert!(
        dir_has_ext(&extracted, &["buc", "bum"]),
        "expected the component source (.buc/.bum) in the built zip"
    );
    assert!(
        dir_has_ext(&extracted, &["bcc", "bcm"]),
        "expected the checked output (.bcc/.bcm) in the built zip"
    );
    // The text door stamps a `.project` descriptor, so the built zip imports
    // into Rodin (and back through `rossi import`) under its real name.
    assert!(
        extracted.join(".project").is_file(),
        "expected the .project descriptor in the built zip"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn build_eventb_directory() {
    let tmp = tempdir_unique("rossi-cli-build-dir");
    let src = tmp.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("c.eventb"), ASCII_CONTEXT).unwrap();
    let out_zip = tmp.join("out.zip");

    let output = rossi_command()
        .args([
            "build",
            src.to_str().unwrap(),
            "-o",
            out_zip.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");

    assert!(
        output.status.success(),
        "build from a text directory should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let extracted = tmp.join("extracted");
    std::fs::create_dir_all(&extracted).unwrap();
    extract_zip_to(&out_zip, &extracted);
    assert!(
        dir_has_ext(&extracted, &["bcc", "bcm"]),
        "expected the checked output (.bcc/.bcm) in the built zip"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

const TRAFFIC_LIGHT: &str = "../rossi/examples/traffic-light.zip";

fn build_zip(input: &std::path::Path, out_zip: &std::path::Path) {
    let output = rossi_command()
        .args([
            "build",
            input.to_str().unwrap(),
            "--output",
            out_zip.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn build_zip_preserves_proof_files() {
    // A rebuilt archive carries our generated .bpo (obligations) and
    // .bps (the archive's recorded discharged statuses, carried
    // at their matching stamps) while the input's .bpr proofs are
    // preserved byte-exact.
    let tmp = tempdir_unique("rossi-cli-build-proofs");
    let out_zip = tmp.join("out.zip");
    build_zip(std::path::Path::new(TRAFFIC_LIGHT), &out_zip);

    let extracted = tmp.join("out");
    extract_zip_to(&out_zip, &extracted);
    let root = extracted.join("traffic-light");
    let m0_bpo = std::fs::read_to_string(root.join("M0.bpo")).expect("M0.bpo");
    assert!(m0_bpo.contains(r#"<org.eventb.core.poSequent name="INITIALISATION/inv3/INV""#));
    let m0_bps = std::fs::read_to_string(root.join("M0.bps")).expect("M0.bps");
    assert!(m0_bps.contains(r#"<org.eventb.core.psStatus name="INITIALISATION/inv3/INV" org.eventb.core.confidence="1000" org.eventb.core.poStamp="0" org.eventb.core.psManual="false"/>"#));
    assert_eq!(
        std::fs::read(root.join("M0.bpr")).expect("M0.bpr"),
        zip_entry_bytes(std::path::Path::new(TRAFFIC_LIGHT), "traffic-light/M0.bpr"),
        "proofs must be preserved byte-exact"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn build_zip_rebuild_is_byte_stable() {
    // Rebuilding an unchanged archive must be a no-op on proof state:
    // every .bpo / .bps / .bpr entry comes out byte-identical.
    let tmp = tempdir_unique("rossi-cli-build-stable");
    let out1 = tmp.join("out1.zip");
    let out2 = tmp.join("out2.zip");
    build_zip(std::path::Path::new(TRAFFIC_LIGHT), &out1);
    build_zip(&out1, &out2);

    let proof_entries: Vec<String> = zip_entry_names(&out1)
        .into_iter()
        .filter(|n| n.ends_with(".bpo") || n.ends_with(".bps") || n.ends_with(".bpr"))
        .collect();
    assert!(!proof_entries.is_empty());
    for name in &proof_entries {
        assert_eq!(
            zip_entry_bytes(&out1, name),
            zip_entry_bytes(&out2, name),
            "{name} must survive a rebuild unchanged"
        );
    }

    std::fs::remove_dir_all(&tmp).ok();
}

/// The doctored `INITIALISATION/inv3/INV` status row the carry tests
/// look for after a rebuild.
const DISCHARGED_INV3: &str = r#"name="INITIALISATION/inv3/INV" org.eventb.core.confidence="1000" org.eventb.core.poStamp="0" org.eventb.core.psManual="true""#;

/// Mark the `INITIALISATION/inv3/INV` status row manually discharged,
/// asserting the edit applied — a drifted needle would otherwise let
/// the carry-forward assertions pass vacuously. The zip flow carries
/// the archive's auto-discharged row; the loose-directory flow
/// reconciles against the initially empty destination and starts from
/// a fresh unattempted row. Either becomes a manual discharge.
fn discharge_inv3(text: &str) -> String {
    let shipped = r#"name="INITIALISATION/inv3/INV" org.eventb.core.confidence="1000" org.eventb.core.poStamp="0" org.eventb.core.psManual="false""#;
    let fresh = r#"name="INITIALISATION/inv3/INV" org.eventb.core.confidence="-99" org.eventb.core.poStamp="0" org.eventb.core.psManual="false""#;
    let mut out = text.replacen(shipped, DISCHARGED_INV3, 1);
    if out == text {
        out = text.replacen(fresh, DISCHARGED_INV3, 1);
    }
    assert_ne!(out, text, "the inv3 status row must be present");
    out
}

#[test]
fn build_zip_carries_doctored_statuses_across_rebuilds() {
    // A proof status recorded between builds (here: one obligation
    // discharged manually) must survive the next rebuild verbatim.
    let tmp = tempdir_unique("rossi-cli-build-status-carry");
    let out1 = tmp.join("out1.zip");
    let doctored = tmp.join("doctored.zip");
    let out2 = tmp.join("out2.zip");
    build_zip(std::path::Path::new(TRAFFIC_LIGHT), &out1);

    rewrite_zip_entry(&out1, &doctored, "traffic-light/M0.bps", discharge_inv3);
    build_zip(&doctored, &out2);

    let m0_bps = String::from_utf8(zip_entry_bytes(&out2, "traffic-light/M0.bps")).unwrap();
    assert!(
        m0_bps.contains(DISCHARGED_INV3),
        "the discharged row must carry over verbatim: {m0_bps}"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn build_directory_output_preserves_proof_state() {
    // Loose-file output reconciles against the destination directory:
    // a second build leaves .bpo/.bps byte-identical, a status doctored
    // in between carries over, and .bpr files on disk are never touched.
    let tmp = tempdir_unique("rossi-cli-build-dir-proofs");
    let src = tmp.join("src");
    extract_zip_to(&std::path::PathBuf::from(TRAFFIC_LIGHT), &src);
    let project = src.join("traffic-light");
    let out_dir = tmp.join("out");

    let run = || {
        let output = rossi_command()
            .args([
                "build",
                project.to_str().unwrap(),
                "--output",
                out_dir.to_str().unwrap(),
            ])
            .output()
            .expect("Failed to execute command");
        assert!(
            output.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    };

    run();
    let stray = out_dir.join("STRAY.bpr");
    std::fs::write(&stray, "STRAY-PROOF").unwrap();
    let bpo_first = std::fs::read(out_dir.join("M0.bpo")).unwrap();
    let bps_first = std::fs::read(out_dir.join("M0.bps")).unwrap();

    run();
    assert_eq!(std::fs::read(out_dir.join("M0.bpo")).unwrap(), bpo_first);
    assert_eq!(std::fs::read(out_dir.join("M0.bps")).unwrap(), bps_first);
    assert_eq!(std::fs::read(&stray).unwrap(), b"STRAY-PROOF");

    // Discharge one obligation in place; the next build keeps it.
    let doctored = discharge_inv3(&String::from_utf8(bps_first).unwrap());
    std::fs::write(out_dir.join("M0.bps"), &doctored).unwrap();
    run();
    assert_eq!(
        std::fs::read_to_string(out_dir.join("M0.bps")).unwrap(),
        doctored,
        "the discharged row must carry over verbatim"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn build_ignores_dot_directories_when_classifying_input() {
    // A `.rossi/rodin` workspace (or any dot-directory) inside a source tree
    // holds generated Rodin XML; if the walker saw its `.bum` files, the whole
    // directory would be misread as a Rodin project. The hidden `.bum` here is
    // deliberately invalid so taking that path fails loudly.
    let tmp = tempdir_unique("rossi-cli-build-dot-dirs");
    std::fs::write(tmp.join("main_ctx.eventb"), "CONTEXT main_ctx\nEND\n").unwrap();
    let hidden = tmp.join(".rossi").join("rodin").join("proj");
    std::fs::create_dir_all(&hidden).unwrap();
    std::fs::write(hidden.join("Bogus.bum"), "not xml").unwrap();
    std::fs::write(
        hidden.join("hidden_ctx.eventb"),
        "CONTEXT hidden_ctx\nEND\n",
    )
    .unwrap();
    let out_zip = tmp.join("out.zip");
    build_zip(&tmp, &out_zip);

    let names = zip_entry_names(&out_zip);
    assert!(
        names.iter().any(|n| n.contains("main_ctx")),
        "zip entries: {names:?}"
    );
    assert!(
        names
            .iter()
            .all(|n| !n.contains("hidden_ctx") && !n.contains("Bogus")),
        "dot-directory contents must not be built: {names:?}"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn build_directory_to_zip_carries_proofs_and_statuses() {
    // Directory input with .zip output: the source dir's .bpr proofs
    // land in the archive byte-exact and its .bpo/.bps reconcile in.
    let tmp = tempdir_unique("rossi-cli-build-dir-zip-proofs");
    let src = tmp.join("src");
    extract_zip_to(&std::path::PathBuf::from(TRAFFIC_LIGHT), &src);
    let project = src.join("traffic-light");
    let out_zip = tmp.join("out.zip");
    build_zip(&project, &out_zip);

    assert_eq!(
        zip_entry_bytes(&out_zip, "traffic-light/M0.bpr"),
        std::fs::read(project.join("M0.bpr")).unwrap(),
        "the source dir's proofs must land in the archive byte-exact"
    );
    assert!(
        zip_entry_names(&out_zip)
            .iter()
            .any(|n| n == "traffic-light/M0.bpo"),
        "generated obligations must be present"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn nested_rodin_xml_does_not_route_a_text_directory_to_the_project_loader() {
    // The gate that chooses the Rodin-project path used to walk recursively
    // and compare extensions case-insensitively, while the loader it handed
    // off to reads only direct children and matches case-sensitively. A
    // directory whose only XML sits in a subdirectory therefore passed the
    // gate and loaded as zero components — an empty project, built without a
    // word. The Event-B text beside it is what should be built.
    let tmp = tempdir_unique("rossi-cli-build-nested-xml");
    std::fs::create_dir_all(tmp.join("sub")).unwrap();
    std::fs::write(tmp.join("sub").join("M.bum"), "<not really xml>").unwrap();
    std::fs::write(tmp.join("ctx.eventb"), ASCII_CONTEXT).unwrap();
    let out_zip = tmp.join("out.zip");

    let output = rossi_command()
        .args([
            "build",
            tmp.to_str().unwrap(),
            "--output",
            out_zip.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");

    assert!(
        output.status.success(),
        "the text sibling must be built; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out_zip.exists(), "an output archive must be written");
}

#[test]
fn build_json_reports_written_files_and_diagnostics() {
    let tmp = tempdir_unique("rossi-cli-build-json");
    let out_zip = tmp.join("out.zip");
    let output = rossi_command()
        .args([
            "build",
            "--format",
            "json",
            "../rossi/examples/traffic-light.zip",
            "-o",
            out_zip.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("a JSON report");
    assert!(
        report["output"]
            .as_str()
            .is_some_and(|path| path.ends_with("out.zip")),
        "{report}"
    );
    assert!(out_zip.exists(), "the archive is written");
    assert_eq!(report["errors"], 0, "{report}");
    let files = report["projects"][0]["files"]
        .as_array()
        .expect("files array");
    assert!(
        files.iter().any(|f| {
            f["filename"]
                .as_str()
                .is_some_and(|name| name.ends_with(".bcm"))
                && f["accurate"] == true
        }),
        "{report}"
    );
    assert!(
        files.iter().all(|f| f.get("contents").is_none()),
        "file contents stay out of the report: {report}"
    );
}

#[test]
fn build_json_reports_a_failed_project_without_writing() {
    let tmp = tempdir_unique("rossi-cli-build-json-dup-names");
    std::fs::write(tmp.join("a.eventb"), "MACHINE M\nEND\n").unwrap();
    std::fs::write(tmp.join("b.eventb"), "MACHINE M\nEND\n").unwrap();
    let out_zip = tmp.join("out.zip");
    let output = rossi_command()
        .args([
            "build",
            "--format",
            "json",
            tmp.to_str().unwrap(),
            "--output",
            out_zip.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");
    assert!(
        !output.status.success(),
        "duplicate component names fail the build"
    );
    assert!(!out_zip.exists(), "no output may be written");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("a JSON report");
    assert!(report["output"].is_null(), "{report}");
    let diagnostics = report["projects"][0]["diagnostics"]
        .as_array()
        .expect("diagnostics array");
    assert!(
        diagnostics
            .iter()
            .any(|d| d["rule_id"] == "EB019" && d["severity"] == "error"),
        "{report}"
    );
}
