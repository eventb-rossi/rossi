//! Reconcile regenerated proof-obligation files with a previous build.
//!
//! Regeneration alone resets all proof state: every stamp restarts at
//! `"0"` and every status becomes unattempted, so downstream provers
//! would treat identical obligations as new work. This module merges
//! the freshly generated `.bpo` / `.bps` pair with the files it is
//! about to replace:
//!
//! - a sequent (or top-level predicate set) that is semantically
//!   unchanged — per [`PoView::sequent_eq`] / [`PoView::set_chain_eq`]
//!   — carries its previous `poStamp` forward verbatim; changed or new
//!   elements get a stamp above every stamp in the previous file;
//! - a status row whose obligation still exists is re-emitted with all
//!   its attributes untouched (confidence, `psManual`, `psBroken`,
//!   its recorded stamp); rows for vanished obligations are dropped
//!   and new obligations get fresh unattempted rows;
//! - when nothing changed at all, the previous bytes pass through
//!   verbatim, so rebuilding an unchanged model is byte-stable;
//! - the declarations Rodin writes in hash order keep the order the
//!   previous copy of each file had ([`preserve_child_order`]), in the
//!   checked files as much as here, since Rodin's builder compares an
//!   element's children positionally.
//!
//! A status row whose recorded stamp differs from its sequent's stamp
//! is exactly the signal proof managers use to re-check the stored
//! proof, so no proof-dependency analysis happens here.
//!
//! The module also owns the `.bps` byte format itself — the row
//! spelling ([`fresh_status_row`]) and the surgery on it
//! ([`reset_status_rows`]) that `rossi clean` performs outside any
//! build, since a second writer elsewhere would be a second source of
//! truth for the format.

use std::collections::{BTreeSet, HashMap, HashSet};

use quick_xml::events::{BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};

use crate::ScFile;
use crate::po_view::PoView;
use rossi_prove::confidence::Bucket;

use crate::proofs::cap_if_broken;
use crate::xml_out::{DOC_HEADER, attr, tag as xtag};

/// Reconcile every generated `.bpo` / `.bps` pair in `files` against
/// the previous contents supplied by `old` (keyed by the generated
/// filename; `None` when no previous file exists), and keep every
/// `.bcc` / `.bcm` in the declaration order of its previous copy.
///
/// Returns, per `.bps` filename, the names of the rows that were
/// synthesized fresh rather than carried from a previous row — the
/// rows [`super::status::update_statuses`] may revive from stored
/// proofs (a missing status is computed from the proof file, but
/// never touches a recorded stamp-valid row).
pub fn reconcile_build_files(
    files: &mut [ScFile],
    mut old: impl FnMut(&str) -> Option<String>,
) -> HashMap<String, HashSet<String>> {
    for file in files.iter_mut() {
        if (file.filename.ends_with(".bcc") || file.filename.ends_with(".bcm"))
            && let Some(previous) = old(&file.filename)
        {
            file.contents = preserve_child_order(&file.contents, &previous);
        }
    }
    let mut synthesized = HashMap::new();
    for (i, j) in bpo_bps_pairs(files) {
        let old_bpo = old(&files[i].filename);
        let old_bps = old(&files[j].filename);
        let carried: HashSet<String> = old_bps
            .as_deref()
            .map(|bps| parse_status_rows(bps).into_iter().map(|r| r.name).collect())
            .unwrap_or_default();
        let (bpo_out, bps_out) = reconcile_pair(
            old_bpo.as_deref(),
            old_bps.as_deref(),
            &files[i].contents,
            &files[j].contents,
        );
        files[i].contents = bpo_out;
        files[j].contents = bps_out;
        let fresh: HashSet<String> = parse_status_rows(&files[j].contents)
            .into_iter()
            .map(|row| row.name)
            .filter(|name| !carried.contains(name))
            .collect();
        synthesized.insert(files[j].filename.clone(), fresh);
    }
    synthesized
}

/// The `(bpo index, bps index)` of every same-stem `.bpo` / `.bps` pair —
/// the one implementation of the pairing rule, shared with
/// [`reset_stale_statuses`]. Indices, so callers keep their `&mut` access.
pub(crate) fn bpo_bps_pairs(files: &[ScFile]) -> Vec<(usize, usize)> {
    files
        .iter()
        .enumerate()
        .filter_map(|(i, file)| {
            let stem = file.filename.strip_suffix(".bpo")?;
            let bps_name = format!("{stem}.bps");
            let j = files.iter().position(|f| f.filename == bps_name)?;
            Some((i, j))
        })
        .collect()
}

/// Reconcile one component's generated `.bpo` / `.bps` contents with
/// the previous files they replace. Returns the contents to write.
pub fn reconcile_pair(
    old_bpo: Option<&str>,
    old_bps: Option<&str>,
    new_bpo: &str,
    new_bps: &str,
) -> (String, String) {
    if old_bpo.is_none() && old_bps.is_none() {
        return (new_bpo.to_string(), new_bps.to_string());
    }

    // Without a previous `.bpo` there are no stamps to carry and no
    // stub to detect, so the generated file needs no parsing at all.
    let mut plan = None;
    if let Some(old_bpo) = old_bpo {
        let Ok(new_view) = PoView::from_xml(new_bpo) else {
            return (new_bpo.to_string(), new_bps.to_string());
        };
        // Only a decomposition-stub component generates a file with no
        // predicate sets at all (real components always emit at least
        // the hypothesis roots). Don't wipe real previous state with a
        // stub.
        if new_view.sets.is_empty() && new_view.sequents.is_empty() {
            return (old_bpo.to_string(), old_bps.unwrap_or(new_bps).to_string());
        }
        plan = PoView::from_xml(old_bpo)
            .ok()
            .map(|old| plan_stamps(&old, &new_view));
    }

    // The stamp plan compares sets and sequents order-blind, so the
    // previous file's declaration order is restored only where the
    // generated text is emitted, never for the unchanged pass-through.
    let reordered = || preserve_child_order(new_bpo, old_bpo.unwrap_or(""));
    let bpo_out = match &plan {
        Some(plan) if plan.all_unchanged => old_bpo.expect("plan implies old file").to_string(),
        Some(plan) => rewrite_stamps(&reordered(), plan),
        None => reordered(),
    };

    let old_rows = old_bps.map(parse_status_rows).unwrap_or_default();
    let new_rows = parse_status_rows(new_bps);
    let unchanged = plan.as_ref().is_some_and(|plan| plan.all_unchanged);
    let bps_out = match old_bps {
        Some(old_bps) if unchanged && same_names(&old_rows, &new_rows) => old_bps.to_string(),
        _ => {
            let carried: HashMap<&str, &str> = old_rows
                .iter()
                .map(|row| (row.name.as_str(), row.row.as_str()))
                .collect();
            let rows: Vec<String> = new_rows
                .iter()
                .map(|row| match carried.get(row.name.as_str()) {
                    Some(old_row) => (*old_row).to_string(),
                    None => {
                        let stamp = plan
                            .as_ref()
                            .and_then(|plan| plan.sequents.get(&row.name))
                            .map_or("0", String::as_str);
                        replace_stamp(&row.row, stamp)
                    }
                })
                .collect();
            assemble_status(&rows)
        }
    };
    (bpo_out, bps_out)
}

fn same_names(old_rows: &[StatusRow], new_rows: &[StatusRow]) -> bool {
    let old: BTreeSet<&str> = old_rows.iter().map(|row| row.name.as_str()).collect();
    let new: BTreeSet<&str> = new_rows.iter().map(|row| row.name.as_str()).collect();
    old == new
}

/// Reset stale proof statuses after [`reconcile_build_files`]: a `.bps`
/// row whose recorded `poStamp` differs from its sequent's stamp in the
/// sibling `.bpo` records a proof of an obligation that has since
/// changed. Rodin uses exactly that divergence as its replay signal, but
/// a consumer that cannot replay proofs (eventb-animate's po gate is
/// stamp-blind) must not trust the carried confidence — so every such
/// row is replaced by a fresh unattempted one (confidence `-99`, the
/// sequent's stamp, `psManual="false"`), dropping `psBroken` and any
/// other carried attribute. Stamp-matched rows keep their exact bytes.
///
/// Returns `(bpo filename, open count)` per pair, where open = sequents
/// minus the rows that are stamp-valid and classify as discharged
/// (broken caps a high confidence) — exactly the rows the PO gate skips.
///
/// Deliberately separate from [`reconcile_build_files`]: the persistent
/// Rodin-project writers (LSP rebuild, CLI build, repack) must carry
/// rows verbatim so Rodin itself sees the divergence; they never call
/// this.
pub fn reset_stale_statuses(files: &mut [ScFile]) -> Vec<(String, usize)> {
    let mut counts = Vec::new();
    for (i, j) in bpo_bps_pairs(files) {
        let stamps = sequent_stamps(&files[i].contents);
        let rows = parse_status_rows(&files[j].contents);
        let stamp_valid = |row: &StatusRow| {
            stamps
                .get(&row.name)
                .is_some_and(|stamp| row.stamp.as_deref() == Some(stamp.as_str()))
        };

        // `max` keeps the count conservative when a malformed `.bpo` scan
        // yields fewer sequents than there are rows: those rows all reset
        // and must count as open, not vanish from the total.
        let total = stamps.len().max(rows.len());
        let skipped = rows
            .iter()
            .filter(|row| {
                stamp_valid(row)
                    && cap_if_broken(
                        rossi_prove::Confidence::classify(row.confidence),
                        row.broken,
                    ) == Bucket::Discharged
            })
            .count();

        if rows.iter().any(|row| !stamp_valid(row)) {
            let out: Vec<String> = rows
                .iter()
                .map(|row| {
                    if stamp_valid(row) {
                        row.row.clone()
                    } else {
                        fresh_status_row(
                            &row.name,
                            stamps.get(&row.name).map_or("0", String::as_str),
                        )
                    }
                })
                .collect();
            files[j].contents = assemble_status(&out);
        }
        counts.push((files[i].filename.clone(), total.saturating_sub(skipped)));
    }
    counts
}

/// The `name → poStamp` of every sequent in a `.bpo`, by attribute scan —
/// no formula parsing, unlike [`PoView`], so a Rodin-written predicate our
/// parser cannot reparse never voids the stamps (which would reset every
/// recorded discharge). A sequent without a stamp reads as `"0"`, the
/// generator's default. Malformed XML stops the scan; the missing entries
/// then read as open and their rows reset — conservative, never unsafe.
pub fn sequent_stamps(bpo: &str) -> HashMap<String, String> {
    let mut reader = Reader::from_str(bpo);
    let mut buf = Vec::new();
    let mut stamps = HashMap::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e))
                if e.name().as_ref() == xtag::PO_SEQUENT.as_bytes() =>
            {
                let mut name = None;
                let mut stamp = None;
                for attr in e.attributes().flatten() {
                    let unescaped = || {
                        let raw = String::from_utf8_lossy(&attr.value);
                        match quick_xml::escape::unescape(&raw) {
                            Ok(cow) => cow.into_owned(),
                            Err(_) => raw.into_owned(),
                        }
                    };
                    if attr.key.as_ref() == attr::NAME.as_bytes() {
                        name = Some(unescaped());
                    } else if attr.key.as_ref() == attr::PO_STAMP.as_bytes() {
                        stamp = Some(unescaped());
                    }
                }
                if let Some(name) = name {
                    stamps.insert(name, stamp.unwrap_or_else(|| "0".into()));
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    stamps
}

/// A fresh unattempted status row in the generator's exact shape
/// (`pog::model::into_sc_files`), escaped by the same
/// [`crate::xml_out::escape_attr`] the generator uses. Public so the
/// golden gates fabricate expectations from the same source of truth.
pub fn fresh_status_row(name: &str, stamp: &str) -> String {
    let mut row = format!("<{} {}=\"", xtag::PS_STATUS, attr::NAME);
    crate::xml_out::escape_attr(name, &mut row);
    row.push_str(&format!(
        "\" {}=\"-99\" {}=\"{stamp}\" {}=\"false\"/>",
        attr::CONFIDENCE,
        attr::PO_STAMP,
        attr::PS_MANUAL,
    ));
    row
}

/// Rewrites as fresh unattempted rows the status rows `reset` names,
/// copying the rest of the document verbatim.
///
/// This is `rossi clean`'s other half: emptying a stored proof must
/// leave its obligation unattempted, or the stale row would still
/// claim it discharged. It is surgery rather than a regeneration
/// because a `.bps` Rodin wrote is laid out differently from a
/// generated one, and a maintenance pass should move only what it was
/// asked to.
///
/// A row's new stamp comes from its obligation where `stamps` knows it
/// — Rodin's own does, a proof attempt capturing the PO sequent's
/// stamp when it is created — and otherwise from the row being
/// replaced. Keeping the stamp is what makes the emptied state stick:
/// Rodin's status update skips any row whose stamp still matches.
pub fn reset_status_rows(
    bps: &str,
    reset: &BTreeSet<String>,
    stamps: &HashMap<String, String>,
) -> Result<String, quick_xml::Error> {
    /// The replacement for `e`, when it is a row named in `reset`.
    fn replacement(
        e: &BytesStart<'_>,
        reset: &BTreeSet<String>,
        stamps: &HashMap<String, String>,
    ) -> Option<String> {
        if e.name().as_ref() != xtag::PS_STATUS.as_bytes() {
            return None;
        }
        let name = attribute(e, attr::NAME).filter(|name| reset.contains(name))?;
        let stamp = stamps
            .get(&name)
            .cloned()
            .or_else(|| attribute(e, attr::PO_STAMP))
            .unwrap_or_else(|| "0".into());
        Some(fresh_status_row(&name, &stamp))
    }

    let mut reader = Reader::from_str(bps);
    let mut out = Writer::new(Vec::with_capacity(bps.len()));
    let mut buf = Vec::new();
    // The elements still open below a replaced row. A status row is
    // always self-closing in practice, but `clean` runs on whatever
    // the user hands it, and a row written with a separate end tag
    // would otherwise leave that tag stranded after the replacement.
    let mut swallow: Option<usize> = None;
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => {
                if let Some(depth) = swallow.as_mut() {
                    *depth += 1;
                } else if let Some(row) = replacement(&e, reset, stamps) {
                    out.write_event(Event::Text(BytesText::from_escaped(row)))?;
                    swallow = Some(0);
                } else {
                    out.write_event(Event::Start(e))?;
                }
            }
            Event::Empty(e) if swallow.is_none() => {
                if let Some(row) = replacement(&e, reset, stamps) {
                    out.write_event(Event::Text(BytesText::from_escaped(row)))?;
                } else {
                    out.write_event(Event::Empty(e))?;
                }
            }
            Event::End(e) => match swallow {
                Some(0) => swallow = None,
                Some(depth) => swallow = Some(depth - 1),
                None => out.write_event(Event::End(e))?,
            },
            Event::Eof => break,
            other => {
                if swallow.is_none() {
                    out.write_event(other)?;
                }
            }
        }
        buf.clear();
    }
    Ok(String::from_utf8(out.into_inner()).expect("events of a &str document are UTF-8"))
}

/// One attribute of a start tag, unescaped.
fn attribute(e: &BytesStart<'_>, key: &str) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        (a.key.as_ref() == key.as_bytes()).then(|| {
            let raw = String::from_utf8_lossy(&a.value);
            match quick_xml::escape::unescape(&raw) {
                Ok(cow) => cow.into_owned(),
                Err(_) => raw.into_owned(),
            }
        })
    })
}

/// The stamp each output element should carry.
struct StampPlan {
    /// True iff the old and new files have the same sets and sequents
    /// and every one of them is semantically unchanged.
    all_unchanged: bool,
    /// The stamp for the file root and for every changed or new
    /// element: one above every stamp in the previous file, so a
    /// carried stamp can never collide with a fresh one.
    fresh: String,
    sets: HashMap<String, String>,
    sequents: HashMap<String, String>,
}

fn plan_stamps(old: &PoView, new: &PoView) -> StampPlan {
    let mut max = 0i64;
    for stamp in old
        .stamp
        .iter()
        .chain(old.sets.values().filter_map(|set| set.stamp.as_ref()))
        .chain(old.sequents.values().filter_map(|seq| seq.stamp.as_ref()))
    {
        if let Ok(value) = stamp.parse::<i64>() {
            max = max.max(value);
        }
    }
    let fresh = (max + 1).to_string();

    // Equal-sized name sets plus every new name found unchanged in the
    // old view together imply nothing was added, removed, or edited.
    // Equality implies presence in both views, so indexing is safe.
    let mut all_unchanged =
        old.sets.len() == new.sets.len() && old.sequents.len() == new.sequents.len();
    let mut sets = HashMap::new();
    for name in new.sets.keys() {
        if new.set_chain_eq(old, name) {
            let stamp = old.sets[name].stamp.clone().unwrap_or_else(|| "0".into());
            sets.insert(name.clone(), stamp);
        } else {
            all_unchanged = false;
            sets.insert(name.clone(), fresh.clone());
        }
    }
    let mut sequents = HashMap::new();
    for name in new.sequents.keys() {
        if new.sequent_eq(old, name) {
            let stamp = old.sequents[name]
                .stamp
                .clone()
                .unwrap_or_else(|| "0".into());
            sequents.insert(name.clone(), stamp);
        } else {
            all_unchanged = false;
            sequents.insert(name.clone(), fresh.clone());
        }
    }
    StampPlan {
        all_unchanged,
        fresh,
        sets,
        sequents,
    }
}

/// The ` poStamp="0"` needle every generated stamp carries, built from
/// the emitter's attribute constant so the two cannot drift apart.
fn stamp_zero() -> String {
    format!(" {}=\"0\"", attr::PO_STAMP)
}

/// Substitute the planned stamps into a freshly generated `.bpo`.
///
/// The emitter writes one start tag per line with `name` as the first
/// attribute and quotes escaped inside values, and only the file root,
/// the top-level predicate sets, and the sequents carry a stamp — so a
/// line pass suffices. A line that doesn't match the expected shape is
/// left alone (its stamp stays `"0"`).
fn rewrite_stamps(new_bpo: &str, plan: &StampPlan) -> String {
    let stamp_zero = stamp_zero();
    let file_prefix = format!("<{}", xtag::PO_FILE);
    let set_prefix = format!("<{} name=\"", xtag::PO_PREDICATE_SET);
    let sequent_prefix = format!("<{} name=\"", xtag::PO_SEQUENT);
    let mut out = String::with_capacity(new_bpo.len() + 64);
    for line in new_bpo.lines() {
        let stamp = if !line.contains(&stamp_zero) {
            None
        } else if line.starts_with(&file_prefix) {
            Some(plan.fresh.as_str())
        } else if let Some(name) = line_name(line, &set_prefix) {
            plan.sets.get(&name).map(String::as_str)
        } else if let Some(name) = line_name(line, &sequent_prefix) {
            plan.sequents.get(&name).map(String::as_str)
        } else {
            None
        };
        match stamp {
            Some(stamp) => out.push_str(&replace_stamp(line, stamp)),
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// Replace the `poStamp="0"` attribute in one emitted line. Attribute
/// values escape raw quotes, so the first match is always the
/// attribute itself, never text inside a value.
fn replace_stamp(line: &str, stamp: &str) -> String {
    line.replacen(
        &stamp_zero(),
        &format!(" {}=\"{stamp}\"", attr::PO_STAMP),
        1,
    )
}

/// The element kinds Rodin writes in the iteration order of a Java hash
/// table and rossi writes sorted: carrier sets and constants (one mixed
/// run in Rodin's files), variables, event parameters, and the
/// identifiers of a proof-obligation predicate set. These are the only
/// sibling orders reconciled; every other kind follows source order on
/// both sides.
pub const HASH_ORDERED: [&str; 5] = [
    xtag::SC_CARRIER_SET,
    xtag::SC_CONSTANT,
    xtag::SC_VARIABLE,
    xtag::SC_PARAMETER,
    xtag::PO_IDENTIFIER,
];

/// Reorder the hash-ordered declarations of a freshly generated `.bcc`,
/// `.bcm` or `.bpo` to follow the previous copy of the same file.
///
/// Rodin's builder compares an element's children positionally, so a
/// reorder alone re-stamps every obligation whose hypothesis chain
/// crosses the set, and Rodin's own order is that of its hash tables,
/// which nothing can reproduce sanely. Keeping the previous file's order
/// is exact from the first time Rodin wrote the file on. Within each
/// container (the file root, an internal context, an event, a predicate
/// set) every contiguous run of `HASH_ORDERED` leaves is stably sorted
/// by the name's position among the previous container's children;
/// names the previous file lacks follow in generated order. A container
/// or file without a previous counterpart, and previous text yielding no
/// order at all, leave the generated text byte-identical.
///
/// Text surgery on the emitter's layout, like `rewrite_stamps`: one
/// element per line, flush left, values escaped. The previous file,
/// usually Rodin's, is read as XML and may be indented and spell its
/// attributes in any order.
pub fn preserve_child_order(generated: &str, previous: &str) -> String {
    let order = previous_child_order(previous);
    if order.is_empty() {
        return generated.to_string();
    }
    let flush = |run: &mut Vec<(String, &str)>, path: &[String], out: &mut String| {
        if run.is_empty() {
            return;
        }
        if let Some(positions) = path.last().and_then(|key| order.get(key)) {
            run.sort_by_key(|(name, _)| positions.get(name).copied().unwrap_or(usize::MAX));
        }
        for (_, line) in run.drain(..) {
            out.push_str(line);
        }
    };
    let mut out = String::with_capacity(generated.len());
    let mut path: Vec<String> = Vec::new();
    let mut run: Vec<(String, &str)> = Vec::new();
    for line in generated.split_inclusive('\n') {
        let tag = start_tag(line);
        let leaf = line.trim_end().ends_with("/>");
        if let Some(tag) = tag
            && leaf
            && HASH_ORDERED.contains(&tag)
            && let Some(name) = emitted_name(line)
        {
            run.push((name, line));
            continue;
        }
        flush(&mut run, &path, &mut out);
        match tag {
            Some(tag) if !leaf => {
                let name = emitted_name(line).unwrap_or_default();
                path.push(child_key(path.last(), tag, &name));
            }
            _ if line.starts_with("</") => {
                path.pop();
            }
            _ => {}
        }
        out.push_str(line);
    }
    flush(&mut run, &path, &mut out);
    out
}

/// Per container, the position of every [`HASH_ORDERED`] child of a
/// previous file, by unescaped name. An attribute scan like
/// [`sequent_stamps`]: no formula parsing; malformed input stops the scan
/// and keeps what was seen, so a damaged previous file reorders nothing
/// past the damage rather than failing the build.
fn previous_child_order(xml: &str) -> HashMap<String, HashMap<String, usize>> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut order: HashMap<String, HashMap<String, usize>> = HashMap::new();
    let mut path: Vec<String> = Vec::new();
    loop {
        let event = reader.read_event_into(&mut buf);
        let (element, opens) = match &event {
            Ok(Event::Start(e)) => (e, true),
            Ok(Event::Empty(e)) => (e, false),
            Ok(Event::End(_)) => {
                path.pop();
                buf.clear();
                continue;
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {
                buf.clear();
                continue;
            }
        };
        let tag = String::from_utf8_lossy(element.name().as_ref()).into_owned();
        let name = attribute(element, attr::NAME).unwrap_or_default();
        if HASH_ORDERED.contains(&tag.as_str())
            && let Some(parent) = path.last()
        {
            let children = order.entry(parent.clone()).or_default();
            let position = children.len();
            children.entry(name.clone()).or_insert(position);
        }
        if opens {
            path.push(child_key(path.last(), &tag, &name));
        }
        buf.clear();
    }
    order
}

/// The key both walkers give a container: the path of `(tag, name)` from
/// the root, so a name reused under different parents (every sequent's
/// `SEQHYP`) never aliases another.
fn child_key(parent: Option<&String>, tag: &str, name: &str) -> String {
    format!("{}|{tag}#{name}", parent.map_or("", String::as_str))
}

/// The tag of an emitted start or empty-element line; `None` for an end
/// tag, the XML declaration, or anything that is not an element.
fn start_tag(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('<')?;
    if rest.starts_with(['/', '?', '!']) {
        return None;
    }
    let end = rest.find([' ', '/', '>', '\n']).unwrap_or(rest.len());
    Some(&rest[..end])
}

/// The `name` attribute of an emitted line, unescaped. Values escape raw
/// quotes, so the first ` name="` is the attribute itself.
fn emitted_name(line: &str) -> Option<String> {
    line_name(&line[line.find(" name=\"")?..], " name=\"")
}

/// The element name at the start of an emitted line, unescaped.
fn line_name(line: &str, prefix: &str) -> Option<String> {
    let rest = line.strip_prefix(prefix)?;
    let raw = &rest[..rest.find('"')?];
    Some(
        quick_xml::escape::unescape(raw)
            .map(std::borrow::Cow::into_owned)
            .unwrap_or_else(|_| raw.to_string()),
    )
}

/// One `.bps` row as parsed for reconciliation and stamp guarding.
pub(crate) struct StatusRow {
    /// Unescaped `name` attribute.
    pub(crate) name: String,
    /// The row rebuilt from its raw attribute bytes (carried verbatim).
    pub(crate) row: String,
    /// Unescaped `org.eventb.core.poStamp`, when present.
    pub(crate) stamp: Option<String>,
    /// Parsed `org.eventb.core.confidence`, when present and numeric.
    pub(crate) confidence: Option<i64>,
    /// `org.eventb.core.psBroken="true"`.
    pub(crate) broken: bool,
    /// `org.eventb.core.contextDependent="true"` — such rows are
    /// re-checked on every build, even with a matching stamp.
    pub(crate) context_dependent: bool,
}

/// Parse a `.bps` document into [`StatusRow`]s in document order. Rows
/// rebuild from their raw attribute bytes, so carried values survive
/// byte-for-byte; status rows are attribute-only by construction, so
/// children are not represented.
pub(crate) fn parse_status_rows(bps: &str) -> Vec<StatusRow> {
    let mut reader = Reader::from_str(bps);
    let mut buf = Vec::new();
    let mut rows = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e))
                if e.name().as_ref() == xtag::PS_STATUS.as_bytes() =>
            {
                let mut parsed = StatusRow {
                    name: String::new(),
                    row: format!("<{}", xtag::PS_STATUS),
                    stamp: None,
                    confidence: None,
                    broken: false,
                    context_dependent: false,
                };
                for attr in e.attributes().flatten() {
                    let key = String::from_utf8_lossy(attr.key.as_ref());
                    let raw = String::from_utf8_lossy(&attr.value);
                    let unescaped = || match quick_xml::escape::unescape(&raw) {
                        Ok(cow) => cow.into_owned(),
                        Err(_) => raw.to_string(),
                    };
                    if attr.key.as_ref() == attr::NAME.as_bytes() {
                        parsed.name = unescaped();
                    } else if attr.key.as_ref() == attr::PO_STAMP.as_bytes() {
                        parsed.stamp = Some(unescaped());
                    } else if attr.key.as_ref() == attr::CONFIDENCE.as_bytes() {
                        parsed.confidence = unescaped().parse::<i64>().ok();
                    } else if attr.key.as_ref() == attr::PS_BROKEN.as_bytes() {
                        parsed.broken = raw == "true";
                    } else if attr.key.as_ref() == attr::CONTEXT_DEPENDENT.as_bytes() {
                        parsed.context_dependent = raw == "true";
                    }
                    parsed.row.push(' ');
                    parsed.row.push_str(&key);
                    parsed.row.push_str("=\"");
                    parsed.row.push_str(&raw);
                    parsed.row.push('"');
                }
                parsed.row.push_str("/>");
                rows.push(parsed);
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    rows
}

/// Render a `.bps` document from finished rows, matching the
/// generator's byte format.
/// See [`fresh_status_row`] for why this is public.
pub fn assemble_status(rows: &[String]) -> String {
    let mut out = String::from(DOC_HEADER);
    if rows.is_empty() {
        out.push_str(&format!("<{}/>\n", xtag::PS_FILE));
    } else {
        out.push_str(&format!("<{}>\n", xtag::PS_FILE));
        for row in rows {
            out.push_str(row);
            out.push('\n');
        }
        out.push_str(&format!("</{}>\n", xtag::PS_FILE));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";
    const RODIN_HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n";
    const MACHINE: &str = "org.eventb.core.scMachineFile";
    const CONTEXT: &str = "org.eventb.core.scContextFile";

    /// An emitted declaration, as the generator spells it.
    fn leaf(tag: &str, name: &str) -> String {
        format!("<{tag} name=\"{name}\" org.eventb.core.type=\"ℤ\"/>\n")
    }

    /// The same declaration as Rodin writes it: indented, `name` last.
    fn rodin_leaf(depth: usize, tag: &str, name: &str) -> String {
        let indent = "    ".repeat(depth);
        format!("{indent}<{tag} org.eventb.core.type=\"ℤ\" name=\"{name}\"/>\n")
    }

    fn open(tag: &str, name: Option<&str>) -> String {
        match name {
            Some(name) => format!("<{tag} name=\"{name}\">\n"),
            None => format!("<{tag}>\n"),
        }
    }

    fn close(tag: &str) -> String {
        format!("</{tag}>\n")
    }

    /// The names of every tracked run, in document order.
    fn runs(xml: &str) -> Vec<Vec<String>> {
        let mut runs = Vec::new();
        let mut run = Vec::new();
        for line in xml.lines() {
            let line = line.trim_start();
            match start_tag(line) {
                Some(tag) if HASH_ORDERED.contains(&tag) => run.push(emitted_name(line).unwrap()),
                _ if !run.is_empty() => runs.push(std::mem::take(&mut run)),
                _ => {}
            }
        }
        if !run.is_empty() {
            runs.push(run);
        }
        runs
    }

    #[test]
    fn a_run_follows_the_previous_file_and_keeps_the_generated_layout() {
        let var = xtag::SC_VARIABLE;
        let generated = format!(
            "{HEADER}{}{}{}{}{}",
            open(MACHINE, None),
            leaf(var, "a"),
            leaf(var, "b"),
            leaf(var, "c"),
            close(MACHINE)
        );
        let previous = format!(
            "{RODIN_HEADER}{}{}{}{}{}",
            open(MACHINE, None),
            rodin_leaf(1, var, "c"),
            rodin_leaf(1, var, "a"),
            rodin_leaf(1, var, "b"),
            close(MACHINE)
        );
        let expected = format!(
            "{HEADER}{}{}{}{}{}",
            open(MACHINE, None),
            leaf(var, "c"),
            leaf(var, "a"),
            leaf(var, "b"),
            close(MACHINE)
        );
        assert_eq!(preserve_child_order(&generated, &previous), expected);
    }

    #[test]
    fn carrier_sets_and_constants_interleave_as_the_previous_file_did() {
        // Rodin writes one mixed sequence; the generator writes the sets
        // first. Both kinds are one run.
        let (set, constant) = (xtag::SC_CARRIER_SET, xtag::SC_CONSTANT);
        let generated = format!(
            "{HEADER}{}{}{}{}{}{}",
            open(CONTEXT, None),
            leaf(set, "S"),
            leaf(set, "T"),
            leaf(constant, "c"),
            leaf(constant, "d"),
            close(CONTEXT)
        );
        let previous = format!(
            "{RODIN_HEADER}{}{}{}{}{}{}",
            open(CONTEXT, None),
            rodin_leaf(1, constant, "c"),
            rodin_leaf(1, set, "S"),
            rodin_leaf(1, constant, "d"),
            rodin_leaf(1, set, "T"),
            close(CONTEXT)
        );
        assert_eq!(
            runs(&preserve_child_order(&generated, &previous)),
            [["c", "S", "d", "T"]]
        );
    }

    #[test]
    fn names_the_previous_file_lacks_follow_in_generated_order() {
        let var = xtag::SC_VARIABLE;
        let generated = format!(
            "{HEADER}{}{}{}{}{}",
            open(MACHINE, None),
            leaf(var, "a"),
            leaf(var, "b"),
            leaf(var, "c"),
            close(MACHINE)
        );
        let previous = format!(
            "{RODIN_HEADER}{}{}{}",
            open(MACHINE, None),
            rodin_leaf(1, var, "b"),
            close(MACHINE)
        );
        assert_eq!(
            runs(&preserve_child_order(&generated, &previous)),
            [["b", "a", "c"]]
        );
    }

    #[test]
    fn containers_are_matched_by_their_path() {
        // The same parameter names under two events: each event follows
        // its own previous order, and an event the previous file lacks
        // is left as generated.
        let (event, param) = (xtag::SC_EVENT, xtag::SC_PARAMETER);
        let params = |names: &[&str]| -> String { names.iter().map(|n| leaf(param, n)).collect() };
        let generated = format!(
            "{HEADER}{}{}{}{}{}{}{}{}{}{}{}",
            open(MACHINE, None),
            open(event, Some("e1")),
            params(&["x", "y"]),
            close(event),
            open(event, Some("e2")),
            params(&["x", "y"]),
            close(event),
            open(event, Some("e3")),
            params(&["x", "y"]),
            close(event),
            close(MACHINE)
        );
        let previous = format!(
            "{RODIN_HEADER}{}    {}{}{}    {}    {}{}{}    {}{}",
            open(MACHINE, None),
            open(event, Some("e1")),
            rodin_leaf(2, param, "y"),
            rodin_leaf(2, param, "x"),
            close(event),
            open(event, Some("e2")),
            rodin_leaf(2, param, "x"),
            rodin_leaf(2, param, "y"),
            close(event),
            close(MACHINE)
        );
        assert_eq!(
            runs(&preserve_child_order(&generated, &previous)),
            [["y", "x"], ["x", "y"], ["x", "y"]]
        );
    }

    #[test]
    fn runs_never_cross_other_kinds() {
        // A hypothesis set over two contexts lays out identifiers and
        // predicates in turn; each identifier run follows the set's
        // whole previous order and the predicates keep their lines.
        let (set, ident) = (xtag::PO_PREDICATE_SET, xtag::PO_IDENTIFIER);
        let predicate = |name: &str| {
            format!(
                "<{} name=\"{name}\" org.eventb.core.predicate=\"⊤\"/>\n",
                xtag::PO_PREDICATE
            )
        };
        let generated = format!(
            "{HEADER}{}{}{}{}{}{}{}{}{}{}",
            open(xtag::PO_FILE, None),
            open(set, Some("CTXHYP")),
            leaf(ident, "S"),
            leaf(ident, "c"),
            predicate("PRD0"),
            leaf(ident, "T"),
            leaf(ident, "d"),
            predicate("PRD1"),
            close(set),
            close(xtag::PO_FILE)
        );
        let previous = format!(
            "{RODIN_HEADER}{}    {}{}{}{}{}    {}{}",
            open(xtag::PO_FILE, None),
            open(set, Some("CTXHYP")),
            rodin_leaf(2, ident, "d"),
            rodin_leaf(2, ident, "c"),
            rodin_leaf(2, ident, "T"),
            rodin_leaf(2, ident, "S"),
            close(set),
            close(xtag::PO_FILE)
        );
        let out = preserve_child_order(&generated, &previous);
        assert_eq!(runs(&out), [["c", "S"], ["d", "T"]]);
        // Header, root, set, two identifiers, a predicate, two more, a predicate.
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[5].starts_with(&format!("<{} name=\"PRD0\"", xtag::PO_PREDICATE)));
        assert!(lines[8].starts_with(&format!("<{} name=\"PRD1\"", xtag::PO_PREDICATE)));
    }

    #[test]
    fn a_previous_file_yielding_no_order_leaves_the_bytes_alone() {
        for previous in ["", "OLD", "<?xml version=\"1.0\"?>\n<x/>"] {
            assert_eq!(preserve_child_order("NEW-BCM", previous), "NEW-BCM");
            let generated = format!(
                "{HEADER}{}{}{}",
                open(MACHINE, None),
                leaf(xtag::SC_VARIABLE, "a"),
                close(MACHINE)
            );
            assert_eq!(preserve_child_order(&generated, previous), generated);
        }
    }

    #[test]
    fn names_compare_unescaped() {
        // The generator escapes `>` in an internal name and writes a
        // primed identifier's quote raw; another writer may do the
        // reverse.
        let (event, param) = (xtag::SC_EVENT, xtag::SC_PARAMETER);
        let generated = format!(
            "{HEADER}{}{}{}{}{}{}",
            open(MACHINE, None),
            open(event, Some("&gt;")),
            leaf(param, "x'"),
            leaf(param, "y'"),
            close(event),
            close(MACHINE)
        );
        let previous = format!(
            "{RODIN_HEADER}{}    {}{}{}    {}{}",
            open(MACHINE, None),
            open(event, Some(">")),
            rodin_leaf(2, param, "y&apos;"),
            rodin_leaf(2, param, "x&apos;"),
            close(event),
            close(MACHINE)
        );
        assert_eq!(
            runs(&preserve_child_order(&generated, &previous)),
            [["y'", "x'"]]
        );
    }
}
