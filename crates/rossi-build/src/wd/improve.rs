//! WD lemma simplification — port of rodin-ast's
//! `org.eventb.internal.core.ast.wd.WDImprover` and its `Node`/`Lemma`
//! classes.
//!
//! The computed lemma is decomposed into a tree of conjunctions,
//! implications, universal quantifiers, and opaque leaf predicates. Each
//! leaf is normalized so that predicates under different quantifier
//! prefixes become comparable (Rodin shifts De Bruijn indices as if all
//! quantifiers were hoisted to the root; we rename bound identifiers to
//! positional markers, which is the same equivalence). The tree is then
//! simplified by marking subsumed nodes:
//!
//! - a leaf equal to one of the antecedents in force is dropped;
//! - duplicated antecedents are dropped;
//! - a lemma (antecedent set ⊢ consequent) subsumed by an already-known
//!   lemma — same consequent, smaller antecedent set — is dropped, and
//!   conversely evicts weaker known lemmas.
//!
//! Rebuilding the tree afterwards (subsumed nodes become `⊤`, which the
//! smart constructors absorb) yields Rodin's simplified WD predicate.

use std::collections::BTreeSet;

use rossi::ast::predicate::{LogicalOp, Quantifier};
use rossi::{Predicate, TypedIdentifier};

use crate::wd::builder as fb;
use crate::wd::normal::{BinderRewriter, NamePolicy};
use crate::wd::render::render_predicate;

/// Simplify a computed WD lemma. `Predicate::True` means the whole
/// condition was discharged structurally.
pub fn improve(lemma: Predicate) -> Predicate {
    let mut tree = Tree { nodes: Vec::new() };
    let root = tree.build(lemma);
    tree.normalize(root, &mut Vec::new(), &mut 0);
    let mut known: Vec<Lemma> = Vec::new();
    tree.simplify(root, &mut known, &BTreeSet::new());
    tree.original(root)
}

type NodeId = usize;

struct Tree {
    nodes: Vec<Node>,
}

struct Node {
    kind: Kind,
    subsumed: bool,
}

enum Kind {
    /// Flattened conjunction.
    Land(Vec<NodeId>),
    Limp(NodeId, NodeId),
    Forall(Vec<TypedIdentifier>, NodeId),
    /// Anything else — including `∨`, `⇔`, `∃` and relational predicates
    /// — is an opaque leaf, exactly like Rodin's `NodePred`.
    Leaf {
        original: Predicate,
        /// Set by [`Tree::normalize`]; the comparison key.
        normalized: Option<String>,
        /// The marker-renamed predicate, for composing normalized keys
        /// of enclosing implication nodes.
        normalized_pred: Option<Predicate>,
    },
}

/// An implication in comparable form: a set of antecedent keys and a
/// consequent key. Subsumption: A subsumes B iff same consequent and
/// A's antecedents ⊆ B's antecedents.
#[derive(Clone)]
struct Lemma {
    antecedents: BTreeSet<String>,
    consequent: String,
    origin: NodeId,
}

impl Lemma {
    fn subsumes(&self, other: &Lemma) -> bool {
        self.consequent == other.consequent && self.antecedents.is_subset(&other.antecedents)
    }
}

impl Tree {
    fn add(&mut self, kind: Kind) -> NodeId {
        self.nodes.push(Node {
            kind,
            subsumed: false,
        });
        self.nodes.len() - 1
    }

    /// Decompose the lemma: ∧-chains (any nesting) become n-ary `Land`
    /// nodes — mirroring Rodin's `flatten()` before improvement — `⇒`
    /// becomes `Limp`, `∀` becomes `Forall`, the rest are leaves.
    fn build(&mut self, p: Predicate) -> NodeId {
        match p {
            Predicate::Logical {
                op: LogicalOp::And, ..
            } => {
                let mut conjuncts = Vec::new();
                collect_conjuncts(p, &mut conjuncts);
                let children: Vec<NodeId> = conjuncts.into_iter().map(|c| self.build(c)).collect();
                self.add(Kind::Land(children))
            }
            Predicate::Logical {
                op: LogicalOp::Implies,
                left,
                right,
            } => {
                let l = self.build(*left);
                let r = self.build(*right);
                self.add(Kind::Limp(l, r))
            }
            Predicate::Quantified {
                quantifier: Quantifier::ForAll,
                identifiers,
                predicate,
            } => {
                let child = self.build(*predicate);
                self.add(Kind::Forall(identifiers, child))
            }
            other => self.add(Kind::Leaf {
                original: other,
                normalized: None,
                normalized_pred: None,
            }),
        }
    }

    /// Assign root-first slots to the tree-level quantifier bindings and
    /// normalize every leaf against them. Equivalent to Rodin's
    /// `boundIdentifiersEqualizer`: a binder's identity becomes its
    /// position in the (virtually hoisted) quantifier prefix.
    fn normalize(&mut self, id: NodeId, scope: &mut Vec<(String, String)>, next_slot: &mut usize) {
        match &self.nodes[id].kind {
            Kind::Land(children) => {
                for child in children.clone() {
                    self.normalize(child, scope, &mut next_slot.clone());
                }
            }
            Kind::Limp(l, r) => {
                let (l, r) = (*l, *r);
                self.normalize(l, scope, &mut next_slot.clone());
                self.normalize(r, scope, &mut next_slot.clone());
            }
            Kind::Forall(decls, child) => {
                let child = *child;
                let names: Vec<String> = decls.iter().map(|d| d.name.clone()).collect();
                let depth = scope.len();
                let mut slot = *next_slot;
                for name in names {
                    scope.push((name, format!("${slot}")));
                    slot += 1;
                }
                self.normalize(child, scope, &mut slot);
                scope.truncate(depth);
            }
            Kind::Leaf { .. } => {
                let mut renamer =
                    BinderRewriter::with_scope(scope.clone(), MarkerPolicy { counter: 0 });
                let Kind::Leaf {
                    original,
                    normalized,
                    normalized_pred,
                } = &mut self.nodes[id].kind
                else {
                    unreachable!()
                };
                let renamed = renamer.pred(original);
                *normalized = Some(render_predicate(&renamed));
                *normalized_pred = Some(renamed);
            }
        }
    }

    /// The node as a normalized predicate (`asPredicate(fb, false)`):
    /// subsumed nodes are `⊤`, quantifiers are transparent.
    fn normalized_pred(&self, id: NodeId) -> Predicate {
        let node = &self.nodes[id];
        if node.subsumed {
            return Predicate::True;
        }
        match &node.kind {
            Kind::Land(children) => {
                fb::land(children.iter().map(|c| self.normalized_pred(*c)).collect())
            }
            Kind::Limp(l, r) => fb::limp(self.normalized_pred(*l), self.normalized_pred(*r)),
            Kind::Forall(_, child) => self.normalized_pred(*child),
            Kind::Leaf {
                normalized_pred, ..
            } => normalized_pred.clone().expect("normalize() ran"),
        }
    }

    fn normalized_key(&self, id: NodeId) -> String {
        match &self.nodes[id].kind {
            Kind::Leaf { normalized, .. } if !self.nodes[id].subsumed => {
                normalized.clone().expect("normalize() ran")
            }
            _ => render_predicate(&self.normalized_pred(id)),
        }
    }

    /// Collect this subtree's antecedent keys into `set`. A predicate
    /// already present marks the node subsumed (duplicate antecedents
    /// collapse), exactly like `Node.addPredicateToSet`.
    fn collect_antecedents(&mut self, id: NodeId, set: &mut BTreeSet<String>) {
        match &self.nodes[id].kind {
            Kind::Land(children) => {
                for child in children.clone() {
                    self.collect_antecedents(child, set);
                }
            }
            Kind::Forall(_, child) => {
                let child = *child;
                self.collect_antecedents(child, set);
            }
            Kind::Limp(..) | Kind::Leaf { .. } => self.add_predicate_to_set(id, set),
        }
    }

    fn add_predicate_to_set(&mut self, id: NodeId, set: &mut BTreeSet<String>) {
        if self.nodes[id].subsumed {
            return;
        }
        let key = self.normalized_key(id);
        if !set.insert(key) {
            self.nodes[id].subsumed = true;
        }
    }

    fn simplify(&mut self, id: NodeId, known: &mut Vec<Lemma>, antecedents: &BTreeSet<String>) {
        if self.nodes[id].subsumed {
            return;
        }
        match &self.nodes[id].kind {
            Kind::Land(children) => {
                for child in children.clone() {
                    self.simplify(child, known, antecedents);
                }
            }
            Kind::Forall(_, child) => {
                let child = *child;
                self.simplify(child, known, antecedents);
            }
            Kind::Limp(l, r) => {
                let (l, r) = (*l, *r);
                // The left side is simplified against a private copy of
                // the known lemmas (its discoveries don't leak out) and
                // no antecedents. This must be a full deep clone, not a
                // truncate-to-len / restore: `add_lemma` doesn't only
                // append — it also *evicts* (`known.remove(i)`) every
                // parent lemma the left side subsumes, so trimming back to
                // the prior length would silently resurrect those evicted
                // lemmas. Cloning is the cheapest trivially-correct
                // isolation; `known` is small (a few lemmas per branch).
                let mut left_known: Vec<Lemma> = known.clone();
                self.simplify(l, &mut left_known, &BTreeSet::new());
                // The right side gains the left side as hypotheses.
                let mut right_antes = antecedents.clone();
                self.collect_antecedents(l, &mut right_antes);
                self.simplify(r, known, &right_antes);
            }
            Kind::Leaf { .. } => {
                let key = self.normalized_key(id);
                if antecedents.contains(&key) {
                    self.nodes[id].subsumed = true;
                    return;
                }
                let lemma = Lemma {
                    antecedents: antecedents.clone(),
                    consequent: key,
                    origin: id,
                };
                self.add_lemma(known, lemma);
            }
        }
    }

    /// `Lemma.addToSet`: an existing lemma that subsumes the new one
    /// kills it; otherwise the new one evicts every lemma it subsumes.
    /// Never marks both sides. The eviction (`known.remove`) is why
    /// `simplify`'s `Limp` branch isolates the left side with a deep
    /// `known.clone()` rather than a truncate/restore.
    fn add_lemma(&mut self, known: &mut Vec<Lemma>, lemma: Lemma) {
        let mut i = 0;
        while i < known.len() {
            if known[i].subsumes(&lemma) {
                self.nodes[lemma.origin].subsumed = true;
                return;
            }
            if lemma.subsumes(&known[i]) {
                let evicted = known.remove(i);
                self.nodes[evicted.origin].subsumed = true;
                continue;
            }
            i += 1;
        }
        known.push(lemma);
    }

    /// Rebuild the simplified predicate from the original (source-form)
    /// leaves; subsumed nodes become `⊤` and evaporate in the smart
    /// constructors.
    fn original(&self, id: NodeId) -> Predicate {
        let node = &self.nodes[id];
        if node.subsumed {
            return Predicate::True;
        }
        match &node.kind {
            Kind::Land(children) => fb::land(children.iter().map(|c| self.original(*c)).collect()),
            Kind::Limp(l, r) => fb::limp(self.original(*l), self.original(*r)),
            Kind::Forall(decls, child) => fb::forall(decls.clone(), self.original(*child)),
            Kind::Leaf { original, .. } => original.clone(),
        }
    }
}

fn collect_conjuncts(p: Predicate, out: &mut Vec<Predicate>) {
    match p {
        Predicate::Logical {
            op: LogicalOp::And,
            left,
            right,
        } => {
            collect_conjuncts(*left, out);
            collect_conjuncts(*right, out);
        }
        other => out.push(other),
    }
}

// ---------------------------------------------------------------------
// Leaf normalization: bound-identifier renaming
// ---------------------------------------------------------------------

/// Renames every bound identifier to a globally unique positional marker
/// (`$iN` in declaration preorder) so predicates under different
/// quantifier prefixes become comparable — a name-based equivalent of
/// comparing shifted De Bruijn terms (alpha-equivalence). Tree-level
/// binders enter the rewriter already mapped to their root-first slot
/// (`$0`, `$1`, …); `$` cannot occur in an Event-B identifier, so markers
/// never collide with free names. Drives the shared [`BinderRewriter`].
struct MarkerPolicy {
    counter: usize,
}

impl NamePolicy for MarkerPolicy {
    const NEEDS_NODE_FREE: bool = false;
    fn choose(&mut self, _original: &str, _taken: &dyn Fn(&str) -> bool) -> String {
        let marker = format!("$i{}", self.counter);
        self.counter += 1;
        marker
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rossi::parse_predicate_str;

    fn imp(src: &str) -> String {
        render_predicate(&improve(parse_predicate_str(src).unwrap()))
    }

    #[test]
    fn duplicate_conjuncts_collapse() {
        // Identical consequents under the same antecedents: the second
        // lemma is subsumed by the first.
        assert_eq!(imp("x∈dom(f)∧f∈S ⇸ T∧x∈dom(f)"), "x∈dom(f)∧f∈S ⇸ T");
    }

    #[test]
    fn consequent_equal_to_antecedent_collapses() {
        assert_eq!(imp("x∈dom(f)⇒x∈dom(f)∧f∈S ⇸ T"), "x∈dom(f)⇒f∈S ⇸ T");
    }

    #[test]
    fn weaker_lemma_is_subsumed_by_hypothesis_free_one() {
        // f∈S⇸T holds unconditionally, so the conditional copy under
        // x∈dom(f) is subsumed; the implication then collapses entirely.
        assert_eq!(imp("f∈S ⇸ T∧(x∈dom(f)⇒f∈S ⇸ T)"), "f∈S ⇸ T");
    }

    #[test]
    fn stronger_lemma_evicts_weaker_one() {
        // The unconditional lemma arrives second and evicts the
        // conditional occurrence.
        assert_eq!(imp("(x∈dom(f)⇒f∈S ⇸ T)∧f∈S ⇸ T"), "f∈S ⇸ T");
    }

    #[test]
    fn quantifier_prefixes_are_equalized() {
        // The same leaf at different binding depths: ∀x·P⇒(∀y·P∧Q)
        // where P doesn't mention y — Rodin's canonical example. The
        // inner copy of x∈dom(f) is subsumed by the antecedent.
        assert_eq!(
            imp("∀x·x∈dom(f)⇒(∀y·x∈dom(f)∧y∈dom(f))"),
            "∀x·x∈dom(f)⇒(∀y·y∈dom(f))"
        );
    }

    #[test]
    fn alpha_equivalent_leaves_match_across_branches() {
        // Two sibling quantifiers binding different names: the leaves
        // normalize to the same slot and the second lemma is subsumed.
        assert_eq!(imp("(∀x·x∈S)∧(∀y·y∈S)"), "∀x·x∈S");
    }

    #[test]
    fn fully_subsumed_tree_collapses_to_true() {
        assert_eq!(
            improve(parse_predicate_str("x∈S⇒x∈S∧x∈S").unwrap()),
            {
                // limp() already kills the first copy; the improver kills
                // the rest.
                Predicate::True
            }
        );
    }
}
