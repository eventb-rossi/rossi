//! Proof confidence levels.

/// The confidence attached to a proof rule or a whole proof tree.
///
/// The scale: a closed node carries its rule's confidence
/// and a tree's confidence is the minimum over its nodes, so any
/// uncertain step caps the whole proof. The named constants bound the
/// meaningful ranges — discharged is `(REVIEWED_MAX, DISCHARGED_MAX]`,
/// reviewed `(UNCERTAIN_MAX, REVIEWED_MAX]`, uncertain
/// `(PENDING, UNCERTAIN_MAX]` — and [`Confidence::UNATTEMPTED`] marks
/// a proof tree that was never worked on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Confidence(pub i32);

impl Confidence {
    /// A proof tree with an open root and no comment.
    pub const UNATTEMPTED: Confidence = Confidence(-99);
    /// An open proof tree node.
    pub const PENDING: Confidence = Confidence(0);
    /// Upper bound of the uncertain range: an unknown reasoner or a
    /// stale reasoner version.
    pub const UNCERTAIN_MAX: Confidence = Confidence(100);
    /// Upper bound of the reviewed range: accepted by a human, not a
    /// prover.
    pub const REVIEWED_MAX: Confidence = Confidence(500);
    /// Upper bound of the scale: fully discharged.
    pub const DISCHARGED_MAX: Confidence = Confidence(1000);

    /// Classifies a raw confidence into its reporting bucket,
    /// Whether a recorded confidence marks a really attempted proof —
    /// The status-update revival threshold: strictly above
    /// [`Confidence::UNATTEMPTED`]. Deliberately different from
    /// [`Confidence::classify`]'s unattempted range (every negative):
    /// the `(-99, 0)` zone is attempted-but-uncertain here.
    pub fn is_attempted(confidence: Option<i64>) -> bool {
        confidence.is_some_and(|c| c > i64::from(Self::UNATTEMPTED.0))
    }

    /// eventb-checker's thresholds over the scale above: `None` or
    /// anything below [`Confidence::PENDING`] reads as unattempted.
    pub fn classify(confidence: Option<i64>) -> Bucket {
        match confidence {
            None => Bucket::Unattempted,
            Some(c) if c > i64::from(Self::REVIEWED_MAX.0) => Bucket::Discharged,
            Some(c) if c > i64::from(Self::UNCERTAIN_MAX.0) => Bucket::Reviewed,
            Some(c) if c >= i64::from(Self::PENDING.0) => Bucket::Pending,
            Some(_) => Bucket::Unattempted,
        }
    }
}

/// The reporting bucket a confidence value falls into — the result of
/// [`Confidence::classify`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    Discharged,
    Reviewed,
    Pending,
    Unattempted,
}

impl Bucket {
    /// The word this bucket is reported under, in every format.
    ///
    /// A proof report, a status summary and an agent-facing document all
    /// name a bucket, and a reader comparing two of them should not find
    /// different words for the same verdict, so they share one spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Bucket::Discharged => "discharged",
            Bucket::Reviewed => "reviewed",
            Bucket::Pending => "pending",
            Bucket::Unattempted => "unattempted",
        }
    }
}

impl std::fmt::Display for Bucket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::{Bucket, Confidence};

    #[test]
    fn classify_matches_eventb_checker_thresholds() {
        assert_eq!(Confidence::classify(Some(1000)), Bucket::Discharged);
        assert_eq!(Confidence::classify(Some(501)), Bucket::Discharged);
        assert_eq!(Confidence::classify(Some(500)), Bucket::Reviewed);
        assert_eq!(Confidence::classify(Some(101)), Bucket::Reviewed);
        assert_eq!(Confidence::classify(Some(100)), Bucket::Pending);
        assert_eq!(Confidence::classify(Some(0)), Bucket::Pending);
        // The whole negative range is unattempted, including the zone
        // strictly between UNATTEMPTED and PENDING — a raw `-99` bound
        // once misread it as pending.
        assert_eq!(Confidence::classify(Some(-1)), Bucket::Unattempted);
        assert_eq!(Confidence::classify(Some(-98)), Bucket::Unattempted);
        assert_eq!(
            Confidence::classify(Some(i64::from(Confidence::UNATTEMPTED.0))),
            Bucket::Unattempted
        );
        assert_eq!(Confidence::classify(Some(-100)), Bucket::Unattempted);
        assert_eq!(Confidence::classify(None), Bucket::Unattempted);
    }

    #[test]
    fn ordering_follows_the_scale() {
        assert!(Confidence::UNATTEMPTED < Confidence::PENDING);
        assert!(Confidence::PENDING < Confidence::UNCERTAIN_MAX);
        assert!(Confidence::UNCERTAIN_MAX < Confidence::REVIEWED_MAX);
        assert!(Confidence::REVIEWED_MAX < Confidence::DISCHARGED_MAX);
        // Aggregation over a tree is plain `min`.
        assert_eq!(
            Confidence(750).min(Confidence::DISCHARGED_MAX),
            Confidence(750)
        );
    }
}
