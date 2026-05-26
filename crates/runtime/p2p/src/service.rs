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

use std::{sync::Arc, time::Duration};

use anyhow::anyhow;
use async_trait::async_trait;
use futures::StreamExt;
use libp2p::{gossipsub, request_response, swarm::SwarmEvent, Multiaddr, PeerId, Swarm};
use parking_lot::Mutex;
use protocol::{SignedBlock, SignedVote};
use tokio::{sync::mpsc, task::JoinHandle, time::sleep};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument, warn};

use runtime_core::Service;

use crate::error::{HostError, HostResult};
use crate::gossip::{handler, BlockReceiver, Topic, VoteReceiver};
use crate::host::{
    behaviour::{DevnetBehaviour, DevnetBehaviourEvent, RpcRequest, RpcResponse},
    bootnodes::Bootnode,
    Host, HostCommand, COMMAND_CHANNEL_CAPACITY,
};
use crate::options::HostOptions;
use crate::rpc::{
    blocks_by_root as blocks_handler, outbound::OutboundTable, status as status_handler,
    RpcProvider, SharedRpcProvider,
};

/// Per-topic inbound channel capacity. Sized to absorb a brief burst of
/// gossip without blocking the swarm-poll task. `try_send` drops on
/// overflow; gossipsub mesh replay handles transient loss.
const GOSSIP_CHANNEL_CAPACITY: usize = 256;

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
    /// Pluggable provider that supplies the local `Status` and answers
    /// `BlocksByRoot` lookups. Cloned into the swarm-poll task at
    /// [`Service::start`] so request handlers can call it without
    /// touching `&self`.
    provider: SharedRpcProvider,
    /// One-shot inbound channel for decoded `SignedBlock` payloads.
    /// Populated by [`Service::start`]; consumed once via
    /// [`Self::take_block_receiver`].
    block_rx: Mutex<Option<BlockReceiver>>,
    /// One-shot inbound channel for decoded `SignedVote` payloads.
    /// Populated by [`Service::start`]; consumed once via
    /// [`Self::take_vote_receiver`].
    vote_rx: Mutex<Option<VoteReceiver>>,
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
        /// Actual multiaddr the listener bound to. Equal to the
        /// configured listen address when a concrete port was provided,
        /// or the OS-assigned port when the configured address used
        /// `udp/0`. Surfaced via [`P2pService::bound_addr`] so callers
        /// (notably the two-node integration tests) can dial the
        /// service without knowing the port up front.
        bound_addr: Multiaddr,
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
        provider: SharedRpcProvider,
    ) -> Self {
        Self {
            peer_id,
            state: Mutex::new(State::Idle {
                options,
                swarm: Box::new(swarm),
                bootnodes,
            }),
            provider,
            block_rx: Mutex::new(None),
            vote_rx: Mutex::new(None),
        }
    }

    /// Consumes the inbound block channel. Returns `Some` exactly once
    /// after [`Service::start`] has run; subsequent calls return `None`.
    /// Returns `None` before `start` and after the channel has already
    /// been taken.
    #[must_use]
    pub fn take_block_receiver(&self) -> Option<BlockReceiver> {
        self.block_rx.lock().take()
    }

    /// Consumes the inbound vote channel. Same one-shot semantics as
    /// [`Self::take_block_receiver`].
    #[must_use]
    pub fn take_vote_receiver(&self) -> Option<VoteReceiver> {
        self.vote_rx.lock().take()
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

    /// Returns the multiaddr the listener bound to while the service is
    /// `Running`. Equal to the configured listen address when a concrete
    /// port was provided; equal to the OS-assigned address when the
    /// configured address used `udp/0`. Outside the `Running` state the
    /// listener does not exist (before `start`) or has been torn down
    /// (after `stop`), so the method returns `None`.
    #[must_use]
    pub fn bound_addr(&self) -> Option<Multiaddr> {
        match &*self.state.lock() {
            State::Running { bound_addr, .. } => Some(bound_addr.clone()),
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

    fn install_running(
        &self,
        host: Host,
        cancel: CancellationToken,
        join: JoinHandle<()>,
        bound_addr: Multiaddr,
    ) {
        *self.state.lock() = State::Running {
            host,
            cancel,
            join,
            bound_addr,
        };
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
            State::Running {
                host,
                cancel,
                join,
                bound_addr: _,
            } => {
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

        info!(
            configured_addr = %listen_addr,
            peer_id = %self.peer_id,
            "starting p2p listener",
        );
        let bound_addr = match prepare(&mut swarm, listen_addr.clone()).await {
            Ok(addr) => addr,
            Err(err) => {
                self.restore_idle(options, swarm, bootnodes);
                return Err(err.into());
            }
        };
        info!(configured_addr = %listen_addr, %bound_addr, "host listener up");
        dial_bootnodes(&mut swarm, &bootnodes);

        let (commands_tx, commands_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
        let (block_tx, block_rx) = mpsc::channel::<SignedBlock>(GOSSIP_CHANNEL_CAPACITY);
        let (vote_tx, vote_rx) = mpsc::channel::<SignedVote>(GOSSIP_CHANNEL_CAPACITY);
        let host = Host::new(self.peer_id, commands_tx);
        let cancel = CancellationToken::new();
        let join = tokio::spawn(swarm_task(
            *swarm,
            commands_rx,
            cancel.clone(),
            block_tx,
            vote_tx,
            Arc::clone(&self.provider),
        ));

        *self.block_rx.lock() = Some(BlockReceiver::new(block_rx));
        *self.vote_rx.lock() = Some(VoteReceiver::new(vote_rx));
        self.install_running(host, cancel, join, bound_addr);
        Ok(())
    }

    #[instrument(name = "p2p.stop", skip_all, fields(peer_id = %self.peer_id))]
    async fn stop(&self, cancel: CancellationToken) -> anyhow::Result<()> {
        let Some((task_cancel, mut join, host)) = self.take_running() else {
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
            res = &mut join => {
                if let Err(err) = res {
                    if err.is_panic() {
                        return Err(anyhow!("p2p swarm task panicked: {err}"));
                    }
                    debug!(%err, "swarm task already cancelled");
                }
                Ok(())
            }
            () = cancel.cancelled() => {
                // Caller's deadline fired before the task drained on
                // its own. Abort to guarantee no orphaned swarm task
                // outlives `stop()`; a cooperative shutdown via
                // `task_cancel` may still be in flight, so the abort
                // is belt-and-suspenders.
                warn!("shutdown cancel fired before swarm task drained; aborting");
                join.abort();
                Ok(())
            }
        }
    }

    async fn status(&self) -> anyhow::Result<()> {
        match &*self.state.lock() {
            State::Running { join, .. } => {
                if join.is_finished() {
                    Err(anyhow!("p2p swarm task exited unexpectedly"))
                } else {
                    Ok(())
                }
            }
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
        .map_err(|err| bind_err(addr.clone(), err))?;
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
                        return Err(bind_err(addr, err));
                    }
                    SwarmEvent::ListenerError { listener_id: id, error }
                        if id == listener_id =>
                    {
                        return Err(bind_err(addr, error));
                    }
                    other => {
                        debug!(?other, "swarm event during bind handshake");
                    }
                }
            }
        }
    }
}

fn bind_err(addr: Multiaddr, reason: impl std::fmt::Display) -> HostError {
    HostError::Bind {
        addr,
        reason: reason.to_string(),
    }
}

/// Registers each bootnode's `peer-id ↔ multiaddr` mapping and fires a
/// best-effort dial.
///
/// Bootnode entries strip the trailing `/p2p/<peer-id>` component at
/// parse time, so libp2p has no other way to learn the mapping until
/// identify completes — which is too late for outbound RPC. We register
/// the address up front so any subsequent
/// `request_response::send_request(peer_id, _)` call that triggers an
/// implicit dial can resolve the peer.
///
/// Dial dispatch errors are non-fatal: peers may come up later, and the
/// swarm-poll task surfaces the eventual `OutgoingConnectionError`.
fn dial_bootnodes(swarm: &mut Swarm<DevnetBehaviour>, bootnodes: &[Bootnode]) {
    for bootnode in bootnodes {
        swarm.add_peer_address(bootnode.peer_id, bootnode.addr.clone());
        match swarm.dial(bootnode.addr.clone()) {
            Ok(()) => info!(
                peer = %bootnode.peer_id,
                addr = %bootnode.addr,
                "bootnode dial dispatched",
            ),
            Err(err) => {
                warn!(
                    peer = %bootnode.peer_id,
                    addr = %bootnode.addr,
                    %err,
                    "bootnode dial dispatch failed",
                );
            }
        }
    }
}

/// Groups the fallible setup phases of [`Service::start`] (bind +
/// gossipsub subscribe) behind a single `?`-chained call so the caller
/// rolls state back to `Idle` from one place.
async fn prepare(
    swarm: &mut Swarm<DevnetBehaviour>,
    listen_addr: Multiaddr,
) -> HostResult<Multiaddr> {
    let bound = bind(swarm, listen_addr).await?;
    subscribe_topics(swarm)?;
    Ok(bound)
}

/// Subscribes the swarm's gossipsub behaviour to every [`Topic`] this
/// crate registers. Failure is surfaced as [`HostError::GossipSubscribe`]
/// so the caller (`Service::start`) can roll state back to `Idle`.
fn subscribe_topics(swarm: &mut Swarm<DevnetBehaviour>) -> HostResult<()> {
    for topic in Topic::all() {
        swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&topic.ident())
            .map_err(|err| HostError::GossipSubscribe(err.to_string()))?;
        debug!(topic = %topic.as_str(), "subscribed to gossipsub topic");
    }
    Ok(())
}

/// Long-running swarm-poll task. Owns the `Swarm` and drives it until
/// the cancellation token fires or the command channel closes.
async fn swarm_task(
    mut swarm: Swarm<DevnetBehaviour>,
    mut commands: mpsc::Receiver<HostCommand>,
    cancel: CancellationToken,
    block_tx: mpsc::Sender<SignedBlock>,
    vote_tx: mpsc::Sender<SignedVote>,
    provider: SharedRpcProvider,
) {
    info!(local_peer = %swarm.local_peer_id(), "p2p swarm-poll task up");
    // Pin once outside the loop: the cancellation future is monotonic
    // (once resolved, stays resolved), so reusing it across iterations
    // avoids constructing a fresh waker registration each poll.
    let cancelled = cancel.cancelled();
    tokio::pin!(cancelled);
    // Outbound RPC correlation lives here — single-threaded inside the
    // swarm task, no locking.
    let mut outbound = OutboundTable::default();
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
                    Some(HostCommand::Publish { topic, payload, reply }) => {
                        let result = swarm
                            .behaviour_mut()
                            .gossipsub
                            .publish(topic, payload);
                        // The receiver may already have been dropped if
                        // the caller was cancelled — ignore the send
                        // error so the swarm task keeps running.
                        let _ = reply.send(result);
                    }
                    Some(HostCommand::SendRequest { peer, request, reply }) => {
                        // Route by variant: each protocol lives on its
                        // own `request_response::Behaviour` instance so
                        // multistream-select negotiates the correct
                        // wire protocol. Only the `BlocksByRoot` flow
                        // wires an oneshot reply today; `Status` is
                        // fire-and-validate from `initiate_status_handshake`
                        // and never reaches this command path.
                        let id = match &request {
                            RpcRequest::Status(_) => swarm
                                .behaviour_mut()
                                .status_rr
                                .send_request(&peer, request),
                            RpcRequest::BlocksByRoot(_) => swarm
                                .behaviour_mut()
                                .blocks_rr
                                .send_request(&peer, request),
                        };
                        outbound.insert(id, reply);
                    }
                }
            }
            Some(event) = swarm.next() => {
                handle_swarm_event(
                    event,
                    &mut swarm,
                    &block_tx,
                    &vote_tx,
                    &mut outbound,
                    provider.as_ref(),
                );
            }
        }
    }
    info!("p2p swarm-poll task down");
}

fn handle_swarm_event(
    event: SwarmEvent<DevnetBehaviourEvent>,
    swarm: &mut Swarm<DevnetBehaviour>,
    block_tx: &mpsc::Sender<SignedBlock>,
    vote_tx: &mpsc::Sender<SignedVote>,
    outbound: &mut OutboundTable,
    provider: &dyn RpcProvider,
) {
    match event {
        SwarmEvent::Behaviour(DevnetBehaviourEvent::Gossipsub(gossipsub::Event::Message {
            propagation_source,
            message_id,
            message,
        })) => {
            debug!(
                from = %propagation_source,
                id = %message_id,
                topic = %message.topic.as_str(),
                "gossipsub message received",
            );
            handler::route_gossipsub_message(&message_id, &message, block_tx, vote_tx);
        }
        SwarmEvent::Behaviour(DevnetBehaviourEvent::StatusRr(event)) => {
            handle_status_rr_event(event, swarm, provider);
        }
        SwarmEvent::Behaviour(DevnetBehaviourEvent::BlocksRr(event)) => {
            handle_blocks_rr_event(event, swarm, outbound, provider);
        }
        SwarmEvent::Behaviour(inner) => debug!(?inner, "behaviour event"),
        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
            info!(peer = %peer_id, "connection established");
            initiate_status_handshake(peer_id, swarm, provider);
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

/// Sends a Status request to a freshly-connected peer. Both sides
/// fire this on `ConnectionEstablished`; libp2p assigns distinct
/// `OutboundRequestId`s and substreams so the two outbound requests do
/// not collide.
fn initiate_status_handshake(
    peer_id: PeerId,
    swarm: &mut Swarm<DevnetBehaviour>,
    provider: &dyn RpcProvider,
) {
    debug!(peer = %peer_id, "initiating status handshake");
    let local = provider.local_status();
    let _ = swarm
        .behaviour_mut()
        .status_rr
        .send_request(&peer_id, RpcRequest::Status(local));
}

/// Handles events from the [`Status`-protocol] `request_response::Behaviour`.
/// Status is fire-and-validate: no oneshot caller awaits the response,
/// so outbound failures are logged but never surfaced through the
/// outbound table.
fn handle_status_rr_event(
    event: request_response::Event<RpcRequest, RpcResponse>,
    swarm: &mut Swarm<DevnetBehaviour>,
    provider: &dyn RpcProvider,
) {
    match event {
        request_response::Event::Message { peer, message, .. } => match message {
            request_response::Message::Request {
                request, channel, ..
            } => match request {
                RpcRequest::Status(s) => {
                    status_handler::on_inbound(peer, &s, channel, swarm, provider);
                }
                RpcRequest::BlocksByRoot(_) => {
                    log_wrong_variant(peer, "status", "blocks_by_root request");
                }
            },
            request_response::Message::Response { response, .. } => match response {
                RpcResponse::Status(s) => {
                    status_handler::on_outbound_response(peer, &s, swarm, provider);
                }
                RpcResponse::BlocksByRoot(_) => {
                    log_wrong_variant(peer, "status", "blocks_by_root response");
                }
            },
        },
        request_response::Event::OutboundFailure { peer, error, .. } => {
            log_status_outbound_failure(peer, &error);
        }
        request_response::Event::InboundFailure { peer, error, .. } => {
            log_rpc_failure(peer, &error, "status", "inbound");
        }
        request_response::Event::ResponseSent { .. } => {}
    }
}

/// Handles events from the [`BlocksByRoot`-protocol] `request_response::Behaviour`.
/// Outbound flows are paired with oneshot replies parked in
/// [`OutboundTable`]; failures fail the matching oneshot.
fn handle_blocks_rr_event(
    event: request_response::Event<RpcRequest, RpcResponse>,
    swarm: &mut Swarm<DevnetBehaviour>,
    outbound: &mut OutboundTable,
    provider: &dyn RpcProvider,
) {
    match event {
        request_response::Event::Message { peer, message, .. } => match message {
            request_response::Message::Request {
                request, channel, ..
            } => match request {
                RpcRequest::BlocksByRoot(r) => {
                    blocks_handler::on_inbound(peer, &r, channel, swarm, provider);
                }
                RpcRequest::Status(_) => {
                    log_wrong_variant(peer, "blocks_by_root", "status request");
                }
            },
            request_response::Message::Response {
                request_id,
                response,
            } => match response {
                RpcResponse::BlocksByRoot(_) => {
                    outbound.fulfill(request_id, response);
                }
                RpcResponse::Status(_) => {
                    log_wrong_variant(peer, "blocks_by_root", "status response");
                }
            },
        },
        request_response::Event::OutboundFailure {
            peer,
            request_id,
            error,
            ..
        } => {
            log_rpc_failure(peer, &error, "blocks_by_root", "outbound");
            outbound.fail(request_id, error.to_string());
        }
        request_response::Event::InboundFailure { peer, error, .. } => {
            log_rpc_failure(peer, &error, "blocks_by_root", "inbound");
        }
        request_response::Event::ResponseSent { .. } => {}
    }
}

/// Logs that a peer routed an `RpcRequest` / `RpcResponse` variant onto
/// the wrong `request_response::Behaviour`. Each behaviour negotiates
/// exactly one protocol (`STATUS_PROTOCOL_V1` /
/// `BLOCKS_BY_ROOT_PROTOCOL_V1`); seeing the other side's variant here
/// means the peer's codec mapped the variant to the wrong stream
/// protocol. We log and drop — libp2p surfaces an inbound failure to
/// the peer via the unconsumed `ResponseChannel`.
fn log_wrong_variant(peer: PeerId, on_protocol: &'static str, got: &'static str) {
    warn!(peer = %peer, "unexpected {got} on {on_protocol} protocol");
}

/// Logs an outbound status failure. Ream `master-0bceaee` interoperates
/// through gossip but does not answer this optional fire-and-validate request
/// in the local-pq two-node smoke, so a timeout is diagnostic noise rather
/// than a degraded node condition. Other status failures remain warnings.
fn log_status_outbound_failure(peer: PeerId, error: &request_response::OutboundFailure) {
    match error {
        request_response::OutboundFailure::Timeout => {
            debug!(
                peer = %peer,
                %error,
                "status rpc outbound timeout; peer did not answer optional status request",
            );
        }
        _ => log_rpc_failure(peer, error, "status", "outbound"),
    }
}

/// Logs a `request_response` outbound / inbound failure with a uniform
/// `"{protocol} rpc {direction} failure"` message, so log scraping is
/// consistent across the two protocols. `direction` is either
/// `"outbound"` or `"inbound"`.
fn log_rpc_failure(
    peer: PeerId,
    error: &dyn std::fmt::Display,
    protocol: &'static str,
    direction: &'static str,
) {
    warn!(peer = %peer, %error, "{protocol} rpc {direction} failure");
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

    #[tokio::test]
    async fn bound_addr_reflects_running_lifecycle_and_resolves_port() {
        use libp2p::multiaddr::Protocol;

        let (_dir, service) = build_service();
        assert!(service.bound_addr().is_none(), "none before start");

        service.start().await.unwrap();
        let bound = service.bound_addr().expect("some while running");

        // `udp/0` in the request must resolve to a concrete OS-assigned
        // port; the bound address must therefore expose a non-zero UDP
        // port component.
        let udp_port = bound.iter().find_map(|proto| match proto {
            Protocol::Udp(port) => Some(port),
            _ => None,
        });
        assert!(
            udp_port.is_some_and(|p| p != 0),
            "expected non-zero UDP port in bound addr, got {bound}",
        );

        service.stop(CancellationToken::new()).await.unwrap();
        assert!(service.bound_addr().is_none(), "none after stop");
    }
}
