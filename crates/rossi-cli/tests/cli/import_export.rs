//! `rossi import` / `rossi export`: Rodin round-trips and project layout.

use std::io::Read;

use crate::helpers::{
    ASCII_CONTEXT, MINIMAL_BUILD_CONTEXT_XML, assert_cli_ok, dir_has_ext, extract_zip_to,
    project_descriptor, rossi_command, run_cli, run_cli_with_stdin, tempdir_unique, write_zip,
    zip_entry_bytes, zip_entry_names,
};

#[test]
fn import_rodin_component_file_to_eventb() {
    for (input, output_name, needle) in [
        (
            "../rossi/examples/counter_ctx.buc",
            "counter_ctx.eventb",
            "context counter_ctx",
        ),
        (
            "../rossi/examples/counter.bum",
            "counter.eventb",
            "machine counter",
        ),
    ] {
        let tmp = tempdir_unique("rossi-cli-import-component");
        let out_dir = tmp.join("out");

        let output = rossi_command()
            .args(["import", input, "-o", out_dir.to_str().unwrap()])
            .output()
            .expect("Failed to execute command");

        assert!(
            output.status.success(),
            "import {input} should exit 0; stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = std::fs::read_to_string(out_dir.join(output_name)).unwrap();
        assert!(
            text.contains(needle),
            "expected `{needle}` in {output_name}"
        );

        std::fs::remove_dir_all(&tmp).ok();
    }
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
        &[("../M.bum", &machine_xml), ("safe/N.bum", &machine_xml)],
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
        !tmp.join("M.eventb").exists(),
        "import escaped the output directory"
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
fn export_attaches_standalone_comment_to_following_axiom() {
    let tmp = tempdir_unique("rossi-cli-export-leading-comment");
    let source = tmp.join("C.eventb");
    let out_dir = tmp.join("out");
    std::fs::write(
        &source,
        "context C\nconstants z\naxioms\n  @cx1 z ∈ ℕ\n  /* about cx2 */\n  @cx2 z > 1 // trailing on cx2\nend\n",
    )
    .unwrap();

    let output = rossi_command()
        .args([
            "export",
            "-o",
            out_dir.to_str().unwrap(),
            source.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "export failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let xml = std::fs::read_to_string(out_dir.join("C.buc")).unwrap();
    let rossi::Component::Context(ctx) = rossi::parse_xml(&xml).unwrap() else {
        panic!("expected context");
    };
    assert_eq!(ctx.axioms[0].comment, None);
    assert_eq!(
        ctx.axioms[1].comment.as_deref(),
        Some("about cx2\ntrailing on cx2")
    );

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

fn dir_has_rodin_file(dir: &std::path::Path) -> bool {
    dir_has_ext(dir, &["buc", "bum"])
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

const TRAFFIC_LIGHT: &str = "../rossi/examples/traffic-light.zip";

/// The bundled example's real `M0.bpr` bytes — the byte-exact reference the
/// proof-carry tests compare against.
fn traffic_light_m0_bpr() -> Vec<u8> {
    zip_entry_bytes(std::path::Path::new(TRAFFIC_LIGHT), "traffic-light/M0.bpr")
}

#[test]
fn import_zip_copies_proof_files_next_to_text() {
    let tmp = tempdir_unique("rossi-cli-import-proofs");
    let out = tmp.join("out");

    let output = run_cli(&["import", TRAFFIC_LIGHT, "-o", out.to_str().unwrap()]);
    assert_cli_ok(&output, "import should exit 0");

    assert!(out.join("M0.eventb").is_file(), "text must be written");
    assert_eq!(
        std::fs::read(out.join("M0.bpr")).expect("M0.bpr"),
        traffic_light_m0_bpr(),
        "proofs must be copied byte-exact next to the text"
    );
    assert!(out.join("C1.bpr").is_file());

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn import_multi_project_zip_scopes_proofs_per_project() {
    let tmp = tempdir_unique("rossi-cli-import-proofs-multi");
    let input = tmp.join("multi.zip");
    write_zip(
        &input,
        &[
            ("A/CA.buc", MINIMAL_BUILD_CONTEXT_XML.as_bytes()),
            ("A/CA.bpr", b"proof A"),
            ("B/CB.buc", MINIMAL_BUILD_CONTEXT_XML.as_bytes()),
        ],
    );
    let out = tmp.join("out");

    let output = run_cli(&[
        "import",
        input.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ]);
    assert_cli_ok(&output, "import should exit 0");

    assert_eq!(
        std::fs::read(out.join("A").join("CA.bpr")).unwrap(),
        b"proof A"
    );
    assert!(
        !dir_has_ext(&out.join("B"), &["bpr"]),
        "project B carries no proofs"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn import_no_proofs_skips_proof_files() {
    let tmp = tempdir_unique("rossi-cli-import-no-proofs");
    let out = tmp.join("out");

    let output = run_cli(&[
        "import",
        "--no-proofs",
        TRAFFIC_LIGHT,
        "-o",
        out.to_str().unwrap(),
    ]);
    assert_cli_ok(&output, "import --no-proofs should exit 0");

    assert!(out.join("M0.eventb").is_file());
    assert!(
        !dir_has_ext(&out, &["bpr"]),
        "--no-proofs must skip proof files"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn import_merge_writes_proofs_next_to_the_merged_file() {
    let tmp = tempdir_unique("rossi-cli-import-merge-proofs");
    let out_file = tmp.join("model.eventb");

    let output = run_cli(&[
        "import",
        "--merge",
        TRAFFIC_LIGHT,
        "-o",
        out_file.to_str().unwrap(),
    ]);
    assert_cli_ok(&output, "import --merge should exit 0");

    assert!(out_file.is_file());
    assert_eq!(
        std::fs::read(tmp.join("M0.bpr")).expect("M0.bpr"),
        traffic_light_m0_bpr(),
        "proofs must land next to the merged output file"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn import_skips_unsafe_proof_entry_names() {
    let tmp = tempdir_unique("rossi-cli-import-unsafe-proofs");
    let input = tmp.join("evil.zip");
    write_zip(
        &input,
        &[
            ("t/C.buc", MINIMAL_BUILD_CONTEXT_XML.as_bytes()),
            ("t/../evil.bpr", b"escape attempt"),
        ],
    );
    let out = tmp.join("out");

    let output = run_cli(&[
        "import",
        input.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ]);
    assert_cli_ok(&output, "import should still succeed");

    assert!(out.join("C.eventb").is_file());
    assert!(
        !dir_has_ext(&out, &["bpr"]),
        "the unsafe entry must be skipped"
    );
    assert!(
        !tmp.join("evil.bpr").exists(),
        "nothing may escape the output dir"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn import_then_export_proofs_round_trips_bpr() {
    // The closed loop: import drops the proofs next to the text, and a bare
    // `export --proofs` picks them up from exactly that location.
    let tmp = tempdir_unique("rossi-cli-import-export-roundtrip");
    let text_dir = tmp.join("text");

    let output = run_cli(&["import", TRAFFIC_LIGHT, "-o", text_dir.to_str().unwrap()]);
    assert_cli_ok(&output, "import should exit 0");

    let out_zip = tmp.join("traffic-light.zip");
    let output = run_cli(&[
        "export",
        "--proofs",
        text_dir.to_str().unwrap(),
        "-o",
        out_zip.to_str().unwrap(),
    ]);
    assert_cli_ok(&output, "export --proofs should exit 0");

    assert_eq!(
        zip_entry_bytes(&out_zip, "M0.bpr"),
        traffic_light_m0_bpr(),
        "proofs must survive the full text round-trip byte-exact"
    );
    let names = zip_entry_names(&out_zip);
    for expected in ["M0.bcm", "M0.bpo", "M0.bps"] {
        assert!(
            names.contains(&expected.to_string()),
            "missing {expected} in {names:?}"
        );
    }

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn import_style_camille_writes_camille_text() {
    let tmp = tempdir_unique("rossi-cli-import-style");
    let out_dir = tmp.join("out");

    let output = rossi_command()
        .args([
            "import",
            "--style",
            "camille",
            "../rossi/examples/counter_ctx.buc",
            "-o",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");

    assert!(
        output.status.success(),
        "import --style camille stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = std::fs::read_to_string(out_dir.join("counter_ctx.eventb")).unwrap();
    assert!(
        text.starts_with("context counter_ctx"),
        "expected camille-style output, got:\n{text}"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn import_merge_output_passes_fmt_check() {
    // Merged import writes fmt's canonical text form: one blank line
    // between components and exactly one trailing newline, so the file is
    // a `fmt --check` fixpoint.
    let tmp = tempdir_unique("rossi-cli-import-merge-check");
    let merged = tmp.join("merged.eventb");

    let output = rossi_command()
        .args([
            "import",
            "--merge",
            "../rossi/examples/counter_ctx.buc",
            "../rossi/examples/counter.bum",
            "-o",
            merged.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute command");
    assert!(
        output.status.success(),
        "import --merge stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let check = rossi_command()
        .args(["fmt", "--check", merged.to_str().unwrap()])
        .output()
        .expect("Failed to execute command");
    assert!(
        check.status.success(),
        "merged import must pass fmt --check; stdout={}",
        String::from_utf8_lossy(&check.stdout)
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn import_merge_orders_components_by_dependency() {
    // Loose inputs are read in command-line order, so the machine is offered
    // first in both cases. A bare --merge must still introduce the context it
    // SEES above it; an explicit order stays the escape hatch and wins.
    for (case, merge_arg, context_first) in [
        ("bare", "--merge", true),
        ("explicit", "--merge=counter,counter_ctx", false),
    ] {
        let tmp = tempdir_unique("rossi-cli-import-merge-order");
        let merged = tmp.join("merged.eventb");

        let output = run_cli(&[
            "import",
            merge_arg,
            "../rossi/examples/counter.bum",
            "../rossi/examples/counter_ctx.buc",
            "-o",
            merged.to_str().unwrap(),
        ]);
        assert_cli_ok(&output, &format!("import {merge_arg} should exit 0"));

        let text = std::fs::read_to_string(&merged).unwrap();
        let context = text.find("context counter_ctx").expect("context header");
        let machine = text.find("machine counter ").expect("machine header");
        assert_eq!(
            context < machine,
            context_first,
            "{case} merge ordered the components wrongly, got:\n{text}"
        );

        std::fs::remove_dir_all(&tmp).ok();
    }
}

#[test]
fn import_merge_warns_on_a_cycle_and_still_writes() {
    // A cycle is the checker's to report; merging still produces the file,
    // in input order.
    let tmp = tempdir_unique("rossi-cli-import-merge-cycle");
    // A context that EXTENDS itself: the smallest possible dependency cycle.
    let xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
        <org.eventb.core.contextFile version=\"3\" \
        org.eventb.core.configuration=\"org.eventb.core.fwd\">\
        <org.eventb.core.extendsContext name=\"e1\" org.eventb.core.target=\"C\"/>\
        </org.eventb.core.contextFile>\n";
    let input = tmp.join("C.buc");
    std::fs::write(&input, xml).unwrap();
    let merged = tmp.join("merged.eventb");

    let output = run_cli(&[
        "import",
        "--merge",
        input.to_str().unwrap(),
        "-o",
        merged.to_str().unwrap(),
    ]);
    assert_cli_ok(&output, "a cycle must not fail the merge");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("circular dependency"),
        "expected a cycle warning, got stderr={stderr}"
    );
    assert!(
        std::fs::read_to_string(&merged)
            .unwrap()
            .contains("context C"),
        "the merged file must still be written"
    );

    std::fs::remove_dir_all(&tmp).ok();
}
