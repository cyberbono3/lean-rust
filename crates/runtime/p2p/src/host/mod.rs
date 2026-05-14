//! Host construction primitives + the clone-friendly [`Host`] handle.
//!
//! The handle holds an `mpsc::Sender<HostCommand>` only; the
//! `Swarm<DevnetBehaviour>` is owned by exactly one swarm-poll task
//! spawned at [`crate::service::P2pService::start`]. Current scope only
//! defines the `Shutdown` command — gossip publish / req/resp send
//! extend the enum in later additions.

use libp2p::PeerId;
use tokio::sync::mpsc;

pub(crate) mod behaviour;
pub(crate) mod bootnodes;
pub(crate) mod identity;
pub(crate) mod transport;

/// Capacity of the host-command channel.
///
/// Sized to absorb a brief burst of commands without blocking senders;
/// the consuming task drains in a tight `select!` loop so this should
/// rarely matter under steady state.
pub(crate) const COMMAND_CHANNEL_CAPACITY: usize = 64;

/// Commands the [`Host`] handle dispatches to the swarm-poll task.
///
/// The enum is `#[non_exhaustive]` so later variants (gossip publish,
/// `request_response` send) can be added without churning every match
/// arm in this crate.
#[derive(Debug)]
#[non_exhaustive]
pub(crate) enum HostCommand {
    /// Cancel the swarm-poll loop. Sent at `Service::stop`.
    Shutdown,
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
    /// task in [`crate::service`] can issue `Shutdown` on cancellation.
    pub(crate) fn commands(&self) -> &mpsc::Sender<HostCommand> {
        &self.commands
    }
}
