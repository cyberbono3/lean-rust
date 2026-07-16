//! Regenerates the synthetic devnet-1 wire vectors under `tests/data/synthetic/`.
//!
//! Not a test: `tests/parity.rs` pulls these files in with `include_bytes!`, so a
//! generator living there could never compile before the files it writes exist.
//! Run it by hand after a wire break, commit the bytes it emits, and copy the
//! printed roots into `tests/data/PROVENANCE.md`.
//!
//! Run: `cargo run -p lean-wire --example regen_synthetic`.

use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use protocol::{
    Attestation, AttestationData, BlockBody, Checkpoint, SignedAttestation, Slot, ValidatorIndex,
};
use ssz::{encode, HashTreeRoot};
use types::{Bytes32, Signature};

/// Lower-hex encoding for the PROVENANCE root column — paste-ready and
/// diffable across regenerations.
fn hex_lower(bytes: &[u8; 32]) -> String {
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        // Writing to a `String` is infallible; the `Result` is discarded rather
        // than unwrapped to keep the crate's no-unwrap policy intact.
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    let signed = SignedAttestation {
        message: Attestation {
            validator_id: ValidatorIndex::new(3),
            data: AttestationData {
                slot: Slot::new(7),
                head: Checkpoint::new(Bytes32::new([0x11; 32]), Slot::new(7)),
                target: Checkpoint::new(Bytes32::new([0x22; 32]), Slot::new(4)),
                source: Checkpoint::new(Bytes32::new([0x33; 32]), Slot::new(0)),
            },
        },
        signature: Signature::new([0xab; Signature::LEN]),
    };
    let body = BlockBody {
        attestations: vec![signed.clone(), signed.clone()],
    };

    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/synthetic");
    fs::create_dir_all(&dir)?;

    let signed_bytes = encode(&signed);
    let body_bytes = encode(&body);
    fs::write(dir.join("validator3.signedattestation.ssz"), &signed_bytes)?;
    fs::write(dir.join("two-attestations.blockbody.ssz"), &body_bytes)?;

    println!(
        "validator3.signedattestation.ssz  {} bytes  root {}",
        signed_bytes.len(),
        hex_lower(&signed.hash_tree_root()),
    );
    println!(
        "two-attestations.blockbody.ssz    {} bytes  root {}",
        body_bytes.len(),
        hex_lower(&body.hash_tree_root()),
    );
    Ok(())
}
