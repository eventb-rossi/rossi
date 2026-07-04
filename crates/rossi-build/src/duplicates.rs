//! Duplicate identifier / label detection (EB021 / EB022).
//!
//! Within the single scope where Event-B requires uniqueness, report
//! identifiers (EB021) and labels (EB022) that occur more than once.
//! Identifiers and labels are separate namespaces, so a variable `x` and an
//! invariant labelled `x` do not collide. Cross-component shadowing is out of
//! scope — that is EB023 / the type checker's scope rules.
//!
//! This is the shared core behind every consumer: the static checker
//! (`crate::sc`), `rossi validate`'s loose-text path, and the LSP's
//! as-you-type diagnostics all call into it, so their reports can never
//! drift apart. Each namespace comes back as a [`NamespaceDuplicates`] —
//! the diagnostics plus the duplicated names themselves, which the SC uses
//! to filter the conflicting elements out of its output.

use std::collections::{BTreeMap, BTreeSet};

use rossi::ast::Span;
use rossi::{Component, Context, LabeledAction, LabeledPredicate, Machine};

use crate::{Diagnostic, RuleId};

/// The duplicated names of one uniqueness scope, plus the diagnostics
/// reporting them.
pub struct NamespaceDuplicates {
    /// Names that occur more than once (blank names are never counted).
    pub names: BTreeSet<String>,
    /// One `Error` diagnostic per duplicated name, sorted by name.
    pub diagnostics: Vec<Diagnostic>,
}

impl NamespaceDuplicates {
    /// Whether `name` is duplicated in this namespace.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    /// Whether the namespace is duplicate-free.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// The machine-file namespaces: variables, invariant labels, event labels.
/// Per-event namespaces are separate — see [`event_duplicates`].
pub struct MachineFileDuplicates {
    /// EB021 — variable identifiers.
    pub variables: NamespaceDuplicates,
    /// EB022 — invariant labels.
    pub invariant_labels: NamespaceDuplicates,
    /// EB022 — event labels (INITIALISATION included).
    pub event_labels: NamespaceDuplicates,
}

/// The per-event namespaces: parameters, the shared guard+action label
/// space, and the shared witness label space.
pub struct EventDuplicates {
    /// EB021 — parameter identifiers.
    pub parameters: NamespaceDuplicates,
    /// EB022 — guard and action labels (one shared namespace in Event-B).
    pub guard_action_labels: NamespaceDuplicates,
    /// EB022 — witness labels (`with` + `witnesses` share one namespace).
    pub witness_labels: NamespaceDuplicates,
}

impl EventDuplicates {
    /// Whether every per-event namespace is duplicate-free.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.parameters.is_empty()
            && self.guard_action_labels.is_empty()
            && self.witness_labels.is_empty()
    }
}

/// The context namespaces: one shared identifier namespace (carrier sets,
/// their enumerated elements, constants) and axiom labels.
pub struct ContextDuplicates {
    /// EB021 — carrier set / set element / constant identifiers.
    pub identifiers: NamespaceDuplicates,
    /// EB022 — axiom labels.
    pub axiom_labels: NamespaceDuplicates,
}

/// `(label, span)` for each labelled predicate (invariant / guard / witness /
/// axiom); unlabelled clauses are skipped. Feeds [`namespace_duplicates`].
pub(crate) fn pred_labels(
    preds: &[LabeledPredicate],
) -> impl Iterator<Item = (&str, Option<Span>)> {
    preds
        .iter()
        .filter_map(|p| p.label.as_deref().map(|l| (l, p.span)))
}

/// `(label, span)` for each labelled action; unlabelled actions are skipped.
pub(crate) fn action_labels(
    actions: &[LabeledAction],
) -> impl Iterator<Item = (&str, Option<Span>)> {
    actions
        .iter()
        .filter_map(|a| a.label.as_deref().map(|l| (l, a.span)))
}

/// Byte span of a set's *name*. The declaration span starts at the name but
/// runs through any trailing comment to the next declaration, which would
/// over-underline a name-level diagnostic — clip it to the name's length.
pub(crate) fn set_name_span(set: &rossi::SetDeclaration) -> Option<Span> {
    set.span().map(|s| Span {
        start: s.start,
        end: s.start + set.name().len(),
    })
}

/// Collect one namespace: one `Error` diagnostic per name that occurs more
/// than once in `names` (blank and whitespace-only names are skipped), plus
/// the set of duplicated names. Output is sorted by name for determinism. The
/// verb in the message follows the rule: identifiers are "declared", labels
/// "used".
fn namespace_duplicates<'a>(
    names: impl IntoIterator<Item = (&'a str, Option<Span>)>,
    rule: RuleId,
    kind: &str,
    scope: &str,
    origin_prefix: &str,
) -> NamespaceDuplicates {
    let verb = if rule == RuleId::DuplicateIdentifier {
        "declared"
    } else {
        "used"
    };
    // Count occurrences per name, remembering the first source span seen (the
    // declaration the reader should jump to). A later occurrence upgrades a
    // still-unknown span so a name spanned only on its second mention is still
    // located.
    let mut counts: BTreeMap<&str, (usize, Option<Span>)> = BTreeMap::new();
    for (n, span) in names {
        if n.trim().is_empty() {
            continue;
        }
        let entry = counts.entry(n).or_insert((0, None));
        entry.0 += 1;
        if entry.1.is_none() {
            entry.1 = span;
        }
    }
    let mut duplicated = BTreeSet::new();
    let mut diagnostics = Vec::new();
    for (name, (count, span)) in counts {
        if count <= 1 {
            continue;
        }
        duplicated.insert(name.to_string());
        diagnostics.push(Diagnostic {
            severity: rule.default_severity(),
            origin: format!("{origin_prefix}.{name}"),
            message: format!("duplicate {kind} `{name}` in {scope} ({verb} {count} times)"),
            rule_id: Some(rule),
            span,
        });
    }
    NamespaceDuplicates {
        names: duplicated,
        diagnostics,
    }
}

/// Check the machine-file namespaces of `m` (variables; invariant labels;
/// event labels, with INITIALISATION chained in — rossi stores it apart from
/// `events`, but Event-B treats it as an event sharing the label namespace).
#[must_use]
pub fn machine_file_duplicates(m: &Machine) -> MachineFileDuplicates {
    let scope = format!("machine `{}`", m.name);
    MachineFileDuplicates {
        variables: namespace_duplicates(
            m.variables.iter().map(|v| (v.name.as_str(), v.span)),
            RuleId::DuplicateIdentifier,
            "variable identifier",
            &scope,
            &m.name,
        ),
        invariant_labels: namespace_duplicates(
            pred_labels(&m.invariants),
            RuleId::DuplicateLabel,
            "invariant label",
            &scope,
            &m.name,
        ),
        event_labels: namespace_duplicates(
            m.events.iter().map(|e| (e.name.as_str(), e.span)).chain(
                m.initialisation
                    .as_ref()
                    .map(|i| (crate::sc::initialisation_label(), i.span)),
            ),
            RuleId::DuplicateLabel,
            "event label",
            &scope,
            &m.name,
        ),
    }
}

/// Check the three per-event namespaces (parameters; the shared guard+action
/// label space; the shared witness label space) for duplicates.
#[must_use]
pub fn event_duplicates<'a>(
    machine: &str,
    event: &str,
    parameters: impl IntoIterator<Item = (&'a str, Option<Span>)>,
    guard_action_labels: impl IntoIterator<Item = (&'a str, Option<Span>)>,
    witness_labels: impl IntoIterator<Item = (&'a str, Option<Span>)>,
) -> EventDuplicates {
    let scope = format!("event `{event}` of machine `{machine}`");
    let origin = format!("{machine}.{event}");
    EventDuplicates {
        parameters: namespace_duplicates(
            parameters,
            RuleId::DuplicateIdentifier,
            "parameter identifier",
            &scope,
            &origin,
        ),
        guard_action_labels: namespace_duplicates(
            guard_action_labels,
            RuleId::DuplicateLabel,
            "guard or action label",
            &scope,
            &origin,
        ),
        witness_labels: namespace_duplicates(
            witness_labels,
            RuleId::DuplicateLabel,
            "witness label",
            &scope,
            &origin,
        ),
    }
}

/// Check the context namespaces of `c`. Carrier sets, their enumerated
/// elements, and constants share one identifier namespace, so a set and a
/// constant with the same name collide. (In Event-B, enumerated set elements
/// are constants.) Enumerated elements have no per-element span, so they
/// anchor on the set declaration.
#[must_use]
pub fn context_duplicates(c: &Context) -> ContextDuplicates {
    let scope = format!("context `{}`", c.name);
    let mut ids: Vec<(&str, Option<Span>)> = Vec::new();
    for set in &c.sets {
        ids.push((set.name(), set_name_span(set)));
        ids.extend(set.elements().iter().map(|e| (e.as_str(), set.span())));
    }
    ids.extend(c.constants.iter().map(|k| (k.name.as_str(), k.span)));
    ContextDuplicates {
        identifiers: namespace_duplicates(
            ids,
            RuleId::DuplicateIdentifier,
            "carrier set or constant identifier",
            &scope,
            &c.name,
        ),
        axiom_labels: namespace_duplicates(
            pred_labels(&c.axioms),
            RuleId::DuplicateLabel,
            "axiom label",
            &scope,
            &c.name,
        ),
    }
}

/// All duplicate-name diagnostics for one component, in a stable order:
/// for a machine, the file namespaces (variables, invariant labels, event
/// labels), then each event's namespaces in declaration order, then
/// INITIALISATION's; for a context, identifiers then axiom labels.
#[must_use]
pub fn component_duplicate_diagnostics(component: &Component) -> Vec<Diagnostic> {
    match component {
        Component::Machine(m) => {
            let file = machine_file_duplicates(m);
            let mut diags = [
                file.variables.diagnostics,
                file.invariant_labels.diagnostics,
                file.event_labels.diagnostics,
            ]
            .concat();
            for e in &m.events {
                diags.extend(event_diags(event_duplicates(
                    &m.name,
                    &e.name,
                    e.parameters.iter().map(|p| (p.name.as_str(), p.span)),
                    // Event-B shares one label namespace across guards and
                    // actions.
                    pred_labels(&e.guards).chain(action_labels(&e.actions)),
                    // rossi splits witnesses into `with` (abstract vars) +
                    // `witnesses` (abstract params); Event-B treats them as
                    // one witness namespace.
                    pred_labels(&e.with).chain(pred_labels(&e.witnesses)),
                )));
            }
            // INITIALISATION as an event: no parameters, no guards.
            if let Some(init) = &m.initialisation {
                diags.extend(event_diags(event_duplicates(
                    &m.name,
                    crate::sc::initialisation_label(),
                    std::iter::empty(),
                    action_labels(&init.actions),
                    pred_labels(&init.with).chain(pred_labels(&init.witnesses)),
                )));
            }
            diags
        }
        Component::Context(c) => {
            let dups = context_duplicates(c);
            [dups.identifiers.diagnostics, dups.axiom_labels.diagnostics].concat()
        }
    }
}

/// Flatten one event's namespaces into the reporting order used by
/// [`component_duplicate_diagnostics`].
fn event_diags(dups: EventDuplicates) -> Vec<Diagnostic> {
    [
        dups.parameters.diagnostics,
        dups.guard_action_labels.diagnostics,
        dups.witness_labels.diagnostics,
    ]
    .concat()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Severity;
    use rossi::ast::predicate::ComparisonOp;
    use rossi::{
        Action, Component, Context, Event, Expression, ExpressionKind, InitialisationEvent,
        Machine, NamedElement, Predicate, PredicateKind,
    };

    fn lp(label: &str, predicate: Predicate) -> LabeledPredicate {
        LabeledPredicate {
            label: Some(label.into()),
            is_theorem: false,
            predicate,
            span: None,
            comment: None,
        }
    }

    fn ident(n: &str) -> Expression {
        ExpressionKind::Identifier(n.into()).into()
    }

    fn eq_pred(lhs: Expression, rhs: Expression) -> Predicate {
        PredicateKind::Comparison {
            op: ComparisonOp::Equal,
            left: lhs,
            right: rhs,
        }
        .into()
    }

    fn nv(n: &str) -> NamedElement {
        NamedElement::new(n.into())
    }

    fn dups_of(diags: &[Diagnostic], rule: RuleId) -> Vec<&Diagnostic> {
        diags.iter().filter(|d| d.rule_id == Some(rule)).collect()
    }

    fn labeled_action(label: &str) -> LabeledAction {
        LabeledAction {
            label: Some(label.into()),
            action: Action::assignment("x", ExpressionKind::Integer(0).into()),
            span: None,
            comment: None,
        }
    }

    #[test]
    fn duplicate_identifier_diag_carries_first_occurrence_span() {
        // EB021 anchors on the first declaration of the duplicated name.
        let first = Span { start: 4, end: 5 };
        let second = Span { start: 9, end: 10 };
        let mut m = Machine::new("M".into());
        m.variables = vec![
            rossi::NamedElement::with_span("x".into(), first),
            rossi::NamedElement::with_span("x".into(), second),
            nv("y"),
        ];
        let diags = component_duplicate_diagnostics(&Component::Machine(m));
        let ids = dups_of(&diags, RuleId::DuplicateIdentifier);
        assert_eq!(ids.len(), 1, "{ids:?}");
        assert_eq!(ids[0].span, Some(first));
    }

    #[test]
    fn duplicate_variable_identifier_is_flagged() {
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x"), nv("x"), nv("y")];
        let diags = component_duplicate_diagnostics(&Component::Machine(m));
        let ids = dups_of(&diags, RuleId::DuplicateIdentifier);
        assert_eq!(ids.len(), 1, "{diags:#?}");
        assert_eq!(ids[0].severity, Severity::Error);
        assert_eq!(ids[0].origin, "M.x");
        assert!(
            ids[0].message.contains("variable identifier `x`"),
            "{}",
            ids[0].message
        );
        assert!(ids[0].message.contains("(declared 2 times)"));
    }

    #[test]
    fn duplicate_invariant_label_is_flagged() {
        let mut m = Machine::new("M".into());
        m.invariants = vec![
            lp(
                "inv1",
                eq_pred(ident("x"), ExpressionKind::Integer(0).into()),
            ),
            lp(
                "inv1",
                eq_pred(ident("y"), ExpressionKind::Integer(0).into()),
            ),
        ];
        let diags = component_duplicate_diagnostics(&Component::Machine(m));
        let labels = dups_of(&diags, RuleId::DuplicateLabel);
        assert_eq!(labels.len(), 1, "{diags:#?}");
        assert_eq!(labels[0].origin, "M.inv1");
        assert!(labels[0].message.contains("invariant label `inv1`"));
        assert!(labels[0].message.contains("(used 2 times)"));
    }

    #[test]
    fn duplicate_event_label_is_flagged() {
        let mut m = Machine::new("M".into());
        m.events = vec![Event::new("evt".into()), Event::new("evt".into())];
        let diags = component_duplicate_diagnostics(&Component::Machine(m));
        let labels = dups_of(&diags, RuleId::DuplicateLabel);
        assert_eq!(labels.len(), 1, "{diags:#?}");
        assert_eq!(labels[0].origin, "M.evt");
        assert!(labels[0].message.contains("event label `evt`"));
    }

    #[test]
    fn guard_and_action_sharing_label_is_flagged() {
        // Event-B shares one label namespace across guards and actions, so a
        // guard `lbl` and an action `lbl` in the same event collide.
        let mut e = Event::new("evt".into());
        e.guards = vec![lp("lbl", PredicateKind::True.into())];
        e.actions = vec![labeled_action("lbl")];
        let mut m = Machine::new("M".into());
        m.events = vec![e];
        let diags = component_duplicate_diagnostics(&Component::Machine(m));
        let labels = dups_of(&diags, RuleId::DuplicateLabel);
        assert_eq!(labels.len(), 1, "{diags:#?}");
        assert_eq!(labels[0].origin, "M.evt.lbl");
        assert!(labels[0].message.contains("guard or action label `lbl`"));
        assert!(labels[0].message.contains("event `evt` of machine `M`"));
    }

    #[test]
    fn duplicate_parameter_identifier_is_flagged() {
        let mut e = Event::new("evt".into());
        e.parameters = vec![nv("p"), nv("p")];
        let mut m = Machine::new("M".into());
        m.events = vec![e];
        let diags = component_duplicate_diagnostics(&Component::Machine(m));
        let ids = dups_of(&diags, RuleId::DuplicateIdentifier);
        assert_eq!(ids.len(), 1, "{diags:#?}");
        assert_eq!(ids[0].origin, "M.evt.p");
        assert!(ids[0].message.contains("parameter identifier `p`"));
    }

    #[test]
    fn carrier_set_and_constant_sharing_name_is_flagged_once() {
        // Carrier sets and constants share one identifier namespace.
        let mut c = Context::new("C".into());
        c.sets = vec![rossi::SetDeclaration::Deferred {
            name: "S".into(),
            comment: None,
            span: None,
        }];
        c.constants = vec![nv("S")];
        let diags = component_duplicate_diagnostics(&Component::Context(c));
        let ids = dups_of(&diags, RuleId::DuplicateIdentifier);
        assert_eq!(ids.len(), 1, "{diags:#?}");
        assert_eq!(ids[0].origin, "C.S");
        assert!(
            ids[0]
                .message
                .contains("carrier set or constant identifier `S`")
        );
        assert!(ids[0].message.contains("context `C`"));
    }

    #[test]
    fn duplicate_axiom_label_is_flagged() {
        let mut c = Context::new("C".into());
        c.axioms = vec![
            lp(
                "axm1",
                eq_pred(ident("k"), ExpressionKind::Integer(0).into()),
            ),
            lp(
                "axm1",
                eq_pred(ident("k"), ExpressionKind::Integer(1).into()),
            ),
        ];
        let diags = component_duplicate_diagnostics(&Component::Context(c));
        let labels = dups_of(&diags, RuleId::DuplicateLabel);
        assert_eq!(labels.len(), 1, "{diags:#?}");
        assert_eq!(labels[0].origin, "C.axm1");
        assert!(labels[0].message.contains("axiom label `axm1`"));
    }

    #[test]
    fn duplicate_witness_label_across_with_and_witnesses_is_flagged() {
        // `with` (abstract var) and `witnesses` (abstract param) share one
        // witness-label namespace in Event-B; the same label in each collides.
        let mut e = Event::new("evt".into());
        e.with = vec![lp("w", PredicateKind::True.into())];
        e.witnesses = vec![lp("w", PredicateKind::True.into())];
        let mut m = Machine::new("M".into());
        m.events = vec![e];
        let diags = component_duplicate_diagnostics(&Component::Machine(m));
        let labels = dups_of(&diags, RuleId::DuplicateLabel);
        assert_eq!(labels.len(), 1, "{diags:#?}");
        assert_eq!(labels[0].origin, "M.evt.w");
        assert!(labels[0].message.contains("witness label `w`"));
    }

    #[test]
    fn initialisation_duplicate_action_label_is_flagged() {
        // INITIALISATION is treated as an event sharing the guard/action label
        // namespace, even though rossi stores it apart from `events`.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        m.initialisation = Some(InitialisationEvent {
            actions: vec![labeled_action("act1"), labeled_action("act1")],
            comment: None,
            extended: false,
            with: Vec::new(),
            witnesses: Vec::new(),
            span: None,
            name_span: None,
        });
        let diags = component_duplicate_diagnostics(&Component::Machine(m));
        let labels = dups_of(&diags, RuleId::DuplicateLabel);
        assert_eq!(labels.len(), 1, "{diags:#?}");
        assert_eq!(labels[0].origin, "M.INITIALISATION.act1");
        assert!(labels[0].message.contains("guard or action label `act1`"));
        assert!(
            labels[0]
                .message
                .contains("event `INITIALISATION` of machine `M`")
        );
    }

    #[test]
    fn identifier_and_label_in_separate_namespaces_do_not_conflict() {
        // A variable `x` and an invariant labelled `x` must NOT be reported:
        // identifiers and labels are distinct namespaces.
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x")];
        m.invariants = vec![lp(
            "x",
            eq_pred(ident("x"), ExpressionKind::Integer(0).into()),
        )];
        let diags = component_duplicate_diagnostics(&Component::Machine(m));
        assert!(
            dups_of(&diags, RuleId::DuplicateIdentifier).is_empty()
                && dups_of(&diags, RuleId::DuplicateLabel).is_empty(),
            "{diags:#?}"
        );
    }

    #[test]
    fn context_identifier_and_label_in_separate_namespaces_do_not_conflict() {
        // The symmetric context case: a constant `S` and an axiom labelled `S`
        // must NOT collide — identifiers and labels are distinct namespaces.
        let mut c = Context::new("C".into());
        c.constants = vec![nv("S")];
        c.axioms = vec![lp(
            "S",
            eq_pred(ident("S"), ExpressionKind::Integer(0).into()),
        )];
        let diags = component_duplicate_diagnostics(&Component::Context(c));
        assert!(
            dups_of(&diags, RuleId::DuplicateIdentifier).is_empty()
                && dups_of(&diags, RuleId::DuplicateLabel).is_empty(),
            "{diags:#?}"
        );
    }

    #[test]
    fn unlabeled_guards_are_not_duplicates() {
        // Two guards with no explicit label must not be reported — blank names
        // are skipped.
        let blank = || LabeledPredicate {
            label: None,
            is_theorem: false,
            predicate: PredicateKind::True.into(),
            span: None,
            comment: None,
        };
        let mut e = Event::new("evt".into());
        e.guards = vec![blank(), blank()];
        let mut m = Machine::new("M".into());
        m.events = vec![e];
        let diags = component_duplicate_diagnostics(&Component::Machine(m));
        assert!(
            dups_of(&diags, RuleId::DuplicateLabel).is_empty(),
            "{diags:#?}"
        );
    }

    #[test]
    fn whitespace_only_labels_are_not_duplicates() {
        // Whitespace-only labels are blank and must be skipped, not just the
        // empty string.
        let ws = || LabeledPredicate {
            label: Some("   ".into()),
            is_theorem: false,
            predicate: PredicateKind::True.into(),
            span: None,
            comment: None,
        };
        let mut e = Event::new("evt".into());
        e.guards = vec![ws(), ws()];
        let mut m = Machine::new("M".into());
        m.events = vec![e];
        let diags = component_duplicate_diagnostics(&Component::Machine(m));
        assert!(
            dups_of(&diags, RuleId::DuplicateLabel).is_empty(),
            "{diags:#?}"
        );
    }

    #[test]
    fn clean_model_produces_no_duplicate_findings() {
        let mut m = Machine::new("M".into());
        m.variables = vec![nv("x"), nv("y")];
        m.invariants = vec![
            lp("inv1", PredicateKind::True.into()),
            lp("inv2", PredicateKind::True.into()),
        ];
        let mut e = Event::new("evt".into());
        e.parameters = vec![nv("p"), nv("q")];
        e.guards = vec![lp("grd1", PredicateKind::True.into())];
        e.actions = vec![labeled_action("act1")];
        m.events = vec![e];
        let diags = component_duplicate_diagnostics(&Component::Machine(m));
        assert!(
            dups_of(&diags, RuleId::DuplicateIdentifier).is_empty()
                && dups_of(&diags, RuleId::DuplicateLabel).is_empty(),
            "{diags:#?}"
        );
    }
}
