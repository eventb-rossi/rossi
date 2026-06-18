//! Oracle validation for the parse-time operator-compatibility gate
//! against the Rodin formula parser.
//!
//! `#[ignore]` by default: the Rodin-backed oracle lives outside the workspace.
//! Run with the eventb-checker CLI available:
//!
//! ```sh
//! EVENTB_CHECKER=/path/to/eventb-checker \
//!   cargo test -p rossi-build --test operator_compat_oracle -- --ignored --nocapture
//! ```
//!
//! For every ordered pair of binary set operators, and for the predicate
//! connective cases, the test asks the oracle whether the bare form parses and
//! asserts rossi's parser makes the same accept/reject decision. This is the
//! authority behind `op_info::set_ops_acceptable` and the connective gate; it
//! lets a maintainer re-confirm the matrix if the grammar or the operator set
//! changes.

use std::collections::BTreeMap;
use std::process::Command;

use rossi::{parse_expression_str, parse_predicate_str};

/// Binary set-level operators, by display glyph. The override operator uses the
/// Rodin private-use codepoint U+E103 via an escape so it survives in source.
const SET_OPS: &[&str] = &[
    "∪", "∩", "∖", "×", "\u{E103}", ";", "∘", "◁", "⩤", "▷", "⩥", "⊗", "∥",
];

/// The `eventb-checker` command: `EVENTB_CHECKER` if set, else `eventb-checker`
/// from `PATH`. May be a wrapper (e.g. `java -jar …`) exposed as a single
/// executable.
fn oracle_bin() -> String {
    std::env::var("EVENTB_CHECKER").unwrap_or_else(|_| "eventb-checker".to_string())
}

fn oracle_available(oracle: &str) -> bool {
    Command::new(oracle)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// One probed formula: a unique label, its `.eventb` axiom text, and the two
/// closures that yield the bare and rossi-side strings to compare.
struct Case {
    label: String,
    axiom: String,
    /// Whether rossi rejects the bare form.
    rossi_rejects: bool,
}

fn expr_case(label: String, expr: &str) -> Case {
    let rossi_rejects = parse_expression_str(expr).is_err();
    Case {
        axiom: format!("@{label} r = {expr}"),
        label,
        rossi_rejects,
    }
}

fn pred_case(label: &str, pred: &str) -> Case {
    let rossi_rejects = parse_predicate_str(pred).is_err();
    Case {
        label: label.to_string(),
        axiom: format!("@{label} {pred}"),
        rossi_rejects,
    }
}

fn all_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    // Every ordered set-operator pair: `a <i> b <j> c`.
    for (i, gi) in SET_OPS.iter().enumerate() {
        for (j, gj) in SET_OPS.iter().enumerate() {
            cases.push(expr_case(format!("s_{i}_{j}"), &format!("a {gi} b {gj} c")));
        }
    }
    // Predicate connective cases.
    cases.push(pred_case("p_and_or", "x > 0 ∧ y > 0 ∨ z > 0"));
    cases.push(pred_case("p_or_and", "x > 0 ∨ y > 0 ∧ z > 0"));
    cases.push(pred_case("p_and_exists", "x > 0 ∧ ∃w·w > 0"));
    cases.push(pred_case("p_or_forall", "x > 0 ∨ ∀w·w > 0"));
    cases.push(pred_case("p_paren_exists", "x > 0 ∧ (∃w·w > 0)"));
    cases.push(pred_case("p_and_and", "x > 0 ∧ y > 0 ∧ z > 0"));
    cases.push(pred_case("p_paren_left", "(x > 0 ∧ y > 0) ∨ z > 0"));
    cases.push(pred_case("p_forall_body", "∀w·w > 0 ∧ x > 0"));
    // A trailing bare quantifier under ∧/∨ is allowed iff a closing bracket
    // bounds it; the rule propagates into bodies but resets at ∣-such-that.
    cases.push(pred_case("p_paren_q", "(x > 0 ∧ ∃w·w > 0)"));
    cases.push(pred_case("p_compr_q", "c = {x ∣ x > 0 ∧ ∃w·w > 0}"));
    cases.push(pred_case(
        "p_compr_q_mid",
        "c = {x ∣ x > 0 ∧ ∃w·w > 0 ∧ x > 1}",
    ));
    cases.push(pred_case("p_imp_paren_q", "x > 1 ⇒ (x > 0 ∧ ∃w·w > 0)"));
    cases.push(pred_case("p_paren_forall_q", "(∀x·x > 0 ∧ ∃w·w > 0)"));
    cases.push(pred_case(
        "p_compr_forall_q",
        "c = {z ∣ ∀x·x > 0 ∧ ∃w·w > 0}",
    ));
    cases.push(pred_case("p_lambda_q", "c = (λx·x > 0 ∧ ∃w·w > 0 ∣ x)"));
    cases.push(pred_case(
        "p_explicit_compr_q",
        "c = {x·x > 0 ∧ ∃w·w > 0 ∣ x}",
    ));
    // ⇒/⇔ are each non-associative and mutually incompatible: chaining or
    // mixing them needs parentheses, even inside a surrounding bracket. A bare
    // quantifier may not be a ⇒/⇔ operand either, with the ∧/∨ closing-bracket
    // exception. Their precedence relative to ∧/∨ is real and stands bare.
    cases.push(pred_case("p_imp_chain", "x > 0 ⇒ y > 0 ⇒ z > 0"));
    cases.push(pred_case("p_eqv_chain", "x > 0 ⇔ y > 0 ⇔ z > 0"));
    cases.push(pred_case("p_imp_eqv", "x > 0 ⇒ y > 0 ⇔ z > 0"));
    cases.push(pred_case("p_eqv_imp", "x > 0 ⇔ y > 0 ⇒ z > 0"));
    cases.push(pred_case("p_imp_chain_paren", "(x > 0 ⇒ y > 0 ⇒ z > 0)"));
    cases.push(pred_case("p_imp_grouped", "x > 0 ⇒ (y > 0 ⇒ z > 0)"));
    cases.push(pred_case("p_imp_grouped_left", "(x > 0 ⇒ y > 0) ⇒ z > 0"));
    cases.push(pred_case("p_imp_or", "x > 0 ⇒ y > 0 ∨ z > 0"));
    cases.push(pred_case("p_and_imp", "x > 0 ∧ y > 0 ⇒ z > 0"));
    cases.push(pred_case("p_imp_exists", "x > 0 ⇒ ∃w·w > 0"));
    cases.push(pred_case("p_eqv_exists", "x > 0 ⇔ ∃w·w > 0"));
    cases.push(pred_case("p_imp_paren_exists", "x > 0 ⇒ (∃w·w > 0)"));
    cases.push(pred_case("p_imp_exists_bracketed", "(x > 0 ⇒ ∃w·w > 0)"));
    cases
}

/// Run the oracle once over a context holding every case, and return the set of
/// labels it rejected with an operator-incompatibility (`EB005` whose message
/// names the incompatibility).
fn oracle_incompatible_labels(oracle: &str, cases: &[Case]) -> BTreeMap<String, String> {
    let mut model = String::from("CONTEXT probe\nCONSTANTS\n    a b c r x y z\nAXIOMS\n");
    for case in cases {
        model.push_str("    ");
        model.push_str(&case.axiom);
        model.push('\n');
    }
    model.push_str("END\n");

    // Per-process filename so concurrent runs don't clobber each other.
    let path = std::env::temp_dir().join(format!(
        "rossi_op_compat_oracle_{}.eventb",
        std::process::id()
    ));
    std::fs::write(&path, model).expect("write probe model");

    let out = Command::new(oracle)
        .args(["check", "--format", "json"])
        .arg(&path)
        .output()
        .expect("run eventb-checker");
    let _ = std::fs::remove_file(&path);

    // Surface the real cause (it lands on stderr) instead of an opaque serde EOF.
    assert!(
        !out.stdout.is_empty(),
        "eventb-checker produced no stdout (status {}):\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("parse eventb-checker JSON");

    let mut rejected = BTreeMap::new();
    for e in json["errors"].as_array().into_iter().flatten() {
        let rule = e["ruleId"].as_str().unwrap_or_default();
        let msg = e["message"].as_str().unwrap_or_default();
        if rule == "EB005"
            && msg.contains("is not compatible")
            && let Some(label) = e["element"].as_str()
        {
            rejected.insert(label.to_string(), msg.to_string());
        }
    }
    rejected
}

#[test]
#[ignore = "needs the eventb-checker CLI; run with --ignored"]
fn operator_compatibility_matches_oracle() {
    let oracle = oracle_bin();
    if !oracle_available(&oracle) {
        eprintln!(
            "SKIP operator_compatibility_matches_oracle: `{oracle}` not runnable. \
             Set EVENTB_CHECKER to the eventb-checker CLI (or a `java -jar` wrapper)."
        );
        return;
    }

    let cases = all_cases();
    let oracle_rejected = oracle_incompatible_labels(&oracle, &cases);

    let mut disagreements = Vec::new();
    for case in &cases {
        let oracle_rejects = oracle_rejected.contains_key(&case.label);
        if oracle_rejects != case.rossi_rejects {
            disagreements.push(format!(
                "  {label}: oracle {o}, rossi {r}",
                label = case.label,
                o = if oracle_rejects { "REJECT" } else { "accept" },
                r = if case.rossi_rejects {
                    "REJECT"
                } else {
                    "accept"
                },
            ));
        }
    }

    assert!(
        disagreements.is_empty(),
        "{} case(s) where rossi disagrees with the oracle:\n{}",
        disagreements.len(),
        disagreements.join("\n")
    );

    // Sanity: the oracle must actually have flagged incompatibilities, so a
    // silently-empty oracle run can't pass as agreement.
    assert!(
        !oracle_rejected.is_empty(),
        "oracle reported no incompatibilities — it likely failed to run the probe"
    );
    let rejected_total = cases.iter().filter(|c| c.rossi_rejects).count();
    eprintln!(
        "operator compatibility: {} cases, {} rejected — rossi agrees with the oracle",
        cases.len(),
        rejected_total
    );
}
