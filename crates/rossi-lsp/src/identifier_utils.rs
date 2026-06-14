//! Shared helpers for locating identifiers in source text by cursor position.
//!
//! Several LSP providers (definition, hover, references) need "what identifier
//! is under the cursor?". Keep that logic in one place so the three variants
//! can't drift apart.

use crate::lsp_types::{Location, Position, Range, Url};
use crate::position::{line_run_to_range, utf16_to_char_col};
use crate::text_utils;

/// Return the identifier that contains `position`, together with its range.
///
/// An identifier is a maximal run of `text_utils::is_identifier_char` characters
/// (alphanumeric plus `_`). In structural-name positions (after
/// `MACHINE`/`CONTEXT`/`EVENT`/`REFINES`/`SEES`/`EXTENDS`) the run extends
/// across `-` joins, so a cursor on `ENV_C-1` in a SEES clause resolves the
/// whole component name instead of a fragment (issue #28); in formula lines
/// `-` stays a subtraction boundary. Returns `None` when the line or
/// character position is out of bounds, or when `position` is not on an
/// identifier character.
pub fn identifier_at_position(text: &str, position: Position) -> Option<(String, Range)> {
    let line = text.lines().nth(position.line as usize)?;
    let chars: Vec<char> = line.chars().collect();
    let char_pos = utf16_to_char_col(line, position.character as usize);

    if char_pos >= chars.len() {
        return None;
    }

    let mut start = char_pos;
    while start > 0 && text_utils::is_identifier_char(chars[start - 1]) {
        start -= 1;
    }

    let mut end = char_pos;
    while end < chars.len() && text_utils::is_identifier_char(chars[end]) {
        end += 1;
    }

    if start >= end {
        return None;
    }

    // Hyphen widening can only matter when a `-` is adjacent to the run.
    // Check that cheaply on the current line before doing the O(document)
    // structural-context scan — the overwhelming majority of cursor queries
    // (formula/declaration positions with no adjacent `-`) skip it entirely.
    let hyphen_adjacent =
        (start > 0 && chars[start - 1] == '-') || (end < chars.len() && chars[end] == '-');
    if hyphen_adjacent
        && in_structural_name_context(text, position.line as usize)
        && let Some(hit) = extend_over_hyphens(line, position.line, &chars, start, end)
    {
        return Some(hit);
    }

    let identifier: String = chars[start..end].iter().collect();
    Some((
        identifier,
        line_run_to_range(line, position.line, start, end),
    ))
}

/// Widen the identifier run `[start, end)` across `-` joins and return the
/// resulting component name, or `None` when no widening applies (no adjacent
/// hyphen, or the widened run is not a valid component name — e.g. `a--b`,
/// or a non-ASCII run the grammar could not lex anyway).
fn extend_over_hyphens(
    line: &str,
    line_idx: u32,
    chars: &[char],
    start: usize,
    end: usize,
) -> Option<(String, Range)> {
    let part = |c: char| text_utils::is_identifier_char(c) || c == '-';

    let mut cstart = start;
    while cstart > 0 && part(chars[cstart - 1]) {
        cstart -= 1;
    }
    let mut cend = end;
    while cend < chars.len() && part(chars[cend]) {
        cend += 1;
    }
    // A maximal run may carry stray edge hyphens (`SEES a -b` punctuation);
    // they are not part of any name.
    while cstart < cend && chars[cstart] == '-' {
        cstart += 1;
    }
    while cend > cstart && chars[cend - 1] == '-' {
        cend -= 1;
    }
    if (cstart, cend) == (start, end) {
        return None;
    }

    let candidate: String = chars[cstart..cend].iter().collect();
    rossi::names::is_valid_component_name(&candidate)
        .then(|| (candidate, line_run_to_range(line, line_idx, cstart, cend)))
}

/// Reference clauses whose component-name operands continue onto following
/// lines — unlike the MACHINE/CONTEXT/EVENT headers, which are followed by a
/// body rather than more names.
const REFERENCE_LIST_CLAUSES: [&str; 3] = ["REFINES", "SEES", "EXTENDS"];
/// Status modifiers that may precede an inline `EVENT` header
/// (`convergent EVENT do-step`).
const INLINE_EVENT_STATUS: [&str; 3] = ["ordinary", "convergent", "anticipated"];

fn matches_any(word: &str, set: &[&str]) -> bool {
    set.iter().any(|kw| word.eq_ignore_ascii_case(kw))
}

/// Clause keywords whose operands are component names (hyphen-capable
/// structural names): component headers and the reference clauses.
fn is_structural_name_clause(word: &str) -> bool {
    matches_any(word, &["MACHINE", "CONTEXT", "EVENT"])
        || matches_any(word, &REFERENCE_LIST_CLAUSES)
}

/// Whether tokens on line `line_idx` sit in a structural-name position: the
/// line opens with a structural-name clause keyword (possibly behind an
/// inline event status like `convergent EVENT …`), or it continues a
/// REFINES/SEES/EXTENDS list opened on an earlier line. Formula lines must
/// answer `false` so `x-y` stays a subtraction there.
fn in_structural_name_context(text: &str, line_idx: usize) -> bool {
    // Only lines up to and including the cursor line matter.
    let lines: Vec<&str> = text.lines().take(line_idx + 1).collect();
    if lines.len() != line_idx + 1 {
        return false; // cursor line is past the end of the document
    }

    let words = text_utils::identifier_words(lines[line_idx]);
    if let Some(first) = words.first() {
        if is_structural_name_clause(first) {
            return true;
        }
        // `convergent EVENT do-step` — inline status before the keyword.
        if words.len() >= 2
            && matches_any(first, &INLINE_EVENT_STATUS)
            && words[1].eq_ignore_ascii_case("EVENT")
        {
            return true;
        }
        if text_utils::is_clause_boundary_keyword(first) {
            return false;
        }
    }

    // Continuation line: the nearest clause opened above decides.
    for prev in lines[..line_idx].iter().rev() {
        if let Some(first) = text_utils::first_identifier_word(prev) {
            if matches_any(&first, &REFERENCE_LIST_CLAUSES) {
                return true;
            }
            if text_utils::is_clause_boundary_keyword(&first) {
                return false;
            }
        }
    }
    false
}

/// Return the token that contains `position`, together with its range.
///
/// A cursor on an identifier character belongs to that identifier. Any other
/// cursor targets the character under it: first as an operator from the
/// canonical `rossi::operators` table (maximal munch, so multi-character
/// operators like `:=` come back whole no matter which of their characters
/// the cursor sits on), then as the trailing edge of the identifier ending
/// just before the cursor, and finally as the bare character.
///
/// Returns `None` when the line or character position is out of bounds.
pub fn word_at_position(text: &str, position: Position) -> Option<(String, Range)> {
    let line = text.lines().nth(position.line as usize)?;
    let char_pos = utf16_to_char_col(line, position.character as usize);

    // The operator must be tried before `identifier_at_position`'s
    // trailing-edge rule, or an operator glued to an identifier would lose
    // its first character to the word on its left (`count:=1`, cursor on `:`).
    let on_identifier = line
        .chars()
        .nth(char_pos)
        .is_some_and(text_utils::is_identifier_char);
    if on_identifier {
        return identifier_at_position(text, position);
    }

    // `operator_at` is char-indexed in and out; the converter turns its char
    // range back into a UTF-16 range.
    if let Some((operator, range)) = rossi::operators::operator_at(line, char_pos) {
        return Some((
            operator.to_string(),
            line_run_to_range(line, position.line, range.start, range.end),
        ));
    }

    // Cursor just past a word (trailing edge) keeps resolving to that word.
    if let Some(hit) = identifier_at_position(text, position) {
        return Some(hit);
    }

    let ch = line.chars().nth(char_pos)?;
    Some((
        ch.to_string(),
        line_run_to_range(line, position.line, char_pos, char_pos + 1),
    ))
}

// Position/offset conversion lives in [`crate::position`] (the single,
// UTF-16-correct source of truth). Re-exported here so the providers that
// historically imported these from `identifier_utils` keep working.
pub use crate::position::{position_to_offset, span_to_range};

/// Word-boundary rule for [`find_whole_word_locations`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordBoundary {
    /// Mathematical identifiers: `-` is an operator, so the `x` of `x-y`
    /// is a whole word.
    MathIdentifier,
    /// Component names: `-` joins name segments, so `ENV_C` inside
    /// `ENV_C-1` is *not* a whole word. Used for component/event renames.
    ComponentName,
}

impl WordBoundary {
    /// The boundary rule appropriate for a needle: a hyphenated name can only
    /// be a component name, so it must use the component boundary (a bare
    /// `do-step` must not match inside `do-step-2`). A hyphen-free needle
    /// keeps the math boundary. Callers that need the component boundary for a
    /// hyphen-free needle (renaming a component `ENV_C` next to `ENV_C-1`)
    /// pass [`WordBoundary::ComponentName`] explicitly.
    pub fn for_name(name: &str) -> Self {
        if name.contains('-') {
            WordBoundary::ComponentName
        } else {
            WordBoundary::MathIdentifier
        }
    }
}

/// Find every whole-word occurrence of `identifier` in `text`, skipping comments,
/// and return them as LSP `Location`s in `uri`.
///
/// Matching is comment-aware: the text is masked through
/// [`rossi::comments::mask_comments_chars`] (one space per comment char, so
/// char columns are unchanged) and only the masked code is searched. Word
/// boundaries follow the given [`WordBoundary`] rule on both sides. When
/// `line_range` is `Some((start, end))`, only lines in that inclusive range
/// contribute matches.
pub fn find_whole_word_locations(
    text: &str,
    identifier: &str,
    uri: &Url,
    line_range: Option<(usize, usize)>,
    boundary: WordBoundary,
) -> Vec<Location> {
    let boundary_char: fn(char) -> bool = match boundary {
        WordBoundary::ComponentName => |c| text_utils::is_identifier_char(c) || c == '-',
        WordBoundary::MathIdentifier => text_utils::is_identifier_char,
    };
    let mut locations = Vec::new();
    let id_chars: Vec<char> = identifier.chars().collect();
    if id_chars.is_empty() {
        return locations;
    }
    let masked = rossi::comments::mask_comments_chars(text);

    for (line_idx, line) in masked.lines().enumerate() {
        if let Some((start_line, end_line)) = line_range
            && (line_idx < start_line || line_idx > end_line)
        {
            continue;
        }

        let chars: Vec<char> = line.chars().collect();
        for col in 0..chars.len().saturating_sub(id_chars.len() - 1) {
            if chars[col..col + id_chars.len()] != id_chars {
                continue;
            }
            let before_ok = col == 0 || !boundary_char(chars[col - 1]);
            let after_ok =
                col + id_chars.len() >= chars.len() || !boundary_char(chars[col + id_chars.len()]);
            if before_ok && after_ok {
                locations.push(Location::new(
                    uri.clone(),
                    Range::new(
                        Position::new(line_idx as u32, col as u32),
                        Position::new(line_idx as u32, (col + id_chars.len()) as u32),
                    ),
                ));
            }
        }
    }

    locations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    fn word_at(text: &str, position: Position) -> Option<String> {
        word_at_position(text, position).map(|(word, _)| word)
    }

    #[test]
    fn word_in_middle_of_identifier() {
        assert_eq!(word_at("count := 0", pos(0, 2)).as_deref(), Some("count"));
    }

    #[test]
    fn cursor_resolution_is_utf16_after_astral_char() {
        // `𝔹` (U+1D539) is one char but two UTF-16 units, so `foo` begins at
        // UTF-16 column 3. The cursor position (UTF-16) must map to the right
        // char, and the returned range must be in UTF-16 columns.
        let text = "𝔹 foo";
        let (word, range) = identifier_at_position(text, pos(0, 3)).expect("cursor on `foo`");
        assert_eq!(word, "foo");
        assert_eq!(range.start, pos(0, 3));
        assert_eq!(range.end, pos(0, 6));
        // The same through word_at_position (trailing edge / operator path).
        assert_eq!(word_at(text, pos(0, 4)).as_deref(), Some("foo"));
    }

    #[test]
    fn word_at_start_of_identifier() {
        assert_eq!(word_at("count := 0", pos(0, 0)).as_deref(), Some("count"));
    }

    #[test]
    fn multichar_operator_at_position() {
        // cursor on `:` or `=` of `:=` — the whole operator comes back
        assert_eq!(word_at("count := 0", pos(0, 6)).as_deref(), Some(":="));
        assert_eq!(word_at("count := 0", pos(0, 7)).as_deref(), Some(":="));
    }

    #[test]
    fn unspaced_operator_beats_trailing_identifier() {
        // `:=` glued to `count` — the cursor on `:` targets the operator, not
        // the trailing edge of the identifier (issue #34 for unspaced sources).
        assert_eq!(word_at("count:=1", pos(0, 5)).as_deref(), Some(":="));
        assert_eq!(word_at("count:=1", pos(0, 6)).as_deref(), Some(":="));
    }

    #[test]
    fn lone_colon_is_an_operator_token() {
        // ASCII set membership; pins the single-char operator path.
        assert_eq!(word_at("x : S", pos(0, 2)).as_deref(), Some(":"));
    }

    #[test]
    fn trailing_edge_still_resolves_identifier() {
        // Cursor on the space right after `count` — no operator there, so
        // the trailing-edge rule keeps the identifier.
        assert_eq!(word_at("count := 0", pos(0, 5)).as_deref(), Some("count"));
    }

    #[test]
    fn operator_range_at_position() {
        let (word, range) = word_at_position("count := 0", pos(0, 7)).unwrap();
        assert_eq!(word, ":=");
        assert_eq!(range, Range::new(pos(0, 6), pos(0, 8)));
    }

    #[test]
    fn single_char_fallback_on_non_operator_punctuation() {
        let (word, range) = word_at_position("f (x)", pos(0, 2)).unwrap();
        assert_eq!(word, "(");
        assert_eq!(range, Range::new(pos(0, 2), pos(0, 3)));
    }

    #[test]
    fn underscored_identifier() {
        assert_eq!(word_at("my_var := 0", pos(0, 3)).as_deref(), Some("my_var"));
    }

    #[test]
    fn out_of_bounds_line() {
        assert!(word_at("x", pos(5, 0)).is_none());
    }

    #[test]
    fn out_of_bounds_char() {
        assert!(word_at("x", pos(0, 99)).is_none());
    }

    #[test]
    fn multibyte_identifier() {
        // chars (not bytes) are counted
        assert_eq!(word_at("α_name := 0", pos(0, 3)).as_deref(), Some("α_name"));
    }

    #[test]
    fn hyphenated_component_name_in_structural_position() {
        // Cursor anywhere on `ENV_C-1` in a SEES clause resolves the whole
        // component name, hyphens included (issue #28).
        let text = "MACHINE m1\nSEES ENV_C-1\nEND\n";
        let (word, range) = identifier_at_position(text, pos(1, 6)).unwrap();
        assert_eq!(word, "ENV_C-1");
        assert_eq!(range, Range::new(pos(1, 5), pos(1, 12)));
        // …also from the segment after the hyphen, and from the hyphen
        // itself (via `identifier_at_position`, which rename/definition use;
        // `word_at_position` keeps preferring the `-` operator there).
        assert_eq!(word_at(text, pos(1, 11)).as_deref(), Some("ENV_C-1"));
        let (word, _) = identifier_at_position(text, pos(1, 10)).unwrap();
        assert_eq!(word, "ENV_C-1");
    }

    #[test]
    fn hyphenated_name_on_clause_continuation_line() {
        let text = "MACHINE m1\nREFINES\n    M-ALPHA-0\nEND\n";
        assert_eq!(word_at(text, pos(2, 6)).as_deref(), Some("M-ALPHA-0"));
    }

    #[test]
    fn hyphen_stays_subtraction_in_formula_lines() {
        // INVARIANTS continuation: `x-y` is `x − y`, not a name.
        let text = "MACHINE m1\nINVARIANTS\n    @inv1 x-y > 0\nEND\n";
        assert_eq!(word_at(text, pos(2, 10)).as_deref(), Some("x"));
        assert_eq!(word_at(text, pos(2, 12)).as_deref(), Some("y"));
    }

    #[test]
    fn invalid_hyphen_run_falls_back_to_fragment() {
        // `a--b` is not a valid component name; the cursor keeps the fragment.
        let text = "SEES a--b\n";
        assert_eq!(word_at(text, pos(0, 5)).as_deref(), Some("a"));
    }

    #[test]
    fn component_boundary_protects_longer_names() {
        let uri = Url::parse("file:///t.eventb").unwrap();
        let text = "MACHINE m1\nSEES ENV_C ENV_C-1\nEND\n";
        // Math boundary (today's behavior): `ENV_C` also matches the prefix
        // of `ENV_C-1` because `-` is a boundary char there.
        let math =
            find_whole_word_locations(text, "ENV_C", &uri, None, WordBoundary::MathIdentifier);
        assert_eq!(math.len(), 2);
        // Component boundary: renaming component `ENV_C` must leave the
        // sibling `ENV_C-1` alone.
        let comp =
            find_whole_word_locations(text, "ENV_C", &uri, None, WordBoundary::ComponentName);
        assert_eq!(comp.len(), 1);
        assert_eq!(comp[0].range.start, pos(1, 5));
        // `WordBoundary::for_name` picks the component boundary for a
        // hyphenated needle, so `ENV_C-1` matches only itself.
        assert_eq!(
            WordBoundary::for_name("ENV_C-1"),
            WordBoundary::ComponentName
        );
        assert_eq!(
            WordBoundary::for_name("ENV_C"),
            WordBoundary::MathIdentifier
        );
        let hyphenated = find_whole_word_locations(
            text,
            "ENV_C-1",
            &uri,
            None,
            WordBoundary::for_name("ENV_C-1"),
        );
        assert_eq!(hyphenated.len(), 1);
        assert_eq!(hyphenated[0].range.start, pos(1, 11));
    }
}
