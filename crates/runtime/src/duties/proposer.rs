//! O(1) round-robin proposer lookup over the node's local validator set.
//!
//! The spec proposer for slot `s` is `s % num_validators`
//! (leanSpec `types/validator.py:24`, asserted in
//! `containers/state/state.py`). The previous scheduler computed this by
//! iterating the local validator slice and calling
//! [`protocol::is_proposer`] for each entry — O(N) per slot. At
//! mainnet validator-set sizes that linear scan is the hot path of
//! every tick.
//!
//! [`LocalProposers`] precomputes the local set as a [`HashSet`] so a
//! per-slot lookup is one modulo plus one hash probe — flat in the
//! validator-set size. The selection rule is byte-for-byte the spec
//! rule (`slot % num_validators`); no offset, cache, or shuffle is
//! introduced.

use std::collections::HashSet;

use protocol::{Slot, ValidatorIndex};

/// Precomputed local validator set + total registry size, supporting
/// O(1) proposer lookup per slot.
#[derive(Debug, Clone)]
pub struct LocalProposers {
    /// The validators this node owns, as a set for O(1) membership.
    local: HashSet<ValidatorIndex>,
    /// Total validators in the registry — the modulus of the
    /// round-robin rule.
    total_validators: u64,
}

impl LocalProposers {
    /// Builds the lookup from the local validator indices and the total
    /// registry size.
    #[must_use]
    pub fn new(local: impl IntoIterator<Item = ValidatorIndex>, total_validators: u64) -> Self {
        Self {
            local: local.into_iter().collect(),
            total_validators,
        }
    }

    /// Returns the local validator that proposes `slot`, or `None` when
    /// this node does not own the slot's proposer.
    ///
    /// The proposer index is `slot % total_validators` — the exact spec
    /// rule. `None` is returned when the registry is empty (modulo
    /// undefined) or the computed proposer is not in the local set.
    #[must_use]
    pub fn proposer_for_slot(&self, slot: Slot) -> Option<ValidatorIndex> {
        if self.total_validators == 0 {
            return None;
        }
        let proposer = ValidatorIndex::new(slot.get() % self.total_validators);
        self.local.contains(&proposer).then_some(proposer)
    }

    /// Iterates the local validator set (order unspecified). Lets the
    /// consensus driver's attester pass reuse this set instead of holding a
    /// separate validator slice.
    pub fn local(&self) -> impl Iterator<Item = ValidatorIndex> + '_ {
        self.local.iter().copied()
    }

    /// The local validators that should attest at `slot`: the whole local set,
    /// minus the slot's proposer when this node owns it. Order unspecified, as
    /// for [`Self::local`].
    ///
    /// The proposer is excluded because it already signed AND locally
    /// re-imported its own attestation during block production (the block
    /// carries that signed attestation), so attesting again here would sign the
    /// SAME `(validator, slot)` twice at epoch = slot — a leanSig one-time-key
    /// reuse. Callers get the correct attester set without having to know that
    /// rule.
    ///
    /// `None` from [`Self::proposer_for_slot`] means this node does not own the
    /// slot's proposer, so nothing is excluded.
    ///
    /// # Example
    /// ```
    /// use protocol::{Slot, ValidatorIndex};
    /// use runtime::duties::LocalProposers;
    ///
    /// // This node owns validators 0 and 1 of a 4-validator registry.
    /// let proposers = LocalProposers::new([ValidatorIndex::new(0), ValidatorIndex::new(1)], 4);
    ///
    /// // Slot 1's proposer is validator 1 (`1 % 4`) and IS local, so it is
    /// // excluded and only validator 0 attests.
    /// let attesters: Vec<_> = proposers.attesters_for_slot(Slot::new(1)).collect();
    /// assert_eq!(attesters, [ValidatorIndex::new(0)]);
    ///
    /// // Slot 2's proposer is validator 2 — NOT local, so nothing is excluded
    /// // and both local validators attest. (Sorted: iteration order is
    /// // unspecified.)
    /// let mut attesters: Vec<_> = proposers.attesters_for_slot(Slot::new(2)).collect();
    /// attesters.sort();
    /// assert_eq!(attesters, [ValidatorIndex::new(0), ValidatorIndex::new(1)]);
    /// ```
    pub fn attesters_for_slot(&self, slot: Slot) -> impl Iterator<Item = ValidatorIndex> + '_ {
        let proposer = self.proposer_for_slot(slot);
        self.local()
            .filter(move |&validator| Some(validator) != proposer)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use protocol::is_proposer;

    #[test]
    fn matches_spec_rule_when_local_owns_all() {
        // Local set == full registry: proposer_for_slot must agree with
        // the spec `is_proposer` rule for every slot.
        let total = 30;
        let all: Vec<_> = (0..total).map(ValidatorIndex::new).collect();
        let proposers = LocalProposers::new(all.iter().copied(), total);
        for s in 0..200 {
            let slot = Slot::new(s);
            let want = ValidatorIndex::new(s % total);
            assert_eq!(proposers.proposer_for_slot(slot), Some(want));
            assert!(is_proposer(want, slot, total).unwrap());
        }
    }

    #[test]
    fn returns_none_when_proposer_not_local() {
        // Local owns only even indices; odd-proposer slots return None.
        let total = 10;
        let evens: Vec<_> = (0..total)
            .filter(|i| i % 2 == 0)
            .map(ValidatorIndex::new)
            .collect();
        let proposers = LocalProposers::new(evens, total);
        // slot 3 -> proposer 3 (odd, not local).
        assert_eq!(proposers.proposer_for_slot(Slot::new(3)), None);
        // slot 4 -> proposer 4 (even, local).
        assert_eq!(
            proposers.proposer_for_slot(Slot::new(4)),
            Some(ValidatorIndex::new(4))
        );
    }

    #[test]
    fn parity_with_is_proposer_over_partial_local_set() {
        // For a partial local set, proposer_for_slot returns Some(v)
        // exactly when `is_proposer(v, slot, total)` AND v is local.
        let total = 30;
        let local: Vec<_> = [0_u64, 3, 6, 9, 12]
            .into_iter()
            .map(ValidatorIndex::new)
            .collect();
        let local_set: HashSet<_> = local.iter().copied().collect();
        let proposers = LocalProposers::new(local, total);
        for s in 0..300 {
            let slot = Slot::new(s);
            let spec_proposer = ValidatorIndex::new(s % total);
            let want = local_set.contains(&spec_proposer).then_some(spec_proposer);
            assert_eq!(proposers.proposer_for_slot(slot), want, "slot {s}");
        }
    }

    #[test]
    fn empty_registry_yields_none() {
        let proposers = LocalProposers::new([ValidatorIndex::new(0)], 0);
        assert_eq!(proposers.proposer_for_slot(Slot::new(0)), None);
    }

    // --- Attester set (proposer double-sign resolution) --------------------

    #[test]
    fn attesters_exclude_the_slot_proposer() {
        // Two local validators out of four total; whichever is the slot's
        // proposer must be excluded from the attest pass so it does not sign its
        // own attestation twice at the same epoch (one-time-key reuse) — it
        // already signed + re-imported that vote inside `produce_block`.
        let proposers = LocalProposers::new([ValidatorIndex::new(0), ValidatorIndex::new(1)], 4);
        for slot in 0..8u64 {
            let s = Slot::new(slot);
            let proposer = proposers.proposer_for_slot(s);
            let attesters: Vec<ValidatorIndex> = proposers.attesters_for_slot(s).collect();

            if let Some(p) = proposer {
                assert!(
                    !attesters.contains(&p),
                    "slot {slot}: proposer {p:?} must be skipped in the attest pass",
                );
            }
            for validator in proposers.local() {
                if Some(validator) != proposer {
                    assert!(
                        attesters.contains(&validator),
                        "slot {slot}: non-proposer {validator:?} must still attest",
                    );
                }
            }
        }
    }
}
