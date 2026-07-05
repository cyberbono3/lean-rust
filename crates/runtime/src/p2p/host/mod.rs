//! Host construction primitives + the clone-friendly [`Host`] handle.
//!
//! The handle holds an `mpsc::Sender<HostCommand>` only; the
//! `Swarm<DevnetBehaviour>` is owned by exactly one swarm-poll task
//! spawned at [`crate::p2p::service::P2pService::start`]. The command enum
//! covers three operations — `Shutdown` (lifecycle teardown), `Publish`
//! (gossipsub publish via the typed [`crate::p2p::gossip::publisher`]
//! methods), and `SendRequest` (outbound `BlocksByRoot` RPC via
//! [`crate::p2p::rpc::client`]) — and is `#[non_exhaustive]` so further
//! variants can land without a breaking match-arm churn.

use libp2p::{gossipsub, PeerId};
use tokio::sync::{mpsc, oneshot};

pub(crate) mod behaviour;
pub(crate) mod bootnodes;
pub(crate) mod keypair;
pub(crate) mod transport;

/// Capacity of the host-command channel.
///
/// Sized to absorb a brief burst of commands without blocking senders;
/// the consuming task drains in a tight `select!` loop so this should
/// rarely matter under steady state.
pub(crate) const COMMAND_CHANNEL_CAPACITY: usize = 64;

/// Commands the [`Host`] handle dispatches to the swarm-poll task.
///
/// The enum is `#[non_exhaustive]` so later variants
/// (`request_response` send, etc.) can be added without churning every
/// match arm in this crate.
#[derive(Debug)]
#[non_exhaustive]
pub(crate) enum HostCommand {
    /// Cancel the swarm-poll loop. Sent at `Service::stop`.
    Shutdown,
    /// Publish a gossipsub message and reply with the resulting
    /// [`gossipsub::MessageId`] or libp2p [`gossipsub::PublishError`].
    /// Constructed by [`crate::p2p::Host::publish_block`] /
    /// [`crate::p2p::Host::publish_vote`].
    Publish {
        /// Pre-built libp2p topic (constructed from the canonical
        /// [`lean_wire::BLOCK_TOPIC_V1`] / [`lean_wire::VOTE_TOPIC_V1`]
        /// strings via [`gossipsub::IdentTopic::new`]).
        topic: gossipsub::IdentTopic,
        /// SSZ + Snappy-block-compressed payload — produced upstream by
        /// [`lean_wire::encode_gossip`] so the swarm task does not need
        /// to know the payload type.
        payload: Vec<u8>,
        /// One-shot reply channel — the swarm task forwards the libp2p
        /// publish result here; the caller maps it into a typed
        /// [`crate::p2p::gossip::PublishError`].
        reply: oneshot::Sender<Result<gossipsub::MessageId, gossipsub::PublishError>>,
    },
    /// Send an outbound RPC request to `peer` and reply with the typed
    /// [`crate::p2p::rpc::RpcResponse`] (or an [`crate::p2p::rpc::RpcError`] on
    /// failure). Constructed by [`crate::p2p::Host::send_blocks_by_root`].
    SendRequest {
        /// Target peer for the request.
        peer: PeerId,
        /// Typed request — the swarm task hands it directly to
        /// `request_response::Behaviour::send_request`.
        request: crate::p2p::rpc::RpcRequest,
        /// One-shot reply channel — the swarm task parks it in the
        /// outbound correlation table until the matching libp2p
        /// response or failure event arrives.
        reply: oneshot::Sender<Result<crate::p2p::rpc::RpcResponse, crate::p2p::rpc::RpcError>>,
    },
}

/// Cheap clone-friendly handle pointing at one swarm-poll task.
///
/// `Host` is the only externally visible surface for interacting with
/// the running swarm. Cloning it returns a fresh `mpsc::Sender` to the
/// same task.
#[derive(Debug, Clone)]
pub struct Host {
    peer_id: PeerId,
    commands: mpsc::Sender<HostCommand>,
}

impl Host {
    pub(crate) fn new(peer_id: PeerId, commands: mpsc::Sender<HostCommand>) -> Self {
        Self { peer_id, commands }
    }

    /// Returns the local peer id of the host. Stable across the host's
    /// lifetime — derived from the identity keypair persisted on disk.
    #[must_use]
    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    /// Borrowed view of the command channel. `pub(crate)` so the swarm
    /// task in [`crate::p2p::service`] can issue `Shutdown` on cancellation.
    pub(crate) fn commands(&self) -> &mpsc::Sender<HostCommand> {
        &self.commands
    }
}
