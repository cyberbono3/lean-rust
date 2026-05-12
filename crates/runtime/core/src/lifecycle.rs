//! `Node` lifecycle: ordered start with start-time unwinding, reverse-
//! ordered stop with best-effort error collection, aggregated status, and
//! a one-shot `run` driver.

use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};

use crate::error::NodeError;
use crate::node::{NamedService, Node};
use crate::service::Service;

impl Node {
    /// Brings every wired service up in slot order
    /// (`chain → p2p → sync → duties → http → metrics`).
    ///
    /// # Errors
    /// - [`NodeError::AlreadyStarted`] if `start` was already called.
    /// - [`NodeError::Start`] if any service errors. Services that already
    ///   started in this call are stopped in reverse order before the
    ///   error returns (start-time unwinding).
    #[instrument(skip(self))]
    pub async fn start(&self) -> Result<(), NodeError> {
        let services = {
            let mut state = self.state.lock();
            if state.is_some() {
                return Err(NodeError::AlreadyStarted);
            }
            *state = Some(Vec::new());
            self.ordered_slots()
        };

        let mut started: Vec<NamedService> = Vec::with_capacity(services.len());
        for (name, svc) in services {
            match svc.start().await {
                Ok(()) => {
                    debug!(service = name, "ServiceStarted");
                    started.push((name, svc));
                }
                Err(source) => {
                    warn!(service = name, %source, "ServiceStartFailed");
                    let cancel = derive_stop_token(self.config.shutdown_timeout);
                    stop_services_reverse(&started, &cancel).await;
                    *self.state.lock() = None;
                    return Err(NodeError::Start {
                        service: name,
                        source,
                    });
                }
            }
        }

        info!(count = started.len(), "ServicesStarted");
        *self.state.lock() = Some(started);
        Ok(())
    }

    /// Brings every previously-started service down in reverse slot order.
    ///
    /// Best-effort: all started services receive a `stop` call even if
    /// one returns an error or exceeds the shutdown deadline. The first
    /// failure surfaces; subsequent failures are logged via
    /// [`tracing::warn`].
    ///
    /// # Errors
    /// [`NodeError::Stop`] if any service errors during shutdown.
    #[instrument(skip(self))]
    pub async fn stop(&self) -> Result<(), NodeError> {
        let Some(started) = self.state.lock().take() else {
            return Ok(());
        };
        let cancel = derive_stop_token(self.config.shutdown_timeout);
        let first_error = stop_services_reverse(&started, &cancel).await;
        match first_error {
            Some((service, source)) => Err(NodeError::Stop { service, source }),
            None => Ok(()),
        }
    }

    /// Aggregates the health status of every wired service.
    ///
    /// Iterates slots in start order, collecting any errors. Returns
    /// [`Ok`] when every service reports healthy.
    ///
    /// # Errors
    /// [`NodeError::Status`] when one or more services report unhealthy.
    #[instrument(skip(self))]
    pub async fn status(&self) -> Result<(), NodeError> {
        let mut errors: Vec<(&'static str, anyhow::Error)> = Vec::new();
        for (name, svc) in self.ordered_slots() {
            if let Err(source) = svc.status().await {
                errors.push((name, source));
            }
        }
        if errors.is_empty() {
            return Ok(());
        }
        let summary = errors
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(", ");
        Err(NodeError::Status {
            count: errors.len(),
            summary,
            errors,
        })
    }

    /// Convenience driver: `start`, then await `shutdown.cancelled()`,
    /// then `stop`. The stop phase uses a fresh deadline derived from
    /// [`NodeConfig::shutdown_timeout`](crate::NodeConfig::shutdown_timeout)
    /// — independent of the caller's `shutdown` token, so a long graceful
    /// budget still applies after SIGINT.
    ///
    /// # Errors
    /// Propagates [`NodeError`] from either lifecycle phase.
    #[instrument(skip(self, shutdown))]
    pub async fn run(&self, shutdown: CancellationToken) -> Result<(), NodeError> {
        self.start().await?;
        info!("NodeRunning");
        shutdown.cancelled().await;
        info!("NodeStopping");
        self.stop().await
    }
}

/// Stops `services` in reverse order. Returns the first error encountered;
/// subsequent errors are logged.
async fn stop_services_reverse(
    services: &[NamedService],
    cancel: &CancellationToken,
) -> Option<(&'static str, anyhow::Error)> {
    let mut first_error: Option<(&'static str, anyhow::Error)> = None;
    for (name, svc) in services.iter().rev() {
        match stop_one(svc, cancel).await {
            Ok(()) => debug!(service = name, "ServiceStopped"),
            Err(source) => {
                warn!(service = name, %source, "ServiceStopFailed");
                if first_error.is_none() {
                    first_error = Some((*name, source));
                }
            }
        }
    }
    first_error
}

/// Wraps a single `service.stop` call with the node-level deadline.
async fn stop_one(svc: &Arc<dyn Service>, cancel: &CancellationToken) -> anyhow::Result<()> {
    // The token already encodes the deadline (it will be cancelled when
    // the budget elapses). Race the stop future against the token: when
    // the token fires we surface a deadline error so the offending
    // service is named in the returned NodeError.
    tokio::select! {
        biased;
        result = svc.stop(cancel.clone()) => result,
        () = cancel.cancelled() => Err(anyhow!("shutdown deadline exceeded")),
    }
}

/// Builds the cancellation token a stop phase passes to its services.
///
/// - `Duration::ZERO` → token that is never cancelled; services may take
///   as long as they need.
/// - Any other duration → token that is cancelled when the budget elapses
///   via a spawned timer.
fn derive_stop_token(shutdown_timeout: Duration) -> CancellationToken {
    let token = CancellationToken::new();
    if !shutdown_timeout.is_zero() {
        let cancel = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(shutdown_timeout).await;
            cancel.cancel();
        });
    }
    token
}
