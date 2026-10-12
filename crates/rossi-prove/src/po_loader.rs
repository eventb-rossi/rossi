//! Loading `.bpo` proof obligations into prover sequents.
//!
//! The sequent behind an obligation: its hypothesis-set chain is
//! walked root-first, enriching the type environment with each set's
//! identifiers and appending each hypothesis followed immediately by
//! its well-definedness conjuncts; the goal's WD
//! conjuncts are added too unless the obligation's name ends in
//! `/WD`. WD predicates whose source contains a universal quantifier
//! in its predicate skeleton are skipped as uninteresting, `⊤`
//! conjuncts are dropped, and selection hints mark the initially
//! selected hypotheses. A shared set is parsed once and reused by
//! every obligation whose chain passes through it.

use std::collections::BTreeMap;
use std::io::BufRead;
use std::sync::OnceLock;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use rossi::formula::tag::{LiteralPredOp, QuantPredOp};
use rossi::formula::{
    Predicate, PredicateKind, SealedTypeEnvironment, Type, TypeEnvironmentBuilder,
};
use rossi::parse_predicate_str;

use crate::sequent::ProverSequent;
use crate::xml::{attrs, get};

const PO_FILE: &str = "org.eventb.core.poFile";
const PO_PREDICATE_SET: &str = "org.eventb.core.poPredicateSet";
const PO_SEQUENT: &str = "org.eventb.core.poSequent";
const PO_PREDICATE: &str = "org.eventb.core.poPredicate";
const PO_IDENTIFIER: &str = "org.eventb.core.poIdentifier";
const PO_SEL_HINT: &str = "org.eventb.core.poSelHint";

const NAME: &str = "name";
const PARENT_SET: &str = "org.eventb.core.parentSet";
const PREDICATE: &str = "org.eventb.core.predicate";
const TYPE: &str = "org.eventb.core.type";
const PO_STAMP: &str = "org.eventb.core.poStamp";
const PO_DESC: &str = "org.eventb.core.poDesc";
const ACCURATE: &str = "org.eventb.core.accurate";
const PO_SOURCE: &str = "org.eventb.core.poSource";
const PO_ROLE: &str = "org.eventb.core.poRole";
const SOURCE: &str = "org.eventb.core.source";
const SEL_HINT_FST: &str = "org.eventb.core.poSelHintFst";
const SEL_HINT_SND: &str = "org.eventb.core.poSelHintSnd";

/// A parsed `.bpo` document: the shared predicate-set chains and the
/// sequents referencing them. Sequents are loaded on demand.
#[derive(Debug, Default)]
pub struct PoFile {
    sets: BTreeMap<String, PoSet>,
    sequents: Vec<PoSequentEntry>,
    /// First position of each obligation name — `sequent` is called
    /// once per obligation over a component, so lookups must not
    /// re-scan the list.
    by_name: BTreeMap<String, usize>,
}

#[derive(Debug, Default)]
struct PoSet {
    parent: Option<SetRef>,
    idents: Vec<(String, String)>,
    preds: Vec<(String, String)>,
    /// The parsed form, built on first use; every obligation whose
    /// chain passes through a shared set reuses it.
    loaded: OnceLock<Result<LoadedSet, String>>,
}

/// A predicate set parsed against its chain.
#[derive(Debug)]
struct LoadedSet {
    /// The chain's environment after this set's identifiers.
    env: SealedTypeEnvironment,
    /// The set's predicates in document order, for selection hints.
    preds: Vec<Predicate>,
    /// The hypothesis contribution: each predicate followed by its WD
    /// conjuncts.
    hyps: Vec<Predicate>,
}

impl PoSet {
    /// Parses the set against `parent`, the chain's environment so far.
    fn load(&self, parent: &SealedTypeEnvironment) -> Result<LoadedSet, String> {
        let env = if self.idents.is_empty() {
            parent.clone()
        } else {
            let mut builder = parent.to_builder();
            for (name, ty) in &self.idents {
                let ty = Type::parse_rodin(ty).ok_or_else(|| format!("identifier type `{ty}`"))?;
                builder.insert(name, ty);
            }
            builder.into_snapshot()
        };
        let mut preds = Vec::with_capacity(self.preds.len());
        let mut hyps = Vec::new();
        for (_, source) in &self.preds {
            let hypothesis = parse_typed(source, &env)?;
            hyps.push(hypothesis.clone());
            add_wd_predicates(&hypothesis, &mut hyps);
            preds.push(hypothesis);
        }
        Ok(LoadedSet { env, preds, hyps })
    }
}

/// One proof obligation of the document.
#[derive(Debug)]
pub struct PoSequentEntry {
    /// The obligation's name, e.g. `evt/inv1/INV`.
    pub name: String,
    /// The obligation's stamp, the signal status rows are keyed on.
    pub stamp: Option<String>,
    /// What the obligation asks to prove, as the generator worded it
    /// (`poDesc`).
    pub description: String,
    /// Whether every element the obligation depends on passed its
    /// checks.
    pub accurate: bool,
    /// The elements the obligation traces back to: `(role, handle)`
    /// rows in document order, the handles as the file spells them.
    pub sources: Vec<(String, String)>,
    local: PoSet,
    goal: Option<String>,
    hints: Vec<Hint>,
}

/// A reference to a shared predicate set: the `.bpo` file's handle
/// path (predicate-set chains cross files — a machine's `CTXHYP`
/// parent points into the seen context's `.bpo`) and the set's name.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SetRef {
    file: String,
    set: String,
}

/// A hypothesis-set key inside one sequent's chain.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SetKey {
    /// The sequent's own local set.
    Local,
    /// A shared set, possibly in another component's file.
    Named(SetRef),
}

#[derive(Debug)]
enum Hint {
    /// Select every predicate of the sets strictly between `start`
    /// (exclusive) and `end` (inclusive), walking parent-ward.
    Interval { start: SetKey, end: SetKey },
    /// Select one predicate of one set.
    Single { set: SetKey, pred: String },
}

/// A `.bpo`-level failure.
#[derive(Debug, thiserror::Error)]
pub enum PoError {
    /// The XML is malformed.
    #[error("XML error: {0}")]
    Xml(#[from] quick_xml::Error),
    /// Not a proof-obligation file.
    #[error("unsupported proof-obligation file: {0}")]
    Unsupported(String),
}

impl PoFile {
    /// Parses a `.bpo` document.
    pub fn read(reader: impl BufRead) -> Result<PoFile, PoError> {
        read_bpo(reader)
    }

    /// The proof obligations, in document order.
    pub fn sequents(&self) -> impl Iterator<Item = &PoSequentEntry> {
        self.sequents.iter()
    }

    /// The named obligation, if present.
    pub fn sequent(&self, name: &str) -> Option<&PoSequentEntry> {
        self.by_name.get(name).map(|&index| &self.sequents[index])
    }

    fn push_sequent(&mut self, entry: PoSequentEntry) {
        self.by_name
            .entry(entry.name.clone())
            .or_insert(self.sequents.len());
        self.sequents.push(entry);
    }
}

/// The `.bpo` files of one project, keyed by the handle path their
/// cross-references use (`/<project>/<component>.bpo`). Hypothesis-set
/// chains cross component files, so sequents load against the project.
#[derive(Debug, Default)]
pub struct PoProject {
    files: BTreeMap<String, PoFile>,
}

impl PoProject {
    /// Registers one parsed file under its handle path.
    pub fn insert(&mut self, path: impl Into<String>, file: PoFile) {
        self.files.insert(path.into(), file);
    }

    /// The registered file at `path`.
    pub fn file(&self, path: &str) -> Option<&PoFile> {
        self.files.get(path)
    }

    /// Builds the prover sequent of one obligation.
    pub fn load(&self, path: &str, name: &str) -> Result<ProverSequent, String> {
        let file = self
            .files
            .get(path)
            .ok_or_else(|| format!("no component at `{path}`"))?;
        let entry = file
            .sequent(name)
            .ok_or_else(|| format!("no obligation named `{name}`"))?;
        let total_sets: usize = self.files.values().map(|f| f.sets.len()).sum();

        // Handles resolve to basenames while the loose multi-project
        // layout keys files with a directory prefix; chains never
        // cross projects, so a basename resolves inside the loaded
        // file's directory, falling back to the bare basename for
        // unprefixed projects.
        let dir = &path[..path.rfind('/').map_or(0, |slash| slash + 1)];

        // The hypothesis-set chain, root first, ending at the local set.
        let mut chain: Vec<(SetKey, &PoSet)> = vec![(SetKey::Local, &entry.local)];
        let mut parent = entry.local.parent.as_ref();
        while let Some(reference) = parent {
            if chain.len() > total_sets + 1 {
                return Err("cyclic predicate-set chain".to_string());
            }
            let set = self
                .files
                .get(&format!("{dir}{}", reference.file))
                .or_else(|| self.files.get(&reference.file))
                .and_then(|file| file.sets.get(&reference.set))
                .ok_or_else(|| {
                    format!(
                        "dangling parent set `{}` in `{}`",
                        reference.set, reference.file
                    )
                })?;
            chain.push((SetKey::Named(reference.clone()), set));
            parent = set.parent.as_ref();
        }
        chain.reverse();

        // The sets marked selected by interval hints: from the end set
        // walking parent-ward (i.e. toward the chain root) until the
        // start set, exclusive.
        let position = |key: &SetKey| chain.iter().position(|(k, _)| k == key);
        let mut selected_sets = vec![false; chain.len()];
        let mut selected_preds: Vec<(&SetKey, &str)> = Vec::new();
        for hint in &entry.hints {
            match hint {
                Hint::Interval { start, end } => {
                    let Some(end) = position(end) else { continue };
                    let first = position(start).map_or(0, |start| start + 1);
                    for slot in selected_sets.iter_mut().take(end + 1).skip(first) {
                        *slot = true;
                    }
                }
                Hint::Single { set, pred } => selected_preds.push((set, pred)),
            }
        }

        // Every set loads once, root first, each against its parent's
        // environment; shared sets are then reused by the other
        // obligations of their chains. Duplicate hypotheses are dropped
        // by the sequent's insertion-ordered sets, which keep first
        // occurrences.
        let mut env = TypeEnvironmentBuilder::new().make_snapshot();
        let mut hypotheses: Vec<Predicate> = Vec::new();
        let mut selected: Vec<Predicate> = Vec::new();
        for (index, (key, set)) in chain.iter().enumerate() {
            let loaded = set
                .loaded
                .get_or_init(|| set.load(&env))
                .as_ref()
                .map_err(String::clone)?;
            env = loaded.env.clone();
            hypotheses.extend(loaded.hyps.iter().cloned());
            if selected_sets[index] {
                selected.extend(loaded.preds.iter().cloned());
            } else {
                for ((pred_name, _), pred) in set.preds.iter().zip(&loaded.preds) {
                    if selected_preds
                        .iter()
                        .any(|(set, pred)| *set == key && pred == pred_name)
                    {
                        selected.push(pred.clone());
                    }
                }
            }
        }

        let goal = entry
            .goal
            .as_deref()
            .ok_or_else(|| format!("no goal for `{name}`"))?;
        let goal = parse_typed(goal, &env)?;
        if !name.ends_with("/WD") {
            add_wd_predicates(&goal, &mut hypotheses);
        }

        Ok(ProverSequent::new(env, hypotheses, [], selected, goal))
    }
}

fn parse_typed(
    source: &str,
    env: &rossi::formula::SealedTypeEnvironment,
) -> Result<Predicate, String> {
    let parsed =
        parse_predicate_str(source).map_err(|err| format!("predicate `{source}`: {err}"))?;
    let checked = parsed.type_check(env);
    if !checked.inferred.is_empty() {
        return Err(format!("predicate `{source}` uses undeclared identifiers"));
    }
    let typed = checked
        .typed
        .ok_or_else(|| format!("predicate `{source}` does not type-check"))?;
    Ok(typed.strip_ascriptions())
}

/// Rewrites every negative integer literal into the parse-normal
/// `−(lit)` shape.
fn unfold_negative_literals(pred: &Predicate) -> Predicate {
    use rossi::formula::rewrite::FormulaRewriter;
    use rossi::formula::{Expression, ExpressionKind};
    struct Unfold;
    impl FormulaRewriter for Unfold {
        fn rewrite_expression(&mut self, expr: &Expression) -> Expression {
            let ExpressionKind::IntegerLiteral(value) = expr.kind() else {
                return expr.clone();
            };
            if value.sign() != num_bigint::Sign::Minus {
                return expr.clone();
            }
            let ff = expr.factory();
            let positive = ff.integer_literal(-value, expr.span());
            ff.unary_expression(
                rossi::formula::tag::UnaryExprOp::UnMinus,
                positive,
                expr.span(),
            )
        }
    }
    pred.rewrite(&mut Unfold)
}

/// Appends the WD conjuncts of `pred`:
/// skipped entirely when the source predicate contains a universal
/// quantifier in its predicate skeleton, otherwise the WD lemma's
/// top-level conjuncts minus `⊤`, deduplicated into the hypothesis
/// list.
fn add_wd_predicates(pred: &Predicate, hypotheses: &mut Vec<Predicate>) {
    if contains_forall_skeleton(pred) {
        return;
    }
    // The lemma construction folds a literal's unary minus into a
    // negative literal, but everything else in a loaded sequent keeps
    // the parse-normal −(lit) shape — including the stored proofs'
    // recorded dependencies, which these hypotheses must match
    // structurally (the reference folds on both sides, so it agrees
    // with itself; a half-folded rossi would not).
    let wd = unfold_negative_literals(&pred.wd_lemma());
    let conjuncts: Vec<Predicate> = match wd.kind() {
        PredicateKind::Associative {
            op: rossi::formula::tag::AssocPredOp::LAnd,
            children,
        } => children.to_vec(),
        _ => vec![wd.clone()],
    };
    for conjunct in conjuncts {
        if matches!(
            conjunct.kind(),
            PredicateKind::Literal(LiteralPredOp::BTrue)
        ) {
            continue;
        }
        if !hypotheses.contains(&conjunct) {
            hypotheses.push(conjunct);
        }
    }
}

/// Whether the predicate skeleton contains a `∀` — the no-forall
/// inspector: the walk descends through predicate connectives and the
/// bodies of existential quantifiers but never into expressions or
/// extended formulas.
fn contains_forall_skeleton(pred: &Predicate) -> bool {
    match pred.kind() {
        PredicateKind::Quantified { op, pred, .. } => match op {
            QuantPredOp::Forall => true,
            QuantPredOp::Exists => contains_forall_skeleton(pred),
        },
        PredicateKind::Associative { children, .. } => {
            children.iter().any(contains_forall_skeleton)
        }
        PredicateKind::Binary { left, right, .. } => {
            contains_forall_skeleton(left) || contains_forall_skeleton(right)
        }
        PredicateKind::Not(child) => contains_forall_skeleton(child),
        _ => false,
    }
}

fn read_bpo(reader: impl BufRead) -> Result<PoFile, PoError> {
    let mut xml = Reader::from_reader(reader);
    let mut buf = Vec::new();
    let mut file = PoFile::default();
    let mut saw_root = false;
    // The enclosing element stack: which container each child lands in.
    enum Ctx {
        Set(String, PoSet),
        Sequent(PoSequentEntry),
        LocalSet,
        Other,
    }
    let mut stack: Vec<Ctx> = Vec::new();

    let handle_start = |e: &BytesStart<'_>,
                        empty: bool,
                        stack: &mut Vec<Ctx>,
                        file: &mut PoFile,
                        saw_root: &mut bool|
     -> Result<(), PoError> {
        let name = e.name();
        let name = name.as_ref();
        if !*saw_root {
            if name != PO_FILE.as_bytes() {
                return Err(PoError::Unsupported(format!(
                    "root element {}",
                    String::from_utf8_lossy(name)
                )));
            }
            *saw_root = true;
            return Ok(());
        }
        let attrs = attrs(e);
        let ctx = match stack.last_mut() {
            None => {
                if name == PO_PREDICATE_SET.as_bytes() {
                    Ctx::Set(
                        get(&attrs, NAME).unwrap_or_default().to_string(),
                        PoSet {
                            parent: get(&attrs, PARENT_SET).map(parse_set_ref),
                            ..PoSet::default()
                        },
                    )
                } else if name == PO_SEQUENT.as_bytes() {
                    Ctx::Sequent(PoSequentEntry {
                        name: get(&attrs, NAME).unwrap_or_default().to_string(),
                        stamp: get(&attrs, PO_STAMP).map(str::to_string),
                        description: get(&attrs, PO_DESC).unwrap_or_default().to_string(),
                        accurate: get(&attrs, ACCURATE) == Some("true"),
                        sources: Vec::new(),
                        local: PoSet::default(),
                        goal: None,
                        hints: Vec::new(),
                    })
                } else {
                    Ctx::Other
                }
            }
            Some(Ctx::Set(_, set)) => {
                record_set_child(name, &attrs, set);
                Ctx::Other
            }
            Some(Ctx::Sequent(entry)) => {
                if name == PO_PREDICATE_SET.as_bytes() {
                    entry.local.parent = get(&attrs, PARENT_SET).map(parse_set_ref);
                    Ctx::LocalSet
                } else if name == PO_PREDICATE.as_bytes() {
                    let predicate = get(&attrs, PREDICATE).unwrap_or_default().to_string();
                    if entry.goal.is_none() {
                        entry.goal = Some(predicate);
                    }
                    Ctx::Other
                } else if name == PO_SEL_HINT.as_bytes() {
                    if let Some(hint) = parse_hint(&attrs) {
                        entry.hints.push(hint);
                    }
                    Ctx::Other
                } else {
                    // A source names an element the obligation came
                    // from. Loading does not need it, but a report
                    // about the obligation does, and reading it here
                    // saves that reader a second parse of the file.
                    if name == PO_SOURCE.as_bytes()
                        && let Some(handle) = get(&attrs, SOURCE)
                    {
                        entry.sources.push((
                            get(&attrs, PO_ROLE).unwrap_or_default().to_string(),
                            handle.to_string(),
                        ));
                    }
                    Ctx::Other
                }
            }
            Some(Ctx::LocalSet) => {
                // Children of the sequent's local set: reach through to
                // the sequent frame below.
                if let Some(index) = stack.len().checked_sub(2)
                    && let Some(Ctx::Sequent(entry)) = stack.get_mut(index)
                {
                    record_set_child(name, &attrs, &mut entry.local);
                }
                Ctx::Other
            }
            Some(Ctx::Other) => Ctx::Other,
        };
        if empty {
            match ctx {
                Ctx::Set(name, set) => {
                    file.sets.insert(name, set);
                }
                Ctx::Sequent(entry) => file.push_sequent(entry),
                Ctx::LocalSet | Ctx::Other => {}
            }
        } else {
            stack.push(ctx);
        }
        Ok(())
    };

    loop {
        match xml.read_event_into(&mut buf)? {
            Event::Start(e) => handle_start(&e, false, &mut stack, &mut file, &mut saw_root)?,
            Event::Empty(e) => handle_start(&e, true, &mut stack, &mut file, &mut saw_root)?,
            Event::End(_) => {
                if let Some(ctx) = stack.pop() {
                    match ctx {
                        Ctx::Set(name, set) => {
                            file.sets.insert(name, set);
                        }
                        Ctx::Sequent(entry) => file.push_sequent(entry),
                        Ctx::LocalSet | Ctx::Other => {}
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(file)
}

fn record_set_child(name: &[u8], attrs: &[(String, String)], set: &mut PoSet) {
    if name == PO_IDENTIFIER.as_bytes() {
        set.idents.push((
            get(attrs, NAME).unwrap_or_default().to_string(),
            get(attrs, TYPE).unwrap_or_default().to_string(),
        ));
    } else if name == PO_PREDICATE.as_bytes() {
        set.preds.push((
            get(attrs, NAME).unwrap_or_default().to_string(),
            get(attrs, PREDICATE).unwrap_or_default().to_string(),
        ));
    }
}

/// Splits on unescaped occurrences of `sep`, keeping escapes intact
/// inside the pieces. Handle grammar (matching rossi-build's
/// `handles.rs` writer): `/`, `|`, `#`, `\` inside names are escaped
/// with a backslash — generated element names draw on an alphabet that
/// includes every delimiter.
fn split_raw(raw: &str, sep: char) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                let last = parts.last_mut().expect("non-empty");
                last.push('\\');
                last.push(next);
            }
        } else if c == sep {
            parts.push(String::new());
        } else {
            parts.last_mut().expect("non-empty").push(c);
        }
    }
    parts
}

/// Removes the backslash escapes of one handle piece.
fn unescape_piece(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The `name` of a raw `type#name` handle piece, unescaped.
fn piece_name(piece: &str) -> String {
    let raw = split_raw(piece, '#');
    unescape_piece(raw.last().map(String::as_str).unwrap_or(piece))
}

/// A set handle split into the referenced file's basename and the
/// set's name. Chains resolve by basename because exported archives
/// are routinely renamed: the handle keeps the original project name
/// while the archive directory does not, and chains never cross
/// projects.
fn parse_set_ref(handle: &str) -> SetRef {
    let parts = split_raw(handle, '|');
    let path = parts.first().map(String::as_str).unwrap_or(handle);
    let file = split_raw(path, '/')
        .last()
        .map(String::as_str)
        .map(unescape_piece)
        .unwrap_or_default();
    SetRef {
        file,
        set: parts
            .last()
            .map(String::as_str)
            .map(piece_name)
            .unwrap_or_default(),
    }
}

/// Parses one selection hint. A hint with a second handle is an
/// interval over predicate sets; one with only the first handle names
/// a single predicate inside a set.
fn parse_hint(attrs: &[(String, String)]) -> Option<Hint> {
    let fst = get(attrs, SEL_HINT_FST)?;
    match get(attrs, SEL_HINT_SND) {
        Some(snd) => Some(Hint::Interval {
            start: set_key(fst),
            end: set_key(snd),
        }),
        None => {
            // `…|poPredicateSet#SET|poPredicate#PRD` — the predicate is
            // the last piece, its set the handle up to it.
            let parts = split_raw(fst, '|');
            let (pred, set_parts) = parts.split_last()?;
            Some(Hint::Single {
                set: set_key(&set_parts.join("|")),
                pred: piece_name(pred),
            })
        }
    }
}

/// A set handle: one pointing inside a sequent is that sequent's local
/// set, otherwise a shared set reference.
fn set_key(handle: &str) -> SetKey {
    let local = split_raw(handle, '|')
        .iter()
        .any(|part| part.starts_with(&format!("{PO_SEQUENT}#")));
    if local {
        SetKey::Local
    } else {
        SetKey::Named(parse_set_ref(handle))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{env, pred};
    use indoc::formatdoc;

    const SET_PREFIX: &str = "/P/M.bpo|org.eventb.core.poFile#M|org.eventb.core.poPredicateSet#";

    /// A chain CTXHYP ← ALLHYP with hypotheses exercising WD addition:
    /// a division hypothesis (WD `y≠0`), a WD-free one, and a
    /// universally quantified one in the sequent's local set (its WD
    /// is skipped by the no-forall filter).
    fn fixture(sequents: &str) -> PoProject {
        fixture_at("M.bpo", sequents)
    }

    fn fixture_at(path: &str, sequents: &str) -> PoProject {
        let xml = formatdoc!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
            <org.eventb.core.poFile org.eventb.core.poStamp="0">
            <org.eventb.core.poPredicateSet name="CTXHYP" org.eventb.core.poStamp="0">
            <org.eventb.core.poIdentifier name="y" org.eventb.core.type="ℤ"/>
            <org.eventb.core.poPredicate name="PRD0" org.eventb.core.predicate="y≥1"/>
            </org.eventb.core.poPredicateSet>
            <org.eventb.core.poPredicateSet name="ALLHYP" org.eventb.core.parentSet="{SET_PREFIX}CTXHYP" org.eventb.core.poStamp="0">
            <org.eventb.core.poIdentifier name="x" org.eventb.core.type="ℤ"/>
            <org.eventb.core.poPredicate name="PRD1" org.eventb.core.predicate="x÷y=1"/>
            <org.eventb.core.poPredicate name="PRD2" org.eventb.core.predicate="x=1"/>
            </org.eventb.core.poPredicateSet>
            {sequents}
            </org.eventb.core.poFile>"#
        );
        let mut project = PoProject::default();
        project.insert(path, PoFile::read(xml.as_bytes()).expect("readable file"));
        project
    }

    fn ints() -> rossi::formula::SealedTypeEnvironment {
        env(&[("x", "ℤ"), ("y", "ℤ")])
    }

    #[test]
    fn loads_chain_wd_and_selection() {
        let sequents = formatdoc!(
            r#"<org.eventb.core.poSequent name="evt/inv1/INV" org.eventb.core.poStamp="3">
            <org.eventb.core.poPredicateSet name="SEQHYP" org.eventb.core.parentSet="{SET_PREFIX}ALLHYP">
            <org.eventb.core.poPredicate name="PRD9" org.eventb.core.predicate="∀z·z÷y=1"/>
            </org.eventb.core.poPredicateSet>
            <org.eventb.core.poPredicate name="SEQG" org.eventb.core.predicate="(x+y)÷x=1"/>
            <org.eventb.core.poSelHint name="H1" org.eventb.core.poSelHintFst="{SET_PREFIX}CTXHYP" org.eventb.core.poSelHintSnd="{SET_PREFIX}ALLHYP"/>
            <org.eventb.core.poSelHint name="H2" org.eventb.core.poSelHintFst="{SET_PREFIX}CTXHYP|org.eventb.core.poPredicate#PRD0"/>
            </org.eventb.core.poSequent>"#
        );
        let file = fixture(&sequents);
        let entry = file
            .file("M.bpo")
            .and_then(|f| f.sequent("evt/inv1/INV"))
            .expect("sequent present");
        assert_eq!(entry.stamp.as_deref(), Some("3"));

        let seq = file.load("M.bpo", "evt/inv1/INV").expect("loads");
        let ints = ints();
        assert_eq!(seq.type_env().get("x"), ints.get("x"));
        assert_eq!(seq.type_env().get("y"), ints.get("y"));
        assert_eq!(seq.goal(), &pred(&ints, "(x+y)÷x=1"));

        // Hypotheses in chain order, each followed by its WD conjuncts:
        // the WD-free y≥1, then x÷y=1 with its y≠0, then x=1, then the
        // quantified local hypothesis (its WD skipped by the no-forall
        // filter), then the goal's WD x≠0.
        let hyps: Vec<_> = seq.hyp_iter().cloned().collect();
        assert_eq!(
            hyps,
            vec![
                pred(&ints, "y≥1"),
                pred(&ints, "x÷y=1"),
                pred(&ints, "y≠0"),
                pred(&ints, "x=1"),
                pred(&ints, "∀z·z÷y=1"),
                pred(&ints, "x≠0"),
            ]
        );

        // Selection: the interval (CTXHYP, ALLHYP] marks ALLHYP's
        // hypotheses; the single hint marks PRD0. WD conjuncts are
        // never selected.
        assert!(seq.is_selected(&pred(&ints, "x÷y=1")));
        assert!(seq.is_selected(&pred(&ints, "x=1")));
        assert!(seq.is_selected(&pred(&ints, "y≥1")));
        assert!(!seq.is_selected(&pred(&ints, "y≠0")));
        assert!(!seq.is_selected(&pred(&ints, "∀z·z÷y=1")));
    }

    /// A file registered under a directory-prefixed path (the loose
    /// multi-project layout) still resolves its basename parent-set
    /// handles: the handle is looked up inside the referring file's
    /// directory.
    #[test]
    fn parent_sets_resolve_inside_the_referring_files_directory() {
        let sequents = formatdoc!(
            r#"<org.eventb.core.poSequent name="evt/inv1/INV" org.eventb.core.poStamp="0">
            <org.eventb.core.poPredicateSet name="SEQHYP" org.eventb.core.parentSet="{SET_PREFIX}ALLHYP"/>
            <org.eventb.core.poPredicate name="SEQG" org.eventb.core.predicate="x=1"/>
            </org.eventb.core.poSequent>"#
        );
        let file = fixture_at("proj/M.bpo", &sequents);
        let seq = file.load("proj/M.bpo", "evt/inv1/INV").expect("loads");
        let ints = ints();
        assert_eq!(seq.goal(), &pred(&ints, "x=1"));
        let hyps: Vec<_> = seq.hyp_iter().cloned().collect();
        assert_eq!(
            hyps,
            vec![
                pred(&ints, "y≥1"),
                pred(&ints, "x÷y=1"),
                pred(&ints, "y≠0"),
                pred(&ints, "x=1"),
            ]
        );
    }

    #[test]
    fn wd_conjuncts_keep_negative_literals_parse_normal() {
        // The WD lemma builder folds −(2) into a negative literal, but
        // every parsed formula in the sequent keeps the −(lit) shape;
        // a folded hypothesis would never match a stored proof's
        // recorded dependencies.
        let ints = ints();
        let mut hyps = Vec::new();
        add_wd_predicates(&pred(&ints, "x÷(−2)=1"), &mut hyps);
        assert_eq!(hyps, vec![pred(&ints, "−2≠0")]);
    }

    #[test]
    fn wd_obligations_skip_the_goal_wd() {
        let sequents = formatdoc!(
            r#"<org.eventb.core.poSequent name="evt/act1/WD" org.eventb.core.poStamp="0">
            <org.eventb.core.poPredicateSet name="SEQHYP" org.eventb.core.parentSet="{SET_PREFIX}CTXHYP"/>
            <org.eventb.core.poPredicate name="SEQG" org.eventb.core.predicate="y÷y=1"/>
            </org.eventb.core.poSequent>"#
        );
        let file = fixture(&sequents);
        let seq = file.load("M.bpo", "evt/act1/WD").expect("loads");
        let ints = ints();
        // Only CTXHYP's hypothesis; no y≠0 from the goal.
        let hyps: Vec<_> = seq.hyp_iter().cloned().collect();
        assert_eq!(hyps, vec![pred(&ints, "y≥1")]);
    }

    #[test]
    fn wd_conjuncts_deduplicate_against_hypotheses() {
        let sequents = formatdoc!(
            r#"<org.eventb.core.poSequent name="evt/inv2/INV" org.eventb.core.poStamp="0">
            <org.eventb.core.poPredicateSet name="SEQHYP" org.eventb.core.parentSet="{SET_PREFIX}ALLHYP"/>
            <org.eventb.core.poPredicate name="SEQG" org.eventb.core.predicate="x÷y=2"/>
            </org.eventb.core.poSequent>"#
        );
        let file = fixture(&sequents);
        let seq = file.load("M.bpo", "evt/inv2/INV").expect("loads");
        let ints = ints();
        // The goal's WD y≠0 is already present as the WD of x÷y=1.
        let count = seq
            .hyp_iter()
            .filter(|hyp| **hyp == pred(&ints, "y≠0"))
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn problems_are_reported() {
        let sequents = formatdoc!(
            r#"<org.eventb.core.poSequent name="evt/bad/INV" org.eventb.core.poStamp="0">
            <org.eventb.core.poPredicateSet name="SEQHYP" org.eventb.core.parentSet="{SET_PREFIX}MISSING"/>
            <org.eventb.core.poPredicate name="SEQG" org.eventb.core.predicate="x=1"/>
            </org.eventb.core.poSequent>
            <org.eventb.core.poSequent name="evt/undeclared/INV" org.eventb.core.poStamp="0">
            <org.eventb.core.poPredicateSet name="SEQHYP" org.eventb.core.parentSet="{SET_PREFIX}CTXHYP"/>
            <org.eventb.core.poPredicate name="SEQG" org.eventb.core.predicate="w=1"/>
            </org.eventb.core.poSequent>
            <org.eventb.core.poSequent name="evt/nogoal/INV" org.eventb.core.poStamp="0">
            <org.eventb.core.poPredicateSet name="SEQHYP" org.eventb.core.parentSet="{SET_PREFIX}CTXHYP"/>
            </org.eventb.core.poSequent>"#
        );
        let file = fixture(&sequents);
        assert!(
            file.load("M.bpo", "evt/bad/INV")
                .unwrap_err()
                .contains("dangling")
        );
        assert!(
            file.load("M.bpo", "evt/undeclared/INV")
                .unwrap_err()
                .contains("undeclared")
        );
        assert!(
            file.load("M.bpo", "evt/nogoal/INV")
                .unwrap_err()
                .contains("no goal")
        );
        assert!(
            file.load("M.bpo", "evt/absent/INV")
                .unwrap_err()
                .contains("no obligation")
        );
    }
}
