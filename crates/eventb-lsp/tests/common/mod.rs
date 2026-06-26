//! Helpers shared between eventb-lsp integration test binaries.

use eventb_lsp::identifier_utils::position_to_offset;
use eventb_lsp::lsp_types::{
    PartialResultParams, Range, SemanticTokensParams, SemanticTokensResult, TextDocumentIdentifier,
    Url, WorkDoneProgressParams,
};
use eventb_lsp::semantic_tokens::SemanticTokensProvider;

/// Slice `text` at a char-based `range`, going through the same
/// position-to-offset mapping the providers use (columns are characters,
/// never bytes).
pub fn slice_range(text: &str, range: Range) -> String {
    assert!(range.start <= range.end, "reversed range: {range:?}");
    let start = position_to_offset(text, range.start)
        .unwrap_or_else(|| panic!("range start {:?} out of bounds", range.start));
    let end = position_to_offset(text, range.end)
        .unwrap_or_else(|| panic!("range end {:?} out of bounds", range.end));
    text[start..end].to_string()
}

/// Decode delta-encoded semantic tokens for `text` into
/// `(line, col, len, token_type)`, all 0-indexed.
pub fn decode_tokens(text: &str) -> Vec<(u32, u32, u32, u32)> {
    decode_tokens_with_modifiers(text)
        .into_iter()
        .map(|(line, col, len, token_type, _)| (line, col, len, token_type))
        .collect()
}

/// Like [`decode_tokens`] but also returns each token's modifier bitset, as
/// `(line, col, len, token_type, token_modifiers)`.
pub fn decode_tokens_with_modifiers(text: &str) -> Vec<(u32, u32, u32, u32, u32)> {
    let provider = SemanticTokensProvider::new();
    let params = SemanticTokensParams {
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        text_document: TextDocumentIdentifier {
            uri: Url::parse("file:///decode-probe.eventb").unwrap(),
        },
    };
    let parsed = rossi::parse_components_with_recovery(text);
    let components = parsed.component.as_deref().unwrap_or_default();
    let data = match provider.semantic_tokens(&params, text, components) {
        Some(SemanticTokensResult::Tokens(tokens)) => tokens.data,
        other => panic!("expected tokens, got {other:?}"),
    };

    let mut decoded = Vec::new();
    let (mut line, mut col) = (0u32, 0u32);
    for token in &data {
        line += token.delta_line;
        col = if token.delta_line == 0 {
            col + token.delta_start
        } else {
            token.delta_start
        };
        decoded.push((
            line,
            col,
            token.length,
            token.token_type,
            token.token_modifiers_bitset,
        ));
    }
    decoded
}
