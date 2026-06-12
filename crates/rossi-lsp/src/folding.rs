//! Folding Range provider for Event-B
//!
//! Provides code folding support for:
//! - CONTEXT...END blocks
//! - MACHINE...END blocks
//! - EVENT...END blocks
//! - INITIALISATION...END blocks
//! - Clause sections (SETS, CONSTANTS, VARIABLES, INVARIANTS, AXIOMS, etc.)

use crate::lsp_types::{FoldingRange, FoldingRangeKind, FoldingRangeParams};
use rossi::keywords::{self, KeywordGroup, KeywordId};

/// Provides folding ranges for Event-B documents
pub struct FoldingRangeProvider;

impl FoldingRangeProvider {
    pub fn new() -> Self {
        Self
    }

    /// Provide folding ranges for a document
    pub fn folding_ranges(
        &self,
        _params: &FoldingRangeParams,
        text: &str,
    ) -> Option<Vec<FoldingRange>> {
        let ranges = self.detect_folding_ranges(text);

        if ranges.is_empty() {
            None
        } else {
            Some(ranges)
        }
    }

    /// Detect all folding ranges in the document
    fn detect_folding_ranges(&self, text: &str) -> Vec<FoldingRange> {
        let mut ranges = Vec::new();

        // Keyword detection runs on comment-masked lines so `END` or `EVENT`
        // spelled inside a `//` or `/* */` comment never opens or closes a
        // fold. The original lines ride along for the one check that must
        // see comments: a comment-only line is content, not a blank line.
        let masked = rossi::comments::mask_comments_chars(text);
        let masked_lines: Vec<&str> = masked.lines().collect();
        let lines: Vec<&str> = text.lines().collect();

        // Detect component blocks (CONTEXT...END, MACHINE...END)
        ranges.extend(self.detect_component_blocks(&masked_lines));

        // Detect event blocks
        ranges.extend(self.detect_event_blocks(&masked_lines));

        // Detect clause sections
        ranges.extend(self.detect_clause_sections(&masked_lines, &lines));

        ranges
    }

    /// Detect CONTEXT...END and MACHINE...END blocks
    fn detect_component_blocks(&self, lines: &[&str]) -> Vec<FoldingRange> {
        let mut ranges = Vec::new();
        let mut component_start: Option<usize> = None;

        for (idx, line) in lines.iter().enumerate() {
            let trimmed = line.trim();

            // Check for component start
            if trimmed.starts_with(keywords::spell(KeywordId::Context))
                || trimmed.starts_with(keywords::spell(KeywordId::Machine))
            {
                component_start = Some(idx);
            }

            // Check for component end
            if trimmed == "END"
                && component_start.is_some()
                && let Some(start) = component_start
            {
                // Only create folding range if there's content to fold
                if idx > start {
                    ranges.push(FoldingRange {
                        start_line: start as u32,
                        start_character: None,
                        end_line: idx as u32,
                        end_character: None,
                        kind: Some(FoldingRangeKind::Region),
                        collapsed_text: None,
                    });
                }
                component_start = None;
            }
        }

        ranges
    }

    /// Detect EVENT...END and INITIALISATION...END blocks
    fn detect_event_blocks(&self, lines: &[&str]) -> Vec<FoldingRange> {
        let mut ranges = Vec::new();
        let mut event_stack: Vec<usize> = Vec::new();

        for (idx, line) in lines.iter().enumerate() {
            let trimmed = line.trim();

            // Check for event start
            if trimmed.starts_with(keywords::spell(KeywordId::Event))
                || trimmed.starts_with(keywords::spell(KeywordId::Initialisation))
            {
                event_stack.push(idx);
            }

            // Check for event end
            if trimmed == "END"
                && !event_stack.is_empty()
                && let Some(start) = event_stack.pop()
            {
                // Only create folding range if there's content to fold
                if idx > start {
                    ranges.push(FoldingRange {
                        start_line: start as u32,
                        start_character: None,
                        end_line: idx as u32,
                        end_character: None,
                        kind: Some(FoldingRangeKind::Region),
                        collapsed_text: None,
                    });
                }
            }
        }

        ranges
    }

    /// Detect clause sections (SETS, CONSTANTS, VARIABLES, etc.)
    fn detect_clause_sections(&self, masked_lines: &[&str], lines: &[&str]) -> Vec<FoldingRange> {
        let mut ranges = Vec::new();

        // Context and machine clause keywords, from the single source of truth.
        for clause in keywords::iter_group(KeywordGroup::ContextClause)
            .chain(keywords::iter_group(KeywordGroup::MachineClause))
        {
            ranges.extend(self.detect_single_clause_section(masked_lines, lines, clause.text()));
        }

        ranges
    }

    /// Detect a single clause section.
    ///
    /// Keyword checks use `masked_lines`; the blank-line terminator checks
    /// the original `lines`, because a comment-only line masks to blank but
    /// is clause content (Camille-style block comments sit inside clauses).
    fn detect_single_clause_section(
        &self,
        masked_lines: &[&str],
        lines: &[&str],
        clause_name: &str,
    ) -> Vec<FoldingRange> {
        let mut ranges = Vec::new();
        let mut clause_start: Option<usize> = None;

        for (idx, line) in masked_lines.iter().enumerate() {
            let trimmed = line.trim();

            // Check if this line is the clause keyword
            if trimmed == clause_name {
                clause_start = Some(idx);
                continue;
            }

            // If we're in a clause, check if we've reached the end
            if let Some(start) = clause_start {
                // Check if this line starts a new clause or is END
                let is_end_of_clause = lines[idx].trim().is_empty()
                    || trimmed == "END"
                    || trimmed.starts_with("CONTEXT")
                    || trimmed.starts_with("MACHINE")
                    || trimmed.starts_with("EVENT")
                    || trimmed.starts_with("INITIALISATION")
                    || self.is_clause_keyword(trimmed);

                if is_end_of_clause {
                    // Only create folding range if there's content to fold
                    if idx > start + 1 {
                        ranges.push(FoldingRange {
                            start_line: start as u32,
                            start_character: None,
                            end_line: (idx - 1) as u32,
                            end_character: None,
                            kind: Some(FoldingRangeKind::Region),
                            collapsed_text: None,
                        });
                    }
                    clause_start = None;
                }
            }
        }

        // Handle clause that extends to end of file
        if let Some(start) = clause_start {
            let end = lines.len() - 1;
            if end > start {
                ranges.push(FoldingRange {
                    start_line: start as u32,
                    start_character: None,
                    end_line: end as u32,
                    end_character: None,
                    kind: Some(FoldingRangeKind::Region),
                    collapsed_text: None,
                });
            }
        }

        ranges
    }

    /// Check if a line starts with a clause keyword (its first whitespace-separated token)
    fn is_clause_keyword(&self, trimmed: &str) -> bool {
        let first = trimmed.split_whitespace().next().unwrap_or("");
        keywords::is_clause_keyword(first)
    }
}

impl Default for FoldingRangeProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fold_context_block() {
        let provider = FoldingRangeProvider::new();
        let text = "CONTEXT test\nSETS S\nCONSTANTS c\nEND";

        let ranges = provider.detect_folding_ranges(text);

        // Should have a range for the CONTEXT block
        let has_context = ranges.iter().any(|r| r.start_line == 0 && r.end_line == 3);
        assert!(has_context, "Should detect CONTEXT...END block");
    }

    #[test]
    fn test_fold_machine_block() {
        let provider = FoldingRangeProvider::new();
        let text = "MACHINE test\nVARIABLES x\nINVARIANTS @inv1 x > 0\nEND";

        let ranges = provider.detect_folding_ranges(text);

        // Should have a range for the MACHINE block
        let has_machine = ranges.iter().any(|r| r.start_line == 0 && r.end_line == 3);
        assert!(has_machine, "Should detect MACHINE...END block");
    }

    #[test]
    fn test_fold_event_block() {
        let provider = FoldingRangeProvider::new();
        let text = "MACHINE test\nEVENTS\n    EVENT evt\n    THEN x := x + 1\n    END\nEND";

        let ranges = provider.detect_folding_ranges(text);

        // Should have a range for the EVENT block
        let has_event = ranges.iter().any(|r| r.start_line == 2 && r.end_line == 4);
        assert!(has_event, "Should detect EVENT...END block");
    }

    #[test]
    fn test_fold_initialisation_block() {
        let provider = FoldingRangeProvider::new();
        let text = "MACHINE test\nEVENTS\n    EVENT INITIALISATION\n    THEN x := 0\n    END\nEND";

        let ranges = provider.detect_folding_ranges(text);

        // Should have a range for the INITIALISATION block
        let has_init = ranges.iter().any(|r| r.start_line == 2 && r.end_line == 4);
        assert!(has_init, "Should detect INITIALISATION...END block");
    }

    #[test]
    fn test_fold_variables_clause() {
        let provider = FoldingRangeProvider::new();
        let text = "MACHINE test\nVARIABLES\n    x\n    y\n    z\nINVARIANTS\nEND";

        let ranges = provider.detect_folding_ranges(text);

        // Should have a range for the VARIABLES clause
        let has_vars = ranges.iter().any(|r| r.start_line == 1 && r.end_line == 4);
        assert!(has_vars, "Should detect VARIABLES clause");
    }

    #[test]
    fn test_fold_invariants_clause() {
        let provider = FoldingRangeProvider::new();
        let text = "MACHINE test\nVARIABLES x\nINVARIANTS\n    @inv1 x > 0\n    @inv2 x < 100\nEVENTS\nEND";

        let ranges = provider.detect_folding_ranges(text);

        // Should have a range for the INVARIANTS clause
        let has_invs = ranges.iter().any(|r| r.start_line == 2 && r.end_line == 4);
        assert!(has_invs, "Should detect INVARIANTS clause");
    }

    #[test]
    fn test_fold_axioms_clause() {
        let provider = FoldingRangeProvider::new();
        let text = "CONTEXT test\nCONSTANTS c\nAXIOMS\n    @axm1 c > 0\n    @axm2 c < 100\nEND";

        let ranges = provider.detect_folding_ranges(text);

        // Should have a range for the AXIOMS clause
        let has_axms = ranges.iter().any(|r| r.start_line == 2 && r.end_line == 4);
        assert!(has_axms, "Should detect AXIOMS clause");
    }

    #[test]
    fn test_fold_variant_clause() {
        // VARIANT was previously missing from the folding clause list; it now folds.
        let provider = FoldingRangeProvider::new();
        let text = "MACHINE test\nVARIABLES x\nVARIANT\n    max - x\n    - 1\nEVENTS\nEND";

        let ranges = provider.detect_folding_ranges(text);

        let has_variant = ranges.iter().any(|r| r.start_line == 2 && r.end_line == 4);
        assert!(has_variant, "Should detect VARIANT clause");
    }

    #[test]
    fn test_theorems_section_is_folded() {
        // THEOREMS is a real context/machine clause, so its body folds like any other.
        let provider = FoldingRangeProvider::new();
        let text = "CONTEXT test\nTHEOREMS\n    @thm1 1 = 1\n    @thm2 2 = 2\nEND";

        let ranges = provider.detect_folding_ranges(text);

        let folds_theorems = ranges.iter().any(|r| r.start_line == 1);
        assert!(folds_theorems, "THEOREMS clause should produce a fold");
    }

    #[test]
    fn test_no_fold_for_empty_clause() {
        let provider = FoldingRangeProvider::new();
        let text = "MACHINE test\nVARIABLES\nINVARIANTS\nEND";

        let ranges = provider.detect_folding_ranges(text);

        // Should not have ranges for empty clauses
        let vars_range = ranges.iter().find(|r| r.start_line == 1);
        assert!(
            vars_range.is_none(),
            "Should not create folding range for empty VARIABLES clause"
        );
    }

    #[test]
    fn test_multiple_events() {
        let provider = FoldingRangeProvider::new();
        let text = "MACHINE test\nEVENTS\n    EVENT evt1\n    THEN x := 1\n    END\n    EVENT evt2\n    THEN y := 2\n    END\nEND";

        let ranges = provider.detect_folding_ranges(text);

        // Should have ranges for both events
        let has_evt1 = ranges.iter().any(|r| r.start_line == 2 && r.end_line == 4);
        let has_evt2 = ranges.iter().any(|r| r.start_line == 5 && r.end_line == 7);

        assert!(has_evt1, "Should detect first EVENT block");
        assert!(has_evt2, "Should detect second EVENT block");
    }

    #[test]
    fn test_all_folding_kinds_are_region() {
        let provider = FoldingRangeProvider::new();
        let text =
            "MACHINE test\nVARIABLES x y\nINVARIANTS inv: x > 0\nEVENTS\n    EVENT e\n    END\nEND";

        let ranges = provider.detect_folding_ranges(text);

        // All ranges should have kind Region
        for range in ranges {
            assert_eq!(
                range.kind,
                Some(FoldingRangeKind::Region),
                "All folding ranges should be of kind Region"
            );
        }
    }
}
