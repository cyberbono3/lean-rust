//! Deterministic sample values for the storage-contract suite.
//!
//! Each helper is keyed off a `seed: u8`; equal seeds produce equal values,
//! distinct seeds produce distinct values. Used by the concurrent smoke
//! test to give each thread its own root without collisions.

#![allow(
    dead_code,
    missing_docs,
    clippy::expect_used,
    clippy::missing_const_for_fn,
    clippy::must_use_candidate,
    clippy::unwrap_used
)]

use protocol::{
    Attestation, AttestationData, Block, BlockBody, BlockHeader, BlockWithAttestation, Checkpoint,
    ProtocolConfig, SignedBlockWithAttestation, Slot, State, ValidatorIndex,
};
use storage::HeadInfo;
use types::{Bytes32, Signature};

pub const fn sample_root(seed: u8) -> Bytes32 {
    Bytes32::new([seed; 32])
}

pub fn sample_signed_block(seed: u8) -> SignedBlockWithAttestation {
    let attestation = Attestation {
        validator_id: ValidatorIndex::new(u64::from(seed)),
        data: AttestationData {
            slot: Slot::new(u64::from(seed)),
            head: Checkpoint::new(sample_root(seed), Slot::new(u64::from(seed))),
            target: Checkpoint::default(),
            source: Checkpoint::default(),
        },
    };
    SignedBlockWithAttestation {
        message: BlockWithAttestation {
            block: Block {
                slot: Slot::new(u64::from(seed)),
                proposer_index: ValidatorIndex::new(u64::from(seed)),
                parent_root: sample_root(seed.wrapping_sub(1)),
                state_root: sample_root(seed.wrapping_add(1)),
                body: BlockBody {
                    attestations: vec![attestation],
                },
            },
            proposer_attestation: attestation,
        },
        signature: [Signature::new([seed; Signature::LEN])]
            .into_iter()
            .collect(),
    }
}

pub fn sample_state(seed: u8) -> State {
    State {
        config: ProtocolConfig {
            num_validators: u64::from(seed.max(1)),
            genesis_time: 1_700_000_000,
        },
        slot: Slot::new(u64::from(seed)),
        latest_block_header: BlockHeader {
            slot: Slot::new(u64::from(seed)),
            proposer_index: ValidatorIndex::new(u64::from(seed)),
            parent_root: sample_root(seed.wrapping_sub(1)),
            state_root: Bytes32::zero(),
            body_root: sample_root(seed.wrapping_add(2)),
        },
        ..State::default()
    }
}

pub fn sample_head(seed: u8) -> HeadInfo {
    HeadInfo::new(
        Checkpoint::new(sample_root(seed), Slot::new(u64::from(seed))),
        Checkpoint::new(sample_root(seed.wrapping_sub(1)), Slot::ZERO),
    )
}
