//! Node-local bridge from decoded p2p gossip into the chain service.
//!
//! `runtime-p2p` owns wire decoding and exposes typed one-shot receivers.
//! `lean-chain` owns validation and persistence. This service is the
//! composition-layer glue that drains those receivers while the node is
//! running.

use std::sync::Arc;

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use engine::{AttestationImportResult, BlockImportResult};
use lean_chain::Service as ChainService;
use lean_core::Service;
use parking_lot::Mutex;
use runtime_p2p::{BlockReceiver, P2pService, VoteReceiver};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};

pub(crate) struct GossipIngestService {
    p2p: Arc<P2pService>,
    chain: Arc<ChainService>,
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    run: Option<RunHandle>,
    last_err: Option<String>,
}

struct RunHandle {
    block_task: JoinHandle<()>,
    vote_task: JoinHandle<()>,
    cancel: CancellationToken,
}

impl GossipIngestService {
    pub(crate) fn new(p2p: Arc<P2pService>, chain: Arc<ChainService>) -> Self {
        Self {
            p2p,
            chain,
            state: Arc::new(Mutex::new(State::default())),
        }
    }
}

impl Drop for GossipIngestService {
    fn drop(&mut self) {
        if let Some(run) = self.state.lock().run.take() {
            run.cancel.cancel();
        }
    }
}

#[async_trait]
impl Service for GossipIngestService {
    fn name(&self) -> &'static str {
        "gossip-ingest"
    }

    #[instrument(level = "info", name = "gossip_ingest.start", skip_all, err)]
    async fn start(&self) -> anyhow::Result<()> {
        let mut state = self.state.lock();
        if state.run.is_some() {
            return Err(anyhow!("gossip ingest service is already running"));
        }

        let block_rx = self
            .p2p
            .take_block_receiver()
            .ok_or_else(|| anyhow!("p2p block gossip receiver is unavailable"))?;
        let vote_rx = self
            .p2p
            .take_vote_receiver()
            .ok_or_else(|| anyhow!("p2p vote gossip receiver is unavailable"))?;

        state.last_err = None;
        let cancel = CancellationToken::new();
        let block_task = tokio::spawn(drain_blocks(
            block_rx,
            Arc::clone(&self.chain),
            cancel.clone(),
            Arc::clone(&self.state),
        ));
        let vote_task = tokio::spawn(drain_votes(
            vote_rx,
            Arc::clone(&self.chain),
            cancel.clone(),
            Arc::clone(&self.state),
        ));
        state.run = Some(RunHandle {
            block_task,
            vote_task,
            cancel,
        });
        Ok(())
    }

    #[instrument(level = "info", name = "gossip_ingest.stop", skip_all, err)]
    async fn stop(&self, cancel: CancellationToken) -> anyhow::Result<()> {
        let Some(RunHandle {
            block_task,
            vote_task,
            cancel: own_cancel,
        }) = self.state.lock().run.take()
        else {
            return Ok(());
        };
        own_cancel.cancel();

        let mut result = Ok(());
        for (name, task) in [("block", block_task), ("vote", vote_task)] {
            if let Err(err) = join_task(name, task, &cancel).await {
                result = result.and(Err(err));
            }
        }
        result
    }

    async fn status(&self) -> anyhow::Result<()> {
        let state = self.state.lock();
        match state.run.as_ref() {
            None => Err(anyhow!("gossip ingest service is not running")),
            Some(run) if run.block_task.is_finished() => {
                Err(anyhow!("gossip ingest block task exited prematurely"))
            }
            Some(run) if run.vote_task.is_finished() => {
                Err(anyhow!("gossip ingest vote task exited prematurely"))
            }
            Some(_) => match state.last_err.as_ref() {
                Some(err) => Err(anyhow!("gossip ingest last error: {err}")),
                None => Ok(()),
            },
        }
    }
}

async fn join_task(
    name: &'static str,
    mut task: JoinHandle<()>,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    tokio::select! {
        biased;
        () = cancel.cancelled() => {
            task.abort();
            let _ = task.await;
            Err(anyhow!("gossip ingest {name} task did not stop within shutdown budget"))
        }
        join = &mut task => {
            join.with_context(|| format!("gossip ingest {name} task panicked"))?;
            Ok(())
        }
    }
}

async fn drain_blocks(
    mut rx: BlockReceiver,
    chain: Arc<ChainService>,
    cancel: CancellationToken,
    state: Arc<Mutex<State>>,
) {
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            maybe_block = rx.recv() => {
                let Some(block) = maybe_block else {
                    debug!("block gossip receiver closed");
                    break;
                };
                let slot = block.message.slot;
                let proposer = block.message.proposer_index;
                match chain.import_block(block).await {
                    Ok(outcome) => log_block_outcome(slot, proposer, &outcome),
                    Err(err) => {
                        warn!(slot = slot.get(), proposer = proposer.get(), %err, "gossip block import failed");
                        state.lock().last_err = Some(format!("block import failed: {err}"));
                    }
                }
            }
        }
    }
}

async fn drain_votes(
    mut rx: VoteReceiver,
    chain: Arc<ChainService>,
    cancel: CancellationToken,
    state: Arc<Mutex<State>>,
) {
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            maybe_vote = rx.recv() => {
                let Some(vote) = maybe_vote else {
                    debug!("vote gossip receiver closed");
                    break;
                };
                let slot = vote.message.slot;
                let validator = vote.validator_id;
                match chain.import_attestation(vote).await {
                    Ok(outcome) => log_vote_outcome(slot, validator, &outcome),
                    Err(err) => {
                        warn!(slot = slot.get(), validator = validator.get(), %err, "gossip vote import failed");
                        state.lock().last_err = Some(format!("vote import failed: {err}"));
                    }
                }
            }
        }
    }
}

fn log_block_outcome(
    slot: protocol::Slot,
    proposer: protocol::ValidatorIndex,
    outcome: &BlockImportResult,
) {
    match outcome {
        BlockImportResult::Accepted {
            block_root,
            head_root,
            ..
        } => info!(
            slot = slot.get(),
            proposer = proposer.get(),
            block_root = %block_root.to_hex(),
            head_root = %head_root.to_hex(),
            "gossip block accepted",
        ),
        BlockImportResult::Rejected { error, .. } => {
            debug!(
                slot = slot.get(),
                proposer = proposer.get(),
                %error,
                "gossip block rejected",
            );
        }
        _ => debug!(
            slot = slot.get(),
            proposer = proposer.get(),
            ?outcome,
            "gossip block import outcome",
        ),
    }
}

fn log_vote_outcome(
    slot: protocol::Slot,
    validator: protocol::ValidatorIndex,
    outcome: &AttestationImportResult,
) {
    match outcome {
        AttestationImportResult::Accepted {
            validator_id,
            head_root,
            ..
        } => debug!(
            slot = slot.get(),
            validator = validator_id.get(),
            head_root = %head_root.to_hex(),
            "gossip vote accepted",
        ),
        AttestationImportResult::Rejected {
            validator_id,
            error,
        } => debug!(
            slot = slot.get(),
            validator = validator_id.get(),
            %error,
            "gossip vote rejected",
        ),
        _ => debug!(
            slot = slot.get(),
            validator = validator.get(),
            ?outcome,
            "gossip vote import outcome",
        ),
    }
}
