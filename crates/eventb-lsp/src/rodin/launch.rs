//! Locating, registering with, and launching the Rodin platform.
//!
//! Ported from the VSCode extension's former TypeScript implementation. Two
//! distinct invocations of the same install: the *headless* run (Eclipse's
//! `org.eclipse.ant.core.antRunner` executing the bundled importer task to
//! register the project in the workspace) needs the real executable even for
//! a macOS `.app` bundle, while the *GUI* launch goes through `open` there so
//! macOS applies its normal app activation.

use std::path::{Path, PathBuf};
use std::process::Stdio;

/// The compiled Ant task that registers a `.project` into an Eclipse
/// workspace. Canonical copy; source and regeneration instructions live in
/// `importer/README.md`.
const IMPORTER_CLASS: &[u8] = include_bytes!("importer/RodinProjectImportTask.class");

/// The class's package path inside the transient importer classpath dir. The
/// historical `org.rossi.vscode` package name is kept — renaming it would
/// force recompiling the class for zero benefit.
const IMPORTER_CLASS_RELATIVE: &str = "org/rossi/vscode/RodinProjectImportTask.class";

/// Transient directory (inside the Rodin workspace) holding the importer
/// classpath and `build.xml` for the duration of one registration run.
pub const IMPORTER_DIR_NAME: &str = ".rossi-importer";

/// Rodin's Proving perspective, as `org.eventb.ide` declares it.
pub(crate) const PROVING_PERSPECTIVE_ID: &str = "org.eventb.ui.perspective.proving";

/// Host platform, passed explicitly so every variant is unit-testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Windows,
    Other,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Other
        }
    }
}

/// The configured Rodin path, or the platform default when unset.
pub fn effective_rodin_path(configured: &str, platform: Platform) -> String {
    let trimmed = configured.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }
    match platform {
        Platform::MacOs => "/Applications/Rodin.app".to_string(),
        Platform::Windows => "rodin.exe".to_string(),
        Platform::Other => "rodin".to_string(),
    }
}

/// On macOS, a Rodin path may be a `.app` bundle path or a bare application
/// name (e.g. "Rodin"). `None` on other platforms or for a plain executable
/// path, so callers fall back to spawning it directly.
enum MacRodinApp {
    Bundle(String),
    Name(String),
}

fn mac_rodin_app(rodin_path: &str, platform: Platform) -> Option<MacRodinApp> {
    if platform != Platform::MacOs {
        return None;
    }
    if rodin_path.to_lowercase().ends_with(".app") {
        return Some(MacRodinApp::Bundle(rodin_path.to_string()));
    }
    if !has_path_separator(rodin_path)
        && rodin_path
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase())
    {
        return Some(MacRodinApp::Name(rodin_path.to_string()));
    }
    None
}

/// Whether a tool setting denotes a filesystem path rather than a bare name
/// resolved elsewhere (`PATH`, `open -a`) — the shared classification every
/// pre-flight existence check must agree on with the spawn it fronts.
pub(crate) fn has_path_separator(value: &str) -> bool {
    value.contains('/') || value.contains('\\')
}

/// The concrete filesystem location a Rodin path setting denotes, when it
/// denotes one — a plain path or a macOS `.app` bundle. `None` for bare
/// names resolved elsewhere (`PATH` for executables, `open -a` for app
/// names), which callers must hand to the spawn to find out. This is the
/// same classification the launch commands use, so an existence pre-check
/// can never disagree with what would actually be launched.
pub fn concrete_path(rodin_path: &str, platform: Platform) -> Option<PathBuf> {
    match mac_rodin_app(rodin_path, platform) {
        Some(MacRodinApp::Bundle(app)) => Some(PathBuf::from(app)),
        Some(MacRodinApp::Name(_)) => None,
        None => has_path_separator(rodin_path).then(|| PathBuf::from(rodin_path)),
    }
}

/// The executable to run for headless (antRunner) invocations.
pub fn ant_runner_executable(rodin_path: &str, platform: Platform) -> Result<PathBuf, String> {
    match mac_rodin_app(rodin_path, platform) {
        Some(MacRodinApp::Bundle(app)) => mac_app_executable(Path::new(&app)),
        Some(MacRodinApp::Name(name)) => {
            mac_app_executable(&Path::new("/Applications").join(format!("{name}.app")))
        }
        None => Ok(PathBuf::from(rodin_path)),
    }
}

/// The launcher binary inside a macOS `.app` bundle: `Contents/MacOS/<name>`
/// with the bundle's lowercased stem, falling back to the first file present.
fn mac_app_executable(app_path: &Path) -> Result<PathBuf, String> {
    let executable_dir = app_path.join("Contents").join("MacOS");
    let stem = app_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_lowercase();
    let preferred = executable_dir.join(&stem);
    if preferred.is_file() {
        return Ok(preferred);
    }
    let entries = std::fs::read_dir(&executable_dir).map_err(|e| {
        format!(
            "cannot find Rodin executable inside {}: {e}",
            app_path.display()
        )
    })?;
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    files.sort();
    files
        .into_iter()
        .next()
        .ok_or_else(|| format!("cannot find Rodin executable inside {}", app_path.display()))
}

/// The command and arguments that launch the Rodin GUI on a workspace.
pub fn launch_command(
    rodin_path: &str,
    workspace_dir: &Path,
    platform: Platform,
) -> (String, Vec<String>) {
    let data_args = ["-data".to_string(), workspace_dir.display().to_string()];
    match mac_rodin_app(rodin_path, platform) {
        Some(MacRodinApp::Bundle(app)) => (
            "open".to_string(),
            ["-n".to_string(), app, "--args".to_string()]
                .into_iter()
                .chain(data_args)
                .collect(),
        ),
        Some(MacRodinApp::Name(name)) => (
            "open".to_string(),
            [
                "-n".to_string(),
                "-a".to_string(),
                name,
                "--args".to_string(),
            ]
            .into_iter()
            .chain(data_args)
            .collect(),
        ),
        None => (rodin_path.to_string(), data_args.to_vec()),
    }
}

/// Whether the workspace's Eclipse project registry already knows `name`, in
/// which case the (slow) headless registration run can be skipped entirely.
pub fn project_registered(workspace_dir: &Path, project_name: &str) -> bool {
    workspace_dir
        .join(".metadata")
        .join(".plugins")
        .join("org.eclipse.core.resources")
        .join(".projects")
        .join(project_name)
        .is_dir()
}

/// Seed workbench preferences so the workspace opens usable: no Eclipse
/// Welcome page hiding the project, and both auto-refresh mechanisms on, so
/// a running Rodin picks up files rossi rebuilds underneath it
/// (`refresh.enabled` starts Eclipse's polling monitor — the only detector
/// on macOS/Linux, no native hooks exist there — and
/// `refresh.lightweight.enabled` refreshes resources on access). Eclipse
/// writes its own keys into these same files (e.g. `encoding=`), so seeding
/// upserts our keys and preserves every other line; this workspace is
/// tool-managed, so re-asserting our keys over a manual change is intended.
/// Best effort — a preference must never block opening Rodin.
///
/// `proving_perspective` additionally selects Rodin's Proving perspective
/// over the one its product defaults to. Eclipse reads that preference
/// only when it opens a window with no perspective state to restore, so it
/// decides how a workspace Rodin has never opened comes up and nothing after
/// that; the key is therefore only ever written, never removed, since a
/// removal would also discard a default the user chose inside Rodin.
pub fn seed_workspace_prefs(workspace_dir: &Path, proving_perspective: bool) {
    let settings_dir = workspace_dir
        .join(".metadata")
        .join(".plugins")
        .join("org.eclipse.core.runtime")
        .join(".settings");
    if let Err(e) = std::fs::create_dir_all(&settings_dir) {
        tracing::info!("could not seed workspace preferences: {e}");
        return;
    }
    let seed = |file: &str, keys: &[(&str, &str)]| {
        if let Err(e) = upsert_prefs(&settings_dir.join(file), keys) {
            tracing::info!("could not seed {file}: {e}");
        }
    };
    let mut ui_keys = vec![("showIntro", "false")];
    if proving_perspective {
        ui_keys.push(("defaultPerspectiveId", PROVING_PERSPECTIVE_ID));
    }
    seed("org.eclipse.ui.prefs", &ui_keys);
    seed(
        "org.eclipse.core.resources.prefs",
        &[
            ("refresh.enabled", "true"),
            ("refresh.lightweight.enabled", "true"),
        ],
    );
}

/// Set `key=value` pairs in an Eclipse `.prefs` file, preserving all other
/// lines; a missing file starts from the standard version header. The keys
/// involved are plain ASCII, so Java-properties escaping never applies.
fn upsert_prefs(path: &Path, keys: &[(&str, &str)]) -> std::io::Result<()> {
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            "eclipse.preferences.version=1\n".to_string()
        }
        Err(e) => return Err(e),
    };
    let mut lines: Vec<&str> = existing
        .lines()
        .filter(|line| {
            !keys.iter().any(|(key, _)| {
                line.strip_prefix(key)
                    .is_some_and(|rest| rest.starts_with('='))
            })
        })
        .collect();
    let upserts: Vec<String> = keys
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    lines.extend(upserts.iter().map(String::as_str));
    std::fs::write(path, lines.join("\n") + "\n")
}

/// The `build.xml` driving the importer task; `projectDir` arrives via `-D`.
fn importer_build_xml(importer_dir: &Path) -> String {
    let mut classpath = String::new();
    rossi_build::xml_out::escape_attr(&importer_dir.display().to_string(), &mut classpath);
    format!(
        "<project name=\"rossi-rodin-import\" default=\"import\">\n  \
         <taskdef name=\"rossiImportProject\" \
         classname=\"org.rossi.vscode.RodinProjectImportTask\" classpath=\"{classpath}\"/>\n  \
         <target name=\"import\">\n    \
         <rossiImportProject projectDir=\"${{projectDir}}\"/>\n  \
         </target>\n</project>\n"
    )
}

/// Write the importer classpath (compiled task class) and `build.xml` into
/// `importer_dir`; returns the `build.xml` path.
fn write_importer_files(importer_dir: &Path) -> std::io::Result<PathBuf> {
    let class_path = importer_dir.join(IMPORTER_CLASS_RELATIVE);
    if let Some(parent) = class_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&class_path, IMPORTER_CLASS)?;
    let build_file = importer_dir.join("build.xml");
    std::fs::write(&build_file, importer_build_xml(importer_dir))?;
    Ok(build_file)
}

/// Register `project_dir`'s `.project` into the workspace by running Rodin
/// headlessly (`-application org.eclipse.ant.core.antRunner`). The transient
/// importer directory is removed again on every path.
pub async fn register_project(
    ant_runner: &Path,
    workspace_dir: &Path,
    project_dir: &Path,
) -> Result<(), String> {
    let importer_dir = workspace_dir.join(IMPORTER_DIR_NAME);
    let build_file = write_importer_files(&importer_dir).map_err(|e| {
        format!(
            "cannot write importer files into {}: {e}",
            importer_dir.display()
        )
    })?;

    let result = tokio::process::Command::new(ant_runner)
        .arg("-nosplash")
        .arg("-application")
        .arg("org.eclipse.ant.core.antRunner")
        .arg("-data")
        .arg(workspace_dir)
        .arg("-buildfile")
        .arg(&build_file)
        .arg(format!("-DprojectDir={}", project_dir.display()))
        .stdin(Stdio::null())
        .output()
        .await;
    std::fs::remove_dir_all(&importer_dir).ok();

    match result {
        Err(e) => Err(format!("failed to run {}: {e}", ant_runner.display())),
        Ok(output) if !output.status.success() => Err(format!(
            "Rodin project registration failed ({}): {}",
            output.status,
            output_tail(&output)
        )),
        Ok(_) => Ok(()),
    }
}

/// The last part of a failed process's output, stderr preferred.
fn output_tail(output: &std::process::Output) -> String {
    let text = if output.stderr.is_empty() {
        String::from_utf8_lossy(&output.stdout)
    } else {
        String::from_utf8_lossy(&output.stderr)
    };
    crate::text_utils::output_excerpt(&text, 2000)
}

/// Launch the Rodin GUI detached: the server neither owns nor waits on it
/// beyond reaping, and it survives a server shutdown.
pub fn launch_gui(command: &str, args: &[String]) -> Result<(), String> {
    let mut cmd = tokio::process::Command::new(command);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    {
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
        cmd.creation_flags(0x0000_0008 | 0x0000_0200);
    }
    match cmd.spawn() {
        Ok(mut child) => {
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
            Ok(())
        }
        Err(e) => Err(format!("failed to start '{command}': {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeding_a_fresh_workspace_writes_both_prefs_files() {
        let dir = crate::test_util::TempDir::new("rossi-prefs-fresh");
        seed_workspace_prefs(dir.path(), false);

        let settings = dir
            .path()
            .join(".metadata/.plugins/org.eclipse.core.runtime/.settings");
        let ui = std::fs::read_to_string(settings.join("org.eclipse.ui.prefs")).unwrap();
        assert!(ui.contains("eclipse.preferences.version=1\n"));
        assert!(ui.contains("showIntro=false\n"));
        assert!(!ui.contains("defaultPerspectiveId"));
        let resources =
            std::fs::read_to_string(settings.join("org.eclipse.core.resources.prefs")).unwrap();
        assert!(resources.contains("eclipse.preferences.version=1\n"));
        assert!(resources.contains("refresh.enabled=true\n"));
        assert!(resources.contains("refresh.lightweight.enabled=true\n"));
    }

    #[test]
    fn seeding_preserves_foreign_keys_and_replaces_conflicting_ones() {
        let dir = crate::test_util::TempDir::new("rossi-prefs-upsert");
        let settings = dir
            .path()
            .join(".metadata/.plugins/org.eclipse.core.runtime/.settings");
        std::fs::create_dir_all(&settings).unwrap();
        // What Eclipse's own Workspace preference page leaves behind.
        std::fs::write(
            settings.join("org.eclipse.core.resources.prefs"),
            "eclipse.preferences.version=1\nencoding=UTF-8\n\
             refresh.lightweight.enabled=false\nversion=1\n",
        )
        .unwrap();

        seed_workspace_prefs(dir.path(), false);
        seed_workspace_prefs(dir.path(), false); // idempotent

        let resources =
            std::fs::read_to_string(settings.join("org.eclipse.core.resources.prefs")).unwrap();
        assert!(resources.contains("encoding=UTF-8\n"));
        assert!(resources.contains("version=1\n"));
        assert!(!resources.contains("refresh.lightweight.enabled=false"));
        let occurrences = |needle: &str| resources.matches(needle).count();
        assert_eq!(occurrences("refresh.enabled=true\n"), 1);
        assert_eq!(occurrences("refresh.lightweight.enabled=true\n"), 1);
    }

    #[test]
    fn seeding_the_proving_perspective_adds_one_key_beside_the_others() {
        let dir = crate::test_util::TempDir::new("rossi-prefs-perspective");
        seed_workspace_prefs(dir.path(), true);
        seed_workspace_prefs(dir.path(), true); // idempotent

        let ui = std::fs::read_to_string(
            dir.path()
                .join(".metadata/.plugins/org.eclipse.core.runtime/.settings/org.eclipse.ui.prefs"),
        )
        .unwrap();
        assert!(ui.contains("showIntro=false\n"));
        assert_eq!(
            ui.matches("defaultPerspectiveId=org.eventb.ui.perspective.proving\n")
                .count(),
            1
        );
    }

    #[test]
    fn default_rodin_path_per_platform() {
        assert_eq!(
            effective_rodin_path("", Platform::MacOs),
            "/Applications/Rodin.app"
        );
        assert_eq!(effective_rodin_path("", Platform::Windows), "rodin.exe");
        assert_eq!(effective_rodin_path("", Platform::Other), "rodin");
        assert_eq!(
            effective_rodin_path("  /opt/rodin ", Platform::Other),
            "/opt/rodin"
        );
    }

    #[test]
    fn launch_command_variants() {
        let ws = Path::new("/ws");
        assert_eq!(
            launch_command("/Applications/Rodin.app", ws, Platform::MacOs),
            (
                "open".to_string(),
                vec![
                    "-n".into(),
                    "/Applications/Rodin.app".into(),
                    "--args".into(),
                    "-data".into(),
                    "/ws".into()
                ]
            )
        );
        assert_eq!(
            launch_command("Rodin", ws, Platform::MacOs),
            (
                "open".to_string(),
                vec![
                    "-n".into(),
                    "-a".into(),
                    "Rodin".into(),
                    "--args".into(),
                    "-data".into(),
                    "/ws".into()
                ]
            )
        );
        // A lowercase bare name or an explicit path on macOS is spawned directly.
        assert_eq!(
            launch_command("rodin", ws, Platform::MacOs),
            ("rodin".to_string(), vec!["-data".into(), "/ws".into()])
        );
        assert_eq!(
            launch_command("C:\\Rodin\\rodin.exe", ws, Platform::Windows),
            (
                "C:\\Rodin\\rodin.exe".to_string(),
                vec!["-data".into(), "/ws".into()]
            )
        );
    }

    #[test]
    fn concrete_path_matches_the_launch_classification() {
        // Paths and bundles are concrete and can be existence-checked.
        assert_eq!(
            concrete_path("/Applications/Rodin.app", Platform::MacOs),
            Some(PathBuf::from("/Applications/Rodin.app"))
        );
        assert_eq!(
            concrete_path("/opt/rodin", Platform::Other),
            Some(PathBuf::from("/opt/rodin"))
        );
        assert_eq!(
            concrete_path("C:\\Rodin\\rodin.exe", Platform::Windows),
            Some(PathBuf::from("C:\\Rodin\\rodin.exe"))
        );
        // Bare names resolve via PATH / `open -a`: nothing to pre-check.
        assert_eq!(concrete_path("Rodin", Platform::MacOs), None);
        assert_eq!(concrete_path("rodin", Platform::Other), None);
    }

    #[test]
    fn ant_runner_for_plain_path_is_the_path() {
        assert_eq!(
            ant_runner_executable("/usr/bin/rodin", Platform::Other).unwrap(),
            PathBuf::from("/usr/bin/rodin")
        );
        // A .app path elsewhere than macOS is still treated as a plain path.
        assert_eq!(
            ant_runner_executable("/x/Rodin.app", Platform::Other).unwrap(),
            PathBuf::from("/x/Rodin.app")
        );
    }

    #[test]
    fn build_xml_escapes_classpath() {
        let xml = importer_build_xml(Path::new("/tmp/a\"b&c"));
        assert!(xml.contains("classpath=\"/tmp/a&quot;b&amp;c\""));
        assert!(xml.contains("classname=\"org.rossi.vscode.RodinProjectImportTask\""));
        assert!(xml.contains("<rossiImportProject projectDir=\"${projectDir}\"/>"));
    }

    #[test]
    fn embedded_importer_class_is_valid() {
        // Java class magic; the committed .class is the canonical copy.
        assert_eq!(&IMPORTER_CLASS[..4], &[0xCA, 0xFE, 0xBA, 0xBE]);
    }
}
