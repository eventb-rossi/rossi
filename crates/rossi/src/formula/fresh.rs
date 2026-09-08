//! Fresh-name computation for bound identifiers.
//!
//! Declaration names are printing hints: whenever a formula is
//! rendered (or identifiers must be materialized, e.g. when turning
//! bound identifiers back into free ones), each declaration needs a
//! concrete name that does not collide with the names already visible
//! at that point. The solver keeps the hint when it is free and
//! otherwise increments a numeric suffix between the hint's stem and
//! its prime marks: `x` → `x0` → `x1`, `x1` → `x2`, `x'` → `x0'`.

use std::collections::HashSet;

use crate::builtins::is_reserved_name;
use crate::names::is_valid_math_identifier;

use super::decl::BoundIdentDecl;

/// Computes names that are fresh with respect to a set of used names
/// (and always lexically valid, never a reserved word).
pub struct FreshNameSolver {
    used: HashSet<String>,
}

impl FreshNameSolver {
    /// A solver considering `used` taken.
    pub fn new(used: impl IntoIterator<Item = String>) -> Self {
        FreshNameSolver {
            used: used.into_iter().collect(),
        }
    }

    /// Whether `name` is already taken.
    pub fn contains(&self, name: &str) -> bool {
        self.used.contains(name)
    }

    /// Marks `name` as taken.
    pub fn add(&mut self, name: impl Into<String>) {
        self.used.insert(name.into());
    }

    /// A name resembling `name` that is not taken, without recording
    /// it.
    pub fn solve(&self, name: &str) -> String {
        if self.is_acceptable(name) {
            return name.to_string();
        }
        let mut structured = StructuredName::parse(name);
        loop {
            structured.increment();
            let candidate = structured.render();
            if self.is_acceptable(&candidate) {
                return candidate;
            }
        }
    }

    /// A name resembling `name` that is not taken, recorded as taken.
    pub fn solve_and_add(&mut self, name: &str) -> String {
        let solved = self.solve(name);
        self.used.insert(solved.clone());
        solved
    }

    fn is_acceptable(&self, name: &str) -> bool {
        is_valid_math_identifier(name) && !is_reserved_name(name) && !self.used.contains(name)
    }
}

/// Resolves the printing names of a declaration list, left to right:
/// each declaration keeps its hint when free, gets a freshened variant
/// otherwise, and the chosen name is recorded so the declarations also
/// stay distinct from each other.
pub fn resolve_idents(decls: &[BoundIdentDecl], solver: &mut FreshNameSolver) -> Vec<String> {
    decls
        .iter()
        .map(|decl| solver.solve_and_add(decl.name()))
        .collect()
}

/// Resolves the printing names of a binding construct's declarations
/// against everything visible inside it.
///
/// A bound occurrence refers to its declaration by de Bruijn index, so a
/// declaration's name is free to change; what must not change is which
/// identifier the reader sees. The names that would capture an
/// occurrence in the binder's subtree are its own free identifiers
/// (`free`) plus the enclosing declarations that subtree reaches
/// (`dangling`, resolved against `enclosing`, innermost last). Seeding
/// the solver with both leaves every occurrence reading as itself.
///
/// This is the whole of the naming policy: both the pretty-printer and
/// any other consumer that has to report a bound identifier by name call
/// it, so the answers cannot diverge.
pub fn resolve_binder_names(
    decls: &[BoundIdentDecl],
    free: &[String],
    dangling: &[u32],
    enclosing: &[String],
) -> Vec<String> {
    let mut solver = FreshNameSolver::new(free.iter().cloned());
    for index in dangling {
        if let Some(i) = enclosing.len().checked_sub(1 + *index as usize) {
            solver.add(enclosing[i].clone());
        }
    }
    resolve_idents(decls, &mut solver)
}

/// An identifier split into stem, optional numeric suffix, and trailing
/// prime marks, so that incrementing touches only the number.
struct StructuredName {
    prefix: String,
    suffix: Option<u64>,
    quotes: String,
}

impl StructuredName {
    fn parse(name: &str) -> StructuredName {
        let stem = name.trim_end_matches('\'');
        let quotes = name[stem.len()..].to_string();
        let prefix = stem.trim_end_matches(|c: char| c.is_ascii_digit());
        let digits = &stem[prefix.len()..];
        // An over-long numeric suffix stays part of the stem; appending
        // fresh digits still terminates. An all-digit or empty stem
        // falls back to a neutral prefix so a valid candidate exists.
        let (prefix, suffix) = if digits.is_empty() {
            (prefix.to_string(), None)
        } else if let Ok(value) = digits.parse::<u64>() {
            (prefix.to_string(), Some(value))
        } else {
            (stem.to_string(), None)
        };
        let prefix = if prefix.is_empty() {
            "x".to_string()
        } else {
            prefix
        };
        let mut structured = StructuredName {
            prefix,
            suffix,
            quotes,
        };
        // A hint that cannot render as a lexically valid identifier
        // would make the solver loop forever — a numeric suffix never
        // repairs an invalid stem or a run of primes. Fall back to a
        // neutral shape: keep at most the single prime the lexicon
        // allows, and replace an unsalvageable stem outright.
        if !is_valid_math_identifier(&structured.candidate_shape()) {
            structured.quotes.truncate(1);
            if !is_valid_math_identifier(&structured.candidate_shape()) {
                structured.prefix = "x".to_string();
            }
        }
        structured
    }

    /// A representative numbered candidate, for lexical validation:
    /// every name the solver can produce from this shape is valid iff
    /// this one is.
    fn candidate_shape(&self) -> String {
        format!("{}0{}", self.prefix, self.quotes)
    }

    fn increment(&mut self) {
        self.suffix = Some(self.suffix.map_or(0, |n| n + 1));
    }

    fn render(&self) -> String {
        match self.suffix {
            Some(suffix) => format!("{}{}{}", self.prefix, suffix, self.quotes),
            None => format!("{}{}", self.prefix, self.quotes),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formula::FormulaFactory;

    fn solver(used: &[&str]) -> FreshNameSolver {
        FreshNameSolver::new(used.iter().map(|s| s.to_string()))
    }

    #[test]
    fn free_hints_are_kept() {
        assert_eq!(solver(&["y"]).solve("x"), "x");
    }

    #[test]
    fn conflicts_grow_a_numeric_suffix() {
        assert_eq!(solver(&["x"]).solve("x"), "x0");
        assert_eq!(solver(&["x", "x0", "x1"]).solve("x"), "x2");
        assert_eq!(solver(&["x1"]).solve("x1"), "x2");
    }

    #[test]
    fn primes_stay_behind_the_suffix() {
        assert_eq!(solver(&["x'"]).solve("x'"), "x0'");
        assert_eq!(solver(&["x'", "x0'"]).solve("x'"), "x1'");
    }

    #[test]
    fn reserved_words_are_never_produced() {
        // `card` is reserved vocabulary, so even an unused hint moves
        // to its numbered variant.
        assert_eq!(solver(&[]).solve("card"), "card0");
    }

    #[test]
    fn unrenderable_hints_terminate_with_a_valid_name() {
        // A doubled prime can never render as a valid identifier; the
        // solver must not spin on it. It keeps one prime and numbers.
        assert_eq!(solver(&["x'"]).solve("x''"), "x0'");
        // An invalid stem falls back to the neutral prefix.
        assert_eq!(solver(&[]).solve("a b"), "x0");
    }

    #[test]
    fn solve_and_add_makes_names_mutually_distinct() {
        let mut s = solver(&[]);
        assert_eq!(s.solve_and_add("x"), "x");
        assert_eq!(s.solve_and_add("x"), "x0");
        assert_eq!(s.solve_and_add("x"), "x1");
    }

    #[test]
    fn resolve_idents_freshens_against_used_and_each_other() {
        let ff = FormulaFactory::default_factory();
        let decls = vec![
            ff.bound_ident_decl("y", None, None, None),
            ff.bound_ident_decl("y", None, None, None),
            ff.bound_ident_decl("z", None, None, None),
        ];
        let mut s = solver(&["z"]);
        assert_eq!(resolve_idents(&decls, &mut s), ["y", "y0", "z0"]);
    }

    fn decls(names: &[&str]) -> Vec<BoundIdentDecl> {
        let ff = FormulaFactory::default_factory();
        names
            .iter()
            .map(|n| ff.bound_ident_decl(*n, None, None, None))
            .collect()
    }

    fn owned(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_string()).collect()
    }

    #[test]
    fn binder_names_avoid_the_free_identifiers_of_the_body() {
        // `x` occurs free below the binder, so the declaration cannot
        // also print as `x` without capturing it.
        assert_eq!(
            resolve_binder_names(&decls(&["x"]), &owned(&["x"]), &[], &[]),
            ["x0"]
        );
    }

    #[test]
    fn binder_names_avoid_the_enclosing_declarations_the_body_reaches() {
        // The body reaches one level out (index 0), which the enclosing
        // scope resolves to `x` — innermost last.
        assert_eq!(
            resolve_binder_names(&decls(&["x"]), &[], &[0], &owned(&["y", "x"])),
            ["x0"]
        );
        // An index the body never mentions constrains nothing.
        assert_eq!(
            resolve_binder_names(&decls(&["x"]), &[], &[1], &owned(&["y", "x"])),
            ["x"]
        );
    }

    #[test]
    fn binder_names_stay_distinct_from_each_other() {
        assert_eq!(
            resolve_binder_names(&decls(&["x", "x", "x"]), &[], &[], &[]),
            ["x", "x0", "x1"]
        );
    }

    #[test]
    fn a_dangling_index_past_the_enclosing_scope_is_ignored() {
        // Index 3 has no enclosing declaration; resolution must not
        // panic on the underflow, it simply constrains nothing.
        assert_eq!(
            resolve_binder_names(&decls(&["x"]), &[], &[3], &owned(&["x"])),
            ["x"]
        );
    }
}
