//! Chain [`Service`] — the single engine writer.
//!
//! Wraps [`crate::chain::engine::Engine`] + [`storage::Store`] and exposes async
//! `import_block` / `import_attestation` / `produce_block` /
//! `produce_attestation` / `tick_interval`, each funnelling through the
//! engine mutex. The self-driving consensus loop (`node` crate) drives the
//! forkchoice clock via [`Service::tick_interval`].
//!
//! See [`Service::import_block`] for the storage / engine divergence
//! contract on persistence failure.
//!
//! # Concurrency model
//!
//! The engine `Mutex` is the sole write-serialization primitive: no derived
//! `RwLock` cache, no command channel. Every mutation and every read funnels
//! through it. Writers pay no post-write refresh; readers pay a microsecond
//! `Copy` under the same lock (and wait out any writer holding it).
//!
//! ```text
//!   WRITERS (latency-critical)              READERS (no deadline)
//!   ─────────────────────────               ─────────────────────
//!   ConsensusLoop (node task)               p2p swarm task
//!     ├ tick_interval                         └ local_status() ─ RPC handshake
//!     ├ produce_block / produce_attestation  /metrics scrape
//!     └ import_block / import_attestation      ├ lean_chain_slot
//!   sync Loop                                  ├ lean_chain_justified_slot
//!     └ import_block / import_attestation      └ lean_chain_finalized_slot
//!   gossip drain                            (each gauge samples its own
//!     └ import_*                             snapshot() — see below)
//!         │  &self, sync lock,                    │  snapshot() =
//!         │  guard never crosses .await           │  ChainSnapshot::from_engine
//!         ▼                                       ▼
//!   ┌──────────────────────────────────────────────────────────┐
//!   │                       ChainService                        │
//!   │   write ─►  engine : Mutex< forkchoice store >  ◄─ read   │
//!   │              lock() ─ STF + SSZ-HTR (hot path) ─ unlock    │
//!   │            store : Arc<dyn Store>   (persist plan)         │
//!   └──────────────────────────────────────────────────────────┘
//!        one lock · one funnel · writers serialize each other
//! ```
//!
//! Trade-off (accepted): a read takes the *same* lock, so it serializes
//! behind an in-progress writer for that writer's lock hold — a read is **not**
//! decoupled from writes. A read on a latency-sensitive task (the swarm loop's
//! [`Service::local_status`]) inherits the STF+HTR hold as tail latency, which
//! scales with future state / PQ cost. The three `/metrics` gauges each sample
//! their own [`Service::snapshot`], so one scrape spans three independent lock
//! acquisitions; a torn read is possible but the ordering invariant
//! `finalized <= justified <= current` holds under any interleaving.
//!
//! The deleted design decoupled reads through an `Arc<RwLock<ChainSnapshot>>`
//! cache refreshed after every write (eventually consistent); this commits to
//! one primitive instead.

// The engine `Mutex` is the sole write-serialization primitive. Deny
// `await_holding_lock` so any future edit that holds a lock guard across an
// `.await` (which would stall the tokio worker thread) fails the build. Note
// this lint only catches a guard held across `.await`; it does not catch a
// synchronous lock acquisition blocking an async worker — reads on the p2p
// swarm task serialize behind writers by design (see `Service::snapshot`).
#![deny(clippy::await_holding_lock)]

use std::sync::Arc;

use crate::chain::engine::{AttestationImportResult, BlockImportResult, Engine, PersistPlan};
use crate::duties::LocalSigner;
use async_trait::async_trait;
use lean_wire::Status;
use parking_lot::Mutex;
use protocol::{
    Attestation, BlockSignatures, BlockWithAttestation, Checkpoint, SignedAttestation,
    SignedBlockWithAttestation, Slot, ValidatorIndex,
};
use storage::HeadInfo;
use tokio_util::sync::CancellationToken;
use tracing::{debug, instrument, warn};
use types::Bytes32;

use super::cache::ChainSnapshot;
use super::error::ChainError;

/// Single-writer wrapper around [`Engine`] + [`storage::Store`].
///
/// # Concurrency
///
/// The engine `Mutex` is the **sole** write-serialization primitive. There
/// is no derived `RwLock` cache and no command channel. `import_block`,
/// `import_attestation`, `produce_block`, `produce_attestation`, and
/// `tick_interval` all serialize on that one lock; sync import and gossip
/// drain call the same methods and serialize on it too. Multiple callers
/// may invoke them concurrently — the engine is the funnel. Non-writer
/// readers capture a consistent [`ChainSnapshot`] on demand via
/// [`Self::snapshot`], which locks the engine, copies, and unlocks. Readers
/// therefore serialize behind an in-progress writer's lock hold; this is the
/// accepted trade-off (reads have no deadline) of committing to one primitive
/// instead of a derived cache.
pub struct Service {
    engine: Engine,
    store: Arc<dyn storage::Store>,
    /// Local validators' signer. Interior-mutable because `produce_*` take
    /// `&self` while `sign_attestation` needs `&mut LocalSigner` (the one-time-key
    /// index advances). The `parking_lot` guard is held for the sign call ONLY and
    /// dropped before any persist / `.await` — the crate `#![deny(clippy::await_holding_lock)]`
    /// (top of this file) makes a crossing edit fail the build.
    signer: Arc<Mutex<LocalSigner>>,
}

impl Service {
    /// Builds a NON-SIGNING service around `engine` and `store` — an observer
    /// with no local validators. `produce_block` / `produce_attestation` on such a
    /// service return [`ChainError::Sign`] ([`SignError::UnknownValidator`]) rather
    /// than a placeholder; a validating node uses [`Self::with_signer`] instead.
    #[must_use]
    pub fn new(engine: Engine, store: Arc<dyn storage::Store>) -> Self {
        Self::with_signer(engine, store, Arc::new(Mutex::new(LocalSigner::empty())))
    }

    /// Builds a SIGNING service: `signer` holds the local validators' secret keys
    /// (loaded by the composition root from the genesis key material). The signer
    /// is applied at this runtime boundary — never inside `forkchoice` or the
    /// engine store lock.
    #[must_use]
    pub fn with_signer(
        engine: Engine,
        store: Arc<dyn storage::Store>,
        signer: Arc<Mutex<LocalSigner>>,
    ) -> Self {
        Self {
            engine,
            store,
            signer,
        }
    }

    /// Captures a consistent [`ChainSnapshot`] under one engine-lock
    /// acquisition and returns it by value.
    ///
    /// Non-writer callers (`register_chain_gauges`, [`Self::local_status`])
    /// read through this instead of a derived cache. A read acquires the
    /// engine lock, copies the small `Copy` snapshot, and releases it, so it
    /// serializes behind any in-progress writer for that writer's lock hold
    /// (a state-transition on the write path). This is the accepted trade-off
    /// of the single-`Mutex` model: reads have no deadline, and dropping the
    /// cache removes the post-write refresh from every writer. Callers that
    /// run a read on a latency-sensitive task (e.g. the p2p swarm loop calling
    /// [`Self::local_status`]) inherit that write-hold as tail latency.
    ///
    /// Acquires the non-reentrant engine `parking_lot::Mutex`: never call this
    /// while already holding the engine lock, or the thread self-deadlocks. No
    /// current caller does.
    #[must_use]
    pub fn snapshot(&self) -> ChainSnapshot {
        ChainSnapshot::from_engine(&self.engine)
    }

    /// Imports `signed` through the engine. On [`BlockImportResult::Accepted`],
    /// persists the block, post-state, and head to storage.
    ///
    /// # Storage / engine divergence
    ///
    /// Persistence runs synchronously inside this call. If a `save_*`
    /// call fails after the engine has accepted the block, the engine
    /// in-memory state is ahead of storage: this method returns
    /// [`ChainError::Storage`] and the runtime cascade-stops. Recovery
    /// (replay-on-restart from the last persisted head) is tracked
    /// separately; it is intentionally out of scope here.
    ///
    /// # Errors
    /// - [`ChainError::Storage`] if any `save_*` call fails.
    /// - [`ChainError::PostStateMissing`] if the engine accepted the
    ///   block but the post-state has vanished by the time persistence
    ///   re-acquires the lock (engine invariant violation).
    #[instrument(level = "debug", skip_all, fields(slot = signed.message.block.slot.get()), err)]
    pub async fn import_block(
        &self,
        signed: SignedBlockWithAttestation,
    ) -> Result<BlockImportResult, ChainError> {
        let slot = signed.message.block.slot;
        // Import and capture the persist inputs under one engine-lock
        // acquisition, so no concurrent writer can shift the head/finalized
        // checkpoint between accept and capture.
        let (outcome, plan) = self.engine.import_block_capturing(signed);
        self.persist_accepted(slot, outcome, plan)
    }

    /// Sync-backfill import: SKIPS the import-boundary signature verify gate.
    ///
    /// The sync loop imports peer-provided blocks through this entry. Those
    /// blocks are hash-chained and STF-validated by the sync walk but are NOT
    /// signature-verified, and the sync trigger is peer-inducible — so this is a
    /// deliberate trust boundary, not "already-canonical" history. It is safe
    /// while no live verifier is wired (the gate is inert); the ingress must be
    /// closed (verify on sync, or bound the imported segment to a trusted
    /// finalized checkpoint) before the live verifier is activated. Live gossip
    /// uses [`Self::import_block`] (the verifying path).
    ///
    /// `pub(crate)`: only the in-crate self-sync loop may take the skip; no
    /// downstream crate can reach the verification-skipping path.
    ///
    /// # Errors
    /// Same as [`Self::import_block`].
    #[instrument(level = "debug", skip_all, fields(slot = signed.message.block.slot.get()), err)]
    pub(crate) async fn import_block_synced(
        &self,
        signed: SignedBlockWithAttestation,
    ) -> Result<BlockImportResult, ChainError> {
        let slot = signed.message.block.slot;
        let (outcome, plan) = self.engine.import_block_synced_capturing(signed);
        self.persist_accepted(slot, outcome, plan)
    }

    /// Persists the block on [`BlockImportResult::Accepted`]. Shared by
    /// [`Self::import_block`] and [`Self::import_block_synced`] so the two entry
    /// points cannot drift on the persist path.
    fn persist_accepted(
        &self,
        slot: Slot,
        outcome: BlockImportResult,
        plan: Option<PersistPlan>,
    ) -> Result<BlockImportResult, ChainError> {
        if let BlockImportResult::Accepted {
            block_root,
            head_root,
            ..
        } = &outcome
        {
            let plan = plan.ok_or(ChainError::PostStateMissing {
                block_root: *block_root,
            })?;
            self.persist_plan(plan)?;
            debug!(
                slot = slot.get(),
                block_root = %block_root.to_hex(),
                head_root = %head_root.to_hex(),
                "chain accepted block persisted",
            );
        }
        Ok(outcome)
    }

    /// Imports `signed` through the engine.
    ///
    /// # Errors
    /// This method is currently infallible at the infrastructure layer —
    /// the [`Result`] is preserved for symmetry with [`Self::import_block`]
    /// and to leave room for future side effects.
    #[instrument(level = "debug", skip_all, fields(validator = signed.message.validator_id.get()), err)]
    pub async fn import_attestation(
        &self,
        signed: SignedAttestation,
    ) -> Result<AttestationImportResult, ChainError> {
        let slot = signed.message.data.slot;
        let validator = signed.message.validator_id;
        let outcome = self.engine.import_attestation(signed);
        if let AttestationImportResult::Accepted { head_root, .. } = &outcome {
            debug!(
                slot = slot.get(),
                validator = validator.get(),
                head_root = %head_root.to_hex(),
                "chain accepted attestation applied",
            );
        }
        Ok(outcome)
    }

    /// Advances the forkchoice clock by one interval. `has_proposal` reflects
    /// whether this node produced a block in the current slot's proposal
    /// interval; the engine uses it to decide whether post-proposal votes are
    /// accepted this tick.
    ///
    /// Replaces the deleted background tick loop: the self-driving consensus
    /// loop (`node` crate) now calls this once per interval with a truthful
    /// `has_proposal`.
    ///
    /// # Errors
    /// [`ChainError::Engine`] if the engine rejects the tick.
    #[instrument(level = "debug", skip_all, fields(has_proposal), err)]
    pub async fn tick_interval(&self, has_proposal: bool) -> Result<(), ChainError> {
        // `tick_interval` locks the engine synchronously and returns before the
        // `.await` boundary; no lock guard crosses it.
        self.engine.tick_interval(has_proposal)?;
        Ok(())
    }

    /// Builds one locally authored block, signs the proposer's own attestation
    /// with a REAL leanSig signature (devnet-1: the proposer signs only its own
    /// attestation), and persists block + post-state + head to storage.
    ///
    /// The engine returns UNSIGNED output ([`Engine::produce_block_unsigned`]);
    /// signing happens HERE, at the runtime boundary, so the one-time-key `&mut`
    /// advance never runs under the engine store lock. The proposer's own vote is
    /// re-imported locally (as [`Self::produce_attestation`] does) so forkchoice
    /// counts it WITHOUT the attester pass re-signing the same slot — the
    /// consensus loop skips the proposer in its attest pass.
    ///
    /// The post-state guard is checked BEFORE signing so the unreachable
    /// [`ChainError::PostStateMissing`] path never burns an irreversible
    /// one-time-key index.
    ///
    /// # Errors
    /// - [`ChainError::Engine`] if [`Engine::produce_block_unsigned`] rejects the
    ///   request (unauthorized proposer, missing head state, etc.).
    /// - [`ChainError::PostStateMissing`] if the just-produced block's post-state
    ///   is absent (engine invariant violation).
    /// - [`ChainError::Sign`] if the proposer's key is missing or signing fails.
    /// - [`ChainError::Storage`] from the persist sweep.
    #[instrument(level = "debug", skip_all, fields(slot = slot.get(), validator = validator.get()), err)]
    pub async fn produce_block(
        &self,
        slot: Slot,
        validator: ValidatorIndex,
    ) -> Result<SignedBlockWithAttestation, ChainError> {
        // Produce (unsigned) + capture persist inputs under one engine-lock
        // acquisition; sign at the boundary after the lock is released.
        let prod = self.engine.produce_block_unsigned(slot, validator)?;
        // Check the post-state guard BEFORE signing: the unreachable
        // PostStateMissing path must not advance (burn) a one-time-key index.
        let inputs = prod.persist.ok_or(ChainError::PostStateMissing {
            block_root: prod.block_root,
        })?;
        // Devnet-1: sign ONLY the proposer's own attestation (block.py:108-115).
        let proposer_attestation = Attestation {
            validator_id: validator,
            data: prod.proposer_vote.vote,
        };
        let signature = {
            // Guard scope = the sign call only; dropped before the re-import /
            // persist below (no lock guard crosses `.await`).
            let mut signer = self.signer.lock();
            signer
                .sign_attestation(&proposer_attestation)
                .map_err(ChainError::Sign)?
        };
        // Re-import the proposer's OWN attestation into `latest_known_votes` so
        // forkchoice counts it. `Attestation` is `Copy`, so it stays available for
        // the block envelope below (NO `.clone()`); `signature` is non-`Copy`, so
        // its clone here is the one required clone for the re-imported envelope.
        let self_vote = SignedAttestation {
            message: proposer_attestation,
            signature: signature.clone(),
        };
        if let AttestationImportResult::Rejected { .. } = self.engine.import_attestation(self_vote)
        {
            warn!(
                slot = slot.get(),
                validator = validator.get(),
                "proposer self-vote re-import rejected (vote still carried by the block)",
            );
        }
        // Part 13 fills the proposer's single signature; positional-list assembly
        // (body attestations + proposer last) is a later part. `BlockSignatures`'
        // only constructor is `FromIterator<Signature>`.
        let block_signatures: BlockSignatures = core::iter::once(signature).collect();
        let signed = SignedBlockWithAttestation {
            message: BlockWithAttestation {
                block: prod.block,
                proposer_attestation,
            },
            signature: block_signatures,
        };
        let plan = PersistPlan::new(
            prod.block_root,
            inputs.head,
            inputs.finalized,
            inputs.post_state,
            signed.clone(),
        );
        self.persist_plan(plan)?;
        debug!(
            slot = slot.get(),
            validator = validator.get(),
            block_root = %prod.block_root.to_hex(),
            "chain produced block persisted",
        );
        Ok(signed)
    }

    /// Builds one locally authored attestation via
    /// [`Engine::produce_attestation_vote`], signs it with a REAL leanSig
    /// signature over `hash_tree_root(attestation)` at epoch = `data.slot`, and
    /// re-imports the vote locally so it lands in the engine's
    /// `latest_known_votes` pool.
    ///
    /// The local re-import is load-bearing: without it, this validator's
    /// own attestations only reach peers via gossip, and the next produced
    /// block would omit them — quorum on a small devnet can stall. Mirrors
    /// the upstream chain-service fix for the same stall.
    ///
    /// # Errors
    /// - [`ChainError::Engine`] if [`Engine::produce_attestation_vote`]
    ///   rejects the request.
    /// - [`ChainError::Sign`] if the validator's key is missing or signing fails.
    #[instrument(level = "debug", skip_all, fields(slot = slot.get(), validator = validator.get()), err)]
    pub async fn produce_attestation(
        &self,
        slot: Slot,
        validator: ValidatorIndex,
    ) -> Result<SignedAttestation, ChainError> {
        let produced = self.engine.produce_attestation_vote(slot)?;
        let message = Attestation {
            validator_id: validator,
            data: produced.vote,
        };
        // Sign at the boundary. Guard scope = the sign call only; dropped before
        // the engine re-import below (no lock guard crosses `.await`).
        let signature = {
            let mut signer = self.signer.lock();
            signer
                .sign_attestation(&message)
                .map_err(ChainError::Sign)?
        };
        let signed = SignedAttestation { message, signature };
        // Best-effort re-import: when `latest_justified` is still the
        // zero-sentinel (e.g. fresh anchor before the first justified
        // checkpoint), the produced vote's source.root is unresolvable
        // and the engine returns `Rejected`. The upstream client behaves the
        // same and warn-logs; we mirror that and continue.
        let outcome = self.engine.import_attestation(signed.clone());
        match &outcome {
            AttestationImportResult::Accepted { head_root, .. } => {
                debug!(
                    slot = slot.get(),
                    validator = validator.get(),
                    head_root = %head_root.to_hex(),
                    "chain own attestation reimported",
                );
            }
            AttestationImportResult::Rejected { .. } => {
                warn!(
                    ?outcome,
                    slot = slot.get(),
                    validator = validator.get(),
                    "own-attestation re-import rejected (vote still propagates to peers)",
                );
            }
            _ => {
                debug!(
                    ?outcome,
                    slot = slot.get(),
                    validator = validator.get(),
                    "own-attestation re-import outcome",
                );
            }
        }
        Ok(signed)
    }

    /// Returns the local node's current [`Status`] for the peer-handshake.
    ///
    /// Captured on demand under the engine lock via [`Self::snapshot`]. The
    /// value is a consistent single-lock read; the sync protocol tolerates a
    /// one-tick handshake lag.
    #[must_use]
    pub fn local_status(&self) -> Status {
        let snap = self.snapshot();
        let head = Checkpoint::new(snap.head_root, Slot::new(snap.current_slot));
        Status {
            finalized: snap.latest_finalized,
            head,
        }
    }

    /// Reports whether `root` is already known to local storage.
    ///
    /// # Errors
    /// [`ChainError::Storage`] when the backing store call fails.
    pub fn has_block(&self, root: &Bytes32) -> Result<bool, ChainError> {
        Ok(self.store.has_block(root)?)
    }

    /// Commits an engine-captured [`PersistPlan`] to storage.
    ///
    /// The plan was materialized atomically under the engine lock (head,
    /// post-state, and finalized checkpoint from one consistent snapshot), so
    /// this method only decomposes it and issues the single atomic
    /// [`storage::Store::save_accepted`] write: block + post-state + head
    /// commit together, and a mid-persist failure can never strand the head
    /// ahead of its block or state.
    fn persist_plan(&self, plan: PersistPlan) -> Result<(), ChainError> {
        let (block_root, block, post_state, head, finalized) = plan.into_parts();
        // The engine lock is already released here, so unwrapping the Arc (and
        // deep-cloning only if the store still shares it) happens off the hot
        // path — the under-lock cost was just the refcount bump in capture.
        self.store.save_accepted(
            block_root,
            block,
            Arc::unwrap_or_clone(post_state),
            HeadInfo::new(head, finalized),
        )?;
        Ok(())
    }
}

#[async_trait]
impl crate::core::Service for Service {
    fn name(&self) -> &'static str {
        "chain"
    }

    /// No-op: the chain service no longer owns a driving loop. The
    /// self-driving consensus loop (`node` crate) advances the engine via
    /// [`Service::tick_interval`]; the chain service only funnels engine
    /// mutations under the single writer lock.
    async fn start(&self) -> anyhow::Result<()> {
        Ok(())
    }

    /// No-op: nothing to tear down (no owned task).
    async fn stop(&self, _cancel: CancellationToken) -> anyhow::Result<()> {
        Ok(())
    }

    /// Always healthy: the chain service is a passive engine funnel with no
    /// background task to observe.
    async fn status(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

// The former `sync::Chain` / `duties::Chain` port traits collapsed to this
// concrete type: `sync::Loop` and `node::ConsensusLoop` drive the service
// directly through its concrete async API (`import_*`, `produce_*`,
// `tick_interval`) rather than through a trait adapter.
