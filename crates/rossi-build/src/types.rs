//! Event-B type system as used by the static checker.
//!
//! These mirror the `org.eventb.core.ast.Type` hierarchy in Rodin. The
//! canonical string produced by [`Type::to_rodin_canonical`] is the form
//! Rodin writes into the `org.eventb.core.type` attribute of checked
//! elements, e.g. `ℙ(USERS×(AUCTIONS×ITEMS))`.

/// An Event-B type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type {
    /// `BOOL`
    Boolean,
    /// `ℤ`
    Integer,
    /// A given set from a carrier-set declaration, e.g. `USERS`.
    GivenSet(String),
    /// `ℙ(T)`
    PowerSet(Box<Type>),
    /// `T × U` (left, right)
    Product(Box<Type>, Box<Type>),
}

impl Type {
    /// Powerset convenience constructor: `Type::pow(Type::Integer)` → `ℙ(ℤ)`.
    pub fn pow(t: Type) -> Type {
        Type::PowerSet(Box::new(t))
    }

    /// Cartesian product convenience constructor.
    pub fn prod(left: Type, right: Type) -> Type {
        Type::Product(Box::new(left), Box::new(right))
    }

    /// Relation / function type `ℙ(left × right)` — Event-B's `left ↔ right`.
    pub fn relation(left: Type, right: Type) -> Type {
        Type::pow(Type::prod(left, right))
    }

    /// Inverse of [`Type::relation`]: if `self` is `ℙ(α × β)`, consume it and
    /// yield `(α, β)`; otherwise `None`. Consuming (not borrowing) so callers
    /// move the component types out without cloning, matching the inference
    /// engine's idiom.
    pub fn into_relation(self) -> Option<(Type, Type)> {
        match self {
            Type::PowerSet(inner) => match *inner {
                Type::Product(l, r) => Some((*l, *r)),
                _ => None,
            },
            _ => None,
        }
    }

    /// A carrier-set `S` has type `ℙ(S)` in Rodin's system — this is the
    /// type of the set itself, not of its elements.
    pub fn carrier_set_type(name: &str) -> Type {
        Type::pow(Type::GivenSet(name.to_string()))
    }

    /// The canonical Rodin string. This is what ends up in the
    /// `org.eventb.core.type="..."` attribute of `.bcc`/`.bcm` elements.
    ///
    /// The form collapses whitespace and uses Unicode symbols only:
    /// - `BOOL`
    /// - `ℤ`
    /// - `USERS`
    /// - `ℙ(ℤ)`
    /// - `USERS×AUCTIONS`
    /// - `ℙ(USERS×(AUCTIONS×ITEMS))`
    ///
    /// Products are right-associative and parenthesised only on the right-hand
    /// side of another product, matching Rodin's `Formula.toString()` shape
    /// (confirmed against `AuctionMachine.bcm` and `binary-search/M2.bcm`).
    pub fn to_rodin_canonical(&self) -> String {
        let mut out = String::new();
        self.write_canonical(&mut out);
        out
    }

    fn write_canonical(&self, out: &mut String) {
        match self {
            Type::Boolean => out.push_str("BOOL"),
            Type::Integer => out.push('ℤ'),
            Type::GivenSet(name) => out.push_str(name),
            Type::PowerSet(inner) => {
                out.push('ℙ');
                out.push('(');
                inner.write_canonical(out);
                out.push(')');
            }
            Type::Product(left, right) => {
                left.write_canonical(out);
                out.push('×');
                // Right operand of a product gets parenthesised if it is
                // itself a product — matches Rodin's shape.
                match right.as_ref() {
                    Type::Product(..) => {
                        out.push('(');
                        right.write_canonical(out);
                        out.push(')');
                    }
                    _ => right.write_canonical(out),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_primitives() {
        assert_eq!(Type::Integer.to_rodin_canonical(), "ℤ");
        assert_eq!(Type::Boolean.to_rodin_canonical(), "BOOL");
        assert_eq!(Type::GivenSet("USERS".into()).to_rodin_canonical(), "USERS");
    }

    #[test]
    fn canonical_carrier_set() {
        // A carrier set USERS has type ℙ(USERS) — what appears on scCarrierSet.
        assert_eq!(
            Type::carrier_set_type("USERS").to_rodin_canonical(),
            "ℙ(USERS)"
        );
    }

    #[test]
    fn canonical_flat_product() {
        // AUCTIONS × ITEMS
        let t = Type::prod(
            Type::GivenSet("AUCTIONS".into()),
            Type::GivenSet("ITEMS".into()),
        );
        assert_eq!(t.to_rodin_canonical(), "AUCTIONS×ITEMS");
    }

    #[test]
    fn canonical_right_nested_product() {
        // USERS × (AUCTIONS × ITEMS) — from AuctionMachine.bcm's `buyer` var.
        let t = Type::prod(
            Type::GivenSet("USERS".into()),
            Type::prod(
                Type::GivenSet("AUCTIONS".into()),
                Type::GivenSet("ITEMS".into()),
            ),
        );
        assert_eq!(t.to_rodin_canonical(), "USERS×(AUCTIONS×ITEMS)");
    }

    #[test]
    fn canonical_powerset_of_product() {
        // ℙ(USERS × (AUCTIONS × ITEMS))
        let t = Type::pow(Type::prod(
            Type::GivenSet("USERS".into()),
            Type::prod(
                Type::GivenSet("AUCTIONS".into()),
                Type::GivenSet("ITEMS".into()),
            ),
        ));
        assert_eq!(t.to_rodin_canonical(), "ℙ(USERS×(AUCTIONS×ITEMS))");
    }

    #[test]
    fn canonical_relation_type() {
        // ℙ(ℤ×ℤ) — from binary-search's constant `f`. Built from the primitive
        // `pow`/`prod` on purpose, to keep the canonical-form check decoupled
        // from the `relation` constructor (see `relation_constructor`).
        let t = Type::pow(Type::prod(Type::Integer, Type::Integer));
        assert_eq!(t.to_rodin_canonical(), "ℙ(ℤ×ℤ)");
    }

    #[test]
    fn relation_constructor() {
        // relation(α, β) is ℙ(α×β) — equal to the primitive spelling and
        // rendering the same canonical string.
        let r = Type::relation(Type::Integer, Type::GivenSet("S".into()));
        assert_eq!(
            r,
            Type::pow(Type::prod(Type::Integer, Type::GivenSet("S".into())))
        );
        assert_eq!(r.to_rodin_canonical(), "ℙ(ℤ×S)");
    }

    #[test]
    fn into_relation_roundtrip_and_rejects_non_relations() {
        // relation/into_relation are inverses.
        let (a, b) = (Type::Integer, Type::GivenSet("S".into()));
        assert_eq!(
            Type::relation(a.clone(), b.clone()).into_relation(),
            Some((a, b))
        );
        // A powerset of a non-product (ℙ(ℤ)) and a bare type both yield None.
        assert_eq!(Type::pow(Type::Integer).into_relation(), None);
        assert_eq!(Type::Integer.into_relation(), None);
    }
}
