//! Consensus [`State`] container plus its inner [`ProtocolConfig`].
//!
//! The state declares 9 SSZ fields in order:
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

use ssz::merkleize::merkleize;
use ssz::{Decode, DecodeError, Encode, HashTreeRoot};
use types::{Bitlist, Bytes32};

use crate::block::{BlockHeader, BLOCK_HEADER_SSZ_LEN};
use crate::checkpoint::Checkpoint;
use crate::internal::{
    bitlist_hash_tree_root, bytes32_list_hash_tree_root, decode_bytes32_list, encode_bytes32_list,
    ensure_len, read_fixed, read_offset, u64_chunk, write_offset, BYTES32_LEN,
    BYTES_PER_LENGTH_OFFSET,
};
use crate::slot::Slot;

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

const PROTOCOL_CONFIG_SSZ_LEN: usize = 16;
const SLOT_SSZ_LEN: usize = 8;
const CHECKPOINT_SSZ_LEN: usize = 40;
const STATE_VARIABLE_FIELD_COUNT: usize = 4;

/// Length of the fixed portion of a [`State`] (5 fixed fields plus 4 offsets
/// for the variable-length tails).
pub const STATE_FIXED_PART_LEN: usize = PROTOCOL_CONFIG_SSZ_LEN
    + SLOT_SSZ_LEN
    + BLOCK_HEADER_SSZ_LEN
    + CHECKPOINT_SSZ_LEN
    + CHECKPOINT_SSZ_LEN
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
        let latest_justified = Checkpoint::from_ssz_bytes(&bytes[c..c + CHECKPOINT_SSZ_LEN])?;
        c += CHECKPOINT_SSZ_LEN;
        let latest_finalized = Checkpoint::from_ssz_bytes(&bytes[c..c + CHECKPOINT_SSZ_LEN])?;
        c += CHECKPOINT_SSZ_LEN;

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
        // 9 fields → merkleize zero-pads to width 16.
        merkleize(&[
            self.config.hash_tree_root(),
            self.slot.hash_tree_root(),
            self.latest_block_header.hash_tree_root(),
            self.latest_justified.hash_tree_root(),
            self.latest_finalized.hash_tree_root(),
            bytes32_list_hash_tree_root(&self.historical_block_hashes, HISTORICAL_ROOTS_LIMIT),
            bitlist_hash_tree_root(&self.justified_slots),
            bytes32_list_hash_tree_root(&self.justifications_roots, HISTORICAL_ROOTS_LIMIT),
            bitlist_hash_tree_root(&self.justifications_validators),
        ])
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use ssz::{decode, encode, SszError};
    use types::Bytes32;

    use crate::validator::ValidatorIndex;

    fn sample_block_header() -> BlockHeader {
        BlockHeader {
            slot: Slot::new(7),
            proposer_index: ValidatorIndex::new(2),
            parent_root: Bytes32::new([0x11; 32]),
            state_root: Bytes32::new([0x22; 32]),
            body_root: Bytes32::new([0x33; 32]),
        }
    }

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

    #[test]
    fn state_hash_tree_root_responds_to_each_field() {
        let baseline = sample_state().hash_tree_root();

        let mut s = sample_state();
        s.config.num_validators = 5;
        assert_ne!(s.hash_tree_root(), baseline);

        let mut s = sample_state();
        s.config.genesis_time = 1_800_000_000;
        assert_ne!(s.hash_tree_root(), baseline);

        let mut s = sample_state();
        s.slot = Slot::new(10);
        assert_ne!(s.hash_tree_root(), baseline);

        let mut s = sample_state();
        s.latest_block_header.body_root = Bytes32::new([0x99; 32]);
        assert_ne!(s.hash_tree_root(), baseline);

        let mut s = sample_state();
        s.latest_justified = Checkpoint::default();
        assert_ne!(s.hash_tree_root(), baseline);

        let mut s = sample_state();
        s.latest_finalized = Checkpoint::default();
        assert_ne!(s.hash_tree_root(), baseline);

        let mut s = sample_state();
        s.historical_block_hashes.push(Bytes32::zero());
        assert_ne!(s.hash_tree_root(), baseline);

        let mut s = sample_state();
        s.justified_slots.set(7, true).unwrap();
        assert_ne!(s.hash_tree_root(), baseline);

        let mut s = sample_state();
        s.justifications_roots.push(Bytes32::zero());
        assert_ne!(s.hash_tree_root(), baseline);

        let mut s = sample_state();
        s.justifications_validators.set(11, true).unwrap();
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
