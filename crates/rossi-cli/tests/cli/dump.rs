//! `rossi dump`: the document, its exit codes, and where it goes.

use std::path::{Path, PathBuf};

use crate::helpers::{
    MINIMAL_BUILD_CONTEXT_XML, rossi_command, run_cli, tempdir_unique, write_zip,
};

/// A context and a machine that check cleanly.
const CONTEXT: &str = "\
CONTEXT c
SETS
    S
CONSTANTS
    k
AXIOMS
    @axm1 k : S
END
";

const MACHINE: &str = "\
MACHINE m
SEES
    c
VARIABLES
    v
INVARIANTS
    @inv1 v : S
EVENTS
    EVENT INITIALISATION
    THEN
        @act1 v := k
    END
END
";

/// A machine naming something that was never declared, so checking fails.
const BROKEN: &str = "\
MACHINE broken
VARIABLES
    v
INVARIANTS
    @inv1 v : undeclared
EVENTS
    EVENT INITIALISATION
    THEN
        @act1 v := 0
    END
END
";

fn fixture_dir(prefix: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = tempdir_unique(prefix);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, body) in files {
        std::fs::write(dir.join(name), body).unwrap();
    }
    dir
}

fn dump(args: &[&str]) -> std::process::Output {
    let mut all = vec!["dump"];
    all.extend_from_slice(args);
    run_cli(&all)
}

fn parse(output: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "the document should parse: {e}; stdout={}, stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn a_clean_project_dumps_and_exits_zero() {
    let dir = fixture_dir(
        "rossi-cli-dump-clean",
        &[("c.eventb", CONTEXT), ("m.eventb", MACHINE)],
    );
    let output = dump(&[dir.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0));
    let document = parse(&output);
    assert_eq!(document["format"], "rossi-model");
    assert_eq!(document["version"], 1);
    assert_eq!(document["generator"]["name"], "rossi");
    assert_eq!(document["contexts"][0]["name"], "c");
    assert_eq!(document["machines"][0]["name"], "m");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_dumped_handle_is_the_one_a_build_writes() {
    // The loader reads Event-B text directly so spans survive, where `build`
    // routes it through an archive. This pins the compensation that keeps the
    // handles identical anyway: components take their Rodin names, and the
    // project name is normalised the way an archive descriptor normalises it.
    let dir = fixture_dir(
        "rossi-cli-dump-handles",
        &[("c.eventb", CONTEXT), ("m.eventb", MACHINE)],
    );
    let name = dir.file_name().unwrap().to_str().unwrap().to_string();
    let document = parse(&dump(&[dir.to_str().unwrap()]));

    assert_eq!(
        document["contexts"][0]["source"],
        format!("/{name}/c.buc|org.eventb.core.contextFile#c")
    );
    assert_eq!(
        document["machines"][0]["source"],
        format!("/{name}/m.bum|org.eventb.core.machineFile#m")
    );
    // The source list still says where the text really came from.
    let sources = document["sources"].as_array().unwrap();
    let context = sources.iter().find(|s| s["id"] == "c.buc").unwrap();
    assert!(
        context["path"].as_str().unwrap().ends_with("c.eventb"),
        "path was {}",
        context["path"]
    );
    assert_eq!(context["kind"], "eventb");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_single_text_file_dumps() {
    let dir = fixture_dir("rossi-cli-dump-single", &[("c.eventb", CONTEXT)]);
    let output = dump(&[dir.join("c.eventb").to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(parse(&output)["contexts"][0]["name"], "c");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_rejected_project_still_writes_a_document_and_exits_one() {
    let dir = fixture_dir("rossi-cli-dump-broken", &[("broken.eventb", BROKEN)]);
    let output = dump(&[dir.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(1));
    let document = parse(&output);
    let errors: Vec<_> = document["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["severity"] == "error")
        .collect();
    assert!(!errors.is_empty(), "expected an error diagnostic");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_output_file_takes_the_document_and_leaves_stdout_empty() {
    let dir = fixture_dir("rossi-cli-dump-output", &[("c.eventb", CONTEXT)]);
    let out = dir.join("model.json");
    let output = dump(&[
        dir.join("c.eventb").to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ]);

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty(), "stdout should be empty");
    let written = std::fs::read_to_string(&out).unwrap();
    let document: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(document["format"], "rossi-model");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_document_is_one_line_unless_asked_to_be_read() {
    // A document is written for a tool, and indenting one costs several times
    // the bytes, so the plain form is the compact one.
    let dir = fixture_dir("rossi-cli-dump-shape", &[("c.eventb", CONTEXT)]);
    let path = dir.join("c.eventb");

    let plain = dump(&[path.to_str().unwrap()]);
    assert_eq!(plain.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&plain.stdout);
    assert_eq!(stdout.lines().count(), 1, "default output: {stdout}");

    let indented = dump(&[path.to_str().unwrap(), "--pretty"]);
    assert_eq!(indented.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&indented.stdout);
    assert!(stdout.lines().count() > 1, "indented output: {stdout}");

    // Both spell the same document.
    let plain: serde_json::Value = serde_json::from_slice(&plain.stdout).expect("plain parses");
    let indented: serde_json::Value =
        serde_json::from_slice(&indented.stdout).expect("indented parses");
    assert_eq!(plain, indented);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_schema_is_printed_on_request() {
    let output = dump(&["--schema"]);

    assert_eq!(output.status.code(), Some(0));
    let schema: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(schema["title"], "rossi-model");
    assert!(schema["$schema"].is_string());
}

#[test]
fn an_archive_of_several_projects_needs_a_choice() {
    let dir = tempdir_unique("rossi-cli-dump-multi");
    std::fs::create_dir_all(&dir).unwrap();
    let zip = dir.join("two.zip");
    write_zip(
        &zip,
        &[
            ("one/c.buc", MINIMAL_BUILD_CONTEXT_XML.as_bytes()),
            ("two/c.buc", MINIMAL_BUILD_CONTEXT_XML.as_bytes()),
        ],
    );

    let ambiguous = dump(&[zip.to_str().unwrap()]);
    assert_eq!(ambiguous.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&ambiguous.stderr);
    assert!(stderr.contains("--project"), "stderr: {stderr}");
    assert!(
        stderr.contains("one") && stderr.contains("two"),
        "stderr: {stderr}"
    );

    let chosen = dump(&[zip.to_str().unwrap(), "--project", "one"]);
    assert_eq!(chosen.status.code(), Some(0));

    let unknown = dump(&[zip.to_str().unwrap(), "--project", "three"]);
    assert_eq!(unknown.status.code(), Some(2));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_missing_input_is_a_usage_error() {
    let output = dump(&["definitely-not-here.eventb"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "nothing should reach stdout");
}

#[test]
fn an_unsupported_extension_is_reported() {
    let dir = fixture_dir("rossi-cli-dump-ext", &[("notes.md", "not a model")]);
    let output = dump(&[dir.join("notes.md").to_str().unwrap()]);

    assert_ne!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty(), "nothing should reach stdout");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_rodin_project_directory_dumps_without_spans() {
    // Rodin XML carries no source text, so a document built from it names no
    // positions rather than reporting offsets into an attribute string.
    let dir = fixture_dir("rossi-cli-dump-xml", &[]);
    std::fs::write(dir.join("c.buc"), MINIMAL_BUILD_CONTEXT_XML).unwrap();
    let output = dump(&[dir.to_str().unwrap()]);

    assert_eq!(output.status.code(), Some(0));
    let document = parse(&output);
    assert_eq!(document["sources"][0]["kind"], "buc");
    assert!(document["contexts"][0].get("span").is_none());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_document_is_the_same_on_every_run() {
    let dir = fixture_dir(
        "rossi-cli-dump-stable",
        &[("c.eventb", CONTEXT), ("m.eventb", MACHINE)],
    );
    let path = dir.to_str().unwrap();
    let first = dump(&[path]);
    let second = dump(&[path]);

    assert_eq!(first.stdout, second.stdout);
    std::fs::remove_dir_all(&dir).ok();
}

/// The command is wired into the binary, not merely compiled.
#[test]
fn the_subcommand_is_listed_in_help() {
    let output = rossi_command().arg("--help").output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("dump"), "help: {stdout}");
}

/// Directories are made under the system temp root, so a helper that assumes
/// one exists is checked here rather than failing obscurely later.
#[test]
fn the_fixture_helper_makes_a_usable_directory() {
    let dir = fixture_dir("rossi-cli-dump-helper", &[("c.eventb", CONTEXT)]);
    assert!(Path::new(&dir).join("c.eventb").exists());
    std::fs::remove_dir_all(&dir).ok();
}

/// A second context and machine, independent of `c` and `m`, so a selection
/// has something to leave out.
const OTHER_CONTEXT: &str = "\
CONTEXT other
SETS
    T
END
";

const OTHER_MACHINE: &str = "\
MACHINE n
SEES
    other
VARIABLES
    w
INVARIANTS
    @inv1 w : T
EVENTS
    EVENT INITIALISATION
    THEN
        @act1 w :: T
    END
END
";

fn branched(prefix: &str) -> PathBuf {
    fixture_dir(
        prefix,
        &[
            ("c.eventb", CONTEXT),
            ("m.eventb", MACHINE),
            ("other.eventb", OTHER_CONTEXT),
            ("n.eventb", OTHER_MACHINE),
        ],
    )
}

#[test]
fn a_component_selection_keeps_its_closure_and_drops_the_rest() {
    let dir = branched("rossi-cli-dump-component");
    let output = dump(&[dir.to_str().unwrap(), "--component", "m"]);

    assert_eq!(output.status.code(), Some(0));
    let document = parse(&output);
    let machines: Vec<&str> = document["machines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    let contexts: Vec<&str> = document["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();

    assert_eq!(machines, ["m"]);
    // The context `m` sees comes along; the unrelated one does not.
    assert_eq!(contexts, ["c"]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn several_component_selections_accumulate() {
    let dir = branched("rossi-cli-dump-components");
    let output = dump(&[
        dir.to_str().unwrap(),
        "--component",
        "m",
        "--component",
        "n",
    ]);

    assert_eq!(output.status.code(), Some(0));
    let document = parse(&output);
    assert_eq!(document["machines"].as_array().unwrap().len(), 2);
    assert_eq!(document["contexts"].as_array().unwrap().len(), 2);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_unknown_component_is_a_usage_error_naming_what_there_is() {
    let dir = branched("rossi-cli-dump-component-typo");
    let output = dump(&[dir.to_str().unwrap(), "--component", "M"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "nothing should reach stdout");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("'M'"), "stderr: {stderr}");
    // The available names are listed, so a typo is fixable from the message.
    assert!(
        stderr.contains('m') && stderr.contains('n'),
        "stderr: {stderr}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_selection_reports_only_the_components_it_kept() {
    // The unselected machine fails checking, so the project as a whole does.
    // Asking about `m` should not fail on something `m` has nothing to do
    // with, and the document could not explain such a finding anyway.
    let dir = fixture_dir(
        "rossi-cli-dump-component-errors",
        &[
            ("c.eventb", CONTEXT),
            ("m.eventb", MACHINE),
            ("broken.eventb", BROKEN),
        ],
    );
    let path = dir.to_str().unwrap();

    let whole = dump(&[path]);
    assert_eq!(whole.status.code(), Some(1));

    let selection = dump(&[path, "--component", "m"]);
    assert_eq!(
        selection.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&selection.stderr)
    );
    let document = parse(&selection);
    let errors = document["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["severity"] == "error")
        .count();
    assert_eq!(errors, 0);
    std::fs::remove_dir_all(&dir).ok();
}
