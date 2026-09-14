//! Proof-obligation natures: what each generated sequent asks to prove.
//!
//! The description strings are written verbatim into the `poDesc`
//! attribute; provers and status tools match on them, so they are part
//! of the file format (including the historical double space in the
//! invariant natures).

/// The nature of a proof obligation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nature {
    ActionFeasibility,
    ActionSimulation,
    ActionWellDefinedness,
    AxiomWellDefinedness,
    CommonVariableEquality,
    EventVariant,
    EventNaturalNumberVariant,
    GuardStrengtheningMerge,
    GuardStrengtheningSplit,
    GuardWellDefinedness,
    InvariantEstablishment,
    InvariantPreservation,
    InvariantWellDefinedness,
    Theorem,
    TheoremWellDefinedness,
    VariantFiniteness,
    VariantWellDefinedness,
    WitnessFeasibility,
    WitnessWellDefinedness,
}

impl Nature {
    /// Every nature, in declaration order.
    pub const ALL: [Nature; 19] = [
        Nature::ActionFeasibility,
        Nature::ActionSimulation,
        Nature::ActionWellDefinedness,
        Nature::AxiomWellDefinedness,
        Nature::CommonVariableEquality,
        Nature::EventVariant,
        Nature::EventNaturalNumberVariant,
        Nature::GuardStrengtheningMerge,
        Nature::GuardStrengtheningSplit,
        Nature::GuardWellDefinedness,
        Nature::InvariantEstablishment,
        Nature::InvariantPreservation,
        Nature::InvariantWellDefinedness,
        Nature::Theorem,
        Nature::TheoremWellDefinedness,
        Nature::VariantFiniteness,
        Nature::VariantWellDefinedness,
        Nature::WitnessFeasibility,
        Nature::WitnessWellDefinedness,
    ];

    /// The nature a `poDesc` attribute value names, when it is one of
    /// the generator's.
    #[must_use]
    pub fn from_description(description: &str) -> Option<Nature> {
        Nature::ALL
            .into_iter()
            .find(|nature| nature.description() == description)
    }

    /// The stable name of this nature, as a report or a filter spells
    /// it. Distinct from [`Nature::description`], which is the wording
    /// the file format carries and provers match on.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Nature::ActionFeasibility => "ActionFeasibility",
            Nature::ActionSimulation => "ActionSimulation",
            Nature::ActionWellDefinedness => "ActionWellDefinedness",
            Nature::AxiomWellDefinedness => "AxiomWellDefinedness",
            Nature::CommonVariableEquality => "CommonVariableEquality",
            Nature::EventVariant => "EventVariant",
            Nature::EventNaturalNumberVariant => "EventNaturalNumberVariant",
            Nature::GuardStrengtheningMerge => "GuardStrengtheningMerge",
            Nature::GuardStrengtheningSplit => "GuardStrengtheningSplit",
            Nature::GuardWellDefinedness => "GuardWellDefinedness",
            Nature::InvariantEstablishment => "InvariantEstablishment",
            Nature::InvariantPreservation => "InvariantPreservation",
            Nature::InvariantWellDefinedness => "InvariantWellDefinedness",
            Nature::Theorem => "Theorem",
            Nature::TheoremWellDefinedness => "TheoremWellDefinedness",
            Nature::VariantFiniteness => "VariantFiniteness",
            Nature::VariantWellDefinedness => "VariantWellDefinedness",
            Nature::WitnessFeasibility => "WitnessFeasibility",
            Nature::WitnessWellDefinedness => "WitnessWellDefinedness",
        }
    }

    /// The nature `name` spells, when it spells one.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Nature> {
        Nature::ALL.into_iter().find(|nature| nature.name() == name)
    }

    /// The `poDesc` attribute value.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Nature::ActionFeasibility => "Feasibility of action",
            Nature::ActionSimulation => "Action simulation",
            Nature::ActionWellDefinedness => "Well-definedness of action",
            Nature::AxiomWellDefinedness => "Well-definedness of Axiom",
            Nature::CommonVariableEquality => "Equality of common variables",
            Nature::EventVariant => "Variant of event",
            Nature::EventNaturalNumberVariant => "Natural number variant of event",
            Nature::GuardStrengtheningMerge => "Guard strengthening (merge)",
            Nature::GuardStrengtheningSplit => "Guard strengthening (split)",
            Nature::GuardWellDefinedness => "Well-definedness of Guard",
            Nature::InvariantEstablishment => "Invariant  establishment",
            Nature::InvariantPreservation => "Invariant  preservation",
            Nature::InvariantWellDefinedness => "Well-definedness of Invariant",
            Nature::Theorem => "Theorem",
            Nature::TheoremWellDefinedness => "Well-definedness of Theorem",
            Nature::VariantFiniteness => "Finiteness of variant",
            Nature::VariantWellDefinedness => "Well-definedness of variant",
            Nature::WitnessFeasibility => "Feasibility of witness",
            Nature::WitnessWellDefinedness => "Well-definedness of witness",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_finds_its_nature_back() {
        for nature in Nature::ALL {
            assert_eq!(Nature::from_name(nature.name()), Some(nature));
        }
        assert_eq!(Nature::from_name("InvariantPreservation "), None);
    }

    #[test]
    fn every_description_names_its_nature_back() {
        for nature in Nature::ALL {
            assert_eq!(Nature::from_description(nature.description()), Some(nature));
        }
        assert_eq!(Nature::from_description("Invariant preservation"), None);
    }
}
