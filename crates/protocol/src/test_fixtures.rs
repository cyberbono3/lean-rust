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

use types::{Bytes32, Bytes4000};

use crate::block::{Block, BlockBody, BlockHeader, SignedBlock};
use crate::checkpoint::Checkpoint;
use crate::slot::Slot;
use crate::validator::ValidatorIndex;
use crate::vote::{SignedVote, Vote};

/// Deterministic [`SignedVote`] keyed off `seed`.
pub(crate) fn sample_signed_vote(seed: u64) -> SignedVote {
    let byte = u8::try_from(seed & 0xff).unwrap_or(0);
    SignedVote {
        validator_id: ValidatorIndex::new(seed),
        message: Vote {
            slot: Slot::new(seed),
            head: Checkpoint::new(Bytes32::new([byte; 32]), Slot::new(seed)),
            target: Checkpoint::default(),
            source: Checkpoint::default(),
        },
        signature: Bytes4000::new([byte; 4000]),
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
            attestations: vec![sample_signed_vote(1), sample_signed_vote(2)],
        },
    }
}

/// Canonical [`SignedBlock`] wrapping [`sample_block`] with a 0xcd signature.
pub(crate) fn sample_signed_block() -> SignedBlock {
    SignedBlock {
        message: sample_block(),
        signature: Bytes4000::new([0xcd; 4000]),
    }
}
