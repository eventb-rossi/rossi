//! Fixtures and process-driving helpers shared by the concern modules.

use std::io::Read;
use std::path::PathBuf;
use std::process::Command;

pub fn rossi_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rossi"))
}

/// Run the CLI with `args`, returning the completed process output.
pub fn run_cli(args: &[&str]) -> std::process::Output {
    rossi_command()
        .args(args)
        .output()
        .expect("Failed to execute command")
}

/// Assert a CLI run succeeded, quoting its stderr on failure.
pub fn assert_cli_ok(output: &std::process::Output, case: &str) {
    assert!(
        output.status.success(),
        "{case}; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
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
pub const LINT_FIXTURE_BUM: &str = r#"<?xml version="1.0"?>
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
pub const LINT_FIXTURE_BUC: &str = r#"<?xml version="1.0"?>
<org.eventb.core.contextFile version="3" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.constant name="_c1" org.eventb.core.identifier="lo"/>
<org.eventb.core.axiom name="_a1" org.eventb.core.label="axm1" org.eventb.core.predicate="lo ∈ ℤ" org.eventb.core.theorem="false"/>
</org.eventb.core.contextFile>
"#;

/// Write the lint fixture (machine + seen context) as a zip in a fresh temp
/// dir; returns `(tempdir, zip_path)` — remove the tempdir when done.
pub fn lint_fixture_zip(prefix: &str) -> (PathBuf, PathBuf) {
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

/// Write the lint fixture (machine + seen context) as loose files in a fresh
/// temp directory — the unzipped Rodin project layout.
pub fn lint_fixture_dir(prefix: &str) -> PathBuf {
    let tmp = tempdir_unique(prefix);
    std::fs::write(tmp.join("Ctx.buc"), LINT_FIXTURE_BUC).unwrap();
    std::fs::write(tmp.join("Lint.bum"), LINT_FIXTURE_BUM).unwrap();
    tmp
}

/// A warning-free machine whose `10 ÷ x` invariant has a non-trivial WD
/// condition. Used to isolate EB010's opt-in and exit-code behavior.
pub fn wd_fixture_dir(prefix: &str) -> PathBuf {
    const MACHINE: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_type" org.eventb.core.label="type" org.eventb.core.predicate="x ∈ ℤ"/>
<org.eventb.core.invariant name="_wd" org.eventb.core.label="wd" org.eventb.core.predicate="10 ÷ x &gt; 0"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_init_action" org.eventb.core.assignment="x ≔ 1" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_inc" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="increment">
<org.eventb.core.action name="_inc_action" org.eventb.core.assignment="x ≔ x + 1" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>
"#;
    let tmp = tempdir_unique(prefix);
    std::fs::write(tmp.join("M.bum"), MACHINE).unwrap();
    tmp
}

pub const ASCII_CONTEXT: &str = "CONTEXT c\nCONSTANTS\n    x\nAXIOMS\n    @axm1 x : NAT\nEND\n";

pub const RESERVED_NAT_MACHINE: &str = "machine M\nvariables NAT\ninvariants\n  @inv1 NAT ∈ 0 ‥ 5\nevents\n  event INITIALISATION\n  then\n    @act1 NAT ≔ 0\n  end\n  event inc\n  where\n    @grd1 NAT < 5\n  then\n    @act1 NAT ≔ NAT + 1\n  end\nend\n";

pub const DUP_VARIABLE_MACHINE: &str = "MACHINE M\nVARIABLES\n    x x\nINVARIANTS\n    @inv1 x >= 0\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x := 0\n    END\nEND\n";

pub const MINIMAL_BUILD_CONTEXT_XML: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
    <org.eventb.core.contextFile version=\"3\" \
    org.eventb.core.configuration=\"org.eventb.core.fwd\"></org.eventb.core.contextFile>\n";

pub struct BuildFixture {
    pub root: PathBuf,
    pub output: PathBuf,
}

impl BuildFixture {
    pub fn new(entries: &[&str], output: &str) -> Self {
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

    pub fn run(&self) -> std::process::Output {
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

    pub fn assert_success(&self, case: &str) {
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

/// Recursively check whether `dir` contains a file whose extension matches one
/// of `exts` (case-insensitive).
pub fn dir_has_ext(dir: &std::path::Path, exts: &[&str]) -> bool {
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

pub fn write_zip(zip_path: &std::path::Path, entries: &[(&str, &[u8])]) {
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
pub fn project_descriptor(name: &str) -> Vec<u8> {
    format!("<projectDescription><name>{name}</name></projectDescription>").into_bytes()
}

pub fn tempdir_unique(prefix: &str) -> PathBuf {
    // The timestamp alone is not unique: tests run in parallel and the clock's
    // resolution is coarser than the rate at which they call this, so two
    // fixtures could share a directory and clobber each other's files. A
    // per-process counter makes the name unique whatever the clock does.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("{prefix}-{nanos}-{seq}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn extract_zip_to(zip_path: &PathBuf, dest: &std::path::Path) {
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

pub fn zip_entry_bytes(zip_path: &std::path::Path, name: &str) -> Vec<u8> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(zip_path).unwrap()).unwrap();
    let mut entry = archive.by_name(name).unwrap_or_else(|_| panic!("{name}"));
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).unwrap();
    bytes
}

pub fn zip_entry_names(zip_path: &std::path::Path) -> Vec<String> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(zip_path).unwrap()).unwrap();
    (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect()
}

/// Rewrite one text entry of a zip through `transform`, copying every
/// other entry as-is.
pub fn rewrite_zip_entry(
    zip_path: &std::path::Path,
    out_path: &std::path::Path,
    entry_name: &str,
    transform: impl Fn(&str) -> String,
) {
    use std::io::Write;
    let mut archive = zip::ZipArchive::new(std::fs::File::open(zip_path).unwrap()).unwrap();
    let mut cursor = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(&mut cursor);
    let options = zip::write::SimpleFileOptions::default();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        let name = entry.name().to_string();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        if name == entry_name {
            bytes = transform(std::str::from_utf8(&bytes).unwrap()).into_bytes();
        }
        writer.start_file(name, options).unwrap();
        writer.write_all(&bytes).unwrap();
    }
    writer.finish().unwrap();
    std::fs::write(out_path, cursor.into_inner()).unwrap();
}

/// Run the CLI with `stdin_data` piped to its standard input.
pub fn run_cli_with_stdin(args: &[&str], stdin_data: &str) -> std::process::Output {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = rossi_command()
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn rossi-cli");
    // The CLI may reject its arguments and exit before reading stdin (as
    // `export - --proofs` does), closing the pipe under us. That is a valid
    // outcome for the run; the assertions live on the child's output.
    match child
        .stdin
        .as_mut()
        .expect("child stdin")
        .write_all(stdin_data.as_bytes())
    {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
        Err(e) => panic!("write stdin: {e}"),
    }
    // `wait_with_output` closes stdin (signalling EOF) before collecting output.
    child.wait_with_output().expect("wait for rossi-cli")
}

/// A machine whose only defect is a nondeterministic choice in an ordinary
/// event: EB100, a runtime-translation warning, with nothing else to raise a
/// warning of any kind. Used to isolate the `--runtime` exit-code behaviour
/// from the bundled examples, which carry broken proofs of their own.
pub fn runtime_fixture_dir(prefix: &str) -> PathBuf {
    const MACHINE: &str = r#"<?xml version="1.0"?>
<org.eventb.core.machineFile version="5" org.eventb.core.configuration="org.eventb.core.fwd">
<org.eventb.core.variable name="_x" org.eventb.core.identifier="x"/>
<org.eventb.core.invariant name="_type" org.eventb.core.label="type" org.eventb.core.predicate="x ∈ 0 ‥ 9"/>
<org.eventb.core.event name="_init" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="INITIALISATION">
<org.eventb.core.action name="_init_action" org.eventb.core.assignment="x ≔ 0" org.eventb.core.label="act1"/>
</org.eventb.core.event>
<org.eventb.core.event name="_pick" org.eventb.core.convergence="0" org.eventb.core.extended="false" org.eventb.core.label="pick">
<org.eventb.core.action name="_pick_action" org.eventb.core.assignment="x :∈ 0 ‥ 9" org.eventb.core.label="act1"/>
</org.eventb.core.event>
</org.eventb.core.machineFile>"#;
    let tmp = tempdir_unique(prefix);
    std::fs::write(tmp.join("Pick.bum"), MACHINE).unwrap();
    tmp
}
