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

/// Whether `event`'s line range contains `line_idx`, scanned over comment-masked
/// text (an `EVENT`/`END` spelled in a comment cannot bound the range). The
/// shared containment check behind [`event_parameter_at_position`] (and, once
/// completion needs it, the enclosing-event lookup), so callers cannot disagree
/// on what counts as "inside this event".
fn event_contains_line(masked: &str, event: &rossi::Event, line_idx: usize) -> bool {
    // `masked` is masked once by the caller; scanning it per event avoids
    // re-masking the whole document each time.
    event_line_range_in(masked, &event.name)
        .is_some_and(|(start, end)| (start..=end).contains(&line_idx))
}

/// The event whose `ANY` clause declares `identifier`, when `position` falls
/// inside that event's line range. Returns `None` when the cursor is not inside
/// any event or the identifier is not one of its parameters.
///
/// Scans the events whose range contains the cursor line and returns the first
/// that declares `identifier`. Usually only one event encloses a line, but error
/// recovery can leave an unterminated event's range overlapping a later
/// sibling's, so more than one can match; picking the event that actually
/// declares the parameter keeps hover / find-references / rename resolving the
/// inner event's own parameter rather than giving up on the outer one.
pub(crate) fn event_parameter_at_position<'a>(
    machine: &'a rossi::Machine,
    masked: &str,
    position: crate::lsp_types::Position,
    identifier: &str,
) -> Option<&'a rossi::Event> {
    let line_idx = position.line as usize;
    machine
        .events
        .iter()
        .filter(|event| event_contains_line(masked, event, line_idx))
        .find(|event| event.parameters.iter().any(|p| p.name == identifier))
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

    #[test]
    fn event_parameter_resolves_inner_event_when_recovery_overlaps_ranges() {
        // A missing `END` on `e1` makes its recovered line range swallow `e2`, so
        // both events' ranges contain the cursor on `e2`'s parameter line. The
        // resolver must still return the event that actually declares the
        // parameter, not give up because the first (outer) event lacks it.
        let src = "MACHINE m\nVARIABLES\n    v\nEVENTS\n  EVENT e1\n  ANY\n    p1\n  THEN\n    @act1 v := 0\n  EVENT e2\n  ANY\n    p2\n  THEN\n    @act2 v := 1\n  END\nEND";
        let masked = rossi::comments::mask_comments_chars(src);
        let parsed = rossi::parse_components_with_recovery(src);
        let components = parsed.component.as_deref().expect("recovers components");
        let Component::Machine(machine) = &components[0] else {
            panic!("expected a machine, got {:?}", components[0]);
        };
        assert_eq!(machine.events.len(), 2, "recovery keeps both events");

        // `p2` on its line (index 11) is inside both e1's and e2's ranges, yet
        // resolves to `e2` — the event that declares it.
        let event = event_parameter_at_position(machine, &masked, Position::new(11, 4), "p2");
        assert_eq!(event.map(|e| e.name.as_str()), Some("e2"));
    }
}
