//! Shared sample-value helpers for `cfg(test)` use across the crate.
//!
//! Centralized so individual `mod tests` blocks no longer redefine the same
//! sample containers. Each helper returns a deterministic, populated value
//! suitable for round-trip and hash-tree-root assertions.

// Retained construction sites for the deprecated `Bytes4000` placeholder.
// Scoped to this file so unrelated deprecations elsewhere in the crate are
// still surfaced. `expect` rather than `allow`: once this file's last site
// moves to `Signature`, the unfulfilled expectation fails the build instead of
// lingering as a stale allow.
#![expect(deprecated)]
#![allow(dead_code)]
// `assert_ssz_round_trip` reports a decode failure by panicking, which is the
// only way a test helper can fail. Narrower than the blanket allow a `mod tests`
// block takes: `unwrap_used` / `expect_used` stay denied here, so the
// `.unwrap_or(0)` style below remains enforced.
#![allow(clippy::panic)]

use ssz::{decode, encode, Decode, Encode};
use types::{Bytes32, Bytes4000, Signature};

use crate::block::{Block, BlockBody, BlockHeader, SignedBlock};
use crate::checkpoint::Checkpoint;
use crate::slot::Slot;
use crate::validator::ValidatorIndex;
use crate::vote::{Attestation, AttestationData, SignedAttestation};

/// Asserts that `value` survives an SSZ encode/decode round-trip unchanged, and
/// that its self-reported [`Encode::ssz_bytes_len`] agrees with the encoding.
///
/// The one round-trip assertion for this crate's wire containers — call it
/// instead of open-coding `decode(&encode(v))` in each test.
///
/// # Panics
/// Panics if `value` fails to decode, does not round-trip, or reports a length
/// that disagrees with its encoded form.
pub(crate) fn assert_ssz_round_trip<T>(value: &T)
where
    T: Encode + Decode + PartialEq + core::fmt::Debug,
{
    let bytes = encode(value);
    assert_eq!(
        bytes.len(),
        value.ssz_bytes_len(),
        "ssz_bytes_len disagrees with encoded length",
    );
    match decode::<T>(&bytes) {
        Ok(back) => assert_eq!(&back, value, "ssz round-trip mismatch"),
        Err(e) => panic!("ssz decode failed: {e:?}"),
    }
}

/// Deterministic [`Signature`] filled with `seed`.
///
/// The one construction site for signature test values — call this rather than
/// open-coding the byte array, so the container width lives in one place.
pub(crate) fn sample_signature(seed: u8) -> Signature {
    Signature::new([seed; Signature::LEN])
}

/// Deterministic [`SignedAttestation`] keyed off `seed`.
pub(crate) fn sample_signed_attestation(seed: u64) -> SignedAttestation {
    let byte = u8::try_from(seed & 0xff).unwrap_or(0);
    SignedAttestation {
        message: Attestation {
            validator_id: ValidatorIndex::new(seed),
            data: AttestationData {
                slot: Slot::new(seed),
                head: Checkpoint::new(Bytes32::new([byte; 32]), Slot::new(seed)),
                target: Checkpoint::default(),
                source: Checkpoint::default(),
            },
        },
        signature: sample_signature(byte),
    }
}

/// Canonical [`BlockHeader`] used by block / state tests.
pub(crate) fn sample_block_header() -> BlockHeader {
    BlockHeader {
        slot: Slot::new(7),
        proposer_index: ValidatorIndex::new(2),
        parent_root: Bytes32::new([0x11; 32]),
        state_root: Bytes32::new([0x22; 32]),
        body_root: Bytes32::new([0x33; 32]),
    }
}

/// Canonical [`Block`] with two attestations.
pub(crate) fn sample_block() -> Block {
    Block {
        slot: Slot::new(7),
        proposer_index: ValidatorIndex::new(2),
        parent_root: Bytes32::new([0x11; 32]),
        state_root: Bytes32::new([0x22; 32]),
        body: BlockBody {
            attestations: vec![sample_signed_attestation(1), sample_signed_attestation(2)],
        },
    }
}

/// Canonical [`SignedBlock`] wrapping [`sample_block`] with a 0xcd signature.
///
/// Still on the [`Bytes4000`] placeholder — the block envelope moves to
/// [`Signature`] with the block container refactor, not here.
pub(crate) fn sample_signed_block() -> SignedBlock {
    SignedBlock {
        message: sample_block(),
        signature: Bytes4000::new([0xcd; 4000]),
    }
}
