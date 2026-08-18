//! Consensus [`State`] container plus its inner [`ProtocolConfig`].
//!
//! The native lean-rust state SSZ container declares 10 fields in order:
//!
//! 1. `config: ProtocolConfig` — fixed (16-byte container).
//! 2. `slot: Slot` — fixed 8 bytes.
//! 3. `latest_block_header: BlockHeader` — fixed 112 bytes.
//! 4. `latest_justified: Checkpoint` — fixed 40 bytes.
//! 5. `latest_finalized: Checkpoint` — fixed 40 bytes.
//! 6. `historical_block_hashes: List[Bytes32, HISTORICAL_ROOTS_LIMIT]` —
//!    variable.
//! 7. `justified_slots: Bitlist[HISTORICAL_ROOTS_LIMIT]` — variable.
//! 8. `validators: List[Validator, VALIDATOR_REGISTRY_LIMIT]` — variable.
//! 9. `justifications_roots: List[Bytes32, HISTORICAL_ROOTS_LIMIT]` —
//!    variable.
//! 10. `justifications_validators: Bitlist[JUSTIFICATIONS_VALIDATORS_LIMIT]`
//!     — variable.
//!
//! Field bounds are pinned to the [`config::DEVNET_CONFIG`] caps. The five
//! variable-length fields each contribute a 4-byte offset to the fixed
//! portion ([`STATE_FIXED_PART_LEN`] = 236 bytes).
//!
//! The hash-tree-root commits to all ten fields in this order; the
//! cross-client compatibility of that shape (and the genesis-interop
//! decoder for the compact form) lives in [`crate::ream`].

use std::collections::BTreeMap;

use ssz::merkleize::merkleize;
use ssz::{Decode, DecodeError, Encode, HashTreeRoot};
use types::{Bitlist, Bytes32};

use crate::block::{Block, BlockHeader, SignedBlockWithAttestation};
use crate::checkpoint::Checkpoint;
use crate::error::{AttSlotKind, StateTransitionError};
use crate::internal::{
    bitlist_hash_tree_root, decode_bytes32_list, decode_fixed_element_list, encode_bytes32_list,
    encode_fixed_element_list, ensure_len, list_hash_tree_root, read_fixed, read_offset, u64_chunk,
    write_offset, BLOCK_HEADER_LEN, BYTES32_LEN, BYTES_PER_LENGTH_OFFSET, CHECKPOINT_LEN, SLOT_LEN,
    U64_LEN, VALIDATOR_SSZ_LEN,
};
use crate::slot::Slot;
use crate::validator::{is_proposer, Validator, Validators};
use crate::vote::Attestation;

/// Maximum number of historical block roots retained in the state.
///
/// Pinned to [`config::DEVNET_CONFIG::historical_roots_limit`] (`262_144` on
/// devnet0).
#[allow(clippy::cast_possible_truncation)]
pub const HISTORICAL_ROOTS_LIMIT: usize = config::DEVNET_CONFIG.historical_roots_limit as usize;

/// Maximum validator-registry size; bounds the `validators` registry and
/// per-root vote bitlists.
///
/// Aliases [`config::VALIDATOR_REGISTRY_LIMIT`] — the single-source cap
/// (`4_096` on devnet0). Re-exported from the crate root; consumed by
/// [`JUSTIFICATIONS_VALIDATORS_LIMIT`] and the `validators` codec/HTR.
pub const VALIDATOR_REGISTRY_LIMIT: usize = config::VALIDATOR_REGISTRY_LIMIT;

/// Bound on the flattened validator-vote bitlist:
/// [`HISTORICAL_ROOTS_LIMIT`] × [`VALIDATOR_REGISTRY_LIMIT`].
///
/// Equals `262_144 * 4_096 = 1_073_741_824` on devnet0.
pub const JUSTIFICATIONS_VALIDATORS_LIMIT: usize =
    HISTORICAL_ROOTS_LIMIT * VALIDATOR_REGISTRY_LIMIT;

// `pub(crate)` so the sibling [`crate::ream`] module can reuse them; both
// are also consumed by `STATE_FIXED_PART_LEN` below.
pub(crate) const PROTOCOL_CONFIG_SSZ_LEN: usize = U64_LEN; // 8
pub(crate) const STATE_VARIABLE_FIELD_COUNT: usize = 5;

/// Length of the fixed portion of a [`State`] (5 fixed fields plus 5 offsets
/// for the variable-length tails).
pub const STATE_FIXED_PART_LEN: usize = PROTOCOL_CONFIG_SSZ_LEN
    + SLOT_LEN
    + BLOCK_HEADER_LEN
    + CHECKPOINT_LEN
    + CHECKPOINT_LEN
    + STATE_VARIABLE_FIELD_COUNT * BYTES_PER_LENGTH_OFFSET; // 228

// =====================================================================
// ProtocolConfig (the inner `config` field of State)
// =====================================================================

/// In-state runtime parameters carried by the consensus [`State`].
///
/// One fixed-size `u64` field → 8-byte SSZ payload. Distinct from the
/// chain-wide [`config::Config`] preset: this container records only the
/// chain genesis time committed to the state hash-tree-root.
///
/// The validator-set size is NOT carried here — it is derived from
/// [`State::num_validators`], the validator-registry length.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ProtocolConfig {
    /// Unix timestamp (seconds) of chain genesis.
    pub genesis_time: u64,
}

impl ProtocolConfig {
    /// Builds the in-state runtime parameters from the chain genesis time.
    ///
    /// # Example
    /// ```
    /// use protocol::ProtocolConfig;
    ///
    /// let cfg = ProtocolConfig::new(1_700_000_000);
    /// assert_eq!(cfg.genesis_time, 1_700_000_000);
    /// ```
    #[must_use]
    pub const fn new(genesis_time: u64) -> Self {
        Self { genesis_time }
    }
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
            genesis_time: read_fixed::<u64>(bytes, &mut c)?,
        })
    }
}

impl HashTreeRoot for ProtocolConfig {
    fn hash_tree_root(&self) -> [u8; 32] {
        // 1 field → width 1 → `merkleize` returns the chunk itself, which is
        // the SSZ root of a single-field container.
        merkleize(&[u64_chunk(self.genesis_time)])
    }
}

// =====================================================================
// State
// =====================================================================

/// Consensus state container.
///
/// Variable-length SSZ container: the five list/bitlist tails follow the
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
    /// Bounded validator registry (`List[Validator, VALIDATOR_REGISTRY_LIMIT]`).
    pub validators: Validators,
    /// Bounded list of roots whose per-validator vote bitlist is tracked.
    pub justifications_roots: Vec<Bytes32>,
    /// Flattened per-validator vote bitlist for [`Self::justifications_roots`].
    pub justifications_validators: Bitlist<JUSTIFICATIONS_VALIDATORS_LIMIT>,
}

impl State {
    /// Number of validators in the registry — the single source of the
    /// validator-set size.
    ///
    /// The consensus spec derives every validator-count bound from the length
    /// of the validator registry; the in-state `config` container carries only
    /// `genesis_time`. Reading the registry here keeps the count and the
    /// entries it describes from drifting apart.
    ///
    /// Saturates at `u64::MAX` for a registry longer than `u64` can index.
    /// [`VALIDATOR_REGISTRY_LIMIT`] makes that unreachable; the saturation
    /// exists so the accessor is infallible at every call site.
    ///
    /// # Example
    /// ```
    /// use protocol::State;
    ///
    /// let state = State::default();
    /// assert_eq!(state.num_validators(), 0);
    /// ```
    #[must_use]
    pub fn num_validators(&self) -> u64 {
        u64::try_from(self.validators.len()).unwrap_or(u64::MAX)
    }

    /// Returns the five variable-length tail payloads encoded into their wire
    /// bytes, in declaration order.
    fn variable_tail_payloads(&self) -> [Vec<u8>; STATE_VARIABLE_FIELD_COUNT] {
        let mut historical_buf =
            Vec::with_capacity(self.historical_block_hashes.len() * BYTES32_LEN);
        encode_bytes32_list(&self.historical_block_hashes, &mut historical_buf);

        let mut validators_buf = Vec::with_capacity(self.validators.len() * VALIDATOR_SSZ_LEN);
        encode_fixed_element_list(&self.validators, &mut validators_buf);

        let mut roots_buf = Vec::with_capacity(self.justifications_roots.len() * BYTES32_LEN);
        encode_bytes32_list(&self.justifications_roots, &mut roots_buf);

        [
            historical_buf,
            self.justified_slots.as_bytes(),
            validators_buf,
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
        let validators =
            decode_fixed_element_list::<Validator>(tail_slice(2), VALIDATOR_REGISTRY_LIMIT)?;
        let justifications_roots = decode_bytes32_list(tail_slice(3), HISTORICAL_ROOTS_LIMIT)?;
        let justifications_validators = Bitlist::<JUSTIFICATIONS_VALIDATORS_LIMIT>::from_bytes(
            tail_slice(4),
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
            validators,
            justifications_roots,
            justifications_validators,
        })
    }
}

impl HashTreeRoot for State {
    fn hash_tree_root(&self) -> [u8; 32] {
        // Native lean state root: 10 fields → merkleize width 16.
        merkleize(&[
            self.config.hash_tree_root(),
            self.slot.hash_tree_root(),
            self.latest_block_header.hash_tree_root(),
            self.latest_justified.hash_tree_root(),
            self.latest_finalized.hash_tree_root(),
            list_hash_tree_root(&self.historical_block_hashes, HISTORICAL_ROOTS_LIMIT),
            bitlist_hash_tree_root(&self.justified_slots),
            list_hash_tree_root(&self.validators, VALIDATOR_REGISTRY_LIMIT),
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
    ///   [`is_proposer`] when the validator registry is empty.
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
        if !is_proposer(block.proposer_index, self.slot, self.num_validators())? {
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
    /// Cached validator-registry length as a `usize`.
    num_validators: usize,
}

impl TryFrom<&State> for Justifications {
    type Error = StateTransitionError;

    /// Hydrates the working view from `state.justifications_*`.
    ///
    /// Returns [`StateTransitionError::StateBoundExceeded`] when the flat
    /// bitlist length is not a multiple of the validator-registry length
    /// (i.e. an on-state invariant break). The registry length is already a
    /// `usize`, so no fallible conversion is involved.
    fn try_from(state: &State) -> Result<Self, Self::Error> {
        let n = state.validators.len();

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
    /// mismatch, target at or before its source, target not justifiable
    /// after the finalized slot) cause the offending vote to be silently
    /// skipped: the rest of the batch still applies, and so does the block
    /// that carried it. A skipped vote contributes no weight and moves
    /// neither justification nor finalization.
    ///
    /// That list is NOT yet the reference's full set, so do not read it as
    /// closed. The reference additionally skips a vote whose source or target
    /// root is all-zero; this loop does not. A slot no block was proposed for
    /// holds an all-zero entry in `historical_block_hashes`, so an all-zero
    /// target root can satisfy the history comparison above. Tracked
    /// separately.
    ///
    /// All mutation is staged in working copies and committed atomically
    /// after the loop, so an `Err` return leaves the state byte-equal to
    /// its pre-call value.
    ///
    /// The finalized checkpoint is deliberately *live* inside the loop: a
    /// vote that advances finalization changes the justifiability window
    /// every later vote in the same batch is tested against. Acceptance is
    /// therefore sensitive to the order of `attestations`, matching the
    /// reference specification, which reassigns its own `finalized_slot`
    /// local mid-loop for the same reason. Neither read may be hoisted to a
    /// pre-loop snapshot. Two tests pin the reads and a third pins the
    /// complementary case:
    /// `process_attestations_is_order_dependent_when_finalization_advances`
    /// covers the vote-eligibility read,
    /// `process_attestations_finalization_scan_reads_the_live_checkpoint`
    /// covers the read inside the finalization scan, and
    /// `process_attestations_is_order_independent_without_finalization_advance`
    /// covers the batches whose order genuinely does not matter.
    ///
    /// # Errors
    /// - [`StateTransitionError::AttestationSlotOutOfRange`] when a vote
    ///   references a slot beyond `state.justified_slots.len()` or
    ///   `state.historical_block_hashes.len()`.
    /// - [`StateTransitionError::AttestationValidatorOutOfRange`] when
    ///   `validator_id` is past the validator-registry length.
    /// - [`StateTransitionError::StateBoundExceeded`] forwarded from the
    ///   working bitmap rebuild.
    pub fn process_attestations(
        &mut self,
        attestations: &[Attestation],
    ) -> Result<(), StateTransitionError> {
        // Same quantity in two widths: the `u64` form is what the error
        // variant carries, the `usize` form is what indexes the vote vector.
        let num_validators = self.num_validators();
        let validator_limit = self.validators.len();
        let just_len = self.justified_slots.len();
        let hist_len = self.historical_block_hashes.len();

        // Working copies — committed at end if every iteration succeeds.
        let mut justifications = Justifications::try_from(&*self)?;
        let mut justified_slots = self.justified_slots.clone();
        let mut latest_justified = self.latest_justified;
        let mut latest_finalized = self.latest_finalized;

        for att in attestations {
            let vote = &att.data;
            let validator_id = att.validator_id;
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
    /// `hash_tree_root` equals `signed_block.message.block.state_root`.
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
    ///   signed_block.message.block.state_root`.
    pub fn state_transition(
        &mut self,
        signed_block: &SignedBlockWithAttestation,
        validate_state_root: bool,
    ) -> Result<(), StateTransitionError> {
        let block = &signed_block.message.block;
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

    use crate::test_fixtures::{
        assert_htr_eq, assert_ssz_round_trip, regen_vector, sample_block_header, sample_validator,
        sample_validators,
    };

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
            config: ProtocolConfig::new(1_700_000_000),
            slot: Slot::new(9),
            latest_block_header: sample_block_header(),
            latest_justified: Checkpoint::new(Bytes32::new([0x44; 32]), Slot::new(8)),
            latest_finalized: Checkpoint::new(Bytes32::new([0x55; 32]), Slot::new(0)),
            historical_block_hashes: vec![Bytes32::new([0xaa; 32]), Bytes32::new([0xbb; 32])],
            justified_slots,
            validators: sample_validators(2),
            justifications_roots: vec![Bytes32::new([0xcc; 32]), Bytes32::new([0xdd; 32])],
            justifications_validators,
        }
    }

    // -- Constants ----------------------------------------------------------

    #[test]
    fn fixed_part_is_two_twenty_eight_bytes() {
        assert_eq!(STATE_FIXED_PART_LEN, 228);
        assert_eq!(STATE_VARIABLE_FIELD_COUNT, 5);
    }

    #[test]
    fn limits_match_devnet_config() {
        assert_eq!(HISTORICAL_ROOTS_LIMIT, 262_144);
        assert_eq!(VALIDATOR_REGISTRY_LIMIT, 4_096);
        assert_eq!(JUSTIFICATIONS_VALIDATORS_LIMIT, 262_144 * 4_096);
    }

    // -- Validator count ----------------------------------------------------

    #[test]
    fn num_validators_matches_registry_length() {
        for n in [0_u8, 1, 4] {
            let state = State {
                validators: sample_validators(n),
                ..State::default()
            };
            assert_eq!(state.num_validators(), u64::from(n), "registry of {n}");
        }
    }

    // -- ProtocolConfig SSZ -------------------------------------------------

    #[test]
    fn protocol_config_ssz_fixed_len_is_eight() {
        assert_eq!(<ProtocolConfig as Encode>::ssz_fixed_len(), 8);
        assert!(<ProtocolConfig as Encode>::is_ssz_fixed_len());
    }

    #[test]
    fn protocol_config_round_trip() {
        let cfg = ProtocolConfig::new(0x1234_5678);
        let bytes = encode(&cfg);
        assert_eq!(bytes.len(), PROTOCOL_CONFIG_SSZ_LEN);
        assert_eq!(&bytes[..8], &0x1234_5678_u64.to_le_bytes());
        let back: ProtocolConfig = decode(&bytes).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn protocol_config_decode_rejects_wrong_length() {
        assert!(decode::<ProtocolConfig>(&[0_u8; 7]).is_err());
        assert!(decode::<ProtocolConfig>(&[0_u8; 9]).is_err());
    }

    #[test]
    fn protocol_config_hash_tree_root_is_the_genesis_time_chunk() {
        // A one-field container merkleizes to its single chunk: the field's
        // little-endian bytes, zero-padded to 32. That is what a peer computes,
        // so this pins the shape, not just self-consistency.
        let cfg = ProtocolConfig::new(0x1234_5678);
        let mut want = [0_u8; 32];
        want[..8].copy_from_slice(&0x1234_5678_u64.to_le_bytes());
        assert_eq!(cfg.hash_tree_root(), want);
    }

    // -- State SSZ ---------------------------------------------------------

    #[test]
    fn state_default_round_trip() {
        let s = State::default();
        let bytes = encode(&s);
        // Each empty Bitlist encodes to a single delimiter byte (0x01); the
        // empty Vec<Bytes32> and empty validators tails are zero-length.
        // Total = 236 + 0 (historical) + 1 (justified_slots) + 0 (validators)
        // + 0 (roots) + 1 (justifications_validators).
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
        let off_pos = STATE_FIXED_PART_LEN - 20;
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
        let off_pos = STATE_FIXED_PART_LEN - 20;
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
        let off0_pos = STATE_FIXED_PART_LEN - 20;
        let off1_pos = STATE_FIXED_PART_LEN - 16;
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
    // The all-ten-fields responsiveness check that documents cross-client
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

    // -- Validator registry ------------------------------------------------

    #[test]
    fn state_with_validators_round_trips() {
        let mut s = sample_state();
        s.validators = sample_validators(3);
        assert_ssz_round_trip(&s);
    }

    #[test]
    fn state_root_changes_when_validators_change() {
        let baseline = sample_state();
        let mut extended = sample_state();
        extended.validators.push(sample_validator(9));
        assert_ne!(extended.hash_tree_root(), baseline.hash_tree_root());
    }

    #[test]
    fn state_root_regression_vector() {
        // Frozen native State root. Regenerate by printing
        // test_fixtures::regen_vector(&sample_state()) — never hand-derive.
        //
        // Moved when the in-state config dropped `num_validators`: the config
        // container went from two fields to one, so its subtree root and the
        // whole state root change. The validator count now comes from the
        // registry.
        const STATE_ROOT: [u8; 32] = [
            0xbe, 0xe5, 0xae, 0x6b, 0x5e, 0x42, 0x72, 0xdb, 0xd8, 0xfc, 0x54, 0x42, 0xf1, 0xc9,
            0x01, 0x67, 0x08, 0xae, 0xcc, 0x75, 0x32, 0xa8, 0xdd, 0xfc, 0x21, 0x06, 0x91, 0x50,
            0x3c, 0x58, 0x7c, 0xfb,
        ];
        let state = sample_state();
        assert_htr_eq(&state, STATE_ROOT);

        // The SA2 vector contract: the frozen bytes decode back to the same
        // value, so the (bytes, root) pair is self-consistent.
        let (bytes, root) = regen_vector(&state);
        assert_eq!(root, STATE_ROOT);
        let back: State = decode(&bytes).unwrap();
        assert_eq!(back, state);
    }

    #[test]
    fn decode_rejects_over_cap_registry() {
        // A validators tail encoding one element past the registry cap is
        // rejected by the shared fixed-element-list codec (single-source cap).
        let over_cap = vec![0_u8; (VALIDATOR_REGISTRY_LIMIT + 1) * VALIDATOR_SSZ_LEN];
        let err = decode_fixed_element_list::<Validator>(&over_cap, VALIDATOR_REGISTRY_LIMIT)
            .unwrap_err();
        assert!(matches!(err, DecodeError::BytesInvalid(_)));
    }

    // -- property tests ----------------------------------------------------

    proptest! {
        #[test]
        fn protocol_config_round_trips(
            ts in any::<u64>(),
        ) {
            let cfg = ProtocolConfig::new(ts);
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
            config: ProtocolConfig::new(0),
            validators: crate::test_fixtures::registry_of(num_validators),
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

    use crate::checkpoint::Checkpoint;
    use crate::validator::ValidatorIndex;
    use crate::vote::{Attestation, AttestationData};

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
            config: ProtocolConfig::new(0),
            validators: crate::test_fixtures::registry_of(num_validators),
            slot: Slot::new(historical_roots.len() as u64),
            latest_finalized: Checkpoint::new(Bytes32::zero(), latest_finalized_slot),
            historical_block_hashes: historical_roots,
            justified_slots,
            ..State::default()
        }
    }

    /// Shared with `state_transition_tests`, which builds the same votes at
    /// block level. `head` is set equal to `target` because
    /// `process_attestations` never reads it.
    pub(super) fn attestation(
        validator_id: u64,
        source_root: Bytes32,
        source_slot: u64,
        target_root: Bytes32,
        target_slot: u64,
    ) -> Attestation {
        Attestation {
            validator_id: ValidatorIndex::new(validator_id),
            data: AttestationData {
                slot: Slot::new(target_slot),
                head: Checkpoint::new(target_root, Slot::new(target_slot)),
                target: Checkpoint::new(target_root, Slot::new(target_slot)),
                source: Checkpoint::new(source_root, Slot::new(source_slot)),
            },
        }
    }

    pub(super) fn root(byte: u8) -> Bytes32 {
        Bytes32::new([byte; 32])
    }

    // -- Range checks: aborting paths ---------------------------------------

    #[test]
    fn out_of_range_source_slot_aborts() {
        let mut state = populated_state(4, vec![root(0xaa)], &[true], Slot::ZERO);
        let votes = vec![attestation(0, root(0xaa), 5, root(0xbb), 6)];
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
        let votes = vec![attestation(0, root(0xaa), 0, root(0xcc), 9)];
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
        let votes = vec![attestation(99, root(0xaa), 0, root(0xbb), 1)];
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
        let votes = vec![attestation(99, root(0xaa), 0, root(0xbb), 1)];
        let _ = state.process_attestations(&votes).unwrap_err();
        assert_eq!(state, snapshot);
    }

    // -- Semantic filters: skip paths --------------------------------------

    #[test]
    fn skips_when_source_not_justified() {
        let mut state =
            populated_state(4, vec![root(0xaa), root(0xbb)], &[false, false], Slot::ZERO);
        let snapshot = state.clone();
        let votes = vec![attestation(0, root(0xaa), 0, root(0xbb), 1)];
        state.process_attestations(&votes).unwrap();
        assert_eq!(state, snapshot);
    }

    #[test]
    fn skips_when_target_already_justified() {
        let mut state = populated_state(4, vec![root(0xaa), root(0xbb)], &[true, true], Slot::ZERO);
        let snapshot = state.clone();
        let votes = vec![attestation(0, root(0xaa), 0, root(0xbb), 1)];
        state.process_attestations(&votes).unwrap();
        assert_eq!(state, snapshot);
    }

    #[test]
    fn skips_when_source_root_mismatch() {
        let mut state =
            populated_state(4, vec![root(0xaa), root(0xbb)], &[true, false], Slot::ZERO);
        let snapshot = state.clone();
        let votes = vec![attestation(0, root(0xff), 0, root(0xbb), 1)];
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
        let votes = vec![attestation(0, root(0xaa), 0, root(0xaa), 0)];
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
        let votes = vec![attestation(0, root(0), 0, root(7), 7)];
        state.process_attestations(&votes).unwrap();
        assert_eq!(state, snapshot);
    }

    #[test]
    fn justifiable_boundary_counts_pronic_six_and_skips_seven() {
        // delta = 6 is pronic (2*3), so the target IS justifiable. delta = 7 is
        // neither within the small window nor a square nor pronic, and is the
        // SMALLEST non-justifiable distance. Every other filter sees identical
        // input on both rows, so the pair isolates the justifiability filter
        // and nothing else.
        //
        // `skips_when_target_not_justifiable` above covers the same distance-7
        // vote and stays: it asserts whole-state equality, which is a stronger
        // claim than this row's tally check. The row exists so the counted case
        // has a counterpart on an identical fixture.
        struct Case {
            name: &'static str,
            /// Doubles as the history index, so `root(target_slot)` is the root
            /// recorded for that slot. `u8` because that is what `root` takes.
            target_slot: u8,
            counted: bool,
        }
        let cases = [
            Case {
                name: "delta 6 is pronic",
                target_slot: 6,
                counted: true,
            },
            Case {
                name: "delta 7 is neither square nor pronic",
                target_slot: 7,
                counted: false,
            },
        ];

        for case in cases {
            let history: Vec<Bytes32> = (0_u8..8).map(root).collect();
            let mut pattern = vec![false; 8];
            pattern[0] = true;
            let mut state = populated_state(4, history, &pattern, Slot::ZERO);
            let target_root = root(case.target_slot);
            let votes = vec![attestation(
                0,
                root(0),
                0,
                target_root,
                u64::from(case.target_slot),
            )];

            state.process_attestations(&votes).unwrap();

            if case.counted {
                assert_eq!(
                    state.justifications_roots,
                    vec![target_root],
                    "case {}: the vote must be tallied",
                    case.name,
                );
                assert_eq!(
                    state.justifications_validators.get(0),
                    Some(true),
                    "case {}: validator 0's bit must be set",
                    case.name,
                );
            } else {
                assert!(
                    state.justifications_roots.is_empty(),
                    "case {}: the vote must be skipped",
                    case.name,
                );
            }
        }
    }

    #[test]
    fn target_at_finalized_slot_follows_the_justification_bit() {
        // The reference treats every slot at or before the finalized boundary as
        // implicitly justified, so a target there is skipped by the
        // already-justified filter and never reaches the justifiability filter
        // at all. This client indexes its justification bitlist absolutely and
        // READS the bit, so the two arms below differ: with the bit set the vote
        // is skipped as the reference skips it; with the bit clear the vote is
        // tallied, because distance zero IS justifiable.
        //
        // Arm 2 is LATENT rather than live, but only along the state-transition
        // path. No state this module PRODUCES has an unjustified finalized slot:
        // `latest_finalized` is only ever assigned a vote's source, and a vote
        // reaches that assignment only past the source-is-justified filter; bits
        // are only ever raised, never cleared; and the genesis block raises
        // index 0 while leaving the finalized slot at zero.
        //
        // A state this module DECODES is a different matter. `from_ssz_bytes`
        // reconstructs `latest_finalized` and `justified_slots` as independent
        // fields with no cross-field check, so a decoded state — resumed from
        // storage, or read from a genesis state file — can carry the combination
        // arm 2 describes. That is the third trigger, alongside a
        // finalized-anchored justification window and a checkpoint-sync entry
        // point, and it exists today.
        //
        // So arm 2 is pinned to keep the divergence from changing silently. It
        // is not evidence of a defect a peer can reach through block import.
        for finalized_slot_is_justified in [true, false] {
            let history: Vec<Bytes32> = (0_u8..8).map(root).collect();
            let mut pattern = vec![false; 8];
            pattern[0] = true;
            pattern[3] = finalized_slot_is_justified;
            let mut state = populated_state(4, history, &pattern, Slot::new(3));
            let votes = vec![attestation(0, root(0), 0, root(3), 3)];

            state.process_attestations(&votes).unwrap();

            if finalized_slot_is_justified {
                assert!(
                    state.justifications_roots.is_empty(),
                    "justified: the already-justified filter fires first",
                );
            } else {
                assert_eq!(
                    state.justifications_roots,
                    vec![root(3)],
                    "not justified: distance zero is justifiable, so the vote is tallied",
                );
            }
        }
    }

    // -- Tally and supermajority --------------------------------------------

    #[test]
    fn single_subthreshold_vote_does_not_justify() {
        let mut state =
            populated_state(4, vec![root(0xaa), root(0xbb)], &[true, false], Slot::ZERO);
        let votes = vec![attestation(0, root(0xaa), 0, root(0xbb), 1)];
        state.process_attestations(&votes).unwrap();
        assert_eq!(state.justified_slots.get(1), Some(false));
        assert_eq!(state.justifications_roots, vec![root(0xbb)]);
        assert_eq!(state.justifications_validators.len(), 4);
        assert_eq!(state.justifications_validators.get(0), Some(true));
        assert_eq!(state.justifications_validators.get(1), Some(false));
    }

    #[test]
    fn attestation_bound_uses_registry_length() {
        // Pins the bound's exact value at the boundary: with a 2-entry
        // registry, id 3 is out of range and the error reports
        // `num_validators: 2`. `out_of_range_validator_aborts` uses a far
        // out-of-range id, so it would still pass under an off-by-one bound.
        let mut state =
            populated_state(2, vec![root(0xaa), root(0xbb)], &[true, false], Slot::ZERO);
        let votes = vec![attestation(3, root(0xaa), 0, root(0xbb), 1)];
        let err = state.process_attestations(&votes).unwrap_err();
        assert_eq!(
            err,
            StateTransitionError::AttestationValidatorOutOfRange {
                validator: ValidatorIndex::new(3),
                num_validators: 2,
            }
        );
    }

    #[test]
    fn supermajority_threshold_uses_registry_length() {
        // Exercises EXACT division in the supermajority: `ceil(2*3/3) == 2`,
        // so two of three votes justify. The existing
        // `supermajority_justifies_target` uses 4 validators, where the same
        // formula rounds up — the two cases pin different arms of `div_ceil`.
        let mut state =
            populated_state(3, vec![root(0xaa), root(0xbb)], &[true, false], Slot::ZERO);
        let votes = vec![
            attestation(0, root(0xaa), 0, root(0xbb), 1),
            attestation(1, root(0xaa), 0, root(0xbb), 1),
        ];
        state.process_attestations(&votes).unwrap();
        assert_eq!(state.justified_slots.get(1), Some(true));
        assert_eq!(state.latest_justified.root, root(0xbb));
    }

    #[test]
    fn supermajority_justifies_target() {
        let mut state =
            populated_state(4, vec![root(0xaa), root(0xbb)], &[true, false], Slot::ZERO);
        let votes = vec![
            attestation(0, root(0xaa), 0, root(0xbb), 1),
            attestation(1, root(0xaa), 0, root(0xbb), 1),
            attestation(2, root(0xaa), 0, root(0xbb), 1),
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
            attestation(0, root(0xaa), 0, root(0xbb), 1),
            attestation(1, root(0xaa), 0, root(0xbb), 1),
            attestation(2, root(0xaa), 0, root(0xbb), 1),
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
            attestation(0, root(0), 0, root(9), 9),
            attestation(1, root(0), 0, root(9), 9),
            attestation(2, root(0), 0, root(9), 9),
        ];
        state.process_attestations(&votes).unwrap();
        assert_eq!(state.justified_slots.get(9), Some(true));
        assert_eq!(state.latest_finalized, original_finalized);
    }

    #[test]
    fn duplicate_vote_for_same_validator_is_idempotent() {
        let mut once = populated_state(4, vec![root(0xaa), root(0xbb)], &[true, false], Slot::ZERO);
        let mut twice = once.clone();
        let votes = vec![attestation(0, root(0xaa), 0, root(0xbb), 1)];
        once.process_attestations(&votes).unwrap();
        let votes_twice = vec![votes[0], votes[0]];
        twice.process_attestations(&votes_twice).unwrap();
        assert_eq!(once.hash_tree_root(), twice.hash_tree_root());
    }

    // -- Batch ordering -----------------------------------------------------

    /// Three validators (so a supermajority is two votes) and `history_len`
    /// slots of history whose root at slot `i` is `root(i)`, finalized at
    /// slot 0 with `pre_justified` slots already justified.
    ///
    /// Shared by the ordering tests so that they differ only in the batch
    /// they feed and the property they assert.
    fn ordering_state(history_len: u8, pre_justified: &[usize]) -> State {
        let history: Vec<Bytes32> = (0..history_len).map(root).collect();
        let mut pattern = vec![false; usize::from(history_len)];
        for &slot in pre_justified {
            pattern[slot] = true;
        }
        populated_state(3, history, &pattern, Slot::ZERO)
    }

    /// One vote from `validator_id`, sourced at `source_slot` and targeting
    /// `target_slot`.
    ///
    /// Both roots are derived from the slot, matching the history
    /// [`ordering_state`] builds, so a vote always agrees with the chain and
    /// the two cannot drift apart at a call site.
    fn ordering_vote(validator_id: u64, source_slot: u8, target_slot: u8) -> Attestation {
        attestation(
            validator_id,
            root(source_slot),
            u64::from(source_slot),
            root(target_slot),
            u64::from(target_slot),
        )
    }

    /// The two identical votes that carry a target to supermajority in the
    /// three-validator registry [`ordering_state`] builds.
    fn ordering_supermajority(source_slot: u8, target_slot: u8) -> [Attestation; 2] {
        [
            ordering_vote(0, source_slot, target_slot),
            ordering_vote(1, source_slot, target_slot),
        ]
    }

    /// `state` with `batch` applied, leaving the caller's copy untouched so
    /// one pre-state can seed several orderings.
    fn after(state: &State, batch: &[Attestation]) -> State {
        let mut post = state.clone();
        post.process_attestations(batch).unwrap();
        post
    }

    #[test]
    fn process_attestations_is_order_dependent_when_finalization_advances() {
        // Pins the eligibility read against the reference specification,
        // which reassigns its own finalized-slot local mid-loop
        // (containers/state/state.py:543-:545 at 0c9528ac) and reads it in
        // the justifiability guard at :486.
        //
        // The finalized checkpoint is read live inside the loop, so a vote
        // that advances finalization widens the justifiability window for
        // every later vote in the same batch. Slot 7 is the discriminator:
        // `Slot(7).is_justifiable_after(Slot(0))` is false (delta 7 is
        // neither a perfect square nor a pronic, and exceeds the delta <= 5
        // fast path) while `Slot(7).is_justifiable_after(Slot(1))` is true
        // (delta 6 == 2 * 3 is pronic).
        let state = ordering_state(8, &[0, 1]);

        // Justifies slot 2 from the slot-1 source, finalizing slot 1.
        let advance = ordering_supermajority(1, 2);
        // Targets slot 7 from the slot-0 source — acceptable only once
        // finalization has reached slot 1.
        let late = ordering_supermajority(0, 7);

        let widened = after(&state, &[advance, late].concat());
        let narrow = after(&state, &[late, advance].concat());

        // Both orderings finalize slot 1, so the batches agree on everything
        // except what the slot-7 votes were measured against.
        assert_eq!(widened.latest_finalized.slot, Slot::new(1));
        assert_eq!(narrow.latest_finalized.slot, Slot::new(1));
        assert_eq!(widened.justified_slots.get(7), Some(true));
        assert_eq!(narrow.justified_slots.get(7), Some(false));
        // Names the checkpoint each ordering settled on, so a regression
        // reports the property that broke rather than a bare bit index.
        assert_eq!(widened.latest_justified.slot, Slot::new(7));
        assert_eq!(narrow.latest_justified.slot, Slot::new(2));
        assert_ne!(widened.hash_tree_root(), narrow.hash_tree_root());
    }

    #[test]
    fn process_attestations_is_order_independent_without_finalization_advance() {
        const ORDERINGS: [[usize; 3]; 6] = [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ];

        // Pins the complementary case: with the finalized slot unmoved,
        // batch order cannot matter. The specification's finalized-slot
        // local is only reassigned on a finalization advance
        // (containers/state/state.py:543-:545).
        //
        // Justifying slot 2 from the slot-0 source cannot finalize: slot 1
        // lies strictly between them and is justifiable after slot 0. The
        // finalized checkpoint therefore never moves, and no vote can change
        // the window another vote is tested against.
        let state = ordering_state(8, &[0]);
        let votes = [
            ordering_vote(0, 0, 2),
            ordering_vote(1, 0, 2),
            ordering_vote(2, 0, 3),
        ];

        let mut post_roots = Vec::with_capacity(ORDERINGS.len());
        for ordering in ORDERINGS {
            let batch: Vec<Attestation> = ordering.iter().map(|&i| votes[i]).collect();
            let permuted = after(&state, &batch);

            // Non-vacuity: the batch justifies a target and leaves a pending
            // tally behind, so equality across orderings is a real property
            // and not the trivial agreement of six no-op batches.
            assert_eq!(permuted.justified_slots.get(2), Some(true));
            assert_eq!(permuted.justifications_roots, vec![root(3)]);
            assert_eq!(permuted.latest_finalized, state.latest_finalized);

            post_roots.push(permuted.hash_tree_root());
        }
        assert!(post_roots.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn process_attestations_finalization_scan_reads_the_live_checkpoint() {
        // Pins the read inside the finalization scan, the counterpart of
        // the specification's `not any(... is_justifiable_after(
        // finalized_slot))` at containers/state/state.py:539-:541.
        //
        // Isolates the finalized read inside the no-intermediate scan. The
        // two tests above cannot reach it: their scan ranges are either
        // empty or short-circuit identically under a frozen read.
        //
        // Slot 12 is justifiable after both slot 0 (delta 12 == 3 * 4,
        // pronic) and slot 6 (delta 6 == 2 * 3, pronic), so the vote passes
        // the eligibility predicate under either reading and only the scan
        // can discriminate.
        let state = ordering_state(13, &[0, 6]);
        let post = after(
            &state,
            &[
                // Justify slot 9 and finalize slot 6: the scan range 7..9
                // holds no slot justifiable after slot 0.
                ordering_supermajority(6, 9),
                // Justify slot 12. The scan range 10..12 holds slots that
                // ARE justifiable after the live finalized slot 6 (deltas 4
                // and 5, inside the fast path), so finalization must not
                // advance. Against a frozen slot 0 neither slot is
                // justifiable and finalization would wrongly reach slot 9.
                ordering_supermajority(9, 12),
            ]
            .concat(),
        );

        assert_eq!(post.latest_justified.slot, Slot::new(12));
        assert_eq!(post.latest_finalized.slot, Slot::new(6));
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
        State {
            config: ProtocolConfig::new(GENESIS_TIME),
            validators: crate::test_fixtures::registry_of(NUM_VALIDATORS),
            latest_block_header: BlockHeader::genesis(),
            ..State::default()
        }
    }

    /// Produces a valid block for `state` at `state.slot` whose body is empty.
    fn valid_block_for(state: &State) -> Block {
        let parent_root: Bytes32 = state.latest_block_header.hash_tree_root().into();
        let proposer_index = ValidatorIndex::new(state.slot.get() % state.num_validators());
        Block {
            slot: state.slot,
            proposer_index,
            parent_root,
            state_root: Bytes32::zero(),
            body: BlockBody::default(),
        }
    }

    // -- Validator count comes from the registry ----------------------------

    #[test]
    fn proposer_check_uses_registry_length() {
        // Pins the modulus and its wrap-around: with a 4-entry registry the
        // proposer for slot 5 is `5 % 4 == 1`, and validator 5 — an index the
        // registry does not contain — is rejected. Asserts both directions;
        // `incorrect_proposer_rejects` covers only the negative case.
        let mut state = genesis();
        state.process_slots(Slot::new(5)).unwrap();

        let mut block = valid_block_for(&state);
        block.proposer_index = ValidatorIndex::new(1);
        state.clone().process_block_header(&block).unwrap();

        block.proposer_index = ValidatorIndex::new(5);
        let err = state.process_block_header(&block).unwrap_err();
        assert_eq!(
            err,
            StateTransitionError::IncorrectBlockProposer {
                slot: Slot::new(5),
                proposer: ValidatorIndex::new(5),
            }
        );
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
        // Empty the REGISTRY, not a scalar: the registry is the validator-set
        // size, so this is what makes the round-robin modulus zero.
        state.validators.clear();
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

    /// Minimal fixture: a non-default `State` whose `latest_block_header`
    /// commits to the empty `BlockBody`. Mirrors the slot-0 shape used by
    /// `crate::stf::genesis_state` without going through the module path.
    fn fresh_state() -> State {
        State {
            latest_block_header: BlockHeader::genesis(),
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

    use crate::block::{BlockBody, BlockSignatures, BlockWithAttestation};
    use crate::validator::ValidatorIndex;

    const GENESIS_TIME: u64 = 1_700_000_000;

    fn genesis_state(num_validators: u64) -> State {
        crate::stf::genesis_state(
            GENESIS_TIME,
            crate::test_fixtures::registry_of(num_validators),
        )
    }

    /// Two-phase build: produce a `SignedBlockWithAttestation` for `state`
    /// whose body carries `attestations` and whose `state_root` matches the
    /// post-state reached by applying the transition on a clone of `state`.
    ///
    /// This helper does NOT bound `attestations`. The `MAX_ATTESTATIONS` cap is
    /// enforced where a block arrives from the wire — `BlockBody`'s decode
    /// rejects an over-long list, and the import path checks the length again
    /// before applying anything — so a direct constructor like this one sits
    /// outside both. No caller here needs a bound; the largest passes four.
    ///
    /// It is not a pure constructor either: phase 1 below runs
    /// `process_block_header` and `process_attestations` on a probe and
    /// `unwrap`s both. So an over-cap vector does not yield a
    /// merely-undecodable block — it either panics in this helper or produces a
    /// `state_root` derived from an over-cap body. Either way the caller owns
    /// the consequence, and a large list costs test time rather than a
    /// rejection, because the tally is linear per vote.
    fn build_signed_block(
        state: &State,
        slot: Slot,
        attestations: Vec<Attestation>,
    ) -> SignedBlockWithAttestation {
        let proposer_index = ValidatorIndex::new(slot.get() % state.num_validators());

        // Phase 1: compute the post-state with `state_root = zero`.
        let mut probe = state.clone();
        probe.process_slots(slot).unwrap();
        let parent_root: Bytes32 = probe.latest_block_header.hash_tree_root().into();
        let mut block = Block {
            slot,
            proposer_index,
            parent_root,
            state_root: Bytes32::zero(),
            body: BlockBody { attestations },
        };
        probe.process_block_header(&block).unwrap();
        probe
            .process_attestations(&block.body.attestations)
            .unwrap();
        let state_root: Bytes32 = probe.hash_tree_root().into();

        // Phase 2: rewrite the block with the computed state_root.
        block.state_root = state_root;
        SignedBlockWithAttestation {
            message: BlockWithAttestation {
                block,
                proposer_attestation: Attestation::default(),
            },
            signature: BlockSignatures::default(),
        }
    }

    /// Empty-body chain of `n` consecutive valid signed blocks starting from
    /// `start`. Each block's `state_root` is the post-state root after
    /// applying the prior blocks.
    fn build_chain(start: &State, n: usize) -> Vec<SignedBlockWithAttestation> {
        let mut chain = Vec::with_capacity(n);
        let mut walker = start.clone();
        for i in 1..=n {
            let slot = Slot::new(i as u64);
            let sb = build_signed_block(&walker, slot, Vec::new());
            walker.state_transition(&sb, true).unwrap();
            chain.push(sb);
        }
        chain
    }

    /// Asserts that `post` — the state after applying a block that CARRIES one
    /// extra attestation — agrees with `baseline`, the state after applying the
    /// same block WITHOUT it, everywhere that attestation could have had an
    /// effect.
    ///
    /// Full `State` equality cannot hold: the two bodies have different
    /// hash-tree-roots, and `process_block_header` commits that root into
    /// `latest_block_header.body_root`. So pin that one field to the body that
    /// produced it, assert the two sides genuinely differ there, then compare
    /// the WHOLE state — which keeps the assertion honest as `State` grows
    /// fields, where a hand-enumerated field list would not.
    ///
    /// The claim is bounded, not absolute: this cannot mask a difference in any
    /// field other than the one it names — **as long as `State`'s `PartialEq`
    /// stays derived**. That is what makes overriding one leaf of one field
    /// safe: `State` derives `PartialEq` over all its fields, `BlockHeader`
    /// derives it over exactly five, and `Bitlist` derives it over
    /// `(bytes, length)` rather than bytes alone, so a length difference cannot
    /// hide behind equal bytes. A hand-written `PartialEq` that skipped a field
    /// would silently void this helper's guarantee.
    ///
    /// The named field's VALUE is asserted against `carrier`'s body rather than
    /// merely asserted to differ.
    /// Without that first assertion a `body_root` mis-committed by
    /// `process_block_header` would be consistently wrong on both sides, and
    /// the post-state-root check would not catch it either, because the
    /// two-phase builder derives `state_root` through the same code path.
    fn assert_post_states_agree_except_body_root(
        post: &State,
        carrier: &SignedBlockWithAttestation,
        baseline: &State,
    ) {
        let want_body_root: Bytes32 = carrier.message.block.body.hash_tree_root().into();
        assert_eq!(
            post.latest_block_header.body_root, want_body_root,
            "the committed body_root must be the root of the body that was applied",
        );
        assert_ne!(
            post.latest_block_header.body_root, baseline.latest_block_header.body_root,
            "fixture is wrong: the two blocks must carry different bodies",
        );
        let mut normalized = post.clone();
        normalized.latest_block_header.body_root = baseline.latest_block_header.body_root;
        assert_eq!(normalized, *baseline);
    }

    /// Eight empty-body blocks from a four-validator genesis, with the
    /// preconditions the stale-target tests depend on asserted rather than
    /// assumed. A fixture that skipped the vote for the wrong reason would make
    /// those tests pass while proving nothing.
    fn chain_of_eight() -> State {
        let mut state = genesis_state(4);
        for sb in build_chain(&state, 8) {
            state.state_transition(&sb, true).unwrap();
        }

        assert_eq!(state.slot, Slot::new(8));
        assert_eq!(state.latest_finalized.slot, Slot::ZERO);
        assert_eq!(state.historical_block_hashes.len(), 8);
        assert_eq!(state.justified_slots.get(0), Some(true));
        assert_eq!(state.justified_slots.get(6), Some(false));
        assert_eq!(state.justified_slots.get(7), Some(false));
        // The tally starts empty, so a later `justifications_roots` entry can
        // only have come from a vote the test supplied.
        assert!(state.justifications_roots.is_empty());
        // Every assertion here is on the slot-8 state, while the filters run one
        // `process_block_header` later: the carrying block at slot 9 pushes a
        // parent root and writes index 8. That write only ever EXTENDS, because
        // its index is `justified_slots.len()`, so indices 0, 6 and 7 survive it.
        //
        // The length assertion below does NOT falsify that — a header that
        // back-filled an existing index on the ninth block would leave the length
        // at 8 and pass. Its contribution is pinning the extension pattern: any
        // change that grows or shrinks the bitlist per block breaks it here,
        // where the cause is obvious, rather than downstream. Note it does not
        // bound the range check at the vote's own processing point, which sees
        // length 9 after the slot-9 header write and so admits a target at slot
        // 8. The back-fill cases are caught
        // downstream instead: a wrongly-set bit 7 fails the `get(7)` assertion in
        // the skip test, and a wrongly-set bit 6 or a cleared bit 0 fails the
        // positive control's `latest_justified` assertions, because the votes
        // would then die on filter 2 or filter 1. Both of those assertions
        // discriminate only because their votes carry supermajority strength.
        assert_eq!(state.justified_slots.len(), 8);
        state
    }

    // -- Composition --------------------------------------------------------

    #[test]
    fn composes_slots_block_attestations_in_order() {
        let mut driven = genesis_state(4);
        let mut hand = driven.clone();
        let sb = build_signed_block(&driven, Slot::new(1), Vec::new());
        let block = sb.message.block.clone();

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
        let mut sb = build_signed_block(&state, Slot::new(1), Vec::new());
        let want = sb.message.block.state_root;
        // Flip a byte in the declared post-state root.
        let mut tampered = want;
        tampered.0[0] ^= 0xff;
        sb.message.block.state_root = tampered;

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
        let mut sb = build_signed_block(&state, Slot::new(1), Vec::new());
        sb.message.block.state_root.0[0] ^= 0xff;
        // With validation off the tampered root is ignored.
        state.state_transition(&sb, false).unwrap();
        assert_eq!(state.slot, Slot::new(1));
    }

    // -- Error propagation --------------------------------------------------

    #[test]
    fn propagates_block_header_error() {
        let mut state = genesis_state(4);
        let mut sb = build_signed_block(&state, Slot::new(1), Vec::new());
        sb.message.block.parent_root = Bytes32::new([0xab; 32]);
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
        let sb0 = build_signed_block(&state, Slot::new(1), Vec::new());
        state.state_transition(&sb0, true).unwrap();
        let snapshot = state.clone();

        // Now attempt a block with a corrupted parent_root.
        let mut sb = build_signed_block(&state, Slot::new(2), Vec::new());
        sb.message.block.parent_root = Bytes32::new([0xab; 32]);
        let _ = state.state_transition(&sb, true).unwrap_err();
        assert_eq!(state, snapshot);
    }

    #[test]
    fn error_path_leaves_state_unchanged_on_state_root_mismatch() {
        // The most subtle path: process_attestations has already committed
        // its working copies before the post-state-root check fires.
        let mut state = genesis_state(4);
        let sb0 = build_signed_block(&state, Slot::new(1), Vec::new());
        state.state_transition(&sb0, true).unwrap();
        let snapshot = state.clone();

        let mut sb = build_signed_block(&state, Slot::new(2), Vec::new());
        sb.message.block.state_root.0[0] ^= 0xff;
        let err = state.state_transition(&sb, true).unwrap_err();
        assert!(matches!(
            err,
            StateTransitionError::StateRootMismatch { .. }
        ));
        assert_eq!(state, snapshot);
    }

    // -- Unusable attestations do not take the block down -------------------

    #[test]
    fn block_with_stale_attestation_target_is_accepted() {
        let state = chain_of_eight();
        // delta = 7 - 0 = 7: not within the small window, not a perfect square,
        // not pronic. The reference skips such a vote and still applies the
        // block; rejecting it would split this node off every conforming client
        // on one stale vote.
        //
        // `stale_attestation_target_does_not_move_justification` is the positive
        // control for this fixture: on the same eight-block chain it targets slot
        // 6, whose distance IS justifiable, and those votes are tallied. So the
        // vote below cannot be dying on an earlier filter — filters 1 to 5 see
        // equivalent input in both tests.
        // The body carries SUPERMAJORITY strength — 3 of 4 validators, since
        // 3*3 >= 2*4 — deliberately. With a single vote the justification
        // assertions below would hold whether or not the predicate existed,
        // because one vote never clears the threshold and the bit is therefore
        // never raised: they would read as guarantees while constraining
        // nothing. At three votes, deleting the predicate justifies slot 7 and
        // moves the justified checkpoint, so they discriminate.
        let unusable: Vec<Attestation> = (0..3)
            .map(|id| {
                super::attestation_tests::attestation(
                    id,
                    state.historical_block_hashes[0],
                    0,
                    state.historical_block_hashes[7],
                    7,
                )
            })
            .collect();

        let carrier = build_signed_block(&state, Slot::new(9), unusable);
        let mut post = state.clone();
        post.state_transition(&carrier, true).unwrap();

        let mut baseline = state.clone();
        baseline
            .state_transition(&build_signed_block(&state, Slot::new(9), Vec::new()), true)
            .unwrap();

        assert_eq!(
            post.justified_slots.get(7),
            Some(false),
            "the skipped votes must not have justified their target",
        );
        assert_eq!(
            post.latest_justified, state.latest_justified,
            "the skipped votes must not have moved the justified checkpoint",
        );
        assert_post_states_agree_except_body_root(&post, &carrier, &baseline);
    }

    #[test]
    fn stale_attestation_target_does_not_move_justification() {
        // The filters are per-vote: votes for a justifiable target are tallied
        // in the same body where votes for an unusable one are dropped.
        // delta = 6 is pronic (2*3) and justifiable; delta = 7 is not.
        //
        // BOTH groups carry supermajority strength — 3 of 4 validators, since
        // 3*3 >= 2*4 — so each group's assertion discriminates. A single
        // unusable vote could never clear the threshold, which would leave the
        // slot-7 assertion below holding with or without the predicate.
        // Validator ids overlap between the groups because the tally is keyed
        // per target root.
        let state = chain_of_eight();
        let source_root = state.historical_block_hashes[0];
        let pronic_target = state.historical_block_hashes[6];
        let unusable_target = state.historical_block_hashes[7];

        let tallied: Vec<Attestation> = (0..3)
            .map(|id| super::attestation_tests::attestation(id, source_root, 0, pronic_target, 6))
            .collect();
        let dropped: Vec<Attestation> = (1..4)
            .map(|id| super::attestation_tests::attestation(id, source_root, 0, unusable_target, 7))
            .collect();

        let mut body = tallied.clone();
        body.extend(dropped);
        let carrier = build_signed_block(&state, Slot::new(9), body);
        let mut post = state.clone();
        post.state_transition(&carrier, true).unwrap();

        let mut baseline = state.clone();
        baseline
            .state_transition(&build_signed_block(&state, Slot::new(9), tallied), true)
            .unwrap();

        // The valid votes did their work ...
        assert_eq!(post.justified_slots.get(6), Some(true));
        assert_eq!(post.latest_justified.root, pronic_target);
        assert_eq!(post.latest_justified.slot, Slot::new(6));
        // ... and the unusable ones left no trace anywhere.
        assert_eq!(post.justified_slots.get(7), Some(false));
        assert_post_states_agree_except_body_root(&post, &carrier, &baseline);
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
