//! Advisory lint passes over a parsed [`Project`].
//!
//! Unlike the SC pipeline in `crate::sc`, these passes don't read or write
//! the on-disk `.bcc`/`.bcm` representation — they walk the AST and emit
//! advisory [`Diagnostic`]s. `crate::build()` does **not** invoke [`run`];
//! callers (today: `rossi validate`) opt in explicitly so the existing
//! `rossi build` output stays stable.
//!
//! Coverage (rule IDs mirror eventb-checker's catalogue):
//!
//! - **EB011** dead variable     — declared, never referenced
//! - **EB012** unmodified var    — referenced, never assigned by any event
//! - **EB013** dead constant     — declared, never referenced in any axiom
//! - **EB014** incomplete INIT   — variable not assigned by INITIALISATION
//! - **EB019** duplicate component — same name defined in more than one file
//! - **EB021** shadowed name     — declared name re-lexes as a textual token
//!
//! EB010 (well-definedness) and EB015–17 (proof status) are deliberately
//! out of scope here; they need their own modules.

use std::collections::{BTreeMap, BTreeSet};

use rossi::{Action, Component, Context, Machine};

use crate::ast_util::lhs_variables;
use crate::project::Project;
use crate::sc::identifier_walker::{
    collect_referenced_in_action_rhs, collect_referenced_in_action_rhs_with_locals,
    collect_referenced_in_expression, collect_referenced_in_predicate,
    collect_referenced_in_predicate_with_locals,
};
use crate::{Diagnostic, RuleId, Severity};

/// Run every available lint over `project` and collect the diagnostics.
#[must_use]
pub fn run(project: &Project) -> Vec<Diagnostic> {
    let mut diags = lint_duplicate_component(project);
    let index = ProjectIndex::build(project);
    for pc in &project.components {
        diags.extend(run_component(&pc.component));
        match &pc.component {
            Component::Machine(m) => {
                let referenced = index.effective_refs_for_machine(m.name.as_str());
                let assigned = index.effective_assigned_for_machine(m.name.as_str());
                diags.extend(lint_dead_variable(m, &referenced));
                diags.extend(lint_unmodified_variable(m, &referenced, &assigned));
                diags.extend(lint_incomplete_init(m));
            }
            Component::Context(c) => {
                let referenced = index.effective_refs_for_context(c.name.as_str());
                diags.extend(lint_dead_constant(c, &referenced));
            }
        }
    }
    diags
}

/// Run the lints that need no cross-component context over one component.
/// Loose `.eventb` text files have no [`Project`] (a single file's SEES /
/// EXTENDS parents are usually absent, so the reference-based lints would
/// false-positive); these local passes are safe to run anywhere.
#[must_use]
pub fn run_component(component: &Component) -> Vec<Diagnostic> {
    match component {
        Component::Machine(m) => lint_shadowed_names_machine(m),
        Component::Context(c) => lint_shadowed_names_context(c),
    }
}

// ---------- individual lint passes -----------------------------------------

fn lint_dead_variable(m: &Machine, referenced: &BTreeSet<String>) -> Vec<Diagnostic> {
    m.variables
        .iter()
        .filter(|v| !referenced.contains(&v.name))
        .map(|v| Diagnostic {
            severity: Severity::Warning,
            origin: format!("{}.{}", m.name, v.name),
            message: format!("variable `{}` is declared but never referenced", v.name),
            rule_id: Some(RuleId::DeadVariable),
        })
        .collect()
}

fn lint_unmodified_variable(
    m: &Machine,
    referenced: &BTreeSet<String>,
    assigned: &BTreeSet<String>,
) -> Vec<Diagnostic> {
    m.variables
        .iter()
        .filter(|v| referenced.contains(&v.name) && !assigned.contains(&v.name))
        .map(|v| Diagnostic {
            severity: Severity::Warning,
            origin: format!("{}.{}", m.name, v.name),
            message: format!(
                "variable `{}` is referenced but never assigned by any event",
                v.name
            ),
            rule_id: Some(RuleId::UnmodifiedVariable),
        })
        .collect()
}

fn lint_dead_constant(c: &Context, referenced: &BTreeSet<String>) -> Vec<Diagnostic> {
    c.constants
        .iter()
        .filter(|k| !referenced.contains(&k.name))
        .map(|k| Diagnostic {
            severity: Severity::Warning,
            origin: format!("{}.{}", c.name, k.name),
            message: format!(
                "constant `{}` is declared but never referenced in any axiom",
                k.name
            ),
            rule_id: Some(RuleId::DeadConstant),
        })
        .collect()
}

fn lint_incomplete_init(m: &Machine) -> Vec<Diagnostic> {
    let Some(init) = &m.initialisation else {
        // No INITIALISATION at all: report once per declared variable.
        return m
            .variables
            .iter()
            .map(|v| Diagnostic {
                severity: Severity::Warning,
                origin: format!("{}.INITIALISATION", m.name),
                message: format!(
                    "variable `{}` is not assigned by INITIALISATION (no INITIALISATION event)",
                    v.name
                ),
                rule_id: Some(RuleId::IncompleteInitialisation),
            })
            .collect();
    };

    // An `extended` INIT inherits the parent's assignments — assume completeness
    // until we have access to the refinement chain in this pass.
    if init.extended {
        return Vec::new();
    }

    let lhs: BTreeSet<&str> = init
        .actions
        .iter()
        .flat_map(|la| lhs_variables(&la.action))
        .collect();
    m.variables
        .iter()
        .filter(|v| !lhs.contains(v.name.as_str()))
        .map(|v| Diagnostic {
            severity: Severity::Warning,
            origin: format!("{}.INITIALISATION", m.name),
            message: format!("variable `{}` is not assigned by INITIALISATION", v.name),
            rule_id: Some(RuleId::IncompleteInitialisation),
        })
        .collect()
}

fn lint_duplicate_component(project: &Project) -> Vec<Diagnostic> {
    let mut by_name: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for pc in &project.components {
        let name = pc.component.name();
        by_name.entry(name).or_default().push(pc.filename.as_str());
    }
    by_name
        .into_iter()
        .filter(|(_, files)| files.len() > 1)
        .map(|(name, files)| Diagnostic {
            severity: Severity::Warning,
            origin: name.to_string(),
            message: format!(
                "component `{name}` is defined in multiple files: {}",
                files.join(", ")
            ),
            rule_id: Some(RuleId::DuplicateComponent),
        })
        .collect()
}

/// EB021: a declared name that rossi's *textual* syntax can re-lex as a
/// token. The parser hard-rejects the kernel_lang §2.2 reserved words
/// ([`rossi::builtins::is_reserved_word`]) but deliberately accepts the rest
/// — Rodin allows them as identifiers, so imported models must load. The
/// trap is silent: a constant `POW` declares fine and `POW = f` works, but
/// `POW(f)` parses as the powerset `ℙ(f)`; a constant `Nat` can never be
/// referenced at all (`Nat` lexes as `ℕ`). Warn at the declaration.
fn shadowed_name_diag(component: &str, kind: &str, name: &str) -> Option<Diagnostic> {
    if !rossi::builtins::is_reserved_name(name) || rossi::builtins::is_reserved_word(name) {
        return None;
    }
    Some(Diagnostic {
        severity: Severity::Warning,
        origin: format!("{component}.{name}"),
        message: format!(
            "{kind} `{name}` collides with rossi's textual operator vocabulary; \
             uses can silently parse as the built-in token instead of this \
             identifier (e.g. `POW(S)` is the powerset, `Nat` is ℕ) — rename it"
        ),
        rule_id: Some(RuleId::ShadowedName),
    })
}

fn lint_shadowed_names_context(c: &Context) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for set in &c.sets {
        diags.extend(shadowed_name_diag(&c.name, "carrier set", set.name()));
        if let rossi::SetDeclaration::Enumerated { elements, .. } = set {
            for e in elements {
                diags.extend(shadowed_name_diag(&c.name, "set element", e));
            }
        }
    }
    for k in &c.constants {
        diags.extend(shadowed_name_diag(&c.name, "constant", &k.name));
    }
    diags
}

fn lint_shadowed_names_machine(m: &Machine) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for v in &m.variables {
        diags.extend(shadowed_name_diag(&m.name, "variable", &v.name));
    }
    for event in &m.events {
        for p in &event.parameters {
            diags.extend(shadowed_name_diag(
                &format!("{}.{}", m.name, event.name),
                "parameter",
                &p.name,
            ));
        }
    }
    diags
}

// ---------- reference collection -------------------------------------------
//
// Traversal lives in `crate::sc::identifier_walker`; this module wires the
// shared collectors through the machine/context AST. Event parameters are
// passed as initial bound names so a guard mentioning a parameter doesn't
// leak that name into the machine-level reference set.

fn referenced_in_machine(m: &Machine) -> BTreeSet<String> {
    let mut acc = BTreeSet::new();
    for inv in &m.invariants {
        collect_referenced_in_predicate(&inv.predicate, &mut acc);
    }
    if let Some(v) = &m.variant {
        collect_referenced_in_expression(v, &mut acc);
    }
    if let Some(init) = &m.initialisation {
        for la in &init.actions {
            collect_referenced_in_action_rhs(&la.action, &mut acc);
        }
        for w in &init.with {
            collect_referenced_in_predicate(&w.predicate, &mut acc);
        }
        for w in &init.witnesses {
            collect_referenced_in_predicate(&w.predicate, &mut acc);
        }
    }
    for e in &m.events {
        let params: Vec<&str> = e.parameters.iter().map(|p| p.name.as_str()).collect();
        for g in &e.guards {
            collect_referenced_in_predicate_with_locals(&g.predicate, &params, &mut acc);
        }
        for w in &e.with {
            collect_referenced_in_predicate_with_locals(&w.predicate, &params, &mut acc);
        }
        for w in &e.witnesses {
            collect_referenced_in_predicate_with_locals(&w.predicate, &params, &mut acc);
        }
        for la in &e.actions {
            collect_referenced_in_action_rhs_with_locals(&la.action, &params, &mut acc);
            // `f(x) := e` writes to f but also reads its old value.
            if let Action::FunctionOverride { function, .. } = &la.action {
                acc.insert(function.clone());
            }
        }
    }
    acc
}

fn referenced_in_context(c: &Context) -> BTreeSet<String> {
    let mut acc = BTreeSet::new();
    for ax in &c.axioms {
        collect_referenced_in_predicate(&ax.predicate, &mut acc);
    }
    acc
}

fn assigned_in_machine(m: &Machine) -> BTreeSet<String> {
    let mut acc = BTreeSet::new();
    let labeled_actions = m
        .initialisation
        .iter()
        .flat_map(|init| &init.actions)
        .chain(m.events.iter().flat_map(|e| &e.actions));
    for la in labeled_actions {
        for v in lhs_variables(&la.action) {
            acc.insert(v.to_string());
        }
    }
    acc
}

// ---------- cross-chain index ----------------------------------------------
//
// A naive per-component lint produces false positives for any identifier
// declared in one component and used only in a transitive descendant: a
// constant `k` in context A used solely by `B extends A`, or a variable `v`
// in machine M1 read only by `M2 refines M1`. `ProjectIndex` builds, once
// per `lint::run` call, the inverted indexes needed to union references and
// assignments across the refinement/extension chain.

struct ProjectIndex<'a> {
    /// Per-context, the references appearing in its own axioms.
    ctx_refs: BTreeMap<&'a str, BTreeSet<String>>,
    /// Per-machine, references appearing in its invariants/variant/events.
    mach_refs: BTreeMap<&'a str, BTreeSet<String>>,
    /// Per-machine, the set of variable names assigned by INIT or events.
    mach_assigned: BTreeMap<&'a str, BTreeSet<String>>,
    /// `ctx → {ctx names that EXTEND it transitively, excluding self}`.
    ctx_extends_descendants: BTreeMap<&'a str, BTreeSet<&'a str>>,
    /// `machine → {machine names that REFINE it transitively, excluding self}`.
    mach_refines_descendants: BTreeMap<&'a str, BTreeSet<&'a str>>,
    /// `ctx → {machine names that can syntactically reference this ctx's
    ///         declarations: machines that SEE it directly, machines that
    ///         SEE any of its extends-descendants, and the refines-descendants
    ///         of any such machine}`.
    ctx_consumer_machines: BTreeMap<&'a str, BTreeSet<&'a str>>,
}

impl<'a> ProjectIndex<'a> {
    fn build(project: &'a Project) -> Self {
        let mut ctx_parents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        let mut mach_parent: BTreeMap<&str, &str> = BTreeMap::new();
        let mut mach_sees: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        let mut ctx_refs: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
        let mut mach_refs: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
        let mut mach_assigned: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();

        for pc in &project.components {
            match &pc.component {
                Component::Context(c) => {
                    ctx_parents.insert(
                        c.name.as_str(),
                        c.extends.iter().map(String::as_str).collect(),
                    );
                    ctx_refs.insert(c.name.as_str(), referenced_in_context(c));
                }
                Component::Machine(m) => {
                    if let Some(p) = &m.refines {
                        mach_parent.insert(m.name.as_str(), p.as_str());
                    }
                    mach_sees.insert(m.name.as_str(), m.sees.iter().map(String::as_str).collect());
                    mach_refs.insert(m.name.as_str(), referenced_in_machine(m));
                    mach_assigned.insert(m.name.as_str(), assigned_in_machine(m));
                }
            }
        }

        // Invert parent maps to child maps.
        let mut ctx_children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (&child, parents) in &ctx_parents {
            for &parent in parents {
                ctx_children.entry(parent).or_default().push(child);
            }
        }
        let mut mach_children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (&child, &parent) in &mach_parent {
            mach_children.entry(parent).or_default().push(child);
        }

        // `children` maps PARENT → CHILDREN, so its keys are the names that
        // have at least one child — exactly the roots we need to compute
        // descendant closures for. Leaf nodes are absent from the result;
        // `effective_*` callers handle the missing-key case as "no
        // descendants", which is correct.
        let ctx_extends_descendants =
            transitive_descendants(&ctx_children, ctx_children.keys().copied());
        let mach_refines_descendants =
            transitive_descendants(&mach_children, mach_children.keys().copied());

        // ctx_consumer_machines: for each context, all machines that can
        // syntactically refer to its declarations.
        let mut ctx_consumer_machines: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for (&mach, seen_ctxs) in &mach_sees {
            for &ctx in seen_ctxs {
                // mach sees ctx, so it also sees every extends-ancestor of ctx.
                let mut ctx_and_ancestors: BTreeSet<&str> = BTreeSet::new();
                ctx_and_ancestors.insert(ctx);
                collect_ancestors_via(ctx, &ctx_parents, &mut ctx_and_ancestors);
                for &c in &ctx_and_ancestors {
                    let entry = ctx_consumer_machines.entry(c).or_default();
                    entry.insert(mach);
                    if let Some(descs) = mach_refines_descendants.get(mach) {
                        entry.extend(descs.iter().copied());
                    }
                }
            }
        }

        Self {
            ctx_refs,
            mach_refs,
            mach_assigned,
            ctx_extends_descendants,
            mach_refines_descendants,
            ctx_consumer_machines,
        }
    }

    fn effective_refs_for_machine(&self, name: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        union_self_and_descendants(
            name,
            &self.mach_refs,
            self.mach_refines_descendants.get(name),
            &mut out,
        );
        out
    }

    fn effective_assigned_for_machine(&self, name: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        union_self_and_descendants(
            name,
            &self.mach_assigned,
            self.mach_refines_descendants.get(name),
            &mut out,
        );
        out
    }

    fn effective_refs_for_context(&self, name: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        union_self_and_descendants(
            name,
            &self.ctx_refs,
            self.ctx_extends_descendants.get(name),
            &mut out,
        );
        if let Some(machs) = self.ctx_consumer_machines.get(name) {
            for &m in machs {
                if let Some(r) = self.mach_refs.get(m) {
                    out.extend(r.iter().cloned());
                }
            }
        }
        out
    }
}

/// Insert `entries[name]` plus `entries[d]` for every `d ∈ descendants`
/// into `out`. Skips missing keys silently — leaf components and
/// components without contributions are common.
fn union_self_and_descendants(
    name: &str,
    entries: &BTreeMap<&str, BTreeSet<String>>,
    descendants: Option<&BTreeSet<&str>>,
    out: &mut BTreeSet<String>,
) {
    if let Some(own) = entries.get(name) {
        out.extend(own.iter().cloned());
    }
    if let Some(descs) = descendants {
        for &d in descs {
            if let Some(set) = entries.get(d) {
                out.extend(set.iter().cloned());
            }
        }
    }
}

/// For each key in `roots`, compute the transitive closure of `children`
/// excluding the root itself.
fn transitive_descendants<'a, I>(
    children: &BTreeMap<&'a str, Vec<&'a str>>,
    roots: I,
) -> BTreeMap<&'a str, BTreeSet<&'a str>>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut out: BTreeMap<&'a str, BTreeSet<&'a str>> = BTreeMap::new();
    for root in roots {
        let mut descs = BTreeSet::new();
        let mut stack: Vec<&'a str> = children.get(root).cloned().unwrap_or_default();
        while let Some(node) = stack.pop() {
            if !descs.insert(node) {
                continue;
            }
            if let Some(cs) = children.get(node) {
                stack.extend(cs.iter().copied());
            }
        }
        out.insert(root, descs);
    }
    out
}

fn collect_ancestors_via<'a>(
    ctx: &'a str,
    parents: &BTreeMap<&'a str, Vec<&'a str>>,
    acc: &mut BTreeSet<&'a str>,
) {
    let mut stack: Vec<&'a str> = parents.get(ctx).cloned().unwrap_or_default();
    while let Some(node) = stack.pop() {
        if !acc.insert(node) {
            continue;
        }
        if let Some(ps) = parents.get(node) {
            stack.extend(ps.iter().copied());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rossi::{
        Action, Component, Context, Event, Expression, InitialisationEvent, LabeledAction,
        LabeledPredicate, Machine, NamedElement, Predicate,
    };

    use crate::project::ProjectComponent;
    use crate::rodin_ids::RodinIds;

    fn pc(filename: &str, component: Component) -> ProjectComponent {
        ProjectComponent {
            filename: filename.into(),
            component,
            rodin_ids: RodinIds::default(),
        }
    }

    fn proj(components: Vec<ProjectComponent>) -> Project {
        Project {
            name: "test".into(),
            components,
        }
    }

    fn lp(label: &str, predicate: Predicate) -> LabeledPredicate {
        LabeledPredicate {
            label: Some(label.into()),
            is_theorem: false,
            predicate,
            span: None,
            comment: None,
        }
    }

    fn la(action: Action) -> LabeledAction {
        LabeledAction {
            label: None,
            action,
            span: None,
            comment: None,
        }
    }

    fn ident(n: &str) -> Expression {
        Expression::Identifier(n.into())
    }

    fn eq_pred(lhs: Expression, rhs: Expression) -> Predicate {
        use rossi::ast::predicate::ComparisonOp;
        Predicate::Comparison {
            op: ComparisonOp::Equal,
            left: lhs,
            right: rhs,
        }
    }

    fn nv(n: &str) -> NamedElement {
        NamedElement::new(n.into())
    }

    #[test]
    fn dead_variable_is_flagged() {
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x"), nv("y")];
        m.invariants = vec![lp("inv1", eq_pred(ident("x"), Expression::Integer(0)))];
        m.initialisation = Some(InitialisationEvent {
            actions: vec![
                la(Action::assignment("x", Expression::Integer(0))),
                la(Action::assignment("y", Expression::Integer(0))),
            ],
            comment: None,
            extended: false,
            with: Vec::new(),
            witnesses: Vec::new(),
        });

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        let dead: Vec<_> = diags
            .iter()
            .filter(|d| d.rule_id == Some(RuleId::DeadVariable))
            .collect();
        assert_eq!(
            dead.len(),
            1,
            "expected exactly one dead-var diag: {diags:#?}"
        );
        assert!(dead[0].message.contains('`'));
        assert!(dead[0].message.contains('y'));
    }

    #[test]
    fn unmodified_variable_is_flagged() {
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        m.invariants = vec![lp("inv1", eq_pred(ident("x"), Expression::Integer(0)))];
        // No INITIALISATION, no events → x is referenced but never assigned.
        // Note: lint_incomplete_init will also fire here; we only assert EB012.

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        let unmod: Vec<_> = diags
            .iter()
            .filter(|d| d.rule_id == Some(RuleId::UnmodifiedVariable))
            .collect();
        assert_eq!(unmod.len(), 1, "expected one EB012: {diags:#?}");
    }

    #[test]
    fn dead_constant_is_flagged() {
        let mut c = Context::new("C".into());
        c.constants = vec![nv("k1"), nv("k2")];
        c.axioms = vec![lp("ax1", eq_pred(ident("k1"), Expression::Integer(0)))];

        let diags = run(&proj(vec![pc("C.buc", Component::Context(c))]));
        let dead: Vec<_> = diags
            .iter()
            .filter(|d| d.rule_id == Some(RuleId::DeadConstant))
            .collect();
        assert_eq!(dead.len(), 1);
        assert!(dead[0].message.contains("k2"));
    }

    #[test]
    fn shadowed_names_are_flagged() {
        // `POW` (exact ASCII operator spelling) and `Nat` (case variant of
        // the case-insensitive ℕ token) warn; `Dom`, `pow`, `OR` are ordinary
        // identifiers and stay silent. The §2.2 reserved words never reach
        // the lint — the parser rejects their declarations outright.
        let mut c = Context::new("C".into());
        c.constants = vec![nv("POW"), nv("Dom"), nv("pow"), nv("OR"), nv("price")];
        c.sets = vec![rossi::SetDeclaration::Deferred {
            name: "Nat".into(),
            comment: None,
        }];
        c.axioms = vec![lp("ax1", eq_pred(ident("price"), Expression::Integer(0)))];

        let diags = run(&proj(vec![pc("C.buc", Component::Context(c))]));
        let shadowed: Vec<_> = diags
            .iter()
            .filter(|d| d.rule_id == Some(RuleId::ShadowedName))
            .collect();
        assert_eq!(shadowed.len(), 2, "{shadowed:?}");
        assert!(shadowed.iter().any(|d| d.origin == "C.POW"));
        assert!(shadowed.iter().any(|d| d.origin == "C.Nat"));
    }

    #[test]
    fn shadowed_machine_names_are_flagged() {
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("or"), nv("count")];
        let mut e = Event::new("evt".into());
        e.parameters = vec![nv("circ"), nv("p")];
        m.events = vec![e];

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        let shadowed: Vec<_> = diags
            .iter()
            .filter(|d| d.rule_id == Some(RuleId::ShadowedName))
            .collect();
        assert_eq!(shadowed.len(), 2, "{shadowed:?}");
        assert!(shadowed.iter().any(|d| d.origin == "M.or"));
        assert!(shadowed.iter().any(|d| d.origin == "M.evt.circ"));
    }

    #[test]
    fn quantifier_binder_does_not_count_as_use() {
        // ∀x · TRUE — the `x` binder shouldn't count as a use of the machine
        // variable named `x`, so the variable should still be flagged dead.
        use rossi::TypedIdentifier;
        use rossi::ast::predicate::Quantifier;
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        m.invariants = vec![lp(
            "inv1",
            Predicate::Quantified {
                quantifier: Quantifier::ForAll,
                identifiers: vec![TypedIdentifier {
                    name: "x".into(),
                    type_expr: None,
                }],
                predicate: Box::new(Predicate::True),
            },
        )];
        m.initialisation = Some(InitialisationEvent {
            actions: vec![la(Action::assignment("x", Expression::Integer(0)))],
            comment: None,
            extended: false,
            with: Vec::new(),
            witnesses: Vec::new(),
        });

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        let dead: Vec<_> = diags
            .iter()
            .filter(|d| d.rule_id == Some(RuleId::DeadVariable))
            .collect();
        assert_eq!(
            dead.len(),
            1,
            "binder should not satisfy reference: {diags:#?}"
        );
    }

    #[test]
    fn incomplete_init_is_flagged() {
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x"), nv("y")];
        m.initialisation = Some(InitialisationEvent {
            actions: vec![la(Action::assignment("x", Expression::Integer(0)))],
            comment: None,
            extended: false,
            with: Vec::new(),
            witnesses: Vec::new(),
        });

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        let missing: Vec<_> = diags
            .iter()
            .filter(|d| d.rule_id == Some(RuleId::IncompleteInitialisation))
            .collect();
        assert_eq!(missing.len(), 1);
        assert!(missing[0].message.contains('y'));
    }

    #[test]
    fn bsu_primed_reference_counts_as_use() {
        // INITIALISATION uses `x :| x' = 0` — the predicate references
        // `x'`, which after prime-stripping is a use of `x`. So `x` is
        // both assigned (LHS) and referenced (RHS): no EB011, no EB012.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        m.initialisation = Some(InitialisationEvent {
            actions: vec![la(Action::BecomesSuchThat {
                variables: vec!["x".into()],
                predicate: eq_pred(ident("x'"), Expression::Integer(0)),
            })],
            comment: None,
            extended: false,
            with: Vec::new(),
            witnesses: Vec::new(),
        });

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        assert!(
            !diags
                .iter()
                .any(|d| d.rule_id == Some(RuleId::DeadVariable)),
            "x' should count as a use of x: {diags:#?}"
        );
        assert!(
            !diags
                .iter()
                .any(|d| d.rule_id == Some(RuleId::UnmodifiedVariable)),
            "x is assigned via BSU LHS: {diags:#?}"
        );
    }

    #[test]
    fn duplicate_component_is_flagged() {
        let project = proj(vec![
            pc("a/M.bum", Component::Machine(Machine::new("M".into()))),
            pc("b/M.bum", Component::Machine(Machine::new("M".into()))),
        ]);
        let diags = run(&project);
        let dups: Vec<_> = diags
            .iter()
            .filter(|d| d.rule_id == Some(RuleId::DuplicateComponent))
            .collect();
        assert_eq!(dups.len(), 1);
        assert!(dups[0].message.contains("a/M.bum"));
        assert!(dups[0].message.contains("b/M.bum"));
    }

    #[test]
    fn cross_extends_keeps_constant_alive() {
        // A declares k; B extends A and uses k in an axiom. A alone has no
        // reference to k, but EB013 should NOT fire because B sees k via
        // EXTENDS.
        let mut a = Context::new("A".into());
        a.constants = vec![nv("k")];
        let mut b = Context::new("B".into());
        b.extends = vec!["A".into()];
        b.axioms = vec![lp("ax1", eq_pred(ident("k"), Expression::Integer(0)))];

        let project = proj(vec![
            pc("A.buc", Component::Context(a)),
            pc("B.buc", Component::Context(b)),
        ]);
        let diags = run(&project);
        assert!(
            !diags
                .iter()
                .any(|d| d.rule_id == Some(RuleId::DeadConstant) && d.message.contains("`k`")),
            "k is alive via extends: {diags:#?}"
        );
    }

    #[test]
    fn machine_sees_context_keeps_constant_alive() {
        // C declares k; M sees C and references k. k must not be flagged dead.
        let mut c = Context::new("C".into());
        c.constants = vec![nv("k")];
        let mut m = Machine::new("M".into());
        m.sees = vec!["C".into()];
        m.invariants = vec![lp("inv1", eq_pred(ident("k"), Expression::Integer(0)))];

        let project = proj(vec![
            pc("C.buc", Component::Context(c)),
            pc("M.bum", Component::Machine(m)),
        ]);
        let diags = run(&project);
        assert!(
            !diags
                .iter()
                .any(|d| d.rule_id == Some(RuleId::DeadConstant) && d.message.contains("`k`")),
            "k is alive via SEES: {diags:#?}"
        );
    }

    #[test]
    fn cross_refines_keeps_variable_alive() {
        // M1 declares v but doesn't reference it; M2 refines M1 and uses v
        // in an invariant. M1 should not flag v as dead.
        let mut m1 = Machine::new("M1".into());
        m1.variables = vec![nv("v")];
        m1.initialisation = Some(InitialisationEvent {
            actions: vec![la(Action::assignment("v", Expression::Integer(0)))],
            comment: None,
            extended: false,
            with: Vec::new(),
            witnesses: Vec::new(),
        });
        let mut m2 = Machine::new("M2".into());
        m2.refines = Some("M1".into());
        m2.invariants = vec![lp("inv1", eq_pred(ident("v"), Expression::Integer(0)))];

        let project = proj(vec![
            pc("M1.bum", Component::Machine(m1)),
            pc("M2.bum", Component::Machine(m2)),
        ]);
        let diags = run(&project);
        assert!(
            !diags
                .iter()
                .any(|d| d.rule_id == Some(RuleId::DeadVariable) && d.origin.starts_with("M1.")),
            "v in M1 is alive via REFINES: {diags:#?}"
        );
    }

    #[test]
    fn cross_refines_keeps_variable_assigned() {
        // M1 declares v but never assigns it; M2 refines M1 and assigns v
        // in an event. M1 references v (so not dead) but is its assignment
        // covered? It should be — through M2.
        let mut m1 = Machine::new("M1".into());
        m1.variables = vec![nv("v")];
        m1.invariants = vec![lp("inv1", eq_pred(ident("v"), Expression::Integer(0)))];
        // Note: deliberately no INIT and no events that assign v.
        let mut m2 = Machine::new("M2".into());
        m2.refines = Some("M1".into());
        m2.events = vec![Event {
            name: "step".into(),
            status: None,
            refines: None,
            parameters: Vec::new(),
            guards: Vec::new(),
            with: Vec::new(),
            witnesses: Vec::new(),
            actions: vec![la(Action::assignment("v", Expression::Integer(1)))],
            span: None,
            name_span: None,
            comment: None,
            extended: false,
        }];

        let project = proj(vec![
            pc("M1.bum", Component::Machine(m1)),
            pc("M2.bum", Component::Machine(m2)),
        ]);
        let diags = run(&project);
        assert!(
            !diags
                .iter()
                .any(|d| d.rule_id == Some(RuleId::UnmodifiedVariable)
                    && d.origin.starts_with("M1.")),
            "v is assigned in M2: {diags:#?}"
        );
    }

    #[test]
    fn becomes_in_lhs_marks_variable_as_assigned() {
        // `x :∈ S` — x is assigned via BecomesIn, so EB012 must not fire.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        m.invariants = vec![lp("inv1", eq_pred(ident("x"), Expression::Integer(0)))];
        m.events = vec![Event {
            name: "evt".into(),
            status: None,
            refines: None,
            parameters: Vec::new(),
            guards: Vec::new(),
            with: Vec::new(),
            witnesses: Vec::new(),
            actions: vec![la(Action::BecomesIn {
                variables: vec!["x".into()],
                set: Expression::Naturals,
            })],
            span: None,
            name_span: None,
            comment: None,
            extended: false,
        }];

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        assert!(
            !diags
                .iter()
                .any(|d| d.rule_id == Some(RuleId::UnmodifiedVariable)),
            "x is assigned via BecomesIn: {diags:#?}"
        );
    }

    #[test]
    fn function_override_lhs_marks_variable_as_assigned() {
        // `f(x) := 0` — f is assigned via FunctionOverride.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("f")];
        m.invariants = vec![lp("inv1", eq_pred(ident("f"), Expression::Integer(0)))];
        m.events = vec![Event {
            name: "evt".into(),
            status: None,
            refines: None,
            parameters: Vec::new(),
            guards: Vec::new(),
            with: Vec::new(),
            witnesses: Vec::new(),
            actions: vec![la(Action::FunctionOverride {
                function: "f".into(),
                arguments: vec![Expression::Integer(1)],
                expression: Expression::Integer(0),
            })],
            span: None,
            name_span: None,
            comment: None,
            extended: false,
        }];

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        assert!(
            !diags
                .iter()
                .any(|d| d.rule_id == Some(RuleId::UnmodifiedVariable)),
            "f is assigned via FunctionOverride: {diags:#?}"
        );
    }

    #[test]
    fn lambda_binder_does_not_count_as_use() {
        // The only mention of machine variable `x` is as a lambda parameter.
        // The lambda introduces a fresh binder, so the machine variable
        // should still be flagged dead.
        use rossi::IdentPattern;
        use rossi::TypedIdentifier;

        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        let lambda = Expression::Lambda {
            pattern: IdentPattern::Identifier(TypedIdentifier {
                name: "x".into(),
                type_expr: None,
            }),
            predicate: Box::new(Predicate::True),
            expression: Box::new(ident("x")),
        };
        m.invariants = vec![lp("inv1", eq_pred(lambda, Expression::Integer(0)))];
        m.initialisation = Some(InitialisationEvent {
            actions: vec![la(Action::assignment("x", Expression::Integer(0)))],
            comment: None,
            extended: false,
            with: Vec::new(),
            witnesses: Vec::new(),
        });

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        assert!(
            diags
                .iter()
                .any(|d| d.rule_id == Some(RuleId::DeadVariable) && d.message.contains("`x`")),
            "lambda binder must not satisfy reference to machine variable: {diags:#?}"
        );
    }

    #[test]
    fn no_initialisation_at_all_flags_every_variable() {
        // No INITIALISATION event present: each declared variable should
        // produce one EB014 diagnostic.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("a"), nv("b"), nv("c")];

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        let missing: Vec<_> = diags
            .iter()
            .filter(|d| d.rule_id == Some(RuleId::IncompleteInitialisation))
            .collect();
        assert_eq!(
            missing.len(),
            3,
            "expected one EB014 per variable: {diags:#?}"
        );
    }

    #[test]
    fn event_parameter_shadows_variable() {
        // Event has parameter named `x`; guard `x = 0` uses the parameter,
        // not the machine variable. The variable should still be flagged
        // unmodified (referenced=false, assigned=false → dead, not unmod).
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        m.events = vec![Event {
            name: "evt".into(),
            status: None,
            refines: None,
            parameters: vec![nv("x")],
            guards: vec![lp("g1", eq_pred(ident("x"), Expression::Integer(0)))],
            with: Vec::new(),
            witnesses: Vec::new(),
            actions: Vec::new(),
            span: None,
            name_span: None,
            comment: None,
            extended: false,
        }];

        let diags = run(&proj(vec![pc("M.bum", Component::Machine(m))]));
        // x is dead — the only mention is shadowed by the parameter.
        assert!(
            diags
                .iter()
                .any(|d| d.rule_id == Some(RuleId::DeadVariable) && d.message.contains('x')),
            "expected EB011 for shadowed variable: {diags:#?}"
        );
    }
}
