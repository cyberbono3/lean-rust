//! Shared test doubles + helpers for the sync integration tests.
//!
//! Included via `mod sync_common;` from each `tests/*.rs` file (each is its own
//! crate, so the helpers are compiled into each). `dead_code` is allowed
//! because not every test file uses every helper.

#![allow(dead_code, clippy::expect_used, clippy::unwrap_used, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use runtime::sync::{PeerEventProvider, PeerId, SyncError};
use tokio::sync::mpsc;

/// In-memory [`PeerEventProvider`]: `subscribe` hands back a bounded
/// receiver and stashes the sender so a test can push peer-connect events
/// after `start` and close the channel by dropping the sender.
pub struct ChannelPeers {
    handle: Mutex<Option<mpsc::Sender<PeerId>>>,
}

impl ChannelPeers {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            handle: Mutex::new(None),
        })
    }

    /// Clone of the sender surfaced after `subscribe`. Panics if called
    /// before the loop subscribed.
    pub fn sender(&self) -> mpsc::Sender<PeerId> {
        self.handle
            .lock()
            .as_ref()
            .expect("subscribe before sender")
            .clone()
    }
}

#[async_trait]
impl PeerEventProvider for ChannelPeers {
    async fn subscribe_outbound_connected_peers(
        &self,
    ) -> Result<mpsc::Receiver<PeerId>, SyncError> {
        let (tx, rx) = mpsc::channel(256);
        *self.handle.lock() = Some(tx);
        Ok(rx)
    }
}

/// Polls `cond` every 2 ms until it holds or `deadline_ms` elapses,
/// returning the final value of `cond`. Bounded so a missed condition
/// surfaces as a test failure rather than hanging.
pub async fn poll_until(deadline_ms: u64, cond: impl Fn() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(deadline_ms);
    while tokio::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    cond()
}
