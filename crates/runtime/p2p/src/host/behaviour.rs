//! Composite [`libp2p::swarm::NetworkBehaviour`] for the devnet0 host.
//!
//! Combines gossipsub, `request_response` (for the `Status` and
//! `BlocksByRoot` protocols), identify, and ping into a single typed
//! behaviour. The gossipsub message-id function is wired to the
//! deterministic 20-byte primitive in [`networking::compute_gossipsub_message_id`].
//!
//! Request/response handler logic lives outside this crate — the
//! [`SszSnappyCodec`] stub below is a placeholder that returns
//! "unsupported" until the real handler replaces it.

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
    compute_gossipsub_message_id, BLOCKS_BY_ROOT_PROTOCOL_V1, MESSAGE_DOMAIN_INVALID_SNAPPY,
    MESSAGE_DOMAIN_VALID_SNAPPY, STATUS_PROTOCOL_V1,
};

use crate::error::{HostError, HostResult};
use crate::options::AgentVersion;

mod codec;
pub(crate) use codec::SszSnappyCodec;

/// Application-specific identify protocol-version string advertised at
/// the libp2p identify handshake.
pub(crate) const IDENTIFY_PROTOCOL_VERSION: &str = "lean/0.1.0";

/// Composite behaviour driven by the host swarm task.
///
/// Field names double as gossipsub / `request_response` / identify / ping
/// dispatch tags via the `NetworkBehaviour` derive macro — renaming a
/// field renames the generated `DevnetBehaviourEvent` variant.
#[derive(NetworkBehaviour)]
pub(crate) struct DevnetBehaviour {
    pub(crate) gossipsub: gossipsub::Behaviour,
    pub(crate) request_response: request_response::Behaviour<SszSnappyCodec>,
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
            gossipsub: build_gossipsub()?,
            request_response: build_request_response(),
            identify: build_identify(keypair, agent_version),
            ping: ping::Behaviour::new(ping::Config::new()),
        })
    }
}

fn build_identify(keypair: &Keypair, agent_version: &AgentVersion) -> identify::Behaviour {
    identify::Behaviour::new(
        identify::Config::new(IDENTIFY_PROTOCOL_VERSION.to_owned(), keypair.public())
            .with_agent_version(agent_version.as_str().to_owned()),
    )
}

fn build_gossipsub() -> HostResult<gossipsub::Behaviour> {
    // Devnet0 publishes unsigned messages; `Anonymous` authenticity
    // pairs with the message-id function below to give deterministic
    // 20-byte IDs derived purely from `(topic, payload)`.
    let config = gossipsub::ConfigBuilder::default()
        .validation_mode(gossipsub::ValidationMode::Anonymous)
        .heartbeat_interval(Duration::from_secs(1))
        .message_id_fn(gossipsub_message_id)
        .build()
        .map_err(|err| HostError::GossipsubInit(err.to_string()))?;
    gossipsub::Behaviour::new(gossipsub::MessageAuthenticity::Anonymous, config)
        .map_err(|err| HostError::GossipsubInit(err.to_string()))
}

fn build_request_response() -> request_response::Behaviour<SszSnappyCodec> {
    let protocols = [STATUS_PROTOCOL_V1, BLOCKS_BY_ROOT_PROTOCOL_V1]
        .map(|p| (StreamProtocol::new(p.as_str()), ProtocolSupport::Full));
    request_response::Behaviour::new(protocols, request_response::Config::default())
}

/// Defensive upper bound on a single decompressed gossipsub payload. A
/// peer claiming a larger frame is rejected as invalid without
/// allocating the buffer.
const MAX_SNAPPY_DECOMPRESSED: usize = 16 * 1024 * 1024;

thread_local! {
    static SNAPPY_PROBE: RefCell<(snap::raw::Decoder, Vec<u8>)> =
        RefCell::new((snap::raw::Decoder::new(), Vec::with_capacity(4096)));
}

/// Reports whether `data` is a fully-decodable snappy frame. Reuses a
/// thread-local decoder + scratch buffer to keep this call alloc-free
/// on the gossipsub hot path.
fn is_valid_snappy(data: &[u8]) -> bool {
    let Ok(decompressed_len) = snap::raw::decompress_len(data) else {
        return false;
    };
    if decompressed_len > MAX_SNAPPY_DECOMPRESSED {
        return false;
    }
    SNAPPY_PROBE.with(|cell| {
        let (decoder, buf) = &mut *cell.borrow_mut();
        if buf.len() < decompressed_len {
            buf.resize(decompressed_len, 0);
        }
        decoder
            .decompress(data, &mut buf[..decompressed_len])
            .is_ok()
    })
}

/// Resolves the snappy domain by attempting to decode `msg.data`, then
/// delegates to [`networking::compute_gossipsub_message_id`].
fn gossipsub_message_id(msg: &gossipsub::Message) -> gossipsub::MessageId {
    let domain = if is_valid_snappy(&msg.data) {
        MESSAGE_DOMAIN_VALID_SNAPPY
    } else {
        MESSAGE_DOMAIN_INVALID_SNAPPY
    };
    let id = compute_gossipsub_message_id(domain, msg.topic.as_str().as_bytes(), &msg.data);
    gossipsub::MessageId::from(id.to_vec())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use libp2p::gossipsub::{Message, TopicHash};

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
    fn message_id_invalid_snappy_path() {
        let payload = vec![0xFF, 0xFF, 0xFF, 0xFF]; // undecodable
        let topic = "/lean/block";
        let got = gossipsub_message_id(&message(topic, payload.clone()));
        assert_eq!(
            got,
            expected_id(MESSAGE_DOMAIN_INVALID_SNAPPY, topic, &payload)
        );
    }

    #[test]
    fn message_id_valid_snappy_path() {
        let encoded = snappy_encode(b"hello world");
        let topic = "/lean/vote";
        let got = gossipsub_message_id(&message(topic, encoded.clone()));
        assert_eq!(
            got,
            expected_id(MESSAGE_DOMAIN_VALID_SNAPPY, topic, &encoded)
        );
    }

    #[test]
    fn is_valid_snappy_rejects_empty_input() {
        assert!(!is_valid_snappy(&[]));
    }

    #[test]
    fn is_valid_snappy_rejects_truncated_header() {
        assert!(!is_valid_snappy(&[0xFF, 0xFF]));
    }

    #[test]
    fn is_valid_snappy_rejects_oversize_claim() {
        // LEB128 varint for 32 MiB (0x02000000), well past the 16 MiB
        // MAX_SNAPPY_DECOMPRESSED cap; no body follows. The cap path
        // must reject before any buffer allocation is attempted.
        const _: () = assert!(MAX_SNAPPY_DECOMPRESSED < 32 * 1024 * 1024);
        let bogus = [0x80, 0x80, 0x80, 0x10];
        assert!(!is_valid_snappy(&bogus));
    }

    #[test]
    fn is_valid_snappy_accepts_round_trip() {
        let encoded = snappy_encode(b"payload");
        assert!(is_valid_snappy(&encoded));
    }

    #[test]
    fn build_succeeds_with_valid_inputs() {
        let keypair = Keypair::generate_ed25519();
        let agent = AgentVersion::new("test/0.1.0").unwrap();
        let _ = DevnetBehaviour::build(&keypair, &agent).unwrap();
    }
}
