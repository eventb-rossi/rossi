//! Iterative nesting-depth pre-scan guarding the recursive parser.
//!
//! Both the pest-generated parser and the AST builder recurse on nested
//! formula constructs, so a small adversarial input (hundreds of nested
//! parentheses, `¬` chains, quantifier chains) can overflow the stack and
//! abort the whole process — fatal for the language server, which parses
//! every `.eventb` file under the workspace root. `check_nesting` scans the
//! raw text iteratively *before* pest runs and rejects inputs whose
//! worst-case parser recursion depth exceeds [`MAX_NESTING_DEPTH`], turning a
//! process abort into an ordinary [`ParseError`].
//!
//! The scan is sound (it never under-counts the parser's real recursion
//! depth) but conservative: it may over-count, which can only reject
//! pathological inputs far beyond anything real models contain.
//!
//! A *pre*-scan is needed because the overflowing recursion lives in pest's
//! generated code, which exposes no depth hook: released pest (2.8.x) only
//! has `pest::set_call_limit`, a global total-call bound whose counter never
//! decreases, so it cannot tell deep input from large input. Upstream tracks
//! real depth limiting as pest-parser/pest#1129 (open PR
//! pest-parser/pest#1140, which would *repurpose* `set_call_limit` to count
//! depth). If that ships it can back this scan up in depth, but the scan
//! stays the source of the located, limit-stating [`ParseError`] diagnostic.
//!
//! The recursion drivers mirror grammar.pest:
//! - brackets `(` `[` `{` (parenthesized expressions/predicates, relational
//!   image, set enumeration/comprehension, function application, `bool(…)`),
//! - prefix/binder tokens that nest without brackets: `¬`/`not`, `∀`/`!`,
//!   `∃`/`#`, `λ`/`%`, `⋃`/`UNION`, `⋂`/`INTER`, `ℙ`/`POW`/`ℙ1`/`POW1`,
//!   `dom`, `ran`, `if` (if-then-else), and unary minus `-`/`−`.
//!
//! Infix operator chains (`∧`, `+`, `↦`, …) are parsed iteratively by both
//! pest (`(op ~ rhs)*` repetitions) and the AST builder, so they are
//! deliberately not counted.

use crate::ast::Span;
use crate::error::ParseError;

/// Maximum combined nesting depth (bracket depth + prefix-operator chain
/// length) a formula may have before parsing is refused.
///
/// Real Event-B models — including machine-generated ones — stay well under
/// ~50; 256 leaves generous headroom while staying cheap to guarantee
/// stack-wise (see `PARSER_STACK_SIZE`).
pub const MAX_NESTING_DEPTH: usize = 256;

/// Per-nesting-level stack budget used to size the `stacker::maybe_grow` red
/// zone at the parser entry points. The measured worst case is ~86 KB/level
/// (expression parens, debug build); 128 KB gives a 1.5× margin.
const PER_LEVEL_STACK_BUDGET: usize = 128 * 1024;

/// Flat stack budget for everything outside the per-level recursion (entry
/// frames, pest bookkeeping, shallow formulas).
const BASE_STACK_BUDGET: usize = 4 * 1024 * 1024;

/// Red zone for `stacker::maybe_grow`, proportional to the nesting depth
/// [`check_nesting`] measured for this input: a new stack segment is
/// allocated only when less than this remains. Scaling by depth means small
/// formulas (the per-XML-attribute import path, per-keystroke LSP parses)
/// skip the segment mmap entirely on ordinary stacks, while at-limit input
/// (256 × 128 KB + base = 36 MB) still always grows.
pub(crate) fn parser_stack_red_zone(depth: usize) -> usize {
    BASE_STACK_BUDGET + depth * PER_LEVEL_STACK_BUDGET
}

/// Size of the stack segment `stacker::maybe_grow` allocates when the red
/// zone is hit (~3× the measured worst case at [`MAX_NESTING_DEPTH`]:
/// 256 levels × ~86 KB/level ≈ 21.5 MB in debug builds).
pub(crate) const PARSER_STACK_SIZE: usize = 64 * 1024 * 1024;

/// Bytes that end an operand (identifier, literal, closing bracket, postfix
/// inverse). A `-` directly following one of these is a *binary* minus, which
/// pest parses iteratively, so it must not be counted as a recursion driver.
/// Anything not listed here conservatively makes a following `-` count as
/// unary (over-counting is sound, under-counting is not). String literals
/// don't appear here: the `b'"'` match arm sets `after_operand` itself.
fn is_ascii_operand_end(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'\'' | b')' | b']' | b'}' | b'~')
}

/// Unicode counterpart of [`is_ascii_operand_end`] for the common carrier
/// sets and postfix inverse, so formulas like `ℤ ∖ … − x` don't accumulate
/// spurious unary-minus counts.
fn is_unicode_operand_end(c: char) -> bool {
    matches!(c, 'ℤ' | 'ℕ' | '∅' | '∼')
}

/// Case-sensitive keyword forms of prefix/unary operators (grammar.pest:
/// `op_not`, `op_domain`, `op_range`, `op_powerset`, `op_powerset1`).
fn is_prefix_word(word: &str) -> bool {
    matches!(word, "not" | "dom" | "ran" | "POW" | "POW1")
        // Case-insensitive keywords: `kw_UNION`, `kw_INTER`, `kw_if`.
        || word.eq_ignore_ascii_case("union")
        || word.eq_ignore_ascii_case("inter")
        || word.eq_ignore_ascii_case("if")
}

/// Scan `input` and reject it if its worst-case parser recursion depth
/// exceeds [`MAX_NESTING_DEPTH`]. On success, return the maximum depth
/// metric observed, which sizes the stack red zone for the parse that
/// follows ([`parser_stack_red_zone`]).
///
/// Single forward pass, no recursion; the only allocation is the
/// bracket-snapshot stack. Comments, string literals, and `@label` text are
/// skipped exactly as the grammar lexes them, so brackets inside them don't
/// count.
pub(crate) fn check_nesting(input: &str) -> Result<usize, ParseError> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    // Open-bracket depth so far.
    let mut bracket_depth: usize = 0;
    // Prefix/binder operators seen in the current formula at the current
    // bracket level. Reset at `@` labels (no grammar rule other than `label`
    // consumes `@`, so parser recursion never persists across one) and
    // restored at closing brackets (a prefix operator opened inside `(…)` has
    // unwound by the time `)` is consumed).
    let mut prefix_count: usize = 0;
    // `prefix_count` snapshots taken at opening brackets.
    let mut snapshots: Vec<usize> = Vec::new();
    // Whether the previous significant token ended an operand (drives the
    // unary-vs-binary minus distinction).
    let mut after_operand = false;
    // Largest depth metric seen so far (the function's return value).
    let mut max_metric: usize = 0;

    let too_deep = |offset: usize| -> ParseError {
        let (line, column) = Span {
            start: offset,
            end: offset,
        }
        .to_line_col(input);
        ParseError::NestingTooDeep {
            limit: MAX_NESTING_DEPTH,
            line: line + 1,
            column: column + 1,
        }
    };

    while i < len {
        let b = bytes[i];
        match b {
            b' ' | b'\t' | b'\r' | b'\n' => {
                i += 1;
            }
            b'/' => {
                if bytes.get(i + 1) == Some(&b'*') {
                    // Block comment; if unterminated, pest lexes `/` `*` as
                    // ordinary tokens, so we fall through and keep counting.
                    match input[i + 2..].find("*/") {
                        Some(end) => i += 2 + end + 2,
                        None => {
                            after_operand = false;
                            i += 1;
                        }
                    }
                } else if bytes.get(i + 1) == Some(&b'/') {
                    // Line comment runs to end of line (or EOF).
                    match input[i + 2..].find('\n') {
                        Some(end) => i += 2 + end + 1,
                        None => i = len,
                    }
                } else {
                    // Division or an ASCII op like `/:` — not a driver.
                    after_operand = false;
                    i += 1;
                }
            }
            b'"' => {
                // String literal with `\"` and `\\` escapes; if unterminated,
                // treat the quote as an ordinary char and keep counting.
                let mut j = i + 1;
                let close = loop {
                    match bytes.get(j) {
                        None => break None,
                        Some(b'\\') => j += 2,
                        Some(b'"') => break Some(j),
                        Some(_) => j += 1,
                    }
                };
                match close {
                    Some(end) => {
                        i = end + 1;
                        after_operand = true;
                    }
                    None => {
                        after_operand = false;
                        i += 1;
                    }
                }
            }
            b'@' => {
                // Label: everything up to the next whitespace belongs to it
                // (may legally contain brackets, `!`, …). A label also starts
                // a fresh formula, so the prefix chain resets.
                i += 1;
                while i < len && !matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n') {
                    i += 1;
                }
                prefix_count = 0;
                after_operand = false;
            }
            b'(' | b'[' | b'{' => {
                snapshots.push(prefix_count);
                bracket_depth += 1;
                max_metric = max_metric.max(bracket_depth + prefix_count);
                if max_metric > MAX_NESTING_DEPTH {
                    return Err(too_deep(i));
                }
                after_operand = false;
                i += 1;
            }
            b')' | b']' | b'}' => {
                bracket_depth = bracket_depth.saturating_sub(1);
                if let Some(saved) = snapshots.pop() {
                    prefix_count = saved;
                }
                after_operand = true;
                i += 1;
            }
            b'!' | b'#' | b'%' => {
                // ASCII forms of ∀ / ∃ / λ.
                prefix_count += 1;
                max_metric = max_metric.max(bracket_depth + prefix_count);
                if max_metric > MAX_NESTING_DEPTH {
                    return Err(too_deep(i));
                }
                after_operand = false;
                i += 1;
            }
            b'-' => {
                let (next, contribution) = scan_minus_run(input, i, after_operand);
                prefix_count += contribution;
                max_metric = max_metric.max(bracket_depth + prefix_count);
                if max_metric > MAX_NESTING_DEPTH {
                    return Err(too_deep(i));
                }
                after_operand = false;
                i = next;
            }
            _ if b.is_ascii_alphabetic() || b == b'_' => {
                let start = i;
                while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                if is_prefix_word(&input[start..i]) {
                    prefix_count += 1;
                    max_metric = max_metric.max(bracket_depth + prefix_count);
                    if max_metric > MAX_NESTING_DEPTH {
                        return Err(too_deep(start));
                    }
                    after_operand = false;
                } else {
                    after_operand = true;
                }
            }
            _ if b.is_ascii() => {
                after_operand = is_ascii_operand_end(b);
                i += 1;
            }
            _ => {
                // Multi-byte char. Safe to unwrap: i is on a char boundary.
                let c = input[i..].chars().next().unwrap();
                match c {
                    '¬' | '∀' | '∃' | 'λ' | '⋃' | '⋂' | 'ℙ' => {
                        prefix_count += 1;
                        max_metric = max_metric.max(bracket_depth + prefix_count);
                        if max_metric > MAX_NESTING_DEPTH {
                            return Err(too_deep(i));
                        }
                        after_operand = false;
                        i += c.len_utf8();
                    }
                    '−' => {
                        let (next, contribution) = scan_minus_run(input, i, after_operand);
                        prefix_count += contribution;
                        max_metric = max_metric.max(bracket_depth + prefix_count);
                        if max_metric > MAX_NESTING_DEPTH {
                            return Err(too_deep(i));
                        }
                        after_operand = false;
                        i = next;
                    }
                    _ => {
                        after_operand = is_unicode_operand_end(c);
                        i += c.len_utf8();
                    }
                }
            }
        }
    }

    Ok(max_metric)
}

/// Count a maximal run of minus signs (`-` and `−` mixed) starting at `start`.
///
/// A run of `k` minuses contributes `k` potential unary recursions, minus one
/// if the run directly follows an operand (the first minus is then a binary
/// `additive_expr` operator, parsed iteratively), minus one if the run is
/// directly followed by `>` (the trailing `-` belongs to an ASCII arrow such
/// as `-->`, `+->`, `|->`). Returns `(index just past the run, contribution)`.
fn scan_minus_run(input: &str, start: usize, after_operand: bool) -> (usize, usize) {
    let bytes = input.as_bytes();
    let mut i = start;
    let mut run = 0usize;
    loop {
        if bytes.get(i) == Some(&b'-') {
            run += 1;
            i += 1;
        } else if input[i..].starts_with('−') {
            run += 1;
            i += '−'.len_utf8();
        } else {
            break;
        }
    }
    let mut contribution = run;
    if after_operand {
        contribution = contribution.saturating_sub(1);
    }
    if bytes.get(i) == Some(&b'>') {
        contribution = contribution.saturating_sub(1);
    }
    (i, contribution)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn depth_err(input: &str) -> bool {
        matches!(check_nesting(input), Err(ParseError::NestingTooDeep { .. }))
    }

    #[test]
    fn shallow_input_passes() {
        assert!(check_nesting("context C axioms @a ((x + 1)) = 1 end").is_ok());
    }

    #[test]
    fn deep_parens_rejected() {
        let n = MAX_NESTING_DEPTH + 1;
        let s = format!("{}x{}", "(".repeat(n), ")".repeat(n));
        assert!(depth_err(&s));
    }

    #[test]
    fn at_limit_parens_pass() {
        let n = MAX_NESTING_DEPTH;
        let s = format!("{}x{}", "(".repeat(n), ")".repeat(n));
        assert!(check_nesting(&s).is_ok());
    }

    #[test]
    fn deep_negation_rejected() {
        assert!(depth_err(&format!(
            "{}(1=1)",
            "¬".repeat(MAX_NESTING_DEPTH + 1)
        )));
        assert!(depth_err(&format!(
            "{}1=1",
            "not ".repeat(MAX_NESTING_DEPTH + 1)
        )));
    }

    #[test]
    fn deep_quantifiers_rejected() {
        let s: String = (0..MAX_NESTING_DEPTH + 1)
            .map(|k| format!("∀ x{k} · "))
            .collect();
        assert!(depth_err(&format!("{s}1=1")));
    }

    #[test]
    fn parens_in_comments_strings_labels_ignored() {
        let deep = "(".repeat(MAX_NESTING_DEPTH * 2);
        assert!(check_nesting(&format!("/* {deep} */ x = 1")).is_ok());
        assert!(check_nesting(&format!("// {deep}\nx = 1")).is_ok());
        assert!(check_nesting(&format!("\"{deep}\" = s")).is_ok());
        assert!(check_nesting(&format!("@lbl{deep} x = 1")).is_ok());
    }

    #[test]
    fn unterminated_comment_or_string_still_counts() {
        let deep = "(".repeat(MAX_NESTING_DEPTH + 1);
        assert!(depth_err(&format!("/* {deep}")));
        assert!(depth_err(&format!("\"{deep}")));
    }

    #[test]
    fn unterminated_lexemes_before_multibyte_chars_are_boundary_safe() {
        // The unterminated-comment/string fallbacks resume scanning one byte
        // after the introducer; the next char may be multi-byte.
        assert!(check_nesting("\"¬").is_ok());
        assert!(check_nesting("/*¬").is_ok());
        assert!(check_nesting("\"é\\").is_ok());
        assert!(depth_err(&format!(
            "\"∀{}",
            "(".repeat(MAX_NESTING_DEPTH + 1)
        )));
    }

    #[test]
    fn returns_max_metric() {
        assert_eq!(check_nesting("x = 1").unwrap(), 0);
        assert_eq!(check_nesting("((x)) = 1").unwrap(), 2);
        assert_eq!(check_nesting("¬(x = 1)").unwrap(), 2);
        // Metric is the running max, not the final state.
        assert_eq!(check_nesting("(((x))) ∧ (y)").unwrap(), 3);
    }

    #[test]
    fn label_resets_prefix_chain() {
        // Many small formulas, each individually shallow.
        let s: String = (0..2000).map(|k| format!("@a{k} ¬(x{k} = 1)\n")).collect();
        assert!(check_nesting(&s).is_ok());
    }

    #[test]
    fn brackets_restore_prefix_chain() {
        // (¬a) ∧ (¬b) ∧ … — prefix operators inside brackets unwind.
        let s = format!("@a {}1=1", "(¬x) ∧ ".repeat(MAX_NESTING_DEPTH * 2));
        assert!(check_nesting(&s).is_ok());
    }

    #[test]
    fn ascii_arrows_and_binary_minus_not_counted() {
        let arrows = "f : A --> B ∧ ".repeat(MAX_NESTING_DEPTH * 2);
        assert!(check_nesting(&format!("@a {arrows} 1=1")).is_ok());
        let minuses = "a - ".repeat(MAX_NESTING_DEPTH * 2);
        assert!(check_nesting(&format!("@a x = {minuses}b")).is_ok());
        let maplets = "1 ↦ 2, ".repeat(MAX_NESTING_DEPTH * 2);
        assert!(check_nesting(&format!("@a s = {{{maplets}3 ↦ 4}}")).is_ok());
    }

    #[test]
    fn unary_minus_chains_counted() {
        assert!(depth_err(&format!(
            "x = {}y",
            "-".repeat(MAX_NESTING_DEPTH * 2)
        )));
        assert!(depth_err(&format!(
            "x = {}y",
            "−".repeat(MAX_NESTING_DEPTH * 2)
        )));
    }

    #[test]
    fn prefix_words_counted_identifiers_not() {
        assert!(depth_err(&format!(
            "x = {}y",
            "dom ".repeat(MAX_NESTING_DEPTH + 1)
        )));
        // Identifiers merely containing keyword substrings are not drivers.
        let idents = "random domain pownot ".repeat(MAX_NESTING_DEPTH);
        assert!(check_nesting(&format!("@a {idents} = 1")).is_ok());
    }

    #[test]
    fn multibyte_input_is_safe() {
        assert!(check_nesting("@inv1 ∀x·x∈ℕ ⇒ x ≥ 0 ∧ é“λ”ℙ(S) ≠ ∅").is_ok());
    }

    #[test]
    fn error_reports_position() {
        let s = format!("x = 1\n{}y", "(".repeat(MAX_NESTING_DEPTH + 1));
        match check_nesting(&s) {
            Err(ParseError::NestingTooDeep { limit, line, .. }) => {
                assert_eq!(limit, MAX_NESTING_DEPTH);
                assert_eq!(line, 2);
            }
            other => panic!("expected NestingTooDeep, got {other:?}"),
        }
    }
}
