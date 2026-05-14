//! [`P2pService`] — `runtime_core::Service` impl driving the libp2p swarm.
//!
//! Lifecycle:
//! 1. [`DevnetHost::build`](crate::DevnetHost::build) constructs the
//!    service in the `Idle` state — the `Swarm<DevnetBehaviour>` exists
//!    but no task is running.
//! 2. [`Service::start`] calls `Swarm::listen_on`, awaits the first
//!    `NewListenAddr` (or a listener error) with a 2-second deadline,
//!    dials the loaded bootnodes, and spawns the swarm-poll task. State
//!    transitions to `Running`.
//! 3. [`Service::stop`] sends `HostCommand::Shutdown`, joins the poll
//!    task (bounded by the supplied `CancellationToken`), and transitions
//!    to `Stopped`. Idempotent on a not-running service.
//!
//! The `Swarm` is owned by exactly one task. The public [`Host`] handle
//! reaches it via `mpsc::Sender<HostCommand>`.

use std::time::Duration;

use anyhow::anyhow;
use async_trait::async_trait;
use futures::StreamExt;
use libp2p::{swarm::SwarmEvent, Multiaddr, PeerId, Swarm};
use parking_lot::Mutex;
use tokio::{sync::mpsc, task::JoinHandle, time::sleep};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument, warn};

use runtime_core::Service;

use crate::error::{HostError, HostResult};
use crate::host::{
    behaviour::DevnetBehaviour, bootnodes::Bootnode, Host, HostCommand, COMMAND_CHANNEL_CAPACITY,
};
use crate::options::HostOptions;

/// How long [`Service::start`] waits for the first `NewListenAddr` /
/// `ListenerClosed(Err)` event before treating the bind as failed.
const BIND_DEADLINE: Duration = Duration::from_secs(2);

/// Long-lived service driven by the swarm-poll task.
///
/// Holds construction-time state (options, peer id, bootnodes, swarm)
/// behind a single mutex so the typed [`runtime_core::Service`] surface
/// can take `&self` everywhere.
pub struct P2pService {
    peer_id: PeerId,
    state: Mutex<State>,
}

enum State {
    /// Constructed but not yet started. Holds the assembled `Swarm`
    /// and the bootnodes pending dial.
    Idle {
        options: HostOptions,
        swarm: Box<Swarm<DevnetBehaviour>>,
        bootnodes: Vec<Bootnode>,
    },
    /// Swarm-poll task is running. Held until [`Service::stop`] drains
    /// the task.
    Running {
        host: Host,
        cancel: CancellationToken,
        join: JoinHandle<()>,
    },
    /// Lifecycle terminal: `stop` ran. `start` from this state would
    /// surface [`HostError::AlreadyStarted`] just like a double-start
    /// from `Running`; the one-shot service shape is intentional.
    Stopped,
    /// Transient placeholder used while transitioning between states.
    /// Never observed outside the locked critical section.
    Transitioning,
}

impl P2pService {
    /// Construction entry point used by [`crate::DevnetHost::build`].
    /// Boxes the swarm so the enum stays size-balanced.
    pub(crate) fn new(
        options: HostOptions,
        peer_id: PeerId,
        swarm: Swarm<DevnetBehaviour>,
        bootnodes: Vec<Bootnode>,
    ) -> Self {
        Self {
            peer_id,
            state: Mutex::new(State::Idle {
                options,
                swarm: Box::new(swarm),
                bootnodes,
            }),
        }
    }

    /// Returns the local peer id (stable across the service lifetime).
    #[must_use]
    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    /// Returns a cheap clone-friendly [`Host`] handle while the service
    /// is `Running`. Outside of that lifecycle state the handle does
    /// not yet exist (before `start`) or is no longer valid (after
    /// `stop`).
    #[must_use]
    pub fn host(&self) -> Option<Host> {
        match &*self.state.lock() {
            State::Running { host, .. } => Some(host.clone()),
            _ => None,
        }
    }

    fn take_idle(&self) -> HostResult<(HostOptions, Box<Swarm<DevnetBehaviour>>, Vec<Bootnode>)> {
        let mut guard = self.state.lock();
        match std::mem::replace(&mut *guard, State::Transitioning) {
            State::Idle {
                options,
                swarm,
                bootnodes,
            } => Ok((options, swarm, bootnodes)),
            // Restore the original state before returning the error so
            // `start` is observably a no-op on the failure path.
            other => {
                *guard = other;
                Err(HostError::AlreadyStarted)
            }
        }
    }

    fn install_running(&self, host: Host, cancel: CancellationToken, join: JoinHandle<()>) {
        *self.state.lock() = State::Running { host, cancel, join };
    }

    fn restore_idle(
        &self,
        options: HostOptions,
        swarm: Box<Swarm<DevnetBehaviour>>,
        bootnodes: Vec<Bootnode>,
    ) {
        *self.state.lock() = State::Idle {
            options,
            swarm,
            bootnodes,
        };
    }

    fn take_running(&self) -> Option<(CancellationToken, JoinHandle<()>, Host)> {
        let mut guard = self.state.lock();
        match std::mem::replace(&mut *guard, State::Transitioning) {
            State::Running { host, cancel, join } => {
                *guard = State::Stopped;
                Some((cancel, join, host))
            }
            other => {
                *guard = other;
                None
            }
        }
    }
}

#[async_trait]
impl Service for P2pService {
    fn name(&self) -> &'static str {
        "runtime-p2p"
    }

    #[instrument(name = "p2p.start", skip(self), fields(peer_id = %self.peer_id))]
    async fn start(&self) -> anyhow::Result<()> {
        let (options, mut swarm, bootnodes) = self.take_idle()?;
        let listen_addr = options.listen_addr().as_multiaddr().clone();

        match bind(&mut swarm, listen_addr.clone()).await {
            Ok(bound_addr) => {
                info!(%bound_addr, "host listener up");
            }
            Err(err) => {
                self.restore_idle(options, swarm, bootnodes);
                return Err(err.into());
            }
        }

        for bootnode in &bootnodes {
            // Dial errors at this stage are non-fatal — peers may come
            // up later; the swarm-poll task will receive and surface
            // the eventual `OutgoingConnectionError`.
            if let Err(err) = swarm.dial(bootnode.addr.clone()) {
                warn!(
                    peer = %bootnode.peer_id,
                    addr = %bootnode.addr,
                    %err,
                    "bootnode dial dispatch failed",
                );
            }
        }

        let (commands_tx, commands_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
        let host = Host::new(self.peer_id, commands_tx);
        let cancel = CancellationToken::new();
        let join = tokio::spawn(swarm_task(*swarm, commands_rx, cancel.clone()));

        self.install_running(host, cancel, join);
        Ok(())
    }

    #[instrument(name = "p2p.stop", skip_all, fields(peer_id = %self.peer_id))]
    async fn stop(&self, cancel: CancellationToken) -> anyhow::Result<()> {
        let Some((task_cancel, join, host)) = self.take_running() else {
            debug!("stop called on non-running service");
            return Ok(());
        };

        // Best-effort: surface shutdown to the swarm task even if the
        // channel is already closed (e.g. the task panicked).
        let _ = host.commands().send(HostCommand::Shutdown).await;
        task_cancel.cancel();

        // Bound the join on the caller-supplied cancellation token —
        // when it fires we abort the task and return.
        tokio::select! {
            res = join => {
                if let Err(err) = res {
                    if err.is_panic() {
                        return Err(anyhow!("p2p swarm task panicked: {err}"));
                    }
                    debug!(%err, "swarm task already cancelled");
                }
                Ok(())
            }
            () = cancel.cancelled() => {
                warn!("shutdown cancel fired before swarm task drained");
                Ok(())
            }
        }
    }

    async fn status(&self) -> anyhow::Result<()> {
        match &*self.state.lock() {
            State::Running { join, .. } if join.is_finished() => {
                Err(anyhow!("p2p swarm task exited unexpectedly"))
            }
            State::Running { .. } => Ok(()),
            State::Idle { .. } => Err(anyhow!("p2p service not started")),
            State::Stopped => Err(anyhow!("p2p service stopped")),
            State::Transitioning => Err(anyhow!("p2p service mid-transition")),
        }
    }
}

/// Calls `Swarm::listen_on` and races a deadline against the swarm's
/// first listener event. Returns the bound multiaddr on success.
async fn bind(swarm: &mut Swarm<DevnetBehaviour>, addr: Multiaddr) -> HostResult<Multiaddr> {
    let listener_id = swarm
        .listen_on(addr.clone())
        .map_err(|err| bind_err(addr.clone(), err.to_string()))?;
    let bind_deadline = sleep(BIND_DEADLINE);
    tokio::pin!(bind_deadline);
    loop {
        tokio::select! {
            () = &mut bind_deadline => {
                return Err(bind_err(
                    addr,
                    format!("listener did not become ready within {BIND_DEADLINE:?}"),
                ));
            }
            Some(event) = swarm.next() => {
                match event {
                    SwarmEvent::NewListenAddr { listener_id: id, address }
                        if id == listener_id =>
                    {
                        return Ok(address);
                    }
                    SwarmEvent::ListenerClosed { listener_id: id, reason: Err(err), .. }
                        if id == listener_id =>
                    {
                        return Err(bind_err(addr, err.to_string()));
                    }
                    SwarmEvent::ListenerError { listener_id: id, error }
                        if id == listener_id =>
                    {
                        return Err(bind_err(addr, error.to_string()));
                    }
                    other => {
                        debug!(?other, "swarm event during bind handshake");
                    }
                }
            }
        }
    }
}

fn bind_err(addr: Multiaddr, reason: impl Into<String>) -> HostError {
    HostError::Bind {
        addr,
        reason: reason.into(),
    }
}

/// Long-running swarm-poll task. Owns the `Swarm` and drives it until
/// the cancellation token fires or the command channel closes.
async fn swarm_task(
    mut swarm: Swarm<DevnetBehaviour>,
    mut commands: mpsc::Receiver<HostCommand>,
    cancel: CancellationToken,
) {
    info!(local_peer = %swarm.local_peer_id(), "p2p swarm-poll task up");
    // Pin once outside the loop: the cancellation future is monotonic
    // (once resolved, stays resolved), so reusing it across iterations
    // avoids constructing a fresh waker registration each poll.
    let cancelled = cancel.cancelled();
    tokio::pin!(cancelled);
    loop {
        // `biased` polls arms in source order, skipping the per-poll
        // RNG that `tokio::select!` uses for fairness. Cancel first
        // (fastest shutdown response), commands second (rare admin
        // events), swarm events third (the steady-state firehose).
        tokio::select! {
            biased;
            () = &mut cancelled => {
                debug!("swarm task observed cancellation");
                break;
            }
            command = commands.recv() => {
                match command {
                    Some(HostCommand::Shutdown) | None => break,
                }
            }
            Some(event) = swarm.next() => {
                handle_swarm_event(event);
            }
        }
    }
    info!("p2p swarm-poll task down");
}

#[allow(clippy::needless_pass_by_value)]
fn handle_swarm_event(event: SwarmEvent<crate::host::behaviour::DevnetBehaviourEvent>) {
    // Event dispatch logic for gossip / req-resp / identify lands in
    // later additions. For host-construction scope, structured
    // tracing is enough.
    match event {
        SwarmEvent::Behaviour(inner) => debug!(?inner, "behaviour event"),
        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
            debug!(peer = %peer_id, "connection established");
        }
        SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
            debug!(peer = %peer_id, ?cause, "connection closed");
        }
        SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
            warn!(peer = ?peer_id, %error, "outgoing connection error");
        }
        SwarmEvent::IncomingConnectionError { error, .. } => {
            warn!(%error, "incoming connection error");
        }
        SwarmEvent::ListenerError { error, .. } => {
            error!(%error, "listener error");
        }
        other => debug!(?other, "swarm event"),
    }
}

// Compile-time witness that we wired the same peer-id channel both
// inside the lifecycle and out through the `Host` handle.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::devnet::DevnetHost;
    use crate::options::HostOptions;
    use std::path::Path;
    use tempfile::tempdir;

    fn options_in(dir: &Path) -> HostOptions {
        HostOptions::try_new(
            "/ip4/127.0.0.1/udp/0/quic-v1",
            "test/0.1.0",
            &dir.join("id"),
            None,
        )
        .unwrap()
    }

    fn build_service() -> (tempfile::TempDir, P2pService) {
        let dir = tempdir().unwrap();
        let service = DevnetHost::build(options_in(dir.path())).unwrap();
        (dir, service)
    }

    #[tokio::test]
    async fn double_start_returns_already_started() {
        let (_dir, service) = build_service();
        service.start().await.unwrap();
        let err = service.start().await.unwrap_err();
        let downcast = err
            .downcast::<HostError>()
            .expect("AlreadyStarted should round-trip through anyhow");
        assert!(matches!(downcast, HostError::AlreadyStarted));
        service.stop(CancellationToken::new()).await.unwrap();
    }

    #[tokio::test]
    async fn stop_on_idle_is_noop() {
        let (_dir, service) = build_service();
        service.stop(CancellationToken::new()).await.unwrap();
    }

    #[tokio::test]
    async fn host_handle_available_only_while_running() {
        let (_dir, service) = build_service();
        assert!(service.host().is_none());
        service.start().await.unwrap();
        assert!(service.host().is_some());
        service.stop(CancellationToken::new()).await.unwrap();
        assert!(service.host().is_none());
    }
}
