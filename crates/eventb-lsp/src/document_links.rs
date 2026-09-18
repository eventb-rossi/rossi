//! Document links provider for Event-B
//!
//! Implements `textDocument/documentLink` to provide clickable links for:
//! - SEES references (machine → context)
//! - REFINES references (concrete machine → abstract machine)
//! - EXTENDS references (context → parent context)

use crate::lsp_types::{DocumentLink, DocumentLinkParams, Uri};
use std::sync::Arc;
use tracing::debug;

use crate::cross_references::CrossReferenceManager;
use crate::text_utils;
use rossi::keywords::KeywordId;

/// Provides document link functionality
pub struct DocumentLinkProvider {
    /// Cross-reference manager for resolving component URIs
    cross_ref_manager: Option<Arc<CrossReferenceManager>>,
}

impl Default for DocumentLinkProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl DocumentLinkProvider {
    /// Create a new document link provider
    pub fn new() -> Self {
        Self {
            cross_ref_manager: None,
        }
    }

    /// Set the cross-reference manager
    pub fn set_cross_reference_manager(&mut self, manager: Arc<CrossReferenceManager>) {
        self.cross_ref_manager = Some(manager);
    }

    /// Handle document link request
    ///
    /// Returns a list of clickable links for SEES, REFINES, and EXTENDS clauses
    pub fn document_links(
        &self,
        _params: &DocumentLinkParams,
        text: &str,
    ) -> Option<Vec<DocumentLink>> {
        debug!("Processing document link request");

        let cross_ref_manager = self.cross_ref_manager.as_ref()?;
        let mut links = Vec::new();

        // Scan comment-masked text (char columns preserved): clause keywords
        // and component names inside comments must not produce links. Ranges
        // are built against the original `text` so UTF-16 columns are correct.
        let masked = rossi::comments::mask_comments_chars(text);

        // Find all SEES, REFINES, and EXTENDS clauses
        links.extend(self.find_clause_links(&masked, text, "SEES", cross_ref_manager));
        links.extend(self.find_clause_links(&masked, text, "REFINES", cross_ref_manager));
        links.extend(self.find_clause_links(&masked, text, "EXTENDS", cross_ref_manager));

        debug!("Found {} document links", links.len());

        if links.is_empty() { None } else { Some(links) }
    }

    /// Find links in a specific clause (SEES, REFINES, or EXTENDS). Tokens are
    /// scanned from comment-masked `text`; ranges are resolved against the
    /// original `source` (masking preserves char layout, so the char columns
    /// line up) through the shared UTF-16 converter.
    fn find_clause_links(
        &self,
        text: &str,
        source: &str,
        clause_keyword: &str,
        cross_ref_manager: &CrossReferenceManager,
    ) -> Vec<DocumentLink> {
        let mut links = Vec::new();
        let source_lines: Vec<&str> = source.lines().collect();

        for token in clause_identifier_tokens(text, clause_keyword) {
            if let Some(target_uri) = cross_ref_manager.find_component_uri(&token.name) {
                if let Ok(url) = target_uri.parse::<Uri>() {
                    // `token.start`/`end` are char columns from the scanner;
                    // route them through the single UTF-16 converter against the
                    // real source line.
                    let line_text = source_lines.get(token.line).copied().unwrap_or("");
                    let range = crate::position::line_run_to_range(
                        line_text,
                        token.line as u32,
                        token.start,
                        token.end,
                    );

                    links.push(DocumentLink {
                        range,
                        target: Some(url),
                        tooltip: Some(format!(
                            "Go to {} {}",
                            if clause_keyword == "REFINES" {
                                "machine"
                            } else {
                                "context"
                            },
                            token.name
                        )),
                        data: None,
                    });

                    debug!(
                        "Found link: {} at line {} col {} -> {}",
                        token.name, token.line, token.start, target_uri
                    );
                }
            } else {
                debug!(
                    "Could not resolve {} reference: {}",
                    clause_keyword, token.name
                );
            }
        }

        links
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IdentifierToken {
    name: String,
    line: usize,
    start: usize,
    end: usize,
}

fn clause_identifier_tokens(text: &str, clause_keyword: &str) -> Vec<IdentifierToken> {
    let lines: Vec<&str> = text.lines().collect();
    let mut tokens = Vec::new();

    for (line_idx, line) in lines.iter().enumerate() {
        let Some(clause_pos) = find_keyword_position(line, clause_keyword) else {
            continue;
        };

        // A mid-line keyword only counts on a component header line
        // (camille's `machine M1 refines M0 sees C1`): `sees`, `refines`,
        // and `extends` are legal ordinary identifiers, so anywhere else
        // the clause keyword must be the line's own leading word — a
        // formula such as `@inv1 sees > 0` must not start a clause scan.
        let at_line_start = line.chars().take(clause_pos).all(char::is_whitespace);
        if !at_line_start && !is_component_header(line) {
            continue;
        }

        if matches!(clause_keyword, "REFINES" | "EXTENDS") && is_inside_event(&lines, line_idx) {
            continue;
        }

        let start_col = clause_pos + clause_keyword.chars().count();
        // The clause's own tokens end at the next structural keyword on the
        // line: an inline header (`machine M1 refines M0 sees C1`) carries
        // several clauses, and M0 alone belongs to REFINES.
        let mut line_tokens = tokenize_identifier_positions(line, line_idx, start_col);
        if let Some(cut) = line_tokens
            .iter()
            .position(|token| text_utils::is_clause_boundary_keyword(&token.name))
        {
            line_tokens.truncate(cut);
            tokens.extend(line_tokens);
            continue;
        }
        tokens.extend(line_tokens);

        for (next_line_idx, next_line) in lines.iter().enumerate().skip(line_idx + 1) {
            if is_clause_boundary(next_line) {
                break;
            }
            tokens.extend(tokenize_identifier_positions(next_line, next_line_idx, 0));
        }
    }

    tokens
}

/// Char position of `keyword` as a standalone word anywhere in `line`. The
/// camille layout prints header clauses inline (`machine M1 refines M0
/// sees C1`), so the keyword is not necessarily the line's first word. A
/// match must be delimited on both sides — `@` and `-` also break it, so a
/// label like `@sees` or a hyphenated name segment never counts.
fn find_keyword_position(line: &str, keyword: &str) -> Option<usize> {
    let keyword_chars: Vec<char> = keyword.chars().collect();
    let chars: Vec<char> = line.chars().collect();
    let breaks_word = |ch: char| text_utils::is_identifier_char(ch) || ch == '@' || ch == '-';

    for idx in 0..chars.len().saturating_sub(keyword_chars.len() - 1) {
        if idx > 0 && breaks_word(chars[idx - 1]) {
            continue;
        }
        if !chars[idx..idx + keyword_chars.len()]
            .iter()
            .zip(&keyword_chars)
            .all(|(ch, keyword_ch)| ch.eq_ignore_ascii_case(keyword_ch))
        {
            continue;
        }
        let after_idx = idx + keyword_chars.len();
        if after_idx >= chars.len() || !breaks_word(chars[after_idx]) {
            return Some(idx);
        }
    }

    None
}

/// Whether the line is a component header (`machine M1 …`, `CONTEXT C2`) —
/// the only lines that legitimately carry clause keywords mid-line.
fn is_component_header(line: &str) -> bool {
    text_utils::line_keyword_is(line, KeywordId::Machine)
        || text_utils::line_keyword_is(line, KeywordId::Context)
}

fn is_clause_boundary(line: &str) -> bool {
    text_utils::first_identifier_word(line)
        .is_some_and(|first_word| text_utils::is_clause_boundary_keyword(&first_word))
}

fn is_inside_event(lines: &[&str], line_idx: usize) -> bool {
    let mut in_event = false;

    for line in lines.iter().take(line_idx + 1) {
        if text_utils::event_name_from_line(line).is_some() {
            in_event = true;
            continue;
        }

        // Match the terminator through the keyword table, not a `@`-stripped
        // first word: a labelled action `@end x := 0` is not the END keyword and
        // must not close the event early.
        if text_utils::line_keyword_is(line, KeywordId::End) && in_event {
            in_event = false;
        }
    }

    in_event
}

fn tokenize_identifier_positions(
    line: &str,
    line_idx: usize,
    start_col: usize,
) -> Vec<IdentifierToken> {
    let chars: Vec<char> = line.chars().collect();
    let mut tokens = Vec::new();
    let mut current_start = None;
    let mut current = String::new();
    let mut col = start_col;

    while col < chars.len() {
        let ch = chars[col];
        if ch == '\n' {
            break;
        }

        // `-` is a name-segment joiner here: this scanner only runs over
        // SEES/REFINES/EXTENDS clause regions, whose tokens are component
        // names (`ENV_C-1`), never subtraction.
        if text_utils::is_identifier_char(ch) || ch == '-' {
            if current_start.is_none() {
                current_start = Some(col);
            }
            current.push(ch);
        } else if let Some(start) = current_start.take() {
            tokens.push(IdentifierToken {
                name: std::mem::take(&mut current),
                line: line_idx,
                start,
                end: col,
            });
        }

        col += 1;
    }

    if let Some(start) = current_start {
        tokens.push(IdentifierToken {
            name: current,
            line: line_idx,
            start,
            end: col,
        });
    }

    tokens
}

/// Tokenize identifiers from a string
///
/// Extracts identifier tokens (alphanumeric + underscore) from a string,
/// ignoring commas, whitespace, and other separators.
#[cfg(test)]
fn tokenize_identifiers(text: &str) -> Vec<String> {
    tokenize_identifier_positions(text, 0, 0)
        .into_iter()
        .map(|token| token.name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cross_references::CrossReferenceManager;

    fn make_params(filename: &str) -> DocumentLinkParams {
        DocumentLinkParams {
            text_document: crate::lsp_types::TextDocumentIdentifier {
                uri: format!("file:///{filename}").parse::<Uri>().unwrap(),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        }
    }

    #[test]
    fn test_tokenize_identifiers() {
        let text = "ctx1, ctx2, ctx3";
        let tokens = tokenize_identifiers(text);
        assert_eq!(tokens, vec!["ctx1", "ctx2", "ctx3"]);
    }

    #[test]
    fn test_tokenize_identifiers_with_whitespace() {
        let text = "  ctx1   ctx2   ";
        let tokens = tokenize_identifiers(text);
        assert_eq!(tokens, vec!["ctx1", "ctx2"]);
    }

    #[test]
    fn test_tokenize_identifiers_empty() {
        let text = "   ";
        let tokens = tokenize_identifiers(text);
        assert_eq!(tokens.len(), 0);
    }

    #[test]
    fn test_tokenize_identifiers_stops_at_newline() {
        let text = "ctx1, ctx2\nVARIABLES";
        let tokens = tokenize_identifiers(text);
        assert_eq!(tokens, vec!["ctx1", "ctx2"]);
    }

    #[test]
    fn test_document_links_sees() {
        let cross_ref_manager = Arc::new(CrossReferenceManager::new());

        // Index a context
        let context = r#"
CONTEXT test_ctx
CONSTANTS
    max_val
END
"#;
        cross_ref_manager.update_component("file:///test_ctx.eventb".to_string(), context);

        // Create a machine that SEES the context
        let machine = r#"
MACHINE test_mch
SEES test_ctx
VARIABLES
    count
END
"#;

        let mut provider_with_manager = DocumentLinkProvider::new();
        provider_with_manager.set_cross_reference_manager(cross_ref_manager);

        let params = DocumentLinkParams {
            text_document: crate::lsp_types::TextDocumentIdentifier {
                uri: ("file:///test_mch.eventb").parse::<Uri>().unwrap(),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let links = provider_with_manager.document_links(&params, machine);

        assert!(links.is_some());
        let links = links.unwrap();
        assert_eq!(links.len(), 1);

        let link = &links[0];
        assert_eq!(
            link.target.as_ref().unwrap().as_str(),
            "file:///test_ctx.eventb"
        );
        assert!(link.tooltip.is_some());
        assert!(link.tooltip.as_ref().unwrap().contains("test_ctx"));
    }

    #[test]
    fn test_document_links_multiline_sees() {
        let cross_ref_manager = Arc::new(CrossReferenceManager::new());
        cross_ref_manager.update_component(
            "file:///test_ctx.eventb".to_string(),
            "CONTEXT test_ctx\nEND",
        );

        let machine = "MACHINE test_mch\nSEES\n    test_ctx\nVARIABLES\n    count\nEND\n";

        let mut provider = DocumentLinkProvider::new();
        provider.set_cross_reference_manager(cross_ref_manager);

        let links = provider.document_links(&make_params("test_mch.eventb"), machine);

        assert!(links.is_some());
        let links = links.unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].target.as_ref().unwrap().as_str(),
            "file:///test_ctx.eventb"
        );
        assert_eq!(links[0].range.start, crate::lsp_types::Position::new(2, 4));
        assert_eq!(links[0].range.end, crate::lsp_types::Position::new(2, 12));
    }

    #[test]
    fn test_document_links_multiline_multiple_references() {
        let cross_ref_manager = Arc::new(CrossReferenceManager::new());
        cross_ref_manager.update_component("file:///ctx1.eventb".to_string(), "CONTEXT ctx1\nEND");
        cross_ref_manager.update_component("file:///ctx2.eventb".to_string(), "CONTEXT ctx2\nEND");
        cross_ref_manager.update_component("file:///ctx3.eventb".to_string(), "CONTEXT ctx3\nEND");

        let machine =
            "MACHINE test_mch\nSEES\n    ctx1,\n    ctx2 ctx3\nVARIABLES\n    count\nEND\n";

        let mut provider = DocumentLinkProvider::new();
        provider.set_cross_reference_manager(cross_ref_manager);

        let links = provider
            .document_links(&make_params("test_mch.eventb"), machine)
            .unwrap();

        assert_eq!(links.len(), 3);
        let targets: Vec<_> = links
            .iter()
            .map(|link| link.target.as_ref().unwrap().as_str())
            .collect();
        assert!(targets.contains(&"file:///ctx1.eventb"));
        assert!(targets.contains(&"file:///ctx2.eventb"));
        assert!(targets.contains(&"file:///ctx3.eventb"));
    }

    #[test]
    fn test_document_links_multiline_refines() {
        let cross_ref_manager = Arc::new(CrossReferenceManager::new());
        cross_ref_manager.update_component("file:///M0.eventb".to_string(), "MACHINE M0\nEND");

        let machine = "MACHINE M1\nREFINES\n    M0\nVARIABLES\n    state\nEND\n";

        let mut provider = DocumentLinkProvider::new();
        provider.set_cross_reference_manager(cross_ref_manager);

        let links = provider
            .document_links(&make_params("M1.eventb"), machine)
            .unwrap();

        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].target.as_ref().unwrap().as_str(),
            "file:///M0.eventb"
        );
    }

    #[test]
    fn test_is_inside_event_ignores_labelled_end_action() {
        // A labelled action `@end …` must not be read as the END keyword and
        // close the event early: a line after it but before the real END is
        // still inside the event. Resolving the leading token through the
        // keyword table (not a `@`-stripped word) keeps `@end` out of `END`.
        let source = "\
MACHINE m
EVENTS
    EVENT e
    THEN
        @end x := 0
        @act y := 1
    END
END";
        let lines: Vec<&str> = source.lines().collect();
        // Line 5 (`@act y := 1`) sits after the `@end` action but inside event e.
        assert!(is_inside_event(&lines, 5));
        // Line 7 (the trailing machine END) is past the event's real END.
        assert!(!is_inside_event(&lines, 7));
    }

    #[test]
    fn test_document_links_inline_camille_header() {
        let cross_ref_manager = Arc::new(CrossReferenceManager::new());
        cross_ref_manager.update_component("file:///M0.eventb".to_string(), "machine M0\nend");
        cross_ref_manager.update_component("file:///C1.eventb".to_string(), "context C1\nend");

        let machine = "machine M1 refines M0 sees C1\nvariables x\nend\n";

        let mut provider = DocumentLinkProvider::new();
        provider.set_cross_reference_manager(cross_ref_manager);

        let links = provider
            .document_links(&make_params("M1.eventb"), machine)
            .unwrap();

        let mut targets: Vec<_> = links
            .iter()
            .map(|link| link.target.as_ref().unwrap().as_str())
            .collect();
        targets.sort_unstable();
        assert_eq!(targets, vec!["file:///C1.eventb", "file:///M0.eventb"]);
    }

    #[test]
    fn test_document_links_ignore_keyword_identifier_in_formula() {
        // `sees` is a legal ordinary identifier. Using it inside a formula
        // must not start a SEES clause scan that turns identifiers on the
        // following lines into bogus links.
        let cross_ref_manager = Arc::new(CrossReferenceManager::new());
        cross_ref_manager.update_component("file:///ctx1.eventb".to_string(), "CONTEXT ctx1\nEND");

        let machine = "MACHINE test_mch\nSEES ctx1\nVARIABLES\n    sees\nINVARIANTS\n    @inv1 sees > 0\n    @inv2 ctx1 = ctx1\nEND\n";

        let mut provider = DocumentLinkProvider::new();
        provider.set_cross_reference_manager(cross_ref_manager);

        let links = provider
            .document_links(&make_params("test_mch.eventb"), machine)
            .unwrap();

        // Only the genuine `SEES ctx1` reference on line 1 is a link; the
        // `ctx1` occurrences inside @inv2 are not.
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].range.start, crate::lsp_types::Position::new(1, 5));
    }

    #[test]
    fn test_document_links_ignore_event_refines() {
        let cross_ref_manager = Arc::new(CrossReferenceManager::new());
        cross_ref_manager.update_component("file:///M0.eventb".to_string(), "MACHINE M0\nEND");

        let machine = "MACHINE M1\nEVENTS\n    EVENT update\n    REFINES M0\n    THEN\n        @act1 skip\n    END\nEND\n";

        let mut provider = DocumentLinkProvider::new();
        provider.set_cross_reference_manager(cross_ref_manager);

        let links = provider.document_links(&make_params("M1.eventb"), machine);

        assert!(links.is_none());
    }

    #[test]
    fn test_document_links_multiline_extends() {
        let cross_ref_manager = Arc::new(CrossReferenceManager::new());
        cross_ref_manager.update_component("file:///C0.eventb".to_string(), "CONTEXT C0\nEND");

        let context = "CONTEXT C1\nEXTENDS\n    C0\nCONSTANTS\n    c\nEND\n";

        let mut provider = DocumentLinkProvider::new();
        provider.set_cross_reference_manager(cross_ref_manager);

        let links = provider
            .document_links(&make_params("C1.eventb"), context)
            .unwrap();

        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].target.as_ref().unwrap().as_str(),
            "file:///C0.eventb"
        );
    }

    #[test]
    fn test_document_links_refines() {
        let cross_ref_manager = Arc::new(CrossReferenceManager::new());

        // Index an abstract machine
        let abstract_mch = r#"
MACHINE abstract_mch
VARIABLES
    state
END
"#;
        cross_ref_manager.update_component("file:///abstract_mch.eventb".to_string(), abstract_mch);

        // Create a concrete machine that REFINES the abstract machine
        let concrete_mch = r#"
MACHINE concrete_mch
REFINES abstract_mch
VARIABLES
    state
    detail
END
"#;

        let mut provider = DocumentLinkProvider::new();
        provider.set_cross_reference_manager(cross_ref_manager);

        let params = DocumentLinkParams {
            text_document: crate::lsp_types::TextDocumentIdentifier {
                uri: ("file:///concrete_mch.eventb").parse::<Uri>().unwrap(),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let links = provider.document_links(&params, concrete_mch);

        assert!(links.is_some());
        let links = links.unwrap();
        assert_eq!(links.len(), 1);

        let link = &links[0];
        assert_eq!(
            link.target.as_ref().unwrap().as_str(),
            "file:///abstract_mch.eventb"
        );
    }

    #[test]
    fn test_document_links_extends() {
        let cross_ref_manager = Arc::new(CrossReferenceManager::new());

        // Index a base context
        let base_ctx = r#"
CONTEXT base_ctx
SETS
    STATUS
END
"#;
        cross_ref_manager.update_component("file:///base_ctx.eventb".to_string(), base_ctx);

        // Create a derived context that EXTENDS the base context
        let derived_ctx = r#"
CONTEXT derived_ctx
EXTENDS base_ctx
CONSTANTS
    max_val
END
"#;

        let mut provider = DocumentLinkProvider::new();
        provider.set_cross_reference_manager(cross_ref_manager);

        let params = DocumentLinkParams {
            text_document: crate::lsp_types::TextDocumentIdentifier {
                uri: ("file:///derived_ctx.eventb").parse::<Uri>().unwrap(),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let links = provider.document_links(&params, derived_ctx);

        assert!(links.is_some());
        let links = links.unwrap();
        assert_eq!(links.len(), 1);

        let link = &links[0];
        assert_eq!(
            link.target.as_ref().unwrap().as_str(),
            "file:///base_ctx.eventb"
        );
    }

    #[test]
    fn test_document_links_multiple_references() {
        let cross_ref_manager = Arc::new(CrossReferenceManager::new());

        // Index multiple contexts
        cross_ref_manager.update_component("file:///ctx1.eventb".to_string(), "CONTEXT ctx1\nEND");
        cross_ref_manager.update_component("file:///ctx2.eventb".to_string(), "CONTEXT ctx2\nEND");
        cross_ref_manager.update_component("file:///ctx3.eventb".to_string(), "CONTEXT ctx3\nEND");

        // Create a machine that SEES multiple contexts
        let machine = r#"
MACHINE test_mch
SEES ctx1 ctx2 ctx3
VARIABLES
    count
END
"#;

        let mut provider = DocumentLinkProvider::new();
        provider.set_cross_reference_manager(cross_ref_manager);

        let params = DocumentLinkParams {
            text_document: crate::lsp_types::TextDocumentIdentifier {
                uri: ("file:///test_mch.eventb").parse::<Uri>().unwrap(),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let links = provider.document_links(&params, machine);

        assert!(links.is_some());
        let links = links.unwrap();
        assert_eq!(links.len(), 3);

        // Verify all three contexts are linked
        let targets: Vec<_> = links
            .iter()
            .map(|link| link.target.as_ref().unwrap().as_str())
            .collect();
        assert!(targets.contains(&"file:///ctx1.eventb"));
        assert!(targets.contains(&"file:///ctx2.eventb"));
        assert!(targets.contains(&"file:///ctx3.eventb"));
    }

    #[test]
    fn test_document_links_ignore_comments() {
        let cross_ref_manager = Arc::new(CrossReferenceManager::new());
        cross_ref_manager.update_component(
            "file:///test_ctx.eventb".to_string(),
            "CONTEXT test_ctx\nEND",
        );
        cross_ref_manager.update_component("file:///ctx2.eventb".to_string(), "CONTEXT ctx2\nEND");

        // A SEES clause spelled in comments must produce no link, and a
        // component name in a trailing comment must not become a link.
        let machine = "MACHINE test_mch\n// SEES ctx2\nSEES test_ctx // also: ctx2\nVARIABLES\n    count /* SEES ctx2 */\nEND\n";

        let mut provider = DocumentLinkProvider::new();
        provider.set_cross_reference_manager(cross_ref_manager);

        let links = provider
            .document_links(&make_params("test_mch.eventb"), machine)
            .unwrap();

        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].target.as_ref().unwrap().as_str(),
            "file:///test_ctx.eventb"
        );
    }

    #[test]
    fn test_document_links_no_cross_references() {
        let cross_ref_manager = Arc::new(CrossReferenceManager::new());

        // Create a simple context with no references
        let context = r#"
CONTEXT simple_ctx
CONSTANTS
    max_val
END
"#;

        let mut provider_with_manager = DocumentLinkProvider::new();
        provider_with_manager.set_cross_reference_manager(cross_ref_manager);

        let params = DocumentLinkParams {
            text_document: crate::lsp_types::TextDocumentIdentifier {
                uri: ("file:///simple_ctx.eventb").parse::<Uri>().unwrap(),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let links = provider_with_manager.document_links(&params, context);

        assert!(links.is_none());
    }

    #[test]
    fn test_document_links_unresolved_reference() {
        let cross_ref_manager = Arc::new(CrossReferenceManager::new());

        // Create a machine that SEES a context that doesn't exist
        let machine = r#"
MACHINE test_mch
SEES nonexistent_ctx
VARIABLES
    count
END
"#;

        let mut provider = DocumentLinkProvider::new();
        provider.set_cross_reference_manager(cross_ref_manager);

        let params = DocumentLinkParams {
            text_document: crate::lsp_types::TextDocumentIdentifier {
                uri: ("file:///test_mch.eventb").parse::<Uri>().unwrap(),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let links = provider.document_links(&params, machine);

        // Should return None because the reference cannot be resolved
        assert!(links.is_none());
    }
}
