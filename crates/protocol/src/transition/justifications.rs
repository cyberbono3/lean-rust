//! Typed working view over the on-state per-target-root vote bitmap.
//!
//! On `State` the per-target-root vote tally is stored as a parallel pair:
//! `justifications_roots: Vec<Bytes32>` and a flat
//! `justifications_validators: Bitlist<…>` packing `len(roots) *
//! num_validators` bits. This module hydrates that pair into a
//! [`BTreeMap<Bytes32, Vec<bool>>`] for ergonomic per-vote mutation in
//! [`State::process_attestations`], and writes it back at the end of the
//! call.
//!
//! The `BTreeMap` ordering keeps the round-trip deterministic: the same
//! tally always serializes to the same `(roots, bits)` pair.

use std::collections::BTreeMap;

use types::{Bitlist, Bytes32};

use super::StateTransitionError;
use crate::state::{State, JUSTIFICATIONS_VALIDATORS_LIMIT};

/// Hydrated per-target-root vote tally for the duration of one
/// `process_attestations` call.
#[derive(Debug)]
pub(super) struct Justifications {
    /// Per-target-root vote vector, length = `num_validators` per entry.
    pub table: BTreeMap<Bytes32, Vec<bool>>,
    /// Cached `state.config.num_validators` as a `usize`.
    pub num_validators: usize,
}

impl Justifications {
    /// Hydrates the working view from `state.justifications_*`.
    ///
    /// # Errors
    /// - [`StateTransitionError::StateBoundExceeded`] when
    ///   `state.config.num_validators` does not fit in `usize`, or when the
    ///   flat bitlist length is not a multiple of `num_validators` (i.e. an
    ///   on-state invariant break).
    pub(super) fn from_state(state: &State) -> Result<Self, StateTransitionError> {
        let n = usize::try_from(state.config.num_validators).map_err(|_| {
            StateTransitionError::StateBoundExceeded {
                context: "num_validators",
            }
        })?;

        let mut table = BTreeMap::new();
        if n == 0 {
            return Ok(Self {
                table,
                num_validators: 0,
            });
        }

        let bits = &state.justifications_validators;
        let expected = state.justifications_roots.len().checked_mul(n).ok_or(
            StateTransitionError::StateBoundExceeded {
                context: "justifications_validators",
            },
        )?;
        if bits.len() != expected {
            return Err(StateTransitionError::StateBoundExceeded {
                context: "justifications_validators",
            });
        }

        for (i, root) in state.justifications_roots.iter().copied().enumerate() {
            let mut votes = vec![false; n];
            for (j, vote) in votes.iter_mut().enumerate() {
                *vote = bits.get(i * n + j).unwrap_or(false);
            }
            table.insert(root, votes);
        }
        Ok(Self {
            table,
            num_validators: n,
        })
    }

    /// Writes the working view back into `state.justifications_*`.
    ///
    /// `BTreeMap` iteration order is by key, so the resulting `(roots,
    /// bits)` pair is deterministic for any given `table`.
    ///
    /// # Errors
    /// - [`StateTransitionError::StateBoundExceeded`] when the flattened
    ///   bitlist would exceed [`JUSTIFICATIONS_VALIDATORS_LIMIT`].
    pub(super) fn write_back(self, state: &mut State) -> Result<(), StateTransitionError> {
        let n = self.num_validators;
        let total_bits =
            self.table
                .len()
                .checked_mul(n)
                .ok_or(StateTransitionError::StateBoundExceeded {
                    context: "justifications_validators",
                })?;
        if total_bits > JUSTIFICATIONS_VALIDATORS_LIMIT {
            return Err(StateTransitionError::StateBoundExceeded {
                context: "justifications_validators",
            });
        }

        let mut roots = Vec::with_capacity(self.table.len());
        let mut flat = Bitlist::<JUSTIFICATIONS_VALIDATORS_LIMIT>::with_length(total_bits)
            .map_err(|_| StateTransitionError::StateBoundExceeded {
                context: "justifications_validators",
            })?;

        for (i, (root, votes)) in self.table.into_iter().enumerate() {
            roots.push(root);
            for (j, voted) in votes.into_iter().enumerate() {
                if voted {
                    flat.set(i * n + j, true).map_err(|_| {
                        StateTransitionError::StateBoundExceeded {
                            context: "justifications_validators",
                        }
                    })?;
                }
            }
        }
        state.justifications_roots = roots;
        state.justifications_validators = flat;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::state::ProtocolConfig;

    fn state_with(num_validators: u64) -> State {
        State {
            config: ProtocolConfig {
                num_validators,
                genesis_time: 0,
            },
            ..State::default()
        }
    }

    #[test]
    fn empty_state_round_trips() {
        let state = state_with(4);
        let view = Justifications::from_state(&state).unwrap();
        assert_eq!(view.num_validators, 4);
        assert!(view.table.is_empty());

        let mut state2 = state_with(4);
        view.write_back(&mut state2).unwrap();
        assert!(state2.justifications_roots.is_empty());
        assert_eq!(state2.justifications_validators.len(), 0);
    }

    #[test]
    fn round_trip_preserves_votes_in_canonical_order() {
        let mut state = state_with(3);
        let mut view = Justifications {
            table: BTreeMap::new(),
            num_validators: 3,
        };
        view.table
            .insert(Bytes32::new([0x22; 32]), vec![true, false, true]);
        view.table
            .insert(Bytes32::new([0x11; 32]), vec![false, true, false]);

        view.write_back(&mut state).unwrap();

        // BTreeMap orders by key — 0x11 root precedes 0x22.
        assert_eq!(
            state.justifications_roots,
            vec![Bytes32::new([0x11; 32]), Bytes32::new([0x22; 32])]
        );
        assert_eq!(state.justifications_validators.len(), 6);
        // 0x11 chunk: [false, true, false] → bits 0,1,2
        assert_eq!(state.justifications_validators.get(0), Some(false));
        assert_eq!(state.justifications_validators.get(1), Some(true));
        assert_eq!(state.justifications_validators.get(2), Some(false));
        // 0x22 chunk: [true, false, true] → bits 3,4,5
        assert_eq!(state.justifications_validators.get(3), Some(true));
        assert_eq!(state.justifications_validators.get(4), Some(false));
        assert_eq!(state.justifications_validators.get(5), Some(true));

        let view2 = Justifications::from_state(&state).unwrap();
        let map: Vec<(Bytes32, Vec<bool>)> = view2.table.into_iter().collect();
        assert_eq!(map.len(), 2);
        assert_eq!(map[0].0, Bytes32::new([0x11; 32]));
        assert_eq!(map[0].1, vec![false, true, false]);
        assert_eq!(map[1].0, Bytes32::new([0x22; 32]));
        assert_eq!(map[1].1, vec![true, false, true]);
    }

    #[test]
    fn rejects_inconsistent_flat_length() {
        let mut state = state_with(3);
        state.justifications_roots = vec![Bytes32::new([0xaa; 32])];
        // Set a bit at index 5 — that gives the flat bitlist live length 6,
        // not 3. from_state should reject the inconsistency.
        state.justifications_validators.set(5, true).unwrap();
        let err = Justifications::from_state(&state).unwrap_err();
        assert_eq!(
            err,
            StateTransitionError::StateBoundExceeded {
                context: "justifications_validators",
            }
        );
    }
}
