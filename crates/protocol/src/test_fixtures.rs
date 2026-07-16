//! Shared sample-value helpers for `cfg(test)` use across the crate.
//!
//! Centralized so individual `mod tests` blocks no longer redefine the same
//! sample containers. Each helper returns a deterministic, populated value
//! suitable for round-trip and hash-tree-root assertions.

#![allow(dead_code)]

use types::{Bytes32, Signature};

use crate::block::{
    Block, BlockBody, BlockHeader, BlockSignatures, BlockWithAttestation,
    SignedBlockWithAttestation,
};
use crate::checkpoint::Checkpoint;
use crate::slot::Slot;
use crate::validator::ValidatorIndex;
use crate::vote::{Attestation, AttestationData, SignedAttestation};

/// SA1: deterministic [`Signature`] keyed off `seed`. Replaces per-test
/// `[seed; 3116]` open-coding.
pub(crate) fn sample_signature(seed: u8) -> Signature {
    Signature::new([seed; Signature::LEN])
}

/// Deterministic [`AttestationData`] keyed off `seed`.
pub(crate) fn sample_attestation_data(seed: u64) -> AttestationData {
    let byte = u8::try_from(seed & 0xff).unwrap_or(0);
    AttestationData {
        slot: Slot::new(seed),
        head: Checkpoint::new(Bytes32::new([byte; 32]), Slot::new(seed)),
        target: Checkpoint::default(),
        source: Checkpoint::default(),
    }
}

/// Deterministic [`Attestation`] keyed off `seed`.
pub(crate) fn sample_attestation(seed: u64) -> Attestation {
    Attestation {
        validator_id: ValidatorIndex::new(seed),
        data: sample_attestation_data(seed),
    }
}

/// Deterministic [`SignedAttestation`] keyed off `seed`.
pub(crate) fn sample_signed_attestation(seed: u64) -> SignedAttestation {
    let byte = u8::try_from(seed & 0xff).unwrap_or(0);
    SignedAttestation {
        message: sample_attestation(seed),
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
