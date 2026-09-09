//! The published schema against real documents.
//!
//! A schema that has drifted from what the tool writes is worse than none,
//! because a consumer will trust it. These validate freshly produced
//! documents, so the two cannot part company: the schema forbids unknown
//! keys, which catches a field added to the document and not to the schema,
//! and validation catches one removed.
#![cfg(feature = "serde")]

use rossi_build::dump;
use rossi_build::project::{Project, ProjectComponent};

mod common;
use serde_json::Value;
use std::collections::BTreeSet;

fn validator() -> jsonschema::Validator {
    let schema: Value =
        serde_json::from_str(dump::JSON_SCHEMA).expect("the published schema is valid JSON");
    jsonschema::validator_for(&schema).expect("the published schema is a valid JSON Schema")
}

fn document(files: &[(&str, &str)]) -> Value {
    let mut components = Vec::new();
    for (filename, text) in files {
        components.extend(
            ProjectComponent::from_eventb(*filename, text)
                .unwrap_or_else(|e| panic!("{filename} parses: {e}")),
        );
    }
    let project = Project::new("p", components);
    let (build, sc) = rossi_build::check_with_model(&project);
    let model = dump::model(&project, &sc, &build, &dump::Options::default());
    serde_json::to_value(&model).expect("the document serializes")
}

fn assert_valid(name: &str, document: &Value) {
    let validator = validator();
    let errors: Vec<String> = validator
        .iter_errors(document)
        .map(|e| format!("{}: {e}", e.instance_path()))
        .collect();
    assert!(
        errors.is_empty(),
        "{name} does not match the schema:\n{}",
        errors.join("\n")
    );
}

#[test]
fn the_published_schema_is_a_valid_schema() {
    let _ = validator();
}

/// The schema lists the operator names a document may use. Validating real
/// documents only exercises the operators those models happen to contain, so
/// a name added to the converter and forgotten in the schema would go
/// unnoticed until a consumer hit it. This compares the two lists directly.
#[test]
fn the_schema_lists_every_operator_the_converter_can_emit() {
    use rossi::formula::tag::{
        AssocExprOp, AssocPredOp, AtomicOp, BinaryExprOp, BinaryPredOp, LiteralPredOp, QuantExprOp,
        QuantPredOp, RelationalOp, UnaryExprOp,
    };

    let schema: Value = serde_json::from_str(dump::JSON_SCHEMA).expect("the schema parses");
    let listed: BTreeSet<String> = schema["$defs"]["op"]["enum"]
        .as_array()
        .expect("the operator enum")
        .iter()
        .map(|v| v.as_str().expect("an operator name").to_string())
        .collect();

    let mut emitted: BTreeSet<String> = BTreeSet::new();
    // The operators that come from a table.
    emitted.extend(
        RelationalOp::ALL
            .iter()
            .map(|op| dump::op_name_relational(*op).to_string()),
    );
    emitted.extend(
        BinaryExprOp::ALL
            .iter()
            .map(|op| dump::op_name_binary_expr(*op).to_string()),
    );
    emitted.extend(
        BinaryPredOp::ALL
            .iter()
            .map(|op| dump::op_name_binary_pred(*op).to_string()),
    );
    emitted.extend(
        AssocExprOp::ALL
            .iter()
            .map(|op| dump::op_name_assoc_expr(*op).to_string()),
    );
    emitted.extend(
        AssocPredOp::ALL
            .iter()
            .map(|op| dump::op_name_assoc_pred(*op).to_string()),
    );
    emitted.extend(
        AtomicOp::ALL
            .iter()
            .map(|op| dump::op_name_atomic(*op).to_string()),
    );
    emitted.extend(
        LiteralPredOp::ALL
            .iter()
            .map(|op| dump::op_name_literal_pred(*op).to_string()),
    );
    emitted.extend(
        UnaryExprOp::ALL
            .iter()
            .map(|op| dump::op_name_unary_expr(*op).to_string()),
    );
    emitted.extend(
        QuantExprOp::ALL
            .iter()
            .map(|op| dump::op_name_quant_expr(*op).to_string()),
    );
    emitted.extend(
        QuantPredOp::ALL
            .iter()
            .map(|op| dump::op_name_quant_pred(*op).to_string()),
    );
    // The operators the converter names directly, having no operator enum.
    for fixed in dump::FIXED_OP_NAMES {
        emitted.insert((*fixed).to_string());
    }

    let missing: Vec<&String> = emitted.difference(&listed).collect();
    let extra: Vec<&String> = listed.difference(&emitted).collect();
    assert!(
        missing.is_empty(),
        "the converter can emit operators the schema does not list: {missing:?}"
    );
    assert!(
        extra.is_empty(),
        "the schema lists operators the converter never emits: {extra:?}"
    );
}

#[test]
fn the_shared_example_models_match_the_schema() {
    for (name, files) in common::DUMP_MODELS {
        let project = common::example_project(name, files);
        let (build, sc) = rossi_build::check_with_model(&project);
        let model = dump::model(&project, &sc, &build, &dump::Options::default());
        let document = serde_json::to_value(&model).expect("the document serializes");
        assert_valid(name, &document);
    }
}

/// Every element kind and every assignment form in one project, so the parts
/// a simpler model never reaches are validated too.
const EVERYTHING: &str = "\
context c
sets S
constants k
axioms
    @axm1 k ∈ S
end
";

const EVERYTHING_ABSTRACT: &str = "\
machine base
sees c
variables
    v
    f
invariants
    @inv1 v ∈ ℕ
    @thm1 theorem v ≥ 0
    @inv2 f ∈ S ⇸ ℕ
variant
    v
events
event INITIALISATION
then
    @act1 v ≔ 0
    @act2 f ≔ ∅
end
event pick
any
    p
where
    @grd1 p ∈ ℕ
then
    @act1 v :∈ ℕ
end
event guess
status convergent
where
    @grd1 v > 0
then
    @act1 v :∣ v' < v
end
event noop
then
    @act1 skip
end
end
";

const EVERYTHING_CONCRETE: &str = "\
machine ref
refines base
sees c
variables
    v
    f
invariants
    @inv3 v ≤ 100
events
event INITIALISATION extends INITIALISATION
end
event pick
refines pick
any
    q
where
    @grd1 q ∈ ℕ
with
    @p p = q
then
    @act1 v :∈ ℕ
end
event guess extends guess
end
event noop extends noop
end
end
";

#[test]
fn every_element_kind_matches_the_schema() {
    let document = document(&[
        ("c.buc", EVERYTHING),
        ("base.bum", EVERYTHING_ABSTRACT),
        ("ref.bum", EVERYTHING_CONCRETE),
    ]);

    // The project has to exercise the parts, not merely be accepted.
    let machines = document["machines"].as_array().expect("machines");
    let base = machines
        .iter()
        .find(|m| m["name"] == "base")
        .expect("the abstract machine");
    assert!(!base["variants"].as_array().expect("variants").is_empty());
    let ops: Vec<&str> = base["events"]
        .as_array()
        .expect("events")
        .iter()
        .flat_map(|e| e["actions"].as_array().expect("actions"))
        .filter_map(|a| a.get("assignment").and_then(|x| x["op"].as_str()))
        .collect();
    assert!(ops.contains(&"BECOMES_EQUAL_TO"));
    assert!(ops.contains(&"BECOMES_MEMBER_OF"));
    assert!(ops.contains(&"BECOMES_SUCH_THAT"));

    let refined = machines
        .iter()
        .find(|m| m["name"] == "ref")
        .expect("the refinement");
    let witnesses: usize = refined["events"]
        .as_array()
        .expect("events")
        .iter()
        .map(|e| e["witnesses"].as_array().expect("witnesses").len())
        .sum();
    assert!(witnesses > 0, "the project exercises no witness");

    assert_valid("everything", &document);
}

#[test]
fn a_document_with_errors_still_matches_the_schema() {
    // A rejected project still produces a document, so the schema has to
    // accept one whose elements are incomplete.
    let broken = "\
machine m
variables
    v
invariants
    @inv1 v ∈ undeclared_set
events
event INITIALISATION
then
    @act1 v ≔ 0
end
end
";
    let document = document(&[("m.bum", broken)]);
    let errors = document["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .filter(|d| d["severity"] == "error")
        .count();
    assert!(errors > 0, "the project was expected to fail checking");

    assert_valid("broken", &document);
}

#[test]
fn the_schema_rejects_what_the_format_does_not_define() {
    // A schema that accepts anything would pass every test above while
    // telling a consumer nothing, so its strictness is checked directly.
    let validator = validator();
    let mut document = document(&[("c.buc", EVERYTHING)]);

    document["contexts"][0]["surprise"] = Value::from(1);
    assert!(
        !validator.is_valid(&document),
        "an unknown key was accepted"
    );

    let mut document = document.clone();
    document["contexts"][0]
        .as_object_mut()
        .expect("a context object")
        .remove("surprise");
    assert!(
        validator.is_valid(&document),
        "the document should be valid again"
    );

    // An operator name outside the emitted vocabulary, such as one of
    // Rodin's deprecated projection spellings.
    document["contexts"][0]["axioms"][0]["predicate"]["op"] = Value::from("KPRJ1");
    assert!(
        !validator.is_valid(&document),
        "an operator outside the vocabulary was accepted"
    );
}
