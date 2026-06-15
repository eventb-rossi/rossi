//! Bounded source-text scans that locate a declared name's position.
//!
//! The AST records which symbols a component declares but not the exact column
//! of each name, so go-to-definition ([`crate::definition`]) and workspace
//! symbol search ([`crate::workspace`]) re-scan the source to pin it down. Each
//! provider used to carry its own copy of that scan; the functions here are the
//! single source of truth they now share. Each returns a [`Position`] (line,
//! UTF-16 column); callers wrap it into a `Location`/`Range` as their API needs.
//!
//! Callers pass text whose comments are masked through
//! [`rossi::comments::mask_comments_chars`] (as the providers already do), so a
//! keyword or name spelled inside a comment is never matched. The char-preserving
//! mask keeps every column identical to the real document.

use crate::component_util::lines_in_window;
use crate::lsp_types::Position;
use crate::text_utils;

/// Position of `identifier`'s first whole-word occurrence inside the `clause`
/// declaration clause (e.g. `SETS`, `CONSTANTS`, `VARIABLES`), searched within
/// the inclusive line `window`.
///
/// The scan enters on the clause header ([`text_utils::line_enters_clause`]) and
/// stops at the next structural boundary
/// ([`text_utils::is_declaration_scan_boundary`]); the name's column comes from
/// [`text_utils::whole_word_utf16_col`].
pub(crate) fn find_symbol_in_clause(
    text: &str,
    clause: &str,
    identifier: &str,
    window: (usize, usize),
) -> Option<Position> {
    let mut in_clause = false;
    for (line_num, line) in lines_in_window(text, window) {
        if text_utils::line_enters_clause(line, clause) {
            in_clause = true;
            continue;
        }
        if in_clause && text_utils::is_declaration_scan_boundary(line) {
            break;
        }
        if in_clause && let Some(col) = text_utils::whole_word_utf16_col(line, identifier) {
            return Some(Position::new(line_num as u32, col));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whole-file search window, the single-component default.
    const FULL: (usize, usize) = (0, usize::MAX);

    #[test]
    fn finds_identifier_in_clause() {
        let text = "MACHINE test\nVARIABLES\n    count\n    total\nEND";

        let pos = find_symbol_in_clause(text, "VARIABLES", "count", FULL).unwrap();
        assert_eq!(pos.line, 2);
        assert!(pos.character >= 4); // after indentation

        let pos = find_symbol_in_clause(text, "VARIABLES", "total", FULL).unwrap();
        assert_eq!(pos.line, 3);
    }

    #[test]
    fn clause_entry_is_case_insensitive() {
        // Lowercase keyword (Camille style) must open the clause like UPPERCASE.
        let text = "machine test\nvariables\n    count\nend";
        assert_eq!(
            find_symbol_in_clause(text, "VARIABLES", "count", FULL).map(|p| p.line),
            Some(2)
        );
    }

    #[test]
    fn keeps_status_as_a_set_name() {
        // STATUS is a contextual keyword but a common set name; a SETS member
        // named STATUS must be found, and must not end the scan early so a
        // following member is still reachable. Lowercase header to boot.
        let text = "context c\nsets\n    STATUS\n    Colours\nend";

        assert_eq!(
            find_symbol_in_clause(text, "SETS", "STATUS", FULL).map(|p| p.line),
            Some(2)
        );
        assert_eq!(
            find_symbol_in_clause(text, "SETS", "Colours", FULL).map(|p| p.line),
            Some(3)
        );
    }

    #[test]
    fn unicode_on_prior_lines_does_not_shift_columns() {
        // BMP `∈`/`ℕ` on a preceding line are one UTF-16 unit each and only on a
        // prior line, so the target column is unaffected.
        let text = "MACHINE test\nINVARIANTS\n    @inv1 x ∈ ℕ\nVARIABLES\n    count\nEND";

        let pos = find_symbol_in_clause(text, "VARIABLES", "count", FULL).unwrap();
        assert_eq!(pos.line, 4);
        assert_eq!(pos.character, 4);
    }

    #[test]
    fn reports_utf16_column_after_astral() {
        // An astral character (`𝔹`, U+1D539) on the identifier's own line is two
        // UTF-16 code units but a single `char`. LSP columns are UTF-16, so the
        // reported column must skip the surrogate pair, not count chars.
        let text = "MACHINE test\nVARIABLES\n    𝔹 count\nEND";

        let pos = find_symbol_in_clause(text, "VARIABLES", "count", FULL).unwrap();
        assert_eq!(pos.line, 2);
        // 4 spaces + `𝔹` (2 units) + 1 space = column 7, not the char index 6.
        assert_eq!(pos.character, 7);
    }
}
