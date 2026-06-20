//! Guards that the committed editor grammars are regenerated. If the canonical
//! token tables change without re-running `cargo xtask gen-grammars`, this fails — the
//! same protection CI gives, available from `cargo test`.

use std::process::Command;

#[test]
fn editor_grammars_are_up_to_date() {
    let bin = env!("CARGO_BIN_EXE_xtask");
    let output = Command::new(bin)
        .args(["gen-grammars", "--check"])
        .output()
        .expect("run `cargo xtask gen-grammars --check`");

    assert!(
        output.status.success(),
        "editor grammars are out of date; run `cargo xtask gen-grammars` to regenerate.\n\
         out-of-date files:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
