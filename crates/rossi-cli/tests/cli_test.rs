use std::io::Read;
use std::path::PathBuf;
use std::process::Command;

fn rossi_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rossi"))
}

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
fn test_fmt_stdin_inverse_operator_conversion() {
    // ASCII `~` is accepted on input; `fmt` emits Unicode ∼ (U+223C) and
    // `fmt --ascii` emits `~` (U+007E).
    let source = "CONTEXT test\nCONSTANTS\n    f r\nAXIOMS\n    @axm1 r = f~\nEND\n";

    let output = run_cli_with_stdin(&["fmt", "-"], source);
    assert!(
        output.status.success(),
        "fmt - should accept ASCII ~ inverse"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("f\u{223C}"),
        "Unicode fmt should emit ∼, got: {stdout}"
    );

    let output = run_cli_with_stdin(&["fmt", "--ascii", "-"], source);
    assert!(
        output.status.success(),
        "fmt --ascii should accept ASCII ~ inverse"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("f~"),
        "ASCII fmt should emit ~, got: {stdout}"
    );
    assert!(
        !stdout.contains('\u{223C}'),
        "ASCII fmt output must not contain U+223C"
    );
}

#[test]
fn test_cli_valid_context() {
    let output = rossi_command()
        .args(["validate", "../rossi/examples/counter.eventb"])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("✓ ../rossi/examples/counter.eventb"));
    assert!(stdout.contains("Valid Context 'counter_ctx'"));
}

#[test]
fn test_cli_valid_machine() {
    let output = rossi_command()
        .args(["validate", "../rossi/examples/counter_machine.eventb"])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("✓ ../rossi/examples/counter_machine.eventb"));
    assert!(stdout.contains("Valid Machine 'counter'"));
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
    assert!(stdout.contains("✓ ../rossi/examples/counter_machine.eventb"));
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
fn test_cli_json_output() {
    let output = rossi_command()
        .args([
            "validate",
            "--format",
            "json",
            "../rossi/examples/counter.eventb",
        ])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"file\": \"../rossi/examples/counter.eventb\""));
    assert!(stdout.contains("\"success\": true"));
    assert!(stdout.contains("\"component_type\": \"Context\""));
    assert!(stdout.contains("\"component_name\": \"counter_ctx\""));
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
fn test_cli_valid_zip_file() {
    let output = rossi_command()
        .args([
            "validate",
            "--no-semantic",
            "../rossi/examples/traffic-light.zip",
        ])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("✓ ../rossi/examples/traffic-light.zip:C1.buc"));
    assert!(stdout.contains("Valid Context 'C1'"));
    assert!(stdout.contains("✓ ../rossi/examples/traffic-light.zip:M0.bum"));
    assert!(stdout.contains("Valid Machine 'M0'"));
    assert!(stdout.contains("Summary:"));
    assert!(stdout.contains("Total:  4"));
    assert!(stdout.contains("Passed: 4 ✓"));
}

#[test]
fn test_cli_zip_file_json_output() {
    let output = rossi_command()
        .args([
            "validate",
            "--format",
            "json",
            "../rossi/examples/binary-search.zip",
        ])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"file\": \"../rossi/examples/binary-search.zip\""));
    assert!(stdout.contains("\"inner_filename\": \"C0.buc\""));
    assert!(stdout.contains("\"success\": true"));
    assert!(stdout.contains("\"component_type\": \"Context\""));
    assert!(stdout.contains("\"component_name\": \"C0\""));
    assert!(stdout.contains("\"inner_filename\": \"M0.bum\""));
    assert!(stdout.contains("\"component_name\": \"M0\""));
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
    // Check summary
    assert!(stdout.contains("Summary:"));
    assert!(stdout.contains("Total:  6"));
    assert!(stdout.contains("Passed: 6 ✓"));
}

/// A minimal machine whose variable `dead` is declared but never referenced
/// outside its typing invariant, so EB011 fires at its declaring level —
/// independent of any refinement-chain lint semantics (the bundled example
/// zips' kept-variable warnings depend on what inherited clauses count as
/// references). `dead` carries a typing invariant (an untyped variable is
/// an EB006 *Error* and would flip the exit code) but is deliberately never
/// assigned, so this machine stays warnings-only (EB011 dead + EB014 not
/// initialised). It SEES [`LINT_FIXTURE_BUC`] so the fixture also exercises
/// context loading and cross-file SEES resolution.
const LINT_FIXTURE_BUM: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.seesContext name="_s1" org.eventb.core.target="Ctx"/>
<org.eventb.core.variable name="_v1" org.eventb.core.identifier="x"/>
<org.eventb.core.variable name="_v2" org.eventb.core.identifier="dead"/>
<org.eventb.core.invariant name="_i1" org.eventb.core.label="inv1" org.eventb.core.predicate="x ∈ ℤ" org.eventb.core.theorem="false"/>
<org.eventb.core.invariant name="_i2" org.eventb.core.label="inv2" org.eventb.core.predicate="dead ∈ ℤ" org.eventb.core.theorem="false"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_a1" org.eventb.core.assignment="x ≔ lo" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;

/// The context [`LINT_FIXTURE_BUM`] sees. Its constant is referenced by its
/// own axiom (and the machine's INIT), so the context itself is warning-free.
const LINT_FIXTURE_BUC: &str = r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.constant name="_c1" org.eventb.core.identifier="lo"/>
<org.eventb.core.axiom name="_a1" org.eventb.core.label="axm1" org.eventb.core.predicate="lo ∈ ℤ" org.eventb.core.theorem="false"/>
</org.eventb.core.contextFile>
"#;

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

/// Write the lint fixture (machine + seen context) as a zip in a fresh temp
/// dir; returns `(tempdir, zip_path)` — remove the tempdir when done.
fn lint_fixture_zip(prefix: &str) -> (PathBuf, PathBuf) {
    let tmp = tempdir_unique(prefix);
    let zip_path = tmp.join("lint-fixture.zip");
    write_zip(
        &zip_path,
        &[
            ("Ctx.buc", LINT_FIXTURE_BUC.as_bytes()),
            ("Lint.bum", LINT_FIXTURE_BUM.as_bytes()),
        ],
    );
    (tmp, zip_path)
}

#[test]
fn validate_zip_lint_warning_exits_zero() {
    // The fixture machine leaves `dead` unreferenced, so EB011 fires.
    // Warnings must not flip the exit code.
    let (tmp, zip_path) = lint_fixture_zip("rossi-cli-lint-warn-zip");
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

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_no_lints_drops_lint_rows() {
    // Same model, but --no-lints disables the advisory passes. No EB011
    // rows should remain.
    let (tmp, zip_path) = lint_fixture_zip("rossi-cli-no-lints");
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
fn validate_directory_input() {
    // Validate a project given as a directory of Rodin files (the layout
    // Rodin uses on disk) — a context and a machine that SEES it, so the
    // directory path is checked for .buc loading and cross-file SEES
    // resolution, not just single-machine parsing.
    let tmp = tempdir_unique("rossi-cli-validate-dir");
    std::fs::write(tmp.join("Ctx.buc"), LINT_FIXTURE_BUC).unwrap();
    std::fs::write(tmp.join("Lint.bum"), LINT_FIXTURE_BUM).unwrap();

    let output = rossi_command()
        .args(["validate", tmp.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    assert!(
        output.status.success(),
        "directory validation should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Valid Context 'Ctx'"));
    assert!(stdout.contains("Valid Machine 'Lint'"));
    // Lint warnings still surface from the directory path.
    assert!(stdout.contains("[EB011]"));

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

const DUP_VARIABLE_MACHINE: &str = "MACHINE M\nVARIABLES\n    x x\nINVARIANTS\n    @inv1 x >= 0\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        x := 0\n    END\nEND\n";

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
fn import_rodin_buc_file_to_eventb() {
    let tmp = tempdir_unique("rossi-cli-import-buc");
    let out_dir = tmp.join("out");

    let output = rossi_command()
        .args([
            "import",
            "../rossi/examples/counter_ctx.buc",
            "-o",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");

    assert!(
        output.status.success(),
        "import .buc should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = std::fs::read_to_string(out_dir.join("counter_ctx.eventb")).unwrap();
    assert!(text.contains("CONTEXT counter_ctx"));

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn import_rodin_bum_file_to_eventb() {
    let tmp = tempdir_unique("rossi-cli-import-bum");
    let out_dir = tmp.join("out");

    let output = rossi_command()
        .args([
            "import",
            "../rossi/examples/counter.bum",
            "-o",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");

    assert!(
        output.status.success(),
        "import .bum should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = std::fs::read_to_string(out_dir.join("counter.eventb")).unwrap();
    assert!(text.contains("MACHINE counter"));

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn import_rodin_directory_to_eventb_files() {
    let tmp = tempdir_unique("rossi-cli-import-rodin-dir");
    let rodin_dir = tmp.join("rodin");
    let out_dir = tmp.join("out");
    std::fs::create_dir_all(&rodin_dir).unwrap();
    std::fs::copy(
        "../rossi/examples/counter_ctx.buc",
        rodin_dir.join("counter_ctx.buc"),
    )
    .unwrap();
    std::fs::copy(
        "../rossi/examples/counter.bum",
        rodin_dir.join("counter.bum"),
    )
    .unwrap();

    let output = rossi_command()
        .args([
            "import",
            rodin_dir.to_str().unwrap(),
            "-o",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");

    assert!(
        output.status.success(),
        "import Rodin dir should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out_dir.join("counter_ctx.eventb").exists());
    assert!(out_dir.join("counter.eventb").exists());

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn import_multi_project_archive_writes_per_project_subdirs() {
    // A machine reused under two sibling projects with the SAME component
    // basename ("M.bum") — the case the old flat import collapsed into one
    // overwritten output file.
    let tmp = tempdir_unique("rossi-cli-import-multi");
    let zip_path = tmp.join("decomp.zip");
    let out_dir = tmp.join("out");

    let machine_xml = std::fs::read("../rossi/examples/counter.bum").unwrap();
    let proj_a = project_descriptor("A");
    let proj_b = project_descriptor("B");
    write_zip(
        &zip_path,
        &[
            ("A/.project", &proj_a),
            ("A/M.bum", &machine_xml),
            ("B/.project", &proj_b),
            ("B/M.bum", &machine_xml),
        ],
    );

    let output = rossi_command()
        .args([
            "import",
            zip_path.to_str().unwrap(),
            "-o",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");
    assert!(
        output.status.success(),
        "multi-project import should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Each project's component lands under its own subdirectory (the component
    // is renamed to its file stem `M`); neither overwrites the other, and
    // nothing is written flat at the output root.
    assert!(out_dir.join("A").join("M.eventb").exists());
    assert!(out_dir.join("B").join("M.eventb").exists());
    assert!(!out_dir.join("M.eventb").exists());

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn import_keys_subdirs_on_prefix_not_colliding_name() {
    // Two sibling projects whose `.project` descriptors resolve to the SAME
    // name but sit under distinct archive directories. Keying output on the
    // unique prefix (not the resolved name) keeps them apart instead of one
    // overwriting the other.
    let tmp = tempdir_unique("rossi-cli-import-namecollide");
    let zip_path = tmp.join("decomp.zip");
    let out_dir = tmp.join("out");
    let machine_xml = std::fs::read("../rossi/examples/counter.bum").unwrap();
    // Both descriptors claim the same project name "Dup".
    let dup = project_descriptor("Dup");
    write_zip(
        &zip_path,
        &[
            ("A/.project", &dup),
            ("A/M.bum", &machine_xml),
            ("B/.project", &dup),
            ("B/N.bum", &machine_xml),
        ],
    );

    let output = rossi_command()
        .args([
            "import",
            zip_path.to_str().unwrap(),
            "-o",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Subdirs are the archive prefixes A/ and B/, not the colliding name "Dup".
    assert!(out_dir.join("A").join("M.eventb").exists());
    assert!(out_dir.join("B").join("N.eventb").exists());
    assert!(!out_dir.join("Dup").exists());

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn import_contains_path_traversal_project_name() {
    // A hostile archive whose project directory is `..` must not write outside
    // the chosen output directory; the segment is sanitized to a safe name.
    // Two distinct prefixes so multi-project (subdir) mode triggers; one tries
    // to escape via `../`.
    let tmp = tempdir_unique("rossi-cli-import-traversal");
    let zip_path = tmp.join("evil.zip");
    let out_dir = tmp.join("out");
    let machine_xml = std::fs::read("../rossi/examples/counter.bum").unwrap();
    write_zip(
        &zip_path,
        &[
            ("../escape/M.bum", &machine_xml),
            ("safe/N.bum", &machine_xml),
        ],
    );

    let output = rossi_command()
        .args([
            "import",
            zip_path.to_str().unwrap(),
            "-o",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The `../` project is neutralized to the safe fallback segment `project/`
    // inside out/, and nothing escapes to the output's parent.
    assert!(out_dir.join("safe").join("N.eventb").exists());
    assert!(out_dir.join("project").join("M.eventb").exists());
    assert!(
        !tmp.join("escape").exists(),
        "import escaped the output directory"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn export_eventb_file_to_zip() {
    let tmp = tempdir_unique("rossi-cli-export-eventb");
    let out_zip = tmp.join("out.zip");

    let output = rossi_command()
        .args([
            "export",
            "../rossi/examples/counter.eventb",
            "-o",
            out_zip.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");

    assert!(
        output.status.success(),
        "export should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out_zip.exists());

    let extracted = tmp.join("extracted");
    std::fs::create_dir_all(&extracted).unwrap();
    extract_zip_to(&out_zip, &extracted);
    let has_rodin = std::fs::read_dir(&extracted).unwrap().flatten().any(|e| {
        e.path()
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| x.eq_ignore_ascii_case("buc") || x.eq_ignore_ascii_case("bum"))
    });
    assert!(has_rodin, "expected a .buc/.bum entry in the exported zip");

    std::fs::remove_dir_all(&tmp).ok();
}

const ASCII_CONTEXT: &str = "CONTEXT c\nCONSTANTS\n    x\nAXIOMS\n    @axm1 x : NAT\nEND\n";

#[test]
fn fmt_ascii_text_to_unicode_stdout() {
    let tmp = tempdir_unique("rossi-cli-fmt-ascii");
    let file = tmp.join("c.eventb");
    std::fs::write(&file, ASCII_CONTEXT).unwrap();

    let output = rossi_command()
        .args(["fmt", file.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    assert!(
        output.status.success(),
        "fmt should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains('∈'), "expected Unicode ∈ in: {stdout}");
    assert!(stdout.contains('ℕ'), "expected Unicode ℕ in: {stdout}");

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn fmt_indent_option_changes_indentation() {
    let tmp = tempdir_unique("rossi-cli-fmt-indent");
    let file = tmp.join("c.eventb");
    std::fs::write(&file, ASCII_CONTEXT).unwrap();

    let output = rossi_command()
        .args(["fmt", "--indent", "  ", file.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");

    assert!(
        output.status.success(),
        "fmt --indent stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\n  @axm1"),
        "expected 2-space indentation in: {stdout}"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn fmt_check_then_in_place() {
    let tmp = tempdir_unique("rossi-cli-fmt-check");
    let file = tmp.join("c.eventb");
    std::fs::write(&file, ASCII_CONTEXT).unwrap();

    // --check on an ASCII file (canonical form is Unicode) flags it and exits non-zero.
    let checked = rossi_command()
        .args(["fmt", "--check", file.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");
    assert!(
        !checked.status.success(),
        "fmt --check should flag an unformatted file"
    );
    let check_out = String::from_utf8_lossy(&checked.stdout);
    assert!(
        check_out.contains("c.eventb"),
        "expected the path in --check output: {check_out}"
    );

    // -i rewrites the file in place to Unicode.
    let fixed = rossi_command()
        .args(["fmt", "-i", file.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");
    assert!(
        fixed.status.success(),
        "fmt -i stderr={}",
        String::from_utf8_lossy(&fixed.stderr)
    );
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(
        text.contains('∈'),
        "the in-place file should now use Unicode: {text}"
    );

    // --check now passes (exit 0).
    let recheck = rossi_command()
        .args(["fmt", "--check", file.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");
    assert!(
        recheck.status.success(),
        "fmt --check should pass after formatting; stderr={}",
        String::from_utf8_lossy(&recheck.stderr)
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn fmt_ascii_on_rodin_zip_is_rejected() {
    let output = rossi_command()
        .args(["fmt", "--ascii", "../rossi/examples/traffic-light.zip"])
        .output()
        .expect("Failed to execute command");
    assert!(
        !output.status.success(),
        "fmt --ascii on a Rodin zip should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unicode"),
        "expected a Unicode-required error: {stderr}"
    );
}

#[test]
fn fmt_normalizes_rodin_zip() {
    let tmp = tempdir_unique("rossi-cli-fmt-zip");
    let out_zip = tmp.join("norm.zip");

    let output = rossi_command()
        .args([
            "fmt",
            "../rossi/examples/traffic-light.zip",
            "-o",
            out_zip.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");

    assert!(
        output.status.success(),
        "fmt zip stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out_zip.exists());

    let extracted = tmp.join("extracted");
    std::fs::create_dir_all(&extracted).unwrap();
    extract_zip_to(&out_zip, &extracted);
    assert!(
        dir_has_rodin_file(&extracted),
        "expected .buc/.bum entries in the normalized zip"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn fmt_preserves_multi_project_archive_structure() {
    // A two-project archive with a component and a non-component (proof) entry
    // per project. fmt normalises the components under their original paths,
    // leaving the per-project layout and the proof bytes intact.
    let tmp = tempdir_unique("rossi-cli-fmt-multi");
    let in_zip = tmp.join("decomp.zip");
    let out_zip = tmp.join("out.zip");

    let machine_xml = std::fs::read("../rossi/examples/counter.bum").unwrap();
    let proofs = [("A", b"PROOF-A".to_vec()), ("B", b"PROOF-B".to_vec())];
    let proj_a = project_descriptor("A");
    let proj_b = project_descriptor("B");
    write_zip(
        &in_zip,
        &[
            ("A/.project", &proj_a),
            ("A/M.bum", &machine_xml),
            ("A/M.bpr", &proofs[0].1),
            ("B/.project", &proj_b),
            ("B/M.bum", &machine_xml),
            ("B/M.bpr", &proofs[1].1),
        ],
    );

    let output = rossi_command()
        .args([
            "fmt",
            in_zip.to_str().unwrap(),
            "-o",
            out_zip.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");
    assert!(
        output.status.success(),
        "multi-project fmt should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let entry_names = |path: &std::path::Path| -> Vec<String> {
        let mut a = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut names: Vec<String> = (0..a.len())
            .map(|i| a.by_index(i).unwrap().name().to_string())
            .collect();
        names.sort();
        names
    };
    // The per-project layout (every prefix) survives unchanged.
    assert_eq!(entry_names(&in_zip), entry_names(&out_zip));

    // Non-component proof entries are byte-identical; components stay valid XML.
    let mut out = zip::ZipArchive::new(std::fs::File::open(&out_zip).unwrap()).unwrap();
    for (proj, proof) in &proofs {
        let mut buf = Vec::new();
        out.by_name(&format!("{proj}/M.bpr"))
            .unwrap()
            .read_to_end(&mut buf)
            .unwrap();
        assert_eq!(&buf, proof, "proof entry must be preserved verbatim");
        let mut xml = String::new();
        out.by_name(&format!("{proj}/M.bum"))
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        assert!(
            xml.contains("machineFile"),
            "component should remain a machine file"
        );
    }

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn fmt_raw_copies_non_component_entries() {
    let tmp = tempdir_unique("rossi-cli-fmt-raw-copy");
    let in_zip = tmp.join("input.zip");
    let out_zip = tmp.join("output.zip");
    let timestamp = zip::DateTime::from_date_and_time(2024, 2, 6, 12, 34, 56).unwrap();
    let machine_xml = std::fs::read("../rossi/examples/counter.bum").unwrap();

    let file = std::fs::File::create(&in_zip).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    writer
        .set_raw_comment(b"archive comment".to_vec().into_boxed_slice())
        .unwrap();
    let directory_options = zip::write::SimpleFileOptions::default()
        .last_modified_time(timestamp)
        .unix_permissions(0o750)
        .into_full_options()
        .with_file_comment("directory comment");
    writer
        .add_directory("project/proofs/", directory_options)
        .unwrap();
    let proof_options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(timestamp)
        .unix_permissions(0o640)
        .into_full_options()
        .with_file_comment("proof comment");
    writer
        .start_file("project/proofs/M.bpr", proof_options)
        .unwrap();
    std::io::Write::write_all(&mut writer, b"retained proof payload").unwrap();
    writer
        .start_file("project/M.bum", zip::write::SimpleFileOptions::default())
        .unwrap();
    std::io::Write::write_all(&mut writer, &machine_xml).unwrap();
    writer.finish().unwrap();

    let output = rossi_command()
        .args([
            "fmt",
            in_zip.to_str().unwrap(),
            "-o",
            out_zip.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");
    assert!(
        output.status.success(),
        "fmt zip stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let input = std::fs::read(&in_zip).unwrap();
    let output = std::fs::read(&out_zip).unwrap();
    let input_archive = zip::ZipArchive::new(std::io::Cursor::new(&input)).unwrap();
    let output_archive = zip::ZipArchive::new(std::io::Cursor::new(&output)).unwrap();
    assert_eq!(output_archive.comment(), input_archive.comment());
    assert_eq!(
        zip_entry_snapshot(&output, "project/proofs/"),
        zip_entry_snapshot(&input, "project/proofs/")
    );
    assert_eq!(
        zip_entry_snapshot(&output, "project/proofs/M.bpr"),
        zip_entry_snapshot(&input, "project/proofs/M.bpr")
    );

    std::fs::remove_dir_all(&tmp).ok();
}

const MINIMAL_BUILD_CONTEXT_XML: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
    <org.eventb.core.contextFile version=\"3\" \
    org.eventb.core.configuration=\"org.eventb.core.fwd\"></org.eventb.core.contextFile>\n";

struct BuildFixture {
    root: PathBuf,
    output: PathBuf,
}

impl BuildFixture {
    fn new(entries: &[&str], output: &str) -> Self {
        let root = tempdir_unique("rossi-cli-build-output-paths");
        let input = root.join("input.zip");
        let output = root.join(output);
        let entries: Vec<_> = entries
            .iter()
            .map(|name| (*name, MINIMAL_BUILD_CONTEXT_XML.as_bytes()))
            .collect();
        write_zip(&input, &entries);
        Self { root, output }
    }

    fn run(&self) -> std::process::Output {
        rossi_command()
            .args([
                "build",
                self.root.join("input.zip").to_str().unwrap(),
                "-o",
                self.output.to_str().unwrap(),
            ])
            .output()
            .expect("Failed to execute command")
    }

    fn assert_success(&self, case: &str) {
        let output = self.run();
        assert!(
            output.status.success(),
            "{case} should succeed; stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

impl Drop for BuildFixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).ok();
    }
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

fn dir_has_rodin_file(dir: &std::path::Path) -> bool {
    dir_has_ext(dir, &["buc", "bum"])
}

/// Recursively check whether `dir` contains a file whose extension matches one
/// of `exts` (case-insensitive).
fn dir_has_ext(dir: &std::path::Path, exts: &[&str]) -> bool {
    std::fs::read_dir(dir).unwrap().flatten().any(|e| {
        let p = e.path();
        if p.is_dir() {
            return dir_has_ext(&p, exts);
        }
        p.extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| exts.iter().any(|want| x.eq_ignore_ascii_case(want)))
    })
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

#[test]
fn export_eventb_to_rodin_zip_includes_project_descriptor() {
    let tmp = tempdir_unique("rossi-cli-export-project-zip");
    let out_zip = tmp.join("counter project.zip");

    let output = rossi_command()
        .args([
            "export",
            "../rossi/examples/counter.eventb",
            "-o",
            out_zip.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");

    assert!(
        output.status.success(),
        "export .eventb should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let file = std::fs::File::open(&out_zip).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let project_xml = {
        let mut project = archive.by_name(".project").unwrap();
        let mut project_xml = String::new();
        project.read_to_string(&mut project_xml).unwrap();
        project_xml
    };
    // Descriptor *content* (nature, builder, XML escaping) is covered by the
    // rossi lib tests; here we only check the CLI wiring: a .project named
    // after the output stem, plus the component, both landed in the zip.
    assert!(project_xml.contains("<name>counter project</name>"));
    archive.by_name("counter_ctx.buc").unwrap();

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn export_eventb_to_rodin_directory_includes_project_descriptor() {
    let tmp = tempdir_unique("rossi-cli-export-project-dir");
    let out_dir = tmp.join("counter project");

    let output = rossi_command()
        .args([
            "export",
            "../rossi/examples/counter.eventb",
            "-o",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");

    assert!(
        output.status.success(),
        "export .eventb to directory should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Descriptor *content* is covered by the rossi lib tests; here we only check
    // the CLI wiring: a .project named after the output stem, plus the
    // component, both landed in the directory.
    let project_xml = std::fs::read_to_string(out_dir.join(".project")).unwrap();
    assert!(project_xml.contains("<name>counter project</name>"));
    assert!(out_dir.join("counter_ctx.buc").exists());

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn export_directory_of_subprojects_to_multi_project_zip() {
    // A directory whose Event-B text lives only under immediate subdirectories
    // exports as one Rodin project per subdirectory (the inverse of a
    // multi-project import). Each project gets its own `<name>/` prefix and
    // `.project`, so sibling components sharing a basename never collide.
    let tmp = tempdir_unique("rossi-cli-export-multi");
    let src = tmp.join("src");
    for (proj, comp, body) in [
        ("ProjA", "shared.eventb", "CONTEXT shared\nEND\n"),
        ("ProjB", "shared.eventb", "MACHINE shared\nEND\n"),
    ] {
        let dir = src.join(proj);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(comp), body).unwrap();
    }
    let out_zip = tmp.join("out.zip");

    let output = rossi_command()
        .args([
            "export",
            src.to_str().unwrap(),
            "-o",
            out_zip.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");
    assert!(
        output.status.success(),
        "multi-project export should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut archive = zip::ZipArchive::new(std::fs::File::open(&out_zip).unwrap()).unwrap();
    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    // The colliding `shared` component is kept apart under each project prefix.
    for expected in [
        "ProjA/.project",
        "ProjA/shared.buc",
        "ProjB/.project",
        "ProjB/shared.bum",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "expected {expected} in {names:?}"
        );
    }
    let mut descriptor = String::new();
    archive
        .by_name("ProjA/.project")
        .unwrap()
        .read_to_string(&mut descriptor)
        .unwrap();
    assert!(descriptor.contains("<name>ProjA</name>"));

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn export_stray_top_level_txt_still_splits_subprojects() {
    // A benign generic .txt (README/notes) directly under the source directory
    // must NOT collapse the per-subdirectory project split — only a definite
    // `.eventb` source does.
    let tmp = tempdir_unique("rossi-cli-export-strawtxt");
    let src = tmp.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("README.txt"), "just notes, not Event-B\n").unwrap();
    for (proj, body) in [("ProjA", "CONTEXT a\nEND\n"), ("ProjB", "MACHINE b\nEND\n")] {
        let dir = src.join(proj);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("c.eventb"), body).unwrap();
    }
    let out_zip = tmp.join("out.zip");

    let output = rossi_command()
        .args([
            "export",
            src.to_str().unwrap(),
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

    let mut archive = zip::ZipArchive::new(std::fs::File::open(&out_zip).unwrap()).unwrap();
    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    // Both subdirectories became their own project despite the stray README.txt.
    assert!(
        names.iter().any(|n| n == "ProjA/.project"),
        "names={names:?}"
    );
    assert!(
        names.iter().any(|n| n == "ProjB/.project"),
        "names={names:?}"
    );

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
        "MACHINE M\nVARIABLES\n    x x\nINVARIANTS\n    @inv1 x >= 0\n    @inv1 x <= 5\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        x := 0\n    END\nEND\n",
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

// ---- helpers ---------------------------------------------------------------

fn write_zip(zip_path: &std::path::Path, entries: &[(&str, &[u8])]) {
    let file =
        std::fs::File::create(zip_path).unwrap_or_else(|e| panic!("create {zip_path:?}: {e}"));
    let mut zw = zip::ZipWriter::new(file);
    let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
    for (name, body) in entries {
        zw.start_file(*name, opts).unwrap();
        std::io::Write::write_all(&mut zw, body).unwrap();
    }
    zw.finish().unwrap();
}

fn zip_entry_snapshot(
    bytes: &[u8],
    name: &str,
) -> (
    zip::CompressionMethod,
    Option<zip::DateTime>,
    Option<u32>,
    String,
    bool,
    Vec<u8>,
) {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let entry = archive.by_name(name).unwrap();
    let start = entry.data_start().unwrap() as usize;
    let end = start + entry.compressed_size() as usize;
    (
        entry.compression(),
        entry.last_modified(),
        entry.unix_mode(),
        entry.comment().to_string(),
        entry.is_dir(),
        bytes[start..end].to_vec(),
    )
}

/// The bytes of a minimal Rodin `.project` descriptor naming `name`.
fn project_descriptor(name: &str) -> Vec<u8> {
    format!("<projectDescription><name>{name}</name></projectDescription>").into_bytes()
}

fn tempdir_unique(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn extract_zip_to(zip_path: &PathBuf, dest: &std::path::Path) {
    let file = std::fs::File::open(zip_path).unwrap_or_else(|e| panic!("open {zip_path:?}: {e}"));
    let mut archive = zip::ZipArchive::new(file).unwrap();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        if entry.is_dir() {
            continue;
        }
        let out = dest.join(entry.name());
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).unwrap();
        std::fs::write(out, buf).unwrap();
    }
}

/// Run the CLI with `stdin_data` piped to its standard input.
fn run_cli_with_stdin(args: &[&str], stdin_data: &str) -> std::process::Output {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = rossi_command()
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn rossi-cli");
    child
        .stdin
        .as_mut()
        .expect("child stdin")
        .write_all(stdin_data.as_bytes())
        .expect("write stdin");
    // `wait_with_output` closes stdin (signalling EOF) before collecting output.
    child.wait_with_output().expect("wait for rossi-cli")
}

/// Run structured validation after closing the read end of its stdout pipe.
fn run_validate_with_closed_stdout(format: &str, stdin_data: &str) -> std::process::Output {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = rossi_command()
        .args(["validate", "--format", format, "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn rossi-cli");
    drop(child.stdout.take());
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(stdin_data.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait for rossi-cli")
}

#[test]
fn validate_structured_output_handles_broken_pipe_without_panicking() {
    for format in ["json", "sarif"] {
        let output = run_validate_with_closed_stdout(format, ASCII_CONTEXT);
        assert!(
            output.status.success(),
            "valid {format} validation should ignore BrokenPipe; stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains("panicked"),
            "{format} output must not panic on BrokenPipe"
        );

        let output = run_validate_with_closed_stdout(format, "CONTEXT");
        assert!(
            !output.status.success(),
            "BrokenPipe must preserve failed {format} validation status"
        );
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains("panicked"),
            "failed {format} validation must not panic on BrokenPipe"
        );
    }
}

#[test]
fn fmt_stdin_text_to_unicode() {
    let output = run_cli_with_stdin(&["fmt", "-"], ASCII_CONTEXT);
    assert!(
        output.status.success(),
        "fmt - should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains('∈'), "expected Unicode ∈ in: {stdout}");
    assert!(stdout.contains('ℕ'), "expected Unicode ℕ in: {stdout}");
}

#[test]
fn validate_stdin_uses_stdin_filename() {
    let output = run_cli_with_stdin(
        &[
            "validate",
            "--format",
            "json",
            "--stdin-filename",
            "foo.eventb",
            "-",
        ],
        ASCII_CONTEXT,
    );
    assert!(
        output.status.success(),
        "validate - should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"file\": \"foo.eventb\""),
        "expected the stdin filename in JSON: {stdout}"
    );
    assert!(
        stdout.contains("\"success\": true"),
        "expected a successful parse: {stdout}"
    );
}

#[test]
fn export_stdin_to_zip() {
    let tmp = tempdir_unique("rossi-cli-export-stdin");
    let out_zip = tmp.join("out.zip");

    let output = run_cli_with_stdin(
        &["export", "-", "-o", out_zip.to_str().unwrap()],
        ASCII_CONTEXT,
    );
    assert!(
        output.status.success(),
        "export - should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let extracted = tmp.join("extracted");
    std::fs::create_dir_all(&extracted).unwrap();
    extract_zip_to(&out_zip, &extracted);
    assert!(
        dir_has_rodin_file(&extracted),
        "expected a .buc/.bum entry in the exported zip"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

use rossi::MAX_NESTING_DEPTH;

/// A context whose single axiom nests parentheses `n` deep.
fn nested_paren_context(n: usize) -> String {
    format!(
        "context C axioms @a {}x{} = 1 end",
        "(".repeat(n),
        ")".repeat(n)
    )
}

#[test]
fn validate_stdin_at_nesting_limit_succeeds() {
    // Runs the full validate pipeline in a debug build — proves the parser
    // stack headroom covers the depth limit end to end.
    let output = run_cli_with_stdin(&["validate", "-"], &nested_paren_context(MAX_NESTING_DEPTH));
    assert!(
        output.status.success(),
        "validate - at the nesting limit should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_stdin_over_nesting_limit_reports_error_not_crash() {
    // Used to die with SIGABRT ("has overflowed its stack"); must now exit
    // with an ordinary diagnostic.
    let output = run_cli_with_stdin(&["validate", "-"], &nested_paren_context(5000));
    assert!(
        output.status.code().is_some(),
        "validate - must not be killed by a signal"
    );
    assert!(!output.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("nesting exceeds the maximum depth"),
        "expected a NestingTooDeep diagnostic, got: {combined}"
    );
}

#[test]
fn fmt_stdin_at_nesting_limit_succeeds() {
    let output = run_cli_with_stdin(&["fmt", "-"], &nested_paren_context(MAX_NESTING_DEPTH));
    assert!(
        output.status.success(),
        "fmt - at the nesting limit should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_stdin_at_limit_negation_chain_succeeds() {
    // Unlike parens (which collapse in the AST), a negation chain stays
    // nested all the way through the static checks — this exercises the
    // downstream consumers at depth in a debug build.
    let source = format!(
        "context C axioms @a {}(1=1) end",
        "¬".repeat(MAX_NESTING_DEPTH - 1)
    );
    let output = run_cli_with_stdin(&["validate", "-"], &source);
    assert!(
        output.status.code().is_some(),
        "validate - must not be killed by a signal"
    );
    assert!(
        output.status.success(),
        "validate - at-limit negation chain should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_completions_emits_script() {
    // Smoke test: the subcommand is wired up (enum variant + dispatch) and
    // renders a non-empty completion script for the `rossi` binary. The script
    // body is clap_complete's to produce, so we only check it ran and named the
    // right binary — `_rossi` (the function) plus the `complete` builtin prove
    // a real bash script was emitted for `rossi`.
    let output = rossi_command()
        .args(["completions", "bash"])
        .output()
        .expect("Failed to execute command");

    assert!(
        output.status.success(),
        "completions bash should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("_rossi"), "bash output: {stdout}");
    assert!(stdout.contains("complete"), "bash output: {stdout}");
}
