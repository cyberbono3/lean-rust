//! Inbound gossipsub routing.
//!
//! [`route_gossipsub_message`] is invoked by the swarm-poll task in
//! [`crate::p2p::service`] for every `gossipsub::Event::Message`. The message
//! topic is matched against the [`networking`] topic constants, the
//! payload is SSZ + Snappy decoded, and the typed value is forwarded
//! over the matching per-topic `mpsc::Sender`.
//!
//! ## Decompression cache
//!
//! libp2p invokes the configured `message_id_fn` (our
//! [`crate::p2p::host::behaviour::gossipsub_message_id`]) on every inbound
//! message before yielding the matching `Event::Message`. That call
//! already snappy-decompresses the payload to choose the
//! valid/invalid-snappy domain byte. The decompressed bytes are
//! snapshotted into a thread-local cache keyed by the freshly-computed
//! [`gossipsub::MessageId`]; this routing layer drains the cache via
//! [`crate::p2p::host::behaviour::take_decompressed_for`] so the SSZ decode
//! path can skip the second `decode_gossip` snappy round-trip. On a
//! cache miss (rare in steady state) we fall back to
//! [`lean_wire::decode_gossip`].
//!
//! Decode failures and full receivers are logged at `warn` and dropped
//! — gossipsub mesh replay covers transient loss, and the decode error
//! path is non-fatal (peers may publish junk).

use std::sync::Arc;

use lean_wire::NetworkingError;
use libp2p::gossipsub;
use protocol::{SignedAttestation, SignedBlockWithAttestation};
use ssz::Decode;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::p2p::admission::{AdmitGuard, PeerAdmission};
use crate::p2p::host::behaviour::take_decompressed_for;
use crate::sync::PeerId;

/// Inbound channel for decoded gossipsub payloads of a single type.
///
/// Created in [`crate::p2p::service::P2pService`] at start; taken out once
/// via [`crate::p2p::P2pService::take_block_receiver`] /
/// [`crate::p2p::P2pService::take_vote_receiver`] by the consumer (typically
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

    /// Non-blocking poll for a decoded payload. Used by the consensus
    /// loop's per-tick gossip drain, which must never block the interval
    /// ticker on `recv().await`.
    ///
    /// # Errors
    /// - [`TryRecvError::Empty`] when no payload is currently queued.
    /// - [`TryRecvError::Disconnected`] when the swarm-poll task has exited.
    ///
    /// [`TryRecvError::Empty`]: tokio::sync::mpsc::error::TryRecvError::Empty
    /// [`TryRecvError::Disconnected`]: tokio::sync::mpsc::error::TryRecvError::Disconnected
    pub fn try_recv(&mut self) -> Result<T, tokio::sync::mpsc::error::TryRecvError> {
        self.0.try_recv()
    }
}

/// Inbound channel for [`SignedBlockWithAttestation`] payloads received on
/// [`lean_wire::BLOCK_TOPIC_V1`]. Each payload carries the [`AdmitGuard`] that
/// admitted it, released when the consumer drops the tuple after import.
pub type BlockReceiver = GossipReceiver<(AdmitGuard, SignedBlockWithAttestation)>;

/// Inbound channel for [`SignedAttestation`] payloads received on
/// [`lean_wire::VOTE_TOPIC_V1`]. Carries its [`AdmitGuard`] alongside the payload.
pub type VoteReceiver = GossipReceiver<(AdmitGuard, SignedAttestation)>;

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
    propagation_source: &libp2p::PeerId,
    admission: &Arc<PeerAdmission>,
    message_id: &gossipsub::MessageId,
    msg: &gossipsub::Message,
    block_tx: &mpsc::Sender<(AdmitGuard, SignedBlockWithAttestation)>,
    vote_tx: &mpsc::Sender<(AdmitGuard, SignedAttestation)>,
) {
    // Convert the libp2p peer id to the runtime's `libp2p`-free `sync::PeerId` once at
    // the boundary. Base-58 is non-empty, but drop (not panic) on the defensive empty
    // case — mirrors `sync::PeerId::new` usage in the sync loop.
    let Ok(peer) = PeerId::new(propagation_source.to_base58()) else {
        warn!("empty gossip source peer id; dropping message");
        return;
    };
    // Drain the cache exactly once per message regardless of topic —
    // keeps the populate-by-`gossipsub_message_id` ↔ consume-here
    // lifetime symmetric and prevents an unknown-topic entry from
    // lingering in the slot until the next valid-snappy populate.
    let cached = take_decompressed_for(message_id);
    let topic_str = msg.topic.as_str();
    match topic_str {
        lean_wire::BLOCK_TOPIC_V1 => {
            forward::<SignedBlockWithAttestation>(
                &peer,
                admission,
                &msg.data,
                cached.as_deref(),
                block_tx,
                "block",
            );
        }
        lean_wire::VOTE_TOPIC_V1 => {
            forward::<SignedAttestation>(
                &peer,
                admission,
                &msg.data,
                cached.as_deref(),
                vote_tx,
                "vote",
            );
        }
        _ => debug!(topic = %topic_str, "unknown gossip topic"),
    }
}

/// Admits, then decodes, then forwards one gossip payload to `tx`.
///
/// Per-peer admission runs BEFORE the decode: the decode work (SSZ, plus snappy
/// on the cache-miss fallback) is the very cost the bound exists to protect, so
/// a peer at its in-flight cap has its excess dropped before any of it is spent
/// (mesh replay covers legitimate loss). The [`AdmitGuard`] is RAII: it releases
/// the peer's slot automatically on every early return — decode failure, full
/// channel — and otherwise rides the channel with the payload until the consumer
/// drops it.
///
/// Bound scope, honestly: the snappy decompression performed inside
/// `gossipsub_message_id` (which populates `cached_decompressed`) happens
/// upstream in gossipsub before this router runs and cannot be gated here; the
/// admission bound covers the SSZ decode and the full snappy+SSZ fallback.
fn forward<T>(
    peer: &PeerId,
    admission: &Arc<PeerAdmission>,
    data: &[u8],
    cached_decompressed: Option<&[u8]>,
    tx: &mpsc::Sender<(AdmitGuard, T)>,
    kind: &'static str,
) where
    T: Decode,
{
    // Admission FIRST — before any decode work is spent on this peer's payload.
    let Some(guard) = admission.try_admit(peer) else {
        warn!(%peer, kind, "peer inbound cap reached; dropping message");
        return;
    };
    let decoded: Result<T, NetworkingError> = match cached_decompressed {
        // Cache hit: snappy already done by `gossipsub_message_id`.
        Some(bytes) => ssz::decode(bytes).map_err(Into::into),
        // Cache miss: run the full snappy + SSZ decode.
        None => lean_wire::decode_gossip::<T>(data),
    };
    let value = match decoded {
        Ok(value) => value,
        Err(err) => {
            // `guard` drops here → the slot is released immediately.
            warn!(%err, kind, "gossip decode failed");
            return;
        }
    };
    // On a full channel the guard drops here → the slot is released; the message never
    // entered the queue.
    if tx.try_send((guard, value)).is_err() {
        warn!(kind, "gossip receiver lagging; dropping message");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::p2p::admission::AdmissionConfig;
    use libp2p::gossipsub::{IdentTopic, Message};

    type BlockChan = (
        mpsc::Sender<(AdmitGuard, SignedBlockWithAttestation)>,
        mpsc::Receiver<(AdmitGuard, SignedBlockWithAttestation)>,
    );
    type VoteChan = (
        mpsc::Sender<(AdmitGuard, SignedAttestation)>,
        mpsc::Receiver<(AdmitGuard, SignedAttestation)>,
    );

    fn block_chan(cap: usize) -> BlockChan {
        mpsc::channel(cap)
    }
    fn vote_chan(cap: usize) -> VoteChan {
        mpsc::channel(cap)
    }

    fn source_peer() -> libp2p::PeerId {
        libp2p::PeerId::random()
    }

    fn admission() -> Arc<PeerAdmission> {
        PeerAdmission::new(AdmissionConfig::default())
    }

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
        let (block_tx, mut block_rx) = block_chan(8);
        let (vote_tx, mut vote_rx) = vote_chan(8);

        let block = SignedBlockWithAttestation::default();
        let payload = lean_wire::encode_gossip(&block);
        route_gossipsub_message(
            &source_peer(),
            &admission(),
            &dummy_id(),
            &synth_message(lean_wire::BLOCK_TOPIC_V1, payload),
            &block_tx,
            &vote_tx,
        );

        let (_admit, got) = block_rx.recv().await.expect("block must be forwarded");
        assert_eq!(got, block);
        assert!(vote_rx.try_recv().is_err(), "vote channel must stay empty");
    }

    #[tokio::test]
    async fn routes_carry_admit_guard() {
        let (block_tx, mut block_rx) = block_chan(8);
        let (vote_tx, _vote_rx) = vote_chan(8);
        let peer = source_peer();
        let adm = admission();

        let payload = lean_wire::encode_gossip(&SignedBlockWithAttestation::default());
        route_gossipsub_message(
            &peer,
            &adm,
            &dummy_id(),
            &synth_message(lean_wire::BLOCK_TOPIC_V1, payload),
            &block_tx,
            &vote_tx,
        );

        // The forwarded tuple carries the guard for the source peer; the slot is held
        // until the consumer drops it.
        let (admit, _block) = block_rx.recv().await.expect("block forwarded");
        let expected =
            crate::sync::PeerId::new(peer.to_base58()).expect("non-empty base58 peer id");
        assert_eq!(admit.peer(), &expected);
        assert_eq!(adm.tracked_peer_count(), 1, "slot held while guard is live");
        drop(admit);
        assert_eq!(adm.tracked_peer_count(), 0, "slot released on drop");
    }

    #[tokio::test]
    async fn over_cap_peer_message_dropped() {
        // Cap of 1: the first message is admitted, the second (same peer) is dropped.
        let adm = PeerAdmission::new(AdmissionConfig::new(
            core::num::NonZeroUsize::new(1).unwrap(),
        ));
        let (block_tx, mut block_rx) = block_chan(8);
        let (vote_tx, _vote_rx) = vote_chan(8);
        let peer = source_peer();

        let payload = || lean_wire::encode_gossip(&SignedBlockWithAttestation::default());
        route_gossipsub_message(
            &peer,
            &adm,
            &dummy_id(),
            &synth_message(lean_wire::BLOCK_TOPIC_V1, payload()),
            &block_tx,
            &vote_tx,
        );
        // First message admitted and queued (its guard still held → slot occupied).
        let (_first_admit, _first) = block_rx.recv().await.expect("first admitted");

        // Second message from the same peer while the first slot is still held → dropped.
        route_gossipsub_message(
            &peer,
            &adm,
            &dummy_id(),
            &synth_message(lean_wire::BLOCK_TOPIC_V1, payload()),
            &block_tx,
            &vote_tx,
        );
        assert!(
            block_rx.try_recv().is_err(),
            "over-cap message must be dropped, not queued"
        );
    }

    #[tokio::test]
    async fn slot_released_on_channel_full() {
        // Channel capacity 1, already full → try_send fails → the guard drops and the
        // peer's slot is released (no leak). PR-202.
        let adm = admission();
        let (block_tx, mut block_rx) = block_chan(1);
        let (vote_tx, _vote_rx) = vote_chan(8);
        let peer = source_peer();
        let sync_peer = crate::sync::PeerId::new(peer.to_base58()).unwrap();

        // Fill the channel with a directly-sent tuple (holding one of this peer's slots)
        // so `forward`'s `try_send` fails.
        let filler = adm.try_admit(&sync_peer).expect("admit filler");
        block_tx
            .try_send((filler, SignedBlockWithAttestation::default()))
            .expect("channel accepts first");
        // Exactly one slot held for this peer (the filler) before the failed forward.
        assert_eq!(adm.in_flight_for(&sync_peer), 1);

        let payload = lean_wire::encode_gossip(&SignedBlockWithAttestation::default());
        route_gossipsub_message(
            &peer,
            &adm,
            &dummy_id(),
            &synth_message(lean_wire::BLOCK_TOPIC_V1, payload),
            &block_tx,
            &vote_tx,
        );

        // The forward admitted a second slot then dropped it when `try_send` failed:
        // the PER-PEER slot count is back to exactly the filler's one (a leak would
        // leave it at 2 — the peer-entry count stays 1 either way, so it is asserted on
        // the slot count, not the entry count).
        assert_eq!(
            adm.in_flight_for(&sync_peer),
            1,
            "failed send must release the admitted slot (no per-peer leak)"
        );

        // Draining the filler and dropping its guard frees the peer entirely.
        let (filler_guard, _) = block_rx.recv().await.expect("filler delivered");
        drop(filler_guard);
        assert_eq!(adm.in_flight_for(&sync_peer), 0);
        assert_eq!(
            adm.tracked_peer_count(),
            0,
            "entry pruned once no slots remain"
        );
    }

    #[tokio::test]
    async fn routes_valid_vote_to_vote_receiver() {
        let (block_tx, mut block_rx) = block_chan(8);
        let (vote_tx, mut vote_rx) = vote_chan(8);

        let vote = SignedAttestation::default();
        let payload = lean_wire::encode_gossip(&vote);
        route_gossipsub_message(
            &source_peer(),
            &admission(),
            &dummy_id(),
            &synth_message(lean_wire::VOTE_TOPIC_V1, payload),
            &block_tx,
            &vote_tx,
        );

        let (_admit, got) = vote_rx.recv().await.expect("vote must be forwarded");
        assert_eq!(got, vote);
        assert!(
            block_rx.try_recv().is_err(),
            "block channel must stay empty"
        );
    }

    #[tokio::test]
    async fn unknown_topic_is_ignored() {
        let (block_tx, mut block_rx) = block_chan(8);
        let (vote_tx, mut vote_rx) = vote_chan(8);

        route_gossipsub_message(
            &source_peer(),
            &admission(),
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
        let (block_tx, mut block_rx) = block_chan(8);
        let (vote_tx, _vote_rx) = vote_chan(8);

        // Valid snappy frame, but the decompressed bytes are too short
        // to be a SignedBlockWithAttestation — decode_gossip returns NetworkingError::Ssz.
        let payload = lean_wire::encode_gossip_data(&[0_u8; 4]);
        route_gossipsub_message(
            &source_peer(),
            &admission(),
            &dummy_id(),
            &synth_message(lean_wire::BLOCK_TOPIC_V1, payload),
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
        use crate::p2p::host::behaviour::gossipsub_message_id;

        let (block_tx, mut block_rx) = block_chan(8);
        let (vote_tx, mut vote_rx) = vote_chan(8);

        let block = SignedBlockWithAttestation::default();
        let payload = lean_wire::encode_gossip(&block);
        let mut msg = synth_message(lean_wire::BLOCK_TOPIC_V1, payload);
        let id = gossipsub_message_id(&msg);

        // Corrupt the raw snappy bytes; a fallback would now fail.
        msg.data.fill(0xFF);

        route_gossipsub_message(&source_peer(), &admission(), &id, &msg, &block_tx, &vote_tx);

        let (_admit, got) = block_rx
            .recv()
            .await
            .expect("cache must deliver the block despite the corrupted raw payload");
        assert_eq!(got, block);
        assert!(vote_rx.try_recv().is_err());
    }
}
