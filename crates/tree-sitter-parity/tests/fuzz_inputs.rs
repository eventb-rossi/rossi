//! Both-direction parity between Rossi's parser and the tree-sitter grammar,
//! over a directory of inputs produced by the fuzzer.
//!
//! `corpus.rs` checks one direction over real models: everything Rossi accepts,
//! tree-sitter must accept too. Fuzz inputs are the other half of the picture —
//! they are generated *from* the tree-sitter grammar, so they also show where
//! tree-sitter accepts text Rossi refuses, which is where the two grammars
//! disagree about the language.
//!
//! This test reports; it does not gate. Point it at an input directory:
//!
//!   EVENTB_FUZZ_INPUTS=<dir> \
//!     cargo test --manifest-path crates/tree-sitter-parity/Cargo.toml \
//!     --test fuzz_inputs -- --nocapture
//!
//! With the variable unset it prints a SKIP line and passes. That is a weaker
//! rule than the fuzzer applies to the grammar itself, whose absence fails
//! under CI — deliberately, because these inputs are produced by a run rather
//! than checked in, so there is nothing for CI to find.

mod common;

/// Above this many inputs, only the disagreements are listed.
const PER_INPUT_LIMIT: usize = 200;

/// How Rossi answered, with a crash as a verdict of its own.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Accept,
    Reject,
    Crash,
}

impl Verdict {
    fn label(self) -> &'static str {
        match self {
            Verdict::Accept => "accept",
            Verdict::Reject => "REJECT",
            Verdict::Crash => "CRASH",
        }
    }
}

/// Whether the two parsers gave the same answer.
fn agree(rossi: Verdict, tree_sitter: bool) -> bool {
    matches!(
        (rossi, tree_sitter),
        (Verdict::Accept, true) | (Verdict::Reject, false)
    )
}

fn rossi_verdict(text: &str) -> Verdict {
    match common::without_panicking(|| rossi::parse_components(text).is_ok()) {
        Some(true) => Verdict::Accept,
        Some(false) => Verdict::Reject,
        None => Verdict::Crash,
    }
}

#[test]
fn report_parity_over_fuzz_inputs() {
    let Some(directory) = common::directory_from_env("EVENTB_FUZZ_INPUTS") else {
        eprintln!("SKIP fuzz_inputs: no input directory (set EVENTB_FUZZ_INPUTS)");
        return;
    };
    let inputs = common::collect_files(&directory, "eventb").expect("read the input directory");
    assert!(
        !inputs.is_empty(),
        "input directory holds no .eventb files: {}",
        directory.display()
    );

    let mut parser = common::eventb_parser();
    let mut both = 0usize;
    let mut neither = 0usize;
    let mut rossi_only = Vec::new();
    let mut tree_sitter_only = Vec::new();
    let mut whitespace_only = Vec::new();
    let mut crashed = Vec::new();
    let mut normalised = 0usize;

    for input in &inputs {
        let text = std::fs::read_to_string(input).expect("read input");
        let rossi = rossi_verdict(&text);
        let tree_sitter = common::tree_sitter_accepts(&mut parser, &text);

        // Both grammars separate on Rodin's set now, so this is a drift guard
        // rather than a filter. Were the two sets to part again, a whitespace
        // disagreement would be indistinguishable from a grammar one, and it
        // is the grammar findings this report exists for. Re-run both parsers
        // over the normalised text: a disagreement that survives is about the
        // language, one that does not is about whitespace.
        //
        // `&&` keeps both the normalised copy and the re-parse off the
        // agreeing majority; only the counter looks at every input.
        let rewritten = common::has_exotic_separator(&text);
        if rewritten {
            normalised += 1;
        }
        let whitespace_is_the_difference = rewritten
            && !agree(rossi, tree_sitter)
            && {
                let plain = common::normalize_whitespace(&text);
                agree(
                    rossi_verdict(&plain),
                    common::tree_sitter_accepts(&mut parser, &plain),
                )
            };

        match (rossi, tree_sitter) {
            (Verdict::Accept, true) => both += 1,
            (Verdict::Reject, false) => neither += 1,
            (Verdict::Crash, _) => crashed.push(input.clone()),
            (Verdict::Accept, false) | (Verdict::Reject, true)
                if whitespace_is_the_difference =>
            {
                whitespace_only.push(input.clone());
            }
            (Verdict::Accept, false) => rossi_only.push(input.clone()),
            (Verdict::Reject, true) => tree_sitter_only.push(input.clone()),
        }
        // A per-input line is what makes a small, hand-built set of probe
        // cases readable; a fuzzing run's directory is far too large for it.
        if inputs.len() <= PER_INPUT_LIMIT {
            let name = input.file_stem().unwrap_or_default().to_string_lossy();
            println!(
                "  {name:<34} rossi={:<7} tree-sitter={}",
                rossi.label(),
                if tree_sitter { "accept" } else { "REJECT" }
            );
        }
    }

    println!(
        "parity over {} inputs: {both} accepted by both, {neither} by neither, \
         {} only by rossi, {} only by tree-sitter, {} whitespace-only, \
         {} crashed rossi ({normalised} inputs held a separator normalisation rewrote)",
        inputs.len(),
        rossi_only.len(),
        tree_sitter_only.len(),
        whitespace_only.len(),
        crashed.len()
    );
    for (label, group) in [
        ("ROSSI-ONLY      ", &rossi_only),
        ("TREE-SITTER-ONLY", &tree_sitter_only),
        ("WHITESPACE-ONLY ", &whitespace_only),
        ("CRASHED-ROSSI   ", &crashed),
    ] {
        for input in group.iter().take(20) {
            println!("  {label} {}", input.display());
        }
    }
}
