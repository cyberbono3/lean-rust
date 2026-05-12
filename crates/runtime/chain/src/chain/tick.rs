//! Background tick loop — advances the forkchoice clock by one interval
//! on every `SECONDS_PER_INTERVAL` boundary.
//!
//! The loop is spawned by [`super::Service::start`] and terminated by
//! cancelling its [`CancellationToken`] from [`super::Service::stop`].

use std::sync::Arc;
use std::time::Duration;

use engine::Engine;
use parking_lot::RwLock;
use tokio::time::{interval, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::cache::ChainSnapshot;

/// Tick period derived from the workspace-level slot timing constants.
pub(super) fn tick_period() -> Duration {
    Duration::from_secs(config::SECONDS_PER_INTERVAL)
}

/// Drives `engine.tick_interval(false)` once per [`tick_period`] until
/// `cancel` fires. `has_proposal` is hard-coded `false`; the duties
/// service (#30) flips it once block production lands.
pub(super) async fn run_tick_loop(
    engine: Engine,
    snapshot: Arc<RwLock<ChainSnapshot>>,
    cancel: CancellationToken,
) {
    let mut ticker = interval(tick_period());
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // The first `tick()` resolves immediately — burn it so the first
    // engine advance lines up with the first elapsed period.
    ticker.tick().await;

    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            _ = ticker.tick() => {
                if let Err(err) = engine.tick_interval(false) {
                    warn!(%err, "chain tick failed; continuing");
                    continue;
                }
                snapshot.write().refresh(&engine);
            }
        }
    }
}
