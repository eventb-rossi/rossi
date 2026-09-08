//! Rodin constant names for formula tags.
//!
//! Every node in the dump carries both its numeric `tag` and an `op` naming
//! it. The number is the machine-readable identity and matches Rodin's
//! `Formula` constants exactly; the name is what makes a document readable
//! and greppable, and it is spelled as Rodin spells the constant.
//!
//! The tables run forward, from the operator value to its name, rather than
//! backward from a tag. Forward is total, so no caller has to handle a
//! missing name; the compiler checks that every operator is covered, which no
//! test over a tag range can do; and it sidesteps the deliberate gap in the
//! unary expression range, where three tags are reserved for spellings this
//! model does not build.

use rossi::formula::tag::{
    AssocExprOp, AssocPredOp, AtomicOp, BinaryExprOp, BinaryPredOp, LiteralPredOp, QuantExprOp,
    QuantPredOp, RelationalOp, UnaryExprOp,
};

pub(crate) fn relational(op: RelationalOp) -> &'static str {
    match op {
        RelationalOp::Equal => "EQUAL",
        RelationalOp::NotEqual => "NOTEQUAL",
        RelationalOp::Lt => "LT",
        RelationalOp::Le => "LE",
        RelationalOp::Gt => "GT",
        RelationalOp::Ge => "GE",
        RelationalOp::In => "IN",
        RelationalOp::NotIn => "NOTIN",
        RelationalOp::Subset => "SUBSET",
        RelationalOp::NotSubset => "NOTSUBSET",
        RelationalOp::SubsetEq => "SUBSETEQ",
        RelationalOp::NotSubsetEq => "NOTSUBSETEQ",
    }
}

pub(crate) fn binary_expr(op: BinaryExprOp) -> &'static str {
    match op {
        BinaryExprOp::Mapsto => "MAPSTO",
        BinaryExprOp::Rel => "REL",
        BinaryExprOp::TRel => "TREL",
        BinaryExprOp::SRel => "SREL",
        BinaryExprOp::STRel => "STREL",
        BinaryExprOp::PFun => "PFUN",
        BinaryExprOp::TFun => "TFUN",
        BinaryExprOp::PInj => "PINJ",
        BinaryExprOp::TInj => "TINJ",
        BinaryExprOp::PSur => "PSUR",
        BinaryExprOp::TSur => "TSUR",
        BinaryExprOp::TBij => "TBIJ",
        BinaryExprOp::SetMinus => "SETMINUS",
        BinaryExprOp::CProd => "CPROD",
        BinaryExprOp::DProd => "DPROD",
        BinaryExprOp::PProd => "PPROD",
        BinaryExprOp::DomRes => "DOMRES",
        BinaryExprOp::DomSub => "DOMSUB",
        BinaryExprOp::RanRes => "RANRES",
        BinaryExprOp::RanSub => "RANSUB",
        BinaryExprOp::UpTo => "UPTO",
        BinaryExprOp::Minus => "MINUS",
        BinaryExprOp::Div => "DIV",
        BinaryExprOp::Mod => "MOD",
        BinaryExprOp::Expn => "EXPN",
        BinaryExprOp::FunImage => "FUNIMAGE",
        BinaryExprOp::RelImage => "RELIMAGE",
    }
}

pub(crate) fn binary_pred(op: BinaryPredOp) -> &'static str {
    match op {
        BinaryPredOp::LImp => "LIMP",
        BinaryPredOp::LEqv => "LEQV",
    }
}

pub(crate) fn assoc_expr(op: AssocExprOp) -> &'static str {
    match op {
        AssocExprOp::BUnion => "BUNION",
        AssocExprOp::BInter => "BINTER",
        AssocExprOp::BComp => "BCOMP",
        AssocExprOp::FComp => "FCOMP",
        AssocExprOp::Ovr => "OVR",
        AssocExprOp::Plus => "PLUS",
        AssocExprOp::Mul => "MUL",
    }
}

pub(crate) fn assoc_pred(op: AssocPredOp) -> &'static str {
    match op {
        AssocPredOp::LAnd => "LAND",
        AssocPredOp::LOr => "LOR",
    }
}

pub(crate) fn atomic(op: AtomicOp) -> &'static str {
    match op {
        AtomicOp::Integer => "INTEGER",
        AtomicOp::Natural => "NATURAL",
        AtomicOp::Natural1 => "NATURAL1",
        AtomicOp::Bool => "BOOL",
        AtomicOp::True => "TRUE",
        AtomicOp::False => "FALSE",
        AtomicOp::EmptySet => "EMPTYSET",
        AtomicOp::KPred => "KPRED",
        AtomicOp::KSucc => "KSUCC",
        AtomicOp::KPrj1Gen => "KPRJ1_GEN",
        AtomicOp::KPrj2Gen => "KPRJ2_GEN",
        AtomicOp::KIdGen => "KID_GEN",
    }
}

pub(crate) fn literal_pred(op: LiteralPredOp) -> &'static str {
    match op {
        LiteralPredOp::BTrue => "BTRUE",
        LiteralPredOp::BFalse => "BFALSE",
    }
}

pub(crate) fn unary_expr(op: UnaryExprOp) -> &'static str {
    match op {
        UnaryExprOp::KCard => "KCARD",
        UnaryExprOp::Pow => "POW",
        UnaryExprOp::Pow1 => "POW1",
        UnaryExprOp::KUnion => "KUNION",
        UnaryExprOp::KInter => "KINTER",
        UnaryExprOp::KDom => "KDOM",
        UnaryExprOp::KRan => "KRAN",
        UnaryExprOp::KMin => "KMIN",
        UnaryExprOp::KMax => "KMAX",
        UnaryExprOp::Converse => "CONVERSE",
        UnaryExprOp::UnMinus => "UNMINUS",
    }
}

pub(crate) fn quant_expr(op: QuantExprOp) -> &'static str {
    match op {
        QuantExprOp::QUnion => "QUNION",
        QuantExprOp::QInter => "QINTER",
        QuantExprOp::CSet => "CSET",
    }
}

pub(crate) fn quant_pred(op: QuantPredOp) -> &'static str {
    match op {
        QuantPredOp::Forall => "FORALL",
        QuantPredOp::Exists => "EXISTS",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Every operator in a range must have its own name, and the name must
    /// look like a Rodin constant. A duplicate would make two different
    /// operators indistinguishable to a consumer reading `op`.
    fn names_are_distinct_and_shaped(names: &[&'static str]) {
        let unique: BTreeSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "duplicate name in {names:?}");
        for name in names {
            assert!(!name.is_empty());
            assert!(
                name.bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'),
                "{name} is not spelled like a Rodin constant"
            );
        }
    }

    #[test]
    fn every_operator_has_a_distinct_rodin_name() {
        names_are_distinct_and_shaped(&RelationalOp::ALL.map(relational));
        names_are_distinct_and_shaped(&BinaryExprOp::ALL.map(binary_expr));
        names_are_distinct_and_shaped(&BinaryPredOp::ALL.map(binary_pred));
        names_are_distinct_and_shaped(&AssocExprOp::ALL.map(assoc_expr));
        names_are_distinct_and_shaped(&AssocPredOp::ALL.map(assoc_pred));
        names_are_distinct_and_shaped(&AtomicOp::ALL.map(atomic));
        names_are_distinct_and_shaped(&LiteralPredOp::ALL.map(literal_pred));
        names_are_distinct_and_shaped(&UnaryExprOp::ALL.map(unary_expr));
        names_are_distinct_and_shaped(&QuantExprOp::ALL.map(quant_expr));
        names_are_distinct_and_shaped(&QuantPredOp::ALL.map(quant_pred));
    }

    /// The pairing of name to number is the format's contract with any
    /// consumer built on Rodin, so a few anchors from each range are pinned
    /// here. If a tag ever moves, this fails before a golden does.
    #[test]
    fn names_are_pinned_to_rodin_tag_numbers() {
        let pinned: &[(&str, u32)] = &[
            (relational(RelationalOp::Equal), 101),
            (relational(RelationalOp::NotSubsetEq), 112),
            (binary_expr(BinaryExprOp::Mapsto), 201),
            (binary_expr(BinaryExprOp::RelImage), 227),
            (binary_pred(BinaryPredOp::LImp), 251),
            (binary_pred(BinaryPredOp::LEqv), 252),
            (assoc_expr(AssocExprOp::BUnion), 301),
            (assoc_expr(AssocExprOp::Ovr), 305),
            (assoc_pred(AssocPredOp::LAnd), 351),
            (assoc_pred(AssocPredOp::LOr), 352),
            (atomic(AtomicOp::Integer), 401),
            (atomic(AtomicOp::EmptySet), 407),
            (atomic(AtomicOp::KIdGen), 412),
            (literal_pred(LiteralPredOp::BTrue), 610),
            (unary_expr(UnaryExprOp::KCard), 751),
            // The three deprecated spellings occupy 758 to 760, so the
            // minimum lands at 761 rather than continuing from 757.
            (unary_expr(UnaryExprOp::KMin), 761),
            (unary_expr(UnaryExprOp::UnMinus), 764),
            (quant_expr(QuantExprOp::QUnion), 801),
            (quant_expr(QuantExprOp::CSet), 803),
            (quant_pred(QuantPredOp::Forall), 851),
            (quant_pred(QuantPredOp::Exists), 852),
        ];
        let tags: &[u32] = &[
            RelationalOp::Equal.tag(),
            RelationalOp::NotSubsetEq.tag(),
            BinaryExprOp::Mapsto.tag(),
            BinaryExprOp::RelImage.tag(),
            BinaryPredOp::LImp.tag(),
            BinaryPredOp::LEqv.tag(),
            AssocExprOp::BUnion.tag(),
            AssocExprOp::Ovr.tag(),
            AssocPredOp::LAnd.tag(),
            AssocPredOp::LOr.tag(),
            AtomicOp::Integer.tag(),
            AtomicOp::EmptySet.tag(),
            AtomicOp::KIdGen.tag(),
            LiteralPredOp::BTrue.tag(),
            UnaryExprOp::KCard.tag(),
            UnaryExprOp::KMin.tag(),
            UnaryExprOp::UnMinus.tag(),
            QuantExprOp::QUnion.tag(),
            QuantExprOp::CSet.tag(),
            QuantPredOp::Forall.tag(),
            QuantPredOp::Exists.tag(),
        ];
        for ((name, expected), actual) in pinned.iter().zip(tags) {
            assert_eq!(*expected, *actual, "{name} moved off tag {expected}");
        }
    }
}
