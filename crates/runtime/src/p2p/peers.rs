//! Shared peer registry — connected set, per-peer [`Status`] cache, and
//! bounded connect-event subscribers.
//!
//! Owned as an `Arc<PeerRegistry>` by [`P2pService`](super::P2pService) and
//! cloned into the swarm-poll task. The swarm task writes (connect /
//! disconnect / handshaked status); the public [`P2pService`] surface reads
//! (`connected_peers`, `peer_status`) and registers subscribers
//! (`subscribe_connected_peers`). All state sits behind one `RwLock`, so the
//! `sync` module reaches the outbound peer view without a trait port or any
//! `libp2p` dependency (it speaks base-58 `String` peer ids).

use std::collections::{HashMap, HashSet};

use lean_wire::Status;
use libp2p::PeerId;
use parking_lot::RwLock;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tracing::debug;

/// Connected peers + their last handshaked [`Status`] + bounded
/// connect-event subscribers.
#[derive(Default)]
pub(crate) struct PeerRegistry {
    inner: RwLock<Inner>,
}

#[derive(Default)]
struct Inner {
    connected: HashSet<PeerId>,
    statuses: HashMap<PeerId, Status>,
    subscribers: Vec<mpsc::Sender<String>>,
}

impl PeerRegistry {
    /// Records a newly-connected peer and notifies every subscriber over
    /// its bounded channel. A full channel drops the event (lossy but
    /// bounded — the sync loop dedups and retries); a closed channel evicts
    /// the dead subscriber.
    pub(crate) fn on_connect(&self, peer: PeerId) {
        let id = peer.to_base58();
        let mut inner = self.inner.write();
        inner.connected.insert(peer);
        inner
            .subscribers
            .retain(|tx| match tx.try_send(id.clone()) {
                Ok(()) | Err(TrySendError::Full(_)) => true,
                Err(TrySendError::Closed(_)) => false,
            });
    }

    /// Drops a disconnected peer from the connected set and status cache.
    pub(crate) fn on_disconnect(&self, peer: &PeerId) {
        let mut inner = self.inner.write();
        inner.connected.remove(peer);
        inner.statuses.remove(peer);
    }

    /// Caches `peer`'s handshaked [`Status`] for later readback.
    pub(crate) fn set_status(&self, peer: PeerId, status: Status) {
        self.inner.write().statuses.insert(peer, status);
    }

    /// Returns `peer`'s last handshaked [`Status`], if cached.
    pub(crate) fn status_of(&self, peer: &PeerId) -> Option<Status> {
        self.inner.read().statuses.get(peer).copied()
    }

    /// Snapshot of currently-connected peers as base-58 strings.
    pub(crate) fn connected(&self) -> Vec<String> {
        self.inner
            .read()
            .connected
            .iter()
            .map(|peer| peer.to_base58())
            .collect()
    }

    /// Registers a bounded subscriber and returns its receiver. `bound` is
    /// floored at 1 (an `mpsc` channel cannot have capacity 0).
    pub(crate) fn subscribe(&self, bound: usize) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel(bound.max(1));
        self.inner.write().subscribers.push(tx);
        debug!(bound, "registered connected-peer subscriber");
        rx
    }
}
