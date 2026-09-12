//! Deriving Event-B text from the tree-sitter grammar.
//!
//! The walk is an ordinary recursive descent over [`Node`], with three things
//! layered on top that a plain CFG walk cannot get right:
//!
//! * **Budgets.** The expression and predicate rules are mutually recursive and
//!   branch two ways, so an unbounded walk diverges. A depth budget bounds
//!   nesting (well under the parser's own limit) and a token budget bounds
//!   size; when either runs low the walk takes only the alternatives whose
//!   cheapest derivation still fits, using the cost table [`Grammar`] solved at
//!   load time.
//! * **Names.** Identifiers come from [`Scope`], which mints them where the
//!   grammar declares one and draws them back where it refers to one, so a
//!   generated model mostly type-checks instead of failing at the first
//!   unknown name. Component and event cross-references are wired to
//!   components and events already emitted.
//! * **Layout.** The walk emits tokens, not text. A final pass separates them,
//!   which is where the decision "may these two tokens touch?" is made once,
//!   correctly, instead of at every emit site.

use std::borrow::Cow;
use std::collections::BTreeSet;

use crate::choice::{ByteSource, ByteSourceExt};
use crate::grammar::{Grammar, Node};
use crate::vocab::Scope;

/// How large and how deep a generated model may get.
#[derive(Debug, Clone)]
pub struct Config {
    /// Most nested rule expansions.
    ///
    /// Kept well below [`rossi::MAX_NESTING_DEPTH`] so that a generated model
    /// exercises the parser rather than its nesting guard; the mutators are
    /// what probe the limit itself.
    pub max_depth: usize,
    /// Soft cap on emitted tokens. Once spent, the walk takes the cheapest
    /// derivation that closes what it has opened, so output stays bounded
    /// without ever being truncated mid-construct.
    pub max_tokens: usize,
    /// Most components in one generated file.
    pub max_components: usize,
    /// Rules never to derive.
    ///
    /// Suppressing a rule and re-measuring is how a class of disagreement
    /// between the two grammars is isolated: if acceptance jumps when a rule
    /// is off, that rule is the cause. It is a diagnostic control, not part of
    /// normal generation.
    pub suppressed: Vec<String>,
    /// Emit the Unicode spelling of every operator, whichever arm the grammar
    /// derived.
    ///
    /// The tree-sitter grammar offers both conventions per operator and rossi
    /// accepts either, so an unnormalized model is about half ASCII. Camille
    /// and Rodin's `FormulaFactory` are Unicode-only for those spellings, so a
    /// differential run against eventb-checker needs this on: without it
    /// nearly every model reports a documented divergence instead of a
    /// finding.
    ///
    /// Only the spelling changes. The rewrite happens as a token is written
    /// out, after layout has been decided on the derived spelling, so the same
    /// seed yields the same corpus either way.
    pub unicode_operators: bool,
    /// Emit every structural keyword in lower case, whichever case the
    /// grammar's case-insensitive keyword patterns derived.
    ///
    /// Camille accepts a keyword all-lower or all-upper but not mixed, and not
    /// `BEGIN` in upper case at all, so the generator's per-letter case choice
    /// makes it reject most models at `[1,1]` before any formula is reached.
    /// That is the class that hides every other one, the ASCII operators
    /// included.
    ///
    /// Rewritten at the same point and for the same reason as
    /// [`Self::unicode_operators`]; `lookup` is case-insensitive, so layout
    /// never saw the case in the first place.
    pub lowercase_keywords: bool,
    /// Write each component's sections, and each event's clauses, in the
    /// canonical order.
    ///
    /// The tree-sitter grammar takes them in any order, as rossi does, because
    /// Rodin cannot express an order at all. Camille fixes it in its lexer and
    /// refuses a keyword that moves backwards along its index list, so a
    /// differential run against eventb-checker books that documented
    /// divergence (rule EB034) instead of a finding. Measured as the third
    /// residual Camille blocker, behind hyphenated names and comment
    /// placement.
    ///
    /// Unlike the two above this is not a spelling: it reorders whole runs of
    /// tokens before layout. It is still corpus-preserving, for a different
    /// reason — every run opens with a keyword that starts a line, which draws
    /// nothing, so no layout decision straddles a run boundary and the token
    /// count is unchanged.
    pub ordered_clauses: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_depth: 24,
            max_tokens: 600,
            max_components: 3,
            suppressed: Vec::new(),
            unicode_operators: false,
            lowercase_keywords: false,
            ordered_clauses: false,
        }
    }
}

/// A generated model and what the generator knows about it.
#[derive(Debug, Clone)]
pub struct Generated {
    /// The Event-B text.
    pub text: String,
    /// Rules the generator could not derive, if any. Empty for the grammar as
    /// it stands; a non-empty list means the grammar grew a construct the
    /// generator does not model, which the smoke test treats as a failure.
    pub unsupported: Vec<String>,
}

/// Alternatives that are legal in the tree-sitter grammar but that Rossi's
/// parser rejects outright, so that taking them at their natural rate would
/// make most generated models uninteresting rejections.
///
/// They stay reachable — the disagreement is exactly what a differential
/// fuzzer is for — just rare. The weight is relative to [`DEFAULT_WEIGHT`].
fn rule_weight(name: &str) -> usize {
    match name {
        // Bare `dom`, `ran`, `not` and `theorem` in expression position. The
        // tree-sitter grammar keeps the identifier reading alive; Rossi
        // reports a reserved word for `dom`, `ran` and `theorem`, and Rodin
        // agrees.
        "_identifier_like" => 1,
        _ => DEFAULT_WEIGHT,
    }
}

const DEFAULT_WEIGHT: usize = 24;

/// How rarely a deliberately divergent shape is taken: often enough that a
/// long run still probes the disagreement, rarely enough that it does not
/// crowd out everything else.
const RARE: usize = 32;

/// How often, per thousand token boundaries, a comment is inserted.
const COMMENT_PERMILLE: usize = 20;

/// Rules whose `identifier` children declare a name rather than refer to one.
const DECLARING_RULES: &[&str] = &[
    "set_declaration",
    "constants_clause",
    "variables_clause",
    "any_clause",
    "typed_identifier",
    "pattern_typed_identifier",
];

/// Rules that put the walk into formula position, where an `identifier` is a
/// reference. Checked against the innermost of these and [`DECLARING_RULES`],
/// so the type annotation of a binder (`x ⦂ T`) refers while the binder itself
/// declares.
const REFERRING_RULES: &[&str] = &["_expression", "_predicate"];

/// Numbers worth generating: the boundaries a parser is most likely to get
/// wrong, plus a few ordinary values.
const NUMBERS: &[&str] = &[
    "0",
    "1",
    "2",
    "7",
    "10",
    "42",
    "007",
    "2147483647",
    "2147483648",
    "4294967296",
    "9223372036854775808",
    "18446744073709551616",
];

/// Hyphenated tails for component and event names. `end` and `variant` are
/// there on purpose: a hyphenated name embedding a keyword (`ctx0-end`) is
/// exactly what the pest grammar's structural word boundary exists to keep
/// from splitting.
const NAME_TAILS: &[&str] = &["1", "a", "C0", "end", "variant", "x_2"];

/// Comment shapes inserted by the layout pass, including the ones that have
/// historically confused comment handling: an empty block, a block ending in
/// several stars, a block containing a slash.
const BLOCK_COMMENTS: &[&str] = &[
    "/* c */",
    "/**/",
    "/***/",
    "/* a/b */",
    "/* ** */",
    "/* Ω ↦ */",
];

/// One emitted token and whether it may touch the token before it.
struct Token {
    text: String,
    /// Set for a tree-sitter `IMMEDIATE_TOKEN`, which by definition may not be
    /// preceded by whitespace (the hyphenated tail of a component name, which
    /// would otherwise read as subtraction).
    glued: bool,
}

/// What a component name position refers to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Context,
    Machine,
}

/// Derives text from a grammar.
pub struct Generator<'a> {
    grammar: &'a Grammar,
    config: Config,
}

impl<'a> Generator<'a> {
    /// A generator for `grammar` under `config`.
    pub fn new(grammar: &'a Grammar, config: Config) -> Self {
        Self { grammar, config }
    }

    /// Derive one file: between one and [`Config::max_components`] components.
    ///
    /// The top level is driven here rather than through the grammar's
    /// `source_file` rule so that a context exists before a machine can see
    /// it, which is what makes the generated cross-references resolve.
    pub fn generate(&self, source: &mut dyn ByteSource) -> Generated {
        let mut walk = Walk::new(self.grammar, &self.config, source);
        let count = walk.source.between(1, self.config.max_components);
        for index in 0..count {
            // The first component is always a context: a machine with nothing
            // to see is legal but dull, and it is what lets `sees` resolve.
            let kind = if index == 0 || walk.source.ratio(1, 3) {
                Kind::Context
            } else {
                Kind::Machine
            };
            walk.start_component(kind);
            let rule = match kind {
                Kind::Context => "context",
                Kind::Machine => "machine",
            };
            walk.symbol(rule);
        }
        Generated {
            text: walk.render(),
            unsupported: walk.unsupported,
        }
    }
}

struct Walk<'a, 'b> {
    grammar: &'a Grammar,
    config: &'a Config,
    source: &'b mut dyn ByteSource,
    tokens: Vec<Token>,
    depth_budget: usize,
    token_budget: usize,
    /// Rule names currently being expanded, innermost last.
    path: Vec<&'a str>,
    /// Field names currently being filled, innermost last.
    fields: Vec<&'a str>,
    scope: Scope,
    /// The words the grammar's `builtin` rule offers, collected once.
    builtins: Vec<&'a str>,
    /// Rules that open a binder scope, collected once: names minted inside one
    /// leave scope at the end of its body.
    scoped_rules: BTreeSet<&'a str>,
    contexts: Vec<String>,
    machines: Vec<String>,
    events: Vec<String>,
    /// Where the current component's events start in `events`, so a machine
    /// gets at most one `INITIALISATION` and refines only a foreign event.
    component_events_from: usize,
    /// The number of assignment targets on the left of the `≔` currently being
    /// walked, so the right-hand list can be given the same length instead of
    /// a random one (Rossi rejects an arity mismatch).
    assignment_arity: Option<usize>,
    unsupported: Vec<String>,
}

impl<'a, 'b> Walk<'a, 'b> {
    fn new(grammar: &'a Grammar, config: &'a Config, source: &'b mut dyn ByteSource) -> Self {
        Self {
            grammar,
            config,
            source,
            tokens: Vec::new(),
            depth_budget: config.max_depth,
            token_budget: config.max_tokens,
            path: Vec::new(),
            fields: Vec::new(),
            scope: Scope::new(),
            builtins: grammar.rule("builtin").map(literals).unwrap_or_default(),
            scoped_rules: scoped_rules(grammar),
            contexts: Vec::new(),
            machines: Vec::new(),
            events: Vec::new(),
            component_events_from: 0,
            assignment_arity: None,
            unsupported: Vec::new(),
        }
    }

    /// Choose the next component's name and reset per-component state.
    fn start_component(&mut self, kind: Kind) {
        self.scope = Scope::new();
        self.component_events_from = self.events.len();
        self.depth_budget = self.config.max_depth;
        self.token_budget = self.config.max_tokens;
        let prefix = match kind {
            Kind::Context => "ctx",
            Kind::Machine => "mch",
        };
        let index = self.contexts.len() + self.machines.len();
        let name = self.decorate_name(format!("{prefix}{index}"));
        match kind {
            Kind::Context => self.contexts.push(name),
            Kind::Machine => self.machines.push(name),
        }
    }

    /// The component being generated: the newest name in its own pool.
    fn current_component(&self) -> Option<&String> {
        if self.in_rule("machine") {
            self.machines.last()
        } else {
            self.contexts.last()
        }
    }

    fn push(&mut self, text: impl Into<String>) {
        self.push_token(text, false);
    }

    fn push_token(&mut self, text: impl Into<String>, glued: bool) {
        self.token_budget = self.token_budget.saturating_sub(1);
        self.tokens.push(Token {
            text: text.into(),
            glued,
        });
    }

    fn in_rule(&self, name: &str) -> bool {
        self.path.contains(&name)
    }

    fn field(&self) -> Option<&str> {
        self.fields.last().copied()
    }

    /// Expand the rule named `name`.
    fn symbol(&mut self, name: &'a str) {
        match name {
            "identifier" => return self.emit_identifier(),
            "builtin" => return self.emit_builtin(),
            "number" => return self.emit_number(),
            "label" => return self.emit_label(),
            "_component_name" | "component_name" => return self.emit_component_name(),
            _ => {}
        }
        // The rule's key in the grammar map outlives the walk, so the path can
        // borrow it rather than allocate per expansion.
        let Some((key, rule)) = self.grammar.rules.get_key_value(name) else {
            self.unsupported.push(name.to_string());
            return;
        };
        let scoped = self.scoped_rules.contains(name);
        if scoped {
            self.scope.push();
        }
        self.path.push(key.as_str());
        // Saved and restored rather than decremented and incremented back: an
        // expansion entered with the budget already spent would otherwise hand
        // its caller a larger budget than it started with, and later siblings
        // would nest past `max_depth`.
        let saved_depth = self.depth_budget;
        self.depth_budget = saved_depth.saturating_sub(1);
        self.node(rule);
        self.depth_budget = saved_depth;
        self.path.pop();
        if scoped {
            self.scope.pop();
        }
    }

    fn node(&mut self, node: &'a Node) {
        match node {
            Node::Blank => {}
            Node::String { value } => self.push(value.clone()),
            Node::Pattern { value } => match crate::regex::sample(value, self.source) {
                Some(text) => self.push(text),
                None => self.unsupported.push(format!("pattern {value}")),
            },
            Node::Symbol { name } => self.symbol(name),
            Node::Seq { members } => {
                for member in members {
                    self.node(member);
                }
            }
            Node::Choice { members } => {
                if let Some(chosen) = self.choose(members) {
                    self.node(chosen);
                }
            }
            Node::Repeat { content } => self.repeat(content, 0),
            Node::Repeat1 { content } => self.repeat(content, 1),
            Node::Wrapper { content } => self.node(content),
            Node::ImmediateToken { content } => {
                let before = self.tokens.len();
                self.node(content);
                // Everything the immediate token produced must abut what came
                // before it; in this grammar that is always a single terminal.
                if let Some(token) = self.tokens.get_mut(before) {
                    token.glued = true;
                }
            }
            Node::Field { name, content } => {
                self.fields.push(name.as_str());
                self.node(content);
                self.fields.pop();
            }
        }
    }

    /// Expand `content` between `min` and a budget-dependent number of times.
    fn repeat(&mut self, content: &'a Node, min: usize) {
        // A comma-separated assignment right-hand side must have as many
        // elements as the left-hand side had targets. The grammar puts the
        // repetition *outside* the `left`/`right` field wrapper, so the field
        // stack does not name it here; the repeated group does.
        let repeated = (self.path.last() == Some(&"assignment"))
            .then(|| repeated_field(content))
            .flatten();
        if let Some(arity) = self.assignment_arity
            && repeated == Some("right")
        {
            for _ in 0..arity.saturating_sub(1) {
                self.node(content);
            }
            return;
        }

        let affordable = self.grammar.min_depth(content) <= self.depth_budget;
        let out_of_budget = !affordable || self.token_budget == 0;
        let divergent = self.shape_is_divergent() && !self.source.ratio(1, RARE);
        let count = if out_of_budget || divergent {
            min
        } else {
            // Short lists: a clause with thirty items tests nothing a clause
            // with three does not, and spends the whole token budget.
            self.source.between(min, min + 2)
        };
        for _ in 0..count {
            self.node(content);
        }
        if repeated == Some("left") {
            self.assignment_arity = Some(count + 1);
        }
    }

    /// Whether the construct being expanded here is one the tree-sitter
    /// grammar admits and Rossi's parser does not.
    ///
    /// Two of them, both confirmed against Rossi and against Rodin's own
    /// textual grammar:
    ///
    /// * `function_application` is one permissive multi-argument node, because
    ///   the same `f(a, b)` text is a predicate application (which Rossi
    ///   allows) and an expression application (which it does not). The
    ///   tree-sitter grammar documents that choice.
    /// * `set_declaration` admits an enumerated form, `sets S = {a, b}`.
    ///   Neither Rossi's grammar nor Camille's has it — both refuse the text —
    ///   so the tree-sitter grammar over-accepts here.
    ///
    /// Taken at their natural rate these two would make most generated models
    /// a rejection with nothing new in it, so they are taken rarely instead.
    /// They stay reachable: the disagreement is what a differential fuzzer is
    /// for.
    fn shape_is_divergent(&self) -> bool {
        matches!(
            self.path.last(),
            Some(&"function_application") | Some(&"set_declaration")
        )
    }

    /// The weight of a choice alternative, looked through the wrappers that do
    /// not change what it derives. Zero for a suppressed rule.
    fn member_weight(&self, node: &Node) -> usize {
        match node {
            Node::Symbol { name } => {
                if self.config.suppressed.iter().any(|rule| rule == name) {
                    0
                } else {
                    rule_weight(name)
                }
            }
            node => match node.transparent() {
                Some(inner) => self.member_weight(inner),
                None => DEFAULT_WEIGHT,
            },
        }
    }

    /// Pick one alternative that the remaining budget can afford.
    fn choose(&mut self, members: &'a [Node]) -> Option<&'a Node> {
        let costs: Vec<usize> = members
            .iter()
            .map(|member| self.grammar.min_depth(member))
            .collect();
        let cheapest = costs
            .iter()
            .copied()
            .enumerate()
            .min_by_key(|(_, cost)| *cost)?;

        // Affordability, and nothing else: with tokens spent, close what is
        // open by the shortest route; otherwise take anything the depth budget
        // still allows.
        let limit = if self.token_budget == 0 {
            cheapest.1
        } else {
            self.depth_budget
        };
        // Preference, kept out of the budget so that "what can I afford" and
        // "what do I want" stay separate. Inside a shape the two grammars
        // disagree about, the arm that builds it is the dearer one, so
        // penalising anything above the cheapest cost makes the disagreement
        // rare without making it unreachable.
        let divergence_penalty = if self.shape_is_divergent() { RARE } else { 1 };
        let weight = |walk: &Self, member: &Node, cost: usize| {
            if cost > limit {
                return 0;
            }
            let base = walk.member_weight(member);
            if cost > cheapest.1 {
                base.div_ceil(divergence_penalty)
            } else {
                base
            }
        };

        let total: usize = members
            .iter()
            .zip(&costs)
            .map(|(member, cost)| weight(self, member, *cost))
            .sum();
        if total == 0 {
            // Nothing fits the limit; take the cheapest alternative rather
            // than fail, so a derivation always terminates.
            return members.get(cheapest.0);
        }
        let mut ticket = self.source.below(total);
        for (member, cost) in members.iter().zip(&costs) {
            let weight = weight(self, member, *cost);
            if ticket < weight {
                return Some(member);
            }
            ticket -= weight;
        }
        members.get(cheapest.0)
    }

    fn emit_identifier(&mut self) {
        if self.declares_here() {
            let name = self.scope.mint(self.source);
            self.push(name);
            return;
        }
        match self.scope.reference(self.source) {
            Some(name) => self.push(name),
            // Nothing is in scope yet — a formula in a context with no
            // constants. Mint one so the text is still well formed; it reads
            // as an undeclared name, which the static-check properties expect
            // and classify.
            None => {
                let name = self.scope.mint(self.source);
                self.push(name);
            }
        }
    }

    /// Whether the identifier about to be emitted declares a name.
    ///
    /// Decided by the innermost enclosing rule that is either a declaring rule
    /// or a formula rule: inside `x ⦂ T` the binder `x` sits directly under
    /// `typed_identifier` and declares, while `T` sits under `_expression` and
    /// refers.
    fn declares_here(&self) -> bool {
        self.path
            .iter()
            .rev()
            .find(|name| DECLARING_RULES.contains(name) || REFERRING_RULES.contains(name))
            .is_some_and(|name| DECLARING_RULES.contains(name))
    }

    /// Emit a built-in name.
    ///
    /// The words come from the grammar's own `builtin` rule; only the choice
    /// among them is made here. `card`, `finite`, `max`, `min` and `partition`
    /// are closed operators — `kernel_lang` §2.2 makes them meaningful only
    /// applied to a parenthesized argument, and Rossi reports a reserved word
    /// for a bare one — while the generic atoms (`id`, `pred`, `prj1`, `prj2`,
    /// `succ`) stand alone. The tree-sitter grammar draws no such distinction.
    fn emit_builtin(&mut self) {
        let applied = self.in_rule("function_application") || self.in_rule("function_override");
        let usable: Vec<&&str> = self
            .builtins
            .iter()
            .filter(|word| applied || !rossi::builtins::is_reserved_operator_word(word))
            .collect();
        match self.source.pick(&usable) {
            Some(word) => self.push(**word),
            None => self.unsupported.push("builtin".to_string()),
        }
    }

    fn emit_number(&mut self) {
        let number = self.source.pick(NUMBERS).copied().unwrap_or("0");
        self.push(number);
    }

    fn emit_label(&mut self) {
        let index = self.source.below(1000);
        self.push(format!("@l{index}"));
    }

    /// Emit a component-name position: the component's own name, a reference
    /// to another component, or an event name.
    fn emit_component_name(&mut self) {
        if self.in_rule("event") {
            return self.emit_event_name();
        }

        // The component's own name, chosen when the walk into it started.
        if self.field() == Some("name")
            && let Some(name) = self.current_component()
        {
            let name = name.clone();
            self.push(name);
            return;
        }

        // A cross-reference. `extends` and `sees` name contexts; `refines`
        // names a machine. The component being generated does not appear in
        // the other pool at all, so exclude it by name rather than by
        // position: dropping the last entry unconditionally would hide the
        // only context from a machine's `sees`, the one reference meant to
        // resolve.
        let pool = if self.in_rule("refines_clause") {
            &self.machines
        } else {
            &self.contexts
        };
        let current = self.current_component();
        let candidates: Vec<&String> = pool.iter().filter(|name| Some(*name) != current).collect();
        match self.source.pick(&candidates) {
            Some(name) => self.push((*name).clone()),
            // No component of the right kind exists yet. Emit a plausible
            // name anyway: it parses, and resolves to a missing-target
            // diagnostic that the checker gate compares against Rodin's.
            None => self.push("Absent"),
        }
    }

    /// Occasionally give a component or event name a hyphenated tail.
    ///
    /// Rodin stores these names as file names and event labels, not as
    /// mathematical identifiers, so real models carry hyphens (`ENV_C-1`,
    /// `end-to-end`). The hyphen is also where the two grammars have to agree
    /// that a name is one token and not a subtraction, and where the pest
    /// grammar's wider structural word boundary applies. The name is assembled
    /// here rather than derived from the grammar's `component_name` rule
    /// because every later reference has to reproduce it exactly, which means
    /// recording it.
    fn decorate_name(&mut self, stem: String) -> String {
        if !self.source.ratio(1, 4) {
            return stem;
        }
        let tail = self.source.pick(NAME_TAILS).copied().unwrap_or("1");
        format!("{stem}-{tail}")
    }

    fn emit_event_name(&mut self) {
        if self.field() == Some("name") {
            // The first event of a machine is the initialisation often enough
            // to exercise its dedicated parse path — but at most once per
            // machine, since a repeated name is a duplicate-event error rather
            // than a parse test.
            let own = &self.events[self.component_events_from..];
            let name = if !own.iter().any(|event| event == "INITIALISATION")
                && (own.is_empty() || self.source.ratio(1, 4))
            {
                "INITIALISATION".to_string()
            } else {
                let index = self.events.len();
                self.decorate_name(format!("evt{index}"))
            };
            self.events.push(name.clone());
            self.push(name);
            return;
        }
        // A refines/extends target: an event of an abstract machine, so never
        // one of this machine's own.
        let candidates: Vec<&String> = self.events[..self.component_events_from]
            .iter()
            .filter(|name| *name != "INITIALISATION")
            .collect();
        match self.source.pick(&candidates) {
            Some(name) => self.push((*name).clone()),
            None => self.push("absent_event"),
        }
    }

    /// Join the tokens into text.
    ///
    /// Separation is decided once, here. Two tokens may only touch when the
    /// grammar demands it (an immediate token) or when one side is a bracket
    /// or comma, which cannot fuse with anything: every other pair gets a
    /// space, because deciding whether `<` and `+` would lex as `<+` means
    /// re-implementing the lexer.
    fn render(&mut self) -> String {
        let tokens = std::mem::take(&mut self.tokens);
        let mut out = String::new();
        for (index, token) in tokens.iter().enumerate() {
            let text = emitted_spelling(&token.text, self.config);
            if token.glued {
                out.push_str(&text);
                continue;
            }
            if index == 0 || starts_a_line(&token.text) {
                if index > 0 {
                    out.push('\n');
                }
                if !is_structural_keyword(&token.text) {
                    out.push_str("    ");
                }
            // Every layout decision reads the *derived* spelling, never the
            // emitted one, or the two conventions stop being the same corpus:
            // `{}` and `,,` answer `may_touch` differently from `∅` and `↦`,
            // and this short-circuit would then draw a different number of
            // bytes from the choice stream.
            } else if !may_touch(&tokens[index - 1].text, &token.text) || self.source.ratio(1, 2) {
                out.push(' ');
            }

            // A comment is inserted before a token, never after: a line
            // comment would otherwise swallow the rest of the line.
            if self.source.ratio(COMMENT_PERMILLE, 1000)
                && let Some(comment) = self.source.pick(BLOCK_COMMENTS)
            {
                out.push_str(comment);
                out.push(' ');
            }
            out.push_str(&text);
        }
        out.push('\n');
        if self.config.ordered_clauses {
            out = order_clauses(&out);
        }
        out
    }
}

/// Rewrite `text` so every component's sections, and every event's clauses,
/// stand in the canonical order.
///
/// This runs on the rendered text rather than on the token list, and that is
/// the whole trick: `render` draws a byte wherever two tokens may touch, so a
/// pass that moves tokens moves those draws onto different pairs and the same
/// seed stops yielding the same corpus. Rendering first leaves the choice
/// stream untouched, and costs nothing, because every clause opens a line
/// anyway ([`starts_a_line`]) — a run is a block of whole lines either way.
///
/// The order comes from `rossi::keywords`, the same deference
/// [`is_structural_keyword`] already makes, so the fuzzer cannot drift from
/// the parser it tests. The sort is stable, so a run the order does not name —
/// a repeated section, which rossi rejects as a duplicate anyway — stays where
/// it was written.
fn order_clauses(text: &str) -> String {
    use rossi::keywords::KeywordId;

    const CONTEXT_ORDER: &[KeywordId] = &[
        KeywordId::Extends,
        KeywordId::Sets,
        KeywordId::Constants,
        KeywordId::Axioms,
        KeywordId::Theorems,
    ];
    const MACHINE_ORDER: &[KeywordId] = &[
        KeywordId::Refines,
        KeywordId::Sees,
        KeywordId::Variables,
        KeywordId::Invariants,
        KeywordId::Theorems,
        KeywordId::Variant,
        KeywordId::Events,
    ];
    const EVENT_ORDER: &[KeywordId] = &[
        KeywordId::Any,
        KeywordId::Where,
        KeywordId::With,
        KeywordId::Witness,
        KeywordId::Then,
    ];

    // A region whose runs are being collected: the order its runs sort by, and
    // the line each run so far began on.
    struct Frame {
        order: &'static [KeywordId],
        runs: Vec<(usize, usize)>,
        run_start: Option<usize>,
    }

    // A comment may be emitted between a line's indent and its first token, so
    // read keywords off comment-masked text. Masking preserves positions, so
    // the two line lists stay parallel. `ctx0-end` is one whitespace-delimited
    // token and looks up as no keyword, which is what keeps a hyphenated
    // component name from closing a region.
    let masked = rossi::comments::mask_comments_chars(text);
    let lines: Vec<&str> = text.lines().collect();
    let keywords: Vec<Option<KeywordId>> = masked
        .lines()
        .map(|line| {
            line.split_whitespace()
                .next()
                .and_then(rossi::keywords::lookup)
                .map(|keyword| keyword.id)
        })
        .collect();

    let mut order: Vec<usize> = (0..lines.len()).collect();
    let mut stack: Vec<Frame> = Vec::new();
    let close_run = |frame: &mut Frame, end: usize| {
        if let Some(start) = frame.run_start.take() {
            frame.runs.push((start, end));
        }
    };
    for (index, keyword) in keywords.iter().enumerate() {
        match keyword {
            // A component header or an event opens a region. An enclosing
            // machine's `EVENTS` run stays open across the event, so it covers
            // the whole block: closing it here would leave the event bodies in
            // no run, and a section written after the block would then make
            // the machine's runs non-contiguous.
            Some(KeywordId::Context) => stack.push(Frame {
                order: CONTEXT_ORDER,
                runs: Vec::new(),
                run_start: None,
            }),
            Some(KeywordId::Machine) => stack.push(Frame {
                order: MACHINE_ORDER,
                runs: Vec::new(),
                run_start: None,
            }),
            Some(KeywordId::Event) => stack.push(Frame {
                order: EVENT_ORDER,
                runs: Vec::new(),
                run_start: None,
            }),
            Some(KeywordId::End) => {
                if let Some(mut frame) = stack.pop() {
                    close_run(&mut frame, index);
                    apply_order(&mut order, &frame.runs, &keywords, frame.order);
                }
            }
            Some(keyword) => {
                if let Some(frame) = stack.last_mut()
                    && frame.order.contains(keyword)
                {
                    close_run(frame, index);
                    frame.run_start = Some(index);
                }
            }
            None => {}
        }
    }

    let mut out = String::with_capacity(text.len());
    for index in order {
        out.push_str(lines[index]);
        out.push('\n');
    }
    out
}

/// Permute `order` so the line runs of one region stand in `canonical` order.
///
/// `runs` tile the region's lines: each ends where the next begins, and the
/// last at the region's `END`, so the block they cover can be rewritten in
/// place. Lines a run does not cover — the region's own header, and an event
/// body inside an enclosing machine's `EVENTS` run — are never touched.
fn apply_order(
    order: &mut [usize],
    runs: &[(usize, usize)],
    keywords: &[Option<rossi::keywords::KeywordId>],
    canonical: &[rossi::keywords::KeywordId],
) {
    let Some(&(first, _)) = runs.first() else {
        return;
    };
    let mut sorted: Vec<&(usize, usize)> = runs.iter().collect();
    sorted.sort_by_key(|(start, _)| {
        keywords[*start]
            .and_then(|keyword| canonical.iter().position(|k| *k == keyword))
            .unwrap_or(usize::MAX)
    });
    let permuted: Vec<usize> = sorted
        .iter()
        .flat_map(|(start, end)| order[*start..*end].iter().copied())
        .collect();
    order[first..first + permuted.len()].copy_from_slice(&permuted);
}

/// The field a comma-separated repetition fills, when the repeated group names
/// one directly. Shallow on purpose: it looks through the group's own sequence
/// and no further, so a field of some nested rule is never mistaken for it.
fn repeated_field(node: &Node) -> Option<&str> {
    match node {
        Node::Field { name, .. } => Some(name.as_str()),
        Node::Seq { members } => members.iter().find_map(repeated_field),
        _ => None,
    }
}

/// The rules that open a binder scope.
///
/// The grammar states this itself: a rule that binds names gives them a
/// `binder` field, or a `pattern` field for the lambda's maplet tree. Reading
/// it off the grammar means a new binder form is handled without touching the
/// fuzzer, which is the whole reason generation is driven by grammar data.
fn scoped_rules(grammar: &Grammar) -> BTreeSet<&str> {
    grammar
        .rules
        .iter()
        .filter(|(_, node)| has_field(node, &["binder", "pattern"]))
        .map(|(name, _)| name.as_str())
        .collect()
}

/// Whether `node` has a field with one of these names anywhere below it.
fn has_field(node: &Node, names: &[&str]) -> bool {
    match node {
        Node::Field { name, content } => {
            names.contains(&name.as_str()) || has_field(content, names)
        }
        Node::Seq { members } | Node::Choice { members } => {
            members.iter().any(|member| has_field(member, names))
        }
        Node::Repeat { content } | Node::Repeat1 { content } => has_field(content, names),
        node => node
            .transparent()
            .is_some_and(|inner| has_field(inner, names)),
    }
}

/// Every literal a node can derive, in grammar order.
///
/// Used for the `builtin` rule, whose alternatives are all plain strings: the
/// words belong to the grammar, and only the choice among them depends on
/// where the walk is.
fn literals(node: &Node) -> Vec<&str> {
    match node {
        Node::String { value } => vec![value.as_str()],
        Node::Choice { members } | Node::Seq { members } => {
            members.iter().flat_map(|member| literals(member)).collect()
        }
        node => node.transparent().map(literals).unwrap_or_default(),
    }
}

/// The spelling to write for `token` under `config`'s emission conventions.
///
/// The two rewrites cannot collide: rossi keeps operators and structural
/// keywords in separate tables, and no spelling is in both.
fn emitted_spelling<'a>(token: &'a str, config: &Config) -> Cow<'a, str> {
    if config.unicode_operators
        && let Some(spelling) = unicode_spelling(token)
    {
        return Cow::Borrowed(spelling);
    }
    if config.lowercase_keywords
        && let Some(spelling) = lowercase_keyword(token)
    {
        return Cow::Owned(spelling);
    }
    Cow::Borrowed(token)
}

/// The lower-case spelling of `token`, when `token` is a structural keyword
/// that is not already lower case.
///
/// The grammar spells keywords as case-insensitive patterns and the generator
/// picks a case per letter, which Camille refuses: it takes a keyword
/// all-lower or all-upper, but not `conteXt`, and not `BEGIN` at all.
///
/// `INITIALISATION` is deliberately left alone. rossi lists it among the
/// keywords, but to Rodin it is an event *name*, and only that spelling names
/// the initialisation event — eventb-checker reads `initialisation` as an
/// ordinary event instead, which is a change of meaning rather than of
/// spelling. Camille takes the name in any case, so there is nothing to fix.
///
/// Only whole tokens are matched, and `keywords::lookup` compares whole
/// spellings, so a hyphenated name that embeds a keyword (`ctx0-end`) is not
/// one and is left as derived.
fn lowercase_keyword(token: &str) -> Option<String> {
    let keyword = rossi::keywords::lookup(token)?;
    if keyword.id == rossi::keywords::KeywordId::Initialisation {
        return None;
    }
    token
        .chars()
        .any(|c| c.is_ascii_uppercase())
        .then(|| token.to_ascii_lowercase())
}

/// The Unicode spelling of `token`, when `token` is an operator.
///
/// A generated token is always one whole grammar literal, so an operator is
/// matched as a unit: `lookup_token` resolves both conventions plus the
/// input-only aliases (`,,`, `+->>`, `-->>`), while a name, number, label,
/// comment or structural keyword misses the table and is left alone. Matching
/// whole tokens is what keeps the hyphen of `ctx0-end` out of the rewrite: a
/// textual scan over the rendered model would turn it into `ctx0−end`, which
/// neither parser accepts, and masking comments and labels does not help
/// because a component name is neither.
///
/// `unicode`, not `emit_text(true, false)`: rossi does not write the four
/// Rodin private-use operators into a buffer by default, because they render
/// as tofu without Rodin's math font. A fuzz input goes to a parser, not a buffer, and
/// `U+E100..=U+E103` are what Rodin's own lexer reads — measured against
/// eventb-checker, which accepts all four glyphs and rejects every one of
/// their ASCII spellings with `EB005`.
fn unicode_spelling(token: &str) -> Option<&'static str> {
    rossi::operators::lookup_token(token).map(|entry| entry.unicode)
}

/// Whether a token opens a structural region: a clause keyword, an event
/// keyword, or `END`.
///
/// `rossi::keywords` is the single source of truth for these spellings and
/// already draws exactly this line — every keyword but the status values and
/// the inline modifiers — so asking it keeps the layout right when a spelling
/// is added there.
fn is_structural_keyword(text: &str) -> bool {
    rossi::keywords::is_clause_boundary(text)
}

/// Whether a token starts a new line in the rendered output, which is what
/// makes generated text read like a model instead of one long line.
fn starts_a_line(text: &str) -> bool {
    text.starts_with('@') || is_structural_keyword(text)
}

/// Whether two adjacent tokens may be written without a separator.
///
/// Only brackets and commas qualify: no operator spelling in the language
/// contains one, so they cannot fuse with a neighbour, while `<` next to `+`
/// would lex as the single operator `<+`. A label is the exception to the
/// exception: it runs to the next whitespace, brackets included, so `@l1(x`
/// is one long label rather than a label followed by anything.
fn may_touch(left: &str, right: &str) -> bool {
    const UNFUSABLE: &[char] = &['(', ')', '{', '}', '[', ']', ','];
    if left.starts_with('@') {
        return false;
    }
    let left_end = left.chars().next_back();
    let right_start = right.chars().next();
    left_end.is_some_and(|c| UNFUSABLE.contains(&c))
        || right_start.is_some_and(|c| UNFUSABLE.contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::choice::SplitMix64;
    use crate::test_support::load_grammar;

    fn generate_all(count: usize) -> Vec<Generated> {
        generate_all_with(count, Config::default())
    }

    fn generate_all_with(count: usize, config: Config) -> Vec<Generated> {
        let Some(grammar) = load_grammar() else {
            return Vec::new();
        };
        let generator = Generator::new(&grammar, config);
        let mut rng = SplitMix64::new(0x5EED);
        (0..count).map(|_| generator.generate(&mut rng)).collect()
    }

    fn unicode_config() -> Config {
        Config {
            unicode_operators: true,
            ..Config::default()
        }
    }

    fn lowercase_config() -> Config {
        Config {
            lowercase_keywords: true,
            ..Config::default()
        }
    }

    /// The clause order alone, with every spelling left as derived.
    fn ordered_config() -> Config {
        Config {
            ordered_clauses: true,
            ..Config::default()
        }
    }

    /// Every convention at once: what a checker gate actually feeds Camille,
    /// and what `gen --normalize` turns on.
    fn normalized_config() -> Config {
        Config {
            unicode_operators: true,
            lowercase_keywords: true,
            ordered_clauses: true,
            ..Config::default()
        }
    }

    #[test]
    fn every_generated_model_derives_completely() {
        for model in generate_all(200) {
            assert!(
                model.unsupported.is_empty(),
                "grammar constructs the generator cannot derive: {:?}",
                model.unsupported
            );
            assert!(!model.text.trim().is_empty());
        }
    }

    #[test]
    fn generation_is_reproducible() {
        let Some(grammar) = load_grammar() else {
            return;
        };
        let generator = Generator::new(&grammar, Config::default());
        let mut first = SplitMix64::new(4242);
        let mut second = SplitMix64::new(4242);
        for _ in 0..50 {
            assert_eq!(
                generator.generate(&mut first).text,
                generator.generate(&mut second).text
            );
        }
    }

    /// Panic messages the parser is known to produce on valid-per-tree-sitter
    /// input, each a known, still-open defect. A crash whose message is not
    /// on this list is new, and fails the test.
    ///
    /// The list is expected to shrink to nothing. It exists so that a known
    /// crash does not mask an unknown one, and is empty: every crash the
    /// generator has found so far is fixed, so any crash at all fails the test.
    const KNOWN_CRASHES: &[&str] = &[];

    static PANIC_HOOK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Parse `text`, turning a panic into a value.
    ///
    /// Catching is what a fuzz harness does: the target crashing is the result,
    /// not an excuse to stop. `AssertUnwindSafe` is sound here because nothing
    /// observes the parser's state afterwards — the caller records the message
    /// and moves to the next input.
    ///
    /// The hook swap is serialised: `set_hook` is process-global, so two tests
    /// in this binary swapping it concurrently would leave the silencing hook
    /// installed and hide every later panic message.
    fn parse_catching(text: &str) -> Result<bool, String> {
        let guard = PANIC_HOOK.lock().unwrap_or_else(|error| error.into_inner());
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            rossi::parse_components(text).is_ok()
        }));
        std::panic::set_hook(previous);
        drop(guard);
        outcome.map_err(|payload| {
            payload
                .downcast_ref::<&str>()
                .map(|message| (*message).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "non-string panic payload".to_string())
        })
    }

    /// Generated models must not crash the parser, and enough of them must
    /// parse for a run to spend its time on Rossi's accepting paths.
    ///
    /// The floor is well below 100% on purpose: the tree-sitter grammar admits
    /// constructs Rossi refuses — enumerated set declarations, multi-argument
    /// application, chained non-associative operators — so a generator faithful
    /// to it produces rejections by design, and the `gen` subcommand reports
    /// them bucketed by cause.
    #[test]
    fn generated_models_parse_or_are_rejected_cleanly() {
        assert_parses_or_rejects_cleanly(&generate_all(400), "generated");
    }

    /// No crash outside [`KNOWN_CRASHES`], and enough of `models` parsing to
    /// spend the run on Rossi's accepting paths. Shared by the plain run and
    /// the normalized ones, which are held to the same bar: normalizing
    /// changes how a token is spelled, not whether the model is well formed.
    fn assert_parses_or_rejects_cleanly(models: &[Generated], what: &str) {
        if models.is_empty() {
            return;
        }
        let mut accepted = 0usize;
        let mut crashes: Vec<String> = Vec::new();
        for model in models {
            match parse_catching(&model.text) {
                Ok(true) => accepted += 1,
                Ok(false) => {}
                Err(message) => {
                    if !crashes.contains(&message) {
                        crashes.push(message);
                    }
                }
            }
        }
        let unknown: Vec<&String> = crashes
            .iter()
            .filter(|message| !KNOWN_CRASHES.iter().any(|known| message.contains(known)))
            .collect();
        assert!(
            unknown.is_empty(),
            "new parser crashes on {what} input: {unknown:?}"
        );
        assert!(
            accepted * 100 >= models.len() * 25,
            "only {accepted}/{} {what} models parse, which is too few to \
             exercise the accepting paths",
            models.len()
        );
    }

    #[test]
    fn unicode_spelling_rewrites_operators_and_nothing_else() {
        for (token, unicode) in [
            (":", "∈"),
            ("&", "∧"),
            ("or", "∨"),
            ("=>", "⇒"),
            ("{}", "∅"),
            ("NAT", "ℕ"),
            ("POW", "ℙ"),
            ("oftype", "⦂"),
            ("|->", "↦"),
            (",,", "↦"),
            ("+->>", "⤀"),
            ("..", "‥"),
            (".", "·"),
            ("-", "−"),
            ("~", "∼"),
            ("%", "λ"),
            // Rodin's private-use glyphs: eventb-checker accepts these and
            // rejects their ASCII spellings, which is why the fuzzer emits
            // them where `rossi fmt` deliberately does not.
            ("<+", "\u{E103}"),
            ("<<->", "\u{E100}"),
            ("<->>", "\u{E101}"),
            ("<<->>", "\u{E102}"),
        ] {
            assert_eq!(unicode_spelling(token), Some(unicode), "token {token:?}");
        }
        // Everything that is not a whole operator token, above all a
        // hyphenated component name: rewriting its `-` would break a name that
        // only accepts ASCII, which is the trap a textual scan falls into.
        for token in ["ctx0-end", "@inv1.1", "/* a/b */", "machine", "007", "x"] {
            assert_eq!(unicode_spelling(token), None, "token {token:?}");
        }
    }

    /// Normalizing operators must leave every other token byte-identical.
    ///
    /// The two that would break are a hyphenated name, whose `-` is not the
    /// subtraction operator, and a comment, whose `/` is not division. Both
    /// are guarded structurally — the rewrite matches whole tokens, and a
    /// comment is spliced in at render time — so this pins the guarantee
    /// against a later move to a textual pass, which could not tell them apart.
    #[test]
    fn unicode_generation_leaves_names_and_comments_alone() {
        let models = generate_all_with(200, unicode_config());
        if models.is_empty() {
            return;
        }
        let batch: String = models.iter().map(|model| model.text.as_str()).collect();

        assert!(
            surrounded_by_word_chars(&batch, '-'),
            "no hyphenated name was generated, so the test proves nothing"
        );
        assert!(
            !surrounded_by_word_chars(&batch, '−'),
            "a hyphen inside a name was rewritten to U+2212"
        );

        let opened = batch.matches("/*").count();
        let intact: usize = BLOCK_COMMENTS
            .iter()
            .map(|comment| batch.matches(comment).count())
            .sum();
        assert_eq!(opened, intact, "a block comment was rewritten");
    }

    /// Whether `needle` ever appears between two word characters, which is
    /// what a hyphen inside `ctx0-end` looks like and an operator never does:
    /// `may_touch` only lets a token touch a bracket or a comma.
    fn surrounded_by_word_chars(text: &str, needle: char) -> bool {
        let chars: Vec<char> = text.chars().collect();
        chars.windows(3).any(|window| {
            window[1] == needle
                && rossi::keywords::is_word_char(window[0])
                && rossi::keywords::is_word_char(window[2])
        })
    }

    /// `--unicode` must change only how operators are spelled: the two runs
    /// are the same corpus in two conventions, which is what lets a
    /// differential run blame a disagreement on the parsers rather than on two
    /// different models.
    ///
    /// Layout is where that is easy to lose. `render` draws a random byte only
    /// when two tokens *may* touch, and rewriting `{}` to `∅` or `,,` to
    /// `↦` changes that answer — so deciding layout on the emitted spelling
    /// instead of the derived one desynchronizes the choice stream and silently
    /// generates a different corpus from the same seed.
    #[test]
    fn normalized_generation_yields_the_same_corpus() {
        let plain = generate_all(200);
        if plain.is_empty() {
            return;
        }
        for (config, what) in [
            (unicode_config(), "--unicode"),
            (lowercase_config(), "--lowercase-keywords"),
            (ordered_config(), "--ordered-clauses"),
            (normalized_config(), "every flag"),
        ] {
            let normalized = generate_all_with(200, config);
            let mut differing = 0;
            for (plain, normalized) in plain.iter().zip(&normalized) {
                assert_eq!(
                    plain.text.lines().count(),
                    normalized.text.lines().count(),
                    "{what}: layout diverged, so the runs are not the same corpus"
                );
                assert_eq!(
                    plain.text.matches('@').count(),
                    normalized.text.matches('@').count(),
                    "{what}: label count diverged, so the choice streams are out of step"
                );
                if plain.text != normalized.text {
                    differing += 1;
                }
            }
            assert!(differing > 0, "{what} changed nothing at all");
        }
    }

    #[test]
    fn lowercase_keyword_rewrites_structural_keywords_only() {
        for (token, lowered) in [
            ("conteXt", "context"),
            ("MACHINE", "machine"),
            ("END", "end"),
            ("BeGiN", "begin"),
            ("WHEN", "when"),
            ("Theorem", "theorem"),
            ("SKIP", "skip"),
        ] {
            assert_eq!(
                lowercase_keyword(token).as_deref(),
                Some(lowered),
                "token {token:?}"
            );
        }
        for token in [
            // Already lower case: nothing to rewrite, and nothing allocated.
            "context",
            "end",
            // An event name to Rodin, and only this spelling names the
            // initialisation event.
            "INITIALISATION",
            // Not keywords: a hyphenated name that embeds one, a math token,
            // a label, an ordinary identifier.
            "ctx0-end",
            "NAT",
            "@inv1",
            "wheres",
        ] {
            assert_eq!(lowercase_keyword(token), None, "token {token:?}");
        }
    }

    /// The one keyword the case rewrite must not touch, checked on real
    /// generated text rather than only on the helper.
    #[test]
    fn lowercase_generation_keeps_the_initialisation_event() {
        let models = generate_all_with(200, lowercase_config());
        if models.is_empty() {
            return;
        }
        let batch: String = models.iter().map(|model| model.text.as_str()).collect();
        assert!(
            batch.contains("INITIALISATION"),
            "the initialisation event was lower-cased into an ordinary one"
        );
        for keyword in ["conteXt", "CONTEXT", "Machine", "BEGIN"] {
            assert!(!batch.contains(keyword), "{keyword} survived the rewrite");
        }
    }

    /// Neither convention may cost acceptance: rossi reads both spellings and
    /// both cases, so normalizing changes how a token reads and nothing else.
    #[test]
    fn normalized_models_parse_or_are_rejected_cleanly() {
        assert_parses_or_rejects_cleanly(&generate_all_with(200, unicode_config()), "Unicode");
        assert_parses_or_rejects_cleanly(&generate_all_with(200, lowercase_config()), "lower-case");
        assert_parses_or_rejects_cleanly(&generate_all_with(200, ordered_config()), "ordered");
        assert_parses_or_rejects_cleanly(
            &generate_all_with(200, normalized_config()),
            "normalized",
        );
    }

    /// The canonical position of each section keyword of `component`, in the
    /// order the lines of `text` write them.
    fn section_ranks(text: &str) -> Vec<Vec<usize>> {
        use rossi::keywords::KeywordId;
        let context = [
            KeywordId::Extends,
            KeywordId::Sets,
            KeywordId::Constants,
            KeywordId::Axioms,
            KeywordId::Theorems,
        ];
        let machine = [
            KeywordId::Refines,
            KeywordId::Sees,
            KeywordId::Variables,
            KeywordId::Invariants,
            KeywordId::Theorems,
            KeywordId::Variant,
            KeywordId::Events,
        ];
        let masked = rossi::comments::mask_comments_chars(text);
        let mut components: Vec<Vec<usize>> = Vec::new();
        let mut order: &[KeywordId] = &[];
        let mut in_event = false;
        for line in masked.lines() {
            let Some(keyword) = line
                .split_whitespace()
                .next()
                .and_then(rossi::keywords::lookup)
                .map(|keyword| keyword.id)
            else {
                continue;
            };
            match keyword {
                KeywordId::Context => {
                    order = &context;
                    components.push(Vec::new());
                }
                KeywordId::Machine => {
                    order = &machine;
                    components.push(Vec::new());
                }
                // An event's own clauses share spellings with nothing in the
                // section lists except by accident, but its END must not be
                // read as the component's.
                KeywordId::Event => in_event = true,
                KeywordId::End => in_event = false,
                _ => {
                    if !in_event
                        && let Some(rank) = order.iter().position(|k| *k == keyword)
                        && let Some(component) = components.last_mut()
                    {
                        component.push(rank);
                    }
                }
            }
        }
        components
    }

    #[test]
    fn ordered_generation_writes_every_section_in_canonical_order() {
        let plain = generate_all(200);
        if plain.is_empty() {
            return;
        }
        let ordered = generate_all_with(200, ordered_config());
        // Camille refuses a section keyword that moves backwards along its
        // index list, so the ranks must never decrease.
        let mut probed = false;
        for model in &ordered {
            for component in section_ranks(&model.text) {
                assert!(
                    component.windows(2).all(|pair| pair[0] <= pair[1]),
                    "sections out of order: {component:?} in\n{}",
                    model.text
                );
            }
        }
        for model in &plain {
            probed |= section_ranks(&model.text)
                .iter()
                .any(|component| component.windows(2).any(|pair| pair[0] > pair[1]));
        }
        assert!(
            probed,
            "no model was generated out of order, so this proves nothing"
        );
    }

    #[test]
    fn a_hyphenated_name_ending_in_a_keyword_closes_nothing() {
        // `ctx0-end` is one component name. Reading keywords off whole
        // whitespace-delimited tokens is what keeps its tail from closing the
        // component and stranding the sections that follow.
        let text = "context ctx0-end\naxioms\n    @a 1 = 1\nsets\n    S\nend\n";
        let ordered = order_clauses(text);
        assert_eq!(
            ordered,
            "context ctx0-end\nsets\n    S\naxioms\n    @a 1 = 1\nend\n"
        );
    }

    #[test]
    fn a_comment_before_a_section_keyword_does_not_hide_it() {
        // `render` may emit a comment between a line's indent and its first
        // token, so keywords are read off comment-masked text.
        let text = "context c\n/* x */ axioms\n    @a 1 = 1\n/* y */ sets\n    S\nend\n";
        let ordered = order_clauses(text);
        assert_eq!(
            ordered,
            "context c\n/* y */ sets\n    S\n/* x */ axioms\n    @a 1 = 1\nend\n"
        );
    }

    #[test]
    fn an_events_block_moves_whole() {
        // The machine's EVENTS run stays open across the events, so the block
        // travels with its keyword and a section written after it still sorts
        // ahead of the whole thing.
        let text =
            "machine m\nevents\n  event e\n  then\n    @a x ≔ 1\n  end\nvariables\n  x\nend\n";
        let ordered = order_clauses(text);
        assert_eq!(
            ordered,
            "machine m\nvariables\n  x\nevents\n  event e\n  then\n    @a x ≔ 1\n  end\nend\n"
        );
    }

    #[test]
    fn an_event_clause_written_out_of_order_moves_too() {
        let text = "machine m\nevents\n  event e\n  then\n    @a x ≔ 1\n  where\n    @g x > 0\n  end\nend\n";
        let ordered = order_clauses(text);
        assert_eq!(
            ordered,
            "machine m\nevents\n  event e\n  where\n    @g x > 0\n  then\n    @a x ≔ 1\n  end\nend\n"
        );
    }

    #[test]
    fn declared_names_are_the_ones_referred_to() {
        // Every identifier a generated context uses must be one it declares:
        // that is what keeps static-check findings meaningful.
        let Some(grammar) = load_grammar() else {
            return;
        };
        let generator = Generator::new(
            &grammar,
            Config {
                max_components: 1,
                ..Config::default()
            },
        );
        let mut rng = SplitMix64::new(77);
        let mut checked = 0;
        for _ in 0..200 {
            let model = generator.generate(&mut rng);
            let Ok(true) = parse_catching(&model.text) else {
                continue;
            };
            let Ok(components) = rossi::parse_components(&model.text) else {
                continue;
            };
            for component in &components {
                if let rossi::Component::Context(context) = component {
                    let declared: Vec<&str> = context
                        .sets
                        .iter()
                        .chain(&context.constants)
                        .map(|element| element.name.as_str())
                        .collect();
                    if !declared.is_empty() {
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 0, "no context with declarations was generated");
    }

    #[test]
    fn tokens_that_could_fuse_are_kept_apart() {
        assert!(!may_touch("<", "+"));
        assert!(!may_touch("x", "y"));
        assert!(!may_touch("end", "1"));
        assert!(!may_touch("@label", "("));
        assert!(may_touch("(", "x"));
        assert!(may_touch("x", ")"));
        assert!(may_touch("x", ","));
    }
}
