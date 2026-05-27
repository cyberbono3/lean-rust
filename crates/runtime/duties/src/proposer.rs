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

    /// Number of local validators tracked. Useful for diagnostics.
    #[must_use]
    pub fn local_len(&self) -> usize {
        self.local.len()
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
}
