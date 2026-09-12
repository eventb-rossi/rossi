//! `rossi validate`: exit codes, rule tagging, and the text/JSON/SARIF reports.

use std::path::PathBuf;

use crate::helpers::{
    DUP_VARIABLE_MACHINE, lint_fixture_dir, lint_fixture_zip, project_descriptor, rossi_command,
    run_cli_with_stdin, tempdir_unique, wd_fixture_dir, write_zip,
};

#[test]
fn test_cli_help() {
    let output = rossi_command()
        .args(["validate", "--help"])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Validate Event-B model files"));
    assert!(stdout.contains("Usage: rossi validate"));
}

#[test]
fn test_cli_version() {
    let output = rossi_command()
        .args(["validate", "--version"])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
}

#[test]
fn test_cli_multiple_files() {
    let output = rossi_command()
        .args([
            "validate",
            "../rossi/examples/counter.eventb",
            "../rossi/examples/counter_machine.eventb",
        ])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("✓ ../rossi/examples/counter.eventb"));
    assert!(stdout.contains("Valid Context 'counter_ctx'"));
    assert!(stdout.contains("✓ ../rossi/examples/counter_machine.eventb"));
    assert!(stdout.contains("Valid Machine 'counter'"));
    assert!(stdout.contains("Summary:"));
    assert!(stdout.contains("Total:  2"));
    assert!(stdout.contains("Passed: 2 ✓"));
}

#[test]
fn test_cli_nonexistent_file() {
    let output = rossi_command()
        .args(["validate", "nonexistent.eventb"])
        .output()
        .expect("Failed to execute command");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("✗ nonexistent.eventb"));
    assert!(stderr.contains("File not found"));
}

#[test]
fn test_cli_quiet_mode_success() {
    let output = rossi_command()
        .args(["validate", "--quiet", "../rossi/examples/counter.eventb"])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // In quiet mode, successful validations should produce no output
    assert!(!stdout.contains("✓"));
}

#[test]
fn test_cli_quiet_mode_with_error() {
    let output = rossi_command()
        .args(["validate", "--quiet", "nonexistent.eventb"])
        .output()
        .expect("Failed to execute command");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // In quiet mode, errors should still be shown
    assert!(stderr.contains("✗ nonexistent.eventb"));
}

#[test]
fn test_cli_quiet_mode_continue_on_error_shows_all_errors() {
    let output = rossi_command()
        .args([
            "validate",
            "--quiet",
            "--continue-on-error",
            "missing-one.eventb",
            "missing-two.eventb",
        ])
        .output()
        .expect("Failed to execute command");

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("✗ missing-one.eventb"));
    assert!(stderr.contains("✗ missing-two.eventb"));
}

#[test]
fn test_cli_no_files_provided() {
    let output = rossi_command()
        .args(["validate"])
        .output()
        .expect("Failed to execute command");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Should show error about missing FILE argument
    assert!(stderr.contains("FILE") || stderr.contains("required"));
}

#[test]
fn validate_multi_project_archive_is_per_project() {
    // Two sibling projects each define a context named `C` (same `C.buc`
    // basename). Flattened into one project this falsely fires EB019
    // (duplicate component); validating each project on its own must not, and
    // the rows must be project-qualified so editors can tell them apart.
    let tmp = tempdir_unique("rossi-cli-validate-multi");
    let zip_path = tmp.join("decomp.zip");
    let ctx_xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
        <org.eventb.core.contextFile version=\"3\" \
        org.eventb.core.configuration=\"org.eventb.core.fwd\"></org.eventb.core.contextFile>\n";
    let proj_a = project_descriptor("A");
    let proj_b = project_descriptor("B");
    write_zip(
        &zip_path,
        &[
            ("A/.project", &proj_a),
            ("A/C.buc", ctx_xml.as_bytes()),
            ("B/.project", &proj_b),
            ("B/C.buc", ctx_xml.as_bytes()),
        ],
    );

    let output = rossi_command()
        .args(["validate", "--format", "json", zip_path.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");
    assert!(
        output.status.success(),
        "multi-project validate should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Each project's component is reported under its own prefix.
    assert!(
        stdout.contains("\"inner_filename\": \"A/C.buc\""),
        "expected A/C.buc in {stdout}"
    );
    assert!(
        stdout.contains("\"inner_filename\": \"B/C.buc\""),
        "expected B/C.buc in {stdout}"
    );
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(&stdout).expect("JSON output should be valid");
    for member in ["A/C.buc", "B/C.buc"] {
        let row = rows
            .iter()
            .find(|row| row["inner_filename"] == member)
            .unwrap_or_else(|| panic!("missing {member} row in {stdout}"));
        assert_eq!(
            row["path"],
            format!("{}!/{member}", zip_path.display()),
            "row: {row}"
        );
    }
    // The same name across projects is NOT a duplicate component.
    assert!(
        !stdout.contains("EB019"),
        "sibling projects sharing a component name must not flag EB019: {stdout}"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_stray_root_descriptor_keeps_single_project_basenames() {
    // One real project under `Sub/` plus a stray root-level `.project`
    // descriptor (no components). The descriptor-only group must not count
    // toward the multi gate, so rows keep their bare basename rather than being
    // spuriously prefix-qualified.
    let tmp = tempdir_unique("rossi-cli-validate-strayproj");
    let zip_path = tmp.join("model.zip");
    let ctx_xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
        <org.eventb.core.contextFile version=\"3\" \
        org.eventb.core.configuration=\"org.eventb.core.fwd\"></org.eventb.core.contextFile>\n";
    let root = project_descriptor("root");
    let sub = project_descriptor("Sub");
    write_zip(
        &zip_path,
        &[
            (".project", &root),
            ("Sub/.project", &sub),
            ("Sub/C.buc", ctx_xml.as_bytes()),
        ],
    );

    let output = rossi_command()
        .args(["validate", "--format", "json", zip_path.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"inner_filename\": \"C.buc\""),
        "a single real project keeps the bare basename: {stdout}"
    );
    assert!(
        !stdout.contains("Sub/C.buc"),
        "a descriptor-only sibling must not trigger prefix-qualification: {stdout}"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn test_cli_mixed_text_and_zip_files() {
    let output = rossi_command()
        .args([
            "validate",
            "--no-semantic",
            "../rossi/examples/counter.eventb",
            "../rossi/examples/binary-search.zip",
        ])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Check text file
    assert!(stdout.contains("✓ ../rossi/examples/counter.eventb"));
    assert!(stdout.contains("Valid Context 'counter_ctx'"));
    // Check zip file
    assert!(stdout.contains("✓ ../rossi/examples/binary-search.zip:C0.buc"));
    assert!(stdout.contains("Valid Context 'C0'"));
    assert!(stdout.contains("✓ ../rossi/examples/binary-search.zip:M0.bum"));
    assert!(stdout.contains("Valid Machine 'M0'"));
    // Check proof status (binary-search.zip ships Rodin proof files, which
    // are picked up even under --no-semantic). The refreshed archive
    // records six FIS/SIM proofs the toolchain marks broken: their stored
    // proofs predate its regenerated obligations and the default
    // auto-tactics cannot re-close them.
    assert!(stdout.contains("Proofs: 35/41 discharged, 6 pending, 6 broken"));
    // Check summary (6 components + 6 EB016 warning rows; the
    // proof-summary row is bookkeeping and is not counted)
    assert!(stdout.contains("Summary:"));
    assert!(stdout.contains("Total:  12"));
    assert!(stdout.contains("Passed: 12 ✓"));
}

#[test]
fn validate_zip_reports_proof_status() {
    // Proof files are checked whenever they're present — no flag. One
    // obligation, never proved (EB015) and with a stale script (EB016);
    // the summary row carries the counts. Warnings keep exit code 0.
    let tmp = tempdir_unique("rossi-cli-validate-proofs");
    let zip_path = tmp.join("proofs.zip");
    write_zip(
        &zip_path,
        &[
            (
                "C0.buc",
                br#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
    <org.eventb.core.carrierSet name="_s1" org.eventb.core.identifier="S"/>
</org.eventb.core.contextFile>"#,
            ),
            (
                "C0.bpo",
                br#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.poFile>
    <org.eventb.core.poSequent name="axm1/THM"/>
    <org.eventb.core.poSequent name="inv1/INV"/>
</org.eventb.core.poFile>"#,
            ),
            (
                // Confidence is read from the .bps status (eventb-checker
                // 1.6). axm1/THM is discharged-level but broken, so it
                // counts pending and reports as EB016 only; inv1/INV is a
                // plain pending obligation that reports as EB015.
                "C0.bps",
                br#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.psFile>
    <org.eventb.core.psStatus name="axm1/THM" org.eventb.core.confidence="1000" org.eventb.core.psBroken="true"/>
    <org.eventb.core.psStatus name="inv1/INV" org.eventb.core.confidence="50" org.eventb.core.psBroken="false"/>
</org.eventb.core.psFile>"#,
            ),
        ],
    );

    let output = rossi_command()
        .args(["validate", "--format", "json", zip_path.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    assert!(
        output.status.success(),
        "proof warnings should not flip the exit code; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for needle in [
        "\"rule_id\": \"EB015\"",
        "Proof obligation not discharged: inv1/INV (pending)",
        "\"rule_id\": \"EB016\"",
        "Broken proof: axm1/THM",
        "\"proof_summary\"",
        "\"pending\": 2",
        "\"broken\": 1",
    ] {
        assert!(stdout.contains(needle), "expected {needle} in: {stdout}");
    }
    // The broken obligation reports once, as EB016 — never also as EB015.
    assert!(
        !stdout.contains("axm1/THM (pending)"),
        "broken obligation must not also emit EB015: {stdout}"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

const EXTENDED_LABEL_M0: &str = r#"MACHINE M0
VARIABLES
    x
INVARIANTS
    @inv1 x ∈ ℤ
EVENTS
    EVENT INITIALISATION
    THEN
        @init1 x ≔ 0
    END

    EVENT evt
    WHERE
        @grd1 x ≥ 0
    THEN
        @act1 x ≔ x + 1
    END
END
"#;

const EXTENDED_LABEL_M1: &str = r#"MACHINE M1
REFINES
    M0
VARIABLES
    x
INVARIANTS
    @inv2 x ∈ ℤ
EVENTS
    EVENT INITIALISATION extends INITIALISATION
    END

    EVENT evt extends evt
    WHERE
        @grd1 missing ≥ 0
        @grd2 x ≥ 1
    END
END
"#;

fn extended_label_fixture(prefix: &str) -> PathBuf {
    let tmp = tempdir_unique(prefix);
    std::fs::write(tmp.join("M0.eventb"), EXTENDED_LABEL_M0).unwrap();
    std::fs::write(tmp.join("M1.eventb"), EXTENDED_LABEL_M1).unwrap();
    tmp
}

#[test]
fn validate_lints_toggle_on_zip() {
    // The fixture machine leaves `dead` unreferenced, so EB011 fires.
    // Warnings must not flip the exit code.
    let (tmp, zip_path) = lint_fixture_zip("rossi-cli-lints-toggle");
    let output = rossi_command()
        .args(["validate", zip_path.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success(), "warning-only run should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[EB011]"),
        "expected EB011 in stdout: {stdout}"
    );

    // Same model, but --no-lints disables the advisory passes. No EB011
    // rows should remain.
    let output = rossi_command()
        .args(["validate", "--no-lints", zip_path.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("EB011"),
        "EB011 should be suppressed under --no-lints: {stdout}"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_json_includes_rule_id_for_lint() {
    let (tmp, zip_path) = lint_fixture_zip("rossi-cli-json-rule-id");
    let output = rossi_command()
        .args(["validate", "--format", "json", zip_path.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"rule_id\": \"EB011\""),
        "expected structured rule_id in JSON: {stdout}"
    );
    assert!(
        stdout.contains("\"severity\": \"warning\""),
        "expected severity field in JSON: {stdout}"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_directory_flags_new_event_assigning_inherited_variable() {
    // M2 refines M1, which owns `v`, and keeps `v`. M2's *new* event `newstep`
    // (no REFINES clause) assigns the retained inherited `v` — an unprovable
    // skip-refinement. EB024 (Error) must fire and flip the exit code. M2's
    // INITIALISATION also assigns `v`, which is legitimate and must NOT be
    // flagged. EB024 needs the cross-component lint::run path, so this exercises
    // a directory project rather than a single loose file.
    let tmp = tempdir_unique("rossi-cli-validate-eb024");
    let m1 = "MACHINE M1\n\
        VARIABLES\n    v\n\
        INVARIANTS\n    @inv1 v >= 0\n\
        EVENTS\n\
        EVENT INITIALISATION\n    THEN\n        @act1 v := 0\n    END\n\n\
        EVENT tick\n    THEN\n        @act1 v := v + 1\n    END\n\
        END\n";
    let m2 = "MACHINE M2\n\
        REFINES M1\n\
        VARIABLES\n    v\n\
        INVARIANTS\n    @inv1 v >= 0\n\
        EVENTS\n\
        EVENT INITIALISATION\n    THEN\n        @act1 v := 0\n    END\n\n\
        EVENT newstep\n    THEN\n        @act1 v := v + 1\n    END\n\
        END\n";
    std::fs::write(tmp.join("M1.eventb"), m1).unwrap();
    std::fs::write(tmp.join("M2.eventb"), m2).unwrap();

    let output = rossi_command()
        .args(["validate", "--format", "json", tmp.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(&stdout).expect("validate JSON output should parse");
    let eb024: Vec<&serde_json::Value> = rows.iter().filter(|r| r["rule_id"] == "EB024").collect();
    assert_eq!(
        eb024.len(),
        1,
        "exactly one EB024, on the new event only (not INITIALISATION): {stdout}"
    );
    let row = eb024[0];
    assert_eq!(row["severity"], "error", "EB024 is Error severity: {row}");
    assert_eq!(
        row["origin"], "M2.newstep",
        "EB024 must be attributed to the new event: {row}"
    );
    assert!(
        row["error"]
            .as_str()
            .is_some_and(|m| m.contains("inherited variable")),
        "EB024 message should name the inherited variable: {row}"
    );
    assert!(
        !output.status.success(),
        "an Error-severity lint must flip the exit code; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_flags_assignment_operator_in_invariant() {
    // `@inv1 x := 5` writes an assignment where a predicate is required. Rodin
    // rejects it; rossi reports EB026 (Error) with a precise message instead of
    // a generic whole-file parse error, and the exit code flips.
    let tmp = tempdir_unique("rossi-cli-validate-eb026");
    let m = "MACHINE M\n\
        VARIABLES\n    x\n\
        INVARIANTS\n    @inv1 x := 5\n\
        EVENTS\n\
        EVENT INITIALISATION\n    THEN\n        @act1 x := 0\n    END\n\
        END\n";
    let file = tmp.join("M.eventb");
    std::fs::write(&file, m).unwrap();

    let output = rossi_command()
        .args(["validate", "--format", "json", file.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(&stdout).expect("validate JSON output should parse");
    let eb026: Vec<&serde_json::Value> = rows.iter().filter(|r| r["rule_id"] == "EB026").collect();
    assert_eq!(eb026.len(), 1, "exactly one EB026 row: {stdout}");
    assert_eq!(
        eb026[0]["severity"], "error",
        "EB026 is Error severity: {stdout}"
    );
    assert!(
        eb026[0]["error"]
            .as_str()
            .is_some_and(|m| m.contains("assignment operator")),
        "EB026 message should name the assignment operator: {stdout}"
    );
    assert!(
        !output.status.success(),
        "an Error-severity diagnostic must flip the exit code; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_reports_parallel_assignment_arity_as_eb005() {
    for (name, assignment, targets, expressions) in
        [("few", "x, y := 1", 2, 1), ("many", "x := 1, 2", 1, 2)]
    {
        let tmp = tempdir_unique(&format!("rossi-cli-validate-assignment-arity-{name}"));
        let source = format!(
            "MACHINE M\nVARIABLES\n    x\n    y\nEVENTS\n    EVENT evt\n    THEN\n        @act1 {assignment}\n    END\nEND\n"
        );
        let file = tmp.join("M.eventb");
        std::fs::write(&file, source).unwrap();

        let output = rossi_command()
            .args(["validate", "--format", "json", file.to_str().unwrap()])
            .output()
            .expect("Failed to execute command");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let rows: Vec<serde_json::Value> =
            serde_json::from_str(&stdout).expect("validate JSON output should parse");
        let errors: Vec<_> = rows
            .iter()
            .filter(|row| row["rule_id"] == "EB005")
            .collect();
        assert_eq!(errors.len(), 1, "exactly one EB005 row: {stdout}");
        let message = errors[0]["error"].as_str().unwrap();
        assert!(
            message.contains(&format!("target count ({targets})"))
                && message.contains(&format!("expression count ({expressions})")),
            "message must carry both counts: {stdout}"
        );
        assert_eq!(errors[0]["region"]["start_line"], 8);
        assert!(
            !rows.iter().any(|row| row["rule_id"] == "EB004"),
            "a lone precise assignment error must not also emit EB004: {stdout}"
        );
        assert!(!output.status.success(), "EB005 must fail validation");

        std::fs::remove_dir_all(&tmp).ok();
    }
}

#[test]
fn validate_sarif_output_is_valid() {
    let (tmp, zip_path) = lint_fixture_zip("rossi-cli-sarif");
    let output = rossi_command()
        .args(["validate", "--format", "sarif", zip_path.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let doc: serde_json::Value =
        serde_json::from_str(&stdout).expect("SARIF output should be valid JSON");

    assert_eq!(doc["version"], "2.1.0");
    assert!(doc["$schema"].is_string());
    let runs = doc["runs"].as_array().expect("runs array");
    assert_eq!(runs.len(), 1);
    let driver = &runs[0]["tool"]["driver"];
    assert_eq!(driver["name"], "rossi");
    let rules: Vec<&str> = driver["rules"]
        .as_array()
        .expect("rules array")
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(
        rules.contains(&"EB011"),
        "EB011 should be in driver.rules: {rules:?}"
    );
    let results = runs[0]["results"].as_array().expect("results array");
    assert!(!results.is_empty(), "expected at least one EB011 result");
    for r in results {
        let rid = r["ruleId"].as_str().expect("ruleId");
        assert!(
            rules.contains(&rid),
            "result ruleId {rid} not in tool.rules"
        );
    }

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_sarif_keeps_ruleless_errors() {
    let tmp = tempdir_unique("rossi-cli-sarif-ruleless");
    let missing = tmp.join("missing.eventb");
    let empty = tmp.join("empty");
    std::fs::create_dir(&empty).unwrap();

    for (input, expected_message) in [
        (
            &missing,
            format!("File not found: {}", missing.to_string_lossy()),
        ),
        (
            &empty,
            "No Event-B components found in directory".to_string(),
        ),
    ] {
        let output = rossi_command()
            .args(["validate", "--format", "sarif", input.to_str().unwrap()])
            .output()
            .expect("Failed to execute command");

        assert_eq!(output.status.code(), Some(1));
        let doc: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("SARIF output should be valid");
        let results = doc["runs"][0]["results"].as_array().expect("results array");
        assert_eq!(results.len(), 1, "one result for {}", input.display());
        let result = &results[0];
        assert!(
            result.get("ruleId").is_none(),
            "operational errors have no validation rule: {result}"
        );
        assert_eq!(result["level"], "error");
        assert_eq!(result["message"]["text"], expected_message);
        assert_eq!(
            result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
            input.to_string_lossy().as_ref()
        );
    }

    std::fs::remove_dir_all(&tmp).ok();
}

/// The `artifactLocation.uri` of the first result carrying one.
fn first_sarif_uri(stdout: &str) -> String {
    let doc: serde_json::Value =
        serde_json::from_str(stdout).expect("SARIF output should be valid JSON");
    doc["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
        .as_str()
        .expect("a result should carry an artifactLocation.uri")
        .to_string()
}

#[test]
fn validate_sarif_member_uri_follows_input_kind() {
    // A SARIF consumer resolves artifactLocation.uri against the repository
    // tree, so a member of a *directory* must be the path it really is —
    // `proj/Lint.bum`, never the archive form `proj!/Lint.bum`.
    let dir = lint_fixture_dir("rossi-cli-sarif-dir");
    let output = rossi_command()
        .args(["validate", "--format", "sarif", dir.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    let uri = first_sarif_uri(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(uri, format!("{}/Lint.bum", dir.display()));
    assert!(
        !uri.contains("!/"),
        "directory members are real paths: {uri}"
    );

    // The other half of the rule: an archive member is not a file on disk, so
    // it keeps SARIF's `!/` separator.
    let (zip_tmp, zip_path) = lint_fixture_zip("rossi-cli-sarif-zip-uri");
    let output = rossi_command()
        .args(["validate", "--format", "sarif", zip_path.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    let uri = first_sarif_uri(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(uri, format!("{}!/Lint.bum", zip_path.display()));

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&zip_tmp).ok();
}

#[test]
fn validate_json_reports_input_kind_and_joined_path() {
    // `path` is the ready-to-use location. The component fields stay available
    // for consumers that need to distinguish inputs and members.
    let dir = lint_fixture_dir("rossi-cli-json-input-kind");
    let (zip_tmp, zip_path) = lint_fixture_zip("rossi-cli-json-input-kind-zip");

    for (input, expected_kind, member, expected_path, component_type, component_name) in [
        (
            dir.to_str().unwrap(),
            "directory",
            Some("Lint.bum"),
            format!("{}/Lint.bum", dir.display()),
            "Machine",
            "Lint",
        ),
        (
            zip_path.to_str().unwrap(),
            "archive",
            Some("Lint.bum"),
            format!("{}!/Lint.bum", zip_path.display()),
            "Machine",
            "Lint",
        ),
        (
            "../rossi/examples/counter.eventb",
            "file",
            None,
            "../rossi/examples/counter.eventb".to_string(),
            "Context",
            "counter_ctx",
        ),
    ] {
        let output = rossi_command()
            .args(["validate", "--format", "json", input])
            .output()
            .expect("Failed to execute command");
        let rows: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("JSON output should be valid");
        let rows = rows.as_array().expect("rows array");
        assert!(!rows.is_empty(), "{input} produced no rows");
        for row in rows {
            assert_eq!(row["input"], expected_kind, "row: {row}");
        }
        let row = member.map_or(&rows[0], |member| {
            rows.iter()
                .find(|row| row["inner_filename"] == member)
                .unwrap_or_else(|| panic!("missing {member} row in {rows:?}"))
        });
        assert_eq!(row["path"], expected_path, "row: {row}");
        assert_eq!(row["component_type"], component_type, "row: {row}");
        assert_eq!(row["component_name"], component_name, "row: {row}");
        if expected_kind == "archive" {
            // Each archive member carries its own component identity — the
            // seen context must not inherit the machine's.
            let ctx_row = rows
                .iter()
                .find(|row| row["inner_filename"] == "Ctx.buc")
                .unwrap_or_else(|| panic!("missing Ctx.buc row in {rows:?}"));
            assert_eq!(ctx_row["component_type"], "Context", "row: {ctx_row}");
            assert_eq!(ctx_row["component_name"], "Ctx", "row: {ctx_row}");
        }
    }

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&zip_tmp).ok();
}

#[test]
fn validate_text_joins_directory_members_as_paths() {
    // The human format follows the same rule, so the reported location can be
    // opened (or clicked) directly.
    let tmp = lint_fixture_dir("rossi-cli-text-dir-path");
    let output = rossi_command()
        .args(["validate", tmp.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let member = format!("{}/Lint.bum", tmp.display());
    assert!(
        stdout.contains(&member),
        "expected `{member}` in output: {stdout}"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_deny_warnings_fails_on_advisory_lints() {
    // The fixture's `dead` variable raises EB011, which is advisory and exits
    // 0 — a CI job that wants to gate on it would otherwise have to parse the
    // JSON itself.
    let tmp = lint_fixture_dir("rossi-cli-deny-warnings");

    let lenient = rossi_command()
        .args(["validate", tmp.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");
    assert!(
        lenient.status.success(),
        "advisory lints keep exiting 0 by default"
    );

    let strict = rossi_command()
        .args(["validate", "--deny-warnings", tmp.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");
    assert_eq!(strict.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&strict.stdout);
    assert!(
        stdout.contains("[EB011]"),
        "the warning is still reported as a warning"
    );
    // Hardening the gate must not cut the report short at the first finding:
    // a run that gates on lints is exactly the one that needs to see them all.
    assert!(
        stdout.contains("Valid Machine 'Lint'") && stdout.contains("Valid Context 'Ctx'"),
        "every row is still reported: {stdout}"
    );

    // The severity a consumer sees must not change: hardening the exit code
    // must not inflate what code scanning records.
    let sarif = rossi_command()
        .args([
            "validate",
            "--deny-warnings",
            "--format",
            "sarif",
            tmp.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");
    let doc: serde_json::Value =
        serde_json::from_slice(&sarif.stdout).expect("SARIF output should be valid");
    let eb011 = doc["runs"][0]["results"]
        .as_array()
        .expect("results array")
        .iter()
        .find(|r| r["ruleId"] == "EB011")
        .expect("EB011 is reported");
    assert_eq!(eb011["level"], "warning");
    assert_eq!(sarif.status.code(), Some(1));

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_flags_a_primed_declared_name() {
    // Rodin parses every declaration with primes disallowed
    // (`IdentifierModule`), so `constants c'` is `InvalidIdentifierError`
    // there and EB033 here. It is semantic rather than advisory: the exit
    // code flips, `--no-lints` must not silence it, `--no-semantic` must.
    let tmp = tempdir_unique("rossi-cli-primed-decl");
    let file = tmp.join("primed.eventb");
    std::fs::write(
        &file,
        "context c\nconstants c'\naxioms\n  @axm1 c' \u{2208} \u{2115}\nend\n",
    )
    .unwrap();
    let path = file.to_str().unwrap();

    let output = rossi_command()
        .args(["validate", "--format", "json", path])
        .output()
        .expect("Failed to execute command");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(&stdout).expect("validate JSON output should parse");
    let eb033: Vec<&serde_json::Value> = rows.iter().filter(|r| r["rule_id"] == "EB033").collect();
    assert_eq!(eb033.len(), 1, "exactly one EB033 row: {stdout}");
    assert_eq!(
        eb033[0]["severity"], "error",
        "EB033 is Error severity: {stdout}"
    );
    assert!(
        eb033[0]["error"]
            .as_str()
            .is_some_and(|m| m.contains("after-state prime")),
        "EB033 message should name the prime: {stdout}"
    );
    assert!(
        !output.status.success(),
        "an Error-severity diagnostic must flip the exit code; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let lints_off = rossi_command()
        .args(["validate", "--format", "json", "--no-lints", path])
        .output()
        .expect("Failed to execute command");
    let stdout = String::from_utf8_lossy(&lints_off.stdout);
    assert!(
        stdout.contains("EB033"),
        "EB033 is semantic, not advisory: {stdout}"
    );

    let semantics_off = rossi_command()
        .args(["validate", "--format", "json", "--no-semantic", path])
        .output()
        .expect("Failed to execute command");
    let stdout = String::from_utf8_lossy(&semantics_off.stdout);
    assert!(
        !stdout.contains("EB033"),
        "--no-semantic should suppress EB033: {stdout}"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_flags_a_primed_use_in_a_lone_file() {
    // A lone `.eventb` file has no SEES / EXTENDS parents, so `validate`
    // cannot decide most scope questions -- but it can decide this one: no
    // declaration may be primed, so no parent could ever put `c'` in scope.
    // The file must report exactly what the directory does.
    let tmp = tempdir_unique("rossi-cli-primed-use");
    std::fs::write(
        tmp.join("C.eventb"),
        "context C\nconstants c\naxioms\n  @a c' = 1\nend\n",
    )
    .unwrap();
    let file = tmp.join("C.eventb");
    let file = file.to_str().unwrap();
    let dir = tmp.to_str().unwrap();

    let row_of = |args: &[&str]| -> serde_json::Value {
        let out = rossi_command()
            .args(args)
            .output()
            .expect("Failed to execute command");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let rows: Vec<serde_json::Value> =
            serde_json::from_str(&stdout).expect("validate JSON output should parse");
        rows.into_iter()
            .find(|r| r["rule_id"] == "EB018")
            .unwrap_or_else(|| panic!("expected an EB018 row in {stdout}"))
    };

    let from_file = row_of(&["validate", "--format", "json", file]);
    let from_dir = row_of(&["validate", "--format", "json", dir]);
    assert_eq!(from_file["error"], from_dir["error"]);
    assert_eq!(from_file["origin"], from_dir["origin"]);
    assert_eq!(from_file["region"], from_dir["region"]);
    assert!(
        from_file["error"]
            .as_str()
            .is_some_and(|m| m.contains("unknown identifier 'c''")),
        "EB018 should name the primed identifier: {from_file}"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_flags_structural_keyword_names() {
    // A carrier set named `end` is legal in Rodin's object model but no
    // textual notation can represent it: rossi used to accept it silently and
    // `fmt` wrote a file no text tool could read back. EB028 is advisory, so
    // it exits 0 (`--deny-warnings` gating is rule-independent and covered by
    // `validate_deny_warnings_fails_on_advisory_lints`). The `--no-lints`
    // run is the only loose-file exercise of that gate: the zip toggle test
    // goes through the project path.
    let tmp = tempdir_unique("rossi-cli-keyword-name");
    let file = tmp.join("set_end.eventb");
    std::fs::write(
        &file,
        "context c\nsets end\nconstants k\naxioms\n  @axm1 k ∈ ℕ\nend\n",
    )
    .unwrap();
    let path = file.to_str().unwrap();

    let lenient = rossi_command()
        .args(["validate", path])
        .output()
        .expect("Failed to execute command");
    assert!(lenient.status.success(), "EB028 is advisory");
    let stdout = String::from_utf8_lossy(&lenient.stdout);
    assert!(
        stdout.contains("[EB028]"),
        "expected EB028 in stdout: {stdout}"
    );

    let silenced = rossi_command()
        .args(["validate", "--no-lints", path])
        .output()
        .expect("Failed to execute command");
    assert!(silenced.status.success());
    let stdout = String::from_utf8_lossy(&silenced.stdout);
    assert!(
        !stdout.contains("EB028"),
        "EB028 should be suppressed under --no-lints: {stdout}"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_deny_warnings_still_reports_the_rows_it_fails_on() {
    // `--quiet` keeps only what matters, and under `--deny-warnings` the
    // advisory rows are exactly what fails the run — suppressing them would
    // exit 1 having printed nothing at all. The summary must agree with the
    // exit code too, rather than closing with "Failed: 0".
    let tmp = lint_fixture_dir("rossi-cli-deny-warnings-quiet");

    let quiet = rossi_command()
        .args([
            "validate",
            "--quiet",
            "--deny-warnings",
            tmp.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");
    assert_eq!(quiet.status.code(), Some(1));
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&quiet.stdout),
        String::from_utf8_lossy(&quiet.stderr)
    );
    assert!(
        printed.contains("[EB011]"),
        "a failing run must say what failed: {printed:?}"
    );

    let loud = rossi_command()
        .args(["validate", "--deny-warnings", tmp.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");
    let stdout = String::from_utf8_lossy(&loud.stdout);
    assert!(
        !stdout.contains("Failed: 0"),
        "the summary must not claim nothing failed on a failing run: {stdout}"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_deny_warnings_leaves_a_clean_project_passing() {
    let output = rossi_command()
        .args([
            "validate",
            "--deny-warnings",
            "../rossi/examples/counter.eventb",
        ])
        .output()
        .expect("Failed to execute command");
    assert!(
        output.status.success(),
        "a diagnostic-free run still passes; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_hides_eb010_until_show_info_is_requested() {
    let tmp = wd_fixture_dir("rossi-cli-wd-opt-in");

    let default = rossi_command()
        .args(["validate", tmp.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");
    assert!(default.status.success());
    assert!(
        !String::from_utf8_lossy(&default.stdout).contains("EB010"),
        "default validation must stay unchanged"
    );

    let shown = rossi_command()
        .args(["validate", "--show-info", tmp.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");
    assert!(
        shown.status.success(),
        "INFO does not fail validation: {}",
        String::from_utf8_lossy(&shown.stderr)
    );
    let stdout = String::from_utf8_lossy(&shown.stdout);
    assert!(stdout.contains("i "), "INFO uses its own glyph: {stdout}");
    assert!(
        stdout.contains("(M.wd)"),
        "formula origin is preserved: {stdout}"
    );
    assert!(stdout.contains("[EB010] Well-definedness condition: x≠0"));

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_show_info_checks_loose_eventb_well_definedness() {
    let tmp = tempdir_unique("rossi-cli-wd-loose");
    let file = tmp.join("M.eventb");
    std::fs::write(
        &file,
        "MACHINE M\nVARIABLES\n    x\nINVARIANTS\n    @type x ∈ ℤ\n    @wd 10 ÷ x > 0\nEND\n",
    )
    .unwrap();

    let output = rossi_command()
        .args(["validate", "--show-info", file.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("(M.wd)"),
        "formula origin is preserved: {stdout}"
    );
    assert!(
        stdout.contains("[EB010] Well-definedness condition: x≠0"),
        "loose files include WD diagnostics: {stdout}"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_eb010_is_structured_as_info_and_never_denied() {
    let tmp = wd_fixture_dir("rossi-cli-wd-json");
    let output = rossi_command()
        .args([
            "validate",
            "--show-info",
            "--deny-warnings",
            "--format",
            "json",
            tmp.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");
    assert!(
        output.status.success(),
        "--deny-warnings must not promote INFO: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: Vec<serde_json::Value> =
        serde_json::from_slice(&output.stdout).expect("JSON output should be valid");
    let eb010 = rows
        .iter()
        .find(|row| row["rule_id"] == "EB010")
        .expect("EB010 row");
    assert_eq!(eb010["severity"], "info");
    assert_eq!(eb010["success"], true);
    assert_eq!(eb010["origin"], "M.wd");
    assert_eq!(eb010["inner_filename"], "M.bum");

    let quiet = rossi_command()
        .args(["validate", "--quiet", "--show-info", tmp.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");
    assert!(quiet.status.success());
    assert!(quiet.stdout.is_empty(), "quiet suppresses INFO rows");
    assert!(quiet.stderr.is_empty());

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_sarif_emits_eb010_as_a_note() {
    let tmp = wd_fixture_dir("rossi-cli-wd-sarif");
    let output = rossi_command()
        .args([
            "validate",
            "--show-info",
            "--format",
            "sarif",
            tmp.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");
    assert!(output.status.success());
    let doc: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("SARIF output should be valid");
    let result = doc["runs"][0]["results"]
        .as_array()
        .expect("results array")
        .iter()
        .find(|result| result["ruleId"] == "EB010")
        .expect("EB010 result");
    assert_eq!(result["level"], "note");
    let descriptor = doc["runs"][0]["tool"]["driver"]["rules"]
        .as_array()
        .expect("rules array")
        .iter()
        .find(|rule| rule["id"] == "EB010")
        .expect("EB010 descriptor");
    assert_eq!(descriptor["defaultConfiguration"]["level"], "note");

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_sarif_category_names_the_analysis() {
    // A repository uploading more than one rossi analysis tells them apart by
    // category, which code scanning reads from runs[].automationDetails.id.
    let tmp = lint_fixture_dir("rossi-cli-sarif-category");
    let output = rossi_command()
        .args([
            "validate",
            "--format",
            "sarif",
            "--sarif-category",
            "rossi-models",
            tmp.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");

    let doc: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("SARIF output should be valid");
    assert_eq!(doc["runs"][0]["automationDetails"]["id"], "rossi-models");

    // Absent unless asked for: an untagged run must not claim a category.
    let untagged = rossi_command()
        .args(["validate", "--format", "sarif", tmp.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");
    let doc: serde_json::Value =
        serde_json::from_slice(&untagged.stdout).expect("SARIF output should be valid");
    assert!(doc["runs"][0]["automationDetails"].is_null());

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_sarif_category_requires_the_sarif_format() {
    for format in ["text", "json"] {
        let output = rossi_command()
            .args([
                "validate",
                "--format",
                format,
                "--sarif-category",
                "rossi",
                "../rossi/examples/counter.eventb",
            ])
            .output()
            .expect("Failed to execute command");
        assert_eq!(output.status.code(), Some(2), "format {format}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("--sarif-category requires"),
            "format {format} should explain the misuse"
        );
    }
}

#[test]
fn validate_sarif_emits_one_run_for_many_inputs() {
    // Since 2025-07-21 code scanning rejects an upload whose runs share a
    // category, so a consumer relies on rossi producing exactly one run
    // however many inputs it was given.
    let dir = lint_fixture_dir("rossi-cli-sarif-single-run");
    let (zip_tmp, zip_path) = lint_fixture_zip("rossi-cli-sarif-single-run-zip");
    let broken = broken_member_dir("rossi-cli-sarif-single-run-broken");

    let output = rossi_command()
        .args([
            "validate",
            "--format",
            "sarif",
            dir.to_str().unwrap(),
            zip_path.to_str().unwrap(),
            broken.to_str().unwrap(),
            "../rossi/examples/counter.eventb",
        ])
        .output()
        .expect("Failed to execute command");

    let doc: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("SARIF output should be valid");
    let runs = doc["runs"].as_array().expect("runs array");
    assert_eq!(runs.len(), 1, "four inputs must still be one run");
    // …and that run really did collect rows from more than one input.
    let uris: Vec<String> = runs[0]["results"]
        .as_array()
        .expect("results array")
        .iter()
        .map(|r| {
            r["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    for input in [dir.to_str().unwrap(), zip_path.to_str().unwrap()] {
        assert!(
            uris.iter().any(|u| u.starts_with(input)),
            "no result from {input}: {uris:?}"
        );
    }

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&zip_tmp).ok();
    std::fs::remove_dir_all(&broken).ok();
}

#[test]
fn validate_output_writes_the_structured_report_to_a_file() {
    // A CI step wants the report in a file it can upload, without depending on
    // shell redirection inside a composite action.
    let tmp = lint_fixture_dir("rossi-cli-output-structured");
    for (format, parses) in [
        ("json", true),
        ("sarif", true),
        ("text", false), // not JSON, just a report
    ] {
        let out = tmp.join(format!("report.{format}"));
        let output = rossi_command()
            .args([
                "validate",
                "--format",
                format,
                "--output",
                out.to_str().unwrap(),
                tmp.to_str().unwrap(),
            ])
            .output()
            .expect("Failed to execute command");

        assert!(
            output.stdout.is_empty(),
            "{format}: the report went to the file, not stdout: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let written = std::fs::read_to_string(&out).expect("report file written");
        assert!(!written.is_empty(), "{format}: report file is empty");
        if parses {
            serde_json::from_str::<serde_json::Value>(&written)
                .unwrap_or_else(|e| panic!("{format} report should parse: {e}"));
        } else {
            assert!(
                written.contains("Valid Machine 'Lint'"),
                "text report should hold the rows: {written}"
            );
        }
    }

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_output_captures_error_rows_that_would_go_to_stderr() {
    // The human format splits its streams; redirecting it must not drop the
    // error rows on the floor.
    let tmp = broken_member_dir("rossi-cli-output-text-errors");
    let out = tmp.join("report.txt");
    let output = rossi_command()
        .args([
            "validate",
            "--output",
            out.to_str().unwrap(),
            tmp.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    let written = std::fs::read_to_string(&out).expect("report file written");
    assert!(
        written.contains("[EB004]") && written.contains("Valid Machine 'clean'"),
        "both streams land in the file: {written}"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_output_to_an_unwritable_path_fails_before_validating() {
    let output = rossi_command()
        .args([
            "validate",
            "--output",
            "no/such/directory/report.json",
            "--format",
            "json",
            "../rossi/examples/counter.eventb",
        ])
        .output()
        .expect("Failed to execute command");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to open"),
        "the failure names the output path: {stderr}"
    );
}

#[test]
fn validate_sarif_includes_parse_error_region_issue_42() {
    // A reserved word used as a constant name: SARIF must carry a
    // physicalLocation.region covering the offending word (issue #42).
    let source = "CONTEXT c0\nCONSTANTS\n    dom\nEND\n";
    let output = run_cli_with_stdin(
        &[
            "validate",
            "--format",
            "sarif",
            "--stdin-filename",
            "broken.eventb",
            "-",
        ],
        source,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let doc: serde_json::Value =
        serde_json::from_str(&stdout).expect("SARIF output should be valid JSON");
    let results = doc["runs"][0]["results"].as_array().expect("results array");
    let region = results
        .iter()
        .map(|r| &r["locations"][0]["physicalLocation"]["region"])
        .find(|reg| reg.is_object())
        .expect("a parse-error result should carry a physicalLocation.region");
    assert_eq!(region["startLine"], 3);
    assert_eq!(region["startColumn"], 5);
    assert_eq!(region["endLine"], 3);
    assert_eq!(region["endColumn"], 8);
}

#[test]
fn validate_directory_without_components_is_rejected() {
    let tmp = tempdir_unique("rossi-cli-validate-empty-dir");
    let output = rossi_command()
        .args(["validate", "--format", "json", tmp.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    assert_eq!(output.status.code(), Some(1));
    let rows: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON output should be valid");
    let rows = rows.as_array().expect("rows array");
    assert_eq!(rows.len(), 1, "empty directory should emit one row");
    let row = &rows[0];
    assert_eq!(row["file"], tmp.to_str().unwrap());
    assert_eq!(row["input"], "directory");
    assert_eq!(row["success"], false);
    assert_eq!(row["severity"], "error");
    assert_eq!(row["error"], "No Event-B components found in directory");
    assert!(row.get("inner_filename").is_none());
    assert!(row.get("rule_id").is_none());

    std::fs::remove_dir_all(&tmp).ok();
}

/// A machine whose EVENTS section is empty — the Camille grammar wants at
/// least one EVENT, so it fails to parse at the closing `END` on line 7.
const BROKEN_MEMBER: &str =
    "MACHINE broken\nVARIABLES\n    count\nINVARIANTS\n    @inv1 count ∈ ℕ\nEVENTS\nEND\n";

/// A well-formed sibling of [`BROKEN_MEMBER`] in the same directory.
const CLEAN_MEMBER: &str = "MACHINE clean\nVARIABLES\n    count\nINVARIANTS\n    @inv1 count ∈ ℕ\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 count ≔ 0\n    END\nEND\n";

fn broken_member_dir(prefix: &str) -> PathBuf {
    let tmp = tempdir_unique(prefix);
    std::fs::write(tmp.join("broken.eventb"), BROKEN_MEMBER).unwrap();
    std::fs::write(tmp.join("clean.eventb"), CLEAN_MEMBER).unwrap();
    tmp
}

#[test]
fn validate_directory_parse_error_names_the_member_and_its_position() {
    // Loading the project aborts on the malformed member. Reporting that as
    // one error against the bare directory loses both the file and the
    // position, so neither a CI annotation nor a SARIF location can be built
    // — and the notation is Camille, so the rule is EB004, exactly as it is
    // when the same file is validated on its own.
    let tmp = broken_member_dir("rossi-cli-dir-parse-error");
    let output = rossi_command()
        .args(["validate", tmp.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let located = format!("{}/broken.eventb:7:1", tmp.display());
    assert!(
        stderr.contains(&located) && stderr.contains("[EB004]"),
        "expected `{located}` reported as EB004: {stderr}"
    );
    // The sibling is still validated: one broken file no longer hides the
    // rest of the project.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Valid Machine 'clean'"),
        "sibling components must still be reported: {stdout}"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_directory_reports_every_sibling_whatever_the_entry_order() {
    // Stopping at the first failing row would drop rows this fallback had
    // already validated, and which ones survived would depend on the order
    // read_dir happens to return entries in — so the same project would report
    // differently on different filesystems.
    let tmp = tempdir_unique("rossi-cli-dir-entry-order");
    std::fs::write(tmp.join("aaa_broken.eventb"), BROKEN_MEMBER).unwrap();
    for name in ["b", "c", "d", "e", "f"] {
        let source = CLEAN_MEMBER.replace("clean", &format!("{name}_clean"));
        std::fs::write(tmp.join(format!("{name}_clean.eventb")), source).unwrap();
    }

    let output = rossi_command()
        .args(["validate", tmp.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    let stdout = String::from_utf8_lossy(&output.stdout);
    for name in ["b", "c", "d", "e", "f"] {
        assert!(
            stdout.contains(&format!("Valid Machine '{name}_clean'")),
            "{name}_clean is missing from the report: {stdout}"
        );
    }
    assert!(String::from_utf8_lossy(&output.stderr).contains("[EB004]"));
    assert_eq!(output.status.code(), Some(1));

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_directory_parse_error_is_located_in_json_and_sarif() {
    let tmp = broken_member_dir("rossi-cli-dir-parse-error-structured");

    let json = rossi_command()
        .args(["validate", "--format", "json", tmp.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");
    let rows: serde_json::Value =
        serde_json::from_slice(&json.stdout).expect("JSON output should be valid");
    let failure = rows
        .as_array()
        .expect("rows array")
        .iter()
        .find(|r| r["success"] == false)
        .expect("the malformed member fails");
    assert_eq!(failure["rule_id"], "EB004");
    assert_eq!(failure["inner_filename"], "broken.eventb");
    assert_eq!(failure["region"]["start_line"], 7);
    assert_eq!(failure["region"]["start_column"], 1);

    let sarif = rossi_command()
        .args(["validate", "--format", "sarif", tmp.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");
    let doc: serde_json::Value =
        serde_json::from_slice(&sarif.stdout).expect("SARIF output should be valid");
    let result = doc["runs"][0]["results"]
        .as_array()
        .expect("results array")
        .iter()
        .find(|r| r["ruleId"] == "EB004")
        .expect("the parse failure reaches SARIF");
    let location = &result["locations"][0]["physicalLocation"];
    assert_eq!(
        location["artifactLocation"]["uri"],
        serde_json::json!(format!("{}/broken.eventb", tmp.display()))
    );
    assert_eq!(location["region"]["startLine"], 7);

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_duplicate_component_names_fail_with_eb019() {
    // Two `.eventb` files declaring the same machine name — a state Rodin
    // cannot represent (a component's name is its file identity), so EB019
    // is an Error and validation must fail.
    let tmp = tempdir_unique("rossi-cli-validate-dup-names");
    std::fs::write(tmp.join("a.eventb"), "MACHINE M\nEND\n").unwrap();
    std::fs::write(tmp.join("b.eventb"), "MACHINE M\nEND\n").unwrap();

    let output = rossi_command()
        .args(["validate", tmp.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    assert!(
        !output.status.success(),
        "duplicate component names must fail validation; stdout={}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[EB019]"), "stderr: {stderr}");

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_directory_reports_ill_typed_action_as_eb006() {
    let tmp = tempdir_unique("rossi-cli-validate-type-error");
    std::fs::write(
        tmp.join("M.eventb"),
        r#"MACHINE M
VARIABLES
    x
INVARIANTS
    @typing x ∈ ℤ
EVENTS
    EVENT INITIALISATION
    THEN
        @act1 x ≔ 0
    END

    EVENT bad
    THEN
        @act1 x ≔ TRUE + FALSE
    END
END
"#,
    )
    .unwrap();

    let output = rossi_command()
        .args(["validate", "--format", "json", tmp.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    assert!(
        !output.status.success(),
        "ill-typed action must fail validation; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&stdout).unwrap();
    let diagnostic = rows
        .iter()
        .find(|row| row["rule_id"] == "EB006")
        .unwrap_or_else(|| panic!("missing EB006 diagnostic: {stdout}"));
    assert_eq!(diagnostic["severity"], "error");
    assert_eq!(diagnostic["origin"], "M.bad.act1");
    assert_eq!(diagnostic["inner_filename"], "M.eventb");
    assert!(
        diagnostic["region"].is_object(),
        "the error must be positioned on the invalid action: {diagnostic}"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_directory_reports_inherited_event_label_as_eb022_error() {
    let tmp = extended_label_fixture("rossi-cli-validate-inherited-label");
    let output = rossi_command()
        .args(["validate", "--format", "json", tmp.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    assert!(
        !output.status.success(),
        "inherited EB022 must fail validation; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&stdout).unwrap();
    let conflicts: Vec<_> = rows
        .iter()
        .filter(|row| row["rule_id"] == "EB022")
        .collect();
    assert_eq!(conflicts.len(), 1, "{stdout}");
    let conflict = conflicts[0];
    assert_eq!(conflict["severity"], "error");
    assert_eq!(conflict["origin"], "M1.evt.grd1");
    assert_eq!(conflict["inner_filename"], "M1.eventb");
    assert!(
        conflict["region"].is_object(),
        "the error must be positioned on the concrete clause: {conflict}"
    );
    assert!(
        conflict["error"]
            .as_str()
            .is_some_and(|m| m.contains("inherited guard label")),
        "{conflict}"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_project_reports_duplicate_identifier_exactly_once() {
    // EB021 comes from the SC build; the lint pass must not repeat it, or
    // every duplicate would show up twice in a project validation.
    let tmp = tempdir_unique("rossi-cli-validate-dup-var-once");
    std::fs::write(tmp.join("M.eventb"), DUP_VARIABLE_MACHINE).unwrap();

    let output = rossi_command()
        .args(["validate", "--format", "json", tmp.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    assert!(!output.status.success(), "EB021 is an error");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.matches("\"rule_id\": \"EB021\"").count(),
        1,
        "EB021 must be reported exactly once: {stdout}"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_directory_with_no_semantic_is_rejected() {
    let tmp = tempdir_unique("rossi-cli-validate-dir-nosem");
    std::fs::create_dir_all(&tmp).unwrap();

    let output = rossi_command()
        .args(["validate", "--no-semantic", tmp.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("directory inputs require semantic checks"));

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_zip_wrong_root_reports_eb002() {
    // A .buc whose root is neither contextFile nor machineFile passes
    // parse_zip_file_with_recovery silently (the per-extension parser is
    // tolerant) but is rejected by Project::from_zip_file → parse_xml,
    // which surfaces UnexpectedXmlRoot. The CLI maps that to EB002.
    assert_validate_zip_json_contains_rule(
        "rossi-cli-validate-eb002",
        "wrong-root.zip",
        "WrongRoot.buc",
        br#"<?xml version="1.0" encoding="UTF-8"?>
<some.unknown.root version="3"/>"#,
        "EB002",
    );
}

#[test]
fn validate_zip_missing_target_reports_eb003() {
    // A contextFile with an extendsContext element lacking its target
    // attribute surfaces MissingXmlAttribute from
    // parse_zip_file_with_recovery wrapped in FileContext; the CLI helper
    // unwraps the wrapper and tags the row as EB003.
    assert_validate_zip_json_contains_rule(
        "rossi-cli-validate-eb003",
        "missing-target.zip",
        "Bad.buc",
        br#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
    <org.eventb.core.context name="Bad"/>
    <org.eventb.core.extendsContext name="internal"/>
</org.eventb.core.contextFile>"#,
        "EB003",
    );
}

#[test]
fn validate_stdin_empty_clause_reports_eb029_at_the_keyword() {
    // A structural mistake the grammar can name is reported as itself, at the
    // construct that is wrong, rather than as the whole-file EB004 fallback
    // anchored on the line the parser happened to reach.
    let source = "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT e\n    WHERE\n    THEN\n        @act1 x ≔ 1\n    END\nEND\n";
    let output = run_cli_with_stdin(&["validate", "--format", "json", "-"], source);
    assert!(!output.status.success(), "expected non-zero exit for EB029");
    let rows: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON output should be valid");
    let row = rows
        .as_array()
        .expect("rows array")
        .iter()
        .find(|row| row["rule_id"] == "EB029")
        .unwrap_or_else(|| panic!("expected an EB029 row: {rows}"));
    assert_eq!(row["error"], "`WHERE` clause is empty");
    assert_eq!(row["region"]["start_line"], 6);
    assert_eq!(row["region"]["start_column"], 5);
}

#[test]
fn validate_stdin_clause_out_of_order_reports_eb030() {
    let source = "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT e\n    THEN\n        @act1 x ≔ 1\n    WITH\n        @w y = 1\n    END\nEND\n";
    let output = run_cli_with_stdin(&["validate", "--format", "json", "-"], source);
    assert!(!output.status.success(), "expected non-zero exit for EB030");
    let rows: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON output should be valid");
    let row = rows
        .as_array()
        .expect("rows array")
        .iter()
        .find(|row| row["rule_id"] == "EB030")
        .unwrap_or_else(|| panic!("expected an EB030 row: {rows}"));
    assert_eq!(row["error"], "`WITH` clause must come before `THEN`");
    assert_eq!(row["region"]["start_line"], 8);
    assert_eq!(row["region"]["start_column"], 5);
}

#[test]
fn validate_stdin_section_out_of_order_reports_eb034() {
    // EB034 is advice, not a rejection: the parser accepts the file, so the
    // run stays green and `--no-lints` silences the row.
    let source = "CONTEXT c\nAXIOMS\n    @a 1 = 1\nSETS\n    S\nEND\n";
    let output = run_cli_with_stdin(&["validate", "--format", "json", "-"], source);
    assert!(output.status.success(), "EB034 is a Warning, not an Error");
    let rows: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON output should be valid");
    let row = rows
        .as_array()
        .expect("rows array")
        .iter()
        .find(|row| row["rule_id"] == "EB034")
        .unwrap_or_else(|| panic!("expected an EB034 row: {rows}"));
    assert_eq!(row["severity"], "warning");
    assert_eq!(row["region"]["start_line"], 4);

    let quiet = run_cli_with_stdin(&["validate", "--format", "json", "--no-lints", "-"], source);
    let rows: serde_json::Value =
        serde_json::from_slice(&quiet.stdout).expect("JSON output should be valid");
    assert!(
        !rows
            .as_array()
            .expect("rows array")
            .iter()
            .any(|row| row["rule_id"] == "EB034"),
        "EB034 should be suppressed under --no-lints: {rows}"
    );
}

#[test]
fn validate_stdin_missing_label_reports_eb032_at_the_item() {
    let source =
        "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT e\n    THEN\n        x ≔ 1\n    END\nEND\n";
    let output = run_cli_with_stdin(&["validate", "--format", "json", "-"], source);
    assert!(!output.status.success(), "expected non-zero exit for EB032");
    let rows: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON output should be valid");
    let row = rows
        .as_array()
        .expect("rows array")
        .iter()
        .find(|row| row["rule_id"] == "EB032")
        .unwrap_or_else(|| panic!("expected an EB032 row: {rows}"));
    assert_eq!(row["error"], "an action needs a label");
    assert_eq!(row["region"]["start_line"], 7);
    assert_eq!(row["region"]["start_column"], 9);
}

#[test]
fn validate_stdin_camille_error_reports_eb004() {
    // Loose `.eventb` text that the Camille grammar rejects is tagged EB004
    // (whole-file Camille parse error), not EB005 (formula-level error).
    let output = run_cli_with_stdin(
        &["validate", "--format", "json", "-"],
        "MACHINE broken\nTHIS IS NOT EVENT-B\nEND\n",
    );
    assert!(!output.status.success(), "expected non-zero exit for EB004");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"rule_id\": \"EB004\""),
        "expected EB004 in JSON: {stdout}"
    );
}

#[test]
fn validate_stdin_duplicate_identifier_and_label_report_eb021_eb022() {
    // A machine that declares `x` twice and reuses invariant label `inv1` is
    // structurally invalid (Rodin's static checker rejects it). rossi reports
    // EB021 (duplicate identifier) and EB022 (duplicate label) at Error
    // severity, so the run exits non-zero.
    let output = run_cli_with_stdin(
        &["validate", "--format", "json", "-"],
        "MACHINE M\nVARIABLES\n    x x\nINVARIANTS\n    @inv1 x >= 0\n    @inv1 x <= 5\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x := 0\n    END\nEND\n",
    );
    assert!(
        !output.status.success(),
        "duplicate identifier/label must fail validation"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"rule_id\": \"EB021\""),
        "expected EB021 (duplicate identifier) in JSON: {stdout}"
    );
    assert!(
        stdout.contains("\"rule_id\": \"EB022\""),
        "expected EB022 (duplicate label) in JSON: {stdout}"
    );
}

#[test]
fn validate_zip_bad_formula_reports_eb005() {
    // A formula attribute inside Rodin XML that the grammar rejects stays
    // EB005 — EB004 is reserved for whole-file Camille parse failures.
    assert_validate_zip_json_contains_rule(
        "rossi-cli-validate-eb005",
        "bad-formula.zip",
        "Bad.buc",
        br#"<?xml version="1.0" encoding="UTF-8"?>
<org.eventb.core.contextFile version="3">
    <org.eventb.core.constant name="c1" org.eventb.core.identifier="x"/>
    <org.eventb.core.axiom name="a1" org.eventb.core.label="axm1" org.eventb.core.predicate="x ==== ((("/>
</org.eventb.core.contextFile>"#,
        "EB005",
    );
}

fn assert_validate_zip_json_contains_rule(
    tmp_prefix: &str,
    zip_name: &str,
    entry_name: &str,
    entry_body: &[u8],
    expected_rule: &str,
) {
    let tmp = tempdir_unique(tmp_prefix);
    let zip_path = tmp.join(zip_name);
    write_zip(&zip_path, &[(entry_name, entry_body)]);

    let output = rossi_command()
        .args(["validate", "--format", "json", zip_path.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    assert!(
        !output.status.success(),
        "expected non-zero exit for {expected_rule}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let needle = format!("\"rule_id\": \"{expected_rule}\"");
    assert!(
        stdout.contains(&needle),
        "expected {expected_rule} in JSON: {stdout}"
    );

    std::fs::remove_dir_all(&tmp).ok();
}
