//! Tests for semantic tokens provider

use rossi_lsp::lsp_types::{SemanticTokensParams, TextDocumentIdentifier, Url};
use rossi_lsp::semantic_tokens::SemanticTokensProvider;

#[test]
fn test_semantic_tokens_simple_machine() {
    let provider = SemanticTokensProvider::new();

    let text = r#"
MACHINE Counter
VARIABLES
    count
INVARIANTS
    @inv1 count >= 0
EVENTS
    EVENT INITIALISATION
    THEN
        count := 0
    END

    EVENT Increment
    THEN
        count := count + 1
    END
END
"#;

    let uri = Url::parse("file:///test.eventb").unwrap();
    let params = SemanticTokensParams {
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
        text_document: TextDocumentIdentifier { uri },
    };

    let result = provider.semantic_tokens(&params, text);

    assert!(result.is_some(), "Should return semantic tokens");

    if let Some(rossi_lsp::lsp_types::SemanticTokensResult::Tokens(tokens)) = result {
        assert!(!tokens.data.is_empty(), "Should have semantic tokens");
        assert!(
            tokens.data.len() >= 5,
            "Should have at least 5 semantic tokens"
        );
    } else {
        panic!("Expected SemanticTokensResult::Tokens");
    }
}

#[test]
fn test_semantic_tokens_legend() {
    let legend = SemanticTokensProvider::legend();

    assert!(!legend.token_types.is_empty(), "Should have token types");
    assert!(
        !legend.token_modifiers.is_empty(),
        "Should have token modifiers"
    );

    let type_strings: Vec<String> = legend
        .token_types
        .iter()
        .map(|t| t.as_str().to_string())
        .collect();

    assert!(
        type_strings.contains(&"keyword".to_string()),
        "Should have keyword token type"
    );
    assert!(
        type_strings.contains(&"variable".to_string()),
        "Should have variable token type"
    );
    assert!(
        type_strings.contains(&"parameter".to_string()),
        "Should have parameter token type"
    );

    let modifier_strings: Vec<String> = legend
        .token_modifiers
        .iter()
        .map(|m| m.as_str().to_string())
        .collect();

    assert!(
        modifier_strings.contains(&"declaration".to_string()),
        "Should have declaration modifier"
    );
    assert!(
        modifier_strings.contains(&"readonly".to_string()),
        "Should have readonly modifier"
    );
}

#[test]
fn test_semantic_tokens_returns_none_for_unparseable_input() {
    let provider = SemanticTokensProvider::new();
    let uri = Url::parse("file:///test.eventb").unwrap();

    for text in ["", "INVALID SYNTAX HERE\nTHIS IS NOT VALID EVENT-B\n"] {
        let params = SemanticTokensParams {
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            text_document: TextDocumentIdentifier { uri: uri.clone() },
        };
        let result = provider.semantic_tokens(&params, text);
        assert!(
            result.is_none(),
            "expected None for unparseable input {text:?}, got {result:?}"
        );
    }
}

#[test]
fn test_theorems_header_is_highlighted() {
    let provider = SemanticTokensProvider::new();
    let token_count = |text: &str| -> usize {
        let uri = Url::parse("file:///test.eventb").unwrap();
        let params = SemanticTokensParams {
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            text_document: TextDocumentIdentifier { uri },
        };
        match provider.semantic_tokens(&params, text) {
            Some(rossi_lsp::lsp_types::SemanticTokensResult::Tokens(t)) => t.data.len(),
            other => panic!("expected tokens, got {other:?}"),
        }
    };

    // Same predicates and labels; the only difference is the THEOREMS header, which
    // must contribute exactly one extra keyword token. (The inline `theorem` flag is
    // not tokenized, so the inline variant has one fewer.)
    let sectioned = "CONTEXT c\nAXIOMS\n    @axm1 1 = 1\nTHEOREMS\n    @thm1 2 = 2\nEND\n";
    let inline = "CONTEXT c\nAXIOMS\n    @axm1 1 = 1\n    theorem @thm1 2 = 2\nEND\n";

    assert_eq!(token_count(sectioned), token_count(inline) + 1);
}
