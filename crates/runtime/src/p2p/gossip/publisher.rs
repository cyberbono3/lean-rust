//! Publish path: typed [`Host`] methods that dispatch
//! [`HostCommand::Publish`] to the swarm-poll task and await the result.
//!
//! Each publish builds the wire payload via [`lean_wire::encode_gossip`]
//! (SSZ + Snappy block compression), hands it to the swarm task with the
//! topic + a oneshot reply channel, and surfaces the libp2p
//! [`gossipsub::PublishError`] as a typed [`PublishError`] for callers.

use libp2p::gossipsub;
use protocol::{SignedAttestation, SignedBlockWithAttestation};
use tokio::sync::oneshot;
use tracing::warn;

use crate::p2p::host::behaviour::{INTEROP_SAFE_PAYLOAD_BYTES, MAX_GOSSIP_PAYLOAD_BYTES};
use crate::p2p::host::{Host, HostCommand};

use super::Topic;

/// Re-export of the libp2p gossipsub message-id type so callers do not
/// need to depend on `libp2p` directly.
pub use libp2p::gossipsub::MessageId;

/// Failure surface for [`Host::publish_block`] and
/// [`Host::publish_vote`].
#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    /// The host command channel is closed — the swarm-poll task has
    /// already exited (typically `Service::stop` ran).
    #[error("host command channel closed")]
    ChannelClosed,
    /// libp2p gossipsub rejected the publish. The most common variant
    /// is [`gossipsub::PublishError::InsufficientPeers`] when no mesh
    /// peer is currently subscribed to the topic.
    #[error("gossipsub publish: {0}")]
    Gossipsub(#[from] gossipsub::PublishError),
    /// The encoded payload exceeds [`MAX_GOSSIP_PAYLOAD_BYTES`].
    ///
    /// Checked HERE rather than left to libp2p so the failure names the numbers.
    /// A block grows by one 3116-byte signature per body attestation, so the way
    /// this is reached is a validator count that outgrew the limit — and the
    /// caller has already persisted the block and moved its head onto it. An
    /// opaque `MessageTooLarge` after a round-trip through the swarm task would
    /// leave an operator with no way to see that.
    #[error("{topic} payload is {bytes} bytes, over the {limit}-byte gossip limit")]
    PayloadTooLarge {
        /// Topic the oversized payload was destined for.
        topic: &'static str,
        /// Encoded payload size.
        bytes: usize,
        /// [`MAX_GOSSIP_PAYLOAD_BYTES`].
        limit: usize,
    },
}

impl Host {
    /// SSZ-encodes + snappy-compresses `block`, then publishes it on
    /// the [`Topic::Block`] gossipsub topic.
    ///
    /// # Errors
    /// - [`PublishError::ChannelClosed`] if the swarm-poll task has
    ///   exited.
    /// - [`PublishError::Gossipsub`] for any libp2p-surfaced publish
    ///   failure (most often `InsufficientPeers` until the mesh forms).
    pub async fn publish_block(
        &self,
        block: &SignedBlockWithAttestation,
    ) -> Result<MessageId, PublishError> {
        self.publish_raw(Topic::Block, lean_wire::encode_gossip(block))
            .await
    }

    /// SSZ-encodes + snappy-compresses `vote`, then publishes it on the
    /// [`Topic::Vote`] gossipsub topic.
    ///
    /// # Errors
    /// Same shape as [`Self::publish_block`].
    pub async fn publish_vote(&self, vote: &SignedAttestation) -> Result<MessageId, PublishError> {
        self.publish_raw(Topic::Vote, lean_wire::encode_gossip(vote))
            .await
    }

    /// Sends a pre-encoded payload to the swarm task and awaits the
    /// gossipsub `publish` result via a oneshot reply channel.
    async fn publish_raw(&self, topic: Topic, payload: Vec<u8>) -> Result<MessageId, PublishError> {
        if payload.len() > MAX_GOSSIP_PAYLOAD_BYTES {
            return Err(PublishError::PayloadTooLarge {
                topic: topic.as_str(),
                bytes: payload.len(),
                limit: MAX_GOSSIP_PAYLOAD_BYTES,
            });
        }
        // A peer still on libp2p's 64 KiB default drops anything larger on
        // receipt, and nothing reports that back: the publish succeeds here, the
        // proposer logs success, and the block reaches nobody. Raising our own
        // limit turned that from a loud local error into a silent remote drop, so
        // say it locally at the threshold a default-configured peer still enforces.
        if payload.len() > INTEROP_SAFE_PAYLOAD_BYTES {
            warn!(
                topic = topic.as_str(),
                bytes = payload.len(),
                interop_safe = INTEROP_SAFE_PAYLOAD_BYTES,
                "gossip payload exceeds the default peer limit; peers that have not \
                 raised max_transmit_size will drop it without reporting",
            );
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        self.commands()
            .send(HostCommand::Publish {
                topic: topic.ident(),
                payload,
                reply: reply_tx,
            })
            .await
            .map_err(|_| PublishError::ChannelClosed)?;
        reply_rx
            .await
            .map_err(|_| PublishError::ChannelClosed)?
            .map_err(PublishError::from)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use libp2p::PeerId;
    use protocol::{
        Attestation, Block, BlockBody, BlockWithAttestation, SignedBlockWithAttestation,
    };
    use tokio::sync::mpsc;
    use types::Signature;

    use super::*;
    use crate::p2p::host::COMMAND_CHANNEL_CAPACITY;

    /// An INCOMPRESSIBLE signature.
    ///
    /// Load-bearing for these tests: the size gate runs on the wire payload, which
    /// `lean_wire::encode_gossip` snappy-compresses. A `Signature::zero()` fixture
    /// compresses by roughly three orders of magnitude, so a "huge" block built
    /// from zeros sails under the limit and the test would silently prove nothing.
    /// Real XMSS signatures are high-entropy and compress essentially not at all,
    /// so the fixture has to be too. xorshift keeps it deterministic without
    /// pulling in an rng.
    fn noise_signature(seed: u64) -> Signature {
        // Mix the seed before use: a bare `seed | 1` maps 0 and 1 (and 2 and 3, and
        // so on) onto the same state, which makes half the signatures byte-identical
        // and hands snappy exactly the redundancy this fixture must not have.
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut bytes = [0u8; Signature::LEN];
        for b in &mut bytes {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            // High byte: the low bits of an xorshift word carry visible structure.
            // `u64 >> 56` is always a byte. `unwrap_or(0)` would be worse than
            // useless here: a run of zeros is exactly the compressible pattern this
            // fixture must not contain, so a silent fallback would quietly restore
            // the vacuity the seed-mixing above exists to prevent.
            *b = u8::try_from(state >> 56).expect("u64 >> 56 is a byte");
        }
        Signature::new(bytes)
    }

    /// A block carrying `n` body attestations and the matching positional
    /// signature list (`n + 1` elements), which is what the producer emits.
    fn block_with_body(n: usize) -> SignedBlockWithAttestation {
        SignedBlockWithAttestation {
            message: BlockWithAttestation {
                block: Block {
                    body: BlockBody {
                        attestations: vec![Attestation::default(); n],
                    },
                    ..Block::default()
                },
                proposer_attestation: Attestation::default(),
            },
            signature: (0..=n as u64).map(noise_signature).collect(),
        }
    }

    /// A host whose command channel is CLOSED, so any dispatch fails instantly
    /// with `ChannelClosed`. That is what makes the two tests below able to prove
    /// ORDERING: a payload rejected for size returns `PayloadTooLarge`, while one
    /// that clears the size gate reaches the dispatch and returns `ChannelClosed`.
    fn detached_host() -> Host {
        let (tx, rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
        drop(rx);
        Host::new(PeerId::random(), tx)
    }

    #[tokio::test]
    async fn publish_rejects_a_block_over_the_gossip_limit() {
        // Each body attestation adds its own signature, so the block outgrows the
        // limit at a validator count a devnet could plausibly reach. Sized from the
        // limit itself rather than a magic number, so this keeps testing the
        // boundary if the limit moves.
        //
        // Counted in SIGNATURE bytes only. The gate runs on the compressed payload,
        // and the attestation half of each entry is fixture zeros that snappy erases
        // — so the uncompressed `3252 * N` figure overestimates what this fixture
        // puts on the wire, and sizing by it lands under the limit. Real traffic
        // carries real attestations; this is a property of the fixture, not of the
        // limit.
        let over = MAX_GOSSIP_PAYLOAD_BYTES / Signature::LEN + 4;
        let err = detached_host()
            .publish_block(&block_with_body(over))
            .await
            .expect_err("a block past the gossip limit must not be published");

        match err {
            PublishError::PayloadTooLarge {
                topic,
                bytes,
                limit,
            } => {
                assert_eq!(topic, lean_wire::BLOCK_TOPIC_V1);
                assert!(bytes > limit, "{bytes} should exceed {limit}");
                assert_eq!(limit, MAX_GOSSIP_PAYLOAD_BYTES);
            }
            other => panic!("expected PayloadTooLarge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn publish_admits_a_block_within_the_gossip_limit() {
        // The complement: without it, a limit of zero would satisfy the test above
        // while making every block unpublishable. Reaching ChannelClosed proves the
        // payload cleared the size gate and was dispatched.
        let err = detached_host()
            .publish_block(&block_with_body(8))
            .await
            .expect_err("the command channel is closed, so the dispatch fails");
        assert!(
            matches!(err, PublishError::ChannelClosed),
            "a small block must clear the size gate and reach dispatch, got {err:?}",
        );
    }
}
