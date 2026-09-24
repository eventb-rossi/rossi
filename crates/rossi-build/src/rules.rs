//! Stable `EBnnn` rule identifiers for validation diagnostics.
//!
//! A diagnostic that carries a [`RuleId`] is one that downstream tools (CI
//! gates, SARIF consumers, IDEs) can reason about by code. Internal
//! catch-all sites (e.g. "failed to check context: {e}") deliberately stay
//! untagged — they expose no stable contract.

use crate::Severity;

/// Validation rule identifiers exposed in `Diagnostic.rule_id`.
///
/// Codes use the stable `EBnnn` scheme: `"EB001"`..`"EB035"` for the
/// validation rules, with one gap for a rule removed as valueless (EB013
/// dead constant — every hit was already an EB006 typing Error), and
/// `"EB100"`.. for the runtime-translation suitability rules of
/// [`crate::runtime`], which `rossi validate --runtime` reports. EB023,
/// EB024, EB028, EB031, EB034 and every EB1xx code are rossi-only
/// extensions; EB025 is a refinement
/// static-check emitted by `crate::build`; EB029, EB030 and EB032 are
/// structural parse errors raised by the Camille grammar
/// (`rossi::ParseError`), not by a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuleId {
    /// EB001 — XML parse error (corrupt Rodin archive, malformed `.buc`/`.bum`).
    XmlParseError,
    /// EB002 — XML root element is neither `contextFile` nor `machineFile`.
    XmlRootError,
    /// EB003 — A required XML attribute is missing from a Rodin element.
    XmlAttributeError,
    /// EB004 — Camille parse error (an `.eventb` file rejected as a whole).
    CamilleParseError,
    /// EB005 — Formula parse error (Camille / pest grammar rejected source text).
    FormulaParseError,
    /// EB006 — Type error (ill-typed predicate or expression; element dropped).
    TypeError,
    /// EB007 — Circular EXTENDS chain among contexts.
    CircularExtends,
    /// EB008 — Circular REFINES chain among machines.
    CircularRefines,
    /// EB009 — Cross-reference target not found (unknown SEES / EXTENDS / REFINES name).
    CrossReferenceNotFound,
    /// EB010 — Formula has a non-trivial well-definedness condition.
    WellDefinedness,
    /// EB011 — Variable never used: no reference outside typing
    /// invariants and no event assigns it.
    DeadVariable,
    /// EB012 — Variable never assigned outside INITIALISATION: a
    /// constant in disguise.
    UnmodifiedVariable,
    /// EB014 — INITIALISATION leaves one or more variables unassigned.
    IncompleteInitialisation,
    /// EB015 — Proof obligation not fully discharged (pending, reviewed, or
    /// unattempted).
    UndischargedProof,
    /// EB016 — Proof script is no longer valid (`psBroken` in `.bps`).
    BrokenProof,
    /// EB017 — A proof file (`.bpr`/`.bpo`/`.bps`) could not be parsed.
    ProofFileParseError,
    /// EB018 — Undeclared identifier in a guard, witness, or action.
    UndeclaredIdentifier,
    /// EB019 — Same component name defined in more than one file.
    DuplicateComponent,
    /// EB020 — An axiom, invariant or guard reads a constant, variable or
    /// parameter before the predicate that types it. Rodin types these
    /// predicates in source order, so the type is still unknown there and
    /// the predicate is dropped.
    UnknownType,
    /// EB021 — An identifier (variable, constant, carrier set, or event
    /// parameter) is declared more than once within the same scope.
    DuplicateIdentifier,
    /// EB022 — A label (invariant, event, guard, action, axiom, or witness)
    /// is used more than once within the same scope.
    DuplicateLabel,
    /// EB023 — Declared name collides with rossi's textual operator
    /// vocabulary and can be silently re-lexed as a token. (rossi-only.)
    ShadowedName,
    /// EB024 — A new event (one that does not REFINE an abstract event)
    /// assigns a variable inherited from an abstract machine. (rossi-only.)
    NewEventAssignsInheritedVariable,
    /// EB025 — An event assigns a variable that an abstract machine declares
    /// but this refinement dropped (data-refined away), so it no longer exists
    /// in the concrete state and cannot be assigned.
    DisappearedVariable,
    /// EB026 — A predicate context (invariant, guard, witness, or axiom) uses an
    /// assignment operator (`:=`/`≔`, `:∈`/`::`, `:|`/`:∣`) where a predicate is
    /// required; the intended operator is almost always `=`.
    AssignmentInPredicate,
    /// EB027 — An event merging several abstract events violates a merge
    /// constraint: the abstract events' actions differ, their action labels
    /// do not coincide, a shared abstract parameter name has conflicting
    /// types, or an extended event declares several targets.
    EventMergeMismatch,
    /// EB028 — Declared name spells a structural keyword (`END`, `SETS`,
    /// `THEN`, …) that rossi or Camille re-lexes where the name is written.
    /// (rossi-only.)
    KeywordName,
    /// EB029 — A clause header (`WHERE`, `INVARIANTS`, `THEN`, …) has nothing
    /// under it, or a label has no formula after it.
    EmptyClause,
    /// EB030 — An event clause is written after one it must precede (Rodin
    /// fixes the order `ANY`, `WHERE`, `WITH`, `WITNESS`, `THEN`).
    ClauseOutOfOrder,
    /// EB031 — A structural position separates two names with a Unicode space
    /// that Rodin's math lexer accepts but stock Camille cannot read.
    /// (rossi-only.)
    NonPortableWhitespace,
    /// EB032 — A predicate or action is written with no `@label`.
    MissingLabel,
    /// EB033 — A declared name carries the after-state prime (`c'`), which
    /// only a witness label may.
    PrimedDeclaredName,
    /// EB034 — A context or machine section is written after one it must
    /// precede (`SETS` below `AXIOMS`, say). rossi accepts it, as Rodin and
    /// CamilleX do; stock Camille does not. (rossi-only.)
    SectionOutOfOrder,
    /// EB035 — A refining event gives no well-typed witness for an abstract
    /// parameter it drops, or for the after-state of a disappearing variable
    /// its abstract event assigns nondeterministically; `⊤` is assumed.
    MissingWitness,

    // Runtime-translation suitability (EB1xx). Reported by
    // `crate::runtime` over the leaf machines and the contexts they see,
    // and only when `rossi validate --runtime` asks for them.
    /// EB100 — An ordinary event of a leaf machine chooses from a set
    /// (`x :∈ S`) that is not a singleton, so the model does not say which
    /// value a run takes. (rossi-only.)
    BecomesMemberOfInEvent,
    /// EB101 — An ordinary event of a leaf machine assigns by predicate
    /// (`x :∣ P`) outside the form a translation can read a value from.
    /// (rossi-only.)
    NonCanonicalBecomesSuchThat,
    /// EB102 — A leaf machine's INITIALISATION does not determine the
    /// initial state, or reads a variable that has none yet. (rossi-only.)
    NondeterministicInitialisation,
    /// EB103 — No guard of a leaf machine's event fixes one of its
    /// parameters to a value. (rossi-only.)
    UndeterminedParameter,
    /// EB104 — A quantifier or comprehension binds a variable with neither a
    /// domain to iterate nor a finite type. (rossi-only.)
    UnboundedQuantifier,
    /// EB105 — An infinite set is used where a value is needed rather than
    /// where a type is given. (rossi-only.)
    InfiniteSetValue,
    /// EB106 — A carrier set used as a value has no axiom giving it a finite
    /// cardinality. (rossi-only.)
    DeferredSetWithoutCardinality,
    /// EB107 — An integer identifier is bounded only by its type, so nothing
    /// says which machine integer can hold it. (rossi-only.)
    UnboundedInteger,
    /// EB108 — A constant has no axiom determining its value. (rossi-only.)
    UndeterminedConstant,
    /// EB109 — Two constants of the same carrier set are never related by
    /// `=` or `≠`, so nothing says whether they differ. (rossi-only.)
    IndistinctCarrierSetConstants,
    /// EB110 — An event's actions only agree with the model when they are
    /// applied simultaneously. (rossi-only.)
    ParallelAssignmentHazard,
    /// EB111 — A guard is well-defined only under a condition no earlier
    /// guard establishes. (rossi-only.)
    GuardEvaluationOrder,
    /// EB112 — A leaf machine drops a variable its abstraction declares.
    /// (rossi-only.)
    DroppedAbstractVariable,
    /// EB113 — A witness constrains the abstract value without determining
    /// it. (rossi-only.)
    NonEqualityWitness,
}

impl RuleId {
    /// Stable string code (`"EB001"`..`"EB035"`, `"EB100"`..`"EB113"`).
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            RuleId::XmlParseError => "EB001",
            RuleId::XmlRootError => "EB002",
            RuleId::XmlAttributeError => "EB003",
            RuleId::CamilleParseError => "EB004",
            RuleId::FormulaParseError => "EB005",
            RuleId::TypeError => "EB006",
            RuleId::CircularExtends => "EB007",
            RuleId::CircularRefines => "EB008",
            RuleId::CrossReferenceNotFound => "EB009",
            RuleId::WellDefinedness => "EB010",
            RuleId::DeadVariable => "EB011",
            RuleId::UnmodifiedVariable => "EB012",
            RuleId::IncompleteInitialisation => "EB014",
            RuleId::UndischargedProof => "EB015",
            RuleId::BrokenProof => "EB016",
            RuleId::ProofFileParseError => "EB017",
            RuleId::UndeclaredIdentifier => "EB018",
            RuleId::DuplicateComponent => "EB019",
            RuleId::UnknownType => "EB020",
            RuleId::DuplicateIdentifier => "EB021",
            RuleId::DuplicateLabel => "EB022",
            RuleId::ShadowedName => "EB023",
            RuleId::NewEventAssignsInheritedVariable => "EB024",
            RuleId::DisappearedVariable => "EB025",
            RuleId::AssignmentInPredicate => "EB026",
            RuleId::EventMergeMismatch => "EB027",
            RuleId::KeywordName => "EB028",
            RuleId::EmptyClause => "EB029",
            RuleId::ClauseOutOfOrder => "EB030",
            RuleId::NonPortableWhitespace => "EB031",
            RuleId::MissingLabel => "EB032",
            RuleId::PrimedDeclaredName => "EB033",
            RuleId::SectionOutOfOrder => "EB034",
            RuleId::MissingWitness => "EB035",
            RuleId::BecomesMemberOfInEvent => "EB100",
            RuleId::NonCanonicalBecomesSuchThat => "EB101",
            RuleId::NondeterministicInitialisation => "EB102",
            RuleId::UndeterminedParameter => "EB103",
            RuleId::UnboundedQuantifier => "EB104",
            RuleId::InfiniteSetValue => "EB105",
            RuleId::DeferredSetWithoutCardinality => "EB106",
            RuleId::UnboundedInteger => "EB107",
            RuleId::UndeterminedConstant => "EB108",
            RuleId::IndistinctCarrierSetConstants => "EB109",
            RuleId::ParallelAssignmentHazard => "EB110",
            RuleId::GuardEvaluationOrder => "EB111",
            RuleId::DroppedAbstractVariable => "EB112",
            RuleId::NonEqualityWitness => "EB113",
        }
    }

    /// Short human-readable name, used as SARIF `shortDescription`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            RuleId::XmlParseError => "XML parse error",
            RuleId::XmlRootError => "Unexpected XML root",
            RuleId::XmlAttributeError => "Missing XML attribute",
            RuleId::CamilleParseError => "Camille parse error",
            RuleId::FormulaParseError => "Formula parse error",
            RuleId::TypeError => "Type error",
            RuleId::CircularExtends => "Circular EXTENDS",
            RuleId::CircularRefines => "Circular REFINES",
            RuleId::CrossReferenceNotFound => "Cross-reference not found",
            RuleId::WellDefinedness => "Well-definedness condition",
            RuleId::DeadVariable => "Dead variable",
            RuleId::UnmodifiedVariable => "Unmodified variable",
            RuleId::IncompleteInitialisation => "Incomplete INITIALISATION",
            RuleId::UndischargedProof => "Undischarged proof obligation",
            RuleId::BrokenProof => "Broken proof",
            RuleId::ProofFileParseError => "Proof file parse error",
            RuleId::UndeclaredIdentifier => "Undeclared identifier",
            RuleId::DuplicateComponent => "Duplicate component",
            RuleId::UnknownType => "Unknown type",
            RuleId::DuplicateIdentifier => "Duplicate identifier",
            RuleId::DuplicateLabel => "Duplicate label",
            RuleId::ShadowedName => "Shadowed identifier",
            RuleId::NewEventAssignsInheritedVariable => "New event assigns inherited variable",
            RuleId::DisappearedVariable => "Disappeared variable assigned",
            RuleId::AssignmentInPredicate => "Assignment operator in predicate",
            RuleId::EventMergeMismatch => "Merged abstract events mismatch",
            RuleId::KeywordName => "Structural keyword as identifier",
            RuleId::EmptyClause => "Empty clause or label",
            RuleId::ClauseOutOfOrder => "Event clause out of order",
            RuleId::NonPortableWhitespace => "Non-portable whitespace",
            RuleId::MissingLabel => "Missing label",
            RuleId::PrimedDeclaredName => "Primed declared name",
            RuleId::SectionOutOfOrder => "Section out of order",
            RuleId::MissingWitness => "Missing witness",
            RuleId::BecomesMemberOfInEvent => "Nondeterministic choice in an event",
            RuleId::NonCanonicalBecomesSuchThat => {
                "Assignment by predicate is not in canonical form"
            }
            RuleId::NondeterministicInitialisation => "Initial state is not determined",
            RuleId::UndeterminedParameter => "Event parameter is not determined by the guards",
            RuleId::UnboundedQuantifier => "Quantifier has no finite domain",
            RuleId::InfiniteSetValue => "Infinite set used as a value",
            RuleId::DeferredSetWithoutCardinality => "Carrier set has no cardinality",
            RuleId::UnboundedInteger => "Unbounded integer",
            RuleId::UndeterminedConstant => "Constant is not determined by an equality",
            RuleId::IndistinctCarrierSetConstants => "Carrier-set constants are not distinguished",
            RuleId::ParallelAssignmentHazard => "Actions must be applied simultaneously",
            RuleId::GuardEvaluationOrder => "Guard depends on the evaluation order",
            RuleId::DroppedAbstractVariable => "Abstract variable dropped in the leaf machine",
            RuleId::NonEqualityWitness => "Witness is not an equality",
        }
    }

    /// One-line explanation, used as SARIF `fullDescription`.
    #[must_use]
    pub fn help(self) -> &'static str {
        match self {
            RuleId::XmlParseError => {
                "A Rodin XML file (.buc, .bum, .bcc, .bcm) could not be parsed."
            }
            RuleId::XmlRootError => {
                "A Rodin XML file's root element is not `org.eventb.core.contextFile` or `org.eventb.core.machineFile`."
            }
            RuleId::XmlAttributeError => {
                "A Rodin XML element is missing a required attribute (e.g. the `target` of an extends/refines/sees clause)."
            }
            RuleId::CamilleParseError => {
                "An .eventb file could not be parsed using the Camille textual notation grammar."
            }
            RuleId::FormulaParseError => {
                "A predicate or expression rejected by the Event-B formula grammar."
            }
            RuleId::TypeError => {
                "A predicate or expression failed type checking and was dropped from the output."
            }
            RuleId::CircularExtends => "A cycle was detected among contexts connected by EXTENDS.",
            RuleId::CircularRefines => "A cycle was detected among machines connected by REFINES.",
            RuleId::CrossReferenceNotFound => {
                "A SEES, EXTENDS, or REFINES clause names a component that does not exist."
            }
            RuleId::WellDefinedness => {
                "A formula has a non-trivial well-definedness condition (e.g. division by zero, function application domain)."
            }
            RuleId::DeadVariable => {
                "A machine variable is never used: nothing references it outside typing-shaped invariants, and no event assigns it."
            }
            RuleId::UnmodifiedVariable => {
                "A machine variable is assigned by INITIALISATION and never modified by any event, here or in any refinement — a constant in disguise; consider a CONSTANT with the initialisation as an axiom."
            }
            RuleId::IncompleteInitialisation => {
                "INITIALISATION leaves one or more machine variables unassigned."
            }
            RuleId::UndischargedProof => {
                "A proof obligation has not been fully discharged (it is pending, reviewed, or unattempted)."
            }
            RuleId::BrokenProof => {
                "A proof obligation is marked as broken, meaning its proof script is no longer valid."
            }
            RuleId::ProofFileParseError => {
                "A proof-related file (.bpr/.bpo/.bps) could not be parsed as XML."
            }
            RuleId::UndeclaredIdentifier => {
                "A guard, witness, or action references an identifier that is not in scope."
            }
            RuleId::DuplicateComponent => {
                "The same component name is defined in more than one file in the project."
            }
            RuleId::UnknownType => {
                "An axiom, invariant or guard reads an identifier before the predicate that types it. Axioms, invariants and guards are typed in source order, each against the types the ones before it established, so the predicate is dropped; move the typing predicate in front of it."
            }
            RuleId::DuplicateIdentifier => {
                "An identifier (variable, constant, carrier set, or event parameter) is declared more than once within the same scope."
            }
            RuleId::DuplicateLabel => {
                "A label (invariant, event, guard, action, axiom, or witness) is used more than once within the same scope."
            }
            RuleId::ShadowedName => {
                "A declared identifier collides with rossi's textual operator vocabulary (an ASCII operator spelling like `POW`/`or`, or a case variant of a literal token like `Nat`); uses of it can silently parse as the built-in token instead of the identifier."
            }
            RuleId::NewEventAssignsInheritedVariable => {
                "A new event (one that does not REFINE an abstract event) assigns a variable inherited from an abstract machine and kept in this refinement. A new event implicitly refines `skip`, so it must not modify inherited state; doing so leaves the event's refinement proof obligation unprovable. Either REFINES the abstract event that changes the variable, or data-refine the variable."
            }
            RuleId::DisappearedVariable => {
                "An event assigns a variable that an abstract machine declares but this refinement does not keep (it was data-refined away). A disappeared variable no longer exists in the concrete state, so it cannot be assigned; either redeclare it in this machine's VARIABLES, or remove the assignment."
            }
            RuleId::AssignmentInPredicate => {
                "An invariant, guard, witness, or axiom uses an assignment operator (`:=`/`≔`, `:∈`/`::`, or `:|`/`:∣`) where a predicate is required. An assignment cannot stand in a predicate position; the intended operator is most likely `=` for equality."
            }
            RuleId::EventMergeMismatch => {
                "An event that merges several abstract events must merge compatible ones: the abstract events' actions must be identical with coinciding labels, an abstract parameter name shared between them must have one type, and an extended event cannot merge at all."
            }
            RuleId::KeywordName => {
                "A declared name (context, machine, carrier set, constant, variable, event, or event parameter) is spelled like a structural keyword that textual notation cannot read back as a name: rossi's grammar recognises the keyword where the name is written (the keyword that ends its list, as in `sets a end`, or `INITIALISATION` as an event name), or stock Camille reserves that lowercase spelling outright (`machine`). Rodin's object model allows the name, but the model cannot round-trip through `.eventb` text."
            }
            RuleId::EmptyClause => {
                "A clause header carries no members — `WHERE` followed straight by `THEN`, `INVARIANTS` by the next section — or a label carries no formula. Write the guards, actions or predicates the clause needs, or delete the header; delete a label that has nothing to name."
            }
            RuleId::ClauseOutOfOrder => {
                "An event's clauses are written in a fixed order — `ANY`, `WHERE`, `WITH`, `WITNESS`, `THEN` — so a clause below one it must precede cannot be read. Move it above that clause."
            }
            RuleId::NonPortableWhitespace => {
                "Two names in a structural position (a component header, or an `EXTENDS` / `SETS` / `CONSTANTS` / `REFINES` / `SEES` / `VARIABLES` / `ANY` list) are separated by a Unicode space outside Camille's `layout_char` set. Rodin's math lexer treats these as whitespace and so does rossi, but stock Camille answers \"Unknown token\" and cannot open the file, so the model does not round-trip through Rodin's text editor. Run `rossi fmt -i` to rewrite the separators. The same code points inside a formula are folded into the formula text and handed to Rodin, so they are portable there and are not reported."
            }
            RuleId::MissingLabel => {
                "An axiom, invariant, guard, witness or action is written with no `@label`. Rodin's textual grammar requires one on every item and its static checker reports a missing one as an error, so the model does not round-trip through the toolchain. Write a label before the formula; only the first `VARIANT` item may go without."
            }
            RuleId::PrimedDeclaredName => {
                "A carrier set, constant, variable or event parameter is declared with the after-state prime (`c'`). The prime names the post-value of an assigned variable, so it belongs to a formula and never to a declaration: Rodin parses every declaration with primes disallowed and reports `InvalidIdentifierError`, dropping the name from the checked model. Only a witness label may be primed. Rename the declaration without the prime."
            }
            RuleId::SectionOutOfOrder => {
                "A context or machine writes a section after one it must precede — `SETS` below `AXIOMS`, `VARIABLES` below `INVARIANTS`. Rodin stores a component's children as an unordered set and cannot express an order, so rossi accepts any, but stock Camille fixes the order in its lexer and refuses to open the file (\"Set declarations are only allowed before the constants declarations\"). Run `rossi fmt -i` to reorder the sections."
            }
            RuleId::MissingWitness => {
                "A refining event drops an abstract parameter, or its abstract event assigns a variable that disappears here nondeterministically, and gives no well-typed witness for it (`@p …` for the parameter, `@x' …` for the after-state). Rodin warns and assumes `⊤`, the witness that says nothing: the refinement proof obligations then have to hold for any value, which usually makes them unprovable. Write a witness relating the dropped value to the concrete state."
            }
            RuleId::BecomesMemberOfInEvent => {
                "An ordinary event of a leaf machine assigns by choice from a set (`x :∈ S`) whose set is not a singleton. Event-B leaves the choice to the refinement; a runtime translation has to make it, and the model does not say which value to take, so two faithful translations can disagree on every run. Refine the action into a deterministic assignment, or narrow the set to one element. Only a singleton set is exempt: the choice is then certain."
            }
            RuleId::NonCanonicalBecomesSuchThat => {
                "An ordinary event of a leaf machine assigns by predicate (`x :∣ P`) whose condition is not a disjunction of branches that each fix every assigned variable with one equality over its primed name (`x′ = E`). A translation reads the value out of those equalities; outside that form it has to solve the predicate, which needs a solver and may admit several solutions. Rewrite the condition as `(p₁ ∧ x′ = E₁) ∨ … ∨ (pₙ ∧ x′ = Eₙ)`, or refine the action into a deterministic assignment. A condition in that form with more than one branch is reported at INFO: the shape is recognised, but that the branches are exhaustive and mutually exclusive is not verified here."
            }
            RuleId::NondeterministicInitialisation => {
                "A leaf machine's INITIALISATION does not pin the initial state: an action chooses from a non-singleton set, assigns by a predicate outside the canonical form, or reads a machine variable, which has no value before initialisation. A runtime translation must then be given the initial state from outside the model, and every consumer supplies it differently. Give each variable a deterministic initial value."
            }
            RuleId::UndeterminedParameter => {
                "An event of a leaf machine takes a parameter that no guard fixes with an equality `p = …`. A translation that fires the event has to come up with a value: it can enumerate the parameter when a guard confines it to a finite set, and otherwise has to guess, or be handed the value. This is reported at INFO because it is not always a defect — under trace-driven checking the trace supplies the parameter, and the model is right not to. Where the value is meant to follow from the model, add the equality guard."
            }
            RuleId::UnboundedQuantifier => {
                "A quantifier or comprehension (`∀`, `∃`, a set comprehension, `λ`, `⋃`, `⋂`) binds a variable that has neither a finite type nor a guard on its domain, so evaluating the formula means iterating a domain with no end. Rodin only has to reason about the formula, never evaluate it, so this is ordinary Event-B; a translation has to enumerate. Restrict the variable with a membership (`∀x · x ∈ S ⇒ …`), an equality, or a maplet membership in a relation. A variable of a finite type, `BOOL` or a bounded carrier set, needs no restriction and is not reported."
            }
            RuleId::InfiniteSetValue => {
                "An infinite set — `ℤ`, `ℕ`, `ℕ1`, a power set or a relation or function space over one, or a carrier set with no cardinality — appears where a value is needed: an assignment right-hand side, the set of a `:∈`, an operand of `card`, `min`, `max`, `union` or `inter`, a member of an enumerated set, an operand of `∪`, every operand of an `∩`, or the left of `∖`. A translation has to build the set to compute with it. Using an infinite set to give a type is fine and is not reported: `f ∈ ℕ → ℤ` states a type, `card(ℕ)` asks for a computation."
            }
            RuleId::DeferredSetWithoutCardinality => {
                "A carrier set is used where a value is needed, and no visible axiom bounds it. Every carrier set in Event-B is deferred — the language has no enumerated declaration — so nothing says how many elements it has, and a translation that has to build it either invents a size or grows the set on demand, where `card` then answers with the population so far rather than the modelled size. Add `finite(S)`, `partition(S, …)`, an enumeration `S = {a, b}`, or `card(S) = n`. A `partition` is usually the one to reach for: it settles this and EB109 at once."
            }
            RuleId::UnboundedInteger => {
                "A variable, constant or event parameter of type `ℤ` is constrained by nothing but its type: no relational bound, no interval, no enumerated set. Event-B integers are mathematical and unbounded; every implementation type is not, so a translation has to pick a width the model never mentions, and the tools surveyed pick different ones — a 201-element window, the int16 range, a 256-bit signed integer. The model is only wrong at the edges, which is exactly where those choices differ. Give the identifier an interval or a relational bound, or adopt a bounded integer type throughout."
            }
            RuleId::UndeterminedConstant => {
                "A constant has no axiom that fixes its value: no defining equality, no membership in a `partition`, and, for a set-valued constant, no `finite` plus `card` pair. A translation has to have values for the constants before it can run anything, and with only a constraint like `n ∈ ℕ` it has to solve for one, ask the user, or invent one. A defining expression may refer to other constants as long as they are themselves determined and the definitions do not form a cycle; a cycle is reported the same way, naming the constant the definition waits on."
            }
            RuleId::IndistinctCarrierSetConstants => {
                "Two constants of the same carrier set are never related by `=` or `≠`, so the model does not say whether they denote the same element. A translation that has to give them values cannot tell whether one object or two is meant. Relate them, or declare the set with a `partition`, which settles every pair at once and is what most models do — with four constants that is one predicate instead of six inequalities. Reported at INFO because a model lacking the relation is more often under-specified than wrong."
            }
            RuleId::ParallelAssignmentHazard => {
                "An event assigns several variables at once, or one of its actions reads a variable another assigns. Event-B applies every action of an event simultaneously, all reading the before-state, so `x, y ≔ y, x` swaps and `x ≔ y ∥ y ≔ x` does too. Emitting them as statements in sequence does not: the second reads what the first wrote. The model is unambiguous here, so this is not an uncertainty — it is a mistranslation trap, and a measured one: a generator was found turning a two-maplet override into two sequential assignments that do not swap. Translations that get it right copy the read variables first, compute every value before applying any, or prove the sequentialisation correct. Reported at INFO because the model is right; what has to be checked is the consumer."
            }
            RuleId::GuardEvaluationOrder => {
                "A guard is well-defined only under a condition that no earlier guard of the same event states — typically `f(x)` with `x ∈ dom(f)` established later or not at all. In Event-B a guard's well-definedness is a separate proof obligation and guards are a conjunction, not a sequence, so nothing is wrong with the model. A translation that evaluates guards in written order and aborts on the first partial application will fail where the model merely says the event is disabled. Order the guards so each one's well-definedness follows from those above it, or evaluate guards defensively. Overlaps EB010, which reports the well-definedness condition itself; this rule adds only the ordering claim."
            }
            RuleId::DroppedAbstractVariable => {
                "A leaf machine does not keep a variable its abstraction declares, whatever gluing invariant relates what replaced it. Refinement allows this and the proof obligations cover it, but a translation of the leaf alone cannot reproduce anything stated about the dropped variable, so any formula over it is silently lost. Keep the variable, or accept that properties expressed over it are not observable in what runs. Related to EB025, which reports a variable that disappears without the refinement machinery to justify it."
            }
            RuleId::NonEqualityWitness => {
                "A witness gives the abstract parameter or variable a constraint rather than a value: its predicate is not `abstract = E`. The refinement proof only needs a witness to exist, so a constraining witness is perfectly sound. Anything that consumes the refinement relation — a trace checker mapping a concrete run onto the abstract level — has to compute the abstract value, and a constraint does not give it one. Write the witness as an equality where the concrete state determines the abstract value."
            }
        }
    }

    /// The severity a diagnostic carrying this rule typically reports at.
    /// Used by SARIF as `defaultConfiguration.level`.
    #[must_use]
    pub fn default_severity(self) -> Severity {
        match self {
            RuleId::XmlParseError
            | RuleId::XmlRootError
            | RuleId::XmlAttributeError
            | RuleId::CamilleParseError
            | RuleId::FormulaParseError
            | RuleId::TypeError
            | RuleId::CircularExtends
            | RuleId::CircularRefines
            | RuleId::CrossReferenceNotFound
            | RuleId::UndeclaredIdentifier
            | RuleId::UnknownType
            | RuleId::DuplicateIdentifier
            | RuleId::DuplicateLabel
            | RuleId::NewEventAssignsInheritedVariable
            | RuleId::DisappearedVariable
            | RuleId::AssignmentInPredicate
            | RuleId::EventMergeMismatch
            | RuleId::EmptyClause
            | RuleId::ClauseOutOfOrder
            | RuleId::MissingLabel
            | RuleId::PrimedDeclaredName
            | RuleId::DuplicateComponent => Severity::Error,
            RuleId::WellDefinedness
            | RuleId::UndeterminedParameter
            | RuleId::UnboundedInteger
            | RuleId::IndistinctCarrierSetConstants
            | RuleId::ParallelAssignmentHazard
            | RuleId::GuardEvaluationOrder
            | RuleId::DroppedAbstractVariable
            | RuleId::NonEqualityWitness => Severity::Info,
            RuleId::DeadVariable
            | RuleId::UnmodifiedVariable
            | RuleId::IncompleteInitialisation
            | RuleId::UndischargedProof
            | RuleId::BrokenProof
            | RuleId::ProofFileParseError
            | RuleId::ShadowedName
            | RuleId::KeywordName
            | RuleId::NonPortableWhitespace
            | RuleId::SectionOutOfOrder
            | RuleId::MissingWitness
            | RuleId::BecomesMemberOfInEvent
            | RuleId::NonCanonicalBecomesSuchThat
            | RuleId::NondeterministicInitialisation
            | RuleId::UnboundedQuantifier
            | RuleId::InfiniteSetValue
            | RuleId::DeferredSetWithoutCardinality
            | RuleId::UndeterminedConstant => Severity::Warning,
        }
    }

    /// The rule a parse error belongs to, when the grammar names the mistake
    /// precisely enough for one. `None` means the failure carries no more than
    /// "this text was rejected", which each consumer tags with the rule for
    /// its own notation — EB004 for Camille text, EB001 for Rodin XML.
    ///
    /// One home for the mapping keeps the CLI, the SARIF report and the editor
    /// naming the same mistake the same way.
    #[must_use]
    pub fn for_parse_error(error: &rossi::ParseError) -> Option<RuleId> {
        match error {
            rossi::ParseError::EmptyClause { .. } | rossi::ParseError::MissingFormula { .. } => {
                Some(RuleId::EmptyClause)
            }
            rossi::ParseError::ClauseOutOfOrder { .. } => Some(RuleId::ClauseOutOfOrder),
            rossi::ParseError::MissingLabel { .. } => Some(RuleId::MissingLabel),
            rossi::ParseError::AssignmentInPredicate { .. } => Some(RuleId::AssignmentInPredicate),
            rossi::ParseError::NotAPrefixOperator { .. }
            | rossi::ParseError::UnknownInfixOperator { .. }
            | rossi::ParseError::IncompatibleOperators { .. } => Some(RuleId::FormulaParseError),
            _ => None,
        }
    }

    /// Every defined rule, in catalogue order. Used to build the SARIF
    /// `tool.driver.rules[]` descriptor list.
    #[must_use]
    pub fn all() -> &'static [RuleId] {
        &[
            RuleId::XmlParseError,
            RuleId::XmlRootError,
            RuleId::XmlAttributeError,
            RuleId::CamilleParseError,
            RuleId::FormulaParseError,
            RuleId::TypeError,
            RuleId::CircularExtends,
            RuleId::CircularRefines,
            RuleId::CrossReferenceNotFound,
            RuleId::WellDefinedness,
            RuleId::DeadVariable,
            RuleId::UnmodifiedVariable,
            RuleId::IncompleteInitialisation,
            RuleId::UndischargedProof,
            RuleId::BrokenProof,
            RuleId::ProofFileParseError,
            RuleId::UndeclaredIdentifier,
            RuleId::DuplicateComponent,
            RuleId::UnknownType,
            RuleId::DuplicateIdentifier,
            RuleId::DuplicateLabel,
            RuleId::ShadowedName,
            RuleId::NewEventAssignsInheritedVariable,
            RuleId::DisappearedVariable,
            RuleId::AssignmentInPredicate,
            RuleId::EventMergeMismatch,
            RuleId::KeywordName,
            RuleId::EmptyClause,
            RuleId::ClauseOutOfOrder,
            RuleId::NonPortableWhitespace,
            RuleId::MissingLabel,
            RuleId::PrimedDeclaredName,
            RuleId::SectionOutOfOrder,
            RuleId::MissingWitness,
            RuleId::BecomesMemberOfInEvent,
            RuleId::NonCanonicalBecomesSuchThat,
            RuleId::NondeterministicInitialisation,
            RuleId::UndeterminedParameter,
            RuleId::UnboundedQuantifier,
            RuleId::InfiniteSetValue,
            RuleId::DeferredSetWithoutCardinality,
            RuleId::UnboundedInteger,
            RuleId::UndeterminedConstant,
            RuleId::IndistinctCarrierSetConstants,
            RuleId::ParallelAssignmentHazard,
            RuleId::GuardEvaluationOrder,
            RuleId::DroppedAbstractVariable,
            RuleId::NonEqualityWitness,
        ]
    }
}

impl std::fmt::Display for RuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(RuleId::XmlParseError.code(), "EB001");
        assert_eq!(RuleId::XmlRootError.code(), "EB002");
        assert_eq!(RuleId::XmlAttributeError.code(), "EB003");
        assert_eq!(RuleId::CamilleParseError.code(), "EB004");
        assert_eq!(RuleId::FormulaParseError.code(), "EB005");
        assert_eq!(RuleId::TypeError.code(), "EB006");
        assert_eq!(RuleId::CircularExtends.code(), "EB007");
        assert_eq!(RuleId::CircularRefines.code(), "EB008");
        assert_eq!(RuleId::CrossReferenceNotFound.code(), "EB009");
        assert_eq!(RuleId::WellDefinedness.code(), "EB010");
        assert_eq!(RuleId::DeadVariable.code(), "EB011");
        assert_eq!(RuleId::UnmodifiedVariable.code(), "EB012");
        assert_eq!(RuleId::IncompleteInitialisation.code(), "EB014");
        assert_eq!(RuleId::UndischargedProof.code(), "EB015");
        assert_eq!(RuleId::BrokenProof.code(), "EB016");
        assert_eq!(RuleId::ProofFileParseError.code(), "EB017");
        assert_eq!(RuleId::UndeclaredIdentifier.code(), "EB018");
        assert_eq!(RuleId::DuplicateComponent.code(), "EB019");
        assert_eq!(RuleId::UnknownType.code(), "EB020");
        assert_eq!(RuleId::DuplicateIdentifier.code(), "EB021");
        assert_eq!(RuleId::DuplicateLabel.code(), "EB022");
        assert_eq!(RuleId::ShadowedName.code(), "EB023");
        assert_eq!(RuleId::NewEventAssignsInheritedVariable.code(), "EB024");
        assert_eq!(RuleId::DisappearedVariable.code(), "EB025");
        assert_eq!(RuleId::AssignmentInPredicate.code(), "EB026");
        assert_eq!(RuleId::KeywordName.code(), "EB028");
        assert_eq!(RuleId::EmptyClause.code(), "EB029");
        assert_eq!(RuleId::ClauseOutOfOrder.code(), "EB030");
        assert_eq!(RuleId::NonPortableWhitespace.code(), "EB031");
        assert_eq!(RuleId::MissingLabel.code(), "EB032");
        assert_eq!(RuleId::PrimedDeclaredName.code(), "EB033");
        assert_eq!(RuleId::SectionOutOfOrder.code(), "EB034");
        assert_eq!(RuleId::MissingWitness.code(), "EB035");
        assert_eq!(RuleId::BecomesMemberOfInEvent.code(), "EB100");
        assert_eq!(RuleId::NonCanonicalBecomesSuchThat.code(), "EB101");
        assert_eq!(RuleId::NondeterministicInitialisation.code(), "EB102");
        assert_eq!(RuleId::UndeterminedParameter.code(), "EB103");
        assert_eq!(RuleId::UnboundedQuantifier.code(), "EB104");
        assert_eq!(RuleId::InfiniteSetValue.code(), "EB105");
        assert_eq!(RuleId::DeferredSetWithoutCardinality.code(), "EB106");
        assert_eq!(RuleId::UnboundedInteger.code(), "EB107");
        assert_eq!(RuleId::UndeterminedConstant.code(), "EB108");
        assert_eq!(RuleId::IndistinctCarrierSetConstants.code(), "EB109");
        assert_eq!(RuleId::ParallelAssignmentHazard.code(), "EB110");
        assert_eq!(RuleId::GuardEvaluationOrder.code(), "EB111");
        assert_eq!(RuleId::DroppedAbstractVariable.code(), "EB112");
        assert_eq!(RuleId::NonEqualityWitness.code(), "EB113");
    }

    /// `all()` is a hand-maintained array with no exhaustiveness check, unlike
    /// the `match` arms the compiler polices. A rule missing from it emits a
    /// SARIF `results[].ruleId` with no matching `tool.driver.rules[]` entry,
    /// which consumers reject — so check it against the code scheme itself: a
    /// length alone would still pass when a new variant is added and the count
    /// bumped without touching `all()`. Comparing the exact list in catalogue
    /// order also subsumes the uniqueness and length checks.
    #[test]
    fn all_lists_every_rule() {
        // `EB001`..`EB035` minus the one documented gap (EB013), then the
        // runtime-translation block `EB100`..`EB113`.
        let expected: Vec<String> = (1..=35)
            .filter(|n| *n != 13)
            .chain(100..=113)
            .map(|n| format!("EB{n:03}"))
            .collect();
        let listed: Vec<&str> = RuleId::all().iter().map(|r| r.code()).collect();
        assert_eq!(listed, expected);
    }

    #[test]
    fn display_uses_code() {
        assert_eq!(format!("{}", RuleId::CircularExtends), "EB007");
    }
}
