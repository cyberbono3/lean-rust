//! Shared sample-value helpers for `cfg(test)` use across the crate.
//!
//! Centralized so individual `mod tests` blocks no longer redefine the same
//! sample containers. Each helper returns a deterministic, populated value
//! suitable for round-trip and hash-tree-root assertions.

#![allow(dead_code)]
// `assert_ssz_round_trip` reports a decode failure by panicking, which is the
// only way a test helper can fail. Narrower than the blanket allow a `mod tests`
// block takes: `unwrap_used` / `expect_used` stay denied here, so the
// `.unwrap_or(0)` style below remains enforced.
#![allow(clippy::panic)]

use ssz::{decode, encode, Decode, Encode, HashTreeRoot};
use types::{Bytes32, PublicKey, Signature};

use crate::block::{
    Block, BlockBody, BlockHeader, BlockSignatures, BlockWithAttestation,
    SignedBlockWithAttestation,
};
use crate::checkpoint::Checkpoint;
use crate::slot::Slot;
use crate::validator::{Validator, ValidatorIndex, Validators};
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

/// Asserts that `value.hash_tree_root()` equals `expected`.
///
/// The one frozen-root helper for this crate's wire containers — call it
/// instead of open-coding the `hash_tree_root` equality in each vector test.
///
/// # Panics
/// Panics if the root does not match `expected`.
pub(crate) fn assert_htr_eq<T: HashTreeRoot>(value: &T, expected: [u8; 32]) {
    assert_eq!(value.hash_tree_root(), expected, "hash-tree-root mismatch");
}

/// Emits the `(ssz_bytes, root)` pair used to freeze a wire vector.
///
/// Print the pair after a wire break and paste it into the frozen constant the
/// regression test asserts — never hand-derive a root.
pub(crate) fn regen_vector<T: Encode + HashTreeRoot>(value: &T) -> (Vec<u8>, [u8; 32]) {
    (encode(value), value.hash_tree_root())
}

/// Deterministic [`Signature`] filled with `seed`.
///
/// The one construction site for signature test values — call this rather than
/// open-coding the byte array, so the container width lives in one place.
pub(crate) fn sample_signature(seed: u8) -> Signature {
    Signature::new([seed; Signature::LEN])
}

/// Deterministic [`Validator`] keyed off `seed`.
pub(crate) fn sample_validator(seed: u8) -> Validator {
    Validator {
        pubkey: PublicKey::new([seed; PublicKey::LEN]),
        index: ValidatorIndex::new(u64::from(seed)),
    }
}

/// Deterministic [`Validators`] registry with `n` entries (seeds `0..n`).
pub(crate) fn sample_validators(n: u8) -> Validators {
    (0..n).map(sample_validator).collect()
}

/// Deterministic [`Attestation`] keyed off `seed`.
pub(crate) fn sample_attestation(seed: u64) -> Attestation {
    let byte = u8::try_from(seed & 0xff).unwrap_or(0);
    Attestation {
        validator_id: ValidatorIndex::new(seed),
        data: AttestationData {
            slot: Slot::new(seed),
            head: Checkpoint::new(Bytes32::new([byte; 32]), Slot::new(seed)),
            target: Checkpoint::default(),
            source: Checkpoint::default(),
        },
    }
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

/// Canonical [`Block`] with two plain attestations.
pub(crate) fn sample_block() -> Block {
    Block {
        slot: Slot::new(7),
        proposer_index: ValidatorIndex::new(2),
        parent_root: Bytes32::new([0x11; 32]),
        state_root: Bytes32::new([0x22; 32]),
        body: BlockBody {
            attestations: vec![sample_attestation(1), sample_attestation(2)],
        },
    }
}

/// Canonical [`BlockWithAttestation`] — [`sample_block`] plus a proposer
/// attestation sibling.
pub(crate) fn sample_block_with_attestation() -> BlockWithAttestation {
    BlockWithAttestation {
        block: sample_block(),
        proposer_attestation: sample_attestation(2),
    }
}

/// Deterministic [`BlockSignatures`] with `n` signatures (seeds `0..n`).
pub(crate) fn sample_block_signatures(n: u8) -> BlockSignatures {
    (0..n).map(sample_signature).collect()
}

/// Canonical [`SignedBlockWithAttestation`] wrapping
/// [`sample_block_with_attestation`] with three signatures.
pub(crate) fn sample_signed_block_with_attestation() -> SignedBlockWithAttestation {
    SignedBlockWithAttestation {
        message: sample_block_with_attestation(),
        signature: sample_block_signatures(3),
    }
}
