//! Rewriting a `.bpr` proof file entry by entry.
//!
//! Proof maintenance either drops a stored proof (Rodin's *Proof
//! Purger*: the obligation it was recorded against is gone) or empties
//! one (the *POCleaner* plug-in: the stored proof no longer opens).
//! Both are decisions about whole `prProof` elements, so this pass
//! copies the document verbatim and acts only at their boundaries — it
//! never parses a formula, resolves an intern reference, or consults
//! the reasoner registry.
//!
//! That is the point rather than an economy: the proofs worth
//! rewriting are exactly the ones [`crate::bpr`] reports as
//! [`ProofBody::Unsupported`], so a rewriter built on the typed model
//! could not fix them. Everything outside the entries the caller acts
//! on — whitespace, comments, attribute spelling and escaping included
//! — comes out byte for byte as it went in.
//!
//! Emptying an entry reproduces what Rodin writes when a fresh,
//! untouched proof attempt is committed. `PRProof.doSetProofTree`
//! clears the element and then returns early, because an unmodified
//! tree's confidence is `IConfidence.UNATTEMPTED`; `clear` removes
//! every registered attribute and every child, and the XML `name`
//! survives only because it is Rodin's handle slot rather than an
//! attribute type. The element is emptied, never deleted:
//!
//! ```xml
//! <org.eventb.core.prProof name="INITIALISATION/Inv1/INV"/>
//! ```
//!
//! [`ProofBody::Unsupported`]: crate::bpr::ProofBody::Unsupported

use std::io::{BufRead, Write};

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer};

use crate::bpr::{BprError, NAME, PR_FILE, PR_PROOF};
use crate::xml::{attr, attrs, get};

/// What [`rewrite_bpr`] does with one proof entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofAction {
    /// Copy the entry and its subtree through unchanged.
    Keep,
    /// Delete the entry outright — the purger's action for a proof
    /// whose obligation no longer exists.
    Drop,
    /// Replace the entry with its emptied form, keeping only `name`.
    Reset,
}

/// What one [`rewrite_bpr`] pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RewriteStats {
    /// Entries copied through unchanged.
    pub kept: usize,
    /// Entries deleted.
    pub dropped: usize,
    /// Entries emptied.
    pub reset: usize,
}

impl RewriteStats {
    /// Whether the pass changed the document.
    pub fn changed(&self) -> bool {
        self.dropped > 0 || self.reset > 0
    }

    /// How many entries the rewritten document holds.
    pub fn remaining(&self) -> usize {
        self.kept + self.reset
    }
}

/// The pass's position in the document.
#[derive(Default)]
struct State {
    /// `Some(depth)` while swallowing a dropped or emptied entry's
    /// subtree, counting the elements open below the entry itself.
    swallowing: Option<usize>,
    /// Whether the `prFile` root has been seen and validated.
    saw_root: bool,
    /// Elements open outside a swallowed subtree, the root included;
    /// back to zero once the root closes.
    open: usize,
    /// Whether the whitespace trailing the current entry goes with it.
    /// A dropped entry takes it, so purging a thousand proofs leaves
    /// no thousand blank lines behind; an emptied one stays where it
    /// was and must keep the layout around it.
    take_ws: bool,
}

/// Copies a `.bpr` document from `reader` to `writer`, applying
/// `decide` to each proof entry by obligation name.
///
/// The root element is validated exactly as [`crate::bpr::visit_bpr`]
/// validates it, so a file that is not a version-1 proof file is
/// refused rather than rewritten, and a document that ends with
/// elements still open fails as [`BprError::Truncated`] — a truncated
/// input must not come out as a shorter but well-formed file. On any
/// error the output is incomplete and must be discarded.
///
/// Proof trees nest arbitrarily deep and these files reach hundreds of
/// megabytes, so this is a single streaming pass driven by a depth
/// counter: no recursion, and no whole-document buffering. `writer` is
/// written but never flushed; a buffered caller flushes its own.
pub fn rewrite_bpr(
    reader: impl BufRead,
    writer: impl Write,
    mut decide: impl FnMut(&str) -> ProofAction,
) -> Result<RewriteStats, BprError> {
    let mut xml = Reader::from_reader(reader);
    let mut out = Writer::new(writer);
    let mut buf = Vec::new();
    let mut stats = RewriteStats::default();
    let mut state = State::default();

    loop {
        let event = xml.read_event_into(&mut buf)?;

        // The whitespace a dropped entry takes with it is the run
        // that follows the entry, never one inside a subtree still
        // being swallowed.
        if state.swallowing.is_none() {
            let skip = state.take_ws
                && matches!(&event, Event::Text(t) if t.iter().all(u8::is_ascii_whitespace));
            state.take_ws = false;
            if skip {
                drop(event);
                buf.clear();
                continue;
            }
        }

        match event {
            Event::Start(e) => open(&e, false, &mut out, &mut decide, &mut state, &mut stats)?,
            Event::Empty(e) => open(&e, true, &mut out, &mut decide, &mut state, &mut stats)?,
            Event::End(e) => match state.swallowing {
                // The entry's own end tag: stop swallowing, write
                // nothing.
                Some(0) => state.swallowing = None,
                Some(depth) => state.swallowing = Some(depth - 1),
                None => {
                    state.open -= 1;
                    out.write_event(Event::End(e))?;
                }
            },
            Event::Eof => {
                // quick-xml reports a bare EOF even with elements
                // still open.
                if !(state.saw_root && state.open == 0) {
                    return Err(BprError::Truncated);
                }
                break;
            }
            other => {
                if state.swallowing.is_none() {
                    out.write_event(other)?;
                }
            }
        }
        buf.clear();
    }

    Ok(stats)
}

/// A new `.bpr` document holding the emptied form of an entry for each
/// of `names`, in order, laid out as Rodin writes the file: what Rodin's
/// proof-status updater leaves for an obligation it has no stored proof
/// for (`PSUpdater.updateStatus` creates the element and commits nothing
/// to it). Without that element, Rodin's prover logs an error each time
/// it opens the obligation.
pub fn unattempted_bpr<'a>(names: impl IntoIterator<Item = &'a str>) -> String {
    let mut doc = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n<{PR_FILE} version=\"1\">\n"
    );
    for name in names {
        doc.push_str(&format!(
            "    <{PR_PROOF} {NAME}=\"{}\"/>\n",
            quick_xml::escape::escape(name)
        ));
    }
    doc.push_str(&format!("</{PR_FILE}>\n"));
    doc
}

/// Handles one start tag, `empty` for the self-closing form.
fn open<W: Write>(
    e: &BytesStart<'_>,
    empty: bool,
    out: &mut Writer<W>,
    decide: &mut impl FnMut(&str) -> ProofAction,
    state: &mut State,
    stats: &mut RewriteStats,
) -> Result<(), BprError> {
    // Inside a swallowed entry nothing is written; only the depth
    // matters, and a self-closing element opens nothing.
    if let Some(depth) = state.swallowing.as_mut() {
        if !empty {
            *depth += 1;
        }
        return Ok(());
    }

    let name = e.name();
    let name = name.as_ref();

    if !state.saw_root {
        if name != PR_FILE.as_bytes() {
            return Err(BprError::Unsupported(format!(
                "root element {}",
                String::from_utf8_lossy(name)
            )));
        }
        let attrs = attrs(e);
        if get(&attrs, "version") != Some("1") {
            return Err(BprError::Unsupported(format!(
                "file version {:?}",
                get(&attrs, "version")
            )));
        }
        // A self-closing root is a complete, proof-less file: it
        // opens nothing, so `open` stays at zero and the document
        // already reads as closed.
        state.saw_root = true;
        if !empty {
            state.open += 1;
        }
        return write_open(e, empty, out);
    }

    // A `prProof` nests nowhere else, so reaching one here always
    // means a fresh entry.
    if name == PR_PROOF.as_bytes() {
        // An entry with no name cannot be selected against, so it is
        // never one the caller asked for.
        let action = match attr(e, NAME) {
            Some(po) => decide(&po),
            None => ProofAction::Keep,
        };
        match action {
            ProofAction::Keep => {
                stats.kept += 1;
                if !empty {
                    state.open += 1;
                }
                return write_open(e, empty, out);
            }
            ProofAction::Drop => {
                stats.dropped += 1;
                state.take_ws = true;
            }
            ProofAction::Reset => {
                stats.reset += 1;
                // Re-emit `name` from its raw bytes rather than the
                // decoded string, so the entry keeps whatever
                // escaping it arrived with.
                let mut reset = BytesStart::new(PR_PROOF);
                for attr in e.attributes().flatten() {
                    if attr.key.as_ref() == NAME.as_bytes() {
                        reset.push_attribute(attr);
                    }
                }
                out.write_event(Event::Empty(reset))?;
            }
        }
        // Whatever stood under the entry goes unwritten; a
        // self-closing one has nothing to swallow.
        if !empty {
            state.swallowing = Some(0);
        }
        return Ok(());
    }

    if !empty {
        state.open += 1;
    }
    write_open(e, empty, out)
}

/// Writes a start tag back in the form it was read.
fn write_open<W: Write>(
    e: &BytesStart<'_>,
    empty: bool,
    out: &mut Writer<W>,
) -> Result<(), BprError> {
    let e = e.borrow();
    out.write_event(if empty {
        Event::Empty(e)
    } else {
        Event::Start(e)
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-proof file: the first entry already emptied, the second
    /// a full proof with a nested rule tree.
    const DOC: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.prFile version="1">
    <org.eventb.core.prProof name="evt/inv1/INV"/>
    <org.eventb.core.prProof name="evt/inv2/INV" org.eventb.core.confidence="1000" org.eventb.core.prGoal="p0">
        <org.eventb.core.prRule name="r0" org.eventb.core.confidence="1000">
            <org.eventb.core.prAnte name="'">
                <org.eventb.core.prHypAction name="HYPACTION0"/>
            </org.eventb.core.prAnte>
        </org.eventb.core.prRule>
        <org.eventb.core.prPred name="p0" org.eventb.core.predicate="x&gt;1"/>
    </org.eventb.core.prProof>
</org.eventb.core.prFile>"#;

    fn run(doc: &str, decide: impl FnMut(&str) -> ProofAction) -> (String, RewriteStats) {
        let mut out = Vec::new();
        let stats = rewrite_bpr(doc.as_bytes(), &mut out, decide).expect("rewritable");
        (String::from_utf8(out).expect("utf-8"), stats)
    }

    #[test]
    fn keeping_everything_is_byte_identical() {
        let (out, stats) = run(DOC, |_| ProofAction::Keep);
        assert_eq!(out, DOC);
        assert_eq!(
            stats,
            RewriteStats {
                kept: 2,
                dropped: 0,
                reset: 0
            }
        );
        assert!(!stats.changed());
        assert_eq!(stats.remaining(), 2);
    }

    #[test]
    fn dropping_removes_the_entry_and_its_subtree() {
        let (out, stats) = run(DOC, |name| match name {
            "evt/inv2/INV" => ProofAction::Drop,
            _ => ProofAction::Keep,
        });
        assert!(!out.contains("prRule"), "{out}");
        assert!(!out.contains("evt/inv2/INV"), "{out}");
        assert!(out.contains(r#"<org.eventb.core.prProof name="evt/inv1/INV"/>"#));
        assert_eq!(
            stats,
            RewriteStats {
                kept: 1,
                dropped: 1,
                reset: 0
            }
        );
        assert!(stats.changed());
        assert_eq!(stats.remaining(), 1);
    }

    #[test]
    fn resetting_keeps_only_the_name() {
        let (out, stats) = run(DOC, |name| match name {
            "evt/inv2/INV" => ProofAction::Reset,
            _ => ProofAction::Keep,
        });
        assert!(
            out.contains(r#"<org.eventb.core.prProof name="evt/inv2/INV"/>"#),
            "{out}"
        );
        assert!(!out.contains("confidence"), "{out}");
        assert!(!out.contains("prRule"), "{out}");
        assert_eq!(stats.reset, 1);
        assert_eq!(stats.remaining(), 2);
    }

    #[test]
    fn acts_on_an_entry_that_arrives_self_closing() {
        let (dropped, stats) = run(DOC, |name| match name {
            "evt/inv1/INV" => ProofAction::Drop,
            _ => ProofAction::Keep,
        });
        assert!(!dropped.contains("evt/inv1/INV"), "{dropped}");
        assert!(dropped.contains("prRule"), "{dropped}");
        assert_eq!(stats.dropped, 1);

        // Resetting an already-empty entry reproduces it unchanged.
        let (reset, stats) = run(DOC, |name| match name {
            "evt/inv1/INV" => ProofAction::Reset,
            _ => ProofAction::Keep,
        });
        assert_eq!(reset, DOC);
        assert_eq!(stats.reset, 1);
    }

    #[test]
    fn dropping_takes_the_whitespace_that_followed() {
        let (out, _) = run(DOC, |_| ProofAction::Drop);
        assert_eq!(
            out,
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n\
             <org.eventb.core.prFile version=\"1\">\n    </org.eventb.core.prFile>"
        );
    }

    /// An emptied entry stays where it was, so the layout around it
    /// must not move with the subtree that went.
    #[test]
    fn resetting_keeps_the_whitespace_that_followed() {
        let (out, _) = run(DOC, |_| ProofAction::Reset);
        assert_eq!(
            out,
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n\
             <org.eventb.core.prFile version=\"1\">\n\
             \x20   <org.eventb.core.prProof name=\"evt/inv1/INV\"/>\n\
             \x20   <org.eventb.core.prProof name=\"evt/inv2/INV\"/>\n\
             </org.eventb.core.prFile>"
        );
    }

    /// The reset form must be Rodin's: `PRProof.doSetProofTree` clears
    /// every registered attribute and child, and only the handle name
    /// survives.
    #[test]
    fn reset_matches_rodins_emptied_entry() {
        let doc = r#"<org.eventb.core.prFile version="1"><org.eventb.core.prProof name="a/b/INV" org.eventb.core.confidence="1000" org.eventb.core.psManual="true" org.eventb.core.prSets="S"><org.eventb.core.lang name="L"/></org.eventb.core.prProof></org.eventb.core.prFile>"#;
        let (out, _) = run(doc, |_| ProofAction::Reset);
        assert_eq!(
            out,
            r#"<org.eventb.core.prFile version="1"><org.eventb.core.prProof name="a/b/INV"/></org.eventb.core.prFile>"#
        );
    }

    #[test]
    fn an_entry_name_keeps_its_escaping() {
        let doc = r#"<org.eventb.core.prFile version="1"><org.eventb.core.prProof name="a&amp;b/INV" org.eventb.core.confidence="1000"/></org.eventb.core.prFile>"#;
        let mut seen = Vec::new();
        let mut out = Vec::new();
        rewrite_bpr(doc.as_bytes(), &mut out, |name| {
            seen.push(name.to_string());
            ProofAction::Reset
        })
        .expect("rewritable");
        // The decision sees the decoded name, the document keeps the
        // escaped one.
        assert_eq!(seen, ["a&b/INV"]);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            r#"<org.eventb.core.prFile version="1"><org.eventb.core.prProof name="a&amp;b/INV"/></org.eventb.core.prFile>"#
        );
    }

    /// Rule trees nest arbitrarily deep, so the pass must not recurse.
    #[test]
    fn deep_nesting_does_not_grow_the_stack() {
        const DEPTH: usize = 60_000;
        let mut doc = String::from(
            r#"<org.eventb.core.prFile version="1"><org.eventb.core.prProof name="deep">"#,
        );
        for _ in 0..DEPTH {
            doc.push_str(r#"<org.eventb.core.prAnte name="'">"#);
        }
        for _ in 0..DEPTH {
            doc.push_str("</org.eventb.core.prAnte>");
        }
        doc.push_str("</org.eventb.core.prProof></org.eventb.core.prFile>");

        let (kept, _) = run(&doc, |_| ProofAction::Keep);
        assert_eq!(kept, doc);
        let (dropped, stats) = run(&doc, |_| ProofAction::Drop);
        assert_eq!(
            dropped,
            r#"<org.eventb.core.prFile version="1"></org.eventb.core.prFile>"#
        );
        assert_eq!(stats.dropped, 1);
    }

    /// The rewriter parses no formula, so a proof `bpr.rs` refuses is
    /// still rewritable — which is the whole point of resetting.
    #[test]
    fn rewrites_a_proof_the_reader_cannot_represent() {
        let doc = r#"<org.eventb.core.prFile version="1"><org.eventb.core.prProof name="old/INV" org.eventb.core.confidence="1000"><org.eventb.core.lang name="L">extended</org.eventb.core.lang><some.old.reasoner name="r0"/></org.eventb.core.prProof></org.eventb.core.prFile>"#;
        let entries = crate::bpr::read_bpr(doc.as_bytes(), |_| crate::bpr::Keep::Full)
            .expect("the file itself reads");
        assert!(
            matches!(entries[0].body, crate::bpr::ProofBody::Unsupported(_)),
            "expected an unsupported proof, got {:?}",
            entries[0].body
        );

        let (out, stats) = run(doc, |_| ProofAction::Reset);
        assert_eq!(
            out,
            r#"<org.eventb.core.prFile version="1"><org.eventb.core.prProof name="old/INV"/></org.eventb.core.prFile>"#
        );
        assert_eq!(stats.reset, 1);
    }

    #[test]
    fn a_proofless_root_round_trips() {
        for doc in [
            r#"<org.eventb.core.prFile version="1"/>"#,
            r#"<org.eventb.core.prFile version="1"></org.eventb.core.prFile>"#,
        ] {
            let (out, stats) = run(doc, |_| ProofAction::Drop);
            assert_eq!(out, doc);
            assert_eq!(stats, RewriteStats::default());
        }
    }

    #[test]
    fn rejects_a_document_that_is_not_a_proof_file() {
        let mut out = Vec::new();
        let err = rewrite_bpr(r#"<org.eventb.core.psFile/>"#.as_bytes(), &mut out, |_| {
            ProofAction::Keep
        })
        .expect_err("not a proof file");
        assert!(matches!(err, BprError::Unsupported(_)), "{err:?}");

        let mut out = Vec::new();
        let err = rewrite_bpr(
            r#"<org.eventb.core.prFile version="2"/>"#.as_bytes(),
            &mut out,
            |_| ProofAction::Keep,
        )
        .expect_err("unknown version");
        assert!(matches!(err, BprError::Unsupported(_)), "{err:?}");
    }

    #[test]
    fn a_truncated_document_fails_rather_than_shortening() {
        let mut out = Vec::new();
        let err = rewrite_bpr(
            r#"<org.eventb.core.prFile version="1"><org.eventb.core.prProof name="a"/>"#.as_bytes(),
            &mut out,
            |_| ProofAction::Keep,
        )
        .expect_err("truncated");
        assert!(matches!(err, BprError::Truncated), "{err:?}");
    }

    #[test]
    fn a_new_file_holds_one_emptied_entry_per_obligation() {
        let doc = unattempted_bpr(["evt/inv1/INV", "a<b&c"]);
        assert_eq!(
            doc,
            r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<org.eventb.core.prFile version="1">
    <org.eventb.core.prProof name="evt/inv1/INV"/>
    <org.eventb.core.prProof name="a&lt;b&amp;c"/>
</org.eventb.core.prFile>
"#
        );
        let entries =
            crate::bpr::read_bpr(doc.as_bytes(), |_| crate::bpr::Keep::Deps).expect("readable");
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["evt/inv1/INV", "a<b&c"]);
        assert!(entries.iter().all(|e| e.confidence.is_none() && !e.manual));
    }
}
