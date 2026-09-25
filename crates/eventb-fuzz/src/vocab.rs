//! The names a generated model may use.
//!
//! Identifiers are the one terminal a grammar walk cannot get right on its
//! own: the grammar says "any word", but a word that collides with a keyword
//! or a reserved name changes how the text parses, and a word that was never
//! declared makes every later static check fail for an uninteresting reason.
//! This module owns both halves — which words are legal, and which words are in
//! scope right now.
//!
//! The legality half defers entirely to `rossi`'s own tables
//! ([`rossi::builtins::is_reserved_name`] and [`rossi::keywords::is_keyword`]),
//! so the fuzzer cannot drift away from the parser it is testing.

use std::collections::HashSet;

use crate::choice::{ByteSource, ByteSourceExt};

/// Whether `word` may name a user identifier.
///
/// Composes the two blocklists a caller has to apply when *introducing* a
/// name: the mathematical language's reserved words (exact-case, per
/// `kernel_lang` §2.2, plus rossi's own keyword-token and ASCII-operator
/// spellings) and the structural keywords (case-insensitive).
pub fn is_usable_name(word: &str) -> bool {
    rossi::names::is_valid_math_identifier(word)
        && !rossi::builtins::is_reserved_name(word)
        && !rossi::keywords::is_keyword(word)
}

/// Short, ordinary names, the kind real models use.
const PLAIN_STEMS: &[&str] = &[
    "x", "y", "z", "n", "m", "k", "s", "t", "f", "g", "r", "v", "c", "p", "q", "d", "e", "u",
];

/// Names that are legal but sit next to something that is not: a keyword with
/// a suffix, a reserved word in the wrong case, a leading underscore, a word
/// that is a prefix of a keyword. These are where the word-boundary guards in
/// the pest grammar and the keyword extraction in the tree-sitter grammar
/// disagree, so a generator that only emits `x1` never probes them.
const RISKY_STEMS: &[&str] = &[
    "end_state",
    "events_of",
    "theorem_count",
    "variant_x",
    "wheres",
    "thenable",
    "anys",
    "Dom",
    "CARD",
    "Not",
    "Mod",
    "_private",
    "__",
    "en",
    "even",
    "machin",
    "contexts",
    "witnessed",
    "statuses",
];

/// Mints declaration names and hands out references to them.
///
/// A generated component declares before it refers: carrier sets, constants,
/// variables and event parameters are minted here and recorded, and every
/// identifier in a formula is drawn back out of the same pool. Scopes nest so
/// a quantifier's bound variable leaves the pool when its body ends.
#[derive(Debug, Default)]
pub struct Scope {
    /// One entry per nesting level; the last is the innermost.
    levels: Vec<Vec<String>>,
    /// Every name minted in this component, so a second declaration never
    /// shadows a first (a duplicate declaration is a static-check error, and
    /// the fault-injection mutators produce those deliberately). A set, not a
    /// list: it is consulted on every mint, which happens once per declared
    /// name in every generated model.
    minted: HashSet<String>,
    counter: usize,
}

impl Scope {
    /// An empty scope with one (component-level) level open.
    pub fn new() -> Self {
        Self {
            levels: vec![Vec::new()],
            minted: HashSet::new(),
            counter: 0,
        }
    }

    /// Open a nested scope, for a quantifier or comprehension binder.
    pub fn push(&mut self) {
        self.levels.push(Vec::new());
    }

    /// Close the innermost scope, dropping its names.
    pub fn pop(&mut self) {
        if self.levels.len() > 1 {
            self.levels.pop();
        }
    }

    /// A fresh name, recorded in the innermost scope.
    ///
    /// Guaranteed usable (never a keyword or reserved word) and unique within
    /// the component.
    pub fn mint(&mut self, source: &mut dyn ByteSource) -> String {
        for _ in 0..16 {
            let stem = if source.ratio(1, 5) {
                source.pick(RISKY_STEMS)
            } else {
                source.pick(PLAIN_STEMS)
            };
            let Some(stem) = stem else { continue };
            let candidate = if source.ratio(1, 2) {
                (*stem).to_string()
            } else {
                format!("{stem}{}", source.below(10))
            };
            if is_usable_name(&candidate) && !self.minted.contains(&candidate) {
                self.record(candidate.clone());
                return candidate;
            }
        }
        // Fall back to a counter-suffixed name, skipping any the stem-plus-digit
        // path has already used.
        loop {
            self.counter += 1;
            let candidate = format!("v{}_", self.counter);
            if !self.minted.contains(&candidate) {
                self.record(candidate.clone());
                return candidate;
            }
        }
    }

    fn record(&mut self, name: String) {
        self.minted.insert(name.clone());
        if let Some(level) = self.levels.last_mut() {
            level.push(name);
        }
    }

    /// A name that is currently in scope, or `None` when nothing is declared.
    ///
    /// Innermost names are not preferred: drawing uniformly keeps outer
    /// declarations live so a generated formula mixes bound and free names.
    pub fn reference(&self, source: &mut dyn ByteSource) -> Option<String> {
        let visible: Vec<&String> = self.levels.iter().flatten().collect();
        source.pick(&visible).map(|name| (*name).clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::choice::SplitMix64;

    #[test]
    fn every_stem_in_the_pools_is_a_usable_name() {
        for stem in PLAIN_STEMS.iter().chain(RISKY_STEMS) {
            assert!(is_usable_name(stem), "{stem:?} is not usable as a name");
        }
    }

    #[test]
    fn keywords_and_reserved_words_are_rejected() {
        for word in [
            "end", "END", "End", "machine", "sets", "theorem", "card", "dom", "TRUE", "bool",
            "prj1", "", "1x", "x-y",
        ] {
            assert!(!is_usable_name(word), "{word:?} should not be usable");
        }
    }

    #[test]
    fn minted_names_are_usable_and_unique() {
        let mut rng = SplitMix64::new(3);
        let mut scope = Scope::new();
        let names: Vec<String> = (0..500).map(|_| scope.mint(&mut rng)).collect();
        for name in &names {
            assert!(is_usable_name(name), "minted unusable name {name:?}");
        }
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "minted a duplicate name");
    }

    #[test]
    fn risky_stems_are_reached() {
        let mut rng = SplitMix64::new(5);
        let mut scope = Scope::new();
        let names: Vec<String> = (0..400).map(|_| scope.mint(&mut rng)).collect();
        assert!(
            names
                .iter()
                .any(|name| RISKY_STEMS.iter().any(|stem| name.starts_with(stem))),
            "risky names never generated"
        );
    }

    #[test]
    fn a_nested_scope_hides_its_names_once_closed() {
        let mut rng = SplitMix64::new(7);
        let mut scope = Scope::new();
        let outer = scope.mint(&mut rng);
        scope.push();
        let inner = scope.mint(&mut rng);
        let visible: Vec<String> = (0..50).filter_map(|_| scope.reference(&mut rng)).collect();
        assert!(visible.contains(&inner) && visible.contains(&outer));
        scope.pop();
        for _ in 0..50 {
            assert_ne!(scope.reference(&mut rng).as_deref(), Some(inner.as_str()));
        }
        assert_eq!(scope.minted.len(), 2, "minted names stay recorded");
    }

    #[test]
    fn reference_is_none_when_nothing_is_declared() {
        let mut rng = SplitMix64::new(11);
        assert!(Scope::new().reference(&mut rng).is_none());
    }
}
