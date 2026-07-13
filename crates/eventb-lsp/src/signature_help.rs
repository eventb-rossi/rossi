//! Signature help for Event-B binding constructs.
//!
//! Rossi identifies the innermost construct and active syntactic part from the
//! document's shared parse/syntax snapshot. This module only maps that result
//! to LSP labels and documentation.

use crate::document::ParsedDocument;
use crate::identifier_utils::position_to_offset;
use crate::lsp_types::{
    ParameterInformation, ParameterLabel, SignatureHelp, SignatureHelpParams, SignatureInformation,
};
use rossi::{SyntaxAtOffset, SyntaxConstruct, SyntaxParameter};

#[derive(Debug, Clone, Copy)]
struct RossiSignature {
    label: &'static str,
    parameters: &'static [ParameterInfo],
    documentation: &'static str,
}

#[derive(Debug, Clone, Copy)]
struct ParameterInfo {
    syntax: SyntaxParameter,
    label: &'static str,
    documentation: &'static str,
}

const IDENTIFIERS: ParameterInfo = ParameterInfo {
    syntax: SyntaxParameter::Identifiers,
    label: "identifiers",
    documentation: "Comma-separated list of bound variables (for example, `x,y,z`)",
};
const PATTERN: ParameterInfo = ParameterInfo {
    syntax: SyntaxParameter::Pattern,
    label: "pattern",
    documentation: "Lambda binding pattern, including maplet patterns such as `x ↦ y`",
};
const PREDICATE: ParameterInfo = ParameterInfo {
    syntax: SyntaxParameter::Predicate,
    label: "predicate",
    documentation: "Predicate constraining the bound values",
};
const EXPRESSION: ParameterInfo = ParameterInfo {
    syntax: SyntaxParameter::Expression,
    label: "expression",
    documentation: "Expression evaluated for values satisfying the predicate",
};

/// Provides signature help for Event-B documents.
pub struct SignatureHelpProvider;

impl Default for SignatureHelpProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SignatureHelpProvider {
    pub fn new() -> Self {
        Self
    }

    /// Generate signature help from one atomic parsed document snapshot.
    pub fn signature_help(
        &self,
        params: &SignatureHelpParams,
        document: &ParsedDocument,
    ) -> Option<SignatureHelp> {
        let position = params.text_document_position_params.position;
        let offset = position_to_offset(document.text(), position)?;
        let syntax = document.syntax_at_offset(offset)?;
        let (signature, active_parameter) = signature_for(syntax)?;

        Some(SignatureHelp {
            signatures: vec![SignatureInformation {
                label: signature.label.to_string(),
                documentation: Some(crate::lsp_types::Documentation::MarkupContent(
                    crate::lsp_types::MarkupContent {
                        kind: crate::lsp_types::MarkupKind::Markdown,
                        value: signature.documentation.to_string(),
                    },
                )),
                parameters: Some(
                    signature
                        .parameters
                        .iter()
                        .map(|parameter| ParameterInformation {
                            label: ParameterLabel::Simple(parameter.label.to_string()),
                            documentation: Some(crate::lsp_types::Documentation::MarkupContent(
                                crate::lsp_types::MarkupContent {
                                    kind: crate::lsp_types::MarkupKind::Markdown,
                                    value: parameter.documentation.to_string(),
                                },
                            )),
                        })
                        .collect(),
                ),
                active_parameter: Some(active_parameter),
            }],
            active_signature: Some(0),
            active_parameter: Some(active_parameter),
        })
    }
}

fn signature_for(syntax: SyntaxAtOffset) -> Option<(RossiSignature, u32)> {
    let signature = match syntax.construct {
        SyntaxConstruct::UniversalQuantifier => RossiSignature {
            label: "∀ identifiers · predicate",
            parameters: &[IDENTIFIERS, PREDICATE],
            documentation: "**Universal Quantifier**\n\nThe predicate must hold for every value of the bound identifiers.\n\nExample: `∀x·x ∈ ℕ ⇒ x ≥ 0`",
        },
        SyntaxConstruct::ExistentialQuantifier => RossiSignature {
            label: "∃ identifiers · predicate",
            parameters: &[IDENTIFIERS, PREDICATE],
            documentation: "**Existential Quantifier**\n\nThe predicate must hold for at least one value of the bound identifiers.\n\nExample: `∃x·x ∈ S ∧ x > 0`",
        },
        SyntaxConstruct::Lambda => RossiSignature {
            label: "λ pattern · predicate ∣ expression",
            parameters: &[PATTERN, PREDICATE, EXPRESSION],
            documentation: "**Lambda Expression**\n\nDefines a relation from values matching the pattern and predicate to the expression.\n\nExample: `λx·x ∈ ℕ ∣ x + 1`",
        },
        SyntaxConstruct::BasicSetComprehension => RossiSignature {
            label: "{identifiers ∣ predicate}",
            parameters: &[IDENTIFIERS, PREDICATE],
            documentation: "**Set Comprehension**\n\nBuilds the set of bound values satisfying the predicate.\n\nExample: `{x ∣ x ∈ S ∧ x > 0}`",
        },
        SyntaxConstruct::ExtendedSetComprehension => RossiSignature {
            label: "{identifiers · predicate ∣ expression}",
            parameters: &[IDENTIFIERS, PREDICATE, EXPRESSION],
            documentation: "**Extended Set Comprehension**\n\nEvaluates the expression for bound values satisfying the predicate.\n\nExample: `{x·x ∈ ℕ ∧ x < 10 ∣ x × 2}`",
        },
        SyntaxConstruct::SetBuilder => RossiSignature {
            label: "{expression ∣ predicate}",
            parameters: &[EXPRESSION, PREDICATE],
            documentation: "**Set Builder**\n\nBuilds a set from an expression for values satisfying the predicate.\n\nExample: `{x ↦ y ∣ x ∈ S ∧ y ∈ T}`",
        },
    };

    let active_parameter = signature
        .parameters
        .iter()
        .position(|parameter| parameter.syntax == syntax.parameter)?
        as u32;
    Some((signature, active_parameter))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp_types::{
        Position, TextDocumentIdentifier, TextDocumentPositionParams, Url, WorkDoneProgressParams,
    };
    use crate::position::offset_to_position;

    const SOURCE: &str = concat!(
        "MACHINE m\nINVARIANTS\n",
        "@q ∀x·x > 0 ⇒ x ∈ ℕ\n",
        "@e ∃x·x ∈ ℕ\n",
        "@l (λx·x ∈ ℕ ∣ x + 1)(1) = 2\n",
        "@s {x·x ∈ ℕ ∣ x + 1} ⊆ ℕ\n",
        "@b {x ↦ x + 1 ∣ x ∈ ℕ} ⊆ ℕ × ℕ\n",
        "END\n",
    );

    fn params(position: Position) -> SignatureHelpParams {
        SignatureHelpParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: Url::parse("file:///test.eventb").unwrap(),
                },
                position,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            context: None,
        }
    }

    fn help_at(text: &str, offset: usize) -> Option<SignatureHelp> {
        let document = ParsedDocument::from_text(text.to_string());
        SignatureHelpProvider::new()
            .signature_help(&params(offset_to_position(text, offset)), &document)
    }

    fn offset(text: &str, needle: &str) -> usize {
        text.find(needle).expect("needle present")
    }

    fn assert_help(text: &str, needle: &str, label: &str, active: u32) {
        let help = help_at(text, offset(text, needle)).expect("signature help");
        assert_eq!(help.signatures[0].label, label);
        assert_eq!(help.active_parameter, Some(active));
    }

    #[test]
    fn quantifiers_follow_the_grammar() {
        assert_help(SOURCE, "∀x", "∀ identifiers · predicate", 0);
        assert_help(SOURCE, "x > 0", "∀ identifiers · predicate", 1);
        assert_help(SOURCE, "∃x", "∃ identifiers · predicate", 0);
        assert_help(SOURCE, "x ∈ ℕ\n@l", "∃ identifiers · predicate", 1);
    }

    #[test]
    fn lambda_has_pattern_predicate_and_expression() {
        assert_help(SOURCE, "λx", "λ pattern · predicate ∣ expression", 0);
        assert_help(SOURCE, "x ∈ ℕ ∣", "λ pattern · predicate ∣ expression", 1);
        assert_help(SOURCE, "x + 1)(1)", "λ pattern · predicate ∣ expression", 2);
    }

    #[test]
    fn distinguishes_set_comprehension_forms() {
        assert_help(SOURCE, "{x·", "{identifiers · predicate ∣ expression}", 0);
        assert_help(
            SOURCE,
            "x + 1} ⊆",
            "{identifiers · predicate ∣ expression}",
            2,
        );
        assert_help(
            SOURCE,
            "x ∈ ℕ ∣ x + 1}",
            "{identifiers · predicate ∣ expression}",
            1,
        );
        assert_help(SOURCE, "x ↦ x + 1", "{expression ∣ predicate}", 0);
        assert_help(SOURCE, "x ∈ ℕ} ⊆", "{expression ∣ predicate}", 1);

        let basic = "MACHINE m\nINVARIANTS\n@i {x ∣ x ∈ ℕ} ⊆ ℕ\nEND\n";
        assert_help(basic, "x ∈ ℕ", "{identifiers ∣ predicate}", 1);
    }

    #[test]
    fn supports_ascii_spellings() {
        let text = concat!(
            "MACHINE m\nINVARIANTS\n",
            "@q !x.x : NAT\n",
            "@l (%x.x : NAT | x + 1)(1) = 2\n",
            "@s {x | x : NAT} ⊆ NAT\n",
            "@e {x.x : NAT | x + 1} ⊆ NAT\n",
            "END\n",
        );
        assert_help(text, "x : NAT\n", "∀ identifiers · predicate", 1);
        assert_help(text, "x + 1", "λ pattern · predicate ∣ expression", 2);
        assert_help(text, "x : NAT} ⊆", "{identifiers ∣ predicate}", 1);
        assert_help(
            text,
            "x : NAT | x + 1}",
            "{identifiers · predicate ∣ expression}",
            1,
        );
    }

    #[test]
    fn incomplete_construct_uses_bounded_fallback() {
        let text = "MACHINE m\nINVARIANTS\n@bad %x.x : NAT | \nEND\n";
        let cursor = text.find(" \nEND").unwrap();
        let help = help_at(text, cursor).expect("fallback signature help");
        assert_eq!(
            help.signatures[0].label,
            "λ pattern · predicate ∣ expression"
        );
        assert_eq!(help.active_parameter, Some(2));
    }

    #[test]
    fn ignores_comments_and_positions_after_the_construct() {
        let text = "MACHINE m\nINVARIANTS\n@i ∀x·x > 0 /* ∃y·y > 0 */\nEND\n";
        assert!(help_at(text, offset(text, "∃y")).is_none());
        assert!(help_at(text, text.find(" /*").unwrap()).is_none());
    }

    #[test]
    fn has_no_arbitrary_lookup_window() {
        let identifiers = (0..400)
            .map(|index| format!("x{index}"))
            .collect::<Vec<_>>()
            .join(",");
        let text = format!("MACHINE m\nINVARIANTS\n@i ∀{identifiers}·x0 = x0\nEND\n");
        let cursor = text.find("x399").unwrap();
        let help = help_at(&text, cursor).expect("signature beyond 1000 bytes");
        assert_eq!(help.signatures[0].label, "∀ identifiers · predicate");
        assert_eq!(help.active_parameter, Some(0));
    }
}
