//! Tests for semantic tokens provider

use rossi_lsp::lsp_types::{SemanticTokensParams, TextDocumentIdentifier, Url};
use rossi_lsp::semantic_tokens::SemanticTokensProvider;

mod common;
use common::{decode_tokens, slice_range};

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
    let token_count = |text: &str| decode_tokens(text).len();

    // Same predicates and labels; the only difference is the THEOREMS header, which
    // must contribute exactly one extra keyword token. (The inline `theorem` flag is
    // not tokenized, so the inline variant has one fewer.)
    let sectioned = "CONTEXT c\nAXIOMS\n    @axm1 1 = 1\nTHEOREMS\n    @thm1 2 = 2\nEND\n";
    let inline = "CONTEXT c\nAXIOMS\n    @axm1 1 = 1\n    theorem @thm1 2 = 2\nEND\n";

    assert_eq!(token_count(sectioned), token_count(inline) + 1);
}

// ============================================================================
// Issue #24: tokens must never land inside comments, and comments must get
// their own COMMENT tokens (otherwise editors that prioritize semantic tokens
// lose comment highlighting after a colon).
// ============================================================================

/// Index of `name` in the legend's token types — derived, so reordering the
/// legend cannot silently re-point these tests at the wrong token type.
fn token_type_index(name: &str) -> u32 {
    SemanticTokensProvider::legend()
        .token_types
        .iter()
        .position(|t| t.as_str() == name)
        .unwrap_or_else(|| panic!("token type {name} not in legend")) as u32
}

/// Slice the token at `(line, col, len)` out of `text` (0-indexed,
/// char-based columns like the provider's).
fn comment_text(text: &str, line: u32, col: u32, len: u32) -> String {
    use rossi_lsp::lsp_types::{Position, Range};
    slice_range(
        text,
        Range::new(Position::new(line, col), Position::new(line, col + len)),
    )
}

#[test]
fn test_no_identifier_tokens_inside_comments() {
    // The comment mentions `x` before its declaration; the variable token
    // must land on the declaration, not inside the comment (issue #24).
    let text = "MACHINE m\nVARIABLES\n    // state: x is the counter\n    x\nINVARIANTS\n    @inv1 x >= 0\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        x := 0\n    END\nEND\n";

    let tokens = decode_tokens(text);

    let variable = token_type_index("variable");
    let variables: Vec<_> = tokens.iter().filter(|t| t.3 == variable).collect();
    assert!(
        variables.iter().all(|t| t.0 != 2),
        "no variable token may sit on the comment line, got {variables:?}"
    );
    assert!(
        variables.iter().any(|t| t.0 == 3 && t.1 == 4),
        "the declaration of x must carry the variable token, got {variables:?}"
    );
}

#[test]
fn test_no_label_tokens_inside_comments() {
    let text = "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    // inv1: x stays a natural\n    @inv1 x >= 0\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        x := 0\n    END\nEND\n";

    let tokens = decode_tokens(text);

    let label = token_type_index("macro"); // labels use the MACRO slot
    let labels: Vec<_> = tokens.iter().filter(|t| t.3 == label).collect();
    assert!(
        labels.iter().all(|t| t.0 != 4),
        "no label token may sit on the comment line, got {labels:?}"
    );
    assert!(
        labels.iter().any(|t| t.0 == 5 && t.1 == 5),
        "the real @inv1 must carry the label token, got {labels:?}"
    );
}

#[test]
fn test_keyword_in_comment_not_tokenized() {
    // "AXIOMS" and "END" appear in a comment before the real keywords.
    let text =
        "CONTEXT c\n// the AXIOMS: section and its END: marker\nAXIOMS\n    @axm1 1 = 1\nEND\n";

    let tokens = decode_tokens(text);

    let keyword = token_type_index("keyword");
    let keywords: Vec<_> = tokens.iter().filter(|t| t.3 == keyword).collect();
    assert!(
        keywords.iter().all(|t| t.0 != 1),
        "no keyword token may sit on the comment line, got {keywords:?}"
    );
    assert!(
        keywords.iter().any(|t| t.0 == 2 && t.1 == 0),
        "the real AXIOMS keyword must be tokenized, got {keywords:?}"
    );
    assert!(
        keywords.iter().any(|t| t.0 == 4 && t.1 == 0),
        "the real END keyword must be tokenized, got {keywords:?}"
    );
}

#[test]
fn test_comment_tokens_emitted() {
    let text = "CONTEXT c\n// leading: note\nAXIOMS\n    @axm1 1 = 1 // trailing: note\n    /* block: first\n       second */\n    @axm2 2 = 2\nEND\n";

    let tokens = decode_tokens(text);

    let comment = token_type_index("comment");
    let comments: Vec<_> = tokens
        .iter()
        .filter(|t| t.3 == comment)
        .map(|t| comment_text(text, t.0, t.1, t.2))
        .collect();
    assert_eq!(
        comments,
        [
            "// leading: note",
            "// trailing: note",
            "/* block: first",
            "       second */", // continuation lines are covered from column 0
        ],
        "every comment line must be covered by one COMMENT token"
    );
}

#[test]
fn test_keyword_not_matched_inside_identifier() {
    // `extended` contains "end"; the END keyword tokens must sit on the
    // real END lines, never inside the identifier.
    let text = "MACHINE m\nVARIABLES\n    extended\nINVARIANTS\n    @inv1 extended >= 0\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        extended := 0\n    END\nEND\n";

    let tokens = decode_tokens(text);

    let keyword = token_type_index("keyword");
    let keywords: Vec<_> = tokens.iter().filter(|t| t.3 == keyword).collect();
    assert!(
        keywords.iter().all(|t| !(t.0 == 2 || t.0 == 4 || t.0 == 8)),
        "no keyword token may land inside the identifier `extended`, got {keywords:?}"
    );
    let variable = token_type_index("variable");
    let variables: Vec<_> = tokens.iter().filter(|t| t.3 == variable).collect();
    assert!(
        variables.iter().any(|t| t.0 == 2 && t.1 == 4),
        "the declaration of `extended` must carry the variable token, got {variables:?}"
    );
}

#[test]
fn test_hyphenated_name_not_split_into_keyword() {
    // Regression for issue #36: a structural keyword scan must not latch onto
    // the `end` fragment of the hyphenated event name `end-update`. Here the
    // machine's END search crosses the second event (INITIALISATION is walked
    // separately, so the EVENTS clause re-scan starts mid-document) and, under
    // the old math word boundary, matched `end` because `-` counted as a
    // boundary. The structural boundary treats `-` as part of the word.
    let text = "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        x := 0\n    END\n    EVENT end-update\n    THEN\n        x := 1\n    END\nEND\n";
    assert!(rossi::parse(text).is_ok(), "fixture must be strictly valid");

    let tokens = decode_tokens(text);

    // `EVENT end-update` is line 8; the name starts at char 10 (`    EVENT `).
    // No keyword token may land in the name region (char >= 10); a keyword on
    // the leading `EVENT` (char 4) would be fine.
    let keyword = token_type_index("keyword");
    let in_name: Vec<_> = tokens
        .iter()
        .filter(|t| t.3 == keyword && t.0 == 8 && t.1 >= 10)
        .collect();
    assert!(
        in_name.is_empty(),
        "no keyword token may land inside the hyphenated name `end-update`, got {in_name:?}"
    );
}

#[test]
fn test_token_columns_are_chars_not_bytes() {
    // `∈` and `ℕ` are 3 UTF-8 bytes but 1 character (and 1 UTF-16 unit)
    // each; a comment after them must be positioned and sized in characters,
    // or clients paint it shifted right with an inflated length.
    let text = "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ∈ ℕ // typing: ℕ\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        x := 0\n    END\nEND\n";

    let tokens = decode_tokens(text);

    let comment_line = "    @inv1 x ∈ ℕ // typing: ℕ";
    let expected_col = comment_line.chars().position(|c| c == '/').unwrap() as u32;
    let expected_len = "// typing: ℕ".chars().count() as u32;
    let comment = token_type_index("comment");
    let comments: Vec<_> = tokens.iter().filter(|t| t.3 == comment).collect();
    assert_eq!(
        comments,
        [&(4, expected_col, expected_len, comment)],
        "comment token must use char-based column and length"
    );
}

#[test]
fn test_comment_markers_inside_label_do_not_split_tokens() {
    // `label_text` is atomic: `@inv//1` is one label, so the label token
    // must cover it whole and no COMMENT token may overlap it.
    let text = "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv//1 x >= 0\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        x := 0\n    END\nEND\n";
    assert!(rossi::parse(text).is_ok(), "fixture must be strictly valid");

    let tokens = decode_tokens(text);

    let label = token_type_index("macro");
    let labels: Vec<_> = tokens.iter().filter(|t| t.3 == label).collect();
    assert_eq!(
        labels,
        [&(4, 5, 6, label)],
        "the whole label `inv//1` (sans @) must carry one label token"
    );
    let comment = token_type_index("comment");
    assert!(
        !tokens.iter().any(|t| t.3 == comment),
        "no comment token may be carved out of a label, got {tokens:?}"
    );
}

#[test]
fn test_quote_in_label_does_not_hide_real_comment() {
    // A Rodin-imported label like `SAF5"` must not open string mode and
    // swallow the real trailing comment (grammar: any non-whitespace after
    // `@` is label text).
    let text = "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @SAF5\" x >= 0 // note: real\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        x := 0\n    END\nEND\n";
    assert!(rossi::parse(text).is_ok(), "fixture must be strictly valid");

    let tokens = decode_tokens(text);

    let comment = token_type_index("comment");
    let comments: Vec<_> = tokens.iter().filter(|t| t.3 == comment).collect();
    let expected_col = "    @SAF5\" x >= 0 ".chars().count() as u32;
    assert_eq!(
        comments,
        [&(4, expected_col, "// note: real".len() as u32, comment)],
        "the real comment must get its token despite the quote in the label"
    );
}

#[test]
fn test_multiline_string_content_is_not_a_comment() {
    // The grammar's `string_inner` spans lines; `//` on a continuation line
    // is string content. A phantom comment here used to swallow real code.
    let text = "CONTEXT c\nCONSTANTS\n    s\nAXIOMS\n    @axm1 s = \"a\n// not: a comment\"\nEND\n";
    assert!(rossi::parse(text).is_ok(), "fixture must be strictly valid");

    let tokens = decode_tokens(text);

    let comment = token_type_index("comment");
    assert!(
        !tokens.iter().any(|t| t.3 == comment),
        "string content must not be tokenized as comment, got {tokens:?}"
    );
    let keyword = token_type_index("keyword");
    let last_line = text.lines().count() as u32 - 1;
    assert!(
        tokens.iter().any(|t| t.3 == keyword && t.0 == last_line),
        "the END keyword after the string must keep its token, got {tokens:?}"
    );
}

#[test]
fn test_broken_document_keeps_comment_and_keyword_tokens() {
    // Mid-edit documents go through error recovery; highlighting (including
    // the issue-#24 comment tokens) must not vanish on every keystroke.
    let text = "MACHINE m\nVARIABLES\n    x\n    +\nINVARIANTS\n    // inv1: stays natural\n    @inv1 x >= 0\nEND\n";
    assert!(rossi::parse(text).is_err(), "fixture must be broken");

    let tokens = decode_tokens(text);

    let keyword = token_type_index("keyword");
    assert!(
        tokens.iter().any(|t| t.3 == keyword && t.0 == 0),
        "MACHINE must keep its keyword token on a broken document, got {tokens:?}"
    );
    let comment = token_type_index("comment");
    assert!(
        tokens.iter().any(|t| t.3 == comment && t.0 == 5),
        "the comment must keep its token on a broken document, got {tokens:?}"
    );
}
