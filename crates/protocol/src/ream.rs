//! Cross-client genesis interop: decoding the compact "leanchain" state
//! SSZ emitted by the shared local-pq genesis generator.
//!
//! pq-devnet-0 anchors lean-rust and the ream client (`master-0bceaee`)
//! from the *same* `genesis.ssz`, produced by the
//! `eth-beacon-genesis:pk910-leanchain` generator. That file uses a
//! **compact** state layout — `config`, the two checkpoints, and four
//! variable-field offsets — rather than the native 9-field [`State`] SSZ
//! container defined in [`crate::state`]. This module isolates the
//! decoder for that interop shape so the canonical container code does
//! not carry it.
//!
//! The decoder is **validate-then-discard**: it parses and shape-checks
//! the historical / justification tails (surfacing malformed input as
//! typed [`DecodeError`]s) but synthesizes the canonical slot-0 anchor
//! rather than reconstructing the full history — matching how the rest
//! of the codebase treats imported anchor states.
//!
//! Wired through `lean-cli`'s genesis loader; see that crate's
//! `loads_ream_legacy_local_pq_state_from_ssz` test for the contract.

use ssz::{Decode, DecodeError, HashTreeRoot};
use types::Bitlist;

use crate::block::{BlockBody, BlockHeader};
use crate::checkpoint::Checkpoint;
use crate::internal::{
    decode_bytes32_list, read_fixed, read_offset, BYTES_PER_LENGTH_OFFSET, CHECKPOINT_LEN,
};
use crate::state::{
    ProtocolConfig, State, HISTORICAL_ROOTS_LIMIT, JUSTIFICATIONS_VALIDATORS_LIMIT,
    PROTOCOL_CONFIG_SSZ_LEN, STATE_VARIABLE_FIELD_COUNT,
};

/// Byte length of the fixed portion of the compact ream "leanchain"
/// state: the `config` container, both checkpoints, and the four
/// variable-field offsets. `112` bytes on devnet0.
const REAM_LEAN_STATE_FIXED_PART_LEN: usize = PROTOCOL_CONFIG_SSZ_LEN
    + CHECKPOINT_LEN
    + CHECKPOINT_LEN
    + STATE_VARIABLE_FIELD_COUNT * BYTES_PER_LENGTH_OFFSET; // 112

impl State {
    /// Decodes the compact ream "leanchain" genesis state emitted by the
    /// `eth-beacon-genesis:pk910-leanchain` local-pq generator into a
    /// canonical slot-0 anchor [`State`].
    ///
    /// The on-disk shape is the compact interop layout (not the native
    /// 9-field [`State`] SSZ): a [`ProtocolConfig`] container, the
    /// `latest_justified` / `latest_finalized` checkpoints, and four
    /// SSZ offsets pointing at the `historical_block_hashes`,
    /// `justified_slots`, `justifications_roots`, and
    /// `justifications_validators` tails. ream `master-0bceaee` reads the
    /// generated `num_validators` / `genesis_time` from this and builds
    /// its own anchor; lean-rust does the same so both clients share a
    /// genesis (and therefore agree on head roots).
    ///
    /// This is a **validate-then-discard** producer: every variable tail
    /// is parsed and bounds-checked (so malformed generator output
    /// surfaces as a typed error), but the tails are *not* copied into
    /// the returned state. The result is the canonical slot-0 anchor —
    /// the configured `config` and checkpoints, an empty history, and
    /// default bitlists — consistent with how the rest of the codebase
    /// treats imported anchor states.
    ///
    /// # Errors
    ///
    /// Returns an SSZ [`DecodeError`] when `bytes` is shorter than the
    /// fixed part, the offsets are malformed (into the fixed portion,
    /// decreasing, or out of bounds), or a variable tail violates the
    /// devnet0 bounds (over its limit, wrong raw-bitlist length, or
    /// non-zero trailing bits).
    pub fn from_ream_legacy_ssz_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() < REAM_LEAN_STATE_FIXED_PART_LEN {
            return Err(DecodeError::InvalidByteLength {
                len: bytes.len(),
                expected: REAM_LEAN_STATE_FIXED_PART_LEN,
            });
        }

        let mut c = 0;
        let config = read_fixed::<ProtocolConfig>(bytes, &mut c)?;
        let latest_justified = Checkpoint::from_ssz_bytes(&bytes[c..c + CHECKPOINT_LEN])?;
        c += CHECKPOINT_LEN;
        let latest_finalized = Checkpoint::from_ssz_bytes(&bytes[c..c + CHECKPOINT_LEN])?;
        c += CHECKPOINT_LEN;

        let mut offsets = [0_usize; STATE_VARIABLE_FIELD_COUNT];
        for offset in &mut offsets {
            *offset = read_offset(bytes, &mut c)?;
        }

        if offsets[0] != REAM_LEAN_STATE_FIXED_PART_LEN {
            return Err(DecodeError::OffsetIntoFixedPortion(offsets[0]));
        }
        for pair in offsets.windows(2) {
            if pair[1] < pair[0] {
                return Err(DecodeError::OffsetsAreDecreasing(pair[1]));
            }
        }
        let last_offset = offsets[STATE_VARIABLE_FIELD_COUNT - 1];
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
        let _justified_slots = decode_ream_raw_bitlist::<HISTORICAL_ROOTS_LIMIT>(
            tail_slice(1),
            historical_block_hashes.len(),
            "justified_slots",
        )?;
        let justifications_roots = decode_bytes32_list(tail_slice(2), HISTORICAL_ROOTS_LIMIT)?;
        let validator_count = usize::try_from(config.num_validators).map_err(|_| {
            DecodeError::BytesInvalid("num_validators does not fit usize".to_owned())
        })?;
        let justifications_validator_bits = justifications_roots
            .len()
            .checked_mul(validator_count)
            .ok_or_else(|| {
                DecodeError::BytesInvalid("justifications_validators length overflow".to_owned())
            })?;
        let _justifications_validators = decode_ream_raw_bitlist::<JUSTIFICATIONS_VALIDATORS_LIMIT>(
            tail_slice(3),
            justifications_validator_bits,
            "justifications_validators",
        )?;

        // The decoded variable-length fields are intentionally NOT
        // copied into the returned State: this function is a
        // validate-then-discard anchor-state producer, not a full
        // legacy-shape reconstruction. It runs the interop-format SSZ
        // through the field-level parsers (so we surface malformed input
        // as typed errors) and then returns the canonical slot-0 anchor
        // shape (empty history, default bitlists) consistent with how
        // the rest of the codebase treats imported anchor states. See
        // lean-cli's loads_ream_legacy_local_pq_state_from_ssz test
        // for the contract.
        Ok(Self {
            config,
            latest_block_header: BlockHeader {
                body_root: BlockBody::default().hash_tree_root().into(),
                ..BlockHeader::default()
            },
            latest_justified,
            latest_finalized,
            ..Self::default()
        })
    }
}

/// Decodes a raw (un-SSZ-framed) bitlist tail of `bit_len` bits from the
/// compact ream state into a bounded [`Bitlist`].
///
/// Unlike SSZ's length-delimited bitlist encoding, the generator writes
/// the bits packed little-endian with no sentinel, so the bit length is
/// supplied by the caller (derived from the companion list lengths). The
/// payload is validated against `LIMIT`, the expected byte count
/// (`ceil(bit_len / 8)`), and zero trailing bits; `context` names the
/// field in any error.
///
/// # Errors
///
/// [`DecodeError::BytesInvalid`] when `bit_len` exceeds `LIMIT`, the byte
/// length does not match `ceil(bit_len / 8)`, the bitlist constructor
/// rejects the length, or the final byte carries non-zero trailing bits.
fn decode_ream_raw_bitlist<const LIMIT: usize>(
    bytes: &[u8],
    bit_len: usize,
    context: &'static str,
) -> Result<Bitlist<LIMIT>, DecodeError> {
    if bit_len > LIMIT {
        return Err(DecodeError::BytesInvalid(format!(
            "{context}: bit length {bit_len} exceeds limit {LIMIT}"
        )));
    }
    let expected_bytes = bit_len.div_ceil(8);
    if bytes.len() != expected_bytes {
        return Err(DecodeError::BytesInvalid(format!(
            "{context}: raw bit payload has {} bytes, expected {expected_bytes}",
            bytes.len()
        )));
    }

    let mut bitlist = Bitlist::<LIMIT>::with_length(bit_len)
        .map_err(|err| DecodeError::BytesInvalid(format!("{context}: {err}")))?;
    for bit_index in 0..bit_len {
        let bit_set = bytes[bit_index / 8] & (1_u8 << (bit_index % 8)) != 0;
        if bit_set {
            bitlist
                .set(bit_index, true)
                .map_err(|err| DecodeError::BytesInvalid(format!("{context}: {err}")))?;
        }
    }

    if bit_len % 8 != 0 {
        let trailing_mask = !((1_u8 << (bit_len % 8)) - 1);
        if bytes.last().is_some_and(|last| last & trailing_mask != 0) {
            return Err(DecodeError::BytesInvalid(format!(
                "{context}: non-zero trailing bits"
            )));
        }
    }

    Ok(bitlist)
}
