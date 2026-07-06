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
    /// Records a newly-connected peer. Does **not** notify sync subscribers —
    /// that happens in [`Self::set_status`] once the connect handshake has
    /// populated the peer's `Status`. Notifying on bare connect would wake
    /// the sync walk before the handshake reply lands, so it would read an
    /// empty status cache and no-op with no retry.
    pub(crate) fn on_connect(&self, peer: PeerId) {
        self.inner.write().connected.insert(peer);
    }

    /// Drops a disconnected peer from the connected set and status cache.
    pub(crate) fn on_disconnect(&self, peer: &PeerId) {
        let mut inner = self.inner.write();
        inner.connected.remove(peer);
        inner.statuses.remove(peer);
    }

    /// Caches `peer`'s handshaked [`Status`] and notifies sync subscribers
    /// that the peer is ready to sync — its head is now known. Firing the
    /// connect event here (not in [`Self::on_connect`]) is load-bearing: the
    /// sync walk reads `peer_status` immediately on this event, so it must
    /// see a populated cache. A full channel drops the event (bounded,
    /// lossy — the watch loop dedups and re-syncs on the next status); a
    /// closed channel evicts the dead subscriber.
    pub(crate) fn set_status(&self, peer: PeerId, status: Status) {
        let id = peer.to_base58();
        let mut inner = self.inner.write();
        inner.statuses.insert(peer, status);
        inner
            .subscribers
            .retain(|tx| match tx.try_send(id.clone()) {
                Ok(()) | Err(TrySendError::Full(_)) => true,
                Err(TrySendError::Closed(_)) => false,
            });
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
        let mut inner = self.inner.write();
        // Drop senders whose receiver is gone (e.g. a stopped sync service)
        // so repeated subscribes without an intervening connect cannot grow
        // the vec unboundedly.
        inner.subscribers.retain(|s| !s.is_closed());
        inner.subscribers.push(tx);
        debug!(bound, "registered connected-peer subscriber");
        rx
    }
}
