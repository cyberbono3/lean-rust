//! Composite [`libp2p::swarm::NetworkBehaviour`] for the devnet0 host.
//!
//! Combines gossipsub, two `request_response` behaviours (one each for
//! [`STATUS_PROTOCOL_V1`] and [`BLOCKS_BY_ROOT_PROTOCOL_V1`]), identify,
//! and ping into a single typed behaviour. The gossipsub message-id
//! function is wired to the deterministic 20-byte primitive in
//! [`networking::compute_gossipsub_message_id`].
//!
//! Request/response handler logic lives outside this crate; the codec
//! is implemented in [`codec::SszSnappyCodec`] and dispatched per
//! protocol in [`crate::service`].

use std::{cell::RefCell, time::Duration};

use libp2p::{
    gossipsub, identify,
    identity::Keypair,
    ping,
    request_response::{self, ProtocolSupport},
    swarm::NetworkBehaviour,
    StreamProtocol,
};
use networking::{
    compute_gossipsub_message_id, ProtocolId, BLOCKS_BY_ROOT_PROTOCOL_V1,
    MESSAGE_DOMAIN_INVALID_SNAPPY, MESSAGE_DOMAIN_VALID_SNAPPY, STATUS_PROTOCOL_V1,
};

use crate::error::{HostError, HostResult};
use crate::options::AgentVersion;

pub(crate) mod codec;
pub(crate) use codec::{RpcRequest, RpcResponse, SszSnappyCodec};

/// Application-specific identify protocol-version string advertised at
/// the libp2p identify handshake.
const IDENTIFY_PROTOCOL_VERSION: &str = "lean/0.1.0";

/// Gossipsub heartbeat interval. Drives mesh maintenance cadence; lower
/// values shorten mesh-formation latency at the cost of more bookkeeping
/// traffic. One second matches the devnet0 reference profile.
const GOSSIPSUB_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);

/// Composite behaviour driven by the host swarm task.
///
/// Field names double as gossipsub / `request_response` / identify / ping
/// dispatch tags via the `NetworkBehaviour` derive macro — renaming a
/// field renames the generated `DevnetBehaviourEvent` variant.
///
/// `Status` and `BlocksByRoot` are split across two
/// [`request_response::Behaviour`] instances rather than colocated on
/// one. Multistream-select picks a single protocol per outbound
/// substream from the first matching entry in the proposing peer's
/// protocol list; with one combined behaviour, a `BlocksByRoot` request
/// would always negotiate onto the leading protocol (e.g. Status) and
/// the codec would reject the variant. Separating them gives each
/// request kind its own behaviour and therefore its own protocol
/// negotiation path.
#[derive(NetworkBehaviour)]
pub(crate) struct DevnetBehaviour {
    pub(crate) gossipsub: gossipsub::Behaviour,
    pub(crate) status_rr: request_response::Behaviour<SszSnappyCodec>,
    pub(crate) blocks_rr: request_response::Behaviour<SszSnappyCodec>,
    pub(crate) identify: identify::Behaviour,
    pub(crate) ping: ping::Behaviour,
}

impl DevnetBehaviour {
    /// Builds the composite behaviour from the host keypair + options.
    ///
    /// # Errors
    /// - [`HostError::GossipsubInit`] when the gossipsub builder
    ///   rejects the internal config (programming error — config is
    ///   wholly internal).
    pub(crate) fn build(keypair: &Keypair, agent_version: &AgentVersion) -> HostResult<Self> {
        Ok(Self {
            gossipsub: Self::build_gossipsub()?,
            status_rr: Self::build_request_response(STATUS_PROTOCOL_V1),
            blocks_rr: Self::build_request_response(BLOCKS_BY_ROOT_PROTOCOL_V1),
            identify: Self::build_identify(keypair, agent_version),
            ping: ping::Behaviour::new(ping::Config::new()),
        })
    }

    fn build_gossipsub() -> HostResult<gossipsub::Behaviour> {
        // Devnet0 publishes unsigned messages; `Anonymous` authenticity
        // pairs with the message-id function below to give
        // deterministic 20-byte IDs derived purely from `(topic, payload)`.
        let config = gossipsub::ConfigBuilder::default()
            .validation_mode(gossipsub::ValidationMode::Anonymous)
            .heartbeat_interval(GOSSIPSUB_HEARTBEAT_INTERVAL)
            .message_id_fn(gossipsub_message_id)
            .build()
            .map_err(Self::gossipsub_init_err)?;
        gossipsub::Behaviour::new(gossipsub::MessageAuthenticity::Anonymous, config)
            .map_err(Self::gossipsub_init_err)
    }

    fn build_identify(keypair: &Keypair, agent_version: &AgentVersion) -> identify::Behaviour {
        identify::Behaviour::new(
            identify::Config::new(IDENTIFY_PROTOCOL_VERSION.to_owned(), keypair.public())
                .with_agent_version(agent_version.to_string()),
        )
    }

    fn build_request_response(protocol: ProtocolId) -> request_response::Behaviour<SszSnappyCodec> {
        let protocols = [(
            StreamProtocol::new(protocol.as_str()),
            ProtocolSupport::Full,
        )];
        request_response::Behaviour::new(protocols, request_response::Config::default())
    }

    fn gossipsub_init_err<E: std::fmt::Display>(err: E) -> HostError {
        HostError::GossipsubInit(err.to_string())
    }
}

/// Defensive upper bound on a single decompressed gossipsub payload. A
/// peer claiming a larger frame is rejected as invalid without
/// allocating the buffer.
const MAX_SNAPPY_DECOMPRESSED: usize = 16 * 1024 * 1024;

thread_local! {
    /// Scratch buffer reused across [`is_valid_snappy`] calls. The
    /// snappy `Decoder` itself is a zero-sized type (`snap::raw::Decoder`
    /// holds no state), so only the output buffer is worth keeping
    /// thread-local.
    static SNAPPY_SCRATCH: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(4096));

    /// Single-slot inbound-routing cache. Populated by
    /// [`gossipsub_message_id`] whenever it observes a valid snappy
    /// payload, then consumed by
    /// [`crate::gossip::handler::route_gossipsub_message`] via
    /// [`take_decompressed_for`] to skip a second decompression on the
    /// SSZ-decode path. Same-poll write/read on the swarm-poll task
    /// guarantees hit for inbound messages; outbound publishes write
    /// entries that get evicted on the next inbound. See module-level
    /// note on libp2p call ordering.
    static DECOMPRESSED_CACHE: RefCell<Option<(gossipsub::MessageId, Vec<u8>)>> =
        const { RefCell::new(None) };
}

/// Reports whether `data` is a fully-decodable snappy frame, returning
/// the decompressed length on success so the caller can read the bytes
/// from the [`SNAPPY_SCRATCH`] thread-local. Reuses the scratch buffer
/// to keep this call allocation-free on the gossipsub hot path (once
/// the buffer has grown enough for the largest payload seen on this
/// thread).
fn is_valid_snappy(data: &[u8]) -> Option<usize> {
    let decompressed_len = snap::raw::decompress_len(data).ok()?;
    if decompressed_len > MAX_SNAPPY_DECOMPRESSED {
        return None;
    }
    SNAPPY_SCRATCH.with_borrow_mut(|buf| {
        if buf.len() < decompressed_len {
            buf.resize(decompressed_len, 0);
        }
        snap::raw::Decoder::new()
            .decompress(data, &mut buf[..decompressed_len])
            .ok()
            .map(|_| decompressed_len)
    })
}

/// Resolves the snappy domain by attempting to decode `msg.data`, then
/// delegates to [`networking::compute_gossipsub_message_id`]. Wired in
/// as the gossipsub `message_id_fn` at host build time.
///
/// On a valid snappy payload, the decompressed bytes are snapshotted
/// into [`DECOMPRESSED_CACHE`] keyed by the freshly-computed
/// [`gossipsub::MessageId`] so the inbound routing layer can skip a
/// second decompression — libp2p calls this function and emits the
/// matching `Event::Message` within the same `Swarm::poll_next` cycle
/// on the swarm-poll task, so the cache is consumed before any other
/// gossipsub call can evict it.
pub(crate) fn gossipsub_message_id(msg: &gossipsub::Message) -> gossipsub::MessageId {
    let valid_len = is_valid_snappy(&msg.data);
    let domain = if valid_len.is_some() {
        MESSAGE_DOMAIN_VALID_SNAPPY
    } else {
        MESSAGE_DOMAIN_INVALID_SNAPPY
    };
    let id = gossipsub::MessageId::from(compute_gossipsub_message_id(
        domain,
        msg.topic.as_str().as_bytes(),
        &msg.data,
    ));

    if let Some(len) = valid_len {
        // Snapshot scratch → cache. The clone is a memcpy and amortises
        // against the avoided second snappy decompression in
        // `route_gossipsub_message` (snappy ≈ 1 GB/s; memcpy ≈ 10–30 GB/s).
        let bytes = SNAPPY_SCRATCH.with_borrow(|scratch| scratch[..len].to_vec());
        DECOMPRESSED_CACHE.with_borrow_mut(|cache| *cache = Some((id.clone(), bytes)));
    }
    id
}

/// Returns and clears the cached decompressed payload if the most-recent
/// [`gossipsub_message_id`] call computed `id`. Returns `None` on a
/// cache miss (different id, the entry was already taken, or the
/// previous payload was not a valid snappy frame). Single-slot,
/// thread-local; intended to be called once per inbound message from
/// [`crate::gossip::handler::route_gossipsub_message`].
pub(crate) fn take_decompressed_for(id: &gossipsub::MessageId) -> Option<Vec<u8>> {
    DECOMPRESSED_CACHE.with_borrow_mut(|cache| {
        cache
            .take_if(|(cached_id, _)| cached_id == id)
            .map(|(_, v)| v)
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use libp2p::gossipsub::{Message, TopicHash};
    use networking::{BLOCK_TOPIC_V1, VOTE_TOPIC_V1};

    fn message(topic: &str, data: Vec<u8>) -> Message {
        Message {
            source: None,
            data,
            sequence_number: None,
            topic: TopicHash::from_raw(topic.to_owned()),
        }
    }

    fn expected_id(domain: [u8; 4], topic: &str, data: &[u8]) -> gossipsub::MessageId {
        gossipsub::MessageId::from(
            compute_gossipsub_message_id(domain, topic.as_bytes(), data).to_vec(),
        )
    }

    fn snappy_encode(raw: &[u8]) -> Vec<u8> {
        snap::raw::Encoder::new().compress_vec(raw).unwrap()
    }

    #[test]
    fn message_id_picks_domain_by_snappy_validity() {
        // (case_name, topic, payload, expected_domain)
        let cases: [(&str, &str, Vec<u8>, [u8; 4]); 2] = [
            (
                "invalid_snappy",
                BLOCK_TOPIC_V1,
                vec![0xFF, 0xFF, 0xFF, 0xFF],
                MESSAGE_DOMAIN_INVALID_SNAPPY,
            ),
            (
                "valid_snappy",
                VOTE_TOPIC_V1,
                snappy_encode(b"hello world"),
                MESSAGE_DOMAIN_VALID_SNAPPY,
            ),
        ];
        for (name, topic, payload, domain) in cases {
            let got = gossipsub_message_id(&message(topic, payload.clone()));
            assert_eq!(got, expected_id(domain, topic, &payload), "case {name}");
        }
    }

    #[test]
    fn is_valid_snappy_rejects_empty_input() {
        assert!(is_valid_snappy(&[]).is_none());
    }

    #[test]
    fn is_valid_snappy_rejects_truncated_header() {
        assert!(is_valid_snappy(&[0xFF, 0xFF]).is_none());
    }

    #[test]
    fn is_valid_snappy_rejects_oversize_claim() {
        // LEB128 varint for 32 MiB (0x02000000), well past the 16 MiB
        // MAX_SNAPPY_DECOMPRESSED cap; no body follows. The cap path
        // must reject before any buffer allocation is attempted.
        const _: () = assert!(MAX_SNAPPY_DECOMPRESSED < 32 * 1024 * 1024);
        let bogus = [0x80, 0x80, 0x80, 0x10];
        assert!(is_valid_snappy(&bogus).is_none());
    }

    #[test]
    fn is_valid_snappy_accepts_round_trip() {
        let raw = b"payload";
        let encoded = snappy_encode(raw);
        let len = is_valid_snappy(&encoded).expect("valid snappy frame");
        assert_eq!(len, raw.len());
    }

    #[test]
    fn decompressed_cache_round_trips_through_gossipsub_message_id() {
        // Computing the message ID on a valid snappy payload populates
        // the cache; the routing layer (via `take_decompressed_for`)
        // then consumes it once. A miss with a different id, and a
        // second take of the same id, both return None.
        let raw = b"unit-test payload";
        let encoded = snappy_encode(raw);
        let msg = message(BLOCK_TOPIC_V1, encoded);
        let id = gossipsub_message_id(&msg);

        let other = gossipsub::MessageId::from(vec![0u8; 20]);
        assert!(take_decompressed_for(&other).is_none(), "miss on wrong id");

        let bytes = take_decompressed_for(&id).expect("cache must be populated");
        assert_eq!(bytes, raw, "cached bytes must equal pre-snappy payload");
        assert!(
            take_decompressed_for(&id).is_none(),
            "cache slot must clear after take",
        );
    }

    #[test]
    fn invalid_snappy_payload_does_not_populate_cache() {
        let msg = message(BLOCK_TOPIC_V1, vec![0xFF, 0xFF, 0xFF, 0xFF]);
        let id = gossipsub_message_id(&msg);
        assert!(
            take_decompressed_for(&id).is_none(),
            "no cache entry for invalid snappy frames",
        );
    }

    #[test]
    fn build_succeeds_with_valid_inputs() {
        let keypair = Keypair::generate_ed25519();
        let agent = AgentVersion::new("test/0.1.0").unwrap();
        let _ = DevnetBehaviour::build(&keypair, &agent).unwrap();
    }
}
