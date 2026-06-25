use std::io::Read;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn test_cli_help() {
    let output = Command::new("cargo")
        .args(["run", "-p", "rossi-cli", "--", "validate", "--help"])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Validate Event-B model files"));
    assert!(stdout.contains("Usage: rossi validate"));
}

#[test]
fn test_cli_version() {
    let output = Command::new("cargo")
        .args(["run", "-p", "rossi-cli", "--", "validate", "--version"])
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
    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
            "validate",
            "../rossi/examples/counter.eventb",
        ])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("✓ ../rossi/examples/counter.eventb"));
    assert!(stdout.contains("Valid Context 'counter_ctx'"));
}

#[test]
fn test_cli_valid_machine() {
    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
            "validate",
            "../rossi/examples/counter_machine.eventb",
        ])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("✓ ../rossi/examples/counter_machine.eventb"));
    assert!(stdout.contains("Valid Machine 'counter'"));
}

#[test]
fn test_cli_multiple_files() {
    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
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
    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
            "validate",
            "nonexistent.eventb",
        ])
        .output()
        .expect("Failed to execute command");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("✗ nonexistent.eventb"));
    assert!(stderr.contains("File not found"));
}

#[test]
fn test_cli_json_output() {
    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
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
    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
            "validate",
            "--quiet",
            "../rossi/examples/counter.eventb",
        ])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // In quiet mode, successful validations should produce no output
    assert!(!stdout.contains("✓"));
}

#[test]
fn test_cli_quiet_mode_with_error() {
    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
            "validate",
            "--quiet",
            "nonexistent.eventb",
        ])
        .output()
        .expect("Failed to execute command");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // In quiet mode, errors should still be shown
    assert!(stderr.contains("✗ nonexistent.eventb"));
}

#[test]
fn test_cli_no_files_provided() {
    let output = Command::new("cargo")
        .args(["run", "-p", "rossi-cli", "--", "validate"])
        .output()
        .expect("Failed to execute command");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Should show error about missing FILE argument
    assert!(stderr.contains("FILE") || stderr.contains("required"));
}

#[test]
fn test_cli_valid_zip_file() {
    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
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
    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
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

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
            "validate",
            "--format",
            "json",
            zip_path.to_str().unwrap(),
        ])
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

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
            "validate",
            "--format",
            "json",
            zip_path.to_str().unwrap(),
        ])
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
    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
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

#[test]
fn validate_zip_lint_warning_exits_zero() {
    // binary-search.zip leaves variable `r` unreferenced after refinement,
    // so EB011 fires. Warnings must not flip the exit code.
    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
            "validate",
            "../rossi/examples/binary-search.zip",
        ])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success(), "warning-only run should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[EB011]"),
        "expected EB011 in stdout: {stdout}"
    );
}

#[test]
fn validate_no_lints_drops_lint_rows() {
    // Same model, but --no-lints disables the advisory passes. No EB011
    // rows should remain.
    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
            "validate",
            "--no-lints",
            "../rossi/examples/binary-search.zip",
        ])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("EB011"),
        "EB011 should be suppressed under --no-lints: {stdout}"
    );
}

#[test]
fn validate_json_includes_rule_id_for_lint() {
    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
            "validate",
            "--format",
            "json",
            "../rossi/examples/binary-search.zip",
        ])
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
}

#[test]
fn validate_sarif_output_is_valid() {
    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
            "validate",
            "--format",
            "sarif",
            "../rossi/examples/binary-search.zip",
        ])
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
    // Extract binary-search.zip into a temp dir and validate the directory.
    // The archive nests its files under a `binary-search/` folder, so we
    // pass that to validate (matching the layout Rodin uses on disk).
    let zip_path = PathBuf::from("../rossi/examples/binary-search.zip");
    let tmp = tempdir_unique("rossi-cli-validate-dir");
    extract_zip_to(&zip_path, &tmp);
    let project_dir = first_subdir_with_rodin_files(&tmp);

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
            "validate",
            project_dir.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");

    assert!(
        output.status.success(),
        "directory validation should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Valid Context 'C0'"));
    assert!(stdout.contains("Valid Machine 'M0'"));
    // Lint warnings still surface from the directory path.
    assert!(stdout.contains("[EB011]"));

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn validate_directory_with_no_semantic_is_rejected() {
    let tmp = tempdir_unique("rossi-cli-validate-dir-nosem");
    std::fs::create_dir_all(&tmp).unwrap();

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
            "validate",
            "--no-semantic",
            tmp.to_str().unwrap(),
        ])
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

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
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

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
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

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
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

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
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

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
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

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
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

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
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

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
            "fmt",
            file.to_str().unwrap(),
        ])
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

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
            "fmt",
            "--indent",
            "  ",
            file.to_str().unwrap(),
        ])
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
    let checked = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
            "fmt",
            "--check",
            file.to_str().unwrap(),
        ])
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
    let fixed = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
            "fmt",
            "-i",
            file.to_str().unwrap(),
        ])
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
    let recheck = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
            "fmt",
            "--check",
            file.to_str().unwrap(),
        ])
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
    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
            "fmt",
            "--ascii",
            "../rossi/examples/traffic-light.zip",
        ])
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

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
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

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
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

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
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

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
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

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
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

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
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

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
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

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
            "validate",
            "--format",
            "json",
            zip_path.to_str().unwrap(),
        ])
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

/// Walk down from `root` until we hit a directory that directly contains
/// at least one `.buc` or `.bum`. Mirrors what Rodin projects look like
/// once unzipped (the archive usually has a single top-level folder).
fn first_subdir_with_rodin_files(root: &std::path::Path) -> PathBuf {
    let mut cur = root.to_path_buf();
    loop {
        let entries: Vec<_> = std::fs::read_dir(&cur)
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        let has_rodin = entries.iter().any(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x == "buc" || x == "bum")
        });
        if has_rodin {
            return cur;
        }
        let only_dir = entries
            .iter()
            .filter(|e| e.path().is_dir())
            .collect::<Vec<_>>();
        if only_dir.len() == 1 {
            cur = only_dir[0].path();
        } else {
            return cur;
        }
    }
}

/// Run the CLI with `stdin_data` piped to its standard input.
fn run_cli_with_stdin(args: &[&str], stdin_data: &str) -> std::process::Output {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new("cargo")
        .args(["run", "-p", "rossi-cli", "--"])
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
