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
fn print_rodin_buc_file_to_eventb() {
    let tmp = tempdir_unique("rossi-cli-print-buc");
    let out_dir = tmp.join("out");

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
            "print",
            "../rossi/examples/counter_ctx.buc",
            "-o",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");

    assert!(
        output.status.success(),
        "print .buc should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = std::fs::read_to_string(out_dir.join("counter_ctx.eventb")).unwrap();
    assert!(text.contains("CONTEXT counter_ctx"));

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn print_rodin_bum_file_to_eventb() {
    let tmp = tempdir_unique("rossi-cli-print-bum");
    let out_dir = tmp.join("out");

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "rossi-cli",
            "--",
            "print",
            "../rossi/examples/counter.bum",
            "-o",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");

    assert!(
        output.status.success(),
        "print .bum should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = std::fs::read_to_string(out_dir.join("counter.eventb")).unwrap();
    assert!(text.contains("MACHINE counter"));

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn print_rodin_directory_to_eventb_files() {
    let tmp = tempdir_unique("rossi-cli-print-rodin-dir");
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
            "print",
            rodin_dir.to_str().unwrap(),
            "-o",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");

    assert!(
        output.status.success(),
        "print Rodin dir should exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out_dir.join("counter_ctx.eventb").exists());
    assert!(out_dir.join("counter.eventb").exists());

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
