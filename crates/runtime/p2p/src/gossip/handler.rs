//! Inbound gossipsub routing.
//!
//! [`route_gossipsub_message`] is invoked by the swarm-poll task in
//! [`crate::service`] for every `gossipsub::Event::Message`. The message
//! topic is matched against the [`networking`] topic constants, the
//! payload is SSZ + Snappy decoded via [`networking::decode_gossip`],
//! and the typed value is forwarded over the matching per-topic
//! `mpsc::Sender`.
//!
//! Decode failures and full receivers are logged at `warn` and dropped
//! — gossipsub mesh replay covers transient loss, and the decode error
//! path is non-fatal (peers may publish junk).

use libp2p::gossipsub;
use protocol::{SignedBlock, SignedVote};
use ssz::Decode;
use tokio::sync::mpsc;
use tracing::{debug, warn};

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
/// Topic match is by canonical string (the [`networking`] constants).
/// Unknown topics are logged at `debug` and ignored. Decode failures
/// are logged at `warn` and dropped. Full receivers log at `warn` and
/// drop the message — gossipsub mesh replay covers transient loss.
pub(crate) fn route_gossipsub_message(
    msg: &gossipsub::Message,
    block_tx: &mpsc::Sender<SignedBlock>,
    vote_tx: &mpsc::Sender<SignedVote>,
) {
    let topic_str = msg.topic.as_str();
    if topic_str == networking::BLOCK_TOPIC_V1 {
        forward::<SignedBlock>(&msg.data, block_tx, "block");
    } else if topic_str == networking::VOTE_TOPIC_V1 {
        forward::<SignedVote>(&msg.data, vote_tx, "vote");
    } else {
        debug!(topic = %topic_str, "unknown gossip topic");
    }
}

fn forward<T>(data: &[u8], tx: &mpsc::Sender<T>, kind: &'static str)
where
    T: Decode,
{
    match networking::decode_gossip::<T>(data) {
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

    #[tokio::test]
    async fn routes_valid_block_to_block_receiver() {
        let (block_tx, mut block_rx) = mpsc::channel::<SignedBlock>(8);
        let (vote_tx, mut vote_rx) = mpsc::channel::<SignedVote>(8);

        let block = SignedBlock::default();
        let payload = networking::encode_gossip(&block);
        route_gossipsub_message(
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
            &synth_message(networking::BLOCK_TOPIC_V1, payload),
            &block_tx,
            &vote_tx,
        );

        assert!(block_rx.try_recv().is_err());
    }
}
