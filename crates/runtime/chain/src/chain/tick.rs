//! Background tick loop — advances the forkchoice clock by one interval
//! on every `SECONDS_PER_INTERVAL` boundary.
//!
//! The loop is spawned by [`super::Service::start`] and terminated by
//! cancelling its [`CancellationToken`] from [`super::Service::stop`].

use std::sync::Arc;
use std::time::Duration;

use crate::engine::Engine;
use parking_lot::RwLock;
use tokio::time::{interval_at, Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use tracing::{instrument, warn};

use super::cache::ChainSnapshot;

/// Tick period derived from the workspace-level slot timing constants.
const TICK_PERIOD: Duration = Duration::from_secs(config::SECONDS_PER_INTERVAL);

/// Drives `engine.tick_interval(false)` once per [`TICK_PERIOD`] until
/// `cancel` fires. `has_proposal` is hard-coded `false`; the duties
/// service will flip it once block production lands.
#[instrument(level = "trace", name = "chain.tick_loop", skip_all)]
pub(super) async fn run_tick_loop(
    engine: Engine,
    snapshot: Arc<RwLock<ChainSnapshot>>,
    cancel: CancellationToken,
) {
    let mut ticker = interval_at(Instant::now() + TICK_PERIOD, TICK_PERIOD);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            // `biased`: cancellation has priority over the tick — bounded teardown latency.
            biased;
            () = cancel.cancelled() => break,
            _ = ticker.tick() => {
                match engine.tick_interval(false) {
                    Ok(()) => *snapshot.write() = ChainSnapshot::from_engine(&engine),
                    Err(err) => warn!(%err, "chain tick failed; continuing"),
                }
            }
        }
    }
}
