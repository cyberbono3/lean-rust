//! Deterministic 20-byte gossipsub message-id primitive.
//!
//! Implements the consensus gossipsub message-id spec. The id is the
//! first 20 bytes of SHA-256 over the byte layout:
//!
//! ```text
//! domain (4B) || topic_len (8B LE u64) || topic || payload
//! ```
//!
//! The domain prefix distinguishes valid-snappy decoding
//! ([`MESSAGE_DOMAIN_VALID_SNAPPY`]) from the invalid-snappy path
//! ([`MESSAGE_DOMAIN_INVALID_SNAPPY`]) used when no decompressor is
//! supplied or decompression fails.
//!
//! Snappy resolution itself lives in the libp2p adapter layer — this
//! module exposes the pure SHA-256-truncation primitive. Callers pass
//! the resolved domain alongside the topic and payload.

use sha2::{Digest, Sha256};

/// Length in bytes of a gossipsub domain prefix.
pub const DOMAIN_LEN: usize = 4;

/// Length in bytes of a gossipsub message-id.
pub const MESSAGE_ID_LEN: usize = 20;

/// Domain prefix selecting the invalid-snappy decoding path. Used when
/// no decompressor is supplied OR when decompression fails.
pub const MESSAGE_DOMAIN_INVALID_SNAPPY: [u8; DOMAIN_LEN] = [0, 0, 0, 0];

/// Domain prefix selecting the valid-snappy decoding path. Used when a
/// supplied decompressor returns `Ok(_)`.
pub const MESSAGE_DOMAIN_VALID_SNAPPY: [u8; DOMAIN_LEN] = [1, 0, 0, 0];

/// Computes the deterministic 20-byte gossipsub message-id for the given
/// `(domain, topic, payload)` triple.
///
/// The result is the first 20 bytes of SHA-256 over the layout
/// `domain (4B) || topic_len (8B LE u64) || topic || payload`. Input
/// segments stream into the hasher — no intermediate `Vec`.
#[must_use]
pub fn compute_gossipsub_message_id(
    domain: [u8; DOMAIN_LEN],
    topic: &[u8],
    payload: &[u8],
) -> [u8; MESSAGE_ID_LEN] {
    let digest = Sha256::new()
        .chain_update(domain)
        .chain_update(topic_len_le_bytes(topic))
        .chain_update(topic)
        .chain_update(payload)
        .finalize();
    let mut out = [0_u8; MESSAGE_ID_LEN];
    out.copy_from_slice(&digest[..MESSAGE_ID_LEN]);
    out
}

/// Encodes the topic length as the 8-byte little-endian uint64 required by
/// the message-id layout. Saturates at [`u64::MAX`] on theoretical platforms
/// where `usize` is wider than `u64` — never reached in practice (gossipsub
/// topics are short strings).
fn topic_len_le_bytes(topic: &[u8]) -> [u8; 8] {
    u64::try_from(topic.len()).unwrap_or(u64::MAX).to_le_bytes()
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    clippy::struct_field_names,
    clippy::unwrap_used
)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use static_assertions::const_assert_eq;

    /// Materializes the same layout `compute_gossipsub_message_id` streams
    /// into the hasher. Test-only — production callers never need the
    /// intermediate buffer.
    fn build_message_id_hash_input(
        domain: [u8; DOMAIN_LEN],
        topic: &[u8],
        payload: &[u8],
    ) -> Vec<u8> {
        [
            domain.as_slice(),
            &topic_len_le_bytes(topic),
            topic,
            payload,
        ]
        .concat()
    }

    // -- compile-time witnesses --------------------------------------------

    const_assert_eq!(DOMAIN_LEN, 4);
    const_assert_eq!(MESSAGE_ID_LEN, 20);

    // -- unit tests --------------------------------------------------------

    #[test]
    fn domain_constants_match_canonical_bytes() {
        assert_eq!(MESSAGE_DOMAIN_INVALID_SNAPPY, [0, 0, 0, 0]);
        assert_eq!(MESSAGE_DOMAIN_VALID_SNAPPY, [1, 0, 0, 0]);
    }

    #[test]
    fn empty_topic_and_payload_produce_twenty_byte_id() {
        let id = compute_gossipsub_message_id(MESSAGE_DOMAIN_INVALID_SNAPPY, &[], &[]);
        assert_eq!(id.len(), MESSAGE_ID_LEN);
    }

    #[test]
    fn topic_length_serializes_little_endian_u64() {
        let topic = b"abcde"; // 5 bytes
        let built = build_message_id_hash_input(MESSAGE_DOMAIN_INVALID_SNAPPY, topic, &[]);
        // domain(4) + topic_len(8) + topic(5) = 17 bytes.
        assert_eq!(built.len(), 17);
        assert_eq!(&built[4..12], &[5, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn streaming_matches_vec_path() {
        // `compute(...) == sha256(build(...))[..20]` must hold across a
        // representative spread of inputs. Pins the invariant that the
        // streaming and Vec-materialized paths produce the same bytes.
        let cases: &[(&[u8; 4], &[u8], &[u8])] = &[
            (&MESSAGE_DOMAIN_INVALID_SNAPPY, b"", b""),
            (&MESSAGE_DOMAIN_VALID_SNAPPY, b"topic", b"data"),
            (&MESSAGE_DOMAIN_INVALID_SNAPPY, &[0; 100], &[0xff; 256]),
            (
                &MESSAGE_DOMAIN_VALID_SNAPPY,
                b"longer topic name",
                b"payload-bytes-here",
            ),
        ];
        for (domain, topic, payload) in cases {
            let built = build_message_id_hash_input(**domain, topic, payload);
            let expected = Sha256::digest(&built);
            let got = compute_gossipsub_message_id(**domain, topic, payload);
            assert_eq!(&got[..], &expected[..MESSAGE_ID_LEN]);
        }
    }

    // -- JSON parity replay ------------------------------------------------

    const FIXTURE: &str = include_str!("../tests/data/gossipsub.json");

    #[derive(Deserialize)]
    struct Fixture {
        cases: Vec<Case>,
    }

    #[derive(Deserialize)]
    struct Case {
        id: String,
        input: CaseInput,
        output: CaseOutput,
    }

    #[derive(Deserialize)]
    struct CaseInput {
        topic_hex: String,
        raw_data_hex: String,
        snappy_mode: String,
        #[serde(default)]
        decompressed_hex: Option<String>,
    }

    #[derive(Deserialize)]
    struct CaseOutput {
        domain_hex: String,
        hash_input_hex: String,
        message_id_hex: String,
    }

    impl Case {
        /// Resolves the payload by matching the snappy decompression
        /// outcome — picks `decompressed_hex` when snappy succeeded,
        /// else `raw_data_hex`.
        fn resolve_payload(&self) -> Vec<u8> {
            let hex_payload = if self.input.snappy_mode == "success" {
                self.input
                    .decompressed_hex
                    .as_ref()
                    .expect("success case has decompressed_hex")
            } else {
                &self.input.raw_data_hex
            };
            hex::decode(hex_payload).expect("valid payload hex")
        }

        fn topic(&self) -> Vec<u8> {
            hex::decode(&self.input.topic_hex).expect("valid topic hex")
        }

        fn domain(&self) -> [u8; DOMAIN_LEN] {
            let bytes = hex::decode(&self.output.domain_hex).expect("valid domain hex");
            <[u8; DOMAIN_LEN]>::try_from(bytes.as_slice()).expect("4-byte domain")
        }
    }

    #[test]
    fn replay_gossipsub_fixture() {
        let fixture: Fixture = serde_json::from_str(FIXTURE).expect("parse fixture");
        assert!(!fixture.cases.is_empty());

        for case in &fixture.cases {
            let domain = case.domain();
            let topic = case.topic();
            let payload = case.resolve_payload();

            // 1. Hash-input layout byte-parity.
            let built = build_message_id_hash_input(domain, &topic, &payload);
            assert_eq!(
                hex::encode(&built),
                case.output.hash_input_hex,
                "hash_input mismatch in case {}",
                case.id,
            );

            // 2. Message-id byte-parity.
            let id = compute_gossipsub_message_id(domain, &topic, &payload);
            assert_eq!(
                hex::encode(id),
                case.output.message_id_hex,
                "message_id mismatch in case {}",
                case.id,
            );
        }
    }
}
