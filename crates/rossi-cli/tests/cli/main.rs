mod helpers;

mod build;
mod clean;
mod dump;
mod export_build;
mod fmt;
mod import_export;
mod prove;
mod stdin;
mod validate;

use helpers::rossi_command;

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
