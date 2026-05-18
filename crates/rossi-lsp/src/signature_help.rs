//! Signature Help provider for Event-B
//!
//! Provides parameter hints for:
//! - Universal quantifiers: ∀x,y·P ⇒ Q
//! - Existential quantifiers: ∃x,y·P
//! - Lambda functions: λx,y·E
//! - Set comprehension: {x·P | E}

use lsp_types::{
    ParameterInformation, ParameterLabel, Position, SignatureHelp, SignatureHelpParams,
    SignatureInformation,
};

/// Signature information for Event-B constructs
#[derive(Debug, Clone)]
struct RossiSignature {
    /// The signature label (e.g., "∀ identifiers · predicate ⇒ predicate")
    label: String,
    /// Parameter information
    parameters: Vec<ParameterInfo>,
    /// Documentation for this signature
    documentation: String,
}

#[derive(Debug, Clone)]
struct ParameterInfo {
    /// Parameter label (e.g., "identifiers", "predicate", "expression")
    label: String,
    /// Documentation for this parameter
    documentation: String,
}

/// Context about a signature at cursor position
#[derive(Debug, Clone)]
struct SignatureContext {
    /// The signature being used
    signature: RossiSignature,
    /// The active parameter index
    active_parameter: usize,
}

/// Provides signature help for Event-B documents
pub struct SignatureHelpProvider {}

impl Default for SignatureHelpProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SignatureHelpProvider {
    pub fn new() -> Self {
        Self {}
    }

    /// Generate signature help for the given position
    pub fn signature_help(
        &self,
        params: &SignatureHelpParams,
        text: &str,
    ) -> Option<SignatureHelp> {
        let position = params.text_document_position_params.position;

        // Find the signature context at cursor position
        let context = self.find_signature_context(text, position)?;

        // Convert to LSP SignatureHelp
        Some(SignatureHelp {
            signatures: vec![SignatureInformation {
                label: context.signature.label.clone(),
                documentation: Some(lsp_types::Documentation::MarkupContent(
                    lsp_types::MarkupContent {
                        kind: lsp_types::MarkupKind::Markdown,
                        value: context.signature.documentation.clone(),
                    },
                )),
                parameters: Some(
                    context
                        .signature
                        .parameters
                        .iter()
                        .map(|p| ParameterInformation {
                            label: ParameterLabel::Simple(p.label.clone()),
                            documentation: Some(lsp_types::Documentation::MarkupContent(
                                lsp_types::MarkupContent {
                                    kind: lsp_types::MarkupKind::Markdown,
                                    value: p.documentation.clone(),
                                },
                            )),
                        })
                        .collect(),
                ),
                active_parameter: Some(context.active_parameter as u32),
            }],
            active_signature: Some(0),
            active_parameter: Some(context.active_parameter as u32),
        })
    }

    /// Find the signature context at the given position
    fn find_signature_context(&self, text: &str, position: Position) -> Option<SignatureContext> {
        let offset = position_to_offset(text, position)?;

        // Try to find each type of signature
        self.find_quantifier_signature(text, offset)
            .or_else(|| self.find_lambda_signature(text, offset))
            .or_else(|| self.find_set_comprehension_signature(text, offset))
    }

    /// Find universal or existential quantifier signature
    fn find_quantifier_signature(&self, text: &str, offset: usize) -> Option<SignatureContext> {
        // Look backwards to find quantifier symbol
        let prefix = &text[..offset];

        // Find the last occurrence of ∀ or ∃ (or their ASCII equivalents)
        let (quant_start, is_universal) = self.find_last_quantifier(prefix)?;

        // Make sure we're actually inside this quantifier
        let remaining = &text[quant_start..];

        // Parse the quantifier structure
        // Expected format: ∀ identifiers · predicate [⇒ predicate]
        let content = remaining.get(..std::cmp::min(1000, remaining.len()))?;

        // Find the · separator (bullet point or middle dot)
        let bullet_pos = content.find('·').or_else(|| content.find('.'))?;

        // Check if cursor is before or after the bullet
        let relative_offset = offset - quant_start;

        let (signature, active_param) = if is_universal {
            // Universal quantifier: ∀ identifiers · predicate ⇒ predicate
            let has_implication =
                content[bullet_pos..].contains('⇒') || content[bullet_pos..].contains("=>");

            if relative_offset <= bullet_pos {
                // Cursor is in identifiers part
                (self.universal_quantifier_signature(), 0)
            } else if has_implication {
                // Check if we're in the first or second predicate
                let impl_pos = content[bullet_pos..]
                    .find('⇒')
                    .or_else(|| content[bullet_pos..].find("=>"))?;
                let abs_impl_pos = bullet_pos + impl_pos;

                if relative_offset <= abs_impl_pos {
                    (self.universal_quantifier_signature(), 1)
                } else {
                    (self.universal_quantifier_signature(), 2)
                }
            } else {
                // Only one predicate (shorthand form: ∀x·P)
                (self.universal_quantifier_short_signature(), 1)
            }
        } else {
            // Existential quantifier: ∃ identifiers · predicate
            if relative_offset <= bullet_pos {
                (self.existential_quantifier_signature(), 0)
            } else {
                (self.existential_quantifier_signature(), 1)
            }
        };

        Some(SignatureContext {
            signature,
            active_parameter: active_param,
        })
    }

    /// Find lambda function signature
    fn find_lambda_signature(&self, text: &str, offset: usize) -> Option<SignatureContext> {
        // Look backwards to find λ symbol
        let prefix = &text[..offset];
        let lambda_start = prefix.rfind('λ').or_else(|| prefix.rfind("%lambda"))?;

        // Parse the lambda structure
        // Expected format: λ identifiers · expression
        let remaining = &text[lambda_start..];
        let content = remaining.get(..std::cmp::min(1000, remaining.len()))?;

        // Find the · separator
        let bullet_pos = content.find('·').or_else(|| content.find('.'))?;

        // Check if cursor is before or after the bullet
        let relative_offset = offset - lambda_start;

        let active_param = if relative_offset <= bullet_pos { 0 } else { 1 };

        Some(SignatureContext {
            signature: self.lambda_signature(),
            active_parameter: active_param,
        })
    }

    /// Find set comprehension signature
    fn find_set_comprehension_signature(
        &self,
        text: &str,
        offset: usize,
    ) -> Option<SignatureContext> {
        // Look backwards to find opening brace
        let prefix = &text[..offset];
        let brace_start = prefix.rfind('{')?;

        // Parse the set comprehension structure
        // Expected format: {identifiers · predicate | expression} or {identifiers | predicate}
        let remaining = &text[brace_start..];
        let content = remaining.get(..std::cmp::min(1000, remaining.len()))?;

        // Check if there's a closing brace (to ensure we're inside)
        let _close_brace = content.find('}')?;

        // Find separators (· and |)
        let bullet_pos = content.find('·').or_else(|| content.find('.'));
        let pipe_pos = content.find('|');

        let relative_offset = offset - brace_start;

        // Determine the signature format and active parameter
        let (signature, active_param) = match (bullet_pos, pipe_pos) {
            (Some(bullet), Some(pipe)) if bullet < pipe => {
                // Full form: {identifiers · predicate | expression}
                if relative_offset <= bullet {
                    (self.set_comprehension_full_signature(), 0)
                } else if relative_offset <= pipe {
                    (self.set_comprehension_full_signature(), 1)
                } else {
                    (self.set_comprehension_full_signature(), 2)
                }
            }
            (None, Some(pipe)) => {
                // Short form: {identifiers | predicate}
                if relative_offset <= pipe {
                    (self.set_comprehension_short_signature(), 0)
                } else {
                    (self.set_comprehension_short_signature(), 1)
                }
            }
            _ => return None,
        };

        Some(SignatureContext {
            signature,
            active_parameter: active_param,
        })
    }

    /// Find the last quantifier in the text
    fn find_last_quantifier(&self, text: &str) -> Option<(usize, bool)> {
        // Find positions of quantifiers
        let forall_unicode = text.rfind('∀');
        let forall_ascii = text.rfind("!");
        let exists_unicode = text.rfind('∃');
        let exists_ascii = text.rfind("#");

        // Get the rightmost quantifier
        let mut max_pos = None;
        let mut is_universal = false;

        if let Some(pos) = forall_unicode {
            max_pos = Some(pos);
            is_universal = true;
        }

        if let Some(pos) = forall_ascii
            && max_pos.is_none_or(|m| pos > m)
        {
            max_pos = Some(pos);
            is_universal = true;
        }

        if let Some(pos) = exists_unicode
            && max_pos.is_none_or(|m| pos > m)
        {
            max_pos = Some(pos);
            is_universal = false;
        }

        if let Some(pos) = exists_ascii
            && max_pos.is_none_or(|m| pos > m)
        {
            max_pos = Some(pos);
            is_universal = false;
        }

        max_pos.map(|pos| (pos, is_universal))
    }

    // Signature definitions

    fn universal_quantifier_signature(&self) -> RossiSignature {
        RossiSignature {
            label: "∀ identifiers · antecedent ⇒ consequent".to_string(),
            parameters: vec![
                ParameterInfo {
                    label: "identifiers".to_string(),
                    documentation: "Comma-separated list of bound variables (e.g., `x,y,z`)"
                        .to_string(),
                },
                ParameterInfo {
                    label: "antecedent".to_string(),
                    documentation: "Predicate that constrains the bound variables".to_string(),
                },
                ParameterInfo {
                    label: "consequent".to_string(),
                    documentation: "Predicate that must hold when antecedent is true".to_string(),
                },
            ],
            documentation: "**Universal Quantifier**\n\nFor all values of the identifiers where the antecedent holds, the consequent must also hold.\n\nExample: `∀x·x ∈ ℕ ⇒ x ≥ 0`".to_string(),
        }
    }

    fn universal_quantifier_short_signature(&self) -> RossiSignature {
        RossiSignature {
            label: "∀ identifiers · predicate".to_string(),
            parameters: vec![
                ParameterInfo {
                    label: "identifiers".to_string(),
                    documentation: "Comma-separated list of bound variables (e.g., `x,y,z`)"
                        .to_string(),
                },
                ParameterInfo {
                    label: "predicate".to_string(),
                    documentation: "Predicate that must hold for all values".to_string(),
                },
            ],
            documentation:
                "**Universal Quantifier (Short Form)**\n\nThe predicate must hold for all values of the identifiers.\n\nExample: `∀x·x ∈ S`"
                    .to_string(),
        }
    }

    fn existential_quantifier_signature(&self) -> RossiSignature {
        RossiSignature {
            label: "∃ identifiers · predicate".to_string(),
            parameters: vec![
                ParameterInfo {
                    label: "identifiers".to_string(),
                    documentation: "Comma-separated list of bound variables (e.g., `x,y,z`)"
                        .to_string(),
                },
                ParameterInfo {
                    label: "predicate".to_string(),
                    documentation: "Predicate that must hold for at least one value".to_string(),
                },
            ],
            documentation: "**Existential Quantifier**\n\nThere exists at least one value of the identifiers for which the predicate holds.\n\nExample: `∃x·x ∈ S ∧ x > 0`".to_string(),
        }
    }

    fn lambda_signature(&self) -> RossiSignature {
        RossiSignature {
            label: "λ identifiers · expression".to_string(),
            parameters: vec![
                ParameterInfo {
                    label: "identifiers".to_string(),
                    documentation: "Comma-separated list of parameters (e.g., `x,y,z`)".to_string(),
                },
                ParameterInfo {
                    label: "expression".to_string(),
                    documentation: "Expression that computes the function result".to_string(),
                },
            ],
            documentation: "**Lambda Function**\n\nDefines an anonymous function that maps the identifiers to the expression.\n\nExample: `λx·x + 1`".to_string(),
        }
    }

    fn set_comprehension_full_signature(&self) -> RossiSignature {
        RossiSignature {
            label: "{identifiers · predicate | expression}".to_string(),
            parameters: vec![
                ParameterInfo {
                    label: "identifiers".to_string(),
                    documentation: "Comma-separated list of bound variables (e.g., `x,y`)".to_string(),
                },
                ParameterInfo {
                    label: "predicate".to_string(),
                    documentation: "Predicate that constrains which values are included".to_string(),
                },
                ParameterInfo {
                    label: "expression".to_string(),
                    documentation: "Expression that transforms each element".to_string(),
                },
            ],
            documentation: "**Set Comprehension (Full Form)**\n\nCreates a set by evaluating the expression for all values satisfying the predicate.\n\nExample: `{x·x ∈ ℕ ∧ x < 10 | x × 2}`".to_string(),
        }
    }

    fn set_comprehension_short_signature(&self) -> RossiSignature {
        RossiSignature {
            label: "{identifiers | predicate}".to_string(),
            parameters: vec![
                ParameterInfo {
                    label: "identifiers".to_string(),
                    documentation: "Comma-separated list of bound variables (e.g., `x,y`)".to_string(),
                },
                ParameterInfo {
                    label: "predicate".to_string(),
                    documentation: "Predicate that defines set membership".to_string(),
                },
            ],
            documentation: "**Set Comprehension (Short Form)**\n\nCreates a set of all values satisfying the predicate.\n\nExample: `{x | x ∈ S ∧ x > 0}`".to_string(),
        }
    }
}

/// Convert LSP Position to byte offset in text
fn position_to_offset(text: &str, position: Position) -> Option<usize> {
    let mut line = 0;
    let mut col = 0;
    let mut offset = 0;

    for ch in text.chars() {
        if line == position.line as usize && col == position.character as usize {
            return Some(offset);
        }

        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }

        offset += ch.len_utf8();
    }

    // Handle position at end of file
    if line == position.line as usize && col == position.character as usize {
        Some(offset)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_position(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    fn make_params(position: Position) -> SignatureHelpParams {
        SignatureHelpParams {
            text_document_position_params: lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier {
                    uri: lsp_types::Url::parse("file:///test.eventb").unwrap(),
                },
                position,
            },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            context: None,
        }
    }

    #[test]
    fn test_position_to_offset() {
        let text = "line1\nline2\nline3";
        assert_eq!(position_to_offset(text, make_position(0, 0)), Some(0));
        assert_eq!(position_to_offset(text, make_position(0, 5)), Some(5));
        assert_eq!(position_to_offset(text, make_position(1, 0)), Some(6));
        assert_eq!(position_to_offset(text, make_position(1, 3)), Some(9));
        assert_eq!(position_to_offset(text, make_position(2, 5)), Some(17));
    }

    #[test]
    fn test_universal_quantifier_identifiers() {
        let provider = SignatureHelpProvider::new();
        let text = "∀x,y,z·x ∈ S ⇒ y ∈ T";
        let params = make_params(make_position(0, 3)); // cursor on 'x,y,z'

        let result = provider.signature_help(&params, text);
        assert!(result.is_some());

        let help = result.unwrap();
        assert_eq!(help.signatures.len(), 1);
        assert_eq!(help.active_parameter, Some(0));
        assert!(help.signatures[0].label.contains("identifiers"));
    }

    #[test]
    fn test_universal_quantifier_antecedent() {
        let provider = SignatureHelpProvider::new();
        let text = "∀x·x ∈ S ⇒ x > 0";
        let params = make_params(make_position(0, 6)); // cursor on 'x ∈ S'

        let result = provider.signature_help(&params, text);
        assert!(result.is_some());

        let help = result.unwrap();
        assert_eq!(help.active_parameter, Some(1)); // antecedent
    }

    #[test]
    fn test_universal_quantifier_consequent() {
        let provider = SignatureHelpProvider::new();
        let text = "∀x·x ∈ S ⇒ x > 0";
        let params = make_params(make_position(0, 14)); // cursor on 'x > 0'

        let result = provider.signature_help(&params, text);
        assert!(result.is_some());

        let help = result.unwrap();
        assert_eq!(help.active_parameter, Some(2)); // consequent
    }

    #[test]
    fn test_universal_quantifier_short_form() {
        let provider = SignatureHelpProvider::new();
        let text = "∀x·x ∈ S";
        let params = make_params(make_position(0, 5)); // cursor on 'x ∈ S'

        let result = provider.signature_help(&params, text);
        assert!(result.is_some());

        let help = result.unwrap();
        assert_eq!(help.active_parameter, Some(1));
    }

    #[test]
    fn test_existential_quantifier() {
        let provider = SignatureHelpProvider::new();
        let text = "∃x·x ∈ S ∧ x > 0";
        let params = make_params(make_position(0, 6)); // cursor on predicate

        let result = provider.signature_help(&params, text);
        assert!(result.is_some());

        let help = result.unwrap();
        assert_eq!(help.signatures.len(), 1);
        assert_eq!(help.active_parameter, Some(1)); // predicate
        assert!(help.signatures[0].label.contains("∃"));
    }

    #[test]
    fn test_lambda_function_identifiers() {
        let provider = SignatureHelpProvider::new();
        let text = "λx,y·x + y";
        let params = make_params(make_position(0, 2)); // cursor on 'x,y'

        let result = provider.signature_help(&params, text);
        assert!(result.is_some());

        let help = result.unwrap();
        assert_eq!(help.active_parameter, Some(0)); // identifiers
        assert!(help.signatures[0].label.contains("λ"));
    }

    #[test]
    fn test_lambda_function_expression() {
        let provider = SignatureHelpProvider::new();
        let text = "λx·x + 1";
        let params = make_params(make_position(0, 5)); // cursor on 'x + 1'

        let result = provider.signature_help(&params, text);
        assert!(result.is_some());

        let help = result.unwrap();
        assert_eq!(help.active_parameter, Some(1)); // expression
    }

    #[test]
    fn test_set_comprehension_short_form() {
        let provider = SignatureHelpProvider::new();
        let text = "{x | x ∈ S ∧ x > 0}";
        let params = make_params(make_position(0, 8)); // cursor on predicate

        let result = provider.signature_help(&params, text);
        assert!(result.is_some());

        let help = result.unwrap();
        assert_eq!(help.active_parameter, Some(1)); // predicate
    }

    #[test]
    fn test_set_comprehension_full_form() {
        let provider = SignatureHelpProvider::new();
        let text = "{x·x ∈ ℕ ∧ x < 10 | x × 2}";
        let params = make_params(make_position(0, 22)); // cursor on expression

        let result = provider.signature_help(&params, text);
        assert!(result.is_some());

        let help = result.unwrap();
        assert_eq!(help.active_parameter, Some(2)); // expression
    }

    #[test]
    fn test_set_comprehension_full_form_predicate() {
        let provider = SignatureHelpProvider::new();
        let text = "{x·x ∈ ℕ | x}";
        let params = make_params(make_position(0, 5)); // cursor on predicate

        let result = provider.signature_help(&params, text);
        assert!(result.is_some());

        let help = result.unwrap();
        assert_eq!(help.active_parameter, Some(1)); // predicate
    }

    #[test]
    fn test_no_signature_in_regular_code() {
        let provider = SignatureHelpProvider::new();
        let text = "MACHINE Test\nVARIABLES x\nEND";
        let params = make_params(make_position(1, 5)); // cursor on VARIABLES

        let result = provider.signature_help(&params, text);
        assert!(result.is_none());
    }

    #[test]
    fn test_ascii_quantifiers() {
        let provider = SignatureHelpProvider::new();
        let text = "!x.x : S => x > 0";
        let params = make_params(make_position(0, 10)); // cursor on consequent

        let result = provider.signature_help(&params, text);
        assert!(result.is_some());

        let help = result.unwrap();
        assert_eq!(help.active_parameter, Some(2)); // consequent
    }
}
