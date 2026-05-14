//! Inbound gossipsub routing.
//!
//! [`route_gossipsub_message`] is invoked by the swarm-poll task in
//! [`crate::service`] for every `gossipsub::Event::Message`. The message
//! topic is matched against the [`networking`] topic constants, the
//! payload is SSZ + Snappy decoded, and the typed value is forwarded
//! over the matching per-topic `mpsc::Sender`.
//!
//! ## Decompression cache
//!
//! libp2p invokes the configured `message_id_fn` (our
//! [`crate::host::behaviour::gossipsub_message_id`]) on every inbound
//! message before yielding the matching `Event::Message`. That call
//! already snappy-decompresses the payload to choose the
//! valid/invalid-snappy domain byte. The decompressed bytes are
//! snapshotted into a thread-local cache keyed by the freshly-computed
//! [`gossipsub::MessageId`]; this routing layer drains the cache via
//! [`crate::host::behaviour::take_decompressed_for`] so the SSZ decode
//! path can skip the second `decode_gossip` snappy round-trip. On a
//! cache miss (rare in steady state) we fall back to
//! [`networking::decode_gossip`].
//!
//! Decode failures and full receivers are logged at `warn` and dropped
//! — gossipsub mesh replay covers transient loss, and the decode error
//! path is non-fatal (peers may publish junk).

use libp2p::gossipsub;
use networking::NetworkingError;
use protocol::{SignedBlock, SignedVote};
use ssz::Decode;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::host::behaviour::take_decompressed_for;

/// Inbound channel for decoded gossipsub payloads of a single type.
///
/// Created in [`crate::service::P2pService`] at start; taken out once
/// via [`crate::P2pService::take_block_receiver`] /
/// [`crate::P2pService::take_vote_receiver`] by the consumer (typically
/// the chain-validation / vote-aggregation tasks in the composing
/// binary).
#[derive(Debug)]
pub struct GossipReceiver<T>(mpsc::Receiver<T>);

impl<T> GossipReceiver<T> {
    pub(crate) fn new(rx: mpsc::Receiver<T>) -> Self {
        Self(rx)
    }

    /// Awaits the next decoded payload. Returns `None` when the
    /// sending half has been dropped (typically because the swarm-poll
    /// task has exited).
    pub async fn recv(&mut self) -> Option<T> {
        self.0.recv().await
    }
}

/// Inbound channel for [`SignedBlock`] payloads received on
/// [`networking::BLOCK_TOPIC_V1`].
pub type BlockReceiver = GossipReceiver<SignedBlock>;

/// Inbound channel for [`SignedVote`] payloads received on
/// [`networking::VOTE_TOPIC_V1`].
pub type VoteReceiver = GossipReceiver<SignedVote>;

/// Routes an inbound `gossipsub::Message` to the matching per-topic
/// sender after SSZ + Snappy decode.
///
/// `message_id` is the id libp2p just emitted alongside `msg`; it is
/// used to look up the decompressed payload cached by the
/// `message_id_fn` so we can skip a second snappy decompression. On
/// cache miss (e.g. invalid-snappy payload, evicted entry) the full
/// `decode_gossip` path runs as a fallback.
///
/// Topic match is by canonical string (the [`networking`] constants).
/// Unknown topics are logged at `debug` and ignored. Decode failures
/// are logged at `warn` and dropped. Full receivers log at `warn` and
/// drop the message — gossipsub mesh replay covers transient loss.
pub(crate) fn route_gossipsub_message(
    message_id: &gossipsub::MessageId,
    msg: &gossipsub::Message,
    block_tx: &mpsc::Sender<SignedBlock>,
    vote_tx: &mpsc::Sender<SignedVote>,
) {
    let topic_str = msg.topic.as_str();
    if topic_str != networking::BLOCK_TOPIC_V1 && topic_str != networking::VOTE_TOPIC_V1 {
        debug!(topic = %topic_str, "unknown gossip topic");
        return;
    }
    // Drain the cache exactly once per message regardless of topic — a
    // stale entry left in the slot would otherwise collide with the
    // next inbound message's id.
    let cached = take_decompressed_for(message_id);
    if topic_str == networking::BLOCK_TOPIC_V1 {
        forward::<SignedBlock>(&msg.data, cached.as_deref(), block_tx, "block");
    } else {
        forward::<SignedVote>(&msg.data, cached.as_deref(), vote_tx, "vote");
    }
}

fn forward<T>(
    data: &[u8],
    cached_decompressed: Option<&[u8]>,
    tx: &mpsc::Sender<T>,
    kind: &'static str,
) where
    T: Decode,
{
    let decoded: Result<T, NetworkingError> = match cached_decompressed {
        // Cache hit: snappy already done by `gossipsub_message_id`.
        Some(bytes) => ssz::decode(bytes).map_err(Into::into),
        // Cache miss: run the full snappy + SSZ decode.
        None => networking::decode_gossip::<T>(data),
    };
    match decoded {
        Ok(value) => {
            if tx.try_send(value).is_err() {
                warn!(kind, "gossip receiver lagging; dropping message");
            }
        }
        Err(err) => warn!(%err, kind, "gossip decode failed"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use libp2p::gossipsub::{IdentTopic, Message};

    fn synth_message(topic: &str, data: Vec<u8>) -> Message {
        Message {
            source: None,
            data,
            sequence_number: None,
            topic: IdentTopic::new(topic).hash(),
        }
    }

    /// A throwaway id used in tests that exercise the cache-miss
    /// fallback. Real callers source the id from libp2p's
    /// `Event::Message`; tests that don't first prime the cache via
    /// `gossipsub_message_id` should pass this so the take returns
    /// `None` and the full `decode_gossip` path runs.
    fn dummy_id() -> gossipsub::MessageId {
        gossipsub::MessageId::from(vec![0u8; 20])
    }

    #[tokio::test]
    async fn routes_valid_block_to_block_receiver() {
        let (block_tx, mut block_rx) = mpsc::channel::<SignedBlock>(8);
        let (vote_tx, mut vote_rx) = mpsc::channel::<SignedVote>(8);

        let block = SignedBlock::default();
        let payload = networking::encode_gossip(&block);
        route_gossipsub_message(
            &dummy_id(),
            &synth_message(networking::BLOCK_TOPIC_V1, payload),
            &block_tx,
            &vote_tx,
        );

        let got = block_rx.recv().await.expect("block must be forwarded");
        assert_eq!(got, block);
        assert!(vote_rx.try_recv().is_err(), "vote channel must stay empty");
    }

    #[tokio::test]
    async fn routes_valid_vote_to_vote_receiver() {
        let (block_tx, mut block_rx) = mpsc::channel::<SignedBlock>(8);
        let (vote_tx, mut vote_rx) = mpsc::channel::<SignedVote>(8);

        let vote = SignedVote::default();
        let payload = networking::encode_gossip(&vote);
        route_gossipsub_message(
            &dummy_id(),
            &synth_message(networking::VOTE_TOPIC_V1, payload),
            &block_tx,
            &vote_tx,
        );

        let got = vote_rx.recv().await.expect("vote must be forwarded");
        assert_eq!(got, vote);
        assert!(
            block_rx.try_recv().is_err(),
            "block channel must stay empty"
        );
    }

    #[tokio::test]
    async fn unknown_topic_is_ignored() {
        let (block_tx, mut block_rx) = mpsc::channel::<SignedBlock>(8);
        let (vote_tx, mut vote_rx) = mpsc::channel::<SignedVote>(8);

        route_gossipsub_message(
            &dummy_id(),
            &synth_message("/lean/unknown", vec![0xDE, 0xAD]),
            &block_tx,
            &vote_tx,
        );

        assert!(block_rx.try_recv().is_err());
        assert!(vote_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn malformed_block_payload_drops_silently() {
        let (block_tx, mut block_rx) = mpsc::channel::<SignedBlock>(8);
        let (vote_tx, _vote_rx) = mpsc::channel::<SignedVote>(8);

        // Valid snappy frame, but the decompressed bytes are too short
        // to be a SignedBlock — decode_gossip returns NetworkingError::Ssz.
        let payload = networking::encode_gossip_data(&[0_u8; 4]);
        route_gossipsub_message(
            &dummy_id(),
            &synth_message(networking::BLOCK_TOPIC_V1, payload),
            &block_tx,
            &vote_tx,
        );

        assert!(block_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn cache_hit_skips_snappy_decode() {
        // Prove the cache path is the one being exercised by:
        // 1. Building a valid snappy gossip payload from a real block.
        // 2. Priming the cache via `gossipsub_message_id`, which copies
        //    the SSZ bytes into the thread-local slot keyed by the id.
        // 3. Mutating `msg.data` (the raw snappy bytes) to garbage so
        //    the fallback `decode_gossip` would fail.
        // 4. Routing and asserting the block IS delivered — only
        //    possible because the cache supplied the SSZ bytes.
        use crate::host::behaviour::gossipsub_message_id;

        let (block_tx, mut block_rx) = mpsc::channel::<SignedBlock>(8);
        let (vote_tx, mut vote_rx) = mpsc::channel::<SignedVote>(8);

        let block = SignedBlock::default();
        let payload = networking::encode_gossip(&block);
        let mut msg = synth_message(networking::BLOCK_TOPIC_V1, payload);
        let id = gossipsub_message_id(&msg);

        // Corrupt the raw snappy bytes; a fallback would now fail.
        msg.data.fill(0xFF);

        route_gossipsub_message(&id, &msg, &block_tx, &vote_tx);

        let got = block_rx
            .recv()
            .await
            .expect("cache must deliver the block despite the corrupted raw payload");
        assert_eq!(got, block);
        assert!(vote_rx.try_recv().is_err());
    }
}
