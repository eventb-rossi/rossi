//! `rossi/proofState`: one obligation's sequent, for a goal view.
//!
//! This is rossi's counterpart of Lean's `$/lean/plainGoal` and coq-lsp's
//! `proof/goals`: the typed identifiers, the hypotheses and the goal of the
//! obligation the user asks about, pretty-printed in the user's configured
//! operator style. The obligation is regenerated on request from the current
//! closure rather than read from the list the overlay holds, because the
//! overlay keeps only names and statuses; a sequent is asked for one at a
//! time, and regenerating is the same save-cadence build the list itself
//! came from.

use rossi_build::po_view::PoView;
use serde::{Deserialize, Serialize};

use crate::component_loader::ComponentLoader;
use crate::document::ParsedDocument;
use crate::lsp_types::*;

/// The custom request that returns one obligation's sequent.
pub const REQUEST_STATE: &str = "rossi/proofState";

/// `rossi/proofState` parameters: the document and the obligation's name as
/// `rossi/proofObligations` reported it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofStateParams {
    pub text_document: TextDocumentIdentifier,
    pub name: String,
}

/// One typed identifier of a sequent's environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedIdentifier {
    pub name: String,
    /// The type as the generator spells it.
    #[serde(rename = "type")]
    pub ty: String,
}

/// A sequent, both structured and rendered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofState {
    pub name: String,
    pub component: String,
    pub description: String,
    pub identifiers: Vec<TypedIdentifier>,
    /// Hypotheses root-first through the predicate-set chain, then the
    /// obligation's own, each in the configured operator style.
    pub hypotheses: Vec<String>,
    pub goal: String,
    /// The whole sequent as Event-B text, for a client that shows it in a
    /// read-only editor: comments carry the metadata, formulas stand on
    /// their own lines, and a `⊢` line separates hypotheses from goal.
    pub text: String,
}

/// The sequent of the obligation `name` in `doc`, or `None` when the
/// document does not parse, the obligation is not among the generated ones,
/// or its component was dropped by the static check.
pub(crate) fn compute(
    doc: &ParsedDocument,
    loader: &ComponentLoader,
    name: &str,
    printer: &rossi::PrettyPrinter,
) -> Option<ProofState> {
    if !doc.parse().errors.is_empty() {
        return None;
    }
    let project = crate::closure::project_for(doc, loader, "lsp-proof-state");
    let result = rossi_build::build(&project);

    for component in doc.components() {
        let component_name = component.name();
        let Some(bpo) = result.file(&format!("{component_name}.bpo")) else {
            continue;
        };
        let Ok(view) = PoView::from_xml(&bpo.contents) else {
            continue;
        };
        let Some(sequent) = view.sequents.get(name) else {
            continue;
        };

        let identifiers: Vec<TypedIdentifier> = view
            .flattened_identifiers(name)
            .into_iter()
            .map(|(ident, ty)| TypedIdentifier {
                name: ident.to_string(),
                ty: ty.to_string(),
            })
            .collect();
        let hypotheses: Vec<String> = view
            .flattened_hypotheses(name)
            .into_iter()
            .map(|hypothesis| printer.print_formula_predicate_wrapped(&hypothesis.predicate))
            .collect();
        let goal = sequent
            .goal
            .as_ref()
            .map(|goal| printer.print_formula_predicate_wrapped(&goal.predicate))
            .unwrap_or_default();

        let mut state = ProofState {
            name: name.to_string(),
            component: component_name.to_string(),
            description: sequent.description.clone(),
            identifiers,
            hypotheses,
            goal,
            text: String::new(),
        };
        state.text = render(&state);
        return Some(state);
    }
    None
}

/// The sequent as a block of Event-B text.
fn render(state: &ProofState) -> String {
    let ProofState {
        name,
        component,
        description,
        identifiers,
        hypotheses,
        goal,
        ..
    } = state;
    let mut out = String::new();
    out.push_str(&format!("// {component}: {name}\n// {description}\n"));
    if !identifiers.is_empty() {
        out.push_str("//\n// Identifiers\n");
        for identifier in identifiers {
            out.push_str(&format!("//   {} : {}\n", identifier.name, identifier.ty));
        }
    }
    out.push_str("//\n// Hypotheses\n");
    for hypothesis in hypotheses {
        out.push_str(hypothesis);
        out.push('\n');
    }
    out.push_str("\u{22a2}\n");
    out.push_str(goal);
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rendering_separates_hypotheses_from_the_goal() {
        let text = render(&ProofState {
            name: "bump/inv1/INV".into(),
            component: "m".into(),
            description: "Invariant  preservation".into(),
            identifiers: vec![TypedIdentifier {
                name: "x".into(),
                ty: "\u{2124}".into(),
            }],
            hypotheses: vec!["x \u{2208} \u{2115}".into()],
            goal: "x + 1 \u{2208} \u{2115}".into(),
            text: String::new(),
        });
        assert_eq!(
            text,
            "// m: bump/inv1/INV\n// Invariant  preservation\n//\n// Identifiers\n//   x : \u{2124}\n//\n// Hypotheses\nx \u{2208} \u{2115}\n\u{22a2}\nx + 1 \u{2208} \u{2115}\n"
        );
    }
}
