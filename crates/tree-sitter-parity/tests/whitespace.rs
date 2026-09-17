//! What each parser accepts as the separator between two declared names.
//!
//! `corpus.rs` and `fuzz_inputs.rs` cannot see this. The corpus gate feeds
//! tree-sitter Rossi's own pretty-printed output, and the fuzzer's layout pass
//! emits only `' '` and `'\n'` — so neither ever produces an exotic separator,
//! and the one direction where Rossi is the more permissive parser goes
//! unmeasured. These probes measure it directly.
//!
//! Three lexers, three answers, and the verdicts below were each probed against
//! `eventb-checker` (Camille's `eventbstruct` grammar for structure, Rodin's
//! real `FormulaFactory` for formulas):
//!
//! - **Rossi** mirrors Rodin's math lexer: `LexicalClass.isWhitespace` plus
//!   `FormulaFactory.isEventBWhiteSpace`, i.e. every Zs/Zl/Zp separator with
//!   U+0009..U+000D and U+001C..U+001F.
//! - **Camille** uses `layout_char = [0..32] + [127..160] + [8206..8207] +
//!   [8232..8233]` (`EventBParser.scc`) — it takes U+0085 and U+00A0, and
//!   answers "Unknown token" from U+1680 up. `rossi validate` reports that gap
//!   as EB031.
//! - **tree-sitter** once wrote `extras: [/\s/]`, which its generated
//!   `parser.c` compiled to `('\t' <= c && c <= '\r') || c == ' '` — 6 of the
//!   28 Rossi separates on. `grammar.js` now spells that class out and gives
//!   `label` its complement, `src/parser.c` is regenerated, and the submodule
//!   points at that commit.
//!
//! The tree-sitter column therefore asserts rather than reports. Both grammars
//! name the same set now, and holding them to it across a regeneration is the
//! only thing that keeps them there. Rossi's own column is asserted
//! in-workspace, where CI actually runs it, by
//! `rossi::tests::simple_predicate_test`; the EB031 column is asserted per row
//! below.

mod common;

/// `(separator, name, camille_accepts)` for every code point worth probing.
///
/// The Camille column is measured, not derived: each probe was run through
/// `eventb-checker check --format json`, where a rejection reads
/// `EB004 Camille parse error: [2,12] Unknown token`.
const SEPARATORS: &[(char, &str, bool)] = &[
    ('\u{0020}', "SPACE", true),
    ('\u{0009}', "TAB", true),
    ('\u{000B}', "VERTICAL TAB", true),
    ('\u{000C}', "FORM FEED", true),
    ('\u{001C}', "FILE SEPARATOR", true),
    ('\u{001F}', "UNIT SEPARATOR", true),
    ('\u{0085}', "NEXT LINE", true),
    ('\u{00A0}', "NO-BREAK SPACE", true),
    ('\u{1680}', "OGHAM SPACE MARK", false),
    ('\u{2000}', "EN QUAD", false),
    ('\u{2007}', "FIGURE SPACE", false),
    ('\u{2028}', "LINE SEPARATOR", true),
    ('\u{2029}', "PARAGRAPH SEPARATOR", true),
    ('\u{202F}', "NARROW NO-BREAK SPACE", false),
    ('\u{205F}', "MEDIUM MATHEMATICAL SPACE", false),
    ('\u{3000}', "IDEOGRAPHIC SPACE", false),
    ('\u{200B}', "ZERO WIDTH SPACE", false),
];

/// A minimal context whose `CONSTANTS` list is separated by `separator`.
///
/// A structural position on purpose: this is Camille's `normal` lexer state,
/// the one where an unknown code point is fatal. Inside a formula the same
/// character is folded into the formula token and handed to Rodin, which reads
/// it as whitespace — portable, and a different question.
fn probe(separator: char) -> String {
    format!("context C\nconstants a{separator}b\naxioms\n  @a1 a = 1\n  @a2 b = 2\nend\n")
}

#[test]
fn report_whitespace_verdicts() {
    let mut parser = common::eventb_parser();
    let mut divergent: Vec<String> = Vec::new();

    for &(separator, name, camille) in SEPARATORS {
        let source = probe(separator);
        let rossi = common::without_panicking(|| rossi::parse_components(&source).is_ok())
            .unwrap_or(false);
        let tree_sitter = common::tree_sitter_accepts(&mut parser, &source);
        // The gap between the rossi and Camille columns is what EB031 reports;
        // the set identity behind it is asserted in `rossi`'s own tests, which
        // CI runs. Checking it here too keeps the printed table honest.
        let eb031 = rossi::keywords::camille_unreadable_separator(separator);
        assert_eq!(
            eb031,
            rossi && !camille,
            "U+{:04X}: EB031 must mark exactly the separators rossi reads and Camille does not",
            separator as u32
        );

        if rossi != tree_sitter {
            divergent.push(format!("U+{:04X} {name}", separator as u32));
        }
        println!(
            "  U+{:04X} {name:<26} rossi={:<6} camille={:<6} tree-sitter={:<6}{}",
            separator as u32,
            if rossi { "accept" } else { "REJECT" },
            if camille { "accept" } else { "REJECT" },
            if tree_sitter { "accept" } else { "REJECT" },
            if eb031 { "  <- EB031" } else { "" },
        );
    }

    println!(
        "{} of {} separators divide rossi and tree-sitter",
        divergent.len(),
        SEPARATORS.len()
    );

    // After the table, so a failure still prints every row it was drawn from.
    assert!(
        divergent.is_empty(),
        "rossi and the tree-sitter grammar must separate on the same code \
         points; they part on {divergent:?}"
    );
}
