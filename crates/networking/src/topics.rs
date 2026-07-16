//! Gossipsub topic identifiers for the consensus networking layer.
//!
//! Topic strings are the canonical identifiers libp2p hashes into a
//! `TopicHash` and that the deterministic message-id function
//! ([`crate::compute_gossipsub_message_id`]) folds into the SHA-256 input.
//! Centralising them here keeps `lean-p2p-host` free of protocol-level
//! constants — `lean-p2p-host::gossip::Topic` is a typed wrapper that
//! delegates to these values.

/// Gossipsub topic carrying [`protocol::SignedBlock`] payloads
/// (SSZ + Snappy block compression — see [`crate::encode_gossip`]).
pub const BLOCK_TOPIC_V1: &str = "/leanconsensus/devnet0/block/ssz_snappy";

/// Gossipsub topic carrying [`protocol::SignedAttestation`] payloads
/// (SSZ + Snappy block compression — see [`crate::encode_gossip`]).
pub const VOTE_TOPIC_V1: &str = "/leanconsensus/devnet0/vote/ssz_snappy";

// Compile-time enforcement of the libp2p `StreamProtocol` / `IdentTopic`
// invariant: topic strings must start with `/`. Violations fail the
// build, not just the test suite.
const _: () = {
    assert!(BLOCK_TOPIC_V1.as_bytes()[0] == b'/');
    assert!(VOTE_TOPIC_V1.as_bytes()[0] == b'/');
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_constants_match_spec() {
        assert_eq!(BLOCK_TOPIC_V1, "/leanconsensus/devnet0/block/ssz_snappy");
        assert_eq!(VOTE_TOPIC_V1, "/leanconsensus/devnet0/vote/ssz_snappy");
    }
}
