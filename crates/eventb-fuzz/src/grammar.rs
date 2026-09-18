//! The tree-sitter grammar, read as data.
//!
//! `editors/tree-sitter-eventb/src/grammar.json` is the generated, fully
//! desugared form of `grammar.js`: a plain context-free grammar with no
//! external scanner and no lexer state, so it can be walked as a generator
//! rather than only as a parser. It is the fuzzer's generation source; the pest
//! grammar is a PEG whose ordered choice and sixteen negative-lookahead guards
//! make a derivation mean something other than what it reads, so it serves as
//! the acceptance oracle instead.

use std::collections::BTreeMap;

use serde::Deserialize;

/// One node of a grammar rule's right-hand side.
///
/// Every variant tree-sitter emits for this grammar is represented. The
/// precedence wrappers carry no information a generator can use: they steer the
/// LR parser's conflict resolution, and any derivation they permit is still a
/// derivation, so they are transparent here.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum Node {
    /// Concatenation.
    #[serde(rename = "SEQ")]
    Seq { members: Vec<Node> },
    /// Alternation.
    #[serde(rename = "CHOICE")]
    Choice { members: Vec<Node> },
    /// Zero or more repetitions.
    #[serde(rename = "REPEAT")]
    Repeat { content: Box<Node> },
    /// One or more repetitions.
    #[serde(rename = "REPEAT1")]
    Repeat1 { content: Box<Node> },
    /// A reference to another rule.
    #[serde(rename = "SYMBOL")]
    Symbol { name: String },
    /// A literal terminal.
    #[serde(rename = "STRING")]
    String { value: String },
    /// A regular-expression terminal.
    #[serde(rename = "PATTERN")]
    Pattern { value: String },
    /// A node that wraps another without changing what it derives.
    ///
    /// Five kinds collapse to one here because a generator can tell them
    /// apart only by what they produce, and they all produce their content:
    /// `TOKEN` assembles a terminal without interleaved extras, `ALIAS`
    /// renames a node in the parse tree, the three `PREC` forms steer the LR
    /// parser's conflict resolution, and `RESERVED` swaps the reserved word
    /// set its subtree lexes under. Any derivation they permit is still a
    /// derivation.
    ///
    /// `RESERVED` does restrict what the *parser* accepts, but not what the
    /// grammar derives: the word sets live in a top-level table this model
    /// does not read, and the terminal a reserved word would collide with is
    /// the `identifier` pattern, which derives whatever the pattern allows
    /// either way.
    #[serde(rename = "TOKEN")]
    #[serde(alias = "ALIAS")]
    #[serde(alias = "PREC")]
    #[serde(alias = "PREC_LEFT")]
    #[serde(alias = "PREC_RIGHT")]
    #[serde(alias = "PREC_DYNAMIC")]
    #[serde(alias = "RESERVED")]
    Wrapper { content: Box<Node> },
    /// Like a [`Node::Wrapper`], and additionally forbidden to be preceded by
    /// whitespace. Kept distinct because that is a fact about the emitted
    /// text: the hyphenated tail of a component name must abut the name, or
    /// `ctx-1` reads as a subtraction instead.
    #[serde(rename = "IMMEDIATE_TOKEN")]
    ImmediateToken { content: Box<Node> },
    /// A named child slot. Transparent for what it derives, but the name says
    /// what role the child plays, which is how declarations are told from
    /// references.
    #[serde(rename = "FIELD")]
    Field { name: String, content: Box<Node> },
    /// The empty string.
    #[serde(rename = "BLANK")]
    Blank,
}

impl Node {
    /// The node this one wraps, for every kind that only wraps.
    ///
    /// One place answers "is this transparent, and to what", so a new wrapper
    /// kind is handled everywhere at once instead of in each match that walks
    /// the tree.
    pub fn transparent(&self) -> Option<&Node> {
        match self {
            Node::Wrapper { content }
            | Node::ImmediateToken { content }
            | Node::Field { content, .. } => Some(content),
            _ => None,
        }
    }
}

/// A parsed `grammar.json`, with the derivation-cost table a bounded generator
/// needs.
#[derive(Debug)]
pub struct Grammar {
    /// Rules by name. Sorted, so every traversal is deterministic.
    pub rules: BTreeMap<String, Node>,
    /// For each rule, the smallest number of nested rule expansions needed to
    /// derive any string from it. See [`Grammar::min_depth`].
    min_depths: BTreeMap<String, usize>,
}

#[derive(Deserialize)]
struct RawGrammar {
    rules: BTreeMap<String, Node>,
}

/// Stands in for "no terminating derivation known yet" while the cost table is
/// being solved. Large enough to be unreachable as a real depth, small enough
/// that adding one cannot overflow.
const UNREACHABLE: usize = usize::MAX / 4;

impl Grammar {
    /// Parse a `grammar.json` document.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let raw: RawGrammar = serde_json::from_str(json)?;
        let min_depths = solve_min_depths(&raw.rules);
        Ok(Self {
            rules: raw.rules,
            min_depths,
        })
    }

    /// The rule named `name`.
    pub fn rule(&self, name: &str) -> Option<&Node> {
        self.rules.get(name)
    }

    /// The cheapest derivation depth of `node`, counting one per rule
    /// expansion.
    ///
    /// A bounded generator uses this to keep from painting itself into a
    /// corner: with `budget` expansions left it may only take alternatives
    /// whose minimum depth fits, which is what stops the mutually recursive
    /// expression and predicate rules from running away.
    pub fn min_depth(&self, node: &Node) -> usize {
        node_min_depth(node, &self.min_depths)
    }
}

/// Least-fixed-point solve of the per-rule minimum derivation depth.
///
/// Every rule starts unreachable and is relaxed until nothing improves. A
/// grammar whose rules all terminate leaves no rule unreachable; one that does
/// not (a rule that can only expand into itself) keeps its sentinel, and the
/// generator's budget check then simply never selects it.
fn solve_min_depths(rules: &BTreeMap<String, Node>) -> BTreeMap<String, usize> {
    let mut depths: BTreeMap<String, usize> = rules
        .keys()
        .map(|name| (name.clone(), UNREACHABLE))
        .collect();
    loop {
        let mut changed = false;
        for (name, node) in rules {
            let depth = node_min_depth(node, &depths);
            if depth < depths[name] {
                depths.insert(name.clone(), depth);
                changed = true;
            }
        }
        if !changed {
            return depths;
        }
    }
}

fn node_min_depth(node: &Node, depths: &BTreeMap<String, usize>) -> usize {
    if let Some(inner) = node.transparent() {
        return node_min_depth(inner, depths);
    }
    match node {
        // A terminal needs no expansion at all.
        Node::String { .. } | Node::Pattern { .. } | Node::Blank => 0,
        // An empty repetition is a terminating derivation.
        Node::Repeat { .. } => 0,
        Node::Repeat1 { content } => node_min_depth(content, depths),
        // A sequence costs as much as its most expensive member: all of them
        // must be derived.
        Node::Seq { members } => members
            .iter()
            .map(|member| node_min_depth(member, depths))
            .max()
            .unwrap_or(0),
        // A choice costs as little as its cheapest alternative.
        Node::Choice { members } => members
            .iter()
            .map(|member| node_min_depth(member, depths))
            .min()
            .unwrap_or(0),
        Node::Symbol { name } => depths
            .get(name)
            .copied()
            .unwrap_or(UNREACHABLE)
            .saturating_add(1),
        // Every wrapper kind returned above.
        Node::Wrapper { .. } | Node::ImmediateToken { .. } | Node::Field { .. } => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Grammar {
        Grammar::from_json(json).expect("grammar parses")
    }

    /// The cost of deriving the rule named `name`, through the same entry
    /// point the generator uses.
    fn cost(grammar: &Grammar, name: &str) -> usize {
        grammar.min_depth(grammar.rule(name).expect("rule exists"))
    }

    #[test]
    fn reads_rules_and_terminals() {
        let grammar = parse(
            r#"{"name":"t","rules":{
                "a":{"type":"SEQ","members":[
                    {"type":"STRING","value":"x"},
                    {"type":"PATTERN","value":"[0-9]+"}]}}}"#,
        );
        assert!(matches!(grammar.rule("a"), Some(Node::Seq { .. })));
        assert_eq!(cost(&grammar, "a"), 0);
    }

    #[test]
    fn a_recursive_rule_costs_its_cheapest_exit() {
        // expr -> "1" | expr "+" expr
        let grammar = parse(
            r#"{"name":"t","rules":{
                "expr":{"type":"CHOICE","members":[
                    {"type":"STRING","value":"1"},
                    {"type":"SEQ","members":[
                        {"type":"SYMBOL","name":"expr"},
                        {"type":"STRING","value":"+"},
                        {"type":"SYMBOL","name":"expr"}]}]}}}"#,
        );
        assert_eq!(cost(&grammar, "expr"), 0);
        let Some(Node::Choice { members }) = grammar.rule("expr") else {
            panic!("expected a choice");
        };
        // The literal exit is free; the recursive arm costs one expansion.
        assert_eq!(grammar.min_depth(&members[0]), 0);
        assert_eq!(grammar.min_depth(&members[1]), 1);
    }

    #[test]
    fn indirect_recursion_accumulates_one_step_per_rule() {
        // a -> b, b -> c, c -> "x"
        let grammar = parse(
            r#"{"name":"t","rules":{
                "a":{"type":"SYMBOL","name":"b"},
                "b":{"type":"SYMBOL","name":"c"},
                "c":{"type":"STRING","value":"x"}}}"#,
        );
        assert_eq!(cost(&grammar, "c"), 0);
        assert_eq!(cost(&grammar, "b"), 1);
        assert_eq!(cost(&grammar, "a"), 2);
    }

    #[test]
    fn a_rule_that_never_terminates_stays_unreachable() {
        let grammar = parse(
            r#"{"name":"t","rules":{
                "loop":{"type":"SEQ","members":[
                    {"type":"SYMBOL","name":"loop"},
                    {"type":"STRING","value":"x"}]}}}"#,
        );
        assert!(cost(&grammar, "loop") >= UNREACHABLE);
    }

    #[test]
    fn an_empty_repetition_is_free_but_repeat1_is_not() {
        let grammar = parse(
            r#"{"name":"t","rules":{
                "a":{"type":"REPEAT","content":{"type":"SYMBOL","name":"b"}},
                "b":{"type":"REPEAT1","content":{"type":"SYMBOL","name":"c"}},
                "c":{"type":"STRING","value":"x"}}}"#,
        );
        assert_eq!(cost(&grammar, "a"), 0);
        assert_eq!(cost(&grammar, "b"), 1);
    }
}
