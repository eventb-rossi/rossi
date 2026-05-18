//! #24 — property tests for the normalization / inference layer.
//!
//! These are regression insurance: they assert laws that must hold for
//! every input, and they catch classes of bugs that hand-written
//! fixtures miss. Run with `cargo test -p rossi-build --test properties`.
//!
//! Laws covered:
//!
//! 1. **Idempotence**: `canonical(parse(canonical(parse(s))))
//!                     == canonical(parse(s))`.
//! 2. **Parseability**: every canonical form parses back.
//! 3. **AST round-trip** (modulo type ascriptions):
//!    `strip(parse(canonical(p))) == strip(p)`.
//! 4. **Inference monotonicity**: if `infer_constants` types `c` from
//!    axioms `A`, then `infer_constants` still types `c` from any
//!    superset `A ∪ B` — adding axioms never "untypes" a constant.
//! 5. **Scope stack**: push/insert/pop restores outer env regardless
//!    of how many layers.

use proptest::prelude::*;
use rossi::{parse_action_str, parse_predicate_str};
use rossi_build::infer::infer_constants;
use rossi_build::normalize::{canonical_action, canonical_predicate};
use rossi_build::sc_view::{strip_type_ascriptions_action, strip_type_ascriptions_pred};
use rossi_build::type_env::TypeEnv;
use rossi_build::types::Type;

// ---------------------------------------------------------------------
// Strategies — hand-curated string samples instead of grammar-walking.
// Covers the predicate/action shapes that actually appear in the corpus.
// ---------------------------------------------------------------------

fn predicate_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("n ∈ ℕ".to_string()),
        Just("register ⊆ USERS".to_string()),
        Just("x ∈ dom(f)".to_string()),
        Just("f(x) ≤ f(y)".to_string()),
        Just("x ∉ ran(f)".to_string()),
        Just("a ↦ b ∈ rel".to_string()),
        Just("x ∈ S ∧ y ∈ T".to_string()),
        Just("p ∈ dom(m) ⇒ m(p) ∈ ran(m)".to_string()),
        Just("∀x · x ∈ S ⇒ x ∈ T".to_string()),
        Just("∃y · y ∈ S ∧ y ≠ z".to_string()),
        Just("¬(x = y)".to_string()),
        Just("card(S) > 0".to_string()),
        Just("x ∈ S ∩ T".to_string()),
        Just("r ⊆ S × T".to_string()),
        Just("f ∈ S → T".to_string()),
    ]
}

fn action_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("x ≔ 1".to_string()),
        Just("x ≔ x + 1".to_string()),
        Just("register ≔ register ∪ {u}".to_string()),
        Just("register ≔ register ∖ {u}".to_string()),
        Just("x, y ≔ y, x".to_string()),
        Just("x :∈ S".to_string()),
        Just("x :∣ x' > x".to_string()),
    ]
}

// ---------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// `canonical_predicate` is idempotent: applying it twice changes
    /// nothing. Formally, for any valid predicate string `s`:
    ///   let p1 = parse(s); let c1 = canonical(p1);
    ///   let p2 = parse(c1); let c2 = canonical(p2);
    ///   c1 == c2
    #[test]
    fn canonical_predicate_is_idempotent(s in predicate_strategy()) {
        let p1 = parse_predicate_str(&s).expect("strategy yields parseable predicates");
        let c1 = canonical_predicate(&p1);
        let p2 = parse_predicate_str(&c1).expect("canonical form must re-parse");
        let c2 = canonical_predicate(&p2);
        prop_assert_eq!(c1, c2);
    }

    /// Every canonical predicate string re-parses cleanly. This is the
    /// guarantee that our Rodin-canonical output never produces text
    /// our own parser refuses.
    #[test]
    fn canonical_predicate_reparseable(s in predicate_strategy()) {
        let p = parse_predicate_str(&s).unwrap();
        let c = canonical_predicate(&p);
        prop_assert!(
            parse_predicate_str(&c).is_ok(),
            "canonical form did not re-parse: {c:?}"
        );
    }

    /// AST round-trip modulo type ascriptions: `strip(parse(canonical(p))) == strip(p)`.
    /// The strip eats the `⦂T` annotations Rodin adds during type-check
    /// so predicates of the form `∀x·P` and `∀x⦂ℤ·P` compare equal.
    #[test]
    fn canonical_predicate_preserves_ast(s in predicate_strategy()) {
        let p = parse_predicate_str(&s).unwrap();
        let c = canonical_predicate(&p);
        let round = parse_predicate_str(&c).unwrap();
        prop_assert_eq!(
            strip_type_ascriptions_pred(round),
            strip_type_ascriptions_pred(p)
        );
    }

    /// Same three laws for actions.
    #[test]
    fn canonical_action_is_idempotent(s in action_strategy()) {
        let a1 = parse_action_str(&s).unwrap();
        let c1 = canonical_action(&a1);
        let a2 = parse_action_str(&c1).unwrap();
        let c2 = canonical_action(&a2);
        prop_assert_eq!(c1, c2);
    }

    #[test]
    fn canonical_action_reparseable(s in action_strategy()) {
        let a = parse_action_str(&s).unwrap();
        let c = canonical_action(&a);
        prop_assert!(parse_action_str(&c).is_ok(), "action canonical did not re-parse: {c:?}");
    }

    #[test]
    fn canonical_action_preserves_ast(s in action_strategy()) {
        let a = parse_action_str(&s).unwrap();
        let c = canonical_action(&a);
        let round = parse_action_str(&c).unwrap();
        prop_assert_eq!(
            strip_type_ascriptions_action(round),
            strip_type_ascriptions_action(a)
        );
    }

    /// Strip is idempotent on predicates.
    #[test]
    fn strip_predicate_is_idempotent(s in predicate_strategy()) {
        let p = parse_predicate_str(&s).unwrap();
        let once = strip_type_ascriptions_pred(p);
        let twice = strip_type_ascriptions_pred(once.clone());
        prop_assert_eq!(once, twice);
    }

    /// Strip is idempotent on actions.
    #[test]
    fn strip_action_is_idempotent(s in action_strategy()) {
        let a = parse_action_str(&s).unwrap();
        let once = strip_type_ascriptions_action(a);
        let twice = strip_type_ascriptions_action(once.clone());
        prop_assert_eq!(once, twice);
    }
}

// ---------------------------------------------------------------------
// Inference monotonicity.
//
// Strategy: fabricate a carrier-set + constant set + handful of typing
// axioms. Run inference. Then re-run with the axioms reshuffled and /
// or augmented with extra (unrelated) axioms. The set of typed
// constants must not shrink, and types must not change.
// ---------------------------------------------------------------------

fn axiom_string_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("a ∈ USERS".to_string()),
        Just("b ⊆ USERS".to_string()),
        Just("n = 42".to_string()),
        Just("S = {1, 2, 3}".to_string()),
        Just("partition(USERS, {a}, {b})".to_string()),
        Just("r ∈ USERS ↔ USERS".to_string()),
        // Intentionally unrelated axioms that shouldn't type anything.
        Just("TRUE = TRUE".to_string()),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(60))]

    #[test]
    fn infer_constants_is_monotone(
        core_axioms in proptest::collection::vec(axiom_string_strategy(), 1..6),
        extra_axioms in proptest::collection::vec(axiom_string_strategy(), 0..4),
    ) {
        // Seed with a single carrier set USERS and the constants that
        // appear in our axiom strategy.
        let constants: Vec<String> = ["a", "b", "n", "S", "r"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let parsed_core: Vec<_> = core_axioms
            .iter()
            .filter_map(|s| parse_predicate_str(s).ok())
            .collect();
        let parsed_superset: Vec<_> = core_axioms
            .iter()
            .chain(extra_axioms.iter())
            .filter_map(|s| parse_predicate_str(s).ok())
            .collect();

        let mut env_small = TypeEnv::new();
        env_small.add_carrier_set("USERS");
        infer_constants(&mut env_small, &constants, &parsed_core);

        let mut env_big = TypeEnv::new();
        env_big.add_carrier_set("USERS");
        infer_constants(&mut env_big, &constants, &parsed_superset);

        // Every name typed with the small axiom set must still be
        // typed with the superset — and with the same type.
        for name in &constants {
            if let Some(ty_small) = env_small.get(name) {
                let ty_big = env_big.get(name);
                prop_assert_eq!(
                    Some(ty_small),
                    ty_big,
                    "name {} was typed as {:?} with core axioms but {:?} with superset",
                    name,
                    ty_small,
                    ty_big
                );
            }
        }
    }
}

// ---------------------------------------------------------------------
// TypeEnv scope stack: deeply-nested push/pop restores faithfully.
// ---------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Opening N scopes, inserting one name per scope, and popping N
    /// times restores the exact outer env that was present before the
    /// first push.
    #[test]
    fn scope_stack_restores_after_n_pushes(n in 0usize..10usize) {
        let mut env = TypeEnv::new();
        env.insert("x", Type::Integer);
        let snapshot: Vec<(String, Type)> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();

        for i in 0..n {
            env.push_scope();
            env.insert("x", Type::GivenSet(format!("S{i}")));
            env.insert(format!("y{i}"), Type::Boolean);
        }
        for _ in 0..n {
            env.pop_scope();
        }

        // After all pops, env must be exactly the snapshot.
        let after: Vec<(String, Type)> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        prop_assert_eq!(after, snapshot);
    }
}
