//! Shared line-level text helpers for the LSP providers.
//!
//! None of these helpers know about comments. Callers that scan document
//! structure (keywords, identifiers, clause boundaries) must pass lines of
//! text masked through [`rossi::comments::mask_comments_chars`] — the single
//! comment lexer — so that an `EVENT` or `END` spelled inside a `//` or
//! `/* */` comment is never mistaken for code. The char-preserving mask
//! keeps every line's char columns identical to the original, so positions
//! computed on masked lines are valid in the real document.

pub fn is_identifier_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

pub fn identifier_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();

    for ch in line.chars() {
        if is_identifier_char(ch) {
            current.push(ch);
        } else if !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
    }

    if !current.is_empty() {
        words.push(current);
    }

    words
}

pub fn first_identifier_word(line: &str) -> Option<String> {
    identifier_words(line).into_iter().next()
}

pub fn event_name_from_line(line: &str) -> Option<String> {
    // Whitespace-delimited so a hyphenated event name (`EVENT do-step`,
    // issue #28) comes back whole — `identifier_words` would split it at the
    // hyphen and return only `do`, so `event_line_range`'s match against the
    // AST event name would fail for every hyphenated-named event.
    let mut tokens = line.split_whitespace();
    while let Some(token) = tokens.next() {
        if token.eq_ignore_ascii_case("EVENT") {
            return tokens.next().map(str::to_string);
        }
    }
    None
}

pub fn is_clause_boundary_keyword(word: &str) -> bool {
    rossi::keywords::is_clause_boundary(word)
}

/// Whether `line`'s first whitespace-delimited token is the keyword `id`,
/// resolved case-insensitively through the canonical keyword table. A header
/// line like `EVENT do-step` or `sees C1` matches on its leading keyword
/// regardless of casing or what follows.
///
/// Uses the whole first token (not [`first_identifier_word`], which would strip
/// a leading `@`): a labelled action such as `@end y := 0` must NOT be read as
/// the `END` keyword.
pub fn line_keyword_is(line: &str, id: rossi::keywords::KeywordId) -> bool {
    line.split_whitespace()
        .next()
        .and_then(rossi::keywords::lookup)
        .map(|keyword| keyword.id)
        == Some(id)
}

/// Whether `line` begins a new structural region (clause, component, or event)
/// and so bounds a scan that walks a clause/section body — its first token is a
/// clause-boundary keyword, EXCEPT `STATUS`.
///
/// `STATUS` is a contextual keyword that only acts as one inside an EVENT and is
/// commonly used as a set/constant name, so a line that is just `STATUS` in a
/// declaration clause is a declaration to be found, not a boundary to stop at.
/// Matching the first token (not the whole line) keeps a header carrying inline
/// content — e.g. `EVENT incr` — recognised as a boundary.
pub fn is_declaration_scan_boundary(line: &str) -> bool {
    let first = line.split_whitespace().next().unwrap_or("");
    is_clause_boundary_keyword(first) && !first.eq_ignore_ascii_case("STATUS")
}

/// Find an event's `[start_line, end_line]` range over text that is already
/// comment-masked, so a caller scanning many events masks the document once
/// instead of per event. An `EVENT foo` or `END` inside a comment is blanked in
/// `masked` and so cannot open or close the range; the terminator is matched
/// through the keyword table ([`line_keyword_is`]), so a labelled action whose
/// label spells a keyword (`@end x := 0`) is not read as `END`.
pub(crate) fn event_line_range_in(masked: &str, event_name: &str) -> Option<(usize, usize)> {
    let lines: Vec<&str> = masked.lines().collect();
    let start_line = lines
        .iter()
        .position(|line| event_name_from_line(line).as_deref() == Some(event_name))?;

    let end_line = lines
        .iter()
        .enumerate()
        .skip(start_line + 1)
        .find_map(|(line_idx, line)| {
            line_keyword_is(line, rossi::keywords::KeywordId::End).then_some(line_idx)
        })
        .unwrap_or_else(|| lines.len().saturating_sub(1));

    Some((start_line, end_line))
}

/// The event whose `ANY` clause declares `identifier`, when `position` falls
/// inside that event's line range. `masked` is the comment-masked document, so
/// an `EVENT`/`END` spelled in a comment cannot bound the range. Returns `None`
/// when the cursor is not inside any event or the identifier is not one of its
/// parameters.
///
/// This is the single source of truth for "is this name an event parameter at
/// this position", shared by find-references / rename and hover so the event
/// scoping cannot drift between features. Only `position.line` is consulted;
/// the column is irrelevant to the line-range scoping.
pub(crate) fn event_parameter_at_position<'a>(
    machine: &'a rossi::Machine,
    masked: &str,
    position: crate::lsp_types::Position,
    identifier: &str,
) -> Option<&'a rossi::Event> {
    let line_idx = position.line as usize;
    machine.events.iter().find(|event| {
        // `masked` is masked once by the caller; scanning it per event avoids
        // re-masking the whole document each time. Events do not overlap, so the
        // first event whose range contains the line is the enclosing one.
        event_line_range_in(masked, &event.name)
            .is_some_and(|(start, end)| (start..=end).contains(&line_idx))
            && event
                .parameters
                .iter()
                .any(|parameter| parameter.name == identifier)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp_types::Position;
    use rossi::Component;

    fn machine_of(src: &str) -> rossi::Machine {
        match rossi::parse(src).expect("parses") {
            Component::Machine(machine) => machine,
            other => panic!("expected a machine, got {other:?}"),
        }
    }

    #[test]
    fn event_parameter_at_position_scopes_to_the_enclosing_event() {
        // `p` is the ANY parameter of event `e` (lines 4..=11); `k` only appears
        // in the guard text and is not a parameter.
        let src = "MACHINE m\nVARIABLES\n    v\nEVENTS\n  EVENT e\n  ANY\n    p\n  WHERE\n    p > k\n  THEN\n    v := 0\n  END\nEND";
        let masked = rossi::comments::mask_comments_chars(src);
        let machine = machine_of(src);

        // `p` on the guard line resolves to its event.
        let event = event_parameter_at_position(&machine, &masked, Position::new(8, 4), "p");
        assert_eq!(event.map(|e| e.name.as_str()), Some("e"));

        // `k` is in range but is not an ANY parameter.
        assert!(event_parameter_at_position(&machine, &masked, Position::new(8, 8), "k").is_none());

        // The real parameter name `p` on a line outside event `e`'s range
        // (line 2 is in the VARIABLES block) still resolves to nothing —
        // parameters are scoped to their event's line range, and only
        // `position.line` is consulted, so the column is irrelevant here.
        assert!(event_parameter_at_position(&machine, &masked, Position::new(2, 4), "p").is_none());
    }
}
