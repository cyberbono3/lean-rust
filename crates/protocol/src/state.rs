//! Consensus [`State`] container plus its inner [`ProtocolConfig`].
//!
//! The native lean-rust state SSZ container declares 9 fields in order:
//!
//! 1. `config: ProtocolConfig` — fixed (16-byte container).
//! 2. `slot: Slot` — fixed 8 bytes.
//! 3. `latest_block_header: BlockHeader` — fixed 112 bytes.
//! 4. `latest_justified: Checkpoint` — fixed 40 bytes.
//! 5. `latest_finalized: Checkpoint` — fixed 40 bytes.
//! 6. `historical_block_hashes: List[Bytes32, HISTORICAL_ROOTS_LIMIT]` —
//!    variable.
//! 7. `justified_slots: Bitlist[HISTORICAL_ROOTS_LIMIT]` — variable.
//! 8. `justifications_roots: List[Bytes32, HISTORICAL_ROOTS_LIMIT]` —
//!    variable.
//! 9. `justifications_validators: Bitlist[JUSTIFICATIONS_VALIDATORS_LIMIT]`
//!    — variable.
//!
//! Field bounds are pinned to the [`config::DEVNET_CONFIG`] caps. The four
//! variable-length fields each contribute a 4-byte offset to the fixed
//! portion ([`STATE_FIXED_PART_LEN`] = 232 bytes).
//!
//! The hash-tree-root commits to all nine fields in this order; the
//! cross-client compatibility of that shape (and the genesis-interop
//! decoder for the compact form) lives in [`crate::ream`].

// Retained construction sites for the deprecated `Bytes4000` placeholder.
// Scoped to this file so unrelated deprecations elsewhere in the crate are
// still surfaced; removed when this file's last site moves to `Signature`.
#![allow(deprecated)]

use std::collections::BTreeMap;

use ssz::merkleize::merkleize;
use ssz::{Decode, DecodeError, Encode, HashTreeRoot};
use types::{Bitlist, Bytes32};

use crate::block::{Block, BlockHeader, SignedBlock};
use crate::checkpoint::Checkpoint;
use crate::error::{AttSlotKind, StateTransitionError};
use crate::internal::{
    bitlist_hash_tree_root, decode_bytes32_list, encode_bytes32_list, ensure_len,
    list_hash_tree_root, read_fixed, read_offset, u64_chunk, write_offset, BLOCK_HEADER_LEN,
    BYTES32_LEN, BYTES_PER_LENGTH_OFFSET, CHECKPOINT_LEN, SLOT_LEN, U64_LEN,
};
use crate::slot::Slot;
use crate::validator::is_proposer;
use crate::vote::SignedVote;

/// Maximum number of historical block roots retained in the state.
///
/// Pinned to [`config::DEVNET_CONFIG::historical_roots_limit`] (`262_144` on
/// devnet0).
#[allow(clippy::cast_possible_truncation)]
pub const HISTORICAL_ROOTS_LIMIT: usize = config::DEVNET_CONFIG.historical_roots_limit as usize;

/// Maximum validator-registry size used to bound per-root vote bitlists.
///
/// Pinned to [`config::DEVNET_CONFIG::validator_registry_limit`] (`4_096` on
/// devnet0).
#[allow(clippy::cast_possible_truncation)]
pub const VALIDATOR_REGISTRY_LIMIT: usize = config::DEVNET_CONFIG.validator_registry_limit as usize;

/// Bound on the flattened validator-vote bitlist:
/// [`HISTORICAL_ROOTS_LIMIT`] × [`VALIDATOR_REGISTRY_LIMIT`].
///
/// Equals `262_144 * 4_096 = 1_073_741_824` on devnet0.
pub const JUSTIFICATIONS_VALIDATORS_LIMIT: usize =
    HISTORICAL_ROOTS_LIMIT * VALIDATOR_REGISTRY_LIMIT;

// `pub(crate)` so the sibling [`crate::ream`] module can reuse them; both
// are also consumed by `STATE_FIXED_PART_LEN` below.
pub(crate) const PROTOCOL_CONFIG_SSZ_LEN: usize = 2 * U64_LEN; // 16
pub(crate) const STATE_VARIABLE_FIELD_COUNT: usize = 4;

/// Length of the fixed portion of a [`State`] (5 fixed fields plus 4 offsets
/// for the variable-length tails).
pub const STATE_FIXED_PART_LEN: usize = PROTOCOL_CONFIG_SSZ_LEN
    + SLOT_LEN
    + BLOCK_HEADER_LEN
    + CHECKPOINT_LEN
    + CHECKPOINT_LEN
    + STATE_VARIABLE_FIELD_COUNT * BYTES_PER_LENGTH_OFFSET; // 232

// =====================================================================
// ProtocolConfig (the inner `config` field of State)
// =====================================================================

/// In-state runtime parameters carried by the consensus [`State`].
///
/// Two fixed-size `u64` fields → 16-byte SSZ payload. Distinct from the
/// chain-wide [`config::Config`] preset: this container records the
/// validator-set size and chain genesis time committed to the state
/// hash-tree-root.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ProtocolConfig {
    /// Number of active validators tracked by the chain.
    pub num_validators: u64,
    /// Unix timestamp (seconds) of chain genesis.
    pub genesis_time: u64,
}

impl Encode for ProtocolConfig {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        PROTOCOL_CONFIG_SSZ_LEN
    }

    fn ssz_bytes_len(&self) -> usize {
        PROTOCOL_CONFIG_SSZ_LEN
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        self.num_validators.ssz_append(buf);
        self.genesis_time.ssz_append(buf);
    }
}

impl Decode for ProtocolConfig {
    fn is_ssz_fixed_len() -> bool {
        true
    }

    fn ssz_fixed_len() -> usize {
        PROTOCOL_CONFIG_SSZ_LEN
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        ensure_len(bytes, PROTOCOL_CONFIG_SSZ_LEN)?;
        let mut c = 0;
        Ok(Self {
            num_validators: read_fixed::<u64>(bytes, &mut c)?,
            genesis_time: read_fixed::<u64>(bytes, &mut c)?,
        })
    }
}

impl HashTreeRoot for ProtocolConfig {
    fn hash_tree_root(&self) -> [u8; 32] {
        // 2 fields → width 2 → single hash_pair via merkleize.
        merkleize(&[u64_chunk(self.num_validators), u64_chunk(self.genesis_time)])
    }
}

// =====================================================================
// State
// =====================================================================

/// Consensus state container.
///
/// Variable-length SSZ container: the four list/bitlist tails follow the
/// fixed portion in declaration order, each addressed by a 4-byte offset
/// stored inline at its declaration position.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// In-state runtime parameters (validator-set size + genesis time).
    pub config: ProtocolConfig,
    /// Current slot of the state.
    pub slot: Slot,
    /// Latest [`BlockHeader`] applied to this state.
    pub latest_block_header: BlockHeader,
    /// Latest justified checkpoint.
    pub latest_justified: Checkpoint,
    /// Latest finalized checkpoint.
    pub latest_finalized: Checkpoint,
    /// Bounded list of historical block roots indexed by slot.
    pub historical_block_hashes: Vec<Bytes32>,
    /// Bounded bitlist marking which historical slots are justified.
    pub justified_slots: Bitlist<HISTORICAL_ROOTS_LIMIT>,
    /// Bounded list of roots whose per-validator vote bitlist is tracked.
    pub justifications_roots: Vec<Bytes32>,
    /// Flattened per-validator vote bitlist for [`Self::justifications_roots`].
    pub justifications_validators: Bitlist<JUSTIFICATIONS_VALIDATORS_LIMIT>,
}

impl State {
    /// Returns the four variable-length tail payloads encoded into their wire
    /// bytes, in declaration order.
    fn variable_tail_payloads(&self) -> [Vec<u8>; STATE_VARIABLE_FIELD_COUNT] {
        let mut historical_buf =
            Vec::with_capacity(self.historical_block_hashes.len() * BYTES32_LEN);
        encode_bytes32_list(&self.historical_block_hashes, &mut historical_buf);

        let mut roots_buf = Vec::with_capacity(self.justifications_roots.len() * BYTES32_LEN);
        encode_bytes32_list(&self.justifications_roots, &mut roots_buf);

        [
            historical_buf,
            self.justified_slots.as_bytes(),
            roots_buf,
            self.justifications_validators.as_bytes(),
        ]
    }
}

impl Encode for State {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn ssz_bytes_len(&self) -> usize {
        let tails = self.variable_tail_payloads();
        STATE_FIXED_PART_LEN + tails.iter().map(Vec::len).sum::<usize>()
    }

    fn ssz_append(&self, buf: &mut Vec<u8>) {
        let tails = self.variable_tail_payloads();
        let mut offset = STATE_FIXED_PART_LEN;

        // Fixed fields first.
        self.config.ssz_append(buf);
        self.slot.ssz_append(buf);
        self.latest_block_header.ssz_append(buf);
        self.latest_justified.ssz_append(buf);
        self.latest_finalized.ssz_append(buf);

        // Four offsets, one per variable-length tail. Offsets are absolute
        // byte positions from the start of the encoded container.
        for tail in &tails {
            write_offset(buf, offset);
            offset += tail.len();
        }

        // Variable tails appended in declaration order.
        for tail in &tails {
            buf.extend_from_slice(tail);
        }
    }
}

impl Decode for State {
    fn is_ssz_fixed_len() -> bool {
        false
    }

    fn from_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() < STATE_FIXED_PART_LEN {
            return Err(DecodeError::InvalidByteLength {
                len: bytes.len(),
                expected: STATE_FIXED_PART_LEN,
            });
        }
        let mut c = 0;
        let config = read_fixed::<ProtocolConfig>(bytes, &mut c)?;
        let slot = read_fixed::<Slot>(bytes, &mut c)?;
        let latest_block_header = read_fixed::<BlockHeader>(bytes, &mut c)?;
        let latest_justified = Checkpoint::from_ssz_bytes(&bytes[c..c + CHECKPOINT_LEN])?;
        c += CHECKPOINT_LEN;
        let latest_finalized = Checkpoint::from_ssz_bytes(&bytes[c..c + CHECKPOINT_LEN])?;
        c += CHECKPOINT_LEN;

        let mut offsets = [0_usize; STATE_VARIABLE_FIELD_COUNT];
        for offset in &mut offsets {
            *offset = read_offset(bytes, &mut c)?;
        }

        // First offset MUST equal the fixed-part length; subsequent offsets
        // MUST be non-decreasing and within the input slice.
        if offsets[0] != STATE_FIXED_PART_LEN {
            return Err(DecodeError::OffsetIntoFixedPortion(offsets[0]));
        }
        for pair in offsets.windows(2) {
            if pair[1] < pair[0] {
                return Err(DecodeError::OffsetsAreDecreasing(pair[1]));
            }
        }
        let last_offset = *offsets.last().unwrap_or(&STATE_FIXED_PART_LEN);
        if last_offset > bytes.len() {
            return Err(DecodeError::OffsetOutOfBounds(last_offset));
        }

        let tail_slice = |idx: usize| -> &[u8] {
            let start = offsets[idx];
            let end = if idx + 1 < STATE_VARIABLE_FIELD_COUNT {
                offsets[idx + 1]
            } else {
                bytes.len()
            };
            &bytes[start..end]
        };

        let historical_block_hashes = decode_bytes32_list(tail_slice(0), HISTORICAL_ROOTS_LIMIT)?;
        let justified_slots = Bitlist::<HISTORICAL_ROOTS_LIMIT>::from_bytes(tail_slice(1))
            .map_err(|err| DecodeError::BytesInvalid(format!("justified_slots: {err}")))?;
        let justifications_roots = decode_bytes32_list(tail_slice(2), HISTORICAL_ROOTS_LIMIT)?;
        let justifications_validators = Bitlist::<JUSTIFICATIONS_VALIDATORS_LIMIT>::from_bytes(
            tail_slice(3),
        )
        .map_err(|err| DecodeError::BytesInvalid(format!("justifications_validators: {err}")))?;

        Ok(Self {
            config,
            slot,
            latest_block_header,
            latest_justified,
            latest_finalized,
            historical_block_hashes,
            justified_slots,
            justifications_roots,
            justifications_validators,
        })
    }
}

impl HashTreeRoot for State {
    fn hash_tree_root(&self) -> [u8; 32] {
        // Native lean state root: 9 fields → merkleize width 16.
        merkleize(&[
            self.config.hash_tree_root(),
            self.slot.hash_tree_root(),
            self.latest_block_header.hash_tree_root(),
            self.latest_justified.hash_tree_root(),
            self.latest_finalized.hash_tree_root(),
            list_hash_tree_root(&self.historical_block_hashes, HISTORICAL_ROOTS_LIMIT),
            bitlist_hash_tree_root(&self.justified_slots),
            list_hash_tree_root(&self.justifications_roots, HISTORICAL_ROOTS_LIMIT),
            bitlist_hash_tree_root(&self.justifications_validators),
        ])
    }
}

// =====================================================================
// process_slot / process_slots
// =====================================================================

/// Maps [`Slot::advance`] (`Option<Slot>`) onto [`StateTransitionError::SlotOverflow`].
fn advance_slot(slot: Slot) -> Result<Slot, StateTransitionError> {
    slot.advance()
        .ok_or(StateTransitionError::SlotOverflow { slot })
}

impl State {
    /// Caches the pre-block state root into `latest_block_header` when block
    /// processing left the header's `state_root` as the all-zero sentinel.
    /// On any other input — including when no block has been applied since
    /// the previous slot — the state is left unchanged.
    ///
    /// # Errors
    /// Currently infallible. The `Result` return matches the consensus-spec
    /// `process_slot` signature and stays forward-compatible for future
    /// validation steps that may surface a [`StateTransitionError`] variant.
    pub fn process_slot(&mut self) -> Result<(), StateTransitionError> {
        if self.latest_block_header.state_root == Bytes32::zero() {
            self.latest_block_header.state_root = self.hash_tree_root().into();
        }

        Ok(())
    }

    /// Advances `self` slot-by-slot up to (but not past) `target_slot`.
    ///
    /// Each iteration runs [`State::process_slot`] then increments
    /// `self.slot` by one.
    ///
    /// # Errors
    /// - [`StateTransitionError::TargetSlotNotInFuture`] when
    ///   `target_slot <= self.slot`.
    /// - [`StateTransitionError::SlotOverflow`] when slot arithmetic would
    ///   exceed `u64::MAX`. Cannot fire once the future-target check
    ///   passes, but surfaced explicitly to keep the loop `unwrap`-free.
    pub fn process_slots(&mut self, target_slot: Slot) -> Result<(), StateTransitionError> {
        if target_slot <= self.slot {
            return Err(StateTransitionError::TargetSlotNotInFuture {
                current: self.slot,
                target: target_slot,
            });
        }
        let steps = target_slot.get() - self.slot.get();
        for _ in 0..steps {
            self.process_slot()?;
            self.slot = advance_slot(self.slot)?;
        }
        Ok(())
    }
}

// =====================================================================
// process_block_header
// =====================================================================

impl State {
    /// Validates `block` against `self` and commits its header-derived state.
    ///
    /// Mirrors the consensus-spec `process_block_header`. The method is
    /// transactional in spirit: every validation runs before any field on
    /// `self` is mutated, so an `Err` return leaves the state byte-equal to
    /// its pre-call value.
    ///
    /// # Errors
    /// - [`StateTransitionError::BlockSlotMismatch`] when `block.slot != self.slot`.
    /// - [`StateTransitionError::BlockOlderThanLatest`] when `block.slot <= self.latest_block_header.slot`.
    /// - [`StateTransitionError::IncorrectBlockProposer`] when
    ///   `block.proposer_index` is not the round-robin proposer for `self.slot`.
    /// - [`StateTransitionError::BlockParentRootMismatch`] when
    ///   `block.parent_root != hash_tree_root(self.latest_block_header)`.
    /// - [`StateTransitionError::StateBoundExceeded`] when the appended
    ///   parent root plus zero-padded empty slots would push
    ///   `historical_block_hashes` or `justified_slots` past their bounds.
    /// - [`StateTransitionError::Protocol`] forwarded from
    ///   [`is_proposer`] when `self.config.num_validators == 0`.
    pub fn process_block_header(&mut self, block: &Block) -> Result<(), StateTransitionError> {
        // -- Validation gate: cheap checks first, hash last. ----------------
        if block.slot != self.slot {
            return Err(StateTransitionError::BlockSlotMismatch {
                got: block.slot,
                want: self.slot,
            });
        }
        if block.slot <= self.latest_block_header.slot {
            return Err(StateTransitionError::BlockOlderThanLatest {
                slot: block.slot,
                latest: self.latest_block_header.slot,
            });
        }
        if !is_proposer(block.proposer_index, self.slot, self.config.num_validators)? {
            return Err(StateTransitionError::IncorrectBlockProposer {
                slot: self.slot,
                proposer: block.proposer_index,
            });
        }
        let parent_root: Bytes32 = self.latest_block_header.hash_tree_root().into();
        if block.parent_root != parent_root {
            return Err(StateTransitionError::BlockParentRootMismatch {
                slot: block.slot,
                got: block.parent_root,
                want: parent_root,
            });
        }

        // -- Derived values. ------------------------------------------------
        let body_root: Bytes32 = block.body.hash_tree_root().into();
        let was_genesis = self.latest_block_header.slot.is_zero();
        let prev_slot = self.latest_block_header.slot.get();
        // Safe: `block.slot > prev_slot` (validated above) ⇒ subtraction
        // cannot underflow; the result is a `u64` slot count.
        let empty_slots = block.slot.get() - prev_slot - 1;
        let empty_slots_usize =
            usize::try_from(empty_slots).map_err(|_| StateTransitionError::StateBoundExceeded {
                context: "historical_block_hashes",
            })?;
        let next_history_len = self
            .historical_block_hashes
            .len()
            .checked_add(1)
            .and_then(|n| n.checked_add(empty_slots_usize))
            .ok_or(StateTransitionError::StateBoundExceeded {
                context: "historical_block_hashes",
            })?;
        if next_history_len > HISTORICAL_ROOTS_LIMIT {
            return Err(StateTransitionError::StateBoundExceeded {
                context: "historical_block_hashes",
            });
        }

        // -- Commit. --------------------------------------------------------
        if was_genesis {
            self.latest_justified.root = parent_root;
            self.latest_finalized.root = parent_root;
        }

        let parent_idx = self.justified_slots.len();
        self.historical_block_hashes.push(parent_root);
        self.justified_slots
            .set(parent_idx, was_genesis)
            .map_err(|_| StateTransitionError::StateBoundExceeded {
                context: "justified_slots",
            })?;

        self.historical_block_hashes
            .extend(std::iter::repeat_n(Bytes32::zero(), empty_slots_usize));
        for _ in 0..empty_slots_usize {
            let idx = self.justified_slots.len();
            self.justified_slots.set(idx, false).map_err(|_| {
                StateTransitionError::StateBoundExceeded {
                    context: "justified_slots",
                }
            })?;
        }

        self.latest_block_header = BlockHeader {
            slot: block.slot,
            proposer_index: block.proposer_index,
            parent_root: block.parent_root,
            state_root: Bytes32::zero(),
            body_root,
        };
        Ok(())
    }
}

// =====================================================================
// process_attestations
// =====================================================================

/// Hydrated per-target-root vote tally for the duration of one
/// [`State::process_attestations`] call.
///
/// On `State` the per-target-root vote tally is stored as a parallel pair:
/// `justifications_roots: Vec<Bytes32>` and a flat
/// `justifications_validators: Bitlist<…>` packing `len(roots) *
/// num_validators` bits. This view hydrates that pair into a
/// [`BTreeMap<Bytes32, Vec<bool>>`] for ergonomic per-vote mutation, and
/// writes it back at the end of the call.
///
/// `BTreeMap` ordering keeps the round-trip deterministic: the same tally
/// always serializes to the same `(roots, bits)` pair.
#[derive(Debug)]
struct Justifications {
    /// Per-target-root vote vector, length = `num_validators` per entry.
    table: BTreeMap<Bytes32, Vec<bool>>,
    /// Cached `state.config.num_validators` as a `usize`.
    num_validators: usize,
}

impl TryFrom<&State> for Justifications {
    type Error = StateTransitionError;

    /// Hydrates the working view from `state.justifications_*`.
    ///
    /// Returns [`StateTransitionError::StateBoundExceeded`] when
    /// `state.config.num_validators` does not fit in `usize`, or when the
    /// flat bitlist length is not a multiple of `num_validators` (i.e. an
    /// on-state invariant break).
    fn try_from(state: &State) -> Result<Self, Self::Error> {
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
}

impl Justifications {
    /// Writes the working view back into `state.justifications_*`.
    ///
    /// `BTreeMap` iteration order is by key, so the resulting `(roots,
    /// bits)` pair is deterministic for any given `table`.
    fn write_back(self, state: &mut State) -> Result<(), StateTransitionError> {
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

/// Converts `slot` to a `usize` and validates `slot.get() < len`.
///
/// Both the `try_from` overflow path and the out-of-bounds path produce
/// [`StateTransitionError::AttestationSlotOutOfRange`] tagged with `kind`.
fn bounded_slot_index(
    slot: Slot,
    kind: AttSlotKind,
    len: usize,
) -> Result<usize, StateTransitionError> {
    usize::try_from(slot.get())
        .ok()
        .filter(|&i| i < len)
        .ok_or(StateTransitionError::AttestationSlotOutOfRange { kind, slot, len })
}

impl State {
    /// Applies `attestations` to `self` per the 3sf-mini consensus rules:
    ///
    /// - Each vote is recorded against its target root in the per-target-root
    ///   validator bitmap.
    /// - Once a 2/3 supermajority votes for the same target, the target slot
    ///   is justified and `latest_justified` updates.
    /// - If the target is the next valid justifiable slot after the source
    ///   (no other justifiable slot strictly between), the source is
    ///   finalized and `latest_finalized` updates.
    ///
    /// Range checks (out-of-range source/target slot, validator id past
    /// `num_validators`) abort the whole call with an error. Semantic
    /// filters (source not yet justified, target already justified, root
    /// mismatch, target not justifiable) cause the offending vote to be
    /// silently skipped.
    ///
    /// All mutation is staged in working copies and committed atomically
    /// after the loop, so an `Err` return leaves the state byte-equal to
    /// its pre-call value.
    ///
    /// # Errors
    /// - [`StateTransitionError::AttestationSlotOutOfRange`] when a vote
    ///   references a slot beyond `state.justified_slots.len()` or
    ///   `state.historical_block_hashes.len()`.
    /// - [`StateTransitionError::AttestationValidatorOutOfRange`] when
    ///   `validator_id >= state.config.num_validators`.
    /// - [`StateTransitionError::StateBoundExceeded`] forwarded from the
    ///   working bitmap rebuild.
    pub fn process_attestations(
        &mut self,
        attestations: &[SignedVote],
    ) -> Result<(), StateTransitionError> {
        let num_validators = self.config.num_validators;
        let just_len = self.justified_slots.len();
        let hist_len = self.historical_block_hashes.len();

        // Working copies — committed at end if every iteration succeeds.
        let mut justifications = Justifications::try_from(&*self)?;
        let mut justified_slots = self.justified_slots.clone();
        let mut latest_justified = self.latest_justified;
        let mut latest_finalized = self.latest_finalized;

        let validator_limit = usize::try_from(num_validators).map_err(|_| {
            StateTransitionError::StateBoundExceeded {
                context: "num_validators",
            }
        })?;

        for signed in attestations {
            let vote = &signed.message;
            let validator_id = signed.validator_id;
            let source_slot = vote.source.slot;
            let target_slot = vote.target.slot;

            // -- Range checks: any failure aborts the whole call. ----------
            let source_idx = bounded_slot_index(source_slot, AttSlotKind::Source, just_len)?;
            let _ = bounded_slot_index(source_slot, AttSlotKind::Source, hist_len)?;
            let target_idx = bounded_slot_index(target_slot, AttSlotKind::Target, just_len)?;
            let _ = bounded_slot_index(target_slot, AttSlotKind::Target, hist_len)?;
            let validator_idx = usize::try_from(validator_id.get())
                .ok()
                .filter(|&i| i < validator_limit)
                .ok_or(StateTransitionError::AttestationValidatorOutOfRange {
                    validator: validator_id,
                    num_validators,
                })?;

            // -- Semantic filters: skip on mismatch. -----------------------
            let acceptable = justified_slots.get(source_idx) == Some(true)
                && justified_slots.get(target_idx) == Some(false)
                && vote.source.root == self.historical_block_hashes[source_idx]
                && vote.target.root == self.historical_block_hashes[target_idx]
                && target_slot > source_slot
                && target_slot.is_justifiable_after(latest_finalized.slot);
            if !acceptable {
                continue;
            }

            // -- Tally. ----------------------------------------------------
            let n = justifications.num_validators;
            let votes = justifications
                .table
                .entry(vote.target.root)
                .or_insert_with(|| vec![false; n]);
            votes[validator_idx] = true;
            let count = votes.iter().filter(|&&v| v).count();

            // 2/3 supermajority: `3 * count >= 2 * num_validators` avoids
            // integer-division shortfall for small `num_validators`.
            if 3 * count < 2 * validator_limit {
                continue;
            }

            // -- Justify target. ------------------------------------------
            latest_justified = vote.target;
            justified_slots.set(target_idx, true).map_err(|_| {
                StateTransitionError::StateBoundExceeded {
                    context: "justified_slots",
                }
            })?;
            justifications.table.remove(&vote.target.root);

            // -- Finalize source if no justifiable slot lies strictly
            //    between source and target.
            // `mid as u64`: `mid < target_idx <= just_len <= usize::MAX <= u64::MAX`,
            // so the cast is lossless on every supported target.
            let no_intermediate = ((source_idx + 1)..target_idx).all(|mid| {
                let candidate = Slot::new(mid as u64);
                !candidate.is_justifiable_after(latest_finalized.slot)
            });
            if no_intermediate {
                latest_finalized = vote.source;
            }
        }

        // -- Commit. -------------------------------------------------------
        self.justified_slots = justified_slots;
        self.latest_justified = latest_justified;
        self.latest_finalized = latest_finalized;
        justifications.write_back(self)
    }
}

// =====================================================================
// state_transition (driver)
// =====================================================================

impl State {
    /// Applies the full state transition for `signed_block`.
    ///
    /// Composes [`State::process_slots`] (to the block's slot),
    /// [`State::process_block_header`], and [`State::process_attestations`].
    /// When `validate_state_root` is `true`, also asserts that the post-state
    /// `hash_tree_root` equals `signed_block.message.state_root`.
    ///
    /// Transactional: the transition is computed in a local working copy
    /// and swapped into `self` only when every step succeeds, so an `Err`
    /// return leaves `self` byte-equal to its pre-call value. Cost: one
    /// `State` clone per call.
    ///
    /// # Errors
    /// - Forwarded from [`State::process_slots`] /
    ///   [`State::process_block_header`] / [`State::process_attestations`].
    /// - [`StateTransitionError::StateRootMismatch`] when
    ///   `validate_state_root` is `true` and `next.hash_tree_root() !=
    ///   signed_block.message.state_root`.
    pub fn state_transition(
        &mut self,
        signed_block: &SignedBlock,
        validate_state_root: bool,
    ) -> Result<(), StateTransitionError> {
        let block = &signed_block.message;
        let mut next = self.clone();
        next.process_slots(block.slot)?;
        next.process_block_header(block)?;
        next.process_attestations(&block.body.attestations)?;
        if validate_state_root {
            let got: Bytes32 = next.hash_tree_root().into();
            if got != block.state_root {
                return Err(StateTransitionError::StateRootMismatch {
                    slot: block.slot,
                    got,
                    want: block.state_root,
                });
            }
        }
        *self = next;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use ssz::{decode, encode, SszError};
    use types::Bytes32;

    use crate::test_fixtures::sample_block_header;

    fn sample_state() -> State {
        let mut justified_slots: Bitlist<HISTORICAL_ROOTS_LIMIT> = Bitlist::new();
        justified_slots.set(0, true).unwrap();
        justified_slots.set(2, true).unwrap();

        let mut justifications_validators: Bitlist<JUSTIFICATIONS_VALIDATORS_LIMIT> =
            Bitlist::new();
        for i in [0_usize, 2, 5, 7] {
            justifications_validators.set(i, true).unwrap();
        }

        State {
            config: ProtocolConfig {
                num_validators: 4,
                genesis_time: 1_700_000_000,
            },
            slot: Slot::new(9),
            latest_block_header: sample_block_header(),
            latest_justified: Checkpoint::new(Bytes32::new([0x44; 32]), Slot::new(8)),
            latest_finalized: Checkpoint::new(Bytes32::new([0x55; 32]), Slot::new(0)),
            historical_block_hashes: vec![Bytes32::new([0xaa; 32]), Bytes32::new([0xbb; 32])],
            justified_slots,
            justifications_roots: vec![Bytes32::new([0xcc; 32]), Bytes32::new([0xdd; 32])],
            justifications_validators,
        }
    }

    // -- Constants ----------------------------------------------------------

    #[test]
    fn fixed_part_is_two_thirty_two_bytes() {
        assert_eq!(STATE_FIXED_PART_LEN, 232);
    }

    #[test]
    fn limits_match_devnet_config() {
        assert_eq!(HISTORICAL_ROOTS_LIMIT, 262_144);
        assert_eq!(VALIDATOR_REGISTRY_LIMIT, 4_096);
        assert_eq!(JUSTIFICATIONS_VALIDATORS_LIMIT, 262_144 * 4_096);
    }

    // -- ProtocolConfig SSZ -------------------------------------------------

    #[test]
    fn protocol_config_ssz_fixed_len_is_sixteen() {
        assert_eq!(<ProtocolConfig as Encode>::ssz_fixed_len(), 16);
        assert!(<ProtocolConfig as Encode>::is_ssz_fixed_len());
    }

    #[test]
    fn protocol_config_round_trip() {
        let cfg = ProtocolConfig {
            num_validators: 0xdead_beef,
            genesis_time: 0x1234_5678,
        };
        let bytes = encode(&cfg);
        assert_eq!(bytes.len(), PROTOCOL_CONFIG_SSZ_LEN);
        assert_eq!(&bytes[..8], &0xdead_beef_u64.to_le_bytes());
        assert_eq!(&bytes[8..16], &0x1234_5678_u64.to_le_bytes());
        let back: ProtocolConfig = decode(&bytes).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn protocol_config_decode_rejects_wrong_length() {
        assert!(decode::<ProtocolConfig>(&[0_u8; 15]).is_err());
        assert!(decode::<ProtocolConfig>(&[0_u8; 17]).is_err());
    }

    #[test]
    fn protocol_config_hash_tree_root_distinguishes_fields() {
        let a = ProtocolConfig {
            num_validators: 7,
            genesis_time: 0,
        };
        let b = ProtocolConfig {
            num_validators: 0,
            genesis_time: 7,
        };
        assert_ne!(a.hash_tree_root(), b.hash_tree_root());
    }

    // -- State SSZ ---------------------------------------------------------

    #[test]
    fn state_default_round_trip() {
        let s = State::default();
        let bytes = encode(&s);
        // Empty Bitlist encodes to a single delimiter byte (0x01); two
        // empty Vec<Bytes32> tails are zero-length. Total = 232 + 0 + 1 + 0 + 1.
        assert_eq!(bytes.len(), STATE_FIXED_PART_LEN + 2);
        let back: State = decode(&bytes).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn state_populated_round_trip() {
        let s = sample_state();
        let bytes = encode(&s);
        let back: State = decode(&bytes).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn state_first_offset_equals_fixed_part_len() {
        let s = sample_state();
        let bytes = encode(&s);
        let off_pos = STATE_FIXED_PART_LEN - 16;
        let off0 = u32::from_le_bytes([
            bytes[off_pos],
            bytes[off_pos + 1],
            bytes[off_pos + 2],
            bytes[off_pos + 3],
        ]);
        assert_eq!(off0 as usize, STATE_FIXED_PART_LEN);
    }

    #[test]
    fn state_decode_rejects_short_input() {
        let err = decode::<State>(&[0_u8; STATE_FIXED_PART_LEN - 1]).unwrap_err();
        assert!(matches!(err, SszError::Decode { .. }));
    }

    #[test]
    fn state_decode_rejects_invalid_first_offset() {
        let s = State::default();
        let mut bytes = encode(&s);
        let off_pos = STATE_FIXED_PART_LEN - 16;
        bytes[off_pos..off_pos + 4].copy_from_slice(
            &u32::try_from(STATE_FIXED_PART_LEN - 1)
                .unwrap()
                .to_le_bytes(),
        );
        let err = decode::<State>(&bytes).unwrap_err();
        assert!(matches!(err, SszError::Decode { .. }));
    }

    #[test]
    fn state_decode_rejects_decreasing_offsets() {
        let s = sample_state();
        let mut bytes = encode(&s);
        let off0_pos = STATE_FIXED_PART_LEN - 16;
        let off1_pos = STATE_FIXED_PART_LEN - 12;
        let off0 = u32::from_le_bytes([
            bytes[off0_pos],
            bytes[off0_pos + 1],
            bytes[off0_pos + 2],
            bytes[off0_pos + 3],
        ]);
        let off1 = u32::from_le_bytes([
            bytes[off1_pos],
            bytes[off1_pos + 1],
            bytes[off1_pos + 2],
            bytes[off1_pos + 3],
        ]);
        if off0 == off1 {
            return;
        }
        bytes[off0_pos..off0_pos + 4].copy_from_slice(&off1.to_le_bytes());
        bytes[off1_pos..off1_pos + 4].copy_from_slice(&off0.to_le_bytes());
        let err = decode::<State>(&bytes).unwrap_err();
        assert!(matches!(err, SszError::Decode { .. }));
    }

    // -- State HashTreeRoot ------------------------------------------------
    //
    // The all-nine-fields responsiveness check that documents cross-client
    // (ream) HTR-shape compatibility lives in `crate::ream`'s tests; the
    // check below covers the remaining `slot` / `latest_block_header` fields.

    #[test]
    fn state_hash_tree_root_responds_to_slot_and_latest_header() {
        let baseline = sample_state().hash_tree_root();

        let mut s = sample_state();
        s.slot = Slot::new(10);
        assert_ne!(s.hash_tree_root(), baseline);

        let mut s = sample_state();
        s.latest_block_header.body_root = Bytes32::new([0x99; 32]);
        assert_ne!(s.hash_tree_root(), baseline);
    }

    #[test]
    fn state_hash_tree_root_is_deterministic() {
        let s = sample_state();
        assert_eq!(s.hash_tree_root(), s.hash_tree_root());
    }

    // -- property tests ----------------------------------------------------

    proptest! {
        #[test]
        fn protocol_config_round_trips(
            num in any::<u64>(),
            ts in any::<u64>(),
        ) {
            let cfg = ProtocolConfig {
                num_validators: num,
                genesis_time: ts,
            };
            let back: ProtocolConfig = decode(&encode(&cfg)).unwrap();
            prop_assert_eq!(back, cfg);
        }

        #[test]
        fn state_round_trips_with_varied_tails(
            slot in any::<u64>(),
            n_hist in 0_usize..=8,
            n_roots in 0_usize..=8,
            justified_bits in proptest::collection::vec(any::<u8>(), 0..=4),
            validator_bits in proptest::collection::vec(any::<u8>(), 0..=8),
        ) {
            let mut s = State {
                slot: Slot::new(slot),
                ..State::default()
            };
            for i in 0..n_hist {
                let byte = u8::try_from(i & 0xff).unwrap();
                s.historical_block_hashes.push(Bytes32::new([byte; 32]));
            }
            for i in 0..n_roots {
                let byte = u8::try_from((i + 0x80) & 0xff).unwrap();
                s.justifications_roots.push(Bytes32::new([byte; 32]));
            }
            for &i in &justified_bits {
                s.justified_slots.set(usize::from(i) % 32, true).unwrap();
            }
            for &i in &validator_bits {
                s.justifications_validators
                    .set(usize::from(i) % 64, true)
                    .unwrap();
            }
            let back: State = decode(&encode(&s)).unwrap();
            prop_assert_eq!(back, s);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod justifications_tests {
    use super::*;

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
        let view = Justifications::try_from(&state).unwrap();
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

        let view2 = Justifications::try_from(&state).unwrap();
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
        // not 3. The conversion should reject the inconsistency.
        state.justifications_validators.set(5, true).unwrap();
        let err = Justifications::try_from(&state).unwrap_err();
        assert_eq!(
            err,
            StateTransitionError::StateBoundExceeded {
                context: "justifications_validators",
            }
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod attestation_tests {
    use super::*;

    use crate::block::BlockHeader;
    use crate::checkpoint::Checkpoint;
    use crate::validator::ValidatorIndex;
    use crate::vote::{SignedVote, Vote};

    /// Builds a state with `num_validators` validators, populated history of
    /// `historical_roots`, and `justified_slots` matching the
    /// `justified_pattern` (bool per slot).
    fn populated_state(
        num_validators: u64,
        historical_roots: Vec<Bytes32>,
        justified_pattern: &[bool],
        latest_finalized_slot: Slot,
    ) -> State {
        let mut justified_slots: Bitlist<HISTORICAL_ROOTS_LIMIT> = Bitlist::new();
        for (i, &v) in justified_pattern.iter().enumerate() {
            justified_slots.set(i, v).unwrap();
        }
        State {
            config: ProtocolConfig {
                num_validators,
                genesis_time: 0,
            },
            slot: Slot::new(historical_roots.len() as u64),
            latest_block_header: BlockHeader::default(),
            latest_justified: Checkpoint::default(),
            latest_finalized: Checkpoint::new(Bytes32::zero(), latest_finalized_slot),
            historical_block_hashes: historical_roots,
            justified_slots,
            justifications_roots: Vec::new(),
            justifications_validators: Bitlist::new(),
        }
    }

    fn signed_vote(
        validator_id: u64,
        source_root: Bytes32,
        source_slot: u64,
        target_root: Bytes32,
        target_slot: u64,
    ) -> SignedVote {
        SignedVote {
            validator_id: ValidatorIndex::new(validator_id),
            message: Vote {
                slot: Slot::new(target_slot),
                head: Checkpoint::new(target_root, Slot::new(target_slot)),
                target: Checkpoint::new(target_root, Slot::new(target_slot)),
                source: Checkpoint::new(source_root, Slot::new(source_slot)),
            },
            signature: types::Bytes4000::default(),
        }
    }

    fn root(byte: u8) -> Bytes32 {
        Bytes32::new([byte; 32])
    }

    // -- Range checks: aborting paths ---------------------------------------

    #[test]
    fn out_of_range_source_slot_aborts() {
        let mut state = populated_state(4, vec![root(0xaa)], &[true], Slot::ZERO);
        let votes = vec![signed_vote(0, root(0xaa), 5, root(0xbb), 6)];
        let err = state.process_attestations(&votes).unwrap_err();
        assert!(matches!(
            err,
            StateTransitionError::AttestationSlotOutOfRange {
                kind: AttSlotKind::Source,
                ..
            }
        ));
    }

    #[test]
    fn out_of_range_target_slot_aborts() {
        let mut state =
            populated_state(4, vec![root(0xaa), root(0xbb)], &[true, false], Slot::ZERO);
        let votes = vec![signed_vote(0, root(0xaa), 0, root(0xcc), 9)];
        let err = state.process_attestations(&votes).unwrap_err();
        assert!(matches!(
            err,
            StateTransitionError::AttestationSlotOutOfRange {
                kind: AttSlotKind::Target,
                ..
            }
        ));
    }

    #[test]
    fn out_of_range_validator_aborts() {
        let mut state =
            populated_state(4, vec![root(0xaa), root(0xbb)], &[true, false], Slot::ZERO);
        let votes = vec![signed_vote(99, root(0xaa), 0, root(0xbb), 1)];
        let err = state.process_attestations(&votes).unwrap_err();
        assert_eq!(
            err,
            StateTransitionError::AttestationValidatorOutOfRange {
                validator: ValidatorIndex::new(99),
                num_validators: 4,
            }
        );
    }

    #[test]
    fn range_check_error_leaves_state_unchanged() {
        let mut state =
            populated_state(4, vec![root(0xaa), root(0xbb)], &[true, false], Slot::ZERO);
        let snapshot = state.clone();
        let votes = vec![signed_vote(99, root(0xaa), 0, root(0xbb), 1)];
        let _ = state.process_attestations(&votes).unwrap_err();
        assert_eq!(state, snapshot);
    }

    // -- Semantic filters: skip paths --------------------------------------

    #[test]
    fn skips_when_source_not_justified() {
        let mut state =
            populated_state(4, vec![root(0xaa), root(0xbb)], &[false, false], Slot::ZERO);
        let snapshot = state.clone();
        let votes = vec![signed_vote(0, root(0xaa), 0, root(0xbb), 1)];
        state.process_attestations(&votes).unwrap();
        assert_eq!(state, snapshot);
    }

    #[test]
    fn skips_when_target_already_justified() {
        let mut state = populated_state(4, vec![root(0xaa), root(0xbb)], &[true, true], Slot::ZERO);
        let snapshot = state.clone();
        let votes = vec![signed_vote(0, root(0xaa), 0, root(0xbb), 1)];
        state.process_attestations(&votes).unwrap();
        assert_eq!(state, snapshot);
    }

    #[test]
    fn skips_when_source_root_mismatch() {
        let mut state =
            populated_state(4, vec![root(0xaa), root(0xbb)], &[true, false], Slot::ZERO);
        let snapshot = state.clone();
        let votes = vec![signed_vote(0, root(0xff), 0, root(0xbb), 1)];
        state.process_attestations(&votes).unwrap();
        assert_eq!(state, snapshot);
    }

    #[test]
    fn skips_when_target_le_source() {
        let mut state = populated_state(
            4,
            vec![root(0xaa), root(0xbb), root(0xcc)],
            &[true, true, false],
            Slot::ZERO,
        );
        let snapshot = state.clone();
        let votes = vec![signed_vote(0, root(0xaa), 0, root(0xaa), 0)];
        state.process_attestations(&votes).unwrap();
        assert_eq!(state, snapshot);
    }

    #[test]
    fn skips_when_target_not_justifiable() {
        // delta = 7 - 0 = 7 — neither perfect square nor pronic and > 5.
        let history: Vec<Bytes32> = (0_u8..8).map(root).collect();
        let mut just_pattern = vec![false; 8];
        just_pattern[0] = true;
        let mut state = populated_state(4, history, &just_pattern, Slot::ZERO);
        let snapshot = state.clone();
        let votes = vec![signed_vote(0, root(0), 0, root(7), 7)];
        state.process_attestations(&votes).unwrap();
        assert_eq!(state, snapshot);
    }

    // -- Tally and supermajority --------------------------------------------

    #[test]
    fn single_subthreshold_vote_does_not_justify() {
        let mut state =
            populated_state(4, vec![root(0xaa), root(0xbb)], &[true, false], Slot::ZERO);
        let votes = vec![signed_vote(0, root(0xaa), 0, root(0xbb), 1)];
        state.process_attestations(&votes).unwrap();
        assert_eq!(state.justified_slots.get(1), Some(false));
        assert_eq!(state.justifications_roots, vec![root(0xbb)]);
        assert_eq!(state.justifications_validators.len(), 4);
        assert_eq!(state.justifications_validators.get(0), Some(true));
        assert_eq!(state.justifications_validators.get(1), Some(false));
    }

    #[test]
    fn supermajority_justifies_target() {
        let mut state =
            populated_state(4, vec![root(0xaa), root(0xbb)], &[true, false], Slot::ZERO);
        let votes = vec![
            signed_vote(0, root(0xaa), 0, root(0xbb), 1),
            signed_vote(1, root(0xaa), 0, root(0xbb), 1),
            signed_vote(2, root(0xaa), 0, root(0xbb), 1),
        ];
        state.process_attestations(&votes).unwrap();
        assert_eq!(state.justified_slots.get(1), Some(true));
        assert_eq!(state.latest_justified.root, root(0xbb));
        assert_eq!(state.latest_justified.slot, Slot::new(1));
        assert!(state.justifications_roots.is_empty());
        assert_eq!(state.justifications_validators.len(), 0);
    }

    #[test]
    fn finalizes_source_when_target_is_next_justifiable_slot() {
        let mut state =
            populated_state(4, vec![root(0xaa), root(0xbb)], &[true, false], Slot::ZERO);
        let votes = vec![
            signed_vote(0, root(0xaa), 0, root(0xbb), 1),
            signed_vote(1, root(0xaa), 0, root(0xbb), 1),
            signed_vote(2, root(0xaa), 0, root(0xbb), 1),
        ];
        state.process_attestations(&votes).unwrap();
        assert_eq!(state.latest_finalized.root, root(0xaa));
        assert_eq!(state.latest_finalized.slot, Slot::ZERO);
    }

    #[test]
    fn does_not_finalize_when_intermediate_justifiable_slot_exists() {
        let history: Vec<Bytes32> = (0_u8..10).map(root).collect();
        let mut just_pattern = vec![false; 10];
        just_pattern[0] = true;
        let mut state = populated_state(4, history, &just_pattern, Slot::ZERO);
        let original_finalized = state.latest_finalized;
        let votes = vec![
            signed_vote(0, root(0), 0, root(9), 9),
            signed_vote(1, root(0), 0, root(9), 9),
            signed_vote(2, root(0), 0, root(9), 9),
        ];
        state.process_attestations(&votes).unwrap();
        assert_eq!(state.justified_slots.get(9), Some(true));
        assert_eq!(state.latest_finalized, original_finalized);
    }

    #[test]
    fn duplicate_vote_for_same_validator_is_idempotent() {
        let mut once = populated_state(4, vec![root(0xaa), root(0xbb)], &[true, false], Slot::ZERO);
        let mut twice = once.clone();
        let votes = vec![signed_vote(0, root(0xaa), 0, root(0xbb), 1)];
        once.process_attestations(&votes).unwrap();
        let votes_twice = vec![votes[0].clone(), votes[0].clone()];
        twice.process_attestations(&votes_twice).unwrap();
        assert_eq!(once.hash_tree_root(), twice.hash_tree_root());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod block_processing_tests {
    use super::*;
    use crate::block::BlockBody;
    use crate::validator::ValidatorIndex;

    const NUM_VALIDATORS: u64 = 4;
    const GENESIS_TIME: u64 = 1_700_000_000;

    /// Genesis-shape `State` for a 4-validator chain whose
    /// `latest_block_header` commits to the empty body.
    fn genesis() -> State {
        let body_root: Bytes32 = BlockBody::default().hash_tree_root().into();
        State {
            config: ProtocolConfig {
                num_validators: NUM_VALIDATORS,
                genesis_time: GENESIS_TIME,
            },
            latest_block_header: BlockHeader {
                body_root,
                ..BlockHeader::default()
            },
            ..State::default()
        }
    }

    /// Produces a valid block for `state` at `state.slot` whose body is empty.
    fn valid_block_for(state: &State) -> Block {
        let parent_root: Bytes32 = state.latest_block_header.hash_tree_root().into();
        let proposer_index = ValidatorIndex::new(state.slot.get() % state.config.num_validators);
        Block {
            slot: state.slot,
            proposer_index,
            parent_root,
            state_root: Bytes32::zero(),
            body: BlockBody::default(),
        }
    }

    // -- Validation: rejection paths ----------------------------------------

    #[test]
    fn block_slot_mismatch_rejects() {
        let mut state = genesis();
        state.process_slots(Slot::new(2)).unwrap();
        let mut block = valid_block_for(&state);
        block.slot = Slot::new(3);
        let err = state.process_block_header(&block).unwrap_err();
        assert_eq!(
            err,
            StateTransitionError::BlockSlotMismatch {
                got: Slot::new(3),
                want: Slot::new(2),
            }
        );
    }

    #[test]
    fn block_older_than_latest_rejects() {
        let mut state = genesis();
        state.process_slots(Slot::new(3)).unwrap();
        state.latest_block_header.slot = Slot::new(3);
        let block = valid_block_for(&state);
        let err = state.process_block_header(&block).unwrap_err();
        assert_eq!(
            err,
            StateTransitionError::BlockOlderThanLatest {
                slot: Slot::new(3),
                latest: Slot::new(3),
            }
        );
    }

    #[test]
    fn incorrect_proposer_rejects() {
        let mut state = genesis();
        state.process_slots(Slot::new(1)).unwrap();
        let mut block = valid_block_for(&state);
        // slot 1 round-robin proposer with N=4 is index 1; choose 2 instead.
        block.proposer_index = ValidatorIndex::new(2);
        let err = state.process_block_header(&block).unwrap_err();
        assert_eq!(
            err,
            StateTransitionError::IncorrectBlockProposer {
                slot: Slot::new(1),
                proposer: ValidatorIndex::new(2),
            }
        );
    }

    #[test]
    fn parent_root_mismatch_rejects() {
        let mut state = genesis();
        state.process_slots(Slot::new(1)).unwrap();
        let mut block = valid_block_for(&state);
        block.parent_root = Bytes32::new([0xff; 32]);
        let err = state.process_block_header(&block).unwrap_err();
        assert!(matches!(
            err,
            StateTransitionError::BlockParentRootMismatch { slot, .. } if slot == Slot::new(1)
        ));
    }

    #[test]
    fn zero_validators_surfaces_protocol_error() {
        let mut state = genesis();
        state.config.num_validators = 0;
        state.process_slots(Slot::new(1)).unwrap();
        let block = Block {
            slot: Slot::new(1),
            ..Default::default()
        };
        let err = state.process_block_header(&block).unwrap_err();
        assert!(matches!(err, StateTransitionError::Protocol(_)));
    }

    // -- Validation: state preserved on error -------------------------------

    #[test]
    fn error_path_leaves_state_unchanged() {
        let mut state = genesis();
        state.process_slots(Slot::new(2)).unwrap();
        let snapshot = state.clone();
        let mut block = valid_block_for(&state);
        block.parent_root = Bytes32::new([0xab; 32]);
        let _ = state.process_block_header(&block).unwrap_err();
        assert_eq!(state, snapshot);
    }

    // -- Happy path: commitment ---------------------------------------------

    #[test]
    fn happy_path_commits_header_and_root() {
        let mut state = genesis();
        state.process_slots(Slot::new(1)).unwrap();
        let block = valid_block_for(&state);
        let parent_root = block.parent_root;
        let body_root: Bytes32 = block.body.hash_tree_root().into();

        state.process_block_header(&block).unwrap();

        assert_eq!(state.latest_block_header.slot, Slot::new(1));
        assert_eq!(state.latest_block_header.parent_root, parent_root);
        assert_eq!(state.latest_block_header.body_root, body_root);
        // process_block_header zeroes the post-state root sentinel.
        assert_eq!(state.latest_block_header.state_root, Bytes32::zero());
        assert_eq!(
            state.latest_block_header.proposer_index,
            block.proposer_index
        );
    }

    #[test]
    fn genesis_seeds_justified_and_finalized_root() {
        let mut state = genesis();
        state.process_slots(Slot::new(1)).unwrap();
        let block = valid_block_for(&state);
        let parent_root = block.parent_root;

        assert_eq!(state.latest_justified, Checkpoint::default());
        assert_eq!(state.latest_finalized, Checkpoint::default());
        state.process_block_header(&block).unwrap();
        assert_eq!(state.latest_justified.root, parent_root);
        assert_eq!(state.latest_finalized.root, parent_root);
        // Slots stay at their default zero values; only the root is seeded.
        assert_eq!(state.latest_justified.slot, Slot::ZERO);
        assert_eq!(state.latest_finalized.slot, Slot::ZERO);
    }

    #[test]
    fn appends_parent_root_and_genesis_justified_bit() {
        let mut state = genesis();
        state.process_slots(Slot::new(1)).unwrap();
        let block = valid_block_for(&state);
        let parent_root = block.parent_root;

        state.process_block_header(&block).unwrap();
        assert_eq!(state.historical_block_hashes, vec![parent_root]);
        assert_eq!(state.justified_slots.len(), 1);
        // Genesis branch records the parent slot (0) as justified.
        assert_eq!(state.justified_slots.get(0), Some(true));
    }

    #[test]
    fn empty_slots_filled_with_zero_root_and_unjustified_bits() {
        // First block at slot 1 (no empty slots).
        let mut state = genesis();
        state.process_slots(Slot::new(1)).unwrap();
        let block_a = valid_block_for(&state);
        let parent_root_a = block_a.parent_root;
        state.process_block_header(&block_a).unwrap();

        // Second block at slot 4 — empty_slots = 4 - 1 - 1 = 2.
        state.process_slots(Slot::new(4)).unwrap();
        let block_b = Block {
            slot: Slot::new(4),
            proposer_index: ValidatorIndex::new(0),
            parent_root: state.latest_block_header.hash_tree_root().into(),
            state_root: Bytes32::zero(),
            body: BlockBody::default(),
        };
        let parent_root_b = block_b.parent_root;

        state.process_block_header(&block_b).unwrap();

        assert_eq!(state.historical_block_hashes.len(), 4);
        assert_eq!(state.historical_block_hashes[0], parent_root_a);
        assert_eq!(state.historical_block_hashes[1], parent_root_b);
        assert_eq!(state.historical_block_hashes[2], Bytes32::zero());
        assert_eq!(state.historical_block_hashes[3], Bytes32::zero());

        assert_eq!(state.justified_slots.len(), 4);
        assert_eq!(state.justified_slots.get(0), Some(true));
        assert_eq!(state.justified_slots.get(1), Some(false));
        assert_eq!(state.justified_slots.get(2), Some(false));
        assert_eq!(state.justified_slots.get(3), Some(false));
    }

    #[test]
    fn second_block_does_not_reseed_justified_root() {
        let mut state = genesis();
        state.process_slots(Slot::new(1)).unwrap();
        let block_a = valid_block_for(&state);
        let parent_root_a = block_a.parent_root;
        state.process_block_header(&block_a).unwrap();

        state.process_slots(Slot::new(2)).unwrap();
        let block_b = Block {
            slot: Slot::new(2),
            proposer_index: ValidatorIndex::new(2),
            parent_root: state.latest_block_header.hash_tree_root().into(),
            state_root: Bytes32::zero(),
            body: BlockBody::default(),
        };

        state.process_block_header(&block_b).unwrap();
        // Genesis-seeding only fires once: the second block leaves the
        // justified root pointing at the genesis parent.
        assert_eq!(state.latest_justified.root, parent_root_a);
        assert_eq!(state.latest_finalized.root, parent_root_a);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod slot_processing_tests {
    use super::*;
    use proptest::prelude::*;

    use crate::block::BlockBody;

    /// Minimal fixture: a non-default `State` whose `latest_block_header`
    /// commits to the empty `BlockBody`. Mirrors the slot-0 shape used by
    /// `crate::stf::genesis_state` without going through the module path.
    fn fresh_state() -> State {
        State {
            latest_block_header: BlockHeader {
                body_root: BlockBody::default().hash_tree_root().into(),
                ..BlockHeader::default()
            },
            ..State::default()
        }
    }

    // -- advance_slot --------------------------------------------------------

    #[test]
    fn advance_slot_increments() {
        assert_eq!(advance_slot(Slot::ZERO).unwrap(), Slot::ONE);
        assert_eq!(advance_slot(Slot::new(41)).unwrap(), Slot::new(42));
    }

    #[test]
    fn advance_slot_rejects_overflow() {
        let err = advance_slot(Slot::new(u64::MAX)).unwrap_err();
        assert_eq!(
            err,
            StateTransitionError::SlotOverflow {
                slot: Slot::new(u64::MAX),
            }
        );
    }

    // -- process_slot --------------------------------------------------------

    #[test]
    fn process_slot_caches_previous_state_root_when_zero() {
        let mut state = fresh_state();
        let pre_root: Bytes32 = state.hash_tree_root().into();
        state.process_slot().unwrap();
        assert_eq!(state.latest_block_header.state_root, pre_root);
    }

    #[test]
    fn process_slot_no_op_when_state_root_already_set() {
        let mut state = fresh_state();
        state.latest_block_header.state_root = Bytes32::new([0xab; 32]);
        let snapshot = state.clone();
        state.process_slot().unwrap();
        assert_eq!(state, snapshot);
    }

    // -- process_slots: error paths -----------------------------------------

    #[test]
    fn process_slots_rejects_equal_target() {
        let mut state = fresh_state();
        let target = state.slot;
        let err = state.process_slots(target).unwrap_err();
        assert_eq!(
            err,
            StateTransitionError::TargetSlotNotInFuture {
                current: Slot::ZERO,
                target: Slot::ZERO,
            }
        );
    }

    #[test]
    fn process_slots_rejects_past_target() {
        let mut state = fresh_state();
        state.slot = Slot::new(5);
        let err = state.process_slots(Slot::new(3)).unwrap_err();
        assert_eq!(
            err,
            StateTransitionError::TargetSlotNotInFuture {
                current: Slot::new(5),
                target: Slot::new(3),
            }
        );
    }

    // -- process_slots: advancement -----------------------------------------

    #[test]
    fn process_slots_advances_to_target() {
        let mut state = fresh_state();
        state.process_slots(Slot::new(5)).unwrap();
        assert_eq!(state.slot, Slot::new(5));
    }

    #[test]
    fn process_slots_single_step_advance() {
        let mut state = fresh_state();
        state.process_slots(Slot::ONE).unwrap();
        assert_eq!(state.slot, Slot::ONE);
    }

    // Genesis-shape state has the zero-root sentinel → first iteration
    // caches it; on subsequent iterations the no-op branch fires, so the
    // cached root survives through the remaining steps.
    #[test]
    fn process_slots_caches_state_root_on_first_step_only() {
        let mut state = fresh_state();
        let pre_root: Bytes32 = state.hash_tree_root().into();
        state.process_slots(Slot::new(3)).unwrap();
        assert_eq!(state.latest_block_header.state_root, pre_root);
    }

    // -- property tests -----------------------------------------------------

    proptest! {
        #[test]
        fn process_slots_path_equivalence(t1 in 1_u64..32, t2_offset in 1_u64..32) {
            let t2 = t1 + t2_offset;

            let mut direct = fresh_state();
            direct.process_slots(Slot::new(t2)).unwrap();

            let mut via_intermediate = fresh_state();
            via_intermediate.process_slots(Slot::new(t1)).unwrap();
            via_intermediate.process_slots(Slot::new(t2)).unwrap();

            prop_assert_eq!(direct.hash_tree_root(), via_intermediate.hash_tree_root());
        }

        #[test]
        fn process_slots_final_slot_equals_target(target in 1_u64..64) {
            let mut state = fresh_state();
            state.process_slots(Slot::new(target)).unwrap();
            prop_assert_eq!(state.slot, Slot::new(target));
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod state_transition_tests {
    use super::*;
    use proptest::prelude::*;
    use ssz::{decode, encode};

    use crate::block::BlockBody;
    use crate::validator::ValidatorIndex;

    const GENESIS_TIME: u64 = 1_700_000_000;

    fn genesis_state(num_validators: u64) -> State {
        crate::stf::genesis_state(num_validators, GENESIS_TIME)
    }

    /// Two-phase build: produce a `SignedBlock` for `state` whose body is
    /// empty and whose `state_root` matches the post-state reached by
    /// applying the transition on a clone of `state`.
    fn build_signed_block(state: &State, slot: Slot) -> SignedBlock {
        let proposer_index = ValidatorIndex::new(slot.get() % state.config.num_validators);

        // Phase 1: compute the post-state with `state_root = zero`.
        let mut probe = state.clone();
        probe.process_slots(slot).unwrap();
        let parent_root: Bytes32 = probe.latest_block_header.hash_tree_root().into();
        let mut block = Block {
            slot,
            proposer_index,
            parent_root,
            state_root: Bytes32::zero(),
            body: BlockBody::default(),
        };
        probe.process_block_header(&block).unwrap();
        probe
            .process_attestations(&block.body.attestations)
            .unwrap();
        let state_root: Bytes32 = probe.hash_tree_root().into();

        // Phase 2: rewrite the block with the computed state_root.
        block.state_root = state_root;
        SignedBlock {
            message: block,
            signature: types::Bytes4000::default(),
        }
    }

    /// Empty-body chain of `n` consecutive valid signed blocks starting from
    /// `start`. Each block's `state_root` is the post-state root after
    /// applying the prior blocks.
    fn build_chain(start: &State, n: usize) -> Vec<SignedBlock> {
        let mut chain = Vec::with_capacity(n);
        let mut walker = start.clone();
        for i in 1..=n {
            let slot = Slot::new(i as u64);
            let sb = build_signed_block(&walker, slot);
            walker.state_transition(&sb, true).unwrap();
            chain.push(sb);
        }
        chain
    }

    // -- Composition --------------------------------------------------------

    #[test]
    fn composes_slots_block_attestations_in_order() {
        let mut driven = genesis_state(4);
        let mut hand = driven.clone();
        let sb = build_signed_block(&driven, Slot::new(1));
        let block = sb.message.clone();

        driven.state_transition(&sb, true).unwrap();
        hand.process_slots(block.slot).unwrap();
        hand.process_block_header(&block).unwrap();
        hand.process_attestations(&block.body.attestations).unwrap();

        assert_eq!(driven, hand);
    }

    // -- Validation flag ----------------------------------------------------

    #[test]
    fn state_root_mismatch_when_validation_on_and_root_tampered() {
        let mut state = genesis_state(4);
        let mut sb = build_signed_block(&state, Slot::new(1));
        let want = sb.message.state_root;
        // Flip a byte in the declared post-state root.
        let mut tampered = want;
        tampered.0[0] ^= 0xff;
        sb.message.state_root = tampered;

        let err = state.state_transition(&sb, true).unwrap_err();
        assert!(matches!(
            err,
            StateTransitionError::StateRootMismatch { slot, got, want: w }
                if slot == Slot::new(1) && got == want && w == tampered
        ));
    }

    #[test]
    fn state_root_validation_off_skips_root_check() {
        let mut state = genesis_state(4);
        let mut sb = build_signed_block(&state, Slot::new(1));
        sb.message.state_root.0[0] ^= 0xff;
        // With validation off the tampered root is ignored.
        state.state_transition(&sb, false).unwrap();
        assert_eq!(state.slot, Slot::new(1));
    }

    // -- Error propagation --------------------------------------------------

    #[test]
    fn propagates_block_header_error() {
        let mut state = genesis_state(4);
        let mut sb = build_signed_block(&state, Slot::new(1));
        sb.message.parent_root = Bytes32::new([0xab; 32]);
        let err = state.state_transition(&sb, true).unwrap_err();
        assert!(matches!(
            err,
            StateTransitionError::BlockParentRootMismatch { .. }
        ));
    }

    // -- Transactional behaviour -------------------------------------------

    #[test]
    fn error_path_leaves_state_unchanged_on_header_error() {
        // Pre-state is non-trivial: advance by one valid block first.
        let mut state = genesis_state(4);
        let sb0 = build_signed_block(&state, Slot::new(1));
        state.state_transition(&sb0, true).unwrap();
        let snapshot = state.clone();

        // Now attempt a block with a corrupted parent_root.
        let mut sb = build_signed_block(&state, Slot::new(2));
        sb.message.parent_root = Bytes32::new([0xab; 32]);
        let _ = state.state_transition(&sb, true).unwrap_err();
        assert_eq!(state, snapshot);
    }

    #[test]
    fn error_path_leaves_state_unchanged_on_state_root_mismatch() {
        // The most subtle path: process_attestations has already committed
        // its working copies before the post-state-root check fires.
        let mut state = genesis_state(4);
        let sb0 = build_signed_block(&state, Slot::new(1));
        state.state_transition(&sb0, true).unwrap();
        let snapshot = state.clone();

        let mut sb = build_signed_block(&state, Slot::new(2));
        sb.message.state_root.0[0] ^= 0xff;
        let err = state.state_transition(&sb, true).unwrap_err();
        assert!(matches!(
            err,
            StateTransitionError::StateRootMismatch { .. }
        ));
        assert_eq!(state, snapshot);
    }

    // -- Property tests ----------------------------------------------------

    proptest! {
        /// Same chain on two equal starting states yields equal post-states.
        #[test]
        fn determinism(
            chain_len in 1_usize..=8,
            num_validators in 1_u64..=8,
        ) {
            let genesis = genesis_state(num_validators);
            let chain = build_chain(&genesis, chain_len);

            let mut a = genesis.clone();
            let mut b = genesis;
            for sb in &chain {
                a.state_transition(sb, true).unwrap();
                b.state_transition(sb, true).unwrap();
            }
            prop_assert_eq!(a.hash_tree_root(), b.hash_tree_root());
        }

        /// Splitting a chain at any point and re-applying the second half
        /// yields the same post-state as applying the whole chain end-to-end.
        #[test]
        fn path_independence(
            chain_len in 2_usize..=8,
            split_seed in 1_usize..=7,
            num_validators in 1_u64..=8,
        ) {
            let split = split_seed.min(chain_len - 1);
            let genesis = genesis_state(num_validators);
            let chain = build_chain(&genesis, chain_len);

            let mut whole = genesis.clone();
            for sb in &chain {
                whole.state_transition(sb, true).unwrap();
            }

            let mut split_path = genesis;
            for sb in &chain[..split] {
                split_path.state_transition(sb, true).unwrap();
            }
            for sb in &chain[split..] {
                split_path.state_transition(sb, true).unwrap();
            }

            prop_assert_eq!(whole.hash_tree_root(), split_path.hash_tree_root());
        }

        /// SSZ round-trip on the post-state preserves byte-equality.
        #[test]
        fn ssz_roundtrip_on_post_state(
            chain_len in 1_usize..=4,
            num_validators in 1_u64..=4,
        ) {
            let genesis = genesis_state(num_validators);
            let chain = build_chain(&genesis, chain_len);
            let mut state = genesis;
            for sb in &chain {
                state.state_transition(sb, true).unwrap();
            }
            let bytes = encode(&state);
            let back: State = decode(&bytes).unwrap();
            prop_assert_eq!(state, back);
        }
    }
}
