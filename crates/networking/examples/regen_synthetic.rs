//! Regenerates the synthetic devnet-1 wire vectors under `tests/data/synthetic/`.
//!
//! Not a test: `tests/parity.rs` pulls these files in with `include_bytes!`, so a
//! generator living there could never compile before the files it writes exist.
//! Run it by hand after a wire break, commit the bytes it emits, and copy the
//! printed roots into `tests/data/PROVENANCE.md`.
//!
//! Run: `cargo run -p lean-wire --example regen_synthetic`.

use std::error::Error;
use std::fs;
use std::path::Path;

use protocol::{
    stf::genesis_state_with_validators, Attestation, AttestationData, Block, BlockBody,
    BlockSignatures, BlockWithAttestation, Checkpoint, SignedAttestation,
    SignedBlockWithAttestation, Slot, Validator, ValidatorIndex,
};
use ssz::{encode, HashTreeRoot};
use types::{Bytes32, PublicKey, Signature};

fn main() -> Result<(), Box<dyn Error>> {
    let attestation = Attestation {
        validator_id: ValidatorIndex::new(3),
        data: AttestationData {
            slot: Slot::new(7),
            head: Checkpoint::new(Bytes32::new([0x11; 32]), Slot::new(7)),
            target: Checkpoint::new(Bytes32::new([0x22; 32]), Slot::new(4)),
            source: Checkpoint::new(Bytes32::new([0x33; 32]), Slot::new(0)),
        },
    };
    let signed = SignedAttestation {
        message: attestation,
        signature: Signature::new([0xab; Signature::LEN]),
    };
    // The block body carries PLAIN attestations; the per-vote signatures live in
    // the block-signature list on the signed envelope, not per body element.
    let body = BlockBody {
        attestations: vec![attestation, attestation],
    };

    // Empty-body signed block in the devnet-1 envelope. Its inner `Block` matches
    // `wire-parity/slot1-empty.block.ssz` (slot 1, proposer 1, parent 0x03..,
    // state 0x04..), wrapped with a default proposer attestation and an empty
    // signature list.
    let signed_block = SignedBlockWithAttestation {
        message: BlockWithAttestation {
            block: Block {
                slot: Slot::new(1),
                proposer_index: ValidatorIndex::new(1),
                parent_root: Bytes32::new([0x03; 32]),
                state_root: Bytes32::new([0x04; 32]),
                body: BlockBody::default(),
            },
            proposer_attestation: Attestation::default(),
        },
        signature: BlockSignatures::default(),
    };

    // Genesis 4-validator devnet-1 `State` with a populated registry. Demoted
    // here from `wire-parity/` when the validator-registry field changed the
    // native `State` SSZ format — there is no devnet-1 cross-client `State`
    // blob to copy yet, so this is a self-generated vector (see PROVENANCE.md).
    let validators: Vec<Validator> = (0..4_u8)
        .map(|i| Validator {
            pubkey: PublicKey::new([i; PublicKey::LEN]),
            index: ValidatorIndex::new(u64::from(i)),
        })
        .collect();
    let genesis_state = genesis_state_with_validators(4, 1_700_000_000, validators);

    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/synthetic");
    fs::create_dir_all(&dir)?;

    let signed_bytes = encode(&signed);
    let body_bytes = encode(&body);
    let signed_block_bytes = encode(&signed_block);
    let genesis_state_bytes = encode(&genesis_state);
    fs::write(dir.join("validator3.signedattestation.ssz"), &signed_bytes)?;
    fs::write(dir.join("two-attestations.blockbody.ssz"), &body_bytes)?;
    fs::write(dir.join("slot1-empty.signedblock.ssz"), &signed_block_bytes)?;
    fs::write(dir.join("genesis-4val.state.ssz"), &genesis_state_bytes)?;

    // `hex::encode` produces the same lower-hex the PROVENANCE table records; it
    // is already a dev-dependency and used by tests/parity.rs.
    println!(
        "validator3.signedattestation.ssz  {} bytes  root {}",
        signed_bytes.len(),
        hex::encode(signed.hash_tree_root()),
    );
    println!(
        "two-attestations.blockbody.ssz    {} bytes  root {}",
        body_bytes.len(),
        hex::encode(body.hash_tree_root()),
    );
    println!(
        "slot1-empty.signedblock.ssz       {} bytes  root {}",
        signed_block_bytes.len(),
        hex::encode(signed_block.hash_tree_root()),
    );
    println!(
        "genesis-4val.state.ssz            {} bytes  root {}",
        genesis_state_bytes.len(),
        hex::encode(genesis_state.hash_tree_root()),
    );
    Ok(())
}
