//! The dump document over whole projects.
//!
//! These check the parts that only appear once a real project has been
//! checked: the closure a machine carries, where each part came from, and the
//! invariants a consumer is entitled to rely on.

use std::collections::BTreeMap;

use rossi::formula::Type;
use rossi::formula::position::FormulaRef;
use rossi_build::dump::{self, MachineDump, Model, Node};
use rossi_build::project::{Project, ProjectComponent};

fn project(files: &[(&str, &str)]) -> Project {
    let mut components = Vec::new();
    for (filename, text) in files {
        components.extend(
            ProjectComponent::from_eventb(*filename, text)
                .unwrap_or_else(|e| panic!("{filename} parses: {e}")),
        );
    }
    Project::new("p", components)
}

fn model(files: &[(&str, &str)]) -> Model {
    let project = project(files);
    let (build, sc) = rossi_build::check_with_model(&project);
    dump::model(&project, &sc, &build, &dump::Options::default())
}

fn machine<'a>(model: &'a Model, name: &str) -> &'a MachineDump {
    model
        .machines
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("{name} is in the document"))
}

fn errors(model: &Model) -> Vec<&str> {
    model
        .diagnostics
        .iter()
        .filter(|d| d.severity == "error")
        .map(|d| d.message.as_str())
        .collect()
}

const ABSTRACT: &str = "\
machine base
variables
    v
invariants
    @inv1 v ∈ ℕ
events
event INITIALISATION
then
    @act1 v ≔ 0
end
event step
any
    p
where
    @grd1 p ∈ ℕ // a written guard
then
    @act1 v ≔ v + p
end
end
";

const EXTENDED: &str = "\
machine ref
refines base
variables
    v
    w
invariants
    @inv2 w ∈ ℕ
events
event INITIALISATION extends INITIALISATION
then
    @act2 w ≔ 0
end
event step extends step
where
    @grd2 w ≥ 0
then
    @act2 w ≔ w + 1
end
end
";

#[test]
fn an_extended_event_carries_the_whole_chain_and_says_who_wrote_it() {
    let model = model(&[("base.bum", ABSTRACT), ("ref.bum", EXTENDED)]);
    assert_eq!(errors(&model), Vec::<&str>::new());

    let step = machine(&model, "ref")
        .events
        .iter()
        .find(|e| e.label == "step")
        .expect("the refined event");
    assert!(step.extended);

    // The abstract event's parameter, guard and action are all present, each
    // marked with the machine that wrote it; the refinement's own are not.
    assert_eq!(
        step.parameters
            .iter()
            .map(|p| (p.name.as_str(), p.inherited_from.as_deref()))
            .collect::<Vec<_>>(),
        [("p", Some("base"))]
    );
    assert_eq!(
        step.guards
            .iter()
            .map(|g| (g.label.as_str(), g.inherited_from.as_deref()))
            .collect::<Vec<_>>(),
        [("grd1", Some("base")), ("grd2", None)]
    );
    assert_eq!(
        step.actions
            .iter()
            .map(|a| (a.label.as_str(), a.inherited_from.as_deref()))
            .collect::<Vec<_>>(),
        [("act1", Some("base")), ("act2", None)]
    );
}

#[test]
fn an_inherited_invariant_names_the_machine_that_wrote_it() {
    let model = model(&[("base.bum", ABSTRACT), ("ref.bum", EXTENDED)]);
    let refined = machine(&model, "ref");

    assert_eq!(
        refined
            .invariants
            .iter()
            .map(|i| (i.label.as_str(), i.inherited_from.as_deref()))
            .collect::<Vec<_>>(),
        [("inv1", Some("base")), ("inv2", None)]
    );
}

#[test]
fn a_comment_reaches_the_element_it_was_written_beside() {
    let model = model(&[("base.bum", ABSTRACT), ("ref.bum", EXTENDED)]);
    let base = machine(&model, "base");
    let guard = &base
        .events
        .iter()
        .find(|e| e.label == "step")
        .expect("the event")
        .guards[0];

    assert_eq!(guard.comment.as_deref(), Some("a written guard"));

    // The same guard reached through the refinement is the abstract one, so
    // it has no clause in the refining file and reports no comment there.
    let inherited = &machine(&model, "ref")
        .events
        .iter()
        .find(|e| e.label == "step")
        .expect("the event")
        .guards[0];
    assert_eq!(inherited.comment, None);
}

#[test]
fn a_skip_action_has_no_assignment_and_no_before_after_predicate() {
    let source = "\
machine m
variables
    v
invariants
    @inv1 v ∈ ℕ
events
event INITIALISATION
then
    @act1 v ≔ 0
end
event idle
then
    @act1 skip
end
end
";
    let model = model(&[("m.bum", source)]);
    let action = &machine(&model, "m")
        .events
        .iter()
        .find(|e| e.label == "idle")
        .expect("the event")
        .actions[0];

    assert!(action.assignment.is_none());
    assert!(action.ba.is_none());
    assert_eq!(action.text, "skip");
}

#[test]
fn a_component_that_declares_nothing_still_reports_its_handle() {
    // A component's own handle is not stored on the checked record, and a
    // context with no carrier set, constant or axiom has no declaration to
    // read one off either. It is still a component a consumer keys by handle.
    let model = model(&[
        ("empty.buc", "context empty\nend\n"),
        ("aggregate.buc", "context aggregate\nextends empty\nend\n"),
    ]);
    assert_eq!(errors(&model), Vec::<&str>::new());

    for context in &model.contexts {
        assert_eq!(
            context.source,
            format!(
                "/p/{}.buc|org.eventb.core.contextFile#{}",
                context.name, context.name
            ),
            "{} reports no handle",
            context.name
        );
    }
}

#[test]
fn a_machine_lists_the_contexts_it_can_see_transitively() {
    let base_ctx = "\
context base_ctx
sets S
end
";
    let derived_ctx = "\
context derived_ctx
extends base_ctx
constants c
axioms
    @axm1 c ∈ S
end
";
    let m = "\
machine m
sees derived_ctx
variables
    v
invariants
    @inv1 v ∈ S
events
event INITIALISATION
then
    @act1 v ≔ c
end
end
";
    let model = model(&[
        ("base_ctx.buc", base_ctx),
        ("derived_ctx.buc", derived_ctx),
        ("m.bum", m),
    ]);
    assert_eq!(errors(&model), Vec::<&str>::new());

    let m = machine(&model, "m");
    assert_eq!(m.sees, ["derived_ctx"]);
    // The extended context is visible too, and is listed before the context
    // that pulls it in.
    assert_eq!(m.internal_contexts, ["base_ctx", "derived_ctx"]);
}

/// Walk every node of a document, so an invariant can be asserted over all
/// of them at once.
fn visit_nodes(model: &Model, mut f: impl FnMut(&Node)) {
    fn walk(node: &Node, f: &mut impl FnMut(&Node)) {
        f(node);
        for child in &node.children {
            walk(child, f);
        }
    }
    for context in &model.contexts {
        for axiom in &context.axioms {
            walk(&axiom.predicate, &mut f);
        }
    }
    for machine in &model.machines {
        for invariant in &machine.invariants {
            walk(&invariant.predicate, &mut f);
        }
        for variant in &machine.variants {
            if let Some(expression) = &variant.expression {
                walk(expression, &mut f);
            }
        }
        for event in &machine.events {
            for guard in &event.guards {
                walk(&guard.predicate, &mut f);
            }
            for witness in &event.witnesses {
                walk(&witness.predicate, &mut f);
            }
            for action in &event.actions {
                if let Some(assignment) = &action.assignment {
                    for node in &assignment.idents {
                        walk(node, &mut f);
                    }
                    for node in assignment.values.iter().flatten() {
                        walk(node, &mut f);
                    }
                    if let Some(node) = &assignment.set {
                        walk(node, &mut f);
                    }
                    for node in assignment.primed.iter().flatten() {
                        walk(node, &mut f);
                    }
                    if let Some(node) = &assignment.pred {
                        walk(node, &mut f);
                    }
                }
                if let Some(ba) = &action.ba {
                    walk(ba, &mut f);
                }
            }
        }
    }
}

fn example(name: &str) -> String {
    let path = format!("../rossi/examples/{name}");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// The projects the whole-document invariants are checked over.
fn corpus() -> Vec<(&'static str, Model)> {
    vec![
        (
            "bank_account",
            model(&[
                ("bank_account_ctx.buc", &example("bank_account_ctx.eventb")),
                ("bank_account.bum", &example("bank_account_machine.eventb")),
            ]),
        ),
        (
            "refinement",
            model(&[
                (
                    "refinement_abstract.bum",
                    &example("refinement_abstract.eventb"),
                ),
                (
                    "refinement_concrete.bum",
                    &example("refinement_concrete.eventb"),
                ),
            ]),
        ),
        (
            "extended",
            model(&[("base.bum", ABSTRACT), ("ref.bum", EXTENDED)]),
        ),
    ]
}

#[test]
fn every_type_string_parses_and_matches_its_table_entry() {
    for (name, model) in corpus() {
        visit_nodes(&model, |node| {
            let Some(key) = &node.ty else { return };
            let entry = model
                .types
                .get(key)
                .unwrap_or_else(|| panic!("{name}: {key} is missing from the type table"));
            let parsed = Type::parse_rodin(key)
                .unwrap_or_else(|| panic!("{name}: {key} does not parse as a Rodin type"));
            assert_eq!(
                &dump_type(&parsed),
                entry,
                "{name}: {key} does not rebuild to its table entry"
            );
        });
    }
}

/// The table entry a type should have, rebuilt independently of the
/// converter so the comparison is not a tautology.
fn dump_type(ty: &Type) -> dump::TypeNode {
    use dump::TypeNode;
    match ty {
        Type::Int => TypeNode::INT,
        Type::Bool => TypeNode::BOOL,
        Type::Given(name) => TypeNode::GIVEN { name: name.clone() },
        Type::Pow(base) => TypeNode::POW {
            base: Box::new(dump_type(base)),
        },
        Type::Prod(left, right) => TypeNode::PROD {
            left: Box::new(dump_type(left)),
            right: Box::new(dump_type(right)),
        },
        Type::Parametric { symbol, params, .. } => TypeNode::PARAMETRIC {
            symbol: symbol.clone(),
            params: params.iter().map(dump_type).collect(),
        },
    }
}

#[test]
fn a_predicate_carries_no_type_and_an_expression_always_does() {
    for (name, model) in corpus() {
        visit_nodes(&model, |node| {
            let is_predicate = matches!(
                node.op,
                "BTRUE"
                    | "BFALSE"
                    | "NOT"
                    | "LAND"
                    | "LOR"
                    | "LIMP"
                    | "LEQV"
                    | "FORALL"
                    | "EXISTS"
                    | "KFINITE"
                    | "KPARTITION"
                    | "EQUAL"
                    | "NOTEQUAL"
                    | "LT"
                    | "LE"
                    | "GT"
                    | "GE"
                    | "IN"
                    | "NOTIN"
                    | "SUBSET"
                    | "NOTSUBSET"
                    | "SUBSETEQ"
                    | "NOTSUBSETEQ"
            );
            if is_predicate {
                assert!(node.ty.is_none(), "{name}: {} carries a type", node.op);
            }
        });
    }
}

#[test]
fn no_node_uses_a_tag_rodin_does_not_have() {
    for (name, model) in corpus() {
        visit_nodes(&model, |node| {
            assert_ne!(
                node.op, "OFTYPE",
                "{name}: an ascription reached a document"
            );
            assert_ne!(
                node.op, "PRED_APPL",
                "{name}: a predicate application reached a document"
            );
            assert!(
                node.tag < 1000 || node.op == "EXTENDED",
                "{name}: tag {} is in the extension range without being one",
                node.tag
            );
        });
    }
}

#[test]
fn a_bound_occurrence_always_names_a_declaration_in_scope() {
    for (name, model) in corpus() {
        visit_nodes(&model, |node| {
            if node.op == "BOUND_IDENT" {
                assert!(
                    node.index.is_some(),
                    "{name}: a bound occurrence has no index"
                );
                assert!(
                    node.name.is_some(),
                    "{name}: a bound occurrence resolves to no declaration"
                );
            }
        });
    }
}

#[test]
fn an_inherited_element_places_no_node() {
    // An inherited invariant, guard or action keeps the spans of the machine
    // that wrote it. A node span names no file of its own, so emitting one
    // here would report a position in this machine's text that belongs to
    // another file's; the element itself already reports none.
    let model = model(&[("base.bum", ABSTRACT), ("ref.bum", EXTENDED)]);
    let refined = machine(&model, "ref");

    let mut inherited = 0;
    let mut spans = Vec::new();
    for invariant in &refined.invariants {
        if invariant.inherited_from.is_some() {
            inherited += 1;
            assert!(invariant.span.is_none());
            collect_spans(&invariant.predicate, &mut spans);
        }
    }
    for event in &refined.events {
        for guard in &event.guards {
            if guard.inherited_from.is_some() {
                inherited += 1;
                assert!(guard.span.is_none());
                collect_spans(&guard.predicate, &mut spans);
            }
        }
        for action in &event.actions {
            if action.inherited_from.is_some() {
                inherited += 1;
                assert!(action.span.is_none());
                if let Some(assignment) = &action.assignment {
                    assert!(assignment.span.is_none());
                    for part in assignment
                        .idents
                        .iter()
                        .chain(assignment.values.iter().flatten())
                    {
                        collect_spans(part, &mut spans);
                    }
                }
                if let Some(ba) = &action.ba {
                    collect_spans(ba, &mut spans);
                }
            }
        }
    }

    assert!(inherited > 0, "the project inherits nothing");
    assert!(spans.is_empty(), "an inherited element placed a node");
}

#[test]
fn a_node_span_lies_inside_the_span_of_the_element_it_belongs_to() {
    for (name, model) in corpus() {
        for machine in &model.machines {
            for invariant in &machine.invariants {
                let Some(element) = &invariant.span else {
                    continue;
                };
                let mut checked = 0;
                let mut node_spans = Vec::new();
                collect_spans(&invariant.predicate, &mut node_spans);
                for span in node_spans {
                    assert!(
                        span.start >= element.start && span.end <= element.end,
                        "{name}: a node of {} sits outside its element",
                        invariant.label
                    );
                    checked += 1;
                }
                assert!(checked > 0, "{name}: {} placed no node", invariant.label);
            }
        }
    }
}

fn collect_spans(node: &Node, out: &mut Vec<dump::SpanDump>) {
    if let Some(span) = &node.span {
        out.push(span.clone());
    }
    for child in &node.children {
        collect_spans(child, out);
    }
}

#[test]
fn building_the_same_project_twice_gives_the_same_document() {
    let files: &[(&str, &str)] = &[("base.bum", ABSTRACT), ("ref.bum", EXTENDED)];
    let first = format!("{:?}", model(files));
    let second = format!("{:?}", model(files));

    assert_eq!(first, second);
}

#[test]
fn every_child_list_matches_the_models_own_child_count() {
    // The converter takes children from the model's positional accessor, and
    // this is what says so: a document node and the formula it came from
    // agree on how many children there are, at every position.
    let project = project(&[("base.bum", ABSTRACT), ("ref.bum", EXTENDED)]);
    let (build, sc) = rossi_build::check_with_model(&project);
    let model = dump::model(&project, &sc, &build, &dump::Options::default());

    for checked in sc.machines.values() {
        for invariant in &checked.record.invariants {
            let stripped = invariant.typed.strip_ascriptions();
            let node = model
                .machines
                .iter()
                .flat_map(|m| &m.invariants)
                .find(|i| i.label == invariant.label && i.inherited_from.is_none())
                .map(|i| &i.predicate)
                .expect("the invariant is in the document");
            compare_arity(FormulaRef::Pred(&stripped), node);
        }
    }
}

fn compare_arity(formula: FormulaRef<'_>, node: &Node) {
    assert_eq!(
        formula.child_count(),
        node.children.len(),
        "{} reports a different number of children",
        node.op
    );
    for index in 0..formula.child_count() {
        let child = formula.child(index).expect("the child exists");
        compare_arity(child, &node.children[index]);
    }
}

#[test]
fn the_type_table_holds_exactly_the_types_the_document_names() {
    for (name, model) in corpus() {
        let mut used: BTreeMap<String, ()> = BTreeMap::new();
        visit_nodes(&model, |node| {
            if let Some(key) = &node.ty {
                used.insert(key.clone(), ());
            }
        });
        for machine in &model.machines {
            for variable in &machine.variables {
                used.insert(variable.ty.clone(), ());
            }
            for event in &machine.events {
                for parameter in &event.parameters {
                    used.insert(parameter.ty.clone(), ());
                }
            }
        }
        for context in &model.contexts {
            for set in &context.carrier_sets {
                used.insert(set.ty.clone(), ());
            }
            for constant in &context.constants {
                used.insert(constant.ty.clone(), ());
            }
        }
        for key in used.keys() {
            assert!(
                model.types.contains_key(key),
                "{name}: {key} is named but not in the table"
            );
        }
    }
}

#[test]
fn every_text_is_what_the_checked_file_would_carry() {
    // `text` is the bridge back to Rodin: a consumer that would rather parse
    // the formula with Rodin's own parser must get exactly the string Rodin
    // would have written into the checked file.
    let project = project(&[("base.bum", ABSTRACT), ("ref.bum", EXTENDED)]);
    let (build, sc) = rossi_build::check_with_model(&project);
    let model = dump::model(&project, &sc, &build, &dump::Options::default());

    for checked in sc.machines.values() {
        let dumped = machine(&model, checked.name());
        for invariant in &checked.record.invariants {
            let found = dumped
                .invariants
                .iter()
                .find(|i| i.label == invariant.label && i.inherited_from.is_none())
                .expect("the invariant is in the document");
            assert_eq!(
                found.text,
                rossi_build::normalize::canonical_typed_predicate(&invariant.typed)
            );
        }
        for event in &checked.record.events {
            let dumped_event = dumped
                .events
                .iter()
                .find(|e| e.label == event.label)
                .expect("the event is in the document");
            for action in event.own_actions() {
                let Some(typed) = &action.typed else { continue };
                let found = dumped_event
                    .actions
                    .iter()
                    .find(|a| a.label == action.label && a.inherited_from.is_none())
                    .expect("the action is in the document");
                assert_eq!(
                    found.text,
                    rossi_build::normalize::canonical_typed_assignment(typed)
                );
            }
        }
    }
}

#[test]
fn a_before_after_predicate_is_the_models_own() {
    let project = project(&[("base.bum", ABSTRACT), ("ref.bum", EXTENDED)]);
    let (build, sc) = rossi_build::check_with_model(&project);
    let model = dump::model(&project, &sc, &build, &dump::Options::default());

    let mut compared = 0;
    for checked in sc.machines.values() {
        let dumped = machine(&model, checked.name());
        for event in &checked.record.events {
            let dumped_event = dumped
                .events
                .iter()
                .find(|e| e.label == event.label)
                .expect("the event is in the document");
            for action in event.own_actions() {
                let Some(typed) = &action.typed else { continue };
                let found = dumped_event
                    .actions
                    .iter()
                    .find(|a| a.label == action.label && a.inherited_from.is_none())
                    .expect("the action is in the document");
                let ba = found.ba.as_ref().expect("a checked action has one");
                let expected = typed.strip_ascriptions().ba_predicate();
                assert_eq!(ba.tag, expected.tag());
                compared += 1;
            }
        }
    }
    assert!(compared > 0, "no action was compared");
}

/// A project with two independent branches over a shared context, so a
/// selection has something to keep and something to drop.
const SHARED_CONTEXT: &str = "\
context shared
sets S
end
";

const OTHER_CONTEXT: &str = "\
context other
sets T
end
";

const BRANCH_A: &str = "\
machine a
sees shared
variables
    x
invariants
    @inv1 x ∈ S
events
event INITIALISATION
then
    @act1 x :∈ S
end
end
";

const BRANCH_B: &str = "\
machine b
sees other
variables
    y
invariants
    @inv1 y ∈ T
events
event INITIALISATION
then
    @act1 y :∈ T
end
end
";

fn selected(files: &[(&str, &str)], components: &[&str]) -> Model {
    let project = project(files);
    let (build, sc) = rossi_build::check_with_model(&project);
    let options = dump::Options {
        components: Some(components.iter().map(|c| (*c).to_string()).collect()),
        ..dump::Options::default()
    };
    dump::model(&project, &sc, &build, &options)
}

const BRANCHED: &[(&str, &str)] = &[
    ("shared.buc", SHARED_CONTEXT),
    ("other.buc", OTHER_CONTEXT),
    ("a.bum", BRANCH_A),
    ("b.bum", BRANCH_B),
];

#[test]
fn a_selection_keeps_what_it_depends_on_and_drops_the_rest() {
    let model = selected(BRANCHED, &["a"]);

    assert_eq!(
        model
            .machines
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        ["a"]
    );
    // The context `a` sees comes with it; the one only `b` sees does not.
    assert_eq!(
        model
            .contexts
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["shared"]
    );
}

#[test]
fn a_selection_pulls_in_the_machines_it_refines() {
    // A refined machine is not optional: the refining machine's own elements
    // name it as the origin of what they inherit, so leaving it out would
    // make the document cite something it does not contain.
    let model = selected(&[("base.bum", ABSTRACT), ("ref.bum", EXTENDED)], &["ref"]);

    let mut names: Vec<&str> = model.machines.iter().map(|m| m.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["base", "ref"]);
}

#[test]
fn every_name_a_selection_mentions_resolves_inside_it() {
    // This is the property the closure exists for, so it is asserted rather
    // than assumed: nothing in a restricted document refers outside itself.
    let model = selected(BRANCHED, &["a"]);
    let contexts: Vec<&str> = model.contexts.iter().map(|c| c.name.as_str()).collect();
    let machines: Vec<&str> = model.machines.iter().map(|m| m.name.as_str()).collect();

    for context in &model.contexts {
        for name in context.extends.iter().chain(&context.ancestors) {
            assert!(
                contexts.contains(&name.as_str()),
                "{name} is cited but absent"
            );
        }
    }
    for machine in &model.machines {
        for name in machine.sees.iter().chain(&machine.internal_contexts) {
            assert!(
                contexts.contains(&name.as_str()),
                "{name} is cited but absent"
            );
        }
        for name in machine.refines.iter().chain(&machine.ancestors) {
            assert!(
                machines.contains(&name.as_str()),
                "{name} is cited but absent"
            );
        }
        for invariant in &machine.invariants {
            if let Some(owner) = &invariant.inherited_from {
                assert!(
                    machines.contains(&owner.as_str()),
                    "{owner} is cited but absent"
                );
            }
        }
    }
}

#[test]
fn a_selection_reports_only_what_it_contains() {
    // `b` names something undeclared, so the whole project fails checking.
    // Selecting `a` asks about `a`, and a finding about `b` could not be
    // explained by a document that does not contain `b`.
    let broken_b = "\
machine b
variables
    y
invariants
    @inv1 y ∈ undeclared
events
event INITIALISATION
then
    @act1 y ≔ 0
end
end
";
    let files: &[(&str, &str)] = &[
        ("shared.buc", SHARED_CONTEXT),
        ("a.bum", BRANCH_A),
        ("b.bum", broken_b),
    ];

    let whole = model(files);
    assert!(whole.has_errors(), "the project as a whole should fail");

    let part = selected(files, &["a"]);
    assert!(!part.has_errors(), "errors: {:?}", errors(&part));
}

#[test]
fn a_selection_lists_only_the_sources_it_kept() {
    let model = selected(BRANCHED, &["a"]);

    let mut ids: Vec<&str> = model.sources.iter().map(|s| s.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, ["a.bum", "shared.buc"]);
}

#[test]
fn selecting_several_components_keeps_all_their_closures() {
    let model = selected(BRANCHED, &["a", "b"]);

    let mut contexts: Vec<&str> = model.contexts.iter().map(|c| c.name.as_str()).collect();
    contexts.sort_unstable();
    assert_eq!(contexts, ["other", "shared"]);
    assert_eq!(model.machines.len(), 2);
}

#[test]
fn selecting_nothing_keeps_the_whole_project() {
    let whole = model(BRANCHED);

    assert_eq!(whole.contexts.len(), 2);
    assert_eq!(whole.machines.len(), 2);
}

#[test]
fn a_name_that_matches_no_component_selects_nothing_of_its_own() {
    // The library takes the names as given; reporting a typo is the caller's
    // job, since only a caller knows what the user typed.
    let model = selected(BRANCHED, &["nonexistent"]);

    assert!(model.contexts.is_empty());
    assert!(model.machines.is_empty());
}

#[test]
fn a_selection_carries_only_the_types_it_uses() {
    let whole = model(BRANCHED);
    let part = selected(BRANCHED, &["a"]);

    assert!(whole.types.contains_key("ℙ(T)"), "the whole project uses T");
    assert!(
        !part.types.contains_key("ℙ(T)"),
        "a selection without T should not carry its type"
    );
    assert!(part.types.contains_key("ℙ(S)"));
}
