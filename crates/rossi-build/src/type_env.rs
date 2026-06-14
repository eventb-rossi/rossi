//! A typing environment: identifier → [`Type`] with push/pop scopes.
//!
//! Equivalent in spirit to Rodin's `ITypeEnvironmentBuilder`. Adds a scope
//! stack so callers can introduce short-lived bindings — event parameters,
//! quantifier binders — and revert them cleanly. See
//! [`TypeEnv::push_scope`] / [`TypeEnv::pop_scope`] / [`TypeEnv::scoped`].

use std::collections::BTreeMap;

use crate::types::Type;

#[derive(Debug, Default, Clone)]
pub struct TypeEnv {
    /// Current effective env: outer-scope + all active overlays flattened.
    by_name: BTreeMap<String, Type>,
    /// For each open scope, the values we need to restore (or delete) on
    /// pop. `None` means "name was absent before the scope opened; delete
    /// on pop". `Some(t)` means "restore to this value on pop".
    scopes: Vec<BTreeMap<String, Option<Type>>>,
}

impl TypeEnv {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a declaration. Later inserts for the same name overwrite.
    /// If a scope is active, the old value is recorded so it can be
    /// restored on `pop_scope`.
    pub fn insert(&mut self, name: impl Into<String>, ty: Type) {
        let name = name.into();
        if let Some(scope) = self.scopes.last_mut() {
            // Only record the *first* shadow within this scope — if the
            // same name is rewritten twice in one scope, we still want to
            // restore the pre-scope value on pop, not the intermediate.
            scope
                .entry(name.clone())
                .or_insert_with(|| self.by_name.get(&name).cloned());
        }
        self.by_name.insert(name, ty);
    }

    /// Hide any binding of `name` for the duration of the current scope,
    /// so it reads as undeclared until [`TypeEnv::pop_scope`]. Used when a
    /// binder shadows an outer name but its own type can't be inferred: the
    /// name must mask the outer declaration rather than leak its type. With
    /// no scope open this is a permanent removal.
    pub fn remove(&mut self, name: &str) {
        if let Some(scope) = self.scopes.last_mut() {
            scope
                .entry(name.to_string())
                .or_insert_with(|| self.by_name.get(name).cloned());
        }
        self.by_name.remove(name);
    }

    /// Insert only if the name is not yet present. Returns `true` if this
    /// call inserted the entry.
    pub fn insert_if_absent(&mut self, name: impl Into<String>, ty: Type) -> bool {
        use std::collections::btree_map::Entry;
        match self.by_name.entry(name.into()) {
            Entry::Vacant(e) => {
                e.insert(ty);
                true
            }
            Entry::Occupied(_) => false,
        }
    }

    pub fn get(&self, name: &str) -> Option<&Type> {
        self.by_name.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    /// Seed the environment with a carrier set `S`. The set itself has type
    /// `ℙ(S)` (so `S ∈ ℙ(…)` is well-formed; Rodin stores `ℙ(S)` in the
    /// carrier set's `type` attribute).
    pub fn add_carrier_set(&mut self, name: &str) {
        self.insert(name, Type::carrier_set_type(name));
    }

    /// Iterate entries, sorted by name. Handy for deterministic output.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Type)> {
        self.by_name.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Open a new scope. Subsequent `insert` calls can be reverted by
    /// [`TypeEnv::pop_scope`].
    pub fn push_scope(&mut self) {
        self.scopes.push(BTreeMap::new());
    }

    /// Close the most recently opened scope, restoring every name to its
    /// pre-scope value (or removing it, if it was absent before).
    ///
    /// # Panics
    /// Panics if no scope is open — callers are expected to pair
    /// `push_scope` and `pop_scope` exactly, or use [`TypeEnv::scoped`]
    /// which handles the pairing automatically.
    pub fn pop_scope(&mut self) {
        let scope = self
            .scopes
            .pop()
            .expect("pop_scope called without a matching push_scope");
        for (name, previous) in scope {
            match previous {
                Some(t) => {
                    self.by_name.insert(name, t);
                }
                None => {
                    self.by_name.remove(&name);
                }
            }
        }
    }

    /// Run `body` with a fresh scope automatically popped on exit.
    pub fn scoped<F, R>(&mut self, body: F) -> R
    where
        F: FnOnce(&mut TypeEnv) -> R,
    {
        self.push_scope();
        let r = body(self);
        self.pop_scope();
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carrier_set_has_powerset_type() {
        let mut env = TypeEnv::new();
        env.add_carrier_set("USERS");
        assert_eq!(env.get("USERS").unwrap().to_rodin_canonical(), "ℙ(USERS)");
    }

    #[test]
    fn insert_if_absent_keeps_first() {
        let mut env = TypeEnv::new();
        assert!(env.insert_if_absent("x", Type::Integer));
        assert!(!env.insert_if_absent("x", Type::Boolean));
        assert_eq!(env.get("x").unwrap(), &Type::Integer);
    }

    // ------------------------------------------------------------------
    // Scope-stack behaviour (drives M0 task #17).
    // ------------------------------------------------------------------

    #[test]
    fn scope_push_and_pop_restores_outer() {
        let mut env = TypeEnv::new();
        env.insert("x", Type::Integer);
        env.push_scope();
        env.insert("x", Type::Boolean); // shadows
        assert_eq!(env.get("x"), Some(&Type::Boolean));
        env.pop_scope();
        assert_eq!(env.get("x"), Some(&Type::Integer));
    }

    #[test]
    fn pop_removes_names_introduced_in_scope() {
        let mut env = TypeEnv::new();
        env.push_scope();
        env.insert("p", Type::Integer);
        assert!(env.contains("p"));
        env.pop_scope();
        assert!(!env.contains("p"));
    }

    #[test]
    fn nested_scopes_restore_each_layer() {
        let mut env = TypeEnv::new();
        env.insert("x", Type::Integer);
        env.push_scope();
        env.insert("x", Type::Boolean);
        env.push_scope();
        env.insert("x", Type::carrier_set_type("S"));
        assert_eq!(env.get("x"), Some(&Type::carrier_set_type("S")));
        env.pop_scope();
        assert_eq!(env.get("x"), Some(&Type::Boolean));
        env.pop_scope();
        assert_eq!(env.get("x"), Some(&Type::Integer));
    }

    #[test]
    fn scoped_guard_runs_body_and_pops_even_on_return() {
        let mut env = TypeEnv::new();
        env.insert("x", Type::Integer);
        let seen_inside = env.scoped(|env| {
            env.insert("x", Type::Boolean);
            env.get("x").cloned()
        });
        assert_eq!(seen_inside, Some(Type::Boolean));
        assert_eq!(env.get("x"), Some(&Type::Integer));
    }

    #[test]
    #[should_panic]
    fn pop_without_matching_push_panics() {
        let mut env = TypeEnv::new();
        env.pop_scope();
    }
}
