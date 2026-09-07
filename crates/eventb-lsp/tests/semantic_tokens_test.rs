//! Tests for semantic tokens provider

use eventb_lsp::lsp_types::{SemanticTokensParams, TextDocumentIdentifier, Url};
use eventb_lsp::semantic_tokens::SemanticTokensProvider;

mod common;
use common::{decode_tokens, decode_tokens_with_modifiers, slice_range};

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
        let parsed = rossi::parse_components_with_recovery(text);
        let result = provider.semantic_tokens(
            &params,
            text,
            parsed.component.as_deref().unwrap_or_default(),
        );
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

/// Position of `name` in a legend name list (token types or modifiers),
/// panicking if absent. Backs both `token_type_index` and `modifier_bit` so the
/// "find the legend slot by name" lookup lives in one place.
fn legend_index<'a>(names: impl IntoIterator<Item = &'a str>, name: &str) -> usize {
    names
        .into_iter()
        .position(|n| n == name)
        .unwrap_or_else(|| panic!("{name} not in legend"))
}

/// Index of `name` in the legend's token types — derived, so reordering the
/// legend cannot silently re-point these tests at the wrong token type.
fn token_type_index(name: &str) -> u32 {
    let legend = SemanticTokensProvider::legend();
    legend_index(legend.token_types.iter().map(|t| t.as_str()), name) as u32
}

/// Slice the token at `(line, col, len)` out of `text` (0-indexed,
/// char-based columns like the provider's).
fn comment_text(text: &str, line: u32, col: u32, len: u32) -> String {
    use eventb_lsp::lsp_types::{Position, Range};
    slice_range(
        text,
        Range::new(Position::new(line, col), Position::new(line, col + len)),
    )
}

#[test]
fn test_tokens_never_land_inside_comments() {
    // Each fixture's comment mentions the entity before its real occurrence
    // (issue #24): no token of the row's type may sit on the comment line, and
    // each real occurrence must carry it.
    struct Case {
        case: &'static str,
        text: &'static str,
        token_type: &'static str,
        comment_line: u32,
        real_positions: &'static [(u32, u32)],
    }
    let cases = [
        Case {
            case: "variable mentioned in a comment before its declaration",
            text: "MACHINE m\nVARIABLES\n    // state: x is the counter\n    x\nINVARIANTS\n    @inv1 x >= 0\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x := 0\n    END\nEND\n",
            token_type: "variable",
            comment_line: 2,
            real_positions: &[(3, 4)],
        },
        Case {
            case: "label mentioned in a comment before the real @inv1",
            text: "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    // inv1: x stays a natural\n    @inv1 x >= 0\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x := 0\n    END\nEND\n",
            token_type: "macro", // labels use the MACRO slot
            comment_line: 4,
            real_positions: &[(5, 5)],
        },
        Case {
            case: "AXIOMS and END spelled in a comment before the real keywords",
            text: "CONTEXT c\n// the AXIOMS: section and its END: marker\nAXIOMS\n    @axm1 1 = 1\nEND\n",
            token_type: "keyword",
            comment_line: 1,
            real_positions: &[(2, 0), (4, 0)],
        },
    ];

    for Case {
        case,
        text,
        token_type,
        comment_line,
        real_positions,
    } in cases
    {
        let tokens = decode_tokens(text);
        let ty = token_type_index(token_type);
        let of_type: Vec<_> = tokens.iter().filter(|t| t.3 == ty).collect();
        assert!(
            of_type.iter().all(|t| t.0 != comment_line),
            "{case}: no {token_type} token may sit on comment line {comment_line}, got {of_type:?}"
        );
        for &(line, col) in real_positions {
            assert!(
                of_type.iter().any(|t| t.0 == line && t.1 == col),
                "{case}: the real occurrence at ({line}, {col}) must carry the {token_type} token, got {of_type:?}"
            );
        }
    }
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
    let text = "MACHINE m\nVARIABLES\n    extended\nINVARIANTS\n    @inv1 extended >= 0\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 extended := 0\n    END\nEND\n";

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
    // Regression for issue #36: no keyword scan may latch onto the `end`
    // fragment of the hyphenated event name `end-update`. `find_keyword` uses
    // the structural word boundary (where `-` is part of a word), so the `end`
    // of `end-update` is never a whole keyword; the name itself is emitted from
    // its AST span rather than text-searched.
    let text = "MACHINE m\nVARIABLES\n    x\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x := 0\n    END\n    EVENT end-update\n    THEN\n        @act1 x := 1\n    END\nEND\n";
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
    let text = "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ∈ ℕ // typing: ℕ\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x := 0\n    END\nEND\n";

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
    let text = "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv//1 x >= 0\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x := 0\n    END\nEND\n";
    assert!(rossi::parse(text).is_ok(), "fixture must be strictly valid");

    let tokens = decode_tokens(text);

    let label = token_type_index("macro");
    // The invariant's line only: the action carries a label token of its own.
    let labels: Vec<_> = tokens.iter().filter(|t| t.3 == label && t.0 == 4).collect();
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
    let text = "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @SAF5\" x >= 0 // note: real\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act1 x := 0\n    END\nEND\n";
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

#[test]
fn test_labels_consistent_across_events_with_initialisation() {
    // A machine with an INITIALISATION event (the overwhelmingly common case)
    // followed by ordinary events. The old text-search tokenizer processed
    // INITIALISATION first, advanced its single forward cursor past it, then
    // searched *forward* for the EVENTS keyword — which sits *before*
    // INITIALISATION — so the search failed, the whole events loop was skipped,
    // and every `@grd1`/`@act1` in `step`/`step2` lost its label token (falling
    // back to the TextMate scope, a different color). Every `@`-label must now
    // carry the same MACRO token at its own position, regardless of the walk.
    let text = "MACHINE m\nVARIABLES\n    v\nINVARIANTS\n    @inv1 v >= 0\nEVENTS\n    EVENT INITIALISATION\n    THEN\n        @act0 v := 0\n    END\n    EVENT step\n    WHERE\n        @grd1 v > 0\n    THEN\n        @act1 v := v + 1\n    END\n    EVENT step2\n    WHERE\n        @grd1 v > 0\n    THEN\n        @act1 v := v + 1\n    END\nEND\n";
    assert!(rossi::parse(text).is_ok(), "fixture must be strictly valid");

    let tokens = decode_tokens(text);
    let label = token_type_index("macro"); // labels use the MACRO slot

    let label_at = |line: u32, col: u32, len: u32| {
        tokens
            .iter()
            .any(|&(l, c, n, t)| l == line && c == col && n == len && t == label)
    };
    // Label names (sans `@`) are 4 chars. Event bodies are indented 8 spaces, so
    // each name starts at column 9; the invariant `@inv1` at column 5.
    assert!(
        label_at(4, 5, 4),
        "@inv1 must be a label token, got {tokens:?}"
    );
    assert!(
        label_at(8, 9, 4),
        "@act0 (init) must be a label token, got {tokens:?}"
    );
    assert!(
        label_at(12, 9, 4),
        "step @grd1 must be a label token, got {tokens:?}"
    );
    assert!(
        label_at(14, 9, 4),
        "step @act1 must be a label token, got {tokens:?}"
    );
    assert!(
        label_at(18, 9, 4),
        "step2 @grd1 must be a label token, got {tokens:?}"
    );
    assert!(
        label_at(20, 9, 4),
        "step2 @act1 must be a label token, got {tokens:?}"
    );

    // Exactly six labels: @inv1 + @act0 + 2×@grd1 + 2×@act1. Before the fix the
    // events loop was skipped, so only @inv1 and @act0 survived (2 of 6) — a
    // crisp before/after discriminator.
    let macros: Vec<_> = tokens.iter().filter(|t| t.3 == label).collect();
    assert_eq!(
        macros.len(),
        6,
        "every @-label gets exactly one label token, got {macros:?}"
    );
}

#[test]
fn label_tokens_cover_complete_names_in_valid_and_broken_documents() {
    for name in ["axm1:", "a::", ":", "😀:", "a@b", "a//:", "SAF5\""] {
        for separator in [" ", "\t", "\u{1c}", "\u{a0}"] {
            for predicate in ["1 = 1", "1 ="] {
                let text = format!("CONTEXT c\nAXIOMS\n    @{name}{separator}{predicate}\nEND\n");
                assert_eq!(rossi::parse(&text).is_ok(), predicate == "1 = 1");
                let tokens = decode_tokens(&text);
                let label = token_type_index("macro");
                let labels: Vec<_> = tokens.iter().filter(|t| t.3 == label).collect();
                assert_eq!(
                    labels,
                    [&(2, 5, name.encode_utf16().count() as u32, label)],
                    "{text}"
                );
            }
        }
    }
}

#[test]
fn event_clause_keywords_coloured_when_guard_parse_fails() {
    // A broken guard (`∖` missing → `Union CurrUnion`) causes error recovery to
    // leave event.guards empty.  WHERE and THEN must still receive keyword
    // tokens so the event body doesn't appear completely unhighlighted.
    let text = "MACHINE m\nVARIABLES\n    v\nEVENTS\n    EVENT step\n    WHERE\n        @grd1 v ∈ ℕ  ℕ\n    THEN\n        @act1 v := 0\n    END\nEND\n";
    assert!(rossi::parse(text).is_err(), "fixture must be broken");

    let tokens = decode_tokens(text);
    let keyword = token_type_index("keyword");
    let lines: Vec<&str> = text.lines().collect();

    let kw_at = |line: u32, text_slice: &str| {
        let col = lines[line as usize].find(text_slice).unwrap() as u32;
        tokens
            .iter()
            .any(|&(l, c, _, t)| l == line && c == col && t == keyword)
    };

    assert!(
        kw_at(5, "WHERE"),
        "WHERE must get a keyword token despite failed guard"
    );
    assert!(
        kw_at(7, "THEN"),
        "THEN must get a keyword token despite failed guard"
    );
    assert!(
        kw_at(9, "END"),
        "END must get a keyword token despite failed guard"
    );
}

#[test]
fn formula_body_identifiers_are_classified() {
    // Per-identifier spans let semantic tokens colour identifiers inside formula
    // bodies, not just declarations: a variable used in an invariant is a
    // VARIABLE token, and a quantifier binder with its bound use are PARAMETER
    // tokens (this machine has no event parameters, so any PARAMETER token can
    // only come from the formula walk).
    let text =
        "MACHINE m\nVARIABLES\ncount\nINVARIANTS\n@inv1 count >= 0\n@inv2 ∀ q · q ∈ ℕ\nEND\n";
    assert!(rossi::parse(text).is_ok(), "fixture must be strictly valid");

    let tokens = decode_tokens(text);
    let variable = token_type_index("variable");
    let parameter = token_type_index("parameter");

    // The `count` declaration plus its use in @inv1.
    let variables = tokens.iter().filter(|t| t.3 == variable).count();
    assert!(
        variables >= 2,
        "expected the count declaration and its body use: {tokens:?}"
    );

    // The quantifier binder `q` and its bound use in @inv2.
    let parameters = tokens.iter().filter(|t| t.3 == parameter).count();
    assert_eq!(parameters, 2, "binder q and its bound use: {tokens:?}");
}

#[test]
fn all_any_parameters_coloured_when_guard_parse_fails() {
    // A broken guard forces recovery. Every whitespace-separated ANY parameter
    // declaration must still get a PARAMETER token — not just the first, and not
    // only the ones named in a still-valid guard. `roleName` appears in no guard,
    // so a token on it can only come from the parameter-declaration path.
    let text = "MACHINE m\nVARIABLES\n    v\nEVENTS\n    EVENT step\n    ANY\n        user\n        subject\n        roleName\n    WHERE\n        @grd1 user ∈ S :\n        @grd2 subject ∈ T\n    THEN\n        @act1 v ≔ 0\n    END\nEND\n";
    assert!(rossi::parse(text).is_err(), "fixture must be broken");

    let tokens = decode_tokens(text);
    let parameter = token_type_index("parameter");
    let lines: Vec<&str> = text.lines().collect();

    let param_decl_at = |line: u32, name: &str| {
        let col = lines[line as usize].find(name).unwrap() as u32;
        tokens
            .iter()
            .any(|&(l, c, _, t)| l == line && c == col && t == parameter)
    };

    for (line, name) in [(6, "user"), (7, "subject"), (8, "roleName")] {
        assert!(
            param_decl_at(line, name),
            "ANY parameter `{name}` (line {line}) must get a PARAMETER token despite the failed guard: {tokens:?}"
        );
    }
}

/// Bit mask for modifier `name` in the legend (`1 << its index`); like
/// [`token_type_index`], derived so a legend reorder cannot mis-point the test.
fn modifier_bit(name: &str) -> u32 {
    let legend = SemanticTokensProvider::legend();
    1 << legend_index(legend.token_modifiers.iter().map(|m| m.as_str()), name)
}

#[test]
fn constants_are_read_only_variables_not_numbers() {
    // A constant is an immutable binding: a VARIABLE carrying the read-only
    // modifier — distinct from a number literal (its former colour) and from a
    // mutable variable (which has no read-only modifier).
    let types: Vec<String> = SemanticTokensProvider::legend()
        .token_types
        .iter()
        .map(|t| t.as_str().to_string())
        .collect();
    assert!(
        !types.contains(&"number".to_string()),
        "constants no longer map to the number token type: {types:?}"
    );

    let variable = token_type_index("variable");
    let type_ = token_type_index("type");
    let readonly = modifier_bit("readonly");

    // Context: a set `S` (read-only TYPE) and a constant `k` (read-only VARIABLE),
    // each declared then used in the axiom. A context has no mutable variables,
    // so every variable/type token here must carry the read-only modifier.
    let ctx = "CONTEXT c\nSETS S\nCONSTANTS k\nAXIOMS\n@axm1 k ∈ S\nEND\n";
    assert!(rossi::parse(ctx).is_ok(), "fixture must be strictly valid");
    let ctx_tokens = decode_tokens_with_modifiers(ctx);
    assert!(
        ctx_tokens.iter().any(|&(_, _, _, t, _)| t == variable),
        "constant `k` should produce a VARIABLE token: {ctx_tokens:?}"
    );
    // The read-only loop below is vacuous unless a TYPE token actually exists, so
    // pin its presence too (otherwise a SETS-colouring regression would pass).
    assert!(
        ctx_tokens.iter().any(|&(_, _, _, t, _)| t == type_),
        "set `S` should produce a TYPE token: {ctx_tokens:?}"
    );
    for &(_, _, _, t, mods) in &ctx_tokens {
        if t == variable || t == type_ {
            assert_ne!(
                mods & readonly,
                0,
                "set/constant token must be read-only: {ctx_tokens:?}"
            );
        }
    }

    // Machine: a VARIABLE `v` is mutable, so neither its declaration nor its use
    // may carry the read-only modifier.
    let mch = "MACHINE m\nVARIABLES\nv\nINVARIANTS\n@inv1 v ∈ ℕ\nEND\n";
    assert!(rossi::parse(mch).is_ok(), "fixture must be strictly valid");
    let mch_tokens = decode_tokens_with_modifiers(mch);
    let variables: Vec<_> = mch_tokens
        .iter()
        .filter(|&&(_, _, _, t, _)| t == variable)
        .collect();
    assert!(
        !variables.is_empty(),
        "variable `v` should produce VARIABLE tokens: {mch_tokens:?}"
    );
    for &&(_, _, _, _, mods) in &variables {
        assert_eq!(
            mods & readonly,
            0,
            "mutable variable token must not be read-only: {mch_tokens:?}"
        );
    }
}

/// An action's lowering can put two AST nodes on one piece of source — the
/// target and the synthesized right-hand `f` of `f(x) ≔ E`, the write target
/// and the primed declaration of `x :∣ P` — and each carries the span of the
/// identifier that is actually written. The encoded stream must still hold one
/// token per position: overlapping tokens are outside the protocol, and here
/// the two would disagree about the token type.
#[test]
fn lowered_actions_emit_one_token_per_position() {
    for text in [
        "MACHINE m\nVARIABLES\n    f\nINVARIANTS\n    @inv1 f ∈ ℕ → ℕ\nEVENTS\n    EVENT e\n    THEN\n        @act1 f(0) ≔ 1\n    END\nEND\n",
        "MACHINE m\nVARIABLES\n    x\nINVARIANTS\n    @inv1 x ∈ ℕ\nEVENTS\n    EVENT e\n    THEN\n        @act1 x :∣ x' = x + 1\n    END\nEND\n",
    ] {
        let tokens = decode_tokens(text);
        let mut positions: Vec<_> = tokens.iter().map(|&(line, col, ..)| (line, col)).collect();
        positions.dedup();
        assert_eq!(
            positions.len(),
            tokens.len(),
            "two tokens share a position in {text:?}: {tokens:?}"
        );
    }
}
