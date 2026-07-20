//! Per-peer inbound admission bound for the gossip ingress.
//!
//! Caps how many un-imported gossip messages a single source peer may have in flight
//! (queued + in-processing), so one peer cannot monopolize the shared ingress. Excess
//! is dropped at ingress (gossipsub mesh replay covers legitimate loss). Keyed on
//! [`crate::sync::PeerId`] so this type never compiles against `libp2p`.

use core::num::NonZeroUsize;
use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::sync::PeerId;

/// Builds a [`NonZeroUsize`] from a literal at compile time; panics if the input is
/// zero. Mirrors the `sync::config` idiom for non-zero associated constants.
const fn nz(n: usize) -> NonZeroUsize {
    match NonZeroUsize::new(n) {
        Some(v) => v,
        None => panic!("expected non-zero constant"),
    }
}

/// Admission tunables. `#[non_exhaustive]` so fields can grow without breaking callers.
/// Crate-internal: built by `P2pService` at start; no external consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub(crate) struct AdmissionConfig {
    /// Max un-imported gossip messages permitted in flight per source peer.
    pub(crate) max_in_flight_per_peer: NonZeroUsize,
}

impl AdmissionConfig {
    /// Default per-peer inbound depth. Bounds per-peer memory (this many block/vote
    /// clones) while leaving ample headroom for a healthy peer's steady-state gossip.
    pub(crate) const DEFAULT_MAX_IN_FLIGHT_PER_PEER: NonZeroUsize = nz(32);

    /// Builds a config from an explicit per-peer depth.
    #[must_use]
    pub(crate) const fn new(max_in_flight_per_peer: NonZeroUsize) -> Self {
        Self {
            max_in_flight_per_peer,
        }
    }
}

impl Default for AdmissionConfig {
    fn default() -> Self {
        Self::new(Self::DEFAULT_MAX_IN_FLIGHT_PER_PEER)
    }
}

/// Per-peer in-flight counter. Entries are created lazily and removed at zero, so a
/// churning peer set cannot leak entries. Crate-internal — the only external leak is the
/// [`AdmitGuard`] it hands out via the public receiver aliases.
#[derive(Debug)]
pub(crate) struct PeerAdmission {
    in_flight: Mutex<HashMap<PeerId, usize>>,
    cap: NonZeroUsize,
}

impl PeerAdmission {
    /// Builds an admission bound with the given config, ready to share across the
    /// ingress task and the drain via `Arc`.
    #[must_use]
    pub(crate) fn new(config: AdmissionConfig) -> Arc<Self> {
        Arc::new(Self {
            in_flight: Mutex::new(HashMap::new()),
            cap: config.max_in_flight_per_peer,
        })
    }

    /// Admits one message for `peer` if it is under its cap, returning a guard whose
    /// drop releases the slot. Returns `None` when the peer is at cap — the caller
    /// drops the message.
    #[must_use]
    pub(crate) fn try_admit(self: &Arc<Self>, peer: &PeerId) -> Option<AdmitGuard> {
        let mut map = self.in_flight.lock();
        let count = map.entry(peer.clone()).or_insert(0);
        if *count >= self.cap.get() {
            return None;
        }
        *count += 1;
        Some(AdmitGuard {
            admission: Arc::clone(self),
            peer: peer.clone(),
        })
    }

    /// Releases one in-flight slot for `peer` (called from [`AdmitGuard`]'s `Drop`).
    /// Decrements the peer's counter and removes the entry once it reaches zero, so
    /// idle peers leave no residue in the map.
    fn release(&self, peer: &PeerId) {
        let mut map = self.in_flight.lock();
        if let Some(count) = map.get_mut(peer) {
            // Under the invariant (a live guard's entry always has count >= 1) this is
            // count-1; `saturating_sub` + the debug-assert make it fail-safe against a
            // future refactor breaking the invariant, so a release build can never
            // underflow-wrap and leak the entry.
            debug_assert!(*count > 0, "release of an unheld admission slot");
            *count = count.saturating_sub(1);
            if *count == 0 {
                map.remove(peer);
            }
        }
    }

    /// Number of peers currently holding at least one slot. Test-only introspection to
    /// assert the map is pruned at zero (no entry leak).
    #[cfg(test)]
    pub(crate) fn tracked_peer_count(&self) -> usize {
        self.in_flight.lock().len()
    }

    /// The number of in-flight slots currently held for `peer` (0 if absent). Test-only
    /// introspection to detect a per-peer slot leak, which the peer-entry count alone
    /// cannot (a leaked slot leaves the entry at count 1+, still one tracked peer).
    #[cfg(test)]
    pub(crate) fn in_flight_for(&self, peer: &PeerId) -> usize {
        self.in_flight.lock().get(peer).copied().unwrap_or(0)
    }
}

/// RAII slot held from admission until the message is imported (and dropped). Rides the
/// ingress channel with its payload; its `Drop` frees the peer's slot.
///
/// MUST NOT derive/implement `Clone`: exactly one increment per guard and one decrement
/// per drop is the load-bearing accounting invariant (a second copy would
/// double-release).
#[derive(Debug)]
pub struct AdmitGuard {
    admission: Arc<PeerAdmission>,
    peer: PeerId,
}

impl AdmitGuard {
    /// The source peer this slot was admitted for — lets the drain log `%peer` on an
    /// import failure now that the channel carries the guard, not a bare `PeerId`.
    #[must_use]
    pub fn peer(&self) -> &PeerId {
        &self.peer
    }
}

impl Drop for AdmitGuard {
    fn drop(&mut self) {
        self.admission.release(&self.peer);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn peer(id: &str) -> PeerId {
        PeerId::new(id).unwrap()
    }

    fn admission(cap: usize) -> Arc<PeerAdmission> {
        PeerAdmission::new(AdmissionConfig::new(NonZeroUsize::new(cap).unwrap()))
    }

    #[test]
    fn admits_up_to_cap_then_drops() {
        let adm = admission(2);
        let p = peer("peerA");

        let g1 = adm.try_admit(&p);
        let g2 = adm.try_admit(&p);
        assert!(g1.is_some() && g2.is_some(), "two admits under cap succeed");
        // At cap → dropped.
        assert!(adm.try_admit(&p).is_none(), "third admit at cap is refused");

        // Freeing one slot admits again.
        drop(g1);
        assert!(adm.try_admit(&p).is_some(), "freed slot admits again");
        drop(g2);
    }

    #[test]
    fn distinct_peers_independent() {
        let adm = admission(1);
        let a = peer("peerA");
        let b = peer("peerB");

        let _ga = adm.try_admit(&a).expect("peer a admitted");
        assert!(adm.try_admit(&a).is_none(), "peer a at cap");
        // Peer b is unaffected by peer a's saturation.
        assert!(adm.try_admit(&b).is_some(), "peer b independent");
    }

    #[test]
    fn guard_release_frees_slot_and_prunes() {
        let adm = admission(1);
        let p = peer("peerA");

        let g = adm.try_admit(&p).expect("admitted");
        assert_eq!(adm.tracked_peer_count(), 1);
        drop(g);
        // Last guard dropped → entry removed, not just decremented.
        assert_eq!(adm.tracked_peer_count(), 0, "idle entry pruned at zero");
    }

    #[test]
    fn admission_default_is_config_sourced() {
        assert_eq!(
            AdmissionConfig::default().max_in_flight_per_peer,
            AdmissionConfig::DEFAULT_MAX_IN_FLIGHT_PER_PEER,
        );
    }
}
